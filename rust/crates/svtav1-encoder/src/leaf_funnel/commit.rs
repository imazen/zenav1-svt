//! Commit an evaluated winner back into the neighbour arrays and recon planes.
//!
//! C `md_update_all_neighbour_arrays` plus the MD recon plane writes
//! `copy_recon_md` feeds.
//!
//! Split out of `leaf_funnel/mod.rs` on 2026-08-25. PURE CODE MOVEMENT: every
//! item keeps its name, order and effective visibility (file-private became
//! `pub(super)`, the same scope).

use super::*;

/// Commit an evaluated winner — C `md_update_all_neighbour_arrays` (+ the
/// MD recon plane writes `copy_recon_md` feeds): luma recon into
/// `y_recon`, chroma into the funnel's decision planes, mode/skip/uv
/// rows, chosen-tx txfm dims, per-txb + chroma coefficient contexts.
/// Every array write spans exactly the block, so re-committing a parent
/// block after its children were committed overwrites them completely
/// (the C winner-overwrite in `test_split_partition`).
// `clippy::manual_checked_ops` post-dates the 1.89 MSRV floor's clippy, so the
// allow has to tolerate being unknown there (`cargo +1.89 clippy` otherwise
// reports `unknown lint` at this line).
#[allow(unknown_lints, clippy::manual_checked_ops)] // the `> 0` guard scopes a whole block, not a single
// division; `checked_div` cannot express it without restructuring hot RD control flow
pub(crate) fn commit_leaf(
    fx: &mut FunnelCtx<'_>,
    y_recon: &mut [u8],
    y_stride: usize,
    ev: &LeafEval,
    // IBC chunk 8: the C PartitionType stamped onto the mi map with this
    // block (C `svt_aom_update_mi_map(pcs, ctx, pc_tree->partition, ...)`,
    // product_coding_loop.c:670 — the currently-evaluated shape during the
    // NSQ walk, the winning shape at the final re-stamp :10696). Dead when
    // the frame has no IBC state (the map is None).
    partition: u8,
) {
    let (abs_x, abs_y) = (ev.abs_x, ev.abs_y);
    let (w, h) = (ev.w, ev.h);
    // IBC chunk 8: stamp the MD mi map (C svt_aom_update_mi_map) — the
    // INTRA_FRAME MVP scans read these entries. Stamped at every mid-walk
    // commit and NEVER restored by node snapshots, mirroring C (losing
    // shapes' stamps linger until overwritten).
    if let Some(mvp) = fx.ibc_mvp.as_deref_mut() {
        let stride = fx.ibc.map(|i| i.mi_cols as usize).unwrap_or(0);
        if stride > 0 {
            let entry = crate::intrabc_mvp::MvpMiEntry {
                bsize: c_bsize_index(w, h) as u8,
                mode: ev.win.mode,
                use_intrabc: ev.win.ibc.is_some(),
                ref_frame: [0, -1], // {INTRA_FRAME, NONE_FRAME}
                mv: [
                    ev.win.ibc.map(|(dv, _)| dv).unwrap_or_default(),
                    svtav1_types::motion::Mv::default(),
                ],
                partition,
            };
            let (mi_x, mi_y) = (abs_x / 4, abs_y / 4);
            for my in mi_y..(mi_y + h / 4).min(mvp.len() / stride) {
                for cell in mvp
                    [my * stride + mi_x..(my * stride + mi_x + w / 4).min((my + 1) * stride)]
                    .iter_mut()
                {
                    *cell = entry;
                }
            }
        }
    }
    let (ccx, ccy, cw, chh) = (ev.ccx, ev.ccy, ev.cw, ev.chh);
    let cand = &ev.win;
    // Task #95 (both-partial p6 mode flip): a boundary block whose recon
    // STRADDLES past the aligned width writes columns `abs_x..abs_x+w` into a
    // row of stride `y_stride` (= the aligned width). When `abs_x + w >
    // y_stride`, the off-aligned columns spill past the row boundary and — the
    // recon buffer being SB-extent-sized but aligned-strided — WRAP into the
    // NEXT row's low columns, silently corrupting an already-committed
    // neighbour SB's recon that a later SB then reads as its intra-prediction
    // reference (e.g. an aligned-72 frame's SB(0,1) VERT 32x64 at x64..96 wraps
    // cols 72..96 into the next row's cols 0..24, flat-filling SB(0,0)'s
    // row-63 V_PRED reference → V mispredicts → DC wins → byte divergence).
    // C's recon buffer has an SB-extent stride so the straddle lands in place;
    // the off-aligned columns are never READ by any in-frame block (nothing
    // predicts, deblocks, or outputs past the aligned extent), so clipping the
    // write to the row boundary matches C's readable recon exactly and is
    // byte-neutral where nothing straddles (`abs_x + w <= y_stride`).
    let wr = w.min(y_stride.saturating_sub(abs_x));
    for r in 0..h {
        let dst = (abs_y + r) * y_stride + abs_x;
        y_recon[dst..dst + wr].copy_from_slice(&cand.y_recon[r * w..r * w + wr]);
    }
    // bd10 mode funnel (task #94): write the winner's 10-bit recon into the
    // bd10 canvas for the next block's neighbour prediction (same straddle clip
    // as the u8 recon above). `None` on the u8 path — byte-neutral for bd8.
    if let Some(canvas10) = fx.y_recon10.as_deref_mut() {
        for r in 0..h {
            let dst = (abs_y + r) * y_stride + abs_x;
            canvas10[dst..dst + wr].copy_from_slice(&ev.win_recon10[r * w..r * w + wr]);
        }
    }
    if ev.has_uv {
        // Same straddle clip on chroma (c_stride = the aligned chroma width).
        let cwr = cw.min(fx.c_stride.saturating_sub(ccx));
        for r in 0..chh {
            let dst = (ccy + r) * fx.c_stride + ccx;
            fx.u_recon[dst..dst + cwr].copy_from_slice(&cand.u_recon[r * cw..r * cw + cwr]);
            fx.v_recon[dst..dst + cwr].copy_from_slice(&cand.v_recon[r * cw..r * cw + cwr]);
        }
        // bd10 FULL-RD chroma canvases — the chroma twin of the luma write
        // above, closing the same sequential coupling for chroma prediction.
        if !ev.win_u_recon10.is_empty() {
            let c_stride = fx.c_stride;
            for (canvas, src) in [
                (fx.u_recon10.as_deref_mut(), &ev.win_u_recon10),
                (fx.v_recon10.as_deref_mut(), &ev.win_v_recon10),
            ] {
                let canvas = canvas.expect("bd10 full-RD requires both chroma canvases");
                for r in 0..chh {
                    let dst = (ccy + r) * c_stride + ccx;
                    canvas[dst..dst + cwr].copy_from_slice(&src[r * cw..r * cw + cwr]);
                }
            }
        }
        // SVTAV1_CEDGE: the committed winner's recon EDGES, mirroring the C
        // `--wrap svt_aom_update_mi_map` `CEDGE` dump (blk_ptr->neigh_top/
        // left_recon_16bit = the block's bottom row / right column). Joining
        // the two bisects an MD recon drift to its first divergent block, and
        // separates a LUMA root (lyb/lyr, which also feeds CfL's AC) from a
        // CHROMA one (cu/cv, whose average IS the next block's DC base).
        #[cfg(feature = "std")]
        if !ev.win_u_recon10.is_empty() && {
            static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
            dbg_on(&ON, "SVTAV1_CEDGE")
        } {
            let lyb: u64 = ev.win_recon10[(h - 1) * w..h * w]
                .iter()
                .map(|&s| u64::from(s))
                .sum();
            let lyr: u64 = (0..h)
                .map(|r| u64::from(ev.win_recon10[r * w + w - 1]))
                .sum();
            let col = |v: &[u16]| {
                (0..chh)
                    .map(|r| v[r * cw + cw - 1].to_string())
                    .collect::<alloc::vec::Vec<_>>()
                    .join(",")
            };
            // Raw luma edges for one pinned block (SVTAV1_CEDGE_XY="x,y") —
            // which SAMPLES differ localises a divergence to one TX unit.
            static XY: std::sync::OnceLock<Option<(usize, usize)>> = std::sync::OnceLock::new();
            let raw = (dbg_xy(&XY, "SVTAV1_CEDGE_XY") == Some((abs_x, abs_y))).then(|| {
                let j = |it: alloc::vec::Vec<u16>| {
                    it.iter()
                        .map(|v| v.to_string())
                        .collect::<alloc::vec::Vec<_>>()
                        .join(",")
                };
                alloc::format!(
                    " lyB={} lyR={}",
                    j(ev.win_recon10[(h - 1) * w..h * w].to_vec()),
                    j((0..h).map(|r| ev.win_recon10[r * w + w - 1]).collect())
                )
            });
            eprintln!(
                "CEDGE org=({abs_x},{abs_y}) {w}x{h} lyb={lyb} lyr={lyr} uvr={cw}x{chh} cu={} cv={}{}",
                col(&ev.win_u_recon10),
                col(&ev.win_v_recon10),
                raw.unwrap_or_default()
            );
        }
    }
    let skip = !cand.block_has_coeff;
    fx.ectx
        .record_block(abs_x, abs_y, w, h, cand.mode, cand.uv, skip);
    // IBC chunk 9 (Root 6 twin, MD side): stamp the inter-neighbour dims
    // — the funnel's tx_size_ctx reads them for the C is_inter override.
    fx.ectx
        .record_inter_dims(abs_x, abs_y, w, h, cand.ibc.is_some());
    // MD-time palette neighbour state (C mbmi->palette_mode_info, stamped for
    // EVERY committed winner in coding order — mirrors the pack walk's
    // record_palette + the record_block above). Read back by the NEXT
    // block's evaluate_leaf via palette_cache (colour cache / centroid snap)
    // and palette_neighbor_ctx (mode-flag ctx). None for a non-palette
    // winner => neighbour state stays empty, so non-screen content (no
    // palette winner) is byte-identical.
    fx.ectx.record_palette(
        abs_x,
        abs_y,
        w,
        h,
        cand.palette
            .as_ref()
            .map(|(colors, _idx)| colors.as_slice()),
    );
    // MD partition-context bytes (mode_decision_update_neighbor_arrays,
    // product_coding_loop.c:179-192: partition_context_lookup[bsize]
    // written over the block span — per-DIMENSION levels for rect NSQ
    // children). Consumed by the depth walk's partition rates
    // (update_part_neighs); inert for the fixed-tree paths (nothing
    // reads the decision ectx's partition bytes there).
    fx.ectx.update_partition_ctx_leaf(abs_x, abs_y, w, h);
    // set_txfm_ctxs with the CHOSEN tx dims (mode_decision_update:246-256)
    // — the skip && is_inter arm stores the BLOCK dims instead (IntraBC
    // skip winners; entropy_coding.c:4620-4624).
    let (txw, txh) = txb_dims_at_depth(w, h, cand.tx_depth);
    if cand.ibc.is_some() && skip {
        fx.ectx.record_txfm_dims(abs_x, abs_y, w, h, w, h);
    } else {
        fx.ectx.record_txfm_dims(abs_x, abs_y, w, h, txw, txh);
    }
    // Per-txb luma cul bytes; chroma culs over the chroma span. The
    // winner's txb arrays are stored in the SEARCH walk order — the
    // inter z-order (txb_org_inter) for IntraBC winners, raster for
    // intra — so the index -> position mapping must match, or a depth-2
    // IBC winner's culs land on the wrong cells and every later block's
    // coeff contexts (pack vs decode) desync.
    let cols = w / txw;
    for (txb, &cul) in cand.txb_cul.iter().enumerate() {
        let (tx_x, tx_y) = if cand.ibc.is_some() {
            txb_org_inter(w, h, cand.tx_depth, txb)
        } else {
            ((txb % cols) * txw, (txb / cols) * txh)
        };
        fx.ectx
            .record_coeff(abs_x + tx_x, abs_y + tx_y, txw, txh, cul);
    }
    if ev.has_uv {
        fx.ectx.record_coeff_uv(0, ccx, ccy, cw, chh, cand.u_cul);
        fx.ectx.record_coeff_uv(1, ccx, ccy, cw, chh, cand.v_cul);
    }
}
