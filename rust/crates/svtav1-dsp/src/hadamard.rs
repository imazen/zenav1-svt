//! Hadamard transform for SATD computation.
//!
//! Spec 02: SATD for mode decision cost metric.
//!
//! The Hadamard transform is used to compute SATD (Sum of Absolute
//! Transformed Differences) — a frequency-domain distortion metric
//! that better predicts coded size than SAD.
//!
//! SATD is the primary cost metric used in mode decision.

use archmage::prelude::*;

/// Compute 4x4 Hadamard transform of residual and return SATD.
///
/// SATD = sum of absolute values of Hadamard-transformed residual.
pub fn satd_4x4(src: &[u8], src_stride: usize, ref_: &[u8], ref_stride: usize) -> u32 {
    incant!(
        satd_4x4_impl(src, src_stride, ref_, ref_stride),
        [v3, neon, scalar]
    )
}

/// Compute 8x8 Hadamard transform of residual and return SATD.
pub fn satd_8x8(src: &[u8], src_stride: usize, ref_: &[u8], ref_stride: usize) -> u32 {
    incant!(
        satd_8x8_impl(src, src_stride, ref_, ref_stride),
        [v3, neon, scalar]
    )
}

// --- Scalar implementations ---

fn satd_4x4_impl_scalar(
    _token: ScalarToken,
    src: &[u8],
    src_stride: usize,
    ref_: &[u8],
    ref_stride: usize,
) -> u32 {
    satd_4x4_core(src, src_stride, ref_, ref_stride)
}

fn satd_8x8_impl_scalar(
    _token: ScalarToken,
    src: &[u8],
    src_stride: usize,
    ref_: &[u8],
    ref_stride: usize,
) -> u32 {
    satd_8x8_core(src, src_stride, ref_, ref_stride)
}

// --- AVX2 implementations ---

#[cfg(target_arch = "x86_64")]
#[arcane]
fn satd_4x4_impl_v3(
    _token: Desktop64,
    src: &[u8],
    src_stride: usize,
    ref_: &[u8],
    ref_stride: usize,
) -> u32 {
    // Auto-vectorize with AVX2 enabled — the butterfly add/sub pattern
    // vectorizes well with target_feature(enable = "avx2,fma")
    satd_4x4_core(src, src_stride, ref_, ref_stride)
}

#[cfg(target_arch = "x86_64")]
#[arcane]
fn satd_8x8_impl_v3(
    _token: Desktop64,
    src: &[u8],
    src_stride: usize,
    ref_: &[u8],
    ref_stride: usize,
) -> u32 {
    satd_8x8_core(src, src_stride, ref_, ref_stride)
}

// --- NEON implementations ---

#[cfg(target_arch = "aarch64")]
#[arcane]
fn satd_4x4_impl_neon(
    _token: NeonToken,
    src: &[u8],
    src_stride: usize,
    ref_: &[u8],
    ref_stride: usize,
) -> u32 {
    satd_4x4_core(src, src_stride, ref_, ref_stride)
}

#[cfg(target_arch = "aarch64")]
#[arcane]
fn satd_8x8_impl_neon(
    _token: NeonToken,
    src: &[u8],
    src_stride: usize,
    ref_: &[u8],
    ref_stride: usize,
) -> u32 {
    satd_8x8_core(src, src_stride, ref_, ref_stride)
}

// --- Core algorithm (shared across all dispatch tiers) ---

#[inline]
fn satd_4x4_core(src: &[u8], src_stride: usize, ref_: &[u8], ref_stride: usize) -> u32 {
    // Compute residual
    let mut diff = [0i16; 16];
    for row in 0..4 {
        for col in 0..4 {
            diff[row * 4 + col] =
                src[row * src_stride + col] as i16 - ref_[row * ref_stride + col] as i16;
        }
    }

    // 4x4 Hadamard transform (separable: row then column)
    let mut tmp = [0i16; 16];

    // Row transforms
    for row in 0..4 {
        let i = row * 4;
        let a = diff[i] + diff[i + 1];
        let b = diff[i] - diff[i + 1];
        let c = diff[i + 2] + diff[i + 3];
        let d = diff[i + 2] - diff[i + 3];
        tmp[i] = a + c;
        tmp[i + 1] = b + d;
        tmp[i + 2] = a - c;
        tmp[i + 3] = b - d;
    }

    // Column transforms and accumulate absolute values
    let mut satd: u32 = 0;
    for col in 0..4 {
        let a = tmp[col] + tmp[4 + col];
        let b = tmp[col] - tmp[4 + col];
        let c = tmp[8 + col] + tmp[12 + col];
        let d = tmp[8 + col] - tmp[12 + col];
        satd += (a + c).unsigned_abs() as u32;
        satd += (b + d).unsigned_abs() as u32;
        satd += (a - c).unsigned_abs() as u32;
        satd += (b - d).unsigned_abs() as u32;
    }

    // Normalization: divide by 2 (standard for 4x4 Hadamard)
    (satd + 1) >> 1
}

#[inline]
fn satd_8x8_core(src: &[u8], src_stride: usize, ref_: &[u8], ref_stride: usize) -> u32 {
    // Compute residual
    let mut diff = [0i16; 64];
    for row in 0..8 {
        for col in 0..8 {
            diff[row * 8 + col] =
                src[row * src_stride + col] as i16 - ref_[row * ref_stride + col] as i16;
        }
    }

    // 8x8 Hadamard via butterfly decomposition
    let mut tmp = [0i32; 64];

    // Row transforms (8-point Hadamard butterfly)
    for row in 0..8 {
        let i = row * 8;
        let d = &diff[i..i + 8];

        let a0 = d[0] as i32 + d[4] as i32;
        let a1 = d[1] as i32 + d[5] as i32;
        let a2 = d[2] as i32 + d[6] as i32;
        let a3 = d[3] as i32 + d[7] as i32;
        let a4 = d[0] as i32 - d[4] as i32;
        let a5 = d[1] as i32 - d[5] as i32;
        let a6 = d[2] as i32 - d[6] as i32;
        let a7 = d[3] as i32 - d[7] as i32;

        let b0 = a0 + a2;
        let b1 = a1 + a3;
        let b2 = a0 - a2;
        let b3 = a1 - a3;
        let b4 = a4 + a6;
        let b5 = a5 + a7;
        let b6 = a4 - a6;
        let b7 = a5 - a7;

        tmp[i] = b0 + b1;
        tmp[i + 1] = b0 - b1;
        tmp[i + 2] = b2 + b3;
        tmp[i + 3] = b2 - b3;
        tmp[i + 4] = b4 + b5;
        tmp[i + 5] = b4 - b5;
        tmp[i + 6] = b6 + b7;
        tmp[i + 7] = b6 - b7;
    }

    // Column transforms and accumulate absolute values
    let mut satd: u32 = 0;
    for col in 0..8 {
        let a0 = tmp[col] + tmp[32 + col];
        let a1 = tmp[8 + col] + tmp[40 + col];
        let a2 = tmp[16 + col] + tmp[48 + col];
        let a3 = tmp[24 + col] + tmp[56 + col];
        let a4 = tmp[col] - tmp[32 + col];
        let a5 = tmp[8 + col] - tmp[40 + col];
        let a6 = tmp[16 + col] - tmp[48 + col];
        let a7 = tmp[24 + col] - tmp[56 + col];

        let b0 = a0 + a2;
        let b1 = a1 + a3;
        let b2 = a0 - a2;
        let b3 = a1 - a3;
        let b4 = a4 + a6;
        let b5 = a5 + a7;
        let b6 = a4 - a6;
        let b7 = a5 - a7;

        satd += (b0 + b1).unsigned_abs();
        satd += (b0 - b1).unsigned_abs();
        satd += (b2 + b3).unsigned_abs();
        satd += (b2 - b3).unsigned_abs();
        satd += (b4 + b5).unsigned_abs();
        satd += (b4 - b5).unsigned_abs();
        satd += (b6 + b7).unsigned_abs();
        satd += (b6 - b7).unsigned_abs();
    }

    // Normalization: divide by 4 (standard for 8x8 Hadamard)
    (satd + 2) >> 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn satd_4x4_identical() {
        let block = [128u8; 64];
        assert_eq!(satd_4x4(&block, 8, &block, 8), 0);
    }

    #[test]
    fn satd_4x4_uniform_diff() {
        let src = [110u8; 16];
        let ref_ = [100u8; 16];
        // Uniform difference of 10 across 4x4 block.
        // Hadamard of constant = value * N at DC, 0 elsewhere
        // DC = 10 * 16 = 160, SATD = |160| / 2 = 80
        assert_eq!(satd_4x4(&src, 4, &ref_, 4), 80);
    }

    #[test]
    fn satd_8x8_identical() {
        let block = [128u8; 128];
        assert_eq!(satd_8x8(&block, 16, &block, 16), 0);
    }

    #[test]
    fn satd_8x8_uniform_diff() {
        let src = [110u8; 64];
        let ref_ = [100u8; 64];
        // DC = 10 * 64 = 640, SATD = |640| / 4 = 160
        assert_eq!(satd_8x8(&src, 8, &ref_, 8), 160);
    }

    #[test]
    fn satd_greater_than_zero_for_different() {
        let mut src = [0u8; 64];
        let ref_ = [128u8; 64];
        for (i, v) in src.iter_mut().enumerate() {
            *v = (i * 7 % 256) as u8;
        }
        assert!(satd_4x4(&src, 8, &ref_, 8) > 0);
        assert!(satd_8x8(&src, 8, &ref_, 8) > 0);
    }

    #[test]
    fn satd_geq_sad() {
        // SATD should generally be >= SAD / N for non-trivial patterns
        // (Hadamard preserves energy)
        let mut src = [0u8; 64];
        let ref_ = [0u8; 64];
        for (i, v) in src.iter_mut().enumerate() {
            *v = if i % 2 == 0 { 200 } else { 50 };
        }
        let satd = satd_4x4(&src, 8, &ref_, 8);
        assert!(satd > 0);
    }
}

#[cfg(test)]
mod dispatch_tests {
    use super::*;

    use alloc::vec::Vec;
    use archmage::testing::{CompileTimePolicy, for_each_token_permutation};

    #[test]
    fn satd_4x4_all_dispatch_levels() {
        let src: Vec<u8> = (0..64).map(|i| (i * 3 + 17) as u8).collect();
        let ref_: Vec<u8> = (0..64).map(|i| (i * 5 + 42) as u8).collect();
        let reference_result = satd_4x4(&src, 8, &ref_, 8);

        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            let result = satd_4x4(&src, 8, &ref_, 8);
            assert_eq!(
                result, reference_result,
                "satd_4x4 mismatch at dispatch level"
            );
        });
    }

    #[test]
    fn satd_8x8_all_dispatch_levels() {
        let src: Vec<u8> = (0..64).map(|i| (i * 3 + 17) as u8).collect();
        let ref_: Vec<u8> = (0..64).map(|i| (i * 5 + 42) as u8).collect();
        let reference_result = satd_8x8(&src, 8, &ref_, 8);

        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            let result = satd_8x8(&src, 8, &ref_, 8);
            assert_eq!(
                result, reference_result,
                "satd_8x8 mismatch at dispatch level"
            );
        });
    }
}

// ===========================================================================
// C-exact aom Hadamard kernels for the MD fast loop (MDS0 SATD path).
//
// Verbatim ports of SVT-AV1 `svt_aom_hadamard_8x8_c` / `_16x16_c` /
// `_32x32_c` and `svt_aom_satd_c` (Source/Lib/C_DEFAULT/
// picture_operators_c.c:118-330, common_dsp_rtcd.c:48). These operate on
// int16 residuals and produce the int32 coefficient blocks C's
// `hadamard_path` (product_coding_loop.c:1187) feeds to `svt_aom_satd`.
// Differentially fuzzed vs the C reference in tests/c_parity_hadamard.rs.
// ===========================================================================

/// C `hadamard_col8`: one 8-point Hadamard butterfly over strided input.
/// Output order is the C permutation, not natural order.
fn hadamard_col8(src_diff: &[i16], src_stride: usize, coeff: &mut [i16; 8]) {
    let s = |i: usize| src_diff[i * src_stride] as i32;
    let b0 = s(0) + s(1);
    let b1 = s(0) - s(1);
    let b2 = s(2) + s(3);
    let b3 = s(2) - s(3);
    let b4 = s(4) + s(5);
    let b5 = s(4) - s(5);
    let b6 = s(6) + s(7);
    let b7 = s(6) - s(7);

    let c0 = b0 + b2;
    let c1 = b1 + b3;
    let c2 = b0 - b2;
    let c3 = b1 - b3;
    let c4 = b4 + b6;
    let c5 = b5 + b7;
    let c6 = b4 - b6;
    let c7 = b5 - b7;

    coeff[0] = (c0 + c4) as i16;
    coeff[7] = (c1 + c5) as i16;
    coeff[3] = (c2 + c6) as i16;
    coeff[4] = (c3 + c7) as i16;
    coeff[2] = (c0 - c4) as i16;
    coeff[6] = (c1 - c5) as i16;
    coeff[1] = (c2 - c6) as i16;
    coeff[5] = (c3 - c7) as i16;
}

/// C `hadamard_col4` (picture_operators_c.c:72): 4-point butterfly with a
/// `>> 1` on the first stage.
fn hadamard_col4(src_diff: &[i16], src_stride: usize, coeff: &mut [i16; 4]) {
    let b0 = (src_diff[0] + src_diff[src_stride]) >> 1;
    let b1 = (src_diff[0] - src_diff[src_stride]) >> 1;
    let b2 = (src_diff[2 * src_stride] + src_diff[3 * src_stride]) >> 1;
    let b3 = (src_diff[2 * src_stride] - src_diff[3 * src_stride]) >> 1;
    coeff[0] = b0 + b2;
    coeff[1] = b1 + b3;
    coeff[2] = b0 - b2;
    coeff[3] = b1 - b3;
}

/// C `svt_aom_hadamard_4x4_c` (picture_operators_c.c:85): 2D 4x4 Hadamard
/// (column pass, row pass over the transposed intermediate, then the
/// extra transpose matching the SSE2 kernel's output order).
pub fn aom_hadamard_4x4(src_diff: &[i16], src_stride: usize, coeff: &mut [i32]) {
    let mut buffer = [0i16; 16];
    let mut buffer2 = [0i16; 16];
    for idx in 0..4 {
        let mut out = [0i16; 4];
        hadamard_col4(&src_diff[idx..], src_stride, &mut out);
        buffer[idx * 4..idx * 4 + 4].copy_from_slice(&out);
    }
    for idx in 0..4 {
        let mut out = [0i16; 4];
        hadamard_col4(&buffer[idx..], 4, &mut out);
        buffer2[idx * 4..idx * 4 + 4].copy_from_slice(&out);
    }
    for i in 0..4 {
        for j in 0..4 {
            coeff[i * 4 + j] = buffer2[j * 4 + i] as i32;
        }
    }
}

/// C `svt_aom_hadamard_8x8_c`: 2D 8x8 Hadamard of an int16 residual block
/// (stride `src_stride`) into 64 int32 coefficients. No scaling.
pub fn aom_hadamard_8x8(src_diff: &[i16], src_stride: usize, coeff: &mut [i32]) {
    let mut buffer = [0i16; 64];
    let mut buffer2 = [0i16; 64];
    // Column pass: one butterfly per column, walking columns left→right.
    for idx in 0..8 {
        let col = &src_diff[idx..];
        let mut out = [0i16; 8];
        hadamard_col8(col, src_stride, &mut out);
        buffer[idx * 8..idx * 8 + 8].copy_from_slice(&out);
    }
    // Row pass over the transposed intermediate.
    for idx in 0..8 {
        let mut out = [0i16; 8];
        hadamard_col8(&buffer[idx..], 8, &mut out);
        buffer2[idx * 8..idx * 8 + 8].copy_from_slice(&out);
    }
    for idx in 0..64 {
        coeff[idx] = buffer2[idx] as i32;
    }
}

// ---------------------------------------------------------------------------
// 16x16 / 32x32: ported from the AVX2 kernels, NOT the `_c` references.
//
// `svt_aom_hadamard_{16x16,32x32}` are RTCD function POINTERS that the encoder
// binds to the AVX2 implementations on any AVX2 host
// (`SET_AVX2(svt_aom_hadamard_32x32, _c, _avx2)`, common_dsp_rtcd.c:1047-1048),
// and the AVX2 kernels are NOT equivalent to the `_c` ones once the residual
// leaves the 8-bit range they were written for (their own comment: "src_diff:
// 9 bit, dynamic range [-255, 255]"):
//
//   * `_c` carries the 8x8 sub-results into the 16x16 cross-combine as
//     `int32_t` and the 16x16 sub-results into the 32x32 combine as `int32_t`;
//     nothing after the 8x8 stage can wrap.
//   * `_avx2` keeps BOTH of those stages in 16-bit lanes: the 16x16 combine is
//     `_mm256_{add,sub}_epi16` + `_mm256_srai_epi16` (wrapping), and
//     `svt_aom_hadamard_32x32_avx2` buffers its four 16x16 sub-transforms in an
//     `int16_t temp_coeff[32*32]` (`is_final = 0`,
//     pic_operators_intrin_avx2.c:1721-1732) before sign-extending to 32-bit,
//     doing the `>> 2` in 32-bit, SATURATING back to 16-bit
//     (`_mm256_packs_epi32`) and finishing with wrapping 16-bit add/sub.
//
// At 8-bit residuals the 16x16 stage spans [-32640, 32640] and the post-shift
// 32x32 operands span [-16320, 16320], so no wrap or saturation is reachable
// and the two kernels agree bit-for-bit — which is why the 8-bit identity
// gates are unaffected by porting the AVX2 semantics. At 10-bit residuals
// (the bd10 MD fast loop, task #94) the 16x16 stage reaches ~+/-130560 and the
// AVX2 kernel wraps where `_c` does not, so ONLY the AVX2 form reproduces the
// encoder's SATD. Pinned against both references in tests/c_parity_hadamard.rs
// (`_c` over the 8-bit range, `_avx2` over the 8-bit AND 10-bit ranges).
// ---------------------------------------------------------------------------

/// `svt_aom_hadamard_16x16_avx2`: four 8x8 sub-transforms + a cross-combine
/// carried in WRAPPING 16-bit lanes (`_mm256_{add,sub}_epi16`,
/// `_mm256_srai_epi16`), widened to `int32` on store (`store_tran_low`).
pub fn aom_hadamard_16x16(src_diff: &[i16], src_stride: usize, coeff: &mut [i32]) {
    for idx in 0..4usize {
        let off = (idx >> 1) * 8 * src_stride + (idx & 1) * 8;
        aom_hadamard_8x8(&src_diff[off..], src_stride, &mut coeff[idx * 64..]);
    }
    for i in 0..64usize {
        // The 8x8 stage already produced int16-valued coefficients (C's
        // `buffer2` / the AVX2 `temp_coeff` are both int16), so reading them
        // back as i16 is lossless and matches the AVX2 lane width.
        let a0 = coeff[i] as i16;
        let a1 = coeff[i + 64] as i16;
        let a2 = coeff[i + 128] as i16;
        let a3 = coeff[i + 192] as i16;
        let b0 = a0.wrapping_add(a1) >> 1;
        let b1 = a0.wrapping_sub(a1) >> 1;
        let b2 = a2.wrapping_add(a3) >> 1;
        let b3 = a2.wrapping_sub(a3) >> 1;
        coeff[i] = b0.wrapping_add(b2) as i32;
        coeff[i + 64] = b1.wrapping_add(b3) as i32;
        coeff[i + 128] = b0.wrapping_sub(b2) as i32;
        coeff[i + 192] = b1.wrapping_sub(b3) as i32;
    }
}

/// `svt_aom_hadamard_32x32_avx2`: four 16x16 sub-transforms buffered as
/// `int16` (`is_final = 0`), then sign-extended to 32-bit for the pairwise
/// sum/difference and `>> 2`, SATURATED back to 16-bit (`_mm256_packs_epi32`)
/// and combined with wrapping 16-bit add/sub before the 32-bit store.
pub fn aom_hadamard_32x32(src_diff: &[i16], src_stride: usize, coeff: &mut [i32]) {
    for idx in 0..4usize {
        let off = (idx >> 1) * 16 * src_stride + (idx & 1) * 16;
        aom_hadamard_16x16(&src_diff[off..], src_stride, &mut coeff[idx * 256..]);
    }
    for i in 0..256usize {
        // `temp_coeff` is int16: the 16x16 stage is read back through 16-bit
        // lanes and sign-extended (the AVX2 `sign_extend_16bit_to_32bit`).
        let a0 = coeff[i] as i16 as i32;
        let a1 = coeff[i + 256] as i16 as i32;
        let a2 = coeff[i + 512] as i16 as i32;
        let a3 = coeff[i + 768] as i16 as i32;
        // 32-bit add/sub then arithmetic `>> 2` (`_mm256_srai_epi32`).
        let b0 = (a0 + a1) >> 2;
        let b1 = (a0 - a1) >> 2;
        let b2 = (a2 + a3) >> 2;
        let b3 = (a2 - a3) >> 2;
        // `_mm256_packs_epi32`: SATURATING 32 -> 16 narrowing.
        let sat = |v: i32| v.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        let (b0, b1, b2, b3) = (sat(b0), sat(b1), sat(b2), sat(b3));
        // `_mm256_{add,sub}_epi16`: WRAPPING 16-bit, then sign-extended store.
        coeff[i] = b0.wrapping_add(b2) as i32;
        coeff[i + 256] = b1.wrapping_add(b3) as i32;
        coeff[i + 512] = b0.wrapping_sub(b2) as i32;
        coeff[i + 768] = b1.wrapping_sub(b3) as i32;
    }
}

/// C `svt_aom_satd_c`: plain sum of absolute int32 coefficients.
pub fn aom_satd(coeff: &[i32]) -> i32 {
    let mut satd: i32 = 0;
    for &c in coeff {
        satd += c.abs();
    }
    satd
}
