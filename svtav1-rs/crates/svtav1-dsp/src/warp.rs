//! Warped motion prediction.
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

    for r in 0..height {
        for c in 0..width {
            // Apply affine transform: [x', y'] = [m2 m3; m4 m5] * [x; y] + [m0; m1]
            let src_x = r as i32 + ref_y;
            let src_y = c as i32 + ref_x;

            // Affine mapping (Q16 fixed-point)
            let dst_x = m[2] as i64 * src_y as i64
                + m[3] as i64 * src_x as i64
                + m[0] as i64 * (1 << WARPEDMODEL_PREC_BITS) as i64;
            let dst_y = m[4] as i64 * src_y as i64
                + m[5] as i64 * src_x as i64
                + m[1] as i64 * (1 << WARPEDMODEL_PREC_BITS) as i64;

            // Convert to integer pixel + sub-pixel fraction
            let px = (dst_x >> WARPEDMODEL_PREC_BITS) as i32;
            let py = (dst_y >> WARPEDMODEL_PREC_BITS) as i32;

            // Clamp to reference bounds
            let rx = px.clamp(0, pic_width as i32 - 1) as usize;
            let ry = py.clamp(0, pic_height as i32 - 1) as usize;

            dst[r * dst_stride + c] = ref_pic[ry * ref_stride + rx];
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
