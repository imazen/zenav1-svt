//! Encode exactly one conformance case (gradient 64x64 CLI-qp50 s10) so
//! the `symtrace` feature's per-symbol log can be diffed against a
//! gdb-instrumented reference decoder. See tools/decode_conformance.sh.
//!
//! The qp argument is CLI-domain (0..63, C `--qp` semantics); the
//! pipeline maps it to qindex via quantizer_to_qindex (50 -> 200).

use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::rate_control::{RcConfig, RcMode};

fn main() {
    // args: [out] [content] [size] [qp] [speed]  (defaults: gradient 64 50 10)
    let args: Vec<String> = std::env::args().collect();
    let out = args.get(1).cloned().unwrap_or_else(|| "/tmp/trace_one.obu".into());
    let content = args.get(2).map(String::as_str).unwrap_or("gradient").to_string();
    let sz: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(64);
    let qp: u8 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(50);
    let speed: u8 = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(10);

    let sb = 64usize;
    let pw = sz.div_ceil(sb) * sb;
    let ph = pw;
    let mut small = vec![0u8; sz * sz];
    for r in 0..sz {
        for c in 0..sz {
            small[r * sz + c] = match content.as_str() {
                "uniform" => 128,
                "flat140" => 140,
                "flat250" => 250,
                "edges" => {
                    if (r / 8 + c / 8) % 2 == 0 { 32 } else { 224 }
                }
                _ => ((r * 255) / sz) as u8 ^ ((c * 3) & 0x3F) as u8,
            };
        }
    }
    let mut src = vec![128u8; pw * ph];
    for r in 0..sz {
        for c in 0..sz {
            src[r * pw + c] = small[r * sz + c];
        }
        for c in sz..pw {
            src[r * pw + c] = src[r * pw + sz - 1];
        }
    }
    for r in sz..ph {
        for c in 0..pw {
            src[r * pw + c] = src[(sz - 1) * pw + c];
        }
    }
    let rc = RcConfig { mode: RcMode::Cqp, qp, ..RcConfig::default() };
    let mut pipeline = EncodePipeline::new(pw as u32, ph as u32, speed, rc, 0, 1);
    let obu = pipeline.encode_frame(&src, pw);
    std::fs::write(&out, &obu).unwrap();
    eprintln!("wrote {} bytes to {out}", obu.len());
}
