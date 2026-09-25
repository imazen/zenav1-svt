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
    let input = [100i16; 16];
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
    let input = [50i16; 64];
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
    let input = [30i16; 256];
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
    fwd_txfm2d_4x4_dct_dct(&[0i16; 16], &mut output, 4);
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
