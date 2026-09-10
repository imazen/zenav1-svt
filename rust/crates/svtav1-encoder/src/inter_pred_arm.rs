//! The encoder's entry into C's INTER RECONSTRUCTION prediction.
//!
//! `svtav1_dsp::port_pd_pred::av1_inter_prediction_light_pd1`
//! (`enc_inter_prediction.c:2781`) and the convolve family under it
//! (`port_convolve`, tier-1 gated in `c_parity_port_convolve.rs`) have been
//! ported and gated for some time and **nothing in the encoder called them**.
//! This module is the adapter: it turns the pipeline's geometry — a block
//! origin, a block size, a padded reference from the DPB and an eighth-pel MV
//! — into the `BlkGeom` / `RefPlane` / `MbEdges` / `ScaleFactors` set the
//! driver takes.
//!
//! # Why it is its own module and not a call inside the search
//!
//! `docs/INTER-ENCODE-PLAN.md` §1s item 1 says both C-exact mode-decision
//! paths are switched off on any frame with a reference, and that work landed
//! in the pre-campaign recursion is churn the moment those gates come off.
//! The ADAPTER is not churn — every MD path needs exactly this conversion —
//! so it lives here, and only its CALL SITE is in code item 1 will bypass.
//!
//! # What is here and what is not
//!
//! [`predict_inter_luma`] is the luma half (§1s item 5) and
//! [`predict_inter_yuv`] the luma + chroma one (item 6): C's driver does all
//! three planes in ONE call under a `component_mask`, so the chroma half is
//! not a second prediction — it is the same `compute_subpel_params` result at
//! a halved origin, which is exactly why doing it separately would be a
//! different arithmetic. Both take the same geometry; the caller picks by
//! whether it has the two chroma `PaddedPlane`s from
//! [`crate::picture::PaddedRef::uv`].
//!
//! # Compound, warped and OBMC
//!
//! `av1_inter_prediction_light_pd1` takes an `mvs` SLICE and averages when it
//! has two — but no candidate this port injects is compound, so this adapter
//! takes ONE mv and says so rather than accepting a slice it cannot fill.
//! Warped motion and OBMC are different C entry points
//! (`enc_make_inter_predictor`'s `warp` arm, `svtav1_dsp::obmc`), both ported
//! and both unwired; a candidate that sets `motion_mode` must route to them
//! instead of here.

use crate::picture::PaddedPlane;
use svtav1_dsp::port_pd_pred::{
    BlkGeom, CHROMA_MASK, LUMA_MASK, PredPlanes, PredPlanes16, RefPlane, RefPlane16,
    av1_inter_prediction_light_pd1, av1_inter_prediction_light_pd1_hbd,
};
use svtav1_dsp::port_scale_factors::ScaleFactors;
use svtav1_dsp::port_subpel_params::{MbEdges, Mv as DspMv, RefGeometry};
use svtav1_types::motion::Mv;

/// One inter-predicted LUMA block, exactly as C reconstructs it.
///
/// `org_x` / `org_y` are the block's FRAME origin in luma pixels, `bw` / `bh`
/// its dims, `mv` its eighth-pel motion vector, and `interp_filters` C's
/// packed `(y) | (x << 16)` pair (0 = `EIGHTTAP_REGULAR` in both directions).
/// `frame_w` / `frame_h` are the CODED frame dims, which
/// `compute_subpel_params` clamps the MV against.
///
/// The reference must carry C's replicated margin
/// ([`crate::picture::REF_BORDER`]); a legal MV reads outside the frame and
/// the samples there are the replicated edge, not a constant.
#[allow(clippy::too_many_arguments)]
pub fn predict_inter_luma(
    reference: &PaddedPlane,
    org_x: usize,
    org_y: usize,
    bw: usize,
    bh: usize,
    mv: Mv,
    interp_filters: u32,
    sb_size: usize,
    frame_w: usize,
    frame_h: usize,
    out: &mut [u8],
    out_stride: usize,
) {
    // C `svt_aom_setup_scale_factors_for_frame` with equal dims — the
    // reference is never resampled on this port's path (superres refuses an
    // inter frame), so the factors are the identity and `is_scaled()` is
    // false, which is what keeps `compute_subpel_params` on its unscaled arm.
    let sf = ScaleFactors::setup_for_frame(
        frame_w as i32,
        frame_h as i32,
        frame_w as i32,
        frame_h as i32,
    );
    let edges = mb_edges(org_x, org_y, bw, bh, frame_w, frame_h);
    let rp = RefPlane {
        buf: &reference.buf,
        origin: reference.origin,
        stride: reference.stride,
        width: reference.width as i32,
        height: reference.height as i32,
    };
    // The chroma planes are unread under `LUMA_ONLY`, but the driver takes
    // them by slice, so hand it the luma plane rather than an empty one — an
    // empty slice would panic on the index if the mask were ever widened
    // here, which is a silent trap for whoever wires item 6.
    let mut u_scratch = [0u8; 1];
    let mut v_scratch = [0u8; 1];
    let mut pred = PredPlanes {
        y: out,
        y_stride: out_stride,
        u: &mut u_scratch,
        u_stride: 1,
        v: &mut v_scratch,
        v_stride: 1,
    };
    av1_inter_prediction_light_pd1(
        &BlkGeom {
            org_x: org_x as i32,
            org_y: org_y as i32,
            bwidth: bw,
            bheight: bh,
            // C `blk_geom->bwidth_uv` is `MAX(4, bwidth >> 1)` (utility.c:274),
            // which differs from this only below 8. A sub-8 block never
            // reaches here: its chroma covers the parent 8x8 and is stitched
            // from the covered cells' own MVs, which is a different C function
            // (`inter_chroma_4xn_pred`) — see
            // `inter_md_arm::predict_inter_chroma_sub8`.
            bwidth_uv: bw / 2,
            bheight_uv: bh / 2,
            super_block_size: sb_size as i32,
        },
        // `svtav1_dsp` carries its own two-field `Mv` (the DSP crate does
        // not depend on the encoder's types); the components are identical
        // eighth-pel `i16`s, so this is a re-spelling, not a conversion.
        &[DspMv { x: mv.x, y: mv.y }],
        &[rp],
        &[],
        &[],
        &[sf],
        &edges,
        interp_filters,
        &mut pred,
        LUMA_MASK,
    );
}

/// The PD0 luma prediction — C `svt_aom_inter_pu_prediction_av1_pd0`
/// (`enc_inter_prediction.c:3718`) -> `av1_inter_prediction_pd0` (`:2723`).
///
/// It is NOT [`predict_inter_luma`] with a different name. PD0's driver is a
/// SEPARATE C entry point with a materially smaller body: no `component_mask`
/// (luma only, always), no interpolation filter (it reaches
/// `svt_inter_predictor_pd0`, whose kernel is the PD0 one), and — the part
/// that matters for byte parity — **it does not call `compute_subpel_params`
/// on the unscaled path at all**. `pos_x` / `pos_y` are
/// `blk_org + (mv >> 3)` straight, with NO clamp against the frame edges, so
/// a legal MV reads into the reference's replicated margin and the caller
/// must supply one ([`crate::picture::PaddedRef`]).
///
/// The `MbEdges` argument therefore exists only for the SCALED branch, which
/// this port never takes (superres refuses an inter frame, so the scale
/// factors are the identity and `is_scaled()` is false). It is built the same
/// way anyway rather than faked, so that wiring a scaled reference later is a
/// change of one caller and not of this function's contract.
#[allow(clippy::too_many_arguments)]
pub fn predict_inter_luma_pd0(
    reference: &PaddedPlane,
    org_x: usize,
    org_y: usize,
    bw: usize,
    bh: usize,
    mv: Mv,
    sb_size: usize,
    frame_w: usize,
    frame_h: usize,
    out: &mut [u8],
    out_stride: usize,
) {
    let sf = ScaleFactors::setup_for_frame(
        frame_w as i32,
        frame_h as i32,
        frame_w as i32,
        frame_h as i32,
    );
    let edges = mb_edges(org_x, org_y, bw, bh, frame_w, frame_h);
    let rp = RefPlane {
        buf: &reference.buf,
        origin: reference.origin,
        stride: reference.stride,
        width: reference.width as i32,
        height: reference.height as i32,
    };
    svtav1_dsp::port_pd_pred::av1_inter_prediction_pd0(
        &BlkGeom {
            org_x: org_x as i32,
            org_y: org_y as i32,
            bwidth: bw,
            bheight: bh,
            // PD0 is luma-only; C `av1_inter_prediction_pd0` never reads the
            // chroma dims. They are filled with the real 4:2:0 values rather
            // than zeros so a future chroma arm cannot inherit a lie.
            // C `blk_geom->bwidth_uv` is `MAX(4, bwidth >> 1)` (utility.c:274),
            // which differs from this only below 8. A sub-8 block never
            // reaches here: its chroma covers the parent 8x8 and is stitched
            // from the covered cells' own MVs, which is a different C function
            // (`inter_chroma_4xn_pred`) — see
            // `inter_md_arm::predict_inter_chroma_sub8`.
            bwidth_uv: bw / 2,
            bheight_uv: bh / 2,
            super_block_size: sb_size as i32,
        },
        &[DspMv { x: mv.x, y: mv.y }],
        &[rp],
        &[sf],
        &edges,
        out,
        out_stride,
    );
}

/// The same block predicted on ALL THREE planes — §1s item 6.
///
/// C's driver takes one `component_mask` and does luma and both chroma planes
/// in one call (`enc_inter_prediction.c`; the chroma arm reuses the LUMA
/// block dims for `compute_subpel_params` and only halves the ORIGIN, see
/// `port_pd_pred.rs:302-306`), so this is one call and not three.
///
/// `u_out` / `v_out` are `bw/2 x bh/2` at `uv_stride`. All three references
/// must carry their replicated margins — C pads chroma at
/// `(border + ss_x) >> ss_x` (`enc_dec_process.c:1102`), which
/// [`crate::picture::PaddedRef`] already does.
#[allow(clippy::too_many_arguments)]
pub fn predict_inter_yuv(
    refs: (&PaddedPlane, &PaddedPlane, &PaddedPlane),
    org_x: usize,
    org_y: usize,
    bw: usize,
    bh: usize,
    mv: Mv,
    interp_filters: u32,
    sb_size: usize,
    frame_w: usize,
    frame_h: usize,
    y_out: &mut [u8],
    y_stride: usize,
    u_out: &mut [u8],
    v_out: &mut [u8],
    uv_stride: usize,
) {
    let sf = ScaleFactors::setup_for_frame(
        frame_w as i32,
        frame_h as i32,
        frame_w as i32,
        frame_h as i32,
    );
    let edges = mb_edges(org_x, org_y, bw, bh, frame_w, frame_h);
    fn plane(p: &PaddedPlane) -> RefPlane<'_> {
        RefPlane {
            buf: &p.buf,
            origin: p.origin,
            stride: p.stride,
            width: p.width as i32,
            height: p.height as i32,
        }
    }
    let mut pred = PredPlanes {
        y: y_out,
        y_stride,
        u: u_out,
        u_stride: uv_stride,
        v: v_out,
        v_stride: uv_stride,
    };
    av1_inter_prediction_light_pd1(
        &BlkGeom {
            org_x: org_x as i32,
            org_y: org_y as i32,
            bwidth: bw,
            bheight: bh,
            // C `blk_geom->bwidth_uv` is `MAX(4, bwidth >> 1)` (utility.c:274),
            // which differs from this only below 8. A sub-8 block never
            // reaches here: its chroma covers the parent 8x8 and is stitched
            // from the covered cells' own MVs, which is a different C function
            // (`inter_chroma_4xn_pred`) — see
            // `inter_md_arm::predict_inter_chroma_sub8`.
            bwidth_uv: bw / 2,
            bheight_uv: bh / 2,
            super_block_size: sb_size as i32,
        },
        &[DspMv { x: mv.x, y: mv.y }],
        &[plane(refs.0)],
        &[plane(refs.1)],
        &[plane(refs.2)],
        &[sf],
        &edges,
        interp_filters,
        &mut pred,
        LUMA_MASK | CHROMA_MASK,
    );
}

/// [`predict_inter_yuv`] against a TRUE 10-BIT reference.
///
/// The bd10 full-RD funnel residuals every candidate against a 10-bit
/// prediction (`Cand::pred10`). An INTRA candidate gets one from
/// `predict_unit_hbd` off the 10-bit recon canvas; an inter candidate had NO
/// producer at all, so `cand.pred10` stayed empty and `tx_unit_hbd` indexed a
/// zero-length slice — the panic that made 10-bit video unreachable.
///
/// The geometry is byte-for-byte [`predict_inter_yuv`]'s: same
/// `ScaleFactors`, same `MbEdges`, same `BlkGeom`, same single-MV slice. Only
/// the sample type and the convolve entry differ, which is the same split C
/// makes inside `svt_inter_predictor_light_pd1` on `bd`.
///
/// `chroma` is `None` on a monochrome block, exactly as
/// [`crate::picture::PaddedRefHbd::uv`] is.
#[allow(clippy::too_many_arguments)]
pub fn predict_inter_yuv_hbd(
    y_ref: &crate::picture::PaddedPlaneHbd,
    chroma: Option<(
        &crate::picture::PaddedPlaneHbd,
        &crate::picture::PaddedPlaneHbd,
    )>,
    org_x: usize,
    org_y: usize,
    bw: usize,
    bh: usize,
    mv: Mv,
    interp_filters: u32,
    sb_size: usize,
    frame_w: usize,
    frame_h: usize,
    bit_depth: u8,
    y_out: &mut [u16],
    y_stride: usize,
    u_out: &mut [u16],
    v_out: &mut [u16],
    uv_stride: usize,
) {
    let sf = ScaleFactors::setup_for_frame(
        frame_w as i32,
        frame_h as i32,
        frame_w as i32,
        frame_h as i32,
    );
    let edges = mb_edges(org_x, org_y, bw, bh, frame_w, frame_h);
    fn plane(p: &crate::picture::PaddedPlaneHbd) -> RefPlane16<'_> {
        RefPlane16 {
            buf: &p.buf,
            origin: p.origin,
            stride: p.stride,
            width: p.width as i32,
            height: p.height as i32,
        }
    }
    // Same contract as `predict_inter_luma`'s scratch: the chroma planes are
    // unread under `LUMA_MASK`, but the driver takes them by slice, so hand it
    // something indexable rather than an empty slice.
    let mut u_scratch = [0u16; 1];
    let mut v_scratch = [0u16; 1];
    let mask = if chroma.is_some() {
        LUMA_MASK | CHROMA_MASK
    } else {
        LUMA_MASK
    };
    let (u_dst, u_dst_stride): (&mut [u16], usize) = if chroma.is_some() {
        (u_out, uv_stride)
    } else {
        (&mut u_scratch, 1)
    };
    let (v_dst, v_dst_stride): (&mut [u16], usize) = if chroma.is_some() {
        (v_out, uv_stride)
    } else {
        (&mut v_scratch, 1)
    };
    let mut pred = PredPlanes16 {
        y: y_out,
        y_stride,
        u: u_dst,
        u_stride: u_dst_stride,
        v: v_dst,
        v_stride: v_dst_stride,
    };
    let (uref, vref) = match chroma {
        Some((u, v)) => (plane(u), plane(v)),
        None => (plane(y_ref), plane(y_ref)),
    };
    av1_inter_prediction_light_pd1_hbd(
        &BlkGeom {
            org_x: org_x as i32,
            org_y: org_y as i32,
            bwidth: bw,
            bheight: bh,
            // C `blk_geom->bwidth_uv` is `MAX(4, bwidth >> 1)` (utility.c:274),
            // which differs from this only below 8. A sub-8 block never
            // reaches here: its chroma covers the parent 8x8 and is stitched
            // from the covered cells' own MVs, which is a different C function
            // (`inter_chroma_4xn_pred`) — see
            // `inter_md_arm::predict_inter_chroma_sub8`.
            bwidth_uv: bw / 2,
            bheight_uv: bh / 2,
            super_block_size: sb_size as i32,
        },
        &[DspMv { x: mv.x, y: mv.y }],
        &[plane(y_ref)],
        &[uref],
        &[vref],
        &[sf],
        &edges,
        interp_filters,
        &mut pred,
        mask,
        i32::from(bit_depth),
    );
}

/// One WARPED-CAUSAL block, luma + chroma, exactly as C reconstructs it.
///
/// This is NOT [`predict_inter_yuv`] with a flag. C reaches warp through a
/// DIFFERENT driver: `av1_inter_prediction` (enc_inter_prediction.c:3205), not
/// `av1_inter_prediction_light_pd1`, and inside it `is_wm` routes each plane
/// to `svt_av1_warp_plane` instead of the convolve — there is no
/// `compute_subpel_params` on that path at all, and **the motion vector is
/// unused**: the affine model IS the motion.
///
/// Two things C makes easy to get wrong, both reproduced here:
///
/// * **The reference extent handed to the warp is PLANE-LOCAL** — `width >>
///   ss_x`, `height >> ss_y` — with the block's position as `p_col` / `p_row`.
/// * **Chroma falls back to TRANSLATION when it would be smaller than 8x8**
///   (`is_wm = ... && bwidth_uv >= 8 && bheight_uv >= 8`, :3382-3385, which is
///   spec 7.11.3.1). A 8x8 luma block warps its luma and CONVOLVES its chroma.
///   Getting this wrong is wrong pixels on every small warped block.
///
/// The luma `is_wm` needs no such guard because the injector cannot produce a
/// warped candidate below 8x8 (`svt_aom_warped_motion_parameters` returns
/// false for `bwidth < 8 || bheight < 8`), which C asserts at :3279.
#[allow(clippy::too_many_arguments)]
pub fn predict_inter_yuv_warped(
    refs: (&PaddedPlane, Option<(&PaddedPlane, &PaddedPlane)>),
    wm: &mut svtav1_types::motion::WarpedMotionParams,
    org_x: usize,
    org_y: usize,
    bw: usize,
    bh: usize,
    mv: Mv,
    interp_filters: u32,
    sb_size: usize,
    frame_w: usize,
    frame_h: usize,
    y_out: &mut [u8],
    y_stride: usize,
    u_out: &mut [u8],
    v_out: &mut [u8],
    uv_stride: usize,
) {
    use svtav1_dsp::port_convolve::ConvolveParams;
    use svtav1_dsp::port_enc_make_pred::{DstPlane, SrcPlanes, enc_make_inter_predictor};

    let sf = ScaleFactors::setup_for_frame(
        frame_w as i32,
        frame_h as i32,
        frame_w as i32,
        frame_h as i32,
    );
    let edges = mb_edges(org_x, org_y, bw, bh, frame_w, frame_h);
    let geom = RefGeometry {
        super_block_size: sb_size as i32,
        frame_width: frame_w as i32,
        frame_height: frame_h as i32,
    };
    // C `get_conv_params_no_round(0, tmp_dst_y, 128, is_compound, bit_depth)`
    // for luma and stride 64 for chroma. Not compound here — this port injects
    // no compound candidate — so the conv buffer is never read; it is sized
    // rather than elided so that wiring compound later is a call-site change.
    let mut conv_buf = alloc::vec![0u16; 128 * 128];
    let cp_y = ConvolveParams::no_round(false, 128, false, 8);
    let cp_uv = ConvolveParams::no_round(false, 64, false, 8);

    enc_make_inter_predictor(
        SrcPlanes::Lbd(&refs.0.buf),
        refs.0.origin,
        refs.0.stride,
        DstPlane::Lbd(y_out),
        y_stride,
        &mut conv_buf,
        org_y as i32,
        org_x as i32,
        DspMv { x: mv.x, y: mv.y },
        &sf,
        &cp_y,
        interp_filters,
        None,
        Some(wm),
        geom,
        bw,
        bh,
        &edges,
        0,
        0,
        0,
        8,
        false,
        true,
    )
    .expect("the luma warp leaf takes an 8-bit plane into an 8-bit destination");

    let Some((uref, vref)) = refs.1 else {
        return;
    };
    let (cw, chh) = (bw / 2, bh / 2);
    // Spec 7.11.3.1 / C :3382-3385.
    let uv_is_wm = cw >= 8 && chh >= 8;
    // C `ROUND_UV(x) / 2` — the chroma origin of the block, which for a 4:2:0
    // plane is the luma origin rounded DOWN to even and halved.
    let (cx, cy) = ((org_x & !1) / 2, (org_y & !1) / 2);
    for (plane, r, dst) in [(1usize, uref, &mut *u_out), (2, vref, &mut *v_out)] {
        enc_make_inter_predictor(
            SrcPlanes::Lbd(&r.buf),
            r.origin,
            r.stride,
            DstPlane::Lbd(dst),
            uv_stride,
            &mut conv_buf,
            cy as i32,
            cx as i32,
            DspMv { x: mv.x, y: mv.y },
            &sf,
            &cp_uv,
            interp_filters,
            None,
            Some(wm),
            geom,
            cw,
            chh,
            &edges,
            plane,
            1,
            1,
            8,
            false,
            uv_is_wm,
        )
        .expect("the chroma leaf takes an 8-bit plane into an 8-bit destination");
    }
}

/// [`predict_inter_yuv_warped`] against a TRUE 10-BIT reference.
///
/// The 10-bit twin of the warp arm, for the bd10 level re-encode. Same driver
/// (`enc_make_inter_predictor` with `is_wm`), same plane-local extent, same
/// spec-7.11.3.1 chroma fallback below 8x8 — only the sample type differs, and
/// C makes the same split inside `svt_av1_warp_plane`.
#[allow(clippy::too_many_arguments)]
pub fn predict_inter_yuv_warped_hbd(
    y_ref: &crate::picture::PaddedPlaneHbd,
    chroma: Option<(
        &crate::picture::PaddedPlaneHbd,
        &crate::picture::PaddedPlaneHbd,
    )>,
    wm: &mut svtav1_types::motion::WarpedMotionParams,
    org_x: usize,
    org_y: usize,
    bw: usize,
    bh: usize,
    mv: Mv,
    interp_filters: u32,
    sb_size: usize,
    frame_w: usize,
    frame_h: usize,
    bit_depth: u8,
    y_out: &mut [u16],
    y_stride: usize,
    u_out: &mut [u16],
    v_out: &mut [u16],
    uv_stride: usize,
) {
    use svtav1_dsp::port_convolve::ConvolveParams;
    use svtav1_dsp::port_enc_make_pred::{DstPlane, SrcPlanes, enc_make_inter_predictor};

    let sf = ScaleFactors::setup_for_frame(
        frame_w as i32,
        frame_h as i32,
        frame_w as i32,
        frame_h as i32,
    );
    let edges = mb_edges(org_x, org_y, bw, bh, frame_w, frame_h);
    let geom = RefGeometry {
        super_block_size: sb_size as i32,
        frame_width: frame_w as i32,
        frame_height: frame_h as i32,
    };
    let bd = i32::from(bit_depth);
    let mut conv_buf = alloc::vec![0u16; 128 * 128];
    let cp_y = ConvolveParams::no_round(false, 128, false, bd);
    let cp_uv = ConvolveParams::no_round(false, 64, false, bd);

    enc_make_inter_predictor(
        SrcPlanes::Hbd(&y_ref.buf),
        y_ref.origin,
        y_ref.stride,
        DstPlane::Hbd(y_out),
        y_stride,
        &mut conv_buf,
        org_y as i32,
        org_x as i32,
        DspMv { x: mv.x, y: mv.y },
        &sf,
        &cp_y,
        interp_filters,
        None,
        Some(wm),
        geom,
        bw,
        bh,
        &edges,
        0,
        0,
        0,
        bd,
        false,
        true,
    )
    .expect("the hbd warp leaf takes a u16 plane into a u16 destination");

    let Some((uref, vref)) = chroma else {
        return;
    };
    let (cw, chh) = (bw / 2, bh / 2);
    let uv_is_wm = cw >= 8 && chh >= 8;
    let (cx, cy) = ((org_x & !1) / 2, (org_y & !1) / 2);
    for (plane, r, dst) in [(1usize, uref, &mut *u_out), (2, vref, &mut *v_out)] {
        enc_make_inter_predictor(
            SrcPlanes::Hbd(&r.buf),
            r.origin,
            r.stride,
            DstPlane::Hbd(dst),
            uv_stride,
            &mut conv_buf,
            cy as i32,
            cx as i32,
            DspMv { x: mv.x, y: mv.y },
            &sf,
            &cp_uv,
            interp_filters,
            None,
            Some(wm),
            geom,
            cw,
            chh,
            &edges,
            plane,
            1,
            1,
            bd,
            false,
            uv_is_wm,
        )
        .expect("the hbd chroma leaf takes a u16 plane into a u16 destination");
    }
}

/// One mode-info cell of the parent 8x8, as C's `xd->mi[row * mi_stride + col]`
/// hands it to `inter_chroma_4xn_pred`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Sub8ChromaMi {
    /// C `is_inter_block(&this_mbmi->block_mi)`.
    pub is_inter: bool,
    /// C `this_mbmi->block_mi.ref_frame[0]`. Bipred is disallowed below 8, so
    /// there is never a second one.
    pub ref_frame: i8,
    /// C `this_mbmi->block_mi.mv[0]`.
    pub mv: Mv,
    /// C `this_mbmi->block_mi.interp_filters`.
    pub interp_filters: u32,
}

/// C `ROUND_UV(x) / 2` (`definitions.h:348`) — the chroma origin of a block's
/// chroma REFERENCE area, which for a sub-8 block is the parent 8x8's, not the
/// block's own halved origin.
#[inline]
#[must_use]
pub fn round_uv_half(x: usize) -> usize {
    ((x >> 3) << 3) / 2
}

/// One chroma plane of one rectangle, through C's `svt_aom_enc_make_inter_predictor`.
#[allow(clippy::too_many_arguments)]
fn chroma_unit(
    r: &PaddedPlane,
    plane: usize,
    cx: usize,
    cy: usize,
    cwidth: usize,
    cheight: usize,
    mv: Mv,
    interp_filters: u32,
    sf: &ScaleFactors,
    geom: RefGeometry,
    edges: &svtav1_dsp::port_subpel_params::MbEdges,
    conv_buf: &mut [u16],
    dst: &mut [u8],
    dst_stride: usize,
) {
    use svtav1_dsp::port_convolve::ConvolveParams;
    use svtav1_dsp::port_enc_make_pred::{DstPlane, SrcPlanes, enc_make_inter_predictor};
    let cp = ConvolveParams::no_round(false, 64, false, 8);
    enc_make_inter_predictor(
        SrcPlanes::Lbd(&r.buf),
        r.origin,
        r.stride,
        DstPlane::Lbd(dst),
        dst_stride,
        conv_buf,
        cy as i32,
        cx as i32,
        DspMv { x: mv.x, y: mv.y },
        sf,
        &cp,
        interp_filters,
        None,
        None,
        geom,
        cwidth,
        cheight,
        edges,
        plane,
        1,
        1,
        8,
        false,
        false,
    )
    .expect("the chroma unit takes an 8-bit plane into an 8-bit destination");
}

/// C `inter_chroma_4xn_pred` (`enc_inter_prediction.c:3023-3200`) — the chroma
/// of a 4xN / Nx4 inter block.
///
/// A sub-8 luma block's chroma covers MORE than that block: at 4:2:0 the
/// chroma reference area is the PARENT 8x8's, so it spans the 4xN block and
/// its sibling. C therefore does not predict it with one MV — it walks the
/// covered mode-info cells and predicts each `b4_w x b4_h` piece with THAT
/// cell's own reference, MV and interpolation filters. Predicting the whole
/// area with the current block's MV is wrong pixels wherever the sibling
/// chose a different one, and no decoder reproduces them.
///
/// `mis` is indexed `[row + 1][col + 1]` for C's `row`/`col` in `-1..=0`;
/// `mis[1][1]` is the CURRENT block, which C fills from the candidate being
/// predicted (`:3036-3043`) rather than from the grid.
///
/// Returns `false` exactly where C returns 0 — the block is not sub-8, or one
/// of the covered cells is INTRA — and the caller then predicts the chroma
/// area the ordinary way, with this block's own MV (`:3374`, `!sub8x8_inter`).
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn predict_inter_chroma_sub8x8(
    refs_by_frame: &[Option<(&PaddedPlane, &PaddedPlane)>; 8],
    mis: &[[Sub8ChromaMi; 2]; 2],
    org_x: usize,
    org_y: usize,
    bw: usize,
    bh: usize,
    sb_size: usize,
    frame_w: usize,
    frame_h: usize,
    u_out: &mut [u8],
    v_out: &mut [u8],
    uv_stride: usize,
) -> bool {
    // C `sub8x8_inter` (:3045) at ss_x = ss_y = 1.
    if bw >= 8 && bh >= 8 {
        return false;
    }
    // C :3055-3056.
    let row_start: i32 = if bh == 4 { -1 } else { 0 };
    let col_start: i32 = if bw == 4 { -1 } else { 0 };
    // C :3058-3065 — ANY covered cell being intra abandons the whole path.
    for row in row_start..=0 {
        for col in col_start..=0 {
            if !mis[(row + 1) as usize][(col + 1) as usize].is_inter {
                return false;
            }
        }
    }
    // C :3069-3074. `svt_aom_scale_chroma_bsize` at 4:2:0 raises each
    // dimension to at least 8, so `b8_*` is the chroma extent the funnel
    // already calls `cw`/`chh`.
    let (b4_w, b4_h) = (bw >> 1, bh >> 1);
    let (b8_w, b8_h) = (bw.max(8) >> 1, bh.max(8) >> 1);
    let (cx, cy) = (round_uv_half(org_x), round_uv_half(org_y));

    let sf = ScaleFactors::setup_for_frame(
        frame_w as i32,
        frame_h as i32,
        frame_w as i32,
        frame_h as i32,
    );
    // C passes the CURRENT block's `xd`, so the clamp edges are the block's.
    let edges = mb_edges(org_x, org_y, bw, bh, frame_w, frame_h);
    let geom = RefGeometry {
        super_block_size: sb_size as i32,
        frame_width: frame_w as i32,
        frame_height: frame_h as i32,
    };
    let mut conv_buf = alloc::vec![0u16; 64 * 64];

    let mut row = row_start;
    let mut y = 0;
    while y < b8_h {
        let mut col = col_start;
        let mut x = 0;
        while x < b8_w {
            let mi = mis[(row + 1) as usize][(col + 1) as usize];
            let Some((uref, vref)) = refs_by_frame[mi.ref_frame.max(0) as usize] else {
                // The grid named a reference this frame does not carry. C
                // cannot reach this (`ref_frame_type_arr` is the same table
                // both sides read), so it is a wiring bug, not a fallback.
                panic!(
                    "a sub-8 chroma neighbour names reference {} with no DPB picture",
                    mi.ref_frame
                );
            };
            for (plane, r, dst) in [(1usize, uref, &mut *u_out), (2, vref, &mut *v_out)] {
                chroma_unit(
                    r,
                    plane,
                    cx + x,
                    cy + y,
                    b4_w,
                    b4_h,
                    mi.mv,
                    mi.interp_filters,
                    &sf,
                    geom,
                    &edges,
                    &mut conv_buf,
                    &mut dst[y * uv_stride + x..],
                    uv_stride,
                );
            }
            col += 1;
            x += b4_w;
        }
        row += 1;
        y += b4_h;
    }
    true
}

/// The `!sub8x8_inter` chroma arm of `svt_aom_inter_prediction` (`:3374`):
/// the whole chroma reference area, at the ROUND_UV origin, with THIS block's
/// own MV. For a block 8x8 or larger this is just "the block's chroma"; for a
/// sub-8 one it is the parent 8x8's chroma predicted from one MV, which is
/// what C falls back to when a covered cell is intra.
#[allow(clippy::too_many_arguments)]
pub fn predict_inter_chroma_whole(
    uref: &PaddedPlane,
    vref: &PaddedPlane,
    org_x: usize,
    org_y: usize,
    bw: usize,
    bh: usize,
    mv: Mv,
    interp_filters: u32,
    sb_size: usize,
    frame_w: usize,
    frame_h: usize,
    u_out: &mut [u8],
    v_out: &mut [u8],
    uv_stride: usize,
) {
    let (cw, chh) = (bw.max(8) >> 1, bh.max(8) >> 1);
    let (cx, cy) = (round_uv_half(org_x), round_uv_half(org_y));
    let sf = ScaleFactors::setup_for_frame(
        frame_w as i32,
        frame_h as i32,
        frame_w as i32,
        frame_h as i32,
    );
    let edges = mb_edges(org_x, org_y, bw, bh, frame_w, frame_h);
    let geom = RefGeometry {
        super_block_size: sb_size as i32,
        frame_width: frame_w as i32,
        frame_height: frame_h as i32,
    };
    let mut conv_buf = alloc::vec![0u16; 64 * 64];
    for (plane, r, dst) in [(1usize, uref, &mut *u_out), (2, vref, &mut *v_out)] {
        chroma_unit(
            r,
            plane,
            cx,
            cy,
            cw,
            chh,
            mv,
            interp_filters,
            &sf,
            geom,
            &edges,
            &mut conv_buf,
            dst,
            uv_stride,
        );
    }
}

/// C's `is_wm` (`enc_inter_prediction.c:3276-3277`, and again at `:3382` for
/// chroma): a block goes through the WARP driver when its motion mode says so
/// **or** when it is a GLOBALMV block whose reference carries a model above
/// TRANSLATION.
///
/// The second term is the one that is easy to miss. A GLOBALMV candidate is
/// injected with `motion_mode = SIMPLE_TRANSLATION`
/// (`mode_decision.c:2639`) and an MV from `gm_get_motion_vector_enc`, so it
/// LOOKS like a translation; but with a ROTZOOM or AFFINE model the decoder
/// warps it, and predicting it as a translation is wrong pixels that no
/// decoder reproduces. The MV is still passed — the warp driver reads it for
/// the reference block's position.
#[must_use]
pub fn inter_pred_uses_warp(
    motion_mode: crate::port_entropy_inter::modes::MotionMode,
    mode: u8,
    bw: usize,
    bh: usize,
    wm: &svtav1_types::motion::WarpedMotionParams,
) -> bool {
    motion_mode == crate::port_entropy_inter::modes::MotionMode::WarpedCausal
        || crate::port_entropy_inter::modes::is_global_mv_block_dims(mode, bw, bh, wm.wm_type)
}

/// One inter-predicted block at TRUE 10 BITS, dispatching on its motion mode.
///
/// The bd10 level re-encode's entry: it rebuilds a COMMITTED leaf's prediction
/// rather than searching, so it takes the decision's own motion mode and warp
/// model instead of deriving either.
#[allow(clippy::too_many_arguments)]
pub fn predict_inter_leaf_hbd(
    y_ref: &crate::picture::PaddedPlaneHbd,
    chroma: Option<(
        &crate::picture::PaddedPlaneHbd,
        &crate::picture::PaddedPlaneHbd,
    )>,
    motion_mode: crate::port_entropy_inter::modes::MotionMode,
    // C's `is_wm` for this block — `inter_pred_uses_warp`.
    is_wm: bool,
    wm_params: svtav1_types::motion::WarpedMotionParams,
    org_x: usize,
    org_y: usize,
    bw: usize,
    bh: usize,
    mv: Mv,
    interp_filters: u32,
    sb_size: usize,
    frame_w: usize,
    frame_h: usize,
    bit_depth: u8,
    y_out: &mut [u16],
    y_stride: usize,
    u_out: &mut [u16],
    v_out: &mut [u16],
    uv_stride: usize,
) {
    use crate::port_entropy_inter::modes::MotionMode;
    // OBMC is unwired; a committed leaf cannot carry it, and REFUSING is the
    // only honest answer if one ever does — silently predicting it as a plain
    // translation is wrong pixels a decoder will not reproduce.
    assert!(
        motion_mode != MotionMode::ObmcCausal,
        "no OBMC candidate is injected, so no leaf can commit one"
    );
    if is_wm {
        let mut wm = wm_params;
        predict_inter_yuv_warped_hbd(
            y_ref,
            chroma,
            &mut wm,
            org_x,
            org_y,
            bw,
            bh,
            mv,
            interp_filters,
            sb_size,
            frame_w,
            frame_h,
            bit_depth,
            y_out,
            y_stride,
            u_out,
            v_out,
            uv_stride,
        );
    } else {
        predict_inter_yuv_hbd(
            y_ref,
            chroma,
            org_x,
            org_y,
            bw,
            bh,
            mv,
            interp_filters,
            sb_size,
            frame_w,
            frame_h,
            bit_depth,
            y_out,
            y_stride,
            u_out,
            v_out,
            uv_stride,
        );
    }
}

/// C `xd->mb_to_*_edge` (`svt_aom_init_xd`, adaptive_mv_pred.c:1054-1057), in
/// EIGHTH-pel: `-((mi_col * MI_SIZE) * 8)` and
/// `((mi_cols - bw_mi - mi_col) * MI_SIZE) * 8`. They bound the MV clamp, so
/// getting the sign or the unit wrong moves the prediction rather than
/// failing.
fn mb_edges(
    org_x: usize,
    org_y: usize,
    bw: usize,
    bh: usize,
    frame_w: usize,
    frame_h: usize,
) -> MbEdges {
    let (mi_cols, mi_rows) = (frame_w.div_ceil(4) as i32, frame_h.div_ceil(4) as i32);
    let (mi_col, mi_row) = ((org_x / 4) as i32, (org_y / 4) as i32);
    let (bw_mi, bh_mi) = ((bw / 4) as i32, (bh / 4) as i32);
    MbEdges {
        to_left: -((mi_col * 4) * 8),
        to_right: (mi_cols - bw_mi - mi_col) * 4 * 8,
        to_top: -((mi_row * 4) * 8),
        to_bottom: (mi_rows - bh_mi - mi_row) * 4 * 8,
    }
}
