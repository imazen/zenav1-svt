//! C PD1 depth refinement + inter-depth partition decision for the
//! still/420 funnel path (allintra presets 4..=5, `dr_mode = 1` =
//! PD0_DEPTH_ADAPTIVE).
//!
//! At M6+ the depth refinement mode is PD0_DEPTH_PRED_PART_ONLY
//! (`pred_depth_only`): PD1 codes exactly the PD0 tree, which is the
//! existing `encode_fixed_tree` path. At M0..M5 (`dr_mode = 1`,
//! enc_mode_config.c `set_block_based_depth_refinement_controls` cases
//! 6/9 — the M5DBG CFG dump fields dr_*), PD1 re-decides depths around
//! the PD0 prediction:
//!
//! 1. `perform_pred_depth_refinement` (enc_dec_process.c:1985) walks the
//!    PD0 `pc_tree` and, per PD0 leaf, admits parent (s_depth = -1) and/or
//!    child (e_depth = 1) depths via cost-deviation gates over the PD0
//!    PART_N costs (`set_start_end_depth` :1787,
//!    `is_parent_to_current_deviation_small` :1650,
//!    `is_child_to_current_deviation_small` :1709,
//!    `update_pred_th_offset` :1545). s2/e2 = 255 map to MIN_SIGNED, so
//!    at most ONE depth either side is ever admitted at M4/M5.
//! 2. `svt_aom_pick_partition` (product_coding_loop.c:11549) walks the
//!    refined scan: `test_depth` (:11396) evaluates the PART_N funnel
//!    block + its partition rate at the REAL left/above partition
//!    contexts (`update_part_neighs` :11225, `svt_aom_partition_rate_cost`
//!    rd_cost.c:1834), `test_split_partition` (:11304) recurses the
//!    children with per-quadrant early exits and picks split vs parent by
//!    `parent_cost_bias(995) * parent_rd <= split_cost * 1000`.
//!    `use_accurate_part_ctx = 1` at M4/M5 (capture acc_part=1) so the
//!    SPLIT rate is NOT doubled.
//!
//! Commit discipline: C evaluates the parent depth first (no neighbour
//! commit), then each split quadrant commits its winning subtree as it
//! resolves (`md_update_all_neighbour_arrays_multiple` for `mds->index
//! < 3`; the 4th quadrant defers to the compare); when the parent wins,
//! its commit overwrites the children's writes completely (every
//! neighbour-array/recon write spans exactly the block). We commit each
//! quadrant eagerly and overwrite on a parent win — state-equivalent:
//! nothing reads between the 4th quadrant's resolve and the winner
//! commit, and the parent commit covers the union of the children's
//! spans.
//!
//! `depths_qp_based_th_scaling = 0` for allintra <= M6
//! (enc_handle.c set_qp_based_th_scaling_ctrls_all_intra), so every
//! refinement threshold is used RAW (the 255 sentinels still map to
//! MIN_SIGNED).

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::leaf_funnel::{FunnelCtx, LeafEval, commit_leaf, evaluate_leaf};
use crate::partition::{BlockDecision, PartitionTree, PartitionType};
use crate::pd0::{M6Pd0Tables, Pd0Eval};

/// C `RDCOST` (rd_cost.h:36).
#[inline]
fn rdcost(lambda: u64, rate: u64, dist: u64) -> u64 {
    ((rate * lambda + 256) >> 9) + (dist << 7)
}

// ---------------------------------------------------------------------------
// Depth refinement controls (C DepthRefinementCtrls)
// ---------------------------------------------------------------------------

/// `set_block_based_depth_refinement_controls` levels 6 (M4) / 9 (M5),
/// verified against both the C source and the M5DBG CFG dump (dr_*
/// fields, docs/captures/m0m5_config_dlf.txt). s2/e2 = 255 → the second
/// tier is MIN_SIGNED (always passes), so s ∈ {0,-1}, e ∈ {0,1}.
#[derive(Clone, Copy, Debug)]
pub struct DrCtrls {
    /// PD0_DEPTH_ADAPTIVE (M0..M5). false = PD0_DEPTH_PRED_PART_ONLY
    /// (M6+): s = e = 0 everywhere, the walk degenerates to the PD0 tree.
    pub adaptive: bool,
    /// `s1_parent_to_current_th` (M4: 15, M5: 10).
    pub s1_th: i64,
    /// `e1_sub_to_current_th` (M4: 15, M5: 10).
    pub e1_th: i64,
    /// `parent_max_cost_th_mult` (M4: 10, M5: 0).
    pub parent_max_cost_mult: u64,
    /// `cost_band_based_modulation` (M4: 0, M5: 1).
    pub band_mod: bool,
    /// `max_cost_multiplier` (M5: 400).
    pub max_cost_multiplier: u64,
    /// `max_band_cnt` (M5: 4).
    pub max_band_cnt: u64,
    /// `decrement_per_band` (M5: [MAX, MAX, 10, 5]); i64::MAX = the
    /// C MAX_SIGNED_VALUE sentinel (band forces s = e = 0).
    pub decrement_per_band: [i64; 4],
    /// `lower_depth_split_cost_th` (M4: 20, M5: 100).
    pub lower_split_th: u64,
    /// `split_rate_th` (M4: 10, M5: 5); +20 applied at use (CLN_PD0,
    /// enc_dec_process.c:1598).
    pub split_rate_th: u64,
    /// `limit_max_min_to_pd0` (1 at both).
    pub limit_to_pd0: usize,
    /// `pd0_unavail_mode_depth` (M4: 2, M5: 0).
    pub unavail_mode: u8,
    /// `ctx->disallow_4x4` (svt_aom_get_disallow_4x4_allintra,
    /// enc_mode_config.c:11638: <= M3 -> false). Gates the e-depth caps
    /// (set_start_end_depth :1811) and the refined-scan child marking.
    pub disallow_4x4: bool,
}

impl DrCtrls {
    /// Per-preset derivation: `set_depth_ctrls` level from the capture
    /// (depth_ref_lvl 6 at M0-M4, 9 at M5, 10 at M6+ = PRED_PART_ONLY).
    pub fn for_preset(preset: u8) -> Self {
        match preset {
            0..=4 => DrCtrls {
                adaptive: true,
                s1_th: 15,
                e1_th: 15,
                parent_max_cost_mult: 10,
                band_mod: false,
                max_cost_multiplier: 0,
                max_band_cnt: 1,
                decrement_per_band: [0; 4],
                lower_split_th: 20,
                split_rate_th: 10,
                limit_to_pd0: 1,
                unavail_mode: 2,
                disallow_4x4: preset >= 4,
            },
            5 => DrCtrls {
                adaptive: true,
                s1_th: 10,
                e1_th: 10,
                parent_max_cost_mult: 0,
                band_mod: true,
                max_cost_multiplier: 400,
                max_band_cnt: 4,
                decrement_per_band: [i64::MAX, i64::MAX, 10, 5],
                lower_split_th: 100,
                split_rate_th: 5,
                limit_to_pd0: 1,
                unavail_mode: 0,
                disallow_4x4: true,
            },
            _ => DrCtrls {
                adaptive: false,
                s1_th: 0,
                e1_th: 0,
                parent_max_cost_mult: 0,
                band_mod: false,
                max_cost_multiplier: 0,
                max_band_cnt: 1,
                decrement_per_band: [0; 4],
                lower_split_th: 0,
                split_rate_th: 0,
                limit_to_pd0: 0,
                unavail_mode: 0,
                disallow_4x4: true,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Refined depth scan (C MdScan marks after perform_pred_depth_refinement)
// ---------------------------------------------------------------------------

/// One square node of the refined scan.
#[derive(Debug, Clone)]
pub struct RefScan {
    pub sq: usize,
    /// C `mds->tot_shapes == 1`: evaluate PART_N at this node.
    pub test_this: bool,
    /// C `mds->split_flag`: recurse into the children.
    pub split_flag: bool,
    pub children: Option<Box<[RefScan; 4]>>,
}

impl RefScan {
    fn leaf(sq: usize) -> Self {
        RefScan {
            sq,
            test_this: false,
            split_flag: false,
            children: None,
        }
    }

    /// C `set_child_to_be_tested` (enc_dec_process.c:1522): mark the
    /// child depth for evaluation (`disallow_4x4` blocks 8x8 -> 4x4).
    fn set_children_tested(&mut self, e_depth: i32, disallow_4x4: bool) {
        // disallow_4x4 blocks 8x8 -> 4x4; 4x4 never has children.
        if self.sq <= 4 || (disallow_4x4 && self.sq <= 8) {
            return;
        }
        self.split_flag = true;
        let half = self.sq / 2;
        let mut ch: [RefScan; 4] = [
            RefScan::leaf(half),
            RefScan::leaf(half),
            RefScan::leaf(half),
            RefScan::leaf(half),
        ];
        for c in ch.iter_mut() {
            c.test_this = true;
            if e_depth > 1 {
                c.set_children_tested(e_depth - 1, disallow_4x4);
            }
        }
        self.children = Some(Box::new(ch));
    }
}

/// Environment for the refinement gates: the chained rate tables supply
/// the ctx-0 PARTITION_SPLIT rates (`svt_aom_partition_rate_cost(.., 0,
/// 0)` — C passes zero partition contexts here, enc_dec_process.c:1585 /
/// :1613 / :1764).
struct RefineEnv<'a> {
    ctrls: &'a DrCtrls,
    lambda: u64,
    tables: &'a M6Pd0Tables,
    max_pd0: usize,
    min_pd0: usize,
}

/// C `update_pred_th_offset` (enc_dec_process.c:1545) + the deviation
/// gates, producing this PD0 leaf's admitted (s_depth, e_depth).
/// `parent` is the enclosing square's PD0 eval (None only for the SB
/// root, whose s is forced 0 by the max-size clamp anyway).
fn set_start_end_depth(
    env: &RefineEnv<'_>,
    node: &Pd0Eval,
    parent: Option<&Pd0Eval>,
    abs_x: usize,
    abs_y: usize,
) -> (i32, i32) {
    let ctrls = env.ctrls;
    if !ctrls.adaptive {
        return (0, 0);
    }
    let sq = node.sq;
    let mut s: i32 = -2;
    let mut e: i32 = 2;
    // 4x4 has no children; disallow_4x4 caps the sub-depths
    // (set_start_end_depth, enc_dec_process.c:1799-1813). With 4x4
    // allowed (M0-M3) only the 4x4-has-no-children cap applies.
    e = if ctrls.disallow_4x4 {
        match sq {
            4 | 8 => 0,
            16 => e.min(1),
            32 => e.min(2),
            _ => e,
        }
    } else {
        match sq {
            4 => 0,
            _ => e,
        }
    };
    // max_sq_size = 64 (max_block_size 64 below M8, I_SLICE cap 64,
    // default max_tx_size): :1835-1839.
    if sq == 64 {
        s = 0;
    } else if s == -2 && sq * 2 == 64 {
        s = -1;
    }

    let mut add_parent = true;
    let mut add_sub = true;
    if s != 0 || e != 0 {
        add_parent = false;
        add_sub = false;

        // limit_max_min_to_pd0 (:1846-1863).
        if ctrls.limit_to_pd0 != 0 && env.max_pd0 / env.min_pd0 > ctrls.limit_to_pd0 {
            if sq == env.max_pd0 {
                s = 0;
            }
            if sq == env.min_pd0 {
                e = 0;
            }
            if s == -2 && sq * 2 == env.max_pd0 {
                s = -1;
            }
            if e == 2 && sq / 2 == env.min_pd0 {
                e = 1;
            }
        }
        // coeff_lvl_modulation: dead on I-slices (:1866).

        let mut s_off: i64 = 0;
        let mut e_off: i64 = 0;
        // update_pred_th_offset (:1545): cost-band modulation (M5 only).
        if ctrls.band_mod {
            let max_cost = rdcost(env.lambda, 16, ctrls.max_cost_multiplier * (sq * sq) as u64);
            if node.tested && node.cost <= max_cost {
                let band_size = max_cost / ctrls.max_band_cnt;
                let band_idx = (node.cost / band_size) as usize;
                // cost == max_cost lands on band_idx == max_band_cnt; the
                // C ctrls array has no such slot (uninitialized read of a
                // zeroed struct field in practice) — treat as offset 0.
                if band_idx < 4 {
                    if ctrls.decrement_per_band[band_idx] == i64::MAX {
                        s = 0;
                        e = 0;
                    } else {
                        s_off = -ctrls.decrement_per_band[band_idx];
                        e_off = -ctrls.decrement_per_band[band_idx];
                    }
                }
            }
        }
        // lower_depth_split_cost_th (:1573-1592): drop the parent depth
        // when splitting the PARENT is very cheap relative to its cost.
        if s != 0 && ctrls.lower_split_th != 0 {
            if let Some(p) = parent {
                if p.tested {
                    let split_cost = rdcost(env.lambda, env.tables.split_bits(p.sq), 0);
                    if split_cost * 10000 < p.cost * ctrls.lower_split_th {
                        s = 0;
                    }
                }
            }
        }
        // split_rate_th (+20, CLN_PD0 :1594-1619): drop the child depth
        // when splitting THIS block is expensive relative to its cost.
        if ctrls.split_rate_th != 0 && node.tested {
            let th = ctrls.split_rate_th + 20;
            let split_cost = rdcost(env.lambda, env.tables.split_bits(sq), 0);
            if split_cost * 1000 > node.cost * th {
                e = 0;
            }
        }
        // use_ref_info: dead on I-slices (:1623).

        // is_parent_to_current_deviation_small (:1650): only called for
        // tested blocks below the SB size (:1876-1883).
        if s != 0 && node.tested && sq < 64 {
            match parent.filter(|p| p.tested) {
                Some(p) => {
                    // s1 used RAW + offset (the qp-scaling is disabled:
                    // depths_qp_based_th_scaling = 0 for allintra <= M6);
                    // s2 = 255 -> MIN_SIGNED (always passes).
                    let s1_th = ctrls.s1_th + s_off;
                    let max_cost = if ctrls.parent_max_cost_mult != 0 {
                        rdcost(
                            env.lambda,
                            18000 * ctrls.parent_max_cost_mult,
                            60 * ctrls.parent_max_cost_mult * (sq * sq) as u64 * 4,
                        )
                    } else {
                        0
                    };
                    let cur4 = (node.cost * 4).max(1) as i64;
                    let dev = ((p.cost.max(1) as i64) - cur4) * 100 / cur4;
                    if dev >= s1_th && p.cost >= max_cost {
                        s = 0;
                    } else {
                        // dev >= s2 (MIN_SIGNED) always.
                        s = -1;
                    }
                }
                None => {
                    // pd0_unavail_mode_depth (:1700-1706): 0 -> s = 0;
                    // 1 -> s = max(s, -1); 2 -> unchanged.
                    match ctrls.unavail_mode {
                        0 => s = 0,
                        1 => s = s.max(-1),
                        _ => {}
                    }
                }
            }
            if s != 0 {
                add_parent = true;
            }
        }

        // is_child_to_current_deviation_small (:1709): gated on tested +
        // sq > 4 (:1885-1892).
        if e != 0 && node.tested && sq > 4 {
            let tested_children: Vec<&Pd0Eval> = node
                .children
                .as_ref()
                .map(|ch| ch.iter().filter(|c| c.tested).collect())
                .unwrap_or_default();
            if !tested_children.is_empty() {
                // e1 qp-scaled with factors 1/1 (scaling disabled) + off;
                // e2 = 255 -> MIN_SIGNED.
                let e1_th = ctrls.e1_th + e_off;
                let sum: u64 = tested_children.iter().map(|c| c.cost).sum();
                let mut child_cost = (sum / tested_children.len() as u64) * 4;
                child_cost += rdcost(env.lambda, env.tables.split_bits(sq), 0);
                let cur = node.cost.max(1) as i64;
                let dev = ((child_cost.max(1) as i64) - cur) * 100 / cur;
                if dev >= e1_th {
                    e = 0;
                } else {
                    // dev >= e2 (MIN_SIGNED) always.
                    e = 1;
                }
            } else {
                match ctrls.unavail_mode {
                    0 => e = 0,
                    1 => e = e.min(1),
                    _ => {}
                }
            }
            if e != 0 {
                add_sub = true;
            }
        }
    }

    if nsqdbg_here(abs_x, abs_y) {
        let ch_costs: Vec<u64> = node
            .children
            .as_ref()
            .map(|ch| ch.iter().filter(|c| c.tested).map(|c| c.cost).collect())
            .unwrap_or_default();
        eprintln!(
            "NSQDBG REFINE mi=({},{}) sq={} tested={} cost={} pcost={} maxpd0={} minpd0={} sb={} psb={} ch={:?} s={} e={}",
            abs_y / 4,
            abs_x / 4,
            sq,
            u8::from(node.tested),
            node.cost,
            parent.map(|p| p.cost as i64).unwrap_or(-1),
            env.max_pd0,
            env.min_pd0,
            env.tables.split_bits(sq),
            parent.map(|p| env.tables.split_bits(p.sq) as i64).unwrap_or(-1),
            ch_costs,
            if add_parent { s } else { 0 },
            if add_sub { e } else { 0 },
        );
    }
    (if add_parent { s } else { 0 }, if add_sub { e } else { 0 })
}

/// C `refine_depth` (enc_dec_process.c:1901): walk the PD0 pc_tree and
/// build the refined MdScan marks. Returns the subtree's s_depth
/// propagation (parent-depth admissions bubble up: a SPLIT node whose
/// children admit their parent evaluates ITS PART_N, :1947-1953).
fn refine_depth(
    env: &RefineEnv<'_>,
    node: &Pd0Eval,
    parent: Option<&Pd0Eval>,
    abs_x: usize,
    abs_y: usize,
) -> (RefScan, i32) {
    let mut scan = RefScan::leaf(node.sq);
    if !node.split {
        scan.test_this = true;
        let (s, e) = set_start_end_depth(env, node, parent, abs_x, abs_y);
        if e > 0 {
            scan.set_children_tested(e, env.ctrls.disallow_4x4);
        }
        (scan, s)
    } else {
        let ch_evals = node.children.as_ref().expect("split children");
        let mut s_min = 0i32;
        let half = node.sq / 2;
        let mut ch: [RefScan; 4] = [
            RefScan::leaf(half),
            RefScan::leaf(half),
            RefScan::leaf(half),
            RefScan::leaf(half),
        ];
        for (i, cev) in ch_evals.iter().enumerate() {
            let (cs, s_child) = refine_depth(
                env,
                cev,
                Some(node),
                abs_x + (i & 1) * half,
                abs_y + (i >> 1) * half,
            );
            ch[i] = cs;
            s_min = s_min.min(s_child);
        }
        scan.split_flag = true;
        scan.children = Some(Box::new(ch));
        let mut s = s_min;
        // I-slice: blocks < 128 allowed (:1946).
        if s < 0 && node.sq < 128 {
            scan.test_this = true;
            s += 1;
        }
        (scan, s)
    }
}

/// C `perform_pred_depth_refinement` (enc_dec_process.c:1985).
pub(crate) fn build_refined_scan(
    root: &Pd0Eval,
    ctrls: &DrCtrls,
    lambda: u64,
    tables: &M6Pd0Tables,
) -> RefScan {
    build_refined_scan_at(root, ctrls, lambda, tables, 0, 0)
}

/// [`build_refined_scan`] with the SB's pixel origin, so the NSQDBG REFINE
/// dump (gated by SVTAV1_DBG_MI) can label nodes with absolute mi coords.
pub(crate) fn build_refined_scan_at(
    root: &Pd0Eval,
    ctrls: &DrCtrls,
    lambda: u64,
    tables: &M6Pd0Tables,
    sb_x: usize,
    sb_y: usize,
) -> RefScan {
    let mut max_pd0 = 0usize;
    let mut min_pd0 = 255usize;
    if ctrls.limit_to_pd0 != 0 {
        root.max_min_picked(&mut max_pd0, &mut min_pd0);
    } else {
        max_pd0 = 1;
        min_pd0 = 1;
    }
    let env = RefineEnv {
        ctrls,
        lambda,
        tables,
        max_pd0,
        min_pd0,
    };
    refine_depth(&env, root, None, sb_x, sb_y).0
}

// ---------------------------------------------------------------------------
// Partition rates at real contexts
// ---------------------------------------------------------------------------

/// `partition_fac_bits[PARTITION_CONTEXTS][..]` — per-row costs from a
/// (possibly chained) frame context's partition CDFs. Row layout matches
/// the writer: `bsl * 4 + (left*2 + above)`; rows 0..3 (8x8) carry 4
/// symbols, 4..15 carry 10 (64-SB frames never touch the 128 rows).
pub(crate) struct PartRates {
    rows: [[i32; 10]; 16],
}

impl PartRates {
    pub(crate) fn from_fc(fc: &svtav1_entropy::context::FrameContext) -> Self {
        let mut rows = [[0i32; 10]; 16];
        for (row, out) in rows.iter_mut().enumerate() {
            let nsyms = if row < 4 { 4 } else { 10 };
            crate::quant::syntax_rate_from_cdf(&mut out[..nsyms], &fc.partition_cdf[row]);
        }
        PartRates { rows }
    }

    /// `svt_aom_partition_rate_cost` (rd_cost.c:1834) for in-frame square
    /// blocks (has_rows && has_cols — 64-aligned frames only reach here):
    /// context row from the partition neighbour bytes.
    #[inline]
    pub(crate) fn bits(&self, ctx_row: usize, p: PartitionType) -> u64 {
        debug_assert!(ctx_row < 16);
        self.rows[ctx_row][p as usize] as u64
    }
}

// ---------------------------------------------------------------------------
// NSQ geometry + search controls (C NsqGeomCtrls / NsqSearchCtrls)
// ---------------------------------------------------------------------------

/// The still-funnel NSQ controls: geometry level 2 fields
/// (`svt_aom_set_nsq_geom_ctrls`, enc_mode_config.c:6408 — min_nsq 0,
/// allow_HV4 1, allow_HVA_HVB 0 at M0..M3) + the `set_nsq_search_ctrls`
/// (:6464) level fields after the tail adjustments:
/// `nsq_qp_based_th_scaling = 0` for allintra <= M3
/// (set_qp_based_th_scaling_ctrls_all_intra, enc_handle.c:4085) so
/// component/split thresholds stay RAW, and the unconditional
/// `max_part0_to_part1_dev -= 5` offset (:6797-6801, offset scaled by the
/// same disabled factors).
///
/// The runtime values were capture-verified per cell (NSQCFG rows,
/// docs/captures/nsq_m2m3/): M3 lvl 19/18/16 at qp 20/40/55, M2 lvl
/// 17/16/14.
pub(crate) struct NsqCfg {
    pub enabled: bool,
    pub min_nsq: usize,
    pub allow_hv4: bool,
    pub sq_weight: u64,
    pub hv_weight: u64,
    pub max_part0_to_part1_dev: u64,
    pub nsq_split_cost_th: u64,
    pub lower_depth_split_cost_th: u64,
    pub h_vs_v_split_rate_th: u64,
    pub non_hv_split_rate_th: u64,
    pub rate_th_offset_lte16: u64,
    /// `psq_txs_lvl` != 0 (levels 17..19 use lvl 1: hv_to_sq_th 1000,
    /// h_to_v_th 100 — set_sq_txs_ctrls case 1, enc_mode_config.c:5266).
    pub psq_txs: bool,
    pub component_multiple_th: u64,
}

impl NsqCfg {
    /// Disabled (presets >= 4 or non-funnel paths).
    pub(crate) fn off() -> Self {
        NsqCfg {
            enabled: false,
            min_nsq: 0,
            allow_hv4: false,
            sq_weight: u64::MAX,
            hv_weight: u64::MAX,
            max_part0_to_part1_dev: 0,
            nsq_split_cost_th: 0,
            lower_depth_split_cost_th: 0,
            h_vs_v_split_rate_th: 0,
            non_hv_split_rate_th: 0,
            rate_th_offset_lte16: 0,
            psq_txs: false,
            component_multiple_th: 0,
        }
    }

    /// `svt_aom_get_nsq_search_level_allintra` (enc_mode_config.c:11936):
    /// base level M0 3 / M1 10 / M2 14 / M3 16, then the seq_qp_mod
    /// offsets (mod 2|3: qp <= 39 +3, <= 45 +2, <= 48 +1; mod 1|2:
    /// qp > 59 -1) — capture-verified (+3/+2/+0 at qp 20/40/55).
    pub(crate) fn for_preset_qp(preset: u8, cli_qp: u32) -> Self {
        let base: i32 = match preset {
            0 => 3,
            1 => 10,
            2 => 14,
            3 => 16,
            _ => 0,
        };
        if base == 0 {
            return Self::off();
        }
        let mut level = base;
        if cli_qp <= 39 {
            level = if level + 3 > 19 { 0 } else { level + 3 };
        } else if cli_qp <= 45 {
            level = if level + 2 > 19 { 0 } else { level + 2 };
        } else if cli_qp <= 48 {
            level = if level + 1 > 19 { 0 } else { level + 1 };
        } else if cli_qp > 59 {
            // seq_qp_mod = 2 unconditionally (enc_handle.c:4221) — the
            // mod 1|2 arm applies.
            level = (level - 1).max(1);
        }
        if level == 0 {
            return Self::off();
        }

        // set_nsq_search_ctrls level rows (enc_mode_config.c:6496-6786),
        // levels reachable from the allintra bases + offsets (2..=19).
        // Level 2 is M0's base 3 minus the qp>59 offset (min 1, but 3-1=2).
        // (sq_w, max_dev, split_th, lower_th, hvv, nonhv, off16, psq, comp, hv_w)
        let row: (u64, u64, u64, u64, u64, u64, u64, u8, u64, u64) = match level {
            2 => (105, 0, 150, 3, 0, 0, 10, 0, 0, 115),
            3 => (105, 0, 100, 3, 0, 0, 10, 0, 0, 115),
            4 => (100, 0, 100, 3, 0, 0, 10, 0, 80, 115),
            5 => (100, 0, 100, 5, 0, 0, 10, 0, 80, 110),
            6 => (100, 0, 100, 5, 0, 0, 10, 0, 80, 100),
            7 => (95, 0, 80, 5, 0, 0, 10, 0, 80, 100),
            8 => (95, 0, 80, 5, 30, 20, 10, 0, 80, 100),
            9 => (95, 0, 80, 5, 40, 30, 10, 0, 60, 100),
            10 => (95, 0, 60, 10, 40, 30, 10, 0, 60, 100),
            11 => (95, 0, 60, 10, 50, 30, 10, 0, 40, 100),
            12 => (95, 0, 60, 10, 50, 30, 10, 0, 20, 100),
            13 => (95, 0, 60, 10, 60, 40, 10, 0, 20, 100),
            14 => (95, 5, 50, 10, 60, 40, 10, 0, 20, 100),
            15 => (90, 20, 40, 20, 60, 50, 10, 0, 15, 75),
            16 => (90, 50, 40, 20, 70, 60, 10, 0, 15, 75),
            17 => (90, 50, 40, 20, 70, 60, 15, 1, 10, 75),
            18 => (90, 75, 40, 20, 80, 70, 15, 1, 5, 75),
            19 => (90, 80, 35, 20, 85, 70, 15, 1, 5, 75),
            _ => unreachable!("nsq search level {level}"),
        };
        // Tail (:6788-6801): qp-based scaling factors are 1/1 (the nsq
        // flag is 0 for allintra <= M3), so only the -5 dev offset lands.
        let dev = row.1.saturating_sub(5);
        NsqCfg {
            enabled: true,
            min_nsq: 0,
            allow_hv4: true,
            sq_weight: row.0,
            max_part0_to_part1_dev: dev,
            nsq_split_cost_th: row.2,
            lower_depth_split_cost_th: row.3,
            h_vs_v_split_rate_th: row.4,
            non_hv_split_rate_th: row.5,
            rate_th_offset_lte16: row.6,
            psq_txs: row.7 != 0,
            component_multiple_th: row.8,
            hv_weight: row.9,
        }
    }
}

/// The d1 shapes tested at a SQ node, in the C Part-enum iteration order
/// (`set_blocks_to_test`, enc_dec_process.c:1403: N, H, V, H4, V4 —
/// HA/HB/VA/VB filtered by `allow_HVA_HVB = 0` at every geom level 2/3
/// preset; H4/V4 by `allow_HV4` and never at sq 8 or 128).
fn shapes_for_size(size: usize, nsq: &NsqCfg) -> &'static [PartitionType] {
    const N_ONLY: [PartitionType; 1] = [PartitionType::None];
    const NHV: [PartitionType; 3] = [
        PartitionType::None,
        PartitionType::Horz,
        PartitionType::Vert,
    ];
    const NHV4: [PartitionType; 5] = [
        PartitionType::None,
        PartitionType::Horz,
        PartitionType::Vert,
        PartitionType::Horz4,
        PartitionType::Vert4,
    ];
    if !nsq.enabled || size <= nsq.min_nsq || size == 4 {
        &N_ONLY
    } else if size == 8 || !nsq.allow_hv4 || size == 128 {
        &NHV
    } else {
        &NHV4
    }
}

/// Child geometry of a shape at a `size` SQ node: (dx, dy, w, h) in
/// coding order (C `partition_mi_offset` + `num_ns_per_shape`).
fn shape_children(size: usize, p: PartitionType) -> Vec<(usize, usize, usize, usize)> {
    let half = size / 2;
    let quarter = size / 4;
    match p {
        PartitionType::None => alloc::vec![(0, 0, size, size)],
        PartitionType::Horz => alloc::vec![(0, 0, size, half), (0, half, size, half)],
        PartitionType::Vert => alloc::vec![(0, 0, half, size), (half, 0, half, size)],
        PartitionType::Horz4 => (0..4).map(|i| (0, i * quarter, size, quarter)).collect(),
        PartitionType::Vert4 => (0..4).map(|i| (i * quarter, 0, quarter, size)).collect(),
        other => unreachable!("funnel shape {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// The PD1 depth walk
// ---------------------------------------------------------------------------

/// `skip_sub_depth_ctrls` level 1 (allintra <= M7, enc_mode_config.c —
/// the ALLINTRA sig-deriv tail): cond1 cancels sub-depth testing for
/// blocks <= 16x16 whose winner has flat quadrant distortions and few
/// coefficients.
struct SkipSubCtrls {
    max_size: usize,
    quad_deviation_th: f32,
    coeff_perc: u32,
}

pub(crate) struct DepthWalk<'a, 'b> {
    pub fx: &'a mut FunnelCtx<'b>,
    /// Full luma source plane (absolute coordinates).
    pub y_src: &'a [u8],
    pub y_src_stride: usize,
    /// Full luma decision recon plane.
    pub y_recon: &'a mut [u8],
    pub y_stride: usize,
    pub lambda: u64,
    pub part_rates: &'a PartRates,
    pub nsq: &'a NsqCfg,
    /// `ctx->disallow_4x4` — the skip_sub quadrant-arm's 8x8 clause
    /// (product_coding_loop.c:10156-10158).
    pub disallow_4x4: bool,
}

struct NodeRes {
    /// C `pc_tree->rdc.rd_cost` (partition rate + block/subtree cost).
    rd: u64,
    tree: PartitionTree,
    decisions: Vec<BlockDecision>,
}

enum SplitOut {
    /// Early exit — parent wins without full child evaluation.
    Invalid,
    /// All quadrants evaluated; parent won the final compare.
    ParentKept,
    Chosen(Box<NodeRes>),
}

/// Snapshot of the node-rect decision state — C's
/// `svt_aom_copy_neighbour_arrays` [0] <-> [1] save/restore around NSQ
/// shape evaluation, expressed on our full-plane model: the whole
/// EntropyCtx (cheap: per-frame line buffers) + the node's recon rects.
struct NodeSnap {
    ectx: crate::pipeline::EntropyCtx,
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
}

/// The SQ (PART_N) evaluation of the current node + the derived gate
/// inputs (C pc_tree->block_data[PART_N][0] and ctx side-products).
struct SqInfo {
    ev: LeafEval,
    /// `ctx->rec_dist_per_quadrant` (calc_scr_to_recon_dist_per_quadrant
    /// on the winner, product_coding_loop.c:10153-10160) when armed.
    quad: Option<[u64; 4]>,
    /// `ctx->min_nz_h / min_nz_v` (non_normative_txs :9641) when psq
    /// armed and the winner kept coefficients.
    min_nz: Option<(u16, u16)>,
}

/// SVTAV1_NSQDBG=1: mirror the instrumented C NSQDBG line format
/// (docs/captures/nsq_m2m3/) on stderr for direct MD-level diffing.
fn nsqdbg_on() -> bool {
    std::env::var_os("SVTAV1_NSQDBG").is_some()
}

/// SVTAV1_DBG_MI="mi_row,mi_col": restrict NSQDBG output to the one 64px SB
/// containing that mi (e.g. `64,112`). Unset = whole frame. Frame-wide dumps
/// are ~45 MB / 35k lines on a 512x512 photo; one SB is ~50 lines — always
/// set this when drilling a known divergence (drill_cell.sh does).
fn nsqdbg_sb() -> Option<(usize, usize)> {
    static SB: std::sync::OnceLock<Option<(usize, usize)>> = std::sync::OnceLock::new();
    *SB.get_or_init(|| {
        let v = std::env::var("SVTAV1_DBG_MI").ok()?;
        let (r, c) = v.split_once(',')?;
        Some((r.trim().parse().ok()?, c.trim().parse().ok()?))
    })
}

/// Dump gate for a record about the block at pixel (abs_x, abs_y).
pub(crate) fn nsqdbg_here(abs_x: usize, abs_y: usize) -> bool {
    nsqdbg_on()
        && match nsqdbg_sb() {
            None => true,
            Some((r, c)) => (abs_y >> 6, abs_x >> 6) == (r >> 4, c >> 4),
        }
}

/// C BLOCK_SIZES enum value of a square block (dump parity).
fn c_bsize_sq(size: usize) -> u32 {
    match size {
        4 => 0,
        8 => 3,
        16 => 6,
        32 => 9,
        _ => 12,
    }
}

/// C `Part` enum value of a funnel shape (dump parity).
fn c_part(p: PartitionType) -> u32 {
    match p {
        PartitionType::None => 0,
        PartitionType::Horz => 1,
        PartitionType::Vert => 2,
        PartitionType::Horz4 => 3,
        PartitionType::Vert4 => 4,
        _ => 255,
    }
}

impl DepthWalk<'_, '_> {
    const PARENT_COST_BIAS: u64 = 995; // ctx->parent_cost_bias, allintra
    const EE_SPLIT_TH: u64 = 50; // depth_early_exit level 1
    const EE_EARLY_TH: u64 = 1000; // early_exit_th 0 -> 1000
    /// C `CONSERVATIVE_OFFSET_0` / `AGGRESSIVE_OFFSET_1` (definitions.h:
    /// 255/258) — sq_weight adjustments in update_skip_nsq_shapes.
    const CONSERVATIVE_OFFSET_0: u64 = 5;

    fn skip_sub() -> SkipSubCtrls {
        SkipSubCtrls {
            max_size: 16,
            quad_deviation_th: 250.0,
            coeff_perc: 15,
        }
    }

    /// C `calc_scr_to_recon_dist_per_quadrant` (product_coding_loop.c:
    /// 8290): per-quadrant SSE vs the source — luma always, both chroma
    /// planes when quadrant_size > 4 (chroma dims quartered).
    ///
    /// LUMA reads the TX_DEPTH-0 recon, NOT the winning depth's: C's
    /// `cand_bf->recon` is the shared ctx temp buffer; deeper tx depths
    /// reconstruct into the aux tx-depth buffers and `update_tx_cand_bf`
    /// copies pred/coeffs/eob back but never the recon, so at gate time the
    /// shared buffer still holds the depth-0 recon. Proven on 1147124 q20 p4
    /// SB(4,6) (76,96): C's fill luma quads sum to its OWN depth-0 dist
    /// (971<<4 == 15536) while the winning depth-1 recon measures 744<<4.
    /// Chroma has no tx-depth split — the winner chroma recon is correct
    /// (and was already byte-matching C).
    fn quad_rec_dists(&self, ev: &LeafEval) -> [u64; 4] {
        let sq = ev.w;
        let quad = sq / 2;
        let mut dists = [0u64; 4];
        let yrec = ev.gate_y();
        for r in 0..2usize {
            for c in 0..2usize {
                let mut d: u64 = 0;
                for y in 0..quad {
                    let sy = (ev.abs_y + r * quad + y) * self.y_src_stride + ev.abs_x + c * quad;
                    let ry = (r * quad + y) * sq + c * quad;
                    for x in 0..quad {
                        let diff = self.y_src[sy + x] as i64 - yrec[ry + x] as i64;
                        d += (diff * diff) as u64;
                    }
                }
                if quad > 4 {
                    let cq = quad / 2;
                    let (urec, vrec) = ev.gate_uv();
                    let cw = sq / 2;
                    let ccx = ev.abs_x / 2 + c * cq;
                    let ccy = ev.abs_y / 2 + r * cq;
                    for y in 0..cq {
                        let sy = (ccy + y) * self.fx.c_stride + ccx;
                        let ry = (r * cq + y) * cw + c * cq;
                        for x in 0..cq {
                            let du = self.fx.u_src[sy + x] as i64 - urec[ry + x] as i64;
                            let dv = self.fx.v_src[sy + x] as i64 - vrec[ry + x] as i64;
                            d += (du * du) as u64 + (dv * dv) as u64;
                        }
                    }
                }
                dists[r * 2 + c] = d;
            }
        }
        if nsqdbg_here(ev.abs_x, ev.abs_y) {
            // Luma-only re-pass for the SKIPSUBQ-parity dump.
            let mut luma = [0u64; 4];
            for r in 0..2usize {
                for c in 0..2usize {
                    let mut d: u64 = 0;
                    for y in 0..quad {
                        let sy = (ev.abs_y + r * quad + y) * self.y_src_stride + ev.abs_x + c * quad;
                        let ry = (r * quad + y) * sq + c * quad;
                        for x in 0..quad {
                            let diff = self.y_src[sy + x] as i64 - yrec[ry + x] as i64;
                            d += (diff * diff) as u64;
                        }
                    }
                    luma[r * 2 + c] = d;
                }
            }
            // Pred-vs-input quads from the whole-block depth-0 prediction —
            // the C-side probe's predq counterpart (what cand_bf->pred holds
            // at C's fill time is the open question this answers).
            let pred = ev.dbg_pred();
            let mut predq = [0u64; 4];
            for r in 0..2usize {
                for c in 0..2usize {
                    let mut d: u64 = 0;
                    for y in 0..quad {
                        let sy = (ev.abs_y + r * quad + y) * self.y_src_stride + ev.abs_x + c * quad;
                        let ry = (r * quad + y) * sq + c * quad;
                        for x in 0..quad {
                            let diff = self.y_src[sy + x] as i64 - pred[ry + x] as i64;
                            d += (diff * diff) as u64;
                        }
                    }
                    predq[r * 2 + c] = d;
                }
            }
            eprintln!(
                "NSQDBG SKIPSUBQ mi=({},{}) sq={} luma={:?} tot={:?} predq={:?}",
                ev.abs_y / 4,
                ev.abs_x / 4,
                sq,
                luma,
                dists,
                predq,
            );
        }
        dists
    }

    /// C `eval_sub_depth_skip_cond1` (product_coding_loop.c:10871): f32
    /// std-deviation of the winner's per-quadrant recon SSE and the
    /// nonzero-coefficient percentage.
    fn sub_depth_skip_cond1(&self, ev: &LeafEval, quad: &[u64; 4]) -> bool {
        let ss = Self::skip_sub();
        // C float arithmetic (sum/average/pow/sqrtf).
        let n = 4f32;
        let sum: f32 = quad.iter().map(|&d| d as f32).sum();
        let average = sum / n;
        let sum1: f32 = quad
            .iter()
            .map(|&d| {
                let x = d as f32 - average;
                x * x
            })
            .sum();
        let variance = sum1 / n;
        let std_deviation = variance.sqrt();
        let total_samples = (ev.w * ev.h) as u32;
        let coeff_perc = ev.cnt_nz_coeff() * 100 / total_samples;
        std_deviation < ss.quad_deviation_th && coeff_perc < ss.coeff_perc
    }

    fn take_snap(&self, abs_x: usize, abs_y: usize, size: usize) -> NodeSnap {
        let mut y = alloc::vec![0u8; size * size];
        for r in 0..size {
            let src = (abs_y + r) * self.y_stride + abs_x;
            y[r * size..(r + 1) * size].copy_from_slice(&self.y_recon[src..src + size]);
        }
        let half = size / 2;
        let (cx, cy) = (abs_x / 2, abs_y / 2);
        let mut u = alloc::vec![0u8; half * half];
        let mut v = alloc::vec![0u8; half * half];
        for r in 0..half {
            let src = (cy + r) * self.fx.c_stride + cx;
            u[r * half..(r + 1) * half].copy_from_slice(&self.fx.u_recon[src..src + half]);
            v[r * half..(r + 1) * half].copy_from_slice(&self.fx.v_recon[src..src + half]);
        }
        NodeSnap {
            ectx: self.fx.ectx.clone(),
            y,
            u,
            v,
        }
    }

    fn restore_snap(&mut self, snap: &NodeSnap, abs_x: usize, abs_y: usize, size: usize) {
        *self.fx.ectx = snap.ectx.clone();
        for r in 0..size {
            let dst = (abs_y + r) * self.y_stride + abs_x;
            self.y_recon[dst..dst + size].copy_from_slice(&snap.y[r * size..(r + 1) * size]);
        }
        let half = size / 2;
        let (cx, cy) = (abs_x / 2, abs_y / 2);
        for r in 0..half {
            let dst = (cy + r) * self.fx.c_stride + cx;
            self.fx.u_recon[dst..dst + half].copy_from_slice(&snap.u[r * half..(r + 1) * half]);
            self.fx.v_recon[dst..dst + half].copy_from_slice(&snap.v[r * half..(r + 1) * half]);
        }
    }

    /// C `update_skip_nsq_based_on_split_rate` (product_coding_loop.c:
    /// 10181): the four partition-rate sub-gates.
    #[allow(clippy::too_many_arguments)]
    fn skip_by_split_rate(
        &self,
        shape: PartitionType,
        sq: &SqInfo,
        best_part: PartitionType,
        ctx_row: usize,
        sq_size: usize,
        split_flag: bool,
    ) -> bool {
        let nsq = self.nsq;
        let sq_cost = sq.ev.block_cost();

        let mut nsq_split_cost_th = nsq.nsq_split_cost_th;
        if nsq_split_cost_th != 0 {
            if sq_size <= 16 {
                nsq_split_cost_th = nsq_split_cost_th
                    .saturating_sub(nsq.rate_th_offset_lte16)
                    .max(1);
            }
            let split_rate = self.part_rates.bits(ctx_row, shape);
            let part_cost = rdcost(self.lambda, split_rate, 0);
            if part_cost * 1000 > sq_cost * nsq_split_cost_th {
                return true;
            }
        }

        let mut h_vs_v_th = nsq.h_vs_v_split_rate_th;
        if h_vs_v_th != 0 && matches!(shape, PartitionType::Horz | PartitionType::Vert) {
            if sq_size <= 16 {
                h_vs_v_th += nsq.rate_th_offset_lte16;
            }
            let h_cost = rdcost(
                self.lambda,
                self.part_rates.bits(ctx_row, PartitionType::Horz),
                0,
            );
            let v_cost = rdcost(
                self.lambda,
                self.part_rates.bits(ctx_row, PartitionType::Vert),
                0,
            );
            if shape == PartitionType::Horz && h_cost * h_vs_v_th > v_cost * 100 {
                return true;
            }
            if shape == PartitionType::Vert && v_cost * h_vs_v_th > h_cost * 100 {
                return true;
            }
        }

        let mut non_hv_th = nsq.non_hv_split_rate_th;
        if non_hv_th != 0 && !matches!(shape, PartitionType::Horz | PartitionType::Vert) {
            if sq_size <= 16 {
                non_hv_th += nsq.rate_th_offset_lte16;
            }
            let part_cost = rdcost(self.lambda, self.part_rates.bits(ctx_row, shape), 0);
            let best_cost = rdcost(self.lambda, self.part_rates.bits(ctx_row, best_part), 0);
            if part_cost * non_hv_th > best_cost * 100 {
                return true;
            }
        }

        let mut lower_th = nsq.lower_depth_split_cost_th;
        if lower_th != 0 && split_flag {
            if sq_size <= 16 {
                lower_th += nsq.rate_th_offset_lte16;
            }
            let split_cost = rdcost(
                self.lambda,
                self.part_rates.bits(ctx_row, PartitionType::Split),
                0,
            );
            if split_cost * 10000 < sq_cost * lower_th {
                return true;
            }
        }

        if nsq.component_multiple_th != 0 {
            let rate_cost = rdcost(self.lambda, sq.ev.total_rate(), 0);
            let dist_cost = rdcost(self.lambda, 0, sq.ev.full_dist());
            let max_comp = rate_cost.max(dist_cost);
            let min_comp = rate_cost.min(dist_cost);
            if max_comp > nsq.component_multiple_th * min_comp {
                return true;
            }
        }
        false
    }

    /// C `update_skip_nsq_based_on_sq_txs` (:10533): parent-SQ TX-split
    /// nonzero counts vs the SQ winner's count.
    fn skip_by_sq_txs(&self, shape: PartitionType, sq: &SqInfo) -> bool {
        if !self.nsq.psq_txs {
            return false;
        }
        let Some((nz_h, nz_v)) = sq.min_nz else {
            return false;
        };
        let cnt_nz = sq.ev.cnt_nz_coeff() as u64;
        // psq_txs_lvl 1: hv_to_sq_th 1000, h_to_v_th 100.
        let (hv_to_sq_th, h_to_v_th) = (1000u64, 100u64);
        let cnt_h_best = (nz_h as u64) << 1;
        let cnt_v_best = (nz_v as u64) << 1;
        if cnt_h_best >= cnt_nz * hv_to_sq_th / 100 && cnt_v_best >= cnt_nz * hv_to_sq_th / 100 {
            return true;
        }
        if matches!(shape, PartitionType::Horz | PartitionType::Horz4)
            && cnt_v_best <= cnt_h_best
            && cnt_h_best >= cnt_nz * h_to_v_th / 100
        {
            return true;
        }
        if matches!(shape, PartitionType::Vert | PartitionType::Vert4)
            && cnt_h_best <= cnt_v_best
            && cnt_v_best >= cnt_nz * h_to_v_th / 100
        {
            return true;
        }
        false
    }

    /// C `update_skip_nsq_based_on_sq_recon_dist` (:10317).
    fn skip_by_recon_dist(&self, shape: PartitionType, sq: &SqInfo) -> bool {
        let mut max_dev = self.nsq.max_part0_to_part1_dev;
        if max_dev == 0 {
            return false;
        }
        let Some(quad) = &sq.quad else {
            return false;
        };
        let full_lambda = self.lambda;
        let dist = rdcost(full_lambda, 0, sq.ev.full_dist());
        let cost = sq.ev.block_cost();
        let dist_cost_ratio = (dist * 100) / cost;
        let (min_ratio, max_ratio) = (50u64, 100u64);
        let modulated_th = if dist_cost_ratio > min_ratio {
            (100 * (dist_cost_ratio - min_ratio)) / (max_ratio - min_ratio)
        } else {
            0 // unused: the <= min_ratio arm forces the threshold to 0
        };

        // Parent SQ mode modulation (C PredictionMode indices: DC 0, V 1,
        // H 2, D45..D67 3..8, SMOOTH* 9..11, PAETH 12).
        let mode = sq.ev.mode();
        match mode {
            0 | 1 | 2 => max_dev *= 2,
            3..=12 => max_dev <<= 2,
            _ => {}
        }

        let dq: [u64; 4] = [
            quad[0].max(1),
            quad[1].max(1),
            quad[2].max(1),
            quad[3].max(1),
        ];
        if matches!(shape, PartitionType::Horz | PartitionType::Horz4) {
            // V/D67/D113/D45/D135 -> x4; H -> 0.
            if matches!(mode, 1 | 8 | 5 | 3 | 4) {
                max_dev <<= 2;
            } else if mode == 2 {
                max_dev = 0;
            }
            let dist_h0 = dq[0] + dq[1];
            let dist_h1 = dq[2] + dq[3];
            let dev =
                ((dist_h0 as i64 - dist_h1 as i64).unsigned_abs() * 100) / dist_h0.min(dist_h1);
            let quad_dev_t =
                ((dq[0] as i64 - dq[1] as i64).unsigned_abs() * 100) / dq[0].min(dq[1]);
            let quad_dev_b =
                ((dq[2] as i64 - dq[3] as i64).unsigned_abs() * 100) / dq[2].min(dq[3]);
            max_dev += max_dev * quad_dev_t.min(quad_dev_b) / 100;
            max_dev = if dist_cost_ratio <= min_ratio {
                0
            } else if dist_cost_ratio <= max_ratio {
                (max_dev * modulated_th) / 100
            } else {
                dist_cost_ratio
            };
            if dev < max_dev {
                return true;
            }
        }
        if matches!(shape, PartitionType::Vert | PartitionType::Vert4) {
            // H/D157/D203/D45/D135 -> x4; V -> 0.
            if matches!(mode, 2 | 6 | 7 | 3 | 4) {
                max_dev <<= 2;
            } else if mode == 1 {
                max_dev = 0;
            }
            let dist_v0 = dq[0] + dq[2];
            let dist_v1 = dq[1] + dq[3];
            let dev =
                ((dist_v0 as i64 - dist_v1 as i64).unsigned_abs() * 100) / dist_v0.min(dist_v1);
            let quad_dev_l =
                ((dq[0] as i64 - dq[2] as i64).unsigned_abs() * 100) / dq[0].min(dq[2]);
            let quad_dev_r =
                ((dq[1] as i64 - dq[3] as i64).unsigned_abs() * 100) / dq[1].min(dq[3]);
            max_dev += max_dev * quad_dev_l.min(quad_dev_r) / 100;
            max_dev = if dist_cost_ratio <= min_ratio {
                0
            } else if dist_cost_ratio <= max_ratio {
                (max_dev * modulated_th) / 100
            } else {
                dist_cost_ratio
            };
            if dev < max_dev {
                return true;
            }
        }
        false
    }

    /// C `update_skip_nsq_shapes` (:10454): SQ-vs-H/V relative-cost skip
    /// for the non-HV shapes (H4/V4 here; HA/HB/VA/VB are geometry-off).
    fn skip_by_shapes(
        &self,
        shape: PartitionType,
        sq: &SqInfo,
        h_children: &Option<[(u64, bool); 2]>,
        v_children: &Option<[(u64, bool); 2]>,
    ) -> bool {
        let mut sq_weight = self.nsq.sq_weight;
        if sq_weight == u64::MAX {
            return false;
        }
        if matches!(shape, PartitionType::Horz4 | PartitionType::Vert4) {
            sq_weight += Self::CONSERVATIVE_OFFSET_0;
        }
        let sq_cost = sq.ev.block_cost();
        if shape == PartitionType::Horz4 {
            if let Some(h) = h_children {
                let h_cost = h[0].0 + h[1].0;
                let mut skip = h_cost > (sq_cost * sq_weight) / 100;
                if !skip {
                    if let Some(v) = v_children {
                        let v_cost = v[0].0 + v[1].0;
                        skip = h_cost > (v_cost * self.nsq.hv_weight) / 100;
                    }
                }
                return skip;
            }
        }
        if shape == PartitionType::Vert4 {
            if let Some(v) = v_children {
                let v_cost = v[0].0 + v[1].0;
                let mut skip = v_cost > (sq_cost * sq_weight) / 100;
                if !skip {
                    if let Some(h) = h_children {
                        let h_cost = h[0].0 + h[1].0;
                        skip = v_cost > (h_cost * self.nsq.hv_weight) / 100;
                    }
                }
                return skip;
            }
        }
        false
    }

    /// C `get_skip_processing_nsq_block` (:10826): the gates in order.
    #[allow(clippy::too_many_arguments)]
    fn skip_processing_nsq(
        &self,
        shape: PartitionType,
        sq: &SqInfo,
        best_part: PartitionType,
        ctx_row: usize,
        sq_size: usize,
        split_flag: bool,
        h_children: &Option<[(u64, bool); 2]>,
        v_children: &Option<[(u64, bool); 2]>,
    ) -> bool {
        if self.skip_by_split_rate(shape, sq, best_part, ctx_row, sq_size, split_flag) {
            return true;
        }
        if self.skip_by_sq_txs(shape, sq) {
            return true;
        }
        if self.skip_by_recon_dist(shape, sq) {
            return true;
        }
        if self.skip_by_shapes(shape, sq, h_children, v_children) {
            return true;
        }
        false
    }

    /// C `svt_aom_pick_partition` (product_coding_loop.c:11549) —
    /// test_depth (:11396, the d1 shape loop) + the sub-depth walk.
    fn pick(&mut self, scan: &RefScan, abs_x: usize, abs_y: usize) -> NodeRes {
        let size = scan.sq;
        let mut split_flag = scan.split_flag;

        // C test_depth state: rdc (best partition so far), the SQ info,
        // the H/V child costs for the H4/V4 gates, and the winning
        // shape's evaluations for the final commit.
        let mut best: Option<(PartitionType, u64, Vec<LeafEval>)> = None;
        let mut sq_info: Option<SqInfo> = None;
        let mut h_children: Option<[(u64, bool); 2]> = None;
        let mut v_children: Option<[(u64, bool); 2]> = None;
        let mut snap: Option<NodeSnap> = None;
        let mut committed_since_snap = false;

        if scan.test_this {
            // update_part_neighs: partition contexts read once per node.
            let (ctx_row, _) = self.fx.ectx.partition_ctx(abs_x, abs_y, size);

            let shapes = shapes_for_size(size, self.nsq);
            for &shape in shapes {
                // Restore the pre-shape state (C: copy [1] -> [0] at
                // nsi == 0 when a previous shape saved it).
                if committed_since_snap {
                    if let Some(sn) = snap.take() {
                        self.restore_snap(&sn, abs_x, abs_y, size);
                        snap = Some(sn);
                        committed_since_snap = false;
                    }
                }

                // C `svt_aom_partition_rate_cost` (rd_cost.c:1837) returns 0 for
                // `bsize < BLOCK_8X8`: a 4x4 codes NO partition symbol. The only
                // square `size` node below 8 is the 4x4 (4x8/8x4 are NSQ children,
                // not square nodes), so gate the partition rate there.
                let part_rate = if size >= 8 {
                    self.part_rates.bits(ctx_row, shape)
                } else {
                    0
                };
                let mut part_cost = rdcost(self.lambda, part_rate, 0);
                let children = shape_children(size, shape);
                let mut evals: Vec<LeafEval> = Vec::with_capacity(children.len());
                let mut valid = true;

                for (nsi, &(dx, dy, cw, ch)) in children.iter().enumerate() {
                    if shape != PartitionType::None && nsi == 0 {
                        // faster_md_settings_nsq: I-slice-dead (C gates
                        // the call on slice_type != I_SLICE, :11470).
                        let sq = sq_info.as_ref().expect("PART_N tested first");
                        let best_part = best
                            .as_ref()
                            .map(|(p, _, _)| *p)
                            .unwrap_or(PartitionType::None);
                        if self.skip_processing_nsq(
                            shape,
                            sq,
                            best_part,
                            ctx_row,
                            size,
                            scan.split_flag,
                            &h_children,
                            &v_children,
                        ) {
                            if nsqdbg_here(abs_x, abs_y) {
                                let g = if self.skip_by_split_rate(
                                    shape,
                                    sq,
                                    best_part,
                                    ctx_row,
                                    size,
                                    scan.split_flag,
                                ) {
                                    1
                                } else if self.skip_by_sq_txs(shape, sq) {
                                    2
                                } else if self.skip_by_recon_dist(shape, sq) {
                                    3
                                } else {
                                    4
                                };
                                eprintln!(
                                    "NSQDBG SKIP mi=({},{}) bsize={} shape={} gate={}",
                                    abs_y / 4,
                                    abs_x / 4,
                                    c_bsize_sq(size),
                                    c_part(shape),
                                    g,
                                );
                            }
                            valid = false;
                            break;
                        }
                    }

                    let cx = abs_x + dx;
                    let cy = abs_y + dy;
                    let ev = evaluate_leaf(
                        self.fx,
                        self.y_src,
                        self.y_src_stride,
                        cy * self.y_src_stride + cx,
                        self.y_recon,
                        self.y_stride,
                        cx,
                        cy,
                        cw,
                        ch,
                        false, // is_dc_only gate: eff-M9 only
                        // sb_is_lvl6: ignored here (txs_lvl6_gate is false for
                        // every preset that reaches the depth-refine walk).
                        true,
                    );
                    if nsqdbg_here(abs_x, abs_y) {
                        eprintln!(
                            "NSQDBG BLK mi=({},{}) bsize={} shape={} nsi={} cost={} rate={} dist={} mode={} coeff={} nz={} txd={} uv={} txt=[{}] ye=[{}] ue={} ve={} fi={} ady={} aduv={} qdc=[{}]",
                            abs_y / 4,
                            abs_x / 4,
                            c_bsize_sq(size),
                            c_part(shape),
                            nsi,
                            ev.block_cost(),
                            ev.total_rate(),
                            ev.full_dist(),
                            ev.mode(),
                            u8::from(ev.block_has_coeff()),
                            ev.cnt_nz_coeff(),
                            ev.tx_depth(),
                            ev.uv_mode(),
                            ev.dbg_txb_types(),
                            ev.dbg_txb_eobs(),
                            ev.dbg_uv_eobs().0,
                            ev.dbg_uv_eobs().1,
                            ev.dbg_fi(),
                            ev.dbg_deltas().0,
                            ev.dbg_deltas().1,
                            ev.dbg_qdcs(),
                        );
                    }
                    part_cost += ev.block_cost();
                    evals.push(ev);

                    if let Some((_, best_rd, _)) = &best {
                        if part_cost >= *best_rd {
                            if nsqdbg_here(abs_x, abs_y) {
                                eprintln!(
                                    "NSQDBG ABORT mi=({},{}) bsize={} shape={} nsi={} part_cost={} best={}",
                                    abs_y / 4,
                                    abs_x / 4,
                                    c_bsize_sq(size),
                                    c_part(shape),
                                    nsi,
                                    part_cost,
                                    best_rd,
                                );
                            }
                            valid = false;
                            break;
                        }
                    }

                    if nsi + 1 < children.len() {
                        if snap.is_none() {
                            snap = Some(self.take_snap(abs_x, abs_y, size));
                        }
                        committed_since_snap = true;
                        let ev = evals.last().unwrap();
                        commit_leaf(self.fx, self.y_recon, self.y_stride, ev);
                    }
                }

                // Track H/V child costs for the H4/V4 gates (C
                // tested_blk[PART_H/V][0..1] + block_has_coeff).
                if matches!(shape, PartitionType::Horz | PartitionType::Vert) && evals.len() == 2 {
                    let pair = [
                        (evals[0].block_cost(), evals[0].block_has_coeff()),
                        (evals[1].block_cost(), evals[1].block_has_coeff()),
                    ];
                    if shape == PartitionType::Horz {
                        h_children = Some(pair);
                    } else {
                        v_children = Some(pair);
                    }
                }

                if shape == PartitionType::None {
                    debug_assert!(valid, "PART_N cannot abort (rdc starts invalid)");
                    let ev = &evals[0];
                    // rec_dist_per_quadrant (C gate :10153): the NSQ
                    // recon-dist arm OR the skip_sub arm.
                    let nsq_arm = self.nsq.enabled
                        && self.nsq.max_part0_to_part1_dev != 0
                        && size >= 8
                        && size > self.nsq.min_nsq;
                    let ss = Self::skip_sub();
                    let skip_sub_arm = size <= ss.max_size
                        && scan.split_flag
                        && (size >= 16 || (!self.disallow_4x4 && size == 8));
                    let quad = if nsq_arm || skip_sub_arm {
                        Some(self.quad_rec_dists(ev))
                    } else {
                        None
                    };
                    // non_normative_txs (C gate :10174).
                    let min_nz = if self.nsq.enabled
                        && self.nsq.psq_txs
                        && size >= 8
                        && size > self.nsq.min_nsq
                    {
                        crate::leaf_funnel::min_nz_hv(
                            ev,
                            self.fx.frame.base_qindex,
                            self.fx.frame.qm_levels[0],
                        )
                    } else {
                        None
                    };
                    sq_info = Some(SqInfo {
                        ev: evals.pop().unwrap(),
                        quad,
                        min_nz,
                    });
                    if valid {
                        best = Some((PartitionType::None, part_cost, Vec::new()));
                    }
                } else if valid {
                    let better = match &best {
                        None => true,
                        Some((_, rd, _)) => part_cost < *rd,
                    };
                    if better {
                        best = Some((shape, part_cost, evals));
                    }
                }
                if nsqdbg_here(abs_x, abs_y) {
                    let (bp, brd) = best
                        .as_ref()
                        .map(|(p, rd, _)| (*p as u32, *rd))
                        .unwrap_or((255, 0));
                    eprint!(
                        "NSQDBG SHAPE mi=({},{}) bsize={} shape={} valid={} part_cost={} part_rate={} best={}/{}",
                        abs_y / 4,
                        abs_x / 4,
                        c_bsize_sq(size),
                        c_part(shape),
                        u8::from(valid),
                        part_cost,
                        part_rate,
                        bp,
                        brd,
                    );
                    if shape == PartitionType::None {
                        let sq = sq_info.as_ref().unwrap();
                        let q = sq.quad.unwrap_or([0; 4]);
                        let (nzh, nzv) = sq.min_nz.unwrap_or((0, 0));
                        eprint!(
                            " q=[{},{},{},{}] nzh={} nzv={}",
                            q[0], q[1], q[2], q[3], nzh, nzv
                        );
                    }
                    eprintln!();
                }
            }

            // skip_sub_depth cond1 (svt_aom_pick_partition:11563-11568) —
            // on the SQ winner's quadrant dists.
            if let Some(sq) = &sq_info {
                if split_flag && size <= Self::skip_sub().max_size {
                    if let Some(quad) = &sq.quad {
                        if self.sub_depth_skip_cond1(&sq.ev, quad) {
                            split_flag = false;
                        }
                    }
                }
            }

            // C: restore [1] -> [0] before the sub-depth walk.
            if committed_since_snap && split_flag {
                if let Some(sn) = snap.take() {
                    self.restore_snap(&sn, abs_x, abs_y, size);
                    snap = Some(sn);
                    committed_since_snap = false;
                }
            }
        }

        let parent_rd = best.as_ref().map(|(_, rd, _)| *rd);
        if split_flag {
            match self.test_split(scan, abs_x, abs_y, parent_rd) {
                SplitOut::Chosen(res) => return *res,
                SplitOut::ParentKept | SplitOut::Invalid => {
                    // Parent (best shape) stays; fall through to its
                    // commit (test_split_partition's winner overwrite).
                }
            }
        }

        // Commit the winning shape (C md_update_all_neighbour_arrays_
        // multiple over the chosen partition's blocks). If a losing
        // shape's partial commits are still live, restore first —
        // equivalent to C's winner-overwrite since every write spans
        // exactly the block.
        if committed_since_snap {
            if let Some(sn) = snap.take() {
                self.restore_snap(&sn, abs_x, abs_y, size);
            }
        }
        let (win_part, win_rd, win_evals) = best.expect("refined node with no valid shape");
        if win_part == PartitionType::None {
            let sq = sq_info.expect("SQ info for PART_N winner");
            commit_leaf(self.fx, self.y_recon, self.y_stride, &sq.ev);
            let decision = crate::partition::funnel_block_decision(sq.ev.to_choice(), size, size);
            return NodeRes {
                rd: win_rd,
                tree: PartitionTree::Leaf(decision.clone()),
                decisions: alloc::vec![decision],
            };
        }
        let mut decisions: Vec<BlockDecision> = Vec::with_capacity(win_evals.len());
        let mut child_trees: Vec<PartitionTree> = Vec::with_capacity(win_evals.len());
        for ev in &win_evals {
            commit_leaf(self.fx, self.y_recon, self.y_stride, ev);
            let d = crate::partition::funnel_block_decision(ev.to_choice(), ev.w, ev.h);
            decisions.push(d.clone());
            child_trees.push(PartitionTree::Leaf(d));
        }
        NodeRes {
            rd: win_rd,
            tree: PartitionTree::Split {
                partition_type: win_part,
                width: size as u16,
                height: size as u16,
                children: child_trees,
            },
            decisions,
        }
    }

    /// C `test_split_partition` (product_coding_loop.c:11304).
    fn test_split(
        &mut self,
        scan: &RefScan,
        abs_x: usize,
        abs_y: usize,
        parent_rd: Option<u64>,
    ) -> SplitOut {
        let size = scan.sq;
        let (ctx_row, _) = self.fx.ectx.partition_ctx(abs_x, abs_y, size);
        // use_accurate_part_ctx = 1: no x2 bias.
        let split_rate = self.part_rates.bits(ctx_row, PartitionType::Split);
        let mut split_cost = rdcost(self.lambda, split_rate, 0);

        let half = size / 2;
        let children = scan.children.as_ref().expect("split_flag children");
        let mut trees: Vec<PartitionTree> = Vec::with_capacity(4);
        let mut decisions: Vec<BlockDecision> = Vec::new();
        let mut child_rd = [0u64; 4]; // NSQDBG only: per-quadrant pick() RD
        for (i, child) in children.iter().enumerate() {
            // Per-quadrant early exit vs the parent depth cost
            // (:11346-11360; th 50 for i == 0, else 1000; bias 995).
            if let Some(prd) = parent_rd {
                let th = if i == 0 {
                    Self::EE_SPLIT_TH
                } else {
                    Self::EE_EARLY_TH
                };
                if (prd as u128) * (th as u128) * (Self::PARENT_COST_BIAS as u128)
                    <= (split_cost as u128) * 1_000_000
                {
                    if nsqdbg_here(abs_x, abs_y) {
                        eprintln!(
                            "NSQDBG TSX mi=({},{}) bsize={} i={} parent={} split={}",
                            abs_y / 4,
                            abs_x / 4,
                            c_bsize_sq(size),
                            i,
                            prd,
                            split_cost,
                        );
                    }
                    return SplitOut::Invalid;
                }
            }
            let cx = abs_x + (i & 1) * half;
            let cy = abs_y + (i >> 1) * half;
            let res = self.pick(child, cx, cy);
            child_rd[i] = res.rd;
            split_cost += res.rd;
            trees.push(res.tree);
            decisions.extend(res.decisions);
        }

        // Final compare (:11375): parent wins on
        // bias * parent_rd <= split_cost * 1000.
        if nsqdbg_here(abs_x, abs_y) {
            let chose = match parent_rd {
                Some(prd)
                    if (Self::PARENT_COST_BIAS as u128) * (prd as u128)
                        <= (split_cost as u128) * 1000 =>
                {
                    "parent"
                }
                _ => "split",
            };
            eprintln!(
                "NSQDBG TS mi=({},{}) bsize={} parent_valid={} parent={} split={} sr={} c=[{},{},{},{}] chose={}",
                abs_y / 4,
                abs_x / 4,
                c_bsize_sq(size),
                u8::from(parent_rd.is_some()),
                parent_rd.unwrap_or(0),
                split_cost,
                split_rate,
                child_rd[0],
                child_rd[1],
                child_rd[2],
                child_rd[3],
                chose,
            );
        }
        if let Some(prd) = parent_rd {
            if (Self::PARENT_COST_BIAS as u128) * (prd as u128) <= (split_cost as u128) * 1000 {
                return SplitOut::ParentKept;
            }
        }
        SplitOut::Chosen(Box::new(NodeRes {
            rd: split_cost,
            tree: PartitionTree::Split {
                partition_type: PartitionType::Split,
                width: size as u16,
                height: size as u16,
                children: trees,
            },
            decisions,
        }))
    }
}

/// Decide one SB with the refined depth walk; the result mirrors
/// `encode_fixed_tree`'s funnel output (tree + decisions in coding
/// order).
#[allow(clippy::too_many_arguments)]
pub(crate) fn decide_sb_refined(
    scan: &RefScan,
    fx: &mut FunnelCtx<'_>,
    y_src: &[u8],
    y_src_stride: usize,
    y_recon: &mut [u8],
    y_stride: usize,
    lambda: u64,
    part_rates: &PartRates,
    nsq: &NsqCfg,
    disallow_4x4: bool,
    sb_x: usize,
    sb_y: usize,
) -> crate::partition::PartitionResult {
    let mut walk = DepthWalk {
        fx,
        y_src,
        y_src_stride,
        y_recon,
        y_stride,
        lambda,
        part_rates,
        nsq,
        disallow_4x4,
    };
    let res = walk.pick(scan, sb_x, sb_y);
    let num_blocks = res.decisions.len() as u32;
    crate::partition::PartitionResult {
        partition_type: match &res.tree {
            PartitionTree::Leaf(_) => PartitionType::None,
            _ => PartitionType::Split,
        },
        rd_cost: res.rd,
        distortion: 0,
        rate: 0,
        decisions: res.decisions,
        tree: Some(res.tree),
        num_blocks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nsq_cfg_matches_instrumented_captures() {
        // NSQCFG rows (docs/captures/nsq_m2m3/): M3 levels 19/18/16 at
        // qp 20/40/55, M2 levels 17/16/14 — post-tail values (dev - 5).
        let c = NsqCfg::for_preset_qp(3, 20);
        assert!(c.enabled && c.allow_hv4 && c.psq_txs);
        assert_eq!(
            (c.sq_weight, c.hv_weight, c.max_part0_to_part1_dev),
            (90, 75, 75)
        );
        assert_eq!((c.nsq_split_cost_th, c.lower_depth_split_cost_th), (35, 20));
        assert_eq!((c.h_vs_v_split_rate_th, c.non_hv_split_rate_th), (85, 70));
        assert_eq!((c.rate_th_offset_lte16, c.component_multiple_th), (15, 5));
        let c = NsqCfg::for_preset_qp(3, 40);
        assert_eq!((c.max_part0_to_part1_dev, c.nsq_split_cost_th), (70, 40));
        assert_eq!((c.h_vs_v_split_rate_th, c.non_hv_split_rate_th), (80, 70));
        assert!(c.psq_txs);
        let c = NsqCfg::for_preset_qp(3, 55);
        assert_eq!(
            (c.max_part0_to_part1_dev, c.component_multiple_th),
            (45, 15)
        );
        assert!(!c.psq_txs); // level 16
        let c = NsqCfg::for_preset_qp(2, 20);
        assert!(c.psq_txs); // level 17
        assert_eq!((c.max_part0_to_part1_dev, c.rate_th_offset_lte16), (45, 15));
        let c = NsqCfg::for_preset_qp(2, 40);
        assert!(!c.psq_txs); // level 16
        assert_eq!(c.max_part0_to_part1_dev, 45);
        let c = NsqCfg::for_preset_qp(2, 55);
        assert_eq!((c.max_part0_to_part1_dev, c.component_multiple_th), (0, 20));
        assert_eq!((c.sq_weight, c.hv_weight), (95, 100));
        // Presets >= 4: search off.
        assert!(!NsqCfg::for_preset_qp(4, 40).enabled);
    }

    #[test]
    fn dr_ctrls_match_capture() {
        // M5DBG CFG enc_mode=4: dr_s1=15 dr_e1=15 dr_maxmult=10
        // dr_bandmod=0 dr_lowsplit=20 dr_splitrate=10 dr_limitpd0=1
        // dr_unavail=2 (docs/captures/m0m5_config_dlf.txt line 14).
        let m4 = DrCtrls::for_preset(4);
        assert!(m4.adaptive);
        assert_eq!((m4.s1_th, m4.e1_th), (15, 15));
        assert_eq!(m4.parent_max_cost_mult, 10);
        assert!(!m4.band_mod);
        assert_eq!((m4.lower_split_th, m4.split_rate_th), (20, 10));
        assert_eq!((m4.limit_to_pd0, m4.unavail_mode), (1, 2));
        // enc_mode=5: dr_s1=10 dr_e1=10 dr_maxmult=0 dr_bandmod=1
        // dr_maxcostmult=400 dr_bands=4 dr_lowsplit=100 dr_splitrate=5
        // dr_unavail=0.
        let m5 = DrCtrls::for_preset(5);
        assert!(m5.adaptive);
        assert_eq!((m5.s1_th, m5.e1_th), (10, 10));
        assert_eq!(m5.parent_max_cost_mult, 0);
        assert!(m5.band_mod);
        assert_eq!((m5.max_cost_multiplier, m5.max_band_cnt), (400, 4));
        assert_eq!(m5.decrement_per_band, [i64::MAX, i64::MAX, 10, 5]);
        assert_eq!((m5.lower_split_th, m5.split_rate_th), (100, 5));
        assert_eq!((m5.limit_to_pd0, m5.unavail_mode), (1, 0));
        // M6+ collapses to PRED_PART_ONLY.
        assert!(!DrCtrls::for_preset(6).adaptive);
    }

    /// The identity-harness gradient content (identity_run.rs) at 64x64.
    fn gradient64() -> alloc::vec::Vec<u8> {
        let (w, h) = (64usize, 64usize);
        let mut y = alloc::vec![0u8; w * h];
        for r in 0..h {
            for c in 0..w {
                y[r * w + c] = (((r * 255) / h) ^ ((c * 3) & 0x3f)) as u8;
            }
        }
        y
    }

    /// Refined-scan shape pins vs the instrumented M5DBG WIN dumps
    /// (docs/captures/m0m5_config_dlf.txt, gradient 64x64 preset 5):
    /// - q20/q40: PD0 tree = 64 SPLIT + 4x32 NONE; 16x16 evaluations
    ///   appear ONLY under the (32,0) quadrant (the child-deviation gate
    ///   admits the sub-depth for quadrant 1, rejects 0/2/3), and there
    ///   is NO 64x64 WIN row (the parent depth is not admitted).
    /// - q55: PD0 tree = single 64x64 NONE and the WIN dump has ONLY the
    ///   64x64 row — no 32x32 evaluations (e_depth 0 for the root leaf).
    #[test]
    fn m5_gradient64_scan_matches_capture() {
        let y = gradient64();
        let ctrls = DrCtrls::for_preset(5);
        for (qp, qindex, lambda) in [(20u32, 80u8, 25650u64), (40, 160, 248207)] {
            let tables = crate::pd0::build_m6_pd0_tables(qindex);
            let eval =
                crate::pd0::pd0_pick_sb_partition_m6_eval(&y, 64, 0, 0, qp, qindex, &tables, 8, 1, false);
            assert!(eval.split, "q{qp}: PD0 splits the 64");
            let scan = build_refined_scan(&eval, &ctrls, lambda, &tables);
            assert!(!scan.test_this, "q{qp}: no 64x64 parent-depth eval");
            assert!(scan.split_flag);
            let ch = scan.children.as_ref().unwrap();
            assert!(ch.iter().all(|c| c.test_this), "q{qp}: all 32s evaluated");
            assert!(
                !ch[0].split_flag && ch[1].split_flag && !ch[2].split_flag && !ch[3].split_flag,
                "q{qp}: 16x16 depth admitted only under (32,0)"
            );
        }
        // q55: 64x64 NONE, no deeper evals.
        let tables = crate::pd0::build_m6_pd0_tables(220);
        let eval = crate::pd0::pd0_pick_sb_partition_m6_eval(&y, 64, 0, 0, 55, 220, &tables, 8, 1, false);
        assert!(!eval.split);
        let scan = build_refined_scan(&eval, &ctrls, 1527856, &tables);
        assert!(scan.test_this && !scan.split_flag && scan.children.is_none());
    }

    /// M4 (dr level 6) on the same content: the wider e1 threshold (15
    /// vs 10) and the M4 leaf funnel own the g128-q20 SB0 (0,32) 32x32
    /// -> 4x16 flip the differ chased (byte-identical after this port);
    /// at 64x64 the admissions stay quadrant-1-only like M5 (pinned so
    /// gate drift is caught without the harness).
    #[test]
    fn m4_gradient64_scan_shape() {
        let y = gradient64();
        let ctrls = DrCtrls::for_preset(4);
        let tables = crate::pd0::build_m6_pd0_tables(80);
        let eval = crate::pd0::pd0_pick_sb_partition_m6_eval(&y, 64, 0, 0, 20, 80, &tables, 8, 1, false);
        assert!(eval.split);
        let scan = build_refined_scan(&eval, &ctrls, 25650, &tables);
        assert!(!scan.test_this && scan.split_flag);
        let ch = scan.children.as_ref().unwrap();
        assert!(ch.iter().all(|c| c.test_this));
        assert!(!ch[0].split_flag && ch[1].split_flag && !ch[2].split_flag && !ch[3].split_flag);
    }

    #[test]
    fn pred_part_only_scan_equals_pd0_tree() {
        // A PRED_PART_ONLY refinement must mark exactly the PD0 leaves.
        let eval = Pd0Eval {
            sq: 64,
            tested: true,
            cost: 100,
            split: true,
            children: Some(Box::new([
                Pd0Eval {
                    sq: 32,
                    tested: true,
                    cost: 25,
                    split: false,
                    children: None,
                },
                Pd0Eval {
                    sq: 32,
                    tested: true,
                    cost: 25,
                    split: false,
                    children: None,
                },
                Pd0Eval {
                    sq: 32,
                    tested: true,
                    cost: 25,
                    split: false,
                    children: None,
                },
                Pd0Eval {
                    sq: 32,
                    tested: true,
                    cost: 25,
                    split: false,
                    children: None,
                },
            ])),
        };
        let ctrls = DrCtrls::for_preset(6);
        let tables = crate::pd0::build_m6_pd0_tables(160);
        let scan = build_refined_scan(&eval, &ctrls, 248207, &tables);
        assert!(!scan.test_this && scan.split_flag);
        for c in scan.children.as_ref().unwrap().iter() {
            assert!(c.test_this && !c.split_flag && c.children.is_none());
        }
    }
}
