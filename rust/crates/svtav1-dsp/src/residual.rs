//! Residual, reconstruction and coefficient-domain distortion kernels.
//!
//! The three element-wise loops that bracket every transform unit. They were
//! scalar loops inlined into `leaf_funnel::tx_unit`, which a post-transform-port
//! re-profile measured as the single hottest leaf in the encoder — 11.47 % of
//! preset-6 self time at 512x512. C ships them as `svt_residual_kernel8bit_neon`
//! and `svt_full_distortion_kernel32_bits_neon`.
//!
//! All three are pure element-wise maps or a sum of independent products, so
//! lanes carry no cross-element dependency and the SIMD arms are **bit-identical
//! to the scalar reference by construction** — the only ordering that changes is
//! the addition order inside [`sse_i32`]'s accumulator, and that is exact
//! integer addition (see its doc for the overflow bound).

#[allow(unused_imports)]
use archmage::prelude::*;

/// `out[r*w + c] = src[r*src_stride + c] as i32 - pred[r*pred_stride + c] as i32`.
///
/// `out` must be exactly `w * h` long; every element is written.
pub fn residual_i32(
    src: &[u8],
    src_stride: usize,
    pred: &[u8],
    pred_stride: usize,
    w: usize,
    h: usize,
    out: &mut [i32],
) {
    incant!(
        residual_i32_impl(src, src_stride, pred, pred_stride, w, h, out),
        [v3, neon, scalar]
    )
}

#[inline]
fn residual_i32_core(
    src: &[u8],
    src_stride: usize,
    pred: &[u8],
    pred_stride: usize,
    w: usize,
    h: usize,
    out: &mut [i32],
) {
    for r in 0..h {
        let s = &src[r * src_stride..r * src_stride + w];
        let p = &pred[r * pred_stride..r * pred_stride + w];
        let o = &mut out[r * w..r * w + w];
        for ((o, &s), &p) in o.iter_mut().zip(s).zip(p) {
            *o = s as i32 - p as i32;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn residual_i32_impl_scalar(
    _token: ScalarToken,
    src: &[u8],
    src_stride: usize,
    pred: &[u8],
    pred_stride: usize,
    w: usize,
    h: usize,
    out: &mut [i32],
) {
    residual_i32_core(src, src_stride, pred, pred_stride, w, h, out);
}

#[cfg(target_arch = "x86_64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn residual_i32_impl_v3(
    _token: Desktop64,
    src: &[u8],
    src_stride: usize,
    pred: &[u8],
    pred_stride: usize,
    w: usize,
    h: usize,
    out: &mut [i32],
) {
    residual_i32_core(src, src_stride, pred, pred_stride, w, h, out);
}

/// 16 columns per iteration: widen both u8 rows to i16, subtract (the
/// difference is in `[-255, 255]`, so i16 is exact), then widen the two halves
/// to i32. Tail columns fall to the scalar core, which computes the identical
/// value.
#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn residual_i32_impl_neon(
    _token: NeonToken,
    src: &[u8],
    src_stride: usize,
    pred: &[u8],
    pred_stride: usize,
    w: usize,
    h: usize,
    out: &mut [i32],
) {
    for r in 0..h {
        let s = &src[r * src_stride..r * src_stride + w];
        let p = &pred[r * pred_stride..r * pred_stride + w];
        let o = &mut out[r * w..r * w + w];
        let mut c = 0usize;
        while c + 16 <= w {
            let sa: &[u8; 16] = s[c..c + 16].try_into().unwrap();
            let pa: &[u8; 16] = p[c..c + 16].try_into().unwrap();
            let sv = vld1q_u8(sa);
            let pv = vld1q_u8(pa);
            let d_lo = vreinterpretq_s16_u16(vsubl_u8(vget_low_u8(sv), vget_low_u8(pv)));
            let d_hi = vreinterpretq_s16_u16(vsubl_high_u8(sv, pv));
            let q0: &mut [i32; 4] = (&mut o[c..c + 4]).try_into().unwrap();
            vst1q_s32(q0, vmovl_s16(vget_low_s16(d_lo)));
            let q1: &mut [i32; 4] = (&mut o[c + 4..c + 8]).try_into().unwrap();
            vst1q_s32(q1, vmovl_high_s16(d_lo));
            let q2: &mut [i32; 4] = (&mut o[c + 8..c + 12]).try_into().unwrap();
            vst1q_s32(q2, vmovl_s16(vget_low_s16(d_hi)));
            let q3: &mut [i32; 4] = (&mut o[c + 12..c + 16]).try_into().unwrap();
            vst1q_s32(q3, vmovl_high_s16(d_hi));
            c += 16;
        }
        for k in c..w {
            o[k] = s[k] as i32 - p[k] as i32;
        }
    }
}

/// `out[r*w + c] = clamp(pred[r*pred_stride + c] as i32 + inv[r*w + c], 0, 255)`.
///
/// The reconstruction add after the inverse transform. `inv` and `out` are both
/// `w * h` in the transform block's own raster; `pred` carries its own stride.
pub fn recon_add_clamp(
    pred: &[u8],
    pred_stride: usize,
    inv: &[i32],
    w: usize,
    h: usize,
    out: &mut [u8],
) {
    incant!(
        recon_add_clamp_impl(pred, pred_stride, inv, w, h, out),
        [v3, neon, scalar]
    )
}

#[inline]
fn recon_add_clamp_core(
    pred: &[u8],
    pred_stride: usize,
    inv: &[i32],
    w: usize,
    h: usize,
    out: &mut [u8],
) {
    for r in 0..h {
        let p = &pred[r * pred_stride..r * pred_stride + w];
        let i = &inv[r * w..r * w + w];
        let o = &mut out[r * w..r * w + w];
        for ((o, &p), &i) in o.iter_mut().zip(p).zip(i) {
            *o = (p as i32 + i).clamp(0, 255) as u8;
        }
    }
}

fn recon_add_clamp_impl_scalar(
    _token: ScalarToken,
    pred: &[u8],
    pred_stride: usize,
    inv: &[i32],
    w: usize,
    h: usize,
    out: &mut [u8],
) {
    recon_add_clamp_core(pred, pred_stride, inv, w, h, out);
}

#[cfg(target_arch = "x86_64")]
#[arcane]
fn recon_add_clamp_impl_v3(
    _token: Desktop64,
    pred: &[u8],
    pred_stride: usize,
    inv: &[i32],
    w: usize,
    h: usize,
    out: &mut [u8],
) {
    recon_add_clamp_core(pred, pred_stride, inv, w, h, out);
}

/// 8 columns per iteration. The sum `pred + inv` is formed in i32 (the residual
/// can exceed i16 before clamping), then `vqmovun_s32` + `vqmovn_u16` saturate
/// to `[0, 255]` — which IS the `clamp(0, 255)` the scalar does, because
/// unsigned-saturating narrowing maps negatives to 0 and anything above 255 to
/// 255. The two-step narrow cannot alias: after the first saturation the value
/// is already inside `[0, 65535]` and inside `[0, 255]` in particular.
#[cfg(target_arch = "aarch64")]
#[arcane]
fn recon_add_clamp_impl_neon(
    _token: NeonToken,
    pred: &[u8],
    pred_stride: usize,
    inv: &[i32],
    w: usize,
    h: usize,
    out: &mut [u8],
) {
    for r in 0..h {
        let p = &pred[r * pred_stride..r * pred_stride + w];
        let iv = &inv[r * w..r * w + w];
        let o = &mut out[r * w..r * w + w];
        let mut c = 0usize;
        while c + 8 <= w {
            let pa: &[u8; 8] = p[c..c + 8].try_into().unwrap();
            let pw = vmovl_u16(vget_low_u16(vmovl_u8(vld1_u8(pa))));
            let ph = vmovl_high_u16(vmovl_u8(vld1_u8(pa)));
            let i0: &[i32; 4] = iv[c..c + 4].try_into().unwrap();
            let i1: &[i32; 4] = iv[c + 4..c + 8].try_into().unwrap();
            let s0 = vaddq_s32(vreinterpretq_s32_u32(pw), vld1q_s32(i0));
            let s1 = vaddq_s32(vreinterpretq_s32_u32(ph), vld1q_s32(i1));
            let n16 = vcombine_u16(vqmovun_s32(s0), vqmovun_s32(s1));
            let oa: &mut [u8; 8] = (&mut o[c..c + 8]).try_into().unwrap();
            vst1_u8(oa, vqmovn_u16(n16));
            c += 8;
        }
        for k in c..w {
            o[k] = (p[k] as i32 + iv[k]).clamp(0, 255) as u8;
        }
    }
}

/// `sum_i (a[i] - b[i])^2` over equal-length `i32` slices, in `u64`.
///
/// The frequency-domain residual distortion
/// (`svt_aom_picture_full_distortion32_bits_single`). Pass `b` empty-equivalent
/// by calling [`sq_sum_i32`] when there is no subtrahend.
pub fn sse_i32(a: &[i32], b: &[i32]) -> u64 {
    incant!(sse_i32_impl(a, b), [v3, neon, scalar])
}

#[inline]
fn sse_i32_core(a: &[i32], b: &[i32]) -> u64 {
    let mut d: u64 = 0;
    for (&x, &y) in a.iter().zip(b) {
        let e = (x - y) as i64;
        d += (e * e) as u64;
    }
    d
}

fn sse_i32_impl_scalar(_token: ScalarToken, a: &[i32], b: &[i32]) -> u64 {
    sse_i32_core(a, b)
}

#[cfg(target_arch = "x86_64")]
#[arcane]
fn sse_i32_impl_v3(_token: Desktop64, a: &[i32], b: &[i32]) -> u64 {
    sse_i32_core(a, b)
}

/// Four i64 lanes accumulate the squared differences; `vmlal_s32` /
/// `vmlal_high_s32` give the widened product-accumulate directly, so the
/// per-lane sum is exact in i64 exactly as the scalar's is.
///
/// Re-association: the scalar adds the squares left to right into one u64; this
/// adds them into four independent i64 lanes and sums the lanes at the end.
/// Every partial sum is exact (no wrapping — coefficients are transform
/// outputs, and even the theoretical worst case of 1024 squares of a full i32
/// difference is under 2^73... which is why the scalar itself widens to i64
/// per term and the *total* is the value both forms produce), so the two agree
/// bit for bit by associativity of integer addition.
#[cfg(target_arch = "aarch64")]
#[arcane]
fn sse_i32_impl_neon(_token: NeonToken, a: &[i32], b: &[i32]) -> u64 {
    let n = a.len().min(b.len());
    let mut acc0 = vdupq_n_s64(0);
    let mut acc1 = vdupq_n_s64(0);
    let mut i = 0usize;
    while i + 4 <= n {
        let xa: &[i32; 4] = a[i..i + 4].try_into().unwrap();
        let ya: &[i32; 4] = b[i..i + 4].try_into().unwrap();
        let d = vsubq_s32(vld1q_s32(xa), vld1q_s32(ya));
        acc0 = vmlal_s32(acc0, vget_low_s32(d), vget_low_s32(d));
        acc1 = vmlal_high_s32(acc1, d, d);
        i += 4;
    }
    let mut d = (vaddvq_s64(vaddq_s64(acc0, acc1))) as u64;
    for k in i..n {
        let e = (a[k] - b[k]) as i64;
        d += (e * e) as u64;
    }
    d
}

/// `sum_i a[i]^2` — the `eob == 0` arm of the frequency-domain distortion.
pub fn sq_sum_i32(a: &[i32]) -> u64 {
    incant!(sq_sum_i32_impl(a), [v3, neon, scalar])
}

#[inline]
fn sq_sum_i32_core(a: &[i32]) -> u64 {
    let mut d: u64 = 0;
    for &x in a {
        let e = x as i64;
        d += (e * e) as u64;
    }
    d
}

fn sq_sum_i32_impl_scalar(_token: ScalarToken, a: &[i32]) -> u64 {
    sq_sum_i32_core(a)
}

#[cfg(target_arch = "x86_64")]
#[arcane]
fn sq_sum_i32_impl_v3(_token: Desktop64, a: &[i32]) -> u64 {
    sq_sum_i32_core(a)
}

#[cfg(target_arch = "aarch64")]
#[arcane]
fn sq_sum_i32_impl_neon(_token: NeonToken, a: &[i32]) -> u64 {
    let n = a.len();
    let mut acc0 = vdupq_n_s64(0);
    let mut acc1 = vdupq_n_s64(0);
    let mut i = 0usize;
    while i + 4 <= n {
        let xa: &[i32; 4] = a[i..i + 4].try_into().unwrap();
        let v = vld1q_s32(xa);
        acc0 = vmlal_s32(acc0, vget_low_s32(v), vget_low_s32(v));
        acc1 = vmlal_high_s32(acc1, v, v);
        i += 4;
    }
    let mut d = (vaddvq_s64(vaddq_s64(acc0, acc1))) as u64;
    for &x in &a[i..n] {
        let e = x as i64;
        d += (e * e) as u64;
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use archmage::testing::{CompileTimePolicy, for_each_token_permutation};
    use alloc::vec;
    use alloc::vec::Vec;

    fn lcg(state: &mut u64) -> u64 {
        *state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        *state >> 33
    }

    /// Every dispatch tier reproduces the scalar core bit for bit, across every
    /// AV1 transform width/height AND widths that are not multiples of the
    /// vector body (so the scalar tail runs), including the saturating extremes
    /// of the recon clamp.
    #[test]
    fn residual_recon_distortion_all_tiers_match_core() {
        let mut st = 0x5EED_1234_u64;
        for &(w, h) in &[
            (4usize, 4usize), (8, 8), (16, 16), (32, 32), (64, 64), (4, 16), (16, 4),
            (8, 32), (32, 8), (5, 3), (7, 9), (13, 2), (1, 1), (3, 1),
        ] {
            let sstride = w + 7;
            let pstride = w + 3;
            let src: Vec<u8> = (0..sstride * h + 32).map(|_| (lcg(&mut st) & 0xff) as u8).collect();
            let pred: Vec<u8> = (0..pstride * h + 32).map(|_| (lcg(&mut st) & 0xff) as u8).collect();
            // Residual
            let mut want = vec![0i32; w * h];
            residual_i32_core(&src, sstride, &pred, pstride, w, h, &mut want);
            let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_p| {
                let mut got = vec![0i32; w * h];
                residual_i32(&src, sstride, &pred, pstride, w, h, &mut got);
                assert_eq!(got, want, "residual {w}x{h}");
            });
            // Recon: inv values chosen to straddle both clamp bounds hard.
            let inv: Vec<i32> = (0..w * h)
                .map(|i| match i % 5 {
                    0 => -100000,
                    1 => -255,
                    2 => 0,
                    3 => 255,
                    _ => 100000,
                })
                .collect();
            let mut rwant = vec![0u8; w * h];
            recon_add_clamp_core(&pred, pstride, &inv, w, h, &mut rwant);
            let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_p| {
                let mut got = vec![0u8; w * h];
                recon_add_clamp(&pred, pstride, &inv, w, h, &mut got);
                assert_eq!(got, rwant, "recon {w}x{h}");
            });
            // Coefficient-domain distortion, including large magnitudes.
            let ca: Vec<i32> =
                (0..w * h).map(|_| (lcg(&mut st) as i32) >> ((lcg(&mut st) % 20) as u32)).collect();
            let cb: Vec<i32> =
                (0..w * h).map(|_| (lcg(&mut st) as i32) >> (12 + (lcg(&mut st) % 8) as u32)).collect();
            let dwant = sse_i32_core(&ca, &cb);
            let swant = sq_sum_i32_core(&ca);
            let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_p| {
                assert_eq!(sse_i32(&ca, &cb), dwant, "sse_i32 {w}x{h}");
                assert_eq!(sq_sum_i32(&ca), swant, "sq_sum_i32 {w}x{h}");
            });
        }
    }
}
