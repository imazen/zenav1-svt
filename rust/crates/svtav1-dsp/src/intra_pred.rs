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
        dst[row * dst_stride..row * dst_stride + width].fill(left[row]);
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
    incant!(
        predict_smooth_impl(dst, dst_stride, above, left, width, height),
        [v3, neon, scalar]
    );
}

fn predict_smooth_impl_scalar(
    _token: ScalarToken,
    dst: &mut [u8],
    dst_stride: usize,
    above: &[u8],
    left: &[u8],
    width: usize,
    height: usize,
) {
    predict_smooth_core(dst, dst_stride, above, left, width, height);
}

#[cfg(target_arch = "aarch64")]
#[arcane]
fn predict_smooth_impl_neon(
    token: NeonToken,
    dst: &mut [u8],
    dst_stride: usize,
    above: &[u8],
    left: &[u8],
    width: usize,
    height: usize,
) {
    // Same factored form as the v3 arm:
    //   pred[c] = (wh * top[c] + ww[c] * d + K) >> 9
    //   d = left[row] - right,  K = 256*right + (256 - wh)*below + 256
    // — every product is nonneg and the total is <= 260,866, so i32 lanes
    // are exact, `>> 9` is the scalar floor-div, and `.min(255)` is the
    // u8 saturating narrow.
    if width < 8 || width % 8 != 0 || width > 64 {
        predict_smooth_core(dst, dst_stride, above, left, width, height);
        return;
    }
    let below = left[height - 1] as i32;
    let right = above[width - 1] as i32;
    let sm_h = smooth_weights(height);
    let sm_w = smooth_weights(width);
    // Hoist the column-varying vectors (top, ww) widened to i32 per block.
    let mut tops = [vdupq_n_s32(0); 16];
    let mut wws = [vdupq_n_s32(0); 16];
    for j in 0..width / 4 {
        tops[j] = smooth_ld4_widen_neon(token, &above[j * 4..j * 4 + 4]);
        wws[j] = smooth_ld4_widen_neon(token, &sm_w[j * 4..j * 4 + 4]);
    }
    for row in 0..height {
        let wh = sm_h[row] as i32;
        let dv = vdupq_n_s32(left[row] as i32 - right);
        let kv = vdupq_n_s32(256 * right + (256 - wh) * below + 256);
        let whv = vdupq_n_s32(wh);
        let base = row * dst_stride;
        for j in 0..width / 8 {
            let a0 = vshrq_n_s32::<9>(vmlaq_s32(vmlaq_s32(kv, tops[2 * j], whv), wws[2 * j], dv));
            let a1 = vshrq_n_s32::<9>(vmlaq_s32(
                vmlaq_s32(kv, tops[2 * j + 1], whv),
                wws[2 * j + 1],
                dv,
            ));
            // i32 -> i16 saturate (values <= 511, exact) -> u8 saturate.
            let lo = vcombine_s16(vqmovn_s32(a0), vqmovn_s32(a1));
            let d8: &mut [u8; 8] = (&mut dst[base + j * 8..base + j * 8 + 8])
                .try_into()
                .unwrap();
            vst1_u8(d8, vqmovun_s16(lo));
        }
    }
}

/// Widen 4 u8 values to i32 lanes via a broadcast-load (the dup lands the
/// same 4 bytes in both halves; only the low half is widened).
#[cfg(target_arch = "aarch64")]
#[rite]
fn smooth_ld4_widen_neon(_token: NeonToken, v: &[u8]) -> int32x4_t {
    let b: &[u8; 4] = v.try_into().unwrap();
    let d = vreinterpret_u8_u32(vld1_dup_u32(&u32::from_le_bytes(*b)));
    vreinterpretq_s32_u32(vmovl_u16(vget_low_u16(vmovl_u8(d))))
}

#[cfg(target_arch = "x86_64")]
#[arcane]
fn predict_smooth_impl_v3(
    token: Desktop64,
    dst: &mut [u8],
    dst_stride: usize,
    above: &[u8],
    left: &[u8],
    width: usize,
    height: usize,
) {
    use magetypes::simd::generic::{i32x8, u8x32};
    if width != 8 && (width < 16 || width % 16 != 0 || width > 128) {
        predict_smooth_core(dst, dst_stride, above, left, width, height);
        return;
    }
    // Per-row factored form of C's bilinear:
    //   pred[c] = (wh * top[c] + ww[c] * d + K) >> 9
    //   d = left[row] - right,  K = 256*right + (256 - wh)*below + 256
    // — the same value as C's `(wh*top + ww*lft + (256-ww)*right +
    // (256-wh)*below + 256) / 512`: every product is nonneg and the total is
    // ≤ 260,866, so i32 lanes are exact, `>> 9` is the scalar floor-div, and
    // `.min(255)` is the u8 narrowing's saturation. The column-varying
    // vectors (top, ww) are hoisted per block; only wh/d/K change per row.
    let below = i32::from(left[height - 1]);
    let right = i32::from(above[width - 1]);
    let sm_h = smooth_weights(height);
    let sm_w = smooth_weights(width);
    let mut tops = [i32x8::splat(token, 0); 16];
    let mut wws = [i32x8::splat(token, 0); 16];
    let mut j = 0usize;
    while j * 8 < width {
        let take = (width - j * 8).min(16);
        let mut ta = [0u8; 32];
        let mut wa = [0u8; 32];
        ta[..take].copy_from_slice(&above[j * 8..j * 8 + take]);
        wa[..take].copy_from_slice(&sm_w[j * 8..j * 8 + take]);
        let t16 = u8x32::load(token, &ta).widen_low().bitcast_i16x16();
        let w16 = u8x32::load(token, &wa).widen_low().bitcast_i16x16();
        tops[j] = t16.widen_low();
        tops[j + 1] = t16.widen_high();
        wws[j] = w16.widen_low();
        wws[j + 1] = w16.widen_high();
        j += 2;
    }
    if width == 8 {
        // One i32x8 covers a whole row; two rows pack into a u8x32 whose
        // lanes 8..16 and 24..32 are junk (the second `narrow` operand is a
        // duplicate of the first) — only bytes 0..8 and 16..24 are copied.
        let mut row = 0usize;
        while row + 1 < height {
            let mut halves = [i32x8::splat(token, 0); 4];
            for (i, r) in [row, row + 1].into_iter().enumerate() {
                let wh = i32::from(sm_h[r]);
                let d = i32::from(left[r]) - right;
                let k = 256 * right + (256 - wh) * below + 256;
                let a = (tops[0] * wh + wws[0] * d + k).shr_logical_const::<9>();
                halves[2 * i] = a;
                halves[2 * i + 1] = a;
            }
            let mut tmp = [0u8; 32];
            halves[0]
                .narrow_saturating_i16(halves[1])
                .narrow_saturating_u8(halves[2].narrow_saturating_i16(halves[3]))
                .store(&mut tmp);
            dst[row * dst_stride..row * dst_stride + 8].copy_from_slice(&tmp[..8]);
            dst[(row + 1) * dst_stride..(row + 1) * dst_stride + 8].copy_from_slice(&tmp[16..24]);
            row += 2;
        }
        if row < height {
            let rest = height - row;
            predict_smooth_core(
                &mut dst[row * dst_stride..],
                dst_stride,
                above,
                &left[row..row + rest],
                width,
                rest,
            );
        }
        return;
    }
    if width == 16 {
        // A 16-px row pairs with the next row in one u8x32 narrow; block
        // heights are always even, and the scalar tail covers a defensive
        // odd `height`.
        let mut row = 0usize;
        while row + 1 < height {
            let mut halves = [i32x8::splat(token, 0); 4];
            for (i, r) in [row, row + 1].into_iter().enumerate() {
                let wh = i32::from(sm_h[r]);
                let d = i32::from(left[r]) - right;
                let k = 256 * right + (256 - wh) * below + 256;
                halves[2 * i] = (tops[0] * wh + wws[0] * d + k).shr_logical_const::<9>();
                halves[2 * i + 1] = (tops[1] * wh + wws[1] * d + k).shr_logical_const::<9>();
            }
            let mut tmp = [0u8; 32];
            halves[0]
                .narrow_saturating_i16(halves[1])
                .narrow_saturating_u8(halves[2].narrow_saturating_i16(halves[3]))
                .store(&mut tmp);
            dst[row * dst_stride..row * dst_stride + 16].copy_from_slice(&tmp[..16]);
            dst[(row + 1) * dst_stride..(row + 1) * dst_stride + 16].copy_from_slice(&tmp[16..]);
            row += 2;
        }
        if row < height {
            let rest = height - row;
            predict_smooth_core(
                &mut dst[row * dst_stride..],
                dst_stride,
                above,
                &left[row..row + rest],
                width,
                rest,
            );
        }
        return;
    }
    for row in 0..height {
        let wh = i32::from(sm_h[row]);
        let d = i32::from(left[row]) - right;
        let k = 256 * right + (256 - wh) * below + 256;
        let base = row * dst_stride;
        let mut c = 0usize;
        // 32 output pixels per fold: two 16-lane halves narrowed into u8x32.
        // Each tops[]/wws[] slot covers EIGHT pixels (i16x16 split into two
        // i32x8), so a 32-px fold needs slots 4c..4c+3 — indexing 2c..2c+3
        // here overlapped the previous chunk from c=1 on (64-wide blocks).
        while c * 32 < width {
            let a0 = (tops[4 * c] * wh + wws[4 * c] * d + k).shr_logical_const::<9>();
            let a1 = (tops[4 * c + 1] * wh + wws[4 * c + 1] * d + k).shr_logical_const::<9>();
            let b0 = (tops[4 * c + 2] * wh + wws[4 * c + 2] * d + k).shr_logical_const::<9>();
            let b1 = (tops[4 * c + 3] * wh + wws[4 * c + 3] * d + k).shr_logical_const::<9>();
            a0.narrow_saturating_i16(a1)
                .narrow_saturating_u8(b0.narrow_saturating_i16(b1))
                .store(
                    (&mut dst[base + c * 32..base + c * 32 + 32])
                        .try_into()
                        .unwrap(),
                );
            c += 1;
        }
    }
}

fn predict_smooth_core(
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
    incant!(
        predict_smooth_v_impl(dst, dst_stride, above, left, height, width),
        [neon, scalar]
    );
}

fn predict_smooth_v_impl_scalar(
    _token: ScalarToken,
    dst: &mut [u8],
    dst_stride: usize,
    above: &[u8],
    left: &[u8],
    height: usize,
    width: usize,
) {
    predict_smooth_v_core(dst, dst_stride, above, left, height, width);
}

fn predict_smooth_v_core(
    dst: &mut [u8],
    dst_stride: usize,
    above: &[u8],
    left: &[u8],
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

#[cfg(target_arch = "aarch64")]
#[arcane]
fn predict_smooth_v_impl_neon(
    token: NeonToken,
    dst: &mut [u8],
    dst_stride: usize,
    above: &[u8],
    left: &[u8],
    height: usize,
    width: usize,
) {
    // pred[c] = (w * top[c] + K) >> 8,  K = (256 - w)*below + 128 — same
    // factored form as predict_smooth; i32 lanes exact (<= 131,200).
    if width < 8 || width % 8 != 0 || width > 64 {
        predict_smooth_v_core(dst, dst_stride, above, left, height, width);
        return;
    }
    let below = left[height - 1] as i32;
    let sm_weights = smooth_weights(height);
    let mut tops = [vdupq_n_s32(0); 16];
    for j in 0..width / 4 {
        tops[j] = smooth_ld4_widen_neon(token, &above[j * 4..j * 4 + 4]);
    }
    for row in 0..height {
        let w = sm_weights[row] as i32;
        let wv = vdupq_n_s32(w);
        let kv = vdupq_n_s32((256 - w) * below + 128);
        let base = row * dst_stride;
        for j in 0..width / 8 {
            let a0 = vshrq_n_s32::<8>(vmlaq_s32(kv, tops[2 * j], wv));
            let a1 = vshrq_n_s32::<8>(vmlaq_s32(kv, tops[2 * j + 1], wv));
            let lo = vcombine_s16(vqmovn_s32(a0), vqmovn_s32(a1));
            let d8: &mut [u8; 8] = (&mut dst[base + j * 8..base + j * 8 + 8])
                .try_into()
                .unwrap();
            vst1_u8(d8, vqmovun_s16(lo));
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
    incant!(
        predict_smooth_h_impl(dst, dst_stride, above, left, width, height),
        [neon, scalar]
    );
}

fn predict_smooth_h_impl_scalar(
    _token: ScalarToken,
    dst: &mut [u8],
    dst_stride: usize,
    above: &[u8],
    left: &[u8],
    width: usize,
    height: usize,
) {
    predict_smooth_h_core(dst, dst_stride, above, left, width, height);
}

fn predict_smooth_h_core(
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

#[cfg(target_arch = "aarch64")]
#[arcane]
fn predict_smooth_h_impl_neon(
    token: NeonToken,
    dst: &mut [u8],
    dst_stride: usize,
    above: &[u8],
    left: &[u8],
    width: usize,
    height: usize,
) {
    // pred[c] = (ww[c] * d + K) >> 8,  d = left[row] - right,
    // K = 256*right + 128 — same factored form; i32 lanes exact.
    if width < 8 || width % 8 != 0 || width > 64 {
        predict_smooth_h_core(dst, dst_stride, above, left, width, height);
        return;
    }
    let right = above[width - 1] as i32;
    let sm_w = smooth_weights(width);
    let mut wws = [vdupq_n_s32(0); 16];
    for j in 0..width / 4 {
        wws[j] = smooth_ld4_widen_neon(token, &sm_w[j * 4..j * 4 + 4]);
    }
    let kv = vdupq_n_s32(256 * right + 128);
    for row in 0..height {
        let dv = vdupq_n_s32(left[row] as i32 - right);
        let base = row * dst_stride;
        for j in 0..width / 8 {
            let a0 = vshrq_n_s32::<8>(vmlaq_s32(kv, wws[2 * j], dv));
            let a1 = vshrq_n_s32::<8>(vmlaq_s32(kv, wws[2 * j + 1], dv));
            let lo = vcombine_s16(vqmovn_s32(a0), vqmovn_s32(a1));
            let d8: &mut [u8; 8] = (&mut dst[base + j * 8..base + j * 8 + 8])
                .try_into()
                .unwrap();
            vst1_u8(d8, vqmovun_s16(lo));
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
    //
    // Below 16 the C `4xH`/`8xH` row-vector kernel takes over. Its loads
    // top out at `edge[origin + base + 15]` (upsampled `vld1q`) or
    // `edge[origin + base + 8]` with `base < max_base`, so the guard is
    // `origin + max_base + 16` / `+ 9`. `svt_aom_use_intra_edge_upsample`
    // can only return 1 for `bw + bh <= 16`, so `max_base <= 30` whenever
    // the upsample branch runs in production and the 160-byte edged
    // buffer always satisfies the guard there; oversized or short
    // buffers still fall back to the scalar core.
    if bw == 4 || bw == 8 {
        let up = upsample_above as usize;
        let max_base = ((bw + bh - 1) << up) as usize;
        if above.len() >= origin + max_base + if upsample_above { 16 } else { 9 } {
            incant!(
                dr_z1_edged_small(dst, dst_stride, bw, bh, above, origin, upsample_above, dx),
                [neon, scalar]
            );
            return;
        }
    }
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

/// One `u8x8` output vector of the shared z1/z3 small-block kernel: z1
/// row `r`, or z3 column `c` (the same computation on `left`/`dy` — C's
/// `dr_prediction_z1_WxH_internal_neon_small` is reused for both). Lane
/// `i` interpolates `edge[base + i * (1 << up)]` vs its successor, lanes
/// past the valid count take `edge[max_base]` — the fill the scalar
/// core's `else` arm writes.
///
/// SCALAR-EXACT, and deliberately NOT a transliteration of C's mask
/// count: C computes `base_max_diff = (max_base - base) >> upsample`
/// (floor), while the C scalar core interpolates column `c` whenever
/// `base + c * base_inc < max_base` (ceil). On odd `max_base - base`
/// with `upsample == 1` those differ by one lane — the x86 AVX2 kernel
/// shares NEON's floor form, so C's own ISAs disagree with C's scalar
/// on that lane. The port keeps every tier on the scalar formula:
/// `n_valid = ceil(diff / base_inc)`, so this arm and the scalar core
/// cannot diverge no matter which ISA runs.
///
/// Interpolation is `(a0 << 5) + (a1 - a0) * shift` in `u16x8` with a
/// `vrshrn_n_u16::<5>` narrow — the same mod-2^16-exact rewrite
/// [`dr_z1_edged_flat_neon`] documents.
#[cfg(target_arch = "aarch64")]
#[rite]
fn dr_small_row_neon(
    _token: NeonToken,
    w: usize,
    edge: &[u8],
    origin: usize,
    upsample: bool,
    max_base: i32,
    x: i32,
) -> uint8x8_t {
    let up = upsample as i32;
    let fill_v = vdup_n_u8(edge[origin + max_base as usize]);
    let base = x >> (6 - up);
    if base >= max_base {
        return fill_v;
    }
    let n_valid = (((max_base - base) + (1i32 << up) - 1) >> up).min(w as i32) as u8;
    let shift = (((x << up) & 0x3f) >> 1) as u16;
    let bi = origin + base as usize;
    let (a0, a1) = if upsample {
        // One 16-byte load + uzp deinterleave == C's `vld2_u8` pair
        // (even lanes `edge[base + 2i]`, odd `edge[base + 2i + 1]`).
        let v: &[u8; 16] = edge[bi..bi + 16].try_into().unwrap();
        let q = vld1q_u8(v);
        (vget_low_u8(vuzp1q_u8(q, q)), vget_low_u8(vuzp2q_u8(q, q)))
    } else {
        (
            vld1_u8(edge[bi..bi + 8].try_into().unwrap()),
            vld1_u8(edge[bi + 1..bi + 9].try_into().unwrap()),
        )
    };
    let res = vmlaq_u16(vshll_n_u8::<5>(a0), vsubl_u8(a1, a0), vdupq_n_u16(shift));
    vbsl_u8(
        vclt_u8(vld1_u8(&Z2_IDX8), vdup_n_u8(n_valid)),
        vrshrn_n_u16::<5>(res),
        fill_v,
    )
}

/// Scalar-tier mirror of [`dr_z1_edged_small_neon`] for `incant!`.
fn dr_z1_edged_small_scalar(
    _token: ScalarToken,
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u8],
    origin: usize,
    upsample_above: bool,
    dx: i32,
) {
    dr_z1_edged_core(dst, dst_stride, bw, bh, above, origin, upsample_above, dx);
}

/// C `dr_prediction_z1_4xH_neon` / `dr_prediction_z1_8xH_neon`: one
/// `u8x8` per row via [`dr_small_row_neon`], stored as 4 or 8 bytes.
/// Handles `upsample_above` (the deinterleaved load) — the flat arm
/// cannot — and any `bh <= 64`.
#[cfg(target_arch = "aarch64")]
#[arcane]
fn dr_z1_edged_small_neon(
    token: NeonToken,
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u8],
    origin: usize,
    upsample_above: bool,
    dx: i32,
) {
    let max_base = ((bw + bh) as i32 - 1) << upsample_above as i32;
    let mut x = dx;
    for r in 0..bh {
        let v = dr_small_row_neon(token, bw, above, origin, upsample_above, max_base, x);
        let d = &mut dst[r * dst_stride..r * dst_stride + bw];
        if bw == 8 {
            vst1_u8(d.try_into().unwrap(), v);
        } else {
            let word = vget_lane_u32::<0>(vreinterpret_u32_u8(v));
            d.copy_from_slice(&word.to_le_bytes());
        }
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
    // Small blocks: C's `dr_prediction_z2_4xH_neon` / `_8xH_neon` (any
    // upsample combination). The loads reach `above[base_x .. base_x + 16)`
    // for `base_x` as low as `min_base_x - 1 - 8 * (1 << upsample_above)`
    // (only fully-masked lanes ever underflow, and the kernel clamps the
    // start index to 0 in that case) and `left[origin - 2 .. origin + 46)`,
    // so `origin >= 16` plus the length guards cover every read.
    #[cfg(target_arch = "aarch64")]
    if (bw == 4 || bw == 8)
        && origin >= 16
        && above.len() >= origin + 16
        && left.len() >= origin + 46
    {
        incant!(
            dr_z2_edged_small(
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
            ),
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

#[cfg(target_arch = "aarch64")]
#[allow(clippy::too_many_arguments)]
fn dr_z2_edged_small_scalar(
    _token: ScalarToken,
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

/// NEON arms for `bw` 4 and 8 — faithful transliterations of C
/// `dr_prediction_z2_4xH_neon` / `dr_prediction_z2_8xH_neon`
/// (`ASM_NEON/intra_prediction_neon.c:302` / `:400`), which compute BOTH
/// the above-edge and the left-edge interpolation for a whole row and
/// select per column with `vbsl` over `base_mask`. Unlike the `>= 16`
/// [`dr_z2_edged_flat_neon`] staircase split this handles upsampling too,
/// because the table gather (`vqtbl`) covers it for free.
///
/// Exactness: `vmlaq_u16` is mod-2^16 and the true interpolation value is
/// a convex combination in `[0, 255 * 32]`, so the wrapped difference
/// term cancels — same argument [`dr_z2_edged_flat_neon`] records.
/// `vrshrn_n_u16::<5>` IS `(v + 16) >> 5`. `vshl_s16` by `-frac_bits_y`
/// is the scalar `y >> frac_bits_y` on the s16 lanes; lanes whose wrapped
/// `y` exceeds s16 are always outside the left region and masked away
/// (the same lanes C's own s16 kernel computes).
#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn dr_z2_edged_small_neon(
    token: NeonToken,
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
    // The left-edge gather table covers `left[-2 .. 45]` (48 bytes), so the
    // largest usable `base_y` is 44 (`idx = base_y + 3 <= 47`). The left
    // region is a row SUFFIX (`base_x` decreases in `r`), and a used lane's
    // `base_y = (r << 6 - (c+1) * dy) >> frac_bits_y` peaks at lane 0 of
    // the last row — one scalar check bounds every lane the `vbsl` can
    // keep. Beyond it the scalar core runs (same as today). C's own 4xH
    // table WRAPS past index 17 (`vextq_u8(left_0, left_0, 14)`), which is
    // only sound while `base_y <= 14`; the port cannot copy that because
    // its oracle is the x86 kernel's scalar formula, not aarch64-C.
    let up_l = upsample_left as i32;
    let frac_bits_y = 6 - up_l;
    let min_base_x = -(1i32 << upsample_above as i32);
    let frac_bits_x = 6 - upsample_above as i32;
    let last_row_has_left = (-(bh as i32) * dx >> frac_bits_x) < min_base_x;
    let y_hi = (((bh - 1) as i32) << 6) - dy;
    if last_row_has_left && (y_hi >> frac_bits_y) > 44 {
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
        return;
    }
    match bw {
        4 => dr_z2_4xh_neon(
            token,
            dst,
            dst_stride,
            bh,
            above,
            left,
            origin,
            upsample_above,
            upsample_left,
            dx,
            dy,
        ),
        8 => dr_z2_8xh_neon(
            token,
            dst,
            dst_stride,
            bh,
            above,
            left,
            origin,
            upsample_above,
            upsample_left,
            dx,
            dy,
        ),
        _ => dr_z2_edged_core(
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
        ),
    }
}

/// Load `above` at `origin + base_x`, clamped to index 0 on underflow.
/// With `origin >= 16` an underflow implies `base_x <= -17`, at which
/// every lane of the row is left-region and the vector is discarded by
/// the `base_mask` blend, so any in-bounds bytes are a correct load.
#[cfg(target_arch = "aarch64")]
#[rite]
fn z2_above_neon(_token: NeonToken, origin: usize, base_x: i32) -> usize {
    (origin as i32 + base_x).max(0) as usize
}

/// Column index vector `[0 .. 8)` for `vclt` mask construction.
#[cfg(target_arch = "aarch64")]
const Z2_IDX8: [u8; 8] = [0, 1, 2, 3, 4, 5, 6, 7];

/// The shared `vqtbl3` gather table: three contiguous 16-byte loads
/// covering `left[-2 .. 45]`. The edged buffer is 160 bytes with
/// `origin >= 16`, so all three reads are in bounds.
#[cfg(target_arch = "aarch64")]
#[rite]
fn z2_left_table_neon(_token: NeonToken, left: &[u8], origin: usize) -> uint8x16x3_t {
    let m2: &[u8; 16] = left[origin - 2..origin + 14].try_into().unwrap();
    let l14: &[u8; 16] = left[origin + 14..origin + 30].try_into().unwrap();
    let l30: &[u8; 16] = left[origin + 30..origin + 46].try_into().unwrap();
    uint8x16x3_t(vld1q_u8(m2), vld1q_u8(l14), vld1q_u8(l30))
}

/// C `dr_prediction_z2_4xH_neon`: one `uint16x8` per row, low four lanes
/// the above-edge interpolation, high four the left-edge gather, then one
/// `vmlaq` + `vrshrn` + `vbsl` + a 4-byte lane store.
#[cfg(target_arch = "aarch64")]
#[rite]
#[allow(clippy::too_many_arguments)]
fn dr_z2_4xh_neon(
    token: NeonToken,
    dst: &mut [u8],
    dst_stride: usize,
    bh: usize,
    above: &[u8],
    left: &[u8],
    origin: usize,
    upsample_above: bool,
    upsample_left: bool,
    dx: i32,
    dy: i32,
) {
    let up_a = upsample_above as i32;
    let up_l = upsample_left as i32;
    let min_base_x = -(1i32 << up_a);
    let frac_bits_x = 6 - up_a;
    let frac_bits_y = 6 - up_l;

    let idx8 = vld1_u8(&Z2_IDX8);
    let c1f = vdup_n_u16(0x1f);
    let v_1234 = vcreate_s16(0x0004_0003_0002_0001);
    let dy64 = vdup_n_s16(dy as i16);
    let neg_fby = vdup_n_s16(-(frac_bits_y as i16));

    // 48-byte CONTIGUOUS gather table covering `left[-2 .. 45]` — unlike
    // C's wrapped 32-byte table this keeps the scalar formula's values for
    // `base_y` up to 44 (the caller guards taller reaches).
    let left_vals = z2_left_table_neon(token, left, origin);

    for r in 0..bh {
        let y = r as i32 + 1;
        let x = -y * dx;
        let base_x = x >> frac_bits_x;
        let mut base_shift = 0;
        if base_x < min_base_x - 1 {
            base_shift = (min_base_x - base_x - 1) >> up_a;
        }
        let base_min_diff = ((min_base_x - base_x + up_a) >> up_a).clamp(0, 4);

        let mut a0_x = vdupq_n_u16(0);
        let mut a1_x = vdupq_n_u16(0);
        let mut sh_lo = vdup_n_u16(0);
        let mut sh_hi = vdup_n_u16(0);

        if base_shift <= 4 {
            let lo = z2_above_neon(token, origin, base_x);
            if upsample_above {
                // `vld2` deinterleave via load + unzip.
                let a: &[u8; 16] = above[lo..lo + 16].try_into().unwrap();
                let v = vld1q_u8(a);
                a0_x = vmovl_u8(vget_low_u8(vuzp1q_u8(v, v)));
                a1_x = vmovl_u8(vget_low_u8(vuzp2q_u8(v, v)));
                sh_lo = vdup_n_u16((x & 0x1f) as u16);
            } else {
                let a: &[u8; 8] = above[lo..lo + 8].try_into().unwrap();
                let b: &[u8; 8] = above[lo + 1..lo + 9].try_into().unwrap();
                a0_x = vmovl_u8(vld1_u8(a));
                a1_x = vmovl_u8(vld1_u8(b));
                sh_lo = vdup_n_u16(((x & 0x3f) >> 1) as u16);
            }
        }

        if base_x < min_base_x {
            let y_c64 = vmls_s16(vdup_n_s16((r << 6) as i16), v_1234, dy64);
            let base_y = vshl_s16(y_c64, neg_fby);
            let idx0 = vreinterpret_u8_s16(vadd_s16(base_y, vdup_n_s16(2)));
            let idx1 = vreinterpret_u8_s16(vadd_s16(base_y, vdup_n_s16(3)));
            // Gathered byte + a zero byte make each u16 lane.
            let a0_y = vtrn1_u8(vqtbl3_u8(left_vals, idx0), vdup_n_u8(0));
            let a1_y = vtrn1_u8(vqtbl3_u8(left_vals, idx1), vdup_n_u8(0));
            a0_x = vcombine_u16(vget_low_u16(a0_x), vreinterpret_u16_u8(a0_y));
            a1_x = vcombine_u16(vget_low_u16(a1_x), vreinterpret_u16_u8(a1_y));
            sh_hi = if upsample_left {
                vand_u16(vreinterpret_u16_s16(y_c64), c1f)
            } else {
                vand_u16(vshr_n_u16::<1>(vreinterpret_u16_s16(y_c64)), c1f)
            };
        }

        let shift = vcombine_u16(sh_lo, sh_hi);
        let res = vmlaq_u16(vshlq_n_u16::<5>(a0_x), vsubq_u16(a1_x, a0_x), shift);
        let resx = vrshrn_n_u16::<5>(res);
        let resy = vext_u8::<4>(resx, vdup_n_u8(0));
        let mask = vclt_u8(idx8, vdup_n_u8(base_min_diff as u8));
        let resxy = vbsl_u8(mask, resy, resx);
        let word = vget_lane_u32::<0>(vreinterpret_u32_u8(resxy));
        dst[r * dst_stride..r * dst_stride + 4].copy_from_slice(&word.to_le_bytes());
    }
}

/// C `dr_prediction_z2_8xH_neon`: `uint16x8x2` per row — `val[0]` the
/// above-edge interpolation, `val[1]` the 48-entry `vqtbl3q` left-edge
/// gather — then `vmlaq` + `vrshrn` + `vbsl` + an 8-byte store.
#[cfg(target_arch = "aarch64")]
#[rite]
#[allow(clippy::too_many_arguments)]
fn dr_z2_8xh_neon(
    token: NeonToken,
    dst: &mut [u8],
    dst_stride: usize,
    bh: usize,
    above: &[u8],
    left: &[u8],
    origin: usize,
    upsample_above: bool,
    upsample_left: bool,
    dx: i32,
    dy: i32,
) {
    let up_a = upsample_above as i32;
    let up_l = upsample_left as i32;
    let min_base_x = -(1i32 << up_a);
    let frac_bits_x = 6 - up_a;
    let frac_bits_y = 6 - up_l;

    let idx8 = vld1_u8(&Z2_IDX8);
    let c1f = vdupq_n_u16(0x1f);
    let c1234 = vcombine_s16(
        vcreate_s16(0x0004_0003_0002_0001),
        vcreate_s16(0x0008_0007_0006_0005),
    );
    let dy128 = vdupq_n_s16(dy as i16);
    let neg_fby = vdupq_n_s16(-(frac_bits_y as i16));

    // Same contiguous 48-byte gather table as the 4xH kernel.
    let left_vals = z2_left_table_neon(token, left, origin);

    for r in 0..bh {
        let y = r as i32 + 1;
        let x = -y * dx;
        let base_x = x >> frac_bits_x;
        let mut base_shift = 0;
        if base_x < min_base_x - 1 {
            base_shift = (min_base_x - base_x - 1) >> up_a;
        }
        let base_min_diff = ((min_base_x - base_x + up_a) >> up_a).clamp(0, 8);

        let resx = if base_shift <= 8 {
            let lo = z2_above_neon(token, origin, base_x);
            let (a0, a1, sx) = if upsample_above {
                let a: &[u8; 16] = above[lo..lo + 16].try_into().unwrap();
                let v = vld1q_u8(a);
                (
                    vget_low_u8(vuzp1q_u8(v, v)),
                    vget_low_u8(vuzp2q_u8(v, v)),
                    (x & 0x1f) as u16,
                )
            } else {
                let a: &[u8; 8] = above[lo..lo + 8].try_into().unwrap();
                let b: &[u8; 8] = above[lo + 1..lo + 9].try_into().unwrap();
                (vld1_u8(a), vld1_u8(b), ((x & 0x3f) >> 1) as u16)
            };
            vrshrn_n_u16::<5>(vmlaq_u16(
                vshll_n_u8::<5>(a0),
                vsubl_u8(a1, a0),
                vdupq_n_u16(sx),
            ))
        } else {
            vdup_n_u8(0)
        };

        let mut resxy = resx;
        if base_x < min_base_x {
            let y_c128 = vmlsq_s16(vdupq_n_s16((r << 6) as i16), c1234, dy128);
            let base_y = vshlq_s16(y_c128, neg_fby);
            let idx0 = vreinterpretq_u8_s16(vaddq_s16(base_y, vdupq_n_s16(2)));
            let idx1 = vreinterpretq_u8_s16(vaddq_s16(base_y, vdupq_n_s16(3)));
            // Even bytes of each s16 lane are the gather indices; uzp1 packs
            // the eight a0 indices then the eight a1 indices.
            let idx01 = vuzp1q_u8(idx0, idx1);
            let a01 = vqtbl3q_u8(left_vals, idx01);
            let a0_y = vget_low_u8(a01);
            let a1_y = vget_high_u8(a01);
            let sh_y = if upsample_left {
                vandq_u16(vreinterpretq_u16_s16(y_c128), c1f)
            } else {
                vandq_u16(vshrq_n_u16::<1>(vreinterpretq_u16_s16(y_c128)), c1f)
            };
            let resy =
                vrshrn_n_u16::<5>(vmlaq_u16(vshll_n_u8::<5>(a0_y), vsubl_u8(a1_y, a0_y), sh_y));
            let mask = vclt_u8(idx8, vdup_n_u8(base_min_diff as u8));
            resxy = vbsl_u8(mask, resy, resx);
        }

        let d: &mut [u8; 8] = (&mut dst[r * dst_stride..r * dst_stride + 8])
            .try_into()
            .unwrap();
        vst1_u8(d, resxy);
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
    //
    // `bh` of 4 or 8 takes the shared small kernel + a transposed store —
    // C's z3 small shapes. Same buffer guard as [`dr_z1_edged`]'s: the
    // loads top out at `edge[origin + max_base + {15,8}]`, and the
    // upsample branch only runs for `bw + bh <= 16` in production.
    if (bh == 4 || bh == 8) && (bw == 4 || bw % 8 == 0) && bw <= 64 {
        let up = upsample_left as usize;
        let max_base = ((bw + bh - 1) << up) as usize;
        if left.len() >= origin + max_base + if upsample_left { 16 } else { 9 } {
            incant!(
                dr_z3_edged_small(dst, dst_stride, bw, bh, left, origin, upsample_left, dy),
                [neon, scalar]
            );
            return;
        }
    }
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

/// C `transpose4x8_8x4_neon`: four `u8x8` column vectors (each = one z3
/// column, rows in lanes) become four `u32x2`; element `r / 2` lane
/// `r % 2` is output row `r` (4 bytes). `bh == 4` callers use only the
/// first two elements (C's `_low` variant stops after `d[0]`).
#[cfg(target_arch = "aarch64")]
#[rite]
fn transpose4x8_8x4_neon(_token: NeonToken, x: &[uint8x8_t; 4]) -> [uint32x2_t; 4] {
    let w0 = vzip_u8(x[0], x[1]);
    let w1 = vzip_u8(x[2], x[3]);
    let d0 = vzip_u16(vreinterpret_u16_u8(w0.0), vreinterpret_u16_u8(w1.0));
    let d1 = vzip_u16(vreinterpret_u16_u8(w0.1), vreinterpret_u16_u8(w1.1));
    [
        vreinterpret_u32_u16(d0.0),
        vreinterpret_u32_u16(d0.1),
        vreinterpret_u32_u16(d1.0),
        vreinterpret_u32_u16(d1.1),
    ]
}

/// C `transpose8x8_neon`: eight `u8x8` column vectors become eight
/// `u32x2` output rows (element `r` = row `r`, 8 bytes). `bh == 4`
/// callers keep only the first four (C's `transpose8x8_low_neon`).
#[cfg(target_arch = "aarch64")]
#[rite]
fn transpose8x8_neon(_token: NeonToken, x: &[uint8x8_t; 8]) -> [uint32x2_t; 8] {
    let w0 = vzip_u8(x[0], x[1]);
    let w1 = vzip_u8(x[2], x[3]);
    let w2 = vzip_u8(x[4], x[5]);
    let w3 = vzip_u8(x[6], x[7]);
    let w4 = vzip_u16(vreinterpret_u16_u8(w0.0), vreinterpret_u16_u8(w1.0));
    let w5 = vzip_u16(vreinterpret_u16_u8(w2.0), vreinterpret_u16_u8(w3.0));
    let w6 = vzip_u16(vreinterpret_u16_u8(w0.1), vreinterpret_u16_u8(w1.1));
    let w7 = vzip_u16(vreinterpret_u16_u8(w2.1), vreinterpret_u16_u8(w3.1));
    let d0 = vzip_u32(vreinterpret_u32_u16(w4.0), vreinterpret_u32_u16(w5.0));
    let d1 = vzip_u32(vreinterpret_u32_u16(w4.1), vreinterpret_u32_u16(w5.1));
    let d2 = vzip_u32(vreinterpret_u32_u16(w6.0), vreinterpret_u32_u16(w7.0));
    let d3 = vzip_u32(vreinterpret_u32_u16(w6.1), vreinterpret_u32_u16(w7.1));
    [d0.0, d0.1, d1.0, d1.1, d2.0, d2.1, d3.0, d3.1]
}

/// Scalar-tier mirror of [`dr_z3_edged_small_neon`] for `incant!`.
fn dr_z3_edged_small_scalar(
    _token: ScalarToken,
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    left: &[u8],
    origin: usize,
    upsample_left: bool,
    dy: i32,
) {
    dr_z3_edged_core(dst, dst_stride, bw, bh, left, origin, upsample_left, dy);
}

/// C's z3 small shapes (`dr_prediction_z3_{4x4,8x8,4x8,8x4,16x8,16x4,
/// 32x8,32x4}_neon`): the shared [`dr_small_row_neon`] computes one
/// `u8x8` per COLUMN (`left`/`dy` playing the z1 roles), then the
/// vzip-transpose helpers store rows. Covers every `bw` that is 4 or a
/// multiple of 8 with `bh` of 4 or 8 — the union of C's small switch
/// arms, same semantics.
#[cfg(target_arch = "aarch64")]
#[arcane]
fn dr_z3_edged_small_neon(
    token: NeonToken,
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    left: &[u8],
    origin: usize,
    upsample_left: bool,
    dy: i32,
) {
    let max_base = ((bw + bh) as i32 - 1) << upsample_left as i32;
    let mut vecs = [vdup_n_u8(0); 64];
    let mut y = dy;
    for v in vecs.iter_mut().take(bw) {
        *v = dr_small_row_neon(token, bh, left, origin, upsample_left, max_base, y);
        y += dy;
    }
    // vecs[c][r] = dst[r][c]; store transposed.
    if bw == 4 {
        let rows = transpose4x8_8x4_neon(token, vecs[..4].try_into().unwrap());
        for r in 0..bh {
            let word = if r % 2 == 0 {
                vget_lane_u32::<0>(rows[r / 2])
            } else {
                vget_lane_u32::<1>(rows[r / 2])
            };
            dst[r * dst_stride..r * dst_stride + 4].copy_from_slice(&word.to_le_bytes());
        }
    } else {
        for g in 0..bw / 8 {
            let rows = transpose8x8_neon(token, vecs[g * 8..g * 8 + 8].try_into().unwrap());
            for (r, &rv) in rows.iter().enumerate().take(bh) {
                let d: &mut [u8; 8] = (&mut dst
                    [r * dst_stride + g * 8..r * dst_stride + g * 8 + 8])
                    .try_into()
                    .unwrap();
                vst1_u8(d, vreinterpret_u8_u32(rv));
            }
        }
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

fn predict_filter_intra_impl_scalar(
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
fn filter_intra_emit_neon(
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
fn predict_filter_intra_impl_neon(
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
fn filter_intra_emit_v3(
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
fn predict_filter_intra_impl_v3(
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

fn predict_filter_intra_core(
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

    /// The dispatched `predict_smooth` (v3/neon/scalar via `incant!`) must
    /// equal `predict_smooth_core` on every AV1 block size — a SIMD kernel
    /// that differs from its twin makes the bitstream tier-dependent.
    /// Regression witness for the 4c-slot indexing fix (see git log): the
    /// 4x4-only `smooth_corners` test never exercised the SIMD path.
    #[test]
    fn predict_smooth_dispatch_matches_core_all_sizes() {
        let mut seed = 0x12345u32;
        let mut next = || {
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            (seed >> 16) as u8
        };
        for &width in &[4usize, 8, 16, 32, 64] {
            for &height in &[4usize, 8, 16, 32, 64] {
                let above: Vec<u8> = (0..width).map(|_| next()).collect();
                let left: Vec<u8> = (0..height).map(|_| next()).collect();
                let mut want = vec![0u8; width * height];
                let mut got = vec![0u8; width * height];
                predict_smooth_core(&mut want, width, &above, &left, width, height);
                predict_smooth(&mut got, width, &above, &left, width, height);
                if let Some(i) = want.iter().zip(&got).position(|(a, b)| a != b) {
                    panic!(
                        "predict_smooth {width}x{height} diverges at idx {i} \
                         (row {}, col {}): want {} got {}",
                        i / width,
                        i % width,
                        want[i],
                        got[i]
                    );
                }
            }
        }
    }

    #[test]
    fn dr_z2_edged_dispatch_matches_core_all_sizes_flags_angles() {
        // Exercises dr_z2_edged's dispatch arms (the >= 16 staircase NEON,
        // the 4xH/8xH table-gather NEON, and the scalar tail) against the
        // scalar core over every upsample combination, all block sizes the
        // arms take, and (dx, dy) pairs spanning shallow to steep slopes —
        // including dy large enough to push base_x past the load-clamp
        // region (fully left-region rows).
        let mut seed = 0x2468u32;
        let mut next = || {
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            (seed >> 16) as u8
        };
        let origin = EDGE_ORIGIN;
        for &bw in &[4usize, 8, 16] {
            for &bh in &[4usize, 8, 16, 32] {
                let above: Vec<u8> = (0..EDGE_BUF_LEN).map(|_| next()).collect();
                let left: Vec<u8> = (0..EDGE_BUF_LEN).map(|_| next()).collect();
                // Real angle-derived (dx, dy) pairs — the inverse slope
                // relationship is what keeps base_y >= -1 inside the left
                // region. (547, 3) + upsample_above drives base_x to -18 at
                // r = 0, exercising the clamped load on a fully-masked row.
                for &(dx, dy) in &[
                    (27i32, 151i32),
                    (151, 27),
                    (64, 64),
                    (7, 372),
                    (372, 7),
                    (3, 1023),
                    (1023, 3),
                    (547, 3),
                ] {
                    for &(ua, ul) in &[(false, false), (true, false), (false, true), (true, true)] {
                        let mut want = vec![0u8; bw * bh];
                        let mut got = vec![0u8; bw * bh];
                        dr_z2_edged_core(
                            &mut want, bw, bw, bh, &above, &left, origin, ua, ul, dx, dy,
                        );
                        dr_z2_edged(&mut got, bw, bw, bh, &above, &left, origin, ua, ul, dx, dy);
                        if want != got {
                            let i = want.iter().zip(&got).position(|(a, b)| a != b).unwrap();
                            panic!(
                                "dr_z2_edged {bw}x{bh} dx={dx} dy={dy} ua={ua} ul={ul} \
                                 diverges at ({}, {}): want {} got {}",
                                i / bw,
                                i % bw,
                                want[i],
                                got[i]
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn dr_z1_edged_dispatch_matches_core_all_sizes_flags_angles() {
        // dr_z1_edged's arms — the 4xH/8xH row-vector NEON (with and
        // without upsample) and the >= 16 flat NEON — against the scalar
        // core over every size/flag/angle the arms take. The dx set is
        // the legal `DR_INTRA_DERIVATIVE` range, shallow to steep, so
        // `base` crosses `max_base` mid-block (the fill boundary) and
        // odd `max_base - base` remainders exercise the scalar-exact
        // `ceil` lane count.
        let mut seed = 0x1357u32;
        let mut next = || {
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            (seed >> 16) as u8
        };
        let origin = EDGE_ORIGIN;
        for &bw in &[4usize, 8, 16, 32] {
            for &bh in &[4usize, 8, 16, 32, 64] {
                // 256 > EDGE_BUF_LEN: `max_base` reaches
                // `(32 + 64 - 1) << 1 = 190` under upsample, and the
                // scalar core (like C) indexes up to `origin + max_base`
                // unconditionally. Production can't generate that —
                // `svt_aom_use_intra_edge_upsample` requires
                // `bw + bh <= 16` — but the sweep covers it, so the
                // buffer needs the headroom C's stack slack gave it.
                let above: Vec<u8> = (0..256).map(|_| next()).collect();
                for &dx in &[1023i32, 547, 372, 273, 151, 90, 64, 45, 27, 15, 7, 3] {
                    for &ua in &[false, true] {
                        let mut want = vec![0u8; bw * bh];
                        let mut got = vec![0u8; bw * bh];
                        dr_z1_edged_core(&mut want, bw, bw, bh, &above, origin, ua, dx);
                        dr_z1_edged(&mut got, bw, bw, bh, &above, origin, ua, dx);
                        if want != got {
                            let i = want.iter().zip(&got).position(|(a, b)| a != b).unwrap();
                            panic!(
                                "dr_z1_edged {bw}x{bh} dx={dx} ua={ua} \
                                 diverges at ({}, {}): want {} got {}",
                                i / bw,
                                i % bw,
                                want[i],
                                got[i]
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn dr_z3_edged_dispatch_matches_core_all_sizes_flags_angles() {
        // dr_z3_edged's arms — the shared small kernel + transposed
        // store (bh 4/8) and the >= 16-row flat NEON — against the
        // scalar core. Same legal derivative set as the z1 sweep.
        let mut seed = 0x8642u32;
        let mut next = || {
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            (seed >> 16) as u8
        };
        let origin = EDGE_ORIGIN;
        for &bw in &[4usize, 8, 16, 32, 64] {
            for &bh in &[4usize, 8, 16, 32] {
                // 256-byte buffer for the same reason the z1 sweep notes:
                // upsampled `max_base` reaches `(64 + 8 - 1) << 1 = 142`.
                let left: Vec<u8> = (0..256).map(|_| next()).collect();
                for &dy in &[1023i32, 547, 372, 273, 151, 90, 64, 45, 27, 15, 7, 3] {
                    for &ul in &[false, true] {
                        let mut want = vec![0u8; bw * bh];
                        let mut got = vec![0u8; bw * bh];
                        dr_z3_edged_core(&mut want, bw, bw, bh, &left, origin, ul, dy);
                        dr_z3_edged(&mut got, bw, bw, bh, &left, origin, ul, dy);
                        if want != got {
                            let i = want.iter().zip(&got).position(|(a, b)| a != b).unwrap();
                            panic!(
                                "dr_z3_edged {bw}x{bh} dy={dy} ul={ul} \
                                 diverges at ({}, {}): want {} got {}",
                                i / bw,
                                i % bw,
                                want[i],
                                got[i]
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn predict_smooth_vh_dispatch_matches_core_all_sizes() {
        let mut seed = 0x6789au32;
        let mut next = || {
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            (seed >> 16) as u8
        };
        for &width in &[4usize, 8, 16, 32, 64] {
            for &height in &[4usize, 8, 16, 32, 64] {
                let above: Vec<u8> = (0..width).map(|_| next()).collect();
                let left: Vec<u8> = (0..height).map(|_| next()).collect();
                let mut want = vec![0u8; width * height];
                let mut got = vec![0u8; width * height];
                predict_smooth_v_core(&mut want, width, &above, &left, height, width);
                predict_smooth_v(&mut got, width, &above, &left, 0, height, width);
                assert_eq!(want, got, "predict_smooth_v {width}x{height}");
                predict_smooth_h_core(&mut want, width, &above, &left, width, height);
                predict_smooth_h(&mut got, width, &above, &left, width, height);
                assert_eq!(want, got, "predict_smooth_h {width}x{height}");
            }
        }
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

    /// `predict_filter_intra` under every dispatch tier must equal the scalar
    /// core — over all 5 modes and every legal block dim, across flat,
    /// extreme and pseudo-random neighbor patterns (the cases that stress
    /// the maddubs sign/saturation and the mulhrs-vs-explicit-rounding
    /// divergence on negative sums). The report is consumed, matching the
    /// `for_each_tier` contract `cdef.rs`'s tests document.
    #[test]
    fn filter_intra_all_tiers_match_scalar() {
        use archmage::testing::{CompileTimePolicy, TokenPermutation, for_each_token_permutation};
        let mut check = |_: &TokenPermutation| {
            let mut st = 0x9E3779B97F4A7C15u64;
            let mut lcg = || {
                st ^= st << 13;
                st ^= st >> 7;
                st ^= st << 17;
                (st >> 33) as u8
            };
            for kind in 0..4 {
                for w in [4usize, 8, 16, 32] {
                    for h in [4usize, 8, 16, 32] {
                        let above: Vec<u8> = (0..w + 1)
                            .map(|i| match kind {
                                0 => 128,
                                1 => 255,
                                2 => ((i * 37 + w) & 0xFF) as u8,
                                _ => lcg(),
                            })
                            .collect();
                        let left: Vec<u8> = (0..h)
                            .map(|i| match kind {
                                0 => 128,
                                1 => 0,
                                2 => ((i * 53 + h) & 0xFF) as u8,
                                _ => lcg(),
                            })
                            .collect();
                        for mode in 0..5u8 {
                            let mut want = vec![0u8; w * h];
                            predict_filter_intra_core(&mut want, w, &above, &left, w, h, mode);
                            let mut got = vec![0u8; w * h];
                            predict_filter_intra(&mut got, w, &above, &left, w, h, mode);
                            assert_eq!(
                                got, want,
                                "kind={kind} {w}x{h} mode={mode}: dispatch diverged \
                                 from scalar"
                            );
                        }
                    }
                }
            }
        };
        let report = for_each_token_permutation(CompileTimePolicy::WarnStderr, &mut check);
        assert!(
            report.warnings.is_empty(),
            "archmage excluded token(s): {:?}",
            report.warnings
        );
        assert!(
            report.permutations_run >= 2,
            "tier sweep ran {} permutation(s) — cannot catch a SIMD-vs-scalar \
             divergence.",
            report.permutations_run
        );
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
