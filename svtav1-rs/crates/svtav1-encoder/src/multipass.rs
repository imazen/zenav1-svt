//! Multi-pass rate control — first-pass stats and second-pass optimization.
//!
//! Spec 09: First-pass stats and second-pass QP optimization.
//!
//! Two-pass encoding: the first pass collects statistics about each frame
//! (complexity, motion, etc.) without producing output. The second pass
//! uses these stats to optimally distribute bits across frames.
//!
//! Ported from SVT-AV1's initial_rc_process.c and rc_process.c.

use alloc::vec::Vec;

/// First-pass statistics for a single frame.
#[derive(Debug, Clone, Default)]
pub struct FirstPassStats {
    /// Frame display order.
    pub frame_number: u64,
    /// Average intra prediction error (SSE per pixel).
    pub intra_error: f64,
    /// Average inter prediction error.
    pub coded_error: f64,
    /// Average motion magnitude.
    pub mv_magnitude: f64,
    /// Percentage of blocks coded as intra.
    pub percent_intra: f64,
    /// Percentage of blocks with zero motion.
    pub percent_zero_mv: f64,
    /// Frame-level activity (variance).
    pub activity: f64,
    /// Estimated bits to encode at base QP.
    pub estimated_bits: u64,
}

/// Collected first-pass statistics for an entire sequence.
#[derive(Debug, Default)]
pub struct FirstPassData {
    pub stats: Vec<FirstPassStats>,
    pub total_frames: u64,
    pub avg_intra_error: f64,
    pub avg_coded_error: f64,
}

impl FirstPassData {
    /// Compute aggregate statistics after collecting all frame stats.
    pub fn finalize(&mut self) {
        if self.stats.is_empty() {
            return;
        }
        self.total_frames = self.stats.len() as u64;
        self.avg_intra_error =
            self.stats.iter().map(|s| s.intra_error).sum::<f64>() / self.total_frames as f64;
        self.avg_coded_error =
            self.stats.iter().map(|s| s.coded_error).sum::<f64>() / self.total_frames as f64;
    }

    /// Estimate the optimal QP for a frame based on first-pass stats.
    pub fn estimate_qp(&self, frame_idx: usize, target_bitrate: u64) -> u8 {
        if frame_idx >= self.stats.len() || target_bitrate == 0 {
            return 30;
        }

        let frame = &self.stats[frame_idx];
        let complexity_ratio = if self.avg_coded_error > 0.0 {
            frame.coded_error / self.avg_coded_error
        } else {
            1.0
        };

        // Higher complexity → higher QP (more bits needed but constrained)
        let base_qp = 30.0;
        let qp = base_qp + (complexity_ratio - 1.0) * 10.0;
        qp.clamp(0.0, 63.0) as u8
    }
}

/// Collect first-pass statistics for a frame.
///
/// Performs a simplified encode (intra-only, no output) to measure
/// frame complexity for second-pass QP optimization.
pub fn collect_first_pass_stats(
    y_plane: &[u8],
    width: usize,
    height: usize,
    stride: usize,
    frame_number: u64,
) -> FirstPassStats {
    let block_size = 16;
    let blocks_x = width.div_ceil(block_size);
    let blocks_y = height.div_ceil(block_size);
    let total_blocks = blocks_x * blocks_y;

    let mut total_intra_error: f64 = 0.0;
    let mut total_activity: f64 = 0.0;

    for by in 0..blocks_y {
        for bx in 0..blocks_x {
            let x0 = bx * block_size;
            let y0 = by * block_size;
            let bw = block_size.min(width - x0);
            let bh = block_size.min(height - y0);

            // Compute block variance (activity measure)
            let mut sum: u64 = 0;
            let mut sum_sq: u64 = 0;
            for r in 0..bh {
                for c in 0..bw {
                    let v = y_plane[(y0 + r) * stride + x0 + c] as u64;
                    sum += v;
                    sum_sq += v * v;
                }
            }
            let n = (bw * bh) as f64;
            let mean = sum as f64 / n;
            let variance = sum_sq as f64 / n - mean * mean;
            total_activity += variance;

            // Intra error: SSE of DC prediction
            let dc = mean as u8;
            let mut sse: f64 = 0.0;
            for r in 0..bh {
                for c in 0..bw {
                    let diff = y_plane[(y0 + r) * stride + x0 + c] as f64 - dc as f64;
                    sse += diff * diff;
                }
            }
            total_intra_error += sse / n;
        }
    }

    FirstPassStats {
        frame_number,
        intra_error: total_intra_error / total_blocks as f64,
        coded_error: total_intra_error / total_blocks as f64, // Same as intra for first pass
        mv_magnitude: 0.0,
        percent_intra: 100.0,
        percent_zero_mv: 100.0,
        activity: total_activity / total_blocks as f64,
        estimated_bits: (total_intra_error * 0.5) as u64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn first_pass_flat_frame() {
        let frame = vec![128u8; 64 * 64];
        let stats = collect_first_pass_stats(&frame, 64, 64, 64, 0);
        assert!(
            stats.intra_error < 1.0,
            "flat frame should have near-zero error"
        );
        assert!(
            stats.activity < 1.0,
            "flat frame should have near-zero activity"
        );
    }

    #[test]
    fn first_pass_complex_frame() {
        let mut frame = vec![0u8; 64 * 64];
        for r in 0..64 {
            for c in 0..64 {
                frame[r * 64 + c] = ((r * 7 + c * 13) % 256) as u8;
            }
        }
        let stats = collect_first_pass_stats(&frame, 64, 64, 64, 0);
        assert!(
            stats.intra_error > 10.0,
            "complex frame should have high error"
        );
        assert!(
            stats.activity > 100.0,
            "complex frame should have high activity"
        );
    }

    #[test]
    fn first_pass_data_finalize() {
        let mut data = FirstPassData::default();
        data.stats.push(FirstPassStats {
            intra_error: 100.0,
            coded_error: 100.0,
            ..Default::default()
        });
        data.stats.push(FirstPassStats {
            intra_error: 200.0,
            coded_error: 200.0,
            ..Default::default()
        });
        data.finalize();
        assert_eq!(data.total_frames, 2);
        assert!((data.avg_intra_error - 150.0).abs() < 0.01);
    }

    #[test]
    fn estimate_qp_average_complexity() {
        let mut data = FirstPassData::default();
        for i in 0..10 {
            data.stats.push(FirstPassStats {
                coded_error: 100.0 + i as f64 * 10.0,
                ..Default::default()
            });
        }
        data.finalize();
        let qp = data.estimate_qp(5, 1000);
        assert!(
            (25..=40).contains(&qp),
            "average frame should get moderate QP: {qp}"
        );
    }
}
