use super::*;

#[inline(always)]
pub(super) fn eval_intra_chroma(
    fx: &mut FunnelCtx<'_>,
    cx: &chroma::ChromaCtx,
    bd10_rd: &Option<Bd10Rd>,
    lambda: u64,
    y_src: &[u8],
    y_src_stride: usize,
    y_src_off: usize,
    y_recon: &[u8],
    y_stride: usize,
    ind_uv: &Option<[(u8, i8); 13]>,
    frame: &FunnelFrame,
    rates: &MdRates,
    cfg: FunnelCfg,
    do_rdoq: bool,
    w: usize,
    h: usize,
    abs_x: usize,
    abs_y: usize,
    has_uv: bool,
    cfl_allowed: usize,
    use_angle: bool,
    cw: usize,
    chh: usize,
    ccx: usize,
    ccy: usize,
    mut nb_u: Option<crate::partition::NeighborEdges>,
    mut nb_v: Option<crate::partition::NeighborEdges>,
    uv_geom: UnitGeom,
    filt_type_uv: i32,
    uv_crop: (usize, usize),
    qt_u: QuantTable,
    qt_v: QuantTable,
    cb_tsc: usize,
    cb_dsc: usize,
    cr_tsc: usize,
    cr_dsc: usize,
    allow_pal: bool,
    pal_uv_no: u64,
    pal_uv_no_y1: u64,
    best_recon: &crate::vecpool::PoolVec<u8>,
    best_recon10: &Vec<u16>,
    best_pred: crate::vecpool::PoolVec<u8>,
    best_pred10: Vec<u16>,
    chroma_gate: TxGate,
    cand: &Cand,
    u_out: &mut TxUnitOut,
    v_out: &mut TxUnitOut,
    uv_out10: &mut Option<(TxUnitOutHbd, TxUnitOutHbd)>,
    uv_mode_final: &mut u8,
    uv_delta_final: &mut i8,
    fcr_final: &mut u64,
    cfl_idx_final: &mut u8,
    cfl_signs_final: &mut u8,
) {
    // IntraBC candidates: no chroma detector, no CfL, no uv rewrite —
    // C excludes inter-classified candidates from every chroma search
    // (search_best_mds3_uv_mode :7335, the CfL arm :6932-equivalent).
    if has_uv && !cand.is_inter() {
        // Chroma complexity detector (chroma_complexity_check_pred,
        // product_coding_loop.c:6095), use_var=1: cfl_complexity ==
        // COMPONENT_CHROMA iff the SAD arm (cb/cr pred SAD > 2x luma
        // pred SAD) OR the variance arm (per-pixel source variance >
        // cplx_th) fires. Uses the candidate's uv PREDICTION.
        let mut u_pred = dirty_pool::<u8>(cw * chh);
        let mut v_pred = dirty_pool::<u8>(cw * chh);
        predict_unit(
            fx.u_recon,
            fx.c_stride,
            ccx,
            ccy,
            cw,
            chh,
            cand.uv,
            cand.uv_delta,
            FI_NONE,
            &uv_geom,
            cfg.edge_filter,
            filt_type_uv,
            &mut nb_u,
            &mut u_pred,
        );
        predict_unit(
            fx.v_recon,
            fx.c_stride,
            ccx,
            ccy,
            cw,
            chh,
            cand.uv,
            cand.uv_delta,
            FI_NONE,
            &uv_geom,
            cfg.edge_filter,
            filt_type_uv,
            &mut nb_v,
            &mut v_pred,
        );
        let c_off = ccy * fx.c_stride + ccx;
        // LUMA reference for the detector's SAD: C reads
        // `cand_buffer->pred->y_buffer` (product_coding_loop.c:6106), and
        // by the time the detector runs (:7178) the luma TX loop (:7139)
        // has already returned. What that leaves in the buffer depends on
        // the winning tx_depth:
        //   - depth 0: the TX loop re-predicts only `if (ctx->tx_depth)`
        //     (:5393-5395) and at depth 0 `tx_cand_bf == cand_bf`
        //     (:5363-5365), so the buffer still holds the MDS0 whole-block
        //     prediction == `cand.pred`.
        //   - depth > 0: each txb is re-predicted from RECON neighbours
        //     into a SEPARATE scratch buffer (`ctx->cand_bf_tx_depth_1/2`),
        //     and on winning, `update_tx_cand_bf` (:5269, called :5487)
        //     memcpy's that scratch pred back over the full
        //     bheight x bwidth of `cand_bf->pred->y_buffer`.
        // So the detector's luma SAD is against the WINNING DEPTH's
        // prediction, not the MDS0 one. Passing `cand.pred` here made the
        // port's `y_dist` diverge on every candidate whose winning depth
        // was > 0 (measured: 1040/7323 records on 258947 q40 p3, and zero
        // mismatches at depth 0), flipping `sad_arm` — and hence whether
        // CfL is evaluated at all — on 22 of them.
        // At bd10 C runs this SAD arm on the 10-bit source and the 10-bit
        // candidate prediction (:6048-6072), which does NOT reduce to the
        // u8 arm — see `chroma_detector_fires_hbd`. The chroma predictions
        // are the same (uv, uv_delta) pair `u_pred`/`v_pred` above, at 10
        // bits; the luma one is `best_pred10`, the winning depth's 10-bit
        // prediction.
        let sad_arm = match &bd10_rd {
            Some(b) => {
                let mut u_p10d = vec![0u16; cw * chh];
                let mut v_p10d = vec![0u16; cw * chh];
                for (plane_recon, dst) in [
                    (fx.u_recon10.as_deref().unwrap(), &mut u_p10d),
                    (fx.v_recon10.as_deref().unwrap(), &mut v_p10d),
                ] {
                    predict_unit_hbd(
                        plane_recon,
                        fx.c_stride,
                        ccx,
                        ccy,
                        cw,
                        chh,
                        cand.uv,
                        cand.uv_delta,
                        FI_NONE,
                        &uv_geom,
                        cfg.edge_filter,
                        filt_type_uv,
                        dst,
                        b.bd,
                    );
                }
                chroma_detector_fires_hbd(
                    &b.y_src10,
                    w,
                    &best_pred10,
                    w,
                    &b.u_src10,
                    &b.v_src10,
                    &u_p10d,
                    &v_p10d,
                    cw,
                    cw,
                    chh,
                )
            }
            None => chroma_detector_fires(
                y_src,
                y_src_stride,
                y_src_off,
                &best_pred,
                w,
                fx.u_src,
                fx.v_src,
                &u_pred,
                &v_pred,
                fx.c_stride,
                c_off,
                cw,
                chh,
            ),
        };
        // M6 cfl_level 4 -> cplx_th 10. Both detector arms use it: the
        // caller gates CfL on cfl_complexity == COMPONENT_CHROMA when
        // cplx_th != 0 (product_coding_loop.c:7183). At bd10 C runs the
        // use_var arm through `vf_hbd_10` on the u16 source
        // (product_coding_loop.c:6177-6190): the u8 arm reads the truncated
        // source and uses different rounding, so it flips the razor-edge
        // `> cplx_th` comparisons this gate exists to decide.
        let var_arm = cfg.cfl_cplx_th != 0
            && match &bd10_rd {
                Some(b) => {
                    chroma_var_arm_fires_hbd(&b.u_src10, &b.v_src10, cw, chh, cfg.cfl_cplx_th, b.bd)
                }
                None => chroma_var_arm_fires(
                    fx.u_src,
                    fx.v_src,
                    fx.c_stride,
                    c_off,
                    cw,
                    chh,
                    cfg.cfl_cplx_th,
                ),
            };
        // cplx_th 0 (cfl_level 1/2, M0) BYPASSES the detector — CfL is
        // always evaluated (C :7183 `!cplx_th`); otherwise gate on either
        // detector arm (SAD 2x-luma or per-pixel variance > cplx_th).
        let cfl_would_run = cfg.cfl_cplx_th == 0 || sad_arm || var_arm;
        #[cfg(feature = "std")]
        if crate::dbgenv::canddbg() && crate::depth_refine::nsqdbg_here(abs_x, abs_y) {
            eprintln!(
                "NSQDBG CFDET mi=({},{}) {w}x{h} mode={} uv={} uvd={} cplx_th={} sad_arm={sad_arm} var_arm={var_arm} run={cfl_would_run}",
                abs_y / 4,
                abs_x / 4,
                cand.mode,
                cand.uv,
                cand.uv_delta,
                cfg.cfl_cplx_th,
            );
        }
        // Two CfL decision paths, both C `cfl_prediction`
        // (product_coding_loop.c:3795), gated identically on
        // `cfl_ctrls.enabled` + detector + intra + MDS3 + MAX(dims)<=32
        // (:7183-7193) — NO ind_uv gate there. They differ only in the
        // CfL-vs-non-CfL COMPARISON:
        //  - uv-follows-luma (!ind_uv_avail, M6): non_cfl_cost via
        //    full_loop_uv is_full_loop=0 (TRANSFORM domain) vs cfl_rd
        //    (transform) — the freq decision below.
        //  - independent-uv (ind_uv_avail, M0..M5): CfL forwarded, then
        //    `check_best_indepedant_cfl` (:3964, called :7237) compares
        //    `cfl_uv_cost` vs `best_uv_cost[mode]` — BOTH via full_loop_uv
        //    is_full_loop=1 (SPATIAL @ SSSE_MDS3 for allintra), the
        //    spatial decision in the else-if below.
        // C `ctx->ind_uv_avail` is PER-BLOCK RUNTIME state, not a preset
        // constant: it is reset to 0 for every block (:9931) and set to 1
        // only when the independent-uv search actually RUNS — gated at
        // :10165 on `uv_mode == CHROMA_MODE_0 && ind_uv_last_mds &&
        // sq_size < 128 && has_uv && perform_ind_uv_search_last_mds(...)`.
        // That predicate (:1470) counts MDS3 intra candidates as
        // `!is_inter && (!skip_ind_uv_if_only_dc || uv_mode != UV_DC_PRED)`
        // and returns `count > 0`; at M2..M5 (chroma_level 4,
        // enc_mode_config.c:5781) `skip_ind_uv_if_only_dc = 1`, so when
        // EVERY MDS3 candidate is UV_DC the search is skipped and
        // ind_uv_avail stays 0. C then reaches `if (cfl_performed) { if
        // (ctx->ind_uv_avail) check_best_indepedant_cfl(...) }` (:7258)
        // with a FALSE ind_uv_avail, so no `check_best_indepedant_cfl`
        // revert runs and CfL is decided by the uv-follows-luma
        // TRANSFORM-domain compare inside `cfl_prediction` instead of the
        // ind-uv SPATIAL compare. `ind_uv` above is Some iff that same
        // search ran (its `any(uv != 0)` gate IS
        // perform_ind_uv_search_last_mds for skip_ind_uv_if_only_dc = 1,
        // and the M0/M1 independent branch always runs) — so it is
        // exactly `ind_uv_avail`. Keying the two CfL paths off the preset
        // flags instead made the port take the SPATIAL path on the 263/7323
        // blocks where C has ind_uv_avail == 0, picking CfL where C keeps DC.
        let cfl_uv_follows = ind_uv.is_none();
        let cfl_ind_uv = ind_uv.is_some();
        // The uv-follows-luma arm below runs at BOTH depths (task: bd10
        // CfL). Under the bd10 full-RD the DECISION terms — the non-CfL
        // chroma cost, every per-alpha CfL cost, and hence the winning
        // alpha — are all computed at 10 bits (`cfl_predict_hbd` +
        // `tx_unit_hbd` + the bd10 quant tables + `full_lambda_md[
        // EB_10_BIT_MD]`), exactly as C does when `hbd_md != 0`. The u8
        // chroma buffers then FOLLOW that decision, which is the same
        // model the rest of the bd10 funnel uses (10-bit costs decide,
        // u8 buffers are carried for the pre-filter searches).
        //
        // The `cfl_ind_uv` arm (M0..M5) is still 8-bit only: its decision
        // is `check_best_indepedant_cfl`'s SPATIAL compare against
        // `best_uv_cost[mode]`, which needs the whole independent-uv
        // search at 10 bits, not just the CfL side. So it stays gated on
        // `bd10_rd.is_none()` below — at p0..p5 no bd10 leaf can be CfL,
        // which keeps `bd10_tree_supported` (widened to admit CfL) in
        // lockstep with what the search can actually produce.
        let cfl_gate = cfg.cfl_enabled && cfl_would_run && w <= 32 && h <= 32;
        if cfl_gate && cfl_uv_follows {
            // ---- cfl_prediction (product_coding_loop.c:3795) ----
            // non_cfl_cost = RDCOST(coeff_bits + uv fast rate, dist) over
            // the non-CFL chroma. C recomputes it with svt_aom_full_loop_uv
            // is_full_loop=0 -> TRANSFORM-domain distortion (product_coding
            // _loop.c:3800-3860), which is NOT the spatial SSE u_out/v_out
            // carry (those feed the final block RD). Re-run the non-CFL
            // chroma TX with spatial_dist=false to get the matching freq
            // distortion; coeffs/bits are unchanged by the dist domain so
            // the rate stays u_out/v_out.bits. cand.fcr is the uv fast rate
            // on the uv-follows-luma path.
            let nc_tt = uv_tx_type(cand.uv, cw, chh);
            let u_nc = tx_unit(
                fx.u_src,
                fx.c_stride,
                c_off,
                &u_pred,
                cw,
                0,
                cw,
                chh,
                nc_tt,
                1,
                cb_tsc,
                cb_dsc,
                0,
                &qt_u,
                frame,
                rates,
                frame.rdoq,
                false,
                uv_crop,
                // R1: only `.dist` is read (the `non_cfl_cost` rdcost
                // below takes its RATE from `u_out`/`v_out`). C's
                // `cfl_prediction` recomputes this cost through
                // `svt_aom_full_loop_uv` with `is_full_loop = 0`
                // (product_coding_loop.c:3800-3860), which never enters
                // the `is_full_loop && mds_do_spatial_sse` inverse
                // transform at full_loop.c:2313.
                false,
                RateMode::Exact,
            );
            let v_nc = tx_unit(
                fx.v_src,
                fx.c_stride,
                c_off,
                &v_pred,
                cw,
                0,
                cw,
                chh,
                nc_tt,
                1,
                cr_tsc,
                cr_dsc,
                0,
                &qt_v,
                frame,
                rates,
                frame.rdoq,
                false,
                uv_crop,
                // R1: only `.dist` is read (the `non_cfl_cost` rdcost
                // below takes its RATE from `u_out`/`v_out`). C's
                // `cfl_prediction` recomputes this cost through
                // `svt_aom_full_loop_uv` with `is_full_loop = 0`
                // (product_coding_loop.c:3800-3860), which never enters
                // the `is_full_loop && mds_do_spatial_sse` inverse
                // transform at full_loop.c:2313.
                false,
                RateMode::Exact,
            );
            let non_cfl_cost = rdcost(
                lambda,
                u_out.bits as u64 + v_out.bits as u64 + cand.fcr,
                u_nc.dist + v_nc.dist,
            );
            // compute_cfl_ac_components: subsample the winning luma recon
            // (whole block, origin 0) and subtract its DC.
            // Pooled: the CfL scratch is rebuilt for every candidate that
            // reaches the CfL arm -- 43,214 allocating calls on the canonical
            // alloc cell, with the DC pair below another 86,428.
            let mut pred_buf_q3 =
                zeroed_pool::<i16>(svtav1_dsp::intra_pred::CFL_BUF_LINE * chh.max(1));
            cfl_ac_subsample(
                y_recon,
                y_stride,
                best_recon,
                abs_x,
                abs_y,
                w,
                h,
                &mut pred_buf_q3,
            );
            svtav1_dsp::intra_pred::cfl_subtract_average(&mut pred_buf_q3, cw, chh);
            // CfL base is the DC chroma prediction (C regenerates it when
            // the non-CFL uv mode != DC).
            let mut u_dc = dirty_pool::<u8>(cw * chh);
            let mut v_dc = dirty_pool::<u8>(cw * chh);
            predict_unit(
                fx.u_recon,
                fx.c_stride,
                ccx,
                ccy,
                cw,
                chh,
                0,
                0,
                FI_NONE,
                &uv_geom,
                cfg.edge_filter,
                filt_type_uv,
                &mut nb_u,
                &mut u_dc,
            );
            predict_unit(
                fx.v_recon,
                fx.c_stride,
                ccx,
                ccy,
                cw,
                chh,
                0,
                0,
                FI_NONE,
                &uv_geom,
                cfg.edge_filter,
                filt_type_uv,
                &mut nb_v,
                &mut v_dc,
            );
            // bd10 decision depth: the 10-bit AC luma (subsampled from the
            // 10-bit WINNING luma recon, C `compute_cfl_ac_components` at
            // `hbd_md != 0`) and the 10-bit DC chroma base. Hoisted out of
            // the compare below because the chosen-alpha chroma TX needs
            // them again once CfL wins.
            let cfl10: Option<(crate::vecpool::PoolVec<i16>, Vec<u16>, Vec<u16>)> =
                bd10_rd.as_ref().map(|b| {
                    let mut ac10 =
                        zeroed_pool::<i16>(svtav1_dsp::intra_pred::CFL_BUF_LINE * chh.max(1));
                    cfl_ac_subsample_hbd(
                        fx.y_recon10.as_deref().unwrap(),
                        y_stride,
                        best_recon10,
                        abs_x,
                        abs_y,
                        w,
                        h,
                        &mut ac10,
                    );
                    svtav1_dsp::intra_pred::cfl_subtract_average(&mut ac10, cw, chh);
                    let mut u_dc10 = vec![0u16; cw * chh];
                    let mut v_dc10 = vec![0u16; cw * chh];
                    for (plane_recon, dst) in [
                        (fx.u_recon10.as_deref().unwrap(), &mut u_dc10),
                        (fx.v_recon10.as_deref().unwrap(), &mut v_dc10),
                    ] {
                        predict_unit_hbd(
                            plane_recon,
                            fx.c_stride,
                            ccx,
                            ccy,
                            cw,
                            chh,
                            0, // UV_DC_PRED — CfL's base
                            0,
                            FI_NONE,
                            &uv_geom,
                            cfg.edge_filter,
                            filt_type_uv,
                            dst,
                            b.bd,
                        );
                    }
                    (ac10, u_dc10, v_dc10)
                });
            // SVTAV1_UVDC: the bd10 CfL DC base, one line per (block,
            // candidate). Mirrors the C `--wrap svt_aom_full_loop_uv`
            // `pu=/pv=` readout (cand_bf->pred origin at the CfL-search
            // calls), which is the only externally observable handle on
            // C's chroma recon NEIGHBOUR state — `cfl_prediction` and
            // friends are static and cannot be wrapped. Constant per
            // (block, plane), so joining the two dumps on `org` bisects a
            // chroma neighbour-recon drift to its first divergent block.
            #[cfg(feature = "std")]
            if let Some((_, u_dc10, v_dc10)) = cfl10.as_ref() {
                static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
                if dbg_on(&ON, "SVTAV1_UVDC") {
                    eprintln!(
                        "UVDC org=({abs_x},{abs_y}) {w}x{h} udc={} vdc={}",
                        u_dc10[0], v_dc10[0]
                    );
                }
            }
            // The spatial-run chroma coeff bits at the decision depth — the
            // rate half of `non_cfl_cost`. Read out before the compare so
            // `uv_out10` is free to be replaced when CfL wins.
            let uv10_bits: u64 = uv_out10
                .as_ref()
                .map_or(0, |(u10, v10)| u10.bits as u64 + v10.bits as u64);
            // C `av1_cost_calc_cfl` for one component at hbd: CfL-predict
            // from the 10-bit DC base + AC luma, then TX/quant with the
            // bd10 table and take the TRANSFORM-domain distortion
            // (`svt_aom_full_loop_uv` is_full_loop=0).
            let plane_cost10 = |plane: usize, alpha_q3: i32| -> (u64, i32) {
                let b = bd10_rd.as_ref().unwrap();
                let (ac10, u_dc10, v_dc10) = cfl10.as_ref().unwrap();
                let (src, dc, tsc, dsc, qt) = if plane == 0 {
                    (&b.u_src10, u_dc10, cb_tsc, cb_dsc, &b.qt_u)
                } else {
                    (&b.v_src10, v_dc10, cr_tsc, cr_dsc, &b.qt_v)
                };
                let mut cfl_pred = vec![0u16; cw * chh];
                svtav1_dsp::hbd::cfl_predict_hbd(
                    ac10,
                    dc,
                    cw,
                    &mut cfl_pred,
                    cw,
                    alpha_q3,
                    b.bd,
                    cw,
                    chh,
                );
                let o = tx_unit_hbd(
                    frame.coded_lossless,
                    src,
                    cw,
                    0,
                    &cfl_pred,
                    cw,
                    0,
                    cw,
                    chh,
                    0,
                    1,
                    tsc,
                    dsc,
                    qt,
                    frame.rdoq_level,
                    b.lambda,
                    frame.sharpness,
                    frame.rdoq_allintra_rd_mult,
                    cand.inter.is_some(),
                    rates,
                    do_rdoq,
                    b.bd,
                    qt.qm_level,
                    Some(&TxRdArgs {
                        spatial_dist: false,
                        intra_dir: 0,
                        coeff_rate_est_lvl: cfg.coeff_rate_est_lvl,
                        tx_bias: frame.tx_bias,
                        crop: uv_crop,
                    }),
                );
                (o.dist, o.bits)
            };
            // The alpha search AND the CfL-vs-non-CfL compare both run at
            // the decision depth. Mixing them (an 8-bit CfL cost against a
            // 10-bit non-CfL cost, or vice versa) is a ~16x scale error and
            // decides every block wrongly — which is why the two costs are
            // produced by the same `b.lambda` / bd10-quant pair here.
            let (cfl_idx, cfl_signs, cfl_rd, cfl_cmp_cost) = match &bd10_rd {
                Some(b) => {
                    // non_cfl_cost at 10 bits: same expression as the u8 one
                    // above (spatial-run coeff bits + uv fast rate, against
                    // the freq-domain re-run's distortion).
                    let mut u_p10 = vec![0u16; cw * chh];
                    let mut v_p10 = vec![0u16; cw * chh];
                    for (plane_recon, dst) in [
                        (fx.u_recon10.as_deref().unwrap(), &mut u_p10),
                        (fx.v_recon10.as_deref().unwrap(), &mut v_p10),
                    ] {
                        predict_unit_hbd(
                            plane_recon,
                            fx.c_stride,
                            ccx,
                            ccy,
                            cw,
                            chh,
                            cand.uv,
                            cand.uv_delta,
                            FI_NONE,
                            &uv_geom,
                            cfg.edge_filter,
                            filt_type_uv,
                            dst,
                            b.bd,
                        );
                    }
                    let freq10 =
                        |src: &[u16], pred: &[u16], tsc: usize, dsc: usize, qt: &QuantTable| {
                            tx_unit_hbd(
                                frame.coded_lossless,
                                src,
                                cw,
                                0,
                                pred,
                                cw,
                                0,
                                cw,
                                chh,
                                nc_tt,
                                1,
                                tsc,
                                dsc,
                                qt,
                                frame.rdoq_level,
                                b.lambda,
                                frame.sharpness,
                                frame.rdoq_allintra_rd_mult,
                                cand.inter.is_some(),
                                rates,
                                do_rdoq,
                                b.bd,
                                qt.qm_level,
                                Some(&TxRdArgs {
                                    spatial_dist: false,
                                    intra_dir: 0,
                                    coeff_rate_est_lvl: cfg.coeff_rate_est_lvl,
                                    tx_bias: frame.tx_bias,
                                    crop: uv_crop,
                                }),
                            )
                        };
                    let u_nc10 = freq10(&b.u_src10, &u_p10, cb_tsc, cb_dsc, &b.qt_u);
                    let v_nc10 = freq10(&b.v_src10, &v_p10, cr_tsc, cr_dsc, &b.qt_v);
                    let nc10 = rdcost(b.lambda, uv10_bits + cand.fcr, u_nc10.dist + v_nc10.dist);
                    let (i, s, rd) = md_cfl_alpha_search(
                        plane_cost10,
                        rates,
                        b.lambda,
                        cand.mode as usize,
                        cfg.cfl_itr_th,
                    );
                    (i, s, rd, nc10)
                }
                None => {
                    let (i, s, rd) = md_cfl_rd_pick_alpha(
                        &pred_buf_q3,
                        &u_dc,
                        &v_dc,
                        fx.u_src,
                        fx.v_src,
                        fx.c_stride,
                        c_off,
                        cw,
                        chh,
                        uv_crop,
                        cb_tsc,
                        cb_dsc,
                        cr_tsc,
                        cr_dsc,
                        &qt_u,
                        &qt_v,
                        frame,
                        rates,
                        frame.rdoq,
                        lambda,
                        cand.mode as usize,
                        cfg.cfl_itr_th,
                    );
                    (i, s, rd, non_cfl_cost)
                }
            };
            #[cfg(feature = "std")]
            if crate::dbgenv::canddbg() && crate::depth_refine::nsqdbg_here(abs_x, abs_y) {
                eprintln!(
                    "NSQDBG CFLCMP org=({abs_x},{abs_y}) {w}x{h} luma={} uv={} uvd={} \
                     ub={} ud={} ueob={} vb={} vd={} veob={} ncd={} fcr={} ncc={} \
                     cflrd={} cflidx={} cflsgn={} ncub={} ncvb={} pu={} pv={} \
                     bq={} qu={} qv={} dqu={} dqv={}",
                    cand.mode,
                    cand.uv,
                    cand.uv_delta,
                    u_out.bits,
                    u_out.dist,
                    u_out.eob,
                    v_out.bits,
                    v_out.dist,
                    v_out.eob,
                    if bd10_rd.is_none() {
                        u_nc.dist + v_nc.dist
                    } else {
                        0
                    },
                    cand.fcr,
                    cfl_cmp_cost,
                    cfl_rd,
                    cfl_idx,
                    cfl_signs,
                    u_nc.bits,
                    v_nc.bits,
                    u_pred.first().copied().unwrap_or(0),
                    v_pred.first().copied().unwrap_or(0),
                    frame.base_qindex,
                    frame.qindex_u,
                    frame.qindex_v,
                    qt_u.dequant[0],
                    qt_v.dequant[0],
                );
                if std::env::var_os("SVTAV1_UVLOOP_COEFF").is_some() {
                    let dump = |c: &TxUnitOut| -> String {
                        c.qcoeff
                            .iter()
                            .take(16)
                            .map(|v| v.to_string())
                            .collect::<Vec<_>>()
                            .join(",")
                    };
                    let rdump = |r: &[u8]| -> String {
                        r.iter()
                            .take(8)
                            .map(|v| v.to_string())
                            .collect::<Vec<_>>()
                            .join(",")
                    };
                    eprintln!(
                        "CFLCOEF org=({abs_x},{abs_y}) qc={} qv={} ru8={} rv8={}",
                        dump(&*u_out),
                        dump(&*v_out),
                        rdump(&u_out.recon),
                        rdump(&v_out.recon),
                    );
                }
            }
            if cfl_rd != MAX_MODE_COST && cfl_rd < cfl_cmp_cost {
                // CfL wins: redo chroma with the winning alpha (DCT_DCT)
                // for the full TX path, and swap in the CFL mode + rate.
                let alpha_cb = cfl_idx_to_alpha(cfl_idx, cfl_signs, 0);
                let alpha_cr = cfl_idx_to_alpha(cfl_idx, cfl_signs, 1);
                let mut u_cfl = zeroed_pool::<u8>(cw * chh);
                let mut v_cfl = zeroed_pool::<u8>(cw * chh);
                svtav1_dsp::intra_pred::cfl_predict_lbd(
                    &pred_buf_q3,
                    &u_dc,
                    cw,
                    &mut u_cfl,
                    cw,
                    alpha_cb,
                    cw,
                    chh,
                );
                svtav1_dsp::intra_pred::cfl_predict_lbd(
                    &pred_buf_q3,
                    &v_dc,
                    cw,
                    &mut v_cfl,
                    cw,
                    alpha_cr,
                    cw,
                    chh,
                );
                *u_out = tx_unit(
                    fx.u_src,
                    fx.c_stride,
                    c_off,
                    &u_cfl,
                    cw,
                    0,
                    cw,
                    chh,
                    0,
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
                *v_out = tx_unit(
                    fx.v_src,
                    fx.c_stride,
                    c_off,
                    &v_cfl,
                    cw,
                    0,
                    cw,
                    chh,
                    0,
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
                // bd10: the coded chroma at the decision depth. C runs the
                // SAME chosen-alpha `svt_cfl_predict_hbd` + full TX here
                // (cfl_prediction :3860-3878), so `uv_out10` — which is
                // what the block cost, the coded levels and the neighbour
                // culs are taken from at bd10 — must be rebuilt with the
                // CfL prediction, not left on the non-CfL chroma.
                if let (Some(b), Some((ac10, u_dc10, v_dc10))) = (&bd10_rd, &cfl10) {
                    let rd10 = TxRdArgs {
                        spatial_dist: true, // MDS3 chroma is the spatial SSE
                        intra_dir: 0,
                        coeff_rate_est_lvl: cfg.coeff_rate_est_lvl,
                        tx_bias: frame.tx_bias,
                        crop: uv_crop,
                    };
                    let mut u_cfl10 = vec![0u16; cw * chh];
                    let mut v_cfl10 = vec![0u16; cw * chh];
                    svtav1_dsp::hbd::cfl_predict_hbd(
                        ac10,
                        u_dc10,
                        cw,
                        &mut u_cfl10,
                        cw,
                        alpha_cb,
                        b.bd,
                        cw,
                        chh,
                    );
                    svtav1_dsp::hbd::cfl_predict_hbd(
                        ac10,
                        v_dc10,
                        cw,
                        &mut v_cfl10,
                        cw,
                        alpha_cr,
                        b.bd,
                        cw,
                        chh,
                    );
                    let u10 = tx_unit_hbd(
                        frame.coded_lossless,
                        &b.u_src10,
                        cw,
                        0,
                        &u_cfl10,
                        cw,
                        0,
                        cw,
                        chh,
                        0,
                        1,
                        cb_tsc,
                        cb_dsc,
                        &b.qt_u,
                        frame.rdoq_level,
                        b.lambda,
                        frame.sharpness,
                        frame.rdoq_allintra_rd_mult,
                        cand.inter.is_some(),
                        rates,
                        do_rdoq,
                        b.bd,
                        b.qt_u.qm_level,
                        Some(&rd10),
                    );
                    let v10 = tx_unit_hbd(
                        frame.coded_lossless,
                        &b.v_src10,
                        cw,
                        0,
                        &v_cfl10,
                        cw,
                        0,
                        cw,
                        chh,
                        0,
                        1,
                        cr_tsc,
                        cr_dsc,
                        &b.qt_v,
                        frame.rdoq_level,
                        b.lambda,
                        frame.sharpness,
                        frame.rdoq_allintra_rd_mult,
                        cand.inter.is_some(),
                        rates,
                        do_rdoq,
                        b.bd,
                        b.qt_v.qm_level,
                        Some(&rd10),
                    );
                    *uv_out10 = Some((u10, v10));
                }
                *uv_mode_final = UV_CFL_PRED_IDX as u8;
                *cfl_idx_final = cfl_idx;
                *cfl_signs_final = cfl_signs;
                // Updated uv fast rate (get_intra_uv_fast_rate,
                // use_accurate_cfl=1): UV_CFL_PRED mode bits + alpha bits.
                *fcr_final = rates.uv[cfl_allowed][cand.mode as usize][UV_CFL_PRED_IDX] as u64
                    + rates.cfl_alpha_fac_bits[cfl_signs as usize][0][(cfl_idx >> 4) as usize]
                        as u64
                    + rates.cfl_alpha_fac_bits[cfl_signs as usize][1][(cfl_idx & 15) as usize]
                        as u64;
            }
        } else if cfl_gate && cfl_ind_uv {
            // C independent-uv CfL: cfl_prediction (ind_uv_avail branch,
            // product_coding_loop.c:3888) forwards CfL, then
            // check_best_indepedant_cfl (:3830, called :6875) keeps the
            // non-CfL uv mode iff best_uv_cost[mode] < cfl_uv_cost —
            // where best_uv_cost/best_uv_mode are keyed on the CODED
            // luma mode (DC for FILTER candidates), NOT the candidate's
            // injected uv. At M0 (ind_uv_last_mds==0, no :7063
            // pre-rewrite) a FILTER candidate arrives here still
            // carrying tbl[fimode_to_intramode[fi]]; C discards that
            // eval entirely and arbitrates CfL against the coded-mode
            // row, assigning best_uv_mode[coded] on a non-CfL win. So:
            // re-key the candidate to the coded-mode row before the
            // compare (a no-op for M1/M2/M3, whose pre-MDS3 rewrite
            // already applied it). Both costs are SPATIAL SSE
            // (full_loop_uv is_full_loop=1 @ SSSE_MDS3), unlike the
            // uv-follows-luma freq decision above.
            let (arb_uv, arb_uvd) = ind_uv.as_ref().unwrap()[cand.mode as usize];
            if (cand.uv, cand.uv_delta) != (arb_uv, arb_uvd) {
                let (u2, v2) = chroma::eval_uv(cx, fx, arb_uv, arb_uvd, chroma_gate);
                *u_out = u2;
                *v_out = v2;
                // bd10: the 10-bit chroma decision terms follow the re-key
                // (C re-runs the ind-uv-best chroma at hbd_md in
                // check_best_indepedant_cfl :3957-3995). Only fires at M0
                // (FILTER candidate, no :7063 pre-rewrite); the mds3 configs
                // pre-rewrote so this branch is a no-op there.
                if let Some(b) = bd10_rd.as_ref() {
                    *uv_out10 = Some(chroma::eval_uv_hbd(cx, fx, b, arb_uv, arb_uvd, chroma_gate));
                }
                *uv_mode_final = arb_uv;
                *uv_delta_final = arb_uvd;
                let mut f = rates.uv[cfl_allowed][cand.mode as usize][arb_uv as usize] as u64;
                if use_angle && matches!(arb_uv, 1..=8) {
                    f += rates.angle[arb_uv as usize - 1][(3 + arb_uvd) as usize] as u64;
                }
                if arb_uv == 0 {
                    f += pal_uv_no; // rd_cost.c:514 (inside uv fast rate)
                }
                *fcr_final = f;
            }
            // compute_cfl_ac_components (u8): subsample the winning luma
            // recon; the DC chroma base. Shared by both depths — at bd10
            // the u8 chroma canvas still follows the CfL decision (carried
            // for the pre-filter searches), so it is rebuilt from these.
            let mut pred_buf_q3 =
                zeroed_pool::<i16>(svtav1_dsp::intra_pred::CFL_BUF_LINE * chh.max(1));
            cfl_ac_subsample(
                y_recon,
                y_stride,
                best_recon,
                abs_x,
                abs_y,
                w,
                h,
                &mut pred_buf_q3,
            );
            svtav1_dsp::intra_pred::cfl_subtract_average(&mut pred_buf_q3, cw, chh);
            // CfL base is the DC chroma prediction (C regenerates DC pred
            // when the non-CFL uv mode != DC — we always compute it fresh).
            let mut u_dc = dirty_pool::<u8>(cw * chh);
            let mut v_dc = dirty_pool::<u8>(cw * chh);
            predict_unit(
                fx.u_recon,
                fx.c_stride,
                ccx,
                ccy,
                cw,
                chh,
                0,
                0,
                FI_NONE,
                &uv_geom,
                cfg.edge_filter,
                filt_type_uv,
                &mut nb_u,
                &mut u_dc,
            );
            predict_unit(
                fx.v_recon,
                fx.c_stride,
                ccx,
                ccy,
                cw,
                chh,
                0,
                0,
                FI_NONE,
                &uv_geom,
                cfg.edge_filter,
                filt_type_uv,
                &mut nb_v,
                &mut v_dc,
            );
            // check_best_indepedant_cfl (product_coding_loop.c:3893): CfL vs
            // the best non-CfL uv, BOTH in the MDS3 SPATIAL SSE domain,
            // priced with `full_lambda_md[hbd_md ? EB_10_BIT_MD :
            // EB_8_BIT_MD]` (:3899). At bd10 C runs the whole arbitration at
            // 10 bits (hbd prediction / residual / full-loop). The port ran
            // it u8-only (this branch was `&& bd10_rd.is_none()`), so no
            // bd10 leaf below p6 could ever pick CfL while C does — the
            // block (16,80) divergence on 1001682 q12 p5.
            // The TABLE-side uv fast rate for the arbitration. C compares
            // against `ctx->best_uv_cost[mode]`, which the independent
            // chroma search built with `svt_aom_get_intra_uv_fast_rate(
            // pcs, ctx, cand_bf, 0)` over its OWN buffers
            // (product_coding_loop.c:7484) — candidates that carry
            // `palette_info == NULL`, so their UV_DC row is priced with
            // `palette_uv_mode_fac_bits[0][0]` (rd_cost.c:514-521).
            // `ind_palette_cost_diff` (:3912-3925) is precisely what
            // converts that [0] row to this candidate's [1] row.
            //
            // `fcr_final` is the CANDIDATE's fast_chroma_rate, and a
            // luma-palette candidate's already carries the [1] row (built
            // at :4596 — C does the same, get_intra_uv_fast_rate sees the
            // real palette_info). Feeding it in here AND adding
            // ind_pal_diff counts the [1]-[0] delta TWICE, which pushed
            // the non-CfL side above CfL on an otherwise-matching block.
            // Rebuild the palette-free row instead — the same expression
            // used for every non-palette candidate's fcr (:4261, :5668,
            // :6849), so this is a no-op wherever no luma palette is in
            // play.
            let fcr_ind = {
                let mut f =
                    rates.uv[cfl_allowed][cand.mode as usize][*uv_mode_final as usize] as u64;
                if use_angle && matches!(*uv_mode_final, 1..=8) {
                    f += rates.angle[*uv_mode_final as usize - 1][(3 + *uv_delta_final) as usize]
                        as u64;
                }
                if *uv_mode_final == 0 {
                    f += pal_uv_no; // rd_cost.c:514 (inside uv fast rate)
                }
                f
            };
            match &bd10_rd {
                None => {
                    let best_uv_cost = rdcost(
                        lambda,
                        u_out.bits as u64 + v_out.bits as u64 + fcr_ind,
                        u_out.dist + v_out.dist,
                    );
                    // Alpha search: md_cfl_rd_pick_alpha (transform domain,
                    // spatial_dist=false internally), same call as M6.
                    let (cfl_idx, cfl_signs, cfl_rd) = md_cfl_rd_pick_alpha(
                        &pred_buf_q3,
                        &u_dc,
                        &v_dc,
                        fx.u_src,
                        fx.v_src,
                        fx.c_stride,
                        c_off,
                        cw,
                        chh,
                        uv_crop,
                        cb_tsc,
                        cb_dsc,
                        cr_tsc,
                        cr_dsc,
                        &qt_u,
                        &qt_v,
                        frame,
                        rates,
                        frame.rdoq,
                        lambda,
                        cand.mode as usize,
                        cfg.cfl_itr_th,
                    );
                    if cfl_rd != MAX_MODE_COST {
                        // cfl_uv_cost: the chosen-alpha CfL chroma TX in the
                        // MDS3 SPATIAL domain + the accurate CfL uv fast rate.
                        let alpha_cb = cfl_idx_to_alpha(cfl_idx, cfl_signs, 0);
                        let alpha_cr = cfl_idx_to_alpha(cfl_idx, cfl_signs, 1);
                        let mut u_cfl = zeroed_pool::<u8>(cw * chh);
                        let mut v_cfl = zeroed_pool::<u8>(cw * chh);
                        svtav1_dsp::intra_pred::cfl_predict_lbd(
                            &pred_buf_q3,
                            &u_dc,
                            cw,
                            &mut u_cfl,
                            cw,
                            alpha_cb,
                            cw,
                            chh,
                        );
                        svtav1_dsp::intra_pred::cfl_predict_lbd(
                            &pred_buf_q3,
                            &v_dc,
                            cw,
                            &mut v_cfl,
                            cw,
                            alpha_cr,
                            cw,
                            chh,
                        );
                        let u_cfl_out = tx_unit(
                            fx.u_src,
                            fx.c_stride,
                            c_off,
                            &u_cfl,
                            cw,
                            0,
                            cw,
                            chh,
                            0,
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
                        let v_cfl_out = tx_unit(
                            fx.v_src,
                            fx.c_stride,
                            c_off,
                            &v_cfl,
                            cw,
                            0,
                            cw,
                            chh,
                            0,
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
                        let cfl_fast_rate = rates.uv[cfl_allowed][cand.mode as usize]
                            [UV_CFL_PRED_IDX] as u64
                            + rates.cfl_alpha_fac_bits[cfl_signs as usize][0]
                                [(cfl_idx >> 4) as usize] as u64
                            + rates.cfl_alpha_fac_bits[cfl_signs as usize][1]
                                [(cfl_idx & 15) as usize] as u64;
                        let cfl_uv_cost = rdcost(
                            lambda,
                            u_cfl_out.bits as u64 + v_cfl_out.bits as u64 + cfl_fast_rate,
                            u_cfl_out.dist + v_cfl_out.dist,
                        );
                        #[cfg(feature = "std")]
                        if crate::dbgenv::nsqdbg() && crate::depth_refine::nsqdbg_here(abs_x, abs_y)
                        {
                            eprintln!(
                                "NSQDBG CFLARB mi=({},{}) {}x{} m={} arb=({},{}) ncb={}+{}+{} ncd={}+{} nc={} cflrd={} idx={} sgn={} cb={}+{}+{} cd={}+{} cfl={} udc={} vdc={}",
                                abs_y / 4,
                                abs_x / 4,
                                w,
                                h,
                                cand.mode,
                                *uv_mode_final,
                                *uv_delta_final,
                                u_out.bits,
                                v_out.bits,
                                fcr_ind,
                                u_out.dist,
                                v_out.dist,
                                best_uv_cost,
                                cfl_rd,
                                cfl_idx,
                                cfl_signs,
                                u_cfl_out.bits,
                                v_cfl_out.bits,
                                cfl_fast_rate,
                                u_cfl_out.dist,
                                v_cfl_out.dist,
                                cfl_uv_cost,
                                u_dc[0],
                                v_dc[0]
                            );
                        }
                        // C `check_best_indepedant_cfl` reverts to non-CfL
                        // iff `best_uv_cost < cfl_uv_cost` (:3927-3928) —
                        // i.e. CfL is KEPT unless strictly beaten, so CfL
                        // wins exact ties (the bd10 arm below always had
                        // this right; the old `cfl < best` here kept
                        // non-CfL on ties — witnessed flipping CID22
                        // 5739122 q5 p0 at mi(31,80) 8x4, where both
                        // sides' terms are identical and nc == cfl ==
                        // 130518 exactly: C codes CfL, the port coded H).
                        //
                        // ind_palette_cost_diff (C :3849-3863): the ind-uv
                        // table priced its UV_DC row with palette_uv_mode
                        // _fac_bits[0][0] (its injected candidates carry
                        // palette_info = NULL), but THIS candidate's coded
                        // DC row pays the [use_palette_y=1][0] context —
                        // add the row delta to the table side of the
                        // compare for a luma-palette candidate. Witnessed:
                        // windows95_p4_q20 mi(40,32) 16x16 pal=6 — all
                        // four coeff/dist terms byte-match C (nc 6916533
                        // vs cfl 6916944) yet C codes CfL because its DC
                        // side carries +[1][0]-[0][0]; the port kept DC.
                        let ind_pal_diff: i64 =
                            if *uv_mode_final == 0 && allow_pal && cand.palette.is_some() {
                                rdcost(lambda, pal_uv_no_y1, 0) as i64
                                    - rdcost(lambda, pal_uv_no, 0) as i64
                            } else {
                                0
                            };
                        let best_uv_adj = (best_uv_cost as i64).saturating_add(ind_pal_diff) as u64;
                        // NOT `best_uv_adj >= cfl_uv_cost` (what clippy <=1.89's
                        // nonminimal_bool asks for; current stable no longer does):
                        // the C predicate at product_coding_loop.c:3928 is
                        // `best_uv_cost + ind_palette_cost_diff < cfl_uv_cost` =
                        // REVERT to non-CfL, and this arm is its negation. The tie
                        // case documented above turned on getting that inversion
                        // exactly right, so the shape stays visible.
                        #[allow(clippy::nonminimal_bool)]
                        if !(best_uv_adj < cfl_uv_cost) {
                            *u_out = u_cfl_out;
                            *v_out = v_cfl_out;
                            *uv_mode_final = UV_CFL_PRED_IDX as u8;
                            *cfl_idx_final = cfl_idx;
                            *cfl_signs_final = cfl_signs;
                            *fcr_final = cfl_fast_rate;
                        }
                    } else {
                        #[cfg(feature = "std")]
                        if crate::dbgenv::nsqdbg() && crate::depth_refine::nsqdbg_here(abs_x, abs_y)
                        {
                            eprintln!(
                                "NSQDBG CFLARB mi=({},{}) {}x{} m={} ALPHA-REJECT",
                                abs_y / 4,
                                abs_x / 4,
                                w,
                                h,
                                cand.mode
                            );
                        }
                    }
                }
                Some(b) => {
                    // bd10 arbitration: 10-bit AC/DC, hbd alpha search, hbd
                    // SPATIAL cfl_uv_cost, all priced with `b.lambda` ==
                    // full_lambda_md[EB_10_BIT_MD]. `best_uv_cost` is the
                    // 10-bit non-CfL uv cost — the same value C's
                    // search_best_mds3_uv_mode stored in best_uv_cost[mode]
                    // (spatial SSE, from `uv_out10`); scope the borrow so
                    // `uv_out10` is free to be replaced on a CfL win.
                    let best_uv_cost = {
                        let (u10b, v10b) = uv_out10.as_ref().unwrap();
                        rdcost(
                            b.lambda,
                            // `fcr_ind`, not `fcr_final` — the table-side
                            // palette-free row; see the bd8 arm's note.
                            u10b.bits as u64 + v10b.bits as u64 + fcr_ind,
                            u10b.dist + v10b.dist,
                        )
                    };
                    // compute_cfl_ac_components at hbd: AC from the winning
                    // 10-bit luma recon + the 10-bit DC chroma base.
                    let mut ac10 =
                        zeroed_pool::<i16>(svtav1_dsp::intra_pred::CFL_BUF_LINE * chh.max(1));
                    cfl_ac_subsample_hbd(
                        fx.y_recon10.as_deref().unwrap(),
                        y_stride,
                        best_recon10,
                        abs_x,
                        abs_y,
                        w,
                        h,
                        &mut ac10,
                    );
                    svtav1_dsp::intra_pred::cfl_subtract_average(&mut ac10, cw, chh);
                    let mut u_dc10 = vec![0u16; cw * chh];
                    let mut v_dc10 = vec![0u16; cw * chh];
                    for (plane_recon, dst) in [
                        (fx.u_recon10.as_deref().unwrap(), &mut u_dc10),
                        (fx.v_recon10.as_deref().unwrap(), &mut v_dc10),
                    ] {
                        predict_unit_hbd(
                            plane_recon,
                            fx.c_stride,
                            ccx,
                            ccy,
                            cw,
                            chh,
                            0,
                            0,
                            FI_NONE,
                            &uv_geom,
                            cfg.edge_filter,
                            filt_type_uv,
                            dst,
                            b.bd,
                        );
                    }
                    // av1_cost_calc_cfl at hbd, TRANSFORM domain (is_full_
                    // loop=0) — the alpha search's per-plane cost.
                    let plane_cost10 = |plane: usize, alpha_q3: i32| -> (u64, i32) {
                        let (src, dc, tsc, dsc, qt) = if plane == 0 {
                            (&b.u_src10, &u_dc10, cb_tsc, cb_dsc, &b.qt_u)
                        } else {
                            (&b.v_src10, &v_dc10, cr_tsc, cr_dsc, &b.qt_v)
                        };
                        let mut cfl_pred = vec![0u16; cw * chh];
                        svtav1_dsp::hbd::cfl_predict_hbd(
                            &ac10,
                            dc,
                            cw,
                            &mut cfl_pred,
                            cw,
                            alpha_q3,
                            b.bd,
                            cw,
                            chh,
                        );
                        let o = tx_unit_hbd(
                            frame.coded_lossless,
                            src,
                            cw,
                            0,
                            &cfl_pred,
                            cw,
                            0,
                            cw,
                            chh,
                            0,
                            1,
                            tsc,
                            dsc,
                            qt,
                            frame.rdoq_level,
                            b.lambda,
                            frame.sharpness,
                            frame.rdoq_allintra_rd_mult,
                            cand.inter.is_some(),
                            rates,
                            do_rdoq,
                            b.bd,
                            qt.qm_level,
                            Some(&TxRdArgs {
                                spatial_dist: false,
                                intra_dir: 0,
                                coeff_rate_est_lvl: cfg.coeff_rate_est_lvl,
                                tx_bias: frame.tx_bias,
                                crop: uv_crop,
                            }),
                        );
                        (o.dist, o.bits)
                    };
                    let (cfl_idx, cfl_signs, cfl_rd) = md_cfl_alpha_search(
                        plane_cost10,
                        rates,
                        b.lambda,
                        cand.mode as usize,
                        cfg.cfl_itr_th,
                    );
                    if cfl_rd != MAX_MODE_COST {
                        let alpha_cb = cfl_idx_to_alpha(cfl_idx, cfl_signs, 0);
                        let alpha_cr = cfl_idx_to_alpha(cfl_idx, cfl_signs, 1);
                        // cfl_uv_cost at 10 bits: the chosen-alpha CfL chroma
                        // re-run in the MDS3 SPATIAL domain (full_loop_uv
                        // is_full_loop=1), matching check_best_indepedant_cfl.
                        let rd10 = TxRdArgs {
                            spatial_dist: true,
                            intra_dir: 0,
                            coeff_rate_est_lvl: cfg.coeff_rate_est_lvl,
                            tx_bias: frame.tx_bias,
                            crop: uv_crop,
                        };
                        let mut u_cfl10 = vec![0u16; cw * chh];
                        let mut v_cfl10 = vec![0u16; cw * chh];
                        svtav1_dsp::hbd::cfl_predict_hbd(
                            &ac10,
                            &u_dc10,
                            cw,
                            &mut u_cfl10,
                            cw,
                            alpha_cb,
                            b.bd,
                            cw,
                            chh,
                        );
                        svtav1_dsp::hbd::cfl_predict_hbd(
                            &ac10,
                            &v_dc10,
                            cw,
                            &mut v_cfl10,
                            cw,
                            alpha_cr,
                            b.bd,
                            cw,
                            chh,
                        );
                        let u10 = tx_unit_hbd(
                            frame.coded_lossless,
                            &b.u_src10,
                            cw,
                            0,
                            &u_cfl10,
                            cw,
                            0,
                            cw,
                            chh,
                            0,
                            1,
                            cb_tsc,
                            cb_dsc,
                            &b.qt_u,
                            frame.rdoq_level,
                            b.lambda,
                            frame.sharpness,
                            frame.rdoq_allintra_rd_mult,
                            cand.inter.is_some(),
                            rates,
                            do_rdoq,
                            b.bd,
                            b.qt_u.qm_level,
                            Some(&rd10),
                        );
                        let v10 = tx_unit_hbd(
                            frame.coded_lossless,
                            &b.v_src10,
                            cw,
                            0,
                            &v_cfl10,
                            cw,
                            0,
                            cw,
                            chh,
                            0,
                            1,
                            cr_tsc,
                            cr_dsc,
                            &b.qt_v,
                            frame.rdoq_level,
                            b.lambda,
                            frame.sharpness,
                            frame.rdoq_allintra_rd_mult,
                            cand.inter.is_some(),
                            rates,
                            do_rdoq,
                            b.bd,
                            b.qt_v.qm_level,
                            Some(&rd10),
                        );
                        let cfl_fast_rate = rates.uv[cfl_allowed][cand.mode as usize]
                            [UV_CFL_PRED_IDX] as u64
                            + rates.cfl_alpha_fac_bits[cfl_signs as usize][0]
                                [(cfl_idx >> 4) as usize] as u64
                            + rates.cfl_alpha_fac_bits[cfl_signs as usize][1]
                                [(cfl_idx & 15) as usize] as u64;
                        let cfl_uv_cost = rdcost(
                            b.lambda,
                            u10.bits as u64 + v10.bits as u64 + cfl_fast_rate,
                            u10.dist + v10.dist,
                        );
                        // C `check_best_indepedant_cfl` reverts to non-CfL iff
                        // `best_uv_cost < cfl_uv_cost` (:3927) — i.e. CfL is
                        // KEPT unless strictly beaten, so CfL wins exact ties.
                        // ind_palette_cost_diff (:3849-3863) — see the bd8
                        // arm above: a luma-palette candidate's DC row pays
                        // the [1][0] palette-flag context the table priced
                        // as [0][0]; priced with this arm's 10-bit lambda.
                        let ind_pal_diff: i64 =
                            if *uv_mode_final == 0 && allow_pal && cand.palette.is_some() {
                                rdcost(b.lambda, pal_uv_no_y1, 0) as i64
                                    - rdcost(b.lambda, pal_uv_no, 0) as i64
                            } else {
                                0
                            };
                        let best_uv_adj = (best_uv_cost as i64).saturating_add(ind_pal_diff) as u64;
                        // NOT `best_uv_adj >= cfl_uv_cost` (what clippy <=1.89's
                        // nonminimal_bool asks for; current stable no longer does):
                        // the C predicate at product_coding_loop.c:3928 is
                        // `best_uv_cost + ind_palette_cost_diff < cfl_uv_cost` =
                        // REVERT to non-CfL, and this arm is its negation. The tie
                        // case documented above turned on getting that inversion
                        // exactly right, so the shape stays visible.
                        #[allow(clippy::nonminimal_bool)]
                        if !(best_uv_adj < cfl_uv_cost) {
                            // u8 chroma canvas follows the decision (the
                            // pre-filter searches read it at bd10).
                            let mut u_cfl = zeroed_pool::<u8>(cw * chh);
                            let mut v_cfl = zeroed_pool::<u8>(cw * chh);
                            svtav1_dsp::intra_pred::cfl_predict_lbd(
                                &pred_buf_q3,
                                &u_dc,
                                cw,
                                &mut u_cfl,
                                cw,
                                alpha_cb,
                                cw,
                                chh,
                            );
                            svtav1_dsp::intra_pred::cfl_predict_lbd(
                                &pred_buf_q3,
                                &v_dc,
                                cw,
                                &mut v_cfl,
                                cw,
                                alpha_cr,
                                cw,
                                chh,
                            );
                            *u_out = tx_unit(
                                fx.u_src,
                                fx.c_stride,
                                c_off,
                                &u_cfl,
                                cw,
                                0,
                                cw,
                                chh,
                                0,
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
                            *v_out = tx_unit(
                                fx.v_src,
                                fx.c_stride,
                                c_off,
                                &v_cfl,
                                cw,
                                0,
                                cw,
                                chh,
                                0,
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
                            *uv_out10 = Some((u10, v10));
                            *uv_mode_final = UV_CFL_PRED_IDX as u8;
                            *cfl_idx_final = cfl_idx;
                            *cfl_signs_final = cfl_signs;
                            *fcr_final = cfl_fast_rate;
                        }
                    }
                }
            }
        }
    }
}
