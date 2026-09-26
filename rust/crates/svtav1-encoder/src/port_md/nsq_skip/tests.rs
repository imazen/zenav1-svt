use super::*;
use PredictionMode as M;

/// Tier 4 throughout: every expectation below is hand-derived from the
/// C source line named in its comment. These functions are `static` in
/// `product_coding_loop.c` and export no symbol.
fn sq(mode: PredictionMode) -> ParentSquare {
    ParentSquare {
        cost: 10_000,
        total_rate: 100,
        full_dist: 60,
        cnt_nz_coeff: 40,
        mode,
    }
}

fn nsq() -> NsqSearchCtrls {
    NsqSearchCtrls {
        sq_weight: Some(100),
        hv_weight: 100,
        max_part0_to_part1_dev: 10,
        ..NsqSearchCtrls::default()
    }
}

/// `:9868-9895`. The 75% arm has NO intra member, so an intra-only
/// table cannot produce it; the other two arms gained inter members.
#[test]
fn parent_mode_modulation_covers_the_inter_modes() {
    assert_eq!(modulate_by_parent_mode(100, M::NewMv), 75);
    assert_eq!(modulate_by_parent_mode(100, M::NewNewMv), 75);
    assert_eq!(modulate_by_parent_mode(100, M::NearestNearestMv), 200);
    assert_eq!(modulate_by_parent_mode(100, M::NearNearMv), 200);
    assert_eq!(modulate_by_parent_mode(100, M::DcPred), 200);
    assert_eq!(modulate_by_parent_mode(100, M::GlobalMv), 400);
    assert_eq!(modulate_by_parent_mode(100, M::GlobalGlobalMv), 400);
    assert_eq!(modulate_by_parent_mode(100, M::PaethPred), 400);
    // Everything else keeps the base value — NEARESTMV / NEARMV /
    // NEWMV's compound cousins are all `default:` in C.
    assert_eq!(modulate_by_parent_mode(100, M::NearestMv), 100);
    assert_eq!(modulate_by_parent_mode(100, M::NearestNewMv), 100);
}

/// `:9899-9904` / `:9935-9940`: H_PRED kills the H arm and V_PRED kills
/// the V arm, while the diagonals relax both.
///
/// The parent below is tuned so `dist_cost_ratio` lands in the MIDDLE
/// band (50 < ratio <= 100). That is not incidental: above `max_ratio`
/// the final assignment REPLACES the threshold with `dist_cost_ratio`
/// (`:9925-9929`), which resurrects a threshold the mode exception had
/// just zeroed — so a test run in the high band cannot observe the
/// exception at all. This was measured: the first version of this test
/// used a high-band parent and the "H_PRED kills the H arm" assertion
/// failed against a CORRECT port.
#[test]
fn the_h_and_v_arms_have_opposite_mode_exceptions() {
    // Perfectly uniform quadrants -> deviation 0, so the gate fires for
    // any positive threshold and not at all for a zero one.
    let uniform = [100u64; 4];
    let mut c = nsq();
    c.max_part0_to_part1_dev = 1000;
    // dist = 100 << 7 = 12800; ratio = 1_280_000 / 17_066 = 75.
    let mid_band = ParentSquare {
        full_dist: 100,
        cost: 17_066,
        ..sq(M::HPred)
    };
    assert!(!skip_by_sq_recon_dist(&c, Part::H, &mid_band, uniform, 1));
    assert!(skip_by_sq_recon_dist(&c, Part::V, &mid_band, uniform, 1));
    let v_parent = ParentSquare {
        mode: M::VPred,
        ..mid_band
    };
    assert!(skip_by_sq_recon_dist(&c, Part::H, &v_parent, uniform, 1));
    assert!(!skip_by_sq_recon_dist(&c, Part::V, &v_parent, uniform, 1));
}

/// The counterpart of the note above, stated as its own case: in the
/// HIGH band the mode exception is overwritten and BOTH arms fire.
#[test]
fn above_max_ratio_the_mode_exception_is_overwritten() {
    let uniform = [100u64; 4];
    let mut c = nsq();
    c.max_part0_to_part1_dev = 1000;
    let high_band = ParentSquare {
        full_dist: 10_000,
        cost: 100,
        ..sq(M::HPred)
    };
    assert!(skip_by_sq_recon_dist(&c, Part::H, &high_band, uniform, 1));
    assert!(skip_by_sq_recon_dist(&c, Part::V, &high_band, uniform, 1));
}

/// The AB shapes take the same arms as their plain rects (`:9897`,
/// `:9933`) — the intra port's `H | H4` match would drop them.
#[test]
fn the_ab_shapes_take_the_rect_arms() {
    assert!(Part::Ha.is_horizontal() && Part::Hb.is_horizontal());
    assert!(Part::Va.is_vertical() && Part::Vb.is_vertical());
    assert!(!Part::Ha.is_vertical() && !Part::Va.is_horizontal());
    let uniform = [100u64; 4];
    let mut c = nsq();
    c.max_part0_to_part1_dev = 1000;
    let p = ParentSquare {
        full_dist: 10_000,
        cost: 100,
        ..sq(M::NewMv)
    };
    for shape in [Part::H, Part::Ha, Part::Hb, Part::H4] {
        assert!(
            skip_by_sq_recon_dist(&c, shape, &p, uniform, 1),
            "{shape:?}"
        );
    }
}

/// `:9925-9929`: above `max_ratio` the threshold is REPLACED by
/// `dist_cost_ratio`, not clamped to the modulated value.
#[test]
fn above_max_ratio_the_threshold_becomes_the_ratio_itself() {
    let mut c = nsq();
    c.max_part0_to_part1_dev = 1; // base 1, so a clamp would keep ~1
    // dist_cost_ratio = rdcost(1, 0, 10_000) * 100 / 100 = 1_280_000
    let p = ParentSquare {
        full_dist: 10_000,
        cost: 100,
        ..sq(M::NearestMv)
    };
    // Quadrants deviating by 300% — far above any modulated version of
    // a base of 1, but far below the ratio.
    let skewed = [1u64, 4, 1, 4];
    assert!(skip_by_sq_recon_dist(&c, Part::H, &p, skewed, 1));
}

/// `:9852` — a zero `max_part0_to_part1_dev` disables the gate entirely.
#[test]
fn a_zero_deviation_threshold_disables_the_gate() {
    let mut c = nsq();
    c.max_part0_to_part1_dev = 0;
    let p = ParentSquare {
        full_dist: 10_000,
        cost: 100,
        ..sq(M::NearestMv)
    };
    assert!(!skip_by_sq_recon_dist(&c, Part::H, &p, [100; 4], 1));
}

/// `:10002-10009`: HA reads half 0's coefficients, HB reads half 1's.
#[test]
fn the_aggressive_offset_reads_a_different_half_for_ha_and_hb() {
    let c = NsqSearchCtrls {
        sq_weight: Some(100),
        hv_weight: u32::MAX / 4,
        ..NsqSearchCtrls::default()
    };
    // H costs 1000 total vs a square of 1053: 1000 > 1053*100/100 is
    // false, but 1000 > 1053*90/100 (947) is true.
    let sq_cost = 1053u64;
    let h_first_empty = RectHalves {
        costs: [500, 500],
        has_coeff: [false, true],
    };
    let h_second_empty = RectHalves {
        costs: [500, 500],
        has_coeff: [true, false],
    };
    assert!(skip_by_shapes(
        &c,
        Part::Ha,
        Some(sq_cost),
        Some(h_first_empty),
        None
    ));
    assert!(!skip_by_shapes(
        &c,
        Part::Ha,
        Some(sq_cost),
        Some(h_second_empty),
        None
    ));
    assert!(!skip_by_shapes(
        &c,
        Part::Hb,
        Some(sq_cost),
        Some(h_first_empty),
        None
    ));
    assert!(skip_by_shapes(
        &c,
        Part::Hb,
        Some(sq_cost),
        Some(h_second_empty),
        None
    ));
    // H4 takes the CONSERVATIVE offset instead, which makes skipping
    // LESS likely, so neither coefficient pattern fires.
    assert!(!skip_by_shapes(
        &c,
        Part::H4,
        Some(sq_cost),
        Some(h_first_empty),
        None
    ));
}

/// `:9987` — the disabled sentinel returns immediately.
#[test]
fn a_disabled_sq_weight_never_skips() {
    let c = NsqSearchCtrls {
        sq_weight: None,
        ..NsqSearchCtrls::default()
    };
    let halves = RectHalves {
        costs: [u64::MAX / 8, u64::MAX / 8],
        has_coeff: [false, false],
    };
    assert!(!skip_by_shapes(&c, Part::Ha, Some(1), Some(halves), None));
}

/// `:10072` — the `~0` pair means "not measured", not "zero".
#[test]
fn unmeasured_tx_split_counts_never_skip() {
    let c = NsqPsqTxsCtrls {
        enabled: true,
        hv_to_sq_th: 1000,
        h_to_v_th: 100,
    };
    assert!(!skip_by_sq_txs(&c, Part::H, &sq(M::NewMv), None));
    // 0 nonzero coefficients in both directions IS measured, and both
    // doubled counts (0) are >= the square's threshold only when the
    // square also coded nothing.
    let zero_sq = ParentSquare {
        cnt_nz_coeff: 0,
        ..sq(M::NewMv)
    };
    assert!(skip_by_sq_txs(&c, Part::H, &zero_sq, Some((0, 0))));
}

/// `:10085-10095`: the H test needs `cnt_v <= cnt_h` and the V test the
/// reverse, so a shape is never skipped for being the BETTER direction.
#[test]
fn the_direction_test_favours_the_cheaper_split() {
    let c = NsqPsqTxsCtrls {
        enabled: true,
        hv_to_sq_th: 1000,
        h_to_v_th: 100,
    };
    // cnt_nz 40 -> hv_to_sq 400, h_to_v 40. h=30 -> 60, v=5 -> 10.
    let s = sq(M::NewMv);
    assert!(skip_by_sq_txs(&c, Part::Ha, &s, Some((30, 5))), "H worse");
    assert!(!skip_by_sq_txs(&c, Part::Va, &s, Some((30, 5))), "V better");
}

/// `:10375-10395`. Uniform quadrants give a zero standard deviation;
/// skewed ones do not.
#[test]
fn sub_depth_skip_needs_uniform_quadrants_and_few_coeffs() {
    let c = SkipSubDepthCtrls {
        quad_deviation_th: 250.0,
        coeff_perc: 50,
    };
    // 64x64 = 4096 samples; 40 coeffs -> 0%.
    assert!(eval_sub_depth_skip_cond1(&c, [1000; 4], 40, 64));
    // Wildly skewed quadrants: std dev far above 250.
    assert!(!eval_sub_depth_skip_cond1(&c, [0, 0, 0, 4000], 40, 64));
    // Uniform but coefficient-dense: 3000 * 100 / 4096 = 73 >= 50.
    assert!(!eval_sub_depth_skip_cond1(&c, [1000; 4], 3000, 64));
}

/// `:10404-10411` — global-MV injection is dropped unless the parent
/// square itself picked a global mode.
#[test]
fn global_mv_injection_follows_the_parent_square() {
    let base = NsqSearchCtrls::default();
    let off = faster_md_settings_nsq(true, Some(M::NewMv), false, false, base);
    assert!(!off.global_mv_injection && off.params_status);
    let on = faster_md_settings_nsq(true, Some(M::GlobalMv), false, false, base);
    assert!(on.global_mv_injection && !on.params_status);
    // Disabled GM leaves it alone even under a non-global parent.
    let gm_off = faster_md_settings_nsq(false, Some(M::NewMv), false, false, base);
    assert!(gm_off.global_mv_injection && !gm_off.params_status);
}

/// `:10415-10421` — two thresholds are capped and two are floored, in
/// opposite directions.
#[test]
fn sub_depth_tightening_caps_two_and_floors_two() {
    let base = NsqSearchCtrls {
        sub_depth_block_lvl: 1,
        sq_weight: Some(120),
        nsq_split_cost_th: 90,
        h_vs_v_split_rate_th: 30,
        non_hv_split_rate_th: 90,
        ..NsqSearchCtrls::default()
    };
    let got = faster_md_settings_nsq(false, None, true, true, base).ctrls;
    assert_eq!(got.sq_weight, Some(85), "capped");
    assert_eq!(got.nsq_split_cost_th, 60, "capped");
    assert_eq!(got.h_vs_v_split_rate_th, 60, "floored");
    assert_eq!(got.non_hv_split_rate_th, 90, "already above the floor");
    // Only on a PD1 CHILD.
    let untouched = faster_md_settings_nsq(false, None, true, false, base).ctrls;
    assert_eq!(untouched.sq_weight, Some(120));
}

/// `:9732` reduces the first threshold at `sq_size <= 16` while `:9752`
/// / `:9786` / `:9816` increase theirs.
#[test]
fn the_small_block_offset_goes_the_other_way_for_the_first_gate() {
    let c = NsqSearchCtrls {
        nsq_split_cost_th: 100,
        rate_th_offset_lte16: 40,
        ..NsqSearchCtrls::default()
    };
    let s = sq(M::NewMv);
    // part_cost = rdcost(1, 20, 0) = (20 + 256) >> 9 = 0 -> never fires.
    // Use a rate big enough to matter: rdcost(512, R, 0) = R.
    let rate = |_: Part| 1_000u64;
    // 32x32: th 100. 1000 * 1000 > 10_000 * 100 -> 1e6 > 1e6 is false.
    assert!(!skip_by_split_rate(
        &c,
        Part::H,
        &s,
        32,
        Part::N,
        false,
        512,
        rate
    ));
    // 16x16: th 100 - 40 = 60. 1e6 > 600_000 -> true.
    assert!(skip_by_split_rate(
        &c,
        Part::H,
        &s,
        16,
        Part::N,
        false,
        512,
        rate
    ));
}

/// `:10354-10367` — the reasons come back in C's evaluation order.
#[test]
fn the_composer_reports_the_first_gate_that_fires() {
    let ctrls = NsqSearchCtrls {
        nsq_split_cost_th: 1,
        rate_th_offset_lte16: 0,
        ..NsqSearchCtrls::default()
    };
    let txs = NsqPsqTxsCtrls::default();
    let s = sq(M::NewMv);
    let got = skip_processing_nsq_block(
        &ctrls,
        &txs,
        Part::H,
        &s,
        32,
        Part::N,
        false,
        512,
        [100; 4],
        None,
        None,
        None,
        |_| 1_000_000,
    );
    assert_eq!(got, Some(NsqSkipReason::SplitRate));
    // Nothing enabled -> nothing fires.
    let none = skip_processing_nsq_block(
        &NsqSearchCtrls::default(),
        &txs,
        Part::H,
        &s,
        32,
        Part::N,
        false,
        512,
        [100; 4],
        None,
        None,
        None,
        |_| 1_000_000,
    );
    assert_eq!(none, None);
}

/// The square itself is never skipped by any gate (`:9717`, `:9852`,
/// `:10067`).
#[test]
fn the_square_shape_is_never_skipped() {
    let c = NsqSearchCtrls {
        nsq_split_cost_th: 1,
        max_part0_to_part1_dev: 1000,
        sq_weight: Some(1),
        ..NsqSearchCtrls::default()
    };
    let txs = NsqPsqTxsCtrls {
        enabled: true,
        hv_to_sq_th: 1,
        h_to_v_th: 1,
    };
    let s = sq(M::NewMv);
    assert!(!skip_by_split_rate(
        &c,
        Part::N,
        &s,
        16,
        Part::N,
        true,
        512,
        |_| 1 << 40
    ));
    assert!(!skip_by_sq_recon_dist(&c, Part::N, &s, [1, 9, 1, 9], 512));
    assert!(!skip_by_sq_txs(&txs, Part::N, &s, Some((0, 0))));
    assert!(!skip_by_shapes(&c, Part::N, Some(1), None, None));
}
