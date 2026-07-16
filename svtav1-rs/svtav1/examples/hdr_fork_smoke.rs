//! Fork-mode smoke + decode-gate driver: encodes the same 4:2:0 frame in
//! Mainline and HdrFork modes, writes IVF files + the encoder's own recon
//! planes, and prints their paths. The caller (tools/hdr_fork_decode_gate.sh)
//! then decodes with aomdec and asserts recon == decoder output for BOTH
//! modes — proving fork-mode signaling matches application.
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

fn run(mode: &str, qp: u8, out_prefix: &str) {
    let (w, h) = (128u32, 128u32);
    let y: Vec<u8> = (0..(w * h) as usize)
        .map(|i| (((i % w as usize) * 3 + (i / w as usize) * 5 + ((i % w as usize) * (i / w as usize)) / 64) % 256) as u8)
        .collect();
    let cwh = ((w / 2) * (h / 2)) as usize;
    let u: Vec<u8> = (0..cwh).map(|i| ((i % 64) + 96) as u8).collect();
    let v: Vec<u8> = (0..cwh).map(|i| ((i % 96) + 64) as u8).collect();

    let mut p = EncodePipeline::new(
        w,
        h,
        6,
        RcConfig { mode: RcMode::Cqp, qp, ..RcConfig::default() },
        4,
        1,
    );
    p.chroma_420 = true;
    if mode == "fork" {
        p.hdr = HdrForkConfig::hdr_fork();
    }
    let bs = p.encode_frame_420(&y, &u, &v, w as usize);
    let (ry, ru, rv) = p.last_recon.clone().expect("recon");
    std::fs::File::create(format!("{out_prefix}.ivf"))
        .unwrap()
        .write_all(&ivf(w, h, &bs))
        .unwrap();
    for (tag, plane) in [("y", &ry), ("u", &ru), ("v", &rv)] {
        std::fs::File::create(format!("{out_prefix}.{tag}.bin"))
            .unwrap()
            .write_all(plane)
            .unwrap();
    }
    println!("{mode} qp{qp}: {} bytes -> {out_prefix}.ivf", bs.len());
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| "/tmp/hdr_fork_smoke".into());
    std::fs::create_dir_all(&dir).unwrap();
    for qp in [20u8, 40, 55] {
        run("mainline", qp, &format!("{dir}/main_q{qp}"));
        run("fork", qp, &format!("{dir}/fork_q{qp}"));
    }
}
