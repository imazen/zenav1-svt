//! Rust half of the bitstream-identity harness (tools/identity_diff.sh).
//!
//! Generates deterministic 4:2:0 content, writes it as a raw I420 .yuv
//! (the exact bytes the C driver `tools/capture_c_trace` consumes — both
//! encoders see identical input), encodes it through `EncodePipeline` in
//! 420 still-picture CQP mode, and writes the raw OBU stream.
//!
//! Build with `--features symtrace` and redirect stderr to capture the
//! per-symbol arithmetic-coder trace in the same format the wrapped C
//! library emits (`W CDF ...` / `W BOOL ...` lines).
//!
//! Usage: identity_run <content> <width> <height> <cli_qp 0..63> <preset> <out_prefix>
//!   content: uniform       — y = 128 everywhere
//!            gradient      — y[r][c] = ((r*255/h) ^ ((c*3) & 0x3f)), spec'd by
//!                            the identity campaign brief (trace_one's gradient)
//!            file:<a.png>  — decode the PNG, edge-replicate to <width>x<height>
//!                            if smaller, convert to I420 with the fixed
//!                            deterministic BT.601 limited-range transform below
//!                            (real photographic content — CID22, imazen26).
//!   u = v = 128 for uniform/gradient; real chroma for file: content.
//!
//! Writes <out_prefix>.yuv and <out_prefix>.obu. The critical harness
//! invariant is that this ONE .yuv is the exact byte stream the C driver
//! encodes too, so the RGB->YUV choice need not match any spec — only be
//! fixed and deterministic (both encoders see identical YUV).
//!
//! Env: SVTAV1_TILE_ROWS_LOG2 (default 0) — TileRowsLog2 request, same
//! log2 units as C's cfg.tile_rows / the C driver's SVT_TILE_ROWS (task
//! #86). 0 = single tile row (unchanged default).

use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::rate_control::{RcConfig, RcMode};

fn clip8(x: i32) -> u8 {
    x.clamp(0, 255) as u8
}

/// Decode a PNG to tightly-packed 8-bit RGB (3 bytes/pixel), returning
/// (rgb, width, height). Palette/16-bit/low-bit-gray inputs are normalised
/// via EXPAND + STRIP_16; grayscale and alpha variants are folded to RGB.
fn decode_png_rgb(path: &str) -> (Vec<u8>, usize, usize) {
    let file = std::fs::File::open(path).unwrap_or_else(|e| panic!("open {path}: {e}"));
    let mut dec = png::Decoder::new(std::io::BufReader::new(file));
    // EXPAND: palette -> RGB(A), sub-8-bit grayscale -> 8-bit. STRIP_16:
    // 16-bit -> 8-bit. After both, the output is always 8-bit in one of
    // {Grayscale, GrayscaleAlpha, Rgb, Rgba}.
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
            for px in buf.chunks_exact(4) {
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
            for px in buf.chunks_exact(2) {
                o.extend_from_slice(&[px[0], px[0], px[0]]);
            }
            o
        }
        other => panic!("unsupported PNG color type {other:?} after EXPAND/STRIP_16"),
    };
    assert_eq!(rgb.len(), w * h * 3, "rgb length mismatch");
    (rgb, w, h)
}

/// Edge-replicate an RGB buffer from (pw,ph) up to (w,h) — the same
/// bottom/right pixel-extend padding decode_conformance / AvifEncoder use to
/// reach 64-aligned encode dims. No-op when the image already fills (w,h).
fn pad_rgb_replicate(rgb: &[u8], pw: usize, ph: usize, w: usize, h: usize) -> Vec<u8> {
    if pw == w && ph == h {
        return rgb.to_vec();
    }
    let mut out = vec![0u8; w * h * 3];
    for r in 0..h {
        let sr = r.min(ph - 1);
        for c in 0..w {
            let sc = c.min(pw - 1);
            let si = (sr * pw + sc) * 3;
            let di = (r * w + c) * 3;
            out[di..di + 3].copy_from_slice(&rgb[si..si + 3]);
        }
    }
    out
}

/// Fixed, deterministic BT.601 limited-range ("studio swing") integer
/// RGB->I420. Y is per-pixel; chroma averages each 2x2 RGB block (libyuv's
/// ARGBToI420 shape) before converting, so U/V are (w/2)x(h/2). This choice
/// is arbitrary but FIXED: both encoders consume the identical .yuv this
/// writes, so the comparison stays apples-to-apples regardless of the exact
/// coefficients. (w,h) must be even.
fn rgb_to_i420_bt601(rgb: &[u8], w: usize, h: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    assert!(w % 2 == 0 && h % 2 == 0, "I420 needs even dims");
    let mut y = vec![0u8; w * h];
    for r in 0..h {
        for c in 0..w {
            let i = (r * w + c) * 3;
            let (rr, gg, bb) = (rgb[i] as i32, rgb[i + 1] as i32, rgb[i + 2] as i32);
            y[r * w + c] = clip8(((66 * rr + 129 * gg + 25 * bb + 128) >> 8) + 16);
        }
    }
    let (cw, ch) = (w / 2, h / 2);
    let mut u = vec![0u8; cw * ch];
    let mut v = vec![0u8; cw * ch];
    for cr in 0..ch {
        for cc in 0..cw {
            let mut sr = 0i32;
            let mut sg = 0i32;
            let mut sb = 0i32;
            for dr in 0..2 {
                for dc in 0..2 {
                    let i = ((cr * 2 + dr) * w + (cc * 2 + dc)) * 3;
                    sr += rgb[i] as i32;
                    sg += rgb[i + 1] as i32;
                    sb += rgb[i + 2] as i32;
                }
            }
            let (rr, gg, bb) = ((sr + 2) >> 2, (sg + 2) >> 2, (sb + 2) >> 2);
            u[cr * cw + cc] = clip8(((-38 * rr - 74 * gg + 112 * bb + 128) >> 8) + 128);
            v[cr * cw + cc] = clip8(((112 * rr - 94 * gg - 18 * bb + 128) >> 8) + 128);
        }
    }
    (y, u, v)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 7 {
        eprintln!(
            "usage: {} <content> <width> <height> <cli_qp 0..63> <preset> <out_prefix>",
            args[0]
        );
        std::process::exit(2);
    }
    let content = args[1].as_str();
    let w: usize = args[2].parse().expect("width");
    let h: usize = args[3].parse().expect("height");
    let qp: u8 = args[4].parse().expect("cli_qp");
    let preset: u8 = args[5].parse().expect("preset");
    let prefix = &args[6];
    // I420 needs even dims. Task #95 chunk 1: the pipeline pads TRUE ->
    // ALIGNED (8-round) internally and enforces the in-scope constraint
    // (aligned dims a multiple of 64 = full SBs); dims like 60x60 (-> 64)
    // exercise arbitrary-dimension input+header without partial-SB edge
    // coding. Partial SBs (e.g. 56x56, 200x200) are chunk 2.
    assert!(
        w % 2 == 0 && h % 2 == 0,
        "I420 harness requires even dims (got {w}x{h})"
    );

    let (y, u, v) = if let Some(path) = content.strip_prefix("raw:") {
        // Raw I420 8-bit YUV file (w*h luma + 2*(w/2)*(h/2) chroma), used to
        // drive the identity/decode-both harness with EXACT content — e.g. the
        // decode_conformance failure cases (replicated-border padded content)
        // that synthetic uniform/gradient don't reproduce.
        let bytes = std::fs::read(path).expect("read raw yuv");
        let ysz = w * h;
        let csz = (w / 2) * (h / 2);
        assert!(
            bytes.len() >= ysz + 2 * csz,
            "raw yuv {} too small: {} < {}",
            path,
            bytes.len(),
            ysz + 2 * csz
        );
        (
            bytes[..ysz].to_vec(),
            bytes[ysz..ysz + csz].to_vec(),
            bytes[ysz + csz..ysz + 2 * csz].to_vec(),
        )
    } else if let Some(path) = content.strip_prefix("file:") {
        // Real photographic content. The caller (real_image_matrix.sh) passes
        // (w,h) = the image dims rounded up to a multiple of 64; edge-replicate
        // into that box (a no-op for natively 64-aligned corpora like CID22-512).
        let (rgb, pw, ph) = decode_png_rgb(path);
        assert!(
            w >= pw && h >= ph,
            "requested {w}x{h} is smaller than image {pw}x{ph} — caller must round up to >= image"
        );
        let rgb = pad_rgb_replicate(&rgb, pw, ph, w, h);
        rgb_to_i420_bt601(&rgb, w, h)
    } else {
        let mut y = vec![0u8; w * h];
        for r in 0..h {
            for c in 0..w {
                y[r * w + c] = match content {
                    "uniform" => 128,
                    "gradient" => (((r * 255) / h) as u8) ^ (((c * 3) & 0x3f) as u8),
                    other => panic!("unknown content {other:?} (use uniform|gradient|file:<png>)"),
                };
            }
        }
        let u = vec![128u8; (w / 2) * (h / 2)];
        let v = vec![128u8; (w / 2) * (h / 2)];
        (y, u, v)
    };

    // SVTAV1_BD: encoder bit depth (8 default, or 10). At bd10 the C driver
    // (capture_c_trace <..> 10) reads PACKED u16 LE, so write the input as u16
    // (sample << (bd-8)); the port pipeline is u8 end-to-end, so it encodes the
    // u8 planes directly (chunks 2-4 add the u16 MD path). This is VALID for
    // content whose coded symbols are bit-depth-independent — uniform/skip,
    // where the decoder's DC prediction fills the 10-bit default and the coded
    // tile bytes are identical to bd8 apart from the SH high_bitdepth bit.
    let bd: u8 = std::env::var("SVTAV1_BD").ok().and_then(|v| v.parse().ok()).unwrap_or(8);
    if bd > 8 {
        let shift = (bd - 8) as u32;
        let mut yuv = Vec::with_capacity((w * h + 2 * (w / 2) * (h / 2)) * 2);
        for &s in y.iter().chain(u.iter()).chain(v.iter()) {
            yuv.extend_from_slice(&(((s as u16) << shift).to_le_bytes()));
        }
        std::fs::write(format!("{prefix}.yuv"), &yuv).expect("write .yuv");
    } else {
        let mut yuv = Vec::with_capacity(w * h * 3 / 2);
        yuv.extend_from_slice(&y);
        yuv.extend_from_slice(&u);
        yuv.extend_from_slice(&v);
        std::fs::write(format!("{prefix}.yuv"), &yuv).expect("write .yuv");
    }

    let rc = RcConfig {
        mode: RcMode::Cqp,
        qp, // CLI domain 0..63, same as the C driver's cfg.qp
        ..RcConfig::default()
    };
    // task #86: real tile rows. SVTAV1_TILE_ROWS_LOG2 (default 0) is the
    // log2 domain directly — same units as C's cfg.tile_rows
    // (EbSvtAv1Enc.h:607-611) and capture_c_trace's SVT_TILE_ROWS env var.
    let tile_rows_log2: u8 = std::env::var("SVTAV1_TILE_ROWS_LOG2")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    // SVTAV1_MONO: encode the luma alone via the monochrome path (diagnostic —
    // isolates whether a 4:2:0 divergence is chroma-specific or exists in the
    // shared luma coding). Off by default (the harness is 4:2:0).
    let mono = std::env::var_os("SVTAV1_MONO").is_some();
    let mut pipeline = EncodePipeline::new(w as u32, h as u32, preset, rc, 0, 1)
        .with_tile_rows_log2(tile_rows_log2)
        .with_bit_depth(bd);
    let obu = if mono {
        pipeline.encode_frame(&y, w)
    } else {
        pipeline = pipeline.with_chroma_420(true);
        pipeline.encode_frame_420(&y, &u, &v, w)
    };
    std::fs::write(format!("{prefix}.obu"), &obu).expect("write .obu");

    // SCRATCH: env-gated recon dump for the C-vs-Rust recon diff (tightly
    // packed Y|U|V, same layout as the instrumented C dlf_process dump).
    if let Ok(pfx) = std::env::var("SVTAV1_RECON_DUMP") {
        let dump = |name: &str, r: &Option<(Vec<u8>, Vec<u8>, Vec<u8>)>| {
            if let Some((yy, uu, vv)) = r {
                let mut b = Vec::new();
                b.extend_from_slice(yy);
                b.extend_from_slice(uu);
                b.extend_from_slice(vv);
                std::fs::write(format!("{pfx}.{name}.bin"), &b).expect("write recon dump");
                eprintln!("SVTAV1_RECON_DUMP {name} -> {pfx}.{name}.bin ({} bytes)", b.len());
            }
        };
        dump("pre", &pipeline.last_recon_unfiltered);
        dump("post", &pipeline.last_recon_pre_cdef);
    }
    // bd10 diagnostic: dump the re-encode pass's true-10-bit LUMA recon (u16
    // LE) for the self-consistency check vs the decoder's prefilter output.
    if let Ok(path) = std::env::var("SVTAV1_BD10_RECON") {
        if let Some(r10) = pipeline.last_recon10_y.as_ref() {
            let mut b = Vec::with_capacity(r10.len() * 2);
            for &v in r10 {
                b.extend_from_slice(&v.to_le_bytes());
            }
            std::fs::write(&path, &b).expect("write recon10");
            eprintln!("SVTAV1_BD10_RECON -> {path} ({} u16)", r10.len());
        }
    }
    println!(
        "identity_run: {content} {w}x{h} qp={qp} preset={preset} -> {} bytes",
        obu.len()
    );
}
