//! The full-PD1 inter predictor's structure, its MD entry, and the two
//! drivers it hands off to.
//!
//! Ported from `Source/Lib/Codec/enc_inter_prediction.c` (SVT-AV1 v4.2.0):
//! `svt_aom_inter_prediction` (:3204), `svt_aom_inter_pu_prediction_av1`
//! (:3803), `inter_intra_prediction` (:2217) and `inter_chroma_4xn_pred`
//! (:3023).
//!
//! # What the inventory said, and what was there
//!
//! `svt_aom_inter_prediction` was marked ported on a SUBSTRING match — the
//! stem "inter_prediction" appears in file-name citations, and there were zero
//! real references in the Rust tree. This module is its structure.
//!
//! # Evidence
//!
//! TIER 4 for all four. `svt_aom_inter_prediction` alone takes a
//! `SequenceControlSet`, a `PictureControlSet`, two `WarpedMotionParams`, a
//! `BlkStruct`, a `ModeDecisionContext`, three `NeighborArrayUnit`s and three
//! `EbPictureBufferDesc`s. What is ported is the ORDER of operations, the
//! scratch layout and the per-plane / per-reference conditions — every leaf it
//! reaches is ported elsewhere in this crate, and the ones whose C symbol
//! exports are tier-1 gated there.

use crate::port_masked_compound::CompoundType;
use crate::port_obmc_build::round_uv;
use crate::port_obmc_pred::{COMPONENT_CHROMA, COMPONENT_LUMA};

/// `PICTURE_BUFFER_DESC_FULL_MASK`.
pub const FULL_MASK: u32 = COMPONENT_LUMA | COMPONENT_CHROMA;

/// The CONV_BUF scratch layout `svt_aom_inter_prediction` uses (:3218).
///
/// One 128x128 `uint16_t` buffer serves all three planes: luma at 0, Cb at 0
/// (reused AFTER luma, which is why C's comment says "process chroma after
/// luma to re-use buffer") and Cr at `64*64`. Predicting chroma before luma
/// would corrupt the luma CONV_BUF mid-compound.
pub const TMP_DST_Y_OFFSET: usize = 0;
/// See [`TMP_DST_Y_OFFSET`].
pub const TMP_DST_CB_OFFSET: usize = 0;
/// See [`TMP_DST_Y_OFFSET`].
pub const TMP_DST_CR_OFFSET: usize = 64 * 64;

/// `seg_mask` is computed for LUMA and reused for chroma (:3222), so the
/// DIFFWTD mask a chroma block blends with is the one luma derived.
pub const SEG_MASK_LEN: usize = 2 * 128 * 128;

/// `svt_aom_inter_pu_prediction_av1`'s component-mask derivation (:3824).
///
/// TRAP: the luma-skip optimisation is DISABLED for DIFFWTD compound. C's
/// comment says why — "the mask generation for DIFFWTD compound mode is done
/// for luma, using luma samples, so must always perform luma prediction if
/// DIFFWTD is used". Skipping luma there leaves `seg_mask` stale and the
/// chroma blend reads the previous block's mask.
pub fn pu_component_mask(
    mds_do_chroma: bool,
    md_stage_at_least_1: bool,
    valid_luma_pred: bool,
    need_hbd_comp_mds3: bool,
    is_compound: bool,
    compound_type: CompoundType,
) -> u32 {
    let mut component_mask = if mds_do_chroma {
        FULL_MASK
    } else {
        COMPONENT_LUMA
    };
    if md_stage_at_least_1
        && valid_luma_pred
        && component_mask == FULL_MASK
        && !need_hbd_comp_mds3
        && (!is_compound || compound_type != CompoundType::DiffWtd)
    {
        component_mask = COMPONENT_CHROMA;
    }
    component_mask
}

/// Whether `svt_aom_inter_pu_prediction_av1` runs the interpolation-filter
/// search (:3818).
///
/// All four conditions must hold; note IntraBC is excluded because it always
/// uses BILINEAR.
pub fn runs_ifs(
    mds_do_ifs: bool,
    interpolation_filter_is_switchable: bool,
    use_intrabc: bool,
    interp_needed: bool,
) -> bool {
    mds_do_ifs && interpolation_filter_is_switchable && !use_intrabc && interp_needed
}

/// `svt_aom_inter_prediction`'s two post-passes, in C's order (:3487, :3510).
///
/// Inter-intra runs FIRST, then OBMC — and OBMC asserts `is_compound == 0` and
/// `bwidth > 4 && bheight > 4`. Running them the other way round would blend
/// the intra predictor into an OBMC result rather than the reverse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PostPasses {
    /// `block_mi->is_interintra_used`.
    pub inter_intra: bool,
    /// `block_mi->motion_mode == OBMC_CAUSAL`.
    pub obmc: bool,
}

/// The order the two post-passes run in.
pub fn post_pass_order(p: PostPasses) -> alloc::vec::Vec<&'static str> {
    let mut out = alloc::vec::Vec::new();
    if p.inter_intra {
        out.push("inter_intra_prediction");
    }
    if p.obmc {
        out.push("av1_inter_prediction_obmc");
    }
    out
}

/// `is_masked_compound` as `svt_aom_inter_prediction` passes it per reference
/// (:3480).
///
/// TRAP: it is `ref_itr && is_masked_compound_type(...)` — only the SECOND
/// reference's pass is masked. The first writes the plain CONV_BUF; the mask
/// is applied when the second folds in.
pub fn per_ref_is_masked(ref_itr: usize, compound_type: CompoundType) -> bool {
    ref_itr != 0 && crate::port_masked_compound::is_masked_compound_type(compound_type)
}

/// `inter_intra_prediction`'s plane range (:2224).
///
/// `end_plane = chroma ? MAX_PLANES : 1` — THREE planes, unlike the OBMC BUILD
/// path's two. See `port_obmc_build::build_plane_range` for that contrast.
pub fn inter_intra_plane_range(component_mask: u32) -> (usize, usize) {
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

/// `inter_intra_prediction`'s intra-predictor scratch (:2237).
///
/// One `MAX_SB_SQUARE` buffer serves luma and chroma in turn (C: "luma/chroma
/// computed separately, so can re-use buffer"), with the Y stride `bwidth` and
/// both chroma strides `bwidth >> 1` — note the CHROMA strides come from the
/// LUMA width, not from `block_size_wide[plane_bsize]`.
pub fn intra_pred_strides(bwidth: usize) -> (usize, usize, usize) {
    (bwidth, bwidth >> 1, bwidth >> 1)
}

/// The chroma neighbour origins `inter_intra_prediction` reads (:2256).
pub fn ii_chroma_origins(pu_origin_x: u32, pu_origin_y: u32) -> (u32, u32) {
    (round_uv(pu_origin_x) >> 1, round_uv(pu_origin_y) >> 1)
}

/// The left-neighbour run length `inter_intra_prediction` copies (:2273).
///
/// TRAP: the multiplier is 1 when the block's bottom half would run past the
/// superblock, and 2 otherwise — i.e. the doubled left column is only
/// available when it exists. Using 2 unconditionally reads past the neighbour
/// array at the bottom SB row.
pub fn ii_left_multiplier(pu_origin_y: usize, bheight: usize, sb_size_luma: usize) -> usize {
    if (pu_origin_y % sb_size_luma + bheight * 2) > sb_size_luma {
        1
    } else {
        2
    }
}

/// `inter_chroma_4xn_pred`'s gate (:3048).
///
/// A sub-8x8 chroma block covers more than one luma block, so its prediction
/// must use the NEIGHBOURING luma blocks' MVs rather than this block's. The
/// gate is `(bw < 8 && ss_x) || (bh < 8 && ss_y)` — and IntraBC opts out.
pub fn sub8x8_inter(
    bwidth: usize,
    bheight: usize,
    ss_x: usize,
    ss_y: usize,
    use_intrabc: bool,
) -> bool {
    if use_intrabc {
        return false;
    }
    (bwidth < 8 && ss_x != 0) || (bheight < 8 && ss_y != 0)
}

/// The neighbour window `inter_chroma_4xn_pred` walks (:3055).
///
/// `row_start` is -1 only when the block is 4 tall AND chroma is vertically
/// subsampled; `col_start` is the width twin. Both are 0 otherwise, which
/// makes the walk a single cell.
pub fn sub8x8_window(bwidth: usize, bheight: usize, ss_x: usize, ss_y: usize) -> (i32, i32) {
    let row_start = if bheight == 4 && ss_y != 0 { -1 } else { 0 };
    let col_start = if bwidth == 4 && ss_x != 0 { -1 } else { 0 };
    (row_start, col_start)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The luma-skip optimisation is disabled for DIFFWTD compound.
    #[test]
    fn diffwtd_compound_always_predicts_luma() {
        // Non-compound: luma may be skipped.
        assert_eq!(
            pu_component_mask(true, true, true, false, false, CompoundType::Average),
            COMPONENT_CHROMA
        );
        // Compound WEDGE: still skippable.
        assert_eq!(
            pu_component_mask(true, true, true, false, true, CompoundType::Wedge),
            COMPONENT_CHROMA
        );
        // Compound DIFFWTD: luma must run.
        assert_eq!(
            pu_component_mask(true, true, true, false, true, CompoundType::DiffWtd),
            FULL_MASK
        );
        // need_hbd_comp_mds3 also forces the full mask.
        assert_eq!(
            pu_component_mask(true, true, true, true, false, CompoundType::Average),
            FULL_MASK
        );
        // No chroma requested -> luma only, regardless.
        assert_eq!(
            pu_component_mask(false, true, true, false, false, CompoundType::Average),
            COMPONENT_LUMA
        );
    }

    /// Only the SECOND reference's pass is masked.
    #[test]
    fn only_the_second_reference_is_masked() {
        assert!(!per_ref_is_masked(0, CompoundType::Wedge));
        assert!(per_ref_is_masked(1, CompoundType::Wedge));
        assert!(per_ref_is_masked(1, CompoundType::DiffWtd));
        assert!(!per_ref_is_masked(1, CompoundType::Average));
    }

    /// Inter-intra runs before OBMC.
    #[test]
    fn inter_intra_runs_before_obmc() {
        assert_eq!(
            post_pass_order(PostPasses {
                inter_intra: true,
                obmc: true
            }),
            alloc::vec!["inter_intra_prediction", "av1_inter_prediction_obmc"]
        );
    }

    /// Inter-intra walks THREE planes where the OBMC build path walks two.
    #[test]
    fn inter_intra_plane_range_is_three() {
        assert_eq!(inter_intra_plane_range(FULL_MASK), (0, 3));
        assert_eq!(
            crate::port_obmc_build::build_plane_range(FULL_MASK),
            (0, 2),
            "the OBMC build path stops at 2; these must not be conflated"
        );
    }

    /// The intra scratch's chroma strides come from the LUMA width.
    #[test]
    fn intra_scratch_chroma_stride_is_half_the_luma_width() {
        assert_eq!(intra_pred_strides(32), (32, 16, 16));
        assert_eq!(intra_pred_strides(8), (8, 4, 4));
    }

    /// The left-neighbour multiplier drops to 1 at the bottom of the SB.
    #[test]
    fn left_multiplier_drops_at_the_sb_edge() {
        // 16-tall block at y=0 in a 64 SB: 0 + 32 <= 64, so 2.
        assert_eq!(ii_left_multiplier(0, 16, 64), 2);
        // Same block at y=48: 48 + 32 > 64, so 1.
        assert_eq!(ii_left_multiplier(48, 16, 64), 1);
        // Exactly at the boundary: 32 + 32 == 64 is NOT greater, so 2.
        assert_eq!(ii_left_multiplier(32, 16, 64), 2);
    }

    /// The sub-8x8 chroma gate needs BOTH a small dimension and subsampling on
    /// that axis, and IntraBC opts out entirely.
    #[test]
    fn sub8x8_gate_needs_subsampling_on_the_small_axis() {
        assert!(sub8x8_inter(4, 16, 1, 1, false));
        assert!(sub8x8_inter(16, 4, 1, 1, false));
        // 4:4:4 chroma is not subsampled, so no sub-8x8 case.
        assert!(!sub8x8_inter(4, 16, 0, 0, false));
        // 8x8 and larger never qualifies.
        assert!(!sub8x8_inter(8, 8, 1, 1, false));
        // IntraBC opts out.
        assert!(!sub8x8_inter(4, 16, 1, 1, true));
    }

    /// The neighbour window extends only on the axis that is 4 AND subsampled.
    #[test]
    fn sub8x8_window_extends_only_the_small_axis() {
        assert_eq!(sub8x8_window(4, 16, 1, 1), (0, -1));
        assert_eq!(sub8x8_window(16, 4, 1, 1), (-1, 0));
        assert_eq!(sub8x8_window(4, 4, 1, 1), (-1, -1));
        assert_eq!(sub8x8_window(4, 4, 0, 0), (0, 0));
    }
}
