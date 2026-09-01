//! The `scs->allintra` fork for `pcs->mds0_level` — the MDS0 *pruning*
//! ladder, as opposed to [`crate::encdec_arm`]'s `mds0_use_hadamard_sb`,
//! which chooses MDS0's *metric*.
//!
//! Sibling of [`crate::rate_arm`], [`crate::part_arm`], [`crate::intra_arm`],
//! [`crate::funnel_arm`], [`crate::txs_arm`] and [`crate::nic_arm`]: the
//! ladder pair lives in `svt_aom_sig_deriv_mode_decision_config_{allintra,
//! default}` (`enc_mode_config.c:10042` / `:9232`), reaches MD through a
//! control table (`set_mds0_controls`, `:6764`, called by all three
//! `svt_aom_sig_deriv_enc_dec_*` arms), and the still path stays byte-neutral
//! by construction because the allintra arm is a literal 0 at every preset.
//!
//! # What the arms disagree about on a KEY frame
//!
//! `mds0_level_default` is `0` up to M2, `!is_base` for M3..M5, `is_islice ?
//! 0 : 2` for M6..M10 and **`2` unconditionally above M10**. A key frame is
//! both base and an I-slice, so on the port's video envelope the ONLY
//! divergence is at **M11..M13, where the video arm is level 2 and the
//! allintra arm is 0**. That is confirmed at tier 1 by
//! `c_parity_sig_deriv_md_config.rs`, whose per-preset arm-divergence probe
//! reports `MDS0 allintra=0 video=2` at M11 and no `MDS0` row at M4/M5/M7/M9.
//!
//! # What level 2 does
//!
//! `set_mds0_controls` case 2 sets `pruning_method_th = (uint8_t)~0` and
//! `dist_to_cost_th = 0`. In `fast_loop_core` (`product_coding_loop.c:1309-
//! 1334`) the `!= (uint8_t)~0` guard then routes to the **global** arm:
//!
//! ```text
//! distortion_cost = RDCOST(full_lambda, 0, luma_fast_dist);   // rate-free
//! if (100 * (distortion_cost - mds0_best_cost)) > (mds0_best_cost * 0)
//!     *fast_cost = MAX_MODE_COST;  return;                    // abandoned
//! ```
//!
//! With `dist_to_cost_th == 0` the right-hand side is zero, so the rule is
//! exactly *"abandon any candidate whose distortion ALONE already costs more
//! than the best complete fast cost seen so far in this block"*. `mds0_best_cost`
//! is the running minimum of `*fast_cost` across every candidate scored so
//! far (`:1717`), and an abandoned candidate can never lower it because
//! `MAX_MODE_COST` is not less than anything.
//!
//! It is a PD1-only rule (`ctx->pd_pass == PD_PASS_1`); PD0 has its own fast
//! loop (`fast_loop_core_pd0`) and never reaches this code.
//!
//! # Evidence
//!
//! Tier 1 on the LADDER: it is not transcribed here.
//! [`crate::port_enc_mode_config::md_config::mds0_level_default`] is the same
//! code `sig_deriv_mode_decision_config_default` is differentially gated on
//! (`c_parity_sig_deriv_md_config.rs`,
//! `md_config_matches_c_over_the_preset_and_layer_product`), and the allintra
//! twin is C's literal.
//!
//! The CONTROL TABLE is transcribed here and cannot reach tier 1 as it
//! stands: `set_mds0_controls` is file-`static` and returns `void` into a
//! `ModeDecisionContext`. What stands in for it is
//! [`tests::the_allintra_arm_prunes_nothing_at_every_preset`] plus the
//! reachability pin below.

use crate::leaf_funnel::FunnelCfg;
use crate::port_enc_mode_config::md_config;
use crate::sc_detect::ScArm;

/// C `Mds0Ctrls`, restricted to the three fields `set_mds0_controls` writes
/// (`enc_mode_config.c:6764-6785`).
///
/// `per_class_dist_to_cost_th` is indexed by `CandClass` (`CAND_CLASS_0..3`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Mds0Ctrls {
    /// `ctrls->pruning_method_th`. 0 disables MDS0 pruning outright;
    /// `u8::MAX` selects the GLOBAL arm; anything else selects the per-class
    /// arm, which is gated on `MIN(md_me_dist, md_pme_dist)`.
    pub pruning_method_th: u8,
    /// `ctrls->dist_to_cost_th`, read only by the global arm.
    pub dist_to_cost_th: u16,
    /// `ctrls->per_class_dist_to_cost_th`, read only by the per-class arm.
    pub per_class_dist_to_cost_th: [u16; 4],
}

/// C `set_mds0_controls` (`enc_mode_config.c:6764`).
///
/// C leaves every field it does not assign at whatever the context carried;
/// the context is zero-initialised per block, so the unassigned fields are 0
/// and this returns 0 for them.
///
/// # Panics
/// On a level outside 0..=2 — C `assert(0)`s there.
#[must_use]
pub(crate) fn set_mds0_controls(mds0_level: u8) -> Mds0Ctrls {
    match mds0_level {
        0 => Mds0Ctrls {
            pruning_method_th: 0,
            dist_to_cost_th: 0,
            per_class_dist_to_cost_th: [0; 4],
        },
        1 => Mds0Ctrls {
            pruning_method_th: 100,
            dist_to_cost_th: 0,
            per_class_dist_to_cost_th: [50, 10, 10, 50],
        },
        2 => Mds0Ctrls {
            pruning_method_th: u8::MAX,
            dist_to_cost_th: 0,
            per_class_dist_to_cost_th: [0; 4],
        },
        _ => panic!("mds0 level {mds0_level} outside C's switch"),
    }
}

/// `pcs->mds0_level` for this arm. `enc_mode` must already be
/// [`crate::rate_arm::eff_enc_mode`]-clamped.
#[must_use]
pub(crate) fn mds0_level(arm: ScArm, enc_mode: u8, is_base: bool, is_islice: bool) -> u8 {
    let m = i8::try_from(enc_mode).unwrap_or(i8::MAX);
    match arm {
        ScArm::Allintra => md_config::mds0_level_allintra(m),
        ScArm::Video { .. } => md_config::mds0_level_default(m, is_base, is_islice),
    }
}

/// Stamp this arm's MDS0 pruning threshold onto a [`FunnelCfg`].
///
/// `None` means "no MDS0 prune runs", which is level 0 — and, per the module
/// header, the only level the ALLINTRA arm ever assigns.
///
/// # Panics
/// On level 1, which the port cannot execute: its per-class arm is gated on
/// `MIN(ctx->md_me_dist, ctx->md_pme_dist)`, motion-estimation distortions
/// that only exist on an inter picture. Level 1 is assigned exclusively at
/// M3..M5 for a NON-base video picture, and the port's public entry point
/// refuses inter frames, so every picture it encodes has `is_base == true`.
/// Panicking is deliberate: silently skipping the prune would emit a stream
/// that is neither C's nor a refusal, which
/// `docs/WORKING-ON-THIS.md` §6 forbids. `level_1_is_unreachable_on_every_
/// key_frame_preset` pins the reachability claim.
pub(crate) fn apply(cfg: &mut FunnelCfg, arm: ScArm, enc_mode: u8, is_base: bool, is_islice: bool) {
    let ctrls = set_mds0_controls(mds0_level(arm, enc_mode, is_base, is_islice));
    cfg.mds0_dist_to_cost_th = match ctrls.pruning_method_th {
        0 => None,
        u8::MAX => Some(ctrls.dist_to_cost_th),
        th => panic!(
            "mds0 pruning_method_th {th} selects C's per-class arm, which reads \
             md_me_dist / md_pme_dist — inter-only state this port has none of"
        ),
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The still path is byte-neutral by construction: the allintra arm's
    /// level is a literal 0 at every preset, which `set_mds0_controls` maps to
    /// `pruning_method_th = 0` — no prune, exactly what shipped.
    #[test]
    fn the_allintra_arm_prunes_nothing_at_every_preset() {
        for preset in 0u8..=13 {
            let mut cfg = FunnelCfg::for_preset(preset);
            apply(&mut cfg, ScArm::Allintra, preset, true, true);
            assert_eq!(
                cfg.mds0_dist_to_cost_th, None,
                "allintra MDS0 prune at M{preset}"
            );
        }
        assert_eq!(FunnelCfg::for_preset(6).mds0_dist_to_cost_th, None);
    }

    /// The arms diverge, and only above M10 on a key frame. Without this the
    /// module could return `None` on both arms and every other test here
    /// would still pass.
    #[test]
    fn the_video_arm_prunes_only_above_m10_on_a_key_frame() {
        for preset in 0u8..=13 {
            let mut cfg = FunnelCfg::for_preset(preset);
            apply(
                &mut cfg,
                ScArm::Video { is_islice: true },
                preset,
                true,
                true,
            );
            let want = if preset > 10 { Some(0) } else { None };
            assert_eq!(
                cfg.mds0_dist_to_cost_th, want,
                "video MDS0 prune at M{preset}"
            );
        }
    }

    /// The reachability claim [`apply`]'s panic rests on: no key frame, on
    /// either arm, at any preset, reaches level 1.
    #[test]
    fn level_1_is_unreachable_on_every_key_frame_preset() {
        for preset in 0u8..=13 {
            for arm in [ScArm::Allintra, ScArm::Video { is_islice: true }] {
                assert_ne!(
                    mds0_level(arm, preset, true, true),
                    1,
                    "{arm:?} at M{preset} on a key frame"
                );
            }
        }
        // ... and the positive control: level 1 IS what C assigns for the
        // non-base video pictures the port cannot yet encode, so the panic
        // guards a real case rather than a dead one.
        assert_eq!(
            mds0_level(ScArm::Video { is_islice: false }, 4, false, false),
            1
        );
    }
}
