use super::*;

/// C `svt_cfl_luma_subsampling_420_hbd_c` (intra_prediction.c:437-445): 2x2
/// luma downsample to Q3, high bit depth. Uses `intra_pred::CFL_BUF_LINE`
/// (`pub`, bd-independent stride constant) directly.
pub fn cfl_luma_subsampling_420_hbd(
    luma: &[u16],
    luma_stride: usize,
    output_q3: &mut [i16],
    width: usize,
    height: usize,
) {
    incant!(
        cfl_luma_subsampling_420_hbd_impl(luma, luma_stride, output_q3, width, height),
        [neon, scalar]
    )
}

pub(super) fn cfl_luma_subsampling_420_hbd_impl_scalar(
    _token: ScalarToken,
    luma: &[u16],
    luma_stride: usize,
    output_q3: &mut [i16],
    width: usize,
    height: usize,
) {
    cfl_luma_subsampling_420_hbd_core(luma, luma_stride, output_q3, width, height);
}

pub(super) fn cfl_luma_subsampling_420_hbd_core(
    luma: &[u16],
    luma_stride: usize,
    output_q3: &mut [i16],
    width: usize,
    height: usize,
) {
    for j in (0..height).step_by(2) {
        let out_row = (j / 2) * crate::intra_pred::CFL_BUF_LINE;
        for i in (0..width).step_by(2) {
            let sum = luma[j * luma_stride + i] as i32
                + luma[j * luma_stride + i + 1] as i32
                + luma[(j + 1) * luma_stride + i] as i32
                + luma[(j + 1) * luma_stride + i + 1] as i32;
            output_q3[out_row + i / 2] = (sum * 2) as i16;
        }
    }
}

/// aarch64 arm of [`cfl_luma_subsampling_420_hbd`]: `vpaddlq_u16` gives the
/// horizontal pair sums of each row in u32 lanes, the row add completes the
/// 2x2 sum, `<<1` is the `*2`, and `vmovn_u32` truncates mod 2^16 exactly
/// like the scalar `as i16` (reachable inputs keep `sum*2 <= 32760`, but the
/// wrap is preserved regardless). Two u16x8 loads cover 16 luma columns ->
/// 8 Q3 outputs per iteration.
#[cfg(target_arch = "aarch64")]
#[arcane]
pub(super) fn cfl_luma_subsampling_420_hbd_impl_neon(
    _token: NeonToken,
    luma: &[u16],
    luma_stride: usize,
    output_q3: &mut [i16],
    width: usize,
    height: usize,
) {
    for j in (0..height).step_by(2) {
        let out_row = (j / 2) * crate::intra_pred::CFL_BUF_LINE;
        let r0 = &luma[j * luma_stride..j * luma_stride + width];
        let r1 = &luma[(j + 1) * luma_stride..(j + 1) * luma_stride + width];
        let mut i = 0;
        while i + 16 <= width {
            let a: &[u16; 8] = r0[i..i + 8].try_into().unwrap();
            let b: &[u16; 8] = r0[i + 8..i + 16].try_into().unwrap();
            let c: &[u16; 8] = r1[i..i + 8].try_into().unwrap();
            let d: &[u16; 8] = r1[i + 8..i + 16].try_into().unwrap();
            let lo = vshlq_n_u32::<1>(vaddq_u32(
                vpaddlq_u16(vld1q_u16(a)),
                vpaddlq_u16(vld1q_u16(c)),
            ));
            let hi = vshlq_n_u32::<1>(vaddq_u32(
                vpaddlq_u16(vld1q_u16(b)),
                vpaddlq_u16(vld1q_u16(d)),
            ));
            let out: &mut [i16; 8] = (&mut output_q3[out_row + i / 2..out_row + i / 2 + 8])
                .try_into()
                .unwrap();
            vst1q_s16(
                out,
                vcombine_s16(
                    vreinterpret_s16_u16(vmovn_u32(lo)),
                    vreinterpret_s16_u16(vmovn_u32(hi)),
                ),
            );
            i += 16;
        }
        while i < width {
            let sum = r0[i] as i32 + r0[i + 1] as i32 + r1[i] as i32 + r1[i + 1] as i32;
            output_q3[out_row + i / 2] = (sum * 2) as i16;
            i += 2;
        }
    }
}

/// C `get_scaled_luma_q0` (C_DEFAULT/cfl_c.c:17-20) — shared verbatim by
/// C's lbd and hbd CfL predict; `ROUND_POWER_OF_TWO_SIGNED(alpha_q3 *
/// pred_buf_q3, 6)`.
#[inline]
pub(super) fn get_scaled_luma_q0_hbd(alpha_q3: i32, pred_buf_q3_val: i16) -> i32 {
    let scaled_luma_q6 = alpha_q3 * pred_buf_q3_val as i32;
    if scaled_luma_q6 < 0 {
        -((-scaled_luma_q6 + 32) >> 6)
    } else {
        (scaled_luma_q6 + 32) >> 6
    }
}

/// C `svt_cfl_predict_hbd_c` (C_DEFAULT/cfl_c.c:46-59).
#[allow(clippy::too_many_arguments)]
pub fn cfl_predict_hbd(
    pred_buf_q3: &[i16],
    pred: &[u16],
    pred_stride: usize,
    dst: &mut [u16],
    dst_stride: usize,
    alpha_q3: i32,
    bd: u8,
    width: usize,
    height: usize,
) {
    incant!(
        cfl_predict_hbd_impl(
            pred_buf_q3,
            pred,
            pred_stride,
            dst,
            dst_stride,
            alpha_q3,
            bd,
            width,
            height
        ),
        [neon, scalar]
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn cfl_predict_hbd_impl_scalar(
    _token: ScalarToken,
    pred_buf_q3: &[i16],
    pred: &[u16],
    pred_stride: usize,
    dst: &mut [u16],
    dst_stride: usize,
    alpha_q3: i32,
    bd: u8,
    width: usize,
    height: usize,
) {
    cfl_predict_hbd_core(
        pred_buf_q3,
        pred,
        pred_stride,
        dst,
        dst_stride,
        alpha_q3,
        bd,
        width,
        height,
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) fn cfl_predict_hbd_core(
    pred_buf_q3: &[i16],
    pred: &[u16],
    pred_stride: usize,
    dst: &mut [u16],
    dst_stride: usize,
    alpha_q3: i32,
    bd: u8,
    width: usize,
    height: usize,
) {
    for j in 0..height {
        for i in 0..width {
            let scaled = get_scaled_luma_q0_hbd(
                alpha_q3,
                pred_buf_q3[j * crate::intra_pred::CFL_BUF_LINE + i],
            );
            let val = scaled + pred[j * pred_stride + i] as i32;
            dst[j * dst_stride + i] = clip_pixel_highbd(val, bd);
        }
    }
}

/// aarch64 arm of [`cfl_predict_hbd`]. The scaled-luma term reuses the
/// lbd arm's `vqrdmulhq_s16` rounding (ac_q3 stays i16 at every bd), then
/// widens to i32 for the `+ pred` and the `[0, (1<<bd)-1]` clamp — the lbd
/// arm could stay in i16 (`pred <= 255`), this one cannot (`pred <= 65535`,
/// `sum` spans [-8192, 73727]).
#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
pub(super) fn cfl_predict_hbd_impl_neon(
    _token: NeonToken,
    pred_buf_q3: &[i16],
    pred: &[u16],
    pred_stride: usize,
    dst: &mut [u16],
    dst_stride: usize,
    alpha_q3: i32,
    bd: u8,
    width: usize,
    height: usize,
) {
    let q12 = vshlq_n_s16::<9>(vabsq_s16(vdupq_n_s16(alpha_q3 as i16)));
    let aneg = vdupq_n_s16(if alpha_q3 < 0 { -1 } else { 0 });
    let maxv = vdupq_n_s32(match bd {
        10 => 1023,
        12 => 4095,
        _ => 255, // C: `case 8: default:` grouped together
    });
    let zero = vdupq_n_s32(0);
    for j in 0..height {
        let acr = &pred_buf_q3
            [j * crate::intra_pred::CFL_BUF_LINE..j * crate::intra_pred::CFL_BUF_LINE + width];
        let pr = &pred[j * pred_stride..j * pred_stride + width];
        let or = &mut dst[j * dst_stride..j * dst_stride + width];
        let mut i = 0;
        while i + 8 <= width {
            let acb: &[i16; 8] = acr[i..i + 8].try_into().unwrap();
            let pb: &[u16; 8] = pr[i..i + 8].try_into().unwrap();
            let ac = vld1q_s16(acb);
            let m = veorq_s16(vshrq_n_s16::<15>(ac), aneg);
            let mag = vqrdmulhq_s16(vabsq_s16(ac), q12);
            let signed = vsubq_s16(veorq_s16(mag, m), m);
            let slo = vmovl_s16(vget_low_s16(signed));
            let shi = vmovl_s16(vget_high_s16(signed));
            let p = vld1q_u16(pb);
            let plo = vreinterpretq_s32_u32(vmovl_u16(vget_low_u16(p)));
            let phi = vreinterpretq_s32_u32(vmovl_u16(vget_high_u16(p)));
            let lo = vminq_s32(vmaxq_s32(vaddq_s32(slo, plo), zero), maxv);
            let hi = vminq_s32(vmaxq_s32(vaddq_s32(shi, phi), zero), maxv);
            let out: &mut [u16; 8] = (&mut or[i..i + 8]).try_into().unwrap();
            vst1q_u16(
                out,
                vcombine_u16(
                    vmovn_u32(vreinterpretq_u32_s32(lo)),
                    vmovn_u32(vreinterpretq_u32_s32(hi)),
                ),
            );
            i += 8;
        }
        while i < width {
            let scaled = get_scaled_luma_q0_hbd(alpha_q3, acr[i]);
            or[i] = clip_pixel_highbd(scaled + pr[i] as i32, bd);
            i += 1;
        }
    }
}

// =============================================================================
// 6. Distortion kernels: full_distortion_kernel16_bits, highbd_variance,
//    highbd_sad_kernel.
// C: pic_operators.c:100-123 (`svt_full_distortion_kernel16_bits_c`),
// C_DEFAULT/variance.c:162-181 (`svt_aom_variance_highbd_c`),
// C_DEFAULT/compute_sad_c.c:42-61 (`svt_aom_sad_16b_kernel_c`). All three
// are already GENERIC (W, H) forms in C — no sized-wrapper family to
// enumerate for these (unlike variance, below).
//
// FFI-VERIFIED: tests/c_parity_hbd_distortion.rs pins all three below
// (full_distortion_kernel16_bits / highbd_variance / highbd_sad_kernel)
// against the real exported svt_full_distortion_kernel16_bits_c /
// svt_aom_variance_highbd_c / svt_aom_sad_16b_kernel_c at bd10 + bd12 over 14
// block shapes on strided buffers (offset origin + padded stride), so the
// offset/stride/overflow marshalling is exercised, not just tight-packed.
// =============================================================================

/// C `svt_full_distortion_kernel16_bits_c` (pic_operators.c:100-123): SSE
/// between two 16-bit planes over an `area_width x area_height` window. C
/// reinterprets `uint8_t* input`/`pred` as `uint16_t*` and applies
/// `input_offset`/`pred_offset` AFTER that cast; this port takes `&[u16]`
/// planes directly per docs/bd10-port-map.md ("PORT: use plain u16 planes;
/// never implement 8+2") — offsets here are plain u16-element indices, not
/// byte offsets.
pub fn full_distortion_kernel16_bits(
    input: &[u16],
    input_offset: usize,
    input_stride: usize,
    pred: &[u16],
    pred_offset: usize,
    pred_stride: usize,
    area_width: usize,
    area_height: usize,
) -> u64 {
    let mut sse_distortion: u64 = 0;
    for row in 0..area_height {
        let in_row = input_offset + row * input_stride;
        let pred_row = pred_offset + row * pred_stride;
        for col in 0..area_width {
            let diff = input[in_row + col] as i64 - pred[pred_row + col] as i64;
            sse_distortion += (diff * diff) as u64;
        }
    }
    sse_distortion
}

/// C `svt_aom_variance_highbd_c` (C_DEFAULT/variance.c): generic,
/// unnormalized high-bit-depth variance. This is NOT interchangeable with
/// `svt_aom_highbd_10_variance{W}x{H}_c` (Codec/svt_psnr.c), which rounds
/// SSE and the signed residual sum to the 8-bit scale first.
///
/// C accumulates BOTH `sad` (as `int`) and `*sse` (as `uint32_t`, via `+=
/// diff*diff` where `diff*diff` is `int` arithmetic) — `sad` cannot
/// overflow `i32` for any legal AV1 block (max magnitude ~4095 * 16384 ≈
/// 67M), but `*sse` genuinely CAN wrap `u32` for large, fully-saturated
/// high-bit-depth blocks (max ~4095² * 16384 ≈ 2.75e11 » u32::MAX) — C's
/// `uint32_t` accumulation is well-defined modular arithmetic, not UB, so
/// this port mirrors the wraparound exactly via `wrapping_add` rather than
/// widening to a type that would silently disagree with C on overflow.
pub fn highbd_variance(
    a: &[u16],
    a_stride: usize,
    b: &[u16],
    b_stride: usize,
    w: usize,
    h: usize,
) -> (u32, u32) {
    let mut sad: i32 = 0;
    let mut sse: u32 = 0;
    for row in 0..h {
        for col in 0..w {
            let diff = a[row * a_stride + col] as i32 - b[row * b_stride + col] as i32;
            sad = sad.wrapping_add(diff);
            sse = sse.wrapping_add((diff * diff) as u32);
        }
    }
    let variance = sse as i64 - (sad as i64 * sad as i64) / (w * h) as i64;
    (sse, variance as u32)
}

/// C `svt_aom_sad_16b_kernel_c` (C_DEFAULT/compute_sad_c.c:42-61): generic
/// WxH SAD for two 16-bit planes.
///
/// C's parameter order is `(src, src_stride, ref, ref_stride, height,
/// width)` — **height before width**, unlike this crate's u8 `sad()`
/// convention (`sad.rs`, width before height). This port uses the
/// crate-house `(width, height)` order per task instruction to mirror
/// house conventions; callers wiring this to C's `svt_aom_sad_16b_kernel`
/// call sites MUST swap the last two arguments.
pub fn highbd_sad_kernel(
    src: &[u16],
    src_stride: usize,
    ref_: &[u16],
    ref_stride: usize,
    width: usize,
    height: usize,
) -> u32 {
    let mut sad: u32 = 0;
    for row in 0..height {
        let src_row = row * src_stride;
        let ref_row = row * ref_stride;
        for col in 0..width {
            let s = src[src_row + col] as i32;
            let r = ref_[ref_row + col] as i32;
            sad += (s - r).unsigned_abs();
        }
    }
    sad
}

// =============================================================================
// 7. Deblock (loop_filter), highbd.
// C: deblocking_common.c — masks at lines 194-203 (`highbd_flat_mask3_
// chroma`), 389-437 (`highbd_filter_mask2`, `highbd_filter_mask`,
// `highbd_flat_mask4`, `highbd_hev_mask`), 679-690 (`highbd_filter_mask3_
// chroma`); filters at 439-471 (`highbd_filter4`), 507-524 (`highbd_
// filter8`), 591-627 (`highbd_filter14`), 692-706 (`highbd_filter6`); the
// public kernels at 473-505 / 526-558 / 674-784 (`svt_aom_highbd_lpf_
// {horizontal,vertical}_{4,6,8,14}_c`). `signed_char_clamp_high`:
// deblocking_common.c:34-43.
//
// A key semantic difference from the lbd kernels (`loop_filter.rs`), NOT
// just a type change: `blimit`/`limit`/`thresh` stay `uint8_t` even at
// hbd, but the masks/hev check shift them left by `bd - 8` before
// comparing against real 16-bit sample differences (C: `int16_t limit16 =
// (uint16_t)limit << (bd - 8);` etc.) — ported below via an explicit
// `shift` in every mask/hev helper. `LfThresh`/`lf_thresholds` from
// `loop_filter.rs` (both `pub`, bd-independent — `mblim`/`lim`/`hev_thr`
// derive from `level`/`sharpness` only) are reused directly as the
// threshold carrier type; no hbd variant needed for those two.
//
// FFI-VERIFIED: tests/c_parity_lpf_hbd.rs pins all 8 public lpf_*_hbd entry
// points (and thus the shared mask/hev/filter helpers below) against the real
// exported svt_aom_highbd_lpf_*_c at bd10 AND bd12, over the full
// (level, sharpness) space and random params — the bd-shift twin of the bd8
// tests/c_parity_lpf.rs.
// =============================================================================

/// C `signed_char_clamp_high` (deblocking_common.c:34-43). C's return type
/// is `int16_t`; kept as `i32` here purely for arithmetic convenience in
/// the surrounding i32 expressions (the clamp ranges below always fit i16).
#[inline]
pub(super) fn signed_char_clamp_high(t: i32, bd: u8) -> i32 {
    match bd {
        10 => t.clamp(-512, 511),
        12 => t.clamp(-2048, 2047),
        _ => t.clamp(-128, 127),
    }
}

/// C `highbd_hev_mask` (deblocking_common.c:431-437).
#[inline]
pub(super) fn highbd_hev_mask(thresh: u8, p1: u16, p0: u16, q0: u16, q1: u16, bd: u8) -> i16 {
    let thresh16 = (thresh as i32) << (bd as i32 - 8);
    let mut hev: i16 = 0;
    hev |= -((((p1 as i32) - (p0 as i32)).abs() > thresh16) as i16);
    hev |= -((((q1 as i32) - (q0 as i32)).abs() > thresh16) as i16);
    hev
}

/// C `highbd_filter_mask2` (deblocking_common.c:389-398) for 4-tap edges.
#[inline]
pub(super) fn highbd_filter_mask2(
    limit: u8,
    blimit: u8,
    p1: u16,
    p0: u16,
    q0: u16,
    q1: u16,
    bd: u8,
) -> i8 {
    let shift = bd as i32 - 8;
    let (limit16, blimit16) = ((limit as i32) << shift, (blimit as i32) << shift);
    let (p1, p0, q0, q1) = (p1 as i32, p0 as i32, q0 as i32, q1 as i32);
    let mut mask: i8 = 0;
    mask |= -(((p1 - p0).abs() > limit16) as i8);
    mask |= -(((q1 - q0).abs() > limit16) as i8);
    mask |= -(((p0 - q0).abs() * 2 + (p1 - q1).abs() / 2 > blimit16) as i8);
    !mask
}

/// C `highbd_filter_mask` (deblocking_common.c:401-414) for 8-tap edges.
#[inline]
#[allow(clippy::too_many_arguments)]
pub(super) fn highbd_filter_mask(
    limit: u8,
    blimit: u8,
    p3: u16,
    p2: u16,
    p1: u16,
    p0: u16,
    q0: u16,
    q1: u16,
    q2: u16,
    q3: u16,
    bd: u8,
) -> i8 {
    let shift = bd as i32 - 8;
    let (limit16, blimit16) = ((limit as i32) << shift, (blimit as i32) << shift);
    let (p3, p2, p1, p0) = (p3 as i32, p2 as i32, p1 as i32, p0 as i32);
    let (q0, q1, q2, q3) = (q0 as i32, q1 as i32, q2 as i32, q3 as i32);
    let mut mask: i8 = 0;
    mask |= -(((p3 - p2).abs() > limit16) as i8);
    mask |= -(((p2 - p1).abs() > limit16) as i8);
    mask |= -(((p1 - p0).abs() > limit16) as i8);
    mask |= -(((q1 - q0).abs() > limit16) as i8);
    mask |= -(((q2 - q1).abs() > limit16) as i8);
    mask |= -(((q3 - q2).abs() > limit16) as i8);
    mask |= -(((p0 - q0).abs() * 2 + (p1 - q1).abs() / 2 > blimit16) as i8);
    !mask
}

/// C `highbd_filter_mask3_chroma` (deblocking_common.c:679-690) for 6-tap edges.
#[inline]
#[allow(clippy::too_many_arguments)]
pub(super) fn highbd_filter_mask3_chroma(
    limit: u8,
    blimit: u8,
    p2: u16,
    p1: u16,
    p0: u16,
    q0: u16,
    q1: u16,
    q2: u16,
    bd: u8,
) -> i8 {
    let shift = bd as i32 - 8;
    let (limit16, blimit16) = ((limit as i32) << shift, (blimit as i32) << shift);
    let (p2, p1, p0) = (p2 as i32, p1 as i32, p0 as i32);
    let (q0, q1, q2) = (q0 as i32, q1 as i32, q2 as i32);
    let mut mask: i8 = 0;
    mask |= -(((p2 - p1).abs() > limit16) as i8);
    mask |= -(((p1 - p0).abs() > limit16) as i8);
    mask |= -(((q1 - q0).abs() > limit16) as i8);
    mask |= -(((q2 - q1).abs() > limit16) as i8);
    mask |= -(((p0 - q0).abs() * 2 + (p1 - q1).abs() / 2 > blimit16) as i8);
    !mask
}

/// C `highbd_flat_mask3_chroma` (deblocking_common.c:194-203).
#[inline]
pub(super) fn highbd_flat_mask3_chroma(
    thresh: u8,
    p2: u16,
    p1: u16,
    p0: u16,
    q0: u16,
    q1: u16,
    q2: u16,
    bd: u8,
) -> i8 {
    let thresh16 = (thresh as i32) << (bd as i32 - 8);
    let (p2, p1, p0) = (p2 as i32, p1 as i32, p0 as i32);
    let (q0, q1, q2) = (q0 as i32, q1 as i32, q2 as i32);
    let mut mask: i8 = 0;
    mask |= -(((p1 - p0).abs() > thresh16) as i8);
    mask |= -(((q1 - q0).abs() > thresh16) as i8);
    mask |= -(((p2 - p0).abs() > thresh16) as i8);
    mask |= -(((q2 - q0).abs() > thresh16) as i8);
    !mask
}

/// C `highbd_flat_mask4` (deblocking_common.c:416-427) — also reused by C
/// (and here) for the wider "flat2" check in the 14-tap filter.
#[inline]
#[allow(clippy::too_many_arguments)]
pub(super) fn highbd_flat_mask4(
    thresh: u8,
    p3: u16,
    p2: u16,
    p1: u16,
    p0: u16,
    q0: u16,
    q1: u16,
    q2: u16,
    q3: u16,
    bd: u8,
) -> i8 {
    let thresh16 = (thresh as i32) << (bd as i32 - 8);
    let (p3, p2, p1, p0) = (p3 as i32, p2 as i32, p1 as i32, p0 as i32);
    let (q0, q1, q2, q3) = (q0 as i32, q1 as i32, q2 as i32, q3 as i32);
    let mut mask: i8 = 0;
    mask |= -(((p1 - p0).abs() > thresh16) as i8);
    mask |= -(((q1 - q0).abs() > thresh16) as i8);
    mask |= -(((p2 - p0).abs() > thresh16) as i8);
    mask |= -(((q2 - q0).abs() > thresh16) as i8);
    mask |= -(((p3 - p0).abs() > thresh16) as i8);
    mask |= -(((q3 - q0).abs() > thresh16) as i8);
    !mask
}

/// C `highbd_filter4` (deblocking_common.c:439-471) on a `[p1, p0, q0,
/// q1]` window. The lbd sibling's `^0x80` sign-flip trick becomes a
/// `bd`-scaled bias (`0x80 << (bd - 8)`); at bd8 `bias == 0x80`, so this
/// reduces to the same arithmetic as `loop_filter::filter4_line`.
pub(super) fn highbd_filter4(mask: i8, thresh: u8, w: &mut [u16; 4], bd: u8) {
    let shift = bd as i32 - 8;
    let bias = 0x80i32 << shift;
    let ps1 = w[0] as i32 - bias;
    let ps0 = w[1] as i32 - bias;
    let qs0 = w[2] as i32 - bias;
    let qs1 = w[3] as i32 - bias;
    let hev = highbd_hev_mask(thresh, w[0], w[1], w[2], w[3], bd) as i32;

    let mut filter = signed_char_clamp_high(ps1 - qs1, bd) & hev;
    filter = signed_char_clamp_high(filter + 3 * (qs0 - ps0), bd) & (mask as i32);

    let filter1 = signed_char_clamp_high(filter + 4, bd) >> 3;
    let filter2 = signed_char_clamp_high(filter + 3, bd) >> 3;

    w[2] = (signed_char_clamp_high(qs0 - filter1, bd) + bias) as u16;
    w[1] = (signed_char_clamp_high(ps0 + filter2, bd) + bias) as u16;

    let outer = ((filter1 + 1) >> 1) & !hev;
    w[3] = (signed_char_clamp_high(qs1 - outer, bd) + bias) as u16;
    w[0] = (signed_char_clamp_high(ps1 + outer, bd) + bias) as u16;
}

/// C `ROUND_POWER_OF_TWO(x, 3)` for the flat-filter taps (non-negative),
/// widened to u16 output.
#[inline]
pub(super) fn rpot3_hbd(x: i32) -> u16 {
    ((x + 4) >> 3) as u16
}

/// C `ROUND_POWER_OF_TWO(x, 4)`, widened to u16 output.
#[inline]
pub(super) fn rpot4_hbd(x: i32) -> u16 {
    ((x + 8) >> 4) as u16
}

/// C `highbd_filter6` (deblocking_common.c:692-706) on a `[p2, p1, p0, q0,
/// q1, q2]` window.
pub(super) fn highbd_filter6(mask: i8, thresh: u8, flat: i8, w: &mut [u16; 6], bd: u8) {
    if flat != 0 && mask != 0 {
        let (p2, p1, p0) = (w[0] as i32, w[1] as i32, w[2] as i32);
        let (q0, q1, q2) = (w[3] as i32, w[4] as i32, w[5] as i32);
        w[1] = rpot3_hbd(p2 * 3 + p1 * 2 + p0 * 2 + q0);
        w[2] = rpot3_hbd(p2 + p1 * 2 + p0 * 2 + q0 * 2 + q1);
        w[3] = rpot3_hbd(p1 + p0 * 2 + q0 * 2 + q1 * 2 + q2);
        w[4] = rpot3_hbd(p0 + q0 * 2 + q1 * 2 + q2 * 3);
    } else {
        let mut inner = [w[1], w[2], w[3], w[4]];
        highbd_filter4(mask, thresh, &mut inner, bd);
        [w[1], w[2], w[3], w[4]] = inner;
    }
}

/// C `highbd_filter8` (deblocking_common.c:507-524) on a `[p3..p0,
/// q0..q3]` window.
pub(super) fn highbd_filter8(mask: i8, thresh: u8, flat: i8, w: &mut [u16; 8], bd: u8) {
    if flat != 0 && mask != 0 {
        let (p3, p2, p1, p0) = (w[0] as i32, w[1] as i32, w[2] as i32, w[3] as i32);
        let (q0, q1, q2, q3) = (w[4] as i32, w[5] as i32, w[6] as i32, w[7] as i32);
        w[1] = rpot3_hbd(p3 + p3 + p3 + 2 * p2 + p1 + p0 + q0);
        w[2] = rpot3_hbd(p3 + p3 + p2 + 2 * p1 + p0 + q0 + q1);
        w[3] = rpot3_hbd(p3 + p2 + p1 + 2 * p0 + q0 + q1 + q2);
        w[4] = rpot3_hbd(p2 + p1 + p0 + 2 * q0 + q1 + q2 + q3);
        w[5] = rpot3_hbd(p1 + p0 + q0 + 2 * q1 + q2 + q3 + q3);
        w[6] = rpot3_hbd(p0 + q0 + q1 + 2 * q2 + q3 + q3 + q3);
    } else {
        let mut inner = [w[2], w[3], w[4], w[5]];
        highbd_filter4(mask, thresh, &mut inner, bd);
        [w[2], w[3], w[4], w[5]] = inner;
    }
}

/// C `highbd_filter14` (deblocking_common.c:591-627) on a `[p6..p0,
/// q0..q6]` window.
pub(super) fn highbd_filter14(
    mask: i8,
    thresh: u8,
    flat: i8,
    flat2: i8,
    w: &mut [u16; 14],
    bd: u8,
) {
    if flat2 != 0 && flat != 0 && mask != 0 {
        let (p6, p5, p4, p3) = (w[0] as i32, w[1] as i32, w[2] as i32, w[3] as i32);
        let (p2, p1, p0) = (w[4] as i32, w[5] as i32, w[6] as i32);
        let (q0, q1, q2, q3) = (w[7] as i32, w[8] as i32, w[9] as i32, w[10] as i32);
        let (q4, q5, q6) = (w[11] as i32, w[12] as i32, w[13] as i32);
        w[1] = rpot4_hbd(p6 * 7 + p5 * 2 + p4 * 2 + p3 + p2 + p1 + p0 + q0);
        w[2] = rpot4_hbd(p6 * 5 + p5 * 2 + p4 * 2 + p3 * 2 + p2 + p1 + p0 + q0 + q1);
        w[3] = rpot4_hbd(p6 * 4 + p5 + p4 * 2 + p3 * 2 + p2 * 2 + p1 + p0 + q0 + q1 + q2);
        w[4] = rpot4_hbd(p6 * 3 + p5 + p4 + p3 * 2 + p2 * 2 + p1 * 2 + p0 + q0 + q1 + q2 + q3);
        w[5] = rpot4_hbd(p6 * 2 + p5 + p4 + p3 + p2 * 2 + p1 * 2 + p0 * 2 + q0 + q1 + q2 + q3 + q4);
        w[6] =
            rpot4_hbd(p6 + p5 + p4 + p3 + p2 + p1 * 2 + p0 * 2 + q0 * 2 + q1 + q2 + q3 + q4 + q5);
        w[7] =
            rpot4_hbd(p5 + p4 + p3 + p2 + p1 + p0 * 2 + q0 * 2 + q1 * 2 + q2 + q3 + q4 + q5 + q6);
        w[8] = rpot4_hbd(p4 + p3 + p2 + p1 + p0 + q0 * 2 + q1 * 2 + q2 * 2 + q3 + q4 + q5 + q6 * 2);
        w[9] = rpot4_hbd(p3 + p2 + p1 + p0 + q0 + q1 * 2 + q2 * 2 + q3 * 2 + q4 + q5 + q6 * 3);
        w[10] = rpot4_hbd(p2 + p1 + p0 + q0 + q1 + q2 * 2 + q3 * 2 + q4 * 2 + q5 + q6 * 4);
        w[11] = rpot4_hbd(p1 + p0 + q0 + q1 + q2 + q3 * 2 + q4 * 2 + q5 * 2 + q6 * 5);
        w[12] = rpot4_hbd(p0 + q0 + q1 + q2 + q3 + q4 * 2 + q5 * 2 + q6 * 7);
    } else {
        let mut inner = [w[3], w[4], w[5], w[6], w[7], w[8], w[9], w[10]];
        highbd_filter8(mask, thresh, flat, &mut inner, bd);
        [w[3], w[4], w[5], w[6], w[7], w[8], w[9], w[10]] = inner;
    }
}

/// Gather `N` u16 samples centered on the edge at `base` with step `step`
/// — u16 analogue of `loop_filter`'s private `gather` (not `pub`, hence
/// duplicated).
#[inline]
pub(super) fn gather16<const N: usize>(buf: &[u16], base: usize, step: usize) -> [u16; N] {
    let mut w = [0u16; N];
    let start = base - (N / 2) * step;
    for (k, s) in w.iter_mut().enumerate() {
        *s = buf[start + k * step];
    }
    w
}

/// Scatter the window back (inverse of [`gather16`]).
#[inline]
pub(super) fn scatter16<const N: usize>(buf: &mut [u16], base: usize, step: usize, w: &[u16; N]) {
    let start = base - (N / 2) * step;
    for (k, s) in w.iter().enumerate() {
        buf[start + k * step] = *s;
    }
}

/// C `svt_aom_highbd_lpf_horizontal_4_c` (deblocking_common.c:473-489).
pub fn lpf_horizontal_4_hbd(buf: &mut [u16], off: usize, pitch: usize, t: LfThresh, bd: u8) {
    for i in 0..4 {
        let base = off + i;
        let mut w: [u16; 4] = gather16(buf, base, pitch);
        let mask = highbd_filter_mask2(t.lim, t.mblim, w[0], w[1], w[2], w[3], bd);
        highbd_filter4(mask, t.hev_thr, &mut w, bd);
        scatter16(buf, base, pitch, &w);
    }
}

/// C `svt_aom_highbd_lpf_vertical_4_c` (deblocking_common.c:491-505).
pub fn lpf_vertical_4_hbd(buf: &mut [u16], off: usize, pitch: usize, t: LfThresh, bd: u8) {
    for i in 0..4 {
        let base = off + i * pitch;
        let mut w: [u16; 4] = gather16(buf, base, 1);
        let mask = highbd_filter_mask2(t.lim, t.mblim, w[0], w[1], w[2], w[3], bd);
        highbd_filter4(mask, t.hev_thr, &mut w, bd);
        scatter16(buf, base, 1, &w);
    }
}

/// C `svt_aom_highbd_lpf_horizontal_6_c` (deblocking_common.c:723-739).
pub fn lpf_horizontal_6_hbd(buf: &mut [u16], off: usize, pitch: usize, t: LfThresh, bd: u8) {
    for i in 0..4 {
        let base = off + i;
        let mut w: [u16; 6] = gather16(buf, base, pitch);
        let mask =
            highbd_filter_mask3_chroma(t.lim, t.mblim, w[0], w[1], w[2], w[3], w[4], w[5], bd);
        let flat = highbd_flat_mask3_chroma(1, w[0], w[1], w[2], w[3], w[4], w[5], bd);
        highbd_filter6(mask, t.hev_thr, flat, &mut w, bd);
        scatter16(buf, base, pitch, &w);
    }
}

/// C `svt_aom_highbd_lpf_vertical_6_c` (deblocking_common.c:708-721).
pub fn lpf_vertical_6_hbd(buf: &mut [u16], off: usize, pitch: usize, t: LfThresh, bd: u8) {
    for i in 0..4 {
        let base = off + i * pitch;
        let mut w: [u16; 6] = gather16(buf, base, 1);
        let mask =
            highbd_filter_mask3_chroma(t.lim, t.mblim, w[0], w[1], w[2], w[3], w[4], w[5], bd);
        let flat = highbd_flat_mask3_chroma(1, w[0], w[1], w[2], w[3], w[4], w[5], bd);
        highbd_filter6(mask, t.hev_thr, flat, &mut w, bd);
        scatter16(buf, base, 1, &w);
    }
}

/// C `svt_aom_highbd_lpf_horizontal_8_c` (deblocking_common.c:526-543).
pub fn lpf_horizontal_8_hbd(buf: &mut [u16], off: usize, pitch: usize, t: LfThresh, bd: u8) {
    for i in 0..4 {
        let base = off + i;
        let mut w: [u16; 8] = gather16(buf, base, pitch);
        let mask = highbd_filter_mask(
            t.lim, t.mblim, w[0], w[1], w[2], w[3], w[4], w[5], w[6], w[7], bd,
        );
        let flat = highbd_flat_mask4(1, w[0], w[1], w[2], w[3], w[4], w[5], w[6], w[7], bd);
        highbd_filter8(mask, t.hev_thr, flat, &mut w, bd);
        scatter16(buf, base, pitch, &w);
    }
}

/// C `svt_aom_highbd_lpf_vertical_8_c` (deblocking_common.c:545-558).
pub fn lpf_vertical_8_hbd(buf: &mut [u16], off: usize, pitch: usize, t: LfThresh, bd: u8) {
    for i in 0..4 {
        let base = off + i * pitch;
        let mut w: [u16; 8] = gather16(buf, base, 1);
        let mask = highbd_filter_mask(
            t.lim, t.mblim, w[0], w[1], w[2], w[3], w[4], w[5], w[6], w[7], bd,
        );
        let flat = highbd_flat_mask4(1, w[0], w[1], w[2], w[3], w[4], w[5], w[6], w[7], bd);
        highbd_filter8(mask, t.hev_thr, flat, &mut w, bd);
        scatter16(buf, base, 1, &w);
    }
}

/// Shared 14-tap body on a gathered `[p6..q6]` window. C
/// `highbd_mb_lpf_horizontal_edge_w` / `highbd_mb_lpf_vertical_edge_w`
/// inner loop (deblocking_common.c:629-672 / 741-780).
pub(super) fn lpf14_window_hbd(w: &mut [u16; 14], t: LfThresh, bd: u8) {
    let mask = highbd_filter_mask(
        t.lim, t.mblim, w[3], w[4], w[5], w[6], w[7], w[8], w[9], w[10], bd,
    );
    let flat = highbd_flat_mask4(1, w[3], w[4], w[5], w[6], w[7], w[8], w[9], w[10], bd);
    let flat2 = highbd_flat_mask4(1, w[0], w[1], w[2], w[6], w[7], w[11], w[12], w[13], bd);
    highbd_filter14(mask, t.hev_thr, flat, flat2, w, bd);
}

/// C `svt_aom_highbd_lpf_horizontal_14_c` (deblocking_common.c:674-677).
pub fn lpf_horizontal_14_hbd(buf: &mut [u16], off: usize, pitch: usize, t: LfThresh, bd: u8) {
    for i in 0..4 {
        let base = off + i;
        let mut w: [u16; 14] = gather16(buf, base, pitch);
        lpf14_window_hbd(&mut w, t, bd);
        scatter16(buf, base, pitch, &w);
    }
}

/// C `svt_aom_highbd_lpf_vertical_14_c` (deblocking_common.c:781-784).
pub fn lpf_vertical_14_hbd(buf: &mut [u16], off: usize, pitch: usize, t: LfThresh, bd: u8) {
    for i in 0..4 {
        let base = off + i * pitch;
        let mut w: [u16; 14] = gather16(buf, base, 1);
        lpf14_window_hbd(&mut w, t, bd);
        scatter16(buf, base, 1, &w);
    }
}

// =============================================================================
// 8. CDEF, highbd store variant.
//
// Per docs/bd10-port-map.md: "CDEF: dir search u16-native already; filter
// dst8/dst16 dual out." Verified directly against `svt_cdef_filter_block_c`
// (cdef.c:193-254): the function computes the SAME `y` value regardless of
// bit depth (its taps/constrain/min-max clamp operate on the ALREADY-u16
// intermediate buffer for both bd8 and bd10 sources — CDEF's padded working
// buffer is u16 even in the 8-bit pipeline), and only branches at the very
// last line on which output array to store into:
// ```c
// if (dst8) { dst8[...] = (uint8_t)y; } else { dst16[...] = (uint16_t)y; }
// ```
// So there is ZERO new arithmetic for hbd CDEF — [`cdef_filter_block_hbd`]
// below is a byte-for-byte duplicate of `crate::cdef::cdef_filter_block`'s
// loop body (that fn hardcodes the `dst8` arm; its private helpers
// `constrain`/`cdef_direction`/`CDEF_PRI_TAPS`/`CDEF_SEC_TAPS` are not
// `pub`, hence re-declared here rather than shared — editing cdef.rs's
// visibility is out of this task's scope), with the store retargeted to
// `&mut [u16]` (C's `dst16` arm). `crate::cdef::cdef_find_dir` needs NO hbd
// counterpart at all — it already takes `&[u16]` and is bit-depth-generic
// as-is; reuse it directly, zero new code.
//
// VERIFIED (2026-07-19): `tests/c_parity_cdef.rs::filter_block_hbd_dst16
// _matches_c` drives the REAL `svt_cdef_filter_block_c` through its dst16 arm
// (`ref_cdef_filter_block_16`) over 3000 randomized rounds — 10-bit content,
// every border pattern, all four block shapes, raw-u16 torture, sub=1/2, and
// the coeff_shift=2 domain oversampled 4:1 (that being the only domain this
// arm is reachable in). The former PORT-NOTE(unverified) is discharged; the
// kernel is now load-bearing for the bd10 CDEF strength search.
// =============================================================================
