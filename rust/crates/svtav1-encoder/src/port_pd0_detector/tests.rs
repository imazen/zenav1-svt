use super::*;

/// **EVIDENCE TIER 4** for every test here: both C functions are `static`
/// and were inlined away, and their only caller is the encode-decode
/// thread body. Vectors are hand-derived from the C source at the cited
/// lines.
const _: () = ();

/// `set_pd0_ctrls` (enc_mode_config.c:5415), row for row.
///
/// The two aliasing pairs are the point of this test: 5 and 6 both give
/// `Lvl5`, 7 and 8 both give `Lvl6`, and they are told apart only by the
/// detector rows below them.
#[test]
fn ctrls_for_level_matches_c() {
    for (lvl, want) in [
        (0u8, Pd0Level::Lvl0),
        (1, Pd0Level::Lvl1),
        (2, Pd0Level::Lvl2),
        (3, Pd0Level::Lvl3),
        (4, Pd0Level::Lvl4),
        (5, Pd0Level::Lvl5),
        (6, Pd0Level::Lvl5),
        (7, Pd0Level::Lvl6),
        (8, Pd0Level::Lvl6),
    ] {
        assert_eq!(pd0_ctrls_for_level(lvl).pd0_level, want, "lpd0_lvl {lvl}");
    }
    // 0..=3 arm NO detector at all.
    for lvl in 0..=3u8 {
        let c = pd0_ctrls_for_level(lvl);
        assert!(
            c.use_pd0_detector.iter().all(|d| !d),
            "lpd0_lvl {lvl} must arm no detector"
        );
    }
    let l4 = pd0_ctrls_for_level(4);
    assert!(l4.use_pd0_detector[Pd0Level::Lvl4 as usize]);
    assert_eq!(l4.use_ref_info[Pd0Level::Lvl4 as usize], 2);
    assert_eq!(l4.me_8x8_cost_variance_th[Pd0Level::Lvl4 as usize], 250_000);
    assert_eq!(l4.neigh_me_dist_shift[Pd0Level::Lvl4 as usize], 3);
    assert!(!l4.use_pd0_detector[Pd0Level::Lvl5 as usize]);

    let l5 = pd0_ctrls_for_level(5);
    assert!(l5.use_pd0_detector[Pd0Level::Lvl4 as usize]);
    assert!(l5.use_pd0_detector[Pd0Level::Lvl5 as usize]);
    assert_eq!(l5.use_ref_info[Pd0Level::Lvl5 as usize], 1);
    assert_eq!(l5.me_8x8_cost_variance_th[Pd0Level::Lvl5 as usize], 125_000);
    assert_eq!(l5.neigh_me_dist_shift[Pd0Level::Lvl5 as usize], 2);

    // 6 DISARMS the LVL_4 row that 5 arms, and drops use_ref_info to 0.
    let l6 = pd0_ctrls_for_level(6);
    assert!(!l6.use_pd0_detector[Pd0Level::Lvl4 as usize]);
    assert_eq!(l6.use_ref_info[Pd0Level::Lvl5 as usize], 0);
    assert_eq!(l6.me_8x8_cost_variance_th[Pd0Level::Lvl5 as usize], 500_000);

    let l7 = pd0_ctrls_for_level(7);
    assert_eq!(l7.edge_dist_th[Pd0Level::Lvl5 as usize], u32::MAX);
    assert_eq!(l7.neigh_me_dist_shift[Pd0Level::Lvl5 as usize], u16::MAX);
    assert_eq!(l7.use_ref_info[Pd0Level::Lvl6 as usize], 1);
    assert_eq!(
        pd0_ctrls_for_level(8).use_ref_info[Pd0Level::Lvl6 as usize],
        2
    );
}

/// The wiring fact `pipeline.rs` depends on, as a test rather than a
/// comment: on the port's low-delay-P envelope the L0 reference is the KEY
/// frame, whose every SB is intra, and the ladder then walks
/// `Lvl5 -> Lvl4 -> Lvl3` on the REFERENCE tests alone — no ME threshold is
/// ever consulted, so the answer does not depend on per-SB ME data.
///
/// The I-slice control is the other half: on frame 0 the same picture
/// levels are a NO-OP, which is why the key frame keeps 3 / 4 / 5.
#[test]
fn an_all_intra_l0_reference_walks_every_level_down_to_lvl3() {
    let inter_sb = |was_intra: u8| Pd0SbInput {
        slice_type_is_intra: false,
        ref_l0: RefSbInfo {
            was_intra: Some(was_intra),
            ..RefSbInfo::default()
        },
        // Deliberately EXTREME, so that if the reference arm did not fire
        // the ME arm certainly would and the test could not pass by
        // accident on the same answer.
        me_8x8_cost_variance: u32::MAX / 4,
        me_64x64_distortion: u32::MAX / 4,
        picture_qp: 40,
        is_edge_sb: true,
        ..Pd0SbInput::default()
    };
    for lpd0_lvl in [3u8, 4, 5] {
        let ctrls = pd0_ctrls_for_level(lpd0_lvl);
        assert_eq!(
            pd0_detector(&ctrls, &inter_sb(1)),
            Pd0Level::Lvl3,
            "lpd0_lvl {lpd0_lvl} with an all-intra L0 reference"
        );
    }
    // Positive control: with a NON-intra reference the ladder does NOT
    // stop at Lvl3 for every input — level 5 still steps (the ME arm
    // fires on the extreme values above), which proves the assert above is
    // reading the reference arm and not a constant.
    let ctrls5 = pd0_ctrls_for_level(5);
    assert_ne!(pd0_detector(&ctrls5, &inter_sb(0)), Pd0Level::Lvl5);

    // I-slice: levels 3/4/5 are untouched, because every test in the body
    // is gated on `slice_type != I_SLICE`.
    for (lpd0_lvl, want) in [
        (3u8, Pd0Level::Lvl3),
        (4, Pd0Level::Lvl4),
        (5, Pd0Level::Lvl5),
    ] {
        let ctrls = pd0_ctrls_for_level(lpd0_lvl);
        let sb = Pd0SbInput {
            slice_type_is_intra: true,
            me_8x8_cost_variance: u32::MAX / 4,
            me_64x64_distortion: u32::MAX / 4,
            picture_qp: 40,
            is_edge_sb: true,
            ..Pd0SbInput::default()
        };
        assert_eq!(
            pd0_detector(&ctrls, &sb),
            want,
            "I-slice lpd0_lvl {lpd0_lvl}"
        );
    }
}

fn ctrls_at(level: Pd0Level) -> Pd0Ctrls {
    Pd0Ctrls {
        pd0_level: level,
        use_pd0_detector: [true; PD0_LEVELS],
        use_ref_info: [0; PD0_LEVELS],
        // Disable tests 3 and 4 unless a test turns them on.
        me_8x8_cost_variance_th: [u32::MAX; PD0_LEVELS],
        edge_dist_th: [u32::MAX; PD0_LEVELS],
        neigh_me_dist_shift: [u16::MAX; PD0_LEVELS],
    }
}

fn inter_sb() -> Pd0SbInput {
    Pd0SbInput {
        slice_type_is_intra: false,
        picture_qp: 32,
        ..Default::default()
    }
}

/// `PD0_LVL_6` supports INTER compensation only, so an I_SLICE (or a
/// transition frame) is knocked down BEFORE the detector even runs — and
/// C's closing assert says so.
#[test]
fn lvl6_requires_inter() {
    let mut c = ctrls_at(Pd0Level::Lvl6);
    c.use_pd0_detector = [false; PD0_LEVELS];
    let mut sb = inter_sb();
    assert_eq!(pd0_detector(&c, &sb), Pd0Level::Lvl6);

    sb.slice_type_is_intra = true;
    assert_eq!(pd0_detector(&c, &sb), Pd0Level::Lvl5);

    let mut sb = inter_sb();
    sb.transition_present = true;
    assert_eq!(pd0_detector(&c, &sb), Pd0Level::Lvl5);
}

/// The walk is a LADDER: one call can step down several levels, because
/// each level it lands on is re-tested with its own thresholds.
#[test]
fn the_detector_can_step_down_several_levels_in_one_call() {
    let mut c = ctrls_at(Pd0Level::Lvl6);
    // Every level's ME-variance test fails.
    c.me_8x8_cost_variance_th = [0; PD0_LEVELS];
    let mut sb = inter_sb();
    sb.me_8x8_cost_variance = 1;
    // th == 0 -> (0 >> 5) * qp == 0, and 1 > 0, so every level steps down;
    // the loop bound stops at level 0.
    assert_eq!(pd0_detector(&c, &sb), Pd0Level::Lvl0);
}

/// `use_pd0_detector == false` skips tests 2-4 but NOT the LVL_6 inter
/// requirement, which is outside the guard.
#[test]
fn detector_disabled_still_enforces_the_lvl6_inter_rule() {
    let mut c = ctrls_at(Pd0Level::Lvl6);
    c.use_pd0_detector = [false; PD0_LEVELS];
    c.me_8x8_cost_variance_th = [0; PD0_LEVELS];
    let mut sb = inter_sb();
    sb.me_8x8_cost_variance = u32::MAX;
    // Tests 2-4 are skipped, so an inter SB keeps LVL_6...
    assert_eq!(pd0_detector(&c, &sb), Pd0Level::Lvl6);
    // ...but an intra one is still knocked down once.
    sb.slice_type_is_intra = true;
    assert_eq!(pd0_detector(&c, &sb), Pd0Level::Lvl5);
}

/// The three `use_ref_info` arms differ, and level 2's `!l0_refs || ...`
/// makes an ABSENT list vacuously agree — which reads like a bug and is
/// not.
#[test]
fn use_ref_info_arms_differ() {
    // The arm is set at level 4 ONLY, so a step-down lands on level 3
    // where `use_ref_info == 0` and the ladder stops. Setting it at every
    // level instead walks all the way to 0 — which is correct C behaviour
    // and is what a first draft of this test measured by accident.
    let mk = |arm: u8, l0: Option<u8>, l1: Option<u8>, ref_intra_pct: u32| {
        let mut c = ctrls_at(Pd0Level::Lvl4);
        c.use_ref_info[4] = arm;
        let mut sb = inter_sb();
        sb.ref_l0 = RefSbInfo {
            was_intra: l0,
            ..RefSbInfo::default()
        };
        sb.ref_l1 = RefSbInfo {
            was_intra: l1,
            ..RefSbInfo::default()
        };
        sb.ref_intra_percentage = ref_intra_pct;
        pd0_detector(&c, &sb)
    };
    // Arm 1: EITHER present-and-intra reference steps down.
    assert_eq!(mk(1, Some(1), None, 0), Pd0Level::Lvl3);
    assert_eq!(mk(1, Some(0), None, 0), Pd0Level::Lvl4);
    // Arm 2: one list present and intra, the OTHER ABSENT -> still steps
    // down, because an absent list satisfies `!lN_refs`.
    assert_eq!(mk(2, Some(1), None, 0), Pd0Level::Lvl3);
    // ...but a present-and-NOT-intra list blocks it.
    assert_eq!(mk(2, Some(1), Some(0), 0), Pd0Level::Lvl4);
    // Arm 1 would have stepped down on that same input.
    assert_eq!(mk(1, Some(1), Some(0), 0), Pd0Level::Lvl3);
    // Arm 3 adds the intra-percentage floor: at qp 32 the floor is
    // max(1, 50 - 16) == 34.
    assert_eq!(mk(3, Some(1), None, 34), Pd0Level::Lvl4);
    assert_eq!(mk(3, Some(1), None, 35), Pd0Level::Lvl3);
}

/// The ME-variance threshold's sentinel: at or above `u32::MAX >> 1` the
/// test is DISABLED, not trivially passed.
#[test]
fn me_variance_threshold_sentinel_disables_the_test() {
    let mut c = ctrls_at(Pd0Level::Lvl4);
    c.me_8x8_cost_variance_th = [u32::MAX >> 1; PD0_LEVELS];
    let mut sb = inter_sb();
    sb.me_8x8_cost_variance = u32::MAX;
    assert_eq!(pd0_detector(&c, &sb), Pd0Level::Lvl4);
    // One below the sentinel the test is live, and `(th >> 5) * qp`
    // is the comparison.
    c.me_8x8_cost_variance_th = [(u32::MAX >> 1) - 1; PD0_LEVELS];
    assert_eq!(pd0_detector(&c, &sb), Pd0Level::Lvl0);
}

/// An edge SB has no neighbours, so it uses the absolute
/// `edge_dist_th` instead of the neighbour comparison.
#[test]
fn edge_sb_uses_the_absolute_distortion_threshold() {
    let mut c = ctrls_at(Pd0Level::Lvl4);
    c.edge_dist_th = [1000; PD0_LEVELS];
    c.neigh_me_dist_shift = [0; PD0_LEVELS];
    let mut sb = inter_sb();
    sb.is_edge_sb = true;
    sb.me_64x64_distortion = 1001;
    // Steps down at 4, then again at 3, 2, 1 — the same absolute test
    // fails at every level.
    assert_eq!(pd0_detector(&c, &sb), Pd0Level::Lvl0);
    sb.me_64x64_distortion = 1000;
    assert_eq!(pd0_detector(&c, &sb), Pd0Level::Lvl4);
    // A non-edge SB with the same numbers uses the neighbour sum instead,
    // which is 0 + 0 here, so it steps down for a different reason.
    sb.is_edge_sb = false;
    sb.me_64x64_distortion = 1;
    assert_eq!(pd0_detector(&c, &sb), Pd0Level::Lvl0);
}

/// `neigh_me_dist_shift == u16::MAX` disables BOTH neighbour comparisons
/// but leaves the `use_ref_info` neighbour-intra fallback live.
#[test]
fn neighbour_shift_sentinel_leaves_the_ref_info_fallback_live() {
    let mut c = ctrls_at(Pd0Level::Lvl4);
    c.neigh_me_dist_shift = [u16::MAX; PD0_LEVELS];
    c.use_ref_info = [1; PD0_LEVELS];
    let mut sb = inter_sb();
    sb.me_64x64_distortion = u32::MAX;
    // Both neighbours intra -> step down, even with the shift disabled.
    sb.left_sb_intra = true;
    sb.top_sb_intra = true;
    assert_eq!(pd0_detector(&c, &sb), Pd0Level::Lvl0);
    // One intra, neither skipped -> also steps down.
    let mut sb2 = inter_sb();
    sb2.left_sb_intra = true;
    assert_eq!(pd0_detector(&c, &sb2), Pd0Level::Lvl0);
    // One intra but a neighbour IS skipped -> stays.
    let mut sb3 = inter_sb();
    sb3.left_sb_intra = true;
    sb3.left_sb_skip = true;
    assert_eq!(pd0_detector(&c, &sb3), Pd0Level::Lvl4);
}

/// The mainline accumulation shifts; the fork divides. They agree on
/// non-negative input, which is why citing the wrong arm is easy.
#[test]
fn mainline_and_fork_accumulation_agree_on_real_variances() {
    let v = SbVariance {
        var64: 1000,
        var32: [900, 1100, 1000, 1000],
        var16: [1000; 16],
    };
    let m = accumulate(&v);
    assert_eq!(m.norm_v64, 1000);
    assert_eq!(m.norm_v32, (4000 >> 2) * 4);
    assert_eq!(m.norm_v16, (16000 >> 4) * 16);
    assert_eq!(accumulate_fork(&v), m);
    // They diverge where the shift and the divide disagree: a per-block
    // sum that is not a multiple of the block count.
    let v2 = SbVariance {
        var64: 0,
        var32: [1, 1, 1, 0],
        var16: [0; 16],
    };
    // mainline: (3 >> 2) * 4 == 0; fork: (3/4) * 4 == 3.0 -> 3.
    assert_eq!(accumulate(&v2).norm_v32, 0);
    assert_eq!(accumulate_fork(&v2).norm_v32, 3);
}

/// `pd0_detector_allintra` steps down by ONE when no depth dominates, and
/// does nothing below `PD0_LVL_6`.
#[test]
fn allintra_steps_down_when_no_depth_dominates() {
    let flat = NormalisedVariance {
        norm_v64: 1000,
        norm_v32: 1100,
        norm_v16: 1200,
    };
    let peaky = NormalisedVariance {
        norm_v64: 1000,
        norm_v32: 20_000,
        norm_v16: 1200,
    };
    let s = QpThScaling::default();
    assert_eq!(
        pd0_detector_allintra(Pd0Level::Lvl6, &flat, &s),
        Pd0Level::Lvl5
    );
    assert_eq!(
        pd0_detector_allintra(Pd0Level::Lvl6, &peaky, &s),
        Pd0Level::Lvl6
    );
    // Below LVL_6 it returns immediately.
    assert_eq!(
        pd0_detector_allintra(Pd0Level::Lvl5, &flat, &s),
        Pd0Level::Lvl5
    );
}

/// `DIVIDE_AND_ROUND` is round-half-up, and the 10000-scale weights need
/// 64-bit intermediates.
#[test]
fn qp_scaling_rounds_half_up_in_64_bits() {
    let s = QpThScaling {
        q_weight: 1,
        q_weight_denom: 2,
    };
    assert_eq!(s.scale(3), 2, "1.5 rounds up");
    assert_eq!(s.scale(5), 3, "2.5 rounds up");
    // The exponential branch's shape: q_weight ~ 7000, denom 10000.
    let s = QpThScaling {
        q_weight: 7000,
        q_weight_denom: 10000,
    };
    // 7500 * 7000 == 52_500_000, which fits i32 — but a caller with a
    // larger threshold would overflow it, so the port widens.
    assert_eq!(s.scale(DELTA_VAR_TH), 5250);
    assert_eq!(s.scale(i32::MAX / 2), 751_619_276);
}
