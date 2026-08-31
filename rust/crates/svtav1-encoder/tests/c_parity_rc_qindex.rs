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

/// `FrameContext` must carry C's REAL inter CDFs, not uniform placeholders.
///
/// Seven of these fields were `[CDF_PROB_TOP / 2, 0, 0]` — a uniform table,
/// which codes every symbol at even odds — and twelve more had no field at
/// all. Byte-inert while inter frames are refused at the pipeline entry
/// point, and a tile desync the moment one is coded.
///
/// The comparison is against `port_entropy_inter::cdfs`, whose tables were
/// EXTRACTED from the real `svt_aom_init_mode_probs` rather than transcribed
/// (tier 1, `c_parity_entropy_inter.rs`), so this asserts that `FrameContext`
/// and that tier-1-gated source cannot drift apart.
#[test]
fn frame_context_carries_the_real_inter_cdfs() {
    use svtav1_encoder::entropy::context::FrameContext;
    use svtav1_encoder::port_entropy_inter::cdfs;

    let fc = FrameContext::new_default();

    assert_eq!(fc.skip_mode_cdf, cdfs::SKIP_MODE_CDF, "skip_mode");
    assert_eq!(fc.newmv_cdf, cdfs::NEWMV_CDF, "newmv");
    assert_eq!(fc.globalmv_cdf, cdfs::ZEROMV_CDF, "globalmv/zeromv");
    assert_eq!(fc.refmv_cdf, cdfs::REFMV_CDF, "refmv");
    assert_eq!(fc.drl_cdf, cdfs::DRL_CDF, "drl");
    assert_eq!(
        fc.inter_compound_mode_cdf, cdfs::INTER_COMPOUND_MODE_CDF,
        "inter_compound_mode"
    );
    assert_eq!(
        fc.interp_filter_cdf, cdfs::SWITCHABLE_INTERP_CDF,
        "interp_filter/switchable_interp"
    );

    // The twelve that had no field at all now live on the carrier.
    let inter = &fc.inter;
    assert_eq!(inter.comp_ref_type_cdf, cdfs::COMP_REF_TYPE_CDF);
    assert_eq!(inter.uni_comp_ref_cdf, cdfs::UNI_COMP_REF_CDF);
    assert_eq!(inter.comp_bwdref_cdf, cdfs::COMP_BWDREF_CDF);
    assert_eq!(inter.motion_mode_cdf, cdfs::MOTION_MODE_CDF);
    assert_eq!(inter.obmc_cdf, cdfs::OBMC_CDF);
    assert_eq!(inter.compound_index_cdf, cdfs::COMPOUND_INDEX_CDF);
    assert_eq!(inter.comp_group_idx_cdf, cdfs::COMP_GROUP_IDX_CDF);
    assert_eq!(inter.interintra_cdf, cdfs::INTERINTRA_CDF);
    assert_eq!(inter.interintra_mode_cdf, cdfs::INTERINTRA_MODE_CDF);
    assert_eq!(inter.wedge_interintra_cdf, cdfs::WEDGE_INTERINTRA_CDF);
    assert_eq!(inter.wedge_idx_cdf, cdfs::WEDGE_IDX_CDF);
    assert_eq!(inter.compound_type_cdf, cdfs::COMPOUND_TYPE_CDF);
}

/// Anti-vacuity for the test above: a uniform table would PASS it if the
/// source tables were themselves uniform. They are not — assert that each
/// seeded table actually varies across its rows, which is exactly the
/// property the placeholders lacked.
#[test]
fn the_seeded_inter_cdfs_are_not_uniform() {
    use svtav1_encoder::entropy::context::FrameContext;
    let fc = FrameContext::new_default();

    // A uniform 3-wide CDF is [16384, 0, 0] at every row; a real one varies.
    let rows_vary = |rows: &[[u16; 3]]| rows.iter().any(|r| r[0] != rows[0][0]);
    assert!(rows_vary(&fc.newmv_cdf), "newmv rows must differ");
    assert!(rows_vary(&fc.refmv_cdf), "refmv rows must differ");
    assert!(rows_vary(&fc.drl_cdf), "drl rows must differ");
    assert!(rows_vary(&fc.skip_mode_cdf), "skip_mode rows must differ");
    assert!(
        fc.inter_compound_mode_cdf[0][3] != 0,
        "the 9-wide compound table must use entries past the old 5-wide bound"
    );
}
