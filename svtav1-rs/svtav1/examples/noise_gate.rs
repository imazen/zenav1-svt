//! Grain decode-gate driver: encode fork mode with --noise at several
//! strengths/qps, write IVF + recon; caller decodes with and without
//! --skip-film-grain (skip == recon, plain != recon proves grain applied).
use svtav1_encoder::hdr_mode::HdrForkConfig;
use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::rate_control::{RcConfig, RcMode};
use std::io::Write;

fn ivf(w: u32, h: u32, frame: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"DKIF");
    v.extend_from_slice(&0u16.to_le_bytes());
    v.extend_from_slice(&32u16.to_le_bytes());
    v.extend_from_slice(b"AV01");
    v.extend_from_slice(&(w as u16).to_le_bytes());
    v.extend_from_slice(&(h as u16).to_le_bytes());
    v.extend_from_slice(&30u32.to_le_bytes());
    v.extend_from_slice(&1u32.to_le_bytes());
    v.extend_from_slice(&1u32.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(&(frame.len() as u32).to_le_bytes());
    v.extend_from_slice(&0u64.to_le_bytes());
    v.extend_from_slice(frame);
    v
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| "/tmp/noise_gate".into());
    std::fs::create_dir_all(&dir).unwrap();
    let (w, h) = (128u32, 128u32);
    let y: Vec<u8> = (0..(w * h) as usize)
        .map(|i| {
            let (x, yy) = (i % w as usize, i / w as usize);
            if yy < 64 { 100u8 } else { ((x * 3 + yy * 5 + (x * yy) / 64) % 256) as u8 }
        })
        .collect();
    let cwh = ((w / 2) * (h / 2)) as usize;
    let u: Vec<u8> = (0..cwh).map(|i| ((i % 64) + 96) as u8).collect();
    let v: Vec<u8> = (0..cwh).map(|i| ((i % 96) + 64) as u8).collect();
    for (strength, qp) in [(8u8, 20u8), (25, 40), (120, 55)] {
        let mut p = EncodePipeline::new(
            w, h, 6,
            RcConfig { mode: RcMode::Cqp, qp, ..RcConfig::default() }, 4, 1,
        );
        p.chroma_420 = true;
        p.hdr = HdrForkConfig::hdr_fork();
        p.hdr.noise_strength = strength;
        let bs = p.encode_frame_420(&y, &u, &v, w as usize);
        let (ry, ru, rv) = p.last_recon.clone().expect("recon");
        let pre = format!("{dir}/n{strength}_q{qp}");
        std::fs::File::create(format!("{pre}.ivf")).unwrap().write_all(&ivf(w, h, &bs)).unwrap();
        let mut rec = ry.clone();
        rec.extend_from_slice(&ru);
        rec.extend_from_slice(&rv);
        std::fs::File::create(format!("{pre}.recon.yuv")).unwrap().write_all(&rec).unwrap();
        println!("n{strength} q{qp}: {} bytes", bs.len());
    }
}
