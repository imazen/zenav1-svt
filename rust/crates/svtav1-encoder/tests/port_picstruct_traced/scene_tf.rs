use super::*;

pub(super) fn flat_hist(f: impl Fn(usize, usize, usize) -> u32) -> Box<pp::RegionHistograms> {
    let mut h = Box::new([[[0u32; 256]; 4]; 4]);
    for w in 0..4 {
        for y in 0..4 {
            for b in 0..256 {
                h[w][y][b] = f(w, y, b);
            }
        }
    }
    h
}

/// `calc_ahd` (`pd_process.c:55-84`) — the sum AND the active-region count.
///
/// Derivation: with a constant per-bin difference of `d` over 256 bins, each
/// region contributes `256 * d`, and the active-region test is
/// `ahd_per_region > region_width * region_height`. At 64x64 with a 4x4 grid
/// each region is 16x16 = 256 pixels, so `d = 1` gives exactly 256 which is
/// NOT `> 256` (no region counts), while `d = 2` gives 512 which is.
#[test]
pub(super) fn traced_calc_ahd_sum_and_active_region_threshold() {
    let a = flat_hist(|_, _, _| 10);
    let b1 = flat_hist(|_, _, _| 9);
    let (ahd, active) = pp::calc_ahd(&a, &b1, 64, 64, 4, 4);
    assert_eq!(ahd, 16 * 256, "16 regions x 256 bins x |10-9|");
    assert_eq!(active, 0, "256 is not > 256");

    let b2 = flat_hist(|_, _, _| 8);
    let (ahd, active) = pp::calc_ahd(&a, &b2, 64, 64, 4, 4);
    assert_eq!(ahd, 16 * 512);
    assert_eq!(active, 16, "512 > 256 in every region");

    // A smaller active region grid changes the pixel count per region and so
    // the threshold: at 2x2 over 64x64 each region is 32x32 = 1024 pixels, so
    // 512 no longer qualifies -- and only 4 regions are visited at all.
    let (ahd, active) = pp::calc_ahd(&a, &b2, 64, 64, 2, 2);
    assert_eq!(ahd, 4 * 512);
    assert_eq!(active, 0);

    // Identical histograms: zero, and no active region.
    assert_eq!(pp::calc_ahd(&a, &a, 64, 64, 4, 4), (0, 0));
}

/// `calc_ahd_pd` (`pd_process.c:5192-5215`) — the SIMPLER of the two, with no
/// region-size test at all.
///
/// The two functions are easy to conflate; this pins that `calc_ahd_pd`
/// returns only the sum and never looks at the picture dimensions.
#[test]
pub(super) fn traced_calc_ahd_pd_is_sum_only() {
    let a = flat_hist(|w, h, b| (w * 4 + h + b) as u32);
    let b = flat_hist(|w, h, b| (w * 4 + h + b) as u32 + 3);
    assert_eq!(pp::calc_ahd_pd(&a, &b, 4, 4), 16 * 256 * 3);
    assert_eq!(pp::calc_ahd_pd(&a, &b, 2, 2), 4 * 256 * 3);
    assert_eq!(pp::calc_ahd_pd(&a, &b, 1, 1), 256 * 3);
    assert_eq!(pp::calc_ahd_pd(&a, &a, 4, 4), 0);
}

/// `copy_histograms` (`pd_process.c:4703-4719`) — it copies the FULL 4x4 grid.
///
/// The loops run over `MAX_NUMBER_OF_REGIONS_IN_{WIDTH,HEIGHT}`, not over the
/// sequence's active region counts, so regions the detector never reads are
/// still refreshed. A port that copied only the active regions would leave
/// stale data behind and diverge the moment the region count changed.
#[test]
pub(super) fn traced_copy_histograms_copies_the_full_grid() {
    let mut state = pp::SceneDetectState::default();
    let h = flat_hist(|w, y, b| (w * 1000 + y * 100 + b) as u32);
    let mut inten = [[0u64; 4]; 4];
    for w in 0..4 {
        for y in 0..4 {
            inten[w][y] = (w * 10 + y) as u64;
        }
    }
    pp::copy_histograms(&mut state, &h, &inten);
    for w in 0..4 {
        for y in 0..4 {
            assert_eq!(
                state.prev_picture_histogram[w][y][255], h[w][y][255],
                "[{w}][{y}]"
            );
            assert_eq!(state.prev_average_intensity_per_region[w][y], inten[w][y]);
        }
    }
    // Including the corner a 2x2 detector would never touch.
    assert_eq!(state.prev_picture_histogram[3][3][0], h[3][3][0]);
}

/// `num_64x64_in_pic` — the `NUM64x64INPIC` macro's shift.
///
/// `svt_log2f(BLOCK_SIZE_64) << 1` is `6 << 1` = 12, so the macro is
/// `(w * h) >> 12`, not a division by 64.
#[test]
pub(super) fn traced_num_64x64_in_pic_shift() {
    assert_eq!(pp::num_64x64_in_pic(64, 64), 1);
    assert_eq!(pp::num_64x64_in_pic(1920, 1080), (1920 * 1080) >> 12);
    assert_eq!(
        pp::num_64x64_in_pic(63, 63),
        0,
        "a sub-64x64 region rounds to zero"
    );
    assert_eq!(pp::num_64x64_in_pic(0, 0), 0);
}

/// `scene_transition_detector` (`pd_process.c:256-378`) — the accumulating
/// region size, which is the quirk that makes this function hard to port.
///
/// Derivation. `region_width` and `region_height` are declared OUTSIDE the
/// region loops and updated inside with `+=`. At 1918x1078 with a 4x4 grid the
/// base sizes are 479 and 269, and the remainders are
/// `1918 - 4*479 = 2` and `1078 - 4*269 = 2`. The height remainder is added on
/// EVERY last-height iteration, i.e. once per width column, so `region_height`
/// is 269, 271, 273, 275 in successive columns; the width remainder is added
/// on every iteration of the FINAL column, so `region_width` walks 479 -> 487
/// there. The threshold therefore differs between regions of identical actual
/// size.
///
/// This test does not re-derive the whole detector; it pins the observable
/// consequence: with a picture size that divides evenly the verdict is
/// insensitive to which region carries a given difference, and with one that
/// does not, it is not.
#[test]
pub(super) fn traced_scene_transition_detector_region_size_accumulates() {
    // A difference big enough to trip the threshold in a SMALL region but not
    // in a large one, placed in the last region (largest accumulated size).
    let make = |spike_w: usize, spike_h: usize, amount: u32| {
        (
            flat_hist(move |w, y, _| {
                if w == spike_w && y == spike_h {
                    amount
                } else {
                    0
                }
            }),
            flat_hist(|_, _, _| 0),
        )
    };

    // Evenly divisible: 1024x1024 over 4x4 gives 256x256 regions with zero
    // remainder, so no accumulation happens and every region has the same
    // threshold. The same spike must give the same per-region verdict wherever
    // it sits.
    let mut verdicts_even = Vec::new();
    for (sw, sh) in [(0usize, 0usize), (3, 3), (1, 2)] {
        let (cur, prev) = make(sw, sh, 40_000);
        let mut st = pp::SceneDetectState {
            prev_picture_histogram: prev,
            ..Default::default()
        };
        let cur_i = [[100u64; 4]; 4];
        let fut_i = [[200u64; 4]; 4];
        let r = pp::scene_transition_detector(&mut st, &cur, &cur_i, &fut_i, 1024, 1024, 4, 4);
        verdicts_even.push((r, st.ahd_running_avg[sw][sh]));
    }
    assert!(
        verdicts_even.iter().all(|v| *v == verdicts_even[0]),
        "with no remainder the region position must not matter: {verdicts_even:?}"
    );

    // The running average update: when NO abrupt change is seen, the region's
    // average moves to (3*avg + ahd)/4. Starting from 0 with ahd 0, it stays 0.
    let (cur, prev) = make(0, 0, 0);
    let mut st = pp::SceneDetectState {
        prev_picture_histogram: prev,
        ..Default::default()
    };
    let flat_i = [[100u64; 4]; 4];
    let changed = pp::scene_transition_detector(&mut st, &cur, &flat_i, &flat_i, 1024, 1024, 4, 4);
    assert!(!changed, "identical histograms are not a scene change");
    assert_eq!(st.ahd_running_avg, [[0u32; 4]; 4]);
    assert!(!st.reset_running_avg, "no region was abrupt");

    // reset_running_avg latches when at least half the regions are abrupt, and
    // on the NEXT call it seeds the running average with the raw ahd instead
    // of blending. Drive two frames to reach it.
    let (cur, prev) = (flat_hist(|_, _, _| 5000), flat_hist(|_, _, _| 0));
    let mut st = pp::SceneDetectState {
        prev_picture_histogram: prev,
        ..Default::default()
    };
    let cur_i = [[10u64; 4]; 4];
    let fut_i = [[200u64; 4]; 4];
    let _ = pp::scene_transition_detector(&mut st, &cur, &cur_i, &fut_i, 1024, 1024, 4, 4);
    assert!(
        st.reset_running_avg,
        "every region difference is huge -> all abrupt"
    );
    // My first hand-derivation of the next two steps was WRONG and the test
    // caught it. On frame 1 `reset_running_avg` is still FALSE (C's ctor
    // zeroes it), so the seed does not happen; and the region IS abrupt, so
    // the blend in the else-branch does not happen either. The average is
    // therefore untouched at 0 after frame 1 — the latch only takes effect on
    // the NEXT call.
    assert_eq!(
        st.ahd_running_avg[0][0], 0,
        "frame 1 neither seeds nor blends"
    );
    let _ = pp::scene_transition_detector(&mut st, &cur, &cur_i, &fut_i, 1024, 1024, 4, 4);
    // Each region's ahd is 256 bins x |5000 - 0|.
    assert_eq!(
        st.ahd_running_avg[0][0],
        256 * 5000,
        "with reset latched the average is SEEDED with ahd, not blended"
    );
}

/// `perform_scene_change_detection` (`pd_process.c:4682-4700`) — which arm runs.
///
/// Measured settings, not guesses: `static_config.scene_change_detection` is
/// force-zeroed (`enc_settings.c:839-843`) so the first arm is dead in
/// mainline, while `vq_ctrls.sharpness_ctrls.scene_transition` is 1 in both
/// arms of `derive_vq_params` and zeroed only for LOW_DELAY
/// (`enc_handle.c:3282, 3291, 3324-3326`), so the SECOND arm is live in random
/// access.
#[test]
pub(super) fn traced_perform_scene_change_detection_arms() {
    // Arm 1 (dead in mainline): the detector result becomes scene_change_flag
    // and cra_flag is forced true.
    let o = pp::perform_scene_change_detection(true, true, -1, false, || true);
    assert!(o.scene_change_flag && o.cra_flag && o.is_scene_change_detected);
    assert_eq!(
        o.transition_detected, -1,
        "arm 1 never touches transition_detected"
    );

    // Arm 2 (live in RA): scene_change_flag stays false and the result lands in
    // transition_detected instead.
    let o = pp::perform_scene_change_detection(false, true, -1, false, || true);
    assert!(!o.scene_change_flag && !o.cra_flag);
    assert_eq!(o.transition_detected, 1);

    let o = pp::perform_scene_change_detection(false, true, 0, false, || false);
    assert_eq!(o.transition_detected, 0);

    // Already latched at 1: the detector is NOT re-run and the value stands.
    let o = pp::perform_scene_change_detection(false, true, 1, false, || {
        panic!("the detector must not run while transition_detected is latched")
    });
    assert_eq!(o.transition_detected, 1);

    // Sharpness transition off (LOW_DELAY): neither arm runs.
    let o = pp::perform_scene_change_detection(false, false, -1, true, || {
        panic!("the detector must not run with both gates off")
    });
    assert!(!o.scene_change_flag);
    assert!(o.cra_flag, "an incoming cra_flag survives");
    assert_eq!(o.transition_detected, -1);
}

// ---------------------------------------------------------------------------
// Dynamic-GOP split decision — tier 4 (the HME half is tier 1, see
// c_parity_picstruct_dg.rs)
// ---------------------------------------------------------------------------

/// `early_hme`'s reduction (`pd_process.c:669-684`).
///
/// Derivation, and the trap: the block counts here use a HARDCODED 64
/// (`(aligned_width + 63) / 64`), while `dg_detector_hme_level0` divides by
/// `scs->b64_size`. The two agree at the default b64_size of 64 and would not
/// at 128 — reproduced rather than unified.
#[test]
pub(super) fn traced_early_hme_reduce() {
    let m = pp::DgDetectorMetrics {
        tot_dist: 40_000,
        tot_cplx: 5,
        tot_active: 15,
        sum_in_vectors: -10,
        seg_completed: 1,
    };
    // 320x192 -> 5 x 3 = 20 blocks (192/64 = 3 exactly, 320/64 = 5).
    let r = pp::early_hme_reduce(&m, 320, 192);
    assert_eq!(r.norm_dist, 40_000 / 15);
    assert_eq!(u32::from(r.perc_cplx), (5u32 * 100) / 15);
    assert_eq!(u32::from(r.perc_active), (15u32 * 100) / 15);
    assert_eq!(i32::from(r.mv_in_out_count), (-10i32 * 100) / 15);

    // A partial block row/column rounds UP: 65x65 is 2x2 blocks, not 1x1.
    let r = pp::early_hme_reduce(&m, 65, 65);
    assert_eq!(r.norm_dist, 40_000 / 4);

    // sum_in_vectors is signed, and the percentage keeps its sign.
    let neg = pp::DgDetectorMetrics {
        sum_in_vectors: -400,
        ..m
    };
    assert!(pp::early_hme_reduce(&neg, 640, 640).mv_in_out_count < 0);
}

/// `calc_mini_gop_activity` (`pd_process.c:686-712`) — the split predicate.
///
/// Derivation of the three conditions:
/// * `cond1` requires the TOP layer to be ≥ 95 % active AND rules out the two
///   lopsided sub-layer combinations (one ≥ 95 while the other < 75). It gates
///   everything: without it neither `cond2` nor `cond3` can fire.
/// * `cond2` is the distortion test, and its `bias` is 25 when the previous
///   mini-GOP in this GOP was 5L and this is not the first mini-GOP, else 75 —
///   a hysteresis toward staying at 6L.
/// * `cond3` is the motion test: MIN of the two sub-layer counts > 40 AND MAX
///   > 55.
#[test]
pub(super) fn traced_calc_mini_gop_activity_conditions() {
    // A baseline that satisfies cond1 and cond2 with bias 75.
    let base = |bias_prev_5l: bool| {
        pp::calc_mini_gop_activity(
            if bias_prev_5l { 2 } else { 0 },
            if bias_prev_5l { 5 } else { 4 },
            10_000, // top dist, > LOW_DIST_TH
            95,     // top active
            10,     // top cplx, > 0
            1_000,  // sub0 dist, < HIGH_DIST_TH
            95,     // sub0 active
            10,     // sub0 cplx, < 25
            1_000,  // sub1 dist
            95,     // sub1 active
            10,     // sub1 cplx
            0,      // top mv count (UNUSED in C)
            0,      // sub mv count 1
            0,      // sub mv count 2
        )
    };
    // bias 75: (1000 + 1000)/2 = 1000 < (75 * 10000)/100 = 7500 -> splits.
    assert!(base(false));
    // bias 25: 1000 < (25 * 10000)/100 = 2500 -> still splits.
    assert!(base(true));

    // Raise the sub-layer distortion so bias 75 passes and bias 25 does not.
    // The value must stay UNDER HIGH_DIST_TH (16*16*18 = 4608) or cond2 fails
    // for a different reason — my first attempt used 5000 and failed on
    // exactly that, which is why the number is spelled out here.
    // (4000 + 4000)/2 = 4000; 75% of 10000 is 7500, 25% is 2500.
    let with_dist = |bias_prev_5l: bool| {
        pp::calc_mini_gop_activity(
            if bias_prev_5l { 2 } else { 0 },
            if bias_prev_5l { 5 } else { 4 },
            10_000,
            95,
            10,
            4_000,
            95,
            10,
            4_000,
            95,
            10,
            0,
            0,
            0,
        )
    };
    assert!(with_dist(false), "bias 75 admits the split");
    assert!(
        !with_dist(true),
        "bias 25 is the hysteresis toward staying at 6L"
    );

    // cond1 gates everything: drop the top-layer activity below 95.
    assert!(!pp::calc_mini_gop_activity(
        0, 4, 10_000, 94, 10, 1_000, 95, 10, 1_000, 95, 10, 0, 0, 0
    ));
    // The lopsided sub-layer cases: (>=95, <75) and (<75, >=95).
    assert!(!pp::calc_mini_gop_activity(
        0, 4, 10_000, 95, 10, 1_000, 95, 10, 1_000, 74, 10, 0, 0, 0
    ));
    assert!(!pp::calc_mini_gop_activity(
        0, 4, 10_000, 95, 10, 1_000, 74, 10, 1_000, 95, 10, 0, 0, 0
    ));

    // cond3 alone: cond2 fails (top cplx 0) but the motion test carries it.
    assert!(pp::calc_mini_gop_activity(
        0, 4, 10_000, 95, 0, 1_000, 95, 10, 1_000, 95, 10, 0, 41, 56
    ));
    // MIN must exceed 40 AND MAX must exceed 55 — 41/55 fails on the MAX.
    assert!(!pp::calc_mini_gop_activity(
        0, 4, 10_000, 95, 0, 1_000, 95, 10, 1_000, 95, 10, 0, 41, 55
    ));
    // 40/56 fails on the MIN.
    assert!(!pp::calc_mini_gop_activity(
        0, 4, 10_000, 95, 0, 1_000, 95, 10, 1_000, 95, 10, 0, 40, 56
    ));

    // The top-layer mv count is (void)-cast unused in C: changing it alone
    // must change nothing.
    let a =
        pp::calc_mini_gop_activity(0, 4, 10_000, 95, 0, 1_000, 95, 10, 1_000, 95, 10, 0, 41, 56);
    let b = pp::calc_mini_gop_activity(
        0, 4, 10_000, 95, 0, 1_000, 95, 10, 1_000, 95, 10, 32_767, 41, 56,
    );
    assert_eq!(a, b, "top_layer_mv_in_out_count is unused");

    // HIGH_DIST_TH / LOW_DIST_TH are exclusive bounds.
    assert!(
        !pp::calc_mini_gop_activity(
            0,
            4,
            pp::LOW_DIST_TH,
            95,
            10,
            1_000,
            95,
            10,
            1_000,
            95,
            10,
            0,
            0,
            0
        ),
        "top_layer_dist must be strictly greater than LOW_DIST_TH"
    );
    assert!(
        !pp::calc_mini_gop_activity(
            0,
            4,
            10_000,
            95,
            10,
            pp::HIGH_DIST_TH,
            95,
            10,
            1_000,
            95,
            10,
            0,
            0,
            0
        ),
        "sub_layer_dist0 must be strictly less than HIGH_DIST_TH"
    );
}

/// `eval_sub_mini_gop` (`pd_process.c:713-758`) — the pass-to-layer mapping.
///
/// The three `early_hme` passes run in the order (end,start), (end,mid),
/// (mid,start), and are handed to `calc_mini_gop_activity` as
/// TOP=(end,start), SUB0=(mid,start), SUB1=(end,mid) — NOT in run order.
///
/// **A finding, and it explains a C oddity.** `calc_mini_gop_activity` is
/// FULLY SYMMETRIC in its two sub layers: the distortion enters as
/// `(d0 + d1) / 2`, the two activity guards are mirror images, the two
/// complexity guards are identical, and cond3 takes a MIN and a MAX. So
/// swapping SUB0 and SUB1 can never change the verdict — which is why C's call
/// site can pass `sub_layer_mv_in_out_count1` the (end,mid) value and
/// `..._count2` the (mid,start) value, inverted relative to the layer indices
/// they are named after, without anyone noticing. What DOES matter is the
/// TOP-vs-SUB assignment, and that is asserted below.
#[test]
pub(super) fn traced_eval_sub_mini_gop_layer_mapping() {
    let r = |dist: u64, active: u8, cplx: u8, mv: i16| pp::EarlyHmeResult {
        mv_in_out_count: mv,
        norm_dist: dist,
        perc_cplx: cplx,
        perc_active: active,
    };
    let end_start = r(10_000, 95, 10, 10);
    let end_mid = r(4_000, 95, 10, 20);
    let mid_start = r(1_000, 95, 10, 30);

    // Sub-layer symmetry: swapping the two sub inputs is inert.
    assert_eq!(
        pp::eval_sub_mini_gop(0, 4, end_start, end_mid, mid_start),
        pp::eval_sub_mini_gop(0, 4, end_start, mid_start, end_mid),
        "calc_mini_gop_activity is symmetric in its two sub layers"
    );

    // TOP-vs-SUB is NOT symmetric. With (end,start) as the top layer the
    // distortion test passes: (1000 + 4000)/2 = 2500 < 75% of 10000.
    assert!(pp::eval_sub_mini_gop(0, 4, end_start, end_mid, mid_start));
    // Promote (mid,start) to the top layer instead and it fails: the top
    // distortion is now 1000, and (10000 + 4000)/2 = 7000 is not < 750 — and
    // 10000 also exceeds HIGH_DIST_TH as a sub layer.
    assert!(!pp::eval_sub_mini_gop(0, 4, mid_start, end_mid, end_start));
}

/// `commit_sub_mini_gop_split` (`pd_process.c:707-711`) — the array write.
#[test]
pub(super) fn traced_commit_sub_mini_gop_split() {
    let mut map = pp::MiniGopMap::for_sequence(5);
    map.activity[pp::L6_INDEX] = false;
    map.activity[pp::L5_0_INDEX] = true;
    map.activity[pp::L5_1_INDEX] = true;
    pp::commit_sub_mini_gop_split(&mut map, true, pp::L6_INDEX, pp::L5_0_INDEX, pp::L5_1_INDEX);
    assert!(
        map.activity[pp::L6_INDEX],
        "the 6L shape becomes ACTIVE (to be split)"
    );
    assert!(!map.activity[pp::L5_0_INDEX] && !map.activity[pp::L5_1_INDEX]);

    // A false verdict writes NOTHING — C's whole block is inside the `if`.
    let mut map = pp::MiniGopMap::for_sequence(5);
    map.activity[pp::L6_INDEX] = false;
    map.activity[pp::L5_0_INDEX] = true;
    pp::commit_sub_mini_gop_split(
        &mut map,
        false,
        pp::L6_INDEX,
        pp::L5_0_INDEX,
        pp::L5_1_INDEX,
    );
    assert!(!map.activity[pp::L6_INDEX]);
    assert!(map.activity[pp::L5_0_INDEX]);
}

// ---------------------------------------------------------------------------
// Temporal-filter window — tier 4
// ---------------------------------------------------------------------------

/// `ref_pics_modulation` (`pd_process.c:3642-3745`).
///
/// Derivation of the three shapes:
/// * I slice: three Q16 log1p thresholds, 26572 / 45426 / 71998, yielding
///   6 / 4 / 2 / 0. LOWER noise buys MORE frames — the opposite of the
///   intuitive direction, and the C comment says why.
/// * base layer: five `modulate_pics` tables keyed on
///   `ratio = filt_to_unfilt_diff * 100 / noise`.
/// * non-base: three tables, each a single threshold yielding 0 or 1, and
///   there is NO `case 4` — `modulate_pics == 4` falls through to 0 there
///   while base layer gives 0/1/2.
#[test]
pub(super) fn traced_ref_pics_modulation_three_shapes() {
    let c = |modulate: u8| pp::TfCtrls {
        modulate_pics: modulate,
        ..Default::default()
    };

    // I slice: the three noise bands and the fall-through.
    for (noise, want) in [
        (0i32, 6),
        (26571, 6),
        (26572, 4),
        (45425, 4),
        (45426, 2),
        (71997, 2),
        (71998, 0),
    ] {
        assert_eq!(
            pp::ref_pics_modulation(true, 0, &c(1), noise, 0, 1, 1),
            want,
            "I slice at noise {noise}"
        );
    }
    // The I-slice arm ignores modulate_pics entirely.
    for m in 0u8..=4 {
        assert_eq!(pp::ref_pics_modulation(true, 0, &c(m), 0, 0, 1, 1), 6);
    }

    // Base layer. ratio = diff * 100 / noise; pick noise = 100 so ratio == diff.
    let base = |m: u8, ratio: u32| pp::ref_pics_modulation(false, 0, &c(m), 100, ratio, 1, 1);
    assert_eq!(base(0, 999), 0, "modulate_pics 0 is always zero");
    assert_eq!((base(1, 99), base(1, 100)), (5, pp::TF_MAX_EXTENSION));
    assert_eq!(
        (base(2, 49), base(2, 99), base(2, 100)),
        (3, 5, pp::TF_MAX_EXTENSION)
    );
    assert_eq!((base(3, 49), base(3, 99), base(3, 100)), (3, 4, 5));
    assert_eq!((base(4, 49), base(4, 99), base(4, 100)), (0, 1, 2));
    assert_eq!(base(5, 100), 0, "an unknown level falls through to zero");

    // Non-base: single thresholds, and NO case 4.
    let nb = |m: u8, ratio: u32| pp::ref_pics_modulation(false, 1, &c(m), 100, ratio, 1, 1);
    assert_eq!((nb(1, 24), nb(1, 25)), (0, 1));
    assert_eq!((nb(2, 49), nb(2, 50)), (0, 1));
    assert_eq!((nb(3, 74), nb(3, 75)), (0, 1));
    assert_eq!(nb(4, 999), 0, "non-base has no case 4 -- it falls through");
    assert_eq!(nb(0, 999), 0);

    // Zero noise short-circuits the ratio to 0 rather than dividing.
    assert_eq!(pp::ref_pics_modulation(false, 0, &c(1), 0, 9_999, 1, 1), 5);

    // qp_opt applies DIVIDE_AND_ROUND(offset * q_weight, q_weight_denom).
    let qp = pp::TfCtrls {
        modulate_pics: 1,
        qp_opt: true,
        ..Default::default()
    };
    // offset 5, weight 3/4 -> (15 + 2) / 4 = 4 (rounds, not truncates).
    assert_eq!(pp::ref_pics_modulation(false, 0, &qp, 100, 99, 3, 4), 4);
    // weight 1/1 is the identity.
    assert_eq!(pp::ref_pics_modulation(false, 0, &qp, 100, 99, 1, 1), 5);
}

/// `derive_tf_window_params`' count derivation, all four arms.
///
/// The arms differ in ways that blur together on a quick read; each assertion
/// below names the difference it pins.
#[test]
pub(super) fn traced_derive_tf_window_counts_per_arm() {
    let ctrls = pp::TfCtrls {
        enabled: true,
        modulate_pics: 1,
        num_past_pics: 2,
        num_future_pics: 2,
        max_num_past_pics: 8,
        max_num_future_pics: 8,
        ..Default::default()
    };

    // LOW DELAY: offset applies (modulate_pics != 0), no per-struct cap.
    let c = pp::derive_tf_window_counts(pp::TfWindowArm::LowDelay, &ctrls, 3, 5, 0);
    assert_eq!((c.num_past_pics, c.num_future_pics), (5, 5));
    // With modulate_pics 0 the offset is dropped even if the caller passes one.
    let no_mod = pp::TfCtrls {
        modulate_pics: 0,
        ..ctrls
    };
    let c = pp::derive_tf_window_counts(pp::TfWindowArm::LowDelay, &no_mod, 3, 5, 0);
    assert_eq!((c.num_past_pics, c.num_future_pics), (2, 2));
    // max_num_* caps.
    let capped = pp::TfCtrls {
        max_num_past_pics: 3,
        max_num_future_pics: 3,
        ..ctrls
    };
    let c = pp::derive_tf_window_counts(pp::TfWindowArm::LowDelay, &capped, 3, 5, 0);
    assert_eq!((c.num_past_pics, c.num_future_pics), (3, 3));

    // DELAYED INTRA: NO past pictures, and the future cap is the I_SLICE row
    // of tf_max_ref_per_struct, i.e. 1 << hierarchical_levels.
    for (hier, want) in [(0u32, 1i32), (1, 2), (2, 4), (3, 5)] {
        let c = pp::derive_tf_window_counts(pp::TfWindowArm::DelayedIntra, &ctrls, 3, hier, 0);
        assert_eq!(c.num_past_pics, 0, "delayed intra never filters the past");
        assert_eq!(c.num_future_pics, want, "hier {hier}");
    }
    // RANDOM-ACCESS IDR takes the same shape.
    let a = pp::derive_tf_window_counts(pp::TfWindowArm::DelayedIntra, &ctrls, 3, 2, 0);
    let b = pp::derive_tf_window_counts(pp::TfWindowArm::RandomAccessIdr, &ctrls, 3, 2, 0);
    assert_eq!(a, b);

    // RANDOM-ACCESS INTER: the MAX(1, ...) floor exists only here, so a
    // negative modulation cannot empty the window.
    let c = pp::derive_tf_window_counts(pp::TfWindowArm::RandomAccessInter, &ctrls, -10, 5, 0);
    assert_eq!((c.num_past_pics, c.num_future_pics), (1, 1));
    // The low-delay arm has NO such floor: the same negative offset drives it
    // below zero.
    let c = pp::derive_tf_window_counts(pp::TfWindowArm::LowDelay, &ctrls, -10, 5, 0);
    assert_eq!((c.num_past_pics, c.num_future_pics), (-8, -8));

    // The inter arm's per-struct cap keys off the temporal layer: BASE takes
    // row 1 (7 each side), non-base takes row 2 (1 below 6L, 2 at 6L).
    let wide = pp::TfCtrls {
        num_past_pics: 9,
        num_future_pics: 9,
        max_num_past_pics: 30,
        max_num_future_pics: 30,
        ..ctrls
    };
    let c = pp::derive_tf_window_counts(pp::TfWindowArm::RandomAccessInter, &wide, 0, 4, 0);
    assert_eq!(
        (c.num_past_pics, c.num_future_pics),
        (
            i32::from(pp::TF_MAX_BASE_REF_PICS),
            i32::from(pp::TF_MAX_BASE_REF_PICS)
        )
    );
    let c = pp::derive_tf_window_counts(pp::TfWindowArm::RandomAccessInter, &wide, 0, 4, 1);
    assert_eq!(
        (c.num_past_pics, c.num_future_pics),
        (
            i32::from(pp::TF_MAX_L1_REF_PICS_SUB_6L),
            i32::from(pp::TF_MAX_L1_REF_PICS_SUB_6L)
        )
    );
    let c = pp::derive_tf_window_counts(pp::TfWindowArm::RandomAccessInter, &wide, 0, 5, 1);
    assert_eq!(
        (c.num_past_pics, c.num_future_pics),
        (
            i32::from(pp::TF_MAX_L1_REF_PICS_6L),
            i32::from(pp::TF_MAX_L1_REF_PICS_6L)
        )
    );
}

/// The past-window compaction (`pd_process.c:3914-3920`, `:4091-4098`).
///
/// **This block is DEAD CODE in C** and the port keeps it anyway
/// (`docs/WORKING-ON-THIS.md` §7; written up as
/// `docs/SUSPECTED-C-BUGS.md` #18). `actual_past_pics` is initialised to
/// `num_past_pics` and never modified — only `actual_future_pics` is
/// incremented — so the guard `actual_past_pics != num_past_pics` is always
/// false. Verified by `grep -n actual_past_pics Codec/pd_process.c`: two
/// initialisations, two `past_altref_nframes` assignments, two comparisons, no
/// decrement.
///
/// The first assertion below is the one that found it. My hand-derivation
/// expected a front-holed list (past slots 2 and 3 filled, 0 and 1 NULL — the
/// shape the block was written for) to compact to the front. It does not:
/// C's loop is `while (list[pic_i] != NULL)` starting at index 0, so on
/// exactly that shape it stops immediately. Fixing the counter alone would
/// not fix the block.
#[test]
pub(super) fn traced_compact_tf_past_window() {
    // The shape the block was WRITTEN for: holes at the front. C's loop bound
    // makes it a no-op, and so does the port's.
    let mut list = [None; pp::ALTREF_MAX_NFRAMES];
    list[2] = Some(20);
    list[3] = Some(30);
    list[4] = Some(40); // centre
    list[5] = Some(50);
    list[6] = Some(60);
    let before = list;
    pp::compact_tf_past_window(&mut list, 4, 2);
    assert_eq!(list, before, "the `while != NULL` bound stops at index 0");

    // With no leading hole the shift does happen, which is what the block
    // would do if the counter ever differed.
    let mut list = [None; pp::ALTREF_MAX_NFRAMES];
    list[0] = Some(10);
    list[1] = Some(20);
    list[2] = Some(30);
    list[3] = Some(40);
    list[4] = Some(50);
    pp::compact_tf_past_window(&mut list, 4, 2);
    assert_eq!(&list[..3], &[Some(30), Some(40), Some(50)]);

    // No shortfall: untouched, and the guard is not even entered.
    let mut list = [None; pp::ALTREF_MAX_NFRAMES];
    list[0] = Some(1);
    list[1] = Some(2);
    let before = list;
    pp::compact_tf_past_window(&mut list, 1, 1);
    assert_eq!(list, before);
}

/// The `tf_avg_luma` / `tf_avg_ahd_error` reduction
/// (`pd_process.c:4101-4118`) — the CENTRE picture is excluded.
#[test]
pub(super) fn traced_tf_window_averages_excludes_the_centre() {
    // past 2, centre at index 2, future 1: indices 0, 1, 3 contribute.
    let luma = [100u64, 200, 999_999, 300, 0];
    let err = [10i32, 20, 999_999, 30, 0];
    let (avg_luma, avg_err) = pp::tf_window_averages(&luma, &err, 2, 1);
    assert_eq!(avg_luma, (100 + 200 + 300) / 3);
    assert_eq!(avg_err, (10 + 20 + 30) / 3);

    // An empty window returns zeros without dividing.
    assert_eq!(pp::tf_window_averages(&luma, &err, 0, 0), (0, 0));

    // Centre at index 0 (no past pictures) excludes index 0.
    let (avg_luma, _) = pp::tf_window_averages(&[999_999, 10, 20], &[0, 1, 2], 0, 2);
    assert_eq!(avg_luma, (10 + 20) / 2);
}

/// `low_delay_store_tf_pictures`' store predicate (`pd_process.c:4132`).
///
/// Derivation at hierarchical_levels 3 (mg_size 8) with one past picture:
/// the predicate is `pic_idx_in_mg + 1 + 1 >= 8`, so only index 6 and 7
/// qualify — the last two non-base pictures of the mini-GOP.
#[test]
pub(super) fn traced_low_delay_store_tf_picture_predicate() {
    for idx in 0u32..8 {
        let want = idx >= 6;
        assert_eq!(
            pp::low_delay_should_store_tf_picture(1, idx, 1, 3),
            want,
            "pic_idx_in_mg {idx}"
        );
    }
    // A BASE picture never joins the ring, however late it sits.
    assert!(!pp::low_delay_should_store_tf_picture(0, 7, 1, 3));
    // More past pictures widen the window backwards.
    assert!(pp::low_delay_should_store_tf_picture(1, 4, 3, 3));
    assert!(!pp::low_delay_should_store_tf_picture(1, 3, 3, 3));
}

/// `mctf_frame`'s decision half (`pd_process.c:4194-4250`).
///
/// The trap this pins: the STORE and RELEASE gates are NOT symmetric — RELEASE
/// additionally requires `temporal_layer_index == 0`, because the ring is
/// filled by non-base pictures and drained by the base picture that consumed
/// it.
#[test]
pub(super) fn traced_mctf_frame_decision_store_and_release_are_asymmetric() {
    let ld = pp::PredStructure::LowDelay;
    let ra = pp::PredStructure::RandomAccess;

    // Low delay, base TF enabled, at a NON-base picture: store but do not
    // release.
    let d = pp::mctf_frame_decision(ld, true, true, 1, 0);
    assert!(d.store_ld_tf_pictures && !d.release_ld_tf_pictures);

    // The same at the BASE picture: both.
    let d = pp::mctf_frame_decision(ld, true, true, 0, 0);
    assert!(d.store_ld_tf_pictures && d.release_ld_tf_pictures);

    // Random access: neither, whatever the layer.
    for tl in [0u8, 1] {
        let d = pp::mctf_frame_decision(ra, true, true, tl, 0);
        assert!(!d.store_ld_tf_pictures && !d.release_ld_tf_pictures);
    }

    // tf_ctrls disabled clears do_tf and skips the filter, but the ring
    // operations are gated on the SEQUENCE params, not on tf_ctrls -- so they
    // still run.
    let d = pp::mctf_frame_decision(ld, true, false, 0, 0);
    assert!(!d.run_tf && d.do_tf_cleared);
    assert!(d.store_ld_tf_pictures && d.release_ld_tf_pictures);

    // is_noise_level is a plain threshold on the LAST I picture's noise.
    assert!(!pp::mctf_frame_decision(ra, false, true, 0, pp::VQ_NOISE_LVL_TH - 1).is_noise_level);
    assert!(pp::mctf_frame_decision(ra, false, true, 0, pp::VQ_NOISE_LVL_TH).is_noise_level);
}

/// `mctf_frame`'s motion-direction verdict (`pd_process.c:4232-4238`).
///
/// The margin is `other * 6 / 4` with INTEGER division on the right, so at
/// `vert == 1` the threshold is 1 (not 1.5) and `horz == 2` already wins.
#[test]
pub(super) fn traced_tf_motion_direction() {
    assert_eq!(pp::tf_motion_direction(0, 0), -1);
    assert_eq!(pp::tf_motion_direction(10, 10), -1);
    // 1.5x margin at larger counts.
    assert_eq!(pp::tf_motion_direction(151, 100), 0);
    assert_eq!(pp::tf_motion_direction(150, 100), -1);
    assert_eq!(pp::tf_motion_direction(100, 151), 1);
    // The truncation at small counts: vert 1 -> threshold 6/4 = 1.
    assert_eq!(pp::tf_motion_direction(2, 1), 0);
    assert_eq!(pp::tf_motion_direction(1, 1), -1);
    // Zero on one side always picks the other, if it is non-zero.
    assert_eq!(pp::tf_motion_direction(1, 0), 0);
    assert_eq!(pp::tf_motion_direction(0, 1), 1);
}

/// `svt_aom_tf_max_ref_per_struct` — the port copy, gated at tier 1 in
/// `c_parity_picstruct.rs`. Repeated here only for the two shape facts a
/// reader needs while reading the window derivation above.
#[test]
pub(super) fn traced_tf_max_ref_per_struct_shape() {
    // Only the I_SLICE row grows with the hierarchy.
    assert_eq!(pp::tf_max_ref_per_struct(3, 0, false), 8);
    assert_eq!(
        pp::tf_max_ref_per_struct(3, 1, false),
        pp::TF_MAX_BASE_REF_PICS
    );
    assert_eq!(
        pp::tf_max_ref_per_struct(4, 2, false),
        pp::TF_MAX_L1_REF_PICS_SUB_6L
    );
    assert_eq!(
        pp::tf_max_ref_per_struct(5, 2, false),
        pp::TF_MAX_L1_REF_PICS_6L
    );
    // `direction` is (void)-cast in C.
    for ty in 0u8..=2 {
        assert_eq!(
            pp::tf_max_ref_per_struct(4, ty, false),
            pp::tf_max_ref_per_struct(4, ty, true)
        );
    }
}

/// `mctf_frame_st`'s step ORDER (`pd_process.c:4175-4193`).
///
/// The callees live in other modules; what this port owns is the order, and
/// the order is the content: `me_type = ME_MCTF` must precede
/// `svt_aom_sig_deriv_me_tf` (which branches on it) and the global-motion
/// pre-pass must precede the first segment.
#[test]
pub(super) fn traced_mctf_frame_st_sequence() {
    use pp::MctfStStep::{
        ConsumeDoneSemaphore, GmPreProcessor, InitTemporalFilteringSegment, SetMeTypeMctf,
        SigDerivMeTf,
    };
    assert_eq!(
        pp::mctf_frame_st_sequence(3, true),
        vec![
            SetMeTypeMctf,
            SigDerivMeTf,
            GmPreProcessor,
            InitTemporalFilteringSegment(0),
            InitTemporalFilteringSegment(1),
            InitTemporalFilteringSegment(2),
            ConsumeDoneSemaphore,
        ]
    );
    // The global-motion pre-pass is conditional; nothing else is.
    assert_eq!(
        pp::mctf_frame_st_sequence(1, false),
        vec![
            SetMeTypeMctf,
            SigDerivMeTf,
            InitTemporalFilteringSegment(0),
            ConsumeDoneSemaphore,
        ]
    );
    // Zero segments still posts and consumes nothing but the framing steps.
    assert_eq!(
        pp::mctf_frame_st_sequence(0, false),
        vec![SetMeTypeMctf, SigDerivMeTf, ConsumeDoneSemaphore]
    );
}

/// The low-delay temporal-filter ring: store predicate + drain
/// (`pd_process.c:4127-4174`).
#[test]
pub(super) fn traced_low_delay_tf_ring() {
    let mut ring = pp::LowDelayTfRing::default();
    // hierarchical_levels 3 -> mg_size 8; with 1 past picture only indices 6
    // and 7 qualify, and BASE pictures never do.
    for (poc, layer, idx) in [
        (10u64, 1u8, 5u32),
        (11, 1, 6),
        (12, 1, 7),
        (13, 0, 7),
        (14, 1, 0),
    ] {
        pp::low_delay_store_tf_picture(&mut ring, poc, layer, idx, 1, 3);
    }
    assert_eq!(
        ring.pics,
        vec![11, 12],
        "only the last two non-base pictures"
    );

    pp::low_delay_release_tf_pictures(&mut ring);
    assert!(ring.pics.is_empty(), "the drain resets the ring");
    // Draining an empty ring is a no-op, not a panic.
    pp::low_delay_release_tf_pictures(&mut ring);
    assert!(ring.pics.is_empty());
}

// ---------------------------------------------------------------------------
// `derive_tf_params` + `tf_controls` / `tf_ld_controls` (enc_handle.c:2525-3355)
// ---------------------------------------------------------------------------

/// LOW_DELAY forces `tf_level = 0` through `tf_ld_controls(0)` BEFORE any
/// preset logic — the whole table is disabled no matter what `enable_tf`,
/// the preset, or the hierarchy say.
#[test]
pub(super) fn traced_derive_tf_params_low_delay_always_off() {
    for preset in [-1i8, 0, 6, 13] {
        for hier in [0u8, 1, 5] {
            let (level, t) =
                pp::derive_tf_params(pp::PredStructure::LowDelay, preset, hier, true, false);
            assert_eq!(level, 0, "preset {preset} hier {hier}");
            assert!(
                t.iter().all(|e| !e.enabled),
                "LD table is disabled at preset {preset} hier {hier}"
            );
        }
    }
}

/// The RA ladder: `do_tf` gates on `enable_tf && hier >= 1 && !lossless`,
/// then `enc_mode <= M1 -> 1`, `<= M2 -> 2`, `<= M7 -> 5`, else 9.
#[test]
pub(super) fn traced_derive_tf_params_ra_ladder() {
    use pp::PredStructure::RandomAccess as RA;
    // The level boundaries, all at hier 4 with TF on.
    for (enc_mode, want) in [
        (-1i8, 1u8),
        (0, 1),
        (1, 1),
        (2, 2),
        (3, 5),
        (7, 5),
        (8, 9),
        (13, 9),
    ] {
        let (level, _) = pp::derive_tf_params(RA, enc_mode, 4, true, false);
        assert_eq!(level, want, "enc_mode {enc_mode}");
    }
    // `do_tf == 0` collapses every level to 0 (and the disabled table).
    for (enable_tf, hier, lossless) in [(false, 4, false), (true, 0, false), (true, 4, true)] {
        let (level, t) = pp::derive_tf_params(RA, 6, hier, enable_tf, lossless);
        assert_eq!(level, 0);
        assert!(t.iter().all(|e| !e.enabled));
    }
}

/// `tf_level 5` (presets 3..=7) spot fields: the half-pel mode is 2 (the
/// diagonal-skip mode `check_position` reads), L1 modulates with table 2
/// where BASE uses 3, and `enable_8x8_pred` is off for all three.
#[test]
pub(super) fn traced_tf_controls_level5_fields() {
    let t = pp::tf_controls(4, 5);
    assert!(t.iter().all(|e| e.enabled));
    assert!(
        t.iter()
            .all(|e| e.half_pel_mode == 2 && e.quarter_pel_mode == 1)
    );
    assert!(
        t.iter()
            .all(|e| !e.enable_8x8_pred && e.eight_pel_mode == 0)
    );
    assert!(t.iter().all(|e| e.pred_error_32x32_th == 20 * 32 * 32));
    assert!(
        t.iter()
            .all(|e| e.use_2tap && e.subpel_early_exit_th == 1 && e.qp_opt)
    );
    assert_eq!(t[0].num_future_pics, 24);
    assert_eq!((t[1].modulate_pics, t[2].modulate_pics), (3, 2));
    // Hierarchy-dependent caps at hier 4: BASE min(16, 7)=7, L1 min(8, 1)=1.
    assert_eq!((t[1].max_num_past_pics, t[1].max_num_future_pics), (7, 7));
    assert_eq!((t[2].max_num_past_pics, t[2].max_num_future_pics), (1, 1));
}

/// The max-count caps move with `hierarchical_levels`: L1's cap is
/// `MIN((1 << hier) / 2, tf_max_ref_per_struct(hier, 2, dir))` — below hier 5
/// the struct cap is 1, at hier 5+ it is 2.
#[test]
pub(super) fn traced_tf_controls_l1_caps_follow_hierarchy() {
    for (hier, want) in [(1u8, 1u8), (2, 1), (4, 1), (5, 2), (6, 2)] {
        let t = pp::tf_controls(hier, 5);
        assert_eq!(t[2].max_num_past_pics, want, "hier {hier}");
        assert_eq!(t[2].max_num_future_pics, want, "hier {hier}");
    }
    // The I_SLICE future cap is MIN(1 << hier, 1 << hier) = the mini-GOP size.
    assert_eq!(pp::tf_controls(3, 5)[0].max_num_future_pics, 8);
}

/// `tf_ld_controls` is the only source of `use_zz_based_filter = 1`; level 0
/// — the only level `derive_tf_params` ever selects — disables everything.
#[test]
pub(super) fn traced_tf_ld_controls() {
    assert!(pp::tf_ld_controls(0).iter().all(|e| !e.enabled));
    let t = pp::tf_ld_controls(1);
    assert!(!t[0].enabled && t[1].enabled && !t[2].enabled);
    assert!(t[1].use_zz_based_filter && !t[1].enable_8x8_pred);
    assert_eq!(t[1].chroma_lvl, 1);
    assert_eq!(pp::tf_ld_controls(2)[1].chroma_lvl, 2);
    assert_eq!(pp::tf_ld_controls(2)[1].pred_error_32x32_th, u64::MAX);
    // And the RA table never produces the zz flag.
    for level in 0..=9u8 {
        assert!(
            pp::tf_controls(4, level)
                .iter()
                .all(|e| !e.use_zz_based_filter),
            "level {level}"
        );
    }
}

/// `copy_tf_params` under RANDOM_ACCESS: a key frame honours
/// `enable_tf_key`, a delayed intra takes entry 0, BASE takes 1, L1 takes 2,
/// the highest layer and overlays are disabled.
#[test]
pub(super) fn traced_copy_tf_params_ra_selection() {
    use pp::TfParamsChoice as C;
    let ra = pp::PredStructure::RandomAccess;
    // Delayed intra wins over the base-layer mapping (key + tl0 + delayed).
    assert_eq!(
        pp::copy_tf_params(ra, pp::SliceType::I, true, 0, 4, false, true, true),
        C::DelayedIntra
    );
    // ...but not when `enable_tf_key` is off — C checks the key flag FIRST.
    assert_eq!(
        pp::copy_tf_params(ra, pp::SliceType::I, true, 0, 4, false, false, true),
        C::Disabled
    );
    // tl0 inter -> BASE, tl1 -> L1, highest layer (tl == hier) -> off.
    assert_eq!(
        pp::copy_tf_params(ra, pp::SliceType::B, false, 0, 4, false, true, false),
        C::Base
    );
    assert_eq!(
        pp::copy_tf_params(ra, pp::SliceType::B, false, 1, 4, false, true, false),
        C::L1
    );
    assert_eq!(
        pp::copy_tf_params(ra, pp::SliceType::B, false, 4, 4, false, true, false),
        C::Disabled
    );
    assert_eq!(
        pp::copy_tf_params(ra, pp::SliceType::B, false, 0, 4, true, true, false),
        C::Disabled,
        "overlays are never filtered"
    );
    // Layers above 1 but below the top fall to Disabled too (no L2+ entry).
    assert_eq!(
        pp::copy_tf_params(ra, pp::SliceType::B, false, 2, 4, false, true, false),
        C::Disabled
    );
}

// ---------------------------------------------------------------------------
// MCTF-B: `derive_tf_window_params`' list half, noise carry, filt/unfilt diff
// ---------------------------------------------------------------------------

/// Build the candidate list `assemble_tf_window` searches. `hists` is one
/// boxed 16 KiB region-histogram per picture (kept off the stack);
/// `spec` is `(poc, hierarchical_levels, avg_luma)` per candidate.
pub(super) fn tf_window_cands<'a>(
    hists: &'a [std::boxed::Box<pp::RegionHistograms>],
    spec: &[(u64, u8, u64)],
) -> Vec<pp::TfWindowCand<'a>> {
    spec.iter()
        .enumerate()
        .map(|(i, &(poc, hier, avg_luma))| pp::TfWindowCand {
            picture_number: poc,
            frame_width: 64,
            frame_height: 64,
            hierarchical_levels: hier,
            avg_luma,
            picture_histogram: &hists[i],
        })
        .collect()
}

/// A uniform histogram — every bin the same count.
pub(super) fn tf_hist(bin: u32) -> Box<pp::RegionHistograms> {
    Box::new([[[bin; 256]; 4]; 4])
}

/// A histogram that differs from `tf_hist(base)` by `spread` counts
/// concentrated in `concentrated` bins of region (0,0) — the knob that
/// steers `calc_ahd`'s per-region active test.
pub(super) fn tf_hist_spread(
    base: u32,
    spread: u32,
    concentrated: usize,
) -> Box<pp::RegionHistograms> {
    let mut h = tf_hist(base);
    for b in 0..concentrated {
        h[0][0][b] += spread;
    }
    h
}

/// INTER arm (`pd_process.c:4004-4100`): past+centre from the mini-GOP
/// (poc-matched, dims-checked), future from the positional forward walk,
/// `calc_ahd` stamped on every member including the centre (0 vs itself).
#[test]
pub(super) fn traced_assemble_tf_window_ra_inter() {
    let hists: Vec<Box<pp::RegionHistograms>> = (0..8).map(|_| tf_hist(10)).collect();
    let spec: Vec<(u64, u8, u64)> = (0..8u64).map(|p| (p, 2, 100 + p)).collect();
    let cands = tf_window_cands(&hists, &spec);
    let counts = pp::TfWindowCounts {
        num_past_pics: 2,
        num_future_pics: 2,
    };
    // centre = poc 3; the whole buffer is one mini-GOP.
    let w = pp::assemble_tf_window(
        pp::TfWindowArm::RandomAccessInter,
        &counts,
        3,
        &cands,
        0,
        8,
        &[],
        3,
        4,
        4,
    );
    assert_eq!(w.past_altref_nframes, 2);
    assert_eq!(w.future_altref_nframes, 2);
    assert_eq!(w.members.len(), 5);
    // Slots 0,1 = past (pocs 1,2), slot 2 = centre (poc 3), 3,4 = future.
    let pocs: Vec<u64> = w
        .members
        .iter()
        .map(|m| cands[m.unwrap().index].picture_number)
        .collect();
    assert_eq!(pocs, vec![1, 2, 3, 4, 5]);
    // ahd stamped everywhere; centre-vs-centre is 0.
    assert_eq!(w.members[2].unwrap().ahd_error_to_central, 0);
    assert_eq!(w.members[2].unwrap().index, 3);
    // avg_luma over the four NON-centre members: (101+102+104+105)/4 = 103.
    assert_eq!(w.tf_avg_luma, 103);
    assert_eq!(w.tf_avg_ahd_error, 0);
}

/// The `avail_past_pictures` bound truncates the past side at a mini-GOP
/// start — `num_past_pics` is the REQUESTED count and shrinks only via the
/// avail bound.
#[test]
pub(super) fn traced_assemble_tf_window_ra_inter_avail_past_bound() {
    let hists: Vec<Box<pp::RegionHistograms>> = (0..8).map(|_| tf_hist(10)).collect();
    let spec: Vec<(u64, u8, u64)> = (0..8u64).map(|p| (p, 2, 50)).collect();
    let cands = tf_window_cands(&hists, &spec);
    let counts = pp::TfWindowCounts {
        num_past_pics: 3,
        num_future_pics: 1,
    };
    // centre = poc 1; only ONE past picture exists in the mini-GOP.
    let w = pp::assemble_tf_window(
        pp::TfWindowArm::RandomAccessInter,
        &counts,
        1,
        &cands,
        0,
        8,
        &[],
        1,
        4,
        4,
    );
    assert_eq!(w.past_altref_nframes, 1, "avail_past clamps num_past");
    let pocs: Vec<u64> = w
        .members
        .iter()
        .map(|m| cands[m.unwrap().index].picture_number)
        .collect();
    assert_eq!(pocs, vec![0, 1, 2]);
}

/// A resolution change in the future walk breaks the search — the member
/// after the mismatched one is never reached (`pd_process.c:4056-4060`).
#[test]
pub(super) fn traced_assemble_tf_window_resolution_change_breaks() {
    let hists: Vec<Box<pp::RegionHistograms>> = (0..8).map(|_| tf_hist(10)).collect();
    let spec: Vec<(u64, u8, u64)> = (0..8u64).map(|p| (p, 2, 50)).collect();
    let mut cands = tf_window_cands(&hists, &spec);
    cands[4].frame_width = 128; // resolution change at centre+1
    let counts = pp::TfWindowCounts {
        num_past_pics: 1,
        num_future_pics: 4,
    };
    let w = pp::assemble_tf_window(
        pp::TfWindowArm::RandomAccessInter,
        &counts,
        3,
        &cands,
        0,
        8,
        &[],
        3,
        4,
        4,
    );
    assert_eq!(w.future_altref_nframes, 0);
    assert_eq!(w.members.len(), 2); // past + centre only
}

/// DELAYED-INTRA arm (`pd_process.c:3922-3965`): the centre sits OUTSIDE
/// the searched mini-GOP (it is `prev_delayed_intra`), lands at slot 0,
/// and its future members come from a poc-matched search of the FOLLOWING
/// mini-GOP — never the queue.
#[test]
pub(super) fn traced_assemble_tf_window_delayed_intra() {
    let hists: Vec<Box<pp::RegionHistograms>> = (0..9).map(|_| tf_hist(10)).collect();
    // cand 0 = the held intra (poc 0, hier 4); cands 1..8 = the next MG.
    let mut spec: Vec<(u64, u8, u64)> = vec![(0, 4, 90)];
    spec.extend((1..9u64).map(|p| (p, 3, 100 + p)));
    let cands = tf_window_cands(&hists, &spec);
    let counts = pp::TfWindowCounts {
        num_past_pics: 0,
        num_future_pics: 4,
    };
    let w = pp::assemble_tf_window(
        pp::TfWindowArm::DelayedIntra,
        &counts,
        0,
        &cands,
        1,
        9,
        &[],
        0,
        4,
        4,
    );
    assert_eq!(w.past_altref_nframes, 0);
    assert_eq!(w.future_altref_nframes, 4);
    let pocs: Vec<u64> = w
        .members
        .iter()
        .map(|m| cands[m.unwrap().index].picture_number)
        .collect();
    assert_eq!(pocs, vec![0, 1, 2, 3, 4]);
    // The pred-structure fixup: poc+1's level (3) differs from the
    // centre's (4) -> stamp 3 on the centre.
    assert_eq!(w.hier_fixup, Some(3));
}

/// IDR arm (`pd_process.c:3966-4002`): centre at slot 0, future members
/// from the positional queue walk — the poc-matched mini-GOP search does
/// NOT run here.
#[test]
pub(super) fn traced_assemble_tf_window_ra_idr() {
    let hists: Vec<Box<pp::RegionHistograms>> = (0..8).map(|_| tf_hist(10)).collect();
    let spec: Vec<(u64, u8, u64)> = (0..8u64).map(|p| (p, 2, 100 + p)).collect();
    let cands = tf_window_cands(&hists, &spec);
    let counts = pp::TfWindowCounts {
        num_past_pics: 0,
        num_future_pics: 3,
    };
    let w = pp::assemble_tf_window(
        pp::TfWindowArm::RandomAccessIdr,
        &counts,
        0,
        &cands,
        0,
        8,
        &[],
        0,
        4,
        4,
    );
    assert_eq!((w.past_altref_nframes, w.future_altref_nframes), (0, 3));
    let pocs: Vec<u64> = w
        .members
        .iter()
        .map(|m| cands[m.unwrap().index].picture_number)
        .collect();
    assert_eq!(pocs, vec![0, 1, 2, 3]);
    assert_eq!(w.members[0].unwrap().ahd_error_to_central, 0);
}

/// LOW-DELAY arm (`pd_process.c:3851-3921`): past members come from
/// `tf_pic_array` — the LD ring — with NO dims check and NO `calc_ahd`;
/// a poc the ring lacks leaves the slot empty while
/// `past_altref_nframes` still counts it (C's dead-compaction behaviour).
#[test]
pub(super) fn traced_assemble_tf_window_low_delay() {
    // Window: centre poc 5 + futures 6,7,8,9.
    let hists: Vec<Box<pp::RegionHistograms>> = (0..5).map(|_| tf_hist(10)).collect();
    let spec: Vec<(u64, u8, u64)> = (5..10u64).map(|p| (p, 2, 70)).collect();
    let cands = tf_window_cands(&hists, &spec);
    // The LD ring holds pocs 3,4 — poc 2 is missing (sparse slot).
    let ld_hists: Vec<Box<pp::RegionHistograms>> = (0..2).map(|_| tf_hist(20)).collect();
    let ld = tf_window_cands(&ld_hists, &[(3, 2, 60), (4, 2, 61)]);
    let counts = pp::TfWindowCounts {
        num_past_pics: 3,
        num_future_pics: 2,
    };
    let w = pp::assemble_tf_window(
        pp::TfWindowArm::LowDelay,
        &counts,
        0,
        &cands,
        0,
        5,
        &ld,
        0,
        4,
        4,
    );
    // Requested 3 past: slots 0..3 — poc 2 unfilled, 3 and 4 found.
    assert_eq!(w.past_altref_nframes, 3);
    assert!(w.members[0].is_none(), "poc 2 absent from the ring");
    assert_eq!(w.members[1].unwrap().pool, pp::TfMemberPool::LdRing);
    assert_eq!(w.members[1].unwrap().index, 0);
    assert_eq!(w.members[2].unwrap().index, 1);
    // The LD arm stamps no AHD.
    assert!(!w.members[1].unwrap().active_region_present);
    // Centre at slot 3, future pocs 6,7 at 4,5.
    assert_eq!(w.members[3].unwrap().index, 0);
    assert_eq!(w.members[3].unwrap().pool, pp::TfMemberPool::Window);
    assert_eq!(w.future_altref_nframes, 2);
    assert_eq!(w.members.len(), 6);
}

/// `calc_ahd`'s active-region stamp: `ahd_per_region > region_area` flips
/// `tf_active_region_present` — 64x64 with 4x4 regions means 256 pixels.
#[test]
pub(super) fn traced_assemble_tf_window_ahd_active_region_threshold() {
    // Centre histogram: bin counts 10 everywhere. Member: +300 in ONE bin
    // of region (0,0) — ahd_per_region = 300 > 256 -> active.
    let hists: Vec<Box<pp::RegionHistograms>> = vec![
        tf_hist(10),
        tf_hist_spread(10, 300, 1),
        tf_hist_spread(10, 100, 2), // ahd_per_region = 200 < 256 -> inactive
    ];
    let spec: Vec<(u64, u8, u64)> = vec![(0, 2, 0), (1, 2, 0), (2, 2, 0)];
    let cands = tf_window_cands(&hists, &spec);
    let counts = pp::TfWindowCounts {
        num_past_pics: 0,
        num_future_pics: 2,
    };
    let w = pp::assemble_tf_window(
        pp::TfWindowArm::RandomAccessIdr,
        &counts,
        0,
        &cands,
        0,
        3,
        &[],
        0,
        4,
        4,
    );
    assert_eq!(w.members[1].unwrap().ahd_error_to_central, 300);
    assert!(w.members[1].unwrap().active_region_present);
    assert_eq!(w.members[2].unwrap().ahd_error_to_central, 200);
    assert!(!w.members[2].unwrap().active_region_present);
}

/// `tf_window_noise` — the `do_noise_est` selection and the last-I carry
/// (`pd_process.c:3755-3849`).
#[test]
pub(super) fn traced_tf_window_noise_carry() {
    let mut last_i = [0i32; 3];
    // use_intra off -> always estimate.
    let n = pp::tf_window_noise(false, false, false, Some(7000), [0, 0], &mut last_i);
    assert_eq!(n.levels_log1p_fp16[0], 7000);
    assert_eq!(last_i[0], 7000, "fresh estimate publishes to last_i");
    assert!(n.estimated_y);
    // use_intra on, not an I slice -> reuse the carry.
    let n = pp::tf_window_noise(true, false, false, None, [0, 0], &mut last_i);
    assert_eq!(n.levels_log1p_fp16[0], 7000);
    assert!(!n.estimated_y);
    // An I slice ALWAYS estimates, even under use_intra.
    let n = pp::tf_window_noise(true, true, false, Some(9000), [0, 0], &mut last_i);
    assert_eq!(n.levels_log1p_fp16[0], 9000);
    assert_eq!(last_i[0], 9000);
    // chroma_lvl stamps U/V unconditionally of do_noise_est.
    let n = pp::tf_window_noise(true, false, true, None, [111, 222], &mut last_i);
    assert_eq!(n.levels_log1p_fp16, [9000, 111, 222]);
    // chroma_lvl off leaves C's zero-initialised slots.
    let n = pp::tf_window_noise(true, false, false, None, [111, 222], &mut last_i);
    assert_eq!(n.levels_log1p_fp16[1..], [0, 0]);
    // is_noise_level reads the value in force: 9000 < VQ_NOISE_LVL_TH(15000).
    assert!(!n.is_noise_level);
    last_i[0] = 20000;
    let n = pp::tf_window_noise(true, false, false, None, [0, 0], &mut last_i);
    assert!(n.is_noise_level);
}

/// `filt_to_unfilt_diff` carry (`pd_process.c:5122-5125`): every picture
/// inherits the context value; only an I slice publishes back.
#[test]
pub(super) fn traced_filt_to_unfilt_diff_carry() {
    let mut ctx = pp::PicDecisionCtx::new();
    assert_eq!(ctx.filt_to_unfilt_diff, u32::MAX, "~0 until the first I");
    let mut pic = pp::PicParams::default();
    pic.slice_type = pp::SliceType::B;
    pp::tf_inherit_filt_to_unfilt_diff(&ctx, &mut pic);
    assert_eq!(pic.filt_to_unfilt_diff, u32::MAX);
    // The filter measured a real difference on this pic (MCTF-C's write).
    pic.filt_to_unfilt_diff = 12345;
    pp::tf_publish_filt_to_unfilt_diff(&mut ctx, &pic);
    assert_eq!(
        ctx.filt_to_unfilt_diff,
        u32::MAX,
        "a B slice never publishes"
    );
    pic.slice_type = pp::SliceType::I;
    pp::tf_publish_filt_to_unfilt_diff(&mut ctx, &pic);
    assert_eq!(ctx.filt_to_unfilt_diff, 12345);
}

/// `ref_pics_modulation`'s ratio is computed in UINT32 in C
/// (`pd_process.c:3660`): the `~0` carried diff makes `* 100` wrap to
/// 0xFFFFFF9C — a huge POSITIVE ratio — where a signed multiply would
/// produce 0. This is the difference between the max modulation arm and
/// the min one on the first inter picture after an I slice.
#[test]
pub(super) fn traced_ref_pics_modulation_u32_ratio() {
    let ctrls = pp::TfCtrls {
        modulate_pics: 1,
        ..Default::default()
    };
    // tl0, modulate_pics 1: ratio < 100 -> 5, else TF_MAX_EXTENSION (6).
    assert_eq!(
        pp::ref_pics_modulation(false, 0, &ctrls, 20000, u32::MAX, 1, 1),
        6,
        "the ~0 carry must wrap to a huge u32 ratio"
    );
    // A small measured diff: (5000 * 100) / 20000 = 25 < 100 -> offset 5.
    assert_eq!(
        pp::ref_pics_modulation(false, 0, &ctrls, 20000, 5000, 1, 1),
        5
    );
    // qp_opt applies DIVIDE_AND_ROUND to the offset.
    let qp_ctrls = pp::TfCtrls {
        modulate_pics: 1,
        qp_opt: true,
        ..Default::default()
    };
    assert_eq!(
        pp::ref_pics_modulation(false, 0, &qp_ctrls, 20000, 5000, 1, 2),
        // (5000*100)/20000 = 25 -> offset 5 -> (5*1)/2 rounded = 3.
        3
    );
}
