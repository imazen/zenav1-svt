//! Bisect probe: frame 1 identical to frame 0 (all-skip inter), or random
//! (all-intra inter), selectable by arg 5 ("dup" | "rand").
use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::rate_control::{RcConfig, RcMode};
use svtav1_types::chroma::ChromaFormat;

fn main() {
    let mut args = std::env::args().skip(1);
    let outdir = args.next().unwrap_or_else(|| "target/p444d".to_string());
    let size = args.next().unwrap_or_else(|| "128".to_string());
    let (w, h) = match size.split_once('x') {
        Some((a, b)) => (a.parse().unwrap(), b.parse().unwrap()),
        None => (size.parse().unwrap(), size.parse().unwrap()),
    };
    let qp: u8 = args.next().and_then(|s| s.parse().ok()).unwrap_or(30);
    let preset: u8 = args.next().and_then(|s| s.parse().ok()).unwrap_or(6);
    let mode = args.next().unwrap_or_else(|| "dup".to_string());
    std::fs::create_dir_all(&outdir).expect("create outdir");
    let mk = |i: usize| {
        let mut y = vec![0u8; w * h];
        let mut u = vec![0u8; w * h];
        let mut v = vec![0u8; w * h];
        for r in 0..h {
            for c in 0..w {
                if mode == "rand" && i >= 1 {
                    y[r * w + c] = ((r * 31 + c * 17 + i) & 0xFF) as u8;
                    u[r * w + c] = ((r * 13 + c * 41) & 0xFF) as u8;
                    v[r * w + c] = ((r * 7 + c * 23 + i * 5) & 0xFF) as u8;
                } else if mode == "shift" && i >= 1 {
                    // Frame i = frame 0 shifted — gives ME nonzero integer
                    // MVs to find, and the residual blocks nonzero chroma
                    // content to code.
                    let sr = (r as i32 + 3 * i as i32).clamp(0, h as i32 - 1) as usize;
                    let sc = (c as i32 + 5 * i as i32).clamp(0, w as i32 - 1) as usize;
                    y[r * w + c] = ((sr * 255) / h.max(1)) as u8 ^ ((sc * 3) & 0x3F) as u8;
                    u[r * w + c] =
                        (((sr * 3) & 0x7F) + 64) as u8 ^ (((sc / 4) & 1) as u8 * 40);
                    v[r * w + c] = (((sc * 5) & 0x7F) + 64) as u8;
                } else {
                    y[r * w + c] = ((r * 255) / h.max(1)) as u8 ^ ((c * 3) & 0x3F) as u8;
                    u[r * w + c] = (((r * 3) & 0x7F) + 64) as u8 ^ (((c / 4) & 1) as u8 * 40);
                    v[r * w + c] = (((c * 5) & 0x7F) + 64) as u8;
                }
            }
        }
        (y, u, v)
    };
    let rc = RcConfig {
        mode: RcMode::Cqp,
        qp,
        ..RcConfig::default()
    };
    let mut pipeline = EncodePipeline::new(w as u32, h as u32, preset, rc, 0, 64)
        .with_chroma_format(Some(ChromaFormat::Yuv444))
        .with_recon_output(true);
    let mut stream = Vec::new();
    let nframes: usize = std::env::var("N444").ok().and_then(|s| s.parse().ok()).unwrap_or(2);
    let (aw, ah) = (w.div_ceil(8) * 8, h.div_ceil(8) * 8);
    for i in 0..nframes {
        let (y, u, v) = mk(i);
        match pipeline.try_encode_frame_444(&y, &u, &v, w) {
            Ok(obu) => {
                std::fs::write(format!("{outdir}/f{i}.obu"), &obu).unwrap();
                stream.extend_from_slice(&obu);
            }
            Err(e) => {
                eprintln!("REFUSED f{i}: {e}");
                std::process::exit(2);
            }
        }
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
    println!("{mode} {w}x{h} qp {qp} p{preset} -> {} bytes", stream.len());
}
