use super::*;

/// **EVIDENCE TIERS in this module, stated per test.** [`ssim2`] and
/// everything under it are pinned at TIER 1 by
/// `tests/c_parity_enc_dec_metrics.rs`. The 10-bit chain, `get_sse_10bit`
/// and [`recode_loop_decision_maker`] are TIER 4 — hand-derived vectors
/// traced against the C source — because the 10-bit walker's symbol has a
/// constant-folded ABI and the other two were inlined away entirely. Each
/// test below says which it is.
const _: () = ();

fn seq(seq: &mut SeqRc) -> &mut SeqRc {
    seq
}

/// TIER 4. The 8x8 accumulators, hand-computed for a uniform window.
#[test]
fn ssim_parms_8x8_accumulates_into_the_caller_s_sums() {
    let s = vec![10u8; 64];
    let r = vec![20u8; 64];
    let mut sums = SsimSums::default();
    ssim_parms_8x8(&s, 8, &r, 8, &mut sums);
    assert_eq!(sums.sum_s, 64 * 10);
    assert_eq!(sums.sum_r, 64 * 20);
    assert_eq!(sums.sum_sq_s, 64 * 100);
    assert_eq!(sums.sum_sq_r, 64 * 400);
    assert_eq!(sums.sum_sxr, 64 * 200);
    // C ADDS INTO the out-params without clearing them; a second call must
    // double, not replace.
    ssim_parms_8x8(&s, 8, &r, 8, &mut sums);
    assert_eq!(sums.sum_s, 2 * 64 * 10);
}

/// TIER 4. The split 10-bit sample assembly:
/// `(hi << 2) | ((lo >> 6) & 3)`.
#[test]
fn highbd_ssim_parms_8x8_assembles_the_split_sample() {
    let hi = vec![128u8; 64];
    // 0b1000_0000 >> 6 == 0b10 == 2.
    let lo = vec![128u8; 64];
    let r = vec![511u16; 64];
    let mut sums = SsimSums::default();
    highbd_ssim_parms_8x8(&hi, 8, &lo, 8, &r, 8, &mut sums);
    let ss = (128u32 << 2) + 2; // 514
    assert_eq!(sums.sum_s, 64 * ss);
    assert_eq!(sums.sum_sq_s, 64 * ss * ss);
    assert_eq!(sums.sum_sxr, 64 * ss * 511);
}

/// TIER 4, and the vector is the one that CAUGHT the constant-folded C
/// symbol: a uniform 8x8 window at `ss = 514`, `r = 511`, `bd = 10`,
/// `shift = 2`. Computed independently from the C source arithmetic
/// (the sums shift by `shift` / `2 * shift` AFTER accumulation, and the
/// bd-10 constants are `(428658 * 64 * 64) >> 12` and
/// `(3857925 * 64 * 64) >> 12`).
///
/// The promoted `aom_highbd_ssim2` returns 0.9999828709000638 for the same
/// input, which is the `shift = 0` answer — that is the specialisation,
/// not a port bug. See `link_globalized_enc_dec_statics` in
/// `svtav1-cref/build.rs`.
#[test]
fn highbd_ssim_8x8_applies_the_shift_after_accumulation() {
    let hi = vec![128u8; 64];
    let lo = vec![128u8; 64];
    let r = vec![511u16; 64];
    let got = highbd_ssim_8x8(&hi, 8, &lo, 8, &r, 8, 10, 2);
    assert_eq!(got.to_bits(), 0.999_982_921_923_913_5_f64.to_bits());
    // shift = 0 is the value the specialised C symbol returns.
    let unshifted = highbd_ssim_8x8(&hi, 8, &lo, 8, &r, 8, 10, 0);
    assert_eq!(unshifted.to_bits(), 0.999_982_870_900_063_8_f64.to_bits());
    assert_ne!(got.to_bits(), unshifted.to_bits());
}

/// TIER 4. The `<= 8` guard, on BOTH dimensions, returns NaN — and 8 is
/// inside the guard, not outside it.
#[test]
fn ssim2_returns_nan_for_a_too_small_region() {
    let a = vec![0u8; 16 * 16];
    assert!(ssim2(&a, 16, &a, 16, 8, 16).is_nan());
    assert!(ssim2(&a, 16, &a, 16, 16, 8).is_nan());
    assert!(ssim2(&a, 16, &a, 16, 4, 4).is_nan());
    assert!(!ssim2(&a, 16, &a, 16, 9, 9).is_nan());
    let hi = vec![0u8; 16 * 16];
    let r = vec![0u16; 16 * 16];
    assert!(highbd_ssim2(&hi, 16, &hi, 16, &r, 16, 8, 16, 10, 2).is_nan());
}

/// TIER 4. The window steps by **4**, not 8, so a 16x16 region yields
/// 3x3 = 9 windows rather than 2x2 = 4. Stepping by 8 would be a
/// different metric that still "works".
#[test]
fn ssim2_steps_the_window_by_four() {
    // Two planes that differ only in one 4x4 quadrant: an 8-step walk
    // would visit it once, a 4-step walk four times, so the scores differ.
    let mut a = vec![100u8; 16 * 16];
    let b = vec![100u8; 16 * 16];
    for y in 4..8 {
        for x in 4..8 {
            a[y * 16 + x] = 200;
        }
    }
    let four_step = ssim2(&a, 16, &b, 16, 16, 16);
    // Recompute with an 8-step walk by hand to show it is a different
    // number, i.e. that this test can actually fail.
    let mut total = 0.0;
    let mut n = 0u32;
    let mut i = 0;
    while i + 8 <= 16 {
        let mut j = 0;
        while j + 8 <= 16 {
            total += ssim_8x8(&a[i * 16 + j..], 16, &b[i * 16 + j..], 16);
            n += 1;
            j += 8;
        }
        i += 8;
    }
    let eight_step = total / f64::from(n);
    assert_eq!(n, 4);
    assert_ne!(four_step.to_bits(), eight_step.to_bits());
}

/// TIER 4. `svt_aom_similarity` selects its stabilisers on `bd`, and the
/// three arms give three different scores for the same sums.
#[test]
fn similarity_selects_constants_on_bit_depth() {
    let (a, b, c, d, e) = (8224u32, 8176u32, 1_056_784u32, 1_044_484u32, 1_050_616u32);
    let v8 = similarity(a, b, c, d, e, 64, 8);
    let v10 = similarity(a, b, c, d, e, 64, 10);
    let v12 = similarity(a, b, c, d, e, 64, 12);
    assert_ne!(v8.to_bits(), v10.to_bits());
    assert_ne!(v10.to_bits(), v12.to_bits());
}

/// TIER 4. C's `else` arm zeroes c1/c2 and asserts; the port panics.
#[test]
#[should_panic(expected = "unsupported bit depth")]
fn similarity_refuses_an_unknown_bit_depth() {
    let _ = similarity(1, 1, 1, 1, 1, 64, 9);
}

/// TIER 4. `get_sse_10bit` assembles its sample with an OR and no mask,
/// and squares a SIGNED difference.
#[test]
fn get_sse_10bit_squares_a_signed_difference() {
    let hi = vec![128u8; 16];
    let lo = vec![128u8; 16]; // >> 6 == 2
    // sample = (128 << 2) | 2 == 514
    let b = vec![500u16; 16];
    let sse = get_sse_10bit(&hi, 4, &lo, 4, &b, 4, 4, 4);
    assert_eq!(sse, 16 * (514i64 - 500) * (514 - 500));
    // A reference ABOVE the source gives the same magnitude.
    let b_hi = vec![528u16; 16];
    let sse2 = get_sse_10bit(&hi, 4, &lo, 4, &b_hi, 4, 4, 4);
    assert_eq!(sse2, 16 * 14 * 14);
}

fn rc_with_cap(cap: i32) -> RateControl {
    RateControl {
        max_frame_bandwidth: cap,
        ..Default::default()
    }
}

/// TIER 4. The RTC-CBR path RETURNS EARLY: it never runs the VBR
/// bisection, and when it declines a recode it does NOT reset
/// `loop_count` (the VBR path does).
#[test]
fn recode_loop_decision_maker_rtc_cbr_returns_early() {
    let mut s = SeqRc::default();
    let scs = seq(&mut s);
    let rc = rc_with_cap(1_000_000);

    let yes = recode_loop_decision_maker(scs, &rc, true, true, 120, 3, false, 0, 200, true);
    assert!(yes.do_recode);
    assert_eq!(yes.base_q_idx, 120, "the RTC path keeps the qindex it had");
    assert_eq!(yes.loop_count, 4);
    assert!(yes.reseed_sb_qindex);

    let no = recode_loop_decision_maker(scs, &rc, true, false, 120, 3, false, 0, 200, true);
    assert!(!no.do_recode);
    assert_eq!(
        no.loop_count, 3,
        "the RTC early return must NOT reset loop_count"
    );
    assert!(!no.reseed_sb_qindex);
}

/// TIER 4. An overlay frame already under the burst cap CANCELS a recode
/// the bisection asked for — and the cancel path resets `loop_count`.
#[test]
fn recode_loop_decision_maker_overlay_under_cap_cancels() {
    let mut s = SeqRc::default();
    let scs = seq(&mut s);
    let rc = rc_with_cap(1_000_000);

    let cancelled =
        recode_loop_decision_maker(scs, &rc, false, false, 120, 2, true, 500_000, 200, true);
    assert!(!cancelled.do_recode);
    assert_eq!(cancelled.loop_count, 0);

    // At or above the cap the cancel does not apply.
    let kept =
        recode_loop_decision_maker(scs, &rc, false, false, 120, 2, true, 1_000_000, 200, true);
    assert!(kept.do_recode);
    assert_eq!(kept.loop_count, 3);

    // A non-overlay frame is never cancelled.
    let inter =
        recode_loop_decision_maker(scs, &rc, false, false, 120, 2, false, 500_000, 200, true);
    assert!(inter.do_recode);
}

/// TIER 4. On a recode the new qindex is clamped through
/// `quantizer_to_qindex[min/max_qp_allowed]` and `picture_qp` is
/// re-derived as `(base_q_idx + 2) >> 2` clamped to the QP domain — the
/// two clamps are in DIFFERENT domains and using one for both is the easy
/// mistake.
#[test]
fn recode_loop_decision_maker_clamps_qindex_and_qp_in_their_own_domains() {
    let mut s = SeqRc {
        min_qp_allowed: 10,
        max_qp_allowed: 40,
        ..Default::default()
    };
    let scs = seq(&mut s);
    let rc = rc_with_cap(1_000_000);

    // A wildly high q clamps to quantizer_to_qindex[40].
    let d = recode_loop_decision_maker(scs, &rc, false, false, 100, 0, false, 0, 250, true);
    let qmax = i32::from(crate::rate_control::qp_to_qindex(40));
    assert_eq!(d.base_q_idx, qmax);
    assert_eq!(d.picture_qp, 40);

    // A wildly low q clamps to quantizer_to_qindex[10].
    let d = recode_loop_decision_maker(scs, &rc, false, false, 100, 0, false, 0, -5, true);
    let qmin = i32::from(crate::rate_control::qp_to_qindex(10));
    assert_eq!(d.base_q_idx, qmin);
    assert_eq!(d.picture_qp, 10);
}
