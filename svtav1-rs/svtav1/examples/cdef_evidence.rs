//! CDEF evidence probe: show the signaled strengths/damping and the
//! before/after-CDEF PSNR at high qindex, with recon parity verified
//! against aomdec for every measured stream.
//!
//! QP DOMAIN: takes CLI-domain qps (0..63, C `--qp` semantics). CLI qps
//! 43/55/63 map to qindexes 172/220/255 where the qp-strength picker
//! signals y = 17/43/63 — the deringing regime. `psnr_pre` is measured on
//! the post-deblock pre-CDEF recon, `psnr_post` after CDEF, both against
//! the source: the delta is CDEF's isolated contribution. `filtered/chg`
//! are the pipeline's non-vacuity counters (pixels covered by a
//! nonzero-strength kernel call / pixels whose value changed).
//!
//! Usage: cargo run --release -p svtav1 --example cdef_evidence -- [outdir]
//! Env:   AOMDEC (default: /root/aomdec-build/aomdec)

use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::rate_control::{RcConfig, RcMode, qp_to_qindex};

fn gen_content(content: &str, sz: usize) -> Vec<u8> {
    let mut y = vec![0u8; sz * sz];
    for r in 0..sz {
        for c in 0..sz {
            y[r * sz + c] = match content {
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

fn psnr(a: &[u8], b: &[u8]) -> f64 {
    let mut sse = 0u64;
    for (&x, &y) in a.iter().zip(b) {
        let d = x as i64 - y as i64;
        sse += (d * d) as u64;
    }
    if sse == 0 {
        return f64::INFINITY;
    }
    let mse = sse as f64 / a.len() as f64;
    10.0 * (255.0f64 * 255.0 / mse).log10()
}

fn decode_y(path: &str, w: usize, h: usize) -> Option<Vec<u8>> {
    let data = std::fs::read(path).ok()?;
    let hdr_end = data.iter().position(|&b| b == b'\n')?;
    let frame_pos = data.windows(5).skip(hdr_end).position(|x| x == b"FRAME")? + hdr_end;
    let y_start = data[frame_pos..].iter().position(|&b| b == b'\n')? + frame_pos + 1;
    (data.len() >= y_start + w * h).then(|| data[y_start..y_start + w * h].to_vec())
}

fn main() {
    let outdir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/cdef_evidence".to_string());
    std::fs::create_dir_all(&outdir).unwrap();
    let aomdec =
        std::env::var("AOMDEC").unwrap_or_else(|_| "/root/aomdec-build/aomdec".to_string());

    println!(
        "{:<10} {:>3} {:>6} {:>12} {:>9} {:>9} {:>12} {:>12} {:>8} {:>7}",
        "content",
        "qp",
        "qindex",
        "cdef[d,y,uv]",
        "filtered",
        "changed",
        "psnr_pre",
        "psnr_post",
        "parity",
        "bytes"
    );

    let sz = 128usize;
    for content in ["edges", "gradient"] {
        for qp in [43u8, 55, 63] {
            let qindex = qp_to_qindex(qp);
            let params = svtav1_encoder::cdef::pick_cdef_params_key_frame(qindex);
            let y = gen_content(content, sz);
            let rc = RcConfig {
                mode: RcMode::Cqp,
                qp,
                ..Default::default()
            };
            let mut p = EncodePipeline::new(sz as u32, sz as u32, 4, rc, 0, 1);
            let obu = p.encode_frame(&y, sz);
            let (post, _, _) = p.last_recon.clone().unwrap();
            let (pre_cdef, _, _) = p.last_recon_pre_cdef.clone().unwrap();
            let stats = p.last_cdef_stats;

            let psnr_pre = psnr(&y, &pre_cdef);
            let psnr_post = psnr(&y, &post);

            // Recon parity: aomdec must reproduce the POST-CDEF recon.
            let obu_path = format!("{outdir}/{content}_q{qp}.obu");
            let y4m_path = format!("{outdir}/{content}_q{qp}.y4m");
            std::fs::write(&obu_path, &obu).unwrap();
            let st = std::process::Command::new(&aomdec)
                .args([&obu_path, "-o", &y4m_path])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .expect("run aomdec");
            let parity =
                st.success() && decode_y(&y4m_path, sz, sz).as_deref() == Some(post.as_slice());

            println!(
                "{:<10} {:>3} {:>6} {:>12} {:>9} {:>9} {:>12.2} {:>12.2} {:>8} {:>7}",
                content,
                qp,
                qindex,
                format!(
                    "[{},{},{}]",
                    params.damping, params.y_strength, params.uv_strength
                ),
                stats.filtered_px,
                stats.changed_px,
                psnr_pre,
                psnr_post,
                if parity { "EXACT" } else { "FAIL" },
                obu.len()
            );
            assert!(parity, "{content} q{qp}: recon parity failed");
            assert!(
                stats.filtered_px > 0,
                "{content} q{qp}: CDEF did not fire — vacuous evidence"
            );
        }
    }
}
