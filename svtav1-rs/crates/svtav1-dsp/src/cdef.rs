//! CDEF (Constrained Directional Enhancement Filter) kernels, C-exact.
//!
//! Ported from SVT-AV1 `Source/Lib/Codec/cdef.c` (v4.2.0-rc):
//! - [`cdef_find_dir`]      = `svt_aom_cdef_find_dir_c`      (cdef.c:88)
//! - [`cdef_find_dir_8bit`] = `svt_aom_cdef_find_dir_8bit_c` (cdef.c:303)
//! - [`cdef_filter_block`]  = `svt_cdef_filter_block_c`, dst8 arm (cdef.c:193)
//! - [`cdef_filter_block_8bit`] = `svt_cdef_filter_block_8bit_c` (cdef.c:257)
//!
//! libaom's `cdef_find_dir_c` / `cdef_filter_block_internal`
//! (av1/common/cdef_block.c — what aomdec runs) are the same math with two
//! packaging differences, both proven output-neutral at 8 bit:
//!
//! 1. **Sentinel value**: SVT marks unavailable pixels with
//!    `CDEF_VERY_LARGE = 0x7f7f`; libaom uses `0x4000`. Both are
//!    constrain-neutral — for any threshold <= 63 and damping <= 6+shift the
//!    damped clamp `max(0, threshold - (|diff| >> shift))` is exactly 0 when
//!    `|diff| >= 0x4000 - 255` (16129 >> 6 = 252 > 63) — are excluded from
//!    `max` by an equality compare against the *same* constant, and being
//!    large positive can never win `min`. Identical availability geometry
//!    therefore yields bit-identical output for either constant.
//! 2. **Strength-index dispatch**: libaom routes (t, sec) through 4 kernel
//!    variants where the min/max clamp only exists when BOTH strengths are
//!    nonzero. The SVT kernel (ported here) always clamps. Equivalent: with
//!    one side disabled its constrain() terms are 0, and a single-side
//!    filtered value provably stays within [min, max] of its own live taps
//!    (total tap weight 12: `y - x <= (8 + 12*d) >> 4 <= d` for the largest
//!    positive live-tap diff `d`, symmetrically for min), so the extra clamp
//!    never fires; with both disabled the output is exactly `x` either way.
//!
//! Only the 8-bit (`dst8`, `coeff_shift = 0..`) arm of the filter is ported —
//! the pipeline is 8-bit-only (the C `dst16` arm serves HBD and the encoder's
//! packed-output RDO search, neither of which exists here). All kernels are
//! differentially fuzzed bit-exact against `libSvtAv1Enc.a` in
//! `tests/c_parity_cdef.rs`.

/// 6-bit packed strength: `pri * CDEF_SEC_STRENGTHS + sec` (spec 5.9.19).
pub const CDEF_STRENGTH_BITS: u32 = 6;
/// Number of primary strengths (4-bit field).
pub const CDEF_PRI_STRENGTHS: i32 = 16;
/// Number of *signaled* secondary strengths (2-bit field; 3 decodes as 4).
pub const CDEF_SEC_STRENGTHS: i32 = 4;

/// Rows buffered above/below a filter block (`CDEF_VBORDER`, cdef.h).
pub const CDEF_VBORDER: usize = 3;
/// Columns buffered left/right (`CDEF_HBORDER`, cdef.h — 8 for alignment;
/// taps only reach +-2).
pub const CDEF_HBORDER: usize = 8;
/// Padded row stride of the CDEF intermediate buffer:
/// `ALIGN_POWER_OF_TWO(128 + 2 * CDEF_HBORDER, 3)` = 144.
pub const CDEF_BSTRIDE: usize = 144;
/// Intermediate buffer size (covers a 128px superblock; we use 64px).
pub const CDEF_INBUF_SIZE: usize = CDEF_BSTRIDE * (128 + 2 * CDEF_VBORDER);

/// Unavailable-pixel sentinel, SVT convention (`(uint8_t)~0 >> 1 |
/// ((uint8_t)~0 >> 1) << 8` = 0x7f7f). libaom uses 0x4000; see the module
/// docs for the bit-exactness argument.
pub const CDEF_VERY_LARGE: u16 = 0x7f7f;

/// SVT/libaom `BLOCK_4X4` (definitions.h:924 — enum starts at 0).
pub const BLOCK_4X4: i32 = 0;
/// SVT/libaom `BLOCK_4X8`.
pub const BLOCK_4X8: i32 = 1;
/// SVT/libaom `BLOCK_8X4`.
pub const BLOCK_8X4: i32 = 2;
/// SVT/libaom `BLOCK_8X8`.
pub const BLOCK_8X8: i32 = 3;

/// `eb_cdef_directions_padded` (cdef.c:35): Cdef_Directions (spec 7.15.3)
/// with 2 padding entries at each end so `dir - 2 .. dir + 2` indexes without
/// masking. Offsets are into a `CDEF_BSTRIDE`-strided buffer.
const CDEF_DIRECTIONS_PADDED: [[i32; 2]; 12] = {
    const S: i32 = CDEF_BSTRIDE as i32;
    [
        /* padding: directions[6] */ [S, 2 * S],
        /* padding: directions[7] */ [S, 2 * S - 1],
        [-S + 1, -2 * S + 2],
        [1, -S + 2],
        [1, 2],
        [1, S + 2],
        [S + 1, 2 * S + 2],
        [S, 2 * S + 1],
        [S, 2 * S],
        [S, 2 * S - 1],
        /* padding: directions[0] */ [-S + 1, -2 * S + 2],
        /* padding: directions[1] */ [1, -S + 2],
    ]
};

/// `svt_aom_eb_cdef_directions[dir][k]` with the C `+2` base offset:
/// accepts `dir` in `-2..=9`.
#[inline]
fn cdef_direction(dir: i32, k: usize) -> i32 {
    CDEF_DIRECTIONS_PADDED[(dir + 2) as usize][k]
}

/// `svt_aom_eb_cdef_pri_taps` (cdef.c:189), row selected by
/// `(pri_strength >> coeff_shift) & 1`.
const CDEF_PRI_TAPS: [[i32; 2]; 2] = [[4, 2], [3, 3]];
/// `svt_aom_eb_cdef_sec_taps` (cdef.c:190) — both rows identical.
const CDEF_SEC_TAPS: [[i32; 2]; 2] = [[2, 1], [2, 1]];

/// C `get_msb` (definitions.h:603): `31 - clz(n)` = floor(log2(n)), n > 0.
#[inline]
fn get_msb(n: u32) -> i32 {
    debug_assert!(n != 0);
    31 - n.leading_zeros() as i32
}

/// C `constrain` (cdef.c:20): damped, sign-preserving tap clamp.
#[inline]
fn constrain(diff: i32, threshold: i32, damping: i32) -> i32 {
    if threshold == 0 {
        return 0;
    }
    let shift = (damping - get_msb(threshold as u32)).max(0);
    let sign = if diff < 0 { -1 } else { 1 };
    sign * diff.abs().min((threshold - (diff.abs() >> shift)).max(0))
}

/// C `adjust_strength` (cdef.c:66): scale the primary strength for a luma
/// 8x8 by its directional-variance class (`var` from [`cdef_find_dir`]).
#[inline]
pub fn adjust_strength(strength: i32, var: i32) -> i32 {
    let i = if var >> 6 != 0 {
        get_msb((var >> 6) as u32).min(12)
    } else {
        0
    };
    if var != 0 { (strength * (4 + i) + 8) >> 4 } else { 0 }
}

/// `svt_aom_cdef_find_dir_c` (cdef.c:88): direction search over an 8x8 block
/// of 16-bit pixels. Returns `(best_dir, var)`. 0 = 45-degree up-right,
/// 2 = horizontal, 4 = down-right, 6 = vertical (spec 7.15.3 ordering).
///
/// Reads exactly the 8x8 interior — border/sentinel pixels are never seen.
pub fn cdef_find_dir(img: &[u16], stride: usize, coeff_shift: i32) -> (u8, i32) {
    let mut cost = [0i32; 8];
    let mut partial = [[0i32; 15]; 8];
    let mut best_cost = 0i32;
    let mut best_dir = 0usize;
    // 840/n for n in 1..=8 (offset by 1; entry 0 unused).
    const DIV_TABLE: [i32; 9] = [0, 840, 420, 280, 210, 168, 140, 120, 105];
    for i in 0..8usize {
        for j in 0..8usize {
            let x = ((img[i * stride + j] as i32) >> coeff_shift) - 128;
            partial[0][i + j] += x;
            partial[1][i + j / 2] += x;
            partial[2][i] += x;
            partial[3][3 + i - j / 2] += x;
            partial[4][7 + i - j] += x;
            partial[5][3 - i / 2 + j] += x;
            partial[6][j] += x;
            partial[7][i / 2 + j] += x;
        }
    }
    for i in 0..8 {
        cost[2] += partial[2][i] * partial[2][i];
        cost[6] += partial[6][i] * partial[6][i];
    }
    cost[2] *= DIV_TABLE[8];
    cost[6] *= DIV_TABLE[8];
    for i in 0..7 {
        cost[0] +=
            (partial[0][i] * partial[0][i] + partial[0][14 - i] * partial[0][14 - i]) * DIV_TABLE[i + 1];
        cost[4] +=
            (partial[4][i] * partial[4][i] + partial[4][14 - i] * partial[4][14 - i]) * DIV_TABLE[i + 1];
    }
    cost[0] += partial[0][7] * partial[0][7] * DIV_TABLE[8];
    cost[4] += partial[4][7] * partial[4][7] * DIV_TABLE[8];
    let mut i = 1;
    while i < 8 {
        for j in 0..5 {
            cost[i] += partial[i][3 + j] * partial[i][3 + j];
        }
        cost[i] *= DIV_TABLE[8];
        for j in 0..3 {
            cost[i] +=
                (partial[i][j] * partial[i][j] + partial[i][10 - j] * partial[i][10 - j])
                    * DIV_TABLE[2 * j + 2];
        }
        i += 2;
    }
    for (i, &c) in cost.iter().enumerate() {
        if c > best_cost {
            best_cost = c;
            best_dir = i;
        }
    }
    let mut var = best_cost - cost[(best_dir + 4) & 7];
    var >>= 10;
    (best_dir as u8, var)
}

/// `svt_aom_cdef_find_dir_8bit_c` (cdef.c:303): widen an 8x8 of 8-bit pixels
/// to 16 bit and delegate to [`cdef_find_dir`].
pub fn cdef_find_dir_8bit(img: &[u8], stride: usize, coeff_shift: i32) -> (u8, i32) {
    let mut img16 = [0u16; 64];
    for i in 0..8 {
        for j in 0..8 {
            img16[i * 8 + j] = img[i * stride + j] as u16;
        }
    }
    cdef_find_dir(&img16, 8, coeff_shift)
}

/// C `clamp` (definitions.h).
#[inline]
fn clamp_i32(value: i32, low: i32, high: i32) -> i32 {
    if value < low {
        low
    } else if value > high {
        high
    } else {
        value
    }
}

/// `svt_cdef_filter_block_c` (cdef.c:193), dst8 arm: primary + secondary
/// directional filtering of one block inside a `CDEF_BSTRIDE`-strided 16-bit
/// buffer where unavailable pixels hold [`CDEF_VERY_LARGE`].
///
/// `inb`/`ioff`: padded input buffer and the index of the block's (0,0)
/// pixel (tap offsets are signed; `ioff` must leave `CDEF_VBORDER` rows and
/// at least 2 columns of headroom, which any `CDEF_INBUF_SIZE` layout does).
/// `dst`/`doff`/`dstride`: 8-bit output. `dir`: 0..=7. `bsize`: one of
/// [`BLOCK_8X8`]/[`BLOCK_4X8`]/[`BLOCK_8X4`]/[`BLOCK_4X4`].
/// `subsampling_factor` (1 or 2) skips every other row (C search decimation;
/// the decoder path always passes 1).
#[allow(clippy::too_many_arguments)]
pub fn cdef_filter_block(
    dst: &mut [u8],
    doff: usize,
    dstride: usize,
    inb: &[u16],
    ioff: usize,
    pri_strength: i32,
    sec_strength: i32,
    dir: i32,
    pri_damping: i32,
    sec_damping: i32,
    bsize: i32,
    coeff_shift: i32,
    subsampling_factor: usize,
) {
    let s = CDEF_BSTRIDE as i32;
    let pri_taps = CDEF_PRI_TAPS[((pri_strength >> coeff_shift) & 1) as usize];
    let sec_taps = CDEF_SEC_TAPS[((pri_strength >> coeff_shift) & 1) as usize];
    let rows = if bsize == BLOCK_8X8 || bsize == BLOCK_4X8 { 8 } else { 4 };
    let cols = if bsize == BLOCK_8X8 || bsize == BLOCK_8X4 { 8 } else { 4 };

    let at = |i: i32, j: i32, off: i32| -> u16 {
        inb[(ioff as i32 + i * s + j + off) as usize]
    };

    let mut i = 0i32;
    while i < rows {
        for j in 0..cols {
            let mut sum = 0i16;
            let x = at(i, j, 0) as i16;
            let mut max = x as i32;
            let mut min = x as i32;
            for k in 0..2usize {
                let p0 = at(i, j, cdef_direction(dir, k)) as i16;
                let p1 = at(i, j, -cdef_direction(dir, k)) as i16;
                sum = sum.wrapping_add(
                    (pri_taps[k] * constrain(p0 as i32 - x as i32, pri_strength, pri_damping)) as i16,
                );
                sum = sum.wrapping_add(
                    (pri_taps[k] * constrain(p1 as i32 - x as i32, pri_strength, pri_damping)) as i16,
                );
                if p0 as u16 != CDEF_VERY_LARGE {
                    max = (p0 as i32).max(max);
                }
                if p1 as u16 != CDEF_VERY_LARGE {
                    max = (p1 as i32).max(max);
                }
                min = (p0 as i32).min(min);
                min = (p1 as i32).min(min);
                let s0 = at(i, j, cdef_direction(dir + 2, k)) as i16;
                let s1 = at(i, j, -cdef_direction(dir + 2, k)) as i16;
                let s2 = at(i, j, cdef_direction(dir - 2, k)) as i16;
                let s3 = at(i, j, -cdef_direction(dir - 2, k)) as i16;
                if s0 as u16 != CDEF_VERY_LARGE {
                    max = (s0 as i32).max(max);
                }
                if s1 as u16 != CDEF_VERY_LARGE {
                    max = (s1 as i32).max(max);
                }
                if s2 as u16 != CDEF_VERY_LARGE {
                    max = (s2 as i32).max(max);
                }
                if s3 as u16 != CDEF_VERY_LARGE {
                    max = (s3 as i32).max(max);
                }
                min = (s0 as i32).min(min);
                min = (s1 as i32).min(min);
                min = (s2 as i32).min(min);
                min = (s3 as i32).min(min);
                sum = sum.wrapping_add(
                    (sec_taps[k] * constrain(s0 as i32 - x as i32, sec_strength, sec_damping)) as i16,
                );
                sum = sum.wrapping_add(
                    (sec_taps[k] * constrain(s1 as i32 - x as i32, sec_strength, sec_damping)) as i16,
                );
                sum = sum.wrapping_add(
                    (sec_taps[k] * constrain(s2 as i32 - x as i32, sec_strength, sec_damping)) as i16,
                );
                sum = sum.wrapping_add(
                    (sec_taps[k] * constrain(s3 as i32 - x as i32, sec_strength, sec_damping)) as i16,
                );
            }
            let y = clamp_i32(
                x as i32 + ((8 + sum as i32 - i32::from(sum < 0)) >> 4),
                min,
                max,
            );
            dst[doff + i as usize * dstride + j as usize] = y as u8;
        }
        i += subsampling_factor as i32;
    }
}

/// `svt_cdef_filter_block_8bit_c` (cdef.c:257): native 8-bit interior filter
/// — identical math to [`cdef_filter_block`] but reads an 8-bit padded
/// buffer with NO sentinel handling (every tap participates in min/max), so
/// it is only valid for blocks whose full tap halo is real pixels.
#[allow(clippy::too_many_arguments)]
pub fn cdef_filter_block_8bit(
    dst: &mut [u8],
    doff: usize,
    dstride: usize,
    inb: &[u8],
    ioff: usize,
    pri_strength: i32,
    sec_strength: i32,
    dir: i32,
    damping: i32,
    bsize: i32,
    coeff_shift: i32,
    subsampling_factor: usize,
) {
    let s = CDEF_BSTRIDE as i32;
    let pri_taps = CDEF_PRI_TAPS[((pri_strength >> coeff_shift) & 1) as usize];
    let sec_taps = CDEF_SEC_TAPS[((pri_strength >> coeff_shift) & 1) as usize];
    let rows = if bsize == BLOCK_8X8 || bsize == BLOCK_4X8 { 8 } else { 4 };
    let cols = if bsize == BLOCK_8X8 || bsize == BLOCK_8X4 { 8 } else { 4 };
    let sub = if bsize == BLOCK_4X4 { 1 } else { subsampling_factor };

    let at = |i: i32, j: i32, off: i32| -> i16 {
        inb[(ioff as i32 + i * s + j + off) as usize] as i16
    };

    let mut i = 0i32;
    while i < rows {
        for j in 0..cols {
            let x = at(i, j, 0);
            let mut sum = 0i16;
            let mut max = x as i32;
            let mut min = x as i32;
            for k in 0..2usize {
                let p0 = at(i, j, cdef_direction(dir, k));
                let p1 = at(i, j, -cdef_direction(dir, k));
                sum = sum.wrapping_add(
                    (pri_taps[k] * constrain(p0 as i32 - x as i32, pri_strength, damping)) as i16,
                );
                sum = sum.wrapping_add(
                    (pri_taps[k] * constrain(p1 as i32 - x as i32, pri_strength, damping)) as i16,
                );
                max = (p0 as i32).max(max);
                max = (p1 as i32).max(max);
                min = (p0 as i32).min(min);
                min = (p1 as i32).min(min);
                let s0 = at(i, j, cdef_direction(dir + 2, k));
                let s1 = at(i, j, -cdef_direction(dir + 2, k));
                let s2 = at(i, j, cdef_direction(dir - 2, k));
                let s3 = at(i, j, -cdef_direction(dir - 2, k));
                sum = sum.wrapping_add(
                    (sec_taps[k] * constrain(s0 as i32 - x as i32, sec_strength, damping)) as i16,
                );
                sum = sum.wrapping_add(
                    (sec_taps[k] * constrain(s1 as i32 - x as i32, sec_strength, damping)) as i16,
                );
                sum = sum.wrapping_add(
                    (sec_taps[k] * constrain(s2 as i32 - x as i32, sec_strength, damping)) as i16,
                );
                sum = sum.wrapping_add(
                    (sec_taps[k] * constrain(s3 as i32 - x as i32, sec_strength, damping)) as i16,
                );
                max = (s0 as i32).max(max);
                max = (s1 as i32).max(max);
                max = (s2 as i32).max(max);
                max = (s3 as i32).max(max);
                min = (s0 as i32).min(min);
                min = (s1 as i32).min(min);
                min = (s2 as i32).min(min);
                min = (s3 as i32).min(min);
            }
            let y = clamp_i32(
                x as i32 + ((8 + sum as i32 - i32::from(sum < 0)) >> 4),
                min,
                max,
            );
            dst[doff + i as usize * dstride + j as usize] = y as u8;
        }
        i += sub as i32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spec 7.15.3 Cdef_Directions cross-check: the padded table's live rows
    /// (index 2..10) decoded back to (dy, dx) must equal the spec table.
    #[test]
    fn direction_table_matches_spec() {
        const SPEC: [[[i32; 2]; 2]; 8] = [
            [[-1, 1], [-2, 2]],
            [[0, 1], [-1, 2]],
            [[0, 1], [0, 2]],
            [[0, 1], [1, 2]],
            [[1, 1], [2, 2]],
            [[1, 0], [2, 1]],
            [[1, 0], [2, 0]],
            [[1, 0], [2, -1]],
        ];
        let s = CDEF_BSTRIDE as i32;
        for dir in 0..8 {
            for k in 0..2 {
                let off = cdef_direction(dir, k);
                // decode: dy = round-to-nearest row (offsets have |dx| <= 2)
                let dy = if off >= 0 { (off + s / 2) / s } else { -((-off + s / 2) / s) };
                let dx = off - dy * s;
                assert_eq!([dy, dx], SPEC[dir as usize][k], "dir {dir} k {k}");
            }
        }
        // padded ends replicate dir 6,7 and 0,1
        assert_eq!(cdef_direction(-2, 0), cdef_direction(6, 0));
        assert_eq!(cdef_direction(-1, 1), cdef_direction(7, 1));
        assert_eq!(cdef_direction(8, 0), cdef_direction(0, 0));
        assert_eq!(cdef_direction(9, 1), cdef_direction(1, 1));
    }

    /// A flat block has no direction energy: var must be 0 and filtering at
    /// any strength must be the identity.
    #[test]
    fn flat_block_identity() {
        let mut inb = alloc::vec![CDEF_VERY_LARGE; CDEF_INBUF_SIZE];
        let ioff = CDEF_VBORDER * CDEF_BSTRIDE + CDEF_HBORDER;
        for r in 0..8 {
            for c in 0..8 {
                inb[ioff + r * CDEF_BSTRIDE + c] = 77;
            }
        }
        let (_dir, var) = cdef_find_dir(&inb[ioff..], CDEF_BSTRIDE, 0);
        assert_eq!(var, 0);
        let mut dst = [0u8; 64];
        cdef_filter_block(&mut dst, 0, 8, &inb, ioff, 15, 4, 3, 6, 6, BLOCK_8X8, 0, 1);
        assert!(dst.iter().all(|&v| v == 77));
    }

    /// constrain() reproduces the C damping shape at hand-checked points.
    #[test]
    fn constrain_c_values() {
        // threshold 0 -> 0 regardless
        assert_eq!(constrain(1000, 0, 6), 0);
        // shift = max(0, 4 - msb(4)=2) = 2: c(5,4,4) = min(5, 4 - (5>>2)) = 3
        assert_eq!(constrain(5, 4, 4), 3);
        assert_eq!(constrain(-5, 4, 4), -3);
        // sentinel-sized diff is fully damped to 0
        assert_eq!(constrain(0x7f7f - 128, 15, 6), 0);
        assert_eq!(constrain(0x4000 - 128, 15, 6), 0);
        // large threshold, small diff: passes through
        assert_eq!(constrain(2, 15, 3), 2);
    }

    /// adjust_strength C anchor points.
    #[test]
    fn adjust_strength_c_values() {
        assert_eq!(adjust_strength(12, 0), 0);
        // var=63: var>>6 = 0 -> i=0 -> (12*4+8)>>4 = 3
        assert_eq!(adjust_strength(12, 63), 3);
        // var=1<<18: i = min(msb(1<<12)=12, 12) -> (12*16+8)>>4 = 12
        assert_eq!(adjust_strength(12, 1 << 18), 12);
    }
}
