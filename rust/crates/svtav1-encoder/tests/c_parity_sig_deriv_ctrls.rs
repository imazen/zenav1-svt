//! Differential parity for the `level -> controls` tables of
//! `Source/Lib/Codec/enc_mode_config.c`.
//!
//! **Evidence tier 1** for the five EXPORTED setters (`svt_aom_set_wm_controls`,
//! `svt_aom_set_bipred3x3_controls`,
//! `svt_aom_set_dist_based_ref_pruning_controls`,
//! `svt_aom_md_pme_search_controls`, `svt_aom_set_gm_controls`): the shim calls
//! the real symbol on a ZEROED `ModeDecisionContext` /
//! `PictureParentControlSet` and copies the struct out, so the comparison
//! covers the fields a C arm leaves untouched as well as the ones it writes.
//!
//! The three `static` tables in this module (`set_obmc_controls`,
//! `set_inter_comp_controls`, `set_inter_intra_ctrls`,
//! `set_interpolation_search_level_ctrls`) have no exported symbol, so they are
//! **tier 4** — hand-derived vectors traced against the C source — and are
//! marked as such at each test.

use svtav1_cref::sig_deriv as cref;
use svtav1_encoder::port_enc_mode_config::ResolutionRange;
use svtav1_encoder::port_enc_mode_config::ctrls;

const RESOLUTIONS: [ResolutionRange; 7] = [
    ResolutionRange::R240p,
    ResolutionRange::R360p,
    ResolutionRange::R480p,
    ResolutionRange::R720p,
    ResolutionRange::R1080p,
    ResolutionRange::R4k,
    ResolutionRange::R8k,
];

#[test]
fn wm_controls_match_c() {
    for level in 0u8..=ctrls::MAX_WARP_LVL {
        let ours = ctrls::set_wm_controls(level).expect("level in range");
        let theirs = cref::set_wm_controls(level);
        assert_eq!(
            [
                u32::from(ours.enabled),
                u32::from(ours.use_wm_for_mvp),
                u32::from(ours.refinement_iterations),
                u32::from(ours.refine_diag),
                u32::from(ours.refine_level),
                u32::from(ours.lower_band_th),
                u32::from(ours.upper_band_th),
                u32::from(ours.shut_approx_if_not_mds0),
            ],
            theirs,
            "wm_level={level}"
        );
    }
    // Out-of-range is C's `assert(0)`; the port refuses rather than inventing a
    // plausible control set.
    assert!(ctrls::set_wm_controls(ctrls::MAX_WARP_LVL + 1).is_none());
}

#[test]
fn bipred3x3_controls_match_c() {
    for level in 0u8..=4 {
        let ours = ctrls::set_bipred3x3_controls(level).expect("level in range");
        let theirs = cref::set_bipred3x3_controls(level);
        assert_eq!(
            [
                u32::from(ours.enabled),
                u32::from(ours.search_diag),
                u32::from(ours.use_best_list),
                u32::from(ours.use_l0_l1_dev),
            ],
            theirs,
            "bipred3x3 level={level}"
        );
    }
    assert!(ctrls::set_bipred3x3_controls(5).is_none());
}

#[test]
fn dist_based_ref_pruning_controls_match_c() {
    for level in 0u8..=8 {
        let ours = ctrls::set_dist_based_ref_pruning_controls(level).expect("level in range");
        let theirs = cref::set_dist_based_ref_pruning_controls(level);
        let mut flat = [0u32; 25];
        flat[0] = u32::from(ours.enabled);
        flat[1] = u32::from(ours.use_tpl_info_offset);
        flat[2] = u32::from(ours.check_closest_multiplier);
        for i in 0..ctrls::TOT_INTER_GROUP {
            flat[3 + i] = ours.max_dev_to_best[i];
            flat[14 + i] = u32::from(ours.closest_refs[i]);
        }
        assert_eq!(flat, theirs, "ref-pruning level={level}");
    }
    assert!(ctrls::set_dist_based_ref_pruning_controls(9).is_none());
}

/// A positive control: level 8 must cap almost every group at 0 while leaving
/// GLOBAL uncapped. Without this the sweep above could pass with both sides
/// producing an all-zero struct.
#[test]
fn dist_based_ref_pruning_positive_control() {
    let c8 = cref::set_dist_based_ref_pruning_controls(8);
    assert_eq!(c8[0], 1, "level 8 must be enabled");
    assert_eq!(
        c8[3 + ctrls::inter_cand_group::GLOBAL],
        u32::MAX,
        "GLOBAL_GROUP is uncapped at every level"
    );
    assert_eq!(c8[3 + ctrls::inter_cand_group::PA_ME], 0);
    let c1 = cref::set_dist_based_ref_pruning_controls(1);
    assert_eq!(c1[3 + ctrls::inter_cand_group::PA_ME], u32::MAX);
}

#[test]
fn md_pme_search_controls_match_c() {
    for level in 0u8..=5 {
        let ours = ctrls::md_pme_search_controls(level).expect("level in range");
        let theirs = cref::md_pme_search_controls(level);
        assert_eq!(
            [
                i32::from(ours.enabled),
                ours.dist_type as i32,
                i32::from(ours.full_pel_search_width),
                i32::from(ours.full_pel_search_height),
                ours.early_check_mv_th_multiplier,
                ours.pre_fp_pme_to_me_cost_th,
                ours.pre_fp_pme_to_me_mv_th,
                ours.post_fp_pme_to_me_cost_th,
                ours.post_fp_pme_to_me_mv_th,
                i32::from(ours.enable_psad),
                i32::from(ours.sa_q_weight),
            ],
            theirs,
            "md_pme level={level}"
        );
    }
    assert!(ctrls::md_pme_search_controls(6).is_none());
}

/// Positive control for the MIN/MAX_SIGNED_VALUE transcription: level 1 must
/// carry `i32::MIN` in the mv thresholds and `i32::MAX` in the cost ones.
#[test]
fn md_pme_signed_sentinels_positive_control() {
    let c1 = cref::md_pme_search_controls(1);
    assert_eq!(c1[4], i32::MIN, "early_check_mv_th_multiplier");
    assert_eq!(c1[5], i32::MAX, "pre_fp_pme_to_me_cost_th");
    assert_eq!(c1[6], i32::MIN, "pre_fp_pme_to_me_mv_th");
}

#[test]
fn gm_controls_match_c() {
    for level in 0u8..=4 {
        for &r in &RESOLUTIONS {
            let ours = ctrls::set_gm_controls(level, r).expect("level in range");
            let theirs = cref::set_gm_controls(level, r.as_u8());
            assert_eq!(
                [
                    u32::from(ours.enabled),
                    u32::from(ours.identiy_exit),
                    u32::from(ours.search_start_model),
                    u32::from(ours.search_end_model),
                    u32::from(ours.skip_identity),
                    u32::from(ours.bypass_based_on_me),
                    u32::from(ours.params_refinement_steps),
                    u32::from(ours.downsample_level),
                    u32::from(ours.corners),
                    u32::from(ours.chess_rfn),
                    u32::from(ours.match_sz),
                    u32::from(ours.inj_psq_glb),
                    u32::from(ours.pp_enabled),
                    u32::from(ours.ref_idx0_only),
                    u32::from(ours.rfn_early_exit),
                    u32::from(ours.correspondence_method),
                ],
                theirs,
                "gm_level={level} res={r:?}"
            );
        }
    }
    assert!(ctrls::set_gm_controls(5, ResolutionRange::R1080p).is_none());
}

/// Positive control for the resolution-dependent correspondence method: level 3
/// must pick MV_8x8 at 480p and below, MV_16x16 up to 1080p, MV_32x32 above.
#[test]
fn gm_correspondence_method_positive_control() {
    use ctrls::correspondence_method as cm;
    assert_eq!(
        cref::set_gm_controls(3, ResolutionRange::R480p.as_u8())[15],
        u32::from(cm::MV_8X8)
    );
    assert_eq!(
        cref::set_gm_controls(3, ResolutionRange::R1080p.as_u8())[15],
        u32::from(cm::MV_16X16)
    );
    assert_eq!(
        cref::set_gm_controls(3, ResolutionRange::R4k.as_u8())[15],
        u32::from(cm::MV_32X32)
    );
    assert_eq!(
        cref::set_gm_controls(1, ResolutionRange::R4k.as_u8())[15],
        u32::from(cm::CORNERS)
    );
}

// ---------------------------------------------------------------------------
// TIER 4 — file-`static` in C, no exported symbol to differential against.
// Vectors below are read off the C source at the cited line and are the WEAKEST
// evidence tier (WORKING-ON-THIS.md 4).
// ---------------------------------------------------------------------------

/// TIER 4. `set_obmc_controls` (`enc_mode_config.c:2878`).
///
/// Note C's `default:` arm here is NOT `assert(0)` — it clears `enabled` — so
/// the port must return a value for every `u8`, which is asserted at the end.
#[test]
fn obmc_controls_traced_vectors() {
    /// `(level, enabled, max_blk_size_to_refine, max_blk_size, refine_level,
    /// trans_face_off, fpel_search_range, fpel_search_diag)`
    type ObmcVec = (u8, u8, u8, u8, u8, u8, u8, u8);
    let expect: [ObmcVec; 7] = [
        (0, 0, 0, 0, 0, 0, 0, 0),
        (1, 1, 128, 128, 0, 0, 16, 1),
        (2, 1, 64, 128, 1, 0, 16, 1),
        (3, 1, 32, 128, 1, 0, 8, 0),
        (4, 1, 32, 128, 1, 1, 16, 1),
        (5, 1, 32, 32, 4, 1, 8, 0),
        (6, 1, 16, 16, 4, 1, 8, 0),
    ];
    for &(lvl, en, mbr, mbs, rl, tfo, fsr, fsd) in &expect {
        let c = ctrls::set_obmc_controls(lvl);
        assert_eq!(
            (
                c.enabled,
                c.max_blk_size_to_refine,
                c.max_blk_size,
                c.refine_level,
                c.trans_face_off,
                c.fpel_search_range,
                c.fpel_search_diag
            ),
            (en, mbr, mbs, rl, tfo, fsr, fsd),
            "obmc level={lvl}"
        );
    }
    // C's default arm: any other level disables OBMC rather than asserting.
    for lvl in 7u8..=255 {
        assert_eq!(ctrls::set_obmc_controls(lvl).enabled, 0, "obmc level={lvl}");
    }
}

/// TIER 4. `set_inter_comp_controls` (`enc_mode_config.c:2589`).
#[test]
fn inter_comp_controls_traced_vectors() {
    let l0 = ctrls::set_inter_comp_controls(0).expect("level 0");
    assert_eq!(l0.tot_comp_types, ctrls::md_comp::DIST);
    assert!(!l0.do_me && !l0.do_pme && !l0.do_global);
    // Level 0 does NOT write the five trailing fields; they stay zeroed.
    assert_eq!(l0.pred0_to_pred1_mult, 0);
    assert_eq!(l0.max_mv_length, 0);
    assert!(!l0.skip_on_ref_info && !l0.use_rate && !l0.no_sym_dist);

    let l1 = ctrls::set_inter_comp_controls(1).expect("level 1");
    assert_eq!(l1.tot_comp_types, ctrls::md_comp::TYPES);
    assert!(l1.do_nearest_near_new && l1.do_3x3_bi && l1.use_rate);
    assert_eq!(l1.pred0_to_pred1_mult, 0);

    let l2 = ctrls::set_inter_comp_controls(2).expect("level 2");
    assert!(l2.do_pme && !l2.do_nearest_near_new && !l2.use_rate);
    assert_eq!(l2.pred0_to_pred1_mult, 1);
    assert!(!l2.skip_on_ref_info && !l2.no_sym_dist);

    let l3 = ctrls::set_inter_comp_controls(3).expect("level 3");
    assert!(!l3.do_pme && l3.skip_on_ref_info && l3.no_sym_dist);
    assert_eq!((l3.pred0_to_pred1_mult, l3.max_mv_length), (1, 0));

    let l4 = ctrls::set_inter_comp_controls(4).expect("level 4");
    assert_eq!((l4.pred0_to_pred1_mult, l4.max_mv_length), (4, 32));
    assert!(l4.skip_on_ref_info && l4.no_sym_dist && !l4.use_rate);

    assert!(ctrls::set_inter_comp_controls(5).is_none());
}

/// TIER 4. `set_inter_intra_ctrls` (`enc_mode_config.c:5385`).
#[test]
fn inter_intra_ctrls_traced_vectors() {
    let expect: [(u8, u8, u8, u8, u8); 3] = [(0, 0, 0, 0, 0), (1, 1, 1, 1, 1), (2, 1, 0, 0, 2)];
    for &(lvl, en, rd, wsq, wnsq) in &expect {
        let c = ctrls::set_inter_intra_ctrls(lvl).expect("level in range");
        assert_eq!(
            (c.enabled, c.use_rd_model, c.wedge_mode_sq, c.wedge_mode_nsq),
            (en, rd, wsq, wnsq),
            "inter_intra level={lvl}"
        );
    }
    assert!(ctrls::set_inter_intra_ctrls(3).is_none());
}

/// TIER 4. `set_interpolation_search_level_ctrls` (`enc_mode_config.c:4069`).
#[test]
fn interpolation_search_level_traced_vectors() {
    use ctrls::IfsLevel;
    let expect = [
        (0u8, IfsLevel::Off),
        (1, IfsLevel::Mds0),
        (2, IfsLevel::Mds1),
        (3, IfsLevel::Mds2),
        (4, IfsLevel::Mds3),
    ];
    for &(lvl, want) in &expect {
        assert_eq!(
            ctrls::set_interpolation_search_level_ctrls(lvl),
            Some(want),
            "ifs level={lvl}"
        );
    }
    assert!(ctrls::set_interpolation_search_level_ctrls(5).is_none());
}
