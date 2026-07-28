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
    // Sum via the vpaddlq/vpadalq widening chain; sum-of-squares via vmull_u8
    // (u8*u8 fits u16) drained into u32 lanes every iteration.
    //
    // Overflow: a u16 lane holds at most 255*255 = 65025, so squares must NOT
    // accumulate in u16. Draining to u32 per 16-byte chunk keeps the largest
    // block used here (128x128 = 16384 px, worst case 65025*16384 = 1.07e9)
    // inside u32's 4.29e9.
    let mut sum_acc = vdupq_n_u32(0);
    let mut sq_acc = vdupq_n_u32(0);
    let mut tail_sum: u64 = 0;
    let mut tail_sq: u64 = 0;

    for row in 0..height {
        let off = row * src_stride;
        let mut col = 0;
        while col + 16 <= width {
            let c: &[u8; 16] = src[off + col..off + col + 16].try_into().unwrap();
            let v = vld1q_u8(c);
            sum_acc = vpadalq_u16(sum_acc, vpaddlq_u8(v));
            let lo = vget_low_u8(v);
            let hi = vget_high_u8(v);
            sq_acc = vpadalq_u16(sq_acc, vmull_u8(lo, lo));
            sq_acc = vpadalq_u16(sq_acc, vmull_u8(hi, hi));
            col += 16;
        }
        while col < width {
            let v = src[off + col] as u64;
            tail_sum += v;
            tail_sq += v * v;
            col += 1;
        }
    }

    let sum = vaddvq_u32(sum_acc) as u64 + tail_sum;
    let sum_sq = vaddvq_u32(sq_acc) as u64 + tail_sq;
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
    // |a-b| via vabdq_u8 (exact for u8), squared with vmull_u8 into u16 and
    // drained to u32 each chunk — squares reach 65025 so they must not
    // accumulate in u16. The u32 accumulator is drained to u64 per ROW, so an
    // arbitrarily tall block cannot overflow it.
    let mut total: u64 = 0;
    let mut tail: u64 = 0;

    for row in 0..height {
        let s_off = row * src_stride;
        let r_off = row * ref_stride;
        let mut col = 0;
        let mut acc = vdupq_n_u32(0);

        while col + 16 <= width {
            let a: &[u8; 16] = src[s_off + col..s_off + col + 16].try_into().unwrap();
            let b: &[u8; 16] = ref_[r_off + col..r_off + col + 16].try_into().unwrap();
            let d = vabdq_u8(vld1q_u8(a), vld1q_u8(b));
            let lo = vget_low_u8(d);
            let hi = vget_high_u8(d);
            acc = vpadalq_u16(acc, vmull_u8(lo, lo));
            acc = vpadalq_u16(acc, vmull_u8(hi, hi));
            col += 16;
        }
        total += vaddvq_u32(acc) as u64;

        while col < width {
            let diff = src[s_off + col] as i32 - ref_[r_off + col] as i32;
            tail += (diff * diff) as u64;
            col += 1;
        }
    }
    total + tail
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
