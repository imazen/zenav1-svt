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
//! Usage: `cargo run --release -p zenav1-svt --example decode_conformance -- <outdir> [chroma]`
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

/// Fine 4-pixel vertical stripes, 2 colors — deliberately forces the encoder
/// to WIN palette blocks (few colors + high local variance). This is the
/// regression guard for the palette filter_intra desync (fix a0b505b4f): a
/// winning palette block must not emit an extra use_filter_intra flag. Without
/// the fix, every size here desyncs under aomdec.
fn make_stripes(w: usize, h: usize) -> Vec<u8> {
    let mut v = vec![0u8; w * h];
    for r in 0..h {
        for c in 0..w {
            v[r * w + c] = if (c / 4) % 2 == 0 { 40 } else { 200 };
        }
    }
    v
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
    let arg2 = std::env::args().nth(2);
    let chroma_mode = arg2.as_deref() == Some("chroma");
    // Issue #9 item 6: the same corpus driven through the PUBLIC
    // `AvifEncoder` surface instead of `EncodePipeline` — `encode_yuv420`
    // (4:2:0, at the TRUE size, no caller-side padding) and `encode_y8`
    // (monochrome). This is what proves the AVIF wrapper emits real AV1: it
    // used to return three concatenated mono streams behind u32 length
    // prefixes, which no decoder accepts.
    let avif_mode = arg2.as_deref() == Some("avif");
    std::fs::create_dir_all(&outdir).expect("create outdir");
    if avif_mode {
        avif_corpus(&outdir);
        return;
    }

    #[allow(clippy::type_complexity)]
    // inline tuple documents the shape; a `type` alias would hide it
    let mut contents: Vec<(&str, fn(usize, usize) -> Vec<u8>)> = vec![
        ("gradient", make_gradient),
        ("uniform", make_uniform),
        ("edges", make_edges),
        ("stripes", make_stripes), // palette-forcing regression guard (a0b505b4f)
    ];
    if chroma_mode {
        // Luma gradient + chroma that actually carries content.
        contents.push(("color", make_gradient));
    }
    // Square sizes padded internally to 64-aligned. The odd multi-SB sizes
    // (80/96/112) were historical failure cases; all decode since the palette
    // filter_intra fix (a0b505b4f).
    let sizes = [32usize, 48, 64, 80, 96, 112, 128];
    // CLI-domain qps -> qindex {80, 128, 172, 220, 255} (see header note).
    let qps = [20u8, 32, 43, 55, 63];
    let speeds = [0u8, 1, 2, 3, 4, 5, 6, 8, 10];

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
                    let mut pipeline = EncodePipeline::new(pw as u32, ph as u32, speed, rc, 0, 1);
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

/// Issue #9 item 6: emit the decode corpus through `AvifEncoder`'s public
/// entry points.
///
/// Sizes are EVEN but deliberately not all 64-multiples: the 4:2:0 path
/// signals the true frame size and pads internally, so `98x66` must decode as
/// 98x66. Qualities span the CLI-qp range the wrapper can reach; speeds cover
/// both sides of the preset-6 boundary (below it the monochrome path refuses
/// partial superblocks, which is a typed error, not a stream).
fn avif_corpus(outdir: &str) {
    use svtav1::avif::AvifEncoder;
    let sizes = [32usize, 48, 64, 66, 98, 128];
    let qualities = [10.0f32, 35.0, 60.0, 85.0];
    let speeds = [1u8, 5, 6, 8, 10];
    let mut count = 0usize;
    for &sz in &sizes {
        for &q in &qualities {
            for &speed in &speeds {
                let enc = AvifEncoder::new().with_quality(q).with_speed(speed);
                let y = make_gradient(sz, sz);
                let (u, v) = make_chroma("color", sz / 2, sz / 2);
                let obu = enc
                    .encode_yuv420(&y, &u, &v, sz as u32, sz as u32, sz as u32)
                    .expect("AvifEncoder::encode_yuv420")
                    .data;
                let name = format!("avif420_{sz}x{sz}_q{q}_s{speed}.obu");
                std::fs::write(format!("{outdir}/{name}"), &obu).expect("write obu");
                println!("{name}\t{} bytes", obu.len());
                count += 1;

                // Monochrome twin. Below preset 6 the pipeline refuses a
                // partial superblock; that refusal is the CORRECT behaviour,
                // so record it rather than writing a stream.
                match enc.encode_y8(&y, sz as u32, sz as u32, sz as u32) {
                    Ok(r) => {
                        let name = format!("avifmono_{sz}x{sz}_q{q}_s{speed}.obu");
                        std::fs::write(format!("{outdir}/{name}"), &r.data).expect("write obu");
                        println!("{name}\t{} bytes", r.data.len());
                        count += 1;
                    }
                    Err(e) => println!("avifmono_{sz}x{sz}_q{q}_s{speed}\tREFUSED: {e}"),
                }
            }
        }
    }
    eprintln!("wrote {count} AvifEncoder streams to {outdir}");
}
