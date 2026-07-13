//! Decode-conformance corpus generator.
//!
//! Encodes a matrix of (content x size x qp x speed) still frames with the
//! Rust pipeline and writes each raw OBU stream to a file. A driver script
//! (`tools/decode_conformance.sh`) then feeds every stream to the AV1
//! reference decoder (`aomdec`) — the project's decode-conformance gate.
//!
//! The matrix deliberately includes every historical PASS and FAIL case from
//! STATUS.md (all-skip uniform frames, high-q gradients, 80/96/112 multi-SB
//! sizes, speed sweeps) so regressions and fixes are both visible.
//!
//! QP DOMAIN: the qp values are CLI-domain (0..63, C `--qp` semantics) and
//! map through quantizer_to_qindex — {20, 32, 43, 55, 63} hit qindex
//! {80, 128, 172, 220, 255}, spanning all four CDF q buckets and the high
//! qindex range where deblock levels are material. (The old {30..90} list
//! predates the domain split: values ran as qindexes 30..63 after the
//! CLI clamp, so q70/q90 were duplicate qindex-63 cells.)
//!
//! Usage: `cargo run --release -p svtav1 --example decode_conformance -- <outdir> [chroma]`
//!
//! With the optional `chroma` mode argument the same matrix is encoded via
//! `encode_frame_420` (mono_chrome=0, NumPlanes=3): the three mono contents
//! get flat u=v=128 chroma, plus a fourth `color` content whose chroma
//! planes carry real patterns (u=((r*3)&0x7F)+64, v=((c*5)&0x7F)+64).

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

/// Chroma plane pair for the 420 matrix: `color` content gets real
/// patterns (r, c in CHROMA coords), everything else flat 128.
fn make_chroma(cname: &str, cw: usize, chh: usize) -> (Vec<u8>, Vec<u8>) {
    if cname == "color" {
        let mut u = vec![0u8; cw * chh];
        let mut v = vec![0u8; cw * chh];
        for r in 0..chh {
            for c in 0..cw {
                u[r * cw + c] = (((r * 3) & 0x7F) + 64) as u8;
                v[r * cw + c] = (((c * 5) & 0x7F) + 64) as u8;
            }
        }
        (u, v)
    } else {
        (vec![128u8; cw * chh], vec![128u8; cw * chh])
    }
}

fn main() {
    let outdir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/decode_conformance".to_string());
    let chroma_mode = std::env::args().nth(2).as_deref() == Some("chroma");
    std::fs::create_dir_all(&outdir).expect("create outdir");

    let mut contents: Vec<(&str, fn(usize, usize) -> Vec<u8>)> = vec![
        ("gradient", make_gradient),
        ("uniform", make_uniform),
        ("edges", make_edges),
    ];
    if chroma_mode {
        // Luma gradient + chroma that actually carries content.
        contents.push(("color", make_gradient));
    }
    // Square sizes padded internally to 64-aligned; the multi-SB odd sizes
    // (80/96/112) are historical failure cases.
    let sizes = [32usize, 48, 64, 80, 96, 112, 128];
    // CLI-domain qps -> qindex {80, 128, 172, 220, 255} (see header note).
    let qps = [20u8, 32, 43, 55, 63];
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
                    let obu = if chroma_mode {
                        pipeline = pipeline.with_chroma_420(true);
                        let (u, v) = make_chroma(cname, pw / 2, ph / 2);
                        pipeline.encode_frame_420(&src, &u, &v, pw)
                    } else {
                        pipeline.encode_frame(&src, pw)
                    };

                    let name = format!("{cname}_{sz}x{sz}_q{qp}_s{speed}.obu");
                    std::fs::write(format!("{outdir}/{name}"), &obu).expect("write obu");
                    println!("{name}\t{} bytes", obu.len());
                    count += 1;
                }
            }
        }
    }
    let mode = if chroma_mode { "chroma-420" } else { "mono" };
    eprintln!("wrote {count} {mode} streams to {outdir}");
}
