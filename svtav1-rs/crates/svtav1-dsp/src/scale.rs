//! Scaled inter prediction — reference scaling when dimensions differ.
//!
//! Spec 06: Scaled inter prediction when ref dimensions differ.
//!
//! When the reference frame has different dimensions than the current
//! frame (e.g., due to super-resolution or spatial scalability), the
//! reference is scaled before motion compensation.
//!
//! Ported from SVT-AV1's inter_prediction.c scaled convolution functions.

/// Scale factors for reference frame scaling.
#[derive(Debug, Clone, Copy)]
pub struct ScaleFactors {
    /// Horizontal scale in Q14 fixed-point (ref_width << 14) / cur_width.
    pub x_scale: i32,
    /// Vertical scale in Q14 fixed-point.
    pub y_scale: i32,
}

impl ScaleFactors {
    /// Compute scale factors from reference and current dimensions.
    pub fn new(ref_width: u32, ref_height: u32, cur_width: u32, cur_height: u32) -> Self {
        Self {
            x_scale: ((ref_width as i64 * (1 << 14)) / cur_width as i64) as i32,
            y_scale: ((ref_height as i64 * (1 << 14)) / cur_height as i64) as i32,
        }
    }

    /// Check if scaling is needed (not 1:1).
    pub fn is_scaled(&self) -> bool {
        self.x_scale != (1 << 14) || self.y_scale != (1 << 14)
    }

    /// Scale a horizontal coordinate.
    pub fn scale_x(&self, x: i32) -> i32 {
        ((x as i64 * self.x_scale as i64) >> 14) as i32
    }

    /// Scale a vertical coordinate.
    pub fn scale_y(&self, y: i32) -> i32 {
        ((y as i64 * self.y_scale as i64) >> 14) as i32
    }
}

/// Apply scaled inter prediction.
///
/// Copies from the reference frame with coordinate scaling,
/// then applies sub-pixel interpolation.
pub fn scaled_prediction(
    ref_pic: &[u8],
    ref_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    block_x: i32,
    block_y: i32,
    width: usize,
    height: usize,
    sf: &ScaleFactors,
    ref_width: usize,
    ref_height: usize,
) {
    let filters = &svtav1_tables::interp::SUB_PEL_FILTERS_8;
    const FILTER_CENTER: i32 = 3;

    for r in 0..height {
        for c in 0..width {
            // For Q14 scale factors, the fractional part after scaling
            // comes from the lower bits of the full-precision computation.
            // Recompute at higher precision for sub-pixel extraction.
            let full_x =
                (block_x + c as i32) as i64 * sf.x_scale as i64; // Q14 result
            let full_y =
                (block_y + r as i32) as i64 * sf.y_scale as i64;

            let int_x = (full_x >> 14) as i32;
            let int_y = (full_y >> 14) as i32;
            let frac_x = ((full_x - ((int_x as i64) << 14)) >> 10) as usize; // 4-bit
            let frac_y = ((full_y - ((int_y as i64) << 14)) >> 10) as usize;

            if frac_x == 0 && frac_y == 0 {
                // Integer position — no interpolation needed
                let rx = int_x.clamp(0, ref_width as i32 - 1) as usize;
                let ry = int_y.clamp(0, ref_height as i32 - 1) as usize;
                dst[r * dst_stride + c] = ref_pic[ry * ref_stride + rx];
            } else {
                // 8-tap separable interpolation
                let h_filter = &filters[frac_x.min(15)];
                let v_filter = &filters[frac_y.min(15)];

                let mut intermediate = [0i32; 8];
                for tap_row in 0..8i32 {
                    let ry = (int_y - FILTER_CENTER + tap_row)
                        .clamp(0, ref_height as i32 - 1) as usize;
                    let mut sum = 0i32;
                    for tap_col in 0..8i32 {
                        let rx = (int_x - FILTER_CENTER + tap_col)
                            .clamp(0, ref_width as i32 - 1) as usize;
                        sum +=
                            ref_pic[ry * ref_stride + rx] as i32 * h_filter[tap_col as usize] as i32;
                    }
                    intermediate[tap_row as usize] = (sum + 64) >> 7;
                }

                let mut vsum = 0i32;
                for tap in 0..8 {
                    vsum += intermediate[tap] * v_filter[tap] as i32;
                }
                dst[r * dst_stride + c] = ((vsum + 64) >> 7).clamp(0, 255) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    #[test]
    fn scale_factors_1to1() {
        let sf = ScaleFactors::new(1920, 1080, 1920, 1080);
        assert!(!sf.is_scaled());
        assert_eq!(sf.x_scale, 1 << 14);
    }

    #[test]
    fn scale_factors_2x() {
        let sf = ScaleFactors::new(3840, 2160, 1920, 1080);
        assert!(sf.is_scaled());
        assert_eq!(sf.x_scale, 2 << 14);
    }

    #[test]
    fn scaled_prediction_identity() {
        let ref_pic: Vec<u8> = (0..64).map(|i| (i * 4) as u8).collect();
        let mut dst = [0u8; 16];
        let sf = ScaleFactors::new(8, 8, 8, 8); // 1:1
        scaled_prediction(&ref_pic, 8, &mut dst, 4, 2, 2, 4, 4, &sf, 8, 8);
        // Should match ref at (2,2)
        assert_eq!(dst[0], ref_pic[2 * 8 + 2]);
    }

    #[test]
    fn scaled_prediction_2x_downscale() {
        let ref_pic = vec![128u8; 16 * 16]; // 16x16 reference
        let mut dst = [0u8; 64]; // 8x8 destination
        let sf = ScaleFactors::new(16, 16, 8, 8); // 2x downscale
        scaled_prediction(&ref_pic, 16, &mut dst, 8, 0, 0, 8, 8, &sf, 16, 16);
        // Uniform reference → uniform output
        assert!(dst.iter().all(|&v| v == 128));
    }
}
