//! `svt_aom_enc_make_inter_predictor`'s dispatch, and the two masked
//! predictors it can reach.
//!
//! Ported from `Source/Lib/Codec/enc_inter_prediction.c` (SVT-AV1 v4.2.0):
//! `svt_aom_enc_make_inter_predictor` (:2515),
//! `av1_make_masked_warp_inter_predictor` (:1633) and
//! `av1_make_masked_scaled_inter_predictor` (:77).
//!
//! # What was already "ported", and what actually was not
//!
//! The inventory called `svt_aom_enc_make_inter_predictor` ported. What
//! existed was `svtav1-encoder/src/intrabc_pred.rs` — the IntraBC arm, with
//! whole-pel and half-pel bilinear closed forms. The INTER arm (a real MV, the
//! 8-tap filters, compound `conv_params`, and the warp / masked branches) had
//! no counterpart. This module is that arm's DISPATCH; the leaves it reaches
//! are already ported: [`crate::port_subpel_params::compute_subpel_params`],
//! [`crate::port_inter_predictor::inter_predictor`] /
//! `highbd_inter_predictor`, and `svtav1-dsp/src/warp.rs`
//! (`c_parity_warp.rs`).
//!
//! # Evidence
//!
//! TIER 4 for the dispatch and the two masked predictors. All three take a
//! `SequenceControlSet`, a `MacroBlockD` and raw plane pointers whose layout
//! depends on the encoder's packed-buffer convention; a shim cannot synthesise
//! them. What is ported is the branch structure and the pointer arithmetic,
//! which is where a re-derivation goes wrong.

use crate::port_scale_factors::ScaleFactors;

/// `INTERPOLATION_OFFSET` (definitions.h:365).
pub const INTERPOLATION_OFFSET: usize = 8;

/// Which leaf `svt_aom_enc_make_inter_predictor` dispatches to.
///
/// The order of the tests matters: `is_wm` is checked FIRST and short-circuits
/// everything else, then `is_masked_compound` inside each arm. So a warped
/// masked-compound block takes [`Self::MaskedWarp`] and never reaches
/// [`Self::MaskedScaled`], even though both names say "masked".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MakePredLeaf {
    /// `av1_make_masked_warp_inter_predictor` — `is_wm && is_masked_compound`.
    MaskedWarp,
    /// `svt_av1_warp_plane` — `is_wm && !is_masked_compound`.
    Warp,
    /// `av1_make_masked_scaled_inter_predictor` —
    /// `!is_wm && is_masked_compound`.
    MaskedScaled,
    /// `svt_highbd_inter_predictor` — `!is_wm && !is_masked_compound && is16bit`.
    HighbdInter,
    /// `svt_inter_predictor` — the plain 8-bit arm.
    Inter,
}

/// `svt_aom_enc_make_inter_predictor`'s branch (:2521, :2590, :2612).
///
/// TRAP: both masked arms set `conv_params->do_average = 0` BEFORE calling the
/// masked predictor — the mask replaces the average, so leaving `do_average`
/// set double-blends. [`masked_arm_clears_do_average`] is that fact.
pub fn make_pred_leaf(is_wm: bool, is_masked_compound: bool, is16bit: bool) -> MakePredLeaf {
    match (is_wm, is_masked_compound, is16bit) {
        (true, true, _) => MakePredLeaf::MaskedWarp,
        (true, false, _) => MakePredLeaf::Warp,
        (false, true, _) => MakePredLeaf::MaskedScaled,
        (false, false, true) => MakePredLeaf::HighbdInter,
        (false, false, false) => MakePredLeaf::Inter,
    }
}

/// Whether the chosen leaf clears `do_average` first.
pub fn masked_arm_clears_do_average(leaf: MakePredLeaf) -> bool {
    matches!(leaf, MakePredLeaf::MaskedWarp | MakePredLeaf::MaskedScaled)
}

/// The warp arm's plane extents (:2551): `frame_width >> ss_x` and
/// `frame_height >> ss_y` — the FRAME dimensions subsampled, not the block's.
pub fn warp_plane_extents(
    frame_width: usize,
    frame_height: usize,
    ss_x: usize,
    ss_y: usize,
) -> (usize, usize) {
    (frame_width >> ss_x, frame_height >> ss_y)
}

/// The source pointer offset the non-warp arm computes (:2583).
///
/// TRAP: the `<< is16bit` is applied ONLY when there is no `src_ptr_2b`. With
/// a packed 2-bit plane present, `src_ptr` is an 8-bit MSB plane and the
/// offset is in SAMPLES; without one, `src_ptr` is already the 16-bit plane's
/// bytes and the offset must be doubled. Getting this backwards halves or
/// doubles every 10-bit source address.
pub fn src_offset(
    pos_x: i32,
    pos_y: i32,
    src_stride: usize,
    has_2b_plane: bool,
    is16bit: bool,
) -> isize {
    let linear = pos_x as isize + pos_y as isize * src_stride as isize;
    if has_2b_plane {
        linear
    } else {
        linear * (1 << usize::from(is16bit)) as isize
    }
}

/// The packed-buffer geometry the 10-bit arm builds (:2617) when a 2-bit plane
/// is present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackedSrcGeom {
    /// `src_stride16`, after the 8-alignment.
    pub stride: usize,
    /// Width passed to `svt_aom_pack_block`.
    pub width: usize,
    /// Height passed to `svt_aom_pack_block`.
    pub height: usize,
    /// Offset of the block origin inside the packed scratch.
    pub origin: usize,
}

/// `svt_aom_enc_make_inter_predictor`'s packed-buffer sizing (:2617-2640).
///
/// The scale factors widen the packed area: 2x per axis that is scaled,
/// because a superres or reference-scaled block reads up to twice as far. The
/// stride is then rounded UP to a multiple of 8 — C writes
/// `if (src_stride16 % 8) src_stride16 = ALIGN_POWER_OF_TWO(src_stride16, 3)`,
/// which is the same as rounding up unconditionally, but only because the
/// guard makes the already-aligned case a no-op.
pub fn packed_src_geom(blk_width: usize, blk_height: usize, sf: &ScaleFactors) -> PackedSrcGeom {
    let offset = INTERPOLATION_OFFSET;
    let (mut width_scale, mut height_scale) = (1usize, 1usize);
    if sf.is_scaled() {
        width_scale = if sf.x_scale_fp != crate::port_scale_factors::REF_NO_SCALE {
            2
        } else {
            1
        };
        height_scale = if sf.y_scale_fp != crate::port_scale_factors::REF_NO_SCALE {
            2
        } else {
            1
        };
    }
    let mut stride = blk_width * width_scale + (offset << 1);
    if !stride.is_multiple_of(8) {
        stride = stride.next_multiple_of(8);
    }
    PackedSrcGeom {
        stride,
        width: blk_width * width_scale + (offset << 1),
        height: blk_height * height_scale + (offset << 1),
        origin: offset + offset * stride,
    }
}

/// The two masked predictors' shared shape
/// (`av1_make_masked_warp_inter_predictor` :1633,
/// `av1_make_masked_scaled_inter_predictor` :77).
///
/// Both do the same three things: redirect `conv_params->dst` at a private
/// `MAX_SB_SIZE`-stride scratch, run the ORDINARY predictor for the second
/// reference into it, then blend that scratch with the first reference's
/// CONV_BUF through [`crate::port_masked_blend::build_masked_compound_no_round`]
/// — and finally NULL `conv_params->dst` "to avoid misuse" (:1707).
///
/// `assert(conv_params->do_average == 0)` holds because
/// `svt_aom_enc_make_inter_predictor` cleared it on the way in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaskedPredScratch {
    /// `tmp_buf_stride` — `MAX_SB_SIZE`.
    pub stride: usize,
    /// The saved `conv_params->dst` stride, restored implicitly by the caller
    /// keeping its own copy.
    pub saved_dst_stride: usize,
}

impl MaskedPredScratch {
    /// `MAX_SB_SIZE`.
    pub const MAX_SB_SIZE: usize = 128;

    /// Redirect the CONV_BUF at the private scratch.
    pub fn redirect(saved_dst_stride: usize) -> Self {
        Self {
            stride: Self::MAX_SB_SIZE,
            saved_dst_stride,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `is_wm` is tested FIRST: a warped masked-compound block takes the
    /// masked WARP leaf, never the masked SCALED one.
    #[test]
    fn warp_is_tested_before_masked() {
        assert_eq!(make_pred_leaf(true, true, false), MakePredLeaf::MaskedWarp);
        assert_eq!(make_pred_leaf(true, true, true), MakePredLeaf::MaskedWarp);
        assert_eq!(make_pred_leaf(true, false, true), MakePredLeaf::Warp);
        assert_eq!(
            make_pred_leaf(false, true, true),
            MakePredLeaf::MaskedScaled
        );
        assert_eq!(
            make_pred_leaf(false, false, true),
            MakePredLeaf::HighbdInter
        );
        assert_eq!(make_pred_leaf(false, false, false), MakePredLeaf::Inter);
    }

    /// Both masked arms clear `do_average`; no other arm does.
    #[test]
    fn only_the_masked_arms_clear_do_average() {
        for (wm, mc) in [(true, true), (false, true)] {
            assert!(masked_arm_clears_do_average(make_pred_leaf(wm, mc, false)));
        }
        for (wm, mc) in [(true, false), (false, false)] {
            assert!(!masked_arm_clears_do_average(make_pred_leaf(wm, mc, false)));
        }
    }

    /// The `<< is16bit` applies ONLY without a packed 2-bit plane.
    #[test]
    fn src_offset_depends_on_the_packed_plane() {
        assert_eq!(src_offset(3, 2, 64, false, false), 3 + 128);
        assert_eq!(src_offset(3, 2, 64, false, true), (3 + 128) * 2);
        // With a 2-bit plane the offset stays in samples at 10 bits.
        assert_eq!(src_offset(3, 2, 64, true, true), 3 + 128);
    }

    /// The packed scratch widens 2x per SCALED axis and its stride is
    /// 8-aligned.
    #[test]
    fn packed_geometry_widens_per_scaled_axis() {
        let unscaled = ScaleFactors::setup_for_frame(64, 64, 64, 64);
        let g = packed_src_geom(16, 16, &unscaled);
        assert_eq!(g.width, 16 + 16);
        assert_eq!(g.height, 16 + 16);
        assert_eq!(g.stride % 8, 0);

        // x scaled only: width doubles, height does not.
        let x_only = ScaleFactors::setup_for_frame(128, 64, 64, 64);
        assert!(x_only.is_scaled());
        let g = packed_src_geom(16, 16, &x_only);
        assert_eq!(g.width, 32 + 16);
        assert_eq!(g.height, 16 + 16);

        // Both axes scaled.
        let both = ScaleFactors::setup_for_frame(128, 128, 64, 64);
        let g = packed_src_geom(16, 16, &both);
        assert_eq!(g.width, 32 + 16);
        assert_eq!(g.height, 32 + 16);

        // A stride that is not a multiple of 8 is rounded UP.
        let g = packed_src_geom(20, 20, &unscaled);
        assert_eq!(g.stride, 40);
        let g = packed_src_geom(21, 21, &unscaled);
        assert_eq!(g.stride, 40); // 37 -> 40
        assert_eq!(g.origin, INTERPOLATION_OFFSET + INTERPOLATION_OFFSET * 40);
    }

    /// The masked predictors' private scratch is MAX_SB_SIZE-strided.
    #[test]
    fn masked_scratch_stride() {
        let s = MaskedPredScratch::redirect(64);
        assert_eq!(s.stride, 128);
        assert_eq!(s.saved_dst_stride, 64);
    }
}
