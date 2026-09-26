use super::*;

// --- Quantization Matrix tests ---

#[test]
fn qm_flat_is_identity() {
    let qm = QuantMatrix::flat();
    let mut coeffs = [100i32, -200, 300, -400, 0, 0, 0, 0];
    let original = coeffs;
    qm.apply(&mut coeffs, 2);
    // Flat QM (all 256) should leave coefficients unchanged
    assert_eq!(coeffs, original);
}

#[test]
fn qm_reduces_high_frequency() {
    let qm = QuantMatrix::av1_default_8x8();
    let mut coeffs = [1000i32; 64];
    let original = coeffs;
    qm.apply(&mut coeffs, 8);
    // DC (weight=256) should be unchanged
    assert_eq!(coeffs[0], original[0]);
    // High frequency (weight>256) should be reduced
    assert!(coeffs[63] < original[63], "high freq should be reduced");
}

#[test]
fn qm_apply_unapply_roundtrip() {
    let qm = QuantMatrix::av1_default_8x8();
    let original = [
        1000i32, -500, 250, -125, 60, -30, 15, -8, 0, 0, 0, 0, 0, 0, 0, 0,
    ];
    let mut coeffs = original;
    qm.apply(&mut coeffs, 4);
    qm.unapply(&mut coeffs, 4);
    // Should approximately recover (within rounding)
    for i in 0..8 {
        let diff = (coeffs[i] - original[i]).abs();
        assert!(diff <= 2, "roundtrip error at {i}: diff={diff}");
    }
}

// --- VAQ tests ---

#[test]
fn activity_map_flat_image() {
    let pixels = vec![128u8; 64 * 64];
    let map = ActivityMap::compute(&pixels, 64, 64, 64);
    assert_eq!(map.cols, 8);
    assert_eq!(map.rows, 8);
    // Flat image should have low, uniform activity
    for &a in &map.activities {
        assert!(a < 2.0, "flat block activity {a} too high");
    }
}

#[test]
fn activity_map_gradient_image() {
    let mut pixels = vec![0u8; 64 * 64];
    for r in 0..64 {
        for c in 0..64 {
            pixels[r * 64 + c] = (r * 4) as u8;
        }
    }
    let map = ActivityMap::compute(&pixels, 64, 64, 64);
    // Gradient has moderate variance
    assert!(map.frame_avg > 1.0);
}

#[test]
fn vaq_qp_delta_smooth_vs_textured() {
    let mut pixels = vec![128u8; 64 * 64];
    // Make top-left blocks smooth (uniform)
    // Make bottom-right blocks textured (random-ish)
    for r in 32..64 {
        for c in 32..64 {
            pixels[r * 64 + c] = ((r * 7 + c * 13) % 256) as u8;
        }
    }

    let map = ActivityMap::compute(&pixels, 64, 64, 64);
    let smooth_delta = map.qp_delta(0, 0, 1.0); // Top-left (smooth)
    let texture_delta = map.qp_delta(7, 7, 1.0); // Bottom-right (textured)

    // Smooth should get negative (or less positive) delta than textured
    assert!(
        smooth_delta <= texture_delta,
        "smooth delta {smooth_delta} should be <= texture delta {texture_delta}"
    );
}

#[test]
fn vaq_strength_zero_gives_zero_delta() {
    let pixels = vec![128u8; 64 * 64];
    let map = ActivityMap::compute(&pixels, 64, 64, 64);
    let delta = map.qp_delta(0, 0, 0.0);
    assert_eq!(delta, 0);
}

// --- Still-image tuning tests ---

#[test]
fn still_image_reduces_filters() {
    let config = StillImageConfig::default();
    let (deblock, cdef) = adjust_loop_filter_for_still(30, &config);
    let (deblock_normal, cdef_normal) = adjust_loop_filter_for_still(
        30,
        &StillImageConfig {
            reduce_cdef: false,
            reduce_deblock: false,
            ..Default::default()
        },
    );
    assert!(deblock < deblock_normal);
    assert!(cdef < cdef_normal);
}

// --- Trellis quantization tests ---

#[test]
fn trellis_zero_coeffs() {
    let coeffs = [0i32; 16];
    let result = trellis_quantize(&coeffs, &[4, 8], 1.0, 16);
    assert!(result.iter().all(|&v| v == 0));
}

#[test]
fn trellis_preserves_large_coefficients() {
    let coeffs = [10000i32, -20000, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    let result = trellis_quantize(&coeffs, &[4, 8], 0.5, 16);
    // Large coefficients should survive with correct sign
    assert!(result[0] > 0);
    assert!(result[1] < 0);
}

#[test]
fn trellis_zeros_small_coefficients_at_high_lambda() {
    // With very high lambda (rate penalty), small coefficients should be zeroed
    let coeffs = [5i32, -3, 2, -1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    let result = trellis_quantize(&coeffs, &[4, 8], 100.0, 16);
    // High lambda should zero most small coefficients
    let non_zero = result.iter().filter(|&&v| v != 0).count();
    assert!(
        non_zero <= 2,
        "high lambda should zero small coefficients: {non_zero} non-zero"
    );
}

// --- Segmentation boost tests ---

#[test]
fn seg_boost_identity() {
    assert_eq!(apply_seg_boost(30, 5, 1.0), 35);
    assert_eq!(apply_seg_boost(30, -5, 1.0), 25);
}

#[test]
fn seg_boost_amplifies() {
    let normal = apply_seg_boost(30, 4, 1.0); // 34
    let boosted = apply_seg_boost(30, 4, 2.0); // 38
    assert!(boosted > normal);
}

#[test]
fn seg_boost_clamps() {
    assert_eq!(apply_seg_boost(60, 10, 2.0), 63); // Clamped to max
    assert_eq!(apply_seg_boost(5, -10, 2.0), 0); // Clamped to min
}
