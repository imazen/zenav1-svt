//! The `scs->allintra` fork for three more leaf-funnel ladders:
//! `pcs->txt_level` (the transform-type search), `pcs->cfl_level`
//! (chroma-from-luma) and `pcs->chroma_level` (the independent-chroma
//! search).
//!
//! Sibling of [`crate::rate_arm`] (the three RATE ladders),
//! [`crate::part_arm`] (the three PARTITION ladders) and
//! [`crate::intra_arm`] (`pic_filter_intra_level` + `intra_level`). Same
//! shape: the ladder pair lives in
//! `svt_aom_sig_deriv_mode_decision_config_{allintra,default}`
//! (`enc_mode_config.c:9961`/`:9983`/`:9986` and `:9057`/`:9100`/`:9103`),
//! each reaches the funnel through a control table
//! (`svt_aom_set_txt_controls` `:3894`, `set_cfl_ctrls` `:6390`,
//! `svt_aom_set_chroma_controls` `:4326` — applied to the mode-decision
//! context inside `svt_aom_sig_deriv_enc_dec_{default,allintra}` `:7842`/
//! `:8071`), and the still path stays byte-neutral by construction because
//! the allintra arm reproduces what `FunnelCfg::for_preset` baked.
//!
//! What the arms disagree about on a KEY frame (`is_islice`, `is_base`):
//!
//! | preset | txt allintra | txt video | cfl allintra | cfl video | chroma allintra | chroma video |
//! |---|---|---|---|---|---|---|
//! | 0 | 2 | 2 | 1 | 1 | 1 | 1 |
//! | 1 | 2 | 2 | 4 | 1 | 2 | **4** |
//! | 2 | 2 | 2 | 4 | **2** | 4 | 4 |
//! | 3 | 2 | **7** | 4 | 2 | 4 | 4 |
//! | 4..=5 | 3 | 7 | 4 | 2 | 4 | 4 |
//! | 6 | 8 | **7** | 4 | **2** | 5 | 5 |
//! | 7..=8 | 10 | 7 | 0 | 2 | 5 | 5 |
//! | 9 | 0 | 7 | 0 | 2 | 5 | 5 |
//! | 10..=11 | 0 (clamped M9) | 7 / 10 | 0 | 2 / 0 | 5 (clamped M9) | 5 / 5 |
//!
//! `chroma_level` is the one the video-key census caught
//! (`benchmarks/video_parity_census_2026-09-26.meta`): the arms differ on a
//! key frame ONLY at M1, where the video arm keeps `chroma_level` 4 —
//! `search_best_mds3_uv_mode` over the MDS3 survivors' uv-follows-luma
//! modes — against the still arm's 2 (`search_best_independent_uv_mode`,
//! `ind_uv_last_mds == 1`). On every other key frame the ladders agree
//! (1 / 1 / 4 / 4 / 5 over MR / M0 / M2..M5 / M6+); on an INTER frame they
//! also differ at M0 (`is_islice ? 1 : 4` at `enc_mode <= ENC_M0`).
//!
//! At the inter campaign's reference preset (M6) the video arm searches a
//! WIDER tx-type set — `txt_level` 7 restores both intra groups to
//! `MAX_TX_TYPE_GROUP` where level 8 had cut them to 5 / 4 — and turns the
//! CfL complexity detector OFF (`cfl_level` 2 has `cplx_th = 0`, which C
//! reads as "evaluate CfL unconditionally", `product_coding_loop.c:7183`)
//! where level 4 gated it on the detector firing at threshold 10.
//!
//! # Evidence
//!
//! Tier 1 on both LADDERS: neither is transcribed here. They live in
//! [`crate::port_enc_mode_config::md_config`] beside the rest of the
//! `sig_deriv_mode_decision_config` ladders, and
//! `tests/c_parity_sig_deriv_md_config.rs` drives BOTH exported C entry points
//! from one input population and compares `pcs->txt_level` / `pcs->cfl_level`
//! slot-for-slot over the whole (preset x is_base x is_islice) grid
//! (`txt_and_cfl_ladders_match_c_on_both_arms`).
//!
//! The two CONTROL TABLES are transcribed here, and cannot reach tier 1 as
//! they stand: `svt_aom_set_txt_controls` and `set_cfl_ctrls` return `void`
//! into a `ModeDecisionContext`, so a shim would have to synthesise one. What
//! stands in for it is `allintra_flattening_matches_the_ladder`, which pins
//! every transcribed row against `FunnelCfg::for_preset`'s baked values —
//! derived independently, from an instrumented C config dump — at every
//! preset.

use crate::leaf_funnel::FunnelCfg;
use crate::port_enc_mode_config::{leaf, md_config};
use crate::sc_detect::ScArm;

/// `pcs->txt_level` for this arm. `enc_mode` must already be
/// [`crate::rate_arm::eff_enc_mode`]-clamped.
///
/// `reference` selects the `85842c43c` research-preset arms on the video
/// ladder under `GhostRobot`.
#[must_use]
pub(crate) fn txt_level(
    arm: ScArm,
    enc_mode: i8,
    is_base: bool,
    reference: crate::reference::SvtReference,
) -> u8 {
    let m = enc_mode;
    match arm {
        ScArm::Allintra => md_config::txt_level_allintra(m),
        ScArm::Video { .. } => md_config::txt_level_default(m, is_base, reference),
    }
}

/// `svt_aom_set_txt_controls` (`enc_mode_config.c:3894`) as
/// `(enabled, txt_group_intra_lt, txt_group_intra_ge, txt_group_inter_lt,
/// txt_group_inter_ge, satd_early_exit_th_intra, satd_early_exit_th_inter,
/// txt_rate_cost_th)`.
///
/// The two group pairs split at level 6+ and the SATD thresholds at level 5+;
/// which one a candidate reads is C's `is_intra_mode(mode)` (group counts) and
/// `is_inter_mode || use_intrabc` (SATD th) — see `leaf_funnel::txt`.
/// `early_exit_dist_th` / `early_exit_coeff_th` are 0 in every case and
/// `satd_th_q_weight` is 1 in every case that enables the search, so neither
/// varies with the level.
///
/// # Panics
/// On a level outside 0..=10 — C `assert(0)`s there.
#[must_use]
#[allow(clippy::type_complexity)]
pub(crate) fn txt_ctrls(level: u8) -> (bool, i32, i32, i32, i32, u64, u64, u64) {
    // MAX_TX_TYPE_GROUP (definitions.h:1076).
    const MAX_GROUP: i32 = 6;
    match level {
        0 => (false, 1, 1, 1, 1, 0, 0, 0),
        1 => (true, MAX_GROUP, MAX_GROUP, MAX_GROUP, MAX_GROUP, 0, 0, 0),
        2 => (
            true, MAX_GROUP, MAX_GROUP, MAX_GROUP, MAX_GROUP, 20, 20, 250,
        ),
        3 => (
            true, MAX_GROUP, MAX_GROUP, MAX_GROUP, MAX_GROUP, 15, 15, 250,
        ),
        4 => (
            true, MAX_GROUP, MAX_GROUP, MAX_GROUP, MAX_GROUP, 10, 10, 250,
        ),
        5 => (true, MAX_GROUP, MAX_GROUP, MAX_GROUP, MAX_GROUP, 10, 5, 100),
        6 => (true, MAX_GROUP, MAX_GROUP, MAX_GROUP, 5, 10, 5, 100),
        7 => (true, MAX_GROUP, MAX_GROUP, 5, 5, 10, 5, 100),
        8 => (true, 5, 4, 4, 3, 10, 5, 100),
        9 => (true, 4, 3, 3, 2, 10, 5, 65),
        10 => (true, 3, 2, 2, 1, 10, 5, 50),
        _ => panic!("txt level {level} outside C's switch"),
    }
}

/// `pcs->cfl_level` for this arm. `enc_mode` must already be
/// [`crate::rate_arm::eff_enc_mode`]-clamped.
#[must_use]
pub(crate) fn cfl_level(arm: ScArm, enc_mode: i8, is_base: bool, is_islice: bool) -> u8 {
    let m = enc_mode;
    match arm {
        ScArm::Allintra => md_config::cfl_level_allintra(m),
        ScArm::Video { .. } => md_config::cfl_level_default(m, is_base, is_islice),
    }
}

/// `pcs->chroma_level` for this arm. `enc_mode` must already be
/// [`crate::rate_arm::eff_enc_mode`]-clamped.
#[must_use]
pub(crate) fn chroma_level(arm: ScArm, enc_mode: i8, is_islice: bool) -> u8 {
    let m = enc_mode;
    match arm {
        ScArm::Allintra => leaf::get_chroma_level_allintra(m),
        ScArm::Video { .. } => leaf::get_chroma_level_default(m, is_islice),
    }
}

/// `svt_aom_set_chroma_controls` (`enc_mode_config.c:4326`) as the three
/// [`FunnelCfg`] fields the port models the search split with:
/// `(ind_uv_independent, ind_uv_last_mds1, ind_uv_mds3)`.
///
/// The `svt_aom_get_chroma_level_*` ladders only produce 1, 2, 4 and 5:
/// CHROMA_MODE_0 with `ind_uv_last_mds` 0 / 1 / 2, then CHROMA_MODE_1 (uv
/// follows luma). The two thresholds the table also sets —
/// `inter_vs_intra_cost_th` and `skip_ind_uv_if_only_dc` — are baked into
/// the `ind_uv_mds3` path at level 4's values (100 / 1,
/// `leaf_funnel::mds3`); `uv_nic_scaling_num` (16 / 8 at levels 1 / 2)
/// rides on `ind_uv_independent`. `uv_mode <= CHROMA_MODE_1` makes
/// `blk_skip_decision` true at every one of these levels, so there is no
/// fourth field.
///
/// # Panics
/// On a level the ladders cannot assign (0 — CHROMA_MODE_2, chroma off —
/// or 3), like the sibling tables.
#[must_use]
pub(crate) fn chroma_ctrls(level: u8) -> (Option<u16>, bool, bool) {
    match level {
        1 => (Some(16), false, false),
        2 => (Some(8), true, false),
        4 => (None, false, true),
        5 => (None, false, false),
        _ => panic!("chroma level {level} outside C's ladders"),
    }
}

/// `set_cfl_ctrls` (`enc_mode_config.c:6390`) as
/// `(enabled, Some((itr_th, cplx_th)))`.
///
/// `None` for the thresholds is not "zero": C's `case 0` writes ONLY
/// `ctrls->enabled = 0` and leaves `itr_th` / `cplx_th` holding whatever the
/// context carried, so there is no level-0 value to report. Returning a
/// fabricated `(0, 0)` there would make this function disagree with
/// `FunnelCfg::for_preset` on a pair nothing reads — which is exactly how a
/// by-construction pin turns into a false alarm, or worse, gets weakened to
/// make it pass.
///
/// `cplx_th == 0` on an ENABLED level is meaningful and is not the same thing:
/// C reads it as a BYPASS of the chroma-complexity detector, so CfL is
/// evaluated for every MDS3 intra candidate (`product_coding_loop.c:7183`,
/// `!cplx_th ||`).
///
/// # Panics
/// On a level outside 0..=5 — C `assert(0)`s there.
#[must_use]
pub(crate) fn cfl_ctrls(level: u8) -> (bool, Option<(u8, u32)>) {
    match level {
        0 => (false, None),
        1 => (true, Some((2, 0))),
        2 => (true, Some((1, 0))),
        3 => (true, Some((1, 5))),
        4 => (true, Some((1, 10))),
        5 => (true, Some((1, 55))),
        _ => panic!("cfl level {level} outside C's switch"),
    }
}

/// Stamp both ladders' results onto a [`FunnelCfg`], replacing the values
/// `FunnelCfg::for_preset` baked from the allintra arm.
pub(crate) fn apply(
    cfg: &mut FunnelCfg,
    arm: ScArm,
    enc_mode: i8,
    is_islice: bool,
    is_base: bool,
    reference: crate::reference::SvtReference,
) {
    let (txt_on, lt16, ge16, ilt16, ige16, satd_th, satd_th_inter, rate_th) =
        txt_ctrls(txt_level(arm, enc_mode, is_base, reference));
    cfg.txt_on = txt_on;
    cfg.txt_group_lt16 = lt16;
    cfg.txt_group_ge16 = ge16;
    cfg.txt_group_inter_lt16 = ilt16;
    cfg.txt_group_inter_ge16 = ige16;
    cfg.txt_satd_th = satd_th;
    cfg.txt_satd_th_inter = satd_th_inter;
    cfg.txt_rate_th = rate_th;

    let (cfl_on, ths) = cfl_ctrls(cfl_level(arm, enc_mode, is_base, is_islice));
    cfg.cfl_enabled = cfl_on;
    if let Some((itr_th, cplx_th)) = ths {
        cfg.cfl_itr_th = itr_th;
        cfg.cfl_cplx_th = cplx_th;
    }

    let (ind_indep, last_mds1, mds3) = chroma_ctrls(chroma_level(arm, enc_mode, is_islice));
    cfg.ind_uv_independent = ind_indep;
    cfg.ind_uv_last_mds1 = last_mds1;
    cfg.ind_uv_mds3 = mds3;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The still path is byte-neutral BY CONSTRUCTION: at every preset the
    /// allintra arm walked through the two ladders and their control tables
    /// reproduces exactly what `FunnelCfg::for_preset` baked, on every field
    /// the funnel READS.
    ///
    /// The two exclusions are not a weakening, they are the two places where
    /// the baked table and C's own tables disagree on a value nothing looks
    /// at, and each is proven dead at its use site:
    ///
    /// - `svt_aom_set_txt_controls` case 0 resets the group counts to 1 and
    ///   both thresholds to 0; `for_preset`'s `_ =>` arm left them at the M6
    ///   row's 5 / 4 / 10 / 100. Dead because `leaf_funnel::txt`'s `only_dct`
    ///   is `!cfg.txt_on || ...`, and on that branch the group count is pinned
    ///   to 1 and the SATD threshold to 0 without reading either field.
    /// - `set_cfl_ctrls` case 0 writes ONLY `enabled = 0`, leaving `itr_th` /
    ///   `cplx_th` at whatever the context held — so there is no level-0 value
    ///   to compare against (which is why [`cfl_ctrls`] returns `None`). Dead
    ///   because `mds3`'s `cfl_gate` is `cfg.cfl_enabled && ...`.
    ///
    /// The baked table is the regression oracle for everything else — it was
    /// derived from an instrumented C config dump, independently of this
    /// transcription.
    #[test]
    fn allintra_flattening_matches_the_ladder() {
        for preset in 0i8..=13 {
            let baked = FunnelCfg::for_preset(preset);
            let mut walked = baked;
            let eff = crate::rate_arm::eff_enc_mode(ScArm::Allintra, preset);
            apply(
                &mut walked,
                ScArm::Allintra,
                eff,
                true,
                true,
                crate::reference::SvtReference::Hybrid3115,
            );
            assert_eq!(baked.txt_on, walked.txt_on, "txt enabled at M{preset}");
            if baked.txt_on {
                assert_eq!(
                    (
                        baked.txt_group_lt16,
                        baked.txt_group_ge16,
                        baked.txt_group_inter_lt16,
                        baked.txt_group_inter_ge16,
                        baked.txt_satd_th,
                        baked.txt_satd_th_inter,
                        baked.txt_rate_th
                    ),
                    (
                        walked.txt_group_lt16,
                        walked.txt_group_ge16,
                        walked.txt_group_inter_lt16,
                        walked.txt_group_inter_ge16,
                        walked.txt_satd_th,
                        walked.txt_satd_th_inter,
                        walked.txt_rate_th
                    ),
                    "allintra txt ladder vs FunnelCfg::for_preset at M{preset}"
                );
            }
            assert_eq!(
                baked.cfl_enabled, walked.cfl_enabled,
                "cfl enabled at M{preset}"
            );
            if baked.cfl_enabled {
                assert_eq!(
                    (baked.cfl_itr_th, baked.cfl_cplx_th),
                    (walked.cfl_itr_th, walked.cfl_cplx_th),
                    "allintra cfl ladder vs FunnelCfg::for_preset at M{preset}"
                );
            }
            assert_eq!(
                (
                    baked.ind_uv_independent,
                    baked.ind_uv_last_mds1,
                    baked.ind_uv_mds3
                ),
                (
                    walked.ind_uv_independent,
                    walked.ind_uv_last_mds1,
                    walked.ind_uv_mds3
                ),
                "allintra chroma ladder vs FunnelCfg::for_preset at M{preset}"
            );
        }
    }

    /// The M6 key-frame row the inter campaign stands on.
    #[test]
    fn video_m6_key_frame_widens_txt_and_unconditionalizes_cfl() {
        let arm = ScArm::Video { is_islice: true };
        assert_eq!(
            txt_level(arm, 6, true, crate::reference::SvtReference::Hybrid3115),
            7
        );
        assert_eq!(txt_ctrls(7), (true, 6, 6, 5, 5, 10, 5, 100));
        assert_eq!(cfl_level(arm, 6, true, true), 2);
        assert_eq!(cfl_ctrls(2), (true, Some((1, 0))));
        // The still path at M6 cuts both tx-type groups and gates CfL on the
        // complexity detector.
        let m6 = FunnelCfg::for_preset(6);
        assert_eq!(
            (m6.txt_group_lt16, m6.txt_group_ge16, m6.cfl_cplx_th),
            (5, 4, 10)
        );
    }
}
