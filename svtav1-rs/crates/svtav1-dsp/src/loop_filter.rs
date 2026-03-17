//! Loop filters: deblocking and CDEF.
//!
//! Spec 08 (loop-filters.md): Deblocking, CDEF, Wiener, sgrproj.
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

/// Apply wide (8-tap) deblocking filter to a vertical edge.
///
/// AV1 spec Section 7.14.5: Modifies p2..p0 and q0..q2 (6 pixels) using
/// an 8-pixel window (p3..p0, q0..q3). Used for strong edges between
/// blocks with different prediction modes or at SB boundaries.
pub fn deblock_vert_wide(
    pixels: &mut [u8],
    stride: usize,
    strength: i32,
    threshold: i32,
    edge_col: usize,
    height: usize,
) {
    if strength == 0 || edge_col < 4 {
        return;
    }
    for row in 0..height {
        let base = row * stride;
        if base + edge_col + 4 > pixels.len() {
            break;
        }
        let p3 = pixels[base + edge_col - 4] as i32;
        let p2 = pixels[base + edge_col - 3] as i32;
        let p1 = pixels[base + edge_col - 2] as i32;
        let p0 = pixels[base + edge_col - 1] as i32;
        let q0 = pixels[base + edge_col] as i32;
        let q1 = pixels[base + edge_col + 1] as i32;
        let q2 = pixels[base + edge_col + 2] as i32;
        let q3 = pixels[base + edge_col + 3] as i32;

        // Flatness check: skip if edge is already smooth
        if (p0 - q0).abs() * 2 + ((p1 - q1).abs() >> 1) > threshold {
            continue;
        }

        // 8-tap wide filter (AV1 spec lpf_8)
        let p2_new = (p3 + p3 + p3 + 2 * p2 + p1 + p0 + q0 + 4) >> 3;
        let p1_new = (p3 + p3 + p2 + 2 * p1 + p0 + q0 + q1 + 4) >> 3;
        let p0_new = (p3 + p2 + p1 + 2 * p0 + q0 + q1 + q2 + 4) >> 3;
        let q0_new = (p2 + p1 + p0 + 2 * q0 + q1 + q2 + q3 + 4) >> 3;
        let q1_new = (p1 + p0 + q0 + 2 * q1 + q2 + q3 + q3 + 4) >> 3;
        let q2_new = (p0 + q0 + q1 + 2 * q2 + q3 + q3 + q3 + 4) >> 3;

        pixels[base + edge_col - 3] = p2_new.clamp(0, 255) as u8;
        pixels[base + edge_col - 2] = p1_new.clamp(0, 255) as u8;
        pixels[base + edge_col - 1] = p0_new.clamp(0, 255) as u8;
        pixels[base + edge_col] = q0_new.clamp(0, 255) as u8;
        pixels[base + edge_col + 1] = q1_new.clamp(0, 255) as u8;
        pixels[base + edge_col + 2] = q2_new.clamp(0, 255) as u8;
    }
}

/// Apply wide (8-tap) deblocking filter to a horizontal edge.
pub fn deblock_horz_wide(
    pixels: &mut [u8],
    stride: usize,
    strength: i32,
    threshold: i32,
    edge_row: usize,
    width: usize,
) {
    if strength == 0 || edge_row < 4 {
        return;
    }
    for col in 0..width {
        let r3 = edge_row - 4;
        let r2 = edge_row - 3;
        let r1 = edge_row - 2;
        let r0 = edge_row - 1;
        if (edge_row + 3) * stride + col >= pixels.len() {
            break;
        }
        let p3 = pixels[r3 * stride + col] as i32;
        let p2 = pixels[r2 * stride + col] as i32;
        let p1 = pixels[r1 * stride + col] as i32;
        let p0 = pixels[r0 * stride + col] as i32;
        let q0 = pixels[edge_row * stride + col] as i32;
        let q1 = pixels[(edge_row + 1) * stride + col] as i32;
        let q2 = pixels[(edge_row + 2) * stride + col] as i32;
        let q3 = pixels[(edge_row + 3) * stride + col] as i32;

        if (p0 - q0).abs() * 2 + ((p1 - q1).abs() >> 1) > threshold {
            continue;
        }

        let p2_new = (p3 + p3 + p3 + 2 * p2 + p1 + p0 + q0 + 4) >> 3;
        let p1_new = (p3 + p3 + p2 + 2 * p1 + p0 + q0 + q1 + 4) >> 3;
        let p0_new = (p3 + p2 + p1 + 2 * p0 + q0 + q1 + q2 + 4) >> 3;
        let q0_new = (p2 + p1 + p0 + 2 * q0 + q1 + q2 + q3 + 4) >> 3;
        let q1_new = (p1 + p0 + q0 + 2 * q1 + q2 + q3 + q3 + 4) >> 3;
        let q2_new = (p0 + q0 + q1 + 2 * q2 + q3 + q3 + q3 + 4) >> 3;

        pixels[r2 * stride + col] = p2_new.clamp(0, 255) as u8;
        pixels[r1 * stride + col] = p1_new.clamp(0, 255) as u8;
        pixels[r0 * stride + col] = p0_new.clamp(0, 255) as u8;
        pixels[edge_row * stride + col] = q0_new.clamp(0, 255) as u8;
        pixels[(edge_row + 1) * stride + col] = q1_new.clamp(0, 255) as u8;
        pixels[(edge_row + 2) * stride + col] = q2_new.clamp(0, 255) as u8;
    }
}

/// Apply 14-tap deblocking filter to a vertical edge (strongest filter).
///
/// AV1 spec Section 7.14.6: Modifies p5..p0 and q0..q5 (12 pixels) using
/// a 14-pixel window. Used for the strongest edges (high QP SB boundaries).
pub fn deblock_vert_14tap(
    pixels: &mut [u8],
    stride: usize,
    strength: i32,
    threshold: i32,
    edge_col: usize,
    height: usize,
) {
    if strength == 0 || edge_col < 7 {
        return;
    }
    for row in 0..height {
        let base = row * stride;
        if base + edge_col + 7 > pixels.len() {
            break;
        }
        let p = |i: usize| pixels[base + edge_col - 1 - i] as i32;
        let q = |i: usize| pixels[base + edge_col + i] as i32;

        // Flatness check
        if (p(0) - q(0)).abs() * 2 + ((p(1) - q(1)).abs() >> 1) > threshold {
            continue;
        }

        // 14-tap wide filter: weighted average centered on the edge
        let p6 = p(6);
        let p5 = p(5);
        let p4 = p(4);
        let p3 = p(3);
        let p2 = p(2);
        let p1 = p(1);
        let p0 = p(0);
        let q0 = q(0);
        let q1 = q(1);
        let q2 = q(2);
        let q3 = q(3);
        let q4 = q(4);
        let q5 = q(5);
        let q6 = q(6);

        // lpf_14: averages progressively centered on the edge
        let f = |vals: &[i32]| -> u8 {
            let s: i32 = vals.iter().sum();
            ((s + vals.len() as i32 / 2) / vals.len() as i32).clamp(0, 255) as u8
        };

        pixels[base + edge_col - 6] =
            f(&[p6, p6, p6, p6, p6, p6, p5, p4, p3, p2, p1, p0, q0]);
        pixels[base + edge_col - 5] =
            f(&[p6, p6, p6, p6, p6, p5, p4, p3, p2, p1, p0, q0, q1]);
        pixels[base + edge_col - 4] = f(&[p6, p6, p6, p6, p5, p4, p3, p2, p1, p0, q0, q1, q2]);
        pixels[base + edge_col - 3] = f(&[p6, p6, p6, p5, p4, p3, p2, p1, p0, q0, q1, q2, q3]);
        pixels[base + edge_col - 2] =
            f(&[p6, p6, p5, p4, p3, p2, p1, p0, q0, q1, q2, q3, q4]);
        pixels[base + edge_col - 1] =
            f(&[p6, p5, p4, p3, p2, p1, p0, q0, q1, q2, q3, q4, q5]);
        pixels[base + edge_col] = f(&[p5, p4, p3, p2, p1, p0, q0, q1, q2, q3, q4, q5, q6]);
        pixels[base + edge_col + 1] =
            f(&[p4, p3, p2, p1, p0, q0, q1, q2, q3, q4, q5, q6, q6]);
        pixels[base + edge_col + 2] =
            f(&[p3, p2, p1, p0, q0, q1, q2, q3, q4, q5, q6, q6, q6]);
        pixels[base + edge_col + 3] = f(&[p2, p1, p0, q0, q1, q2, q3, q4, q5, q6, q6, q6, q6]);
        pixels[base + edge_col + 4] =
            f(&[p1, p0, q0, q1, q2, q3, q4, q5, q6, q6, q6, q6, q6]);
        pixels[base + edge_col + 5] =
            f(&[p0, q0, q1, q2, q3, q4, q5, q6, q6, q6, q6, q6, q6]);
    }
}

/// Derive deblocking filter strength from QP per the AV1 spec.
///
/// Returns (filter_level, threshold) for a given QP.
/// Higher QP → stronger filtering to reduce blocking artifacts.
pub fn derive_deblock_strength(qp: u8) -> (i32, i32) {
    // AV1 spec table approximation: filter level scales with QP
    let level = match qp {
        0..=15 => qp as i32,
        16..=31 => 15 + (qp as i32 - 15) * 2,
        32..=63 => 47 + (qp as i32 - 31),
        _ => 63,
    };
    let threshold = (level * 2 + 4).min(127);
    (level, threshold)
}

/// Select deblock filter width based on edge type and QP.
///
/// Returns the recommended filter tap count: 4, 8, or 14.
pub fn select_deblock_filter_size(is_sb_boundary: bool, qp: u8) -> u8 {
    if is_sb_boundary && qp >= 20 {
        14
    } else if is_sb_boundary || qp >= 10 {
        8
    } else {
        4
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

// =============================================================================
// Wiener restoration filter
// Ported from restoration.c — 7-tap separable symmetric filter
// =============================================================================

/// Apply Wiener restoration filter to a block.
///
/// The Wiener filter is a 7-tap separable symmetric filter applied
/// horizontally then vertically. Coefficients are signaled in the bitstream.
///
/// `coeffs`: [3] symmetric filter coefficients (the center tap is derived).
/// The full 7-tap kernel is: [c2, c1, c0, center, c0, c1, c2]
/// where center = 128 - 2*(c0 + c1 + c2)
pub fn wiener_filter(
    src: &[u8],
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    width: usize,
    height: usize,
    h_coeffs: [i16; 3],
    v_coeffs: [i16; 3],
) {
    // Build full 7-tap kernels
    let h_tap = build_wiener_kernel(h_coeffs);
    let v_tap = build_wiener_kernel(v_coeffs);

    // Intermediate buffer (i16 to avoid overflow)
    let mut tmp = alloc::vec![0i16; width * (height + 6)];

    // Horizontal pass: src → tmp (with 3-pixel border)
    let pad = 3;
    for r in 0..height + 2 * pad {
        let src_r = (r as i32 - pad as i32).clamp(0, height as i32 - 1) as usize;
        for c in 0..width {
            let mut sum: i32 = 0;
            for k in 0..7 {
                let sc = (c as i32 + k as i32 - 3).clamp(0, width as i32 - 1) as usize;
                sum += src[src_r * src_stride + sc] as i32 * h_tap[k] as i32;
            }
            // Round to preserve precision: (sum + 64) >> 7, but keep as i16
            tmp[r * width + c] = ((sum + (1 << 6)) >> 7) as i16;
        }
    }

    // Vertical pass: tmp → dst
    for r in 0..height {
        for c in 0..width {
            let mut sum: i32 = 0;
            for k in 0..7 {
                sum += tmp[(r + k) * width + c] as i32 * v_tap[k] as i32;
            }
            dst[r * dst_stride + c] = ((sum + (1 << 6)) >> 7).clamp(0, 255) as u8;
        }
    }
}

/// Build a 7-tap symmetric Wiener kernel from 3 coefficients.
fn build_wiener_kernel(coeffs: [i16; 3]) -> [i16; 7] {
    let center = 128 - 2 * (coeffs[0] + coeffs[1] + coeffs[2]);
    [
        coeffs[2], coeffs[1], coeffs[0], center, coeffs[0], coeffs[1], coeffs[2],
    ]
}

/// Find optimal Wiener filter coefficients by searching over the coefficient space.
///
/// Compares the filtered reconstruction against the original source to
/// minimize SSE. Tests a range of coefficient values and returns the best set.
///
/// This replaces the QP-based heuristic with per-restoration-unit optimization
/// (simplified RDO for Wiener coefficients).
pub fn optimize_wiener_coefficients(
    source: &[u8],
    src_stride: usize,
    degraded: &[u8],
    deg_stride: usize,
    width: usize,
    height: usize,
) -> ([i16; 3], [i16; 3]) {
    let mut best_sse = u64::MAX;
    let mut best_h = [0i16; 3];
    let mut best_v = [0i16; 3];

    // Search range: spec allows coefficients in [-5, 10] for outer taps
    // and larger range for inner taps. We search a practical subset.
    let search_vals: &[i16] = &[0, 1, 2, 3, 4, 5, 6, 8];

    // Simplified search: try symmetric h == v coefficients first (most common)
    let mut tmp_dst = alloc::vec![0u8; width * height];
    for &c0 in search_vals {
        for &c1 in &[0i16, 1, 2, 3, 4] {
            for &c2 in &[0i16, 1, 2] {
                let h_coeffs = [c0, c1, c2];
                // Verify kernel sums to 128 (center = 128 - 2*(c0+c1+c2))
                let center = 128 - 2 * (c0 + c1 + c2);
                if !(0..=128).contains(&center) {
                    continue;
                }

                wiener_filter(
                    degraded,
                    deg_stride,
                    &mut tmp_dst,
                    width,
                    width,
                    height,
                    h_coeffs,
                    h_coeffs, // symmetric: h == v
                );

                // Compute SSE against source
                let mut sse: u64 = 0;
                for r in 0..height {
                    for c in 0..width {
                        let s = source[r * src_stride + c] as i64;
                        let d = tmp_dst[r * width + c] as i64;
                        sse += ((s - d) * (s - d)) as u64;
                    }
                }

                if sse < best_sse {
                    best_sse = sse;
                    best_h = h_coeffs;
                    best_v = h_coeffs;
                }
            }
        }
    }

    (best_h, best_v)
}

// =============================================================================
// Self-guided restoration filter (sgrproj)
// Ported from restoration.c — guided filter with box sums
// =============================================================================

/// Self-guided restoration filter parameters.
#[derive(Debug, Clone, Copy)]
pub struct SgrprojParams {
    /// Radius for pass 0 (0 = skip this pass).
    pub r0: u8,
    /// Radius for pass 1 (0 = skip this pass).
    pub r1: u8,
    /// Strength parameter for pass 0 (sgr_params[set_idx].s[0]).
    pub s0: i32,
    /// Strength parameter for pass 1.
    pub s1: i32,
    /// Mixing weights: output = w0 * pass0 + w1 * pass1 + (1 - w0 - w1) * src
    pub xqd: [i32; 2],
}

/// Apply self-guided restoration filter to a block.
///
/// Uses box filtering with self-guided projection to denoise while
/// preserving edges. Two passes with different radii are blended.
pub fn sgrproj_filter(
    src: &[u8],
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    width: usize,
    height: usize,
    params: &SgrprojParams,
) {
    let mut flt0 = alloc::vec![0i32; width * height];
    let mut flt1 = alloc::vec![0i32; width * height];

    // Pass 0: box filter with radius r0
    if params.r0 > 0 {
        box_filter_sgr(
            src,
            src_stride,
            &mut flt0,
            width,
            height,
            params.r0 as usize,
            params.s0,
        );
    }

    // Pass 1: box filter with radius r1
    if params.r1 > 0 {
        box_filter_sgr(
            src,
            src_stride,
            &mut flt1,
            width,
            height,
            params.r1 as usize,
            params.s1,
        );
    }

    // Blend: dst = clip(w0 * flt0 + w1 * flt1 + (1 - w0 - w1) * src)
    let w0 = params.xqd[0];
    let w1 = params.xqd[1];
    let w_src = (1 << 7) - w0 - w1; // Weights sum to 128

    for r in 0..height {
        for c in 0..width {
            let idx = r * width + c;
            let s = src[r * src_stride + c] as i32;
            let f0 = if params.r0 > 0 { flt0[idx] } else { s << 4 };
            let f1 = if params.r1 > 0 { flt1[idx] } else { s << 4 };
            let val = (w0 * f0 + w1 * f1 + w_src * (s << 4) + (1 << 10)) >> 11;
            dst[r * dst_stride + c] = val.clamp(0, 255) as u8;
        }
    }
}

/// Box filter for self-guided restoration (single pass).
fn box_filter_sgr(
    src: &[u8],
    src_stride: usize,
    output: &mut [i32],
    width: usize,
    height: usize,
    radius: usize,
    strength: i32,
) {
    let n = (2 * radius + 1) * (2 * radius + 1);
    let n_inv = ((1 << 12) + n as i32 / 2) / n as i32; // Approximate 1/n in Q12

    // Build integral images (summed area tables) for O(1) box sums.
    // int_sum[r][c] = sum of src[0..r, 0..c] with edge clamping.
    // int_sq[r][c] = sum of src[0..r, 0..c]^2 with edge clamping.
    let pad = radius;
    let iw = width + 2 * pad;
    let ih = height + 2 * pad;
    let mut int_sum = alloc::vec![0i32; (ih + 1) * (iw + 1)];
    let mut int_sq = alloc::vec![0i64; (ih + 1) * (iw + 1)];
    let is = iw + 1; // integral image stride

    // Fill integral images with clamped source values
    for r in 0..ih {
        let sr = (r as i32 - pad as i32).clamp(0, height as i32 - 1) as usize;
        for c in 0..iw {
            let sc = (c as i32 - pad as i32).clamp(0, width as i32 - 1) as usize;
            let v = src[sr * src_stride + sc] as i32;
            let idx = (r + 1) * is + (c + 1);
            int_sum[idx] =
                v + int_sum[r * is + (c + 1)] + int_sum[(r + 1) * is + c] - int_sum[r * is + c];
            int_sq[idx] = v as i64 * v as i64 + int_sq[r * is + (c + 1)] + int_sq[(r + 1) * is + c]
                - int_sq[r * is + c];
        }
    }

    for r in 0..height {
        for c in 0..width {
            // Box sum via integral image: O(1) per pixel
            let r0 = r; // top-left of box in integral coords
            let c0 = c;
            let r1 = r + 2 * pad + 1; // bottom-right + 1
            let c1 = c + 2 * pad + 1;

            let sum = int_sum[r1 * is + c1] - int_sum[r0 * is + c1] - int_sum[r1 * is + c0]
                + int_sum[r0 * is + c0];
            let sum_sq = int_sq[r1 * is + c1] - int_sq[r0 * is + c1] - int_sq[r1 * is + c0]
                + int_sq[r0 * is + c0];

            // Compute variance-based weight
            let mean = (sum * n_inv + (1 << 11)) >> 12;
            let mean_sq = mean * mean;
            let sq_mean = ((sum_sq * n_inv as i64 + (1 << 11)) >> 12) as i32;
            let var = (sq_mean - mean_sq).max(0);

            // Self-guided: a = var / (var + strength), b = mean * (1 - a)
            let denom = var + strength;
            let a = if denom > 0 {
                (var << 12) / denom
            } else {
                1 << 12
            };
            let b = ((1 << 12) - a) * mean;

            let v = src[r * src_stride + c] as i32;
            output[r * width + c] = (a * v + b + (1 << 7)) >> 8;
        }
    }
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

        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
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

        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
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

        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            let mut result = base_pixels.clone();
            deblock_horz(&mut result, stride, 10, 10, 4, width);
            assert_eq!(
                result, reference,
                "deblock_horz mismatch at dispatch level {_perm}"
            );
        });
    }
}
