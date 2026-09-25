use super::*;

/// Encode one chroma plane's block for the 4:2:0 path: UV_DC prediction
/// from the live chroma reconstruction plane, full-block DCT-DCT transform
/// and quantization at the SAME qindex tables as luma (the frame header
/// signals DeltaQUDc = DeltaQUAc = 0, so the decoder dequantizes chroma
/// with the identical step sizes), reconstructing into the plane.
///
/// `src`/`recon` are full (w/2 x h/2) chroma planes with `stride`; the
/// block lives at chroma coords (cx, cy) with chroma dims (cw, ch).
/// Neighbor extraction reuses the C-exact edge fill (127/129/left[0]/
/// above[0] rules) on the chroma plane; DC prediction and the
/// transform/quant/recon cycle are the same decoder-mirrored paths the
/// luma side uses. Must be called in coding order — the prediction reads
/// previously reconstructed chroma neighbors exactly as the decoder will.
///
/// Returns (qcoeffs raster cw x ch, eob) for the entropy writer.
///
/// `cq`: the frame-level C-exact coding quantizer (still path). C's MDS3
/// runs RDOQ on chroma too when enc-dec is bypassed (`md_stage_3` clears
/// `rdoq_ctrls.skip_uv`, product_coding_loop.c) — plane_type 1 selects the
/// chroma cost tables and `plane_rd_mult` 13.
/// `tile_top` / `tile_left` are the CHROMA-plane pixel row/column where
/// the current tile starts (0/0 = single tile). Callers pass the luma
/// tile origin halved — exact since tile boundaries are always 64-luma-px
/// (SB-aligned) multiples and 4:2:0 chroma is exactly half resolution on
/// both axes.
#[allow(clippy::too_many_arguments)]
pub fn encode_chroma_block_dc(
    src: &[u8],
    recon: &mut [u8],
    stride: usize,
    cx: usize,
    cy: usize,
    cw: usize,
    ch: usize,
    qindex: u8,
    cq: Option<&crate::quant::CodingQuantCfg>,
    qm_level: u8,
    tile_top: usize,
    tile_left: usize,
    // This CHROMA plane's aligned extent — the reference-sample clamp
    // (`extract_neighbors_tiled`'s `plane_w`/`plane_h`).
    plane_w: usize,
    plane_h: usize,
) -> (alloc::vec::Vec<i32>, u16) {
    let nb = extract_neighbors_tiled(
        recon, stride, cx, cy, cw, ch, tile_top, tile_left, plane_w, plane_h,
    );
    let (above, left, _top_left, has_above, has_left) = nb.parts();

    let mut pred = alloc::vec![0u8; cw * ch];
    svtav1_dsp::intra_pred::predict_dc(&mut pred, cw, above, left, cw, ch, has_above, has_left);

    if qindex == 0 {
        // Coded-lossless: the decoder preempts EVERY plane's transform to
        // the 4x4 Walsh-Hadamard pair (`av1_get_tx_size` -> TX_4X4 at
        // xd->lossless; `av1_inverse_transform_block` -> iwht4x4), still
        // dequantizing with the qindex-0 table. `encode_block_tx_cq`'s DCT
        // arm would emit DCT-domain levels the decoder IWHTs into the
        // wrong pixels. Mirror `lossless_mono`'s
        // fwht -> transpose -> quantize_b -> iwht arm.
        debug_assert_eq!((cw, ch), (4, 4), "lossless TXBs are TX_4X4");
        let mut residual = [0i16; 16];
        for r in 0..4 {
            for c in 0..4 {
                residual[r * 4 + c] =
                    src[(cy + r) * stride + cx + c] as i16 - pred[r * 4 + c] as i16;
            }
        }
        let mut wht = [0i32; 16];
        svtav1_dsp::fwd_txfm::fwht4x4(&residual, &mut wht, 4);
        // Transpose — the scan convention of C `svt_av1_estimate_transform`'s
        // lossless arm (the WHT output is emitted transposed).
        let mut coeffs = [0i32; 16];
        for r in 0..4 {
            for c in 0..4 {
                coeffs[c * 4 + r] = wht[r * 4 + c];
            }
        }
        let qt = crate::quant::build_quant_table(0);
        let scan = crate::entropy::scan_tables::scan(crate::entropy::coeff_c::TX_4X4, 0);
        let mut q = alloc::vec![0i32; 16];
        let mut dq = [0i32; 16];
        let eob = crate::quant::quantize_b(&coeffs, scan, &qt, 0, &mut q, &mut dq);
        let pred16: [u16; 16] = core::array::from_fn(|i| u16::from(pred[i]));
        let mut decoded = [0u16; 16];
        svtav1_dsp::inv_txfm::highbd_iwht4x4_16_add(&dq, &pred16, 4, &mut decoded, 4, 8);
        for r in 0..4 {
            let dst = (cy + r) * stride + cx;
            for c in 0..4 {
                recon[dst + c] = decoded[r * 4 + c] as u8;
            }
        }
        return (q, eob);
    }

    let enc = crate::encode_loop::encode_block_tx_cq(
        &src[cy * stride + cx..],
        stride,
        &pred,
        cw,
        cw,
        ch,
        qindex,
        svtav1_types::transform::TxType::DctDct,
        cq,
        1,
        qm_level,
    );

    for r in 0..ch {
        let dst = (cy + r) * stride + cx;
        recon[dst..dst + cw].copy_from_slice(&enc.recon[r * cw..r * cw + cw]);
    }

    (enc.qcoeffs, enc.eob)
}

/// Inter-block chroma TXB: residual against an EXPLICIT predictor — the
/// block's motion-compensated chroma — the `decision.is_inter` twin of
/// [`encode_chroma_block_dc`]. The decoder codes no `uv_mode` for an inter
/// block: its chroma prediction is the block's own motion, so the residual
/// must be against that prediction or the streams disagree on every nonzero
/// coefficient. A skipped inter block's chroma recon is the prediction
/// itself, which eob==0 reproduces.
///
/// `pred` is the TXB's top-left sample inside a `pred_stride`-wide buffer
/// (the caller predicts the whole chroma plane block once and hands each
/// TXB its origin). `tx_type` is the decoder-derived chroma type
/// (`inter_uv_tx_type`: the covering luma txb's coded type filtered by the
/// chroma inter ext-tx set) — the forward transform must match the basis
/// the decoder inverts.
#[allow(clippy::too_many_arguments)]
pub fn encode_chroma_block_pred(
    src: &[u8],
    recon: &mut [u8],
    stride: usize,
    cx: usize,
    cy: usize,
    cw: usize,
    ch: usize,
    pred: &[u8],
    pred_stride: usize,
    qindex: u8,
    cq: Option<&crate::quant::CodingQuantCfg>,
    qm_level: u8,
    tx_type: svtav1_types::transform::TxType,
) -> (alloc::vec::Vec<i32>, u16) {
    if qindex == 0 {
        // Coded-lossless preempts EVERY plane's transform to the 4x4
        // Walsh-Hadamard pair — the same arm as `encode_chroma_block_dc`,
        // against the explicit predictor.
        debug_assert_eq!((cw, ch), (4, 4), "lossless TXBs are TX_4X4");
        let mut residual = [0i16; 16];
        for r in 0..4 {
            for c in 0..4 {
                residual[r * 4 + c] =
                    src[(cy + r) * stride + cx + c] as i16 - pred[r * pred_stride + c] as i16;
            }
        }
        let mut wht = [0i32; 16];
        svtav1_dsp::fwd_txfm::fwht4x4(&residual, &mut wht, 4);
        let mut coeffs = [0i32; 16];
        for r in 0..4 {
            for c in 0..4 {
                coeffs[c * 4 + r] = wht[r * 4 + c];
            }
        }
        let qt = crate::quant::build_quant_table(0);
        let scan = crate::entropy::scan_tables::scan(crate::entropy::coeff_c::TX_4X4, 0);
        let mut q = alloc::vec![0i32; 16];
        let mut dq = [0i32; 16];
        let eob = crate::quant::quantize_b(&coeffs, scan, &qt, 0, &mut q, &mut dq);
        let pred16: [u16; 16] =
            core::array::from_fn(|i| u16::from(pred[(i / 4) * pred_stride + i % 4]));
        let mut decoded = [0u16; 16];
        svtav1_dsp::inv_txfm::highbd_iwht4x4_16_add(&dq, &pred16, 4, &mut decoded, 4, 8);
        for r in 0..4 {
            let dst = (cy + r) * stride + cx;
            for c in 0..4 {
                recon[dst + c] = decoded[r * 4 + c] as u8;
            }
        }
        return (q, eob);
    }

    let enc = crate::encode_loop::encode_block_tx_cq(
        &src[cy * stride + cx..],
        stride,
        pred,
        pred_stride,
        cw,
        ch,
        qindex,
        tx_type,
        cq,
        1,
        qm_level,
    );

    for r in 0..ch {
        let dst = (cy + r) * stride + cx;
        recon[dst..dst + cw].copy_from_slice(&enc.recon[r * cw..r * cw + cw]);
    }

    (enc.qcoeffs, enc.eob)
}

/// Helper: extract neighbors from frame context and encode a single block.
///
/// `partition` is the partition type this block will be SIGNALED under
/// (the parent candidate's type; None for PARTITION_NONE leaves). It must
/// be the real one: the decoder selects different has_top_right /
/// has_bottom_left availability tables for PARTITION_VERT_A/VERT_B
/// children (libaom get_has_tr_table / get_has_bl_table), and coding a
/// directional block against the wrong availability makes the encoder
/// extend edges with pixels the decoder never sees.
///
/// `dc_only` restricts the intra candidate set to exactly {DC_PRED} — the
/// C `is_dc_only_safe` outcome on the still/PD1 fixed-tree path (C injects
/// no other candidate, so no cost compare runs; mode_decision.c:3633).
#[allow(clippy::too_many_arguments)]
pub(super) fn encode_with_neighbors(
    src: &[u8],
    src_stride: usize,
    recon: &mut [u8],
    recon_stride: usize,
    width: usize,
    height: usize,
    qindex: u8,
    config: &PartitionSearchConfig,
    abs_x: usize,
    abs_y: usize,
    ref_ctx: Option<&RefFrameCtx>,
    partition: svtav1_types::partition::PartitionType,
    dc_only: bool,
) -> PartitionResult {
    let nb = extract_neighbors_tiled(
        recon,
        recon_stride,
        abs_x,
        abs_y,
        width,
        height,
        config.tile_top_px,
        config.tile_left_px,
        config.aligned_w,
        config.aligned_h,
    );
    let (above, left, top_left, has_above, has_left) = nb.parts();
    encode_single_block(
        src,
        src_stride,
        recon,
        recon_stride,
        width,
        height,
        qindex,
        config,
        above,
        left,
        top_left,
        has_above,
        has_left,
        ref_ctx,
        abs_x,
        abs_y,
        partition,
        dc_only,
    )
}

/// Encode a single block with mode decision — tries multiple intra
/// prediction modes and picks the one with lowest RD cost.
/// When `ref_ctx` is provided, also tries inter prediction using ME.
///
/// Generate an inter prediction block from reference + MV with bilinear interpolation.
/// Supports full-pel, half-pel, and quarter-pel positions.
/// [`generate_inter_pred`] for the padded-reference gate in
/// `pipeline::inter_decision_probe`, which must drive the REAL prediction
/// path rather than a copy of it — a second implementation could read the
/// margin correctly while the live one still filled 128.
#[cfg(test)]
pub fn generate_inter_pred_for_test(
    rfc: &RefFrameCtx,
    mv: svtav1_types::motion::Mv,
    abs_x: usize,
    abs_y: usize,
    width: usize,
    height: usize,
) -> alloc::vec::Vec<u8> {
    generate_inter_pred(rfc, mv, abs_x, abs_y, width, height)
}

pub(super) fn generate_inter_pred(
    rfc: &RefFrameCtx,
    mv: svtav1_types::motion::Mv,
    abs_x: usize,
    abs_y: usize,
    width: usize,
    height: usize,
) -> alloc::vec::Vec<u8> {
    let n = width * height;
    let mut pred = alloc::vec![128u8; n];
    // C's 8-tap convolve over a REPLICATED-MARGIN reference
    // (`crate::inter_pred_arm`, which drives
    // `port_pd_pred::av1_inter_prediction_light_pd1`), replacing a homegrown
    // BILINEAR that also filled every out-of-frame sample with the constant
    // 128 — `docs/INTER-ENCODE-PLAN.md` §1s items 4 and 5. Both were
    // decoder-conformance defects AND mode-decision ones: §1t measured that
    // on this campaign's cell the correct MV matches EXACTLY only against a
    // replicated margin, so C's `skip = 1` was unreachable either way.
    let Some(rp) = rfc.y_padded else {
        // No padded reference means no inter prediction. REFUSE rather than
        // fall back to the fill this replaced.
        return pred;
    };
    crate::inter_pred_arm::predict_inter_luma(
        rp,
        abs_x,
        abs_y,
        width,
        height,
        mv,
        // EIGHTTAP_REGULAR in both directions. There is no interpolation
        // filter search on this path, and the pack writes the same 0 into
        // the SWITCHABLE symbol (`partition::InterDecision::interp_filters`),
        // so the prediction and the bitstream agree by construction.
        0,
        rfc.sb_size,
        rfc.pic_width,
        rfc.pic_height,
        &mut pred,
        width,
    );
    pred
}

/// Uses the provided `above`/`left`/`top_left` neighbor arrays for prediction.
/// `has_above`/`has_left` control DC prediction averaging (false at frame edges).
/// (Spec 05, Section 7.11.2)
pub(super) fn encode_single_block(
    src: &[u8],
    src_stride: usize,
    recon: &mut [u8],
    recon_stride: usize,
    width: usize,
    height: usize,
    qindex: u8,
    config: &PartitionSearchConfig,
    above: &[u8],
    left: &[u8],
    top_left: u8,
    has_above: bool,
    has_left: bool,
    ref_ctx: Option<&RefFrameCtx>,
    abs_x: usize,
    abs_y: usize,
    partition: svtav1_types::partition::PartitionType,
    dc_only: bool,
) -> PartitionResult {
    let n = width * height;
    // Mode-RD lambda: CLI-qp-calibrated closed form via the exact inverse
    // mapping (see qp_to_lambda's domain note — feeding the raw qindex
    // would scale lambda by ~2^48 and make rate dominate every decision).
    // Pre-existing wrinkle kept as-is: this recomputes an UNSCALED lambda
    // while the partition-level RD uses the speed-scaled one.
    let lambda =
        crate::rate_control::qp_to_lambda(crate::rate_control::qindex_to_qp(qindex)) as u64;

    // Try multiple intra modes via mode decision.
    // Number of candidates controlled by block size and spec 03 NIC rules.
    let block_size = if width >= 8 && height >= 8 {
        svtav1_types::block::BlockSize::Block8x8
    } else {
        svtav1_types::block::BlockSize::Block4x4
    };
    let all_candidates = crate::mode_decision::generate_intra_candidates(block_size);
    // Limit candidates per config.max_intra_candidates (spec 03: NIC)
    let max_cands = config
        .max_intra_candidates
        .min(if width <= 4 || height <= 4 { 3 } else { 13 });
    // dc_only = the C is_dc_only_safe gate fired: the candidate set is
    // exactly {DC_PRED} (generate_intra_candidates puts DC first), like
    // C's inject_intra_candidates with dc_cand_only_flag.
    let candidates = if dc_only {
        &all_candidates[..1]
    } else {
        &all_candidates[..max_cands.min(all_candidates.len())]
    };

    let mut best_enc = None;
    let mut best_cost = u64::MAX;
    let mut chose_inter = false;
    let mut chosen_mv = svtav1_types::motion::Mv::ZERO;
    // AV1 y_mode index of the winning intra candidate — MUST match what the
    // bitstream signals, or the decoder predicts with a different mode than
    // the one the residual was built against.
    let mut chosen_mode: u8 = 0;
    let mut chosen_tx: u8 = 0; // C TxType index (DCT_DCT = 0)

    for cand in candidates {
        let mut pred_block = alloc::vec![128u8; n];

        // Generate prediction for this mode
        match cand.mode {
            svtav1_types::prediction::PredictionMode::DcPred => {
                svtav1_dsp::intra_pred::predict_dc(
                    &mut pred_block,
                    width,
                    above,
                    left,
                    width,
                    height,
                    has_above,
                    has_left,
                );
            }
            svtav1_types::prediction::PredictionMode::VPred => {
                svtav1_dsp::intra_pred::predict_v(&mut pred_block, width, above, width, height);
            }
            svtav1_types::prediction::PredictionMode::HPred => {
                svtav1_dsp::intra_pred::predict_h(&mut pred_block, width, left, width, height);
            }
            svtav1_types::prediction::PredictionMode::SmoothPred => {
                svtav1_dsp::intra_pred::predict_smooth(
                    &mut pred_block,
                    width,
                    above,
                    left,
                    width,
                    height,
                );
            }
            svtav1_types::prediction::PredictionMode::PaethPred => {
                svtav1_dsp::intra_pred::predict_paeth(
                    &mut pred_block,
                    width,
                    above,
                    left,
                    top_left,
                    width,
                    height,
                );
            }
            svtav1_types::prediction::PredictionMode::SmoothVPred => {
                svtav1_dsp::intra_pred::predict_smooth_v(
                    &mut pred_block,
                    width,
                    above,
                    left,
                    0,
                    height,
                    width,
                );
            }
            svtav1_types::prediction::PredictionMode::SmoothHPred => {
                svtav1_dsp::intra_pred::predict_smooth_h(
                    &mut pred_block,
                    width,
                    above,
                    left,
                    width,
                    height,
                );
            }
            svtav1_types::prediction::PredictionMode::D45Pred
            | svtav1_types::prediction::PredictionMode::D67Pred
            | svtav1_types::prediction::PredictionMode::D135Pred
            | svtav1_types::prediction::PredictionMode::D113Pred
            | svtav1_types::prediction::PredictionMode::D157Pred
            | svtav1_types::prediction::PredictionMode::D203Pred => {
                let angle = match cand.mode {
                    svtav1_types::prediction::PredictionMode::D45Pred => 45,
                    svtav1_types::prediction::PredictionMode::D67Pred => 67,
                    svtav1_types::prediction::PredictionMode::D113Pred => 113,
                    svtav1_types::prediction::PredictionMode::D135Pred => 135,
                    svtav1_types::prediction::PredictionMode::D157Pred => 157,
                    svtav1_types::prediction::PredictionMode::D203Pred => 203,
                    _ => 45,
                };
                // Build the extended neighbor arrays exactly like the
                // decoder (libaom build_intra_predictors): real
                // above-right / bottom-left pixels where
                // has_top_right/has_bottom_left say they are decoded,
                // replication of the last real sample otherwise, and the
                // decoder's unavailable-edge fills — instead of the old
                // flat-128 padding the decoder never sees.
                //
                // `partition` is the type this block is SIGNALED under:
                // PARTITION_VERT_A/B children select the has_tr_vert_*/
                // has_bl_vert_* availability tables in the decoder
                // (libaom get_has_tr_table), everything else the generic
                // ones. The search DOES emit VERT_A/B (ext partitions,
                // preset <= 8) — passing None here coded VERT_A/B
                // D-mode children against above-right/bottom-left pixels
                // the decoder never has (recon-parity failures at
                // qindex >= 80 where ext partitions start winning).
                // The allocation includes spare superblock rows; those are
                // not decoded neighbors. Present the actual aligned canvas.
                let frame_rows = config.aligned_h.min(recon.len() / recon_stride);
                match crate::intra_edge::build_directional_edges(
                    &recon[..frame_rows * recon_stride],
                    recon_stride,
                    abs_x,
                    abs_y,
                    width,
                    height,
                    angle,
                    partition,
                    config.sb_mi_size,
                ) {
                    crate::intra_edge::DirEdges::Flat(v) => pred_block.fill(v),
                    crate::intra_edge::DirEdges::Edges {
                        above: ext_above,
                        left: ext_left,
                        top_left: ext_top_left,
                    } => {
                        svtav1_dsp::intra_pred::predict_directional(
                            &mut pred_block,
                            width,
                            &ext_above,
                            &ext_left,
                            ext_top_left,
                            width,
                            height,
                            angle,
                        );
                    }
                }
            }
            _ => {
                // Remaining directional modes and advanced modes — use DC as fallback
                svtav1_dsp::intra_pred::predict_dc(
                    &mut pred_block,
                    width,
                    above,
                    left,
                    width,
                    height,
                    has_above,
                    has_left,
                );
            }
        }

        // Encode with this prediction — try DCT-DCT first
        let enc_dct = crate::encode_loop::encode_block_tx_cq(
            src,
            src_stride,
            &pred_block,
            width,
            width,
            height,
            qindex,
            svtav1_types::transform::TxType::DctDct,
            config.c_quant.as_deref(),
            0,
            config.c_quant.as_deref().map_or(15, |c| c.qm_levels[0]),
        );
        let cost_dct = enc_dct.distortion + ((lambda * enc_dct.rate as u64) >> 8);

        if cost_dct < best_cost {
            best_cost = cost_dct;
            best_enc = Some(enc_dct);
            chosen_mode = cand.mode as u8;
            chosen_tx = svtav1_types::transform::TxType::DctDct as u8;
        }

        // RDO transform type selection for non-DC modes at sizes <= 16.
        // Gated by rdo_tx_decision (Spec 03: only at low presets) and
        // enable_adst (Spec 04: "ADST captures asymmetric energy").
        if config.rdo_tx_decision
            && config.enable_adst
            && width <= 16
            && height <= 16
            && cand.mode.is_intra()
        {
            // Select candidate TX types based on prediction mode
            let tx_candidates: &[svtav1_types::transform::TxType] = match cand.mode {
                svtav1_types::prediction::PredictionMode::VPred
                | svtav1_types::prediction::PredictionMode::D67Pred => {
                    // Vertical: ADST in column, DCT in row
                    &[svtav1_types::transform::TxType::AdstDct]
                }
                svtav1_types::prediction::PredictionMode::HPred
                | svtav1_types::prediction::PredictionMode::D203Pred => {
                    // Horizontal: DCT in column, ADST in row
                    &[svtav1_types::transform::TxType::DctAdst]
                }
                svtav1_types::prediction::PredictionMode::D45Pred
                | svtav1_types::prediction::PredictionMode::D135Pred => {
                    // Diagonal: ADST-ADST
                    &[svtav1_types::transform::TxType::AdstAdst]
                }
                svtav1_types::prediction::PredictionMode::PaethPred => {
                    // Paeth: try ADST-DCT
                    &[svtav1_types::transform::TxType::AdstDct]
                }
                _ => &[], // DC and smooth: DCT-DCT is optimal
            };

            for &alt_tx in tx_candidates {
                let enc_alt = crate::encode_loop::encode_block_tx_cq(
                    src,
                    src_stride,
                    &pred_block,
                    width,
                    width,
                    height,
                    qindex,
                    alt_tx,
                    config.c_quant.as_deref(),
                    0,
                    config.c_quant.as_deref().map_or(15, |c| c.qm_levels[0]),
                );
                let cost_alt = enc_alt.distortion + ((lambda * enc_alt.rate as u64) >> 8);
                if cost_alt < best_cost {
                    best_cost = cost_alt;
                    best_enc = Some(enc_alt);
                    chosen_mode = cand.mode as u8;
                    chosen_tx = alt_tx as u8;
                }
            }
        }
    }

    // Filter-intra candidates are NOT evaluated: the sequence header
    // signals enable_filter_intra = 0, so the bitstream cannot represent
    // them — using their prediction would diverge from the decoder.

    // Try inter prediction if a reference frame is available.
    // Runs hierarchical ME (full-pel + half-pel refinement) to find the best MV,
    // generates a bilinear-interpolated prediction, and compares RD cost.
    if let Some(rfc) = ref_ctx {
        let me_params = crate::motion_est::MeSearchParams {
            search_area_width: 16,
            search_area_height: 16,
            use_hme: false,
            subpel_level: 2, // half-pel + quarter-pel refinement
        };
        // Use spatial MV predictor from neighboring blocks as search center
        let center_mv = rfc.get_mv_predictor(abs_x, abs_y);
        let me_result = crate::motion_est::hierarchical_me_centered(
            src,
            src_stride,
            rfc.y_plane,
            rfc.stride,
            abs_x as i32,
            abs_y as i32,
            width,
            height,
            &me_params,
            rfc.pic_width,
            rfc.pic_height,
            center_mv,
        );

        // Generate inter prediction from reference + MV. NO OBMC blending:
        // the block signals `motion_mode = SimpleTranslation`, for which the
        // decoder never blends — a blended prediction here would be a
        // recon-vs-decode divergence wherever it fired (this path lights up
        // on the 4:4:4 inter arm; on 4:2:0 the funnel decides inter instead).
        let inter_pred = generate_inter_pred(rfc, me_result.mv, abs_x, abs_y, width, height);

        let enc_inter = crate::encode_loop::encode_block(
            src,
            src_stride,
            &inter_pred,
            width,
            width,
            height,
            qindex,
        );
        // Add MV rate overhead (~2 bytes for simple MVs)
        let mv_rate = if me_result.mv.x == 0 && me_result.mv.y == 0 {
            64 // zero MV: ~0.25 bits
        } else {
            256 // nonzero MV: ~1 bit for joint + magnitude
        };
        let inter_cost = enc_inter.distortion + ((lambda * (enc_inter.rate + mv_rate) as u64) >> 8);
        if inter_cost < best_cost {
            best_enc = Some(enc_inter);
            chose_inter = true;
            chosen_mv = me_result.mv;
        }
    }

    let enc = best_enc.unwrap_or_else(|| {
        let pred_block = alloc::vec![128u8; n];
        crate::encode_loop::encode_block_tx_cq(
            src,
            src_stride,
            &pred_block,
            width,
            width,
            height,
            qindex,
            svtav1_types::transform::TxType::DctDct,
            config.c_quant.as_deref(),
            0,
            config.c_quant.as_deref().map_or(15, |c| c.qm_levels[0]),
        )
    });

    // Task #95 straddle clip — the twin of `leaf_funnel::commit_leaf`'s. A
    // boundary block whose recon STRADDLES past the aligned width (the mono
    // fixed-tree path's single-edge rect at a thin right edge, e.g. a VERT
    // 32x64 at x=192 on an aligned-200 frame with 8 in-frame columns) would
    // write columns `abs_x..abs_x+width` into a row of stride `recon_stride`
    // (= the aligned width): the off-aligned columns spill past the row and
    // WRAP into the NEXT row's low columns, silently overwriting an already
    // committed neighbour's recon that the next SB ROW then reads as its
    // above intra reference — the encoder predicts from the wrapped pixels,
    // the decoder from the real ones, and every SB row after the first
    // decodes wrong from column 0 outward (MEASURED 2026-08-27: 200x136 mono
    // preset 6 qp 10 decoded at 27.9 dB, first SB row 55 dB, second row
    // 23 dB at column 0; 192x136 / 200x64 clean). Nothing reads past the
    // aligned extent (`extract_neighbors_tiled` clamps to `config.aligned_w`,
    // like the decoder's spec-7.11.2 replicate), so clipping the STORE is
    // byte-neutral wherever nothing straddles.
    let row_end = recon_stride.min(config.aligned_w);
    let wr = width.min(row_end.saturating_sub(abs_x));
    for r in 0..height {
        let dst = (abs_y + r) * recon_stride + abs_x;
        recon[dst..dst + wr].copy_from_slice(&enc.recon[r * width..r * width + wr]);
    }

    let decision = BlockDecision {
        partition_type: PartitionType::None,
        is_inter: chose_inter,
        // An inter leaf codes no intra y_mode — stamp DC_PRED, matching
        // the funnel's inter-winner convention (leaf_funnel/types.rs:602)
        // and the decoder's `av1_get_intra_mode_context`, which reads
        // DC_PRED for an inter NEIGHBOUR. Stamping the losing intra
        // candidate's mode would move the next intra block's mode ctx
        // onto a CDF row the decoder never selected — a tile desync.
        intra_mode: if chose_inter { 0 } else { chosen_mode },
        tx_type: if chose_inter { 0 } else { chosen_tx },
        mv: chosen_mv,
        // What this homegrown search actually decided, in the form the pack
        // codes. Everything DERIVED (`predmv`, `inter_mode_ctx`, `drl_ctx`)
        // is left to the pack, which reads the committed mode-info map —
        // see `InterDecision`. The three fixed values are honest about what
        // is unported rather than placeholders:
        //
        // * `mode` is always `NEWMV` and `ref_frame` always
        //   `{LAST_FRAME, NONE}` because this search injects exactly one
        //   candidate: one reference, one MV, no NEAREST/NEAR/GLOBAL and no
        //   compound. `drl_index` is 0 for the same reason.
        // * `interp_filters` is `EIGHTTAP_REGULAR` in both directions
        //   (packed 0). This HOMEGROWN search runs no interpolation-filter
        //   search (the funnel's MDS3 search, `leaf_funnel::ifs`, does not
        //   run on this path); the frame header signals SWITCHABLE, so the
        //   symbol IS coded and 0 is the filter this block's prediction was
        //   actually built with.
        // * `motion_mode` is `SimpleTranslation` with no projected or
        //   overlappable neighbours, which is what makes
        //   `motion_mode_allowed` resolve to it — OBMC and warped motion
        //   are unported on this path.
        inter: chose_inter.then(|| {
            alloc::boxed::Box::new(InterDecision {
                mode: svtav1_types::prediction::PredictionMode::NewMv,
                ref_frame: [1, -1],
                mv: [chosen_mv, svtav1_types::motion::Mv::ZERO],
                drl_index: 0,
                interp_filters: 0,
                motion_mode: crate::port_entropy_inter::modes::MotionMode::SimpleTranslation,
                num_proj_ref: 0,
                overlappable_neighbors: 0,
                skip_mode: false,
                comp_group_idx: 0,
                compound_idx: 0,
                interinter_comp_type: 0,
                interinter_mask_type: 0,
                interinter_wedge_index: 0,
                interinter_wedge_sign: false,
                is_interintra_used: false,
                interintra_mode: 0,
                use_wedge_interintra: false,
                interintra_wedge_index: 0,
                wm_params: Default::default(),
                wm_params_l1: Default::default(),
            })
        }),
        qcoeffs: enc.qcoeffs.to_vec(),
        eob: enc.eob,
        width: width as u16,
        height: height as u16,
        ..Default::default()
    };

    let tree = PartitionTree::Leaf(decision.clone());

    PartitionResult {
        partition_type: PartitionType::None,
        rd_cost: enc.distortion + ((enc.rate as u64) << 4),
        distortion: enc.distortion,
        rate: enc.rate,
        decisions: alloc::vec![decision],
        tree: Some(tree),
        num_blocks: 1,
    }
}
