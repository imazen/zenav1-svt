use super::*;

#[inline(always)]
pub(super) fn search_tx_depths(
    fx: &mut FunnelCtx<'_>,
    bd10_rd: &Option<Bd10Rd>,
    qt: &QuantTable,
    lambda: u64,
    y_src: &[u8],
    y_src_stride: usize,
    y_src_off: usize,
    y_recon: &[u8],
    y_stride: usize,
    cands: &mut [Cand],
    ci: usize,
    sc: &mut Mds3Scratch,
    frame: &FunnelFrame,
    rates: &MdRates,
    cfg: FunnelCfg,
    w: usize,
    h: usize,
    abs_x: usize,
    abs_y: usize,
    y_geom: UnitGeom,
    filt_type_y: i32,
    aligned_dims: crate::frame_geom::FrameDims,
    end_depth: u8,
    tsz_cat: usize,
    tsz_ctx: usize,
    lambda3: u64,
    perform_mds1: bool,
    use_tx_shortcuts_mds3: bool,
    bypass_tx: bool,
    dct_tx_path: bool,
    best_depth: &mut u8,
    mut best_cost: u64,
    best_bits: &mut u64,
    best_dist: &mut u64,
    best_txb_q: &mut Vec<crate::vecpool::PoolVec<i32>>,
    best_txb_eob: &mut smallvec::SmallVec<[u16; 16]>,
    best_txb_cul: &mut smallvec::SmallVec<[u8; 16]>,
    best_txb_type: &mut smallvec::SmallVec<[u8; 16]>,
    best_recon: &mut crate::vecpool::PoolVec<u8>,
    best_recon10: &mut Vec<u16>,
    best_pred: &mut crate::vecpool::PoolVec<u8>,
    best_pred10: &mut Vec<u16>,
    d0_recon: &mut crate::vecpool::PoolVec<u8>,
    d0_recon10: &mut Vec<u16>,
    best_coeff_count: &mut u32,
    start_depth: u8,
    cand_end_depth: u8,
) {
    for depth in start_depth..=cand_end_depth {
        // prev_depth_coeff_exit_th (1 at txs_level <=4; 100 at eff-M9
        // txs_level 5): skip a deeper depth when the best depth so far
        // kept fewer than the threshold's worth of non-zero coeffs.
        if *best_coeff_count < cfg.txs_prev_depth_exit {
            continue;
        }
        // C tx geometry at this depth (tx_depth_to_tx_size /
        // tx_blocks_per_depth / the intra tx_org raster).
        let (txw, txh) = txb_dims_at_depth(w, h, depth);
        let cols = w / txw;
        let txbs = cols * (h / txh);
        // TX-local dc_sign/cul overlay (tx_reset_neighbor_arrays).
        sc.loc_above.clear();
        sc.loc_above
            .extend_from_slice(fx.ectx.above_coeff_span(abs_x, w));
        sc.loc_left.clear();
        sc.loc_left
            .extend_from_slice(fx.ectx.left_coeff_span(abs_y, h));
        let loc_above = &mut sc.loc_above;
        let loc_left = &mut sc.loc_left;
        let mut dep_bits: u64 = 0;
        let mut dep_dist: u64 = 0;
        // MEASURED NOT worth hoisting out of the depth loop. Doing that, with
        // `best_txb_q.clear(); best_txb_q.extend(dep_q.drain(..))` on a new
        // best, moved the allocation rather than removing it -- `best_txb_q`
        // starts empty per candidate, so the `extend` allocates exactly where
        // the move below did not. 1,311,342 -> 1,344,182 on the canonical alloc
        // cell. The MOVE is already the cheap form.
        let mut dep_q: Vec<crate::vecpool::PoolVec<i32>> = Vec::with_capacity(txbs);
        let mut dep_eob: smallvec::SmallVec<[u16; 16]> = smallvec::SmallVec::with_capacity(txbs);
        let mut dep_cul: smallvec::SmallVec<[u8; 16]> = smallvec::SmallVec::with_capacity(txbs);
        let mut dep_type: smallvec::SmallVec<[u8; 16]> = smallvec::SmallVec::with_capacity(txbs);
        // DIRTY, not zeroed. Every position is written before it is read:
        // the TXB loop below covers the whole block, and the only exit that
        // leaves it partial is `if aborted && depth > 0 { continue; }`, which
        // discards the depth without touching `best_recon`. Depth 0 never
        // aborts (the guard is `depth > 0`), so the `d0_recon` copy always sees
        // a complete block. Validated by the 0x5A poisoning control recorded on
        // `vecpool::dirty_pool`.
        let mut dep_recon = dirty_pool::<u8>(w * h);
        // This depth's assembled whole-block luma prediction (see
        // `best_pred`); mirrors what C leaves in `cand_bf->pred->y_buffer`.
        let mut dep_pred = dirty_pool::<u8>(w * h);
        // Its 10-bit twin, assembled from the same per-txb predictions.
        let mut dep_pred10 = if bd10_rd.is_some() {
            zeroed_pool::<u16>(w * h)
        } else {
            crate::vecpool::PoolVec::new()
        };
        let mut dep_has_coeff = false;
        let mut aborted = false;
        // bd10 FULL-RD (task #94): the depth's 10-bit recon, which the
        // NEXT txb of a deeper depth predicts from (the same intra-block
        // sequential coupling the u8 `dep_recon` carries). `dep_dist` /
        // `dep_bits` above accumulate the 10-bit terms when active, so the
        // depth compare — and therefore tx_depth — is decided at bd10.
        let mut dep_recon10 = if bd10_rd.is_some() {
            vec![0u16; w * h]
        } else {
            Vec::new()
        };

        for txb in 0..txbs {
            let cand = &cands[ci];
            // Inter (IntraBC) txbs walk the C tx_org is_inter=1 rows
            // (z-order at depth 2); intra keeps the plain raster.
            let (tx_x, tx_y) = if cand.is_inter() {
                txb_org_inter(w, h, depth, txb)
            } else {
                ((txb % cols) * txw, (txb / cols) * txh)
            };
            // Per-txb prediction: depth 0 reuses the MDS0 pred;
            // depth > 0 predicts from the live canvas (frame recon
            // outside the block, this depth's recon inside).
            // Grow-only, and NOT re-zeroed: every one of the `txw * txh`
            // elements is written below on all three branches (a full-length
            // `copy_from_slice`, a per-row `copy_from_slice`, or
            // `predict_unit_overlay`). `clear()` + `resize(n, 0)` was measured
            // SLOWER than the `vec![0u8; n]` it replaced at 512x512 preset 2
            // (0.998x with the whole span above 1.0, reproduced) — `vec!` gets
            // its zeros from fresh `calloc` pages for free, an explicit resize
            // pays a real `memset`.
            if sc.txb_pred.len() < txw * txh {
                sc.txb_pred.resize(txw * txh, 0);
            }
            let txb_pred: &mut [u8] = &mut sc.txb_pred[..txw * txh];
            if depth == 0 {
                txb_pred.copy_from_slice(&cand.pred);
            } else if cand.palette.is_some() || cand.is_inter() {
                // Palette: position-only substitution. IntraBC: C
                // computes the INTER residual once from the block-level
                // prediction and never re-predicts per txb (the
                // `if (!is_inter)` skip, product_coding_loop.c:5325) —
                // a deeper-depth txb pred is the slice of the DV copy.
                // Palette prediction is position-only substitution
                // (enc_intra_prediction.c:640-651 runs per tx block
                // over the SAME map — no neighbor edges), so a
                // deeper-depth txb pred is just the slice of the
                // whole-block substitution already in cand.pred.
                for r in 0..txh {
                    let src0 = (tx_y + r) * w + tx_x;
                    txb_pred[r * txw..(r + 1) * txw].copy_from_slice(&cand.pred[src0..src0 + txw]);
                }
            } else {
                // Overlay canvas: temporarily splice this depth's
                // reconstructed txbs into the frame recon.
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
                    &y_geom,
                    cfg.edge_filter,
                    filt_type_y,
                    &mut txb_pred[..],
                );
            }
            // Accumulate this depth's whole-block prediction. At depth 0
            // txbs == 1, so this reproduces `cand.pred` exactly.
            for r in 0..txh {
                let dst = (tx_y + r) * w + tx_x;
                dep_pred[dst..dst + txw].copy_from_slice(&txb_pred[r * txw..(r + 1) * txw]);
            }
            // The SAME per-txb prediction at 10 bits, by the same three
            // rules: depth 0 reuses the MDS0 10-bit whole-block pred;
            // palette is position-only substitution (no neighbour edges),
            // so a deeper txb is a slice of it; otherwise predict from the
            // 10-bit overlay canvas.
            // NOT scratch-backed, deliberately: this buffer's LENGTH is
            // observable — `copy_from_slice` below needs it to equal
            // `cand.pred10`, and it is handed on whole as `pred10:
            // &txb_pred10`. A grow-only scratch would silently make it longer
            // than `txw * txh` for a smaller txb. The bd10 MD path is not on
            // the 8-bit still arm this chunk measures, so it keeps the
            // per-call `Vec` until someone measures it.
            let mut txb_pred10: Vec<u16> = Vec::new();
            if bd10_rd.is_some() {
                txb_pred10 = vec![0u16; txw * txh];
                if depth == 0 {
                    txb_pred10.copy_from_slice(&cand.pred10);
                } else if cand.palette.is_some() || cand.is_inter() {
                    for r in 0..txh {
                        let src0 = (tx_y + r) * w + tx_x;
                        txb_pred10[r * txw..(r + 1) * txw]
                            .copy_from_slice(&cand.pred10[src0..src0 + txw]);
                    }
                } else {
                    predict_unit_overlay_hbd(
                        fx.y_recon10.as_deref().unwrap(),
                        y_stride,
                        abs_x,
                        abs_y,
                        &dep_recon10,
                        w,
                        h,
                        tx_x,
                        tx_y,
                        txw,
                        txh,
                        cand.mode,
                        cand.delta,
                        cand.fi,
                        &y_geom,
                        cfg.edge_filter,
                        filt_type_y,
                        &mut txb_pred10,
                        frame.bit_depth,
                    );
                }
                // Accumulate this depth's whole-block 10-bit prediction,
                // exactly as `dep_pred` does for u8 — C writes both
                // through the same `cand_bf->pred->y_buffer`.
                for r in 0..txh {
                    let dst = (tx_y + r) * w + tx_x;
                    dep_pred10[dst..dst + txw].copy_from_slice(&txb_pred10[r * txw..(r + 1) * txw]);
                }
            }
            // Per-txb contexts from the TX-local overlay (real at M6;
            // 0/0 at M7/M8 where update_skip_ctx_dc_sign_ctx == 0, so
            // cul_level never accumulates — full_loop.c:1880).
            let (tsc, dsc) = if cfg.real_coeff_ctx {
                txb_ctx_from_spans(loc_above, loc_left, tx_x, tx_y, txw, txh, depth == 0)
            } else {
                (0, 0)
            };
            // TXT search over this txb. Inter-classified txbs carry a
            // sentinel: INTER_TXT_DIR for real inter (`pred_mode >=
            // NEARESTMV`), IBC_TXT_DIR for IntraBC (the inter ext-tx set
            // + inter tx-type rate rows, but the intra RDOQ rdmult and
            // the intra-DC coeff-cost tx-type row — see coeff_rate.rs).
            let intra_dir = if cand.inter.is_some() {
                INTER_TXT_DIR
            } else if cand.ibc.is_some() {
                IBC_TXT_DIR
            } else if cand.fi != FI_NONE {
                FIMODE_TO_INTRADIR[cand.fi as usize] as usize
            } else {
                cand.mode as usize
            };
            let bd10_txb = bd10_rd.as_ref().map(|b| Bd10Txb {
                src10: &b.y_src10,
                src10_stride: w,
                src10_off: tx_y * w + tx_x,
                pred10: &txb_pred10,
                qt: &b.qt,
                lambda: b.lambda,
                bd: b.bd,
            });
            #[cfg(feature = "std")]
            let txt_dbg_tag = {
                static XY: std::sync::OnceLock<Option<(usize, usize)>> = std::sync::OnceLock::new();
                (dbg_xy(&XY, "SVTAV1_TXT_XY") == Some((abs_x, abs_y))).then_some(TxtDbg {
                    abs_x,
                    abs_y,
                    tx_x,
                    tx_y,
                    mode: cand.mode,
                    fi: cand.fi,
                    inter: cand.inter.as_deref().map(|ic| {
                        (
                            ic.mode as u8,
                            ic.ref_frame[0],
                            ic.ref_frame[1],
                            ic.mv[0].x,
                            ic.mv[0].y,
                            ic.drl_index,
                        )
                    }),
                })
            };
            #[cfg(not(feature = "std"))]
            let txt_dbg_tag = None;
            // C `cropped_tx_width`/`cropped_tx_height` for THIS txb
            // (product_coding_loop.c:4664-4665 / :5752-5754): the tx
            // origin is the block origin plus the txb offset, and the
            // bound is the ALIGNED frame extent. Identity on a
            // 64-aligned frame.
            let txb_crop = crate::frame_geom::cropped_tx_dims(
                &aligned_dims,
                abs_x + tx_x,
                abs_y + tx_y,
                txw,
                txh,
            );
            // C's `pf_shape` derivation (product_coding_loop.c:4664-4676
            // for `tx_type_search`, :5725-5735 for `perform_dct_dct_tx`):
            // N4 keeps only the top-left (w>>2)x(h>>2) quadrant of the
            // transform output. The `tx_type_search` path has TWO arms —
            // `use_tx_shortcuts_mds3` and `apply_pf_on_coeffs` — while
            // `perform_dct_dct_tx` has ONLY the `apply_pf_on_coeffs` arm.
            // `dct_tx_path` selects between them; it is C's own :6981-6997
            // dispatch to `perform_dct_dct_tx`.
            //
            // The `apply_pf_on_coeffs` threshold reads the MDS1 value of
            // `cnt_nz_coeff` (`cand.mds1_cnt_nz`), NOT the post-luma-sweep
            // `cnt_nz_coeff` that C writes back at :7005 — the writeback
            // happens after the whole luma loop.
            //
            // `th = (txw>>4) * (txh>>4)` — C's `txbheight_original`
            // (the UNCROPPED txb height) at :4674.
            let apply_pf_n4 = cfg.tx_shortcut.apply_pf_on_coeffs != 0
                && perform_mds1
                && (cands[ci].mds1_cnt_nz < (txw as u32 >> 4) * (txh as u32 >> 4)
                    || !cands[ci].mds1_has_coeff);
            let n4 = if dct_tx_path {
                apply_pf_n4
            } else {
                use_tx_shortcuts_mds3 || apply_pf_n4
            };
            let (out, out10, txt) = txt_search(
                y_src,
                y_src_stride,
                y_src_off + tx_y * y_src_stride + tx_x,
                txb_pred,
                txw,
                txh,
                txb_crop,
                depth,
                tsc,
                dsc,
                intra_dir,
                qt,
                frame,
                rates,
                frame.rdoq,
                lambda,
                bd10_txb.as_ref(),
                txt_dbg_tag,
                // R2: on this branch the exact coefficient rate is
                // COMPUTED AND THEN OVERWRITTEN by the closed form below
                // (`txb_bits`, :6230-ish). C never computes it — its rate
                // tiers are an `if / else if / else` and only the taken arm
                // runs (product_coding_loop.c:5540-5564). Producing the
                // closed form inside `tx_unit` yields the SAME `bits`
                // arithmetic, so this is not a deadness claim.
                if cfg.coeff_rate_est_lvl == 0 && end_depth > 0 {
                    RateMode::Lvl0Closed
                } else {
                    RateMode::Exact
                },
                TxtGate {
                    force_dct: use_tx_shortcuts_mds3 || bypass_tx,
                    skip_tx: bypass_tx,
                    n4,
                },
            );
            // SVTAV1_QLEV_XY="x,y": per-txb winner (tx_type, eob, levels)
            // at one pinned block, to join against the C `--wrap
            // svt_aom_quantize_inv_quantize` QLEV dump. bd8 takes `out`;
            // bd10's full-RD result is `out10`.
            #[cfg(feature = "std")]
            {
                static XY: std::sync::OnceLock<Option<(usize, usize)>> = std::sync::OnceLock::new();
                if dbg_xy(&XY, "SVTAV1_QLEV_XY") == Some((abs_x, abs_y)) {
                    let (eob_d, qcoeff): (usize, &[i32]) = match &out10 {
                        Some(o) => (usize::from(o.eob), &o.qcoeff),
                        None => (usize::from(out.eob), &out.qcoeff),
                    };
                    let nz: alloc::vec::Vec<_> = qcoeff
                        .iter()
                        .enumerate()
                        .filter(|&(_, &v)| v != 0)
                        .map(|(i, v)| alloc::format!("{i}:{v}"))
                        .collect();
                    // `SVTAV1_QLEV_CO` additionally prints C QLEV's `co=`
                    // field — the PRE-quant packed coefficients — recomputed
                    // here from the same (src, pred, tx_type) triple
                    // `tx_unit_inner` consumed: residual + fwd 2D + the
                    // 64-dim fold to `pw x ph`.
                    let mut co_field = alloc::string::String::new();
                    if crate::dbgenv::qlev_co() {
                        let n = txw * txh;
                        let mut res = alloc::vec![0i16; n];
                        svtav1_dsp::residual::residual_i16(
                            &y_src[y_src_off + tx_y * y_src_stride + tx_x..],
                            y_src_stride,
                            &txb_pred[..],
                            txw,
                            txw,
                            txh,
                            &mut res,
                        );
                        let mut cf = alloc::vec![0i32; n];
                        let c_tx = cc::tx_size_from_dims(txw, txh);
                        let ok = svtav1_dsp::txfm_dispatch::fwd_txfm2d_dispatch(
                            &res,
                            &mut cf,
                            txw,
                            tx_pipeline::TX_SIZE_FROM_C[c_tx],
                            tx_pipeline::TX_TYPE_FROM_C[txt],
                        );
                        debug_assert!(ok, "fwd txfm {txw}x{txh} type {txt}");
                        let (pw, ph) = (txw.min(32), txh.min(32));
                        let mut v = alloc::vec::Vec::new();
                        for r in 0..ph {
                            for (i, &val) in cf[r * txw..r * txw + pw].iter().enumerate() {
                                if val != 0 {
                                    v.push(alloc::format!("{}:{val}", r * pw + i));
                                }
                            }
                        }
                        co_field = alloc::format!(" co=[{}]", v.join(","));
                    }
                    let cand_tag = cand.inter.as_deref().map_or_else(
                        || alloc::format!("mode={}", cand.mode),
                        |ic| {
                            alloc::format!(
                                "mode={} rf={},{} mv0={},{} mv1={},{} drl={} itf={:#x}",
                                cand.mode,
                                ic.ref_frame[0],
                                ic.ref_frame[1],
                                ic.mv[0].x,
                                ic.mv[0].y,
                                ic.mv[1].x,
                                ic.mv[1].y,
                                ic.drl_index,
                                ic.interp_filters,
                            )
                        },
                    );
                    eprintln!(
                        "PQLEV org=({abs_x},{abs_y}) d={depth} tx=({tx_x},{tx_y}) {txw}x{txh} txt={txt} dq={:?} lam={lambda} eob={} {cand_tag} nz=[{}]{co_field}",
                        qt.dequant,
                        eob_d,
                        nz.join(",")
                    );
                }
            }
            // The decision terms: 10-bit when the bd10 full-RD is active.
            let (dec_eob, dec_bits_raw, dec_dist, dec_cul) = match &out10 {
                Some(o) => (o.eob, o.bits, o.dist, o.cul),
                None => (out.eob, out.bits, out.dist, out.cul),
            };
            // eff-M9 (coeff_rate_est_lvl 0) prices the luma coeff RATE in
            // the RD compare with the fast per-txb approximation from C
            // `tx_type_search` (product_coding_loop.c:4976), NOT the real
            // cost_coeffs_txb: th = (txw*txh)>>6; eob<th ? 6000+eob*1000
            // : 3000+eob*100. The real bits still drove RDOQ/eob inside
            // `tx_unit` (unchanged). Gated on end_depth>0 == C's
            // perform_tx_partitioning path; end_depth==0 blocks go through
            // perform_dct_dct_tx and keep the funnel's estimate (their
            // single-candidate decision is rate-invariant).
            let txb_bits = if cfg.coeff_rate_est_lvl == 0 && end_depth > 0 {
                let th = (txw * txh) >> 6;
                if (dec_eob as usize) < th {
                    6000 + dec_eob as u64 * 1000
                } else {
                    3000 + dec_eob as u64 * 100
                }
            } else {
                dec_bits_raw as u64
            };
            dep_bits += txb_bits;
            dep_dist += dec_dist;
            dep_has_coeff |= dec_eob > 0;
            // tx_update_neighbor_arrays: cul byte over the txb span. Clamp
            // the START to the span length (partial-SB straddle: an
            // off-frame txb's 4x4 origin exceeds the in-frame-clipped span)
            // so the range is empty rather than start>end. No in-frame cell
            // reads an off-frame txb's cul, so skipping the write matches C;
            // byte-neutral for every in-frame txb (start <= len).
            let a0 = (tx_x / 4).min(loc_above.len());
            let a1 = (a0 + txw / 4).min(loc_above.len());
            for v in loc_above[a0..a1].iter_mut() {
                *v = dec_cul;
            }
            let l0 = (tx_y / 4).min(loc_left.len());
            let l1 = (l0 + txh / 4).min(loc_left.len());
            for v in loc_left[l0..l1].iter_mut() {
                *v = dec_cul;
            }
            for r in 0..txh {
                let dst = (tx_y + r) * w + tx_x;
                dep_recon[dst..dst + txw].copy_from_slice(&out.recon[r * txw..(r + 1) * txw]);
            }
            if let Some(o) = &out10 {
                for r in 0..txh {
                    let dst = (tx_y + r) * w + tx_x;
                    dep_recon10[dst..dst + txw].copy_from_slice(&o.recon[r * txw..(r + 1) * txw]);
                }
            }

            // The CODED levels. With the bd10 full-RD active these come from
            // the 10-bit quantize/RDOQ — which is what C codes, and which
            // (unlike the level-only re-encode post-pass) carries this
            // txb's REAL txb_skip/dc_sign contexts into the trellis. Both
            // forms are the same packed (32-capped) pw*ph layout the
            // entropy walk re-expands (partition.rs funnel_block_decision).
            dep_q.push(match out10 {
                // The bd10 chain's `TxUnitOutHbd` is still a plain `Vec`
                // (its `recon: Vec<u16>` has no free list yet), so the 10-bit
                // arm pays one copy into the pool here. The 8-bit arm MOVES,
                // which is the hot path and the one heaptrack measured.
                Some(o) => crate::vecpool::PoolVec::from_slice(&o.qcoeff),
                None => out.qcoeff,
            });
            dep_eob.push(dec_eob);
            dep_cul.push(dec_cul);
            dep_type.push(txt as u8);

            // C txb loop early exit: current accumulated cost already
            // above the best depth cost.
            if rdcost(lambda3, dep_bits, dep_dist) > best_cost {
                aborted = true;
                break;
            }
            // C quadrant early-abort (txs_ctrls.quadrant_th_sf,
            // product_coding_loop.c:5437): for a deeper depth, if the
            // accumulated cost (incl. this depth's full tx_size bits)
            // already exceeds its proportional share of the best depth
            // cost, drop the depth. `svt_aom_get_tx_size_bits` for intra
            // == the tx_size rate at (cat, ctx, depth) (skip/has-coeff
            // only gate the inter path).
            if cfg.txs_quadrant_sf != 0 && depth > 0 {
                let normlized = ((txb as u64 + 1) * best_cost) / txbs as u64;
                let tsb = if cands[ci].is_inter() {
                    // Inert at the IBC presets (quadrant_sf == 0 at
                    // txs_level 2/3) — kept faithful to
                    // svt_aom_get_tx_size_bits' inter arm regardless.
                    if dep_has_coeff && block_signals_txsize(w, h) && !frame.coded_lossless {
                        crate::vartx::tx_size_bits_vartx(
                            &rates.txfm_partition_fac_bits,
                            fx.ectx.txfm_above_span(abs_x, w),
                            fx.ectx.txfm_left_span(abs_y, h),
                            w,
                            h,
                            depth,
                            abs_y,
                            frame.frame_h_px,
                        )
                    } else {
                        0
                    }
                } else if frame.coded_lossless {
                    0 // svt_aom_tx_size_bits: no tx_size bits on a lossless segment
                } else {
                    rates.tx_size[tsz_cat][tsz_ctx][depth as usize] as u64
                };
                let cost_tmp = rdcost(lambda3, dep_bits + tsb, dep_dist);
                if cost_tmp * 100 > normlized * cfg.txs_quadrant_sf {
                    aborted = true;
                    break;
                }
            }
        }
        if aborted && depth > 0 {
            continue;
        }
        // C: 4x4 codes no tx_size symbol (block_signals_txsize == bsize > 4x4).
        // IntraBC (inter-classified): svt_aom_get_tx_size_bits prices the
        // var-tx walk when the depth kept coeffs, 0 bits when skip
        // (`!(is_inter_tx && skip)`).
        let tx_size_bits = if cands[ci].is_inter() {
            if dep_has_coeff && block_signals_txsize(w, h) && !frame.coded_lossless {
                crate::vartx::tx_size_bits_vartx(
                    &rates.txfm_partition_fac_bits,
                    fx.ectx.txfm_above_span(abs_x, w),
                    fx.ectx.txfm_left_span(abs_y, h),
                    w,
                    h,
                    depth,
                    abs_y,
                    frame.frame_h_px,
                )
            } else {
                0
            }
        } else if block_signals_txsize(w, h) && !frame.coded_lossless {
            // C `svt_aom_tx_size_bits` (rd_cost.c:1755) prices 0 bits on a
            // lossless segment — the pack writes no tx_size symbol either.
            rates.tx_size[tsz_cat][tsz_ctx][depth as usize] as u64
        } else {
            0
        };
        let cost = rdcost(lambda3, dep_bits + tx_size_bits, dep_dist);
        // SVTAV1_TXDEPTH_XY="x,y": per-tx_depth RD terms at one pinned
        // block ORIGIN — the port counterpart of the C
        // `perform_tx_partitioning` depth compare
        // (product_coding_loop.c:5425-5432), so a tx-depth flip can be
        // attributed to the coeff rate, the tx_size rate or the
        // distortion without re-deriving any of them.
        #[cfg(feature = "std")]
        {
            static XY: std::sync::OnceLock<Option<(usize, usize)>> = std::sync::OnceLock::new();
            if dbg_xy(&XY, "SVTAV1_TXDEPTH_XY") == Some((abs_x, abs_y)) {
                eprintln!(
                    "PTXDEPTH org=({abs_x},{abs_y}) {w}x{h} d={depth} ibc={} mode={} ycb={dep_bits} txsz={tx_size_bits} dist={dep_dist} cost={cost} best={best_cost}",
                    u8::from(cands[ci].is_inter()),
                    cands[ci].mode,
                );
            }
        }
        // Depth 0 never aborts (the abort guard is `depth > 0`), so this
        // is always populated for every candidate that reaches MDS3.
        if depth == 0 {
            *d0_recon = dep_recon.clone();
            if !dep_recon10.is_empty() {
                *d0_recon10 = dep_recon10.clone();
            }
        }
        if cost < best_cost {
            best_cost = cost;
            *best_depth = depth;
            *best_bits = dep_bits;
            *best_dist = dep_dist;
            *best_txb_q = dep_q;
            *best_txb_eob = dep_eob.clone();
            *best_txb_cul = dep_cul;
            *best_txb_type = dep_type;
            *best_recon = dep_recon;
            *best_recon10 = core::mem::take(&mut dep_recon10);
            *best_pred = dep_pred;
            *best_pred10 = core::mem::take(&mut dep_pred10);
            *best_coeff_count = dep_eob.iter().map(|&e| e as u32).sum();
            let _ = dep_has_coeff;
        }
    }
}
