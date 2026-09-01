//! Hand-derived vectors traced against `Source/Lib/Codec/enc_mode_config.c`
//! for the deblocking-level ladder and its non-base modulation.
//!
//! **These were tier 4 when written and are now SUPERSEDED at tier 1** by
//! `tests/c_parity_dlf_ctrls.rs`, which drives the real
//! `svt_aom_sig_deriv_mode_decision_config_{default,allintra}` through
//! `shims/dlf_shims.c` and compares `ppcs->dlf_ctrls`. The route the old
//! header said was closed turned out to be open: the LEVEL is not observable
//! (C never stores it), but `svt_aom_set_dlf_controls` maps each of the eight
//! levels to a DISTINCT control set, so comparing the controls pins the level.
//! What was actually missing was the ported controls table, not an exported
//! symbol. (The sigderiv lane's own shim still holds `enable_dlf_flag` at 0 —
//! that is why this lane has its own shim TU.)
//!
//! They are kept because a readable ladder table is worth having next to the
//! exhaustive differential, and because the deblocking level feeds the frame
//! header's `loop_filter_level` directly.

use svtav1_encoder::port_enc_mode_config::leaf;
use svtav1_encoder::port_enc_mode_config::{InputCoeffLvl, ResolutionRange};

/// TIER 4. `get_dlf_level_default` (`enc_mode_config.c:1466`), fast-decode
/// 0/1 arm, on a BASE picture (where the modulation does not run).
#[test]
fn dlf_level_default_base_ladder_traced() {
    let probe = |m: i8, not_last: u8, coeff: InputCoeffLvl| {
        leaf::get_dlf_level_default(m, not_last, 0, ResolutionRange::R1080p, true, coeff, 0)
    };
    // <= M0: 1. <= M3: 2. <= M6: 3 / 6 by layer.
    assert_eq!(probe(-1, 1, InputCoeffLvl::Normal), 1);
    assert_eq!(probe(0, 1, InputCoeffLvl::Normal), 1);
    assert_eq!(probe(1, 1, InputCoeffLvl::Normal), 2);
    assert_eq!(probe(3, 1, InputCoeffLvl::Normal), 2);
    assert_eq!(probe(4, 1, InputCoeffLvl::Normal), 3);
    assert_eq!(probe(6, 0, InputCoeffLvl::Normal), 6);
    // <= M7: same levels, but modulation_mode becomes 3 (inert on a base pic).
    assert_eq!(probe(7, 1, InputCoeffLvl::Normal), 3);
    assert_eq!(probe(7, 0, InputCoeffLvl::Normal), 6);
    // <= M9: 6 / 0.
    assert_eq!(probe(8, 1, InputCoeffLvl::Normal), 6);
    assert_eq!(probe(9, 0, InputCoeffLvl::Normal), 0);
    // <= M11: coeff-level dependent, and `is_base` enters the expression here
    // rather than only gating the modulation.
    assert_eq!(probe(10, 1, InputCoeffLvl::High), 6);
    assert_eq!(probe(10, 1, InputCoeffLvl::Normal), 6);
    assert_eq!(probe(11, 0, InputCoeffLvl::Normal), 6);
    // Above M11: 0.
    assert_eq!(probe(12, 1, InputCoeffLvl::Normal), 0);
    assert_eq!(probe(13, 1, InputCoeffLvl::Normal), 0);
}

/// TIER 4. The M10..M11 arm on a NON-base picture, where `is_base` selects a
/// different expression BEFORE the modulation runs.
#[test]
fn dlf_level_default_non_base_m10_m11_traced() {
    // ref_skip_percentage 0 makes modulation_mode 3 inert (it only acts above
    // 75), so this isolates the ladder itself.
    let probe = |m: i8, not_last: u8, coeff: InputCoeffLvl| {
        leaf::get_dlf_level_default(m, not_last, 0, ResolutionRange::R1080p, false, coeff, 0)
    };
    assert_eq!(
        probe(10, 1, InputCoeffLvl::High),
        0,
        "HIGH coeff, non-base: 0"
    );
    assert_eq!(probe(10, 0, InputCoeffLvl::High), 0);
    assert_eq!(probe(10, 1, InputCoeffLvl::Normal), 7, "not-last-layer: 7");
    assert_eq!(probe(10, 0, InputCoeffLvl::Normal), 0, "last layer: 0");
}

/// TIER 4. The fast-decode-2 arm, which is taken ONLY above 360p — at 360p and
/// below, any fast_decode falls into the first arm.
#[test]
fn dlf_level_default_fast_decode_arm_traced() {
    let fd2 = |m: i8, not_last: u8, res: ResolutionRange| {
        leaf::get_dlf_level_default(m, not_last, 2, res, true, InputCoeffLvl::Normal, 0)
    };
    assert_eq!(fd2(0, 1, ResolutionRange::R1080p), 4, "<= M6 is a flat 4");
    assert_eq!(fd2(6, 1, ResolutionRange::R1080p), 4);
    assert_eq!(fd2(7, 1, ResolutionRange::R1080p), 6);
    assert_eq!(fd2(8, 1, ResolutionRange::R1080p), 6);
    assert_eq!(fd2(10, 0, ResolutionRange::R1080p), 0);
    assert_eq!(fd2(11, 1, ResolutionRange::R1080p), 7);
    assert_eq!(fd2(11, 0, ResolutionRange::R1080p), 0);
    // At 360p and below the first arm wins even at fast_decode 2.
    assert_eq!(
        fd2(0, 1, ResolutionRange::R360p),
        1,
        "<= M0 on the first arm"
    );
    assert_eq!(fd2(6, 1, ResolutionRange::R240p), 3);
}

/// TIER 4. `dlf_level_modulation` (`enc_mode_config.c:1442`) on its own. It
/// runs on NON-BASE pictures only, so it is inert on a key frame.
#[test]
fn dlf_level_modulation_traced() {
    // modulation_mode 0: never changes anything.
    for lvl in 0u8..=7 {
        for perc in [0u8, 20, 30, 60, 80, 100] {
            assert_eq!(leaf::dlf_level_modulation(lvl, 0, perc), lvl);
        }
    }
    // modulation_mode 1 (towards bd-rate) below 25%: 0 -> 6, > 5 drops by 2
    // but never below 5, else unchanged.
    assert_eq!(leaf::dlf_level_modulation(0, 1, 24), 6);
    assert_eq!(leaf::dlf_level_modulation(6, 1, 24), 5);
    assert_eq!(leaf::dlf_level_modulation(7, 1, 24), 5);
    assert_eq!(leaf::dlf_level_modulation(5, 1, 24), 5);
    assert_eq!(leaf::dlf_level_modulation(3, 1, 24), 3);
    // 25..49: 0 -> 7, > 5 drops by 1.
    assert_eq!(leaf::dlf_level_modulation(0, 1, 25), 7);
    assert_eq!(leaf::dlf_level_modulation(0, 1, 49), 7);
    assert_eq!(leaf::dlf_level_modulation(7, 1, 30), 6);
    assert_eq!(leaf::dlf_level_modulation(6, 1, 30), 5);
    assert_eq!(leaf::dlf_level_modulation(5, 1, 30), 5);
    // >= 50: mode 1 does nothing.
    assert_eq!(leaf::dlf_level_modulation(0, 1, 50), 0);
    assert_eq!(leaf::dlf_level_modulation(7, 1, 50), 7);

    // modulation_mode 3 (towards speed) acts only when the level is > 4.
    assert_eq!(
        leaf::dlf_level_modulation(4, 3, 96),
        4,
        "level 4 is untouched"
    );
    assert_eq!(leaf::dlf_level_modulation(5, 3, 96), 7, "> 95: +2 below 6");
    assert_eq!(leaf::dlf_level_modulation(6, 3, 96), 0, ">= 6 goes to 0");
    assert_eq!(leaf::dlf_level_modulation(7, 3, 96), 0);
    assert_eq!(leaf::dlf_level_modulation(5, 3, 76), 6, "> 75: +1");
    assert_eq!(leaf::dlf_level_modulation(6, 3, 76), 7);
    assert_eq!(leaf::dlf_level_modulation(7, 3, 76), 0, "7 goes to 0");
    assert_eq!(leaf::dlf_level_modulation(7, 3, 75), 7, "75 is not > 75");

    // modulation_mode 2 runs BOTH halves, in order: the bd-rate half first,
    // then the speed half on its RESULT. At 24% the first half takes 0 -> 6,
    // and the second half then sees 6 with 24 <= 75, so nothing more happens.
    assert_eq!(leaf::dlf_level_modulation(0, 2, 24), 6);
    // At 96% the first half does nothing (96 >= 50) and the second takes over.
    assert_eq!(leaf::dlf_level_modulation(5, 2, 96), 7);
    assert_eq!(leaf::dlf_level_modulation(6, 2, 96), 0);
}

/// TIER 4. `get_dlf_level_allintra` (`enc_mode_config.c:1535`), for the
/// side-by-side comparison the lane's premise rests on: the two arms differ at
/// nearly every preset.
#[test]
fn dlf_level_allintra_differs_from_default_at_most_presets() {
    let mut differing = 0;
    for m in -1i8..=13 {
        let ai = leaf::get_dlf_level_allintra(m, 0, ResolutionRange::R1080p);
        let df = leaf::get_dlf_level_default(
            m,
            1,
            0,
            ResolutionRange::R1080p,
            true,
            InputCoeffLvl::Normal,
            0,
        );
        if ai != df {
            differing += 1;
        }
    }
    // Pin the allintra ladder itself: <= M3 -> 1, <= M5 -> 2, else 5.
    assert_eq!(
        leaf::get_dlf_level_allintra(3, 0, ResolutionRange::R1080p),
        1
    );
    assert_eq!(
        leaf::get_dlf_level_allintra(5, 0, ResolutionRange::R1080p),
        2
    );
    assert_eq!(
        leaf::get_dlf_level_allintra(6, 0, ResolutionRange::R1080p),
        5
    );
    // ...and its fast-decode-2 arm.
    assert_eq!(
        leaf::get_dlf_level_allintra(7, 2, ResolutionRange::R1080p),
        0
    );
    assert_eq!(
        leaf::get_dlf_level_allintra(8, 2, ResolutionRange::R1080p),
        5
    );
    // The premise this lane was given: the arms differ at most presets. This
    // is a MEASUREMENT of that claim, not a restatement of it.
    assert!(
        differing >= 10,
        "the video and allintra deblocking ladders agreed at all but \
         {} of 15 presets, which contradicts the lane premise",
        15 - differing
    );
}

/// TIER 4. `get_dlf_level_rtc` (`enc_mode_config.c:1512`).
#[test]
fn dlf_level_rtc_traced() {
    // Base: the modulation does not run.
    assert_eq!(leaf::get_dlf_level_rtc(7, true, 0), 3);
    assert_eq!(leaf::get_dlf_level_rtc(9, true, 0), 6);
    assert_eq!(leaf::get_dlf_level_rtc(10, true, 0), 7);
    assert_eq!(leaf::get_dlf_level_rtc(11, true, 0), 0);
    // Non-base at <= M7 uses modulation_mode 1: below 25% level 3 is
    // unchanged (it is not > 5).
    assert_eq!(leaf::get_dlf_level_rtc(7, false, 10), 3);
    // Non-base at M8..M9 uses mode 3: level 6 above 95% goes to 0.
    assert_eq!(leaf::get_dlf_level_rtc(9, false, 96), 0);
    assert_eq!(leaf::get_dlf_level_rtc(9, false, 76), 7);
    assert_eq!(leaf::get_dlf_level_rtc(9, false, 10), 6);
}
