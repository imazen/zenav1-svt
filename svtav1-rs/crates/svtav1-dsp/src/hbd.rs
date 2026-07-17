//! 10-bit (bd10) DSP kernel layer — bulk source translation (task #94).
//!
//! **UNWIRED.** This module is NOT referenced by `lib.rs` (no `pub mod
//! hbd;` line exists there) and is never compiled as part of the crate.
//! Wiring is the first step of the integration session that picks this up:
//! add `pub mod hbd;` to `crates/svtav1-dsp/src/lib.rs`, then run the FFI
//! parity + bd10 uniform-64 verification described below before anything
//! calls into this file from production code.
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
//! - The sized `svt_aom_highbd_10_variance{W}x{H}_c` family that
//!   `av1me.c`'s `vf_hbd_10` function-pointer table binds
//!   (`av1me.c:24-33`) is declared (`aom_dsp_rtcd.h:582+`) with `_sse2`/
//!   `_avx2`/`_neon`/`_sve` bodies, but repo-wide text search found NO `_c`
//!   scalar body anywhere in this checkout (only `test/HbdVarianceTest.cc`
//!   references). `CONFIG_ENABLE_HIGH_BIT_DEPTH` defaults to **1** in a
//!   normal (non-`RTC_BUILD`) build (`EbConfigMacros.h:89-91`,
//!   `CMakeLists.txt:76` — `RTC_BUILD` defaults `OFF`), and the sizes with
//!   no ASM fallback (4x4/4x8/4x16/8x4) are wired via `SET_ONLY_C` — i.e.
//!   the missing `_c` body would fail to link in a normal build, if this
//!   checkout is actually built standalone. This was NOT verified by an
//!   actual C build/link (read-only source per task scope), so treat as an
//!   open question, not a settled "dead code" claim. [`highbd_variance`]
//!   below ports the one GENERIC (non-sized) highbd variance kernel that
//!   DOES have a real body, `svt_aom_variance_highbd_c`
//!   (C_DEFAULT/variance.c:162-181), and is shift-table-equivalent to the
//!   sized family's math (see that function's doc for the derivation).

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
// PORT-NOTE(unverified) on every function below: verify vs FFI parity once
// wired (see module doc verification plan).
// =============================================================================

/// C `highbd_v_predictor` (intra_prediction.c:1202-1210). `bd` unused (C:
/// `(void)bd;`) — kept in the signature only for call-shape parity with the
/// other hbd predictors and the directional dispatcher.
pub fn predict_v_hbd(dst: &mut [u16], dst_stride: usize, above: &[u16], width: usize, height: usize) {
    for row in 0..height {
        dst[row * dst_stride..row * dst_stride + width].copy_from_slice(&above[..width]);
    }
}

/// C `highbd_h_predictor` (intra_prediction.c:1212-1220). `bd` unused.
pub fn predict_h_hbd(dst: &mut [u16], dst_stride: usize, left: &[u16], width: usize, height: usize) {
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
    for row in 0..height {
        for col in 0..width {
            dst[row * dst_stride + col] = paeth_predictor_single_hbd(left[row], above[col], top_left);
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
    let dc = match (has_above, has_left) {
        (true, true) => {
            let sum: u32 =
                above[..width].iter().map(|&v| v as u32).sum::<u32>() + left[..height].iter().map(|&v| v as u32).sum::<u32>();
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
        for col in 0..width {
            dst[row * dst_stride + col] = dc;
        }
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
    255, 240, 225, 210, 196, 182, 169, 157, 145, 133, 122, 111, 101, 92, 83, 74, 66, 59, 52, 45, 39, 34, 29, 25, 21,
    17, 14, 12, 10, 9, 8, 8,
];
static SM_WEIGHTS_64_HBD: [u8; 64] = [
    255, 248, 240, 233, 225, 218, 210, 203, 196, 189, 182, 176, 169, 163, 156, 150, 144, 138, 133, 127, 121, 116,
    111, 106, 101, 96, 91, 86, 82, 77, 73, 69, 65, 61, 57, 54, 50, 47, 44, 41, 38, 35, 32, 29, 27, 25, 22, 20, 18, 16,
    15, 13, 12, 10, 9, 8, 7, 6, 6, 5, 5, 4, 4, 4,
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
pub fn predict_smooth_hbd(dst: &mut [u16], dst_stride: usize, above: &[u16], left: &[u16], width: usize, height: usize) {
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
            let pred = (wh * top + (256 - wh) * below_pred + ww * lft + (256 - ww) * right_pred + 256) / 512;
            dst[row * dst_stride + col] = pred as u16;
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

/// C `highbd_smooth_h_predictor` (intra_prediction.c:1312-1334). `bd` unused.
pub fn predict_smooth_h_hbd(
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
    0, 0, 0, 1023, 0, 0, 547, 0, 0, 372, 0, 0, 0, 0, 273, 0, 0, 215, 0, 0, 178, 0, 0, 151, 0, 0, 132, 0, 0, 116, 0, 0,
    102, 0, 0, 0, 90, 0, 0, 80, 0, 0, 71, 0, 0, 64, 0, 0, 57, 0, 0, 51, 0, 0, 45, 0, 0, 0, 40, 0, 0, 35, 0, 0, 31, 0,
    0, 27, 0, 0, 23, 0, 0, 19, 0, 0, 15, 0, 0, 0, 0, 11, 0, 0, 7, 0, 0, 3, 0, 0,
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
    let s = (left[origin] as i32 * 5 + above[origin - 1] as i32 * 6 + above[origin] as i32 * 5 + 8) >> 4;
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
        let s = -(input[i] as i32) + 9 * input[i + 1] as i32 + 9 * input[i + 2] as i32 - input[i + 3] as i32;
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
                let val =
                    left[origin + base as usize] as i32 * (32 - shift) + left[origin + base as usize + 1] as i32 * shift;
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
        dr_z1_edged_hbd(dst, dst_stride, width, height, above, origin, upsample_above, dx, bd);
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
        dr_z3_edged_hbd(dst, dst_stride, width, height, left, origin, upsample_left, dy, bd);
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

    let mut buffer = [[0u16; 33]; 33];
    buffer[0][..width + 1].copy_from_slice(&above[..width + 1]);
    for r in 0..height {
        buffer[r + 1][0] = left[r];
    }

    let taps = &FILTER_INTRA_TAPS_HBD[mode as usize];
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
                    clip_pixel_highbd(round_power_of_two_signed_hbd(val, FILTER_INTRA_SCALE_BITS_HBD), bd);
            }
        }
    }

    for r in 0..height {
        dst[r * dst_stride..r * dst_stride + width].copy_from_slice(&buffer[r + 1][1..1 + width]);
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
pub fn cfl_luma_subsampling_420_hbd(luma: &[u16], luma_stride: usize, output_q3: &mut [i16], width: usize, height: usize) {
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
    for j in 0..height {
        for i in 0..width {
            let scaled = get_scaled_luma_q0_hbd(alpha_q3, pred_buf_q3[j * crate::intra_pred::CFL_BUF_LINE + i]);
            let val = scaled + pred[j * pred_stride + i] as i32;
            dst[j * dst_stride + i] = clip_pixel_highbd(val, bd);
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
// PORT-NOTE(unverified) on all three functions below: verify vs FFI parity
// once wired.
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

/// C `svt_aom_variance_highbd_c` (C_DEFAULT/variance.c:162-181): the one
/// GENERIC (non-sized) highbd variance kernel with a discoverable C body in
/// this checkout — see the module doc "Findings" for the sized
/// `svt_aom_highbd_10_variance{W}x{H}_c` family that `av1me.c`'s
/// `vf_hbd_10` function-pointer table (`av1me.c:24-33`, only wired under
/// `CONFIG_ENABLE_HIGH_BIT_DEPTH`) actually binds.
///
/// This generic form's math IS shift-table-equivalent to that sized
/// family: the `_sse2` implementation (`ASM_SSE2/highbd_variance_sse2.c:
/// 54-75`, `VAR_FN` macro) computes `var = sse - ((sum*sum) >> shift)` with
/// `shift = log2(w*h)` for every listed block size, and `(sum*sum) >>
/// shift == (int64_t)sad*sad / (w*h)` for any non-negative `sad*sad` and
/// power-of-two `w*h` (always true for AV1 block sizes) — so this fn
/// should be safe to bind to `vf_hbd_10` once that path is exercised.
///
/// C accumulates BOTH `sad` (as `int`) and `*sse` (as `uint32_t`, via `+=
/// diff*diff` where `diff*diff` is `int` arithmetic) — `sad` cannot
/// overflow `i32` for any legal AV1 block (max magnitude ~4095 * 16384 ≈
/// 67M), but `*sse` genuinely CAN wrap `u32` for large, fully-saturated
/// high-bit-depth blocks (max ~4095² * 16384 ≈ 2.75e11 » u32::MAX) — C's
/// `uint32_t` accumulation is well-defined modular arithmetic, not UB, so
/// this port mirrors the wraparound exactly via `wrapping_add` rather than
/// widening to a type that would silently disagree with C on overflow.
pub fn highbd_variance(a: &[u16], a_stride: usize, b: &[u16], b_stride: usize, w: usize, h: usize) -> (u32, u32) {
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
pub fn highbd_sad_kernel(src: &[u16], src_stride: usize, ref_: &[u16], ref_stride: usize, width: usize, height: usize) -> u32 {
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
// PORT-NOTE(unverified) on every function below: verify vs FFI parity once
// wired (mirroring `tests/c_parity_lpf.rs`'s bd8 coverage).
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
fn highbd_filter_mask3_chroma(limit: u8, blimit: u8, p2: u16, p1: u16, p0: u16, q0: u16, q1: u16, q2: u16, bd: u8) -> i8 {
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
fn highbd_flat_mask3_chroma(thresh: u8, p2: u16, p1: u16, p0: u16, q0: u16, q1: u16, q2: u16, bd: u8) -> i8 {
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
fn highbd_flat_mask4(thresh: u8, p3: u16, p2: u16, p1: u16, p0: u16, q0: u16, q1: u16, q2: u16, q3: u16, bd: u8) -> i8 {
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
        w[6] = rpot4_hbd(p6 + p5 + p4 + p3 + p2 + p1 * 2 + p0 * 2 + q0 * 2 + q1 + q2 + q3 + q4 + q5);
        w[7] = rpot4_hbd(p5 + p4 + p3 + p2 + p1 + p0 * 2 + q0 * 2 + q1 * 2 + q2 + q3 + q4 + q5 + q6);
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
        let mask = highbd_filter_mask3_chroma(t.lim, t.mblim, w[0], w[1], w[2], w[3], w[4], w[5], bd);
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
        let mask = highbd_filter_mask3_chroma(t.lim, t.mblim, w[0], w[1], w[2], w[3], w[4], w[5], bd);
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
        let mask = highbd_filter_mask(t.lim, t.mblim, w[0], w[1], w[2], w[3], w[4], w[5], w[6], w[7], bd);
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
        let mask = highbd_filter_mask(t.lim, t.mblim, w[0], w[1], w[2], w[3], w[4], w[5], w[6], w[7], bd);
        let flat = highbd_flat_mask4(1, w[0], w[1], w[2], w[3], w[4], w[5], w[6], w[7], bd);
        highbd_filter8(mask, t.hev_thr, flat, &mut w, bd);
        scatter16(buf, base, 1, &w);
    }
}

/// Shared 14-tap body on a gathered `[p6..q6]` window. C
/// `highbd_mb_lpf_horizontal_edge_w` / `highbd_mb_lpf_vertical_edge_w`
/// inner loop (deblocking_common.c:629-672 / 741-780).
fn lpf14_window_hbd(w: &mut [u16; 14], t: LfThresh, bd: u8) {
    let mask = highbd_filter_mask(t.lim, t.mblim, w[3], w[4], w[5], w[6], w[7], w[8], w[9], w[10], bd);
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
// PORT-NOTE(unverified): verify vs FFI parity once wired (mirroring
// `tests/c_parity_cdef.rs`'s bd8 coverage, extended to a dst16 assertion).
// =============================================================================

use crate::cdef::{BLOCK_4X8, BLOCK_8X4, BLOCK_8X8, CDEF_BSTRIDE, CDEF_VERY_LARGE};

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
    let s = CDEF_BSTRIDE as i32;
    let pri_taps = CDEF_PRI_TAPS_HBD[((pri_strength >> coeff_shift) & 1) as usize];
    let sec_taps = CDEF_SEC_TAPS_HBD[((pri_strength >> coeff_shift) & 1) as usize];
    let rows = if bsize == BLOCK_8X8 || bsize == BLOCK_4X8 { 8 } else { 4 };
    let cols = if bsize == BLOCK_8X8 || bsize == BLOCK_8X4 { 8 } else { 4 };

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
                sum = sum.wrapping_add((pri_taps[k] * constrain_hbd(p0 as i32 - x as i32, pri_strength, pri_damping)) as i16);
                sum = sum.wrapping_add((pri_taps[k] * constrain_hbd(p1 as i32 - x as i32, pri_strength, pri_damping)) as i16);
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
                sum = sum.wrapping_add((sec_taps[k] * constrain_hbd(s0 as i32 - x as i32, sec_strength, sec_damping)) as i16);
                sum = sum.wrapping_add((sec_taps[k] * constrain_hbd(s1 as i32 - x as i32, sec_strength, sec_damping)) as i16);
                sum = sum.wrapping_add((sec_taps[k] * constrain_hbd(s2 as i32 - x as i32, sec_strength, sec_damping)) as i16);
                sum = sum.wrapping_add((sec_taps[k] * constrain_hbd(s3 as i32 - x as i32, sec_strength, sec_damping)) as i16);
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
// PORT-NOTE(unverified) on `dc_quant_qtx`/`ac_quant_qtx`: the SWITCH SHAPE
// is a faithful, complete translation of C; verify vs FFI parity once the
// qlookup tables are transcribed and wired.
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
/// PORT-NOTE(unverified): table body NOT transcribed — run
/// `xtask/transcribe_bd10_qlookup.py` then replace with
/// `include!("bd10_qlookup_tables.rs")` or equivalent (see the sibling
/// placeholder in `svtav1_encoder::bd10` for the intended shape).
pub fn dc_qlookup_10(_qindex: u8) -> i16 {
    unimplemented!("run xtask/transcribe_bd10_qlookup.py (PORT-NOTE above)")
}

/// C `ac_qlookup_10_QTX` (inv_transforms.c:3373-3423), 256 entries.
///
/// PORT-NOTE(unverified): table body NOT transcribed — see [`dc_qlookup_10`].
pub fn ac_qlookup_10(_qindex: u8) -> i16 {
    unimplemented!("run xtask/transcribe_bd10_qlookup.py (PORT-NOTE above)")
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
