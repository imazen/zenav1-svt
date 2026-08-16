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
//!            crop:<a.png>  — like file: but CENTER-CROP a large image down to
//!                            <width>x<height> (used by the wider-corpus sweep to
//!                            run the big clic2025 / gb82-sc screen corpora at a
//!                            bounded encode size).
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
//!
//! Env: SVTAV1_TILE_COLS_LOG2 (default 0) — the column analogue, matching
//! C's cfg.tile_columns / the C driver's SVT_TILE_COLUMNS (task #96).
//! Both are CLAMPED to what the frame geometry supports, exactly like C
//! (`svt_aom_set_tile_info`), so an over-request degrades rather than
//! diverging. `tools/tile_gate.sh` drives the pair as SVTAV1_TILES.
//!
//! Env: SVTAV1_SB (task #91) — pin the superblock size to 64 or 128.
//! UNSET (the default) derives it with C's own rule
//! (`sb128_geom::derive_super_block_size`, Globals/enc_handle.c:4071-4111).
//! There is deliberately NO matching flag on the C driver: there is no
//! `super_block_size` field in EbSvtAv1EncConfiguration, so C derives it
//! too — both encoders agree from (aligned area, preset) alone. SB128 is
//! reached at aligned luma area >= 165,120 AND preset <= 1 in allintra;
//! below that area C forces 64 regardless of preset, which is why every
//! pre-existing cell here is an SB64 encode. The override exists for the
//! anti-vacuity witness (force 64 on an SB128 cell -> must diverge).

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
/// coefficients.
///
/// ODD DIMENSIONS ARE SUPPORTED. They used to be rejected here
/// (`assert!(w % 2 == 0 && h % 2 == 0, "I420 needs even dims")`), and that
/// assert was a HARNESS limitation being mistaken for a real constraint: AV1
/// 4:2:0 with odd luma dims is well defined (CEILING chroma, `(w+1)/2`), the
/// port has supported it since the #95 odd-dims work, and the synthetic content
/// paths already exercised it. Only this 2x2 averaging loop could not do it,
/// because it read a full 2x2 RGB block unconditionally and would run off the
/// end of the last column/row.
///
/// The cost of that assert was NOT a missing convenience — it was a coverage
/// hole. No gate cell could encode an odd-height frame of REAL content, and
/// hiding behind it was a public-API panic (`unsupported partition shape
/// (Horz4, 3)`) on a partition shape that only real content picks, reachable at
/// 512x481 on the gb82-sc corpus.
///
/// An edge chroma sample now averages the 1, 2 or 4 source pixels that actually
/// exist. Which edge rule to use is arbitrary in the same way the coefficients
/// are (libyuv duplicates the last column instead) — what matters is that both
/// encoders read the identical .yuv, and they do. For EVEN dims every sample
/// still averages a full 2x2, so every existing cell is byte-neutral.
fn rgb_to_i420_bt601(rgb: &[u8], w: usize, h: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    assert!(w > 0 && h > 0, "empty frame");
    let mut y = vec![0u8; w * h];
    for r in 0..h {
        for c in 0..w {
            let i = (r * w + c) * 3;
            let (rr, gg, bb) = (rgb[i] as i32, rgb[i + 1] as i32, rgb[i + 2] as i32);
            y[r * w + c] = clip8(((66 * rr + 129 * gg + 25 * bb + 128) >> 8) + 16);
        }
    }
    // CEILING chroma, matching `encode_frame_420` and the pic-buffer convention.
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut u = vec![0u8; cw * ch];
    let mut v = vec![0u8; cw * ch];
    for cr in 0..ch {
        for cc in 0..cw {
            let mut sr = 0i32;
            let mut sg = 0i32;
            let mut sb = 0i32;
            let mut n = 0i32;
            for dr in 0..2 {
                for dc in 0..2 {
                    // Clamp to the pixels that exist: at an odd right/bottom
                    // edge the 2x2 group is half or a quarter present.
                    let (sy, sx) = (cr * 2 + dr, cc * 2 + dc);
                    if sy >= h || sx >= w {
                        continue;
                    }
                    let i = (sy * w + sx) * 3;
                    sr += rgb[i] as i32;
                    sg += rgb[i + 1] as i32;
                    sb += rgb[i + 2] as i32;
                    n += 1;
                }
            }
            debug_assert!(n > 0, "a chroma sample always covers >= 1 luma pixel");
            let half = n / 2;
            let (rr, gg, bb) = ((sr + half) / n, (sg + half) / n, (sb + half) / n);
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
    // I420 chroma dims: AV1 4:2:0 uses CEILING rounding for odd luma dims
    // ((w+1)/2), matching the port's `encode_frame_420` (which takes ceiling
    // chroma) and the pic-buffer/app convention. Task #95 goal 1: ODD true
    // dims (65x65, 65x64, 64x65) are now in scope for the synthetic content
    // paths (uniform/gradient/diag, flat u=v=128 chroma — the floor-vs-ceiling
    // choice is inert in flat chroma CONTENT; only the DLF chroma BOUND differs
    // at odd width, which the port replicates per the port-map). The file:/raw:
    // paths keep even dims (their 2x2 RGB averaging / floor .yuv layout needs
    // it). For EVEN dims ceiling == floor, so every existing cell is byte-
    // neutral. Chunk 1 (full-SB) + chunk 2 (partial-SB) already handled the
    // aligned/8-round + partial-SB edge coding; this only adds odd true dims.
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));

    let (y, u, v) = if let Some(path) = content.strip_prefix("raw:") {
        // Raw I420 8-bit YUV file (w*h luma + 2*(w/2)*(h/2) chroma), used to
        // drive the identity/decode-both harness with EXACT content — e.g. the
        // decode_conformance failure cases (replicated-border padded content)
        // that synthetic uniform/gradient don't reproduce.
        assert!(
            w.is_multiple_of(2) && h.is_multiple_of(2),
            "raw: I420 harness requires even dims (floor .yuv layout); got {w}x{h}"
        );
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
    } else if let Some(path) = content.strip_prefix("crop:") {
        // Real content CENTER-CROPPED to (w,h). Unlike `file:` (which pads a
        // small image UP to the box and rejects an image LARGER than the box),
        // `crop:` takes a (w,h) window from the centre of a large image — the
        // wider-corpus sweep uses it to run the LARGE clic2025 (~2.7 MP) and
        // gb82-sc screen (up to ~5.6 MP) corpora at a bounded 512x512 encode so
        // preset-0 (the primary, SB128-triggering config) is tractable while
        // still exercising that corpus's real content statistics. If the image
        // is smaller than the crop in either axis the crop is clamped to the
        // image and the remainder edge-replicated (same padding as `file:`), so
        // this is a strict superset of `file:`'s box handling. Both encoders
        // still consume the ONE shared .yuv, so the comparison stays exact.
        let (rgb, pw, ph) = decode_png_rgb(path);
        let cwp = w.min(pw);
        let chp = h.min(ph);
        let ox = (pw - cwp) / 2;
        let oy = (ph - chp) / 2;
        let mut cropped = vec![0u8; cwp * chp * 3];
        for r in 0..chp {
            for c in 0..cwp {
                let si = ((oy + r) * pw + (ox + c)) * 3;
                let di = (r * cwp + c) * 3;
                cropped[di..di + 3].copy_from_slice(&rgb[si..si + 3]);
            }
        }
        let rgb = pad_rgb_replicate(&cropped, cwp, chp, w, h);
        rgb_to_i420_bt601(&rgb, w, h)
    } else {
        let mut y = vec![0u8; w * h];
        for r in 0..h {
            for c in 0..w {
                y[r * w + c] = match content {
                    "uniform" => 128,
                    "gradient" => (((r * 255) / h) as u8) ^ (((c * 3) & 0x3f) as u8),
                    // Constant along the r-c (down-right) diagonal → strong
                    // directional correlation, exercises the angled intra modes
                    // (D45/D135/…) that `gradient` never selects. Used to verify
                    // the bd10 directional re-encode (dr_predict_hbd).
                    "diag" => (((r as i32 - c as i32).rem_euclid(64)) * 4) as u8,
                    // SCREEN content: few distinct luma values, hard edges, no
                    // gradient — the shape `svt_aom_is_screen_content_
                    // antialiasing_aware` (pic_analysis_process.c:1207)
                    // classifies as palette blocks, so `sc_class5` fires and the
                    // whole screen-content vertical (palette level, intrabc
                    // level, allow_screen_content_tools) turns ON in BOTH
                    // encoders.
                    //
                    // This exists because every other synthetic content here is
                    // photographic in character, so no gate cell could ever
                    // exercise palette — which is how a bd10 palette gap sat
                    // unmeasured while the real-corpus sweep showed preset 6
                    // bd10 at 380/515 with every failure on screen content.
                    // A gate that cannot reach the feature cannot guard it.
                    //
                    // Layout: a 4-value background grid (window-like panels)
                    // overlaid with 2-value horizontal runs (text-like), giving
                    // 8x8 blocks of 2..6 distinct values — inside palette's
                    // `colors <= 64` bound and well inside PALETTE_MAX_SIZE
                    // after k-means.
                    "screen" => {
                        let panel = ((r / 24) & 1) as u8 * 2 + ((c / 32) & 1) as u8;
                        let bg = [35u8, 110, 180, 235][panel as usize];
                        let text_row = (r % 24) >= 6 && (r % 24) < 12;
                        let glyph = (c / 3 + r / 24) % 5 != 0;
                        if text_row && glyph { 16 } else { bg }
                    }
                    // SCREEN content with EXACTLY REPEATED distant regions —
                    // what IntraBC exists to exploit. `screen` alone arms the
                    // detector (allow_intrabc = true at preset <= 4) but never
                    // makes an IBC candidate WIN the RD, so an IBC gap is
                    // invisible on it: measured, `screen` at bd10 is
                    // byte-identical to C with IBC gated out entirely.
                    //
                    // Here the left half carries a deterministic pseudo-random
                    // glyph field and the right half REPLAYS it verbatim at a
                    // fixed displacement, so a block copy is exact (distortion
                    // 0) while any intra prediction must code a real residual.
                    // The 64px offset also clears IBC's 256px wavefront-delay
                    // and already-coded constraints (`is_dv_valid`).
                    // The repeated region is deliberately HIGH-ENTROPY (a
                    // per-pixel hash, ~250 distinct values). Palette bails
                    // above 64 colours per block and loses the RD long before
                    // that, so it cannot win here and mask the IBC candidate —
                    // an earlier low-colour version of this content coded 68-80
                    // palette blocks and ZERO IBC blocks. Flat panel bands stay
                    // in the top rows so the screen-content detector still arms.
                    "screenrep" => {
                        if r < 32 {
                            [24u8, 96, 168, 240][((r / 8) & 1) * 2 + ((c / 32) & 1)]
                        } else {
                            let (sr, sc) = if c >= w / 2 { (r, c - w / 2) } else { (r, c) };
                            let hashed = (sc * 2654435761 + sr * 40503) % 65521;
                            ((hashed % 251) + 2) as u8
                        }
                    }
                    other => {
                        panic!(
                            "unknown content {other:?} \
                             (use uniform|gradient|diag|screen|screenrep|file:<png>|raw:<yuv>)"
                        )
                    }
                };
            }
        }
        let u = vec![128u8; cw * ch];
        let v = vec![128u8; cw * ch];
        (y, u, v)
    };

    // SVTAV1_BD: encoder bit depth (8 default, or 10). At bd10 the C driver
    // (capture_c_trace <..> 10) reads PACKED u16 LE, so write the input as u16
    // (sample << (bd-8)); the port pipeline is u8 end-to-end, so it encodes the
    // u8 planes directly (chunks 2-4 add the u16 MD path). This is VALID for
    // content whose coded symbols are bit-depth-independent — uniform/skip,
    // where the decoder's DC prediction fills the 10-bit default and the coded
    // tile bytes are identical to bd8 apart from the SH high_bitdepth bit.
    let bd: u8 = std::env::var("SVTAV1_BD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8);
    // SVTAV1_HBD_SRC (task #6 chunk 2): generate a REAL 10-bit source instead
    // of `u8 << 2`. The low 2 bits carry a deterministic spatial pattern, so
    // both encoders see identical NON-widened 10-bit samples and the gate
    // actually exercises the u16 path end-to-end. The .yuv written below is
    // the SAME bytes the C driver reads, so the oracle needs no changes —
    // `capture_c_trace` already consumes a 16-bit-LE .yuv at bd10.
    let hbd_src = bd > 8 && std::env::var_os("SVTAV1_HBD_SRC").is_some();
    let low_bits = |v: usize, r: usize, c: usize| -> u16 {
        // 0..3, varying in BOTH directions (a constant or row-only pattern
        // would be invisible to a horizontal-only predictor).
        (((r * 3 + c * 5 + v) % 4) as u16) & 3
    };
    let (y10, u10, v10): (Vec<u16>, Vec<u16>, Vec<u16>) = if hbd_src {
        let shift = (bd - 8) as u32;
        let mk = |p: &[u8], pw: usize| -> Vec<u16> {
            p.iter()
                .enumerate()
                .map(|(i, &s)| ((s as u16) << shift) | low_bits(s as usize, i / pw, i % pw))
                .collect()
        };
        (mk(&y, w), mk(&u, cw), mk(&v, cw))
    } else {
        (Vec::new(), Vec::new(), Vec::new())
    };
    if bd > 8 {
        let shift = (bd - 8) as u32;
        let mut yuv = Vec::with_capacity((w * h + 2 * cw * ch) * 2);
        if hbd_src {
            for &s in y10.iter().chain(u10.iter()).chain(v10.iter()) {
                yuv.extend_from_slice(&s.to_le_bytes());
            }
        } else {
            for &s in y.iter().chain(u.iter()).chain(v.iter()) {
                yuv.extend_from_slice(&(((s as u16) << shift).to_le_bytes()));
            }
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
    // task #96: tile columns, same log2 domain (C cfg.tile_columns /
    // capture_c_trace's SVT_TILE_COLUMNS).
    let tile_cols_log2: u8 = std::env::var("SVTAV1_TILE_COLS_LOG2")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    // SVTAV1_MONO: encode the luma alone via the monochrome path (diagnostic —
    // isolates whether a 4:2:0 divergence is chroma-specific or exists in the
    // shared luma coding). Off by default (the harness is 4:2:0).
    let mono = std::env::var_os("SVTAV1_MONO").is_some();
    // SVTAV1_SB (task #91): pin the superblock size to 64 or 128 instead of
    // deriving it. UNSET (the default, and every pre-existing invocation) =
    // derive with C's own rule (`sb128_geom::derive_super_block_size`,
    // Globals/enc_handle.c:4071-4111), which is what makes the port agree
    // with the oracle WITHOUT a matching C-side flag — there is no
    // superblock field in `EbSvtAv1EncConfiguration`, C derives it too.
    //
    // The override exists for the anti-vacuity witness: forcing 64 on a
    // cell C codes at 128 must DIVERGE, proving an sb128 gate is not just
    // re-proving the sb64 gate.
    let sb_size: Option<usize> = std::env::var("SVTAV1_SB").ok().and_then(|v| v.parse().ok());
    assert!(
        matches!(sb_size, None | Some(64) | Some(128)),
        "SVTAV1_SB must be 64 or 128, got {sb_size:?}"
    );
    // SVTAV1_SUPERRES=<denom 9..16> (superres chunk B.3): encode at the
    // reduced width `w * 8 / denom` and signal `superres_params()`; the C
    // driver reads the SAME denominator from SVT_SUPERRES_KF_DENOM. Unset =
    // no superres, i.e. every pre-existing cell is untouched.
    let superres_denom: Option<u8> = std::env::var("SVTAV1_SUPERRES")
        .ok()
        .and_then(|v| v.parse().ok());
    let mut pipeline = EncodePipeline::new(w as u32, h as u32, preset, rc, 0, 1)
        .with_tile_rows_log2(tile_rows_log2)
        .with_tile_cols_log2(tile_cols_log2)
        .with_bit_depth(bd)
        .with_sb_size(sb_size)
        // The recon dumps below (SVTAV1_RECON_BIN and friends) read
        // last_recon*, which is opt-in since the post-filter passes that
        // produce it are byte-inert at preset >= 7.
        .with_recon_output(true);
    // NOTE: the warning goes to STDOUT, never stderr — identity_diff.sh
    // captures this process's stderr verbatim into `rs.trace` (the symtrace
    // op stream the differ parses), so any stray stderr line corrupts every
    // comparison. Same reason the wrapper buffers cargo's build chatter.
    if let Some(d) = superres_denom {
        pipeline = pipeline.with_superres(d);
        // SVTAV1_SR_DUMP=<path>: write the DOWNSCALED I420 source the pipeline
        // will encode (same `resize_plane_horizontal` call, which is pinned
        // byte-exact vs C). Diagnostic only — lets a superres divergence be
        // split into "the coded-width encode of this content" (re-run it via
        // `raw:` with no superres) vs "a statistic C derives BEFORE scaling".
        if let Ok(path) = std::env::var("SVTAV1_SR_DUMP") {
            let cwid = pipeline.true_width as usize;
            let (ucw, uch) = (w.div_ceil(2), h.div_ceil(2));
            let ccw = cwid.div_ceil(2);
            let mut yd = vec![0u8; cwid * h];
            let mut ud = vec![0u8; ccw * uch];
            let mut vd = vec![0u8; ccw * uch];
            svtav1_dsp::resize::resize_plane_horizontal(&y, h, w, w, &mut yd, cwid, cwid);
            svtav1_dsp::resize::resize_plane_horizontal(&u, uch, ucw, ucw, &mut ud, ccw, ccw);
            svtav1_dsp::resize::resize_plane_horizontal(&v, uch, ucw, ucw, &mut vd, ccw, ccw);
            let mut out = Vec::with_capacity(yd.len() + ud.len() + vd.len());
            out.extend_from_slice(&yd);
            out.extend_from_slice(&ud);
            out.extend_from_slice(&vd);
            std::fs::write(&path, &out).expect("write SVTAV1_SR_DUMP");
            eprintln!("SVTAV1_SR_DUMP {cwid}x{h} -> {path}");
        }
    }
    let sb128_fallback = pipeline.sb128_fallback;
    let sb_size_used = pipeline.sb_size;
    // SVT_HDR_MODE / SVT_FORK_*: the SAME env names the C driver reads, so one
    // env vector configures both encoders (hdr_mode::HdrForkConfig::from_env).
    // Unset => mainline, i.e. every pre-existing invocation is unchanged.
    pipeline.hdr = svtav1_encoder::hdr_mode::HdrForkConfig::from_env();
    // SVTAV1_TUNE=<0..5>: mainline `--tune`. The C driver reads the same value
    // from SVT_TUNE, so one env vector configures both encoders. Tune 3 (IQ)
    // and 4 (MS_SSIM) pull in C's whole override block (qm, sharpness,
    // variance boost, and for IQ max_tx_size + screen content) via
    // `HdrForkConfig::apply_tune_overrides`, which the pipeline calls at
    // encode time. Unset => tune 1 (PSNR) => every pre-existing cell unchanged.
    if let Ok(t) = std::env::var("SVTAV1_TUNE")
        && let Ok(v) = t.parse::<u8>()
    {
        pipeline.hdr.tune = v;
    }
    // SVTAV1_Y_STRIDE=<n> (>= w): hand the LUMA plane to the encoder at a
    // stride WIDER than the frame, with the slack POISONED (0xA5 / 0x0A5A5).
    //
    // The project's pixel-buffer rule is that any multi-row function handles
    // `stride != width`, and the encoder's public entry points take a luma
    // stride — but no gate ever passed one that differed. The #15 intra-clamp
    // defect turned on exactly that confusion in the other direction
    // (`frame_h = y_recon.len() / y_stride` treated a buffer's shape as the
    // frame's extent), so "a padded stride never changes the bitstream" is
    // worth PROVING rather than assuming.
    //
    // Poison, not edge-replication: replicating would make a stray read of the
    // padding return a plausible value and hide the bug. The `.yuv` the C
    // oracle reads stays tightly packed, so a cell that byte-matches C at a
    // padded stride has proven both halves at once.
    let y_stride_env: Option<usize> = std::env::var("SVTAV1_Y_STRIDE")
        .ok()
        .and_then(|v| v.parse().ok());
    if let Some(s) = y_stride_env {
        assert!(
            s >= w,
            "SVTAV1_Y_STRIDE {s} is narrower than the frame ({w})"
        );
    }
    let y_stride = y_stride_env.unwrap_or(w);
    let restride = |p: &[u8], pw: usize, ph: usize| -> Vec<u8> {
        if y_stride == pw {
            return p.to_vec();
        }
        let mut o = vec![0xA5u8; y_stride * ph];
        for r in 0..ph {
            o[r * y_stride..r * y_stride + pw].copy_from_slice(&p[r * pw..r * pw + pw]);
        }
        o
    };
    let y_in = restride(&y, w, h);
    let y10_in: Vec<u16> = if hbd_src && y_stride != w {
        let mut o = vec![0xA5A5u16; y_stride * h];
        for r in 0..h {
            o[r * y_stride..r * y_stride + w].copy_from_slice(&y10[r * w..r * w + w]);
        }
        o
    } else {
        y10.clone()
    };
    let (y, y10) = (y_in, y10_in);

    let obu = if hbd_src {
        // Task #6: the native-10-bit entry points — the port sees the SAME
        // real u16 samples written to the .yuv the C oracle reads.
        if mono {
            pipeline
                .try_encode_frame_hbd(&y10, y_stride)
                .expect("hbd mono encode inside the documented envelope")
        } else {
            pipeline = pipeline.with_chroma_420(true);
            pipeline
                .try_encode_frame_420_hbd(&y10, &u10, &v10, y_stride)
                .expect("hbd 4:2:0 encode inside the documented envelope")
        }
    } else if mono {
        unwrap_or_refuse(pipeline.try_encode_frame(&y, y_stride))
    } else {
        pipeline = pipeline.with_chroma_420(true);
        unwrap_or_refuse(pipeline.try_encode_frame_420(&y, &u, &v, y_stride))
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
                eprintln!(
                    "SVTAV1_RECON_DUMP {name} -> {pfx}.{name}.bin ({} bytes)",
                    b.len()
                );
            }
        };
        dump("pre", &pipeline.last_recon_unfiltered);
        dump("post", &pipeline.last_recon_pre_cdef);
    }
    // SVTAV1_FINAL_RECON=<path>: the FINAL (deblock -> CDEF -> LR) encoder
    // reconstruction, CROPPED to the true coded dims and tightly packed as
    // I420 Y|U|V — byte-for-byte comparable with what `aomdec -o <f>.y4m`
    // writes for this same stream.
    //
    // This is the oracle-free half of the alignment gate. Byte-identity to C
    // cannot see an encoder/decoder prediction MISMATCH as a CORRECTNESS
    // problem: if the port and C were both wrong the same way it would stay
    // green. #15's intra-reference-clamp defect WAS such a mismatch (a
    // straddling block predicted from recon rows a conforming decoder
    // replicates), and it is the reason this dump exists.
    //
    // `last_recon` is at the ALIGNED stride; the crop to (true_w, true_h) is
    // what a decoder outputs. Chroma uses CEILING dims, matching the .yuv
    // layout and aomdec's y4m.
    if let Ok(path) = std::env::var("SVTAV1_FINAL_RECON") {
        let aw = pipeline.width as usize;
        let (tw2, th2) = (pipeline.true_width as usize, pipeline.true_height as usize);
        let (acw, tcw2, tch2) = (aw / 2, tw2.div_ceil(2), th2.div_ceil(2));
        let crop = |p: &[u8], stride: usize, cw: usize, chh: usize| -> Vec<u8> {
            let mut o = Vec::with_capacity(cw * chh);
            for r in 0..chh {
                o.extend_from_slice(&p[r * stride..r * stride + cw]);
            }
            o
        };
        let (ry, ru, rv) = pipeline
            .last_recon
            .as_ref()
            .expect("with_recon_output(true) is set above");
        let mut b = crop(ry, aw, tw2, th2);
        if !ru.is_empty() {
            b.extend_from_slice(&crop(ru, acw, tcw2, tch2));
            b.extend_from_slice(&crop(rv, acw, tcw2, tch2));
        }
        std::fs::write(&path, &b).expect("write SVTAV1_FINAL_RECON");
    }
    // bd10 diagnostic: dump the re-encode pass's true-10-bit LUMA recon (u16
    // LE) for the self-consistency check vs the decoder's prefilter output.
    if let Ok(path) = std::env::var("SVTAV1_BD10_RECON")
        && let Some(r10) = pipeline.last_recon10_y.as_ref()
    {
        let mut b = Vec::with_capacity(r10.len() * 2);
        for &v in r10 {
            b.extend_from_slice(&v.to_le_bytes());
        }
        std::fs::write(&path, &b).expect("write recon10");
        eprintln!("SVTAV1_BD10_RECON -> {path} ({} u16)", r10.len());
    }
    println!(
        "identity_run: {content} {w}x{h} qp={qp} preset={preset} sb={} -> {} bytes",
        sb_size_used,
        obu.len()
    );
    if sb128_fallback {
        // Loud, so a gate cell can never silently "pass" while the port
        // quietly coded a different superblock geometry than C did.
        println!(
            "identity_run: SB128-FALLBACK — C codes {w}x{h} preset {preset} with 128px \
             superblocks (sb128_geom::derive_super_block_size); this port emitted a valid \
             64px-SB stream that will NOT byte-match."
        );
    }
}

/// Turn a pipeline refusal into a DISTINCT exit status instead of a panic.
///
/// The encoder deliberately REFUSES configurations it cannot encode faithfully
/// (unsupported bit depth, qp 0 / lossless, an inter frame, an out-of-envelope
/// superres) rather than emit a plausible-but-wrong stream. Going through the
/// infallible `encode_frame*` wrappers turned every one of those into a panic
/// via their `.expect()`, which is indistinguishable from a real crash to a
/// harness — `tools/arbitrary_size_robustness.sh` reported 48 refusals as
/// PANIC. Exit code 3 lets a gate tell "correctly refused" from "crashed".
fn unwrap_or_refuse(r: svtav1_encoder::EncodeResult<Vec<u8>>) -> Vec<u8> {
    match r {
        Ok(v) => v,
        Err(e) => {
            eprintln!("identity_run: REFUSED by the encoder: {e}");
            std::process::exit(3);
        }
    }
}
