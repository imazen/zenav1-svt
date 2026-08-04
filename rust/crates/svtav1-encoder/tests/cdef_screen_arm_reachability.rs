//! Reachability gate for the CDEF screen-content qp-strength arm
//! (`svt_pick_cdef_from_qp`, enc_cdef.c:837-844).
//!
//! `tests/c_parity_cdef_pick.rs` proves the arm's ARITHMETIC against the C
//! shim. This file proves the arm is actually WIRED: that a frame the ported
//! detector classifies as `sc_class5` at a `use_qp_strength` preset really
//! does reach a different frame-header CDEF strength, and that the encoder
//! produces a different bitstream because of it.
//!
//! Why preset 7: C's `use_qp_strength` needs `cdef_search_level == 10`, which
//! the all-intra derivation gives at M7+ (enc_mode_config.c:3543-3600), and
//! screen-content detection is force-disabled at M8+
//! (enc_handle.c:4641-4651, mirrored by `sc_detect::derive_allintra_sc`'s
//! `preset <= 7` gate). So M7 is exactly the default-config preset where the
//! screen arm is live.

use svtav1_encoder::cdef::pick_cdef_params_key_frame;
use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::rate_control::{RcConfig, RcMode};
use svtav1_encoder::sc_detect;

const W: usize = 128;
const H: usize = 128;

/// A 4px-period two-value checkerboard: every 8x8/16x16 block has exactly 2
/// colors and huge variance, so the AA-aware detector raises every class
/// including `sc_class5`. Same construction as
/// `c_parity_sc_detect::detector_classes_on_constructed_planes`.
fn screen_plane() -> Vec<u8> {
    let mut p = vec![0u8; W * H];
    for r in 0..H {
        for c in 0..W {
            p[r * W + c] = if ((r / 4) + (c / 4)) % 2 == 0 {
                16
            } else {
                240
            };
        }
    }
    p
}

/// A smooth gradient: photo/solid blocks only, every class false.
fn photo_plane() -> Vec<u8> {
    let mut p = vec![0u8; W * H];
    for r in 0..H {
        for c in 0..W {
            p[r * W + c] = ((r + c) / 2) as u8;
        }
    }
    p
}

/// The wiring input: at preset 7 the ported derivation must classify the
/// screen plane `sc_class5` and the photo plane not-`sc_class5`. Without
/// this, the arm below could never be selected and every downstream
/// assertion would be vacuous.
#[test]
fn preset7_derivation_separates_screen_from_photo() {
    let screen = screen_plane();
    let photo = photo_plane();
    assert!(
        sc_detect::derive_allintra_sc(7, &screen, W, W, H)
            .classes
            .sc_class5,
        "the checkerboard plane must be sc_class5 at preset 7"
    );
    assert!(
        !sc_detect::derive_allintra_sc(7, &photo, W, W, H)
            .classes
            .sc_class5,
        "the gradient plane must NOT be sc_class5 at preset 7"
    );
    // M8+ force-disables detection (enc_handle.c:4641-4651): the same plane
    // must fall back to the intra arm there.
    assert!(
        !sc_detect::derive_allintra_sc(8, &screen, W, W, H)
            .classes
            .sc_class5,
        "screen detection must be off at preset 8 (enc_handle.c:4641-4651)"
    );
}

/// The frame-header consequence at every qp this encoder can be driven with:
/// on an `sc_class5` frame the two arms must pick DIFFERENT cdef strengths
/// at nearly every operating point, so the flag is a real header divergence.
#[test]
fn screen_arm_changes_the_header_across_the_qp_range() {
    let mut differing_qps = 0usize;
    for qp in 0..=63u8 {
        let qindex = svtav1_encoder::rate_control::qp_to_qindex(qp);
        let screen = pick_cdef_params_key_frame(qindex, 8, true);
        let intra = pick_cdef_params_key_frame(qindex, 8, false);
        assert_eq!(
            screen.damping, intra.damping,
            "damping is arm-independent (CDEF_DAMPING_FROM_QP, enc_cdef.c:897)"
        );
        if (screen.y_strength, screen.uv_strength) != (intra.y_strength, intra.uv_strength) {
            differing_qps += 1;
        }
    }
    assert!(
        differing_qps >= 60,
        "the screen arm must change the signaled strengths at essentially every \
         qp (got {differing_qps}/64)"
    );
}

/// End-to-end: a preset-7 encode of the `sc_class5` plane must produce a
/// bitstream, and that bitstream must differ from the one the SAME encoder
/// produces for the same content when the frame is not screen-classified.
///
/// The two encodes here use IDENTICAL pixel data; the only difference is the
/// preset (7 = detection live -> screen arm; 8 = detection force-disabled ->
/// intra arm). A preset change moves other things too, so this test does not
/// attribute the whole delta to CDEF — it asserts the weaker, still
/// meaningful property that the screen-classified encode is reached, runs,
/// and produces a decodable-length stream. The per-arm byte attribution is
/// covered by `c_parity_cdef_pick.rs` plus the header assertion above.
///
/// MEASURED byte attribution for the wiring (2026-08-03, this exact cell,
/// by re-running the preset-7 encode with `pick_cdef_params_key_frame`'s
/// screen flag forced back to `false`): the stream stays 155 bytes and
/// exactly TWO bytes change, +20 (0xd3 -> 0xa2) and +21 (0x20 -> 0x60).
/// At the bit level that is the 12-bit `cdef_y_strength[0] ||
/// cdef_uv_strength[0]` pair at absolute bit offset 158, going from
/// `001101 001100` (y = 13, uv = 12 — the intra fit at qindex 160) to
/// `001010 001001` (y = 10, uv = 9 — the screen fit), i.e. precisely the
/// values `pick_cdef_params_key_frame(160, 8, {false,true})` returns. The
/// preset-8 stream is byte-identical across the same probe, confirming the
/// M8+ detection kill-switch.
#[test]
fn preset7_screen_encode_runs_and_differs_from_the_nonscreen_preset() {
    let y = screen_plane();
    let chroma = vec![128u8; (W / 2) * (H / 2)];
    let mut bytes = Vec::new();
    for preset in [7u8, 8] {
        let rc = RcConfig {
            mode: RcMode::Cqp,
            qp: 40,
            ..Default::default()
        };
        let mut p = EncodePipeline::new(W as u32, H as u32, preset, rc, 0, 1)
            .with_chroma_420(true)
            .with_thread_count(1);
        let obu = p.encode_frame_420(&y, &chroma, &chroma, W);
        assert!(!obu.is_empty(), "preset {preset} produced no bitstream");
        bytes.push(obu);
    }
    assert_ne!(
        bytes[0], bytes[1],
        "the screen-classified preset-7 encode must not coincide with the \
         detection-disabled preset-8 encode"
    );
}

/// THE WIRING GATE (added by the adversarial verification pass, 2026-08-03).
///
/// The three tests above do NOT observe the pipeline at all: two call
/// `pick_cdef_params_key_frame` / `derive_allintra_sc` directly, and the
/// third only asserts that a preset-7 encode differs from a preset-8 one —
/// which is true for a dozen unrelated reasons. MEASURED: replacing the
/// pipeline's `sc_derivation.classes.sc_class5` argument with a literal
/// `false` (i.e. deleting the wiring half of this port) left the ENTIRE
/// 951-test workspace green. Per `rust/CLAUDE.md` "Gate Discipline — a gate
/// that would pass without the feature is a DEFECT", that is a defect.
///
/// This test closes it by reading the strengths the pipeline actually wrote
/// into the frame header (`last_cdef_signaled`) and requiring them to be the
/// SCREEN arm's values on an `sc_class5` frame and the INTRA arm's on a
/// photo frame. Byte-count checks cannot see this: `cdef_y_strength[0]` and
/// `cdef_uv_strength[0]` are fixed-width header fields, so a wrong arm moves
/// no byte boundary (the measured probe kept the stream at 155 bytes and
/// changed exactly two bytes in place).
#[test]
fn pipeline_signals_the_screen_arm_on_an_sc_class5_frame() {
    let chroma = vec![128u8; (W / 2) * (H / 2)];
    let qindex = svtav1_encoder::rate_control::qp_to_qindex(40);
    let screen_expect = pick_cdef_params_key_frame(qindex, 8, true);
    let intra_expect = pick_cdef_params_key_frame(qindex, 8, false);
    // The cell is only meaningful if the two arms actually differ here.
    assert_ne!(
        (screen_expect.y_strength, screen_expect.uv_strength),
        (intra_expect.y_strength, intra_expect.uv_strength),
        "qp 40 (qindex {qindex}) must be a cell where the arms disagree, \
         else this gate is vacuous"
    );

    for (label, plane, expect, other) in [
        ("screen", screen_plane(), screen_expect, intra_expect),
        ("photo", photo_plane(), intra_expect, screen_expect),
    ] {
        let rc = RcConfig {
            mode: RcMode::Cqp,
            qp: 40,
            ..Default::default()
        };
        let mut p = EncodePipeline::new(W as u32, H as u32, 7, rc, 0, 1)
            .with_chroma_420(true)
            .with_thread_count(1);
        let obu = p.encode_frame_420(&plane, &chroma, &chroma, W);
        assert!(!obu.is_empty(), "{label} plane produced no bitstream");
        let got = p
            .last_cdef_signaled
            .expect("the pipeline must record the signaled CDEF strengths");
        assert_eq!(
            (got.y_strength, got.uv_strength),
            (expect.y_strength, expect.uv_strength),
            "the {label} frame must signal the {} arm's strengths at qindex \
             {qindex} (got {got:?}); if this fails, check that pipeline.rs \
             still passes `sc_derivation.classes.sc_class5` to \
             pick_cdef_params_key_frame (enc_cdef.c:913-918)",
            if label == "screen" { "SCREEN" } else { "INTRA" }
        );
        assert_ne!(
            (got.y_strength, got.uv_strength),
            (other.y_strength, other.uv_strength),
            "the {label} frame must NOT signal the other arm's strengths"
        );
    }
}
