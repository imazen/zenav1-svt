//! Overlapped Block Motion Compensation (OBMC).
//!
//! Spec 06: Overlapped block motion compensation blending.
//!
//! OBMC blends the prediction of the current block with predictions
//! from neighboring blocks to reduce blocking artifacts at boundaries.
//! Ported from SVT-AV1's inter_prediction.c OBMC functions.

/// OBMC blend mask weights for above neighbor overlap.
///
/// The mask smoothly transitions from the neighbor's prediction
/// (weight=64) at the boundary to the current block's prediction
/// (weight=0) over a few rows.
pub fn obmc_blend_above(
    dst: &mut [u8],
    dst_stride: usize,
    above_pred: &[u8],
    above_stride: usize,
    width: usize,
    height: usize,
    overlap: usize,
) {
    let masks = obmc_mask(overlap);
    for r in 0..overlap.min(height) {
        let mask = masks[r] as u32;
        for c in 0..width {
            let cur = dst[r * dst_stride + c] as u32;
            let above = above_pred[r * above_stride + c] as u32;
            // Blend: (cur * (64 - mask) + above * mask + 32) >> 6
            dst[r * dst_stride + c] = ((cur * (64 - mask) + above * mask + 32) >> 6) as u8;
        }
    }
}

/// OBMC blend mask weights for left neighbor overlap.
pub fn obmc_blend_left(
    dst: &mut [u8],
    dst_stride: usize,
    left_pred: &[u8],
    left_stride: usize,
    width: usize,
    height: usize,
    overlap: usize,
) {
    let masks = obmc_mask(overlap);
    for r in 0..height {
        for c in 0..overlap.min(width) {
            let mask = masks[c] as u32;
            let cur = dst[r * dst_stride + c] as u32;
            let left = left_pred[r * left_stride + c] as u32;
            dst[r * dst_stride + c] = ((cur * (64 - mask) + left * mask + 32) >> 6) as u8;
        }
    }
}

/// Get OBMC blend mask for a given overlap size.
/// Weights decrease from 64 (full neighbor) to 0 (full current).
fn obmc_mask(overlap: usize) -> &'static [u8] {
    match overlap {
        1 => &[64],
        2 => &[45, 19],
        4 => &[53, 32, 11, 4],
        8 => &[56, 45, 34, 25, 17, 11, 6, 3],
        16 => &[58, 52, 45, 39, 34, 28, 23, 19, 15, 11, 8, 6, 4, 3, 2, 1],
        32 => &[
            59, 55, 52, 48, 45, 41, 38, 35, 32, 29, 26, 23, 21, 18, 16, 14, 12, 10, 9, 7, 6, 5, 4,
            3, 3, 2, 2, 1, 1, 1, 0, 0,
        ],
        _ => &[32], // fallback
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obmc_blend_above_basic() {
        let mut dst = [100u8; 16]; // 4x4
        let above = [200u8; 16];
        obmc_blend_above(&mut dst, 4, &above, 4, 4, 4, 2);
        // First row should be heavily blended toward above (mask=45)
        // (100 * 19 + 200 * 45 + 32) >> 6 = (1900 + 9000 + 32) >> 6 = 10932 >> 6 = 170
        assert!(
            dst[0] > 150,
            "row 0 should be blended toward above: {}",
            dst[0]
        );
        // Row 2+ should be unchanged
        assert_eq!(dst[8], 100, "row 2 should be unchanged");
    }

    #[test]
    fn obmc_blend_left_basic() {
        let mut dst = [100u8; 16];
        let left = [200u8; 16];
        obmc_blend_left(&mut dst, 4, &left, 4, 4, 4, 2);
        // First column should be blended
        assert!(dst[0] > 150);
        // Column 2+ should be unchanged
        assert_eq!(dst[2], 100);
    }

    #[test]
    fn obmc_mask_sizes() {
        assert_eq!(obmc_mask(2).len(), 2);
        assert_eq!(obmc_mask(4).len(), 4);
        assert_eq!(obmc_mask(8).len(), 8);
        assert_eq!(obmc_mask(16).len(), 16);
        assert_eq!(obmc_mask(32).len(), 32);
    }
}
