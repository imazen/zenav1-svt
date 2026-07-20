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

pub fn fdct64(input: &[TranLow], output: &mut [TranLow], cos_bit: i8) {
    let cospi = cospi_arr(cos_bit);
    let cos_bit = cos_bit as u32;
    let mut step = [0i32; 64];

    // stage 1
    output[0] = input[0] + input[63];
    output[1] = input[1] + input[62];
    output[2] = input[2] + input[61];
    output[3] = input[3] + input[60];
    output[4] = input[4] + input[59];
    output[5] = input[5] + input[58];
    output[6] = input[6] + input[57];
    output[7] = input[7] + input[56];
    output[8] = input[8] + input[55];
    output[9] = input[9] + input[54];
    output[10] = input[10] + input[53];
    output[11] = input[11] + input[52];
    output[12] = input[12] + input[51];
    output[13] = input[13] + input[50];
    output[14] = input[14] + input[49];
    output[15] = input[15] + input[48];
    output[16] = input[16] + input[47];
    output[17] = input[17] + input[46];
    output[18] = input[18] + input[45];
    output[19] = input[19] + input[44];
    output[20] = input[20] + input[43];
    output[21] = input[21] + input[42];
    output[22] = input[22] + input[41];
    output[23] = input[23] + input[40];
    output[24] = input[24] + input[39];
    output[25] = input[25] + input[38];
    output[26] = input[26] + input[37];
    output[27] = input[27] + input[36];
    output[28] = input[28] + input[35];
    output[29] = input[29] + input[34];
    output[30] = input[30] + input[33];
    output[31] = input[31] + input[32];
    output[32] = -input[32] + input[31];
    output[33] = -input[33] + input[30];
    output[34] = -input[34] + input[29];
    output[35] = -input[35] + input[28];
    output[36] = -input[36] + input[27];
    output[37] = -input[37] + input[26];
    output[38] = -input[38] + input[25];
    output[39] = -input[39] + input[24];
    output[40] = -input[40] + input[23];
    output[41] = -input[41] + input[22];
    output[42] = -input[42] + input[21];
    output[43] = -input[43] + input[20];
    output[44] = -input[44] + input[19];
    output[45] = -input[45] + input[18];
    output[46] = -input[46] + input[17];
    output[47] = -input[47] + input[16];
    output[48] = -input[48] + input[15];
    output[49] = -input[49] + input[14];
    output[50] = -input[50] + input[13];
    output[51] = -input[51] + input[12];
    output[52] = -input[52] + input[11];
    output[53] = -input[53] + input[10];
    output[54] = -input[54] + input[9];
    output[55] = -input[55] + input[8];
    output[56] = -input[56] + input[7];
    output[57] = -input[57] + input[6];
    output[58] = -input[58] + input[5];
    output[59] = -input[59] + input[4];
    output[60] = -input[60] + input[3];
    output[61] = -input[61] + input[2];
    output[62] = -input[62] + input[1];
    output[63] = -input[63] + input[0];

    // stage 2
    let o = |i: usize| -> i32 { output[i] };
    step[0] = o(0) + o(31);
    step[1] = o(1) + o(30);
    step[2] = o(2) + o(29);
    step[3] = o(3) + o(28);
    step[4] = o(4) + o(27);
    step[5] = o(5) + o(26);
    step[6] = o(6) + o(25);
    step[7] = o(7) + o(24);
    step[8] = o(8) + o(23);
    step[9] = o(9) + o(22);
    step[10] = o(10) + o(21);
    step[11] = o(11) + o(20);
    step[12] = o(12) + o(19);
    step[13] = o(13) + o(18);
    step[14] = o(14) + o(17);
    step[15] = o(15) + o(16);
    step[16] = -o(16) + o(15);
    step[17] = -o(17) + o(14);
    step[18] = -o(18) + o(13);
    step[19] = -o(19) + o(12);
    step[20] = -o(20) + o(11);
    step[21] = -o(21) + o(10);
    step[22] = -o(22) + o(9);
    step[23] = -o(23) + o(8);
    step[24] = -o(24) + o(7);
    step[25] = -o(25) + o(6);
    step[26] = -o(26) + o(5);
    step[27] = -o(27) + o(4);
    step[28] = -o(28) + o(3);
    step[29] = -o(29) + o(2);
    step[30] = -o(30) + o(1);
    step[31] = -o(31) + o(0);
    step[32] = o(32);
    step[33] = o(33);
    step[34] = o(34);
    step[35] = o(35);
    step[36] = o(36);
    step[37] = o(37);
    step[38] = o(38);
    step[39] = o(39);
    step[40] = half_btf(-cospi[32], o(40), cospi[32], o(55), cos_bit);
    step[41] = half_btf(-cospi[32], o(41), cospi[32], o(54), cos_bit);
    step[42] = half_btf(-cospi[32], o(42), cospi[32], o(53), cos_bit);
    step[43] = half_btf(-cospi[32], o(43), cospi[32], o(52), cos_bit);
    step[44] = half_btf(-cospi[32], o(44), cospi[32], o(51), cos_bit);
    step[45] = half_btf(-cospi[32], o(45), cospi[32], o(50), cos_bit);
    step[46] = half_btf(-cospi[32], o(46), cospi[32], o(49), cos_bit);
    step[47] = half_btf(-cospi[32], o(47), cospi[32], o(48), cos_bit);
    step[48] = half_btf(cospi[32], o(48), cospi[32], o(47), cos_bit);
    step[49] = half_btf(cospi[32], o(49), cospi[32], o(46), cos_bit);
    step[50] = half_btf(cospi[32], o(50), cospi[32], o(45), cos_bit);
    step[51] = half_btf(cospi[32], o(51), cospi[32], o(44), cos_bit);
    step[52] = half_btf(cospi[32], o(52), cospi[32], o(43), cos_bit);
    step[53] = half_btf(cospi[32], o(53), cospi[32], o(42), cos_bit);
    step[54] = half_btf(cospi[32], o(54), cospi[32], o(41), cos_bit);
    step[55] = half_btf(cospi[32], o(55), cospi[32], o(40), cos_bit);
    step[56] = o(56);
    step[57] = o(57);
    step[58] = o(58);
    step[59] = o(59);
    step[60] = o(60);
    step[61] = o(61);
    step[62] = o(62);
    step[63] = o(63);

    // stage 3
    let s = |i: usize| -> i32 { step[i] };
    output[0] = s(0) + s(15);
    output[1] = s(1) + s(14);
    output[2] = s(2) + s(13);
    output[3] = s(3) + s(12);
    output[4] = s(4) + s(11);
    output[5] = s(5) + s(10);
    output[6] = s(6) + s(9);
    output[7] = s(7) + s(8);
    output[8] = -s(8) + s(7);
    output[9] = -s(9) + s(6);
    output[10] = -s(10) + s(5);
    output[11] = -s(11) + s(4);
    output[12] = -s(12) + s(3);
    output[13] = -s(13) + s(2);
    output[14] = -s(14) + s(1);
    output[15] = -s(15) + s(0);
    output[16] = s(16);
    output[17] = s(17);
    output[18] = s(18);
    output[19] = s(19);
    output[20] = half_btf(-cospi[32], s(20), cospi[32], s(27), cos_bit);
    output[21] = half_btf(-cospi[32], s(21), cospi[32], s(26), cos_bit);
    output[22] = half_btf(-cospi[32], s(22), cospi[32], s(25), cos_bit);
    output[23] = half_btf(-cospi[32], s(23), cospi[32], s(24), cos_bit);
    output[24] = half_btf(cospi[32], s(24), cospi[32], s(23), cos_bit);
    output[25] = half_btf(cospi[32], s(25), cospi[32], s(22), cos_bit);
    output[26] = half_btf(cospi[32], s(26), cospi[32], s(21), cos_bit);
    output[27] = half_btf(cospi[32], s(27), cospi[32], s(20), cos_bit);
    output[28] = s(28);
    output[29] = s(29);
    output[30] = s(30);
    output[31] = s(31);
    output[32] = s(32) + s(47);
    output[33] = s(33) + s(46);
    output[34] = s(34) + s(45);
    output[35] = s(35) + s(44);
    output[36] = s(36) + s(43);
    output[37] = s(37) + s(42);
    output[38] = s(38) + s(41);
    output[39] = s(39) + s(40);
    output[40] = -s(40) + s(39);
    output[41] = -s(41) + s(38);
    output[42] = -s(42) + s(37);
    output[43] = -s(43) + s(36);
    output[44] = -s(44) + s(35);
    output[45] = -s(45) + s(34);
    output[46] = -s(46) + s(33);
    output[47] = -s(47) + s(32);
    output[48] = -s(48) + s(63);
    output[49] = -s(49) + s(62);
    output[50] = -s(50) + s(61);
    output[51] = -s(51) + s(60);
    output[52] = -s(52) + s(59);
    output[53] = -s(53) + s(58);
    output[54] = -s(54) + s(57);
    output[55] = -s(55) + s(56);
    output[56] = s(56) + s(55);
    output[57] = s(57) + s(54);
    output[58] = s(58) + s(53);
    output[59] = s(59) + s(52);
    output[60] = s(60) + s(51);
    output[61] = s(61) + s(50);
    output[62] = s(62) + s(49);
    output[63] = s(63) + s(48);

    // stage 4
    let o = |i: usize| -> i32 { output[i] };
    step[0] = o(0) + o(7);
    step[1] = o(1) + o(6);
    step[2] = o(2) + o(5);
    step[3] = o(3) + o(4);
    step[4] = -o(4) + o(3);
    step[5] = -o(5) + o(2);
    step[6] = -o(6) + o(1);
    step[7] = -o(7) + o(0);
    step[8] = o(8);
    step[9] = o(9);
    step[10] = half_btf(-cospi[32], o(10), cospi[32], o(13), cos_bit);
    step[11] = half_btf(-cospi[32], o(11), cospi[32], o(12), cos_bit);
    step[12] = half_btf(cospi[32], o(12), cospi[32], o(11), cos_bit);
    step[13] = half_btf(cospi[32], o(13), cospi[32], o(10), cos_bit);
    step[14] = o(14);
    step[15] = o(15);
    step[16] = o(16) + o(23);
    step[17] = o(17) + o(22);
    step[18] = o(18) + o(21);
    step[19] = o(19) + o(20);
    step[20] = -o(20) + o(19);
    step[21] = -o(21) + o(18);
    step[22] = -o(22) + o(17);
    step[23] = -o(23) + o(16);
    step[24] = -o(24) + o(31);
    step[25] = -o(25) + o(30);
    step[26] = -o(26) + o(29);
    step[27] = -o(27) + o(28);
    step[28] = o(28) + o(27);
    step[29] = o(29) + o(26);
    step[30] = o(30) + o(25);
    step[31] = o(31) + o(24);
    step[32] = o(32);
    step[33] = o(33);
    step[34] = o(34);
    step[35] = o(35);
    step[36] = half_btf(-cospi[16], o(36), cospi[48], o(59), cos_bit);
    step[37] = half_btf(-cospi[16], o(37), cospi[48], o(58), cos_bit);
    step[38] = half_btf(-cospi[16], o(38), cospi[48], o(57), cos_bit);
    step[39] = half_btf(-cospi[16], o(39), cospi[48], o(56), cos_bit);
    step[40] = half_btf(-cospi[48], o(40), -cospi[16], o(55), cos_bit);
    step[41] = half_btf(-cospi[48], o(41), -cospi[16], o(54), cos_bit);
    step[42] = half_btf(-cospi[48], o(42), -cospi[16], o(53), cos_bit);
    step[43] = half_btf(-cospi[48], o(43), -cospi[16], o(52), cos_bit);
    step[44] = o(44);
    step[45] = o(45);
    step[46] = o(46);
    step[47] = o(47);
    step[48] = o(48);
    step[49] = o(49);
    step[50] = o(50);
    step[51] = o(51);
    step[52] = half_btf(cospi[48], o(52), -cospi[16], o(43), cos_bit);
    step[53] = half_btf(cospi[48], o(53), -cospi[16], o(42), cos_bit);
    step[54] = half_btf(cospi[48], o(54), -cospi[16], o(41), cos_bit);
    step[55] = half_btf(cospi[48], o(55), -cospi[16], o(40), cos_bit);
    step[56] = half_btf(cospi[16], o(56), cospi[48], o(39), cos_bit);
    step[57] = half_btf(cospi[16], o(57), cospi[48], o(38), cos_bit);
    step[58] = half_btf(cospi[16], o(58), cospi[48], o(37), cos_bit);
    step[59] = half_btf(cospi[16], o(59), cospi[48], o(36), cos_bit);
    step[60] = o(60);
    step[61] = o(61);
    step[62] = o(62);
    step[63] = o(63);

    // stage 5
    let s = |i: usize| -> i32 { step[i] };
    output[0] = s(0) + s(3);
    output[1] = s(1) + s(2);
    output[2] = -s(2) + s(1);
    output[3] = -s(3) + s(0);
    output[4] = s(4);
    output[5] = half_btf(-cospi[32], s(5), cospi[32], s(6), cos_bit);
    output[6] = half_btf(cospi[32], s(6), cospi[32], s(5), cos_bit);
    output[7] = s(7);
    output[8] = s(8) + s(11);
    output[9] = s(9) + s(10);
    output[10] = -s(10) + s(9);
    output[11] = -s(11) + s(8);
    output[12] = -s(12) + s(15);
    output[13] = -s(13) + s(14);
    output[14] = s(14) + s(13);
    output[15] = s(15) + s(12);
    output[16] = s(16);
    output[17] = s(17);
    output[18] = half_btf(-cospi[16], s(18), cospi[48], s(29), cos_bit);
    output[19] = half_btf(-cospi[16], s(19), cospi[48], s(28), cos_bit);
    output[20] = half_btf(-cospi[48], s(20), -cospi[16], s(27), cos_bit);
    output[21] = half_btf(-cospi[48], s(21), -cospi[16], s(26), cos_bit);
    output[22] = s(22);
    output[23] = s(23);
    output[24] = s(24);
    output[25] = s(25);
    output[26] = half_btf(cospi[48], s(26), -cospi[16], s(21), cos_bit);
    output[27] = half_btf(cospi[48], s(27), -cospi[16], s(20), cos_bit);
    output[28] = half_btf(cospi[16], s(28), cospi[48], s(19), cos_bit);
    output[29] = half_btf(cospi[16], s(29), cospi[48], s(18), cos_bit);
    output[30] = s(30);
    output[31] = s(31);
    output[32] = s(32) + s(39);
    output[33] = s(33) + s(38);
    output[34] = s(34) + s(37);
    output[35] = s(35) + s(36);
    output[36] = -s(36) + s(35);
    output[37] = -s(37) + s(34);
    output[38] = -s(38) + s(33);
    output[39] = -s(39) + s(32);
    output[40] = -s(40) + s(47);
    output[41] = -s(41) + s(46);
    output[42] = -s(42) + s(45);
    output[43] = -s(43) + s(44);
    output[44] = s(44) + s(43);
    output[45] = s(45) + s(42);
    output[46] = s(46) + s(41);
    output[47] = s(47) + s(40);
    output[48] = s(48) + s(55);
    output[49] = s(49) + s(54);
    output[50] = s(50) + s(53);
    output[51] = s(51) + s(52);
    output[52] = -s(52) + s(51);
    output[53] = -s(53) + s(50);
    output[54] = -s(54) + s(49);
    output[55] = -s(55) + s(48);
    output[56] = -s(56) + s(63);
    output[57] = -s(57) + s(62);
    output[58] = -s(58) + s(61);
    output[59] = -s(59) + s(60);
    output[60] = s(60) + s(59);
    output[61] = s(61) + s(58);
    output[62] = s(62) + s(57);
    output[63] = s(63) + s(56);

    // stage 6
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
    step[16] = o(16) + o(19);
    step[17] = o(17) + o(18);
    step[18] = -o(18) + o(17);
    step[19] = -o(19) + o(16);
    step[20] = -o(20) + o(23);
    step[21] = -o(21) + o(22);
    step[22] = o(22) + o(21);
    step[23] = o(23) + o(20);
    step[24] = o(24) + o(27);
    step[25] = o(25) + o(26);
    step[26] = -o(26) + o(25);
    step[27] = -o(27) + o(24);
    step[28] = -o(28) + o(31);
    step[29] = -o(29) + o(30);
    step[30] = o(30) + o(29);
    step[31] = o(31) + o(28);
    step[32] = o(32);
    step[33] = o(33);
    step[34] = half_btf(-cospi[8], o(34), cospi[56], o(61), cos_bit);
    step[35] = half_btf(-cospi[8], o(35), cospi[56], o(60), cos_bit);
    step[36] = half_btf(-cospi[56], o(36), -cospi[8], o(59), cos_bit);
    step[37] = half_btf(-cospi[56], o(37), -cospi[8], o(58), cos_bit);
    step[38] = o(38);
    step[39] = o(39);
    step[40] = o(40);
    step[41] = o(41);
    step[42] = half_btf(-cospi[40], o(42), cospi[24], o(53), cos_bit);
    step[43] = half_btf(-cospi[40], o(43), cospi[24], o(52), cos_bit);
    step[44] = half_btf(-cospi[24], o(44), -cospi[40], o(51), cos_bit);
    step[45] = half_btf(-cospi[24], o(45), -cospi[40], o(50), cos_bit);
    step[46] = o(46);
    step[47] = o(47);
    step[48] = o(48);
    step[49] = o(49);
    step[50] = half_btf(cospi[24], o(50), -cospi[40], o(45), cos_bit);
    step[51] = half_btf(cospi[24], o(51), -cospi[40], o(44), cos_bit);
    step[52] = half_btf(cospi[40], o(52), cospi[24], o(43), cos_bit);
    step[53] = half_btf(cospi[40], o(53), cospi[24], o(42), cos_bit);
    step[54] = o(54);
    step[55] = o(55);
    step[56] = o(56);
    step[57] = o(57);
    step[58] = half_btf(cospi[56], o(58), -cospi[8], o(37), cos_bit);
    step[59] = half_btf(cospi[56], o(59), -cospi[8], o(36), cos_bit);
    step[60] = half_btf(cospi[8], o(60), cospi[56], o(35), cos_bit);
    step[61] = half_btf(cospi[8], o(61), cospi[56], o(34), cos_bit);
    step[62] = o(62);
    step[63] = o(63);

    // stage 7
    let s = |i: usize| -> i32 { step[i] };
    output[0] = s(0);
    output[1] = s(1);
    output[2] = s(2);
    output[3] = s(3);
    output[4] = half_btf(cospi[56], s(4), cospi[8], s(7), cos_bit);
    output[5] = half_btf(cospi[24], s(5), cospi[40], s(6), cos_bit);
    output[6] = half_btf(cospi[24], s(6), -cospi[40], s(5), cos_bit);
    output[7] = half_btf(cospi[56], s(7), -cospi[8], s(4), cos_bit);
    output[8] = s(8) + s(9);
    output[9] = -s(9) + s(8);
    output[10] = -s(10) + s(11);
    output[11] = s(11) + s(10);
    output[12] = s(12) + s(13);
    output[13] = -s(13) + s(12);
    output[14] = -s(14) + s(15);
    output[15] = s(15) + s(14);
    output[16] = s(16);
    output[17] = half_btf(-cospi[8], s(17), cospi[56], s(30), cos_bit);
    output[18] = half_btf(-cospi[56], s(18), -cospi[8], s(29), cos_bit);
    output[19] = s(19);
    output[20] = s(20);
    output[21] = half_btf(-cospi[40], s(21), cospi[24], s(26), cos_bit);
    output[22] = half_btf(-cospi[24], s(22), -cospi[40], s(25), cos_bit);
    output[23] = s(23);
    output[24] = s(24);
    output[25] = half_btf(cospi[24], s(25), -cospi[40], s(22), cos_bit);
    output[26] = half_btf(cospi[40], s(26), cospi[24], s(21), cos_bit);
    output[27] = s(27);
    output[28] = s(28);
    output[29] = half_btf(cospi[56], s(29), -cospi[8], s(18), cos_bit);
    output[30] = half_btf(cospi[8], s(30), cospi[56], s(17), cos_bit);
    output[31] = s(31);
    output[32] = s(32) + s(35);
    output[33] = s(33) + s(34);
    output[34] = -s(34) + s(33);
    output[35] = -s(35) + s(32);
    output[36] = -s(36) + s(39);
    output[37] = -s(37) + s(38);
    output[38] = s(38) + s(37);
    output[39] = s(39) + s(36);
    output[40] = s(40) + s(43);
    output[41] = s(41) + s(42);
    output[42] = -s(42) + s(41);
    output[43] = -s(43) + s(40);
    output[44] = -s(44) + s(47);
    output[45] = -s(45) + s(46);
    output[46] = s(46) + s(45);
    output[47] = s(47) + s(44);
    output[48] = s(48) + s(51);
    output[49] = s(49) + s(50);
    output[50] = -s(50) + s(49);
    output[51] = -s(51) + s(48);
    output[52] = -s(52) + s(55);
    output[53] = -s(53) + s(54);
    output[54] = s(54) + s(53);
    output[55] = s(55) + s(52);
    output[56] = s(56) + s(59);
    output[57] = s(57) + s(58);
    output[58] = -s(58) + s(57);
    output[59] = -s(59) + s(56);
    output[60] = -s(60) + s(63);
    output[61] = -s(61) + s(62);
    output[62] = s(62) + s(61);
    output[63] = s(63) + s(60);

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
    step[8] = half_btf(cospi[60], o(8), cospi[4], o(15), cos_bit);
    step[9] = half_btf(cospi[28], o(9), cospi[36], o(14), cos_bit);
    step[10] = half_btf(cospi[44], o(10), cospi[20], o(13), cos_bit);
    step[11] = half_btf(cospi[12], o(11), cospi[52], o(12), cos_bit);
    step[12] = half_btf(cospi[12], o(12), -cospi[52], o(11), cos_bit);
    step[13] = half_btf(cospi[44], o(13), -cospi[20], o(10), cos_bit);
    step[14] = half_btf(cospi[28], o(14), -cospi[36], o(9), cos_bit);
    step[15] = half_btf(cospi[60], o(15), -cospi[4], o(8), cos_bit);
    step[16] = o(16) + o(17);
    step[17] = -o(17) + o(16);
    step[18] = -o(18) + o(19);
    step[19] = o(19) + o(18);
    step[20] = o(20) + o(21);
    step[21] = -o(21) + o(20);
    step[22] = -o(22) + o(23);
    step[23] = o(23) + o(22);
    step[24] = o(24) + o(25);
    step[25] = -o(25) + o(24);
    step[26] = -o(26) + o(27);
    step[27] = o(27) + o(26);
    step[28] = o(28) + o(29);
    step[29] = -o(29) + o(28);
    step[30] = -o(30) + o(31);
    step[31] = o(31) + o(30);
    step[32] = o(32);
    step[33] = half_btf(-cospi[4], o(33), cospi[60], o(62), cos_bit);
    step[34] = half_btf(-cospi[60], o(34), -cospi[4], o(61), cos_bit);
    step[35] = o(35);
    step[36] = o(36);
    step[37] = half_btf(-cospi[36], o(37), cospi[28], o(58), cos_bit);
    step[38] = half_btf(-cospi[28], o(38), -cospi[36], o(57), cos_bit);
    step[39] = o(39);
    step[40] = o(40);
    step[41] = half_btf(-cospi[20], o(41), cospi[44], o(54), cos_bit);
    step[42] = half_btf(-cospi[44], o(42), -cospi[20], o(53), cos_bit);
    step[43] = o(43);
    step[44] = o(44);
    step[45] = half_btf(-cospi[52], o(45), cospi[12], o(50), cos_bit);
    step[46] = half_btf(-cospi[12], o(46), -cospi[52], o(49), cos_bit);
    step[47] = o(47);
    step[48] = o(48);
    step[49] = half_btf(cospi[12], o(49), -cospi[52], o(46), cos_bit);
    step[50] = half_btf(cospi[52], o(50), cospi[12], o(45), cos_bit);
    step[51] = o(51);
    step[52] = o(52);
    step[53] = half_btf(cospi[44], o(53), -cospi[20], o(42), cos_bit);
    step[54] = half_btf(cospi[20], o(54), cospi[44], o(41), cos_bit);
    step[55] = o(55);
    step[56] = o(56);
    step[57] = half_btf(cospi[28], o(57), -cospi[36], o(38), cos_bit);
    step[58] = half_btf(cospi[36], o(58), cospi[28], o(37), cos_bit);
    step[59] = o(59);
    step[60] = o(60);
    step[61] = half_btf(cospi[60], o(61), -cospi[4], o(34), cos_bit);
    step[62] = half_btf(cospi[4], o(62), cospi[60], o(33), cos_bit);
    step[63] = o(63);

    // stage 9
    let s = |i: usize| -> i32 { step[i] };
    output[0] = s(0);
    output[1] = s(1);
    output[2] = s(2);
    output[3] = s(3);
    output[4] = s(4);
    output[5] = s(5);
    output[6] = s(6);
    output[7] = s(7);
    output[8] = s(8);
    output[9] = s(9);
    output[10] = s(10);
    output[11] = s(11);
    output[12] = s(12);
    output[13] = s(13);
    output[14] = s(14);
    output[15] = s(15);
    output[16] = half_btf(cospi[62], s(16), cospi[2], s(31), cos_bit);
    output[17] = half_btf(cospi[30], s(17), cospi[34], s(30), cos_bit);
    output[18] = half_btf(cospi[46], s(18), cospi[18], s(29), cos_bit);
    output[19] = half_btf(cospi[14], s(19), cospi[50], s(28), cos_bit);
    output[20] = half_btf(cospi[54], s(20), cospi[10], s(27), cos_bit);
    output[21] = half_btf(cospi[22], s(21), cospi[42], s(26), cos_bit);
    output[22] = half_btf(cospi[38], s(22), cospi[26], s(25), cos_bit);
    output[23] = half_btf(cospi[6], s(23), cospi[58], s(24), cos_bit);
    output[24] = half_btf(cospi[6], s(24), -cospi[58], s(23), cos_bit);
    output[25] = half_btf(cospi[38], s(25), -cospi[26], s(22), cos_bit);
    output[26] = half_btf(cospi[22], s(26), -cospi[42], s(21), cos_bit);
    output[27] = half_btf(cospi[54], s(27), -cospi[10], s(20), cos_bit);
    output[28] = half_btf(cospi[14], s(28), -cospi[50], s(19), cos_bit);
    output[29] = half_btf(cospi[46], s(29), -cospi[18], s(18), cos_bit);
    output[30] = half_btf(cospi[30], s(30), -cospi[34], s(17), cos_bit);
    output[31] = half_btf(cospi[62], s(31), -cospi[2], s(16), cos_bit);
    output[32] = s(32) + s(33);
    output[33] = -s(33) + s(32);
    output[34] = -s(34) + s(35);
    output[35] = s(35) + s(34);
    output[36] = s(36) + s(37);
    output[37] = -s(37) + s(36);
    output[38] = -s(38) + s(39);
    output[39] = s(39) + s(38);
    output[40] = s(40) + s(41);
    output[41] = -s(41) + s(40);
    output[42] = -s(42) + s(43);
    output[43] = s(43) + s(42);
    output[44] = s(44) + s(45);
    output[45] = -s(45) + s(44);
    output[46] = -s(46) + s(47);
    output[47] = s(47) + s(46);
    output[48] = s(48) + s(49);
    output[49] = -s(49) + s(48);
    output[50] = -s(50) + s(51);
    output[51] = s(51) + s(50);
    output[52] = s(52) + s(53);
    output[53] = -s(53) + s(52);
    output[54] = -s(54) + s(55);
    output[55] = s(55) + s(54);
    output[56] = s(56) + s(57);
    output[57] = -s(57) + s(56);
    output[58] = -s(58) + s(59);
    output[59] = s(59) + s(58);
    output[60] = s(60) + s(61);
    output[61] = -s(61) + s(60);
    output[62] = -s(62) + s(63);
    output[63] = s(63) + s(62);

    // stage 10
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
    step[16] = o(16);
    step[17] = o(17);
    step[18] = o(18);
    step[19] = o(19);
    step[20] = o(20);
    step[21] = o(21);
    step[22] = o(22);
    step[23] = o(23);
    step[24] = o(24);
    step[25] = o(25);
    step[26] = o(26);
    step[27] = o(27);
    step[28] = o(28);
    step[29] = o(29);
    step[30] = o(30);
    step[31] = o(31);
    step[32] = half_btf(cospi[63], o(32), cospi[1], o(63), cos_bit);
    step[33] = half_btf(cospi[31], o(33), cospi[33], o(62), cos_bit);
    step[34] = half_btf(cospi[47], o(34), cospi[17], o(61), cos_bit);
    step[35] = half_btf(cospi[15], o(35), cospi[49], o(60), cos_bit);
    step[36] = half_btf(cospi[55], o(36), cospi[9], o(59), cos_bit);
    step[37] = half_btf(cospi[23], o(37), cospi[41], o(58), cos_bit);
    step[38] = half_btf(cospi[39], o(38), cospi[25], o(57), cos_bit);
    step[39] = half_btf(cospi[7], o(39), cospi[57], o(56), cos_bit);
    step[40] = half_btf(cospi[59], o(40), cospi[5], o(55), cos_bit);
    step[41] = half_btf(cospi[27], o(41), cospi[37], o(54), cos_bit);
    step[42] = half_btf(cospi[43], o(42), cospi[21], o(53), cos_bit);
    step[43] = half_btf(cospi[11], o(43), cospi[53], o(52), cos_bit);
    step[44] = half_btf(cospi[51], o(44), cospi[13], o(51), cos_bit);
    step[45] = half_btf(cospi[19], o(45), cospi[45], o(50), cos_bit);
    step[46] = half_btf(cospi[35], o(46), cospi[29], o(49), cos_bit);
    step[47] = half_btf(cospi[3], o(47), cospi[61], o(48), cos_bit);
    step[48] = half_btf(cospi[3], o(48), -cospi[61], o(47), cos_bit);
    step[49] = half_btf(cospi[35], o(49), -cospi[29], o(46), cos_bit);
    step[50] = half_btf(cospi[19], o(50), -cospi[45], o(45), cos_bit);
    step[51] = half_btf(cospi[51], o(51), -cospi[13], o(44), cos_bit);
    step[52] = half_btf(cospi[11], o(52), -cospi[53], o(43), cos_bit);
    step[53] = half_btf(cospi[43], o(53), -cospi[21], o(42), cos_bit);
    step[54] = half_btf(cospi[27], o(54), -cospi[37], o(41), cos_bit);
    step[55] = half_btf(cospi[59], o(55), -cospi[5], o(40), cos_bit);
    step[56] = half_btf(cospi[7], o(56), -cospi[57], o(39), cos_bit);
    step[57] = half_btf(cospi[39], o(57), -cospi[25], o(38), cos_bit);
    step[58] = half_btf(cospi[23], o(58), -cospi[41], o(37), cos_bit);
    step[59] = half_btf(cospi[55], o(59), -cospi[9], o(36), cos_bit);
    step[60] = half_btf(cospi[15], o(60), -cospi[49], o(35), cos_bit);
    step[61] = half_btf(cospi[47], o(61), -cospi[17], o(34), cos_bit);
    step[62] = half_btf(cospi[31], o(62), -cospi[33], o(33), cos_bit);
    step[63] = half_btf(cospi[63], o(63), -cospi[1], o(32), cos_bit);

    // stage 11 (output permutation)
    output[0] = step[0];
    output[1] = step[32];
    output[2] = step[16];
    output[3] = step[48];
    output[4] = step[8];
    output[5] = step[40];
    output[6] = step[24];
    output[7] = step[56];
    output[8] = step[4];
    output[9] = step[36];
    output[10] = step[20];
    output[11] = step[52];
    output[12] = step[12];
    output[13] = step[44];
    output[14] = step[28];
    output[15] = step[60];
    output[16] = step[2];
    output[17] = step[34];
    output[18] = step[18];
    output[19] = step[50];
    output[20] = step[10];
    output[21] = step[42];
    output[22] = step[26];
    output[23] = step[58];
    output[24] = step[6];
    output[25] = step[38];
    output[26] = step[22];
    output[27] = step[54];
    output[28] = step[14];
    output[29] = step[46];
    output[30] = step[30];
    output[31] = step[62];
    output[32] = step[1];
    output[33] = step[33];
    output[34] = step[17];
    output[35] = step[49];
    output[36] = step[9];
    output[37] = step[41];
    output[38] = step[25];
    output[39] = step[57];
    output[40] = step[5];
    output[41] = step[37];
    output[42] = step[21];
    output[43] = step[53];
    output[44] = step[13];
    output[45] = step[45];
    output[46] = step[29];
    output[47] = step[61];
    output[48] = step[3];
    output[49] = step[35];
    output[50] = step[19];
    output[51] = step[51];
    output[52] = step[11];
    output[53] = step[43];
    output[54] = step[27];
    output[55] = step[59];
    output[56] = step[7];
    output[57] = step[39];
    output[58] = step[23];
    output[59] = step[55];
    output[60] = step[15];
    output[61] = step[47];
    output[62] = step[31];
    output[63] = step[63];
}

// =============================================================================
// 64-point identity transform
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
        (2, 4) => Some(fadst4), // FLIPADST uses ADST with flipped input
        (2, 8) => Some(fadst8),
        (2, 16) => Some(fadst16),
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
    input: &[TranLow],
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
    let mut buf = vec![0i32; w * h];
    let mut temp_in = vec![0i32; h];
    let mut temp_out = vec![0i32; h];
    // get_rect_tx_log_ratio(col, row)
    let rect_log_ratio = w.trailing_zeros() as i32 - h.trailing_zeros() as i32;

    // Columns
    for c in 0..w {
        if !ud_flip {
            for r in 0..h {
                temp_in[r] = input[r * input_stride + c];
            }
        } else {
            for r in 0..h {
                // flip upside down
                temp_in[r] = input[(h - r - 1) * input_stride + c];
            }
        }
        round_shift_array(&mut temp_in, -(shift[0] as i32));
        col_func(&temp_in, &mut temp_out, cos_bit_col);
        round_shift_array(&mut temp_out, -(shift[1] as i32));
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
    let mut row_out = vec![0i32; w];
    for r in 0..h {
        row_func(&buf[r * w..r * w + w], &mut row_out, cos_bit_row);
        round_shift_array(&mut row_out, -(shift[2] as i32));
        if rect_log_ratio.abs() == 1 {
            // Multiply everything by Sqrt2 if the transform is rectangular
            // and the size difference is a factor of 2.
            for v in row_out.iter_mut() {
                *v = round_shift_i64(*v as i64 * NEW_SQRT2 as i64, NEW_SQRT2_BITS);
            }
        }
        output[r * w..r * w + w].copy_from_slice(&row_out);
    }
}

/// Configured C-exact forward 2D transform (svt_av1_transform_two_d semantics).
///
/// `col_1d`/`row_1d`: 0=DCT, 1=ADST, 2=FLIPADST, 3=IDENTITY. Returns false if
/// the (type, size) combination has no 1D kernel (e.g. ADST on 32/64 dims).
#[allow(clippy::too_many_arguments)]
pub fn fwd_txfm2d_c_exact(
    input: &[TranLow],
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
pub fn fwd_txfm2d_64x64_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    fwd_txfm2d_c_exact(input, output, stride, 64, 64, 0, 0, false, false);
}

/// Forward 4x8 DCT-DCT (rectangular).
pub fn fwd_txfm2d_4x8_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    fwd_txfm2d_c_exact(input, output, stride, 4, 8, 0, 0, false, false);
}

/// Forward 8x4 DCT-DCT (rectangular).
pub fn fwd_txfm2d_8x4_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    fwd_txfm2d_c_exact(input, output, stride, 8, 4, 0, 0, false, false);
}

/// Forward 8x16 DCT-DCT (rectangular).
pub fn fwd_txfm2d_8x16_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    fwd_txfm2d_c_exact(input, output, stride, 8, 16, 0, 0, false, false);
}

/// Forward 16x8 DCT-DCT (rectangular).
pub fn fwd_txfm2d_16x8_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    fwd_txfm2d_c_exact(input, output, stride, 16, 8, 0, 0, false, false);
}

/// Forward 16x32 DCT-DCT (rectangular).
pub fn fwd_txfm2d_16x32_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    fwd_txfm2d_c_exact(input, output, stride, 16, 32, 0, 0, false, false);
}

/// Forward 32x16 DCT-DCT (rectangular).
pub fn fwd_txfm2d_32x16_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    fwd_txfm2d_c_exact(input, output, stride, 32, 16, 0, 0, false, false);
}

/// Forward 32x64 DCT-DCT (rectangular).
pub fn fwd_txfm2d_32x64_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    fwd_txfm2d_c_exact(input, output, stride, 32, 64, 0, 0, false, false);
}

/// Forward 64x32 DCT-DCT (rectangular).
pub fn fwd_txfm2d_64x32_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    fwd_txfm2d_c_exact(input, output, stride, 64, 32, 0, 0, false, false);
}

/// Forward 4x16 DCT-DCT (4:1 rectangular).
pub fn fwd_txfm2d_4x16_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    fwd_txfm2d_c_exact(input, output, stride, 4, 16, 0, 0, false, false);
}

/// Forward 16x4 DCT-DCT (4:1 rectangular).
pub fn fwd_txfm2d_16x4_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    fwd_txfm2d_c_exact(input, output, stride, 16, 4, 0, 0, false, false);
}

/// Forward 8x32 DCT-DCT (4:1 rectangular).
pub fn fwd_txfm2d_8x32_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    fwd_txfm2d_c_exact(input, output, stride, 8, 32, 0, 0, false, false);
}

/// Forward 32x8 DCT-DCT (4:1 rectangular).
pub fn fwd_txfm2d_32x8_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    fwd_txfm2d_c_exact(input, output, stride, 32, 8, 0, 0, false, false);
}

/// Forward 16x64 DCT-DCT (4:1 rectangular).
pub fn fwd_txfm2d_16x64_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    fwd_txfm2d_c_exact(input, output, stride, 16, 64, 0, 0, false, false);
}

/// Forward 64x16 DCT-DCT (4:1 rectangular).
pub fn fwd_txfm2d_64x16_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    fwd_txfm2d_c_exact(input, output, stride, 64, 16, 0, 0, false, false);
}

/// Forward 4x4 DCT-DCT using the general framework.
pub fn fwd_txfm2d_4x4_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    incant!(
        fwd_txfm2d_4x4_dct_dct_impl(input, output, stride),
        [v3, neon, scalar]
    )
}

fn fwd_txfm2d_4x4_dct_dct_impl_scalar(
    _token: ScalarToken,
    input: &[TranLow],
    output: &mut [TranLow],
    stride: usize,
) {
    fwd_txfm2d_c_exact(input, output, stride, 4, 4, 0, 0, false, false);
}

#[cfg(target_arch = "x86_64")]
#[arcane]
fn fwd_txfm2d_4x4_dct_dct_impl_v3(
    _token: Desktop64,
    input: &[TranLow],
    output: &mut [TranLow],
    stride: usize,
) {
    fwd_txfm2d_c_exact(input, output, stride, 4, 4, 0, 0, false, false);
}

#[cfg(target_arch = "aarch64")]
#[arcane]
fn fwd_txfm2d_4x4_dct_dct_impl_neon(
    _token: NeonToken,
    input: &[TranLow],
    output: &mut [TranLow],
    stride: usize,
) {
    fwd_txfm2d_c_exact(input, output, stride, 4, 4, 0, 0, false, false);
}

/// Forward 8x8 DCT-DCT.
pub fn fwd_txfm2d_8x8_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    incant!(
        fwd_txfm2d_8x8_dct_dct_impl(input, output, stride),
        [v3, neon, scalar]
    )
}

fn fwd_txfm2d_8x8_dct_dct_impl_scalar(
    _token: ScalarToken,
    input: &[TranLow],
    output: &mut [TranLow],
    stride: usize,
) {
    fwd_txfm2d_c_exact(input, output, stride, 8, 8, 0, 0, false, false);
}

#[cfg(target_arch = "x86_64")]
#[arcane]
fn fwd_txfm2d_8x8_dct_dct_impl_v3(
    _token: Desktop64,
    input: &[TranLow],
    output: &mut [TranLow],
    stride: usize,
) {
    fwd_txfm2d_c_exact(input, output, stride, 8, 8, 0, 0, false, false);
}

#[cfg(target_arch = "aarch64")]
#[arcane]
fn fwd_txfm2d_8x8_dct_dct_impl_neon(
    _token: NeonToken,
    input: &[TranLow],
    output: &mut [TranLow],
    stride: usize,
) {
    fwd_txfm2d_c_exact(input, output, stride, 8, 8, 0, 0, false, false);
}

/// Forward 16x16 DCT-DCT.
pub fn fwd_txfm2d_16x16_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    incant!(
        fwd_txfm2d_16x16_dct_dct_impl(input, output, stride),
        [v3, neon, scalar]
    )
}

fn fwd_txfm2d_16x16_dct_dct_impl_scalar(
    _token: ScalarToken,
    input: &[TranLow],
    output: &mut [TranLow],
    stride: usize,
) {
    fwd_txfm2d_c_exact(input, output, stride, 16, 16, 0, 0, false, false);
}

#[cfg(target_arch = "x86_64")]
#[arcane]
fn fwd_txfm2d_16x16_dct_dct_impl_v3(
    _token: Desktop64,
    input: &[TranLow],
    output: &mut [TranLow],
    stride: usize,
) {
    fwd_txfm2d_c_exact(input, output, stride, 16, 16, 0, 0, false, false);
}

#[cfg(target_arch = "aarch64")]
#[arcane]
fn fwd_txfm2d_16x16_dct_dct_impl_neon(
    _token: NeonToken,
    input: &[TranLow],
    output: &mut [TranLow],
    stride: usize,
) {
    fwd_txfm2d_c_exact(input, output, stride, 16, 16, 0, 0, false, false);
}

/// Forward 32x32 DCT-DCT.
pub fn fwd_txfm2d_32x32_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    incant!(
        fwd_txfm2d_32x32_dct_dct_impl(input, output, stride),
        [v3, neon, scalar]
    )
}

fn fwd_txfm2d_32x32_dct_dct_impl_scalar(
    _token: ScalarToken,
    input: &[TranLow],
    output: &mut [TranLow],
    stride: usize,
) {
    fwd_txfm2d_c_exact(input, output, stride, 32, 32, 0, 0, false, false);
}

#[cfg(target_arch = "x86_64")]
#[arcane]
fn fwd_txfm2d_32x32_dct_dct_impl_v3(
    _token: Desktop64,
    input: &[TranLow],
    output: &mut [TranLow],
    stride: usize,
) {
    fwd_txfm2d_c_exact(input, output, stride, 32, 32, 0, 0, false, false);
}

#[cfg(target_arch = "aarch64")]
#[arcane]
fn fwd_txfm2d_32x32_dct_dct_impl_neon(
    _token: NeonToken,
    input: &[TranLow],
    output: &mut [TranLow],
    stride: usize,
) {
    fwd_txfm2d_c_exact(input, output, stride, 32, 32, 0, 0, false, false);
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- fdct4 tests ---

    #[test]
    fn fdct4_dc_input() {
        let input = [100i32; 4];
        let mut output = [0i32; 4];
        fdct4(&input, &mut output, 12);
        assert!(output[0].abs() > 0, "DC should be nonzero");
        for i in 1..4 {
            assert!(output[i].abs() <= 1, "AC[{i}] = {}", output[i]);
        }
    }

    #[test]
    fn fdct4_zero() {
        let input = [0i32; 4];
        let mut output = [0i32; 4];
        fdct4(&input, &mut output, 12);
        assert!(output.iter().all(|&v| v == 0));
    }

    // --- fdct8 tests ---

    #[test]
    fn fdct8_dc_input() {
        let input = [100i32; 8];
        let mut output = [0i32; 8];
        fdct8(&input, &mut output, 12);
        assert!(output[0].abs() > 0, "DC should be nonzero");
        for i in 1..8 {
            assert!(output[i].abs() <= 1, "AC[{i}] = {}", output[i]);
        }
    }

    #[test]
    fn fdct8_zero() {
        let mut output = [0i32; 8];
        fdct8(&[0i32; 8], &mut output, 12);
        assert!(output.iter().all(|&v| v == 0));
    }

    #[test]
    fn fdct8_alternating() {
        // Alternating +1/-1 should produce energy in higher frequencies
        let input = [1, -1, 1, -1, 1, -1, 1, -1i32];
        let mut output = [0i32; 8];
        fdct8(&input, &mut output, 12);
        // DC should be 0 (equal positive and negative)
        assert_eq!(output[0], 0);
        // Some AC coefficients should be nonzero
        assert!(output.iter().any(|&v| v != 0));
    }

    // --- fdct16 tests ---

    #[test]
    fn fdct16_dc_input() {
        let input = [50i32; 16];
        let mut output = [0i32; 16];
        fdct16(&input, &mut output, 12);
        assert!(output[0].abs() > 0, "DC should be nonzero");
        for i in 1..16 {
            assert!(output[i].abs() <= 1, "AC[{i}] = {}", output[i]);
        }
    }

    #[test]
    fn fdct16_zero() {
        let mut output = [0i32; 16];
        fdct16(&[0i32; 16], &mut output, 12);
        assert!(output.iter().all(|&v| v == 0));
    }

    // --- fdct32 tests ---

    #[test]
    fn fdct32_dc_input() {
        let input = [100i32; 32];
        let mut output = [0i32; 32];
        fdct32(&input, &mut output, 12);
        assert!(output[0].abs() > 0, "DC should be nonzero");
        for i in 1..32 {
            assert!(output[i].abs() <= 1, "AC[{i}] = {}", output[i]);
        }
    }

    #[test]
    fn fdct32_zero() {
        let mut output = [0i32; 32];
        fdct32(&[0i32; 32], &mut output, 12);
        assert!(output.iter().all(|&v| v == 0));
    }

    // --- fadst tests ---

    #[test]
    fn fadst4_zero() {
        let mut output = [0i32; 4];
        fadst4(&[0i32; 4], &mut output, 12);
        assert!(output.iter().all(|&v| v == 0));
    }

    #[test]
    fn fadst8_zero() {
        let mut output = [0i32; 8];
        fadst8(&[0i32; 8], &mut output, 12);
        assert!(output.iter().all(|&v| v == 0));
    }

    // --- identity tests ---

    #[test]
    fn fidentity4_ratio() {
        let input = [10i32, 20, 30, 40];
        let mut output = [0i32; 4];
        fidentity4(&input, &mut output, 12);
        for v in &output {
            assert!(*v != 0);
        }
        let ratio = output[1] as f64 / output[0] as f64;
        assert!((ratio - 2.0).abs() < 0.01, "ratio = {ratio}");
    }

    #[test]
    fn fidentity8_scale() {
        let input = [100i32; 8];
        let mut output = [0i32; 8];
        fidentity8(&input, &mut output, 12);
        // Should be 200 (scaled by 2)
        assert!(output.iter().all(|&v| v == 200));
    }

    // --- 2D transform tests ---

    #[test]
    fn fwd_txfm2d_4x4_dc() {
        let input = [100i32; 16];
        let mut output = [0i32; 16];
        fwd_txfm2d_4x4_dct_dct(&input, &mut output, 4);
        assert!(output[0].abs() > 0, "DC should be nonzero");
        for i in 1..16 {
            assert!(
                output[i].abs() <= 2,
                "AC[{i}] = {} should be ~0 for DC input",
                output[i]
            );
        }
    }

    #[test]
    fn fwd_txfm2d_8x8_dc() {
        let input = [50i32; 64];
        let mut output = [0i32; 64];
        fwd_txfm2d_8x8_dct_dct(&input, &mut output, 8);
        assert!(output[0].abs() > 0, "DC should be nonzero");
        for i in 1..64 {
            assert!(
                output[i].abs() <= 2,
                "8x8 AC[{i}] = {} should be ~0 for DC input",
                output[i]
            );
        }
    }

    #[test]
    fn fwd_txfm2d_16x16_dc() {
        let input = [30i32; 256];
        let mut output = [0i32; 256];
        fwd_txfm2d_16x16_dct_dct(&input, &mut output, 16);
        assert!(output[0].abs() > 0, "DC should be nonzero");
        for i in 1..256 {
            assert!(
                output[i].abs() <= 2,
                "16x16 AC[{i}] = {} should be ~0 for DC input",
                output[i]
            );
        }
    }

    #[test]
    fn fwd_txfm2d_4x4_zero() {
        let mut output = [0i32; 16];
        fwd_txfm2d_4x4_dct_dct(&[0i32; 16], &mut output, 4);
        assert!(output.iter().all(|&v| v == 0));
    }

    // --- half_btf tests ---

    #[test]
    fn half_btf_identity() {
        // half_btf(1*4096, x, 0, 0, 12) should approximately equal x
        let result = half_btf(4096, 1000, 0, 0, 12);
        assert_eq!(result, 1000);
    }

    #[test]
    fn round_shift_basic() {
        assert_eq!(round_shift(100, 0), 100);
        assert_eq!(round_shift(100, 1), 50);
        assert_eq!(round_shift(7, 1), 4); // (7 + 1) >> 1 = 4
        assert_eq!(round_shift(5, 1), 3); // (5 + 1) >> 1 = 3
    }
}

#[cfg(test)]
mod dispatch_tests {
    use super::*;
    use archmage::testing::{CompileTimePolicy, for_each_token_permutation};

    #[test]
    fn fwd_txfm2d_4x4_dct_dct_all_dispatch_levels() {
        let input: [i32; 16] = [
            10, -20, 30, -40, 50, -60, 70, -80, 15, -25, 35, -45, 55, -65, 75, -85,
        ];
        let mut reference = [0i32; 16];
        fwd_txfm2d_4x4_dct_dct(&input, &mut reference, 4);

        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            let mut result = [0i32; 16];
            fwd_txfm2d_4x4_dct_dct(&input, &mut result, 4);
            assert_eq!(
                result, reference,
                "4x4 DCT mismatch at dispatch level {_perm}"
            );
        });
    }

    #[test]
    fn fwd_txfm2d_8x8_dct_dct_all_dispatch_levels() {
        let mut input = [0i32; 64];
        for (i, v) in input.iter_mut().enumerate() {
            *v = (i as i32 * 7 - 30) % 100;
        }
        let mut reference = [0i32; 64];
        fwd_txfm2d_8x8_dct_dct(&input, &mut reference, 8);

        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            let mut result = [0i32; 64];
            fwd_txfm2d_8x8_dct_dct(&input, &mut result, 8);
            assert_eq!(
                result, reference,
                "8x8 DCT mismatch at dispatch level {_perm}"
            );
        });
    }
}
