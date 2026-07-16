//! [SVT_HDR_MODE] fork-mode liveness regressions.
//!
//! These pin that the runtime fork switch and its per-feature knobs are
//! actually reachable end-to-end — a knob whose streams are byte-identical
//! to its disabled form has silently gone dormant (the CodingQuantCfg
//! stamping class of bug). Stream CORRECTNESS for fork mode is gated
//! separately by the aomdec decode gate (`examples/hdr_fork_smoke.rs`
//! header describes the loop; run at presets 2/4/6/8), which cannot run
//! here without the external decoder binary.

use svtav1_encoder::hdr_mode::HdrForkConfig;
use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::rate_control::{RcConfig, RcMode};

/// Flat top / textured bottom 128x128 — the flat half makes variance boost
/// emit real nonzero per-SB deltas (delta_q_present=1, the sharp-tx gate).
fn content() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let (w, h) = (128usize, 128usize);
    let y: Vec<u8> = (0..w * h)
        .map(|i| {
            let (x, yy) = (i % w, i / w);
            if yy < h / 2 {
                100u8
            } else {
                ((x * 3 + yy * 5 + (x * yy) / 64) % 256) as u8
            }
        })
        .collect();
    let cwh = (w / 2) * (h / 2);
    let u: Vec<u8> = (0..cwh).map(|i| ((i % 64) + 96) as u8).collect();
    let v: Vec<u8> = (0..cwh).map(|i| ((i % 96) + 64) as u8).collect();
    (y, u, v)
}

fn encode_with(hdr: Option<HdrForkConfig>, qp: u8, preset: u8) -> Vec<u8> {
    let (y, u, v) = content();
    let mut p = EncodePipeline::new(
        128,
        128,
        preset,
        RcConfig {
            mode: RcMode::Cqp,
            qp,
            ..RcConfig::default()
        },
        4,
        1,
    );
    p.chroma_420 = true;
    if let Some(cfg) = hdr {
        p.hdr = cfg;
    }
    p.encode_frame_420(&y, &u, &v, 128)
}

#[test]
fn fork_mode_differs_from_mainline() {
    for (preset, qp) in [(2u8, 20u8), (6, 40)] {
        let main = encode_with(None, qp, preset);
        let fork = encode_with(Some(HdrForkConfig::hdr_fork()), qp, preset);
        assert_ne!(
            main, fork,
            "p{preset} qp{qp}: fork mode produced mainline-identical bytes"
        );
    }
}

#[test]
fn sharp_tx_is_live_in_fork_mode() {
    // sharp_tx=1 zeroes the RDOQ rate weight and disables the trellis
    // eob-shortening on luma whenever per-SB delta-q is present
    // (full_loop.c:1070-1078) — streams must differ from sharp_tx=0.
    for (preset, qp) in [(2u8, 20u8), (6, 40), (6, 55)] {
        let mut on = HdrForkConfig::hdr_fork();
        on.sharp_tx = 1;
        let mut off = HdrForkConfig::hdr_fork();
        off.sharp_tx = 0;
        let a = encode_with(Some(on), qp, preset);
        let b = encode_with(Some(off), qp, preset);
        assert_ne!(
            a, b,
            "p{preset} qp{qp}: sharp_tx knob is inert (streams identical)"
        );
        // rweight=0 keeps more coefficients: sharp streams never smaller.
        assert!(
            a.len() >= b.len(),
            "p{preset} qp{qp}: sharp_tx=1 stream smaller than sharp_tx=0 \
             ({} < {}) — wrong gate direction",
            a.len(),
            b.len()
        );
    }
}

#[test]
fn noise_norm_is_live_in_fork_mode() {
    for (preset, qp) in [(6u8, 40u8)] {
        let mut on = HdrForkConfig::hdr_fork();
        on.noise_norm_strength = 4;
        let mut off = HdrForkConfig::hdr_fork();
        off.noise_norm_strength = 0;
        let a = encode_with(Some(on), qp, preset);
        let b = encode_with(Some(off), qp, preset);
        assert_ne!(
            a, b,
            "p{preset} qp{qp}: noise_norm_strength knob is inert"
        );
    }
}
