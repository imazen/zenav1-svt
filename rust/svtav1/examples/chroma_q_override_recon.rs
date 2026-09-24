//! Recon-parity + anti-vacuity witness for the `__expert` chroma delta-q
//! override (`EncodePipeline::chroma_q_override`, chroma_q.rs
//! `ChromaQOverride`).
//!
//! The override replaces the derived per-plane chroma deltas, so three things
//! must hold, and each has its own check here:
//!
//! 1. SIGNAL == APPLICATION. The encoder's reconstruction must equal a
//!    conforming decoder's output byte-for-byte on every cell (aomdec always;
//!    dav1d too when `DAV1D` is set). `u != v` exercises the SH
//!    `separate_uv_delta_q = 1` + FH four-delta form on a MAINLINE pipeline,
//!    which no C configuration produces.
//! 2. THE OVERRIDE IS LIVE. Positive deltas must raise the chroma error
//!    against the source, and a one-plane override must hurt that plane more
//!    than the other. A run where no override moved the chroma error proves
//!    nothing and exits 2.
//! 3. MISUSE IS REFUSED. A monochrome frame with an override, and an override
//!    that flips U/V separation after the key frame, must return errors;
//!    a same-separation change mid-sequence must encode and decode.
//!
//! Usage: `tools/chroma_q_override_gate.sh [outdir]`, or directly
//!   `cargo run --release -p zenav1-svt --features __expert \
//!        --example chroma_q_override_recon -- [outdir]`
//!
//! Env: `AOMDEC` (default `aomdec` on PATH), `DAV1D` (optional second decoder).

use svtav1_encoder::chroma_q::ChromaQOverride;
use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::rate_control::{RcConfig, RcMode};

fn gen_luma(content: &str, w: usize, h: usize) -> Vec<u8> {
    let mut y = vec![0u8; w * h];
    for r in 0..h {
        for c in 0..w {
            y[r * w + c] = match content {
                "mixed" => {
                    if (r / 32 + c / 32).is_multiple_of(2) {
                        (((r * 7) ^ (c * 13)) & 0xFF) as u8
                    } else {
                        (96 + ((r / 4 + c / 4) % 9) * 5) as u8
                    }
                }
                _ => ((r * 255) / h.max(1)) as u8 ^ ((c * 3) & 0x3F) as u8,
            };
        }
    }
    y
}

/// Chroma with smooth ramps AND hard edges, so both low- and high-frequency
/// chroma coefficients carry energy the override can destroy.
fn gen_chroma(w: usize, h: usize) -> (Vec<u8>, Vec<u8>) {
    let (cw, ch) = (w / 2, h / 2);
    let mut u = vec![0u8; cw * ch];
    let mut v = vec![0u8; cw * ch];
    for r in 0..ch {
        for c in 0..cw {
            let edge = if (c / 12 + r / 16).is_multiple_of(2) { 36 } else { 0 };
            u[r * cw + c] = (64 + (r * 96) / ch.max(1) + edge) as u8;
            v[r * cw + c] = (200 - (c * 110) / cw.max(1) - edge / 2) as u8;
        }
    }
    (u, v)
}

fn read_i420(path: &str, w: usize, h: usize) -> Option<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    let d = std::fs::read(path).ok()?;
    let ysz = w * h;
    let csz = (w / 2) * (h / 2);
    if d.len() < ysz + 2 * csz {
        return None;
    }
    Some((
        d[..ysz].to_vec(),
        d[ysz..ysz + csz].to_vec(),
        d[ysz + csz..ysz + 2 * csz].to_vec(),
    ))
}

fn mse(a: &[u8], b: &[u8]) -> f64 {
    let n = a.len().min(b.len()).max(1);
    a.iter()
        .zip(b)
        .map(|(&x, &y)| {
            let d = f64::from(x) - f64::from(y);
            d * d
        })
        .sum::<f64>()
        / n as f64
}

fn decode(bin: &str, args: &[&str]) -> bool {
    std::process::Command::new(bin)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

type Recon = (Vec<u8>, Vec<u8>, Vec<u8>);

/// Compare a decoder's I420 output with the encoder recon; `None` = equal.
fn recon_diff(tag: &str, dec: &Recon, enc: &Recon) -> Option<String> {
    let mut out = Vec::new();
    for (plane, d, e) in [("Y", &dec.0, &enc.0), ("U", &dec.1, &enc.1), ("V", &dec.2, &enc.2)] {
        if d.len() != e.len() {
            out.push(format!("{tag} {plane} len dec={} enc={}", d.len(), e.len()));
        } else if d != e {
            let n = d.iter().zip(e).filter(|(a, b)| a != b).count();
            out.push(format!("{tag} {plane} {n} px differ"));
        }
    }
    (!out.is_empty()).then(|| out.join("; "))
}

fn main() {
    let outdir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/chroma_q_override_recon".to_string());
    std::fs::create_dir_all(&outdir).unwrap();
    let aomdec = std::env::var("AOMDEC").unwrap_or_else(|_| "aomdec".to_string());
    let dav1d = std::env::var("DAV1D").ok().filter(|s| !s.is_empty());

    let overrides: [Option<(i8, i8)>; 7] = [
        None,
        Some((0, 0)),
        Some((32, 32)),
        Some((48, 0)),
        Some((0, 48)),
        Some((-32, -32)),
        Some((63, -64)),
    ];
    // hdr mode: 0 = mainline, 1 = mainline + tune IQ (derived deltas are
    // nonzero and get REPLACED), 2 = the fork (always-separate SH).
    let mut cells = Vec::new();
    for content in ["mixed", "ramp"] {
        for (w, h) in [(128usize, 128usize), (192, 128)] {
            for qp in [20u8, 40] {
                cells.push((content, w, h, qp, 0u8));
            }
        }
    }
    cells.push(("mixed", 128, 128, 40, 1));
    cells.push(("mixed", 128, 128, 40, 2));

    let (mut pass, mut fail) = (0usize, 0usize);
    let mut failures: Vec<String> = Vec::new();
    // Anti-vacuity tallies, summed over mode-0 cells.
    let mut base_mse = [0f64; 2];
    let mut both_mse = [0f64; 2];
    let mut u_only = [0f64; 2];
    let mut v_only = [0f64; 2];

    for &(content, w, h, qp, mode) in &cells {
        let y = gen_luma(content, w, h);
        let (u, v) = gen_chroma(w, h);
        for ov in overrides {
            let name = format!(
                "cq_{content}_{w}x{h}_q{qp}_m{mode}_{}",
                ov.map_or("none".to_string(), |(a, b)| format!("u{a}v{b}"))
            );
            let rc = RcConfig {
                mode: RcMode::Cqp,
                qp,
                ..Default::default()
            };
            let mut p = EncodePipeline::new(w as u32, h as u32, 6, rc, 0, 1)
                .with_chroma_420(true)
                .with_recon_output(true);
            match mode {
                1 => p.hdr.tune = svtav1_encoder::tune::TUNE_IQ,
                2 => p.hdr = svtav1_encoder::hdr_mode::HdrForkConfig::hdr_fork(),
                _ => {}
            }
            p.chroma_q_override = ov.map(|(a, b)| ChromaQOverride::new(a, b));
            let obu = match p.try_encode_frame_420(&y, &u, &v, w) {
                Ok(o) => o,
                Err(e) => {
                    fail += 1;
                    failures.push(format!("{name}: ENCODE REFUSED {e}"));
                    continue;
                }
            };
            let enc: Recon = p.last_recon.clone().expect("recon published");
            let obu_path = format!("{outdir}/{name}.obu");
            std::fs::write(&obu_path, &obu).unwrap();

            let mut diffs = Vec::new();
            let aom_yuv = format!("{outdir}/{name}.aom.yuv");
            if !decode(&aomdec, &["--rawvideo", "-o", &aom_yuv, &obu_path]) {
                diffs.push("aomdec DECODE FAILED".to_string());
            } else if let Some(dec) = read_i420(&aom_yuv, w, h) {
                diffs.extend(recon_diff("aomdec", &dec, &enc));
            } else {
                diffs.push("aomdec output short".to_string());
            }
            if let Some(dd) = &dav1d {
                let dd_yuv = format!("{outdir}/{name}.dav1d.yuv");
                if !decode(dd, &["-i", &obu_path, "-o", &dd_yuv]) {
                    diffs.push("dav1d DECODE FAILED".to_string());
                } else if let Some(dec) = read_i420(&dd_yuv, w, h) {
                    diffs.extend(recon_diff("dav1d", &dec, &enc));
                } else {
                    diffs.push("dav1d output short".to_string());
                }
            }

            let m = [mse(&enc.1, &u), mse(&enc.2, &v)];
            println!(
                "{name}: {} B  mse U {:.2} V {:.2}  Y {:.2}",
                obu.len(),
                m[0],
                m[1],
                mse(&enc.0, &y)
            );
            if mode == 0 {
                let acc = match ov {
                    None => Some(&mut base_mse),
                    Some((32, 32)) => Some(&mut both_mse),
                    Some((48, 0)) => Some(&mut u_only),
                    Some((0, 48)) => Some(&mut v_only),
                    _ => None,
                };
                if let Some(a) = acc {
                    a[0] += m[0];
                    a[1] += m[1];
                }
            }
            if diffs.is_empty() {
                pass += 1;
            } else {
                fail += 1;
                failures.push(format!("{name}: {}", diffs.join("; ")));
            }
        }
    }

    // Misuse checks.
    let rc = RcConfig {
        mode: RcMode::Cqp,
        qp: 30,
        ..Default::default()
    };
    let y = gen_luma("mixed", 128, 128);
    let (u, v) = gen_chroma(128, 128);
    let mut mono = EncodePipeline::new(128, 128, 6, rc.clone(), 0, 1);
    mono.chroma_q_override = Some(ChromaQOverride::new(20, 20));
    if mono.try_encode_frame(&y, 128).is_ok() {
        fail += 1;
        failures.push("mono + override was NOT refused".to_string());
    } else {
        pass += 1;
    }
    // Key frame then inter (intra_period 0 = key once, then inter).
    let mut flip = EncodePipeline::new(128, 128, 6, rc.clone(), 0, 0).with_chroma_420(true);
    flip.chroma_q_override = Some(ChromaQOverride::new(10, 10));
    let first = flip.try_encode_frame_420(&y, &u, &v, 128);
    flip.chroma_q_override = Some(ChromaQOverride::new(10, 0));
    let second = flip.try_encode_frame_420(&y, &u, &v, 128);
    if first.is_ok() && second.is_err() {
        pass += 1;
    } else {
        fail += 1;
        failures.push(format!(
            "separation flip after key frame: first ok={} second refused={} (want true/true)",
            first.is_ok(),
            second.is_err()
        ));
    }
    let mut same = EncodePipeline::new(128, 128, 6, rc, 0, 0).with_chroma_420(true);
    same.chroma_q_override = Some(ChromaQOverride::new(10, 10));
    let a = same.try_encode_frame_420(&y, &u, &v, 128);
    same.chroma_q_override = Some(ChromaQOverride::new(24, 24));
    let b = same.try_encode_frame_420(&y, &u, &v, 128);
    match (a, b) {
        (Ok(a), Ok(b)) => {
            let path = format!("{outdir}/cq_same_sep_2frame.obu");
            std::fs::write(&path, [a, b].concat()).unwrap();
            let yuv = format!("{outdir}/cq_same_sep_2frame.aom.yuv");
            if decode(&aomdec, &["--rawvideo", "-o", &yuv, &path]) {
                pass += 1;
            } else {
                fail += 1;
                failures.push("same-separation 2-frame stream: aomdec DECODE FAILED".to_string());
            }
        }
        (a, b) => {
            fail += 1;
            failures.push(format!(
                "same-separation change refused: first ok={} second ok={}",
                a.is_ok(),
                b.is_ok()
            ));
        }
    }

    println!(
        "anti-vacuity (sum over mode-0 cells, mse U/V): none {:.1}/{:.1}  \
         +32/+32 {:.1}/{:.1}  U+48 {:.1}/{:.1}  V+48 {:.1}/{:.1}",
        base_mse[0], base_mse[1], both_mse[0], both_mse[1], u_only[0], u_only[1], v_only[0], v_only[1]
    );
    let live = both_mse[0] > base_mse[0]
        && both_mse[1] > base_mse[1]
        && (u_only[0] - base_mse[0]) > (u_only[1] - base_mse[1])
        && (v_only[1] - base_mse[1]) > (v_only[0] - base_mse[0]);
    println!("chroma-q override recon parity: {pass} passed, {fail} failed");
    for f in &failures {
        println!("  {f}");
    }
    if !live {
        println!(
            "VACUOUS GATE: the override did not raise chroma error as directed \
             (either it is not wired or the content carries no chroma detail)"
        );
        std::process::exit(2);
    }
    if fail > 0 {
        std::process::exit(1);
    }
}
