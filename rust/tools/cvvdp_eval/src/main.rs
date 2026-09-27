//! Perceptual-RD triage for a byte-diverging encoder cell — the direct
//! `cvvdp::VideoScorer` path (no PNG roundtrip, no per-frame subprocess).
//!
//! What it measures: feeds each side's DECODED stream (aomdec) and the
//! uncompressed source through cvvdp's temporal video path — clip-level
//! JOD per side plus a per-frame stills table. A diverging cell whose
//! deltas sit within metric noise is benign mode-decision divergence;
//! a materially negative delta is a real RD regression worth fixing.
//!
//! Usage:
//!   cvvdp_eval <src.i420> <WxH> <c_stream> <port_stream> [options]
//!
//! `<c_stream>` / `<port_stream>`: `.y4m` (decoded) or `.obu`/`.ivf`
//! (decoded on the fly via `aomdec` on PATH — the same decoder
//! `tools/video_selfcheck_gate.sh` already requires).
//!
//! Options:
//!   --display <name>   cvvdp display preset (default `standard_fhd`)
//!   --fps <f32>        temporal-filter frame rate (default 30)
//!   --low-memory       u8 ring window — bit-identical scores, ~4x less
//!                      scorer RSS at real resolutions
//!   --no-stills        skip the per-frame stills table
//!
//! Honest limits (same as tools/cvvdp_equiv.sh): cvvdp is a still metric
//! extended temporally; absolute JODs are display-model-dependent — only
//! the port-minus-C delta is meaningful. At 8-frame census clips the
//! 9-tap filter (fps=30) is barely exercised; treat the headline as a
//! triage number, not a parity proof.

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use cvvdp::display::DisplayPreset;
use cvvdp::{Cvvdp, CvvdpParams, VideoScorer, VideoScorerOptions};
use std::str::FromStr;

struct Args {
    src: PathBuf,
    w: usize,
    h: usize,
    c_stream: PathBuf,
    rs_stream: PathBuf,
    display: String,
    fps: f32,
    low_memory: bool,
    stills: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut pos = Vec::new();
    let mut display = "standard_fhd".to_string();
    let mut fps = 30.0f32;
    let mut low_memory = false;
    let mut stills = true;
    let mut it = env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--display" => {
                display = it.next().ok_or("--display needs a value")?;
            }
            "--fps" => {
                fps = it
                    .next()
                    .ok_or("--fps needs a value")?
                    .parse()
                    .map_err(|_| "--fps must be a number")?;
            }
            "--low-memory" => low_memory = true,
            "--no-stills" => stills = false,
            "-h" | "--help" => return Err(String::new()),
            _ if a.starts_with("--") => return Err(format!("unknown flag {a}")),
            _ => pos.push(a),
        }
    }
    if pos.len() != 4 {
        return Err(String::new());
    }
    let (w, h) = pos[1]
        .split_once('x')
        .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
        .ok_or_else(|| format!("size must be WxH (got {})", pos[1]))?;
    Ok(Args {
        src: PathBuf::from(&pos[0]),
        w,
        h,
        c_stream: PathBuf::from(&pos[2]),
        rs_stream: PathBuf::from(&pos[3]),
        display,
        fps,
        low_memory,
        stills,
    })
}

/// BT.601 limited-range 4:2:0 -> sRGB-8, nearest chroma upsample —
/// the identical integer math `tools/i444_png.py` uses, so numbers
/// stay comparable with the earlier PNG-dir measurements.
fn i420_frame_to_srgb(f: &[u8], w: usize, h: usize) -> Vec<u8> {
    let cw = w / 2;
    let (y, uv) = f.split_at(w * h);
    let (u, v) = uv.split_at(cw * (h / 2));
    let mut rgb = Vec::with_capacity(w * h * 3);
    let clip8 = |x: i32| x.clamp(0, 255) as u8;
    for j in 0..h {
        let ur = &u[(j / 2) * cw..(j / 2) * cw + cw];
        let vr = &v[(j / 2) * cw..(j / 2) * cw + cw];
        let yr = &y[j * w..j * w + w];
        for i in 0..w {
            let yy = yr[i] as i32 - 16;
            let uu = ur[i / 2] as i32 - 128;
            let vv = vr[i / 2] as i32 - 128;
            rgb.push(clip8((298 * yy + 409 * vv + 128) >> 8));
            rgb.push(clip8((298 * yy - 100 * uu - 208 * vv + 128) >> 8));
            rgb.push(clip8((298 * yy + 516 * uu + 128) >> 8));
        }
    }
    rgb
}

fn read_i420(path: &Path, w: usize, h: usize) -> Result<Vec<Vec<u8>>, String> {
    let data = fs::read(path).map_err(|e| format!("{path:?}: {e}"))?;
    let fsz = w * h * 3 / 2;
    if data.len() % fsz != 0 {
        return Err(format!(
            "{path:?}: {} bytes not a multiple of {fsz}",
            data.len()
        ));
    }
    Ok(data.chunks_exact(fsz).map(|c| c.to_vec()).collect())
}

/// y4m reader — header `YUV4MPEG2 W.. H.. .. C420*`, frames under
/// `FRAME` markers (params after the marker ignored). Accepts any
/// `C420*` chroma tag; rejects other subsamplings — the census cells
/// are all 4:2:0.
fn read_y4m(path: &Path, w: usize, h: usize) -> Result<Vec<Vec<u8>>, String> {
    let f = fs::File::open(path).map_err(|e| format!("{path:?}: {e}"))?;
    let mut r = BufReader::new(f);
    let mut header = String::new();
    r.read_line(&mut header)
        .map_err(|e| format!("{path:?}: {e}"))?;
    if !header.starts_with("YUV4MPEG2") {
        return Err(format!("{path:?}: not y4m"));
    }
    let fsz = w * h * 3 / 2;
    let mut out = Vec::new();
    loop {
        let mut marker = String::new();
        match r.read_line(&mut marker) {
            Ok(0) => break,
            Ok(_) => {
                if !marker.starts_with("FRAME") {
                    return Err(format!("{path:?}: expected FRAME marker, got {marker:?}"));
                }
            }
            Err(e) => return Err(format!("{path:?}: {e}")),
        }
        let mut fr = vec![0u8; fsz];
        r.read_exact(&mut fr)
            .map_err(|e| format!("{path:?}: {e}"))?;
        out.push(fr);
    }
    Ok(out)
}

/// Decode an `.obu`/`.ivf` AV1 stream to i420 frames via `aomdec`.
fn decode_stream(path: &Path, w: usize, h: usize) -> Result<Vec<Vec<u8>>, String> {
    let ext = path.extension().and_then(OsStr::to_str).unwrap_or("");
    if ext == "y4m" {
        return read_y4m(path, w, h);
    }
    let tmp = env::temp_dir().join(format!(
        "cvvdp-eval-{}-{}.y4m",
        std::process::id(),
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    let st = Command::new(env::var("AOMDEC").unwrap_or_else(|_| "aomdec".into()))
        .arg("-o")
        .arg(&tmp)
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("aomdec spawn: {e}"))?;
    if !st.success() {
        return Err(format!("aomdec failed on {path:?}"));
    }
    let frames = read_y4m(&tmp, w, h);
    let _ = fs::remove_file(&tmp);
    frames
}

fn main() -> Result<(), String> {
    let a = match parse_args() {
        Ok(a) => a,
        Err(msg) => {
            eprintln!(
                "usage: cvvdp_eval <src.i420> <WxH> <c_stream> <port_stream> [--display NAME] [--fps N] [--low-memory] [--no-stills]"
            );
            if !msg.is_empty() {
                eprintln!("error: {msg}");
            }
            std::process::exit(2);
        }
    };
    let preset = DisplayPreset::from_str(&a.display)
        .map_err(|e| format!("unknown display {:?}: {e}", a.display))?;
    let params = CvvdpParams {
        display: preset.model(),
        ..CvvdpParams::default()
    };
    let geometry = preset.geometry();

    let src_i420 = read_i420(&a.src, a.w, a.h)?;
    let c_i420 = decode_stream(&a.c_stream, a.w, a.h)?;
    let rs_i420 = decode_stream(&a.rs_stream, a.w, a.h)?;
    let n = src_i420.len().min(c_i420.len()).min(rs_i420.len());
    if n == 0 {
        return Err("no frames to score".into());
    }
    let src_rgb: Vec<Vec<u8>> = src_i420[..n]
        .iter()
        .map(|f| i420_frame_to_srgb(f, a.w, a.h))
        .collect();
    let c_rgb: Vec<Vec<u8>> = c_i420[..n]
        .iter()
        .map(|f| i420_frame_to_srgb(f, a.w, a.h))
        .collect();
    let rs_rgb: Vec<Vec<u8>> = rs_i420[..n]
        .iter()
        .map(|f| i420_frame_to_srgb(f, a.w, a.h))
        .collect();

    // Clip-level cvvdp-video JOD — the real temporal path.
    let mut jod = [0.0f32; 2];
    for (slot, frames) in [&c_rgb, &rs_rgb].iter().enumerate() {
        let mut v = VideoScorer::with_options(
            a.w as u32,
            a.h as u32,
            a.fps,
            params,
            geometry,
            VideoScorerOptions {
                low_memory: a.low_memory,
                ..VideoScorerOptions::default()
            },
        )
        .map_err(|e| format!("VideoScorer: {e}"))?;
        for (r, d) in src_rgb.iter().zip(frames.iter()) {
            v.push_frame(r, d).map_err(|e| format!("push_frame: {e}"))?;
        }
        jod[slot] = v.finish().map_err(|e| format!("finish: {e}"))?;
    }
    println!(
        "cvvdp-video ({}, {}fps): JOD_C={:.6} JOD_port={:.6} dJOD={:+.4} (clip-level, temporal{})",
        a.display,
        a.fps,
        jod[0],
        jod[1],
        jod[1] - jod[0],
        if a.low_memory { ", low-mem ring" } else { "" }
    );
    // Whole-stream byte totals — the sweep tool's rate axis.
    let c_bytes = fs::metadata(&a.c_stream).map(|m| m.len()).unwrap_or(0);
    let rs_bytes = fs::metadata(&a.rs_stream).map(|m| m.len()).unwrap_or(0);
    println!("totals bytes: C={c_bytes} port={rs_bytes}");

    // Per-frame stills — the detail view behind the headline.
    if a.stills {
        println!("frame  JOD_C      JOD_port   dJOD");
        let mut scorer = Cvvdp::with_geometry(a.w as u32, a.h as u32, params, geometry)
            .map_err(|e| format!("Cvvdp: {e}"))?;
        let mut tot = 0.0f64;
        for f in 0..n {
            let jc = scorer
                .score(&src_rgb[f], &c_rgb[f])
                .map_err(|e| format!("score f{f}: {e}"))?;
            let jr = scorer
                .score(&src_rgb[f], &rs_rgb[f])
                .map_err(|e| format!("score f{f}: {e}"))?;
            tot += (jr - jc) as f64;
            println!("f{f}     {jc:>9.6} {jr:>10.6} {:>+8.4}", jr - jc);
        }
        println!("stills-mean dJOD={:+.4}", tot / n as f64);
    }
    Ok(())
}
