//! The PD0 and light-PD1 inter-prediction drivers.
//!
//! Ported from `Source/Lib/Codec/enc_inter_prediction.c` (SVT-AV1 v4.2.0):
//! `enc_make_inter_predictor_pd0` (:2509), `av1_inter_prediction_pd0` (:2723),
//! `av1_inter_prediction_light_pd1` (:2781), and the scale-factor setup both
//! MD entry points do (`svt_aom_inter_pu_prediction_av1_pd0` :3718,
//! `svt_aom_inter_pu_prediction_av1_light_pd1` :3759).
//!
//! # Evidence
//!
//! TIER 4. Every function here takes a `SequenceControlSet`, a
//! `ModeDecisionContext`, `EbPictureBufferDesc` planes and a
//! `ModeDecisionCandidateBuffer`; a shim cannot synthesise those without
//! building most of the encoder. The port expresses them over plain plane
//! slices and a small geometry struct, and the control flow is hand-traced.
//!
//! Everything they call IS tier-1 gated:
//! [`crate::port_inter_predictor::inter_predictor_pd0`] and
//! `inter_predictor_light_pd1_8bit` (`c_parity_port_inter_predictor.rs`),
//! [`crate::port_subpel_params::compute_subpel_params`]
//! (`c_parity_port_subpel_params.rs`) and
//! [`crate::port_scale_factors::ScaleFactors::setup_for_frame`]
//! (`c_parity_port_scale_factors.rs`).
//!
//! # Three details a re-derivation gets wrong
//!
//! * PD0 computes `pos = ref_origin + (mv >> 3)` — MVs are EIGHTH-pel, so the
//!   shift is 3, not the 4 the `SUBPEL_BITS` domain uses elsewhere. And it
//!   only calls `compute_subpel_params` at all on the SCALED path: on the
//!   unscaled path the subpel params stay
//!   `{SCALE_SUBPEL_SHIFTS, SCALE_SUBPEL_SHIFTS, 0, 0}` and the MV's sub-pel
//!   part is DISCARDED. Light-PD1 calls it unconditionally.
//! * The CONV_BUF stride differs per path: PD0 uses
//!   `super_block_size == 128 ? 128 : 64`, light-PD1 luma uses a fixed 64, and
//!   light-PD1 chroma uses 32 with Cr's buffer starting at `tmp_dst_y[32*32]`
//!   — i.e. both chroma planes share one 64x64 scratch.
//! * On the second reference C sets `do_average = 1` AND
//!   `use_dist_wtd_comp_avg = 0` — PD0 and light-PD1 never distance-weight,
//!   whatever `svt_av1_dist_wtd_comp_weight_assign` derived.

use crate::port_convolve::ConvolveParams;
use crate::port_inter_predictor::{
    InterpFilters, inter_predictor_light_pd1_8bit, inter_predictor_pd0,
};
use crate::port_scale_factors::{SCALE_SUBPEL_SHIFTS, ScaleFactors, SubpelParams};
use crate::port_subpel_params::{MbEdges, Mv, RefGeometry, compute_subpel_params};
use alloc::vec;

/// `PICTURE_BUFFER_DESC_LUMA_MASK`.
pub const LUMA_MASK: u32 = 1;
/// `PICTURE_BUFFER_DESC_Cb_FLAG`.
pub const CB_FLAG: u32 = 2;
/// `PICTURE_BUFFER_DESC_Cr_FLAG`.
pub const CR_FLAG: u32 = 4;
/// `PICTURE_BUFFER_DESC_CHROMA_MASK`.
pub const CHROMA_MASK: u32 = CB_FLAG | CR_FLAG;

/// One reference plane, as the drivers read it.
pub struct RefPlane<'a> {
    /// The plane samples.
    pub buf: &'a [u8],
    /// Index of (0, 0) inside `buf`; the drivers index negative offsets from
    /// it, so the caller must supply the reference's padding margin.
    pub origin: usize,
    /// Row stride.
    pub stride: usize,
    /// `ref_pic->width` / `->height`, which `compute_subpel_params` clamps
    /// against on the scaled path.
    pub width: i32,
    /// See [`Self::width`].
    pub height: i32,
}

/// The block geometry the drivers read off `ctx->blk_geom` and `ctx`.
#[derive(Debug, Clone, Copy)]
pub struct BlkGeom {
    /// `ctx->blk_org_x`.
    pub org_x: i32,
    /// `ctx->blk_org_y`.
    pub org_y: i32,
    /// `blk_geom->bwidth`.
    pub bwidth: usize,
    /// `blk_geom->bheight`.
    pub bheight: usize,
    /// `blk_geom->bwidth_uv`.
    pub bwidth_uv: usize,
    /// `blk_geom->bheight_uv`.
    pub bheight_uv: usize,
    /// `scs->super_block_size`.
    pub super_block_size: i32,
}

/// `enc_make_inter_predictor_pd0` (enc_inter_prediction.c:2509) — a thin
/// wrapper whose only job is to reach `svt_inter_predictor_pd0`. Kept as its
/// own function because the C call graph has it, and because the PD0 driver's
/// one MC call site is easier to find with a name on it.
#[allow(clippy::too_many_arguments)]
pub fn enc_make_inter_predictor_pd0(
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    conv_buf: &mut [u16],
    blk_width: usize,
    blk_height: usize,
    subpel_params: &SubpelParams,
    conv_params: &ConvolveParams,
) {
    inter_predictor_pd0(
        crate::port_convolve::SrcView::new(src, src_origin, src_stride),
        dst,
        dst_stride,
        conv_buf,
        blk_width,
        blk_height,
        subpel_params,
        conv_params,
    );
}

/// `av1_inter_prediction_pd0` (enc_inter_prediction.c:2723) — luma only, one
/// pass per reference.
///
/// `mvs` is `block_mi->mv[ref_itr]`; supplying two makes it compound
/// (`has_second_ref`).
#[allow(clippy::too_many_arguments)]
pub fn av1_inter_prediction_pd0(
    geom: &BlkGeom,
    mvs: &[Mv],
    refs: &[RefPlane<'_>],
    sfs: &[ScaleFactors],
    edges: &MbEdges,
    dst: &mut [u8],
    dst_stride: usize,
) {
    let is_compound = mvs.len() > 1;
    let conv_buf_stride = if geom.super_block_size == 128 {
        128usize
    } else {
        64
    };
    let mut conv_buf = vec![0u16; conv_buf_stride * conv_buf_stride];
    let mut conv_params = ConvolveParams::no_round(false, conv_buf_stride, is_compound, 8);

    for (ref_itr, mv) in mvs.iter().enumerate() {
        let mut subpel_params = SubpelParams {
            xs: SCALE_SUBPEL_SHIFTS,
            ys: SCALE_SUBPEL_SHIFTS,
            subpel_x: 0,
            subpel_y: 0,
        };
        // Eighth-pel MV, so >> 3.
        let mut pos_x = geom.org_x + (mv.x as i32 >> 3);
        let mut pos_y = geom.org_y + (mv.y as i32 >> 3);

        let rp = &refs[ref_itr];
        let sf = &sfs[ref_itr];
        if sf.is_scaled() {
            let (sp, py, px) = compute_subpel_params(
                RefGeometry {
                    super_block_size: geom.super_block_size,
                    frame_width: rp.width,
                    frame_height: rp.height,
                },
                geom.org_y,
                geom.org_x,
                *mv,
                sf,
                geom.bwidth as i32,
                geom.bheight as i32,
                edges,
                0,
                0,
            );
            subpel_params = sp;
            pos_y = py;
            pos_x = px;
        }

        if ref_itr != 0 {
            conv_params.do_average = true;
            conv_params.use_jnt_comp_avg = false;
        }

        let src_origin =
            (rp.origin as isize + pos_x as isize + pos_y as isize * rp.stride as isize) as usize;
        enc_make_inter_predictor_pd0(
            rp.buf,
            src_origin,
            rp.stride,
            dst,
            dst_stride,
            &mut conv_buf,
            geom.bwidth,
            geom.bheight,
            &subpel_params,
            &conv_params,
        );
    }
}

/// The destination planes light-PD1 writes.
pub struct PredPlanes<'a> {
    /// Y.
    pub y: &'a mut [u8],
    /// Y stride.
    pub y_stride: usize,
    /// Cb.
    pub u: &'a mut [u8],
    /// Cb stride.
    pub u_stride: usize,
    /// Cr.
    pub v: &'a mut [u8],
    /// Cr stride.
    pub v_stride: usize,
}

/// `av1_inter_prediction_light_pd1` (enc_inter_prediction.c:2781), 8-bit.
///
/// `component_mask` splits luma from chroma: MDS0 asks for luma only and MDS3
/// for chroma only, which is what makes the two passes independent.
///
/// Chroma uses `ref_origin / 2` and `ss_x = ss_y = 1`, and BOTH chroma planes
/// share one 64x64 scratch — Cb at offset 0, Cr at 32*32 — with a CONV_BUF
/// stride of 32.
#[allow(clippy::too_many_arguments)]
pub fn av1_inter_prediction_light_pd1(
    geom: &BlkGeom,
    mvs: &[Mv],
    y_refs: &[RefPlane<'_>],
    u_refs: &[RefPlane<'_>],
    v_refs: &[RefPlane<'_>],
    sfs: &[ScaleFactors],
    edges: &MbEdges,
    interp_filters: InterpFilters,
    pred: &mut PredPlanes<'_>,
    component_mask: u32,
) {
    let is_compound = mvs.len() > 1;

    if component_mask & LUMA_MASK != 0 {
        let mut conv_buf = vec![0u16; 64 * 64];
        let mut cp = ConvolveParams::no_round(false, 64, is_compound, 8);
        for (i, mv) in mvs.iter().enumerate() {
            let rp = &y_refs[i];
            let (sp, pos_y, pos_x) = compute_subpel_params(
                RefGeometry {
                    super_block_size: geom.super_block_size,
                    frame_width: rp.width,
                    frame_height: rp.height,
                },
                geom.org_y,
                geom.org_x,
                *mv,
                &sfs[i],
                geom.bwidth as i32,
                geom.bheight as i32,
                edges,
                0,
                0,
            );
            if i != 0 {
                cp.do_average = true;
                cp.use_jnt_comp_avg = false;
            }
            let origin = (rp.origin as isize + pos_x as isize + pos_y as isize * rp.stride as isize)
                as usize;
            inter_predictor_light_pd1_8bit(
                crate::port_convolve::SrcView::new(rp.buf, origin, rp.stride),
                pred.y,
                pred.y_stride,
                &mut conv_buf,
                geom.bwidth,
                geom.bheight,
                interp_filters,
                &sp,
                &cp,
            );
        }
    }

    if component_mask & CHROMA_MASK != 0 {
        // One 64x64 scratch shared by both chroma planes: Cb at 0, Cr at 32*32.
        let mut conv_buf_cb = vec![0u16; 32 * 32];
        let mut conv_buf_cr = vec![0u16; 32 * 32];
        let mut cp_cb = ConvolveParams::no_round(false, 32, is_compound, 8);
        let mut cp_cr = ConvolveParams::no_round(false, 32, is_compound, 8);
        let org_y_c = geom.org_y / 2;
        let org_x_c = geom.org_x / 2;

        for (i, mv) in mvs.iter().enumerate() {
            if i != 0 {
                cp_cb.do_average = true;
                cp_cr.do_average = true;
                cp_cb.use_jnt_comp_avg = false;
                cp_cr.use_jnt_comp_avg = false;
            }
            // NOTE: C passes the LUMA bwidth/bheight to compute_subpel_params
            // here, not bwidth_uv — the clamp is against the luma block, and
            // only the ss flags and the halved origin make it chroma.
            let rp0 = &u_refs[i];
            let (sp, pos_y, pos_x) = compute_subpel_params(
                RefGeometry {
                    super_block_size: geom.super_block_size,
                    frame_width: rp0.width,
                    frame_height: rp0.height,
                },
                org_y_c,
                org_x_c,
                *mv,
                &sfs[i],
                geom.bwidth as i32,
                geom.bheight as i32,
                edges,
                1,
                1,
            );
            if component_mask & CB_FLAG != 0 {
                let rp = &u_refs[i];
                let origin = (rp.origin as isize
                    + pos_x as isize
                    + pos_y as isize * rp.stride as isize) as usize;
                inter_predictor_light_pd1_8bit(
                    crate::port_convolve::SrcView::new(rp.buf, origin, rp.stride),
                    pred.u,
                    pred.u_stride,
                    &mut conv_buf_cb,
                    geom.bwidth_uv,
                    geom.bheight_uv,
                    interp_filters,
                    &sp,
                    &cp_cb,
                );
            }
            if component_mask & CR_FLAG != 0 {
                let rp = &v_refs[i];
                let origin = (rp.origin as isize
                    + pos_x as isize
                    + pos_y as isize * rp.stride as isize) as usize;
                inter_predictor_light_pd1_8bit(
                    crate::port_convolve::SrcView::new(rp.buf, origin, rp.stride),
                    pred.v,
                    pred.v_stride,
                    &mut conv_buf_cr,
                    geom.bwidth_uv,
                    geom.bheight_uv,
                    interp_filters,
                    &sp,
                    &cp_cr,
                );
            }
        }
    }
}

/// The scale-factor setup both MD entry points do
/// (`svt_aom_inter_pu_prediction_av1_pd0` :3722,
/// `svt_aom_inter_pu_prediction_av1_light_pd1` :3763).
///
/// TRAP: when `pcs->ppcs->is_not_scaled` is TRUE the factors are left at
/// `scs->sf_identity` and `svt_av1_setup_scale_factors_for_frame` is NOT
/// called — so a frame flagged unscaled never takes the scaled MC path even if
/// the reference's dimensions differ. Reproducing the derivation
/// unconditionally would change which kernel runs.
pub fn setup_ref_scale_factors(
    is_not_scaled: bool,
    ref_present: bool,
    ref_w: i32,
    ref_h: i32,
    cur_w: i32,
    cur_h: i32,
    sf_identity: ScaleFactors,
) -> ScaleFactors {
    if is_not_scaled || !ref_present {
        return sf_identity;
    }
    ScaleFactors::setup_for_frame(ref_w, ref_h, cur_w, cur_h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    /// The `is_not_scaled` short circuit: identity factors even when the
    /// reference is a different size.
    #[test]
    fn is_not_scaled_short_circuits() {
        let identity = ScaleFactors::setup_for_frame(64, 64, 64, 64);
        let got = setup_ref_scale_factors(true, true, 128, 128, 64, 64, identity);
        assert_eq!(got, identity);
        assert!(!got.is_scaled());
        // And without the flag, the same sizes DO produce a scaled factor.
        let got = setup_ref_scale_factors(false, true, 128, 128, 64, 64, identity);
        assert!(got.is_scaled());
        // A missing reference also keeps the identity.
        let got = setup_ref_scale_factors(false, false, 128, 128, 64, 64, identity);
        assert_eq!(got, identity);
    }

    /// PD0's unscaled path DISCARDS the MV's sub-pel part: only `mv >> 3`
    /// reaches the source pointer, and the subpel params stay whole-pel.
    #[test]
    fn pd0_discards_the_subpel_mv() {
        let geom = BlkGeom {
            org_x: 8,
            org_y: 8,
            bwidth: 8,
            bheight: 8,
            bwidth_uv: 4,
            bheight_uv: 4,
            super_block_size: 64,
        };
        let stride = 64usize;
        let src: Vec<u8> = (0..stride * 64).map(|i| (i % 251) as u8).collect();
        let sf = ScaleFactors::setup_for_frame(64, 64, 64, 64);
        let edges = MbEdges {
            to_left: 0,
            to_right: 0,
            to_top: 0,
            to_bottom: 0,
        };
        let refs = alloc::vec![RefPlane {
            buf: &src,
            origin: 16 * stride + 16,
            stride,
            width: 64,
            height: 64,
        }];
        // (8, 8) and (15, 15) eighth-pel both floor to the same whole pel.
        let mut a = alloc::vec![0u8; 64];
        let mut b = alloc::vec![0u8; 64];
        av1_inter_prediction_pd0(&geom, &[Mv { x: 8, y: 8 }], &refs, &[sf], &edges, &mut a, 8);
        av1_inter_prediction_pd0(
            &geom,
            &[Mv { x: 15, y: 15 }],
            &refs,
            &[sf],
            &edges,
            &mut b,
            8,
        );
        assert_eq!(a, b, "PD0 must ignore the sub-pel part of the MV");
        // And a whole pel apart really does move.
        let mut c = alloc::vec![0u8; 64];
        av1_inter_prediction_pd0(
            &geom,
            &[Mv { x: 16, y: 16 }],
            &refs,
            &[sf],
            &edges,
            &mut c,
            8,
        );
        assert_ne!(a, c);
    }
}
