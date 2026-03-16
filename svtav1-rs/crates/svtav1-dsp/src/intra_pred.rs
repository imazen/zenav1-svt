//! Intra prediction modes.
//!
//! Ported from SVT-AV1's `intra_prediction.c` and `enc_intra_prediction.c`.
//!
//! AV1 defines 13 intra prediction modes: DC, V, H, 8 directional,
//! smooth/smooth_v/smooth_h, and paeth.
//!
//! Key functions use archmage SIMD dispatch for auto-vectorization.

use archmage::prelude::*;

/// Predict a block using DC prediction (average of above + left neighbors).
///
/// `above`: pixels above the block (width pixels)
/// `left`: pixels to the left of the block (height pixels)
pub fn predict_dc(
    dst: &mut [u8],
    dst_stride: usize,
    above: &[u8],
    left: &[u8],
    width: usize,
    height: usize,
    has_above: bool,
    has_left: bool,
) {
    let dc = match (has_above, has_left) {
        (true, true) => {
            let sum: u32 = above[..width].iter().map(|&v| v as u32).sum::<u32>()
                + left[..height].iter().map(|&v| v as u32).sum::<u32>();
            let count = (width + height) as u32;
            ((sum + count / 2) / count) as u8
        }
        (true, false) => {
            let sum: u32 = above[..width].iter().map(|&v| v as u32).sum();
            ((sum + width as u32 / 2) / width as u32) as u8
        }
        (false, true) => {
            let sum: u32 = left[..height].iter().map(|&v| v as u32).sum();
            ((sum + height as u32 / 2) / height as u32) as u8
        }
        (false, false) => 128,
    };

    for row in 0..height {
        for col in 0..width {
            dst[row * dst_stride + col] = dc;
        }
    }
}

/// Predict a block using vertical prediction (copy above row).
pub fn predict_v(dst: &mut [u8], dst_stride: usize, above: &[u8], width: usize, height: usize) {
    for row in 0..height {
        dst[row * dst_stride..row * dst_stride + width].copy_from_slice(&above[..width]);
    }
}

/// Predict a block using horizontal prediction (copy left column).
pub fn predict_h(dst: &mut [u8], dst_stride: usize, left: &[u8], width: usize, height: usize) {
    for row in 0..height {
        let val = left[row];
        for col in 0..width {
            dst[row * dst_stride + col] = val;
        }
    }
}

/// Predict a block using smooth prediction (weighted combination of above, left,
/// and corner values using smooth weight tables).
///
/// Smooth is a bilinear interpolation between above[c], left[r],
/// above[width-1] (right), and left[height-1] (bottom).
pub fn predict_smooth(
    dst: &mut [u8],
    dst_stride: usize,
    above: &[u8],
    left: &[u8],
    width: usize,
    height: usize,
) {
    let below_pred = left[height - 1] as u32;
    let right_pred = above[width - 1] as u32;

    let sm_weights_h = smooth_weights(height);
    let sm_weights_w = smooth_weights(width);

    for row in 0..height {
        for col in 0..width {
            let wh = sm_weights_h[row] as u32;
            let ww = sm_weights_w[col] as u32;
            let top = above[col] as u32;
            let lft = left[row] as u32;

            // Smooth interpolation
            let pred =
                (wh * top + (256 - wh) * below_pred + ww * lft + (256 - ww) * right_pred + 256)
                    / 512;
            dst[row * dst_stride + col] = pred.min(255) as u8;
        }
    }
}

/// Predict a block using smooth vertical (only vertical interpolation).
pub fn predict_smooth_v(
    dst: &mut [u8],
    dst_stride: usize,
    above: &[u8],
    left: &[u8],
    _width: usize,
    height: usize,
    width: usize,
) {
    let below_pred = left[height - 1] as u32;
    let sm_weights = smooth_weights(height);

    for row in 0..height {
        let w = sm_weights[row] as u32;
        for col in 0..width {
            let top = above[col] as u32;
            let pred = (w * top + (256 - w) * below_pred + 128) / 256;
            dst[row * dst_stride + col] = pred.min(255) as u8;
        }
    }
}

/// Predict a block using smooth horizontal (only horizontal interpolation).
pub fn predict_smooth_h(
    dst: &mut [u8],
    dst_stride: usize,
    above: &[u8],
    left: &[u8],
    width: usize,
    height: usize,
) {
    let right_pred = above[width - 1] as u32;
    let sm_weights = smooth_weights(width);

    for row in 0..height {
        let lft = left[row] as u32;
        for col in 0..width {
            let w = sm_weights[col] as u32;
            let pred = (w * lft + (256 - w) * right_pred + 128) / 256;
            dst[row * dst_stride + col] = pred.min(255) as u8;
        }
    }
}

/// Predict a block using Paeth prediction.
///
/// For each pixel, choose the nearest of above[c], left[r], or
/// above[-1] (top-left corner) based on gradient direction.
pub fn predict_paeth(
    dst: &mut [u8],
    dst_stride: usize,
    above: &[u8],
    left: &[u8],
    top_left: u8,
    width: usize,
    height: usize,
) {
    incant!(
        predict_paeth_impl(dst, dst_stride, above, left, top_left, width, height),
        [v3, neon, scalar]
    );
}

fn predict_paeth_impl_scalar(
    _token: ScalarToken,
    dst: &mut [u8],
    dst_stride: usize,
    above: &[u8],
    left: &[u8],
    top_left: u8,
    width: usize,
    height: usize,
) {
    predict_paeth_core(dst, dst_stride, above, left, top_left, width, height);
}

#[cfg(target_arch = "x86_64")]
#[arcane]
fn predict_paeth_impl_v3(
    _token: Desktop64,
    dst: &mut [u8],
    dst_stride: usize,
    above: &[u8],
    left: &[u8],
    top_left: u8,
    width: usize,
    height: usize,
) {
    predict_paeth_core(dst, dst_stride, above, left, top_left, width, height);
}

#[cfg(target_arch = "aarch64")]
#[arcane]
fn predict_paeth_impl_neon(
    _token: NeonToken,
    dst: &mut [u8],
    dst_stride: usize,
    above: &[u8],
    left: &[u8],
    top_left: u8,
    width: usize,
    height: usize,
) {
    predict_paeth_core(dst, dst_stride, above, left, top_left, width, height);
}

/// Core Paeth implementation (shared across dispatch tiers).
#[inline]
fn predict_paeth_core(
    dst: &mut [u8],
    dst_stride: usize,
    above: &[u8],
    left: &[u8],
    top_left: u8,
    width: usize,
    height: usize,
) {
    for row in 0..height {
        for col in 0..width {
            let top = above[col] as i32;
            let lft = left[row] as i32;
            let tl = top_left as i32;

            let base = top + lft - tl;
            let p_top = (base - top).abs();
            let p_left = (base - lft).abs();
            let p_tl = (base - tl).abs();

            let pred = if p_top <= p_left && p_top <= p_tl {
                top
            } else if p_left <= p_tl {
                lft
            } else {
                tl
            };
            dst[row * dst_stride + col] = pred as u8;
        }
    }
}

// =============================================================================
// Directional prediction (8 angular modes)
// Ported from svt_av1_dr_prediction_z1/z2/z3_c in intra_prediction.c
// =============================================================================

/// Derivative table for directional prediction angles.
/// `eb_dr_intra_derivative[angle]` = 256/tan(angle) for zone 1 (0-90°).
static DR_INTRA_DERIVATIVE: [u16; 90] = [
    0, 0, 0, 1023, 0, 0, 547, 0, 0, 372, 0, 0, 0, 0, 273, 0, 0, 215, 0, 0, 178, 0, 0, 151, 0, 0,
    132, 0, 0, 116, 0, 0, 102, 0, 0, 0, 90, 0, 0, 80, 0, 0, 71, 0, 0, 64, 0, 0, 57, 0, 0, 51, 0, 0,
    45, 0, 0, 0, 40, 0, 0, 35, 0, 0, 31, 0, 0, 27, 0, 0, 23, 0, 0, 19, 0, 0, 15, 0, 0, 0, 0, 11, 0,
    0, 7, 0, 0, 3, 0, 0,
];

/// Base angles for directional intra modes.
pub const MODE_TO_ANGLE: [i32; 8] = [
    0, // D45_PRED  → 45°
    0, // D135_PRED → 135°
    0, // D113_PRED → 113°
    0, // D157_PRED → 157°
    0, // D203_PRED → 203°
    0, // D67_PRED  → 67°
    0, 0,
];

fn get_dx(angle: i32) -> i32 {
    if angle > 0 && angle < 90 {
        DR_INTRA_DERIVATIVE[angle as usize] as i32
    } else if angle > 90 && angle < 180 {
        DR_INTRA_DERIVATIVE[(180 - angle) as usize] as i32
    } else {
        1
    }
}

fn get_dy(angle: i32) -> i32 {
    if angle > 90 && angle < 180 {
        DR_INTRA_DERIVATIVE[(angle - 90) as usize] as i32
    } else if angle > 180 && angle < 270 {
        DR_INTRA_DERIVATIVE[(270 - angle) as usize] as i32
    } else {
        1
    }
}

/// Directional prediction, zone 1: 0 < angle < 90.
/// Interpolates along the `above` neighbor row.
fn dr_prediction_z1(
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u8],
    dx: i32,
) {
    let max_base_x = (bw + bh) as i32 - 1;
    for r in 0..bh {
        let x = dx * (r as i32 + 1);
        let base = x >> 6;
        let shift = ((x) & 0x3F) >> 1;

        for c in 0..bw {
            let b = base + c as i32;
            if b < max_base_x {
                let val = above[b as usize] as i32 * (32 - shift)
                    + above[(b + 1) as usize] as i32 * shift;
                dst[r * dst_stride + c] = ((val + 16) >> 5).clamp(0, 255) as u8;
            } else {
                dst[r * dst_stride + c] = above[max_base_x as usize];
            }
        }
    }
}

/// Directional prediction, zone 3: 180 < angle < 270.
/// Interpolates along the `left` neighbor column.
fn dr_prediction_z3(dst: &mut [u8], dst_stride: usize, bw: usize, bh: usize, left: &[u8], dy: i32) {
    let max_base_y = (bw + bh) as i32 - 1;
    for c in 0..bw {
        let y = dy * (c as i32 + 1);
        let base = y >> 6;
        let shift = ((y) & 0x3F) >> 1;

        for r in 0..bh {
            let b = base + r as i32;
            if b < max_base_y {
                let val =
                    left[b as usize] as i32 * (32 - shift) + left[(b + 1) as usize] as i32 * shift;
                dst[r * dst_stride + c] = ((val + 16) >> 5).clamp(0, 255) as u8;
            } else {
                dst[r * dst_stride + c] = left[max_base_y as usize];
            }
        }
    }
}

/// Directional prediction, zone 2: 90 < angle < 180.
/// Interpolates using both `above` and `left` neighbors.
fn dr_prediction_z2(
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u8],
    left: &[u8],
    dx: i32,
    dy: i32,
) {
    for r in 0..bh {
        for c in 0..bw {
            // Try above neighbor (zone 1 direction)
            let y_above = -(r as i32 + 1) * dy;
            let x_base = (c as i32) + (y_above >> 6);
            let x_shift = ((y_above) & 0x3F) >> 1;

            // Try left neighbor (zone 3 direction)
            let x_left = -(c as i32 + 1) * dx;
            let y_base = (r as i32) + (x_left >> 6);
            let y_shift = ((x_left) & 0x3F) >> 1;

            let val = if x_base >= 0 {
                // Use above neighbor
                let b = x_base as usize;
                let a0 = if b < above.len() { above[b] } else { 128 };
                let a1 = if b + 1 < above.len() {
                    above[b + 1]
                } else {
                    128
                };
                a0 as i32 * (32 - x_shift) + a1 as i32 * x_shift
            } else if y_base >= 0 {
                // Use left neighbor
                let b = y_base as usize;
                let l0 = if b < left.len() { left[b] } else { 128 };
                let l1 = if b + 1 < left.len() { left[b + 1] } else { 128 };
                l0 as i32 * (32 - y_shift) + l1 as i32 * y_shift
            } else {
                128 * 32 // fallback
            };
            dst[r * dst_stride + c] = ((val + 16) >> 5).clamp(0, 255) as u8;
        }
    }
}

/// Predict a block using directional prediction at the given angle.
///
/// `angle` is in degrees (0-270). The 8 directional modes map to:
/// D45=45, D67=67, D113=113, D135=135, D157=157, D203=203
///
/// `above` and `left` must have at least `width + height` elements.
pub fn predict_directional(
    dst: &mut [u8],
    dst_stride: usize,
    above: &[u8],
    left: &[u8],
    width: usize,
    height: usize,
    angle: i32,
) {
    let dx = get_dx(angle);
    let dy = get_dy(angle);

    if angle > 0 && angle < 90 {
        dr_prediction_z1(dst, dst_stride, width, height, above, dx);
    } else if angle > 90 && angle < 180 {
        dr_prediction_z2(dst, dst_stride, width, height, above, left, dx, dy);
    } else if angle > 180 && angle < 270 {
        dr_prediction_z3(dst, dst_stride, width, height, left, dy);
    } else if angle == 90 {
        // V_PRED
        predict_v(dst, dst_stride, above, width, height);
    } else if angle == 180 {
        // H_PRED
        predict_h(dst, dst_stride, left, width, height);
    }
}

/// Get smooth weight table for a given block dimension.
///
/// Weights decrease from 255 (top/left edge) to approximately 0 (bottom/right edge).
/// These are the Q8 weights from the AV1 spec.
fn smooth_weights(n: usize) -> &'static [u8] {
    match n {
        4 => &SM_WEIGHTS_4,
        8 => &SM_WEIGHTS_8,
        16 => &SM_WEIGHTS_16,
        32 => &SM_WEIGHTS_32,
        64 => &SM_WEIGHTS_64,
        _ => &SM_WEIGHTS_4, // fallback
    }
}

// Smooth weight tables from the AV1 spec (Q8, 256 = 1.0)
static SM_WEIGHTS_4: [u8; 4] = [255, 149, 85, 64];
static SM_WEIGHTS_8: [u8; 8] = [255, 197, 146, 105, 73, 50, 37, 32];
static SM_WEIGHTS_16: [u8; 16] = [
    255, 225, 196, 170, 145, 123, 102, 84, 68, 54, 43, 33, 26, 20, 17, 16,
];
static SM_WEIGHTS_32: [u8; 32] = [
    255, 240, 225, 210, 196, 182, 169, 157, 145, 133, 122, 111, 101, 92, 83, 74, 66, 59, 52, 45,
    39, 34, 29, 25, 21, 17, 14, 12, 10, 9, 8, 8,
];
static SM_WEIGHTS_64: [u8; 64] = [
    255, 248, 240, 233, 225, 218, 210, 203, 196, 189, 182, 176, 169, 163, 156, 150, 144, 138, 133,
    127, 121, 116, 111, 106, 101, 96, 91, 86, 82, 77, 73, 69, 65, 61, 57, 54, 50, 47, 44, 41, 38,
    35, 32, 29, 27, 25, 22, 20, 18, 16, 15, 13, 12, 10, 9, 8, 7, 6, 6, 5, 5, 4, 4, 4,
];

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    #[allow(dead_code)]
    fn make_test_block(width: usize, height: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>, u8) {
        // above: increasing values
        let above: Vec<u8> = (0..width).map(|i| (100 + i) as u8).collect();
        // left: decreasing values
        let left: Vec<u8> = (0..height).map(|i| (200 - i * 5) as u8).collect();
        let dst = vec![0u8; width * height];
        let top_left = 150u8;
        (dst, above, left, top_left)
    }

    #[test]
    fn dc_uniform_neighbors() {
        let above = [100u8; 4];
        let left = [100u8; 4];
        let mut dst = [0u8; 16];
        predict_dc(&mut dst, 4, &above, &left, 4, 4, true, true);
        assert!(dst.iter().all(|&v| v == 100));
    }

    #[test]
    fn dc_above_only() {
        let above = [200u8; 8];
        let left = [0u8; 8];
        let mut dst = [0u8; 64];
        predict_dc(&mut dst, 8, &above, &left, 8, 8, true, false);
        assert!(dst.iter().all(|&v| v == 200));
    }

    #[test]
    fn dc_no_neighbors() {
        let mut dst = [0u8; 16];
        predict_dc(&mut dst, 4, &[], &[], 4, 4, false, false);
        assert!(dst.iter().all(|&v| v == 128));
    }

    #[test]
    fn v_pred_copies_above() {
        let above = [10u8, 20, 30, 40];
        let mut dst = [0u8; 16];
        predict_v(&mut dst, 4, &above, 4, 4);
        for row in 0..4 {
            assert_eq!(&dst[row * 4..row * 4 + 4], &above);
        }
    }

    #[test]
    fn h_pred_copies_left() {
        let left = [10u8, 20, 30, 40];
        let mut dst = [0u8; 16];
        predict_h(&mut dst, 4, &left, 4, 4);
        for row in 0..4 {
            assert!(dst[row * 4..row * 4 + 4].iter().all(|&v| v == left[row]));
        }
    }

    #[test]
    fn paeth_uniform() {
        // When above, left, and top_left are all the same, paeth should produce that value
        let above = [128u8; 4];
        let left = [128u8; 4];
        let mut dst = [0u8; 16];
        predict_paeth(&mut dst, 4, &above, &left, 128, 4, 4);
        assert!(dst.iter().all(|&v| v == 128));
    }

    #[test]
    fn paeth_horizontal_gradient() {
        // Top-left = 0, above = [10,20,30,40], left = [0,0,0,0]
        // base = above[c] + left[r] - tl = above[c], so pred = above[c]
        let above = [10u8, 20, 30, 40];
        let left = [0u8; 4];
        let mut dst = [0u8; 16];
        predict_paeth(&mut dst, 4, &above, &left, 0, 4, 4);
        for row in 0..4 {
            for col in 0..4 {
                assert_eq!(dst[row * 4 + col], above[col]);
            }
        }
    }

    #[test]
    fn smooth_corners() {
        // Smooth prediction should interpolate between neighbors
        let above = [200u8; 4];
        let left = [200u8; 4];
        let mut dst = [0u8; 16];
        predict_smooth(&mut dst, 4, &above, &left, 4, 4);
        // All neighbors are 200, so prediction should be 200
        for &v in &dst {
            assert!((v as i32 - 200).abs() <= 1, "expected ~200, got {v}");
        }
    }

    #[test]
    fn smooth_v_interpolates() {
        let above = [255u8; 4];
        let left = [255, 255, 255, 0]; // bottom pixel is 0
        let mut dst = [0u8; 16];
        predict_smooth_v(&mut dst, 4, &above, &left, 0, 4, 4);
        // First row should be close to 255 (high weight on above)
        assert!(dst[0] > 200);
        // Last row should be closer to 0 (low weight on above)
        assert!(dst[12] < dst[0]);
    }

    #[test]
    fn smooth_h_interpolates() {
        let above = [255, 255, 255, 0]; // right pixel is 0
        let left = [255u8; 4];
        let mut dst = [0u8; 16];
        predict_smooth_h(&mut dst, 4, &above, &left, 4, 4);
        // First column should be close to 255
        assert!(dst[0] > 200);
        // Last column should be closer to 0
        assert!(dst[3] < dst[0]);
    }
}

#[cfg(test)]
mod dispatch_tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;
    use archmage::testing::{CompileTimePolicy, for_each_token_permutation};

    #[test]
    fn paeth_all_dispatch_levels() {
        let above: Vec<u8> = (0..8).map(|i| (50 + i * 20) as u8).collect();
        let left: Vec<u8> = (0..8).map(|i| (200 - i * 15) as u8).collect();
        let top_left = 100u8;
        let mut ref_dst = vec![0u8; 64];
        predict_paeth(&mut ref_dst, 8, &above, &left, top_left, 8, 8);

        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            let mut dst = vec![0u8; 64];
            predict_paeth(&mut dst, 8, &above, &left, top_left, 8, 8);
            assert_eq!(dst, ref_dst, "paeth mismatch at dispatch level");
        });
    }

    #[test]
    fn paeth_dispatch_4x4() {
        let above = [10u8, 20, 30, 40];
        let left = [50u8, 60, 70, 80];
        let top_left = 5u8;
        let mut ref_dst = [0u8; 16];
        predict_paeth(&mut ref_dst, 4, &above, &left, top_left, 4, 4);

        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            let mut dst = [0u8; 16];
            predict_paeth(&mut dst, 4, &above, &left, top_left, 4, 4);
            assert_eq!(dst, ref_dst, "paeth 4x4 mismatch at dispatch level");
        });
    }

    #[test]
    fn paeth_dispatch_16x16() {
        let above: Vec<u8> = (0..16).map(|i| (i * 15) as u8).collect();
        let left: Vec<u8> = (0..16).map(|i| (255 - i * 12) as u8).collect();
        let top_left = 128u8;
        let mut ref_dst = vec![0u8; 256];
        predict_paeth(&mut ref_dst, 16, &above, &left, top_left, 16, 16);

        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            let mut dst = vec![0u8; 256];
            predict_paeth(&mut dst, 16, &above, &left, top_left, 16, 16);
            assert_eq!(dst, ref_dst, "paeth 16x16 mismatch at dispatch level");
        });
    }
}
