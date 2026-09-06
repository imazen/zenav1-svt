//! Intra prediction modes.
//!
//! Spec 05 (intra-prediction.md): All AV1 intra prediction modes.
//!
//! Ported from SVT-AV1's `intra_prediction.c` and `enc_intra_prediction.c`.
//!
//! AV1 defines 13 intra prediction modes: DC, V, H, 8 directional,
//! smooth/smooth_v/smooth_h, and paeth.
//!
//! Key functions use archmage SIMD dispatch for auto-vectorization.

use archmage::prelude::*;

/// Predict a block using DC prediction (average of above + left neighbors).
///
/// `above`: pixels above the block (width pixels)
/// `left`: pixels to the left of the block (height pixels)
pub fn predict_dc(
    dst: &mut [u8],
    dst_stride: usize,
    above: &[u8],
    left: &[u8],
    width: usize,
    height: usize,
    has_above: bool,
    has_left: bool,
) {
    let dc = match (has_above, has_left) {
        (true, true) => {
            let sum: u32 = above[..width].iter().map(|&v| v as u32).sum::<u32>()
                + left[..height].iter().map(|&v| v as u32).sum::<u32>();
            let count = (width + height) as u32;
            ((sum + count / 2) / count) as u8
        }
        (true, false) => {
            let sum: u32 = above[..width].iter().map(|&v| v as u32).sum();
            ((sum + width as u32 / 2) / width as u32) as u8
        }
        (false, true) => {
            let sum: u32 = left[..height].iter().map(|&v| v as u32).sum();
            ((sum + height as u32 / 2) / height as u32) as u8
        }
        (false, false) => 128,
    };

    if width == 0 || height == 0 {
        return;
    }
    if dst_stride == width {
        dst[..width * height].fill(dc);
    } else {
        for row in 0..height {
            dst[row * dst_stride..row * dst_stride + width].fill(dc);
        }
    }
}

/// Predict a block using vertical prediction (copy above row).
pub fn predict_v(dst: &mut [u8], dst_stride: usize, above: &[u8], width: usize, height: usize) {
    for row in 0..height {
        dst[row * dst_stride..row * dst_stride + width].copy_from_slice(&above[..width]);
    }
}

/// Predict a block using horizontal prediction (copy left column).
pub fn predict_h(dst: &mut [u8], dst_stride: usize, left: &[u8], width: usize, height: usize) {
    for row in 0..height {
        let val = left[row];
        for col in 0..width {
            dst[row * dst_stride + col] = val;
        }
    }
}

/// Predict a block using smooth prediction (weighted combination of above, left,
/// and corner values using smooth weight tables).
///
/// Smooth is a bilinear interpolation between above[c], left[r],
/// above[width-1] (right), and left[height-1] (bottom).
pub fn predict_smooth(
    dst: &mut [u8],
    dst_stride: usize,
    above: &[u8],
    left: &[u8],
    width: usize,
    height: usize,
) {
    let below_pred = left[height - 1] as u32;
    let right_pred = above[width - 1] as u32;

    let sm_weights_h = smooth_weights(height);
    let sm_weights_w = smooth_weights(width);

    for row in 0..height {
        for col in 0..width {
            let wh = sm_weights_h[row] as u32;
            let ww = sm_weights_w[col] as u32;
            let top = above[col] as u32;
            let lft = left[row] as u32;

            // Smooth interpolation
            let pred =
                (wh * top + (256 - wh) * below_pred + ww * lft + (256 - ww) * right_pred + 256)
                    / 512;
            dst[row * dst_stride + col] = pred.min(255) as u8;
        }
    }
}

/// Predict a block using smooth vertical (only vertical interpolation).
pub fn predict_smooth_v(
    dst: &mut [u8],
    dst_stride: usize,
    above: &[u8],
    left: &[u8],
    _width: usize,
    height: usize,
    width: usize,
) {
    let below_pred = left[height - 1] as u32;
    let sm_weights = smooth_weights(height);

    for row in 0..height {
        let w = sm_weights[row] as u32;
        for col in 0..width {
            let top = above[col] as u32;
            let pred = (w * top + (256 - w) * below_pred + 128) / 256;
            dst[row * dst_stride + col] = pred.min(255) as u8;
        }
    }
}

/// Predict a block using smooth horizontal (only horizontal interpolation).
pub fn predict_smooth_h(
    dst: &mut [u8],
    dst_stride: usize,
    above: &[u8],
    left: &[u8],
    width: usize,
    height: usize,
) {
    let right_pred = above[width - 1] as u32;
    let sm_weights = smooth_weights(width);

    for row in 0..height {
        let lft = left[row] as u32;
        for col in 0..width {
            let w = sm_weights[col] as u32;
            let pred = (w * lft + (256 - w) * right_pred + 128) / 256;
            dst[row * dst_stride + col] = pred.min(255) as u8;
        }
    }
}

/// Predict a block using Paeth prediction.
///
/// For each pixel, choose the nearest of above[c], left[r], or
/// above[-1] (top-left corner) based on gradient direction.
pub fn predict_paeth(
    dst: &mut [u8],
    dst_stride: usize,
    above: &[u8],
    left: &[u8],
    top_left: u8,
    width: usize,
    height: usize,
) {
    incant!(
        predict_paeth_impl(dst, dst_stride, above, left, top_left, width, height),
        [v3, neon, scalar]
    );
}

fn predict_paeth_impl_scalar(
    _token: ScalarToken,
    dst: &mut [u8],
    dst_stride: usize,
    above: &[u8],
    left: &[u8],
    top_left: u8,
    width: usize,
    height: usize,
) {
    predict_paeth_core(dst, dst_stride, above, left, top_left, width, height);
}

#[cfg(target_arch = "x86_64")]
#[arcane]
fn predict_paeth_impl_v3(
    _token: Desktop64,
    dst: &mut [u8],
    dst_stride: usize,
    above: &[u8],
    left: &[u8],
    top_left: u8,
    width: usize,
    height: usize,
) {
    predict_paeth_core(dst, dst_stride, above, left, top_left, width, height);
}

#[cfg(target_arch = "aarch64")]
#[arcane]
fn predict_paeth_impl_neon(
    _token: NeonToken,
    dst: &mut [u8],
    dst_stride: usize,
    above: &[u8],
    left: &[u8],
    top_left: u8,
    width: usize,
    height: usize,
) {
    // Paeth in i16 lanes. Within a ROW, `left` and `top_left` are constants and
    // only `above` varies by column, which collapses one of the three
    // distances to a per-row scalar:
    //
    //   base   = top + lft - tl
    //   p_top  = |base - top| = |lft - tl|            <- row-constant
    //   p_left = |base - lft| = |top - tl|
    //   p_tl   = |base - tl|  = |top + lft - 2*tl|
    //
    // i16 is required, not u16: `top + lft - 2*tl` spans [-510, 510].
    //
    // Exact — every operation is integer add/sub/abs/compare/select, so this
    // is bit-identical to the scalar core. Pinned by
    // tests/paeth_neon_parity.rs, which sweeps all 2^24 (top,left,tl) triples
    // through the 1-wide path and every block shape through the vector path.
    let tl = top_left as i16;
    let tl_v = vdupq_n_s16(tl);
    let two_tl_v = vdupq_n_s16(tl * 2);

    for row in 0..height {
        let lft = left[row] as i16;
        let lft_v = vdupq_n_s16(lft);
        let p_top_v = vdupq_n_s16((lft - tl).abs());
        let base_row = row * dst_stride;

        let mut col = 0;
        while col + 8 <= width {
            let a: &[u8; 8] = above[col..col + 8].try_into().unwrap();
            let top_v = vreinterpretq_s16_u16(vmovl_u8(vld1_u8(a)));

            let p_left_v = vabdq_s16(top_v, tl_v);
            let p_tl_v = vabdq_s16(vaddq_s16(top_v, lft_v), two_tl_v);

            // if p_top <= p_left && p_top <= p_tl { top }
            // else if p_left <= p_tl { lft } else { tl }
            let take_top = vandq_u16(vcleq_s16(p_top_v, p_left_v), vcleq_s16(p_top_v, p_tl_v));
            let take_left = vcleq_s16(p_left_v, p_tl_v);

            let pred = vbslq_s16(take_left, lft_v, tl_v);
            let pred = vbslq_s16(take_top, top_v, pred);

            let out: &mut [u8; 8] = (&mut dst[base_row + col..base_row + col + 8])
                .try_into()
                .unwrap();
            vst1_u8(out, vmovn_u16(vreinterpretq_u16_s16(pred)));
            col += 8;
        }

        // Tail: the scalar form verbatim.
        while col < width {
            let top = above[col] as i32;
            let l = lft as i32;
            let t = tl as i32;
            let base = top + l - t;
            let p_top = (base - top).abs();
            let p_left = (base - l).abs();
            let p_tl = (base - t).abs();
            dst[base_row + col] = if p_top <= p_left && p_top <= p_tl {
                top as u8
            } else if p_left <= p_tl {
                l as u8
            } else {
                t as u8
            };
            col += 1;
        }
    }
}

/// Core Paeth implementation (shared across dispatch tiers).
#[inline]
fn predict_paeth_core(
    dst: &mut [u8],
    dst_stride: usize,
    above: &[u8],
    left: &[u8],
    top_left: u8,
    width: usize,
    height: usize,
) {
    for row in 0..height {
        for col in 0..width {
            let top = above[col] as i32;
            let lft = left[row] as i32;
            let tl = top_left as i32;

            let base = top + lft - tl;
            let p_top = (base - top).abs();
            let p_left = (base - lft).abs();
            let p_tl = (base - tl).abs();

            let pred = if p_top <= p_left && p_top <= p_tl {
                top
            } else if p_left <= p_tl {
                lft
            } else {
                tl
            };
            dst[row * dst_stride + col] = pred as u8;
        }
    }
}

// =============================================================================
// Directional prediction (8 angular modes)
// Ported from svt_av1_dr_prediction_z1/z2/z3_c in intra_prediction.c
// =============================================================================

/// Derivative table for directional prediction angles.
/// `eb_dr_intra_derivative[angle]` = 256/tan(angle) for zone 1 (0-90°).
static DR_INTRA_DERIVATIVE: [u16; 90] = [
    0, 0, 0, 1023, 0, 0, 547, 0, 0, 372, 0, 0, 0, 0, 273, 0, 0, 215, 0, 0, 178, 0, 0, 151, 0, 0,
    132, 0, 0, 116, 0, 0, 102, 0, 0, 0, 90, 0, 0, 80, 0, 0, 71, 0, 0, 64, 0, 0, 57, 0, 0, 51, 0, 0,
    45, 0, 0, 0, 40, 0, 0, 35, 0, 0, 31, 0, 0, 27, 0, 0, 23, 0, 0, 19, 0, 0, 15, 0, 0, 0, 0, 11, 0,
    0, 7, 0, 0, 3, 0, 0,
];

/// Base angles for directional intra modes.
pub const MODE_TO_ANGLE: [i32; 8] = [
    0, // D45_PRED  → 45°
    0, // D135_PRED → 135°
    0, // D113_PRED → 113°
    0, // D157_PRED → 157°
    0, // D203_PRED → 203°
    0, // D67_PRED  → 67°
    0, 0,
];

fn get_dx(angle: i32) -> i32 {
    if angle > 0 && angle < 90 {
        DR_INTRA_DERIVATIVE[angle as usize] as i32
    } else if angle > 90 && angle < 180 {
        DR_INTRA_DERIVATIVE[(180 - angle) as usize] as i32
    } else {
        1
    }
}

fn get_dy(angle: i32) -> i32 {
    if angle > 90 && angle < 180 {
        DR_INTRA_DERIVATIVE[(angle - 90) as usize] as i32
    } else if angle > 180 && angle < 270 {
        DR_INTRA_DERIVATIVE[(270 - angle) as usize] as i32
    } else {
        1
    }
}

/// Directional prediction, zone 1: 0 < angle < 90.
/// Interpolates along the `above` neighbor row.
fn dr_prediction_z1(
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u8],
    dx: i32,
) {
    let max_base_x = (bw + bh) as i32 - 1;
    for r in 0..bh {
        let x = dx * (r as i32 + 1);
        let base = x >> 6;
        let shift = ((x) & 0x3F) >> 1;

        for c in 0..bw {
            let b = base + c as i32;
            if b < max_base_x {
                let val = above[b as usize] as i32 * (32 - shift)
                    + above[(b + 1) as usize] as i32 * shift;
                dst[r * dst_stride + c] = ((val + 16) >> 5).clamp(0, 255) as u8;
            } else {
                dst[r * dst_stride + c] = above[max_base_x as usize];
            }
        }
    }
}

/// Directional prediction, zone 3: 180 < angle < 270.
/// Interpolates along the `left` neighbor column.
fn dr_prediction_z3(dst: &mut [u8], dst_stride: usize, bw: usize, bh: usize, left: &[u8], dy: i32) {
    let max_base_y = (bw + bh) as i32 - 1;
    for c in 0..bw {
        let y = dy * (c as i32 + 1);
        let base = y >> 6;
        let shift = ((y) & 0x3F) >> 1;

        for r in 0..bh {
            let b = base + r as i32;
            if b < max_base_y {
                let val =
                    left[b as usize] as i32 * (32 - shift) + left[(b + 1) as usize] as i32 * shift;
                dst[r * dst_stride + c] = ((val + 16) >> 5).clamp(0, 255) as u8;
            } else {
                dst[r * dst_stride + c] = left[max_base_y as usize];
            }
        }
    }
}

/// Directional prediction, zone 2: 90 < angle < 180.
/// Interpolates using both `above` and `left` neighbors plus the
/// top-left corner sample.
///
/// C-exact port of libaom `av1_dr_prediction_z2_c`
/// (av1/common/reconintra.c), specialized to
/// `upsample_above == upsample_left == 0` (our sequence headers signal
/// `enable_intra_edge_filter = 0`, so upsampling never applies):
/// `min_base_x = -1`, `frac_bits_x = frac_bits_y = 6`.
///
/// The C code indexes `above[-1]` / `left[-1]` — the top-left neighbor
/// sample (`above_row[-1] == left_col[-1]` in
/// `build_directional_and_filter_intra_predictors`). Rust slices cannot
/// be indexed at -1, so that sample is passed separately as `top_left`.
fn dr_prediction_z2(
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u8],
    left: &[u8],
    top_left: u8,
    dx: i32,
    dy: i32,
) {
    debug_assert!(dx > 0);
    debug_assert!(dy > 0);
    for r in 0..bh {
        for c in 0..bw {
            // C: int y = r + 1; int x = (c << 6) - y * dx;
            //    const int base_x = x >> frac_bits_x;
            let y = r as i32 + 1;
            let x = ((c as i32) << 6) - y * dx;
            let base_x = x >> 6;
            let val = if base_x >= -1 {
                // C: shift = ((x * (1 << upsample_above)) & 0x3F) >> 1;
                //    val = above[base_x] * (32 - shift) + above[base_x + 1] * shift;
                let shift = (x & 0x3F) >> 1;
                let p0 = if base_x < 0 {
                    top_left
                } else {
                    above[base_x as usize]
                } as i32;
                let p1 = above[(base_x + 1) as usize] as i32;
                p0 * (32 - shift) + p1 * shift
            } else {
                // C: x = c + 1; y = (r << 6) - x * dy;
                //    const int base_y = y >> frac_bits_y;
                //    assert(base_y >= min_base_y);
                let x2 = c as i32 + 1;
                let y2 = ((r as i32) << 6) - x2 * dy;
                let base_y = y2 >> 6;
                debug_assert!(base_y >= -1);
                let shift = (y2 & 0x3F) >> 1;
                let p0 = if base_y < 0 {
                    top_left
                } else {
                    left[base_y as usize]
                } as i32;
                let p1 = left[(base_y + 1) as usize] as i32;
                p0 * (32 - shift) + p1 * shift
            };
            // C: ROUND_POWER_OF_TWO(val, 5) — val is a 32-weight blend of
            // u8 samples, so (val + 16) >> 5 <= 255; no clamp in C either.
            dst[r * dst_stride + c] = ((val + 16) >> 5) as u8;
        }
    }
}

/// Predict a block using directional prediction at the given angle.
///
/// `angle` is in degrees (0-270). The 8 directional modes map to:
/// D45=45, D67=67, D113=113, D135=135, D157=157, D203=203
///
/// Mirrors libaom `dr_predictor` (av1/common/reconintra.c) with
/// `upsample_above == upsample_left == 0`.
///
/// Neighbor array requirements (all indices relative to the block origin,
/// exactly as libaom's `above_row` / `left_col` after
/// `build_directional_and_filter_intra_predictors`):
/// - zone 1 (angle < 90): `above` needs `width + height` samples
///   (block top row + top-right extension).
/// - zone 2 (90 < angle < 180): `above` needs `width`, `left` needs
///   `height`, plus `top_left` (the C `above_row[-1] == left_col[-1]`).
/// - zone 3 (angle > 180): `left` needs `width + height` samples
///   (left column + bottom-left extension).
pub fn predict_directional(
    dst: &mut [u8],
    dst_stride: usize,
    above: &[u8],
    left: &[u8],
    top_left: u8,
    width: usize,
    height: usize,
    angle: i32,
) {
    let dx = get_dx(angle);
    let dy = get_dy(angle);

    if angle > 0 && angle < 90 {
        dr_prediction_z1(dst, dst_stride, width, height, above, dx);
    } else if angle > 90 && angle < 180 {
        dr_prediction_z2(
            dst, dst_stride, width, height, above, left, top_left, dx, dy,
        );
    } else if angle > 180 && angle < 270 {
        dr_prediction_z3(dst, dst_stride, width, height, left, dy);
    } else if angle == 90 {
        // V_PRED
        predict_v(dst, dst_stride, above, width, height);
    } else if angle == 180 {
        // H_PRED
        predict_h(dst, dst_stride, left, width, height);
    }
}

// =============================================================================
// Intra edge filtering / upsampling + upsample-capable directional prediction
// (C SVT-AV1 intra_prediction.c: svt_aom_intra_edge_filter_strength,
// svt_aom_use_intra_edge_upsample, svt_av1_filter_intra_edge_c,
// filter_intra_edge_corner, svt_av1_upsample_intra_edge_c, and the
// upsample-aware svt_av1_dr_prediction_z1/z2/z3_c.)
//
// The edged buffers mirror C's `above_data[.. ]`/`left_data[..]` with the
// block origin at index `EDGE_ORIGIN` (C `above_row = above_data + 16`):
// upsampling writes p[-2], zone-2 reads above[-1]/left[-1] (the top-left
// sample) and, when upsampled, above[-2]/left[-2].
// =============================================================================

/// Origin index inside the edged above/left buffers (C `+ 16`).
pub const EDGE_ORIGIN: usize = 16;
/// Edged buffer length (C `MAX_TX_SIZE * 2 + 32` = 160).
pub const EDGE_BUF_LEN: usize = 64 * 2 + 32;

/// C `svt_aom_intra_edge_filter_strength` (intra_prediction.c:180).
#[allow(clippy::if_same_then_else)] // C intra_prediction.c:190-197 really does have two
// identical `d >= 40` arms for blk_wh <= 12 and <= 16; the port stays shape-faithful
pub fn intra_edge_filter_strength(bs0: i32, bs1: i32, delta: i32, filt_type: i32) -> i32 {
    let d = delta.abs();
    let blk_wh = bs0 + bs1;
    let mut strength = 0;
    if filt_type == 0 {
        if blk_wh <= 8 {
            if d >= 56 {
                strength = 1;
            }
        } else if blk_wh <= 12 {
            if d >= 40 {
                strength = 1;
            }
        } else if blk_wh <= 16 {
            if d >= 40 {
                strength = 1;
            }
        } else if blk_wh <= 24 {
            if d >= 8 {
                strength = 1;
            }
            if d >= 16 {
                strength = 2;
            }
            if d >= 32 {
                strength = 3;
            }
        } else if blk_wh <= 32 {
            if d >= 1 {
                strength = 1;
            }
            if d >= 4 {
                strength = 2;
            }
            if d >= 32 {
                strength = 3;
            }
        } else if d >= 1 {
            strength = 3;
        }
    } else if blk_wh <= 8 {
        if d >= 40 {
            strength = 1;
        }
        if d >= 64 {
            strength = 2;
        }
    } else if blk_wh <= 16 {
        if d >= 20 {
            strength = 1;
        }
        if d >= 48 {
            strength = 2;
        }
    } else if blk_wh <= 24 {
        if d >= 4 {
            strength = 3;
        }
    } else if d >= 1 {
        strength = 3;
    }
    strength
}

/// C `svt_aom_use_intra_edge_upsample` (intra_prediction.c:144).
pub fn use_intra_edge_upsample(bs0: i32, bs1: i32, delta: i32, filt_type: i32) -> bool {
    let d = delta.abs();
    let blk_wh = bs0 + bs1;
    if d <= 0 || d >= 40 {
        return false;
    }
    if filt_type != 0 {
        blk_wh <= 8
    } else {
        blk_wh <= 16
    }
}

/// C `svt_av1_filter_intra_edge_c` (intra_prediction.c:156) applied to
/// `p[start .. start + sz]` (C receives the pointer already offset — the
/// caller passes `origin - ab_le`).
pub fn filter_intra_edge(p: &mut [u8], start: usize, sz: usize, strength: i32) {
    if strength == 0 {
        return;
    }
    const KERNEL: [[i32; 5]; 3] = [[0, 4, 8, 4, 0], [0, 5, 6, 5, 0], [2, 4, 4, 4, 2]];
    let filt = (strength - 1) as usize;
    debug_assert!(sz <= 129);
    let mut edge = [0u8; 129];
    edge[..sz].copy_from_slice(&p[start..start + sz]);
    for i in 1..sz {
        let mut s = 0i32;
        for (j, &k_w) in KERNEL[filt].iter().enumerate() {
            let k = (i as i32 - 2 + j as i32).clamp(0, sz as i32 - 1) as usize;
            s += edge[k] as i32 * k_w;
        }
        p[start + i] = ((s + 8) >> 4) as u8;
    }
}

/// C `filter_intra_edge_corner` (intra_prediction.c:2356): smooths the
/// shared corner sample `above[origin-1] == left[origin-1]`.
pub fn filter_intra_edge_corner(above: &mut [u8], left: &mut [u8], origin: usize) {
    let s = (left[origin] as i32 * 5 + above[origin - 1] as i32 * 6 + above[origin] as i32 * 5 + 8)
        >> 4;
    above[origin - 1] = s as u8;
    left[origin - 1] = s as u8;
}

/// C `svt_av1_upsample_intra_edge_c` (C_DEFAULT/intra_prediction_c.c:39):
/// 2x upsample of `p[origin .. origin + sz]` in place, writing
/// `p[origin - 2 .. origin + 2 * sz - 1]` (reads `p[origin - 1]`).
pub fn upsample_intra_edge(p: &mut [u8], origin: usize, sz: usize) {
    debug_assert!(sz <= 16, "C MAX_UPSAMPLE_SZ");
    debug_assert!(origin >= 2);
    let mut input = [0u8; 16 + 3];
    input[0] = p[origin - 1];
    input[1] = p[origin - 1];
    input[2..2 + sz].copy_from_slice(&p[origin..origin + sz]);
    input[sz + 2] = p[origin + sz - 1];

    p[origin - 2] = input[0];
    for i in 0..sz {
        let s = -(input[i] as i32) + 9 * input[i + 1] as i32 + 9 * input[i + 2] as i32
            - input[i + 3] as i32;
        let s = ((s + 8) >> 4).clamp(0, 255);
        p[origin + 2 * i - 1] = s as u8;
        p[origin + 2 * i] = input[i + 2];
    }
}

/// C `svt_av1_dr_prediction_z1_c` (intra_prediction.c:351) with upsampling.
/// `above[origin + i]` is C `above[i]`.
///
/// Dispatches to a NEON arm on the NON-upsampled path, which is the one the
/// large blocks take (`svt_aom_use_intra_edge_upsample` can only return 1 for
/// `bw + bh <= 16`, C intra_prediction.c). The upsampled path and any caller
/// whose edge buffer is too short for a 16-lane load stay on the scalar core.
#[allow(clippy::too_many_arguments)]
fn dr_z1_edged(
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u8],
    origin: usize,
    upsample_above: bool,
    dx: i32,
) {
    // The vector arm reads `above[origin + base + c + 1 .. + 16]` with
    // `base + c < max_base_x = bw + bh - 1`, so the highest index it can touch
    // is `origin + max_base_x + 1`. Anything shorter takes the scalar core.
    // `bw >= 16` is the gate, not just a guard: below it the vector loop
    // cannot run a single 16-lane chunk (`valid <= bw`), so the `incant!`
    // would summon a token and cross a target-feature boundary to execute
    // the scalar tail. MEASURED: without this the whole change is 0.992x at
    // 256x256 p6 (a REGRESSION, span entirely above 1.0) while still being
    // 1.016x at 512x512 p2 -- the small-block call rate at the fast presets
    // pays the dispatch and gets nothing back.
    if bw >= 16 && !upsample_above && above.len() > origin + bw + bh {
        incant!(
            dr_z1_edged_flat(dst, dst_stride, bw, bh, above, origin, dx),
            [neon, scalar]
        );
        return;
    }
    dr_z1_edged_core(dst, dst_stride, bw, bh, above, origin, upsample_above, dx);
}

fn dr_z1_edged_flat_scalar(
    _token: ScalarToken,
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u8],
    origin: usize,
    dx: i32,
) {
    dr_z1_edged_core(dst, dst_stride, bw, bh, above, origin, false, dx);
}

/// # Why this is a hand-written per-ISA arm and not `#[magetypes]`
///
/// The kernel is `u8 -> u16 -> u8`: it widens two `u8x16` loads to `u16x8`
/// pairs, accumulates `(a0 << 5) + (a1 - a0) * shift` there, and narrows back
/// with a ROUNDING shift. magetypes 0.9.28 cannot express any of those three
/// steps, verified by source read of the local checkout
/// (`~/work/archmage/magetypes`), not from memory:
/// * `src/simd/backends/convert_int.rs` carries **only same-width bitcasts**
///   (`bitcast_u8_to_i8`, `bitcast_u16_to_i16`, ...); there is no
///   `u8x16 -> u16x8` widening in either direction, and
///   `src/simd/generic/cross_width.rs` is **f32-only** (`f32x4 <-> f32x8 <->
///   f32x16`).
/// * `U16x8Backend` has no rounding narrowing shift (`vrshrn`-shaped) and no
///   widening multiply-accumulate (`vmlal`-shaped).
/// * `u8x16`'s only reduction is `reduce_add(..) -> u8`, which wraps.
///
/// So this takes the same route `crate::me_sad` documents for the SAD family:
/// `incant!` + per-ISA `#[arcane]`. If magetypes gains integer widening
/// (`u8xN <-> u16xN`, `i16xN <-> i32xN`) plus a rounding narrowing shift, this
/// kernel and `dr_z3_edged_flat_neon` collapse into one generic body.
///
/// C `svt_av1_dr_prediction_z1_neon`'s inner loop
/// (`ASM_NEON/intra_prediction_neon.c:89`, the `_large` variant), at
/// `upsample_above == 0`.
///
/// EXACT, not an approximation. C's NEON arm rewrites the scalar
/// `(a0 * (32 - shift) + a1 * shift + 16) >> 5` as
/// `((a0 << 5) + (a1 - a0) * shift + 16) >> 5`. The two are the same integer:
/// they differ by algebra only, the true value lies in `[0, 255 * 32]`, and
/// `vsubl_u8(a1, a0)` wrapping when `a1 < a0` cancels in the `vmlaq_u16`
/// because everything is mod 2^16 and the true result fits in 16 bits. No
/// clamp is needed for the same reason (the scalar core's `.clamp(0, 255)` can
/// never fire), and `vrshrn_n_u16::<5>` IS `(v + 16) >> 5`.
#[cfg(target_arch = "aarch64")]
#[arcane]
fn dr_z1_edged_flat_neon(
    _token: NeonToken,
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u8],
    origin: usize,
    dx: i32,
) {
    let max_base_x = (bw + bh) as i32 - 1;
    let fill = above[origin + max_base_x as usize];
    let mut x = dx;
    for r in 0..bh {
        let base = x >> 6;
        if base >= max_base_x {
            for row in dst.chunks_mut(dst_stride).skip(r).take(bh - r) {
                row[..bw].fill(fill);
            }
            return;
        }
        let bi = origin + base as usize;
        let shift = ((x & 0x3f) >> 1) as u16;
        let shift_v = vdupq_n_u16(shift);
        // Columns `c` with `base + c < max_base_x` take the interpolation;
        // the rest take `above[max_base_x]`, exactly as the scalar core does.
        let valid = core::cmp::min(bw, (max_base_x - base) as usize);
        let drow = &mut dst[r * dst_stride..r * dst_stride + bw];
        let mut c = 0;
        while c + 16 <= valid {
            let a0: &[u8; 16] = above[bi + c..bi + c + 16].try_into().unwrap();
            let a1: &[u8; 16] = above[bi + c + 1..bi + c + 17].try_into().unwrap();
            let a0v = vld1q_u8(a0);
            let a1v = vld1q_u8(a1);
            let lo = vmlaq_u16(
                vshll_n_u8::<5>(vget_low_u8(a0v)),
                vsubl_u8(vget_low_u8(a1v), vget_low_u8(a0v)),
                shift_v,
            );
            let hi = vmlaq_u16(
                vshll_n_u8::<5>(vget_high_u8(a0v)),
                vsubl_u8(vget_high_u8(a1v), vget_high_u8(a0v)),
                shift_v,
            );
            let out = vcombine_u8(vrshrn_n_u16::<5>(lo), vrshrn_n_u16::<5>(hi));
            let o: &mut [u8; 16] = (&mut drow[c..c + 16]).try_into().unwrap();
            vst1q_u8(o, out);
            c += 16;
        }
        let sh = shift as i32;
        while c < valid {
            let v = (above[bi + c] as i32 * (32 - sh) + above[bi + c + 1] as i32 * sh + 16) >> 5;
            drow[c] = v as u8;
            c += 1;
        }
        drow[c..].fill(fill);
        x += dx;
    }
}

/// Scalar C `svt_av1_dr_prediction_z1_c` (intra_prediction.c:351) with
/// upsampling. `above[origin + i]` is C `above[i]`. Reached through
/// [`dr_z1_edged`], which routes the flat (non-upsampled) case to a NEON arm.
#[allow(clippy::too_many_arguments)]
fn dr_z1_edged_core(
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u8],
    origin: usize,
    upsample_above: bool,
    dx: i32,
) {
    let up = upsample_above as i32;
    let max_base_x = ((bw + bh) as i32 - 1) << up;
    let frac_bits = 6 - up;
    let base_inc = 1i32 << up;
    let mut x = dx;
    for r in 0..bh {
        let mut base = x >> frac_bits;
        let shift = ((x << up) & 0x3F) >> 1;
        if base >= max_base_x {
            let fill = above[origin + max_base_x as usize];
            for row in dst.chunks_mut(dst_stride).skip(r).take(bh - r) {
                row[..bw].fill(fill);
            }
            return;
        }
        for c in 0..bw {
            let v = if base < max_base_x {
                let val = above[origin + base as usize] as i32 * (32 - shift)
                    + above[origin + base as usize + 1] as i32 * shift;
                ((val + 16) >> 5).clamp(0, 255) as u8
            } else {
                above[origin + max_base_x as usize]
            };
            dst[r * dst_stride + c] = v;
            base += base_inc;
        }
        x += dx;
    }
}

/// C `svt_av1_dr_prediction_z2_c` (intra_prediction.c:386) with upsampling.
/// Reads `above[origin - 1 - up_above ..]` and `left[origin - 1 - up_left ..]`.
///
/// Dispatches to a NEON arm on the NON-upsampled path, which is the one every
/// block of 16 or more takes (`svt_aom_use_intra_edge_upsample` can only
/// return 1 for `bw + bh <= 16`). The scalar core stays the oracle for the
/// upsampled path and for any caller whose edge buffers are too short for a
/// 16-lane load.
#[allow(clippy::too_many_arguments)]
fn dr_z2_edged(
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u8],
    left: &[u8],
    origin: usize,
    upsample_above: bool,
    upsample_left: bool,
    dx: i32,
    dy: i32,
) {
    debug_assert!(dx > 0 && dy > 0);
    #[cfg(target_arch = "x86_64")]
    if (4..=64).contains(&bw)
        && (4..=64).contains(&bh)
        && let Some(token) = X64V3Token::summon()
    {
        dr_z2_edged_split_v3(
            token,
            dst,
            dst_stride,
            bw,
            bh,
            above,
            left,
            origin,
            upsample_above,
            upsample_left,
            dx,
            dy,
        );
        return;
    }

    // `bw >= 16 && bh >= 16` for the reason [`dr_z1_edged`] records: below it
    // neither pass can run a single 16-lane chunk, so the `incant!` would
    // summon a token and cross a target-feature boundary to execute the
    // scalar core.
    //
    // The load bounds. Pass 1 reads `above[origin + base_x + c ..= + 16]` with
    // `c + 16 <= bw` and `base_x <= -1`, so its highest index is
    // `origin + bw - 1`. Pass 2 reads `left[origin + by0 + r ..= + 16]` with
    // `r + 16 <= bh` and `by0 <= -1`, highest `origin + bh - 1`. The `+ 16`
    // in the guards is slack, not a requirement.
    if bw >= 16
        && (16..=64).contains(&bh)
        && !upsample_above
        && !upsample_left
        && above.len() >= origin + bw + 16
        && left.len() >= origin + bh + 16
    {
        incant!(
            dr_z2_edged_flat(dst, dst_stride, bw, bh, above, left, origin, dx, dy),
            [neon, scalar]
        );
        return;
    }
    dr_z2_edged_core(
        dst,
        dst_stride,
        bw,
        bh,
        above,
        left,
        origin,
        upsample_above,
        upsample_left,
        dx,
        dy,
    );
}

/// The scalar C formula switches edges at one column per row. Hoist that
/// boundary and the above-edge interpolation weight out of the pixel loop.
/// The contiguous above suffix can then auto-vectorize under AVX2 without
/// per-pixel edge selection. Specializing the two upsample flags keeps its
/// source stride constant (one or two bytes). The prefix retains C's left
/// indexing and rounding exactly; no gather primitive is needed.
#[cfg(target_arch = "x86_64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn dr_z2_edged_split_v3(
    _token: X64V3Token,
    dst: &mut [u8],
    stride: usize,
    w: usize,
    h: usize,
    above: &[u8],
    left: &[u8],
    origin: usize,
    ua: bool,
    ul: bool,
    dx: i32,
    dy: i32,
) {
    match (ua, ul) {
        (false, false) => {
            dr_z2_edged_split_core::<false, false>(dst, stride, above, left, origin, w, h, dx, dy)
        }
        (false, true) => {
            dr_z2_edged_split_core::<false, true>(dst, stride, above, left, origin, w, h, dx, dy)
        }
        (true, false) => {
            dr_z2_edged_split_core::<true, false>(dst, stride, above, left, origin, w, h, dx, dy)
        }
        (true, true) => {
            dr_z2_edged_split_core::<true, true>(dst, stride, above, left, origin, w, h, dx, dy)
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
fn dr_z2_edged_split_core<const UA: bool, const UL: bool>(
    dst: &mut [u8],
    stride: usize,
    above: &[u8],
    left: &[u8],
    origin: usize,
    w: usize,
    h: usize,
    dx: i32,
    dy: i32,
) {
    let step = 1usize << (UA as usize);
    for r in 0..h {
        let x = -((r as i32 + 1) * dx);
        let base = x >> (6 - UA as u32);
        let first =
            ((-(step as i32) - base + step as i32 - 1) >> (UA as u32)).clamp(0, w as i32) as usize;
        let row = &mut dst[r * stride..r * stride + w];
        for (c, out) in row[..first].iter_mut().enumerate() {
            let y = ((r as i32) << 6) - (c as i32 + 1) * dy;
            let i = (origin as i32 + (y >> (6 - UL as u32))) as usize;
            let shift = ((y << (UL as u32)) & 63) >> 1;
            *out = ((i32::from(left[i]) * (32 - shift) + i32::from(left[i + 1]) * shift + 16) >> 5)
                as u8;
        }
        if first < w {
            let begin = (origin as i32 + base + (first * step) as i32) as usize;
            let len = w - first;
            let source = &above[begin..begin + (len - 1) * step + 2];
            let shift = ((x << (UA as u32)) & 63) >> 1;
            for (out, pair) in row[first..].iter_mut().zip(source.windows(2).step_by(step)) {
                *out = ((i32::from(pair[0]) * (32 - shift) + i32::from(pair[1]) * shift + 16) >> 5)
                    as u8;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn dr_z2_edged_flat_scalar(
    _token: ScalarToken,
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u8],
    left: &[u8],
    origin: usize,
    dx: i32,
    dy: i32,
) {
    dr_z2_edged_core(
        dst, dst_stride, bw, bh, above, left, origin, false, false, dx, dy,
    );
}

/// NEON arm of [`dr_z2_edged`], `upsample_above == upsample_left == 0`.
///
/// # The structure, and why it is not C's
///
/// Zone 2 reads BOTH edges: cell `(r, c)` interpolates along `above` when
/// `base_x + c >= -1` and along `left` otherwise, where
/// `base_x(r) = (-(r + 1) * dx) >> 6`. C's NEON arm
/// (`ASM_NEON/intra_prediction_neon.c:505`, `dr_prediction_z2_WxH_neon`)
/// computes BOTH halves for every 16-column group and selects with
/// `vbslq_u8` over a `base_mask` table — and it reaches the left half with
/// `vqtbl4q_u8`, a 64-byte table lookup, because within a ROW the left
/// half's `base_y` and `shift` BOTH vary with the column.
///
/// This arm splits the block into the two regions instead and walks each one
/// along the axis on which it is contiguous, so there is no gather at all:
///
/// * `base_x` DECREASES with `r`, so `c0(r) = max(0, -1 - base_x(r))` — the
///   first `above` column of row `r` — is NON-DECREASING. The `above` region
///   is therefore the staircase `{ (r, c) : c >= c0(r) }` and its complement
///   is the `left` region.
/// * **Pass 1** walks the `above` region ROW-major. Along a row `base_x + c`
///   advances by one per column and `shift = ((-(r+1)*dx) & 0x3f) >> 1` is
///   CONSTANT, which is exactly [`dr_z1_edged_flat_neon`]'s kernel.
/// * **Pass 2** walks the `left` region COLUMN-major. `y = (r << 6) - (c+1)*dy`
///   and `r << 6` is an exact multiple of 64, so `base_y = r + by0` with
///   `by0 = (-(c+1)*dy) >> 6` and `shift = ((-(c+1)*dy) & 0x3f) >> 1` is
///   CONSTANT down a column — exactly [`dr_z3_edged_flat_neon`]'s kernel,
///   scatter and all. Because `c0` is non-decreasing, the first `left` row of
///   column `c` is non-decreasing too, so one two-pointer walk finds every
///   `r0(c)`.
///
/// The two regions are disjoint and cover the block, so every output byte is
/// written exactly once and neither pass needs a select.
///
/// The interpolation itself is [`dr_z1_edged_flat_neon`]'s, verbatim, with the
/// same exactness argument: `(a0 << 5) + (a1 - a0) * shift` is the same
/// integer as `a0 * (32 - shift) + a1 * shift` (mod 2^16, and the true value
/// lies in `[0, 255 * 32]`), and `vrshrn_n_u16::<5>` IS `(v + 16) >> 5`. The
/// scalar core's `.clamp(0, 255)` can never fire for the same reason.
///
/// # Why this is a hand-written per-ISA arm and not `#[magetypes]`
///
/// Same three missing primitives [`dr_z1_edged_flat_neon`] records, re-checked
/// against **the pinned version** rather than against `archmage`'s `main`:
/// `Cargo.lock` holds `magetypes 0.9.28`, and 0.9.28 is the release
/// IMMEDIATELY BEFORE archmage's `fd66480` (PR #71, uniform variable shifts +
/// saturating add/sub) and `fd3c609` (PR #74, `widen_low` / `widen_high` /
/// `narrow_saturating`). Verified by grep of the published crate source
/// (`cargo read magetypes` -> 0.9.28): `shl_uniform`, `saturating_add`,
/// `widen_low` and `narrow_saturating` are all ABSENT, and
/// `src/simd/backends/convert_int.rs` still carries same-width bitcasts only.
/// So the primitive that decides it is **`u8x16 -> u16x8` widening**, and the
/// pinned crate does not have it at any version this repo can resolve.
///
/// **CORRECTED 2026-09-04 — UPSTREAM HAS IT NOW, AND THIS KERNEL HAS BEEN
/// DEMONSTRATED AS ONE GENERIC BODY.** `imazen/archmage` `origin/main` is
/// `3cd0a04` and carries the widening/narrowing work; against a dev-only git
/// patch this kernel compiles as a single
/// `#[magetypes(define(u8x16, u16x8, i16x8), v4, v3, neon, wasm128, scalar)]`
/// body and is byte-exact against real C on every tier. What is NOT measured
/// is its SPEED. See `docs/perf-status.md`'s magetypes block for the exact
/// expression, the patch stanza and the A/B that has to run before it lands.
///
/// Two further gaps would survive even a bump to archmage `main`:
/// * `narrow_saturating` is a SATURATING narrow, not a ROUNDING one; this
///   kernel needs `vrshrn_n_u16::<5>`, i.e. `(v + 16) >> 5`, which would have
///   to be spelled as an explicit add-then-`shr_logical_const` before it;
/// * the shift here is uniform per row / per column, so PR #71's
///   `shl_uniform` is not what this kernel was missing in the first place.
#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn dr_z2_edged_flat_neon(
    _token: NeonToken,
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u8],
    left: &[u8],
    origin: usize,
    dx: i32,
    dy: i32,
) {
    // `c0[r]`: the first column of row `r` that interpolates along `above`.
    // Clamped to `bw`, which means "this row is entirely `left`".
    let mut c0 = [0usize; 64];
    debug_assert!(bh <= 64);
    for (r, slot) in c0[..bh].iter_mut().enumerate() {
        let x = -((r as i32 + 1) * dx);
        *slot = ((-1 - (x >> 6)).max(0) as usize).min(bw);
    }

    // ---- pass 1: the `above` region, row-major. ----
    for (r, &c_start) in c0[..bh].iter().enumerate() {
        if c_start >= bw {
            continue;
        }
        let x = -((r as i32 + 1) * dx);
        let bi = origin as i32 + (x >> 6); // may be < 0; `bi + c >= origin - 1`
        let shift = ((x & 0x3f) >> 1) as u16;
        let shift_v = vdupq_n_u16(shift);
        let drow = &mut dst[r * dst_stride..r * dst_stride + bw];
        let mut c = c_start;
        if bw - c_start >= 16 {
            while c + 16 <= bw {
                let i0 = (bi + c as i32) as usize;
                let a0: &[u8; 16] = above[i0..i0 + 16].try_into().unwrap();
                let a1: &[u8; 16] = above[i0 + 1..i0 + 17].try_into().unwrap();
                let a0v = vld1q_u8(a0);
                let a1v = vld1q_u8(a1);
                let lo = vmlaq_u16(
                    vshll_n_u8::<5>(vget_low_u8(a0v)),
                    vsubl_u8(vget_low_u8(a1v), vget_low_u8(a0v)),
                    shift_v,
                );
                let hi = vmlaq_u16(
                    vshll_n_u8::<5>(vget_high_u8(a0v)),
                    vsubl_u8(vget_high_u8(a1v), vget_high_u8(a0v)),
                    shift_v,
                );
                let out = vcombine_u8(vrshrn_n_u16::<5>(lo), vrshrn_n_u16::<5>(hi));
                let o: &mut [u8; 16] = (&mut drow[c..c + 16]).try_into().unwrap();
                vst1q_u8(o, out);
                c += 16;
            }
            if c < bw {
                // Overlapping final chunk: `bw - 16 >= c_start` holds because
                // the segment is at least 16 wide, so it rewrites only
                // `above`-region columns.
                let c2 = bw - 16;
                let i0 = (bi + c2 as i32) as usize;
                let a0: &[u8; 16] = above[i0..i0 + 16].try_into().unwrap();
                let a1: &[u8; 16] = above[i0 + 1..i0 + 17].try_into().unwrap();
                let a0v = vld1q_u8(a0);
                let a1v = vld1q_u8(a1);
                let lo = vmlaq_u16(
                    vshll_n_u8::<5>(vget_low_u8(a0v)),
                    vsubl_u8(vget_low_u8(a1v), vget_low_u8(a0v)),
                    shift_v,
                );
                let hi = vmlaq_u16(
                    vshll_n_u8::<5>(vget_high_u8(a0v)),
                    vsubl_u8(vget_high_u8(a1v), vget_high_u8(a0v)),
                    shift_v,
                );
                let out = vcombine_u8(vrshrn_n_u16::<5>(lo), vrshrn_n_u16::<5>(hi));
                let o: &mut [u8; 16] = (&mut drow[c2..c2 + 16]).try_into().unwrap();
                vst1q_u8(o, out);
                c = bw;
            }
        }
        let sh = shift as i32;
        while c < bw {
            let i0 = (bi + c as i32) as usize;
            let v = (above[i0] as i32 * (32 - sh) + above[i0 + 1] as i32 * sh + 16) >> 5;
            drow[c] = v as u8;
            c += 1;
        }
    }

    // ---- pass 2: the `left` region, column-major. ----
    // `r0(c) = min { r : c0[r] > c }`, non-decreasing in `c`: one walk.
    let mut r0 = 0usize;
    let mut tmp = [0u8; 16];
    for c in 0..bw {
        while r0 < bh && c0[r0] <= c {
            r0 += 1;
        }
        if r0 >= bh {
            break; // and no later column has any `left` row either
        }
        let yv = -((c as i32 + 1) * dy);
        let li = origin as i32 + (yv >> 6); // may be < 0; `li + r >= origin - 1`
        let shift = ((yv & 0x3f) >> 1) as u16;
        let shift_v = vdupq_n_u16(shift);
        let mut r = r0;
        if bh - r0 >= 16 {
            loop {
                let rr = if r + 16 <= bh {
                    r
                } else if r < bh {
                    bh - 16 // overlapping final chunk, all inside the region
                } else {
                    break;
                };
                let i0 = (li + rr as i32) as usize;
                let a0: &[u8; 16] = left[i0..i0 + 16].try_into().unwrap();
                let a1: &[u8; 16] = left[i0 + 1..i0 + 17].try_into().unwrap();
                let a0v = vld1q_u8(a0);
                let a1v = vld1q_u8(a1);
                let lo = vmlaq_u16(
                    vshll_n_u8::<5>(vget_low_u8(a0v)),
                    vsubl_u8(vget_low_u8(a1v), vget_low_u8(a0v)),
                    shift_v,
                );
                let hi = vmlaq_u16(
                    vshll_n_u8::<5>(vget_high_u8(a0v)),
                    vsubl_u8(vget_high_u8(a1v), vget_high_u8(a0v)),
                    shift_v,
                );
                vst1q_u8(
                    &mut tmp,
                    vcombine_u8(vrshrn_n_u16::<5>(lo), vrshrn_n_u16::<5>(hi)),
                );
                for (k, &v) in tmp.iter().enumerate() {
                    dst[(rr + k) * dst_stride + c] = v;
                }
                r = rr + 16;
            }
        }
        let sh = shift as i32;
        while r < bh {
            let i0 = (li + r as i32) as usize;
            let v = (left[i0] as i32 * (32 - sh) + left[i0 + 1] as i32 * sh + 16) >> 5;
            dst[r * dst_stride + c] = v as u8;
            r += 1;
        }
    }
}

/// Scalar C `svt_av1_dr_prediction_z2_c` (intra_prediction.c:386) with
/// upsampling. Reached through [`dr_z2_edged`], which routes the flat
/// (non-upsampled) case to a NEON arm.
#[allow(clippy::too_many_arguments)]
fn dr_z2_edged_core(
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u8],
    left: &[u8],
    origin: usize,
    upsample_above: bool,
    upsample_left: bool,
    dx: i32,
    dy: i32,
) {
    debug_assert!(dx > 0 && dy > 0);
    let up_a = upsample_above as i32;
    let up_l = upsample_left as i32;
    let min_base_x = -(1i32 << up_a);
    let frac_bits_x = 6 - up_a;
    let frac_bits_y = 6 - up_l;
    let base_inc_x = 1i32 << up_a;
    let mut x = -dx;
    for r in 0..bh {
        let mut base1 = x >> frac_bits_x;
        let mut y = ((r as i32) << 6) - dy;
        for c in 0..bw {
            let val = if base1 >= min_base_x {
                let shift1 = ((x * (1 << up_a)) & 0x3F) >> 1;
                let i0 = (origin as i32 + base1) as usize;
                above[i0] as i32 * (32 - shift1) + above[i0 + 1] as i32 * shift1
            } else {
                let base2 = y >> frac_bits_y;
                debug_assert!(base2 >= -(1 << up_l));
                let shift2 = ((y * (1 << up_l)) & 0x3F) >> 1;
                let i0 = (origin as i32 + base2) as usize;
                left[i0] as i32 * (32 - shift2) + left[i0 + 1] as i32 * shift2
            };
            dst[r * dst_stride + c] = (((val + 16) >> 5).clamp(0, 255)) as u8;
            base1 += base_inc_x;
            y -= dy;
        }
        x -= dx;
    }
}

/// C `svt_av1_dr_prediction_z3_c` (intra_prediction.c:321) with upsampling.
///
/// Dispatches to a NEON arm on the NON-upsampled path, which is the one large
/// blocks take. The output is COLUMN-major here (`dst[r * stride + c]` for a
/// fixed `c`), so the arm vectorises the arithmetic 16 rows at a time into a
/// stack staging buffer and scatters it out; C's NEON arm reaches the same
/// place by building a tile and transposing it. The scatter is the same number
/// of byte stores the scalar core does — what is saved is the interpolation.
#[allow(clippy::too_many_arguments)]
fn dr_z3_edged(
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    left: &[u8],
    origin: usize,
    upsample_left: bool,
    dy: i32,
) {
    // The vector arm reads `left[origin + base + r + 1 .. + 16]` with
    // `base + r < max_base_y = bw + bh - 1`, so the highest index it can touch
    // is `origin + max_base_y + 1`.
    // `bh >= 16` for the same reason [`dr_z1_edged`] gates on `bw >= 16`:
    // this arm's 16-lane chunk walks ROWS, so below 16 rows it cannot run
    // one and the dispatch is pure overhead.
    if bh >= 16 && !upsample_left && left.len() > origin + bw + bh {
        incant!(
            dr_z3_edged_flat(dst, dst_stride, bw, bh, left, origin, dy),
            [neon, scalar]
        );
        return;
    }
    dr_z3_edged_core(dst, dst_stride, bw, bh, left, origin, upsample_left, dy);
}

fn dr_z3_edged_flat_scalar(
    _token: ScalarToken,
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    left: &[u8],
    origin: usize,
    dy: i32,
) {
    dr_z3_edged_core(dst, dst_stride, bw, bh, left, origin, false, dy);
}

/// NEON arm of [`dr_z3_edged`], `upsample_left == 0`.
///
/// Exactness is [`dr_z1_edged_flat_neon`]'s, verbatim: the same
/// `(a0 << 5) + (a1 - a0) * shift` rewrite of the same two-tap interpolation,
/// with the same mod-2^16 argument and the same `vrshrn_n_u16::<5>` rounding
/// narrow. Only the traversal differs — `base` walks with the ROW here, so a
/// 16-lane load covers 16 consecutive rows of one column.
#[cfg(target_arch = "aarch64")]
#[arcane]
fn dr_z3_edged_flat_neon(
    _token: NeonToken,
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    left: &[u8],
    origin: usize,
    dy: i32,
) {
    let max_base_y = (bw + bh) as i32 - 1;
    let fill = left[origin + max_base_y as usize];
    let mut y = dy;
    for c in 0..bw {
        let base = y >> 6;
        let shift = ((y & 0x3f) >> 1) as u16;
        let shift_v = vdupq_n_u16(shift);
        let bi = origin + base as usize;
        // Rows `r` with `base + r < max_base_y` interpolate; the rest take
        // `left[max_base_y]`, exactly as the scalar core does (its inner
        // `while` fills the remainder of the column and breaks).
        let valid = if base >= max_base_y {
            0
        } else {
            core::cmp::min(bh, (max_base_y - base) as usize)
        };
        let mut r = 0usize;
        let mut tmp = [0u8; 16];
        while r + 16 <= valid {
            let a0: &[u8; 16] = left[bi + r..bi + r + 16].try_into().unwrap();
            let a1: &[u8; 16] = left[bi + r + 1..bi + r + 17].try_into().unwrap();
            let a0v = vld1q_u8(a0);
            let a1v = vld1q_u8(a1);
            let lo = vmlaq_u16(
                vshll_n_u8::<5>(vget_low_u8(a0v)),
                vsubl_u8(vget_low_u8(a1v), vget_low_u8(a0v)),
                shift_v,
            );
            let hi = vmlaq_u16(
                vshll_n_u8::<5>(vget_high_u8(a0v)),
                vsubl_u8(vget_high_u8(a1v), vget_high_u8(a0v)),
                shift_v,
            );
            vst1q_u8(
                &mut tmp,
                vcombine_u8(vrshrn_n_u16::<5>(lo), vrshrn_n_u16::<5>(hi)),
            );
            for (k, &v) in tmp.iter().enumerate() {
                dst[(r + k) * dst_stride + c] = v;
            }
            r += 16;
        }
        let sh = shift as i32;
        while r < valid {
            let v = (left[bi + r] as i32 * (32 - sh) + left[bi + r + 1] as i32 * sh + 16) >> 5;
            dst[r * dst_stride + c] = v as u8;
            r += 1;
        }
        while r < bh {
            dst[r * dst_stride + c] = fill;
            r += 1;
        }
        y += dy;
    }
}

/// Scalar C `svt_av1_dr_prediction_z3_c` (intra_prediction.c:321) with
/// upsampling. Reached through [`dr_z3_edged`], which routes the flat
/// (non-upsampled) case to a NEON arm.
#[allow(clippy::too_many_arguments)]
fn dr_z3_edged_core(
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    left: &[u8],
    origin: usize,
    upsample_left: bool,
    dy: i32,
) {
    let up = upsample_left as i32;
    let max_base_y = ((bw + bh - 1) as i32) << up;
    let frac_bits = 6 - up;
    let base_inc = 1i32 << up;
    let mut y = dy;
    for c in 0..bw {
        let mut base = y >> frac_bits;
        let shift = ((y << up) & 0x3F) >> 1;
        let mut r = 0usize;
        while r < bh {
            if base < max_base_y {
                let val = left[origin + base as usize] as i32 * (32 - shift)
                    + left[origin + base as usize + 1] as i32 * shift;
                dst[r * dst_stride + c] = ((val + 16) >> 5).clamp(0, 255) as u8;
            } else {
                let fill = left[origin + max_base_y as usize];
                while r < bh {
                    dst[r * dst_stride + c] = fill;
                    r += 1;
                }
                break;
            }
            r += 1;
            base += base_inc;
        }
        y += dy;
    }
}

/// C `svt_aom_dr_predictor` over edged buffers (`above[origin + i]` = C
/// `above_row[i]`, `left[origin + i]` = C `left_col[i]`;
/// `above[origin - 1] == left[origin - 1]` is the top-left sample). The
/// upsample flags select the C kernels' upsampled indexing exactly.
#[allow(clippy::too_many_arguments)]
pub fn dr_predictor_edged(
    dst: &mut [u8],
    dst_stride: usize,
    above: &[u8],
    left: &[u8],
    origin: usize,
    upsample_above: bool,
    upsample_left: bool,
    width: usize,
    height: usize,
    angle: i32,
) {
    let dx = get_dx(angle);
    let dy = get_dy(angle);
    if angle > 0 && angle < 90 {
        dr_z1_edged(
            dst,
            dst_stride,
            width,
            height,
            above,
            origin,
            upsample_above,
            dx,
        );
    } else if angle > 90 && angle < 180 {
        dr_z2_edged(
            dst,
            dst_stride,
            width,
            height,
            above,
            left,
            origin,
            upsample_above,
            upsample_left,
            dx,
            dy,
        );
    } else if angle > 180 && angle < 270 {
        dr_z3_edged(
            dst,
            dst_stride,
            width,
            height,
            left,
            origin,
            upsample_left,
            dy,
        );
    } else if angle == 90 {
        predict_v(dst, dst_stride, &above[origin..], width, height);
    } else if angle == 180 {
        predict_h(dst, dst_stride, &left[origin..], width, height);
    }
}

/// Get smooth weight table for a given block dimension.
///
/// Weights decrease from 255 (top/left edge) to approximately 0 (bottom/right edge).
/// These are the Q8 weights from the AV1 spec.
fn smooth_weights(n: usize) -> &'static [u8] {
    match n {
        4 => &SM_WEIGHTS_4,
        8 => &SM_WEIGHTS_8,
        16 => &SM_WEIGHTS_16,
        32 => &SM_WEIGHTS_32,
        64 => &SM_WEIGHTS_64,
        _ => &SM_WEIGHTS_4, // fallback
    }
}

// Smooth weight tables from the AV1 spec (Q8, 256 = 1.0)
static SM_WEIGHTS_4: [u8; 4] = [255, 149, 85, 64];
static SM_WEIGHTS_8: [u8; 8] = [255, 197, 146, 105, 73, 50, 37, 32];
static SM_WEIGHTS_16: [u8; 16] = [
    255, 225, 196, 170, 145, 123, 102, 84, 68, 54, 43, 33, 26, 20, 17, 16,
];
static SM_WEIGHTS_32: [u8; 32] = [
    255, 240, 225, 210, 196, 182, 169, 157, 145, 133, 122, 111, 101, 92, 83, 74, 66, 59, 52, 45,
    39, 34, 29, 25, 21, 17, 14, 12, 10, 9, 8, 8,
];
static SM_WEIGHTS_64: [u8; 64] = [
    255, 248, 240, 233, 225, 218, 210, 203, 196, 189, 182, 176, 169, 163, 156, 150, 144, 138, 133,
    127, 121, 116, 111, 106, 101, 96, 91, 86, 82, 77, 73, 69, 65, 61, 57, 54, 50, 47, 44, 41, 38,
    35, 32, 29, 27, 25, 22, 20, 18, 16, 15, 13, 12, 10, 9, 8, 7, 6, 6, 5, 5, 4, 4, 4,
];

// =============================================================================
// Filter-intra prediction
// Ported from svt_av1_filter_intra_predictor_c in filterintra_c.c
// =============================================================================

/// Scale bits for filter-intra tap application.
const FILTER_INTRA_SCALE_BITS: i32 = 4;

/// Filter-intra tap coefficients: 5 modes, 8 sub-block positions, 7 neighbor taps (+1 padding).
///
/// Indexed as `FILTER_INTRA_TAPS[mode][k][tap]` where:
/// - `mode` 0..5: the 5 filter-intra modes
/// - `k` 0..8: the 8 pixels in a 4x2 sub-block (row = k>>2, col = k&3)
/// - `tap` 0..7: coefficients for p0..p6 (tap 7 is always 0, present for alignment)
#[rustfmt::skip]
static FILTER_INTRA_TAPS: [[[i8; 8]; 8]; 5] = [
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
fn round_power_of_two_signed(value: i32, n: i32) -> i32 {
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

    // 33x33 buffer: row 0 = above (with top-left at [0][0]),
    // column 0 = left (starting at [1][0]).
    let mut buffer = [[0u8; 33]; 33];

    // Initialize top row: buffer[0][0..=bw] = above[0..=bw]
    // above[0] = top_left, above[1..] = above pixels
    buffer[0][..width + 1].copy_from_slice(&above[..width + 1]);

    // Initialize left column: buffer[r+1][0] = left[r]
    for r in 0..height {
        buffer[r + 1][0] = left[r];
    }

    let taps = &FILTER_INTRA_TAPS[mode as usize];

    // Process 4-wide by 2-tall sub-blocks
    for r in (1..height + 1).step_by(2) {
        for c in (1..width + 1).step_by(4) {
            let p0 = buffer[r - 1][c - 1] as i32;
            let p1 = buffer[r - 1][c] as i32;
            let p2 = buffer[r - 1][c + 1] as i32;
            let p3 = buffer[r - 1][c + 2] as i32;
            let p4 = buffer[r - 1][c + 3] as i32;
            let p5 = buffer[r][c - 1] as i32;
            let p6 = buffer[r + 1][c - 1] as i32;

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
                buffer[r + r_offset][c + c_offset] =
                    round_power_of_two_signed(val, FILTER_INTRA_SCALE_BITS).clamp(0, 255) as u8;
            }
        }
    }

    // Copy result from buffer to dst
    for r in 0..height {
        dst[r * dst_stride..r * dst_stride + width].copy_from_slice(&buffer[r + 1][1..1 + width]);
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    /// The branch-free rounding in [`cfl_predict_lbd_core`] must equal the
    /// branchy form over the WHOLE reachable domain: every `ac_q3` an `i16` can
    /// hold, times every legal `alpha_q3` (C `cfl_idx_to_alpha`: magnitude
    /// 1..=16 either sign, plus 0). 2,162,720 pairs, exhaustive — not sampled.
    #[test]
    fn cfl_branch_free_rounding_matches_the_branchy_form() {
        for alpha_mag in 0..=16i32 {
            for sign in [1i32, -1] {
                let alpha_q3 = alpha_mag * sign;
                for ac in i16::MIN..=i16::MAX {
                    let q6 = alpha_q3 * ac as i32;
                    let want = if q6 < 0 {
                        -((-q6 + 32) >> 6)
                    } else {
                        (q6 + 32) >> 6
                    };
                    let s = q6 >> 31;
                    let got = ((((q6 ^ s) - s) + 32) >> 6) ^ s;
                    assert_eq!(got - s, want, "alpha={alpha_q3} ac={ac} q6={q6}");
                }
            }
        }
    }

    #[allow(dead_code)]
    fn make_test_block(width: usize, height: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>, u8) {
        // above: increasing values
        let above: Vec<u8> = (0..width).map(|i| (100 + i) as u8).collect();
        // left: decreasing values
        let left: Vec<u8> = (0..height).map(|i| (200 - i * 5) as u8).collect();
        let dst = vec![0u8; width * height];
        let top_left = 150u8;
        (dst, above, left, top_left)
    }

    #[test]
    fn dc_uniform_neighbors() {
        let above = [100u8; 4];
        let left = [100u8; 4];
        let mut dst = [0u8; 16];
        predict_dc(&mut dst, 4, &above, &left, 4, 4, true, true);
        assert!(dst.iter().all(|&v| v == 100));
    }

    #[test]
    fn dc_above_only() {
        let above = [200u8; 8];
        let left = [0u8; 8];
        let mut dst = [0u8; 64];
        predict_dc(&mut dst, 8, &above, &left, 8, 8, true, false);
        assert!(dst.iter().all(|&v| v == 200));
    }

    #[test]
    fn dc_no_neighbors() {
        let mut dst = [0u8; 16];
        predict_dc(&mut dst, 4, &[], &[], 4, 4, false, false);
        assert!(dst.iter().all(|&v| v == 128));
    }

    #[test]
    fn v_pred_copies_above() {
        let above = [10u8, 20, 30, 40];
        let mut dst = [0u8; 16];
        predict_v(&mut dst, 4, &above, 4, 4);
        for row in 0..4 {
            assert_eq!(&dst[row * 4..row * 4 + 4], &above);
        }
    }

    #[test]
    fn h_pred_copies_left() {
        let left = [10u8, 20, 30, 40];
        let mut dst = [0u8; 16];
        predict_h(&mut dst, 4, &left, 4, 4);
        for row in 0..4 {
            assert!(dst[row * 4..row * 4 + 4].iter().all(|&v| v == left[row]));
        }
    }

    #[test]
    fn paeth_uniform() {
        // When above, left, and top_left are all the same, paeth should produce that value
        let above = [128u8; 4];
        let left = [128u8; 4];
        let mut dst = [0u8; 16];
        predict_paeth(&mut dst, 4, &above, &left, 128, 4, 4);
        assert!(dst.iter().all(|&v| v == 128));
    }

    #[test]
    fn paeth_horizontal_gradient() {
        // Top-left = 0, above = [10,20,30,40], left = [0,0,0,0]
        // base = above[c] + left[r] - tl = above[c], so pred = above[c]
        let above = [10u8, 20, 30, 40];
        let left = [0u8; 4];
        let mut dst = [0u8; 16];
        predict_paeth(&mut dst, 4, &above, &left, 0, 4, 4);
        for row in 0..4 {
            for col in 0..4 {
                assert_eq!(dst[row * 4 + col], above[col]);
            }
        }
    }

    #[test]
    fn smooth_corners() {
        // Smooth prediction should interpolate between neighbors
        let above = [200u8; 4];
        let left = [200u8; 4];
        let mut dst = [0u8; 16];
        predict_smooth(&mut dst, 4, &above, &left, 4, 4);
        // All neighbors are 200, so prediction should be 200
        for &v in &dst {
            assert!((v as i32 - 200).abs() <= 1, "expected ~200, got {v}");
        }
    }

    #[test]
    fn smooth_v_interpolates() {
        let above = [255u8; 4];
        let left = [255, 255, 255, 0]; // bottom pixel is 0
        let mut dst = [0u8; 16];
        predict_smooth_v(&mut dst, 4, &above, &left, 0, 4, 4);
        // First row should be close to 255 (high weight on above)
        assert!(dst[0] > 200);
        // Last row should be closer to 0 (low weight on above)
        assert!(dst[12] < dst[0]);
    }

    #[test]
    fn smooth_h_interpolates() {
        let above = [255, 255, 255, 0]; // right pixel is 0
        let left = [255u8; 4];
        let mut dst = [0u8; 16];
        predict_smooth_h(&mut dst, 4, &above, &left, 4, 4);
        // First column should be close to 255
        assert!(dst[0] > 200);
        // Last column should be closer to 0
        assert!(dst[3] < dst[0]);
    }

    // =========================================================================
    // Directional kernel exact-value tests vs libaom av1_dr_prediction_z*_c
    // (av1/common/reconintra.c, upsample = 0). Expected values are
    // hand-computed from the C formulas — the same math the AV1 reference
    // decoder runs, which is what the recon-parity gate compares against.
    // =========================================================================

    #[test]
    fn dr_z1_d45_exact_diagonal() {
        // D45: dx = DR_INTRA_DERIVATIVE[45] = 64.
        // x = 64*(r+1) → base = r+1, shift = 0 → pred[r][c] = above[r+c+1]
        // while r+c+1 < max_base_x (= 7 for 4x4), else above[max_base_x].
        let above: Vec<u8> = (0..8).map(|i| (10 + i * 10) as u8).collect();
        let left = [0u8; 8];
        let mut dst = [0u8; 16];
        predict_directional(&mut dst, 4, &above, &left, 99, 4, 4, 45);
        for r in 0..4 {
            for c in 0..4 {
                let idx = (r + c + 1).min(7);
                assert_eq!(dst[r * 4 + c], above[idx], "D45 exact at ({r},{c})");
            }
        }
    }

    #[test]
    fn dr_z2_d135_exact_diagonal() {
        // D135: dx = dy = DR_INTRA_DERIVATIVE[45] = 64. All shifts are 0, so
        // the prediction is the pure -45° diagonal through the top-left:
        //   pred[r][c] = above[c-r-1]  (c > r)
        //              = top_left      (c == r)
        //              = left[r-c-1]   (c < r)
        let above = [10u8, 20, 30, 40];
        let left = [50u8, 60, 70, 80];
        let top_left = 100u8;
        let mut dst = [0u8; 16];
        predict_directional(&mut dst, 4, &above, &left, top_left, 4, 4, 135);
        for r in 0..4 {
            for c in 0..4 {
                let expected = match c.cmp(&r) {
                    core::cmp::Ordering::Greater => above[c - r - 1],
                    core::cmp::Ordering::Equal => top_left,
                    core::cmp::Ordering::Less => left[r - c - 1],
                };
                assert_eq!(dst[r * 4 + c], expected, "D135 exact at ({r},{c})");
            }
        }
    }

    #[test]
    fn dr_z2_d113_exact_4x4() {
        // D113: dx = DR_INTRA_DERIVATIVE[180-113] = deriv[67] = 27,
        //        dy = DR_INTRA_DERIVATIVE[113-90]  = deriv[23] = 151.
        // Full 4x4 hand-computed from av1_dr_prediction_z2_c with
        // above = [10,20,30,40], left = [50,60,70,80], above[-1] = 100.
        // e.g. r0c0: x = -27, base_x = -1, shift = 18 →
        //      (100*14 + 10*18 + 16) >> 5 = 49.
        //      r2c0: x = -81, base_x = -2 → left branch: y = 128 - 151 =
        //      -23, base_y = -1, shift = 20 →
        //      (100*12 + 50*20 + 16) >> 5 = 69.
        let above = [10u8, 20, 30, 40];
        let left = [50u8, 60, 70, 80];
        let top_left = 100u8;
        let mut dst = [0u8; 16];
        predict_directional(&mut dst, 4, &above, &left, top_left, 4, 4, 113);
        let expected: [u8; 16] = [
            49, 16, 26, 36, //
            86, 12, 22, 32, //
            69, 35, 17, 27, //
            56, 72, 13, 23,
        ];
        assert_eq!(dst, expected, "D113 exact 4x4");
    }

    #[test]
    fn dr_z2_d157_exact_spots() {
        // D157: dx = deriv[180-157] = deriv[23] = 151,
        //        dy = deriv[157-90]  = deriv[67] = 27.
        let above = [10u8, 20, 30, 40];
        let left = [50u8, 60, 70, 80];
        let top_left = 100u8;
        let mut dst = [0u8; 16];
        predict_directional(&mut dst, 4, &above, &left, top_left, 4, 4, 157);
        // r0c0: x = -151, base_x = -3 → left branch: y = -27, base_y = -1,
        //       shift = 18 → (100*14 + 50*18 + 16) >> 5 = 72
        assert_eq!(dst[0], 72, "D157 r0c0");
        // r0c1: x = -87, base_x = -2 → left: y = -54, base_y = -1,
        //       shift = 5 → (100*27 + 50*5 + 16) >> 5 = 92
        assert_eq!(dst[1], 92, "D157 r0c1");
        // r0c2: x = -23, base_x = -1, shift = 20 →
        //       (100*12 + 10*20 + 16) >> 5 = 44
        assert_eq!(dst[2], 44, "D157 r0c2");
        // r0c3: x = 41, base_x = 0, shift = 20 →
        //       (10*12 + 20*20 + 16) >> 5 = 16
        assert_eq!(dst[3], 16, "D157 r0c3");
        // r1c0: x = -302, base_x = -5 → left: y = 64 - 27 = 37,
        //       base_y = 0, shift = 18 → (50*14 + 60*18 + 16) >> 5 = 56
        assert_eq!(dst[4], 56, "D157 r1c0");
    }

    #[test]
    fn dr_z3_d203_exact_spots() {
        // D203: dy = DR_INTRA_DERIVATIVE[270-203] = deriv[67] = 27.
        let above = [0u8; 8];
        let left: Vec<u8> = (0..8).map(|i| (50 + i * 10) as u8).collect();
        let mut dst = [0u8; 16];
        predict_directional(&mut dst, 4, &above, &left, 99, 4, 4, 203);
        // c0: y = 27, base = 0, shift = 13:
        //   r0: (50*19 + 60*13 + 16) >> 5 = 54
        //   r1: (60*19 + 70*13 + 16) >> 5 = 64
        assert_eq!(dst[0], 54, "D203 r0c0");
        assert_eq!(dst[4], 64, "D203 r1c0");
        // c1: y = 54, base = 0, shift = 27:
        //   r0: (50*5 + 60*27 + 16) >> 5 = 58
        assert_eq!(dst[1], 58, "D203 r0c1");
    }

    #[test]
    fn filter_intra_mode0_4x4() {
        // above[0] = top_left, above[1..5] = above pixels
        let above = [100u8, 110, 120, 130, 140];
        let left = [90u8, 80, 70, 60];
        let mut dst = [0u8; 16];
        predict_filter_intra(&mut dst, 4, &above, &left, 4, 4, 0);
        // Output should be non-zero
        for &v in &dst {
            assert!(v > 0, "filter-intra produced zero pixel");
        }
        // At least some variation in the output
        let min = dst.iter().copied().min().unwrap();
        let max = dst.iter().copied().max().unwrap();
        assert!(
            max > min,
            "filter-intra produced flat output, expected variation"
        );
    }

    #[test]
    fn filter_intra_zero_neighbors() {
        // All neighbors at 128 should produce output close to 128
        let above = [128u8; 5]; // top_left + 4 above
        let left = [128u8; 4];
        let mut dst = [0u8; 16];
        predict_filter_intra(&mut dst, 4, &above, &left, 4, 4, 0);
        for (i, &v) in dst.iter().enumerate() {
            assert!(
                (v as i32 - 128).abs() <= 1,
                "pixel {i}: expected ~128, got {v}"
            );
        }
    }

    #[test]
    fn filter_intra_all_modes_4x4() {
        // Verify all 5 modes produce valid, distinct output
        let above = [100u8, 110, 120, 130, 140];
        let left = [90u8, 80, 70, 60];
        let mut outputs = [[0u8; 16]; 5];
        for mode in 0..5u8 {
            predict_filter_intra(&mut outputs[mode as usize], 4, &above, &left, 4, 4, mode);
        }
        // At least some modes should differ
        let mut any_differ = false;
        for i in 1..5 {
            if outputs[i] != outputs[0] {
                any_differ = true;
                break;
            }
        }
        assert!(
            any_differ,
            "all 5 filter-intra modes produced identical output"
        );
    }

    #[test]
    fn filter_intra_8x8() {
        // Larger block size
        let above = [128u8; 9]; // top_left + 8 above
        let left = [128u8; 8];
        let mut dst = [0u8; 64];
        predict_filter_intra(&mut dst, 8, &above, &left, 8, 8, 2);
        for &v in &dst {
            assert!(
                (v as i32 - 128).abs() <= 1,
                "expected ~128 for uniform input, got {v}"
            );
        }
    }

    #[test]
    fn palette_basic() {
        let palette = [10u8, 50, 100, 200];
        // Color map: 4x4 block with indices into palette
        let color_map = [0u8, 1, 2, 3, 3, 2, 1, 0, 0, 0, 3, 3, 1, 1, 2, 2];
        let mut dst = [0u8; 16];
        predict_palette(&mut dst, 4, &color_map, 4, &palette, 4, 4);

        // Verify each pixel matches palette lookup
        let expected = [
            10u8, 50, 100, 200, 200, 100, 50, 10, 10, 10, 200, 200, 50, 50, 100, 100,
        ];
        assert_eq!(dst, expected);
    }

    #[test]
    fn palette_single_color() {
        let palette = [42u8];
        let color_map = [0u8; 16];
        let mut dst = [0u8; 16];
        predict_palette(&mut dst, 4, &color_map, 4, &palette, 4, 4);
        assert!(dst.iter().all(|&v| v == 42));
    }

    #[test]
    fn palette_with_stride() {
        let palette = [10u8, 20, 30];
        // Map stride larger than width (2x2 block in a 4-wide map)
        let color_map = [
            0u8, 1, 255, 255, // row 0 (only first 2 used)
            2, 0, 255, 255, // row 1
        ];
        let mut dst = [0u8; 8]; // 2x2 block with stride 4
        predict_palette(&mut dst, 4, &color_map, 4, &palette, 2, 2);
        assert_eq!(dst[0], 10); // palette[0]
        assert_eq!(dst[1], 20); // palette[1]
        assert_eq!(dst[4], 30); // palette[2]
        assert_eq!(dst[5], 10); // palette[0]
    }
}

#[cfg(test)]
mod dispatch_tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;
    use archmage::testing::{CompileTimePolicy, for_each_token_permutation};

    #[test]
    fn paeth_all_dispatch_levels() {
        let above: Vec<u8> = (0..8).map(|i| (50 + i * 20) as u8).collect();
        let left: Vec<u8> = (0..8).map(|i| (200 - i * 15) as u8).collect();
        let top_left = 100u8;
        let mut ref_dst = vec![0u8; 64];
        predict_paeth(&mut ref_dst, 8, &above, &left, top_left, 8, 8);

        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            let mut dst = vec![0u8; 64];
            predict_paeth(&mut dst, 8, &above, &left, top_left, 8, 8);
            assert_eq!(dst, ref_dst, "paeth mismatch at dispatch level");
        });
    }

    #[test]
    fn paeth_dispatch_4x4() {
        let above = [10u8, 20, 30, 40];
        let left = [50u8, 60, 70, 80];
        let top_left = 5u8;
        let mut ref_dst = [0u8; 16];
        predict_paeth(&mut ref_dst, 4, &above, &left, top_left, 4, 4);

        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            let mut dst = [0u8; 16];
            predict_paeth(&mut dst, 4, &above, &left, top_left, 4, 4);
            assert_eq!(dst, ref_dst, "paeth 4x4 mismatch at dispatch level");
        });
    }

    #[test]
    fn paeth_dispatch_16x16() {
        let above: Vec<u8> = (0..16).map(|i| (i * 15) as u8).collect();
        let left: Vec<u8> = (0..16).map(|i| (255 - i * 12) as u8).collect();
        let top_left = 128u8;
        let mut ref_dst = vec![0u8; 256];
        predict_paeth(&mut ref_dst, 16, &above, &left, top_left, 16, 16);

        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            let mut dst = vec![0u8; 256];
            predict_paeth(&mut dst, 16, &above, &left, top_left, 16, 16);
            assert_eq!(dst, ref_dst, "paeth 16x16 mismatch at dispatch level");
        });
    }
}
