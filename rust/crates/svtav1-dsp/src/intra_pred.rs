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
    incant!(
        predict_dc_impl(
            dst, dst_stride, above, left, width, height, has_above, has_left
        ),
        [v3, neon, scalar]
    )
}

fn predict_dc_impl_scalar(
    _token: ScalarToken,
    dst: &mut [u8],
    dst_stride: usize,
    above: &[u8],
    left: &[u8],
    width: usize,
    height: usize,
    has_above: bool,
    has_left: bool,
) {
    predict_dc_core(
        dst, dst_stride, above, left, width, height, has_above, has_left,
    );
}

/// Scalar core of [`predict_dc`]; every tier must produce this `dc` value.
fn predict_dc_core(
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

/// Sum the first `n` bytes of `e` with UADDLV widening reductions — 16-byte
/// chunks, then 8- and 4-byte narrow loads so every AV1 edge length (4..64)
/// is fully covered. `vaddlvq_u8`/`vaddlv_u8` return the exact u16 total.
#[cfg(target_arch = "aarch64")]
#[rite]
fn dc_edge_sum_neon(_token: NeonToken, e: &[u8], n: usize) -> u32 {
    let mut sum = 0u32;
    let mut c = 0usize;
    while c + 16 <= n {
        let v: &[u8; 16] = e[c..c + 16].try_into().unwrap();
        sum += u32::from(vaddlvq_u8(vld1q_u8(v)));
        c += 16;
    }
    if c + 8 <= n {
        let v: &[u8; 8] = e[c..c + 8].try_into().unwrap();
        sum += u32::from(vaddlv_u8(vld1_u8(v)));
        c += 8;
    }
    if c + 4 <= n {
        let v: &[u8; 4] = e[c..c + 4].try_into().unwrap();
        sum += u32::from(vaddlv_u8(vcreate_u8(u64::from(u32::from_le_bytes(*v)))));
        c += 4;
    }
    for k in c..n {
        sum += e[k] as u32;
    }
    sum
}

/// aarch64 arm of [`predict_dc`]: the edge sums go through
/// `dc_edge_sum_neon`; the destination fill stays `.fill` (memset — already
/// optimal). Same division and rounding as the scalar core, exact.
#[cfg(target_arch = "aarch64")]
#[arcane]
fn predict_dc_impl_neon(
    token: NeonToken,
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
            let sum = dc_edge_sum_neon(token, above, width) + dc_edge_sum_neon(token, left, height);
            let count = (width + height) as u32;
            ((sum + count / 2) / count) as u8
        }
        (true, false) => {
            let sum = dc_edge_sum_neon(token, above, width);
            ((sum + width as u32 / 2) / width as u32) as u8
        }
        (false, true) => {
            let sum = dc_edge_sum_neon(token, left, height);
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

/// Sum the first `n` bytes of `e` with `psadbw`-against-zero —
/// `_mm256_sad_epu8`/`_mm_sad_epu8` return the exact u64 lane totals.
/// 32-, 16-, 8- and 4-byte chunks cover every AV1 edge length (4..64)
/// completely; a scalar tail keeps the function total for any `n`. x86-64
/// twin of [`dc_edge_sum_neon`].
#[cfg(target_arch = "x86_64")]
#[rite]
fn dc_edge_sum_v3(_token: Desktop64, e: &[u8], n: usize) -> u32 {
    let zero256 = _mm256_setzero_si256();
    let zero128 = _mm_setzero_si128();
    let mut acc = zero256;
    let mut c = 0usize;
    while c + 32 <= n {
        let v: &[u8; 32] = e[c..c + 32].try_into().unwrap();
        acc = _mm256_add_epi64(acc, _mm256_sad_epu8(_mm256_loadu_si256(v), zero256));
        c += 32;
    }
    let mut acc128 = zero128;
    if c + 16 <= n {
        let v: &[u8; 16] = e[c..c + 16].try_into().unwrap();
        acc128 = _mm_add_epi64(acc128, _mm_sad_epu8(_mm_loadu_si128(v), zero128));
        c += 16;
    }
    if c + 8 <= n {
        let v: &[u8; 8] = e[c..c + 8].try_into().unwrap();
        acc128 = _mm_add_epi64(acc128, _mm_sad_epu8(_mm_loadu_si64(v), zero128));
        c += 8;
    }
    if c + 4 <= n {
        let v: &[u8; 4] = e[c..c + 4].try_into().unwrap();
        acc128 = _mm_add_epi64(acc128, _mm_sad_epu8(_mm_loadu_si32(v), zero128));
        c += 4;
    }
    let s256 = _mm_add_epi64(_mm256_castsi256_si128(acc), _mm256_extracti128_si256::<1>(acc));
    let acc128 = _mm_add_epi64(acc128, s256);
    let mut sum = (_mm_cvtsi128_si64(acc128)
        + _mm_cvtsi128_si64(_mm_srli_si128::<8>(acc128))) as u32;
    for k in c..n {
        sum += e[k] as u32;
    }
    sum
}

/// x86-64 v3 arm of [`predict_dc`]: the edge sums go through
/// `dc_edge_sum_v3`; the destination fill stays `.fill` (memset — already
/// optimal). Same division and rounding as the scalar core, exact.
#[cfg(target_arch = "x86_64")]
#[arcane]
fn predict_dc_impl_v3(
    token: Desktop64,
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
            let sum = dc_edge_sum_v3(token, above, width) + dc_edge_sum_v3(token, left, height);
            let count = (width + height) as u32;
            ((sum + count / 2) / count) as u8
        }
        (true, false) => {
            let sum = dc_edge_sum_v3(token, above, width);
            ((sum + width as u32 / 2) / width as u32) as u8
        }
        (false, true) => {
            let sum = dc_edge_sum_v3(token, left, height);
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
        [v3, neon, scalar]
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

/// x86-64 v3 arm of [`predict_smooth_v`]. [`predict_smooth_impl_v3`]'s
/// structure with the horizontal term removed: per row only `w` varies, so
///   pred[c] = (w * top[c] + K) >> 8,  K = (256 - w)*below + 128
/// — the numerator is nonneg and <= 130,433, so i32 lanes are exact, `>> 8`
/// is the scalar floor-div, and the u8 saturating narrow is `.min(255)`.
#[cfg(target_arch = "x86_64")]
#[arcane]
fn predict_smooth_v_impl_v3(
    token: Desktop64,
    dst: &mut [u8],
    dst_stride: usize,
    above: &[u8],
    left: &[u8],
    height: usize,
    width: usize,
) {
    use magetypes::simd::generic::{i32x8, u8x32};
    // Same gate as the NEON arm: one i32x8 covers 8 columns, so widths that
    // are not a multiple of 8 stay scalar.
    if width < 8 || width % 8 != 0 || width > 64 {
        predict_smooth_v_core(dst, dst_stride, above, left, height, width);
        return;
    }
    let below = i32::from(left[height - 1]);
    let sm_weights = smooth_weights(height);
    // Hoist `above` widened to i32 lanes: one i32x8 per 8 columns.
    let mut tops = [i32x8::splat(token, 0); 8];
    let mut j = 0usize;
    while j * 8 < width {
        let take = (width - j * 8).min(16);
        let mut ta = [0u8; 32];
        ta[..take].copy_from_slice(&above[j * 8..j * 8 + take]);
        let t16 = u8x32::load(token, &ta).widen_low().bitcast_i16x16();
        tops[j] = t16.widen_low();
        tops[j + 1] = t16.widen_high();
        j += 2;
    }
    for row in 0..height {
        let w = i32::from(sm_weights[row]);
        let k = (256 - w) * below + 128;
        let base = row * dst_stride;
        let mut c = 0usize;
        while c + 16 <= width {
            let a0 = (tops[c / 8] * w + k).shr_logical_const::<8>();
            let a1 = (tops[c / 8 + 1] * w + k).shr_logical_const::<8>();
            let a16 = a0.narrow_saturating_i16(a1);
            let mut tmp = [0u8; 32];
            a16.narrow_saturating_u8(a16).store(&mut tmp);
            dst[base + c..base + c + 16].copy_from_slice(&tmp[..16]);
            c += 16;
        }
        if c + 8 <= width {
            let a = (tops[c / 8] * w + k).shr_logical_const::<8>();
            let a16 = a.narrow_saturating_i16(a);
            let mut tmp = [0u8; 32];
            a16.narrow_saturating_u8(a16).store(&mut tmp);
            dst[base + c..base + c + 8].copy_from_slice(&tmp[..8]);
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
        [v3, neon, scalar]
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

/// x86-64 v3 arm of [`predict_smooth_h`]. Mirror of
/// [`predict_smooth_v_impl_v3`]: per row only `d` varies, so
///   pred[c] = (ww[c] * d + K) >> 8,  d = left[row] - right,
///   K = 256*right + 128
/// — `ww*d` can be negative in an i32 lane but the total equals the all-
/// nonneg scalar numerator (<= 130,433), so `>> 8` is the scalar floor-div
/// and the saturating narrow is `.min(255)`.
#[cfg(target_arch = "x86_64")]
#[arcane]
fn predict_smooth_h_impl_v3(
    token: Desktop64,
    dst: &mut [u8],
    dst_stride: usize,
    above: &[u8],
    left: &[u8],
    width: usize,
    height: usize,
) {
    use magetypes::simd::generic::{i32x8, u8x32};
    if width < 8 || width % 8 != 0 || width > 64 {
        predict_smooth_h_core(dst, dst_stride, above, left, width, height);
        return;
    }
    let right = i32::from(above[width - 1]);
    let sm_w = smooth_weights(width);
    // Hoist the per-column weights widened to i32 lanes.
    let mut wws = [i32x8::splat(token, 0); 8];
    let mut j = 0usize;
    while j * 8 < width {
        let take = (width - j * 8).min(16);
        let mut wa = [0u8; 32];
        wa[..take].copy_from_slice(&sm_w[j * 8..j * 8 + take]);
        let w16 = u8x32::load(token, &wa).widen_low().bitcast_i16x16();
        wws[j] = w16.widen_low();
        wws[j + 1] = w16.widen_high();
        j += 2;
    }
    let k = 256 * right + 128;
    for row in 0..height {
        let d = i32::from(left[row]) - right;
        let base = row * dst_stride;
        let mut c = 0usize;
        while c + 16 <= width {
            let a0 = (wws[c / 8] * d + k).shr_logical_const::<8>();
            let a1 = (wws[c / 8 + 1] * d + k).shr_logical_const::<8>();
            let a16 = a0.narrow_saturating_i16(a1);
            let mut tmp = [0u8; 32];
            a16.narrow_saturating_u8(a16).store(&mut tmp);
            dst[base + c..base + c + 16].copy_from_slice(&tmp[..16]);
            c += 16;
        }
        if c + 8 <= width {
            let a = (wws[c / 8] * d + k).shr_logical_const::<8>();
            let a16 = a.narrow_saturating_i16(a);
            let mut tmp = [0u8; 32];
            a16.narrow_saturating_u8(a16).store(&mut tmp);
            dst[base + c..base + c + 8].copy_from_slice(&tmp[..8]);
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

#[cfg(test)]
mod tests;
#[cfg(test)]
mod dispatch_tests;
mod directional;
pub use directional::*;

mod filter_intra;
pub use filter_intra::*;
