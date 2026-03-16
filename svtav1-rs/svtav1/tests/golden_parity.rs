//! Golden parity tests — verify Rust produces bit-exact output vs C SVT-AV1.
//!
//! Golden values extracted from C SVT-AV1 using tools/extract_golden.c.
//! Every value is MEASURED, not estimated.

use svtav1_dsp::fwd_txfm::*;

/// Assert two i32 arrays are identical. Print detailed diff on failure.
fn assert_exact(name: &str, rust: &[i32], c_golden: &[i32]) {
    assert_eq!(rust.len(), c_golden.len(), "{name}: length mismatch");
    let mut mismatches = Vec::new();
    for i in 0..rust.len() {
        if rust[i] != c_golden[i] {
            mismatches.push((i, rust[i], c_golden[i]));
        }
    }
    if !mismatches.is_empty() {
        eprintln!("MISMATCH in {name}:");
        eprintln!("  rust:   {:?}", rust);
        eprintln!("  golden: {:?}", c_golden);
        for (i, r, g) in &mismatches {
            eprintln!("  [{i}]: rust={r}, golden={g}, diff={}", r - g);
        }
        panic!(
            "{name}: {} mismatches out of {} coefficients",
            mismatches.len(),
            rust.len()
        );
    }
}

// =============================================================================
// fdct4 golden parity
// =============================================================================

#[test]
fn fdct4_dc_golden() {
    let input = [100i32, 100, 100, 100];
    let golden = [283, 0, 0, 0];
    let mut output = [0i32; 4];
    fdct4(&input, &mut output);
    assert_exact("fdct4_dc", &output, &golden);
}

#[test]
fn fdct4_zero_golden() {
    let input = [0i32; 4];
    let golden = [0, 0, 0, 0];
    let mut output = [0i32; 4];
    fdct4(&input, &mut output);
    assert_exact("fdct4_zero", &output, &golden);
}

#[test]
fn fdct4_mixed_golden() {
    let input = [100i32, -50, 200, -150];
    let golden = [71, 135, -141, 327];
    let mut output = [0i32; 4];
    fdct4(&input, &mut output);
    assert_exact("fdct4_mixed", &output, &golden);
}

#[test]
fn fdct4_impulse_golden() {
    let input = [1i32, 0, 0, 0];
    let golden = [1, 1, 1, 0];
    let mut output = [0i32; 4];
    fdct4(&input, &mut output);
    assert_exact("fdct4_impulse", &output, &golden);
}

#[test]
fn fdct4_alt_golden() {
    let input = [1i32, -1, 1, -1];
    let golden = [0, 1, 0, 3];
    let mut output = [0i32; 4];
    fdct4(&input, &mut output);
    assert_exact("fdct4_alt", &output, &golden);
}

// =============================================================================
// fdct8 golden parity
// =============================================================================

#[test]
fn fdct8_dc_golden() {
    let input = [100i32; 8];
    let golden = [566, 0, 0, 0, 0, 0, 0, 0];
    let mut output = [0i32; 8];
    fdct8(&input, &mut output);
    assert_exact("fdct8_dc", &output, &golden);
}

#[test]
fn fdct8_zero_golden() {
    let input = [0i32; 8];
    let golden = [0, 0, 0, 0, 0, 0, 0, 0];
    let mut output = [0i32; 8];
    fdct8(&input, &mut output);
    assert_exact("fdct8_zero", &output, &golden);
}

#[test]
fn fdct8_mixed_golden() {
    let input = [50i32, -25, 100, -75, 200, -150, 80, -40];
    let golden = [99, 87, -66, 3, 92, -27, -141, 554];
    let mut output = [0i32; 8];
    fdct8(&input, &mut output);
    assert_exact("fdct8_mixed", &output, &golden);
}

#[test]
fn fdct8_alt_golden() {
    let input = [1i32, -1, 1, -1, 1, -1, 1, -1];
    let golden = [0, 1, 0, 1, 0, 2, 0, 5];
    let mut output = [0i32; 8];
    fdct8(&input, &mut output);
    assert_exact("fdct8_alt", &output, &golden);
}

// =============================================================================
// fdct16 golden parity
// =============================================================================

#[test]
fn fdct16_dc_golden() {
    let input = [50i32; 16];
    let golden = [566, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    let mut output = [0i32; 16];
    fdct16(&input, &mut output);
    assert_exact("fdct16_dc", &output, &golden);
}

#[test]
fn fdct16_ramp_golden() {
    let mut input = [0i32; 16];
    for i in 0..16 {
        input[i] = i as i32 * 10 - 80;
    }
    let golden = [
        -57, -517, 0, -57, 0, -20, 0, -10, 0, -5, 0, -3, 0, -2, 0, -1,
    ];
    let mut output = [0i32; 16];
    fdct16(&input, &mut output);
    assert_exact("fdct16_ramp", &output, &golden);
}

// =============================================================================
// fadst4 golden parity
// =============================================================================

#[test]
fn fadst4_zero_golden() {
    let input = [0i32; 4];
    let golden = [0, 0, 0, 0];
    let mut output = [0i32; 4];
    fadst4(&input, &mut output);
    assert_exact("fadst4_zero", &output, &golden);
}

#[test]
fn fadst4_mixed_golden() {
    let input = [100i32, -50, 200, -150];
    let golden = [26, 163, -145, 319];
    let mut output = [0i32; 4];
    fadst4(&input, &mut output);
    assert_exact("fadst4_mixed", &output, &golden);
}

// =============================================================================
// fadst8 golden parity
// =============================================================================

#[test]
fn fadst8_zero_golden() {
    let input = [0i32; 8];
    let golden = [0, 0, 0, 0, 0, 0, 0, 0];
    let mut output = [0i32; 8];
    fadst8(&input, &mut output);
    assert_exact("fadst8_zero", &output, &golden);
}

#[test]
fn fadst8_mixed_golden() {
    let input = [50i32, -25, 100, -75, 200, -150, 80, -40];
    let golden = [56, 125, -19, -40, 84, 33, -360, 445];
    let mut output = [0i32; 8];
    fadst8(&input, &mut output);
    assert_exact("fadst8_mixed", &output, &golden);
}

// =============================================================================
// Cosine table parity
// =============================================================================

#[test]
fn cospi_table_matches_c() {
    // C: cospi_q12[0]=4096 [16]=3784 [32]=2896 [48]=1567 [63]=101
    assert_eq!(COSPI[0], 4096);
    assert_eq!(COSPI[16], 3784);
    assert_eq!(COSPI[32], 2896);
    assert_eq!(COSPI[48], 1567);
    assert_eq!(COSPI[63], 101);
}

#[test]
fn sinpi_table_matches_c() {
    // C: sinpi_q12: [0, 1321, 2482, 3344, 3803]
    assert_eq!(SINPI[0], 0);
    assert_eq!(SINPI[1], 1321);
    assert_eq!(SINPI[2], 2482);
    assert_eq!(SINPI[3], 3344);
    assert_eq!(SINPI[4], 3803);
}
