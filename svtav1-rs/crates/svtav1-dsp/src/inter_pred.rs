//! Inter prediction (sub-pixel convolution).
//!
//! Ported from SVT-AV1's `convolve.c`.
//!
//! AV1 inter prediction uses 8-tap separable filters for sub-pixel
//! interpolation. Each filter is applied in two passes: horizontal
//! then vertical. The filter coefficients come from
//! `svtav1_tables::interp`.

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
/// `filter` is an 8-tap kernel from `svtav1_tables::interp`.
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

#[cfg(target_arch = "aarch64")]
#[arcane]
fn convolve_horiz_impl_neon(
    _token: NeonToken,
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
/// `filter` is an 8-tap kernel from `svtav1_tables::interp`.
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

#[cfg(target_arch = "aarch64")]
#[arcane]
fn convolve_vert_impl_neon(
    _token: NeonToken,
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

#[cfg(target_arch = "aarch64")]
#[arcane]
fn convolve_2d_impl_neon(
    _token: NeonToken,
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
mod tests {
    use super::*;
    use crate::copy::block_copy;
    use svtav1_tables::interp::SUB_PEL_FILTERS_8;

    /// `convolve_copy` must produce identical output to `block_copy`.
    #[test]
    fn convolve_copy_equals_block_copy() {
        let width = 8;
        let height = 8;
        let stride = 16;
        // Source with known pattern
        let src: alloc::vec::Vec<u8> = (0..(stride * height) as u16)
            .map(|i| (i % 256) as u8)
            .collect();
        let mut dst_copy = alloc::vec![0u8; stride * height];
        let mut dst_conv = alloc::vec![0u8; stride * height];

        block_copy(&mut dst_copy, stride, &src, stride, width, height);
        convolve_copy(&src, stride, &mut dst_conv, stride, width, height);

        assert_eq!(dst_copy, dst_conv);
    }

    /// Phase-0 filter `[0,0,0,128,0,0,0,0]` is identity — the result should
    /// match a plain copy of the centered source pixels.
    #[test]
    fn integer_pel_filter_equals_copy() {
        let identity_filter = &SUB_PEL_FILTERS_8[0]; // [0,0,0,128,0,0,0,0]

        let width = 4;
        let height = 4;
        // We need FILTER_CENTER columns of padding on the left. Build a
        // padded source where logical pixel (r,c) is at src[(r)*(width+7) + (c+3)].
        let padded_w = width + FILTER_TAPS - 1; // 4 + 7 = 11
        let padded_h = height;
        let src: alloc::vec::Vec<u8> = (0..(padded_w * padded_h) as u16)
            .map(|i| ((i * 7 + 13) % 256) as u8)
            .collect();

        let mut dst_horiz = alloc::vec![0u8; width * height];
        convolve_horiz(
            &src,
            padded_w,
            &mut dst_horiz,
            width,
            identity_filter,
            width,
            height,
        );

        // The identity filter should just pick out the center tap, which is at
        // offset FILTER_CENTER from the start of each row's window.
        for row in 0..height {
            for col in 0..width {
                let expected = src[row * padded_w + col + FILTER_CENTER];
                assert_eq!(
                    dst_horiz[row * width + col],
                    expected,
                    "mismatch at row={row}, col={col}"
                );
            }
        }
    }

    /// Uniform input should produce the same uniform output after convolution.
    #[test]
    fn uniform_input_stays_uniform() {
        let val = 100u8;
        let width = 4;
        let height = 4;
        let padded_w = width + FILTER_TAPS - 1;
        let padded_h = height + FILTER_TAPS - 1;
        let src = alloc::vec![val; padded_w * padded_h];

        // Test with a non-trivial filter (phase 8 = half-pixel)
        let filter = &SUB_PEL_FILTERS_8[8];

        // Horizontal
        let mut dst_h = alloc::vec![0u8; width * padded_h];
        convolve_horiz(&src, padded_w, &mut dst_h, width, filter, width, padded_h);
        for (i, &v) in dst_h.iter().enumerate() {
            assert_eq!(v, val, "horizontal: mismatch at index {i}");
        }

        // Vertical
        let mut dst_v = alloc::vec![0u8; width * height];
        convolve_vert(&src, padded_w, &mut dst_v, width, filter, width, height);
        for (i, &v) in dst_v.iter().enumerate() {
            assert_eq!(v, val, "vertical: mismatch at index {i}");
        }

        // 2D
        let mut dst_2d = alloc::vec![0u8; width * height];
        convolve_2d(
            &src,
            padded_w,
            &mut dst_2d,
            width,
            filter,
            filter,
            width,
            height,
        );
        for (i, &v) in dst_2d.iter().enumerate() {
            assert_eq!(v, val, "2d: mismatch at index {i}");
        }
    }

    /// Known filter + known input: verify against hand-computed values.
    #[test]
    fn known_filter_known_input() {
        // Use the regular filter at phase 8 (half-pixel):
        // [0, 2, -14, 76, 76, -14, 2, 0]
        let filter = &SUB_PEL_FILTERS_8[8];

        // 1-row, 1-pixel output. Source = 8 consecutive values.
        let src: [u8; 8] = [10, 20, 30, 40, 50, 60, 70, 80];
        let mut dst = [0u8; 1];

        convolve_horiz(&src, 8, &mut dst, 1, filter, 1, 1);

        // Hand-compute:
        // sum = 0*10 + 2*20 + (-14)*30 + 76*40 + 76*50 + (-14)*60 + 2*70 + 0*80
        //     = 0 + 40 - 420 + 3040 + 3800 - 840 + 140 + 0 = 5760
        // normalized = (5760 + 64) >> 7 = 5824 >> 7 = 45
        let expected =
            (((2 * 20) + (-14) * 30 + 76 * 40 + 76 * 50 + (-14) * 60 + 2 * 70) + 64) >> 7;
        assert_eq!(
            dst[0], expected as u8,
            "expected {expected}, got {}",
            dst[0]
        );
    }

    /// Vertical convolution with known values.
    #[test]
    fn known_vertical_convolution() {
        // 8 rows, 1 column, stride = 1
        let src: [u8; 8] = [10, 20, 30, 40, 50, 60, 70, 80];
        let filter = &SUB_PEL_FILTERS_8[8]; // [0, 2, -14, 76, 76, -14, 2, 0]
        let mut dst = [0u8; 1];

        convolve_vert(&src, 1, &mut dst, 1, filter, 1, 1);

        let expected =
            (((2 * 20) + (-14) * 30 + 76 * 40 + 76 * 50 + (-14) * 60 + 2 * 70) + 64) >> 7;
        assert_eq!(
            dst[0], expected as u8,
            "expected {expected}, got {}",
            dst[0]
        );
    }

    /// 2D convolution with identity horizontal, non-trivial vertical.
    #[test]
    fn convolve_2d_identity_horiz() {
        let identity = &SUB_PEL_FILTERS_8[0];
        let half_pel = &SUB_PEL_FILTERS_8[8];
        let width = 1;
        let height = 1;
        let padded_w = width + FILTER_TAPS - 1; // 8
        let padded_h = height + FILTER_TAPS - 1; // 8

        // Build a padded source. Each row has padded_w pixels.
        // We want the vertical column to be [10,20,30,40,50,60,70,80].
        // With identity horiz filter, the center tap (index 3) is picked.
        let mut src = alloc::vec![0u8; padded_w * padded_h];
        let col_vals: [u8; 8] = [10, 20, 30, 40, 50, 60, 70, 80];
        for row in 0..padded_h {
            // Put the desired value at the center-tap position
            src[row * padded_w + FILTER_CENTER] = col_vals[row];
        }

        let mut dst = [0u8; 1];
        convolve_2d(
            &src, padded_w, &mut dst, 1, identity, half_pel, width, height,
        );

        let expected =
            (((2 * 20) + (-14) * 30 + 76 * 40 + 76 * 50 + (-14) * 60 + 2 * 70) + 64) >> 7;
        assert_eq!(dst[0], expected as u8);
    }
}

#[cfg(test)]
mod dispatch_tests {
    use super::*;
    use archmage::testing::{CompileTimePolicy, for_each_token_permutation};
    use svtav1_tables::interp::SUB_PEL_FILTERS_8;

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
