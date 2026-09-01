//! The `scs->allintra` fork for two more leaf-funnel ladders:
//! `pcs->txt_level` (the transform-type search) and `pcs->cfl_level`
//! (chroma-from-luma).
//!
//! Sibling of [`crate::rate_arm`] (the three RATE ladders),
//! [`crate::part_arm`] (the three PARTITION ladders) and
//! [`crate::intra_arm`] (`pic_filter_intra_level` + `intra_level`). Same
//! shape: the ladder pair lives in
//! `svt_aom_sig_deriv_mode_decision_config_{allintra,default}`
//! (`enc_mode_config.c:9961`/`:9986` and `:9057`/`:9103`), each reaches the
//! funnel through a control table (`svt_aom_set_txt_controls` `:3894`,
//! `set_cfl_ctrls` `:6390`), and the still path stays byte-neutral by
//! construction because the allintra arm reproduces what
//! `FunnelCfg::for_preset` baked.
//!
//! What the arms disagree about on a KEY frame (`is_islice`, `is_base`):
//!
//! | preset | txt allintra | txt video | cfl allintra | cfl video |
//! |---|---|---|---|---|
//! | 0 | 2 | 2 | 1 | 1 |
//! | 1 | 2 | 2 | 4 | 1 |
//! | 2 | 2 | 2 | 4 | **2** |
//! | 3 | 2 | **7** | 4 | 2 |
//! | 4..=5 | 3 | 7 | 4 | 2 |
//! | 6 | 8 | **7** | 4 | **2** |
//! | 7..=8 | 10 | 7 | 0 | 2 |
//! | 9 | 0 | 7 | 0 | 2 |
//! | 10..=11 | 0 (clamped M9) | 7 / 10 | 0 | 2 / 0 |
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
use crate::port_enc_mode_config::md_config;
use crate::sc_detect::ScArm;

/// `pcs->txt_level` for this arm. `enc_mode` must already be
/// [`crate::rate_arm::eff_enc_mode`]-clamped.
#[must_use]
pub(crate) fn txt_level(arm: ScArm, enc_mode: u8, is_base: bool) -> u8 {
    let m = i8::try_from(enc_mode).unwrap_or(i8::MAX);
    match arm {
        ScArm::Allintra => md_config::txt_level_allintra(m),
        ScArm::Video { .. } => md_config::txt_level_default(m, is_base),
    }
}

/// `svt_aom_set_txt_controls` (`enc_mode_config.c:3894`), restricted to the
/// four values `FunnelCfg` carries:
/// `(enabled, txt_group_intra_lt_16x16, txt_group_intra_gt_eq_16x16,
/// satd_early_exit_th_intra, txt_rate_cost_th)`.
///
/// The INTER twins (`txt_group_inter_*`, `satd_early_exit_th_inter`) are not
/// returned: `FunnelCfg` has no field for them, and the port's video envelope
/// is key-frames-only so no candidate is ever priced through them. When inter
/// candidates arrive they have to be added here, not re-derived elsewhere.
/// `early_exit_dist_th` / `early_exit_coeff_th` are 0 in every case and
/// `satd_th_q_weight` is 1 in every case that enables the search, so neither
/// varies with the level.
///
/// # Panics
/// On a level outside 0..=10 — C `assert(0)`s there.
#[must_use]
pub(crate) fn txt_ctrls(level: u8) -> (bool, i32, i32, u64, u64) {
    // MAX_TX_TYPE_GROUP (definitions.h:1076).
    const MAX_GROUP: i32 = 6;
    match level {
        0 => (false, 1, 1, 0, 0),
        1 => (true, MAX_GROUP, MAX_GROUP, 0, 0),
        2 => (true, MAX_GROUP, MAX_GROUP, 20, 250),
        3 => (true, MAX_GROUP, MAX_GROUP, 15, 250),
        4 => (true, MAX_GROUP, MAX_GROUP, 10, 250),
        5 | 6 | 7 => (true, MAX_GROUP, MAX_GROUP, 10, 100),
        8 => (true, 5, 4, 10, 100),
        9 => (true, 4, 3, 10, 65),
        10 => (true, 3, 2, 10, 50),
        _ => panic!("txt level {level} outside C's switch"),
    }
}

/// `pcs->cfl_level` for this arm. `enc_mode` must already be
/// [`crate::rate_arm::eff_enc_mode`]-clamped.
#[must_use]
pub(crate) fn cfl_level(arm: ScArm, enc_mode: u8, is_base: bool, is_islice: bool) -> u8 {
    let m = i8::try_from(enc_mode).unwrap_or(i8::MAX);
    match arm {
        ScArm::Allintra => md_config::cfl_level_allintra(m),
        ScArm::Video { .. } => md_config::cfl_level_default(m, is_base, is_islice),
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
pub(crate) fn apply(cfg: &mut FunnelCfg, arm: ScArm, enc_mode: u8, is_islice: bool, is_base: bool) {
    let (txt_on, lt16, ge16, satd_th, rate_th) = txt_ctrls(txt_level(arm, enc_mode, is_base));
    cfg.txt_on = txt_on;
    cfg.txt_group_lt16 = lt16;
    cfg.txt_group_ge16 = ge16;
    cfg.txt_satd_th = satd_th;
    cfg.txt_rate_th = rate_th;

    let (cfl_on, ths) = cfl_ctrls(cfl_level(arm, enc_mode, is_base, is_islice));
    cfg.cfl_enabled = cfl_on;
    if let Some((itr_th, cplx_th)) = ths {
        cfg.cfl_itr_th = itr_th;
        cfg.cfl_cplx_th = cplx_th;
    }
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
        for preset in 0u8..=13 {
            let baked = FunnelCfg::for_preset(preset);
            let mut walked = baked;
            let eff = crate::rate_arm::eff_enc_mode(ScArm::Allintra, preset);
            apply(&mut walked, ScArm::Allintra, eff, true, true);
            assert_eq!(baked.txt_on, walked.txt_on, "txt enabled at M{preset}");
            if baked.txt_on {
                assert_eq!(
                    (
                        baked.txt_group_lt16,
                        baked.txt_group_ge16,
                        baked.txt_satd_th,
                        baked.txt_rate_th
                    ),
                    (
                        walked.txt_group_lt16,
                        walked.txt_group_ge16,
                        walked.txt_satd_th,
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
        }
    }

    /// The M6 key-frame row the inter campaign stands on.
    #[test]
    fn video_m6_key_frame_widens_txt_and_unconditionalizes_cfl() {
        let arm = ScArm::Video { is_islice: true };
        assert_eq!(txt_level(arm, 6, true), 7);
        assert_eq!(txt_ctrls(7), (true, 6, 6, 10, 100));
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
