//! Temporal filtering — alt-ref frame generation.
//!
//! Spec 17 (temporal-filtering.md): Alt-ref frame generation.
//!
//! AUDIT 2026-07-14 (pre-v4.2-bump port correctness). Status by function:
//!
//! - [`estimate_noise_fp16`] — bit-exact port of C `svt_estimate_noise_fp16_c`
//!   (temporal_filtering.c:3555), validated in `tests/c_parity_temporal.rs`.
//!   The function body is unchanged 4.1->4.2 (the nearby bit-affecting hunk
//!   only adds VMAF RTCD declarations).
//! - [`temporal_filter`] — HOMEGROWN similarity-weighted average; NOT a port
//!   of C `svt_av1_apply_temporal_filter_planewise_medium_partial_c` (which
//!   uses exp()-decayed per-block weights driven by ME + block variance).
//!   Non-normative (TF only changes the alt-ref *content*, i.e. which bits get
//!   coded, never the bitstream format). Reachability: `pipeline.rs` inter
//!   path only (`enable_temporal_filter && !is_key`) — dormant for the
//!   key/still-frame conformance + identity gates.
//! - [`estimate_noise`] — HOMEGROWN f64 5-tap-Laplacian heuristic, currently
//!   unused; superseded by the exact [`estimate_noise_fp16`] port. Kept as a
//!   simple activity proxy; NOT the C noise estimator.
//!
//! `temporal_filtering.c` is bit-affecting-changed 4.1->4.2, so a future full
//! port of `produce_temporally_filtered_pic` must track the v4.2 source.

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
) -> crate::EncodeResult<TfResult> {
    let mut filtered = svtav1_types::try_vec![0u16; width * height]?;
    let mut weight_sum = svtav1_types::try_vec![0u16; width * height]?;

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
    let mut output = svtav1_types::try_vec![0u8; width * height]?;
    for i in 0..width * height {
        if weight_sum[i] > 0 {
            output[i] = ((filtered[i] + weight_sum[i] / 2) / weight_sum[i]) as u8;
        } else {
            output[i] = center_frame[(i / width) * stride + (i % width)];
        }
    }

    Ok(TfResult {
        filtered: output,
        width,
        height,
    })
}

/// FP16 constants from `temporal_filtering.h`.
const EDGE_THRESHOLD: i32 = 50;
const SQRT_PI_BY_2_FP16: i64 = 82137;
const SMOOTH_THRESHOLD: i64 = 16;

/// Estimate the noise level of a luma plane as an FP16 fixed-point value.
///
/// Bit-exact port of C `svt_estimate_noise_fp16_c` (temporal_filtering.c:3555):
/// for every interior pixel, a Sobel gradient rejects edge pixels
/// (`|g_x| + |g_y| >= EDGE_THRESHOLD`), and the remaining smooth pixels
/// contribute `|Laplacian|` (9-tap: `4*c - 2*(4 edges) + (4 corners)`). The
/// result is `sum * SQRT_PI_BY_2_FP16 / (6 * num)` in FP16, or `-65536`
/// (-1 in FP16) when fewer than `SMOOTH_THRESHOLD` smooth pixels are found.
/// Degenerate sizes (`< 3` in either dimension) yield the same -1 sentinel
/// (num would be 0). Validated in `tests/c_parity_temporal.rs`.
pub fn estimate_noise_fp16(src: &[u8], width: usize, height: usize, y_stride: usize) -> i32 {
    if width < 3 || height < 3 {
        return -65536;
    }
    let mut sum: i64 = 0;
    let mut num: i64 = 0;
    for i in 1..height - 1 {
        for j in 1..width - 1 {
            let k = i * y_stride + j;
            let p = |off: usize| src[off] as i32;
            // Sobel gradients (reject edge pixels).
            let g_x = (p(k - y_stride - 1) - p(k - y_stride + 1))
                + (p(k + y_stride - 1) - p(k + y_stride + 1))
                + 2 * (p(k - 1) - p(k + 1));
            let g_y = (p(k - y_stride - 1) - p(k + y_stride - 1))
                + (p(k - y_stride + 1) - p(k + y_stride + 1))
                + 2 * (p(k - y_stride) - p(k + y_stride));
            let ga = g_x.abs() + g_y.abs();
            if ga < EDGE_THRESHOLD {
                // 9-tap Laplacian.
                let v = 4 * p(k) - 2 * (p(k - 1) + p(k + 1) + p(k - y_stride) + p(k + y_stride))
                    + (p(k - y_stride - 1)
                        + p(k - y_stride + 1)
                        + p(k + y_stride - 1)
                        + p(k + y_stride + 1));
                sum += v.unsigned_abs() as i64;
                num += 1;
            }
        }
    }
    if num < SMOOTH_THRESHOLD {
        return -65536; // -1 in FP16: estimate unreliable
    }
    ((sum * SQRT_PI_BY_2_FP16) / (6 * num)) as i32
}

/// Homegrown activity/noise proxy (NOT the C estimator — see
/// [`estimate_noise_fp16`] for the bit-exact port). Average absolute 5-tap
/// Laplacian over interior pixels, as an f64. Currently unused.
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
        let result = temporal_filter(&frame, &refs, 16, 16, 16, &TfConfig::default()).unwrap();
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

        let result = temporal_filter(&center, &refs, 16, 16, 16, &TfConfig::default()).unwrap();
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
