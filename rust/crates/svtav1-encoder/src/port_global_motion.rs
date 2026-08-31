//! Global motion — a port of `Codec/global_motion.c`'s model-conversion and
//! refinement half, plus the `svt_av1_warp_error` it depends on.
//!
//! # Reachability (verified, not inferred)
//!
//! `svt_aom_get_gm_core_level` (`enc_mode_config.c:180`) gives level 4 for
//! `enc_mode <= ENC_M4` and 0 above, and `svt_aom_derive_gm_level` forces 0 on
//! I-slices. So global motion affects **presets 0..4, inter frames only**; at
//! presets >= 5 the frame header just writes `is_global = 0` per reference and
//! no ported code is needed.
//!
//! # CROSS-GROUP: `svt_av1_warp_error` lives in `enc_warped_motion.c`
//!
//! `svt_av1_refine_integerized_param` cannot be ported without it — every
//! candidate step is scored by it — and the inventory marks
//! `svt_av1_warp_error` / `warp_error` MISSING (rows 539-540). It belongs to a
//! different module group. It is ported HERE, deliberately and with this
//! notice, because (a) it is the encoder-side companion of `warped_motion.c`,
//! which this lane does own and has just ported, (b) it is EXPORTED so it
//! lands at tier 1 rather than as an unverified stub, and (c) leaving it out
//! would make the entire GM refinement unportable. If the enc_warped_motion
//! lane ports it too, this is the one to delete — it is a leaf function with a
//! single caller.
//!
//! # What is NOT here, and why
//!
//! * `correspondence_from_mvs` (`global_motion.c:239`) and its dispatcher
//!   `gm_compute_correspondence` (`:341`) walk
//!   `pcs->pa_me_data->me_results[..]->me_candidate_array` and remap block
//!   indices through `me_idx_85_8x8_to_16x16_conversion` /
//!   `me_idx_16x16_to_parent_32x32_conversion`. That is the ME module's data
//!   layout, not this group's, and porting it against a guessed layout would
//!   be a stub wearing a port's name.
//! * `determine_gm_params` (`:364`) is a one-line wrapper over
//!   `svt_aom_ransac` (`Codec/ransac.c`) — double-precision least squares with
//!   a PRNG-driven sample draw. The cost of that item is ransac.c, not the
//!   wrapper.
//!
//! # Evidence
//!
//! Tier 1 throughout — `tests/c_parity_global_motion.rs` drives the real
//! exported `svt_av1_convert_model_to_params`,
//! `svt_av1_is_enough_erroradvantage`, `svt_av1_warp_error` and
//! `svt_av1_refine_integerized_param`. The C-static helpers
//! (`convert_to_params`, `get_wmtype`, `force_wmtype`, `add_param_offset`,
//! `warp_error`) have no exported symbol and are covered TRANSITIVELY through
//! those four, which is what drives them in C.

use svtav1_dsp::port_warp::{
    WARPEDMODEL_PREC_BITS, WarpConvolveParams, get_shear_params, warp_plane,
};
use svtav1_dsp::sad::sad;
use svtav1_types::motion::{TransformationType, WarpedMotionParams};

// --------------------------------------------------------------------------
// Constants (definitions.h:1737-1760, global_motion.h)
// --------------------------------------------------------------------------

/// `GM_TRANS_PREC_BITS` (definitions.h:1737).
pub const GM_TRANS_PREC_BITS: i32 = 6;
/// `GM_ABS_TRANS_BITS` (definitions.h:1738).
pub const GM_ABS_TRANS_BITS: i32 = 12;
/// `GM_TRANS_PREC_DIFF` (definitions.h:1740).
pub const GM_TRANS_PREC_DIFF: i32 = WARPEDMODEL_PREC_BITS - GM_TRANS_PREC_BITS;
/// `GM_TRANS_DECODE_FACTOR` (definitions.h:1742).
pub const GM_TRANS_DECODE_FACTOR: i32 = 1 << GM_TRANS_PREC_DIFF;
/// `GM_ALPHA_PREC_BITS` (definitions.h:1744).
pub const GM_ALPHA_PREC_BITS: i32 = 15;
/// `GM_ABS_ALPHA_BITS` (definitions.h:1745).
pub const GM_ABS_ALPHA_BITS: i32 = 12;
/// `GM_ALPHA_PREC_DIFF` (definitions.h:1746).
pub const GM_ALPHA_PREC_DIFF: i32 = WARPEDMODEL_PREC_BITS - GM_ALPHA_PREC_BITS;
/// `GM_ALPHA_DECODE_FACTOR` (definitions.h:1747).
pub const GM_ALPHA_DECODE_FACTOR: i32 = 1 << GM_ALPHA_PREC_DIFF;
/// `GM_TRANS_MAX` / `GM_TRANS_MIN` (definitions.h:1749/1752).
pub const GM_TRANS_MAX: i32 = 1 << GM_ABS_TRANS_BITS;
pub const GM_TRANS_MIN: i32 = -GM_TRANS_MAX;
/// `GM_ALPHA_MAX` / `GM_ALPHA_MIN` (definitions.h:1750/1753).
pub const GM_ALPHA_MAX: i32 = 1 << GM_ABS_ALPHA_BITS;
pub const GM_ALPHA_MIN: i32 = -GM_ALPHA_MAX;

/// `ERRORADV_BORDER` (global_motion.c:26) — zero in this tree, which makes
/// every `dst + border * d_stride + border` in the refinement a no-op. Kept as
/// a named constant so the arithmetic reads like C's.
pub const ERRORADV_BORDER: i32 = 0;

/// `WARP_ERROR_BLOCK` (enc_warped_motion.c:19).
pub const WARP_ERROR_BLOCK: i32 = 32;

/// `erroradv_tr` (global_motion.c:27).
const ERRORADV_TR: [f64; 3] = [0.65, 0.50, 0.45];
/// `erroradv_prod_tr` (global_motion.c:28).
const ERRORADV_PROD_TR: [f64; 3] = [20000.0, 15000.0, 14000.0];

/// `GM_ERRORADV_TYPE` (global_motion.h:31).
pub const GM_ERRORADV_TR_0: usize = 0;
pub const GM_ERRORADV_TR_1: usize = 1;
pub const GM_ERRORADV_TR_2: usize = 2;

/// `max_trans_model_params` (global_motion.c:120) — how many of the six model
/// parameters each transformation type actually searches.
const MAX_TRANS_MODEL_PARAMS: [usize; 4] = [0, 2, 4, 6];

// --------------------------------------------------------------------------
// Model conversion (global_motion.c:30 / :36 / :51 / :63)
// --------------------------------------------------------------------------

/// Port of `svt_av1_is_enough_erroradvantage` (global_motion.c:30) — the gate
/// that decides whether a global model is kept at all, and also the
/// `rfn_early_exit` test inside the refinement.
///
/// BOTH conditions must hold: the ratio must beat the threshold AND the
/// ratio-times-params-cost product must beat its own threshold. Dropping
/// either keeps models C rejects.
#[inline]
pub fn is_enough_erroradvantage(
    best_erroradvantage: f64,
    params_cost: i32,
    erroradv_type: usize,
) -> bool {
    debug_assert!(erroradv_type < 3);
    best_erroradvantage < ERRORADV_TR[erroradv_type]
        && best_erroradvantage * f64::from(params_cost) < ERRORADV_PROD_TR[erroradv_type]
}

/// Port of `convert_to_params` (global_motion.c:36) — double -> Q16 with the
/// GM_TRANS / GM_ALPHA precision reduction.
///
/// Three details the bit-exactness of the header-coded model rests on:
/// * the rounding is `floor(x * scale + 0.5)`, NOT `rint`/`round` — it is
///   half-up, including for negatives (`floor(-2.5 + 0.5) == -2`), where
///   `round` would give -3.
/// * the diagonal entries are zero-centred BEFORE clamping and re-centred
///   after, so the clamp applies to the deviation from unity, not to the
///   value.
/// * translation is clamped and THEN multiplied by the decode factor; the
///   alpha entries are re-centred and THEN multiplied. The two orders differ.
pub fn convert_to_params(params: &[f64; 6]) -> [i32; 6] {
    let mut model = [0i32; 6];
    for i in 0..2 {
        let v = (params[i] * f64::from(1 << GM_TRANS_PREC_BITS) + 0.5).floor() as i32;
        model[i] = v.clamp(GM_TRANS_MIN, GM_TRANS_MAX) * GM_TRANS_DECODE_FACTOR;
    }
    for i in 2..6 {
        let diag_value = if i == 2 || i == 5 {
            1 << GM_ALPHA_PREC_BITS
        } else {
            0
        };
        let v = (params[i] * f64::from(1 << GM_ALPHA_PREC_BITS) + 0.5).floor() as i32;
        let v = (v - diag_value).clamp(GM_ALPHA_MIN, GM_ALPHA_MAX);
        model[i] = (v + diag_value) * GM_ALPHA_DECODE_FACTOR;
    }
    model
}

/// Port of `get_wmtype` (global_motion.c:51) — classifies a model as
/// IDENTITY / TRANSLATION / ROTZOOM / AFFINE. This is the value coded in the
/// frame header per reference.
///
/// The first test is the FULL identity-diagonal test (`wmmat[5] == 1<<16 &&
/// !wmmat[4] && wmmat[2] == 1<<16 && !wmmat[3]`); only inside it does the
/// translation part decide IDENTITY vs TRANSLATION.
#[inline]
pub fn get_wmtype(gm: &WarpedMotionParams) -> TransformationType {
    let m = &gm.wmmat;
    if m[5] == (1 << WARPEDMODEL_PREC_BITS)
        && m[4] == 0
        && m[2] == (1 << WARPEDMODEL_PREC_BITS)
        && m[3] == 0
    {
        return if m[1] == 0 && m[0] == 0 {
            TransformationType::Identity
        } else {
            TransformationType::Translation
        };
    }
    if m[2] == m[5] && m[3] == -m[4] {
        TransformationType::RotZoom
    } else {
        TransformationType::Affine
    }
}

/// Port of `svt_av1_convert_model_to_params` (global_motion.c:63) — turns a
/// RANSAC double model into the integer `WarpedMotionParams` that get written
/// into the frame header's `global_motion_params`.
pub fn convert_model_to_params(params: &[f64; 6]) -> WarpedMotionParams {
    let mut model = WarpedMotionParams {
        wmmat: convert_to_params(params),
        ..Default::default()
    };
    model.wm_type = get_wmtype(&model);
    model.invalid = false;
    model
}

// --------------------------------------------------------------------------
// Refinement helpers (global_motion.c:72 / :95)
// --------------------------------------------------------------------------

/// Port of `add_param_offset` (global_motion.c:72) — the per-parameter
/// step/clamp used by the refinement hill-climb.
///
/// The zero-centre / shift / clamp / rescale / re-centre sequence is exact and
/// order-sensitive; `param_index` selects both the precision (`< 2` is
/// translation) and whether the parameter is one-centred (2 and 5, the
/// diagonal).
#[inline]
pub fn add_param_offset(param_index: usize, param_value: i32, offset: i32) -> i32 {
    let scale_vals = [GM_TRANS_PREC_DIFF, GM_ALPHA_PREC_DIFF];
    let clamp_vals = [GM_TRANS_MAX, GM_ALPHA_MAX];
    let param_type = usize::from(param_index >= 2);
    let is_one_centered = i32::from(param_index == 2 || param_index == 5);

    let mut v =
        (param_value - (is_one_centered << WARPEDMODEL_PREC_BITS)) >> scale_vals[param_type];
    v += offset;
    v = v.clamp(-clamp_vals[param_type], clamp_vals[param_type]);
    v *= 1 << scale_vals[param_type];
    v + (is_one_centered << WARPEDMODEL_PREC_BITS)
}

/// Port of `force_wmtype` (global_motion.c:95) — zeroes the parameters above
/// the searched model order.
///
/// C uses FALLTHROUGH deliberately: IDENTITY falls into TRANSLATION falls into
/// ROTZOOM falls into AFFINE, so forcing IDENTITY zeroes the translation AND
/// resets the diagonal AND derives `wmmat[4..6]`. A `match` without the
/// cascade would leave stale parameters behind, and this function is called
/// TWICE inside `refine_integerized_param` — before and after the climb — so
/// the difference reaches the coded model.
pub fn force_wmtype(wm: &mut WarpedMotionParams, wmtype: TransformationType) {
    // Fallthrough cascade, written out.
    if wmtype == TransformationType::Identity {
        wm.wmmat[0] = 0;
        wm.wmmat[1] = 0;
    }
    if wmtype == TransformationType::Identity || wmtype == TransformationType::Translation {
        wm.wmmat[2] = 1 << WARPEDMODEL_PREC_BITS;
        wm.wmmat[3] = 0;
    }
    if wmtype == TransformationType::Identity
        || wmtype == TransformationType::Translation
        || wmtype == TransformationType::RotZoom
    {
        wm.wmmat[4] = -wm.wmmat[3];
        wm.wmmat[5] = wm.wmmat[2];
    }
    wm.wm_type = wmtype;
}

// --------------------------------------------------------------------------
// svt_av1_warp_error (enc_warped_motion.c:21 / :77) — CROSS-GROUP, see the
// module doc.
// --------------------------------------------------------------------------

/// Port of the `static` `warp_error` (enc_warped_motion.c:21).
///
/// Warps the reference in `WARP_ERROR_BLOCK`-sized tiles and accumulates SAD
/// against `dst`. Two behaviours that are easy to lose:
/// * **the chess pattern.** With `chess_refn`, every other ROW of tiles starts
///   one block to the right (`jstart = (i_itr & 1) ? p_col : p_col +
///   WARP_ERROR_BLOCK`) and steps by TWO blocks, then the total is DOUBLED at
///   the end. `i_itr` counts tile-rows, not pixels.
/// * **the early-out.** As soon as the running sum exceeds `best_error` the
///   function returns it IMMEDIATELY — un-doubled, and mid-frame. So the
///   return value is not a complete error whenever it exceeds the bound, and
///   callers rely on that only as an upper bound.
#[allow(clippy::too_many_arguments)]
pub fn warp_error(
    wm: &mut WarpedMotionParams,
    reference: &[u8],
    width: i32,
    height: i32,
    stride: usize,
    dst: &[u8],
    dst_origin: usize,
    p_col: i32,
    p_row: i32,
    p_width: i32,
    p_height: i32,
    p_stride: usize,
    subsampling_x: i32,
    subsampling_y: i32,
    chess_refn: bool,
    best_error: i64,
) -> i64 {
    let mut gm_sumerr: i64 = 0;
    let error_bsize_w = p_width.min(WARP_ERROR_BLOCK);
    let error_bsize_h = p_height.min(WARP_ERROR_BLOCK);
    let mut tmp = [0u8; (WARP_ERROR_BLOCK * WARP_ERROR_BLOCK) as usize];
    let conv_params = WarpConvolveParams::simple(false, 8);

    let mut i_itr = 0i32;
    let mut i = p_row;
    while i < p_row + p_height {
        let (jstart, jstep) = if chess_refn {
            (
                if i_itr & 1 != 0 {
                    p_col
                } else {
                    p_col + WARP_ERROR_BLOCK
                },
                2,
            )
        } else {
            (p_col, 1)
        };

        let mut j = jstart;
        while j < p_col + p_width {
            let warp_w = error_bsize_w.min(p_col + p_width - j);
            let warp_h = error_bsize_h.min(p_row + p_height - i);
            warp_plane(
                wm,
                reference,
                width,
                height,
                stride,
                &mut tmp,
                None,
                j,
                i,
                warp_w,
                warp_h,
                WARP_ERROR_BLOCK as usize,
                subsampling_x,
                subsampling_y,
                &conv_params,
            );
            let d = dst_origin + j as usize + i as usize * p_stride;
            gm_sumerr += i64::from(sad(
                &tmp,
                WARP_ERROR_BLOCK as usize,
                &dst[d..],
                p_stride,
                warp_w as usize,
                warp_h as usize,
            ));
            if gm_sumerr > best_error {
                // Deliberately NOT doubled — C returns here.
                return gm_sumerr;
            }
            j += jstep * WARP_ERROR_BLOCK;
        }
        i_itr += 1;
        i += WARP_ERROR_BLOCK;
    }

    if chess_refn { gm_sumerr * 2 } else { gm_sumerr }
}

/// Port of `svt_av1_warp_error` (enc_warped_motion.c:77) — derives the shear
/// first and returns the sentinel `1` if the model is not warp-legal.
#[allow(clippy::too_many_arguments)]
pub fn av1_warp_error(
    wm: &mut WarpedMotionParams,
    reference: &[u8],
    width: i32,
    height: i32,
    stride: usize,
    dst: &[u8],
    dst_origin: usize,
    p_col: i32,
    p_row: i32,
    p_width: i32,
    p_height: i32,
    p_stride: usize,
    subsampling_x: i32,
    subsampling_y: i32,
    chess_refn: bool,
    best_error: i64,
) -> i64 {
    if wm.wm_type as u8 <= TransformationType::Affine as u8 && !get_shear_params(wm) {
        // The sentinel is 1, not 0 and not i64::MAX.
        return 1;
    }
    warp_error(
        wm,
        reference,
        width,
        height,
        stride,
        dst,
        dst_origin,
        p_col,
        p_row,
        p_width,
        p_height,
        p_stride,
        subsampling_x,
        subsampling_y,
        chess_refn,
        best_error,
    )
}

// --------------------------------------------------------------------------
// svt_av1_refine_integerized_param (global_motion.c:117)
// --------------------------------------------------------------------------

/// The `GmControls` fields this function reads.
#[derive(Debug, Clone, Copy, Default)]
pub struct GmRefineCtrls {
    /// `GmControls::rfn_early_exit` — 0 off, 1 enable early exit from the
    /// parameter refinement.
    pub rfn_early_exit: bool,
}

/// Port of `svt_av1_refine_integerized_param` (global_motion.c:117) — the
/// integer refinement hill-climb that produces the final coded global model.
///
/// `wm` is refined IN PLACE. The step starts at 16 (`1 << (5 - 1)`) and halves
/// each refinement iteration. For each parameter C tries `-step` then `+step`
/// from the SAME `curr_param` (not from the just-improved value), keeps the
/// better of the three, and writes it back before moving to the next
/// parameter. `force_wmtype` runs BEFORE the climb and again after, and the
/// final `wm_type` is then RE-DERIVED by `get_wmtype` — so a model forced to
/// ROTZOOM can come out classified IDENTITY.
///
/// The early exit is `rfn_early_exit && !is_enough_erroradvantage(best_error /
/// pic_sad, params_cost, GM_ERRORADV_TR_1)` — note it returns `best_error`
/// WITHOUT the trailing `force_wmtype` / `get_wmtype`, leaving `wm` as
/// `force_wmtype(wmtype)` left it.
#[allow(clippy::too_many_arguments)]
pub fn refine_integerized_param(
    ctrls: &GmRefineCtrls,
    wm: &mut WarpedMotionParams,
    wmtype: TransformationType,
    reference: &[u8],
    r_width: i32,
    r_height: i32,
    r_stride: usize,
    dst: &[u8],
    d_width: i32,
    d_height: i32,
    d_stride: usize,
    n_refinements: i32,
    chess_refn: bool,
    best_frame_error: i64,
    pic_sad: u32,
    params_cost: i32,
) -> i64 {
    let border = ERRORADV_BORDER;
    let n_params = MAX_TRANS_MODEL_PARAMS[wmtype as usize];

    force_wmtype(wm, wmtype);
    let dst_origin = (border as usize) * d_stride + border as usize;
    let mut best_error = av1_warp_error(
        wm,
        reference,
        r_width,
        r_height,
        r_stride,
        dst,
        dst_origin,
        border,
        border,
        d_width - 2 * border,
        d_height - 2 * border,
        d_stride,
        0,
        0,
        chess_refn,
        best_frame_error,
    );
    best_error = best_error.min(best_frame_error);
    if ctrls.rfn_early_exit
        && !is_enough_erroradvantage(
            best_error as f64 / f64::from(pic_sad),
            params_cost,
            GM_ERRORADV_TR_1,
        )
    {
        return best_error;
    }

    let mut step: i32 = 1 << (5 - 1); // initial step = 16
    for _ in 0..n_refinements {
        for p in 0..n_params {
            let curr_param = wm.wmmat[p];
            let mut best_param = curr_param;

            // look to the left
            wm.wmmat[p] = add_param_offset(p, curr_param, -step);
            let step_error = av1_warp_error(
                wm,
                reference,
                r_width,
                r_height,
                r_stride,
                dst,
                dst_origin,
                border,
                border,
                d_width - 2 * border,
                d_height - 2 * border,
                d_stride,
                0,
                0,
                chess_refn,
                best_error,
            );
            if step_error < best_error {
                best_error = step_error;
                best_param = wm.wmmat[p];
            }

            // look to the right
            wm.wmmat[p] = add_param_offset(p, curr_param, step);
            let step_error = av1_warp_error(
                wm,
                reference,
                r_width,
                r_height,
                r_stride,
                dst,
                dst_origin,
                border,
                border,
                d_width - 2 * border,
                d_height - 2 * border,
                d_stride,
                0,
                0,
                chess_refn,
                best_error,
            );
            if step_error < best_error {
                best_error = step_error;
                best_param = wm.wmmat[p];
            }
            wm.wmmat[p] = best_param;
        }
        step >>= 1;
    }
    force_wmtype(wm, wmtype);
    wm.wm_type = get_wmtype(wm);
    best_error
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convert_to_params_rounds_half_up_not_to_even() {
        // floor(x*scale + 0.5). At exactly .5 the result rounds UP (toward
        // +inf), including for negatives: floor(-2.5 + 0.5) == -2.
        let scale = f64::from(1 << GM_TRANS_PREC_BITS);
        let p = [2.5 / scale, -2.5 / scale, 1.0, 0.0, 0.0, 1.0];
        let m = convert_to_params(&p);
        assert_eq!(m[0], 3 * GM_TRANS_DECODE_FACTOR);
        assert_eq!(m[1], -2 * GM_TRANS_DECODE_FACTOR);
    }

    #[test]
    fn identity_double_model_classifies_as_identity() {
        let wm = convert_model_to_params(&[0.0, 0.0, 1.0, 0.0, 0.0, 1.0]);
        assert_eq!(wm.wm_type, TransformationType::Identity);
        assert_eq!(wm.wmmat[2], 1 << WARPEDMODEL_PREC_BITS);
        assert_eq!(wm.wmmat[5], 1 << WARPEDMODEL_PREC_BITS);
    }

    #[test]
    fn translation_only_model_classifies_as_translation() {
        let wm = convert_model_to_params(&[3.0, -2.0, 1.0, 0.0, 0.0, 1.0]);
        assert_eq!(wm.wm_type, TransformationType::Translation);
    }

    #[test]
    fn force_wmtype_cascade_zeroes_everything_above_the_order() {
        let mut wm = WarpedMotionParams {
            wmmat: [11, 22, 33, 44, 55, 66],
            ..Default::default()
        };
        force_wmtype(&mut wm, TransformationType::Identity);
        // IDENTITY must fall through TRANSLATION and ROTZOOM: translation
        // zeroed, diagonal reset, and 4/5 DERIVED from the reset 2/3.
        assert_eq!(
            wm.wmmat,
            [
                0,
                0,
                1 << WARPEDMODEL_PREC_BITS,
                0,
                0,
                1 << WARPEDMODEL_PREC_BITS
            ]
        );
        assert_eq!(wm.wm_type, TransformationType::Identity);

        // ROTZOOM keeps 0..4 and derives 4/5 only.
        let mut wm = WarpedMotionParams {
            wmmat: [11, 22, 33, 44, 55, 66],
            ..Default::default()
        };
        force_wmtype(&mut wm, TransformationType::RotZoom);
        assert_eq!(wm.wmmat, [11, 22, 33, 44, -44, 33]);

        // AFFINE changes nothing but the type.
        let mut wm = WarpedMotionParams {
            wmmat: [11, 22, 33, 44, 55, 66],
            ..Default::default()
        };
        force_wmtype(&mut wm, TransformationType::Affine);
        assert_eq!(wm.wmmat, [11, 22, 33, 44, 55, 66]);
    }

    #[test]
    fn erroradvantage_needs_both_conditions() {
        // Ratio passes, product fails.
        assert!(!is_enough_erroradvantage(0.4, 100_000, GM_ERRORADV_TR_1));
        // Product passes, ratio fails.
        assert!(!is_enough_erroradvantage(0.9, 1, GM_ERRORADV_TR_1));
        // Both pass.
        assert!(is_enough_erroradvantage(0.4, 100, GM_ERRORADV_TR_1));
    }

    #[test]
    fn add_param_offset_is_one_centered_on_the_diagonal_only() {
        // Parameter 2 (diagonal): an offset of 0 on the unity value must be a
        // fixed point.
        let unity = 1 << WARPEDMODEL_PREC_BITS;
        assert_eq!(add_param_offset(2, unity, 0), unity);
        assert_eq!(add_param_offset(5, unity, 0), unity);
        // Parameter 3 (off-diagonal): zero is the fixed point.
        assert_eq!(add_param_offset(3, 0, 0), 0);
        // Parameter 0 (translation): the shift is GM_TRANS_PREC_DIFF, not
        // GM_ALPHA_PREC_DIFF, so the quantisation step differs.
        assert_eq!(add_param_offset(0, 0, 1), 1 << GM_TRANS_PREC_DIFF);
        assert_eq!(add_param_offset(3, 0, 1), 1 << GM_ALPHA_PREC_DIFF);
    }
}
