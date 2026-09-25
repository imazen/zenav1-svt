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
use crate::port_enc_mode_config::encdec::SkipSubDepthCtrls;

use svtav1_types::math::rd::rdcost_u64 as rdcost;

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
    /// C `mode == PD0_DEPTH_NO_RESTRICTION` (level 0). C's mode is a
    /// three-state (`md_process.h:227-229`) and this port carried only two,
    /// because the ALLINTRA ladder never selects level 0. The VIDEO ladder
    /// does — at M0 on non-screen content, and at M0..M3 on screen content.
    ///
    /// NO_RESTRICTION differs from ADAPTIVE in exactly one place
    /// (`enc_dec_process.c:1826`): the narrowing block that re-derives
    /// `add_parent_depth` / `add_sub_depth` from the deviation gates is
    /// SKIPPED, so every admitted depth stays admitted. It shares
    /// ADAPTIVE's `s = -2 / e = 2` seed and every clamp above that block
    /// (4x4, `disallow_4x4`, depth removal, `max_block_size`).
    ///
    /// `adaptive` stays `true` alongside it so the PRED_PART_ONLY early
    /// return keeps its meaning.
    pub no_restriction: bool,
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
    /// `use_ref_info` (`enc_mode_config.c` levels 7/8/9 = 1, all lower = 0).
    /// When set, `update_pred_th_offset`'s tail (enc_dec_process.c:1606-1631)
    /// forces s = e = 0 on a SUPERBLOCK-sized node whose co-located reference
    /// SB coded a uniform square (`ref sb_min_sq_size == sb_max_sq_size ==
    /// sq`). Dead on I-slices; live only on inter frames at level >= 7.
    pub use_ref_info: bool,
    /// `pd0_unavail_mode_depth` (M4: 2, M5: 0).
    pub unavail_mode: u8,
    /// `ctx->disallow_4x4` (svt_aom_get_disallow_4x4_allintra,
    /// enc_mode_config.c:11638: <= M3 -> false). Gates the e-depth caps
    /// (set_start_end_depth :1811) and the refined-scan child marking.
    pub disallow_4x4: bool,
    /// `depth_refinement_ctrls.coeff_lvl_modulation` — `1` on every
    /// adaptive level (1-9, enc_mode_config.c:6830-6984), absent at
    /// levels 0/10. Live only on non-I-slices (`set_start_end_depth`
    /// :1865-1870): at NORMAL/HIGH `coeff_lvl` it clamps the admitted
    /// span to `s = MAX(s, -1)` / `e = MIN(e, 1)` — one parent level,
    /// one child level — where the raw -2/+2 seed would let the walk
    /// test a grandparent shape C never prices (e.g. a 32x16 leaf on a
    /// PD0 tree that ran all the way to 8x8).
    pub coeff_lvl_mod: bool,
    /// C `ctx->pic_pred_depth_only` (`enc_mode_config.c:7095`):
    /// `depth_refinement_ctrls.mode == PD0_DEPTH_PRED_PART_ONLY`, which ONLY
    /// `set_block_based_depth_refinement_controls` case 10 (`:6986`) sets.
    /// Read by `set_depth_early_exit_ctrls` (`:7229-7233`), where it forces
    /// `depth_early_exit_lvl` 1 — `early_exit_th` 0 — even at a `pd0_level`
    /// above PD0_LVL_1.
    pub pred_depth_only: bool,
}

/// C `(uint8_t)~0` -> `MIN_SIGNED_VALUE` sentinel for the second-tier
/// (s2/e2) thresholds: the compare always succeeds.
const S2E2_ALWAYS: i64 = i64::MIN;

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
        // 8 slots covers sizes 1..=128 by `trailing_zeros`; only 3..=7 are used.
        snaps: (0..8).map(|_| NodeSnap::default()).collect(),
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
mod tests;

#[cfg(test)]
mod partial_sb_edge_tests;

#[cfg(test)]
mod nsq_mode_table_tests;

mod ctrls;
pub use ctrls::*;

mod scan;
pub(crate) use scan::*;

mod nsq;
pub(crate) use nsq::*;

mod walk;
pub(crate) use walk::*;
