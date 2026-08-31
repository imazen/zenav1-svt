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
