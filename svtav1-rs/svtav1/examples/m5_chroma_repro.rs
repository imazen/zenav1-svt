//! Focused repro for the c420_gradient_96pad_q20_s5 recon-parity U-plane
//! mismatch: encode the exact case, decode with aomdec, print every
//! differing chroma pixel + the SVTAV1_DUMP_TREE leaf covering it.

use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::rate_control::{RcConfig, RcMode};

fn gen_gradient(sz: usize) -> Vec<u8> {
    // recon_parity's gen_content "gradient" arm, verbatim.
    let mut v = vec![0u8; sz * sz];
    for r in 0..sz {
        for c in 0..sz {
            v[r * sz + c] = (((r * 255) / sz) as u8) ^ (((c * 3) & 0x3F) as u8);
        }
    }
    v
}

fn pad_replicate(src: &[u8], sz: usize, enc: usize) -> Vec<u8> {
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

fn main() {
    let enc = 128usize;
    let y = pad_replicate(&gen_gradient(96), 96, enc);
    let u = (0..(enc / 2) * (enc / 2))
        .map(|i| (((i * 3) & 0x7F) + 64) as u8)
        .collect::<Vec<u8>>();
    let v = (0..(enc / 2) * (enc / 2))
        .map(|i| (((i * 5) & 0x7F) + 64) as u8)
        .collect::<Vec<u8>>();
    let rc = RcConfig {
        mode: RcMode::Cqp,
        qp: 20,
        ..Default::default()
    };
    let mut p = EncodePipeline::new(enc as u32, enc as u32, 5, rc, 0, 1).with_chroma_420(true);
    let obu = p.encode_frame_420(&y, &u, &v, enc);
    let (_ry, ru, rv) = p.last_recon.clone().expect("recon");
    let (_uy, uu, _uv2) = p.last_recon_unfiltered.clone().expect("unfiltered");
    let (_py, pu, _pv2) = p.last_recon_pre_cdef.clone().expect("pre-cdef");
    std::fs::write("/tmp/m5repro.obu", &obu).unwrap();
    let st = std::process::Command::new("/root/aomdec-build/aomdec")
        .args(["/tmp/m5repro.obu", "-o", "/tmp/m5repro.y4m"])
        .status()
        .unwrap();
    assert!(st.success());
    let data = std::fs::read("/tmp/m5repro.y4m").unwrap();
    let hdr_end = data.iter().position(|&b| b == b'\n').unwrap();
    let frame_pos = data
        .windows(5)
        .skip(hdr_end)
        .position(|w| w == b"FRAME")
        .unwrap()
        + hdr_end;
    let y_start = data[frame_pos..].iter().position(|&b| b == b'\n').unwrap() + frame_pos + 1;
    let ysz = enc * enc;
    let csz = (enc / 2) * (enc / 2);
    let du = &data[y_start + ysz..y_start + ysz + csz];
    let dv = &data[y_start + ysz + csz..y_start + ysz + 2 * csz];
    let cw = enc / 2;
    // Pre-deblock window around the diverging edges.
    for r in 46..60 {
        let row: Vec<String> = (36..44).map(|c| format!("{:3}", uu[r * cw + c])).collect();
        println!("preU r{r}: {}", row.join(" "));
    }
    for r in 46..60 {
        let row: Vec<String> = (36..44).map(|c| format!("{:3}", pu[r * cw + c])).collect();
        println!("postdblkU r{r}: {}", row.join(" "));
    }
    for (plane, dec, encp) in [("U", du, &ru[..]), ("V", dv, &rv[..])] {
        let n = dec.iter().zip(encp.iter()).filter(|(a, b)| a != b).count();
        println!("{plane}: {n} differing px");
        for (i, (a, b)) in dec.iter().zip(encp.iter()).enumerate() {
            if a != b {
                println!(
                    "  {plane} chroma (c{}, r{}) luma ({}, {}) dec={a} enc={b} pre_dblk={} pre_cdef={}",
                    i % cw,
                    i / cw,
                    2 * (i % cw),
                    2 * (i / cw),
                    if plane == "U" { uu[i] } else { 0 },
                    if plane == "U" { pu[i] } else { 0 },
                );
            }
        }
    }
}
