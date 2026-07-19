//! Variance and SSE (Sum of Squared Errors) computation.
//!
//! Spec 13 (segmentation.md): Variance for adaptive quantization.
//!
//! Variance is used for adaptive quantization, activity masking,
//! and screen content detection. SSE is the primary distortion metric
//! for rate-distortion optimization.

use archmage::prelude::*;

/// Compute variance of an 8-bit pixel block.
///
/// Returns (variance, mean) where variance = E[x²] - E[x]² scaled by N.
/// More precisely: variance = sum((x - mean)²) = sum(x²) - sum(x)²/N
pub fn variance(src: &[u8], src_stride: usize, width: usize, height: usize) -> (u64, u32) {
    incant!(
        variance_impl(src, src_stride, width, height),
        [v3, neon, scalar]
    )
}

/// Compute SSE between two blocks of 8-bit pixels.
pub fn sse(
    src: &[u8],
    src_stride: usize,
    ref_: &[u8],
    ref_stride: usize,
    width: usize,
    height: usize,
) -> u64 {
    incant!(
        sse_impl(src, src_stride, ref_, ref_stride, width, height),
        [v3, neon, scalar]
    )
}

// --- Scalar implementations ---

fn variance_impl_scalar(
    _token: ScalarToken,
    src: &[u8],
    src_stride: usize,
    width: usize,
    height: usize,
) -> (u64, u32) {
    let mut sum: u64 = 0;
    let mut sum_sq: u64 = 0;
    for row in 0..height {
        let offset = row * src_stride;
        for col in 0..width {
            let v = src[offset + col] as u64;
            sum += v;
            sum_sq += v * v;
        }
    }
    let n = (width * height) as u64;
    let variance = sum_sq * n - sum * sum;
    let mean = (sum / n) as u32;
    (variance, mean)
}

fn sse_impl_scalar(
    _token: ScalarToken,
    src: &[u8],
    src_stride: usize,
    ref_: &[u8],
    ref_stride: usize,
    width: usize,
    height: usize,
) -> u64 {
    let mut sse: u64 = 0;
    for row in 0..height {
        let s_off = row * src_stride;
        let r_off = row * ref_stride;
        for col in 0..width {
            let diff = src[s_off + col] as i32 - ref_[r_off + col] as i32;
            sse += (diff * diff) as u64;
        }
    }
    sse
}

// --- AVX2 implementations ---

#[cfg(target_arch = "x86_64")]
#[arcane]
fn variance_impl_v3(
    _token: Desktop64,
    src: &[u8],
    src_stride: usize,
    width: usize,
    height: usize,
) -> (u64, u32) {
    // Auto-vectorize with AVX2 enabled — compiler does well here
    let mut sum: u64 = 0;
    let mut sum_sq: u64 = 0;
    for row in 0..height {
        let offset = row * src_stride;
        for col in 0..width {
            let v = src[offset + col] as u64;
            sum += v;
            sum_sq += v * v;
        }
    }
    let n = (width * height) as u64;
    let variance = sum_sq * n - sum * sum;
    let mean = (sum / n) as u32;
    (variance, mean)
}

#[cfg(target_arch = "x86_64")]
#[arcane]
fn sse_impl_v3(
    _token: Desktop64,
    src: &[u8],
    src_stride: usize,
    ref_: &[u8],
    ref_stride: usize,
    width: usize,
    height: usize,
) -> u64 {
    let mut sse: u64 = 0;
    for row in 0..height {
        let s_off = row * src_stride;
        let r_off = row * ref_stride;
        for col in 0..width {
            let diff = src[s_off + col] as i32 - ref_[r_off + col] as i32;
            sse += (diff * diff) as u64;
        }
    }
    sse
}

// --- NEON implementations ---

#[cfg(target_arch = "aarch64")]
#[arcane]
fn variance_impl_neon(
    _token: NeonToken,
    src: &[u8],
    src_stride: usize,
    width: usize,
    height: usize,
) -> (u64, u32) {
    let mut sum: u64 = 0;
    let mut sum_sq: u64 = 0;
    for row in 0..height {
        let offset = row * src_stride;
        for col in 0..width {
            let v = src[offset + col] as u64;
            sum += v;
            sum_sq += v * v;
        }
    }
    let n = (width * height) as u64;
    let variance = sum_sq * n - sum * sum;
    let mean = (sum / n) as u32;
    (variance, mean)
}

#[cfg(target_arch = "aarch64")]
#[arcane]
fn sse_impl_neon(
    _token: NeonToken,
    src: &[u8],
    src_stride: usize,
    ref_: &[u8],
    ref_stride: usize,
    width: usize,
    height: usize,
) -> u64 {
    let mut sse: u64 = 0;
    for row in 0..height {
        let s_off = row * src_stride;
        let r_off = row * ref_stride;
        for col in 0..width {
            let diff = src[s_off + col] as i32 - ref_[r_off + col] as i32;
            sse += (diff * diff) as u64;
        }
    }
    sse
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variance_uniform_block() {
        let block = [128u8; 64];
        let (var, mean) = variance(&block, 8, 8, 8);
        assert_eq!(var, 0, "uniform block should have zero variance");
        assert_eq!(mean, 128);
    }

    #[test]
    fn variance_known_values() {
        // 4x4 block: 0,1,2,...,15
        let mut block = [0u8; 16];
        for (i, b) in block.iter_mut().enumerate() {
            *b = i as u8;
        }
        let (var, _mean) = variance(&block, 4, 4, 4);
        // sum = 120, sum_sq = 1240, n = 16
        // var = 1240 * 16 - 120 * 120 = 19840 - 14400 = 5440
        assert_eq!(var, 5440);
    }

    #[test]
    fn sse_identical_blocks() {
        let block = [42u8; 64];
        assert_eq!(sse(&block, 8, &block, 8, 8, 8), 0);
    }

    #[test]
    fn sse_known_value() {
        let src = [10u8; 16];
        let ref_ = [20u8; 16];
        // Each pixel diff = 10, diff² = 100, 16 pixels => SSE = 1600
        assert_eq!(sse(&src, 4, &ref_, 4, 4, 4), 1600);
    }

    #[test]
    fn sse_max_difference() {
        let src = [0u8; 16];
        let ref_ = [255u8; 16];
        assert_eq!(sse(&src, 4, &ref_, 4, 4, 4), 255 * 255 * 16);
    }
}

#[cfg(test)]
mod dispatch_tests {
    use super::*;

    use alloc::vec::Vec;
    use archmage::testing::{CompileTimePolicy, for_each_token_permutation};

    #[test]
    fn variance_all_dispatch_levels() {
        let block: Vec<u8> = (0..64).map(|i| (i * 3 + 17) as u8).collect();
        let reference_result = variance(&block, 8, 8, 8);

        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            let result = variance(&block, 8, 8, 8);
            assert_eq!(
                result, reference_result,
                "variance mismatch at dispatch level"
            );
        });
    }

    #[test]
    fn sse_all_dispatch_levels() {
        let src: Vec<u8> = (0..64).map(|i| (i * 3 + 17) as u8).collect();
        let ref_: Vec<u8> = (0..64).map(|i| (i * 5 + 42) as u8).collect();
        let reference_result = sse(&src, 8, &ref_, 8, 8, 8);

        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            let result = sse(&src, 8, &ref_, 8, 8, 8);
            assert_eq!(result, reference_result, "sse mismatch at dispatch level");
        });
    }
}
