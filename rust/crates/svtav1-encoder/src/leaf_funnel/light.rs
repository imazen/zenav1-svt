//! The Light-PD1 leaf evaluator — C `md_encode_block_light_pd1`
//! (product_coding_loop.c:9037).
//!
//! When the per-superblock `pd1_level` resolves above `REGULAR_PD1` the
//! regular NIC/MDS1/MDS3 funnel is replaced wholesale by the light lane:
//!
//!   * `generate_md_stage_0_cand_light_pd1` (mode_decision.c:3526) — the
//!     candidate set is the STRICT subset
//!     [`crate::port_md::inject::inject_inter_candidates_light_pd1`] emits
//!     (MVP + ME-NEWMV only) plus `inject_intra_candidates`'s regular intra
//!     set under `dc_cand_only`.
//!   * `md_stage_0_light_pd1` (product_coding_loop.c:1525) — ONE fast pass
//!     that scores every candidate's `variance << 4` against the source and
//!     keeps a single best index (C ping-pongs two candidate buffers; the
//!     port keeps the winner's index). There is no MDS1 and no
//!     multi-candidate MDS3.
//!   * `md_stage_3_light_pd1` (:7144) -> `full_loop_core_light_pd1` (:6541)
//!     on that ONE winner — `lpd1_should_perform_tx`, the freq-domain
//!     DCT_DCT transform (`perform_dct_dct_tx_light_pd1`, :5434), the
//!     luma-skip-RD and chroma-complexity gates, and `svt_aom_full_cost`'s
//!     inter skip decision — all in the coefficient-distortion domain the
//!     regular MDS3 does NOT use.
//!
//! This module is reached only from [`super::evaluate_leaf`] when
//! [`FunnelCtx::lpd1`] is `Some`; on every other frame the field is `None`
//! and this file is dead code. It is deliberately a parallel evaluator, not
//! a branch inside the regular one, because the cost domains differ
//! throughout (freq-domain SSD + flat `6000+eob*1000` bits here, versus
//! spatial SSE + real coeff bits there) and interleaving them produced the
//! `docs/INTER-ENCODE-PLAN.md` divergences this lane exists to close.

use super::*;
use crate::port_enc_mode_config::light_pd1::LightPd1Signals;
use crate::port_md::lpd1::{
    BlockPlanes, ComponentType, Neighbours, Plane, SkipLumaOutcome, SkipRdInputs, UvGeom,
    blk_skip_luma_rd, chroma_complexity_check, should_perform_tx,
};
use crate::port_md::lpd1_loop::MAX_MODE_COST;

/// Per-superblock Light-PD1 context — everything `evaluate_leaf`'s light
/// lane reads that is not already on [`FunnelCtx`]. Built once per SB by the
/// pipeline's video arm when the resolved `pd1_level` is above
/// `REGULAR_PD1`.
#[derive(Clone)]
pub(crate) struct Lpd1Leaf {
    /// `svt_aom_sig_deriv_enc_dec_light_pd1_default`'s output for this SB.
    /// (`ctx->lpd1_ctrls.pd1_level` is the value the resolver checked `>
    /// REGULAR_PD1` before constructing this — `sig` encodes the effective
    /// controls, so it is not repeated here.)
    pub sig: LightPd1Signals,
    /// C `ctx->full_lambda_md[EB_8_BIT_MD]` — the FULL mode-decision lambda
    /// (the light path never uses `fast_lambda_md`).
    pub lambda: u64,
    /// C `ctx->qp_index` — `lpd1_should_perform_tx`'s QP bias + the
    /// `th_normalizer` multiplier.
    pub qp_index: u32,
    /// C `ctx->nic_pruning_ctrls.merge_inter_cands_mult` — read by
    /// `generate_md_stage_0_cand_light_pd1` for `merge_inter_cands`; carried
    /// so `build_inter_candidates` keeps C's candidate-class stamp (inert in
    /// the light MDS0, which pools all classes, but priced identically).
    pub merge_inter_cands_mult: u8,
}

/// C `lpd1_chroma_complexity_check` -> `COMPONENT_TYPE`.
type ChromaComp = ComponentType;

/// `ctx->rdoq_ctrls` as `md_stage_3_light_pd1` leaves it
/// (product_coding_loop.c:7147-7151): the per-SB `sig.rdoq` row with
/// `skip_uv`/`dct_dct_only` CLEARED when EncDec is bypassed (which it is at
/// every preset the light lane runs — `get_bypass_encdec_default` =
/// `enc_mode > M2`), leaving `eob_th`/`eob_fast_th`/`cut_off` live.
fn light_rdoq(fx: &FunnelCtx<'_>) -> RdoqCtrls {
    let l = fx.lpd1.as_ref().expect("light lane");
    let mut r = l.sig.rdoq;
    r.clear_when_bypassed(fx.frame.cfg.bypass_encdec);
    r
}

/// The winner's light full-loop output, folded into a [`Cand`]'s MDS3
/// fields by [`finish_lpd1`].
struct LightFull {
    /// `y_full_distortion[DIST_SSD][0]` — freq-domain RESIDUAL distortion
    /// (post skip-decision, so the PREDICTION value when skip won).
    y_res: u64,
    /// `block_has_coeff` AFTER the `svt_aom_full_cost` skip arms.
    block_has_coeff: bool,
    /// `cand_bf->full_cost` / `total_rate` / `full_dist`.
    cost: u64,
    total_rate: u64,
    full_dist: u64,
}

/// The light fast-cost for one candidate — C `fast_loop_core_light_pd1`'s
/// tail (:1057-1065): `av1_product_fast_cost_func_table[is_inter]`.
///
/// Intra is `svt_aom_intra_fast_cost(full_lambda, variance << 4)` =
/// `rdcost(lambda, flr + fcr, dist)`. Inter is `inter_fast_cost_light`'s
/// `finish` arm: `skip_mode_rate` arbitrated against `luma_rate` on RATE,
/// then `rdcost(lambda, chosen_rate, dist)`.
fn light_fast_cost(c: &Cand, lambda: u64, luma_fast_dist: u64, fx: &FunnelCtx<'_>) -> u64 {
    let d = luma_fast_dist << 4;
    match c.inter.as_deref() {
        Some(ic) => {
            // C `av1_inter_fast_cost_light` -> `finish`: the skip-mode rate
            // is arbitrated against `luma_rate` on RATE alone, then the cost
            // is `rdcost(lambda, min_rate, dist)`.
            let luma_rate = c.flr;
            let skip_mode_rate = if ic.skip_mode_allowed {
                fx.inter
                    .expect("an inter candidate implies inter frame state")
                    .fac
                    .skip_mode[ic.skip_mode_ctx as usize][1] as u64
            } else {
                0
            };
            let rate = if ic.skip_mode_allowed && skip_mode_rate < luma_rate {
                skip_mode_rate
            } else {
                luma_rate
            };
            rdcost(lambda, rate, d)
        }
        None => rdcost(lambda, c.flr + c.fcr, d),
    }
}

/// C `md_stage_0_light_pd1` (:1525-1555): one fast pass over every
/// candidate, tracking the running best. Returns the winner's index.
///
/// `fast_cost` on each [`Cand`] is rewritten to the light value so the
/// `Cand` that becomes `win` carries the number C committed.
fn light_mds0(
    cands: &mut [Cand],
    fx: &FunnelCtx<'_>,
    g: &LeafGeom,
    lambda: u64,
    y_src: &[u8],
    y_src_stride: usize,
    y_src_off: usize,
) -> usize {
    let sig = &fx.lpd1.as_ref().expect("light lane").sig;
    let elim = &sig.cand_reduction.cand_elimination_ctrls;
    let (w, h) = (g.w, g.h);
    let area = (w * h) as u64;
    let mut best_cost: u64 = u64::MAX;
    let mut best_idx = usize::MAX;
    let mut best_dist: u64 = 0; // the best candidate's luma_fast_dist
    for (i, c) in cands.iter_mut().enumerate() {
        // `fn_ptr->vf(pred, src)` — `luma_fast_dist` is the un-shifted
        // variance.
        let var = u64::from(svtav1_dsp::variance::variance_diff(
            &c.pred,
            w,
            &y_src[y_src_off..],
            y_src_stride,
            w,
            h,
        ));
        // :1017-1030 — intra-mode cand_elimination pre-check against the
        // running best's distortion (only when a best already exists).
        if best_cost != u64::MAX && c.inter.is_none() && c.ibc.is_none() && elim.enabled != 0 {
            let th = u64::from(if c.mode == 0 {
                // DC_PRED — the harsher `skip_dc_th`.
                elim.skip_dc_th
            } else {
                elim.dc_only_th
            }) * area;
            if best_dist < th {
                c.fast_cost = MAX_MODE_COST;
                continue;
            }
        }
        // :1049-1055 — the distortion-only early-out.
        if best_cost != u64::MAX {
            let dcost = rdcost(lambda, 0, var << 4);
            if dcost > best_cost {
                c.fast_cost = MAX_MODE_COST;
                continue;
            }
        }
        let fast = light_fast_cost(c, lambda, var, fx);
        c.fast_cost = fast;
        if fast < best_cost {
            best_cost = fast;
            best_idx = i;
            best_dist = var;
        }
    }
    best_idx
}

/// `lpd1_should_perform_tx`'s neighbour read (`:6352-6374`): the mi cells
/// above and left of THIS block, gated on `left_available && up_available`.
/// `None` (C skips the whole bonus) when either neighbour is off-grid.
/// The grid is C's `mi_grid_base` as MD stamps it — [`FunnelCtx::ibc_mvp`].
fn lpd1_neighbours(fx: &FunnelCtx<'_>, g: &LeafGeom) -> Option<Neighbours> {
    let (grid, mi_cols, tile) = match (fx.ibc_mvp.as_deref(), fx.inter) {
        (Some(grid), Some(inter)) => (grid, inter.mi_cols, inter.tile),
        _ => return None,
    };
    let nb = crate::inter_md_arm::neighbors_from_grid(
        grid,
        mi_cols,
        (g.abs_y / 4) as i32,
        (g.abs_x / 4) as i32,
        tile,
    );
    let (above, left) = (nb.above, nb.left);
    if !(nb.up_available && nb.left_available) {
        return None;
    }
    let nearest = |m: u8| {
        m == svtav1_types::prediction::PredictionMode::NearestMv as u8
            || m == svtav1_types::prediction::PredictionMode::NearestNearestMv as u8
    };
    Some(Neighbours {
        both_skip: above.is_some_and(|a| a.skip) && left.is_some_and(|l| l.skip),
        both_nearest: above.is_some_and(|a| nearest(a.mode))
            && left.is_some_and(|l| nearest(l.mode)),
    })
}

/// The luma leg of `full_loop_core_light_pd1` (`lpd1_should_perform_tx` +
/// `perform_dct_dct_tx_light_pd1`), writing the freq-domain
/// `y_full_distortion` pair and `y_coeff_bits` and the candidate's
/// `qcoeff`/`eob`/`recon`/`txb_type`/`y_has_coeff`.
///
/// Returns `(y_residual_dist, y_prediction_dist, y_coeff_bits, y_has_coeff)`.
/// `pred` is the candidate's whole-block luma prediction (`cand.pred`).
#[allow(clippy::too_many_arguments)]
fn light_luma_tx(
    fx: &FunnelCtx<'_>,
    g: &LeafGeom,
    qt: &QuantTable,
    y_src: &[u8],
    y_src_stride: usize,
    y_src_off: usize,
    cand: &mut Cand,
    luma_fast_dist: u64,
    full_dist: u64,
    lvl: u8,
    l: &Lpd1Leaf,
) -> (u64, u64, u64, bool) {
    let (w, h) = (g.w, g.h);
    // `lpd1_should_perform_tx` (:6329). `is_luma_dominant_input` is
    // `input_is_luma_dominant` — intra inputs are luma-dominant; inter
    // (predicted) inputs are not (C :6366 reads the same flag).
    let perform_tx = should_perform_tx(
        cand.is_inter(),
        &l.sig.lpd1_tx_skip_decision,
        luma_fast_dist,
        w,
        h,
        l.qp_index,
        lpd1_neighbours(fx, g),
        SkipRdInputs {
            full_lambda: l.lambda,
            fast_luma_rate: cand.flr,
            skip_fac_bits_coded: fx.rates.skip[g.skip_ctx][0] as u64,
            skip_fac_bits_skip: fx.rates.skip[g.skip_ctx][1] as u64,
            full_dist,
        },
        !cand.is_inter(),
    );
    let luma_eob_zero = |cand: &mut Cand| {
        cand.txb_eob.clear();
        cand.txb_eob.push(0);
        cand.txb_type.clear();
        cand.txb_type.push(cc::DCT_DCT as u8);
        cand.txb_q.clear();
        cand.txb_q.push(crate::vecpool::zeroed_pool::<i32>(w * h));
        cand.y_recon = crate::vecpool::PoolVec::from_slice(&cand.pred);
        cand.y_recon_d0 = crate::vecpool::PoolVec::from_slice(&cand.pred);
    };
    if !perform_tx {
        // :6573-6585 — no TX: dists 0, bits 6000, DCT_DCT, no coeff.
        luma_eob_zero(cand);
        return (0, 0, 6000, false);
    }
    // `perform_dct_dct_tx_light_pd1`: DCT_DCT, freq-domain (`spatial_dist =
    // false`), `need_recon`, the light coefficient-rate tier. `intra_dir`
    // selects the coeff-rate DC/tx-type row: the inter sentinel for inter,
    // the (filter-intra-mapped) intra mode for intra.
    let intra_dir = if cand.inter.is_some() {
        INTER_TXT_DIR
    } else if cand.ibc.is_some() {
        IBC_TXT_DIR
    } else if cand.fi != FI_NONE {
        FIMODE_TO_INTRADIR[cand.fi as usize] as usize
    } else {
        cand.mode as usize
    };
    let out = tx_pipeline::tx_unit(
        y_src,
        y_src_stride,
        y_src_off,
        &cand.pred,
        w,
        0,
        w,
        h,
        cc::DCT_DCT,
        0,
        0,
        0,
        intra_dir,
        qt,
        fx.frame,
        fx.rates,
        // `ctx->rdoq_ctrls` on this lane — `md_stage_3_light_pd1`'s per-SB
        // row (`mds_do_rdoq = true`, :7153), NOT `frame.rdoq`.
        light_rdoq(fx),
        false,
        g.blk_crop,
        true,
        tx_pipeline::RateMode::LightPd1(lvl),
    );
    if crate::port_md::lpd1_loop::luma_eob_zero_takes_the_early_exit(out.eob, lvl) {
        // :5495-5507 — dists 0, bits 6000, DCT_DCT, inter UV forced DCT_DCT.
        luma_eob_zero(cand);
        return (0, 0, 6000, false);
    }
    let eob = out.eob;
    cand.txb_eob.clear();
    cand.txb_eob.push(eob);
    cand.txb_type.clear();
    cand.txb_type.push(cc::DCT_DCT as u8);
    cand.txb_q.clear();
    cand.txb_q.push(out.qcoeff);
    cand.y_recon = out.recon;
    cand.y_recon_d0 = crate::vecpool::PoolVec::from_slice(&cand.y_recon);
    (out.dist, out.dist_pred, out.bits as u64, eob > 0)
}

/// The chroma prediction the light path residuals the source against —
/// `inter.u_pred`/`v_pred` (the MC prediction already on the candidate) for
/// inter, `predict_unit(uv)` for intra. Also the committed chroma recon for
/// a plane the chroma leg skipped: a coefficient-free plane reconstructs to
/// its prediction.
fn light_chroma_pred(
    fx: &FunnelCtx<'_>,
    cx: &chroma::ChromaCtx,
    cand: &Cand,
    u: bool,
) -> alloc::vec::Vec<u8> {
    match cand.inter.as_deref() {
        Some(ic) => {
            if u {
                ic.u_pred.clone()
            } else {
                ic.v_pred.clone()
            }
        }
        None => {
            let (cw, chh, ccx, ccy) = (cx.cw, cx.chh, cx.ccx, cx.ccy);
            let mut p = alloc::vec![0u8; cw * chh];
            predict_unit(
                if u { &fx.u_recon[..] } else { &fx.v_recon[..] },
                fx.c_stride,
                ccx,
                ccy,
                cw,
                chh,
                cand.uv,
                cand.uv_delta,
                FI_NONE,
                &cx.uv_geom,
                fx.frame.cfg.edge_filter,
                cx.filt_type_uv,
                &mut None,
                &mut p,
            );
            p
        }
    }
}

/// `(res_dist, pred_dist, bits, eob)` for one chroma plane's light TX.
type ChromaTxOut = (u64, u64, u64, u16);

/// The chroma leg of `full_loop_core_light_pd1`
/// (`svt_aom_full_loop_chroma_light_pd1`, full_loop.c:1983) for the planes
/// `component` selects. Freq-domain (`spatial_dist = false`), `DCT_DCT` for
/// inter / `uv_tx_type(uv)` for intra, the light coefficient tier,
/// `need_recon`.
///
/// Returns `(cb, cr)` as [`ChromaTxOut`] pairs; the untouched side is
/// all-zero.
#[allow(clippy::too_many_arguments)]
fn light_chroma_tx(
    fx: &FunnelCtx<'_>,
    cx: &chroma::ChromaCtx,
    cand: &mut Cand,
    comp: ChromaComp,
    lvl: u8,
) -> (ChromaTxOut, ChromaTxOut) {
    let (cw, chh, ccx, ccy) = (cx.cw, cx.chh, cx.ccx, cx.ccy);
    // `perform_dct_dct_tx_light_pd1` residuals the SOURCE chroma against the
    // prediction.
    let mk_pred =
        |cand: &Cand, u: bool| -> alloc::vec::Vec<u8> { light_chroma_pred(fx, cx, cand, u) };
    let run =
        |src: &[u8], tsc: usize, dsc: usize, qt: &QuantTable, pred: &[u8], intra_dir: usize| {
            tx_pipeline::tx_unit(
                src,
                fx.c_stride,
                ccy * fx.c_stride + ccx,
                pred,
                cw,
                0,
                cw,
                chh,
                // `cand->transform_type_uv`: DCT_DCT for inter, the intra UV tx
                // type for intra (the light path runs no chroma tx search).
                if cand.inter.is_some() {
                    cc::DCT_DCT
                } else {
                    crate::leaf_funnel::tx_geom::uv_tx_type(cand.uv, cw, chh)
                },
                1,
                tsc,
                dsc,
                intra_dir,
                qt,
                fx.frame,
                fx.rates,
                cx.rdoq,
                false,
                cx.uv_crop,
                true,
                tx_pipeline::RateMode::LightPd1(lvl),
            )
        };
    // `COMPONENT_CHROMA` runs both; `_CB`/`_CR` run only theirs (C's
    // `component_type` dispatch at :2019/:2109).
    let cb = if matches!(comp, ChromaComp::Chroma | ChromaComp::Cb) {
        let pred = mk_pred(&*cand, true);
        let o = run(
            fx.u_src,
            cx.cb_tsc,
            cx.cb_dsc,
            &cx.qt_u,
            &pred,
            cand.uv as usize,
        );
        cand.u_eob = o.eob;
        cand.u_recon = o.recon;
        cand.u_q = o.qcoeff;
        (o.dist, o.dist_pred, o.bits as u64, o.eob)
    } else {
        cand.u_eob = 0;
        (0, 0, 0, 0)
    };
    let cr = if matches!(comp, ChromaComp::Chroma | ChromaComp::Cr) {
        let pred = mk_pred(&*cand, false);
        let o = run(
            fx.v_src,
            cx.cr_tsc,
            cx.cr_dsc,
            &cx.qt_v,
            &pred,
            cand.uv as usize,
        );
        cand.v_eob = o.eob;
        cand.v_recon = o.recon;
        cand.v_q = o.qcoeff;
        (o.dist, o.dist_pred, o.bits as u64, o.eob)
    } else {
        cand.v_eob = 0;
        (0, 0, 0, 0)
    };
    (cb, cr)
}

/// C `svt_aom_full_cost` (rd_cost.c:1349) restricted to the light path's
/// inter arms — `blk_skip_decision` (:1371-1423) and the `skip_mode`
/// arbitration (:1440-1474) — over the freq-domain distortions the light
/// full-loop produced. Mirrors the regular path's spelling in `mds3.rs`.
#[allow(clippy::too_many_arguments)]
fn light_full_cost(
    fx: &FunnelCtx<'_>,
    g: &LeafGeom,
    lambda: u64,
    cand: &mut Cand,
    mut y: (u64, u64),
    cb: (u64, u64),
    cr: (u64, u64),
    y_bits: u64,
    cb_bits: u64,
    cr_bits: u64,
    mut block_has_coeff: bool,
) -> LightFull {
    let skip_ctx = g.skip_ctx;
    let sf = &fx.rates.skip[skip_ctx];
    let is_inter = cand.inter.is_some();
    // `non_skip_tx_size_bits` — C gates it on `tx_mode == TX_MODE_SELECT` &&
    // `block_has_coeff`; the inter var-tx `tx_size` coding is what
    // `block_signals_txsize` + `tx_size_bits_vartx` produce at `tx_depth 0`.
    // `skip_tx_size_bits` is 0 for every inter mode (rd_cost.c:1369).
    let nstx = if block_signals_txsize(g.w, g.h) && !fx.frame.coded_lossless && block_has_coeff {
        crate::vartx::tx_size_bits_vartx(
            &fx.rates.txfm_partition_fac_bits,
            fx.ectx.txfm_above_span(g.abs_x, g.w),
            fx.ectx.txfm_left_span(g.abs_y, g.h),
            g.w,
            g.h,
            0, // tx_depth is always 0 on the light path
            g.abs_y,
            fx.frame.frame_h_px,
        )
    } else {
        0
    };
    // `blk_skip_decision` — gated on `block_has_coeff && is_inter` and the
    // light path's `ctx->blk_skip_decision` (always true here).
    let mut skip = false;
    if !fx.frame.coded_lossless && block_has_coeff && is_inter {
        let nsc = rdcost(
            lambda,
            y_bits + cb_bits + cr_bits + nstx + sf[0] as u64,
            y.0 + cb.0 + cr.0,
        );
        let sc = rdcost(lambda, sf[1] as u64, y.1 + cb.1 + cr.1);
        if crate::dbgenv::skipdbg() {
            eprintln!(
                "LSKIP blk=({},{}) yb={y_bits} cbb={cb_bits} crb={cr_bits} nstx={nstx} \
                 sf0={} sf1={} yres={} ypred={} nsc={nsc} sc={sc} lam={lambda} -> {}",
                g.abs_x,
                g.abs_y,
                sf[0],
                sf[1],
                y.0,
                y.1,
                if sc < nsc { "SKIP" } else { "KEEP" },
            );
        }
        if sc < nsc {
            y.0 = y.1;
            block_has_coeff = false;
            skip = true;
        }
    }
    let _ = skip;
    let coeff_rate = if block_has_coeff {
        y_bits + cb_bits + cr_bits + nstx + sf[0] as u64
    } else {
        sf[1] as u64
    };
    let mut mode_rate = cand.flr + cand.fcr + coeff_rate;
    let mut mode_dist = y.0 + cb.0 + cr.0;
    let mut cost = rdcost(lambda, mode_rate, mode_dist);
    // `skip_mode` arbitration (:1440-1474) — `<=` on a tie.
    if let Some(ic) = cand.inter.as_deref()
        && ic.skip_mode_allowed
    {
        let sm_rate = fx
            .inter
            .expect("inter candidate implies inter frame state")
            .fac
            .skip_mode[ic.skip_mode_ctx as usize][1] as u64;
        let sm_dist = y.1 + cb.1 + cr.1;
        let sm_cost = rdcost(lambda, sm_rate, sm_dist);
        if sm_cost <= cost {
            cost = sm_cost;
            mode_rate = sm_rate;
            mode_dist = sm_dist;
            block_has_coeff = false;
            cand.inter.as_deref_mut().unwrap().skip_mode = true;
        }
    }
    LightFull {
        y_res: y.0,
        block_has_coeff,
        cost,
        total_rate: mode_rate,
        full_dist: mode_dist,
    }
}

/// C `md_encode_block_light_pd1`'s MDS0 + MDS3 halves, run on the candidate
/// set [`super::inject::inject_candidates`] already produced (with the light
/// inter injectors — see [`Lpd1Leaf`]). Returns the decided [`LeafEval`].
#[allow(clippy::too_many_arguments)]
pub(super) fn finish_lpd1(
    fx: &mut FunnelCtx<'_>,
    g: &LeafGeom,
    cx: &chroma::ChromaCtx,
    qt: &QuantTable,
    _lambda: u64,
    mut cands: Vec<Cand>,
    y_src: &[u8],
    y_src_stride: usize,
    y_src_off: usize,
) -> LeafEval {
    let (w, h) = (g.w, g.h);
    let lsig = fx.lpd1.as_ref().expect("light lane").clone();
    // The light path prices everything at `full_lambda_md[EB_8_BIT_MD]`
    // (`lsig.lambda`, the PER-SB value), not the frame-level `lambda` the
    // regular funnel reads — under a variance-boost qindex plan they differ.
    let lambda = lsig.lambda;
    let lvl = lsig.sig.rate_est.coeff_rate_est_lvl;

    // ---- C `md_stage_0_light_pd1` ----
    let win = light_mds0(
        &mut cands,
        fx,
        g,
        lsig.lambda,
        y_src,
        y_src_stride,
        y_src_off,
    );
    assert!(win != usize::MAX, "light-PD1 leaf produced no candidate");
    // The winner's `luma_fast_dist`/`full_dist` feed the TX gates
    // (`lpd1_should_perform_tx` reads both).
    let (luma_fast_dist, full_dist) = {
        let c = &cands[win];
        (
            u64::from(svtav1_dsp::variance::variance_diff(
                &c.pred,
                w,
                &y_src[y_src_off..],
                y_src_stride,
                w,
                h,
            )),
            svtav1_dsp::variance::sse(&y_src[y_src_off..], y_src_stride, &c.pred, w, w, h),
        )
    };

    // ---- C `full_loop_core_light_pd1` (:6541) on the single winner ----
    let cand = &mut cands[win];
    let (y_res, y_pred, y_bits, y_has_coeff) = light_luma_tx(
        fx,
        g,
        qt,
        y_src,
        y_src_stride,
        y_src_off,
        cand,
        luma_fast_dist,
        full_dist,
        lvl,
        &lsig,
    );
    let mut block_has_coeff = y_has_coeff;

    // `lpd1_blk_skip_luma_rd` (:6593-6598) — only when the pct gate is armed
    // (0 at LPD1_L0..=L2). A committed luma skip clears the luma coeff and
    // forces chroma off.
    let luma_skip_committed = lsig.sig.lpd1_blk_skip_luma_rd_pct != 0
        && block_has_coeff
        && cand.inter.is_some()
        && matches!(
            blk_skip_luma_rd(
                lsig.lambda,
                y_bits,
                fx.rates.skip[g.skip_ctx][0] as u64,
                fx.rates.skip[g.skip_ctx][1] as u64,
                y_res,
                y_pred,
                u64::from(lsig.sig.lpd1_blk_skip_luma_rd_pct),
            ),
            SkipLumaOutcome::CommitSkip
        );
    if luma_skip_committed {
        block_has_coeff = false;
    }

    // `plan_chroma` folds `:6587-6631` — `perform_chroma`, the
    // `chroma_complexity_check` detector and the per-plane zeroing.
    let plan = crate::port_md::lpd1_loop::plan_chroma(
        block_has_coeff,
        lsig.sig.lpd1_tx.zero_y_coeff_exit != 0,
        luma_skip_committed,
        lsig.sig.lpd1_tx.chroma_detector_level,
        // `recon_needed` — `svt_aom_do_md_recon`; the light lane always
        // reconstructs the committed block for the next one's neighbours.
        true,
        || {
            // `chroma_complexity_check` (:6143): source block vs the MC
            // reference (inter) or the variance arm (intra).
            let geom = UvGeom {
                bwidth_uv: cx.cw,
                bheight_uv: cx.chh,
                bsize_uv: crate::entropy::context::block_size_index(cx.cw, cx.chh),
            };
            let input = BlockPlanes {
                y: Plane {
                    data: &y_src[y_src_off..],
                    stride: y_src_stride,
                },
                u: Plane {
                    data: &fx.u_src[cx.ccy * fx.c_stride + cx.ccx..],
                    stride: fx.c_stride,
                },
                v: Plane {
                    data: &fx.v_src[cx.ccy * fx.c_stride + cx.ccx..],
                    stride: fx.c_stride,
                },
            };
            let reference = cand.inter.as_deref().map(|ic| BlockPlanes {
                y: Plane {
                    data: &cand.pred,
                    stride: w,
                },
                u: Plane {
                    data: &ic.u_pred,
                    stride: cx.cw,
                },
                v: Plane {
                    data: &ic.v_pred,
                    stride: cx.cw,
                },
            });
            chroma_complexity_check(
                cand.inter.is_some(),
                lsig.sig.lpd1_tx.chroma_detector_level,
                geom,
                input,
                reference,
            )
        },
    );
    let perform_chroma = g.has_uv && plan.perform_chroma;
    let chroma_component = if g.has_uv {
        plan.chroma_component
    } else {
        ChromaComp::Luma
    };

    // Chroma TX + `svt_aom_full_cost`, or the no-chroma epilogue.
    let full = if perform_chroma {
        let (cb, cr) = light_chroma_tx(fx, cx, cand, chroma_component, lvl);
        block_has_coeff = y_has_coeff || cb.3 > 0 || cr.3 > 0;
        light_full_cost(
            fx,
            g,
            lambda,
            cand,
            (y_res, y_pred),
            (cb.0, cb.1),
            (cr.0, cr.1),
            y_bits,
            cb.2,
            cr.2,
            block_has_coeff,
        )
    } else {
        // :6694-6704 — no chroma TX and no `svt_aom_full_cost`: `u_has_coeff
        // = v_has_coeff = 0`, and a `skip_mode_allowed` candidate goes
        // straight to `skip_mode = true`. `block_has_coeff` stays at the
        // post-skip-RD value (always 0 here — both no-chroma arms leave the
        // block coefficient-free), so the committed block is SKIP.
        cand.u_eob = 0;
        cand.v_eob = 0;
        if cand.inter.as_deref().is_some_and(|i| i.skip_mode_allowed) {
            cand.inter.as_deref_mut().unwrap().skip_mode = true;
        }
        LightFull {
            y_res,
            block_has_coeff,
            // `full_cost`/`total_rate`/`full_dist` are not refreshed on this
            // arm (C leaves them at their MDS0 values); `blk_ptr->cost` is
            // unused by the fixed partition walk, so the skip cost is the
            // closest faithful value for the funnel's bookkeeping.
            cost: rdcost(lambda, fx.rates.skip[g.skip_ctx][1] as u64, y_pred),
            total_rate: fx.rates.skip[g.skip_ctx][1] as u64,
            full_dist: y_pred,
        }
    };

    // Fold the light full-loop result into the winner's MDS3 fields — the
    // same names `commit_leaf` and the entropy walk read.
    let cand = &mut cands[win];
    cand.mds3_cost = full.cost;
    cand.full_cost = full.cost;
    cand.total_rate = full.total_rate;
    cand.full_dist = full.full_dist;
    cand.block_has_coeff = full.block_has_coeff;
    cand.y_bits = 0;
    cand.y_dist = full.y_res;
    cand.tx_depth = 0;
    if !full.block_has_coeff {
        // skip / skip_mode: no coefficients, DCT_DCT, recon = prediction.
        // `txb_q` keeps the depth-0 SHAPE the commit path expects — one
        // zeroed w*h raster, exactly as the regular path's skip fold writes
        // it (mds3.rs) — a bare `[0]` would panic the depth-0 unpack.
        cand.txb_eob.clear();
        cand.txb_eob.push(0);
        cand.txb_type.clear();
        cand.txb_type.push(cc::DCT_DCT as u8);
        cand.txb_q.clear();
        cand.txb_q.push(crate::vecpool::zeroed_pool::<i32>(w * h));
        cand.u_eob = 0;
        cand.v_eob = 0;
        cand.u_q = crate::vecpool::zeroed_pool::<i32>(cx.cw * cx.chh);
        cand.v_q = crate::vecpool::zeroed_pool::<i32>(cx.cw * cx.chh);
        cand.y_recon = crate::vecpool::PoolVec::from_slice(&cand.pred);
        cand.y_recon_d0 = crate::vecpool::PoolVec::from_slice(&cand.pred);
        // The signalled block codes NO chroma coefficients, so a decoder
        // reconstructs each plane as its bare prediction. If the chroma leg
        // ran before `blk_skip_decision`/`skip_mode` committed the skip, its
        // residual-applied `u_recon`/`v_recon` must be overwritten here —
        // otherwise the encoder's reference (and final recon) carries a
        // residual the bitstream never sent: a decoder-visible drift that
        // compounds down the GOP. MEASURED: `vidyo1 256x256 q20 p11` f6,
        // U-plane ±1 diffs exactly where a skip-committed light block's U
        // residual had been applied.
        if g.has_uv {
            let pu = light_chroma_pred(fx, cx, cand, true);
            cand.u_recon = crate::vecpool::PoolVec::from_slice(&pu);
            let pv = light_chroma_pred(fx, cx, cand, false);
            cand.v_recon = crate::vecpool::PoolVec::from_slice(&pv);
        }
    }
    cand.u_cul = 0;
    cand.v_cul = 0;
    // A plane the chroma leg did not run — `!perform_chroma`, a `Cb`-/`Cr`-
    // only `component_type`, or the skip fold — reconstructs to its
    // prediction (`cand_bf->recon` is filled from `pred` for a
    // coefficient-free plane). `commit_leaf` writes `u_recon`/`v_recon`
    // unconditionally when `has_uv`, so an empty buffer is a crash, not a
    // skip.
    if g.has_uv {
        if cand.u_recon.is_empty() {
            let p = light_chroma_pred(fx, cx, cand, true);
            cand.u_recon = crate::vecpool::PoolVec::from_slice(&p);
        }
        if cand.v_recon.is_empty() {
            let p = light_chroma_pred(fx, cx, cand, false);
            cand.v_recon = crate::vecpool::PoolVec::from_slice(&p);
        }
    }

    // The shared gate/prediction buffers the `LeafEval` contract carries —
    // the light path's `cand_bf->recon` is the winner's recon.
    let win_cand = cands.swap_remove(win);
    let gate_y = crate::vecpool::PoolVec::from_slice(&win_cand.y_recon);
    let gate_u = crate::vecpool::PoolVec::from_slice(&win_cand.u_recon);
    let gate_v = crate::vecpool::PoolVec::from_slice(&win_cand.v_recon);
    // `psq_resid` — the last MDS3 candidate's depth-0 residual; the light
    // path runs ONE candidate, so it is the winner's `src - pred`.
    // Pooled scratch + the SIMD residual kernel — same pattern as
    // `evaluate_leaf`: every element is written, so a `vec![0]` fill is dead.
    let mut psq_resid = crate::vecpool::dirty_pool::<i32>(w * h);
    svtav1_dsp::residual::residual_i32(
        &y_src[y_src_off..],
        y_src_stride,
        &win_cand.pred,
        w,
        w,
        h,
        &mut psq_resid,
    );
    let psq_resid = psq_resid.into_vec();
    LeafEval {
        abs_x: g.abs_x,
        abs_y: g.abs_y,
        w,
        h,
        has_uv: g.has_uv,
        ccx: cx.ccx,
        ccy: cx.ccy,
        cw: cx.cw,
        chh: cx.chh,
        win: win_cand,
        gate_y,
        gate_u,
        gate_v,
        // The light path is the 8-bit lane (hbd_md=0 in C); no u16 gate
        // twins exist here.
        gate_y10: Vec::new(),
        gate_u10: Vec::new(),
        gate_v10: Vec::new(),
        psq_resid,
        psq_resid10: Vec::new(),
        win_recon10: Vec::new(),
        win_u_recon10: Vec::new(),
        win_v_recon10: Vec::new(),
    }
}
