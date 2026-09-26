//! The cell's source pixels: synthetic patterns, PNG crops and raw I420/I444.
//!
//! The critical harness invariant is that the one `.yuv` written from these
//! planes is the exact byte stream the C driver encodes too, so the RGB->YUV
//! choice need not match any spec — only be fixed and deterministic.

use svtav1_types::chroma::ChromaFormat;

/// Frame 0's 8-bit planes, and the whole sequence for `rawseq:` content.
pub struct Source {
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
    /// `rawseq:` stashes the WHOLE sequence here; the multi-frame path then
    /// uses these real frames instead of warping frame 0.
    pub rawseq: Option<Vec<u8>>,
}

/// Generate or load `content` at `w`x`h`, chroma at `fmt`'s geometry
/// (4:2:0 ceiling for odd dims; 4:4:4 full-resolution).
pub fn load(content: &str, w: usize, h: usize, assert_nonflat: bool, fmt: ChromaFormat) -> Source {
    let (cw, ch) = (fmt.chroma_width(w), fmt.chroma_height(h));
    let mut rawseq: Option<Vec<u8>> = None;
    let (y, u, v) = if let Some(path) = content.strip_prefix("rawseq:") {
        // A REAL multi-frame I420 8-bit sequence: `SVTAV1_FRAMES` frames of
        // (w*h luma + 2*(w/2)*(h/2) chroma) concatenated, as produced by
        // `tools/mk_video_assets.py` from a public-domain y4m.
        //
        // WHY THIS IS NOT `raw:` + the warp. The multi-frame path below
        // synthesises later frames by translating frame 0 by a global integer
        // offset. Open-loop ME finds that offset exactly, so the residual SAD
        // floors to zero — measured `avg_me_sad=0` and `is_gm_on=0` across
        // {gradient,diag,screen} x {64,128,256,512} (INTER-ENCODE-PLAN.md).
        // Every inter cell in this repo is therefore encoding a motion field
        // that C's search never has to work for. Real frames have occlusion,
        // lighting change, non-rigid motion and noise, none of which an
        // integer translation of one frame can present.
        //
        // Both encoders still consume the ONE shared `.yuv` this writes, so
        // the differential stays exact.
        assert!(
            fmt != ChromaFormat::Yuv420 || (w.is_multiple_of(2) && h.is_multiple_of(2)),
            "rawseq: I420 harness requires even dims; got {w}x{h} (I444 is full-res)"
        );
        let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read rawseq {path}: {e}"));
        let frame_len = w * h + 2 * cw * ch;
        assert!(
            bytes.len().is_multiple_of(frame_len) && !bytes.is_empty(),
            "rawseq {path}: {} bytes is not a whole number of {w}x{h} frames ({frame_len} B each)",
            bytes.len()
        );
        let ysz = w * h;
        let csz = cw * ch;
        let planes = (
            bytes[..ysz].to_vec(),
            bytes[ysz..ysz + csz].to_vec(),
            bytes[ysz + csz..ysz + 2 * csz].to_vec(),
        );
        rawseq = Some(bytes);
        planes
    } else if let Some(path) = content.strip_prefix("raw:") {
        // Raw I420 8-bit YUV file (w*h luma + 2*(w/2)*(h/2) chroma), used to
        // drive the identity/decode-both harness with EXACT content — e.g. the
        // decode_conformance failure cases (replicated-border padded content)
        // that synthetic uniform/gradient don't reproduce.
        assert!(
            fmt != ChromaFormat::Yuv420 || (w.is_multiple_of(2) && h.is_multiple_of(2)),
            "raw: I420 harness requires even dims (floor .yuv layout); got {w}x{h} \
             (I444 is full-res)"
        );
        let bytes = std::fs::read(path).expect("read raw yuv");
        let ysz = w * h;
        let csz = cw * ch;
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
        rgb_to_yuv_bt601(&rgb, w, h, fmt)
    } else if let Some(rest) = content.strip_prefix("crop@") {
        // `crop@X,Y:path` -- an EXPLICIT crop origin instead of the centre.
        //
        // Why this exists (issue #23): `crop:` takes the CENTRE window, and for
        // gb82-sc/gmessages.png the centre 512x512 is a single flat colour
        // (0x0d141c, all 262144 px). A palette block needs >= 2 colours and
        // IntraBC cannot beat exact DC on a constant plane, so 26 cells across
        // the three screen-content gates were passing while structurally unable
        // to reach the paths those gates exist to guard. MEASURED at 512x512
        // q20 p0: centre 46 B / 382 tile ops, content window 5524 B / 60118 ops.
        //
        // Plain `crop:` is UNCHANGED, so no existing cell's bytes move.
        let (spec, path) = rest
            .split_once(':')
            .unwrap_or_else(|| panic!("crop@ needs X,Y:path, got {rest:?}"));
        let (sx, sy) = spec
            .split_once(',')
            .unwrap_or_else(|| panic!("crop@ needs X,Y, got {spec:?}"));
        let ox: usize = sx.parse().expect("crop@ X must be a number");
        let oy: usize = sy.parse().expect("crop@ Y must be a number");
        let (rgb, pw, ph) = decode_png_rgb(path);
        let cwp = w.min(pw);
        let chp = h.min(ph);
        assert!(
            ox + cwp <= pw && oy + chp <= ph,
            "crop@{ox},{oy} of {cwp}x{chp} does not fit in {pw}x{ph} ({path})"
        );
        let cropped = crop_rgb(&rgb, pw, ox, oy, cwp, chp);
        assert_non_flat(assert_nonflat, &cropped, path, ox, oy);
        let rgb = pad_rgb_replicate(&cropped, cwp, chp, w, h);
        rgb_to_yuv_bt601(&rgb, w, h, fmt)
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
        let cropped = crop_rgb(&rgb, pw, ox, oy, cwp, chp);
        let rgb = pad_rgb_replicate(&cropped, cwp, chp, w, h);
        rgb_to_yuv_bt601(&rgb, w, h, fmt)
    } else {
        let mut y = vec![0u8; w * h];
        for r in 0..h {
            for c in 0..w {
                y[r * w + c] = match content {
                    "uniform" => 128,
                    "grain" => {
                        let mut z = (r * w + c + 1) as u32;
                        z ^= z << 13;
                        z ^= z >> 17;
                        z ^= z << 5;
                        (80 + (r * 80 / h) + (z as usize % 25)) as u8
                    }
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
                    // screencopy repeats panels at 256px: unlike screenrep's
                    // high-entropy field it keeps lossless screen detection on.
                    // At 512x128 QP0 p4, C selects 320 IntraBC blocks.
                    "screen" | "screencopy" => {
                        let c = if content == "screencopy" { c % 256 } else { c };
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
                    // in the top rows, but do not guarantee IntraBC is enabled:
                    // C leaves it off at QP0 on 128x128/256x256. Use screencopy
                    // for the lossless block-copy regression premise.
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
                             (use uniform|gradient|diag|screen|screencopy|screenrep|file:<png>|raw:<yuv>)"
                        )
                    }
                };
            }
        }
        let u = vec![128u8; cw * ch];
        let v = vec![128u8; cw * ch];
        (y, u, v)
    };
    Source { y, u, v, rawseq }
}

/// RGB->YUV at the cell's chroma geometry: I420 via the 2x2-averaged
/// [`rgb_to_i420_bt601`]; I444 takes every pixel's own chroma (full
/// resolution, `SVT_CHROMA=444` — the .yuv Ghost Robot's EB_YUV444 path
/// reads). Any other format is refused: the harness has no writer for it.
fn rgb_to_yuv_bt601(
    rgb: &[u8],
    w: usize,
    h: usize,
    fmt: ChromaFormat,
) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    match fmt {
        ChromaFormat::Yuv420 => rgb_to_i420_bt601(rgb, w, h),
        ChromaFormat::Yuv444 => rgb_to_i444_bt601(rgb, w, h),
        other => panic!("no RGB->{other:?} converter (420/444 only)"),
    }
}

/// Per-pixel BT.601 limited-range RGB->I444: the same coefficients as
/// [`rgb_to_i420_bt601`] with no 2x2 averaging — the chroma planes are
/// full-resolution `w`x`h`.
fn rgb_to_i444_bt601(rgb: &[u8], w: usize, h: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    assert!(w > 0 && h > 0, "empty frame");
    let mut y = vec![0u8; w * h];
    let mut u = vec![0u8; w * h];
    let mut v = vec![0u8; w * h];
    for r in 0..h {
        for c in 0..w {
            let i = (r * w + c) * 3;
            let (rr, gg, bb) = (rgb[i] as i32, rgb[i + 1] as i32, rgb[i + 2] as i32);
            y[r * w + c] = clip8(((66 * rr + 129 * gg + 25 * bb + 128) >> 8) + 16);
            u[r * w + c] = clip8(((-38 * rr - 74 * gg + 112 * bb + 128) >> 8) + 128);
            v[r * w + c] = clip8(((112 * rr - 94 * gg - 18 * bb + 128) >> 8) + 128);
        }
    }
    (y, u, v)
}

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
        other => panic!("unsupported PNG color type {other:?} after EXPAND/STRIP_16"),
    };
    assert_eq!(rgb.len(), w * h * 3, "rgb length mismatch");
    (rgb, w, h)
}

/// Fail loudly when a crop meant to carry screen content is a flat colour.
///
/// Issue #23: a vacuous crop makes a gate PASS while exercising nothing. This is
/// caller-controlled, never silent -- it only fires under `SVTAV1_ASSERT_NONFLAT`,
/// which the three screen-content gates set, so synthetic uniform cells (which
/// are legitimately flat) are unaffected.
fn assert_non_flat(enabled: bool, rgb: &[u8], path: &str, ox: usize, oy: usize) {
    if !enabled {
        return;
    }
    let first = &rgb[0..3];
    if rgb.chunks_exact(3).all(|px| px == first) {
        panic!(
            "SVTAV1_ASSERT_NONFLAT: the crop at ({ox},{oy}) of {path} is a single \
             colour {first:02x?}. A screen-content gate cannot reach palette or \
             IntraBC on a constant plane, so this cell would pass while testing \
             nothing (issue #23). Re-aim the crop."
        );
    }
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

/// The `cwp`x`chp` window of a `pw`-wide RGB image at (`ox`, `oy`).
fn crop_rgb(rgb: &[u8], pw: usize, ox: usize, oy: usize, cwp: usize, chp: usize) -> Vec<u8> {
    let mut cropped = vec![0u8; cwp * chp * 3];
    for r in 0..chp {
        let si = ((oy + r) * pw + ox) * 3;
        cropped[r * cwp * 3..(r + 1) * cwp * 3].copy_from_slice(&rgb[si..si + cwp * 3]);
    }
    cropped
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
