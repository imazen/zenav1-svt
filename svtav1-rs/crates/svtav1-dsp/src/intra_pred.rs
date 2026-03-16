//! Intra prediction modes.
//!
//! Ported from SVT-AV1's `intra_prediction.c` and `enc_intra_prediction.c`.
//!
//! AV1 defines 13 intra prediction modes: DC, V, H, 8 directional,
//! smooth/smooth_v/smooth_h, and paeth.

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
    for row in 0..height {
        for col in 0..width {
            let top = above[col] as i32;
            let lft = left[row] as i32;
            let tl = top_left as i32;

            // base = top + left - top_left
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
