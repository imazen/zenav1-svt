// The CfL predict kernel's SIMD arms. Included from `intra_pred.rs` (`include!`
// is NOT used — this is a real module; see `intra_pred::cfl_predict_lbd`).
//
// C's kernel is `svt_cfl_predict_lbd_avx2` (`ASM_AVX2/cfl_avx2.c:38`), reached
// through the RTCD FUNCTION POINTER `svt_cfl_predict_lbd`
// (`Codec/common_dsp_rtcd.h:73`, bound at `common_dsp_rtcd.c:494`) — so C pays
// an INDIRECT, OUT-OF-LINE call per alpha per plane and still costs 99 Ir a
// call. Its whole advantage is the ARITHMETIC, not the linkage.
//
//   ac_sign = sign_epi16(alpha_sign, ac_q3)          // sign(alpha)*sign(ac)
//   mag     = mulhrs_epi16(abs_epi16(ac_q3), |alpha| << 9)
//   res     = add_epi16(sign_epi16(mag, ac_sign), dc_q0)
//
// `mulhrs(x, y) = (x*y + 16384) >> 15`, so with `y = |alpha| << 9`,
// `mag = (|ac| * |alpha| * 512 + 16384) >> 15 = (|ac| * |alpha| + 32) >> 6`
// exactly (the numerator is `512 * (|ac|*|alpha| + 32)`) — C's
// round-half-away-from-zero by 6, done 16 i16 lanes at a time with no i32
// widening at all. The port's branch-free scalar loop is the same value
// computed in i32, which on the x86-64 BASELINE costs a `pmuludq`/`pshufd`
// quartet per four lanes because LLVM cannot know `alpha_q3` fits in 16 bits.
//
// The sign is applied here as `(x ^ m) - m` with `m = (ac >> 15) ^ alpha_neg`
// (0 or -1) rather than as two `sign_epi16`s: one instruction more on x86, but
// it is the same expression on both ISAs and NEON has no `sign_epi16`.
//
// DOMAIN. Every arm is exact against
// [`super::intra_pred::cfl_predict_lbd_core`] for every `alpha_q3` C can
// produce (`cfl_idx_to_alpha`: 0 and magnitudes 1..=16 either sign) and every
// `ac_q3` an `i16` can hold EXCEPT `i16::MIN`, where `abs_epi16`/`vabsq_s16`
// return `i16::MIN` unchanged. C's own AVX2 and NEON kernels have exactly that
// quirk (and disagree with each other there), so matching it is matching the
// oracle. It is unreachable regardless: `cfl_luma_subsampling_420` writes
// `2 * (sum of four u8) <= 2040` and `cfl_subtract_average` subtracts a mean of
// the same buffer, so `|ac_q3| <= 2040`. Pinned by
// `cfl_simd_matches_scalar_over_the_reachable_domain`.

use archmage::prelude::*;

use super::intra_pred::{CFL_BUF_LINE, cfl_predict_lbd_core};

pub(crate) fn cfl_predict_lbd_dispatch(
    pred_buf_q3: &[i16],
    pred: &[u8],
    pred_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    alpha_q3: i32,
    width: usize,
    height: usize,
) {
    incant!(
        cfl_predict_lbd_impl(
            pred_buf_q3,
            pred,
            pred_stride,
            dst,
            dst_stride,
            alpha_q3,
            width,
            height
        ),
        [v3, neon, scalar]
    )
}

#[allow(clippy::too_many_arguments)]
fn cfl_predict_lbd_impl_scalar(
    _token: ScalarToken,
    pred_buf_q3: &[i16],
    pred: &[u8],
    pred_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    alpha_q3: i32,
    width: usize,
    height: usize,
) {
    cfl_predict_lbd_core(
        pred_buf_q3,
        pred,
        pred_stride,
        dst,
        dst_stride,
        alpha_q3,
        width,
        height,
    );
}

#[cfg(target_arch = "x86_64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn cfl_predict_lbd_impl_v3(
    _token: Desktop64,
    pred_buf_q3: &[i16],
    pred: &[u8],
    pred_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    alpha_q3: i32,
    width: usize,
    height: usize,
) {
    // |alpha| << 9 and the "negate the product" mask, both loop-invariant.
    let a16 = _mm_set1_epi16(alpha_q3 as i16);
    let q12_128 = _mm_slli_epi16::<9>(_mm_abs_epi16(a16));
    let q12_256 = _mm256_slli_epi16::<9>(_mm256_abs_epi16(_mm256_set1_epi16(alpha_q3 as i16)));
    let neg16 = if alpha_q3 < 0 { -1i16 } else { 0 };
    let aneg_128 = _mm_set1_epi16(neg16);
    let aneg_256 = _mm256_set1_epi16(neg16);

    for j in 0..height {
        let acr = &pred_buf_q3[j * CFL_BUF_LINE..j * CFL_BUF_LINE + width];
        let pr = &pred[j * pred_stride..j * pred_stride + width];
        let or = &mut dst[j * dst_stride..j * dst_stride + width];
        let mut i = 0;

        while i + 16 <= width {
            let ac: &[i16; 16] = acr[i..i + 16].try_into().unwrap();
            let pb: &[u8; 16] = pr[i..i + 16].try_into().unwrap();
            let ac = _mm256_loadu_si256(ac);
            let m = _mm256_xor_si256(_mm256_srai_epi16::<15>(ac), aneg_256);
            let mag = _mm256_mulhrs_epi16(_mm256_abs_epi16(ac), q12_256);
            let signed = _mm256_sub_epi16(_mm256_xor_si256(mag, m), m);
            let p = _mm256_cvtepu8_epi16(_mm_loadu_si128(pb));
            let sum = _mm256_add_epi16(signed, p);
            let packed = _mm_packus_epi16(
                _mm256_castsi256_si128(sum),
                _mm256_extracti128_si256::<1>(sum),
            );
            let ob: &mut [u8; 16] = (&mut or[i..i + 16]).try_into().unwrap();
            _mm_storeu_si128(ob, packed);
            i += 16;
        }
        while i + 8 <= width {
            let ac: &[i16; 8] = acr[i..i + 8].try_into().unwrap();
            let pb: &[u8; 8] = pr[i..i + 8].try_into().unwrap();
            let ac = _mm_loadu_si128(ac);
            let m = _mm_xor_si128(_mm_srai_epi16::<15>(ac), aneg_128);
            let mag = _mm_mulhrs_epi16(_mm_abs_epi16(ac), q12_128);
            let signed = _mm_sub_epi16(_mm_xor_si128(mag, m), m);
            let p = _mm_cvtepu8_epi16(_mm_loadu_si64(pb));
            let sum = _mm_add_epi16(signed, p);
            let ob: &mut [u8; 8] = (&mut or[i..i + 8]).try_into().unwrap();
            _mm_storeu_si64(ob, _mm_packus_epi16(sum, sum));
            i += 8;
        }
        while i + 4 <= width {
            let ac: &[i16; 4] = acr[i..i + 4].try_into().unwrap();
            let pb: &[u8; 4] = pr[i..i + 4].try_into().unwrap();
            let ac = _mm_loadu_si64(ac);
            let m = _mm_xor_si128(_mm_srai_epi16::<15>(ac), aneg_128);
            let mag = _mm_mulhrs_epi16(_mm_abs_epi16(ac), q12_128);
            let signed = _mm_sub_epi16(_mm_xor_si128(mag, m), m);
            let p = _mm_cvtepu8_epi16(_mm_loadu_si32(pb));
            let sum = _mm_add_epi16(signed, p);
            let ob: &mut [u8; 4] = (&mut or[i..i + 4]).try_into().unwrap();
            _mm_storeu_si32(ob, _mm_packus_epi16(sum, sum));
            i += 4;
        }
        // Tail (width % 4): the scalar core's body verbatim.
        for k in i..width {
            or[k] = scalar_one(alpha_q3, acr[k], pr[k]);
        }
    }
}

#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn cfl_predict_lbd_impl_neon(
    _token: NeonToken,
    pred_buf_q3: &[i16],
    pred: &[u8],
    pred_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    alpha_q3: i32,
    width: usize,
    height: usize,
) {
    // `vqrdmulhq_s16(a, b) = sat((2*a*b + 0x8000) >> 16) = (a*b + 16384) >> 15`
    // — the same value `_mm_mulhrs_epi16` produces; the saturation can only
    // fire at `a == b == i16::MIN`, outside this kernel's domain.
    let q12 = vshlq_n_s16::<9>(vabsq_s16(vdupq_n_s16(alpha_q3 as i16)));
    let aneg = vdupq_n_s16(if alpha_q3 < 0 { -1 } else { 0 });

    for j in 0..height {
        let acr = &pred_buf_q3[j * CFL_BUF_LINE..j * CFL_BUF_LINE + width];
        let pr = &pred[j * pred_stride..j * pred_stride + width];
        let or = &mut dst[j * dst_stride..j * dst_stride + width];
        let mut i = 0;

        while i + 8 <= width {
            let acb: &[i16; 8] = acr[i..i + 8].try_into().unwrap();
            let pb: &[u8; 8] = pr[i..i + 8].try_into().unwrap();
            let ac = vld1q_s16(acb);
            let m = veorq_s16(vshrq_n_s16::<15>(ac), aneg);
            let mag = vqrdmulhq_s16(vabsq_s16(ac), q12);
            let signed = vsubq_s16(veorq_s16(mag, m), m);
            let p = vreinterpretq_s16_u16(vmovl_u8(vld1_u8(pb)));
            let sum = vaddq_s16(signed, p);
            let ob: &mut [u8; 8] = (&mut or[i..i + 8]).try_into().unwrap();
            vst1_u8(ob, vqmovun_s16(sum));
            i += 8;
        }
        // Tail (width % 8): the scalar core's body verbatim.
        for k in i..width {
            or[k] = scalar_one(alpha_q3, acr[k], pr[k]);
        }
    }
}

/// One element of [`cfl_predict_lbd_core`], for the SIMD arms' tails. Kept
/// byte-identical to the loop body there — if you change one, change both;
/// `cfl_simd_matches_scalar_over_the_reachable_domain` compares them.
#[inline(always)]
fn scalar_one(alpha_q3: i32, ac: i16, p: u8) -> u8 {
    let q6 = alpha_q3 * ac as i32;
    let s = q6 >> 31;
    let q0 = ((((q6 ^ s) - s) + 32) >> 6) ^ s;
    ((q0 - s) + p as i32).clamp(0, 255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use archmage::testing::{CompileTimePolicy, TokenPermutation, for_each_token_permutation};

    /// Sweep EVERY dispatch arm, and fail if the sweep degenerated to the
    /// native tier (the silent-coverage hazard `CLAUDE.md` documents: a
    /// discarded `PermutationReport` turns an all-tiers test into a one-tier
    /// test and it still reads green).
    fn for_each_tier(label: &str, f: impl FnMut(&TokenPermutation)) {
        let report = for_each_token_permutation(CompileTimePolicy::WarnStderr, f);
        assert!(
            report.warnings.is_empty(),
            "{label}: archmage excluded {} token(s): {:?}",
            report.warnings.len(),
            report.warnings
        );
        assert!(
            report.permutations_run >= 2,
            "{label}: the tier sweep ran {} permutation(s) -- only the native \
             tier, which cannot catch a SIMD-vs-scalar divergence.",
            report.permutations_run
        );
    }

    /// Every SIMD arm must equal the scalar core over the domain C can reach:
    /// `alpha_q3` from `cfl_idx_to_alpha` (0 and magnitudes 1..=16 either
    /// sign), `ac_q3` over the whole `i16` range except `i16::MIN` (see the
    /// module note), `pred` over the whole `u8` range, and every chroma block
    /// shape CfL is applied to. The buffers are filled so that each row sweeps
    /// a different slice of the `ac` range and every width path (16/8/4 and
    /// each `width % 4` tail) is exercised.
    /// The EXHAUSTIVE half: every `(alpha_q3, ac_q3)` pair the kernel can be
    /// handed — 33 alphas x 65,535 `ac` values, `i16::MIN` excluded per the
    /// module note — driven through the 16-wide, 8-wide and 4-wide paths at
    /// three `pred` values, compared against the scalar core element for
    /// element. 6.5 M comparisons, not sampled.
    #[test]
    fn cfl_simd_matches_scalar_for_every_alpha_and_ac() {
        for_each_tier("cfl_simd_matches_scalar_for_every_alpha_and_ac", |_| {
            let alphas: alloc::vec::Vec<i32> = core::iter::once(0)
                .chain((1..=16).flat_map(|m| [m, -m]))
                .collect();
            // i16::MIN+1 ..= i16::MAX, 65,535 values, padded to a multiple of 16.
            let acs: alloc::vec::Vec<i16> = (i16::MIN as i32 + 1..=i16::MAX as i32)
                .map(|v| v as i16)
                .collect();
            let rows = acs.len().div_ceil(16);
            let mut ac = vec![0i16; CFL_BUF_LINE * rows];
            for (n, &v) in acs.iter().enumerate() {
                ac[(n / 16) * CFL_BUF_LINE + (n % 16)] = v;
            }
            for width in [16usize, 8, 4] {
                for pv in [0u8, 128, 255] {
                    let p = vec![pv; width * rows];
                    let mut want = vec![0u8; width * rows];
                    cfl_predict_lbd_core(&ac, &p, width, &mut want, width, 1, width, rows);
                    for &alpha_q3 in &alphas {
                        cfl_predict_lbd_core(
                            &ac, &p, width, &mut want, width, alpha_q3, width, rows,
                        );
                        let mut got = vec![0u8; width * rows];
                        cfl_predict_lbd_dispatch(
                            &ac, &p, width, &mut got, width, alpha_q3, width, rows,
                        );
                        assert_eq!(got, want, "alpha={alpha_q3} width={width} pred={pv}");
                    }
                }
            }
        });
    }

    #[test]
    fn cfl_simd_matches_scalar_over_the_reachable_domain() {
        for_each_tier("cfl_simd_matches_scalar_over_the_reachable_domain", |_| {
            let alphas: alloc::vec::Vec<i32> = core::iter::once(0)
                .chain((1..=16).flat_map(|m| [m, -m]))
                .collect();
            // Widths 1..=32 cover the 16/8/4 paths and every tail length; the
            // shapes CfL actually sees (4/8/16/32) are all in here.
            for &alpha_q3 in &alphas {
                for width in 1..=32usize {
                    for height in [1usize, 3, 8] {
                        let mut ac = vec![0i16; CFL_BUF_LINE * height];
                        for (n, v) in ac.iter_mut().enumerate() {
                            // Walk the i16 range in a stride coprime with 2^16 so
                            // every arm sees positives, negatives and zero, and
                            // never i16::MIN.
                            let raw = (n as i64 * 30011 + alpha_q3 as i64 * 7919) % 65535;
                            *v = (raw - 32767) as i16;
                        }
                        let p: alloc::vec::Vec<u8> = (0..width * height)
                            .map(|n| ((n * 37 + width * 11) & 0xff) as u8)
                            .collect();
                        let mut want = vec![0u8; width * height];
                        cfl_predict_lbd_core(
                            &ac, &p, width, &mut want, width, alpha_q3, width, height,
                        );
                        let mut got = vec![0u8; width * height];
                        cfl_predict_lbd_dispatch(
                            &ac, &p, width, &mut got, width, alpha_q3, width, height,
                        );
                        assert_eq!(got, want, "alpha={alpha_q3} width={width} height={height}");
                    }
                }
            }
        });
    }
}
