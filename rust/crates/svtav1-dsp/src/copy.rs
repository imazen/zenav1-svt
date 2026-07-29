//! Block copy and blend operations.
//!
//! Spec 06: Block copy/average/blend for compound prediction.
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

/// NEON block average: `(a + b + 1) >> 1`, 16 pixels per iteration.
///
/// `vrhaddq_u8` IS this expression — an unsigned rounding halving add — so the
/// kernel is one instruction per 16 pixels and exactly equal to the scalar
/// form by definition of the instruction, with no intermediate widening.
#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
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
    for row in 0..height {
        let d_off = row * dst_stride;
        let a_off = row * a_stride;
        let b_off = row * b_stride;
        let mut col = 0usize;
        while col + 16 <= width {
            let va = vld1q_u8(a[a_off + col..a_off + col + 16].try_into().unwrap());
            let vb = vld1q_u8(b[b_off + col..b_off + col + 16].try_into().unwrap());
            let out: &mut [u8; 16] = (&mut dst[d_off + col..d_off + col + 16])
                .try_into()
                .unwrap();
            vst1q_u8(out, vrhaddq_u8(va, vb));
            col += 16;
        }
        for c in col..width {
            let va = a[a_off + c] as u16;
            let vb = b[b_off + c] as u16;
            dst[d_off + c] = ((va + vb + 1) >> 1) as u8;
        }
    }
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

/// NEON AOM_BLEND_A64: `(a*w + b*(64-w) + 32) >> 6`, 16 pixels per iteration.
///
/// Exact, not approximate. `a*w + b*(64-w) <= 255*64 = 16320` fits u16 with
/// room to spare, and `vrshrn_n_u16::<6>` IS `(x + 32) >> 6` followed by a
/// narrow — the rounding constant is the instruction's, not an added term. The
/// result cannot exceed 255, so the narrow cannot lose information.
#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
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
    for row in 0..height {
        let d_off = row * dst_stride;
        let a_off = row * a_stride;
        let b_off = row * b_stride;
        let m_off = row * mask_stride;
        let mut col = 0usize;
        while col + 16 <= width {
            let va = vld1q_u8(a[a_off + col..a_off + col + 16].try_into().unwrap());
            let vb = vld1q_u8(b[b_off + col..b_off + col + 16].try_into().unwrap());
            let vm = vld1q_u8(mask[m_off + col..m_off + col + 16].try_into().unwrap());
            let inv = vsubq_u8(vdupq_n_u8(64), vm);

            let lo = vmlal_u8(
                vmull_u8(vget_low_u8(va), vget_low_u8(vm)),
                vget_low_u8(vb),
                vget_low_u8(inv),
            );
            let hi = vmlal_u8(
                vmull_u8(vget_high_u8(va), vget_high_u8(vm)),
                vget_high_u8(vb),
                vget_high_u8(inv),
            );
            let out: &mut [u8; 16] = (&mut dst[d_off + col..d_off + col + 16])
                .try_into()
                .unwrap();
            vst1q_u8(
                out,
                vcombine_u8(vrshrn_n_u16::<6>(lo), vrshrn_n_u16::<6>(hi)),
            );
            col += 16;
        }
        for c in col..width {
            let va = a[a_off + c] as u32;
            let vb = b[b_off + c] as u32;
            let w = mask[m_off + c] as u32;
            dst[d_off + c] = ((va * w + vb * (64 - w) + 32) >> 6) as u8;
        }
    }
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
    use alloc::{vec, vec::Vec};
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

    /// A tiny xorshift so the sweep below uses content, not constants.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }
        fn u8(&mut self) -> u8 {
            (self.next() >> 33) as u8
        }
    }

    /// Every tier vs the scalar reference across widths that straddle the
    /// vector body.
    ///
    /// The three tests around this one all use a single 4x4 block. The NEON
    /// kernels process 16 pixels per iteration, so at width 4 the vector body
    /// NEVER RUNS and only the scalar tail is exercised — those tests would
    /// pass against a completely broken vector path. This sweeps widths 1..=40
    /// (crossing 16 and 32, and every remainder), with strides wider than the
    /// block so a kernel that ignored stride would be caught.
    #[test]
    fn average_and_blend_all_widths_all_tiers() {
        let mut rng = Rng(0x51ED_BEEF_0000_0001);
        for width in 1..=40usize {
            for height in [1usize, 3, 8] {
                let stride = width + 7; // deliberately not equal to width
                let n = stride * (height + 2);
                let a: Vec<u8> = (0..n).map(|_| rng.u8()).collect();
                let b: Vec<u8> = (0..n).map(|_| rng.u8()).collect();
                // Mask values are 0..=64 in AOM_BLEND_A64.
                let mask: Vec<u8> = (0..n).map(|_| rng.u8() % 65).collect();

                let mut ref_avg = vec![0u8; n];
                block_average_inner(&mut ref_avg, stride, &a, stride, &b, stride, width, height);
                let mut ref_blend = vec![0u8; n];
                block_blend_inner(
                    &mut ref_blend, stride, &a, stride, &b, stride, &mask, stride, width, height,
                );

                let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
                    let mut got = vec![0u8; n];
                    block_average(&mut got, stride, &a, stride, &b, stride, width, height);
                    assert_eq!(
                        got, ref_avg,
                        "average w={width} h={height} tier {_perm}"
                    );

                    let mut got = vec![0u8; n];
                    block_blend(
                        &mut got, stride, &a, stride, &b, stride, &mask, stride, width, height,
                    );
                    assert_eq!(
                        got, ref_blend,
                        "blend w={width} h={height} tier {_perm}"
                    );
                });
            }
        }
    }

    /// Mask 0 and 64 are the saturating ends of AOM_BLEND_A64 and must select
    /// b and a exactly.
    #[test]
    fn blend_mask_extremes_are_exact_selects() {
        let width = 33usize; // crosses the 16-wide body twice plus a tail
        let mut rng = Rng(0xF00D_1234);
        let a: Vec<u8> = (0..width).map(|_| rng.u8()).collect();
        let b: Vec<u8> = (0..width).map(|_| rng.u8()).collect();
        for (mval, expect) in [(0u8, &b), (64u8, &a)] {
            let mask = vec![mval; width];
            let mut got = vec![0u8; width];
            block_blend(&mut got, width, &a, width, &b, width, &mask, width, width, 1);
            assert_eq!(&got, expect, "mask={mval} must select exactly");
        }
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
