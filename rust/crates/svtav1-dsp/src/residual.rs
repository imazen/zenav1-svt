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
//! the addition order inside [`sse_i32`]'s accumulator, and two's-complement
//! addition is associative even when it wraps.
//!
//! The distortion kernels reproduce C's ARITHMETIC WIDTHS, not Rust's defaults:
//! C subtracts coefficients in `int64_t` and accumulates in a wrapping
//! `uint64_t`. See [`sse_i32_core`] for the three places that matters and what
//! was measured about each.

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

/// `out[r*w + c] = src[r*src_stride + c] as i16 - pred[r*pred_stride + c] as i16`.
///
/// The i16 twin of [`residual_i32`], for the Hadamard/SATD path (C
/// `svt_residual_kernel8bit`, whose output is `int16_t`). The difference of two
/// `u8`s lies in `[-255, 255]`, so i16 is exact and this is not a narrowing.
///
/// `out` must be exactly `w * h` long; every element is written.
///
/// # Why this is a hand-written per-ISA arm and not `#[magetypes]`
///
/// It needs a `u8x16 -> i16x8` WIDENING subtract. **This paragraph's original
/// reason is STALE and the claim is now only that the generic body is
/// UNMEASURED here.** It used to read "magetypes 0.9.28 has no integer widening
/// in either direction"; that was true of the pinned version, and archmage
/// PR #74 (`widen_low` / `widen_high` / `narrow_saturating`) plus PR #96
/// (`abs_diff`, `madd_adjacent`) have since supplied every primitive this arm
/// needs — `crates/svtav1-dsp/src/variance.rs` now carries a real
/// `#[magetypes]` body against them.
///
/// What the same measurement showed is the reason to bench before collapsing
/// this one: in `variance::sse` the generic body is **1.45x-2.20x SLOWER than
/// the hand NEON arm** on an M4 Pro and a clear win on x86, so the answer is
/// per-ISA and cannot be assumed
/// (`benchmarks/sse_madd_2026-09-05.meta` §5). Same standing question for
/// [`residual_i32`], `crate::me_sad` (which has its own, different and still
/// live, reason — see that module's header) and the directional-intra arms.
pub fn residual_i16(
    src: &[u8],
    src_stride: usize,
    pred: &[u8],
    pred_stride: usize,
    w: usize,
    h: usize,
    out: &mut [i16],
) {
    incant!(
        residual_i16_impl(src, src_stride, pred, pred_stride, w, h, out),
        [v3, neon, scalar]
    )
}

#[inline]
fn residual_i16_core(
    src: &[u8],
    src_stride: usize,
    pred: &[u8],
    pred_stride: usize,
    w: usize,
    h: usize,
    out: &mut [i16],
) {
    for r in 0..h {
        let s = &src[r * src_stride..r * src_stride + w];
        let p = &pred[r * pred_stride..r * pred_stride + w];
        let o = &mut out[r * w..r * w + w];
        for ((o, &s), &p) in o.iter_mut().zip(s).zip(p) {
            *o = s as i16 - p as i16;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn residual_i16_impl_scalar(
    _token: ScalarToken,
    src: &[u8],
    src_stride: usize,
    pred: &[u8],
    pred_stride: usize,
    w: usize,
    h: usize,
    out: &mut [i16],
) {
    residual_i16_core(src, src_stride, pred, pred_stride, w, h, out);
}

#[cfg(target_arch = "x86_64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn residual_i16_impl_v3(
    _token: Desktop64,
    src: &[u8],
    src_stride: usize,
    pred: &[u8],
    pred_stride: usize,
    w: usize,
    h: usize,
    out: &mut [i16],
) {
    residual_i16_core(src, src_stride, pred, pred_stride, w, h, out);
}

/// 16 columns per iteration: widen both u8 rows to i16 and subtract (the
/// difference is in `[-255, 255]`, so i16 is exact). Tail columns fall to the
/// scalar core, which computes the identical value.
#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn residual_i16_impl_neon(
    _token: NeonToken,
    src: &[u8],
    src_stride: usize,
    pred: &[u8],
    pred_stride: usize,
    w: usize,
    h: usize,
    out: &mut [i16],
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
            let q0: &mut [i16; 8] = (&mut o[c..c + 8]).try_into().unwrap();
            vst1q_s16(q0, d_lo);
            let q1: &mut [i16; 8] = (&mut o[c + 8..c + 16]).try_into().unwrap();
            vst1q_s16(q1, d_hi);
            c += 16;
        }
        for k in c..w {
            o[k] = s[k] as i16 - p[k] as i16;
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

/// Bit-exact transcription of `svt_full_distortion_kernel32_bits_c`'s RESIDUAL
/// term (`Codec/pic_operators.c:86`):
///
/// ```c
/// residual_distortion += (int64_t)SQR((int64_t)(coeff[i]) - (recon_coeff[i]));
/// ```
///
/// Three widths matter and each one is C's, not Rust's default:
///
/// * **The subtraction is in `int64_t`.** C's cast promotes BOTH operands
///   before subtracting, so the difference of two `int32_t` can never wrap.
///   This loop used to compute `(x - y) as i64` — an i32 subtraction, THEN a
///   widen — which wraps for differences outside i32 and would hand the RD
///   search a distortion C never computes. (Measured 2026-08-11: it never
///   fires on a real encode — 0 wraps in 59,088,480 elements over 127 cells,
///   max |difference| 788 against an i32 ceiling of 2,147,483,647. The fix is
///   for the arithmetic contract, not for an observed miscompare; see
///   `benchmarks/sse_i32_width_2026-08-11.meta`.)
/// * **The square wraps rather than panicking.** After widening, `e` can reach
///   `2^32` in magnitude and `e * e` then exceeds `i64::MAX`; C's signed
///   multiply wraps in practice, and a debug-build panic where the release
///   build silently wraps is its own defect. `wrapping_mul` pins both builds
///   to the C result. The low 64 bits are the same read signed or unsigned.
/// * **The accumulator wraps.** C accumulates into `uint64_t`, which is
///   defined to wrap; `+=` on a Rust `u64` panics in debug instead. That is
///   what made `residual_recon_distortion_all_tiers_match_core` red.
#[inline]
fn sse_i32_core(a: &[i32], b: &[i32]) -> u64 {
    let mut d: u64 = 0;
    for (&x, &y) in a.iter().zip(b) {
        let e = (x as i64) - (y as i64);
        d = d.wrapping_add(e.wrapping_mul(e) as u64);
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
/// `vmlal_high_s32` give the widened product-accumulate directly, so each term
/// is an exact i64 product exactly as the scalar's is.
///
/// **Why the difference is still taken in i32 here, when the scalar widens
/// first.** `vsubl_s32` would give the exact `int64x2_t` difference, but there
/// is no i64xi64 multiply in NEON to square it with (`vmull_s32` widens
/// 32x32->64, which is the operation we already have via `vmlal_s32`), and
/// synthesising a 64x64 square from 32-bit halves costs more than the whole
/// kernel. So this arm keeps the cheap `vsubq_s32` and **detects** the only
/// case where it is not the C answer: an i32 subtraction wraps iff it differs
/// from the SATURATING one, so the loop ORs `vsubq_s32 ^ vqsubq_s32` into a
/// witness (three extra instructions per 4 lanes, no reduction in the loop)
/// and, if the witness is nonzero at the end, discards the vector result and
/// returns the exact scalar core. Fast path exact, slow path exact — no tier
/// silently wrapping.
///
/// **The witness's wall cost measured NULL** — `tools/perf_ab.sh`,
/// witness-removed vs witness, 9 interleaved paired rounds x 9 cells
/// (64/256/512 x preset 2/6/10), median cell ratio 0.9977 with every arm's
/// min/max band overlapping and all 9 cells byte-identical. Tagged: the box
/// was not quiet (a neighbouring agent's campaign resumed mid-run), so this
/// bounds the cost loosely rather than proving zero — a sub-1 % cost could
/// hide in the bands. `benchmarks/sse_i32_witness_ab_2026-08-11.tsv`.
///
/// Re-association: the scalar adds the squares left to right into one u64; this
/// adds them into four independent i64 lanes and sums the lanes at the end.
/// Two's-complement addition is a group operation mod 2^64, so re-association
/// is bit-exact even where the total wraps — and on the fast path every TERM is
/// exact (a widened i32xi32 product), which is what makes the two agree.
#[cfg(target_arch = "aarch64")]
#[arcane]
fn sse_i32_impl_neon(_token: NeonToken, a: &[i32], b: &[i32]) -> u64 {
    let n = a.len().min(b.len());
    let mut acc0 = vdupq_n_s64(0);
    let mut acc1 = vdupq_n_s64(0);
    let mut wrapped = vdupq_n_u32(0);
    let mut i = 0usize;
    while i + 4 <= n {
        let xa: &[i32; 4] = a[i..i + 4].try_into().unwrap();
        let ya: &[i32; 4] = b[i..i + 4].try_into().unwrap();
        let x = vld1q_s32(xa);
        let y = vld1q_s32(ya);
        let d = vsubq_s32(x, y);
        // Nonzero in any lane whose i32 difference wrapped.
        wrapped = vorrq_u32(
            wrapped,
            veorq_u32(
                vreinterpretq_u32_s32(d),
                vreinterpretq_u32_s32(vqsubq_s32(x, y)),
            ),
        );
        acc0 = vmlal_s32(acc0, vget_low_s32(d), vget_low_s32(d));
        acc1 = vmlal_high_s32(acc1, d, d);
        i += 4;
    }
    if vmaxvq_u32(wrapped) != 0 {
        return sse_i32_core(a, b);
    }
    let mut d = (vaddvq_s64(vaddq_s64(acc0, acc1))) as u64;
    for k in i..n {
        let e = (a[k] as i64) - (b[k] as i64);
        d = d.wrapping_add(e.wrapping_mul(e) as u64);
    }
    d
}

/// `sum_i a[i]^2` — the `eob == 0` arm of the frequency-domain distortion.
pub fn sq_sum_i32(a: &[i32]) -> u64 {
    incant!(sq_sum_i32_impl(a), [v3, neon, scalar])
}

/// The PREDICTION term of the same C kernel (`pic_operators.c:87`):
/// `prediction_distortion += (int64_t)SQR((int64_t)(coeff[i]));`. Widening one
/// `int32_t` cannot overflow, so only the accumulator width matters here — and
/// C's is a wrapping `uint64_t`, where Rust's `+=` panics in debug. Same
/// reasoning as [`sse_i32_core`].
#[inline]
fn sq_sum_i32_core(a: &[i32]) -> u64 {
    let mut d: u64 = 0;
    for &x in a {
        let e = x as i64;
        d = d.wrapping_add((e * e) as u64);
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
        d = d.wrapping_add((e * e) as u64);
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;
    use archmage::testing::{CompileTimePolicy, for_each_token_permutation};

    fn lcg(state: &mut u64) -> u64 {
        *state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
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
            (4usize, 4usize),
            (8, 8),
            (16, 16),
            (32, 32),
            (64, 64),
            (4, 16),
            (16, 4),
            (8, 32),
            (32, 8),
            (5, 3),
            (7, 9),
            (13, 2),
            (1, 1),
            (3, 1),
        ] {
            let sstride = w + 7;
            let pstride = w + 3;
            let src: Vec<u8> = (0..sstride * h + 32)
                .map(|_| (lcg(&mut st) & 0xff) as u8)
                .collect();
            let pred: Vec<u8> = (0..pstride * h + 32)
                .map(|_| (lcg(&mut st) & 0xff) as u8)
                .collect();
            // Residual
            let mut want = vec![0i32; w * h];
            residual_i32_core(&src, sstride, &pred, pstride, w, h, &mut want);
            let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_p| {
                let mut got = vec![0i32; w * h];
                residual_i32(&src, sstride, &pred, pstride, w, h, &mut got);
                assert_eq!(got, want, "residual {w}x{h}");
            });
            // i16 twin, same grid and the same tier sweep.
            let mut want16 = vec![0i16; w * h];
            residual_i16_core(&src, sstride, &pred, pstride, w, h, &mut want16);
            assert!(
                want16.iter().zip(&want).all(|(&a, &b)| i32::from(a) == b),
                "residual_i16 core disagrees with residual_i32 core at {w}x{h}"
            );
            let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_p| {
                let mut got = vec![0i16; w * h];
                residual_i16(&src, sstride, &pred, pstride, w, h, &mut got);
                assert_eq!(got, want16, "residual_i16 {w}x{h}");
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
            let ca: Vec<i32> = (0..w * h)
                .map(|_| (lcg(&mut st) as i32) >> ((lcg(&mut st) % 20) as u32))
                .collect();
            let cb: Vec<i32> = (0..w * h)
                .map(|_| (lcg(&mut st) as i32) >> (12 + (lcg(&mut st) % 8) as u32))
                .collect();
            let dwant = sse_i32_core(&ca, &cb);
            let swant = sq_sum_i32_core(&ca);
            let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_p| {
                assert_eq!(sse_i32(&ca, &cb), dwant, "sse_i32 {w}x{h}");
                assert_eq!(sq_sum_i32(&ca), swant, "sq_sum_i32 {w}x{h}");
            });
        }
    }

    /// C's residual term computed in `i128` and reduced mod 2^64 at the very
    /// end. It shares no code with the kernels under test: the arithmetic is
    /// exact and only the final reduction reproduces `uint64_t`'s wrap, so it
    /// pins BOTH of C's widths (`pic_operators.c:86`) independently of how the
    /// port spells them.
    fn sse_oracle_i128(a: &[i32], b: &[i32]) -> u64 {
        let mut acc: i128 = 0;
        for (&x, &y) in a.iter().zip(b) {
            let e = x as i128 - y as i128;
            acc += e * e;
        }
        (acc as u128 & u64::MAX as u128) as u64
    }

    /// The width bug this test exists to catch: subtract in i32 (wrapping),
    /// THEN widen. Present only so the test can PROVE its inputs discriminate
    /// the two forms — a case set where this agrees with the oracle would make
    /// the assertions below vacuous.
    fn sse_i32_subtract_then_widen(a: &[i32], b: &[i32]) -> u64 {
        let mut d: u64 = 0;
        for (&x, &y) in a.iter().zip(b) {
            let e = x.wrapping_sub(y) as i64;
            d = d.wrapping_add(e.wrapping_mul(e) as u64);
        }
        d
    }

    /// Every tier reproduces C's `int64_t` subtraction and wrapping `uint64_t`
    /// accumulator at inputs where a 32-bit subtraction would wrap.
    ///
    /// Measured 2026-08-11: this does NOT happen on a real encode (0 wraps in
    /// 59,088,480 elements; max |difference| 788 against an i32 ceiling of
    /// 2,147,483,647 — `benchmarks/sse_i32_width_2026-08-11.meta`). The gate
    /// is on the arithmetic contract, so that a future caller with a wider
    /// coefficient domain — 10/12-bit, a lossless path, an inter residual —
    /// cannot silently inherit a wrapped distortion.
    #[test]
    fn sse_i32_matches_c_widths_at_i32_extremes() {
        // Cases chosen so the wrap lands in the VECTOR body, in the SCALAR
        // tail, and in a length below one vector — the three places the NEON
        // arm treats differently.
        let cases: [(alloc::vec::Vec<i32>, alloc::vec::Vec<i32>); 6] = [
            // Widest possible difference (2^32 - 1); its square exceeds i64.
            (vec![i32::MAX; 8], vec![i32::MIN; 8]),
            (vec![i32::MIN; 8], vec![i32::MAX; 8]),
            // Wrap only in the 3-element tail of a 7-element slice. (The two
            // wrapped terms must not be symmetric: with `i32::MAX` in both
            // slots their mod-2^64 errors are +-2^33 and CANCEL, and the case
            // silently stops discriminating.)
            (
                vec![1, 2, 3, 4, i32::MAX, i32::MAX - 5, 6],
                vec![1, 0, 3, 0, -2, i32::MIN, 6],
            ),
            // Wrap only in the vector body; clean tail.
            (
                vec![i32::MIN, 5, -7, 9, 11, 13, 15],
                vec![i32::MAX, 1, 1, 1, 1, 1, 1],
            ),
            // Shorter than one vector.
            (vec![i32::MIN, 3], vec![7, 3]),
            // Accumulator wrap: 64 squares of ~2^62 sum past 2^64.
            (vec![i32::MIN; 64], vec![0; 64]),
        ];
        let mut discriminating = 0usize;
        for (i, (a, b)) in cases.iter().enumerate() {
            let want = sse_oracle_i128(a, b);
            if sse_i32_subtract_then_widen(a, b) != want {
                discriminating += 1;
            }
            assert_eq!(sse_i32_core(a, b), want, "scalar core, case {i}");
            let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_p| {
                assert_eq!(sse_i32(a, b), want, "sse_i32 case {i}");
            });
            // sq_sum's own accumulator must wrap like C's uint64_t too.
            let sq_want = {
                let mut acc: i128 = 0;
                for &x in a {
                    acc += (x as i128) * (x as i128);
                }
                (acc as u128 & u64::MAX as u128) as u64
            };
            assert_eq!(sq_sum_i32_core(a), sq_want, "sq_sum core, case {i}");
            let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_p| {
                assert_eq!(sq_sum_i32(a), sq_want, "sq_sum_i32 case {i}");
            });
        }
        // Anti-vacuity: without this the case set could be all small values and
        // every assertion above would pass on the broken kernel too.
        // Case 5 carries no wrap by construction (it exists for the
        // accumulator), so 5 of 6 is the maximum available here.
        assert_eq!(
            discriminating, 5,
            "expected 5 of 6 cases to distinguish an i32 subtraction from C's \
             i64 one — the gate has no teeth"
        );
    }
}
