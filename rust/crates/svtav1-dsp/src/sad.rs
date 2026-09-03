//! Sum of Absolute Differences (SAD) computation.
//!
//! Spec 02 (motion-estimation.md): SAD for ME distortion metric.
//!
//! SAD is the most-called function in motion estimation — it measures
//! the distortion between a source block and a reference block.
//!
//! Ported from SVT-AV1's sad_calculation functions.
//! SIMD implementations use archmage for dispatch.

/// Compute SAD between two blocks of 8-bit pixels.
///
/// # Arguments
/// * `src` - Source block pixels (row-major)
/// * `src_stride` - Distance between source rows in bytes
/// * `ref_` - Reference block pixels (row-major)
/// * `ref_stride` - Distance between reference rows in bytes
/// * `width` - Block width in pixels
/// * `height` - Block height in pixels
///
/// This is a thin alias for [`crate::me_sad::block_sad`]. It used to carry its
/// OWN scalar / AVX2 / NEON arms, which made it a SECOND TRANSCRIPTION of the
/// same C kernel `me_sad` transcribes — the exact hazard
/// `docs/WORKING-ON-THIS.md` §4 records ("TWO transcriptions of the same C
/// function will diverge — grep before you write the second"). Pointing both
/// at one implementation removes the hazard, and it also gives this entry
/// point the `arm_v2` dotprod arm and the 8-wide arm it never had (its NEON
/// path fell entirely to scalar below 16 px wide).
pub fn sad(
    src: &[u8],
    src_stride: usize,
    ref_: &[u8],
    ref_stride: usize,
    width: usize,
    height: usize,
) -> u32 {
    crate::me_sad::block_sad(src, src_stride, ref_, ref_stride, width, height)
}

/// SAD for specific common block sizes — 8x8.
pub fn sad_8x8(src: &[u8], src_stride: usize, ref_: &[u8], ref_stride: usize) -> u32 {
    sad(src, src_stride, ref_, ref_stride, 8, 8)
}

/// SAD for specific common block sizes — 16x16.
pub fn sad_16x16(src: &[u8], src_stride: usize, ref_: &[u8], ref_stride: usize) -> u32 {
    sad(src, src_stride, ref_, ref_stride, 16, 16)
}

/// SAD for specific common block sizes — 32x32.
pub fn sad_32x32(src: &[u8], src_stride: usize, ref_: &[u8], ref_stride: usize) -> u32 {
    sad(src, src_stride, ref_, ref_stride, 32, 32)
}

/// SAD for specific common block sizes — 64x64.
pub fn sad_64x64(src: &[u8], src_stride: usize, ref_: &[u8], ref_stride: usize) -> u32 {
    sad(src, src_stride, ref_, ref_stride, 64, 64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sad_zero_for_identical() {
        let block = [128u8; 64 * 64];
        assert_eq!(sad(&block, 64, &block, 64, 8, 8), 0);
        assert_eq!(sad(&block, 64, &block, 64, 16, 16), 0);
        assert_eq!(sad(&block, 64, &block, 64, 32, 32), 0);
        assert_eq!(sad(&block, 64, &block, 64, 64, 64), 0);
    }

    #[test]
    fn sad_known_value_4x4() {
        let src = [10u8; 16];
        let ref_ = [20u8; 16];
        // Each pixel differs by 10, 16 pixels total => SAD = 160
        assert_eq!(sad(&src, 4, &ref_, 4, 4, 4), 160);
    }

    #[test]
    fn sad_known_value_8x8() {
        let mut src = [0u8; 64];
        let mut ref_ = [0u8; 64];
        for i in 0..64 {
            src[i] = (i * 3) as u8;
            ref_[i] = (i * 3 + 1) as u8;
        }
        // Each pixel differs by 1, 64 pixels => SAD = 64
        assert_eq!(sad(&src, 8, &ref_, 8, 8, 8), 64);
    }

    #[test]
    fn sad_max_difference() {
        let src = [0u8; 16];
        let ref_ = [255u8; 16];
        assert_eq!(sad(&src, 4, &ref_, 4, 4, 4), 255 * 16);
    }

    #[test]
    fn sad_with_stride() {
        // Source is embedded in a larger buffer with stride 16
        let mut src = [0u8; 16 * 4];
        let mut ref_ = [0u8; 16 * 4];
        for row in 0..4 {
            for col in 0..4 {
                src[row * 16 + col] = 100;
                ref_[row * 16 + col] = 110;
            }
        }
        assert_eq!(sad(&src, 16, &ref_, 16, 4, 4), 10 * 16);
    }

    #[test]
    fn sad_convenience_functions() {
        let block = [42u8; 64 * 64];
        assert_eq!(sad_8x8(&block, 64, &block, 64), 0);
        assert_eq!(sad_16x16(&block, 64, &block, 64), 0);
        assert_eq!(sad_32x32(&block, 64, &block, 64), 0);
        assert_eq!(sad_64x64(&block, 64, &block, 64), 0);
    }
}

#[cfg(test)]
mod dispatch_tests {
    use super::*;

    use alloc::vec::Vec;
    use archmage::testing::{CompileTimePolicy, for_each_token_permutation};

    #[test]
    fn sad_all_dispatch_levels() {
        let src: Vec<u8> = (0..256).map(|i| (i * 7 + 13) as u8).collect();
        let ref_: Vec<u8> = (0..256).map(|i| (i * 11 + 29) as u8).collect();

        for size in [(4, 4), (8, 8), (16, 16)] {
            let reference = sad(&src, 16, &ref_, 16, size.0, size.1);
            let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
                let result = sad(&src, 16, &ref_, 16, size.0, size.1);
                assert_eq!(result, reference, "sad {}x{} mismatch", size.0, size.1);
            });
        }
    }
}
