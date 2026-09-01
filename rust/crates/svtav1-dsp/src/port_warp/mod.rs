//! Warped motion — a faithful port of `Codec/warped_motion.c`.
//!
//! # Why this module exists next to `warp.rs`
//!
//! `warp.rs::warp_prediction` is **not** a port of the C kernel and never was:
//! it uses the 16-phase `SUB_PEL_FILTERS_8` table, ignores the shear entirely,
//! has no 8x8 tiling and does a plain `(sum + 64) >> 7` twice instead of the
//! ROUND0/ROUND1 offset scheme. `tests/c_parity_warp.rs` pins that divergence
//! with an `assert_ne!`. `rust/CLAUDE.md`'s 2026-07-14 audit records it
//! correctly as a STUB; `docs/INTER-ENCODE-PLAN.md` section 0 lists warped
//! motion as "ALREADY ported and C-gated", which is wrong — that row is
//! corrected in the same change as this module.
//!
//! This module is the real thing: the normative 193-phase
//! `svt_aom_warped_filter` kernel with shear, 8x8 tiling and the two-stage
//! rounding, plus the model derivation (`find_projection`, `get_shear_params`)
//! and the neighbour-sample selection that feeds it.
//!
//! # Reachability (verified against `enc_mode_config.c`, not inferred)
//!
//! `allow_warped_motion` is 1 for every non-key, non-error-resilient,
//! non-superres frame whose `wm_level != 0`, and `wm_level` is nonzero for
//! base-layer frames at presets 0..11 (`enc_mode_config.c:8968-9006`). No
//! `#if TUNE_*` guards any of it. So warp is live across nearly the whole
//! preset range, and a WARPED_CAUSAL block the port cannot derive is a DECODE
//! MISMATCH (the decoder derives its own model from the same neighbours), not
//! merely an RD divergence.
//!
//! # Evidence
//!
//! Tier 1 throughout — see `tests/c_parity_warp_model.rs`, which drives the
//! real exported `svt_av1_warp_affine_c`, `svt_get_shear_params`,
//! `svt_find_projection`, `svt_aom_select_samples`, `svt_warp_plane` and
//! `svt_av1_warp_plane` through `svtav1-cref`. The `static` helpers
//! (`find_affine_int`, `is_affine_valid`, `is_affine_shear_allowed`,
//! `resolve_divisor_32/64`, `get_mult_shift_diag/ndiag`) have no exported
//! symbol of their own and are validated TRANSITIVELY through those wrappers,
//! which is what actually drives them in C.
//!
//! # `#if USE_LIMITED_PREC_MULT`
//!
//! `warped_motion.c:218` is `#define USE_LIMITED_PREC_MULT 0`, so the
//! `resolve_multiplier_64` variants of `get_mult_shift_diag/ndiag`
//! (`:241`/`:258`) are DEAD and the plain int64 `#else` arms (`:276`/`:283`)
//! are what compiles. Only the `#else` arms are ported here.

pub mod tables;

use svtav1_types::block::BlockSize;
use svtav1_types::motion::{Mv, TransformationType, WarpedMotionParams};
use svtav1_types::tables::block::{BLOCK_SIZE_HIGH, BLOCK_SIZE_WIDE};

use tables::{DIV_LUT, WARPED_FILTER};

// --------------------------------------------------------------------------
// Constants (warped_motion.h / warped_motion.c / definitions.h)
// --------------------------------------------------------------------------

/// `WARPEDMODEL_PREC_BITS` (warped_motion.h:22).
pub const WARPEDMODEL_PREC_BITS: i32 = 16;
/// `WARPEDMODEL_TRANS_CLAMP` (warped_motion.h:25).
pub const WARPEDMODEL_TRANS_CLAMP: i32 = 128 << WARPEDMODEL_PREC_BITS;
/// `WARPEDMODEL_NONDIAGAFFINE_CLAMP` (warped_motion.h:26).
pub const WARPEDMODEL_NONDIAGAFFINE_CLAMP: i32 = 1 << (WARPEDMODEL_PREC_BITS - 3);
/// `WARPEDPIXEL_PREC_BITS` (warped_motion.h:30).
pub const WARPEDPIXEL_PREC_BITS: i32 = 6;
/// `WARPEDPIXEL_PREC_SHIFTS` (warped_motion.h:31).
pub const WARPEDPIXEL_PREC_SHIFTS: i32 = 1 << WARPEDPIXEL_PREC_BITS;
/// `WARP_PARAM_REDUCE_BITS` (warped_motion.h:33).
pub const WARP_PARAM_REDUCE_BITS: i32 = 6;
/// `WARPEDDIFF_PREC_BITS` (warped_motion.h:35).
pub const WARPEDDIFF_PREC_BITS: i32 = WARPEDMODEL_PREC_BITS - WARPEDPIXEL_PREC_BITS;

/// `FILTER_BITS` (definitions.h:456).
pub const FILTER_BITS: i32 = 7;
/// `ROUND0_BITS` (convolve.h:22).
pub const ROUND0_BITS: i32 = 3;
/// `COMPOUND_ROUND1_BITS` (convolve.h:23).
pub const COMPOUND_ROUND1_BITS: i32 = 7;

/// `MI_SIZE` (definitions.h:375).
const MI_SIZE: i32 = 4;

/// `LS_MV_MAX` — max MV in 1/8-pel (warped_motion.c:22).
const LS_MV_MAX: i32 = 256;
/// `LS_STEP` (warped_motion.c:24).
const LS_STEP: i32 = 8;
/// `LS_MAT_DOWN_BITS` (warped_motion.c:37).
const LS_MAT_DOWN_BITS: i32 = 2;

const DIV_LUT_PREC_BITS: i32 = 14;
const DIV_LUT_BITS: i32 = 8;

// --------------------------------------------------------------------------
// Helper macros, transcribed
// --------------------------------------------------------------------------

/// `ROUND_POWER_OF_TWO(value, n)` (definitions.h:478) on `i32`.
///
/// C's macro is `((value) + ((1 << n) >> 1)) >> n`. It is documented "for use
/// when n >= 0, value >= 0" but `svt_av1_warp_affine_c` applies it to a
/// possibly-negative `sx`, where C's `>>` on a signed value is an arithmetic
/// shift on every compiler this project builds with. Rust's `>>` on `i32` is
/// arithmetic too, so this is the same operation.
#[inline]
const fn round_power_of_two(value: i32, n: i32) -> i32 {
    (value + ((1 << n) >> 1)) >> n
}

/// `ROUND_POWER_OF_TWO_64(value, n)` (definitions.h:485).
#[inline]
const fn round_power_of_two_64(value: i64, n: i32) -> i64 {
    (value + ((1i64 << n) >> 1)) >> n
}

/// `ROUND_POWER_OF_TWO_SIGNED(value, n)` (definitions.h:481) — note this is
/// NOT the same as `round_power_of_two` on a negative input: C negates,
/// rounds, and negates back, so it rounds AWAY from zero on ties for negative
/// values while the plain macro rounds toward +inf.
#[inline]
const fn round_power_of_two_signed(value: i32, n: i32) -> i32 {
    if value < 0 {
        -round_power_of_two(-value, n)
    } else {
        round_power_of_two(value, n)
    }
}

/// `ROUND_POWER_OF_TWO_SIGNED_64(value, n)` (definitions.h:488).
#[inline]
const fn round_power_of_two_signed_64(value: i64, n: i32) -> i64 {
    if value < 0 {
        -round_power_of_two_64(-value, n)
    } else {
        round_power_of_two_64(value, n)
    }
}

/// `get_msb(n)` (definitions.h:617) — index of the most significant set bit.
/// Undefined in C for `n == 0`; every call site here proves `n != 0` first.
#[inline]
const fn get_msb(n: u32) -> i32 {
    debug_assert!(n != 0);
    31 - n.leading_zeros() as i32
}

/// `clip_pixel` — clamp to 8-bit.
#[inline]
const fn clip_pixel(v: i32) -> u8 {
    if v < 0 {
        0
    } else if v > 255 {
        255
    } else {
        v as u8
    }
}

/// `clip_pixel_highbd(v, bd)`.
#[inline]
const fn clip_pixel_highbd(v: i32, bd: i32) -> u16 {
    let max = (1i32 << bd) - 1;
    if v < 0 {
        0
    } else if v > max {
        max as u16
    } else {
        v as u16
    }
}

/// `LS_SQUARE(a)` (warped_motion.c:46).
#[inline]
const fn ls_square(a: i32) -> i32 {
    (a * a * 4 + a * 4 * LS_STEP + LS_STEP * LS_STEP * 2) >> (2 + LS_MAT_DOWN_BITS)
}

/// `LS_PRODUCT1(a, b)` (warped_motion.c:47).
#[inline]
const fn ls_product1(a: i32, b: i32) -> i32 {
    (a * b * 4 + (a + b) * 2 * LS_STEP + LS_STEP * LS_STEP) >> (2 + LS_MAT_DOWN_BITS)
}

/// `LS_PRODUCT2(a, b)` (warped_motion.c:48).
#[inline]
const fn ls_product2(a: i32, b: i32) -> i32 {
    (a * b * 4 + (a + b) * 2 * LS_STEP + LS_STEP * LS_STEP * 2) >> (2 + LS_MAT_DOWN_BITS)
}

// --------------------------------------------------------------------------
// Fixed-point reciprocals (warped_motion.c:317 / :334)
// --------------------------------------------------------------------------

/// Port of `resolve_divisor_64` (warped_motion.c:317).
///
/// Decomposes a divisor `d` so that `1/d = y / 2^shift`, with `y` returned at
/// `DIV_LUT_PREC_BITS` precision. Bit-exactness of the whole warp model rests
/// on this and `resolve_divisor_32`.
///
/// Returns `(y, shift)`.
#[inline]
pub fn resolve_divisor_64(d: u64) -> (i16, i16) {
    let mut shift: i16 = if (d >> 32) != 0 {
        (get_msb((d >> 32) as u32) + 32) as i16
    } else {
        get_msb(d as u32) as i16
    };
    // e is obtained from D after resetting the most significant 1 bit.
    let e = (d - (1u64 << shift)) as i64;
    let f = if i32::from(shift) > DIV_LUT_BITS {
        round_power_of_two_64(e, i32::from(shift) - DIV_LUT_BITS)
    } else {
        e << (DIV_LUT_BITS - i32::from(shift))
    };
    debug_assert!(f <= 256);
    shift += DIV_LUT_PREC_BITS as i16;
    (DIV_LUT[f as usize] as i16, shift)
}

/// Port of `resolve_divisor_32` (warped_motion.c:334) — the 32-bit twin, used
/// in the shear derivation.
///
/// Returns `(y, shift)`.
#[inline]
pub fn resolve_divisor_32(d: u32) -> (i16, i16) {
    let mut shift: i16 = get_msb(d) as i16;
    let e = (d - (1u32 << shift)) as i32;
    let f = if i32::from(shift) > DIV_LUT_BITS {
        round_power_of_two(e, i32::from(shift) - DIV_LUT_BITS)
    } else {
        e << (DIV_LUT_BITS - i32::from(shift))
    };
    debug_assert!(f <= 256);
    shift += DIV_LUT_PREC_BITS as i16;
    (DIV_LUT[f as usize] as i16, shift)
}

/// Port of `get_mult_shift_diag`, the `USE_LIMITED_PREC_MULT == 0` arm
/// (warped_motion.c:283). The `:258` arm is dead — see the module doc.
#[inline]
fn get_mult_shift_diag(p_x: i64, i_det: i16, shift: i32) -> i32 {
    let v = p_x * i64::from(i_det);
    round_power_of_two_signed_64(v, shift).clamp(
        i64::from((1 << WARPEDMODEL_PREC_BITS) - WARPEDMODEL_NONDIAGAFFINE_CLAMP + 1),
        i64::from((1 << WARPEDMODEL_PREC_BITS) + WARPEDMODEL_NONDIAGAFFINE_CLAMP - 1),
    ) as i32
}

/// Port of `get_mult_shift_ndiag`, the `USE_LIMITED_PREC_MULT == 0` arm
/// (warped_motion.c:276).
#[inline]
fn get_mult_shift_ndiag(p_x: i64, i_det: i16, shift: i32) -> i32 {
    let v = p_x * i64::from(i_det);
    round_power_of_two_signed_64(v, shift).clamp(
        i64::from(-WARPEDMODEL_NONDIAGAFFINE_CLAMP + 1),
        i64::from(WARPEDMODEL_NONDIAGAFFINE_CLAMP - 1),
    ) as i32
}

// --------------------------------------------------------------------------
// Shear derivation (warped_motion.c:351 / :356 / :907)
// --------------------------------------------------------------------------

/// Port of `is_affine_valid` (warped_motion.c:351) — half of the shear
/// validity test. Literally `mat[2] > 0`.
#[inline]
pub fn is_affine_valid(wm: &WarpedMotionParams) -> bool {
    wm.wmmat[2] > 0
}

/// Port of `is_affine_shear_allowed` (warped_motion.c:356) — the WARP_PARAM
/// bounds. The comment above `svt_av1_warp_affine_c` explains them: the
/// horizontal filter generates 15 rows of 8 columns from a point projected at
/// (4, 4) within the block, so the fractional offsets must satisfy
/// `4|alpha| + 7|beta| <= 1` and `4|gamma| + 4|delta| <= 1` for an 8-tap
/// filter to cover them.
#[inline]
pub fn is_affine_shear_allowed(alpha: i16, beta: i16, gamma: i16, delta: i16) -> bool {
    let a = i32::from(alpha).abs();
    let b = i32::from(beta).abs();
    let g = i32::from(gamma).abs();
    let d = i32::from(delta).abs();
    !((4 * a + 7 * b >= (1 << WARPEDMODEL_PREC_BITS))
        || (4 * g + 4 * d >= (1 << WARPEDMODEL_PREC_BITS)))
}

/// Port of `svt_get_shear_params` (warped_motion.c:907) — spec 7.11.3.8
/// `setupShear`. Derives `alpha`/`beta`/`gamma`/`delta` into `wm` and returns
/// whether the model is legal for the fast warp filter.
///
/// This is both a NORMATIVE derivation (the decoder runs the same one) and the
/// legality gate for a global-motion model (`global_me.c:423`), so a wrong
/// shear changes both whether warp is allowed AND every predicted pixel.
pub fn get_shear_params(wm: &mut WarpedMotionParams) -> bool {
    if !is_affine_valid(wm) {
        return false;
    }
    let mat = wm.wmmat;
    wm.alpha = (mat[2] - (1 << WARPEDMODEL_PREC_BITS))
        .clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
    wm.beta = mat[3].clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;

    let (y0, shift) = resolve_divisor_32(mat[2].unsigned_abs());
    let y = i64::from(y0) * if mat[2] < 0 { -1 } else { 1 };

    let v = (i64::from(mat[4]) * (1i64 << WARPEDMODEL_PREC_BITS)) * y;
    wm.gamma = (round_power_of_two_signed_64(v, i32::from(shift)) as i32)
        .clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;

    let v = (i64::from(mat[3]) * i64::from(mat[4])) * y;
    wm.delta = (mat[5]
        - (round_power_of_two_signed_64(v, i32::from(shift)) as i32)
        - (1 << WARPEDMODEL_PREC_BITS))
        .clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;

    wm.alpha = (round_power_of_two_signed(i32::from(wm.alpha), WARP_PARAM_REDUCE_BITS)
        * (1 << WARP_PARAM_REDUCE_BITS)) as i16;
    wm.beta = (round_power_of_two_signed(i32::from(wm.beta), WARP_PARAM_REDUCE_BITS)
        * (1 << WARP_PARAM_REDUCE_BITS)) as i16;
    wm.gamma = (round_power_of_two_signed(i32::from(wm.gamma), WARP_PARAM_REDUCE_BITS)
        * (1 << WARP_PARAM_REDUCE_BITS)) as i16;
    wm.delta = (round_power_of_two_signed(i32::from(wm.delta), WARP_PARAM_REDUCE_BITS)
        * (1 << WARP_PARAM_REDUCE_BITS)) as i16;

    is_affine_shear_allowed(wm.alpha, wm.beta, wm.gamma, wm.delta)
}

// --------------------------------------------------------------------------
// Model derivation (warped_motion.c:365 / :473)
// --------------------------------------------------------------------------

/// Port of `find_affine_int` (warped_motion.c:365) — the least-squares affine
/// solve from the neighbour sample pairs.
///
/// `pts1` are the source points and `pts2` the destination points, both as
/// interleaved `[x0, y0, x1, y1, ...]` in 1/8-pel units. Returns `true` on
/// FAILURE (matching C's nonzero-is-failure convention, which
/// `find_projection` inverts).
fn find_affine_int(
    np: usize,
    pts1: &[i32],
    pts2: &[i32],
    bsize: BlockSize,
    mv: Mv,
    wm: &mut WarpedMotionParams,
    mi_row: i32,
    mi_col: i32,
) -> bool {
    let mut a = [[0i32; 2]; 2];
    let mut bx = [0i32; 2];
    let mut by = [0i32; 2];

    let bw = i32::from(BLOCK_SIZE_WIDE[bsize as usize]);
    let bh = i32::from(BLOCK_SIZE_HIGH[bsize as usize]);
    let rsuy = bh.max(MI_SIZE) / 2 - 1;
    let rsux = bw.max(MI_SIZE) / 2 - 1;
    let suy = rsuy * 8;
    let sux = rsux * 8;
    let duy = suy + i32::from(mv.y);
    let dux = sux + i32::from(mv.x);
    let isuy = mi_row * MI_SIZE + rsuy;
    let isux = mi_col * MI_SIZE + rsux;

    for i in 0..np {
        let dx = pts2[i * 2] - dux;
        let dy = pts2[i * 2 + 1] - duy;
        let sx = pts1[i * 2] - sux;
        let sy = pts1[i * 2 + 1] - suy;
        if (sx - dx).abs() < LS_MV_MAX && (sy - dy).abs() < LS_MV_MAX {
            a[0][0] += ls_square(sx);
            a[0][1] += ls_product1(sx, sy);
            a[1][1] += ls_square(sy);
            bx[0] += ls_product2(sx, dx);
            bx[1] += ls_product1(sy, dx);
            by[0] += ls_product1(sx, dy);
            by[1] += ls_product2(sy, dy);
        }
    }

    // Determinant of A.
    let det = i64::from(a[0][0]) * i64::from(a[1][1]) - i64::from(a[0][1]) * i64::from(a[0][1]);
    if det == 0 {
        return true;
    }
    let (mag, shift0) = resolve_divisor_64(det.unsigned_abs());
    let mut i_det = mag * if det < 0 { -1 } else { 1 };
    let mut shift = i32::from(shift0) - WARPEDMODEL_PREC_BITS;
    if shift < 0 {
        i_det <<= -shift;
        shift = 0;
    }

    // These, divided by the det, are the least-squares solutions.
    let p_x0 = i64::from(a[1][1]) * i64::from(bx[0]) - i64::from(a[0][1]) * i64::from(bx[1]);
    let p_x1 = -i64::from(a[0][1]) * i64::from(bx[0]) + i64::from(a[0][0]) * i64::from(bx[1]);
    let p_y0 = i64::from(a[1][1]) * i64::from(by[0]) - i64::from(a[0][1]) * i64::from(by[1]);
    let p_y1 = -i64::from(a[0][1]) * i64::from(by[0]) + i64::from(a[0][0]) * i64::from(by[1]);

    wm.wmmat[2] = get_mult_shift_diag(p_x0, i_det, shift);
    wm.wmmat[3] = get_mult_shift_ndiag(p_x1, i_det, shift);
    wm.wmmat[4] = get_mult_shift_ndiag(p_y0, i_det, shift);
    wm.wmmat[5] = get_mult_shift_diag(p_y1, i_det, shift);

    // C's comment: the max value of each of the 2nd and 3rd terms is
    // (2^16 - 1) * (2^13 - 1), which leaves room for the first term so the
    // sum fits in 32 bits. C computes this in `int32_t`, so the port uses
    // wrapping arithmetic to reproduce C's behaviour exactly rather than
    // panicking in a debug build on an input C would simply wrap.
    let vx = (i32::from(mv.x))
        .wrapping_mul(1 << (WARPEDMODEL_PREC_BITS - 3))
        .wrapping_sub(
            isux.wrapping_mul(wm.wmmat[2] - (1 << WARPEDMODEL_PREC_BITS))
                .wrapping_add(isuy.wrapping_mul(wm.wmmat[3])),
        );
    let vy = (i32::from(mv.y))
        .wrapping_mul(1 << (WARPEDMODEL_PREC_BITS - 3))
        .wrapping_sub(
            isux.wrapping_mul(wm.wmmat[4])
                .wrapping_add(isuy.wrapping_mul(wm.wmmat[5] - (1 << WARPEDMODEL_PREC_BITS))),
        );
    wm.wmmat[0] = vx.clamp(-WARPEDMODEL_TRANS_CLAMP, WARPEDMODEL_TRANS_CLAMP - 1);
    wm.wmmat[1] = vy.clamp(-WARPEDMODEL_TRANS_CLAMP, WARPEDMODEL_TRANS_CLAMP - 1);

    false
}

/// Port of `svt_find_projection` (warped_motion.c:473) — the normative
/// local-warp model derivation, called from `svt_aom_warped_motion_parameters`
/// (`adaptive_mv_pred.c:1814`).
///
/// Returns `true` on FAILURE, exactly like C (whose return is `1` on failure
/// and `0` on success — note the inverted sense against
/// `get_shear_params`, which returns 1 on SUCCESS; this asymmetry is C's and
/// is preserved so call sites transcribe one-for-one).
pub fn find_projection(
    np: usize,
    pts1: &[i32],
    pts2: &[i32],
    bsize: BlockSize,
    mv: Mv,
    wm_params: &mut WarpedMotionParams,
    mi_row: i32,
    mi_col: i32,
) -> bool {
    if find_affine_int(np, pts1, pts2, bsize, mv, wm_params, mi_row, mi_col) {
        return true;
    }
    // Check compatibility with the fast warp filter.
    if !get_shear_params(wm_params) {
        return true;
    }
    false
}

/// Port of `svt_aom_select_samples` (warped_motion.c:935) — keeps only the
/// neighbour samples whose MV difference is within a block-size-derived
/// threshold, COMPACTING the survivors to the front of both arrays in place.
///
/// A different sample subset yields a different affine model, hence different
/// predicted pixels and different bytes.
///
/// Returns the number of samples kept — at least 1, per C's final
/// `AOMMAX(ret, 1)`, which can exceed the number that actually passed the
/// threshold. That is deliberate in C (a model is always derived) and is
/// preserved here.
pub fn select_samples(
    mv: Mv,
    pts: &mut [i32],
    pts_inref: &mut [i32],
    len: usize,
    bsize: BlockSize,
) -> u8 {
    let bw = i32::from(BLOCK_SIZE_WIDE[bsize as usize]);
    let bh = i32::from(BLOCK_SIZE_HIGH[bsize as usize]);
    let thresh = bw.max(bh).clamp(16, 112);
    let mut ret = 0usize;

    for i in 0..len {
        let diff = (pts_inref[2 * i] - pts[2 * i] - i32::from(mv.x)).abs()
            + (pts_inref[2 * i + 1] - pts[2 * i + 1] - i32::from(mv.y)).abs();
        if diff > thresh {
            continue;
        }
        if ret != i {
            pts[2 * ret] = pts[2 * i];
            pts[2 * ret + 1] = pts[2 * i + 1];
            pts_inref[2 * ret] = pts_inref[2 * i];
            pts_inref[2 * ret + 1] = pts_inref[2 * i + 1];
        }
        ret += 1;
    }
    (ret.max(1)) as u8
}

// --------------------------------------------------------------------------
// Convolve params (convolve.h:41)
// --------------------------------------------------------------------------

/// Port of `ConvolveParams` (definitions.h:681) restricted to the fields the
/// warp kernels read. The compound destination buffer is passed separately to
/// the kernels so this stays `Copy` and the borrow checker sees one mutable
/// borrow at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WarpConvolveParams {
    pub do_average: bool,
    pub dst_stride: usize,
    pub round_0: i32,
    pub round_1: i32,
    pub is_compound: bool,
    pub use_jnt_comp_avg: bool,
    pub fwd_offset: i32,
    pub bck_offset: i32,
}

impl WarpConvolveParams {
    /// Port of `get_conv_params_no_round` (convolve.h:41).
    pub fn no_round(do_average: bool, dst_stride: usize, is_compound: bool, bd: i32) -> Self {
        debug_assert!(!do_average || is_compound);
        let mut round_0 = ROUND0_BITS;
        let mut round_1 = if is_compound {
            COMPOUND_ROUND1_BITS
        } else {
            2 * FILTER_BITS - ROUND0_BITS
        };
        let intbufrange = bd + FILTER_BITS - round_0 + 2;
        if intbufrange > 16 {
            round_0 += intbufrange - 16;
            if !is_compound {
                round_1 -= intbufrange - 16;
            }
        }
        Self {
            do_average,
            dst_stride,
            round_0,
            round_1,
            is_compound,
            use_jnt_comp_avg: false,
            fwd_offset: 0,
            bck_offset: 0,
        }
    }

    /// Port of `get_conv_params(do_average, bd)` (convolve.h:68) — the
    /// non-compound case, which is what `svt_warp_plane` is called with for
    /// single-reference warped prediction.
    pub fn simple(do_average: bool, bd: i32) -> Self {
        Self::no_round(do_average, 0, false, bd)
    }
}

/// `DIST_PRECISION_BITS` (definitions.h) — the jnt-comp average shift.
const DIST_PRECISION_BITS: i32 = 4;

// --------------------------------------------------------------------------
// The warp kernel (warped_motion.c:571)
// --------------------------------------------------------------------------

/// Port of `svt_av1_warp_affine_c` (warped_motion.c:571) — 8-bit.
///
/// Splits the destination into 8x8 blocks; for each, projects the point
/// `(4, 4)` to get the block position, keeps full `WARPEDMODEL` precision,
/// filters horizontally into a 15x8 intermediate (rows aligned with the
/// REFERENCE image, columns with the DESTINATION) and then vertically into the
/// output. The shear factorisation that makes the two passes separable is
/// described at length in the C comment above the function.
///
/// `dst` is the compound destination buffer, required (and only read/written)
/// when `conv_params.is_compound`.
#[allow(clippy::too_many_arguments)]
pub fn warp_affine(
    mat: &[i32; 6],
    reference: &[u8],
    width: i32,
    height: i32,
    stride: usize,
    pred: &mut [u8],
    dst: Option<&mut [u16]>,
    p_col: i32,
    p_row: i32,
    p_width: i32,
    p_height: i32,
    p_stride: usize,
    subsampling_x: i32,
    subsampling_y: i32,
    conv_params: &WarpConvolveParams,
    alpha: i16,
    beta: i16,
    gamma: i16,
    delta: i16,
) {
    let mut tmp = [0i32; 15 * 8];
    let bd = 8i32;
    let reduce_bits_horiz = conv_params.round_0;
    let reduce_bits_vert = if conv_params.is_compound {
        conv_params.round_1
    } else {
        2 * FILTER_BITS - reduce_bits_horiz
    };
    let offset_bits_horiz = bd + FILTER_BITS - 1;
    let offset_bits_vert = bd + 2 * FILTER_BITS - reduce_bits_horiz;
    let round_bits = 2 * FILTER_BITS - conv_params.round_0 - conv_params.round_1;
    let offset_bits = bd + 2 * FILTER_BITS - conv_params.round_0;

    let mut dst = dst;

    let mut i = p_row;
    while i < p_row + p_height {
        let mut j = p_col;
        while j < p_col + p_width {
            // Centre of this 8x8 block, projected to luma coordinates (if in a
            // subsampled chroma plane), transformed, then converted back.
            let src_x = (j + 4) << subsampling_x;
            let src_y = (i + 4) << subsampling_y;
            let dst_x = mat[2]
                .wrapping_mul(src_x)
                .wrapping_add(mat[3].wrapping_mul(src_y))
                .wrapping_add(mat[0]);
            let dst_y = mat[4]
                .wrapping_mul(src_x)
                .wrapping_add(mat[5].wrapping_mul(src_y))
                .wrapping_add(mat[1]);
            let x4 = dst_x >> subsampling_x;
            let y4 = dst_y >> subsampling_y;

            let ix4 = x4 >> WARPEDMODEL_PREC_BITS;
            let mut sx4 = x4 & ((1 << WARPEDMODEL_PREC_BITS) - 1);
            let iy4 = y4 >> WARPEDMODEL_PREC_BITS;
            let mut sy4 = y4 & ((1 << WARPEDMODEL_PREC_BITS) - 1);

            sx4 += i32::from(alpha) * (-4) + i32::from(beta) * (-4);
            sy4 += i32::from(gamma) * (-4) + i32::from(delta) * (-4);

            sx4 &= !((1 << WARP_PARAM_REDUCE_BITS) - 1);
            sy4 &= !((1 << WARP_PARAM_REDUCE_BITS) - 1);

            // Horizontal filter: 15 rows of 8.
            for k in -7..8i32 {
                let iy = (iy4 + k).clamp(0, height - 1);
                let mut sx = sx4 + i32::from(beta) * (k + 4);
                for l in -4..4i32 {
                    let ix = ix4 + l - 3;
                    let offs =
                        round_power_of_two(sx, WARPEDDIFF_PREC_BITS) + WARPEDPIXEL_PREC_SHIFTS;
                    debug_assert!((0..=WARPEDPIXEL_PREC_SHIFTS * 3).contains(&offs));
                    let coeffs = &WARPED_FILTER[offs as usize];

                    let mut sum = 1i32 << offset_bits_horiz;
                    for m in 0..8i32 {
                        let sample_x = (ix + m).clamp(0, width - 1);
                        sum += i32::from(reference[iy as usize * stride + sample_x as usize])
                            * i32::from(coeffs[m as usize]);
                    }
                    sum = round_power_of_two(sum, reduce_bits_horiz);
                    tmp[((k + 7) * 8 + (l + 4)) as usize] = sum;
                    sx += i32::from(alpha);
                }
            }

            // Vertical filter.
            let k_end = 4.min(p_row + p_height - i - 4);
            let l_end = 4.min(p_col + p_width - j - 4);
            for k in -4..k_end {
                let mut sy = sy4 + i32::from(delta) * (k + 4);
                for l in -4..l_end {
                    let offs =
                        round_power_of_two(sy, WARPEDDIFF_PREC_BITS) + WARPEDPIXEL_PREC_SHIFTS;
                    debug_assert!((0..=WARPEDPIXEL_PREC_SHIFTS * 3).contains(&offs));
                    let coeffs = &WARPED_FILTER[offs as usize];

                    let mut sum = 1i32 << offset_bits_vert;
                    for m in 0..8i32 {
                        sum += tmp[((k + m + 4) * 8 + (l + 4)) as usize]
                            * i32::from(coeffs[m as usize]);
                    }
                    let out_row = (i - p_row + k + 4) as usize;
                    let out_col = (j - p_col + l + 4) as usize;
                    if conv_params.is_compound {
                        let dst_buf = dst
                            .as_mut()
                            .expect("compound warp_affine requires a destination buffer");
                        let pi = out_row * conv_params.dst_stride + out_col;
                        let sum = round_power_of_two(sum, reduce_bits_vert);
                        if conv_params.do_average {
                            let mut tmp32 = i32::from(dst_buf[pi]);
                            if conv_params.use_jnt_comp_avg {
                                tmp32 =
                                    tmp32 * conv_params.fwd_offset + sum * conv_params.bck_offset;
                                tmp32 >>= DIST_PRECISION_BITS;
                            } else {
                                tmp32 += sum;
                                tmp32 >>= 1;
                            }
                            tmp32 = tmp32
                                - (1 << (offset_bits - conv_params.round_1))
                                - (1 << (offset_bits - conv_params.round_1 - 1));
                            pred[out_row * p_stride + out_col] =
                                clip_pixel(round_power_of_two(tmp32, round_bits));
                        } else {
                            dst_buf[pi] = sum as u16;
                        }
                    } else {
                        let sum = round_power_of_two(sum, reduce_bits_vert);
                        pred[out_row * p_stride + out_col] =
                            clip_pixel(sum - (1 << (bd - 1)) - (1 << bd));
                    }
                    sy += i32::from(gamma);
                }
            }
            j += 8;
        }
        i += 8;
    }
}

/// The high-bit-depth warp kernel's reference, in either of the two
/// representations `svt_av1_highbd_warp_affine_c` accepts.
///
/// C takes `ref8b` + `ref2b` with SEPARATE strides and combines them per
/// sample as `(msb << 2) | ((lsb >> 6) & 3)` (warped_motion.c:774-775). An
/// already-unpacked `u16` plane is the same samples with that combine already
/// done, so both forms reach the IDENTICAL kernel — which is why this is a
/// view over the one read, not a second kernel.
///
/// `highbd_warp_plane` passes ONE stride for both C planes; the two-stride
/// form is kept because the kernel's own signature has it.
#[derive(Debug, Clone, Copy)]
pub enum HbdWarpRef<'a> {
    /// One 10-bit plane.
    Unpacked {
        /// The samples.
        plane: &'a [u16],
        /// Row stride, in samples.
        stride: usize,
    },
    /// SVT's split pair: eight MSBs, and two LSBs in each byte's top bits.
    Split {
        /// The eight most significant bits.
        msb: &'a [u8],
        /// The two least significant bits, in each byte's top two bits.
        lsb: &'a [u8],
        /// `stride8b`.
        stride8b: usize,
        /// `stride2b`.
        stride2b: usize,
    },
}

impl HbdWarpRef<'_> {
    /// One reference sample, at 10 bits either way.
    #[inline]
    fn sample(&self, y: usize, x: usize) -> i32 {
        match *self {
            HbdWarpRef::Unpacked { plane, stride } => i32::from(plane[y * stride + x]),
            HbdWarpRef::Split {
                msb,
                lsb,
                stride8b,
                stride2b,
            } => (i32::from(msb[y * stride8b + x]) << 2) | i32::from(lsb[y * stride2b + x] >> 6),
        }
    }
}

/// Port of `svt_av1_highbd_warp_affine_c` (warped_motion.c:719) — the 10-bit
/// normative warp prediction. `bd` is 10 or 12.
///
/// [`highbd_warp_affine`] is the same kernel over an already-unpacked plane,
/// which is what every caller outside the warp leaf of
/// `crate::port_enc_make_pred::enc_make_inter_predictor` holds.
#[allow(clippy::too_many_arguments)]
pub fn highbd_warp_affine_ref(
    mat: &[i32; 6],
    reference: HbdWarpRef<'_>,
    width: i32,
    height: i32,
    pred: &mut [u16],
    dst: Option<&mut [u16]>,
    p_col: i32,
    p_row: i32,
    p_width: i32,
    p_height: i32,
    p_stride: usize,
    subsampling_x: i32,
    subsampling_y: i32,
    bd: i32,
    conv_params: &WarpConvolveParams,
    alpha: i16,
    beta: i16,
    gamma: i16,
    delta: i16,
) {
    let mut tmp = [0i32; 15 * 8];
    // NOTE the extra term the 8-bit kernel does NOT have: the high-bit-depth
    // kernel widens the horizontal reduction so the intermediate still fits a
    // 14-bit-plus-sign word (warped_motion.c:725).
    let reduce_bits_horiz =
        conv_params.round_0 + (bd + FILTER_BITS - conv_params.round_0 - 14).max(0);
    let reduce_bits_vert = if conv_params.is_compound {
        conv_params.round_1
    } else {
        2 * FILTER_BITS - reduce_bits_horiz
    };
    let offset_bits_horiz = bd + FILTER_BITS - 1;
    let offset_bits_vert = bd + 2 * FILTER_BITS - reduce_bits_horiz;
    let round_bits = 2 * FILTER_BITS - conv_params.round_0 - conv_params.round_1;
    let offset_bits = bd + 2 * FILTER_BITS - conv_params.round_0;

    let mut dst = dst;

    let mut i = p_row;
    while i < p_row + p_height {
        let mut j = p_col;
        while j < p_col + p_width {
            let src_x = (j + 4) << subsampling_x;
            let src_y = (i + 4) << subsampling_y;
            let dst_x = mat[2]
                .wrapping_mul(src_x)
                .wrapping_add(mat[3].wrapping_mul(src_y))
                .wrapping_add(mat[0]);
            let dst_y = mat[4]
                .wrapping_mul(src_x)
                .wrapping_add(mat[5].wrapping_mul(src_y))
                .wrapping_add(mat[1]);
            let x4 = dst_x >> subsampling_x;
            let y4 = dst_y >> subsampling_y;

            let ix4 = x4 >> WARPEDMODEL_PREC_BITS;
            let mut sx4 = x4 & ((1 << WARPEDMODEL_PREC_BITS) - 1);
            let iy4 = y4 >> WARPEDMODEL_PREC_BITS;
            let mut sy4 = y4 & ((1 << WARPEDMODEL_PREC_BITS) - 1);

            sx4 += i32::from(alpha) * (-4) + i32::from(beta) * (-4);
            sy4 += i32::from(gamma) * (-4) + i32::from(delta) * (-4);

            sx4 &= !((1 << WARP_PARAM_REDUCE_BITS) - 1);
            sy4 &= !((1 << WARP_PARAM_REDUCE_BITS) - 1);

            for k in -7..8i32 {
                let iy = (iy4 + k).clamp(0, height - 1);
                let mut sx = sx4 + i32::from(beta) * (k + 4);
                for l in -4..4i32 {
                    let ix = ix4 + l - 3;
                    let offs =
                        round_power_of_two(sx, WARPEDDIFF_PREC_BITS) + WARPEDPIXEL_PREC_SHIFTS;
                    let coeffs = &WARPED_FILTER[offs as usize];
                    let mut sum = 1i32 << offset_bits_horiz;
                    for m in 0..8i32 {
                        let sample_x = (ix + m).clamp(0, width - 1);
                        sum += reference.sample(iy as usize, sample_x as usize)
                            * i32::from(coeffs[m as usize]);
                    }
                    sum = round_power_of_two(sum, reduce_bits_horiz);
                    tmp[((k + 7) * 8 + (l + 4)) as usize] = sum;
                    sx += i32::from(alpha);
                }
            }

            let k_end = 4.min(p_row + p_height - i - 4);
            let l_end = 4.min(p_col + p_width - j - 4);
            for k in -4..k_end {
                let mut sy = sy4 + i32::from(delta) * (k + 4);
                for l in -4..l_end {
                    let offs =
                        round_power_of_two(sy, WARPEDDIFF_PREC_BITS) + WARPEDPIXEL_PREC_SHIFTS;
                    let coeffs = &WARPED_FILTER[offs as usize];
                    // C accumulates in `int32_t`; the port matches so an
                    // out-of-range model wraps the way C wraps instead of
                    // producing a different (wider) answer.
                    let mut sum = 1i32 << offset_bits_vert;
                    for m in 0..8i32 {
                        sum = sum.wrapping_add(
                            tmp[((k + m + 4) * 8 + (l + 4)) as usize]
                                .wrapping_mul(i32::from(coeffs[m as usize])),
                        );
                    }
                    let out_row = (i - p_row + k + 4) as usize;
                    let out_col = (j - p_col + l + 4) as usize;
                    if conv_params.is_compound {
                        let dst_buf = dst
                            .as_mut()
                            .expect("compound warp_affine requires a destination buffer");
                        let pi = out_row * conv_params.dst_stride + out_col;
                        let sum = round_power_of_two(sum, reduce_bits_vert);
                        if conv_params.do_average {
                            let mut tmp32 = i32::from(dst_buf[pi]);
                            if conv_params.use_jnt_comp_avg {
                                tmp32 =
                                    tmp32 * conv_params.fwd_offset + sum * conv_params.bck_offset;
                                tmp32 >>= DIST_PRECISION_BITS;
                            } else {
                                tmp32 += sum;
                                tmp32 >>= 1;
                            }
                            tmp32 = tmp32
                                - (1 << (offset_bits - conv_params.round_1))
                                - (1 << (offset_bits - conv_params.round_1 - 1));
                            pred[out_row * p_stride + out_col] =
                                clip_pixel_highbd(round_power_of_two(tmp32, round_bits), bd);
                        } else {
                            dst_buf[pi] = sum as u16;
                        }
                    } else {
                        let sum = round_power_of_two(sum, reduce_bits_vert);
                        pred[out_row * p_stride + out_col] =
                            clip_pixel_highbd(sum - (1 << (bd - 1)) - (1 << bd), bd);
                    }
                    sy += i32::from(gamma);
                }
            }
            j += 8;
        }
        i += 8;
    }
}

// --------------------------------------------------------------------------
// Plane drivers (warped_motion.c:686 / :829 / :868)
// --------------------------------------------------------------------------

/// Port of `svt_warp_plane` (warped_motion.c:686) — the 8-bit driver.
///
/// The ROTZOOM fix-up is NORMATIVE and mutates `wm`: a ROTZOOM model transmits
/// only `wmmat[2]`/`wmmat[3]`, and `wmmat[5] = wmmat[2]`, `wmmat[4] =
/// -wmmat[3]` are derived. Skipping it predicts with a stale `wmmat[4..6]`.
#[allow(clippy::too_many_arguments)]
pub fn warp_plane(
    wm: &mut WarpedMotionParams,
    reference: &[u8],
    width: i32,
    height: i32,
    stride: usize,
    pred: &mut [u8],
    dst: Option<&mut [u16]>,
    p_col: i32,
    p_row: i32,
    p_width: i32,
    p_height: i32,
    p_stride: usize,
    subsampling_x: i32,
    subsampling_y: i32,
    conv_params: &WarpConvolveParams,
) {
    debug_assert!(wm.wm_type as u8 <= TransformationType::Affine as u8);
    if wm.wm_type == TransformationType::RotZoom {
        wm.wmmat[5] = wm.wmmat[2];
        wm.wmmat[4] = -wm.wmmat[3];
    }
    let mat = wm.wmmat;
    warp_affine(
        &mat,
        reference,
        width,
        height,
        stride,
        pred,
        dst,
        p_col,
        p_row,
        p_width,
        p_height,
        p_stride,
        subsampling_x,
        subsampling_y,
        conv_params,
        wm.alpha,
        wm.beta,
        wm.gamma,
        wm.delta,
    );
}

/// Port of `svt_av1_highbd_warp_affine_c` over an already-unpacked 10-bit
/// plane — the shape every existing caller uses.
#[allow(clippy::too_many_arguments)]
pub fn highbd_warp_affine(
    mat: &[i32; 6],
    reference: &[u16],
    width: i32,
    height: i32,
    stride: usize,
    pred: &mut [u16],
    dst: Option<&mut [u16]>,
    p_col: i32,
    p_row: i32,
    p_width: i32,
    p_height: i32,
    p_stride: usize,
    subsampling_x: i32,
    subsampling_y: i32,
    bd: i32,
    conv_params: &WarpConvolveParams,
    alpha: i16,
    beta: i16,
    gamma: i16,
    delta: i16,
) {
    highbd_warp_affine_ref(
        mat,
        HbdWarpRef::Unpacked {
            plane: reference,
            stride,
        },
        width,
        height,
        pred,
        dst,
        p_col,
        p_row,
        p_width,
        p_height,
        p_stride,
        subsampling_x,
        subsampling_y,
        bd,
        conv_params,
        alpha,
        beta,
        gamma,
        delta,
    );
}

/// Port of `highbd_warp_plane` (warped_motion.c:829) — the 10-bit driver.
/// Same ROTZOOM fix-up as the 8-bit path.
///
/// [`highbd_warp_plane`] is this over an already-unpacked plane.
#[allow(clippy::too_many_arguments)]
pub fn highbd_warp_plane_ref(
    wm: &mut WarpedMotionParams,
    reference: HbdWarpRef<'_>,
    width: i32,
    height: i32,
    pred: &mut [u16],
    dst: Option<&mut [u16]>,
    p_col: i32,
    p_row: i32,
    p_width: i32,
    p_height: i32,
    p_stride: usize,
    subsampling_x: i32,
    subsampling_y: i32,
    bd: i32,
    conv_params: &WarpConvolveParams,
) {
    debug_assert!(wm.wm_type as u8 <= TransformationType::Affine as u8);
    if wm.wm_type == TransformationType::RotZoom {
        wm.wmmat[5] = wm.wmmat[2];
        wm.wmmat[4] = -wm.wmmat[3];
    }
    let mat = wm.wmmat;
    highbd_warp_affine_ref(
        &mat,
        reference,
        width,
        height,
        pred,
        dst,
        p_col,
        p_row,
        p_width,
        p_height,
        p_stride,
        subsampling_x,
        subsampling_y,
        bd,
        conv_params,
        wm.alpha,
        wm.beta,
        wm.gamma,
        wm.delta,
    );
}

/// Port of `highbd_warp_plane` over an already-unpacked 10-bit plane.
#[allow(clippy::too_many_arguments)]
pub fn highbd_warp_plane(
    wm: &mut WarpedMotionParams,
    reference: &[u16],
    width: i32,
    height: i32,
    stride: usize,
    pred: &mut [u16],
    dst: Option<&mut [u16]>,
    p_col: i32,
    p_row: i32,
    p_width: i32,
    p_height: i32,
    p_stride: usize,
    subsampling_x: i32,
    subsampling_y: i32,
    bd: i32,
    conv_params: &WarpConvolveParams,
) {
    highbd_warp_plane_ref(
        wm,
        HbdWarpRef::Unpacked {
            plane: reference,
            stride,
        },
        width,
        height,
        pred,
        dst,
        p_col,
        p_row,
        p_width,
        p_height,
        p_stride,
        subsampling_x,
        subsampling_y,
        bd,
        conv_params,
    );
}

/// Port of `svt_av1_warp_plane` (warped_motion.c:868) — the bit-depth
/// dispatcher, which is the entry point `enc_inter_prediction.c:1660,2546`
/// actually calls. The Rust signature splits C's single `use_hbd` +
/// `uint8_t*` pair into a typed enum so a caller cannot pass an 8-bit buffer
/// with `use_hbd = 1`.
pub enum WarpPlaneIo<'a> {
    /// 8-bit: reference plane and prediction output.
    Lowbd {
        reference: &'a [u8],
        pred: &'a mut [u8],
    },
    /// High bit depth: unpacked `u16` reference plane and prediction output.
    Highbd {
        /// The reference samples.
        reference: &'a [u16],
        /// The prediction output.
        pred: &'a mut [u16],
        /// 10 or 12.
        bd: i32,
    },
    /// High bit depth over SVT's SPLIT reference — what C's `ref_2b != NULL`
    /// arm takes (warped_motion.c:868 hands both planes to
    /// `highbd_warp_plane`, which hands them to
    /// `svt_av1_highbd_warp_affine_c`). Both planes are indexed at the single
    /// `stride` argument, as C does.
    HighbdSplit {
        /// The eight most significant bits.
        msb: &'a [u8],
        /// The two least significant bits, in each byte's top two bits.
        lsb: &'a [u8],
        /// The prediction output.
        pred: &'a mut [u16],
        /// 10 or 12.
        bd: i32,
    },
}

/// Port of `svt_av1_warp_plane` (warped_motion.c:868).
#[allow(clippy::too_many_arguments)]
pub fn av1_warp_plane(
    wm: &mut WarpedMotionParams,
    io: WarpPlaneIo<'_>,
    width: i32,
    height: i32,
    stride: usize,
    dst: Option<&mut [u16]>,
    p_col: i32,
    p_row: i32,
    p_width: i32,
    p_height: i32,
    p_stride: usize,
    subsampling_x: i32,
    subsampling_y: i32,
    conv_params: &WarpConvolveParams,
) {
    match io {
        WarpPlaneIo::Lowbd { reference, pred } => warp_plane(
            wm,
            reference,
            width,
            height,
            stride,
            pred,
            dst,
            p_col,
            p_row,
            p_width,
            p_height,
            p_stride,
            subsampling_x,
            subsampling_y,
            conv_params,
        ),
        WarpPlaneIo::HighbdSplit { msb, lsb, pred, bd } => highbd_warp_plane_ref(
            wm,
            HbdWarpRef::Split {
                msb,
                lsb,
                stride8b: stride,
                stride2b: stride,
            },
            width,
            height,
            pred,
            dst,
            p_col,
            p_row,
            p_width,
            p_height,
            p_stride,
            subsampling_x,
            subsampling_y,
            bd,
            conv_params,
        ),
        WarpPlaneIo::Highbd {
            reference,
            pred,
            bd,
        } => highbd_warp_plane(
            wm,
            reference,
            width,
            height,
            stride,
            pred,
            dst,
            p_col,
            p_row,
            p_width,
            p_height,
            p_stride,
            subsampling_x,
            subsampling_y,
            bd,
            conv_params,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warped_filter_rows_sum_to_128() {
        for (i, row) in WARPED_FILTER.iter().enumerate() {
            let s: i32 = row.iter().map(|&v| i32::from(v)).sum();
            assert_eq!(s, 1 << FILTER_BITS, "warped filter row {i} sums to {s}");
        }
    }

    #[test]
    fn div_lut_endpoints() {
        assert_eq!(DIV_LUT.len(), 257);
        assert_eq!(DIV_LUT[0], 1 << DIV_LUT_PREC_BITS);
        assert_eq!(DIV_LUT[256], 1 << (DIV_LUT_PREC_BITS - 1));
        // Monotone decreasing (it is 1/x over [1, 2)).
        for w in DIV_LUT.windows(2) {
            assert!(w[0] >= w[1]);
        }
    }

    #[test]
    fn identity_model_shear_is_zero_and_allowed() {
        let mut wm = WarpedMotionParams::default();
        assert!(get_shear_params(&mut wm));
        assert_eq!((wm.alpha, wm.beta, wm.gamma, wm.delta), (0, 0, 0, 0));
    }

    #[test]
    fn non_positive_diagonal_is_invalid() {
        let mut wm = WarpedMotionParams {
            wmmat: [0, 0, 0, 0, 0, 1 << WARPEDMODEL_PREC_BITS],
            ..Default::default()
        };
        assert!(!is_affine_valid(&wm));
        assert!(!get_shear_params(&mut wm));
    }

    #[test]
    fn select_samples_keeps_at_least_one() {
        // Every sample is far outside the threshold, so C's AOMMAX(ret, 1)
        // still reports 1 — the port must not report 0.
        let mut pts = [0, 0, 8, 8];
        let mut inref = [10_000, 10_000, 20_000, 20_000];
        let n = select_samples(
            Mv { x: 0, y: 0 },
            &mut pts,
            &mut inref,
            2,
            BlockSize::Block16x16,
        );
        assert_eq!(n, 1);
    }
}
