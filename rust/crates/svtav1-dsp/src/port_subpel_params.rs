//! Turning a motion vector into `SubpelParams` plus a reference position.
//!
//! Ported from `Source/Lib/Codec/enc_inter_prediction.c` (SVT-AV1 v4.2.0):
//! `clamp_mv_to_umv_border_sb` (:55) and `compute_subpel_params` (:2400);
//! plus `clamp_mv` (mv.h:70).
//!
//! # What was already "ported", and what actually was not
//!
//! `svtav1-encoder/src/intrabc_pred.rs` carries the IntraBC `ss = 1` chroma arm
//! of `compute_subpel_params` and a comment (`:7`) explaining why
//! `clamp_mv_to_umv_border_sb` CANNOT bind for a display vector. For a real
//! inter MV it binds constantly — MVs point outside the frame at every border
//! block — and it clamps the MV that MC then uses, so a wrong clamp is wrong
//! pixels on every frame-border block. That arm, and the scaled arm, are what
//! this module adds.
//!
//! # The two arms are not variations of each other
//!
//! The unscaled arm clamps the MV against the block's UMV border and derives
//! the phase from the CLAMPED MV. The scaled arm does not clamp the MV at all:
//! it maps the position through `sf->scale_value_{x,y}`, adds `SCALE_EXTRA_OFF`,
//! and clamps the resulting POSITION against the reference's padded extent.
//! Different quantity, different units, different clamp.

use crate::port_scale_factors::{
    SCALE_EXTRA_BITS, SCALE_SUBPEL_BITS, SCALE_SUBPEL_SHIFTS, ScaleFactors, SubpelParams,
};

/// `AOM_INTERP_EXTEND` (definitions.h:77).
pub const AOM_INTERP_EXTEND: i32 = 4;
/// `INTERPOLATION_OFFSET` (definitions.h:365).
pub const INTERPOLATION_OFFSET: i32 = 8;
/// `SUBPEL_BITS` (definitions.h:457).
pub const SUBPEL_BITS: i32 = 4;
/// `SUBPEL_SHIFTS` (definitions.h:459).
pub const SUBPEL_SHIFTS: i32 = 1 << SUBPEL_BITS;
/// `SUBPEL_MASK` (definitions.h:458).
pub const SUBPEL_MASK: i32 = SUBPEL_SHIFTS - 1;
/// `SCALE_SUBPEL_MASK` (definitions.h:464).
pub const SCALE_SUBPEL_MASK: i32 = SCALE_SUBPEL_SHIFTS - 1;
/// `SCALE_EXTRA_OFF` (definitions.h:466) — `(1 << SCALE_EXTRA_BITS) / 2`.
pub const SCALE_EXTRA_OFF: i32 = (1 << SCALE_EXTRA_BITS) / 2;

/// `Mv` — an eighth-pel motion vector, `int16_t` per component as in C.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mv {
    /// Horizontal component.
    pub x: i16,
    /// Vertical component.
    pub y: i16,
}

/// The `MacroBlockD` edge distances `clamp_mv_to_umv_border_sb` reads. C keeps
/// these in eighth-pel units relative to the block.
#[derive(Debug, Clone, Copy)]
pub struct MbEdges {
    /// `xd->mb_to_left_edge`.
    pub to_left: i32,
    /// `xd->mb_to_right_edge`.
    pub to_right: i32,
    /// `xd->mb_to_top_edge`.
    pub to_top: i32,
    /// `xd->mb_to_bottom_edge`.
    pub to_bottom: i32,
}

/// `clamp_mv` (mv.h:70) — clamps in place, then truncates to `int16_t`.
pub fn clamp_mv(mv: &mut Mv, min_col: i32, max_col: i32, min_row: i32, max_row: i32) {
    mv.x = (mv.x as i32).clamp(min_col, max_col) as i16;
    mv.y = (mv.y as i32).clamp(min_row, max_row) as i16;
}

/// `clamp_mv_to_umv_border_sb` (enc_inter_prediction.c:55).
///
/// TRAP: the MV is first scaled by `1 << (1 - ss)` — a LEFT shift for luma
/// (`ss = 0`) and a no-op for chroma (`ss = 1`) — and the product is truncated
/// to `int16_t` BEFORE the clamp. So a large luma MV can wrap here, and C
/// relies on that wrap not happening rather than preventing it. The
/// truncation is reproduced.
///
/// The four bounds are asymmetric on purpose: `spel_right` is
/// `spel_left - SUBPEL_SHIFTS` and `spel_bottom` is
/// `spel_top - SUBPEL_SHIFTS`, so the positive side allows one less full pel.
pub fn clamp_mv_to_umv_border_sb(
    edges: &MbEdges,
    src_mv: Mv,
    bw: i32,
    bh: i32,
    ss_x: i32,
    ss_y: i32,
) -> Mv {
    debug_assert!(ss_x <= 1 && ss_y <= 1);
    let spel_left = (AOM_INTERP_EXTEND + bw) << SUBPEL_BITS;
    let spel_right = spel_left - SUBPEL_SHIFTS;
    let spel_top = (AOM_INTERP_EXTEND + bh) << SUBPEL_BITS;
    let spel_bottom = spel_top - SUBPEL_SHIFTS;

    let mut clamped = Mv {
        x: (src_mv.x as i32 * (1 << (1 - ss_x))) as i16,
        y: (src_mv.y as i32 * (1 << (1 - ss_y))) as i16,
    };
    clamp_mv(
        &mut clamped,
        edges.to_left * (1 << (1 - ss_x)) - spel_left,
        edges.to_right * (1 << (1 - ss_x)) + spel_right,
        edges.to_top * (1 << (1 - ss_y)) - spel_top,
        edges.to_bottom * (1 << (1 - ss_y)) + spel_bottom,
    );
    clamped
}

/// The reference-frame geometry `compute_subpel_params`'s scaled arm clamps
/// the position against.
#[derive(Debug, Clone, Copy)]
pub struct RefGeometry {
    /// `scs->super_block_size` — 64 or 128.
    pub super_block_size: i32,
    /// `frame_width`.
    pub frame_width: i32,
    /// `frame_height`.
    pub frame_height: i32,
}

/// `compute_subpel_params` (enc_inter_prediction.c:2400) -> `(subpel_params,
/// pos_y, pos_x)`.
///
/// `pre_y` / `pre_x` are the block's position in the reference plane.
///
/// The scaled arm's `border_in_pixels` is `super_block_size * 2 + 32` — C says
/// explicitly that when `is_scaled` the recon padding is that constant rather
/// than 288, and that the top/left offsets use `INTERPOLATION_OFFSET` (8) not
/// `AOM_INTERP_EXTEND` (4) because `svt_aom_pack_block` reads 8 pixels back
/// (upstream issue 1835). Substituting 4 there reintroduces that read.
#[allow(clippy::too_many_arguments)]
pub fn compute_subpel_params(
    geom: RefGeometry,
    pre_y: i32,
    pre_x: i32,
    mv: Mv,
    sf: &ScaleFactors,
    blk_width: i32,
    blk_height: i32,
    edges: &MbEdges,
    ss_y: i32,
    ss_x: i32,
) -> (SubpelParams, i32, i32) {
    if sf.is_scaled() {
        let mut orig_pos_y = pre_y << SUBPEL_BITS;
        orig_pos_y += mv.y as i32 * (1 << (1 - ss_y));
        let mut orig_pos_x = pre_x << SUBPEL_BITS;
        orig_pos_x += mv.x as i32 * (1 << (1 - ss_x));
        let mut pos_y = sf.scale_value_y(orig_pos_y) + SCALE_EXTRA_OFF;
        let mut pos_x = sf.scale_value_x(orig_pos_x) + SCALE_EXTRA_OFF;

        let border_in_pixels = geom.super_block_size * 2 + 32;
        let top = -(((border_in_pixels >> ss_y) - INTERPOLATION_OFFSET) << SCALE_SUBPEL_BITS);
        let left = -(((border_in_pixels >> ss_x) - INTERPOLATION_OFFSET) << SCALE_SUBPEL_BITS);
        let bottom = ((geom.frame_height >> ss_y) + AOM_INTERP_EXTEND) << SCALE_SUBPEL_BITS;
        let right = ((geom.frame_width >> ss_x) + AOM_INTERP_EXTEND) << SCALE_SUBPEL_BITS;

        pos_y = pos_y.clamp(top, bottom);
        pos_x = pos_x.clamp(left, right);

        let sp = SubpelParams {
            subpel_x: pos_x & SCALE_SUBPEL_MASK,
            subpel_y: pos_y & SCALE_SUBPEL_MASK,
            xs: sf.x_step_q4,
            ys: sf.y_step_q4,
        };
        (sp, pos_y >> SCALE_SUBPEL_BITS, pos_x >> SCALE_SUBPEL_BITS)
    } else {
        let mv_q4 = clamp_mv_to_umv_border_sb(edges, mv, blk_width, blk_height, ss_x, ss_y);
        let sp = SubpelParams {
            subpel_x: (mv_q4.x as i32 & SUBPEL_MASK) << SCALE_EXTRA_BITS,
            subpel_y: (mv_q4.y as i32 & SUBPEL_MASK) << SCALE_EXTRA_BITS,
            xs: SCALE_SUBPEL_SHIFTS,
            ys: SCALE_SUBPEL_SHIFTS,
        };
        (
            sp,
            pre_y + (mv_q4.y as i32 >> SUBPEL_BITS),
            pre_x + (mv_q4.x as i32 >> SUBPEL_BITS),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The unscaled arm produces `xs == ys == SCALE_SUBPEL_SHIFTS`, which is
    /// exactly what `has_scale` tests for — so the two functions agree on
    /// which arm the MC dispatcher will then take.
    #[test]
    fn unscaled_arm_yields_unscaled_steps() {
        let sf = ScaleFactors::setup_for_frame(64, 64, 64, 64);
        assert!(!sf.is_scaled());
        let edges = MbEdges {
            to_left: 0,
            to_right: 512,
            to_top: 0,
            to_bottom: 512,
        };
        let (sp, py, px) = compute_subpel_params(
            RefGeometry {
                super_block_size: 64,
                frame_width: 64,
                frame_height: 64,
            },
            8,
            8,
            Mv { x: 5, y: -3 },
            &sf,
            8,
            8,
            &edges,
            0,
            0,
        );
        assert_eq!((sp.xs, sp.ys), (SCALE_SUBPEL_SHIFTS, SCALE_SUBPEL_SHIFTS));
        assert!(!crate::port_scale_factors::has_scale(sp.xs, sp.ys));
        // ss = 0 doubles the MV, so (5, -3) -> (10, -6): 10 >> 4 == 0 and
        // -6 >> 4 == -1 (arithmetic shift), and the phases are the low nibbles
        // promoted into the SCALE_SUBPEL domain.
        assert_eq!(px, 8);
        assert_eq!(py, 7);
        assert_eq!(sp.subpel_x, 10 << SCALE_EXTRA_BITS);
        assert_eq!(sp.subpel_y, (-6i32 & SUBPEL_MASK) << SCALE_EXTRA_BITS);
    }

    /// The positive-side bounds allow one less full pel than the negative
    /// side — dropping that `- SUBPEL_SHIFTS` is a one-pel error at the right
    /// and bottom frame borders only.
    #[test]
    fn umv_bounds_are_asymmetric() {
        let edges = MbEdges {
            to_left: 0,
            to_right: 0,
            to_top: 0,
            to_bottom: 0,
        };
        // A huge positive MV clamps to spel_right = (4 + 8) * 16 - 16 = 176.
        let clamped = clamp_mv_to_umv_border_sb(&edges, Mv { x: 4000, y: 0 }, 8, 8, 1, 1);
        assert_eq!(clamped.x, 176);
        // A huge negative one clamps to -spel_left = -192.
        let clamped = clamp_mv_to_umv_border_sb(&edges, Mv { x: -4000, y: 0 }, 8, 8, 1, 1);
        assert_eq!(clamped.x, -192);
    }
}
