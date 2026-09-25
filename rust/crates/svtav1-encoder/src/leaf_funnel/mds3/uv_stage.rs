use super::*;

#[inline(always)]
pub(super) fn chroma_complexity_is_luma(
    fx: &mut FunnelCtx<'_>,
    bd10_rd: &Option<Bd10Rd>,
    y_src: &[u8],
    y_src_stride: usize,
    y_src_off: usize,
    cands: &mut [Cand],
    ci: usize,
    cfg: FunnelCfg,
    w: usize,
    has_uv: bool,
    cw: usize,
    chh: usize,
    ccx: usize,
    ccy: usize,
    nb_u: &mut Option<crate::partition::NeighborEdges>,
    nb_v: &mut Option<crate::partition::NeighborEdges>,
    uv_geom: UnitGeom,
    filt_type_uv: i32,
    use_tx_shortcuts_mds3: bool,
    best_pred: &crate::vecpool::PoolVec<u8>,
    best_pred10: &[u16],
) -> bool {
    // ---- Chroma complexity detector for the tx-shortcut N4 gate ----
    // C `chroma_complexity_check_pred` (product_coding_loop.c:7014-7017)
    // runs for EVERY candidate when chroma_detector_level is armed and
    // the tx-shortcut gate is on — not just the intra CfL lane.
    // chroma_complexity == COMPONENT_LUMA iff neither the SAD arm
    // (chroma SAD > 2*luma SAD) nor the variance arm (th=150) fired.
    let chroma_luma = if has_uv
        && cfg.tx_shortcut.chroma_detector_level != 0
        && (cfg.tx_shortcut.apply_pf_on_coeffs != 0 || use_tx_shortcuts_mds3)
    {
        let c_off = ccy * fx.c_stride + ccx;
        let (sad, var) = match &bd10_rd {
            Some(b) => {
                // 10-bit: the detector's SAD arm uses the 10-bit
                // chroma predictions and luma, and the variance arm
                // uses vf_hbd_10 on the 10-bit chroma source.
                let (det_u10, det_v10) = if let Some(ic) = cands[ci].inter.as_deref() {
                    (ic.u_pred10.clone(), ic.v_pred10.clone())
                } else {
                    let mut u = dirty_pool::<u16>(cw * chh);
                    let mut v = dirty_pool::<u16>(cw * chh);
                    predict_unit_hbd(
                        fx.u_recon10.as_deref().unwrap(),
                        fx.c_stride,
                        ccx,
                        ccy,
                        cw,
                        chh,
                        cands[ci].uv,
                        cands[ci].uv_delta,
                        FI_NONE,
                        &uv_geom,
                        cfg.edge_filter,
                        filt_type_uv,
                        &mut u,
                        b.bd,
                    );
                    predict_unit_hbd(
                        fx.v_recon10.as_deref().unwrap(),
                        fx.c_stride,
                        ccx,
                        ccy,
                        cw,
                        chh,
                        cands[ci].uv,
                        cands[ci].uv_delta,
                        FI_NONE,
                        &uv_geom,
                        cfg.edge_filter,
                        filt_type_uv,
                        &mut v,
                        b.bd,
                    );
                    (u.into_vec(), v.into_vec())
                };
                let s = chroma_detector_fires_hbd(
                    &b.y_src10,
                    w,
                    best_pred10,
                    w,
                    &b.u_src10,
                    &b.v_src10,
                    &det_u10,
                    &det_v10,
                    cw,
                    cw,
                    chh,
                );
                // HBD variance: same formula as the u8 arm but on
                // 10-bit data — C calls vf_hbd_10 on the u16 source
                // with offset 1<<(bd-1) (eb_av1_var_offs_hbd).
                let v_ = chroma_var_arm_fires_hbd(&b.u_src10, &b.v_src10, cw, chh, 150, b.bd);
                (s, v_)
            }
            None => {
                let (det_u, det_v) = if let Some(ic) = cands[ci].inter.as_deref() {
                    (ic.u_pred.clone(), ic.v_pred.clone())
                } else {
                    let mut u = dirty_pool::<u8>(cw * chh);
                    let mut v = dirty_pool::<u8>(cw * chh);
                    predict_unit(
                        fx.u_recon,
                        fx.c_stride,
                        ccx,
                        ccy,
                        cw,
                        chh,
                        cands[ci].uv,
                        cands[ci].uv_delta,
                        FI_NONE,
                        &uv_geom,
                        cfg.edge_filter,
                        filt_type_uv,
                        nb_u,
                        &mut u,
                    );
                    predict_unit(
                        fx.v_recon,
                        fx.c_stride,
                        ccx,
                        ccy,
                        cw,
                        chh,
                        cands[ci].uv,
                        cands[ci].uv_delta,
                        FI_NONE,
                        &uv_geom,
                        cfg.edge_filter,
                        filt_type_uv,
                        nb_v,
                        &mut v,
                    );
                    (u.into_vec(), v.into_vec())
                };
                let s = chroma_detector_fires(
                    y_src,
                    y_src_stride,
                    y_src_off,
                    best_pred,
                    w,
                    fx.u_src,
                    fx.v_src,
                    &det_u,
                    &det_v,
                    fx.c_stride,
                    c_off,
                    cw,
                    chh,
                );
                // The variance arm's `th` is hardcoded 150
                // (product_coding_loop.c:6128), NOT `cplx_th`.
                let v_ = chroma_var_arm_fires(fx.u_src, fx.v_src, fx.c_stride, c_off, cw, chh, 150);
                (s, v_)
            }
        };
        !sad && !var
    } else {
        true // COMPONENT_LUMA — no detector ran
    };
    chroma_luma
}

#[inline(always)]
pub(super) fn eval_inter_chroma(
    fx: &mut FunnelCtx<'_>,
    cx: &chroma::ChromaCtx,
    frame: &FunnelFrame,
    rates: &MdRates,
    has_uv: bool,
    cw: usize,
    chh: usize,
    ccx: usize,
    ccy: usize,
    uv_crop: (usize, usize),
    qt_u: QuantTable,
    qt_v: QuantTable,
    cb_tsc: usize,
    cb_dsc: usize,
    cr_tsc: usize,
    cr_dsc: usize,
    best_txb_eob: &smallvec::SmallVec<[u16; 16]>,
    best_txb_type: &smallvec::SmallVec<[u8; 16]>,
    chroma_gate: TxGate,
    cand: &Cand,
    ibc_uv_tt: &mut Option<usize>,
) -> (TxUnitOut, TxUnitOut) {
    let (u_out, v_out) = if has_uv && let Some(ic) = cand.inter.as_deref() {
        // INTER chroma (docs/INTER-ENCODE-PLAN.md §1s item 6): the
        // motion-compensated prediction, produced with the LUMA one in a
        // single `av1_inter_prediction_light_pd1` call at injection — C's
        // chroma arm reuses the luma block's `compute_subpel_params` result
        // at a halved origin, so predicting it here would be different
        // arithmetic. The tx-type rule is the INTER one, identical to the
        // IntraBC arm below (tx_type_search, product_coding_loop.c:5087),
        // INCLUDING the decoder's tx_type_map semantics: the map holds
        // DCT_DCT for an all-zero covering luma txb (decodetxb.c:148-154),
        // which `inter_uv_tx_type` reproduces from `best_txb_eob[0]`.
        let tt = inter_uv_tx_type(
            best_txb_eob.first().copied().unwrap_or(0),
            best_txb_type.first().copied().unwrap_or(0),
            frame.coded_lossless,
            cw,
            chh,
        );
        let u_out = tx_unit(
            fx.u_src,
            fx.c_stride,
            ccy * fx.c_stride + ccx,
            &ic.u_pred,
            cw,
            0,
            cw,
            chh,
            tt,
            1,
            cb_tsc,
            cb_dsc,
            0,
            &qt_u,
            frame,
            rates,
            frame.rdoq,
            true,
            uv_crop,
            true,
            RateMode::Exact,
        );
        let v_out = tx_unit(
            fx.v_src,
            fx.c_stride,
            ccy * fx.c_stride + ccx,
            &ic.v_pred,
            cw,
            0,
            cw,
            chh,
            tt,
            1,
            cr_tsc,
            cr_dsc,
            0,
            &qt_v,
            frame,
            rates,
            frame.rdoq,
            true,
            uv_crop,
            true,
            RateMode::Exact,
        );
        *ibc_uv_tt = Some(tt);
        (u_out, v_out)
    } else if has_uv && let Some((dv, _)) = cand.ibc {
        // IBC chunk 7: IntraBC chroma — the DV copy / half-pel bilinear
        // from the chroma recon canvases (enc_inter_prediction chroma
        // arm, sf_identity), with the INTER chroma tx type rule: the
        // luma winner's txb-0 type when the chroma ext set allows it,
        // else DCT (tx_type_search, product_coding_loop.c:5087-5096) —
        // with the decoder's map semantics: an all-zero covering luma txb
        // reads DCT_DCT (decodetxb.c:148-154).
        // No CfL, no ind-uv, no detector (all intra-only).
        let mut u_pred = dirty_pool::<u8>(cw * chh);
        let mut v_pred = dirty_pool::<u8>(cw * chh);
        let frame_ch = frame.frame_h_px / 2;
        crate::intrabc_pred::predict_intrabc_chroma(
            fx.u_recon,
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
            fx.v_recon,
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
        let tt = inter_uv_tx_type(
            best_txb_eob.first().copied().unwrap_or(0),
            best_txb_type.first().copied().unwrap_or(0),
            frame.coded_lossless,
            cw,
            chh,
        );
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
            cb_tsc,
            cb_dsc,
            0,
            &qt_u,
            frame,
            rates,
            frame.rdoq,
            true,
            uv_crop,
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
            cr_tsc,
            cr_dsc,
            0,
            &qt_v,
            frame,
            rates,
            frame.rdoq,
            true,
            uv_crop,
            true,
            RateMode::Exact,
        );
        *ibc_uv_tt = Some(tt);
        (u_out, v_out)
    } else if has_uv {
        chroma::eval_uv(cx, fx, cand.uv, cand.uv_delta, chroma_gate)
    } else {
        (TxUnitOut::absent(), TxUnitOut::absent())
    };
    (u_out, v_out)
}
