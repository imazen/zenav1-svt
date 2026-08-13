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
use crate::partition::{PartitionTree, PartitionType};
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
    /// `s2_parent_to_current_th` / `e2_sub_to_current_th`. C stores these as
    /// `uint8`; the `(uint8)~0` sentinel maps to `MIN_SIGNED_VALUE` = "always
    /// passes" (levels 5/6/9), while levels 1-4 store a literal `0`. We carry
    /// the resolved i64 threshold directly: `i64::MIN` = the sentinel,
    /// otherwise the literal value. When the sentinel, the second-tier compare
    /// always succeeds (the pre-fix behaviour); a literal `0` admits the extra
    /// parent/child depth only when the deviation is negative.
    pub s2_th: i64,
    pub e2_th: i64,
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

/// C `(uint8_t)~0` -> `MIN_SIGNED_VALUE` sentinel for the second-tier
/// (s2/e2) thresholds: the compare always succeeds.
const S2E2_ALWAYS: i64 = i64::MIN;

impl DrCtrls {
    /// C allintra depth-refinement level derivation
    /// (enc_mode_config.c:10067-10090). The level is keyed on `sc_class5`
    /// (screen-content class 5) AND the preset — NOT the preset alone. This is
    /// a clean switch: the r0-modulation (the non-allintra :9350 block) and
    /// `coeff_lvl_modulation` are absent/dead on the allintra I-slice path.
    ///
    /// ```text
    /// sc_class5:  M0/M1 -> 1, M2 -> 5, M3/M4 -> 6, M5 -> 9, M6+ -> 10 (PRED_PART_ONLY)
    /// !sc_class5: M0..M4 -> 6, M5 -> 9, M6+ -> 10
    /// ```
    /// Verified against the instrumented C `depth_refinement_ctrls.mode`/thresholds:
    /// `graph` (sc_class5) reports level 1/1/5/6/6/9 at p0..p5, `codec_wiki`
    /// (!sc_class5) reports 6/6/6/6/6/9 — the port previously used the
    /// !sc_class5 row for every image, over-pruning the depth descent on
    /// screen content at M0-M2 (e1 15 instead of 200/30).
    pub fn for_preset_sc(preset: u8, sc_class5: bool) -> Self {
        let level: u8 = if sc_class5 {
            match preset {
                0 | 1 => 1,
                2 => 5,
                3 | 4 => 6,
                5 => 9,
                _ => 10,
            }
        } else {
            match preset {
                0..=4 => 6,
                5 => 9,
                _ => 10,
            }
        };
        Self::for_level(level, preset)
    }

    /// Pre-fix entry: the !sc_class5 row (level 6 at M0-M4, 9 at M5, 10 at M6+).
    /// Retained for the unit tests, which assert the non-screen behaviour.
    pub fn for_preset(preset: u8) -> Self {
        Self::for_preset_sc(preset, false)
    }

    /// Build the ctrls for a `set_block_based_depth_refinement_controls` level
    /// (enc_mode_config.c:6816). `disallow_4x4` is preset-based
    /// (`svt_aom_get_disallow_4x4_allintra`, <= M3 -> false), independent of
    /// the level. Only the levels reachable from the allintra derivation
    /// (1, 5, 6, 9, 10) are materialised.
    fn for_level(level: u8, preset: u8) -> Self {
        let disallow_4x4 = preset >= 4;
        match level {
            // case 1: sc_class5 M0/M1. s2/e2 = literal 0 (NOT the sentinel).
            1 => DrCtrls {
                adaptive: true,
                s1_th: 200,
                e1_th: 200,
                s2_th: 0,
                e2_th: 0,
                parent_max_cost_mult: 10,
                band_mod: false,
                max_cost_multiplier: 0,
                max_band_cnt: 1,
                decrement_per_band: [0; 4],
                lower_split_th: 0,
                split_rate_th: 0,
                limit_to_pd0: 0,
                unavail_mode: 2,
                disallow_4x4,
            },
            // case 5: sc_class5 M2. s2/e2 = sentinel (always passes).
            5 => DrCtrls {
                adaptive: true,
                s1_th: 30,
                e1_th: 30,
                s2_th: S2E2_ALWAYS,
                e2_th: S2E2_ALWAYS,
                parent_max_cost_mult: 10,
                band_mod: false,
                max_cost_multiplier: 0,
                max_band_cnt: 1,
                decrement_per_band: [0; 4],
                lower_split_th: 10,
                split_rate_th: 10,
                limit_to_pd0: 2,
                unavail_mode: 2,
                disallow_4x4,
            },
            // case 6: M0-M4 (!sc_class5) and sc_class5 M3/M4.
            6 => DrCtrls {
                adaptive: true,
                s1_th: 15,
                e1_th: 15,
                s2_th: S2E2_ALWAYS,
                e2_th: S2E2_ALWAYS,
                parent_max_cost_mult: 10,
                band_mod: false,
                max_cost_multiplier: 0,
                max_band_cnt: 1,
                decrement_per_band: [0; 4],
                lower_split_th: 20,
                split_rate_th: 10,
                limit_to_pd0: 1,
                unavail_mode: 2,
                disallow_4x4,
            },
            // case 9: M5.
            9 => DrCtrls {
                adaptive: true,
                s1_th: 10,
                e1_th: 10,
                s2_th: S2E2_ALWAYS,
                e2_th: S2E2_ALWAYS,
                parent_max_cost_mult: 0,
                band_mod: true,
                max_cost_multiplier: 400,
                max_band_cnt: 4,
                decrement_per_band: [i64::MAX, i64::MAX, 10, 5],
                lower_split_th: 100,
                split_rate_th: 5,
                limit_to_pd0: 1,
                unavail_mode: 0,
                disallow_4x4,
            },
            // case 10 (M6+): PRED_PART_ONLY — s = e = 0 everywhere.
            _ => DrCtrls {
                adaptive: false,
                s1_th: 0,
                e1_th: 0,
                s2_th: S2E2_ALWAYS,
                e2_th: S2E2_ALWAYS,
                parent_max_cost_mult: 0,
                band_mod: false,
                max_cost_multiplier: 0,
                max_band_cnt: 1,
                decrement_per_band: [0; 4],
                lower_split_th: 0,
                split_rate_th: 0,
                limit_to_pd0: 0,
                unavail_mode: 0,
                disallow_4x4,
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
    /// C `max_sq_size` (enc_dec_process.c:1814-1817):
    /// `ctx->max_block_size`, then `MIN(.., 32)` when
    /// `static_config.max_tx_size == 32`.
    ///
    /// This used to be the literal 64. The port ALREADY derives
    /// `max_tx_size = 32` at tune IQ with qp <= 45 (`hdr_mode.rs`) and threads
    /// it into every PD0 entry, so hardcoding 64 here admitted a shallower
    /// depth than C tests: at `--tune 3`, qp <= 45, presets 0-5 a 32x32 node
    /// got `s = -1` where C gives 0, and a 16x16 node `s = -2` where C gives
    /// -1.
    max_sq: usize,
}

/// C `update_pred_th_offset` (enc_dec_process.c:1545) + the deviation
/// gates, producing this PD0 leaf's admitted (s_depth, e_depth).
/// `parent` is the enclosing square's PD0 eval (None only for the SB
/// root, whose s is forced 0 by the max-size clamp anyway).
// `abs_x`/`abs_y` are consumed only by the std-gated NSQDBG REFINE dump below.
#[cfg_attr(not(feature = "std"), allow(unused_variables))]
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
    // C :1819-1823, against the real `max_sq_size` (see `RefineEnv::max_sq`).
    if sq == env.max_sq {
        s = 0;
    } else if s == -2 && sq * 2 == env.max_sq {
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
            // C `update_pred_th_offset` (enc_dec_process.c:1550) guards on
            // `tested_blk[PART_N][0]`: an incomplete block whose PART_N was
            // never costed has no SQ cost to band.
            if node.sq_tested && node.cost <= max_cost {
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
                // C :1566 `pc_tree->parent->tested_blk[PART_N][0]`.
                if p.sq_tested {
                    let split_cost = rdcost(env.lambda, env.tables.split_bits(p.sq), 0);
                    if split_cost * 10000 < p.cost * ctrls.lower_split_th {
                        s = 0;
                    }
                }
            }
        }
        // split_rate_th (+20, CLN_PD0 :1594-1619): drop the child depth
        // when splitting THIS block is expensive relative to its cost.
        // C :1586 `split_cost_th && pc_tree->tested_blk[PART_N][0]`.
        if ctrls.split_rate_th != 0 && node.sq_tested {
            let th = ctrls.split_rate_th + 20;
            let split_cost = rdcost(env.lambda, env.tables.split_bits(sq), 0);
            if split_cost * 1000 > node.cost * th {
                e = 0;
            }
        }
        // use_ref_info: dead on I-slices (:1623).

        // is_parent_to_current_deviation_small (:1650): only called for
        // tested blocks below the SB size (:1876-1883).
        // C :1859-1861 — "Check tested_blk b/c use block's cost inside".
        if s != 0 && node.sq_tested && sq < 64 {
            // C `is_parent_to_current_deviation_small`'s own
            // `pc_tree->parent->tested_blk[PART_N][0]` guard (:1634).
            match parent.filter(|p| p.sq_tested) {
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
                    } else if dev >= ctrls.s2_th {
                        // s2 = MIN_SIGNED sentinel (levels 5/6/9) -> always
                        // here; s2 = literal 0 (levels 1-4) -> here iff dev>=0.
                        s = -1;
                    } else {
                        // C `MAX(*s_depth, -2)` (:1697): a negative parent
                        // deviation admits the grandparent depth too.
                        s = s.max(-2);
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
        // C :1868-1870 — the same tested_blk guard on the child arm.
        if e != 0 && node.sq_tested && sq > 4 {
            let tested_children: Vec<&Pd0Eval> = node
                .children
                .as_ref()
                // C `is_child_to_current_deviation_small` (:1697-1712) sums
                // `split[i]->block_data[PART_N][0]->cost` only for children
                // whose `tested_blk[PART_N][0]` is set — a boundary child
                // contributes neither cost nor count.
                .map(|ch| ch.iter().filter(|c| c.sq_tested).collect())
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
                } else if dev >= ctrls.e2_th {
                    // e2 = MIN_SIGNED sentinel (levels 5/6/9) -> always here;
                    // e2 = literal 0 (levels 1-4) -> here iff dev>=0.
                    e = 1;
                } else {
                    // C `MIN(*e_depth, 2)` (:1729): a negative child deviation
                    // admits the grandchild depth too.
                    e = e.min(2);
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

    #[cfg(feature = "std")]
    if nsqdbg_here(abs_x, abs_y) {
        let ch_costs: Vec<u64> = node
            .children
            .as_ref()
            .map(|ch| ch.iter().filter(|c| c.sq_tested).map(|c| c.cost).collect())
            .unwrap_or_default();
        eprintln!(
            "NSQDBG REFINE mi=({},{}) sq={} tested={} cost={} pcost={} maxpd0={} minpd0={} sb={} psb={} ch={:?} s={} e={}",
            abs_y / 4,
            abs_x / 4,
            sq,
            u8::from(node.sq_tested),
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
    // `None` = derive the PD0 max/min from THIS root alone. Correct whenever
    // the root spans a whole superblock — i.e. every SB64 case and the tests
    // below. The SB128 pipeline passes the whole-128-SB fold instead (see
    // `build_refined_scan_at`).
    build_refined_scan_at(root, ctrls, lambda, tables, 0, 0, None, 64)
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
    // Whole-superblock PD0 (max, min) block sizes. `None` = derive from
    // `root` alone (the single-64x64-unit case: SB64, or a partial SB128
    // unit). `Some` carries C's WHOLE-128-SB fold for the SB128 refined
    // path: C's `get_max_min_pd0_depths` (enc_dec_process.c:1943) walks the
    // ENTIRE SB pc_tree — at SB128 that is all four 64x64 coding-unit
    // quadrants — so `max_pd0_size`/`min_pd0_size` fed to `set_start_end_depth`
    // span the whole 128 SB, NOT this one 64x64 unit. Computing them
    // per-unit made a quadrant whose PD0 max was 16 (while a sibling quadrant
    // reached 32) cap its shallowest tested depth at 16x16, force-splitting
    // the 32x32 nodes C keeps (`limit_max_min_to_pd0`, :1830-1846). Only bit
    // at SB128 where units.len() > 1; at SB64 the fold equals the root's own
    // max/min, so passing it is byte-identical.
    sb_max_min: Option<(usize, usize)>,
    // C `static_config.max_tx_size` (32 or 64; tune IQ sets 32 at qp <= 45).
    // Caps `max_sq_size` -- see `RefineEnv::max_sq`.
    max_tx_size: u8,
) -> RefScan {
    let mut max_pd0 = 0usize;
    let mut min_pd0 = 255usize;
    if ctrls.limit_to_pd0 != 0 {
        match sb_max_min {
            Some((mx, mn)) => {
                max_pd0 = mx;
                min_pd0 = mn;
            }
            None => root.max_min_picked(&mut max_pd0, &mut min_pd0),
        }
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
        // C: max_block_size (64 on this still/I_SLICE path, below M8) capped
        // to 32 when static_config.max_tx_size == 32 (enc_dec_process.c:1814).
        max_sq: if max_tx_size == 32 { 32 } else { 64 },
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
    /// C `partition_vert_alike_fac_bits[ctx][p == PARTITION_SPLIT]` — the
    /// BOTTOM-edge (`!has_rows`) binary alphabet (md_rate_estimation.c:89-97).
    vert_alike: [[u32; 2]; 16],
    /// C `partition_horz_alike_fac_bits[ctx][p == PARTITION_SPLIT]` — the
    /// RIGHT-edge (`!has_cols`) binary alphabet (md_rate_estimation.c:110-118).
    horz_alike: [[u32; 2]; 16],
}

impl PartRates {
    pub(crate) fn from_fc(fc: &svtav1_entropy::context::FrameContext) -> Self {
        let mut rows = [[0i32; 10]; 16];
        let mut vert_alike = [[0u32; 2]; 16];
        let mut horz_alike = [[0u32; 2]; 16];
        for (row, out) in rows.iter_mut().enumerate() {
            let nsyms = if row < 4 { 4 } else { 10 };
            crate::quant::syntax_rate_from_cdf(&mut out[..nsyms], &fc.partition_cdf[row]);
            // is_128 = false: SB64 squares are <= 64x64. C builds both the
            // 16x16 and the 128x128 gather per context row; the 128 rows are
            // unreachable here (an SB128 refined path would need the `true`
            // variant, which `partition_alike_costs` already takes).
            vert_alike[row] = svtav1_entropy::context::partition_alike_costs(
                &fc.partition_cdf[row],
                true, // !has_rows -> vert_alike (bottom edge)
                false,
            );
            horz_alike[row] = svtav1_entropy::context::partition_alike_costs(
                &fc.partition_cdf[row],
                false, // !has_cols -> horz_alike (right edge)
                false,
            );
        }
        PartRates {
            rows,
            vert_alike,
            horz_alike,
        }
    }

    /// `svt_aom_partition_rate_cost` (rd_cost.c:1834) for in-frame square
    /// blocks (has_rows && has_cols — 64-aligned frames only reach here):
    /// context row from the partition neighbour bytes.
    #[inline]
    pub(crate) fn bits(&self, ctx_row: usize, p: PartitionType) -> u64 {
        debug_assert!(ctx_row < 16);
        self.rows[ctx_row][p as usize] as u64
    }

    /// The FULL `svt_aom_partition_rate_cost` (rd_cost.c:1834-1867), including
    /// the two frame-boundary arms the port previously never reached:
    ///
    /// * `!has_rows && !has_cols` -> 0 (the node codes no partition symbol);
    /// * `!has_rows && has_cols`  -> `partition_vert_alike_fac_bits[ctx][split]`;
    /// * `has_rows && !has_cols`  -> `partition_horz_alike_fac_bits[ctx][split]`.
    ///
    /// On a 64-aligned frame every node has both flags true, so this is exactly
    /// [`PartRates::bits`] there.
    #[inline]
    pub(crate) fn bits_edge(
        &self,
        ctx_row: usize,
        p: PartitionType,
        has_rows: bool,
        has_cols: bool,
    ) -> u64 {
        debug_assert!(ctx_row < 16);
        if has_rows && has_cols {
            return self.rows[ctx_row][p as usize] as u64;
        }
        if !has_rows && !has_cols {
            return 0;
        }
        let table = if !has_rows {
            &self.vert_alike
        } else {
            &self.horz_alike
        };
        table[ctx_row][usize::from(p == PartitionType::Split)] as u64
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
    /// ALIGNED frame dims (the `av1_cm->mi_cols`/`mi_rows` grid, in pixels).
    /// Every `has_rows`/`has_cols` predicate in `set_blocks_to_test`
    /// (enc_dec_process.c:1397-1398), `test_depth` (:10896-10897),
    /// `test_split_partition` (:10805) and `svt_aom_partition_rate_cost`
    /// (rd_cost.c:1831-1832) is against this grid. On a 64-aligned frame every
    /// node has both flags true, so every branch keyed on them is dead.
    pub aligned_w: usize,
    pub aligned_h: usize,
    /// `ctx->nsq_geom_ctrls.enabled` (`svt_aom_get_nsq_geom_level_allintra`,
    /// enc_mode_config.c: allintra enc_mode <= M6 -> level 1/2/3 -> enabled).
    /// Gates the ONE-shape injection at a single-edge node: with geometry off,
    /// `set_blocks_to_test` yields `tot_shapes = 0` and the node force-splits
    /// (enc_dec_process.c:1405-1410).
    pub nsq_geom_enabled: bool,
}

/// C `set_blocks_to_test`'s edge predicate (enc_dec_process.c:1397-1398) — the
/// same `hbs`-against-the-aligned-grid test `test_depth`,
/// `test_split_partition` and `svt_aom_partition_rate_cost` each re-derive.
#[inline]
fn edge_flags(abs_x: usize, abs_y: usize, size: usize, aw: usize, ah: usize) -> (bool, bool) {
    let half = size / 2;
    (abs_y + half < ah, abs_x + half < aw)
}

/// The free form of [`DepthWalk::shapes_at`] — C `set_blocks_to_test`
/// (enc_dec_process.c:1394-1438). Split out so the edge rules are unit-testable
/// without a live funnel context.
fn shapes_at_edge(
    size: usize,
    nsq: &NsqCfg,
    nsq_geom_enabled: bool,
    has_rows: bool,
    has_cols: bool,
) -> &'static [PartitionType] {
    const NONE_AT_ALL: [PartitionType; 0] = [];
    const H_ONLY: [PartitionType; 1] = [PartitionType::Horz];
    const V_ONLY: [PartitionType; 1] = [PartitionType::Vert];
    if has_rows && has_cols {
        return shapes_for_size(size, nsq);
    }
    if (!has_rows && !has_cols) || !nsq_geom_enabled {
        return &NONE_AT_ALL;
    }
    if !has_rows { &H_ONLY } else { &V_ONLY }
}

/// The free form of [`DepthWalk::shape_block_cnt`] — C `test_depth`'s
/// `shape_block_cnt--` (product_coding_loop.c:10899-10904).
///
/// REACHABILITY, MEASURED 2026-08-04 (adversarial re-verification): the
/// `!has_rows || !has_cols` arm is LIVE — deleting it fails partial-SB cells.
/// The two H4/V4 quarter clauses are **inert on every cell measured so far**:
/// with both terms deleted, `tools/partial_sb_gate.sh` still reports 141/141
/// and a further 13 probe geometries chosen to target them (aligned 40/48 on
/// one axis x {96,120,128} on the other, gradient+screen, presets 0-3, where a
/// 64x64 node has both edge flags true yet its 4th quarter starts at/after the
/// aligned extent) are byte-identical to C with or without them. So the clause
/// is a faithful transcription of a C line whose effect no measured cell
/// observes — kept and documented per the "DEAD-LOOKING C STAYS TRANSLATED"
/// rule in rust/CLAUDE.md, NOT because it was seen to fire. Its behaviour is
/// pinned by `partial_sb_edge_tests::shape_block_cnt_drops_out_of_frame_subblocks`
/// alone. If you need it live, note that H4/V4 must first WIN the d1 compare at
/// such a node, which is what the probes did not achieve.
#[allow(clippy::too_many_arguments)]
fn shape_block_cnt_edge(
    size: usize,
    shape: PartitionType,
    n: usize,
    abs_x: usize,
    abs_y: usize,
    aligned_w: usize,
    aligned_h: usize,
    has_rows: bool,
    has_cols: bool,
) -> usize {
    let quarter = size / 4;
    if !has_rows
        || !has_cols
        || (shape == PartitionType::Horz4 && abs_y + 3 * quarter >= aligned_h)
        || (shape == PartitionType::Vert4 && abs_x + 3 * quarter >= aligned_w)
    {
        n - 1
    } else {
        n
    }
}

struct NodeRes {
    /// C `pc_tree->rdc.rd_cost` (partition rate + block/subtree cost).
    rd: u64,
    /// The node's partition tree. It is the ONLY copy of the node's block
    /// decisions: a parallel `decisions: Vec<BlockDecision>` used to be
    /// carried alongside it, deep-cloned leaf by leaf, and the only thing
    /// anyone ever read out of it was its `len()` — which
    /// `PartitionTree::count_leaves` answers without allocating. A
    /// `BlockDecision` owns up to nine `Vec`s, so the duplicate was a full
    /// second allocate+memcpy+free of every coded block in the frame.
    tree: PartitionTree,
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
#[cfg(feature = "std")]
fn nsqdbg_on() -> bool {
    crate::dbgenv::nsqdbg()
}

/// SVTAV1_DBG_MI="mi_row,mi_col": restrict NSQDBG output to the one 64px SB
/// containing that mi (e.g. `64,112`). Unset = whole frame. Frame-wide dumps
/// are ~45 MB / 35k lines on a 512x512 photo; one SB is ~50 lines — always
/// set this when drilling a known divergence (drill_cell.sh does).
#[cfg(feature = "std")]
fn nsqdbg_sb() -> Option<(usize, usize)> {
    static SB: std::sync::OnceLock<Option<(usize, usize)>> = std::sync::OnceLock::new();
    *SB.get_or_init(|| {
        let v = std::env::var("SVTAV1_DBG_MI").ok()?;
        let (r, c) = v.split_once(',')?;
        Some((r.trim().parse().ok()?, c.trim().parse().ok()?))
    })
}

/// Dump gate for a record about the block at pixel (abs_x, abs_y).
#[cfg(feature = "std")]
pub(crate) fn nsqdbg_here(abs_x: usize, abs_y: usize) -> bool {
    nsqdbg_on()
        && match nsqdbg_sb() {
            None => true,
            Some((r, c)) => (abs_y >> 6, abs_x >> 6) == (r >> 4, c >> 4),
        }
}

/// C BLOCK_SIZES enum value of a square block (dump parity).
#[cfg(feature = "std")]
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
#[cfg(feature = "std")]
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
        // bd10 (task #94, root #2): C `calc_scr_to_recon_dist_per_quadrant`
        // (product_coding_loop.c:8065) scores the per-quadrant SSE with
        // `svt_full_distortion_kernel16_bits` at `hbd_md` — the 10-bit source
        // (u8 << 2, the C driver's input) vs the 10-bit `cand_bf->recon`. The
        // ratio the NSQ H/V skip gate (`update_skip_nsq_based_on_sq_recon_dist`)
        // reads is NOT scale-invariant once the 10-bit recon differs from
        // `recon8 << 2` (the hbd-predictor rounding), so scoring it on the u8
        // recon flips the skip on a near-tie. The bd10 recon twin of `gate_y`
        // is `win_recon10` (chroma `win_uv_recon10`) — but ONLY at
        // bypass_encdec=0 (preset <= 3): there `gate_y` is the WINNER's
        // (winning-depth) recon, of which `win_recon10` is the exact twin. At
        // bypass_encdec=1 (preset >= 4) `gate_y` is instead the LAST MDS3
        // candidate's depth-0 recon (no 10-bit twin is kept), so the fix is
        // strictly p0..p3-scoped and p4+ keeps the u8 path. bd8 (empty
        // `win_recon10`) also keeps the u8 path. Both are byte-inert.
        let yrec = ev.gate_y();
        let yrec10 = ev.win_recon10();
        let (urec10, vrec10) = ev.win_uv_recon10();
        // Take the bd10 path only at bypass_encdec=0 AND when every plane the
        // gate reads has its 10-bit recon (luma always; chroma only when the
        // sub-quadrant carries it, `quad > 4`). This can never mix a 10-bit
        // plane with an 8-bit one in a quadrant SSE, and falls back to the
        // byte-inert u8 path on any block whose bd10 recon is absent (rather
        // than panicking).
        let bd10 = !self.fx.frame.cfg.bypass_encdec
            && !yrec10.is_empty()
            && (quad <= 4 || (!urec10.is_empty() && !vrec10.is_empty()));
        for r in 0..2usize {
            for c in 0..2usize {
                let mut d: u64 = 0;
                for y in 0..quad {
                    let sy = (ev.abs_y + r * quad + y) * self.y_src_stride + ev.abs_x + c * quad;
                    let ry = (r * quad + y) * sq + c * quad;
                    for x in 0..quad {
                        let diff = if bd10 {
                            ((self.y_src[sy + x] as i64) << 2) - yrec10[ry + x] as i64
                        } else {
                            self.y_src[sy + x] as i64 - yrec[ry + x] as i64
                        };
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
                            let (du, dv) = if bd10 {
                                (
                                    ((self.fx.u_src[sy + x] as i64) << 2) - urec10[ry + x] as i64,
                                    ((self.fx.v_src[sy + x] as i64) << 2) - vrec10[ry + x] as i64,
                                )
                            } else {
                                (
                                    self.fx.u_src[sy + x] as i64 - urec[ry + x] as i64,
                                    self.fx.v_src[sy + x] as i64 - vrec[ry + x] as i64,
                                )
                            };
                            d += (du * du) as u64 + (dv * dv) as u64;
                        }
                    }
                }
                dists[r * 2 + c] = d;
            }
        }
        #[cfg(feature = "std")]
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

    /// Save/restore span of a node rect on the ALIGNED-strided recon planes.
    /// A node whose square extent STRADDLES past the aligned width would, at
    /// stride `y_stride`, read/write the off-aligned columns out of the row and
    /// into the NEXT row's low columns. `commit_leaf` already clips its writes
    /// to the row boundary for exactly that reason (leaf_funnel.rs:7705), so
    /// nothing inside the node ever modifies those wrapped bytes and the
    /// snapshot must clip identically — otherwise `restore_snap` would write
    /// stale bytes over an already-committed neighbour's recon. Byte-neutral
    /// wherever nothing straddles (`abs + span <= stride`), i.e. always on a
    /// 64-aligned frame.
    #[inline]
    fn clip_span(stride: usize, abs: usize, span: usize) -> usize {
        span.min(stride.saturating_sub(abs))
    }

    fn take_snap(&self, abs_x: usize, abs_y: usize, size: usize) -> NodeSnap {
        let yw = Self::clip_span(self.y_stride, abs_x, size);
        let mut y = alloc::vec![0u8; size * size];
        for r in 0..size {
            let src = (abs_y + r) * self.y_stride + abs_x;
            y[r * size..r * size + yw].copy_from_slice(&self.y_recon[src..src + yw]);
        }
        let half = size / 2;
        let (cx, cy) = (abs_x / 2, abs_y / 2);
        let cwid = Self::clip_span(self.fx.c_stride, cx, half);
        let mut u = alloc::vec![0u8; half * half];
        let mut v = alloc::vec![0u8; half * half];
        for r in 0..half {
            let src = (cy + r) * self.fx.c_stride + cx;
            u[r * half..r * half + cwid].copy_from_slice(&self.fx.u_recon[src..src + cwid]);
            v[r * half..r * half + cwid].copy_from_slice(&self.fx.v_recon[src..src + cwid]);
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
        let yw = Self::clip_span(self.y_stride, abs_x, size);
        for r in 0..size {
            let dst = (abs_y + r) * self.y_stride + abs_x;
            self.y_recon[dst..dst + yw].copy_from_slice(&snap.y[r * size..r * size + yw]);
        }
        let half = size / 2;
        let (cx, cy) = (abs_x / 2, abs_y / 2);
        let cwid = Self::clip_span(self.fx.c_stride, cx, half);
        for r in 0..half {
            let dst = (cy + r) * self.fx.c_stride + cx;
            self.fx.u_recon[dst..dst + cwid].copy_from_slice(&snap.u[r * half..r * half + cwid]);
            self.fx.v_recon[dst..dst + cwid].copy_from_slice(&snap.v[r * half..r * half + cwid]);
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

    /// C `set_blocks_to_test` (enc_dec_process.c:1394-1438) for this node —
    /// the d1 shape list, honouring the frame-boundary rules. Returns an EMPTY
    /// list for C's `tot_shapes = 0` (forced SPLIT).
    ///
    /// The three C rules this reproduces, none of which can fire on a
    /// 64-aligned frame (both flags are always true there, so the function
    /// degenerates to `shapes_for_size`):
    ///  * both flags false -> `tot_shapes = 0` (:1405-1410);
    ///  * exactly one false, NSQ geometry OFF -> also `tot_shapes = 0` (same
    ///    clause; the `sq_size <= MAX(min_nsq, min_nsq_block_size)` term is
    ///    inert here — an edge node on an 8-aligned frame is always >= 16);
    ///  * exactly one false, NSQ geometry ON -> `inj_hv_incomp` keeps EXACTLY
    ///    ONE shape and EXCLUDES PARTITION_NONE (:1417-1421): PART_H when
    ///    `!has_rows`, PART_V when `!has_cols`. Note `max_part` is PART_V for
    ///    an incomplete node even when `md_disallow_nsq_search` is set (:1414
    ///    ANDs that term with `!inj_hv_incomp`), so presets 4/5 — whose NSQ
    ///    SEARCH is off — still inject the edge shape.
    fn shapes_at(&self, size: usize, has_rows: bool, has_cols: bool) -> &'static [PartitionType] {
        shapes_at_edge(size, self.nsq, self.nsq_geom_enabled, has_rows, has_cols)
    }

    /// C `test_depth`'s `shape_block_cnt` adjustment (product_coding_loop.c:
    /// 10899-10904): a single-edge node codes only the FIRST rect of its
    /// injected shape (the in-frame half), and an H4/V4 whose 4th quarter
    /// starts outside the aligned frame drops that quarter.
    fn shape_block_cnt(
        &self,
        size: usize,
        shape: PartitionType,
        n: usize,
        abs_x: usize,
        abs_y: usize,
        has_rows: bool,
        has_cols: bool,
    ) -> usize {
        shape_block_cnt_edge(
            size,
            shape,
            n,
            abs_x,
            abs_y,
            self.aligned_w,
            self.aligned_h,
            has_rows,
            has_cols,
        )
    }

    /// C `svt_aom_pick_partition` (product_coding_loop.c:11549) —
    /// test_depth (:11396, the d1 shape loop) + the sub-depth walk.
    /// `None` mirrors C's `pc_tree->rdc.valid == 0` return: the node produced
    /// no valid partition at all, which invalidates the parent's SPLIT.
    fn pick(&mut self, scan: &RefScan, abs_x: usize, abs_y: usize) -> Option<NodeRes> {
        let size = scan.sq;
        let mut split_flag = scan.split_flag;
        let (has_rows, has_cols) = edge_flags(abs_x, abs_y, size, self.aligned_w, self.aligned_h);

        // C test_depth state: rdc (best partition so far), the SQ info,
        // the H/V child costs for the H4/V4 gates, and the winning
        // shape's evaluations for the final commit.
        let mut best: Option<(PartitionType, u64, Vec<LeafEval>)> = None;
        let mut sq_info: Option<SqInfo> = None;
        let mut h_children: Option<[(u64, bool); 2]> = None;
        let mut v_children: Option<[(u64, bool); 2]> = None;
        let mut snap: Option<NodeSnap> = None;
        let mut committed_since_snap = false;

        let shapes = self.shapes_at(size, has_rows, has_cols);
        // C `svt_aom_pick_partition`: `if (mds->tot_shapes) test_depth(...)`.
        // An empty list is `tot_shapes = 0` — the node force-splits.
        if scan.test_this && !shapes.is_empty() {
            // update_part_neighs: partition contexts read once per node.
            let (ctx_row, _) = self.fx.ectx.partition_ctx(abs_x, abs_y, size);

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
                    self.part_rates
                        .bits_edge(ctx_row, shape, has_rows, has_cols)
                } else {
                    0
                };
                let mut part_cost = rdcost(self.lambda, part_rate, 0);
                let mut children = shape_children(size, shape);
                // C `shape_block_cnt--` (product_coding_loop.c:10899-10904):
                // drop the trailing out-of-frame sub-block. Inert on a
                // 64-aligned frame (both flags true, H4/V4 quarters in-frame).
                children.truncate(self.shape_block_cnt(
                    size,
                    shape,
                    children.len(),
                    abs_x,
                    abs_y,
                    has_rows,
                    has_cols,
                ));
                let mut evals: Vec<LeafEval> = Vec::with_capacity(children.len());
                let mut valid = true;

                for (nsi, &(dx, dy, cw, ch)) in children.iter().enumerate() {
                    // C `get_skip_processing_nsq_block`'s four gates each
                    // return false when `pc_tree->tested_blk[PART_N][0]` is
                    // unset (product_coding_loop.c:9717, 9852, 10067, and
                    // `update_skip_nsq_shapes`), which is exactly the
                    // single-edge node where PART_N is never tested.
                    if shape != PartitionType::None && nsi == 0 && sq_info.is_some() {
                        // faster_md_settings_nsq: I-slice-dead (C gates
                        // the call on slice_type != I_SLICE, :11470).
                        let sq = sq_info.as_ref().expect("checked above");
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
                            #[cfg(feature = "std")]
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
                    // IBC chunk 8: the do_intra_bc gate inputs for this
                    // leaf (mode_decision.c:3597-3616) — the shape under
                    // evaluation + this node's PART_N (square) winner.
                    self.fx.ibc_gate = crate::leaf_funnel::IbcGateInput {
                        partition: shape as u8,
                        is_part_n: shape == PartitionType::None,
                        sibling_n0: match &sq_info {
                            Some(sq) => (true, sq.ev.used_ibc()),
                            None => (false, false),
                        },
                    };
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
                    #[cfg(feature = "std")]
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
                            #[cfg(feature = "std")]
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
                        commit_leaf(self.fx, self.y_recon, self.y_stride, ev, shape as u8);
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
                            self.fx.frame.bit_depth,
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
                #[cfg(feature = "std")]
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
                SplitOut::Chosen(res) => return Some(*res),
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
        // C `svt_aom_pick_partition` returns `pc_tree->rdc.valid` — 0 when the
        // node tested no shape AND its SPLIT was invalid. Reachable only on a
        // partial SB: a `set_child_to_be_tested`-created child (split_flag
        // false, tot_shapes forced 1) that lands on a BOTH-false node, where
        // `set_blocks_to_test` then zeroes tot_shapes and there is nothing to
        // fall back to. The parent's SPLIT is invalidated
        // (product_coding_loop.c:10826-10829), which keeps its own depth.
        let (win_part, win_rd, win_evals) = best?;
        if win_part == PartitionType::None {
            let sq = sq_info.expect("SQ info for PART_N winner");
            commit_leaf(
                self.fx,
                self.y_recon,
                self.y_stride,
                &sq.ev,
                PartitionType::None as u8,
            );
            let decision = crate::partition::funnel_block_decision(sq.ev.into_choice(), size, size);
            return Some(NodeRes {
                rd: win_rd,
                tree: PartitionTree::Leaf(decision),
            });
        }
        let mut child_trees: Vec<PartitionTree> = Vec::with_capacity(win_evals.len());
        for ev in win_evals {
            commit_leaf(self.fx, self.y_recon, self.y_stride, &ev, win_part as u8);
            let (ew, eh) = (ev.w, ev.h);
            let d = crate::partition::funnel_block_decision(ev.into_choice(), ew, eh);
            child_trees.push(PartitionTree::Leaf(d));
        }
        Some(NodeRes {
            rd: win_rd,
            tree: PartitionTree::Split {
                partition_type: win_part,
                width: size as u16,
                height: size as u16,
                children: child_trees,
            },
        })
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
        let (has_rows, has_cols) = edge_flags(abs_x, abs_y, size, self.aligned_w, self.aligned_h);
        // use_accurate_part_ctx = 1: no x2 bias.
        // The SPLIT rate is the boundary BINARY alphabet's at a one-false node
        // and 0 at a both-false node — C calls the same
        // `svt_aom_partition_rate_cost` here as everywhere else
        // (product_coding_loop.c:10784-10791). Identical to `bits` on a
        // 64-aligned frame.
        let split_rate =
            self.part_rates
                .bits_edge(ctx_row, PartitionType::Split, has_rows, has_cols);
        let mut split_cost = rdcost(self.lambda, split_rate, 0);

        let half = size / 2;
        let children = scan.children.as_ref().expect("split_flag children");
        let mut trees: Vec<PartitionTree> = Vec::with_capacity(4);
        let mut child_rd = [0u64; 4]; // NSQDBG only: per-quadrant pick() RD
        for (i, child) in children.iter().enumerate() {
            let cx = abs_x + (i & 1) * half;
            let cy = abs_y + (i >> 1) * half;
            // C `test_split_partition` (product_coding_loop.c:10802-10808):
            // "if block fully outside pic, don't process" — the quadrant is
            // skipped BEFORE the early-exit compare, so the next in-frame
            // quadrant still sees its own `i` (and hence the 1000 threshold).
            // Never taken on a 64-aligned frame.
            if cx >= self.aligned_w || cy >= self.aligned_h {
                continue;
            }
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
                    #[cfg(feature = "std")]
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
            // C: `if (!valid_split_partition) return false;` — every quadrant
            // must produce a valid partition for SPLIT to be selectable
            // (product_coding_loop.c:10825-10829).
            let Some(res) = self.pick(child, cx, cy) else {
                return SplitOut::Invalid;
            };
            child_rd[i] = res.rd;
            split_cost += res.rd;
            trees.push(res.tree);
        }

        // Final compare (:11375): parent wins on
        // bias * parent_rd <= split_cost * 1000.
        #[cfg(feature = "std")]
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
    // ALIGNED frame dims + `nsq_geom_ctrls.enabled` — the partial-SB edge
    // rules. On a 64-aligned frame every predicate keyed on them is true and
    // this walk is byte-identical to the pre-#95 one.
    aligned_w: usize,
    aligned_h: usize,
    nsq_geom_enabled: bool,
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
        aligned_w,
        aligned_h,
        nsq_geom_enabled,
    };
    // The SB root is always in-frame and always splittable, so C's
    // `rdc.valid == 0` return cannot reach the top of a superblock.
    let res = walk
        .pick(scan, sb_x, sb_y)
        .expect("SB root produced no valid partition");
    let num_blocks = res.tree.count_leaves() as u32;
    crate::partition::PartitionResult {
        partition_type: match &res.tree {
            PartitionTree::Leaf(_) => PartitionType::None,
            _ => PartitionType::Split,
        },
        rd_cost: res.rd,
        distortion: 0,
        rate: 0,
        decisions: alloc::vec::Vec::new(),
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

    #[test]
    fn dr_ctrls_sc_class5_level_mapping() {
        // C allintra derivation (enc_mode_config.c:10067-10090), verified
        // against the instrumented `ctx->depth_refinement_ctrls` dump:
        //   graph (sc_class5):  p0/p1 -> lvl1, p2 -> lvl5, p3/p4 -> lvl6, p5 -> lvl9
        //   codec_wiki (!sc):   p0..p4 -> lvl6, p5 -> lvl9
        // sc_class5 M0/M1 = level 1: s1=e1=200, s2=e2=0 (NOT the sentinel),
        // split_rate_th=0, limit=0.
        for p in [0u8, 1] {
            let c = DrCtrls::for_preset_sc(p, true);
            assert!(c.adaptive);
            assert_eq!((c.s1_th, c.e1_th), (200, 200));
            assert_eq!((c.s2_th, c.e2_th), (0, 0), "level 1 s2/e2 are literal 0");
            assert_eq!((c.split_rate_th, c.limit_to_pd0), (0, 0));
        }
        // sc_class5 M2 = level 5: s1=e1=30, s2=e2=sentinel, limit=2, lower=10.
        let c = DrCtrls::for_preset_sc(2, true);
        assert!(c.adaptive);
        assert_eq!((c.s1_th, c.e1_th), (30, 30));
        assert_eq!((c.s2_th, c.e2_th), (i64::MIN, i64::MIN));
        assert_eq!((c.lower_split_th, c.limit_to_pd0), (10, 2));
        // sc_class5 M3/M4 = level 6, same as the !sc row.
        for p in [3u8, 4] {
            let sc = DrCtrls::for_preset_sc(p, true);
            assert_eq!((sc.s1_th, sc.e1_th), (15, 15));
            assert_eq!((sc.limit_to_pd0, sc.lower_split_th), (1, 20));
        }
        // sc_class5 M5 = level 9 (same band-modulated ctrls as !sc M5).
        let sc5 = DrCtrls::for_preset_sc(5, true);
        assert!(sc5.band_mod);
        assert_eq!((sc5.s1_th, sc5.e1_th), (10, 10));
        // !sc_class5 keeps the pre-fix per-preset row for every preset, so the
        // whole non-screen envelope (every mainline gate) is byte-identical.
        for p in 0..=6u8 {
            let a = DrCtrls::for_preset_sc(p, false);
            let b = DrCtrls::for_preset(p);
            assert_eq!((a.s1_th, a.e1_th, a.adaptive), (b.s1_th, b.e1_th, b.adaptive));
            assert_eq!((a.limit_to_pd0, a.split_rate_th), (b.limit_to_pd0, b.split_rate_th));
        }
        // The screen and non-screen rows differ exactly at M0/M1/M2.
        for p in [0u8, 1, 2] {
            assert_ne!(
                DrCtrls::for_preset_sc(p, true).e1_th,
                DrCtrls::for_preset_sc(p, false).e1_th,
                "sc_class5 must lower e1 at M{p}"
            );
        }
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
                crate::pd0::pd0_pick_sb_partition_m6_eval(&y, 64, 0, 0, qp, qindex, &tables, 8, 1, false, true, 64, 64, 0, 0, None, 64);
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
        let eval = crate::pd0::pd0_pick_sb_partition_m6_eval(&y, 64, 0, 0, 55, 220, &tables, 8, 1, false, true, 64, 64, 0, 0, None, 64);
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
        let eval = crate::pd0::pd0_pick_sb_partition_m6_eval(&y, 64, 0, 0, 20, 80, &tables, 8, 1, false, true, 64, 64, 0, 0, None, 64);
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
            sq_tested: true,
            cost: 100,
            split: true,
            off: false,
            children: Some(Box::new([
                Pd0Eval {
                    sq: 32,
                    tested: true,
                    sq_tested: true,
                    cost: 25,
                    split: false,
                    off: false,
                    children: None,
                },
                Pd0Eval {
                    sq: 32,
                    tested: true,
                    sq_tested: true,
                    cost: 25,
                    split: false,
                    off: false,
                    children: None,
                },
                Pd0Eval {
                    sq: 32,
                    tested: true,
                    sq_tested: true,
                    cost: 25,
                    split: false,
                    off: false,
                    children: None,
                },
                Pd0Eval {
                    sq: 32,
                    tested: true,
                    sq_tested: true,
                    cost: 25,
                    split: false,
                    off: false,
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

    /// `max_sq_size` must follow `static_config.max_tx_size`
    /// (enc_dec_process.c:1814-1817), not the literal 64 it was hardcoded to.
    ///
    /// C caps `max_sq_size` to 32 when `max_tx_size == 32`, which the port
    /// already derives at tune IQ with qp <= 45 (`hdr_mode.rs`) and threads
    /// into every PD0 entry. With the cap missing, a 32x32 node was allowed
    /// `s_depth = -1` (test a 64x64 parent) where C forces 0, admitting a
    /// shallower depth C never tests.
    ///
    /// ANTI-VACUITY: this asserts the two `max_tx_size` values produce
    /// DIFFERENT scans. With the old hardcoded 64 both arms are identical and
    /// the final assert fails.
    #[test]
    fn max_sq_size_follows_max_tx_size() {
        // A 32x32 PD0 leaf that was tested and not split: `set_start_end_depth`
        // may hand it a parent depth unless `sq == max_sq`.
        let leaf32 = |cost: u64| Pd0Eval {
            sq: 32,
            tested: true,
            sq_tested: true,
            cost,
            split: false,
            off: false,
            children: None,
        };
        let eval = Pd0Eval {
            sq: 64,
            tested: true,
            sq_tested: true,
            cost: 100,
            split: true,
            off: false,
            children: Some(Box::new([leaf32(25), leaf32(25), leaf32(25), leaf32(25)])),
        };
        // A preset whose refinement mode is ADAPTIVE (so s_depth can be nonzero
        // at all) -- presets 0-5 per DrCtrls::for_preset.
        let ctrls = DrCtrls::for_preset(4);
        let tables = crate::pd0::build_m6_pd0_tables(160);

        let scan64 = build_refined_scan_at(&eval, &ctrls, 248207, &tables, 0, 0, None, 64);
        let scan32 = build_refined_scan_at(&eval, &ctrls, 248207, &tables, 0, 0, None, 32);

        // At max_tx_size 32 the 32x32 nodes ARE the max square, so C forces
        // s_depth = 0 -- they must not request their 64x64 parent.
        let parent_tested = |sc: &RefScan| sc.test_this;
        assert!(
            parent_tested(&scan64) != parent_tested(&scan32),
            "max_tx_size must change whether the 64x64 parent is admitted \
             (64 -> {}, 32 -> {}); if these agree, the max_sq cap is not wired",
            parent_tested(&scan64),
            parent_tested(&scan32)
        );
        assert!(
            !parent_tested(&scan32),
            "at max_tx_size 32 a 32x32 node is the max square: C forces s_depth = 0"
        );
    }
}

#[cfg(test)]
mod partial_sb_edge_tests {
    use super::*;

    /// C `set_blocks_to_test` (enc_dec_process.c:1394-1438) at a frame
    /// boundary. This is the rule the PD1 walk was missing entirely: before
    /// the partial-SB fix, presets 0..=5 never ran this walk on an incomplete
    /// superblock at all (`pipeline.rs`'s `refined` required `full_sb`), so a
    /// non-64-aligned frame took a structurally different search than C's.
    ///
    /// ANTI-VACUITY: every assert below is about a `has_rows`/`has_cols` FALSE
    /// case. On a 64-aligned frame both are always true and only the two
    /// interior asserts are exercised — which is exactly why the aligned gate
    /// stays at 1036/1036 while the partial cells moved 72 -> 187 of 216.
    #[test]
    fn set_blocks_to_test_edge_rules() {
        let nsq_on = NsqCfg::for_preset_qp(3, 20); // NSQ SEARCH on (preset <= 3)
        let nsq_off = NsqCfg::off(); // presets 4/5: md_disallow_nsq_search
        // Interior node, NSQ search on: the full N/H/V/H4/V4 list.
        assert_eq!(shapes_at_edge(32, &nsq_on, true, true, true).len(), 5);
        // Interior node, NSQ search off: PART_N only.
        assert_eq!(
            shapes_at_edge(32, &nsq_off, true, true, true),
            &[PartitionType::None]
        );
        // BOTH flags false -> tot_shapes = 0 -> forced SPLIT (:1405-1410).
        assert!(shapes_at_edge(32, &nsq_on, true, false, false).is_empty());
        assert!(shapes_at_edge(32, &nsq_off, true, false, false).is_empty());
        // BOTTOM edge (!has_rows) -> EXACTLY PART_H, PARTITION_NONE EXCLUDED
        // (:1417-1421).
        assert_eq!(
            shapes_at_edge(32, &nsq_on, true, false, true),
            &[PartitionType::Horz]
        );
        // RIGHT edge (!has_cols) -> EXACTLY PART_V.
        assert_eq!(
            shapes_at_edge(32, &nsq_on, true, true, false),
            &[PartitionType::Vert]
        );
        // The edge shape is injected even when the NSQ *search* is disabled:
        // C ANDs `md_disallow_nsq_search` with `!inj_hv_incomp` (:1414). That
        // is what lets presets 4/5 code a boundary rect at all.
        assert_eq!(
            shapes_at_edge(32, &nsq_off, true, false, true),
            &[PartitionType::Horz]
        );
        // NSQ GEOMETRY off (allintra enc_mode > M6) -> no shape is injected and
        // the node force-splits (the presets >= 7 rule, same clause :1405).
        assert!(shapes_at_edge(32, &nsq_on, false, false, true).is_empty());
    }

    /// C `test_depth`'s `shape_block_cnt--` (product_coding_loop.c:10899-10904).
    #[test]
    fn shape_block_cnt_drops_out_of_frame_subblocks() {
        // Interior 64x64 in a 128x128 aligned frame: nothing dropped.
        assert_eq!(
            shape_block_cnt_edge(64, PartitionType::Horz, 2, 0, 0, 128, 128, true, true),
            2
        );
        assert_eq!(
            shape_block_cnt_edge(64, PartitionType::Horz4, 4, 0, 0, 128, 128, true, true),
            4
        );
        // A single-edge node codes ONLY the first rect (the in-frame half).
        assert_eq!(
            shape_block_cnt_edge(64, PartitionType::Horz, 2, 0, 64, 128, 88, false, true),
            1
        );
        assert_eq!(
            shape_block_cnt_edge(64, PartitionType::Vert, 2, 64, 0, 72, 128, true, false),
            1
        );
        // H4 at y = 0 in a 48-tall aligned frame: has_rows is TRUE (0+32 < 48),
        // yet the 4th quarter starts at y = 48 == aligned_h, so it is dropped.
        assert_eq!(
            shape_block_cnt_edge(64, PartitionType::Horz4, 4, 0, 0, 64, 48, true, true),
            3
        );
        // The V4 mirror on a 48-wide aligned frame.
        assert_eq!(
            shape_block_cnt_edge(64, PartitionType::Vert4, 4, 0, 0, 48, 64, true, true),
            3
        );
    }

    /// C `svt_aom_partition_rate_cost`'s boundary arms (rd_cost.c:1846-1866).
    /// The port's `PartRates` only ever indexed the full 10-symbol alphabet; at
    /// a boundary node C prices the BINARY split-vs-{H,V} alphabet instead, and
    /// BOTH entries are live (entry 0 is `test_depth`'s `part_rate` for the
    /// injected rect, and `update_skip_nsq_based_on_split_rate` reads it too).
    #[test]
    fn partition_rate_uses_the_boundary_alphabet() {
        let fc = svtav1_entropy::context::FrameContext::new_default();
        let r = PartRates::from_fc(&fc);
        // 32x32 -> bsl 2 -> ctx row 8 (left = above = 0).
        let row = 8usize;
        let full_split = r.bits(row, PartitionType::Split);
        let full_horz = r.bits(row, PartitionType::Horz);
        // Interior: unchanged, the full alphabet.
        assert_eq!(
            r.bits_edge(row, PartitionType::Split, true, true),
            full_split
        );
        // Both false: C returns 0 — the node codes no partition symbol.
        assert_eq!(r.bits_edge(row, PartitionType::Split, false, false), 0);
        assert_eq!(r.bits_edge(row, PartitionType::Horz, false, false), 0);
        // Bottom edge: the vert_alike binary pair. SPLIT and the rect must
        // differ from each other and from the full-alphabet costs, else the
        // whole boundary-rate distinction would be vacuous.
        let b_split = r.bits_edge(row, PartitionType::Split, false, true);
        let b_rect = r.bits_edge(row, PartitionType::Horz, false, true);
        assert_ne!(b_split, b_rect);
        assert_ne!(b_split, full_split);
        assert_ne!(b_rect, full_horz);
        // Right edge uses the OTHER gather, and the two tables are genuinely
        // different (C's horz/vert alike gathers are asymmetric).
        let r_split = r.bits_edge(row, PartitionType::Split, true, false);
        let r_rect = r.bits_edge(row, PartitionType::Vert, true, false);
        assert_ne!(r_split, b_split);
        assert_ne!(r_rect, b_rect);
        // The boundary table is keyed on `p == PARTITION_SPLIT` ONLY, so every
        // non-SPLIT symbol collapses onto the same rect entry.
        assert_eq!(r.bits_edge(row, PartitionType::None, true, false), r_rect);
        assert_eq!(r.bits_edge(row, PartitionType::Horz4, true, false), r_rect);
    }

    /// C's `tested_blk[PART_N][0]` guard on every PD1 refinement gate
    /// (enc_dec_process.c:1550, 1566, 1586, 1634, 1698-1711, 1859, 1868),
    /// spelled out in C's own comment at :1547 — "For incomplete blocks, H/V
    /// partitions may be allowed, while square is not ... check that the SQ
    /// block is available before using the cost."
    ///
    /// A PD0 leaf that only ever costed its boundary rect must therefore get
    /// `s_depth = e_depth = 0`: it is coded at its own depth, never refined.
    /// Without the `Pd0Eval::sq_tested` distinction the rect cost was fed to
    /// the deviation gates as if it were a square PART_N cost.
    #[test]
    fn boundary_pd0_leaf_is_never_refined() {
        let tables = crate::pd0::build_m6_pd0_tables(160);
        let ctrls = DrCtrls::for_preset(4); // ADAPTIVE, s1 = e1 = 15
        let kid16 = || crate::pd0::Pd0Eval {
            sq: 16,
            tested: true,
            sq_tested: true,
            cost: 5_000_000,
            split: false,
            off: false,
            children: None,
        };
        // A PD0 leaf at 32x32 whose four visited 16x16 children came in far
        // cheaper — `is_child_to_current_deviation_small` admits the child
        // depth (e = 1) whenever the SQ cost is available.
        let leaf32 = |sq_tested: bool| crate::pd0::Pd0Eval {
            sq: 32,
            tested: true,
            sq_tested,
            cost: 100_000_000,
            split: false,
            off: false,
            children: Some(alloc::boxed::Box::new([kid16(), kid16(), kid16(), kid16()])),
        };
        let mk = |sq_tested: bool| crate::pd0::Pd0Eval {
            sq: 64,
            tested: true,
            sq_tested: true,
            cost: 400_000_000,
            split: true,
            off: false,
            children: Some(alloc::boxed::Box::new([
                leaf32(sq_tested),
                leaf32(true),
                leaf32(true),
                leaf32(true),
            ])),
        };
        let with_sq = build_refined_scan(&mk(true), &ctrls, 25650, &tables);
        let boundary = build_refined_scan(&mk(false), &ctrls, 25650, &tables);
        let q0 = |s: &RefScan| {
            let c = s.children.as_ref().expect("root splits");
            (c[0].split_flag, c[0].children.is_some())
        };
        assert_eq!(
            q0(&with_sq),
            (true, true),
            "a square-costed PD0 leaf admits the child depth"
        );
        assert_eq!(
            q0(&boundary),
            (false, false),
            "a boundary PD0 leaf has no SQ cost: C skips both deviation gates"
        );
    }
}
