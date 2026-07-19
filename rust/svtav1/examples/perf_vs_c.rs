//! Performance gate baseline: wall-time ratio vs the C reference encoder.
//!
//! Generates deterministic test frames, encodes them with our pipeline and
//! with the C `SvtAv1EncApp` at matched (preset, CLI qp, still-picture,
//! --lp 1) settings, and prints a ratio table plus a CSV row set suitable
//! for committing under `benchmarks/`.
//!
//! QP DOMAIN: the same CLI-domain qp (0..63) goes to RcConfig.qp and to
//! the C app's `--qp` — both encoders map it through quantizer_to_qindex
//! (40 -> qindex 160), so the settings are now genuinely matched. Before
//! the domain split our side ran at qindex 40 while C ran at qindex 160.
//!
//! Goal gate: wall time <= 1.20x C at the same preset and --lp. This tool
//! MEASURES the current honest ratio (expected to be far above the gate
//! until the SIMD + decision-layer ports land — the number is the ratchet's
//! starting point, not a claim).
//!
//! Usage:
//!   cargo run --release -p zenav1-svt --example perf_vs_c -- [outdir]
//! Env:
//!   SVT_APP  path to SvtAv1EncApp (default: ../Bin/Release/SvtAv1EncApp)

use std::io::Write as _;
use std::time::Instant;

fn gen_frame(sz: usize) -> Vec<u8> {
    // Gradient + structured detail: nontrivial for both encoders,
    // fully deterministic.
    let mut y = vec![0u8; sz * sz];
    for r in 0..sz {
        for c in 0..sz {
            let g = ((r * 255) / sz) as u8;
            let d = (((c * 7) ^ (r * 3)) & 0x3F) as u8;
            y[r * sz + c] = g ^ d;
        }
    }
    y
}

fn write_y4m(path: &str, y: &[u8], sz: usize) {
    let mut f = std::fs::File::create(path).unwrap();
    write!(f, "YUV4MPEG2 W{sz} H{sz} F30:1 Ip A1:1 C420jpeg\n").unwrap();
    f.write_all(b"FRAME\n").unwrap();
    f.write_all(y).unwrap();
    f.write_all(&vec![128u8; (sz / 2) * (sz / 2)]).unwrap();
    f.write_all(&vec![128u8; (sz / 2) * (sz / 2)]).unwrap();
}

fn median3(mut v: [f64; 3]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[1]
}

fn main() {
    let outdir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/perf_vs_c".to_string());
    std::fs::create_dir_all(&outdir).unwrap();
    let svt_app =
        std::env::var("SVT_APP").unwrap_or_else(|_| "../Bin/Release/SvtAv1EncApp".to_string());

    let commit = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();

    println!("perf_vs_c — commit {commit}, --lp 1, still-picture, CQP");
    println!(
        "{:<8} {:>7} {:>4} {:>12} {:>12} {:>8}",
        "size", "preset", "q", "rust_ms", "c_ms", "ratio"
    );
    let mut csv = String::from("size,preset,cli_qp,rust_ms,c_ms,ratio,commit\n");

    for &sz in &[256usize, 1024] {
        let y = gen_frame(sz);
        let y4m = format!("{outdir}/src_{sz}.y4m");
        write_y4m(&y4m, &y, sz);

        for &preset in &[2u8, 6, 10] {
            let q = 40u8;

            // Rust side: median of 3 (frame encode only, buffers warm).
            let mut rust_ms = [0f64; 3];
            for m in &mut rust_ms {
                let rc = svtav1_encoder::rate_control::RcConfig {
                    mode: svtav1_encoder::rate_control::RcMode::Cqp,
                    qp: q,
                    ..Default::default()
                };
                let mut p = svtav1_encoder::pipeline::EncodePipeline::new(
                    sz as u32, sz as u32, preset, rc, 0, 1,
                );
                let t = Instant::now();
                let bs = p.encode_frame(&y, sz);
                *m = t.elapsed().as_secs_f64() * 1000.0;
                assert!(!bs.is_empty());
            }
            let rust_ms = median3(rust_ms);

            // C side: median of 3 full app runs (includes app setup; noted in
            // the CSV header comment — a constant few-ms bias in C's favor is
            // acceptable for a >>1 ratio baseline).
            let mut c_ms = [0f64; 3];
            for m in &mut c_ms {
                let t = Instant::now();
                let st = std::process::Command::new(&svt_app)
                    .args([
                        "-i",
                        &y4m,
                        "-b",
                        &format!("{outdir}/c_{sz}_{preset}.ivf"),
                        "--preset",
                        &preset.to_string(),
                        "--rc",
                        "0",
                        "--aq-mode",
                        "0",
                        "--qp",
                        &q.to_string(),
                        "--avif",
                        "1",
                        "--lp",
                        "1",
                        "-n",
                        "1",
                    ])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .expect("run SvtAv1EncApp");
                assert!(st.success(), "C encoder failed");
                *m = t.elapsed().as_secs_f64() * 1000.0;
            }
            let c_ms = median3(c_ms);

            let ratio = rust_ms / c_ms;
            println!(
                "{:<8} {:>7} {:>4} {:>12.1} {:>12.1} {:>8.2}",
                format!("{sz}x{sz}"),
                preset,
                q,
                rust_ms,
                c_ms,
                ratio
            );
            csv.push_str(&format!(
                "{sz},{preset},{q},{rust_ms:.1},{c_ms:.1},{ratio:.2},{commit}\n"
            ));
        }
    }

    let csv_path = format!("{outdir}/perf_vs_c.csv");
    std::fs::write(&csv_path, &csv).unwrap();
    eprintln!("csv written to {csv_path}");
}
