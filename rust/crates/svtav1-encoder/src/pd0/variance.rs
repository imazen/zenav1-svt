use super::*;

/// The 85-entry per-64x64-block variance map: index 0 = 64x64,
/// 1..=4 = 32x32 (2x2 raster), 5..=20 = 16x16 (4x4), 21..=84 = 8x8 (8x8).
#[derive(Clone, Copy)]
pub struct SbVariance(pub [u16; 85]);

/// C `compute_b64_variance` at `BLOCK_MEAN_PREC_SUB` (the default,
/// enc_handle.c:4618): 8x8 means/mean-squares from the EVEN rows only
/// (rows 0,2,4,6 of each 8x8), `mean << 3` / `mean_sq << 11` fixed point,
/// averaged up the quad tree with `>> 2`, variance = `(msq - m*m) >> 16`
/// cast to u16 per level.
///
/// The source region must be fully inside the picture (the C input is the
/// edge-padded `input_padded_pic`; every current caller pads frames to
/// 64-aligned dimensions so the region is always complete).
pub(crate) fn compute_b64_variance(
    src: &[u8],
    stride: usize,
    org_x: usize,
    org_y: usize,
) -> SbVariance {
    let mut mean8 = [0u64; 64];
    let mut msq8 = [0u64; 64];
    incant!(
        b64_stats_impl(src, stride, org_x, org_y, &mut mean8, &mut msq8),
        [v3, neon, scalar]
    );
    let mut mean16 = [0u64; 16];
    let mut msq16 = [0u64; 16];
    for by in 0..4 {
        for bx in 0..4 {
            let f = by * 16 + bx * 2;
            mean16[by * 4 + bx] = (mean8[f] + mean8[f + 1] + mean8[f + 8] + mean8[f + 9]) >> 2;
            msq16[by * 4 + bx] = (msq8[f] + msq8[f + 1] + msq8[f + 8] + msq8[f + 9]) >> 2;
        }
    }
    let mut mean32 = [0u64; 4];
    let mut msq32 = [0u64; 4];
    for by in 0..2 {
        for bx in 0..2 {
            let f = by * 8 + bx * 2;
            mean32[by * 2 + bx] = (mean16[f] + mean16[f + 1] + mean16[f + 4] + mean16[f + 5]) >> 2;
            msq32[by * 2 + bx] = (msq16[f] + msq16[f + 1] + msq16[f + 4] + msq16[f + 5]) >> 2;
        }
    }
    let mean64 = (mean32[0] + mean32[1] + mean32[2] + mean32[3]) >> 2;
    let msq64 = (msq32[0] + msq32[1] + msq32[2] + msq32[3]) >> 2;

    const VARIANCE_PRECISION: u32 = 16;
    let mut v = [0u16; 85];
    v[0] = (msq64.wrapping_sub(mean64 * mean64) >> VARIANCE_PRECISION) as u16;
    for i in 0..4 {
        v[1 + i] = (msq32[i].wrapping_sub(mean32[i] * mean32[i]) >> VARIANCE_PRECISION) as u16;
    }
    for i in 0..16 {
        v[5 + i] = (msq16[i].wrapping_sub(mean16[i] * mean16[i]) >> VARIANCE_PRECISION) as u16;
    }
    for i in 0..64 {
        v[21 + i] = (msq8[i].wrapping_sub(mean8[i] * mean8[i]) >> VARIANCE_PRECISION) as u16;
    }
    SbVariance(v)
}

/// Per-8x8-block sampled stats for [`compute_b64_variance`]: the even-row
/// (`r in {0,2,4,6}`) 32-sample `sum`/`sum_sq` of each 8x8 quadrant, stored as
/// `sum << 3` / `sq << 11` fixed point into `mean8`/`msq8`.
pub(super) fn b64_stats_impl_scalar(
    _token: ScalarToken,
    src: &[u8],
    stride: usize,
    org_x: usize,
    org_y: usize,
    mean8: &mut [u64; 64],
    msq8: &mut [u64; 64],
) {
    for by in 0..8 {
        for bx in 0..8 {
            // u32 accumulation is exact: 32 u8 samples -> sum <= 8_160,
            // sq <= 32 * 255^2 = 2_080_800. Keeping the accumulators 32-bit
            // lets LLVM auto-vectorize the pixel loop; widening to u64 blocks
            // it (~6.3K -> ~1.5K instructions per SB).
            let mut sum = 0u32;
            let mut sq = 0u32;
            for r in [0usize, 2, 4, 6] {
                let row = (org_y + by * 8 + r) * stride + org_x + bx * 8;
                for c in 0..8 {
                    let v = src[row + c] as u32;
                    sum += v;
                    sq += v * v;
                }
            }
            mean8[by * 8 + bx] = (sum as u64) << 3;
            msq8[by * 8 + bx] = (sq as u64) << 11;
        }
    }
}

/// AVX2 twin of [`b64_stats_impl_scalar`]: one `u8x16` load covers a horizontal
/// PAIR of 8x8 blocks (their sampled columns are contiguous), so each loop
/// iteration squares and sums 16 pixels with two `madd`s. Four i32x4
/// accumulators keep the pair's lanes separate — per lane the max is
/// `4 rows * 2 squares * 65025 = 520_200`, inside i32.
#[cfg(target_arch = "x86_64")]
#[arcane]
pub(super) fn b64_stats_impl_v3(
    token: Desktop64,
    src: &[u8],
    stride: usize,
    org_x: usize,
    org_y: usize,
    mean8: &mut [u64; 64],
    msq8: &mut [u64; 64],
) {
    use magetypes::simd::generic::{i16x8, i32x4, u8x16};
    let ones = i16x8::splat(token, 1);
    for by in 0..8 {
        for bxp in 0..4 {
            let bx = bxp * 2;
            let mut sum_lo = i32x4::splat(token, 0);
            let mut sum_hi = i32x4::splat(token, 0);
            let mut sq_lo = i32x4::splat(token, 0);
            let mut sq_hi = i32x4::splat(token, 0);
            for r in [0usize, 2, 4, 6] {
                let row = (org_y + by * 8 + r) * stride + org_x + bx * 8;
                let v = u8x16::load(token, src[row..row + 16].try_into().unwrap());
                let lo = v.widen_low().bitcast_i16x8();
                let hi = v.widen_high().bitcast_i16x8();
                sum_lo += lo.madd_adjacent(ones);
                sq_lo += lo.madd_adjacent(lo);
                sum_hi += hi.madd_adjacent(ones);
                sq_hi += hi.madd_adjacent(hi);
            }
            mean8[by * 8 + bx] = u64::from(sum_lo.reduce_add() as u32) << 3;
            msq8[by * 8 + bx] = u64::from(sq_lo.reduce_add() as u32) << 11;
            mean8[by * 8 + bx + 1] = u64::from(sum_hi.reduce_add() as u32) << 3;
            msq8[by * 8 + bx + 1] = u64::from(sq_hi.reduce_add() as u32) << 11;
        }
    }
}

/// NEON twin of [`b64_stats_impl_v3`]: same 16-pixel row-pair shape.
/// `vpadalq_u16` is the pairwise `madd_adjacent(ones)`; for the squares,
/// `vmull_u16` gives per-lane products (not pairwise) so each `u32` lane
/// accumulates `4 rows * 2 halves * 65025 = 520_200` — inside `u32`.
#[cfg(target_arch = "aarch64")]
#[arcane]
pub(super) fn b64_stats_impl_neon(
    _token: NeonToken,
    src: &[u8],
    stride: usize,
    org_x: usize,
    org_y: usize,
    mean8: &mut [u64; 64],
    msq8: &mut [u64; 64],
) {
    for by in 0..8 {
        for bxp in 0..4 {
            let bx = bxp * 2;
            let mut sum_lo = vdupq_n_u32(0);
            let mut sum_hi = vdupq_n_u32(0);
            let mut sq_lo = vdupq_n_u32(0);
            let mut sq_hi = vdupq_n_u32(0);
            for r in [0usize, 2, 4, 6] {
                let row = (org_y + by * 8 + r) * stride + org_x + bx * 8;
                let v = vld1q_u8(src[row..row + 16].try_into().unwrap());
                let lo = vmovl_u8(vget_low_u8(v));
                let hi = vmovl_u8(vget_high_u8(v));
                sum_lo = vpadalq_u16(sum_lo, lo);
                sum_hi = vpadalq_u16(sum_hi, hi);
                let llo = vget_low_u16(lo);
                let lhi = vget_high_u16(lo);
                sq_lo = vaddq_u32(sq_lo, vaddq_u32(vmull_u16(llo, llo), vmull_u16(lhi, lhi)));
                let hlo = vget_low_u16(hi);
                let hhi = vget_high_u16(hi);
                sq_hi = vaddq_u32(sq_hi, vaddq_u32(vmull_u16(hlo, hlo), vmull_u16(hhi, hhi)));
            }
            mean8[by * 8 + bx] = u64::from(vaddvq_u32(sum_lo)) << 3;
            msq8[by * 8 + bx] = u64::from(vaddvq_u32(sq_lo)) << 11;
            mean8[by * 8 + bx + 1] = u64::from(vaddvq_u32(sum_hi)) << 3;
            msq8[by * 8 + bx + 1] = u64::from(vaddvq_u32(sq_hi)) << 11;
        }
    }
}

/// C `svt_aom_get_blk_var_map` (product_coding_loop.c:8368): variance-map
/// index of the square block at (org_x, org_y) relative to the SB, plus
/// its four sub-block indices (valid for block_size >= 16).
pub(super) fn blk_var_map(block_size: usize, org_x: usize, org_y: usize) -> (usize, [usize; 4]) {
    const LOG2_LUT: [usize; 4] = [6, 5, 4, 3];
    const GRID_LUT: [usize; 4] = [1, 2, 4, 8];
    const BASE_LUT: [usize; 4] = [0, 1, 5, 21];
    let lvl = 6 - block_size.ilog2() as usize;
    debug_assert!(lvl <= 3);
    let blk_idx =
        BASE_LUT[lvl] + (org_y >> LOG2_LUT[lvl]) * GRID_LUT[lvl] + (org_x >> LOG2_LUT[lvl]);
    let sub_lvl = lvl + 1;
    let sub = if sub_lvl < 4 {
        let (ss, sb, sg) = (LOG2_LUT[sub_lvl], BASE_LUT[sub_lvl], GRID_LUT[sub_lvl]);
        let (sx, sy) = (org_x >> ss, org_y >> ss);
        [
            sb + sy * sg + sx,
            sb + sy * sg + sx + 1,
            sb + (sy + 1) * sg + sx,
            sb + (sy + 1) * sg + sx + 1,
        ]
    } else {
        [0; 4]
    };
    (blk_idx, sub)
}

// ---------------------------------------------------------------------------
// QP-based threshold scaling + RDCOST + lambda
// ---------------------------------------------------------------------------
