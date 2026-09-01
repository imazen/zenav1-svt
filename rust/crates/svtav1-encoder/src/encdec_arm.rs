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

/// Stamp this arm's `sig_deriv_enc_dec_*` signals onto a [`FunnelCfg`].
pub(crate) fn apply(cfg: &mut FunnelCfg, arm: ScArm) {
    cfg.mds0_use_hadamard_sb = mds0_use_hadamard_sb(arm);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::port_enc_mode_config::encdec;

    /// The still path is byte-neutral by construction: whatever
    /// `FunnelCfg::for_preset` baked stays.
    #[test]
    fn the_allintra_arm_leaves_every_preset_untouched() {
        for preset in 0u8..=13 {
            let baked = FunnelCfg::for_preset(preset);
            let mut walked = baked;
            apply(&mut walked, ScArm::Allintra);
            assert_eq!(
                baked.mds0_use_hadamard_sb, walked.mds0_use_hadamard_sb,
                "allintra mds0_use_hadamard_sb at M{preset}"
            );
            assert!(
                baked.mds0_use_hadamard_sb,
                "the baked still value IS C's allintra literal (enc_mode_config.c:8148)"
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
    }
}
