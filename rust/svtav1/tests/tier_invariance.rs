//! The encoder's OUTPUT BYTES must not depend on which SIMD tier ran.
//!
//! # Why this exists
//!
//! Every byte-parity gate in this repo compares the port against the C encoder
//! on ONE host, using whatever kernels that host's CPU dispatches to. That
//! silently assumes the port emits the same bitstream on every ISA. Nothing
//! tested the assumption.
//!
//! It stopped being hypothetical on 2026-08-04. `screen_palette_bd_gate.sh`
//! carries two cells pinned as known-diff (`screen 64/128 q55 p7 bd10`). On the
//! x86-64 CI runner they MATCHED — the gate's self-promoting pin correctly
//! failed the build demanding they be promoted — while on the aarch64 dev host
//! they still DIFFER, reproducing the pinned byte counts exactly (C=117 port=119
//! and C=350 port=356). One of the two encoders is ISA-dependent, and the two
//! possibilities have completely different severities:
//!
//!   - **C is ISA-dependent.** Entirely possible and not our bug: C's `_c` and
//!     SIMD kernels genuinely disagree (`svt_aom_hadamard_32x32_c` vs `_avx2`
//!     at bd10 magnitudes, pinned in `c_parity_hadamard.rs`, documented as
//!     entry #6 of `docs/SUSPECTED-C-BUGS.md`). Then "byte-identical to C" is
//!     itself a per-ISA statement and the PIN LIST must be ISA-scoped.
//!   - **The port is ISA-dependent.** A shipping bug of the first order. Five
//!     of the last six commits added aarch64/NEON kernels, and `CLAUDE.md`
//!     records that 31 dispatch tests pin to the TOP tier only — so a NEON
//!     kernel that is not bit-exact with its scalar twin has a real place to
//!     hide.
//!
//! This test settles the second question on ANY host, without needing a machine
//! of the other architecture: `for_each_token_permutation` walks every dispatch
//! tier the build supports (on aarch64: NEON and scalar; on x86-64: AVX2, SSE4,
//! scalar) and we assert the encoded OBU is byte-identical across all of them.
//!
//! A green run here means the port emits one bitstream per input regardless of
//! tier, which leaves C's ISA-dependence as the explanation for a cell that
//! matches on one runner and not another. A RED run localizes the divergence to
//! a specific tier, which is strictly more actionable than any cross-machine
//! byte diff could be.
//!
//! # Why end-to-end rather than per-kernel
//!
//! The existing `for_each_token_permutation` sites test kernels in isolation
//! against C. That is the right gate for a kernel, and it is not this gate: it
//! cannot catch a kernel with NO differential coverage, a dispatch site that
//! selects the wrong tier, or a tier difference that only manifests through the
//! RD decisions built on top of the kernel. Bytes out of the whole encoder is
//! the property that actually matters, and it is the one the parity gates
//! assume.
//!
//! # Cost
//!
//! Small frames, few cells — this runs in seconds and belongs in the default
//! test run rather than behind an env var, because a gate nobody runs is a gate
//! that does not exist.

use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::rate_control::{RcConfig, RcMode};

fn cqp(qp: u8) -> RcConfig {
    RcConfig { mode: RcMode::Cqp, qp, ..RcConfig::default() }
}

/// Deterministic screen-like content — the same generator `identity_run`'s
/// `screen` content uses. Palette-friendly (a handful of distinct values per
/// 8x8 block) so the cells below actually reach the palette/screen-content
/// paths, which is where the divergence under investigation lives.
///
/// A tier-invariance test on flat content would pass no matter how broken a
/// kernel was, which is the same vacuity trap `docs/WORKING-ON-THIS.md` §5
/// describes: a gate that cannot reach a feature cannot guard it.
fn screen_plane(w: usize, h: usize) -> Vec<u8> {
    let mut p = vec![0u8; w * h];
    for r in 0..h {
        for c in 0..w {
            let panel = ((r / 24) & 1) as u8 * 2 + ((c / 32) & 1) as u8;
            let bg = [35u8, 110, 180, 235][panel as usize];
            let text_row = (r % 24) >= 6 && (r % 24) < 12;
            let glyph = (c / 3 + r / 24) % 5 != 0;
            p[r * w + c] = if text_row && glyph { 16 } else { bg };
        }
    }
    p
}

/// A gradient plane — photographic in character, so it exercises the transform
/// and intra-prediction kernels rather than the palette path.
fn gradient_plane(w: usize, h: usize) -> Vec<u8> {
    let mut p = vec![0u8; w * h];
    for r in 0..h {
        for c in 0..w {
            p[r * w + c] = ((r * 3 + c * 5) % 256) as u8;
        }
    }
    p
}

fn encode_8bit(plane: &dyn Fn(usize, usize) -> Vec<u8>, w: usize, h: usize, qp: u8, preset: u8) -> Vec<u8> {
    let y = plane(w, h);
    let (cw, ch) = ((w + 1) / 2, (h + 1) / 2);
    let u = vec![128u8; cw * ch];
    let v = vec![128u8; cw * ch];
    let rc = cqp(qp);
    let mut pipe = EncodePipeline::new(w as u32, h as u32, preset, rc, 0, 1).with_chroma_420(true);
    pipe.try_encode_frame_420(&y, &u, &v, w)
        .expect("in-envelope cell must encode")
}

fn encode_10bit(plane: &dyn Fn(usize, usize) -> Vec<u8>, w: usize, h: usize, qp: u8, preset: u8) -> Vec<u8> {
    // Widen the 8-bit generator to 10-bit the same way the harness does, so the
    // content is the identical picture at a higher depth.
    let y: Vec<u16> = plane(w, h).iter().map(|&s| (s as u16) << 2).collect();
    let (cw, ch) = ((w + 1) / 2, (h + 1) / 2);
    let u = vec![512u16; cw * ch];
    let v = vec![512u16; cw * ch];
    let rc = cqp(qp);
    let mut pipe = EncodePipeline::new(w as u32, h as u32, preset, rc, 0, 1)
        .with_chroma_420(true)
        .with_bit_depth(10);
    pipe.try_encode_frame_420_hbd(&y, &u, &v, w)
        .expect("in-envelope cell must encode")
}

/// Run `f` under every dispatch tier this build supports and assert all tiers
/// produce the same bytes. Returns the byte length so the caller can assert the
/// cell was non-trivial (a zero-byte "match" proves nothing).
fn assert_tier_invariant(label: &str, f: impl Fn() -> Vec<u8>) -> usize {
    let mut baseline: Option<Vec<u8>> = None;
    let mut tiers = 0usize;
    let report = archmage::testing::for_each_token_permutation(
        archmage::testing::CompileTimePolicy::WarnStderr,
        |_tok| {
            let got = f();
            tiers += 1;
            match &baseline {
                None => baseline = Some(got),
                Some(want) => {
                    assert_eq!(
                        want.len(),
                        got.len(),
                        "{label}: SIMD tier changed the ENCODED LENGTH \
                         ({} -> {}). The port must emit one bitstream per input \
                         regardless of dispatch tier; a tier-dependent encoder \
                         makes every byte-parity gate a per-host statement.",
                        want.len(),
                        got.len()
                    );
                    let first = want.iter().zip(got.iter()).position(|(a, b)| a != b);
                    assert!(
                        first.is_none(),
                        "{label}: SIMD tier changed the ENCODED BYTES at offset {} \
                         (same length {}). Some kernel is not bit-exact with its \
                         twin on another tier -- localize by running this test \
                         with only one tier disabled at a time.",
                        first.unwrap(),
                        want.len()
                    );
                }
            }
        },
    );
    let _ = report;
    assert!(
        tiers >= 1,
        "{label}: for_each_token_permutation ran ZERO tiers -- the harness did \
         not fire, so a green result here would mean nothing (see \
         docs/WORKING-ON-THIS.md §5: a silent harness and a genuine absence are \
         indistinguishable)."
    );
    baseline.map(|b| b.len()).unwrap_or(0)
}

/// The exact cells whose verdict differed between the x86-64 CI runner and the
/// aarch64 dev host. If the port is the ISA-dependent side, this is where it
/// shows.
#[test]
fn bd10_screen_q55_p7_is_tier_invariant() {
    let n64 = assert_tier_invariant("screen 64x64 q55 p7 bd10", || {
        encode_10bit(&screen_plane, 64, 64, 55, 7)
    });
    let n128 = assert_tier_invariant("screen 128x128 q55 p7 bd10", || {
        encode_10bit(&screen_plane, 128, 128, 55, 7)
    });
    // Anti-vacuity: these cells encode real content, not an empty stream.
    assert!(n64 > 32, "64x64 bd10 screen cell produced {n64}B -- too small to be real");
    assert!(n128 > 64, "128x128 bd10 screen cell produced {n128}B -- too small to be real");
}

/// Broader coverage across depth, content class and preset, so a tier bug in a
/// kernel these two cells happen not to reach still gets caught. Presets are
/// chosen to span the distinct search paths: 0 (full search), 6 (leaf funnel),
/// 7/8 (LVL_1, NSQ disabled), 10 (LPD0).
#[test]
fn encoder_output_is_tier_invariant_across_the_matrix() {
    for &(w, h) in &[(64usize, 64usize), (128, 128)] {
        for &qp in &[20u8, 55] {
            for &preset in &[0u8, 6, 7, 10] {
                assert_tier_invariant(&format!("gradient {w}x{h} q{qp} p{preset} bd8"), || {
                    encode_8bit(&gradient_plane, w, h, qp, preset)
                });
                assert_tier_invariant(&format!("screen {w}x{h} q{qp} p{preset} bd8"), || {
                    encode_8bit(&screen_plane, w, h, qp, preset)
                });
                assert_tier_invariant(&format!("gradient {w}x{h} q{qp} p{preset} bd10"), || {
                    encode_10bit(&gradient_plane, w, h, qp, preset)
                });
                assert_tier_invariant(&format!("screen {w}x{h} q{qp} p{preset} bd10"), || {
                    encode_10bit(&screen_plane, w, h, qp, preset)
                });
            }
        }
    }
}

/// Partial-superblock geometry: the edge rules run different code than the
/// aligned path, and a tier bug there would be invisible to the cells above.
#[test]
fn partial_superblock_output_is_tier_invariant() {
    // Presets 0..5 run the EDGE-AWARE PD1 REFINEMENT WALK on a partial SB — a
    // path that did not exist until 2026-08-04 and had no tier coverage. It is
    // also where a cross-host disagreement showed up: `gradient 96x80 q48 p0`
    // byte-matches C on aarch64 and fails the same gate on the x86-64 runner.
    // Either the port is tier-dependent there (a shipping bug) or C is (entry
    // #9 of docs/SUSPECTED-C-BUGS.md), and only this gate can tell them apart
    // without a machine of the other architecture.
    for &(w, h) in &[(96usize, 80usize), (65, 65), (120, 104), (72, 88), (104, 72)] {
        for &preset in &[0u8, 1, 2, 3, 4, 5, 6, 7] {
            for &qp in &[32u8, 48] {
                assert_tier_invariant(&format!("gradient {w}x{h} q{qp} p{preset} bd8"), || {
                    encode_8bit(&gradient_plane, w, h, qp, preset)
                });
            }
        }
    }
}
