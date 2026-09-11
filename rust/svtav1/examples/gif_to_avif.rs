//! Convert an animated GIF to an animated AVIF.
//!
//! ```text
//! cargo run --release --features avif-container --example gif_to_avif -- in.gif out.avif [quality]
//! ```
//!
//! The conversion itself is the last three statements of `main`. Everything
//! else is argument handling and a summary line.
//!
//! Decoding is `zengif` (pure Rust, `#![forbid(unsafe_code)]`, full disposal /
//! transparency / timing support); encoding and muxing are this crate.

use svtav1::{avif::AvifEncoder, rgba::RgbaFrame};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let (Some(input), Some(output)) = (args.next(), args.next()) else {
        eprintln!("usage: gif_to_avif <in.gif> <out.avif> [quality 1..100, default 70]");
        std::process::exit(2);
    };
    let quality: f32 = args.next().map_or(Ok(70.0), |q| q.parse())?;

    // ---- decode ----
    let bytes = std::fs::read(&input)?;
    let (meta, gif_frames, _stats) =
        zengif::decode_gif(&bytes, zengif::Limits::default(), &zengif::Unstoppable)?;

    // `zengif` hands back full-canvas RGBA per frame with disposal and
    // transparency already applied, and a delay in centiseconds.
    let rgba: Vec<(Vec<u8>, u32)> = gif_frames
        .iter()
        .map(|f| {
            let mut buf = Vec::with_capacity(f.pixels.len() * 4);
            for px in &f.pixels {
                buf.extend_from_slice(&[px.r, px.g, px.b, px.a]);
            }
            // A GIF delay of 0 means "as fast as possible"; browsers clamp it
            // to 100 ms, and copying that keeps playback speed recognisable
            // rather than producing a file that races.
            let ms = if f.delay == 0 {
                100
            } else {
                u32::from(f.delay) * 10
            };
            (buf, ms)
        })
        .collect();

    // ---- encode ----
    let frames: Vec<RgbaFrame<'_>> = rgba
        .iter()
        .map(|(rgba, ms)| RgbaFrame {
            rgba,
            duration_ms: *ms,
        })
        .collect();
    let avif = AvifEncoder::new()
        .with_quality(quality)
        .encode_rgba_animation(&frames, meta.width.into(), meta.height.into())
        // `EncodeError` is `#[non_exhaustive]` and does not implement
        // `std::error::Error`, so it is surfaced by hand rather than with `?`.
        .map_err(|e| format!("encode failed: {e:?}"))?;
    std::fs::write(&output, &avif)?;

    println!(
        "{input} -> {output}: {}x{}, {} frames, {} B -> {} B ({:.1}%)",
        meta.width,
        meta.height,
        frames.len(),
        bytes.len(),
        avif.len(),
        100.0 * avif.len() as f64 / bytes.len() as f64,
    );
    Ok(())
}
