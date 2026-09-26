//! Forward transforms (DCT, ADST, identity).
//!
//! Spec 04 (transforms.md): Forward DCT/ADST/identity transforms.
//!
//! Ported from SVT-AV1's `transforms.c` and `inv_transforms.c`.
//! All transforms are separable (1D column → 1D row) per AV1 spec.
//!
//! Cosine constants from `svt_aom_eb_av1_cospi_arr_data` in `inv_transforms.c`.

use alloc::vec;
use archmage::prelude::*;
use svtav1_types::transform::TranLow;

// =============================================================================
// Cosine constant tables — one row per cos_bit in 10..=16.
// cospi_arr_data[i][j] = round(cos(j * pi / 128) * 2^(10 + i))
// Port of svt_aom_eb_av1_cospi_arr_data (inv_transforms.c:3289).
// =============================================================================
pub const COS_BIT_MIN: i8 = 10;

#[rustfmt::skip]
pub const COSPI_ARR_DATA: [[i32; 64]; 7] = [
    [1024, 1024, 1023, 1021, 1019, 1016, 1013, 1009, 1004, 999, 993, 987, 980, 972, 964, 955,
     946,  936,  926,  915,  903,  891,  878,  865,  851,  837, 822, 807, 792, 775, 759, 742,
     724,  706,  688,  669,  650,  630,  610,  590,  569,  548, 526, 505, 483, 460, 438, 415,
     392,  369,  345,  321,  297,  273,  249,  224,  200,  175, 150, 125, 100, 75,  50,  25],
    [2048, 2047, 2046, 2042, 2038, 2033, 2026, 2018, 2009, 1998, 1987, 1974, 1960, 1945, 1928, 1911,
     1892, 1872, 1851, 1829, 1806, 1782, 1757, 1730, 1703, 1674, 1645, 1615, 1583, 1551, 1517, 1483,
     1448, 1412, 1375, 1338, 1299, 1260, 1220, 1179, 1138, 1096, 1053, 1009, 965,  921,  876,  830,
     784,  737,  690,  642,  595,  546,  498,  449,  400,  350,  301,  251,  201,  151,  100,  50],
    [4096, 4095, 4091, 4085, 4076, 4065, 4052, 4036, 4017, 3996, 3973, 3948, 3920, 3889, 3857, 3822,
     3784, 3745, 3703, 3659, 3612, 3564, 3513, 3461, 3406, 3349, 3290, 3229, 3166, 3102, 3035, 2967,
     2896, 2824, 2751, 2675, 2598, 2520, 2440, 2359, 2276, 2191, 2106, 2019, 1931, 1842, 1751, 1660,
     1567, 1474, 1380, 1285, 1189, 1092, 995,  897,  799,  700,  601,  501,  401,  301,  201,  101],
    [8192, 8190, 8182, 8170, 8153, 8130, 8103, 8071, 8035, 7993, 7946, 7895, 7839, 7779, 7713, 7643,
     7568, 7489, 7405, 7317, 7225, 7128, 7027, 6921, 6811, 6698, 6580, 6458, 6333, 6203, 6070, 5933,
     5793, 5649, 5501, 5351, 5197, 5040, 4880, 4717, 4551, 4383, 4212, 4038, 3862, 3683, 3503, 3320,
     3135, 2948, 2760, 2570, 2378, 2185, 1990, 1795, 1598, 1401, 1202, 1003, 803,  603,  402,  201],
    [16384, 16379, 16364, 16340, 16305, 16261, 16207, 16143, 16069, 15986, 15893, 15791, 15679, 15557, 15426, 15286,
     15137, 14978, 14811, 14635, 14449, 14256, 14053, 13842, 13623, 13395, 13160, 12916, 12665, 12406, 12140, 11866,
     11585, 11297, 11003, 10702, 10394, 10080, 9760,  9434,  9102,  8765,  8423,  8076,  7723,  7366,  7005,  6639,
     6270,  5897,  5520,  5139,  4756,  4370,  3981,  3590,  3196,  2801,  2404,  2006,  1606,  1205,  804,   402],
    [32768, 32758, 32729, 32679, 32610, 32522, 32413, 32286, 32138, 31972, 31786, 31581, 31357, 31114, 30853, 30572,
     30274, 29957, 29622, 29269, 28899, 28511, 28106, 27684, 27246, 26791, 26320, 25833, 25330, 24812, 24279, 23732,
     23170, 22595, 22006, 21403, 20788, 20160, 19520, 18868, 18205, 17531, 16846, 16151, 15447, 14733, 14010, 13279,
     12540, 11793, 11039, 10279, 9512,  8740,  7962,  7180,  6393,  5602,  4808,  4011,  3212,  2411,  1608,  804],
    [65536, 65516, 65457, 65358, 65220, 65043, 64827, 64571, 64277, 63944, 63572, 63162, 62714, 62228, 61705, 61145,
     60547, 59914, 59244, 58538, 57798, 57022, 56212, 55368, 54491, 53581, 52639, 51665, 50660, 49624, 48559, 47464,
     46341, 45190, 44011, 42806, 41576, 40320, 39040, 37736, 36410, 35062, 33692, 32303, 30893, 29466, 28020, 26558,
     25080, 23586, 22078, 20557, 19024, 17479, 15924, 14359, 12785, 11204, 9616,  8022,  6424,  4821,  3216,  1608],
];

/// Sinusoidal constants for ADST-4, one row per cos_bit in 10..=16.
/// Port of svt_aom_eb_av1_sinpi_arr_data (inv_transforms.c:3321).
#[rustfmt::skip]
pub const SINPI_ARR_DATA: [[i32; 5]; 7] = [
    [0, 330, 621, 836, 951],
    [0, 660, 1241, 1672, 1901],
    [0, 1321, 2482, 3344, 3803],
    [0, 2642, 4964, 6689, 7606],
    [0, 5283, 9929, 13377, 15212],
    [0, 10566, 19858, 26755, 30424],
    [0, 21133, 39716, 53510, 60849],
];

/// C `cospi_arr(n)` — select the cosine table row for a cos_bit.
#[inline]
pub fn cospi_arr(cos_bit: i8) -> &'static [i32; 64] {
    &COSPI_ARR_DATA[(cos_bit - COS_BIT_MIN) as usize]
}

/// C `sinpi_arr(n)` — select the ADST-4 sine table row for a cos_bit.
#[inline]
pub fn sinpi_arr(cos_bit: i8) -> &'static [i32; 5] {
    &SINPI_ARR_DATA[(cos_bit - COS_BIT_MIN) as usize]
}

/// Cosine constant table — Q12 row (cos_bit = 12).
pub const COSPI: [i32; 64] = COSPI_ARR_DATA[2];

/// Sinusoidal constants for ADST-4 (Q12).
pub const SINPI: [i32; 5] = SINPI_ARR_DATA[2];

/// Default cos_bit for transforms (the inverse always uses 12 = INV_COS_BIT;
/// the forward uses per-size bits from `FWD_COS_BIT_COL`/`FWD_COS_BIT_ROW`).
pub const COS_BIT: u32 = 12;

/// C `fwd_cos_bit_col[txw_idx][txh_idx]` (transforms.c:17).
/// txw_idx = log2(width) - 2, txh_idx = log2(height) - 2.
#[rustfmt::skip]
pub const FWD_COS_BIT_COL: [[i8; 5]; 5] = [
    [13, 13, 13,  0,  0],
    [13, 13, 13, 12,  0],
    [13, 13, 13, 12, 13],
    [ 0, 13, 13, 12, 13],
    [ 0,  0, 13, 12, 13],
];

/// C `fwd_cos_bit_row[txw_idx][txh_idx]` (transforms.c:19).
#[rustfmt::skip]
pub const FWD_COS_BIT_ROW: [[i8; 5]; 5] = [
    [13, 13, 12,  0,  0],
    [13, 13, 13, 12,  0],
    [13, 13, 12, 13, 12],
    [ 0, 12, 13, 12, 11],
    [ 0,  0, 12, 11, 10],
];

/// C `fwd_txfm_shift_ls` (transforms.c:702-725), keyed by (width, height).
pub fn fwd_txfm_shift(w: usize, h: usize) -> [i8; 3] {
    match (w, h) {
        (4, 4) => [2, 0, 0],
        (8, 8) => [2, -1, 0],
        (16, 16) => [2, -2, 0],
        (32, 32) => [2, -4, 0],
        (64, 64) => [0, -2, -2],
        (4, 8) | (8, 4) => [2, -1, 0],
        (8, 16) | (16, 8) => [2, -2, 0],
        (16, 32) | (32, 16) => [2, -4, 0],
        (32, 64) => [0, -2, -2],
        (64, 32) => [2, -4, -2],
        (4, 16) | (16, 4) => [2, -1, 0],
        (8, 32) | (32, 8) => [2, -2, 0],
        (16, 64) => [0, -2, 0],
        (64, 16) => [2, -4, 0],
        _ => unreachable!("unsupported transform size {w}x{h}"),
    }
}

/// New sqrt(2) constant for rectangular transform scaling.
pub const NEW_SQRT2: i32 = 5793; // 2^12 * sqrt(2)
pub const NEW_SQRT2_BITS: u32 = 12;

/// Round-shift a value by `bit` positions with rounding.
#[inline]
pub fn round_shift(value: i32, bit: u32) -> i32 {
    if bit == 0 {
        value
    } else {
        (value + (1 << (bit - 1))) >> bit
    }
}

/// Round-shift for i64 values.
#[inline]
pub fn round_shift_i64(value: i64, bit: u32) -> i32 {
    if bit == 0 {
        value as i32
    } else {
        ((value + (1i64 << (bit - 1))) >> bit) as i32
    }
}

/// Half-butterfly: (w0 * in0 + w1 * in1 + rounding) >> cos_bit
#[inline]
pub fn half_btf(w0: i32, in0: i32, w1: i32, in1: i32, cos_bit: u32) -> i32 {
    let result = w0 as i64 * in0 as i64 + w1 as i64 * in1 as i64;
    round_shift_i64(result, cos_bit)
}

/// Per-thread staging buffer for the scalar 2D transform cores. `buf` is
/// fully written by the first pass before the second pass reads it, so a
/// fresh `vec![0i32; w*h]` per call pays an alloc + memset of up to 16 KB
/// for bytes nobody reads. Under `std` a thread-local stage is reused;
/// a re-entrant borrow or `no_std` falls back to a fresh Vec.
#[inline]
pub(crate) fn with_txfm_stage<R>(n: usize, f: impl FnOnce(&mut [i32]) -> R) -> R {
    debug_assert!(n <= 4096);
    #[cfg(feature = "std")]
    {
        use core::cell::RefCell;
        std::thread_local! {
            static TXFM_CORE_STAGE: RefCell<[i32; 4096]> = const { RefCell::new([0; 4096]) };
        }
        // Err carries the unconsumed closure so the fallback can run it.
        match TXFM_CORE_STAGE.with(|c| match c.try_borrow_mut() {
            Ok(mut b) => Ok(f(&mut b[..n])),
            Err(_) => Err(f),
        }) {
            Ok(r) => r,
            Err(f) => {
                let mut buf = vec![0i32; n];
                f(&mut buf)
            }
        }
    }
    #[cfg(not(feature = "std"))]
    {
        let mut buf = vec![0i32; n];
        f(&mut buf)
    }
}

/// Round-shift an array in place.
pub fn round_shift_array(arr: &mut [i32], bit: i32) {
    if bit == 0 {
        return;
    }
    if bit > 0 {
        let b = bit as u32;
        for v in arr.iter_mut() {
            *v = round_shift(*v, b);
        }
    } else {
        let b = (-bit) as u32;
        for v in arr.iter_mut() {
            *v <<= b;
        }
    }
}

// =============================================================================
// 4-point forward DCT-II
// Ported from svt_av1_fdct4_new in transforms.c
// =============================================================================

pub fn fdct4(input: &[TranLow], output: &mut [TranLow], cos_bit: i8) {
    let cospi = cospi_arr(cos_bit);
    let cos_bit = cos_bit as u32;

    // stage 1
    let bf0 = [
        input[0] + input[3],
        input[1] + input[2],
        -input[2] + input[1],
        -input[3] + input[0],
    ];

    // stage 2
    output[0] = half_btf(cospi[32], bf0[0], cospi[32], bf0[1], cos_bit);
    output[1] = half_btf(cospi[48], bf0[2], cospi[16], bf0[3], cos_bit);
    output[2] = half_btf(-cospi[32], bf0[1], cospi[32], bf0[0], cos_bit);
    output[3] = half_btf(cospi[48], bf0[3], -cospi[16], bf0[2], cos_bit);
}

// =============================================================================
// 8-point forward DCT-II
// Ported exactly from svt_av1_fdct8_new in transforms.c:776-846
// =============================================================================

pub fn fdct8(input: &[TranLow], output: &mut [TranLow], cos_bit: i8) {
    let cospi = cospi_arr(cos_bit);
    let cos_bit = cos_bit as u32;
    let mut step = [0i32; 8];

    // stage 1
    output[0] = input[0] + input[7];
    output[1] = input[1] + input[6];
    output[2] = input[2] + input[5];
    output[3] = input[3] + input[4];
    output[4] = -input[4] + input[3];
    output[5] = -input[5] + input[2];
    output[6] = -input[6] + input[1];
    output[7] = -input[7] + input[0];

    // stage 2
    let bf0 = &*output;
    step[0] = bf0[0] + bf0[3];
    step[1] = bf0[1] + bf0[2];
    step[2] = -bf0[2] + bf0[1];
    step[3] = -bf0[3] + bf0[0];
    step[4] = bf0[4];
    step[5] = half_btf(-cospi[32], bf0[5], cospi[32], bf0[6], cos_bit);
    step[6] = half_btf(cospi[32], bf0[6], cospi[32], bf0[5], cos_bit);
    step[7] = bf0[7];

    // stage 3
    output[0] = half_btf(cospi[32], step[0], cospi[32], step[1], cos_bit);
    output[1] = half_btf(-cospi[32], step[1], cospi[32], step[0], cos_bit);
    output[2] = half_btf(cospi[48], step[2], cospi[16], step[3], cos_bit);
    output[3] = half_btf(cospi[48], step[3], -cospi[16], step[2], cos_bit);
    output[4] = step[4] + step[5];
    output[5] = -step[5] + step[4];
    output[6] = -step[6] + step[7];
    output[7] = step[7] + step[6];

    // stage 4
    let bf0_4 = output[4];
    let bf0_5 = output[5];
    let bf0_6 = output[6];
    let bf0_7 = output[7];
    step[0] = output[0];
    step[1] = output[1];
    step[2] = output[2];
    step[3] = output[3];
    step[4] = half_btf(cospi[56], bf0_4, cospi[8], bf0_7, cos_bit);
    step[5] = half_btf(cospi[24], bf0_5, cospi[40], bf0_6, cos_bit);
    step[6] = half_btf(cospi[24], bf0_6, -cospi[40], bf0_5, cos_bit);
    step[7] = half_btf(cospi[56], bf0_7, -cospi[8], bf0_4, cos_bit);

    // stage 5 (output permutation)
    output[0] = step[0];
    output[1] = step[4];
    output[2] = step[2];
    output[3] = step[6];
    output[4] = step[1];
    output[5] = step[5];
    output[6] = step[3];
    output[7] = step[7];
}

// =============================================================================
// 16-point forward DCT-II
// Ported exactly from svt_av1_fdct16_new in transforms.c:848-1000
// =============================================================================

pub fn fdct16(input: &[TranLow], output: &mut [TranLow], cos_bit: i8) {
    let cospi = cospi_arr(cos_bit);
    let cos_bit = cos_bit as u32;
    let mut step = [0i32; 16];

    // stage 1
    for i in 0..8 {
        output[i] = input[i] + input[15 - i];
        output[15 - i] = -input[15 - i] + input[i];
    }

    // stage 2
    let _bf0 = output.as_ptr();
    let bf0 = |i: usize| -> i32 { output[i] };
    step[0] = bf0(0) + bf0(7);
    step[1] = bf0(1) + bf0(6);
    step[2] = bf0(2) + bf0(5);
    step[3] = bf0(3) + bf0(4);
    step[4] = -bf0(4) + bf0(3);
    step[5] = -bf0(5) + bf0(2);
    step[6] = -bf0(6) + bf0(1);
    step[7] = -bf0(7) + bf0(0);
    step[8] = bf0(8);
    step[9] = bf0(9);
    step[10] = half_btf(-cospi[32], bf0(10), cospi[32], bf0(13), cos_bit);
    step[11] = half_btf(-cospi[32], bf0(11), cospi[32], bf0(12), cos_bit);
    step[12] = half_btf(cospi[32], bf0(12), cospi[32], bf0(11), cos_bit);
    step[13] = half_btf(cospi[32], bf0(13), cospi[32], bf0(10), cos_bit);
    step[14] = bf0(14);
    step[15] = bf0(15);

    // stage 3
    let s = &step;
    output[0] = s[0] + s[3];
    output[1] = s[1] + s[2];
    output[2] = -s[2] + s[1];
    output[3] = -s[3] + s[0];
    output[4] = s[4];
    output[5] = half_btf(-cospi[32], s[5], cospi[32], s[6], cos_bit);
    output[6] = half_btf(cospi[32], s[6], cospi[32], s[5], cos_bit);
    output[7] = s[7];
    output[8] = s[8] + s[11];
    output[9] = s[9] + s[10];
    output[10] = -s[10] + s[9];
    output[11] = -s[11] + s[8];
    output[12] = -s[12] + s[15];
    output[13] = -s[13] + s[14];
    output[14] = s[14] + s[13];
    output[15] = s[15] + s[12];

    // stage 4
    let o = |i: usize| -> i32 { output[i] };
    step[0] = half_btf(cospi[32], o(0), cospi[32], o(1), cos_bit);
    step[1] = half_btf(-cospi[32], o(1), cospi[32], o(0), cos_bit);
    step[2] = half_btf(cospi[48], o(2), cospi[16], o(3), cos_bit);
    step[3] = half_btf(cospi[48], o(3), -cospi[16], o(2), cos_bit);
    step[4] = o(4) + o(5);
    step[5] = -o(5) + o(4);
    step[6] = -o(6) + o(7);
    step[7] = o(7) + o(6);
    step[8] = o(8);
    step[9] = half_btf(-cospi[16], o(9), cospi[48], o(14), cos_bit);
    step[10] = half_btf(-cospi[48], o(10), -cospi[16], o(13), cos_bit);
    step[11] = o(11);
    step[12] = o(12);
    step[13] = half_btf(cospi[48], o(13), -cospi[16], o(10), cos_bit);
    step[14] = half_btf(cospi[16], o(14), cospi[48], o(9), cos_bit);
    step[15] = o(15);

    // stage 5
    let s = &step;
    output[0] = s[0];
    output[1] = s[1];
    output[2] = s[2];
    output[3] = s[3];
    output[4] = half_btf(cospi[56], s[4], cospi[8], s[7], cos_bit);
    output[5] = half_btf(cospi[24], s[5], cospi[40], s[6], cos_bit);
    output[6] = half_btf(cospi[24], s[6], -cospi[40], s[5], cos_bit);
    output[7] = half_btf(cospi[56], s[7], -cospi[8], s[4], cos_bit);
    output[8] = s[8] + s[9];
    output[9] = -s[9] + s[8];
    output[10] = -s[10] + s[11];
    output[11] = s[11] + s[10];
    output[12] = s[12] + s[13];
    output[13] = -s[13] + s[12];
    output[14] = -s[14] + s[15];
    output[15] = s[15] + s[14];

    // stage 6
    let o = |i: usize| -> i32 { output[i] };
    step[0] = o(0);
    step[1] = o(1);
    step[2] = o(2);
    step[3] = o(3);
    step[4] = o(4);
    step[5] = o(5);
    step[6] = o(6);
    step[7] = o(7);
    step[8] = half_btf(cospi[60], o(8), cospi[4], o(15), cos_bit);
    step[9] = half_btf(cospi[28], o(9), cospi[36], o(14), cos_bit);
    step[10] = half_btf(cospi[44], o(10), cospi[20], o(13), cos_bit);
    step[11] = half_btf(cospi[12], o(11), cospi[52], o(12), cos_bit);
    step[12] = half_btf(cospi[12], o(12), -cospi[52], o(11), cos_bit);
    step[13] = half_btf(cospi[44], o(13), -cospi[20], o(10), cos_bit);
    step[14] = half_btf(cospi[28], o(14), -cospi[36], o(9), cos_bit);
    step[15] = half_btf(cospi[60], o(15), -cospi[4], o(8), cos_bit);

    // stage 7 (output permutation)
    output[0] = step[0];
    output[1] = step[8];
    output[2] = step[4];
    output[3] = step[12];
    output[4] = step[2];
    output[5] = step[10];
    output[6] = step[6];
    output[7] = step[14];
    output[8] = step[1];
    output[9] = step[9];
    output[10] = step[5];
    output[11] = step[13];
    output[12] = step[3];
    output[13] = step[11];
    output[14] = step[7];
    output[15] = step[15];
}

// =============================================================================
// 32-point forward DCT-II
// Ported exactly from svt_av1_fdct32_new in transforms.c:1002-1340
// =============================================================================

pub fn fdct32(input: &[TranLow], output: &mut [TranLow], cos_bit: i8) {
    let cospi = cospi_arr(cos_bit);
    let cos_bit = cos_bit as u32;
    let mut step = [0i32; 32];

    // stage 1
    output[0] = input[0] + input[31];
    output[1] = input[1] + input[30];
    output[2] = input[2] + input[29];
    output[3] = input[3] + input[28];
    output[4] = input[4] + input[27];
    output[5] = input[5] + input[26];
    output[6] = input[6] + input[25];
    output[7] = input[7] + input[24];
    output[8] = input[8] + input[23];
    output[9] = input[9] + input[22];
    output[10] = input[10] + input[21];
    output[11] = input[11] + input[20];
    output[12] = input[12] + input[19];
    output[13] = input[13] + input[18];
    output[14] = input[14] + input[17];
    output[15] = input[15] + input[16];
    output[16] = -input[16] + input[15];
    output[17] = -input[17] + input[14];
    output[18] = -input[18] + input[13];
    output[19] = -input[19] + input[12];
    output[20] = -input[20] + input[11];
    output[21] = -input[21] + input[10];
    output[22] = -input[22] + input[9];
    output[23] = -input[23] + input[8];
    output[24] = -input[24] + input[7];
    output[25] = -input[25] + input[6];
    output[26] = -input[26] + input[5];
    output[27] = -input[27] + input[4];
    output[28] = -input[28] + input[3];
    output[29] = -input[29] + input[2];
    output[30] = -input[30] + input[1];
    output[31] = -input[31] + input[0];

    // stage 2
    let o = |i: usize| -> i32 { output[i] };
    step[0] = o(0) + o(15);
    step[1] = o(1) + o(14);
    step[2] = o(2) + o(13);
    step[3] = o(3) + o(12);
    step[4] = o(4) + o(11);
    step[5] = o(5) + o(10);
    step[6] = o(6) + o(9);
    step[7] = o(7) + o(8);
    step[8] = -o(8) + o(7);
    step[9] = -o(9) + o(6);
    step[10] = -o(10) + o(5);
    step[11] = -o(11) + o(4);
    step[12] = -o(12) + o(3);
    step[13] = -o(13) + o(2);
    step[14] = -o(14) + o(1);
    step[15] = -o(15) + o(0);
    step[16] = o(16);
    step[17] = o(17);
    step[18] = o(18);
    step[19] = o(19);
    step[20] = half_btf(-cospi[32], o(20), cospi[32], o(27), cos_bit);
    step[21] = half_btf(-cospi[32], o(21), cospi[32], o(26), cos_bit);
    step[22] = half_btf(-cospi[32], o(22), cospi[32], o(25), cos_bit);
    step[23] = half_btf(-cospi[32], o(23), cospi[32], o(24), cos_bit);
    step[24] = half_btf(cospi[32], o(24), cospi[32], o(23), cos_bit);
    step[25] = half_btf(cospi[32], o(25), cospi[32], o(22), cos_bit);
    step[26] = half_btf(cospi[32], o(26), cospi[32], o(21), cos_bit);
    step[27] = half_btf(cospi[32], o(27), cospi[32], o(20), cos_bit);
    step[28] = o(28);
    step[29] = o(29);
    step[30] = o(30);
    step[31] = o(31);

    // stage 3
    let s = |i: usize| -> i32 { step[i] };
    output[0] = s(0) + s(7);
    output[1] = s(1) + s(6);
    output[2] = s(2) + s(5);
    output[3] = s(3) + s(4);
    output[4] = -s(4) + s(3);
    output[5] = -s(5) + s(2);
    output[6] = -s(6) + s(1);
    output[7] = -s(7) + s(0);
    output[8] = s(8);
    output[9] = s(9);
    output[10] = half_btf(-cospi[32], s(10), cospi[32], s(13), cos_bit);
    output[11] = half_btf(-cospi[32], s(11), cospi[32], s(12), cos_bit);
    output[12] = half_btf(cospi[32], s(12), cospi[32], s(11), cos_bit);
    output[13] = half_btf(cospi[32], s(13), cospi[32], s(10), cos_bit);
    output[14] = s(14);
    output[15] = s(15);
    output[16] = s(16) + s(23);
    output[17] = s(17) + s(22);
    output[18] = s(18) + s(21);
    output[19] = s(19) + s(20);
    output[20] = -s(20) + s(19);
    output[21] = -s(21) + s(18);
    output[22] = -s(22) + s(17);
    output[23] = -s(23) + s(16);
    output[24] = -s(24) + s(31);
    output[25] = -s(25) + s(30);
    output[26] = -s(26) + s(29);
    output[27] = -s(27) + s(28);
    output[28] = s(28) + s(27);
    output[29] = s(29) + s(26);
    output[30] = s(30) + s(25);
    output[31] = s(31) + s(24);

    // stage 4
    let o = |i: usize| -> i32 { output[i] };
    step[0] = o(0) + o(3);
    step[1] = o(1) + o(2);
    step[2] = -o(2) + o(1);
    step[3] = -o(3) + o(0);
    step[4] = o(4);
    step[5] = half_btf(-cospi[32], o(5), cospi[32], o(6), cos_bit);
    step[6] = half_btf(cospi[32], o(6), cospi[32], o(5), cos_bit);
    step[7] = o(7);
    step[8] = o(8) + o(11);
    step[9] = o(9) + o(10);
    step[10] = -o(10) + o(9);
    step[11] = -o(11) + o(8);
    step[12] = -o(12) + o(15);
    step[13] = -o(13) + o(14);
    step[14] = o(14) + o(13);
    step[15] = o(15) + o(12);
    step[16] = o(16);
    step[17] = o(17);
    step[18] = half_btf(-cospi[16], o(18), cospi[48], o(29), cos_bit);
    step[19] = half_btf(-cospi[16], o(19), cospi[48], o(28), cos_bit);
    step[20] = half_btf(-cospi[48], o(20), -cospi[16], o(27), cos_bit);
    step[21] = half_btf(-cospi[48], o(21), -cospi[16], o(26), cos_bit);
    step[22] = o(22);
    step[23] = o(23);
    step[24] = o(24);
    step[25] = o(25);
    step[26] = half_btf(cospi[48], o(26), -cospi[16], o(21), cos_bit);
    step[27] = half_btf(cospi[48], o(27), -cospi[16], o(20), cos_bit);
    step[28] = half_btf(cospi[16], o(28), cospi[48], o(19), cos_bit);
    step[29] = half_btf(cospi[16], o(29), cospi[48], o(18), cos_bit);
    step[30] = o(30);
    step[31] = o(31);

    // stage 5
    let s = |i: usize| -> i32 { step[i] };
    output[0] = half_btf(cospi[32], s(0), cospi[32], s(1), cos_bit);
    output[1] = half_btf(-cospi[32], s(1), cospi[32], s(0), cos_bit);
    output[2] = half_btf(cospi[48], s(2), cospi[16], s(3), cos_bit);
    output[3] = half_btf(cospi[48], s(3), -cospi[16], s(2), cos_bit);
    output[4] = s(4) + s(5);
    output[5] = -s(5) + s(4);
    output[6] = -s(6) + s(7);
    output[7] = s(7) + s(6);
    output[8] = s(8);
    output[9] = half_btf(-cospi[16], s(9), cospi[48], s(14), cos_bit);
    output[10] = half_btf(-cospi[48], s(10), -cospi[16], s(13), cos_bit);
    output[11] = s(11);
    output[12] = s(12);
    output[13] = half_btf(cospi[48], s(13), -cospi[16], s(10), cos_bit);
    output[14] = half_btf(cospi[16], s(14), cospi[48], s(9), cos_bit);
    output[15] = s(15);
    output[16] = s(16) + s(19);
    output[17] = s(17) + s(18);
    output[18] = -s(18) + s(17);
    output[19] = -s(19) + s(16);
    output[20] = -s(20) + s(23);
    output[21] = -s(21) + s(22);
    output[22] = s(22) + s(21);
    output[23] = s(23) + s(20);
    output[24] = s(24) + s(27);
    output[25] = s(25) + s(26);
    output[26] = -s(26) + s(25);
    output[27] = -s(27) + s(24);
    output[28] = -s(28) + s(31);
    output[29] = -s(29) + s(30);
    output[30] = s(30) + s(29);
    output[31] = s(31) + s(28);

    // stage 6
    let o = |i: usize| -> i32 { output[i] };
    step[0] = o(0);
    step[1] = o(1);
    step[2] = o(2);
    step[3] = o(3);
    step[4] = half_btf(cospi[56], o(4), cospi[8], o(7), cos_bit);
    step[5] = half_btf(cospi[24], o(5), cospi[40], o(6), cos_bit);
    step[6] = half_btf(cospi[24], o(6), -cospi[40], o(5), cos_bit);
    step[7] = half_btf(cospi[56], o(7), -cospi[8], o(4), cos_bit);
    step[8] = o(8) + o(9);
    step[9] = -o(9) + o(8);
    step[10] = -o(10) + o(11);
    step[11] = o(11) + o(10);
    step[12] = o(12) + o(13);
    step[13] = -o(13) + o(12);
    step[14] = -o(14) + o(15);
    step[15] = o(15) + o(14);
    step[16] = o(16);
    step[17] = half_btf(-cospi[8], o(17), cospi[56], o(30), cos_bit);
    step[18] = half_btf(-cospi[56], o(18), -cospi[8], o(29), cos_bit);
    step[19] = o(19);
    step[20] = o(20);
    step[21] = half_btf(-cospi[40], o(21), cospi[24], o(26), cos_bit);
    step[22] = half_btf(-cospi[24], o(22), -cospi[40], o(25), cos_bit);
    step[23] = o(23);
    step[24] = o(24);
    step[25] = half_btf(cospi[24], o(25), -cospi[40], o(22), cos_bit);
    step[26] = half_btf(cospi[40], o(26), cospi[24], o(21), cos_bit);
    step[27] = o(27);
    step[28] = o(28);
    step[29] = half_btf(cospi[56], o(29), -cospi[8], o(18), cos_bit);
    step[30] = half_btf(cospi[8], o(30), cospi[56], o(17), cos_bit);
    step[31] = o(31);

    // stage 7
    let s = |i: usize| -> i32 { step[i] };
    output[0] = s(0);
    output[1] = s(1);
    output[2] = s(2);
    output[3] = s(3);
    output[4] = s(4);
    output[5] = s(5);
    output[6] = s(6);
    output[7] = s(7);
    output[8] = half_btf(cospi[60], s(8), cospi[4], s(15), cos_bit);
    output[9] = half_btf(cospi[28], s(9), cospi[36], s(14), cos_bit);
    output[10] = half_btf(cospi[44], s(10), cospi[20], s(13), cos_bit);
    output[11] = half_btf(cospi[12], s(11), cospi[52], s(12), cos_bit);
    output[12] = half_btf(cospi[12], s(12), -cospi[52], s(11), cos_bit);
    output[13] = half_btf(cospi[44], s(13), -cospi[20], s(10), cos_bit);
    output[14] = half_btf(cospi[28], s(14), -cospi[36], s(9), cos_bit);
    output[15] = half_btf(cospi[60], s(15), -cospi[4], s(8), cos_bit);
    output[16] = s(16) + s(17);
    output[17] = -s(17) + s(16);
    output[18] = -s(18) + s(19);
    output[19] = s(19) + s(18);
    output[20] = s(20) + s(21);
    output[21] = -s(21) + s(20);
    output[22] = -s(22) + s(23);
    output[23] = s(23) + s(22);
    output[24] = s(24) + s(25);
    output[25] = -s(25) + s(24);
    output[26] = -s(26) + s(27);
    output[27] = s(27) + s(26);
    output[28] = s(28) + s(29);
    output[29] = -s(29) + s(28);
    output[30] = -s(30) + s(31);
    output[31] = s(31) + s(30);

    // stage 8
    let o = |i: usize| -> i32 { output[i] };
    step[0] = o(0);
    step[1] = o(1);
    step[2] = o(2);
    step[3] = o(3);
    step[4] = o(4);
    step[5] = o(5);
    step[6] = o(6);
    step[7] = o(7);
    step[8] = o(8);
    step[9] = o(9);
    step[10] = o(10);
    step[11] = o(11);
    step[12] = o(12);
    step[13] = o(13);
    step[14] = o(14);
    step[15] = o(15);
    step[16] = half_btf(cospi[62], o(16), cospi[2], o(31), cos_bit);
    step[17] = half_btf(cospi[30], o(17), cospi[34], o(30), cos_bit);
    step[18] = half_btf(cospi[46], o(18), cospi[18], o(29), cos_bit);
    step[19] = half_btf(cospi[14], o(19), cospi[50], o(28), cos_bit);
    step[20] = half_btf(cospi[54], o(20), cospi[10], o(27), cos_bit);
    step[21] = half_btf(cospi[22], o(21), cospi[42], o(26), cos_bit);
    step[22] = half_btf(cospi[38], o(22), cospi[26], o(25), cos_bit);
    step[23] = half_btf(cospi[6], o(23), cospi[58], o(24), cos_bit);
    step[24] = half_btf(cospi[6], o(24), -cospi[58], o(23), cos_bit);
    step[25] = half_btf(cospi[38], o(25), -cospi[26], o(22), cos_bit);
    step[26] = half_btf(cospi[22], o(26), -cospi[42], o(21), cos_bit);
    step[27] = half_btf(cospi[54], o(27), -cospi[10], o(20), cos_bit);
    step[28] = half_btf(cospi[14], o(28), -cospi[50], o(19), cos_bit);
    step[29] = half_btf(cospi[46], o(29), -cospi[18], o(18), cos_bit);
    step[30] = half_btf(cospi[30], o(30), -cospi[34], o(17), cos_bit);
    step[31] = half_btf(cospi[62], o(31), -cospi[2], o(16), cos_bit);

    // stage 9 (output permutation)
    output[0] = step[0];
    output[1] = step[16];
    output[2] = step[8];
    output[3] = step[24];
    output[4] = step[4];
    output[5] = step[20];
    output[6] = step[12];
    output[7] = step[28];
    output[8] = step[2];
    output[9] = step[18];
    output[10] = step[10];
    output[11] = step[26];
    output[12] = step[6];
    output[13] = step[22];
    output[14] = step[14];
    output[15] = step[30];
    output[16] = step[1];
    output[17] = step[17];
    output[18] = step[9];
    output[19] = step[25];
    output[20] = step[5];
    output[21] = step[21];
    output[22] = step[13];
    output[23] = step[29];
    output[24] = step[3];
    output[25] = step[19];
    output[26] = step[11];
    output[27] = step[27];
    output[28] = step[7];
    output[29] = step[23];
    output[30] = step[15];
    output[31] = step[31];
}

// =============================================================================
// 64-point forward DCT-II
// Ported exactly from svt_av1_fdct64_new in transforms.c:1342-2106
// =============================================================================

pub fn fidentity64(input: &[TranLow], output: &mut [TranLow], _cos_bit: i8) {
    for i in 0..64 {
        output[i] = round_shift_i64(input[i] as i64 * 4 * NEW_SQRT2 as i64, NEW_SQRT2_BITS);
    }
}

// =============================================================================
// 32-point identity transform
// =============================================================================

pub fn fidentity32(input: &[TranLow], output: &mut [TranLow], _cos_bit: i8) {
    for i in 0..32 {
        output[i] = input[i] * 4;
    }
}

// =============================================================================
// 4-point ADST
// Ported from svt_av1_fadst4_new in transforms.c
// =============================================================================

/// Forward 4-point ADST — exact port of svt_av1_fadst4_new from transforms.c:2108.
/// Uses i32 arithmetic matching the C code exactly.
pub fn fadst4(input: &[TranLow], output: &mut [TranLow], cos_bit: i8) {
    let sinpi = sinpi_arr(cos_bit);
    let bit = cos_bit as u32;

    let (x0, x1, x2, x3) = (input[0], input[1], input[2], input[3]);
    if (x0 | x1 | x2 | x3) == 0 {
        output[0] = 0;
        output[1] = 0;
        output[2] = 0;
        output[3] = 0;
        return;
    }

    // stage 1 (i64 intermediates; C accumulates in int32 but promotes to
    // int64 at round_shift — identical for all conformant input ranges)
    let s0 = sinpi[1] as i64 * x0 as i64;
    let s1 = sinpi[4] as i64 * x0 as i64;
    let s2 = sinpi[2] as i64 * x1 as i64;
    let s3 = sinpi[1] as i64 * x1 as i64;
    let s4 = sinpi[3] as i64 * x2 as i64;
    let s5 = sinpi[4] as i64 * x3 as i64;
    let s6 = sinpi[2] as i64 * x3 as i64;
    let mut s7 = (x0 + x1) as i64;

    // stage 2
    s7 -= x3 as i64;

    // stage 3
    let mut x0 = s0 + s2;
    let x1 = sinpi[3] as i64 * s7;
    let mut x2 = s1 - s3;
    let x3 = s4;

    // stage 4
    x0 += s5;
    x2 += s6;

    // stage 5
    let s0 = x0 + x3;
    let s1 = x1;
    let s2 = x2 - x3;
    let mut s3 = x2 - x0;

    // stage 6
    s3 += x3;

    output[0] = round_shift_i64(s0, bit);
    output[1] = round_shift_i64(s1, bit);
    output[2] = round_shift_i64(s2, bit);
    output[3] = round_shift_i64(s3, bit);
}

// =============================================================================
// 4-point identity transform
// =============================================================================

pub fn fidentity4(input: &[TranLow], output: &mut [TranLow], _cos_bit: i8) {
    let new_sqrt2 = NEW_SQRT2;
    for i in 0..4 {
        output[i] = round_shift_i64(input[i] as i64 * new_sqrt2 as i64, NEW_SQRT2_BITS);
    }
}

// =============================================================================
// 8-point ADST
// Ported from svt_av1_fadst8_new in transforms.c
// =============================================================================

pub fn fadst8(input: &[TranLow], output: &mut [TranLow], cos_bit: i8) {
    let cospi = cospi_arr(cos_bit);
    let cos_bit = cos_bit as u32;
    let mut step = [0i32; 8];

    // stage 1
    output[0] = input[0];
    output[1] = -input[7];
    output[2] = -input[3];
    output[3] = input[4];
    output[4] = -input[1];
    output[5] = input[6];
    output[6] = input[2];
    output[7] = -input[5];

    // stage 2
    let bf0 = |i: usize| -> i32 { output[i] };
    step[0] = bf0(0);
    step[1] = bf0(1);
    step[2] = half_btf(cospi[32], bf0(2), cospi[32], bf0(3), cos_bit);
    step[3] = half_btf(cospi[32], bf0(2), -cospi[32], bf0(3), cos_bit);
    step[4] = bf0(4);
    step[5] = bf0(5);
    step[6] = half_btf(cospi[32], bf0(6), cospi[32], bf0(7), cos_bit);
    step[7] = half_btf(cospi[32], bf0(6), -cospi[32], bf0(7), cos_bit);

    // stage 3
    let s = &step;
    output[0] = s[0] + s[2];
    output[1] = s[1] + s[3];
    output[2] = s[0] - s[2];
    output[3] = s[1] - s[3];
    output[4] = s[4] + s[6];
    output[5] = s[5] + s[7];
    output[6] = s[4] - s[6];
    output[7] = s[5] - s[7];

    // stage 4
    let bf0 = |i: usize| -> i32 { output[i] };
    step[0] = bf0(0);
    step[1] = bf0(1);
    step[2] = bf0(2);
    step[3] = bf0(3);
    step[4] = half_btf(cospi[16], bf0(4), cospi[48], bf0(5), cos_bit);
    step[5] = half_btf(cospi[48], bf0(4), -cospi[16], bf0(5), cos_bit);
    step[6] = half_btf(-cospi[48], bf0(6), cospi[16], bf0(7), cos_bit);
    step[7] = half_btf(cospi[16], bf0(6), cospi[48], bf0(7), cos_bit);

    // stage 5
    let s = &step;
    output[0] = s[0] + s[4];
    output[1] = s[1] + s[5];
    output[2] = s[2] + s[6];
    output[3] = s[3] + s[7];
    output[4] = s[0] - s[4];
    output[5] = s[1] - s[5];
    output[6] = s[2] - s[6];
    output[7] = s[3] - s[7];

    // stage 6
    let bf0 = |i: usize| -> i32 { output[i] };
    step[0] = half_btf(cospi[4], bf0(0), cospi[60], bf0(1), cos_bit);
    step[1] = half_btf(cospi[60], bf0(0), -cospi[4], bf0(1), cos_bit);
    step[2] = half_btf(cospi[20], bf0(2), cospi[44], bf0(3), cos_bit);
    step[3] = half_btf(cospi[44], bf0(2), -cospi[20], bf0(3), cos_bit);
    step[4] = half_btf(cospi[36], bf0(4), cospi[28], bf0(5), cos_bit);
    step[5] = half_btf(cospi[28], bf0(4), -cospi[36], bf0(5), cos_bit);
    step[6] = half_btf(cospi[52], bf0(6), cospi[12], bf0(7), cos_bit);
    step[7] = half_btf(cospi[12], bf0(6), -cospi[52], bf0(7), cos_bit);

    // stage 7 (output permutation — exact match to C svt_av1_fadst8_new)
    output[0] = step[1];
    output[1] = step[6];
    output[2] = step[3];
    output[3] = step[4];
    output[4] = step[5];
    output[5] = step[2];
    output[6] = step[7];
    output[7] = step[0];
}

// =============================================================================
// 16-point forward ADST
// Ported exactly from svt_av1_fadst16_new in transforms.c:2294-2486
// =============================================================================

pub fn fadst16(input: &[TranLow], output: &mut [TranLow], cos_bit: i8) {
    let cospi = cospi_arr(cos_bit);
    let cos_bit = cos_bit as u32;
    let mut step = [0i32; 16];

    // stage 1: input permutation with sign flips
    output[0] = input[0];
    output[1] = -input[15];
    output[2] = -input[7];
    output[3] = input[8];
    output[4] = -input[3];
    output[5] = input[12];
    output[6] = input[4];
    output[7] = -input[11];
    output[8] = -input[1];
    output[9] = input[14];
    output[10] = input[6];
    output[11] = -input[9];
    output[12] = input[2];
    output[13] = -input[13];
    output[14] = -input[5];
    output[15] = input[10];

    // stage 2
    let o = |i: usize| -> i32 { output[i] };
    step[0] = o(0);
    step[1] = o(1);
    step[2] = half_btf(cospi[32], o(2), cospi[32], o(3), cos_bit);
    step[3] = half_btf(cospi[32], o(2), -cospi[32], o(3), cos_bit);
    step[4] = o(4);
    step[5] = o(5);
    step[6] = half_btf(cospi[32], o(6), cospi[32], o(7), cos_bit);
    step[7] = half_btf(cospi[32], o(6), -cospi[32], o(7), cos_bit);
    step[8] = o(8);
    step[9] = o(9);
    step[10] = half_btf(cospi[32], o(10), cospi[32], o(11), cos_bit);
    step[11] = half_btf(cospi[32], o(10), -cospi[32], o(11), cos_bit);
    step[12] = o(12);
    step[13] = o(13);
    step[14] = half_btf(cospi[32], o(14), cospi[32], o(15), cos_bit);
    step[15] = half_btf(cospi[32], o(14), -cospi[32], o(15), cos_bit);

    // stage 3
    let s = |i: usize| -> i32 { step[i] };
    output[0] = s(0) + s(2);
    output[1] = s(1) + s(3);
    output[2] = s(0) - s(2);
    output[3] = s(1) - s(3);
    output[4] = s(4) + s(6);
    output[5] = s(5) + s(7);
    output[6] = s(4) - s(6);
    output[7] = s(5) - s(7);
    output[8] = s(8) + s(10);
    output[9] = s(9) + s(11);
    output[10] = s(8) - s(10);
    output[11] = s(9) - s(11);
    output[12] = s(12) + s(14);
    output[13] = s(13) + s(15);
    output[14] = s(12) - s(14);
    output[15] = s(13) - s(15);

    // stage 4
    let o = |i: usize| -> i32 { output[i] };
    step[0] = o(0);
    step[1] = o(1);
    step[2] = o(2);
    step[3] = o(3);
    step[4] = half_btf(cospi[16], o(4), cospi[48], o(5), cos_bit);
    step[5] = half_btf(cospi[48], o(4), -cospi[16], o(5), cos_bit);
    step[6] = half_btf(-cospi[48], o(6), cospi[16], o(7), cos_bit);
    step[7] = half_btf(cospi[16], o(6), cospi[48], o(7), cos_bit);
    step[8] = o(8);
    step[9] = o(9);
    step[10] = o(10);
    step[11] = o(11);
    step[12] = half_btf(cospi[16], o(12), cospi[48], o(13), cos_bit);
    step[13] = half_btf(cospi[48], o(12), -cospi[16], o(13), cos_bit);
    step[14] = half_btf(-cospi[48], o(14), cospi[16], o(15), cos_bit);
    step[15] = half_btf(cospi[16], o(14), cospi[48], o(15), cos_bit);

    // stage 5
    let s = |i: usize| -> i32 { step[i] };
    output[0] = s(0) + s(4);
    output[1] = s(1) + s(5);
    output[2] = s(2) + s(6);
    output[3] = s(3) + s(7);
    output[4] = s(0) - s(4);
    output[5] = s(1) - s(5);
    output[6] = s(2) - s(6);
    output[7] = s(3) - s(7);
    output[8] = s(8) + s(12);
    output[9] = s(9) + s(13);
    output[10] = s(10) + s(14);
    output[11] = s(11) + s(15);
    output[12] = s(8) - s(12);
    output[13] = s(9) - s(13);
    output[14] = s(10) - s(14);
    output[15] = s(11) - s(15);

    // stage 6
    let o = |i: usize| -> i32 { output[i] };
    step[0] = o(0);
    step[1] = o(1);
    step[2] = o(2);
    step[3] = o(3);
    step[4] = o(4);
    step[5] = o(5);
    step[6] = o(6);
    step[7] = o(7);
    step[8] = half_btf(cospi[8], o(8), cospi[56], o(9), cos_bit);
    step[9] = half_btf(cospi[56], o(8), -cospi[8], o(9), cos_bit);
    step[10] = half_btf(cospi[40], o(10), cospi[24], o(11), cos_bit);
    step[11] = half_btf(cospi[24], o(10), -cospi[40], o(11), cos_bit);
    step[12] = half_btf(-cospi[56], o(12), cospi[8], o(13), cos_bit);
    step[13] = half_btf(cospi[8], o(12), cospi[56], o(13), cos_bit);
    step[14] = half_btf(-cospi[24], o(14), cospi[40], o(15), cos_bit);
    step[15] = half_btf(cospi[40], o(14), cospi[24], o(15), cos_bit);

    // stage 7
    let s = |i: usize| -> i32 { step[i] };
    output[0] = s(0) + s(8);
    output[1] = s(1) + s(9);
    output[2] = s(2) + s(10);
    output[3] = s(3) + s(11);
    output[4] = s(4) + s(12);
    output[5] = s(5) + s(13);
    output[6] = s(6) + s(14);
    output[7] = s(7) + s(15);
    output[8] = s(0) - s(8);
    output[9] = s(1) - s(9);
    output[10] = s(2) - s(10);
    output[11] = s(3) - s(11);
    output[12] = s(4) - s(12);
    output[13] = s(5) - s(13);
    output[14] = s(6) - s(14);
    output[15] = s(7) - s(15);

    // stage 8
    let o = |i: usize| -> i32 { output[i] };
    step[0] = half_btf(cospi[2], o(0), cospi[62], o(1), cos_bit);
    step[1] = half_btf(cospi[62], o(0), -cospi[2], o(1), cos_bit);
    step[2] = half_btf(cospi[10], o(2), cospi[54], o(3), cos_bit);
    step[3] = half_btf(cospi[54], o(2), -cospi[10], o(3), cos_bit);
    step[4] = half_btf(cospi[18], o(4), cospi[46], o(5), cos_bit);
    step[5] = half_btf(cospi[46], o(4), -cospi[18], o(5), cos_bit);
    step[6] = half_btf(cospi[26], o(6), cospi[38], o(7), cos_bit);
    step[7] = half_btf(cospi[38], o(6), -cospi[26], o(7), cos_bit);
    step[8] = half_btf(cospi[34], o(8), cospi[30], o(9), cos_bit);
    step[9] = half_btf(cospi[30], o(8), -cospi[34], o(9), cos_bit);
    step[10] = half_btf(cospi[42], o(10), cospi[22], o(11), cos_bit);
    step[11] = half_btf(cospi[22], o(10), -cospi[42], o(11), cos_bit);
    step[12] = half_btf(cospi[50], o(12), cospi[14], o(13), cos_bit);
    step[13] = half_btf(cospi[14], o(12), -cospi[50], o(13), cos_bit);
    step[14] = half_btf(cospi[58], o(14), cospi[6], o(15), cos_bit);
    step[15] = half_btf(cospi[6], o(14), -cospi[58], o(15), cos_bit);

    // stage 9: output permutation
    output[0] = step[1];
    output[1] = step[14];
    output[2] = step[3];
    output[3] = step[12];
    output[4] = step[5];
    output[5] = step[10];
    output[6] = step[7];
    output[7] = step[8];
    output[8] = step[9];
    output[9] = step[6];
    output[10] = step[11];
    output[11] = step[4];
    output[12] = step[13];
    output[13] = step[2];
    output[14] = step[15];
    output[15] = step[0];
}

// =============================================================================
// 32-point ADST
// =============================================================================

/// Port of C `av1_fadst32_new` (transforms.c:2488).
///
/// The last 1-D forward kernel in `transforms.c` without a Rust counterpart.
/// It is `static` in C and reached only through `svt_aom_fwd_txfm_type_to_func`
/// (:2959) — and through the `_N2` (:6118) and `_N4` (:7715) type tables, which
/// both route `TXFM_TYPE_ADST32` at the FULL kernel rather than a pruned twin.
///
/// Reachability, measured rather than assumed: `svt_aom_transform_config`
/// selects it whenever `av1_txfm_type_ls[3][TX_TYPE_1D_ADST]` is picked, i.e.
/// for any ADST/FLIPADST 1-D type at a 32-sample dimension. AV1's ext-tx sets
/// never offer such a type at a 32-dimension block (`tx_size_square_up` of
/// every size with a 32 side is `TX_32X32`, whose set is DCTONLY or
/// DCT_IDTX), so a conformant encode cannot reach it — but the C dispatch
/// table can, the exported `svt_av1_transform_two_d_32x32_c` reaches it with
/// one argument, and this port previously returned `false` (refusing the
/// transform) where C computes a result. Kept per the port's "dead-looking C
/// stays translated" rule.
///
/// C `(void)stage_range`s its argument: unlike [`crate::inv_txfm::iadst32`]
/// there is no per-stage clamp in the forward direction.
pub fn fadst32(input: &[TranLow], output: &mut [TranLow], cos_bit: i8) {
    let cospi = cospi_arr(cos_bit);
    let cos_bit = cos_bit as u32;
    let mut step = [0i32; 32];
    let inp: &[TranLow; 32] = input[..32].try_into().expect("fadst32 input is 32 samples");
    let out: &mut [TranLow; 32] = (&mut output[..32])
        .try_into()
        .expect("fadst32 output is 32 samples");

    // stage 1
    out[0] = inp[31];
    out[1] = inp[0];
    out[2] = inp[29];
    out[3] = inp[2];
    out[4] = inp[27];
    out[5] = inp[4];
    out[6] = inp[25];
    out[7] = inp[6];
    out[8] = inp[23];
    out[9] = inp[8];
    out[10] = inp[21];
    out[11] = inp[10];
    out[12] = inp[19];
    out[13] = inp[12];
    out[14] = inp[17];
    out[15] = inp[14];
    out[16] = inp[15];
    out[17] = inp[16];
    out[18] = inp[13];
    out[19] = inp[18];
    out[20] = inp[11];
    out[21] = inp[20];
    out[22] = inp[9];
    out[23] = inp[22];
    out[24] = inp[7];
    out[25] = inp[24];
    out[26] = inp[5];
    out[27] = inp[26];
    out[28] = inp[3];
    out[29] = inp[28];
    out[30] = inp[1];
    out[31] = inp[30];

    // stage 2
    step[0] = half_btf(cospi[1], out[0], cospi[63], out[1], cos_bit);
    step[1] = half_btf(-cospi[1], out[1], cospi[63], out[0], cos_bit);
    step[2] = half_btf(cospi[5], out[2], cospi[59], out[3], cos_bit);
    step[3] = half_btf(-cospi[5], out[3], cospi[59], out[2], cos_bit);
    step[4] = half_btf(cospi[9], out[4], cospi[55], out[5], cos_bit);
    step[5] = half_btf(-cospi[9], out[5], cospi[55], out[4], cos_bit);
    step[6] = half_btf(cospi[13], out[6], cospi[51], out[7], cos_bit);
    step[7] = half_btf(-cospi[13], out[7], cospi[51], out[6], cos_bit);
    step[8] = half_btf(cospi[17], out[8], cospi[47], out[9], cos_bit);
    step[9] = half_btf(-cospi[17], out[9], cospi[47], out[8], cos_bit);
    step[10] = half_btf(cospi[21], out[10], cospi[43], out[11], cos_bit);
    step[11] = half_btf(-cospi[21], out[11], cospi[43], out[10], cos_bit);
    step[12] = half_btf(cospi[25], out[12], cospi[39], out[13], cos_bit);
    step[13] = half_btf(-cospi[25], out[13], cospi[39], out[12], cos_bit);
    step[14] = half_btf(cospi[29], out[14], cospi[35], out[15], cos_bit);
    step[15] = half_btf(-cospi[29], out[15], cospi[35], out[14], cos_bit);
    step[16] = half_btf(cospi[33], out[16], cospi[31], out[17], cos_bit);
    step[17] = half_btf(-cospi[33], out[17], cospi[31], out[16], cos_bit);
    step[18] = half_btf(cospi[37], out[18], cospi[27], out[19], cos_bit);
    step[19] = half_btf(-cospi[37], out[19], cospi[27], out[18], cos_bit);
    step[20] = half_btf(cospi[41], out[20], cospi[23], out[21], cos_bit);
    step[21] = half_btf(-cospi[41], out[21], cospi[23], out[20], cos_bit);
    step[22] = half_btf(cospi[45], out[22], cospi[19], out[23], cos_bit);
    step[23] = half_btf(-cospi[45], out[23], cospi[19], out[22], cos_bit);
    step[24] = half_btf(cospi[49], out[24], cospi[15], out[25], cos_bit);
    step[25] = half_btf(-cospi[49], out[25], cospi[15], out[24], cos_bit);
    step[26] = half_btf(cospi[53], out[26], cospi[11], out[27], cos_bit);
    step[27] = half_btf(-cospi[53], out[27], cospi[11], out[26], cos_bit);
    step[28] = half_btf(cospi[57], out[28], cospi[7], out[29], cos_bit);
    step[29] = half_btf(-cospi[57], out[29], cospi[7], out[28], cos_bit);
    step[30] = half_btf(cospi[61], out[30], cospi[3], out[31], cos_bit);
    step[31] = half_btf(-cospi[61], out[31], cospi[3], out[30], cos_bit);

    // stage 3
    out[0] = step[0] + step[16];
    out[1] = step[1] + step[17];
    out[2] = step[2] + step[18];
    out[3] = step[3] + step[19];
    out[4] = step[4] + step[20];
    out[5] = step[5] + step[21];
    out[6] = step[6] + step[22];
    out[7] = step[7] + step[23];
    out[8] = step[8] + step[24];
    out[9] = step[9] + step[25];
    out[10] = step[10] + step[26];
    out[11] = step[11] + step[27];
    out[12] = step[12] + step[28];
    out[13] = step[13] + step[29];
    out[14] = step[14] + step[30];
    out[15] = step[15] + step[31];
    out[16] = -step[16] + step[0];
    out[17] = -step[17] + step[1];
    out[18] = -step[18] + step[2];
    out[19] = -step[19] + step[3];
    out[20] = -step[20] + step[4];
    out[21] = -step[21] + step[5];
    out[22] = -step[22] + step[6];
    out[23] = -step[23] + step[7];
    out[24] = -step[24] + step[8];
    out[25] = -step[25] + step[9];
    out[26] = -step[26] + step[10];
    out[27] = -step[27] + step[11];
    out[28] = -step[28] + step[12];
    out[29] = -step[29] + step[13];
    out[30] = -step[30] + step[14];
    out[31] = -step[31] + step[15];

    // stage 4
    step[0] = out[0];
    step[1] = out[1];
    step[2] = out[2];
    step[3] = out[3];
    step[4] = out[4];
    step[5] = out[5];
    step[6] = out[6];
    step[7] = out[7];
    step[8] = out[8];
    step[9] = out[9];
    step[10] = out[10];
    step[11] = out[11];
    step[12] = out[12];
    step[13] = out[13];
    step[14] = out[14];
    step[15] = out[15];
    step[16] = half_btf(cospi[4], out[16], cospi[60], out[17], cos_bit);
    step[17] = half_btf(-cospi[4], out[17], cospi[60], out[16], cos_bit);
    step[18] = half_btf(cospi[20], out[18], cospi[44], out[19], cos_bit);
    step[19] = half_btf(-cospi[20], out[19], cospi[44], out[18], cos_bit);
    step[20] = half_btf(cospi[36], out[20], cospi[28], out[21], cos_bit);
    step[21] = half_btf(-cospi[36], out[21], cospi[28], out[20], cos_bit);
    step[22] = half_btf(cospi[52], out[22], cospi[12], out[23], cos_bit);
    step[23] = half_btf(-cospi[52], out[23], cospi[12], out[22], cos_bit);
    step[24] = half_btf(-cospi[60], out[24], cospi[4], out[25], cos_bit);
    step[25] = half_btf(cospi[60], out[25], cospi[4], out[24], cos_bit);
    step[26] = half_btf(-cospi[44], out[26], cospi[20], out[27], cos_bit);
    step[27] = half_btf(cospi[44], out[27], cospi[20], out[26], cos_bit);
    step[28] = half_btf(-cospi[28], out[28], cospi[36], out[29], cos_bit);
    step[29] = half_btf(cospi[28], out[29], cospi[36], out[28], cos_bit);
    step[30] = half_btf(-cospi[12], out[30], cospi[52], out[31], cos_bit);
    step[31] = half_btf(cospi[12], out[31], cospi[52], out[30], cos_bit);

    // stage 5
    out[0] = step[0] + step[8];
    out[1] = step[1] + step[9];
    out[2] = step[2] + step[10];
    out[3] = step[3] + step[11];
    out[4] = step[4] + step[12];
    out[5] = step[5] + step[13];
    out[6] = step[6] + step[14];
    out[7] = step[7] + step[15];
    out[8] = -step[8] + step[0];
    out[9] = -step[9] + step[1];
    out[10] = -step[10] + step[2];
    out[11] = -step[11] + step[3];
    out[12] = -step[12] + step[4];
    out[13] = -step[13] + step[5];
    out[14] = -step[14] + step[6];
    out[15] = -step[15] + step[7];
    out[16] = step[16] + step[24];
    out[17] = step[17] + step[25];
    out[18] = step[18] + step[26];
    out[19] = step[19] + step[27];
    out[20] = step[20] + step[28];
    out[21] = step[21] + step[29];
    out[22] = step[22] + step[30];
    out[23] = step[23] + step[31];
    out[24] = -step[24] + step[16];
    out[25] = -step[25] + step[17];
    out[26] = -step[26] + step[18];
    out[27] = -step[27] + step[19];
    out[28] = -step[28] + step[20];
    out[29] = -step[29] + step[21];
    out[30] = -step[30] + step[22];
    out[31] = -step[31] + step[23];

    // stage 6
    step[0] = out[0];
    step[1] = out[1];
    step[2] = out[2];
    step[3] = out[3];
    step[4] = out[4];
    step[5] = out[5];
    step[6] = out[6];
    step[7] = out[7];
    step[8] = half_btf(cospi[8], out[8], cospi[56], out[9], cos_bit);
    step[9] = half_btf(-cospi[8], out[9], cospi[56], out[8], cos_bit);
    step[10] = half_btf(cospi[40], out[10], cospi[24], out[11], cos_bit);
    step[11] = half_btf(-cospi[40], out[11], cospi[24], out[10], cos_bit);
    step[12] = half_btf(-cospi[56], out[12], cospi[8], out[13], cos_bit);
    step[13] = half_btf(cospi[56], out[13], cospi[8], out[12], cos_bit);
    step[14] = half_btf(-cospi[24], out[14], cospi[40], out[15], cos_bit);
    step[15] = half_btf(cospi[24], out[15], cospi[40], out[14], cos_bit);
    step[16] = out[16];
    step[17] = out[17];
    step[18] = out[18];
    step[19] = out[19];
    step[20] = out[20];
    step[21] = out[21];
    step[22] = out[22];
    step[23] = out[23];
    step[24] = half_btf(cospi[8], out[24], cospi[56], out[25], cos_bit);
    step[25] = half_btf(-cospi[8], out[25], cospi[56], out[24], cos_bit);
    step[26] = half_btf(cospi[40], out[26], cospi[24], out[27], cos_bit);
    step[27] = half_btf(-cospi[40], out[27], cospi[24], out[26], cos_bit);
    step[28] = half_btf(-cospi[56], out[28], cospi[8], out[29], cos_bit);
    step[29] = half_btf(cospi[56], out[29], cospi[8], out[28], cos_bit);
    step[30] = half_btf(-cospi[24], out[30], cospi[40], out[31], cos_bit);
    step[31] = half_btf(cospi[24], out[31], cospi[40], out[30], cos_bit);

    // stage 7
    out[0] = step[0] + step[4];
    out[1] = step[1] + step[5];
    out[2] = step[2] + step[6];
    out[3] = step[3] + step[7];
    out[4] = -step[4] + step[0];
    out[5] = -step[5] + step[1];
    out[6] = -step[6] + step[2];
    out[7] = -step[7] + step[3];
    out[8] = step[8] + step[12];
    out[9] = step[9] + step[13];
    out[10] = step[10] + step[14];
    out[11] = step[11] + step[15];
    out[12] = -step[12] + step[8];
    out[13] = -step[13] + step[9];
    out[14] = -step[14] + step[10];
    out[15] = -step[15] + step[11];
    out[16] = step[16] + step[20];
    out[17] = step[17] + step[21];
    out[18] = step[18] + step[22];
    out[19] = step[19] + step[23];
    out[20] = -step[20] + step[16];
    out[21] = -step[21] + step[17];
    out[22] = -step[22] + step[18];
    out[23] = -step[23] + step[19];
    out[24] = step[24] + step[28];
    out[25] = step[25] + step[29];
    out[26] = step[26] + step[30];
    out[27] = step[27] + step[31];
    out[28] = -step[28] + step[24];
    out[29] = -step[29] + step[25];
    out[30] = -step[30] + step[26];
    out[31] = -step[31] + step[27];

    // stage 8
    step[0] = out[0];
    step[1] = out[1];
    step[2] = out[2];
    step[3] = out[3];
    step[4] = half_btf(cospi[16], out[4], cospi[48], out[5], cos_bit);
    step[5] = half_btf(-cospi[16], out[5], cospi[48], out[4], cos_bit);
    step[6] = half_btf(-cospi[48], out[6], cospi[16], out[7], cos_bit);
    step[7] = half_btf(cospi[48], out[7], cospi[16], out[6], cos_bit);
    step[8] = out[8];
    step[9] = out[9];
    step[10] = out[10];
    step[11] = out[11];
    step[12] = half_btf(cospi[16], out[12], cospi[48], out[13], cos_bit);
    step[13] = half_btf(-cospi[16], out[13], cospi[48], out[12], cos_bit);
    step[14] = half_btf(-cospi[48], out[14], cospi[16], out[15], cos_bit);
    step[15] = half_btf(cospi[48], out[15], cospi[16], out[14], cos_bit);
    step[16] = out[16];
    step[17] = out[17];
    step[18] = out[18];
    step[19] = out[19];
    step[20] = half_btf(cospi[16], out[20], cospi[48], out[21], cos_bit);
    step[21] = half_btf(-cospi[16], out[21], cospi[48], out[20], cos_bit);
    step[22] = half_btf(-cospi[48], out[22], cospi[16], out[23], cos_bit);
    step[23] = half_btf(cospi[48], out[23], cospi[16], out[22], cos_bit);
    step[24] = out[24];
    step[25] = out[25];
    step[26] = out[26];
    step[27] = out[27];
    step[28] = half_btf(cospi[16], out[28], cospi[48], out[29], cos_bit);
    step[29] = half_btf(-cospi[16], out[29], cospi[48], out[28], cos_bit);
    step[30] = half_btf(-cospi[48], out[30], cospi[16], out[31], cos_bit);
    step[31] = half_btf(cospi[48], out[31], cospi[16], out[30], cos_bit);

    // stage 9
    out[0] = step[0] + step[2];
    out[1] = step[1] + step[3];
    out[2] = -step[2] + step[0];
    out[3] = -step[3] + step[1];
    out[4] = step[4] + step[6];
    out[5] = step[5] + step[7];
    out[6] = -step[6] + step[4];
    out[7] = -step[7] + step[5];
    out[8] = step[8] + step[10];
    out[9] = step[9] + step[11];
    out[10] = -step[10] + step[8];
    out[11] = -step[11] + step[9];
    out[12] = step[12] + step[14];
    out[13] = step[13] + step[15];
    out[14] = -step[14] + step[12];
    out[15] = -step[15] + step[13];
    out[16] = step[16] + step[18];
    out[17] = step[17] + step[19];
    out[18] = -step[18] + step[16];
    out[19] = -step[19] + step[17];
    out[20] = step[20] + step[22];
    out[21] = step[21] + step[23];
    out[22] = -step[22] + step[20];
    out[23] = -step[23] + step[21];
    out[24] = step[24] + step[26];
    out[25] = step[25] + step[27];
    out[26] = -step[26] + step[24];
    out[27] = -step[27] + step[25];
    out[28] = step[28] + step[30];
    out[29] = step[29] + step[31];
    out[30] = -step[30] + step[28];
    out[31] = -step[31] + step[29];

    // stage 10
    step[0] = out[0];
    step[1] = out[1];
    step[2] = half_btf(cospi[32], out[2], cospi[32], out[3], cos_bit);
    step[3] = half_btf(-cospi[32], out[3], cospi[32], out[2], cos_bit);
    step[4] = out[4];
    step[5] = out[5];
    step[6] = half_btf(cospi[32], out[6], cospi[32], out[7], cos_bit);
    step[7] = half_btf(-cospi[32], out[7], cospi[32], out[6], cos_bit);
    step[8] = out[8];
    step[9] = out[9];
    step[10] = half_btf(cospi[32], out[10], cospi[32], out[11], cos_bit);
    step[11] = half_btf(-cospi[32], out[11], cospi[32], out[10], cos_bit);
    step[12] = out[12];
    step[13] = out[13];
    step[14] = half_btf(cospi[32], out[14], cospi[32], out[15], cos_bit);
    step[15] = half_btf(-cospi[32], out[15], cospi[32], out[14], cos_bit);
    step[16] = out[16];
    step[17] = out[17];
    step[18] = half_btf(cospi[32], out[18], cospi[32], out[19], cos_bit);
    step[19] = half_btf(-cospi[32], out[19], cospi[32], out[18], cos_bit);
    step[20] = out[20];
    step[21] = out[21];
    step[22] = half_btf(cospi[32], out[22], cospi[32], out[23], cos_bit);
    step[23] = half_btf(-cospi[32], out[23], cospi[32], out[22], cos_bit);
    step[24] = out[24];
    step[25] = out[25];
    step[26] = half_btf(cospi[32], out[26], cospi[32], out[27], cos_bit);
    step[27] = half_btf(-cospi[32], out[27], cospi[32], out[26], cos_bit);
    step[28] = out[28];
    step[29] = out[29];
    step[30] = half_btf(cospi[32], out[30], cospi[32], out[31], cos_bit);
    step[31] = half_btf(-cospi[32], out[31], cospi[32], out[30], cos_bit);

    // stage 11
    out[0] = step[0];
    out[1] = -step[16];
    out[2] = step[24];
    out[3] = -step[8];
    out[4] = step[12];
    out[5] = -step[28];
    out[6] = step[20];
    out[7] = -step[4];
    out[8] = step[6];
    out[9] = -step[22];
    out[10] = step[30];
    out[11] = -step[14];
    out[12] = step[10];
    out[13] = -step[26];
    out[14] = step[18];
    out[15] = -step[2];
    out[16] = step[3];
    out[17] = -step[19];
    out[18] = step[27];
    out[19] = -step[11];
    out[20] = step[15];
    out[21] = -step[31];
    out[22] = step[23];
    out[23] = -step[7];
    out[24] = step[5];
    out[25] = -step[21];
    out[26] = step[29];
    out[27] = -step[13];
    out[28] = step[9];
    out[29] = -step[25];
    out[30] = step[17];
    out[31] = -step[1];
}

// =============================================================================
// 8-point identity transform
// =============================================================================

pub fn fidentity8(input: &[TranLow], output: &mut [TranLow], _cos_bit: i8) {
    for i in 0..8 {
        output[i] = input[i] * 2;
    }
}

// =============================================================================
// 16-point identity transform
// =============================================================================

pub fn fidentity16(input: &[TranLow], output: &mut [TranLow], _cos_bit: i8) {
    let new_sqrt2 = NEW_SQRT2;
    for i in 0..16 {
        output[i] = round_shift_i64(input[i] as i64 * 2 * new_sqrt2 as i64, NEW_SQRT2_BITS);
    }
}

// =============================================================================
// 1D Transform function type and dispatch
// =============================================================================

/// 1D forward transform function signature.
pub type TxfmFunc = fn(&[TranLow], &mut [TranLow], i8);

/// Get the 1D forward transform function for a given type and size.
pub fn get_fwd_txfm_func(tx_type_1d: u8, size: usize) -> Option<TxfmFunc> {
    // tx_type_1d: 0=DCT, 1=ADST, 2=FLIPADST, 3=IDENTITY
    match (tx_type_1d, size) {
        (0, 4) => Some(fdct4),
        (0, 8) => Some(fdct8),
        (0, 16) => Some(fdct16),
        (0, 32) => Some(fdct32),
        (0, 64) => Some(fdct64),
        (1, 4) => Some(fadst4),
        (1, 8) => Some(fadst8),
        (1, 16) => Some(fadst16),
        // C `av1_txfm_type_ls[3][TX_TYPE_1D_ADST]` is TXFM_TYPE_ADST32, which
        // `svt_aom_fwd_txfm_type_to_func` (transforms.c:2959) answers with
        // `av1_fadst32_new`. No conformant AV1 ext-tx set reaches it (see
        // `fadst32`), but the C dispatch does and so does this one.
        (1, 32) => Some(fadst32),
        (2, 4) => Some(fadst4), // FLIPADST uses ADST with flipped input
        (2, 8) => Some(fadst8),
        (2, 16) => Some(fadst16),
        (2, 32) => Some(fadst32),
        (3, 4) => Some(fidentity4),
        (3, 8) => Some(fidentity8),
        (3, 16) => Some(fidentity16),
        (3, 32) => Some(fidentity32),
        (3, 64) => Some(fidentity64),
        _ => None,
    }
}

// =============================================================================
// General 2D forward transform — C-exact port of av1_tranform_two_d_core_c
// (transforms.c:2978)
// =============================================================================

/// C-exact 2D forward composition.
///
/// Column pass first (with optional upside-down flip on load), then row pass;
/// per-pass round shifts from the C `fwd_txfm_shift_ls` tables, per-pass cos
/// bits from `fwd_cos_bit_col/row`, and the sqrt(2) scale on rows when the
/// log2 aspect ratio is exactly 1 (2:1 rectangles) — 4:1 rectangles get NO
/// extra scale, exactly like C.
#[allow(clippy::too_many_arguments)]
pub fn fwd_txfm2d_core(
    input: &[i16],
    output: &mut [TranLow],
    input_stride: usize,
    w: usize,
    h: usize,
    col_func: TxfmFunc,
    row_func: TxfmFunc,
    cos_bit_col: i8,
    cos_bit_row: i8,
    shift: [i8; 3],
    ud_flip: bool,
    lr_flip: bool,
) {
    // `buf` is fully written by the column pass before the row pass reads
    // it; the temp vectors are overwritten per column/row. Thread-local
    // staging plus fixed stack temps avoid per-call alloc + memset.
    let mut temp_in = [0i32; 64];
    let mut temp_out = [0i32; 64];
    let mut row_out = [0i32; 64];
    // get_rect_tx_log_ratio(col, row)
    let rect_log_ratio = w.trailing_zeros() as i32 - h.trailing_zeros() as i32;
    with_txfm_stage(w * h, |buf| {
        // Columns
        for c in 0..w {
            if !ud_flip {
                for r in 0..h {
                    temp_in[r] = i32::from(input[r * input_stride + c]);
                }
            } else {
                for r in 0..h {
                    // flip upside down
                    temp_in[r] = i32::from(input[(h - r - 1) * input_stride + c]);
                }
            }
            round_shift_array(&mut temp_in[..h], -(shift[0] as i32));
            col_func(&temp_in[..h], &mut temp_out[..h], cos_bit_col);
            round_shift_array(&mut temp_out[..h], -(shift[1] as i32));
            if !lr_flip {
                for r in 0..h {
                    buf[r * w + c] = temp_out[r];
                }
            } else {
                for r in 0..h {
                    // flip from left to right
                    buf[r * w + (w - c - 1)] = temp_out[r];
                }
            }
        }

        // Rows
        for r in 0..h {
            row_func(&buf[r * w..r * w + w], &mut row_out[..w], cos_bit_row);
            round_shift_array(&mut row_out[..w], -(shift[2] as i32));
            if rect_log_ratio.abs() == 1 {
                // Multiply everything by Sqrt2 if the transform is rectangular
                // and the size difference is a factor of 2.
                for v in row_out[..w].iter_mut() {
                    *v = round_shift_i64(*v as i64 * NEW_SQRT2 as i64, NEW_SQRT2_BITS);
                }
            }
            output[r * w..r * w + w].copy_from_slice(&row_out[..w]);
        }
    });
}

/// Configured C-exact forward 2D transform (svt_av1_transform_two_d semantics).
///
/// `col_1d`/`row_1d`: 0=DCT, 1=ADST, 2=FLIPADST, 3=IDENTITY. Returns false if
/// the (type, size) combination has no 1D kernel (e.g. ADST on 32/64 dims).
#[allow(clippy::too_many_arguments)]
pub fn fwd_txfm2d_c_exact(
    input: &[i16],
    output: &mut [TranLow],
    input_stride: usize,
    w: usize,
    h: usize,
    col_1d: u8,
    row_1d: u8,
    ud_flip: bool,
    lr_flip: bool,
) -> bool {
    // SIMD fast path: square DCT-DCT with no flips (byte-exact, additive).
    if col_1d == 0
        && row_1d == 0
        && w == h
        && !ud_flip
        && !lr_flip
        && crate::txfm_simd::try_fwd_dct_square(input, output, input_stride, w)
    {
        return true;
    }
    // SIMD fast path: non-square (rectangular) DCT-DCT, no flips.
    if col_1d == 0
        && row_1d == 0
        && w != h
        && !ud_flip
        && !lr_flip
        && crate::txfm_simd::try_fwd_dct_rect(input, output, input_stride, w, h)
    {
        return true;
    }
    // SIMD fast path: ADST_DCT / DCT_ADST / ADST_ADST (8x8/16x16/8x16/16x8),
    // no flips.
    if !ud_flip
        && !lr_flip
        && crate::txfm_simd::try_fwd_adst(input, output, input_stride, w, h, col_1d, row_1d)
    {
        return true;
    }
    // SIMD fast path: extended types — FLIPADST (all combos, with the block
    // edge flip), IDENTITY (IDTX), and the mixed V_/H_ types. Gated internally
    // by `simd_ext_supported`.
    if crate::txfm_simd::try_fwd_ext(
        input,
        output,
        input_stride,
        w,
        h,
        col_1d,
        row_1d,
        ud_flip,
        lr_flip,
    ) {
        return true;
    }
    // SIMD fast path: 4-dim sizes (4x4/4x8/8x4/4x16/16x4), any tx type.
    if crate::txfm_simd::try_fwd_4dim(
        input,
        output,
        input_stride,
        w,
        h,
        col_1d,
        row_1d,
        ud_flip,
        lr_flip,
    ) {
        return true;
    }
    let col_func = match get_fwd_txfm_func(col_1d, h) {
        Some(f) => f,
        None => return false,
    };
    let row_func = match get_fwd_txfm_func(row_1d, w) {
        Some(f) => f,
        None => return false,
    };
    let txw_idx = w.trailing_zeros() as usize - 2;
    let txh_idx = h.trailing_zeros() as usize - 2;
    fwd_txfm2d_core(
        input,
        output,
        input_stride,
        w,
        h,
        col_func,
        row_func,
        FWD_COS_BIT_COL[txw_idx][txh_idx],
        FWD_COS_BIT_ROW[txw_idx][txh_idx],
        fwd_txfm_shift(w, h),
        ud_flip,
        lr_flip,
    );
    true
}

/// Forward 64x64 DCT-DCT.
pub fn fwd_txfm2d_64x64_dct_dct(input: &[i16], output: &mut [TranLow], stride: usize) {
    fwd_txfm2d_c_exact(input, output, stride, 64, 64, 0, 0, false, false);
}

/// Forward 4x8 DCT-DCT (rectangular).
pub fn fwd_txfm2d_4x8_dct_dct(input: &[i16], output: &mut [TranLow], stride: usize) {
    fwd_txfm2d_c_exact(input, output, stride, 4, 8, 0, 0, false, false);
}

/// Forward 8x4 DCT-DCT (rectangular).
pub fn fwd_txfm2d_8x4_dct_dct(input: &[i16], output: &mut [TranLow], stride: usize) {
    fwd_txfm2d_c_exact(input, output, stride, 8, 4, 0, 0, false, false);
}

/// Forward 8x16 DCT-DCT (rectangular).
pub fn fwd_txfm2d_8x16_dct_dct(input: &[i16], output: &mut [TranLow], stride: usize) {
    fwd_txfm2d_c_exact(input, output, stride, 8, 16, 0, 0, false, false);
}

/// Forward 16x8 DCT-DCT (rectangular).
pub fn fwd_txfm2d_16x8_dct_dct(input: &[i16], output: &mut [TranLow], stride: usize) {
    fwd_txfm2d_c_exact(input, output, stride, 16, 8, 0, 0, false, false);
}

/// Forward 16x32 DCT-DCT (rectangular).
pub fn fwd_txfm2d_16x32_dct_dct(input: &[i16], output: &mut [TranLow], stride: usize) {
    fwd_txfm2d_c_exact(input, output, stride, 16, 32, 0, 0, false, false);
}

/// Forward 32x16 DCT-DCT (rectangular).
pub fn fwd_txfm2d_32x16_dct_dct(input: &[i16], output: &mut [TranLow], stride: usize) {
    fwd_txfm2d_c_exact(input, output, stride, 32, 16, 0, 0, false, false);
}

/// Forward 32x64 DCT-DCT (rectangular).
pub fn fwd_txfm2d_32x64_dct_dct(input: &[i16], output: &mut [TranLow], stride: usize) {
    fwd_txfm2d_c_exact(input, output, stride, 32, 64, 0, 0, false, false);
}

/// Forward 64x32 DCT-DCT (rectangular).
pub fn fwd_txfm2d_64x32_dct_dct(input: &[i16], output: &mut [TranLow], stride: usize) {
    fwd_txfm2d_c_exact(input, output, stride, 64, 32, 0, 0, false, false);
}

/// Forward 4x16 DCT-DCT (4:1 rectangular).
pub fn fwd_txfm2d_4x16_dct_dct(input: &[i16], output: &mut [TranLow], stride: usize) {
    fwd_txfm2d_c_exact(input, output, stride, 4, 16, 0, 0, false, false);
}

/// Forward 16x4 DCT-DCT (4:1 rectangular).
pub fn fwd_txfm2d_16x4_dct_dct(input: &[i16], output: &mut [TranLow], stride: usize) {
    fwd_txfm2d_c_exact(input, output, stride, 16, 4, 0, 0, false, false);
}

/// Forward 8x32 DCT-DCT (4:1 rectangular).
pub fn fwd_txfm2d_8x32_dct_dct(input: &[i16], output: &mut [TranLow], stride: usize) {
    fwd_txfm2d_c_exact(input, output, stride, 8, 32, 0, 0, false, false);
}

/// Forward 32x8 DCT-DCT (4:1 rectangular).
pub fn fwd_txfm2d_32x8_dct_dct(input: &[i16], output: &mut [TranLow], stride: usize) {
    fwd_txfm2d_c_exact(input, output, stride, 32, 8, 0, 0, false, false);
}

/// Forward 16x64 DCT-DCT (4:1 rectangular).
pub fn fwd_txfm2d_16x64_dct_dct(input: &[i16], output: &mut [TranLow], stride: usize) {
    fwd_txfm2d_c_exact(input, output, stride, 16, 64, 0, 0, false, false);
}

/// Forward 64x16 DCT-DCT (4:1 rectangular).
pub fn fwd_txfm2d_64x16_dct_dct(input: &[i16], output: &mut [TranLow], stride: usize) {
    fwd_txfm2d_c_exact(input, output, stride, 64, 16, 0, 0, false, false);
}

/// Forward 4x4 DCT-DCT using the general framework.
pub fn fwd_txfm2d_4x4_dct_dct(input: &[i16], output: &mut [TranLow], stride: usize) {
    incant!(
        fwd_txfm2d_4x4_dct_dct_impl(input, output, stride),
        [v3, neon, scalar]
    )
}

fn fwd_txfm2d_4x4_dct_dct_impl_scalar(
    _token: ScalarToken,
    input: &[i16],
    output: &mut [TranLow],
    stride: usize,
) {
    fwd_txfm2d_c_exact(input, output, stride, 4, 4, 0, 0, false, false);
}

#[cfg(target_arch = "x86_64")]
#[arcane]
fn fwd_txfm2d_4x4_dct_dct_impl_v3(
    _token: Desktop64,
    input: &[i16],
    output: &mut [TranLow],
    stride: usize,
) {
    fwd_txfm2d_c_exact(input, output, stride, 4, 4, 0, 0, false, false);
}

#[cfg(target_arch = "aarch64")]
#[arcane]
fn fwd_txfm2d_4x4_dct_dct_impl_neon(
    _token: NeonToken,
    input: &[i16],
    output: &mut [TranLow],
    stride: usize,
) {
    fwd_txfm2d_c_exact(input, output, stride, 4, 4, 0, 0, false, false);
}

/// Forward 8x8 DCT-DCT.
pub fn fwd_txfm2d_8x8_dct_dct(input: &[i16], output: &mut [TranLow], stride: usize) {
    incant!(
        fwd_txfm2d_8x8_dct_dct_impl(input, output, stride),
        [v3, neon, scalar]
    )
}

fn fwd_txfm2d_8x8_dct_dct_impl_scalar(
    _token: ScalarToken,
    input: &[i16],
    output: &mut [TranLow],
    stride: usize,
) {
    fwd_txfm2d_c_exact(input, output, stride, 8, 8, 0, 0, false, false);
}

#[cfg(target_arch = "x86_64")]
#[arcane]
fn fwd_txfm2d_8x8_dct_dct_impl_v3(
    _token: Desktop64,
    input: &[i16],
    output: &mut [TranLow],
    stride: usize,
) {
    fwd_txfm2d_c_exact(input, output, stride, 8, 8, 0, 0, false, false);
}

#[cfg(target_arch = "aarch64")]
#[arcane]
fn fwd_txfm2d_8x8_dct_dct_impl_neon(
    _token: NeonToken,
    input: &[i16],
    output: &mut [TranLow],
    stride: usize,
) {
    fwd_txfm2d_c_exact(input, output, stride, 8, 8, 0, 0, false, false);
}

/// Forward 16x16 DCT-DCT.
pub fn fwd_txfm2d_16x16_dct_dct(input: &[i16], output: &mut [TranLow], stride: usize) {
    incant!(
        fwd_txfm2d_16x16_dct_dct_impl(input, output, stride),
        [v3, neon, scalar]
    )
}

fn fwd_txfm2d_16x16_dct_dct_impl_scalar(
    _token: ScalarToken,
    input: &[i16],
    output: &mut [TranLow],
    stride: usize,
) {
    fwd_txfm2d_c_exact(input, output, stride, 16, 16, 0, 0, false, false);
}

#[cfg(target_arch = "x86_64")]
#[arcane]
fn fwd_txfm2d_16x16_dct_dct_impl_v3(
    _token: Desktop64,
    input: &[i16],
    output: &mut [TranLow],
    stride: usize,
) {
    fwd_txfm2d_c_exact(input, output, stride, 16, 16, 0, 0, false, false);
}

#[cfg(target_arch = "aarch64")]
#[arcane]
fn fwd_txfm2d_16x16_dct_dct_impl_neon(
    _token: NeonToken,
    input: &[i16],
    output: &mut [TranLow],
    stride: usize,
) {
    fwd_txfm2d_c_exact(input, output, stride, 16, 16, 0, 0, false, false);
}

/// Forward 32x32 DCT-DCT.
pub fn fwd_txfm2d_32x32_dct_dct(input: &[i16], output: &mut [TranLow], stride: usize) {
    incant!(
        fwd_txfm2d_32x32_dct_dct_impl(input, output, stride),
        [v3, neon, scalar]
    )
}

fn fwd_txfm2d_32x32_dct_dct_impl_scalar(
    _token: ScalarToken,
    input: &[i16],
    output: &mut [TranLow],
    stride: usize,
) {
    fwd_txfm2d_c_exact(input, output, stride, 32, 32, 0, 0, false, false);
}

#[cfg(target_arch = "x86_64")]
#[arcane]
fn fwd_txfm2d_32x32_dct_dct_impl_v3(
    _token: Desktop64,
    input: &[i16],
    output: &mut [TranLow],
    stride: usize,
) {
    fwd_txfm2d_c_exact(input, output, stride, 32, 32, 0, 0, false, false);
}

#[cfg(target_arch = "aarch64")]
#[arcane]
fn fwd_txfm2d_32x32_dct_dct_impl_neon(
    _token: NeonToken,
    input: &[i16],
    output: &mut [TranLow],
    stride: usize,
) {
    fwd_txfm2d_c_exact(input, output, stride, 32, 32, 0, 0, false, false);
}

// =============================================================================
// Forward 4x4 Walsh-Hadamard transform (AV1 lossless / qindex 0)
//
// AV1 lossless does NOT use the DCT: `svt_aom_estimate_transform`
// (transforms.c:3950-3961) routes `svt_av1_is_lossless_segment(pcs, seg_id) &&
// transform_size == TX_4X4` to `svt_av1_fwht4x4` instead, asserting
// `transform_type == DCT_DCT`. Any other tx size falls through to the normal
// DCT path (a deliberate guard for gitlab#2373 — a larger lossless block must
// still fill the whole coeff buffer).
//
// NOT WIRED into `txfm_dispatch` on purpose: the frame-level lossless
// derivation (`svt_av1_is_lossless_segment`, segment qindex == 0 with no
// deltas) is a separate port item, so an unreachable-but-correct kernel is the
// intended end state of this chunk.
// =============================================================================

/// C `UNIT_QUANT_SHIFT` (transforms.h:25; identically inv_transforms.h:23).
pub const UNIT_QUANT_SHIFT: u32 = 2;

/// C `UNIT_QUANT_FACTOR` (transforms.h:26) = `1 << UNIT_QUANT_SHIFT`.
pub const UNIT_QUANT_FACTOR: i64 = 1 << UNIT_QUANT_SHIFT;

/// 4-point reversible, orthonormal Walsh-Hadamard transform in 3.5 adds,
/// 0.5 shifts per pixel — C `svt_av1_fwht4x4_c` (transforms.c:3879-3931).
/// Shared by the 8-bit and high-bit-depth lossless paths (the C comment at
/// transforms.c:3875-3878 says so explicitly; nothing in the body depends on
/// bit depth because the input is already a residual).
///
/// `input` is a 4x4 residual block at row stride `stride`; `output` receives
/// 16 coefficients packed at stride 4.
///
/// Two passes, and they are NOT symmetric — this trips up anyone assuming the
/// usual "column pass, transpose, row pass" shape:
/// * pass 0 transforms each **column** of `input` and stores the result of
///   column `i` into **row** `i` of `output` (C advances `ip_pass0` by 1 and
///   `op` by 4), i.e. it transposes;
/// * pass 1 transforms each **column** of `output` in place, writing back to
///   the same column (C advances both `ip` and `op` by 1), i.e. it does not.
///
/// Net effect: `output` is the TRANSPOSE of the natural 2D coefficient matrix.
/// C undoes that in `svt_aom_estimate_transform` (transforms.c:3955-3959) with
/// an explicit `coeff_buffer[(j << 2) + i] = dst[(i << 2) + j]` loop before
/// quantization — the inverse WHT consumes natural (un-transposed) order.
/// That transpose belongs to the dispatch layer and is intentionally NOT
/// folded in here, exactly as in C.
///
/// Arithmetic notes (bug-for-bug):
/// * C carries every intermediate in `int64_t`, so with `int16_t` inputs no
///   step can overflow — the loose analytic bound is 5 x the pass-0 bound of
///   163839, under 2^22 after the x4 scale, and the MEASURED peak over every
///   saturated corner plus 20k random full-i16 blocks is 524288 = 2^19
///   (tests/c_parity_wht.rs `fwht4x4_intermediates_never_leave_i32`). The port
///   uses `i64` anyway, to match C rather than to need the width.
/// * `>> 1` is an arithmetic shift on a possibly-negative value (floor, not
///   truncate-toward-zero). Rust's `>>` on a signed type is arithmetic, so it
///   matches gcc/clang.
/// * the stores are C `(int32_t)` casts of an `int64_t`; Rust's `as i32` is the
///   same modular truncation.
pub fn fwht4x4(input: &[i16], output: &mut [TranLow], stride: usize) {
    assert!(
        output.len() >= 16,
        "fwht4x4 output must hold 16 coefficients"
    );
    assert!(
        input.len() >= 3 * stride + 4,
        "fwht4x4 input must hold 4 rows of 4 at stride {stride}"
    );

    // Pass 0: WHT of each column of `input` -> row `i` of `output`.
    for i in 0..4 {
        let mut a1 = i64::from(input[i]);
        let mut b1 = i64::from(input[stride + i]);
        let mut c1 = i64::from(input[2 * stride + i]);
        let mut d1 = i64::from(input[3 * stride + i]);

        a1 += b1;
        d1 -= c1;
        let e1 = (a1 - d1) >> 1;
        b1 = e1 - b1;
        c1 = e1 - c1;
        a1 -= c1;
        d1 += b1;

        // C store order is (a1, c1, d1, b1) — not (a1, b1, c1, d1).
        output[i * 4] = a1 as i32;
        output[i * 4 + 1] = c1 as i32;
        output[i * 4 + 2] = d1 as i32;
        output[i * 4 + 3] = b1 as i32;
    }

    // Pass 1: WHT of each column of `output`, in place, scaled by
    // UNIT_QUANT_FACTOR. C reads all four inputs before storing, so the
    // in-place update is well defined.
    for i in 0..4 {
        let mut a1 = i64::from(output[i]);
        let mut b1 = i64::from(output[4 + i]);
        let mut c1 = i64::from(output[8 + i]);
        let mut d1 = i64::from(output[12 + i]);

        a1 += b1;
        d1 -= c1;
        let e1 = (a1 - d1) >> 1;
        b1 = e1 - b1;
        c1 = e1 - c1;
        a1 -= c1;
        d1 += b1;

        output[i] = (a1 * UNIT_QUANT_FACTOR) as i32;
        output[4 + i] = (c1 * UNIT_QUANT_FACTOR) as i32;
        output[8 + i] = (d1 * UNIT_QUANT_FACTOR) as i32;
        output[12 + i] = (b1 * UNIT_QUANT_FACTOR) as i32;
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod dispatch_tests;

mod fdct64;
pub use fdct64::*;
