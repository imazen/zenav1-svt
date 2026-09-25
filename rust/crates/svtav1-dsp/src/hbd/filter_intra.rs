use super::*;

pub(super) const FILTER_INTRA_SCALE_BITS_HBD: i32 = 4;

/// Duplicated from `intra_pred.rs`'s private `FILTER_INTRA_TAPS` (not
/// `pub`; same C provenance, `eb_av1_filter_intra_taps`, bd-independent
/// integer coefficients).
#[rustfmt::skip]
pub(super) static FILTER_INTRA_TAPS_HBD: [[[i8; 8]; 8]; 5] = [
    [
        [-6, 10, 0, 0, 0, 12, 0, 0],
        [-5,  2, 10, 0, 0, 9, 0, 0],
        [-3,  1, 1, 10, 0, 7, 0, 0],
        [-3,  1, 1, 2, 10, 5, 0, 0],
        [-4,  6, 0, 0, 0, 2, 12, 0],
        [-3,  2, 6, 0, 0, 2, 9, 0],
        [-3,  2, 2, 6, 0, 2, 7, 0],
        [-3,  1, 2, 2, 6, 3, 5, 0],
    ],
    [
        [-10, 16, 0, 0, 0, 10, 0, 0],
        [ -6,  0, 16, 0, 0, 6, 0, 0],
        [ -4,  0, 0, 16, 0, 4, 0, 0],
        [ -2,  0, 0, 0, 16, 2, 0, 0],
        [-10, 16, 0, 0, 0, 0, 10, 0],
        [ -6,  0, 16, 0, 0, 0, 6, 0],
        [ -4,  0, 0, 16, 0, 0, 4, 0],
        [ -2,  0, 0, 0, 16, 0, 2, 0],
    ],
    [
        [-8, 8, 0, 0, 0, 16, 0, 0],
        [-8, 0, 8, 0, 0, 16, 0, 0],
        [-8, 0, 0, 8, 0, 16, 0, 0],
        [-8, 0, 0, 0, 8, 16, 0, 0],
        [-4, 4, 0, 0, 0, 0, 16, 0],
        [-4, 0, 4, 0, 0, 0, 16, 0],
        [-4, 0, 0, 4, 0, 0, 16, 0],
        [-4, 0, 0, 0, 4, 0, 16, 0],
    ],
    [
        [-2, 8, 0, 0, 0, 10, 0, 0],
        [-1, 3, 8, 0, 0, 6, 0, 0],
        [-1, 2, 3, 8, 0, 4, 0, 0],
        [ 0, 1, 2, 3, 8, 2, 0, 0],
        [-1, 4, 0, 0, 0, 3, 10, 0],
        [-1, 3, 4, 0, 0, 4, 6, 0],
        [-1, 2, 3, 4, 0, 4, 4, 0],
        [-1, 2, 2, 3, 4, 3, 3, 0],
    ],
    [
        [-12, 14, 0, 0, 0, 14, 0, 0],
        [-10,  0, 14, 0, 0, 12, 0, 0],
        [ -9,  0, 0, 14, 0, 11, 0, 0],
        [ -8,  0, 0, 0, 14, 10, 0, 0],
        [-10, 12, 0, 0, 0, 0, 14, 0],
        [ -9,  1, 12, 0, 0, 0, 12, 0],
        [ -8,  0, 0, 12, 0, 1, 11, 0],
        [ -7,  0, 0, 1, 12, 1, 9, 0],
    ],
];

/// C `ROUND_POWER_OF_TWO_SIGNED` applied inline in
/// `svt_aom_highbd_filter_intra_predictor`. Duplicated from
/// `intra_pred.rs`'s private `round_power_of_two_signed` (bd-independent).
#[inline]
pub(super) fn round_power_of_two_signed_hbd(value: i32, n: i32) -> i32 {
    if value < 0 {
        -((-value + (1 << (n - 1))) >> n)
    } else {
        (value + (1 << (n - 1))) >> n
    }
}

/// C `svt_aom_highbd_filter_intra_predictor` (intra_prediction.c:2549-2595).
///
/// `above` layout matches the lbd sibling: `above[0]` = top-left,
/// `above[1..]` = pixels above the block (length `width + 1`); `left`
/// length `height`.
pub fn predict_filter_intra_hbd(
    dst: &mut [u16],
    dst_stride: usize,
    above: &[u16],
    left: &[u16],
    width: usize,
    height: usize,
    mode: u8,
    bd: u8,
) {
    assert!(width <= 32 && height <= 32);
    assert!((mode as usize) < 5);
    assert!(dst.len() >= (height - 1) * dst_stride + width);
    assert!(left.len() >= height);
    assert!(above.len() >= width + 1);
    incant!(
        predict_filter_intra_hbd_impl(dst, dst_stride, above, left, width, height, mode, bd),
        [neon, scalar]
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn predict_filter_intra_hbd_impl_scalar(
    _token: ScalarToken,
    dst: &mut [u16],
    dst_stride: usize,
    above: &[u16],
    left: &[u16],
    width: usize,
    height: usize,
    mode: u8,
    bd: u8,
) {
    predict_filter_intra_hbd_core(dst, dst_stride, above, left, width, height, mode, bd);
}

#[allow(clippy::too_many_arguments)]
pub(super) fn predict_filter_intra_hbd_core(
    dst: &mut [u16],
    dst_stride: usize,
    above: &[u16],
    left: &[u16],
    width: usize,
    height: usize,
    mode: u8,
    bd: u8,
) {
    // Same in-place rewrite as `predict_filter_intra`: every tap reads
    // `above`/`left` or a dst cell an earlier sub-block already wrote, so the
    // `buffer[33][33]` staging — and its per-call zero — is unnecessary.
    // `up[j]` == `buf[r-1][j+1]` (above[1..] on the first row pair, dst row
    // `r - 2` after); the `c == 1` sub-block, whose p0/p5/p6 are border
    // cells, is peeled so the interior loop is branch-free.
    let taps = &FILTER_INTRA_TAPS_HBD[mode as usize];
    for r in (1..height + 1).step_by(2) {
        // Split at row `r - 1`: dst rows `r - 1`,`r` are written while row
        // `r - 2` is the up-tap source; `cur[j]` == `dst[(r-1)*stride + j]`.
        let (above_rows, cur) = dst.split_at_mut((r - 1) * dst_stride);
        let up: &[u16] = if r == 1 {
            &above[1..]
        } else {
            &above_rows[(r - 2) * dst_stride..]
        };
        {
            let p0 = (if r == 1 { above[0] } else { left[r - 2] }) as i32;
            let p1 = up[0] as i32;
            let p2 = up[1] as i32;
            let p3 = up[2] as i32;
            let p4 = up[3] as i32;
            let p5 = left[r - 1] as i32;
            let p6 = left[r] as i32;
            for k in 0..8 {
                let val = taps[k][0] as i32 * p0
                    + taps[k][1] as i32 * p1
                    + taps[k][2] as i32 * p2
                    + taps[k][3] as i32 * p3
                    + taps[k][4] as i32 * p4
                    + taps[k][5] as i32 * p5
                    + taps[k][6] as i32 * p6;
                cur[(k >> 2) * dst_stride + (k & 0x03)] = clip_pixel_highbd(
                    round_power_of_two_signed_hbd(val, FILTER_INTRA_SCALE_BITS_HBD),
                    bd,
                );
            }
        }
        for c in (5..width + 1).step_by(4) {
            let p0 = up[c - 2] as i32;
            let p1 = up[c - 1] as i32;
            let p2 = up[c] as i32;
            let p3 = up[c + 1] as i32;
            let p4 = up[c + 2] as i32;
            let p5 = cur[c - 2] as i32;
            let p6 = cur[dst_stride + c - 2] as i32;

            for k in 0..8 {
                let r_offset = k >> 2;
                let c_offset = k & 0x03;
                let val = taps[k][0] as i32 * p0
                    + taps[k][1] as i32 * p1
                    + taps[k][2] as i32 * p2
                    + taps[k][3] as i32 * p3
                    + taps[k][4] as i32 * p4
                    + taps[k][5] as i32 * p5
                    + taps[k][6] as i32 * p6;
                cur[r_offset * dst_stride + (c + c_offset - 1)] = clip_pixel_highbd(
                    round_power_of_two_signed_hbd(val, FILTER_INTRA_SCALE_BITS_HBD),
                    bd,
                );
            }
        }
    }
}

/// One 4x2 sub-block of [`predict_filter_intra_hbd`]: the eight outputs are
/// `taps[k][t] * p[t]` summed over t — an i32x4 matvec across k. Output
/// k=0..3 lands contiguously at `cur[c-1..c+3]`, k=4..7 at the next row's
/// same span, so the rounded+clipped i32x4s narrow straight into two u16x4
/// stores. The rounding is the sign-symmetric `ROUND_POWER_OF_TWO_SIGNED`
/// (`s = v>>31; (((|v|+8)>>4)^s)-s` — exact, `|v|` cannot be i32::MIN:
/// taps are i8 and p are u16, so `|v| <= 7*127*65535`). The serial
/// p5/p6 reads of already-written `cur` cells stay scalar.
#[cfg(target_arch = "aarch64")]
#[rite]
pub(super) fn filter_intra_4x2_neon(
    _token: NeonToken,
    p: &[i32; 7],
    tt: &[[int32x4_t; 2]; 7],
    maxv: int32x4_t,
) -> (uint16x4_t, uint16x4_t) {
    let mut lo = vdupq_n_s32(0);
    let mut hi = vdupq_n_s32(0);
    for t in 0..7 {
        let pv = vdupq_n_s32(p[t]);
        lo = vmlaq_s32(lo, tt[t][0], pv);
        hi = vmlaq_s32(hi, tt[t][1], pv);
    }
    let eight = vdupq_n_s32(8);
    let zero = vdupq_n_s32(0);
    let s_lo = vshrq_n_s32::<31>(lo);
    let s_hi = vshrq_n_s32::<31>(hi);
    let m_lo = vshrq_n_s32::<4>(vaddq_s32(vabsq_s32(lo), eight));
    let m_hi = vshrq_n_s32::<4>(vaddq_s32(vabsq_s32(hi), eight));
    let lo = vsubq_s32(veorq_s32(m_lo, s_lo), s_lo);
    let hi = vsubq_s32(veorq_s32(m_hi, s_hi), s_hi);
    let lo = vminq_s32(vmaxq_s32(lo, zero), maxv);
    let hi = vminq_s32(vmaxq_s32(hi, zero), maxv);
    (
        vmovn_u32(vreinterpretq_u32_s32(lo)),
        vmovn_u32(vreinterpretq_u32_s32(hi)),
    )
}

/// aarch64 arm of [`predict_filter_intra_hbd`]: taps are transposed once
/// per call into `tt[t][k]` i32x4 pairs, then every sub-block is one
/// [`filter_intra_4x2_neon`] matvec + two u16x4 stores.
#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
pub(super) fn predict_filter_intra_hbd_impl_neon(
    token: NeonToken,
    dst: &mut [u16],
    dst_stride: usize,
    above: &[u16],
    left: &[u16],
    width: usize,
    height: usize,
    mode: u8,
    bd: u8,
) {
    let taps = &FILTER_INTRA_TAPS_HBD[mode as usize];
    let mut tt_arr = [[0i32; 8]; 7];
    for t in 0..7 {
        for k in 0..8 {
            tt_arr[t][k] = taps[k][t] as i32;
        }
    }
    let mut tt = [[vdupq_n_s32(0); 2]; 7];
    for t in 0..7 {
        tt[t][0] = vld1q_s32((&tt_arr[t][..4]).try_into().unwrap());
        tt[t][1] = vld1q_s32((&tt_arr[t][4..]).try_into().unwrap());
    }
    let maxv = vdupq_n_s32(match bd {
        10 => 1023,
        12 => 4095,
        _ => 255,
    });
    for r in (1..height + 1).step_by(2) {
        let (above_rows, cur) = dst.split_at_mut((r - 1) * dst_stride);
        let up: &[u16] = if r == 1 {
            &above[1..]
        } else {
            &above_rows[(r - 2) * dst_stride..]
        };
        {
            let p = [
                (if r == 1 { above[0] } else { left[r - 2] }) as i32,
                up[0] as i32,
                up[1] as i32,
                up[2] as i32,
                up[3] as i32,
                left[r - 1] as i32,
                left[r] as i32,
            ];
            let (lo, hi) = filter_intra_4x2_neon(token, &p, &tt, maxv);
            vst1_u16((&mut cur[0..4]).try_into().unwrap(), lo);
            vst1_u16(
                (&mut cur[dst_stride..dst_stride + 4]).try_into().unwrap(),
                hi,
            );
        }
        for c in (5..width + 1).step_by(4) {
            let p = [
                up[c - 2] as i32,
                up[c - 1] as i32,
                up[c] as i32,
                up[c + 1] as i32,
                up[c + 2] as i32,
                cur[c - 2] as i32,
                cur[dst_stride + c - 2] as i32,
            ];
            let (lo, hi) = filter_intra_4x2_neon(token, &p, &tt, maxv);
            vst1_u16((&mut cur[c - 1..c + 3]).try_into().unwrap(), lo);
            vst1_u16(
                (&mut cur[dst_stride + c - 1..dst_stride + c + 3])
                    .try_into()
                    .unwrap(),
                hi,
            );
        }
    }
}

// =============================================================================
// 5. Chroma-from-Luma (CfL), highbd.
// C: intra_prediction.c:437-445 (`svt_cfl_luma_subsampling_420_hbd_c`),
// C_DEFAULT/cfl_c.c:46-59 (`svt_cfl_predict_hbd_c`).
//
// `svt_subtract_average_c` (C_DEFAULT/cfl_c.c:451-472) is ALREADY
// bit-depth-generic (Q3 AC values are `int16_t` regardless of source bd) —
// it is ported verbatim as `intra_pred::cfl_subtract_average` (`pub fn`,
// takes `&mut [i16]`). Reuse that directly; zero new code needed for the
// subtract-average step.
//
// PORT-NOTE(unverified) on the two functions below: verify vs FFI parity
// once wired.
// =============================================================================
