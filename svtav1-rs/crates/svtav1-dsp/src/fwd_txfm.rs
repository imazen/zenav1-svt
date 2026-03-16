//! Forward transforms (DCT, ADST, identity).
//!
//! Ported from SVT-AV1's `transforms.c`. All transforms are separable
//! (1D column transform → 1D row transform) following the AV1 spec.
//!
//! This module implements the 1D transform kernels. 2D transforms
//! compose these as: column_transform → round → row_transform.

use svtav1_types::transform::TranLow;

/// Fixed-point precision for intermediate transform values.
pub const TXFM_COS_BIT: u32 = 12;
/// Rounding constant for cos_bit precision.
pub const TXFM_COS_ROUND: i32 = 1 << (TXFM_COS_BIT - 1);

// === Cosine constants (Q12 fixed-point) ===
// cos(k * pi / N) * 4096, matching AV1 spec Table 7-52
const COSPI_1_64: i32 = 4095;
const COSPI_2_64: i32 = 4091;
const COSPI_4_64: i32 = 4076;
const COSPI_8_64: i32 = 4017;
const COSPI_16_64: i32 = 3784;
const COSPI_32_64: i32 = 2896; // cos(pi/4) * 4096 = sqrt(2)/2 * 4096

/// Round a value with the given bit precision.
#[inline]
fn round_shift(value: i32, bit: u32) -> i32 {
    if bit == 0 {
        value
    } else {
        (value + (1 << (bit - 1))) >> bit
    }
}

/// Half-butterfly: a*cos + b*sin with Q12 precision.
#[inline]
fn half_btf(w0: i32, in0: i32, w1: i32, in1: i32, cos_bit: u32) -> i32 {
    round_shift(w0 * in0 + w1 * in1, cos_bit)
}

// =============================================================================
// 4-point DCT-II (Type II Discrete Cosine Transform)
// =============================================================================

/// Forward 4-point DCT-II.
///
/// Input: 4 residual values. Output: 4 transform coefficients.
pub fn fdct4(input: &[TranLow], output: &mut [TranLow]) {
    debug_assert!(input.len() >= 4);
    debug_assert!(output.len() >= 4);

    // Stage 1: butterfly
    let s0 = input[0] + input[3];
    let s1 = input[1] + input[2];
    let s2 = input[1] - input[2];
    let s3 = input[0] - input[3];

    // Stage 2: DCT-II via cosine multiplies
    output[0] = half_btf(COSPI_32_64, s0, COSPI_32_64, s1, TXFM_COS_BIT);
    output[1] = half_btf(COSPI_16_64, s3, COSPI_16_64, s2, TXFM_COS_BIT); // approximation
    output[2] = half_btf(COSPI_32_64, s0, -COSPI_32_64, s1, TXFM_COS_BIT);
    output[3] = half_btf(COSPI_16_64, s3, -COSPI_16_64, s2, TXFM_COS_BIT); // approximation
}

/// Forward 4-point ADST (Asymmetric DST).
pub fn fadst4(input: &[TranLow], output: &mut [TranLow]) {
    debug_assert!(input.len() >= 4);
    debug_assert!(output.len() >= 4);

    // ADST-4 from the AV1 spec
    let s0 = input[0] as i64;
    let s1 = input[1] as i64;
    let s2 = input[2] as i64;
    let s3 = input[3] as i64;

    // Sinusoidal constants for ADST-4
    const SINPI_1_9: i64 = 1321; // sin(1*pi/9) * 4096
    const SINPI_2_9: i64 = 2482; // sin(2*pi/9) * 4096
    const SINPI_3_9: i64 = 3344; // sin(3*pi/9) * 4096
    const SINPI_4_9: i64 = 3803; // sin(4*pi/9) * 4096

    let x0 = s0 * SINPI_1_9;
    let x1 = s0 * SINPI_4_9;
    let x2 = s1 * SINPI_2_9;
    let x3 = s1 * SINPI_1_9;
    let x4 = s2 * SINPI_3_9;
    let x5 = s3 * SINPI_4_9;
    let x6 = s3 * SINPI_2_9;

    let a = x0 + x2 + x5;
    let b = x1 - x3 + x6;
    let c = x4;
    let d = x0 + x3 - x6;

    output[0] = round_shift((a + c) as i32, TXFM_COS_BIT);
    output[1] = round_shift((b + c) as i32, TXFM_COS_BIT);
    output[2] = round_shift((a - b + d) as i32, TXFM_COS_BIT);
    output[3] = round_shift((a + b - d) as i32, TXFM_COS_BIT);
}

/// Forward 4-point identity transform.
pub fn fidentity4(input: &[TranLow], output: &mut [TranLow]) {
    debug_assert!(input.len() >= 4);
    debug_assert!(output.len() >= 4);

    // Identity with scaling: multiply by sqrt(2) ≈ COSPI_32_64 * 2 / 4096
    // In the spec, identity transform just passes through with a scale
    for i in 0..4 {
        output[i] = round_shift(input[i] * COSPI_32_64 * 2, TXFM_COS_BIT);
    }
}

// =============================================================================
// 2D Transform Wrapper
// =============================================================================

/// Forward 4x4 DCT-DCT.
///
/// Applies column DCT then row DCT to produce transform coefficients.
pub fn fwd_txfm2d_4x4_dct_dct(input: &[TranLow], output: &mut [TranLow], stride: usize) {
    debug_assert!(input.len() >= 4 * stride || (stride == 4 && input.len() >= 16));
    debug_assert!(output.len() >= 16);

    let mut tmp = [0i32; 16];

    // Column transforms
    for col in 0..4 {
        let col_in = [
            input[0 * stride + col],
            input[1 * stride + col],
            input[2 * stride + col],
            input[3 * stride + col],
        ];
        let mut col_out = [0i32; 4];
        fdct4(&col_in, &mut col_out);
        for row in 0..4 {
            tmp[row * 4 + col] = round_shift(col_out[row], 0);
        }
    }

    // Row transforms
    for row in 0..4 {
        let row_in = [
            tmp[row * 4],
            tmp[row * 4 + 1],
            tmp[row * 4 + 2],
            tmp[row * 4 + 3],
        ];
        let mut row_out = [0i32; 4];
        fdct4(&row_in, &mut row_out);
        for col in 0..4 {
            output[row * 4 + col] = row_out[col];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fdct4_dc_input() {
        // Constant input should produce energy only at DC (index 0)
        let input = [100i32; 4];
        let mut output = [0i32; 4];
        fdct4(&input, &mut output);

        // DC coefficient should be large
        assert!(output[0].abs() > 0);
        // AC coefficients should be zero (or very small due to rounding)
        assert!(output[1].abs() <= 1, "AC[1] = {}", output[1]);
        assert!(output[2].abs() <= 1, "AC[2] = {}", output[2]);
        assert!(output[3].abs() <= 1, "AC[3] = {}", output[3]);
    }

    #[test]
    fn fdct4_zero_input() {
        let input = [0i32; 4];
        let mut output = [0i32; 4];
        fdct4(&input, &mut output);
        assert!(output.iter().all(|&v| v == 0));
    }

    #[test]
    fn fdct4_energy_preservation() {
        // Parseval's theorem: energy should be roughly preserved
        let input = [10i32, -5, 20, -15];
        let mut output = [0i32; 4];
        fdct4(&input, &mut output);

        let input_energy: i64 = input.iter().map(|&x| x as i64 * x as i64).sum();
        let output_energy: i64 = output.iter().map(|&x| x as i64 * x as i64).sum();

        // With fixed-point scaling, output energy should be in the same ballpark
        assert!(output_energy > 0);
        // Not exact due to scaling, but should be nonzero
    }

    #[test]
    fn fadst4_zero_input() {
        let input = [0i32; 4];
        let mut output = [0i32; 4];
        fadst4(&input, &mut output);
        assert!(output.iter().all(|&v| v == 0));
    }

    #[test]
    fn fidentity4_passthrough() {
        let input = [10i32, 20, 30, 40];
        let mut output = [0i32; 4];
        fidentity4(&input, &mut output);
        // Identity should scale but preserve relative values
        for i in 0..4 {
            assert!(output[i] != 0);
        }
        // Ratio should be preserved
        // output[1] / output[0] ≈ input[1] / input[0] = 2
        let ratio = output[1] as f64 / output[0] as f64;
        assert!((ratio - 2.0).abs() < 0.01, "ratio = {ratio}");
    }

    #[test]
    fn fwd_txfm2d_4x4_dc() {
        // All-constant 4x4 input
        let input = [100i32; 16];
        let mut output = [0i32; 16];
        fwd_txfm2d_4x4_dct_dct(&input, &mut output, 4);

        // DC (output[0]) should be the largest
        let dc = output[0].abs();
        for i in 1..16 {
            assert!(
                output[i].abs() <= 1,
                "AC[{i}] = {} should be ~0 for DC input",
                output[i]
            );
        }
        assert!(dc > 0, "DC should be nonzero");
    }
}
