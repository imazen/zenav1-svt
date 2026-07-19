//! Deblocking evidence probe: show the picked filter levels and the
//! before/after-deblock PSNR at high qindex, with recon parity verified
//! against aomdec for every measured stream.
//!
//! QP DOMAIN: takes CLI-domain qps (0..63, C `--qp` semantics). At CLI
//! qp 55 (qindex 220) and 63 (qindex 255) the q-picked levels are large
//! enough that the loop filter does material work — this probe is the
//! regression evidence that the filter actually runs on the output recon
//! (levels, pixel delta count, PSNR movement) while staying bit-exact
//! with the reference decoder.
//!
//! Usage: cargo run --release -p zenav1-svt --example deblock_evidence -- [outdir]
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
        .unwrap_or_else(|| "target/deblock_evidence".to_string());
    std::fs::create_dir_all(&outdir).unwrap();
    let aomdec =
        std::env::var("AOMDEC").unwrap_or_else(|_| "/root/aomdec-build/aomdec".to_string());

    println!(
        "{:<10} {:>3} {:>6} {:>15} {:>9} {:>12} {:>12} {:>8} {:>7}",
        "content",
        "qp",
        "qindex",
        "lf[y,y,u,v]",
        "px_moved",
        "psnr_pre",
        "psnr_post",
        "parity",
        "bytes"
    );

    let sz = 128usize;
    for content in ["edges", "gradient"] {
        for qp in [55u8, 63] {
            let qindex = qp_to_qindex(qp);
            let levels = svtav1_encoder::deblock::pick_filter_levels_key_frame(qindex, 8).levels;
            let y = gen_content(content, sz);
            let rc = RcConfig {
                mode: RcMode::Cqp,
                qp,
                ..Default::default()
            };
            let mut p = EncodePipeline::new(sz as u32, sz as u32, 4, rc, 0, 1);
            let obu = p.encode_frame(&y, sz);
            let (post, _, _) = p.last_recon.clone().unwrap();
            let (pre, _, _) = p.last_recon_unfiltered.clone().unwrap();

            let moved = pre.iter().zip(&post).filter(|(a, b)| a != b).count();
            let psnr_pre = psnr(&y, &pre);
            let psnr_post = psnr(&y, &post);

            // Recon parity: aomdec must reproduce the POST-deblock recon.
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
                "{:<10} {:>3} {:>6} {:>15} {:>9} {:>12.2} {:>12.2} {:>8} {:>7}",
                content,
                qp,
                qindex,
                format!("{levels:?}"),
                moved,
                psnr_pre,
                psnr_post,
                if parity { "EXACT" } else { "FAIL" },
                obu.len()
            );
            assert!(parity, "{content} q{qp}: recon parity failed");
        }
    }
}
