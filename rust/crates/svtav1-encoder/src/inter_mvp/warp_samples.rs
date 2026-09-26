use super::*;

/// C `is_neighbor_overlappable` (inter_prediction.h:271-273).
#[inline]
pub(super) fn is_neighbor_overlappable(e: &MvpMiEntry) -> bool {
    e.ref_frame[0] > INTRA_FRAME
}

/// C `count_overlappable_nb_above` (adaptive_mv_pred.c:1830-1861).
///
/// The `mi_step == 1` arm rewinds the LOOP VARIABLE (`above_mi_col &= ~1`)
/// and then reads the cell one to its right — a 4-wide block is treated as
/// half of a chroma pair — so the rewind is observable in the iteration
/// order, not just in which cell is read.
pub(super) fn count_overlappable_nb_above(grid: &MvpGrid, ctx: &MvpBlockCtx, nb_max: u32) -> u32 {
    let mut nb_count = 0u32;
    if !ctx.up_available {
        return nb_count;
    }
    let end_col = (ctx.mi_col + ctx.n8_w).min(ctx.mi_cols);
    let mut above_mi_col = ctx.mi_col;
    while above_mi_col < end_col && nb_count < nb_max {
        // prev_row_mi + above_mi_col == xd->mi[-mi_stride + (col - mi_col)]
        let mut off = -grid.stride + (above_mi_col - ctx.mi_col);
        let mut mi_step =
            i32::from(NUM_4X4_BLOCKS_WIDE[usize::from(cell_at(grid, off).bsize)]).min(16);
        if mi_step == 1 {
            above_mi_col &= !1;
            off = -grid.stride + (above_mi_col - ctx.mi_col) + 1;
            mi_step = 2;
        }
        if is_neighbor_overlappable(cell_at(grid, off)) {
            nb_count += 1;
        }
        above_mi_col += mi_step;
    }
    nb_count
}

/// C `count_overlappable_nb_left` (adaptive_mv_pred.c:1864-1891).
pub(super) fn count_overlappable_nb_left(grid: &MvpGrid, ctx: &MvpBlockCtx, nb_max: u32) -> u32 {
    let mut nb_count = 0u32;
    if !ctx.left_available {
        return nb_count;
    }
    let end_row = (ctx.mi_row + ctx.n8_h).min(ctx.mi_rows);
    let mut left_mi_row = ctx.mi_row;
    while left_mi_row < end_row && nb_count < nb_max {
        // prev_col_mi + left_mi_row * mi_stride
        //   == xd->mi[(row - mi_row) * mi_stride - 1]
        let mut off = (left_mi_row - ctx.mi_row) * grid.stride - 1;
        let mut mi_step =
            i32::from(NUM_4X4_BLOCKS_HIGH[usize::from(cell_at(grid, off).bsize)]).min(16);
        if mi_step == 1 {
            left_mi_row &= !1;
            off = (left_mi_row + 1 - ctx.mi_row) * grid.stride - 1;
            mi_step = 2;
        }
        if is_neighbor_overlappable(cell_at(grid, off)) {
            nb_count += 1;
        }
        left_mi_row += mi_step;
    }
    nb_count
}

/// C `svt_av1_count_overlappable_neighbors` (adaptive_mv_pred.c:1893-1906,
/// EXPORTED): `blk_ptr->overlappable_neighbors`, the OBMC gate. Zero for
/// any block narrower or shorter than 8 px.
pub fn count_overlappable_neighbors(grid: &MvpGrid, ctx: &MvpBlockCtx, bsize: usize) -> u32 {
    if !is_motion_variation_allowed_bsize(bsize) {
        return 0;
    }
    count_overlappable_nb_above(grid, ctx, u32::MAX)
        + count_overlappable_nb_left(grid, ctx, u32::MAX)
}

// ---------------------------------------------------------------------------
// av1_find_samples (adaptive_mv_pred.c:1594-1750) — the WARPED-MOTION sample
// scan, whose COUNT decides the motion-mode ALPHABET
// ---------------------------------------------------------------------------

/// C `LEAST_SQUARES_SAMPLES_MAX` (definitions.h:469) — `1 << 3`.
pub const LEAST_SQUARES_SAMPLES_MAX: usize = 8;

/// C `record_samples` (adaptive_mv_pred.c:1594) — one neighbour's centre
/// point and its projection through that neighbour's MV, both in EIGHTH pel
/// and relative to this block's top-left pixel.
///
/// `sign_r` / `sign_c` are C's own `+1` / `-1` selectors, not booleans: the
/// above scan passes `(0, -1, col_offset, 1)` and the left scan
/// `(row_offset, 1, 0, -1)`, so the two differ in WHICH axis gets the
/// half-block bias and in its direction.
#[must_use]
pub(super) fn record_samples(
    e: &crate::intrabc_mvp::MvpMiEntry,
    row_offset: i32,
    sign_r: i32,
    col_offset: i32,
    sign_c: i32,
) -> ([i32; 2], [i32; 2]) {
    let bw = i32::from(svtav1_types::tables::block::BLOCK_SIZE_WIDE[e.bsize as usize]);
    let bh = i32::from(svtav1_types::tables::block::BLOCK_SIZE_HIGH[e.bsize as usize]);
    // C `MI_SIZE` is 4.
    let x = col_offset * 4 + sign_c * bw.max(4) / 2 - 1;
    let y = row_offset * 4 + sign_r * bh.max(4) / 2 - 1;
    (
        [x * 8, y * 8],
        [x * 8 + i32::from(e.mv[0].x), y * 8 + i32::from(e.mv[0].y)],
    )
}

/// C `av1_find_samples` (adaptive_mv_pred.c:1610-1750) — how many
/// SINGLE-reference neighbours predict from `rf0`, and where they are.
///
/// # Why the COUNT is bitstream-critical even with warped motion switched off
///
/// `motion_mode_allowed` promotes a block to `WARPED_CAUSAL` — and with it
/// the THREE-symbol `MOTION_MODES` alphabet instead of the two-symbol OBMC
/// one — when `allow_warped_motion` is set and this count is `>= 1`. The
/// DECODER runs the same scan. A port that leaves the count at 0 writes the
/// wrong ALPHABET on every inter block with an overlappable neighbour, and
/// the arithmetic coder desynchronises: `docs/INTER-ENCODE-PLAN.md` §1z¹⁸
/// measured `aomdec` rejecting 22 of the campaign's 96 cells for exactly
/// this, every one of them at the preset where `allow_warped_motion` is 1.
///
/// **So this is not a warped-motion feature.** Turning `wm_ctrls` off keeps
/// warped motion out of the candidate SET and does nothing about the symbol
/// every inter block writes.
///
/// Four scans in C's order — above, left, top-left, top-right — each with
/// C's own early return at [`LEAST_SQUARES_SAMPLES_MAX`]. The `do_tl` /
/// `do_tr` suppressions are set by the ABOVE and LEFT scans' "current block
/// is no wider/taller than the neighbour" arms and are read by the last two,
/// so the order is load-bearing.
///
/// The sample POINTS are returned as well as the count. Nothing in this port
/// consumes them yet — warped-motion parameter estimation is unported — but
/// they are what a future `svt_aom_warped_motion_parameters` needs, and
/// computing the count without them would be a second, partial transcription
/// of the same scan.
///
/// Evidence tier 4 — `av1_find_samples` is `static` with no exported symbol.
#[must_use]
pub fn find_warp_samples(
    grid: &crate::intrabc_mvp::MvpGrid<'_>,
    ctx: &crate::intrabc_mvp::MvpBlockCtx,
    rf0: i8,
) -> (
    u8,
    [[i32; 2]; LEAST_SQUARES_SAMPLES_MAX],
    [[i32; 2]; LEAST_SQUARES_SAMPLES_MAX],
) {
    let mut pts = [[0i32; 2]; LEAST_SQUARES_SAMPLES_MAX];
    let mut pts_inref = [[0i32; 2]; LEAST_SQUARES_SAMPLES_MAX];
    let mut np: usize = 0;
    let (mut do_tl, mut do_tr) = (true, true);
    let stride = grid.stride;

    // C's `mbmi->block_mi.ref_frame[0] == rf0 && ref_frame[1] == NONE_FRAME`.
    let matches = |e: &crate::intrabc_mvp::MvpMiEntry| -> bool {
        e.ref_frame[0] == rf0 && e.ref_frame[1] == NONE_FRAME
    };
    let n4w = |e: &crate::intrabc_mvp::MvpMiEntry| -> i32 {
        i32::from(svtav1_types::tables::block::BLOCK_SIZE_WIDE[e.bsize as usize]) >> 2
    };
    let n4h = |e: &crate::intrabc_mvp::MvpMiEntry| -> i32 {
        i32::from(svtav1_types::tables::block::BLOCK_SIZE_HIGH[e.bsize as usize]) >> 2
    };

    // ---- the nearest ABOVE row ----
    if ctx.up_available {
        let above = *grid.at(-stride);
        let a_n4_w = n4w(&above);
        if ctx.n8_w <= a_n4_w {
            // C `int col_offset = -mi_col % n4_w;` — a C remainder, which is
            // NEGATIVE for a positive mi_col, and both suppressions below read
            // that sign. Rust's `%` on i32 truncates toward zero exactly as
            // C's does, so this is the same expression.
            let col_offset = -ctx.mi_col % a_n4_w;
            if col_offset < 0 {
                do_tl = false;
            }
            if col_offset + a_n4_w > ctx.n8_w {
                do_tr = false;
            }
            if matches(&above) {
                let (p, q) = record_samples(&above, 0, -1, col_offset, 1);
                pts[np] = p;
                pts_inref[np] = q;
                np += 1;
                if np >= LEAST_SQUARES_SAMPLES_MAX {
                    return (LEAST_SQUARES_SAMPLES_MAX as u8, pts, pts_inref);
                }
            }
        } else {
            let mut i = 0i32;
            let end = ctx.n8_w.min(ctx.mi_cols - ctx.mi_col);
            while i < end {
                let e = *grid.at(i - stride);
                // C `mi_step = AOMMIN(xd->n4_w, n4_w)` with no lower bound —
                // it does not need one, because every `BlockSize` is at least
                // 4 px wide and `mi_size_wide` is therefore at least 1. The
                // `.max(1)` is a LOOP-TERMINATION guard on a value C proves
                // and Rust does not; it cannot change behaviour for any real
                // bsize, and without it a corrupt grid entry would hang.
                let step = ctx.n8_w.min(n4w(&e)).max(1);
                if matches(&e) {
                    let (p, q) = record_samples(&e, 0, -1, i, 1);
                    pts[np] = p;
                    pts_inref[np] = q;
                    np += 1;
                    if np >= LEAST_SQUARES_SAMPLES_MAX {
                        return (LEAST_SQUARES_SAMPLES_MAX as u8, pts, pts_inref);
                    }
                }
                i += step;
            }
        }
    }

    // ---- the nearest LEFT column ----
    if ctx.left_available {
        let left = *grid.at(-1);
        let l_n4_h = n4h(&left);
        if ctx.n8_h <= l_n4_h {
            let row_offset = -ctx.mi_row % l_n4_h;
            if row_offset < 0 {
                do_tl = false;
            }
            if matches(&left) {
                let (p, q) = record_samples(&left, row_offset, 1, 0, -1);
                pts[np] = p;
                pts_inref[np] = q;
                np += 1;
                if np >= LEAST_SQUARES_SAMPLES_MAX {
                    return (LEAST_SQUARES_SAMPLES_MAX as u8, pts, pts_inref);
                }
            }
        } else {
            let mut i = 0i32;
            let end = ctx.n8_h.min(ctx.mi_rows - ctx.mi_row);
            while i < end {
                let e = *grid.at(i * stride - 1);
                // Same guard as the above scan — see there.
                let step = ctx.n8_h.min(n4h(&e)).max(1);
                if matches(&e) {
                    let (p, q) = record_samples(&e, i, 1, 0, -1);
                    pts[np] = p;
                    pts_inref[np] = q;
                    np += 1;
                    if np >= LEAST_SQUARES_SAMPLES_MAX {
                        return (LEAST_SQUARES_SAMPLES_MAX as u8, pts, pts_inref);
                    }
                }
                i += step;
            }
        }
    }

    // ---- TOP-LEFT ----
    if do_tl && ctx.left_available && ctx.up_available {
        let e = *grid.at(-stride - 1);
        if matches(&e) {
            let (p, q) = record_samples(&e, 0, -1, 0, -1);
            pts[np] = p;
            pts_inref[np] = q;
            np += 1;
            if np >= LEAST_SQUARES_SAMPLES_MAX {
                return (LEAST_SQUARES_SAMPLES_MAX as u8, pts, pts_inref);
            }
        }
    }

    // ---- TOP-RIGHT ----
    if do_tr
        && crate::intrabc_mvp::has_top_right(grid, ctx, ctx.n8_w.max(ctx.n8_h))
        && crate::intrabc_mvp::is_inside(ctx.tile, ctx.mi_col, ctx.mi_row, -1, ctx.n8_w)
    {
        let e = *grid.at(ctx.n8_w - stride);
        if matches(&e) {
            let (p, q) = record_samples(&e, 0, -1, ctx.n8_w, 1);
            pts[np] = p;
            pts_inref[np] = q;
            np += 1;
        }
    }

    (np as u8, pts, pts_inref)
}
