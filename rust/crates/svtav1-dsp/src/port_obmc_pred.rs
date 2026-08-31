//! The OBMC neighbour walk and the final blend into the prediction.
//!
//! Ported from `Source/Lib/Codec/enc_inter_prediction.c` (SVT-AV1 v4.2.0):
//! `foreach_overlappable_nb_above` (:708), `foreach_overlappable_nb_left`
//! (:741), `av1_setup_build_prediction_by_above_pred` (:769),
//! `av1_setup_build_prediction_by_left_pred` (:781),
//! `build_obmc_inter_pred_above` (:1438), `build_obmc_inter_pred_left` (:1480)
//! and `av1_build_obmc_inter_prediction` (:1525); plus
//! `is_neighbor_overlappable` (inter_prediction.h:271).
//!
//! `CONFIG_ENABLE_OBMC` is 1 in mainline — see
//! [`crate::port_obmc_data`]'s module doc for the two-definition check.
//!
//! # Evidence
//!
//! Every function here is `static` and reaches the `MacroBlockD` mi grid and
//! the `Av1Common` frame dimensions, so a shim cannot synthesise their inputs
//! without building most of the encoder: TIER 4, hand-traced against the C
//! source. What they *call* is gated: the blends are
//! [`crate::obmc::obmc_blend_above`] / `obmc_blend_left`
//! (`c_parity_obmc.rs`), [`crate::port_masked_compound::highbd_blend_a64_hmask_16bit`]
//! (`c_parity_port_masked_compound.rs`) and
//! [`crate::port_obmc_data::skip_u4x4_pred_in_obmc`]
//! (`c_parity_port_obmc_data.rs`), all tier 1.
//!
//! # The 4-wide pairing rule, which is the part that is easy to get wrong
//!
//! Both walks contain this: when the neighbour's `mi_step` comes out as 1 (a
//! 4-wide or 4-tall block), the loop variable is rounded DOWN to even, the
//! neighbour pointer is advanced to the SECOND block of the pair, and
//! `mi_step` becomes 2. C's comment says why: a width-4 block is half of a
//! pair whose chroma lives in the second half. Dropping any one of those three
//! steps changes which neighbour's motion the OBMC prediction uses.

use crate::obmc::{obmc_blend_above, obmc_blend_left};
use crate::port_masked_compound::highbd_blend_a64_hmask_16bit;
use crate::port_obmc_data::{MI_SIZE, block_size_high, block_size_wide, skip_u4x4_pred_in_obmc};
use alloc::vec::Vec;
use svtav1_types::block::BlockSize;

/// `max_neighbor_obmc` (enc_inter_prediction.c:703), indexed by
/// `mi_size_{wide,high}_log2[bsize]`.
pub const MAX_NEIGHBOR_OBMC: [usize; 6] = [0, 1, 2, 3, 4, 4];

/// `mi_size_wide_log2[bsize]`.
pub const MI_SIZE_WIDE_LOG2: [usize; BlockSize::SIZES_ALL] = [
    0, 0, 1, 1, 1, 2, 2, 2, 3, 3, 3, 4, 4, 4, 5, 5, 0, 2, 1, 3, 2, 4,
];
/// `mi_size_high_log2[bsize]`.
pub const MI_SIZE_HIGH_LOG2: [usize; BlockSize::SIZES_ALL] = [
    0, 1, 0, 1, 2, 1, 2, 3, 2, 3, 4, 3, 4, 5, 4, 5, 2, 0, 3, 1, 4, 2,
];
/// `mi_size_wide[bsize]` — width in 4x4 units.
pub const MI_SIZE_WIDE: [usize; BlockSize::SIZES_ALL] = [
    1, 1, 2, 2, 2, 4, 4, 4, 8, 8, 8, 16, 16, 16, 32, 32, 1, 4, 2, 8, 4, 16,
];
/// `mi_size_high[bsize]`.
pub const MI_SIZE_HIGH: [usize; BlockSize::SIZES_ALL] = [
    1, 2, 1, 2, 4, 2, 4, 8, 4, 8, 16, 8, 16, 32, 16, 32, 4, 1, 8, 2, 16, 4,
];
/// `mi_size_wide[BLOCK_64X64]` / `mi_size_high[BLOCK_64X64]` — the per-step cap.
const MI_SIZE_64X64: usize = 16;

/// One entry of the mi grid, reduced to what the OBMC walk reads.
#[derive(Debug, Clone, Copy)]
pub struct NbMi {
    /// `mbmi->bsize`.
    pub bsize: BlockSize,
    /// `is_neighbor_overlappable(mbmi)` — `ref_frame[0] > INTRA_FRAME`.
    pub overlappable: bool,
}

/// A visited neighbour: its offset from the current block and the extent the
/// visitor is handed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisitedNb {
    /// `above_mi_col - mi_col`, or `left_mi_row - mi_row`.
    pub rel_mi: usize,
    /// `AOMMIN(xd->n4_w, mi_step)`, or the `n4_h` twin.
    pub nb_mi_size: usize,
}

/// `foreach_overlappable_nb_above` (enc_inter_prediction.c:708), as an
/// enumeration rather than a callback.
///
/// `row` is the mi row ABOVE the block, indexed from `mi_col` — i.e.
/// `row[k]` is `prev_row_mi[mi_col + k]`. `mi_cols` is `cm->mi_cols`.
pub fn foreach_overlappable_nb_above(
    up_available: bool,
    row: &[NbMi],
    mi_col: usize,
    n4_w: usize,
    mi_cols: usize,
    nb_max: usize,
) -> Vec<VisitedNb> {
    let mut out = Vec::new();
    if !up_available {
        return out;
    }
    let end_col = (mi_col + n4_w).min(mi_cols);
    let mut nb_count = 0usize;
    let mut above_mi_col = mi_col;
    while above_mi_col < end_col && nb_count < nb_max {
        let mut idx = above_mi_col - mi_col;
        let mut mi_step = MI_SIZE_WIDE[row[idx].bsize as usize].min(MI_SIZE_64X64);
        // The 4-wide pairing rule: round the column down to even, look at the
        // SECOND block of the pair, and step over both.
        if mi_step == 1 {
            above_mi_col &= !1usize;
            idx = above_mi_col - mi_col + 1;
            mi_step = 2;
        }
        if row[idx].overlappable {
            nb_count += 1;
            out.push(VisitedNb {
                rel_mi: above_mi_col - mi_col,
                nb_mi_size: n4_w.min(mi_step),
            });
        }
        above_mi_col += mi_step;
    }
    out
}

/// `foreach_overlappable_nb_left` (enc_inter_prediction.c:741).
///
/// `col[k]` is `prev_col_mi[(mi_row + k) * mi_stride]`.
pub fn foreach_overlappable_nb_left(
    left_available: bool,
    col: &[NbMi],
    mi_row: usize,
    n4_h: usize,
    mi_rows: usize,
    nb_max: usize,
) -> Vec<VisitedNb> {
    let mut out = Vec::new();
    if !left_available {
        return out;
    }
    let end_row = (mi_row + n4_h).min(mi_rows);
    let mut nb_count = 0usize;
    let mut left_mi_row = mi_row;
    while left_mi_row < end_row && nb_count < nb_max {
        let mut idx = left_mi_row - mi_row;
        let mut mi_step = MI_SIZE_HIGH[col[idx].bsize as usize].min(MI_SIZE_64X64);
        if mi_step == 1 {
            left_mi_row &= !1usize;
            idx = left_mi_row - mi_row + 1;
            mi_step = 2;
        }
        if col[idx].overlappable {
            nb_count += 1;
            out.push(VisitedNb {
                rel_mi: left_mi_row - mi_row,
                nb_mi_size: n4_h.min(mi_step),
            });
        }
        left_mi_row += mi_step;
    }
    out
}

/// The `MacroBlockD` edges `av1_setup_build_prediction_by_*_pred` rewrite
/// before predicting from a neighbour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PredEdges {
    /// `xd->mb_to_left_edge`.
    pub to_left: i32,
    /// `xd->mb_to_right_edge`.
    pub to_right: i32,
    /// `xd->mb_to_top_edge`.
    pub to_top: i32,
    /// `xd->mb_to_bottom_edge`.
    pub to_bottom: i32,
}

/// `av1_setup_build_prediction_by_above_pred` (enc_inter_prediction.c:769) —
/// the edge half. The `ctxt->mv` / `ctxt->ref_pic_list0` half is the
/// NEIGHBOUR's motion and reference, which the caller supplies.
pub fn setup_build_prediction_by_above_pred(
    edges: &mut PredEdges,
    mi_col: i32,
    rel_mi_col: i32,
    above_mi_width: i32,
    n4_w: i32,
    mb_to_far_edge: i32,
) {
    let above_mi_col = mi_col + rel_mi_col;
    edges.to_left = 8 * MI_SIZE as i32 * (-above_mi_col);
    edges.to_right = mb_to_far_edge + (n4_w - rel_mi_col - above_mi_width) * MI_SIZE as i32 * 8;
}

/// `av1_setup_build_prediction_by_left_pred` (enc_inter_prediction.c:781).
pub fn setup_build_prediction_by_left_pred(
    edges: &mut PredEdges,
    mi_row: i32,
    rel_mi_row: i32,
    left_mi_height: i32,
    n4_h: i32,
    mb_to_far_edge: i32,
) {
    let left_mi_row = mi_row + rel_mi_row;
    edges.to_top = 8 * MI_SIZE as i32 * (-left_mi_row);
    edges.to_bottom = mb_to_far_edge + (n4_h - rel_mi_row - left_mi_height) * MI_SIZE as i32 * 8;
}

/// `PICTURE_BUFFER_DESC_LUMA_MASK`.
pub const COMPONENT_LUMA: u32 = 1;
/// `PICTURE_BUFFER_DESC_CHROMA_MASK`.
pub const COMPONENT_CHROMA: u32 = 6;

/// The `plane` range a `component_mask` selects: luma only, chroma only, or
/// both — `[start, end)`.
fn plane_range(component_mask: u32) -> (usize, usize) {
    let start = if component_mask & COMPONENT_LUMA != 0 {
        0
    } else {
        1
    };
    let end = if component_mask & COMPONENT_CHROMA != 0 {
        3
    } else {
        1
    };
    (start, end)
}

/// One plane's destination and neighbour-prediction buffers, 8-bit.
pub struct ObmcPlanes<'a> {
    /// `final_dst_ptr_*` per plane.
    pub dst: [&'a mut [u8]; 3],
    /// `final_dst_stride_*` per plane.
    pub dst_stride: [usize; 3],
}

/// The neighbour predictions the OBMC blend reads, 8-bit.
pub struct ObmcAdjacent<'a> {
    /// `ctxt->adjacent[plane]`.
    pub plane: [&'a [u8]; 3],
    /// `ctxt->adjacent_stride[plane]`.
    pub stride: [usize; 3],
}

/// `build_obmc_inter_pred_above` (enc_inter_prediction.c:1438), 8-bit.
///
/// The overlap is `min(block_size_high[bsize], 64) >> 1` — from the block's
/// HEIGHT even though this is the above pass, and it is the blend depth in
/// luma samples before the plane's `subsampling_y` shift.
pub fn build_obmc_inter_pred_above(
    planes: &mut ObmcPlanes<'_>,
    adjacent: &ObmcAdjacent<'_>,
    bsize: BlockSize,
    rel_mi_col: usize,
    above_mi_width: usize,
    component_mask: u32,
) {
    let overlap = block_size_high(bsize).min(64) >> 1;
    let (start_plane, end_plane) = plane_range(component_mask);
    for plane in start_plane..end_plane {
        let ss = usize::from(plane > 0);
        let bw = (above_mi_width * MI_SIZE) >> ss;
        let bh = overlap >> ss;
        let plane_col = (rel_mi_col * MI_SIZE) >> ss;
        if skip_u4x4_pred_in_obmc(bsize, 0, ss, ss) != 0 {
            continue;
        }
        let dst_stride = planes.dst_stride[plane];
        let tmp_stride = adjacent.stride[plane];
        obmc_blend_above(
            &mut planes.dst[plane][plane_col..],
            dst_stride,
            &adjacent.plane[plane][plane_col..],
            tmp_stride,
            bw,
            bh,
            overlap >> ss,
        );
    }
}

/// `build_obmc_inter_pred_left` (enc_inter_prediction.c:1480), 8-bit.
///
/// The overlap is `min(block_size_wide[bsize], 64) >> 1` — from the block's
/// WIDTH, the mirror of the above pass.
pub fn build_obmc_inter_pred_left(
    planes: &mut ObmcPlanes<'_>,
    adjacent: &ObmcAdjacent<'_>,
    bsize: BlockSize,
    rel_mi_row: usize,
    left_mi_height: usize,
    component_mask: u32,
) {
    let overlap = block_size_wide(bsize).min(64) >> 1;
    let (start_plane, end_plane) = plane_range(component_mask);
    for plane in start_plane..end_plane {
        let ss = usize::from(plane > 0);
        let bw = overlap >> ss;
        let bh = (left_mi_height * MI_SIZE) >> ss;
        let plane_row = (rel_mi_row * MI_SIZE) >> ss;
        if skip_u4x4_pred_in_obmc(bsize, 1, ss, ss) != 0 {
            continue;
        }
        let dst_stride = planes.dst_stride[plane];
        let tmp_stride = adjacent.stride[plane];
        obmc_blend_left(
            &mut planes.dst[plane][plane_row * dst_stride..],
            dst_stride,
            &adjacent.plane[plane][plane_row * tmp_stride..],
            tmp_stride,
            bw,
            bh,
            bw,
        );
    }
}

/// `build_obmc_inter_pred_left`'s 10-bit arm — the sole caller of
/// `svt_aom_highbd_blend_a64_hmask_16bit`.
pub fn build_obmc_inter_pred_left_hbd(
    dst: &mut [u16],
    dst_stride: usize,
    tmp: &[u16],
    tmp_stride: usize,
    bsize: BlockSize,
    rel_mi_row: usize,
    left_mi_height: usize,
    ss: usize,
    mask: &[u8],
) {
    let overlap = block_size_wide(bsize).min(64) >> 1;
    let bw = overlap >> ss;
    let bh = (left_mi_height * MI_SIZE) >> ss;
    if skip_u4x4_pred_in_obmc(bsize, 1, ss, ss) != 0 {
        return;
    }
    let plane_row = (rel_mi_row * MI_SIZE) >> ss;
    // C blends dst with itself as src0 (`dst, dst_stride, dst, dst_stride,
    // tmp, ...`), so the destination is both an input and the output.
    let src0: alloc::vec::Vec<u16> = (0..bh)
        .flat_map(|r| {
            let base = plane_row * dst_stride + r * dst_stride;
            dst[base..base + bw].to_vec()
        })
        .collect();
    let mut out = alloc::vec![0u16; bw * bh];
    let src1: alloc::vec::Vec<u16> = (0..bh)
        .flat_map(|r| {
            let base = plane_row * tmp_stride + r * tmp_stride;
            tmp[base..base + bw].to_vec()
        })
        .collect();
    highbd_blend_a64_hmask_16bit(&mut out, bw, &src0, bw, &src1, bw, mask, bw, bh);
    for r in 0..bh {
        let base = plane_row * dst_stride + r * dst_stride;
        dst[base..base + bw].copy_from_slice(&out[r * bw..r * bw + bw]);
    }
}

/// `av1_build_obmc_inter_prediction` (enc_inter_prediction.c:1525), 8-bit:
/// the above pass over its neighbours, then the left pass over its own.
///
/// The two neighbour lists come from [`foreach_overlappable_nb_above`] /
/// `_left`; the `nb_max` C passes is
/// `max_neighbor_obmc[mi_size_wide_log2[bsize]]` for the above walk and the
/// `high` twin for the left one — note the ABOVE walk is bounded by the
/// block's WIDTH log2 and the LEFT walk by its HEIGHT log2.
pub fn build_obmc_inter_prediction(
    planes: &mut ObmcPlanes<'_>,
    above: &ObmcAdjacent<'_>,
    above_nbs: &[VisitedNb],
    left: &ObmcAdjacent<'_>,
    left_nbs: &[VisitedNb],
    bsize: BlockSize,
    component_mask: u32,
) {
    for nb in above_nbs {
        build_obmc_inter_pred_above(
            planes,
            above,
            bsize,
            nb.rel_mi,
            nb.nb_mi_size,
            component_mask,
        );
    }
    for nb in left_nbs {
        build_obmc_inter_pred_left(
            planes,
            left,
            bsize,
            nb.rel_mi,
            nb.nb_mi_size,
            component_mask,
        );
    }
}

/// The `nb_max` bound for the two walks.
pub fn nb_max_above(bsize: BlockSize) -> usize {
    MAX_NEIGHBOR_OBMC[MI_SIZE_WIDE_LOG2[bsize as usize]]
}

/// The `nb_max` bound for the left walk — HEIGHT log2, not width.
pub fn nb_max_left(bsize: BlockSize) -> usize {
    MAX_NEIGHBOR_OBMC[MI_SIZE_HIGH_LOG2[bsize as usize]]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nb(bsize: BlockSize, overlappable: bool) -> NbMi {
        NbMi {
            bsize,
            overlappable,
        }
    }

    /// The 4-wide pairing rule: a row of 4x4 neighbours is visited in PAIRS,
    /// at even offsets, and the overlappability of the SECOND block of each
    /// pair is what decides.
    #[test]
    fn four_wide_neighbours_are_paired() {
        // Eight 4x4 mi units; only the odd ones are overlappable.
        let row: Vec<NbMi> = (0..8)
            .map(|k| nb(BlockSize::Block4x4, k % 2 == 1))
            .collect();
        let got = foreach_overlappable_nb_above(true, &row, 0, 8, 64, 4);
        assert_eq!(
            got,
            alloc::vec![
                VisitedNb {
                    rel_mi: 0,
                    nb_mi_size: 2
                },
                VisitedNb {
                    rel_mi: 2,
                    nb_mi_size: 2
                },
                VisitedNb {
                    rel_mi: 4,
                    nb_mi_size: 2
                },
                VisitedNb {
                    rel_mi: 6,
                    nb_mi_size: 2
                },
            ]
        );
        // With only the EVEN ones overlappable, nothing is visited — the rule
        // reads the second of each pair.
        let row: Vec<NbMi> = (0..8)
            .map(|k| nb(BlockSize::Block4x4, k % 2 == 0))
            .collect();
        assert!(foreach_overlappable_nb_above(true, &row, 0, 8, 64, 4).is_empty());
    }

    /// `nb_max` is bounded by WIDTH log2 above and HEIGHT log2 left — the two
    /// differ on every non-square block.
    #[test]
    fn nb_max_uses_the_matching_axis() {
        // BLOCK_16X64 = 20: wide log2 2 -> 2, high log2 4 -> 4.
        assert_eq!(nb_max_above(BlockSize::Block16x64), 2);
        assert_eq!(nb_max_left(BlockSize::Block16x64), 4);
        // BLOCK_64X16 = 21 is the mirror.
        assert_eq!(nb_max_above(BlockSize::Block64x16), 4);
        assert_eq!(nb_max_left(BlockSize::Block64x16), 2);
    }

    /// The walk stops at `nb_max` and at `mi_cols`, whichever comes first.
    #[test]
    fn walk_respects_both_bounds() {
        let row: Vec<NbMi> = (0..16).map(|_| nb(BlockSize::Block8x8, true)).collect();
        // nb_max 2 stops after two neighbours even though eight fit.
        assert_eq!(
            foreach_overlappable_nb_above(true, &row, 0, 16, 64, 2).len(),
            2
        );
        // mi_cols 4 stops after two 8x8 (2 mi each) neighbours.
        assert_eq!(
            foreach_overlappable_nb_above(true, &row, 0, 16, 4, 8).len(),
            2
        );
        // Not available -> nothing at all.
        assert!(foreach_overlappable_nb_above(false, &row, 0, 16, 64, 8).is_empty());
    }

    /// The edge rewrites use the NEIGHBOUR's absolute position.
    #[test]
    fn setup_edges_use_the_neighbours_position() {
        let mut e = PredEdges::default();
        setup_build_prediction_by_above_pred(&mut e, 4, 2, 2, 8, 1024);
        assert_eq!(e.to_left, 8 * 4 * -(4 + 2));
        assert_eq!(e.to_right, 1024 + (8 - 2 - 2) * 4 * 8);
        setup_build_prediction_by_left_pred(&mut e, 4, 2, 2, 8, 1024);
        assert_eq!(e.to_top, 8 * 4 * -(4 + 2));
        assert_eq!(e.to_bottom, 1024 + (8 - 2 - 2) * 4 * 8);
    }
}
