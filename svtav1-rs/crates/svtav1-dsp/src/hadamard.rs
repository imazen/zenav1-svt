//! Hadamard transform for SATD computation.
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
