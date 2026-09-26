//! Inter prediction (sub-pixel convolution).
//!
//! Spec 06 (inter-prediction.md): Sub-pixel interpolation for inter prediction.
//!
//! Ported from SVT-AV1's `convolve.c`.
//!
//! AV1 inter prediction uses 8-tap separable filters for sub-pixel
//! interpolation. Each filter is applied in two passes: horizontal
//! then vertical. The filter coefficients come from
//! `svtav1_types::tables::interp`.

use archmage::prelude::*;

/// Number of filter bits for normalization (sum of taps = 128 = 1 << 7).
const FILTER_BITS: i32 = 7;

/// Number of taps in the interpolation filter.
const FILTER_TAPS: usize = 8;

/// Offset to center of the 8-tap filter (tap index 3 is the center).
pub const FILTER_CENTER: usize = 3;

/// Apply horizontal 8-tap convolution for sub-pixel interpolation.
///
/// `src` should have at least `FILTER_CENTER` pixels of padding before the
/// logical origin in each row (the caller must offset the slice).
/// `filter` is an 8-tap kernel from `svtav1_types::tables::interp`.
///
/// The output is clipped to `[0, 255]`.
pub fn convolve_horiz(
    src: &[u8],
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    filter: &[i16; 8],
    width: usize,
    height: usize,
) {
    incant!(
        convolve_horiz_impl(src, src_stride, dst, dst_stride, filter, width, height),
        [v3, neon, scalar]
    )
}

fn convolve_horiz_impl_scalar(
    _token: ScalarToken,
    src: &[u8],
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    filter: &[i16; 8],
    width: usize,
    height: usize,
) {
    convolve_horiz_inner(src, src_stride, dst, dst_stride, filter, width, height);
}

#[cfg(target_arch = "x86_64")]
#[arcane]
fn convolve_horiz_impl_v3(
    _token: Desktop64,
    src: &[u8],
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    filter: &[i16; 8],
    width: usize,
    height: usize,
) {
    convolve_horiz_inner(src, src_stride, dst, dst_stride, filter, width, height);
}

/// NEON 8-tap horizontal convolve body, shared by the `convolve_horiz` entry
/// point and the two-pass `convolve_2d`.
///
/// The window slides one byte per output pixel, so unlike the vertical filter
/// the taps cannot come from separate loads. One 16-byte load per 8 outputs
/// covers every tap, and `vextq_s16` extracts window `k` from the widened
/// halves — the tap index must be a constant, so the 8 taps are unrolled.
///
/// BOUNDS: the 16-byte load reaches `col + 15`, while the scalar reads only up
/// to `col + 7`. At the last body column that is one byte FURTHER than the
/// scalar ever touches, so the vector path is additionally guarded on the real
/// buffer length and falls back to scalar rather than over-reading. Callers
/// with a padded frame buffer take the fast path throughout.
#[cfg(target_arch = "aarch64")]
#[rite]
fn convolve_horiz_body_neon(
    _token: NeonToken,
    src: &[u8],
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    filter: &[i16; 8],
    width: usize,
    height: usize,
) {
    for row in 0..height {
        let s_row = row * src_stride;
        let d_row = row * dst_stride;
        let mut col = 0usize;
        while col + 8 <= width && s_row + col + 16 <= src.len() {
            let v = vld1q_u8(src[s_row + col..s_row + col + 16].try_into().unwrap());
            let lo = vreinterpretq_s16_u16(vmovl_u8(vget_low_u8(v)));
            let hi = vreinterpretq_s16_u16(vmovl_u8(vget_high_u8(v)));

            let mut a0 = vdupq_n_s32(0);
            let mut a1 = vdupq_n_s32(0);
            // Unrolled: vextq_s16's lane offset must be a const generic.
            let w = [
                lo,
                vextq_s16::<1>(lo, hi),
                vextq_s16::<2>(lo, hi),
                vextq_s16::<3>(lo, hi),
                vextq_s16::<4>(lo, hi),
                vextq_s16::<5>(lo, hi),
                vextq_s16::<6>(lo, hi),
                vextq_s16::<7>(lo, hi),
            ];
            for k in 0..FILTER_TAPS {
                a0 = vmlal_n_s16(a0, vget_low_s16(w[k]), filter[k]);
                a1 = vmlal_n_s16(a1, vget_high_s16(w[k]), filter[k]);
            }
            let packed = vcombine_s16(
                vqrshrn_n_s32::<FILTER_BITS>(a0),
                vqrshrn_n_s32::<FILTER_BITS>(a1),
            );
            let out: &mut [u8; 8] = (&mut dst[d_row + col..d_row + col + 8]).try_into().unwrap();
            vst1_u8(out, vqmovun_s16(packed));
            col += 8;
        }
        for c in col..width {
            let mut sum: i32 = 0;
            for k in 0..FILTER_TAPS {
                sum += src[s_row + c + k] as i32 * filter[k] as i32;
            }
            let val = (sum + (1 << (FILTER_BITS - 1))) >> FILTER_BITS;
            dst[d_row + c] = val.clamp(0, 255) as u8;
        }
    }
}

/// NEON 8-tap horizontal convolve. See [`convolve_horiz_body_neon`].
#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn convolve_horiz_impl_neon(
    token: NeonToken,
    src: &[u8],
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    filter: &[i16; 8],
    width: usize,
    height: usize,
) {
    convolve_horiz_body_neon(
        token, src, src_stride, dst, dst_stride, filter, width, height,
    );
}

#[inline]
fn convolve_horiz_inner(
    src: &[u8],
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    filter: &[i16; 8],
    width: usize,
    height: usize,
) {
    for row in 0..height {
        let s_row = row * src_stride;
        let d_row = row * dst_stride;
        for col in 0..width {
            let mut sum: i32 = 0;
            for k in 0..FILTER_TAPS {
                let src_idx = s_row + col + k;
                sum += src[src_idx] as i32 * filter[k] as i32;
            }
            let val = (sum + (1 << (FILTER_BITS - 1))) >> FILTER_BITS;
            dst[d_row + col] = val.clamp(0, 255) as u8;
        }
    }
}

/// Apply vertical 8-tap convolution for sub-pixel interpolation.
///
/// `src` should have at least `FILTER_CENTER` rows of padding above the
/// logical origin (the caller must offset the slice).
/// `filter` is an 8-tap kernel from `svtav1_types::tables::interp`.
///
/// The output is clipped to `[0, 255]`.
pub fn convolve_vert(
    src: &[u8],
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    filter: &[i16; 8],
    width: usize,
    height: usize,
) {
    incant!(
        convolve_vert_impl(src, src_stride, dst, dst_stride, filter, width, height),
        [v3, neon, scalar]
    )
}

fn convolve_vert_impl_scalar(
    _token: ScalarToken,
    src: &[u8],
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    filter: &[i16; 8],
    width: usize,
    height: usize,
) {
    convolve_vert_inner(src, src_stride, dst, dst_stride, filter, width, height);
}

#[cfg(target_arch = "x86_64")]
#[arcane]
fn convolve_vert_impl_v3(
    _token: Desktop64,
    src: &[u8],
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    filter: &[i16; 8],
    width: usize,
    height: usize,
) {
    convolve_vert_inner(src, src_stride, dst, dst_stride, filter, width, height);
}

/// NEON 8-tap vertical convolve, 8 columns per iteration.
///
/// Columns are independent and each tap reads a whole row, so the vertical
/// filter vectorizes without any sliding-window shuffling: load 8 bytes from
/// each of the 8 source rows, widen, and multiply-accumulate by the scalar tap.
///
/// Exact. Accumulation is i32, matching the scalar `sum`. The final
/// `(sum + 64) >> 7` and the `clamp(0, 255)` are the instructions' own
/// semantics rather than added terms: `vqrshrn_n_s32::<7>` is a rounding
/// shift-right-by-7 with saturation, and `vqmovun_s16` saturates a signed
/// value into `[0, 255]`. Neither saturation can trigger spuriously — with
/// `|src| <= 255` and eight taps summing to 128, `|sum| >> 7` stays far inside
/// i16 — but they are the correct behaviour if a caller passes an unusual
/// filter, exactly as the scalar clamp is.
#[cfg(target_arch = "aarch64")]
#[rite]
fn convolve_vert_body_neon(
    _token: NeonToken,
    src: &[u8],
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    filter: &[i16; 8],
    width: usize,
    height: usize,
) {
    for row in 0..height {
        let d_row = row * dst_stride;
        let mut col = 0usize;
        while col + 8 <= width {
            let mut lo = vdupq_n_s32(0);
            let mut hi = vdupq_n_s32(0);
            for k in 0..FILTER_TAPS {
                let off = (row + k) * src_stride + col;
                let sv = vld1_u8(src[off..off + 8].try_into().unwrap());
                let s16 = vreinterpretq_s16_u16(vmovl_u8(sv));
                lo = vmlal_n_s16(lo, vget_low_s16(s16), filter[k]);
                hi = vmlal_n_s16(hi, vget_high_s16(s16), filter[k]);
            }
            let packed = vcombine_s16(
                vqrshrn_n_s32::<FILTER_BITS>(lo),
                vqrshrn_n_s32::<FILTER_BITS>(hi),
            );
            let out: &mut [u8; 8] = (&mut dst[d_row + col..d_row + col + 8]).try_into().unwrap();
            vst1_u8(out, vqmovun_s16(packed));
            col += 8;
        }
        for c in col..width {
            let mut sum: i32 = 0;
            for k in 0..FILTER_TAPS {
                sum += src[(row + k) * src_stride + c] as i32 * filter[k] as i32;
            }
            let val = (sum + (1 << (FILTER_BITS - 1))) >> FILTER_BITS;
            dst[d_row + c] = val.clamp(0, 255) as u8;
        }
    }
}

#[inline]
fn convolve_vert_inner(
    src: &[u8],
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    filter: &[i16; 8],
    width: usize,
    height: usize,
) {
    for row in 0..height {
        let d_row = row * dst_stride;
        for col in 0..width {
            let mut sum: i32 = 0;
            for k in 0..FILTER_TAPS {
                let src_idx = (row + k) * src_stride + col;
                sum += src[src_idx] as i32 * filter[k] as i32;
            }
            let val = (sum + (1 << (FILTER_BITS - 1))) >> FILTER_BITS;
            dst[d_row + col] = val.clamp(0, 255) as u8;
        }
    }
}

/// Full 2D sub-pixel interpolation (horizontal then vertical).
///
/// Applies `h_filter` horizontally, then `v_filter` vertically. An
/// intermediate buffer is allocated internally.
///
/// `src` must be padded by `FILTER_CENTER` pixels in all four directions
/// from the logical block origin. That is, the slice starts at
/// `(logical_row - FILTER_CENTER) * src_stride + (logical_col - FILTER_CENTER)`.
pub fn convolve_2d(
    src: &[u8],
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    h_filter: &[i16; 8],
    v_filter: &[i16; 8],
    width: usize,
    height: usize,
) {
    incant!(
        convolve_2d_impl(
            src, src_stride, dst, dst_stride, h_filter, v_filter, width, height
        ),
        [v3, neon, scalar]
    )
}

fn convolve_2d_impl_scalar(
    _token: ScalarToken,
    src: &[u8],
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    h_filter: &[i16; 8],
    v_filter: &[i16; 8],
    width: usize,
    height: usize,
) {
    convolve_2d_inner(
        src, src_stride, dst, dst_stride, h_filter, v_filter, width, height,
    );
}

#[cfg(target_arch = "x86_64")]
#[arcane]
fn convolve_2d_impl_v3(
    _token: Desktop64,
    src: &[u8],
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    h_filter: &[i16; 8],
    v_filter: &[i16; 8],
    width: usize,
    height: usize,
) {
    convolve_2d_inner(
        src, src_stride, dst, dst_stride, h_filter, v_filter, width, height,
    );
}

/// NEON two-pass convolve: the same u8-intermediate composition the scalar
/// core performs, with both passes vectorized. Identical structure, so it
/// stays bit-exact with the scalar path (and hence with the C composition the
/// parity test checks) as long as each pass is.
#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn convolve_2d_impl_neon(
    token: NeonToken,
    src: &[u8],
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    h_filter: &[i16; 8],
    v_filter: &[i16; 8],
    width: usize,
    height: usize,
) {
    let intermediate_height = height + FILTER_TAPS - 1;
    let intermediate_stride = width;
    let mut intermediate = alloc::vec![0u8; intermediate_height * intermediate_stride];

    convolve_horiz_body_neon(
        token,
        src,
        src_stride,
        &mut intermediate,
        intermediate_stride,
        h_filter,
        width,
        intermediate_height,
    );
    convolve_vert_body_neon(
        token,
        &intermediate,
        intermediate_stride,
        dst,
        dst_stride,
        v_filter,
        width,
        height,
    );
}

#[inline]
fn convolve_2d_inner(
    src: &[u8],
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    h_filter: &[i16; 8],
    v_filter: &[i16; 8],
    width: usize,
    height: usize,
) {
    let intermediate_height = height + FILTER_TAPS - 1;
    let intermediate_stride = width;
    let mut intermediate = alloc::vec![0u8; intermediate_height * intermediate_stride];

    convolve_horiz_inner(
        src,
        src_stride,
        &mut intermediate,
        intermediate_stride,
        h_filter,
        width,
        intermediate_height,
    );

    convolve_vert_inner(
        &intermediate,
        intermediate_stride,
        dst,
        dst_stride,
        v_filter,
        width,
        height,
    );
}

/// Integer-pel copy (no filtering needed).
///
/// This is the special case when both horizontal and vertical sub-pixel
/// offsets are zero (phase 0 filter = identity).
pub fn convolve_copy(
    src: &[u8],
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    width: usize,
    height: usize,
) {
    for row in 0..height {
        let s_off = row * src_stride;
        let d_off = row * dst_stride;
        dst[d_off..d_off + width].copy_from_slice(&src[s_off..s_off + width]);
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod dispatch_tests {
    use super::*;
    use archmage::testing::{CompileTimePolicy, for_each_token_permutation};
    use svtav1_types::tables::interp::SUB_PEL_FILTERS_8;

    #[test]
    fn convolve_horiz_all_dispatch_levels() {
        let filter = &SUB_PEL_FILTERS_8[8]; // half-pixel filter
        let width = 4;
        let height = 4;
        let padded_w = width + FILTER_TAPS - 1;
        let src: alloc::vec::Vec<u8> = (0..(padded_w * height) as u16)
            .map(|i| ((i * 7 + 13) % 256) as u8)
            .collect();
        let mut reference = alloc::vec![0u8; width * height];
        convolve_horiz(&src, padded_w, &mut reference, width, filter, width, height);

        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            let mut result = alloc::vec![0u8; width * height];
            convolve_horiz(&src, padded_w, &mut result, width, filter, width, height);
            assert_eq!(
                result, reference,
                "horiz mismatch at dispatch level {_perm}"
            );
        });
    }

    #[test]
    fn convolve_vert_all_dispatch_levels() {
        let filter = &SUB_PEL_FILTERS_8[8];
        let width = 4;
        let height = 4;
        let padded_h = height + FILTER_TAPS - 1;
        let src: alloc::vec::Vec<u8> = (0..(width * padded_h) as u16)
            .map(|i| ((i * 11 + 3) % 256) as u8)
            .collect();
        let mut reference = alloc::vec![0u8; width * height];
        convolve_vert(&src, width, &mut reference, width, filter, width, height);

        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            let mut result = alloc::vec![0u8; width * height];
            convolve_vert(&src, width, &mut result, width, filter, width, height);
            assert_eq!(result, reference, "vert mismatch at dispatch level {_perm}");
        });
    }

    #[test]
    fn convolve_2d_all_dispatch_levels() {
        let h_filter = &SUB_PEL_FILTERS_8[8];
        let v_filter = &SUB_PEL_FILTERS_8[4];
        let width = 4;
        let height = 4;
        let padded_w = width + FILTER_TAPS - 1;
        let padded_h = height + FILTER_TAPS - 1;
        let src: alloc::vec::Vec<u8> = (0..(padded_w * padded_h) as u16)
            .map(|i| ((i * 13 + 7) % 256) as u8)
            .collect();
        let mut reference = alloc::vec![0u8; width * height];
        convolve_2d(
            &src,
            padded_w,
            &mut reference,
            width,
            h_filter,
            v_filter,
            width,
            height,
        );

        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            let mut result = alloc::vec![0u8; width * height];
            convolve_2d(
                &src,
                padded_w,
                &mut result,
                width,
                h_filter,
                v_filter,
                width,
                height,
            );
            assert_eq!(result, reference, "2d mismatch at dispatch level {_perm}");
        });
    }
}

/// NEON 8-tap vertical convolve entry point. See [`convolve_vert_body_neon`].
#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn convolve_vert_impl_neon(
    token: NeonToken,
    src: &[u8],
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    filter: &[i16; 8],
    width: usize,
    height: usize,
) {
    convolve_vert_body_neon(
        token, src, src_stride, dst, dst_stride, filter, width, height,
    );
}
