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
