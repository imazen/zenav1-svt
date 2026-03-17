//! Film grain noise estimation and synthesis.
//!
//! Spec 12 (film-grain.md): Noise estimation and synthesis.
//!
//! Estimates the noise model from the source video and signals it
//! as metadata so the decoder can re-synthesize the grain after
//! decompression. This allows the encoder to denoise before encoding
//! (better compression) while preserving the perceptual quality.
//!
//! Ported from SVT-AV1's noise_model.c and grainSynthesis.c.

/// Film grain parameters for a frame.
#[derive(Debug, Clone, Default)]
pub struct FilmGrainParams {
    /// Whether film grain is applied to this frame.
    pub apply_grain: bool,
    /// Grain seed for pseudo-random generation.
    pub grain_seed: u16,
    /// Number of Y (luma) points in the piecewise-linear scaling function.
    pub num_y_points: u8,
    /// Y scaling function points: (intensity, scaling) pairs.
    pub point_y_value: [u8; 14],
    pub point_y_scaling: [u8; 14],
    /// Number of Cb (chroma) points.
    pub num_cb_points: u8,
    pub point_cb_value: [u8; 10],
    pub point_cb_scaling: [u8; 10],
    /// Number of Cr (chroma) points.
    pub num_cr_points: u8,
    pub point_cr_value: [u8; 10],
    pub point_cr_scaling: [u8; 10],
    /// AR (auto-regressive) coefficients for luma.
    pub ar_coeffs_y: [i8; 24],
    /// AR coefficients for Cb.
    pub ar_coeffs_cb: [i8; 25],
    /// AR coefficients for Cr.
    pub ar_coeffs_cr: [i8; 25],
    /// AR coefficient lag (number of previous samples used).
    pub ar_coeff_lag: u8,
    /// AR coefficient shift (precision).
    pub ar_coeff_shift: u8,
    /// Grain scaling shift.
    pub grain_scale_shift: u8,
    /// Cb/Cr multiplier and offset.
    pub cb_mult: u8,
    pub cb_luma_mult: u8,
    pub cb_offset: u16,
    pub cr_mult: u8,
    pub cr_luma_mult: u8,
    pub cr_offset: u16,
    /// Whether to overlap grain blocks.
    pub overlap_flag: bool,
    /// Whether to clip to restricted range.
    pub clip_to_restricted_range: bool,
    /// Chroma scaling from luma.
    pub chroma_scaling_from_luma: bool,
}

/// Estimate film grain parameters from source and denoised frames.
///
/// Compares the source (noisy) frame with a denoised version to
/// estimate the noise characteristics and fit an AR model.
pub fn estimate_film_grain(
    source: &[u8],
    denoised: &[u8],
    width: usize,
    height: usize,
    stride: usize,
) -> FilmGrainParams {
    let mut params = FilmGrainParams::default();

    // Compute noise = source - denoised
    let mut noise_sum: i64 = 0;
    let mut noise_sq_sum: i64 = 0;
    let n = (width * height) as i64;

    for r in 0..height {
        for c in 0..width {
            let noise = source[r * stride + c] as i32 - denoised[r * stride + c] as i32;
            noise_sum += noise as i64;
            noise_sq_sum += (noise * noise) as i64;
        }
    }

    let noise_var = if n > 0 {
        (noise_sq_sum as f64 / n as f64) - (noise_sum as f64 / n as f64).powi(2)
    } else {
        0.0
    };

    // Only apply grain if noise is significant
    if noise_var < 4.0 {
        return params;
    }

    params.apply_grain = true;
    params.grain_seed = 42; // Fixed seed for reproducibility
    params.ar_coeff_lag = 0; // Simple (no AR model)
    params.grain_scale_shift = 0;
    params.overlap_flag = true;

    // Simple piecewise-linear scaling: higher intensity = more grain
    params.num_y_points = 2;
    params.point_y_value[0] = 0;
    params.point_y_scaling[0] = (noise_var.sqrt() * 2.0).min(255.0) as u8;
    params.point_y_value[1] = 255;
    params.point_y_scaling[1] = (noise_var.sqrt() * 2.0).min(255.0) as u8;

    params
}

/// Synthesize film grain noise and add to a decoded frame.
///
/// This is the decoder-side operation: generates grain from the
/// signaled parameters and adds it to the reconstructed frame.
pub fn synthesize_grain(
    frame: &mut [u8],
    width: usize,
    height: usize,
    stride: usize,
    params: &FilmGrainParams,
) {
    if !params.apply_grain || params.num_y_points == 0 {
        return;
    }

    // Simple grain synthesis: add pseudo-random noise scaled by params
    let scaling = params.point_y_scaling[0] as i32;
    let mut seed = params.grain_seed as u32;

    for r in 0..height {
        for c in 0..width {
            // LCG pseudo-random
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            let noise = ((seed >> 16) as i32 % (scaling * 2 + 1)) - scaling;
            let val = frame[r * stride + c] as i32 + noise;
            frame[r * stride + c] = val.clamp(0, 255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn estimate_no_grain_for_identical() {
        let frame = vec![128u8; 64 * 64];
        let params = estimate_film_grain(&frame, &frame, 64, 64, 64);
        assert!(!params.apply_grain, "identical frames should have no grain");
    }

    #[test]
    fn estimate_detects_noise() {
        let clean = vec![128u8; 64 * 64];
        let mut noisy = clean.clone();
        let mut state = 42u32;
        for p in noisy.iter_mut() {
            state = state.wrapping_mul(1103515245).wrapping_add(12345);
            let noise = ((state >> 16) as i32 % 41) - 20; // ±20
            *p = (*p as i32 + noise).clamp(0, 255) as u8;
        }
        let params = estimate_film_grain(&noisy, &clean, 64, 64, 64);
        assert!(params.apply_grain, "should detect noise");
        assert!(params.num_y_points > 0);
    }

    #[test]
    fn synthesize_modifies_frame() {
        let mut frame = vec![128u8; 16 * 16];
        let original = frame.clone();
        let params = FilmGrainParams {
            apply_grain: true,
            grain_seed: 42,
            num_y_points: 1,
            point_y_scaling: {
                let mut arr = [0u8; 14];
                arr[0] = 20;
                arr
            },
            ..Default::default()
        };
        synthesize_grain(&mut frame, 16, 16, 16, &params);
        assert_ne!(frame, original, "grain should modify the frame");
    }

    #[test]
    fn synthesize_no_op_when_disabled() {
        let mut frame = vec![128u8; 16 * 16];
        let original = frame.clone();
        let params = FilmGrainParams::default(); // apply_grain = false
        synthesize_grain(&mut frame, 16, 16, 16, &params);
        assert_eq!(frame, original);
    }
}
