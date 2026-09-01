//! Can `pic_pd0_lvl`'s subresolution level reach PD1's `md_stage_1`?
//!
//! **Measured answer: no, on every regular-PD1 arm.** This file exists because
//! the opposite reading is one grep away and was written into
//! `docs/INTER-ENCODE-PLAN.md` as the next chunk of the inter campaign.
//!
//! The reading that looks right: `md_stage_1` sets
//! `ctx->mds_subres_step = ctx->subres_ctrls.step` with NO `PD_PASS_1` guard
//! (`product_coding_loop.c:7027`), while `md_stage_2` (`:7052`) and
//! `md_stage_3` (`:7156`) both guard it to 0 for PD1. `set_subres_controls` is
//! called from `svt_aom_sig_deriv_enc_dec_pd0` (`enc_mode_config.c:7357`),
//! where the VIDEO arm's `pic_pd0_lvl = 3` yields `subres_level = 1` on an
//! I-slice. So PD1's MDS1 — the stage that picks the MDS3 survivors — would
//! run at half vertical resolution on a video-mode key frame and at full
//! resolution on a still one.
//!
//! Why it is wrong: `set_subres_controls` has **four** call sites, not one.
//! The three REGULAR-PD1 derivations each call `set_subres_controls(ctx, 0)`
//! unconditionally — `_default` `:7919`, `_rtc` `:8035`, `_allintra` `:8151`,
//! none of them behind a branch or after a `return` — and `enc_dec_process.c`
//! runs one of them on the SAME `ModeDecisionContext` (`:3046-3050`) between
//! PD0 and PD1's md loop. By the time `:7027` runs, the step is 0.
//!
//! The two LIGHT-PD1 derivations do NOT reset it (they set only
//! `subres_ctrls.odd_to_even_deviation_th = 0`, `:7574` / `:7811`), so the
//! carry-over is real there — and inert, because light PD1's loop is
//! `md_stage_0_light_pd1` + `md_stage_3_light_pd1` and the latter forces
//! `mds_subres_step = 0` (`:7133`); `md_stage_1` is never called on that path.
//! That asymmetry is asserted below too: it is the positive control that this
//! probe can observe a surviving step at all, so the zero on the regular arms
//! is a real reset and not a silent harness (`WORKING-ON-THIS.md` §5).
//!
//! **Evidence tier 1** (`docs/WORKING-ON-THIS.md` §4): every entry point here
//! is an exported C symbol and the shim drives the real ones in the real
//! order. Nothing is transcribed.

use svtav1_cref::sig_deriv as cref;
use svtav1_cref::sig_deriv::{Pd1Arm, pd0_in};

/// The inter campaign's reference cell as PD0 sees it: `gradient 64x64 q40
/// p6`, video arm, I-slice, base picture, 8-bit, 4x4 disallowed, complete
/// 64x64 SB. `pd0_level` 3 is what `sig_deriv_mode_decision_config_default`
/// assigns at M6 where the allintra arm assigns 1
/// (`docs/INTER-ENCODE-PLAN.md` §1c).
fn video_key_reference_cell() -> [i32; pd0_in::COUNT] {
    let mut i = [0i32; pd0_in::COUNT];
    i[pd0_in::LEVEL] = 3;
    i[pd0_in::IS_ISLICE] = 1;
    i[pd0_in::ALLINTRA] = 0;
    i[pd0_in::RTC] = 0;
    i[pd0_in::UPDATE_TYPE] = 0; // KF_UPDATE, not a leaf
    i[pd0_in::ENC_MODE] = 6;
    i[pd0_in::LAMBDA8] = 1000;
    i[pd0_in::LAMBDA10] = 4000;
    i[pd0_in::ME64_DIST] = 50_000;
    i[pd0_in::ME8_VAR] = 1500;
    i[pd0_in::ME8_DIST] = 3000;
    i[pd0_in::BASE_Q] = 128;
    i[pd0_in::DISALLOW_4X4] = 1;
    i[pd0_in::B64_COMPLETE] = 1;
    i[pd0_in::SB_SIZE] = 64;
    i
}

/// ANTI-VACUITY. Everything else here is a claim about a value being ZERO, and
/// a zero from a probe that never fired is indistinguishable from a zero the
/// code produced (`WORKING-ON-THIS.md` §5). So first prove PD0 sets it to ONE
/// on the video arm — and to zero on the allintra arm, which is the whole
/// reason the field looked like a live divergence.
#[test]
fn pd0_really_does_set_subres_on_the_video_arm_and_not_the_still_one() {
    let video = cref::subres_pd0_then_pd1(&video_key_reference_cell(), Pd1Arm::Default);
    assert_eq!(
        video.step_after_pd0, 1,
        "video arm, pd0_level 3, I-slice: C's set_subres_controls should give step 1"
    );
    assert_eq!(
        video.dev_th_after_pd0, 5,
        "step != 0 sets odd_to_even_deviation_th = 5 (enc_mode_config.c:3409)"
    );

    // The allintra arm at M6 assigns pd0_level 1, and `pd0_level <= PD0_LVL_2`
    // short-circuits subres_level to 0 (:7345) before any detector runs.
    let mut still = video_key_reference_cell();
    still[pd0_in::LEVEL] = 1;
    still[pd0_in::ALLINTRA] = 1;
    let still = cref::subres_pd0_then_pd1(&still, Pd1Arm::Allintra);
    assert_eq!(still.step_after_pd0, 0, "allintra arm, pd0_level 1");
    assert_eq!(still.dev_th_after_pd0, 0);
}

/// The result this file is for: each REGULAR-PD1 derivation zeroes the step,
/// so `md_stage_1`'s unguarded read at `product_coding_loop.c:7027` sees 0 on
/// PD1 regardless of the PD0 level.
#[test]
fn every_regular_pd1_arm_resets_the_subres_step_pd0_set() {
    for arm in [Pd1Arm::Default, Pd1Arm::Rtc, Pd1Arm::Allintra] {
        let c = cref::subres_pd0_then_pd1(&video_key_reference_cell(), arm);
        assert_eq!(
            c.step_after_pd0, 1,
            "{arm:?}: PD0 must have set a nonzero step for this to mean anything"
        );
        assert_eq!(
            c.step_after_pd1, 0,
            "{arm:?}: set_subres_controls(ctx, 0) should have run before PD1's md loop"
        );
        assert_eq!(
            c.dev_th_after_pd1, 0,
            "{arm:?}: step 0 sets odd_to_even_deviation_th = 0"
        );
    }
}

/// POSITIVE CONTROL for the test above, and a real asymmetry in C: the two
/// light-PD1 derivations do not call `set_subres_controls` at all, so PD0's
/// step survives into PD1 there. It is unread — `md_stage_3_light_pd1` forces
/// `mds_subres_step = 0` (`:7133`) and light PD1 never calls `md_stage_1` —
/// but observing it here proves the probe can see a surviving step, so the
/// zeroes above are resets rather than a probe that did nothing.
#[test]
fn light_pd1_arms_do_not_reset_the_step_only_the_deviation_threshold() {
    for arm in [Pd1Arm::LightDefault, Pd1Arm::LightRtc] {
        let c = cref::subres_pd0_then_pd1(&video_key_reference_cell(), arm);
        assert_eq!(c.step_after_pd0, 1, "{arm:?}");
        assert_eq!(
            c.step_after_pd1, 1,
            "{arm:?}: light PD1 leaves subres_ctrls.step alone (enc_mode_config.c:7574/:7811)"
        );
        assert_eq!(
            c.dev_th_after_pd1, 0,
            "{arm:?}: it does zero odd_to_even_deviation_th"
        );
    }
}

/// The step PD0 sets is a function of `pd0_level`, and every level a video-mode
/// key frame can take is reset the same way. Sweeping the level here is what
/// makes the conclusion about `pic_pd0_lvl` rather than about the one value 3.
#[test]
fn the_reset_holds_at_every_pd0_level() {
    // PD0_LVL_0..=PD0_LVL_6 (definitions.h); 6 returns early before subres.
    for level in 0..=6i32 {
        let mut input = video_key_reference_cell();
        input[pd0_in::LEVEL] = level;
        let c = cref::subres_pd0_then_pd1(&input, Pd1Arm::Default);
        assert_eq!(
            c.step_after_pd1, 0,
            "pd0_level {level}: PD1's derivation must zero the step \
             (step after PD0 was {})",
            c.step_after_pd0
        );
    }
}
