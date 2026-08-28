//! MDS1: the luma-only full loop.
//!
//! C `md_stage_1` (product_coding_loop.c:7269) at staging mode 1: each MDS0
//! survivor gets one whole-block DCT_DCT transform at tx depth 0, `quantize_b`
//! with RDOQ OFF, and a FREQUENCY-domain distortion. The spatial SSE belongs to
//! MDS3 (`spatial_sse_full_loop_level = 3`, SSSE_MDS3), and MDS1's own
//! reconstruction is never read -- C's inverse-transform gate
//! (`mds_do_spatial_sse || (!is_inter && tx_depth)`, :4783) is false on both
//! disjuncts here.
//!
//! The full cost it writes is what the MDS1 -> MDS3 staging in [`super::nic`]
//! then prunes on, per candidate class.
//!
//! Split out of `evaluate_leaf` on 2026-08-25; body VERBATIM, carriers
//! destructured back into the original local names at the top.

use super::*;

/// Score every MDS0 survivor's luma-only full cost, writing it back into
/// `cands`.
#[allow(clippy::too_many_arguments)]
pub(super) fn run_mds1(
    fx: &FunnelCtx<'_>,
    g: &LeafGeom,
    bd10_rd: &Option<Bd10Rd>,
    qt: &QuantTable,
    lambda: u64,
    y_src: &[u8],
    y_src_stride: usize,
    y_src_off: usize,
    y_recon: &[u8],
    y_stride: usize,
    cands: &mut [Cand],
    order: &[usize],
    n1: usize,
) {
    // Destructure the carriers back into the names the moved body uses, so the
    // body itself is byte-for-byte what it was inside `evaluate_leaf`.
    let frame = fx.frame;
    let rates = fx.rates;
    let cfg = frame.cfg;
    let LeafGeom {
        w,
        h,
        abs_x,
        abs_y,
        skip_ctx,
        blk_crop,
        y_geom,
        filt_type_y,
        aligned_dims,
        ..
    } = *g;

    // -- MDS1: luma-only full loop (freq dist, quantize_b, DCT, depth 0) --
    for &ci in order.iter().take(n1) {
        let cand = &mut cands[ci];
        let (txb_skip_ctx, dc_sign_ctx) = if cfg.real_coeff_ctx {
            let (above, left) = fx.ectx.coeff_neighbors(abs_x, abs_y, w, h);
            cc::get_txb_ctx(0, above, left, true, false)
        } else {
            (0, 0)
        };
        // The intra dir feeding the ext-tx-type rate row: C prices FILTER
        // candidates at the fi-MAPPED direction (fimode_to_intradir; rd_cost.c
        // :135) at EVERY stage. MDS3's txt_search already mapped it — MDS1
        // didn't, under-pricing fi=V/H/D157 coeff rates by the row delta
        // (g128 q20 p0 16x4@(2,0): C ycb higher by exactly 630/684/736 for
        // fi=1/2/3 with bit-equal dists; fi=0/4 map to DC and matched).
        let intra_dir = if cand.ibc.is_some() {
            // IBC chunk 7: inter-classified — the coeff cost's tx-type
            // rate reads the INTER rows (av1_txt_rate_est is_inter arm).
            INTER_TXT_DIR
        } else if cand.fi != FI_NONE {
            FIMODE_TO_INTRADIR[cand.fi as usize] as usize
        } else {
            cand.mode as usize
        };
        let out = if frame.coded_lossless {
            // Coded-lossless: `get_start_end_tx_depth` forces depth 1 at EVERY
            // MD stage (product_coding_loop.c:6734 runs inside full_loop_core,
            // :6870), so C's MDS1 codes the 8x8 as four TX_4X4 WHT txbs with
            // per-txb intra prediction (`av1_intra_luma_prediction` at
            // tx_depth > 0, :5333) and the inverse transform live (its gate
            // is `mds_do_spatial_sse || (!is_inter && tx_depth)`). Freq-domain
            // distortion, exact coefficient rate, no RDOQ — the MDS1 contract
            // otherwise unchanged.
            debug_assert!(w == 8 && h == 8, "lossless blocks are 8x8");
            lossless_mds1_txbs(
                fx,
                y_src,
                y_src_stride,
                y_src_off,
                y_recon,
                y_stride,
                abs_x,
                abs_y,
                w,
                h,
                cand,
                intra_dir,
                txb_skip_ctx,
                dc_sign_ctx,
                &y_geom,
                filt_type_y,
                &aligned_dims,
                qt,
            )
        } else {
            tx_unit(
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
                txb_skip_ctx,
                dc_sign_ctx,
                intra_dir,
                qt,
                frame,
                rates,
                false, // no RDOQ at MDS1
                false, // freq-domain dist
                blk_crop,
                // R1: MDS1's reconstruction is UNREAD. The loop body below takes
                // only `out.eob / .bits / .dist` (and `out10`'s twins); grepping
                // the whole MDS1 loop for `out.` finds exactly that one line. C
                // agrees structurally: its inverse-transform gate is
                // `mds_do_spatial_sse || (!is_inter && tx_depth)`
                // (product_coding_loop.c:4783-4784), all-intra pins
                // `spatial_sse_full_loop_level = 3` (SSSE_MDS3,
                // enc_mode_config.c:10010) so `mds_do_spatial_sse` is FALSE at
                // MDS1 (:7025), and MDS1 evaluates tx_depth 0 — both disjuncts
                // false, so C inverts nothing here at any all-intra preset.
                false,
                RateMode::Exact,
            )
        };
        // bd10 FULL-RD (task #94): C's MDS1 at hbd_md != 0 runs the SAME
        // luma-only full loop on 10-bit pixels — 10-bit residual, bd10 quant
        // table, bd10 lambda, and the bit-depth-INDEPENDENT freq-domain
        // distortion (svt_aom_picture_full_distortion32_bits_single). Deciding
        // it at 8 bits picks C's bd8 winner; below eff-M9 several candidates
        // survive to MDS3, so this ordering + the pruning below is binding.
        // The u8 `out` above still runs — nothing downstream of MDS1 reads it,
        // but keeping it keeps the bd8 expression untouched and the two
        // domains directly comparable under SVTAV1_CANDDBG.
        let out10 = bd10_rd.as_ref().map(|b| {
            tx_unit_hbd(
                &b.y_src10,
                w,
                0,
                &cand.pred10,
                w,
                0,
                w,
                h,
                cc::DCT_DCT,
                0,
                txb_skip_ctx,
                dc_sign_ctx,
                &b.qt,
                frame.rdoq_level,
                b.lambda,
                frame.sharpness,
                rates,
                false, // no RDOQ at MDS1 (mirrors the u8 call)
                b.bd,
                b.qt.qm_level,
                Some(&TxRdArgs {
                    spatial_dist: false, // MDS1 = freq-domain residual
                    intra_dir,
                    coeff_rate_est_lvl: cfg.coeff_rate_est_lvl,
                    tx_bias: frame.tx_bias,
                    crop: blk_crop,
                }),
            )
        });
        let (dec_eob, dec_bits, dec_dist, dec_lambda) = match &out10 {
            Some(o) => (
                o.eob,
                o.bits as u64,
                o.dist,
                bd10_rd.as_ref().unwrap().lambda,
            ),
            None => (out.eob, out.bits as u64, out.dist, lambda),
        };
        let has = dec_eob > 0;
        let tsz_cat = tx_size_cat(w, h);
        let tsz_ctx = fx.ectx.tx_size_ctx(abs_x, abs_y, w, h);
        // C: 4x4 codes no tx_size symbol (block_signals_txsize == bsize > 4x4).
        // IBC (inter-classified): tx_size codes via the var-tx walk when the
        // block has coeffs, and ZERO bits when skip (svt_aom_tx_size_bits'
        // `!(is_inter_tx && skip)` gate) — svt_aom_full_cost prices exactly
        // that pair at MDS1 too.
        let coeff_rate = if cand.ibc.is_some() {
            let vartx_bits = if has && block_signals_txsize(w, h) {
                crate::vartx::tx_size_bits_vartx(
                    &rates.txfm_partition_fac_bits,
                    fx.ectx.txfm_above_span(abs_x, w),
                    fx.ectx.txfm_left_span(abs_y, h),
                    w,
                    h,
                    0, // MDS1 evaluates depth 0
                    abs_y,
                    frame.frame_h_px,
                )
            } else {
                0
            };
            if has {
                dec_bits + vartx_bits + rates.skip[skip_ctx][0] as u64
            } else {
                rates.skip[skip_ctx][1] as u64
            }
        } else {
            // C `svt_aom_tx_size_bits` (rd_cost.c:1755): 0 on a lossless
            // segment (no tx_size symbol is coded there).
            let tx_size_bits = if block_signals_txsize(w, h) && !frame.coded_lossless {
                rates.tx_size[tsz_cat][tsz_ctx][0] as u64
            } else {
                0
            };
            if has {
                dec_bits + tx_size_bits + rates.skip[skip_ctx][0] as u64
            } else {
                rates.skip[skip_ctx][1] as u64 + tx_size_bits
            }
        };
        cand.mds1_has_coeff = has;
        cand.full_cost = rdcost(dec_lambda, cand.flr + cand.fcr + coeff_rate, dec_dist);
        #[cfg(feature = "std")]
        if crate::dbgenv::canddbg() && crate::depth_refine::nsqdbg_here(abs_x, abs_y) {
            eprintln!(
                "NSQDBG PMDS1 mi=({},{}) {}x{} mode={} fi={} delta={} uv={} coeff_rate={} dist={} full={}",
                abs_y / 4,
                abs_x / 4,
                w,
                h,
                cand.mode,
                cand.fi,
                cand.delta,
                cand.uv,
                coeff_rate,
                dec_dist,
                cand.full_cost,
            );
        }
    }
}

/// Coded-lossless MDS1 luma loop for one candidate: the 8x8 block as four
/// TX_4X4 WHT txbs in raster order, each predicted from the recon of the txbs
/// before it (C `perform_tx_partitioning` at tx_depth 1, product_coding_loop.c
/// :5282-5420, with `av1_intra_luma_prediction` per txb). Returns the block's
/// summed eob / bits / distortion in a `TxUnitOut` (recon and coefficients
/// are not carried — MDS1 reads only those three, exactly like the depth-0
/// call it replaces).
#[allow(clippy::too_many_arguments)]
fn lossless_mds1_txbs(
    fx: &FunnelCtx<'_>,
    y_src: &[u8],
    y_src_stride: usize,
    y_src_off: usize,
    y_recon: &[u8],
    y_stride: usize,
    abs_x: usize,
    abs_y: usize,
    w: usize,
    h: usize,
    cand: &Cand,
    intra_dir: usize,
    blk_txb_skip_ctx: usize,
    blk_dc_sign_ctx: usize,
    y_geom: &UnitGeom,
    filt_type_y: i32,
    aligned_dims: &crate::frame_geom::FrameDims,
    qt: &QuantTable,
) -> TxUnitOut {
    let frame = fx.frame;
    let rates = fx.rates;
    let cfg = frame.cfg;
    let (txw, txh) = txb_dims_at_depth(w, h, 1);
    let cols = w / txw;
    let txbs = cols * (h / txh);
    let mut loc_above = fx.ectx.above_coeff_span(abs_x, w).to_vec();
    let mut loc_left = fx.ectx.left_coeff_span(abs_y, h).to_vec();
    let mut dep_recon = vec![0u8; w * h];
    let mut eob_total: u32 = 0;
    let mut bits_total: i64 = 0;
    let mut dist_total: u64 = 0;
    for txb in 0..txbs {
        let (tx_x, tx_y) = ((txb % cols) * txw, (txb / cols) * txh);
        let mut txb_pred = vec![0u8; txw * txh];
        if cand.palette.is_some() || cand.ibc.is_some() {
            for r in 0..txh {
                let src0 = (tx_y + r) * w + tx_x;
                txb_pred[r * txw..(r + 1) * txw].copy_from_slice(&cand.pred[src0..src0 + txw]);
            }
        } else {
            predict_unit_overlay(
                y_recon,
                y_stride,
                abs_x,
                abs_y,
                &dep_recon,
                w,
                h,
                tx_x,
                tx_y,
                txw,
                txh,
                cand.mode,
                cand.delta,
                cand.fi,
                y_geom,
                cfg.edge_filter,
                filt_type_y,
                &mut txb_pred,
            );
        }
        let (tsc, dsc) = if cfg.real_coeff_ctx {
            txb_ctx_from_spans(&loc_above, &loc_left, tx_x, tx_y, txw, txh, false)
        } else {
            (blk_txb_skip_ctx, blk_dc_sign_ctx)
        };
        let txb_crop =
            crate::frame_geom::cropped_tx_dims(aligned_dims, abs_x + tx_x, abs_y + tx_y, txw, txh);
        let out = tx_unit(
            y_src,
            y_src_stride,
            y_src_off + tx_y * y_src_stride + tx_x,
            &txb_pred,
            txw,
            0,
            txw,
            txh,
            cc::DCT_DCT,
            0,
            tsc,
            dsc,
            intra_dir,
            qt,
            frame,
            rates,
            false, // no RDOQ at MDS1 (and never on a lossless segment)
            false, // freq-domain dist
            txb_crop,
            true, // the recon feeds the next txb's prediction
            RateMode::Exact,
        );
        eob_total += u32::from(out.eob);
        bits_total += i64::from(out.bits);
        dist_total += out.dist;
        let a0 = (tx_x / 4).min(loc_above.len());
        let a1 = (a0 + txw / 4).min(loc_above.len());
        for v in loc_above[a0..a1].iter_mut() {
            *v = out.cul;
        }
        let l0 = (tx_y / 4).min(loc_left.len());
        let l1 = (l0 + txh / 4).min(loc_left.len());
        for v in loc_left[l0..l1].iter_mut() {
            *v = out.cul;
        }
        for r in 0..txh {
            let dst = (tx_y + r) * w + tx_x;
            dep_recon[dst..dst + txw].copy_from_slice(&out.recon[r * txw..(r + 1) * txw]);
        }
    }
    TxUnitOut {
        eob: eob_total.min(u32::from(u16::MAX)) as u16,
        qcoeff: Vec::new(),
        recon: Vec::new(),
        dist: dist_total,
        bits: bits_total as i32,
        cul: 0,
    }
}
