//! Encode exactly one conformance case (gradient 64x64 q50 s10) so the
//! `symtrace` feature's per-symbol log can be diffed against a
//! gdb-instrumented reference decoder. See tools/decode_conformance.sh.

use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::rate_control::{RcConfig, RcMode};

fn main() {
    let (w, h) = (64usize, 64usize);
    let mut src = vec![0u8; w * h];
    for r in 0..h {
        for c in 0..w {
            src[r * w + c] = ((r * 255) / h) as u8 ^ ((c * 3) & 0x3F) as u8;
        }
    }
    let rc = RcConfig {
        mode: RcMode::Cqp,
        qp: 50,
        ..RcConfig::default()
    };
    let mut pipeline = EncodePipeline::new(w as u32, h as u32, 10, rc, 0, 1);
    let obu = pipeline.encode_frame(&src, w);
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/trace_one.obu".to_string());
    std::fs::write(&out, &obu).unwrap();
    eprintln!("wrote {} bytes to {out}", obu.len());
}
