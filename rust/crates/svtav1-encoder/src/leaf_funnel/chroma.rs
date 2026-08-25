//! One full-loop chroma evaluation of a candidate's chroma prediction.
//!
//! C `svt_aom_full_loop_uv` (full_loop.c:2161), reached from two places with
//! identical settings: `search_best_mds3_uv_mode`'s full loop and MDS3's own
//! chroma pass. Three arms, because the PREDICTION differs while everything
//! after it does not -- an intra `(uv_mode, uv_delta)` pair at 8 bits, the
//! same at 10, and the IntraBC DV copy at 10 (an IBC candidate's chroma is a
//! block copy, not an intra mode, so scoring it through the intra arm would
//! score it against a prediction it does not use).
//!
//! These were three closures in `evaluate_leaf`, defined near the top and
//! called from candidate injection, the independent-uv search and MDS3 --
//! i.e. they spanned the whole 5,000-line body and pinned every one of their
//! ~14 captured locals into that scope. [`ChromaCtx`] names that capture set
//! instead: it is the per-leaf chroma geometry and quantiser state, derived
//! once and unchanged for the leaf's lifetime.

use super::*;

/// The per-leaf chroma constants every `eval_*` call reads.
///
/// Candidate-INDEPENDENT by construction: the chroma pair geometry comes from
/// the block's `ROUND_UV` origin and the neighbour txb contexts are read once
/// before the search (the neighbouring bytes cannot change during this block's
/// own evaluation). That is what makes a shared context correct here rather
/// than a per-candidate recompute.
pub(super) struct ChromaCtx {
    /// Chroma pair dims (C `blk_geom` `bsize_uv`).
    pub(super) cw: usize,
    pub(super) chh: usize,
    /// Chroma-plane origin of the pair (C `ROUND_UV(luma) >> 1`).
    pub(super) ccx: usize,
    pub(super) ccy: usize,
    /// Prediction geometry for the chroma unit (availability tables +
    /// frame-edge clamps), anchored at the PAIR for sub-8 chroma-ref blocks.
    pub(super) uv_geom: UnitGeom,
    /// C `get_filt_type` for the chroma plane -- the above/left coded modes'
    /// smoothness, which selects the edge filter strength.
    pub(super) filt_type_uv: i32,
    /// Spatial-distortion crop: the part of the chroma txb inside the ALIGNED
    /// frame (C `cropped_tx_width_uv`/`_height_uv`, full_loop.c:2228). The
    /// identity on any block that does not straddle the aligned extent.
    pub(super) uv_crop: (usize, usize),
    /// Cb/Cr txb-skip and dc-sign contexts (C `svt_aom_get_txb_ctx`). Zero
    /// unless the config prices real coefficient contexts.
    pub(super) cb_tsc: usize,
    pub(super) cb_dsc: usize,
    pub(super) cr_tsc: usize,
    pub(super) cr_dsc: usize,
    /// Frame RDOQ policy (`frame.rdoq_level > 0`).
    pub(super) do_rdoq: bool,
    /// Per-plane 8-bit quantiser tables (equal to the luma table when the
    /// frame header carries no chroma qindex delta).
    pub(super) qt_u: QuantTable,
    pub(super) qt_v: QuantTable,
}

/// C's chroma RD args, shared by all three arms: MDS3 chroma scores the
/// SPATIAL SSE, and the crop applies only to that arm.
fn rd_args(cx: &ChromaCtx, frame: &FunnelFrame, plane_dir: usize) -> TxRdArgs {
    TxRdArgs {
        spatial_dist: true, // MDS3 chroma is the spatial SSE (<<4)
        intra_dir: plane_dir,
        coeff_rate_est_lvl: frame.cfg.coeff_rate_est_lvl,
        tx_bias: frame.tx_bias,
        crop: cx.uv_crop,
    }
}

/// The 8-bit arm: predict a `(uv_mode, uv_delta)` pair and run both chroma
/// planes through the transform pipeline.
pub(super) fn eval_uv(
    cx: &ChromaCtx,
    fx: &FunnelCtx<'_>,
    uv: u8,
    uv_delta: i8,
) -> (TxUnitOut, TxUnitOut) {
    let (frame, rates) = (fx.frame, fx.rates);
    let (cw, chh, ccx, ccy) = (cx.cw, cx.chh, cx.ccx, cx.ccy);
    let mut u_pred = vec![0u8; cw * chh];
    let mut v_pred = vec![0u8; cw * chh];
    predict_unit(
        fx.u_recon,
        fx.c_stride,
        ccx,
        ccy,
        cw,
        chh,
        uv,
        uv_delta,
        FI_NONE,
        &cx.uv_geom,
        frame.cfg.edge_filter,
        cx.filt_type_uv,
        &mut u_pred,
    );
    predict_unit(
        fx.v_recon,
        fx.c_stride,
        ccx,
        ccy,
        cw,
        chh,
        uv,
        uv_delta,
        FI_NONE,
        &cx.uv_geom,
        frame.cfg.edge_filter,
        cx.filt_type_uv,
        &mut v_pred,
    );
    let tt = uv_tx_type(uv, cw, chh);
    let u_out = tx_unit(
        fx.u_src,
        fx.c_stride,
        ccy * fx.c_stride + ccx,
        &u_pred,
        cw,
        0,
        cw,
        chh,
        tt,
        1,
        cx.cb_tsc,
        cx.cb_dsc,
        0,
        &cx.qt_u,
        frame,
        rates,
        cx.do_rdoq,
        true,
        cx.uv_crop,
        true,
        RateMode::Exact,
    );
    let v_out = tx_unit(
        fx.v_src,
        fx.c_stride,
        ccy * fx.c_stride + ccx,
        &v_pred,
        cw,
        0,
        cw,
        chh,
        tt,
        1,
        cx.cr_tsc,
        cx.cr_dsc,
        0,
        &cx.qt_v,
        frame,
        rates,
        cx.do_rdoq,
        true,
        cx.uv_crop,
        true,
        RateMode::Exact,
    );
    (u_out, v_out)
}

/// The 10-bit twin of [`eval_uv`].
///
/// C reaches the same facades at both depths -- the spatial chroma distortion
/// is `svt_full_distortion_kernel16_bits` at `hbd_md != 0` (pic_operators.c:257)
/// -- so only the pixel type, the quantiser table and the lambda move. This
/// matters because the MDS3 block cost is JOINT (luma + chroma): with the luma
/// terms at 10 bits and chroma left at 8, chroma would be ~16x under-weighted
/// and every uv-follows-luma mode flip would be decided on luma alone.
pub(super) fn eval_uv_hbd(
    cx: &ChromaCtx,
    fx: &FunnelCtx<'_>,
    b: &Bd10Rd,
    uv: u8,
    uv_delta: i8,
) -> (TxUnitOutHbd, TxUnitOutHbd) {
    let frame = fx.frame;
    let (cw, chh, ccx, ccy) = (cx.cw, cx.chh, cx.ccx, cx.ccy);
    let mut u_pred = vec![0u16; cw * chh];
    let mut v_pred = vec![0u16; cw * chh];
    predict_unit_hbd(
        fx.u_recon10.as_deref().unwrap(),
        fx.c_stride,
        ccx,
        ccy,
        cw,
        chh,
        uv,
        uv_delta,
        FI_NONE,
        &cx.uv_geom,
        frame.cfg.edge_filter,
        cx.filt_type_uv,
        &mut u_pred,
        b.bd,
    );
    predict_unit_hbd(
        fx.v_recon10.as_deref().unwrap(),
        fx.c_stride,
        ccx,
        ccy,
        cw,
        chh,
        uv,
        uv_delta,
        FI_NONE,
        &cx.uv_geom,
        frame.cfg.edge_filter,
        cx.filt_type_uv,
        &mut v_pred,
        b.bd,
    );
    let tt = uv_tx_type(uv, cw, chh);
    tx_pair_hbd(cx, fx, b, &u_pred, &v_pred, tt)
}

/// The IntraBC twin of [`eval_uv_hbd`]: an IBC candidate's chroma is the DV
/// copy / half-pel bilinear from the chroma recon (NOT an intra uv mode), so
/// the bd10 arm cannot reuse [`eval_uv_hbd`] -- that would score the candidate
/// against a prediction it does not use. The tx type is the INTER rule the u8
/// arm already applies (the luma winner's txb-0 type when the chroma ext set
/// allows it, else DCT; `tx_type_search`, product_coding_loop.c:5087-5096),
/// which is why the caller passes it in.
pub(super) fn eval_uv_ibc_hbd(
    cx: &ChromaCtx,
    fx: &FunnelCtx<'_>,
    b: &Bd10Rd,
    dv: svtav1_types::motion::Mv,
    tt: usize,
) -> (TxUnitOutHbd, TxUnitOutHbd) {
    let (cw, chh, ccx, ccy) = (cx.cw, cx.chh, cx.ccx, cx.ccy);
    let mut u_pred = vec![0u16; cw * chh];
    let mut v_pred = vec![0u16; cw * chh];
    let frame_ch = fx.frame.frame_h_px / 2;
    crate::intrabc_pred::predict_intrabc_chroma(
        fx.u_recon10.as_deref().unwrap(),
        fx.c_stride,
        ccx,
        ccy,
        cw,
        chh,
        fx.c_stride,
        frame_ch,
        dv,
        &mut u_pred,
    );
    crate::intrabc_pred::predict_intrabc_chroma(
        fx.v_recon10.as_deref().unwrap(),
        fx.c_stride,
        ccx,
        ccy,
        cw,
        chh,
        fx.c_stride,
        frame_ch,
        dv,
        &mut v_pred,
    );
    tx_pair_hbd(cx, fx, b, &u_pred, &v_pred, tt)
}

/// The shared 10-bit tail of both hbd arms: given a Cb/Cr prediction pair and
/// a tx type, run each plane through the 10-bit transform pipeline. The two
/// arms differ ONLY in how `u_pred`/`v_pred` were produced.
fn tx_pair_hbd(
    cx: &ChromaCtx,
    fx: &FunnelCtx<'_>,
    b: &Bd10Rd,
    u_pred: &[u16],
    v_pred: &[u16],
    tt: usize,
) -> (TxUnitOutHbd, TxUnitOutHbd) {
    let (frame, rates) = (fx.frame, fx.rates);
    let (cw, chh) = (cx.cw, cx.chh);
    let u_out = tx_unit_hbd(
        &b.u_src10,
        cw,
        0,
        u_pred,
        cw,
        0,
        cw,
        chh,
        tt,
        1,
        cx.cb_tsc,
        cx.cb_dsc,
        &b.qt_u,
        frame.rdoq_level,
        b.lambda,
        frame.sharpness,
        rates,
        cx.do_rdoq,
        b.bd,
        b.qt_u.qm_level,
        Some(&rd_args(cx, frame, 0)),
    );
    let v_out = tx_unit_hbd(
        &b.v_src10,
        cw,
        0,
        v_pred,
        cw,
        0,
        cw,
        chh,
        tt,
        1,
        cx.cr_tsc,
        cx.cr_dsc,
        &b.qt_v,
        frame.rdoq_level,
        b.lambda,
        frame.sharpness,
        rates,
        cx.do_rdoq,
        b.bd,
        b.qt_v.qm_level,
        Some(&rd_args(cx, frame, 0)),
    );
    (u_out, v_out)
}
