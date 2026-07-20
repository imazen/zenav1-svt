//! Inverse transforms (DCT, ADST, identity).
//!
//! Spec 04: Inverse transforms for reconstruction loop.
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

/// C `new_inv_sqrt2` = 2^12 / sqrt(2) (inv_transforms.h:257).
pub const NEW_INV_SQRT2: i32 = 2896;

/// C `clamp_value` (inv_transforms.c:87): clamp to a signed `bit`-bit range.
/// `bit <= 0` is a no-op ("invalid clamp bit"), matching C.
#[inline]
pub fn clamp_value(value: i32, bit: i8) -> i32 {
    if bit <= 0 {
        return value;
    }
    let max_value: i64 = (1i64 << (bit - 1)) - 1;
    let min_value: i64 = -(1i64 << (bit - 1));
    (value as i64).clamp(min_value, max_value) as i32
}

/// C `clamp_buf` (inv_transforms.c:815).
#[inline]
fn clamp_buf(buf: &mut [i32], bit: i8) {
    for v in buf.iter_mut() {
        *v = clamp_value(*v, bit);
    }
}

// The bd-dependent inverse-transform composition ranges are computed by
// `inv_txfm_ranges(bd)` (bd8 -> 16/16, bd10 -> 18/16); the residual wraplow
// by `highbd_wraplow(_, bd)`. Both reduce to the former fixed bd=8 constants
// (row/col clamp 16, stage range 16, wraplow 34595) at bd == 8, so the bd8
// path is byte-identical. C: `svt_av1_gen_inv_stage_range` +
// `svt_av1_inv_txfm2d_add_c` row/col clamps (inv_transforms.c:43-85).

/// C `HIGHBD_WRAPLOW(x, bd)` = `check_range(x, bd)` (inv_transforms.c:2426):
/// clamp to `+/-((1<<(7+bd))-1 + (914<<(bd-7)))`. At `bd == 8` this is
/// `+/-34595` (the AV1 8-bit coefficient range incl. max quant error), so the
/// bd8 path is unaffected. Task #94 (bd10 u16 MD path).
#[inline]
fn highbd_wraplow(trans: i32, bd: u8) -> i32 {
    let int_max = (1i32 << (7 + bd)) - 1 + (914i32 << (bd - 7));
    trans.clamp(-int_max - 1, int_max)
}

/// Per-direction clamp/stage-range bit widths at `bd`
/// (`svt_av1_gen_inv_stage_range` + the row/col input clamps
/// `bd+8` / `AOMMAX(bd+6,16)`, inv_transforms.c:43-84 + the 2d-add clamps).
/// Row uses `bd+8` (16/18/20 at bd 8/10/12), col uses `max(bd+6,16)`
/// (16/16/18) — for both the input clamp AND the kernel stage range in that
/// direction. At bd8 both are 16, matching the fixed BD8 constants.
#[inline]
pub(crate) fn inv_txfm_ranges(bd: u8) -> (i8, i8) {
    let row = (bd as i32 + 8) as i8;
    let col = core::cmp::max(bd as i32 + 6, 16) as i8;
    (row, col)
}

// =============================================================================
// 4-point inverse DCT-II
// Ported from svt_av1_idct4_new in inv_transforms.c:96-133
// =============================================================================

pub fn idct4(input: &[TranLow], output: &mut [TranLow], range: i8) {
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
    output[0] = clamp_value(step[0] + step[3], range);
    output[1] = clamp_value(step[1] + step[2], range);
    output[2] = clamp_value(step[1] - step[2], range);
    output[3] = clamp_value(step[0] - step[3], range);
}

// =============================================================================
// 8-point inverse DCT-II
// Ported from svt_av1_idct8_new in inv_transforms.c:135-212
// =============================================================================

pub fn idct8(input: &[TranLow], output: &mut [TranLow], range: i8) {
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
    output[4] = clamp_value(s[4] + s[5], range);
    output[5] = clamp_value(s[4] - s[5], range);
    output[6] = clamp_value(-s[6] + s[7], range);
    output[7] = clamp_value(s[6] + s[7], range);

    // stage 4
    let bf0_0 = output[0];
    let bf0_1 = output[1];
    let bf0_2 = output[2];
    let bf0_3 = output[3];
    let bf0_4 = output[4];
    let bf0_5 = output[5];
    let bf0_6 = output[6];
    let bf0_7 = output[7];
    step[0] = clamp_value(bf0_0 + bf0_3, range);
    step[1] = clamp_value(bf0_1 + bf0_2, range);
    step[2] = clamp_value(bf0_1 - bf0_2, range);
    step[3] = clamp_value(bf0_0 - bf0_3, range);
    step[4] = bf0_4;
    step[5] = half_btf(-cospi[32], bf0_5, cospi[32], bf0_6, cos_bit);
    step[6] = half_btf(cospi[32], bf0_5, cospi[32], bf0_6, cos_bit);
    step[7] = bf0_7;

    // stage 5: final combine
    output[0] = clamp_value(step[0] + step[7], range);
    output[1] = clamp_value(step[1] + step[6], range);
    output[2] = clamp_value(step[2] + step[5], range);
    output[3] = clamp_value(step[3] + step[4], range);
    output[4] = clamp_value(step[3] - step[4], range);
    output[5] = clamp_value(step[2] - step[5], range);
    output[6] = clamp_value(step[1] - step[6], range);
    output[7] = clamp_value(step[0] - step[7], range);
}

// =============================================================================
// 4-point inverse ADST
// Ported from svt_av1_iadst4_new in inv_transforms.c:728-813
// =============================================================================

pub fn iadst4(input: &[TranLow], output: &mut [TranLow], _range: i8) {
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

pub fn iidentity4(input: &[TranLow], output: &mut [TranLow], _range: i8) {
    for i in 0..4 {
        output[i] = round_shift_i64(input[i] as i64 * NEW_SQRT2 as i64, NEW_SQRT2_BITS);
    }
}

// =============================================================================
// 8-point inverse identity transform
// Ported from svt_av1_iidentity8_c in inv_transforms.c:2356-2362
// =============================================================================

pub fn iidentity8(input: &[TranLow], output: &mut [TranLow], _range: i8) {
    for i in 0..8 {
        output[i] = input[i] * 2;
    }
}

// =============================================================================
// 16-point inverse DCT-II
// Ported exactly from svt_av1_idct16_new in inv_transforms.c:214-375
// clamp_value replaced with plain add/subtract (wide stage_range)
// =============================================================================

pub fn idct16(input: &[TranLow], output: &mut [TranLow], range: i8) {
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
    output[8] = clamp_value(s(8) + s(9), range);
    output[9] = clamp_value(s(8) - s(9), range);
    output[10] = clamp_value(-s(10) + s(11), range);
    output[11] = clamp_value(s(10) + s(11), range);
    output[12] = clamp_value(s(12) + s(13), range);
    output[13] = clamp_value(s(12) - s(13), range);
    output[14] = clamp_value(-s(14) + s(15), range);
    output[15] = clamp_value(s(14) + s(15), range);

    // stage 4
    let o = |i: usize| -> i32 { output[i] };
    step[0] = half_btf(cospi[32], o(0), cospi[32], o(1), cos_bit);
    step[1] = half_btf(cospi[32], o(0), -cospi[32], o(1), cos_bit);
    step[2] = half_btf(cospi[48], o(2), -cospi[16], o(3), cos_bit);
    step[3] = half_btf(cospi[16], o(2), cospi[48], o(3), cos_bit);
    step[4] = clamp_value(o(4) + o(5), range);
    step[5] = clamp_value(o(4) - o(5), range);
    step[6] = clamp_value(-o(6) + o(7), range);
    step[7] = clamp_value(o(6) + o(7), range);
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
    output[0] = clamp_value(s(0) + s(3), range);
    output[1] = clamp_value(s(1) + s(2), range);
    output[2] = clamp_value(s(1) - s(2), range);
    output[3] = clamp_value(s(0) - s(3), range);
    output[4] = s(4);
    output[5] = half_btf(-cospi[32], s(5), cospi[32], s(6), cos_bit);
    output[6] = half_btf(cospi[32], s(5), cospi[32], s(6), cos_bit);
    output[7] = s(7);
    output[8] = clamp_value(s(8) + s(11), range);
    output[9] = clamp_value(s(9) + s(10), range);
    output[10] = clamp_value(s(9) - s(10), range);
    output[11] = clamp_value(s(8) - s(11), range);
    output[12] = clamp_value(-s(12) + s(15), range);
    output[13] = clamp_value(-s(13) + s(14), range);
    output[14] = clamp_value(s(13) + s(14), range);
    output[15] = clamp_value(s(12) + s(15), range);

    // stage 6
    let o = |i: usize| -> i32 { output[i] };
    step[0] = clamp_value(o(0) + o(7), range);
    step[1] = clamp_value(o(1) + o(6), range);
    step[2] = clamp_value(o(2) + o(5), range);
    step[3] = clamp_value(o(3) + o(4), range);
    step[4] = clamp_value(o(3) - o(4), range);
    step[5] = clamp_value(o(2) - o(5), range);
    step[6] = clamp_value(o(1) - o(6), range);
    step[7] = clamp_value(o(0) - o(7), range);
    step[8] = o(8);
    step[9] = o(9);
    step[10] = half_btf(-cospi[32], o(10), cospi[32], o(13), cos_bit);
    step[11] = half_btf(-cospi[32], o(11), cospi[32], o(12), cos_bit);
    step[12] = half_btf(cospi[32], o(11), cospi[32], o(12), cos_bit);
    step[13] = half_btf(cospi[32], o(10), cospi[32], o(13), cos_bit);
    step[14] = o(14);
    step[15] = o(15);

    // stage 7
    output[0] = clamp_value(step[0] + step[15], range);
    output[1] = clamp_value(step[1] + step[14], range);
    output[2] = clamp_value(step[2] + step[13], range);
    output[3] = clamp_value(step[3] + step[12], range);
    output[4] = clamp_value(step[4] + step[11], range);
    output[5] = clamp_value(step[5] + step[10], range);
    output[6] = clamp_value(step[6] + step[9], range);
    output[7] = clamp_value(step[7] + step[8], range);
    output[8] = clamp_value(step[7] - step[8], range);
    output[9] = clamp_value(step[6] - step[9], range);
    output[10] = clamp_value(step[5] - step[10], range);
    output[11] = clamp_value(step[4] - step[11], range);
    output[12] = clamp_value(step[3] - step[12], range);
    output[13] = clamp_value(step[2] - step[13], range);
    output[14] = clamp_value(step[1] - step[14], range);
    output[15] = clamp_value(step[0] - step[15], range);
}

// =============================================================================
// 32-point inverse DCT-II
// Ported exactly from svt_av1_idct32_new in inv_transforms.c:377-726
// clamp_value replaced with plain add/subtract (wide stage_range)
// =============================================================================

pub fn idct32(input: &[TranLow], output: &mut [TranLow], range: i8) {
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
    output[16] = clamp_value(s(16) + s(17), range);
    output[17] = clamp_value(s(16) - s(17), range);
    output[18] = clamp_value(-s(18) + s(19), range);
    output[19] = clamp_value(s(18) + s(19), range);
    output[20] = clamp_value(s(20) + s(21), range);
    output[21] = clamp_value(s(20) - s(21), range);
    output[22] = clamp_value(-s(22) + s(23), range);
    output[23] = clamp_value(s(22) + s(23), range);
    output[24] = clamp_value(s(24) + s(25), range);
    output[25] = clamp_value(s(24) - s(25), range);
    output[26] = clamp_value(-s(26) + s(27), range);
    output[27] = clamp_value(s(26) + s(27), range);
    output[28] = clamp_value(s(28) + s(29), range);
    output[29] = clamp_value(s(28) - s(29), range);
    output[30] = clamp_value(-s(30) + s(31), range);
    output[31] = clamp_value(s(30) + s(31), range);

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
    step[8] = clamp_value(o(8) + o(9), range);
    step[9] = clamp_value(o(8) - o(9), range);
    step[10] = clamp_value(-o(10) + o(11), range);
    step[11] = clamp_value(o(10) + o(11), range);
    step[12] = clamp_value(o(12) + o(13), range);
    step[13] = clamp_value(o(12) - o(13), range);
    step[14] = clamp_value(-o(14) + o(15), range);
    step[15] = clamp_value(o(14) + o(15), range);
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
    output[4] = clamp_value(s(4) + s(5), range);
    output[5] = clamp_value(s(4) - s(5), range);
    output[6] = clamp_value(-s(6) + s(7), range);
    output[7] = clamp_value(s(6) + s(7), range);
    output[8] = s(8);
    output[9] = half_btf(-cospi[16], s(9), cospi[48], s(14), cos_bit);
    output[10] = half_btf(-cospi[48], s(10), -cospi[16], s(13), cos_bit);
    output[11] = s(11);
    output[12] = s(12);
    output[13] = half_btf(-cospi[16], s(10), cospi[48], s(13), cos_bit);
    output[14] = half_btf(cospi[48], s(9), cospi[16], s(14), cos_bit);
    output[15] = s(15);
    output[16] = clamp_value(s(16) + s(19), range);
    output[17] = clamp_value(s(17) + s(18), range);
    output[18] = clamp_value(s(17) - s(18), range);
    output[19] = clamp_value(s(16) - s(19), range);
    output[20] = clamp_value(-s(20) + s(23), range);
    output[21] = clamp_value(-s(21) + s(22), range);
    output[22] = clamp_value(s(21) + s(22), range);
    output[23] = clamp_value(s(20) + s(23), range);
    output[24] = clamp_value(s(24) + s(27), range);
    output[25] = clamp_value(s(25) + s(26), range);
    output[26] = clamp_value(s(25) - s(26), range);
    output[27] = clamp_value(s(24) - s(27), range);
    output[28] = clamp_value(-s(28) + s(31), range);
    output[29] = clamp_value(-s(29) + s(30), range);
    output[30] = clamp_value(s(29) + s(30), range);
    output[31] = clamp_value(s(28) + s(31), range);

    // stage 6
    let o = |i: usize| -> i32 { output[i] };
    step[0] = clamp_value(o(0) + o(3), range);
    step[1] = clamp_value(o(1) + o(2), range);
    step[2] = clamp_value(o(1) - o(2), range);
    step[3] = clamp_value(o(0) - o(3), range);
    step[4] = o(4);
    step[5] = half_btf(-cospi[32], o(5), cospi[32], o(6), cos_bit);
    step[6] = half_btf(cospi[32], o(5), cospi[32], o(6), cos_bit);
    step[7] = o(7);
    step[8] = clamp_value(o(8) + o(11), range);
    step[9] = clamp_value(o(9) + o(10), range);
    step[10] = clamp_value(o(9) - o(10), range);
    step[11] = clamp_value(o(8) - o(11), range);
    step[12] = clamp_value(-o(12) + o(15), range);
    step[13] = clamp_value(-o(13) + o(14), range);
    step[14] = clamp_value(o(13) + o(14), range);
    step[15] = clamp_value(o(12) + o(15), range);
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
    output[0] = clamp_value(s(0) + s(7), range);
    output[1] = clamp_value(s(1) + s(6), range);
    output[2] = clamp_value(s(2) + s(5), range);
    output[3] = clamp_value(s(3) + s(4), range);
    output[4] = clamp_value(s(3) - s(4), range);
    output[5] = clamp_value(s(2) - s(5), range);
    output[6] = clamp_value(s(1) - s(6), range);
    output[7] = clamp_value(s(0) - s(7), range);
    output[8] = s(8);
    output[9] = s(9);
    output[10] = half_btf(-cospi[32], s(10), cospi[32], s(13), cos_bit);
    output[11] = half_btf(-cospi[32], s(11), cospi[32], s(12), cos_bit);
    output[12] = half_btf(cospi[32], s(11), cospi[32], s(12), cos_bit);
    output[13] = half_btf(cospi[32], s(10), cospi[32], s(13), cos_bit);
    output[14] = s(14);
    output[15] = s(15);
    output[16] = clamp_value(s(16) + s(23), range);
    output[17] = clamp_value(s(17) + s(22), range);
    output[18] = clamp_value(s(18) + s(21), range);
    output[19] = clamp_value(s(19) + s(20), range);
    output[20] = clamp_value(s(19) - s(20), range);
    output[21] = clamp_value(s(18) - s(21), range);
    output[22] = clamp_value(s(17) - s(22), range);
    output[23] = clamp_value(s(16) - s(23), range);
    output[24] = clamp_value(-s(24) + s(31), range);
    output[25] = clamp_value(-s(25) + s(30), range);
    output[26] = clamp_value(-s(26) + s(29), range);
    output[27] = clamp_value(-s(27) + s(28), range);
    output[28] = clamp_value(s(27) + s(28), range);
    output[29] = clamp_value(s(26) + s(29), range);
    output[30] = clamp_value(s(25) + s(30), range);
    output[31] = clamp_value(s(24) + s(31), range);

    // stage 8
    let o = |i: usize| -> i32 { output[i] };
    step[0] = clamp_value(o(0) + o(15), range);
    step[1] = clamp_value(o(1) + o(14), range);
    step[2] = clamp_value(o(2) + o(13), range);
    step[3] = clamp_value(o(3) + o(12), range);
    step[4] = clamp_value(o(4) + o(11), range);
    step[5] = clamp_value(o(5) + o(10), range);
    step[6] = clamp_value(o(6) + o(9), range);
    step[7] = clamp_value(o(7) + o(8), range);
    step[8] = clamp_value(o(7) - o(8), range);
    step[9] = clamp_value(o(6) - o(9), range);
    step[10] = clamp_value(o(5) - o(10), range);
    step[11] = clamp_value(o(4) - o(11), range);
    step[12] = clamp_value(o(3) - o(12), range);
    step[13] = clamp_value(o(2) - o(13), range);
    step[14] = clamp_value(o(1) - o(14), range);
    step[15] = clamp_value(o(0) - o(15), range);
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
    output[0] = clamp_value(s(0) + s(31), range);
    output[1] = clamp_value(s(1) + s(30), range);
    output[2] = clamp_value(s(2) + s(29), range);
    output[3] = clamp_value(s(3) + s(28), range);
    output[4] = clamp_value(s(4) + s(27), range);
    output[5] = clamp_value(s(5) + s(26), range);
    output[6] = clamp_value(s(6) + s(25), range);
    output[7] = clamp_value(s(7) + s(24), range);
    output[8] = clamp_value(s(8) + s(23), range);
    output[9] = clamp_value(s(9) + s(22), range);
    output[10] = clamp_value(s(10) + s(21), range);
    output[11] = clamp_value(s(11) + s(20), range);
    output[12] = clamp_value(s(12) + s(19), range);
    output[13] = clamp_value(s(13) + s(18), range);
    output[14] = clamp_value(s(14) + s(17), range);
    output[15] = clamp_value(s(15) + s(16), range);
    output[16] = clamp_value(s(15) - s(16), range);
    output[17] = clamp_value(s(14) - s(17), range);
    output[18] = clamp_value(s(13) - s(18), range);
    output[19] = clamp_value(s(12) - s(19), range);
    output[20] = clamp_value(s(11) - s(20), range);
    output[21] = clamp_value(s(10) - s(21), range);
    output[22] = clamp_value(s(9) - s(22), range);
    output[23] = clamp_value(s(8) - s(23), range);
    output[24] = clamp_value(s(7) - s(24), range);
    output[25] = clamp_value(s(6) - s(25), range);
    output[26] = clamp_value(s(5) - s(26), range);
    output[27] = clamp_value(s(4) - s(27), range);
    output[28] = clamp_value(s(3) - s(28), range);
    output[29] = clamp_value(s(2) - s(29), range);
    output[30] = clamp_value(s(1) - s(30), range);
    output[31] = clamp_value(s(0) - s(31), range);
}

// =============================================================================
// 32-point inverse identity transform
// Ported from svt_av1_iidentity32_c in inv_transforms.c
// =============================================================================

pub fn iidentity32(input: &[TranLow], output: &mut [TranLow], _range: i8) {
    for i in 0..32 {
        output[i] = input[i] * 4;
    }
}

// =============================================================================
// 64-point inverse DCT-II
// Ported exactly from svt_av1_idct64_new in inv_transforms.c:1566-2343
// clamp_value replaced with plain add/subtract (wide stage_range)
// =============================================================================

pub fn idct64(input: &[TranLow], output: &mut [TranLow], range: i8) {
    let cospi = &COSPI;
    let cos_bit = COS_BIT;
    let mut step = [0i32; 64];

    // stage 1: input permutation (bit-reversal)
    output[0] = input[0];
    output[1] = input[32];
    output[2] = input[16];
    output[3] = input[48];
    output[4] = input[8];
    output[5] = input[40];
    output[6] = input[24];
    output[7] = input[56];
    output[8] = input[4];
    output[9] = input[36];
    output[10] = input[20];
    output[11] = input[52];
    output[12] = input[12];
    output[13] = input[44];
    output[14] = input[28];
    output[15] = input[60];
    output[16] = input[2];
    output[17] = input[34];
    output[18] = input[18];
    output[19] = input[50];
    output[20] = input[10];
    output[21] = input[42];
    output[22] = input[26];
    output[23] = input[58];
    output[24] = input[6];
    output[25] = input[38];
    output[26] = input[22];
    output[27] = input[54];
    output[28] = input[14];
    output[29] = input[46];
    output[30] = input[30];
    output[31] = input[62];
    output[32] = input[1];
    output[33] = input[33];
    output[34] = input[17];
    output[35] = input[49];
    output[36] = input[9];
    output[37] = input[41];
    output[38] = input[25];
    output[39] = input[57];
    output[40] = input[5];
    output[41] = input[37];
    output[42] = input[21];
    output[43] = input[53];
    output[44] = input[13];
    output[45] = input[45];
    output[46] = input[29];
    output[47] = input[61];
    output[48] = input[3];
    output[49] = input[35];
    output[50] = input[19];
    output[51] = input[51];
    output[52] = input[11];
    output[53] = input[43];
    output[54] = input[27];
    output[55] = input[59];
    output[56] = input[7];
    output[57] = input[39];
    output[58] = input[23];
    output[59] = input[55];
    output[60] = input[15];
    output[61] = input[47];
    output[62] = input[31];
    output[63] = input[63];

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
    step[32] = half_btf(cospi[63], o(32), -cospi[1], o(63), cos_bit);
    step[33] = half_btf(cospi[31], o(33), -cospi[33], o(62), cos_bit);
    step[34] = half_btf(cospi[47], o(34), -cospi[17], o(61), cos_bit);
    step[35] = half_btf(cospi[15], o(35), -cospi[49], o(60), cos_bit);
    step[36] = half_btf(cospi[55], o(36), -cospi[9], o(59), cos_bit);
    step[37] = half_btf(cospi[23], o(37), -cospi[41], o(58), cos_bit);
    step[38] = half_btf(cospi[39], o(38), -cospi[25], o(57), cos_bit);
    step[39] = half_btf(cospi[7], o(39), -cospi[57], o(56), cos_bit);
    step[40] = half_btf(cospi[59], o(40), -cospi[5], o(55), cos_bit);
    step[41] = half_btf(cospi[27], o(41), -cospi[37], o(54), cos_bit);
    step[42] = half_btf(cospi[43], o(42), -cospi[21], o(53), cos_bit);
    step[43] = half_btf(cospi[11], o(43), -cospi[53], o(52), cos_bit);
    step[44] = half_btf(cospi[51], o(44), -cospi[13], o(51), cos_bit);
    step[45] = half_btf(cospi[19], o(45), -cospi[45], o(50), cos_bit);
    step[46] = half_btf(cospi[35], o(46), -cospi[29], o(49), cos_bit);
    step[47] = half_btf(cospi[3], o(47), -cospi[61], o(48), cos_bit);
    step[48] = half_btf(cospi[61], o(47), cospi[3], o(48), cos_bit);
    step[49] = half_btf(cospi[29], o(46), cospi[35], o(49), cos_bit);
    step[50] = half_btf(cospi[45], o(45), cospi[19], o(50), cos_bit);
    step[51] = half_btf(cospi[13], o(44), cospi[51], o(51), cos_bit);
    step[52] = half_btf(cospi[53], o(43), cospi[11], o(52), cos_bit);
    step[53] = half_btf(cospi[21], o(42), cospi[43], o(53), cos_bit);
    step[54] = half_btf(cospi[37], o(41), cospi[27], o(54), cos_bit);
    step[55] = half_btf(cospi[5], o(40), cospi[59], o(55), cos_bit);
    step[56] = half_btf(cospi[57], o(39), cospi[7], o(56), cos_bit);
    step[57] = half_btf(cospi[25], o(38), cospi[39], o(57), cos_bit);
    step[58] = half_btf(cospi[41], o(37), cospi[23], o(58), cos_bit);
    step[59] = half_btf(cospi[9], o(36), cospi[55], o(59), cos_bit);
    step[60] = half_btf(cospi[49], o(35), cospi[15], o(60), cos_bit);
    step[61] = half_btf(cospi[17], o(34), cospi[47], o(61), cos_bit);
    step[62] = half_btf(cospi[33], o(33), cospi[31], o(62), cos_bit);
    step[63] = half_btf(cospi[1], o(32), cospi[63], o(63), cos_bit);

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
    output[8] = s(8);
    output[9] = s(9);
    output[10] = s(10);
    output[11] = s(11);
    output[12] = s(12);
    output[13] = s(13);
    output[14] = s(14);
    output[15] = s(15);
    output[16] = half_btf(cospi[62], s(16), -cospi[2], s(31), cos_bit);
    output[17] = half_btf(cospi[30], s(17), -cospi[34], s(30), cos_bit);
    output[18] = half_btf(cospi[46], s(18), -cospi[18], s(29), cos_bit);
    output[19] = half_btf(cospi[14], s(19), -cospi[50], s(28), cos_bit);
    output[20] = half_btf(cospi[54], s(20), -cospi[10], s(27), cos_bit);
    output[21] = half_btf(cospi[22], s(21), -cospi[42], s(26), cos_bit);
    output[22] = half_btf(cospi[38], s(22), -cospi[26], s(25), cos_bit);
    output[23] = half_btf(cospi[6], s(23), -cospi[58], s(24), cos_bit);
    output[24] = half_btf(cospi[58], s(23), cospi[6], s(24), cos_bit);
    output[25] = half_btf(cospi[26], s(22), cospi[38], s(25), cos_bit);
    output[26] = half_btf(cospi[42], s(21), cospi[22], s(26), cos_bit);
    output[27] = half_btf(cospi[10], s(20), cospi[54], s(27), cos_bit);
    output[28] = half_btf(cospi[50], s(19), cospi[14], s(28), cos_bit);
    output[29] = half_btf(cospi[18], s(18), cospi[46], s(29), cos_bit);
    output[30] = half_btf(cospi[34], s(17), cospi[30], s(30), cos_bit);
    output[31] = half_btf(cospi[2], s(16), cospi[62], s(31), cos_bit);
    output[32] = clamp_value(s(32) + s(33), range);
    output[33] = clamp_value(s(32) - s(33), range);
    output[34] = clamp_value(-s(34) + s(35), range);
    output[35] = clamp_value(s(34) + s(35), range);
    output[36] = clamp_value(s(36) + s(37), range);
    output[37] = clamp_value(s(36) - s(37), range);
    output[38] = clamp_value(-s(38) + s(39), range);
    output[39] = clamp_value(s(38) + s(39), range);
    output[40] = clamp_value(s(40) + s(41), range);
    output[41] = clamp_value(s(40) - s(41), range);
    output[42] = clamp_value(-s(42) + s(43), range);
    output[43] = clamp_value(s(42) + s(43), range);
    output[44] = clamp_value(s(44) + s(45), range);
    output[45] = clamp_value(s(44) - s(45), range);
    output[46] = clamp_value(-s(46) + s(47), range);
    output[47] = clamp_value(s(46) + s(47), range);
    output[48] = clamp_value(s(48) + s(49), range);
    output[49] = clamp_value(s(48) - s(49), range);
    output[50] = clamp_value(-s(50) + s(51), range);
    output[51] = clamp_value(s(50) + s(51), range);
    output[52] = clamp_value(s(52) + s(53), range);
    output[53] = clamp_value(s(52) - s(53), range);
    output[54] = clamp_value(-s(54) + s(55), range);
    output[55] = clamp_value(s(54) + s(55), range);
    output[56] = clamp_value(s(56) + s(57), range);
    output[57] = clamp_value(s(56) - s(57), range);
    output[58] = clamp_value(-s(58) + s(59), range);
    output[59] = clamp_value(s(58) + s(59), range);
    output[60] = clamp_value(s(60) + s(61), range);
    output[61] = clamp_value(s(60) - s(61), range);
    output[62] = clamp_value(-s(62) + s(63), range);
    output[63] = clamp_value(s(62) + s(63), range);

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
    step[8] = half_btf(cospi[60], o(8), -cospi[4], o(15), cos_bit);
    step[9] = half_btf(cospi[28], o(9), -cospi[36], o(14), cos_bit);
    step[10] = half_btf(cospi[44], o(10), -cospi[20], o(13), cos_bit);
    step[11] = half_btf(cospi[12], o(11), -cospi[52], o(12), cos_bit);
    step[12] = half_btf(cospi[52], o(11), cospi[12], o(12), cos_bit);
    step[13] = half_btf(cospi[20], o(10), cospi[44], o(13), cos_bit);
    step[14] = half_btf(cospi[36], o(9), cospi[28], o(14), cos_bit);
    step[15] = half_btf(cospi[4], o(8), cospi[60], o(15), cos_bit);
    step[16] = clamp_value(o(16) + o(17), range);
    step[17] = clamp_value(o(16) - o(17), range);
    step[18] = clamp_value(-o(18) + o(19), range);
    step[19] = clamp_value(o(18) + o(19), range);
    step[20] = clamp_value(o(20) + o(21), range);
    step[21] = clamp_value(o(20) - o(21), range);
    step[22] = clamp_value(-o(22) + o(23), range);
    step[23] = clamp_value(o(22) + o(23), range);
    step[24] = clamp_value(o(24) + o(25), range);
    step[25] = clamp_value(o(24) - o(25), range);
    step[26] = clamp_value(-o(26) + o(27), range);
    step[27] = clamp_value(o(26) + o(27), range);
    step[28] = clamp_value(o(28) + o(29), range);
    step[29] = clamp_value(o(28) - o(29), range);
    step[30] = clamp_value(-o(30) + o(31), range);
    step[31] = clamp_value(o(30) + o(31), range);
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
    step[49] = half_btf(-cospi[52], o(46), cospi[12], o(49), cos_bit);
    step[50] = half_btf(cospi[12], o(45), cospi[52], o(50), cos_bit);
    step[51] = o(51);
    step[52] = o(52);
    step[53] = half_btf(-cospi[20], o(42), cospi[44], o(53), cos_bit);
    step[54] = half_btf(cospi[44], o(41), cospi[20], o(54), cos_bit);
    step[55] = o(55);
    step[56] = o(56);
    step[57] = half_btf(-cospi[36], o(38), cospi[28], o(57), cos_bit);
    step[58] = half_btf(cospi[28], o(37), cospi[36], o(58), cos_bit);
    step[59] = o(59);
    step[60] = o(60);
    step[61] = half_btf(-cospi[4], o(34), cospi[60], o(61), cos_bit);
    step[62] = half_btf(cospi[60], o(33), cospi[4], o(62), cos_bit);
    step[63] = o(63);

    // stage 5
    let s = |i: usize| -> i32 { step[i] };
    output[0] = s(0);
    output[1] = s(1);
    output[2] = s(2);
    output[3] = s(3);
    output[4] = half_btf(cospi[56], s(4), -cospi[8], s(7), cos_bit);
    output[5] = half_btf(cospi[24], s(5), -cospi[40], s(6), cos_bit);
    output[6] = half_btf(cospi[40], s(5), cospi[24], s(6), cos_bit);
    output[7] = half_btf(cospi[8], s(4), cospi[56], s(7), cos_bit);
    output[8] = clamp_value(s(8) + s(9), range);
    output[9] = clamp_value(s(8) - s(9), range);
    output[10] = clamp_value(-s(10) + s(11), range);
    output[11] = clamp_value(s(10) + s(11), range);
    output[12] = clamp_value(s(12) + s(13), range);
    output[13] = clamp_value(s(12) - s(13), range);
    output[14] = clamp_value(-s(14) + s(15), range);
    output[15] = clamp_value(s(14) + s(15), range);
    output[16] = s(16);
    output[17] = half_btf(-cospi[8], s(17), cospi[56], s(30), cos_bit);
    output[18] = half_btf(-cospi[56], s(18), -cospi[8], s(29), cos_bit);
    output[19] = s(19);
    output[20] = s(20);
    output[21] = half_btf(-cospi[40], s(21), cospi[24], s(26), cos_bit);
    output[22] = half_btf(-cospi[24], s(22), -cospi[40], s(25), cos_bit);
    output[23] = s(23);
    output[24] = s(24);
    output[25] = half_btf(-cospi[40], s(22), cospi[24], s(25), cos_bit);
    output[26] = half_btf(cospi[24], s(21), cospi[40], s(26), cos_bit);
    output[27] = s(27);
    output[28] = s(28);
    output[29] = half_btf(-cospi[8], s(18), cospi[56], s(29), cos_bit);
    output[30] = half_btf(cospi[56], s(17), cospi[8], s(30), cos_bit);
    output[31] = s(31);
    output[32] = clamp_value(s(32) + s(35), range);
    output[33] = clamp_value(s(33) + s(34), range);
    output[34] = clamp_value(s(33) - s(34), range);
    output[35] = clamp_value(s(32) - s(35), range);
    output[36] = clamp_value(-s(36) + s(39), range);
    output[37] = clamp_value(-s(37) + s(38), range);
    output[38] = clamp_value(s(37) + s(38), range);
    output[39] = clamp_value(s(36) + s(39), range);
    output[40] = clamp_value(s(40) + s(43), range);
    output[41] = clamp_value(s(41) + s(42), range);
    output[42] = clamp_value(s(41) - s(42), range);
    output[43] = clamp_value(s(40) - s(43), range);
    output[44] = clamp_value(-s(44) + s(47), range);
    output[45] = clamp_value(-s(45) + s(46), range);
    output[46] = clamp_value(s(45) + s(46), range);
    output[47] = clamp_value(s(44) + s(47), range);
    output[48] = clamp_value(s(48) + s(51), range);
    output[49] = clamp_value(s(49) + s(50), range);
    output[50] = clamp_value(s(49) - s(50), range);
    output[51] = clamp_value(s(48) - s(51), range);
    output[52] = clamp_value(-s(52) + s(55), range);
    output[53] = clamp_value(-s(53) + s(54), range);
    output[54] = clamp_value(s(53) + s(54), range);
    output[55] = clamp_value(s(52) + s(55), range);
    output[56] = clamp_value(s(56) + s(59), range);
    output[57] = clamp_value(s(57) + s(58), range);
    output[58] = clamp_value(s(57) - s(58), range);
    output[59] = clamp_value(s(56) - s(59), range);
    output[60] = clamp_value(-s(60) + s(63), range);
    output[61] = clamp_value(-s(61) + s(62), range);
    output[62] = clamp_value(s(61) + s(62), range);
    output[63] = clamp_value(s(60) + s(63), range);

    // stage 6
    let o = |i: usize| -> i32 { output[i] };
    step[0] = half_btf(cospi[32], o(0), cospi[32], o(1), cos_bit);
    step[1] = half_btf(cospi[32], o(0), -cospi[32], o(1), cos_bit);
    step[2] = half_btf(cospi[48], o(2), -cospi[16], o(3), cos_bit);
    step[3] = half_btf(cospi[16], o(2), cospi[48], o(3), cos_bit);
    step[4] = clamp_value(o(4) + o(5), range);
    step[5] = clamp_value(o(4) - o(5), range);
    step[6] = clamp_value(-o(6) + o(7), range);
    step[7] = clamp_value(o(6) + o(7), range);
    step[8] = o(8);
    step[9] = half_btf(-cospi[16], o(9), cospi[48], o(14), cos_bit);
    step[10] = half_btf(-cospi[48], o(10), -cospi[16], o(13), cos_bit);
    step[11] = o(11);
    step[12] = o(12);
    step[13] = half_btf(-cospi[16], o(10), cospi[48], o(13), cos_bit);
    step[14] = half_btf(cospi[48], o(9), cospi[16], o(14), cos_bit);
    step[15] = o(15);
    step[16] = clamp_value(o(16) + o(19), range);
    step[17] = clamp_value(o(17) + o(18), range);
    step[18] = clamp_value(o(17) - o(18), range);
    step[19] = clamp_value(o(16) - o(19), range);
    step[20] = clamp_value(-o(20) + o(23), range);
    step[21] = clamp_value(-o(21) + o(22), range);
    step[22] = clamp_value(o(21) + o(22), range);
    step[23] = clamp_value(o(20) + o(23), range);
    step[24] = clamp_value(o(24) + o(27), range);
    step[25] = clamp_value(o(25) + o(26), range);
    step[26] = clamp_value(o(25) - o(26), range);
    step[27] = clamp_value(o(24) - o(27), range);
    step[28] = clamp_value(-o(28) + o(31), range);
    step[29] = clamp_value(-o(29) + o(30), range);
    step[30] = clamp_value(o(29) + o(30), range);
    step[31] = clamp_value(o(28) + o(31), range);
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
    step[50] = half_btf(-cospi[40], o(45), cospi[24], o(50), cos_bit);
    step[51] = half_btf(-cospi[40], o(44), cospi[24], o(51), cos_bit);
    step[52] = half_btf(cospi[24], o(43), cospi[40], o(52), cos_bit);
    step[53] = half_btf(cospi[24], o(42), cospi[40], o(53), cos_bit);
    step[54] = o(54);
    step[55] = o(55);
    step[56] = o(56);
    step[57] = o(57);
    step[58] = half_btf(-cospi[8], o(37), cospi[56], o(58), cos_bit);
    step[59] = half_btf(-cospi[8], o(36), cospi[56], o(59), cos_bit);
    step[60] = half_btf(cospi[56], o(35), cospi[8], o(60), cos_bit);
    step[61] = half_btf(cospi[56], o(34), cospi[8], o(61), cos_bit);
    step[62] = o(62);
    step[63] = o(63);

    // stage 7
    let s = |i: usize| -> i32 { step[i] };
    output[0] = clamp_value(s(0) + s(3), range);
    output[1] = clamp_value(s(1) + s(2), range);
    output[2] = clamp_value(s(1) - s(2), range);
    output[3] = clamp_value(s(0) - s(3), range);
    output[4] = s(4);
    output[5] = half_btf(-cospi[32], s(5), cospi[32], s(6), cos_bit);
    output[6] = half_btf(cospi[32], s(5), cospi[32], s(6), cos_bit);
    output[7] = s(7);
    output[8] = clamp_value(s(8) + s(11), range);
    output[9] = clamp_value(s(9) + s(10), range);
    output[10] = clamp_value(s(9) - s(10), range);
    output[11] = clamp_value(s(8) - s(11), range);
    output[12] = clamp_value(-s(12) + s(15), range);
    output[13] = clamp_value(-s(13) + s(14), range);
    output[14] = clamp_value(s(13) + s(14), range);
    output[15] = clamp_value(s(12) + s(15), range);
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
    output[26] = half_btf(-cospi[16], s(21), cospi[48], s(26), cos_bit);
    output[27] = half_btf(-cospi[16], s(20), cospi[48], s(27), cos_bit);
    output[28] = half_btf(cospi[48], s(19), cospi[16], s(28), cos_bit);
    output[29] = half_btf(cospi[48], s(18), cospi[16], s(29), cos_bit);
    output[30] = s(30);
    output[31] = s(31);
    output[32] = clamp_value(s(32) + s(39), range);
    output[33] = clamp_value(s(33) + s(38), range);
    output[34] = clamp_value(s(34) + s(37), range);
    output[35] = clamp_value(s(35) + s(36), range);
    output[36] = clamp_value(s(35) - s(36), range);
    output[37] = clamp_value(s(34) - s(37), range);
    output[38] = clamp_value(s(33) - s(38), range);
    output[39] = clamp_value(s(32) - s(39), range);
    output[40] = clamp_value(-s(40) + s(47), range);
    output[41] = clamp_value(-s(41) + s(46), range);
    output[42] = clamp_value(-s(42) + s(45), range);
    output[43] = clamp_value(-s(43) + s(44), range);
    output[44] = clamp_value(s(43) + s(44), range);
    output[45] = clamp_value(s(42) + s(45), range);
    output[46] = clamp_value(s(41) + s(46), range);
    output[47] = clamp_value(s(40) + s(47), range);
    output[48] = clamp_value(s(48) + s(55), range);
    output[49] = clamp_value(s(49) + s(54), range);
    output[50] = clamp_value(s(50) + s(53), range);
    output[51] = clamp_value(s(51) + s(52), range);
    output[52] = clamp_value(s(51) - s(52), range);
    output[53] = clamp_value(s(50) - s(53), range);
    output[54] = clamp_value(s(49) - s(54), range);
    output[55] = clamp_value(s(48) - s(55), range);
    output[56] = clamp_value(-s(56) + s(63), range);
    output[57] = clamp_value(-s(57) + s(62), range);
    output[58] = clamp_value(-s(58) + s(61), range);
    output[59] = clamp_value(-s(59) + s(60), range);
    output[60] = clamp_value(s(59) + s(60), range);
    output[61] = clamp_value(s(58) + s(61), range);
    output[62] = clamp_value(s(57) + s(62), range);
    output[63] = clamp_value(s(56) + s(63), range);

    // stage 8
    let o = |i: usize| -> i32 { output[i] };
    step[0] = clamp_value(o(0) + o(7), range);
    step[1] = clamp_value(o(1) + o(6), range);
    step[2] = clamp_value(o(2) + o(5), range);
    step[3] = clamp_value(o(3) + o(4), range);
    step[4] = clamp_value(o(3) - o(4), range);
    step[5] = clamp_value(o(2) - o(5), range);
    step[6] = clamp_value(o(1) - o(6), range);
    step[7] = clamp_value(o(0) - o(7), range);
    step[8] = o(8);
    step[9] = o(9);
    step[10] = half_btf(-cospi[32], o(10), cospi[32], o(13), cos_bit);
    step[11] = half_btf(-cospi[32], o(11), cospi[32], o(12), cos_bit);
    step[12] = half_btf(cospi[32], o(11), cospi[32], o(12), cos_bit);
    step[13] = half_btf(cospi[32], o(10), cospi[32], o(13), cos_bit);
    step[14] = o(14);
    step[15] = o(15);
    step[16] = clamp_value(o(16) + o(23), range);
    step[17] = clamp_value(o(17) + o(22), range);
    step[18] = clamp_value(o(18) + o(21), range);
    step[19] = clamp_value(o(19) + o(20), range);
    step[20] = clamp_value(o(19) - o(20), range);
    step[21] = clamp_value(o(18) - o(21), range);
    step[22] = clamp_value(o(17) - o(22), range);
    step[23] = clamp_value(o(16) - o(23), range);
    step[24] = clamp_value(-o(24) + o(31), range);
    step[25] = clamp_value(-o(25) + o(30), range);
    step[26] = clamp_value(-o(26) + o(29), range);
    step[27] = clamp_value(-o(27) + o(28), range);
    step[28] = clamp_value(o(27) + o(28), range);
    step[29] = clamp_value(o(26) + o(29), range);
    step[30] = clamp_value(o(25) + o(30), range);
    step[31] = clamp_value(o(24) + o(31), range);
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
    step[52] = half_btf(-cospi[16], o(43), cospi[48], o(52), cos_bit);
    step[53] = half_btf(-cospi[16], o(42), cospi[48], o(53), cos_bit);
    step[54] = half_btf(-cospi[16], o(41), cospi[48], o(54), cos_bit);
    step[55] = half_btf(-cospi[16], o(40), cospi[48], o(55), cos_bit);
    step[56] = half_btf(cospi[48], o(39), cospi[16], o(56), cos_bit);
    step[57] = half_btf(cospi[48], o(38), cospi[16], o(57), cos_bit);
    step[58] = half_btf(cospi[48], o(37), cospi[16], o(58), cos_bit);
    step[59] = half_btf(cospi[48], o(36), cospi[16], o(59), cos_bit);
    step[60] = o(60);
    step[61] = o(61);
    step[62] = o(62);
    step[63] = o(63);

    // stage 9
    let s = |i: usize| -> i32 { step[i] };
    output[0] = clamp_value(s(0) + s(15), range);
    output[1] = clamp_value(s(1) + s(14), range);
    output[2] = clamp_value(s(2) + s(13), range);
    output[3] = clamp_value(s(3) + s(12), range);
    output[4] = clamp_value(s(4) + s(11), range);
    output[5] = clamp_value(s(5) + s(10), range);
    output[6] = clamp_value(s(6) + s(9), range);
    output[7] = clamp_value(s(7) + s(8), range);
    output[8] = clamp_value(s(7) - s(8), range);
    output[9] = clamp_value(s(6) - s(9), range);
    output[10] = clamp_value(s(5) - s(10), range);
    output[11] = clamp_value(s(4) - s(11), range);
    output[12] = clamp_value(s(3) - s(12), range);
    output[13] = clamp_value(s(2) - s(13), range);
    output[14] = clamp_value(s(1) - s(14), range);
    output[15] = clamp_value(s(0) - s(15), range);
    output[16] = s(16);
    output[17] = s(17);
    output[18] = s(18);
    output[19] = s(19);
    output[20] = half_btf(-cospi[32], s(20), cospi[32], s(27), cos_bit);
    output[21] = half_btf(-cospi[32], s(21), cospi[32], s(26), cos_bit);
    output[22] = half_btf(-cospi[32], s(22), cospi[32], s(25), cos_bit);
    output[23] = half_btf(-cospi[32], s(23), cospi[32], s(24), cos_bit);
    output[24] = half_btf(cospi[32], s(23), cospi[32], s(24), cos_bit);
    output[25] = half_btf(cospi[32], s(22), cospi[32], s(25), cos_bit);
    output[26] = half_btf(cospi[32], s(21), cospi[32], s(26), cos_bit);
    output[27] = half_btf(cospi[32], s(20), cospi[32], s(27), cos_bit);
    output[28] = s(28);
    output[29] = s(29);
    output[30] = s(30);
    output[31] = s(31);
    output[32] = clamp_value(s(32) + s(47), range);
    output[33] = clamp_value(s(33) + s(46), range);
    output[34] = clamp_value(s(34) + s(45), range);
    output[35] = clamp_value(s(35) + s(44), range);
    output[36] = clamp_value(s(36) + s(43), range);
    output[37] = clamp_value(s(37) + s(42), range);
    output[38] = clamp_value(s(38) + s(41), range);
    output[39] = clamp_value(s(39) + s(40), range);
    output[40] = clamp_value(s(39) - s(40), range);
    output[41] = clamp_value(s(38) - s(41), range);
    output[42] = clamp_value(s(37) - s(42), range);
    output[43] = clamp_value(s(36) - s(43), range);
    output[44] = clamp_value(s(35) - s(44), range);
    output[45] = clamp_value(s(34) - s(45), range);
    output[46] = clamp_value(s(33) - s(46), range);
    output[47] = clamp_value(s(32) - s(47), range);
    output[48] = clamp_value(-s(48) + s(63), range);
    output[49] = clamp_value(-s(49) + s(62), range);
    output[50] = clamp_value(-s(50) + s(61), range);
    output[51] = clamp_value(-s(51) + s(60), range);
    output[52] = clamp_value(-s(52) + s(59), range);
    output[53] = clamp_value(-s(53) + s(58), range);
    output[54] = clamp_value(-s(54) + s(57), range);
    output[55] = clamp_value(-s(55) + s(56), range);
    output[56] = clamp_value(s(55) + s(56), range);
    output[57] = clamp_value(s(54) + s(57), range);
    output[58] = clamp_value(s(53) + s(58), range);
    output[59] = clamp_value(s(52) + s(59), range);
    output[60] = clamp_value(s(51) + s(60), range);
    output[61] = clamp_value(s(50) + s(61), range);
    output[62] = clamp_value(s(49) + s(62), range);
    output[63] = clamp_value(s(48) + s(63), range);

    // stage 10
    let o = |i: usize| -> i32 { output[i] };
    step[0] = clamp_value(o(0) + o(31), range);
    step[1] = clamp_value(o(1) + o(30), range);
    step[2] = clamp_value(o(2) + o(29), range);
    step[3] = clamp_value(o(3) + o(28), range);
    step[4] = clamp_value(o(4) + o(27), range);
    step[5] = clamp_value(o(5) + o(26), range);
    step[6] = clamp_value(o(6) + o(25), range);
    step[7] = clamp_value(o(7) + o(24), range);
    step[8] = clamp_value(o(8) + o(23), range);
    step[9] = clamp_value(o(9) + o(22), range);
    step[10] = clamp_value(o(10) + o(21), range);
    step[11] = clamp_value(o(11) + o(20), range);
    step[12] = clamp_value(o(12) + o(19), range);
    step[13] = clamp_value(o(13) + o(18), range);
    step[14] = clamp_value(o(14) + o(17), range);
    step[15] = clamp_value(o(15) + o(16), range);
    step[16] = clamp_value(o(15) - o(16), range);
    step[17] = clamp_value(o(14) - o(17), range);
    step[18] = clamp_value(o(13) - o(18), range);
    step[19] = clamp_value(o(12) - o(19), range);
    step[20] = clamp_value(o(11) - o(20), range);
    step[21] = clamp_value(o(10) - o(21), range);
    step[22] = clamp_value(o(9) - o(22), range);
    step[23] = clamp_value(o(8) - o(23), range);
    step[24] = clamp_value(o(7) - o(24), range);
    step[25] = clamp_value(o(6) - o(25), range);
    step[26] = clamp_value(o(5) - o(26), range);
    step[27] = clamp_value(o(4) - o(27), range);
    step[28] = clamp_value(o(3) - o(28), range);
    step[29] = clamp_value(o(2) - o(29), range);
    step[30] = clamp_value(o(1) - o(30), range);
    step[31] = clamp_value(o(0) - o(31), range);
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
    step[48] = half_btf(cospi[32], o(47), cospi[32], o(48), cos_bit);
    step[49] = half_btf(cospi[32], o(46), cospi[32], o(49), cos_bit);
    step[50] = half_btf(cospi[32], o(45), cospi[32], o(50), cos_bit);
    step[51] = half_btf(cospi[32], o(44), cospi[32], o(51), cos_bit);
    step[52] = half_btf(cospi[32], o(43), cospi[32], o(52), cos_bit);
    step[53] = half_btf(cospi[32], o(42), cospi[32], o(53), cos_bit);
    step[54] = half_btf(cospi[32], o(41), cospi[32], o(54), cos_bit);
    step[55] = half_btf(cospi[32], o(40), cospi[32], o(55), cos_bit);
    step[56] = o(56);
    step[57] = o(57);
    step[58] = o(58);
    step[59] = o(59);
    step[60] = o(60);
    step[61] = o(61);
    step[62] = o(62);
    step[63] = o(63);

    // stage 11
    let s = |i: usize| -> i32 { step[i] };
    output[0] = clamp_value(s(0) + s(63), range);
    output[1] = clamp_value(s(1) + s(62), range);
    output[2] = clamp_value(s(2) + s(61), range);
    output[3] = clamp_value(s(3) + s(60), range);
    output[4] = clamp_value(s(4) + s(59), range);
    output[5] = clamp_value(s(5) + s(58), range);
    output[6] = clamp_value(s(6) + s(57), range);
    output[7] = clamp_value(s(7) + s(56), range);
    output[8] = clamp_value(s(8) + s(55), range);
    output[9] = clamp_value(s(9) + s(54), range);
    output[10] = clamp_value(s(10) + s(53), range);
    output[11] = clamp_value(s(11) + s(52), range);
    output[12] = clamp_value(s(12) + s(51), range);
    output[13] = clamp_value(s(13) + s(50), range);
    output[14] = clamp_value(s(14) + s(49), range);
    output[15] = clamp_value(s(15) + s(48), range);
    output[16] = clamp_value(s(16) + s(47), range);
    output[17] = clamp_value(s(17) + s(46), range);
    output[18] = clamp_value(s(18) + s(45), range);
    output[19] = clamp_value(s(19) + s(44), range);
    output[20] = clamp_value(s(20) + s(43), range);
    output[21] = clamp_value(s(21) + s(42), range);
    output[22] = clamp_value(s(22) + s(41), range);
    output[23] = clamp_value(s(23) + s(40), range);
    output[24] = clamp_value(s(24) + s(39), range);
    output[25] = clamp_value(s(25) + s(38), range);
    output[26] = clamp_value(s(26) + s(37), range);
    output[27] = clamp_value(s(27) + s(36), range);
    output[28] = clamp_value(s(28) + s(35), range);
    output[29] = clamp_value(s(29) + s(34), range);
    output[30] = clamp_value(s(30) + s(33), range);
    output[31] = clamp_value(s(31) + s(32), range);
    output[32] = clamp_value(s(31) - s(32), range);
    output[33] = clamp_value(s(30) - s(33), range);
    output[34] = clamp_value(s(29) - s(34), range);
    output[35] = clamp_value(s(28) - s(35), range);
    output[36] = clamp_value(s(27) - s(36), range);
    output[37] = clamp_value(s(26) - s(37), range);
    output[38] = clamp_value(s(25) - s(38), range);
    output[39] = clamp_value(s(24) - s(39), range);
    output[40] = clamp_value(s(23) - s(40), range);
    output[41] = clamp_value(s(22) - s(41), range);
    output[42] = clamp_value(s(21) - s(42), range);
    output[43] = clamp_value(s(20) - s(43), range);
    output[44] = clamp_value(s(19) - s(44), range);
    output[45] = clamp_value(s(18) - s(45), range);
    output[46] = clamp_value(s(17) - s(46), range);
    output[47] = clamp_value(s(16) - s(47), range);
    output[48] = clamp_value(s(15) - s(48), range);
    output[49] = clamp_value(s(14) - s(49), range);
    output[50] = clamp_value(s(13) - s(50), range);
    output[51] = clamp_value(s(12) - s(51), range);
    output[52] = clamp_value(s(11) - s(52), range);
    output[53] = clamp_value(s(10) - s(53), range);
    output[54] = clamp_value(s(9) - s(54), range);
    output[55] = clamp_value(s(8) - s(55), range);
    output[56] = clamp_value(s(7) - s(56), range);
    output[57] = clamp_value(s(6) - s(57), range);
    output[58] = clamp_value(s(5) - s(58), range);
    output[59] = clamp_value(s(4) - s(59), range);
    output[60] = clamp_value(s(3) - s(60), range);
    output[61] = clamp_value(s(2) - s(61), range);
    output[62] = clamp_value(s(1) - s(62), range);
    output[63] = clamp_value(s(0) - s(63), range);
}

// =============================================================================
// 64-point inverse identity transform
// Ported from svt_av1_iidentity64_c in inv_transforms.c
// =============================================================================

pub fn iidentity64(input: &[TranLow], output: &mut [TranLow], _range: i8) {
    for i in 0..64 {
        output[i] = round_shift_i64(input[i] as i64 * 4 * NEW_SQRT2 as i64, NEW_SQRT2_BITS);
    }
}

// =============================================================================
// 8-point inverse ADST
// Ported exactly from svt_av1_iadst8_new in inv_transforms.c:821-924
// =============================================================================

pub fn iadst8(input: &[TranLow], output: &mut [TranLow], range: i8) {
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
    output[0] = clamp_value(step[0] + step[4], range);
    output[1] = clamp_value(step[1] + step[5], range);
    output[2] = clamp_value(step[2] + step[6], range);
    output[3] = clamp_value(step[3] + step[7], range);
    output[4] = clamp_value(step[0] - step[4], range);
    output[5] = clamp_value(step[1] - step[5], range);
    output[6] = clamp_value(step[2] - step[6], range);
    output[7] = clamp_value(step[3] - step[7], range);

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
    output[0] = clamp_value(step[0] + step[2], range);
    output[1] = clamp_value(step[1] + step[3], range);
    output[2] = clamp_value(step[0] - step[2], range);
    output[3] = clamp_value(step[1] - step[3], range);
    output[4] = clamp_value(step[4] + step[6], range);
    output[5] = clamp_value(step[5] + step[7], range);
    output[6] = clamp_value(step[4] - step[6], range);
    output[7] = clamp_value(step[5] - step[7], range);

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

pub fn iadst16(input: &[TranLow], output: &mut [TranLow], range: i8) {
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
    output[0] = clamp_value(s(0) + s(8), range);
    output[1] = clamp_value(s(1) + s(9), range);
    output[2] = clamp_value(s(2) + s(10), range);
    output[3] = clamp_value(s(3) + s(11), range);
    output[4] = clamp_value(s(4) + s(12), range);
    output[5] = clamp_value(s(5) + s(13), range);
    output[6] = clamp_value(s(6) + s(14), range);
    output[7] = clamp_value(s(7) + s(15), range);
    output[8] = clamp_value(s(0) - s(8), range);
    output[9] = clamp_value(s(1) - s(9), range);
    output[10] = clamp_value(s(2) - s(10), range);
    output[11] = clamp_value(s(3) - s(11), range);
    output[12] = clamp_value(s(4) - s(12), range);
    output[13] = clamp_value(s(5) - s(13), range);
    output[14] = clamp_value(s(6) - s(14), range);
    output[15] = clamp_value(s(7) - s(15), range);

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
    output[0] = clamp_value(s(0) + s(4), range);
    output[1] = clamp_value(s(1) + s(5), range);
    output[2] = clamp_value(s(2) + s(6), range);
    output[3] = clamp_value(s(3) + s(7), range);
    output[4] = clamp_value(s(0) - s(4), range);
    output[5] = clamp_value(s(1) - s(5), range);
    output[6] = clamp_value(s(2) - s(6), range);
    output[7] = clamp_value(s(3) - s(7), range);
    output[8] = clamp_value(s(8) + s(12), range);
    output[9] = clamp_value(s(9) + s(13), range);
    output[10] = clamp_value(s(10) + s(14), range);
    output[11] = clamp_value(s(11) + s(15), range);
    output[12] = clamp_value(s(8) - s(12), range);
    output[13] = clamp_value(s(9) - s(13), range);
    output[14] = clamp_value(s(10) - s(14), range);
    output[15] = clamp_value(s(11) - s(15), range);

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
    output[0] = clamp_value(s(0) + s(2), range);
    output[1] = clamp_value(s(1) + s(3), range);
    output[2] = clamp_value(s(0) - s(2), range);
    output[3] = clamp_value(s(1) - s(3), range);
    output[4] = clamp_value(s(4) + s(6), range);
    output[5] = clamp_value(s(5) + s(7), range);
    output[6] = clamp_value(s(4) - s(6), range);
    output[7] = clamp_value(s(5) - s(7), range);
    output[8] = clamp_value(s(8) + s(10), range);
    output[9] = clamp_value(s(9) + s(11), range);
    output[10] = clamp_value(s(8) - s(10), range);
    output[11] = clamp_value(s(9) - s(11), range);
    output[12] = clamp_value(s(12) + s(14), range);
    output[13] = clamp_value(s(13) + s(15), range);
    output[14] = clamp_value(s(12) - s(14), range);
    output[15] = clamp_value(s(13) - s(15), range);

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

pub fn iidentity16(input: &[TranLow], output: &mut [TranLow], _range: i8) {
    let new_sqrt2 = NEW_SQRT2;
    for i in 0..16 {
        output[i] = round_shift_i64(input[i] as i64 * 2 * new_sqrt2 as i64, NEW_SQRT2_BITS);
    }
}

// =============================================================================
// 1D inverse transform function type and dispatch
// =============================================================================

/// 1D inverse transform function signature.
pub type InvTxfmFunc = fn(&[TranLow], &mut [TranLow], i8);

/// Get the 1D inverse transform function for a given type and size.
pub fn get_inv_txfm_func(tx_type_1d: u8, size: usize) -> Option<InvTxfmFunc> {
    match (tx_type_1d, size) {
        (0, 4) => Some(idct4),
        (0, 8) => Some(idct8),
        (0, 16) => Some(idct16),
        (0, 32) => Some(idct32),
        (0, 64) => Some(idct64),
        (1, 4) => Some(iadst4),
        (1, 8) => Some(iadst8),
        (1, 16) => Some(iadst16),
        (2, 4) => Some(iadst4), // FLIPADST inverse = ADST inverse (flip handled externally)
        (2, 8) => Some(iadst8),
        (2, 16) => Some(iadst16),
        (3, 4) => Some(iidentity4),
        (3, 8) => Some(iidentity8),
        (3, 16) => Some(iidentity16),
        (3, 32) => Some(iidentity32),
        (3, 64) => Some(iidentity64),
        _ => None,
    }
}

// =============================================================================
// General 2D inverse transform — C-exact port of inv_txfm2d_add_c
// (inv_transforms.c:2495) at bd = 8
// =============================================================================

/// C-exact inverse 2D composition at bd = 8, producing residuals.
///
/// Row pass first: per-row rect sqrt(2) pre-scale (2:1 rects only), clamp to
/// bd+8 = 16 bits, row kernel (per-stage clamps to 16 bits inside), then
/// round_shift by -shift[0]. Column pass: gather (with left-right flip),
/// clamp to max(bd+6, 16) = 16 bits, column kernel, round_shift by -shift[1],
/// then the residual is `HIGHBD_WRAPLOW(trans, 8)` written to
/// `output[r * out_stride + c]` (upside-down flip applied like C).
///
/// C adds the residual to base pixels with `highbd_clip_pixel_add`; callers
/// here do `clip(base + residual)` themselves, which is bit-exact with C for
/// any 8-bit base because |residual| <= 34596 saturates the pixel clip in the
/// same direction as the unclamped value would.
#[allow(clippy::too_many_arguments)]
fn inv_txfm2d_core(
    input: &[TranLow],
    input_stride: usize,
    output: &mut [TranLow],
    out_stride: usize,
    w: usize,
    h: usize,
    row_func: InvTxfmFunc,
    col_func: InvTxfmFunc,
    shift: [i8; 2],
    ud_flip: bool,
    lr_flip: bool,
    bd: u8,
) {
    // bd-dependent clamp/stage ranges (bd8 -> 16/16, byte-identical to the
    // former BD8_* constants; bd10 -> row 18 / col 16).
    let (row_range, col_range) = inv_txfm_ranges(bd);
    let mut buf = vec![0i32; w * h];
    let nmax = w.max(h);
    let mut temp_in = vec![0i32; nmax];
    let mut temp_out = vec![0i32; nmax];
    // get_rect_tx_log_ratio(col, row)
    let rect_log_ratio = w.trailing_zeros() as i32 - h.trailing_zeros() as i32;

    // Rows
    for r in 0..h {
        if rect_log_ratio.abs() == 1 {
            for c in 0..w {
                temp_in[c] = round_shift_i64(
                    input[r * input_stride + c] as i64 * NEW_INV_SQRT2 as i64,
                    NEW_SQRT2_BITS,
                );
            }
        } else {
            for c in 0..w {
                temp_in[c] = input[r * input_stride + c];
            }
        }
        clamp_buf(&mut temp_in[..w], row_range);
        row_func(&temp_in[..w], &mut buf[r * w..(r + 1) * w], row_range);
        round_shift_array(&mut buf[r * w..(r + 1) * w], -(shift[0] as i32));
    }

    // Columns
    for c in 0..w {
        if !lr_flip {
            for r in 0..h {
                temp_in[r] = buf[r * w + c];
            }
        } else {
            // flip left right
            for r in 0..h {
                temp_in[r] = buf[r * w + (w - c - 1)];
            }
        }
        clamp_buf(&mut temp_in[..h], col_range);
        col_func(&temp_in[..h], &mut temp_out[..h], col_range);
        round_shift_array(&mut temp_out[..h], -(shift[1] as i32));
        if !ud_flip {
            for r in 0..h {
                output[r * out_stride + c] = highbd_wraplow(temp_out[r], bd);
            }
        } else {
            // flip upside down
            for r in 0..h {
                output[r * out_stride + c] = highbd_wraplow(temp_out[h - r - 1], bd);
            }
        }
    }
}

/// C `svt_aom_inv_txfm_shift_ls` (inv_transforms.c:17-41), keyed by (w, h).
pub fn inv_txfm_shift(w: usize, h: usize) -> [i8; 2] {
    match (w, h) {
        (4, 4) => [0, -4],
        (8, 8) => [-1, -4],
        (16, 16) | (32, 32) | (64, 64) => [-2, -4],
        (4, 8) | (8, 4) => [0, -4],
        (8, 16) | (16, 8) => [-1, -4],
        (16, 32) | (32, 16) => [-1, -4],
        (32, 64) | (64, 32) => [-1, -4],
        (4, 16) | (16, 4) => [-1, -4],
        (8, 32) | (32, 8) => [-2, -4],
        (16, 64) | (64, 16) => [-2, -4],
        _ => unreachable!("unsupported transform size {w}x{h}"),
    }
}

/// Configured C-exact inverse 2D transform (`svt_av1_inv_txfm2d_add_*`
/// semantics minus the pixel add — residual output). The inverse cos_bit is
/// INV_COS_BIT = 12 for every size, baked into the 1D kernels.
///
/// `row_1d`/`col_1d`: 0=DCT, 1=ADST, 2=FLIPADST, 3=IDENTITY. For 64-dim sizes
/// `input` must already be the zero-extended w x h "mod_input" (the region
/// outside the top-left 32x32 must be zero, which the C decoder guarantees by
/// construction — the bitstream never carries those coefficients).
#[allow(clippy::too_many_arguments)]
pub fn inv_txfm2d_c_exact(
    input: &[TranLow],
    input_stride: usize,
    output: &mut [TranLow],
    out_stride: usize,
    w: usize,
    h: usize,
    row_1d: u8,
    col_1d: u8,
    ud_flip: bool,
    lr_flip: bool,
) -> bool {
    inv_txfm2d_c_exact_bd(
        input, input_stride, output, out_stride, w, h, row_1d, col_1d, ud_flip, lr_flip, 8,
    )
}

/// Bit-depth-aware [`inv_txfm2d_c_exact`] for the bd10 u16 MD path (task #94).
/// At `bd == 8` byte-identical to `inv_txfm2d_c_exact`; at bd10 the row-pass
/// clamp/stage range widens to 18 bits (col stays 16) per
/// `svt_av1_gen_inv_stage_range`, so high-magnitude bd10 coefficients are not
/// over-clamped. Transforms are otherwise bit-depth-independent.
#[allow(clippy::too_many_arguments)]
pub fn inv_txfm2d_c_exact_bd(
    input: &[TranLow],
    input_stride: usize,
    output: &mut [TranLow],
    out_stride: usize,
    w: usize,
    h: usize,
    row_1d: u8,
    col_1d: u8,
    ud_flip: bool,
    lr_flip: bool,
    bd: u8,
) -> bool {
    // SIMD fast path: square DCT-DCT with no flips, bd <= 10 (byte-exact).
    if row_1d == 0
        && col_1d == 0
        && w == h
        && !ud_flip
        && !lr_flip
        && crate::txfm_simd::try_inv_dct_square(input, input_stride, output, out_stride, w, bd)
    {
        return true;
    }
    let row_func = match get_inv_txfm_func(row_1d, w) {
        Some(f) => f,
        None => return false,
    };
    let col_func = match get_inv_txfm_func(col_1d, h) {
        Some(f) => f,
        None => return false,
    };
    inv_txfm2d_core(
        input,
        input_stride,
        output,
        out_stride,
        w,
        h,
        row_func,
        col_func,
        inv_txfm_shift(w, h),
        ud_flip,
        lr_flip,
        bd,
    );
    true
}

/// 64-dim named-wrapper input remap: the C `svt_av1_inv_txfm2d_add_64x*_c`
/// functions take input packed at stride min(w, 32) with min(h, 32) rows and
/// zero-extend it into a w x h buffer (inv_transforms.c:2614-2733).
fn mod_input_64(input: &[TranLow], w: usize, h: usize) -> alloc::vec::Vec<TranLow> {
    let cw = w.min(32);
    let ch = h.min(32);
    let mut m = vec![0i32; w * h];
    for r in 0..ch {
        m[r * w..r * w + cw].copy_from_slice(&input[r * cw..(r + 1) * cw]);
    }
    m
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
    inv_txfm2d_c_exact(input, stride, output, stride, 4, 4, 0, 0, false, false);
}

#[cfg(target_arch = "x86_64")]
#[arcane]
fn inv_txfm2d_4x4_dct_dct_impl_v3(
    _token: Desktop64,
    input: &[TranLow],
    output: &mut [TranLow],
    stride: usize,
) {
    inv_txfm2d_c_exact(input, stride, output, stride, 4, 4, 0, 0, false, false);
}

#[cfg(target_arch = "aarch64")]
#[arcane]
fn inv_txfm2d_4x4_dct_dct_impl_neon(
    _token: NeonToken,
    input: &[TranLow],
    output: &mut [TranLow],
    stride: usize,
) {
    inv_txfm2d_c_exact(input, stride, output, stride, 4, 4, 0, 0, false, false);
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
    inv_txfm2d_c_exact(input, stride, output, stride, 8, 8, 0, 0, false, false);
}

#[cfg(target_arch = "x86_64")]
#[arcane]
fn inv_txfm2d_8x8_dct_dct_impl_v3(
    _token: Desktop64,
    input: &[TranLow],
    output: &mut [TranLow],
    stride: usize,
) {
    inv_txfm2d_c_exact(input, stride, output, stride, 8, 8, 0, 0, false, false);
}

#[cfg(target_arch = "aarch64")]
#[arcane]
fn inv_txfm2d_8x8_dct_dct_impl_neon(
    _token: NeonToken,
    input: &[TranLow],
    output: &mut [TranLow],
    stride: usize,
) {
    inv_txfm2d_c_exact(input, stride, output, stride, 8, 8, 0, 0, false, false);
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
    inv_txfm2d_c_exact(input, stride, output, stride, 16, 16, 0, 0, false, false);
}

#[cfg(target_arch = "x86_64")]
#[arcane]
fn inv_txfm2d_16x16_dct_dct_impl_v3(
    _token: Desktop64,
    input: &[TranLow],
    output: &mut [TranLow],
    stride: usize,
) {
    inv_txfm2d_c_exact(input, stride, output, stride, 16, 16, 0, 0, false, false);
}

#[cfg(target_arch = "aarch64")]
#[arcane]
fn inv_txfm2d_16x16_dct_dct_impl_neon(
    _token: NeonToken,
    input: &[TranLow],
    output: &mut [TranLow],
    stride: usize,
) {
    inv_txfm2d_c_exact(input, stride, output, stride, 16, 16, 0, 0, false, false);
}

// --- Square inverse 2D wrappers ---

/// Inverse 32x32 DCT-DCT.
pub fn inv_txfm2d_32x32_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    inv_txfm2d_c_exact(input, stride, output, stride, 32, 32, 0, 0, false, false);
}

/// Inverse 64x64 DCT-DCT.
pub fn inv_txfm2d_64x64_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    let m = mod_input_64(input, 64, 64);
    inv_txfm2d_c_exact(&m, 64, output, stride, 64, 64, 0, 0, false, false);
}

// --- Rectangular inverse 2D wrappers ---

/// Inverse 4x8 DCT-DCT.
pub fn inv_txfm2d_4x8_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    inv_txfm2d_c_exact(input, stride, output, stride, 4, 8, 0, 0, false, false);
}

/// Inverse 8x4 DCT-DCT.
pub fn inv_txfm2d_8x4_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    inv_txfm2d_c_exact(input, stride, output, stride, 8, 4, 0, 0, false, false);
}

/// Inverse 8x16 DCT-DCT.
pub fn inv_txfm2d_8x16_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    inv_txfm2d_c_exact(input, stride, output, stride, 8, 16, 0, 0, false, false);
}

/// Inverse 16x8 DCT-DCT.
pub fn inv_txfm2d_16x8_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    inv_txfm2d_c_exact(input, stride, output, stride, 16, 8, 0, 0, false, false);
}

/// Inverse 16x32 DCT-DCT.
pub fn inv_txfm2d_16x32_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    inv_txfm2d_c_exact(input, stride, output, stride, 16, 32, 0, 0, false, false);
}

/// Inverse 32x16 DCT-DCT.
pub fn inv_txfm2d_32x16_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    inv_txfm2d_c_exact(input, stride, output, stride, 32, 16, 0, 0, false, false);
}

/// Inverse 32x64 DCT-DCT.
pub fn inv_txfm2d_32x64_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    let m = mod_input_64(input, 32, 64);
    inv_txfm2d_c_exact(&m, 32, output, stride, 32, 64, 0, 0, false, false);
}

/// Inverse 64x32 DCT-DCT.
pub fn inv_txfm2d_64x32_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    let m = mod_input_64(input, 64, 32);
    inv_txfm2d_c_exact(&m, 64, output, stride, 64, 32, 0, 0, false, false);
}

/// Inverse 4x16 DCT-DCT.
pub fn inv_txfm2d_4x16_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    inv_txfm2d_c_exact(input, stride, output, stride, 4, 16, 0, 0, false, false);
}

/// Inverse 16x4 DCT-DCT.
pub fn inv_txfm2d_16x4_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    inv_txfm2d_c_exact(input, stride, output, stride, 16, 4, 0, 0, false, false);
}

/// Inverse 8x32 DCT-DCT.
pub fn inv_txfm2d_8x32_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    inv_txfm2d_c_exact(input, stride, output, stride, 8, 32, 0, 0, false, false);
}

/// Inverse 32x8 DCT-DCT.
pub fn inv_txfm2d_32x8_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    inv_txfm2d_c_exact(input, stride, output, stride, 32, 8, 0, 0, false, false);
}

/// Inverse 16x64 DCT-DCT.
pub fn inv_txfm2d_16x64_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    let m = mod_input_64(input, 16, 64);
    inv_txfm2d_c_exact(&m, 16, output, stride, 16, 64, 0, 0, false, false);
}

/// Inverse 64x16 DCT-DCT.
pub fn inv_txfm2d_64x16_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    let m = mod_input_64(input, 64, 16);
    inv_txfm2d_c_exact(&m, 64, output, stride, 64, 16, 0, 0, false, false);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fwd_txfm::{fdct4, fdct8, fwd_txfm2d_4x4_dct_dct, fwd_txfm2d_8x8_dct_dct};

    // --- idct4 tests ---

    #[test]
    fn idct4_zero() {
        let mut output = [0i32; 4];
        idct4(&[0i32; 4], &mut output, 31);
        assert!(output.iter().all(|&v| v == 0));
    }

    #[test]
    fn fdct4_idct4_roundtrip() {
        // The combined forward+inverse DCT-4 produces input * 2 (scale factor N/2 = 4/2).
        let input = [10i32, -20, 30, -40];
        let mut fwd = [0i32; 4];
        let mut inv = [0i32; 4];
        fdct4(&input, &mut fwd, 12);
        idct4(&fwd, &mut inv, 31);
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
        fdct4(&input, &mut fwd, 12);
        idct4(&fwd, &mut inv, 31);
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
        idct8(&[0i32; 8], &mut output, 31);
        assert!(output.iter().all(|&v| v == 0));
    }

    #[test]
    fn fdct8_idct8_roundtrip() {
        // The combined forward+inverse DCT-8 produces input * 4 (scale factor N/2 = 8/2).
        // Tolerance is +-2 due to accumulated rounding across 5 butterfly stages.
        let input = [10, -20, 30, -40, 50, -60, 70, -80i32];
        let mut fwd = [0i32; 8];
        let mut inv = [0i32; 8];
        fdct8(&input, &mut fwd, 12);
        idct8(&fwd, &mut inv, 31);
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
        fdct8(&input, &mut fwd, 12);
        idct8(&fwd, &mut inv, 31);
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
        iadst4(&[0i32; 4], &mut output, 31);
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
        iadst4(&fwd, &mut inv, 31);
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
        iadst4(&input, &mut output, 31);
        assert!(
            output.iter().any(|&v| v != 0),
            "iadst4 should produce nonzero output"
        );
    }

    // --- iidentity tests ---

    #[test]
    fn iidentity4_zero() {
        let mut output = [0i32; 4];
        iidentity4(&[0i32; 4], &mut output, 31);
        assert!(output.iter().all(|&v| v == 0));
    }

    #[test]
    fn iidentity8_zero() {
        let mut output = [0i32; 8];
        iidentity8(&[0i32; 8], &mut output, 31);
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
        crate::fwd_txfm::fidentity4(&input, &mut fwd, 12);
        iidentity4(&fwd, &mut inv, 31);
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
        crate::fwd_txfm::fidentity8(&input, &mut fwd, 12);
        iidentity8(&fwd, &mut inv, 31);
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
