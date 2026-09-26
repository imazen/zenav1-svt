//! The `scs->allintra` fork for `pcs->nic_level` — how many candidates survive
//! each MD stage, and how hard the deviation prunes bite.
//!
//! Sibling of [`crate::rate_arm`], [`crate::part_arm`], [`crate::intra_arm`]
//! and [`crate::funnel_arm`]. The ladder pair is
//! `svt_aom_get_nic_level_{allintra,default}` (`enc_mode_config.c:4488` /
//! `:4451`), already ported in [`leaf`] and tier-1 gated; the control table is
//! `svt_aom_set_nic_controls` (`:4518`), whose rows this module transcribes as
//! the [`FunnelCfg`] fields the funnel reads.
//!
//! On a KEY frame (`is_islice`, `is_base`) the arms pick:
//!
//! | preset | allintra | video |
//! |---|---|---|
//! | 0 | 1 | 2 |
//! | 1 | 3 | 4 |
//! | 2 | 3 | 5 |
//! | 3 | 5 | 5 |
//! | 4..=5 | 5 | 7 |
//! | 6 | 6 | **8** |
//! | 7 | 7 | 8 |
//! | 8 | 11 | 9 |
//! | 9..=11 | 11 (clamped M9) | 9 / 11 |
//!
//! At the inter campaign's reference preset the video arm is much TIGHTER:
//! level 8 scales the stage counts by 13 (`{2, 1, 1}` of 16) where level 6
//! scales by 6 (`{6, 6, 6}`), and it cuts `mds1_cand_base_th_intra` 1200 -> 300
//! and both later candidate thresholds 15 -> 3. This is the first of the
//! campaign's *pruning* ladders — every arm chunk before it widened the search,
//! which is why the port had drifted to UNDER-shooting C's byte count on
//! textured content.
//!
//! `enable_skipping_mds1` (1 at levels 8..=11, 0 below) is carried as
//! [`FunnelCfg::enable_skipping_mds1`] since 2026-09-04. C reads it in exactly
//! one place — `post_mds0_nic_pruning` clears `ctx->perform_mds1` when the
//! flag is set AND the post-prune stage-1 total is 1
//! (product_coding_loop.c:7879) — and `md_encode_block` then runs NO stage-1
//! full loop on that block (:9551). The pick cannot differ (MDS3 recomputes
//! the one survivor's full cost either way) but the WORK does: on `gradient
//! 512x512 qp40 p10` C's `perform_mds1` is 0 on all 886 leaves and the
//! port's unconditional `run_mds1` was 17.9 M of its 176.6 M Ir (callgrind,
//! r7900x, `benchmarks/callcount_mds1skip_2026-09-04.meta`). See
//! `docs/INTER-ENCODE-PLAN.md` §1z³⁹.
//!
//! `mds1_class_th` / `mds2_class_th` and the INTER half of the post-MDS0
//! candidate threshold (`mds1_cand_base_th_inter`) ARE carried, since
//! 2026-09-03. They were previously omitted with the reasoning that
//! `post_mds0_nic_pruning` / `post_mds1_nic_pruning` force both class
//! thresholds to the `(uint64_t)~0` sentinel on an I_SLICE
//! (`product_coding_loop.c:7826` / `:7897`) "and every picture this port
//! encodes is an I-slice". **That premise stopped holding when the port
//! started encoding inter frames.** On a P/B slice both are live, and they do
//! not trim a class — they DELETE it. Measured on `diag 72x72 q55 p6` frame 1,
//! block (64,32) 16x32, C at nic level 8 (`SVT_FULLCOST_OUT`, which now dumps
//! the per-class stage counts):
//!
//! ```text
//! n0 = 29,6,3,0,0   candidates injected per class
//! n1 =  0,3,3,0,0   after post_mds0  -> mds1_class_th KILLED all 29 intra
//! n2 =  0,1,0,0,0   after post_mds1  -> mds2_class_th KILLED the NEWMV class
//! n3 =  0,1,0,0,0   MDS3 evaluates ONE candidate
//! ```
//!
//! `merge_inter_cands_mult` is inter-only — it rewrites the candidate CLASS
//! (`leaf_funnel::nic::lane_of` reads the stamped `cand_class`), so it is a
//! no-op on an all-intra frame but live on every P/B one.
//!
//! # Evidence
//!
//! Tier 1 on the LADDER (`svt_aom_get_nic_level_{allintra,default}` are both
//! EXPORTED and driven by `tests/c_parity_sig_deriv_leaf.rs`; the video arm is
//! additionally read back as `pcs->nic_level` by
//! `tests/c_parity_sig_deriv_md_config.rs`). The control table is transcribed
//! here and is tier 1 since 2026-09-04: `svtav1_cref::mode_decision::
//! set_nic_controls` runs the real `svt_aom_set_nic_controls` on a zeroed
//! `ModeDecisionContext` and `nic_ctrls_matches_the_real_c_at_every_level`
//! pins every [`NicRow`] field at all twelve levels. It is additionally
//! pinned against `FunnelCfg::for_preset`'s independently-derived baked rows
//! by `allintra_flattening_matches_the_ladder`.

use crate::leaf_funnel::FunnelCfg;
use crate::port_enc_mode_config::leaf;
use crate::sc_detect::ScArm;

/// C `MD_STAGE_NICS_SCAL_NUM` (`definitions.h:819`), stages 1..3.
/// (Stage 0's column is 0 in every row and is not used.)
const NICS_SCAL_NUM: [(u64, u64, u64); 16] = [
    (20, 20, 20),
    (18, 18, 18),
    (16, 16, 16),
    (12, 12, 12),
    (10, 10, 10),
    (8, 8, 8),
    (6, 6, 6),
    (4, 5, 5),
    (4, 4, 4),
    (3, 4, 4),
    (3, 3, 3),
    (3, 2, 2),
    (3, 1, 1),
    (2, 1, 1),
    (2, 0, 0),
    (0, 0, 0),
];

/// One `svt_aom_set_nic_controls` row, restricted to what [`FunnelCfg`] holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NicRow {
    pub(crate) nic_num: (u64, u64, u64),
    pub(crate) mds1_cand_base_th: u64,
    pub(crate) mds1_cand_base_th_inter: u64,
    pub(crate) mds1_class_th: u64,
    pub(crate) mds1_band_cnt: u8,
    pub(crate) mds2_class_th: u64,
    pub(crate) mds2_band_cnt: u8,
    pub(crate) mds1_rank_factor: u64,
    pub(crate) mds2_cand_base_th: u64,
    pub(crate) mds2_rank_factor: u64,
    pub(crate) mds2_rel_dev_th: u64,
    pub(crate) mds3_cand_base_th: u64,
    pub(crate) mds3_class_th: u64,
    pub(crate) mds3_band_cnt: u8,
    pub(crate) i_mds3_class_th_mult: u64,
    /// `nic_pruning_ctrls->enable_skipping_mds1`: 1 at levels 8..=11
    /// (enc_mode_config.c:4774/:4805/:4836/:4867), 0 below.
    pub(crate) enable_skipping_mds1: bool,
    /// `nic_pruning_ctrls->merge_inter_cands_mult`: 4 at levels 6..=11
    /// (enc_mode_config.c:4725/:4755/:4785/:4816/:4847/:4878), `(uint8_t)~0`
    /// below (:4545/:4575/:4605/:4635/:4665/:4695).
    pub(crate) merge_inter_cands_mult: u8,
}

/// `pcs->nic_level` for this arm. `enc_mode` must already be
/// [`crate::rate_arm::eff_enc_mode`]-clamped.
#[must_use]
pub(crate) fn nic_level(
    arm: ScArm,
    enc_mode: i8,
    is_base: bool,
    reference: crate::reference::SvtReference,
) -> u8 {
    let m = enc_mode;
    match arm {
        ScArm::Allintra => leaf::get_nic_level_allintra(m),
        ScArm::Video { .. } => leaf::get_nic_level_default(m, is_base, reference),
    }
}

/// `svt_aom_set_nic_controls` (`enc_mode_config.c:4518`) as a [`NicRow`].
///
///
/// # Panics
/// On a level outside 0..=11 — C `assert(0)`s there.
#[must_use]
pub(crate) fn nic_ctrls(level: u8) -> NicRow {
    // (scaling, mds1_cand, mds1_rank, mds2_cand, mds2_rank, mds2_dev,
    //  mds3_cand, mds3_class, mds3_band, i_mds3_mult)
    #[allow(clippy::type_complexity)]
    let (sc, m1c, m1r, m2c, m2r, m2d, m3c, m3cl, m3b, imult): (
        usize,
        u64,
        u64,
        u64,
        u64,
        u64,
        u64,
        u64,
        u8,
        u64,
    ) = match level {
        0 => (0, u64::MAX, 0, u64::MAX, 0, 0, u64::MAX, u64::MAX, 0, 0),
        1 => (0, u64::MAX, 0, 50, 0, 0, 50, 25, 4, 50),
        2 => (1, 1200, 0, 30, 0, 0, 30, 25, 8, 50),
        3 => (3, 1200, 0, 30, 0, 0, 25, 25, 8, 50),
        4 => (3, 1200, 0, 20, 0, 0, 15, 20, 12, 50),
        5 => (6, 1200, 0, 20, 0, 0, 15, 15, 16, 50),
        6 => (6, 1200, 3, 15, 1, 5, 15, 5, 16, 50),
        7 => (8, 1200, 3, 15, 1, 5, 15, 5, 16, 50),
        8 => (13, 300, 3, 3, 1, 5, 3, 5, 16, 50),
        9 => (14, 100, 3, 1, 1, 5, 1, 5, 16, 50),
        10 => (15, 1, 3, 1, 1, 5, 1, 5, 16, 50),
        11 => (15, 1, 3, 1, 1, 5, 1, 0, 16, 50),
        _ => panic!("nic level {level} outside C's switch"),
    };
    // The INTER half of the post-MDS0 candidate threshold
    // (`mds1_cand_base_th_inter`) and the two CLASS thresholds, all of which
    // are dead on an I_SLICE and live on every other frame.
    //
    // `mds1_band_cnt` is UNASSIGNED by C at levels 0..3 and `mds2_band_cnt`
    // at level 0 — the struct keeps whatever the previous picture left there.
    // Both are 0 here, and reading either would be a defect on both sides:
    // C only reaches a band count inside the `if (dev)` arm of a class prune
    // whose threshold is not the disabled sentinel, and at exactly those
    // levels the paired class threshold IS the sentinel.
    let (m1ce, m1cl, m1b, m2cl, m2b): (u64, u64, u8, u64, u8) = match level {
        0 => (u64::MAX, u64::MAX, 0, u64::MAX, 0),
        1 => (u64::MAX, u64::MAX, 0, 25, 4),
        2 | 3 => (500, u64::MAX, 0, 25, 4),
        4 => (300, 500, 3, 25, 8),
        5 => (300, 300, 4, 25, 10),
        6 | 7 | 8 => (300, 200, 16, 10, 10),
        9 => (100, 200, 16, 10, 10),
        10 => (1, 150, 16, 5, 10),
        11 => (1, 75, 16, 0, 10),
        _ => unreachable!("guarded by the switch above"),
    };
    NicRow {
        nic_num: NICS_SCAL_NUM[sc],
        mds1_cand_base_th: m1c,
        mds1_cand_base_th_inter: m1ce,
        mds1_class_th: m1cl,
        mds1_band_cnt: m1b,
        mds2_class_th: m2cl,
        mds2_band_cnt: m2b,
        mds1_rank_factor: m1r,
        mds2_cand_base_th: m2c,
        mds2_rank_factor: m2r,
        mds2_rel_dev_th: m2d,
        mds3_cand_base_th: m3c,
        mds3_class_th: m3cl,
        mds3_band_cnt: m3b,
        i_mds3_class_th_mult: imult,
        // `enable_skipping_mds1 = 1` on cases 8..=11 (enc_mode_config.c:4774,
        // :4805, :4836, :4867), `0` on every case below (:4534..:4744).
        enable_skipping_mds1: level >= 8,
        // `merge_inter_cands_mult = 4` on cases 6..=11
        // (enc_mode_config.c:4725..:4878), `(uint8_t)~0` on 0..=5
        // (:4545..:4695).
        merge_inter_cands_mult: if level >= 6 { 4 } else { u8::MAX },
    }
}

/// Stamp the row onto a [`FunnelCfg`], replacing what
/// `FunnelCfg::for_preset` baked from the allintra arm.
pub(crate) fn apply(
    cfg: &mut FunnelCfg,
    arm: ScArm,
    enc_mode: i8,
    is_base: bool,
    reference: crate::reference::SvtReference,
) {
    let r = nic_ctrls(nic_level(arm, enc_mode, is_base, reference));
    cfg.nic_num = r.nic_num;
    cfg.mds1_cand_base_th = r.mds1_cand_base_th;
    cfg.mds1_cand_base_th_inter = r.mds1_cand_base_th_inter;
    cfg.mds1_class_th = r.mds1_class_th;
    cfg.mds1_band_cnt = r.mds1_band_cnt;
    cfg.mds2_class_th = r.mds2_class_th;
    cfg.mds2_band_cnt = r.mds2_band_cnt;
    cfg.mds1_rank_factor = r.mds1_rank_factor;
    cfg.mds2_cand_base_th = r.mds2_cand_base_th;
    cfg.mds2_rank_factor = r.mds2_rank_factor;
    cfg.mds2_rel_dev_th = r.mds2_rel_dev_th;
    cfg.mds3_cand_base_th = r.mds3_cand_base_th;
    cfg.mds3_class_th = r.mds3_class_th;
    cfg.mds3_band_cnt = r.mds3_band_cnt;
    cfg.i_mds3_class_th_mult = r.i_mds3_class_th_mult;
    cfg.enable_skipping_mds1 = r.enable_skipping_mds1;
    cfg.merge_inter_cands_mult = r.merge_inter_cands_mult;
}

#[cfg(test)]
mod tests;
