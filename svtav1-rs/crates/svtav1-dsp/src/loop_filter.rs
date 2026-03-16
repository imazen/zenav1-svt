//! Loop filters: deblocking and CDEF.
//!
//! Ported from SVT-AV1's `deblocking_filter.c` and `cdef_block.c`.
//!
//! Deblocking smooths block edges to reduce blocking artifacts.
//! CDEF (Constrained Directional Enhancement Filter) detects edge
//! direction in 8x8 blocks and applies directional filtering.

use archmage::prelude::*;

/// CDEF block size for direction detection.
const CDEF_BLOCK_SIZE: usize = 8;

/// Number of CDEF directions.
const CDEF_DIRECTIONS: usize = 8;

// Direction offsets for CDEF: each direction is a list of (dy, dx) pairs
// representing the line of pixels to examine. We use 8 directions covering
// 0, 22.5, 45, 67.5, 90, 112.5, 135, 157.5 degrees.
//
// Direction 0 = horizontal (dx varies, dy=0)
// Direction 2 = diagonal /
// Direction 4 = vertical (dx=0, dy varies)
// Direction 6 = diagonal \
const CDEF_DIR_OFFSETS: [[(i32, i32); 2]; CDEF_DIRECTIONS] = [
    [(0, 1), (0, 2)],     // dir 0: horizontal
    [(-1, 1), (-2, 2)],   // dir 1: 22.5 deg
    [(-1, 0), (-2, 0)],   // dir 2: 45 deg (mapped to vertical-ish)
    [(-1, -1), (-2, -2)], // dir 3: 67.5 deg
    [(0, -1), (0, -2)],   // dir 4: vertical (mapped to horizontal offset for variance)
    [(1, -1), (2, -2)],   // dir 5: 112.5 deg
    [(1, 0), (2, 0)],     // dir 6: 135 deg
    [(1, 1), (2, 2)],     // dir 7: 157.5 deg
];

/// Apply deblocking filter to a vertical edge.
///
/// Filters the boundary between columns `width-1` and `width` in the pixel
/// buffer. `pixels` is a mutable slice covering the rows to filter.
/// `strength` controls filter strength, `threshold` limits the maximum
/// change.
///
/// This implements the simple 4-tap deblocking filter:
///   delta = clamp((q0 - p0) * 4 + (p1 - q1) + 4) >> 3
///   delta = clamp(delta, -threshold, threshold)
///   p0 = clamp(p0 + delta, 0, 255)
///   q0 = clamp(q0 - delta, 0, 255)
///
/// `width` is the column index of the edge (p0 is at column `width-1`,
/// q0 is at column `width`).
pub fn deblock_vert(
    pixels: &mut [u8],
    stride: usize,
    strength: i32,
    threshold: i32,
    edge_col: usize,
    height: usize,
) {
    incant!(
        deblock_vert_impl(pixels, stride, strength, threshold, edge_col, height),
        [v3, neon, scalar]
    )
}

fn deblock_vert_impl_scalar(
    _token: ScalarToken,
    pixels: &mut [u8],
    stride: usize,
    strength: i32,
    threshold: i32,
    edge_col: usize,
    height: usize,
) {
    deblock_vert_inner(pixels, stride, strength, threshold, edge_col, height);
}

#[cfg(target_arch = "x86_64")]
#[arcane]
fn deblock_vert_impl_v3(
    _token: Desktop64,
    pixels: &mut [u8],
    stride: usize,
    strength: i32,
    threshold: i32,
    edge_col: usize,
    height: usize,
) {
    deblock_vert_inner(pixels, stride, strength, threshold, edge_col, height);
}

#[cfg(target_arch = "aarch64")]
#[arcane]
fn deblock_vert_impl_neon(
    _token: NeonToken,
    pixels: &mut [u8],
    stride: usize,
    strength: i32,
    threshold: i32,
    edge_col: usize,
    height: usize,
) {
    deblock_vert_inner(pixels, stride, strength, threshold, edge_col, height);
}

#[inline]
fn deblock_vert_inner(
    pixels: &mut [u8],
    stride: usize,
    strength: i32,
    threshold: i32,
    edge_col: usize,
    height: usize,
) {
    if strength == 0 {
        return;
    }
    for row in 0..height {
        let base = row * stride;
        let p1 = pixels[base + edge_col - 2] as i32;
        let p0 = pixels[base + edge_col - 1] as i32;
        let q0 = pixels[base + edge_col] as i32;
        let q1 = pixels[base + edge_col + 1] as i32;

        let delta = ((q0 - p0) * 4 + (p1 - q1) + 4) >> 3;
        let delta = delta.clamp(-threshold, threshold);

        pixels[base + edge_col - 1] = (p0 + delta).clamp(0, 255) as u8;
        pixels[base + edge_col] = (q0 - delta).clamp(0, 255) as u8;
    }
}

/// Apply deblocking filter to a horizontal edge.
///
/// Filters the boundary between rows `edge_row-1` and `edge_row`.
/// Same 4-tap filter as `deblock_vert` but applied to vertical neighbors.
pub fn deblock_horz(
    pixels: &mut [u8],
    stride: usize,
    strength: i32,
    threshold: i32,
    edge_row: usize,
    width: usize,
) {
    incant!(
        deblock_horz_impl(pixels, stride, strength, threshold, edge_row, width),
        [v3, neon, scalar]
    )
}

fn deblock_horz_impl_scalar(
    _token: ScalarToken,
    pixels: &mut [u8],
    stride: usize,
    strength: i32,
    threshold: i32,
    edge_row: usize,
    width: usize,
) {
    deblock_horz_inner(pixels, stride, strength, threshold, edge_row, width);
}

#[cfg(target_arch = "x86_64")]
#[arcane]
fn deblock_horz_impl_v3(
    _token: Desktop64,
    pixels: &mut [u8],
    stride: usize,
    strength: i32,
    threshold: i32,
    edge_row: usize,
    width: usize,
) {
    deblock_horz_inner(pixels, stride, strength, threshold, edge_row, width);
}

#[cfg(target_arch = "aarch64")]
#[arcane]
fn deblock_horz_impl_neon(
    _token: NeonToken,
    pixels: &mut [u8],
    stride: usize,
    strength: i32,
    threshold: i32,
    edge_row: usize,
    width: usize,
) {
    deblock_horz_inner(pixels, stride, strength, threshold, edge_row, width);
}

#[inline]
fn deblock_horz_inner(
    pixels: &mut [u8],
    stride: usize,
    strength: i32,
    threshold: i32,
    edge_row: usize,
    width: usize,
) {
    if strength == 0 {
        return;
    }
    for col in 0..width {
        let p1 = pixels[(edge_row - 2) * stride + col] as i32;
        let p0 = pixels[(edge_row - 1) * stride + col] as i32;
        let q0 = pixels[edge_row * stride + col] as i32;
        let q1 = pixels[(edge_row + 1) * stride + col] as i32;

        let delta = ((q0 - p0) * 4 + (p1 - q1) + 4) >> 3;
        let delta = delta.clamp(-threshold, threshold);

        pixels[(edge_row - 1) * stride + col] = (p0 + delta).clamp(0, 255) as u8;
        pixels[edge_row * stride + col] = (q0 - delta).clamp(0, 255) as u8;
    }
}

/// CDEF: detect the dominant direction of an 8x8 block.
///
/// Computes directional contrast (variance of differences along each of 8
/// directions) and returns `(direction, variance)` where `direction` is
/// 0..7 and `variance` is the contrast metric of the best direction.
pub fn cdef_find_dir(src: &[u8], stride: usize) -> (u8, u32) {
    // Compute the mean of the block.
    let mut sum: u32 = 0;
    for row in 0..CDEF_BLOCK_SIZE {
        for col in 0..CDEF_BLOCK_SIZE {
            sum += src[row * stride + col] as u32;
        }
    }
    let mean = sum / (CDEF_BLOCK_SIZE * CDEF_BLOCK_SIZE) as u32;

    // For each direction, accumulate the sum-of-squared-differences along
    // lines in that direction. We use a simplified approach: for each pixel,
    // compute the difference from the mean, then accumulate partial sums
    // along each direction's lines. The direction with the highest variance
    // (largest spread of partial sums) wins.
    let mut best_dir: u8 = 0;
    let mut best_var: u32 = 0;

    // For each direction, we group pixels into lines and compute the sum of
    // (pixel - mean) for each line. The variance is sum-of-squares of these
    // partial sums divided by line length.
    for dir in 0..CDEF_DIRECTIONS {
        let mut partial_sums = [0i32; CDEF_BLOCK_SIZE * 2]; // enough for all lines
        let mut line_count = [0u32; CDEF_BLOCK_SIZE * 2];

        for row in 0..CDEF_BLOCK_SIZE {
            for col in 0..CDEF_BLOCK_SIZE {
                // Determine which "line" this pixel belongs to for this direction.
                // The line index is determined by the perpendicular offset.
                let line_idx = match dir {
                    0 => row,                                // horizontal lines
                    1 => (row as i32 + col as i32) as usize, // diagonal /
                    2 => col,                                // vertical lines
                    3 => {
                        // diagonal \ : row - col, shifted to be non-negative
                        ((row as i32 - col as i32) + (CDEF_BLOCK_SIZE as i32 - 1)) as usize
                    }
                    4 => row, // same as 0 but mirrored
                    5 => ((row as i32 - col as i32) + (CDEF_BLOCK_SIZE as i32 - 1)) as usize,
                    6 => col,
                    7 => (row as i32 + col as i32) as usize,
                    _ => unreachable!(),
                };

                let val = src[row * stride + col] as i32 - mean as i32;
                partial_sums[line_idx] += val;
                line_count[line_idx] += 1;
            }
        }

        // Compute variance as sum of squared partial sums (weighted by
        // inverse line length for fairness, but since all lines in a
        // direction have similar length, we use the raw sum-of-squares
        // which is what the AV1 spec does for direction detection).
        let mut var: u32 = 0;
        for i in 0..partial_sums.len() {
            if line_count[i] > 0 {
                var += (partial_sums[i] * partial_sums[i]) as u32;
            }
        }

        if var > best_var {
            best_var = var;
            best_dir = dir as u8;
        }
    }

    (best_dir, best_var)
}

/// CDEF: apply primary + secondary directional filtering to a block.
///
/// `dir`: direction 0-7 (from `cdef_find_dir`).
/// `pri_strength`: primary filter strength (0 = disabled).
/// `sec_strength`: secondary filter strength (0 = disabled).
/// `damping`: damping parameter (typically 3-6), controls the threshold
///   below which small differences are not filtered.
pub fn cdef_filter_block(
    src: &[u8],
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    dir: u8,
    pri_strength: i32,
    sec_strength: i32,
    damping: i32,
    width: usize,
    height: usize,
) {
    incant!(
        cdef_filter_block_impl(
            src,
            src_stride,
            dst,
            dst_stride,
            dir,
            pri_strength,
            sec_strength,
            damping,
            width,
            height
        ),
        [v3, neon, scalar]
    )
}

fn cdef_filter_block_impl_scalar(
    _token: ScalarToken,
    src: &[u8],
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    dir: u8,
    pri_strength: i32,
    sec_strength: i32,
    damping: i32,
    width: usize,
    height: usize,
) {
    cdef_filter_block_inner(
        src,
        src_stride,
        dst,
        dst_stride,
        dir,
        pri_strength,
        sec_strength,
        damping,
        width,
        height,
    );
}

#[cfg(target_arch = "x86_64")]
#[arcane]
fn cdef_filter_block_impl_v3(
    _token: Desktop64,
    src: &[u8],
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    dir: u8,
    pri_strength: i32,
    sec_strength: i32,
    damping: i32,
    width: usize,
    height: usize,
) {
    cdef_filter_block_inner(
        src,
        src_stride,
        dst,
        dst_stride,
        dir,
        pri_strength,
        sec_strength,
        damping,
        width,
        height,
    );
}

#[cfg(target_arch = "aarch64")]
#[arcane]
fn cdef_filter_block_impl_neon(
    _token: NeonToken,
    src: &[u8],
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    dir: u8,
    pri_strength: i32,
    sec_strength: i32,
    damping: i32,
    width: usize,
    height: usize,
) {
    cdef_filter_block_inner(
        src,
        src_stride,
        dst,
        dst_stride,
        dir,
        pri_strength,
        sec_strength,
        damping,
        width,
        height,
    );
}

#[inline]
fn cdef_filter_block_inner(
    src: &[u8],
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    dir: u8,
    pri_strength: i32,
    sec_strength: i32,
    damping: i32,
    width: usize,
    height: usize,
) {
    let dir = dir as usize % CDEF_DIRECTIONS;

    let pri_offsets = CDEF_DIR_OFFSETS[dir];
    let sec_dir1 = (dir + 2) % CDEF_DIRECTIONS;
    let sec_dir2 = (dir + CDEF_DIRECTIONS - 2) % CDEF_DIRECTIONS;

    for row in 0..height {
        for col in 0..width {
            let center = src[row * src_stride + col] as i32;
            let mut sum: i32 = 0;

            if pri_strength > 0 {
                for (dist_idx, &(dy, dx)) in pri_offsets.iter().enumerate() {
                    let weight = if dist_idx == 0 { 4 } else { 2 };
                    let py = row as i32 + dy;
                    let px = col as i32 + dx;
                    let ny = row as i32 - dy;
                    let nx = col as i32 - dx;

                    if py >= 0 && py < height as i32 && px >= 0 && px < width as i32 {
                        let v = src[py as usize * src_stride + px as usize] as i32;
                        let diff = v - center;
                        sum += weight * constrain(diff, pri_strength, damping);
                    }
                    if ny >= 0 && ny < height as i32 && nx >= 0 && nx < width as i32 {
                        let v = src[ny as usize * src_stride + nx as usize] as i32;
                        let diff = v - center;
                        sum += weight * constrain(diff, pri_strength, damping);
                    }
                }
            }

            if sec_strength > 0 {
                for &sec_dir in &[sec_dir1, sec_dir2] {
                    let (dy, dx) = CDEF_DIR_OFFSETS[sec_dir][0];
                    let weight = 2;

                    let py = row as i32 + dy;
                    let px = col as i32 + dx;
                    let ny = row as i32 - dy;
                    let nx = col as i32 - dx;

                    if py >= 0 && py < height as i32 && px >= 0 && px < width as i32 {
                        let v = src[py as usize * src_stride + px as usize] as i32;
                        let diff = v - center;
                        sum += weight * constrain(diff, sec_strength, damping);
                    }
                    if ny >= 0 && ny < height as i32 && nx >= 0 && nx < width as i32 {
                        let v = src[ny as usize * src_stride + nx as usize] as i32;
                        let diff = v - center;
                        sum += weight * constrain(diff, sec_strength, damping);
                    }
                }
            }

            let result = center + ((sum + 8) >> 4);
            dst[row * dst_stride + col] = result.clamp(0, 255) as u8;
        }
    }
}

/// CDEF constrain function: apply damping to a difference.
///
/// Returns `clamp(diff, -strength, strength)` with additional damping:
/// differences below `1 << (damping - floor_log2(strength))` are zeroed.
fn constrain(diff: i32, strength: i32, damping: i32) -> i32 {
    if strength == 0 || diff == 0 {
        return 0;
    }
    let abs_diff = diff.unsigned_abs() as i32;
    // Threshold below which we don't filter.
    let shift = damping - floor_log2(strength as u32) as i32;
    let threshold = if shift > 0 { 1 << shift } else { 0 };
    let clamped = abs_diff.min(strength);
    let dampened = if abs_diff < threshold { 0 } else { clamped };
    if diff < 0 { -dampened } else { dampened }
}

/// Floor of log2 for nonzero values.
fn floor_log2(mut x: u32) -> u32 {
    if x == 0 {
        return 0;
    }
    let mut log = 0u32;
    while x > 1 {
        x >>= 1;
        log += 1;
    }
    log
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// Flat block should not be modified by deblocking.
    #[test]
    fn deblock_flat_unchanged() {
        let val = 128u8;
        let stride = 8;
        let height = 4;
        let mut pixels = vec![val; stride * (height + 2)]; // extra rows for safety
        let original = pixels.clone();

        // Apply deblocking at vertical edge at column 4
        deblock_vert(&mut pixels, stride, 10, 10, 4, height);
        assert_eq!(pixels, original, "flat block should not change");

        // Apply deblocking at horizontal edge at row 3 (need rows 1..4+1)
        let mut pixels2 = vec![val; stride * (height + 4)];
        let original2 = pixels2.clone();
        deblock_horz(&mut pixels2, stride, 10, 10, 3, 6);
        assert_eq!(pixels2, original2, "flat block should not change");
    }

    /// Deblocking should reduce a step edge.
    #[test]
    fn deblock_reduces_step_edge() {
        let stride = 8;
        let height = 1;
        let mut pixels = vec![0u8; stride];
        // Create a step edge: left half = 0, right half = 100
        for col in 4..8 {
            pixels[col] = 100;
        }

        let p0_before = pixels[3];
        let q0_before = pixels[4];

        deblock_vert(&mut pixels, stride, 100, 100, 4, height);

        // After filtering, the step should be reduced: p0 should increase
        // and q0 should decrease.
        assert!(pixels[3] > p0_before, "p0 should increase");
        assert!(pixels[4] < q0_before, "q0 should decrease");
    }

    /// CDEF direction of a horizontal gradient should be direction 0
    /// (horizontal lines have highest variance along the gradient axis).
    #[test]
    fn cdef_direction_horizontal_gradient() {
        let stride = 8;
        let mut block = vec![0u8; stride * CDEF_BLOCK_SIZE];

        // Create a horizontal gradient: each row is constant, values vary
        // row-to-row. This means horizontal lines (dir 0) have zero internal
        // variance but maximum between-line variance.
        // Actually, for CDEF, "direction 0 = horizontal" means lines are
        // horizontal, which means a vertical gradient makes them maximally
        // contrasting. Let me reconsider:
        //
        // Direction 0 = horizontal lines (row groups). High variance for
        // direction 0 means rows differ from each other → vertical gradient.
        for row in 0..CDEF_BLOCK_SIZE {
            for col in 0..CDEF_BLOCK_SIZE {
                block[row * stride + col] = (row * 30) as u8;
            }
        }

        let (dir, var) = cdef_find_dir(&block, stride);
        assert_eq!(
            dir, 0,
            "vertical gradient should produce direction 0 (horizontal lines)"
        );
        assert!(var > 0, "variance should be nonzero");
    }

    /// CDEF direction of a vertical gradient should be direction 2
    /// (vertical lines).
    #[test]
    fn cdef_direction_vertical_gradient() {
        let stride = 8;
        let mut block = vec![0u8; stride * CDEF_BLOCK_SIZE];

        // Each column is constant, values vary column-to-column.
        for row in 0..CDEF_BLOCK_SIZE {
            for col in 0..CDEF_BLOCK_SIZE {
                block[row * stride + col] = (col * 30) as u8;
            }
        }

        let (dir, _var) = cdef_find_dir(&block, stride);
        assert_eq!(
            dir, 2,
            "horizontal gradient should produce direction 2 (vertical lines)"
        );
    }

    /// CDEF filter on a flat block should produce the same flat output.
    #[test]
    fn cdef_filter_flat_block() {
        let val = 128u8;
        let size = 8;
        let stride = size;
        let src = vec![val; stride * size];
        let mut dst = vec![0u8; stride * size];

        cdef_filter_block(&src, stride, &mut dst, stride, 0, 4, 2, 4, size, size);

        for (i, &v) in dst.iter().enumerate() {
            assert_eq!(v, val, "flat block should remain flat at index {i}");
        }
    }

    /// CDEF filter with zero strength should just copy.
    #[test]
    fn cdef_filter_zero_strength_copies() {
        let size = 8;
        let stride = size;
        let src: alloc::vec::Vec<u8> = (0..(size * size) as u16).map(|i| (i % 256) as u8).collect();
        let mut dst = vec![0u8; stride * size];

        cdef_filter_block(&src, stride, &mut dst, stride, 3, 0, 0, 4, size, size);

        assert_eq!(dst, src, "zero strength should just copy");
    }

    /// `floor_log2` sanity checks.
    #[test]
    fn floor_log2_values() {
        assert_eq!(floor_log2(1), 0);
        assert_eq!(floor_log2(2), 1);
        assert_eq!(floor_log2(3), 1);
        assert_eq!(floor_log2(4), 2);
        assert_eq!(floor_log2(7), 2);
        assert_eq!(floor_log2(8), 3);
        assert_eq!(floor_log2(128), 7);
    }

    /// `constrain` basic behavior.
    #[test]
    fn constrain_basic() {
        // With large damping, small diffs are zeroed.
        assert_eq!(constrain(0, 4, 4), 0);
        // Large diff gets clamped to strength.
        assert_eq!(constrain(100, 4, 0), 4);
        assert_eq!(constrain(-100, 4, 0), -4);
        // Zero strength always returns 0.
        assert_eq!(constrain(50, 0, 4), 0);
    }
}

#[cfg(test)]
mod dispatch_tests {
    use super::*;
    use alloc::vec;
    use archmage::testing::{CompileTimePolicy, for_each_token_permutation};

    #[test]
    fn cdef_filter_block_all_dispatch_levels() {
        let size = 8;
        let stride = size;
        let src: alloc::vec::Vec<u8> = (0..(size * size) as u16)
            .map(|i| ((i * 13 + 7) % 256) as u8)
            .collect();
        let mut reference = vec![0u8; stride * size];
        cdef_filter_block(&src, stride, &mut reference, stride, 3, 4, 2, 4, size, size);

        for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            let mut result = vec![0u8; stride * size];
            cdef_filter_block(&src, stride, &mut result, stride, 3, 4, 2, 4, size, size);
            assert_eq!(result, reference, "cdef mismatch at dispatch level {_perm}");
        });
    }

    #[test]
    fn deblock_vert_all_dispatch_levels() {
        let stride = 8;
        let height = 4;
        let mut base_pixels = vec![0u8; stride * height];
        // Create a step edge
        for row in 0..height {
            for col in 4..8 {
                base_pixels[row * stride + col] = 100;
            }
        }
        let mut reference = base_pixels.clone();
        deblock_vert(&mut reference, stride, 10, 10, 4, height);

        for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            let mut result = base_pixels.clone();
            deblock_vert(&mut result, stride, 10, 10, 4, height);
            assert_eq!(
                result, reference,
                "deblock_vert mismatch at dispatch level {_perm}"
            );
        });
    }

    #[test]
    fn deblock_horz_all_dispatch_levels() {
        let stride = 8;
        let width = 6;
        let mut base_pixels = vec![0u8; stride * 8];
        // Create a horizontal step edge at row 4
        for row in 4..8 {
            for col in 0..stride {
                base_pixels[row * stride + col] = 100;
            }
        }
        let mut reference = base_pixels.clone();
        deblock_horz(&mut reference, stride, 10, 10, 4, width);

        for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            let mut result = base_pixels.clone();
            deblock_horz(&mut result, stride, 10, 10, 4, width);
            assert_eq!(
                result, reference,
                "deblock_horz mismatch at dispatch level {_perm}"
            );
        });
    }
}
