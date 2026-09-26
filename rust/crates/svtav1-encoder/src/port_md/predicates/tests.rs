use super::*;

fn mv(x: i16, y: i16) -> Mv {
    Mv { x, y }
}

/// TIER 4 — `check_mv_validity` (mode_decision.c:80-94) is `static`.
/// Traced against `is_mv_valid` (inter_prediction.h:275): the bound is
/// STRICT on both sides, so exactly +-16384 is INVALID.
#[test]
fn tier4_check_mv_validity_bounds_are_strict() {
    assert!(check_mv_validity(0, 0, 0));
    assert!(check_mv_validity(16383, 16383, 0));
    assert!(check_mv_validity(-16383, -16383, 0));
    // 16384 == MV_UPP: the comparison is `< MV_UPP`, so this fails.
    assert!(!check_mv_validity(16384, 0, 0));
    assert!(!check_mv_validity(0, 16384, 0));
    assert!(!check_mv_validity(-16384, 0, 0));
    assert!(!check_mv_validity(0, -16384, 0));
}

/// TIER 4 — the `need_shift` promotion. C computes `y_mv << shift` in
/// `int` (integer promotion) and then ASSIGNS it into `Mv`'s
/// `int16_t` field, so the high bits are discarded before the range
/// test runs. `32767 << 1` is `0xFFFE` = `-2`, which PASSES; a port
/// that widened to `i32` would see `65534` and reject it.
#[test]
fn tier4_check_mv_validity_shift_truncates_to_int16() {
    assert_eq!(32767i16.wrapping_shl(1), -2);
    assert!(check_mv_validity(4096, 4096, 1));
    assert!(check_mv_validity(32767, 0, 1));
    assert!(check_mv_validity(0, 32767, 1));
    // 16384 << 1 = 0x8000 = -32768, which is <= MV_LOW -> rejected.
    assert_eq!(16384i16.wrapping_shl(1), -32768);
    assert!(!check_mv_validity(16384, 0, 1));
}

/// TIER 4 — `is_valid_mv_diff` (mode_decision.c:776-791). The bound is
/// `> (1 << 14)`, i.e. a difference of exactly 16384 is ACCEPTED.
#[test]
fn tier4_is_valid_mv_diff_bound_is_inclusive() {
    let pred = [mv(0, 0), mv(0, 0)];
    assert!(is_valid_mv_diff(pred, mv(16384, 0), mv(0, 0), false));
    assert!(!is_valid_mv_diff(pred, mv(16385, 0), mv(0, 0), false));
    assert!(!is_valid_mv_diff(pred, mv(0, -16385), mv(0, 0), false));
    // The second MV is only checked for compound candidates.
    assert!(is_valid_mv_diff(pred, mv(0, 0), mv(32000, 0), false));
    assert!(!is_valid_mv_diff(pred, mv(0, 0), mv(32000, 0), true));
}

/// TIER 4 — `is_valid_bipred_ref` (mode_decision.c:793-813).
fn bipred_state(enabled: bool, do_ref: bool, closest: bool) -> RefPruningState {
    let mut s = RefPruningState {
        enabled,
        ..Default::default()
    };
    for g in 0..TOT_INTER_GROUP {
        s.closest_refs[g] = closest;
        for l in 0..MAX_NUM_OF_REF_PIC_LIST {
            for r in 0..REF_LIST_MAX_DEPTH {
                s.do_ref[g][l][r] = do_ref;
            }
        }
    }
    s
}

#[test]
fn tier4_is_valid_bipred_ref_disabled_accepts_everything() {
    let s = bipred_state(false, false, false);
    assert!(is_valid_bipred_ref(&s, InterCandGroup::PaMe, 0, 3, 1, 3));
}

#[test]
fn tier4_is_valid_bipred_ref_needs_both_refs() {
    let mut s = bipred_state(true, true, false);
    assert!(is_valid_bipred_ref(&s, InterCandGroup::PaMe, 0, 1, 1, 1));
    s.do_ref[InterCandGroup::PaMe as usize][1][1] = false;
    assert!(!is_valid_bipred_ref(&s, InterCandGroup::PaMe, 0, 1, 1, 1));
}

#[test]
fn tier4_is_valid_bipred_ref_closest_relaxation_needs_both_idx_zero() {
    let s = bipred_state(true, false, true);
    // LAST + BWD (both ref_idx 0) survive the relaxation.
    assert!(is_valid_bipred_ref(&s, InterCandGroup::Bi3x3, 0, 0, 1, 0));
    // Either index non-zero and the pair is rejected.
    assert!(!is_valid_bipred_ref(&s, InterCandGroup::Bi3x3, 0, 1, 1, 0));
    assert!(!is_valid_bipred_ref(&s, InterCandGroup::Bi3x3, 0, 0, 1, 2));
}

/// TIER 4 — `mv_is_already_injected` (mode_decision.c:712-760).
#[test]
fn tier4_mv_is_already_injected_unipred_matches_on_mv_and_ref_type() {
    let mut log = InjectedMvLog::default();
    log.push([mv(4, -8), mv(0, 0)], 1);
    let ctrls = RedundantCandCtrls::default();
    let rf = [1i8, -1];
    assert!(mv_is_already_injected(
        &log,
        ctrls,
        false,
        mv(4, -8),
        mv(0, 0),
        1,
        rf
    ));
    // Same MV, different ref type -> not a duplicate.
    assert!(!mv_is_already_injected(
        &log,
        ctrls,
        false,
        mv(4, -8),
        mv(0, 0),
        2,
        rf
    ));
    // Same ref type, different MV -> not a duplicate.
    assert!(!mv_is_already_injected(
        &log,
        ctrls,
        false,
        mv(4, -7),
        mv(0, 0),
        1,
        rf
    ));
}

#[test]
fn tier4_mv_is_already_injected_corrupted_check_drops_out_of_range() {
    let log = InjectedMvLog::default();
    let ctrls = RedundantCandCtrls::default();
    let rf = [1i8, -1];
    // With the check OFF an out-of-range MV is NOT reported injected.
    assert!(!mv_is_already_injected(
        &log,
        ctrls,
        false,
        mv(16384, 0),
        mv(0, 0),
        1,
        rf
    ));
    // With it ON, C folds "invalid" into "already injected".
    assert!(mv_is_already_injected(
        &log,
        ctrls,
        true,
        mv(16384, 0),
        mv(0, 0),
        1,
        rf
    ));
}

#[test]
fn tier4_mv_is_already_injected_bipred_exact_arm() {
    let mut log = InjectedMvLog::default();
    log.push([mv(4, 4), mv(-4, -4)], 9);
    let ctrls = RedundantCandCtrls::default();
    let rf = [1i8, 5];
    assert!(mv_is_already_injected(
        &log,
        ctrls,
        false,
        mv(4, 4),
        mv(-4, -4),
        9,
        rf
    ));
    // Only the second MV differs -> the exact arm keeps it.
    assert!(!mv_is_already_injected(
        &log,
        ctrls,
        false,
        mv(4, 4),
        mv(-4, -3),
        9,
        rf
    ));
}

#[test]
fn tier4_mv_is_already_injected_bipred_score_arm_needs_all_four_high_mag() {
    let mut log = InjectedMvLog::default();
    log.push([mv(100, 100), mv(-100, -100)], 9);
    let ctrls = RedundantCandCtrls {
        score_th: 16,
        mag_th: 32,
    };
    let rf = [1i8, 5];
    // score = 4, every component magnitude > 32 -> pruned.
    assert!(mv_is_already_injected(
        &log,
        ctrls,
        false,
        mv(101, 101),
        mv(-101, -101),
        9,
        rf
    ));
    // One component below mag_th kills is_high_mag for the WHOLE
    // candidate, so the approximate prune no longer fires and only an
    // exact (score == 0) match would.
    assert!(!mv_is_already_injected(
        &log,
        ctrls,
        false,
        mv(101, 101),
        mv(-101, -1),
        9,
        rf
    ));
    // score == 0 still prunes regardless of magnitude.
    let mut small = InjectedMvLog::default();
    small.push([mv(1, 1), mv(1, 1)], 9);
    assert!(mv_is_already_injected(
        &small,
        ctrls,
        false,
        mv(1, 1),
        mv(1, 1),
        9,
        rf
    ));
}

/// TIER 4 — `warped_motion_mode_allowed` (mode_decision.c:207-212).
#[test]
fn tier4_warped_motion_mode_allowed_needs_all_five_conditions() {
    let base = MotionModeCtx {
        trans_face_off: false,
        obmc_enabled: true,
        obmc_max_blk_size: 128,
        is_motion_mode_switchable: true,
        force_integer_mv: 0,
        has_overlappable_candidates: true,
        allow_warped_motion: true,
        wm_enabled: true,
        blk_width: 16,
        blk_height: 16,
    };
    assert!(warped_motion_mode_allowed(&base));
    assert!(!warped_motion_mode_allowed(&MotionModeCtx {
        allow_warped_motion: false,
        ..base
    }));
    assert!(!warped_motion_mode_allowed(&MotionModeCtx {
        has_overlappable_candidates: false,
        ..base
    }));
    assert!(!warped_motion_mode_allowed(&MotionModeCtx {
        blk_width: 4,
        ..base
    }));
    assert!(!warped_motion_mode_allowed(&MotionModeCtx {
        blk_height: 4,
        ..base
    }));
    assert!(!warped_motion_mode_allowed(&MotionModeCtx {
        wm_enabled: false,
        ..base
    }));
}
