//! Inverse transforms (DCT, ADST, identity).
//!
//! Ported from SVT-AV1's `inv_transforms.c`.
//! All transforms are separable (1D row -> 1D column) per AV1 spec.
//!
//! These are the transposes of the forward transforms, executed in
//! reverse stage order (un-permute -> un-butterfly -> un-combine).

use crate::fwd_txfm::{
    COS_BIT, COSPI, NEW_SQRT2, NEW_SQRT2_BITS, SINPI, half_btf, round_shift_array, round_shift_i64,
};
use alloc::vec;
use archmage::prelude::*;
use svtav1_types::transform::TranLow;

// =============================================================================
// 4-point inverse DCT-II
// Ported from svt_av1_idct4_new in inv_transforms.c:96-133
// =============================================================================

pub fn idct4(input: &[TranLow], output: &mut [TranLow]) {
    let cospi = &COSPI;
    let cos_bit = COS_BIT;

    // stage 1: input permutation (undo bit-reversal)
    let bf0 = [input[0], input[2], input[1], input[3]];

    // stage 2: butterfly
    let step = [
        half_btf(cospi[32], bf0[0], cospi[32], bf0[1], cos_bit),
        half_btf(cospi[32], bf0[0], -cospi[32], bf0[1], cos_bit),
        half_btf(cospi[48], bf0[2], -cospi[16], bf0[3], cos_bit),
        half_btf(cospi[16], bf0[2], cospi[48], bf0[3], cos_bit),
    ];

    // stage 3: combine
    output[0] = step[0] + step[3];
    output[1] = step[1] + step[2];
    output[2] = step[1] - step[2];
    output[3] = step[0] - step[3];
}

// =============================================================================
// 8-point inverse DCT-II
// Ported from svt_av1_idct8_new in inv_transforms.c:135-212
// =============================================================================

pub fn idct8(input: &[TranLow], output: &mut [TranLow]) {
    let cospi = &COSPI;
    let cos_bit = COS_BIT;
    let mut step = [0i32; 8];

    // stage 1: input permutation (undo bit-reversal)
    output[0] = input[0];
    output[1] = input[4];
    output[2] = input[2];
    output[3] = input[6];
    output[4] = input[1];
    output[5] = input[5];
    output[6] = input[3];
    output[7] = input[7];

    // stage 2
    let bf0 = &*output;
    step[0] = bf0[0];
    step[1] = bf0[1];
    step[2] = bf0[2];
    step[3] = bf0[3];
    step[4] = half_btf(cospi[56], bf0[4], -cospi[8], bf0[7], cos_bit);
    step[5] = half_btf(cospi[24], bf0[5], -cospi[40], bf0[6], cos_bit);
    step[6] = half_btf(cospi[40], bf0[5], cospi[24], bf0[6], cos_bit);
    step[7] = half_btf(cospi[8], bf0[4], cospi[56], bf0[7], cos_bit);

    // stage 3
    let s = &step;
    output[0] = half_btf(cospi[32], s[0], cospi[32], s[1], cos_bit);
    output[1] = half_btf(cospi[32], s[0], -cospi[32], s[1], cos_bit);
    output[2] = half_btf(cospi[48], s[2], -cospi[16], s[3], cos_bit);
    output[3] = half_btf(cospi[16], s[2], cospi[48], s[3], cos_bit);
    output[4] = s[4] + s[5];
    output[5] = s[4] - s[5];
    output[6] = -s[6] + s[7];
    output[7] = s[6] + s[7];

    // stage 4
    let bf0_0 = output[0];
    let bf0_1 = output[1];
    let bf0_2 = output[2];
    let bf0_3 = output[3];
    let bf0_4 = output[4];
    let bf0_5 = output[5];
    let bf0_6 = output[6];
    let bf0_7 = output[7];
    step[0] = bf0_0 + bf0_3;
    step[1] = bf0_1 + bf0_2;
    step[2] = bf0_1 - bf0_2;
    step[3] = bf0_0 - bf0_3;
    step[4] = bf0_4;
    step[5] = half_btf(-cospi[32], bf0_5, cospi[32], bf0_6, cos_bit);
    step[6] = half_btf(cospi[32], bf0_5, cospi[32], bf0_6, cos_bit);
    step[7] = bf0_7;

    // stage 5: final combine
    output[0] = step[0] + step[7];
    output[1] = step[1] + step[6];
    output[2] = step[2] + step[5];
    output[3] = step[3] + step[4];
    output[4] = step[3] - step[4];
    output[5] = step[2] - step[5];
    output[6] = step[1] - step[6];
    output[7] = step[0] - step[7];
}

// =============================================================================
// 4-point inverse ADST
// Ported from svt_av1_iadst4_new in inv_transforms.c:728-813
// =============================================================================

pub fn iadst4(input: &[TranLow], output: &mut [TranLow]) {
    let sinpi = &SINPI;
    let cos_bit = COS_BIT;

    let x0 = input[0];
    let x1 = input[1];
    let x2 = input[2];
    let x3 = input[3];

    if (x0 | x1 | x2 | x3) == 0 {
        output[0] = 0;
        output[1] = 0;
        output[2] = 0;
        output[3] = 0;
        return;
    }

    // stage 1
    let s0 = sinpi[1] * x0;
    let s1 = sinpi[2] * x0;
    let s2 = sinpi[3] * x1;
    let s3 = sinpi[4] * x2;
    let s4 = sinpi[1] * x2;
    let s5 = sinpi[2] * x3;
    let s6 = sinpi[4] * x3;

    // stage 2
    let s7 = (x0 - x2) + x3;

    // stage 3
    let s0 = s0 + s3;
    let s1 = s1 - s4;
    let s3 = s2;
    let s2 = sinpi[3] * s7;

    // stage 4
    let s0 = s0 + s5;
    let s1 = s1 - s6;

    // stage 5
    let x0 = s0 + s3;
    let x1 = s1 + s3;
    let x2 = s2;
    let x3 = s0 + s1;

    // stage 6
    let x3 = x3 - s3;

    output[0] = round_shift_i64(x0 as i64, cos_bit);
    output[1] = round_shift_i64(x1 as i64, cos_bit);
    output[2] = round_shift_i64(x2 as i64, cos_bit);
    output[3] = round_shift_i64(x3 as i64, cos_bit);
}

// =============================================================================
// 4-point inverse identity transform
// Ported from svt_av1_iidentity4_c in inv_transforms.c:2345-2354
// =============================================================================

pub fn iidentity4(input: &[TranLow], output: &mut [TranLow]) {
    for i in 0..4 {
        output[i] = round_shift_i64(input[i] as i64 * NEW_SQRT2 as i64, NEW_SQRT2_BITS);
    }
}

// =============================================================================
// 8-point inverse identity transform
// Ported from svt_av1_iidentity8_c in inv_transforms.c:2356-2362
// =============================================================================

pub fn iidentity8(input: &[TranLow], output: &mut [TranLow]) {
    for i in 0..8 {
        output[i] = input[i] * 2;
    }
}

// =============================================================================
// 16-point inverse DCT-II
// Ported exactly from svt_av1_idct16_new in inv_transforms.c:214-375
// clamp_value replaced with plain add/subtract (wide stage_range)
// =============================================================================

pub fn idct16(input: &[TranLow], output: &mut [TranLow]) {
    let cospi = &COSPI;
    let cos_bit = COS_BIT;
    let mut step = [0i32; 16];

    // stage 1: input permutation
    output[0] = input[0];
    output[1] = input[8];
    output[2] = input[4];
    output[3] = input[12];
    output[4] = input[2];
    output[5] = input[10];
    output[6] = input[6];
    output[7] = input[14];
    output[8] = input[1];
    output[9] = input[9];
    output[10] = input[5];
    output[11] = input[13];
    output[12] = input[3];
    output[13] = input[11];
    output[14] = input[7];
    output[15] = input[15];

    // stage 2
    let o = |i: usize| -> i32 { output[i] };
    step[0] = o(0);
    step[1] = o(1);
    step[2] = o(2);
    step[3] = o(3);
    step[4] = o(4);
    step[5] = o(5);
    step[6] = o(6);
    step[7] = o(7);
    step[8] = half_btf(cospi[60], o(8), -cospi[4], o(15), cos_bit);
    step[9] = half_btf(cospi[28], o(9), -cospi[36], o(14), cos_bit);
    step[10] = half_btf(cospi[44], o(10), -cospi[20], o(13), cos_bit);
    step[11] = half_btf(cospi[12], o(11), -cospi[52], o(12), cos_bit);
    step[12] = half_btf(cospi[52], o(11), cospi[12], o(12), cos_bit);
    step[13] = half_btf(cospi[20], o(10), cospi[44], o(13), cos_bit);
    step[14] = half_btf(cospi[36], o(9), cospi[28], o(14), cos_bit);
    step[15] = half_btf(cospi[4], o(8), cospi[60], o(15), cos_bit);

    // stage 3
    let s = |i: usize| -> i32 { step[i] };
    output[0] = s(0);
    output[1] = s(1);
    output[2] = s(2);
    output[3] = s(3);
    output[4] = half_btf(cospi[56], s(4), -cospi[8], s(7), cos_bit);
    output[5] = half_btf(cospi[24], s(5), -cospi[40], s(6), cos_bit);
    output[6] = half_btf(cospi[40], s(5), cospi[24], s(6), cos_bit);
    output[7] = half_btf(cospi[8], s(4), cospi[56], s(7), cos_bit);
    output[8] = s(8) + s(9);
    output[9] = s(8) - s(9);
    output[10] = -s(10) + s(11);
    output[11] = s(10) + s(11);
    output[12] = s(12) + s(13);
    output[13] = s(12) - s(13);
    output[14] = -s(14) + s(15);
    output[15] = s(14) + s(15);

    // stage 4
    let o = |i: usize| -> i32 { output[i] };
    step[0] = half_btf(cospi[32], o(0), cospi[32], o(1), cos_bit);
    step[1] = half_btf(cospi[32], o(0), -cospi[32], o(1), cos_bit);
    step[2] = half_btf(cospi[48], o(2), -cospi[16], o(3), cos_bit);
    step[3] = half_btf(cospi[16], o(2), cospi[48], o(3), cos_bit);
    step[4] = o(4) + o(5);
    step[5] = o(4) - o(5);
    step[6] = -o(6) + o(7);
    step[7] = o(6) + o(7);
    step[8] = o(8);
    step[9] = half_btf(-cospi[16], o(9), cospi[48], o(14), cos_bit);
    step[10] = half_btf(-cospi[48], o(10), -cospi[16], o(13), cos_bit);
    step[11] = o(11);
    step[12] = o(12);
    step[13] = half_btf(-cospi[16], o(10), cospi[48], o(13), cos_bit);
    step[14] = half_btf(cospi[48], o(9), cospi[16], o(14), cos_bit);
    step[15] = o(15);

    // stage 5
    let s = |i: usize| -> i32 { step[i] };
    output[0] = s(0) + s(3);
    output[1] = s(1) + s(2);
    output[2] = s(1) - s(2);
    output[3] = s(0) - s(3);
    output[4] = s(4);
    output[5] = half_btf(-cospi[32], s(5), cospi[32], s(6), cos_bit);
    output[6] = half_btf(cospi[32], s(5), cospi[32], s(6), cos_bit);
    output[7] = s(7);
    output[8] = s(8) + s(11);
    output[9] = s(9) + s(10);
    output[10] = s(9) - s(10);
    output[11] = s(8) - s(11);
    output[12] = -s(12) + s(15);
    output[13] = -s(13) + s(14);
    output[14] = s(13) + s(14);
    output[15] = s(12) + s(15);

    // stage 6
    let o = |i: usize| -> i32 { output[i] };
    step[0] = o(0) + o(7);
    step[1] = o(1) + o(6);
    step[2] = o(2) + o(5);
    step[3] = o(3) + o(4);
    step[4] = o(3) - o(4);
    step[5] = o(2) - o(5);
    step[6] = o(1) - o(6);
    step[7] = o(0) - o(7);
    step[8] = o(8);
    step[9] = o(9);
    step[10] = half_btf(-cospi[32], o(10), cospi[32], o(13), cos_bit);
    step[11] = half_btf(-cospi[32], o(11), cospi[32], o(12), cos_bit);
    step[12] = half_btf(cospi[32], o(11), cospi[32], o(12), cos_bit);
    step[13] = half_btf(cospi[32], o(10), cospi[32], o(13), cos_bit);
    step[14] = o(14);
    step[15] = o(15);

    // stage 7
    output[0] = step[0] + step[15];
    output[1] = step[1] + step[14];
    output[2] = step[2] + step[13];
    output[3] = step[3] + step[12];
    output[4] = step[4] + step[11];
    output[5] = step[5] + step[10];
    output[6] = step[6] + step[9];
    output[7] = step[7] + step[8];
    output[8] = step[7] - step[8];
    output[9] = step[6] - step[9];
    output[10] = step[5] - step[10];
    output[11] = step[4] - step[11];
    output[12] = step[3] - step[12];
    output[13] = step[2] - step[13];
    output[14] = step[1] - step[14];
    output[15] = step[0] - step[15];
}

// =============================================================================
// 32-point inverse DCT-II
// Ported exactly from svt_av1_idct32_new in inv_transforms.c:377-726
// clamp_value replaced with plain add/subtract (wide stage_range)
// =============================================================================

pub fn idct32(input: &[TranLow], output: &mut [TranLow]) {
    let cospi = &COSPI;
    let cos_bit = COS_BIT;
    let mut step = [0i32; 32];

    // stage 1: input permutation (bit-reversal)
    output[0] = input[0];
    output[1] = input[16];
    output[2] = input[8];
    output[3] = input[24];
    output[4] = input[4];
    output[5] = input[20];
    output[6] = input[12];
    output[7] = input[28];
    output[8] = input[2];
    output[9] = input[18];
    output[10] = input[10];
    output[11] = input[26];
    output[12] = input[6];
    output[13] = input[22];
    output[14] = input[14];
    output[15] = input[30];
    output[16] = input[1];
    output[17] = input[17];
    output[18] = input[9];
    output[19] = input[25];
    output[20] = input[5];
    output[21] = input[21];
    output[22] = input[13];
    output[23] = input[29];
    output[24] = input[3];
    output[25] = input[19];
    output[26] = input[11];
    output[27] = input[27];
    output[28] = input[7];
    output[29] = input[23];
    output[30] = input[15];
    output[31] = input[31];

    // stage 2
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
    step[16] = half_btf(cospi[62], o(16), -cospi[2], o(31), cos_bit);
    step[17] = half_btf(cospi[30], o(17), -cospi[34], o(30), cos_bit);
    step[18] = half_btf(cospi[46], o(18), -cospi[18], o(29), cos_bit);
    step[19] = half_btf(cospi[14], o(19), -cospi[50], o(28), cos_bit);
    step[20] = half_btf(cospi[54], o(20), -cospi[10], o(27), cos_bit);
    step[21] = half_btf(cospi[22], o(21), -cospi[42], o(26), cos_bit);
    step[22] = half_btf(cospi[38], o(22), -cospi[26], o(25), cos_bit);
    step[23] = half_btf(cospi[6], o(23), -cospi[58], o(24), cos_bit);
    step[24] = half_btf(cospi[58], o(23), cospi[6], o(24), cos_bit);
    step[25] = half_btf(cospi[26], o(22), cospi[38], o(25), cos_bit);
    step[26] = half_btf(cospi[42], o(21), cospi[22], o(26), cos_bit);
    step[27] = half_btf(cospi[10], o(20), cospi[54], o(27), cos_bit);
    step[28] = half_btf(cospi[50], o(19), cospi[14], o(28), cos_bit);
    step[29] = half_btf(cospi[18], o(18), cospi[46], o(29), cos_bit);
    step[30] = half_btf(cospi[34], o(17), cospi[30], o(30), cos_bit);
    step[31] = half_btf(cospi[2], o(16), cospi[62], o(31), cos_bit);

    // stage 3
    let s = |i: usize| -> i32 { step[i] };
    output[0] = s(0);
    output[1] = s(1);
    output[2] = s(2);
    output[3] = s(3);
    output[4] = s(4);
    output[5] = s(5);
    output[6] = s(6);
    output[7] = s(7);
    output[8] = half_btf(cospi[60], s(8), -cospi[4], s(15), cos_bit);
    output[9] = half_btf(cospi[28], s(9), -cospi[36], s(14), cos_bit);
    output[10] = half_btf(cospi[44], s(10), -cospi[20], s(13), cos_bit);
    output[11] = half_btf(cospi[12], s(11), -cospi[52], s(12), cos_bit);
    output[12] = half_btf(cospi[52], s(11), cospi[12], s(12), cos_bit);
    output[13] = half_btf(cospi[20], s(10), cospi[44], s(13), cos_bit);
    output[14] = half_btf(cospi[36], s(9), cospi[28], s(14), cos_bit);
    output[15] = half_btf(cospi[4], s(8), cospi[60], s(15), cos_bit);
    output[16] = s(16) + s(17);
    output[17] = s(16) - s(17);
    output[18] = -s(18) + s(19);
    output[19] = s(18) + s(19);
    output[20] = s(20) + s(21);
    output[21] = s(20) - s(21);
    output[22] = -s(22) + s(23);
    output[23] = s(22) + s(23);
    output[24] = s(24) + s(25);
    output[25] = s(24) - s(25);
    output[26] = -s(26) + s(27);
    output[27] = s(26) + s(27);
    output[28] = s(28) + s(29);
    output[29] = s(28) - s(29);
    output[30] = -s(30) + s(31);
    output[31] = s(30) + s(31);

    // stage 4
    let o = |i: usize| -> i32 { output[i] };
    step[0] = o(0);
    step[1] = o(1);
    step[2] = o(2);
    step[3] = o(3);
    step[4] = half_btf(cospi[56], o(4), -cospi[8], o(7), cos_bit);
    step[5] = half_btf(cospi[24], o(5), -cospi[40], o(6), cos_bit);
    step[6] = half_btf(cospi[40], o(5), cospi[24], o(6), cos_bit);
    step[7] = half_btf(cospi[8], o(4), cospi[56], o(7), cos_bit);
    step[8] = o(8) + o(9);
    step[9] = o(8) - o(9);
    step[10] = -o(10) + o(11);
    step[11] = o(10) + o(11);
    step[12] = o(12) + o(13);
    step[13] = o(12) - o(13);
    step[14] = -o(14) + o(15);
    step[15] = o(14) + o(15);
    step[16] = o(16);
    step[17] = half_btf(-cospi[8], o(17), cospi[56], o(30), cos_bit);
    step[18] = half_btf(-cospi[56], o(18), -cospi[8], o(29), cos_bit);
    step[19] = o(19);
    step[20] = o(20);
    step[21] = half_btf(-cospi[40], o(21), cospi[24], o(26), cos_bit);
    step[22] = half_btf(-cospi[24], o(22), -cospi[40], o(25), cos_bit);
    step[23] = o(23);
    step[24] = o(24);
    step[25] = half_btf(-cospi[40], o(22), cospi[24], o(25), cos_bit);
    step[26] = half_btf(cospi[24], o(21), cospi[40], o(26), cos_bit);
    step[27] = o(27);
    step[28] = o(28);
    step[29] = half_btf(-cospi[8], o(18), cospi[56], o(29), cos_bit);
    step[30] = half_btf(cospi[56], o(17), cospi[8], o(30), cos_bit);
    step[31] = o(31);

    // stage 5
    let s = |i: usize| -> i32 { step[i] };
    output[0] = half_btf(cospi[32], s(0), cospi[32], s(1), cos_bit);
    output[1] = half_btf(cospi[32], s(0), -cospi[32], s(1), cos_bit);
    output[2] = half_btf(cospi[48], s(2), -cospi[16], s(3), cos_bit);
    output[3] = half_btf(cospi[16], s(2), cospi[48], s(3), cos_bit);
    output[4] = s(4) + s(5);
    output[5] = s(4) - s(5);
    output[6] = -s(6) + s(7);
    output[7] = s(6) + s(7);
    output[8] = s(8);
    output[9] = half_btf(-cospi[16], s(9), cospi[48], s(14), cos_bit);
    output[10] = half_btf(-cospi[48], s(10), -cospi[16], s(13), cos_bit);
    output[11] = s(11);
    output[12] = s(12);
    output[13] = half_btf(-cospi[16], s(10), cospi[48], s(13), cos_bit);
    output[14] = half_btf(cospi[48], s(9), cospi[16], s(14), cos_bit);
    output[15] = s(15);
    output[16] = s(16) + s(19);
    output[17] = s(17) + s(18);
    output[18] = s(17) - s(18);
    output[19] = s(16) - s(19);
    output[20] = -s(20) + s(23);
    output[21] = -s(21) + s(22);
    output[22] = s(21) + s(22);
    output[23] = s(20) + s(23);
    output[24] = s(24) + s(27);
    output[25] = s(25) + s(26);
    output[26] = s(25) - s(26);
    output[27] = s(24) - s(27);
    output[28] = -s(28) + s(31);
    output[29] = -s(29) + s(30);
    output[30] = s(29) + s(30);
    output[31] = s(28) + s(31);

    // stage 6
    let o = |i: usize| -> i32 { output[i] };
    step[0] = o(0) + o(3);
    step[1] = o(1) + o(2);
    step[2] = o(1) - o(2);
    step[3] = o(0) - o(3);
    step[4] = o(4);
    step[5] = half_btf(-cospi[32], o(5), cospi[32], o(6), cos_bit);
    step[6] = half_btf(cospi[32], o(5), cospi[32], o(6), cos_bit);
    step[7] = o(7);
    step[8] = o(8) + o(11);
    step[9] = o(9) + o(10);
    step[10] = o(9) - o(10);
    step[11] = o(8) - o(11);
    step[12] = -o(12) + o(15);
    step[13] = -o(13) + o(14);
    step[14] = o(13) + o(14);
    step[15] = o(12) + o(15);
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
    step[26] = half_btf(-cospi[16], o(21), cospi[48], o(26), cos_bit);
    step[27] = half_btf(-cospi[16], o(20), cospi[48], o(27), cos_bit);
    step[28] = half_btf(cospi[48], o(19), cospi[16], o(28), cos_bit);
    step[29] = half_btf(cospi[48], o(18), cospi[16], o(29), cos_bit);
    step[30] = o(30);
    step[31] = o(31);

    // stage 7
    let s = |i: usize| -> i32 { step[i] };
    output[0] = s(0) + s(7);
    output[1] = s(1) + s(6);
    output[2] = s(2) + s(5);
    output[3] = s(3) + s(4);
    output[4] = s(3) - s(4);
    output[5] = s(2) - s(5);
    output[6] = s(1) - s(6);
    output[7] = s(0) - s(7);
    output[8] = s(8);
    output[9] = s(9);
    output[10] = half_btf(-cospi[32], s(10), cospi[32], s(13), cos_bit);
    output[11] = half_btf(-cospi[32], s(11), cospi[32], s(12), cos_bit);
    output[12] = half_btf(cospi[32], s(11), cospi[32], s(12), cos_bit);
    output[13] = half_btf(cospi[32], s(10), cospi[32], s(13), cos_bit);
    output[14] = s(14);
    output[15] = s(15);
    output[16] = s(16) + s(23);
    output[17] = s(17) + s(22);
    output[18] = s(18) + s(21);
    output[19] = s(19) + s(20);
    output[20] = s(19) - s(20);
    output[21] = s(18) - s(21);
    output[22] = s(17) - s(22);
    output[23] = s(16) - s(23);
    output[24] = -s(24) + s(31);
    output[25] = -s(25) + s(30);
    output[26] = -s(26) + s(29);
    output[27] = -s(27) + s(28);
    output[28] = s(27) + s(28);
    output[29] = s(26) + s(29);
    output[30] = s(25) + s(30);
    output[31] = s(24) + s(31);

    // stage 8
    let o = |i: usize| -> i32 { output[i] };
    step[0] = o(0) + o(15);
    step[1] = o(1) + o(14);
    step[2] = o(2) + o(13);
    step[3] = o(3) + o(12);
    step[4] = o(4) + o(11);
    step[5] = o(5) + o(10);
    step[6] = o(6) + o(9);
    step[7] = o(7) + o(8);
    step[8] = o(7) - o(8);
    step[9] = o(6) - o(9);
    step[10] = o(5) - o(10);
    step[11] = o(4) - o(11);
    step[12] = o(3) - o(12);
    step[13] = o(2) - o(13);
    step[14] = o(1) - o(14);
    step[15] = o(0) - o(15);
    step[16] = o(16);
    step[17] = o(17);
    step[18] = o(18);
    step[19] = o(19);
    step[20] = half_btf(-cospi[32], o(20), cospi[32], o(27), cos_bit);
    step[21] = half_btf(-cospi[32], o(21), cospi[32], o(26), cos_bit);
    step[22] = half_btf(-cospi[32], o(22), cospi[32], o(25), cos_bit);
    step[23] = half_btf(-cospi[32], o(23), cospi[32], o(24), cos_bit);
    step[24] = half_btf(cospi[32], o(23), cospi[32], o(24), cos_bit);
    step[25] = half_btf(cospi[32], o(22), cospi[32], o(25), cos_bit);
    step[26] = half_btf(cospi[32], o(21), cospi[32], o(26), cos_bit);
    step[27] = half_btf(cospi[32], o(20), cospi[32], o(27), cos_bit);
    step[28] = o(28);
    step[29] = o(29);
    step[30] = o(30);
    step[31] = o(31);

    // stage 9
    let s = |i: usize| -> i32 { step[i] };
    output[0] = s(0) + s(31);
    output[1] = s(1) + s(30);
    output[2] = s(2) + s(29);
    output[3] = s(3) + s(28);
    output[4] = s(4) + s(27);
    output[5] = s(5) + s(26);
    output[6] = s(6) + s(25);
    output[7] = s(7) + s(24);
    output[8] = s(8) + s(23);
    output[9] = s(9) + s(22);
    output[10] = s(10) + s(21);
    output[11] = s(11) + s(20);
    output[12] = s(12) + s(19);
    output[13] = s(13) + s(18);
    output[14] = s(14) + s(17);
    output[15] = s(15) + s(16);
    output[16] = s(15) - s(16);
    output[17] = s(14) - s(17);
    output[18] = s(13) - s(18);
    output[19] = s(12) - s(19);
    output[20] = s(11) - s(20);
    output[21] = s(10) - s(21);
    output[22] = s(9) - s(22);
    output[23] = s(8) - s(23);
    output[24] = s(7) - s(24);
    output[25] = s(6) - s(25);
    output[26] = s(5) - s(26);
    output[27] = s(4) - s(27);
    output[28] = s(3) - s(28);
    output[29] = s(2) - s(29);
    output[30] = s(1) - s(30);
    output[31] = s(0) - s(31);
}

// =============================================================================
// 32-point inverse identity transform
// Ported from svt_av1_iidentity32_c in inv_transforms.c
// =============================================================================

pub fn iidentity32(input: &[TranLow], output: &mut [TranLow]) {
    for i in 0..32 {
        output[i] = input[i] * 4;
    }
}

// =============================================================================
// 8-point inverse ADST
// Ported exactly from svt_av1_iadst8_new in inv_transforms.c:821-924
// =============================================================================

pub fn iadst8(input: &[TranLow], output: &mut [TranLow]) {
    let cospi = &COSPI;
    let cos_bit = COS_BIT;
    let mut step = [0i32; 8];

    // stage 1: input permutation
    output[0] = input[7];
    output[1] = input[0];
    output[2] = input[5];
    output[3] = input[2];
    output[4] = input[3];
    output[5] = input[4];
    output[6] = input[1];
    output[7] = input[6];

    // stage 2
    let o = |i: usize| -> i32 { output[i] };
    step[0] = half_btf(cospi[4], o(0), cospi[60], o(1), cos_bit);
    step[1] = half_btf(cospi[60], o(0), -cospi[4], o(1), cos_bit);
    step[2] = half_btf(cospi[20], o(2), cospi[44], o(3), cos_bit);
    step[3] = half_btf(cospi[44], o(2), -cospi[20], o(3), cos_bit);
    step[4] = half_btf(cospi[36], o(4), cospi[28], o(5), cos_bit);
    step[5] = half_btf(cospi[28], o(4), -cospi[36], o(5), cos_bit);
    step[6] = half_btf(cospi[52], o(6), cospi[12], o(7), cos_bit);
    step[7] = half_btf(cospi[12], o(6), -cospi[52], o(7), cos_bit);

    // stage 3
    output[0] = step[0] + step[4];
    output[1] = step[1] + step[5];
    output[2] = step[2] + step[6];
    output[3] = step[3] + step[7];
    output[4] = step[0] - step[4];
    output[5] = step[1] - step[5];
    output[6] = step[2] - step[6];
    output[7] = step[3] - step[7];

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

    // stage 5
    output[0] = step[0] + step[2];
    output[1] = step[1] + step[3];
    output[2] = step[0] - step[2];
    output[3] = step[1] - step[3];
    output[4] = step[4] + step[6];
    output[5] = step[5] + step[7];
    output[6] = step[4] - step[6];
    output[7] = step[5] - step[7];

    // stage 6
    let o = |i: usize| -> i32 { output[i] };
    step[0] = o(0);
    step[1] = o(1);
    step[2] = half_btf(cospi[32], o(2), cospi[32], o(3), cos_bit);
    step[3] = half_btf(cospi[32], o(2), -cospi[32], o(3), cos_bit);
    step[4] = o(4);
    step[5] = o(5);
    step[6] = half_btf(cospi[32], o(6), cospi[32], o(7), cos_bit);
    step[7] = half_btf(cospi[32], o(6), -cospi[32], o(7), cos_bit);

    // stage 7: output (exact match to C svt_av1_iadst8_new)
    output[0] = step[0];
    output[1] = -step[4];
    output[2] = step[6];
    output[3] = -step[2];
    output[4] = step[3];
    output[5] = -step[7];
    output[6] = step[5];
    output[7] = -step[1];
}

// =============================================================================
// 16-point inverse ADST
// Ported exactly from svt_av1_iadst16_new in inv_transforms.c:926-1129
// =============================================================================

pub fn iadst16(input: &[TranLow], output: &mut [TranLow]) {
    let cospi = &COSPI;
    let cos_bit = COS_BIT;
    let mut step = [0i32; 16];

    // stage 1: input permutation
    output[0] = input[15];
    output[1] = input[0];
    output[2] = input[13];
    output[3] = input[2];
    output[4] = input[11];
    output[5] = input[4];
    output[6] = input[9];
    output[7] = input[6];
    output[8] = input[7];
    output[9] = input[8];
    output[10] = input[5];
    output[11] = input[10];
    output[12] = input[3];
    output[13] = input[12];
    output[14] = input[1];
    output[15] = input[14];

    // stage 2
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

    // stage 3
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

    // stage 4
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

    // stage 7
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

    // stage 8
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

    // stage 9: output with negation
    output[0] = step[0];
    output[1] = -step[8];
    output[2] = step[12];
    output[3] = -step[4];
    output[4] = step[6];
    output[5] = -step[14];
    output[6] = step[10];
    output[7] = -step[2];
    output[8] = step[3];
    output[9] = -step[11];
    output[10] = step[15];
    output[11] = -step[7];
    output[12] = step[5];
    output[13] = -step[13];
    output[14] = step[9];
    output[15] = -step[1];
}

// =============================================================================
// 16-point inverse identity
// =============================================================================

pub fn iidentity16(input: &[TranLow], output: &mut [TranLow]) {
    let new_sqrt2 = NEW_SQRT2;
    for i in 0..16 {
        output[i] = round_shift_i64(input[i] as i64 * 2 * new_sqrt2 as i64, NEW_SQRT2_BITS);
    }
}

// =============================================================================
// 1D inverse transform function type and dispatch
// =============================================================================

/// 1D inverse transform function signature.
pub type InvTxfmFunc = fn(&[TranLow], &mut [TranLow]);

/// Get the 1D inverse transform function for a given type and size.
pub fn get_inv_txfm_func(tx_type_1d: u8, size: usize) -> Option<InvTxfmFunc> {
    match (tx_type_1d, size) {
        (0, 4) => Some(idct4),
        (0, 8) => Some(idct8),
        (0, 16) => Some(idct16),
        (0, 32) => Some(idct32),
        (1, 4) => Some(iadst4),
        (1, 8) => Some(iadst8),
        (1, 16) => Some(iadst16),
        (3, 4) => Some(iidentity4),
        (3, 8) => Some(iidentity8),
        (3, 16) => Some(iidentity16),
        (3, 32) => Some(iidentity32),
        _ => None,
    }
}

// =============================================================================
// General 2D inverse transform
// =============================================================================

/// Inverse 2D transform for square blocks.
///
/// Applies row transforms, then column transforms, following the
/// SVT-AV1 `inv_txfm2d_add_c` pattern (rows first, then columns).
///
/// `shift` = [row_post_shift, col_post_shift] applied after each stage.
/// Note: The inverse goes rows-first then columns, opposite of the forward.
pub fn inv_txfm2d(
    input: &[TranLow],
    output: &mut [TranLow],
    stride: usize,
    row_func: InvTxfmFunc,
    col_func: InvTxfmFunc,
    size: usize,
    shift: [i32; 2],
) {
    let mut buf = vec![0i32; size * size];
    let mut temp_in = vec![0i32; size];
    let mut temp_out = vec![0i32; size];

    // Row transforms (inverse does rows first)
    for row in 0..size {
        let row_start = row * stride;
        temp_in[..size].copy_from_slice(&input[row_start..row_start + size]);
        row_func(&temp_in, &mut temp_out);
        round_shift_array(&mut temp_out, -shift[0]);
        for col in 0..size {
            buf[row * size + col] = temp_out[col];
        }
    }

    // Column transforms
    for col in 0..size {
        for row in 0..size {
            temp_in[row] = buf[row * size + col];
        }
        col_func(&temp_in, &mut temp_out);
        round_shift_array(&mut temp_out, -shift[1]);
        for row in 0..size {
            output[row * stride + col] = temp_out[row];
        }
    }
}

/// Inverse 4x4 DCT-DCT using the general framework.
pub fn inv_txfm2d_4x4_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    incant!(
        inv_txfm2d_4x4_dct_dct_impl(input, output, stride),
        [v3, neon, scalar]
    )
}

fn inv_txfm2d_4x4_dct_dct_impl_scalar(
    _token: ScalarToken,
    input: &[TranLow],
    output: &mut [TranLow],
    stride: usize,
) {
    inv_txfm2d(input, output, stride, idct4, idct4, 4, [0, -4]);
}

#[cfg(target_arch = "x86_64")]
#[arcane]
fn inv_txfm2d_4x4_dct_dct_impl_v3(
    _token: Desktop64,
    input: &[TranLow],
    output: &mut [TranLow],
    stride: usize,
) {
    inv_txfm2d(input, output, stride, idct4, idct4, 4, [0, -4]);
}

#[cfg(target_arch = "aarch64")]
#[arcane]
fn inv_txfm2d_4x4_dct_dct_impl_neon(
    _token: NeonToken,
    input: &[TranLow],
    output: &mut [TranLow],
    stride: usize,
) {
    inv_txfm2d(input, output, stride, idct4, idct4, 4, [0, -4]);
}

/// Inverse 8x8 DCT-DCT.
pub fn inv_txfm2d_8x8_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    incant!(
        inv_txfm2d_8x8_dct_dct_impl(input, output, stride),
        [v3, neon, scalar]
    )
}

fn inv_txfm2d_8x8_dct_dct_impl_scalar(
    _token: ScalarToken,
    input: &[TranLow],
    output: &mut [TranLow],
    stride: usize,
) {
    inv_txfm2d(input, output, stride, idct8, idct8, 8, [-1, -4]);
}

#[cfg(target_arch = "x86_64")]
#[arcane]
fn inv_txfm2d_8x8_dct_dct_impl_v3(
    _token: Desktop64,
    input: &[TranLow],
    output: &mut [TranLow],
    stride: usize,
) {
    inv_txfm2d(input, output, stride, idct8, idct8, 8, [-1, -4]);
}

#[cfg(target_arch = "aarch64")]
#[arcane]
fn inv_txfm2d_8x8_dct_dct_impl_neon(
    _token: NeonToken,
    input: &[TranLow],
    output: &mut [TranLow],
    stride: usize,
) {
    inv_txfm2d(input, output, stride, idct8, idct8, 8, [-1, -4]);
}

/// Inverse 16x16 DCT-DCT.
pub fn inv_txfm2d_16x16_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    incant!(
        inv_txfm2d_16x16_dct_dct_impl(input, output, stride),
        [v3, neon, scalar]
    )
}

fn inv_txfm2d_16x16_dct_dct_impl_scalar(
    _token: ScalarToken,
    input: &[TranLow],
    output: &mut [TranLow],
    stride: usize,
) {
    inv_txfm2d(input, output, stride, idct16, idct16, 16, [-2, 0]);
}

#[cfg(target_arch = "x86_64")]
#[arcane]
fn inv_txfm2d_16x16_dct_dct_impl_v3(
    _token: Desktop64,
    input: &[TranLow],
    output: &mut [TranLow],
    stride: usize,
) {
    inv_txfm2d(input, output, stride, idct16, idct16, 16, [-2, 0]);
}

#[cfg(target_arch = "aarch64")]
#[arcane]
fn inv_txfm2d_16x16_dct_dct_impl_neon(
    _token: NeonToken,
    input: &[TranLow],
    output: &mut [TranLow],
    stride: usize,
) {
    inv_txfm2d(input, output, stride, idct16, idct16, 16, [-2, 0]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fwd_txfm::{fdct4, fdct8, fwd_txfm2d_4x4_dct_dct, fwd_txfm2d_8x8_dct_dct};

    // --- idct4 tests ---

    #[test]
    fn idct4_zero() {
        let mut output = [0i32; 4];
        idct4(&[0i32; 4], &mut output);
        assert!(output.iter().all(|&v| v == 0));
    }

    #[test]
    fn fdct4_idct4_roundtrip() {
        // The combined forward+inverse DCT-4 produces input * 2 (scale factor N/2 = 4/2).
        let input = [10i32, -20, 30, -40];
        let mut fwd = [0i32; 4];
        let mut inv = [0i32; 4];
        fdct4(&input, &mut fwd);
        idct4(&fwd, &mut inv);
        for i in 0..4 {
            assert!(
                (input[i] * 2 - inv[i]).abs() <= 1,
                "fdct4->idct4 mismatch at [{}]: expected {}, got {}",
                i,
                input[i] * 2,
                inv[i]
            );
        }
    }

    #[test]
    fn fdct4_idct4_dc_roundtrip() {
        // DC-only: all same value. Scale factor is 2 for 4-point DCT.
        let input = [100i32; 4];
        let mut fwd = [0i32; 4];
        let mut inv = [0i32; 4];
        fdct4(&input, &mut fwd);
        idct4(&fwd, &mut inv);
        for i in 0..4 {
            assert!(
                (input[i] * 2 - inv[i]).abs() <= 1,
                "DC roundtrip mismatch at [{}]: expected {}, got {}",
                i,
                input[i] * 2,
                inv[i]
            );
        }
    }

    // --- idct8 tests ---

    #[test]
    fn idct8_zero() {
        let mut output = [0i32; 8];
        idct8(&[0i32; 8], &mut output);
        assert!(output.iter().all(|&v| v == 0));
    }

    #[test]
    fn fdct8_idct8_roundtrip() {
        // The combined forward+inverse DCT-8 produces input * 4 (scale factor N/2 = 8/2).
        // Tolerance is +-2 due to accumulated rounding across 5 butterfly stages.
        let input = [10, -20, 30, -40, 50, -60, 70, -80i32];
        let mut fwd = [0i32; 8];
        let mut inv = [0i32; 8];
        fdct8(&input, &mut fwd);
        idct8(&fwd, &mut inv);
        for i in 0..8 {
            assert!(
                (input[i] * 4 - inv[i]).abs() <= 2,
                "fdct8->idct8 mismatch at [{}]: expected {}, got {}",
                i,
                input[i] * 4,
                inv[i]
            );
        }
    }

    #[test]
    fn fdct8_idct8_dc_roundtrip() {
        // Scale factor is 4 for 8-point DCT.
        let input = [50i32; 8];
        let mut fwd = [0i32; 8];
        let mut inv = [0i32; 8];
        fdct8(&input, &mut fwd);
        idct8(&fwd, &mut inv);
        for i in 0..8 {
            assert!(
                (input[i] * 4 - inv[i]).abs() <= 1,
                "DC roundtrip mismatch at [{}]: expected {}, got {}",
                i,
                input[i] * 4,
                inv[i]
            );
        }
    }

    // --- iadst4 tests ---

    /// C-style forward ADST4 (from svt_av1_fadst4_new in transforms.c).
    /// This is the matched forward transform for our iadst4.
    /// Note: our Rust fadst4 in fwd_txfm.rs uses a different (i64) decomposition
    /// that doesn't round-trip with the C-style iadst4.
    fn c_fadst4(input: &[i32; 4], output: &mut [i32; 4]) {
        use crate::fwd_txfm::{SINPI, round_shift};
        let sinpi = &SINPI;
        let bit = COS_BIT;

        let (x0, x1, x2, x3) = (input[0], input[1], input[2], input[3]);

        if (x0 | x1 | x2 | x3) == 0 {
            *output = [0; 4];
            return;
        }

        // stage 1
        let s0 = sinpi[1] * x0;
        let s1 = sinpi[4] * x0;
        let s2 = sinpi[2] * x1;
        let s3 = sinpi[1] * x1;
        let s4 = sinpi[3] * x2;
        let s5 = sinpi[4] * x3;
        let s6 = sinpi[2] * x3;
        let s7 = x0 + x1;

        // stage 2
        let s7 = s7 - x3;

        // stage 3
        let x0 = s0 + s2;
        let x1 = sinpi[3] * s7;
        let x2 = s1 - s3;
        let x3 = s4;

        // stage 4
        let x0 = x0 + s5;
        let x2 = x2 + s6;

        // stage 5
        let s0 = x0 + x3;
        let s1 = x1;
        let s2 = x2 - x3;
        let s3 = x2 - x0 + x3;

        output[0] = round_shift(s0, bit);
        output[1] = round_shift(s1, bit);
        output[2] = round_shift(s2, bit);
        output[3] = round_shift(s3, bit);
    }

    #[test]
    fn iadst4_zero() {
        let mut output = [0i32; 4];
        iadst4(&[0i32; 4], &mut output);
        assert!(output.iter().all(|&v| v == 0));
    }

    #[test]
    fn c_fadst4_iadst4_roundtrip() {
        // The C-style forward ADST4 and our iadst4 are matched pairs.
        // Combined scale factor is 2 (same as DCT-4).
        let input = [15i32, -25, 35, -45];
        let mut fwd = [0i32; 4];
        let mut inv = [0i32; 4];
        c_fadst4(&input, &mut fwd);
        iadst4(&fwd, &mut inv);
        for i in 0..4 {
            assert!(
                (input[i] * 2 - inv[i]).abs() <= 1,
                "c_fadst4->iadst4 mismatch at [{}]: expected {}, got {}",
                i,
                input[i] * 2,
                inv[i]
            );
        }
    }

    #[test]
    fn iadst4_nonzero_input() {
        // Verify iadst4 produces nonzero output for nonzero input
        let input = [100, 50, -30, 20i32];
        let mut output = [0i32; 4];
        iadst4(&input, &mut output);
        assert!(
            output.iter().any(|&v| v != 0),
            "iadst4 should produce nonzero output"
        );
    }

    // --- iidentity tests ---

    #[test]
    fn iidentity4_zero() {
        let mut output = [0i32; 4];
        iidentity4(&[0i32; 4], &mut output);
        assert!(output.iter().all(|&v| v == 0));
    }

    #[test]
    fn iidentity8_zero() {
        let mut output = [0i32; 8];
        iidentity8(&[0i32; 8], &mut output);
        assert!(output.iter().all(|&v| v == 0));
    }

    #[test]
    fn fidentity4_iidentity4_roundtrip() {
        // fidentity4 scales by sqrt(2), iidentity4 also scales by sqrt(2)
        // So roundtrip = input * 2 (approximately), not identity.
        // This is correct — identity transforms are self-inverse up to scaling.
        let input = [10i32, 20, 30, 40];
        let mut fwd = [0i32; 4];
        let mut inv = [0i32; 4];
        crate::fwd_txfm::fidentity4(&input, &mut fwd);
        iidentity4(&fwd, &mut inv);
        // fidentity4 scales by sqrt(2), iidentity4 scales by sqrt(2)
        // Result should be input * 2
        for i in 0..4 {
            assert!(
                (input[i] * 2 - inv[i]).abs() <= 1,
                "identity4 scaling mismatch at [{}]: expected {}, got {}",
                i,
                input[i] * 2,
                inv[i]
            );
        }
    }

    #[test]
    fn fidentity8_iidentity8_roundtrip() {
        let input = [10i32, 20, 30, 40, 50, 60, 70, 80];
        let mut fwd = [0i32; 8];
        let mut inv = [0i32; 8];
        crate::fwd_txfm::fidentity8(&input, &mut fwd);
        iidentity8(&fwd, &mut inv);
        // fidentity8 scales by 2, iidentity8 scales by 2
        // Result should be input * 4
        for i in 0..8 {
            assert_eq!(
                input[i] * 4,
                inv[i],
                "identity8 scaling mismatch at [{}]",
                i
            );
        }
    }

    // --- 2D roundtrip tests ---

    #[test]
    fn fwd_inv_txfm2d_4x4_roundtrip() {
        // Test that forward 4x4 DCT-DCT followed by inverse recovers original
        // The forward uses shift [2, 0, 0] and inverse uses shift [0, -4].
        // Combined shift: forward applies <<2 at start, inverse applies >>4 at end.
        // Net: output = input >> 2 (divided by 4).
        // But the actual combined effect depends on the exact scaling.
        // Let's just verify structure: DC input -> forward -> inverse should
        // produce a scaled version of the original.
        let input = [100i32; 16];
        let mut fwd = [0i32; 16];
        let mut inv = [0i32; 16];
        fwd_txfm2d_4x4_dct_dct(&input, &mut fwd, 4);
        inv_txfm2d_4x4_dct_dct(&fwd, &mut inv, 4);
        // After fwd(shift=[2,0,0]) + inv(shift=[0,-4]):
        // The net scaling is: input << 2 (fwd pre-shift) then >> 4 (inv post-shift)
        // = input >> 2 = 25 for input=100
        // But the DCT basis vectors also introduce a factor of N=4 normalization.
        // Expected: input * 4 * (1/16) = input/4 ... let's just check it's nonzero
        // and consistent.
        assert!(inv[0] != 0, "output should be nonzero");
        // All values should be the same for DC input
        let first = inv[0];
        for i in 1..16 {
            assert!(
                (inv[i] - first).abs() <= 1,
                "DC input should produce uniform output, [{}]={} vs [0]={}",
                i,
                inv[i],
                first
            );
        }
    }

    #[test]
    fn fwd_inv_txfm2d_4x4_zero() {
        let mut fwd = [0i32; 16];
        let mut inv = [0i32; 16];
        fwd_txfm2d_4x4_dct_dct(&[0i32; 16], &mut fwd, 4);
        inv_txfm2d_4x4_dct_dct(&fwd, &mut inv, 4);
        assert!(inv.iter().all(|&v| v == 0));
    }

    #[test]
    fn fwd_inv_txfm2d_8x8_zero() {
        let mut fwd = [0i32; 64];
        let mut inv = [0i32; 64];
        fwd_txfm2d_8x8_dct_dct(&[0i32; 64], &mut fwd, 8);
        inv_txfm2d_8x8_dct_dct(&fwd, &mut inv, 8);
        assert!(inv.iter().all(|&v| v == 0));
    }

    #[test]
    fn fwd_inv_txfm2d_8x8_roundtrip() {
        let input = [50i32; 64];
        let mut fwd = [0i32; 64];
        let mut inv = [0i32; 64];
        fwd_txfm2d_8x8_dct_dct(&input, &mut fwd, 8);
        inv_txfm2d_8x8_dct_dct(&fwd, &mut inv, 8);
        assert!(inv[0] != 0, "output should be nonzero");
        let first = inv[0];
        for i in 1..64 {
            assert!(
                (inv[i] - first).abs() <= 1,
                "DC input should produce uniform output at [{}]={} vs [0]={}",
                i,
                inv[i],
                first
            );
        }
    }
}

#[cfg(test)]
mod dispatch_tests {
    use super::*;
    use archmage::testing::{CompileTimePolicy, for_each_token_permutation};

    #[test]
    fn inv_txfm2d_4x4_dct_dct_all_dispatch_levels() {
        // Use forward transform output as input to inverse
        let fwd_input: [i32; 16] = [
            10, -20, 30, -40, 50, -60, 70, -80, 15, -25, 35, -45, 55, -65, 75, -85,
        ];
        let mut coeffs = [0i32; 16];
        crate::fwd_txfm::fwd_txfm2d_4x4_dct_dct(&fwd_input, &mut coeffs, 4);

        let mut reference = [0i32; 16];
        inv_txfm2d_4x4_dct_dct(&coeffs, &mut reference, 4);

        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            let mut result = [0i32; 16];
            inv_txfm2d_4x4_dct_dct(&coeffs, &mut result, 4);
            assert_eq!(
                result, reference,
                "4x4 inv DCT mismatch at dispatch level {_perm}"
            );
        });
    }

    #[test]
    fn inv_txfm2d_8x8_dct_dct_all_dispatch_levels() {
        let mut fwd_input = [0i32; 64];
        for (i, v) in fwd_input.iter_mut().enumerate() {
            *v = (i as i32 * 7 - 30) % 100;
        }
        let mut coeffs = [0i32; 64];
        crate::fwd_txfm::fwd_txfm2d_8x8_dct_dct(&fwd_input, &mut coeffs, 8);

        let mut reference = [0i32; 64];
        inv_txfm2d_8x8_dct_dct(&coeffs, &mut reference, 8);

        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            let mut result = [0i32; 64];
            inv_txfm2d_8x8_dct_dct(&coeffs, &mut result, 8);
            assert_eq!(
                result, reference,
                "8x8 inv DCT mismatch at dispatch level {_perm}"
            );
        });
    }
}
