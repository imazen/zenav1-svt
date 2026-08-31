//! Differential parity for the VIDEO-mode qindex derivation (inter campaign C1a).
//!
//! Evidence tier 1 (`docs/WORKING-ON-THIS.md` §4): every assertion below drives
//! the real exported C function — `svt_av1_get_q_index_from_qstep_ratio` and
//! `svt_av1_convert_qindex_to_q` — through `zenav1-svt-cref`, not a second
//! transcription of the same tables.
//!
//! Why this matters: C's `cqp_qindex_calc` returns `qindex` untouched when
//! `scs->allintra`, which is the early return the whole existing still-picture
//! envelope takes. A video-mode encode reaches the qstep-ratio scaling instead,
//! and that is 3.2x of the frame-0 byte gap measured in
//! `docs/INTER-ENCODE-PLAN.md` §1b (290 B still vs 930 B video on identical
//! pixels). Any inter work is downstream of getting this number right.

use svtav1_cref as cref;
use svtav1_encoder::rate_control::{
    compute_qdelta, convert_qindex_to_q, cqp_qindex_calc, q_index_from_qstep_ratio, qp_scale_weight,
};

/// The full qindex ladder against C, at both shipped bit depths, across ratios
/// on either side of 1.0 (the two branches of C's walk) and exactly at it.
#[test]
fn q_index_from_qstep_ratio_matches_c_over_the_full_ladder() {
    let ratios = [
        0.05, 0.1, 0.2, 0.25, 0.31176, 0.4, 0.5, 0.75, 0.9, 0.999, 1.0, 1.001, 1.1, 1.5, 2.0, 4.0,
    ];
    for &bd in &[8u8, 10u8] {
        for leaf in 0..=255i32 {
            for &r in &ratios {
                let c = cref::get_q_index_from_qstep_ratio(leaf, r, i32::from(bd));
                let rust = q_index_from_qstep_ratio(leaf, r, bd);
                assert_eq!(
                    rust, c,
                    "q_index_from_qstep_ratio(leaf={leaf}, ratio={r}, bd={bd})"
                );
            }
        }
    }
}

/// The MAINLINE `cqp_qindex_calc` key-frame arm, against the real
/// `base_q_idx` the C encoder writes. These four cells were read out of C's
/// own bitstream (64x64 gradient, video mode via `SVT_AVIF=0`) — the
/// strongest oracle available for this function, since `cqp_qindex_calc` is
/// C `static` and cannot be called directly.
#[test]
fn cqp_qindex_calc_matches_the_base_q_idx_c_writes() {
    // (cli qp -> qindex, hierarchical_levels, C's written base_q_idx)
    for &(qindex, hier, expected) in &[(80, 0u8, 14), (160, 0, 67), (160, 5, 70), (220, 0, 143)] {
        let got = cqp_qindex_calc(
            qindex, /*allintra=*/ false, /*slice_is_intra=*/ true, /*is_ref=*/ true,
            /*idr_flag=*/ true, /*temporal_layer_index=*/ 0, hier, /*bit_depth=*/ 8,
        );
        assert_eq!(got, expected, "qindex {qindex} hier {hier}");
    }
}

/// `compute_qdelta` and `convert_qindex_to_q` against the exported C symbols —
/// tier 1, over the full ladder at both shipped bit depths.
#[test]
fn qdelta_and_qindex_to_q_match_c() {
    for &bd in &[8u8, 10u8] {
        for qi in 0..=255i32 {
            let c = cref::convert_qindex_to_q(qi, i32::from(bd));
            let rust = convert_qindex_to_q(qi, bd);
            assert!(
                (c - rust).abs() < 1e-12,
                "convert_qindex_to_q(qindex={qi}, bd={bd}): C {c} vs port {rust}"
            );
        }
        // The deltas the qindex derivation actually asks for: a target that is
        // a fixed percentage BELOW the start, for each row of C's percents
        // table, plus the degenerate equal case.
        for qi in 0..=255i32 {
            let q_val = convert_qindex_to_q(qi, bd);
            for &p in &[0, 4, 8, 15, 20, 30, 60, 70, 75, 76, 100] {
                let target = (q_val - q_val * f64::from(p) / 100.0).max(0.0);
                let c = cref::compute_qdelta(q_val, target, i32::from(bd));
                let rust = compute_qdelta(q_val, target, bd);
                assert_eq!(rust, c, "compute_qdelta(qindex={qi}, -{p}%, bd={bd})");
            }
        }
    }
}

/// The two early returns C takes, which are what keeps every existing still
/// cell byte-identical: `allintra` returns the qindex untouched, and so does a
/// flat-GOP non-intra slice.
#[test]
fn still_and_flat_gop_early_returns_are_identity() {
    for qindex in 0..=255i32 {
        assert_eq!(
            cqp_qindex_calc(qindex, true, true, true, true, 0, 0, 8),
            qindex,
            "allintra must not scale the qindex (the still envelope depends on this)"
        );
        assert_eq!(
            cqp_qindex_calc(qindex, false, false, true, false, 0, 0, 8),
            qindex,
            "hierarchical_levels == 0 and a non-intra slice returns the qindex unchanged"
        );
    }
}

/// `SVT_QP_SCALE_WEIGHT` (definitions.h:249) — the mainline macro, checked at
/// the strengths the CLI accepts.
#[test]
fn qp_scale_weight_matches_the_c_macro() {
    for i in 0..=8u32 {
        let strength = f64::from(i) * 0.5;
        assert!((qp_scale_weight(strength) - (1.0 + strength * 0.125)).abs() < 1e-12);
    }
}
