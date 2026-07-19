//! Per-tune decode-gate driver: encode fork mode at each tune x qp, write
//! IVF + recon for the aomdec byte-identity check.
use svtav1_encoder::hdr_mode::HdrForkConfig;
use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::rate_control::{RcConfig, RcMode};
use std::io::Write;
fn main() {
    let dir = std::env::args().nth(1).unwrap();
    let preset: u8 = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(6);
    let (w, h) = (128u32, 128u32);
    let y: Vec<u8> = (0..(w*h) as usize).map(|i| { let (x,yy)=(i%128,i/128); if yy<64 {100} else {((x*3+yy*5+(x*yy)/64)%256) as u8} }).collect();
    let cwh = 64*64;
    let u: Vec<u8> = (0..cwh).map(|i| ((i%64)+96) as u8).collect();
    let v: Vec<u8> = (0..cwh).map(|i| ((i%96)+64) as u8).collect();
    for tune in [0u8, 1, 2, 3, 4, 5] {
        for qp in [20u8, 40, 55] {
            let mut p = EncodePipeline::new(w,h,preset, RcConfig{mode:RcMode::Cqp,qp,..RcConfig::default()},4,1);
            p.chroma_420=true;
            p.hdr=HdrForkConfig::hdr_fork();
            p.hdr.tune=tune;
            let bs = p.encode_frame_420(&y,&u,&v,128);
            let (ry,ru,rv) = p.last_recon.clone().unwrap();
            let pre = format!("{dir}/t{tune}_q{qp}");
            let mut ivf = Vec::new();
            ivf.extend_from_slice(b"DKIF"); ivf.extend_from_slice(&0u16.to_le_bytes()); ivf.extend_from_slice(&32u16.to_le_bytes());
            ivf.extend_from_slice(b"AV01"); ivf.extend_from_slice(&(w as u16).to_le_bytes()); ivf.extend_from_slice(&(h as u16).to_le_bytes());
            ivf.extend_from_slice(&30u32.to_le_bytes()); ivf.extend_from_slice(&1u32.to_le_bytes()); ivf.extend_from_slice(&1u32.to_le_bytes()); ivf.extend_from_slice(&0u32.to_le_bytes());
            ivf.extend_from_slice(&(bs.len() as u32).to_le_bytes()); ivf.extend_from_slice(&0u64.to_le_bytes()); ivf.extend_from_slice(&bs);
            std::fs::File::create(format!("{pre}.ivf")).unwrap().write_all(&ivf).unwrap();
            let mut rec = ry; rec.extend_from_slice(&ru); rec.extend_from_slice(&rv);
            std::fs::File::create(format!("{pre}.recon.yuv")).unwrap().write_all(&rec).unwrap();
        }
    }
}
