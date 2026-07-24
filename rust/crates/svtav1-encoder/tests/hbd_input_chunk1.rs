//! Task #6 chunk 1 gate — native 10-bit (u16) SOURCE input.
//!
//! `rust/docs/hbd-input-port-map.md` specifies two properties, and both are
//! required: either alone is passable by a broken implementation.
//!
//! * **EQUIVALENCE** — pushing `widen(s8) = (s8 as u16) << 2` through the hbd
//!   entry point must emit EXACTLY the bytes the u8 entry point emits for
//!   `s8` on the same bd10 pipeline. That is what proves the threading is a
//!   pure refactor on 8-bit content, and therefore that every pre-existing
//!   bd10 gate cell (all of which feed widened u8) is byte-untouched.
//! * **WITNESS (anti-vacuity)** — a source whose low 2 bits are NOT zero must
//!   emit DIFFERENT bytes than the same source MSB-truncated to 8 bits.
//!   Without it, the equivalence test above would pass on an entry point that
//!   ignored its `&[u16]` argument entirely.
//!
//! Both bd10 consumers are covered: preset 6 (the full-RD funnel — luma AND
//! chroma at 10 bits) and preset 9 (the MDS0 luma funnel + the level
//! re-encode post-pass).

use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::rate_control::{RcConfig, RcMode};

const W: usize = 64;
const H: usize = 64;
const CW: usize = W / 2;
const CH: usize = H / 2;

/// 8-bit test content: a diagonal ramp with a periodic texture on top, so the
/// residual is non-trivial at every block size (a flat frame would code to
/// all-skip and hide any source-precision difference).
fn content8() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let mut y = vec![0u8; W * H];
    for r in 0..H {
        for c in 0..W {
            let ramp = (r * 3 + c * 2) as i32;
            let tex = if (r / 4 + c / 4) % 2 == 0 { 18 } else { -14 };
            y[r * W + c] = (60 + (ramp % 120) + tex).clamp(0, 255) as u8;
        }
    }
    let mut u = vec![0u8; CW * CH];
    let mut v = vec![0u8; CW * CH];
    for r in 0..CH {
        for c in 0..CW {
            u[r * CW + c] = (100 + ((r * 2 + c) % 40)) as u8;
            v[r * CW + c] = (150 - ((r + c * 2) % 40)) as u8;
        }
    }
    (y, u, v)
}

fn widen(s: &[u8]) -> Vec<u16> {
    s.iter().map(|&x| u16::from(x) << 2).collect()
}

/// `widen(s8)` plus a spatially varying low-2-bit pattern — a source that
/// only a real 10-bit path can distinguish from `widen(s8)`.
fn with_low_bits(s: &[u8], stride: usize) -> Vec<u16> {
    s.iter()
        .enumerate()
        .map(|(i, &x)| {
            let (r, c) = (i / stride, i % stride);
            (u16::from(x) << 2) | ((r + c) % 4) as u16
        })
        .collect()
}

fn pipeline(preset: u8, qp: u8) -> EncodePipeline {
    let rc = RcConfig {
        mode: RcMode::Cqp,
        qp,
        ..RcConfig::default()
    };
    EncodePipeline::new(W as u32, H as u32, preset, rc, 0, 1)
        .with_bit_depth(10)
        .with_chroma_420(true)
}

fn encode_u8(preset: u8, qp: u8, y: &[u8], u: &[u8], v: &[u8]) -> Vec<u8> {
    pipeline(preset, qp).encode_frame_420(y, u, v, W)
}

fn encode_hbd(preset: u8, qp: u8, y: &[u16], u: &[u16], v: &[u16]) -> Vec<u8> {
    pipeline(preset, qp)
        .try_encode_frame_420_hbd(y, u, v, W)
        .expect("hbd entry point inside its documented envelope")
}

/// EQUIVALENCE: widened-u8 through the hbd path == u8 through the u8 path,
/// byte for byte, at both bd10 consumers and across the qp range.
#[test]
fn hbd_widened_u8_is_byte_identical_to_the_u8_path() {
    let (y8, u8p, v8p) = content8();
    let (y10, u10, v10) = (widen(&y8), widen(&u8p), widen(&v8p));
    for preset in [6u8, 9] {
        for qp in [8u8, 32, 55] {
            let base = encode_u8(preset, qp, &y8, &u8p, &v8p);
            let hbd = encode_hbd(preset, qp, &y10, &u10, &v10);
            assert_eq!(
                base,
                hbd,
                "preset {preset} qp {qp}: widened-u8 hbd encode must be byte-identical to the \
                 u8 encode ({} vs {} bytes)",
                base.len(),
                hbd.len()
            );
        }
    }
}

/// WITNESS: the low 2 bits reach the encode — a real 10-bit source must not
/// produce the MSB-truncated stream.
#[test]
fn hbd_low_bits_change_the_bitstream() {
    let (y8, u8p, v8p) = content8();
    let (y10, u10, v10) = (
        with_low_bits(&y8, W),
        with_low_bits(&u8p, CW),
        with_low_bits(&v8p, CW),
    );
    for preset in [6u8, 9] {
        let qp = 8;
        let truncated = encode_u8(preset, qp, &y8, &u8p, &v8p);
        let native = encode_hbd(preset, qp, &y10, &u10, &v10);
        assert_ne!(
            truncated, native,
            "preset {preset} qp {qp}: a source carrying non-zero low 2 bits must not encode to \
             the MSB-truncated stream — the u16 argument is being ignored"
        );
    }
}

/// The hbd stream stays a valid AV1 stream: same OBU prefix shape as the u8
/// encode (sequence header + frame OBU), non-empty payload.
#[test]
fn hbd_stream_is_well_formed() {
    let (y8, u8p, v8p) = content8();
    let (y10, u10, v10) = (
        with_low_bits(&y8, W),
        with_low_bits(&u8p, CW),
        with_low_bits(&v8p, CW),
    );
    let native = encode_hbd(9, 32, &y10, &u10, &v10);
    let base = encode_u8(9, 32, &y8, &u8p, &v8p);
    assert!(native.len() > 16, "hbd stream too short: {}", native.len());
    // OBU header byte of the first OBU (temporal delimiter / sequence header)
    // is config-derived, not content-derived, so it must match the u8 encode.
    assert_eq!(
        native[0], base[0],
        "hbd stream must open with the same OBU type as the u8 encode"
    );
}

/// Out-of-envelope configs are REJECTED rather than silently encoding the
/// MSB-truncated content (the "no silent corruption" bar).
#[test]
fn hbd_rejects_configs_with_no_bd10_consumer() {
    let (y8, u8p, v8p) = content8();
    let (y10, u10, v10) = (widen(&y8), widen(&u8p), widen(&v8p));

    // (a) 8-bit pipeline — no bd10 stage at all.
    let rc = RcConfig {
        mode: RcMode::Cqp,
        qp: 32,
        ..RcConfig::default()
    };
    let mut bd8 = EncodePipeline::new(W as u32, H as u32, 9, rc, 0, 1).with_chroma_420(true);
    assert!(
        bd8.try_encode_frame_420_hbd(&y10, &u10, &v10, W).is_err(),
        "an 8-bit pipeline must reject a native 10-bit source"
    );

    // (b) monochrome pipeline through the 4:2:0 entry point.
    let mut mono = pipeline(9, 32);
    mono.chroma_420 = false;
    assert!(
        mono.try_encode_frame_420_hbd(&y10, &u10, &v10, W).is_err(),
        "the 4:2:0 hbd entry point must reject a monochrome pipeline"
    );

    // (c) a sample above the configured bit depth.
    let mut too_wide = y10.clone();
    too_wide[7] = 1 << 10;
    assert!(
        pipeline(9, 32)
            .try_encode_frame_420_hbd(&too_wide, &u10, &v10, W)
            .is_err(),
        "a sample above the configured bit depth must be rejected"
    );
}

/// Monochrome native-10-bit: the level re-encode post-pass consumes the real
/// u16 source (equivalence + witness, same two properties).
#[test]
fn hbd_mono_equivalence_and_witness() {
    let (y8, _, _) = content8();
    let rc = |qp| RcConfig {
        mode: RcMode::Cqp,
        qp,
        ..RcConfig::default()
    };
    let mono = |qp| EncodePipeline::new(W as u32, H as u32, 9, rc(qp), 0, 1).with_bit_depth(10);
    for qp in [8u8, 32] {
        let base = mono(qp).encode_frame(&y8, W);
        let widened = mono(qp)
            .try_encode_frame_hbd(&widen(&y8), W)
            .expect("mono hbd entry point inside its envelope");
        assert_eq!(
            base, widened,
            "mono qp {qp}: widened-u8 hbd encode must be byte-identical to the u8 encode"
        );
    }
    let native = mono(8)
        .try_encode_frame_hbd(&with_low_bits(&y8, W), W)
        .expect("mono hbd entry point inside its envelope");
    assert_ne!(
        mono(8).encode_frame(&y8, W),
        native,
        "mono: the low 2 bits must reach the coded levels"
    );
}
