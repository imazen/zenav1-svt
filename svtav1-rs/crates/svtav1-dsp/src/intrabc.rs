//! Intra Block Copy (IntraBC) — copy from elsewhere in the same frame.
//!
//! IntraBC allows a block to be predicted by copying from a previously
//! reconstructed region of the same frame. This is primarily useful for
//! screen content coding (text, UI elements, repeated patterns).
//!
//! Ported from SVT-AV1's IntraBcUtilTest.cc and inter_prediction.c.

use svtav1_types::motion::Mv;

/// Check if an IntraBC motion vector is valid.
///
/// The source block (at `block_x, block_y`) must not overlap with
/// the destination block when copied from `block_x + mv.x, block_y + mv.y`.
/// Also, the source region must have already been reconstructed.
pub fn is_valid_intrabc_mv(
    mv: Mv,
    block_x: i32,
    block_y: i32,
    block_w: i32,
    block_h: i32,
    frame_width: i32,
    frame_height: i32,
    sb_row: i32,
) -> bool {
    // Convert sub-pel MV to full-pel
    let src_x = block_x + (mv.x as i32 >> 3);
    let src_y = block_y + (mv.y as i32 >> 3);

    // Source must be within frame bounds
    if src_x < 0 || src_y < 0 || src_x + block_w > frame_width || src_y + block_h > frame_height {
        return false;
    }

    // Source must be in already-reconstructed area (above current SB row
    // or to the left in the current SB row)
    if src_y + block_h > sb_row * 64 + 64 {
        return false; // Source extends below current superblock row
    }

    // Source must not overlap with destination

    src_x + block_w <= block_x
        || src_x >= block_x + block_w
        || src_y + block_h <= block_y
        || src_y >= block_y + block_h
}

/// Apply IntraBC prediction — copy from reconstructed region of same frame.
pub fn predict_intrabc(
    recon: &[u8],
    recon_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    block_x: usize,
    block_y: usize,
    mv: Mv,
    width: usize,
    height: usize,
) {
    let src_x = (block_x as i32 + (mv.x as i32 >> 3)) as usize;
    let src_y = (block_y as i32 + (mv.y as i32 >> 3)) as usize;

    for r in 0..height {
        let src_offset = (src_y + r) * recon_stride + src_x;
        let dst_offset = r * dst_stride;
        dst[dst_offset..dst_offset + width].copy_from_slice(&recon[src_offset..src_offset + width]);
    }
}

/// Hash-based IntraBC search — find matching blocks using hash values.
///
/// Computes a hash for each 8x8 block in the reconstructed area and
/// searches for blocks that match the source block's hash.
pub fn hash_search_intrabc(
    src: &[u8],
    src_stride: usize,
    recon: &[u8],
    recon_stride: usize,
    block_x: usize,
    block_y: usize,
    block_w: usize,
    block_h: usize,
    frame_width: usize,
    frame_height: usize,
) -> Option<Mv> {
    // Simple hash: sum of all pixels
    let src_hash = compute_block_hash(src, src_stride, block_w, block_h);

    // Search previously reconstructed area
    let search_y_end = block_y; // Only search above current position
    let best_mv = Mv::ZERO;
    let best_sad = u32::MAX;

    // Search in 8x8 steps for efficiency
    let step = 8;
    for sy in (0..search_y_end).step_by(step) {
        for sx in (0..frame_width.saturating_sub(block_w)).step_by(step) {
            let cand_hash = compute_block_hash(
                &recon[sy * recon_stride + sx..],
                recon_stride,
                block_w.min(frame_width - sx),
                block_h.min(frame_height - sy),
            );

            // Quick hash check before full SAD
            if cand_hash.abs_diff(src_hash) > block_w as u32 * block_h as u32 * 4 {
                continue;
            }

            // Full SAD check
            let mut sad = 0u32;
            for r in 0..block_h.min(frame_height - sy) {
                for c in 0..block_w.min(frame_width - sx) {
                    let s = src[r * src_stride + c] as i32;
                    let ref_val = recon[(sy + r) * recon_stride + sx + c] as i32;
                    sad += (s - ref_val).unsigned_abs();
                }
            }

            if sad < best_sad {
                // MV in sub-pel units (multiply by 8)
                let mv_x = (sx as i32 - block_x as i32) * 8;
                let mv_y = (sy as i32 - block_y as i32) * 8;
                return Some(Mv {
                    x: mv_x as i16,
                    y: mv_y as i16,
                });
            }
        }
    }

    if best_sad < block_w as u32 * block_h as u32 * 2 {
        Some(best_mv)
    } else {
        None
    }
}

fn compute_block_hash(block: &[u8], stride: usize, width: usize, height: usize) -> u32 {
    let mut hash: u32 = 0;
    for r in 0..height {
        for c in 0..width {
            hash = hash.wrapping_add(block[r * stride + c] as u32);
        }
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn valid_intrabc_mv() {
        // Source above destination — should be valid
        let mv = Mv { x: 0, y: -64 }; // 8 full-pel up
        assert!(is_valid_intrabc_mv(mv, 32, 64, 8, 8, 128, 128, 1));
    }

    #[test]
    fn invalid_intrabc_overlap() {
        // Zero MV would mean source == destination — invalid
        let mv = Mv::ZERO;
        assert!(!is_valid_intrabc_mv(mv, 32, 32, 8, 8, 128, 128, 0));
    }

    #[test]
    fn invalid_intrabc_out_of_bounds() {
        // MV that puts source outside frame bounds
        let mv = Mv { x: -400, y: -400 }; // (-50, -50) full-pel
        // Source at (32-50, 32-50) = (-18, -18) — out of frame
        assert!(!is_valid_intrabc_mv(mv, 32, 32, 8, 8, 128, 128, 0));
    }

    #[test]
    fn predict_intrabc_copies() {
        let mut recon = vec![0u8; 16 * 16];
        // Place pattern at (0, 0)
        for r in 0..4 {
            for c in 0..4 {
                recon[r * 16 + c] = ((r * 4 + c) * 10) as u8;
            }
        }

        let mut dst = [0u8; 16];
        let mv = Mv { x: -64, y: -64 }; // (-8, -8) full-pel
        predict_intrabc(&recon, 16, &mut dst, 4, 8, 8, mv, 4, 4);

        // Should copy from (0, 0)
        assert_eq!(dst[0], recon[0]);
        assert_eq!(dst[5], recon[16 + 1]);
    }
}
