//! The motion-compensation dispatchers.
//!
//! Ported from `Source/Lib/Codec/inter_prediction.c` (SVT-AV1 v4.2.0):
//! `convolve_2d_for_intrabc` (:1194), `highbd_convolve_2d_for_intrabc` (:1237),
//! `svt_inter_predictor_pd0` (:1256), `svt_inter_predictor_light_pd1` (:1283),
//! `svt_inter_predictor` (:1386) and `svt_highbd_inter_predictor` (:1444);
//! plus the header inlines `av1_extract_interp_filter`,
//! `av1_make_interp_filters` (filter.h:60-68) and
//! `av1_get_convolve_filter_params` (inter_prediction.h:139).
//!
//! # The trap in the filter packing, read from the source rather than inferred
//!
//! `av1_extract_interp_filter(filters, x_filter)` shifts right by 16 when its
//! second argument is **truthy** — it is a flag, not an index — and
//! `av1_get_convolve_filter_params` passes `1` for X and `0` for Y. So the X
//! filter lives in the HIGH half of the packed `InterpFilters` word and Y in
//! the LOW half, and `av1_make_interp_filters(y, x)` takes Y **first**.
//! Reading that pair the other way round silently swaps the two axes' filters
//! on every block whose axes differ — which the switchable-filter path picks
//! constantly. [`filters_match_c`-style gating lives in the parity test.]
//!
//! # Scaled references
//!
//! Both `is_scaled` arms call `svt_av1_convolve_2d_scale` /
//! `svt_av1_highbd_convolve_2d_scale`. Those landed in
//! [`crate::port_convolve_scale`] (C-gated at tier 1), so these entry points
//! now take the scaled path rather than refusing it. They previously returned
//! `McError::ScaledReferenceNotPorted` — a refusal rather than a
//! plausible-but-wrong prediction (`WORKING-ON-THIS.md` §6) — and the
//! refusal-pinning cell has been replaced by a real scaled-parity cell, which
//! is strictly stronger evidence.
//!
//! `svtav1-dsp/src/scale.rs` is untouched: it is a different, homegrown
//! approximation, and `tests/c_parity_scale.rs` still pins its divergence.

use crate::port_convolve::{
    ConvolveParams, FilterParams, InterpFilterKind, SrcView, convolve_2d_copy_sr, convolve_2d_sr,
    convolve_x_sr, convolve_y_sr, interp_filter_params_list, interp_filter_params_with_block_size,
    jnt_convolve_2d, jnt_convolve_2d_copy, jnt_convolve_x, jnt_convolve_y,
};
use crate::port_convolve_hbd::{
    SrcView16, highbd_convolve_2d_copy_sr, highbd_convolve_2d_sr, highbd_convolve_x_sr,
    highbd_convolve_y_sr, highbd_jnt_convolve_2d, highbd_jnt_convolve_2d_copy,
    highbd_jnt_convolve_x, highbd_jnt_convolve_y,
};
use crate::port_convolve_scale::{convolve_2d_scale, highbd_convolve_2d_scale};
use crate::port_scale_factors::{SubpelParams, has_scale, revert_scale_extra_bits};

/// C's packed `InterpFilters` word: Y filter in the low 16 bits, X in the high.
pub type InterpFilters = u32;

/// `av1_extract_interp_filter` (filter.h:60).
///
/// `x_filter` is a FLAG, not an index: any nonzero value selects the high half.
pub fn extract_interp_filter(filters: InterpFilters, x_filter: bool) -> InterpFilterKind {
    let raw = (filters >> if x_filter { 16 } else { 0 }) & 0xffff;
    match raw {
        0 => InterpFilterKind::EightTapRegular,
        1 => InterpFilterKind::EightTapSmooth,
        2 => InterpFilterKind::MultiTapSharp,
        3 => InterpFilterKind::Bilinear,
        other => panic!("InterpFilters word carries an out-of-range filter index {other}"),
    }
}

/// `av1_make_interp_filters` (filter.h:64) — note the Y filter comes FIRST.
pub fn make_interp_filters(
    y_filter: InterpFilterKind,
    x_filter: InterpFilterKind,
) -> InterpFilters {
    (y_filter as u32 & 0xffff) | ((x_filter as u32 & 0xffff) << 16)
}

/// `av1_broadcast_interp_filter` (filter.h:70).
pub fn broadcast_interp_filter(f: InterpFilterKind) -> InterpFilters {
    make_interp_filters(f, f)
}

/// `av1_get_convolve_filter_params` (inter_prediction.h:139) — the X params are
/// selected with the block WIDTH and the Y params with the block HEIGHT.
pub fn get_convolve_filter_params(
    filters: InterpFilters,
    w: i32,
    h: i32,
) -> (FilterParams, FilterParams) {
    (
        interp_filter_params_with_block_size(extract_interp_filter(filters, true), w),
        interp_filter_params_with_block_size(extract_interp_filter(filters, false), h),
    )
}

/// `svt_aom_convolve[subX][subY][bi]` (inter_prediction.c:1116) dispatched
/// directly. `conv_buf` is `conv_params->dst`; it is read/written only on the
/// compound entries.
#[allow(clippy::too_many_arguments)]
fn dispatch_convolve_8(
    src: SrcView<'_>,
    dst: &mut [u8],
    dst_stride: usize,
    conv_buf: &mut [u16],
    w: usize,
    h: usize,
    fx: &FilterParams,
    fy: &FilterParams,
    subpel_x: i32,
    subpel_y: i32,
    conv_params: &ConvolveParams,
) {
    match (subpel_x != 0, subpel_y != 0, conv_params.is_compound) {
        (false, false, false) => convolve_2d_copy_sr(src, dst, dst_stride, w, h),
        (false, false, true) => {
            jnt_convolve_2d_copy(src, dst, dst_stride, conv_buf, w, h, conv_params)
        }
        (false, true, false) => convolve_y_sr(src, dst, dst_stride, w, h, fy, subpel_y),
        (false, true, true) => jnt_convolve_y(
            src,
            dst,
            dst_stride,
            conv_buf,
            w,
            h,
            fy,
            subpel_y,
            conv_params,
        ),
        (true, false, false) => {
            convolve_x_sr(src, dst, dst_stride, w, h, fx, subpel_x, conv_params)
        }
        (true, false, true) => jnt_convolve_x(
            src,
            dst,
            dst_stride,
            conv_buf,
            w,
            h,
            fx,
            subpel_x,
            conv_params,
        ),
        (true, true, false) => convolve_2d_sr(
            src,
            dst,
            dst_stride,
            w,
            h,
            fx,
            fy,
            subpel_x,
            subpel_y,
            conv_params,
        ),
        (true, true, true) => jnt_convolve_2d(
            src,
            dst,
            dst_stride,
            conv_buf,
            w,
            h,
            fx,
            fy,
            subpel_x,
            subpel_y,
            conv_params,
        ),
    }
}

/// `svt_aom_convolveHbd[subX][subY][bi]` (inter_prediction.c:1094).
#[allow(clippy::too_many_arguments)]
fn dispatch_convolve_hbd(
    src: SrcView16<'_>,
    dst: &mut [u16],
    dst_stride: usize,
    conv_buf: &mut [u16],
    w: usize,
    h: usize,
    fx: &FilterParams,
    fy: &FilterParams,
    subpel_x: i32,
    subpel_y: i32,
    conv_params: &ConvolveParams,
    bd: i32,
) {
    match (subpel_x != 0, subpel_y != 0, conv_params.is_compound) {
        (false, false, false) => highbd_convolve_2d_copy_sr(src, dst, dst_stride, w, h),
        (false, false, true) => {
            highbd_jnt_convolve_2d_copy(src, dst, dst_stride, conv_buf, w, h, conv_params, bd)
        }
        (false, true, false) => highbd_convolve_y_sr(src, dst, dst_stride, w, h, fy, subpel_y, bd),
        (false, true, true) => highbd_jnt_convolve_y(
            src,
            dst,
            dst_stride,
            conv_buf,
            w,
            h,
            fy,
            subpel_y,
            conv_params,
            bd,
        ),
        (true, false, false) => {
            highbd_convolve_x_sr(src, dst, dst_stride, w, h, fx, subpel_x, conv_params, bd)
        }
        (true, false, true) => highbd_jnt_convolve_x(
            src,
            dst,
            dst_stride,
            conv_buf,
            w,
            h,
            fx,
            subpel_x,
            conv_params,
            bd,
        ),
        (true, true, false) => highbd_convolve_2d_sr(
            src,
            dst,
            dst_stride,
            w,
            h,
            fx,
            fy,
            subpel_x,
            subpel_y,
            conv_params,
            bd,
        ),
        (true, true, true) => highbd_jnt_convolve_2d(
            src,
            dst,
            dst_stride,
            conv_buf,
            w,
            h,
            fx,
            fy,
            subpel_x,
            subpel_y,
            conv_params,
            bd,
        ),
    }
}

/// `convolve_2d_for_intrabc` (inter_prediction.c:1194).
///
/// A BILINEAR pair at the fixed phase 8 (half-pel), dispatched on which axes
/// are sub-pel — note the phase passed to the kernel is the literal `8`, not
/// the caller's `subpel_*_q4`, which is only used as a boolean here.
pub fn convolve_2d_for_intrabc(
    src: SrcView<'_>,
    dst: &mut [u8],
    dst_stride: usize,
    w: usize,
    h: usize,
    subpel_x_q4: i32,
    subpel_y_q4: i32,
    conv_params: &ConvolveParams,
) {
    let bil = interp_filter_params_list(InterpFilterKind::Bilinear);
    if subpel_x_q4 != 0 && subpel_y_q4 != 0 {
        convolve_2d_sr(src, dst, dst_stride, w, h, &bil, &bil, 8, 8, conv_params);
    } else if subpel_x_q4 != 0 {
        convolve_x_sr(src, dst, dst_stride, w, h, &bil, 8, conv_params);
    } else {
        convolve_y_sr(src, dst, dst_stride, w, h, &bil, 8);
    }
}

/// `highbd_convolve_2d_for_intrabc` (inter_prediction.c:1237).
pub fn highbd_convolve_2d_for_intrabc(
    src: SrcView16<'_>,
    dst: &mut [u16],
    dst_stride: usize,
    w: usize,
    h: usize,
    subpel_x_q4: i32,
    subpel_y_q4: i32,
    conv_params: &ConvolveParams,
    bd: i32,
) {
    let bil = interp_filter_params_list(InterpFilterKind::Bilinear);
    if subpel_x_q4 != 0 && subpel_y_q4 != 0 {
        highbd_convolve_2d_sr(
            src,
            dst,
            dst_stride,
            w,
            h,
            &bil,
            &bil,
            8,
            8,
            conv_params,
            bd,
        );
    } else if subpel_x_q4 != 0 {
        highbd_convolve_x_sr(src, dst, dst_stride, w, h, &bil, 8, conv_params, bd);
    } else {
        highbd_convolve_y_sr(src, dst, dst_stride, w, h, &bil, 8, bd);
    }
}

/// `svt_inter_predictor_pd0` (inter_prediction.c:1256) — PD_PASS_0's MC entry.
///
/// The unscaled arm indexes `svt_aom_convolve[0][0][is_compound]` with LITERAL
/// zeros: whatever `subpel_params` holds, PD0's whole MC surface is the
/// whole-pel copy (and its compound twin). That is measured from the source,
/// not assumed — the `subpel_params` argument is `UNUSED` on that arm.
pub fn inter_predictor_pd0(
    src: SrcView<'_>,
    dst: &mut [u8],
    dst_stride: usize,
    conv_buf: &mut [u16],
    w: usize,
    h: usize,
    subpel_params: &SubpelParams,
    conv_params: &ConvolveParams,
) {
    if has_scale(subpel_params.xs, subpel_params.ys) {
        // C builds the filters from
        // `av1_make_interp_filters(EIGHTTAP_REGULAR, EIGHTTAP_REGULAR)` here —
        // PD0 pins the filter pair regardless of the block's own choice.
        let (fx, fy) = get_convolve_filter_params(
            broadcast_interp_filter(InterpFilterKind::EightTapRegular),
            w as i32,
            h as i32,
        );
        convolve_2d_scale(
            src,
            dst,
            dst_stride,
            conv_buf,
            w,
            h,
            &fx,
            &fy,
            subpel_params.subpel_x,
            subpel_params.xs,
            subpel_params.subpel_y,
            subpel_params.ys,
            conv_params,
        );
        return;
    }
    if conv_params.is_compound {
        jnt_convolve_2d_copy(src, dst, dst_stride, conv_buf, w, h, conv_params);
    } else {
        convolve_2d_copy_sr(src, dst, dst_stride, w, h);
    }
}

/// `svt_inter_predictor` (inter_prediction.c:1386) — the 8-bit full-PD1 MC
/// dispatcher.
///
/// `sf` is `assert`ed non-NULL by C and then `UNUSED`; the scaled/unscaled
/// decision comes from `subpel_params` alone, so no scale factors are taken.
#[allow(clippy::too_many_arguments)]
pub fn inter_predictor(
    src: SrcView<'_>,
    dst: &mut [u8],
    dst_stride: usize,
    conv_buf: &mut [u16],
    subpel_params: &SubpelParams,
    w: usize,
    h: usize,
    conv_params: &ConvolveParams,
    interp_filters: InterpFilters,
    is_intrabc: bool,
) {
    let (fx, fy) = get_convolve_filter_params(interp_filters, w as i32, h as i32);
    if has_scale(subpel_params.xs, subpel_params.ys) {
        // C asserts IMPLIES(is_intrabc, !is_scaled), so the intrabc arm inside
        // the scaled branch is unreachable in a well-formed call; it is kept
        // because C keeps it.
        if is_intrabc && (subpel_params.subpel_x != 0 || subpel_params.subpel_y != 0) {
            convolve_2d_for_intrabc(
                src,
                dst,
                dst_stride,
                w,
                h,
                subpel_params.subpel_x,
                subpel_params.subpel_y,
                conv_params,
            );
            return;
        }
        convolve_2d_scale(
            src,
            dst,
            dst_stride,
            conv_buf,
            w,
            h,
            &fx,
            &fy,
            subpel_params.subpel_x,
            subpel_params.xs,
            subpel_params.subpel_y,
            subpel_params.ys,
            conv_params,
        );
        return;
    }
    let mut sp = *subpel_params;
    revert_scale_extra_bits(&mut sp);
    if is_intrabc && (sp.subpel_x != 0 || sp.subpel_y != 0) {
        convolve_2d_for_intrabc(
            src,
            dst,
            dst_stride,
            w,
            h,
            sp.subpel_x,
            sp.subpel_y,
            conv_params,
        );
        return;
    }
    dispatch_convolve_8(
        src,
        dst,
        dst_stride,
        conv_buf,
        w,
        h,
        &fx,
        &fy,
        sp.subpel_x,
        sp.subpel_y,
        conv_params,
    );
}

/// `svt_highbd_inter_predictor` (inter_prediction.c:1444).
#[allow(clippy::too_many_arguments)]
pub fn highbd_inter_predictor(
    src: SrcView16<'_>,
    dst: &mut [u16],
    dst_stride: usize,
    conv_buf: &mut [u16],
    subpel_params: &SubpelParams,
    w: usize,
    h: usize,
    conv_params: &ConvolveParams,
    interp_filters: InterpFilters,
    is_intrabc: bool,
    bd: i32,
) {
    let (fx, fy) = get_convolve_filter_params(interp_filters, w as i32, h as i32);
    if has_scale(subpel_params.xs, subpel_params.ys) {
        if is_intrabc && (subpel_params.subpel_x != 0 || subpel_params.subpel_y != 0) {
            highbd_convolve_2d_for_intrabc(
                src,
                dst,
                dst_stride,
                w,
                h,
                subpel_params.subpel_x,
                subpel_params.subpel_y,
                conv_params,
                bd,
            );
            return;
        }
        highbd_convolve_2d_scale(
            src,
            dst,
            dst_stride,
            conv_buf,
            w,
            h,
            &fx,
            &fy,
            subpel_params.subpel_x,
            subpel_params.xs,
            subpel_params.subpel_y,
            subpel_params.ys,
            conv_params,
            bd,
        );
        return;
    }
    let mut sp = *subpel_params;
    revert_scale_extra_bits(&mut sp);
    if is_intrabc && (sp.subpel_x != 0 || sp.subpel_y != 0) {
        highbd_convolve_2d_for_intrabc(
            src,
            dst,
            dst_stride,
            w,
            h,
            sp.subpel_x,
            sp.subpel_y,
            conv_params,
            bd,
        );
        return;
    }
    dispatch_convolve_hbd(
        src,
        dst,
        dst_stride,
        conv_buf,
        w,
        h,
        &fx,
        &fy,
        sp.subpel_x,
        sp.subpel_y,
        conv_params,
        bd,
    );
}

/// `svt_inter_predictor_light_pd1` (inter_prediction.c:1283), **8-bit arm
/// only**.
///
/// C's `bd > EB_EIGHT_BIT` arm packs `src` (8 MSB) and `src_2b` (2 LSB) into a
/// 10-bit scratch with `svt_aom_pack_block` before convolving. This port
/// carries plain `u16` planes by design (`bd10.rs`), so the packed-buffer
/// representation has no counterpart here and the 10-bit light-PD1 path goes
/// through [`highbd_inter_predictor`] instead. That arm is therefore NOT
/// ported — it is not "done", it is out of scope, and it is named as missing.
///
/// Unlike [`inter_predictor_pd0`] the filters here are the caller's, so
/// light-PD1 does reach every kernel in the table.
#[allow(clippy::too_many_arguments)]
pub fn inter_predictor_light_pd1_8bit(
    src: SrcView<'_>,
    dst: &mut [u8],
    dst_stride: usize,
    conv_buf: &mut [u16],
    w: usize,
    h: usize,
    interp_filters: InterpFilters,
    subpel_params: &SubpelParams,
    conv_params: &ConvolveParams,
) {
    let (fx, fy) = get_convolve_filter_params(interp_filters, w as i32, h as i32);
    if has_scale(subpel_params.xs, subpel_params.ys) {
        convolve_2d_scale(
            src,
            dst,
            dst_stride,
            conv_buf,
            w,
            h,
            &fx,
            &fy,
            subpel_params.subpel_x,
            subpel_params.xs,
            subpel_params.subpel_y,
            subpel_params.ys,
            conv_params,
        );
        return;
    }
    let mut sp = *subpel_params;
    revert_scale_extra_bits(&mut sp);
    dispatch_convolve_8(
        src,
        dst,
        dst_stride,
        conv_buf,
        w,
        h,
        &fx,
        &fy,
        sp.subpel_x,
        sp.subpel_y,
        conv_params,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The packing order: Y low, X high. Swapping the two would make this
    /// symmetric and it deliberately is not.
    #[test]
    fn interp_filters_packing_puts_x_high() {
        let f = make_interp_filters(
            InterpFilterKind::EightTapSmooth,
            InterpFilterKind::MultiTapSharp,
        );
        assert_eq!(f, 1 | (2 << 16));
        assert_eq!(
            extract_interp_filter(f, true),
            InterpFilterKind::MultiTapSharp
        );
        assert_eq!(
            extract_interp_filter(f, false),
            InterpFilterKind::EightTapSmooth
        );
        let b = broadcast_interp_filter(InterpFilterKind::Bilinear);
        assert_eq!(extract_interp_filter(b, true), InterpFilterKind::Bilinear);
        assert_eq!(extract_interp_filter(b, false), InterpFilterKind::Bilinear);
    }

    /// The scaled arm now runs the scaled kernel instead of refusing; that
    /// path is C-gated in `c_parity_port_inter_predictor.rs`. What stays
    /// checkable here is that `has_scale` is what selects it.
    #[test]
    fn has_scale_selects_the_scaled_arm() {
        use crate::port_scale_factors::{SCALE_SUBPEL_SHIFTS, has_scale};
        assert!(!has_scale(SCALE_SUBPEL_SHIFTS, SCALE_SUBPEL_SHIFTS));
        assert!(has_scale(SCALE_SUBPEL_SHIFTS + 1, SCALE_SUBPEL_SHIFTS));
        assert!(has_scale(SCALE_SUBPEL_SHIFTS, SCALE_SUBPEL_SHIFTS - 1));
    }
}
