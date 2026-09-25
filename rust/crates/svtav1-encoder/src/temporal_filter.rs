//! The C noise estimator used by temporal filtering: [`estimate_noise_fp16`],
//! a bit-exact port of `svt_estimate_noise_fp16_c` (temporal_filtering.c:3555),
//! validated in `tests/c_parity_temporal.rs`.
//!
//! The ported temporal filter itself is `port_temporal_filtering` /
//! `port_tf_driver`. This module used to also hold a homegrown
//! similarity-weighted average (`temporal_filter`) and an f64 Laplacian noise
//! proxy (`estimate_noise`), neither of them a port of C and neither called by
//! the encoder; both were deleted on 2026-09-25 (plan S1).

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
