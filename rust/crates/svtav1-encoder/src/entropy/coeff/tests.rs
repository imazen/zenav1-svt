use super::*;

#[test]
fn write_zero_block() {
    let mut w = AomWriter::new(256);
    write_coefficients(&mut w, &[], 0, 0, 0);
    let output = w.done();
    assert!(!output.is_empty());
}

#[test]
fn write_single_dc_coeff() {
    let mut w = AomWriter::new(256);
    write_coefficients(&mut w, &[100, 0, 0, 0], 1, 0, 0);
    let output = w.done();
    assert!(!output.is_empty());
}

#[test]
fn write_multiple_coeffs() {
    let mut w = AomWriter::new(1024);
    let coeffs = [
        500, -300, 200, -100, 50, -25, 10, -5, 0, 0, 0, 0, 0, 0, 0, 0,
    ];
    write_coefficients(&mut w, &coeffs, 8, 0, 0);
    let output = w.done();
    assert!(
        output.len() > 5,
        "multiple coeffs should produce substantial output"
    );
}

#[test]
fn txb_skip_context() {
    assert_eq!(get_txb_skip_context(false, false), 0);
    assert_eq!(get_txb_skip_context(true, false), 1);
    assert_eq!(get_txb_skip_context(true, true), 2);
}

#[test]
fn dc_sign_context() {
    assert_eq!(get_dc_sign_context(-1, 0), 0);
    assert_eq!(get_dc_sign_context(0, 0), 1);
    assert_eq!(get_dc_sign_context(1, 0), 2);
}

// ====================================================================
// Tests for the new v2 encoder
// ====================================================================

#[test]
fn eob_bin_conversion() {
    assert_eq!(eob_to_bin(0), 0);
    assert_eq!(eob_to_bin(1), 1);
    assert_eq!(eob_to_bin(2), 2);
    assert_eq!(eob_to_bin(3), 2);
    assert_eq!(eob_to_bin(4), 3);
    assert_eq!(eob_to_bin(7), 3);
    assert_eq!(eob_to_bin(8), 4);
    assert_eq!(eob_to_bin(15), 4);
    assert_eq!(eob_to_bin(16), 5);
    assert_eq!(eob_to_bin(31), 5);
    assert_eq!(eob_to_bin(32), 6);
    assert_eq!(eob_to_bin(63), 6);
    assert_eq!(eob_to_bin(64), 7);
    assert_eq!(eob_to_bin(127), 7);
    assert_eq!(eob_to_bin(128), 8);
    assert_eq!(eob_to_bin(255), 8);
    assert_eq!(eob_to_bin(256), 9);
    assert_eq!(eob_to_bin(511), 9);
    assert_eq!(eob_to_bin(512), 10);
    assert_eq!(eob_to_bin(1023), 10);
}

#[test]
fn diagonal_scan_4x4_matches_hardcoded() {
    let generated = generate_diagonal_scan(4, 4);
    let hardcoded: Vec<u16> = svtav1_types::tables::scan::DEFAULT_SCAN_4X4
        .iter()
        .map(|&x| x as u16)
        .collect();
    assert_eq!(generated, hardcoded, "4x4 diagonal scan mismatch");
}

#[test]
fn diagonal_scan_8x8_matches_hardcoded() {
    let generated = generate_diagonal_scan(8, 8);
    let hardcoded: Vec<u16> = svtav1_types::tables::scan::DEFAULT_SCAN_8X8
        .iter()
        .map(|&x| x as u16)
        .collect();
    assert_eq!(generated, hardcoded, "8x8 diagonal scan mismatch");
}

#[test]
fn diagonal_scan_16x16_covers_all() {
    let scan = generate_diagonal_scan(16, 16);
    assert_eq!(scan.len(), 256);
    let mut visited = [false; 256];
    for &idx in &scan {
        assert!(
            !visited[idx as usize],
            "duplicate index {idx} in 16x16 scan"
        );
        visited[idx as usize] = true;
    }
    assert!(
        visited.iter().all(|&v| v),
        "16x16 scan must cover all positions"
    );
}

#[test]
fn diagonal_scan_32x32_covers_all() {
    let scan = generate_diagonal_scan(32, 32);
    assert_eq!(scan.len(), 1024);
    let mut visited = [false; 1024];
    for &idx in &scan {
        assert!(
            !visited[idx as usize],
            "duplicate index {idx} in 32x32 scan"
        );
        visited[idx as usize] = true;
    }
    assert!(
        visited.iter().all(|&v| v),
        "32x32 scan must cover all positions"
    );
}

#[test]
fn v2_write_zero_block_4x4() {
    let mut w = AomWriter::new(256);
    let mut cdf = CdfCoefCtx::new(0);
    let coeffs = [0i32; 16];
    write_coefficients_v2(&mut w, &coeffs, 0, 4, 4, 0, 1, 0u8, &mut cdf);
    let output = w.done();
    assert!(!output.is_empty(), "zero block should produce output");
}

#[test]
fn v2_write_dc_only_4x4() {
    let mut w = AomWriter::new(256);
    let mut cdf = CdfCoefCtx::new(0);
    let mut coeffs = [0i32; 16];
    coeffs[0] = 42;
    // eob = 1 means "there is 1 non-zero coefficient" in our convention,
    // but for v2, eob is scan_eob, the position. Since DC is at scan position 0,
    // we pass eob=1 to indicate non-zero content exists.
    write_coefficients_v2(&mut w, &coeffs, 1, 4, 4, 0, 1, 0u8, &mut cdf);
    let output = w.done();
    assert!(!output.is_empty(), "dc-only block should produce output");
}

#[test]
fn v2_write_multiple_coeffs_4x4() {
    let mut w = AomWriter::new(1024);
    let mut cdf = CdfCoefCtx::new(0);
    let coeffs = [
        500, -300, 200, -100, 50, -25, 10, -5, 3, -1, 0, 0, 0, 0, 0, 0,
    ];
    // eob should be the number of non-zero coefficients for our caller,
    // but v2 internally finds the scan-order eob.
    write_coefficients_v2(&mut w, &coeffs, 10, 4, 4, 0, 1, 0u8, &mut cdf);
    let output = w.done();
    assert!(
        output.len() > 5,
        "multiple coeffs should produce substantial output, got {} bytes",
        output.len()
    );
}

#[test]
fn v2_write_8x8_block() {
    let mut w = AomWriter::new(2048);
    let mut cdf = CdfCoefCtx::new(1);
    let mut coeffs = [0i32; 64];
    coeffs[0] = 1000;
    coeffs[1] = -500;
    coeffs[8] = 200;
    coeffs[9] = -100;
    write_coefficients_v2(&mut w, &coeffs, 4, 8, 8, 0, 1, 0u8, &mut cdf);
    let output = w.done();
    assert!(!output.is_empty(), "8x8 block should produce output");
}

#[test]
fn v2_write_16x16_block() {
    let mut w = AomWriter::new(4096);
    let mut cdf = CdfCoefCtx::new(2);
    let mut coeffs = [0i32; 256];
    // Place some coefficients
    coeffs[0] = 2000;
    coeffs[1] = -1000;
    coeffs[16] = 500;
    coeffs[17] = -200;
    coeffs[32] = 100;
    write_coefficients_v2(&mut w, &coeffs, 5, 16, 16, 0, 1, 0u8, &mut cdf);
    let output = w.done();
    assert!(!output.is_empty(), "16x16 block should produce output");
}

#[test]
fn v2_write_32x32_block() {
    let mut w = AomWriter::new(8192);
    let mut cdf = CdfCoefCtx::new(3);
    let mut coeffs = [0i32; 1024];
    coeffs[0] = 5000;
    coeffs[1] = -2000;
    coeffs[32] = 1000;
    write_coefficients_v2(&mut w, &coeffs, 3, 32, 32, 0, 1, 0u8, &mut cdf);
    let output = w.done();
    assert!(!output.is_empty(), "32x32 block should produce output");
}

#[test]
fn v2_negative_dc_sign() {
    let mut w = AomWriter::new(256);
    let mut cdf = CdfCoefCtx::new(0);
    let mut coeffs = [0i32; 16];
    coeffs[0] = -42;
    write_coefficients_v2(&mut w, &coeffs, 1, 4, 4, 0, 1, 0u8, &mut cdf);
    let output = w.done();
    assert!(!output.is_empty());
}

#[test]
fn v2_large_coefficient_golomb() {
    let mut w = AomWriter::new(1024);
    let mut cdf = CdfCoefCtx::new(0);
    let mut coeffs = [0i32; 16];
    // Level 100 requires golomb coding (tok >= 15)
    coeffs[0] = 100;
    write_coefficients_v2(&mut w, &coeffs, 1, 4, 4, 0, 1, 0u8, &mut cdf);
    let output = w.done();
    assert!(!output.is_empty(), "large coefficient should be encodable");
}

#[test]
fn v2_all_qp_categories() {
    for qp in 0..4 {
        let mut w = AomWriter::new(512);
        let mut cdf = CdfCoefCtx::new(qp);
        let mut coeffs = [0i32; 16];
        coeffs[0] = 10;
        coeffs[1] = -5;
        write_coefficients_v2(&mut w, &coeffs, 2, 4, 4, 0, 1, 0u8, &mut cdf);
        let output = w.done();
        assert!(!output.is_empty(), "QP category {qp} should produce output");
    }
}

#[test]
fn log2_tx_dim_values() {
    assert_eq!(log2_of_tx_dim(4), 0);
    assert_eq!(log2_of_tx_dim(8), 1);
    assert_eq!(log2_of_tx_dim(16), 2);
    assert_eq!(log2_of_tx_dim(32), 3);
    assert_eq!(log2_of_tx_dim(64), 4);
}

#[test]
fn golomb_roundtrip_values() {
    // Test that golomb encoding produces non-empty output for various values
    for val in [0, 1, 2, 3, 7, 15, 31, 63, 100, 255, 1000] {
        let mut w = AomWriter::new(256);
        write_golomb(&mut w, val);
        let output = w.done();
        assert!(!output.is_empty(), "golomb({val}) should produce output");
    }
}
