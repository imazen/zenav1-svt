//! Recon-parity gate: the encoder's reconstruction must equal the AV1
//! reference decoder's output BIT-EXACTLY for every stream.
//!
//! This is the strongest pixel-integrity check available short of full
//! bitstream identity: any divergence between what the encoder believes it
//! reconstructed and what a conforming decoder produces is a shipping bug
//! (the encoder's RD decisions were made against pixels the decoder never
//! sees). It would have caught every pixel bug found by probing this wave.
//!
//! Encodes a matrix in both mono and 4:2:0 modes, decodes each stream with
//! aomdec, and byte-compares all planes.
//!
//! Usage: cargo run --release -p svtav1 --example recon_parity -- [outdir]
//! Env:   AOMDEC (default: /root/aomdec-build/aomdec — override in CI)

use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::rate_control::{RcConfig, RcMode};

fn gen_content(content: &str, sz: usize) -> Vec<u8> {
    let mut y = vec![0u8; sz * sz];
    for r in 0..sz {
        for c in 0..sz {
            y[r * sz + c] = match content {
                "uniform" => 128,
                "edges" => {
                    if (r / 8 + c / 8) % 2 == 0 {
                        32
                    } else {
                        224
                    }
                }
                _ => (((r * 255) / sz) as u8) ^ (((c * 3) & 0x3F) as u8),
            };
        }
    }
    y
}

/// Edge-replicate `src` (sz x sz) up to enc x enc, exactly like
/// decode_conformance / AvifEncoder pad to superblock alignment. The
/// pipeline requires 64-aligned frame dims (unpadded partial-SB frames are
/// an unimplemented front: the partition writer has no
/// split_or_horz/split_or_vert syntax and the search emits unsignalable
/// leaf shapes there), so non-aligned CONTENT sizes are exercised through
/// the same padding path production callers use.
fn pad_replicate(src: &[u8], sz: usize, enc: usize) -> Vec<u8> {
    if enc == sz {
        return src.to_vec();
    }
    let mut out = vec![128u8; enc * enc];
    for r in 0..sz {
        for c in 0..sz {
            out[r * enc + c] = src[r * sz + c];
        }
        for c in sz..enc {
            out[r * enc + c] = out[r * enc + sz - 1];
        }
    }
    for r in sz..enc {
        for c in 0..enc {
            out[r * enc + c] = out[(sz - 1) * enc + c];
        }
    }
    out
}

fn decode_y4m_planes(
    path: &str,
    w: usize,
    h: usize,
    mono: bool,
) -> Option<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    let data = std::fs::read(path).ok()?;
    let hdr_end = data.iter().position(|&b| b == b'\n')?;
    let frame_pos = data.windows(5).skip(hdr_end).position(|w| w == b"FRAME")? + hdr_end;
    let y_start = data[frame_pos..].iter().position(|&b| b == b'\n')? + frame_pos + 1;
    let ysz = w * h;
    let csz = if mono { 0 } else { (w / 2) * (h / 2) };
    if data.len() < y_start + ysz + 2 * csz {
        return None;
    }
    Some((
        data[y_start..y_start + ysz].to_vec(),
        data[y_start + ysz..y_start + ysz + csz].to_vec(),
        data[y_start + ysz + csz..y_start + ysz + 2 * csz].to_vec(),
    ))
}

fn main() {
    let outdir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/recon_parity".to_string());
    std::fs::create_dir_all(&outdir).unwrap();
    let aomdec =
        std::env::var("AOMDEC").unwrap_or_else(|_| "/root/aomdec-build/aomdec".to_string());

    let mut pass = 0usize;
    let mut fail = 0usize;
    let mut failures: Vec<String> = Vec::new();
    // Non-vacuity accounting: the gate is only meaningful if CDEF (and
    // deblocking before it) actually rewrites pixels in these streams.
    let mut cdef_filtered_px = 0u64;
    let mut cdef_changed_px = 0u64;
    let mut cdef_active_streams = 0usize;
    let mut lr_active_streams = 0usize;
    let mut lr_wiener_units = 0usize;

    for chroma in [false, true] {
        for content in ["gradient", "uniform", "edges"] {
            // (content size, encode size): 96px content is edge-replicated
            // to the 64-aligned encode size like every production caller
            // (see pad_replicate) — it exercises different partition/mode
            // choices than the aligned contents while staying within the
            // pipeline's supported geometry.
            for (sz, enc) in [(64usize, 64usize), (96, 128), (128, 128)] {
                // CLI-domain qps -> qindex {80, 172, 255}: low/mid/max of
                // the real quantizer range, incl. qindex 255 where deblock
                // levels peak (the strongest recon-parity stress). The old
                // {30, 50, 90} list ran as qindexes 30/50/63 pre-split.
                for qp in [20u8, 43, 63] {
                    for speed in [1u8, 2, 3, 4, 5, 6, 10] {
                        let name = format!(
                            "{}_{}_{}{}_q{}_s{}",
                            if chroma { "c420" } else { "mono" },
                            content,
                            sz,
                            if enc != sz { "pad" } else { "" },
                            qp,
                            speed
                        );
                        // Progress marker on stderr — also labels the
                        // SVTAV1_DUMP_TREE output per case.
                        eprintln!("case {name}");
                        let y = pad_replicate(&gen_content(content, sz), sz, enc);
                        let rc = RcConfig {
                            mode: RcMode::Cqp,
                            qp,
                            ..Default::default()
                        };
                        let mut p = EncodePipeline::new(enc as u32, enc as u32, speed, rc, 0, 1)
                            .with_chroma_420(chroma);
                        let (u, v);
                        let obu = if chroma {
                            u = (0..(enc / 2) * (enc / 2))
                                .map(|i| (((i * 3) & 0x7F) + 64) as u8)
                                .collect::<Vec<u8>>();
                            v = (0..(enc / 2) * (enc / 2))
                                .map(|i| (((i * 5) & 0x7F) + 64) as u8)
                                .collect::<Vec<u8>>();
                            p.encode_frame_420(&y, &u, &v, enc)
                        } else {
                            p.encode_frame(&y, enc)
                        };
                        let (ry, ru, rv) = p.last_recon.clone().expect("recon published");
                        let (lr_types, lr_units) = p.last_lr_stats;
                        lr_wiener_units += lr_units;
                        lr_active_streams += usize::from(lr_types.iter().any(|&t| t != 0));
                        let cs = p.last_cdef_stats;
                        cdef_filtered_px += cs.filtered_px;
                        cdef_changed_px += cs.changed_px;
                        cdef_active_streams += usize::from(cs.filtered_px > 0);

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
                            failures.push(format!("{name}: DECODE FAILED"));
                            continue;
                        }
                        let Some((dy, du, dv)) = decode_y4m_planes(&y4m_path, enc, enc, !chroma)
                        else {
                            fail += 1;
                            failures.push(format!("{name}: y4m parse failed"));
                            continue;
                        };

                        let mut diffs = Vec::new();
                        if dy != ry {
                            let n = dy.iter().zip(ry.iter()).filter(|(a, b)| a != b).count();
                            let pos: Vec<String> = dy
                                .iter()
                                .zip(ry.iter())
                                .enumerate()
                                .filter(|(_, (a, b))| a != b)
                                .take(8)
                                .map(|(i, (a, b))| {
                                    format!("(r{} c{}) dec={a} enc={b}", i / enc, i % enc)
                                })
                                .collect();
                            diffs.push(format!("Y {n} px: {}", pos.join(" ")));
                            // Dump the encoder recon for offline analysis.
                            std::fs::write(format!("{outdir}/{name}.encY.raw"), &ry).unwrap();
                        }
                        if chroma {
                            if du != ru {
                                let i = du.iter().zip(ru.iter()).position(|(a, b)| a != b).unwrap();
                                diffs.push(format!("U@{i} dec={} enc={}", du[i], ru[i]));
                            }
                            if dv != rv {
                                let i = dv.iter().zip(rv.iter()).position(|(a, b)| a != b).unwrap();
                                diffs.push(format!("V@{i} dec={} enc={}", dv[i], rv[i]));
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
            }
        }
    }

    println!(
        "CDEF evidence: {cdef_active_streams}/{} streams fired, \
         {cdef_filtered_px} px filtered, {cdef_changed_px} px changed",
        pass + fail
    );
    println!(
        "LR evidence: {lr_active_streams}/{} streams signal wiener, \
         {lr_wiener_units} RUs restored",
        pass + fail
    );
    println!("recon parity: {pass} passed, {fail} failed");
    for f in &failures {
        println!("  {f}");
    }
    if fail > 0 {
        std::process::exit(1);
    }
}
