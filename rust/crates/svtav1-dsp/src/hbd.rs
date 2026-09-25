//! 10-bit (bd10) DSP kernel layer — bulk source translation (task #94).
//!
//! COMPILED (wired at `lib.rs`) and FFI-PARITY-VERIFIED bit-for-bit against real
//! C at bd10+bd12 (`c_parity_*_hbd` / `c_parity_bd10_quant`). Most kernels are
//! now WIRED into the live bd10 pipeline: loop filters (`deblock.rs`), CDEF
//! (`cdef.rs`), distortion (`full_distortion_kernel16_*`), CfL (`cfl_*_hbd`) and
//! the intra predictors are all invoked from production (the bd10 full-RD funnel
//! + post-pass, docs/bd10-port-map.md). Only the `dc/ac_qlookup_10/_12` arms
//!   below stay unwired — the real bd10 quant tables live in the encoder crate
//!   (`svtav1_encoder::bd10`), so these copies are dead by crate-dedup, not a gap.
//!
//! Translated per `docs/bd10-port-map.md` (the spec — plain `u16` pixel
//! planes everywhere; the C 8+2 unpacked-plane split is an *input ingestion
//! memory layout* detail this crate never implements). Every function below
//! mirrors the parameter shape of its existing 8-bit sibling in this crate
//! (`intra_pred.rs` / `loop_filter.rs` / `cdef.rs` / `quant.rs`) — explicit
//! `width`/`height` (never `TxSize`), `dst_stride`/`*_stride` in elements,
//! plus a trailing `bd: u8` wherever C threads `bd`/`bit_depth`.
//!
//! # BULK-PORT MODE status
//!
//! Per CLAUDE.md: every function carries a `PORT-NOTE(unverified)` marker.
//! **Verification plan (deferred, not run by this translation pass):** FFI
//! parity tests against the real exported C symbols (mirroring
//! `tests/c_parity_lpf.rs` / `tests/c_parity_cdef.rs`), then the bd10
//! uniform-64×64 single-partition identity cell described in the port map's
//! milestone note. Delete each marker in the commit that adds its evidence.
//!
//! # Scope per docs/bd10-port-map.md item-by-item
//!
//! 1. Highbd intra predictors — DC family, V, H, Paeth, smooth family,
//!    directional z1/z2/z3 (with edge upsample), filter-intra, CfL 420 +
//!    predict.
//! 2. Recon add + clip — `highbd_clip_pixel_add` (`check_range` /
//!    `HIGHBD_WRAPLOW` chain).
//! 3. Distortion — `full_distortion_kernel16_bits` (generic W×H),
//!    `highbd_variance` (generic W×H), `highbd_sad_kernel` (generic W×H).
//! 4. Deblock — highbd `lpf_{horizontal,vertical}_{4,6,8,14}`.
//! 5. CDEF — see the CDEF section doc: zero new arithmetic, one new
//!    store-type (`u16`) variant of the existing `dst8`/`dst16` dual-out
//!    filter body.
//! 6. Quant — `dc_quant_qtx`/`ac_quant_qtx` bit-depth switch shape. The
//!    256-entry bd10/bd12 table *values* are intentionally NOT transcribed
//!    (pending `xtask/transcribe_bd10_qlookup.py`, not run by this pass —
//!    mirrors the existing placeholder pattern in
//!    `svtav1_encoder::bd10::{dc_qlookup_10, ac_qlookup_10}`). The zbin
//!    factor (`svt_aom_get_qzbin_factor`) is cross-referenced, not
//!    duplicated — see the quant section doc for a correctness finding
//!    about that sibling function.
//!
//! # Findings from this translation pass (not fixed here — out of scope)
//!
//! - **Sibling correctness finding, logged as Known Bug KB-13 in
//!   project CLAUDE.md:** `intra_pred::predict_paeth_core`'s tie-break
//!   order (`if p_top <= p_left && p_top <= p_tl`) does not match the real
//!   C `paeth_predictor_single` (intra_prediction.c:1226-1234, shared by
//!   BOTH the lbd and hbd paeth predictors), which checks `p_left` first:
//!   `(p_left <= p_top && p_left <= p_top_left) ? left : (p_top <=
//!   p_top_left) ? top : top_left`. The two orders disagree exactly when
//!   `p_top == p_left` (both the minimum) — a real, if infrequent,
//!   byte-exactness bug in the already-wired u8 path. This module's
//!   [`predict_paeth_hbd`] is translated directly from the real C order
//!   (left-first), so it does NOT reproduce that bug.
//! - `svt_av1_highbd_dr_prediction_z2_c` (intra_prediction.c:2404-2435)
//!   uses a *different* loop structure than SVT's own lbd
//!   `svt_av1_dr_prediction_z2_c` (intra_prediction.c:386-415): the lbd
//!   version is an incremental accumulator (matches
//!   `intra_pred::dr_z2_edged` exactly); the hbd version independently
//!   recomputes `x`/`y`/`base` from scratch at every `(r, c)`. This is a
//!   genuine difference in SVT-AV1's own C source, not a porting
//!   inconsistency — [`dr_z2_edged_hbd`] below is translated literally
//!   from the hbd C function and intentionally does NOT share
//!   `dr_z2_edged`'s incremental shape. z1 and z3 hbd DO share their lbd
//!   siblings' incremental shape (verified line-for-line).
//! - The sized `svt_aom_highbd_10_variance{W}x{H}_c` family is generated
//!   by `HIGHBD_VAR` in `Codec/svt_psnr.c:155`, and is live through
//!   `av1me.c`'s `vf_hbd_10` table. It rounds SSE by four bits and the
//!   signed residual sum by two bits BEFORE subtracting the DC term.
//!   [`highbd_variance`] is the distinct generic, unnormalized function;
//!   it cannot substitute for the sized 10-bit family. The encoder's
//!   pristine chroma presort has a differential test against that family.

// =============================================================================
// 1. Recon add + clip (docs/bd10-port-map.md item 2)
//
// C: definitions.h:725-735 (`clip_pixel_highbd`), inv_transforms.c:2426-2446
// (`check_range` / `HIGHBD_WRAPLOW` / `highbd_clip_pixel_add`).
//
// PORT-NOTE(unverified) on all four functions below: verify vs FFI parity
// once wired (see module doc verification plan).
// =============================================================================

/// C `clip_pixel_highbd` (definitions.h:725-735).
#[inline]
pub fn clip_pixel_highbd(val: i32, bd: u8) -> u16 {
    let max = match bd {
        10 => 1023,
        12 => 4095,
        _ => 255, // C: `case 8: default:` grouped together
    };
    val.clamp(0, max) as u16
}

/// C `check_range` (inv_transforms.c:2426-2439): clamps a transform
/// coefficient to the bd-dependent representable range. The `assert`s C
/// guards behind `CONFIG_COEFFICIENT_RANGE_CHECKING` are debug-only and
/// NOT ported (no Rust equivalent needed — they never affect output, only
/// trap out-of-range input in debug C builds); the `clamp64` itself is
/// unconditional and IS load-bearing, ported below via `i64::clamp`.
#[inline]
pub fn check_range(input: i64, bd: u8) -> i64 {
    let int_max = ((1i32 << (7 + bd as i32)) - 1 + (914i32 << (bd as i32 - 7))) as i64;
    let int_min = -int_max - 1;
    input.clamp(int_min, int_max)
}

/// C `HIGHBD_WRAPLOW` macro (inv_transforms.c:2441): `(int32_t)check_range(x, bd)`.
#[inline]
pub fn highbd_wraplow(x: i64, bd: u8) -> i32 {
    check_range(x, bd) as i32
}

/// C `highbd_clip_pixel_add` (inv_transforms.c:2443-2446): add a residual to
/// a base pixel with the full range-check chain.
///
/// The 8-bit recon path in this crate (`inv_txfm.rs`'s `inv_txfm2d_core`
/// doc, ~line 1796) takes a documented SHORTCUT — plain `clip(base +
/// residual)` without the intermediate `HIGHBD_WRAPLOW` clamp — justified
/// there ONLY for an 8-bit base by a saturation argument specific to that
/// bit depth ("|residual| <= 34596 saturates the pixel clip in the same
/// direction"). That argument is NOT re-derived here for bd10/bd12; this
/// function ports the FULL C chain (`check_range` then `clip_pixel_highbd`)
/// rather than assuming the bd8 shortcut generalizes.
#[inline]
pub fn highbd_clip_pixel_add(dest: u16, trans: i64, bd: u8) -> u16 {
    let trans = highbd_wraplow(trans, bd);
    clip_pixel_highbd(dest as i32 + trans, bd)
}

// =============================================================================
// 2. Highbd intra predictors — DC family, V, H, Paeth, smooth family
// C: intra_prediction.c:1202-1399 (`highbd_{v,h,paeth,smooth,smooth_v,
// smooth_h,dc,dc_128,dc_left,dc_top}_predictor`), all `#if
// CONFIG_ENABLE_HIGH_BIT_DEPTH`. Mirrors `intra_pred::predict_dc` /
// `predict_v` / `predict_h` / `predict_paeth` / `predict_smooth{,_v,_h}`'s
// combined-arm / per-mode shapes.
//
// FFI-VERIFIED: tests/c_parity_intra_pred_hbd.rs pins the whole predictor
// family below (predict_{v,h,paeth,dc,smooth,smooth_v,smooth_h}_hbd) against
// the real exported sized svt_aom_highbd_*_predictor_WxH_c wrappers over 10
// modes x 19 sizes x bd{10,12} (DC's 4 above/left cases included), plus a
// non-vacuous known-answer guard. The two flagged risk spots BOTH match C:
// the Paeth left-first tie-break and the DC128 128<<(bd-8) shift.
// =============================================================================

/// C `highbd_v_predictor` (intra_prediction.c:1202-1210). `bd` unused (C:
/// `(void)bd;`) — kept in the signature only for call-shape parity with the
/// other hbd predictors and the directional dispatcher.
pub fn predict_v_hbd(
    dst: &mut [u16],
    dst_stride: usize,
    above: &[u16],
    width: usize,
    height: usize,
) {
    for row in 0..height {
        dst[row * dst_stride..row * dst_stride + width].copy_from_slice(&above[..width]);
    }
}

/// C `highbd_h_predictor` (intra_prediction.c:1212-1220). `bd` unused.
pub fn predict_h_hbd(
    dst: &mut [u16],
    dst_stride: usize,
    left: &[u16],
    width: usize,
    height: usize,
) {
    for row in 0..height {
        let val = left[row];
        for col in 0..width {
            dst[row * dst_stride + col] = val;
        }
    }
}

/// C `paeth_predictor_single` (intra_prediction.c:1226-1234) — shared
/// verbatim by C's lbd AND hbd paeth predictors, LEFT checked first. See
/// the module doc "Findings" for the discrepancy vs
/// `intra_pred::predict_paeth_core`'s (TOP-first) order, NOT reproduced
/// here.
#[inline]
fn paeth_predictor_single_hbd(left: u16, top: u16, top_left: u16) -> u16 {
    let base = top as i32 + left as i32 - top_left as i32;
    let p_left = (base - left as i32).abs();
    let p_top = (base - top as i32).abs();
    let p_top_left = (base - top_left as i32).abs();
    if p_left <= p_top && p_left <= p_top_left {
        left
    } else if p_top <= p_top_left {
        top
    } else {
        top_left
    }
}

/// C `highbd_paeth_predictor` (intra_prediction.c:1248-1258). `bd` unused
/// (C: `(void)bd;` — paeth always returns an existing neighbour sample, no
/// clipping needed by construction).
pub fn predict_paeth_hbd(
    dst: &mut [u16],
    dst_stride: usize,
    above: &[u16],
    left: &[u16],
    top_left: u16,
    width: usize,
    height: usize,
) {
    incant!(
        predict_paeth_hbd_impl(dst, dst_stride, above, left, top_left, width, height),
        [neon, scalar]
    )
}

#[allow(clippy::too_many_arguments)]
fn predict_paeth_hbd_impl_scalar(
    _token: ScalarToken,
    dst: &mut [u16],
    dst_stride: usize,
    above: &[u16],
    left: &[u16],
    top_left: u16,
    width: usize,
    height: usize,
) {
    for row in 0..height {
        for col in 0..width {
            dst[row * dst_stride + col] =
                paeth_predictor_single_hbd(left[row], above[col], top_left);
        }
    }
}

/// aarch64 arm of [`predict_paeth_hbd`]. Same row-constant collapse as the
/// u8 arm (`p_top = |lft - tl|` is per-row scalar), but in i32 lanes so the
/// math is exact for ANY u16 sample range — `top + lft - 2*tl` spans
/// [-131070, 131070] for full-range inputs, past i16. CRITICAL: the hbd
/// scalar checks LEFT first (C `paeth_predictor_single` left-then-top
/// tie-break) — the select order below is NOT the u8 arm's top-first.
#[cfg(target_arch = "aarch64")]
#[arcane]
fn predict_paeth_hbd_impl_neon(
    _token: NeonToken,
    dst: &mut [u16],
    dst_stride: usize,
    above: &[u16],
    left: &[u16],
    top_left: u16,
    width: usize,
    height: usize,
) {
    let tl = top_left as i32;
    let tl_v = vdupq_n_s32(tl);
    let two_tl_v = vdupq_n_s32(tl * 2);
    for row in 0..height {
        let lft = left[row] as i32;
        let lft_v = vdupq_n_s32(lft);
        let p_top_v = vdupq_n_s32((lft - tl).abs());
        let base_row = row * dst_stride;
        let mut col = 0;
        while col + 4 <= width {
            let a: &[u16; 4] = above[col..col + 4].try_into().unwrap();
            let top_v = vreinterpretq_s32_u32(vmovl_u16(vld1_u16(a)));
            let p_left_v = vabsq_s32(vsubq_s32(top_v, tl_v));
            let p_tl_v = vabsq_s32(vsubq_s32(vaddq_s32(top_v, lft_v), two_tl_v));
            // if p_left <= p_top && p_left <= p_tl { lft }
            // else if p_top <= p_tl { top } else { tl }
            let take_left = vandq_u32(vcleq_s32(p_left_v, p_top_v), vcleq_s32(p_left_v, p_tl_v));
            let take_top = vcleq_s32(p_top_v, p_tl_v);
            let pred = vbslq_s32(take_top, top_v, tl_v);
            let pred = vbslq_s32(take_left, lft_v, pred);
            let out: &mut [u16; 4] = (&mut dst[base_row + col..base_row + col + 4])
                .try_into()
                .unwrap();
            vst1_u16(out, vmovn_u32(vreinterpretq_u32_s32(pred)));
            col += 4;
        }
        while col < width {
            dst[base_row + col] = paeth_predictor_single_hbd(left[row], above[col], top_left);
            col += 1;
        }
    }
}

/// C `highbd_dc_predictor` / `highbd_dc_left_predictor` /
/// `highbd_dc_top_predictor` / `highbd_dc_128_predictor`
/// (intra_prediction.c:1336-1399), combined into one function mirroring
/// `intra_pred::predict_dc`'s `(has_above, has_left)` branch shape. Only
/// the `(false, false)` arm is bd-dependent (C `128 << (bd - 8)`,
/// `highbd_dc_128_predictor`); the other three arms ignore `bd` (C:
/// `(void)bd;`), matching the u8 sibling's structure exactly.
#[allow(clippy::too_many_arguments)]
pub fn predict_dc_hbd(
    dst: &mut [u16],
    dst_stride: usize,
    above: &[u16],
    left: &[u16],
    width: usize,
    height: usize,
    has_above: bool,
    has_left: bool,
    bd: u8,
) {
    incant!(
        predict_dc_hbd_impl(
            dst, dst_stride, above, left, width, height, has_above, has_left, bd
        ),
        [neon, scalar]
    )
}

#[allow(clippy::too_many_arguments)]
fn predict_dc_hbd_impl_scalar(
    _token: ScalarToken,
    dst: &mut [u16],
    dst_stride: usize,
    above: &[u16],
    left: &[u16],
    width: usize,
    height: usize,
    has_above: bool,
    has_left: bool,
    bd: u8,
) {
    predict_dc_hbd_core(
        dst, dst_stride, above, left, width, height, has_above, has_left, bd,
    );
}

/// Scalar core of [`predict_dc_hbd`]; every tier must produce this `dc`.
#[allow(clippy::too_many_arguments)]
fn predict_dc_hbd_core(
    dst: &mut [u16],
    dst_stride: usize,
    above: &[u16],
    left: &[u16],
    width: usize,
    height: usize,
    has_above: bool,
    has_left: bool,
    bd: u8,
) {
    let dc = match (has_above, has_left) {
        (true, true) => {
            let sum: u32 = above[..width].iter().map(|&v| v as u32).sum::<u32>()
                + left[..height].iter().map(|&v| v as u32).sum::<u32>();
            let count = (width + height) as u32;
            ((sum + count / 2) / count) as u16
        }
        (true, false) => {
            let sum: u32 = above[..width].iter().map(|&v| v as u32).sum();
            ((sum + width as u32 / 2) / width as u32) as u16
        }
        (false, true) => {
            let sum: u32 = left[..height].iter().map(|&v| v as u32).sum();
            ((sum + height as u32 / 2) / height as u32) as u16
        }
        (false, false) => (128u32 << (bd as u32 - 8)) as u16,
    };
    for row in 0..height {
        dst[row * dst_stride..row * dst_stride + width].fill(dc);
    }
}

/// Sum the first `n` u16s of `e` with UADDLV widening reductions — 8-lane
/// chunks, then 4- and 2-lane narrow loads so every AV1 edge length (4..64)
/// is fully covered. `vaddlvq_u16`/`vaddlv_u16` return the exact u32 total
/// (8×65535 and 4×65535 both fit u32 — no input-range assumption, unlike C's
/// u16 tree-sum which presumes ≤12-bit samples).
#[cfg(target_arch = "aarch64")]
#[rite]
fn dc_edge_sum_hbd_neon(_token: NeonToken, e: &[u16], n: usize) -> u32 {
    let mut sum = 0u32;
    let mut c = 0usize;
    while c + 8 <= n {
        let v: &[u16; 8] = e[c..c + 8].try_into().unwrap();
        sum += vaddlvq_u16(vld1q_u16(v));
        c += 8;
    }
    if c + 4 <= n {
        let v: &[u16; 4] = e[c..c + 4].try_into().unwrap();
        sum += vaddlv_u16(vld1_u16(v));
        c += 4;
    }
    if c + 2 <= n {
        let packed = u64::from(e[c] as u32) | (u64::from(e[c + 1] as u32) << 16);
        sum += vaddlv_u16(vcreate_u16(packed));
        c += 2;
    }
    for k in c..n {
        sum += e[k] as u32;
    }
    sum
}

/// aarch64 arm of [`predict_dc_hbd`]: the edge sums go through
/// `dc_edge_sum_hbd_neon`; the destination fill stays `.fill`. Same division
/// and rounding as the scalar core, exact.
#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn predict_dc_hbd_impl_neon(
    token: NeonToken,
    dst: &mut [u16],
    dst_stride: usize,
    above: &[u16],
    left: &[u16],
    width: usize,
    height: usize,
    has_above: bool,
    has_left: bool,
    bd: u8,
) {
    let dc = match (has_above, has_left) {
        (true, true) => {
            let sum = dc_edge_sum_hbd_neon(token, above, width)
                + dc_edge_sum_hbd_neon(token, left, height);
            let count = (width + height) as u32;
            ((sum + count / 2) / count) as u16
        }
        (true, false) => {
            let sum = dc_edge_sum_hbd_neon(token, above, width);
            ((sum + width as u32 / 2) / width as u32) as u16
        }
        (false, true) => {
            let sum = dc_edge_sum_hbd_neon(token, left, height);
            ((sum + height as u32 / 2) / height as u32) as u16
        }
        (false, false) => (128u32 << (bd as u32 - 8)) as u16,
    };
    for row in 0..height {
        dst[row * dst_stride..row * dst_stride + width].fill(dc);
    }
}

/// Smooth-weight tables — bd-independent. Duplicated from `intra_pred.rs`'s
/// private `SM_WEIGHTS_{4,8,16,32,64}` statics (not `pub`, so not reusable
/// across modules without editing that file, out of this task's scope);
/// same C provenance (`sm_weight_arrays`, reconintra tables).
static SM_WEIGHTS_4_HBD: [u8; 4] = [255, 149, 85, 64];
static SM_WEIGHTS_8_HBD: [u8; 8] = [255, 197, 146, 105, 73, 50, 37, 32];
static SM_WEIGHTS_16_HBD: [u8; 16] = [
    255, 225, 196, 170, 145, 123, 102, 84, 68, 54, 43, 33, 26, 20, 17, 16,
];
static SM_WEIGHTS_32_HBD: [u8; 32] = [
    255, 240, 225, 210, 196, 182, 169, 157, 145, 133, 122, 111, 101, 92, 83, 74, 66, 59, 52, 45,
    39, 34, 29, 25, 21, 17, 14, 12, 10, 9, 8, 8,
];
static SM_WEIGHTS_64_HBD: [u8; 64] = [
    255, 248, 240, 233, 225, 218, 210, 203, 196, 189, 182, 176, 169, 163, 156, 150, 144, 138, 133,
    127, 121, 116, 111, 106, 101, 96, 91, 86, 82, 77, 73, 69, 65, 61, 57, 54, 50, 47, 44, 41, 38,
    35, 32, 29, 27, 25, 22, 20, 18, 16, 15, 13, 12, 10, 9, 8, 7, 6, 6, 5, 5, 4, 4, 4,
];

fn smooth_weights_hbd(n: usize) -> &'static [u8] {
    match n {
        4 => &SM_WEIGHTS_4_HBD,
        8 => &SM_WEIGHTS_8_HBD,
        16 => &SM_WEIGHTS_16_HBD,
        32 => &SM_WEIGHTS_32_HBD,
        64 => &SM_WEIGHTS_64_HBD,
        _ => &SM_WEIGHTS_4_HBD,
    }
}

/// C `highbd_smooth_predictor` (intra_prediction.c:1260-1286). `bd` unused
/// (C: `(void)bd;`); `divide_round(_, 9)` = `(x + 256) >> 9`.
pub fn predict_smooth_hbd(
    dst: &mut [u16],
    dst_stride: usize,
    above: &[u16],
    left: &[u16],
    width: usize,
    height: usize,
) {
    incant!(
        predict_smooth_hbd_impl(dst, dst_stride, above, left, width, height),
        [neon, scalar]
    )
}

fn predict_smooth_hbd_impl_scalar(
    _token: ScalarToken,
    dst: &mut [u16],
    dst_stride: usize,
    above: &[u16],
    left: &[u16],
    width: usize,
    height: usize,
) {
    predict_smooth_hbd_core(dst, dst_stride, above, left, width, height);
}

fn predict_smooth_hbd_core(
    dst: &mut [u16],
    dst_stride: usize,
    above: &[u16],
    left: &[u16],
    width: usize,
    height: usize,
) {
    let below_pred = left[height - 1] as u32;
    let right_pred = above[width - 1] as u32;
    let sm_weights_h = smooth_weights_hbd(height);
    let sm_weights_w = smooth_weights_hbd(width);
    for row in 0..height {
        for col in 0..width {
            let wh = sm_weights_h[row] as u32;
            let ww = sm_weights_w[col] as u32;
            let top = above[col] as u32;
            let lft = left[row] as u32;
            let pred =
                (wh * top + (256 - wh) * below_pred + ww * lft + (256 - ww) * right_pred + 256)
                    / 512;
            dst[row * dst_stride + col] = pred as u16;
        }
    }
}

/// aarch64 arm of [`predict_smooth_hbd`] — the u8 arm's factored form in
/// i32 lanes (u16 samples; `top <= 65535` keeps `wh*top <= 16.7M`):
///   pred[c] = (wh * top[c] + ww[c] * d + K) >> 9
///   d = left[row] - right,  K = 256*right + (256 - wh)*below + 256
/// The total equals the all-nonneg scalar numerator (<= 2,096,896), so the
/// arithmetic `>> 9` is the scalar floor-div and `vmovn_u32` is exact.
#[cfg(target_arch = "aarch64")]
#[arcane]
fn predict_smooth_hbd_impl_neon(
    _token: NeonToken,
    dst: &mut [u16],
    dst_stride: usize,
    above: &[u16],
    left: &[u16],
    width: usize,
    height: usize,
) {
    if width % 4 != 0 || width > 64 {
        predict_smooth_hbd_core(dst, dst_stride, above, left, width, height);
        return;
    }
    let below = left[height - 1] as i32;
    let right = above[width - 1] as i32;
    let sm_h = smooth_weights_hbd(height);
    let sm_w = smooth_weights_hbd(width);
    let mut tops = [vdupq_n_s32(0); 16];
    let mut wws = [vdupq_n_s32(0); 16];
    for j in 0..width / 4 {
        let a: &[u16; 4] = above[j * 4..j * 4 + 4].try_into().unwrap();
        tops[j] = vreinterpretq_s32_u32(vmovl_u16(vld1_u16(a)));
        let w4 = [
            sm_w[j * 4] as i32,
            sm_w[j * 4 + 1] as i32,
            sm_w[j * 4 + 2] as i32,
            sm_w[j * 4 + 3] as i32,
        ];
        wws[j] = vld1q_s32(&w4);
    }
    for row in 0..height {
        let wh = sm_h[row] as i32;
        let dv = vdupq_n_s32(left[row] as i32 - right);
        let kv = vdupq_n_s32(256 * right + (256 - wh) * below + 256);
        let whv = vdupq_n_s32(wh);
        let base = row * dst_stride;
        for j in 0..width / 4 {
            let pred = vshrq_n_s32::<9>(vmlaq_s32(vmlaq_s32(kv, tops[j], whv), wws[j], dv));
            let out: &mut [u16; 4] = (&mut dst[base + j * 4..base + j * 4 + 4])
                .try_into()
                .unwrap();
            vst1_u16(out, vmovn_u32(vreinterpretq_u32_s32(pred)));
        }
    }
}

/// C `highbd_smooth_v_predictor` (intra_prediction.c:1288-1310). `bd`
/// unused; `divide_round(_, 8)` = `(x + 128) >> 8`.
pub fn predict_smooth_v_hbd(
    dst: &mut [u16],
    dst_stride: usize,
    above: &[u16],
    left: &[u16],
    width: usize,
    height: usize,
) {
    incant!(
        predict_smooth_v_hbd_impl(dst, dst_stride, above, left, width, height),
        [neon, scalar]
    )
}

fn predict_smooth_v_hbd_impl_scalar(
    _token: ScalarToken,
    dst: &mut [u16],
    dst_stride: usize,
    above: &[u16],
    left: &[u16],
    width: usize,
    height: usize,
) {
    predict_smooth_v_hbd_core(dst, dst_stride, above, left, width, height);
}

fn predict_smooth_v_hbd_core(
    dst: &mut [u16],
    dst_stride: usize,
    above: &[u16],
    left: &[u16],
    width: usize,
    height: usize,
) {
    let below_pred = left[height - 1] as u32;
    let sm_weights = smooth_weights_hbd(height);
    for row in 0..height {
        let w = sm_weights[row] as u32;
        for col in 0..width {
            let top = above[col] as u32;
            let pred = (w * top + (256 - w) * below_pred + 128) / 256;
            dst[row * dst_stride + col] = pred as u16;
        }
    }
}

/// aarch64 arm of [`predict_smooth_v_hbd`]: per row only `w` varies, so
/// `pred[c] = (w * top[c] + K) >> 8` with `K = (256 - w)*below + 128` — one
/// `vmlaq` + arithmetic shift per 4 lanes; `vmovn_u32` is exact (the scalar
/// `as u16` truncates the same way for any value < 2^16).
#[cfg(target_arch = "aarch64")]
#[arcane]
fn predict_smooth_v_hbd_impl_neon(
    _token: NeonToken,
    dst: &mut [u16],
    dst_stride: usize,
    above: &[u16],
    left: &[u16],
    width: usize,
    height: usize,
) {
    let below = left[height - 1] as i32;
    let sm_weights = smooth_weights_hbd(height);
    for row in 0..height {
        let w = sm_weights[row] as i32;
        let wv = vdupq_n_s32(w);
        let kv = vdupq_n_s32((256 - w) * below + 128);
        let base = row * dst_stride;
        let mut col = 0;
        while col + 4 <= width {
            let a: &[u16; 4] = above[col..col + 4].try_into().unwrap();
            let top_v = vreinterpretq_s32_u32(vmovl_u16(vld1_u16(a)));
            let pred = vshrq_n_s32::<8>(vmlaq_s32(kv, top_v, wv));
            let out: &mut [u16; 4] = (&mut dst[base + col..base + col + 4]).try_into().unwrap();
            vst1_u16(out, vmovn_u32(vreinterpretq_u32_s32(pred)));
            col += 4;
        }
        while col < width {
            let top = above[col] as u32;
            let pred = (w as u32 * top + (256 - w as u32) * (below as u32) + 128) / 256;
            dst[base + col] = pred as u16;
            col += 1;
        }
    }
}

/// C `highbd_smooth_h_predictor` (intra_prediction.c:1312-1334). `bd` unused.
pub fn predict_smooth_h_hbd(
    dst: &mut [u16],
    dst_stride: usize,
    above: &[u16],
    left: &[u16],
    width: usize,
    height: usize,
) {
    incant!(
        predict_smooth_h_hbd_impl(dst, dst_stride, above, left, width, height),
        [neon, scalar]
    )
}

fn predict_smooth_h_hbd_impl_scalar(
    _token: ScalarToken,
    dst: &mut [u16],
    dst_stride: usize,
    above: &[u16],
    left: &[u16],
    width: usize,
    height: usize,
) {
    predict_smooth_h_hbd_core(dst, dst_stride, above, left, width, height);
}

fn predict_smooth_h_hbd_core(
    dst: &mut [u16],
    dst_stride: usize,
    above: &[u16],
    left: &[u16],
    width: usize,
    height: usize,
) {
    let right_pred = above[width - 1] as u32;
    let sm_weights = smooth_weights_hbd(width);
    for row in 0..height {
        let lft = left[row] as u32;
        for col in 0..width {
            let w = sm_weights[col] as u32;
            let pred = (w * lft + (256 - w) * right_pred + 128) / 256;
            dst[row * dst_stride + col] = pred as u16;
        }
    }
}

/// aarch64 arm of [`predict_smooth_h_hbd`]: per row only `lft` varies, so
/// `pred[c] = (w[c] * d + K) >> 8` with `d = lft - right`,
/// `K = 256*right + 128` — the intermediate `w*d` can be negative in i32
/// but the total equals the all-nonneg scalar numerator, so `>> 8` is the
/// scalar floor-div and `vmovn_u32` truncates exactly like `as u16`.
#[cfg(target_arch = "aarch64")]
#[arcane]
fn predict_smooth_h_hbd_impl_neon(
    _token: NeonToken,
    dst: &mut [u16],
    dst_stride: usize,
    above: &[u16],
    left: &[u16],
    width: usize,
    height: usize,
) {
    if width > 64 {
        predict_smooth_h_hbd_core(dst, dst_stride, above, left, width, height);
        return;
    }
    let right = above[width - 1] as i32;
    let sm_weights = smooth_weights_hbd(width);
    let mut wws = [vdupq_n_s32(0); 17];
    for j in 0..(width + 3) / 4 {
        let mut w4 = [0i32; 4];
        for k in 0..4 {
            if j * 4 + k < width {
                w4[k] = sm_weights[j * 4 + k] as i32;
            }
        }
        wws[j] = vld1q_s32(&w4);
    }
    let kv = vdupq_n_s32(256 * right + 128);
    for row in 0..height {
        let dv = vdupq_n_s32(left[row] as i32 - right);
        let base = row * dst_stride;
        let mut col = 0;
        while col + 4 <= width {
            let pred = vshrq_n_s32::<8>(vmlaq_s32(kv, wws[col / 4], dv));
            let out: &mut [u16; 4] = (&mut dst[base + col..base + col + 4]).try_into().unwrap();
            vst1_u16(out, vmovn_u32(vreinterpretq_u32_s32(pred)));
            col += 4;
        }
        while col < width {
            let w = sm_weights[col] as u32;
            let pred = (w * left[row] as u32 + (256 - w) * (right as u32) + 128) / 256;
            dst[base + col] = pred as u16;
            col += 1;
        }
    }
}

// =============================================================================
// 3. Directional predictors (z1/z2/z3, edge-upsample-aware) + edge filter
//    helpers. C: intra_prediction.c:2367-2489 (z1/z2 hbd, edge filter/corner
//    high), C_DEFAULT/intra_prediction_c.c:15-93 (z3 hbd, upsample-edge
//    high). Mirrors `intra_pred::dr_{z1,z2,z3}_edged` / `dr_predictor_edged`
//    / `filter_intra_edge` / `filter_intra_edge_corner` /
//    `upsample_intra_edge` call shapes (origin-relative edged buffers, C
//    `above_row[-1] == left_col[-1]` convention — see `intra_pred::
//    EDGE_ORIGIN` / `EDGE_BUF_LEN` for the buffer layout callers should use;
//    both are `pub` and reusable as-is, bd-independent).
//
// `intra_pred::intra_edge_filter_strength` / `use_intra_edge_upsample` are
// ALSO bd-independent and `pub` (verified: neither C function takes a `bd`
// param) — reuse directly, zero new code needed for those two.
//
// PORT-NOTE(unverified) on every function below: verify vs FFI parity once
// wired (see module doc verification plan).
// =============================================================================

use crate::loop_filter::LfThresh;
use crate::cdef::{BLOCK_4X8, BLOCK_8X4, BLOCK_8X8, CDEF_BSTRIDE, CDEF_VERY_LARGE};
use archmage::prelude::*;
use crate::quant_tables::{AC_QLOOKUP_8, DC_QLOOKUP_8};
#[cfg(test)]
mod dispatch_tests;
mod directional;
pub use directional::*;
mod filter_intra;
pub use filter_intra::*;
mod cfl;
pub use cfl::*;

mod cdef;
pub use cdef::*;
