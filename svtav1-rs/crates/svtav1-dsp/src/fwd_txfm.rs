//! Forward transforms (DCT, ADST, identity).
//!
//! Ported from SVT-AV1's `transforms.c` and `inv_transforms.c`.
//! All transforms are separable (1D column → 1D row) per AV1 spec.
//!
//! Cosine constants from `svt_aom_eb_av1_cospi_arr_data` in `inv_transforms.c`.

use alloc::vec;
use svtav1_types::transform::TranLow;

// =============================================================================
// Cosine constant table — Q12 row (cos_bit = 12)
// cospi[j] = round(cos(j * pi / 128) * 2^12)
// =============================================================================
pub const COSPI: [i32; 64] = [
    4096, 4095, 4091, 4085, 4076, 4065, 4052, 4036, 4017, 3996, 3973, 3948, 3920, 3889, 3857, 3822,
    3784, 3745, 3703, 3659, 3612, 3564, 3513, 3461, 3406, 3349, 3290, 3229, 3166, 3102, 3035, 2967,
    2896, 2824, 2751, 2675, 2598, 2520, 2440, 2359, 2276, 2191, 2106, 2019, 1931, 1842, 1751, 1660,
    1567, 1474, 1380, 1285, 1189, 1092, 995, 897, 799, 700, 601, 501, 401, 301, 201, 101,
];

/// Sinusoidal constants for ADST-4 (Q12).
/// sinpi[j] = round(sqrt(2) * sin(j*pi/9) * 2/3 * 2^12)
pub const SINPI: [i32; 5] = [0, 1321, 2482, 3344, 3803];

/// Default cos_bit for transforms.
pub const COS_BIT: u32 = 12;

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

pub fn fdct4(input: &[TranLow], output: &mut [TranLow]) {
    let cospi = &COSPI;
    let cos_bit = COS_BIT;

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

pub fn fdct8(input: &[TranLow], output: &mut [TranLow]) {
    let cospi = &COSPI;
    let cos_bit = COS_BIT;
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

pub fn fdct16(input: &[TranLow], output: &mut [TranLow]) {
    let cospi = &COSPI;
    let cos_bit = COS_BIT;
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
// 4-point ADST
// Ported from svt_av1_fadst4_new in transforms.c
// =============================================================================

pub fn fadst4(input: &[TranLow], output: &mut [TranLow]) {
    let sinpi = &SINPI;
    let cos_bit = COS_BIT;

    let s0 = input[0] as i64;
    let s1 = input[1] as i64;
    let s2 = input[2] as i64;
    let s3 = input[3] as i64;

    let x0 = s0 * sinpi[1] as i64;
    let x1 = s0 * sinpi[4] as i64;
    let x2 = s1 * sinpi[2] as i64;
    let x3 = s1 * sinpi[1] as i64;
    let x4 = s2 * sinpi[3] as i64;
    let x5 = s3 * sinpi[4] as i64;
    let x6 = s3 * sinpi[2] as i64;

    let a = x0 + x2 + x5;
    let b = x1 - x3 + x6;
    let c = x4;
    let d = x0 + x3 - x6;

    output[0] = round_shift_i64(a + c, cos_bit);
    output[1] = round_shift_i64(b + c, cos_bit);
    output[2] = round_shift_i64(a - b + d, cos_bit);
    output[3] = round_shift_i64(a + b - d, cos_bit);
}

// =============================================================================
// 4-point identity transform
// =============================================================================

pub fn fidentity4(input: &[TranLow], output: &mut [TranLow]) {
    let new_sqrt2 = NEW_SQRT2;
    for i in 0..4 {
        output[i] = round_shift_i64(input[i] as i64 * new_sqrt2 as i64, NEW_SQRT2_BITS);
    }
}

// =============================================================================
// 8-point ADST
// Ported from svt_av1_fadst8_new in transforms.c
// =============================================================================

pub fn fadst8(input: &[TranLow], output: &mut [TranLow]) {
    let cospi = &COSPI;
    let cos_bit = COS_BIT;
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

    // stage 7 (output permutation)
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
// 8-point identity transform
// =============================================================================

pub fn fidentity8(input: &[TranLow], output: &mut [TranLow]) {
    for i in 0..8 {
        output[i] = input[i] * 2;
    }
}

// =============================================================================
// 16-point identity transform
// =============================================================================

pub fn fidentity16(input: &[TranLow], output: &mut [TranLow]) {
    let new_sqrt2 = NEW_SQRT2;
    for i in 0..16 {
        output[i] = round_shift_i64(input[i] as i64 * 2 * new_sqrt2 as i64, NEW_SQRT2_BITS);
    }
}

// =============================================================================
// 1D Transform function type and dispatch
// =============================================================================

/// 1D forward transform function signature.
pub type TxfmFunc = fn(&[TranLow], &mut [TranLow]);

/// Get the 1D forward transform function for a given type and size.
pub fn get_fwd_txfm_func(tx_type_1d: u8, size: usize) -> Option<TxfmFunc> {
    // tx_type_1d: 0=DCT, 1=ADST, 2=FLIPADST, 3=IDENTITY
    match (tx_type_1d, size) {
        (0, 4) => Some(fdct4),
        (0, 8) => Some(fdct8),
        (0, 16) => Some(fdct16),
        (1, 4) => Some(fadst4),
        (1, 8) => Some(fadst8),
        (2, 4) => Some(fadst4), // FLIPADST uses ADST with flipped input
        (2, 8) => Some(fadst8),
        (3, 4) => Some(fidentity4),
        (3, 8) => Some(fidentity8),
        (3, 16) => Some(fidentity16),
        _ => None,
    }
}

// =============================================================================
// General 2D forward transform
// =============================================================================

/// Forward 2D transform for square blocks.
///
/// Applies column transforms, then row transforms, following the
/// SVT-AV1 `av1_tranform_two_d_core_c` pattern.
///
/// `shift` = [pre_shift, mid_shift, post_shift] applied at each stage.
pub fn fwd_txfm2d(
    input: &[TranLow],
    output: &mut [TranLow],
    stride: usize,
    col_func: TxfmFunc,
    row_func: TxfmFunc,
    size: usize,
    shift: [i32; 3],
) {
    let mut buf = vec![0i32; size * size];
    let mut temp_in = vec![0i32; size];
    let mut temp_out = vec![0i32; size];

    // Column transforms
    for col in 0..size {
        for row in 0..size {
            temp_in[row] = input[row * stride + col];
        }
        round_shift_array(&mut temp_in, -shift[0]);
        col_func(&temp_in, &mut temp_out);
        round_shift_array(&mut temp_out, -shift[1]);
        for row in 0..size {
            buf[row * size + col] = temp_out[row];
        }
    }

    // Row transforms
    for row in 0..size {
        let row_start = row * size;
        temp_in[..size].copy_from_slice(&buf[row_start..row_start + size]);
        row_func(&temp_in, &mut temp_out);
        round_shift_array(&mut temp_out, -shift[2]);
        output[row_start..row_start + size].copy_from_slice(&temp_out[..size]);
    }
}

/// Forward 4x4 DCT-DCT using the general framework.
pub fn fwd_txfm2d_4x4_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    // Shift values for 4x4 8-bit: [2, 0, 0]
    fwd_txfm2d(input, output, stride, fdct4, fdct4, 4, [2, 0, 0]);
}

/// Forward 8x8 DCT-DCT.
pub fn fwd_txfm2d_8x8_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    // Shift values for 8x8 8-bit: [2, -1, 0]
    fwd_txfm2d(input, output, stride, fdct8, fdct8, 8, [2, -1, 0]);
}

/// Forward 16x16 DCT-DCT.
pub fn fwd_txfm2d_16x16_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    // Shift values for 16x16 8-bit: [2, -2, 0]
    fwd_txfm2d(input, output, stride, fdct16, fdct16, 16, [2, -2, 0]);
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- fdct4 tests ---

    #[test]
    fn fdct4_dc_input() {
        let input = [100i32; 4];
        let mut output = [0i32; 4];
        fdct4(&input, &mut output);
        assert!(output[0].abs() > 0, "DC should be nonzero");
        for i in 1..4 {
            assert!(output[i].abs() <= 1, "AC[{i}] = {}", output[i]);
        }
    }

    #[test]
    fn fdct4_zero() {
        let input = [0i32; 4];
        let mut output = [0i32; 4];
        fdct4(&input, &mut output);
        assert!(output.iter().all(|&v| v == 0));
    }

    // --- fdct8 tests ---

    #[test]
    fn fdct8_dc_input() {
        let input = [100i32; 8];
        let mut output = [0i32; 8];
        fdct8(&input, &mut output);
        assert!(output[0].abs() > 0, "DC should be nonzero");
        for i in 1..8 {
            assert!(output[i].abs() <= 1, "AC[{i}] = {}", output[i]);
        }
    }

    #[test]
    fn fdct8_zero() {
        let mut output = [0i32; 8];
        fdct8(&[0i32; 8], &mut output);
        assert!(output.iter().all(|&v| v == 0));
    }

    #[test]
    fn fdct8_alternating() {
        // Alternating +1/-1 should produce energy in higher frequencies
        let input = [1, -1, 1, -1, 1, -1, 1, -1i32];
        let mut output = [0i32; 8];
        fdct8(&input, &mut output);
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
        fdct16(&input, &mut output);
        assert!(output[0].abs() > 0, "DC should be nonzero");
        for i in 1..16 {
            assert!(output[i].abs() <= 1, "AC[{i}] = {}", output[i]);
        }
    }

    #[test]
    fn fdct16_zero() {
        let mut output = [0i32; 16];
        fdct16(&[0i32; 16], &mut output);
        assert!(output.iter().all(|&v| v == 0));
    }

    // --- fadst tests ---

    #[test]
    fn fadst4_zero() {
        let mut output = [0i32; 4];
        fadst4(&[0i32; 4], &mut output);
        assert!(output.iter().all(|&v| v == 0));
    }

    #[test]
    fn fadst8_zero() {
        let mut output = [0i32; 8];
        fadst8(&[0i32; 8], &mut output);
        assert!(output.iter().all(|&v| v == 0));
    }

    // --- identity tests ---

    #[test]
    fn fidentity4_ratio() {
        let input = [10i32, 20, 30, 40];
        let mut output = [0i32; 4];
        fidentity4(&input, &mut output);
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
        fidentity8(&input, &mut output);
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
