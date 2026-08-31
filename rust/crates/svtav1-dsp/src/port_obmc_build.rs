//! Building each OBMC neighbour's prediction, and the OBMC candidate's entry.
//!
//! Ported from `Source/Lib/Codec/enc_inter_prediction.c` (SVT-AV1 v4.2.0):
//! `get_single_prediction_for_obmc_luma` (:958),
//! `get_single_prediction_for_obmc_chroma` (:1018) and their `_hbd` twins
//! (:791, :853), `build_prediction_by_above_pred` (:1120),
//! `build_prediction_by_left_pred` (:1228),
//! `build_prediction_by_above_preds` (:1335),
//! `build_prediction_by_left_preds` (:1380),
//! `av1_inter_prediction_obmc` (:2925) and
//! `svt_aom_precompute_obmc_data` (:1816).
//!
//! The OBMC prediction is the NEIGHBOUR's motion applied to the current block,
//! so all of this is geometry: which neighbour, over what extent, into which
//! scratch buffer, with which edges installed.
//!
//! # Evidence
//!
//! TIER 4 throughout. Every one of these takes a `PictureControlSet`, a
//! `ModeDecisionContext`, a `MacroBlockD` and `EbPictureBufferDesc` planes —
//! not synthesisable in a shim. What is ported is the geometry and the buffer
//! layout, which is where the mistakes live; the MC underneath is
//! [`crate::port_inter_predictor`] (tier 1) and the blend is
//! [`crate::port_obmc_pred`].

use crate::port_obmc_data::{MI_SIZE, block_size_high, block_size_wide, skip_u4x4_pred_in_obmc};
use crate::port_obmc_pred::PredEdges;
use svtav1_types::block::BlockSize;

/// `MI_SIZE_LOG2`.
pub const MI_SIZE_LOG2: u32 = 2;

/// `ROUND_UV(x)` — round down to an even 4-sample chroma pair.
#[inline]
pub fn round_uv(x: u32) -> u32 {
    (x >> 3) << 3
}

/// The per-plane geometry `build_prediction_by_above_pred` derives for one
/// neighbour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NbPredGeom {
    /// Plane index (0 = Y).
    pub plane: usize,
    /// `bw` — the neighbour's width in this plane's samples.
    pub bw: usize,
    /// `bh`.
    pub bh: usize,
    /// `ctxt->dst_origin_x`.
    pub dst_origin_x: usize,
    /// `ctxt->dst_origin_y`.
    pub dst_origin_y: usize,
    /// `mi_x` — the SOURCE x in luma samples.
    pub mi_x: i32,
    /// `mi_y`.
    pub mi_y: i32,
}

/// The plane range a `component_mask` selects in the BUILD path.
///
/// TRAP: this is NOT the same range as the BLEND path's. `build_prediction_by_*_pred`
/// uses `end_plane = chroma ? 2 : 1` — it builds planes 0 and 1 only and lets
/// the Cb prediction stand in for Cr — while `build_obmc_inter_pred_*` uses
/// `end_plane = chroma ? 3 : 1` and blends all three. Copying one loop's bound
/// into the other silently drops or duplicates a plane.
pub fn build_plane_range(component_mask: u32) -> (usize, usize) {
    let start = if component_mask & crate::port_obmc_pred::COMPONENT_LUMA != 0 {
        0
    } else {
        1
    };
    let end = if component_mask & crate::port_obmc_pred::COMPONENT_CHROMA != 0 {
        2
    } else {
        1
    };
    (start, end)
}

/// `build_prediction_by_above_pred` (enc_inter_prediction.c:1120), geometry
/// half: what each plane's prediction covers and where it lands.
///
/// The height is `clamp(block_size_high[bsize] >> (ss_y + 1), 4,
/// block_size_high[BLOCK_64X64] >> (ss_y + 1))` — HALF the block height,
/// floored at 4 and capped at 32 (luma), which is why the caller also shrinks
/// `mb_to_bottom_edge` by the same amount.
pub fn build_prediction_by_above_pred_geom(
    bsize: BlockSize,
    mi_row: i32,
    mi_col: i32,
    rel_mi_col: usize,
    above_mi_width: usize,
    ss_x: usize,
    ss_y: usize,
    component_mask: u32,
) -> alloc::vec::Vec<NbPredGeom> {
    let mut out = alloc::vec::Vec::new();
    let above_mi_col = mi_col + rel_mi_col as i32;
    let (start_plane, end_plane) = build_plane_range(component_mask);
    for plane in start_plane..end_plane {
        let sx = if plane > 0 { ss_x } else { 0 };
        let sy = if plane > 0 { ss_y } else { 0 };
        let bw = (above_mi_width * MI_SIZE) >> sx;
        let bh = (block_size_high(bsize) >> (sy + 1)).clamp(4, 64 >> (sy + 1));
        if skip_u4x4_pred_in_obmc(bsize, 0, sx, sy) != 0 {
            continue;
        }
        out.push(NbPredGeom {
            plane,
            bw,
            bh,
            dst_origin_x: rel_mi_col << MI_SIZE_LOG2,
            dst_origin_y: 0,
            mi_x: above_mi_col << MI_SIZE_LOG2,
            mi_y: mi_row << MI_SIZE_LOG2,
        });
    }
    out
}

/// `build_prediction_by_left_pred` (enc_inter_prediction.c:1228), geometry
/// half — the mirror: the WIDTH is halved and the destination offset is on the
/// row axis.
pub fn build_prediction_by_left_pred_geom(
    bsize: BlockSize,
    mi_row: i32,
    mi_col: i32,
    rel_mi_row: usize,
    left_mi_height: usize,
    ss_x: usize,
    ss_y: usize,
    component_mask: u32,
) -> alloc::vec::Vec<NbPredGeom> {
    let mut out = alloc::vec::Vec::new();
    let left_mi_row = mi_row + rel_mi_row as i32;
    let (start_plane, end_plane) = build_plane_range(component_mask);
    for plane in start_plane..end_plane {
        let sx = if plane > 0 { ss_x } else { 0 };
        let sy = if plane > 0 { ss_y } else { 0 };
        let bw = (block_size_wide(bsize) >> (sx + 1)).clamp(4, 64 >> (sx + 1));
        let bh = (left_mi_height * MI_SIZE) >> sy;
        if skip_u4x4_pred_in_obmc(bsize, 1, sx, sy) != 0 {
            continue;
        }
        out.push(NbPredGeom {
            plane,
            bw,
            bh,
            dst_origin_x: 0,
            dst_origin_y: rel_mi_row << MI_SIZE_LOG2,
            mi_x: mi_col << MI_SIZE_LOG2,
            mi_y: left_mi_row << MI_SIZE_LOG2,
        });
    }
    out
}

/// `build_prediction_by_above_preds` (enc_inter_prediction.c:1335), the
/// edge-adjust half.
///
/// Returns the temporarily-widened `mb_to_bottom_edge` and the value to
/// restore afterwards. The OBMC prediction block is HALF the original block's
/// height, capped at 32, so the bottom edge is pushed out by the difference —
/// and pulled back at the end. Forgetting the restore leaks the widened edge
/// into the next block's MV clamp.
pub fn above_preds_edge_adjust(n4_h: i32) -> i32 {
    let this_height = n4_h * MI_SIZE as i32;
    let pred_height = (this_height / 2).min(32);
    (this_height - pred_height) * 8
}

/// `build_prediction_by_left_preds` (enc_inter_prediction.c:1380) — the
/// WIDTH-based mirror, applied to `mb_to_right_edge`.
pub fn left_preds_edge_adjust(n4_w: i32) -> i32 {
    let this_width = n4_w * MI_SIZE as i32;
    let pred_width = (this_width / 2).min(32);
    (this_width - pred_width) * 8
}

/// The edges `build_prediction_by_above_preds` restores when it returns:
/// `mb_to_left_edge = -((mi_col * MI_SIZE) * 8)`, `mb_to_right_edge` back to
/// the saved far edge, and the bottom edge un-widened.
pub fn restore_edges_after_above_preds(
    edges: &mut PredEdges,
    mi_col: i32,
    mb_to_far_edge: i32,
    adjust: i32,
) {
    edges.to_left = -((mi_col * MI_SIZE as i32) * 8);
    edges.to_right = mb_to_far_edge;
    edges.to_bottom -= adjust;
}

/// The left twin: `mb_to_top_edge = -((mi_row * MI_SIZE) * 8)`, the right edge
/// un-widened, and the bottom edge back to the saved far edge.
pub fn restore_edges_after_left_preds(
    edges: &mut PredEdges,
    mi_row: i32,
    mb_to_far_edge: i32,
    adjust: i32,
) {
    edges.to_top = -((mi_row * MI_SIZE as i32) * 8);
    edges.to_right -= adjust;
    edges.to_bottom = mb_to_far_edge;
}

/// Byte offsets of the three planes inside one OBMC scratch buffer.
///
/// `av1_inter_prediction_obmc` (:2953) and `svt_aom_precompute_obmc_data`
/// (:1816) both lay out `obmc_buff_{0,1}` as three consecutive `bwidth *
/// bheight` planes, shifted left by `is16bit`. NOTE the two disagree on which
/// dimensions: the predictor uses `block_size_{wide,high}[bsize]` while
/// precompute uses `ctx->blk_geom->b{width,height}` — the same numbers for a
/// square-partition block, and the reason the buffers interoperate.
pub fn obmc_buffer_plane_offsets(bwidth: usize, bheight: usize, is16bit: bool) -> [usize; 3] {
    let n = bwidth * bheight;
    let s = usize::from(is16bit);
    [0, n << s, (n * 2) << s]
}

/// `av1_inter_prediction_obmc`'s final destination offsets (:2996).
///
/// Chroma uses `ROUND_UV(dst_origin) / 2`, NOT `dst_origin / 2` — the origin is
/// first floored to a multiple of 8 luma samples. For a 4xN sub-block those
/// differ, and the wrong one lands the blend a chroma sample off.
pub fn obmc_final_dst_offsets(
    dst_origin_x: u32,
    dst_origin_y: u32,
    y_stride: usize,
    u_stride: usize,
    v_stride: usize,
    is16bit: bool,
) -> [usize; 3] {
    let s = usize::from(is16bit);
    let y = (dst_origin_x as usize + dst_origin_y as usize * y_stride) << s;
    let cx = (round_uv(dst_origin_x) / 2) as usize;
    let cy = (round_uv(dst_origin_y) / 2) as usize;
    [y, (cx + cy * u_stride) << s, (cx + cy * v_stride) << s]
}

/// The chroma origin `get_single_prediction_for_obmc_chroma` predicts from
/// (:1046): `ROUND_UV(pu_origin) >> ss`.
pub fn obmc_chroma_pu_origin(
    pu_origin_x: u32,
    pu_origin_y: u32,
    ss_x: usize,
    ss_y: usize,
) -> (u32, u32) {
    (round_uv(pu_origin_x) >> ss_x, round_uv(pu_origin_y) >> ss_y)
}

/// The CONV_BUF stride the two per-neighbour MC helpers use:
/// `scs->sb_size` for luma and `scs->sb_size >> ss_x` for chroma
/// (:978, :1032) — NOT the 64/128 the PD0 driver uses.
pub fn obmc_conv_buf_stride(sb_size: usize, plane: usize, ss_x: usize) -> usize {
    if plane == 0 { sb_size } else { sb_size >> ss_x }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The BUILD loop stops at plane 2 and the BLEND loop at plane 3 — the
    /// single most copy-pasteable mistake in this file.
    #[test]
    fn build_and_blend_plane_ranges_differ() {
        let full = crate::port_obmc_pred::COMPONENT_LUMA | crate::port_obmc_pred::COMPONENT_CHROMA;
        assert_eq!(build_plane_range(full), (0, 2));
        // The blend path's range, for contrast.
        let blend_end = 3;
        assert_ne!(build_plane_range(full).1, blend_end);
        // Luma-only agrees in both.
        assert_eq!(
            build_plane_range(crate::port_obmc_pred::COMPONENT_LUMA),
            (0, 1)
        );
    }

    /// The above pass halves the HEIGHT (floor 4, cap 32) and the left pass
    /// halves the WIDTH.
    #[test]
    fn neighbour_extents_halve_the_matching_axis() {
        // BLOCK_64X64 luma: above bh = 32, left bw = 32.
        let a = build_prediction_by_above_pred_geom(BlockSize::Block64x64, 0, 0, 0, 16, 1, 1, 1);
        assert_eq!(a[0].bh, 32);
        assert_eq!(a[0].bw, 64);
        let l = build_prediction_by_left_pred_geom(BlockSize::Block64x64, 0, 0, 0, 16, 1, 1, 1);
        assert_eq!(l[0].bw, 32);
        assert_eq!(l[0].bh, 64);
        // BLOCK_8X8 luma: half is 4, which is the floor, not below it.
        let a = build_prediction_by_above_pred_geom(BlockSize::Block8x8, 0, 0, 0, 2, 1, 1, 1);
        assert_eq!(a[0].bh, 4);
        // BLOCK_128X128 luma: half is 64 but the cap is 32.
        let a = build_prediction_by_above_pred_geom(BlockSize::Block128x128, 0, 0, 0, 32, 1, 1, 1);
        assert_eq!(a[0].bh, 32);
    }

    /// The edge widen/restore pair must cancel exactly.
    #[test]
    fn edge_adjust_is_reversible() {
        let mut e = PredEdges {
            to_left: -100,
            to_right: 200,
            to_top: -300,
            to_bottom: 400,
        };
        let before = e;
        let adjust = above_preds_edge_adjust(16);
        e.to_bottom += adjust;
        assert_ne!(e.to_bottom, before.to_bottom);
        restore_edges_after_above_preds(&mut e, 0, before.to_right, adjust);
        assert_eq!(e.to_bottom, before.to_bottom);
        assert_eq!(e.to_right, before.to_right);
        assert_eq!(e.to_left, 0);
        // 128-wide: half is 64 but the cap is 32, so the adjust is bigger.
        assert_eq!(above_preds_edge_adjust(32), (128 - 32) * 8);
        assert_eq!(left_preds_edge_adjust(32), (128 - 32) * 8);
        // 16x16: half is 8, under the cap.
        assert_eq!(above_preds_edge_adjust(4), (16 - 8) * 8);
    }

    /// Chroma destinations go through ROUND_UV, which floors to a multiple of
    /// 8 luma samples BEFORE halving.
    #[test]
    fn chroma_origins_round_before_halving() {
        assert_eq!(round_uv(0), 0);
        assert_eq!(round_uv(4), 0);
        assert_eq!(round_uv(8), 8);
        assert_eq!(round_uv(12), 8);
        let [y, u, v] = obmc_final_dst_offsets(4, 4, 64, 32, 32, false);
        assert_eq!(y, 4 + 4 * 64);
        // ROUND_UV(4) / 2 == 0, not 2.
        assert_eq!(u, 0);
        assert_eq!(v, 0);
        assert_eq!(obmc_chroma_pu_origin(12, 12, 1, 1), (4, 4));
    }

    /// The three OBMC scratch planes are consecutive and doubled at 10 bits.
    #[test]
    fn obmc_scratch_layout() {
        assert_eq!(obmc_buffer_plane_offsets(16, 16, false), [0, 256, 512]);
        assert_eq!(obmc_buffer_plane_offsets(16, 16, true), [0, 512, 1024]);
    }

    /// The per-neighbour MC uses the SB size as its CONV_BUF stride, not the
    /// PD0 driver's 64/128 rule.
    #[test]
    fn obmc_conv_buf_stride_is_the_sb_size() {
        assert_eq!(obmc_conv_buf_stride(64, 0, 1), 64);
        assert_eq!(obmc_conv_buf_stride(64, 1, 1), 32);
        assert_eq!(obmc_conv_buf_stride(128, 0, 1), 128);
    }
}
