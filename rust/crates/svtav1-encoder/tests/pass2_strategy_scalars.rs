//! `Codec/pass2_strategy.c`'s scalar core
//! (`svtav1-encoder/src/port_pass2_strategy.rs`).
//!
//! **EVIDENCE TIER 4** (`docs/WORKING-ON-THIS.md` §4, the weakest tier). Every
//! function here is `static` in `pass2_strategy.c`, and `q_pow_term` is
//! `static const` in the .c file rather than a header — so unlike the SAD
//! lambda and minq tables there is no way for a shim to reach it at all.
//! `nm -g` on `Bin/Release/libSvtAv1Enc.a` exports from that file only
//! `svt_aom_process_rc_stat`, `svt_aom_set_rc_param`, `svt_av1_init_second_pass`,
//! `svt_av1_init_single_pass_lap`, `svt_av1_new_framerate` and
//! `svt_av1_twopass_postencode_update{,_gop_const}` — the two that are in this
//! lane's scope (`svt_aom_set_rc_param`, `svt_av1_new_framerate`) are ported
//! and pinned at TIER 1 in `c_parity_rc_process.rs`, not here.
//!
//! One lever raises the floor: `find_qindex_by_rate_with_correction`'s q
//! ladder is `svt_av1_convert_qindex_to_q`, which IS exported and IS pinned at
//! tier 1 in `c_parity_rc_process.rs`, so the ladder under this search is not
//! a transcription.
//!
//! `pow` is a libm call, so `calc_correction_factor`'s exact value is
//! host-dependent (`WORKING-ON-THIS.md` §5c). The tests below therefore pin
//! the values that are EXACT in any IEEE-754 libm (`pow(1, p) == 1`,
//! `pow(0, p) == 0`) plus the structural relations the interpolation must
//! satisfy — never a decimal that would make this a cross-host tripwire.

use svtav1_encoder::port_pass2_strategy as p2;

// ---------------------------------------------------------------------------
// fclamp / frame_max_bits / qbpm_enumerator
// ---------------------------------------------------------------------------

#[test]
fn fclamp_matches_the_c_body() {
    // C: `value < low ? low : (value > high ? high : value)`.
    assert_eq!(p2::fclamp(-1.0, 0.0, 1.0), 0.0);
    assert_eq!(p2::fclamp(0.5, 0.0, 1.0), 0.5);
    assert_eq!(p2::fclamp(2.0, 0.0, 1.0), 1.0);
    assert_eq!(p2::fclamp(0.0, 0.0, 1.0), 0.0);
    assert_eq!(p2::fclamp(1.0, 0.0, 1.0), 1.0);
}

#[test]
fn frame_max_bits_clamps_before_the_narrow_to_int() {
    // C: `(int64_t)avg * vbrmax / 100`, then CLIP3(0, max_frame_bandwidth, _).
    // 1000 * 200 / 100 == 2000, clamped down to the 1500 bandwidth.
    assert_eq!(p2::frame_max_bits(1000, 1500, 200), 1500);
    // Under the cap it passes through: 1000 * 100 / 100 == 1000.
    assert_eq!(p2::frame_max_bits(1000, 1500, 100), 1000);
    // vbrmax 0 -> 0.
    assert_eq!(p2::frame_max_bits(1000, 1500, 0), 0);
    // A negative product is clamped up to 0 by CLIP3's lower bound.
    assert_eq!(p2::frame_max_bits(-1000, 1500, 100), 0);
    // The i64 intermediate is what stops the narrow from wrapping: this
    // product is 2^31 * 100 / 100, far past INT_MAX, and the clamp catches it.
    assert_eq!(p2::frame_max_bits(i32::MAX, 1000, 100), 1000);
}

#[test]
fn qbpm_enumerator_clamps_tol_minus_25_not_tol() {
    // C: `1250000 + ((300000 * AOMMIN(75, AOMMAX(rate_err_tol - 25, 0))) / 75)`.
    // Flat at the base for every tolerance up to 25, because the max is on
    // `tol - 25`.
    for tol in [-100i32, -1, 0, 1, 24, 25] {
        assert_eq!(p2::qbpm_enumerator(tol), 1_250_000, "tol={tol}");
    }
    // 26 -> (300000 * 1) / 75 == 4000.
    assert_eq!(p2::qbpm_enumerator(26), 1_254_000);
    // 100 -> (300000 * 75) / 75 == 300000, the saturation point.
    assert_eq!(p2::qbpm_enumerator(100), 1_550_000);
    for tol in [100i32, 101, 1000] {
        assert_eq!(p2::qbpm_enumerator(tol), 1_550_000, "tol={tol}");
    }
}

// ---------------------------------------------------------------------------
// calc_correction_factor + q_pow_term
// ---------------------------------------------------------------------------

#[test]
fn q_pow_term_has_the_extra_upper_endpoint() {
    // The table is sized `(QINDEX_RANGE >> 5) + 1` == 9, and
    // calc_correction_factor reads `[q >> 5]` AND `[(q >> 5) + 1]`. At q = 255
    // that is index 8, so a table of 8 would read out of bounds. Exercising
    // the top of the ladder is the check.
    assert_eq!(p2::Q_POW_TERM.len(), 9);
    let _ = p2::calc_correction_factor(96.0, 255);
    assert_eq!(p2::Q_POW_TERM[7], p2::Q_POW_TERM[8]);
}

#[test]
fn correction_factor_is_one_at_the_err_divisor_for_every_q() {
    // error_term == 1 -> pow(1, anything) == 1, exactly, in any IEEE-754 libm.
    // So this pins ERR_DIVISOR == 96 without depending on pow's precision.
    for q in 0..=255i32 {
        assert_eq!(p2::calc_correction_factor(p2::ERR_DIVISOR, q), 1.0, "q={q}");
    }
    assert_eq!(p2::ERR_DIVISOR, 96.0);
}

#[test]
fn correction_factor_clamps_to_005_and_5() {
    // pow(0, p) == 0 for p > 0, clamped up to 0.05.
    assert_eq!(p2::calc_correction_factor(0.0, 128), 0.05);
    // A huge error term overshoots 5.0 and is clamped down.
    assert_eq!(p2::calc_correction_factor(1.0e12, 128), 5.0);
}

#[test]
fn correction_factor_power_term_is_monotone_in_q_above_the_divisor() {
    // Q_POW_TERM is non-decreasing, so for error_term > 1 a larger q must give
    // a value that never decreases; for error_term < 1 it must never increase.
    // This catches an inverted interpolation without pinning any libm digit.
    let above: Vec<f64> = (0..=255)
        .map(|q| p2::calc_correction_factor(200.0, q))
        .collect();
    for w in above.windows(2) {
        assert!(w[0] <= w[1], "not monotone up: {} then {}", w[0], w[1]);
    }
    let below: Vec<f64> = (0..=255)
        .map(|q| p2::calc_correction_factor(50.0, q))
        .collect();
    for w in below.windows(2) {
        assert!(w[0] >= w[1], "not monotone down: {} then {}", w[0], w[1]);
    }
    // Non-vacuous: the two ends must actually differ.
    assert!(
        above[0] < above[255],
        "the sweep is flat, so it proves nothing"
    );
}

#[test]
fn correction_factor_is_flat_where_q_pow_term_repeats() {
    // Q_POW_TERM[6..=8] are all 0.95, so every q from 192 (index 6, offset 0)
    // through 255 interpolates between equal endpoints and gives one value.
    let at_192 = p2::calc_correction_factor(200.0, 192);
    for q in 192..=255i32 {
        assert_eq!(
            p2::calc_correction_factor(200.0, q),
            at_192,
            "q={q} should be flat across the repeated 0.95 tail"
        );
    }
    // And q = 191 must be STRICTLY below it: index 5, offset 31, so the weight
    // is 31/32 — the interpolation never reaches the upper endpoint.
    assert!(
        p2::calc_correction_factor(200.0, 191) < at_192,
        "the q % 32 weight reaches 1.0, so the 31/32 reading is wrong"
    );
}

// ---------------------------------------------------------------------------
// find_qindex_by_rate_with_correction / get_twopass_worst_quality
// ---------------------------------------------------------------------------

#[test]
fn qindex_by_rate_with_correction_is_monotone_and_honours_its_bounds() {
    let f = |bits| p2::find_qindex_by_rate_with_correction(bits, 8, 96.0, 1.0, 50, 0, 255);
    // More bits allowed -> a lower (better) qindex, same direction as
    // find_qindex_by_rate.
    let a = f(100);
    let b = f(100_000);
    assert!(
        b < a,
        "expected a lower qindex at a higher rate ({b} vs {a})"
    );
    // Unsatisfiably high demand walks to best_qindex; unsatisfiably low walks
    // to worst_qindex.
    assert_eq!(f(i32::MAX), 0);
    assert_eq!(f(0), 255);
    // Bounds are honoured.
    assert_eq!(
        p2::find_qindex_by_rate_with_correction(0, 8, 96.0, 1.0, 50, 40, 60),
        60
    );
    assert_eq!(
        p2::find_qindex_by_rate_with_correction(i32::MAX, 8, 96.0, 1.0, 40, 40, 60),
        40
    );
}

/// HONEST LIMIT, stated rather than papered over: this pins the RESULT of C's
/// `section_target_bandwidth <= 0` early-out, not the early-out itself. At
/// exactly 0 the fall-through would ask the search for 0 bits/mb and the
/// search saturates to `worst_quality` as well, so no return value can
/// separate `<= 0` from `< 0`. A mutation from `<=` to `<` therefore does NOT
/// turn this test red — measured, not assumed. What the test does cover is
/// that a non-positive target yields `worst_quality` and nothing else.
#[test]
fn twopass_worst_quality_returns_worst_on_a_non_positive_target() {
    for target in [-1i32, 0] {
        assert_eq!(
            p2::get_twopass_worst_quality(
                false, 1920, 1080, 8, 1.0e9, 0.0, target, 1.0, 50, 50, 0, 217
            ),
            217,
            "target={target}"
        );
    }
}

#[test]
fn twopass_worst_quality_inactive_zone_is_clamped_to_0_1() {
    // fclamp(inactive_zone, 0.0, 1.0) — an out-of-range zone must give the
    // same answer as the clamped one, not a wilder active_mbs.
    let call = |zone| {
        p2::get_twopass_worst_quality(
            false, 1920, 1080, 8, 1.0e8, zone, 500_000, 1.0, 50, 50, 0, 255,
        )
    };
    assert_eq!(call(-5.0), call(0.0));
    assert_eq!(call(5.0), call(1.0));
    // Non-vacuous: 0.0 and 1.0 must actually differ, or the clamp test is
    // comparing two identical things.
    assert_ne!(call(0.0), call(1.0));
}

// ---------------------------------------------------------------------------
// The MB grid — TWO different formulas in the same C file.
// ---------------------------------------------------------------------------

#[test]
fn twopass_mb_grid_doubles_the_numerator_unlike_set_rc_param() {
    use svtav1_encoder::port_rc_process::{SetRcParamInput, set_rc_param};
    // pass2_strategy.c:126 is `2 * (w + 16 - 1) / 16` — left-associative, so
    // the DOUBLING happens inside the division.
    // pass2_strategy.c:914 (svt_aom_set_rc_param) is `((w + 16 - 1) / 16) << 1`
    // — ceil-divide FIRST, then double.
    // At w = 25 those differ: (2*40)/16 == 5 vs (40/16) << 1 == 4.
    let (cols, _) = p2::twopass_mb_grid(true, 25, 25);
    assert_eq!(cols, 5, "twopass grid must double the numerator");
    let set = set_rc_param(&SetRcParamInput {
        first_pass_downsample: true,
        max_input_luma_width: 25,
        max_input_luma_height: 25,
        ..Default::default()
    });
    assert_eq!(set.mb_cols, 4, "set_rc_param must ceil-divide then double");
    assert_ne!(
        cols as i32, set.mb_cols,
        "the two C formulas must disagree here, or this test proves nothing"
    );
    // Off the downsample arm they agree, and both are a plain ceil-divide.
    let (cols2, rows2) = p2::twopass_mb_grid(false, 25, 33);
    assert_eq!((cols2, rows2), (2, 3));
}

// ---------------------------------------------------------------------------
// calculate_modified_err
// ---------------------------------------------------------------------------

#[test]
fn modified_err_is_the_raw_bit_count_or_zero() {
    // C: `if (stats == NULL) return 0; return
    //     (double)this_frame->stat_struct.total_num_bits;`
    // Despite the name there is no modification at all.
    assert_eq!(p2::calculate_modified_err(true, 12_345), 12_345.0);
    assert_eq!(p2::calculate_modified_err(false, 12_345), 0.0);
    assert_eq!(p2::calculate_modified_err(true, 0), 0.0);
}
