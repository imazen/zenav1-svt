//! 4:2:0 pixel probe: encode a frame with a known Y gradient and nontrivial
//! U/V patterns, write the raw OBU stream plus the padded source planes so a
//! driver can decode with aomdec (-o y4m) and compare all three planes.
//!
//! Usage: cargo run --release -p svtav1 --example probe_420 -- <outdir> [size] [qp]
//!
//! The qp argument is CLI-domain (0..63, C `--qp` semantics); the
//! pipeline maps it to qindex via quantizer_to_qindex. (The old usage
//! line said "qindex" — that was the pre-split conflation.)

use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::rate_control::{RcConfig, RcMode};

fn main() {
    let outdir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/probe_420".to_string());
    let size: usize = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(64);
    let qp: u8 = std::env::args()
        .nth(3)
        .and_then(|s| s.parse().ok())
        .unwrap_or(30);
    std::fs::create_dir_all(&outdir).expect("create outdir");

    // The pipeline contract (like AvifEncoder / decode_conformance) is
    // 64-aligned frame dims — pad with edge replication like both do.
    let sb = 64usize;
    let w = size.div_ceil(sb) * sb;
    let h = size.div_ceil(sb) * sb;

    // Same luma gradient as decode_conformance's make_gradient, generated
    // at the requested size then edge-padded.
    let mut y = vec![0u8; w * h];
    for r in 0..h {
        for c in 0..w {
            let sr = r.min(size - 1);
            let sc = c.min(size - 1);
            y[r * w + c] = ((sr * 255) / size.max(1)) as u8 ^ ((sc * 3) & 0x3F) as u8;
        }
    }
    // Nontrivial chroma patterns (r, c in CHROMA coords), padded the same way.
    let cw = w / 2;
    let ch = h / 2;
    let csize = size / 2;
    let mut u = vec![0u8; cw * ch];
    let mut v = vec![0u8; cw * ch];
    for r in 0..ch {
        for c in 0..cw {
            let sr = r.min(csize.saturating_sub(1));
            let sc = c.min(csize.saturating_sub(1));
            u[r * cw + c] = (((sr * 3) & 0x7F) + 64) as u8;
            v[r * cw + c] = (((sc * 5) & 0x7F) + 64) as u8;
        }
    }

    let rc = RcConfig {
        mode: RcMode::Cqp,
        qp,
        ..RcConfig::default()
    };
    let mut pipeline = EncodePipeline::new(w as u32, h as u32, 4, rc, 0, 1).with_chroma_420(true);
    let obu = pipeline.encode_frame_420(&y, &u, &v, w);

    std::fs::write(format!("{outdir}/probe.obu"), &obu).unwrap();
    std::fs::write(format!("{outdir}/src_y.raw"), &y).unwrap();
    std::fs::write(format!("{outdir}/src_u.raw"), &u).unwrap();
    std::fs::write(format!("{outdir}/src_v.raw"), &v).unwrap();
    println!(
        "{}x{} qp {} (qindex {}) -> {} bytes",
        w,
        h,
        qp,
        svtav1_encoder::rate_control::qp_to_qindex(qp),
        obu.len()
    );
}
