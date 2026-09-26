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
//! The chroma half is ported too: per leaf (after the luma txbs, matching
//! C's per-block ordering) the encode pass re-predicts cb/cr on the ep
//! chroma canvases — CfL subsamples the LUMA ep recon the leaf just
//! wrote — re-quantizes through `tx_unit_gated` with `enc_pass` set (the
//! `is_encode_pass` arm ignores `rdoq_ctrls.skip_uv`/`dct_dct_only` and
//! gates `light_rdoq`), and stamps the ep chroma culs
//! (`ep_cb/cr_dc_sign_level_coeff_na`).
//!
//! Not yet ported (same brief's measured gaps):
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

/// Everything the chroma encode-pass arm needs per SB: the tile's chroma
/// recon canvases (the funnel's MD recon at SB entry, evolving to ep
/// recon leaf by leaf — `ep_cb/cr_recon_na`'s role), the chroma input
/// planes, and the per-plane ep cul chains
/// (`ep_cb/cr_dc_sign_level_coeff_na_update`).
pub(crate) struct ChromaEnc<'a> {
    pub u_recon: &'a mut [u8],
    pub v_recon: &'a mut [u8],
    pub u_src: &'a [u8],
    pub v_src: &'a [u8],
    /// Chroma plane stride (input and recon share it).
    pub c_stride: usize,
    pub cb_cul: &'a mut EncPassCul,
    pub cr_cul: &'a mut EncPassCul,
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
    chroma: Option<ChromaEnc<'_>>,
    sb_x: usize,
    sb_y: usize,
) {
    // Same qt construction as the eval path (leaf_funnel/mod.rs:389-390):
    // the frame's luma QM level is stamped onto the table — C's
    // `qmatrix_level` resolve (`frm_hdr.quantization_params.qm[PLANE_Y]`).
    let mut qt = crate::quant::build_quant_table_sharp(frame.quant_qindex(0), frame.sharpness);
    qt.qm_level = frame.qm_levels[0];
    let mut qt_u = crate::quant::build_quant_table_sharp(frame.quant_qindex(1), frame.sharpness);
    qt_u.qm_level = frame.qm_levels[1];
    let mut qt_v = crate::quant::build_quant_table_sharp(frame.quant_qindex(2), frame.sharpness);
    qt_v.qm_level = frame.qm_levels[2];
    let mut chroma = chroma;
    encode_pass_node(
        tree,
        sb_x,
        sb_y,
        recon,
        recon_stride,
        src,
        src_stride,
        &qt,
        &qt_u,
        &qt_v,
        rdoq,
        frame,
        rates,
        filt_modes,
        cul,
        chroma.as_mut(),
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
    qt_u: &crate::quant::QuantTable,
    qt_v: &crate::quant::QuantTable,
    rdoq: RdoqCtrls,
    frame: &FunnelFrame,
    rates: &MdRates,
    filt_modes: &crate::pipeline::EntropyCtx,
    cul: &mut EncPassCul,
    chroma: Option<&mut ChromaEnc<'_>>,
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
            qt_u,
            qt_v,
            rdoq,
            frame,
            rates,
            filt_modes,
            cul,
            chroma,
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
            let mut chroma = chroma;
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
                    qt_u,
                    qt_v,
                    rdoq,
                    frame,
                    rates,
                    filt_modes,
                    cul,
                    chroma.as_deref_mut(),
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
    qt_u: &crate::quant::QuantTable,
    qt_v: &crate::quant::QuantTable,
    rdoq: RdoqCtrls,
    frame: &FunnelFrame,
    rates: &MdRates,
    filt_modes: &crate::pipeline::EntropyCtx,
    cul: &mut EncPassCul,
    chroma: Option<&mut ChromaEnc<'_>>,
    parent_partition: PartitionType,
) {
    encode_pass_luma_leaf(
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
    );
    // C runs the chroma txb loop after the leaf's luma txbs
    // (coding_loop.c:881-1004) — the CfL arm below reads the ep LUMA
    // recon this leaf just wrote.
    if let Some(chroma) = chroma {
        encode_pass_chroma_leaf(
            d,
            x,
            y,
            recon,
            recon_stride,
            chroma,
            qt_u,
            qt_v,
            frame,
            rates,
            rdoq,
            filt_modes,
            parent_partition,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn encode_pass_luma_leaf(
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

/// The chroma half of one leaf's encode pass — the per-leaf chroma loop
/// in `svt_aom_encode_block` (coding_loop.c:881-1004) calling
/// `av1_encode_loop`'s chroma section (:460-590) and the
/// `av1_encode_generate_recon` chroma arm, plus the
/// `ep_cb/cr_dc_sign_level_coeff_na_update` stamps (:1608-1628). One txb
/// per leaf on this path: `svt_aom_uv_tx_count` is 1 for every funnel
/// bsize at 4:2:0 — `av1_get_max_uv_txsize` spans the whole chroma block
/// under the 32-px transform cap.
///
/// `recon`/`recon_stride` is the ep LUMA canvas the luma arm just wrote —
/// a CfL leaf's AC subsample reads it (`av1_encode_generate_cfl_prediction`
/// reads `recon_buffer->y_buffer`, which by then holds the encode-pass
/// luma output, not the MD recon).
#[allow(clippy::too_many_arguments)]
fn encode_pass_chroma_leaf(
    d: &mut crate::partition::BlockDecision,
    x: usize,
    y: usize,
    recon: &[u8],
    recon_stride: usize,
    chroma: &mut ChromaEnc<'_>,
    qt_u: &crate::quant::QuantTable,
    qt_v: &crate::quant::QuantTable,
    frame: &FunnelFrame,
    rates: &MdRates,
    rdoq: RdoqCtrls,
    filt_modes: &crate::pipeline::EntropyCtx,
    parent_partition: PartitionType,
) {
    // `chroma_dec` absent = `has_uv` false — C skips the whole chroma
    // section (no pred, no NA stamps).
    if d.chroma_dec.is_none() {
        return;
    }
    let (bw, bh) = (usize::from(d.width), usize::from(d.height));
    // ROUND_UV pair geometry — the eval path's chroma origin/dims
    // (leaf_funnel/mod.rs): `ccx = (ROUND_UV_TO(x,1)>>1)` with the pair
    // anchor, `cw = MAX(4,luma)/2`.
    let cx = ((x >> 3) << 3) / 2 + if bw >= 8 { (x % 8) / 2 } else { 0 };
    let cy = ((y >> 3) << 3) / 2 + if bh >= 8 { (y % 8) / 2 } else { 0 };
    let (cw, chh) = (bw.max(8) / 2, bh.max(8) / 2);

    if d.inter.is_some() || d.use_intrabc || d.palette.is_some() {
        // C re-quantizes their chroma too (the inter pred path — the port
        // keeps MD levels, same deferral as the luma arm). Their
        // committed `quant_dc` byte still stamps the ep NAs.
        let (u_q, v_q, _, _, _, _) = d.chroma_dec.as_ref().unwrap();
        let (uc, vc) = (compute_cul_level(u_q), compute_cul_level(v_q));
        chroma.cb_cul.record(cx, cy, cw, chh, uc);
        chroma.cr_cul.record(cx, cy, cw, chh, vc);
        return;
    }

    let uv_geom = UnitGeom {
        partition: parent_partition,
        mi_row: ((y >> 3) << 3) >> 2,
        mi_col: ((x >> 3) << 3) >> 2,
        bw_px: bw.max(8),
        bh_px: bh.max(8),
        ss: 1,
        frame_w: frame.frame_w_px,
        frame_h: frame.frame_h_px,
        sb_mi_size: frame.sb_mi_size,
        tile: filt_modes.tile_mi,
    };
    let filt_uv = filt_modes.filt_type_uv(x, y);
    // `svt_av1_predict_intra_block` runs with `mode = uv_mode == CFL ?
    // UV_DC_PRED : uv_mode` (coding_loop.c:932-934) — the CfL overwrite
    // below applies alpha over that DC base.
    let mode = if usize::from(d.uv_mode) == UV_CFL_PRED_IDX {
        0
    } else {
        d.uv_mode
    };
    let mut u_pred = alloc::vec![0u8; cw * chh];
    let mut v_pred = alloc::vec![0u8; cw * chh];
    predict_unit(
        chroma.u_recon,
        chroma.c_stride,
        cx,
        cy,
        cw,
        chh,
        mode,
        d.uv_angle_delta,
        FI_NONE,
        &uv_geom,
        frame.cfg.edge_filter,
        filt_uv,
        &mut None,
        &mut u_pred,
    );
    predict_unit(
        chroma.v_recon,
        chroma.c_stride,
        cx,
        cy,
        cw,
        chh,
        mode,
        d.uv_angle_delta,
        FI_NONE,
        &uv_geom,
        frame.cfg.edge_filter,
        filt_uv,
        &mut None,
        &mut v_pred,
    );
    if usize::from(d.uv_mode) == UV_CFL_PRED_IDX {
        // `av1_encode_generate_cfl_prediction` (coding_loop.c:198):
        // subsample the ep luma recon, subtract the average, predict over
        // the DC base with the COMMITTED alphas.
        let mut lr = alloc::vec![0u8; bw * bh];
        let wr = bw.min(recon_stride.saturating_sub(x));
        for r in 0..bh {
            let s = (y + r) * recon_stride + x;
            let row = &recon[s..s + wr];
            lr[r * bw..r * bw + wr].copy_from_slice(row);
            if let Some(&last) = row.last() {
                lr[r * bw + wr..r * bw + bw].fill(last);
            }
        }
        let mut ac =
            crate::vecpool::zeroed_pool::<i16>(svtav1_dsp::intra_pred::CFL_BUF_LINE * chh.max(1));
        cfl_ac_subsample(recon, recon_stride, &lr, x, y, bw, bh, &mut ac);
        svtav1_dsp::intra_pred::cfl_subtract_average(&mut ac, cw, chh);
        let alpha_cb = cfl_idx_to_alpha(d.cfl_alpha_idx, d.cfl_alpha_signs, 0);
        let alpha_cr = cfl_idx_to_alpha(d.cfl_alpha_idx, d.cfl_alpha_signs, 1);
        let mut cfl_pred = alloc::vec![0u8; cw * chh];
        svtav1_dsp::intra_pred::cfl_predict_lbd(
            &ac,
            &u_pred,
            cw,
            &mut cfl_pred,
            cw,
            alpha_cb,
            cw,
            chh,
        );
        u_pred = cfl_pred;
        let mut cfl_pred = alloc::vec![0u8; cw * chh];
        svtav1_dsp::intra_pred::cfl_predict_lbd(
            &ac,
            &v_pred,
            cw,
            &mut cfl_pred,
            cw,
            alpha_cr,
            cw,
            chh,
        );
        v_pred = cfl_pred;
    }

    // `svt_aom_get_txb_ctx` on the ep chroma NAs (coding_loop.c:903-922)
    // — single txb => block_eq_tx, coords already in chroma px.
    let (cb_tsc, cb_dsc) = txb_ctx_from_spans(
        chroma.cb_cul.above_span(cx, cw),
        chroma.cb_cul.left_span(cy, chh),
        0,
        0,
        cw,
        chh,
        true,
    );
    let (cr_tsc, cr_dsc) = txb_ctx_from_spans(
        chroma.cr_cul.above_span(cx, cw),
        chroma.cr_cul.left_span(cy, chh),
        0,
        0,
        cw,
        chh,
        true,
    );
    let tt = uv_tx_type(d.uv_mode, cw, chh);
    let aligned_dims = crate::frame_geom::FrameDims {
        true_w: frame.frame_w_px,
        true_h: frame.frame_h_px,
        aligned_w: frame.frame_w_px,
        aligned_h: frame.frame_h_px,
        ss_x: 1,
        ss_y: 1,
    };
    let uv_crop = crate::frame_geom::cropped_tx_dims_uv(&aligned_dims, cx, cy, cw, chh);
    let u_out = tx_unit_gated(
        chroma.u_src,
        chroma.c_stride,
        cy * chroma.c_stride + cx,
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
        qt_u,
        frame,
        rates,
        rdoq,
        false,
        uv_crop,
        true,
        RateMode::Exact,
        TxGate {
            enc_pass: true,
            ..TxGate::default()
        },
    );
    let v_out = tx_unit_gated(
        chroma.v_src,
        chroma.c_stride,
        cy * chroma.c_stride + cx,
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
        qt_v,
        frame,
        rates,
        rdoq,
        false,
        uv_crop,
        true,
        RateMode::Exact,
        TxGate {
            enc_pass: true,
            ..TxGate::default()
        },
    );

    // Commit: `blk_ptr->eob.u/v`, `quant` rasters, the recon both into the
    // ep canvases (`recon_pic`) and the stored `chroma_dec` the syntax
    // walk copies into its own planes, then the ep cul stamps
    // (`quant_dc.u/v` = the `(dc_sign<<6)|cul` byte).
    {
        let (u_q, v_q, u_eob, v_eob, u_rec, v_rec) = d.chroma_dec.as_mut().unwrap();
        *u_eob = u_out.eob;
        *v_eob = v_out.eob;
        u_q.clear();
        u_q.extend_from_slice(&u_out.qcoeff);
        v_q.clear();
        v_q.extend_from_slice(&v_out.qcoeff);
        u_rec.clear();
        u_rec.extend_from_slice(&u_out.recon);
        v_rec.clear();
        v_rec.extend_from_slice(&v_out.recon);
    }
    write_canvas(
        chroma.u_recon,
        chroma.c_stride,
        cx,
        cy,
        cw,
        &u_out.recon,
        cw,
        chh,
    );
    write_canvas(
        chroma.v_recon,
        chroma.c_stride,
        cx,
        cy,
        cw,
        &v_out.recon,
        cw,
        chh,
    );
    chroma.cb_cul.record(cx, cy, cw, chh, u_out.cul);
    chroma.cr_cul.record(cx, cy, cw, chh, v_out.cul);
}
