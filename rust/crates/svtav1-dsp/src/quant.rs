//! Quantization and dequantization.
//!
//! Spec 04: Dead-zone quantization and dequantization.
//!
//! Ported from SVT-AV1's quantization routines. Implements dead-zone
//! quantization (forward) and uniform dequantization (inverse).
//!
//! Uses archmage SIMD dispatch for auto-vectorization of the inner loops.

use archmage::prelude::*;
use svtav1_types::transform::TranLow;

/// Quantization parameters for a transform block.
#[derive(Debug, Clone, Copy)]
pub struct QuantParam {
    /// Dequantization factors: `[0]` = DC, `[1]` = AC.
    pub dequant: [i32; 2],
    /// Right-shift applied after multiplication during quantization.
    pub shift: i32,
}

/// Forward quantization with dead-zone.
///
/// For each coefficient `coeffs[i]`:
/// - Determine the dequant factor (`dequant[0]` for DC at i=0, `dequant[1]` for AC).
/// - Compute the quantized value with dead-zone rounding.
/// - Store quantized coefficients in `qcoeffs` and dequantized in `dqcoeffs`.
/// - Returns the end-of-block (eob): one past the last nonzero quantized coefficient.
///
/// `eob_hint` is the number of coefficients to process (typically the block size).
pub fn quantize(
    coeffs: &[TranLow],
    qparam: &QuantParam,
    qcoeffs: &mut [TranLow],
    dqcoeffs: &mut [TranLow],
    eob_hint: usize,
) -> usize {
    incant!(
        quantize_impl(coeffs, qparam, qcoeffs, dqcoeffs, eob_hint),
        [v3, neon, scalar]
    )
}

fn quantize_impl_scalar(
    _token: ScalarToken,
    coeffs: &[TranLow],
    qparam: &QuantParam,
    qcoeffs: &mut [TranLow],
    dqcoeffs: &mut [TranLow],
    eob_hint: usize,
) -> usize {
    quantize_core(coeffs, qparam, qcoeffs, dqcoeffs, eob_hint)
}

#[cfg(target_arch = "x86_64")]
#[arcane]
fn quantize_impl_v3(
    _token: Desktop64,
    coeffs: &[TranLow],
    qparam: &QuantParam,
    qcoeffs: &mut [TranLow],
    dqcoeffs: &mut [TranLow],
    eob_hint: usize,
) -> usize {
    quantize_core(coeffs, qparam, qcoeffs, dqcoeffs, eob_hint)
}

/// NEON quantize.
///
/// The divisor `dequant` is loop-invariant, but NEON has no integer divide, so
/// the quotient is computed in f32. That is EXACT only while the numerator is
/// below 2^24 (f32's exact-integer limit) — the same argument that makes the
/// BRAG unpremultiply kernel exact: a correctly-rounded f32 quotient cannot
/// cross an integer boundary when the true fractional part is a multiple of
/// 1/d and the quotient is small enough that half an ULP stays under 1/d.
///
/// `TranLow` is `i32` and `shift` is caller-supplied, so that bound is NOT
/// guaranteed by the types. Rather than assume a coefficient range, this
/// CHECKS it: one pass computes the maximum |coeff|, and if `max << shift`
/// could reach 2^24 the whole block falls back to `quantize_core`. Real AV1
/// coefficients are far below that, so the fast path is what runs — but a
/// pathological input gets the scalar answer, not a wrong one.
///
/// `eob` is recovered by a backward scan rather than tracked in the loop,
/// which keeps the vector body branch-free.
#[cfg(target_arch = "aarch64")]
#[arcane]
fn quantize_impl_neon(
    _token: NeonToken,
    coeffs: &[TranLow],
    qparam: &QuantParam,
    qcoeffs: &mut [TranLow],
    dqcoeffs: &mut [TranLow],
    eob_hint: usize,
) -> usize {
    let n = coeffs
        .len()
        .min(qcoeffs.len())
        .min(dqcoeffs.len())
        .min(eob_hint);
    let dq_ac = qparam.dequant[1];
    let shift = qparam.shift;

    // Bail to scalar when the f32 quotient would not be provably exact, when
    // the AC divisor is unusable, or when the block is too short to vectorize.
    if n < 8 || dq_ac <= 0 || !(0..=24).contains(&shift) {
        return quantize_core(coeffs, qparam, qcoeffs, dqcoeffs, eob_hint);
    }
    let mut max_abs: i64 = 0;
    for &c in &coeffs[..n] {
        max_abs = max_abs.max((c as i64).abs());
    }
    if (max_abs << shift) >= (1i64 << 24) {
        return quantize_core(coeffs, qparam, qcoeffs, dqcoeffs, eob_hint);
    }

    // Index 0 uses dequant[0]; do it scalar so the vector body has one divisor.
    {
        let dq_dc = qparam.dequant[0];
        if dq_dc == 0 {
            qcoeffs[0] = 0;
            dqcoeffs[0] = 0;
        } else {
            let c = coeffs[0];
            let q = (((c.abs() as i64) << shift) / dq_dc as i64) as i32;
            let s = if c < 0 { -1 } else { 1 };
            qcoeffs[0] = if q == 0 { 0 } else { s * q };
            dqcoeffs[0] = if q == 0 { 0 } else { s * q * dq_dc };
        }
    }

    let inv_dq = vdupq_n_f32(1.0 / dq_ac as f32);
    let dq_v = vdupq_n_s32(dq_ac);
    let zero = vdupq_n_s32(0);

    let mut i = 1usize;
    while i + 4 <= n {
        let c: &[i32; 4] = coeffs[i..i + 4].try_into().unwrap();
        let cv = vld1q_s32(c);
        let a = vabsq_s32(cv);
        let shifted = vshlq_s32(a, vdupq_n_s32(shift));
        // Exact for the checked range: correctly-rounded divide, truncated.
        let q = vcvtq_s32_f32(vmulq_f32(vcvtq_f32_s32(shifted), inv_dq));
        // Re-apply sign; q == 0 stays 0 either way.
        let neg = vcltq_s32(cv, zero);
        let qs = vbslq_s32(neg, vnegq_s32(q), q);
        let dq = vmulq_s32(qs, dq_v);

        let qo: &mut [i32; 4] = (&mut qcoeffs[i..i + 4]).try_into().unwrap();
        vst1q_s32(qo, qs);
        let do_: &mut [i32; 4] = (&mut dqcoeffs[i..i + 4]).try_into().unwrap();
        vst1q_s32(do_, dq);
        i += 4;
    }
    while i < n {
        let c = coeffs[i];
        let q = (((c.abs() as i64) << shift) / dq_ac as i64) as i32;
        let s = if c < 0 { -1 } else { 1 };
        qcoeffs[i] = if q == 0 { 0 } else { s * q };
        dqcoeffs[i] = if q == 0 { 0 } else { s * q * dq_ac };
        i += 1;
    }

    // eob = (index of last nonzero quantized coefficient) + 1.
    let mut eob = 0usize;
    for k in (0..n).rev() {
        if qcoeffs[k] != 0 {
            eob = k + 1;
            break;
        }
    }
    eob
}

#[inline]
fn quantize_core(
    coeffs: &[TranLow],
    qparam: &QuantParam,
    qcoeffs: &mut [TranLow],
    dqcoeffs: &mut [TranLow],
    eob_hint: usize,
) -> usize {
    let n = coeffs
        .len()
        .min(qcoeffs.len())
        .min(dqcoeffs.len())
        .min(eob_hint);
    let mut eob = 0;

    for i in 0..n {
        let coeff = coeffs[i];
        let dequant = if i == 0 {
            qparam.dequant[0]
        } else {
            qparam.dequant[1]
        };

        if dequant == 0 {
            qcoeffs[i] = 0;
            dqcoeffs[i] = 0;
            continue;
        }

        let sign = if coeff < 0 { -1i32 } else { 1i32 };
        let abs_coeff = coeff.abs() as i64;

        // Dead-zone quantization: q = (abs_coeff * (1 << shift)) / dequant
        // The dead-zone means small coefficients quantize to zero naturally.
        let shifted = abs_coeff << qparam.shift;
        let q = (shifted / dequant as i64) as i32;

        if q == 0 {
            qcoeffs[i] = 0;
            dqcoeffs[i] = 0;
        } else {
            qcoeffs[i] = sign * q;
            dqcoeffs[i] = sign * q * dequant;
            eob = i + 1;
        }
    }

    eob
}

/// Dequantization: multiply quantized coefficients by the dequant factor.
///
/// `eob` is end-of-block — coefficients at indices >= eob are set to zero.
pub fn dequantize(qcoeffs: &[TranLow], dequant: &[i32; 2], output: &mut [TranLow], eob: usize) {
    let n = qcoeffs.len().min(output.len());

    for i in 0..n {
        if i >= eob {
            output[i] = 0;
        } else {
            let dq = if i == 0 { dequant[0] } else { dequant[1] };
            output[i] = qcoeffs[i] * dq;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_qparam() -> QuantParam {
        QuantParam {
            dequant: [4, 8],
            shift: 2,
        }
    }

    // --- Zero input ---

    #[test]
    fn quantize_zero_coeffs() {
        let coeffs = [0i32; 16];
        let qparam = default_qparam();
        let mut qcoeffs = [0i32; 16];
        let mut dqcoeffs = [0i32; 16];
        let eob = quantize(&coeffs, &qparam, &mut qcoeffs, &mut dqcoeffs, 16);
        assert_eq!(eob, 0);
        assert!(qcoeffs.iter().all(|&v| v == 0));
        assert!(dqcoeffs.iter().all(|&v| v == 0));
    }

    #[test]
    fn dequantize_zero_coeffs() {
        let qcoeffs = [0i32; 16];
        let dequant = [4, 8];
        let mut output = [999i32; 16];
        dequantize(&qcoeffs, &dequant, &mut output, 16);
        assert!(output.iter().all(|&v| v == 0));
    }

    // --- Sign preservation ---

    #[test]
    fn quantize_dequantize_preserves_sign() {
        let coeffs = [100, -200, 50, -75i32, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let qparam = default_qparam();
        let mut qcoeffs = [0i32; 16];
        let mut dqcoeffs = [0i32; 16];
        let eob = quantize(&coeffs, &qparam, &mut qcoeffs, &mut dqcoeffs, 16);
        assert!(eob > 0);

        // Check sign preservation
        for i in 0..4 {
            if coeffs[i] != 0 && qcoeffs[i] != 0 {
                assert_eq!(
                    coeffs[i].signum(),
                    qcoeffs[i].signum(),
                    "sign mismatch at [{}]",
                    i
                );
                assert_eq!(
                    coeffs[i].signum(),
                    dqcoeffs[i].signum(),
                    "dequant sign mismatch at [{}]",
                    i
                );
            }
        }
    }

    // --- Large coefficients survive ---

    #[test]
    fn quantize_large_coefficients() {
        let coeffs = [
            10000, -20000, 30000, -40000i32, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        let qparam = default_qparam();
        let mut qcoeffs = [0i32; 16];
        let mut dqcoeffs = [0i32; 16];
        let eob = quantize(&coeffs, &qparam, &mut qcoeffs, &mut dqcoeffs, 16);
        assert!(
            eob >= 4,
            "all large coefficients should survive quantization"
        );
        for i in 0..4 {
            assert!(
                qcoeffs[i] != 0,
                "large coeff[{}] should not quantize to zero",
                i
            );
        }
    }

    // --- DC uses dequant[0], AC uses dequant[1] ---

    #[test]
    fn dc_uses_dequant0_ac_uses_dequant1() {
        let coeffs = [100i32, 100, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let qparam = QuantParam {
            dequant: [2, 16], // Very different DC vs AC dequant
            shift: 2,
        };
        let mut qcoeffs = [0i32; 16];
        let mut dqcoeffs = [0i32; 16];
        quantize(&coeffs, &qparam, &mut qcoeffs, &mut dqcoeffs, 16);

        // DC (index 0): dequant = 2, so q = (100 << 2) / 2 = 200
        // dqcoeff = 200 * 2 = 400
        assert_eq!(qcoeffs[0], 200);
        assert_eq!(dqcoeffs[0], 400);

        // AC (index 1): dequant = 16, so q = (100 << 2) / 16 = 25
        // dqcoeff = 25 * 16 = 400
        assert_eq!(qcoeffs[1], 25);
        assert_eq!(dqcoeffs[1], 400);
    }

    // --- Dequantize respects eob ---

    #[test]
    fn dequantize_respects_eob() {
        let qcoeffs = [10i32, 20, 30, 40, 50, 60, 70, 80, 0, 0, 0, 0, 0, 0, 0, 0];
        let dequant = [4, 8];
        let mut output = [0i32; 16];
        dequantize(&qcoeffs, &dequant, &mut output, 4);

        // First 4 should be dequantized
        assert_eq!(output[0], 10 * 4); // DC
        assert_eq!(output[1], 20 * 8); // AC
        assert_eq!(output[2], 30 * 8);
        assert_eq!(output[3], 40 * 8);

        // Rest should be zero (beyond eob)
        for i in 4..16 {
            assert_eq!(output[i], 0, "output[{}] should be 0 beyond eob", i);
        }
    }

    // --- Roundtrip: quantize then dequantize ---

    #[test]
    fn quantize_then_dequantize_roundtrip() {
        let coeffs = [
            500, -300, 200, -100i32, 50, -25, 10, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        let qparam = QuantParam {
            dequant: [4, 4],
            shift: 0,
        };
        let mut qcoeffs = [0i32; 16];
        let mut dqcoeffs_from_quant = [0i32; 16];
        let eob = quantize(&coeffs, &qparam, &mut qcoeffs, &mut dqcoeffs_from_quant, 16);

        let mut dqcoeffs_from_dequant = [0i32; 16];
        dequantize(&qcoeffs, &qparam.dequant, &mut dqcoeffs_from_dequant, eob);

        // Both dequantization paths should agree
        for i in 0..16 {
            assert_eq!(
                dqcoeffs_from_quant[i], dqcoeffs_from_dequant[i],
                "dequant mismatch at [{}]",
                i
            );
        }
    }

    // --- Dead-zone behavior ---

    #[test]
    fn small_coefficients_quantize_to_zero() {
        // With dequant=100 and shift=0, coefficients below 100 should quantize to zero
        let coeffs = [50, -50, 99, -99i32, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let qparam = QuantParam {
            dequant: [100, 100],
            shift: 0,
        };
        let mut qcoeffs = [0i32; 16];
        let mut dqcoeffs = [0i32; 16];
        let eob = quantize(&coeffs, &qparam, &mut qcoeffs, &mut dqcoeffs, 16);
        assert_eq!(eob, 0, "all small coefficients should be in the dead zone");
        assert!(qcoeffs.iter().all(|&v| v == 0));
    }
}

#[cfg(test)]
mod dispatch_tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;
    use archmage::testing::{CompileTimePolicy, for_each_token_permutation};

    #[test]
    fn quantize_all_dispatch_levels() {
        let coeffs = [
            500, -300, 200, -150, 100, -75, 50, -25, 10, -5, 1000, -2000, 3000, -4000, 5000,
            -6000i32,
        ];
        let qparam = QuantParam {
            dequant: [4, 8],
            shift: 2,
        };

        let mut ref_qcoeffs = [0i32; 16];
        let mut ref_dqcoeffs = [0i32; 16];
        let ref_eob = quantize(&coeffs, &qparam, &mut ref_qcoeffs, &mut ref_dqcoeffs, 16);

        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            let mut qcoeffs = [0i32; 16];
            let mut dqcoeffs = [0i32; 16];
            let eob = quantize(&coeffs, &qparam, &mut qcoeffs, &mut dqcoeffs, 16);
            assert_eq!(eob, ref_eob, "eob mismatch at dispatch level");
            assert_eq!(qcoeffs, ref_qcoeffs, "qcoeffs mismatch at dispatch level");
            assert_eq!(
                dqcoeffs, ref_dqcoeffs,
                "dqcoeffs mismatch at dispatch level"
            );
        });
    }

    #[test]
    fn quantize_dispatch_zero_input() {
        let coeffs = [0i32; 16];
        let qparam = QuantParam {
            dequant: [4, 8],
            shift: 1,
        };

        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            let mut qcoeffs = [0i32; 16];
            let mut dqcoeffs = [0i32; 16];
            let eob = quantize(&coeffs, &qparam, &mut qcoeffs, &mut dqcoeffs, 16);
            assert_eq!(eob, 0);
            assert!(qcoeffs.iter().all(|&v| v == 0));
        });
    }

    #[test]
    fn quantize_dispatch_large_block() {
        // 64 coefficients (8x8 block)
        let coeffs: Vec<i32> = (0..64).map(|i| (i - 32) * 100).collect();
        let qparam = QuantParam {
            dequant: [8, 16],
            shift: 2,
        };

        let mut ref_qcoeffs = vec![0i32; 64];
        let mut ref_dqcoeffs = vec![0i32; 64];
        let ref_eob = quantize(&coeffs, &qparam, &mut ref_qcoeffs, &mut ref_dqcoeffs, 64);

        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            let mut qcoeffs = vec![0i32; 64];
            let mut dqcoeffs = vec![0i32; 64];
            let eob = quantize(&coeffs, &qparam, &mut qcoeffs, &mut dqcoeffs, 64);
            assert_eq!(eob, ref_eob);
            assert_eq!(qcoeffs, ref_qcoeffs);
            assert_eq!(dqcoeffs, ref_dqcoeffs);
        });
    }
}
