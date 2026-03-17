//! Sum of Absolute Differences (SAD) computation.
//!
//! Spec 02 (motion-estimation.md): SAD for ME distortion metric.
//!
//! SAD is the most-called function in motion estimation — it measures
//! the distortion between a source block and a reference block.
//!
//! Ported from SVT-AV1's sad_calculation functions.
//! SIMD implementations use archmage for dispatch.

use archmage::prelude::*;

/// Compute SAD between two blocks of 8-bit pixels.
///
/// # Arguments
/// * `src` - Source block pixels (row-major)
/// * `src_stride` - Distance between source rows in bytes
/// * `ref_` - Reference block pixels (row-major)
/// * `ref_stride` - Distance between reference rows in bytes
/// * `width` - Block width in pixels
/// * `height` - Block height in pixels
pub fn sad(
    src: &[u8],
    src_stride: usize,
    ref_: &[u8],
    ref_stride: usize,
    width: usize,
    height: usize,
) -> u32 {
    incant!(
        sad_impl(src, src_stride, ref_, ref_stride, width, height),
        [v3, neon, scalar]
    )
}

/// SAD for specific common block sizes — 8x8.
pub fn sad_8x8(src: &[u8], src_stride: usize, ref_: &[u8], ref_stride: usize) -> u32 {
    sad(src, src_stride, ref_, ref_stride, 8, 8)
}

/// SAD for specific common block sizes — 16x16.
pub fn sad_16x16(src: &[u8], src_stride: usize, ref_: &[u8], ref_stride: usize) -> u32 {
    sad(src, src_stride, ref_, ref_stride, 16, 16)
}

/// SAD for specific common block sizes — 32x32.
pub fn sad_32x32(src: &[u8], src_stride: usize, ref_: &[u8], ref_stride: usize) -> u32 {
    sad(src, src_stride, ref_, ref_stride, 32, 32)
}

/// SAD for specific common block sizes — 64x64.
pub fn sad_64x64(src: &[u8], src_stride: usize, ref_: &[u8], ref_stride: usize) -> u32 {
    sad(src, src_stride, ref_, ref_stride, 64, 64)
}

// --- Scalar implementation ---

fn sad_impl_scalar(
    _token: ScalarToken,
    src: &[u8],
    src_stride: usize,
    ref_: &[u8],
    ref_stride: usize,
    width: usize,
    height: usize,
) -> u32 {
    let mut sum: u32 = 0;
    for row in 0..height {
        let src_row = row * src_stride;
        let ref_row = row * ref_stride;
        for col in 0..width {
            let s = src[src_row + col] as i32;
            let r = ref_[ref_row + col] as i32;
            sum += (s - r).unsigned_abs();
        }
    }
    sum
}

// --- AVX2 implementation ---

#[cfg(target_arch = "x86_64")]
#[arcane]
fn sad_impl_v3(
    _token: Desktop64,
    src: &[u8],
    src_stride: usize,
    ref_: &[u8],
    ref_stride: usize,
    width: usize,
    height: usize,
) -> u32 {
    // For widths >= 32, use full AVX2 _mm256_sad_epu8
    // For widths >= 16, use SSE2 _mm_sad_epu8
    // For smaller widths, fall through to scalar
    //
    // Note: This is a starting implementation. Will be optimized further
    // with dedicated per-size kernels after parity tests pass.

    if width >= 32 {
        sad_avx2_wide(_token, src, src_stride, ref_, ref_stride, width, height)
    } else if width >= 16 {
        sad_sse2_16(_token, src, src_stride, ref_, ref_stride, width, height)
    } else {
        // Scalar fallback for small blocks (4x4, 8x8, etc.)
        // The compiler will auto-vectorize this with AVX2 enabled
        let mut sum: u32 = 0;
        for row in 0..height {
            let src_row = row * src_stride;
            let ref_row = row * ref_stride;
            for col in 0..width {
                let s = src[src_row + col] as i32;
                let r = ref_[ref_row + col] as i32;
                sum += (s - r).unsigned_abs();
            }
        }
        sum
    }
}

#[cfg(target_arch = "x86_64")]
#[rite]
fn sad_avx2_wide(
    _token: Desktop64,
    src: &[u8],
    src_stride: usize,
    ref_: &[u8],
    ref_stride: usize,
    width: usize,
    height: usize,
) -> u32 {
    let mut total_sad: u64 = 0;

    for row in 0..height {
        let src_offset = row * src_stride;
        let ref_offset = row * ref_stride;
        let mut col = 0;

        while col + 32 <= width {
            let s_arr: &[u8; 32] = src[src_offset + col..src_offset + col + 32]
                .try_into()
                .unwrap();
            let r_arr: &[u8; 32] = ref_[ref_offset + col..ref_offset + col + 32]
                .try_into()
                .unwrap();
            let s = _mm256_loadu_si256(s_arr);
            let r = _mm256_loadu_si256(r_arr);
            let sad = _mm256_sad_epu8(s, r);

            // Extract the 4 u64 partial sums
            let lo = _mm256_castsi256_si128(sad);
            let hi = _mm256_extracti128_si256::<1>(sad);
            let sum128 = _mm_add_epi64(lo, hi);
            let hi64 = _mm_srli_si128::<8>(sum128);
            let sum64 = _mm_add_epi64(sum128, hi64);
            total_sad += _mm_cvtsi128_si64(sum64) as u64;

            col += 32;
        }

        // Handle remaining columns with scalar
        while col < width {
            let s = src[src_offset + col] as i32;
            let r = ref_[ref_offset + col] as i32;
            total_sad += (s - r).unsigned_abs() as u64;
            col += 1;
        }
    }

    total_sad as u32
}

#[cfg(target_arch = "x86_64")]
#[rite]
fn sad_sse2_16(
    _token: Desktop64,
    src: &[u8],
    src_stride: usize,
    ref_: &[u8],
    ref_stride: usize,
    width: usize,
    height: usize,
) -> u32 {
    let mut total_sad: u64 = 0;

    for row in 0..height {
        let src_offset = row * src_stride;
        let ref_offset = row * ref_stride;
        let mut col = 0;

        while col + 16 <= width {
            let s_arr: &[u8; 16] = src[src_offset + col..src_offset + col + 16]
                .try_into()
                .unwrap();
            let r_arr: &[u8; 16] = ref_[ref_offset + col..ref_offset + col + 16]
                .try_into()
                .unwrap();
            let s = _mm_loadu_si128(s_arr);
            let r = _mm_loadu_si128(r_arr);
            let sad = _mm_sad_epu8(s, r);

            let hi64 = _mm_srli_si128::<8>(sad);
            let sum64 = _mm_add_epi64(sad, hi64);
            total_sad += _mm_cvtsi128_si64(sum64) as u64;

            col += 16;
        }

        while col < width {
            let s = src[src_offset + col] as i32;
            let r = ref_[ref_offset + col] as i32;
            total_sad += (s - r).unsigned_abs() as u64;
            col += 1;
        }
    }

    total_sad as u32
}

// --- NEON implementation ---

#[cfg(target_arch = "aarch64")]
#[arcane]
fn sad_impl_neon(
    _token: NeonToken,
    src: &[u8],
    src_stride: usize,
    ref_: &[u8],
    ref_stride: usize,
    width: usize,
    height: usize,
) -> u32 {
    // NEON: use vabdl_u8 + vpaddlq for absolute difference and accumulate
    // Starting with scalar-with-autovectorize; will add explicit NEON intrinsics
    let mut sum: u32 = 0;
    for row in 0..height {
        let src_row = row * src_stride;
        let ref_row = row * ref_stride;
        for col in 0..width {
            let s = src[src_row + col] as i32;
            let r = ref_[ref_row + col] as i32;
            sum += (s - r).unsigned_abs();
        }
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sad_zero_for_identical() {
        let block = [128u8; 64 * 64];
        assert_eq!(sad(&block, 64, &block, 64, 8, 8), 0);
        assert_eq!(sad(&block, 64, &block, 64, 16, 16), 0);
        assert_eq!(sad(&block, 64, &block, 64, 32, 32), 0);
        assert_eq!(sad(&block, 64, &block, 64, 64, 64), 0);
    }

    #[test]
    fn sad_known_value_4x4() {
        let src = [10u8; 16];
        let ref_ = [20u8; 16];
        // Each pixel differs by 10, 16 pixels total => SAD = 160
        assert_eq!(sad(&src, 4, &ref_, 4, 4, 4), 160);
    }

    #[test]
    fn sad_known_value_8x8() {
        let mut src = [0u8; 64];
        let mut ref_ = [0u8; 64];
        for i in 0..64 {
            src[i] = (i * 3) as u8;
            ref_[i] = (i * 3 + 1) as u8;
        }
        // Each pixel differs by 1, 64 pixels => SAD = 64
        assert_eq!(sad(&src, 8, &ref_, 8, 8, 8), 64);
    }

    #[test]
    fn sad_max_difference() {
        let src = [0u8; 16];
        let ref_ = [255u8; 16];
        assert_eq!(sad(&src, 4, &ref_, 4, 4, 4), 255 * 16);
    }

    #[test]
    fn sad_with_stride() {
        // Source is embedded in a larger buffer with stride 16
        let mut src = [0u8; 16 * 4];
        let mut ref_ = [0u8; 16 * 4];
        for row in 0..4 {
            for col in 0..4 {
                src[row * 16 + col] = 100;
                ref_[row * 16 + col] = 110;
            }
        }
        assert_eq!(sad(&src, 16, &ref_, 16, 4, 4), 10 * 16);
    }

    #[test]
    fn sad_convenience_functions() {
        let block = [42u8; 64 * 64];
        assert_eq!(sad_8x8(&block, 64, &block, 64), 0);
        assert_eq!(sad_16x16(&block, 64, &block, 64), 0);
        assert_eq!(sad_32x32(&block, 64, &block, 64), 0);
        assert_eq!(sad_64x64(&block, 64, &block, 64), 0);
    }
}

#[cfg(test)]
mod dispatch_tests {
    use super::*;

    use alloc::vec::Vec;
    use archmage::testing::{CompileTimePolicy, for_each_token_permutation};

    #[test]
    fn sad_all_dispatch_levels() {
        let src: Vec<u8> = (0..256).map(|i| (i * 7 + 13) as u8).collect();
        let ref_: Vec<u8> = (0..256).map(|i| (i * 11 + 29) as u8).collect();

        for size in [(4, 4), (8, 8), (16, 16)] {
            let reference = sad(&src, 16, &ref_, 16, size.0, size.1);
            let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
                let result = sad(&src, 16, &ref_, 16, size.0, size.1);
                assert_eq!(result, reference, "sad {}x{} mismatch", size.0, size.1);
            });
        }
    }
}
