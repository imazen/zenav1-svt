//! `HIERARCHICAL_LEVELS_AUTO` resolution — C's `enc_handle.c:4556-4579` table
//! replayed against `pred_structure` / `rate_control_mode` / `enc_mode` /
//! `input_resolution`, then the all-intra clamp.
//!
//! Why this exists: `cfg.hierarchical_levels` in C defaults to the AUTO
//! sentinel, and the library — not the app — resolves it. A harness that
//! substitutes flat 0 for "unset" silently encodes a flat stream where C
//! builds a pyramid; that substitution masked the port's entire RA
//! emission-order machinery behind a `0 frames vs 7` header diff on
//! 2026-09-25. These tests pin the resolution table itself so the default
//! cannot drift back.

use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::port_picstruct::{HIERARCHICAL_LEVELS_AUTO, PredStructure};
use svtav1_encoder::rate_control::{RcConfig, RcMode};

fn pipe(w: u32, h: u32, preset: u8, mode: RcMode, intra_period: u32) -> EncodePipeline {
    let rc = RcConfig {
        mode,
        qp: 40,
        ..RcConfig::default()
    };
    EncodePipeline::new(w, h, preset, rc, HIERARCHICAL_LEVELS_AUTO, intra_period)
        .with_chroma_420(true)
}

/// RA + CQP + preset <= M8 + below-1080p input falls through every `4` arm
/// to `5` — the value C's untouched default produces on the campaign's
/// video cells (measured: `SVT_HIER_LEVELS` unset, p6, 256x256).
#[test]
fn ra_auto_resolves_five_for_small_cqp() {
    let p = pipe(256, 256, 6, RcMode::Cqp, 64).with_pred_structure(PredStructure::RandomAccess);
    assert_eq!(p.gop.hierarchical_levels, 5);
    assert_eq!(p.gop.mini_gop_size, 32);
}

/// `rc == VBR || rc == CBR` short-circuits the random-access arm to 4.
#[test]
fn ra_auto_resolves_four_for_rate_controlled_modes() {
    for mode in [RcMode::Vbr, RcMode::Cbr] {
        let p = pipe(256, 256, 6, mode, 64).with_pred_structure(PredStructure::RandomAccess);
        assert_eq!(p.gop.hierarchical_levels, 4, "mode {mode:?}");
    }
}

/// `!(enc_mode <= ENC_M8)` on the random-access arm — presets above M8 take
/// 4 whatever the resolution.
#[test]
fn ra_auto_resolves_four_above_m8() {
    let p = pipe(256, 256, 9, RcMode::Cqp, 64).with_pred_structure(PredStructure::RandomAccess);
    assert_eq!(p.gop.hierarchical_levels, 4);
}

/// `input_resolution >= 1080p_RANGE && enc_mode >= ENC_M8` on the
/// random-access arm — the resolution and preset conditions are conjunctive.
#[test]
fn ra_auto_resolves_four_for_1080p_at_m8() {
    let p = pipe(1920, 1080, 8, RcMode::Cqp, 64).with_pred_structure(PredStructure::RandomAccess);
    assert_eq!(p.gop.hierarchical_levels, 4);
    // 1080p alone is not enough below M8.
    let p = pipe(1920, 1080, 6, RcMode::Cqp, 64).with_pred_structure(PredStructure::RandomAccess);
    assert_eq!(p.gop.hierarchical_levels, 5);
    // Resolution below 1080p cannot reach the conjunction either.
    let p = pipe(1280, 720, 8, RcMode::Cqp, 64).with_pred_structure(PredStructure::RandomAccess);
    assert_eq!(p.gop.hierarchical_levels, 5);
}

/// `allintra` (`intra_period == 1` here; `intra_period_length == 0 || avif
/// || pred_structure == ALL_INTRA` in C) clamps the resolved level to 2 —
/// applied AFTER the pred-structure table in C, so it wins over the RA arm.
#[test]
fn auto_allintra_clamps_to_two() {
    let p = pipe(256, 256, 6, RcMode::Cqp, 1).with_pred_structure(PredStructure::RandomAccess);
    assert_eq!(p.gop.hierarchical_levels, 2);
}

/// An explicit level is carried through untouched — the AUTO arm does not
/// run, matching C where only `== HIERARCHICAL_LEVELS_AUTO` is rewritten.
#[test]
fn explicit_level_is_not_resolved() {
    let rc = RcConfig {
        mode: RcMode::Cqp,
        qp: 40,
        ..RcConfig::default()
    };
    let p = EncodePipeline::new(256, 256, 6, rc, 2, 64)
        .with_chroma_420(true)
        .with_pred_structure(PredStructure::RandomAccess);
    assert_eq!(p.gop.hierarchical_levels, 2);
}

/// The low-delay arm resolves lazily at the first encode (no
/// `with_pred_structure` ever ran): `LOW_DELAY && !(CBR || enc_mode > M9)`
/// gives 3 — C's video default under low delay.
#[test]
fn ld_auto_resolves_three_at_first_encode() {
    const W: usize = 64;
    const H: usize = 64;
    let mut p = pipe(W as u32, H as u32, 6, RcMode::Cqp, 64);
    let (y, u, v) = (
        vec![0x80u8; W * H],
        vec![0x80u8; W * H / 4],
        vec![0x80u8; W * H / 4],
    );
    p.encode_frame_420(&y, &u, &v, W);
    assert_eq!(p.gop.hierarchical_levels, 3);
}
