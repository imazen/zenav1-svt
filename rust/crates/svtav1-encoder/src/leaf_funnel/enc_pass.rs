//! Ghost Robot encode-pass luma re-encode — C `svt_aom_encode_sb`,
//! invoked per superblock right after `svt_aom_pick_partition` returns
//! (`svt_aom_mode_decision_kernel_iter`, enc_dec_process.c:3112-3143) and
//! only when `pic_bypass_encdec` is 0.
//!
//! C's encode pass (`av1_encode_loop`, coding_loop.c:429) re-derives every
//! committed leaf's prediction on the LIVE `recon_pic` canvas — which by
//! then holds every EARLIER encode-passed block's reconstruction —
//! re-forms the residual, re-quantizes with `is_encode_pass = true`, and
//! writes THAT pass's levels into `blk_ptr->level`, its recon into
//! `recon_pic`, and its `(dc_sign<<6)|cul` byte into
//! `ep_luma_dc_sign_level_coeff_na`. The encode-only arms of
//! `svt_aom_quantize_inv_quantize` (full_loop.c:1975-2000) — the fork's
//! luma noise normalization and the light-RDOQ chroma arm — fire HERE on
//! the winner, never during the mode-decision evaluations that priced it.
//! Under `pic_bypass_encdec` C's coefficients come straight from mode
//! decision and are never normalized, so this module is never entered.
//!
//! This pass reproduces the LUMA half: it walks the committed
//! `PartitionTree` in decode order, re-predicts each intra leaf on the
//! tile recon canvas, re-quantizes through `tx_unit` with
//! `TxGate::enc_pass` set (the `is_encode_pass` equivalent that gates the
//! noise-normalization arm), rewrites the leaf's coefficients / eobs /
//! tx-types, splices the new recon back into the canvas, and maintains
//! the encode-pass coefficient-context chain so `optimize_b`'s
//! `(txb_skip_ctx, dc_sign_ctx)` reads post-norm cul values — C's
//! `ep_luma_dc_sign_level_coeff_na`.
//!
//! Because the canvas at entry still holds this SB's MODE-DECISION recon
//! (only earlier SBs carry encode-pass pixels — C's cross-SB reading of
//! `recon_pic`), splicing in decode order reproduces C exactly: within an
//! SB the txb predictions read the already-encoded neighbours' normed
//! recon, while the next SB's mode decision sees this SB fully normed.
//!
//! `ec_ctx_array` ordering: C updates the per-SB coefficient CDF chain on
//! the encode pass (`update_coeff_cdf`, coding_loop.c:1713), so the
//! caller runs this BEFORE the `funnel_chain` recode — the chain then
//! codes the same normed levels a decoder reads.
//!
//! Not yet ported (same brief's measured gaps):
//! - chroma: C's encode pass also re-quantizes cb/cr with the
//!   `is_encode_pass` light-RDOQ arm — the port keeps MD chroma levels.
//! - inter/IBC/palette leaves: C re-quantizes them identically; here they
//!   keep their MD levels (their committed cul still enters the neighbour
//!   chain, as it does in C for every coded leaf).

use super::*;
use crate::partition::PartitionTree;
use svtav1_types::partition::PartitionType;

/// C `ep_luma_dc_sign_level_coeff_na`: the `(dc_sign << 6) | cul` byte per
/// 4x4 position, `NEIGHBOR_ARRAY_INVALID` (0xFF) where nothing coded yet —
/// `reset_encode_pass_neighbor_arrays` (enc_dec_process.c:185) resets it
/// per tile, so the caller constructs this per tile.
pub(crate) struct EncPassCul {
    above: Vec<u8>,
    left: Vec<u8>,
}

impl EncPassCul {
    pub(crate) fn new(w_px: usize, h_px: usize) -> Self {
        Self {
            above: vec![0xFF; w_px.div_ceil(4)],
            left: vec![0xFF; h_px.div_ceil(4)],
        }
    }
    fn above_span(&self, x: usize, w: usize) -> &[u8] {
        let x4 = x / 4;
        &self.above[x4..(x4 + w / 4).min(self.above.len())]
    }
    fn left_span(&self, y: usize, h: usize) -> &[u8] {
        let y4 = y / 4;
        &self.left[y4..(y4 + h / 4).min(self.left.len())]
    }
    fn record(&mut self, x: usize, y: usize, w: usize, h: usize, cul: u8) {
        let (x4, y4) = (x / 4, y / 4);
        let right = (x4 + w / 4).min(self.above.len());
        let bottom = (y4 + h / 4).min(self.left.len());
        self.above[x4..right].fill(cul);
        self.left[y4..bottom].fill(cul);
    }
}

/// Splice one encode-pass txb recon into the canvas — `commit_leaf`'s
/// straddle clip (task #95): rows past the aligned width would wrap into
/// the next row's low columns and corrupt an already-committed neighbour.
fn write_canvas(
    recon: &mut [u8],
    stride: usize,
    x: usize,
    y: usize,
    w: usize,
    src: &[u8],
    srcw: usize,
    h: usize,
) {
    let wr = w.min(stride.saturating_sub(x));
    for r in 0..h {
        let dst = (y + r) * stride + x;
        recon[dst..dst + wr].copy_from_slice(&src[r * srcw..r * srcw + wr]);
    }
}

/// Entry: run the encode pass over one superblock's committed tree. `recon`
/// is the tile's frame canvas (stride `recon_stride`, absolute px coords);
/// `src`/`src_stride` is the SB-extent-padded input the funnel evaluated
/// against. `rdoq` is the SB's resolved `ctx->rdoq_ctrls` row (the encode
/// pass reads the same signal table mode decision left behind).
#[allow(clippy::too_many_arguments)]
pub(crate) fn encode_pass_luma_sb(
    tree: &mut PartitionTree,
    recon: &mut [u8],
    recon_stride: usize,
    src: &[u8],
    src_stride: usize,
    frame: &FunnelFrame,
    rates: &MdRates,
    rdoq: RdoqCtrls,
    filt_modes: &crate::pipeline::EntropyCtx,
    cul: &mut EncPassCul,
    sb_x: usize,
    sb_y: usize,
) {
    // Same qt construction as the eval path (leaf_funnel/mod.rs:389-390):
    // the frame's luma QM level is stamped onto the table — C's
    // `qmatrix_level` resolve (`frm_hdr.quantization_params.qm[PLANE_Y]`).
    let mut qt = crate::quant::build_quant_table_sharp(frame.quant_qindex(0), frame.sharpness);
    qt.qm_level = frame.qm_levels[0];
    encode_pass_node(
        tree,
        sb_x,
        sb_y,
        recon,
        recon_stride,
        src,
        src_stride,
        &qt,
        rdoq,
        frame,
        rates,
        filt_modes,
        cul,
        PartitionType::None,
    );
}

#[allow(clippy::too_many_arguments)]
fn encode_pass_node(
    tree: &mut PartitionTree,
    x: usize,
    y: usize,
    recon: &mut [u8],
    recon_stride: usize,
    src: &[u8],
    src_stride: usize,
    qt: &crate::quant::QuantTable,
    rdoq: RdoqCtrls,
    frame: &FunnelFrame,
    rates: &MdRates,
    filt_modes: &crate::pipeline::EntropyCtx,
    cul: &mut EncPassCul,
    parent_partition: PartitionType,
) {
    match tree {
        PartitionTree::Leaf(d) => encode_pass_leaf(
            d,
            x,
            y,
            recon,
            recon_stride,
            src,
            src_stride,
            qt,
            rdoq,
            frame,
            rates,
            filt_modes,
            cul,
            parent_partition,
        ),
        PartitionTree::Split {
            partition_type,
            width,
            height,
            children,
        } => {
            // Child origins — `bd10_reencode_node`'s exact rule (its comment
            // explains the partial-SB shapes this must survive: SPLIT prunes
            // out-of-frame quadrants, HORZ/VERT can carry one child, extended
            // shapes drop from the tail).
            let (nw, nh) = (*width as usize, *height as usize);
            let (hw, hh, qw, qh) = (nw / 2, nh / 2, nw / 4, nh / 4);
            let mut recurse = |child: &mut PartitionTree, cx, cy| {
                encode_pass_node(
                    child,
                    cx,
                    cy,
                    recon,
                    recon_stride,
                    src,
                    src_stride,
                    qt,
                    rdoq,
                    frame,
                    rates,
                    filt_modes,
                    cul,
                    match partition_type {
                        PartitionType::VertA => PartitionType::VertA,
                        PartitionType::VertB => PartitionType::VertB,
                        _ => PartitionType::None,
                    },
                );
            };
            match *partition_type {
                PartitionType::Split => {
                    let mut ci = 0usize;
                    for i in 0..4usize {
                        let cx = x + (i & 1) * hw;
                        let cy = y + (i >> 1) * hh;
                        if cx >= frame.frame_w_px || cy >= frame.frame_h_px {
                            continue;
                        }
                        recurse(&mut children[ci], cx, cy);
                        ci += 1;
                    }
                    debug_assert_eq!(ci, children.len());
                }
                PartitionType::Horz => {
                    let (first, rest) = children.split_at_mut(1);
                    recurse(&mut first[0], x, y);
                    if let Some(bot) = rest.first_mut() {
                        recurse(bot, x, y + hh);
                    }
                }
                PartitionType::Vert => {
                    let (first, rest) = children.split_at_mut(1);
                    recurse(&mut first[0], x, y);
                    if let Some(right) = rest.first_mut() {
                        recurse(right, x + hw, y);
                    }
                }
                ext => {
                    let offs: &[(usize, usize)] = match ext {
                        PartitionType::HorzA => &[(0, 0), (hw, 0), (0, hh)],
                        PartitionType::HorzB => &[(0, 0), (0, hh), (hw, hh)],
                        PartitionType::VertA => &[(0, 0), (0, hh), (hw, 0)],
                        PartitionType::VertB => &[(0, 0), (hw, 0), (hw, hh)],
                        PartitionType::Horz4 => &[(0, 0), (0, qh), (0, 2 * qh), (0, 3 * qh)],
                        PartitionType::Vert4 => &[(0, 0), (qw, 0), (2 * qw, 0), (3 * qw, 0)],
                        other => panic!("encode pass: unsupported partition {other:?}"),
                    };
                    for (child, &(dx, dy)) in children.iter_mut().zip(offs) {
                        recurse(child, x + dx, y + dy);
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn encode_pass_leaf(
    d: &mut crate::partition::BlockDecision,
    x: usize,
    y: usize,
    recon: &mut [u8],
    recon_stride: usize,
    src: &[u8],
    src_stride: usize,
    qt: &crate::quant::QuantTable,
    rdoq: RdoqCtrls,
    frame: &FunnelFrame,
    rates: &MdRates,
    filt_modes: &crate::pipeline::EntropyCtx,
    cul: &mut EncPassCul,
    parent_partition: PartitionType,
) {
    let (bw, bh) = (usize::from(d.width), usize::from(d.height));

    if d.inter.is_some() || d.use_intrabc || d.palette.is_some() {
        // C's encode pass re-quantizes these too; the port keeps their MD
        // levels (module doc). Their committed cul still enters the
        // neighbour chain — C stamps it for every leaf.
        if d.tx_depth == 0 {
            cul.record(x, y, bw, bh, compute_cul_level(&d.qcoeffs));
        } else {
            let (txw, txh) = txb_dims_at_depth(bw, bh, d.tx_depth);
            let cols = (bw / txw).max(1);
            for (i, q) in d.txb_qcoeffs.iter().enumerate() {
                cul.record(
                    x + (i % cols) * txw,
                    y + (i / cols) * txh,
                    txw,
                    txh,
                    compute_cul_level(q),
                );
            }
        }
        return;
    }

    let geom = UnitGeom {
        partition: parent_partition,
        mi_row: y >> 2,
        mi_col: x >> 2,
        bw_px: bw,
        bh_px: bh,
        ss: 0,
        frame_w: frame.frame_w_px,
        frame_h: frame.frame_h_px,
        sb_mi_size: frame.sb_mi_size,
        tile: filt_modes.tile_mi,
    };
    // C's encode pass reads `blk_ptr->block_mi.filt_type` — `get_filt_type`
    // over the committed coded modes, which the funnel's live EntropyCtx
    // holds once the SB's walk is done.
    let filt = filt_modes.filt_type_y(x, y);
    let intra_dir = if d.filter_intra_mode != FI_NONE {
        usize::from(FIMODE_TO_INTRADIR[d.filter_intra_mode as usize])
    } else {
        usize::from(d.intra_mode)
    };
    // C `ed_ctx->md_skip_blk` (coding_loop.c:387): an MD-committed
    // no-coefficient leaf is force-zeroed WITHOUT quantizing — the fresh
    // encode-pass residual could otherwise resurrect levels a decoder
    // never reads (the same trap `bd10_reencode`'s `committed_no_coeffs`
    // arm exists for). Recon is the pure prediction.
    let committed_zero = d.eob == 0
        && d.txb_eobs.iter().all(|&e| e == 0)
        && d.qcoeffs.iter().all(|&v| v == 0)
        && d.txb_qcoeffs.iter().all(|q| q.iter().all(|&v| v == 0));

    // The txb-local cul overlay: seeded at the leaf origin, updated per txb
    // — `txb_ctx_from_spans`' contract (`loc_above`/`loc_left` in
    // mds3/tx_depth.rs).
    let mut loc_above: Vec<u8> = cul.above_span(x, bw).to_vec();
    let mut loc_left: Vec<u8> = cul.left_span(y, bh).to_vec();

    if d.tx_depth == 0 {
        let mut pred = alloc::vec![0u8; bw * bh];
        predict_unit(
            recon,
            recon_stride,
            x,
            y,
            bw,
            bh,
            d.intra_mode,
            d.angle_delta,
            d.filter_intra_mode,
            &geom,
            frame.cfg.edge_filter,
            filt,
            &mut None,
            &mut pred,
        );
        if committed_zero {
            write_canvas(recon, recon_stride, x, y, bw, &pred, bw, bh);
            cul.record(x, y, bw, bh, 0);
            d.tx_type = 0;
            return;
        }
        // Encode-pass ctxs are unconditional (coding_loop.c:776
        // `svt_aom_get_txb_ctx` on `ep_luma_dc_sign_level_coeff_na`) —
        // unlike the eval's `cfg.real_coeff_ctx`-gated derivation.
        let (tsc, dsc) = txb_ctx_from_spans(&loc_above, &loc_left, 0, 0, bw, bh, true);
        let out = tx_unit_gated(
            src,
            src_stride,
            y * src_stride + x,
            &pred,
            bw,
            0,
            bw,
            bh,
            usize::from(d.tx_type),
            0,
            tsc,
            dsc,
            intra_dir,
            qt,
            frame,
            rates,
            rdoq,
            false,
            (bw, bh),
            true,
            RateMode::Exact,
            TxGate {
                enc_pass: true,
                ..TxGate::default()
            },
        );
        // `d.qcoeffs` is the FULL w x h raster (funnel_block_decision's
        // unpack); `out.qcoeff` is the 32-capped packed txb.
        let (pw, ph) = (bw.min(32), bh.min(32));
        d.qcoeffs.clear();
        d.qcoeffs.resize(bw * bh, 0);
        for r in 0..ph {
            d.qcoeffs[r * bw..r * bw + pw].copy_from_slice(&out.qcoeff[r * pw..r * pw + pw]);
        }
        d.eob = out.eob;
        // C `av1_encode_loop`: `blk_ptr->tx_type[txb_itr] = DCT_DCT` when
        // the encode-pass eob is 0 (coding_loop.c:443).
        if out.eob == 0 {
            d.tx_type = 0;
        }
        write_canvas(recon, recon_stride, x, y, bw, &out.recon, bw, bh);
        cul.record(x, y, bw, bh, out.cul);
        return;
    }

    // tx depth > 0: per-txb raster order, partial-recon overlay.
    let (txw, txh) = txb_dims_at_depth(bw, bh, d.tx_depth);
    let cols = (bw / txw).max(1);
    let txbs = d.txb_qcoeffs.len().min(cols * (bh / txh));
    // Seed the in-block overlay with the canvas's CURRENT (MD-committed)
    // pixels: C's `ep_luma_recon_na` is seeded from `recon_pic` at SB entry,
    // so a txb whose directional reference reaches a not-yet-encoded in-block
    // position (extended DR edges read up to 2*txw/2*txh past the unit) sees
    // the pre-pass recon there — not 0.
    let mut dep_recon = alloc::vec![0u8; bw * bh];
    let wr = bw.min(recon_stride.saturating_sub(x));
    for r in 0..bh {
        let src_row = &recon[(y + r) * recon_stride + x..(y + r) * recon_stride + x + wr];
        dep_recon[r * bw..r * bw + wr].copy_from_slice(src_row);
        if let Some(&last) = src_row.last() {
            dep_recon[r * bw + wr..r * bw + bw].fill(last);
        }
    }
    let mut eob_sum = 0u32;
    for i in 0..txbs {
        let (tx_x, tx_y) = ((i % cols) * txw, (i / cols) * txh);
        let mut pred = alloc::vec![0u8; txw * txh];
        predict_unit_overlay(
            recon,
            recon_stride,
            x,
            y,
            &dep_recon,
            bw,
            bh,
            tx_x,
            tx_y,
            txw,
            txh,
            d.intra_mode,
            d.angle_delta,
            d.filter_intra_mode,
            &geom,
            frame.cfg.edge_filter,
            filt,
            &mut pred,
        );
        if committed_zero {
            for r in 0..txh {
                let dst = (tx_y + r) * bw + tx_x;
                dep_recon[dst..dst + txw].copy_from_slice(&pred[r * txw..r * txw + txw]);
            }
            write_canvas(
                recon,
                recon_stride,
                x + tx_x,
                y + tx_y,
                txw,
                &pred,
                txw,
                txh,
            );
            d.txb_qcoeffs[i].iter_mut().for_each(|v| *v = 0);
            d.txb_eobs[i] = 0;
            d.txb_tx_types[i] = 0;
            cul.record(x + tx_x, y + tx_y, txw, txh, 0);
            continue;
        }
        let (tsc, dsc) = txb_ctx_from_spans(&loc_above, &loc_left, tx_x, tx_y, txw, txh, false);
        let out = tx_unit_gated(
            src,
            src_stride,
            (y + tx_y) * src_stride + x + tx_x,
            &pred,
            txw,
            0,
            txw,
            txh,
            usize::from(d.txb_tx_types[i]),
            0,
            tsc,
            dsc,
            intra_dir,
            qt,
            frame,
            rates,
            rdoq,
            false,
            (txw, txh),
            true,
            RateMode::Exact,
            TxGate {
                enc_pass: true,
                ..TxGate::default()
            },
        );
        d.txb_qcoeffs[i].clear();
        d.txb_qcoeffs[i].extend_from_slice(&out.qcoeff);
        d.txb_eobs[i] = out.eob;
        if out.eob == 0 {
            d.txb_tx_types[i] = 0;
        }
        eob_sum += u32::from(out.eob);
        for r in 0..txh {
            let dst = (tx_y + r) * bw + tx_x;
            dep_recon[dst..dst + txw].copy_from_slice(&out.recon[r * txw..r * txw + txw]);
        }
        write_canvas(
            recon,
            recon_stride,
            x + tx_x,
            y + tx_y,
            txw,
            &out.recon,
            txw,
            txh,
        );
        cul.record(x + tx_x, y + tx_y, txw, txh, out.cul);
        // leaf-local overlay so the NEXT txb of this leaf reads this cul.
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
    }
    if !committed_zero {
        d.eob = eob_sum.min(u16::MAX as u32) as u16;
    }
}
