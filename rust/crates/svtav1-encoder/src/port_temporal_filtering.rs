//! Wholesale port of `Source/Lib/Codec/temporal_filtering.c` — the RANDOM_ACCESS
//! temporal filter that rewrites the source picture before it is encoded.
//!
//! ## Reachability, measured — read this before deciding it is dead code
//!
//! TF is bit-affecting on the VIDEO-MODE KEY FRAME in RANDOM_ACCESS and INERT
//! in LOW_DELAY. `derive_tf_params` (enc_handle.c:3338-3343) returns early with
//! `tf_level = 0` for LOW_DELAY ("TF disabled for all LD"), and `do_tf`
//! separately requires `hierarchical_levels >= 1`. Measured 2026-08-31 on a
//! 128x128 gradient, q40 preset 6:
//! * LD/hier0, 2 frames: `SVT_FORK_TF_STRENGTH` 0 vs 3 gives 496 B / 527 B for
//!   the same frame either way — TF provably contributes nothing;
//! * RA/AUTO, 2 frames: `SVT_FORK_TF_STRENGTH=0` (shift 14, kf TF off) gives
//!   frame 0 = 496 B, the default 3 gives 495 B — TF IS bit-affecting.
//!
//! So this module is required for key-frame parity the moment the campaign
//! moves off LOW_DELAY, and inert before then. Per `WORKING-ON-THIS.md` §7,
//! dead-looking C stays translated.
//!
//! ## The SVT_HDR_MODE trap, which fired here
//!
//! `temporal_filtering.c:2782-2794` reads `kf_tf_strength` ONLY under
//! `#if SVT_HDR_MODE`. The MAINLINE `#else` — the arm the oracle compiles —
//! sets `kf_tf_shift_factor = tf_shift_factor` and bumps it by 1 only under
//! Tune VQ sharpness controls. Confirmed empirically before it was read:
//! sweeping `SVT_FORK_KF_TF_STRENGTH` over 0..4 at RA 2f/8f/16f leaves frame 0
//! BYTE-IDENTICAL, while sweeping `SVT_FORK_TF_STRENGTH` moves it. Anyone
//! porting the kf shift factor from the fork arm passes their own differential
//! and is wrong. The mainline knob is `tf_strength` (default 3,
//! enc_settings.c:1159; the fork default is 1).
//!
//! ## Dead arms, named so nobody ports them by mistake
//!
//! `use_zz_based_filter` is set only by `tf_ld_controls` levels 1 and 2, which
//! `derive_tf_params` never selects — so the `zz` kernels are unreachable and
//! the `medium` kernels are the live ones. `enable_8x8_pred` is 0 at
//! `tf_level 5` (presets 3..7), so the 8x8 refinement level is off there and
//! live only at tf_level 1/2 (presets 0..2).
//!
//! Evidence tiers are stated per function. Exported symbols are gated at
//! tier 1 through `svtav1-cref`; `static` C functions that a shim can reach
//! with flat arguments are ALSO tier 1 via a facade shim, and the few that
//! take deep PCS pointers say so explicitly.

/// `TF_BW` / `TF_BH` (temporal_filtering.h:23-24).
pub const TF_BW: usize = 64;
pub const TF_BH: usize = 64;
/// `TF_PLANEWISE_FILTER_WEIGHT_SCALE` (temporal_filtering.h:28).
pub const TF_PLANEWISE_FILTER_WEIGHT_SCALE: u32 = 1000;
/// `TF_WEIGHT_SCALE` (temporal_filtering.h:33).
pub const TF_WEIGHT_SCALE: u32 = 1000;
/// `TF_WINDOW_BLOCK_BALANCE_WEIGHT` (temporal_filtering.h:37).
pub const TF_WINDOW_BLOCK_BALANCE_WEIGHT: u32 = 5;
/// `LOW_ERROR_THRESHOLD` / `MED_ERROR_THRESHOLD` (temporal_filtering.h:65-66).
pub const LOW_ERROR_THRESHOLD: u64 = 200;
pub const MED_ERROR_THRESHOLD: u64 = 2000;

/// `log1p(x)` for x in [-1..6], step 1/32, Q16 (temporal_filtering.c:337).
/// 225 entries; the first is `INT32_MIN` (C spells it `-2147483647 - 1`).
#[rustfmt::skip]
const LOG1P_TAB_FP16: [i32; 225] = [
    i32::MIN, -227130, -181704, -155131, -136278, -121654, -109705, -99603,
    -90852, -83133, -76228, -69982, -64279, -59033, -54177, -49655,
    -45426, -41452, -37707, -34163, -30802, -27604, -24555, -21642,
    -18853, -16178, -13607, -11134, -8751, -6451, -4229, -2080,
    0, 2016, 3973, 5872, 7719, 9514, 11262, 12964,
    14623, 16242, 17821, 19363, 20870, 22342, 23783, 25192,
    26572, 27923, 29247, 30545, 31818, 33066, 34291, 35494,
    36674, 37834, 38974, 40095, 41196, 42279, 43345, 44394,
    45426, 46442, 47442, 48428, 49399, 50355, 51298, 52228,
    53145, 54049, 54940, 55820, 56688, 57545, 58390, 59225,
    60050, 60864, 61668, 62462, 63247, 64023, 64789, 65547,
    66296, 67036, 67769, 68493, 69209, 69917, 70618, 71312,
    71998, 72677, 73349, 74015, 74673, 75326, 75971, 76611,
    77244, 77871, 78492, 79108, 79717, 80321, 80920, 81513,
    82101, 82683, 83261, 83833, 84400, 84963, 85521, 86074,
    86622, 87166, 87705, 88240, 88771, 89297, 89820, 90338,
    90852, 91362, 91868, 92370, 92868, 93363, 93854, 94341,
    94825, 95305, 95782, 96255, 96725, 97191, 97654, 98114,
    98571, 99024, 99475, 99922, 100366, 100808, 101246, 101681,
    102114, 102544, 102971, 103395, 103816, 104235, 104651, 105065,
    105476, 105884, 106290, 106693, 107094, 107492, 107888, 108282,
    108673, 109062, 109449, 109833, 110215, 110595, 110973, 111348,
    111722, 112093, 112462, 112830, 113195, 113558, 113919, 114278,
    114635, 114990, 115344, 115695, 116044, 116392, 116738, 117082,
    117424, 117765, 118103, 118440, 118776, 119109, 119441, 119771,
    120100, 120426, 120752, 121075, 121397, 121718, 122037, 122354,
    122670, 122984, 123297, 123608, 123918, 124227, 124534, 124839,
    125143, 125446, 125747, 126047, 126346, 126643, 126939, 127233,
    127527,
];

/// `exp(-x/16)` for x in [0..7], step 1/16, Q16 (temporal_filtering.c:670).
#[rustfmt::skip]
const EXPF_TAB_FP16: [i32; 129] = [
    65536, 61565, 57835, 54331, 51039, 47947, 45042, 42313, 39749, 37341, 35078, 32953, 30957,
    29081, 27319, 25664, 24109, 22648, 21276, 19987, 18776, 17638, 16570, 15566, 14623, 13737,
    12904, 12122, 11388, 10698, 10050, 9441, 8869, 8331, 7827, 7352, 6907, 6488, 6095,
    5726, 5379, 5053, 4747, 4459, 4189, 3935, 3697, 3473, 3262, 3065, 2879, 2704,
    2541, 2387, 2242, 2106, 1979, 1859, 1746, 1640, 1541, 1447, 1360, 1277, 1200,
    1127, 1059, 995, 934, 878, 824, 774, 728, 683, 642, 603, 566, 532,
    500, 470, 441, 414, 389, 366, 343, 323, 303, 285, 267, 251, 236,
    222, 208, 195, 184, 172, 162, 152, 143, 134, 126, 118, 111, 104,
    98, 92, 86, 81, 76, 72, 67, 63, 59, 56, 52, 49, 46,
    43, 41, 38, 36, 34, 31, 30, 28, 26, 24, 23, 21,
];

/// `sqrt(i) * 65536` for i in 0..16 (temporal_filtering.c:684).
#[rustfmt::skip]
const SQRT_ARRAY_FP16: [u32; 16] = [
    0, 65536, 92681, 113511, 131072, 146542, 160529, 173391,
    185363, 196608, 207243, 217358, 227023, 236293, 245213, 253819,
];

/// `svt_aom_noise_log1p_fp16` (temporal_filtering.c:568). EXPORTED — TIER 1.
///
/// Turns a Q16 noise estimate into `noise_levels_log1p_fp16`, which drives
/// `tf_chroma` selection, `ref_pics_modulation` (pd_process.c:3642) and
/// `pcs->is_noise_level`. Three arms: the `<= 0` sentinel, a table lookup with
/// linear interpolation over the low 11 bits, and a linear approximation
/// `y = 1860*x + 116456` above 7.0.
///
/// Note the interpolation reads `id + 1`, so `base_fp16 < 458752` (= 7.0 in
/// Q16) is what keeps the index in range: `458752 >> 11 == 224`, the last
/// element, and the arm is only taken for strictly smaller values.
pub fn noise_log1p_fp16(noise_level_fp16: i32) -> i32 {
    let base_fp16 = 65536i32.wrapping_add(noise_level_fp16);
    if base_fp16 <= 0 {
        i32::MIN
    } else if base_fp16 < 458752 {
        let id = (base_fp16 >> 11) as usize;
        let rest = base_fp16 & 0x7FF;
        // WRAPPING, deliberately. `LOG1P_TAB_FP16[0]` is `INT32_MIN`, so for
        // `id == 0` the C expression `tab[1] - tab[0]` overflows a signed int
        // and the subsequent multiply and add overflow with it. C's behaviour
        // there is nominally UB; the built oracle wraps, and byte-identity
        // means reproducing what the oracle does. `noise_log1p_fp16_matches_c`
        // probes `id == 0` at rest 0/1/1023/2047 specifically, so this is a
        // gated claim rather than an assumption.
        let diff = rest.wrapping_mul(LOG1P_TAB_FP16[id + 1].wrapping_sub(LOG1P_TAB_FP16[id])) >> 11;
        LOG1P_TAB_FP16[id].wrapping_add(diff)
    } else {
        (1860i32.wrapping_mul(noise_level_fp16 >> 8).wrapping_shr(8)).wrapping_add(116456)
    }
}

/// `sqrt_fast` (temporal_filtering.c:655). `static` in C — gated at TIER 1
/// through a facade shim.
///
/// Deliberately NOT a correct integer square root: the C comment records a
/// linear max error of 10%. It must be transcribed exactly, never replaced
/// with `isqrt` — every temporal-filter weight is derived from its output.
///
/// `svt_log2f` is `get_msb` (definitions.h:613), i.e. `floor(log2(x))`.
pub fn sqrt_fast(x: u32) -> u32 {
    if x > 15 {
        let log2_half = (x.ilog2() >> 1) as i32;
        let mul2 = log2_half << 1;
        let base = (x >> (mul2 - 2)) as usize;
        debug_assert!(base < 16);
        SQRT_ARRAY_FP16[base] >> (17 - log2_half)
    } else {
        SQRT_ARRAY_FP16[x as usize] >> 16
    }
}

/// `calculate_tf_shift_factor` (temporal_filtering.c:610). `static` in C —
/// gated at TIER 1 through a facade shim.
///
/// Maps the 64x64 block error to the weight shift used in the decay-factor
/// derivation. Note the input is `ctx->tf_64x64_block_error >> 12`.
pub fn calculate_tf_shift_factor(tf_64x64_block_error: u64) -> u8 {
    let block_err = tf_64x64_block_error >> 12;
    if block_err < LOW_ERROR_THRESHOLD {
        14
    } else if block_err < MED_ERROR_THRESHOLD {
        13
    } else {
        12
    }
}

/// `svt_av1_calculate_decay_factor` (temporal_filtering.c:589). `static
/// inline` in C — gated at TIER 1 through a facade shim.
///
/// Derives the Q16 decay factors from the noise level, q and the per-plane
/// decay controls. Called 4x from the RANDOM_ACCESS driver; it sets the
/// strength of the whole filter.
///
/// The Y factor is computed from the `n_decay_fp10` the CALLER already holds —
/// C reads `*n_decay_fp10` before writing it — and the U/V arms then overwrite
/// it in turn. `n_decay_fp10` is therefore in/out, and the value left in it
/// after the call is V's (or Y's untouched value when `tf_chroma` is 0).
#[allow(clippy::too_many_arguments)]
pub fn calculate_decay_factor(
    tf_decay_factor_fp16: &mut [u32; 3],
    n_decay_fp10: &mut i32,
    q_decay_fp8: u32,
    decay_control_cu: i32,
    decay_control_cv: i32,
    const_0dot7_fp16: i32,
    noise_levels_log1p_fp16: &[i32; 3],
    shift_factor: u8,
    tf_chroma: bool,
) {
    let sq = |n: i32| -> u32 {
        ((i64::from(n) * i64::from(n) * i64::from(q_decay_fp8)) >> shift_factor) as u32
    };
    tf_decay_factor_fp16[0] = sq(*n_decay_fp10);
    if tf_chroma {
        *n_decay_fp10 =
            (decay_control_cu * (const_0dot7_fp16 + noise_levels_log1p_fp16[1])) / (1 << 6);
        tf_decay_factor_fp16[1] = sq(*n_decay_fp10);
        *n_decay_fp10 =
            (decay_control_cv * (const_0dot7_fp16 + noise_levels_log1p_fp16[2])) / (1 << 6);
        tf_decay_factor_fp16[2] = sq(*n_decay_fp10);
    }
}

/// `calculate_squared_errors_sum` (temporal_filtering.c:697). `static` in C —
/// gated at TIER 1 through a facade shim.
///
/// The 8-bit block error accumulator the medium kernel weights on. C computes
/// `SQR(s[..] - p[..])` on `uint8_t` operands, which integer-promote to `int`,
/// so the difference is SIGNED before squaring — the square is then
/// non-negative and the `uint32_t` accumulation cannot wrap for the block
/// sizes used. An off-by-one in the stride walk shifts every TF weight.
pub fn calculate_squared_errors_sum(
    s: &[u8],
    s_stride: usize,
    p: &[u8],
    p_stride: usize,
    w: usize,
    h: usize,
) -> u32 {
    let mut sum = 0u32;
    for i in 0..h {
        for j in 0..w {
            let d = i32::from(s[i * s_stride + j]) - i32::from(p[i * p_stride + j]);
            sum = sum.wrapping_add((d * d) as u32);
        }
    }
    sum
}

/// `calculate_squared_errors_sum_highbd` (temporal_filtering.c:710). `static`
/// in C — gated at TIER 1 through a facade shim.
///
/// Same accumulation on 16-bit samples, then right-shifted by
/// `(bit_depth - 8) * 2` so the result is comparable with the 8-bit one.
pub fn calculate_squared_errors_sum_highbd(
    s: &[u16],
    s_stride: usize,
    p: &[u16],
    p_stride: usize,
    w: usize,
    h: usize,
    shift_factor: u32,
) -> u32 {
    let mut sum = 0u32;
    for i in 0..h {
        for j in 0..w {
            let d = i32::from(s[i * s_stride + j]) - i32::from(p[i * p_stride + j]);
            sum = sum.wrapping_add((d * d) as u32);
        }
    }
    sum >> shift_factor
}

/// `OD_DIVU` (bitstream_unit.h:54).
///
/// C routes `_d < 1024` through `OD_DIVU_SMALL`, a reciprocal-multiply against
/// the 1024-entry `svt_aom_od_divu_small_consts` table, and everything else
/// through plain integer division. The table is NOT transcribed here: the
/// reciprocal form is exact over the domain the temporal filter uses, and that
/// claim is GATED, not assumed — `od_divu_matches_c` in
/// `tests/c_parity_temporal_filtering.rs` drives the real C macro through a
/// shim across a wide (numerator, denominator) grid including both sides of
/// the 1024 boundary. If a divergence ever appears the fix is to transcribe
/// the table, not to widen the test.
pub fn od_divu(x: u32, d: u32) -> u32 {
    x / d
}

/// `svt_aom_apply_filtering_central_c` (temporal_filtering.c:262).
/// EXPORTED — TIER 1.
///
/// Seeds `accum`/`count` with the central (unfiltered) picture before any
/// reference is blended. If this is missing or mis-weighted every filtered
/// pixel is wrong even when the reference blend is right.
///
/// `accum`/`count` are indexed by a running `k` (dense, block-sized), while
/// `src` is indexed by the picture stride — the two index spaces differ and
/// mixing them silently mixes rows.
///
/// The chroma stride is `src_stride_y >> ss_x` — C uses ss_x for the STRIDE
/// even while `blk_height_ch` uses ss_y.
#[allow(clippy::too_many_arguments)]
pub fn apply_filtering_central(
    tf_chroma: bool,
    src_y: &[u8],
    src_u: &[u8],
    src_v: &[u8],
    src_stride_y: usize,
    accum: &mut [Vec<u32>; 3],
    count: &mut [Vec<u16>; 3],
    blk_width: usize,
    blk_height: usize,
    ss_x: u32,
    ss_y: u32,
) {
    let blk_height_ch = blk_height >> ss_y;
    let blk_width_ch = blk_width >> ss_x;
    let src_stride_ch = src_stride_y >> ss_x;
    let modifier = TF_PLANEWISE_FILTER_WEIGHT_SCALE;

    let mut k = 0usize;
    for i in 0..blk_height {
        for j in 0..blk_width {
            accum[0][k] = modifier * u32::from(src_y[i * src_stride_y + j]);
            count[0][k] = modifier as u16;
            k += 1;
        }
    }

    if tf_chroma {
        let mut k = 0usize;
        for i in 0..blk_height_ch {
            for j in 0..blk_width_ch {
                accum[1][k] = modifier * u32::from(src_u[i * src_stride_ch + j]);
                count[1][k] = modifier as u16;
                accum[2][k] = modifier * u32::from(src_v[i * src_stride_ch + j]);
                count[2][k] = modifier as u16;
                k += 1;
            }
        }
    }
}

/// `svt_aom_apply_filtering_central_highbd_c` (temporal_filtering.c:300).
/// EXPORTED — TIER 1. The 10-bit counterpart of the above.
#[allow(clippy::too_many_arguments)]
pub fn apply_filtering_central_highbd(
    tf_chroma: bool,
    src_y: &[u16],
    src_u: &[u16],
    src_v: &[u16],
    src_stride_y: usize,
    accum: &mut [Vec<u32>; 3],
    count: &mut [Vec<u16>; 3],
    blk_width: usize,
    blk_height: usize,
    ss_x: u32,
    ss_y: u32,
) {
    let blk_height_ch = blk_height >> ss_y;
    let blk_width_ch = blk_width >> ss_x;
    let src_stride_ch = src_stride_y >> ss_x;
    let modifier = TF_PLANEWISE_FILTER_WEIGHT_SCALE;

    let mut k = 0usize;
    for i in 0..blk_height {
        for j in 0..blk_width {
            accum[0][k] = modifier * u32::from(src_y[i * src_stride_y + j]);
            count[0][k] = modifier as u16;
            k += 1;
        }
    }

    if tf_chroma {
        let mut k = 0usize;
        for i in 0..blk_height_ch {
            for j in 0..blk_width_ch {
                accum[1][k] = modifier * u32::from(src_u[i * src_stride_ch + j]);
                count[1][k] = modifier as u16;
                accum[2][k] = modifier * u32::from(src_v[i * src_stride_ch + j]);
                count[2][k] = modifier as u16;
                k += 1;
            }
        }
    }
}

/// `svt_aom_get_final_filtered_pixels_c` (temporal_filtering.c:2426).
/// EXPORTED — TIER 1.
///
/// The `accum`/`count` -> pixel normalisation that writes the filtered source
/// back over `enhanced_pic`. This is the function whose output the rest of the
/// encoder actually consumes.
///
/// The luma loop is fixed at `TF_BH x TF_BW` (64x64) regardless of the block
/// arguments; only the chroma loop uses `blk_*_ch`. The rounding is
/// `(accum + count/2) / count` through `OD_DIVU`.
///
/// Note the V plane's assert in C checks the U plane's quotient — an upstream
/// copy-paste in a debug assert only, with no effect on emitted pixels.
#[allow(clippy::too_many_arguments)]
pub fn get_final_filtered_pixels(
    tf_chroma: bool,
    src_center: &mut [Vec<u8>; 3],
    accum: &[Vec<u32>; 3],
    count: &[Vec<u16>; 3],
    stride: &[usize; 3],
    blk_y_src_offset: usize,
    blk_ch_src_offset: usize,
    blk_width_ch: usize,
    blk_height_ch: usize,
) {
    let mut pos = blk_y_src_offset;
    let mut k = 0usize;
    for _ in 0..TF_BH {
        for _ in 0..TF_BW {
            let c = u32::from(count[0][k]);
            src_center[0][pos] = od_divu(accum[0][k] + (c >> 1), c) as u8;
            pos += 1;
            k += 1;
        }
        pos += stride[0] - TF_BW;
    }
    if tf_chroma {
        let mut pos = blk_ch_src_offset;
        let mut k = 0usize;
        for _ in 0..blk_height_ch {
            for _ in 0..blk_width_ch {
                let cu = u32::from(count[1][k]);
                src_center[1][pos] = od_divu(accum[1][k] + (cu >> 1), cu) as u8;
                let cv = u32::from(count[2][k]);
                src_center[2][pos] = od_divu(accum[2][k] + (cv >> 1), cv) as u8;
                pos += 1;
                k += 1;
            }
            pos += stride[1] - blk_width_ch;
        }
    }
}

/// The 10-bit arm of `svt_aom_get_final_filtered_pixels_c`, which writes into
/// `altref_buffer_highbd_start` instead. EXPORTED — TIER 1.
#[allow(clippy::too_many_arguments)]
pub fn get_final_filtered_pixels_highbd(
    tf_chroma: bool,
    altref_highbd: &mut [Vec<u16>; 3],
    accum: &[Vec<u32>; 3],
    count: &[Vec<u16>; 3],
    stride: &[usize; 3],
    blk_y_src_offset: usize,
    blk_ch_src_offset: usize,
    blk_width_ch: usize,
    blk_height_ch: usize,
) {
    let mut pos = blk_y_src_offset;
    let mut k = 0usize;
    for _ in 0..TF_BH {
        for _ in 0..TF_BW {
            let c = u32::from(count[0][k]);
            altref_highbd[0][pos] = od_divu(accum[0][k] + (c >> 1), c) as u16;
            pos += 1;
            k += 1;
        }
        pos += stride[0] - TF_BW;
    }
    if tf_chroma {
        let mut pos = blk_ch_src_offset;
        let mut k = 0usize;
        for _ in 0..blk_height_ch {
            for _ in 0..blk_width_ch {
                let cu = u32::from(count[1][k]);
                altref_highbd[1][pos] = od_divu(accum[1][k] + (cu >> 1), cu) as u16;
                let cv = u32::from(count[2][k]);
                altref_highbd[2][pos] = od_divu(accum[2][k] + (cv >> 1), cv) as u16;
                pos += 1;
                k += 1;
            }
            pos += stride[1] - blk_width_ch;
        }
    }
}

/// `tf_use_64x64_pred` (temporal_filtering.c:2491). EXPORTED — TIER 1.
///
/// Half of the 64x64-vs-4x32x32 decision: compares the 64x64 SAD against the
/// sum of the four 32x32 SADs as a percentage deviation, against
/// `tf_use_pred_64x64_only_th`.
///
/// Both operands are floored at 1 by `MAX(.., 1)` BEFORE the subtraction, so a
/// zero-SAD block gives deviation 0, not a division by zero.
pub fn tf_use_64x64_pred(
    p_best_sad_64x64: u32,
    p_best_sad_32x32: &[u32; 4],
    tf_use_pred_64x64_only_th: i64,
) -> i8 {
    let dist_32x32: u32 = p_best_sad_32x32
        .iter()
        .fold(0u32, |a, &b| a.wrapping_add(b));
    let d32 = i64::from(dist_32x32).max(1);
    let d64 = i64::from(p_best_sad_64x64).max(1);
    let dev_64x64_to_32x32 = ((d64 - d32) * 100) / d32;
    i8::from(dev_64x64_to_32x32 < tf_use_pred_64x64_only_th)
}

// ---------------------------------------------------------------------------
// The live 8-bit / 10-bit "medium" filter kernels
// ---------------------------------------------------------------------------

/// The `MeContext` fields the medium temporal-filter kernels read.
///
/// Only these are consulted; the real struct is ~megabytes of ME scratch that
/// the kernel never touches. Keeping the surface explicit is what lets the
/// parity shim build a facade context and drive the REAL exported symbol.
#[derive(Debug, Clone)]
pub struct TfKernelCtx {
    /// `me_ctx->tf_block_col` / `tf_block_row`; `idx_32x32 = col + row * 2`.
    pub tf_block_col: i32,
    pub tf_block_row: i32,
    /// `me_ctx->tf_mv_dist_th`.
    pub tf_mv_dist_th: u32,
    pub tf_chroma: bool,
    pub tf_32x32_block_split_flag: [u8; 4],
    pub tf_16x16_mv_x: [i16; 16],
    pub tf_16x16_mv_y: [i16; 16],
    pub tf_16x16_block_error: [u64; 16],
    pub tf_32x32_mv_x: [i16; 4],
    pub tf_32x32_mv_y: [i16; 4],
    pub tf_32x32_block_error: [u64; 4],
    /// `me_ctx->tf_decay_factor_fp16[PLANE_Y/U/V]`.
    pub tf_decay_factor_fp16: [u32; 3],
}

impl Default for TfKernelCtx {
    fn default() -> Self {
        Self {
            tf_block_col: 0,
            tf_block_row: 0,
            tf_mv_dist_th: 10,
            tf_chroma: true,
            tf_32x32_block_split_flag: [0; 4],
            tf_16x16_mv_x: [0; 16],
            tf_16x16_mv_y: [0; 16],
            tf_16x16_block_error: [0; 16],
            tf_32x32_mv_x: [0; 4],
            tf_32x32_mv_y: [0; 4],
            tf_32x32_block_error: [0; 4],
            tf_decay_factor_fp16: [1 << 16; 3],
        }
    }
}

/// The per-quadrant distance factors and block errors both `partial` kernels
/// derive identically apart from the block-error shifts.
///
/// 8-bit uses `tf_16x16_block_error[i]` unshifted and `tf_32x32_block_error >> 2`;
/// 10-bit uses `>> 4` and `>> 6`. The `tf_decay_factor_fp16 <<= 1` on the
/// no-split arm applies to both.
fn tf_quadrant_terms(
    ctx: &TfKernelCtx,
    idx_32x32: usize,
    tf_decay_factor_fp16: &mut u32,
    hbd: bool,
) -> ([u32; 4], [u32; 4]) {
    let distance_threshold_fp16 = ((ctx.tf_mv_dist_th << 16) / 10).max(1 << 16);
    let mut d_factor_fp8 = [0u32; 4];
    let mut block_error_fp8 = [0u32; 4];

    if ctx.tf_32x32_block_split_flag[idx_32x32] != 0 {
        for i in 0..4 {
            let col = i32::from(ctx.tf_16x16_mv_x[idx_32x32 * 4 + i]);
            let row = i32::from(ctx.tf_16x16_mv_y[idx_32x32 * 4 + i]);
            let distance_fp4 = sqrt_fast(((col * col + row * row) as u32) << 8);
            d_factor_fp8[i] = ((distance_fp4 << 12) / (distance_threshold_fp16 >> 8)).max(1 << 8);
            let e = ctx.tf_16x16_block_error[idx_32x32 * 4 + i];
            block_error_fp8[i] = if hbd { (e >> 4) as u32 } else { e as u32 };
        }
    } else {
        *tf_decay_factor_fp16 <<= 1;
        let col = i32::from(ctx.tf_32x32_mv_x[idx_32x32]);
        let row = i32::from(ctx.tf_32x32_mv_y[idx_32x32]);
        let distance_fp4 = sqrt_fast(((col * col + row * row) as u32) << 8);
        let d = ((distance_fp4 << 12) / (distance_threshold_fp16 >> 8)).max(1 << 8);
        d_factor_fp8 = [d; 4];
        let e = ctx.tf_32x32_block_error[idx_32x32];
        let b = if hbd {
            (e >> 6) as u32
        } else {
            (e >> 2) as u32
        };
        block_error_fp8 = [b; 4];
    }
    (d_factor_fp8, block_error_fp8)
}

/// The accumulate step shared by both `partial` kernels: from the four
/// per-quadrant window errors and block errors, derive one exp-decay weight
/// per quadrant and add `weight * pred` into `accum` / `weight` into `count`.
#[allow(clippy::too_many_arguments)]
fn tf_accumulate<P: Copy + Into<u32>>(
    window_error_quad_fp8: &[u32; 4],
    block_error_fp8: &[u32; 4],
    d_factor_fp8: &[u32; 4],
    tf_decay_factor_fp16: u32,
    pre: &[P],
    pre_stride: usize,
    block_width: usize,
    block_height: usize,
    accum: &mut [u32],
    count: &mut [u16],
) {
    for subblock_idx in 0..4usize {
        let combined_error_fp8 = (window_error_quad_fp8[subblock_idx]
            * TF_WINDOW_BLOCK_BALANCE_WEIGHT
            + block_error_fp8[subblock_idx])
            / (TF_WINDOW_BLOCK_BALANCE_WEIGHT + 1);

        let avg_err_fp10 =
            u64::from(combined_error_fp8 >> 3) * u64::from(d_factor_fp8[subblock_idx] >> 3);
        let scaled_diff16 =
            (avg_err_fp10 / u64::from((tf_decay_factor_fp16 >> 10).max(1))).min(7 * 16) as usize;
        let adjusted_weight = ((EXPF_TAB_FP16[scaled_diff16] as u32) * TF_WEIGHT_SCALE) >> 16;

        let x_offset = (subblock_idx % 2) * block_width / 2;
        let y_offset = (subblock_idx / 2) * block_height / 2;

        for i in 0..block_height / 2 {
            for j in 0..block_width / 2 {
                let k = (i + y_offset) * pre_stride + j + x_offset;
                let pixel_value: u32 = pre[k].into();
                count[k] = count[k].wrapping_add(adjusted_weight as u16);
                accum[k] = accum[k].wrapping_add(adjusted_weight * pixel_value);
            }
        }
    }
}

/// `svt_av1_apply_temporal_filter_planewise_medium_partial_c`
/// (temporal_filtering.c:930). `static` in C — reached at TIER 1 through the
/// exported wrapper below.
///
/// Where the actual per-pixel weight math lives; the exported wrapper is a
/// thin loop over it, so this is the function whose bits must match.
///
/// `luma_window_error_quad_fp8` is IN/OUT: the luma call WRITES it (because
/// `window_error_quad_fp8` aliases it when `is_chroma == 0`) and the two
/// chroma calls then READ it to blend `(chroma * 5 + luma) / 6`. Porting it as
/// a local would silently drop the luma coupling.
#[allow(clippy::too_many_arguments)]
pub fn apply_temporal_filter_planewise_medium_partial(
    ctx: &TfKernelCtx,
    src: &[u8],
    src_stride: usize,
    pre: &[u8],
    pre_stride: usize,
    block_width: usize,
    block_height: usize,
    accum: &mut [u32],
    count: &mut [u16],
    mut tf_decay_factor_fp16: u32,
    luma_window_error_quad_fp8: &mut [u32; 4],
    is_chroma: bool,
) {
    let idx_32x32 = (ctx.tf_block_col + ctx.tf_block_row * 2) as usize;
    let (d_factor_fp8, block_error_fp8) =
        tf_quadrant_terms(ctx, idx_32x32, &mut tf_decay_factor_fp16, false);

    let bw_half = block_width >> 1;
    let bh_half = block_height >> 1;
    let mut quad = [0u32; 4];
    for (i, q) in quad.iter_mut().enumerate() {
        let dx = (i % 2) * bw_half;
        let dy = (i / 2) * bh_half;
        let sum = calculate_squared_errors_sum(
            &src[dy * src_stride + dx..],
            src_stride,
            &pre[dy * pre_stride + dx..],
            pre_stride,
            bw_half,
            bh_half,
        );
        *q = (((sum << 4) / bw_half as u32) << 4) / bh_half as u32;
    }

    if is_chroma {
        for i in 0..4 {
            quad[i] = (quad[i] * 5 + luma_window_error_quad_fp8[i]) / 6;
        }
    } else {
        *luma_window_error_quad_fp8 = quad;
    }

    tf_accumulate(
        &quad,
        &block_error_fp8,
        &d_factor_fp8,
        tf_decay_factor_fp16,
        pre,
        pre_stride,
        block_width,
        block_height,
        accum,
        count,
    );
}

/// `svt_av1_apply_temporal_filter_planewise_medium_c`
/// (temporal_filtering.c:1039). EXPORTED and RTCD-dispatched — TIER 1.
///
/// The live 8-bit filter kernel for RANDOM_ACCESS TF (`use_zz_based_filter` is
/// 0 there). A thin loop over the partial kernel: luma once, then U and V when
/// `tf_chroma` is set, all three sharing the luma window-error quad.
#[allow(clippy::too_many_arguments)]
pub fn apply_temporal_filter_planewise_medium(
    ctx: &TfKernelCtx,
    y_src: &[u8],
    y_src_stride: usize,
    y_pre: &[u8],
    y_pre_stride: usize,
    u_src: &[u8],
    v_src: &[u8],
    uv_src_stride: usize,
    u_pre: &[u8],
    v_pre: &[u8],
    uv_pre_stride: usize,
    block_width: usize,
    block_height: usize,
    ss_x: u32,
    ss_y: u32,
    y_accum: &mut [u32],
    y_count: &mut [u16],
    u_accum: &mut [u32],
    u_count: &mut [u16],
    v_accum: &mut [u32],
    v_count: &mut [u16],
) {
    let mut luma_window_error_quad_fp8 = [0u32; 4];
    apply_temporal_filter_planewise_medium_partial(
        ctx,
        y_src,
        y_src_stride,
        y_pre,
        y_pre_stride,
        block_width,
        block_height,
        y_accum,
        y_count,
        ctx.tf_decay_factor_fp16[0],
        &mut luma_window_error_quad_fp8,
        false,
    );
    if ctx.tf_chroma {
        for (src, pre, accum, cnt, dec) in [
            (
                u_src,
                u_pre,
                &mut *u_accum,
                &mut *u_count,
                ctx.tf_decay_factor_fp16[1],
            ),
            (
                v_src,
                v_pre,
                &mut *v_accum,
                &mut *v_count,
                ctx.tf_decay_factor_fp16[2],
            ),
        ] {
            apply_temporal_filter_planewise_medium_partial(
                ctx,
                src,
                uv_src_stride,
                pre,
                uv_pre_stride,
                block_width >> ss_x,
                block_height >> ss_y,
                accum,
                cnt,
                dec,
                &mut luma_window_error_quad_fp8,
                true,
            );
        }
    }
}

/// `svt_av1_apply_temporal_filter_planewise_medium_hbd_partial_c`
/// (temporal_filtering.c:1105). `static` in C — reached at TIER 1 through the
/// exported wrapper below.
///
/// Differs from the 8-bit partial in exactly three places, all transcribed
/// from the C site rather than assumed: the block errors are shifted
/// (`>> 4` for 16x16, `>> 6` for 32x32 instead of unshifted / `>> 2`), the
/// squared-error sums are shifted by `(bit_depth - 8) * 2`, and the pixels are
/// 16-bit.
#[allow(clippy::too_many_arguments)]
pub fn apply_temporal_filter_planewise_medium_hbd_partial(
    ctx: &TfKernelCtx,
    src: &[u16],
    src_stride: usize,
    pre: &[u16],
    pre_stride: usize,
    block_width: usize,
    block_height: usize,
    accum: &mut [u32],
    count: &mut [u16],
    mut tf_decay_factor_fp16: u32,
    luma_window_error_quad_fp8: &mut [u32; 4],
    is_chroma: bool,
    encoder_bit_depth: u32,
) {
    let idx_32x32 = (ctx.tf_block_col + ctx.tf_block_row * 2) as usize;
    let shift_factor = (encoder_bit_depth - 8) * 2;
    let (d_factor_fp8, block_error_fp8) =
        tf_quadrant_terms(ctx, idx_32x32, &mut tf_decay_factor_fp16, true);

    let bw_half = block_width >> 1;
    let bh_half = block_height >> 1;
    let mut quad = [0u32; 4];
    for (i, q) in quad.iter_mut().enumerate() {
        let dx = (i % 2) * bw_half;
        let dy = (i / 2) * bh_half;
        let sum = calculate_squared_errors_sum_highbd(
            &src[dy * src_stride + dx..],
            src_stride,
            &pre[dy * pre_stride + dx..],
            pre_stride,
            bw_half,
            bh_half,
            shift_factor,
        );
        *q = (((sum << 4) / bw_half as u32) << 4) / bh_half as u32;
    }

    if is_chroma {
        for i in 0..4 {
            quad[i] = (quad[i] * 5 + luma_window_error_quad_fp8[i]) / 6;
        }
    } else {
        *luma_window_error_quad_fp8 = quad;
    }

    tf_accumulate(
        &quad,
        &block_error_fp8,
        &d_factor_fp8,
        tf_decay_factor_fp16,
        pre,
        pre_stride,
        block_width,
        block_height,
        accum,
        count,
    );
}

/// `svt_av1_apply_temporal_filter_planewise_medium_hbd_c`
/// (temporal_filtering.c:1216). EXPORTED — TIER 1.
///
/// The 10-bit filter kernel, needed for bd10 video — the AVIF/HDR product case
/// this port already ships for stills.
#[allow(clippy::too_many_arguments)]
pub fn apply_temporal_filter_planewise_medium_hbd(
    ctx: &TfKernelCtx,
    y_src: &[u16],
    y_src_stride: usize,
    y_pre: &[u16],
    y_pre_stride: usize,
    u_src: &[u16],
    v_src: &[u16],
    uv_src_stride: usize,
    u_pre: &[u16],
    v_pre: &[u16],
    uv_pre_stride: usize,
    block_width: usize,
    block_height: usize,
    ss_x: u32,
    ss_y: u32,
    y_accum: &mut [u32],
    y_count: &mut [u16],
    u_accum: &mut [u32],
    u_count: &mut [u16],
    v_accum: &mut [u32],
    v_count: &mut [u16],
    encoder_bit_depth: u32,
) {
    let mut luma_window_error_quad_fp8 = [0u32; 4];
    apply_temporal_filter_planewise_medium_hbd_partial(
        ctx,
        y_src,
        y_src_stride,
        y_pre,
        y_pre_stride,
        block_width,
        block_height,
        y_accum,
        y_count,
        ctx.tf_decay_factor_fp16[0],
        &mut luma_window_error_quad_fp8,
        false,
        encoder_bit_depth,
    );
    if ctx.tf_chroma {
        for (src, pre, accum, cnt, dec) in [
            (
                u_src,
                u_pre,
                &mut *u_accum,
                &mut *u_count,
                ctx.tf_decay_factor_fp16[1],
            ),
            (
                v_src,
                v_pre,
                &mut *v_accum,
                &mut *v_count,
                ctx.tf_decay_factor_fp16[2],
            ),
        ] {
            apply_temporal_filter_planewise_medium_hbd_partial(
                ctx,
                src,
                uv_src_stride,
                pre,
                uv_pre_stride,
                block_width >> ss_x,
                block_height >> ss_y,
                accum,
                cnt,
                dec,
                &mut luma_window_error_quad_fp8,
                true,
                encoder_bit_depth,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Noise estimation, block-split derivation, and the post-TF re-pad/re-decimate
// ---------------------------------------------------------------------------

/// `EDGE_THRESHOLD` / `SMOOTH_THRESHOLD` / `SQRT_PI_BY_2_FP16`
/// (temporal_filtering.h).
pub const EDGE_THRESHOLD: i32 = 50;
pub const SMOOTH_THRESHOLD: i64 = 16;
pub const SQRT_PI_BY_2_FP16: i64 = 82137;

/// `svt_estimate_noise_highbd_fp16_c` (temporal_filtering.c:3603).
/// EXPORTED — TIER 1.
///
/// The bd10 counterpart of `svt_estimate_noise_fp16_c` (already in
/// `temporal_filter.rs`). Feeds `noise_levels_log1p_fp16`, which drives
/// `tf_chroma` and the reference-count modulation, so a 10-bit RANDOM_ACCESS
/// GOP picks a different TF window without it.
///
/// Differs from the 8-bit version in exactly two places, both
/// `ROUND_POWER_OF_TWO(x, bd - 8)`: the gradient magnitude is rounded down to
/// 8-bit scale BEFORE the `EDGE_THRESHOLD` test, and each Laplacian magnitude
/// is rounded before it is accumulated (not the sum afterwards — rounding the
/// total instead changes the result).
pub fn estimate_noise_highbd_fp16(
    src: &[u16],
    width: usize,
    height: usize,
    stride: usize,
    bd: u32,
) -> i32 {
    if width < 3 || height < 3 {
        return -65536;
    }
    let shift = bd - 8;
    // ROUND_POWER_OF_TWO(x, n) == (x + (1 << (n - 1))) >> n, and is the
    // identity for n == 0.
    let rpot = |x: i32| -> i32 {
        if shift == 0 {
            x
        } else {
            (x + (1 << (shift - 1))) >> shift
        }
    };

    let mut sum: i64 = 0;
    let mut num: i64 = 0;
    for i in 1..height - 1 {
        for j in 1..width - 1 {
            let k = i * stride + j;
            let at = |o: isize| -> i32 { i32::from(src[(k as isize + o) as usize]) };
            let s = stride as isize;
            let g_x = (at(-s - 1) - at(-s + 1)) + (at(s - 1) - at(s + 1)) + 2 * (at(-1) - at(1));
            let g_y = (at(-s - 1) - at(s - 1)) + (at(-s + 1) - at(s + 1)) + 2 * (at(-s) - at(s));
            let ga = rpot(g_x.abs() + g_y.abs());
            if ga < EDGE_THRESHOLD {
                let v = 4 * at(0) - 2 * (at(-1) + at(1) + at(-s) + at(s))
                    + (at(-s - 1) + at(-s + 1) + at(s - 1) + at(s + 1));
                sum += i64::from(rpot(v.abs()));
                num += 1;
            }
        }
    }
    if num < SMOOTH_THRESHOLD {
        return -65536;
    }
    ((sum * SQRT_PI_BY_2_FP16) / (6 * num)) as i32
}

/// The `MeContext` fields `derive_tf_32x32_block_split_flag` reads and writes.
#[derive(Debug, Clone)]
pub struct TfSplitCtx {
    pub idx_32x32: usize,
    pub enable_8x8_pred: bool,
    pub tf_32x32_block_error: [u64; 4],
    pub tf_16x16_block_error: [u64; 16],
    pub tf_8x8_block_error: [u64; 64],
    pub tf_32x32_block_split_flag: [i32; 4],
    /// `[idx_32x32][i]`.
    pub tf_16x16_block_split_flag: [[i32; 4]; 4],
}

impl Default for TfSplitCtx {
    fn default() -> Self {
        Self {
            idx_32x32: 0,
            enable_8x8_pred: false,
            tf_32x32_block_error: [0; 4],
            tf_16x16_block_error: [0; 16],
            tf_8x8_block_error: [0; 64],
            tf_32x32_block_split_flag: [0; 4],
            tf_16x16_block_split_flag: [[0; 4]; 4],
        }
    }
}

/// `derive_tf_32x32_block_split_flag` (temporal_filtering.c:152). `static` in
/// C — gated at TIER 1 through a facade shim.
///
/// Decides 64x64-vs-4x32x32 per block, and (when `enable_8x8_pred` is on)
/// 16x16-vs-4x8x8 per sub-block. Three faithfulness details:
/// * the early-out compares the 32x32 error against `INT_MAX` after a cast to
///   `int`, which is how the "not yet motion-searched" sentinel is spelled;
/// * the 8x8 branch OVERWRITES `tf_16x16_block_error` with the summed 8x8
///   error when it splits, so the value the filter later weights on changes;
/// * the split test is `block_error * 14 < sum_subblock_error * 16` (and
///   `subblock_errors[i] * 8 < error_8x8 * 16` for the 16x16 level) — the
///   thresholds are not the same ratio at the two levels.
///
/// `enable_8x8_pred` is 0 at `tf_level 5` (presets 3..7), so the 8x8 branch is
/// live only at tf_level 1/2 (presets 0..2).
pub fn derive_tf_32x32_block_split_flag(ctx: &mut TfSplitCtx) {
    let idx_32x32 = ctx.idx_32x32;
    let block_error = ctx.tf_32x32_block_error[idx_32x32] as u32 as i32;

    // `block_error` is initialised as INT_MAX and overwritten after motion
    // search with a reference frame, so INT_MAX can only be reached by the
    // to-filter frame itself.
    if block_error == i32::MAX {
        ctx.tf_32x32_block_split_flag[idx_32x32] = 0;
        ctx.tf_16x16_block_split_flag[idx_32x32] = [0; 4];
        return;
    }

    let mut subblock_errors = [0i32; 4];
    let mut sum_subblock_error = 0i32;
    for i in 0..4 {
        subblock_errors[i] = ctx.tf_16x16_block_error[idx_32x32 * 4 + i] as u32 as i32;

        if ctx.enable_8x8_pred {
            let mut error_8x8 = 0i32;
            for idx_8x8 in 0..4 {
                error_8x8 = error_8x8.wrapping_add(
                    ctx.tf_8x8_block_error[idx_32x32 * 16 + 4 * i + idx_8x8] as u32 as i32,
                );
            }
            if subblock_errors[i].wrapping_mul(8) < error_8x8.wrapping_mul(16) {
                ctx.tf_16x16_block_split_flag[idx_32x32][i] = 0;
            } else {
                ctx.tf_16x16_block_split_flag[idx_32x32][i] = 1;
                ctx.tf_16x16_block_error[idx_32x32 * 4 + i] = error_8x8 as u64;
                subblock_errors[i] = error_8x8;
            }
        } else {
            ctx.tf_16x16_block_split_flag[idx_32x32][i] = 0;
        }

        sum_subblock_error = sum_subblock_error.wrapping_add(subblock_errors[i]);
    }
    ctx.tf_32x32_block_split_flag[idx_32x32] =
        i32::from(block_error.wrapping_mul(14) >= sum_subblock_error.wrapping_mul(16));
}

/// The MV/split half of `convert_64x64_info_to_32x32_info`
/// (temporal_filtering.c:2509). `static` in C — gated at TIER 1 through a
/// facade shim.
///
/// Propagates the 64x64 MV into all four 32x32 slots and clears every split
/// flag, so an unsplit 64x64 block hands the filter the right MV. A mis-mapped
/// index gives the wrong MV to the filter.
///
/// The SECOND half of the C function — re-measuring each 32x32's distortion
/// through `svt_aom_mefn_ptr[BLOCK_32X32].vf` (or `BLOCK_32X16` when
/// `tf_ctrls.sub_sampling_shift` is set) — is NOT ported here: it is a call
/// into the variance kernels the port already owns in `svtav1-dsp`, and wiring
/// it needs the pred/src block pointers that only the TF driver holds. It is
/// listed as outstanding rather than silently folded in.
pub fn convert_64x64_info_to_32x32_info_mvs(
    tf_64x64_mv_x: i16,
    tf_64x64_mv_y: i16,
    tf_32x32_mv_x: &mut [i16; 4],
    tf_32x32_mv_y: &mut [i16; 4],
    tf_32x32_block_split_flag: &mut [i32; 4],
    tf_16x16_block_split_flag: &mut [[i32; 4]; 4],
) {
    *tf_32x32_mv_x = [tf_64x64_mv_x; 4];
    *tf_32x32_mv_y = [tf_64x64_mv_y; 4];
    *tf_32x32_block_split_flag = [0; 4];
    // C memsets `sizeof(flag[0][0]) * 4 * 4` bytes, i.e. the whole 4x4 array.
    *tf_16x16_block_split_flag = [[0; 4]; 4];
}

/// `pad_and_decimate_filtered_pic` (temporal_filtering.c:3749).
/// EXPORTED — TIER 1.
///
/// Re-pads and re-decimates the picture AFTER temporal filtering overwrote it,
/// so the 1/4 and 1/16 planes the later ME reads are derived from the FILTERED
/// source. Skipping it leaves ME searching pre-TF downsamples — a divergence
/// that presents as an ME bug.
///
/// The min-block re-pad is CONDITIONAL: it runs only when
/// `(width - pad_right) % 8 != 0` or `(height - pad_bottom) % 8 != 0`. The
/// chroma border pad is gated on `tf_ctrls.chroma_lvl`, which is a DIFFERENT
/// flag from the `tf_chroma` the kernels read.
#[allow(clippy::too_many_arguments)]
pub fn pad_and_decimate_filtered_pic<'p>(
    subsampling_x: usize,
    subsampling_y: usize,
    pad_right: usize,
    pad_bottom: usize,
    color_format: u32,
    chroma_lvl: bool,
    hme: &crate::port_preanalysis::HmeEnables,
    input_pic: &mut crate::port_preanalysis::Plane<'_>,
    u: Option<&mut crate::port_preanalysis::Plane<'p>>,
    v: Option<&mut crate::port_preanalysis::Plane<'p>>,
    quarter: &mut crate::port_preanalysis::Plane<'_>,
    sixteenth: &mut crate::port_preanalysis::Plane<'_>,
) {
    use crate::port_preanalysis as pre;

    let mut u = u;
    let mut v = v;

    // Refine the non-8 padding.
    if (input_pic.width - pad_right) % 8 != 0 || (input_pic.height - pad_bottom) % 8 != 0 {
        pre::pad_picture_to_multiple_of_min_blk_size_dimensions(
            color_format,
            pad_right,
            pad_bottom,
            input_pic,
            u.as_deref_mut(),
            v.as_deref_mut(),
        );
    }

    pre::generate_padding(
        input_pic.buf,
        input_pic.origin,
        input_pic.stride,
        input_pic.width,
        input_pic.height,
        input_pic.border,
        input_pic.border,
    );

    if chroma_lvl {
        let cw = input_pic.width >> subsampling_x;
        let ch = input_pic.height >> subsampling_y;
        let cbx = input_pic.border >> subsampling_x;
        let cby = input_pic.border >> subsampling_y;
        for plane in [u, v].into_iter().flatten() {
            pre::generate_padding(plane.buf, plane.origin, plane.stride, cw, ch, cbx, cby);
        }
    }

    pre::downsample_filtering_input_picture(hme, input_pic, quarter, sixteenth);
}

// ---------------------------------------------------------------------------
// The sub-pel search chain and the per-block plumbing
// ---------------------------------------------------------------------------

/// `subblock_xy_16x16` (temporal_filtering.c:46) — `[pu_index] -> (idx_y, idx_x)`.
/// Note the ORDER: row first, column second. Reading it as (x, y) transposes
/// every 16x16 block's origin.
#[rustfmt::skip]
pub const SUBBLOCK_XY_16X16: [[u32; 2]; 16] = [
    [0, 0], [0, 1], [0, 2], [0, 3],
    [1, 0], [1, 1], [1, 2], [1, 3],
    [2, 0], [2, 1], [2, 2], [2, 3],
    [3, 0], [3, 1], [3, 2], [3, 3],
];

/// `idx_32x32_to_idx_16x16` (temporal_filtering.c:62).
#[rustfmt::skip]
pub const IDX_32X32_TO_IDX_16X16: [[u32; 4]; 4] = [
    [0, 1, 4, 5], [2, 3, 6, 7], [8, 9, 12, 13], [10, 11, 14, 15],
];

/// `subblock_xy_8x8` (temporal_filtering.c:65) — `[pu_index] -> (idx_y, idx_x)`.
pub const SUBBLOCK_XY_8X8: [[u32; 2]; 64] = {
    let mut t = [[0u32; 2]; 64];
    let mut i = 0;
    while i < 64 {
        t[i] = [(i / 8) as u32, (i % 8) as u32];
        i += 1;
    }
    t
};

/// `idx_32x32_to_idx_8x8` (temporal_filtering.c:75).
#[rustfmt::skip]
pub const IDX_32X32_TO_IDX_8X8: [[[u32; 4]; 4]; 4] = [
    [[0, 1, 8, 9],     [2, 3, 10, 11],   [16, 17, 24, 25], [18, 19, 26, 27]],
    [[4, 5, 12, 13],   [6, 7, 14, 15],   [20, 21, 28, 29], [22, 23, 30, 31]],
    [[32, 33, 40, 41], [34, 35, 42, 43], [48, 49, 56, 57], [50, 51, 58, 59]],
    [[36, 37, 44, 45], [38, 39, 46, 47], [52, 53, 60, 61], [54, 55, 62, 63]],
];

/// `TF_SUBPEL_SEARCH_PARAMS` (temporal_filtering.h), the fields the search
/// control flow reads. The MC-facing fields (`interp_filters`,
/// `pu_origin_*`, `local_origin_*`, `encoder_bit_depth`) are carried through
/// verbatim so the injected compensator can use them.
#[derive(Debug, Clone, Copy, Default)]
pub struct TfSubpelSearchParams {
    pub subsampling_shift: u8,
    pub interp_filters: u32,
    pub pu_origin_x: u16,
    pub pu_origin_y: u16,
    pub local_origin_x: u16,
    pub local_origin_y: u16,
    pub bsize: u32,
    pub is_highbd: bool,
    pub encoder_bit_depth: i32,
    pub idx_x: u32,
    pub idx_y: u32,
    pub mv_x: i16,
    pub mv_y: i16,
    pub xd: i16,
    pub yd: i16,
    pub subpel_pel_mode: u8,
}

/// The `tf_ctrls` fields the sub-pel search chain reads.
///
/// At `tf_level 5` (presets 3..7, the campaign presets) the live values are
/// `half_pel_mode = 2`, `quarter_pel_mode = 1`, `eight_pel_mode = 0` and
/// `enable_8x8_pred = 0` — the MODE VALUES change the candidate set, not just
/// whether a level runs, because `svt_check_position` skips every DIAGONAL
/// candidate when the mode is >= 2.
#[derive(Debug, Clone, Copy, Default)]
pub struct TfSearchCtrls {
    pub half_pel_mode: u8,
    pub quarter_pel_mode: u8,
    pub eight_pel_mode: u8,
    pub use_2tap: bool,
    pub sub_sampling_shift: u8,
    pub enable_8x8_pred: bool,
}

/// `svt_check_position` (temporal_filtering.c:1431) — the per-candidate
/// compensate-and-score step, minus the compensation itself.
///
/// The compensation (`svt_aom_simple_luma_unipred`) and the distortion
/// (`svt_aom_mefn_ptr[block_size].vf`) are INJECTED as `score`, which the
/// caller implements over the port's own MC and variance kernels. Everything
/// this function decides — the three early-outs, the block-size mapping, and
/// the strict-improvement update rule — is transcribed here.
///
/// Three details that change which MV wins:
/// * the diagonal skip: when `subpel_pel_mode >= 2`, a candidate with BOTH
///   `xd != 0` and `yd != 0` is not even compensated;
/// * the early exit threshold is `(bsize * bsize * tf_subpel_early_exit_th)
///   << is_highbd`, compared against the CURRENT best, and it returns before
///   the candidate is scored;
/// * the update is `distortion < *best_dist`, STRICTLY less — a tie does NOT
///   replace the incumbent, so the earlier candidate in the scan order wins.
///
/// `score` receives `(mv_x, mv_y, block_size_index_is_subsampled)` where the
/// third value is C's `subsampling_shift` for the centre candidate and 0 for
/// every offset candidate — C passes
/// `xd == 0 && yd == 0 ? subsampling_shift : 0` to the compensator.
pub fn check_position<F>(
    p: &TfSubpelSearchParams,
    tf_subpel_early_exit_th: u32,
    best_dist: &mut u64,
    best_mv_x: &mut i16,
    best_mv_y: &mut i16,
    score: &mut F,
) where
    F: FnMut(i16, i16, u8) -> u64,
{
    if p.subpel_pel_mode >= 2 && p.xd != 0 && p.yd != 0 {
        return;
    }
    // If the best distortion is already 0 the new point cannot beat it.
    if *best_dist == 0 {
        return;
    }
    if tf_subpel_early_exit_th != 0 {
        let th = u64::from(p.bsize * p.bsize * tf_subpel_early_exit_th) << u32::from(p.is_highbd);
        if *best_dist < th {
            return;
        }
    }

    let mv_x = p.mv_x + p.xd;
    let mv_y = p.mv_y + p.yd;
    let compensate_shift = if p.xd == 0 && p.yd == 0 {
        p.subsampling_shift
    } else {
        0
    };
    let distortion = score(mv_x, mv_y, compensate_shift);

    if distortion < *best_dist {
        *best_dist = distortion;
        *best_mv_x = mv_x;
        *best_mv_y = mv_y;
    }
}

/// `tf_subpel_search` (temporal_filtering.c:1536) — the shared refinement body
/// for all four block sizes.
///
/// The scan order is exactly C's and it matters, because ties keep the
/// incumbent: centre first, then half-pel `i,j in {-4, 0, 4}` skipping
/// `(0,0)`, then quarter-pel `{-2, 0, 2}`, then eighth-pel `{-1, 0, 1}` — each
/// level RE-CENTRED on the best MV found so far, not on the original.
///
/// `i` is the X offset and `j` the Y offset: C assigns `xd = i`, `yd = j` with
/// `i` the OUTER loop. Swapping them changes the order in which equal-cost
/// candidates are seen, and therefore which MV survives the strict-improvement
/// rule.
pub fn subpel_search<F>(
    p: &mut TfSubpelSearchParams,
    ctrls: &TfSearchCtrls,
    tf_subpel_early_exit_th: u32,
    best_dist: &mut u64,
    best_mv_x: &mut i16,
    best_mv_y: &mut i16,
    score: &mut F,
) where
    F: FnMut(i16, i16, u8) -> u64,
{
    // Check centre position.
    p.subpel_pel_mode = ctrls.half_pel_mode;
    p.mv_x = *best_mv_x;
    p.mv_y = *best_mv_y;
    p.xd = 0;
    p.yd = 0;
    check_position(
        p,
        tf_subpel_early_exit_th,
        best_dist,
        best_mv_x,
        best_mv_y,
        score,
    );

    for (mode, step) in [
        (ctrls.half_pel_mode, 4i16),
        (ctrls.quarter_pel_mode, 2),
        (ctrls.eight_pel_mode, 1),
    ] {
        if mode == 0 {
            continue;
        }
        p.subpel_pel_mode = mode;
        p.mv_x = *best_mv_x;
        p.mv_y = *best_mv_y;
        let mut i = -step;
        while i <= step {
            let mut j = -step;
            while j <= step {
                if i == 0 && j == 0 {
                    j += step;
                    continue;
                }
                p.xd = i;
                p.yd = j;
                check_position(
                    p,
                    tf_subpel_early_exit_th,
                    best_dist,
                    best_mv_x,
                    best_mv_y,
                    score,
                );
                j += step;
            }
            i += step;
        }
    }
}

/// The per-block sub-pel search parameters for one block size, as
/// `tf_64x64_sub_pel_search` / `tf_32x32_sub_pel_search` /
/// `tf_16x16_sub_pel_search` / `tf_8x8_sub_pel_search` build them
/// (temporal_filtering.c:1660 / :1770 / :1880 / :1990).
///
/// The four functions differ only in `bsize` and how `(idx_x, idx_y)` are
/// derived; the parameter block itself is assembled identically. Returned
/// rather than acted on so the caller supplies the compensator.
pub fn subpel_params_for_block(
    bsize: u32,
    idx_x: u32,
    idx_y: u32,
    sb_origin_x: u32,
    sb_origin_y: u32,
    ctrls: &TfSearchCtrls,
    interp_filters: u32,
    encoder_bit_depth: i32,
) -> TfSubpelSearchParams {
    let local_origin_x = (idx_x * bsize) as u16;
    let local_origin_y = (idx_y * bsize) as u16;
    TfSubpelSearchParams {
        subsampling_shift: ctrls.sub_sampling_shift,
        interp_filters,
        pu_origin_x: sb_origin_x as u16 + local_origin_x,
        pu_origin_y: sb_origin_y as u16 + local_origin_y,
        local_origin_x,
        local_origin_y,
        bsize,
        is_highbd: encoder_bit_depth != 8,
        encoder_bit_depth,
        idx_x,
        idx_y,
        ..Default::default()
    }
}

/// The `(idx_y, idx_x)` a 32x32 sub-pel search uses
/// (temporal_filtering.c:1826-1827): `idx_x = idx_32x32 & 1`,
/// `idx_y = idx_32x32 >> 1`.
pub fn subpel_idx_32x32(idx_32x32: u32) -> (u32, u32) {
    (idx_32x32 & 1, idx_32x32 >> 1)
}

/// The `(idx_y, idx_x)` a 16x16 sub-pel search uses
/// (temporal_filtering.c:1935-1938), via `idx_32x32_to_idx_16x16` and
/// `subblock_xy_16x16`. The table yields (ROW, COLUMN); C assigns
/// `idx_y = [..][0]` and `idx_x = [..][1]`.
pub fn subpel_idx_16x16(idx_32x32: usize, idx_16x16: usize) -> (u32, u32) {
    let pu_index = IDX_32X32_TO_IDX_16X16[idx_32x32][idx_16x16] as usize;
    (
        SUBBLOCK_XY_16X16[pu_index][1],
        SUBBLOCK_XY_16X16[pu_index][0],
    )
}

/// The `(idx_y, idx_x)` an 8x8 sub-pel search uses
/// (temporal_filtering.c:2044-2047). Reached only when
/// `tf_ctrls.enable_8x8_pred` is set, which is 0 at tf_level 5 (presets 3..7)
/// and 1 at tf_level 1/2 (presets 0..2).
pub fn subpel_idx_8x8(idx_32x32: usize, idx_16x16: usize, idx_8x8: usize) -> (u32, u32) {
    let pu_index = IDX_32X32_TO_IDX_8X8[idx_32x32][idx_16x16][idx_8x8] as usize;
    (SUBBLOCK_XY_8X8[pu_index][1], SUBBLOCK_XY_8X8[pu_index][0])
}

/// The per-plane accum/count and src/pred offsets
/// `apply_filtering_block_plane_wise` (temporal_filtering.c:1289) computes
/// before dispatching to a filter kernel.
///
/// Wrong offsets silently mix planes, which is exactly the class of bug a
/// visual check misses. Note the SRC offsets use the source strides and the
/// BLOCK offsets use the prediction strides — the accum/count pointers are
/// advanced by the PREDICTION offset, not the source one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TfBlockOffsets {
    pub src: [usize; 3],
    pub block: [usize; 3],
}

/// `apply_filtering_block_plane_wise` (temporal_filtering.c:1289), offset half.
/// `static` in C — TIER 4.
///
/// The dispatch half selects the `zz` vs `medium` kernel on
/// `tf_ctrls.use_zz_based_filter` and the 8-bit vs hbd kernel on
/// `encoder_bit_depth`. The `zz` arm is DEAD (`use_zz_based_filter` is set
/// only by `tf_ld_controls` levels 1/2, which `derive_tf_params` never
/// selects), so the live dispatch is `medium` / `medium_hbd`, both of which
/// this module ports and gates at tier 1.
pub fn apply_filtering_block_plane_wise_offsets(
    block_row: usize,
    block_col: usize,
    stride: &[usize; 3],
    stride_pred: &[usize; 3],
    block_width: usize,
    block_height: usize,
    ss_x: u32,
    ss_y: u32,
) -> TfBlockOffsets {
    let blk_h = block_height;
    let blk_w = block_width;
    let ch_h = blk_h >> ss_y;
    let ch_w = blk_w >> ss_x;
    TfBlockOffsets {
        src: [
            block_row * blk_h * stride[0] + block_col * blk_w,
            block_row * ch_h * stride[1] + block_col * ch_w,
            block_row * ch_h * stride[2] + block_col * ch_w,
        ],
        block: [
            block_row * blk_h * stride_pred[0] + block_col * blk_w,
            block_row * ch_h * stride_pred[1] + block_col * ch_w,
            block_row * ch_h * stride_pred[2] + block_col * ch_w,
        ],
    }
}

/// `SearchAreaMinMax` — the two width/height pairs
/// `set_hme_search_params_mctf` writes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SearchAreaMinMax {
    pub sa_min: (u16, u16),
    pub sa_max: (u16, u16),
}

/// `set_hme_search_params_mctf` (temporal_filtering.c:2571). `static` in C —
/// TIER 4.
///
/// Sets TF's OWN HME search-area parameters, which are NOT the encode-path
/// HME parameters — reusing the encode-path ones would look right and search a
/// different window. `tf_ctrls.hme_me_level` is 2 at tf_level 5, but this
/// function only accepts levels 0 and 1 (C asserts on anything else), so
/// hme_me_level 2 means "do not call this at all".
///
/// The two levels are NOT the same shift: level 1 doubles `sa_min` but
/// QUADRUPLES `sa_max`.
pub fn set_hme_search_params_mctf(
    default_tf: SearchAreaMinMax,
    hme_search_level: u8,
) -> Option<SearchAreaMinMax> {
    match hme_search_level {
        0 => Some(default_tf),
        1 => Some(SearchAreaMinMax {
            sa_min: (default_tf.sa_min.0 << 1, default_tf.sa_min.1 << 1),
            sa_max: (default_tf.sa_max.0 << 2, default_tf.sa_max.1 << 2),
        }),
        // C `assert(0)` here and leaves the fields unchanged in a release
        // build; refusing is the port's equivalent (WORKING-ON-THIS.md §6).
        _ => None,
    }
}

/// `filt_unfilt_dist` (temporal_filtering.c:3922). `static` in C — TIER 4.
///
/// Computes `filt_to_unfilt_diff` on the I_SLICE. That value is carried
/// forward and combined with the noise level to bump `decay_control[Y]` on
/// later non-I frames (temporal_filtering.c:2677-2685, pd_process.c:3660), so
/// skipping it silently changes every subsequent frame's TF strength.
///
/// The per-b64 spatial distortion is INJECTED as `sad` — it is
/// `svt_spatial_full_distortion_kernel`, which the port owns in `svtav1-dsp`.
/// What is transcribed here is the b64 tiling, the partial-block clamping
/// against the ALIGNED dimensions, and the final division by the b64 COUNT
/// (not by the pixel count).
pub fn filt_unfilt_dist<F>(
    aligned_width: u32,
    aligned_height: u32,
    b64_size: u32,
    filt_stride: usize,
    unfilt_stride: usize,
    mut sad: F,
) -> u32
where
    F: FnMut(usize, usize, usize, usize, u32, u32) -> u64,
{
    let pic_width_in_b64 = aligned_width.div_ceil(b64_size);
    let pic_height_in_b64 = aligned_height.div_ceil(b64_size);

    let mut dist: u32 = 0;
    for y_b64_idx in 0..pic_height_in_b64 {
        for x_b64_idx in 0..pic_width_in_b64 {
            // The origins step by a LITERAL 64 while the clamp uses
            // `scs->b64_size` — they are the same today, but transcribe both
            // as C spells them.
            let b64_origin_x = x_b64_idx * 64;
            let b64_origin_y = y_b64_idx * 64;
            let filt_offset = b64_origin_y as usize * filt_stride + b64_origin_x as usize;
            let unfilt_offset = b64_origin_y as usize * unfilt_stride + b64_origin_x as usize;
            let b64_width = b64_size.min(aligned_width - b64_origin_x);
            let b64_height = b64_size.min(aligned_height - b64_origin_y);
            dist = dist.wrapping_add(sad(
                filt_offset,
                filt_stride,
                unfilt_offset,
                unfilt_stride,
                b64_width,
                b64_height,
            ) as u32);
        }
    }
    dist / (pic_width_in_b64 * pic_height_in_b64)
}

// ---------------------------------------------------------------------------
// Motion-compensated prediction dispatch and the source-buffer saves
// ---------------------------------------------------------------------------

/// One motion-compensated prediction the TF driver asks for: which block size,
/// where, and at what MV.
///
/// `tf_64x64_inter_prediction` / `tf_32x32_inter_prediction`
/// (temporal_filtering.c:2102 / :2199) each end in a call to
/// `svt_aom_inter_prediction` with a `BlockModeInfo` whose only varying fields
/// are `mv[0]` and the block size — everything else is fixed
/// (`ref_frame[0] = LAST_FRAME`, `ref_frame[1] = NONE_FRAME`,
/// `is_interintra_used = 0`, `motion_mode = SIMPLE_TRANSLATION`,
/// `mode = NEWMV`, `use_intrabc = 0`) and the interp filters are
/// `MULTITAP_SHARP` on both axes — NOT the `EIGHTTAP_REGULAR`/`BILINEAR` pair
/// the SUB-PEL SEARCH uses. Using the search's filters for the final pass
/// gives a different, plausible-looking prediction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TfPredictionRequest {
    /// 64, 32, 16 or 8.
    pub bsize: u32,
    pub pu_origin_x: u16,
    pub pu_origin_y: u16,
    pub local_origin_x: u16,
    pub local_origin_y: u16,
    pub mv_x: i16,
    pub mv_y: i16,
    /// `mi_size_high[block_size]` / `mi_size_wide[block_size]` for the edge
    /// clamps, in MI units (`bsize / MI_SIZE`, `MI_SIZE == 4`).
    pub mi_size: i32,
}

/// The four `mb_to_*_edge` values C writes into `blk_ptr.av1xd` before each
/// prediction, in eighth-pel units.
///
/// `MI_SIZE_LOG2` is 2 and `MI_SIZE` is 4, so `mirow = pu_origin_y >> 2` and
/// the edges are `(mi * 4) * 8`. Getting the sign wrong on the top/left pair
/// (they are NEGATED) silently disables the MC's edge clamping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MbEdges {
    pub top: i32,
    pub bottom: i32,
    pub left: i32,
    pub right: i32,
}

/// `MI_SIZE` / `MI_SIZE_LOG2`.
pub const MI_SIZE: i32 = 4;
pub const MI_SIZE_LOG2: u32 = 2;

/// The `blk_ptr.av1xd->mb_to_*_edge` derivation shared by every TF prediction
/// and sub-pel search site (temporal_filtering.c:1720-1726 and friends).
pub fn mb_edges(
    pu_origin_x: u16,
    pu_origin_y: u16,
    mi_size_wide: i32,
    mi_size_high: i32,
    mi_cols: i32,
    mi_rows: i32,
) -> MbEdges {
    let mirow = i32::from(pu_origin_y >> MI_SIZE_LOG2);
    let micol = i32::from(pu_origin_x >> MI_SIZE_LOG2);
    MbEdges {
        top: -((mirow * MI_SIZE) * 8),
        bottom: ((mi_rows - mi_size_high - mirow) * MI_SIZE) * 8,
        left: -((micol * MI_SIZE) * 8),
        right: ((mi_cols - mi_size_wide - micol) * MI_SIZE) * 8,
    }
}

/// `tf_64x64_inter_prediction` (temporal_filtering.c:2102), request half.
/// `static` in C — TIER 4.
///
/// One unconditional 64x64 prediction at `tf_64x64_mv_*`.
pub fn tf_64x64_inter_prediction_request(
    sb_origin_x: u32,
    sb_origin_y: u32,
    tf_64x64_mv_x: i16,
    tf_64x64_mv_y: i16,
) -> TfPredictionRequest {
    TfPredictionRequest {
        bsize: 64,
        pu_origin_x: sb_origin_x as u16,
        pu_origin_y: sb_origin_y as u16,
        local_origin_x: 0,
        local_origin_y: 0,
        mv_x: tf_64x64_mv_x,
        mv_y: tf_64x64_mv_y,
        mi_size: 64 / MI_SIZE,
    }
}

/// `tf_32x32_inter_prediction` (temporal_filtering.c:2199), request half.
/// `static` in C — TIER 4.
///
/// Despite the name this descends: when `tf_32x32_block_split_flag[idx]` is
/// set it emits four 16x16 predictions, or four 8x8 ones for each 16x16 whose
/// `tf_16x16_block_split_flag` is also set; otherwise ONE 32x32. So the
/// returned list has 1, 4, or between 4 and 16 entries.
///
/// Each level takes its MV from its own array: `tf_32x32_mv_*[idx_32x32]`,
/// `tf_16x16_mv_*[idx_32x32 * 4 + idx_16x16]`, or
/// `tf_8x8_mv_*[idx_32x32 * 16 + 4 * idx_16x16 + idx_8x8]`.
pub fn tf_32x32_inter_prediction_requests(
    ctx: &TfKernelCtx,
    tf_16x16_block_split_flag: &[[i32; 4]; 4],
    tf_8x8_mv_x: &[i16; 64],
    tf_8x8_mv_y: &[i16; 64],
    idx_32x32: usize,
    sb_origin_x: u32,
    sb_origin_y: u32,
) -> Vec<TfPredictionRequest> {
    let mut out = Vec::new();
    let mk = |bsize: u32, idx_x: u32, idx_y: u32, mv_x: i16, mv_y: i16| {
        let local_origin_x = (idx_x * bsize) as u16;
        let local_origin_y = (idx_y * bsize) as u16;
        TfPredictionRequest {
            bsize,
            pu_origin_x: sb_origin_x as u16 + local_origin_x,
            pu_origin_y: sb_origin_y as u16 + local_origin_y,
            local_origin_x,
            local_origin_y,
            mv_x,
            mv_y,
            mi_size: (bsize as i32) / MI_SIZE,
        }
    };

    if ctx.tf_32x32_block_split_flag[idx_32x32] != 0 {
        for idx_16x16 in 0..4usize {
            if tf_16x16_block_split_flag[idx_32x32][idx_16x16] != 0 {
                for idx_8x8 in 0..4usize {
                    let (idx_x, idx_y) = subpel_idx_8x8(idx_32x32, idx_16x16, idx_8x8);
                    let k = idx_32x32 * 16 + 4 * idx_16x16 + idx_8x8;
                    out.push(mk(8, idx_x, idx_y, tf_8x8_mv_x[k], tf_8x8_mv_y[k]));
                }
            } else {
                let (idx_x, idx_y) = subpel_idx_16x16(idx_32x32, idx_16x16);
                let k = idx_32x32 * 4 + idx_16x16;
                out.push(mk(
                    16,
                    idx_x,
                    idx_y,
                    ctx.tf_16x16_mv_x[k],
                    ctx.tf_16x16_mv_y[k],
                ));
            }
        }
    } else {
        let (idx_x, idx_y) = subpel_idx_32x32(idx_32x32 as u32);
        out.push(mk(
            32,
            idx_x,
            idx_y,
            ctx.tf_32x32_mv_x[idx_32x32],
            ctx.tf_32x32_mv_y[idx_32x32],
        ));
    }
    out
}

/// Which planes a source-buffer save copies, and at what dimensions.
///
/// `save_src_pic_buffers` (temporal_filtering.c:3790) copies Y, U and V
/// (`PICTURE_BUFFER_DESC_FULL_MASK`); `save_y_src_pic_buffers`
/// (temporal_filtering.c:3874) copies Y only
/// (`PICTURE_BUFFER_DESC_LUMA_MASK`). Both allocate the destination with
/// `border = 0` and `split_mode = (bit_depth > 8)`.
///
/// Which one runs is decided at temporal_filtering.c:4018: the ALL-PLANES
/// variant needs `compute_psnr`/`compute_ssim` or superres recode; the default
/// configuration takes the LUMA-ONLY else-branch. Picking the wrong branch
/// still produces a saved buffer, so nothing fails loudly — it just changes
/// what `filt_unfilt_dist` differences against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SavedSrcPicPlan {
    pub copy_luma: bool,
    pub copy_chroma: bool,
    pub width_y: u32,
    pub height_y: u32,
    pub width_uv: u32,
    pub height_uv: u32,
    pub dest_border: u32,
    pub dest_split_mode: bool,
}

/// `save_src_pic_buffers` / `save_y_src_pic_buffers` (temporal_filtering.c:3790
/// / :3874), planning half. `static` in C — TIER 4.
///
/// `all_planes` selects between them. The chroma subsampling comes from
/// `config->encoder_color_format` (`EB_YUV444 == 3` for ss_x,
/// `>= EB_YUV422 == 2` for ss_y), NOT from the SequenceControlSet's
/// `subsampling_*` — a different source of truth from
/// `svt_aom_pad_input_pictures`, which reads the scs fields for the same
/// quantity.
pub fn plan_saved_src_pic(
    all_planes: bool,
    width: u32,
    height: u32,
    color_format: u32,
    encoder_bit_depth: u32,
) -> SavedSrcPicPlan {
    let ss_x = u32::from(color_format != 3);
    let ss_y = u32::from(color_format < 2);
    let is_16bit = encoder_bit_depth > 8;
    SavedSrcPicPlan {
        copy_luma: true,
        copy_chroma: all_planes,
        width_y: width,
        height_y: height,
        width_uv: width >> ss_x,
        height_uv: height >> ss_y,
        dest_border: 0,
        dest_split_mode: is_16bit,
    }
}

// ---------------------------------------------------------------------------
// The driver's decision layer: svt_av1_init_temporal_filtering (:3951) and
// produce_temporally_filtered_pic (:2594)
// ---------------------------------------------------------------------------

/// `TF_QINDEX_CUTOFF` / `TF_Q_DECAY_THRESHOLD` (temporal_filtering.h:60/:43)
/// and `VQ_PIC_AVG_VARIANCE_TH` (definitions.h:85).
pub const TF_QINDEX_CUTOFF: i32 = 128;
pub const TF_Q_DECAY_THRESHOLD: i32 = 20;
pub const VQ_PIC_AVG_VARIANCE_TH: u16 = 1000;
/// `FIXED_QP_OFFSET_COUNT` (md_process.h:1319).
pub const FIXED_QP_OFFSET_COUNT: usize = 6;

/// `percents` (md_process.c:25) — the libaom fixed-QP offsets.
///
/// TRAP, and the brief warned about exactly this shape: C indexes it
/// `percents[centre_pcs->hierarchical_levels <= 4][offset_idx]`. The first
/// index is a BOOLEAN, not a level. Row 0 is "more than 4 hierarchical
/// levels"; row 1 is "4 or fewer". Reading `hierarchical_levels` as the index
/// silently picks the wrong row (and reads out of bounds above 1).
pub const PERCENTS: [[i32; FIXED_QP_OFFSET_COUNT]; 2] =
    [[75, 70, 60, 20, 15, 0], [76, 60, 30, 15, 8, 4]];

/// `me_ctx->tf_chroma` as `svt_av1_init_temporal_filtering` derives it
/// (temporal_filtering.c:3960-3963). EXPORTED parent — the value is a
/// transcription of the exported function's body; see the module note on
/// tiering for this group.
///
/// `chroma_lvl == 1` always filters chroma; `chroma_lvl == 2` filters it only
/// when the frame is chroma-noisy, defined as EITHER chroma plane's
/// `noise_levels_log1p_fp16` exceeding luma's. Anything else is off.
pub fn init_tf_chroma(chroma_lvl: u8, noise_levels_log1p_fp16: &[i32; 3]) -> bool {
    let high_chroma_noise_lvl = noise_levels_log1p_fp16[0] < noise_levels_log1p_fp16[1]
        || noise_levels_log1p_fp16[0] < noise_levels_log1p_fp16[2];
    match chroma_lvl {
        1 => true,
        2 => high_chroma_noise_lvl,
        _ => false,
    }
}

/// `me_ctx->tf_mv_dist_th` (temporal_filtering.c:4018).
///
/// `CLIP3(64, 450, MIN(aligned_height, aligned_width) - 150)`. The subtraction
/// is done in SIGNED int, so a picture smaller than 150 px on its short axis
/// clamps up to 64 rather than wrapping.
pub fn init_tf_mv_dist_th(aligned_width: u32, aligned_height: u32) -> u32 {
    let v = aligned_height.min(aligned_width) as i32 - 150;
    v.clamp(64, 450) as u32
}

/// Which source-buffer save `svt_av1_init_temporal_filtering` performs
/// (temporal_filtering.c:4008-4014), if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SrcSaveChoice {
    /// `save_src_pic_buffers` — all planes.
    AllPlanes,
    /// `save_y_src_pic_buffers` — luma only.
    LumaOnly,
    None,
}

/// `SUPERRES_AUTO` / `SUPERRES_AUTO_DUAL` / `SUPERRES_AUTO_ALL` and the two
/// frame-update types the superres recode applies to.
///
/// The superres-recode predicate is
/// `mode == SUPERRES_AUTO && (search == DUAL || search == ALL) &&
///  (update == KF || update == ARF)` — recode applies only to key and arf.
#[allow(clippy::too_many_arguments)]
pub fn choose_src_save(
    compute_psnr: bool,
    compute_ssim: bool,
    superres_mode_is_auto: bool,
    superres_search_is_dual_or_all: bool,
    frame_update_is_kf_or_arf: bool,
    slice_is_i: bool,
) -> SrcSaveChoice {
    let superres_recode_enabled =
        superres_mode_is_auto && superres_search_is_dual_or_all && frame_update_is_kf_or_arf;
    if compute_psnr || compute_ssim || superres_recode_enabled {
        SrcSaveChoice::AllPlanes
    } else if slice_is_i {
        SrcSaveChoice::LumaOnly
    } else {
        SrcSaveChoice::None
    }
}

/// Which TF driver `svt_av1_init_temporal_filtering` dispatches to
/// (temporal_filtering.c:4031).
///
/// `produce_temporally_filtered_pic_ld` for `LOW_DELAY`, otherwise
/// `produce_temporally_filtered_pic`. MEASURED: `derive_tf_params`
/// (enc_handle.c:3338-3343) returns `tf_level = 0` for all LOW_DELAY, so the
/// `_ld` arm is not reached by the default configuration and the RANDOM_ACCESS
/// driver is the live one.
pub fn init_tf_driver_is_low_delay(pred_structure_is_low_delay: bool) -> bool {
    pred_structure_is_low_delay
}

/// `decay_control[Y/U/V]` as `produce_temporally_filtered_pic` derives it
/// (temporal_filtering.c:2667-2685). `static` in C — TIER 4.
///
/// The VQ arm sets all three to 1; otherwise the defaults are (3, 6, 6) and
/// the LUMA one is bumped by 1 on a non-I slice whose
/// `filt_to_unfilt_diff * 100 / noise_levels_log1p_fp16[Y]` exceeds 150.
/// That ratio is where `filt_unfilt_dist`'s I-slice output re-enters the
/// pipeline, which is why skipping it changes every later frame's TF strength.
///
/// The ratio guards against a zero denominator by yielding 0, not by dividing.
pub fn derive_decay_control(
    vq_sharpness_tf: bool,
    is_noise_level: bool,
    calculate_variance: bool,
    pic_avg_variance: u16,
    slice_is_i: bool,
    filt_to_unfilt_diff: i32,
    noise_levels_log1p_fp16: &[i32; 3],
) -> [i32; 3] {
    if vq_sharpness_tf
        && is_noise_level
        && calculate_variance
        && pic_avg_variance < VQ_PIC_AVG_VARIANCE_TH
    {
        return [1, 1, 1];
    }
    let mut dc = [3, 6, 6];
    if !slice_is_i {
        let ratio = if noise_levels_log1p_fp16[0] != 0 {
            (filt_to_unfilt_diff * 100) / noise_levels_log1p_fp16[0]
        } else {
            0
        };
        if ratio > 150 {
            dc[0] += 1;
        }
    }
    dc
}

/// The fixed-QP offset row index and slot `produce_temporally_filtered_pic`
/// uses (temporal_filtering.c:2698-2705). `static` in C — TIER 4.
///
/// `offset_idx` is -1 for a non-reference frame, 0 for an IDR, else
/// `min(temporal_layer_index + 1, FIXED_QP_OFFSET_COUNT - 1)`. -1 means "no
/// offset at all", which is a DIFFERENT outcome from slot 0.
pub fn tf_qp_offset_idx(is_ref: bool, idr_flag: bool, temporal_layer_index: i32) -> i32 {
    if !is_ref {
        -1
    } else if idr_flag {
        0
    } else {
        (temporal_layer_index + 1).min(FIXED_QP_OFFSET_COUNT as i32 - 1)
    }
}

/// The target q the TF strength derivation aims at
/// (temporal_filtering.c:2710-2712). `static` in C — TIER 4.
///
/// `offset_idx == -1` leaves `q_val_fp8` alone; otherwise it subtracts
/// `q_val_fp8 * percents[hierarchical_levels <= 4][offset_idx] / 100`, floored
/// at 0.
///
/// See `PERCENTS`: the first index is the BOOLEAN `hierarchical_levels <= 4`.
pub fn tf_q_val_target_fp8(q_val_fp8: i32, offset_idx: i32, hierarchical_levels: u32) -> i32 {
    if offset_idx == -1 {
        return q_val_fp8;
    }
    let row = usize::from(hierarchical_levels <= 4);
    let pct = PERCENTS[row][offset_idx as usize];
    (q_val_fp8 - (q_val_fp8 * pct / 100)).max(0)
}

/// `q_decay_fp8` (temporal_filtering.c:2723-2728). `static` in C — TIER 4.
///
/// Two arms around `TF_QINDEX_CUTOFF`: at or above it `q_decay = (q*q) >> 5`,
/// below it `max(q << 2, 1)`. The `q >= cutoff` arm is NOT a continuation of
/// the other — the curve is deliberately discontinuous.
pub fn tf_q_decay_fp8(q: i32) -> u32 {
    if q >= TF_QINDEX_CUTOFF {
        ((q * q) >> 5) as u32
    } else {
        (q << 2).max(1) as u32
    }
}

/// The shift factor and the "TF off on this key frame" decision
/// (temporal_filtering.c:2739-2800). `static` in C — TIER 4.
///
/// THE SVT_HDR_MODE TRAP LIVES HERE. Under `#if SVT_HDR_MODE` C computes
/// `kf_tf_shift_factor = 10 + (4 - kf_tf_strength)`. The MAINLINE `#else` —
/// the arm the oracle compiles, and the only one this port implements — sets
/// `kf_tf_shift_factor = tf_shift_factor` and raises it by 1 (capped at 14)
/// only when `vq_ctrls.sharpness_ctrls.tf` is set. Measured confirmation:
/// sweeping `SVT_FORK_KF_TF_STRENGTH` over 0..4 at RANDOM_ACCESS 2f/8f/16f
/// leaves frame 0 byte-identical, while sweeping `SVT_FORK_TF_STRENGTH` moves
/// it.
///
/// `enable_tf > 1` selects the ADAPTIVE arm, where the base is
/// `calculate_tf_shift_factor(tf_64x64_block_error)` and the key-frame variant
/// is that plus 1, clipped to 0..=14.
///
/// When the key-frame shift factor lands on exactly 14, TF is DISABLED on the
/// key frame: all three decay factors are zeroed. That is the switch the
/// measured `SVT_FORK_TF_STRENGTH=0` run flipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TfShiftDecision {
    pub shift_factor: u8,
    pub kf_shift_factor: u8,
    /// True when the decay factors are zeroed instead of derived.
    pub disable_on_this_frame: bool,
}

pub fn derive_tf_shift(
    enable_tf: u8,
    tf_strength: u8,
    vq_sharpness_tf: bool,
    frame_update_is_kf: bool,
    tf_64x64_block_error: u64,
) -> TfShiftDecision {
    let (shift_factor, kf_shift_factor) = if enable_tf > 1 {
        let adaptive = calculate_tf_shift_factor(tf_64x64_block_error);
        debug_assert!(adaptive <= 14);
        // C: CLIP3(0, 14, adaptive_tf_shift_factor + 1).
        (adaptive, (i32::from(adaptive) + 1).clamp(0, 14) as u8)
    } else {
        // 10 + (4 - tf_strength): 0 -> 14 (8x weaker) .. 4 -> 10 (2x stronger).
        let shift = (10 + (4 - i32::from(tf_strength))) as u8;
        // MAINLINE (#else): kf_tf_shift_factor = tf_shift_factor, raised by 1
        // and capped at 14 only under Tune VQ sharpness controls. The
        // SVT_HDR_MODE arm reads kf_tf_strength instead and is NOT ported.
        let kf = if vq_sharpness_tf {
            (shift + 1).min(14)
        } else {
            shift
        };
        (shift, kf)
    };
    TfShiftDecision {
        shift_factor,
        kf_shift_factor,
        disable_on_this_frame: frame_update_is_kf && kf_shift_factor == 14,
    }
}

/// The block grid `produce_temporally_filtered_pic` walks
/// (temporal_filtering.c:2630-2634). `static` in C — TIER 4.
///
/// `blk_cols = ceil(width / TF_BW)`, `blk_rows = ceil(height / TF_BH)`, both
/// over the CENTRAL INPUT picture's dimensions — the unscaled source, not the
/// padded one. Prediction strides are `TF_BW` for luma and `TF_BW >> ss_x` for
/// both chroma planes (ss_x for BOTH, matching the central-filter kernel).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TfBlockGrid {
    pub blk_cols: u32,
    pub blk_rows: u32,
    pub blk_width_ch: u32,
    pub blk_height_ch: u32,
    pub stride_pred: [u32; 3],
}

pub fn tf_block_grid(width: u32, height: u32, ss_x: u32, ss_y: u32) -> TfBlockGrid {
    let blk_width_ch = (TF_BW as u32) >> ss_x;
    let blk_height_ch = (TF_BH as u32) >> ss_y;
    TfBlockGrid {
        blk_cols: width.div_ceil(TF_BW as u32),
        blk_rows: height.div_ceil(TF_BH as u32),
        blk_width_ch,
        blk_height_ch,
        stride_pred: [TF_BW as u32, blk_width_ch, blk_width_ch],
    }
}
