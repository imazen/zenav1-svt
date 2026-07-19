//! Intra-prediction edge availability and extended edge-array construction
//! for directional modes.
//!
//! C-exact port of libaom `av1/common/reconintra.c` (the AV1 reference
//! decoder — the authority the recon-parity gate judges against):
//! - `has_tr_*` / `has_bl_*` availability tables (verbatim)
//! - `has_top_right()` / `has_bottom_left()`
//! - the directional-mode edge preparation of
//!   `build_directional_and_filter_intra_predictors()` combined with the
//!   `n_top_px` / `n_topright_px` / `n_left_px` / `n_bottomleft_px`
//!   derivation from `av1_predict_intra_block()`.
//!
//! Specializations valid for every stream this encoder emits (asserted or
//! documented at the call site):
//! - one whole-block transform per coded block → `row_off == col_off == 0`
//!   and `tx_size_wide_unit == mi_size_wide[bsize]` (ditto height);
//! - directional modes are luma-only (chroma is UV_DC) → `ss_x == ss_y == 0`;
//! - a single tile spanning the frame → `tile.mi_row_end == mi_rows`,
//!   `tile.mi_col_end == mi_cols`, availability from `mi_row/mi_col > 0`;
//! - SH signals `enable_intra_edge_filter = 0` and no filter_intra →
//!   `disable_edge_filter` in C: no edge filtering, no upsampling — only
//!   array construction;
//! - `use_128x128_superblock = 0` → 64x64 superblocks (`sb_mi_size == 16`).

use svtav1_types::partition::PartitionType;

/// C `MAX_MIB_SIZE_LOG2` (av1_common_int.h): superblock size in mi units,
/// log2, for the LARGEST superblock (128px = 32 mi). The availability
/// tables are laid out for a 128x128 superblock grid regardless of the
/// actual `sb_size`; `mi_row & (sb_mi_size - 1)` keeps indices inside the
/// top-left quadrant for 64x64 superblocks.
const MAX_MIB_SIZE_LOG2: usize = 5;

// =============================================================================
// Top-right availability tables — verbatim from libaom reconintra.c
// (`has_tr_4x4` .. `has_tr_64x16`). Each bit is one block position in
// raster order within a 128x128 superblock:
// `(table[idx / 8] >> (idx % 8)) & 1`.
// =============================================================================

#[rustfmt::skip]
static HAS_TR_4X4: [u8; 128] = [
    255, 255, 255, 255, 85, 85, 85, 85, 119, 119, 119, 119, 85, 85, 85, 85,
    127, 127, 127, 127, 85, 85, 85, 85, 119, 119, 119, 119, 85, 85, 85, 85,
    255, 127, 255, 127, 85, 85, 85, 85, 119, 119, 119, 119, 85, 85, 85, 85,
    127, 127, 127, 127, 85, 85, 85, 85, 119, 119, 119, 119, 85, 85, 85, 85,
    255, 255, 255, 127, 85, 85, 85, 85, 119, 119, 119, 119, 85, 85, 85, 85,
    127, 127, 127, 127, 85, 85, 85, 85, 119, 119, 119, 119, 85, 85, 85, 85,
    255, 127, 255, 127, 85, 85, 85, 85, 119, 119, 119, 119, 85, 85, 85, 85,
    127, 127, 127, 127, 85, 85, 85, 85, 119, 119, 119, 119, 85, 85, 85, 85,
];
#[rustfmt::skip]
static HAS_TR_4X8: [u8; 64] = [
    255, 255, 255, 255, 119, 119, 119, 119, 127, 127, 127, 127, 119,
    119, 119, 119, 255, 127, 255, 127, 119, 119, 119, 119, 127, 127,
    127, 127, 119, 119, 119, 119, 255, 255, 255, 127, 119, 119, 119,
    119, 127, 127, 127, 127, 119, 119, 119, 119, 255, 127, 255, 127,
    119, 119, 119, 119, 127, 127, 127, 127, 119, 119, 119, 119,
];
#[rustfmt::skip]
static HAS_TR_8X4: [u8; 64] = [
    255, 255, 0, 0, 85, 85, 0, 0, 119, 119, 0, 0, 85, 85, 0, 0,
    127, 127, 0, 0, 85, 85, 0, 0, 119, 119, 0, 0, 85, 85, 0, 0,
    255, 127, 0, 0, 85, 85, 0, 0, 119, 119, 0, 0, 85, 85, 0, 0,
    127, 127, 0, 0, 85, 85, 0, 0, 119, 119, 0, 0, 85, 85, 0, 0,
];
#[rustfmt::skip]
static HAS_TR_8X8: [u8; 32] = [
    255, 255, 85, 85, 119, 119, 85, 85, 127, 127, 85, 85, 119, 119, 85, 85,
    255, 127, 85, 85, 119, 119, 85, 85, 127, 127, 85, 85, 119, 119, 85, 85,
];
#[rustfmt::skip]
static HAS_TR_8X16: [u8; 16] = [
    255, 255, 119, 119, 127, 127, 119, 119,
    255, 127, 119, 119, 127, 127, 119, 119,
];
#[rustfmt::skip]
static HAS_TR_16X8: [u8; 16] = [
    255, 0, 85, 0, 119, 0, 85, 0, 127, 0, 85, 0, 119, 0, 85, 0,
];
#[rustfmt::skip]
static HAS_TR_16X16: [u8; 8] = [
    255, 85, 119, 85, 127, 85, 119, 85,
];
static HAS_TR_16X32: [u8; 4] = [255, 119, 127, 119];
static HAS_TR_32X16: [u8; 4] = [15, 5, 7, 5];
static HAS_TR_32X32: [u8; 2] = [95, 87];
static HAS_TR_32X64: [u8; 1] = [127];
static HAS_TR_64X32: [u8; 1] = [19];
static HAS_TR_64X64: [u8; 1] = [7];
static HAS_TR_64X128: [u8; 1] = [3];
static HAS_TR_128X64: [u8; 1] = [1];
static HAS_TR_128X128: [u8; 1] = [1];
#[rustfmt::skip]
static HAS_TR_4X16: [u8; 32] = [
    255, 255, 255, 255, 127, 127, 127, 127, 255, 127, 255,
    127, 127, 127, 127, 127, 255, 255, 255, 127, 127, 127,
    127, 127, 255, 127, 255, 127, 127, 127, 127, 127,
];
#[rustfmt::skip]
static HAS_TR_16X4: [u8; 32] = [
    255, 0, 0, 0, 85, 0, 0, 0, 119, 0, 0, 0, 85, 0, 0, 0,
    127, 0, 0, 0, 85, 0, 0, 0, 119, 0, 0, 0, 85, 0, 0, 0,
];
#[rustfmt::skip]
static HAS_TR_8X32: [u8; 8] = [
    255, 255, 127, 127, 255, 127, 127, 127,
];
#[rustfmt::skip]
static HAS_TR_32X8: [u8; 8] = [
    15, 0, 5, 0, 7, 0, 5, 0,
];
static HAS_TR_16X64: [u8; 2] = [255, 127];
static HAS_TR_64X16: [u8; 2] = [3, 1];

// C `has_tr_vert_*`: block visit order for PARTITION_VERT_A/VERT_B (the
// last split level goes TL, BL, TR, BR). Verbatim from reconintra.c.
#[rustfmt::skip]
static HAS_TR_VERT_8X8: [u8; 32] = [
    255, 255, 0, 0, 119, 119, 0, 0, 127, 127, 0, 0, 119, 119, 0, 0,
    255, 127, 0, 0, 119, 119, 0, 0, 127, 127, 0, 0, 119, 119, 0, 0,
];
#[rustfmt::skip]
static HAS_TR_VERT_16X16: [u8; 8] = [
    255, 0, 119, 0, 127, 0, 119, 0,
];
static HAS_TR_VERT_32X32: [u8; 2] = [15, 7];
static HAS_TR_VERT_64X64: [u8; 1] = [3];

/// C `has_tr_tables[BLOCK_SIZES_ALL]`, keyed by block pixel dims.
/// Returns `None` for (w, h) pairs that are not an AV1 block size.
fn has_tr_table_plain(bw: usize, bh: usize) -> Option<&'static [u8]> {
    Some(match (bw, bh) {
        (4, 4) => &HAS_TR_4X4[..],
        (4, 8) => &HAS_TR_4X8[..],
        (8, 4) => &HAS_TR_8X4[..],
        (8, 8) => &HAS_TR_8X8[..],
        (8, 16) => &HAS_TR_8X16[..],
        (16, 8) => &HAS_TR_16X8[..],
        (16, 16) => &HAS_TR_16X16[..],
        (16, 32) => &HAS_TR_16X32[..],
        (32, 16) => &HAS_TR_32X16[..],
        (32, 32) => &HAS_TR_32X32[..],
        (32, 64) => &HAS_TR_32X64[..],
        (64, 32) => &HAS_TR_64X32[..],
        (64, 64) => &HAS_TR_64X64[..],
        (64, 128) => &HAS_TR_64X128[..],
        (128, 64) => &HAS_TR_128X64[..],
        (128, 128) => &HAS_TR_128X128[..],
        (4, 16) => &HAS_TR_4X16[..],
        (16, 4) => &HAS_TR_16X4[..],
        (8, 32) => &HAS_TR_8X32[..],
        (32, 8) => &HAS_TR_32X8[..],
        (16, 64) => &HAS_TR_16X64[..],
        (64, 16) => &HAS_TR_64X16[..],
        _ => return None,
    })
}

/// C `has_tr_vert_tables[BLOCK_SIZES]` (NULL entries → `None`, which the C
/// code asserts against — only reachable for partitions we never emit).
fn has_tr_table_vert(bw: usize, bh: usize) -> Option<&'static [u8]> {
    Some(match (bw, bh) {
        (4, 8) => &HAS_TR_4X8[..],
        (8, 8) => &HAS_TR_VERT_8X8[..],
        (8, 16) => &HAS_TR_8X16[..],
        (16, 16) => &HAS_TR_VERT_16X16[..],
        (16, 32) => &HAS_TR_16X32[..],
        (32, 32) => &HAS_TR_VERT_32X32[..],
        (32, 64) => &HAS_TR_32X64[..],
        (64, 64) => &HAS_TR_VERT_64X64[..],
        (64, 128) => &HAS_TR_64X128[..],
        (128, 128) => &HAS_TR_128X128[..],
        _ => return None,
    })
}

/// C `get_has_tr_table()`. Panics where the C code would assert (a NULL
/// table entry) — unreachable for the partition types this encoder emits.
fn get_has_tr_table(partition: PartitionType, bw: usize, bh: usize) -> &'static [u8] {
    let ret = if partition == PartitionType::VertA || partition == PartitionType::VertB {
        has_tr_table_vert(bw, bh)
    } else {
        has_tr_table_plain(bw, bh)
    };
    ret.expect("no has_tr table for this block shape (C would assert)")
}

// =============================================================================
// Bottom-left availability tables — verbatim from libaom reconintra.c
// (`has_bl_4x4` .. `has_bl_64x16`).
// =============================================================================

#[rustfmt::skip]
static HAS_BL_4X4: [u8; 128] = [
    84, 85, 85, 85, 16, 17, 17, 17, 84, 85, 85, 85, 0,  1,  1,  1,  84, 85, 85,
    85, 16, 17, 17, 17, 84, 85, 85, 85, 0,  0,  1,  0,  84, 85, 85, 85, 16, 17,
    17, 17, 84, 85, 85, 85, 0,  1,  1,  1,  84, 85, 85, 85, 16, 17, 17, 17, 84,
    85, 85, 85, 0,  0,  0,  0,  84, 85, 85, 85, 16, 17, 17, 17, 84, 85, 85, 85,
    0,  1,  1,  1,  84, 85, 85, 85, 16, 17, 17, 17, 84, 85, 85, 85, 0,  0,  1,
    0,  84, 85, 85, 85, 16, 17, 17, 17, 84, 85, 85, 85, 0,  1,  1,  1,  84, 85,
    85, 85, 16, 17, 17, 17, 84, 85, 85, 85, 0,  0,  0,  0,
];
#[rustfmt::skip]
static HAS_BL_4X8: [u8; 64] = [
    16, 17, 17, 17, 0, 1, 1, 1, 16, 17, 17, 17, 0, 0, 1, 0,
    16, 17, 17, 17, 0, 1, 1, 1, 16, 17, 17, 17, 0, 0, 0, 0,
    16, 17, 17, 17, 0, 1, 1, 1, 16, 17, 17, 17, 0, 0, 1, 0,
    16, 17, 17, 17, 0, 1, 1, 1, 16, 17, 17, 17, 0, 0, 0, 0,
];
#[rustfmt::skip]
static HAS_BL_8X4: [u8; 64] = [
    254, 255, 84, 85, 254, 255, 16, 17, 254, 255, 84, 85, 254, 255, 0, 1,
    254, 255, 84, 85, 254, 255, 16, 17, 254, 255, 84, 85, 254, 255, 0, 0,
    254, 255, 84, 85, 254, 255, 16, 17, 254, 255, 84, 85, 254, 255, 0, 1,
    254, 255, 84, 85, 254, 255, 16, 17, 254, 255, 84, 85, 254, 255, 0, 0,
];
#[rustfmt::skip]
static HAS_BL_8X8: [u8; 32] = [
    84, 85, 16, 17, 84, 85, 0, 1, 84, 85, 16, 17, 84, 85, 0, 0,
    84, 85, 16, 17, 84, 85, 0, 1, 84, 85, 16, 17, 84, 85, 0, 0,
];
#[rustfmt::skip]
static HAS_BL_8X16: [u8; 16] = [
    16, 17, 0, 1, 16, 17, 0, 0, 16, 17, 0, 1, 16, 17, 0, 0,
];
#[rustfmt::skip]
static HAS_BL_16X8: [u8; 16] = [
    254, 84, 254, 16, 254, 84, 254, 0, 254, 84, 254, 16, 254, 84, 254, 0,
];
#[rustfmt::skip]
static HAS_BL_16X16: [u8; 8] = [
    84, 16, 84, 0, 84, 16, 84, 0,
];
static HAS_BL_16X32: [u8; 4] = [16, 0, 16, 0];
static HAS_BL_32X16: [u8; 4] = [78, 14, 78, 14];
static HAS_BL_32X32: [u8; 2] = [4, 4];
static HAS_BL_32X64: [u8; 1] = [0];
static HAS_BL_64X32: [u8; 1] = [34];
static HAS_BL_64X64: [u8; 1] = [0];
static HAS_BL_64X128: [u8; 1] = [0];
static HAS_BL_128X64: [u8; 1] = [0];
static HAS_BL_128X128: [u8; 1] = [0];
#[rustfmt::skip]
static HAS_BL_4X16: [u8; 32] = [
    0, 1, 1, 1, 0, 0, 1, 0, 0, 1, 1, 1, 0, 0, 0, 0,
    0, 1, 1, 1, 0, 0, 1, 0, 0, 1, 1, 1, 0, 0, 0, 0,
];
#[rustfmt::skip]
static HAS_BL_16X4: [u8; 32] = [
    254, 254, 254, 84, 254, 254, 254, 16, 254, 254, 254, 84, 254, 254, 254, 0,
    254, 254, 254, 84, 254, 254, 254, 16, 254, 254, 254, 84, 254, 254, 254, 0,
];
#[rustfmt::skip]
static HAS_BL_8X32: [u8; 8] = [
    0, 1, 0, 0, 0, 1, 0, 0,
];
#[rustfmt::skip]
static HAS_BL_32X8: [u8; 8] = [
    238, 78, 238, 14, 238, 78, 238, 14,
];
static HAS_BL_16X64: [u8; 2] = [0, 0];
static HAS_BL_64X16: [u8; 2] = [42, 42];

#[rustfmt::skip]
static HAS_BL_VERT_8X8: [u8; 32] = [
    254, 255, 16, 17, 254, 255, 0, 1, 254, 255, 16, 17, 254, 255, 0, 0,
    254, 255, 16, 17, 254, 255, 0, 1, 254, 255, 16, 17, 254, 255, 0, 0,
];
#[rustfmt::skip]
static HAS_BL_VERT_16X16: [u8; 8] = [
    254, 16, 254, 0, 254, 16, 254, 0,
];
static HAS_BL_VERT_32X32: [u8; 2] = [14, 14];
static HAS_BL_VERT_64X64: [u8; 1] = [2];

/// C `has_bl_tables[BLOCK_SIZES_ALL]`, keyed by block pixel dims.
fn has_bl_table_plain(bw: usize, bh: usize) -> Option<&'static [u8]> {
    Some(match (bw, bh) {
        (4, 4) => &HAS_BL_4X4[..],
        (4, 8) => &HAS_BL_4X8[..],
        (8, 4) => &HAS_BL_8X4[..],
        (8, 8) => &HAS_BL_8X8[..],
        (8, 16) => &HAS_BL_8X16[..],
        (16, 8) => &HAS_BL_16X8[..],
        (16, 16) => &HAS_BL_16X16[..],
        (16, 32) => &HAS_BL_16X32[..],
        (32, 16) => &HAS_BL_32X16[..],
        (32, 32) => &HAS_BL_32X32[..],
        (32, 64) => &HAS_BL_32X64[..],
        (64, 32) => &HAS_BL_64X32[..],
        (64, 64) => &HAS_BL_64X64[..],
        (64, 128) => &HAS_BL_64X128[..],
        (128, 64) => &HAS_BL_128X64[..],
        (128, 128) => &HAS_BL_128X128[..],
        (4, 16) => &HAS_BL_4X16[..],
        (16, 4) => &HAS_BL_16X4[..],
        (8, 32) => &HAS_BL_8X32[..],
        (32, 8) => &HAS_BL_32X8[..],
        (16, 64) => &HAS_BL_16X64[..],
        (64, 16) => &HAS_BL_64X16[..],
        _ => return None,
    })
}

/// C `has_bl_vert_tables[BLOCK_SIZES]` (NULL entries → `None`).
fn has_bl_table_vert(bw: usize, bh: usize) -> Option<&'static [u8]> {
    Some(match (bw, bh) {
        (4, 8) => &HAS_BL_4X8[..],
        (8, 8) => &HAS_BL_VERT_8X8[..],
        (8, 16) => &HAS_BL_8X16[..],
        (16, 16) => &HAS_BL_VERT_16X16[..],
        (16, 32) => &HAS_BL_16X32[..],
        (32, 32) => &HAS_BL_VERT_32X32[..],
        (32, 64) => &HAS_BL_32X64[..],
        (64, 64) => &HAS_BL_VERT_64X64[..],
        (64, 128) => &HAS_BL_64X128[..],
        (128, 128) => &HAS_BL_128X128[..],
        _ => return None,
    })
}

/// C `get_has_bl_table()`.
fn get_has_bl_table(partition: PartitionType, bw: usize, bh: usize) -> &'static [u8] {
    let ret = if partition == PartitionType::VertA || partition == PartitionType::VertB {
        has_bl_table_vert(bw, bh)
    } else {
        has_bl_table_plain(bw, bh)
    };
    ret.expect("no has_bl table for this block shape (C would assert)")
}

/// Whether (w, h) is one of the 22 AV1 block sizes (BLOCK_SIZES_ALL).
/// RDO can transiently evaluate unsignalable shapes (e.g. an 8x64 strip
/// from a 4:1 split of a partial-superblock region); those never survive
/// into a decodable stream, so callers fall back to "extension pixels
/// unavailable" for them instead of consulting the tables.
pub fn is_av1_block_shape(w: usize, h: usize) -> bool {
    has_tr_table_plain(w, h).is_some()
}

/// C `has_top_right()` from libaom reconintra.c — exact port.
///
/// - `sb_mi_size`: `mi_size_high[sb_size]` (16 for 64x64 superblocks,
///   32 for 128x128).
/// - `bw_px`/`bh_px`: `block_size_wide/high[bsize]` of the CODED block.
/// - `mi_row`/`mi_col`: the block's absolute mi position (4px units).
/// - `top_available`/`right_available`: C `have_top` (`row_off ||
///   up_available`) and `mi_col + ((col_off + txw) << ss_x) <
///   tile.mi_col_end`.
/// - `tx_w_mi`: `tx_size_wide_unit[txsz]`.
/// - `row_off`/`col_off`: tx-block offset within the coded block, in
///   plane mi units.
#[allow(clippy::too_many_arguments)]
pub fn has_top_right(
    sb_mi_size: usize,
    bw_px: usize,
    bh_px: usize,
    mi_row: usize,
    mi_col: usize,
    top_available: bool,
    right_available: bool,
    partition: PartitionType,
    tx_w_mi: usize,
    row_off: usize,
    col_off: usize,
    ss_x: usize,
    ss_y: usize,
) -> bool {
    if !top_available || !right_available {
        return false;
    }

    let bw_unit = bw_px >> 2; // mi_size_wide[bsize]
    let plane_bw_unit = (bw_unit >> ss_x).max(1);
    let top_right_count_unit = tx_w_mi;

    if row_off > 0 {
        // Just need to check if enough pixels on the right.
        if bw_px > 64 {
            // C: block_size_wide[bsize] > block_size_wide[BLOCK_64X64].
            // Special case: For 128x128 blocks, the transform unit whose
            // top-right corner is at the center of the block does in fact
            // have pixels available at its top-right corner.
            // (mi_size_high[BLOCK_64X64] == mi_size_wide[BLOCK_64X64] == 16)
            if row_off == (16 >> ss_y) && col_off + top_right_count_unit == (16 >> ss_x) {
                return true;
            }
            let plane_bw_unit_64 = 16 >> ss_x;
            let col_off_64 = col_off % plane_bw_unit_64;
            return col_off_64 + top_right_count_unit < plane_bw_unit_64;
        }
        col_off + top_right_count_unit < plane_bw_unit
    } else {
        // All top-right pixels are in the block above, which is already
        // available.
        if col_off + top_right_count_unit < plane_bw_unit {
            return true;
        }

        let bw_in_mi_log2 = (bw_px >> 2).trailing_zeros() as usize;
        let bh_in_mi_log2 = (bh_px >> 2).trailing_zeros() as usize;
        let blk_row_in_sb = (mi_row & (sb_mi_size - 1)) >> bh_in_mi_log2;
        let blk_col_in_sb = (mi_col & (sb_mi_size - 1)) >> bw_in_mi_log2;

        // Top row of superblock: so top-right pixels are in the top and/or
        // top-right superblocks, both of which are already available.
        if blk_row_in_sb == 0 {
            return true;
        }

        // Rightmost column of superblock (and not the top row): so
        // top-right pixels fall in the right superblock, which is not
        // available yet.
        if ((blk_col_in_sb + 1) << bw_in_mi_log2) >= sb_mi_size {
            return false;
        }

        // General case (neither top row nor rightmost column): check if
        // the top-right block is coded before the current block.
        let this_blk_index = (blk_row_in_sb << (MAX_MIB_SIZE_LOG2 - bw_in_mi_log2)) + blk_col_in_sb;
        let idx1 = this_blk_index / 8;
        let idx2 = this_blk_index % 8;
        let table = get_has_tr_table(partition, bw_px, bh_px);
        ((table[idx1] >> idx2) & 1) != 0
    }
}

/// C `has_bottom_left()` from libaom reconintra.c — exact port.
///
/// - `bottom_available`: C `(yd > 0) && (mi_row + ((row_off + txh) <<
///   ss_y) < tile.mi_row_end)`.
/// - `left_available`: C `have_left` (`col_off || left_available`).
/// - `tx_h_mi`: `tx_size_high_unit[txsz]`.
#[allow(clippy::too_many_arguments)]
pub fn has_bottom_left(
    sb_mi_size: usize,
    bw_px: usize,
    bh_px: usize,
    mi_row: usize,
    mi_col: usize,
    bottom_available: bool,
    left_available: bool,
    partition: PartitionType,
    tx_h_mi: usize,
    row_off: usize,
    col_off: usize,
    ss_x: usize,
    ss_y: usize,
) -> bool {
    if !bottom_available || !left_available {
        return false;
    }

    // Special case for 128x* blocks, when col_off is half the block width.
    // This is needed because 128x* superblocks are divided into 64x*
    // blocks in raster order.
    if bw_px > 64 && col_off > 0 {
        let plane_bw_unit_64 = 16 >> ss_x;
        let col_off_64 = col_off % plane_bw_unit_64;
        if col_off_64 == 0 {
            // We are at the left edge of top-right or bottom-right 64x* block.
            let plane_bh_unit_64 = 16 >> ss_y;
            let row_off_64 = row_off % plane_bh_unit_64;
            let plane_bh_unit = ((bh_px >> 2) >> ss_y).min(plane_bh_unit_64);
            // Check if all bottom-left pixels are in the left 64x* block
            // (which is already coded).
            return row_off_64 + tx_h_mi < plane_bh_unit;
        }
    }

    if col_off > 0 {
        // Bottom-left pixels are in the bottom-left block, which is not
        // available.
        false
    } else {
        let bh_unit = bh_px >> 2; // mi_size_high[bsize]
        let plane_bh_unit = (bh_unit >> ss_y).max(1);
        let bottom_left_count_unit = tx_h_mi;

        // All bottom-left pixels are in the left block, which is already
        // available.
        if row_off + bottom_left_count_unit < plane_bh_unit {
            return true;
        }

        let bw_in_mi_log2 = (bw_px >> 2).trailing_zeros() as usize;
        let bh_in_mi_log2 = (bh_px >> 2).trailing_zeros() as usize;
        let blk_row_in_sb = (mi_row & (sb_mi_size - 1)) >> bh_in_mi_log2;
        let blk_col_in_sb = (mi_col & (sb_mi_size - 1)) >> bw_in_mi_log2;

        // Leftmost column of superblock: so bottom-left pixels maybe in
        // the left and/or bottom-left superblocks. But only the left
        // superblock is available, so check if all required pixels fall
        // in that superblock.
        if blk_col_in_sb == 0 {
            // C: blk_row_in_sb << (bh_in_mi_log2 + MI_SIZE_LOG2 -
            //    MI_SIZE_LOG2) >> ss_y  (the MI_SIZE_LOG2 terms cancel)
            let blk_start_row_off = (blk_row_in_sb << bh_in_mi_log2) >> ss_y;
            let row_off_in_sb = blk_start_row_off + row_off;
            let sb_height_unit = sb_mi_size >> ss_y;
            return row_off_in_sb + bottom_left_count_unit < sb_height_unit;
        }

        // Bottom row of superblock (and not the leftmost column): so
        // bottom-left pixels fall in the bottom superblock, which is not
        // available yet.
        if ((blk_row_in_sb + 1) << bh_in_mi_log2) >= sb_mi_size {
            return false;
        }

        // General case (neither leftmost column nor bottom row): check if
        // the bottom-left block is coded before the current block.
        let this_blk_index = (blk_row_in_sb << (MAX_MIB_SIZE_LOG2 - bw_in_mi_log2)) + blk_col_in_sb;
        let idx1 = this_blk_index / 8;
        let idx2 = this_blk_index % 8;
        let table = get_has_bl_table(partition, bw_px, bh_px);
        ((table[idx1] >> idx2) & 1) != 0
    }
}

/// Edge arrays for one directional prediction, matching what the decoder's
/// `build_directional_and_filter_intra_predictors` hands to `dr_predictor`.
pub enum DirEdges {
    /// The decoder's early exit: the required edge has zero real pixels,
    /// so the whole prediction is one flat value (no dr kernel runs).
    Flat(u8),
    /// `above[i]` = C `above_row[i]` (len `w + max(w, h)`), `left[i]` =
    /// C `left_col[i]` (len `h + max(w, h)`), `top_left` = C
    /// `above_row[-1] == left_col[-1]`. The dr kernels read at most
    /// `w + h` entries of each; regions the C code never constructs keep
    /// its memset defaults (127 for above, 129 for left).
    Edges {
        above: alloc::vec::Vec<u8>,
        left: alloc::vec::Vec<u8>,
        top_left: u8,
    },
}

/// Build the extended neighbor arrays for a directional intra prediction
/// of a `width` x `height` luma block at absolute pixel position
/// (`abs_x`, `abs_y`), reading previously-reconstructed pixels from the
/// frame-sized `recon` buffer (`stride` = frame width; the frame height is
/// `recon.len() / stride`).
///
/// Mirrors libaom `av1_predict_intra_block` (n_*_px derivation) +
/// `build_directional_and_filter_intra_predictors` (array construction)
/// for the specialization documented in the module header: whole-block
/// transforms (`row_off == col_off == 0`), luma plane (`ss == 0`), single
/// tile, 64x64 superblocks, `disable_edge_filter`.
///
/// `partition` is the partition type the block will be signaled under
/// (only PARTITION_VERT_A/B select different availability tables; this
/// encoder never emits those, so callers pass `PartitionType::None`).
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
pub fn build_directional_edges(
    recon: &[u8],
    stride: usize,
    abs_x: usize,
    abs_y: usize,
    width: usize,
    height: usize,
    p_angle: i32,
    partition: PartitionType,
    // C `seq_header.sb_mi_size` — 16 (SB64) or 32 (SB128), task #91.
    sb_mi_size: usize,
) -> DirEdges {
    let frame_w = stride;
    let frame_h = recon.len() / stride;
    // C av1_set_mb_mi: mi dims are the frame dims aligned up to 8 px.
    // All pipeline frames are 64-aligned; the debug_assert documents that
    // unaligned dims would need the decoder's padded-recon semantics.
    debug_assert_eq!(recon.len() % stride, 0);
    debug_assert!(frame_w % 8 == 0 && frame_h % 8 == 0);
    let mi_cols = 2 * ((frame_w + 7) >> 3);
    let mi_rows = 2 * ((frame_h + 7) >> 3);
    let mi_row = abs_y >> 2;
    let mi_col = abs_x >> 2;
    let txwpx = width;
    let txhpx = height;
    let tx_w_mi = width >> 2;
    let tx_h_mi = height >> 2;

    // C: have_top = row_off || up_available; have_left = col_off ||
    // left_available (row_off == col_off == 0; single tile → available
    // whenever a neighbor row/col exists).
    let have_top = abs_y > 0;
    let have_left = abs_x > 0;

    // C: xr/yd = distance from the block's right/bottom edge to the
    // frame's mi-aligned right/bottom edge, in pixels.
    let xr = (4 * mi_cols) as i64 - (abs_x + width) as i64;
    let yd = (4 * mi_rows) as i64 - (abs_y + height) as i64;

    // C: right_available = mi_col + ((col_off + txw) << ss_x) <
    //    tile.mi_col_end; bottom_available = (yd > 0) && (mi_row +
    //    ((row_off + txh) << ss_y) < tile.mi_row_end).
    let right_available = mi_col + tx_w_mi < mi_cols;
    let bottom_available = yd > 0 && mi_row + tx_h_mi < mi_rows;

    // C: need_top_right = p_angle < 90; need_bottom_left = p_angle > 180.
    let need_top_right = p_angle < 90;
    let need_bottom_left = p_angle > 180;

    let n_top_px: i64 = if have_top {
        (txwpx as i64).min(xr + txwpx as i64)
    } else {
        0
    };
    let n_left_px: i64 = if have_left {
        (txhpx as i64).min(yd + txhpx as i64)
    } else {
        0
    };

    // -1: not needed; 0: needed but unavailable; 1: available.
    // Unsignalable RDO-transient shapes (see is_av1_block_shape) get the
    // conservative "unavailable" so no table lookup can panic; they never
    // reach the bitstream.
    let shape_ok = is_av1_block_shape(width, height);
    let have_top_right: i32 = if need_top_right {
        i32::from(
            shape_ok
                && has_top_right(
                    sb_mi_size,
                    width,
                    height,
                    mi_row,
                    mi_col,
                    have_top,
                    right_available,
                    partition,
                    tx_w_mi,
                    0,
                    0,
                    0,
                    0,
                ),
        )
    } else {
        -1
    };
    let have_bottom_left: i32 = if need_bottom_left {
        i32::from(
            shape_ok
                && has_bottom_left(
                    sb_mi_size,
                    width,
                    height,
                    mi_row,
                    mi_col,
                    bottom_available,
                    have_left,
                    partition,
                    tx_h_mi,
                    0,
                    0,
                    0,
                    0,
                ),
        )
    } else {
        -1
    };
    let n_topright_px: i64 = if have_top_right > 0 {
        (txwpx as i64).min(xr)
    } else {
        have_top_right as i64
    };
    let n_bottomleft_px: i64 = if have_bottom_left > 0 {
        (txhpx as i64).min(yd)
    } else {
        have_bottom_left as i64
    };

    // === build_directional_and_filter_intra_predictors, dr-mode path ===
    // C: if (p_angle <= 90) need_above=1, need_left=0, need_above_left=1;
    //    else if (p_angle < 180) all three; else need_above=0, need_left=1,
    //    need_above_left=1.
    let (need_above, need_left) = if p_angle <= 90 {
        (true, false)
    } else if p_angle < 180 {
        (true, true)
    } else {
        (false, true)
    };

    // C ref pointers: above_ref = ref - ref_stride; left_ref = ref - 1.
    let above_ref = |i: usize| recon[(abs_y - 1) * stride + abs_x + i];
    let left_ref = |i: usize| recon[(abs_y + i) * stride + abs_x - 1];

    // C early exit: the required edge has no real pixels → flat fill.
    if (!need_above && n_left_px == 0) || (!need_left && n_top_px == 0) {
        let val = if need_left {
            if n_top_px > 0 { above_ref(0) } else { 129 }
        } else if n_left_px > 0 {
            left_ref(0)
        } else {
            127
        };
        return DirEdges::Flat(val);
    }

    // C: memset(left_data, 129, ...); memset(above_data, 127, ...).
    //
    // Array sizing: C uses one fixed NUM_INTRA_NEIGHBOUR_PIXELS (=
    // 2*MAX_TX_SIZE + 32) buffer. The construction below writes real
    // pixels up to index txwpx + n_topright_px - 1 (top-right copy caps
    // at txwpx, NOT txhpx — for wide blocks that exceeds txwpx + txhpx),
    // while the dr kernels only ever read up to index
    // txwpx + txhpx - 1 (max_base_x). Size for the larger of the two so
    // the construction is bit-for-bit C (including which sample the
    // replication step reads as `above_row[i - 1]`).
    let above_len = txwpx + txwpx.max(txhpx);
    let left_len = txhpx + txwpx.max(txhpx);
    let mut above = alloc::vec![127u8; above_len];
    let mut left = alloc::vec![129u8; left_len];

    // NEED_LEFT
    if need_left {
        let num_left_pixels_needed = txhpx + if n_bottomleft_px >= 0 { txwpx } else { 0 };
        if n_left_px > 0 {
            let n = n_left_px as usize;
            for (i, l) in left.iter_mut().enumerate().take(n) {
                *l = left_ref(i);
            }
            let mut i = n;
            if n_bottomleft_px > 0 {
                // C: assert(i == txhpx)
                debug_assert_eq!(i, txhpx);
                let end = txhpx + n_bottomleft_px as usize;
                while i < end {
                    left[i] = left_ref(i);
                    i += 1;
                }
            }
            if i < num_left_pixels_needed {
                let v = left[i - 1];
                for l in left.iter_mut().take(num_left_pixels_needed).skip(i) {
                    *l = v;
                }
            }
        } else if n_top_px > 0 {
            let v = above_ref(0);
            for l in left.iter_mut().take(num_left_pixels_needed) {
                *l = v;
            }
        }
    }

    // NEED_ABOVE
    if need_above {
        let num_top_pixels_needed = txwpx + if n_topright_px >= 0 { txhpx } else { 0 };
        if n_top_px > 0 {
            let n = n_top_px as usize;
            for (i, a) in above.iter_mut().enumerate().take(n) {
                *a = above_ref(i);
            }
            let mut i = n;
            if n_topright_px > 0 {
                // C: assert(n_top_px == txwpx)
                debug_assert_eq!(n, txwpx);
                let end = txwpx + n_topright_px as usize;
                while i < end {
                    above[i] = above_ref(i);
                    i += 1;
                }
            }
            if i < num_top_pixels_needed {
                let v = above[i - 1];
                for a in above.iter_mut().take(num_top_pixels_needed).skip(i) {
                    *a = v;
                }
            }
        } else if n_left_px > 0 {
            let v = left_ref(0);
            for a in above.iter_mut().take(num_top_pixels_needed) {
                *a = v;
            }
        }
    }

    // need_above_left (always set for directional modes).
    let top_left = if n_top_px > 0 && n_left_px > 0 {
        recon[(abs_y - 1) * stride + abs_x - 1] // above_ref[-1]
    } else if n_top_px > 0 {
        above_ref(0)
    } else if n_left_px > 0 {
        left_ref(0)
    } else {
        128
    };

    DirEdges::Edges {
        above,
        left,
        top_left,
    }
}

// ---------------------------------------------------------------------------
// Generalized C-exact directional prediction (edge fill + optional intra
// edge filter/upsample + dr kernel), covering whole blocks AND sub-TX
// units, luma AND chroma planes — the ENCODER MD path
// (`svt_av1_predict_intra_block` geometry, enc_intra_prediction.c:452-566,
// + `build_intra_predictors` dr branch :181-215).
// ---------------------------------------------------------------------------

/// Geometry of one directional prediction unit.
///
/// All `*_px` are PLANE-domain pixels; `mi_*` are LUMA mi (4px) units of
/// the CODED block (C `xd->mi_row/mi_col`, `blk_geom->bsize`); `row_off` /
/// `col_off` are the TX offset within the block in PLANE mi units (C
/// `tx_org >> 2`).
pub struct DrGeom {
    /// Plane-domain absolute origin of the TX unit.
    pub px: usize,
    pub py: usize,
    /// TX dims (plane px).
    pub txw: usize,
    pub txh: usize,
    /// Coded block's luma mi position.
    pub mi_row: usize,
    pub mi_col: usize,
    /// Coded block dims in luma px (`block_size_wide/high[bsize]`).
    pub bw_px: usize,
    pub bh_px: usize,
    /// TX offset within the block, plane mi units.
    pub row_off: usize,
    pub col_off: usize,
    /// Plane subsampling (0 = luma, 1 = 4:2:0 chroma).
    pub ss: usize,
    /// LUMA frame dims in px (64-aligned in this encoder).
    pub frame_w: usize,
    pub frame_h: usize,
    /// C `seq_header.sb_mi_size` — superblock size in MI (4px) units: 16
    /// at SB64, 32 at SB128 (task #91). The availability tables index
    /// blocks by `mi & (sb_mi_size - 1)`, so this is NOT a constant once
    /// SB128 is reachable: a block at mi_col 16 is the SB's left column at
    /// SB64 but its right half at SB128, with different top-right /
    /// bottom-left availability.
    pub sb_mi_size: usize,
    /// Task #96: the CURRENT TILE's bounds in LUMA mi units — C's
    /// `xd->tile.mi_{row,col}_{start,end}`. AV1 intra prediction stops at
    /// a tile boundary on every side, so C derives all four availability
    /// predicates from the TILE, not the frame:
    ///
    /// ```text
    /// have_top         = row_off || (mi_row > tile.mi_row_start)   // adaptive_mv_pred.c:1058
    /// have_left        = col_off || (mi_col > tile.mi_col_start)   // :1059
    /// right_available  = mi_col + ((col_off + txw) << ss) < tile.mi_col_end
    /// bottom_available = yd > 0 && mi_row + ((row_off + txh) << ss) < tile.mi_row_end
    /// ```
    ///
    /// `TileMi::whole_frame(w, h)` reproduces the previous single-tile
    /// behaviour exactly (start 0, end = frame mi), so every single-tile
    /// caller is byte-identical by construction.
    pub tile: TileMi,
}

/// A tile's bounds in LUMA mi (4px) units — C `TileInfo`'s
/// `mi_{row,col}_{start,end}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TileMi {
    pub mi_row_start: usize,
    pub mi_row_end: usize,
    pub mi_col_start: usize,
    pub mi_col_end: usize,
}

impl TileMi {
    /// The whole frame as one tile — the single-tile default, and exactly
    /// the bounds the pre-task-#96 code hardcoded.
    pub fn whole_frame(frame_w: usize, frame_h: usize) -> Self {
        Self {
            mi_row_start: 0,
            mi_row_end: 2 * ((frame_h + 7) >> 3),
            mi_col_start: 0,
            mi_col_end: 2 * ((frame_w + 7) >> 3),
        }
    }

    /// Plane-domain pixel origin of the tile's top edge (`ss` = 0 luma,
    /// 1 for 4:2:0 chroma).
    pub fn top_px(&self, ss: usize) -> usize {
        (self.mi_row_start * 4) >> ss
    }

    /// Plane-domain pixel origin of the tile's left edge.
    pub fn left_px(&self, ss: usize) -> usize {
        (self.mi_col_start * 4) >> ss
    }
}

/// C `mode_to_angle_map` (V..D67) + `ANGLE_STEP = 3`.
pub const MODE_TO_ANGLE_MAP: [i32; 13] = [0, 90, 180, 45, 135, 113, 157, 203, 67, 0, 0, 0, 0];

/// Directional prediction of one TX unit, C-exact against the encoder MD
/// path: `svt_av1_predict_intra_block` n_px geometry +
/// `build_intra_predictors`' dr branch, including the intra edge filter,
/// corner filter and edge upsampling when `edge_filter` (SH
/// `enable_intra_edge_filter`) is set. `sample(x, y)` returns the plane
/// pixel at plane coords (x, y) — callers implement live-plane or
/// depth-overlay reads; it is only consulted for previously-coded
/// positions per the availability rules.
///
/// `filt_type` is C `get_filt_type(xd, plane)`: 1 when the above OR left
/// neighbor of the CODED BLOCK is smooth (per-plane mode), else 0.
#[allow(clippy::too_many_arguments)]
pub fn dr_predict<S: Fn(usize, usize) -> u8>(
    sample: S,
    g: &DrGeom,
    p_angle: i32,
    edge_filter: bool,
    filt_type: i32,
    partition: PartitionType,
    dst: &mut [u8],
) {
    use svtav1_dsp::intra_pred as ip;
    // `frame_w`/`frame_h` are the luma dims — TRUE (possibly odd, task #95)
    // OR 8-aligned; both round to the SAME aligned mi grid below because
    // `(d + 7) >> 3 == aligned(d) / 8` by definition, so `mi_cols`/`mi_rows`
    // equal C `av1_cm->mi_cols/rows` for either. (The old `% 8 == 0` assert
    // wrongly rejected the legitimate odd-true-dim input.)
    debug_assert!(g.frame_w > 0 && g.frame_h > 0);
    let mi_cols = 2 * ((g.frame_w + 7) >> 3);
    let mi_rows = 2 * ((g.frame_h + 7) >> 3);
    let txwpx = g.txw as i64;
    let txhpx = g.txh as i64;
    let wpx = (g.bw_px >> g.ss) as i64;
    let hpx = (g.bh_px >> g.ss) as i64;
    let x_off = (g.col_off * 4) as i64;
    let y_off = (g.row_off * 4) as i64;

    // C: have_top = row_off || up_available; have_left = col_off ||
    // left_available — and BOTH are tile-scoped
    // (`xd->up_available = mi_row > tile->mi_row_start`,
    // adaptive_mv_pred.c:1058-1059). Identical to the previous `> 0` for
    // a whole-frame tile.
    let have_top = g.row_off > 0 || g.mi_row > g.tile.mi_row_start;
    let have_left = g.col_off > 0 || g.mi_col > g.tile.mi_col_start;

    // C: xr/yd — distance from the TX unit's right/bottom edge to the
    // frame's mi-aligned edges, in PLANE px (mb_to_right_edge >> (3+ss)).
    let xr = (((mi_cols * 4) as i64 - (g.mi_col * 4) as i64 - g.bw_px as i64) >> g.ss)
        + (wpx - x_off - txwpx);
    let yd = (((mi_rows * 4) as i64 - (g.mi_row * 4) as i64 - g.bh_px as i64) >> g.ss)
        + (hpx - y_off - txhpx);

    let txw_mi = g.txw / 4;
    let txh_mi = g.txh / 4;
    // Task #96: C bounds these by the TILE's mi_col_end / mi_row_end, not
    // the frame's — a block on the tile's right edge has no top-right
    // even mid-frame. `mi_cols`/`mi_rows` for a whole-frame tile.
    let right_available = g.mi_col + ((g.col_off + txw_mi) << g.ss) < g.tile.mi_col_end;
    let bottom_available = yd > 0 && g.mi_row + ((g.row_off + txh_mi) << g.ss) < g.tile.mi_row_end;

    let shape_ok = is_av1_block_shape(g.bw_px, g.bh_px);
    let have_top_right = shape_ok
        && has_top_right(
            g.sb_mi_size,
            g.bw_px,
            g.bh_px,
            g.mi_row,
            g.mi_col,
            have_top,
            right_available,
            partition,
            txw_mi,
            g.row_off,
            g.col_off,
            g.ss,
            g.ss,
        );
    let have_bottom_left = shape_ok
        && has_bottom_left(
            g.sb_mi_size,
            g.bw_px,
            g.bh_px,
            g.mi_row,
            g.mi_col,
            bottom_available,
            have_left,
            partition,
            txh_mi,
            g.row_off,
            g.col_off,
            g.ss,
            g.ss,
        );

    let n_top_px = if have_top { txwpx.min(xr + txwpx) } else { 0 };
    let n_topright_px = if have_top_right { txwpx.min(xr) } else { 0 };
    let n_left_px = if have_left { txhpx.min(yd + txhpx) } else { 0 };
    let n_bottomleft_px = if have_bottom_left { txhpx.min(yd) } else { 0 };

    // dr-mode need flags (build_intra_predictors:67-76).
    let (need_above, need_left) = if p_angle <= 90 {
        (true, false)
    } else if p_angle < 180 {
        (true, true)
    } else {
        (false, true)
    };
    let need_right = p_angle < 90;
    let need_bottom = p_angle > 180;

    let above_ref = |i: i64| sample(((g.px as i64) + i) as usize, g.py - 1);
    let left_ref = |i: i64| sample(g.px - 1, ((g.py as i64) + i) as usize);

    // Early flat exit (:86-99).
    if (!need_above && n_left_px == 0) || (!need_left && n_top_px == 0) {
        let val = if need_left {
            if n_top_px > 0 { above_ref(0) } else { 129 }
        } else if n_left_px > 0 {
            left_ref(0)
        } else {
            127
        };
        for r in 0..g.txh {
            dst[r * g.txw..(r + 1) * g.txw].fill(val);
        }
        return;
    }

    let origin = ip::EDGE_ORIGIN;
    let mut above = [127u8; ip::EDGE_BUF_LEN];
    let mut left = [129u8; ip::EDGE_BUF_LEN];

    // NEED_LEFT (:100-131).
    if need_left {
        let num_left = (txhpx + if need_bottom { txwpx } else { 0 }) as usize;
        if n_left_px > 0 {
            for i in 0..n_left_px {
                left[origin + i as usize] = left_ref(i);
            }
            let mut i = n_left_px as usize;
            if need_bottom && n_bottomleft_px > 0 {
                debug_assert_eq!(i as i64, txhpx);
                while (i as i64) < txhpx + n_bottomleft_px {
                    left[origin + i] = left_ref(i as i64);
                    i += 1;
                }
            }
            if i < num_left {
                let v = left[origin + i - 1];
                left[origin + i..origin + num_left].fill(v);
            }
        } else if n_top_px > 0 {
            let v = above_ref(0);
            left[origin..origin + num_left].fill(v);
        }
    }

    // NEED_ABOVE (:133-161).
    if need_above {
        let num_top = (txwpx + if need_right { txhpx } else { 0 }) as usize;
        if n_top_px > 0 {
            for i in 0..n_top_px {
                above[origin + i as usize] = above_ref(i);
            }
            let mut i = n_top_px as usize;
            if need_right && n_topright_px > 0 {
                debug_assert_eq!(i as i64, txwpx);
                while (i as i64) < txwpx + n_topright_px {
                    above[origin + i] = above_ref(i as i64);
                    i += 1;
                }
            }
            if i < num_top {
                let v = above[origin + i - 1];
                above[origin + i..origin + num_top].fill(v);
            }
        } else if n_left_px > 0 {
            let v = left_ref(0);
            above[origin..origin + num_top].fill(v);
        }
    }

    // need_above_left (always set for dr modes, :163-176).
    let top_left = if n_top_px > 0 && n_left_px > 0 {
        sample(g.px - 1, g.py - 1)
    } else if n_top_px > 0 {
        above_ref(0)
    } else if n_left_px > 0 {
        left_ref(0)
    } else {
        128
    };
    above[origin - 1] = top_left;
    left[origin - 1] = top_left;

    // dr edge filter + upsample (:181-215).
    let mut upsample_above = false;
    let mut upsample_left = false;
    if edge_filter {
        if p_angle != 90 && p_angle != 180 {
            let ab_le = 1usize; // need_above_left
            if need_above && need_left && (txwpx + txhpx >= 24) {
                ip::filter_intra_edge_corner(&mut above, &mut left, origin);
            }
            if need_above && n_top_px > 0 {
                let strength = ip::intra_edge_filter_strength(
                    txwpx as i32,
                    txhpx as i32,
                    p_angle - 90,
                    filt_type,
                );
                let n_px = (n_top_px + ab_le as i64 + if need_right { txhpx } else { 0 }) as usize;
                ip::filter_intra_edge(&mut above, origin - ab_le, n_px, strength);
            }
            if need_left && n_left_px > 0 {
                let strength = ip::intra_edge_filter_strength(
                    txhpx as i32,
                    txwpx as i32,
                    p_angle - 180,
                    filt_type,
                );
                let n_px =
                    (n_left_px + ab_le as i64 + if need_bottom { txwpx } else { 0 }) as usize;
                ip::filter_intra_edge(&mut left, origin - ab_le, n_px, strength);
            }
        }
        upsample_above =
            ip::use_intra_edge_upsample(txwpx as i32, txhpx as i32, p_angle - 90, filt_type);
        if need_above && upsample_above {
            let n_px = (txwpx + if need_right { txhpx } else { 0 }) as usize;
            ip::upsample_intra_edge(&mut above, origin, n_px);
        }
        upsample_left =
            ip::use_intra_edge_upsample(txhpx as i32, txwpx as i32, p_angle - 180, filt_type);
        if need_left && upsample_left {
            let n_px = (txhpx + if need_bottom { txwpx } else { 0 }) as usize;
            ip::upsample_intra_edge(&mut left, origin, n_px);
        }
    }

    ip::dr_predictor_edged(
        dst,
        g.txw,
        &above,
        &left,
        origin,
        upsample_above,
        upsample_left,
        g.txw,
        g.txh,
        p_angle,
    );
}

/// High-bit-depth (u16) twin of [`dr_predict`] for the bd10 MD re-encode path
/// (task #94 follow-up). Byte-for-byte the same geometry / availability /
/// edge-array construction (all bit-depth-INDEPENDENT — see `dr_predict`),
/// swapping: the flat-fill defaults `{129,127,128}` for their bd-general forms
/// `{base+1, base-1, base}` with `base = 128 << (bd-8)` (C
/// `build_intra_predictors_high`, enc_intra_prediction.c:261-374); the u8 edge
/// filter / corner / upsample / dr kernels for the hbd siblings
/// (`svtav1_dsp::hbd::{filter_intra_edge_high, filter_intra_edge_corner_high,
/// upsample_intra_edge_high, dr_predictor_edged_hbd}`, all FFI-verified). The
/// filter-strength / use-upsample predicates take only dims+angle+filt_type, so
/// the u8 `ip::` versions are reused unchanged. ADDITIVE — the u8 `dr_predict`
/// is untouched, so the bd8 bitstream is byte-identical.
#[allow(clippy::too_many_arguments)]
pub fn dr_predict_hbd<S: Fn(usize, usize) -> u16>(
    sample: S,
    g: &DrGeom,
    p_angle: i32,
    edge_filter: bool,
    filt_type: i32,
    partition: PartitionType,
    dst: &mut [u16],
    bd: u8,
) {
    use svtav1_dsp::hbd as hp;
    use svtav1_dsp::intra_pred as ip;
    // `frame_w`/`frame_h` are the luma dims — TRUE (possibly odd, task #95)
    // OR 8-aligned; both round to the SAME aligned mi grid below because
    // `(d + 7) >> 3 == aligned(d) / 8` by definition, so `mi_cols`/`mi_rows`
    // equal C `av1_cm->mi_cols/rows` for either. (The old `% 8 == 0` assert
    // wrongly rejected the legitimate odd-true-dim input.)
    debug_assert!(g.frame_w > 0 && g.frame_h > 0);
    let base: i32 = 128 << (bd - 8);
    let mi_cols = 2 * ((g.frame_w + 7) >> 3);
    let mi_rows = 2 * ((g.frame_h + 7) >> 3);
    let txwpx = g.txw as i64;
    let txhpx = g.txh as i64;
    let wpx = (g.bw_px >> g.ss) as i64;
    let hpx = (g.bh_px >> g.ss) as i64;
    let x_off = (g.col_off * 4) as i64;
    let y_off = (g.row_off * 4) as i64;

    let have_top = g.row_off > 0 || g.mi_row > 0;
    let have_left = g.col_off > 0 || g.mi_col > 0;

    let xr = (((mi_cols * 4) as i64 - (g.mi_col * 4) as i64 - g.bw_px as i64) >> g.ss)
        + (wpx - x_off - txwpx);
    let yd = (((mi_rows * 4) as i64 - (g.mi_row * 4) as i64 - g.bh_px as i64) >> g.ss)
        + (hpx - y_off - txhpx);

    let txw_mi = g.txw / 4;
    let txh_mi = g.txh / 4;
    let right_available = g.mi_col + ((g.col_off + txw_mi) << g.ss) < mi_cols;
    let bottom_available = yd > 0 && g.mi_row + ((g.row_off + txh_mi) << g.ss) < mi_rows;

    let shape_ok = is_av1_block_shape(g.bw_px, g.bh_px);
    let have_top_right = shape_ok
        && has_top_right(
            g.sb_mi_size, g.bw_px, g.bh_px, g.mi_row, g.mi_col, have_top, right_available, partition,
            txw_mi, g.row_off, g.col_off, g.ss, g.ss,
        );
    let have_bottom_left = shape_ok
        && has_bottom_left(
            g.sb_mi_size, g.bw_px, g.bh_px, g.mi_row, g.mi_col, bottom_available, have_left,
            partition, txh_mi, g.row_off, g.col_off, g.ss, g.ss,
        );

    let n_top_px = if have_top { txwpx.min(xr + txwpx) } else { 0 };
    let n_topright_px = if have_top_right { txwpx.min(xr) } else { 0 };
    let n_left_px = if have_left { txhpx.min(yd + txhpx) } else { 0 };
    let n_bottomleft_px = if have_bottom_left { txhpx.min(yd) } else { 0 };

    let (need_above, need_left) = if p_angle <= 90 {
        (true, false)
    } else if p_angle < 180 {
        (true, true)
    } else {
        (false, true)
    };
    let need_right = p_angle < 90;
    let need_bottom = p_angle > 180;

    let above_ref = |i: i64| sample(((g.px as i64) + i) as usize, g.py - 1);
    let left_ref = |i: i64| sample(g.px - 1, ((g.py as i64) + i) as usize);

    // Early flat exit (build_intra_predictors_high:289-301).
    if (!need_above && n_left_px == 0) || (!need_left && n_top_px == 0) {
        let val = if need_left {
            if n_top_px > 0 { above_ref(0) } else { (base + 1) as u16 }
        } else if n_left_px > 0 {
            left_ref(0)
        } else {
            (base - 1) as u16
        };
        for r in 0..g.txh {
            dst[r * g.txw..(r + 1) * g.txw].fill(val);
        }
        return;
    }

    let origin = ip::EDGE_ORIGIN;
    let mut above = [(base - 1) as u16; ip::EDGE_BUF_LEN];
    let mut left = [(base + 1) as u16; ip::EDGE_BUF_LEN];

    // NEED_LEFT.
    if need_left {
        let num_left = (txhpx + if need_bottom { txwpx } else { 0 }) as usize;
        if n_left_px > 0 {
            for i in 0..n_left_px {
                left[origin + i as usize] = left_ref(i);
            }
            let mut i = n_left_px as usize;
            if need_bottom && n_bottomleft_px > 0 {
                debug_assert_eq!(i as i64, txhpx);
                while (i as i64) < txhpx + n_bottomleft_px {
                    left[origin + i] = left_ref(i as i64);
                    i += 1;
                }
            }
            if i < num_left {
                let v = left[origin + i - 1];
                left[origin + i..origin + num_left].fill(v);
            }
        } else if n_top_px > 0 {
            let v = above_ref(0);
            left[origin..origin + num_left].fill(v);
        }
    }

    // NEED_ABOVE.
    if need_above {
        let num_top = (txwpx + if need_right { txhpx } else { 0 }) as usize;
        if n_top_px > 0 {
            for i in 0..n_top_px {
                above[origin + i as usize] = above_ref(i);
            }
            let mut i = n_top_px as usize;
            if need_right && n_topright_px > 0 {
                debug_assert_eq!(i as i64, txwpx);
                while (i as i64) < txwpx + n_topright_px {
                    above[origin + i] = above_ref(i as i64);
                    i += 1;
                }
            }
            if i < num_top {
                let v = above[origin + i - 1];
                above[origin + i..origin + num_top].fill(v);
            }
        } else if n_left_px > 0 {
            let v = left_ref(0);
            above[origin..origin + num_top].fill(v);
        }
    }

    // need_above_left (always set for dr modes).
    let top_left = if n_top_px > 0 && n_left_px > 0 {
        sample(g.px - 1, g.py - 1)
    } else if n_top_px > 0 {
        above_ref(0)
    } else if n_left_px > 0 {
        left_ref(0)
    } else {
        base as u16
    };
    above[origin - 1] = top_left;
    left[origin - 1] = top_left;

    // dr edge filter + upsample (hbd kernels).
    let mut upsample_above = false;
    let mut upsample_left = false;
    if edge_filter {
        if p_angle != 90 && p_angle != 180 {
            let ab_le = 1usize; // need_above_left
            if need_above && need_left && (txwpx + txhpx >= 24) {
                hp::filter_intra_edge_corner_high(&mut above, &mut left, origin);
            }
            if need_above && n_top_px > 0 {
                let strength = ip::intra_edge_filter_strength(
                    txwpx as i32,
                    txhpx as i32,
                    p_angle - 90,
                    filt_type,
                );
                let n_px = (n_top_px + ab_le as i64 + if need_right { txhpx } else { 0 }) as usize;
                hp::filter_intra_edge_high(&mut above, origin - ab_le, n_px, strength);
            }
            if need_left && n_left_px > 0 {
                let strength = ip::intra_edge_filter_strength(
                    txhpx as i32,
                    txwpx as i32,
                    p_angle - 180,
                    filt_type,
                );
                let n_px =
                    (n_left_px + ab_le as i64 + if need_bottom { txwpx } else { 0 }) as usize;
                hp::filter_intra_edge_high(&mut left, origin - ab_le, n_px, strength);
            }
        }
        upsample_above =
            ip::use_intra_edge_upsample(txwpx as i32, txhpx as i32, p_angle - 90, filt_type);
        if need_above && upsample_above {
            let n_px = (txwpx + if need_right { txhpx } else { 0 }) as usize;
            hp::upsample_intra_edge_high(&mut above, origin, n_px, bd);
        }
        upsample_left =
            ip::use_intra_edge_upsample(txhpx as i32, txwpx as i32, p_angle - 180, filt_type);
        if need_left && upsample_left {
            let n_px = (txhpx + if need_bottom { txwpx } else { 0 }) as usize;
            hp::upsample_intra_edge_high(&mut left, origin, n_px, bd);
        }
    }

    hp::dr_predictor_edged_hbd(
        dst,
        g.txw,
        &above,
        &left,
        origin,
        upsample_above,
        upsample_left,
        g.txw,
        g.txh,
        p_angle,
        bd,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    /// 128x128 test frame with a recognizable per-pixel pattern.
    fn test_frame() -> (Vec<u8>, usize) {
        let stride = 128;
        let mut recon = alloc::vec![0u8; stride * 128];
        for y in 0..128usize {
            for x in 0..128usize {
                recon[y * stride + x] = ((y * 7 + x * 3) & 0xFF) as u8;
            }
        }
        (recon, stride)
    }

    const P_NONE: PartitionType = PartitionType::None;

    // ---- has_top_right (64x64 SB, whole-block tx, luma) ----

    #[test]
    fn has_tr_top_row_of_sb_available() {
        // 32x32 at mi(16, 8): top row of its SB → available.
        assert!(has_top_right(
            16, 32, 32, 16, 8, true, true, P_NONE, 8, 0, 0, 0, 0
        ));
    }

    #[test]
    fn has_tr_rightmost_col_of_sb_unavailable() {
        // 32x32 at mi(8, 8): bottom-right quadrant → rightmost column of
        // SB and not the top row → unavailable.
        assert!(!has_top_right(
            16, 32, 32, 8, 8, true, true, P_NONE, 8, 0, 0, 0, 0
        ));
    }

    #[test]
    fn has_tr_bottom_left_quadrant_from_table() {
        // 32x32 at mi(8, 0): bottom-left quadrant. TR pixels are the
        // bottom row of the top-right quadrant, already coded →
        // has_tr_32x32[0] bit 4 = 1 (95 = 0b0101_1111).
        assert!(has_top_right(
            16, 32, 32, 8, 0, true, true, P_NONE, 8, 0, 0, 0, 0
        ));
    }

    #[test]
    fn has_tr_unavailable_without_top_or_right() {
        assert!(!has_top_right(
            16, 32, 32, 8, 0, false, true, P_NONE, 8, 0, 0, 0, 0
        ));
        assert!(!has_top_right(
            16, 32, 32, 8, 0, true, false, P_NONE, 8, 0, 0, 0, 0
        ));
    }

    #[test]
    fn has_tr_16x16_interior() {
        // 16x16 at mi(4, 4): this_blk_index = (1 << 3) + 1 = 9 →
        // has_tr_16x16[1] = 85 = 0b0101_0101, bit 1 = 0 → unavailable.
        assert!(!has_top_right(
            16, 16, 16, 4, 4, true, true, P_NONE, 4, 0, 0, 0, 0
        ));
        // 16x16 at mi(4, 8): index = (1 << 3) + 2 = 10 → bit 2 of 85 = 1.
        assert!(has_top_right(
            16, 16, 16, 4, 8, true, true, P_NONE, 4, 0, 0, 0, 0
        ));
    }

    // ---- has_bottom_left ----

    #[test]
    fn has_bl_leftmost_col_within_sb() {
        // 16x16 at mi(4, 0): leftmost column of SB, BL pixels rows within
        // the SB → row_off_in_sb (4) + 4 < 16 → available.
        assert!(has_bottom_left(
            16, 16, 16, 4, 0, true, true, P_NONE, 4, 0, 0, 0, 0
        ));
        // 32x32 at mi(8, 0): 8 + 8 < 16 is false → unavailable.
        assert!(!has_bottom_left(
            16, 32, 32, 8, 0, true, true, P_NONE, 8, 0, 0, 0, 0
        ));
    }

    #[test]
    fn has_bl_16x16_top_right_area_from_table() {
        // 16x16 at mi(0, 8): index = (0 << 3) + 2 = 2 →
        // has_bl_16x16[0] = 84 = 0b0101_0100, bit 2 = 1 → available
        // (BL pixels are the TL 32x32 quadrant's right edge, coded first).
        assert!(has_bottom_left(
            16, 16, 16, 0, 8, true, true, P_NONE, 4, 0, 0, 0, 0
        ));
        // 16x16 at mi(4, 4): index = (1 << 3) + 1 = 9 →
        // has_bl_16x16[1] = 16 = 0b0001_0000, bit 1 = 0 → unavailable.
        assert!(!has_bottom_left(
            16, 16, 16, 4, 4, true, true, P_NONE, 4, 0, 0, 0, 0
        ));
    }

    #[test]
    fn has_bl_bottom_row_of_sb_unavailable() {
        // 16x16 at mi(12, 4): bottom row of SB, not leftmost col → 0.
        assert!(!has_bottom_left(
            16, 16, 16, 12, 4, true, true, P_NONE, 4, 0, 0, 0, 0
        ));
    }

    // ---- build_directional_edges ----

    #[test]
    fn edges_z1_real_top_right_when_available() {
        let (recon, stride) = test_frame();
        // 32x32 at (64, 64): mi(16, 16), top row of SB(1,1) → TR available;
        // xr = 128 - 96 = 32 → n_topright = 32 real pixels.
        match build_directional_edges(&recon, stride, 64, 64, 32, 32, 45, P_NONE, 16) {
            DirEdges::Edges { above, .. } => {
                for i in 0..64 {
                    assert_eq!(
                        above[i],
                        recon[63 * stride + 64 + i],
                        "above[{i}] must be the real reconstructed pixel"
                    );
                }
            }
            DirEdges::Flat(_) => panic!("expected edges"),
        }
    }

    #[test]
    fn edges_z1_replicates_when_top_right_unavailable() {
        let (recon, stride) = test_frame();
        // 32x32 at (96, 64): mi(16, 24) → rightmost column of SB(1,1),
        // not its top row? blk_row_in_sb = (16 & 15) >> 3 = 0 → TOP row →
        // available, but xr = 128 - 128 = 0 → right_available false →
        // has_top_right = 0 → replicate above[31].
        match build_directional_edges(&recon, stride, 96, 64, 32, 32, 45, P_NONE, 16) {
            DirEdges::Edges { above, .. } => {
                for i in 0..32 {
                    assert_eq!(above[i], recon[63 * stride + 96 + i]);
                }
                let last = recon[63 * stride + 127];
                for i in 32..64 {
                    assert_eq!(
                        above[i], last,
                        "above[{i}] must replicate the last real pixel"
                    );
                }
            }
            DirEdges::Flat(_) => panic!("expected edges"),
        }
    }

    #[test]
    fn edges_z1_no_top_flat_fill_from_left() {
        let (recon, stride) = test_frame();
        // 32x32 at (32, 0): no top row, angle 45 needs above only →
        // C early exit: flat fill with left_ref[0] = recon[0*128 + 31].
        match build_directional_edges(&recon, stride, 32, 0, 32, 32, 45, P_NONE, 16) {
            DirEdges::Flat(v) => assert_eq!(v, recon[31]),
            DirEdges::Edges { .. } => panic!("expected C's flat-fill early exit"),
        }
    }

    #[test]
    fn edges_frame_corner_defaults() {
        let (recon, stride) = test_frame();
        // Block at (0,0): z1 → flat 127; z3 → flat 129; z2 → 127/129/128.
        match build_directional_edges(&recon, stride, 0, 0, 32, 32, 45, P_NONE, 16) {
            DirEdges::Flat(v) => assert_eq!(v, 127),
            _ => panic!("expected flat"),
        }
        match build_directional_edges(&recon, stride, 0, 0, 32, 32, 203, P_NONE, 16) {
            DirEdges::Flat(v) => assert_eq!(v, 129),
            _ => panic!("expected flat"),
        }
        match build_directional_edges(&recon, stride, 0, 0, 32, 32, 135, P_NONE, 16) {
            DirEdges::Edges {
                above,
                left,
                top_left,
            } => {
                assert!(above.iter().all(|&v| v == 127));
                assert!(left.iter().all(|&v| v == 129));
                assert_eq!(top_left, 128);
            }
            DirEdges::Flat(_) => panic!("z2 needs both edges; no early exit"),
        }
    }

    #[test]
    fn edges_z3_real_bottom_left_when_available() {
        let (recon, stride) = test_frame();
        // 16x16 at (32, 0): mi(0, 8) → has_bl_16x16 bit 2 = 1, yd = 112 →
        // n_bottomleft = 16 real pixels below the block at col 31.
        match build_directional_edges(&recon, stride, 32, 0, 16, 16, 203, P_NONE, 16) {
            DirEdges::Edges { left, .. } => {
                for i in 0..32 {
                    assert_eq!(
                        left[i],
                        recon[i * stride + 31],
                        "left[{i}] must be the real reconstructed pixel"
                    );
                }
            }
            DirEdges::Flat(_) => panic!("expected edges"),
        }
    }

    #[test]
    fn edges_z3_replicates_when_bottom_left_unavailable() {
        let (recon, stride) = test_frame();
        // 16x16 at (16, 16): mi(4, 4) → has_bl bit 1 of 16 = 0 →
        // replicate left[15].
        match build_directional_edges(&recon, stride, 16, 16, 16, 16, 203, P_NONE, 16) {
            DirEdges::Edges { left, .. } => {
                for i in 0..16 {
                    assert_eq!(left[i], recon[(16 + i) * stride + 15]);
                }
                let last = recon[31 * stride + 15];
                for i in 16..32 {
                    assert_eq!(left[i], last);
                }
            }
            DirEdges::Flat(_) => panic!("expected edges"),
        }
    }

    #[test]
    fn edges_z2_no_top_fills_above_from_left() {
        let (recon, stride) = test_frame();
        // 16x16 at (16, 0): z2 (135) with no top row: above row filled
        // with left_ref[0]; top_left = left_ref[0].
        match build_directional_edges(&recon, stride, 16, 0, 16, 16, 135, P_NONE, 16) {
            DirEdges::Edges {
                above,
                left,
                top_left,
            } => {
                let lref0 = recon[15];
                // num_top = txwpx (n_topright = -1 for z2).
                for a in above.iter().take(16) {
                    assert_eq!(*a, lref0);
                }
                // Beyond num_top: C memset default 127.
                for a in above.iter().skip(16) {
                    assert_eq!(*a, 127);
                }
                for (i, l) in left.iter().enumerate().take(16) {
                    assert_eq!(*l, recon[i * stride + 15]);
                }
                assert_eq!(top_left, lref0);
            }
            DirEdges::Flat(_) => panic!("expected edges"),
        }
    }

    #[test]
    fn invalid_rdo_shape_does_not_panic() {
        let (recon, stride) = test_frame();
        // 8x64 is not an AV1 block size (RDO transient from 4:1 splits of
        // partial-SB areas). Must fall back to unavailable TR/BL, not panic.
        let e = build_directional_edges(&recon, stride, 8, 64, 8, 64, 45, P_NONE, 16);
        match e {
            DirEdges::Edges { above, .. } => {
                // n_topright = 0 → replication after the 8 real pixels.
                let last = recon[63 * stride + 15];
                for a in above.iter().take(72).skip(8) {
                    assert_eq!(*a, last);
                }
            }
            DirEdges::Flat(_) => panic!("expected edges"),
        }
    }

    /// `dr_predict_hbd` at bd=8 MUST reproduce the C-verified u8 `dr_predict`
    /// byte-for-byte on u8-range content (base = 128, hbd kernels clip to 255).
    /// This transitively verifies the hbd wrapper's edge-array construction and
    /// kernel wiring (the only bd10-specific parts are the base±1 constants —
    /// checked against C `build_intra_predictors_high` — and the bd param to the
    /// FFI-verified hbd kernels). The bd8 path itself is untouched (additive).
    #[test]
    fn dr_predict_hbd_bd8_matches_u8_dr_predict() {
        let (recon, stride) = test_frame();
        let recon16: Vec<u16> = recon.iter().map(|&p| p as u16).collect();
        // Directional angles: D45..D203 (modes 3..=8) and V/H (1,2) with a
        // nonzero delta — the cases that route through dr_predict.
        let cases: &[(u8, i8)] = &[
            (3, 0), (4, 0), (5, 0), (6, 0), (7, 0), (8, 0),
            (3, 2), (4, -2), (5, 3), (6, -3), (7, 1), (8, -1),
            (1, 1), (1, -2), (2, 2), (2, -3),
        ];
        // Interior, top-edge, left-edge and corner positions × square sizes.
        let blocks: &[(usize, usize, usize, usize)] = &[
            (64, 64, 32, 32), (72, 68, 8, 8), (80, 80, 16, 16),
            (96, 0, 16, 16), (0, 96, 16, 16), (0, 0, 8, 8),
            (64, 0, 32, 32), (0, 64, 32, 32), (16, 48, 16, 16),
        ];
        for &(px, py, txw, txh) in blocks {
            for &(mode, delta) in cases {
                let p_angle = MODE_TO_ANGLE_MAP[mode as usize] + delta as i32 * 3;
                let g = DrGeom {
                    px, py, txw, txh,
                    mi_row: py >> 2, mi_col: px >> 2,
                    bw_px: txw, bh_px: txh,
                    row_off: 0, col_off: 0, ss: 0,
                    frame_w: 128, frame_h: 128,
                    // 64px superblocks — this bd8-vs-bd10 equivalence test
                    // is about the predictor, not the SB geometry.
                    sb_mi_size: 16,
                    tile: TileMi::whole_frame(128, 128),
                };
                for &edge_filter in &[false, true] {
                    for &filt_type in &[0i32, 1] {
                        let mut d8 = alloc::vec![0u8; txw * txh];
                        dr_predict(
                            |x, y| recon[y * stride + x],
                            &g, p_angle, edge_filter, filt_type, P_NONE, &mut d8,
                        );
                        let mut d16 = alloc::vec![0u16; txw * txh];
                        dr_predict_hbd(
                            |x, y| recon16[y * stride + x],
                            &g, p_angle, edge_filter, filt_type, P_NONE, &mut d16, 8,
                        );
                        for (i, (&a, &b)) in d8.iter().zip(d16.iter()).enumerate() {
                            assert_eq!(
                                a as u16, b,
                                "mismatch px{px} py{py} {txw}x{txh} mode{mode} d{delta} \
                                 ef{edge_filter} ft{filt_type} idx{i}"
                            );
                        }
                    }
                }
            }
        }
    }
}
