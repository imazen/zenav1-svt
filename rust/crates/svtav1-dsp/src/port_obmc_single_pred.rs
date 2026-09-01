//! The OBMC neighbour's own prediction, EXECUTABLE.
//!
//! Ported from `Source/Lib/Codec/enc_inter_prediction.c` (SVT-AV1 v4.2.0):
//! `get_single_prediction_for_obmc_luma` (:958),
//! `get_single_prediction_for_obmc_chroma` (:1018) and their `_hbd` twins
//! (:791, :853).
//!
//! # What already existed
//!
//! [`crate::port_obmc_build`] ports these four functions' GEOMETRY — which
//! extent each plane covers, where it lands, the buffer offsets — and says so.
//! What it does not do is RUN one. Each of these four IS a single
//! [`crate::port_enc_make_pred::enc_make_inter_predictor`] call (the chroma
//! pair is two, U then V) around a fixed parameter set, and that call only
//! became executable in this lane, so this module could not exist before it.
//!
//! # The fixed parameter set, which is where the four differ
//!
//! | | luma | chroma |
//! |---|---|---|
//! | `conv_params` CONV_BUF stride | `scs->sb_size` | `scs->sb_size >> ss_x` |
//! | `plane` | 0 | 1 then 2 |
//! | `ss_y` / `ss_x` passed on | 0 / 0 | the caller's |
//! | block position | `pu_origin_{y,x}` | `ROUND_UV(pu_origin_{y,x}) >> ss_{y,x}` |
//!
//! Note the chroma CONV_BUF stride is `>> ss_x` for BOTH planes — never
//! `>> ss_y` — and that the two chroma planes share ONE `obmc_conv_buf`, so V
//! overwrites U's intermediate. Both are C's, reproduced.
//!
//! **The CONV_BUF stride is INERT on this path, and the differential says so
//! rather than pretending otherwise.** All four functions pass
//! `is_compound = 0`, and the single-prediction kernels never touch
//! `conv_params->dst`. MEASURED: changing `>> ss_x` to `>> ss_y` leaves every
//! cell green. The value is reproduced for faithfulness — an OBMC caller that
//! ever set `is_compound` would need it — not because a test can see it.
//!
//! The 8-bit pair hardwires `bit_depth = EB_EIGHT_BIT` and `is16bit = false`;
//! the `_hbd` pair takes `bit_depth` and passes SVT's SPLIT reference
//! (`y_buffer` + `y_buffer_bit_inc`) with `is16bit = true`. Every one of the
//! four passes `interinter_comp = NULL`, `use_intrabc = 0`,
//! `is_masked_compound = 0`, `is_wm = false` — so this is always the plain
//! regular leaf.
//!
//! # Evidence
//!
//! The four C functions are `static`, so there is no symbol to bind. What is
//! gated is what they CONTAIN: `tests/c_parity_port_obmc_single_pred.rs`
//! drives the real exported `svt_aom_enc_make_inter_predictor` with exactly
//! the parameter set each one builds, which is the whole body. TIER 1 for the
//! call, TIER 4 for the four-line wrapper around it.

use crate::port_convolve::ConvolveParams;
use crate::port_enc_make_pred::{DstPlane, MakePredError, SrcPlanes, enc_make_inter_predictor};
use crate::port_inter_predictor::InterpFilters;
use crate::port_obmc_build::round_uv;
use crate::port_scale_factors::ScaleFactors;
use crate::port_subpel_params::{MbEdges, Mv, RefGeometry};

/// One plane of the reference picture and of the prediction buffer.
pub struct ObmcPlaneIo<'a> {
    /// The reference plane's samples.
    pub reference: SrcPlanes<'a>,
    /// `ref_pic->{y,u,v}_stride`.
    pub src_stride: usize,
    /// The prediction buffer.
    pub prediction: DstPlane<'a>,
    /// `prediction_ptr->{y,u,v}_stride`.
    pub dst_stride: usize,
}

/// The picture-level sizes the four functions read.
#[derive(Debug, Clone, Copy)]
pub struct ObmcPicDims {
    /// `ref_pic_list0->width` / `->height`.
    pub reference: (i32, i32),
    /// `prediction_ptr->width` / `->height`.
    pub prediction: (i32, i32),
    /// `scs->sb_size`.
    pub sb_size: usize,
}

/// `get_single_prediction_for_obmc_luma` (:958) and its `_hbd` twin (:791).
///
/// The two are one function here: they differ only in `bit_depth` and in
/// whether the reference arrives as one plane or SVT's split pair, and
/// [`SrcPlanes`] already carries that distinction.
#[allow(clippy::too_many_arguments)]
pub fn get_single_prediction_for_obmc_luma(
    io: ObmcPlaneIo<'_>,
    src_origin: usize,
    dims: ObmcPicDims,
    interp_filters: InterpFilters,
    mv: Mv,
    pu_origin_x: u32,
    pu_origin_y: u32,
    dst_origin_x: u32,
    dst_origin_y: u32,
    bwidth: usize,
    bheight: usize,
    edges: &MbEdges,
    obmc_conv_buf: &mut [u16],
    bit_depth: i32,
) -> Result<(), MakePredError> {
    let conv_params = ConvolveParams::no_round(false, dims.sb_size, false, bit_depth);
    let sf = ScaleFactors::setup_for_frame(
        dims.reference.0,
        dims.reference.1,
        dims.prediction.0,
        dims.prediction.1,
    );
    let ObmcPlaneIo {
        reference,
        src_stride,
        prediction,
        dst_stride,
    } = io;
    let offset = dst_origin_x as usize + dst_origin_y as usize * dst_stride;
    let dst = shift_dst(prediction, offset);
    enc_make_inter_predictor(
        reference,
        src_origin,
        src_stride,
        dst,
        dst_stride,
        obmc_conv_buf,
        pu_origin_y as i32,
        pu_origin_x as i32,
        mv,
        &sf,
        &conv_params,
        interp_filters,
        None,
        None,
        RefGeometry {
            super_block_size: dims.sb_size as i32,
            frame_width: dims.reference.0,
            frame_height: dims.reference.1,
        },
        bwidth,
        bheight,
        edges,
        0,
        0,
        0,
        bit_depth,
        false,
        false,
    )
}

/// `get_single_prediction_for_obmc_chroma` (:1018) and its `_hbd` twin (:853),
/// for ONE chroma plane.
///
/// C's function does U then V in one body; splitting it per plane is the same
/// two calls, and it makes the `plane` argument (1 then 2) explicit rather
/// than duplicated. The caller passes the SAME `obmc_conv_buf` for both, which
/// is what C does — V overwrites U's intermediate, and nothing reads it back.
#[allow(clippy::too_many_arguments)]
pub fn get_single_prediction_for_obmc_chroma_plane(
    io: ObmcPlaneIo<'_>,
    src_origin: usize,
    dims: ObmcPicDims,
    plane: usize,
    interp_filters: InterpFilters,
    mv: Mv,
    pu_origin_x: u32,
    pu_origin_y: u32,
    dst_origin_x: u32,
    dst_origin_y: u32,
    bwidth: usize,
    bheight: usize,
    edges: &MbEdges,
    ss_x: i32,
    ss_y: i32,
    obmc_conv_buf: &mut [u16],
    bit_depth: i32,
) -> Result<(), MakePredError> {
    debug_assert!(plane == 1 || plane == 2, "chroma planes are 1 and 2");
    // `>> ss_x` for BOTH chroma planes — never `>> ss_y`.
    let conv_params = ConvolveParams::no_round(false, dims.sb_size >> ss_x, false, bit_depth);
    let sf = ScaleFactors::setup_for_frame(
        dims.reference.0,
        dims.reference.1,
        dims.prediction.0,
        dims.prediction.1,
    );
    let pu_origin_y_chroma = (round_uv(pu_origin_y) >> ss_y) as i32;
    let pu_origin_x_chroma = (round_uv(pu_origin_x) >> ss_x) as i32;
    let ObmcPlaneIo {
        reference,
        src_stride,
        prediction,
        dst_stride,
    } = io;
    let offset = (round_uv(dst_origin_x) >> ss_x) as usize
        + (round_uv(dst_origin_y) >> ss_y) as usize * dst_stride;
    let dst = shift_dst(prediction, offset);
    enc_make_inter_predictor(
        reference,
        src_origin,
        src_stride,
        dst,
        dst_stride,
        obmc_conv_buf,
        pu_origin_y_chroma,
        pu_origin_x_chroma,
        mv,
        &sf,
        &conv_params,
        interp_filters,
        None,
        None,
        RefGeometry {
            super_block_size: dims.sb_size as i32,
            frame_width: dims.reference.0,
            frame_height: dims.reference.1,
        },
        bwidth,
        bheight,
        edges,
        plane,
        ss_y,
        ss_x,
        bit_depth,
        false,
        false,
    )
}

/// C reaches the block's corner with pointer arithmetic on the plane base;
/// the port slices instead. Both depths offset in SAMPLES — the `<< is16bit`
/// C writes in the 8-bit luma case (:986) is a BYTE step over `uint8_t`
/// samples, and the `_hbd` twin casts to `uint16_t*` BEFORE adding, so the
/// two are the same sample count.
fn shift_dst(dst: DstPlane<'_>, offset: usize) -> DstPlane<'_> {
    match dst {
        DstPlane::Lbd(d) => DstPlane::Lbd(&mut d[offset..]),
        DstPlane::Hbd(d) => DstPlane::Hbd(&mut d[offset..]),
    }
}
