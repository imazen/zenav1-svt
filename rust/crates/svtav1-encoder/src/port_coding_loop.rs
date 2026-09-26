//! The MFMV writeback from `Source/Lib/Codec/coding_loop.c`.
//!
//! ## Coverage — 1 of 1 function in this group
//!
//! | C function | line | here |
//! |---|---|---|
//! | `av1_copy_frame_mvs` | 1038 | [`copy_frame_mvs`] |
//!
//! MISSING from coding_loop.c: everything else — `update_b`, `encode_b`, the
//! encode pass and the recon conversions are not ported here.
//!
//! ## Why this matters, and exactly when
//!
//! `update_b` (coding_loop.c:1758) calls this for every coded block whenever
//! `pcs->scs->mfmv_enabled && pcs->slice_type != I_SLICE && pcs->ppcs->is_ref`.
//! It writes the reference object's `MV_REF` field at HALF mi resolution.
//! `md_config_process.c`'s `motion_field_projection` consumes exactly that
//! field to build `pcs->tpl_mvs`, which `adaptive_mv_pred.c:366` reads as the
//! TEMPORAL MVP candidate.
//!
//! So: with this missing, every frame from the SECOND inter frame onward gets
//! wrong TMVP candidates, therefore a wrong `ref_mv` stack, therefore wrong
//! DRL indices and wrong MV differences. It is NOT needed for a 2-frame cell
//! (the first inter frame projects from a key frame that coded no MVs); it is
//! needed the moment the GOP is three frames or longer.
//!
//! ## Evidence tier — 4, and why not 1
//!
//! `av1_copy_frame_mvs` is C `static`: `nm -g` on
//! `Bin/Release/libSvtAv1Enc.a` prints nothing for it. Its only caller,
//! `update_b`, is also `static`, and the nearest exported ancestor
//! (`svt_aom_encode_pass`) would need a whole `EncDecContext` +
//! `PictureControlSet` + a real coded block before the call is reached — a
//! shell far larger and less trustworthy than the twenty lines under test. So
//! the tests below are **hand-derived vectors traced against the C source**,
//! the weakest tier (`docs/WORKING-ON-THIS.md` §4), and they say so.

use crate::inter_mvp::MvRef;
use svtav1_types::motion::Mv;

/// C `REFMVS_LIMIT` (coding_loop.c:1036) = `(1 << 12) - 1`.
pub const REFMVS_LIMIT: i32 = (1 << 12) - 1;

pub use svtav1_types::reference::NONE_FRAME;

pub use svtav1_types::reference::INTRA_FRAME;

pub use svtav1_types::reference::REF_FRAMES;

/// C `ROUND_POWER_OF_TWO(value, 1)`.
#[inline]
fn round_power_of_two_1(value: i32) -> i32 {
    (value + 1) >> 1
}

/// C `av1_copy_frame_mvs` (coding_loop.c:1038-1069).
///
/// Writes one block's motion into the reference object's `MV_REF` plane at
/// HALF mi resolution: the plane's stride is `ROUND_POWER_OF_TWO(mi_cols, 1)`,
/// the origin is `(mi_row >> 1, mi_col >> 1)`, and the extents are the
/// ROUNDED-UP halves of `x_mis` / `y_mis`. Rounding up rather than down is
/// what makes an odd-sized block still claim its partial cell.
///
/// Every cell is first RESET to `(NONE_FRAME, 0)` and only then conditionally
/// overwritten, so a block with no usable reference clears whatever the
/// previous frame's block left there.
///
/// Three details that change the temporal MVP field:
///
/// * the two `ref_frame` slots are tried IN ORDER and each writes the cell
///   outright, so when BOTH qualify the SECOND one wins. C does not `break`.
/// * `pcs->ref_frame_side[ref_frame]` is a veto: a NON-ZERO entry skips the
///   slot. (It is `int8_t`, so a negative entry also vetoes.)
/// * the `REFMVS_LIMIT` test is on the ABSOLUTE value of each component
///   independently and is strictly `>`, so exactly ±4095 is still stored.
///
/// `mvs` is the reference object's whole `object_ptr->mvs` allocation, indexed
/// as C indexes it. `ref_frame_side` is `pcs->ref_frame_side`.
#[allow(clippy::too_many_arguments)]
pub fn copy_frame_mvs(
    mvs: &mut [MvRef],
    mi_cols: i32,
    ref_frame: [i8; 2],
    mv: [Mv; 2],
    ref_frame_side: &[i8; REF_FRAMES],
    mi_row: i32,
    mi_col: i32,
    x_mis: i32,
    y_mis: i32,
) {
    let frame_mvs_stride = round_power_of_two_1(mi_cols);
    let mut frame_mvs = (mi_row >> 1) * frame_mvs_stride + (mi_col >> 1);
    let x_mis = round_power_of_two_1(x_mis);
    let y_mis = round_power_of_two_1(y_mis);

    for _h in 0..y_mis {
        let mut cell = frame_mvs;
        for _w in 0..x_mis {
            let slot = &mut mvs[cell as usize];
            slot.ref_frame = NONE_FRAME;
            slot.mv = Mv { x: 0, y: 0 };

            for idx in 0..2 {
                let rf = ref_frame[idx];
                if rf > INTRA_FRAME {
                    let ref_idx = ref_frame_side[rf as usize];
                    if ref_idx != 0 {
                        continue;
                    }
                    if i32::from(mv[idx].y).abs() > REFMVS_LIMIT
                        || i32::from(mv[idx].x).abs() > REFMVS_LIMIT
                    {
                        continue;
                    }
                    slot.ref_frame = rf;
                    slot.mv = mv[idx];
                }
            }
            cell += 1;
        }
        frame_mvs += frame_mvs_stride;
    }
}

#[cfg(test)]
mod tests;
