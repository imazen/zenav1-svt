//! IntraBC MVP (DV predictor) stack — the `INTRA_FRAME` slice of SVT's
//! reference-MV list machinery (IBC chunk 6, `docs/ibc-port-map.md` §D).
//!
//! C sources (SVT-AV1 v4.2.0 `adaptive_mv_pred.c`, verified line-for-line):
//! - `add_ref_mv_candidate` (:57-128) — the single-ref arm (for
//!   `INTRA_FRAME`, `rf = {INTRA_FRAME, NONE_FRAME}`). Non-inter
//!   neighbours are skipped via `is_inter_block` = `use_intrabc ||
//!   ref_frame[0] > INTRA_FRAME` — on a KEY frame that means ONLY intrabc
//!   neighbours contribute (their DVs).
//! - `scan_row_mbmi` / `scan_col_mbmi` / `scan_blk_mbmi` (:130-264) —
//!   the ROW-1/COL-1/TR/TL/ROW-3,5/COL-3,5 neighbour scans with the exact
//!   len-stepping (including the candidate-width overshoot quirk) and
//!   weight/processed accounting.
//! - `has_top_right` (:266-325), `find_valid_row/col_offset` (:327-333).
//! - `setup_ref_mv_list` (:651-971) — scan order, `REF_CAT_LEVEL`
//!   boost, the 3-counter mode-context derivation, [`sort_mvp_table`],
//!   the light rescan, and the final [`clamp_mv_ref`]. The temporal-MVP
//!   block (:756-860) is NOT ported: it is gated on
//!   `frm_hdr.use_ref_frame_mvs`, which is 0 on the KEY-frame allintra
//!   envelope (and must be re-visited before any inter-frame work).
//! - `sort_mvp_table` (:450-465) — the stable "bubble with nr_len"
//!   descending-weight sort; ties keep insertion order (a load-bearing
//!   tie-break, map §F.8).
//! - `scan_row_col_light` (:469-648) — single-ref arm. On a KEY frame its
//!   scans add nothing (candidates need `ref_frame[i] > INTRA_FRAME`,
//!   never true), but the trailing gm-fill writes `this_mv = gm (= 0)`
//!   into slots `count..2` WITHOUT touching count or weight — replicated
//!   exactly (visible in the raw stack, invisible to the from-stack
//!   reads).
//! - `svt_av1_get_ref_mv_from_stack` (:2002-2028) +
//!   `svt_av1_find_best_ref_mvs_from_stack` (:2030-2040) +
//!   `lower_mv_precision` (inter_prediction.h:229).
//! - `svt_aom_generate_av1_mvp_table`'s `INTRA_FRAME` slice (:1329-1405):
//!   zero the stack + count, `gm_mv = {0, 0}`, run `setup_ref_mv_list`.
//! - `svt_aom_init_xd` (:1038-1123) — the per-block context derivation
//!   ([`derive_block_ctx`]): n8 dims, availability, `is_sec_rect`,
//!   `mb_to_*_edge`.
//!
//! Everything here is differentially locked against the exported C
//! symbols (`setup_ref_mv_list`, `sort_mvp_table`,
//! `svt_av1_find_best_ref_mvs_from_stack`) in
//! `tests/c_parity_intrabc_mvp.rs` on randomized KEY-frame mode-info
//! grids with intrabc neighbours.

use crate::intrabc::TileMiBounds;
use svtav1_tables::block::{NUM_4X4_BLOCKS_HIGH, NUM_4X4_BLOCKS_WIDE};
use svtav1_types::motion::{CandidateMv, MAX_REF_MV_STACK_SIZE, Mv};

/// C `MVREF_ROWS` / `MVREF_COLS` (adaptive_mv_pred.c:30-31).
const MVREF_ROWS: i32 = 3;
/// C `REF_CAT_LEVEL` (definitions.h:1365).
pub const REF_CAT_LEVEL: i32 = 640;
/// C `MAX_MV_REF_CANDIDATES` (definitions.h:1362).
pub const MAX_MV_REF_CANDIDATES: usize = 2;
/// C `MV_BORDER` (inter_prediction.h:31): 16 pels in 1/8-pel units.
const MV_BORDER: i32 = 16 << 3;
/// C `INVALID_MV` (mv.h:21).
pub const INVALID_MV: u32 = 0x8000_8000;
/// C `REFMV_OFFSET` / `GLOBALMV_OFFSET` (definitions.h:1345-1346).
const REFMV_OFFSET: i16 = 4;

/// One mode-info grid cell, replicated per 4x4 mi cell over each block's
/// footprint exactly as SVT's `mi_grid_base` is (`svt_copy_mi_map_grid`).
/// Carries the fields the INTRA_FRAME scans read from C's
/// `MbModeInfo`/`BlockModeInfo`.
#[derive(Debug, Clone, Copy)]
pub struct MvpMiEntry {
    /// C `MbModeInfo::bsize` (BlockSize enum value).
    pub bsize: u8,
    /// C `BlockModeInfo::mode` (PredictionMode enum value).
    pub mode: u8,
    /// C `BlockModeInfo::use_intrabc`.
    pub use_intrabc: bool,
    /// C `BlockModeInfo::ref_frame[2]` (`INTRA_FRAME = 0`,
    /// `NONE_FRAME = -1`; genuinely-inter values are > 0 and cannot occur
    /// on the KEY-frame grids this slice serves).
    pub ref_frame: [i8; 2],
    /// C `BlockModeInfo::mv[2]` (eighth-pel; a DV for intrabc blocks).
    pub mv: [Mv; 2],
    /// C `MbModeInfo::partition` (PartitionType; read by
    /// `has_top_right`'s VERT_A special case on the CURRENT cell).
    pub partition: u8,
}

impl Default for MvpMiEntry {
    /// A plain intra DC cell (the KEY-frame default).
    fn default() -> Self {
        Self {
            bsize: 0,
            mode: 0, // DC_PRED
            use_intrabc: false,
            ref_frame: [0, -1], // {INTRA_FRAME, NONE_FRAME}
            mv: [Mv::default(); 2],
            partition: 0,
        }
    }
}

/// C `is_inter_block` (block_structures.h:119-121).
#[inline]
fn is_inter_block(e: &MvpMiEntry) -> bool {
    e.use_intrabc || e.ref_frame[0] > 0
}

/// C `svt_aom_have_newmv_in_inter_mode` (NEWMV-family modes; PredictionMode
/// values per definitions.h:1183-1215). Unreachable-on-KEY but transcribed.
#[inline]
fn have_newmv_in_inter_mode(mode: u8) -> bool {
    // NEWMV=16, NEAREST_NEWMV=19, NEW_NEARESTMV=20, NEAR_NEWMV=21,
    // NEW_NEARMV=22, NEW_NEWMV=24.
    matches!(mode, 16 | 19 | 20 | 21 | 22 | 24)
}

/// The mode-info grid view: `entries` is a `rows * stride` cell array in
/// ABSOLUTE mi coordinates; `base` indexes the CURRENT block's top-left
/// cell (C's `xd->mi` pointer). Neighbour offsets are relative to `base`
/// exactly as C's `xd->mi[off]` (negative offsets reach up/left).
pub struct MvpGrid<'a> {
    pub entries: &'a [MvpMiEntry],
    pub stride: i32,
    pub base: i32,
}

impl MvpGrid<'_> {
    #[inline]
    fn at(&self, offset: i32) -> &MvpMiEntry {
        &self.entries[(self.base + offset) as usize]
    }
}

/// The per-block context `setup_ref_mv_list` reads off C's `MacroBlockD`,
/// derived per `svt_aom_init_xd` (adaptive_mv_pred.c:1038-1123) by
/// [`derive_block_ctx`].
#[derive(Debug, Clone, Copy)]
pub struct MvpBlockCtx {
    pub mi_row: i32,
    pub mi_col: i32,
    /// Block mi dims (C `xd->n8_w`/`n8_h` — despite the name these are
    /// 4x4-mi units, `bwidth >> MI_SIZE_LOG2`).
    pub n8_w: i32,
    pub n8_h: i32,
    /// Frame mi dims (C `cm->mi_cols`/`mi_rows`).
    pub mi_rows: i32,
    pub mi_cols: i32,
    pub tile: TileMiBounds,
    /// `mi_size_wide[seq_header.sb_size]` (16 for 64px SB, 32 for 128px).
    pub sb_mi_size: i32,
    pub is_sec_rect: bool,
    pub up_available: bool,
    pub left_available: bool,
    /// C `xd->mb_to_*_edge` (eighth-pel, init_xd:1054-1057).
    pub mb_to_left_edge: i32,
    pub mb_to_right_edge: i32,
    pub mb_to_top_edge: i32,
    pub mb_to_bottom_edge: i32,
}

/// C `svt_aom_init_xd`'s derivation slice (adaptive_mv_pred.c:1046-1082).
pub fn derive_block_ctx(
    mi_row: i32,
    mi_col: i32,
    bsize: usize,
    mi_rows: i32,
    mi_cols: i32,
    tile: TileMiBounds,
    sb_mi_size: i32,
) -> MvpBlockCtx {
    let bw = i32::from(NUM_4X4_BLOCKS_WIDE[bsize]);
    let bh = i32::from(NUM_4X4_BLOCKS_HIGH[bsize]);
    // is_sec_rect (:1063-1077).
    let mut is_sec_rect = false;
    if bw < bh && (mi_col + bw) & (bh - 1) == 0 {
        is_sec_rect = true;
    }
    if bw > bh && mi_row & (bw - 1) != 0 {
        is_sec_rect = true;
    }
    MvpBlockCtx {
        mi_row,
        mi_col,
        n8_w: bw,
        n8_h: bh,
        mi_rows,
        mi_cols,
        tile,
        sb_mi_size,
        is_sec_rect,
        up_available: mi_row > tile.mi_row_start,
        left_available: mi_col > tile.mi_col_start,
        mb_to_left_edge: -((mi_col * 4) * 8),
        mb_to_right_edge: ((mi_cols - bw - mi_col) * 4) * 8,
        mb_to_top_edge: -((mi_row * 4) * 8),
        mb_to_bottom_edge: ((mi_rows - bh - mi_row) * 4) * 8,
    }
}

/// C `clamp_mv_ref` (adaptive_mv_pred.c:49-55): clamp into the block's
/// UMV border box (`bw`/`bh` in PIXELS = `n8 << MI_SIZE_LOG2`).
fn clamp_mv_ref(mv: &mut Mv, bw_px: i32, bh_px: i32, ctx: &MvpBlockCtx) {
    let clamp_c = |v: i32, low: i32, high: i32| -> i32 {
        if v < low {
            low
        } else if v > high {
            high
        } else {
            v
        }
    };
    mv.x = clamp_c(
        i32::from(mv.x),
        ctx.mb_to_left_edge - bw_px * 8 - MV_BORDER,
        ctx.mb_to_right_edge + bw_px * 8 + MV_BORDER,
    ) as i16;
    mv.y = clamp_c(
        i32::from(mv.y),
        ctx.mb_to_top_edge - bh_px * 8 - MV_BORDER,
        ctx.mb_to_bottom_edge + bh_px * 8 + MV_BORDER,
    ) as i16;
}

/// C `add_ref_mv_candidate` (adaptive_mv_pred.c:57-128), single-ref arm
/// (`rf = {INTRA_FRAME, NONE_FRAME}` — the only shape this slice serves).
///
/// The `is_global_mv_block` branch (:71) is structurally dead here: it
/// needs `mode ∈ {GLOBALMV, GLOBAL_GLOBALMV}` (impossible on a KEY frame,
/// where inter-classified blocks are intrabc with `mode == DC_PRED`) AND
/// `gm_params[INTRA_FRAME].wmtype > TRANSLATION` (SVT leaves
/// `global_motion[INTRA_FRAME]` at IDENTITY). Debug-asserted, not ported.
#[allow(clippy::too_many_arguments)]
fn add_ref_mv_candidate(
    candidate: &MvpMiEntry,
    refmv_count: &mut u8,
    ref_match_count: &mut u8,
    newmv_count: &mut u8,
    ref_mv_stack: &mut [CandidateMv; MAX_REF_MV_STACK_SIZE],
    len: i32,
    weight: i32,
) {
    if !is_inter_block(candidate) {
        return; // for intrabc
    }
    debug_assert!(
        !matches!(candidate.mode, 15 | 23),
        "GLOBALMV-family mode on a KEY-frame grid (see fn doc)"
    );
    for r in 0..2 {
        if candidate.ref_frame[r] == 0 {
            // == rf[0] == INTRA_FRAME
            let this_refmv = candidate.mv[r];
            let mut index = usize::from(*refmv_count);
            for (i, entry) in ref_mv_stack.iter_mut().enumerate().take(usize::from(*refmv_count)) {
                if entry.this_mv.as_int() == this_refmv.as_int() {
                    entry.weight += weight * len;
                    index = i;
                    break;
                }
            }
            if index == usize::from(*refmv_count) && usize::from(*refmv_count) < MAX_REF_MV_STACK_SIZE {
                ref_mv_stack[index].this_mv = this_refmv;
                ref_mv_stack[index].weight = weight * len;
                *refmv_count += 1;
            }
            if have_newmv_in_inter_mode(candidate.mode) {
                *newmv_count += 1;
            }
            *ref_match_count += 1;
        }
    }
}

/// C `scan_row_mbmi` (adaptive_mv_pred.c:130-184).
#[allow(clippy::too_many_arguments)]
fn scan_row_mbmi(
    grid: &MvpGrid,
    ctx: &MvpBlockCtx,
    row_offset: i32,
    ref_mv_stack: &mut [CandidateMv; MAX_REF_MV_STACK_SIZE],
    refmv_count: &mut u8,
    ref_match_count: &mut u8,
    newmv_count: &mut u8,
    max_row_offset: i32,
    processed_rows: &mut i32,
) {
    let mut end_mi = ctx.n8_w.min(ctx.mi_cols - ctx.mi_col);
    end_mi = end_mi.min(16); // mi_size_wide[BLOCK_64X64]
    let n8_w_8 = 2i32; // mi_size_wide[BLOCK_8X8]
    let n8_w_16 = 4i32; // mi_size_wide[BLOCK_16X16]
    let mut col_offset = 0i32;
    if row_offset.abs() > 1 {
        col_offset = 1;
        if ctx.mi_col & 1 != 0 && ctx.n8_w < n8_w_8 {
            col_offset -= 1;
        }
    }
    let use_step_16 = ctx.n8_w >= 16;

    let mut i = 0i32;
    while i < end_mi {
        let candidate = grid.at(row_offset * grid.stride + col_offset + i);
        let cand_bsize = usize::from(candidate.bsize);
        let n8_w = i32::from(NUM_4X4_BLOCKS_WIDE[cand_bsize]);
        let mut len = ctx.n8_w.min(n8_w);
        if use_step_16 {
            len = n8_w_16.max(len);
        } else if row_offset.abs() > 1 {
            len = len.max(n8_w_8);
        }

        let mut weight = 2i32;
        if ctx.n8_w >= n8_w_8 && ctx.n8_w <= n8_w {
            let inc = (-max_row_offset + row_offset + 1).min(i32::from(NUM_4X4_BLOCKS_HIGH[cand_bsize]));
            weight = weight.max(inc); // << shift(0)
            *processed_rows = inc - row_offset - 1;
        }

        add_ref_mv_candidate(candidate, refmv_count, ref_match_count, newmv_count, ref_mv_stack, len, weight);
        i += len;
    }
}

/// C `scan_col_mbmi` (adaptive_mv_pred.c:186-239).
#[allow(clippy::too_many_arguments)]
fn scan_col_mbmi(
    grid: &MvpGrid,
    ctx: &MvpBlockCtx,
    col_offset: i32,
    ref_mv_stack: &mut [CandidateMv; MAX_REF_MV_STACK_SIZE],
    refmv_count: &mut u8,
    ref_match_count: &mut u8,
    newmv_count: &mut u8,
    max_col_offset: i32,
    processed_cols: &mut i32,
) {
    let mut end_mi = ctx.n8_h.min(ctx.mi_rows - ctx.mi_row);
    end_mi = end_mi.min(16); // mi_size_high[BLOCK_64X64]
    let n8_h_8 = 2i32;
    let n8_h_16 = 4i32;
    let mut row_offset = 0i32;
    if col_offset.abs() > 1 {
        row_offset = 1;
        if ctx.mi_row & 1 != 0 && ctx.n8_h < n8_h_8 {
            row_offset -= 1;
        }
    }
    let use_step_16 = ctx.n8_h >= 16;

    let mut i = 0i32;
    while i < end_mi {
        let candidate = grid.at((row_offset + i) * grid.stride + col_offset);
        let cand_bsize = usize::from(candidate.bsize);
        let n8_h = i32::from(NUM_4X4_BLOCKS_HIGH[cand_bsize]);
        let mut len = ctx.n8_h.min(n8_h);
        if use_step_16 {
            len = n8_h_16.max(len);
        } else if col_offset.abs() > 1 {
            len = len.max(n8_h_8);
        }

        let mut weight = 2i32;
        if ctx.n8_h >= n8_h_8 && ctx.n8_h <= n8_h {
            let inc = (-max_col_offset + col_offset + 1).min(i32::from(NUM_4X4_BLOCKS_WIDE[cand_bsize]));
            weight = weight.max(inc);
            *processed_cols = inc - col_offset - 1;
        }

        add_ref_mv_candidate(candidate, refmv_count, ref_match_count, newmv_count, ref_mv_stack, len, weight);
        i += len;
    }
}

/// C `is_inside` (adaptive_mv_pred.c:44-47).
#[inline]
fn is_inside(tile: TileMiBounds, mi_col: i32, mi_row: i32, pos_row: i32, pos_col: i32) -> bool {
    !(mi_row + pos_row < tile.mi_row_start
        || mi_col + pos_col < tile.mi_col_start
        || mi_row + pos_row >= tile.mi_row_end
        || mi_col + pos_col >= tile.mi_col_end)
}

/// C `scan_blk_mbmi` (adaptive_mv_pred.c:241-264).
#[allow(clippy::too_many_arguments)]
fn scan_blk_mbmi(
    grid: &MvpGrid,
    ctx: &MvpBlockCtx,
    row_offset: i32,
    col_offset: i32,
    ref_mv_stack: &mut [CandidateMv; MAX_REF_MV_STACK_SIZE],
    ref_match_count: &mut u8,
    newmv_count: &mut u8,
    refmv_count: &mut u8,
) {
    if is_inside(ctx.tile, ctx.mi_col, ctx.mi_row, row_offset, col_offset) {
        let candidate = grid.at(row_offset * grid.stride + col_offset);
        add_ref_mv_candidate(
            candidate,
            refmv_count,
            ref_match_count,
            newmv_count,
            ref_mv_stack,
            2, // mi_size_wide[BLOCK_8X8]
            2,
        );
    }
}

/// C `has_top_right` (adaptive_mv_pred.c:266-325). `bs` is
/// `max(n8_w, n8_h)`; reads the CURRENT cell's partition for the VERT_A
/// special case (PARTITION_VERT_A = 6, definitions.h:933-943).
fn has_top_right(grid: &MvpGrid, ctx: &MvpBlockCtx, bs: i32) -> bool {
    if bs > 16 {
        // mi_size_wide[BLOCK_64X64]
        return false;
    }
    if ctx.n8_w > ctx.n8_h && ctx.is_sec_rect {
        return false;
    }
    if ctx.n8_w < ctx.n8_h && !ctx.is_sec_rect {
        return true;
    }

    let sb_mi_size = ctx.sb_mi_size;
    let mask_row = ctx.mi_row & (sb_mi_size - 1);
    let mask_col = ctx.mi_col & (sb_mi_size - 1);

    let mut has_tr = !((mask_row & bs != 0) && (mask_col & bs != 0));

    let mut b = bs;
    while b < sb_mi_size {
        if mask_col & b != 0 {
            if (mask_col & (2 * b) != 0) && (mask_row & (2 * b) != 0) {
                has_tr = false;
                break;
            }
        } else {
            break;
        }
        b <<= 1;
    }

    if grid.at(0).partition == 6 {
        // PARTITION_VERT_A
        if ctx.n8_w == ctx.n8_h && mask_row & bs != 0 {
            return false;
        }
    }

    has_tr
}

/// C `find_valid_row_offset` / `find_valid_col_offset`
/// (adaptive_mv_pred.c:327-333).
#[inline]
fn find_valid_row_offset(tile: TileMiBounds, mi_row: i32, row_offset: i32) -> i32 {
    row_offset.clamp(tile.mi_row_start - mi_row, tile.mi_row_end - mi_row - 1)
}

#[inline]
fn find_valid_col_offset(tile: TileMiBounds, mi_col: i32, col_offset: i32) -> i32 {
    col_offset.clamp(tile.mi_col_start - mi_col, tile.mi_col_end - mi_col - 1)
}

/// C `sort_mvp_table` (adaptive_mv_pred.c:450-465, EXPORTED): stable
/// descending-weight bubble sort — a strict-`<` swap keeps equal-weight
/// entries in insertion order (the tie-break, map §F.8).
pub fn sort_mvp_table(ref_mv_stack: &mut [CandidateMv], refmv_count: u8) {
    let mut len = usize::from(refmv_count);
    while len > 0 {
        let mut nr_len = 0usize;
        for idx in 1..len {
            if ref_mv_stack[idx - 1].weight < ref_mv_stack[idx].weight {
                ref_mv_stack.swap(idx - 1, idx);
                nr_len = idx;
            }
        }
        len = nr_len;
    }
}

/// C `scan_row_col_light` (adaptive_mv_pred.c:469-648), single-ref arm
/// (`rf[1] == NONE_FRAME`). See the module doc: on KEY grids the scans
/// are structurally empty (they require `ref_frame[i] > INTRA_FRAME`) but
/// are transcribed anyway; the trailing gm-fill IS observable in the raw
/// stack.
#[allow(clippy::too_many_arguments)]
fn scan_row_col_light(
    grid: &MvpGrid,
    ctx: &MvpBlockCtx,
    ref_mv_stack: &mut [CandidateMv; MAX_REF_MV_STACK_SIZE],
    refmv_count: &mut u8,
    max_row_offset: i32,
    max_col_offset: i32,
) {
    let mut mi_width = 16i32.min(ctx.n8_w);
    mi_width = mi_width.min(ctx.mi_cols - ctx.mi_col);
    let mut mi_height = 16i32.min(ctx.n8_h);
    mi_height = mi_height.min(ctx.mi_rows - ctx.mi_row);
    let mi_size = mi_width.min(mi_height);

    // Single reference frame extension (:577-647). ROW-1 rescan:
    let mut idx = 0i32;
    while max_row_offset.abs() >= 1 && idx < mi_size && usize::from(*refmv_count) < MAX_MV_REF_CANDIDATES {
        let candidate = grid.at(-grid.stride + idx);
        let candidate_bsize = usize::from(candidate.bsize);
        for r in 0..2usize {
            if candidate.ref_frame[r] > 0 {
                // > INTRA_FRAME — never true on a KEY grid; the sign-bias
                // flip (:590-594) is unreachable and not carried.
                let this_mv = candidate.mv[r];
                let mut stack_idx = usize::from(*refmv_count);
                for (i, e) in ref_mv_stack.iter().enumerate().take(usize::from(*refmv_count)) {
                    if this_mv.as_int() == e.this_mv.as_int() {
                        stack_idx = i;
                        break;
                    }
                }
                if stack_idx == usize::from(*refmv_count) {
                    ref_mv_stack[stack_idx].this_mv = this_mv;
                    ref_mv_stack[stack_idx].weight = 2;
                    *refmv_count += 1;
                }
            }
        }
        idx += i32::from(NUM_4X4_BLOCKS_WIDE[candidate_bsize]);
    }

    // COL-1 rescan:
    let mut idx = 0i32;
    while max_col_offset.abs() >= 1 && idx < mi_size && usize::from(*refmv_count) < MAX_MV_REF_CANDIDATES {
        let candidate = grid.at(idx * grid.stride - 1);
        let candidate_bsize = usize::from(candidate.bsize);
        for r in 0..2usize {
            if candidate.ref_frame[r] > 0 {
                let this_mv = candidate.mv[r];
                let mut stack_idx = usize::from(*refmv_count);
                for (i, e) in ref_mv_stack.iter().enumerate().take(usize::from(*refmv_count)) {
                    if this_mv.as_int() == e.this_mv.as_int() {
                        stack_idx = i;
                        break;
                    }
                }
                if stack_idx == usize::from(*refmv_count) {
                    ref_mv_stack[stack_idx].this_mv = this_mv;
                    ref_mv_stack[stack_idx].weight = 2;
                    *refmv_count += 1;
                }
            }
        }
        idx += i32::from(NUM_4X4_BLOCKS_HIGH[candidate_bsize]);
    }

    // gm-fill (:644-646): this_mv only — count and weight untouched.
    for idx in usize::from(*refmv_count)..MAX_MV_REF_CANDIDATES {
        ref_mv_stack[idx].this_mv = Mv::default(); // gm_mv_candidates[0] == 0
    }
}

/// Output of [`setup_ref_mv_list_intra`].
#[derive(Debug, Clone)]
pub struct MvpStack {
    pub stack: [CandidateMv; MAX_REF_MV_STACK_SIZE],
    pub count: u8,
    pub mode_context: i16,
}

/// C `setup_ref_mv_list` (adaptive_mv_pred.c:651-971) for
/// `ref_frame == INTRA_FRAME` on a KEY frame (`use_ref_frame_mvs == 0`;
/// the temporal-MVP block is structurally skipped — see module doc).
/// The caller provides a ZEROED stack per C's `generate_av1_mvp_table`
/// memset (:1358-1359) — [`generate_mvp_table_intra_frame`] does.
pub fn setup_ref_mv_list_intra(grid: &MvpGrid, ctx: &MvpBlockCtx) -> MvpStack {
    let mut stack = [CandidateMv::default(); MAX_REF_MV_STACK_SIZE];
    let mut refmv_count = 0u8;
    let mut mode_context: i16 = 0;

    let bs = ctx.n8_w.max(ctx.n8_h);
    let has_tr = has_top_right(grid, ctx, bs);
    let row_adj = ctx.n8_h < 2 && (ctx.mi_row & 1) != 0;
    let col_adj = ctx.n8_w < 2 && (ctx.mi_col & 1) != 0;
    let mut processed_rows = 0i32;
    let mut processed_cols = 0i32;

    let mut max_row_offset = 0i32;
    let mut max_col_offset = 0i32;

    if ctx.up_available {
        max_row_offset = -(MVREF_ROWS << 1) + i32::from(row_adj);
        if ctx.n8_h < 2 {
            max_row_offset = -(2 << 1) + i32::from(row_adj);
        }
        max_row_offset = find_valid_row_offset(ctx.tile, ctx.mi_row, max_row_offset);
    }

    if ctx.left_available {
        max_col_offset = -(MVREF_ROWS << 1) + i32::from(col_adj);
        if ctx.n8_w < 2 {
            max_col_offset = -(2 << 1) + i32::from(col_adj);
        }
        max_col_offset = find_valid_col_offset(ctx.tile, ctx.mi_col, max_col_offset);
    }

    let mut col_match_count = 0u8;
    let mut row_match_count = 0u8;
    let mut newmv_count = 0u8;

    // ROW-1.
    if max_row_offset.abs() >= 1 {
        scan_row_mbmi(
            grid,
            ctx,
            -1,
            &mut stack,
            &mut refmv_count,
            &mut row_match_count,
            &mut newmv_count,
            max_row_offset,
            &mut processed_rows,
        );
    }
    // COL-1.
    if max_col_offset.abs() >= 1 {
        scan_col_mbmi(
            grid,
            ctx,
            -1,
            &mut stack,
            &mut refmv_count,
            &mut col_match_count,
            &mut newmv_count,
            max_col_offset,
            &mut processed_cols,
        );
    }
    // TOP-RIGHT.
    if has_tr {
        scan_blk_mbmi(
            grid,
            ctx,
            -1,
            ctx.n8_w,
            &mut stack,
            &mut row_match_count,
            &mut newmv_count,
            &mut refmv_count,
        );
    }

    let nearest_match = u8::from(row_match_count > 0) + u8::from(col_match_count > 0);

    for entry in stack.iter_mut().take(usize::from(refmv_count)) {
        entry.weight += REF_CAT_LEVEL;
    }

    // Temporal MVP (:756-860): use_ref_frame_mvs == 0 on KEY — skipped.

    // TOP-LEFT (dummy newmv counter, :863-877).
    let mut dummy_newmv_count = 0u8;
    scan_blk_mbmi(
        grid,
        ctx,
        -1,
        -1,
        &mut stack,
        &mut row_match_count,
        &mut dummy_newmv_count,
        &mut refmv_count,
    );

    // ROW-3/COL-3, ROW-5/COL-5 (:880-915).
    for idx in 2..=MVREF_ROWS {
        let row_offset = -(idx << 1) + 1 + i32::from(row_adj);
        let col_offset = -(idx << 1) + 1 + i32::from(col_adj);

        if row_offset.abs() <= max_row_offset.abs() && row_offset.abs() > processed_rows {
            scan_row_mbmi(
                grid,
                ctx,
                row_offset,
                &mut stack,
                &mut refmv_count,
                &mut row_match_count,
                &mut dummy_newmv_count,
                max_row_offset,
                &mut processed_rows,
            );
        }
        if col_offset.abs() <= max_col_offset.abs() && col_offset.abs() > processed_cols {
            scan_col_mbmi(
                grid,
                ctx,
                col_offset,
                &mut stack,
                &mut refmv_count,
                &mut col_match_count,
                &mut dummy_newmv_count,
                max_col_offset,
                &mut processed_cols,
            );
        }
    }

    // Mode-context derivation (:917-949).
    let ref_match_count = u8::from(row_match_count > 0) + u8::from(col_match_count > 0);
    match nearest_match {
        0 => {
            if ref_match_count >= 1 {
                mode_context |= 1;
            }
            if ref_match_count == 1 {
                mode_context |= 1 << REFMV_OFFSET;
            } else if ref_match_count >= 2 {
                mode_context |= 2 << REFMV_OFFSET;
            }
        }
        1 => {
            mode_context |= if newmv_count > 0 { 2 } else { 3 };
            if ref_match_count == 1 {
                mode_context |= 3 << REFMV_OFFSET;
            } else if ref_match_count >= 2 {
                mode_context |= 4 << REFMV_OFFSET;
            }
        }
        _ => {
            mode_context |= if newmv_count >= 1 { 4 } else { 5 };
            mode_context |= 5 << REFMV_OFFSET;
        }
    }

    // Sort (:952-955).
    if refmv_count > 1 {
        sort_mvp_table(&mut stack, refmv_count);
    }

    // Light rescan (:957-961).
    if usize::from(refmv_count) < MAX_MV_REF_CANDIDATES {
        scan_row_col_light(grid, ctx, &mut stack, &mut refmv_count, max_row_offset, max_col_offset);
    }

    // Final clamp (:963-970; single-ref: this_mv only).
    let bw_px = ctx.n8_w << 2; // MI_SIZE_LOG2
    let bh_px = ctx.n8_h << 2;
    for entry in stack.iter_mut().take(usize::from(refmv_count)) {
        clamp_mv_ref(&mut entry.this_mv, bw_px, bh_px, ctx);
    }

    MvpStack { stack, count: refmv_count, mode_context }
}

/// C `svt_aom_generate_av1_mvp_table`'s `INTRA_FRAME` slice
/// (adaptive_mv_pred.c:1329-1405): zeroed stack + count, `gm_mv = 0`,
/// then [`setup_ref_mv_list_intra`]. (The zeroing is implicit — the
/// setup fn starts from a fresh zeroed stack.)
pub fn generate_mvp_table_intra_frame(grid: &MvpGrid, ctx: &MvpBlockCtx) -> MvpStack {
    setup_ref_mv_list_intra(grid, ctx)
}

/// C `lower_mv_precision` (inter_prediction.h:229-243), the
/// `is_integer = 0` arm.
fn lower_mv_precision(mv: &mut Mv, allow_hp: bool) {
    if !allow_hp {
        if mv.y & 1 != 0 {
            mv.y += if mv.y > 0 { -1 } else { 1 };
        }
        if mv.x & 1 != 0 {
            mv.x += if mv.x > 0 { -1 } else { 1 };
        }
    }
}

/// C `svt_av1_get_ref_mv_from_stack` (adaptive_mv_pred.c:2002-2028),
/// single-ref arm (`ref_frame[1] <= INTRA_FRAME`).
fn get_ref_mv_from_stack(stack: &MvpStack, ref_mv_idx: usize) -> Mv {
    if ref_mv_idx < usize::from(stack.count) {
        stack.stack[ref_mv_idx].this_mv
    } else {
        Mv::from_int(INVALID_MV)
    }
}

/// C `svt_av1_find_best_ref_mvs_from_stack` (adaptive_mv_pred.c:2030-2040)
/// with `allow_hp = 0, is_integer = 0` (the IBC call shape,
/// mode_decision.c:3019). Returns `(nearest_mv, near_mv)` — `INVALID_MV`
/// (0x80008000) when the stack is short, exactly as C.
pub fn find_best_ref_mvs_from_stack(stack: &MvpStack) -> (Mv, Mv) {
    let mut nearest = get_ref_mv_from_stack(stack, 0);
    lower_mv_precision(&mut nearest, false);
    let mut near = get_ref_mv_from_stack(stack, 1);
    lower_mv_precision(&mut near, false);
    (nearest, near)
}

/// The full dv_ref composition of `intra_bc_search`
/// (mode_decision.c:3018-3032): from-stack nearest/near, INVALID → 0,
/// nearest-else-near, zero → `find_ref_dv` fallback. Returns the dv_ref
/// the DV search seeds from (asserted whole-pel, as C does).
pub fn compose_dv_ref(stack: &MvpStack, tile: TileMiBounds, sb_mi_size: i32, mi_row: i32) -> Mv {
    let (mut nearestmv, mut nearmv) = find_best_ref_mvs_from_stack(stack);
    if nearestmv.as_int() == INVALID_MV {
        nearestmv = Mv::default();
    }
    if nearmv.as_int() == INVALID_MV {
        nearmv = Mv::default();
    }
    let dv_ref = crate::intrabc::resolve_dv_ref(nearestmv, nearmv, tile, sb_mi_size, mi_row);
    debug_assert!(dv_ref.x & 7 == 0 && dv_ref.y & 7 == 0);
    dv_ref
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_grid(rows: i32, cols: i32) -> alloc::vec::Vec<MvpMiEntry> {
        alloc::vec![MvpMiEntry::default(); (rows * cols) as usize]
    }

    /// Pure-intra neighbourhood → empty stack, INVALID nearest/near, and
    /// the dv_ref composition falls through to find_ref_dv.
    #[test]
    fn intra_only_grid_gives_empty_stack() {
        let entries = flat_grid(32, 32);
        let tile = TileMiBounds { mi_col_start: 0, mi_col_end: 32, mi_row_start: 0, mi_row_end: 32 };
        let ctx = derive_block_ctx(8, 8, 3 /*8x8*/, 32, 32, tile, 16);
        let grid = MvpGrid { entries: &entries, stride: 32, base: 8 * 32 + 8 };
        let out = generate_mvp_table_intra_frame(&grid, &ctx);
        assert_eq!(out.count, 0);
        // gm-fill leaves this_mv zero in slots 0..2 with zero weight.
        assert_eq!(out.stack[0].this_mv.as_int(), 0);
        let (nearest, near) = find_best_ref_mvs_from_stack(&out);
        assert_eq!(nearest.as_int(), INVALID_MV);
        assert_eq!(near.as_int(), INVALID_MV);
        let dv_ref = compose_dv_ref(&out, tile, 16, 8);
        // find_ref_dv non-first-SB-row default: (0, -sb_mi_size*MI*8).
        assert_eq!((dv_ref.x, dv_ref.y), (0, -(16 * 4 * 8) as i16));
    }

    /// A single intrabc left neighbour: its DV lands on the stack with
    /// weight 2*len + REF_CAT_LEVEL and becomes nearest.
    #[test]
    fn single_intrabc_neighbour_becomes_nearest() {
        let mut entries = flat_grid(32, 32);
        let dv = Mv { x: -64, y: -8 };
        // 8x8 intrabc block occupying cells (8..10, 6..8).
        for r in 8..10 {
            for c in 6..8 {
                entries[(r * 32 + c) as usize] = MvpMiEntry {
                    bsize: 3,
                    mode: 0,
                    use_intrabc: true,
                    ref_frame: [0, -1],
                    mv: [dv, Mv::default()],
                    partition: 0,
                };
            }
        }
        let tile = TileMiBounds { mi_col_start: 0, mi_col_end: 32, mi_row_start: 0, mi_row_end: 32 };
        let ctx = derive_block_ctx(8, 8, 3, 32, 32, tile, 16);
        let grid = MvpGrid { entries: &entries, stride: 32, base: 8 * 32 + 8 };
        let out = generate_mvp_table_intra_frame(&grid, &ctx);
        assert_eq!(out.count, 1);
        assert_eq!(out.stack[0].this_mv.as_int(), dv.as_int());
        // COL-1 scan: len = min(2, 2) = 2, weight = max(2, inc) where
        // inc = min(-max_col_offset - 1 + 1, cand_w=2), max_col_offset=-6
        // (clamped) → inc = min(6, 2) = 2 → weight 2 → 2*2 + 640.
        assert_eq!(out.stack[0].weight, 2 * 2 + REF_CAT_LEVEL);
        let dv_ref = compose_dv_ref(&out, tile, 16, 8);
        assert_eq!(dv_ref.as_int(), dv.as_int());
    }
}
