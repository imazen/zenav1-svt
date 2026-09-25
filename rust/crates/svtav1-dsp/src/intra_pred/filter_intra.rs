use super::*;

/// Scale bits for filter-intra tap application.
pub(super) const FILTER_INTRA_SCALE_BITS: i32 = 4;

/// Filter-intra tap coefficients: 5 modes, 8 sub-block positions, 7 neighbor taps (+1 padding).
///
/// Indexed as `FILTER_INTRA_TAPS[mode][k][tap]` where:
/// - `mode` 0..5: the 5 filter-intra modes
/// - `k` 0..8: the 8 pixels in a 4x2 sub-block (row = k>>2, col = k&3)
/// - `tap` 0..7: coefficients for p0..p6 (tap 7 is always 0, present for alignment)
#[rustfmt::skip]
pub(super) static FILTER_INTRA_TAPS: [[[i8; 8]; 8]; 5] = [
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

/// Round a signed value by `n` bits (ROUND_POWER_OF_TWO_SIGNED).
#[inline]
pub(super) fn round_power_of_two_signed(value: i32, n: i32) -> i32 {
    if value < 0 {
        -((-value + (1 << (n - 1))) >> n)
    } else {
        (value + (1 << (n - 1))) >> n
    }
}

/// Predict a block using filter-intra prediction.
///
/// Ported from `svt_av1_filter_intra_predictor_c` in `filterintra_c.c`.
///
/// Uses a 33x33 intermediate buffer. Processes 4-wide by 2-tall sub-blocks,
/// each pixel computed from 7 neighbor pixels and the mode's tap coefficients.
///
/// # Arguments
/// * `above` - top-left + above pixels, length `width + 1`.
///   `above[0]` = top-left, `above[1..]` = pixels above the block.
/// * `left` - left column pixels, length `height`.
/// * `mode` - filter-intra mode (0..4).
pub fn predict_filter_intra(
    dst: &mut [u8],
    dst_stride: usize,
    above: &[u8],
    left: &[u8],
    width: usize,
    height: usize,
    mode: u8,
) {
    assert!(width <= 32 && height <= 32);
    assert!((mode as usize) < 5);
    // The dst block is written in place: sub-blocks are processed in order
    // (top-to-bottom row pairs, left-to-right inside each), and every tap
    // reads either `above`/`left` or a dst cell an EARLIER sub-block already
    // wrote — the C `buffer[33][33]` staging (and its per-call zero) exists
    // only because that buffer also held the border.  Buffer coords map as
    // `buf[0][c]` = `above[c]`, `buf[r][0]` = `left[r - 1]`, `buf[r][c]` =
    // `dst[(r - 1) * stride + (c - 1)]`.
    assert!(dst.len() >= (height - 1) * dst_stride + width);
    assert!(left.len() >= height);
    assert!(above.len() >= width + 1);
    incant!(
        predict_filter_intra_impl(dst, dst_stride, above, left, width, height, mode),
        [v3, neon, scalar]
    );
}

pub(super) fn predict_filter_intra_impl_scalar(
    _token: ScalarToken,
    dst: &mut [u8],
    dst_stride: usize,
    above: &[u8],
    left: &[u8],
    width: usize,
    height: usize,
    mode: u8,
) {
    predict_filter_intra_core(dst, dst_stride, above, left, width, height, mode);
}

/// Emit one 2x4 output block on NEON — the [`filter_intra_emit_v3`] body
/// restated without `maddubs`/`hadd`: the 8 output pixels share the 7-tap
/// input `p`, so each tap index contributes `p[j] * taps[k][j]` for all
/// eight `k` at once. `p[j] * tap` fits s16 (255 * 127 < 32768) but the
/// 7-term sum does not, so the accumulate runs `vmlal_s16` into i32.
/// `vrshrq_n_s32::<4>` is `(v + 8) >> 4`; it differs from
/// `ROUND_POWER_OF_TWO_SIGNED` only on negative sums that the u8 saturating
/// narrow clips to 0 either way — the same argument that makes C's
/// `mulhrs` kernel identical.
#[cfg(target_arch = "aarch64")]
#[rite]
pub(super) fn filter_intra_emit_neon(
    _token: NeonToken,
    cur: &mut [u8],
    dst_stride: usize,
    off: usize,
    p: &[u8; 8],
    tcol: &[int16x4_t; 14],
) {
    // tcol[j] = taps[0..4][j] (row-0 outputs k=0..3), tcol[7+j] =
    // taps[4..8][j] (row-1 outputs k=4..7).
    let mut acc_lo = vdupq_n_s32(0);
    let mut acc_hi = vdupq_n_s32(0);
    for j in 0..7 {
        let pv = vdup_n_s16(p[j] as i16);
        acc_lo = vmlal_s16(acc_lo, pv, tcol[j]);
        acc_hi = vmlal_s16(acc_hi, pv, tcol[7 + j]);
    }
    // Two saturating narrows: `vqmovun_s32` clamps negatives to 0 (u16
    // range), `vqmovn_u16` clamps the top at 255 — together exactly the
    // scalar `clamp(0, 255)`. Lanes 0..4 are row 0, 4..8 row 1.
    let pair16 = vcombine_u16(
        vqmovun_s32(vrshrq_n_s32::<4>(acc_lo)),
        vqmovun_s32(vrshrq_n_s32::<4>(acc_hi)),
    );
    let pair8 = vqmovn_u16(pair16);
    let mut b = [0u8; 8];
    vst1_u8(&mut b, pair8);
    let (row0, row1) = cur.split_at_mut(dst_stride);
    row0[off..off + 4].copy_from_slice(&b[..4]);
    row1[off..off + 4].copy_from_slice(&b[4..]);
}

#[cfg(target_arch = "aarch64")]
#[arcane]
pub(super) fn predict_filter_intra_impl_neon(
    token: NeonToken,
    dst: &mut [u8],
    dst_stride: usize,
    above: &[u8],
    left: &[u8],
    width: usize,
    height: usize,
    mode: u8,
) {
    // Tap columns for this mode: tcol[j] = taps[0..4][j] (row-0 outputs)
    // for j in 0..7, tcol[7+j] = taps[4..8][j] (row-1 outputs). i8 taps
    // widen to s16; products p[j]*tap fit s16, the 7-term sum needs i32.
    let taps = &FILTER_INTRA_TAPS[mode as usize];
    let mut tl = [[0i16; 4]; 14];
    for j in 0..7 {
        for k in 0..4 {
            tl[j][k] = taps[k][j] as i16;
            tl[7 + j][k] = taps[k + 4][j] as i16;
        }
    }
    let tcol: [int16x4_t; 14] = core::array::from_fn(|i| vld1_s16(&tl[i]));
    for r in (1..height + 1).step_by(2) {
        // Same row split as the scalar path / v3 arm.
        let (above_rows, cur) = dst.split_at_mut((r - 1) * dst_stride);
        let up: &[u8] = if r == 1 {
            &above[1..]
        } else {
            &above_rows[(r - 2) * dst_stride..]
        };
        {
            let mut p = [0u8; 8];
            p[0] = if r == 1 { above[0] } else { left[r - 2] };
            p[1..5].copy_from_slice(&up[0..4]);
            p[5] = left[r - 1];
            p[6] = left[r];
            filter_intra_emit_neon(token, cur, dst_stride, 0, &p, &tcol);
        }
        for c in (5..width + 1).step_by(4) {
            let mut p = [0u8; 8];
            p[..5].copy_from_slice(&up[c - 2..c + 3]);
            p[5] = cur[c - 2];
            p[6] = cur[dst_stride + c - 2];
            filter_intra_emit_neon(token, cur, dst_stride, c - 1, &p, &tcol);
        }
    }
}

/// Emit one 2x4 output block — C `svt_av1_filter_intra_predictor_sse4_1`'s
/// inner body (filterintra_sse4.c:50-71). `p` is the 8-tap vector
/// `[up[c-1..c+4] | cur[r][c-1] | cur[r+1][c-1] | 0]`; the four tap-pair
/// vectors and the mulhrs rounding constant are hoisted by the caller.
/// `mulhrs` differs from `ROUND_POWER_OF_TWO_SIGNED` only on negative sums
/// that both sides clip to 0 anyway — the same argument that makes C's two
/// kernels identical.
#[cfg(target_arch = "x86_64")]
#[rite]
#[allow(clippy::too_many_arguments)]
pub(super) fn filter_intra_emit_v3(
    _token: Desktop64,
    cur: &mut [u8],
    dst_stride: usize,
    off: usize,
    p: &[u8; 8],
    f1f0: __m128i,
    f3f2: __m128i,
    f5f4: __m128i,
    f7f6: __m128i,
    scale_bits: __m128i,
) {
    let p_b = _mm_loadu_si64(p);
    let in_v = _mm_unpacklo_epi64(p_b, p_b);
    let out_01 = _mm_maddubs_epi16(in_v, f1f0);
    let out_23 = _mm_maddubs_epi16(in_v, f3f2);
    let out_45 = _mm_maddubs_epi16(in_v, f5f4);
    let out_67 = _mm_maddubs_epi16(in_v, f7f6);
    let out_0123 = _mm_hadd_epi16(out_01, out_23);
    let out_4567 = _mm_hadd_epi16(out_45, out_67);
    let out_01234567 = _mm_hadd_epi16(out_0123, out_4567);
    let round_w = _mm_mulhrs_epi16(out_01234567, scale_bits);
    let out_r = _mm_packus_epi16(round_w, round_w);
    let (row0, row1) = cur.split_at_mut(dst_stride);
    let d0: &mut [u8; 4] = (&mut row0[off..off + 4]).try_into().unwrap();
    let d1: &mut [u8; 4] = (&mut row1[off..off + 4]).try_into().unwrap();
    _mm_storeu_si32(d0, out_r);
    _mm_storeu_si32(d1, _mm_srli_si128::<4>(out_r));
}

#[cfg(target_arch = "x86_64")]
#[arcane]
pub(super) fn predict_filter_intra_impl_v3(
    token: Desktop64,
    dst: &mut [u8],
    dst_stride: usize,
    above: &[u8],
    left: &[u8],
    width: usize,
    height: usize,
    mode: u8,
) {
    let taps = FILTER_INTRA_TAPS[mode as usize].as_flattened();
    let t01: &[i8; 16] = taps[0..16].try_into().unwrap();
    let t23: &[i8; 16] = taps[16..32].try_into().unwrap();
    let t45: &[i8; 16] = taps[32..48].try_into().unwrap();
    let t67: &[i8; 16] = taps[48..64].try_into().unwrap();
    let f1f0 = _mm_loadu_si128(t01);
    let f3f2 = _mm_loadu_si128(t23);
    let f5f4 = _mm_loadu_si128(t45);
    let f7f6 = _mm_loadu_si128(t67);
    let scale_bits = _mm_set1_epi16(1 << (15 - FILTER_INTRA_SCALE_BITS));

    for r in (1..height + 1).step_by(2) {
        // dst rows `r - 1` and `r` are written; row `r - 2` is the up-tap
        // source — same split as the scalar path.
        let (above_rows, cur) = dst.split_at_mut((r - 1) * dst_stride);
        let up: &[u8] = if r == 1 {
            &above[1..]
        } else {
            &above_rows[(r - 2) * dst_stride..]
        };
        {
            let mut p = [0u8; 8];
            p[0] = if r == 1 { above[0] } else { left[r - 2] };
            p[1..5].copy_from_slice(&up[0..4]);
            p[5] = left[r - 1];
            p[6] = left[r];
            filter_intra_emit_v3(
                token, cur, dst_stride, 0, &p, f1f0, f3f2, f5f4, f7f6, scale_bits,
            );
        }
        for c in (5..width + 1).step_by(4) {
            let mut p = [0u8; 8];
            p[..5].copy_from_slice(&up[c - 2..c + 3]);
            p[5] = cur[c - 2];
            p[6] = cur[dst_stride + c - 2];
            filter_intra_emit_v3(
                token,
                cur,
                dst_stride,
                c - 1,
                &p,
                f1f0,
                f3f2,
                f5f4,
                f7f6,
                scale_bits,
            );
        }
    }
}

pub(super) fn predict_filter_intra_core(
    dst: &mut [u8],
    dst_stride: usize,
    above: &[u8],
    left: &[u8],
    width: usize,
    height: usize,
    mode: u8,
) {
    let taps = &FILTER_INTRA_TAPS[mode as usize];

    // 4-wide by 2-tall sub-blocks. `up[j]` aliases buffer row `r - 1`
    // WITHOUT its left-border cell (`up[j]` == `buf[r-1][j+1]`) — `above[1..]`
    // for the first row pair, dst row `r - 2` afterwards. The leftmost
    // sub-block (`c == 1`, whose p0/p5/p6 are border cells) is peeled so the
    // interior loop is branch-free.
    for r in (1..height + 1).step_by(2) {
        // dst rows `r - 1` and `r` are written; row `r - 2` is the up-tap
        // source. Split at row `r - 1` so the shared up-row borrow coexists
        // with the writes: `cur[j]` == `dst[(r - 1) * stride + j]`.
        let (above_rows, cur) = dst.split_at_mut((r - 1) * dst_stride);
        let up: &[u8] = if r == 1 {
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
                cur[(k >> 2) * dst_stride + (k & 0x03)] =
                    round_power_of_two_signed(val, FILTER_INTRA_SCALE_BITS).clamp(0, 255) as u8;
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
                cur[r_offset * dst_stride + (c + c_offset - 1)] =
                    round_power_of_two_signed(val, FILTER_INTRA_SCALE_BITS).clamp(0, 255) as u8;
            }
        }
    }
}

// =============================================================================
// Palette prediction
// =============================================================================

/// Predict a block using palette mode.
///
/// Each pixel in the block is looked up from a palette of up to 8 colors,
/// using the color map indices.
///
/// # Arguments
/// * `color_map` - palette index per pixel (values 0..palette.len()), row-major
/// * `palette` - palette colors (up to 8 entries)
pub fn predict_palette(
    dst: &mut [u8],
    dst_stride: usize,
    color_map: &[u8],
    map_stride: usize,
    palette: &[u8],
    width: usize,
    height: usize,
) {
    for r in 0..height {
        for c in 0..width {
            dst[r * dst_stride + c] = palette[color_map[r * map_stride + c] as usize];
        }
    }
}

// =============================================================================
// Chroma-from-Luma (CfL) prediction
// Ported from svt_cfl_predict_lbd_c in cfl_c.c
// =============================================================================

/// CfL buffer line stride (max block width).
pub const CFL_BUF_LINE: usize = 32;

/// CfL luma subsampling: downsample luma 2x (for 4:2:0) and store as Q3.
///
/// Takes the reconstructed luma block and produces a Q3 AC prediction buffer
/// by averaging 2x2 luma samples and subtracting the DC component.
pub fn cfl_luma_subsampling_420(
    luma: &[u8],
    luma_stride: usize,
    output_q3: &mut [i16],
    width: usize,
    height: usize,
) {
    // Downsample 2x2 luma to chroma resolution
    for j in (0..height).step_by(2) {
        let out_row = (j / 2) * CFL_BUF_LINE;
        for i in (0..width).step_by(2) {
            let sum = luma[j * luma_stride + i] as i32
                + luma[j * luma_stride + i + 1] as i32
                + luma[(j + 1) * luma_stride + i] as i32
                + luma[(j + 1) * luma_stride + i + 1] as i32;
            // Q3 = sum * 2 (since we're averaging 4 pixels and want Q3 precision)
            output_q3[out_row + i / 2] = (sum * 2) as i16;
        }
    }
}

/// Subtract the DC (average) from the Q3 luma buffer to get AC-only values.
pub fn cfl_subtract_average(pred_buf_q3: &mut [i16], width: usize, height: usize) {
    let mut sum: i32 = 0;
    for j in 0..height {
        let row = j * CFL_BUF_LINE;
        for i in 0..width {
            sum += pred_buf_q3[row + i] as i32;
        }
    }
    let num_pel = width * height;
    let num_pel_log2 = (num_pel as u32).trailing_zeros();
    let round_offset = (1 << num_pel_log2) >> 1;
    let avg = (sum + round_offset) >> num_pel_log2;

    for j in 0..height {
        let row = j * CFL_BUF_LINE;
        for i in 0..width {
            pred_buf_q3[row + i] -= avg as i16;
        }
    }
}

/// CfL prediction: multiply alpha by AC luma values and add to DC chroma prediction.
///
/// `pred_buf_q3`: AC luma values in Q3 (from cfl_subtract_average)
/// `pred`: DC chroma prediction (e.g., from DC mode)
/// `dst`: output chroma prediction
/// `alpha_q3`: CfL alpha parameter in Q3
pub fn cfl_predict_lbd(
    pred_buf_q3: &[i16],
    pred: &[u8],
    pred_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    alpha_q3: i32,
    width: usize,
    height: usize,
) {
    crate::cfl_kernel::cfl_predict_lbd_dispatch(
        pred_buf_q3,
        pred,
        pred_stride,
        dst,
        dst_stride,
        alpha_q3,
        width,
        height,
    )
}

/// The per-element body, BRANCH-FREE so a `target_feature` region can vectorise
/// it.
///
/// `scaled_luma_q0` is C's round-half-away-from-zero by 6 bits
/// (`cfl_c.c`'s `get_scaled_luma_q0`), which the port used to write as an
/// `if q6 < 0 { -((-q6 + 32) >> 6) } else { (q6 + 32) >> 6 }`. The identity
/// `s = q6 >> 31` (0 or -1), `|q6| = (q6 ^ s) - s`, result `= (((|q6| + 32) >> 6)
/// ^ s) - s` is the same value for every `q6` except `i32::MIN`, where the
/// negation overflows — and `q6` is `alpha_q3 * ac_q3` with `|alpha_q3| <= 16`
/// (C `cfl_idx_to_alpha`, `CFL_ALPHABET_SIZE = 16`, magnitudes 1..=16) and
/// `ac_q3` an `i16`, so `|q6| <= 16 * 32768 = 524,288`. Pinned over the WHOLE
/// reachable domain by `cfl_branch_free_rounding_matches_the_branchy_form`.
#[inline]
pub(crate) fn cfl_predict_lbd_core(
    pred_buf_q3: &[i16],
    pred: &[u8],
    pred_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    alpha_q3: i32,
    width: usize,
    height: usize,
) {
    for j in 0..height {
        let ac = &pred_buf_q3[j * CFL_BUF_LINE..j * CFL_BUF_LINE + width];
        let p = &pred[j * pred_stride..j * pred_stride + width];
        let o = &mut dst[j * dst_stride..j * dst_stride + width];
        for ((o, &ac), &p) in o.iter_mut().zip(ac).zip(p) {
            let q6 = alpha_q3 * ac as i32;
            let s = q6 >> 31;
            let q0 = ((((q6 ^ s) - s) + 32) >> 6) ^ s;
            let val = (q0 - s) + p as i32;
            *o = val.clamp(0, 255) as u8;
        }
    }
}
