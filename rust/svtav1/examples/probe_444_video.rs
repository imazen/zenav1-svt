//! 4:4:4 INTER probe: encode N frames with real translational motion and
//! nontrivial FULL-RESOLUTION U/V, write the concatenated OBU stream plus
//! each frame's final reconstruction so a driver can decode with aomdec
//! and compare all three planes of every frame.
//!
//! Usage: cargo run --release -p zenav1-svt --example probe_444_video --
//!        <outdir> <size> <qp> <preset> <frames>
//!
//! Frame i is frame 0 shifted by (i, i/2) pixels plus a 16x16 square that
//! moves diagonally — the inter leaves see both a global-ish translation
//! and a local moving block, on luma AND chroma.

use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::rate_control::{RcConfig, RcMode};
use svtav1_types::chroma::ChromaFormat;

fn frame_planes(w: usize, h: usize, i: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let mut y = vec![0u8; w * h];
    let mut u = vec![0u8; w * h];
    let mut v = vec![0u8; w * h];
    let (dx, dy) = (i, i / 2);
    for r in 0..h {
        for c in 0..w {
            // Source coords of the base pattern this pixel was translated
            // from (clamp = the edge content a partial MV lands on).
            let sr = r.saturating_sub(dy).min(h - 1);
            let sc = c.saturating_sub(dx).min(w - 1);
            y[r * w + c] = ((sr * 255) / h.max(1)) as u8 ^ ((sc * 3) & 0x3F) as u8;
            u[r * w + c] = (((sr * 3) & 0x7F) + 64) as u8 ^ (((sc / 4) & 1) as u8 * 40);
            v[r * w + c] = (((sc * 5) & 0x7F) + 64) as u8;
        }
    }
    // A moving 16x16 block with distinct chroma — the local-motion case the
    // global shift can't produce.
    let bx = (i * 5) % w.saturating_sub(16).max(1);
    let by = (i * 3) % h.saturating_sub(16).max(1);
    for r in by..(by + 16).min(h) {
        for c in bx..(bx + 16).min(w) {
            y[r * w + c] = (30 + ((r + c) & 0x1F)) as u8;
            u[r * w + c] = 200u8.wrapping_sub(((r * 7) & 0x3F) as u8);
            v[r * w + c] = (40 + ((c * 11) & 0x3F)) as u8;
        }
    }
    (y, u, v)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let outdir = args
        .next()
        .unwrap_or_else(|| "target/probe_444_video".to_string());
    let size: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(128);
    let qp: u8 = args.next().and_then(|s| s.parse().ok()).unwrap_or(30);
    let preset: u8 = args.next().and_then(|s| s.parse().ok()).unwrap_or(6);
    let frames: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(4);
    std::fs::create_dir_all(&outdir).expect("create outdir");

    let (w, h) = (size, size);
    let rc = RcConfig {
        mode: RcMode::Cqp,
        qp,
        ..RcConfig::default()
    };
    // intra_period 64: frame 0 keys, every later frame is inter.
    let mut pipeline = EncodePipeline::new(w as u32, h as u32, preset, rc, 0, 64)
        .with_chroma_format(Some(ChromaFormat::Yuv444))
        .with_recon_output(true);

    let mut stream = Vec::new();
    let (aw, ah) = (w.div_ceil(8) * 8, h.div_ceil(8) * 8);
    for i in 0..frames {
        let (y, u, v) = frame_planes(w, h, i);
        let obu = match pipeline.try_encode_frame_444(&y, &u, &v, w) {
            Ok(obu) => obu,
            Err(e) => {
                eprintln!("REFUSED f{i}: {e}");
                std::process::exit(2);
            }
        };
        stream.extend_from_slice(&obu);
        // Same layout as probe_444's recon.raw: luma true-dims, then U/V at
        // aligned stride*rows (the aligned chroma planes at 4:4:4).
        if let Some((ry, ru, rv)) = pipeline.last_recon.as_ref() {
            let mut out = Vec::with_capacity(w * h + 2 * aw * ah);
            for r in 0..h {
                out.extend_from_slice(&ry[r * aw..r * aw + w]);
            }
            for r in 0..ah {
                out.extend_from_slice(&ru[r * aw..r * aw + aw]);
            }
            for r in 0..ah {
                out.extend_from_slice(&rv[r * aw..r * aw + aw]);
            }
            std::fs::write(format!("{outdir}/recon.f{i}"), &out).unwrap();
        }
    }
    std::fs::write(format!("{outdir}/probe.obu"), &stream).unwrap();
    println!(
        "{w}x{h} qp {qp} p{preset} frames {frames} -> {} bytes (aligned chroma {aw}x{ah})",
        stream.len()
    );
}
