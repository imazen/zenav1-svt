//! The temporal-filter motion compensation, EXECUTABLE.
//!
//! Ported from `Source/Lib/Codec/enc_inter_prediction.c` (SVT-AV1 v4.2.0):
//! `tf_inter_predictor` (:2452) and `svt_aom_simple_luma_unipred` (:2677).
//!
//! # What already existed
//!
//! [`crate::port_ifs::SimpleLumaUnipred`] carries the fixed parameter set C
//! builds (`conv_buf` stride 128, `is_compound = 0`) and the destination
//! offset, and says in its own doc that wiring it in is a caller-side change
//! not done there. Nothing ran either function. This module does.
//!
//! # Three details that are easy to get wrong
//!
//! * `subsampling_shift` does NOT scale the block width. It shifts BOTH
//!   strides LEFT and the block HEIGHT right (:2481-2483): the caller is
//!   asking for every other row of a plane whose rows are twice as far apart,
//!   which is how TF builds a half-height view without copying.
//! * `compute_subpel_params` is called with `ss_y = ss_x = 0` HARDWIRED
//!   (:2470-2471), even on a chroma-shaped call.
//! * The 10-bit arm's source offset is `(pos_x + pos_y * src_stride) *
//!   (1 << is_highbd)` — C is stepping a `uint8_t*` over `uint16_t` samples,
//!   so it doubles. There is no 2-bit companion plane on this path at all;
//!   indexing a typed slice makes both depths the same expression.
//!
//! # Evidence
//!
//! TIER 1 — `tf_inter_predictor` and `svt_aom_simple_luma_unipred` are both
//! exported symbols (`nm`: `T`), gated in `tests/c_parity_port_tf_pred.rs`.

use crate::port_convolve::{ConvolveParams, SrcView};
use crate::port_convolve_hbd::SrcView16;
use crate::port_inter_predictor::{InterpFilters, highbd_inter_predictor, inter_predictor};
use crate::port_scale_factors::ScaleFactors;
use crate::port_subpel_params::{MbEdges, Mv, RefGeometry, compute_subpel_params};

/// The reference plane, at whichever depth. There is no split 8+2 form on this
/// path: `tf_inter_predictor` takes one pointer and casts it.
#[derive(Debug, Clone, Copy)]
pub enum TfSrc<'a> {
    /// 8-bit.
    Lbd(&'a [u8]),
    /// 10 or 12-bit.
    Hbd(&'a [u16]),
}

/// The prediction output, matching the source's depth.
#[derive(Debug)]
pub enum TfDst<'a> {
    /// 8-bit.
    Lbd(&'a mut [u8]),
    /// 10 or 12-bit.
    Hbd(&'a mut [u16]),
}

/// The depth of a `TfSrc` / `TfDst` pair disagreed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TfDepthMismatch;

/// `tf_inter_predictor` (enc_inter_prediction.c:2452).
///
/// `src_origin` is where the reference plane's (0, 0) sits in the slice, in
/// SAMPLES; C's `(1 << is_highbd)` byte doubling is the same address once the
/// slice is typed.
#[allow(clippy::too_many_arguments)]
pub fn tf_inter_predictor(
    src: TfSrc<'_>,
    src_origin: usize,
    src_stride: usize,
    dst: TfDst<'_>,
    dst_stride: usize,
    conv_buf: &mut [u16],
    pre_y: i32,
    pre_x: i32,
    mv: Mv,
    sf: &ScaleFactors,
    conv_params: &ConvolveParams,
    interp_filters: InterpFilters,
    geom: RefGeometry,
    blk_width: usize,
    blk_height: usize,
    edges: &MbEdges,
    bit_depth: i32,
    subsampling_shift: u32,
) -> Result<(), TfDepthMismatch> {
    // `ss_y` / `ss_x` are 0 here whatever the plane — C hardwires them.
    let (subpel_params, pos_y, pos_x) = compute_subpel_params(
        geom,
        pre_y,
        pre_x,
        mv,
        sf,
        blk_width as i32,
        blk_height as i32,
        edges,
        0,
        0,
    );
    let idx = src_origin as isize + pos_x as isize + pos_y as isize * src_stride as isize;
    let idx = usize::try_from(idx).expect("reference position before the plane");

    // The shift applies to the STRIDES and the HEIGHT, never the width.
    let src_stride = src_stride << subsampling_shift;
    let dst_stride = dst_stride << subsampling_shift;
    let blk_height = blk_height >> subsampling_shift;

    match (src, dst) {
        (TfSrc::Lbd(p), TfDst::Lbd(d)) => {
            inter_predictor(
                SrcView::new(p, idx, src_stride),
                d,
                dst_stride,
                conv_buf,
                &subpel_params,
                blk_width,
                blk_height,
                conv_params,
                interp_filters,
                false,
            );
            Ok(())
        }
        (TfSrc::Hbd(p), TfDst::Hbd(d)) => {
            highbd_inter_predictor(
                SrcView16::new(p, idx, src_stride),
                d,
                dst_stride,
                conv_buf,
                &subpel_params,
                blk_width,
                blk_height,
                conv_params,
                interp_filters,
                false,
                bit_depth,
            );
            Ok(())
        }
        _ => Err(TfDepthMismatch),
    }
}

/// `svt_aom_simple_luma_unipred`'s CONV_BUF stride (:2686-2703) —
/// `get_conv_params_no_round(0, tmp_dstY, 128, 0, bit_depth)`.
pub const SIMPLE_UNIPRED_CONV_STRIDE: usize = 128;

/// `svt_aom_simple_luma_unipred` (enc_inter_prediction.c:2677).
///
/// The whole body is one [`tf_inter_predictor`] call with the IDENTITY scale
/// factors, `is_compound = 0` and a private 128x128 CONV_BUF. `dst_origin_*`
/// index the prediction buffer; C shifts that offset by `is16bit` because it
/// is stepping a `uint8_t*`, which a typed slice does not need.
#[allow(clippy::too_many_arguments)]
pub fn simple_luma_unipred(
    src: TfSrc<'_>,
    src_stride: usize,
    dst: TfDst<'_>,
    dst_stride: usize,
    dst_origin_x: usize,
    dst_origin_y: usize,
    mv: Mv,
    interp_filters: InterpFilters,
    geom: RefGeometry,
    ref_width: i32,
    ref_height: i32,
    pu_origin_x: i32,
    pu_origin_y: i32,
    blk_width: usize,
    blk_height: usize,
    edges: &MbEdges,
    bit_depth: i32,
    subsampling_shift: u32,
) -> Result<(), TfDepthMismatch> {
    let mut conv_buf = alloc::vec![0u16; SIMPLE_UNIPRED_CONV_STRIDE * SIMPLE_UNIPRED_CONV_STRIDE];
    let conv_params = ConvolveParams::no_round(false, SIMPLE_UNIPRED_CONV_STRIDE, false, bit_depth);
    // `sf_identity` — the caller passes an identity ScaleFactors, so `is_scaled`
    // is false and `compute_subpel_params` takes its unscaled arm.
    let sf = ScaleFactors::setup_for_frame(ref_width, ref_height, ref_width, ref_height);
    let offset = dst_origin_x + dst_origin_y * dst_stride;
    let dst = match dst {
        TfDst::Lbd(d) => TfDst::Lbd(&mut d[offset..]),
        TfDst::Hbd(d) => TfDst::Hbd(&mut d[offset..]),
    };
    tf_inter_predictor(
        src,
        0,
        src_stride,
        dst,
        dst_stride,
        &mut conv_buf,
        pu_origin_y,
        pu_origin_x,
        mv,
        &sf,
        &conv_params,
        interp_filters,
        geom,
        blk_width,
        blk_height,
        edges,
        bit_depth,
        subsampling_shift,
    )
}
