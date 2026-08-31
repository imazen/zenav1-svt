//! The switchable interpolation-filter group.
//!
//! C reference: `Source/Lib/Codec/entropy_coding.c`
//! (`svt_aom_get_ref_filter_type` :1519,
//! `svt_aom_get_pred_context_switchable_interp` :1527,
//! `svt_aom_is_nontrans_global_motion` :1572, `av1_is_interp_needed` :1592,
//! `write_mb_interp_filter` :1608).
//!
//! All four gates matter for the same reason: a wrong answer changes how many
//! symbols the block emits, which desyncs the tile rather than merely costing
//! bits.

use crate::entropy::writer::AomWriter;
use crate::port_entropy_inter::modes::{MotionMode, TransformationType, is_inter_compound_mode};
use crate::port_entropy_inter::refframe::INTRA_FRAME;
use crate::port_entropy_inter::{InterCdfs, NeighborMi, Neighbors};
use svtav1_types::block::BlockSize;
use svtav1_types::tables::block::{NUM_4X4_BLOCKS_HIGH, NUM_4X4_BLOCKS_WIDE};

/// C `SWITCHABLE_FILTERS` == `BILINEAR` == 3 (definitions.h:845).
pub const SWITCHABLE_FILTERS: usize = 3;
/// C `SWITCHABLE` == 4 — the frame-header value meaning "coded per block".
pub const SWITCHABLE: u8 = 4;
/// C `INTER_FILTER_COMP_OFFSET` (filter.h:74).
pub const INTER_FILTER_COMP_OFFSET: usize = SWITCHABLE_FILTERS + 1;
/// C `INTER_FILTER_DIR_OFFSET` (filter.h:75).
pub const INTER_FILTER_DIR_OFFSET: usize = (SWITCHABLE_FILTERS + 1) * 2;
/// C `SWITCHABLE_FILTER_CONTEXTS` (definitions.h:349).
pub const SWITCHABLE_FILTER_CONTEXTS: usize = (SWITCHABLE_FILTERS + 1) * 4;

/// C `av1_extract_interp_filter` (filter.h:60) — the packed pair is
/// `y | (x << 16)`, and the argument selects the HIGH half when nonzero.
#[inline]
pub const fn extract_interp_filter(filters: u32, x_filter: i32) -> u8 {
    ((filters >> (if x_filter != 0 { 16 } else { 0 })) & 0xffff) as u8
}

/// C `svt_aom_get_ref_filter_type` (entropy_coding.c:1519).
///
/// The neighbour contributes its own filter only when it actually references
/// `ref_frame` in EITHER slot; otherwise it reads as `SWITCHABLE_FILTERS`
/// (the "no opinion" value). Note `dir & 0x01` — direction 2/3 would fold
/// onto 0/1, though C only ever passes 0 or 1.
#[inline]
pub fn get_ref_filter_type(mi: &NeighborMi, dir: i32, ref_frame: i8) -> usize {
    if mi.ref_frame[0] == ref_frame || mi.ref_frame[1] == ref_frame {
        extract_interp_filter(mi.interp_filters, dir & 0x01) as usize
    } else {
        SWITCHABLE_FILTERS
    }
}

/// C `svt_aom_get_pred_context_switchable_interp` (entropy_coding.c:1527).
///
/// C reads `xd->mi[-1]` (left) and `xd->mi[-xd->mi_stride]` (above) gated on
/// `left_available` / `up_available` — the mi GRID, not the
/// `above_mbmi`/`left_mbmi` pointers, though in SVT they hold the same
/// blocks. It also takes `rf0`/`rf1` as parameters rather than reading the
/// current block, because MD calls it before `mbmi` is updated.
pub fn pred_context_switchable_interp(rf0: i8, rf1: i8, nb: &Neighbors, dir: i32) -> usize {
    let ctx_offset = usize::from(rf1 > INTRA_FRAME) * INTER_FILTER_COMP_OFFSET;
    debug_assert!(dir == 0 || dir == 1);
    let mut filter_type_ctx = ctx_offset + ((dir & 0x01) as usize) * INTER_FILTER_DIR_OFFSET;

    let left_type = nb
        .left_avail()
        .map(|m| get_ref_filter_type(m, dir, rf0))
        .unwrap_or(SWITCHABLE_FILTERS);
    let above_type = nb
        .above_avail()
        .map(|m| get_ref_filter_type(m, dir, rf0))
        .unwrap_or(SWITCHABLE_FILTERS);

    if left_type == above_type {
        filter_type_ctx += left_type;
    } else if left_type == SWITCHABLE_FILTERS {
        filter_type_ctx += above_type;
    } else if above_type == SWITCHABLE_FILTERS {
        filter_type_ctx += left_type;
    } else {
        filter_type_ctx += SWITCHABLE_FILTERS;
    }
    filter_type_ctx
}

/// C `svt_aom_is_nontrans_global_motion` (entropy_coding.c:1572).
///
/// The compound test is `is_inter_compound_mode(mode)` — the MODE, not
/// `has_second_ref` — so the second reference is inspected exactly when the
/// mode says compound.
pub fn is_nontrans_global_motion(
    mode: u8,
    bsize: BlockSize,
    ref_frame: [i8; 2],
    gm_wmtype: &[TransformationType; 8],
) -> bool {
    use crate::port_entropy_inter::modes::{GLOBAL_GLOBALMV, GLOBALMV};
    if mode != GLOBALMV && mode != GLOBAL_GLOBALMV {
        return false;
    }
    let i = bsize.as_index();
    if NUM_4X4_BLOCKS_WIDE[i].min(NUM_4X4_BLOCKS_HIGH[i]) < 2 {
        return false;
    }
    let is_compound = usize::from(is_inter_compound_mode(mode));
    for r in 0..=is_compound {
        if gm_wmtype[ref_frame[r].clamp(0, 7) as usize] == TransformationType::Translation {
            return false;
        }
    }
    true
}

/// C `av1_is_interp_needed` (entropy_coding.c:1592) — the three early-outs
/// that suppress the interp-filter symbols entirely.
pub fn is_interp_needed(
    skip_mode: bool,
    motion_mode: MotionMode,
    mode: u8,
    bsize: BlockSize,
    ref_frame: [i8; 2],
    gm_wmtype: &[TransformationType; 8],
) -> bool {
    if skip_mode {
        return false;
    }
    if motion_mode == MotionMode::WarpedCausal {
        return false;
    }
    if is_nontrans_global_motion(mode, bsize, ref_frame, gm_wmtype) {
        return false;
    }
    true
}

/// C `write_mb_interp_filter` (entropy_coding.c:1608).
///
/// Emits one symbol per direction, `enable_dual_filter ? 2 : 1` of them.
/// `interpolation_filter` is the FRAME-header value: anything but
/// `SWITCHABLE` (4) codes nothing.
#[allow(clippy::too_many_arguments)]
pub fn write_mb_interp_filter(
    w: &mut AomWriter,
    ic: &mut InterCdfs,
    nb: &Neighbors,
    interpolation_filter: u8,
    enable_dual_filter: bool,
    bsize: BlockSize,
    rf0: i8,
    rf1: i8,
    mode: u8,
    skip_mode: bool,
    motion_mode: MotionMode,
    interp_filters: u32,
    gm_wmtype: &[TransformationType; 8],
) {
    if interpolation_filter != SWITCHABLE
        || !is_interp_needed(skip_mode, motion_mode, mode, bsize, [rf0, rf1], gm_wmtype)
    {
        return;
    }
    let max_dir = if enable_dual_filter { 2 } else { 1 };
    for dir in 0..max_dir {
        let ctx = pred_context_switchable_interp(rf0, rf1, nb, dir);
        debug_assert!(ctx < SWITCHABLE_FILTER_CONTEXTS);
        let filter = extract_interp_filter(interp_filters, dir) as usize;
        debug_assert!(filter < SWITCHABLE_FILTERS);
        w.write_symbol(
            filter,
            &mut ic.switchable_interp_cdf[ctx],
            SWITCHABLE_FILTERS,
        );
    }
}
