//! Rust half of the bitstream-identity harness (tools/identity_diff.sh).
//!
//! Generates deterministic 4:2:0 content, writes it as a raw I420 .yuv
//! (the exact bytes the C driver `tools/capture_c_trace` consumes — both
//! encoders see identical input), encodes it through `EncodePipeline` in
//! 420 still-picture CQP mode, and writes the raw OBU stream.
//!
//! Build with `--features symtrace` and redirect stderr to capture the
//! per-symbol arithmetic-coder trace in the same format the wrapped C
//! library emits (`W CDF ...` / `W BOOL ...` lines).
//!
//! Usage: identity_run <content> <width> <height> <cli_qp 0..63> <preset> <out_prefix>
//!   content: uniform  — y = 128 everywhere
//!            gradient — y[r][c] = ((r*255/h) ^ ((c*3) & 0x3f)), spec'd by
//!                       the identity campaign brief (trace_one's gradient)
//!   u = v = 128 for both.
//! Writes <out_prefix>.yuv and <out_prefix>.obu.

use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::rate_control::{RcConfig, RcMode};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 7 {
        eprintln!(
            "usage: {} <content> <width> <height> <cli_qp 0..63> <preset> <out_prefix>",
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
    assert!(
        w % 64 == 0 && h % 64 == 0,
        "identity harness requires 64-aligned dims (partial-SB support pending, CLAUDE.md gap 5)"
    );

    let mut y = vec![0u8; w * h];
    for r in 0..h {
        for c in 0..w {
            y[r * w + c] = match content {
                "uniform" => 128,
                "gradient" => (((r * 255) / h) as u8) ^ (((c * 3) & 0x3f) as u8),
                other => panic!("unknown content {other:?} (use uniform|gradient)"),
            };
        }
    }
    let u = vec![128u8; (w / 2) * (h / 2)];
    let v = vec![128u8; (w / 2) * (h / 2)];

    // Raw I420 input for the C driver: identical bytes on both sides.
    let mut yuv = Vec::with_capacity(w * h * 3 / 2);
    yuv.extend_from_slice(&y);
    yuv.extend_from_slice(&u);
    yuv.extend_from_slice(&v);
    std::fs::write(format!("{prefix}.yuv"), &yuv).expect("write .yuv");

    let rc = RcConfig {
        mode: RcMode::Cqp,
        qp, // CLI domain 0..63, same as the C driver's cfg.qp
        ..RcConfig::default()
    };
    let mut pipeline =
        EncodePipeline::new(w as u32, h as u32, preset, rc, 0, 1).with_chroma_420(true);
    let obu = pipeline.encode_frame_420(&y, &u, &v, w);
    std::fs::write(format!("{prefix}.obu"), &obu).expect("write .obu");
    println!(
        "identity_run: {content} {w}x{h} qp={qp} preset={preset} -> {} bytes",
        obu.len()
    );
}
