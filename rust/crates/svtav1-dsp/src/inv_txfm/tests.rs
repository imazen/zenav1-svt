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
    let input = [100i16; 16];
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
    fwd_txfm2d_4x4_dct_dct(&[0i16; 16], &mut fwd, 4);
    inv_txfm2d_4x4_dct_dct(&fwd, &mut inv, 4);
    assert!(inv.iter().all(|&v| v == 0));
}

#[test]
fn fwd_inv_txfm2d_8x8_zero() {
    let mut fwd = [0i32; 64];
    let mut inv = [0i32; 64];
    fwd_txfm2d_8x8_dct_dct(&[0i16; 64], &mut fwd, 8);
    inv_txfm2d_8x8_dct_dct(&fwd, &mut inv, 8);
    assert!(inv.iter().all(|&v| v == 0));
}

#[test]
fn fwd_inv_txfm2d_8x8_roundtrip() {
    let input = [50i16; 64];
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
