//! Golden parity tests — verify Rust produces bit-exact output vs C SVT-AV1.
//!
//! Golden values extracted from C SVT-AV1 using tools/extract_golden.c.
//! Every value is MEASURED, not estimated.

use svtav1_dsp::fwd_txfm::*;
use svtav1_dsp::inv_txfm::*;

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
// fdct32 golden parity
// =============================================================================

#[test]
fn fdct32_dc_golden() {
    let input = [100i32; 32];
    let golden = [
        2263, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0,
    ];
    let mut output = [0i32; 32];
    fdct32(&input, &mut output);
    assert_exact("fdct32_dc", &output, &golden);
}

#[test]
fn fdct32_ramp_golden() {
    let mut input = [0i32; 32];
    for i in 0..32 {
        input[i] = i as i32 * 5 - 80;
    }
    let golden = [
        -57, -1036, 0, -115, 0, -41, 0, -21, 0, -13, 0, -8, 0, -5, 0, -4, 0, -3, 0, -3, 0, -2, 0,
        -2, 0, -1, 0, 0, 0, 0, 0, 0,
    ];
    let mut output = [0i32; 32];
    fdct32(&input, &mut output);
    assert_exact("fdct32_ramp", &output, &golden);
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

// =============================================================================
// fidentity4 golden parity
// =============================================================================

#[test]
fn fidentity4_golden() {
    let input = [100i32, 200, 300, 400];
    let golden = [141, 283, 424, 566];
    let mut output = [0i32; 4];
    fidentity4(&input, &mut output);
    assert_exact("fidentity4", &output, &golden);
}

// =============================================================================
// fidentity8 golden parity
// =============================================================================

#[test]
fn fidentity8_golden() {
    let input = [100i32; 8];
    let golden = [200; 8];
    let mut output = [0i32; 8];
    fidentity8(&input, &mut output);
    assert_exact("fidentity8", &output, &golden);
}

// =============================================================================
// idct4 golden parity
// =============================================================================

#[test]
fn idct4_dc_golden() {
    let input = [283i32, 0, 0, 0];
    let golden = [200, 200, 200, 200];
    let mut output = [0i32; 4];
    idct4(&input, &mut output);
    assert_exact("idct4_dc", &output, &golden);
}

#[test]
fn idct4_zero_golden() {
    let input = [0i32; 4];
    let golden = [0, 0, 0, 0];
    let mut output = [0i32; 4];
    idct4(&input, &mut output);
    assert_exact("idct4_zero", &output, &golden);
}

#[test]
fn idct4_from_fdct4_mixed_golden() {
    let input = [71i32, 135, -141, 327];
    let golden = [201, -100, 400, -299];
    let mut output = [0i32; 4];
    idct4(&input, &mut output);
    assert_exact("idct4_from_fdct4_mixed", &output, &golden);
}

// =============================================================================
// idct8 golden parity
// =============================================================================

#[test]
fn idct8_dc_golden() {
    let input = [566i32, 0, 0, 0, 0, 0, 0, 0];
    let golden = [400, 400, 400, 400, 400, 400, 400, 400];
    let mut output = [0i32; 8];
    idct8(&input, &mut output);
    assert_exact("idct8_dc", &output, &golden);
}

#[test]
fn idct8_zero_golden() {
    let input = [0i32; 8];
    let golden = [0, 0, 0, 0, 0, 0, 0, 0];
    let mut output = [0i32; 8];
    idct8(&input, &mut output);
    assert_exact("idct8_zero", &output, &golden);
}

#[test]
fn idct8_from_fdct8_mixed_golden() {
    let input = [99i32, 87, -66, 3, 92, -27, -141, 554];
    let golden = [200, -99, 401, -300, 800, -601, 319, -160];
    let mut output = [0i32; 8];
    idct8(&input, &mut output);
    assert_exact("idct8_from_fdct8_mixed", &output, &golden);
}

// =============================================================================
// iadst4 golden parity
// =============================================================================

#[test]
fn iadst4_zero_golden() {
    let input = [0i32; 4];
    let golden = [0, 0, 0, 0];
    let mut output = [0i32; 4];
    iadst4(&input, &mut output);
    assert_exact("iadst4_zero", &output, &golden);
}

#[test]
fn iadst4_from_fadst4_mixed_golden() {
    let input = [26i32, 163, -145, 319];
    let golden = [200, -101, 400, -300];
    let mut output = [0i32; 4];
    iadst4(&input, &mut output);
    assert_exact("iadst4_from_fadst4_mixed", &output, &golden);
}

// =============================================================================
// iidentity4 golden parity
// =============================================================================

#[test]
fn iidentity4_golden() {
    let input = [100i32, 200, 300, 400];
    let golden = [141, 283, 424, 566];
    let mut output = [0i32; 4];
    iidentity4(&input, &mut output);
    assert_exact("iidentity4", &output, &golden);
}

// =============================================================================
// iidentity8 golden parity
// =============================================================================

#[test]
fn iidentity8_golden() {
    let input = [200i32; 8];
    let golden = [400; 8];
    let mut output = [0i32; 8];
    iidentity8(&input, &mut output);
    assert_exact("iidentity8", &output, &golden);
}

// =============================================================================
// Roundtrip tests: fdct->idct with known scale factors
// =============================================================================

#[test]
fn roundtrip_dct4_golden() {
    // fdct4([100,-50,200,-150]) -> [71,135,-141,327]
    // idct4([71,135,-141,327]) -> [201,-100,400,-299]
    // Scale factor: inv[i]/orig[i] ~= 2x
    let orig = [100i32, -50, 200, -150];
    let mut fwd = [0i32; 4];
    let mut inv = [0i32; 4];
    fdct4(&orig, &mut fwd);
    assert_exact("roundtrip_dct4 fwd", &fwd, &[71, 135, -141, 327]);
    idct4(&fwd, &mut inv);
    assert_exact("roundtrip_dct4 inv", &inv, &[201, -100, 400, -299]);
    // Verify scale factor is ~2x (tolerance +-1 from rounding)
    for i in 0..4 {
        assert!(
            (inv[i] - orig[i] * 2).abs() <= 1,
            "roundtrip_dct4 scale at [{i}]: inv={} expected ~{}",
            inv[i],
            orig[i] * 2
        );
    }
}

#[test]
fn roundtrip_dct8_golden() {
    // fdct8([50,-25,100,-75,200,-150,80,-40]) -> [99,87,-66,3,92,-27,-141,554]
    // idct8([99,87,-66,3,92,-27,-141,554]) -> [200,-99,401,-300,800,-601,319,-160]
    // Scale factor: inv[i]/orig[i] ~= 4x
    let orig = [50i32, -25, 100, -75, 200, -150, 80, -40];
    let mut fwd = [0i32; 8];
    let mut inv = [0i32; 8];
    fdct8(&orig, &mut fwd);
    assert_exact(
        "roundtrip_dct8 fwd",
        &fwd,
        &[99, 87, -66, 3, 92, -27, -141, 554],
    );
    idct8(&fwd, &mut inv);
    assert_exact(
        "roundtrip_dct8 inv",
        &inv,
        &[200, -99, 401, -300, 800, -601, 319, -160],
    );
    // Verify scale factor is ~4x (tolerance +-2 from accumulated rounding)
    for i in 0..8 {
        assert!(
            (inv[i] - orig[i] * 4).abs() <= 2,
            "roundtrip_dct8 scale at [{i}]: inv={} expected ~{}",
            inv[i],
            orig[i] * 4
        );
    }
}

// =============================================================================
// CDF update parity (computed from C algorithm in extract_intra_golden.c)
// =============================================================================

#[test]
fn cdf_update_matches_c_algorithm() {
    use svtav1_entropy::cdf::*;

    // Verified by compiling and running the identical C algorithm (/tmp/test_cdf.c):
    //   rate = 4 + (0>>4) + (4>3) = 5
    //   cdf[0] += (32768-24576)>>5 = 256  → 24832
    //   cdf[1] += (32768-16384)>>5 = 512  → 16896
    //   cdf[2] -= 8192>>5 = 256           → 7936
    //   count: 0+1 = 1
    //
    // C output (measured): [24832, 16896, 7936, 0, count=1]
    let mut cdf = [24576u16, 16384, 8192, 0, 0u16];
    update_cdf(&mut cdf, 2, 4);

    let c_golden = [24832u16, 16896, 7936, 0, 1];
    assert_eq!(cdf[0], c_golden[0], "cdf[0] mismatch");
    assert_eq!(cdf[1], c_golden[1], "cdf[1] mismatch");
    assert_eq!(cdf[2], c_golden[2], "cdf[2] mismatch");
    assert_eq!(cdf[4], c_golden[4], "count mismatch");
}

#[test]
fn cdf_update_10_iterations() {
    use svtav1_entropy::cdf::*;

    // Verify CDF after 10 updates with alternating symbols
    let mut cdf = [16384u16, 0, 0]; // binary CDF: nsymbs=2
    for i in 0..10 {
        update_cdf(&mut cdf, (i % 2) as usize, 2);
    }
    // Count should be 10
    assert_eq!(cdf[2], 10, "count after 10 updates");
    // CDF should be near 16384 (balanced) since equal 0s and 1s
    assert!(
        (cdf[0] as i32 - 16384).abs() < 2000,
        "balanced updates should keep CDF near center: {}",
        cdf[0]
    );
}

// =============================================================================
// Intra prediction parity (spec algorithm — verified by construction)
// =============================================================================

#[test]
fn dc_prediction_4x4_uniform() {
    // DC pred of uniform neighbors = that value
    let above = [100u8; 4];
    let left = [100u8; 4];
    let mut dst = [0u8; 16];
    svtav1_dsp::intra_pred::predict_dc(&mut dst, 4, &above, &left, 4, 4, true, true);
    assert!(
        dst.iter().all(|&v| v == 100),
        "DC of uniform 100 should be 100"
    );
}

#[test]
fn dc_prediction_4x4_mixed() {
    // DC = (sum(above) + sum(left) + 4) / 8
    // above=[10,20,30,40] sum=100, left=[80,70,60,50] sum=260
    // DC = (100+260+4)/8 = 364/8 = 45
    let above = [10u8, 20, 30, 40];
    let left = [80u8, 70, 60, 50];
    let mut dst = [0u8; 16];
    svtav1_dsp::intra_pred::predict_dc(&mut dst, 4, &above, &left, 4, 4, true, true);
    assert!(
        dst.iter().all(|&v| v == 45),
        "DC should be 45, got {}",
        dst[0]
    );
}

#[test]
fn v_prediction_copies_above_exactly() {
    let above = [10u8, 20, 30, 40];
    let mut dst = [0u8; 16];
    svtav1_dsp::intra_pred::predict_v(&mut dst, 4, &above, 4, 4);
    for row in 0..4 {
        assert_eq!(
            &dst[row * 4..row * 4 + 4],
            &above,
            "V-pred row {row} mismatch"
        );
    }
}

#[test]
fn h_prediction_copies_left_exactly() {
    let left = [10u8, 20, 30, 40];
    let mut dst = [0u8; 16];
    svtav1_dsp::intra_pred::predict_h(&mut dst, 4, &left, 4, 4);
    for row in 0..4 {
        assert!(
            dst[row * 4..row * 4 + 4].iter().all(|&v| v == left[row]),
            "H-pred row {row} should all be {}",
            left[row]
        );
    }
}

#[test]
fn paeth_prediction_matches_spec() {
    // Paeth: pred = argmin(|base - top|, |base - left|, |base - tl|)
    // where base = top + left - tl
    //
    // top_left=50, above=[10,20,30,40], left=[60,70,80,90]
    // For pixel (0,0): base = 10+60-50 = 20
    //   |20-10|=10, |20-60|=40, |20-50|=30 → min is top=10
    let above = [10u8, 20, 30, 40];
    let left = [60u8, 70, 80, 90];
    let mut dst = [0u8; 16];
    svtav1_dsp::intra_pred::predict_paeth(&mut dst, 4, &above, &left, 50, 4, 4);
    // Pixel (0,0): base=20, p_top=10, p_left=40, p_tl=30 → top=10
    assert_eq!(dst[0], 10, "paeth(0,0) should be 10 (top)");
    // Pixel (0,3): base=40+60-50=50, p_top=|50-40|=10, p_left=|50-60|=10, p_tl=|50-50|=0 → tl=50
    assert_eq!(dst[3], 50, "paeth(0,3) should be 50 (top_left)");
}

// =============================================================================
// idct16 golden parity (measured from C)
// =============================================================================

#[test]
fn idct16_dc_golden() {
    let mut input = [0i32; 16];
    input[0] = 566;
    let golden = [400i32; 16];
    let mut output = [0i32; 16];
    svtav1_dsp::inv_txfm::idct16(&input, &mut output);
    assert_exact("idct16_dc", &output, &golden);
}

#[test]
fn idct16_from_fdct16_ramp_golden() {
    let input = [
        -57i32, -517, 0, -57, 0, -20, 0, -10, 0, -5, 0, -3, 0, -2, 0, -1,
    ];
    let golden = [
        -640i32, -560, -480, -399, -320, -239, -160, -80, 0, 80, 159, 240, 319, 400, 480, 560,
    ];
    let mut output = [0i32; 16];
    svtav1_dsp::inv_txfm::idct16(&input, &mut output);
    assert_exact("idct16_from_fdct16_ramp", &output, &golden);
}

// =============================================================================
// iadst8 golden parity (measured from C)
// =============================================================================

#[test]
fn iadst8_from_fadst8_mixed_golden() {
    let input = [56i32, 125, -19, -40, 84, 33, -360, 445];
    let golden = [200i32, -101, 401, -300, 798, -599, 320, -160];
    let mut output = [0i32; 8];
    svtav1_dsp::inv_txfm::iadst8(&input, &mut output);
    assert_exact("iadst8_from_fadst8_mixed", &output, &golden);
}
