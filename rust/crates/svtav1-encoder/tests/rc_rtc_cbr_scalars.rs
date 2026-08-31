//! `Codec/rc_rtc_cbr.c`'s three pure-scalar helpers
//! (`svtav1-encoder/src/port_rc_rtc_cbr.rs`).
//!
//! **EVIDENCE TIER 4** (`docs/WORKING-ON-THIS.md` §4, the weakest tier):
//! `find_closest_arg` (:73), `normalize_factors` (:175) and `index2tl` (:186)
//! are all `static` in C with no exported symbol. `nm -g` on
//! `Bin/Release/libSvtAv1Enc.a` exports only `svt_av1_rc_calc_qindex_rtc_cbr`,
//! `svt_av1_rc_postencode_update_rtc_cbr` and
//! `svt_av1_rc_recode_decision_rtc_cbr` from that file — none of these three —
//! and reaching them through those wrappers needs a whole `CyclicRefresh` plus
//! a populated PCS. Every expected value below is a literal derived by hand
//! from the C statement quoted beside it.

use svtav1_encoder::port_rc_rtc_cbr as rtc;

// ---------------------------------------------------------------------------
// find_closest_arg (rc_rtc_cbr.c:73)
// ---------------------------------------------------------------------------

/// A strictly decreasing eval so the search's contract holds:
/// `eval(x) = 100 - 10 * x`, i.e. eval(0)=100, eval(3)=70, eval(10)=0.
fn dec(x: i32) -> f64 {
    100.0 - 10.0 * f64::from(x)
}

#[test]
fn find_closest_arg_picks_the_exact_hit() {
    for x in 0..=10i32 {
        assert_eq!(
            rtc::find_closest_arg(dec(x), 0, 10, dec),
            x,
            "exact hit at {x}"
        );
    }
}

#[test]
fn find_closest_arg_takes_the_lower_neighbour_when_it_is_strictly_closer() {
    // target 74 sits between eval(2)=80 and eval(3)=70. The binary search
    // lands on the first arg with eval <= target, i.e. 3 (70). |80-74| = 6 and
    // |70-74| = 4, so 3 stays. At target 76: |80-76| = 4 < |70-76| = 6, so C's
    // look-back moves it to 2. A plain lower-bound search returns 3 both times.
    assert_eq!(rtc::find_closest_arg(74.0, 0, 10, dec), 3);
    assert_eq!(rtc::find_closest_arg(76.0, 0, 10, dec), 2);
}

#[test]
fn find_closest_arg_breaks_ties_toward_the_upper_arg() {
    // target 75 is equidistant from eval(2)=80 and eval(3)=70. C's test is
    // `fabs(prev - target) < fabs(curr - target)` — STRICTLY less — so the tie
    // keeps `curr_arg`, the upper one.
    assert_eq!(rtc::find_closest_arg(75.0, 0, 10, dec), 3);
}

#[test]
fn find_closest_arg_saturates_at_both_bounds() {
    // Above everything: the search never advances past min_arg, and the
    // look-back is skipped because `curr_arg > min_arg` is false.
    assert_eq!(rtc::find_closest_arg(1000.0, 0, 10, dec), 0);
    // Below everything: `lo` walks to max_arg. The look-back then compares
    // eval(9)=10 and eval(10)=0 against -1000: 10 is farther, so 10 stays.
    assert_eq!(rtc::find_closest_arg(-1000.0, 0, 10, dec), 10);
}

#[test]
fn find_closest_arg_honours_a_nonzero_min_arg() {
    // With min_arg = 5 the look-back must not reach 4.
    assert_eq!(rtc::find_closest_arg(1000.0, 5, 10, dec), 5);
    assert_eq!(rtc::find_closest_arg(dec(7), 5, 10, dec), 7);
}

#[test]
fn find_closest_arg_calls_eval_twice_more_after_the_search() {
    // C does not reuse `mid_val` for the look-back; it calls eval again for
    // both `lo - 1` and `lo`. Pin the call pattern so a "cache the last value"
    // rewrite is a visible change rather than a silent one.
    let mut calls = Vec::new();
    let got = rtc::find_closest_arg(76.0, 0, 10, |x| {
        calls.push(x);
        dec(x)
    });
    assert_eq!(got, 2);
    let n = calls.len();
    assert_eq!(
        &calls[n - 2..],
        &[got, got + 1],
        "the last two evals must be (lo-1, lo); saw {calls:?}"
    );
}

// ---------------------------------------------------------------------------
// normalize_factors (rc_rtc_cbr.c:175)
// ---------------------------------------------------------------------------

#[test]
fn normalize_factors_weights_are_one_one_two_four() {
    // C: `sum += src[k] * (1 << AOMMAX(k - i_start - 1, 0));` — the first TWO
    // entries share weight 1 because of the `- 1`.
    // src = [1, 1, 1, 1] over 0..4: sum = 1*1 + 1*1 + 1*2 + 1*4 = 8,
    // divisor = 1 << (4 - 0 - 1) = 8, avg = 1.0, so every dst is src / 1.0.
    let src = [1.0f64, 1.0, 1.0, 1.0];
    let mut dst = [0.0f64; 4];
    rtc::normalize_factors(&mut dst, &src, 0, 4);
    assert_eq!(dst, [1.0, 1.0, 1.0, 1.0]);

    // src = [4, 0, 0, 0]: sum = 4*1 = 4, divisor 8, avg = 0.5.
    // dst = [8, 0, 0, 0].
    let src = [4.0f64, 0.0, 0.0, 0.0];
    let mut dst = [0.0f64; 4];
    rtc::normalize_factors(&mut dst, &src, 0, 4);
    assert_eq!(dst, [8.0, 0.0, 0.0, 0.0]);

    // src = [0, 0, 0, 2]: sum = 2*4 = 8, divisor 8, avg = 1.0 -> unchanged.
    let src = [0.0f64, 0.0, 0.0, 2.0];
    let mut dst = [0.0f64; 4];
    rtc::normalize_factors(&mut dst, &src, 0, 4);
    assert_eq!(dst, [0.0, 0.0, 0.0, 2.0]);
}

#[test]
fn normalize_factors_offsets_the_weights_by_i_start_not_by_zero() {
    // The exponent is `k - i_start - 1`, so shifting the RANGE must not shift
    // the weights. Same values at 0..3 and at 2..5 give the same dst content.
    let src_a = [3.0f64, 5.0, 7.0];
    let mut dst_a = [0.0f64; 3];
    rtc::normalize_factors(&mut dst_a, &src_a, 0, 3);

    let src_b = [99.0f64, 99.0, 3.0, 5.0, 7.0];
    let mut dst_b = [0.0f64; 5];
    rtc::normalize_factors(&mut dst_b, &src_b, 2, 5);

    assert_eq!(dst_a[..], dst_b[2..]);
    // Outside the range C never writes, so those stay at their initial value.
    assert_eq!(dst_b[0], 0.0);
    assert_eq!(dst_b[1], 0.0);
}

#[test]
fn normalize_factors_weights_sum_to_the_divisor() {
    // The identity the doc comment claims: for a range of length n >= 1 the
    // weights sum to 2^(n-1), which IS C's divisor. Checked by normalizing an
    // all-ones range of every length 1..=8 and requiring an exact 1.0 out.
    for n in 1..=8usize {
        let src = vec![1.0f64; n];
        let mut dst = vec![0.0f64; n];
        rtc::normalize_factors(&mut dst, &src, 0, n);
        assert!(
            dst.iter().all(|&v| v == 1.0),
            "length {n}: weights do not sum to the divisor, got {dst:?}"
        );
    }
}

#[test]
fn normalize_factors_empty_range_writes_nothing() {
    let src = [1.0f64, 2.0];
    let mut dst = [9.0f64, 9.0];
    rtc::normalize_factors(&mut dst, &src, 1, 1);
    assert_eq!(dst, [9.0, 9.0]);
}

// ---------------------------------------------------------------------------
// index2tl (rc_rtc_cbr.c:186)
// ---------------------------------------------------------------------------

#[test]
fn index2tl_is_levels_minus_the_lowest_set_bit() {
    // C: `index ? levels - get_msb(index ^ (index - 1)) : 0`.
    // `index ^ (index - 1)` is the low bit plus everything below it, so
    // get_msb of it is the LOWEST set bit's position — a "highest bit"
    // operation applied to a mask that makes it a lowest-bit query.
    // Worked by hand at levels = 4:
    //   1 ^ 0 = 0b0001, msb 0 -> 4
    //   2 ^ 1 = 0b0011, msb 1 -> 3
    //   3 ^ 2 = 0b0001, msb 0 -> 4
    //   4 ^ 3 = 0b0111, msb 2 -> 2
    //   6 ^ 5 = 0b0011, msb 1 -> 3
    //   8 ^ 7 = 0b1111, msb 3 -> 1
    let levels = 4;
    assert_eq!(rtc::index2tl(0, levels), 0, "the ternary's else-branch");
    assert_eq!(rtc::index2tl(1, levels), 4);
    assert_eq!(rtc::index2tl(2, levels), 3);
    assert_eq!(rtc::index2tl(3, levels), 4);
    assert_eq!(rtc::index2tl(4, levels), 2);
    assert_eq!(rtc::index2tl(5, levels), 4);
    assert_eq!(rtc::index2tl(6, levels), 3);
    assert_eq!(rtc::index2tl(7, levels), 4);
    assert_eq!(rtc::index2tl(8, levels), 1);
}

/// The `x ^ (x - 1)` identity, checked against a from-scratch highest-set-bit
/// scan rather than against `trailing_zeros` — otherwise the test would just
/// be restating the implementation.
#[test]
fn index2tl_matches_a_literal_get_msb_of_the_xor_mask() {
    fn get_msb(mut n: u32) -> i32 {
        assert_ne!(n, 0, "C's get_msb asserts n != 0");
        let mut pos = -1i32;
        while n != 0 {
            n >>= 1;
            pos += 1;
        }
        pos
    }
    for levels in [1i32, 3, 4, 5, 6] {
        for index in 1..64u32 {
            let want = levels - get_msb(index ^ (index - 1));
            assert_eq!(
                rtc::index2tl(index, levels),
                want,
                "index2tl({index}, {levels})"
            );
        }
        assert_eq!(rtc::index2tl(0, levels), 0);
    }
}
