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
//! [`predict_inter_luma`] is the luma half (§1s item 5). The chroma half
//! (item 6) is the same driver with `CHROMA_MASK` and the two `PaddedPlane`s
//! from [`crate::picture::PaddedRef::uv`]; it is NOT exposed yet because the
//! chroma pass that would call it still routes an inter block's chroma
//! through `encode_chroma_block_dc` (an INTRA DC predictor), and adding an
//! entry point with no caller would be surface without a positive control.
//! The driver call it needs is one `component_mask` away.
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
use svtav1_dsp::port_pd_pred::{BlkGeom, PredPlanes, RefPlane, av1_inter_prediction_light_pd1};
use svtav1_dsp::port_scale_factors::ScaleFactors;
use svtav1_dsp::port_subpel_params::{MbEdges, Mv as DspMv};
use svtav1_types::motion::Mv;

/// C `LUMA_MASK` — `av1_inter_prediction_light_pd1`'s luma-only component
/// mask (`enc_inter_prediction.c`; MDS0 asks for luma, MDS3 for chroma,
/// which is what makes the two passes independent).
const LUMA_ONLY: u32 = 1;

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
    // C `xd->mb_to_*_edge` (`svt_aom_init_xd`, adaptive_mv_pred.c:1054-1057),
    // in EIGHTH-pel: `-((mi_col * MI_SIZE) * 8)` and
    // `((mi_cols - bw_mi - mi_col) * MI_SIZE) * 8`. They bound the MV clamp,
    // so getting the sign or the unit wrong moves the prediction rather than
    // failing.
    let (mi_cols, mi_rows) = (frame_w.div_ceil(4) as i32, frame_h.div_ceil(4) as i32);
    let (mi_col, mi_row) = ((org_x / 4) as i32, (org_y / 4) as i32);
    let (bw_mi, bh_mi) = ((bw / 4) as i32, (bh / 4) as i32);
    let edges = MbEdges {
        to_left: -((mi_col * 4) * 8),
        to_right: (mi_cols - bw_mi - mi_col) * 4 * 8,
        to_top: -((mi_row * 4) * 8),
        to_bottom: (mi_rows - bh_mi - mi_row) * 4 * 8,
    };
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
        LUMA_ONLY,
    );
}
