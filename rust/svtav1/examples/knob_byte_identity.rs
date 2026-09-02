//! Knob-to-bitstream identity probe (issue #17).
//!
//! Asks ONE question per (arm, cell): does changing a single `HdrForkConfig`
//! field change the encoded bytes? A knob that is plumbed to config but not
//! to the bitstream answers "no" on every cell, which is what
//! <https://github.com/imazen/zenav1-svt/issues/17> reported for `tune = 0`
//! and `screen_content_mode = Some(3)`.
//!
//! The pipeline is constructed the way the AVIF consumer constructs it
//! (`zenavif::encoder_svt_rs::encode_planes_svt`): `EncodePipeline::new(w, h,
//! preset, Cqp, 0, 1).with_chroma_420(true)`, `hdr` left at
//! `HdrForkConfig::mainline()`, then exactly one field overwritten. That
//! keeps the probe at the port boundary — no consumer, no container, no
//! metric.
//!
//! POSITIVE CONTROLS are the point. A null result is only informative if the
//! probe can detect a difference at all, so every run also encodes arms that
//! MUST move bytes (`tune = 3`, `sharpness = 7`) and arms that isolate the
//! suspected gate (`tune = 0` under `hdr_fork()`, where the per-tune LF
//! sharpness ladder is reachable; `screen_content_mode` at a preset above the
//! auto-detect ceiling).
//!
//! Usage: cargo run --release -p zenav1-svt --example knob_byte_identity -- <outdir>

use svtav1_encoder::hdr_mode::HdrForkConfig;
use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::rate_control::{RcConfig, RcMode};

const DIM: usize = 256;

/// FNV-1a 64. Identity comparison only — the files are written too, so
/// `sha256sum` is available on the same bytes.
fn fnv1a64(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &x in b {
        h ^= u64::from(x);
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h
}

/// Three content classes. `screen` is the one the scm-3 detector exists for:
/// few colours, hard aliased edges, text-like runs.
fn content(kind: &str) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let (w, h) = (DIM, DIM);
    let mut y = vec![0u8; w * h];
    let (cw, ch) = (w / 2, h / 2);
    let mut u = vec![128u8; cw * ch];
    let mut v = vec![128u8; cw * ch];
    match kind {
        "photo" => {
            for r in 0..h {
                for c in 0..w {
                    let g = (r * 180 / h + c * 60 / w) as i32;
                    let n = (((r * 7 + c * 13) % 17) as i32) - 8;
                    y[r * w + c] = (g + 40 + n).clamp(0, 255) as u8;
                }
            }
            for r in 0..ch {
                for c in 0..cw {
                    u[r * cw + c] = (110 + (c * 40 / cw)) as u8;
                    v[r * cw + c] = (150 - (r * 40 / ch)) as u8;
                }
            }
        }
        "screen" => {
            // Flat background, hard-edged glyph-like blocks, 1px rules.
            for r in 0..h {
                for c in 0..w {
                    let mut px = 235u8;
                    if r % 32 < 2 || c % 48 < 1 {
                        px = 30;
                    }
                    let (gr, gc) = (r % 32, c % 16);
                    if (8..24).contains(&gr)
                        && (3..11).contains(&gc)
                        && ((r / 32) + (c / 16)) % 3 != 0
                    {
                        px = 20;
                    }
                    y[r * w + c] = px;
                }
            }
        }
        _ => {
            // "detail": high-frequency, deblock-visible.
            for r in 0..h {
                for c in 0..w {
                    y[r * w + c] = (((r * 255) / h) ^ ((c * 3) & 0x3F)) as u8;
                }
            }
            for r in 0..ch {
                for c in 0..cw {
                    u[r * cw + c] = (((r * 3) & 0x7F) + 64) as u8;
                    v[r * cw + c] = (((c * 5) & 0x7F) + 64) as u8;
                }
            }
        }
    }
    (y, u, v)
}

/// One arm = a name plus the single mutation it applies to a mainline (or,
/// for the fork controls, a fork) config.
struct Arm {
    name: &'static str,
    fork: bool,
    apply: fn(&mut HdrForkConfig),
}

fn arms() -> Vec<Arm> {
    vec![
        Arm {
            name: "base",
            fork: false,
            apply: |_| {},
        },
        // The two arms under test.
        Arm {
            name: "tn0",
            fork: false,
            apply: |h| h.tune = 0,
        },
        Arm {
            name: "scm3",
            fork: false,
            apply: |h| h.screen_content_mode = Some(3),
        },
        // Positive controls: these MUST move bytes on a working probe.
        Arm {
            name: "CTRL_tn3",
            fork: false,
            apply: |h| h.tune = 3,
        },
        Arm {
            name: "CTRL_shp7",
            fork: false,
            apply: |h| h.sharpness = 7,
        },
        // Gate isolation: same tune delta, fork mode, where the per-tune LF
        // sharpness ladder (`tune::lf_sharpness_for_tune`) is reachable.
        Arm {
            name: "forkbase",
            fork: true,
            apply: |_| {},
        },
        Arm {
            name: "fork_tn0",
            fork: true,
            apply: |h| h.tune = 0,
        },
        // scm at a preset where Some(3)'s `preset.min(7)` is NOT the identity.
        Arm {
            name: "scm0",
            fork: false,
            apply: |h| h.screen_content_mode = Some(0),
        },
    ]
}

fn main() {
    let outdir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/knobid".into());
    std::fs::create_dir_all(&outdir).expect("create outdir");

    let presets: [u8; 3] = [4, 6, 8];
    let qps: [u8; 4] = [20, 32, 45, 55];
    let kinds = ["photo", "screen", "detail"];
    let arms = arms();

    let mut tsv = String::from("content\tpreset\tqp\tarm\tfork\tbytes\tfnv1a64\n");
    for kind in kinds {
        let (y, u, v) = content(kind);
        for &preset in &presets {
            for &qp in &qps {
                for a in &arms {
                    let rc = RcConfig {
                        mode: RcMode::Cqp,
                        qp,
                        ..RcConfig::default()
                    };
                    let mut p = EncodePipeline::new(DIM as u32, DIM as u32, preset, rc, 0, 1)
                        .with_chroma_420(true);
                    p.hdr = if a.fork {
                        HdrForkConfig::hdr_fork()
                    } else {
                        HdrForkConfig::mainline()
                    };
                    (a.apply)(&mut p.hdr);
                    let bs = p.encode_frame_420(&y, &u, &v, DIM);
                    let h = fnv1a64(&bs);
                    tsv.push_str(&format!(
                        "{kind}\t{preset}\t{qp}\t{}\t{}\t{}\t{:016x}\n",
                        a.name,
                        a.fork,
                        bs.len(),
                        h
                    ));
                    std::fs::write(
                        format!("{outdir}/{kind}_p{preset}_q{qp}_{}.obu", a.name),
                        &bs,
                    )
                    .unwrap();
                }
            }
        }
    }
    std::fs::write(format!("{outdir}/knob_byte_identity.tsv"), &tsv).unwrap();
    print!("{tsv}");
    eprintln!("wrote {outdir}/knob_byte_identity.tsv");
}
