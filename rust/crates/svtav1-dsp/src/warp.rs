//! Warped motion prediction.
//!
//! Spec 06: Affine warped motion prediction.
//!
//! Applies an affine transformation to the reference block before
//! sub-pixel interpolation. Used for blocks where motion is better
//! described by rotation/zoom/shear than pure translation.
//!
//! Ported from SVT-AV1's warped_motion.c and enc_warped_motion.c.

use svtav1_types::motion::WarpedMotionParams;

/// Warped motion precision constants.
pub const WARPEDMODEL_PREC_BITS: u32 = 16;
pub const WARP_PARAM_REDUCE_BITS: u32 = 6;

/// Apply warped motion prediction to produce a predicted block.
///
/// Uses the affine model in `params` to map each pixel in the output
/// to a sub-pixel location in the reference, then applies 8-tap
/// interpolation.
pub fn warp_prediction(
    ref_pic: &[u8],
    ref_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    params: &WarpedMotionParams,
    ref_x: i32,
    ref_y: i32,
    width: usize,
    height: usize,
    pic_width: usize,
    pic_height: usize,
) {
    let m = &params.wmmat;
    let filters = &svtav1_tables::interp::SUB_PEL_FILTERS_8;
    const FILTER_CENTER: i32 = 3; // Center tap index in 8-tap filter

    for r in 0..height {
        for c in 0..width {
            // Apply affine transform: [x', y'] = [m2 m3; m4 m5] * [x; y] + [m0; m1]
            let src_x = r as i32 + ref_y;
            let src_y = c as i32 + ref_x;

            // Affine mapping (Q16 fixed-point)
            let map_x = m[2] as i64 * src_y as i64
                + m[3] as i64 * src_x as i64
                + m[0] as i64 * (1i64 << WARPEDMODEL_PREC_BITS);
            let map_y = m[4] as i64 * src_y as i64
                + m[5] as i64 * src_x as i64
                + m[1] as i64 * (1i64 << WARPEDMODEL_PREC_BITS);

            // Integer pixel position (floor division via arithmetic right shift)
            let ix = (map_x >> WARPEDMODEL_PREC_BITS) as i32;
            let iy = (map_y >> WARPEDMODEL_PREC_BITS) as i32;

            // Sub-pixel fraction → 4-bit phase index for 16-phase filter table
            let frac_x = (map_x - ((ix as i64) << WARPEDMODEL_PREC_BITS)) as u32;
            let frac_y = (map_y - ((iy as i64) << WARPEDMODEL_PREC_BITS)) as u32;
            let fx = (frac_x >> (WARPEDMODEL_PREC_BITS - 4)) as usize;
            let fy = (frac_y >> (WARPEDMODEL_PREC_BITS - 4)) as usize;

            let h_filter = &filters[fx.min(15)];
            let v_filter = &filters[fy.min(15)];

            // Separable 8-tap interpolation
            // Step 1: Horizontal filter for 8 rows centered on iy
            let mut intermediate = [0i32; 8];
            for tap_row in 0..8i32 {
                let ry = (iy - FILTER_CENTER + tap_row).clamp(0, pic_height as i32 - 1) as usize;
                let mut sum = 0i32;
                for tap_col in 0..8i32 {
                    let rx = (ix - FILTER_CENTER + tap_col).clamp(0, pic_width as i32 - 1) as usize;
                    sum += ref_pic[ry * ref_stride + rx] as i32 * h_filter[tap_col as usize] as i32;
                }
                intermediate[tap_row as usize] = (sum + 64) >> 7;
            }

            // Step 2: Vertical filter on intermediate results
            let mut vsum = 0i32;
            for tap in 0..8 {
                vsum += intermediate[tap] * v_filter[tap] as i32;
            }
            dst[r * dst_stride + c] = ((vsum + 64) >> 7).clamp(0, 255) as u8;
        }
    }
}

/// Compute the number of valid warp samples from neighboring blocks.
///
/// Returns the count of samples that can be used to derive the warp model.
/// Requires at least 3 samples for a valid affine model.
pub fn count_warp_samples(
    _above_mv: Option<(i16, i16)>,
    _left_mv: Option<(i16, i16)>,
    _above_right_mv: Option<(i16, i16)>,
    _below_left_mv: Option<(i16, i16)>,
) -> u8 {
    // Count available motion vectors from neighbors
    let mut count = 0u8;
    if _above_mv.is_some() {
        count += 1;
    }
    if _left_mv.is_some() {
        count += 1;
    }
    if _above_right_mv.is_some() {
        count += 1;
    }
    if _below_left_mv.is_some() {
        count += 1;
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    use alloc::vec::Vec;

    #[test]
    fn warp_identity_is_copy() {
        // Identity warp model should just copy the reference block
        let ref_pic: Vec<u8> = (0..64).map(|i| (i * 4) as u8).collect();
        let mut dst = [0u8; 16]; // 4x4

        let params = WarpedMotionParams::default(); // Identity

        warp_prediction(&ref_pic, 8, &mut dst, 4, &params, 2, 2, 4, 4, 8, 8);

        // With identity transform, output should match ref at (2,2)
        for r in 0..4 {
            for c in 0..4 {
                let expected = ref_pic[(r + 2) * 8 + (c + 2)];
                assert_eq!(
                    dst[r * 4 + c],
                    expected,
                    "identity warp mismatch at ({r},{c})"
                );
            }
        }
    }

    #[test]
    fn count_warp_samples_basic() {
        assert_eq!(count_warp_samples(None, None, None, None), 0);
        assert_eq!(
            count_warp_samples(Some((1, 2)), Some((3, 4)), None, None),
            2
        );
        assert_eq!(
            count_warp_samples(Some((1, 2)), Some((3, 4)), Some((5, 6)), Some((7, 8))),
            4
        );
    }
}
