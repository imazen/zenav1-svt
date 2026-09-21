//! 4:4:4 pixel probe: encode a key frame with a known Y gradient and
//! nontrivial FULL-RESOLUTION U/V patterns, write the raw OBU stream plus
//! the encoder's final reconstruction so a driver can decode with aomdec
//! (-o y4m / --rawvideo) and compare all three planes.
//!
//! Usage: cargo run --release -p zenav1-svt --example probe_444 -- <outdir> [size] [qp]
//!
//! The qp argument is CLI-domain (0..63, C `--qp` semantics); the
//! pipeline maps it to qindex via quantizer_to_qindex.

use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::rate_control::{RcConfig, RcMode};
use svtav1_types::chroma::ChromaFormat;

fn main() {
    let outdir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/probe_444".to_string());
    let size: usize = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(64);
    let qp: u8 = std::env::args()
        .nth(3)
        .and_then(|s| s.parse().ok())
        .unwrap_or(30);
    std::fs::create_dir_all(&outdir).expect("create outdir");

    // TRUE dims exactly as given (the port's FrameDims pads to 8-aligned
    // internally); chroma is FULL-resolution at 4:4:4.
    let (w, h) = (size, size);

    let mut y = vec![0u8; w * h];
    for r in 0..h {
        for c in 0..w {
            y[r * w + c] = ((r * 255) / w.max(1)) as u8 ^ ((c * 3) & 0x3F) as u8;
        }
    }
    let mut u = vec![0u8; w * h];
    let mut v = vec![0u8; w * h];
    for r in 0..h {
        for c in 0..w {
            u[r * w + c] = (((r * 3) & 0x7F) + 64) as u8 ^ (((c / 4) & 1) as u8 * 40);
            v[r * w + c] = (((c * 5) & 0x7F) + 64) as u8;
        }
    }

    let rc = RcConfig {
        mode: RcMode::Cqp,
        qp,
        ..RcConfig::default()
    };
    let mut pipeline = EncodePipeline::new(w as u32, h as u32, 4, rc, 0, 1)
        .with_chroma_format(Some(ChromaFormat::Yuv444))
        .with_recon_output(true);
    let obu = match pipeline.try_encode_frame_444(&y, &u, &v, w) {
        Ok(obu) => obu,
        Err(e) => {
            eprintln!("REFUSED: {e}");
            std::process::exit(2);
        }
    };

    std::fs::write(format!("{outdir}/probe.obu"), &obu).unwrap();
    std::fs::write(format!("{outdir}/src_y.raw"), &y).unwrap();
    std::fs::write(format!("{outdir}/src_u.raw"), &u).unwrap();
    std::fs::write(format!("{outdir}/src_v.raw"), &v).unwrap();

    // The final (deblock -> CDEF -> LR) reconstruction, cropped to the true
    // dims — `last_recon` chroma planes are aligned-stride at 4:4:4.
    if let Some((ry, ru, rv)) = pipeline.last_recon.as_ref() {
        let (aw, ah) = (w.div_ceil(8) * 8, h.div_ceil(8) * 8);
        let mut dump = Vec::with_capacity(w * h * 3);
        for r in 0..h {
            dump.extend_from_slice(&ry[r * aw..r * aw + w]);
        }
        for r in 0..ah {
            dump.extend_from_slice(&ru[r * aw..r * aw + aw]);
        }
        for r in 0..ah {
            dump.extend_from_slice(&rv[r * aw..r * aw + aw]);
        }
        // Store luma (true dims) then chroma planes at ALIGNED stride*rows —
        // the harness crops what it needs.
        let mut out = Vec::with_capacity(w * h + 2 * ah * aw);
        out.extend_from_slice(&dump[..w * h]);
        out.extend_from_slice(&dump[w * h..]);
        std::fs::write(format!("{outdir}/recon.raw"), &out).unwrap();
        println!(
            "recon: luma {}x{} + chroma 2x{}x{} (aligned stride)",
            w, h, aw, ah
        );
    }

    println!(
        "{}x{} qp {} (qindex {}) -> {} bytes",
        w,
        h,
        qp,
        svtav1_encoder::rate_control::qp_to_qindex(qp),
        obu.len()
    );
}
