//! Benchmark Rust svtav1 encoder vs C SVT-AV1.
//!
//! Reads Y4M files and encodes them, measuring time and output size.
//! Run with: cargo run -p svtav1 --example bench_vs_c --release -- /tmp/svtav1-bench/

use std::time::Instant;
use svtav1::avif::AvifEncoder;

fn read_y4m_y_plane(path: &str) -> (Vec<u8>, usize, usize) {
    let data = std::fs::read(path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"));
    // Parse Y4M header
    let header_end = data.iter().position(|&b| b == b'\n').unwrap();
    let header = std::str::from_utf8(&data[..header_end]).unwrap();
    let mut width = 0;
    let mut height = 0;
    for part in header.split(' ') {
        if let Some(w) = part.strip_prefix('W') {
            width = w.parse().unwrap();
        }
        if let Some(h) = part.strip_prefix('H') {
            height = h.parse().unwrap();
        }
    }
    // Skip header + "FRAME\n"
    let frame_start = data[header_end + 1..]
        .windows(6)
        .position(|w| w == b"FRAME\n")
        .unwrap()
        + header_end
        + 1
        + 6;
    let y_plane = data[frame_start..frame_start + width * height].to_vec();
    (y_plane, width, height)
}

fn main() {
    let dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/svtav1-bench/".to_string());

    println!("Rust svtav1 Encoder Benchmarks");
    println!("==============================\n");
    println!(
        "{:<20} {:>6} {:>8} {:>10} {:>10} {:>8}",
        "File", "WxH", "Q", "Time(ms)", "Bytes", "Mpix/s"
    );
    println!("{}", "-".repeat(72));

    for file in ["small", "medium", "large"] {
        let path = format!("{dir}/{file}.y4m");
        if !std::path::Path::new(&path).exists() {
            eprintln!("Skipping {path} (not found)");
            continue;
        }

        let (y_plane, width, height) = read_y4m_y_plane(&path);
        let pixels = width * height;
        let dim = format!("{}x{}", width, height);

        for quality in [30.0, 60.0, 90.0f32] {
            let enc = AvifEncoder::new().with_quality(quality).with_speed(8);

            // Warm up
            let _ = enc.encode_y8(&y_plane, width as u32, height as u32, width as u32);

            // Benchmark (average of 5 runs)
            let mut total_ns = 0u128;
            let mut result_size = 0;
            let runs = 5;
            for _ in 0..runs {
                let start = Instant::now();
                let result = enc
                    .encode_y8(&y_plane, width as u32, height as u32, width as u32)
                    .unwrap();
                total_ns += start.elapsed().as_nanos();
                result_size = result.data.len();
            }
            let avg_ms = total_ns / runs / 1_000_000;
            let mpix_per_sec = pixels as f64 / (total_ns as f64 / runs as f64 / 1e9) / 1e6;

            println!(
                "{:<20} {:>6} {:>8.0} {:>10} {:>10} {:>8.1}",
                file, dim, quality, avg_ms, result_size, mpix_per_sec
            );
        }
    }

    println!("\n--- DSP Primitive Throughput (release) ---");
    let sad_results = svtav1_dsp::bench::bench_sad_throughput();
    let txfm_results = svtav1_dsp::bench::bench_fwd_txfm_throughput();
    svtav1_dsp::bench::print_bench_results(&sad_results);
    svtav1_dsp::bench::print_bench_results(&txfm_results);
}
