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
    fn set_children_tested(&mut self, e_depth: i32) {
        // disallow_4x4 = 1 on the 420 path: 8x8 has no testable children.
        if self.sq <= 8 {
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
                c.set_children_tested(e_depth - 1);
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
) -> (i32, i32) {
    let ctrls = env.ctrls;
    if !ctrls.adaptive {
        return (0, 0);
    }
    let sq = node.sq;
    let mut s: i32 = -2;
    let mut e: i32 = 2;
    // 4x4 has no children; disallow_4x4 = 1 caps the sub-depths
    // (enc_dec_process.c:1811-1813).
    e = match sq {
        4 | 8 => 0,
        16 => e.min(1),
        32 => e.min(2),
        _ => e,
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

    (if add_parent { s } else { 0 }, if add_sub { e } else { 0 })
}

/// C `refine_depth` (enc_dec_process.c:1901): walk the PD0 pc_tree and
/// build the refined MdScan marks. Returns the subtree's s_depth
/// propagation (parent-depth admissions bubble up: a SPLIT node whose
/// children admit their parent evaluates ITS PART_N, :1947-1953).
fn refine_depth(env: &RefineEnv<'_>, node: &Pd0Eval, parent: Option<&Pd0Eval>) -> (RefScan, i32) {
    let mut scan = RefScan::leaf(node.sq);
    if !node.split {
        scan.test_this = true;
        let (s, e) = set_start_end_depth(env, node, parent);
        if e > 0 {
            scan.set_children_tested(e);
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
            let (cs, s_child) = refine_depth(env, cev, Some(node));
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
    refine_depth(&env, root, None).0
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

impl DepthWalk<'_, '_> {
    const PARENT_COST_BIAS: u64 = 995; // ctx->parent_cost_bias, allintra
    const EE_SPLIT_TH: u64 = 50; // depth_early_exit level 1
    const EE_EARLY_TH: u64 = 1000; // early_exit_th 0 -> 1000

    fn skip_sub() -> SkipSubCtrls {
        SkipSubCtrls {
            max_size: 16,
            quad_deviation_th: 250.0,
            coeff_perc: 15,
        }
    }

    /// C `eval_sub_depth_skip_cond1` (product_coding_loop.c:10353 area):
    /// f32 std-deviation of the winner's per-quadrant recon SSE (luma +
    /// both chroma planes at quadrant > 4px) vs the source, and the
    /// nonzero-coefficient percentage.
    fn sub_depth_skip_cond1(&self, ev: &LeafEval) -> bool {
        let ss = Self::skip_sub();
        let sq = ev.size;
        let quad = sq / 2;
        let mut dists = [0u64; 4];
        let yrec = ev.y_recon();
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
                    let (urec, vrec) = ev.uv_recon();
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
        // C float arithmetic (sum/average/pow/sqrtf).
        let n = 4f32;
        let sum: f32 = dists.iter().map(|&d| d as f32).sum();
        let average = sum / n;
        let sum1: f32 = dists
            .iter()
            .map(|&d| {
                let x = d as f32 - average;
                x * x
            })
            .sum();
        let variance = sum1 / n;
        let std_deviation = variance.sqrt();
        let total_samples = (sq * sq) as u32;
        let coeff_perc = ev.cnt_nz_coeff() * 100 / total_samples;
        std_deviation < ss.quad_deviation_th && coeff_perc < ss.coeff_perc
    }

    /// C `svt_aom_pick_partition` (product_coding_loop.c:11549).
    fn pick(&mut self, scan: &RefScan, abs_x: usize, abs_y: usize) -> NodeRes {
        let size = scan.sq;
        let mut split_flag = scan.split_flag;
        let mut cur: Option<(u64, LeafEval)> = None;

        if scan.test_this {
            // update_part_neighs + test_depth's PART_N shape: partition
            // rate at the real contexts, funnel block cost.
            let (ctx_row, _) = self.fx.ectx.partition_ctx(abs_x, abs_y, size);
            let part_rate = self.part_rates.bits(ctx_row, PartitionType::None);
            let ev = evaluate_leaf(
                self.fx,
                self.y_src,
                self.y_src_stride,
                abs_y * self.y_src_stride + abs_x,
                self.y_recon,
                self.y_stride,
                abs_x,
                abs_y,
                size,
                false, // is_dc_only gate: eff-M9 only, dead at M4/M5
            );
            let rd = rdcost(self.lambda, part_rate, 0) + ev.block_cost();
            // skip_sub_depth cond1 (svt_aom_pick_partition:11563-11568).
            if split_flag && size <= Self::skip_sub().max_size && self.sub_depth_skip_cond1(&ev) {
                split_flag = false;
            }
            cur = Some((rd, ev));
        }

        if split_flag {
            match self.test_split(scan, abs_x, abs_y, cur.as_ref().map(|c| c.0)) {
                SplitOut::Chosen(res) => return *res,
                SplitOut::ParentKept | SplitOut::Invalid => {
                    // Parent stays; fall through to the leaf commit
                    // (test_split_partition's winner overwrite /
                    // pick_partition:11589-11591).
                }
            }
        }

        let (rd, ev) = cur.expect("refined scan node with neither shape nor valid split");
        commit_leaf(self.fx, self.y_recon, self.y_stride, &ev);
        let decision = crate::partition::funnel_block_decision(ev.to_choice(), size);
        NodeRes {
            rd,
            tree: PartitionTree::Leaf(decision.clone()),
            decisions: alloc::vec![decision],
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
        // use_accurate_part_ctx = 1 at M4/M5: no x2 bias.
        let split_rate = self.part_rates.bits(ctx_row, PartitionType::Split);
        let mut split_cost = rdcost(self.lambda, split_rate, 0);

        let half = size / 2;
        let children = scan.children.as_ref().expect("split_flag children");
        let mut trees: Vec<PartitionTree> = Vec::with_capacity(4);
        let mut decisions: Vec<BlockDecision> = Vec::new();
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
                    return SplitOut::Invalid;
                }
            }
            let cx = abs_x + (i & 1) * half;
            let cy = abs_y + (i >> 1) * half;
            let res = self.pick(child, cx, cy);
            split_cost += res.rd;
            trees.push(res.tree);
            decisions.extend(res.decisions);
        }

        // Final compare (:11375): parent wins on
        // bias * parent_rd <= split_cost * 1000.
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
            let eval = crate::pd0::pd0_pick_sb_partition_m6_eval(&y, 64, 0, 0, qp, qindex, &tables);
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
        let eval = crate::pd0::pd0_pick_sb_partition_m6_eval(&y, 64, 0, 0, 55, 220, &tables);
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
        let eval = crate::pd0::pd0_pick_sb_partition_m6_eval(&y, 64, 0, 0, 20, 80, &tables);
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
