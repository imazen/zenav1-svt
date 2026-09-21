//! 4:4:4 RD/quality probe: encode REAL PNG content at a CLI QP and preset,
//! writing the I444 source .yuv (aomenc --i444 consumes the identical file),
//! the .obu stream, and the RGB that fed the encoder as a PPM for SSIM2
//! scoring against decoded output.
//!
//! Usage: probe_444_file <a.png> <width> <height> <cli_qp 0..63> <preset> <out_prefix>
//!
//!   <out_prefix>.yuv    — I444 planar source (y + u + v, full-res chroma)
//!   <out_prefix>.obu    — the encoded stream
//!   <out_prefix>_src.png — the (cropped/padded) RGB pixels as 8-bit RGB PNG
//!
//! RGB -> I444 is per-pixel BT.601 limited range — the same matrix
//! identity_run applies to luma, with chroma taken at EVERY pixel instead of
//! a 2x2 average (there is no subsampling at 4:4:4).

use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::rate_control::{RcConfig, RcMode};
use svtav1_types::chroma::ChromaFormat;

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

/// Center-crop to at most w x h, then edge-replicate pad up to w x h.
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

fn rgb_to_i444_bt601(rgb: &[u8], w: usize, h: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let mut y = vec![0u8; w * h];
    let mut u = vec![0u8; w * h];
    let mut v = vec![0u8; w * h];
    for r in 0..h {
        for c in 0..w {
            let i = (r * w + c) * 3;
            let (rr, gg, bb) = (rgb[i] as i32, rgb[i + 1] as i32, rgb[i + 2] as i32);
            let px = r * w + c;
            y[px] = clip8(((66 * rr + 129 * gg + 25 * bb + 128) >> 8) + 16);
            u[px] = clip8(((-38 * rr - 74 * gg + 112 * bb + 128) >> 8) + 128);
            v[px] = clip8(((112 * rr - 94 * gg - 18 * bb + 128) >> 8) + 128);
        }
    }
    (y, u, v)
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
    let (y, u, v) = rgb_to_i444_bt601(&rgb, w, h);

    let rc = RcConfig {
        mode: RcMode::Cqp,
        qp,
        ..RcConfig::default()
    };
    let mut pipeline = EncodePipeline::new(w as u32, h as u32, preset, rc, 0, 1)
        .with_chroma_format(Some(ChromaFormat::Yuv444));
    let obu = match pipeline.try_encode_frame_444(&y, &u, &v, w) {
        Ok(obu) => obu,
        Err(e) => {
            eprintln!("REFUSED: {e}");
            std::process::exit(2);
        }
    };

    let mut yuv = Vec::with_capacity(w * h * 3);
    yuv.extend_from_slice(&y);
    yuv.extend_from_slice(&u);
    yuv.extend_from_slice(&v);
    std::fs::write(format!("{out}.yuv"), &yuv).unwrap();
    std::fs::write(format!("{out}.obu"), &obu).unwrap();

    {
        let f = std::fs::File::create(format!("{out}_src.png")).unwrap();
        let mut enc = png::Encoder::new(std::io::BufWriter::new(f), w as u32, h as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header().unwrap();
        wr.write_image_data(&rgb).unwrap();
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
