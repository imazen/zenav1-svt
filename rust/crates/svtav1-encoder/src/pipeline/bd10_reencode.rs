//! The 10-bit level re-encode pass, kept beside its pipeline caller.

use super::{Bd10CoeffNeighbors, Bd10ModeNeighbors};

/// The INTER arm's 10-bit prediction for one committed leaf — the single-
/// reference [`crate::inter_pred_arm::predict_inter_leaf_hbd`] or, for a
/// compound decision (`ref_frame[1] > 0`), the two-reference
/// [`crate::inter_pred_arm::predict_inter_yuv_hbd_compound`]. Compound
/// candidates are always `SimpleTranslation`, but a GLOBAL_GLOBALMV leaf
/// still warps — `is_wm` is per reference against that ref's own model
/// (C `av1_inter_prediction`, enc_inter_prediction.c:3276) — so the
/// compound arm routes through
/// [`crate::inter_pred_arm::predict_inter_yuv_warped_compound_hbd`] when
/// either model is above TRANSLATION.
#[allow(clippy::too_many_arguments)]
fn predict_inter_leaf_hbd_any(
    inter_refs: &[Option<&crate::picture::PaddedRef>; 8],
    ic: &crate::partition::InterDecision,
    x: usize,
    y: usize,
    bw: usize,
    bh: usize,
    sb_size: usize,
    frame_w: usize,
    frame_h: usize,
    bd: u8,
    want_chroma: bool,
    y_out: &mut [u16],
    y_stride: usize,
    u_out: &mut [u16],
    v_out: &mut [u16],
    uv_stride: usize,
) {
    let hbd0 = inter_refs[ic.ref_frame[0].max(0) as usize]
        .and_then(|p| p.hbd.as_ref())
        .expect(
            "an inter leaf reached the bd10 re-encode with no 10-bit reference \
             in the DPB; `bd10_tree_supported` is supposed to have refused the \
             frame before this point",
        );
    if ic.ref_frame[1] > 0 {
        let hbd1 = inter_refs[ic.ref_frame[1].max(0) as usize]
            .and_then(|p| p.hbd.as_ref())
            .expect(
                "a compound inter leaf reached the bd10 re-encode with no \
                 10-bit second reference in the DPB",
            );
        let is_wm = [
            crate::inter_pred_arm::inter_pred_uses_warp(
                ic.motion_mode,
                ic.mode as u8,
                bw,
                bh,
                &ic.wm_params,
            ),
            crate::inter_pred_arm::inter_pred_uses_warp(
                ic.motion_mode,
                ic.mode as u8,
                bw,
                bh,
                &ic.wm_params_l1,
            ),
        ];
        if is_wm[0] || is_wm[1] {
            let mut wm0 = ic.wm_params;
            let mut wm1 = ic.wm_params_l1;
            crate::inter_pred_arm::predict_inter_yuv_warped_compound_hbd(
                [
                    (
                        &hbd0.y,
                        hbd0.uv
                            .as_ref()
                            .map(|(u, v)| (u, v))
                            .filter(|_| want_chroma),
                    ),
                    (
                        &hbd1.y,
                        hbd1.uv
                            .as_ref()
                            .map(|(u, v)| (u, v))
                            .filter(|_| want_chroma),
                    ),
                ],
                &mut wm0,
                &mut wm1,
                is_wm,
                x,
                y,
                bw,
                bh,
                ic.mv,
                ic.interp_filters,
                sb_size,
                frame_w,
                frame_h,
                bd,
                y_out,
                y_stride,
                u_out,
                v_out,
                uv_stride,
            );
            return;
        }
        crate::inter_pred_arm::predict_inter_yuv_hbd_compound(
            [
                (
                    &hbd0.y,
                    if want_chroma {
                        hbd0.uv.as_ref().map(|(u, v)| (u, v))
                    } else {
                        None
                    },
                ),
                (
                    &hbd1.y,
                    if want_chroma {
                        hbd1.uv.as_ref().map(|(u, v)| (u, v))
                    } else {
                        None
                    },
                ),
            ],
            x,
            y,
            bw,
            bh,
            ic.mv,
            ic.interp_filters,
            sb_size,
            frame_w,
            frame_h,
            bd,
            y_out,
            y_stride,
            u_out,
            v_out,
            uv_stride,
        );
        return;
    }
    // OBMC's BASE prediction is the block's own simple translation; the
    // neighbour blend is applied by the caller in place (`ObmcCausal` itself
    // reaches `predict_inter_leaf_hbd` nowhere — it asserts on it).
    let mm_base = match ic.motion_mode {
        crate::port_entropy_inter::modes::MotionMode::ObmcCausal => {
            crate::port_entropy_inter::modes::MotionMode::SimpleTranslation
        }
        other => other,
    };
    crate::inter_pred_arm::predict_inter_leaf_hbd(
        &hbd0.y,
        if want_chroma {
            hbd0.uv.as_ref().map(|(u, v)| (u, v))
        } else {
            None
        },
        mm_base,
        crate::inter_pred_arm::inter_pred_uses_warp(
            ic.motion_mode,
            ic.mode as u8,
            bw,
            bh,
            &ic.wm_params,
        ),
        ic.wm_params,
        x,
        y,
        bw,
        bh,
        ic.mv[0],
        ic.interp_filters,
        sb_size,
        frame_w,
        frame_h,
        bd,
        y_out,
        y_stride,
        u_out,
        v_out,
        uv_stride,
    );
}

/// Returns the frame's 10-bit luma recon as an **SB-extent-sized, ALIGNED-
/// strided** canvas — the same shape the funnel's `tile_frame_recon10` has, and
/// for the same reason: a boundary leaf may STRADDLE the aligned extent, and
/// C's recon picture has SB-extent stride so the straddle lands in place. Here
/// the stride stays aligned (`w`) and the slack absorbs a right-straddle write's
/// wrap; the caller crops the in-frame `w * h` region for `last_recon10_y`.
/// On a 64-aligned frame the extent equals the aligned dims, so the buffer and
/// every write are byte-identical to the pre-partial-SB pass.
#[allow(clippy::too_many_arguments)]
pub(super) fn bd10_reencode_luma(
    all_trees: &mut [crate::partition::PartitionTree],
    sb_cols: usize,
    sb_size: usize,
    // ISSUE #18: the resolved tile grid. This pass runs POST-merge over the
    // whole frame in raster SB order, so unlike the per-tile funnel it has no
    // tile context to inherit and must derive each SB's tile itself. It used
    // `TileMi::whole_frame`, which let the bd10 predictor read across a tile
    // edge that a conforming decoder cannot see. Raster order is still fine
    // for the RECON reads (every in-tile above/left SB is already written);
    // only the AVAILABILITY had to become tile-scoped.
    tile_grid: &crate::entropy::obu::TileGrid,
    w: usize,
    h: usize,
    // The 10-bit SOURCE, padded to the SB extent at `src_stride` (the u16 twin
    // of `sb_input` / `in_stride`). A straddling leaf's residual gather reads
    // the full block width, so an ALIGNED-sized source would wrap into the next
    // row (right edge) or run past the plane (bottom right).
    src10: &[u16],
    src_stride: usize,
    base_qindex: u8,
    rdoq_level: u8,
    lambda_bd10: u64,
    // C `scs->allintra || scs->static_config.rtc` — the RDOQ plane rate
    // weight arm (`crate::quant::PLANE_RD_MULT`). FALSE on a video frame.
    allintra_rd_mult: bool,
    real_coeff_ctx: bool,
    edge_filter: bool,
    bd: u8,
    qm_level: u8,
    // [SVT_HDR_MODE] fork loop_filter_sharpness (static_config.sharpness). 0 in
    // mainline → the quant table is byte-identical to build_quant_table_bd.
    sharpness: i8,
    // The DPB's reference pictures, whose `hbd` twin the INTER arm predicts
    // from. `None` on a key frame.
    inter_refs: Option<&[Option<&crate::picture::PaddedRef>; 8]>,
) -> crate::EncodeResult<alloc::vec::Vec<u16>> {
    let fc = crate::entropy::context::FrameContext::new_default();
    let cfc = crate::entropy::coeff_c::CoeffFc::default_for_qindex(base_qindex);
    let rates = crate::leaf_funnel::build_md_rates(&fc, &cfc);
    let qt = crate::quant::build_quant_table_bd_sharp(base_qindex, bd, sharpness);
    let ext_w = w.div_ceil(sb_size) * sb_size;
    let ext_h = h.div_ceil(sb_size) * sb_size;
    // Seeded with the 10-bit DC default, NOT 0 — the seed the u8
    // `tile_frame_recon` (128) and the funnel's `tile_frame_recon10` (512)
    // both carry. The reason it is worth carrying: this buffer is now
    // SB-extent-SIZED, so `extract_neighbors_hbd`'s `idx < recon.len()` guard
    // admits slack-region indices that an ALIGNED-sized buffer rejected, and
    // rejecting meant "extend the last available sample" while admitting a
    // ZERO would mean predicting against black.
    // MEASURED byte-inert (2026-08-04) across the whole 198-cell partial-SB
    // eff-M9 grid — 0 of 198 cells changed verdict or byte count — so no read
    // reaches an unwritten cell today. Kept anyway: it costs nothing, it makes
    // the bd10 canvas agree with its u8 twin by construction instead of by
    // luck, and a `0` seed here is a silent wrong-pixels failure the moment one
    // does. (rust/CLAUDE.md: dead-looking translations stay, with the
    // measurement written down.)
    let mut recon10 = svtav1_types::try_vec![(128u16 << (bd - 8)); ext_w * ext_h]?;
    let mut coeff_neighbors = Bd10CoeffNeighbors::new(w, h)?;
    // C `get_filt_type` needs the neighbour MODES, which this pass used to
    // ignore (it passed `filt_type = 0`) -- correct only with the sequence
    // header's intra edge filter off, which is why the frame gate rejected
    // every directional leaf when it is on. See [`Bd10ModeNeighbors`].
    let mut mode_neighbors = Bd10ModeNeighbors::new(w, h)?;
    // The committed mode-info grid `obmc_nb_spans` reads for an OBMC leaf's
    // neighbour motion — the same `MvpMiEntry` layout `commit_leaf` stamps
    // during MD, rebuilt here from the committed trees. Built only when the
    // frame can carry inter leaves at all.
    let mi_stride = w / 4;
    let mut mi_grid =
        svtav1_types::try_vec![crate::intrabc_mvp::MvpMiEntry::default(); mi_stride * (h / 4)]?;
    if inter_refs.is_some() {
        for (sb_idx, tree) in all_trees.iter().enumerate() {
            let sb_col = sb_idx % sb_cols;
            let sb_row = sb_idx / sb_cols;
            stamp_inter_mi_grid(
                tree,
                sb_col * sb_size,
                sb_row * sb_size,
                svtav1_types::partition::PartitionType::None as u8,
                &mut mi_grid,
                mi_stride,
                w,
                h,
            );
        }
    }
    for (sb_idx, tree) in all_trees.iter_mut().enumerate() {
        let sb_col = sb_idx % sb_cols;
        let sb_row = sb_idx / sb_cols;
        let tile_mi = tile_grid.tile_mi_for_sb(sb_row, sb_col, sb_size, w, h);
        coeff_neighbors.enter_sb(sb_col * sb_size, sb_row * sb_size, sb_size, tile_mi);
        mode_neighbors.enter_sb(sb_col * sb_size, sb_row * sb_size, sb_size, tile_mi);
        bd10_reencode_node(
            base_qindex == 0,
            sb_size / 4,
            tree,
            sb_col * sb_size,
            sb_row * sb_size,
            &mut recon10,
            w,
            src10,
            src_stride,
            &qt,
            rdoq_level,
            lambda_bd10,
            allintra_rd_mult,
            &rates,
            real_coeff_ctx,
            &mut coeff_neighbors,
            edge_filter,
            w,
            h,
            bd,
            qm_level,
            tile_mi,
            svtav1_types::partition::PartitionType::None,
            inter_refs,
            &mi_grid,
            mi_stride,
            &mut mode_neighbors,
        );
    }
    Ok(recon10)
}

#[allow(clippy::too_many_arguments)]
fn bd10_reencode_node(
    coded_lossless: bool,
    // C `seq_header.sb_mi_size` (16 SB64 / 32 SB128) — the intra
    // availability tables index by `mi & (sb_mi_size - 1)` (task #91).
    sb_mi_size: usize,
    tree: &mut crate::partition::PartitionTree,
    x: usize,
    y: usize,
    recon10: &mut [u16],
    stride: usize,
    src10: &[u16],
    src_stride: usize,
    qt: &crate::quant::QuantTable,
    rdoq_level: u8,
    lambda: u64,
    // C `scs->allintra || scs->static_config.rtc` — the RDOQ plane rate
    // weight arm (`crate::quant::PLANE_RD_MULT`). FALSE on a video frame.
    allintra_rd_mult: bool,
    rates: &crate::leaf_funnel::MdRates,
    real_coeff_ctx: bool,
    coeff_neighbors: &mut Bd10CoeffNeighbors,
    edge_filter: bool,
    frame_w: usize,
    frame_h: usize,
    bd: u8,
    qm_level: u8,
    // ISSUE #18: the tile CONTAINING this superblock, from
    // `TileGrid::tile_mi_for_sb`. Was `TileMi::whole_frame`.
    tile_mi: crate::intra_edge::TileMi,
    parent_partition: svtav1_types::partition::PartitionType,
    // The DPB's 10-bit reference pictures, for the INTER arm. `None` on a key
    // frame, where no leaf can be inter.
    inter_refs: Option<&[Option<&crate::picture::PaddedRef>; 8]>,
    // The committed mode-info grid the OBMC blend's neighbour walk reads
    // (`stamp_inter_mi_grid`); `mi_stride` is `frame_w / 4`.
    mi_grid: &[crate::intrabc_mvp::MvpMiEntry],
    mi_stride: usize,
    // The neighbour MODE grid `get_filt_type` reads. See [`Bd10ModeNeighbors`].
    mode_neighbors: &mut Bd10ModeNeighbors,
) {
    use crate::partition::PartitionTree as Tr;
    use crate::partition::PartitionType as PT;
    match tree {
        Tr::Leaf(d) => {
            let bw = d.width as usize;
            let bh = d.height as usize;
            if coded_lossless {
                assert_eq!((bw, bh, d.tx_depth), (8, 8, 1));
                let geom = crate::leaf_funnel::UnitGeom {
                    partition: parent_partition,
                    mi_row: y / 4,
                    mi_col: x / 4,
                    bw_px: bw,
                    bh_px: bh,
                    sb_mi_size,
                    ss: 0,
                    frame_w,
                    frame_h,
                    tile: tile_mi,
                };
                let mut local = vec![0u16; 64];
                d.eob = 0;
                d.qcoeffs.clear();
                d.txb_qcoeffs.clear();
                d.txb_eobs.clear();
                d.txb_tx_types = vec![0; 4];
                for tx in 0..4 {
                    let (dx, dy) = ((tx & 1) * 4, (tx >> 1) * 4);
                    let mut pred = [0u16; 16];
                    crate::leaf_funnel::predict_unit_overlay_hbd(
                        recon10,
                        stride,
                        x,
                        y,
                        &local,
                        bw,
                        bh,
                        dx,
                        dy,
                        4,
                        4,
                        d.intra_mode,
                        d.angle_delta,
                        d.filter_intra_mode,
                        &geom,
                        edge_filter,
                        mode_neighbors.filt_type_y(x, y),
                        &mut pred,
                        bd,
                    );
                    let (tsc, dsc) = if real_coeff_ctx {
                        coeff_neighbors.contexts(x + dx, y + dy, 4, 4)
                    } else {
                        (0, 0)
                    };
                    let out = crate::leaf_funnel::tx_unit_hbd(
                        true,
                        src10,
                        src_stride,
                        (y + dy) * src_stride + x + dx,
                        &pred,
                        4,
                        0,
                        4,
                        4,
                        0,
                        0,
                        tsc,
                        dsc,
                        qt,
                        0,
                        lambda,
                        0,
                        allintra_rd_mult,
                        rates,
                        false,
                        bd,
                        qm_level,
                        None,
                    );
                    coeff_neighbors.record(x + dx, y + dy, 4, 4, out.cul);
                    d.eob += out.eob;
                    d.txb_eobs.push(out.eob);
                    d.txb_qcoeffs.push(out.qcoeff);
                    for r in 0..4 {
                        local[(dy + r) * 8 + dx..(dy + r) * 8 + dx + 4]
                            .copy_from_slice(&out.recon[r * 4..r * 4 + 4]);
                    }
                }
                for r in 0..8 {
                    recon10[(y + r) * stride + x..(y + r) * stride + x + 8]
                        .copy_from_slice(&local[r * 8..r * 8 + 8]);
                }
                mode_neighbors.record(x, y, bw, bh, d.intra_mode, d.uv_mode);
                return;
            }
            if d.tx_depth > 0 {
                bd10_reencode_leaf_txs(
                    d,
                    x,
                    y,
                    recon10,
                    stride,
                    src10,
                    src_stride,
                    qt,
                    rdoq_level,
                    lambda,
                    allintra_rd_mult,
                    rates,
                    real_coeff_ctx,
                    coeff_neighbors,
                    edge_filter,
                    frame_w,
                    frame_h,
                    bd,
                    qm_level,
                    tile_mi,
                    parent_partition,
                    sb_mi_size,
                    inter_refs,
                    mi_grid,
                    mi_stride,
                    mode_neighbors,
                );
                mode_neighbors.record(
                    x,
                    y,
                    bw,
                    bh,
                    if d.inter.is_some() { 0 } else { d.intra_mode },
                    if d.inter.is_some() { 0 } else { d.uv_mode },
                );
                return;
            }
            // Predict luma at 10-bit from the running 10-bit recon plane.
            let mut pred = alloc::vec![0u16; bw * bh];
            // Luma geom for directional prediction (ss=0; tx_depth 0 ⇒ tx==block,
            // row_off=col_off=0). filt_type is consulted only when edge_filter is
            // set, and the gate (`bd10_tree_supported`) admits directional leaves
            // ONLY when edge_filter is false — so 0 is inert here.
            let geom = crate::leaf_funnel::UnitGeom {
                partition: parent_partition,
                mi_row: y >> 2,
                mi_col: x >> 2,
                bw_px: bw,
                bh_px: bh,
                sb_mi_size,
                ss: 0,
                frame_w,
                frame_h,
                // ISSUE #18 (2026-09-02): this WAS
                // `TileMi::whole_frame(frame_w, frame_h)`. The re-encode runs
                // post-merge over the frame in raster SB order, so it has no
                // tile context to inherit — but "no context to inherit" is a
                // reason to DERIVE the tile, not to pretend there is one tile.
                // With `whole_frame` a block on a tile's own top row / left
                // column predicted from real pixels across the tile edge while
                // the decoder used the unavailable-edge fills, and everything
                // from the boundary onward drifted (MEASURED at preset 9/10/13:
                // gradient 256x256 q20 with 2 tile rows, 49,606 of 98,304
                // samples differ from aomdec; 0 after this line).
                //
                // The 2026-07-22 coverage-combos note that threading this was
                // "BYTE-INERT on the diverging cells" stands and is not a
                // contradiction: it was measured on the C-BYTE-PARITY axis,
                // where the divergence has a separate upstream cause (the
                // eff-M9 partition search picks a different tree at a tile
                // boundary at bd10 — see docs/coverage-combos-map.md). Byte
                // parity cannot see an encoder/decoder prediction MISMATCH at
                // all, which is why that measurement did not catch this.
                tile: tile_mi,
            };
            // THE INTER ARM. C at `hbd_md == 0` runs its mode decision at 8
            // bits and then rebuilds the 10-bit prediction in EncDec before the
            // residual — which is exactly what this pass models. Predicting an
            // inter leaf with `predict_unit_hbd_partition` instead would code
            // DC-based levels under inter syntax: a decoder desync, which is
            // why the gate used to reject the whole frame rather than let that
            // happen.
            match d.inter.as_deref() {
                Some(ic) => {
                    predict_inter_leaf_hbd_any(
                        inter_refs.expect(
                            "an inter leaf reached the bd10 re-encode on a frame with no DPB",
                        ),
                        ic,
                        x,
                        y,
                        bw,
                        bh,
                        sb_mi_size * 4,
                        frame_w,
                        frame_h,
                        bd,
                        false,
                        &mut pred,
                        bw,
                        &mut [],
                        &mut [],
                        0,
                    );
                    // An OBMC leaf's committed prediction is the base
                    // translation with the neighbours' predictions blended
                    // into the edges — the funnel applies
                    // `predict_obmc_in_place_hbd` after the base call, and
                    // this pass does the same from the reconstructed mi grid.
                    apply_obmc_hbd_postpass(
                        inter_refs.expect(
                            "an inter leaf reached the bd10 re-encode on a frame with no DPB",
                        ),
                        mi_grid,
                        mi_stride,
                        ic,
                        x,
                        y,
                        bw,
                        bh,
                        sb_mi_size * 4,
                        frame_w,
                        frame_h,
                        bd,
                        &mut pred,
                        bw,
                        &mut [],
                        &mut [],
                        0,
                    );
                }
                None => crate::leaf_funnel::predict_unit_hbd_partition(
                    recon10,
                    stride,
                    x,
                    y,
                    bw,
                    bh,
                    d.intra_mode,
                    d.angle_delta,
                    d.filter_intra_mode,
                    &geom,
                    edge_filter,
                    // C `get_filt_type(xd, 0)`. Was a hardcoded 0, which is
                    // only right with the edge filter off -- and that is
                    // exactly why the frame gate had to reject every
                    // directional leaf when it is on.
                    mode_neighbors.filt_type_y(x, y),
                    &mut pred,
                    bd,
                    parent_partition,
                ),
            }
            // Stamp this leaf into the neighbour grid, in decode order, so the
            // NEXT block's `get_filt_type` reads what a decoder reads. An inter
            // leaf codes no intra mode: C's `svt_aom_is_smooth` is false for
            // one, and 0 (DC_PRED) is the non-smooth value the grid is seeded
            // with, so it is the faithful stamp rather than a claim about DC.
            mode_neighbors.record(
                x,
                y,
                bw,
                bh,
                if d.inter.is_some() { 0 } else { d.intra_mode },
                if d.inter.is_some() { 0 } else { d.uv_mode },
            );
            // A `skip_mode` leaf is COMMITTED SYNTAX for a zero residual: the
            // decoder reads `skip_mode` = `skip_txfm`, no txbs, and
            // reconstructs the block as its prediction. The 8-bit mode
            // decision guarantees all-zero levels at the u8 quantizer, but the
            // 10-bit re-quantize below can resurrect a small residual the u8
            // table dropped — and then the writer emits coefficient sections
            // the decoder never reads: a tile desync, MEASURED as aomdec
            // "Failed to decode tile data" on johnny 256x256 q40 p6 f3
            // (2026-09-16), where the first diverging block was mi=(48,16), a
            // skip_mode compound leaf with a nonzero 10-bit luma eob. Keep the
            // committed zero residual: code nothing, reconstruct as the
            // prediction.
            if d.inter.as_deref().is_some_and(|ic| ic.skip_mode) {
                d.qcoeffs = alloc::vec![0i32; bw * bh];
                d.eob = 0;
                coeff_neighbors.record(x, y, bw, bh, 0);
                let wr = bw.min(stride.saturating_sub(x));
                for r in 0..bh {
                    let drow = (y + r) * stride + x;
                    recon10[drow..drow + wr].copy_from_slice(&pred[r * bw..r * bw + wr]);
                }
                return;
            }
            let src_off = y * src_stride + x;
            // C disables context updates at the faster presets. Otherwise
            // derive contexts from the native levels committed in decode order.
            let (txb_skip_ctx, dc_sign_ctx) = if real_coeff_ctx {
                coeff_neighbors.contexts(x, y, bw, bh)
            } else {
                (0, 0)
            };
            let out = crate::leaf_funnel::tx_unit_hbd(
                false, // This level-only post-pass currently accepts only lossy depth-0 trees.
                src10,
                src_stride,
                src_off,
                &pred,
                bw,
                0,
                bw,
                bh,
                d.tx_type as usize,
                0, // luma plane
                txb_skip_ctx,
                dc_sign_ctx,
                qt,
                rdoq_level,
                lambda,
                0, // sharpness
                allintra_rd_mult,
                rates,
                rdoq_level != 0,
                bd,
                qm_level,
                None, // level-only re-encode: no RD terms
            );
            // Overwrite the coded LUMA levels with the 10-bit result. The walk
            // re-derives the scan-order eob + skip from these coeffs.
            //
            // `out.qcoeff` is the TIGHT (32-capped) packed txb at stride pw; the
            // entropy walk (pipeline.rs `tx_depth==0` arm) — like the u8
            // `funnel_block_decision` (partition.rs) — expects `d.qcoeffs` as a
            // full w*h raster at stride w, from which it re-packs the low-freq
            // quadrant. Re-expand so 64-dim transforms (pw<w) don't read past
            // the tight buffer (was: a 64x64 DC leaf at high qindex panicked in
            // the walk's stride-w pack).
            let (pw, ph) = (bw.min(32), bh.min(32));
            let mut full = alloc::vec![0i32; bw * bh];
            for r in 0..ph {
                full[r * bw..r * bw + pw].copy_from_slice(&out.qcoeff[r * pw..r * pw + pw]);
            }
            d.qcoeffs = full;
            d.eob = out.eob;
            coeff_neighbors.record(x, y, bw, bh, out.cul);
            // Write the 10-bit recon back for neighbour prediction of the next
            // block in decode order.
            //
            // STRADDLE CLIP (task #94 partial-SB) — the same rule `commit_leaf`
            // applies to the funnel's canvases: a boundary leaf whose width
            // reaches past the ALIGNED extent would spill past the row boundary
            // and, this buffer being SB-extent-sized but aligned-strided, WRAP
            // into the next row's low columns, corrupting an already-committed
            // neighbour that a later block predicts from. Nothing ever READS
            // past the aligned extent, so clipping the write matches C's
            // readable recon exactly, and it is a no-op wherever
            // `x + bw <= stride` (every 64-aligned frame).
            let wr = bw.min(stride.saturating_sub(x));
            for r in 0..bh {
                let drow = (y + r) * stride + x;
                recon10[drow..drow + wr].copy_from_slice(&out.recon[r * bw..r * bw + wr]);
            }
        }
        Tr::Split {
            partition_type,
            width,
            height,
            children,
        } => {
            let nw = *width as usize;
            let nh = *height as usize;
            let hw = nw / 2;
            let hh = nh / 2;
            let qw = nw / 4;
            let qh = nh / 4;
            // Child origins, derived EXACTLY the way `encode_partition_tree`
            // derives them (the pack walk), because on a partial SB the child
            // list is no longer a fixed length:
            //   * SPLIT walks the four quadrant SLOTS and SKIPS any whose
            //     ORIGIN is outside the aligned frame, pulling the packed
            //     children in order. Zipping a pruned list against the full
            //     offset table mis-places them — a right-edge-only prune leaves
            //     [q0, q2] and would put the BOTTOM-LEFT child at the
            //     TOP-RIGHT offset.
            //   * HORZ/VERT may carry a single in-frame child (C codes block 1
            //     only if `mi_row + hbs < mi_rows`, entropy_coding.c:5490).
            //   * the extended shapes drop children from the TAIL, so a
            //     zip against the full list still pairs correctly.
            // The previous `(partition_type, children.len())` match would have
            // `panic!`ed on every one of those shapes.
            let mut recurse = |child: &mut crate::partition::PartitionTree, cx, cy| {
                bd10_reencode_node(
                    coded_lossless,
                    sb_mi_size,
                    child,
                    cx,
                    cy,
                    recon10,
                    stride,
                    src10,
                    src_stride,
                    qt,
                    rdoq_level,
                    lambda,
                    allintra_rd_mult,
                    rates,
                    real_coeff_ctx,
                    coeff_neighbors,
                    edge_filter,
                    frame_w,
                    frame_h,
                    bd,
                    qm_level,
                    // Children are inside the same superblock, hence the same
                    // tile (issue #18).
                    tile_mi,
                    match partition_type {
                        PT::VertA => svtav1_types::partition::PartitionType::VertA,
                        PT::VertB => svtav1_types::partition::PartitionType::VertB,
                        _ => svtav1_types::partition::PartitionType::None,
                    },
                    inter_refs,
                    mi_grid,
                    mi_stride,
                    mode_neighbors,
                );
            };
            match *partition_type {
                PT::Split => {
                    let mut ci = 0usize;
                    for i in 0..4usize {
                        let cx = x + (i & 1) * hw;
                        let cy = y + (i >> 1) * hh;
                        if cx >= frame_w || cy >= frame_h {
                            continue;
                        }
                        recurse(&mut children[ci], cx, cy);
                        ci += 1;
                    }
                    debug_assert_eq!(
                        ci,
                        children.len(),
                        "bd10 reencode: in-frame quadrant count must equal the packed child count"
                    );
                }
                PT::Horz => {
                    let (first, rest) = children.split_at_mut(1);
                    recurse(&mut first[0], x, y);
                    if let Some(bot) = rest.first_mut() {
                        recurse(bot, x, y + hh);
                    }
                }
                PT::Vert => {
                    let (first, rest) = children.split_at_mut(1);
                    recurse(&mut first[0], x, y);
                    if let Some(right) = rest.first_mut() {
                        recurse(right, x + hw, y);
                    }
                }
                ext => {
                    let offs: &[(usize, usize)] = match ext {
                        PT::HorzA => &[(0, 0), (hw, 0), (0, hh)],
                        PT::HorzB => &[(0, 0), (0, hh), (hw, hh)],
                        PT::VertA => &[(0, 0), (0, hh), (hw, 0)],
                        PT::VertB => &[(0, 0), (hw, 0), (hw, hh)],
                        PT::Horz4 => &[(0, 0), (0, qh), (0, 2 * qh), (0, 3 * qh)],
                        PT::Vert4 => &[(0, 0), (qw, 0), (2 * qw, 0), (3 * qw, 0)],
                        other => panic!("bd10 reencode: unsupported partition {other:?}"),
                    };
                    for (child, &(dx, dy)) in children.iter_mut().zip(offs) {
                        recurse(child, x + dx, y + dy);
                    }
                }
            }
        }
    }
}

/// Stamps the post-pass's own mode-info grid, read by
/// `inter_md_arm::predict_inter_chroma_sub8_hbd`: C's `inter_chroma_4xn_pred`
/// takes the covered 4x4 cells' motion out of `mi_grid` — which on this pass
/// is the COMMITTED tree, walked once into the same `MvpMiEntry` layout
/// `commit_leaf` maintains during MD. Intra leaves keep the default entry
/// (`ref_frame = {INTRA_FRAME, NONE_FRAME}` -> `!is_inter` -> the stitcher's
/// whole-area fallback, matching C's covered-cell check). The child-origin
/// derivation is `bd10_reencode_chroma_node`'s exactly.
fn stamp_inter_mi_grid(
    tree: &crate::partition::PartitionTree,
    x: usize,
    y: usize,
    partition: u8,
    grid: &mut [crate::intrabc_mvp::MvpMiEntry],
    stride: usize,
    lframe_w: usize,
    lframe_h: usize,
) {
    use crate::partition::PartitionTree as Tr;
    use crate::partition::PartitionType as PT;
    match tree {
        Tr::Leaf(d) => {
            let (bw, bh) = (d.width as usize, d.height as usize);
            let ic = d.inter.as_deref();
            let entry = crate::intrabc_mvp::MvpMiEntry {
                bsize: crate::leaf_funnel::c_bsize_index(bw, bh) as u8,
                mode: ic.map_or(d.intra_mode, |i| i.mode as u8),
                use_intrabc: d.use_intrabc,
                ref_frame: ic.map_or([0, -1], |i| i.ref_frame),
                mv: match (ic, d.use_intrabc) {
                    (Some(i), _) => i.mv,
                    (None, true) => [d.dv, svtav1_types::motion::Mv::default()],
                    (None, false) => [svtav1_types::motion::Mv::default(); 2],
                },
                partition,
                interp_filters: ic.map_or(0, |i| i.interp_filters),
                skip_mode: ic.is_some_and(|i| i.skip_mode),
                // `block_mi.skip` (`!block_has_coeff`); the bd10 re-encode
                // only feeds this grid to neighbour reads the light path
                // never reaches at hbd_md, so the luma-eob proxy is enough.
                skip: ic.is_some_and(|i| i.skip_mode) || d.eob == 0,
                comp_group_idx: ic.map_or(0, |i| i.comp_group_idx),
                compound_idx: ic.map_or(0, |i| i.compound_idx),
            };
            let (mi_x, mi_y) = (x / 4, y / 4);
            for my in mi_y..(mi_y + bh / 4).min(grid.len() / stride) {
                for cell in grid
                    [my * stride + mi_x..(my * stride + mi_x + bw / 4).min((my + 1) * stride)]
                    .iter_mut()
                {
                    *cell = entry;
                }
            }
        }
        Tr::Split {
            partition_type,
            width,
            height,
            children,
        } => {
            let nw = *width as usize;
            let nh = *height as usize;
            let (hw, hh, qw, qh) = (nw / 2, nh / 2, nw / 4, nh / 4);
            let child_part = match partition_type {
                PT::VertA => svtav1_types::partition::PartitionType::VertA,
                PT::VertB => svtav1_types::partition::PartitionType::VertB,
                _ => svtav1_types::partition::PartitionType::None,
            } as u8;
            let mut rec = |child: &crate::partition::PartitionTree, cx, cy| {
                stamp_inter_mi_grid(child, cx, cy, child_part, grid, stride, lframe_w, lframe_h);
            };
            match *partition_type {
                PT::Split => {
                    let mut ci = 0usize;
                    for i in 0..4usize {
                        let cx = x + (i & 1) * hw;
                        let cy = y + (i >> 1) * hh;
                        if cx >= lframe_w || cy >= lframe_h {
                            continue;
                        }
                        rec(&children[ci], cx, cy);
                        ci += 1;
                    }
                }
                PT::Horz => {
                    rec(&children[0], x, y);
                    if let Some(bot) = children.get(1) {
                        rec(bot, x, y + hh);
                    }
                }
                PT::Vert => {
                    rec(&children[0], x, y);
                    if let Some(right) = children.get(1) {
                        rec(right, x + hw, y);
                    }
                }
                ext => {
                    let offs: &[(usize, usize)] = match ext {
                        PT::HorzA => &[(0, 0), (hw, 0), (0, hh)],
                        PT::HorzB => &[(0, 0), (0, hh), (hw, hh)],
                        PT::VertA => &[(0, 0), (0, hh), (hw, 0)],
                        PT::VertB => &[(0, 0), (hw, 0), (hw, hh)],
                        PT::Horz4 => &[(0, 0), (0, qh), (0, 2 * qh), (0, 3 * qh)],
                        PT::Vert4 => &[(0, 0), (qw, 0), (2 * qw, 0), (3 * qw, 0)],
                        other => {
                            panic!("bd10 chroma reencode: unsupported partition {other:?}")
                        }
                    };
                    for (child, &(dx, dy)) in children.iter().zip(offs) {
                        rec(child, x + dx, y + dy);
                    }
                }
            }
        }
    }
}

/// The post-pass twin of the funnel's in-place OBMC blend. The committed
/// `mi_grid` (`stamp_inter_mi_grid`) supplies the neighbour spans
/// `obmc_nb_spans` reads, and the neighbours' 10-bit predictions are rebuilt
/// from the same DPB `hbd` twins the block's own base prediction used — so
/// the blend is the funnel's `predict_obmc_in_place_hbd` verbatim, driven by
/// reconstructed (not live-MD) neighbour state. An empty `u` blends luma
/// only (the luma walk); an empty `y` blends chroma only (the chroma walk,
/// which has no luma prediction buffer to write into).
#[allow(clippy::too_many_arguments)]
fn apply_obmc_hbd_postpass(
    inter_refs: &[Option<&crate::picture::PaddedRef>; 8],
    mi_grid: &[crate::intrabc_mvp::MvpMiEntry],
    mi_stride: usize,
    ic: &crate::partition::InterDecision,
    x: usize,
    y: usize,
    bw: usize,
    bh: usize,
    sb_size: usize,
    frame_w: usize,
    frame_h: usize,
    bd: u8,
    y_out: &mut [u16],
    y_stride: usize,
    u_out: &mut [u16],
    v_out: &mut [u16],
    uv_stride: usize,
) {
    if ic.motion_mode != crate::port_entropy_inter::modes::MotionMode::ObmcCausal {
        return;
    }
    let spans = crate::inter_md_arm::obmc_nb_spans(
        mi_grid,
        mi_stride as i32,
        (frame_h / 4) as i32,
        (frame_w / 4) as i32,
        x,
        y,
        bw,
        bh,
    );
    crate::obmc_pred_arm::predict_obmc_in_place_hbd(
        &crate::obmc_pred_arm::ObmcCtx {
            padded_by_ref: inter_refs,
            above_row: &spans.above[..spans.n_above],
            left_col: &spans.left[..spans.n_left],
            up_available: y > 0,
            left_available: x > 0,
            mi_cols: frame_w / 4,
            mi_rows: frame_h / 4,
            sb_size,
            frame_w,
            frame_h,
            edges: crate::inter_pred_arm::block_mb_edges(x, y, bw, bh, frame_w, frame_h),
        },
        svtav1_types::block::BlockSize::from_u8(crate::leaf_funnel::c_bsize_index(bw, bh) as u8)
            .expect("a committed inter leaf must have a real BlockSize"),
        x,
        y,
        bw,
        bh,
        bd,
        y_out,
        y_stride,
        u_out,
        v_out,
        uv_stride,
    );
}

/// bd10 CHROMA re-encode (task #94). The luma re-encode (`bd10_reencode_luma`)
/// recomputes only luma levels; chroma stays at the u8 MD decision
/// (`chroma_dec`). For content whose CHROMA has a coded residual (e.g. the
/// `diag` diagonal edge — its subsampled chroma is NOT flat), the u8 chroma
/// levels diverge from C's bd10 chroma quant: C's higher-precision chroma
/// prediction (the ~+20/px hbd-predictor rounding) yields a small DC residual
/// that quantizes to ±1 at bd10 where the MSB-truncated u8 path rounds to 0.
/// Decode-both localization proved the LUMA plane is already byte-identical
/// (`bd10_reencode_luma`) and every chroma divergence is exactly this (port
/// codes flat 512 where C codes a coded 511). This walk mirrors the luma pass
/// on the U and V planes: predict at bd10 (`predict_unit_hbd` on the running
/// bd10 chroma recon), residual/tx/quant at bd10 (`tx_unit_hbd`, plane 1, the
/// derived `uv_tx_type` + the bd10 chroma quant table), then OVERWRITE
/// `chroma_dec` with the bd10 levels/eob. Gated to complete-SB, in-envelope
/// trees (`bd10_tree_supported`, which now also rejects CfL / directional-uv-
/// with-edge-filter); flat-chroma content (gradient/uniform) re-encodes to the
/// SAME zero-coefficient result, so bd8 and the existing bd10 gate cells stay
/// byte-unchanged. The stored u8 recon in `chroma_dec` is inert (the walk only
/// copies it into the u8 chroma plane, which no `chroma_dec` block reads).
#[allow(clippy::too_many_arguments)]
pub(super) fn bd10_reencode_chroma(
    all_trees: &mut [crate::partition::PartitionTree],
    sb_cols: usize,
    sb_size: usize,
    // ISSUE #18: see the luma twin. Chroma's `UnitGeom` here is built with
    // `ss: 0` over the CHROMA plane's own dims, so the TileMi handed down is
    // derived in that same domain (`sb_size / 2`, `cframe_*`) rather than
    // converted with `ss` the way the funnel does it.
    tile_grid: &crate::entropy::obu::TileGrid,
    w: usize,
    h: usize,
    // The 10-bit CHROMA source, in the SB-extent shape `sb_chroma_owned` has
    // (aligned stride `cstride`, extra edge-replicated rows) so a straddling
    // block's residual gather stays in bounds.
    u_src10: &[u16],
    v_src10: &[u16],
    cstride: usize,
    // The frame's 10-bit LUMA recon from `bd10_reencode_luma` — the SB-EXTENT
    // canvas at stride `y_stride`, not the cropped `w*h`. It is the CfL AC
    // source for UV_CFL_PRED leaves, and `cfl_ac_from_frame_recon_hbd` reads
    // `max(bh, 8)` rows from the block origin, which straddles on a partial SB.
    y_recon10: &[u16],
    y_stride: usize,
    // Frame-level chroma qindex (== base_qindex) — sources ONLY the coeff-rate
    // context (`cfc`), which C builds once per frame from base_qindex (never
    // per plane). The per-plane quant TABLES use qindex_u/qindex_v below.
    chroma_qindex: u8,
    // [SVT_HDR_MODE] per-plane chroma quant qindex = base_qindex + the FH
    // u_ac/v_ac delta (chroma_q.rs / pipeline qindex_u/qindex_v). C dequantizes
    // chroma with the signaled per-plane deltas (separate_uv_delta_q=1), and the
    // bd8 walk already quantizes U/V at these qindices — the bd10 chroma
    // re-encode MUST too, or a small residual that survives at the finer plane
    // qindex is dropped at base (the diag q5 Cr off-by-one: V_PRED predicts the
    // no-neighbour default 511, source is flat 512, so +1/px; at qindex_v it
    // codes, at base it rounds to 0 -> the port codes 511 where C codes 512).
    // Using base for both also DESYNCS the port's own chroma recon from its
    // signaled bitstream (the decoder dequantizes at qindex_v). Mainline: both
    // == base_qindex (all FH chroma deltas 0) -> byte-inert.
    qindex_u: u8,
    qindex_v: u8,
    rdoq_level: u8,
    lambda: u64,
    // C `scs->allintra || scs->static_config.rtc` — the RDOQ plane rate
    // weight arm (`crate::quant::PLANE_RD_MULT`). FALSE on a video frame.
    allintra_rd_mult: bool,
    edge_filter: bool,
    bd: u8,
    // [SVT_HDR_MODE] per-plane QM levels [U, V] (15 = off). C derives them
    // separately via `aom_get_qmlevel(base_qindex + delta_q_ac[plane], ...)`
    // (md_config_process.c:271-279), so they can differ between Cb and Cr —
    // the fork's chroma path gives Cb a +12 delta.
    qm_uv: [u8; 2],
    // [SVT_HDR_MODE] fork loop_filter_sharpness (static_config.sharpness). 0 in
    // mainline → byte-identical to build_quant_table_bd. C applies the same
    // qzbin/qround sharpening to the chroma quantizer rows (u/v_zbin/round).
    sharpness: i8,
    // The DPB's reference pictures, whose 10-bit twin the INTER arm predicts
    // from. `None` on a key frame.
    inter_refs: Option<&[Option<&crate::picture::PaddedRef>; 8]>,
) -> crate::EncodeResult<(alloc::vec::Vec<u16>, alloc::vec::Vec<u16>)> {
    let fc = crate::entropy::context::FrameContext::new_default();
    let cfc = crate::entropy::coeff_c::CoeffFc::default_for_qindex(chroma_qindex);
    let rates = crate::leaf_funnel::build_md_rates(&fc, &cfc);
    // Per-plane chroma quant tables (== each other, and == the old single
    // base-qindex table, whenever the FH chroma deltas are 0 -> mainline inert).
    let qt_u = crate::quant::build_quant_table_bd_sharp(qindex_u, bd, sharpness);
    let qt_v = crate::quant::build_quant_table_bd_sharp(qindex_v, bd, sharpness);
    let (cframe_w, cframe_h) = (w / 2, h / 2);
    // SB-extent-sized, ALIGNED-strided — the chroma twin of the luma canvas
    // above (and of `fun_u_recon` / `fun_v_recon` in the funnel). The caller
    // crops the in-frame `cframe_w * cframe_h` region.
    let ext_cbuf = (w.div_ceil(sb_size) * sb_size / 2) * (h.div_ceil(sb_size) * sb_size / 2);
    // Seeded with the 10-bit DC default like the luma canvas above (and like
    // the funnel's `fun_u_recon` / `fun_v_recon`, which are 128u8) — see the
    // note there for why 0 is wrong once the buffer is SB-extent-sized.
    let seed: u16 = 128u16 << (bd - 8);
    let mut recon10_u = svtav1_types::try_vec![seed; ext_cbuf]?;
    let mut recon10_v = svtav1_types::try_vec![seed; ext_cbuf]?;
    // The chroma twin of the luma pass's grid: `get_filt_type(xd, 1)` reads the
    // neighbour UV modes, and this pass passed a hardcoded 0 for the same
    // reason and with the same consequence -- a directional UV leaf under the
    // sequence header's edge filter dropped the whole frame out of the
    // re-encode. Sized on LUMA coordinates because the walk carries those.
    let mut mode_neighbors = Bd10ModeNeighbors::new(cframe_w * 2, cframe_h * 2)?;
    // The committed mode-info grid `inter_chroma_4xn_pred` reads for a sub-8
    // leaf's covered cells — the MD funnel stamps `commit_leaf`'s; this pass
    // rebuilds it from the committed trees (`stamp_inter_mi_grid`). Built only
    // when the frame can carry inter leaves (the chroma post-pass's INTER arm
    // is what reads it).
    let mi_stride = w / 4;
    let mut mi_grid =
        svtav1_types::try_vec![crate::intrabc_mvp::MvpMiEntry::default(); mi_stride * (h / 4)]?;
    if inter_refs.is_some() {
        for (sb_idx, tree) in all_trees.iter().enumerate() {
            let sb_col = sb_idx % sb_cols;
            let sb_row = sb_idx / sb_cols;
            stamp_inter_mi_grid(
                tree,
                sb_col * sb_size,
                sb_row * sb_size,
                svtav1_types::partition::PartitionType::None as u8,
                &mut mi_grid,
                mi_stride,
                w,
                h,
            );
        }
    }
    for (sb_idx, tree) in all_trees.iter_mut().enumerate() {
        let sb_col = sb_idx % sb_cols;
        let sb_row = sb_idx / sb_cols;
        // The grid is LUMA-indexed, so it takes the LUMA tile bounds.
        let tile_mi_l =
            tile_grid.tile_mi_for_sb(sb_row, sb_col, sb_size, cframe_w * 2, cframe_h * 2);
        mode_neighbors.enter_sb(sb_col * sb_size, sb_row * sb_size, sb_size, tile_mi_l);
        bd10_reencode_chroma_node(
            sb_size / 4,
            tree,
            sb_col * sb_size,
            sb_row * sb_size,
            &mut recon10_u,
            &mut recon10_v,
            cstride,
            u_src10,
            v_src10,
            y_recon10,
            y_stride,
            &qt_u,
            &qt_v,
            rdoq_level,
            lambda,
            allintra_rd_mult,
            &rates,
            edge_filter,
            cframe_w,
            cframe_h,
            bd,
            qm_uv,
            tile_mi_l,
            svtav1_types::partition::PartitionType::None,
            chroma_qindex == 0,
            inter_refs,
            &mi_grid,
            mi_stride,
            &mut mode_neighbors,
        );
    }
    // The frame's true 10-bit CHROMA recon — the post-MD canvas the bd10
    // post-filter chain (deblock -> CDEF search -> LR search) reads, the
    // chroma twin of `bd10_reencode_luma`'s return. C keeps the same thing
    // in the 16-bit recon picture (`svt_aom_get_recon_pic(.., is_16bit)`).
    Ok((recon10_u, recon10_v))
}

/// Re-encode ONE chroma plane's leaf at bd10: predict -> residual/tx/quant ->
/// recon, writing the bd10 recon back into `recon10` for neighbour prediction.
/// Returns `(qcoeff raster, eob, u8-recon)`. `uv_tt`/geom/edge params mirror the
/// walk's chroma coding (`write_chroma_txb`, `uv_tx_type`). The u8 recon is a
/// sane truncation (`>> (bd-8)`) — it is inert (see `bd10_reencode_chroma`).
#[allow(clippy::too_many_arguments)]
fn bd10_reencode_chroma_plane(
    recon10: &mut [u16],
    src10: &[u16],
    cstride: usize,
    cx: usize,
    cy: usize,
    cw: usize,
    ch: usize,
    uv_mode: u8,
    uv_angle_delta: i8,
    uv_tt: usize,
    geom: &crate::leaf_funnel::UnitGeom,
    edge_filter: bool,
    // C `get_filt_type(xd, 1)`. Was a hardcoded 0 — only right with the edge
    // filter off, which is why the frame gate rejected directional UV leaves.
    filt_type: i32,
    qt: &crate::quant::QuantTable,
    rdoq_level: u8,
    lambda: u64,
    // C `scs->allintra || scs->static_config.rtc` — the RDOQ plane rate
    // weight arm (`crate::quant::PLANE_RD_MULT`). FALSE on a video frame.
    allintra_rd_mult: bool,
    rates: &crate::leaf_funnel::MdRates,
    bd: u8,
    qm_level: u8,
    // `Some((ac_luma_q3, alpha_q3))` for a UV_CFL_PRED leaf. C predicts CfL as
    // `svt_cfl_predict_hbd(pred_buf_q3, dc_pred, alpha)` over a **DC** base
    // (`cfl_prediction` regenerates DC at :3798-3801 before calling), so the
    // mode passed to `predict_unit_hbd` is forced to UV_DC_PRED here.
    cfl: Option<(&[i16], i32)>,
    // An INTER leaf's motion-compensated chroma prediction, already built by
    // the caller from the 10-bit reference. `Some` replaces the intra
    // prediction entirely — C codes no intra `uv_mode` on an inter block, so
    // running the intra predictor here would price and reconstruct a mode the
    // stream does not describe.
    inter_pred: Option<&[u16]>,
) -> (alloc::vec::Vec<i32>, u16, alloc::vec::Vec<u8>) {
    let mut pred = alloc::vec![0u16; cw * ch];
    if let Some(p) = inter_pred {
        // An INTER leaf: the caller already motion-compensated this plane from
        // the 10-bit reference. Everything below the prediction is shared.
        pred.copy_from_slice(&p[..cw * ch]);
    } else {
        crate::leaf_funnel::predict_unit_hbd(
            recon10,
            cstride,
            cx,
            cy,
            cw,
            ch,
            if cfl.is_some() { 0 } else { uv_mode },
            if cfl.is_some() { 0 } else { uv_angle_delta },
            crate::leaf_funnel::FI_NONE,
            geom,
            edge_filter,
            filt_type,
            &mut pred,
            bd,
        );
        if let Some((ac, alpha_q3)) = cfl {
            let dc = pred.clone();
            svtav1_dsp::hbd::cfl_predict_hbd(ac, &dc, cw, &mut pred, cw, alpha_q3, bd, cw, ch);
        }
    }
    let src_off = cy * cstride + cx;
    let out = crate::leaf_funnel::tx_unit_hbd(
        false, // This level-only post-pass currently accepts only lossy depth-0 trees.
        src10,
        cstride,
        src_off,
        &pred,
        cw,
        0,
        cw,
        ch,
        uv_tt,
        1, // chroma plane
        0, // txb_skip_ctx (eff-M9 rate_est_level 0)
        0, // dc_sign_ctx
        qt,
        rdoq_level,
        lambda,
        0, // sharpness
        allintra_rd_mult,
        rates,
        rdoq_level != 0,
        bd,
        qm_level,
        None, // level-only re-encode: no RD terms
    );
    // Straddle clip — see the luma twin in `bd10_reencode_node`. A no-op
    // wherever `cx + cw <= cstride`.
    let cwr = cw.min(cstride.saturating_sub(cx));
    for r in 0..ch {
        let drow = (cy + r) * cstride + cx;
        recon10[drow..drow + cwr].copy_from_slice(&out.recon[r * cw..r * cw + cwr]);
    }
    let shift = (bd - 8) as u32;
    let rec_u8: alloc::vec::Vec<u8> = out
        .recon
        .iter()
        .map(|&s| (s >> shift).min(255) as u8)
        .collect();
    (out.qcoeff, out.eob, rec_u8)
}

/// The chroma half of the `skip_mode` contract — see the luma twin in
/// `bd10_reencode_node`. The decoder reads no residual for the leaf, so the
/// plane's reconstruction is its motion-compensated prediction and the coded
/// state is all-zero; re-quantizing here would emit coefficients the decoder
/// never reads.
fn bd10_chroma_skip_plane(
    recon10: &mut [u16],
    cstride: usize,
    cx: usize,
    cy: usize,
    cw: usize,
    ch: usize,
    inter_pred: &[u16],
    bd: u8,
) -> (alloc::vec::Vec<i32>, u16, alloc::vec::Vec<u8>) {
    let cwr = cw.min(cstride.saturating_sub(cx));
    for r in 0..ch {
        let drow = (cy + r) * cstride + cx;
        if drow + cwr <= recon10.len() {
            recon10[drow..drow + cwr].copy_from_slice(&inter_pred[r * cw..r * cw + cwr]);
        }
    }
    let shift = (bd - 8) as u32;
    let rec_u8 = inter_pred[..cw * ch]
        .iter()
        .map(|&s| (s >> shift).min(255) as u8)
        .collect();
    (alloc::vec![0i32; cw * ch], 0, rec_u8)
}

#[allow(clippy::too_many_arguments)]
fn bd10_reencode_chroma_node(
    // C `seq_header.sb_mi_size` (16 SB64 / 32 SB128), task #91.
    sb_mi_size: usize,
    tree: &mut crate::partition::PartitionTree,
    x: usize,
    y: usize,
    recon10_u: &mut [u16],
    recon10_v: &mut [u16],
    cstride: usize,
    u_src10: &[u16],
    v_src10: &[u16],
    y_recon10: &[u16],
    y_stride: usize,
    // Per-plane chroma quant tables (base + FH u_ac / v_ac delta). Equal in
    // mainline (deltas 0) -> byte-inert.
    qt_u: &crate::quant::QuantTable,
    qt_v: &crate::quant::QuantTable,
    rdoq_level: u8,
    lambda: u64,
    // C `scs->allintra || scs->static_config.rtc` — the RDOQ plane rate
    // weight arm (`crate::quant::PLANE_RD_MULT`). FALSE on a video frame.
    allintra_rd_mult: bool,
    rates: &crate::leaf_funnel::MdRates,
    edge_filter: bool,
    cframe_w: usize,
    cframe_h: usize,
    bd: u8,
    qm_uv: [u8; 2],
    // ISSUE #18: this superblock's tile, in LUMA mi units — `UnitGeom::tile`
    // is a luma-mi bound regardless of plane; `TileMi::top_px(ss)` does the
    // plane shift.
    tile_mi: crate::intra_edge::TileMi,
    // The leaf's parent partition type, selecting the has_top_right /
    // has_bottom_left table row — the decoder reads the LUMA partition the
    // chroma block was coded under (`xd->mi[0]->partition`), so this mirrors
    // `bd10_reencode_node`'s threading exactly.
    parent_partition: svtav1_types::partition::PartitionType,
    // Frame lossless (chroma_qindex == 0): `av1_get_tx_type`'s early return
    // forces DCT_DCT for the inter chroma-follows-luma read too.
    lossless: bool,
    // The DPB's reference pictures, for the INTER arm. `None` on a key frame.
    inter_refs: Option<&[Option<&crate::picture::PaddedRef>; 8]>,
    // The committed mode-info grid (`stamp_inter_mi_grid`) a sub-8 inter
    // leaf's chroma stitch reads its covered cells' motion from.
    mi_grid: &[crate::intrabc_mvp::MvpMiEntry],
    mi_stride: usize,
    // The neighbour MODE grid `get_filt_type(xd, 1)` reads.
    mode_neighbors: &mut Bd10ModeNeighbors,
) {
    use crate::partition::PartitionTree as Tr;
    use crate::partition::PartitionType as PT;
    match tree {
        Tr::Leaf(d) => {
            let bw = d.width as usize;
            let bh = d.height as usize;
            // Chroma reference? (walk `blk_has_uv`, pipeline.rs). With the
            // min-8x8 luma policy every leaf is a reference; kept for safety.
            let bw_mi = bw / 4;
            let bh_mi = bh / 4;
            let has_uv = ((y / 4) % 2 == 1 || bh_mi.is_multiple_of(2))
                && ((x / 4) % 2 == 1 || bw_mi.is_multiple_of(2));
            if !has_uv {
                return;
            }
            // Chroma origin/dims — EXACTLY the walk's derivation.
            let cw = bw.max(8) / 2;
            let ch = bh.max(8) / 2;
            let cx = ((x >> 3) << 3) / 2 + if bw >= 8 { (x % 8) / 2 } else { 0 };
            let cy = ((y >> 3) << 3) / 2 + if bh >= 8 { (y % 8) / 2 } else { 0 };
            // UV_CFL_PRED: C's chroma tx_type is forced to DCT_DCT
            // (`cfl_prediction` :3796, `transform_type_uv = DCT_DCT`), and the
            // prediction comes from the 10-bit LUMA recon rather than the
            // chroma neighbours. `uv_tx_type` already maps mode 13 -> DCT_DCT,
            // so only the prediction changes.
            // Inter-classified leaves take the same arm the syntax writer
            // uses (`encode_block_syntax`, pipeline.rs): the decoder derives
            // the chroma tx_type from `tx_type_map` at the luma block origin
            // (`av1_get_tx_type` UV arm, blockd.h:1296-1309) — the coded luma
            // type, gated by the inter ext-tx set for the chroma tx size.
            // Using the intra `uv_tx_type` arm here applied a coded V_DCT DC
            // as a flat DCT_DCT DC, which is exactly the column-structured
            // recon divergence seen against dav1d on inter chroma.
            let uv_tt = if d.is_inter || d.use_intrabc {
                // The decoder derives the chroma type from tx_type_map,
                // which holds the covering luma txb's CODED type: DCT_DCT
                // when that txb coded no coefficients (read_coeffs_txb's
                // all-zero arm, decodetxb.c:148-154) or on lossless
                // (blockd.h:1288). The decision's tx_type can hold a
                // non-DCT search winner on an all-zero luma txb —
                // `inter_uv_tx_type` applies the map semantics.
                let (cover_eob, cover_tt) = if d.tx_depth == 0 {
                    (d.eob, d.tx_type)
                } else {
                    (
                        d.txb_eobs.first().copied().unwrap_or(0),
                        d.txb_tx_types.first().copied().unwrap_or(0),
                    )
                };
                crate::leaf_funnel::inter_uv_tx_type(cover_eob, cover_tt, lossless, cw, ch)
            } else {
                crate::leaf_funnel::uv_tx_type(d.uv_mode, cw, ch)
            };
            let cfl_ac: Option<alloc::vec::Vec<i16>> = if d.uv_mode == 13 {
                let mut ac = alloc::vec![0i16; svtav1_dsp::intra_pred::CFL_BUF_LINE * ch.max(1)];
                crate::leaf_funnel::cfl_ac_from_frame_recon_hbd(
                    y_recon10, y_stride, x, y, bw, bh, cw, ch, &mut ac,
                );
                Some(ac)
            } else {
                None
            };
            let cfl_u = cfl_ac.as_ref().map(|ac| {
                (
                    &ac[..],
                    crate::leaf_funnel::cfl_idx_to_alpha(d.cfl_alpha_idx, d.cfl_alpha_signs, 0),
                )
            });
            let cfl_v = cfl_ac.as_ref().map(|ac| {
                (
                    &ac[..],
                    crate::leaf_funnel::cfl_idx_to_alpha(d.cfl_alpha_idx, d.cfl_alpha_signs, 1),
                )
            });
            // `UnitGeom` is a LUMA-domain contract: luma mi position, luma
            // block dims, `ss` the plane subsampling, LUMA frame px and a
            // luma-mi tile (predict.rs:14-35), mirroring the MD funnel's
            // `uv_geom` (leaf_funnel/mod.rs) — ROUND_UV-pair-aligned mi and
            // `w.max(8)` pair dims so a sub-8 leaf's chroma unit is the pair.
            // The previous chroma-domain `ss: 0` shim left xr/yd/right_
            // available coincidentally right (they scale linearly) but fed
            // `has_top_right`/`has_bottom_left` the chroma mi coords and a
            // hardcoded `None` partition where the decoder indexes by LUMA
            // mi, the scaled chroma bsize, and the coded partition — so
            // `have_bottom_left`/`have_top_right` were wrong and directional
            // chroma pred fabricated the extended edge the decoder read as
            // real recon. MEASURED: vidyo1 p10 q20, f0 chroma ramp drift
            // growing toward block bottom-left.
            let geom = crate::leaf_funnel::UnitGeom {
                partition: parent_partition,
                mi_row: ((y >> 3) << 3) >> 2,
                mi_col: ((x >> 3) << 3) >> 2,
                bw_px: bw.max(8),
                bh_px: bh.max(8),
                sb_mi_size,
                ss: 1,
                frame_w: cframe_w * 2,
                frame_h: cframe_h * 2,
                tile: tile_mi,
            };
            // THE INTER ARM, chroma half. Built once for both planes because
            // C's chroma prediction is one call with a halved origin over the
            // LUMA block's subpel result — predicting each plane separately
            // would be different arithmetic (see `predict_inter_yuv_hbd`).
            let (inter_u, inter_v) = match d.inter.as_deref() {
                Some(ic) => {
                    let (bw, bh) = (d.width as usize, d.height as usize);
                    // A scratch luma destination: the driver predicts luma and
                    // chroma together, and only the chroma halves are read here
                    // (the luma pass already wrote its own).
                    let mut y_scratch = alloc::vec![0u16; bw * bh];
                    let mut u = alloc::vec![0u16; cw * ch];
                    let mut v = alloc::vec![0u16; cw * ch];
                    // A sub-8 luma block's chroma covers the parent 8x8 and C
                    // stitches it from the covered cells' own MVs
                    // (`inter_chroma_4xn_pred`), exactly like the MD funnel's
                    // `predict_inter_chroma_sub8` — the luma-shaped leaf arm
                    // would predict `bw/2 x bh/2` at this block's own origin,
                    // which is neither the right area nor the right motion.
                    let sub8 = bw < 8 || bh < 8;
                    predict_inter_leaf_hbd_any(
                        inter_refs.expect(
                            "an inter leaf reached the bd10 chroma re-encode on a frame with no DPB",
                        ),
                        ic,
                        x,
                        y,
                        bw,
                        bh,
                        sb_mi_size * 4,
                        cframe_w * 2,
                        cframe_h * 2,
                        bd,
                        !sub8,
                        &mut y_scratch,
                        bw,
                        &mut u,
                        &mut v,
                        cw,
                    );
                    if sub8 {
                        // `inter_chroma_4xn_pred` is uni-prediction only —
                        // C asserts `!is_compound` on the sub-8 arm and the
                        // funnel's `allow_bipred` rejects 4-wide/4-tall
                        // blocks, so a compound sub-8 leaf cannot be
                        // committed.
                        debug_assert!(
                            ic.ref_frame[1] <= 0,
                            "a compound sub-8 inter leaf reached the bd10 chroma re-encode"
                        );
                        crate::inter_md_arm::predict_inter_chroma_sub8_hbd(
                            inter_refs.expect(
                                "an inter leaf reached the bd10 chroma re-encode on a frame \
                                 with no DPB",
                            ),
                            mi_grid,
                            mi_stride as i32,
                            x,
                            y,
                            bw,
                            bh,
                            ic.ref_frame[0],
                            ic.mv[0],
                            ic.interp_filters,
                            sb_mi_size * 4,
                            cframe_w * 2,
                            cframe_h * 2,
                            bd,
                            &mut u,
                            &mut v,
                            cw,
                        );
                    } else {
                        // OBMC's chroma blend, matching the luma walk's.
                        // `motion_mode` is only signaled for >=8x8 blocks, so
                        // the sub-8 arm above can never be an OBMC leaf.
                        apply_obmc_hbd_postpass(
                            inter_refs.expect(
                                "an inter leaf reached the bd10 chroma re-encode on a frame \
                                 with no DPB",
                            ),
                            mi_grid,
                            mi_stride,
                            ic,
                            x,
                            y,
                            bw,
                            bh,
                            sb_mi_size * 4,
                            cframe_w * 2,
                            cframe_h * 2,
                            bd,
                            &mut [],
                            0,
                            &mut u,
                            &mut v,
                            cw,
                        );
                    }
                    (Some(u), Some(v))
                }
                None => (None, None),
            };
            let forced_zero = d.inter.as_deref().is_some_and(|ic| ic.skip_mode);
            let (u_q, u_eob, u_rec) = if forced_zero {
                bd10_chroma_skip_plane(
                    recon10_u,
                    cstride,
                    cx,
                    cy,
                    cw,
                    ch,
                    inter_u.as_deref().expect("a skip_mode leaf is inter"),
                    bd,
                )
            } else {
                bd10_reencode_chroma_plane(
                    recon10_u,
                    u_src10,
                    cstride,
                    cx,
                    cy,
                    cw,
                    ch,
                    d.uv_mode,
                    d.uv_angle_delta,
                    uv_tt,
                    &geom,
                    edge_filter,
                    mode_neighbors.filt_type_uv(x, y),
                    qt_u,
                    rdoq_level,
                    lambda,
                    allintra_rd_mult,
                    rates,
                    bd,
                    qm_uv[0],
                    cfl_u,
                    inter_u.as_deref(),
                )
            };
            let (v_q, v_eob, v_rec) = if forced_zero {
                bd10_chroma_skip_plane(
                    recon10_v,
                    cstride,
                    cx,
                    cy,
                    cw,
                    ch,
                    inter_v.as_deref().expect("a skip_mode leaf is inter"),
                    bd,
                )
            } else {
                bd10_reencode_chroma_plane(
                    recon10_v,
                    v_src10,
                    cstride,
                    cx,
                    cy,
                    cw,
                    ch,
                    d.uv_mode,
                    d.uv_angle_delta,
                    uv_tt,
                    &geom,
                    edge_filter,
                    mode_neighbors.filt_type_uv(x, y),
                    qt_v,
                    rdoq_level,
                    lambda,
                    allintra_rd_mult,
                    rates,
                    bd,
                    qm_uv[1],
                    cfl_v,
                    inter_v.as_deref(),
                )
            };
            d.chroma_dec = Some((u_q, v_q, u_eob, v_eob, u_rec, v_rec));
            mode_neighbors.record(
                x,
                y,
                bw,
                bh,
                if d.inter.is_some() { 0 } else { d.intra_mode },
                if d.inter.is_some() { 0 } else { d.uv_mode },
            );
        }
        Tr::Split {
            partition_type,
            width,
            height,
            children,
        } => {
            let nw = *width as usize;
            let nh = *height as usize;
            let hw = nw / 2;
            let hh = nh / 2;
            let qw = nw / 4;
            let qh = nh / 4;
            // Identical child-origin derivation to the luma twin — see the long
            // note in `bd10_reencode_node`. `x`/`y` here are LUMA coordinates
            // (the chroma origin is derived per leaf), so the in-frame test uses
            // the LUMA frame extent, which is `cframe_* * 2`.
            let (lframe_w, lframe_h) = (cframe_w * 2, cframe_h * 2);
            let mut recurse = |child: &mut crate::partition::PartitionTree, cx, cy| {
                bd10_reencode_chroma_node(
                    sb_mi_size,
                    child,
                    cx,
                    cy,
                    recon10_u,
                    recon10_v,
                    cstride,
                    u_src10,
                    v_src10,
                    y_recon10,
                    y_stride,
                    qt_u,
                    qt_v,
                    rdoq_level,
                    lambda,
                    allintra_rd_mult,
                    rates,
                    edge_filter,
                    cframe_w,
                    cframe_h,
                    bd,
                    qm_uv,
                    // Children share the superblock, hence the tile (issue #18).
                    tile_mi,
                    match partition_type {
                        PT::VertA => svtav1_types::partition::PartitionType::VertA,
                        PT::VertB => svtav1_types::partition::PartitionType::VertB,
                        _ => svtav1_types::partition::PartitionType::None,
                    },
                    lossless,
                    inter_refs,
                    mi_grid,
                    mi_stride,
                    mode_neighbors,
                );
            };
            match *partition_type {
                PT::Split => {
                    let mut ci = 0usize;
                    for i in 0..4usize {
                        let cx = x + (i & 1) * hw;
                        let cy = y + (i >> 1) * hh;
                        if cx >= lframe_w || cy >= lframe_h {
                            continue;
                        }
                        recurse(&mut children[ci], cx, cy);
                        ci += 1;
                    }
                    debug_assert_eq!(
                        ci,
                        children.len(),
                        "bd10 chroma reencode: in-frame quadrant count must equal the packed \
                         child count"
                    );
                }
                PT::Horz => {
                    let (first, rest) = children.split_at_mut(1);
                    recurse(&mut first[0], x, y);
                    if let Some(bot) = rest.first_mut() {
                        recurse(bot, x, y + hh);
                    }
                }
                PT::Vert => {
                    let (first, rest) = children.split_at_mut(1);
                    recurse(&mut first[0], x, y);
                    if let Some(right) = rest.first_mut() {
                        recurse(right, x + hw, y);
                    }
                }
                ext => {
                    let offs: &[(usize, usize)] = match ext {
                        PT::HorzA => &[(0, 0), (hw, 0), (0, hh)],
                        PT::HorzB => &[(0, 0), (0, hh), (hw, hh)],
                        PT::VertA => &[(0, 0), (0, hh), (hw, 0)],
                        PT::VertB => &[(0, 0), (hw, 0), (hw, hh)],
                        PT::Horz4 => &[(0, 0), (0, qh), (0, 2 * qh), (0, 3 * qh)],
                        PT::Vert4 => &[(0, 0), (qw, 0), (2 * qw, 0), (3 * qw, 0)],
                        other => panic!("bd10 chroma reencode: unsupported partition {other:?}"),
                    };
                    for (child, &(dx, dy)) in children.iter_mut().zip(offs) {
                        recurse(child, x + dx, y + dy);
                    }
                }
            }
        }
    }
}

/// The `tx_depth > 0` arm of the luma re-encode — C `perform_tx_partitioning`
/// (product_coding_loop.c:5282-5420) at a committed depth, levels only.
///
/// It used to be an `assert_eq!(d.tx_depth, 0)`, and that assert is what kept a
/// 10-bit VIDEO frame out of the post-pass: the transform-size search is on at
/// preset 6, so a single depth-1 leaf in one superblock dropped the whole frame.
///
/// Two things it does that the depth-0 arm does not:
///
/// * **INTRA feeds back.** Each TXB predicts from the recon of the TXBs before
///   it inside this block (`predict_unit_overlay_hbd` over a running
///   `dep_recon`), which is why the loop is sequential and why the block's
///   recon is written out only at the end.
/// * **INTER does not.** C predicts the whole block once and every TXB
///   residuals against the same buffer; there is no per-TXB re-prediction.
///
/// The per-TXB tx TYPES are the committed ones (`d.txb_tx_types`) — this pass
/// re-quantizes, it does not re-decide.
#[allow(clippy::too_many_arguments)]
fn bd10_reencode_leaf_txs(
    d: &mut crate::partition::BlockDecision,
    x: usize,
    y: usize,
    recon10: &mut [u16],
    stride: usize,
    src10: &[u16],
    src_stride: usize,
    qt: &crate::quant::QuantTable,
    rdoq_level: u8,
    lambda: u64,
    allintra_rd_mult: bool,
    rates: &crate::leaf_funnel::MdRates,
    real_coeff_ctx: bool,
    coeff_neighbors: &mut Bd10CoeffNeighbors,
    edge_filter: bool,
    frame_w: usize,
    frame_h: usize,
    bd: u8,
    qm_level: u8,
    tile_mi: crate::intra_edge::TileMi,
    parent_partition: svtav1_types::partition::PartitionType,
    sb_mi_size: usize,
    inter_refs: Option<&[Option<&crate::picture::PaddedRef>; 8]>,
    // The committed mode-info grid the OBMC blend's neighbour walk reads;
    // `mi_stride` is `frame_w / 4`.
    mi_grid: &[crate::intrabc_mvp::MvpMiEntry],
    mi_stride: usize,
    mode_neighbors: &Bd10ModeNeighbors,
) {
    let bw = d.width as usize;
    let bh = d.height as usize;
    let (txw, txh) = crate::leaf_funnel::txb_dims_at_depth(bw, bh, d.tx_depth);
    let cols = bw / txw;
    let txbs = cols * (bh / txh);
    let geom = crate::leaf_funnel::UnitGeom {
        partition: parent_partition,
        mi_row: y >> 2,
        mi_col: x >> 2,
        bw_px: bw,
        bh_px: bh,
        sb_mi_size,
        ss: 0,
        frame_w,
        frame_h,
        tile: tile_mi,
    };
    let filt_type = mode_neighbors.filt_type_y(x, y);
    // The INTER whole-block prediction, built once (C does the same).
    let inter_pred: Option<alloc::vec::Vec<u16>> = d.inter.as_deref().map(|ic| {
        let mut p = alloc::vec![0u16; bw * bh];
        let refs = inter_refs
            .expect("an inter leaf reached the bd10 TXS re-encode on a frame with no DPB");
        predict_inter_leaf_hbd_any(
            refs,
            ic,
            x,
            y,
            bw,
            bh,
            sb_mi_size * 4,
            frame_w,
            frame_h,
            bd,
            false,
            &mut p,
            bw,
            &mut [],
            &mut [],
            0,
        );
        // OBMC blend on the whole-block prediction, in place — the same
        // neighbour state the depth-0 arm reads (the reconstructed mi grid).
        apply_obmc_hbd_postpass(
            refs,
            mi_grid,
            mi_stride,
            ic,
            x,
            y,
            bw,
            bh,
            sb_mi_size * 4,
            frame_w,
            frame_h,
            bd,
            &mut p,
            bw,
            &mut [],
            &mut [],
            0,
        );
        p
    });
    let mut dep_recon = alloc::vec![0u16; bw * bh];
    d.eob = 0;
    d.qcoeffs.clear();
    d.txb_qcoeffs.clear();
    d.txb_eobs.clear();
    let committed_types = core::mem::take(&mut d.txb_tx_types);
    d.txb_tx_types = committed_types.clone();
    // The `skip_mode` contract of the depth-0 arm, one level down: the decoder
    // reads no txbs for the leaf, so no per-txb levels may be re-quantized
    // into existence. Reconstruct the block as its (inter) prediction and
    // leave the per-txb vectors empty — the writer reads them only for a
    // non-skip block.
    let forced_zero = d.inter.as_deref().is_some_and(|ic| ic.skip_mode);
    if forced_zero {
        dep_recon
            .copy_from_slice(&inter_pred.as_deref().expect("a skip_mode leaf is inter")[..bw * bh]);
        coeff_neighbors.record(x, y, bw, bh, 0);
    } else {
        for txb in 0..txbs {
            let (tx_x, tx_y) = ((txb % cols) * txw, (txb / cols) * txh);
            let mut pred = alloc::vec![0u16; txw * txh];
            match inter_pred.as_ref() {
                Some(p) => {
                    for r in 0..txh {
                        let src0 = (tx_y + r) * bw + tx_x;
                        pred[r * txw..(r + 1) * txw].copy_from_slice(&p[src0..src0 + txw]);
                    }
                }
                None => crate::leaf_funnel::predict_unit_overlay_hbd(
                    recon10,
                    stride,
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
                    edge_filter,
                    filt_type,
                    &mut pred,
                    bd,
                ),
            }
            let (tsc, dsc) = if real_coeff_ctx {
                coeff_neighbors.contexts(x + tx_x, y + tx_y, txw, txh)
            } else {
                (0, 0)
            };
            let tt = committed_types.get(txb).copied().unwrap_or(0) as usize;
            let out = crate::leaf_funnel::tx_unit_hbd(
                false,
                src10,
                src_stride,
                (y + tx_y) * src_stride + x + tx_x,
                &pred,
                txw,
                0,
                txw,
                txh,
                tt,
                0,
                tsc,
                dsc,
                qt,
                rdoq_level,
                lambda,
                0,
                allintra_rd_mult,
                rates,
                rdoq_level != 0,
                bd,
                qm_level,
                None,
            );
            coeff_neighbors.record(x + tx_x, y + tx_y, txw, txh, out.cul);
            d.eob += out.eob;
            d.txb_eobs.push(out.eob);
            d.txb_qcoeffs.push(out.qcoeff);
            for r in 0..txh {
                let dst = (tx_y + r) * bw + tx_x;
                dep_recon[dst..dst + txw].copy_from_slice(&out.recon[r * txw..r * txw + txw]);
            }
        }
    }
    // Straddle clip, exactly as the depth-0 arm does.
    let bwr = bw.min(stride.saturating_sub(x));
    for r in 0..bh {
        let drow = (y + r) * stride + x;
        if drow + bwr <= recon10.len() {
            recon10[drow..drow + bwr].copy_from_slice(&dep_recon[r * bw..r * bw + bwr]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::partition::{BlockDecision, InterDecision, PartitionTree};
    use crate::picture::{PaddedPlane, PaddedPlaneHbd, PaddedRef, PaddedRefHbd};
    use svtav1_types::motion::{Mv, WarpedMotionParams};
    use svtav1_types::prediction::PredictionMode;

    /// A 32x32 leaf on a 32x32 frame, driven through the depth-0 arm. The
    /// reference is flat so the prediction is flat; the source carries a hard
    /// gradient so that any residual the re-quantize computes is nonzero.
    /// `skip_mode` flips the committed-syntax contract between the two arms
    /// exercised below.
    fn run_leaf(skip_mode: bool) -> (BlockDecision, alloc::vec::Vec<u16>) {
        const W: usize = 32;
        const H: usize = 32;
        let bd = 10u8;
        let ref_y = alloc::vec![512u16; W * H];
        let ref_c = alloc::vec![512u16; W * H / 4];
        let pref = PaddedRef {
            y: PaddedPlane::from_plane(&alloc::vec![128u8; W * H], W, H, 32),
            uv: Some((
                PaddedPlane::from_plane(&alloc::vec![128u8; W * H / 4], W / 2, H / 2, 16),
                PaddedPlane::from_plane(&alloc::vec![128u8; W * H / 4], W / 2, H / 2, 16),
            )),
            hbd: Some(PaddedRefHbd {
                y: PaddedPlaneHbd::from_plane(&ref_y, W, H, 32),
                uv: Some((
                    PaddedPlaneHbd::from_plane(&ref_c, W / 2, H / 2, 16),
                    PaddedPlaneHbd::from_plane(&ref_c, W / 2, H / 2, 16),
                )),
            }),
        };
        let refs: [Option<&PaddedRef>; 8] = [Some(&pref); 8];
        // Strong slope: src - pred reaches ~+500 at the far edge, which no
        // quantizer can drop to zero.
        let src10: alloc::vec::Vec<u16> = (0..W * H)
            .map(|i| (512 + (i % W) * 24).min(1023) as u16)
            .collect();
        let inter = InterDecision {
            mode: PredictionMode::NearestNearestMv,
            ref_frame: [1, 7],
            mv: [Mv { x: 0, y: 0 }, Mv { x: 0, y: 0 }],
            drl_index: 0,
            interp_filters: 0,
            motion_mode: crate::port_entropy_inter::modes::MotionMode::SimpleTranslation,
            num_proj_ref: 0,
            overlappable_neighbors: 0,
            skip_mode,
            comp_group_idx: 0,
            compound_idx: 1,
            interinter_comp_type: 0,
            wm_params: WarpedMotionParams::default(),
            wm_params_l1: WarpedMotionParams::default(),
        };
        let mut tree = PartitionTree::Leaf(BlockDecision {
            is_inter: true,
            inter: Some(alloc::boxed::Box::new(inter)),
            width: W as u16,
            height: H as u16,
            qcoeffs: alloc::vec![0i32; W * H],
            ..Default::default()
        });
        let fc = crate::entropy::context::FrameContext::new_default();
        let cfc = crate::entropy::coeff_c::CoeffFc::default_for_qindex(40);
        let rates = crate::leaf_funnel::build_md_rates(&fc, &cfc);
        let qt = crate::quant::build_quant_table_bd_sharp(40, bd, 0);
        let mut recon10 = alloc::vec![128u16 << 2; W * H];
        let mut cn = Bd10CoeffNeighbors::new(W, H).unwrap();
        let mut mn = Bd10ModeNeighbors::new(W, H).unwrap();
        // The leaf is not OBMC, so the grid's contents are never read; it
        // only has to be shaped.
        let mi_grid = alloc::vec![crate::intrabc_mvp::MvpMiEntry::default(); (W / 4) * (H / 4)];
        bd10_reencode_node(
            false,
            16,
            &mut tree,
            0,
            0,
            &mut recon10,
            W,
            &src10,
            W,
            &qt,
            0,
            1 << 10,
            false,
            &rates,
            true,
            &mut cn,
            false,
            W,
            H,
            bd,
            0,
            crate::intra_edge::TileMi::whole_frame(W, H),
            svtav1_types::partition::PartitionType::None,
            Some(&refs),
            &mi_grid,
            W / 4,
            &mut mn,
        );
        let PartitionTree::Leaf(d) = tree else {
            panic!("a leaf in, a leaf out")
        };
        (d, recon10)
    }

    /// The witness for the measured tile desync: a `skip_mode` leaf's syntax
    /// is committed to a zero residual, so the 10-bit re-encode must NOT
    /// resurrect levels the decoder will never read. Before the forced-zero
    /// arm this leaf re-quantized the slope into nonzero `eob` while the
    /// writer still suppressed the coefficient sections — aomdec reported
    /// "Failed to decode tile data" (johnny 256x256 q40 p6 f3, 2026-09-16).
    #[test]
    fn skip_mode_leaf_keeps_its_committed_zero_residual() {
        // The control arm first: WITHOUT skip_mode the same leaf must
        // re-quantize the slope into real levels, or the witness below could
        // never observe a regression.
        let (d, _) = run_leaf(false);
        assert!(
            d.eob > 0,
            "control leaf must produce a residual — a witness that cannot \
             fail is not a witness"
        );
        let (d, recon10) = run_leaf(true);
        assert_eq!(d.eob, 0, "skip_mode leaf must stay residual-free");
        assert!(
            d.qcoeffs.iter().all(|&c| c == 0),
            "skip_mode leaf must commit all-zero levels"
        );
        // And the reconstruction is the prediction itself — flat 512 from the
        // flat reference — exactly what a decoder reconstructs for the block.
        assert!(
            recon10.iter().all(|&s| s == 512),
            "skip_mode leaf reconstructs as its prediction"
        );
    }

    /// The sub-8 chroma stitch (`predict_inter_chroma_sub8_hbd`) reads the
    /// SIBLING's motion out of the mi grid the post-pass rebuilds from the
    /// committed tree — `inter_chroma_4xn_pred`'s covered-cell walk. Two 16x4
    /// inter leaves stacked under a Horz split are the minimal shape: the
    /// second (odd-mi) leaf's chroma covers the pair and must see the first
    /// leaf's reference/MV/filter, not a default intra cell.
    #[test]
    fn stamp_inter_mi_grid_carries_covered_sibling_motion() {
        let leaf = |mv: Mv, rf: i8, filt: u32| {
            PartitionTree::Leaf(BlockDecision {
                is_inter: true,
                inter: Some(alloc::boxed::Box::new(InterDecision {
                    mode: PredictionMode::NearestMv,
                    ref_frame: [rf, -1],
                    mv: [mv, Mv::default()],
                    drl_index: 0,
                    interp_filters: filt,
                    motion_mode: crate::port_entropy_inter::modes::MotionMode::SimpleTranslation,
                    num_proj_ref: 0,
                    overlappable_neighbors: 0,
                    skip_mode: false,
                    comp_group_idx: 0,
                    compound_idx: 1,
                    interinter_comp_type: 0,
                    wm_params: WarpedMotionParams::default(),
                    wm_params_l1: WarpedMotionParams::default(),
                })),
                width: 16,
                height: 4,
                ..Default::default()
            })
        };
        // A 16x8 node split Horz into two 16x4 leaves at luma y=0 and y=4 —
        // mi rows 0 and 1, i.e. one chroma pair at chroma (0,0) 8x4.
        let tree = PartitionTree::Split {
            partition_type: crate::partition::PartitionType::Horz,
            width: 16,
            height: 8,
            children: alloc::vec![
                leaf(Mv { x: 8, y: 16 }, 1, 0x10001),
                leaf(Mv { x: -8, y: 0 }, 2, 0x20002),
            ],
        };
        let stride = 4usize; // 16px / 4 = 4 mi cols, 2 mi rows
        let mut grid = alloc::vec![crate::intrabc_mvp::MvpMiEntry::default(); stride * 2];
        stamp_inter_mi_grid(
            &tree,
            0,
            0,
            svtav1_types::partition::PartitionType::Horz as u8,
            &mut grid,
            stride,
            16,
            8,
        );
        // The covered cell the stitcher reads for the second leaf is
        // (mi_row 1 + dr=-1, mi_col 0) -> row 0: the FIRST leaf's motion.
        let e = &grid[0];
        assert!(
            e.use_intrabc || e.ref_frame[0] > 0,
            "sibling must read inter"
        );
        assert_eq!(e.ref_frame[0], 1);
        assert_eq!((e.mv[0].x, e.mv[0].y), (8, 16));
        assert_eq!(e.interp_filters, 0x10001);
        // Row 1 carries the second leaf's own params.
        let e = &grid[stride];
        assert_eq!(e.ref_frame[0], 2);
        assert_eq!((e.mv[0].x, e.mv[0].y), (-8, 0));
        assert_eq!(e.interp_filters, 0x20002);
    }

    /// The post-pass OBMC arm (`apply_obmc_hbd_postpass`): an `ObmcCausal`
    /// leaf's neighbour spans come out of the committed-tree mi grid, the
    /// neighbours' 10-bit predictions are rebuilt from the DPB `hbd` twins,
    /// and the blend rewrites the block's edge band in place — exactly the
    /// funnel's `predict_obmc_in_place_hbd` driven by reconstructed state.
    /// The witness is a `skip_mode` leaf (committed zero residual ⇒ recon IS
    /// the prediction): with an overlappable inter neighbour above carrying
    /// different motion on a non-flat reference, the blended top band must
    /// differ from the pure base translation while the interior is untouched.
    #[test]
    fn obmc_leaf_blends_edges_from_the_committed_grid() {
        const W: usize = 16;
        const H: usize = 16;
        let bd = 10u8;
        // A ramp so the neighbour's displaced prediction differs from the
        // block's own — the blend is only observable when the two differ.
        let ref_y: alloc::vec::Vec<u16> = (0..W * H)
            .map(|i| (((i % W) * 29 + (i / W) * 53) & 1023) as u16)
            .collect();
        let ref_c = alloc::vec![512u16; W * H / 4];
        let pref = PaddedRef {
            y: PaddedPlane::from_plane(&alloc::vec![128u8; W * H], W, H, 32),
            uv: Some((
                PaddedPlane::from_plane(&alloc::vec![128u8; W * H / 4], W / 2, H / 2, 16),
                PaddedPlane::from_plane(&alloc::vec![128u8; W * H / 4], W / 2, H / 2, 16),
            )),
            hbd: Some(PaddedRefHbd {
                y: PaddedPlaneHbd::from_plane(&ref_y, W, H, 32),
                uv: Some((
                    PaddedPlaneHbd::from_plane(&ref_c, W / 2, H / 2, 16),
                    PaddedPlaneHbd::from_plane(&ref_c, W / 2, H / 2, 16),
                )),
            }),
        };
        let refs: [Option<&PaddedRef>; 8] = [Some(&pref); 8];
        // The committed tree: a 16x8 inter leaf on top carrying DIFFERENT
        // motion (mv.y = 16/8 = 2 px), the 16x8 OBMC leaf below it.
        let leaf = |motion_mode, mv: Mv| {
            PartitionTree::Leaf(BlockDecision {
                is_inter: true,
                inter: Some(alloc::boxed::Box::new(InterDecision {
                    mode: PredictionMode::NearestMv,
                    ref_frame: [1, -1],
                    mv: [mv, Mv::default()],
                    drl_index: 0,
                    interp_filters: 0,
                    motion_mode,
                    num_proj_ref: 0,
                    overlappable_neighbors: 0,
                    skip_mode: true,
                    comp_group_idx: 0,
                    compound_idx: 1,
                    interinter_comp_type: 0,
                    wm_params: WarpedMotionParams::default(),
                    wm_params_l1: WarpedMotionParams::default(),
                })),
                width: 16,
                height: 8,
                ..Default::default()
            })
        };
        let tree = PartitionTree::Split {
            partition_type: crate::partition::PartitionType::Horz,
            width: W as u16,
            height: H as u16,
            children: alloc::vec![
                leaf(
                    crate::port_entropy_inter::modes::MotionMode::SimpleTranslation,
                    Mv { x: 0, y: 16 },
                ),
                leaf(
                    crate::port_entropy_inter::modes::MotionMode::ObmcCausal,
                    Mv { x: 0, y: 0 },
                ),
            ],
        };
        let mi_stride = W / 4;
        let mut mi_grid =
            alloc::vec![crate::intrabc_mvp::MvpMiEntry::default(); mi_stride * (H / 4)];
        stamp_inter_mi_grid(
            &tree,
            0,
            0,
            svtav1_types::partition::PartitionType::Horz as u8,
            &mut mi_grid,
            mi_stride,
            W,
            H,
        );
        // The skip_mode contract makes the walk's recon equal to the OBMC
        // prediction: the OBMC leaf's base translation first, then the blend.
        let ic = match &tree {
            PartitionTree::Split { children, .. } => match &children[1] {
                PartitionTree::Leaf(d) => d.inter.as_deref().unwrap(),
                _ => panic!("a leaf in"),
            },
            _ => panic!("a split in"),
        };
        let mut base = alloc::vec![0u16; 16 * 8];
        predict_inter_leaf_hbd_any(
            &refs,
            ic,
            0,
            8,
            16,
            8,
            64,
            W,
            H,
            bd,
            false,
            &mut base,
            16,
            &mut [],
            &mut [],
            0,
        );
        let mut blended = base.clone();
        apply_obmc_hbd_postpass(
            &refs,
            &mi_grid,
            mi_stride,
            ic,
            0,
            8,
            16,
            8,
            64,
            W,
            H,
            bd,
            &mut blended,
            16,
            &mut [],
            &mut [],
            0,
        );
        // The above-blend band is `min(bh,64)/2 = 4` rows deep.
        assert_ne!(
            blended[..4 * 16],
            base[..4 * 16],
            "the top 4 rows must carry the neighbour's blended motion"
        );
        assert_eq!(
            blended[4 * 16..],
            base[4 * 16..],
            "rows below the overlap band keep the block's own prediction"
        );
        // And the chroma-only arm (the chroma walk's call shape — no luma
        // buffer) must not panic and must not touch luma.
        let mut u = alloc::vec![512u16; 8 * 4];
        let mut v = alloc::vec![512u16; 8 * 4];
        apply_obmc_hbd_postpass(
            &refs,
            &mi_grid,
            mi_stride,
            ic,
            0,
            8,
            16,
            8,
            64,
            W,
            H,
            bd,
            &mut [],
            0,
            &mut u,
            &mut v,
            8,
        );
    }
}
