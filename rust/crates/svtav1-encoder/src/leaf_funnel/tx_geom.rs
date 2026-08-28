//! Transform-unit geometry: how a block's tx depth maps to txb dimensions and
//! origins, which blocks signal a tx size at all, and the chroma tx type.
//!
//! C `get_end_tx_depth` (product_coding_loop.c:4171), the `tx_org` tables
//! (transforms.c:48), and `av1_get_tx_type`'s chroma arm. Pure geometry: no
//! pixels, no rates, no RD.
//!
//! Split out of `leaf_funnel/mod.rs` on 2026-08-25. PURE CODE MOVEMENT: every
//! item keeps its name, order and effective visibility (file-private became
//! `pub(super)`, the same scope).

use super::*;

/// C `get_end_tx_depth` (product_coding_loop.c:4171) clamped by
/// `intra_class_max_depth_sq` / `_nsq` (get_start_end_tx_depth :6973;
/// shape == PART_N <=> w == h — HVA/HVB shapes with square children are
/// geometry-disabled at every funnel preset).
pub(super) fn end_tx_depth(w: usize, h: usize, cfg: &FunnelCfg) -> u8 {
    let base: u8 = match (w, h) {
        // 2-depth blocks (the bsize list at :4173-4176).
        (64, 64) | (32, 32) | (16, 16) => 2,
        (64, 32) | (32, 64) | (32, 16) | (16, 32) | (16, 8) | (8, 16) => 2,
        (64, 16) | (16, 64) | (32, 8) | (8, 32) | (16, 4) | (4, 16) => 2,
        (8, 8) => 1,
        _ => 0, // 8x4, 4x8, 4x4
    };
    let cap = if w == h {
        cfg.txs_max_sq
    } else {
        cfg.txs_max_nsq
    };
    base.min(cap)
}

/// The INTER-class twin of [`end_tx_depth`]: same bsize base, capped by
/// `txs_ctrls.inter_class_max_depth_sq/nsq`. NOT what C applies to
/// IntraBC (C's clamp is mode-keyed -> intra caps; see the pinned KNOWN
/// GAP note at the depth-loop call site) — kept as the port's IBC cap so
/// every emitted stream stays within the proven depth<=1 pack chain.
pub(super) fn end_tx_depth_inter(w: usize, h: usize, cfg: &FunnelCfg) -> u8 {
    let base: u8 = match (w, h) {
        (64, 64) | (32, 32) | (16, 16) => 2,
        (64, 32) | (32, 64) | (32, 16) | (16, 32) | (16, 8) | (8, 16) => 2,
        (64, 16) | (16, 64) | (32, 8) | (8, 32) | (16, 4) | (4, 16) => 2,
        (8, 8) => 1,
        _ => 0,
    };
    let cap = if w == h {
        cfg.txs_inter_max_sq
    } else {
        cfg.txs_inter_max_nsq
    };
    base.min(cap)
}

/// Per-txb origin at a depth for an INTER-classified (IntraBC) block —
/// C `tx_org[bsize][is_inter=1][depth][txb]` (transforms.c:48). Depths
/// 0/1 equal the intra raster; at depth 2 the inter rows are the
/// RECURSIVE var-tx z-order — depth-1 parents in raster, the 2x2
/// sub-txbs raster within each parent (verified against the C table:
/// exactly 6 (bsize, depth-2) cells differ from the intra raster —
/// 16X8/16X16/32X16/32X32/64X32/64X64; vertical rects coincide).
/// Currently unreachable at the IBC presets (inter depth caps <= 1) but
/// kept exact for when deeper inter caps arrive.
pub(crate) fn txb_org_inter(w: usize, h: usize, depth: u8, txb: usize) -> (usize, usize) {
    let (txw, txh) = txb_dims_at_depth(w, h, depth);
    if depth < 2 {
        let cols = w / txw;
        return ((txb % cols) * txw, (txb / cols) * txh);
    }
    // Parent (depth-1) geometry.
    let (pw, ph) = txb_dims_at_depth(w, h, 1);
    let sub_per_parent = (pw / txw) * (ph / txh);
    let parent = txb / sub_per_parent;
    let within = txb % sub_per_parent;
    let pcols = w / pw;
    let (px, py) = ((parent % pcols) * pw, (parent / pcols) * ph);
    let scols = pw / txw;
    (px + (within % scols) * txw, py + (within / scols) * txh)
}

/// C `bsize_to_tx_size_cat`: category of the block's max tx size chain —
/// `TXSIZE_SQR_UP` of the max rect TX (== the larger block dim as a
/// square), minus TX_8X8, capped at MAX_TX_CATS-1. 4x8/8x4 -> TX_8X8 ->
/// cat 0; 4x16/16x4 -> TX_16X16 -> cat 1.
pub(super) fn tx_size_cat(w: usize, h: usize) -> usize {
    match w.max(h) {
        4 | 8 => 0,
        16 => 1,
        32 => 2,
        _ => 3, // 64 (TX_64X64 -> cat 3)
    }
}

/// C `block_signals_txsize` (rd_cost.c:1508): `bsize > BLOCK_4X4`. Every block
/// EXCEPT the 4x4 codes a tx_size symbol; for the 4x4 `svt_aom_tx_size_bits`
/// (rd_cost.c:1761) returns 0. The RD of a 4x4 leaf must therefore carry NO
/// tx_size rate — the port previously added `tx_size[cat 0][ctx][0]` (~365 rate
/// units) unconditionally, inflating every 4x4's cost and wrongly keeping an
/// 8x8 where C splits it to four 4x4 (first real-content M2/M3 partition flip).
pub(super) fn block_signals_txsize(w: usize, h: usize) -> bool {
    !(w == 4 && h == 4)
}

/// C `tx_depth_to_tx_size[depth][bsize]` (common_utils.c:95) — the TX
/// dims at a given depth — plus the txb count/raster geometry
/// (`tx_blocks_per_depth` / the intra `tx_org` rows, transforms.c:48;
/// pinned against the instrumented dump in the tests below). Positions
/// are plain raster: x fastest, `w/txw` columns.
pub(crate) fn txb_dims_at_depth(w: usize, h: usize, depth: u8) -> (usize, usize) {
    let (mut tw, mut th) = (w.min(64), h.min(64));
    for _ in 0..depth {
        (tw, th) = sub_tx_dims(tw, th);
    }
    (tw, th)
}

/// C `sub_tx_size_map` chain expressed on dims: square TXs halve both
/// dims (min 4); 2:1 rects halve the long dim; 4:1 rects halve the long
/// dim (64x16 -> 32x16 -> 16x16 per the table).
pub(super) fn sub_tx_dims(tw: usize, th: usize) -> (usize, usize) {
    if tw == th {
        ((tw / 2).max(4), (th / 2).max(4))
    } else if tw > th {
        (tw / 2, th)
    } else {
        (tw, th / 2)
    }
}

/// C `non_normative_txs` (product_coding_loop.c:9641): re-transform the
/// shared MDS3 residual workspace (`cand_bf->residual` = the LAST MDS3
/// candidate's whole-block depth-0 residual — every MDS3 candidate
/// full-loops through ONE pixel workspace at staging mode 1; pointer-
/// instrumented) with the two half-height TXs (H split) and the two
/// half-width TXs (V split), DCT_DCT + `svt_aom_quantize_inv_quantize_
/// light` (plain quantize_b, y tables, full_loop.c:1253), and return
/// the min eob per split direction. `None` when the winner kept no
/// coefficients (C leaves the ~0 sentinels, so the psq gate can't
/// fire).
pub(crate) fn min_nz_hv(
    ev: &LeafEval,
    qindex: u8,
    qm_level_y: u8,
    bit_depth: u8,
) -> Option<(u16, u16)> {
    if !ev.block_has_coeff() {
        return None;
    }
    let (w, h) = (ev.w, ev.h);
    debug_assert!(w == h && w >= 8, "psq gate runs on SQ blocks only");
    // bd10 (task #94, root #2): C's `non_normative_txs` transforms + quantizes
    // this residual at `EB_TEN_BIT` — Q10 tables + `svt_aom_highbd_quantize_b`
    // (full_loop.c:1288). Deciding the H/V nz counts on the bd8 residual + Q8
    // quant flips the `skip_by_sq_txs` gate. bd8 keeps `build_quant_table` +
    // `psq_resid` + `quantize_b`, so it is byte-unchanged by construction.
    let bd10 = bit_depth > 8 && !ev.psq_resid10().is_empty();
    let mut qt = if bd10 {
        crate::quant::build_quant_table_bd(qindex, bit_depth)
    } else {
        crate::quant::build_quant_table(qindex)
    };
    // C's light quantize applies the PLANE_Y QM here too (full_loop.c:1282).
    qt.qm_level = qm_level_y;
    let resid = if bd10 {
        ev.psq_resid10()
    } else {
        ev.psq_resid()
    };
    debug_assert_eq!(resid.len(), w * h);

    let half_eob = |ox: usize, oy: usize, tw: usize, th: usize| -> u16 {
        let n = tw * th;
        let c_tx = cc::tx_size_from_dims(tw, th);
        let mut residual = vec![0i32; n];
        for r in 0..th {
            let rrow = (oy + r) * w + ox;
            residual[r * tw..(r + 1) * tw].copy_from_slice(&resid[rrow..rrow + tw]);
        }
        let mut coeffs = vec![0i32; n];
        let ok = svtav1_dsp::txfm_dispatch::fwd_txfm2d_dispatch(
            &residual,
            &mut coeffs,
            tw,
            rs_tx_size(tw, th),
            TX_TYPE_FROM_C[cc::DCT_DCT],
        );
        debug_assert!(ok, "psq fwd txfm {tw}x{th}");
        // 64-dim fold (the 64x32/32x64 halves of a 64x64 block).
        let (pw, ph) = (tw.min(32), th.min(32));
        let packed = if tw > 32 || th > 32 {
            let mut v = vec![0i32; pw * ph];
            for r in 0..ph {
                v[r * pw..(r + 1) * pw].copy_from_slice(&coeffs[r * tw..r * tw + pw]);
            }
            v
        } else {
            coeffs
        };
        let scan = crate::entropy::scan_tables::scan(
            c_tx,
            crate::entropy::scan_tables::TX_TYPE_TO_SCAN_INDEX[cc::DCT_DCT] as usize,
        );
        let mut qcoeff = vec![0i32; pw * ph];
        let mut dqcoeff = vec![0i32; pw * ph];
        match if qt.qm_level < 15 {
            crate::qm::qm_slices(usize::from(qt.qm_level), false, c_tx)
        } else {
            None
        } {
            Some((wt, iwt)) if bd10 => crate::qm::quantize_b_hbd_qm(
                &packed,
                scan,
                &qt,
                TX_SCALE_TAB[c_tx],
                wt,
                iwt,
                &mut qcoeff,
                &mut dqcoeff,
            ),
            Some((wt, iwt)) => crate::qm::quantize_b_qm(
                &packed,
                scan,
                &qt,
                TX_SCALE_TAB[c_tx],
                wt,
                iwt,
                &mut qcoeff,
                &mut dqcoeff,
            ),
            None if bd10 => crate::quant::quantize_b_hbd(
                &packed,
                scan,
                &qt,
                TX_SCALE_TAB[c_tx],
                &mut qcoeff,
                &mut dqcoeff,
            ),
            None => crate::quant::quantize_b(
                &packed,
                scan,
                &qt,
                TX_SCALE_TAB[c_tx],
                &mut qcoeff,
                &mut dqcoeff,
            ),
        }
    };

    let mut nz_h = u16::MAX;
    for part in 0..2usize {
        nz_h = nz_h.min(half_eob(0, part * (h / 2), w, h / 2));
    }
    let mut nz_v = u16::MAX;
    for part in 0..2usize {
        nz_v = nz_v.min(half_eob(part * (w / 2), 0, w / 2, h));
    }
    Some((nz_h, nz_v))
}

/// Chroma tx type: C `svt_aom_get_intra_uv_tx_type`
/// (mode_decision.c:2991) = `g_intra_mode_to_tx_type[uv_mode]` clamped to
/// DCT_DCT when the chroma tx size's intra ext set doesn't carry the
/// type (32x32 chroma is DCT-only; the WIN dumps' ttuv fields pin the
/// mapping). The uv tx type affects the SCAN + coeff coding only when
/// eob > 0.
pub(crate) fn uv_tx_type(uv: u8, cw: usize, chh: usize) -> usize {
    /// C `g_intra_mode_to_tx_type[INTRA_MODES]` (DCT=0, ADST_DCT=1,
    /// DCT_ADST=2, ADST_ADST=3).
    const MODE_TO_TX: [usize; 13] = [0, 1, 2, 0, 3, 1, 2, 2, 1, 3, 1, 2, 3];
    // UV_CFL_PRED (13): C forces transform_type_uv = DCT_DCT
    // (product_coding_loop.c:3789); the decoder derives DCT_DCT for CfL too.
    if uv as usize == UV_CFL_PRED_IDX {
        return cc::DCT_DCT;
    }
    let t = MODE_TO_TX[uv as usize];
    // DCT-only tx sizes (>= 32 in either dim).
    if cw >= 32 || chh >= 32 {
        cc::DCT_DCT
    } else {
        t
    }
}
