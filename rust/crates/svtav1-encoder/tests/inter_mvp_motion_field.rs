//! **EVIDENCE TIER 4** (`docs/WORKING-ON-THIS.md` §4 — the weakest tier):
//! hand-derived vectors traced against the C source for the motion-field
//! projection half of chunk C2.
//!
//! Why tier 4 and not tier 1: `get_block_position` (md_config_process.c:396),
//! `motion_field_projection` (:427) and `av1_setup_motion_field` (:523) are
//! all `static` in that translation unit and export NO symbol — verified
//! with `nm -gU Bin/Release/libSvtAv1Enc.a`, which lists
//! `svt_aom_generate_av1_mvp_table`, `setup_ref_mv_list`,
//! `svt_aom_gm_get_motion_vector_enc`, `svt_aom_compute_inter_mode_ctx_light`,
//! `svt_aom_get_av1_mv_pred_drl` and `svt_av1_find_best_ref_mvs_from_stack`
//! (all differentially gated in `c_parity_inter_mvp.rs`) but nothing from
//! md_config_process.c. So there is no C function to drive; every expected
//! value below is derived BY HAND from the C arithmetic and the derivation
//! is written out beside it, so a reader can check the number rather than
//! trust it.
//!
//! The helpers `get_mv_projection`, `lower_mv_precision`,
//! `integer_mv_precision`, `check_sb_border` and `get_relative_dist` are
//! `static INLINE` in headers, hence also unreachable as symbols — but they
//! ARE exercised indirectly at tier 1 through `setup_ref_mv_list`'s MFMV
//! block in `c_parity_inter_mvp.rs`. The directed vectors here pin their
//! edge cases (den == 0, the ±MAX_FRAME_DISTANCE clamps, the MV_UPP/MV_LOW
//! saturation, C's truncating `%`) that a random sweep reaches rarely.

use svtav1_encoder::inter_mvp as rmvp;
use svtav1_types::motion::Mv;

// ---------------------------------------------------------------------------
// get_mv_projection (inter_prediction.h:244-253)
// ---------------------------------------------------------------------------

#[test]
fn get_mv_projection_traced() {
    // den is clamped to MAX_FRAME_DISTANCE (31) then indexes
    // div_mult[32] = {0, 16384, 8192, 5461, 4096, ...}; the product is
    // ROUND_POWER_OF_TWO_SIGNED(ref * num * div_mult[den], 14).

    // num/den = 2/4 -> exact halving. div_mult[4] = 4096.
    //   y: 200*2*4096 = 1_638_400; (+8192) >> 14 = 100
    //   x: 100*2*4096 =   819_200; (+8192) >> 14 = 50
    assert_eq!(
        rmvp::get_mv_projection(Mv { x: 100, y: 200 }, 2, 4),
        Mv { x: 50, y: 100 }
    );

    // Negative ref: ROUND_POWER_OF_TWO_SIGNED negates, rounds, negates.
    assert_eq!(
        rmvp::get_mv_projection(Mv { x: -100, y: -200 }, 2, 4),
        Mv { x: -50, y: -100 }
    );

    // Negative num (a backward projection).
    assert_eq!(
        rmvp::get_mv_projection(Mv { x: 100, y: 200 }, -2, 4),
        Mv { x: -50, y: -100 }
    );

    // den == 0 -> div_mult[0] == 0 -> the projection collapses to zero.
    // This is the arm `add_tpl_ref_mv` reaches for a tpl cell whose
    // `ref_frame_offset` was never written (the calloc'd 0).
    assert_eq!(
        rmvp::get_mv_projection(Mv { x: 1234, y: -4321 }, 7, 0),
        Mv { x: 0, y: 0 }
    );

    // den == 1 -> div_mult[1] = 16384 = 1<<14 -> multiply by num exactly.
    assert_eq!(
        rmvp::get_mv_projection(Mv { x: 3, y: -5 }, 4, 1),
        Mv { x: 12, y: -20 }
    );

    // den is clamped at 31, so den = 31 and den = 200 agree.
    assert_eq!(
        rmvp::get_mv_projection(Mv { x: 900, y: -900 }, 5, 31),
        rmvp::get_mv_projection(Mv { x: 900, y: -900 }, 5, 200)
    );

    // num is clamped to +/-MAX_FRAME_DISTANCE the same way.
    assert_eq!(
        rmvp::get_mv_projection(Mv { x: 7, y: -7 }, 31, 1),
        rmvp::get_mv_projection(Mv { x: 7, y: -7 }, 9999, 1)
    );
    assert_eq!(
        rmvp::get_mv_projection(Mv { x: 7, y: -7 }, -31, 1),
        rmvp::get_mv_projection(Mv { x: 7, y: -7 }, -9999, 1)
    );

    // Saturation at MV_UPP-1 / MV_LOW+1 (= +/-16383): 4000 * 31 / 1 is
    // 124_000, far past the clamp.
    assert_eq!(
        rmvp::get_mv_projection(Mv { x: 4000, y: -4000 }, 31, 1),
        Mv {
            x: 16383,
            y: -16383
        }
    );

    // Rounding is round-half-away-from-zero via the +(1<<13) bias, applied
    // to |value|: 1*1*5461 = 5461; (5461 + 8192) >> 14 = 0. And
    // 3*1*5461 = 16383; (16383 + 8192) >> 14 = 1.
    assert_eq!(
        rmvp::get_mv_projection(Mv { x: 1, y: 3 }, 1, 3),
        Mv { x: 0, y: 1 }
    );
    assert_eq!(
        rmvp::get_mv_projection(Mv { x: -1, y: -3 }, 1, 3),
        Mv { x: 0, y: -1 }
    );
}

// ---------------------------------------------------------------------------
// lower_mv_precision / integer_mv_precision (inter_prediction.h:203-243)
// ---------------------------------------------------------------------------

#[test]
fn lower_mv_precision_traced() {
    // allow_hp = true, is_integer = false -> identity.
    let mut mv = Mv { x: 3, y: -3 };
    rmvp::lower_mv_precision(&mut mv, true, false);
    assert_eq!(mv, Mv { x: 3, y: -3 });

    // allow_hp = false -> odd components move TOWARDS zero.
    let mut mv = Mv { x: 3, y: -3 };
    rmvp::lower_mv_precision(&mut mv, false, false);
    assert_eq!(mv, Mv { x: 2, y: -2 });

    // Even components untouched; 0 is even.
    let mut mv = Mv { x: 4, y: 0 };
    rmvp::lower_mv_precision(&mut mv, false, false);
    assert_eq!(mv, Mv { x: 4, y: 0 });

    // is_integer wins over allow_hp (C tests it first).
    // integer_mv_precision: mod = v % 8 (C truncation, so negative v gives
    // a negative mod); subtract it, then push a full step when |mod| > 4.
    //   11 % 8 = 3 -> 11-3 = 8   (|3| <= 4, no push)
    //   13 % 8 = 5 -> 13-5 = 8, |5| > 4 and mod > 0 -> +8 -> 16
    //  -11 % 8 = -3 -> -11+3 = -8
    //  -13 % 8 = -5 -> -13+5 = -8, |−5| > 4 and mod < 0 -> -8 -> -16
    let mut mv = Mv { x: 11, y: 13 };
    rmvp::lower_mv_precision(&mut mv, true, true);
    assert_eq!(mv, Mv { x: 8, y: 16 });
    let mut mv = Mv { x: -11, y: -13 };
    rmvp::lower_mv_precision(&mut mv, false, true);
    assert_eq!(mv, Mv { x: -8, y: -16 });
    // Exactly 4 does NOT push (the test is `> 4`, not `>= 4`).
    let mut mv = Mv { x: 12, y: -12 };
    rmvp::integer_mv_precision(&mut mv);
    assert_eq!(mv, Mv { x: 8, y: -8 });
    // Already integer -> unchanged.
    let mut mv = Mv { x: 16, y: -24 };
    rmvp::integer_mv_precision(&mut mv);
    assert_eq!(mv, Mv { x: 16, y: -24 });
}

// ---------------------------------------------------------------------------
// check_sb_border / get_relative_dist
// ---------------------------------------------------------------------------

#[test]
fn check_sb_border_traced() {
    // The SB grid here is hard-coded 64x64 (16 mi) in C, independent of
    // seq_header.sb_size.
    assert!(rmvp::check_sb_border(5, 5, 2, 2)); // -> (7, 7), inside
    assert!(!rmvp::check_sb_border(5, 5, 11, 0)); // -> row 16, out
    assert!(!rmvp::check_sb_border(5, 5, 0, 11)); // -> col 16, out
    assert!(!rmvp::check_sb_border(5, 5, -6, 0)); // -> row -1, out
    // mi_row/mi_col are masked to the SB: 20 & 15 == 4.
    assert!(rmvp::check_sb_border(20, 20, 11, 11)); // -> (15, 15), inside
    assert!(!rmvp::check_sb_border(20, 20, 12, 0)); // -> row 16, out
    // The extension positions `setup_ref_mv_list` probes: (voffset, -2).
    assert!(!rmvp::check_sb_border(0, 0, 2, -2)); // col -2 < 0
    assert!(rmvp::check_sb_border(0, 4, 2, -2)); // col 2, inside
}

#[test]
fn get_relative_dist_traced() {
    let off = rmvp::OrderHintInfo {
        enable_order_hint: false,
        order_hint_bits: 5,
    };
    let on = rmvp::OrderHintInfo {
        enable_order_hint: true,
        order_hint_bits: 5,
    };
    // Disabled -> always 0 (this is what makes MFMV inert without order hints).
    assert_eq!(rmvp::get_relative_dist(off, 1, 30), 0);

    // bits = 5 -> m = 16, wrap at 32.
    //   a=1,b=30: diff = -29; (-29 & 15) - (-29 & 16) = 3 - 0 = 3
    assert_eq!(rmvp::get_relative_dist(on, 1, 30), 3);
    //   a=30,b=1: diff = 29; (29 & 15) - (29 & 16) = 13 - 16 = -3
    assert_eq!(rmvp::get_relative_dist(on, 30, 1), -3);
    assert_eq!(rmvp::get_relative_dist(on, 5, 5), 0);
    assert_eq!(rmvp::get_relative_dist(on, 7, 5), 2);
    assert_eq!(rmvp::get_relative_dist(on, 5, 7), -2);
    // The half-period is negative: 16 ahead reads as -16.
    assert_eq!(rmvp::get_relative_dist(on, 16, 0), -16);
    assert_eq!(rmvp::get_relative_dist(on, 15, 0), 15);
}

// ---------------------------------------------------------------------------
// get_block_position (md_config_process.c:396-419)
// ---------------------------------------------------------------------------

#[test]
fn get_block_position_traced() {
    // MAX_OFFSET_HEIGHT = 0 and MAX_OFFSET_WIDTH = 64, so the accept window
    // is rows [base_blk_row, base_blk_row + 8) and cols
    // [base_blk_col - 8, base_blk_col + 16), where base = (blk >> 3) << 3.
    // The MV shift is `>> (4 + MI_SIZE_LOG2)` = `>> 6`, rounding TOWARDS
    // zero for negatives via the explicit negate-shift-negate.
    let (rows, cols) = (64i32, 64i32); // mi dims -> the /2 grid is 32x32

    // Zero MV: identity.
    assert_eq!(
        rmvp::get_block_position(rows, cols, 5, 5, Mv { x: 0, y: 0 }, false),
        Some((5, 5))
    );
    // +64 in y is exactly one unit: 64 >> 6 == 1.
    assert_eq!(
        rmvp::get_block_position(rows, cols, 5, 5, Mv { x: 0, y: 64 }, false),
        Some((6, 5))
    );
    // 63 >> 6 == 0 (truncation).
    assert_eq!(
        rmvp::get_block_position(rows, cols, 5, 5, Mv { x: 0, y: 63 }, false),
        Some((5, 5))
    );
    // -64: -((64) >> 6) == -1, i.e. towards zero, not floor.
    assert_eq!(
        rmvp::get_block_position(rows, cols, 5, 5, Mv { x: 0, y: -64 }, false),
        Some((4, 5))
    );
    assert_eq!(
        rmvp::get_block_position(rows, cols, 5, 5, Mv { x: 0, y: -63 }, false),
        Some((5, 5))
    );
    // sign_bias inverts the offset (`dir >> 1` at the call site).
    assert_eq!(
        rmvp::get_block_position(rows, cols, 5, 5, Mv { x: 0, y: 64 }, true),
        Some((4, 5))
    );

    // Row window: base_blk_row = (5 >> 3) << 3 = 0, so row must be < 8.
    // 8*64 = 512 -> row_offset 8 -> row 13, rejected.
    assert_eq!(
        rmvp::get_block_position(rows, cols, 5, 5, Mv { x: 0, y: 512 }, false),
        None
    );
    // row 7 is the last accepted one: offset 2 -> row 7.
    assert_eq!(
        rmvp::get_block_position(rows, cols, 5, 5, Mv { x: 0, y: 128 }, false),
        Some((7, 5))
    );
    // offset 3 -> row 8, one past the group.
    assert_eq!(
        rmvp::get_block_position(rows, cols, 5, 5, Mv { x: 0, y: 192 }, false),
        None
    );

    // Column window is much wider (MAX_OFFSET_WIDTH >> 3 == 8):
    // base_blk_col = 0, accepted cols are [-8, 16). 9*64 = 576 -> col 14.
    assert_eq!(
        rmvp::get_block_position(rows, cols, 5, 5, Mv { x: 576, y: 0 }, false),
        Some((5, 14))
    );
    // 12*64 = 768 -> col 17, past +16.
    assert_eq!(
        rmvp::get_block_position(rows, cols, 5, 5, Mv { x: 768, y: 0 }, false),
        None
    );

    // Frame bound: a negative row is rejected before the window test.
    assert_eq!(
        rmvp::get_block_position(rows, cols, 0, 0, Mv { x: 0, y: -64 }, false),
        None
    );
    // ... and so is col >= (mi_cols >> 1). With mi_cols = 16 the grid is 8
    // wide, so col 9 is out of frame even though it is inside the window.
    assert_eq!(
        rmvp::get_block_position(64, 16, 5, 5, Mv { x: 256, y: 0 }, false),
        None
    );
    // The same offset inside a 64-wide frame is accepted.
    assert_eq!(
        rmvp::get_block_position(64, 64, 5, 5, Mv { x: 256, y: 0 }, false),
        Some((5, 9))
    );
}

// ---------------------------------------------------------------------------
// motion_field_projection / av1_setup_motion_field
// ---------------------------------------------------------------------------

const MI_ROWS: i32 = 16;
const MI_COLS: i32 = 16;
const MI_STRIDE: i32 = 16;
const TPL_STRIDE: i32 = MI_STRIDE / 2; // 8
const TPL_CELLS: usize = (((MI_ROWS + 32) >> 1) * TPL_STRIDE) as usize;

fn empty_tpl() -> Vec<rmvp::TplMvRef> {
    vec![rmvp::TplMvRef::default(); TPL_CELLS]
}

fn oh() -> rmvp::OrderHintInfo {
    rmvp::OrderHintInfo {
        enable_order_hint: true,
        order_hint_bits: 5,
    }
}

/// A reference motion field whose cell (blk_row, blk_col) carries `mv`
/// pointing at `ref_frame`; every other cell is intra.
fn one_cell_field(blk_row: i32, blk_col: i32, mv: Mv, ref_frame: i8) -> Vec<rmvp::MvRef> {
    let mvs_rows = (MI_ROWS + 1) >> 1;
    let mvs_cols = (MI_COLS + 1) >> 1;
    let mut v = vec![
        rmvp::MvRef {
            mv: Mv::default(),
            ref_frame: 0, // INTRA_FRAME -> skipped
        };
        (mvs_rows * mvs_cols) as usize
    ];
    v[(blk_row * mvs_cols + blk_col) as usize] = rmvp::MvRef { mv, ref_frame };
    v
}

#[test]
fn motion_field_projection_traced() {
    // Start frame = LAST (order_hint 10), current frame order_hint 12, so
    // start_to_current = get_relative_dist(10, 12) = -2, and dir == 2 flips
    // it to +2 (this is exactly how C calls it for LAST_FRAME).
    //
    // The start frame's own LAST reference sits at order_hint 8, so
    // ref_offset[LAST] = get_relative_dist(10, 8) = 2 > 0 -> pos_valid.
    //
    // The cell's MV is (x=0, y=128); the projection is
    // get_mv_projection(mv, num = 2, den = 2) = mv (div_mult[2] = 8192,
    // 128*2*8192 = 2_097_152, (+8192) >> 14 = 128).
    //
    // get_block_position(blk_row = 2, blk_col = 2, this_mv = (0,128),
    // sign_bias = dir>>1 = 1) -> row_offset = 2, row = 2 - 2 = 0, col = 2.
    // base_blk_row = 0 so row 0 is inside [0, 8); col 2 inside [-8, 16).
    // -> tpl_mvs[0 * 8 + 2] = { mfmv0 = the RAW fwd mv (0,128),
    //                           ref_frame_offset = 2 }.
    let mut tpl = empty_tpl();
    let mut ref_order_hint = [0i32; 7];
    ref_order_hint[0] = 8; // LAST of the start frame
    let field = one_cell_field(2, 2, Mv { x: 0, y: 128 }, 1);
    let start = rmvp::RefMotionField {
        mvs: &field,
        order_hint: 10,
        ref_order_hint,
        is_intra_only: false,
        mi_rows: MI_ROWS,
        mi_cols: MI_COLS,
    };
    let ret = rmvp::motion_field_projection(
        &mut tpl,
        TPL_STRIDE,
        MI_ROWS,
        MI_COLS,
        12,
        oh(),
        Some(&start),
        2,
    );
    assert_eq!(ret, 1, "projection should report success");
    let hit = &tpl[2];
    assert_eq!(
        hit.mfmv0,
        Mv { x: 0, y: 128 },
        "mfmv0 stores the RAW reference MV, not the projected one"
    );
    assert_eq!(hit.ref_frame_offset, 2);
    // Every other cell stays INVALID.
    assert_eq!(
        tpl.iter()
            .filter(|t| t.mfmv0.as_int() != 0x8000_8000)
            .count(),
        1,
        "exactly one cell should have been written"
    );

    // A KEY / INTRA_ONLY start frame is refused outright.
    let mut tpl = empty_tpl();
    let start = rmvp::RefMotionField {
        mvs: &field,
        order_hint: 10,
        ref_order_hint,
        is_intra_only: true,
        mi_rows: MI_ROWS,
        mi_cols: MI_COLS,
    };
    assert_eq!(
        rmvp::motion_field_projection(
            &mut tpl,
            TPL_STRIDE,
            MI_ROWS,
            MI_COLS,
            12,
            oh(),
            Some(&start),
            2
        ),
        0
    );
    assert!(tpl.iter().all(|t| t.mfmv0.as_int() == 0x8000_8000));

    // A resolution mismatch is refused (AV1 spec 7.9.2).
    let mut tpl = empty_tpl();
    let start = rmvp::RefMotionField {
        mvs: &field,
        order_hint: 10,
        ref_order_hint,
        is_intra_only: false,
        mi_rows: MI_ROWS * 2,
        mi_cols: MI_COLS,
    };
    assert_eq!(
        rmvp::motion_field_projection(
            &mut tpl,
            TPL_STRIDE,
            MI_ROWS,
            MI_COLS,
            12,
            oh(),
            Some(&start),
            2
        ),
        0
    );

    // A NULL start frame is refused.
    let mut tpl = empty_tpl();
    assert_eq!(
        rmvp::motion_field_projection(&mut tpl, TPL_STRIDE, MI_ROWS, MI_COLS, 12, oh(), None, 2),
        0
    );

    // ref_frame_offset <= 0 kills pos_valid: put the start frame's LAST
    // reference at the SAME order hint (offset 0), and nothing is written
    // even though the walk runs.
    let mut tpl = empty_tpl();
    let mut same = [0i32; 7];
    same[0] = 10;
    let start = rmvp::RefMotionField {
        mvs: &field,
        order_hint: 10,
        ref_order_hint: same,
        is_intra_only: false,
        mi_rows: MI_ROWS,
        mi_cols: MI_COLS,
    };
    assert_eq!(
        rmvp::motion_field_projection(
            &mut tpl,
            TPL_STRIDE,
            MI_ROWS,
            MI_COLS,
            12,
            oh(),
            Some(&start),
            2
        ),
        1,
        "the function still returns 1; only the per-cell write is skipped"
    );
    assert!(tpl.iter().all(|t| t.mfmv0.as_int() == 0x8000_8000));
}

#[test]
fn setup_motion_field_traced() {
    // ref_frame_side is computed even when use_ref_frame_mvs is 0, and the
    // tpl field is left ALONE in that case (C returns before the reset
    // loop) — a pre-existing field must survive.
    let mut tpl = empty_tpl();
    tpl[0].mfmv0 = Mv { x: 7, y: 7 };
    let refs = rmvp::MotionFieldRefs {
        refs: [None, None, None, None, None, None, None],
    };
    let side = rmvp::setup_motion_field(
        &mut tpl,
        TPL_STRIDE,
        MI_ROWS,
        MI_COLS,
        12,
        oh(),
        false,
        &refs,
    );
    assert_eq!(
        tpl[0].mfmv0,
        Mv { x: 7, y: 7 },
        "use_ref_frame_mvs == 0 must return BEFORE the tpl reset"
    );
    // Every ref is absent -> order_hint reads as 0, which is 12 behind the
    // current hint, so get_relative_dist(0, 12) = -12 -> side 0.
    assert_eq!(side, [0i8; 8]);

    // Order-hint disabled -> the whole function is a no-op.
    let off = rmvp::OrderHintInfo {
        enable_order_hint: false,
        order_hint_bits: 5,
    };
    let mut tpl = empty_tpl();
    tpl[0].mfmv0 = Mv { x: 7, y: 7 };
    let side =
        rmvp::setup_motion_field(&mut tpl, TPL_STRIDE, MI_ROWS, MI_COLS, 12, off, true, &refs);
    assert_eq!(side, [0i8; 8]);
    assert_eq!(tpl[0].mfmv0, Mv { x: 7, y: 7 });

    // ref_frame_side: 1 when the reference is AHEAD of the current frame,
    // -1 when it is at the same order hint, 0 when behind.
    // Current hint 12; BWDREF at 20 -> dist(20,12) = 8 > 0 -> 1.
    //                  ALTREF2 at 12 -> equal -> -1.
    //                  LAST at 4 -> dist(4,12) = -8 -> 0.
    let field = one_cell_field(2, 2, Mv { x: 0, y: 128 }, 1);
    let mk = |order_hint: i32| rmvp::RefMotionField {
        mvs: &field,
        order_hint,
        ref_order_hint: [0i32; 7],
        is_intra_only: true, // keep the projection itself inert here
        mi_rows: MI_ROWS,
        mi_cols: MI_COLS,
    };
    let refs = rmvp::MotionFieldRefs {
        refs: [
            Some(mk(4)),  // LAST     = 1
            None,         // LAST2    = 2
            None,         // LAST3    = 3
            None,         // GOLDEN   = 4
            Some(mk(20)), // BWDREF   = 5
            Some(mk(12)), // ALTREF2  = 6
            None,         // ALTREF   = 7
        ],
    };
    let mut tpl = empty_tpl();
    let side = rmvp::setup_motion_field(
        &mut tpl,
        TPL_STRIDE,
        MI_ROWS,
        MI_COLS,
        12,
        oh(),
        true,
        &refs,
    );
    assert_eq!(side[1], 0, "LAST is behind");
    assert_eq!(side[5], 1, "BWDREF is ahead");
    assert_eq!(side[6], -1, "ALTREF2 is coincident");
    assert_eq!(side[2], 0, "an absent ref reads order_hint 0 -> behind");
    // use_ref_frame_mvs = 1 -> the reset loop ran, so the field is INVALID
    // everywhere (all the refs above are intra-only, so nothing projects).
    assert!(tpl.iter().all(|t| t.mfmv0.as_int() == 0x8000_8000));

    // End-to-end: one live LAST reference actually writes a tpl cell,
    // with the same arithmetic pinned in motion_field_projection_traced.
    let mut ref_order_hint = [0i32; 7];
    ref_order_hint[0] = 8;
    // LAST's own ALTREF hint must NOT equal the (absent, so 0) GOLDEN hint,
    // or `is_lst_overlay` fires and the projection is skipped — measured:
    // with both at 0 this sub-case writes ZERO cells.
    ref_order_hint[6] = 25;
    let live = rmvp::RefMotionField {
        mvs: &field,
        order_hint: 10,
        ref_order_hint,
        is_intra_only: false,
        mi_rows: MI_ROWS,
        mi_cols: MI_COLS,
    };
    let refs = rmvp::MotionFieldRefs {
        refs: [Some(live), None, None, None, None, None, None],
    };
    let mut tpl = empty_tpl();
    rmvp::setup_motion_field(
        &mut tpl,
        TPL_STRIDE,
        MI_ROWS,
        MI_COLS,
        12,
        oh(),
        true,
        &refs,
    );
    assert_eq!(
        tpl.iter()
            .filter(|t| t.mfmv0.as_int() != 0x8000_8000)
            .count(),
        1,
        "the LAST_FRAME projection (dir = 2) must have written exactly one cell"
    );
    assert_eq!(tpl[2].mfmv0, Mv { x: 0, y: 128 });
    assert_eq!(tpl[2].ref_frame_offset, 2);
}

/// C's `is_lst_overlay` guard: when the LAST reference's own ALTREF order
/// hint equals the GOLDEN reference's order hint, the LAST projection is
/// SKIPPED (but `ref_stamp` is still decremented).
#[test]
fn setup_motion_field_lst_overlay_traced() {
    let field = one_cell_field(2, 2, Mv { x: 0, y: 128 }, 1);
    // LAST's ref_order_hint[ALTREF - LAST] = index 6.
    let mut last_hints = [0i32; 7];
    last_hints[0] = 8; // LAST's own LAST
    last_hints[6] = 30; // LAST's ALTREF
    let last = rmvp::RefMotionField {
        mvs: &field,
        order_hint: 10,
        ref_order_hint: last_hints,
        is_intra_only: false,
        mi_rows: MI_ROWS,
        mi_cols: MI_COLS,
    };
    // GOLDEN present with order_hint 30 -> is_lst_overlay -> skip.
    let golden = rmvp::RefMotionField {
        mvs: &field,
        order_hint: 30,
        ref_order_hint: [0i32; 7],
        is_intra_only: true,
        mi_rows: MI_ROWS,
        mi_cols: MI_COLS,
    };
    let refs = rmvp::MotionFieldRefs {
        refs: [Some(last), None, None, Some(golden), None, None, None],
    };
    let mut tpl = empty_tpl();
    rmvp::setup_motion_field(
        &mut tpl,
        TPL_STRIDE,
        MI_ROWS,
        MI_COLS,
        12,
        oh(),
        true,
        &refs,
    );
    assert!(
        tpl.iter().all(|t| t.mfmv0.as_int() == 0x8000_8000),
        "is_lst_overlay must suppress the LAST_FRAME projection"
    );
}
