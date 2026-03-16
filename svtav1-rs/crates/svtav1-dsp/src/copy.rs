//! Block copy and blend operations.
//!
//! Used extensively in prediction: copying reference blocks, averaging
//! compound predictions, and blending with masks.

use archmage::prelude::*;

/// Copy a rectangular block of 8-bit pixels.
pub fn block_copy(
    dst: &mut [u8],
    dst_stride: usize,
    src: &[u8],
    src_stride: usize,
    width: usize,
    height: usize,
) {
    incant!(
        block_copy_impl(dst, dst_stride, src, src_stride, width, height),
        [v3, neon, scalar]
    )
}

fn block_copy_impl_scalar(
    _token: ScalarToken,
    dst: &mut [u8],
    dst_stride: usize,
    src: &[u8],
    src_stride: usize,
    width: usize,
    height: usize,
) {
    block_copy_inner(dst, dst_stride, src, src_stride, width, height);
}

#[cfg(target_arch = "x86_64")]
#[arcane]
fn block_copy_impl_v3(
    _token: Desktop64,
    dst: &mut [u8],
    dst_stride: usize,
    src: &[u8],
    src_stride: usize,
    width: usize,
    height: usize,
) {
    block_copy_inner(dst, dst_stride, src, src_stride, width, height);
}

#[cfg(target_arch = "aarch64")]
#[arcane]
fn block_copy_impl_neon(
    _token: NeonToken,
    dst: &mut [u8],
    dst_stride: usize,
    src: &[u8],
    src_stride: usize,
    width: usize,
    height: usize,
) {
    block_copy_inner(dst, dst_stride, src, src_stride, width, height);
}

#[inline]
fn block_copy_inner(
    dst: &mut [u8],
    dst_stride: usize,
    src: &[u8],
    src_stride: usize,
    width: usize,
    height: usize,
) {
    for row in 0..height {
        let d_off = row * dst_stride;
        let s_off = row * src_stride;
        dst[d_off..d_off + width].copy_from_slice(&src[s_off..s_off + width]);
    }
}

/// Average two blocks of 8-bit pixels (compound prediction blend).
///
/// dst[i] = (a[i] + b[i] + 1) >> 1
pub fn block_average(
    dst: &mut [u8],
    dst_stride: usize,
    a: &[u8],
    a_stride: usize,
    b: &[u8],
    b_stride: usize,
    width: usize,
    height: usize,
) {
    incant!(
        block_average_impl(dst, dst_stride, a, a_stride, b, b_stride, width, height),
        [v3, neon, scalar]
    )
}

fn block_average_impl_scalar(
    _token: ScalarToken,
    dst: &mut [u8],
    dst_stride: usize,
    a: &[u8],
    a_stride: usize,
    b: &[u8],
    b_stride: usize,
    width: usize,
    height: usize,
) {
    block_average_inner(dst, dst_stride, a, a_stride, b, b_stride, width, height);
}

#[cfg(target_arch = "x86_64")]
#[arcane]
fn block_average_impl_v3(
    _token: Desktop64,
    dst: &mut [u8],
    dst_stride: usize,
    a: &[u8],
    a_stride: usize,
    b: &[u8],
    b_stride: usize,
    width: usize,
    height: usize,
) {
    block_average_inner(dst, dst_stride, a, a_stride, b, b_stride, width, height);
}

#[cfg(target_arch = "aarch64")]
#[arcane]
fn block_average_impl_neon(
    _token: NeonToken,
    dst: &mut [u8],
    dst_stride: usize,
    a: &[u8],
    a_stride: usize,
    b: &[u8],
    b_stride: usize,
    width: usize,
    height: usize,
) {
    block_average_inner(dst, dst_stride, a, a_stride, b, b_stride, width, height);
}

#[inline]
fn block_average_inner(
    dst: &mut [u8],
    dst_stride: usize,
    a: &[u8],
    a_stride: usize,
    b: &[u8],
    b_stride: usize,
    width: usize,
    height: usize,
) {
    for row in 0..height {
        let d_off = row * dst_stride;
        let a_off = row * a_stride;
        let b_off = row * b_stride;
        for col in 0..width {
            let va = a[a_off + col] as u16;
            let vb = b[b_off + col] as u16;
            dst[d_off + col] = ((va + vb + 1) >> 1) as u8;
        }
    }
}

/// Weighted blend of two blocks using a per-pixel mask.
///
/// dst[i] = (a[i] * mask[i] + b[i] * (64 - mask[i]) + 32) >> 6
///
/// mask values are in range [0, 64] (AOM_BLEND_A64_MAX_ALPHA).
pub fn block_blend(
    dst: &mut [u8],
    dst_stride: usize,
    a: &[u8],
    a_stride: usize,
    b: &[u8],
    b_stride: usize,
    mask: &[u8],
    mask_stride: usize,
    width: usize,
    height: usize,
) {
    incant!(
        block_blend_impl(
            dst,
            dst_stride,
            a,
            a_stride,
            b,
            b_stride,
            mask,
            mask_stride,
            width,
            height
        ),
        [v3, neon, scalar]
    )
}

fn block_blend_impl_scalar(
    _token: ScalarToken,
    dst: &mut [u8],
    dst_stride: usize,
    a: &[u8],
    a_stride: usize,
    b: &[u8],
    b_stride: usize,
    mask: &[u8],
    mask_stride: usize,
    width: usize,
    height: usize,
) {
    block_blend_inner(
        dst,
        dst_stride,
        a,
        a_stride,
        b,
        b_stride,
        mask,
        mask_stride,
        width,
        height,
    );
}

#[cfg(target_arch = "x86_64")]
#[arcane]
fn block_blend_impl_v3(
    _token: Desktop64,
    dst: &mut [u8],
    dst_stride: usize,
    a: &[u8],
    a_stride: usize,
    b: &[u8],
    b_stride: usize,
    mask: &[u8],
    mask_stride: usize,
    width: usize,
    height: usize,
) {
    block_blend_inner(
        dst,
        dst_stride,
        a,
        a_stride,
        b,
        b_stride,
        mask,
        mask_stride,
        width,
        height,
    );
}

#[cfg(target_arch = "aarch64")]
#[arcane]
fn block_blend_impl_neon(
    _token: NeonToken,
    dst: &mut [u8],
    dst_stride: usize,
    a: &[u8],
    a_stride: usize,
    b: &[u8],
    b_stride: usize,
    mask: &[u8],
    mask_stride: usize,
    width: usize,
    height: usize,
) {
    block_blend_inner(
        dst,
        dst_stride,
        a,
        a_stride,
        b,
        b_stride,
        mask,
        mask_stride,
        width,
        height,
    );
}

#[inline]
fn block_blend_inner(
    dst: &mut [u8],
    dst_stride: usize,
    a: &[u8],
    a_stride: usize,
    b: &[u8],
    b_stride: usize,
    mask: &[u8],
    mask_stride: usize,
    width: usize,
    height: usize,
) {
    for row in 0..height {
        let d_off = row * dst_stride;
        let a_off = row * a_stride;
        let b_off = row * b_stride;
        let m_off = row * mask_stride;
        for col in 0..width {
            let va = a[a_off + col] as u32;
            let vb = b[b_off + col] as u32;
            let w = mask[m_off + col] as u32;
            // AOM_BLEND_A64: (a*w + b*(64-w) + 32) >> 6
            dst[d_off + col] = ((va * w + vb * (64 - w) + 32) >> 6) as u8;
        }
    }
}

/// Distance-weighted blend of two blocks.
///
/// dst[i] = (a[i] * wt0 + b[i] * wt1 + (1 << (shift-1))) >> shift
pub fn block_dist_wtd_blend(
    dst: &mut [u8],
    dst_stride: usize,
    a: &[u8],
    a_stride: usize,
    b: &[u8],
    b_stride: usize,
    wt0: u32,
    wt1: u32,
    width: usize,
    height: usize,
) {
    const SHIFT: u32 = 4;
    let round = 1u32 << (SHIFT - 1);
    for row in 0..height {
        let d_off = row * dst_stride;
        let a_off = row * a_stride;
        let b_off = row * b_stride;
        for col in 0..width {
            let va = a[a_off + col] as u32;
            let vb = b[b_off + col] as u32;
            dst[d_off + col] = ((va * wt0 + vb * wt1 + round) >> SHIFT) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_basic() {
        let src = [1u8, 2, 3, 4, 5, 6, 7, 8, 9];
        let mut dst = [0u8; 9];
        block_copy(&mut dst, 3, &src, 3, 3, 3);
        assert_eq!(dst, src);
    }

    #[test]
    fn copy_with_stride() {
        let src = [1u8, 2, 0, 0, 3, 4, 0, 0, 5, 6, 0, 0];
        let mut dst = [0u8; 12];
        block_copy(&mut dst, 4, &src, 4, 2, 3);
        assert_eq!(&dst[..2], &[1, 2]);
        assert_eq!(&dst[4..6], &[3, 4]);
        assert_eq!(&dst[8..10], &[5, 6]);
    }

    #[test]
    fn average_basic() {
        let a = [100u8; 16];
        let b = [200u8; 16];
        let mut dst = [0u8; 16];
        block_average(&mut dst, 4, &a, 4, &b, 4, 4, 4);
        // (100 + 200 + 1) >> 1 = 150
        assert!(dst.iter().all(|&v| v == 150));
    }

    #[test]
    fn blend_uniform_mask() {
        let a = [100u8; 4];
        let b = [200u8; 4];
        let mask = [32u8; 4]; // 50% blend
        let mut dst = [0u8; 4];
        block_blend(&mut dst, 2, &a, 2, &b, 2, &mask, 2, 2, 2);
        // (100*32 + 200*32 + 32) >> 6 = (3200 + 6400 + 32) >> 6 = 9632 >> 6 = 150
        assert!(dst.iter().all(|&v| v == 150));
    }

    #[test]
    fn blend_full_mask() {
        let a = [100u8; 4];
        let b = [200u8; 4];
        let mask_a = [64u8; 4]; // 100% a
        let mask_b = [0u8; 4]; // 100% b
        let mut dst_a = [0u8; 4];
        let mut dst_b = [0u8; 4];
        block_blend(&mut dst_a, 2, &a, 2, &b, 2, &mask_a, 2, 2, 2);
        block_blend(&mut dst_b, 2, &a, 2, &b, 2, &mask_b, 2, 2, 2);
        assert!(dst_a.iter().all(|&v| v == 100));
        assert!(dst_b.iter().all(|&v| v == 200));
    }
}

#[cfg(test)]
mod dispatch_tests {
    use super::*;
    use archmage::testing::{CompileTimePolicy, for_each_token_permutation};

    #[test]
    fn block_copy_all_dispatch_levels() {
        let src: [u8; 16] = [
            10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120, 130, 140, 150, 160,
        ];
        let mut reference = [0u8; 16];
        block_copy(&mut reference, 4, &src, 4, 4, 4);

        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            let mut result = [0u8; 16];
            block_copy(&mut result, 4, &src, 4, 4, 4);
            assert_eq!(result, reference, "copy mismatch at dispatch level {_perm}");
        });
    }

    #[test]
    fn block_average_all_dispatch_levels() {
        let a: [u8; 16] = [
            10, 30, 50, 70, 90, 110, 130, 150, 20, 40, 60, 80, 100, 120, 140, 160,
        ];
        let b: [u8; 16] = [
            200, 180, 160, 140, 120, 100, 80, 60, 190, 170, 150, 130, 110, 90, 70, 50,
        ];
        let mut reference = [0u8; 16];
        block_average(&mut reference, 4, &a, 4, &b, 4, 4, 4);

        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            let mut result = [0u8; 16];
            block_average(&mut result, 4, &a, 4, &b, 4, 4, 4);
            assert_eq!(
                result, reference,
                "average mismatch at dispatch level {_perm}"
            );
        });
    }

    #[test]
    fn block_blend_all_dispatch_levels() {
        let a: [u8; 16] = [
            10, 30, 50, 70, 90, 110, 130, 150, 20, 40, 60, 80, 100, 120, 140, 160,
        ];
        let b: [u8; 16] = [
            200, 180, 160, 140, 120, 100, 80, 60, 190, 170, 150, 130, 110, 90, 70, 50,
        ];
        let mask: [u8; 16] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 4, 12, 20, 28, 36, 44, 52];
        let mut reference = [0u8; 16];
        block_blend(&mut reference, 4, &a, 4, &b, 4, &mask, 4, 4, 4);

        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            let mut result = [0u8; 16];
            block_blend(&mut result, 4, &a, 4, &b, 4, &mask, 4, 4, 4);
            assert_eq!(
                result, reference,
                "blend mismatch at dispatch level {_perm}"
            );
        });
    }
}
