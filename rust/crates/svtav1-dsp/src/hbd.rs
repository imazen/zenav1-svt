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

/// Derivative table for directional prediction angles — bd-independent.
/// Duplicated from `intra_pred.rs`'s private `DR_INTRA_DERIVATIVE` (same
/// C provenance, `eb_dr_intra_derivative`).
static DR_INTRA_DERIVATIVE_HBD: [u16; 90] = [
    0, 0, 0, 1023, 0, 0, 547, 0, 0, 372, 0, 0, 0, 0, 273, 0, 0, 215, 0, 0, 178, 0, 0, 151, 0, 0,
    132, 0, 0, 116, 0, 0, 102, 0, 0, 0, 90, 0, 0, 80, 0, 0, 71, 0, 0, 64, 0, 0, 57, 0, 0, 51, 0, 0,
    45, 0, 0, 0, 40, 0, 0, 35, 0, 0, 31, 0, 0, 27, 0, 0, 23, 0, 0, 19, 0, 0, 15, 0, 0, 0, 0, 11, 0,
    0, 7, 0, 0, 3, 0, 0,
];

fn get_dx_hbd(angle: i32) -> i32 {
    if angle > 0 && angle < 90 {
        DR_INTRA_DERIVATIVE_HBD[angle as usize] as i32
    } else if angle > 90 && angle < 180 {
        DR_INTRA_DERIVATIVE_HBD[(180 - angle) as usize] as i32
    } else {
        1
    }
}

fn get_dy_hbd(angle: i32) -> i32 {
    if angle > 90 && angle < 180 {
        DR_INTRA_DERIVATIVE_HBD[(angle - 90) as usize] as i32
    } else if angle > 180 && angle < 270 {
        DR_INTRA_DERIVATIVE_HBD[(270 - angle) as usize] as i32
    } else {
        1
    }
}

/// C `svt_av1_filter_intra_edge_high_c` (intra_prediction.c:2459-2480). No
/// bd/clip needed — every kernel row sums to 16 (convex combination), so a
/// weighted average of samples already within `[0, 2^bd - 1]` cannot exceed
/// that range after the `>>4` round (matches the lbd sibling
/// `intra_pred::filter_intra_edge`, which is also clip-free for the same
/// reason).
pub fn filter_intra_edge_high(p: &mut [u16], start: usize, sz: usize, strength: i32) {
    if strength == 0 {
        return;
    }
    const KERNEL: [[i32; 5]; 3] = [[0, 4, 8, 4, 0], [0, 5, 6, 5, 0], [2, 4, 4, 4, 2]];
    let filt = (strength - 1) as usize;
    debug_assert!(sz <= 129);
    let mut edge = [0u16; 129];
    edge[..sz].copy_from_slice(&p[start..start + sz]);
    for i in 1..sz {
        let mut s = 0i32;
        for (j, &k_w) in KERNEL[filt].iter().enumerate() {
            let k = (i as i32 - 2 + j as i32).clamp(0, sz as i32 - 1) as usize;
            s += edge[k] as i32 * k_w;
        }
        p[start + i] = ((s + 8) >> 4) as u16;
    }
}

/// C `filter_intra_edge_corner_high` (intra_prediction.c:2482-2489).
pub fn filter_intra_edge_corner_high(above: &mut [u16], left: &mut [u16], origin: usize) {
    let s = (left[origin] as i32 * 5 + above[origin - 1] as i32 * 6 + above[origin] as i32 * 5 + 8)
        >> 4;
    above[origin - 1] = s as u16;
    left[origin - 1] = s as u16;
}

/// C `svt_av1_upsample_intra_edge_high_c` (C_DEFAULT/intra_prediction_c.c:
/// 15-37). Unlike the clip-free edge filter above, the FIR-like
/// `[-1, 9, 9, -1]` kernel here has a negative tap and CAN overshoot
/// `[0, 2^bd - 1]`, so C clips via `clip_pixel_highbd` — the lbd sibling
/// `intra_pred::upsample_intra_edge` clips too (`.clamp(0, 255)`); this is
/// just the bd-generalized form of that same clamp.
pub fn upsample_intra_edge_high(p: &mut [u16], origin: usize, sz: usize, bd: u8) {
    debug_assert!(sz <= 16, "C MAX_UPSAMPLE_SZ");
    debug_assert!(origin >= 2);
    let mut input = [0u16; 16 + 3];
    input[0] = p[origin - 1];
    input[1] = p[origin - 1];
    input[2..2 + sz].copy_from_slice(&p[origin..origin + sz]);
    input[sz + 2] = p[origin + sz - 1];

    p[origin - 2] = input[0];
    for i in 0..sz {
        let s = -(input[i] as i32) + 9 * input[i + 1] as i32 + 9 * input[i + 2] as i32
            - input[i + 3] as i32;
        let s = clip_pixel_highbd((s + 8) >> 4, bd);
        p[origin + 2 * i - 1] = s;
        p[origin + 2 * i] = input[i + 2];
    }
}

/// C `svt_av1_highbd_dr_prediction_z1_c` (intra_prediction.c:2367-2401).
/// Shares `intra_pred::dr_z1_edged`'s incremental-accumulator structure
/// exactly (verified line-for-line against the C) — `shift` is constant
/// across a row, so C accumulates `base` incrementally per-column rather
/// than recomputing from scratch.
fn dr_z1_edged_hbd(
    dst: &mut [u16],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u16],
    origin: usize,
    upsample_above: bool,
    dx: i32,
    bd: u8,
) {
    // The vector arm reads `above[origin + base + c .. c + 9]` with
    // `base + c < max_base_x = bw + bh - 1`, so the highest index it can
    // touch is `origin + max_base_x` — one past the scalar core's own
    // `above[origin + max_base_x]` fill read. `bw >= 8` is the u8
    // sibling's `bw >= 16` halved: 8-lane u16 chunks instead of 16-lane
    // u8. Upsampled rows take the scalar core (production upsample only
    // fires for `bw + bh <= 16` anyway).
    if bw >= 8 && !upsample_above && above.len() > origin + bw + bh {
        incant!(
            dr_z1_edged_hbd_flat(dst, dst_stride, bw, bh, above, origin, dx, bd),
            [neon, scalar]
        );
        return;
    }
    dr_z1_edged_hbd_core(
        dst,
        dst_stride,
        bw,
        bh,
        above,
        origin,
        upsample_above,
        dx,
        bd,
    );
}

#[allow(clippy::too_many_arguments)]
fn dr_z1_edged_hbd_flat_scalar(
    _token: ScalarToken,
    dst: &mut [u16],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u16],
    origin: usize,
    dx: i32,
    bd: u8,
) {
    dr_z1_edged_hbd_core(dst, dst_stride, bw, bh, above, origin, false, dx, bd);
}

/// aarch64 arm of [`dr_z1_edged_hbd`], non-upsampled rows: C's
/// `highbd_dr_prediction_z1_upsample0_neon` shape
/// (`highbd_intra_prediction_neon.c:1402`) — u32 widening accumulate
/// `a1*2s + a0*(64-2s)` then `vrshrn_n_u32::<6>`, which equals the scalar
/// `(a0*(32-s) + a1*s + 16) >> 5` exactly (`(2X+32)>>6 == (X+16)>>5`),
/// plus a `vmin` against `bd_max` replicating `clip_pixel_highbd`'s upper
/// clamp (the lower clamp can never fire: every term is non-negative).
#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn dr_z1_edged_hbd_flat_neon(
    _token: NeonToken,
    dst: &mut [u16],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u16],
    origin: usize,
    dx: i32,
    bd: u8,
) {
    let max_base_x = (bw + bh) as i32 - 1;
    let fill = above[origin + max_base_x as usize];
    // Same max as `clip_pixel_highbd`: non-{10,12} bd groups to 255.
    let bd_max: u16 = match bd {
        10 => 1023,
        12 => 4095,
        _ => 255,
    };
    let bd_max_v = vdupq_n_u16(bd_max);
    let mut x = dx;
    for r in 0..bh {
        let base = x >> 6;
        if base >= max_base_x {
            for row in dst.chunks_mut(dst_stride).skip(r).take(bh - r) {
                row[..bw].fill(fill);
            }
            return;
        }
        let shift = ((x & 0x3f) >> 1) as u32;
        // u16 lane factors: s2 = 2*shift in [0,62], w0 = 64-2s in [2,64].
        let s2 = (shift * 2) as u16;
        let w0 = 64 - s2;
        let bi = origin + base as usize;
        let valid = core::cmp::min(bw, (max_base_x - base) as usize);
        let drow = &mut dst[r * dst_stride..r * dst_stride + bw];
        let mut c = 0;
        while c + 8 <= valid {
            let a0 = vld1q_u16(above[bi + c..bi + c + 8].try_into().unwrap());
            let a1 = vld1q_u16(above[bi + c + 1..bi + c + 9].try_into().unwrap());
            let lo = vmlal_n_u16(vmull_n_u16(vget_low_u16(a1), s2), vget_low_u16(a0), w0);
            let hi = vmlal_n_u16(vmull_n_u16(vget_high_u16(a1), s2), vget_high_u16(a0), w0);
            let out = vminq_u16(
                vcombine_u16(vrshrn_n_u32::<6>(lo), vrshrn_n_u32::<6>(hi)),
                bd_max_v,
            );
            vst1q_u16((&mut drow[c..c + 8]).try_into().unwrap(), out);
            c += 8;
        }
        if c + 4 <= valid {
            let a0 = vld1_u16(above[bi + c..bi + c + 4].try_into().unwrap());
            let a1 = vld1_u16(above[bi + c + 1..bi + c + 5].try_into().unwrap());
            let res = vmlal_n_u16(vmull_n_u16(a1, s2), a0, w0);
            let out = vmin_u16(vrshrn_n_u32::<6>(res), vget_low_u16(bd_max_v));
            vst1_u16((&mut drow[c..c + 4]).try_into().unwrap(), out);
            c += 4;
        }
        let sh = shift as i32;
        while c < valid {
            let v = (above[bi + c] as i32 * (32 - sh) + above[bi + c + 1] as i32 * sh + 16) >> 5;
            drow[c] = (v as u16).min(bd_max);
            c += 1;
        }
        drow[c..].fill(fill);
        x += dx;
    }
}

#[allow(clippy::too_many_arguments)]
fn dr_z1_edged_hbd_core(
    dst: &mut [u16],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u16],
    origin: usize,
    upsample_above: bool,
    dx: i32,
    bd: u8,
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
                clip_pixel_highbd((val + 16) >> 5, bd)
            } else {
                above[origin + max_base_x as usize]
            };
            dst[r * dst_stride + c] = v;
            base += base_inc;
        }
        x += dx;
    }
}

/// C `svt_av1_highbd_dr_prediction_z2_c` (intra_prediction.c:2404-2435).
///
/// **Intentionally NOT structured like `intra_pred::dr_z2_edged`** — see
/// the module doc "Findings": SVT's lbd z2 (`svt_av1_dr_prediction_z2_c`,
/// intra_prediction.c:386-415) uses an incremental accumulator (`x`/`base1`
/// carried across the row/column loops), but the hbd z2 independently
/// recomputes `x`, `y`, `base` from scratch at every `(r, c)` — a genuine
/// difference in SVT-AV1's own source, translated literally here rather
/// than reconciled with the lbd shape.
#[allow(clippy::too_many_arguments)]
fn dr_z2_edged_hbd(
    dst: &mut [u16],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u16],
    left: &[u16],
    origin: usize,
    upsample_above: bool,
    upsample_left: bool,
    dx: i32,
    dy: i32,
    bd: u8,
) {
    let up_a = upsample_above as usize;
    let up_l = upsample_left as usize;
    // Worst-case vector reads (see `dr_z2_edged_hbd_simd_neon`): the above
    // pass touches `above[origin + base]` for `base <= base0 + step*(bw-1)`
    // with `base0 <= -1`, plus <=15 elements of chunk/tail padding; the left
    // pass touches `left[origin + base2]` for `base2 <= (bh-1)*step - 1`
    // plus the same padding. Both bounds always hold for the real
    // EDGE_BUF_LEN=160 buffers; the gates exist so synthetic callers with
    // tight slices fall back to the scalar core rather than over-read.
    if origin >= (1 << up_a).max(1 << up_l)
        && above.len() > origin + (bw - 1) * (1 << up_a) + 15
        && left.len() > origin + (bh - 1) * (1 << up_l) + 15
    {
        incant!(
            dr_z2_edged_hbd_simd(
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
                bd,
            ),
            [neon, scalar]
        );
        return;
    }
    dr_z2_edged_hbd_core(
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
        bd,
    );
}

#[allow(clippy::too_many_arguments)]
fn dr_z2_edged_hbd_core(
    dst: &mut [u16],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u16],
    left: &[u16],
    origin: usize,
    upsample_above: bool,
    upsample_left: bool,
    dx: i32,
    dy: i32,
    bd: u8,
) {
    debug_assert!(dx > 0 && dy > 0);
    let up_a = upsample_above as i32;
    let up_l = upsample_left as i32;
    let min_base_x = -(1i32 << up_a);
    let frac_bits_x = 6 - up_a;
    let frac_bits_y = 6 - up_l;
    for r in 0..bh {
        for c in 0..bw {
            let y = r as i32 + 1;
            let x = ((c as i32) << 6) - y * dx;
            let base = x >> frac_bits_x;
            let val = if base >= min_base_x {
                let shift = ((x * (1 << up_a)) & 0x3F) >> 1;
                let i0 = (origin as i32 + base) as usize;
                let v = above[i0] as i32 * (32 - shift) + above[i0 + 1] as i32 * shift;
                (v + 16) >> 5
            } else {
                let x2 = c as i32 + 1;
                let y2 = ((r as i32) << 6) - x2 * dy;
                let base2 = y2 >> frac_bits_y;
                debug_assert!(base2 >= -(1 << up_l));
                let shift = ((y2 * (1 << up_l)) & 0x3F) >> 1;
                let i0 = (origin as i32 + base2) as usize;
                let v = left[i0] as i32 * (32 - shift) + left[i0 + 1] as i32 * shift;
                (v + 16) >> 5
            };
            dst[r * dst_stride + c] = clip_pixel_highbd(val, bd);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn dr_z2_edged_hbd_simd_scalar(
    _token: ScalarToken,
    dst: &mut [u16],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u16],
    left: &[u16],
    origin: usize,
    upsample_above: bool,
    upsample_left: bool,
    dx: i32,
    dy: i32,
    bd: u8,
) {
    dr_z2_edged_hbd_core(
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
        bd,
    );
}

/// Load 8 interpolation pairs starting at element `idx`: `a0[j] =
/// buf[idx + j*step]`, `a1[j] = buf[idx + j*step + 1]`. `step` is the
/// upsample stride — 1 contiguous (two overlapping loads), 2 = `vuzp`
/// deinterleave of a 16-element load.
#[cfg(target_arch = "aarch64")]
#[rite]
fn dr_z2_hbd_load_pairs_neon(
    _token: NeonToken,
    buf: &[u16],
    idx: usize,
    step2: bool,
) -> (uint16x8_t, uint16x8_t) {
    if step2 {
        let v0: &[u16; 8] = buf[idx..idx + 8].try_into().unwrap();
        let v1: &[u16; 8] = buf[idx + 8..idx + 16].try_into().unwrap();
        let e = vld1q_u16(v0);
        let o = vld1q_u16(v1);
        (vuzp1q_u16(e, o), vuzp2q_u16(e, o))
    } else {
        let a0: &[u16; 8] = buf[idx..idx + 8].try_into().unwrap();
        let a1: &[u16; 8] = buf[idx + 1..idx + 9].try_into().unwrap();
        (vld1q_u16(a0), vld1q_u16(a1))
    }
}

/// The shared z1/z2 hbd interpolation: `(a0*(32-s) + a1*s + 16) >> 5`
/// computed as `vrshrn::<6>(a1*2s + a0*(64-2s))` (exact, see
/// `dr_z1_edged_hbd_flat_neon`), then `vmin` replicating the only reachable
/// half of `clip_pixel_highbd` (every term is non-negative).
#[cfg(target_arch = "aarch64")]
#[rite]
fn dr_z2_hbd_interp_neon(
    _token: NeonToken,
    a0: uint16x8_t,
    a1: uint16x8_t,
    s2: u16,
    w0: u16,
    bd_max_v: uint16x8_t,
) -> uint16x8_t {
    let lo = vmlal_n_u16(vmull_n_u16(vget_low_u16(a1), s2), vget_low_u16(a0), w0);
    let hi = vmlal_n_u16(vmull_n_u16(vget_high_u16(a1), s2), vget_high_u16(a0), w0);
    vminq_u16(
        vcombine_u16(vrshrn_n_u32::<6>(lo), vrshrn_n_u32::<6>(hi)),
        bd_max_v,
    )
}

/// aarch64 arm of [`dr_z2_edged_hbd`]. C ships this as tbl-gather kernels
/// (`highbd_dr_prediction_z2_*_neon`), but the index arithmetic is AFFINE,
/// not a real gather:
///
/// - Row `r` (above side): `x = 64c - (r+1)*dx` increases by exactly 64 per
///   column, so `base(c) = base0 + c*step` (`step = 1 << up_a`) and the
///   per-lane shift `((x << up_a) & 0x3F) >> 1` is CONSTANT across the row
///   (`64*step ≡ 0 mod 64`). Above-region lanes are the row SUFFIX
///   `[c*, bw)` where `c*` is the first column with `base >= min_base_x`.
/// - Column `c` (left side): `y2 = 64r - (c+1)*dy` is affine in `r` the same
///   way, so `base2(r) = base2_0 + r*step_y` — contiguous or stride-2
///   loads down the column — and the shift is constant per column. The
///   left-region lanes are the column TAIL `[r*, bh)`.
///
/// The two regions are exact complements of the same predicate, so pass A
/// stores full row suffixes and pass B overwrites the left lanes — no
/// blend needed. Byte-exactness is per-lane: every element computes the
/// scalar core's interpolation on the scalar core's indices.
#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn dr_z2_edged_hbd_simd_neon(
    _token: NeonToken,
    dst: &mut [u16],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u16],
    left: &[u16],
    origin: usize,
    upsample_above: bool,
    upsample_left: bool,
    dx: i32,
    dy: i32,
    bd: u8,
) {
    let up_a = upsample_above as i32;
    let up_l = upsample_left as i32;
    let min_base_x = -(1i32 << up_a);
    let frac_x = 6 - up_a;
    let frac_y = 6 - up_l;
    let step_x = 1usize << up_a;
    let step_y = 1usize << up_l;
    // Same max as `clip_pixel_highbd`: non-{10,12} bd groups to 255.
    let bd_max: u16 = match bd {
        10 => 1023,
        12 => 4095,
        _ => 255,
    };
    let bd_max_v = vdupq_n_u16(bd_max);

    // Pass A — above region: row suffix [c*, bw).
    for r in 0..bh {
        let x0 = -((r as i32 + 1) * dx); // x at c=0
        let base0 = x0 >> frac_x;
        let need = min_base_x - base0;
        let c_star = if need <= 0 {
            0
        } else {
            (need as usize).div_ceil(step_x).min(bw)
        };
        let n = bw - c_star;
        if n == 0 {
            continue;
        }
        let shift = ((x0 << up_a) & 0x3f) >> 1;
        let s2 = (shift * 2) as u16;
        let w0 = 64 - s2;
        // `base0` alone can be far below the window (only the suffix
        // `[c*, bw)` is an above lane) — keep the index arithmetic signed
        // until the first REAL lane's index.
        let bi = (origin as i32 + base0 + (c_star * step_x) as i32) as usize;
        let drow = r * dst_stride + c_star;
        if n >= 8 {
            let mut j = 0usize;
            while j + 8 <= n {
                let (a0, a1) = dr_z2_hbd_load_pairs_neon(_token, above, bi + j * step_x, up_a == 1);
                let out = dr_z2_hbd_interp_neon(_token, a0, a1, s2, w0, bd_max_v);
                vst1q_u16((&mut dst[drow + j..drow + j + 8]).try_into().unwrap(), out);
                j += 8;
            }
            if j < n {
                // Overlap the tail into the last full chunk — the
                // recomputed lanes hold identical values.
                let j = n - 8;
                let (a0, a1) = dr_z2_hbd_load_pairs_neon(_token, above, bi + j * step_x, up_a == 1);
                let out = dr_z2_hbd_interp_neon(_token, a0, a1, s2, w0, bd_max_v);
                vst1q_u16((&mut dst[drow + j..drow + j + 8]).try_into().unwrap(), out);
            }
        } else {
            // n < 8: one vector computes 8 lanes from bounded indices
            // (`base(c* + j)` stays within min_base..min_base+8*step);
            // only the first n lanes store.
            let (a0, a1) = dr_z2_hbd_load_pairs_neon(_token, above, bi, up_a == 1);
            let out = dr_z2_hbd_interp_neon(_token, a0, a1, s2, w0, bd_max_v);
            let mut tmp = [0u16; 8];
            vst1q_u16(&mut tmp, out);
            dst[drow..drow + n].copy_from_slice(&tmp[..n]);
        }
    }

    // Pass B — left region: column tail [r*, bh). For column c the
    // above-side base is `(64c - (r+1)*dx) >> frac_x`, decreasing in r, so
    // the left lanes are exactly the rows with
    // `(r+1)*dx > 64c - (min_base_x << frac_x)`, i.e. `r >= floor(t/dx)`.
    for c in 0..bw {
        let t = 64 * c as i32 - (min_base_x << frac_x);
        let r_star = if t < 0 { 0 } else { (t / dx) as usize }.min(bh);
        let n = bh - r_star;
        if n == 0 {
            continue;
        }
        let y20 = -((c as i32 + 1) * dy); // y2 at r=0
        let shift = ((y20 << up_l) & 0x3f) >> 1;
        let s2 = (shift * 2) as u16;
        let w0 = 64 - s2;
        // Same signed-until-real-lane rule as pass A: `y20 >> frac_y` is the
        // r=0 index, which can be far below the window while the first left
        // lane at `r*` is inside it.
        let bi = (origin as i32 + (y20 >> frac_y) + (r_star * step_y) as i32) as usize;
        let mut k = 0usize;
        while k + 8 <= n {
            let (a0, a1) = dr_z2_hbd_load_pairs_neon(_token, left, bi + k * step_y, up_l == 1);
            let out = dr_z2_hbd_interp_neon(_token, a0, a1, s2, w0, bd_max_v);
            let mut tmp = [0u16; 8];
            vst1q_u16(&mut tmp, out);
            for j in 0..8 {
                dst[(r_star + k + j) * dst_stride + c] = tmp[j];
            }
            k += 8;
        }
        if k < n {
            let (a0, a1) = dr_z2_hbd_load_pairs_neon(_token, left, bi + k * step_y, up_l == 1);
            let out = dr_z2_hbd_interp_neon(_token, a0, a1, s2, w0, bd_max_v);
            let mut tmp = [0u16; 8];
            vst1q_u16(&mut tmp, out);
            for j in 0..n - k {
                dst[(r_star + k + j) * dst_stride + c] = tmp[j];
            }
        }
    }
}

/// C `svt_av1_highbd_dr_prediction_z3_c` (C_DEFAULT/intra_prediction_c.c:
/// 64-93). Shares `intra_pred::dr_z3_edged`'s incremental-accumulator
/// structure exactly (verified line-for-line against the C).
fn dr_z3_edged_hbd(
    dst: &mut [u16],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    left: &[u16],
    origin: usize,
    upsample_left: bool,
    dy: i32,
    bd: u8,
) {
    // Column-wise mirror of [`dr_z1_edged_hbd`]'s flat gate: the arm reads
    // `left[origin + base + r .. r + 9]` with `base + r < max_base_y`, and
    // `bh >= 8` covers one full 8-lane u16 chunk.
    if bh >= 8 && !upsample_left && left.len() > origin + bw + bh {
        incant!(
            dr_z3_edged_hbd_flat(dst, dst_stride, bw, bh, left, origin, dy, bd),
            [neon, scalar]
        );
        return;
    }
    dr_z3_edged_hbd_core(dst, dst_stride, bw, bh, left, origin, upsample_left, dy, bd);
}

#[allow(clippy::too_many_arguments)]
fn dr_z3_edged_hbd_flat_scalar(
    _token: ScalarToken,
    dst: &mut [u16],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    left: &[u16],
    origin: usize,
    dy: i32,
    bd: u8,
) {
    dr_z3_edged_hbd_core(dst, dst_stride, bw, bh, left, origin, false, dy, bd);
}

/// aarch64 arm of [`dr_z3_edged_hbd`], non-upsampled columns: the same
/// u32 widening `a1*2s + a0*(64-2s)` + `vrshrn_n_u32::<6>` + `vmin(bd_max)`
/// interpolation as [`dr_z1_edged_hbd_flat_neon`], computed vertically then
/// scattered — strided column stores cannot vectorize, the same shape
/// [`crate::intra_pred::dr_z3_edged_flat_neon`] uses.
#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn dr_z3_edged_hbd_flat_neon(
    _token: NeonToken,
    dst: &mut [u16],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    left: &[u16],
    origin: usize,
    dy: i32,
    bd: u8,
) {
    let max_base_y = (bw + bh) as i32 - 1;
    let fill = left[origin + max_base_y as usize];
    // Same max as `clip_pixel_highbd`: non-{10,12} bd groups to 255.
    let bd_max: u16 = match bd {
        10 => 1023,
        12 => 4095,
        _ => 255,
    };
    let bd_max_v = vdupq_n_u16(bd_max);
    let mut y = dy;
    for c in 0..bw {
        let base = y >> 6;
        let shift = ((y & 0x3f) >> 1) as u32;
        let s2 = (shift * 2) as u16;
        let w0 = 64 - s2;
        let bi = origin + base as usize;
        let valid = if base >= max_base_y {
            0
        } else {
            core::cmp::min(bh, (max_base_y - base) as usize)
        };
        let mut r = 0usize;
        let mut tmp = [0u16; 8];
        while r + 8 <= valid {
            let a0 = vld1q_u16(left[bi + r..bi + r + 8].try_into().unwrap());
            let a1 = vld1q_u16(left[bi + r + 1..bi + r + 9].try_into().unwrap());
            let lo = vmlal_n_u16(vmull_n_u16(vget_low_u16(a1), s2), vget_low_u16(a0), w0);
            let hi = vmlal_n_u16(vmull_n_u16(vget_high_u16(a1), s2), vget_high_u16(a0), w0);
            let out = vminq_u16(
                vcombine_u16(vrshrn_n_u32::<6>(lo), vrshrn_n_u32::<6>(hi)),
                bd_max_v,
            );
            vst1q_u16(&mut tmp, out);
            for (k, &v) in tmp.iter().enumerate() {
                dst[(r + k) * dst_stride + c] = v;
            }
            r += 8;
        }
        let sh = shift as i32;
        while r < valid {
            let v = (left[bi + r] as i32 * (32 - sh) + left[bi + r + 1] as i32 * sh + 16) >> 5;
            dst[r * dst_stride + c] = (v as u16).min(bd_max);
            r += 1;
        }
        while r < bh {
            dst[r * dst_stride + c] = fill;
            r += 1;
        }
        y += dy;
    }
}

#[allow(clippy::too_many_arguments)]
fn dr_z3_edged_hbd_core(
    dst: &mut [u16],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    left: &[u16],
    origin: usize,
    upsample_left: bool,
    dy: i32,
    bd: u8,
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
                dst[r * dst_stride + c] = clip_pixel_highbd((val + 16) >> 5, bd);
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

/// C `svt_aom_highbd_dr_predictor` (intra_prediction.c:2437-2457), over
/// edged buffers exactly as `intra_pred::dr_predictor_edged` (origin
/// convention documented there — `above[origin - 1] == left[origin - 1]`
/// is the top-left sample).
#[allow(clippy::too_many_arguments)]
pub fn dr_predictor_edged_hbd(
    dst: &mut [u16],
    dst_stride: usize,
    above: &[u16],
    left: &[u16],
    origin: usize,
    upsample_above: bool,
    upsample_left: bool,
    width: usize,
    height: usize,
    angle: i32,
    bd: u8,
) {
    let dx = get_dx_hbd(angle);
    let dy = get_dy_hbd(angle);
    if angle > 0 && angle < 90 {
        dr_z1_edged_hbd(
            dst,
            dst_stride,
            width,
            height,
            above,
            origin,
            upsample_above,
            dx,
            bd,
        );
    } else if angle > 90 && angle < 180 {
        dr_z2_edged_hbd(
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
            bd,
        );
    } else if angle > 180 && angle < 270 {
        dr_z3_edged_hbd(
            dst,
            dst_stride,
            width,
            height,
            left,
            origin,
            upsample_left,
            dy,
            bd,
        );
    } else if angle == 90 {
        predict_v_hbd(dst, dst_stride, &above[origin..], width, height);
    } else if angle == 180 {
        predict_h_hbd(dst, dst_stride, &left[origin..], width, height);
    }
}

// =============================================================================
// 4. Filter-intra prediction, highbd.
// C: intra_prediction.c:2549-2595 (`svt_aom_highbd_filter_intra_predictor`).
// Mirrors `intra_pred::predict_filter_intra` exactly (same 33x33 staging
// buffer, same tap-table indexing), swapping `.clamp(0, 255)` for
// `clip_pixel_highbd(.., bd)`.
//
// PORT-NOTE(unverified): verify vs FFI parity once wired.
// =============================================================================

const FILTER_INTRA_SCALE_BITS_HBD: i32 = 4;

/// Duplicated from `intra_pred.rs`'s private `FILTER_INTRA_TAPS` (not
/// `pub`; same C provenance, `eb_av1_filter_intra_taps`, bd-independent
/// integer coefficients).
#[rustfmt::skip]
static FILTER_INTRA_TAPS_HBD: [[[i8; 8]; 8]; 5] = [
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
fn round_power_of_two_signed_hbd(value: i32, n: i32) -> i32 {
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
fn predict_filter_intra_hbd_impl_scalar(
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
fn predict_filter_intra_hbd_core(
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
fn filter_intra_4x2_neon(
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
fn predict_filter_intra_hbd_impl_neon(
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

fn cfl_luma_subsampling_420_hbd_impl_scalar(
    _token: ScalarToken,
    luma: &[u16],
    luma_stride: usize,
    output_q3: &mut [i16],
    width: usize,
    height: usize,
) {
    cfl_luma_subsampling_420_hbd_core(luma, luma_stride, output_q3, width, height);
}

fn cfl_luma_subsampling_420_hbd_core(
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
fn cfl_luma_subsampling_420_hbd_impl_neon(
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
fn get_scaled_luma_q0_hbd(alpha_q3: i32, pred_buf_q3_val: i16) -> i32 {
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
fn cfl_predict_hbd_impl_scalar(
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
fn cfl_predict_hbd_core(
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
fn cfl_predict_hbd_impl_neon(
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

use crate::loop_filter::LfThresh;

/// C `signed_char_clamp_high` (deblocking_common.c:34-43). C's return type
/// is `int16_t`; kept as `i32` here purely for arithmetic convenience in
/// the surrounding i32 expressions (the clamp ranges below always fit i16).
#[inline]
fn signed_char_clamp_high(t: i32, bd: u8) -> i32 {
    match bd {
        10 => t.clamp(-512, 511),
        12 => t.clamp(-2048, 2047),
        _ => t.clamp(-128, 127),
    }
}

/// C `highbd_hev_mask` (deblocking_common.c:431-437).
#[inline]
fn highbd_hev_mask(thresh: u8, p1: u16, p0: u16, q0: u16, q1: u16, bd: u8) -> i16 {
    let thresh16 = (thresh as i32) << (bd as i32 - 8);
    let mut hev: i16 = 0;
    hev |= -((((p1 as i32) - (p0 as i32)).abs() > thresh16) as i16);
    hev |= -((((q1 as i32) - (q0 as i32)).abs() > thresh16) as i16);
    hev
}

/// C `highbd_filter_mask2` (deblocking_common.c:389-398) for 4-tap edges.
#[inline]
fn highbd_filter_mask2(limit: u8, blimit: u8, p1: u16, p0: u16, q0: u16, q1: u16, bd: u8) -> i8 {
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
fn highbd_filter_mask(
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
fn highbd_filter_mask3_chroma(
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
fn highbd_flat_mask3_chroma(
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
fn highbd_flat_mask4(
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
fn highbd_filter4(mask: i8, thresh: u8, w: &mut [u16; 4], bd: u8) {
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
fn rpot3_hbd(x: i32) -> u16 {
    ((x + 4) >> 3) as u16
}

/// C `ROUND_POWER_OF_TWO(x, 4)`, widened to u16 output.
#[inline]
fn rpot4_hbd(x: i32) -> u16 {
    ((x + 8) >> 4) as u16
}

/// C `highbd_filter6` (deblocking_common.c:692-706) on a `[p2, p1, p0, q0,
/// q1, q2]` window.
fn highbd_filter6(mask: i8, thresh: u8, flat: i8, w: &mut [u16; 6], bd: u8) {
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
fn highbd_filter8(mask: i8, thresh: u8, flat: i8, w: &mut [u16; 8], bd: u8) {
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
fn highbd_filter14(mask: i8, thresh: u8, flat: i8, flat2: i8, w: &mut [u16; 14], bd: u8) {
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
fn gather16<const N: usize>(buf: &[u16], base: usize, step: usize) -> [u16; N] {
    let mut w = [0u16; N];
    let start = base - (N / 2) * step;
    for (k, s) in w.iter_mut().enumerate() {
        *s = buf[start + k * step];
    }
    w
}

/// Scatter the window back (inverse of [`gather16`]).
#[inline]
fn scatter16<const N: usize>(buf: &mut [u16], base: usize, step: usize, w: &[u16; N]) {
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
fn lpf14_window_hbd(w: &mut [u16; 14], t: LfThresh, bd: u8) {
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

use crate::cdef::{BLOCK_4X8, BLOCK_8X4, BLOCK_8X8, CDEF_BSTRIDE, CDEF_VERY_LARGE};
use archmage::prelude::*;

/// Duplicated from `crate::cdef`'s private `CDEF_DIRECTIONS_PADDED` (not
/// `pub`) — identical values, same C provenance
/// (`eb_cdef_directions_padded`, cdef.c:35).
const CDEF_DIRECTIONS_PADDED_HBD: [[i32; 2]; 12] = {
    const S: i32 = CDEF_BSTRIDE as i32;
    [
        [S, 2 * S],
        [S, 2 * S - 1],
        [-S + 1, -2 * S + 2],
        [1, -S + 2],
        [1, 2],
        [1, S + 2],
        [S + 1, 2 * S + 2],
        [S, 2 * S + 1],
        [S, 2 * S],
        [S, 2 * S - 1],
        [-S + 1, -2 * S + 2],
        [1, -S + 2],
    ]
};

#[inline]
fn cdef_direction_hbd(dir: i32, k: usize) -> i32 {
    CDEF_DIRECTIONS_PADDED_HBD[(dir + 2) as usize][k]
}

/// Duplicated from `crate::cdef`'s private `CDEF_PRI_TAPS`/`CDEF_SEC_TAPS`
/// (cdef.c:189-190).
const CDEF_PRI_TAPS_HBD: [[i32; 2]; 2] = [[4, 2], [3, 3]];
const CDEF_SEC_TAPS_HBD: [[i32; 2]; 2] = [[2, 1], [2, 1]];

/// C `get_msb` (definitions.h:603). Duplicated from `crate::cdef`'s
/// private `get_msb`.
#[inline]
fn get_msb_hbd(n: u32) -> i32 {
    debug_assert!(n != 0);
    31 - n.leading_zeros() as i32
}

/// C `constrain` (cdef.c:20). Duplicated from `crate::cdef`'s private
/// `constrain`.
#[inline]
fn constrain_hbd(diff: i32, threshold: i32, damping: i32) -> i32 {
    if threshold == 0 {
        return 0;
    }
    let shift = (damping - get_msb_hbd(threshold as u32)).max(0);
    let sign = if diff < 0 { -1 } else { 1 };
    sign * diff.abs().min((threshold - (diff.abs() >> shift)).max(0))
}

/// `svt_cdef_filter_block_c` (cdef.c:193-254), `dst16` arm: identical
/// arithmetic to `crate::cdef::cdef_filter_block` (the `dst8` arm), storing
/// into a `u16` output instead. See the section doc — this is a pure
/// store-type variant, not a new algorithm.
#[allow(clippy::too_many_arguments)]
pub fn cdef_filter_block_hbd(
    dst: &mut [u16],
    doff: usize,
    dstride: usize,
    inb: &[u16],
    ioff: usize,
    pri_strength: i32,
    sec_strength: i32,
    dir: i32,
    pri_damping: i32,
    sec_damping: i32,
    bsize: i32,
    coeff_shift: i32,
    subsampling_factor: usize,
) {
    incant!(
        cdef_filter_block_hbd_impl(
            dst,
            doff,
            dstride,
            inb,
            ioff,
            pri_strength,
            sec_strength,
            dir,
            pri_damping,
            sec_damping,
            bsize,
            coeff_shift,
            subsampling_factor
        ),
        [v3, neon, scalar]
    )
}

#[allow(clippy::too_many_arguments)]
fn cdef_filter_block_hbd_impl_scalar(
    _token: ScalarToken,
    dst: &mut [u16],
    doff: usize,
    dstride: usize,
    inb: &[u16],
    ioff: usize,
    pri_strength: i32,
    sec_strength: i32,
    dir: i32,
    pri_damping: i32,
    sec_damping: i32,
    bsize: i32,
    coeff_shift: i32,
    subsampling_factor: usize,
) {
    cdef_filter_block_hbd_core(
        dst,
        doff,
        dstride,
        inb,
        ioff,
        pri_strength,
        sec_strength,
        dir,
        pri_damping,
        sec_damping,
        bsize,
        coeff_shift,
        subsampling_factor,
    );
}

/// NEON dst16 CDEF filter.
///
/// Mirrors the AVX2 arm exactly, and shares its column kernel: the filtered
/// values are produced by `cdef::cdef_filter_cols8_neon` — already proven
/// byte-identical to C by `tests/c_parity_cdef.rs` — and this differs from the
/// dst8 arm only in the output cast (`as u16` rather than `as u8`).
///
/// Only the `cols == 8` shapes take the vector path; the 4-wide chroma shapes
/// fall back to the scalar core, same as AVX2.
#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn cdef_filter_block_hbd_impl_neon(
    token: NeonToken,
    dst: &mut [u16],
    doff: usize,
    dstride: usize,
    inb: &[u16],
    ioff: usize,
    pri_strength: i32,
    sec_strength: i32,
    dir: i32,
    pri_damping: i32,
    sec_damping: i32,
    bsize: i32,
    coeff_shift: i32,
    subsampling_factor: usize,
) {
    let cols = if bsize == BLOCK_8X8 || bsize == BLOCK_8X4 {
        8
    } else {
        4
    };
    if cols != 8 {
        cdef_filter_block_hbd_core(
            dst,
            doff,
            dstride,
            inb,
            ioff,
            pri_strength,
            sec_strength,
            dir,
            pri_damping,
            sec_damping,
            bsize,
            coeff_shift,
            subsampling_factor,
        );
        return;
    }
    let rows = if bsize == BLOCK_8X8 || bsize == BLOCK_4X8 {
        8
    } else {
        4
    };
    let mut scratch = [0i32; 64];
    crate::cdef::cdef_filter_cols8_neon(
        token,
        inb,
        ioff,
        pri_strength,
        sec_strength,
        dir,
        pri_damping,
        sec_damping,
        coeff_shift,
        rows,
        subsampling_factor as i32,
        &mut scratch,
    );
    let mut i = 0i32;
    while i < rows {
        let drow = doff + i as usize * dstride;
        let srow = i as usize * 8;
        for j in 0..8usize {
            dst[drow + j] = scratch[srow + j] as u16;
        }
        i += subsampling_factor as i32;
    }
}

/// AVX2 dst16 filter — the bd10/bd12 CDEF search's per-block filter. Byte-identical
/// to [`cdef_filter_block_hbd_core`]; reuses the shared 8-lane compute
/// ([`crate::cdef::cdef_filter_cols8_v3`]) since the dst16 arm differs from dst8
/// only in the output store type (`as u16` vs `as u8`).
#[cfg(target_arch = "x86_64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn cdef_filter_block_hbd_impl_v3(
    token: Desktop64,
    dst: &mut [u16],
    doff: usize,
    dstride: usize,
    inb: &[u16],
    ioff: usize,
    pri_strength: i32,
    sec_strength: i32,
    dir: i32,
    pri_damping: i32,
    sec_damping: i32,
    bsize: i32,
    coeff_shift: i32,
    subsampling_factor: usize,
) {
    let cols = if bsize == BLOCK_8X8 || bsize == BLOCK_8X4 {
        8
    } else {
        4
    };
    if cols != 8 {
        cdef_filter_block_hbd_core(
            dst,
            doff,
            dstride,
            inb,
            ioff,
            pri_strength,
            sec_strength,
            dir,
            pri_damping,
            sec_damping,
            bsize,
            coeff_shift,
            subsampling_factor,
        );
        return;
    }
    let rows = if bsize == BLOCK_8X8 || bsize == BLOCK_4X8 {
        8
    } else {
        4
    };
    let mut scratch = [0i32; 64];
    crate::cdef::cdef_filter_cols8_v3(
        token,
        inb,
        ioff,
        pri_strength,
        sec_strength,
        dir,
        pri_damping,
        sec_damping,
        coeff_shift,
        rows,
        subsampling_factor as i32,
        &mut scratch,
    );
    let mut i = 0i32;
    while i < rows {
        let drow = doff + i as usize * dstride;
        let srow = i as usize * 8;
        for j in 0..8usize {
            dst[drow + j] = scratch[srow + j] as u16;
        }
        i += subsampling_factor as i32;
    }
}

/// Scalar reference body for [`cdef_filter_block_hbd`] (`svt_cdef_filter_block_c`
/// dst16 arm). The AVX2 path is proven byte-identical to this against real C in
/// `tests/c_parity_cdef.rs`.
#[allow(clippy::too_many_arguments)]
fn cdef_filter_block_hbd_core(
    dst: &mut [u16],
    doff: usize,
    dstride: usize,
    inb: &[u16],
    ioff: usize,
    pri_strength: i32,
    sec_strength: i32,
    dir: i32,
    pri_damping: i32,
    sec_damping: i32,
    bsize: i32,
    coeff_shift: i32,
    subsampling_factor: usize,
) {
    let s = CDEF_BSTRIDE as i32;
    let pri_taps = CDEF_PRI_TAPS_HBD[((pri_strength >> coeff_shift) & 1) as usize];
    let sec_taps = CDEF_SEC_TAPS_HBD[((pri_strength >> coeff_shift) & 1) as usize];
    let rows = if bsize == BLOCK_8X8 || bsize == BLOCK_4X8 {
        8
    } else {
        4
    };
    let cols = if bsize == BLOCK_8X8 || bsize == BLOCK_8X4 {
        8
    } else {
        4
    };

    let at = |i: i32, j: i32, off: i32| -> u16 { inb[(ioff as i32 + i * s + j + off) as usize] };

    let mut i = 0i32;
    while i < rows {
        for j in 0..cols {
            let mut sum = 0i16;
            let x = at(i, j, 0) as i16;
            let mut max = x as i32;
            let mut min = x as i32;
            for k in 0..2usize {
                let p0 = at(i, j, cdef_direction_hbd(dir, k)) as i16;
                let p1 = at(i, j, -cdef_direction_hbd(dir, k)) as i16;
                sum = sum.wrapping_add(
                    (pri_taps[k] * constrain_hbd(p0 as i32 - x as i32, pri_strength, pri_damping))
                        as i16,
                );
                sum = sum.wrapping_add(
                    (pri_taps[k] * constrain_hbd(p1 as i32 - x as i32, pri_strength, pri_damping))
                        as i16,
                );
                if p0 as u16 != CDEF_VERY_LARGE {
                    max = (p0 as i32).max(max);
                }
                if p1 as u16 != CDEF_VERY_LARGE {
                    max = (p1 as i32).max(max);
                }
                min = (p0 as i32).min(min);
                min = (p1 as i32).min(min);
                let s0 = at(i, j, cdef_direction_hbd(dir + 2, k)) as i16;
                let s1 = at(i, j, -cdef_direction_hbd(dir + 2, k)) as i16;
                let s2 = at(i, j, cdef_direction_hbd(dir - 2, k)) as i16;
                let s3 = at(i, j, -cdef_direction_hbd(dir - 2, k)) as i16;
                if s0 as u16 != CDEF_VERY_LARGE {
                    max = (s0 as i32).max(max);
                }
                if s1 as u16 != CDEF_VERY_LARGE {
                    max = (s1 as i32).max(max);
                }
                if s2 as u16 != CDEF_VERY_LARGE {
                    max = (s2 as i32).max(max);
                }
                if s3 as u16 != CDEF_VERY_LARGE {
                    max = (s3 as i32).max(max);
                }
                min = (s0 as i32).min(min);
                min = (s1 as i32).min(min);
                min = (s2 as i32).min(min);
                min = (s3 as i32).min(min);
                sum = sum.wrapping_add(
                    (sec_taps[k] * constrain_hbd(s0 as i32 - x as i32, sec_strength, sec_damping))
                        as i16,
                );
                sum = sum.wrapping_add(
                    (sec_taps[k] * constrain_hbd(s1 as i32 - x as i32, sec_strength, sec_damping))
                        as i16,
                );
                sum = sum.wrapping_add(
                    (sec_taps[k] * constrain_hbd(s2 as i32 - x as i32, sec_strength, sec_damping))
                        as i16,
                );
                sum = sum.wrapping_add(
                    (sec_taps[k] * constrain_hbd(s3 as i32 - x as i32, sec_strength, sec_damping))
                        as i16,
                );
            }
            let y = (x as i32 + ((8 + sum as i32 - i32::from(sum < 0)) >> 4)).clamp(min, max);
            dst[doff + i as usize * dstride + j as usize] = y as u16;
        }
        i += subsampling_factor as i32;
    }
}

// =============================================================================
// 9. Quant: dc/ac_quant_qtx bit-depth switch shape.
// C: inv_transforms.c:3462-3490 (`svt_aom_dc_quant_qtx`, `svt_aom_ac_
// quant_qtx`), `MAXQ` = definitions.h:1658.
//
// The 256-entry bd10/bd12 qlookup table VALUES are intentionally NOT
// transcribed here (docs/bd10-port-map.md: generate via
// `xtask/transcribe_bd10_qlookup.py`, NOT run by this translation pass) —
// `dc_qlookup_10`/`ac_qlookup_10`/`_12` below are `unimplemented!()`
// placeholders, mirroring the existing pattern in
// `svtav1_encoder::bd10::{dc_qlookup_10, ac_qlookup_10}` (a SEPARATE crate;
// cannot be reused directly, hence this file's own placeholder copies).
//
// The zbin factor (`svt_aom_get_qzbin_factor`, inv_transforms.c:3492-3505)
// is intentionally NOT duplicated — task scope: "qzbin factor already in
// bd10.rs — cross-reference, don't duplicate." See the correctness finding
// immediately below the placeholders.
//
// NOTE on `dc_quant_qtx`/`ac_quant_qtx`: the SWITCH SHAPE is a faithful,
// complete translation of C. The bd10 tables it dispatches to ARE
// transcribed and FFI-verified against real C — but in the ENCODER crate
// (`svtav1_encoder::bd10::{DC,AC}_QLOOKUP_10`, tests/c_parity_bd10_quant.rs),
// not here: this DSP crate cannot depend on the encoder crate. The bd10/bd12
// arms below are still `unimplemented!()` placeholders; wiring them means
// sharing the encoder tables (or relocating them to a common crate) — a
// tracked de-duplication, NOT a re-transcription.
// =============================================================================

use crate::quant_tables::{AC_QLOOKUP_8, DC_QLOOKUP_8};

/// C `MAXQ` (definitions.h:1658).
const MAXQ: i32 = 255;

/// C `svt_aom_dc_quant_qtx` (inv_transforms.c:3462-3475): bit-depth switch
/// dispatching to the per-bd dc qlookup table.
pub fn dc_quant_qtx(qindex: i32, delta: i32, bd: u8) -> i16 {
    let q_clamped = (qindex + delta).clamp(0, MAXQ) as usize;
    match bd {
        8 => DC_QLOOKUP_8[q_clamped],
        10 => dc_qlookup_10(q_clamped as u8),
        12 => dc_qlookup_12(q_clamped as u8),
        _ => unreachable!("bit_depth should be 8, 10, or 12 (inv_transforms.c:3471-3472 assert)"),
    }
}

/// C `svt_aom_ac_quant_qtx` (inv_transforms.c:3477-3490): bit-depth switch
/// dispatching to the per-bd ac qlookup table.
pub fn ac_quant_qtx(qindex: i32, delta: i32, bd: u8) -> i16 {
    let q_clamped = (qindex + delta).clamp(0, MAXQ) as usize;
    match bd {
        8 => AC_QLOOKUP_8[q_clamped],
        10 => ac_qlookup_10(q_clamped as u8),
        12 => ac_qlookup_12(q_clamped as u8),
        _ => unreachable!("bit_depth should be 8, 10, or 12 (inv_transforms.c:3486-3487 assert)"),
    }
}

/// C `dc_qlookup_10_QTX` (inv_transforms.c:3425-3459), 256 entries.
///
/// The transcribed body lives (and is FFI-verified) at
/// `svtav1_encoder::bd10::DC_QLOOKUP_10` — this DSP crate cannot depend on the
/// encoder crate, so this placeholder stays until the tables are relocated to
/// a shared crate. Wire, don't re-transcribe.
pub fn dc_qlookup_10(_qindex: u8) -> i16 {
    unimplemented!("bd10 DC qlookup lives in svtav1_encoder::bd10 (FFI-verified); share it here")
}

/// C `ac_qlookup_10_QTX` (inv_transforms.c:3373-3423), 256 entries.
///
/// See [`dc_qlookup_10`] — the FFI-verified body is
/// `svtav1_encoder::bd10::AC_QLOOKUP_10`.
pub fn ac_qlookup_10(_qindex: u8) -> i16 {
    unimplemented!("bd10 AC qlookup lives in svtav1_encoder::bd10 (FFI-verified); share it here")
}

/// bd12 is OUT OF SCOPE for this port (docs/bd10-port-map.md: "bd 8 or 10
/// only"); kept only so `dc_quant_qtx`'s switch shape matches C's real
/// 3-arm dispatch. PORT-NOTE(unverified): never intended to be transcribed
/// under this task.
pub fn dc_qlookup_12(_qindex: u8) -> i16 {
    unimplemented!("bd12 out of scope per docs/bd10-port-map.md")
}

/// See [`dc_qlookup_12`].
pub fn ac_qlookup_12(_qindex: u8) -> i16 {
    unimplemented!("bd12 out of scope per docs/bd10-port-map.md")
}

// -----------------------------------------------------------------------------
// Correctness finding (cross-check, NOT fixed here — out of this file's
// scope; `svtav1_encoder::bd10` is a sibling crate this translation pass
// does not touch):
//
// `svtav1_encoder::bd10::qzbin_factor(dc_quant_q3, bd)`'s `else` arm
// returns `64`, but C's actual else arm returns `80`
// (`svt_aom_get_qzbin_factor`, inv_transforms.c:3492-3505: `quant < 148 ?
// 84 : 80` for bd8, and the analogous `84 : 80` shape for bd10/bd12 — NOT
// `84 : 64`). C also special-cases `q == 0 -> 64` UNCONDITIONALLY before
// even looking at `quant`; the sibling function's signature has no `q`
// parameter at all, so it structurally cannot reproduce that special case.
// This looks like a real, pre-existing bug in that (already UNWIRED,
// unverified) sibling module. Flagged here because this task's own item 6
// explicitly cross-references that function; NOT corrected in this pass
// (scope is the new `hbd.rs` module only) — see project CLAUDE.md's
// UNWIRED index entry for `bd10.rs` for the tracking note.
// -----------------------------------------------------------------------------

#[cfg(test)]
mod dispatch_tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    /// Scalar reference routing for [`dr_predictor_edged_hbd`]: the private
    /// cores directly, so every tier of the dispatched entry is compared
    /// against the C-translated scalar bodies (z1/z3 flat NEON arms vs the
    /// same core the non-gated shapes still take).
    #[allow(clippy::too_many_arguments)]
    fn dr_edged_ref(
        dst: &mut [u16],
        dst_stride: usize,
        above: &[u16],
        left: &[u16],
        origin: usize,
        upsample_above: bool,
        upsample_left: bool,
        width: usize,
        height: usize,
        angle: i32,
        bd: u8,
    ) {
        let dx = get_dx_hbd(angle);
        let dy = get_dy_hbd(angle);
        if angle > 0 && angle < 90 {
            dr_z1_edged_hbd_core(
                dst,
                dst_stride,
                width,
                height,
                above,
                origin,
                upsample_above,
                dx,
                bd,
            );
        } else if angle > 90 && angle < 180 {
            dr_z2_edged_hbd_core(
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
                bd,
            );
        } else if angle > 180 && angle < 270 {
            dr_z3_edged_hbd_core(
                dst,
                dst_stride,
                width,
                height,
                left,
                origin,
                upsample_left,
                dy,
                bd,
            );
        } else {
            panic!("test only sweeps z1/z2/z3");
        }
    }

    /// Every dispatched tier of `dr_predictor_edged_hbd` must produce the
    /// scalar core's output on every size/angle/upsample/bd combination —
    /// including the gated flat arms (bw/bh >= 8, non-upsampled) and the
    /// shapes that still fall through to scalar. Consumes the
    /// `PermutationReport` (empty warnings, >= 2 permutations).
    #[test]
    fn dr_edged_hbd_z1_z3_all_tiers_match_core() {
        use archmage::testing::{CompileTimePolicy, for_each_token_permutation};
        let mut st = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            st ^= st << 13;
            st ^= st >> 7;
            st ^= st << 17;
            (st >> 33) as u16
        };
        // Edge buffers are 512-deep: upsampled reads can reach past
        // EDGE_BUF_LEN on oversized shapes (the same harmless over-read the
        // u8 sibling and C perform), so the test buffers carry headroom.
        const BUF: usize = 512;
        const ORIGIN: usize = 16;
        // z1 angles with nonzero DR_INTRA_DERIVATIVE_HBD entries, plus the
        // z3 mirrors (270 - a) and z2 angles spanning shallow to steep
        // slopes. Real AV1 mode angles, not synthetic extremes.
        let z1_angles = [3i32, 9, 26, 45, 57, 72, 84, 87];
        // Only angles where BOTH 180-a and a-90 have nonzero derivative
        // entries are legal z2 modes (dx=0/dy=0 is contract-invalid).
        let z2_angles = [93i32, 99, 126, 132, 135, 141, 144, 174, 177];
        let z3_angles = [183i32, 189, 201, 219, 237, 255, 264, 267];
        let sizes = [
            (4usize, 4usize),
            (8, 4),
            (4, 8),
            (8, 8),
            (8, 16),
            (16, 8),
            (16, 16),
            (32, 8),
            (8, 32),
            (32, 32),
            (64, 16),
            (16, 64),
            (64, 64),
        ];
        for &bd in &[8u8, 10, 12] {
            let max_sample = (1u16 << bd.min(12)) - 1;
            for &(w, h) in &sizes {
                for &up in &[(false, false), (true, false), (false, true), (true, true)] {
                    let above: Vec<u16> = (0..BUF).map(|_| next() & max_sample).collect();
                    let left: Vec<u16> = (0..BUF).map(|_| next() & max_sample).collect();
                    for &angle in z1_angles
                        .iter()
                        .chain(z2_angles.iter())
                        .chain(z3_angles.iter())
                    {
                        for pad in [0usize, 3] {
                            let stride = w + pad;
                            let rep =
                                for_each_token_permutation(CompileTimePolicy::WarnStderr, |perm| {
                                    let mut got = vec![0xAAAAu16; stride * h];
                                    let mut want = vec![0xBBBBu16; stride * h];
                                    dr_predictor_edged_hbd(
                                        &mut got, stride, &above, &left, ORIGIN, up.0, up.1, w, h,
                                        angle, bd,
                                    );
                                    dr_edged_ref(
                                        &mut want, stride, &above, &left, ORIGIN, up.0, up.1, w, h,
                                        angle, bd,
                                    );
                                    for r in 0..h {
                                        assert_eq!(
                                            &got[r * stride..r * stride + w],
                                            &want[r * stride..r * stride + w],
                                            "dr hbd {w}x{h} angle {angle} up {up:?} bd {bd} \
                                             stride {stride} row {r} tier {perm}"
                                        );
                                    }
                                });
                            assert!(
                                rep.warnings.is_empty(),
                                "tokens excluded at compile time: {:?}",
                                rep.warnings
                            );
                            assert!(
                                rep.permutations_run >= 2,
                                "only {} permutation(s) ran",
                                rep.permutations_run
                            );
                        }
                    }
                }
            }
        }
    }

    /// The dispatched `predict_dc_hbd` must equal `predict_dc_hbd_core` on
    /// every size, flag combination and stride — the u8 sibling's
    /// `predict_dc_dispatch_matches_core_all_sizes_flags` for the u16 arm.
    #[test]
    fn predict_dc_hbd_dispatch_matches_core_all_sizes_flags() {
        use archmage::testing::{CompileTimePolicy, for_each_token_permutation};
        let mut st = 0xDEAD_BEEF_CAFE_F00Du64;
        let mut next = move || {
            st ^= st << 13;
            st ^= st >> 7;
            st ^= st << 17;
            (st >> 33) as u16
        };
        for &bd in &[8u8, 10, 12] {
            let max_sample = (1u16 << bd.min(12)) - 1;
            for &(w, h) in &[
                (4usize, 4usize),
                (8, 8),
                (16, 16),
                (32, 32),
                (64, 64),
                (4, 8),
                (8, 4),
                (16, 8),
                (8, 16),
                (32, 8),
                (64, 16),
                (16, 64),
            ] {
                for &(ha, hl) in &[(true, true), (true, false), (false, true), (false, false)] {
                    let above: Vec<u16> = (0..w).map(|_| next() & max_sample).collect();
                    let left: Vec<u16> = (0..h).map(|_| next() & max_sample).collect();
                    for pad in [0usize, 5] {
                        let stride = w + pad;
                        let rep =
                            for_each_token_permutation(CompileTimePolicy::WarnStderr, |perm| {
                                let mut got = vec![0xAAAAu16; stride * h];
                                let mut want = vec![0xBBBBu16; stride * h];
                                predict_dc_hbd(&mut got, stride, &above, &left, w, h, ha, hl, bd);
                                predict_dc_hbd_core(
                                    &mut want, stride, &above, &left, w, h, ha, hl, bd,
                                );
                                for r in 0..h {
                                    assert_eq!(
                                        &got[r * stride..r * stride + w],
                                        &want[r * stride..r * stride + w],
                                        "dc hbd {w}x{h} flags ({ha},{hl}) bd {bd} stride {stride} \
                                         row {r} tier {perm}"
                                    );
                                }
                            });
                        assert!(
                            rep.warnings.is_empty(),
                            "tokens excluded at compile time: {:?}",
                            rep.warnings
                        );
                        assert!(
                            rep.permutations_run >= 2,
                            "only {} permutation(s) ran",
                            rep.permutations_run
                        );
                    }
                }
            }
        }
    }

    /// Every dispatched tier of the hbd paeth/smooth/filter-intra/CfL arms
    /// must produce the scalar core's output — including the non-multiple-of-4
    /// paeth widths that exercise the scalar tails (6, 10, 18 are not real
    /// AV1 sizes but the paeth kernel is generic and the tail is load-bearing).
    /// Consumes the `PermutationReport` (empty warnings, >= 2 permutations).
    #[test]
    fn hbd_predictors_all_tiers_match_core() {
        use archmage::testing::{CompileTimePolicy, for_each_token_permutation};
        let scalar = ScalarToken::summon().unwrap();
        let mut st = 0x243F_6A88_85A3_08D3u64;
        let mut next = move || {
            st ^= st << 13;
            st ^= st >> 7;
            st ^= st << 17;
            (st >> 33) as u16
        };
        // Paeth has no weight tables — generic widths (6/10/18) exercise the
        // NEON tails. The smooth predictors index `smooth_weights_hbd(n)`
        // which only exists for real AV1 sizes {4,8,16,32,64}.
        for &bd in &[10u8, 12, 8] {
            let max_sample = (1u16 << bd.min(12)) - 1;
            for &(w, h) in &[
                (4usize, 4usize),
                (6, 4),
                (8, 8),
                (10, 8),
                (16, 16),
                (18, 8),
                (32, 16),
                (4, 32),
            ] {
                let above: Vec<u16> = (0..w + 1).map(|_| next() & max_sample).collect();
                let left: Vec<u16> = (0..h.max(2)).map(|_| next() & max_sample).collect();
                for pad in [0usize, 3] {
                    let stride = w + pad;
                    let rep = for_each_token_permutation(CompileTimePolicy::WarnStderr, |perm| {
                        let mut got = vec![0xAAAAu16; stride * h];
                        let mut want = vec![0xBBBBu16; stride * h];
                        let tl = next() & max_sample;
                        predict_paeth_hbd(&mut got, stride, &above[1..], &left, tl, w, h);
                        predict_paeth_hbd_impl_scalar(
                            scalar,
                            &mut want,
                            stride,
                            &above[1..],
                            &left,
                            tl,
                            w,
                            h,
                        );
                        for r in 0..h {
                            assert_eq!(
                                &got[r * stride..r * stride + w],
                                &want[r * stride..r * stride + w],
                                "paeth {w}x{h} bd {bd} stride {stride} row {r} tier {perm}"
                            );
                        }
                    });
                    assert!(
                        rep.warnings.is_empty(),
                        "tokens excluded at compile time: {:?}",
                        rep.warnings
                    );
                    assert!(
                        rep.permutations_run >= 2,
                        "only {} permutation(s) ran",
                        rep.permutations_run
                    );
                }
            }
            for &(w, h) in &[
                (4usize, 4usize),
                (8, 8),
                (16, 16),
                (32, 16),
                (4, 32),
                (64, 8),
            ] {
                let above: Vec<u16> = (0..w).map(|_| next() & max_sample).collect();
                let left: Vec<u16> = (0..h).map(|_| next() & max_sample).collect();
                for pad in [0usize, 3] {
                    let stride = w + pad;
                    let rep = for_each_token_permutation(CompileTimePolicy::WarnStderr, |perm| {
                        let mut got = vec![0xAAAAu16; stride * h];
                        let mut want = vec![0xBBBBu16; stride * h];

                        predict_smooth_hbd(&mut got, stride, &above, &left, w, h);
                        predict_smooth_hbd_core(&mut want, stride, &above, &left, w, h);
                        for r in 0..h {
                            assert_eq!(
                                &got[r * stride..r * stride + w],
                                &want[r * stride..r * stride + w],
                                "smooth {w}x{h} bd {bd} stride {stride} row {r} tier {perm}"
                            );
                        }

                        got.fill(0xAAAA);
                        want.fill(0xBBBB);
                        predict_smooth_v_hbd(&mut got, stride, &above, &left, w, h);
                        predict_smooth_v_hbd_core(&mut want, stride, &above, &left, w, h);
                        for r in 0..h {
                            assert_eq!(
                                &got[r * stride..r * stride + w],
                                &want[r * stride..r * stride + w],
                                "smooth_v {w}x{h} bd {bd} stride {stride} row {r} tier {perm}"
                            );
                        }

                        got.fill(0xAAAA);
                        want.fill(0xBBBB);
                        predict_smooth_h_hbd(&mut got, stride, &above, &left, w, h);
                        predict_smooth_h_hbd_core(&mut want, stride, &above, &left, w, h);
                        for r in 0..h {
                            assert_eq!(
                                &got[r * stride..r * stride + w],
                                &want[r * stride..r * stride + w],
                                "smooth_h {w}x{h} bd {bd} stride {stride} row {r} tier {perm}"
                            );
                        }
                    });
                    assert!(
                        rep.warnings.is_empty(),
                        "tokens excluded at compile time: {:?}",
                        rep.warnings
                    );
                    assert!(
                        rep.permutations_run >= 2,
                        "only {} permutation(s) ran",
                        rep.permutations_run
                    );
                }
            }
        }

        // filter_intra: 4x2 sub-blocks, w/h <= 32 and multiples of 4, mode < 5.
        for &bd in &[10u8, 12] {
            let max_sample = (1u16 << bd.min(12)) - 1;
            for &(w, h) in &[
                (4usize, 4usize),
                (8, 8),
                (16, 16),
                (32, 32),
                (4, 16),
                (20, 8),
            ] {
                for mode in 0u8..5 {
                    let above: Vec<u16> = (0..w + 1).map(|_| next() & max_sample).collect();
                    let left: Vec<u16> = (0..h).map(|_| next() & max_sample).collect();
                    for pad in [0usize, 3] {
                        let stride = w + pad;
                        let rep =
                            for_each_token_permutation(CompileTimePolicy::WarnStderr, |perm| {
                                let mut got = vec![0xAAAAu16; stride * h];
                                let mut want = vec![0xBBBBu16; stride * h];
                                predict_filter_intra_hbd(
                                    &mut got, stride, &above, &left, w, h, mode, bd,
                                );
                                predict_filter_intra_hbd_core(
                                    &mut want, stride, &above, &left, w, h, mode, bd,
                                );
                                for r in 0..h {
                                    assert_eq!(
                                        &got[r * stride..r * stride + w],
                                        &want[r * stride..r * stride + w],
                                        "filter_intra {w}x{h} mode {mode} bd {bd} stride \
                                         {stride} row {r} tier {perm}"
                                    );
                                }
                            });
                        assert!(
                            rep.warnings.is_empty(),
                            "tokens excluded at compile time: {:?}",
                            rep.warnings
                        );
                        assert!(
                            rep.permutations_run >= 2,
                            "only {} permutation(s) ran",
                            rep.permutations_run
                        );
                    }
                }
            }
        }

        // CfL hbd pair: subsampling (w,h even) and predict (alpha over the
        // signed q3 domain, incl. the bd=8 default-clip arm).
        for &(w, h) in &[
            (4usize, 4usize),
            (8, 8),
            (16, 16),
            (32, 32),
            (6, 4),
            (18, 8),
        ] {
            let luma: Vec<u16> = (0..h * w).map(|_| next() & 4095).collect();
            let rep = for_each_token_permutation(CompileTimePolicy::WarnStderr, |perm| {
                let mut got = vec![0i16; (h / 2 + 1) * crate::intra_pred::CFL_BUF_LINE + w];
                let mut want = vec![0i16; (h / 2 + 1) * crate::intra_pred::CFL_BUF_LINE + w];
                cfl_luma_subsampling_420_hbd(&luma, w, &mut got, w, h);
                cfl_luma_subsampling_420_hbd_core(&luma, w, &mut want, w, h);
                assert_eq!(got, want, "cfl_subsample {w}x{h} tier {perm}");
            });
            assert!(rep.warnings.is_empty(), "{:?}", rep.warnings);
            assert!(rep.permutations_run >= 2);
        }
        for &bd in &[10u8, 12, 8] {
            let max_sample = (1u16 << bd.min(12)) - 1;
            for &(w, h) in &[
                (4usize, 4usize),
                (8, 8),
                (16, 16),
                (32, 16),
                (6, 4),
                (10, 8),
            ] {
                let buf: Vec<i16> = (0..h * crate::intra_pred::CFL_BUF_LINE + w)
                    .map(|_| (next() & 1023) as i16 - 512)
                    .collect();
                let pred: Vec<u16> = (0..h * w).map(|_| next() & max_sample).collect();
                for &alpha in &[-16i32, -3, 0, 5, 16] {
                    for pad in [0usize, 3] {
                        let stride = w + pad;
                        let rep =
                            for_each_token_permutation(CompileTimePolicy::WarnStderr, |perm| {
                                let mut got = vec![0xAAAAu16; stride * h];
                                let mut want = vec![0xBBBBu16; stride * h];
                                cfl_predict_hbd(&buf, &pred, w, &mut got, stride, alpha, bd, w, h);
                                cfl_predict_hbd_core(
                                    &buf, &pred, w, &mut want, stride, alpha, bd, w, h,
                                );
                                for r in 0..h {
                                    assert_eq!(
                                        &got[r * stride..r * stride + w],
                                        &want[r * stride..r * stride + w],
                                        "cfl_predict {w}x{h} alpha {alpha} bd {bd} stride \
                                         {stride} row {r} tier {perm}"
                                    );
                                }
                            });
                        assert!(
                            rep.warnings.is_empty(),
                            "tokens excluded at compile time: {:?}",
                            rep.warnings
                        );
                        assert!(
                            rep.permutations_run >= 2,
                            "only {} permutation(s) ran",
                            rep.permutations_run
                        );
                    }
                }
            }
        }
    }
}
