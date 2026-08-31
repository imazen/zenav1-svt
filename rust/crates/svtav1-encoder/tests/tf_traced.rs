//! TIER 4 vectors for the `static` temporal-filtering helpers that no exported
//! C symbol reaches (`docs/WORKING-ON-THIS.md` §4: "hand-derived vectors traced
//! against the C source. The weakest tier. Use only when the C function is
//! `static` with no exported symbol, and say so.").
//!
//! Saying so, precisely. These three are `static` in
//! `Source/Lib/Codec/temporal_filtering.c` with no symbol in
//! `libSvtAv1Enc.a` (checked with `nm -g`, not inferred from a header), and
//! their only caller is `produce_temporally_filtered_pic`, which takes a
//! `PictureParentControlSet**` list plus `MotionEstimationContext_t` and
//! segment state — not something a facade shim can stand up:
//!
//! * `svt_av1_calculate_decay_factor`  (temporal_filtering.c:589)
//! * `calculate_tf_shift_factor`       (temporal_filtering.c:610)
//! * `derive_tf_32x32_block_split_flag`(temporal_filtering.c:152)
//! * `convert_64x64_info_to_32x32_info`(temporal_filtering.c:2509), MV half
//!
//! Everything else in `port_temporal_filtering` IS tier 1 — see
//! `c_parity_temporal_filtering.rs`. In particular `sqrt_fast`,
//! `calculate_squared_errors_sum` and `calculate_squared_errors_sum_highbd`
//! are also `static` but ARE reached at tier 1, through the exported
//! `svt_av1_apply_temporal_filter_planewise_medium_c` which computes every
//! distance term and window error through them.
//!
//! Every expected value below is annotated with the arithmetic it was traced
//! from, so a reader can check it against the C line rather than against this
//! file.

use svtav1_encoder::port_temporal_filtering as port;

/// `calculate_tf_shift_factor` (temporal_filtering.c:610) — TIER 4.
///
/// ```c
/// const uint64_t block_err = ctx->tf_64x64_block_error >> 12;
/// if (block_err < LOW_ERROR_THRESHOLD) return 14;      // 200
/// else if (block_err < MED_ERROR_THRESHOLD) return 13; // 2000
/// return 12;
/// ```
#[test]
fn tier4_calculate_tf_shift_factor_boundaries() {
    // block_err = err >> 12, so the boundaries in `err` are 200 << 12 and
    // 2000 << 12.
    assert_eq!(port::calculate_tf_shift_factor(0), 14);
    assert_eq!(port::calculate_tf_shift_factor(4095), 14); // >> 12 == 0
    assert_eq!(port::calculate_tf_shift_factor((200 << 12) - 1), 14); // 199
    assert_eq!(port::calculate_tf_shift_factor(200 << 12), 13); // 200
    assert_eq!(port::calculate_tf_shift_factor((2000 << 12) - 1), 13); // 1999
    assert_eq!(port::calculate_tf_shift_factor(2000 << 12), 12); // 2000
    assert_eq!(port::calculate_tf_shift_factor(u64::MAX), 12);
}

/// `svt_av1_calculate_decay_factor` (temporal_filtering.c:589) — TIER 4.
///
/// ```c
/// tf_decay_factor_fp16[Y] = (uint32_t)((((int64_t)n * (int64_t)n) * q_decay_fp8) >> shift_factor);
/// if (tf_chroma) {
///   n = (decay_control_cu * (const_0dot7_fp16 + noise[U])) / (1 << 6);
///   tf_decay_factor_fp16[U] = ... same shape ...
///   n = (decay_control_cv * (const_0dot7_fp16 + noise[V])) / (1 << 6);
///   tf_decay_factor_fp16[V] = ...
/// }
/// ```
#[test]
fn tier4_calculate_decay_factor_traced() {
    // Y only: n = 1000, q = 256, shift = 12.
    //   (1000 * 1000 * 256) >> 12 = 256_000_000 >> 12 = 62_500
    let mut dec = [0u32; 3];
    let mut n = 1000i32;
    let noise = [0i32, 0, 0];
    port::calculate_decay_factor(&mut dec, &mut n, 256, 0, 0, 0, &noise, 12, false);
    assert_eq!(dec[0], 62_500);
    assert_eq!(dec[1], 0, "chroma untouched when tf_chroma is false");
    assert_eq!(dec[2], 0);
    assert_eq!(n, 1000, "n_decay is not rewritten when tf_chroma is false");

    // With chroma. const_0dot7_fp16 = 45875 (0.7 in Q16), noise[U] = 65536,
    // noise[V] = -32768, decay_control_cu = 3, decay_control_cv = 5.
    //   n_u = (3 * (45875 + 65536)) / 64 = 334233 / 64 = 5222   (C integer div)
    //   dec[U] = (5222 * 5222 * 256) >> 12 = 6_980_936_704 >> 12 = 1_704_330
    //   n_v = (5 * (45875 - 32768)) / 64 = 65535 / 64 = 1023
    //   dec[V] = (1023 * 1023 * 256) >> 12 = 267_911_424 >> 12 = 65_408
    let mut dec = [0u32; 3];
    let mut n = 1000i32;
    let noise = [0i32, 65536, -32768];
    port::calculate_decay_factor(&mut dec, &mut n, 256, 3, 5, 45875, &noise, 12, true);
    assert_eq!(dec[0], 62_500, "Y still uses the caller's incoming n_decay");
    assert_eq!(dec[1], 1_704_330);
    assert_eq!(dec[2], 65_408);
    assert_eq!(n, 1023, "n_decay is left holding the V value");
}

/// `derive_tf_32x32_block_split_flag` (temporal_filtering.c:152) — TIER 4.
#[test]
fn tier4_derive_tf_32x32_block_split_flag_traced() {
    // The INT_MAX sentinel arm: block_error == INT_MAX means the block was
    // never motion-searched, so nothing splits.
    let mut ctx = port::TfSplitCtx {
        idx_32x32: 2,
        tf_32x32_block_split_flag: [1, 1, 1, 1],
        tf_16x16_block_split_flag: [[1; 4]; 4],
        ..Default::default()
    };
    ctx.tf_32x32_block_error[2] = i32::MAX as u64;
    port::derive_tf_32x32_block_split_flag(&mut ctx);
    assert_eq!(ctx.tf_32x32_block_split_flag, [1, 1, 0, 1]);
    assert_eq!(ctx.tf_16x16_block_split_flag[2], [0; 4]);
    assert_eq!(
        ctx.tf_16x16_block_split_flag[0], [1; 4],
        "other rows untouched"
    );

    // No-split: `block_error * 14 < sum_subblock_error * 16`.
    // sum = 4 * 1000 = 4000; 4000 * 16 = 64000; block_error * 14 < 64000
    // holds for block_error <= 4571 (4571*14 = 63994).
    let mut ctx = port::TfSplitCtx {
        idx_32x32: 0,
        ..Default::default()
    };
    ctx.tf_32x32_block_error[0] = 4571;
    for i in 0..4 {
        ctx.tf_16x16_block_error[i] = 1000;
    }
    port::derive_tf_32x32_block_split_flag(&mut ctx);
    assert_eq!(
        ctx.tf_32x32_block_split_flag[0], 0,
        "4571*14 = 63994 < 64000"
    );

    // Do split: 4572 * 14 = 64008 >= 64000.
    ctx.tf_32x32_block_error[0] = 4572;
    port::derive_tf_32x32_block_split_flag(&mut ctx);
    assert_eq!(ctx.tf_32x32_block_split_flag[0], 1);

    // The 8x8 branch. enable_8x8_pred on, idx 0, sub-block 1:
    //   error_8x8 = 100 + 100 + 100 + 100 = 400
    //   subblock_errors[1] = 1000; 1000 * 8 = 8000 < 400 * 16 = 6400? NO
    //   -> split, and tf_16x16_block_error[1] is OVERWRITTEN with 400.
    let mut ctx = port::TfSplitCtx {
        idx_32x32: 0,
        enable_8x8_pred: true,
        ..Default::default()
    };
    ctx.tf_32x32_block_error[0] = 1;
    for i in 0..4 {
        ctx.tf_16x16_block_error[i] = 1000;
    }
    for i in 0..16 {
        ctx.tf_8x8_block_error[i] = 100;
    }
    port::derive_tf_32x32_block_split_flag(&mut ctx);
    assert_eq!(ctx.tf_16x16_block_split_flag[0], [1, 1, 1, 1]);
    assert_eq!(
        &ctx.tf_16x16_block_error[..4],
        &[400u64; 4],
        "the 8x8 branch overwrites the 16x16 error the filter later weights on"
    );
    // sum_subblock_error is now 4 * 400 = 1600; 1 * 14 < 1600 * 16 -> no split.
    assert_eq!(ctx.tf_32x32_block_split_flag[0], 0);

    // And the 16x16 no-split side: error_8x8 = 4 * 100 = 400,
    // subblock_errors = 100; 100 * 8 = 800 < 400 * 16 = 6400 -> no split, and
    // the 16x16 error is NOT overwritten.
    let mut ctx = port::TfSplitCtx {
        idx_32x32: 0,
        enable_8x8_pred: true,
        ..Default::default()
    };
    ctx.tf_32x32_block_error[0] = 1;
    for i in 0..4 {
        ctx.tf_16x16_block_error[i] = 100;
    }
    for i in 0..16 {
        ctx.tf_8x8_block_error[i] = 100;
    }
    port::derive_tf_32x32_block_split_flag(&mut ctx);
    assert_eq!(ctx.tf_16x16_block_split_flag[0], [0, 0, 0, 0]);
    assert_eq!(&ctx.tf_16x16_block_error[..4], &[100u64; 4]);
}

/// `convert_64x64_info_to_32x32_info` (temporal_filtering.c:2509), MV half —
/// TIER 4.
#[test]
fn tier4_convert_64x64_info_to_32x32_info_mvs() {
    let mut mv_x = [7i16; 4];
    let mut mv_y = [-9i16; 4];
    let mut split32 = [1i32; 4];
    let mut split16 = [[1i32; 4]; 4];
    port::convert_64x64_info_to_32x32_info_mvs(
        -12,
        34,
        &mut mv_x,
        &mut mv_y,
        &mut split32,
        &mut split16,
    );
    assert_eq!(mv_x, [-12i16; 4]);
    assert_eq!(mv_y, [34i16; 4]);
    assert_eq!(split32, [0i32; 4]);
    // C memsets sizeof(flag[0][0]) * 4 * 4 bytes — the WHOLE 4x4 array, not
    // one row.
    assert_eq!(split16, [[0i32; 4]; 4]);
}

/// `sqrt_fast` (temporal_filtering.c:655) is `static`, but the medium-kernel
/// differential in `c_parity_temporal_filtering.rs` drives it at TIER 1
/// through every distance term. This test is documentation of the shape, not
/// the gate: it pins the deliberate 10%-error behaviour so a future "fix" to a
/// correct `isqrt` fails loudly here as well as in the parity test.
#[test]
fn sqrt_fast_is_deliberately_approximate() {
    // x <= 15 takes the table arm directly: sqrt_array_fp16[x] >> 16.
    assert_eq!(port::sqrt_fast(0), 0);
    assert_eq!(port::sqrt_fast(1), 1); // 65536 >> 16
    assert_eq!(port::sqrt_fast(4), 2); // 131072 >> 16
    assert_eq!(port::sqrt_fast(15), 3); // 253819 >> 16 == 3, not 3.87
    // x = 16: log2_half = 4 >> 1 = 2, mul2 = 4, base = 16 >> 2 = 4,
    //   sqrt_array_fp16[4] >> (17 - 2) = 131072 >> 15 = 4. Exact here.
    assert_eq!(port::sqrt_fast(16), 4);
    // x = 100: log2f(100) = 6, log2_half = 3, mul2 = 6, base = 100 >> 4 = 6,
    //   sqrt_array_fp16[6] >> (17 - 3) = 160529 >> 14 = 9. True sqrt is 10 —
    //   this is the documented 10% error, and it must NOT be "fixed".
    assert_eq!(port::sqrt_fast(100), 9);
}

// ---------------------------------------------------------------------------
// The sub-pel search chain — TIER 4, and here is exactly why
// ---------------------------------------------------------------------------
//
// `svt_check_position`, `tf_subpel_search` and the four
// `tf_<size>_sub_pel_search` wrappers are `static` in temporal_filtering.c and
// take `PictureParentControlSet*` + `MeContext*` + `BlkStruct*` +
// `EbPictureBufferDesc*`, and they call `svt_aom_simple_luma_unipred` (the
// motion compensator) and `svt_aom_mefn_ptr[bsize].vf` (the variance kernel).
// A facade shim would have to stand up the whole MC path, which is chunk C4's
// surface, not this lane's.
//
// So the port splits them: the CONTROL FLOW — candidate order, early-outs,
// diagonal skip, re-centring, and the strict-improvement tie rule — is ported
// and tested here against vectors traced from the C source, with the
// compensate-and-score step INJECTED. That injection point is the seam where
// the port's own MC and variance kernels (already C-parity gated in
// svtav1-dsp: c_parity_inter_pred.rs, c_parity_variance.rs) plug in.

use std::cell::RefCell;

/// Record every candidate the search asks to score, so the scan ORDER can be
/// asserted, not just the winner.
fn recording_search(
    ctrls: &port::TfSearchCtrls,
    early_exit_th: u32,
    start_mv: (i16, i16),
    start_dist: u64,
    costs: impl Fn(i16, i16) -> u64 + 'static,
) -> (Vec<(i16, i16, u8)>, u64, i16, i16) {
    let seen = RefCell::new(Vec::new());
    let mut p = port::subpel_params_for_block(64, 0, 0, 0, 0, ctrls, 0, 8);
    let mut best_dist = start_dist;
    let mut best_x = start_mv.0;
    let mut best_y = start_mv.1;
    let mut score = |x: i16, y: i16, shift: u8| -> u64 {
        seen.borrow_mut().push((x, y, shift));
        costs(x, y)
    };
    port::subpel_search(
        &mut p,
        ctrls,
        early_exit_th,
        &mut best_dist,
        &mut best_x,
        &mut best_y,
        &mut score,
    );
    (seen.into_inner(), best_dist, best_x, best_y)
}

/// `tf_subpel_search` (temporal_filtering.c:1536) — TIER 4.
///
/// At tf_level 5 the live controls are half_pel_mode = 2, quarter_pel_mode = 1,
/// eight_pel_mode = 0. mode 2 makes `svt_check_position` skip every DIAGONAL
/// candidate, so the half-pel level offers only the four axis-aligned points.
#[test]
fn tier4_subpel_search_candidate_order_at_tf_level5() {
    let ctrls = port::TfSearchCtrls {
        half_pel_mode: 2,
        quarter_pel_mode: 1,
        eight_pel_mode: 0,
        ..Default::default()
    };
    // Constant cost: nothing ever improves on the centre, so the scan runs to
    // completion and the incumbent MV survives (strict-improvement rule).
    let (seen, dist, bx, by) = recording_search(&ctrls, 0, (0, 0), u64::MAX, |_, _| 1000);

    // Centre first, with the subsampling shift; then half-pel axis-aligned
    // only (mode 2 skips diagonals); then quarter-pel, all eight (mode 1).
    // C's loops are `for i in -step..=step` OUTER (x) and `j` INNER (y).
    let expected: Vec<(i16, i16, u8)> = vec![
        (0, 0, 0),
        // half-pel, step 4: i = -4 -> (xd,yd) = (-4,-4) skipped (diagonal),
        // (-4,0) kept, (-4,4) skipped; i = 0 -> (0,-4), (0,0) skipped as the
        // already-searched centre, (0,4); i = 4 -> (4,0) only.
        (-4, 0, 0),
        (0, -4, 0),
        (0, 4, 0),
        (4, 0, 0),
        // quarter-pel, step 2, mode 1: all eight offsets, diagonals included.
        (-2, -2, 0),
        (-2, 0, 0),
        (-2, 2, 0),
        (0, -2, 0),
        (0, 2, 0),
        (2, -2, 0),
        (2, 0, 0),
        (2, 2, 0),
    ];
    assert_eq!(seen, expected, "candidate order or diagonal skip is wrong");
    assert_eq!(dist, 1000);
    assert_eq!((bx, by), (0, 0), "a tie must NOT replace the incumbent");
}

/// The strict-improvement rule and the per-level RE-CENTRING — TIER 4.
#[test]
fn tier4_subpel_search_recentres_each_level_on_the_running_best() {
    let ctrls = port::TfSearchCtrls {
        half_pel_mode: 1,
        quarter_pel_mode: 1,
        eight_pel_mode: 1,
        ..Default::default()
    };
    // Make (4, 0) the unique half-pel winner. The quarter-pel level must then
    // be centred on (4, 0), so it should probe (4 + {-2,0,2}, 0 + {-2,0,2}).
    let (seen, _dist, bx, by) = recording_search(&ctrls, 0, (0, 0), u64::MAX, |x, y| {
        if (x, y) == (4, 0) { 1 } else { 500 }
    });
    assert_eq!((bx, by), (4, 0));
    assert!(
        seen.contains(&(6, 0, 0)) && seen.contains(&(2, 0, 0)),
        "quarter-pel level did not re-centre on the half-pel winner: {seen:?}"
    );
    // And the eighth-pel level re-centres again, on (4, 0).
    assert!(
        seen.contains(&(5, 1, 0)),
        "eighth-pel level did not re-centre: {seen:?}"
    );
}

/// The three early-outs in `svt_check_position` — TIER 4.
#[test]
fn tier4_check_position_early_outs() {
    let ctrls = port::TfSearchCtrls {
        half_pel_mode: 1,
        quarter_pel_mode: 0,
        eight_pel_mode: 0,
        ..Default::default()
    };

    // best_dist == 0: nothing can beat it, so not even the centre is scored.
    let (seen, dist, ..) = recording_search(&ctrls, 0, (3, -5), 0, |_, _| 0);
    assert!(
        seen.is_empty(),
        "a zero incumbent must short-circuit every candidate"
    );
    assert_eq!(dist, 0);

    // tf_subpel_early_exit_th: the threshold is
    // (bsize * bsize * th) << is_highbd = 64 * 64 * 2 = 8192 for bsize 64,
    // 8-bit. An incumbent below it stops the search before scoring.
    let (seen, ..) = recording_search(&ctrls, 2, (0, 0), 8191, |_, _| 1);
    assert!(seen.is_empty(), "8191 < 8192 must early-exit");
    let (seen, ..) = recording_search(&ctrls, 2, (0, 0), 8192, |_, _| 1);
    assert!(
        !seen.is_empty(),
        "8192 is NOT below the threshold and must be searched"
    );
}

/// The block-index derivations — TIER 4. Traced from
/// temporal_filtering.c:1826-1827 / :1935-1938 / :2044-2047 plus the
/// `subblock_xy_*` and `idx_32x32_to_idx_*` tables at :46/:62/:65/:75.
///
/// The tables yield (ROW, COLUMN) and C assigns `idx_y` from index 0 and
/// `idx_x` from index 1; reading them as (x, y) transposes every block origin.
#[test]
fn tier4_subpel_block_index_derivations() {
    // 32x32: idx_x = idx & 1, idx_y = idx >> 1.
    assert_eq!(port::subpel_idx_32x32(0), (0, 0));
    assert_eq!(port::subpel_idx_32x32(1), (1, 0));
    assert_eq!(port::subpel_idx_32x32(2), (0, 1));
    assert_eq!(port::subpel_idx_32x32(3), (1, 1));

    // 16x16, 32x32 quadrant 0: pu indices {0, 1, 4, 5}, whose (row, col) are
    // (0,0) (0,1) (1,0) (1,1) -> (idx_x, idx_y) = (0,0) (1,0) (0,1) (1,1).
    assert_eq!(port::subpel_idx_16x16(0, 0), (0, 0));
    assert_eq!(port::subpel_idx_16x16(0, 1), (1, 0));
    assert_eq!(port::subpel_idx_16x16(0, 2), (0, 1));
    assert_eq!(port::subpel_idx_16x16(0, 3), (1, 1));
    // Quadrant 3: pu indices {10, 11, 14, 15} -> rows 2,2,3,3 cols 2,3,2,3.
    assert_eq!(port::subpel_idx_16x16(3, 0), (2, 2));
    assert_eq!(port::subpel_idx_16x16(3, 3), (3, 3));

    // 8x8, quadrant 0 / 16x16 0: pu indices {0, 1, 8, 9}; subblock_xy_8x8 is
    // 8 wide, so pu 8 is (row 1, col 0) and pu 9 is (row 1, col 1).
    assert_eq!(port::subpel_idx_8x8(0, 0, 0), (0, 0));
    assert_eq!(port::subpel_idx_8x8(0, 0, 1), (1, 0));
    assert_eq!(port::subpel_idx_8x8(0, 0, 2), (0, 1));
    assert_eq!(port::subpel_idx_8x8(0, 0, 3), (1, 1));
    // Quadrant 3 / 16x16 3: {54, 55, 62, 63} -> rows 6,6,7,7 cols 6,7,6,7.
    assert_eq!(port::subpel_idx_8x8(3, 3, 0), (6, 6));
    assert_eq!(port::subpel_idx_8x8(3, 3, 3), (7, 7));
}

/// `apply_filtering_block_plane_wise` offsets (temporal_filtering.c:1289) —
/// TIER 4.
#[test]
fn tier4_apply_filtering_block_plane_wise_offsets() {
    // 4:2:0, a 32x32 block at (block_row 1, block_col 1), source strides
    // distinct from prediction strides so a mixed-up pair shows.
    let stride = [100usize, 50, 51];
    let stride_pred = [64usize, 32, 33];
    let o =
        port::apply_filtering_block_plane_wise_offsets(1, 1, &stride, &stride_pred, 32, 32, 1, 1);
    // src Y   = 1 * 32 * 100 + 1 * 32       = 3232
    // src U   = 1 * 16 * 50  + 1 * 16       = 816
    // src V   = 1 * 16 * 51  + 1 * 16       = 832
    assert_eq!(o.src, [3232, 816, 832]);
    // block Y = 1 * 32 * 64  + 1 * 32       = 2080
    // block U = 1 * 16 * 32  + 1 * 16       = 528
    // block V = 1 * 16 * 33  + 1 * 16       = 544
    assert_eq!(o.block, [2080, 528, 544]);

    // Block (0, 0) is the origin on every plane.
    let o =
        port::apply_filtering_block_plane_wise_offsets(0, 0, &stride, &stride_pred, 64, 64, 1, 1);
    assert_eq!(o.src, [0, 0, 0]);
    assert_eq!(o.block, [0, 0, 0]);
}

/// `set_hme_search_params_mctf` (temporal_filtering.c:2571) — TIER 4.
#[test]
fn tier4_set_hme_search_params_mctf() {
    let default_tf = port::SearchAreaMinMax {
        sa_min: (8, 6),
        sa_max: (64, 32),
    };
    assert_eq!(
        port::set_hme_search_params_mctf(default_tf, 0),
        Some(default_tf)
    );
    // Level 1: sa_min DOUBLES, sa_max QUADRUPLES — not the same shift.
    assert_eq!(
        port::set_hme_search_params_mctf(default_tf, 1),
        Some(port::SearchAreaMinMax {
            sa_min: (16, 12),
            sa_max: (256, 128),
        })
    );
    // C asserts on anything else; the port refuses instead of guessing.
    assert_eq!(port::set_hme_search_params_mctf(default_tf, 2), None);
}

/// `filt_unfilt_dist` (temporal_filtering.c:3922) — TIER 4.
#[test]
fn tier4_filt_unfilt_dist_tiling_and_division() {
    // 130x70 aligned, b64_size 64 -> 3 x 2 = 6 b64 tiles, with the last
    // column 2 px wide and the last row 6 px tall.
    let dims: RefCell<Vec<(u32, u32)>> = RefCell::new(Vec::new());
    let offsets: RefCell<Vec<(usize, usize)>> = RefCell::new(Vec::new());
    let d = port::filt_unfilt_dist(130, 70, 64, 200, 300, |fo, _fs, uo, _us, w, h| {
        dims.borrow_mut().push((w, h));
        offsets.borrow_mut().push((fo, uo));
        // 60 per tile -> total 360, divided by the TILE COUNT 6 -> 60.
        60
    });
    assert_eq!(d, 60, "the divisor is the b64 COUNT, not the pixel count");
    assert_eq!(
        dims.into_inner(),
        vec![(64, 64), (64, 64), (2, 64), (64, 6), (64, 6), (2, 6)],
        "partial b64 clamping against the ALIGNED dimensions is wrong"
    );
    // Tile (row 1, col 2): filt offset = 64 * 200 + 128 = 12928;
    // unfilt offset = 64 * 300 + 128 = 19328.
    assert_eq!(offsets.into_inner()[5], (12928, 19328));
}
