// Copyright (c) Imazen LLC.
// Licensed under AGPL-3.0-or-later OR the Imazen commercial license.

//! Phase-B census harness (wave: benchmarks/zensim_hdr_target_wave_2026-08-27.md).
//! Drives `encode_to_target` over the FROZEN HDR instrument; the judge SHELLS
//! the fleet-proven `zenmetrics score --metric zensim --hdr` per trial
//! (drift-free by construction — no metric math re-implemented). The only
//! in-harness math is the BT.2020nc limited bd10 matrix pair: `to_yuv420_bd10`
//! is MIRRORED VERBATIM from zenmetrics `sweep/hdr.rs` (the corpus's own
//! conversion) and the inverse is round-trip-gated in-binary before any cell.
//!
//!   cargo run --release -p svtav1-target --example zensim_census -- \
//!     <refs.tsv> <refs_dir> <targets-csv> <k> <zenmetrics-bin> <out.tsv>
//! refs.tsv rows: scene\ttier\trendition   (the frozen instrument file)

use std::io::Write;

use svtav1_target::{TargetOptions, encode_to_target};

const KR: f64 = 0.2627;
const KB: f64 = 0.0593;
const KG: f64 = 1.0 - KR - KB;

/// MIRRORED VERBATIM from zenmetrics `sweep/hdr.rs::to_yuv420_bd10` — the
/// conversion the hdrgrid corpus encodes went through.
fn to_yuv420_bd10(rgb16: &[u16], w: usize, h: usize) -> (Vec<u16>, Vec<u16>, Vec<u16>) {
    let n = w * h;
    let mut ynorm = vec![0f64; n];
    let mut y = vec![0u16; n];
    for i in 0..n {
        let r = rgb16[3 * i] as f64 / 65535.0;
        let g = rgb16[3 * i + 1] as f64 / 65535.0;
        let b = rgb16[3 * i + 2] as f64 / 65535.0;
        let yv = KR * r + KG * g + KB * b;
        ynorm[i] = yv;
        y[i] = ((876.0 * yv + 64.0).round() as i64).clamp(0, 1023) as u16;
    }
    let (cw, chd) = (w.div_ceil(2), h.div_ceil(2));
    let mut u = vec![0u16; cw * chd];
    let mut v = vec![0u16; cw * chd];
    for cy in 0..chd {
        for cx in 0..cw {
            let (mut sb, mut sr, mut cnt) = (0f64, 0f64, 0f64);
            for dy in 0..2 {
                for dx in 0..2 {
                    let (py, px) = (cy * 2 + dy, cx * 2 + dx);
                    if py >= h || px >= w {
                        continue;
                    }
                    let i = py * w + px;
                    let r = rgb16[3 * i] as f64 / 65535.0;
                    let b = rgb16[3 * i + 2] as f64 / 65535.0;
                    sb += (b - ynorm[i]) / 1.8814;
                    sr += (r - ynorm[i]) / 1.4746;
                    cnt += 1.0;
                }
            }
            u[cy * cw + cx] = ((896.0 * (sb / cnt) + 512.0).round() as i64).clamp(0, 1023) as u16;
            v[cy * cw + cx] = ((896.0 * (sr / cnt) + 512.0).round() as i64).clamp(0, 1023) as u16;
        }
    }
    (y, u, v)
}

/// Inverse: bd10 YUV420 (aligned luma stride) -> 16-bit PQ RGB, nearest
/// chroma upsample. Round-trip-gated against the forward in `main`.
fn yuv420_bd10_to_rgb16(
    y: &[u16],
    u: &[u16],
    v: &[u16],
    w: usize,
    h: usize,
    y_stride: usize,
) -> Vec<u16> {
    let cw = w.div_ceil(2);
    let cstride = y_stride / 2;
    let mut rgb = vec![0u16; w * h * 3];
    for py in 0..h {
        for px in 0..w {
            let yv = (f64::from(y[py * y_stride + px]) - 64.0) / 876.0;
            let ci = (py / 2) * cstride + (px / 2).min(cw.saturating_sub(1));
            let cb = (f64::from(u[ci]) - 512.0) / 896.0;
            let cr = (f64::from(v[ci]) - 512.0) / 896.0;
            let b = yv + 1.8814 * cb;
            let r = yv + 1.4746 * cr;
            let g = (yv - KR * r - KB * b) / KG;
            let i = (py * w + px) * 3;
            rgb[i] = (r * 65535.0).round().clamp(0.0, 65535.0) as u16;
            rgb[i + 1] = (g * 65535.0).round().clamp(0.0, 65535.0) as u16;
            rgb[i + 2] = (b * 65535.0).round().clamp(0.0, 65535.0) as u16;
        }
    }
    rgb
}

fn load_png16(path: &str) -> (Vec<u16>, usize, usize) {
    let dec = png::Decoder::new(std::io::BufReader::new(
        std::fs::File::open(path).expect("open"),
    ));
    let mut reader = dec.read_info().expect("info");
    let mut buf = vec![0u8; reader.output_buffer_size().expect("size")];
    let info = reader.next_frame(&mut buf).expect("frame");
    assert_eq!(info.bit_depth, png::BitDepth::Sixteen, "{path}: not 16-bit");
    assert_eq!(info.color_type, png::ColorType::Rgb, "{path}: not RGB");
    let (w, h) = (info.width as usize, info.height as usize);
    let rgb: Vec<u16> = buf[..info.buffer_size()]
        .chunks_exact(2)
        .map(|c| u16::from_be_bytes([c[0], c[1]]))
        .collect();
    (rgb, w, h)
}

fn write_png16(path: &str, rgb: &[u16], w: usize, h: usize) {
    let mut buf = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut buf, w as u32, h as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Sixteen);
        let mut wr = enc.write_header().expect("header");
        let bytes: Vec<u8> = rgb.iter().flat_map(|v| v.to_be_bytes()).collect();
        wr.write_image_data(&bytes).expect("data");
    }
    // Splice a cICP chunk right after IHDR — MIRRORING the corpus refs'
    // payload exactly ([1,16,0,1]: BT.709 primaries, PQ transfer, matrix 0,
    // full-range) so the judge's PQ gate accepts the recon. The png crate's
    // chunk API is private; PNG chunk = len + type + data + crc32(type+data).
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in data {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
    let ihdr_end = 8 + 8 + 25 - 8 - 4 + 12; // signature + IHDR chunk (len 13): 8 + (8+13+4)
    let ihdr_end = 8 + 8 + 13 + 4;
    let mut chunk = Vec::new();
    chunk.extend_from_slice(&4u32.to_be_bytes());
    chunk.extend_from_slice(b"cICP");
    chunk.extend_from_slice(&[1, 16, 0, 1]);
    let crc = crc32(&chunk[4..]);
    chunk.extend_from_slice(&crc.to_be_bytes());
    let mut out = Vec::with_capacity(buf.len() + chunk.len());
    out.extend_from_slice(&buf[..ihdr_end]);
    out.extend_from_slice(&chunk);
    out.extend_from_slice(&buf[ihdr_end..]);
    std::fs::write(path, out).expect("write");
}

fn main() {
    // ── round-trip gate before any cell ─────────────────────────────────
    {
        let (w, h) = (64usize, 64usize);
        // In-gamut physical pattern (a wild per-channel pattern goes outside
        // YCbCr gamut; the inverse's clamp then legitimately shifts luma —
        // that is out-of-gamut input, not matrix drift).
        let mut rgb = vec![0u16; w * h * 3];
        for py in 0..h {
            for px in 0..w {
                let base = 8000.0 + 45000.0 * (px as f64 / w as f64);
                let tex = 4000.0 * ((px as f64 * 0.31).sin() * (py as f64 * 0.23).cos());
                let i = (py * w + px) * 3;
                rgb[i] = (base + tex).clamp(0.0, 65535.0) as u16;
                rgb[i + 1] = (base * 0.9 + tex * 0.6).clamp(0.0, 65535.0) as u16;
                rgb[i + 2] = (base * 1.05 - tex * 0.4).clamp(0.0, 65535.0) as u16;
            }
        }
        let (y, u, v) = to_yuv420_bd10(&rgb, w, h);
        let back = yuv420_bd10_to_rgb16(&y, &u, &v, w, h, w);
        let max_luma_err = rgb
            .chunks_exact(3)
            .zip(back.chunks_exact(3))
            .map(|(a, b)| {
                let la = KR * f64::from(a[0]) + KG * f64::from(a[1]) + KB * f64::from(a[2]);
                let lb = KR * f64::from(b[0]) + KG * f64::from(b[1]) + KB * f64::from(b[2]);
                (la - lb).abs()
            })
            .fold(0.0f64, f64::max);
        assert!(
            max_luma_err < 65535.0 / 876.0 * 1.5,
            "matrix round-trip luma drift {max_luma_err} exceeds quantization"
        );
        eprintln!("round-trip gate OK (max luma err {max_luma_err:.1}/65535)");
    }
    let a: Vec<String> = std::env::args().collect();
    let (refs_tsv, refs_dir, targets_csv, k, zm_bin, out) =
        (&a[1], &a[2], &a[3], &a[4], &a[5], &a[6]);
    // Optional 7th arg: seed table TSV (t\ttier\tqp0; tier `*` = any) — the
    // S1/S2 registered seed arms. Absent = blind midpoint (the censused control).
    let seed: std::collections::HashMap<(u32, String), u8> = a
        .get(7)
        .map(|path| {
            std::fs::read_to_string(path)
                .expect("seed tsv")
                .lines()
                .skip(1)
                .filter(|l| !l.trim().is_empty())
                .map(|l| {
                    let f: Vec<&str> = l.split('\t').collect();
                    (
                        (f[0].parse::<f64>().unwrap() as u32, f[1].to_string()),
                        f[2].parse::<u8>().unwrap(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let max_encodes: u8 = k.parse().expect("k");
    let targets: Vec<f64> = targets_csv.split(',').map(|t| t.parse().unwrap()).collect();
    let tmp = std::env::temp_dir().join(format!("svt_census_{}", std::process::id()));
    std::fs::create_dir_all(&tmp).expect("tmp");
    let mut tsv = std::fs::File::create(out).expect("out");
    writeln!(
        tsv,
        "scene\ttier\ttarget\tqp\tencodes_used\tachieved\tabs_err\tbytes\tencode_s"
    )
    .unwrap();
    for line in std::fs::read_to_string(refs_tsv).expect("refs").lines() {
        if line.starts_with('#') || line.starts_with("scene\t") {
            continue;
        }
        let mut f = line.split('\t');
        let (scene, tier, rendition) = (f.next().unwrap(), f.next().unwrap(), f.next().unwrap());
        let ref_path = format!("{refs_dir}/{rendition}");
        let (rgb, w, h) = load_png16(&ref_path);
        let (sy, su, sv) = to_yuv420_bd10(&rgb, w, h);
        for &t in &targets {
            let t0 = std::time::Instant::now();
            let recon_png = tmp.join(format!("recon_{scene}_{t:.0}.png"));
            let recon_png_s = recon_png.to_str().unwrap().to_string();
            let ref_path2 = ref_path.clone();
            let zm = zm_bin.clone();
            let judge = |outp: &svtav1_target::TrialOutput| -> Result<f64, String> {
                let rgbr = yuv420_bd10_to_rgb16(
                    &outp.recon10.0,
                    &outp.recon10.1,
                    &outp.recon10.2,
                    w,
                    h,
                    outp.aligned_w,
                );
                write_png16(&recon_png_s, &rgbr, w, h);
                let o = std::process::Command::new(&zm)
                    .args([
                        "score",
                        "--metric",
                        "zensim",
                        "--hdr",
                        "--reference",
                        &ref_path2,
                        "--distorted",
                        &recon_png_s,
                    ])
                    .output()
                    .map_err(|e| format!("spawn: {e}"))?;
                if !o.status.success() {
                    return Err(format!(
                        "judge rc={:?}: {}",
                        o.status.code(),
                        String::from_utf8_lossy(&o.stderr)
                            .chars()
                            .take(200)
                            .collect::<String>()
                    ));
                }
                let s = String::from_utf8_lossy(&o.stdout);
                s.split("zensim=")
                    .nth(1)
                    .and_then(|x| x.trim().parse::<f64>().ok())
                    .ok_or_else(|| format!("unparsable judge output: {s}"))
            };
            let (res, outp) = encode_to_target(
                &sy,
                &su,
                &sv,
                w,
                h,
                6,
                t,
                &TargetOptions {
                    tolerance: 0.0,
                    max_encodes,
                    qp_start: seed
                        .get(&(t as u32, tier.to_string()))
                        .or_else(|| seed.get(&(t as u32, "*".to_string())))
                        .copied(),
                    ..Default::default()
                },
                judge,
            )
            .unwrap_or_else(|e| panic!("{scene} t{t}: {e:?}"));
            let secs = t0.elapsed().as_secs_f64();
            writeln!(
                tsv,
                "{scene}\t{tier}\t{t:.0}\t{}\t{}\t{:.3}\t{:.3}\t{}\t{secs:.1}",
                res.qp,
                res.encodes_used,
                res.score,
                (res.score - t).abs(),
                outp.bytes.len(),
            )
            .unwrap();
            eprintln!(
                "{scene} t{t:.0}: qp={} achieved={:.2} ({secs:.0}s)",
                res.qp, res.score
            );
        }
    }
    println!("census -> {out}");
}
