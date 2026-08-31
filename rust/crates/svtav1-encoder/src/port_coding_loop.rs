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

/// C `NONE_FRAME` (definitions.h:1379).
pub const NONE_FRAME: i8 = -1;

/// C `INTRA_FRAME` (definitions.h:1380) = 0. `ref_frame > INTRA_FRAME` is the
/// "this slot names a real reference" test.
pub const INTRA_FRAME: i8 = 0;

/// C `REF_FRAMES` = `1 << REF_FRAMES_LOG2` = 8 — the length of
/// `pcs->ref_frame_side`.
pub const REF_FRAMES: usize = 8;

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
mod tests {
    use super::*;

    fn field(n: usize) -> alloc::vec::Vec<MvRef> {
        alloc::vec![
            MvRef {
                mv: Mv { x: 111, y: 222 },
                ref_frame: 9,
            };
            n
        ]
    }

    /// EVIDENCE TIER 4 — hand-derived from coding_loop.c:1038-1069.
    /// `av1_copy_frame_mvs` is C `static` with no exported symbol and no
    /// exported caller reachable without an EncDecContext shell (see the
    /// module doc), so no differential against the real C is available.
    ///
    /// Geometry: mi_cols = 9 -> stride = (9+1)>>1 = 5. A block at
    /// (mi_row, mi_col) = (2, 4) with x_mis = 3, y_mis = 3 writes
    /// (3+1)>>1 = 2 columns and 2 rows starting at cell
    /// (2>>1)*5 + (4>>1) = 1*5 + 2 = 7.
    #[test]
    fn geometry_is_half_resolution_with_round_up_extents() {
        let mut f = field(40);
        copy_frame_mvs(
            &mut f,
            9,
            [1, 0],
            [Mv { x: 8, y: -16 }, Mv { x: 0, y: 0 }],
            &[0; REF_FRAMES],
            2,
            4,
            3,
            3,
        );
        for &cell in &[7usize, 8, 12, 13] {
            assert_eq!(f[cell].ref_frame, 1, "cell {cell} should be written");
            assert_eq!(f[cell].mv, Mv { x: 8, y: -16 });
        }
        for cell in 0..40usize {
            if matches!(cell, 7 | 8 | 12 | 13) {
                continue;
            }
            assert_eq!(f[cell].ref_frame, 9, "cell {cell} must be untouched");
        }
    }

    /// The reset happens before the conditional write, so a block whose only
    /// reference is vetoed CLEARS the cell rather than leaving it stale.
    #[test]
    fn a_vetoed_block_clears_the_cell() {
        let mut f = field(4);
        let mut side = [0i8; REF_FRAMES];
        side[1] = 1; // LAST_FRAME is on the "other side" -> skip
        copy_frame_mvs(
            &mut f,
            2,
            [1, 0],
            [Mv { x: 8, y: 8 }, Mv { x: 0, y: 0 }],
            &side,
            0,
            0,
            2,
            2,
        );
        assert_eq!(f[0].ref_frame, NONE_FRAME);
        assert_eq!(f[0].mv, Mv { x: 0, y: 0 });
    }

    /// An intra block (both slots `INTRA_FRAME`) also clears: `> INTRA_FRAME`
    /// is strict.
    #[test]
    fn intra_block_clears_the_cell() {
        let mut f = field(4);
        copy_frame_mvs(
            &mut f,
            2,
            [INTRA_FRAME, NONE_FRAME],
            [Mv { x: 8, y: 8 }, Mv { x: 4, y: 4 }],
            &[0; REF_FRAMES],
            0,
            0,
            2,
            2,
        );
        assert_eq!(f[0].ref_frame, NONE_FRAME);
        assert_eq!(f[0].mv, Mv { x: 0, y: 0 });
    }

    /// C tries slot 0 then slot 1 and does not break, so when both qualify the
    /// SECOND one is what lands. A port that stopped at the first match would
    /// store the wrong reference in every compound block.
    #[test]
    fn the_second_slot_wins_when_both_qualify() {
        let mut f = field(4);
        copy_frame_mvs(
            &mut f,
            2,
            [1, 4],
            [Mv { x: 8, y: 8 }, Mv { x: -24, y: 32 }],
            &[0; REF_FRAMES],
            0,
            0,
            2,
            2,
        );
        assert_eq!(f[0].ref_frame, 4);
        assert_eq!(f[0].mv, Mv { x: -24, y: 32 });
    }

    /// ...but only when the second qualifies. A vetoed second slot leaves the
    /// first slot's write standing (C `continue`s, it does not reset).
    #[test]
    fn a_vetoed_second_slot_leaves_the_first_standing() {
        let mut f = field(4);
        let mut side = [0i8; REF_FRAMES];
        side[4] = 1;
        copy_frame_mvs(
            &mut f,
            2,
            [1, 4],
            [Mv { x: 8, y: 8 }, Mv { x: -24, y: 32 }],
            &side,
            0,
            0,
            2,
            2,
        );
        assert_eq!(f[0].ref_frame, 1);
        assert_eq!(f[0].mv, Mv { x: 8, y: 8 });
    }

    /// The REFMVS_LIMIT test is strict `>` on each component's absolute value,
    /// independently. ±4095 stores; ±4096 does not, and an over-limit X alone
    /// is enough to reject.
    #[test]
    fn refmvs_limit_is_strict_and_per_component() {
        for (mv, stored) in [
            (Mv { x: 4095, y: 4095 }, true),
            (Mv { x: -4095, y: -4095 }, true),
            (Mv { x: 4096, y: 0 }, false),
            (Mv { x: 0, y: 4096 }, false),
            (Mv { x: -4096, y: 0 }, false),
            (Mv { x: 0, y: -4096 }, false),
        ] {
            let mut f = field(4);
            copy_frame_mvs(
                &mut f,
                2,
                [1, 0],
                [mv, Mv { x: 0, y: 0 }],
                &[0; REF_FRAMES],
                0,
                0,
                2,
                2,
            );
            if stored {
                assert_eq!(f[0].ref_frame, 1, "mv {mv:?} should store");
                assert_eq!(f[0].mv, mv);
            } else {
                assert_eq!(f[0].ref_frame, NONE_FRAME, "mv {mv:?} should be rejected");
                assert_eq!(f[0].mv, Mv { x: 0, y: 0 });
            }
        }
    }

    /// A negative `ref_frame_side` entry also vetoes: C tests `if (ref_idx)`,
    /// not `if (ref_idx > 0)`, and the array is `int8_t`.
    #[test]
    fn a_negative_ref_frame_side_also_vetoes() {
        let mut f = field(4);
        let mut side = [0i8; REF_FRAMES];
        side[1] = -1;
        copy_frame_mvs(
            &mut f,
            2,
            [1, 0],
            [Mv { x: 8, y: 8 }, Mv { x: 0, y: 0 }],
            &side,
            0,
            0,
            2,
            2,
        );
        assert_eq!(f[0].ref_frame, NONE_FRAME);
    }

    /// A 4x4 block (x_mis = y_mis = 1) still claims one cell: the extents
    /// round UP, so `(1 + 1) >> 1 = 1`. Rounding down would write nothing and
    /// leave the previous frame's motion in the field.
    #[test]
    fn a_single_mi_block_still_claims_one_cell() {
        let mut f = field(4);
        copy_frame_mvs(
            &mut f,
            4,
            [1, 0],
            [Mv { x: 8, y: 8 }, Mv { x: 0, y: 0 }],
            &[0; REF_FRAMES],
            0,
            0,
            1,
            1,
        );
        assert_eq!(f[0].ref_frame, 1);
        assert_eq!(f[1].ref_frame, 9, "only one cell may be written");
    }

    /// Odd `mi_cols` rounds the STRIDE up too: mi_cols = 7 -> stride 4, so
    /// row 1 starts at cell 4, not 3.
    #[test]
    fn odd_mi_cols_rounds_the_stride_up() {
        let mut f = field(16);
        copy_frame_mvs(
            &mut f,
            7,
            [1, 0],
            [Mv { x: 8, y: 8 }, Mv { x: 0, y: 0 }],
            &[0; REF_FRAMES],
            2,
            0,
            2,
            2,
        );
        // (2 >> 1) * 4 = 4
        assert_eq!(f[4].ref_frame, 1);
        assert_eq!(f[3].ref_frame, 9);
    }
}
