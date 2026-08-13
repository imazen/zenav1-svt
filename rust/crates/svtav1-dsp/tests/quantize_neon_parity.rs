//! NEON `quantize` must equal the scalar core exactly, including the returned
//! `eob`.
//!
//! The NEON path computes the quotient in f32, which is exact only while the
//! numerator stays under 2^24. It checks that bound at runtime and falls back
//! to the scalar core otherwise — so these tests deliberately probe BOTH sides
//! of that boundary, plus the eob recovery (a backward scan, rather than the
//! scalar's in-loop tracking) and the dequant == 0 and DC-vs-AC divisor paths.

use svtav1_dsp::quant::{QuantParam, quantize};

fn scalar(coeffs: &[i32], qp: &QuantParam, qc: &mut [i32], dqc: &mut [i32], hint: usize) -> usize {
    let n = coeffs.len().min(qc.len()).min(dqc.len()).min(hint);
    let mut eob = 0;
    for i in 0..n {
        let dequant = if i == 0 { qp.dequant[0] } else { qp.dequant[1] };
        if dequant == 0 {
            qc[i] = 0;
            dqc[i] = 0;
            continue;
        }
        let sign = if coeffs[i] < 0 { -1i32 } else { 1 };
        let shifted = (coeffs[i].abs() as i64) << qp.shift;
        let q = (shifted / dequant as i64) as i32;
        if q == 0 {
            qc[i] = 0;
            dqc[i] = 0;
        } else {
            qc[i] = sign * q;
            dqc[i] = sign * q * dequant;
            eob = i + 1;
        }
    }
    eob
}

fn check(coeffs: &[i32], qp: &QuantParam, hint: usize, what: &str) {
    let n = coeffs.len();
    let (mut a, mut b) = (vec![0i32; n], vec![0i32; n]);
    let (mut c, mut d) = (vec![0i32; n], vec![0i32; n]);
    let got_eob = quantize(coeffs, qp, &mut a, &mut b, hint);
    let want_eob = scalar(coeffs, qp, &mut c, &mut d, hint);
    assert_eq!(a, c, "qcoeffs differ: {what}");
    assert_eq!(b, d, "dqcoeffs differ: {what}");
    assert_eq!(got_eob, want_eob, "eob differs: {what}");
}

#[test]
fn quantize_matches_scalar_across_shapes_and_divisors() {
    let mut s = 0x9e37_79b9u32;
    let mut next = move || {
        s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        ((s >> 12) as i32 & 0x1FFF) - 4096
    };
    for &len in &[1usize, 4, 7, 8, 15, 16, 63, 64, 256, 1024] {
        let coeffs: Vec<i32> = (0..len).map(|_| next()).collect();
        for &dc in &[1i32, 8, 20, 255] {
            for &ac in &[1i32, 12, 24, 300] {
                for &shift in &[0i32, 1, 2, 4] {
                    let qp = QuantParam {
                        dequant: [dc, ac],
                        shift,
                    };
                    check(
                        &coeffs,
                        &qp,
                        len,
                        &format!("len={len} dc={dc} ac={ac} shift={shift}"),
                    );
                    // Partial eob_hint must also agree.
                    check(&coeffs, &qp, len / 2, &format!("len={len} half-hint"));
                }
            }
        }
    }
}

#[test]
fn quantize_handles_zero_dequant() {
    let coeffs: Vec<i32> = (0..64).map(|i| (i as i32) * 37 - 900).collect();
    for &(dc, ac) in &[(0i32, 24i32), (20, 0), (0, 0)] {
        let qp = QuantParam {
            dequant: [dc, ac],
            shift: 2,
        };
        check(&coeffs, &qp, 64, &format!("dequant dc={dc} ac={ac}"));
    }
}

#[test]
fn quantize_falls_back_past_the_f32_exact_bound() {
    // The NEON path is exact only while (|coeff| << shift) < 2^24. Straddle it:
    // one set safely under, one set deliberately over (which must take the
    // scalar fallback and still match).
    // Values are chosen to straddle 2^24 AFTER shifting while keeping the
    // final `q * dequant` inside i32 — the scalar reference computes that in
    // i32 and would itself overflow (debug builds panic) on larger inputs, so
    // anything bigger tests nothing but the test's own arithmetic. With
    // shift=4 and dequant=24: coeff 1<<22 gives shifted 1<<26 (past the bound,
    // so the fallback runs) and q*dequant = 67,108,848, well inside i32.
    let under: Vec<i32> = (0..64).map(|i| 1 << 15 | (i as i32)).collect();
    let over: Vec<i32> = (0..64).map(|i| (1 << 22) + (i as i32) * 1013).collect();
    let mixed: Vec<i32> = (0..64)
        .map(|i| if i == 31 { 1 << 22 } else { i as i32 * 11 })
        .collect();
    for &shift in &[0i32, 2, 4] {
        let qp = QuantParam {
            dequant: [20, 24],
            shift,
        };
        check(&under, &qp, 64, &format!("under bound, shift={shift}"));
        check(&over, &qp, 64, &format!("OVER bound, shift={shift}"));
        check(&mixed, &qp, 64, &format!("one huge coeff, shift={shift}"));
    }
}

#[test]
fn quantize_all_zero_gives_eob_zero() {
    let coeffs = vec![0i32; 64];
    let qp = QuantParam {
        dequant: [20, 24],
        shift: 2,
    };
    check(&coeffs, &qp, 64, "all zero");
}
