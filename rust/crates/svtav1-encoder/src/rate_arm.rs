//! The `scs->allintra` fork for the three frame-level RATE ladders:
//! `pcs->rdoq_level`, `pcs->rate_est_level` and `update_cdf_level`.
//!
//! All three are assigned by the same pair of C functions —
//! `svt_aom_sig_deriv_mode_decision_config_allintra` (`enc_mode_config.c:9895`)
//! and `..._default` (`:8900`) — dispatched on `scs->allintra`
//! (`enc_handle.c:4406`). They are kept together here because they are
//! COUPLED: `set_cdf_controls` (`:8468`) derives `update_coef` from
//! `rate_est_level || rdoq_level`, and the port's per-SB CDF chain is only
//! C-faithful when the rate configuration it chains under is the arm's own.
//!
//! What the two arms actually disagree about, for the only video picture this
//! port encodes today (a KEY frame: `is_islice`, `is_base`):
//!
//! | preset | rdoq allintra | rdoq video | rate_est allintra | rate_est video | update_cdf allintra | update_cdf video |
//! |---|---|---|---|---|---|---|
//! | 0..=5 | 1 | 1 | 1 | 1 | 1 (0..=3) / 2 (4,5) | 1 |
//! | 6 | f(coeff_lvl) | 1 | 1 | 1 | 2 | 1 |
//! | 7..=8 | f(coeff_lvl) | 1 | 4 | 1 | **0** | **1** |
//! | 9 | f(coeff_lvl) | 1 | 0 | 1 | 0 | 0 |
//! | 10 | f(coeff_lvl) (clamped M9) | 1 | 0 | 1 | 0 | 0 |
//! | 11..=13 | f(coeff_lvl) (clamped M9) | **2** (clamped M11) | 0 | 1 | 0 | 0 |
//!
//! `f(coeff_lvl)` is `HIGH -> 0, NORMAL -> 3, else 2` — the allintra ladder
//! never returns 1 above M5, and the video ladder never returns anything but
//! 1 below M11.
//!
//! Note the update_cdf column is byte-inert at 4..=6 for an I-slice even
//! though the LEVELS differ: `set_cdf_controls` forces `update_mv = 0` on an
//! I_SLICE (`:8495`) and levels 1 and 2 differ ONLY in `update_mv`. The rows
//! that bite are 7 and 8, where the allintra arm switches CDF adaptation OFF
//! entirely and the video arm keeps it on.
//!
//! # Evidence
//!
//! Tier 1 on both arms, and neither ladder is re-transcribed here:
//!
//! - **video** — `sig_deriv_mode_decision_config_default` is EXPORTED and
//!   `tests/c_parity_sig_deriv_md_config.rs` already drives the real symbol,
//!   reading back `pcs->rdoq_level`, `pcs->rate_est_level` and all four
//!   `cdf_ctrl` fields through `sigderiv_shims.c`. This module calls the same
//!   [`md_config::rdoq_level_default`] / [`md_config::RATE_EST_LEVEL_DEFAULT`]
//!   / [`leaf::get_update_cdf_level_default`] that function's body calls.
//! - **allintra** — `sig_deriv_mode_decision_config_allintra` is likewise
//!   EXPORTED; `tests/c_parity_sig_deriv_md_config.rs`'s
//!   `allintra_rdoq_ladder_matches_c`, `allintra_rate_est_ladder_matches_c`
//!   and `cdf_ctrl_arms_diverge_at_m7_m8_and_coincide_below` drive it through
//!   the new `ref_sig_deriv_md_config_allintra` shim and read the same six
//!   fields back, which upgrades `quant::rdoq_level_allintra` (previously a
//!   hand-transcription with unit tests only) to tier 1.

use crate::port_enc_mode_config::enc_mode::{M9, M10, M11};
use crate::port_enc_mode_config::{leaf, md_config};
use crate::quant::CoeffLvl;
use crate::sc_detect::ScArm;

/// C's per-arm preset clamp (`enc_handle.c:4415-4436`), applied once at
/// `svt_av1_enc_set_parameter` so EVERY downstream ladder sees the clamped
/// `enc_mode`.
///
/// - **allintra**: `> ENC_M9 -> ENC_M9` (`:4416`). The port already applied
///   this one inline as `preset.min(9)`.
/// - **video, non-RTC**: `> ENC_M11 -> ENC_M11` (`:4433`). `static_config.rtc`
///   is 0 for every configuration this port and the inter harness produce
///   (the C driver never sets it), so the RTC arm's `> ENC_M13 -> ENC_M13` is
///   not modelled; when RTC is wired this is the first thing to extend.
///
/// The RANDOM_ACCESS 4K clamp at `:4440` is not modelled either: it needs
/// `pred_structure == RANDOM_ACCESS`, and the port's video envelope is
/// LOW_DELAY.
#[must_use]
pub(crate) fn eff_enc_mode(arm: ScArm, preset: u8) -> u8 {
    match arm {
        ScArm::Allintra => preset.min(M9 as u8),
        ScArm::Video { .. } => preset.min(M11 as u8),
    }
}

/// `pcs->rdoq_level` for this arm. `enc_mode` must already be
/// [`eff_enc_mode`]-clamped.
///
/// `coeff_lvl` is consulted by the allintra arm only. C leaves
/// `pcs->coeff_lvl` at `INVALID_LVL` on a video I-slice
/// (`md_config_process.c:898-902` runs `derive_intra_coeff_level` only when
/// `scs->allintra`), which is sound precisely because this arm's ladder never
/// reads it.
#[must_use]
pub(crate) fn rdoq_level(arm: ScArm, enc_mode: u8, coeff_lvl: CoeffLvl) -> u8 {
    match arm {
        ScArm::Allintra => crate::quant::rdoq_level_allintra(enc_mode, coeff_lvl),
        ScArm::Video { .. } => md_config::rdoq_level_default(i8::try_from(enc_mode).unwrap_or(M10)),
    }
}

/// `pcs->rate_est_level` for this arm. `enc_mode` must already be
/// [`eff_enc_mode`]-clamped.
///
/// allintra (`enc_mode_config.c:9917`): 1 through M6, 4 at M7/M8, 0 above.
/// video (`:8942`): 1, unconditionally.
#[must_use]
pub(crate) fn rate_est_level(arm: ScArm, enc_mode: u8) -> u8 {
    match arm {
        ScArm::Allintra => {
            if enc_mode <= 6 {
                1
            } else if enc_mode <= 8 {
                4
            } else {
                0
            }
        }
        ScArm::Video { .. } => md_config::RATE_EST_LEVEL_DEFAULT,
    }
}

/// `update_cdf_level` for this arm. `enc_mode` must already be
/// [`eff_enc_mode`]-clamped.
///
/// `is_base` is C `pcs->temporal_layer_index == 0`. It is only read on the
/// video arm's M1..M3 band, where it picks 1 vs 2 — and on an I-slice those
/// two levels are behaviourally identical (`set_cdf_controls` forces
/// `update_mv = 0`), so no I-slice outcome depends on it. Threaded rather
/// than assumed so the derivation stays right when non-key video frames land.
#[must_use]
pub(crate) fn update_cdf_level(arm: ScArm, enc_mode: u8, is_base: bool) -> u8 {
    let m = i8::try_from(enc_mode).unwrap_or(M11);
    match arm {
        ScArm::Allintra => leaf::get_update_cdf_level_allintra(m),
        ScArm::Video { is_islice } => leaf::get_update_cdf_level_default(m, is_islice, is_base),
    }
}

/// `(coeff_rate_est_lvl, real_coeff_ctx)` from `set_rate_est_ctrls`
/// (`enc_mode_config.c:6428`) — the two members of `MdRateEstCtrls` the leaf
/// funnel consumes.
///
/// `real_coeff_ctx` stands for the PAIR `update_skip_ctx_dc_sign_ctx` /
/// `update_skip_coeff_ctx`, which agree at every level this port reaches
/// (0 -> 0/0, 1 -> 1/1, 4 -> 0/0). They disagree at levels 2 and 3 (1/0),
/// which no arm of `sig_deriv_mode_decision_config_*` assigns at the picture
/// level; if a level-2/3 path is ever wired, split the field first.
///
/// `lpd0_qp_offset` and `pd0_fast_coeff_est_level` are the other two members;
/// they belong to the LIGHT PD0/PD1 path, which the still and video-key
/// envelopes do not take.
#[must_use]
pub(crate) fn rate_est_ctrls(rate_est_level: u8) -> (u8, bool) {
    match rate_est_level {
        0 => (0, false),
        1 => (1, true),
        2 => (1, false),
        3 => (2, false),
        _ => (2, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The predicate `pipeline.rs` carried inline before this module existed,
    /// kept VERBATIM as the regression oracle for the still path.
    fn old_flattened_eff_mode(preset: u8) -> u8 {
        preset.min(9)
    }

    /// Ditto for the per-SB CDF-chain gate.
    fn old_flattened_chain_gate(preset: u8) -> bool {
        matches!(preset, 0..=6)
    }

    /// Ditto for `FunnelCfg::for_preset`'s baked rate-estimation pair.
    fn old_flattened_rate_est_pair(preset: u8) -> (u8, bool) {
        match preset {
            0..=6 => (1, true),
            7 | 8 => (2, false),
            _ => (0, false),
        }
    }

    /// Byte-neutrality of the still path, by construction AND entry-for-entry:
    /// every allintra arm must reproduce the flattened predicate it replaced,
    /// at every preset the port accepts.
    #[test]
    fn allintra_flattening_matches_the_ladder() {
        for preset in 0u8..=13 {
            let eff = eff_enc_mode(ScArm::Allintra, preset);
            assert_eq!(eff, old_flattened_eff_mode(preset), "eff mode p{preset}");
            assert_eq!(
                update_cdf_level(ScArm::Allintra, eff, true) != 0,
                old_flattened_chain_gate(preset),
                "chain gate p{preset}"
            );
            assert_eq!(
                rate_est_ctrls(rate_est_level(ScArm::Allintra, eff)),
                old_flattened_rate_est_pair(preset),
                "rate-est pair p{preset}"
            );
            for lvl in [
                CoeffLvl::VLow,
                CoeffLvl::Low,
                CoeffLvl::Normal,
                CoeffLvl::High,
            ] {
                assert_eq!(
                    rdoq_level(ScArm::Allintra, eff, lvl),
                    crate::quant::rdoq_level_allintra(old_flattened_eff_mode(preset), lvl),
                    "rdoq p{preset} {lvl:?}"
                );
            }
        }
    }

    /// The video arm, spelled out at every preset so a retune of either C
    /// ladder shows up as a diff here rather than silently in a sweep.
    #[test]
    fn video_arm_ladders() {
        let arm = ScArm::Video { is_islice: true };
        for preset in 0u8..=13 {
            let eff = eff_enc_mode(arm, preset);
            assert_eq!(eff, preset.min(11), "eff mode p{preset}");
            // rdoq is coeff_lvl-independent on this arm.
            let want_rdoq = if eff <= 10 { 1 } else { 2 };
            for lvl in [
                CoeffLvl::VLow,
                CoeffLvl::Low,
                CoeffLvl::Normal,
                CoeffLvl::High,
            ] {
                assert_eq!(rdoq_level(arm, eff, lvl), want_rdoq, "rdoq p{preset}");
            }
            assert_eq!(rate_est_level(arm, eff), 1, "rate_est p{preset}");
            assert_eq!(rate_est_ctrls(1), (1, true));
            assert_eq!(
                update_cdf_level(arm, eff, true) != 0,
                preset <= 8,
                "chain gate p{preset}"
            );
        }
    }

    /// The two arms disagree exactly where the module doc says they do — the
    /// point of the whole fork. A refactor that accidentally aliased the arms
    /// would leave every other test green.
    #[test]
    fn the_arms_actually_differ() {
        let vid = ScArm::Video { is_islice: true };
        // update_cdf: the still arm is OFF at 7/8, the video arm is ON.
        for preset in [7u8, 8] {
            let ea = eff_enc_mode(ScArm::Allintra, preset);
            let ev = eff_enc_mode(vid, preset);
            assert_eq!(update_cdf_level(ScArm::Allintra, ea, true), 0);
            assert_eq!(update_cdf_level(vid, ev, true), 1);
            assert_eq!(
                rate_est_ctrls(rate_est_level(ScArm::Allintra, ea)),
                (2, false)
            );
            assert_eq!(rate_est_ctrls(rate_est_level(vid, ev)), (1, true));
        }
        // rdoq: the still arm's M6+ band is coeff-driven and never 1.
        for preset in 6u8..=13 {
            let ea = eff_enc_mode(ScArm::Allintra, preset);
            assert_eq!(rdoq_level(ScArm::Allintra, ea, CoeffLvl::Normal), 3);
            assert_eq!(rdoq_level(ScArm::Allintra, ea, CoeffLvl::High), 0);
        }
        // ... and the video arm is 1 up to the M11 clamp, 2 at 11..=13.
        assert_eq!(rdoq_level(vid, eff_enc_mode(vid, 10), CoeffLvl::High), 1);
        assert_eq!(rdoq_level(vid, eff_enc_mode(vid, 13), CoeffLvl::High), 2);
    }
}
