//! Super-resolution — 8-tap upscaling filter.
//!
//! Spec 08: Super-resolution 8-tap upscale filter.
//!
//! AV1 super-resolution encodes at a reduced horizontal resolution
//! then upscales in the decoder. This module implements the upscaling
//! filter used in the reconstruction loop.
//!
//! Ported from SVT-AV1's restoration.c super-res functions.

/// Super-resolution upscale filter (8-tap, 16 phases).
/// From the AV1 spec Table 7-13.
static SUPERRES_FILTER: [[i16; 8]; 16] = [
    [0, 0, 0, 128, 0, 0, 0, 0],
    [0, 0, -1, 128, 2, -1, 0, 0],
    [0, 0, -3, 127, 5, -2, 1, 0],
    [0, 0, -4, 126, 8, -3, 1, 0],
    [0, -1, -5, 125, 11, -4, 2, 0],
    [0, -1, -6, 123, 15, -6, 3, 0],
    [0, -1, -7, 121, 18, -7, 4, 0],
    [0, -1, -7, 118, 22, -8, 4, 0],
    [0, -2, -8, 116, 25, -9, 5, 1],
    [0, -2, -8, 113, 29, -10, 6, 0],
    [0, -2, -8, 110, 32, -11, 7, 0],
    [0, -2, -9, 107, 36, -12, 8, 0],
    [0, -2, -9, 103, 40, -13, 9, 0],
    [0, -2, -9, 100, 43, -13, 9, 0],
    [0, -3, -9, 96, 47, -14, 10, 1],
    [0, -3, -9, 93, 50, -14, 10, 1],
];

/// Apply super-resolution upscaling to a single row of pixels.
///
/// Upscales `src` (width `src_width`) to `dst` (width `dst_width`)
/// using the AV1 8-tap interpolation filter.
pub fn superres_upscale_row(src: &[u8], src_width: usize, dst: &mut [u8], dst_width: usize) {
    if src_width == 0 || dst_width == 0 {
        return;
    }

    // Scale factor in Q14: (src_width << 14) / dst_width
    let scale = ((src_width as u64) << 14) / dst_width as u64;

    for c in 0..dst_width {
        // Map destination pixel to source position (Q14 fixed-point)
        let src_pos = (c as u64 * scale) as i64;
        let src_int = (src_pos >> 14) as i32;
        let sub_pel = ((src_pos >> 10) & 0xF) as usize; // 4-bit sub-pixel phase

        let filter = &SUPERRES_FILTER[sub_pel];

        let mut sum: i32 = 0;
        for k in 0..8 {
            let idx = (src_int + k as i32 - 3).clamp(0, src_width as i32 - 1) as usize;
            sum += src[idx] as i32 * filter[k] as i32;
        }

        dst[c] = ((sum + 64) >> 7).clamp(0, 255) as u8;
    }
}

/// Apply super-resolution upscaling to a full frame.
pub fn superres_upscale(
    src: &[u8],
    src_stride: usize,
    src_width: usize,
    dst: &mut [u8],
    dst_stride: usize,
    dst_width: usize,
    height: usize,
) {
    for r in 0..height {
        superres_upscale_row(
            &src[r * src_stride..],
            src_width,
            &mut dst[r * dst_stride..],
            dst_width,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn upscale_identity() {
        // Same width should be approximately identity
        let src = [100u8, 150, 200, 128];
        let mut dst = [0u8; 4];
        superres_upscale_row(&src, 4, &mut dst, 4);
        for i in 0..4 {
            assert!(
                (dst[i] as i32 - src[i] as i32).abs() <= 2,
                "identity upscale at {i}: src={} dst={}",
                src[i],
                dst[i]
            );
        }
    }

    #[test]
    fn upscale_2x() {
        let src = [0u8, 255, 0, 255];
        let mut dst = [0u8; 8];
        superres_upscale_row(&src, 4, &mut dst, 8);
        // Output should interpolate between 0 and 255
        assert!(dst[0] < 50); // Near first src pixel (0)
        assert!(dst[7] > 200); // Near last src pixel (255)
    }

    #[test]
    fn upscale_full_frame() {
        let src = vec![128u8; 4 * 4]; // 4x4
        let mut dst = vec![0u8; 8 * 4]; // 8x4 (2x horizontal upscale)
        superres_upscale(&src, 4, 4, &mut dst, 8, 8, 4);
        // Uniform input should produce uniform output
        for &v in &dst {
            assert!(
                (v as i32 - 128).abs() <= 1,
                "uniform upscale should produce ~128: {v}"
            );
        }
    }
}
