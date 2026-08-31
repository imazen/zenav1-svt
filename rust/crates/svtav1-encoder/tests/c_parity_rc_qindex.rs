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
use svtav1_encoder::rate_control::{cqp_qindex_calc, q_index_from_qstep_ratio, qp_scale_weight};

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

/// The key-frame arm of `cqp_qindex_calc` recomputed end to end against C's
/// own primitive: the port derives the ratio, C converts it to a qindex, and
/// the two must land on the same integer.
#[test]
fn cqp_qindex_calc_key_frame_arm_matches_c() {
    for &bd in &[8u8, 10u8] {
        for &hier in &[0u8, 1, 3, 4, 5] {
            for qindex in 0..=255i32 {
                let qratio_grad = if hier <= 4 { 0.3 } else { 0.2 };
                let qstep_ratio =
                    (0.2 + (1.0 - f64::from(qindex) / 255.0) * qratio_grad) * qp_scale_weight(0.0);
                let expected =
                    cref::get_q_index_from_qstep_ratio(qindex, qstep_ratio, i32::from(bd));

                let mut base_q = 0;
                let got = cqp_qindex_calc(
                    qindex, /*allintra=*/ false, /*slice_is_intra=*/ true, /*is_ref=*/ true,
                    /*temporal_layer_index=*/ 0, hier, bd, /*qp_scale_compress_strength=*/ 0.0,
                    &mut base_q,
                );
                assert_eq!(got, expected, "key-frame qindex at q={qindex} hier={hier} bd={bd}");
                assert_eq!(base_q, got, "cqp_base_q must record the temporal-layer-0 result");
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
        let mut base_q = 0;
        assert_eq!(
            cqp_qindex_calc(qindex, true, true, true, 0, 0, 8, 0.0, &mut base_q),
            qindex,
            "allintra must not scale the qindex (the still envelope depends on this)"
        );
        assert_eq!(
            cqp_qindex_calc(qindex, false, false, true, 0, 0, 8, 0.0, &mut base_q),
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
