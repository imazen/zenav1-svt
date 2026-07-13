//! Decode-conformance corpus generator.
//!
//! Encodes a matrix of (content x size x qp x speed) still frames with the
//! Rust pipeline and writes each raw OBU stream to a file. A driver script
//! (`tools/decode_conformance.sh`) then feeds every stream to the AV1
//! reference decoder (`aomdec`) — the project's decode-conformance gate.
//!
//! The matrix deliberately includes every historical PASS and FAIL case from
//! STATUS.md (all-skip uniform frames, q70 gradients, 80/96/112 multi-SB
//! sizes, speed sweeps) so regressions and fixes are both visible.
//!
//! Usage: `cargo run --release -p svtav1 --example decode_conformance -- <outdir>`

use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::rate_control::{RcConfig, RcMode};

fn make_gradient(w: usize, h: usize) -> Vec<u8> {
    let mut v = vec![0u8; w * h];
    for r in 0..h {
        for c in 0..w {
            v[r * w + c] = ((r * 255) / h.max(1)) as u8 ^ ((c * 3) & 0x3F) as u8;
        }
    }
    v
}

fn make_uniform(w: usize, h: usize) -> Vec<u8> {
    vec![128u8; w * h]
}

fn make_edges(w: usize, h: usize) -> Vec<u8> {
    let mut v = vec![0u8; w * h];
    for r in 0..h {
        for c in 0..w {
            v[r * w + c] = if (r / 8 + c / 8) % 2 == 0 { 32 } else { 224 };
        }
    }
    v
}

fn main() {
    let outdir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/decode_conformance".to_string());
    std::fs::create_dir_all(&outdir).expect("create outdir");

    let contents: [(&str, fn(usize, usize) -> Vec<u8>); 3] = [
        ("gradient", make_gradient),
        ("uniform", make_uniform),
        ("edges", make_edges),
    ];
    // Square sizes padded internally to 64-aligned; the multi-SB odd sizes
    // (80/96/112) are historical failure cases.
    let sizes = [32usize, 48, 64, 80, 96, 112, 128];
    let qps = [30u8, 50, 60, 70, 90];
    let speeds = [2u8, 4, 6, 8, 10];

    let mut count = 0usize;
    for (cname, generator) in contents {
        for &sz in &sizes {
            for &qp in &qps {
                for &speed in &speeds {
                    // Pad to superblock alignment exactly like AvifEncoder.
                    let sb = 64usize;
                    let pw = sz.div_ceil(sb) * sb;
                    let ph = sz.div_ceil(sb) * sb;
                    let src_small = generator(sz, sz);
                    let mut src = vec![128u8; pw * ph];
                    for r in 0..sz {
                        for c in 0..sz {
                            src[r * pw + c] = src_small[r * sz + c];
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

                    let rc = RcConfig {
                        mode: RcMode::Cqp,
                        qp,
                        ..RcConfig::default()
                    };
                    let mut pipeline =
                        EncodePipeline::new(pw as u32, ph as u32, speed, rc, 0, 1);
                    let obu = pipeline.encode_frame(&src, pw);

                    let name = format!("{cname}_{sz}x{sz}_q{qp}_s{speed}.obu");
                    std::fs::write(format!("{outdir}/{name}"), &obu).expect("write obu");
                    println!("{name}\t{} bytes", obu.len());
                    count += 1;
                }
            }
        }
    }
    eprintln!("wrote {count} streams to {outdir}");
}
