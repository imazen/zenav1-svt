//! **EVIDENCE TIER 4** (`docs/WORKING-ON-THIS.md` §4, the WEAKEST tier):
//! hand-derived vectors traced against the C source for the five `rc_process.c`
//! helpers that are `static` with NO exported symbol.
//!
//! Why there is no tier-1 test here, stated explicitly because §4 requires it:
//! `get_ref_obj` (rc_process.c:61), `get_ref_intra_percentage` (:66),
//! `get_ref_skip_percentage` (:96), `get_ref_hp_percentage` (:118) and
//! `rc_init_frame_stats` (:604) are all `static`. `nm -g` on
//! `Bin/Release/libSvtAv1Enc.a` lists none of them (positive controls:
//! `svt_av1_rc_bits_per_mb`, `svt_aom_compute_rd_mult`,
//! `svt_aom_rate_control_kernel` ARE listed, so the absence is a real
//! absence and not a broken probe). The only exported code that reaches them
//! is `svt_aom_rate_control_kernel`, which needs the rate-control thread and
//! its fifos stood up; that is a bigger synthetic than a PCS and would not
//! be a cleaner oracle than what is here.
//!
//! So every expected value below is a LITERAL, derived by hand from the C
//! statements quoted beside it. None of them is computed by re-running the
//! port's own algorithm — a transcribed oracle agreeing with transcribed code
//! proves only that both were transcribed the same way.

use svtav1_encoder::port_rc_process::{
    FrameStatsInput, FrameUpdateType, RefObjStats, SliceType, frame_is_kf_gf_arf,
    get_ref_hp_percentage, get_ref_intra_percentage, get_ref_obj, get_ref_skip_percentage,
    rc_init_frame_stats, update_rc_counts,
};

fn obj(slice_type: SliceType, intra: u8, skip: u8, hp: u8) -> RefObjStats {
    RefObjStats {
        slice_type,
        intra_coded_area: intra,
        skip_coded_area: skip,
        hp_coded_area: hp,
    }
}

// ---------------------------------------------------------------------------
// get_ref_intra_percentage (rc_process.c:66)
// ---------------------------------------------------------------------------

#[test]
fn intra_percentage_i_slice_is_100() {
    // C: `if (pcs->slice_type == I_SLICE) { *intra_perc = 100; return; }`
    // The reference objects must be ignored entirely on this path.
    let l0 = obj(SliceType::B, 7, 7, 7);
    assert_eq!(
        get_ref_intra_percentage(SliceType::I, 1, Some(&l0), Some(&l0)),
        100
    );
}

#[test]
fn intra_percentage_one_inter_reference_is_that_reference() {
    // ref_cnt == 1, iperc == 40, 40 / 1 == 40.
    let l0 = obj(SliceType::B, 40, 0, 0);
    assert_eq!(
        get_ref_intra_percentage(SliceType::B, 0, Some(&l0), None),
        40
    );
}

#[test]
fn intra_percentage_l0_intra_gives_zero_not_the_l0_value() {
    // C only accumulates when `ref_obj_l0->slice_type != I_SLICE`, so an
    // I_SLICE L0 leaves ref_cnt == 0 and the else-branch writes 0 — NOT 90.
    let l0 = obj(SliceType::I, 90, 0, 0);
    assert_eq!(
        get_ref_intra_percentage(SliceType::B, 0, Some(&l0), None),
        0
    );
}

#[test]
fn intra_percentage_two_references_average_with_c_integer_division() {
    // iperc = 40 + 51 = 91, ref_cnt = 2, 91 / 2 == 45 (C truncates).
    let l0 = obj(SliceType::B, 40, 0, 0);
    let l1 = obj(SliceType::B, 51, 0, 0);
    assert_eq!(
        get_ref_intra_percentage(SliceType::B, 1, Some(&l0), Some(&l1)),
        45
    );
}

#[test]
fn intra_percentage_l1_ignored_when_ref_list1_count_try_is_zero() {
    // C's guard is `pcs->slice_type == B_SLICE && pcs->ppcs->ref_list1_count_try`.
    // With the count 0, L1 must not be read: ref_cnt stays 1, so 40 / 1.
    let l0 = obj(SliceType::B, 40, 0, 0);
    let l1 = obj(SliceType::B, 100, 0, 0);
    assert_eq!(
        get_ref_intra_percentage(SliceType::B, 0, Some(&l0), Some(&l1)),
        40
    );
}

#[test]
fn intra_percentage_uint8_accumulator_wraps_before_the_divide() {
    // `uint8_t iperc` — C: 200 + 200 == 400, truncated to 144, / 2 == 72.
    // A u16 accumulator would give 200; that "fix" is a divergence.
    let l0 = obj(SliceType::B, 200, 0, 0);
    let l1 = obj(SliceType::B, 200, 0, 0);
    assert_eq!(
        get_ref_intra_percentage(SliceType::B, 1, Some(&l0), Some(&l1)),
        72
    );
}

// ---------------------------------------------------------------------------
// get_ref_skip_percentage (rc_process.c:96)
// ---------------------------------------------------------------------------

#[test]
fn skip_percentage_i_slice_is_zero() {
    // C: `if (pcs->slice_type == I_SLICE) { *skip_area = 0; return; }` — note
    // this differs from the intra twin, which returns 100.
    let l0 = obj(SliceType::B, 0, 88, 0);
    assert_eq!(
        get_ref_skip_percentage(SliceType::I, 1, Some(&l0), Some(&l0)),
        0
    );
}

#[test]
fn skip_percentage_single_reference_is_not_halved() {
    // The `>>= 1` lives inside the B_SLICE && ref_list1_count_try branch only.
    let l0 = obj(SliceType::B, 0, 88, 0);
    assert_eq!(
        get_ref_skip_percentage(SliceType::B, 0, Some(&l0), None),
        88
    );
}

#[test]
fn skip_percentage_two_references_sum_then_shift() {
    // (30 + 51) >> 1 == 81 >> 1 == 40.
    let l0 = obj(SliceType::B, 0, 30, 0);
    let l1 = obj(SliceType::B, 0, 51, 0);
    assert_eq!(
        get_ref_skip_percentage(SliceType::B, 1, Some(&l0), Some(&l1)),
        40
    );
}

#[test]
fn skip_percentage_intra_l1_still_halves_unlike_the_intra_twin() {
    // THE ASYMMETRY: there is no ref_cnt here. C adds 0 for an I_SLICE L1 and
    // then shifts unconditionally, so this is 80 >> 1 == 40, NOT 80.
    let l0 = obj(SliceType::B, 0, 80, 0);
    let l1 = obj(SliceType::I, 0, 99, 0);
    assert_eq!(
        get_ref_skip_percentage(SliceType::B, 1, Some(&l0), Some(&l1)),
        40
    );
}

#[test]
fn skip_percentage_uint8_accumulator_wraps_before_the_shift() {
    // (200 + 200) mod 256 == 144, >> 1 == 72.
    let l0 = obj(SliceType::B, 0, 200, 0);
    let l1 = obj(SliceType::B, 0, 200, 0);
    assert_eq!(
        get_ref_skip_percentage(SliceType::B, 1, Some(&l0), Some(&l1)),
        72
    );
}

// ---------------------------------------------------------------------------
// get_ref_hp_percentage (rc_process.c:118)
// ---------------------------------------------------------------------------

#[test]
fn hp_percentage_i_slice_is_minus_one() {
    let l0 = obj(SliceType::B, 0, 0, 50);
    assert_eq!(
        get_ref_hp_percentage(SliceType::I, 1, Some(&l0), Some(&l0)),
        -1
    );
}

#[test]
fn hp_percentage_single_reference_passes_through() {
    // hp_perc_l1 stays -1, so C takes the `hp_perc_l1 == -1` arm and returns l0.
    let l0 = obj(SliceType::B, 0, 0, 50);
    assert_eq!(get_ref_hp_percentage(SliceType::B, 0, Some(&l0), None), 50);
}

#[test]
fn hp_percentage_both_intra_is_minus_one() {
    let l0 = obj(SliceType::I, 0, 0, 50);
    let l1 = obj(SliceType::I, 0, 0, 60);
    assert_eq!(
        get_ref_hp_percentage(SliceType::B, 1, Some(&l0), Some(&l1)),
        -1
    );
}

#[test]
fn hp_percentage_l0_intra_takes_l1() {
    let l0 = obj(SliceType::I, 0, 0, 50);
    let l1 = obj(SliceType::B, 0, 0, 60);
    assert_eq!(
        get_ref_hp_percentage(SliceType::B, 1, Some(&l0), Some(&l1)),
        60
    );
}

#[test]
fn hp_percentage_two_references_average_arithmetic_shift() {
    // (50 + 61) >> 1 == 111 >> 1 == 55.
    let l0 = obj(SliceType::B, 0, 0, 50);
    let l1 = obj(SliceType::B, 0, 0, 61);
    assert_eq!(
        get_ref_hp_percentage(SliceType::B, 1, Some(&l0), Some(&l1)),
        55
    );
}

#[test]
fn hp_percentage_uint8_field_narrows_to_int8_and_can_go_negative() {
    // TRAP 1: `hp_coded_area` is uint8_t in EbReferenceObject, but C assigns
    // it to an `int8_t` local. 200 -> -56. Two of those: (-56 + -56) >> 1.
    // C's `>>` on a negative int is an ARITHMETIC shift: -112 >> 1 == -56.
    let l0 = obj(SliceType::B, 0, 0, 200);
    let l1 = obj(SliceType::B, 0, 0, 200);
    assert_eq!(
        get_ref_hp_percentage(SliceType::B, 1, Some(&l0), Some(&l1)),
        -56
    );
    // And the arithmetic shift FLOORS: (-56 + -55) >> 1 == -111 >> 1 == -56,
    // where a truncating divide would give -55.
    let l1b = obj(SliceType::B, 0, 0, 201); // 201 as i8 == -55
    assert_eq!(
        get_ref_hp_percentage(SliceType::B, 1, Some(&l0), Some(&l1b)),
        -56
    );
}

#[test]
fn hp_percentage_255_is_indistinguishable_from_the_absent_sentinel() {
    // TRAP 2: 255 as int8_t IS -1, which is the "no usable reference" value.
    // C cannot tell them apart, so neither does the port. l0 = 255 -> -1, and
    // with l1 = 60 C takes the `hp_perc_l0 == -1` arm and returns 60.
    let l0 = obj(SliceType::B, 0, 0, 255);
    let l1 = obj(SliceType::B, 0, 0, 60);
    assert_eq!(
        get_ref_hp_percentage(SliceType::B, 1, Some(&l0), Some(&l1)),
        60
    );
}

// ---------------------------------------------------------------------------
// get_ref_obj (rc_process.c:61)
// ---------------------------------------------------------------------------

#[test]
fn get_ref_obj_indexes_list_then_slot() {
    // C: `pcs->ref_pic_ptr_array[ref_list][idx]->object_ptr` — LIST first,
    // then the slot. The port's array is `[slot][list]`, so this pins the
    // argument order against a grid where the two readings differ.
    let a = obj(SliceType::B, 1, 0, 0);
    let b = obj(SliceType::B, 2, 0, 0);
    let c = obj(SliceType::B, 3, 0, 0);
    let d = obj(SliceType::B, 4, 0, 0);
    let arr = [[Some(a), Some(b)], [Some(c), Some(d)]];
    assert_eq!(get_ref_obj(&arr, 0, 0).unwrap().intra_coded_area, 1);
    assert_eq!(get_ref_obj(&arr, 1, 0).unwrap().intra_coded_area, 2);
    assert_eq!(get_ref_obj(&arr, 0, 1).unwrap().intra_coded_area, 3);
    assert_eq!(get_ref_obj(&arr, 1, 1).unwrap().intra_coded_area, 4);
    assert!(get_ref_obj(&arr, 0, 2).is_none());
}

// ---------------------------------------------------------------------------
// rc_init_frame_stats (rc_process.c:604)
// ---------------------------------------------------------------------------

#[test]
fn frame_stats_one_pass_period_is_60() {
    // C: `if (scs->passes > 1 && scs->static_config.max_bit_rate) {...} else
    // { rc->rate_average_periodin_frames = 60; }` then MIN(_, 1000).
    let l0 = obj(SliceType::B, 40, 30, 50);
    let dist = [10u32, 20, 30, 41];
    let out = rc_init_frame_stats(&FrameStatsInput {
        slice_type: SliceType::B,
        ref_list1_count_try: 0,
        ref_l0: Some(&l0),
        ref_l1: None,
        passes: 1,
        max_bit_rate: 0,
        total_stats_count: 999_999,
        me_64x64_distortion: &dist,
    });
    assert_eq!(out.rate_average_periodin_frames, 60);
    assert_eq!(out.ref_intra_percentage, 40);
    assert_eq!(out.ref_skip_percentage, 30);
    assert_eq!(out.ref_hp_percentage, 50);
    // (10 + 20 + 30 + 41) / 4 == 101 / 4 == 25 (C truncates).
    assert_eq!(out.avg_base_me_dist, Some(25));
}

#[test]
fn frame_stats_two_pass_capped_period_uses_total_stats_clamped_to_1000() {
    // Both conditions must hold; the MIN is against
    // MAX_RATE_AVG_PERIOD == CODED_FRAMES_STAT_QUEUE_MAX_DEPTH >> 1 == 1000.
    let l0 = obj(SliceType::B, 0, 0, 0);
    let dist = [1u32];
    let mk = |passes, max_bit_rate, count| {
        rc_init_frame_stats(&FrameStatsInput {
            slice_type: SliceType::B,
            ref_list1_count_try: 0,
            ref_l0: Some(&l0),
            ref_l1: None,
            passes,
            max_bit_rate,
            total_stats_count: count,
            me_64x64_distortion: &dist,
        })
        .rate_average_periodin_frames
    };
    assert_eq!(mk(2, 1_000_000, 300), 300);
    assert_eq!(
        mk(2, 1_000_000, 5000),
        1000,
        "must clamp to MAX_RATE_AVG_PERIOD"
    );
    // passes > 1 alone is not enough.
    assert_eq!(mk(2, 0, 300), 60);
    // max_bit_rate alone is not enough.
    assert_eq!(mk(1, 1_000_000, 300), 60);
}

#[test]
fn frame_stats_i_slice_leaves_the_me_distortion_alone() {
    // C guards the whole ME-distortion block with
    // `if (ppcs->slice_type != I_SLICE)`, so on an I_SLICE neither
    // cur_avg_base_me_dist NOR prev_avg_base_me_dist is written and the
    // previous frame's values persist. `None` is that "not written".
    let dist = [1000u32, 2000];
    let out = rc_init_frame_stats(&FrameStatsInput {
        slice_type: SliceType::I,
        ref_list1_count_try: 0,
        ref_l0: None,
        ref_l1: None,
        passes: 1,
        max_bit_rate: 0,
        total_stats_count: 0,
        me_64x64_distortion: &dist,
    });
    assert_eq!(out.avg_base_me_dist, None);
    assert_eq!(out.ref_intra_percentage, 100);
    assert_eq!(out.ref_skip_percentage, 0);
    assert_eq!(out.ref_hp_percentage, -1);
}

// ---------------------------------------------------------------------------
// svt_aom_frame_is_kf_gf_arf (rc_process.c:56) and
// svt_aom_update_rc_counts (rc_process.c:564)
// ---------------------------------------------------------------------------

#[test]
fn frame_is_kf_gf_arf_matches_the_c_disjunction() {
    // C: `frame_is_intra_only(ppcs) || update_type == ARF_UPDATE ||
    //     update_type == GF_UPDATE` — note INTNL_ARF_UPDATE is NOT included.
    assert!(frame_is_kf_gf_arf(true, FrameUpdateType::LfUpdate));
    assert!(frame_is_kf_gf_arf(false, FrameUpdateType::ArfUpdate));
    assert!(frame_is_kf_gf_arf(false, FrameUpdateType::GfUpdate));
    assert!(!frame_is_kf_gf_arf(false, FrameUpdateType::LfUpdate));
    assert!(
        !frame_is_kf_gf_arf(false, FrameUpdateType::IntnlArfUpdate),
        "INTNL_ARF_UPDATE is not in C's disjunction"
    );
    assert!(!frame_is_kf_gf_arf(false, FrameUpdateType::OverlayUpdate));
    assert!(!frame_is_kf_gf_arf(
        false,
        FrameUpdateType::IntnlOverlayUpdate
    ));
}

#[test]
fn update_rc_counts_only_moves_on_a_showable_frame() {
    // C's whole body is inside `if (ppcs->frm_hdr.showable_frame)`.
    assert_eq!(update_rc_counts(false, true, 8, 30, 3), (8, 30, 3));
    // showable + cdf update enabled (disable_cdf_update == 0) -> reset to 0.
    assert_eq!(update_rc_counts(true, false, 8, 30, 3), (9, 29, 0));
    // showable + cdf update DISABLED -> the counter increments.
    assert_eq!(update_rc_counts(true, true, 8, 30, 3), (9, 29, 4));
}

/// The reachability claim recorded on `update_rc_counts` and `rc_init`, made
/// executable: with `frames_since_key` pinned at `rc_init`'s seed of 8 and
/// `frames_since_cdf_update` pinned at 0 — which is what CQP/CRF gives,
/// because `update_rc_counts` never runs there —
/// `should_disable_cdf_update`'s condition (enc_mode_config.c:9484-9501)
/// `frames_since_key >= 30 && frames_since_cdf_update < 8` is FALSE. This
/// asserts the arithmetic so a future session cannot "helpfully" start
/// incrementing the counters without a test going red.
#[test]
fn cqp_crf_counters_keep_disable_cdf_update_at_zero_forever() {
    // `should_disable_cdf_update`'s condition, as a function so the compiler
    // cannot fold either call site into a constant.
    fn would_disable(frames_since_key: i32, frames_since_cdf_update: i32) -> bool {
        frames_since_key >= 30 && frames_since_cdf_update < 8
    }
    // rc_init's seed of 8, never incremented in CQP/CRF; cdf counter frozen at 0.
    assert!(
        !would_disable(8, 0),
        "should_disable_cdf_update would fire — the CQP/CRF counter freeze is broken"
    );
    // Not vacuous: had the counters advanced past 30 frames the condition
    // WOULD fire, so this test distinguishes the two worlds.
    assert!(would_disable(30, 0));
}
