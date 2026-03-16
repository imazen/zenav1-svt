//! Motion estimation — hierarchical ME, full-pel search, sub-pel refinement.
//!
//! Ported from SVT-AV1's `motion_estimation.c` and `av1me.c`.

use svtav1_types::motion::Mv;

/// Search parameters for motion estimation.
#[derive(Debug, Clone, Copy)]
pub struct MeSearchParams {
    /// Search area width (half-width: search from -w to +w).
    pub search_area_width: u16,
    /// Search area height.
    pub search_area_height: u16,
    /// Whether to use hierarchical ME.
    pub use_hme: bool,
    /// Sub-pixel refinement level (0=off, 1=half, 2=quarter, 3=eighth).
    pub subpel_level: u8,
}

impl Default for MeSearchParams {
    fn default() -> Self {
        Self {
            search_area_width: 64,
            search_area_height: 64,
            use_hme: true,
            subpel_level: 3,
        }
    }
}

/// Result of motion estimation for a single block.
#[derive(Debug, Clone, Copy, Default)]
pub struct MeResult {
    /// Best motion vector found.
    pub mv: Mv,
    /// SAD/distortion at the best MV position.
    pub distortion: u32,
}

/// Full-pel integer motion search using SAD.
///
/// Searches a rectangular area around `center_mv` and returns the MV
/// with minimum SAD.
///
/// # Arguments
/// * `src` - Source block (block being encoded)
/// * `src_stride` - Source stride
/// * `ref_pic` - Reference picture buffer
/// * `ref_stride` - Reference stride
/// * `ref_origin_x`, `ref_origin_y` - Origin of the search area in ref
/// * `width`, `height` - Block dimensions
/// * `center_mv` - Initial MV to search around
/// * `search_w`, `search_h` - Search area half-dimensions
/// * `pic_width`, `pic_height` - Reference picture dimensions for bounds checking
pub fn full_pel_search(
    src: &[u8],
    src_stride: usize,
    ref_pic: &[u8],
    ref_stride: usize,
    ref_origin_x: i32,
    ref_origin_y: i32,
    width: usize,
    height: usize,
    center_mv: Mv,
    search_w: i32,
    search_h: i32,
    pic_width: usize,
    pic_height: usize,
) -> MeResult {
    let mut best = MeResult {
        mv: center_mv,
        distortion: u32::MAX,
    };

    // Convert center MV to full-pel
    let cx = (center_mv.x >> 3) as i32;
    let cy = (center_mv.y >> 3) as i32;

    for dy in -search_h..=search_h {
        for dx in -search_w..=search_w {
            let ref_x = ref_origin_x + cx + dx;
            let ref_y = ref_origin_y + cy + dy;

            // Bounds checking
            if ref_x < 0
                || ref_y < 0
                || (ref_x as usize + width) > pic_width
                || (ref_y as usize + height) > pic_height
            {
                continue;
            }

            let ref_offset = ref_y as usize * ref_stride + ref_x as usize;

            // Compute SAD
            let mut sad: u32 = 0;
            for row in 0..height {
                let s_off = row * src_stride;
                let r_off = ref_offset + row * ref_stride;
                for col in 0..width {
                    let s = src[s_off + col] as i32;
                    let r = ref_pic[r_off + col] as i32;
                    sad += (s - r).unsigned_abs();
                }
                // Early termination
                if sad >= best.distortion {
                    break;
                }
            }

            if sad < best.distortion {
                best.distortion = sad;
                best.mv = Mv {
                    x: ((cx + dx) * 8) as i16,
                    y: ((cy + dy) * 8) as i16,
                };
            }
        }
    }

    best
}

/// Half-pel sub-pixel refinement.
///
/// Refines a full-pel MV by checking 8 half-pel positions around it.
/// Uses interpolated reference (simple bilinear average for now).
pub fn half_pel_refine(
    src: &[u8],
    src_stride: usize,
    ref_pic: &[u8],
    ref_stride: usize,
    ref_origin_x: i32,
    ref_origin_y: i32,
    width: usize,
    height: usize,
    mv: Mv,
    pic_width: usize,
    pic_height: usize,
) -> MeResult {
    let mut best = MeResult {
        mv,
        distortion: u32::MAX,
    };

    // 9 positions: center + 8 neighbors at half-pel
    let offsets: [(i16, i16); 9] = [
        (0, 0),
        (-4, 0),
        (4, 0),
        (0, -4),
        (0, 4),
        (-4, -4),
        (4, -4),
        (-4, 4),
        (4, 4),
    ];

    for &(dx, dy) in &offsets {
        let test_mv = Mv {
            x: mv.x + dx,
            y: mv.y + dy,
        };

        // For now, round to nearest full-pel and compute SAD
        // (Real implementation would use filtered interpolation)
        let ref_x = ref_origin_x + ((test_mv.x as i32 + 4) >> 3);
        let ref_y = ref_origin_y + ((test_mv.y as i32 + 4) >> 3);

        if ref_x < 0
            || ref_y < 0
            || (ref_x as usize + width) > pic_width
            || (ref_y as usize + height) > pic_height
        {
            continue;
        }

        let ref_offset = ref_y as usize * ref_stride + ref_x as usize;
        let mut sad: u32 = 0;
        for row in 0..height {
            for col in 0..width {
                let s = src[row * src_stride + col] as i32;
                let r = ref_pic[ref_offset + row * ref_stride + col] as i32;
                sad += (s - r).unsigned_abs();
            }
        }

        if sad < best.distortion {
            best.distortion = sad;
            best.mv = test_mv;
        }
    }

    best
}

/// Hierarchical motion estimation (HME).
///
/// Performs coarse-to-fine search:
/// 1. Search on downsampled (quarter) reference → coarse MV
/// 2. Refine on half-resolution reference
/// 3. Refine on full-resolution reference
///
/// For now, this is a single-level full-pel search. Multi-level HME
/// will be added when downsampling infrastructure is available.
pub fn hierarchical_me(
    src: &[u8],
    src_stride: usize,
    ref_pic: &[u8],
    ref_stride: usize,
    block_x: i32,
    block_y: i32,
    width: usize,
    height: usize,
    params: &MeSearchParams,
    pic_width: usize,
    pic_height: usize,
) -> MeResult {
    // Level 0: full-pel search at full resolution
    let result = full_pel_search(
        src,
        src_stride,
        ref_pic,
        ref_stride,
        block_x,
        block_y,
        width,
        height,
        Mv::ZERO,
        params.search_area_width as i32,
        params.search_area_height as i32,
        pic_width,
        pic_height,
    );

    // Sub-pel refinement
    if params.subpel_level >= 1 {
        return half_pel_refine(
            src, src_stride, ref_pic, ref_stride, block_x, block_y, width, height, result.mv,
            pic_width, pic_height,
        );
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_frames(width: usize, height: usize) -> (Vec<u8>, Vec<u8>) {
        let src = vec![128u8; width * height];
        let ref_ = vec![128u8; width * height];
        (src, ref_)
    }

    #[test]
    fn zero_motion_identical_frames() {
        // Use non-uniform content so only (0,0) gives SAD=0
        let mut src = vec![0u8; 8 * 8];
        let mut ref_ = vec![0u8; 64 * 64];
        for r in 0..8 {
            for c in 0..8 {
                let val = ((r * 8 + c) * 7 + 42) as u8;
                src[r * 8 + c] = val;
                ref_[(16 + r) * 64 + (16 + c)] = val;
            }
        }
        let result = full_pel_search(&src, 8, &ref_, 64, 16, 16, 8, 8, Mv::ZERO, 8, 8, 64, 64);
        assert_eq!(result.distortion, 0);
        assert_eq!(result.mv, Mv::ZERO);
    }

    #[test]
    fn finds_shifted_block() {
        let width = 64;
        let height = 64;
        let mut ref_ = vec![0u8; width * height];
        let mut src = vec![0u8; 8 * 8];

        // Place a distinctive 8x8 pattern at (20, 20) in reference
        for r in 0..8 {
            for c in 0..8 {
                let val = ((r * 8 + c) * 3) as u8;
                ref_[(20 + r) * width + (20 + c)] = val;
                src[r * 8 + c] = val; // Same pattern in source
            }
        }

        // Search around (16, 16) — the actual pattern is at (20, 20)
        let result = full_pel_search(
            &src,
            8,
            &ref_,
            width,
            16,
            16,
            8,
            8,
            Mv::ZERO,
            8,
            8,
            width,
            height,
        );

        // MV should point to (4, 4) full-pel = (32, 32) sub-pel
        assert_eq!(result.distortion, 0);
        assert_eq!(result.mv.x, 4 * 8); // 4 full-pel = 32 sub-pel
        assert_eq!(result.mv.y, 4 * 8);
    }

    #[test]
    fn search_params_default() {
        let params = MeSearchParams::default();
        assert_eq!(params.search_area_width, 64);
        assert!(params.use_hme);
    }

    #[test]
    fn half_pel_refine_doesnt_worsen() {
        let (src, ref_) = make_test_frames(64, 64);
        let full_result =
            full_pel_search(&src, 64, &ref_, 64, 16, 16, 8, 8, Mv::ZERO, 4, 4, 64, 64);
        let half_result =
            half_pel_refine(&src, 64, &ref_, 64, 16, 16, 8, 8, full_result.mv, 64, 64);
        assert!(half_result.distortion <= full_result.distortion);
    }

    #[test]
    fn hierarchical_me_basic() {
        let (src, ref_) = make_test_frames(64, 64);
        let params = MeSearchParams {
            search_area_width: 8,
            search_area_height: 8,
            use_hme: false,
            subpel_level: 0,
        };
        let result = hierarchical_me(&src, 64, &ref_, 64, 16, 16, 8, 8, &params, 64, 64);
        assert_eq!(result.distortion, 0);
    }
}
