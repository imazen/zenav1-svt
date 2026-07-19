//! Performance measurement utilities for DSP functions.
//!
//! Provides lightweight benchmarking to compare Rust DSP performance
//! against C SVT-AV1. Uses wall-clock timing with enough iterations
//! to get stable results.

use alloc::string::String;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

/// Result of a single benchmark run.
#[derive(Debug, Clone)]
pub struct BenchResult {
    pub name: String,
    pub iterations: u64,
    pub total_ns: u64,
    pub ns_per_iter: u64,
    /// Throughput in megapixels/second (if applicable).
    pub mpix_per_sec: Option<f64>,
}

/// Generate a deterministic test image (gradient + noise pattern).
pub fn generate_test_image(width: usize, height: usize, seed: u8) -> Vec<u8> {
    let mut img = vec![0u8; width * height];
    for r in 0..height {
        for c in 0..width {
            let gradient = ((r * 255 / height.max(1)) + (c * 128 / width.max(1))) as u8;
            let noise = seed.wrapping_mul((r * width + c) as u8).wrapping_add(37);
            img[r * width + c] = gradient.wrapping_add(noise >> 2);
        }
    }
    img
}

/// Generate a pair of test images with known offset for ME benchmarks.
pub fn generate_test_image_pair(
    width: usize,
    height: usize,
    dx: i32,
    dy: i32,
) -> (Vec<u8>, Vec<u8>) {
    let src = generate_test_image(width, height, 42);
    let mut ref_img = vec![128u8; width * height];

    for r in 0..height {
        for c in 0..width {
            let sr = (r as i32 + dy).clamp(0, height as i32 - 1) as usize;
            let sc = (c as i32 + dx).clamp(0, width as i32 - 1) as usize;
            ref_img[r * width + c] = src[sr * width + sc];
        }
    }

    (src, ref_img)
}

/// Benchmark SAD across different block sizes and report throughput.
pub fn bench_sad_throughput() -> Vec<BenchResult> {
    let mut results = Vec::new();
    let sizes = [(8, 8), (16, 16), (32, 32), (64, 64)];

    for (w, h) in sizes {
        let src = generate_test_image(w, h, 42);
        let ref_ = generate_test_image(w, h, 99);

        // Warm up
        for _ in 0..100 {
            let _ = crate::sad::sad(&src, w, &ref_, w, w, h);
        }

        let iterations = 100_000u64;
        let start = std::time::Instant::now();
        for _ in 0..iterations {
            let _ = std::hint::black_box(crate::sad::sad(
                std::hint::black_box(&src),
                w,
                std::hint::black_box(&ref_),
                w,
                w,
                h,
            ));
        }
        let elapsed = start.elapsed();
        let total_ns = elapsed.as_nanos() as u64;
        let ns_per_iter = total_ns / iterations;
        let pixels = (w * h) as f64;
        let mpix_per_sec = pixels * iterations as f64 / elapsed.as_secs_f64() / 1e6;

        results.push(BenchResult {
            name: alloc::format!("sad_{w}x{h}"),
            iterations,
            total_ns,
            ns_per_iter,
            mpix_per_sec: Some(mpix_per_sec),
        });
    }

    results
}

/// Benchmark forward transforms and report throughput.
pub fn bench_fwd_txfm_throughput() -> Vec<BenchResult> {
    let mut results = Vec::new();

    // 4x4 DCT
    {
        let input: Vec<i32> = (0..16).map(|i| i * 7 - 50).collect();
        let mut output = [0i32; 16];
        let iterations = 500_000u64;
        let start = std::time::Instant::now();
        for _ in 0..iterations {
            crate::fwd_txfm::fwd_txfm2d_4x4_dct_dct(
                std::hint::black_box(&input),
                std::hint::black_box(&mut output),
                4,
            );
        }
        let elapsed = start.elapsed();
        results.push(BenchResult {
            name: "fwd_txfm2d_4x4_dct".to_string(),
            iterations,
            total_ns: elapsed.as_nanos() as u64,
            ns_per_iter: elapsed.as_nanos() as u64 / iterations,
            mpix_per_sec: Some(16.0 * iterations as f64 / elapsed.as_secs_f64() / 1e6),
        });
    }

    // 8x8 DCT
    {
        let input: Vec<i32> = (0..64).map(|i| i * 3 - 100).collect();
        let mut output = [0i32; 64];
        let iterations = 200_000u64;
        let start = std::time::Instant::now();
        for _ in 0..iterations {
            crate::fwd_txfm::fwd_txfm2d_8x8_dct_dct(
                std::hint::black_box(&input),
                std::hint::black_box(&mut output),
                8,
            );
        }
        let elapsed = start.elapsed();
        results.push(BenchResult {
            name: "fwd_txfm2d_8x8_dct".to_string(),
            iterations,
            total_ns: elapsed.as_nanos() as u64,
            ns_per_iter: elapsed.as_nanos() as u64 / iterations,
            mpix_per_sec: Some(64.0 * iterations as f64 / elapsed.as_secs_f64() / 1e6),
        });
    }

    results
}

// NOTE: bench_encode_block_throughput was moved to the top-level svtav1 crate
// because it depends on svtav1_encoder, which cannot be a dependency of svtav1-dsp
// (encoder depends on dsp, so that would be circular).

/// Print benchmark results in a table format.
pub fn print_bench_results(results: &[BenchResult]) {
    std::println!(
        "{:<30} {:>10} {:>10} {:>12}",
        "Function",
        "ns/iter",
        "iters",
        "Mpix/s"
    );
    std::println!("{}", "-".repeat(66));
    for r in results {
        let mpix = r
            .mpix_per_sec
            .map_or_else(|| "—".into(), |v| alloc::format!("{v:.1}"));
        std::println!(
            "{:<30} {:>10} {:>10} {:>12}",
            r.name,
            r.ns_per_iter,
            r.iterations,
            mpix
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_test_image_deterministic() {
        let a = generate_test_image(16, 16, 42);
        let b = generate_test_image(16, 16, 42);
        assert_eq!(a, b);
    }

    #[test]
    fn generate_test_image_different_seeds() {
        let a = generate_test_image(16, 16, 42);
        let b = generate_test_image(16, 16, 99);
        assert_ne!(a, b);
    }

    #[test]
    fn generate_test_image_pair_offset() {
        let (src, ref_img) = generate_test_image_pair(64, 64, 4, 4);
        assert_eq!(src.len(), 64 * 64);
        assert_eq!(ref_img.len(), 64 * 64);
        assert_ne!(src, ref_img);
    }

    #[test]
    fn bench_sad_runs() {
        let results = bench_sad_throughput();
        assert!(results.len() >= 4);
        for r in &results {
            assert!(r.ns_per_iter > 0);
            assert!(r.mpix_per_sec.unwrap() > 0.0);
        }
    }

    #[test]
    fn bench_txfm_runs() {
        let results = bench_fwd_txfm_throughput();
        assert!(results.len() >= 2);
        for r in &results {
            assert!(r.ns_per_iter > 0);
        }
    }

    // bench_encode_block_runs test moved to top-level svtav1 crate
    // (circular dependency: dsp cannot depend on encoder)
}
