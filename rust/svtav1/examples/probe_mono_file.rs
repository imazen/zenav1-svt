//! Monochrome RD/quality probe: encode REAL PNG content luma-only at a CLI QP
//! and preset. Writes the luma source as I420 (Y + 0x80 chroma — aomenc
//! --monochrome consumes the identical file), the .obu stream, and the
//! BT.601-limited luma as a grayscale PNG for SSIM2 scoring.
//!
//! Usage: probe_mono_file <a.png> <width> <height> <cli_qp 0..63> <preset> <out_prefix>

use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::rate_control::{RcConfig, RcMode};

fn clip8(x: i32) -> u8 {
    x.clamp(0, 255) as u8
}

fn decode_png_rgb(path: &str) -> (Vec<u8>, usize, usize) {
    let file = std::fs::File::open(path).expect("open png");
    let mut dec = png::Decoder::new(std::io::BufReader::new(file));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().expect("png read_info");
    let mut buf = vec![0u8; reader.output_buffer_size().expect("png output_buffer_size")];
    let info = reader.next_frame(&mut buf).expect("png next_frame");
    let w = info.width as usize;
    let h = info.height as usize;
    let rgb = match info.color_type {
        png::ColorType::Rgb => buf[..info.buffer_size()].to_vec(),
        png::ColorType::Rgba => buf[..info.buffer_size()]
            .chunks_exact(4)
            .flat_map(|c| [c[0], c[1], c[2]])
            .collect(),
        png::ColorType::Grayscale => buf[..info.buffer_size()]
            .iter()
            .flat_map(|&g| [g, g, g])
            .collect(),
        png::ColorType::GrayscaleAlpha => buf[..info.buffer_size()]
            .chunks_exact(2)
            .flat_map(|c| [c[0], c[0], c[0]])
            .collect(),
        other => panic!("unsupported png color type {other:?}"),
    };
    assert_eq!(rgb.len(), w * h * 3, "rgb length mismatch");
    (rgb, w, h)
}

fn fit_rgb(rgb: &[u8], sw: usize, sh: usize, w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 3];
    for r in 0..h {
        let sr = (r + sh.saturating_sub(h) / 2).min(sh - 1);
        for c in 0..w {
            let sc = (c + sw.saturating_sub(w) / 2).min(sw - 1);
            let (di, si) = ((r * w + c) * 3, (sr * sw + sc) * 3);
            out[di..di + 3].copy_from_slice(&rgb[si..si + 3]);
        }
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 7 {
        eprintln!(
            "usage: {} <a.png> <width> <height> <cli_qp 0..63> <preset> <out_prefix>",
            args[0]
        );
        std::process::exit(2);
    }
    let (png_path, w, h, qp, preset, out) = (
        args[1].as_str(),
        args[2].parse::<usize>().expect("width"),
        args[3].parse::<usize>().expect("height"),
        args[4].parse::<u8>().expect("cli_qp"),
        args[5].parse::<u8>().expect("preset"),
        args[6].as_str(),
    );

    let (rgb, sw, sh) = decode_png_rgb(png_path);
    let rgb = fit_rgb(&rgb, sw, sh, w, h);

    let mut y = vec![0u8; w * h];
    for r in 0..h {
        for c in 0..w {
            let i = (r * w + c) * 3;
            let (rr, gg, bb) = (rgb[i] as i32, rgb[i + 1] as i32, rgb[i + 2] as i32);
            y[r * w + c] = clip8(((66 * rr + 129 * gg + 25 * bb + 128) >> 8) + 16);
        }
    }

    let rc = RcConfig {
        mode: RcMode::Cqp,
        qp,
        ..RcConfig::default()
    };
    // chroma_format stays None — the monochrome extension path.
    let mut pipeline = EncodePipeline::new(w as u32, h as u32, preset, rc, 0, 1);
    let obu = match pipeline.try_encode_frame(&y, w) {
        Ok(obu) => obu,
        Err(e) => {
            eprintln!("REFUSED: {e}");
            std::process::exit(2);
        }
    };

    // I420 layout (luma + flat chroma) so aomenc --monochrome reads the same
    // bytes; the luma plane is the only scored surface.
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut i420 = Vec::with_capacity(w * h + 2 * cw * ch);
    i420.extend_from_slice(&y);
    i420.resize(w * h + 2 * cw * ch, 0x80);
    std::fs::write(format!("{out}.i420"), &i420).unwrap();
    std::fs::write(format!("{out}.y"), &y).unwrap();
    std::fs::write(format!("{out}.obu"), &obu).unwrap();

    // Reference PNG: the luma the encoder saw, lifted to full-range gray.
    let mut gray = vec![0u8; w * h * 3];
    for (i, &yy) in y.iter().enumerate() {
        let g = clip8((298 * (yy as i32 - 16) + 128) >> 8);
        gray[3 * i] = g;
        gray[3 * i + 1] = g;
        gray[3 * i + 2] = g;
    }
    {
        let f = std::fs::File::create(format!("{out}_src.png")).unwrap();
        let mut enc = png::Encoder::new(std::io::BufWriter::new(f), w as u32, h as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header().unwrap();
        wr.write_image_data(&gray).unwrap();
    }

    println!(
        "{out}: {}x{} qp {} (qindex {}) -> {} bytes",
        w,
        h,
        qp,
        svtav1_encoder::rate_control::qp_to_qindex(qp),
        obu.len()
    );
}
