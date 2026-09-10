//! Centre-crop a PNG to `WxH` and write it back as 8-bit RGB.
//!
//! WHY THIS EXISTS. `tools/imazen26_gate.sh` asserts byte-identity on 20
//! imazen-26 images, and it feeds each one to `identity_run` as
//! `crop:<png>` at 512x512 -- so only a 512x512 centre window of each image is
//! ever encoded. The originals total 289 MiB (one is 9146x5272, another
//! 5504x8256), which is far too much to pull on every CI run for 40 cells that
//! together look at 5 MP.
//!
//! Cropping AHEAD of time is only sound if the crop is bit-exact, so this tool
//! does not reimplement the decode: it uses the SAME `EXPAND | STRIP_16`
//! transformations and the same RGB conversion and centre arithmetic that
//! `identity_run`'s `crop:` branch uses, so `crop:` applied to the output is
//! the identity map on those pixels. The equivalence is not argued from this
//! comment -- `tools/imazen26_gate.sh` was run against the originals and
//! against the crops and produced the same bytes for all 40 cells.
//!
//! Usage: `crop_png <in.png> <out.png> [W] [H]`  (default 512x512)

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: {} <in.png> <out.png> [W] [H]", args[0]);
        std::process::exit(2);
    }
    let (src, dst) = (&args[1], &args[2]);
    let w: usize = args.get(3).and_then(|v| v.parse().ok()).unwrap_or(512);
    let h: usize = args.get(4).and_then(|v| v.parse().ok()).unwrap_or(512);

    let (rgb, pw, ph) = decode_png_rgb(src);
    // The SAME arithmetic as identity_run's `crop:` branch. A source smaller
    // than the window in either axis is clamped there and edge-replicated by
    // the encoder, so cropping it here would move the replication to a
    // different stage; refuse instead of quietly changing what gets encoded.
    assert!(
        pw >= w && ph >= h,
        "{src} is {pw}x{ph}, smaller than the {w}x{h} window; `crop:` would \
         edge-replicate it, which this tool must not pre-bake"
    );
    let (ox, oy) = ((pw - w) / 2, (ph - h) / 2);
    let mut out = vec![0u8; w * h * 3];
    for r in 0..h {
        for c in 0..w {
            let si = ((oy + r) * pw + (ox + c)) * 3;
            let di = (r * w + c) * 3;
            out[di..di + 3].copy_from_slice(&rgb[si..si + 3]);
        }
    }

    let file = std::fs::File::create(dst).unwrap_or_else(|e| panic!("create {dst}: {e}"));
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .expect("png header")
        .write_image_data(&out)
        .expect("png data");
    println!("{src} {pw}x{ph} -> {dst} {w}x{h} (origin {ox},{oy})");
}

/// Byte-for-byte the same decode `identity_run::decode_png_rgb` performs.
fn decode_png_rgb(path: &str) -> (Vec<u8>, usize, usize) {
    let file = std::fs::File::open(path).unwrap_or_else(|e| panic!("open {path}: {e}"));
    let mut dec = png::Decoder::new(std::io::BufReader::new(file));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().expect("png read_info");
    let mut buf = vec![0u8; reader.output_buffer_size().expect("png output_buffer_size")];
    let info = reader.next_frame(&mut buf).expect("png next_frame");
    let (w, h) = (info.width as usize, info.height as usize);
    let buf = &buf[..info.buffer_size()];
    let rgb = match info.color_type {
        png::ColorType::Rgb => buf.to_vec(),
        png::ColorType::Rgba => {
            let mut o = Vec::with_capacity(w * h * 3);
            for px in buf.as_chunks::<4>().0 {
                o.extend_from_slice(&px[..3]);
            }
            o
        }
        png::ColorType::Grayscale => {
            let mut o = Vec::with_capacity(w * h * 3);
            for &g in buf {
                o.extend_from_slice(&[g, g, g]);
            }
            o
        }
        png::ColorType::GrayscaleAlpha => {
            let mut o = Vec::with_capacity(w * h * 3);
            for px in buf.as_chunks::<2>().0 {
                o.extend_from_slice(&[px[0], px[0], px[0]]);
            }
            o
        }
        other => panic!("{path}: unexpected colour type {other:?} after EXPAND"),
    };
    (rgb, w, h)
}
