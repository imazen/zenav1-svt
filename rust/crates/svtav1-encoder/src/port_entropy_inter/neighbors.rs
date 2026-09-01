//! `set_mi_row_col` and the two block-extent helpers that read its output
//! (C `Source/Lib/Codec/entropy_coding.c:4681-4724`, `:4431-4451`).
//!
//! This is the function every inter context depends on and none of them
//! contains. `write_modes_b` calls it once per block, before any symbol, and
//! it establishes:
//!
//! * `up_available` / `left_available` — the TILE-relative availability the
//!   ref-count, reference-mode, comp-reference-type, intra/inter and
//!   switchable-interp contexts read;
//! * `above_mbmi` / `left_mbmi` — the neighbour POINTERS that
//!   `av1_get_skip_mode_context`, `svt_aom_get_comp_index_context_enc` and
//!   `svt_aom_get_comp_group_idx_context_enc` test instead. [`super`]'s
//!   `Neighbors` keeps the two knobs separate because C does; this is where
//!   they are both produced, and the only place their relationship is
//!   decided.
//! * `mb_to_{left,right,top,bottom}_edge` — eighth-pel distances to the
//!   FRAME edge, which [`max_block_wide`] / [`max_block_high`] turn into the
//!   clipped transform-block extents the coefficient walk iterates.
//! * `is_sec_rect` — the "second rectangle of a rectangular split" flag.
//!
//! # Shape
//!
//! C writes eleven fields into a `MacroBlockD` it is handed. This returns a
//! [`BlockGeometry`] value instead, and returns the two neighbours as mi-grid
//! INDICES (`Option<usize>`) rather than pointers: the caller owns the grid,
//! `None` is C's `NULL`, and an unavailable neighbour cannot be dereferenced
//! by construction. The edges stay `i32` because they are C's `int32_t` and
//! are DELIBERATELY negative past the frame edge — that sign is the gate
//! [`max_block_wide`] tests.
//!
//! # Evidence
//!
//! **Tier 1** (`docs/WORKING-ON-THIS.md` §4): `set_mi_row_col` is EXPORTED
//! (`nm -g` prints `T _set_mi_row_col`), so
//! `tests/c_parity_entropy_block.rs` drives the release archive's own copy
//! through the `entropy_block` shim and compares all eleven outputs over a
//! swept grid of positions, block shapes and tile origins. The shim recovers
//! the neighbour POINTERS as indices by filling the mi grid with
//! `(MbModeInfo*)(index + 1)` sentinels, which are never dereferenced.
//!
//! `max_block_wide` / `max_block_high` are `static INLINE` in
//! `entropy_coding.c`, which no shim compiles, so they are tier 4:
//! hand-derived vectors traced against the C source, built on the tier-1
//! edges above.

use svtav1_types::block::BlockSize;
use svtav1_types::tables::block::{BLOCK_SIZE_HIGH, BLOCK_SIZE_WIDE};

/// C `MI_SIZE` (definitions.h) — the mode-info unit, in luma pixels.
pub const MI_SIZE: i32 = 4;

/// C `MI_SIZE_LOG2`.
pub const MI_SIZE_LOG2: u32 = 2;

/// C `tx_size_wide_log2[0]` / `tx_size_high_log2[0]` — TX_4X4's log2 extent,
/// the unit [`max_block_wide`] reports in.
const TX_UNIT_LOG2: u32 = 2;

/// The `TileInfo` fields `set_mi_row_col` reads.
#[derive(Clone, Copy, Debug, Default)]
pub struct TileBounds {
    /// C `tile->mi_row_start`.
    pub mi_row_start: i32,
    /// C `tile->mi_col_start`.
    pub mi_col_start: i32,
}

/// The `MacroBlockD` fields `set_mi_row_col` writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockGeometry {
    /// C `xd->mb_to_top_edge` — eighth-pel, always <= 0.
    pub mb_to_top_edge: i32,
    /// C `xd->mb_to_bottom_edge` — eighth-pel, NEGATIVE when the block
    /// overhangs the frame.
    pub mb_to_bottom_edge: i32,
    /// C `xd->mb_to_left_edge`.
    pub mb_to_left_edge: i32,
    /// C `xd->mb_to_right_edge`.
    pub mb_to_right_edge: i32,
    /// C `xd->up_available` — `mi_row > tile->mi_row_start`. TILE-relative,
    /// not frame-relative: the first row of a tile has no above neighbour
    /// even in the middle of a frame.
    pub up_available: bool,
    /// C `xd->left_available`.
    pub left_available: bool,
    /// C `xd->above_mbmi`, as an index into the mi grid. `None` is C's
    /// `NULL`, which it stores exactly when `up_available` is false.
    pub above_mi: Option<usize>,
    /// C `xd->left_mbmi`, likewise.
    pub left_mi: Option<usize>,
    /// C `xd->n8_w` — block width in mi units.
    pub n8_w: u8,
    /// C `xd->n8_h`.
    pub n8_h: u8,
    /// C `xd->is_sec_rect`.
    pub is_sec_rect: bool,
    /// C `offset = mi_row * mi_stride + mi_col`, i.e. the index `xd->mi`
    /// points at. Returned because the caller needs it to reach the block's
    /// own `MbModeInfo`.
    pub mi_offset: usize,
}

/// C `set_mi_row_col` (entropy_coding.c:4681-4724, EXPORTED).
///
/// `bw`/`bh` are the block's mi-unit dimensions (C's `mi_size_wide[bsize]` /
/// `mi_size_high[bsize]`), NOT pixels. `mi_rows`/`mi_cols` are the FRAME's,
/// while the tile origin is the TILE's — that asymmetry is C's, and it is
/// what makes the edges frame-relative while availability is tile-relative.
#[allow(clippy::too_many_arguments)]
pub fn set_mi_row_col(
    tile: &TileBounds,
    mi_row: i32,
    bh: i32,
    mi_col: i32,
    bw: i32,
    mi_stride: i32,
    mi_rows: i32,
    mi_cols: i32,
) -> BlockGeometry {
    let mb_to_top_edge = -((mi_row * MI_SIZE) * 8);
    let mb_to_bottom_edge = ((mi_rows - bh - mi_row) * MI_SIZE) * 8;
    let mb_to_left_edge = -((mi_col * MI_SIZE) * 8);
    let mb_to_right_edge = ((mi_cols - bw - mi_col) * MI_SIZE) * 8;

    let up_available = mi_row > tile.mi_row_start;
    let left_available = mi_col > tile.mi_col_start;
    let offset = mi_row * mi_stride + mi_col;

    // C reads `xd->mi[-xd->mi_stride]` / `xd->mi[-1]` only inside the
    // availability test, so an unavailable neighbour is never indexed.
    let above_mi = up_available.then(|| (offset - mi_stride) as usize);
    let left_mi = left_available.then(|| (offset - 1) as usize);

    // C's two `if`s are sequential rather than `else if`, but `bw < bh` and
    // `bw > bh` are disjoint, so the three-way match is the same predicate.
    let is_sec_rect = match bw.cmp(&bh) {
        // "Only mark is_sec_rect for the LAST block": for PARTITION_VERT_4
        // that is (0, 0, 0, 1); for the other splits (0, 1).
        core::cmp::Ordering::Less => ((mi_col + bw) & (bh - 1)) == 0,
        core::cmp::Ordering::Greater => (mi_row & (bw - 1)) != 0,
        core::cmp::Ordering::Equal => false,
    };

    BlockGeometry {
        mb_to_top_edge,
        mb_to_bottom_edge,
        mb_to_left_edge,
        mb_to_right_edge,
        up_available,
        left_available,
        above_mi,
        left_mi,
        n8_w: bw as u8,
        n8_h: bh as u8,
        is_sec_rect,
        mi_offset: offset as usize,
    }
}

/// C `max_block_wide` (entropy_coding.c:4431) — the block's width in TX_4X4
/// units, CLIPPED to the frame's right edge.
///
/// C reaches for `gcc_right_shift(a, s)` because a plain `>>` on a negative
/// `int` is implementation-defined in C. Rust's `>>` on `i32` is defined to
/// be arithmetic, so the helper collapses to the shift; the function's
/// comment says as much rather than leaving the reader to wonder why the
/// port looks shorter.
///
/// `plane` shifts one extra bit for chroma (C `3 + !!plane`) — the 4:2:0
/// subsampling, not a general rule.
pub fn max_block_wide(mb_to_right_edge: i32, bsize: BlockSize, plane: usize) -> usize {
    let mut max_blocks_wide = i32::from(BLOCK_SIZE_WIDE[bsize.as_index()]);
    if mb_to_right_edge < 0 {
        max_blocks_wide += mb_to_right_edge >> (3 + u32::from(plane != 0));
    }
    (max_blocks_wide >> TX_UNIT_LOG2).max(0) as usize
}

/// C `max_block_high` (entropy_coding.c:4442) — the vertical twin.
pub fn max_block_high(mb_to_bottom_edge: i32, bsize: BlockSize, plane: usize) -> usize {
    let mut max_blocks_high = i32::from(BLOCK_SIZE_HIGH[bsize.as_index()]);
    if mb_to_bottom_edge < 0 {
        max_blocks_high += mb_to_bottom_edge >> (3 + u32::from(plane != 0));
    }
    (max_blocks_high >> TX_UNIT_LOG2).max(0) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Availability is TILE-relative. A block at frame row 8 that is the
    /// first row of its tile has NO above neighbour — the trap that makes
    /// every tile after the first code different contexts.
    #[test]
    fn availability_is_tile_relative_not_frame_relative() {
        let frame_origin = TileBounds {
            mi_row_start: 0,
            mi_col_start: 0,
        };
        let g = set_mi_row_col(&frame_origin, 8, 4, 8, 4, 64, 64, 64);
        assert!(g.up_available && g.left_available);
        assert_eq!(g.above_mi, Some(8 * 64 + 8 - 64));
        assert_eq!(g.left_mi, Some(8 * 64 + 8 - 1));

        let tile_origin = TileBounds {
            mi_row_start: 8,
            mi_col_start: 8,
        };
        let g = set_mi_row_col(&tile_origin, 8, 4, 8, 4, 64, 64, 64);
        assert!(!g.up_available && !g.left_available);
        assert_eq!(g.above_mi, None, "C stores NULL, not a valid pointer");
        assert_eq!(g.left_mi, None);
    }

    /// The bottom/right edges go NEGATIVE exactly when the block overhangs
    /// the frame, which is the gate `max_block_*` tests.
    #[test]
    fn edges_go_negative_only_past_the_frame() {
        let t = TileBounds::default();
        // 64x64 block (16x16 mi) fully inside a 64x64-mi frame.
        let inside = set_mi_row_col(&t, 0, 16, 0, 16, 64, 64, 64);
        assert!(inside.mb_to_bottom_edge > 0 && inside.mb_to_right_edge > 0);
        // Same block at the last mi row/col: it overhangs.
        let over = set_mi_row_col(&t, 60, 16, 60, 16, 64, 64, 64);
        assert!(over.mb_to_bottom_edge < 0 && over.mb_to_right_edge < 0);
        assert_eq!(over.mb_to_bottom_edge, (64 - 16 - 60) * 4 * 8);
    }

    /// Tier 4, traced against entropy_coding.c:4431-4451: an overhanging
    /// 64x64 block reports only the TX units that fit, and chroma shifts one
    /// extra bit.
    #[test]
    fn max_block_extent_clips_to_the_frame() {
        let t = TileBounds::default();
        let g = set_mi_row_col(&t, 56, 16, 56, 16, 64, 64, 64);
        // 8 mi = 32 px of the 64 fit -> 8 TX_4X4 units of 16.
        assert_eq!(
            max_block_wide(g.mb_to_right_edge, BlockSize::Block64x64, 0),
            8
        );
        assert_eq!(
            max_block_high(g.mb_to_bottom_edge, BlockSize::Block64x64, 0),
            8
        );
        // Chroma: the same overhang halves again.
        assert_eq!(
            max_block_wide(g.mb_to_right_edge, BlockSize::Block64x64, 1),
            12
        );
        // A block fully inside is unclipped.
        let g = set_mi_row_col(&t, 0, 16, 0, 16, 64, 64, 64);
        assert_eq!(
            max_block_wide(g.mb_to_right_edge, BlockSize::Block64x64, 0),
            16
        );
    }

    /// `is_sec_rect` is false for every square block, and is the LAST
    /// sub-block of a rectangular split otherwise.
    #[test]
    fn is_sec_rect_marks_the_last_rectangle() {
        let t = TileBounds::default();
        assert!(!set_mi_row_col(&t, 0, 4, 0, 4, 64, 64, 64).is_sec_rect);
        // 4x16 mi (bw=1, bh=4) — PARTITION_VERT_4's four columns at
        // mi_col 0,1,2,3: only the last is the "second rectangle".
        let flags: alloc::vec::Vec<bool> = (0..4)
            .map(|c| set_mi_row_col(&t, 0, 4, c, 1, 64, 64, 64).is_sec_rect)
            .collect();
        assert_eq!(flags, alloc::vec![false, false, false, true]);
        // 16x4 mi (bw=4, bh=1) — the horizontal twin reads mi_row instead.
        let flags: alloc::vec::Vec<bool> = (0..4)
            .map(|r| set_mi_row_col(&t, r, 1, 0, 4, 64, 64, 64).is_sec_rect)
            .collect();
        assert_eq!(flags, alloc::vec![false, true, true, true]);
    }
}
