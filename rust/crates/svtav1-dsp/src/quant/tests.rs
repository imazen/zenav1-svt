use super::*;

fn default_qparam() -> QuantParam {
    QuantParam {
        dequant: [4, 8],
        shift: 2,
    }
}

// --- Zero input ---

#[test]
fn quantize_zero_coeffs() {
    let coeffs = [0i32; 16];
    let qparam = default_qparam();
    let mut qcoeffs = [0i32; 16];
    let mut dqcoeffs = [0i32; 16];
    let eob = quantize(&coeffs, &qparam, &mut qcoeffs, &mut dqcoeffs, 16);
    assert_eq!(eob, 0);
    assert!(qcoeffs.iter().all(|&v| v == 0));
    assert!(dqcoeffs.iter().all(|&v| v == 0));
}

#[test]
fn dequantize_zero_coeffs() {
    let qcoeffs = [0i32; 16];
    let dequant = [4, 8];
    let mut output = [999i32; 16];
    dequantize(&qcoeffs, &dequant, &mut output, 16);
    assert!(output.iter().all(|&v| v == 0));
}

// --- Sign preservation ---

#[test]
fn quantize_dequantize_preserves_sign() {
    let coeffs = [100, -200, 50, -75i32, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    let qparam = default_qparam();
    let mut qcoeffs = [0i32; 16];
    let mut dqcoeffs = [0i32; 16];
    let eob = quantize(&coeffs, &qparam, &mut qcoeffs, &mut dqcoeffs, 16);
    assert!(eob > 0);

    // Check sign preservation
    for i in 0..4 {
        if coeffs[i] != 0 && qcoeffs[i] != 0 {
            assert_eq!(
                coeffs[i].signum(),
                qcoeffs[i].signum(),
                "sign mismatch at [{}]",
                i
            );
            assert_eq!(
                coeffs[i].signum(),
                dqcoeffs[i].signum(),
                "dequant sign mismatch at [{}]",
                i
            );
        }
    }
}

// --- Large coefficients survive ---

#[test]
fn quantize_large_coefficients() {
    let coeffs = [
        10000, -20000, 30000, -40000i32, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ];
    let qparam = default_qparam();
    let mut qcoeffs = [0i32; 16];
    let mut dqcoeffs = [0i32; 16];
    let eob = quantize(&coeffs, &qparam, &mut qcoeffs, &mut dqcoeffs, 16);
    assert!(
        eob >= 4,
        "all large coefficients should survive quantization"
    );
    for i in 0..4 {
        assert!(
            qcoeffs[i] != 0,
            "large coeff[{}] should not quantize to zero",
            i
        );
    }
}

// --- DC uses dequant[0], AC uses dequant[1] ---

#[test]
fn dc_uses_dequant0_ac_uses_dequant1() {
    let coeffs = [100i32, 100, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    let qparam = QuantParam {
        dequant: [2, 16], // Very different DC vs AC dequant
        shift: 2,
    };
    let mut qcoeffs = [0i32; 16];
    let mut dqcoeffs = [0i32; 16];
    quantize(&coeffs, &qparam, &mut qcoeffs, &mut dqcoeffs, 16);

    // DC (index 0): dequant = 2, so q = (100 << 2) / 2 = 200
    // dqcoeff = 200 * 2 = 400
    assert_eq!(qcoeffs[0], 200);
    assert_eq!(dqcoeffs[0], 400);

    // AC (index 1): dequant = 16, so q = (100 << 2) / 16 = 25
    // dqcoeff = 25 * 16 = 400
    assert_eq!(qcoeffs[1], 25);
    assert_eq!(dqcoeffs[1], 400);
}

// --- Dequantize respects eob ---

#[test]
fn dequantize_respects_eob() {
    let qcoeffs = [10i32, 20, 30, 40, 50, 60, 70, 80, 0, 0, 0, 0, 0, 0, 0, 0];
    let dequant = [4, 8];
    let mut output = [0i32; 16];
    dequantize(&qcoeffs, &dequant, &mut output, 4);

    // First 4 should be dequantized
    assert_eq!(output[0], 10 * 4); // DC
    assert_eq!(output[1], 20 * 8); // AC
    assert_eq!(output[2], 30 * 8);
    assert_eq!(output[3], 40 * 8);

    // Rest should be zero (beyond eob)
    for i in 4..16 {
        assert_eq!(output[i], 0, "output[{}] should be 0 beyond eob", i);
    }
}

// --- Roundtrip: quantize then dequantize ---

#[test]
fn quantize_then_dequantize_roundtrip() {
    let coeffs = [
        500, -300, 200, -100i32, 50, -25, 10, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ];
    let qparam = QuantParam {
        dequant: [4, 4],
        shift: 0,
    };
    let mut qcoeffs = [0i32; 16];
    let mut dqcoeffs_from_quant = [0i32; 16];
    let eob = quantize(&coeffs, &qparam, &mut qcoeffs, &mut dqcoeffs_from_quant, 16);

    let mut dqcoeffs_from_dequant = [0i32; 16];
    dequantize(&qcoeffs, &qparam.dequant, &mut dqcoeffs_from_dequant, eob);

    // Both dequantization paths should agree
    for i in 0..16 {
        assert_eq!(
            dqcoeffs_from_quant[i], dqcoeffs_from_dequant[i],
            "dequant mismatch at [{}]",
            i
        );
    }
}

// --- Dead-zone behavior ---

#[test]
fn small_coefficients_quantize_to_zero() {
    // With dequant=100 and shift=0, coefficients below 100 should quantize to zero
    let coeffs = [50, -50, 99, -99i32, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    let qparam = QuantParam {
        dequant: [100, 100],
        shift: 0,
    };
    let mut qcoeffs = [0i32; 16];
    let mut dqcoeffs = [0i32; 16];
    let eob = quantize(&coeffs, &qparam, &mut qcoeffs, &mut dqcoeffs, 16);
    assert_eq!(eob, 0, "all small coefficients should be in the dead zone");
    assert!(qcoeffs.iter().all(|&v| v == 0));
}
