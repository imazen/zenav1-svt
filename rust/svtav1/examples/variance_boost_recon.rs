//! Recon-parity witness for the MAINLINE variance-boost delta-q path.
//!
//! The encoder's own reconstruction must equal a conforming decoder's output
//! bit-exactly. When per-SB delta-q is signalled, the pack writes
//! `(cur - prev) / delta_q_res` with a TRUNCATING integer divide and then sets
//! `prev = cur` (C `entropy_coding.c:4996-5015`), while the decoder sets
//! `prev = prev + reduced * delta_q_res` (spec 5.11.41). Those two accumulators
//! only agree when every SB qindex is congruent to the frame base modulo
//! `delta_q_res` — which is what C's `svt_av1_normalize_sb_delta_q`
//! (`rc_aq.c:830`, called from `generate_sb_qindex`, `rc_process.c:741-744`,
//! MAINLINE — outside every `#if SVT_HDR_MODE` block) guarantees. Skip it and
//! the residues never cancel: the error COMPOUNDS across the SB raster, so
//! encoder and decoder dequantize with different qindexes and the pixels
//! diverge.
//!
//! Reachability of the mainline path: `HdrForkConfig::apply_tune_overrides`
//! sets `enable_variance_boost = true` for TUNE_IQ / TUNE_MS_SSIM regardless of
//! mode (`hdr_mode.rs:330-346`) and the pipeline calls it unconditionally — so
//! plain MAINLINE with `hdr.tune = 3` at CLI qp >= 20 (qindex >= 80, i.e.
//! `delta_q_res >= 2`) reaches it.
//!
//! Usage: `tools/variance_boost_recon.sh [outdir]`, or directly
//!   `SVTAV1_VB_DUMP=<path> cargo run --release -p zenav1-svt \
//!        --example variance_boost_recon -- [outdir]`
//!
//! `SVTAV1_VB_DUMP` is REQUIRED (the example aborts without it, rather than
//! silently skipping): it is how the per-SB plan is read back for the
//! anti-vacuity accounting — a run in which no cell produced a non-uniform
//! plan at `delta_q_res > 1` proves nothing and is failed explicitly.
//!
//! Env: `AOMDEC` (default `/opt/homebrew/bin/aomdec`), `SVTAV1_VB_DUMP`.

use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::rate_control::{RcConfig, RcMode};

fn gen_luma(content: &str, w: usize, h: usize) -> Vec<u8> {
    let mut y = vec![0u8; w * h];
    let sb_cols = (w / 64).max(1);
    for r in 0..h {
        for c in 0..w {
            // Deliberately mixed variance ACROSS superblocks: flat 64x64
            // patches next to textured ones is what makes the boost produce a
            // non-uniform per-SB qindex plan (a uniform plan is a vacuous cell).
            let sb = (r / 64) * sb_cols + (c / 64);
            y[r * w + c] = match content {
                "mixed" => {
                    if sb % 3 == 0 {
                        128
                    } else if sb % 3 == 1 {
                        (((r * 7) ^ (c * 13)) & 0xFF) as u8
                    } else {
                        (128 + ((r / 4 + c / 4) % 9) * 3) as u8
                    }
                }
                _ => {
                    if sb % 2 == 0 {
                        120
                    } else {
                        (((r * 255) / h) as u8) ^ (((c * 3) & 0x3F) as u8)
                    }
                }
            };
        }
    }
    y
}

fn decode_y4m_planes(path: &str, w: usize, h: usize) -> Option<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    let data = std::fs::read(path).ok()?;
    let hdr_end = data.iter().position(|&b| b == b'\n')?;
    let frame_pos = data.windows(5).skip(hdr_end).position(|w| w == b"FRAME")? + hdr_end;
    let y_start = data[frame_pos..].iter().position(|&b| b == b'\n')? + frame_pos + 1;
    let ysz = w * h;
    let csz = (w / 2) * (h / 2);
    if data.len() < y_start + ysz + 2 * csz {
        return None;
    }
    Some((
        data[y_start..y_start + ysz].to_vec(),
        data[y_start + ysz..y_start + ysz + csz].to_vec(),
        data[y_start + ysz + csz..y_start + ysz + 2 * csz].to_vec(),
    ))
}

/// Parse the `SVTAV1_VB_DUMP` line: `base=N res=N plan=[a, b, ...]`.
fn parse_plan(s: &str) -> (i32, i32, Vec<i32>) {
    let field = |k: &str| -> i32 {
        s.split(k)
            .nth(1)
            .and_then(|t| t.split(|c: char| !c.is_ascii_digit()).next())
            .and_then(|t| t.parse().ok())
            .unwrap_or(-1)
    };
    let plan = s
        .split("plan=[")
        .nth(1)
        .and_then(|t| t.split(']').next())
        .map(|t| {
            t.split(',')
                .filter_map(|x| x.trim().parse::<i32>().ok())
                .collect()
        })
        .unwrap_or_default();
    (field("base="), field("res="), plan)
}

fn main() {
    let outdir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/variance_boost_recon".to_string());
    std::fs::create_dir_all(&outdir).unwrap();
    let aomdec = std::env::var("AOMDEC").unwrap_or_else(|_| "/opt/homebrew/bin/aomdec".to_string());
    // Required, not optional: without it the anti-vacuity accounting below has
    // no input and the gate could pass while testing nothing.
    let dump = std::env::var("SVTAV1_VB_DUMP").expect(
        "SVTAV1_VB_DUMP=<path> is required — it is how the per-SB delta-q plan \
         is read back for anti-vacuity accounting (use tools/variance_boost_recon.sh)",
    );

    let mut pass = 0usize;
    let mut fail = 0usize;
    let mut failures: Vec<String> = Vec::new();
    // Anti-vacuity accounting: a cell only tests anything if the boost produced
    // a NON-UNIFORM per-SB plan at delta_q_res != 1.
    let mut nonuniform_cells = 0usize;
    let mut residue_violations = 0usize;
    let mut planned_sbs = 0usize;

    for content in ["mixed", "split"] {
        for (w, h) in [(128usize, 128usize), (192, 128), (256, 192)] {
            // CLI qp -> qindex 80/120/160/220/255 -> delta_q_res 2/4/8/8/8.
            // qp 55 and 63 additionally give a base whose remainder mod 8 is
            // NON-ZERO (220 % 8 == 4, 255 % 8 == 7) — the case where keying the
            // normalizer on the fork's recentered base (instead of the frame
            // base mainline actually signals) also breaks.
            for qp in [20u8, 30, 40, 55, 63] {
                for speed in [6u8, 10] {
                    let name = format!("vb_{content}_{w}x{h}_q{qp}_s{speed}");
                    let y = gen_luma(content, w, h);
                    let u: Vec<u8> = (0..(w / 2) * (h / 2))
                        .map(|i| (((i * 3) & 0x7F) + 64) as u8)
                        .collect();
                    let v: Vec<u8> = (0..(w / 2) * (h / 2))
                        .map(|i| (((i * 5) & 0x7F) + 64) as u8)
                        .collect();
                    let rc = RcConfig {
                        mode: RcMode::Cqp,
                        qp,
                        ..Default::default()
                    };
                    let mut p = EncodePipeline::new(w as u32, h as u32, speed, rc, 0, 1)
                        .with_chroma_420(true);
                    // MAINLINE mode (the default) + tune IQ: apply_tune_overrides
                    // turns variance boost on regardless of mode.
                    p.hdr.tune = svtav1_encoder::tune::TUNE_IQ;
                    assert!(!p.hdr.is_fork(), "witness must exercise the MAINLINE arm");

                    let obu = p.encode_frame_420(&y, &u, &v, w);

                    let (base, res, sbq) = parse_plan(&std::fs::read_to_string(&dump).unwrap());
                    assert!(
                        !sbq.is_empty(),
                        "{name}: no per-SB plan — variance boost is OFF"
                    );
                    planned_sbs += sbq.len();
                    if res > 1 && sbq.iter().any(|&q| q != sbq[0]) {
                        nonuniform_cells += 1;
                    }
                    let bad = sbq
                        .iter()
                        .filter(|&&q| res > 1 && (q - base).rem_euclid(res) != 0)
                        .count();
                    residue_violations += bad;

                    let (ry, ru, rv) = p.last_recon.clone().expect("recon published");
                    let obu_path = format!("{outdir}/{name}.obu");
                    let y4m_path = format!("{outdir}/{name}.y4m");
                    std::fs::write(&obu_path, &obu).unwrap();
                    let st = std::process::Command::new(&aomdec)
                        .args([&obu_path, "-o", &y4m_path])
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .status()
                        .expect("run aomdec");
                    if !st.success() {
                        fail += 1;
                        failures.push(format!("{name}: DECODE FAILED (base={base} res={res})"));
                        continue;
                    }
                    let Some((dy, du, dv)) = decode_y4m_planes(&y4m_path, w, h) else {
                        fail += 1;
                        failures.push(format!("{name}: y4m parse failed"));
                        continue;
                    };
                    let mut diffs = Vec::new();
                    for (plane, dec, enc) in [("Y", &dy, &ry), ("U", &du, &ru), ("V", &dv, &rv)] {
                        if dec != enc {
                            let n = dec.iter().zip(enc.iter()).filter(|(a, b)| a != b).count();
                            let i = dec
                                .iter()
                                .zip(enc.iter())
                                .position(|(a, b)| a != b)
                                .unwrap();
                            diffs.push(format!(
                                "{plane} {n} px (first @{i}: dec={} enc={})",
                                dec[i], enc[i]
                            ));
                        }
                    }
                    if bad > 0 {
                        diffs.push(format!("{bad} SB qindexes outside base residue class"));
                    }
                    if diffs.is_empty() {
                        pass += 1;
                    } else {
                        fail += 1;
                        failures.push(format!(
                            "{name}: {} | base={base} res={res} plan={sbq:?}",
                            diffs.join("; ")
                        ));
                    }
                }
            }
        }
    }

    println!(
        "variance-boost evidence: {nonuniform_cells}/{} cells produced a non-uniform \
         per-SB plan at delta_q_res > 1 over {planned_sbs} planned SBs; \
         {residue_violations} SB qindexes outside the base residue class",
        pass + fail
    );
    println!("variance-boost recon parity: {pass} passed, {fail} failed");
    for f in &failures {
        println!("  {f}");
    }
    // Anti-vacuity: if no cell ever produced a non-uniform plan, this gate is
    // not testing the delta-q path at all and must fail loudly.
    if nonuniform_cells == 0 {
        println!("VACUOUS GATE: no cell produced a non-uniform per-SB delta-q plan");
        std::process::exit(2);
    }
    if fail > 0 {
        std::process::exit(1);
    }
}
