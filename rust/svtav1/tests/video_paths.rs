//! Video paths through the public pipeline, under `cargo nextest`.
//!
//! The byte gates verify the inter tools against C and a decoder, but they
//! are shell scripts, so the nextest surface (and `tools/coverage_gate.sh`)
//! never ran them: on 2026-09-26 the TPL dispenser, the CBR driver, warp
//! refinement and OBMC refinement read 0-3 % region coverage, and a panic
//! there would surface only in CI's slowest shard. `never_panics` covers LD
//! and RA on tiny noise frames, which reach none of them: TPL needs random
//! access with `aq_mode` 2, and warp / OBMC need real non-translational
//! motion at the low presets.
//!
//! Each case encodes five frames of a texture that pans AND zooms, then
//! asserts that every call succeeds, that the stream is not empty, and that a
//! second encode of the same input is byte-identical. It checks no bytes
//! against C; the gates do that.

use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::port_picstruct::{HIERARCHICAL_LEVELS_AUTO, PredStructure};
use svtav1_encoder::rate_control::{RcConfig, RcMode};
use svtav1_encoder::speed_config::NativePreset;

const W: usize = 128;
const H: usize = 128;
const FRAMES: usize = 5;

/// Frame `f` of a textured scene that pans 2 px/frame and zooms ~3 %/frame
/// about the centre, so the best predictor is not one integer MV.
fn frame(f: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let tex = |x: i64, y: i64| -> u8 {
        let (x, y) = (x.rem_euclid(512) as u64, y.rem_euclid(512) as u64);
        let checker = (((x / 16) ^ (y / 16)) & 1) * 60;
        let ramp = (x * 3 + y * 2) & 0x7f;
        let mut z = x * 2654435761 ^ y * 40503;
        z ^= z >> 13;
        (40 + checker + ramp + (z % 9)) as u8
    };
    // Scale in 1/1024 units: 1024 * 1.03^f.
    let scale = (0..f).fold(1024i64, |s, _| s * 1055 / 1024);
    let (cx, cy) = (W as i64 / 2, H as i64 / 2);
    let sample = |c: i64, r: i64| {
        tex(
            cx + (c - cx) * 1024 / scale + 2 * f as i64,
            cy + (r - cy) * 1024 / scale,
        )
    };
    let y = (0..H)
        .flat_map(|r| (0..W).map(move |c| (r, c)))
        .map(|(r, c)| sample(c as i64, r as i64))
        .collect();
    let (cw, ch) = (W / 2, H / 2);
    let u = (0..cw * ch).map(|i| 110 + ((i % cw) as u8 / 8)).collect();
    let v = (0..cw * ch).map(|i| 140 - ((i / cw) as u8 / 8)).collect();
    (y, u, v)
}

#[derive(Clone, Copy, Debug)]
struct Case {
    preset: i8,
    random_access: bool,
    rc: RcMode,
    aq_mode: u8,
}

fn encode(case: Case) -> Vec<u8> {
    let rc = RcConfig {
        mode: case.rc,
        qp: 40,
        aq_mode: case.aq_mode,
        target_bitrate: if case.rc == RcMode::Cqp { 0 } else { 400 },
        ..RcConfig::default()
    };
    let hier = if case.random_access {
        HIERARCHICAL_LEVELS_AUTO
    } else {
        0
    };
    let preset = NativePreset::new(case.preset).expect("preset in -1..=13");
    let mut p = EncodePipeline::new_with_preset(W as u32, H as u32, preset, rc, hier, 64)
        .with_chroma_420(true);
    if case.random_access {
        p = p.with_pred_structure(PredStructure::RandomAccess);
    }
    let mut out = Vec::new();
    for f in 0..FRAMES {
        let (y, u, v) = frame(f);
        let bytes = p
            .try_encode_frame_420(&y, &u, &v, W)
            .unwrap_or_else(|e| panic!("{case:?} frame {f}: {e}"));
        out.extend_from_slice(&bytes);
    }
    out.extend_from_slice(
        &p.try_flush()
            .unwrap_or_else(|e| panic!("{case:?} flush: {e}")),
    );
    out
}

fn check(case: Case) {
    let a = encode(case);
    assert!(!a.is_empty(), "{case:?}: empty stream");
    let b = encode(case);
    assert!(a == b, "{case:?}: two encodes of the same input differ");
}

#[test]
fn low_delay_low_presets_reach_warp_and_obmc() {
    for preset in [0, 2, 4] {
        check(Case {
            preset,
            random_access: false,
            rc: RcMode::Cqp,
            aq_mode: 0,
        });
    }
}

#[test]
fn low_delay_high_presets() {
    for preset in [8, 13] {
        check(Case {
            preset,
            random_access: false,
            rc: RcMode::Cqp,
            aq_mode: 0,
        });
    }
}

#[test]
fn random_access_with_tpl() {
    // aq_mode 2 turns TPL on under random access (C `get_tpl`).
    for preset in [2, 6, 10] {
        check(Case {
            preset,
            random_access: true,
            rc: RcMode::Cqp,
            aq_mode: 2,
        });
    }
}

#[test]
fn random_access_without_tpl() {
    check(Case {
        preset: 6,
        random_access: true,
        rc: RcMode::Cqp,
        aq_mode: 0,
    });
}

#[test]
fn cbr_low_delay() {
    for preset in [6, 10] {
        check(Case {
            preset,
            random_access: false,
            rc: RcMode::Cbr,
            aq_mode: 0,
        });
    }
}
