//! Port half of the G4 performance gate (`tools/perf_gate.sh`).
//!
//! The timing sibling of `examples/identity_run.rs`: it generates the SAME
//! deterministic 4:2:0 content, writes the SAME raw I420 `.yuv` the C driver
//! consumes (so both encoders see identical input — apples-to-apples), encodes
//! it through `EncodePipeline` at the proven byte-identical still-picture CQP
//! config, writes the `.obu`, and MEASURES the encode.
//!
//! What is timed: ONLY `encode_frame_420` on a FRESH pipeline — the per-frame
//! encode work. `EncodePipeline::new` (the port's one-time setup, C's analogue
//! of `svt_av1_enc_init`) is excluded from the clock, exactly as the C harness
//! (`tools/perf_c_encode`) excludes `svt_av1_enc_init`. Each timed sample is a
//! fresh-pipeline first-frame KEY encode, matching C's fresh-handle single-frame
//! encode.
//!
//! Warmup: `[warmup]` fresh-pipeline encodes run first (untimed) to warm the
//! allocator / OS page cache / branch predictors; only the final encode is
//! reported. Symmetric with the C harness's warmup cycles.
//!
//! Usage: perf_encode <content> <width> <height> <cli_qp 0..63> <preset> <out_prefix> [warmup=1]
//!   content: uniform (y=128) | gradient (identity campaign's gradient)
//! Output (stdout, machine-readable, one line): "ENCODE_NS=<n> BYTES=<m>"
//!         everything else (notes) -> stderr, so the driver parses stdout clean.

use std::time::Instant;
use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::rate_control::{RcConfig, RcMode};

fn gen_content(content: &str, w: usize, h: usize) -> Vec<u8> {
    let mut y = vec![0u8; w * h];
    for r in 0..h {
        for c in 0..w {
            y[r * w + c] = match content {
                "uniform" => 128,
                // Spec'd by the identity campaign (matches identity_run.rs).
                "gradient" => (((r * 255) / h) as u8) ^ (((c * 3) & 0x3f) as u8),
                other => panic!("unknown content {other:?} (use uniform|gradient)"),
            };
        }
    }
    y
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 7 && args.len() != 8 {
        eprintln!(
            "usage: {} <content> <width> <height> <cli_qp 0..63> <preset> <out_prefix> [warmup=1]",
            args[0]
        );
        std::process::exit(2);
    }
    let content = args[1].as_str();
    let w: usize = args[2].parse().expect("width");
    let h: usize = args[3].parse().expect("height");
    let qp: u8 = args[4].parse().expect("cli_qp");
    let preset: u8 = args[5].parse().expect("preset");
    let prefix = &args[6];
    let warmup: usize = args.get(7).map(|s| s.parse().expect("warmup")).unwrap_or(1);

    assert!(w % 2 == 0 && h % 2 == 0, "perf harness uses even dims (floor==ceiling chroma)");

    let y = gen_content(content, w, h);
    let (cw, ch) = (w / 2, h / 2);
    let u = vec![128u8; cw * ch];
    let v = vec![128u8; cw * ch];

    // Write the raw I420 8-bit .yuv the C driver (tools/perf_c_encode) reads —
    // the ONE byte stream both encoders consume, keeping the comparison honest.
    let mut yuv = Vec::with_capacity(w * h + 2 * cw * ch);
    yuv.extend_from_slice(&y);
    yuv.extend_from_slice(&u);
    yuv.extend_from_slice(&v);
    std::fs::write(format!("{prefix}.yuv"), &yuv).expect("write .yuv");

    // Fresh-pipeline encode at the proven byte-identical still-picture CQP
    // config (identity_run.rs / capture_c_trace.c): bd8, 4:2:0, tiles 0/0, SB
    // derived by C's own rule. `new(w,h,preset,rc, hierarchical_levels=0,
    // intra_period=1)` == allintra/still.
    let build = || {
        let rc = RcConfig { mode: RcMode::Cqp, qp, ..RcConfig::default() };
        EncodePipeline::new(w as u32, h as u32, preset, rc, 0, 1)
            .with_bit_depth(8)
            .with_tile_rows_log2(0)
            .with_tile_cols_log2(0)
            .with_sb_size(None)
            .with_chroma_420(true)
    };

    // Untimed warmup: fresh pipeline each time (frame_count=0 first-frame path).
    for _ in 0..warmup {
        let mut p = build();
        let _ = p.encode_frame_420(&y, &u, &v, w);
    }

    // Timed sample: fresh pipeline (setup untimed), time only encode_frame_420.
    let mut p = build();
    if p.sb128_fallback {
        // Loud on stderr: a fallback means the port coded a different SB
        // geometry than C, so this cell is NOT byte-comparable. The driver's
        // per-cell `cmp` catches it too, but flag it here as well.
        eprintln!(
            "perf_encode: SB128-FALLBACK at {w}x{h} preset {preset} — not byte-comparable to C"
        );
    }
    let t = Instant::now();
    let obu = p.encode_frame_420(&y, &u, &v, w);
    let ns = t.elapsed().as_nanos();

    std::fs::write(format!("{prefix}.obu"), &obu).expect("write .obu");
    // The ONLY stdout line — the driver greps it.
    println!("ENCODE_NS={ns} BYTES={}", obu.len());
}
