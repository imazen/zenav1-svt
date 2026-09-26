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
