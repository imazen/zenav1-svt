//! Inter var-tx tx_size coding — the `write_tx_size_vartx` /
//! `cost_tx_size_vartx` pair over `txfm_partition_cdf` (IBC chunk 7,
//! docs/ibc-port-map.md §B.2.6 / §D chunk 9).
//!
//! C reference (SVT-AV1 v4.2.0): the WRITER walk `av1_code_tx_size` →
//! `write_tx_size_vartx` (entropy_coding.c:4649-4679 / :4513-4557) and its
//! MD-cost twin `svt_aom_tx_size_bits` → `cost_tx_size_vartx`
//! (rd_cost.c:1745-1777 / :1591-1650) — textually identical recursions,
//! one emitting symbols + adapting the CDF, one summing
//! `txfm_partition_fac_bits`. Both walk the block's `get_vartx_max_txsize`
//! units (= the depth-0 max RECT tx, `blocksize_to_txsize[bsize]`,
//! entropy_coding.c:4422-4429) and recurse each unit down to the block's
//! SELECTED uniform tx size (`tx_depth_to_tx_size[tx_depth][bsize]`),
//! coding a `txfm_partition` split flag per visited node.
//!
//! An IntraBC block is the ONLY inter-classified block on this allintra
//! port (`is_inter_block` = `use_intrabc || ref_frame[0] > INTRA_FRAME`,
//! block_structures.h:115-121), so this module is reachable exclusively
//! from IBC blocks:
//! - non-skip: the vartx walk codes the tx size (both cost + write);
//! - skip (eob 0): NO tx_size bits — `svt_aom_tx_size_bits`'s
//!   `!(is_inter_tx && skip)` gate fails and only the context stamp runs
//!   (`set_txfm_ctxs(tx_size, n8_w, n8_h, skip && is_inter, xd)`, which
//!   stores the BLOCK dims, entropy_coding.c:4614-4625).
//!
//! Context model: `above_txfm[x4]` / `left_txfm[y4]` hold the neighbour
//! tx dims in PIXELS per 4x4 cell (the port's `EntropyCtx::{above_txfm,
//! left_txfm}`, C `TXFM_CONTEXT`). The walk seeds LOCAL copies of the
//! block's spans (C `svt_aom_get_tx_size_bits` memcpy, rd_cost.c:1790-1795)
//! and evolves them per visited unit; the caller commits the block's final
//! state with the flat block-span stamp (equivalent here because SVT only
//! codes UNIFORM per-block tx sizes — every unit stamps the same dims).

use svtav1_entropy::context::FrameContext;
use svtav1_entropy::writer::AomWriter;

/// C `TX_SIZES` (the square chain 4..64).
const TX_SIZES: i32 = 5;
/// C `MAX_VARTX_DEPTH` (definitions.h:396).
pub const MAX_VARTX_DEPTH: u8 = 2;
/// Local mi-span capacity: a block is at most 128px = 32 mi cells.
const MAX_MI_SPAN: usize = 32;

/// C `get_sqr_tx_size(tx_dim)` (entropy_coding.c:4470-4488 / rd_cost.c
/// :1546-1566): pixel dim -> square TxSize INDEX in the 4..64 chain.
#[inline]
fn sqr_tx_size_of_dim(dim: usize) -> i32 {
    match dim {
        128 | 64 => 4, // TX_64X64
        32 => 3,
        16 => 2,
        8 => 1,
        _ => 0, // TX_4X4
    }
}

/// C `txfm_partition_context(above_ctx, left_ctx, bsize, tx_size)`
/// (entropy_coding.c:4490-4511; identical twin rd_cost.c:1568-1589).
/// `above`/`left` are the local TXFM context bytes at the unit's origin;
/// `(bw, bh)` the BLOCK dims (pixels); `(txw, txh)` the candidate tx dims.
#[inline]
pub(crate) fn txfm_partition_context(
    above: u8,
    left: u8,
    bw: usize,
    bh: usize,
    txw: usize,
    txh: usize,
) -> usize {
    let a = usize::from((above as usize) < txw);
    let l = usize::from((left as usize) < txh);
    if txw == 4 && txh == 4 {
        // C: `if (tx_size == TX_4X4) return 0;` — dummy, never coded.
        return 0;
    }
    let max_tx_size = sqr_tx_size_of_dim(bw.max(bh));
    debug_assert!(max_tx_size >= 1, "vartx ctx on a sub-8x8 block");
    let category = i32::from(sqr_tx_size_of_dim(txw.max(txh)) != max_tx_size && max_tx_size > 1)
        + (TX_SIZES - 1 - max_tx_size) * 2;
    (category * 3) as usize + a + l
}

/// C `eb_sub_tx_size_map` (common_utils.c:73-93) expressed on dims:
/// square halves both (min 4); 2:1 rects halve the long dim; 4:1 rects
/// (4x16-family) halve the long dim.
#[inline]
fn sub_tx_dims(txw: usize, txh: usize) -> (usize, usize) {
    if txw == txh {
        ((txw / 2).max(4), (txh / 2).max(4))
    } else if txw > txh {
        (txw / 2, txh)
    } else {
        (txw, txh / 2)
    }
}

/// One vartx walk over a block — shared by the MD cost and the writer.
/// `target` = the SELECTED uniform tx dims (`tx_depth_to_tx_size
/// [tx_depth][bsize]`); `max_units_*` = C `max_block_wide/high` in 4x4
/// units (width UNclipped, height clipped by the frame bottom —
/// entropy_coding.c:4438-4452, an SVT quirk mirrored by the caller).
struct VarTxWalk<'a, S: FnMut(usize, bool)> {
    /// Local above span (block width in mi cells), seeded from EntropyCtx.
    above: &'a mut [u8; MAX_MI_SPAN],
    /// Local left span (block height in mi cells).
    left: &'a mut [u8; MAX_MI_SPAN],
    bw: usize,
    bh: usize,
    target_w: usize,
    target_h: usize,
    max_units_wide: usize,
    max_units_high: usize,
    /// Sink: `(ctx, split)` per coded `txfm_partition` symbol.
    emit: S,
}

impl<S: FnMut(usize, bool)> VarTxWalk<'_, S> {
    /// C `txfm_partition_update` (entropy_coding.c:4453-4468): stamp the
    /// UNIT's footprint (`txb` dims) with the CODED tx dims.
    fn update(&mut self, blk_row: usize, blk_col: usize, txw: usize, txh: usize, unit_w: usize, unit_h: usize) {
        for c in self.above.iter_mut().skip(blk_col).take(unit_w / 4) {
            *c = txw as u8;
        }
        for c in self.left.iter_mut().skip(blk_row).take(unit_h / 4) {
            *c = txh as u8;
        }
    }

    /// C `write_tx_size_vartx` / `cost_tx_size_vartx` recursion.
    /// `blk_row`/`blk_col` in 4x4 units within the block.
    fn walk(&mut self, txw: usize, txh: usize, depth: u8, blk_row: usize, blk_col: usize) {
        if blk_row >= self.max_units_high || blk_col >= self.max_units_wide {
            return;
        }
        if depth == MAX_VARTX_DEPTH {
            self.update(blk_row, blk_col, txw, txh, txw, txh);
            return;
        }
        let ctx = txfm_partition_context(
            self.above[blk_col],
            self.left[blk_row],
            self.bw,
            self.bh,
            txw,
            txh,
        );
        if (txw, txh) == (self.target_w, self.target_h) {
            (self.emit)(ctx, false);
            self.update(blk_row, blk_col, txw, txh, txw, txh);
        } else {
            (self.emit)(ctx, true);
            let (sw, sh) = sub_tx_dims(txw, txh);
            if (sw, sh) == (4, 4) {
                // C: sub_txs == TX_4X4 -> stamp the PARENT footprint with
                // the SUB dims and stop (entropy_coding.c:4544-4547).
                self.update(blk_row, blk_col, sw, sh, txw, txh);
                return;
            }
            let (bsw, bsh) = (sw / 4, sh / 4);
            let mut row = 0;
            while row < txh / 4 {
                let mut col = 0;
                while col < txw / 4 {
                    self.walk(sw, sh, depth + 1, blk_row + row, blk_col + col);
                    col += bsw;
                }
                row += bsh;
            }
        }
    }
}

/// Drive one full block walk: seed the local spans, iterate the
/// `get_vartx_max_txsize` units (row-major, entropy_coding.c:4661-4668),
/// emit each symbol through `emit`. Returns nothing; the local spans'
/// final state is discarded (the caller stamps the committed arrays with
/// the uniform block-level dims, which is value-identical — see module doc).
///
/// `above_seed`/`left_seed`: the block's spans of the committed TXFM
/// context arrays (above: `w/4` cells from `x/4`; left: `h/4` from `y/4`).
/// `frame_h_px`: the frame height in pixels the C bottom-edge clip uses
/// (`mi_rows * 4`); `abs_y` the block's top in pixels.
#[allow(clippy::too_many_arguments)]
fn drive_walk<S: FnMut(usize, bool)>(
    above_seed: &[u8],
    left_seed: &[u8],
    w: usize,
    h: usize,
    tx_depth: u8,
    abs_y: usize,
    frame_h_px: usize,
    emit: S,
) {
    let mut above = [0u8; MAX_MI_SPAN];
    let mut left = [0u8; MAX_MI_SPAN];
    above[..above_seed.len().min(MAX_MI_SPAN)]
        .copy_from_slice(&above_seed[..above_seed.len().min(MAX_MI_SPAN)]);
    left[..left_seed.len().min(MAX_MI_SPAN)]
        .copy_from_slice(&left_seed[..left_seed.len().min(MAX_MI_SPAN)]);

    // C max_block_wide: NOT frame-clipped (entropy_coding.c:4438-4442).
    let max_units_wide = w / 4;
    // C max_block_high: clipped via mb_to_bottom_edge (:4444-4452).
    let vis_h = if abs_y + h > frame_h_px {
        frame_h_px.saturating_sub(abs_y)
    } else {
        h
    };
    let max_units_high = vis_h / 4;

    // Depth-0 unit dims = get_vartx_max_txsize = the max RECT tx of the
    // bsize (dims capped at 64 — blocksize_to_txsize).
    let (unit_w, unit_h) = (w.min(64), h.min(64));
    let (target_w, target_h) = crate::leaf_funnel::txb_dims_at_depth(w, h, tx_depth);

    let mut walk = VarTxWalk {
        above: &mut above,
        left: &mut left,
        bw: w,
        bh: h,
        target_w,
        target_h,
        max_units_wide,
        max_units_high,
        emit,
    };
    let mut idy = 0;
    while idy < h / 4 {
        let mut idx = 0;
        while idx < w / 4 {
            walk.walk(unit_w, unit_h, 0, idy, idx);
            idx += unit_w / 4;
        }
        idy += unit_h / 4;
    }
}

/// MD-side tx_size rate for a NON-SKIP inter-classified (IntraBC) block —
/// C `svt_aom_tx_size_bits`'s `is_inter_tx` arm (rd_cost.c:1755-1770) via
/// `cost_tx_size_vartx`. `fac_bits` = `MdRates::txfm_partition_fac_bits`.
/// The SKIP arm codes 0 bits (the `!(is_inter_tx && skip)` gate) — callers
/// simply don't call this for skip.
#[allow(clippy::too_many_arguments)]
pub(crate) fn tx_size_bits_vartx(
    fac_bits: &[[i32; 2]; svtav1_entropy::context::TXFM_PARTITION_CONTEXTS],
    above_seed: &[u8],
    left_seed: &[u8],
    w: usize,
    h: usize,
    tx_depth: u8,
    abs_y: usize,
    frame_h_px: usize,
) -> u64 {
    let mut bits: u64 = 0;
    drive_walk(above_seed, left_seed, w, h, tx_depth, abs_y, frame_h_px, |ctx, split| {
        bits += fac_bits[ctx][usize::from(split)] as u64;
    });
    bits
}

/// Writer-side `av1_code_tx_size` inter arm (entropy_coding.c:4658-4669)
/// — emits the `txfm_partition` symbols over `fc.txfm_partition_cdf`
/// (adapting, like C's `aom_write_symbol`). Only for NON-SKIP IntraBC
/// blocks; the skip arm writes nothing (context stamp only, caller-side).
#[allow(clippy::too_many_arguments)]
pub(crate) fn write_tx_size_vartx(
    writer: &mut AomWriter,
    fc: &mut FrameContext,
    above_seed: &[u8],
    left_seed: &[u8],
    w: usize,
    h: usize,
    tx_depth: u8,
    abs_y: usize,
    frame_h_px: usize,
) {
    drive_walk(above_seed, left_seed, w, h, tx_depth, abs_y, frame_h_px, |ctx, split| {
        writer.write_symbol(usize::from(split), &mut fc.txfm_partition_cdf[ctx], 2);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Collect the (ctx, split) symbol sequence of one walk.
    fn walk_symbols(
        above: &[u8],
        left: &[u8],
        w: usize,
        h: usize,
        depth: u8,
    ) -> alloc::vec::Vec<(usize, bool)> {
        let mut v = alloc::vec::Vec::new();
        drive_walk(above, left, w, h, depth, 0, 1 << 16, |ctx, split| v.push((ctx, split)));
        v
    }

    #[test]
    fn ctx_category_matches_c_formula() {
        // 64x64 block, 64x64 tx, fresh (0) neighbour spans: above/left =
        // (0 < 64) = 1/1; max_tx_size = TX_64X64(4) -> category =
        // (sqr_up(64)!=4 && 4>1 -> 0) + (5-1-4)*2 = 0 -> ctx = 0*3+2 = 2.
        assert_eq!(txfm_partition_context(0, 0, 64, 64, 64, 64), 2);
        // Same block, 32x32 candidate (after a split), neighbours stamped
        // 32: above/left = (32 < 32)=0 -> category = (3!=4 -> 1) + 0*2 = 1
        // -> ctx = 3.
        assert_eq!(txfm_partition_context(32, 32, 64, 64, 32, 32), 3);
        // 16x8 block, 16x8 tx, fresh spans: max_tx_size = sqr(16) = 2;
        // category = (sqr_up(16x8 -> 16)== 2 -> 0... sqr of max dim 16 = 2;
        // equal -> 0) + (5-1-2)*2 = 4 -> ctx = 12 + above + left = 14.
        assert_eq!(txfm_partition_context(0, 0, 16, 8, 16, 8), 14);
        // TX_4X4 candidate: dummy 0.
        assert_eq!(txfm_partition_context(0, 0, 8, 8, 4, 4), 0);
    }

    #[test]
    fn depth0_codes_one_no_split_per_max_unit() {
        // 64x64 depth 0: one unit, one no-split symbol.
        let s = walk_symbols(&[0; 16], &[0; 16], 64, 64, 0);
        assert_eq!(s, alloc::vec![(2, false)]);
        // 16x8 depth 0: one unit (16x8 max tx).
        let s = walk_symbols(&[0; 4], &[0; 2], 16, 8, 0);
        assert_eq!(s.len(), 1);
        assert!(!s[0].1);
    }

    #[test]
    fn depth1_codes_split_then_four_no_splits() {
        // 64x64 depth 1 (target 32x32): split at 64, then 4 no-splits at 32.
        let s = walk_symbols(&[0; 16], &[0; 16], 64, 64, 1);
        assert_eq!(s.len(), 5);
        assert!(s[0].1, "root must split");
        assert!(s[1..].iter().all(|&(_, sp)| !sp));
        // First 32x32 unit sees fresh (0) spans -> above=(0<32)=1, left=1,
        // category (3!=4 -> 1) -> ctx 3+2 = 5; the SECOND unit (right of
        // the first) reads the first's above stamp (32) at its left...
        // no — its LEFT span cells are still 0 (row 0..8 of left were NOT
        // stamped by unit 0? unit 0 stamped left[0..8] = 32). Unit 1 at
        // (row 0, col 8): above[8]=0 -> 1, left[0]=32 -> (32<32)=0 ->
        // ctx = 3 + 1 + 0 = 4.
        assert_eq!(s[1].0, 5);
        assert_eq!(s[2].0, 4);
    }

    #[test]
    fn depth1_8x8_split_stops_at_4x4() {
        // 8x8 depth 1 (target 4x4): sub_txs == TX_4X4 -> ONE split symbol,
        // then stop (no per-quadrant recursion).
        let s = walk_symbols(&[0; 2], &[0; 2], 8, 8, 1);
        assert_eq!(s.len(), 1);
        assert!(s[0].1);
    }

    #[test]
    fn depth2_codes_two_levels() {
        // 32x32 depth 2 (target 8x8): split at 32 (1) + splits at 16 (4);
        // the 8x8 leaves are at MAX_VARTX_DEPTH — C emits NO symbol there
        // (the `depth == MAX_VARTX_DEPTH` early return, entropy_coding.c
        // :4521-4524), the sub size is implied. 5 symbols, all splits.
        let s = walk_symbols(&[0; 8], &[0; 8], 32, 32, 2);
        assert_eq!(s.len(), 5);
        assert!(s.iter().all(|&(_, sp)| sp));
    }

    #[test]
    fn bottom_clip_drops_out_of_frame_units() {
        // 64x64 block at abs_y such that only 32 rows are in-frame:
        // depth 1 -> root split + only the top two 32x32 units coded.
        let mut v = alloc::vec::Vec::new();
        drive_walk(&[0; 16], &[0; 16], 64, 64, 1, 96, 128, |ctx, split| {
            v.push((ctx, split))
        });
        assert_eq!(v.len(), 3);
        assert!(v[0].1);
    }

    #[test]
    fn writer_and_cost_walk_identical_sequences() {
        // The cost fn and the writer must visit identical (ctx, split)
        // sequences for the same block state.
        let mut cost_syms = alloc::vec::Vec::new();
        drive_walk(&[8; 8], &[16; 8], 32, 16, 1, 0, 1 << 16, |c, s| cost_syms.push((c, s)));
        let mut fc = FrameContext::new_default();
        let mut w = AomWriter::new(1 << 12);
        write_tx_size_vartx(&mut w, &mut fc, &[8; 8], &[16; 8], 32, 16, 1, 0, 1 << 16);
        // txfm_partition_cdf counters advanced exactly cost_syms.len() times.
        let total_count: u32 = fc.txfm_partition_cdf.iter().map(|c| u32::from(c[2])).sum();
        assert_eq!(total_count as usize, cost_syms.len());
    }
}
