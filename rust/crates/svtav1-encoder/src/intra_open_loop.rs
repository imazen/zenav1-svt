//! Open-loop intra prediction — the residual entry points of
//! `Codec/intra_prediction.c`.
//!
//! [`intra_prediction_open_loop_mb`] is the predictor the OPEN-LOOP intra
//! search runs (`svt_aom_intra_prediction_open_loop_mb`,
//! intra_prediction.c:2654). It is not the mode-decision predictor: it takes
//! raw source neighbours with no edge filtering, no upsampling and no
//! reconstruction, which is what makes it cheap enough for the OIS pass that
//! feeds TPL and motion estimation. That makes it inter-encode
//! infrastructure, not an intra tool.
//!
//! [`is_smooth`] is `svt_aom_is_smooth` (intra_prediction.c:128) — the
//! predicate the deblocking and CDEF paths use to ask whether a block's
//! prediction is one of the three SMOOTH modes.
//!
//! ## What C reaches through a function-pointer table and this does not
//!
//! C dispatches through `svt_aom_eb_pred[mode][tx_size]` and
//! `svt_aom_dc_pred[left][top][tx_size]`, two arrays that
//! `svt_aom_init_intra_predictors_internal` /
//! `svt_aom_init_intra_dc_predictors_c_internal` fill with 100+ sized
//! kernels at library init. This port dispatches on the mode enum and passes
//! `(width, height)` to one generic kernel per mode, so both init functions
//! have no counterpart here BY DESIGN — they build a dispatch table this
//! port does not have, and porting them would be porting the table, not the
//! arithmetic. The arithmetic is [`svtav1_dsp::intra_pred`].
//!
//! Evidence: tier 1 — `crates/svtav1-encoder/tests/c_parity_intra_open_loop.rs`
//! drives the real exported `svt_aom_intra_prediction_open_loop_mb`,
//! `svt_aom_dr_predictor` and `svt_aom_is_smooth`.

use svtav1_dsp::intra_pred;
use svtav1_types::prediction::{PredictionMode, UvPredictionMode};

/// C `av1_is_directional_mode` (intra_prediction.h:206): `V_PRED` through
/// `D67_PRED`. Note that `V_PRED` and `H_PRED` ARE directional — they reach
/// the directional predictor at angle 90 / 180, which then delegates to the
/// vertical / horizontal kernel.
#[must_use]
pub const fn is_directional_mode(mode: PredictionMode) -> bool {
    let m = mode as u8;
    m >= PredictionMode::VPred as u8 && m <= PredictionMode::D67Pred as u8
}

/// C `svt_aom_is_smooth` (intra_prediction.c:128).
///
/// For luma the answer is a property of `mode`. For chroma C first has to
/// rule out an inter block, because `uv_mode` is not written for those — the
/// port takes that as an explicit `is_inter` argument rather than reading a
/// reference-frame field, since the caller already knows.
#[must_use]
pub fn is_smooth(
    mode: PredictionMode,
    uv_mode: UvPredictionMode,
    plane: usize,
    is_inter: bool,
) -> bool {
    if plane == 0 {
        matches!(
            mode,
            PredictionMode::SmoothPred | PredictionMode::SmoothVPred | PredictionMode::SmoothHPred
        )
    } else if is_inter {
        false
    } else {
        matches!(
            uv_mode,
            UvPredictionMode::UvSmoothPred
                | UvPredictionMode::UvSmoothVPred
                | UvPredictionMode::UvSmoothHPred
        )
    }
}

/// The neighbour samples an open-loop prediction reads.
///
/// C passes `above_row` and `left_col` as pointers into buffers whose `[-1]`
/// element is the shared top-left corner sample; this states the same thing
/// without the negative index. `above` must hold `width + height` samples for
/// zone-1 prediction and `left` the same for zone 3 (the directional
/// predictors walk past the block into the extension).
#[derive(Debug, Clone, Copy)]
pub struct Neighbours<'a> {
    /// C `above_row[0 ..]`.
    pub above: &'a [u8],
    /// C `left_col[0 ..]`.
    pub left: &'a [u8],
    /// C `above_row[-1]`, which is also `left_col[-1]`.
    pub top_left: u8,
    /// C `src_origin_x > 0` — a left neighbour exists.
    pub has_left: bool,
    /// C `src_origin_y > 0` — an above neighbour exists.
    pub has_above: bool,
}

/// C `svt_aom_intra_prediction_open_loop_mb` (intra_prediction.c:2654).
///
/// Returns `Err` for the one input C accepts and silently does nothing with:
/// a directional mode whose `p_angle` is outside `(0, 270)` and not 90/180,
/// where `svt_aom_dr_predictor`'s if/else-if chain falls through and leaves
/// `dst` as it found it. C returns `EB_ErrorNone` there and the caller then
/// scores an uninitialized block; a typed refusal is the port's rule
/// (`docs/WORKING-ON-THIS.md` §6) and `dst` is left untouched either way, so
/// this is strictly more informative and never changes a byte C would emit.
///
/// # Errors
/// [`OpenLoopError::AngleOutOfRange`] when the above holds.
pub fn intra_prediction_open_loop_mb(
    mode: PredictionMode,
    p_angle: i32,
    n: Neighbours<'_>,
    width: usize,
    height: usize,
    dst: &mut [u8],
    dst_stride: usize,
) -> Result<(), OpenLoopError> {
    if is_directional_mode(mode) {
        // C: svt_aom_dr_predictor(dst, stride, tx_size, above, left, 0, 0, angle).
        if !(0 < p_angle && p_angle < 270) {
            return Err(OpenLoopError::AngleOutOfRange { p_angle });
        }
        intra_pred::predict_directional(
            dst, dst_stride, n.above, n.left, n.top_left, width, height, p_angle,
        );
        return Ok(());
    }

    match mode {
        // C indexes svt_aom_dc_pred[src_origin_x > 0][src_origin_y > 0], i.e.
        // [has_left][has_above]; dc_pred_c[0][0] = dc_128, [0][1] = dc_top,
        // [1][0] = dc_left, [1][1] = dc (intra_prediction.c:1822-1826).
        PredictionMode::DcPred => intra_pred::predict_dc(
            dst,
            dst_stride,
            n.above,
            n.left,
            width,
            height,
            n.has_above,
            n.has_left,
        ),
        PredictionMode::SmoothPred => {
            intra_pred::predict_smooth(dst, dst_stride, n.above, n.left, width, height);
        }
        PredictionMode::SmoothVPred => {
            // NOTE the argument list: `predict_smooth_v` takes
            // (_width, height, width) — its 5th parameter is unused and
            // the real width is the 7th. Passing `width` twice is correct,
            // not a typo.
            intra_pred::predict_smooth_v(dst, dst_stride, n.above, n.left, width, height, width);
        }
        PredictionMode::SmoothHPred => {
            intra_pred::predict_smooth_h(dst, dst_stride, n.above, n.left, width, height);
        }
        PredictionMode::PaethPred => {
            intra_pred::predict_paeth(dst, dst_stride, n.above, n.left, n.top_left, width, height);
        }
        // Every remaining value is an INTER mode. C would index
        // svt_aom_eb_pred[mode][tx_size] with it and read a NULL slot;
        // refusing is the port's rule.
        _ => return Err(OpenLoopError::NotAnIntraMode { mode }),
    }
    Ok(())
}

/// Inputs `svt_aom_intra_prediction_open_loop_mb` accepts but cannot
/// predict from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenLoopError {
    /// A directional mode with `p_angle` outside `(0, 270)`. C's dispatch
    /// chain falls through and leaves `dst` untouched.
    AngleOutOfRange {
        /// The offending angle.
        p_angle: i32,
    },
    /// A non-intra mode. C would dereference an unfilled dispatch slot.
    NotAnIntraMode {
        /// The offending mode.
        mode: PredictionMode,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn is_smooth_matches_the_three_luma_modes() {
        for m in [
            PredictionMode::SmoothPred,
            PredictionMode::SmoothVPred,
            PredictionMode::SmoothHPred,
        ] {
            assert!(is_smooth(m, UvPredictionMode::UvDcPred, 0, false));
        }
        assert!(!is_smooth(
            PredictionMode::DcPred,
            UvPredictionMode::UvSmoothPred,
            0,
            false
        ));
    }

    #[test]
    fn is_smooth_is_false_for_every_chroma_plane_of_an_inter_block() {
        for plane in 1..=2 {
            assert!(!is_smooth(
                PredictionMode::NewMv,
                UvPredictionMode::UvSmoothPred,
                plane,
                true
            ));
            // The same uv_mode on an INTRA block does count.
            assert!(is_smooth(
                PredictionMode::SmoothPred,
                UvPredictionMode::UvSmoothPred,
                plane,
                false
            ));
        }
    }

    #[test]
    fn v_and_h_are_directional() {
        assert!(is_directional_mode(PredictionMode::VPred));
        assert!(is_directional_mode(PredictionMode::HPred));
        assert!(is_directional_mode(PredictionMode::D67Pred));
        assert!(!is_directional_mode(PredictionMode::DcPred));
        assert!(!is_directional_mode(PredictionMode::SmoothPred));
        assert!(!is_directional_mode(PredictionMode::PaethPred));
    }

    #[test]
    fn out_of_range_angle_refuses_and_leaves_dst_untouched() {
        let above = [200u8; 128];
        let left = [50u8; 128];
        let mut dst = vec![7u8; 64];
        let n = Neighbours {
            above: &above,
            left: &left,
            top_left: 0,
            has_left: true,
            has_above: true,
        };
        let e = intra_prediction_open_loop_mb(PredictionMode::VPred, 270, n, 8, 8, &mut dst, 8);
        assert_eq!(e, Err(OpenLoopError::AngleOutOfRange { p_angle: 270 }));
        assert!(dst.iter().all(|&v| v == 7));
    }

    #[test]
    fn inter_mode_is_refused() {
        let above = [0u8; 128];
        let left = [0u8; 128];
        let mut dst = vec![0u8; 64];
        let n = Neighbours {
            above: &above,
            left: &left,
            top_left: 0,
            has_left: true,
            has_above: true,
        };
        assert_eq!(
            intra_prediction_open_loop_mb(PredictionMode::NewMv, 0, n, 8, 8, &mut dst, 8),
            Err(OpenLoopError::NotAnIntraMode {
                mode: PredictionMode::NewMv
            })
        );
    }
}
