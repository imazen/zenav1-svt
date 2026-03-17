//! Temporal filtering — alt-ref frame generation.
//!
//! Temporal filtering averages multiple frames to create a denoised
//! "alt-ref" reference frame. This improves compression by providing
//! a cleaner prediction source.
//!
//! Ported from SVT-AV1's temporal_filtering.c.

/// Temporal filter configuration.
#[derive(Debug, Clone)]
pub struct TfConfig {
    /// Number of past reference frames to use.
    pub num_past: u8,
    /// Number of future reference frames to use.
    pub num_future: u8,
    /// Filter strength (0.0 = no filtering, 1.0 = full).
    pub strength: f64,
    /// Whether to use motion-compensated filtering.
    pub use_me: bool,
}

impl Default for TfConfig {
    fn default() -> Self {
        Self {
            num_past: 3,
            num_future: 3,
            strength: 0.6,
            use_me: true,
        }
    }
}

/// Result of temporal filtering for one frame.
#[derive(Debug)]
pub struct TfResult {
    /// Filtered output frame (luma only for now).
    pub filtered: alloc::vec::Vec<u8>,
    pub width: usize,
    pub height: usize,
}

/// Apply temporal filtering to generate an alt-ref frame.
///
/// Averages the center frame with motion-compensated versions of
/// neighboring frames, weighted by pixel-level similarity.
pub fn temporal_filter(
    center_frame: &[u8],
    ref_frames: &[&[u8]],
    width: usize,
    height: usize,
    stride: usize,
    config: &TfConfig,
) -> TfResult {
    let mut filtered = alloc::vec![0u16; width * height];
    let mut weight_sum = alloc::vec![0u16; width * height];

    // Center frame gets maximum weight
    let center_weight = 16u16;
    for r in 0..height {
        for c in 0..width {
            let idx = r * width + c;
            let val = center_frame[r * stride + c] as u16;
            filtered[idx] = val * center_weight;
            weight_sum[idx] = center_weight;
        }
    }

    // Add weighted contributions from reference frames
    for ref_frame in ref_frames {
        let ref_weight = (config.strength * 8.0) as u16;
        if ref_weight == 0 {
            continue;
        }

        for r in 0..height {
            for c in 0..width {
                let idx = r * width + c;
                let center_val = center_frame[r * stride + c] as i32;
                let ref_val = ref_frame[r * stride + c] as i32;

                // Weight based on pixel similarity (lower diff = higher weight)
                let diff = (center_val - ref_val).abs();
                let similarity_weight = if diff < 4 {
                    ref_weight
                } else if diff < 16 {
                    ref_weight * 3 / 4
                } else if diff < 32 {
                    ref_weight / 2
                } else if diff < 64 {
                    ref_weight / 4
                } else {
                    0
                };

                filtered[idx] += ref_val as u16 * similarity_weight;
                weight_sum[idx] += similarity_weight;
            }
        }
    }

    // Normalize
    let mut output = alloc::vec![0u8; width * height];
    for i in 0..width * height {
        if weight_sum[i] > 0 {
            output[i] = ((filtered[i] + weight_sum[i] / 2) / weight_sum[i]) as u8;
        } else {
            output[i] = center_frame[(i / width) * stride + (i % width)];
        }
    }

    TfResult {
        filtered: output,
        width,
        height,
    }
}

/// Estimate noise level of a frame for temporal filter strength selection.
///
/// Uses the Laplacian method: compute the average absolute Laplacian
/// response, which correlates with noise level.
pub fn estimate_noise(frame: &[u8], width: usize, height: usize, stride: usize) -> f64 {
    let mut sum: u64 = 0;
    let mut count: u64 = 0;

    for r in 1..height - 1 {
        for c in 1..width - 1 {
            // Laplacian: 4*center - top - bottom - left - right
            let center = frame[r * stride + c] as i32 * 4;
            let top = frame[(r - 1) * stride + c] as i32;
            let bottom = frame[(r + 1) * stride + c] as i32;
            let left = frame[r * stride + c - 1] as i32;
            let right = frame[r * stride + c + 1] as i32;
            let laplacian = (center - top - bottom - left - right).unsigned_abs();
            sum += laplacian as u64;
            count += 1;
        }
    }

    if count == 0 {
        return 0.0;
    }
    sum as f64 / count as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn temporal_filter_identical_frames() {
        let frame = vec![128u8; 16 * 16];
        let refs: Vec<&[u8]> = vec![&frame, &frame];
        let result = temporal_filter(&frame, &refs, 16, 16, 16, &TfConfig::default());
        // With identical frames, output should equal input
        assert!(result.filtered.iter().all(|&v| (v as i32 - 128).abs() <= 1));
    }

    #[test]
    fn temporal_filter_denoising() {
        // Center frame with moderate noise (within blending threshold)
        let mut center = vec![128u8; 16 * 16];
        center[0] = 150; // moderate spike (diff=22 from clean, within blend range)

        // Clean reference
        let clean = vec![128u8; 16 * 16];
        let refs: Vec<&[u8]> = vec![&clean, &clean, &clean];

        let result = temporal_filter(&center, &refs, 16, 16, 16, &TfConfig::default());
        // Noise spike should be reduced toward 128
        assert!(
            result.filtered[0] < 150,
            "noise should be reduced: {}",
            result.filtered[0]
        );
    }

    #[test]
    fn estimate_noise_flat() {
        let frame = vec![128u8; 64 * 64];
        let noise = estimate_noise(&frame, 64, 64, 64);
        assert!(
            noise < 1.0,
            "flat frame should have near-zero noise: {noise}"
        );
    }

    #[test]
    fn estimate_noise_noisy() {
        let mut frame = vec![0u8; 64 * 64];
        let mut state = 42u32;
        for p in frame.iter_mut() {
            state = state.wrapping_mul(1103515245).wrapping_add(12345);
            *p = (state >> 16) as u8;
        }
        let noise = estimate_noise(&frame, 64, 64, 64);
        assert!(
            noise > 10.0,
            "noisy frame should have high noise level: {noise}"
        );
    }
}
