//! CDF CONTINUATION vs the real C encoder — evidence TIER 1
//! (`docs/WORKING-ON-THIS.md` §4): every assertion here drives an exported
//! SVT-AV1 symbol through `zenav1-svt-cref`, not a transcribed table.
//!
//! What is under test is [`svtav1_encoder::port_frame_cdf::FrameCdfs`] — the
//! object a frame saves onto its reference slot and a later frame restores
//! from when its header names a `primary_ref_frame`. Two independent claims,
//! deliberately split:
//!
//! 1. **The starting state agrees.** The port's default `FrameContext` +
//!    `CoeffFc` must equal C's `svt_av1_default_coef_probs(qindex)` +
//!    `svt_aom_init_mode_probs`, field for field. A restore is only as good as
//!    the state it restores.
//! 2. **The counter reset agrees.** `svt_av1_reset_cdf_symbol_counters` zeroes
//!    `cdf[nsymbs]` of every CDF, and `nsymbs` is NOT always `len - 1`
//!    (`partition`, `uv_mode[0]`, `tx_size[0]` and the ext-tx sets all use a
//!    stride wider than their alphabet). Getting one wrong zeroes a
//!    PROBABILITY, which is a silent bitstream corruption on the next frame.
//!
//! Claim 2 is measured on a PAINTED context, never a default one: a default
//! context already has every counter at 0, so a defaults-only comparison
//! passes with the reset deleted. `the_reset_is_observable_at_all` is the
//! positive control that says so out loud.

use svtav1_cref::frame_cdf::{FctxMode, frame_ctx_field, frame_ctx_field_names};
use svtav1_encoder::entropy::coeff_c::CoeffFc;
use svtav1_encoder::entropy::context::FrameContext;
use svtav1_encoder::port_frame_cdf::FrameCdfs;

/// FRAME_CONTEXT fields C carries that this port has no storage for, with the
/// reason each is inert in every configuration this encoder can produce.
///
/// This list is ASSERTED to be exact, not just tolerated: a field that
/// disappears from the port without landing here, or a new C field nobody
/// noticed, fails `the_port_carries_every_c_field_except_the_named_gaps`.
const KNOWN_ABSENT: &[&str] = &[
    // `delta_lf_present` is never signalled by this encoder, so nothing ever
    // codes a delta_lf symbol and both tables stay at their defaults.
    "delta_lf",
    "delta_lf_multi",
    // The port codes `palette_uv_mode` (the flag) but never a UV palette, so
    // it never codes a UV palette SIZE or COLOR INDEX symbol.
    "palette_uv_size",
    "palette_uv_color_index",
];

fn port_state(qindex: u8) -> FrameCdfs {
    FrameCdfs {
        fc: FrameContext::new_default(),
        coeff: CoeffFc::default_for_qindex(qindex),
    }
}

fn port_field_names(qindex: u8) -> Vec<String> {
    let mut names = Vec::new();
    port_state(qindex).for_each_field(&mut |n, _| names.push(n.to_string()));
    names
}

#[test]
fn the_default_frame_context_matches_c_field_for_field() {
    // Four qindex buckets: `svt_av1_default_coef_probs` selects a different
    // coefficient table per bucket, so one qindex would test one quarter of
    // the coefficient CDFs. The bucket edges are 20/60/120 (cabac_context_model.c).
    for qindex in [10u8, 40, 90, 200] {
        let mut checked = 0usize;
        let mut state = port_state(qindex);
        state.for_each_field_mut(&mut |name, ours| {
            let theirs = frame_ctx_field(i32::from(qindex), FctxMode::Defaults, name)
                .unwrap_or_else(|| panic!("C has no FRAME_CONTEXT field named {name}"));
            assert_eq!(
                theirs.len(),
                ours.len(),
                "q{qindex} {name}: C has {} elements, the port has {}",
                theirs.len(),
                ours.len()
            );
            assert_eq!(
                theirs, ours,
                "q{qindex} {name}: default CDFs differ from C's"
            );
            checked += 1;
        });
        assert!(checked >= 90, "only {checked} fields compared at q{qindex}");
    }
}

#[test]
fn the_counter_reset_matches_c_field_for_field() {
    let qindex = 40u8;
    let mut painted = port_state(qindex);
    painted.for_each_field_mut(&mut |_, v| {
        for x in v.iter_mut() {
            // Byte 0x12 in both halves — the same bit pattern the shim paints
            // C's struct with, so the two sides start from the same place.
            *x = 0x1212;
        }
    });
    let mut reset = painted.clone();
    reset.reset_symbol_counters();

    let mut checked = 0usize;
    reset.for_each_field_mut(&mut |name, ours| {
        let theirs = frame_ctx_field(i32::from(qindex), FctxMode::PaintedReset, name)
            .unwrap_or_else(|| panic!("C has no FRAME_CONTEXT field named {name}"));
        assert_eq!(
            theirs, ours,
            "{name}: the counter reset differs from svt_av1_reset_cdf_symbol_counters"
        );
        checked += 1;
    });
    assert!(checked >= 90, "only {checked} fields compared");
}

/// POSITIVE CONTROL for the test above. If painting-then-resetting produced
/// the same bytes as painting alone, that test would pass against a
/// `reset_symbol_counters` that did nothing at all.
#[test]
fn the_reset_is_observable_at_all() {
    let mut moved = 0usize;
    for name in port_field_names(40) {
        let before = frame_ctx_field(40, FctxMode::Painted, &name).unwrap();
        let after = frame_ctx_field(40, FctxMode::PaintedReset, &name).unwrap();
        assert_ne!(
            before, after,
            "{name}: C's reset left it untouched — is it really in C's reset list?"
        );
        moved += 1;
    }
    assert!(moved >= 90, "only {moved} fields observed moving");
    // And the DEFAULT context really is already counter-free, which is why the
    // painted pair is the one the reset test uses.
    for name in ["partition", "coeff_base", "skip"] {
        assert_eq!(
            frame_ctx_field(40, FctxMode::Defaults, name),
            frame_ctx_field(40, FctxMode::DefaultsReset, name),
            "{name}: a default context should already have zero counters"
        );
    }
}

#[test]
fn the_port_carries_every_c_field_except_the_named_gaps() {
    let c_names = frame_ctx_field_names();
    assert!(
        c_names.len() >= 95,
        "the shim only enumerated {} C fields",
        c_names.len()
    );
    let ours = port_field_names(40);
    let missing: Vec<&String> = c_names.iter().filter(|n| !ours.contains(n)).collect();
    let mut missing_sorted: Vec<&str> = missing.iter().map(|s| s.as_str()).collect();
    missing_sorted.sort_unstable();
    let mut expected: Vec<&str> = KNOWN_ABSENT.to_vec();
    expected.sort_unstable();
    assert_eq!(
        missing_sorted, expected,
        "the set of C FRAME_CONTEXT fields the port does not carry has changed"
    );
    // And nothing the port carries is unknown to C.
    let extra: Vec<&String> = ours.iter().filter(|n| !c_names.contains(n)).collect();
    assert!(
        extra.is_empty(),
        "the port names fields C does not have: {extra:?}"
    );
}
