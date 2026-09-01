//! One OBMC neighbour's prediction, end to end.
//!
//! Ported from `Source/Lib/Codec/enc_inter_prediction.c` (SVT-AV1 v4.2.0):
//! `build_prediction_by_above_pred` (:1120) and
//! `build_prediction_by_left_pred` (:1228).
//!
//! # What already existed, and what this joins up
//!
//! [`crate::port_obmc_build`] ports these two as GEOMETRY —
//! `build_prediction_by_{above,left}_pred_geom` yield the per-plane extents,
//! destination corners and source positions, including the
//! `svt_av1_skip_u4x4_pred_in_obmc` skip. [`crate::port_obmc_single_pred`]
//! ports the four leaves they call. Neither side ran the other. This module
//! is the join: geometry in, prediction pixels out.
//!
//! # The two are NOT mirror images, and the asymmetry is the whole point
//!
//! * ABOVE halves the HEIGHT (`clamp(bh >> (ssy+1), 4, 64 >> (ssy+1))`) and
//!   takes the neighbour's full width; the destination corner walks along X
//!   (`rel_mi_col << MI_SIZE_LOG2`, `y = 0`).
//! * LEFT halves the WIDTH and takes the neighbour's full height; the corner
//!   walks along Y (`x = 0`, `rel_mi_row << MI_SIZE_LOG2`).
//! * They pass DIFFERENT `dir` to `svt_av1_skip_u4x4_pred_in_obmc` — 0 above,
//!   1 left — and with `DISABLE_CHROMA_U8X8_OBMC == 0` that predicate is
//!   `dir == 0`, so the ABOVE walk can skip a chroma plane the LEFT walk never
//!   skips.
//!
//! # `tmp_width` / `tmp_height` are the FRAME's, not the scratch's
//!
//! `build_prediction_by_{above,left}_preds` fill
//! `ctxt.tmp_{width,height}` from `pcs->ppcs->enhanced_pic->{width,height}`
//! (:1355-1356), and those become `prediction_ptr->{width,height}` — which is
//! what the leaves hand `svt_av1_setup_scale_factors_for_frame` as the
//! DESTINATION size. So the scale factors compare the reference picture
//! against the FRAME, never against the small OBMC scratch. Reading the field
//! name as "the temp buffer's size" gives scale factors that are wrong
//! whenever the reference is scaled.
//!
//! # Evidence
//!
//! Both C functions are `static`, so there is no symbol to bind. TIER 1 for
//! what they contain — every leaf is
//! [`crate::port_obmc_single_pred`], gated against the real exported
//! `svt_aom_enc_make_inter_predictor` — and TIER 4 for the plane walk, which
//! `tests/c_parity_port_obmc_nb_pred.rs` drives against C one plane at a time
//! with the geometry the port derived.

use crate::port_enc_make_pred::{DstPlane, MakePredError, SrcPlanes};
use crate::port_inter_predictor::InterpFilters;
use crate::port_obmc_build::{
    NbPredGeom, build_prediction_by_above_pred_geom, build_prediction_by_left_pred_geom,
};
use crate::port_obmc_single_pred::{
    ObmcPicDims, ObmcPlaneIo, get_single_prediction_for_obmc_chroma_plane,
    get_single_prediction_for_obmc_luma,
};
use crate::port_subpel_params::{MbEdges, Mv};
use svtav1_types::block::BlockSize;

/// The reference picture's three planes plus its size.
pub struct ObmcRefPic<'a> {
    /// Luma.
    pub y: SrcPlanes<'a>,
    /// Cb.
    pub u: SrcPlanes<'a>,
    /// Cr.
    pub v: SrcPlanes<'a>,
    /// `ref_pic->{y,u,v}_stride`.
    pub stride: [usize; 3],
    /// `ref_pic->width` / `->height`.
    pub dims: (i32, i32),
}

/// The OBMC scratch the neighbour predictions are written into
/// (`ctxt->tmp_buf` / `ctxt->tmp_stride`).
pub struct ObmcScratch<'a> {
    /// Luma.
    pub y: DstPlane<'a>,
    /// Cb.
    pub u: DstPlane<'a>,
    /// Cr.
    pub v: DstPlane<'a>,
    /// `ctxt->tmp_stride`.
    pub stride: [usize; 3],
}

/// Which walk a call belongs to. It selects the geometry AND the `dir`
/// argument of `svt_av1_skip_u4x4_pred_in_obmc`, which are not the same thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NbSide {
    /// `build_prediction_by_above_pred` — `dir = 0`.
    Above,
    /// `build_prediction_by_left_pred` — `dir = 1`.
    Left,
}

/// Everything one neighbour contributes: its motion and its filters.
#[derive(Debug, Clone, Copy)]
pub struct Neighbour {
    /// `ctxt->mv`, set by `av1_setup_build_prediction_by_*_pred`.
    pub mv: Mv,
    /// `above_mbmi->block_mi.interp_filters`.
    pub interp_filters: InterpFilters,
    /// `above_mi_width` (ABOVE) or `left_mi_height` (LEFT), in MI units.
    pub extent_mi: usize,
    /// `rel_mi_col` (ABOVE) or `rel_mi_row` (LEFT).
    pub rel_mi: usize,
}

/// `build_prediction_by_above_pred` (:1120) / `build_prediction_by_left_pred`
/// (:1228) — one neighbour, every plane the component mask selects.
///
/// `frame_dims` is `(enhanced_pic->width, enhanced_pic->height)`; see the
/// module doc for why it is the FRAME's size and not the scratch's.
#[allow(clippy::too_many_arguments)]
pub fn build_prediction_by_nb_pred(
    side: NbSide,
    reference: ObmcRefPic<'_>,
    src_origin: [usize; 3],
    scratch: ObmcScratch<'_>,
    frame_dims: (i32, i32),
    sb_size: usize,
    bsize: BlockSize,
    mi_row: i32,
    mi_col: i32,
    nb: Neighbour,
    edges: &MbEdges,
    ss_x: i32,
    ss_y: i32,
    component_mask: u32,
    obmc_conv_buf: &mut [u16],
    bit_depth: i32,
) -> Result<(), MakePredError> {
    let geoms = match side {
        NbSide::Above => build_prediction_by_above_pred_geom(
            bsize,
            mi_row,
            mi_col,
            nb.rel_mi,
            nb.extent_mi,
            ss_x as usize,
            ss_y as usize,
            component_mask,
        ),
        NbSide::Left => build_prediction_by_left_pred_geom(
            bsize,
            mi_row,
            mi_col,
            nb.rel_mi,
            nb.extent_mi,
            ss_x as usize,
            ss_y as usize,
            component_mask,
        ),
    };
    let ObmcRefPic {
        y,
        u,
        v,
        stride: src_stride,
        dims,
    } = reference;
    let ObmcScratch {
        y: mut dy,
        u: mut du,
        v: mut dv,
        stride: dst_stride,
    } = scratch;
    let pic = ObmcPicDims {
        reference: dims,
        prediction: frame_dims,
        sb_size,
    };

    for g in geoms {
        // C's plane loop index is 0 or 1; index 1 predicts BOTH chroma planes
        // through one call, which is two calls here.
        if g.plane == 0 {
            get_single_prediction_for_obmc_luma(
                ObmcPlaneIo {
                    reference: y,
                    src_stride: src_stride[0],
                    prediction: take(&mut dy),
                    dst_stride: dst_stride[0],
                },
                src_origin[0],
                pic,
                nb.interp_filters,
                nb.mv,
                g.mi_x as u32,
                g.mi_y as u32,
                g.dst_origin_x as u32,
                g.dst_origin_y as u32,
                g.bw,
                g.bh,
                edges,
                obmc_conv_buf,
                bit_depth,
            )?;
        } else {
            for (plane, src, dst, si) in
                [(1usize, u, take(&mut du), 1usize), (2, v, take(&mut dv), 2)]
            {
                get_single_prediction_for_obmc_chroma_plane(
                    ObmcPlaneIo {
                        reference: src,
                        src_stride: src_stride[si],
                        prediction: dst,
                        dst_stride: dst_stride[si],
                    },
                    src_origin[si],
                    pic,
                    plane,
                    nb.interp_filters,
                    nb.mv,
                    g.mi_x as u32,
                    g.mi_y as u32,
                    g.dst_origin_x as u32,
                    g.dst_origin_y as u32,
                    g.bw,
                    g.bh,
                    edges,
                    ss_x,
                    ss_y,
                    obmc_conv_buf,
                    bit_depth,
                )?;
            }
        }
    }
    Ok(())
}

/// Move a `DstPlane` out of a binding, leaving an empty one behind.
///
/// The plane walk consumes each destination exactly once (luma on `plane == 0`,
/// the chroma pair on `plane == 1`), so a re-entry would be a bug — and this
/// makes it one that panics on an empty slice rather than one that silently
/// predicts twice.
fn take<'a>(slot: &mut DstPlane<'a>) -> DstPlane<'a> {
    core::mem::replace(slot, DstPlane::Lbd(&mut []))
}

/// The per-plane geometry the walk derived, for a caller (or a test) that
/// wants to see it without running the prediction.
pub fn nb_pred_geometry(
    side: NbSide,
    bsize: BlockSize,
    mi_row: i32,
    mi_col: i32,
    nb: Neighbour,
    ss_x: usize,
    ss_y: usize,
    component_mask: u32,
) -> alloc::vec::Vec<NbPredGeom> {
    match side {
        NbSide::Above => build_prediction_by_above_pred_geom(
            bsize,
            mi_row,
            mi_col,
            nb.rel_mi,
            nb.extent_mi,
            ss_x,
            ss_y,
            component_mask,
        ),
        NbSide::Left => build_prediction_by_left_pred_geom(
            bsize,
            mi_row,
            mi_col,
            nb.rel_mi,
            nb.extent_mi,
            ss_x,
            ss_y,
            component_mask,
        ),
    }
}
