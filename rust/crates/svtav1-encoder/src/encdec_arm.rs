//! The `scs->allintra` fork inside `svt_aom_sig_deriv_enc_dec_*` — the per-SB
//! PD1 signal derivation, as opposed to the per-PICTURE
//! `sig_deriv_mode_decision_config_*` that every other `*_arm` module wires.
//!
//! That distinction is the reason this module exists rather than a row being
//! added to [`crate::funnel_arm`]. `docs/INTER-ENCODE-PLAN.md` §1c enumerates
//! the video-vs-allintra divergence field for field — but it reads back the
//! 52-slot `MD_O_*` layout of `sig_deriv_mode_decision_config`, and **nothing
//! `sig_deriv_enc_dec_*` decides appears in it**. §1c's own closing paragraph
//! says so ("the divergence table is necessary but not sufficient"); this is
//! the second bug found outside it.
//!
//! # `ctx->mds0_use_hadamard_sb`
//!
//! | arm | value | C |
//! |---|---|---|
//! | allintra | `true` | `enc_mode_config.c:8148` |
//! | video (`_default`) | `false` | `:7916` |
//! | rtc | `false` | `:8032` |
//!
//! There is no ladder and no level: all three are literal assignments, at
//! every preset, on every frame type.
//!
//! It selects MDS0's luma distortion in `fast_loop_core`
//! (`product_coding_loop.c:1259`): `mds0_use_hadamard_blk` (`:9473`, which is
//! `mds0_use_hadamard_sb && fast_candidate_total_count > 1`) picks
//! `hadamard_path` (`:1283`, a SATD) over the two-buffer VARIANCE
//! `fn_ptr->vf` = `svt_aom_variance{W}x{H}` (`:1296-1306`). Both then
//! `<< 4` into the same `RDCOST`, so the ONLY difference is the metric.
//!
//! **Variance is DC-invariant and SATD is not**, so this is not a small
//! reordering. Every candidate whose prediction is FLAT — DC, and every
//! directional mode whose reference edge happens to be constant — gets the
//! SAME variance and DIFFERENT SATDs. Measured on the campaign's reference
//! cell (`gradient 64x64 q40 p6` video, frame 0, block (0,0) 32x32, C's
//! `SVT_FASTCOST_OUT` interposer against the port's `SVTAV1_CANDDBG`):
//!
//! * C (`hadblk=0`, so variance) scores DC, V/0, V/-3, H/0, H/+3, D45/*,
//!   D203/* and D67/* at an IDENTICAL `1392540`, and separates the
//!   left-column family — D135/0 at `1356698`, D135/+3 at `1356851`,
//!   D157/-3 at `1359225` — by ~2.6 %.
//! * the port (SATD) spreads that flat group (DC `53600`, V/0 `53472`,
//!   D45/0 `53472`, D67/0 `53472`) and pulls the left-column family IN
//!   (D135/0 `53504`, only 0.2 % under DC).
//!
//! So C's MDS1 survivor set was `{SMOOTH_V, D135/0, D135/+3, D157/-3,
//! D157/0}` and the port's `{SMOOTH_V, V/0, D67/0, D45/0, D67/-3}` — ONE
//! candidate in common, and its MDS1 full cost agreed to the byte
//! (`48577658` on both sides), which is what says the divergence is the
//! MDS0 METRIC and not the machinery around it.
//!
//! # `ctx->skip_sub_depth_ctrls`
//!
//! | arm | `skip_sub_depth_lvl` ladder | C |
//! |---|---|---|
//! | allintra | `enc_mode <= ENC_M7 -> 1 else 2` | `enc_mode_config.c:8156` |
//! | video (`_default`) | `enc_mode <= ENC_M1 -> 1 else 2` | `:7923` |
//! | rtc | `2` | `:8039` |
//!
//! Levels 1 and 2 differ ONLY in `coeff_perc` — 15 vs 25
//! (`set_skip_sub_depth_ctrls`, :6787); `enabled`, `max_size` 16 and
//! `quad_deviation_th` 250 are identical. The gate is
//! `eval_sub_depth_skip_cond1` (product_coding_loop.c:10871), run from
//! `svt_aom_pick_partition` after the current-depth test: on a <=16x16
//! block whose winner has flat quadrant recon SSEs (< 250 std-dev) and a
//! nonzero-coefficient share below `coeff_perc`, C clears
//! `mds->split_flag` and never tests the split.
//!
//! That is a real partition fork, not a tie: measured on
//! `johnny_256x256_8f` q40 p6 video frame 0, the 16x16 node at (16,32)
//! has 39/256 = 15 % nonzero coefficients and quadrant SSEs
//! {157,610,63,83} (std ~223). C's video-arm level-2 gate fires
//! (15 < 25), the split is never evaluated and the 16x16 stands at
//! rd 3441974; under the still path's level-1 ctrls `15 < 15` fails, the
//! split test runs and wins at 3040770 — four 8x8s where C codes one
//! 16x16.
//!
//! # Evidence
//!
//! Tier 1 on the value: `svt_aom_sig_deriv_enc_dec_{default,allintra}` are
//! both exported and already driven by
//! [`crate::port_enc_mode_config::encdec`], whose `EncDecDefaultSignals`
//! carries `mds0_use_hadamard_sb` and is gated by
//! `tests/c_parity_sig_deriv_enc_dec.rs`. This module only chooses between
//! the two arms; `mds0_use_hadamard_sb_forks_on_the_arm` below pins the pair
//! against that ported model rather than re-transcribing it.
//!
//! The still path is byte-neutral BY CONSTRUCTION: `ScArm::Allintra` returns
//! the `true` that `FunnelCfg::for_preset` already defaults to.

use crate::leaf_funnel::FunnelCfg;
use crate::port_enc_mode_config::enc_mode::{M1, M2, M7, M10};
use crate::port_enc_mode_config::encdec::{self, SkipSubDepthCtrls, TxShortcutCtrls};
use crate::sc_detect::ScArm;

/// C `ctx->mds0_use_hadamard_sb` for this arm.
///
/// Not a function of the preset, the slice type or the layer — C assigns a
/// literal in each of the three `svt_aom_sig_deriv_enc_dec_*` bodies.
#[must_use]
pub(crate) fn mds0_use_hadamard_sb(arm: ScArm) -> bool {
    match arm {
        ScArm::Allintra => true,
        ScArm::Video { .. } => false,
    }
}

/// C `ctx->skip_sub_depth_ctrls` for this arm + `enc_mode` (already
/// [`crate::rate_arm::eff_enc_mode`]-clamped), via
/// `set_skip_sub_depth_ctrls`.
///
/// The arms disagree at M2..M7: a video key frame at M6 derives level 2
/// (`coeff_perc` 25) where the same preset on a still derives level 1
/// (`coeff_perc` 15) — and allintra at M8/M9 derives level 2 as well, so
/// the still bake in `FunnelCfg::for_preset` must not be hardcoded
/// level 1 either.
#[must_use]
pub(crate) fn skip_sub_depth(arm: ScArm, enc_mode: i8) -> SkipSubDepthCtrls {
    let lvl = match arm {
        ScArm::Allintra => u8::from(enc_mode > M7) + 1,
        ScArm::Video { .. } => u8::from(enc_mode > M1) + 1,
    };
    encdec::set_skip_sub_depth_ctrls(lvl).expect("levels 1/2 are in-domain")
}

/// C `ctx->tx_shortcut_ctrls` for this arm + `enc_mode` + `is_base`, via
/// `set_tx_shortcut_ctrls`.
///
/// The video ladder (`sig_deriv_mode_decision_config_default`,
/// `md_config.rs:594-602`) is:
///
/// ```text
///   m <= M2  -> 0
///   m <= M10 -> !is_base           (0 on base, 1 on non-base)
///   is_islice -> 1
///   else     -> 3                  (use_mds3_shortcuts_th = 10)
/// ```
///
/// Allintra pins level 0 unconditionally (:9982) — the zeroed default
/// `FunnelCfg::for_preset` already carries.
#[must_use]
pub(crate) fn tx_shortcut(
    arm: ScArm,
    enc_mode: i8,
    is_base: bool,
    is_not_leaf: bool,
) -> TxShortcutCtrls {
    let level = match arm {
        ScArm::Allintra => 0,
        ScArm::Video { is_islice } => {
            if enc_mode <= M2 {
                0
            } else if enc_mode <= M10 {
                u8::from(!is_base)
            } else if is_islice {
                1
            } else {
                3
            }
        }
    };
    encdec::set_tx_shortcut_ctrls(level, is_not_leaf, enc_mode).expect("levels 0..=3 are in-domain")
}

/// Stamp this arm's `sig_deriv_enc_dec_*` signals onto a [`FunnelCfg`].
/// `enc_mode` must already be [`crate::rate_arm::eff_enc_mode`]-clamped.
pub(crate) fn apply(
    cfg: &mut FunnelCfg,
    arm: ScArm,
    enc_mode: i8,
    is_base: bool,
    is_not_leaf: bool,
) {
    cfg.mds0_use_hadamard_sb = mds0_use_hadamard_sb(arm);
    cfg.skip_sub_depth = skip_sub_depth(arm, enc_mode);
    cfg.tx_shortcut = tx_shortcut(arm, enc_mode, is_base, is_not_leaf);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::port_enc_mode_config::encdec;

    /// The still path is byte-neutral by construction: whatever
    /// `FunnelCfg::for_preset` baked stays.
    #[test]
    fn the_allintra_arm_leaves_every_preset_untouched() {
        for preset in 0i8..=13 {
            let baked = FunnelCfg::for_preset(preset);
            let mut walked = baked;
            apply(
                &mut walked,
                ScArm::Allintra,
                crate::rate_arm::eff_enc_mode(ScArm::Allintra, preset),
                true,
                false,
            );
            assert_eq!(
                baked.mds0_use_hadamard_sb, walked.mds0_use_hadamard_sb,
                "allintra mds0_use_hadamard_sb at M{preset}"
            );
            assert!(
                baked.mds0_use_hadamard_sb,
                "the baked still value IS C's allintra literal (enc_mode_config.c:8148)"
            );
            assert_eq!(
                baked.skip_sub_depth, walked.skip_sub_depth,
                "allintra skip_sub_depth at M{preset}"
            );
        }
    }

    /// The two arms disagree, and the disagreement is what this module is
    /// for. Without this the module could return `true` on both arms and
    /// every other test here would still pass.
    #[test]
    fn mds0_use_hadamard_sb_forks_on_the_arm() {
        assert!(mds0_use_hadamard_sb(ScArm::Allintra));
        assert!(!mds0_use_hadamard_sb(ScArm::Video { is_islice: true }));
        assert!(!mds0_use_hadamard_sb(ScArm::Video { is_islice: false }));
    }

    /// Cross-check the video value against the tier-1-ported model of C's
    /// `svt_aom_sig_deriv_enc_dec_default`, so this module is not a second,
    /// independent transcription of the same literal.
    #[test]
    fn the_video_value_matches_the_ported_sig_deriv_enc_dec_default() {
        let sig = encdec::sig_deriv_enc_dec_default(encdec::EncDecDefaultInputs {
            enc_mode: 6,
            ..Default::default()
        })
        .expect("M6 is in-domain for every table sig_deriv_enc_dec_default reaches");
        assert_eq!(
            sig.mds0_use_hadamard_sb,
            mds0_use_hadamard_sb(ScArm::Video { is_islice: true })
        );
        assert_eq!(
            sig.skip_sub_depth,
            skip_sub_depth(ScArm::Video { is_islice: true }, 6)
        );
    }

    /// The level fork IS the divergence this module fixes: levels 1 and 2
    /// differ only in `coeff_perc` (15 vs 25), so the arms must disagree
    /// exactly on the M2..M7 video band (and allintra must climb to
    /// level 2 at M8/M9 rather than staying at the old baked level 1).
    #[test]
    fn skip_sub_depth_forks_on_the_arm() {
        let video = |is_islice: bool| ScArm::Video { is_islice };
        // coeff_perc is the only level-1-vs-2 delta (enc_mode_config.c:6787).
        assert_eq!(skip_sub_depth(ScArm::Allintra, 6).coeff_perc, 15);
        assert_eq!(skip_sub_depth(video(true), 6).coeff_perc, 25);
        assert_eq!(skip_sub_depth(video(false), 6).coeff_perc, 25);
        // Video's M0/M1 stay at level 1; allintra's M8/M9 climb to 2.
        assert_eq!(skip_sub_depth(video(true), 1).coeff_perc, 15);
        assert_eq!(skip_sub_depth(ScArm::Allintra, 8).coeff_perc, 25);
        // Every reachable level keeps the gate armed on <=16x16 blocks.
        for arm in [ScArm::Allintra, video(true), video(false)] {
            for enc_mode in 0..=13 {
                let ss = skip_sub_depth(arm, enc_mode);
                assert_eq!(ss.enabled, 1);
                assert_eq!(ss.max_size, 16);
                assert_eq!(ss.quad_deviation_th, 250);
            }
        }
    }
}
