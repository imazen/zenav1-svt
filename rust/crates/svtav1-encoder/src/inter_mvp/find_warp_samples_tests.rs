use super::*;
use crate::intrabc::TileMiBounds;
use crate::intrabc_mvp::{MvpGrid, MvpMiEntry, derive_block_ctx};

const MI: i32 = 16; // a 64x64 frame in 4x4 mi units
const BSIZE_8X8: usize = 3;

fn tile() -> TileMiBounds {
    TileMiBounds {
        mi_row_start: 0,
        mi_col_start: 0,
        mi_row_end: MI,
        mi_col_end: MI,
    }
}

/// An 8x8 neighbour predicting from `rf0` with MV `(mvy, mvx)` in
/// eighth-pel, or an INTRA cell when `rf0` is `INTRA_FRAME`.
fn cell(rf0: i8, rf1: i8, mvy: i16, mvx: i16) -> MvpMiEntry {
    MvpMiEntry {
        bsize: BSIZE_8X8 as u8,
        mode: 13, // NEARESTMV
        use_intrabc: false,
        ref_frame: [rf0, rf1],
        mv: [Mv { x: mvx, y: mvy }, Mv::ZERO],
        partition: 0,
        interp_filters: 0,
        skip_mode: false,
        skip: false,
        comp_group_idx: 0,
        compound_idx: 0,
    }
}

/// Grid of intra cells with the above row, left column and top-left
/// corner of the block at mi `(4,4)` set to `n`.
fn grid_with_neighbours(n: MvpMiEntry) -> alloc::vec::Vec<MvpMiEntry> {
    let mut g = alloc::vec![MvpMiEntry::default(); (MI * MI) as usize];
    for c in 0..MI {
        g[(3 * MI + c) as usize] = n; // the whole row above
    }
    for r in 0..MI {
        g[(r * MI + 3) as usize] = n; // the whole column left
    }
    g
}

fn run(entries: &[MvpMiEntry], rf0: i8) -> u8 {
    let ctx = derive_block_ctx(4, 4, BSIZE_8X8, MI, MI, tile(), MI);
    let grid = MvpGrid {
        entries,
        stride: MI,
        base: 4 * MI + 4,
    };
    find_warp_samples(&grid, &ctx, rf0).0
}

/// TIER 4 — ALL FOUR scans fire for an 8x8 block at mi (4,4) with 8x8
/// neighbours above, left, top-left and top-right.
/// `av1_find_samples` is `static` with no exported symbol, so this is
/// hand-derived against `adaptive_mv_pred.c:1610-1750`:
///
/// * above: `xd->n4_w`(2) `<= n4_w`(2), so `col_offset = -4 % 2 = 0`;
///   `col_offset < 0` is false so `do_tl` survives, and
///   `col_offset + n4_w > xd->n4_w` is `2 > 2` = false so `do_tr`
///   survives too. Sample 1.
/// * left: the mirror, `row_offset = -4 % 2 = 0`. Sample 2.
/// * top-left: `do_tl` and both edges. Sample 3.
/// * top-right: `do_tr`, `has_top_right` (`mask_row & 2` and
///   `mask_col & 2` are both 0 at mi 4, so it is not suppressed) and
///   `is_inside`. Sample 4.
///
/// **The first draft of this test expected THREE and was wrong** — it
/// forgot the top-right scan, and the implementation was right. Recorded
/// because a test written to agree with the code it tests is worth
/// nothing; this one was re-derived from C after it failed.
///
/// It is a POSITIVE CONTROL as much as a value check: the whole point of
/// this function is that the count is NON-ZERO where C's is, because a
/// zero writes the wrong motion-mode ALPHABET
/// (`docs/INTER-ENCODE-PLAN.md` §1z¹⁸).
#[test]
fn counts_all_four_scans_for_an_8x8_with_8x8_neighbours() {
    let g = grid_with_neighbours(cell(1, NONE_FRAME, -8, 16));
    assert_eq!(run(&g, 1), 4, "above + left + top-left + top-right");
}

/// TIER 4 — a neighbour that predicts from a DIFFERENT reference is not a
/// sample. C's test is `ref_frame[0] == rf0`, and the scan is run once per
/// reference precisely because the answer differs per reference.
#[test]
fn a_different_reference_contributes_nothing() {
    let g = grid_with_neighbours(cell(1, NONE_FRAME, -8, 16));
    assert_eq!(run(&g, 5), 0, "BWDREF has no samples here");
}

/// TIER 4 — a COMPOUND neighbour is not a sample either: C requires
/// `ref_frame[1] == NONE_FRAME`. Without this half of the test a port
/// that dropped the second condition would still pass the two above.
#[test]
fn a_compound_neighbour_contributes_nothing() {
    let g = grid_with_neighbours(cell(1, 5, -8, 16));
    assert_eq!(run(&g, 1), 0, "LAST_BWD neighbours are not samples");
}

/// TIER 4 — an all-INTRA neighbourhood gives zero, which is the case the
/// port used to hard-code for every block.
#[test]
fn an_intra_neighbourhood_gives_zero() {
    let g = alloc::vec![MvpMiEntry::default(); (MI * MI) as usize];
    assert_eq!(run(&g, 1), 0);
}

/// TIER 4 — the recorded POINT, hand-derived from C's `record_samples`
/// (adaptive_mv_pred.c:1594) for the ABOVE neighbour of an 8x8 block at
/// mi (4,4) with 8x8 neighbours:
///
/// `n4_w` is 2 and `xd->n4_w` is 2, so C takes the "current block width
/// <= above block width" arm with `col_offset = -mi_col % n4_w = -4 % 2 =
/// 0`. Then `x = 0*4 + 1*max(8,4)/2 - 1 = 3` and
/// `y = 0*4 + (-1)*max(8,4)/2 - 1 = -5`, and the point is `(x*8, y*8)`
/// with the projection offset by the neighbour's own MV.
///
/// The MV components are deliberately DIFFERENT (`mv = (y=-8, x=16)`) so
/// a transposed `pts_inref` cannot pass.
#[test]
fn the_recorded_point_matches_c_for_the_above_neighbour() {
    let g = grid_with_neighbours(cell(1, NONE_FRAME, -8, 16));
    let ctx = derive_block_ctx(4, 4, BSIZE_8X8, MI, MI, tile(), MI);
    let grid = MvpGrid {
        entries: &g,
        stride: MI,
        base: 4 * MI + 4,
    };
    let (n, pts, pts_inref) = find_warp_samples(&grid, &ctx, 1);
    assert_eq!(n, 4);
    assert_eq!(pts[0], [3 * 8, -5 * 8], "the above neighbour's centre");
    assert_eq!(
        pts_inref[0],
        [3 * 8 + 16, -5 * 8 + -8],
        "projected through the neighbour's own MV"
    );
}
