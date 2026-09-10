//! The 10-bit level re-encode pass, kept beside its pipeline caller.

use super::{Bd10CoeffNeighbors, Bd10ModeNeighbors};

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
                    let hbd = inter_refs
                        .and_then(|r| r[ic.ref_frame[0].max(0) as usize])
                        .and_then(|p| p.hbd.as_ref())
                        .expect(
                            "an inter leaf reached the bd10 re-encode with no 10-bit reference \
                             in the DPB; `bd10_tree_supported` is supposed to have refused the \
                             frame before this point",
                        );
                    crate::inter_pred_arm::predict_inter_leaf_hbd(
                        &hbd.y,
                        None,
                        ic.motion_mode,
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
    for (sb_idx, tree) in all_trees.iter_mut().enumerate() {
        let sb_col = sb_idx % sb_cols;
        let sb_row = sb_idx / sb_cols;
        let tile_mi_c = tile_grid.tile_mi_for_sb(sb_row, sb_col, sb_size / 2, cframe_w, cframe_h);
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
            tile_mi_c,
            inter_refs,
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
    // ISSUE #18: this superblock's tile, in the CHROMA plane's own pixel
    // domain (the geom below is `ss: 0` over `cframe_*`). Was
    // `TileMi::whole_frame(cframe_w, cframe_h)`.
    tile_mi_c: crate::intra_edge::TileMi,
    // The DPB's reference pictures, for the INTER arm. `None` on a key frame.
    inter_refs: Option<&[Option<&crate::picture::PaddedRef>; 8]>,
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
            let uv_tt = crate::leaf_funnel::uv_tx_type(d.uv_mode, cw, ch);
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
            let geom = crate::leaf_funnel::UnitGeom {
                partition: svtav1_types::partition::PartitionType::None,
                mi_row: cy >> 2,
                mi_col: cx >> 2,
                bw_px: cw,
                bh_px: ch,
                sb_mi_size,
                ss: 0,
                frame_w: cframe_w,
                frame_h: cframe_h,
                // ISSUE #18: see the luma twin above. Was
                // `TileMi::whole_frame(cframe_w, cframe_h)`.
                tile: tile_mi_c,
            };
            // THE INTER ARM, chroma half. Built once for both planes because
            // C's chroma prediction is one call with a halved origin over the
            // LUMA block's subpel result — predicting each plane separately
            // would be different arithmetic (see `predict_inter_yuv_hbd`).
            let (inter_u, inter_v) = match d.inter.as_deref() {
                Some(ic) => {
                    let r = inter_refs
                        .and_then(|r| r[ic.ref_frame[0].max(0) as usize])
                        .expect(
                            "an inter leaf reached the bd10 chroma re-encode with no DPB picture",
                        );
                    let hbd = r.hbd.as_ref().expect(
                        "an inter leaf reached the bd10 chroma re-encode with no 10-bit reference",
                    );
                    let (bw, bh) = (d.width as usize, d.height as usize);
                    // A scratch luma destination: the driver predicts luma and
                    // chroma together, and only the chroma halves are read here
                    // (the luma pass already wrote its own).
                    let mut y_scratch = alloc::vec![0u16; bw * bh];
                    let mut u = alloc::vec![0u16; cw * ch];
                    let mut v = alloc::vec![0u16; cw * ch];
                    crate::inter_pred_arm::predict_inter_leaf_hbd(
                        &hbd.y,
                        hbd.uv.as_ref().map(|(u, v)| (u, v)),
                        ic.motion_mode,
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
                        sb_mi_size * 4,
                        cframe_w * 2,
                        cframe_h * 2,
                        bd,
                        &mut y_scratch,
                        bw,
                        &mut u,
                        &mut v,
                        cw,
                    );
                    (Some(u), Some(v))
                }
                None => (None, None),
            };
            let (u_q, u_eob, u_rec) = bd10_reencode_chroma_plane(
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
            );
            let (v_q, v_eob, v_rec) = bd10_reencode_chroma_plane(
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
            );
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
                    tile_mi_c,
                    inter_refs,
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
        let hbd = inter_refs
            .and_then(|r| r[ic.ref_frame[0].max(0) as usize])
            .and_then(|p| p.hbd.as_ref())
            .expect("an inter leaf reached the bd10 TXS re-encode with no 10-bit reference");
        let mut p = alloc::vec![0u16; bw * bh];
        crate::inter_pred_arm::predict_inter_leaf_hbd(
            &hbd.y,
            None,
            ic.motion_mode,
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
    // Straddle clip, exactly as the depth-0 arm does.
    let bwr = bw.min(stride.saturating_sub(x));
    for r in 0..bh {
        let drow = (y + r) * stride + x;
        if drow + bwr <= recon10.len() {
            recon10[drow..drow + bwr].copy_from_slice(&dep_recon[r * bw..r * bw + bwr]);
        }
    }
}
