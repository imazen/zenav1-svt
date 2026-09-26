use super::*;

fn mv(x: i16, y: i16) -> Mv {
    Mv { x, y }
}

/// TIER 4 — the writer-side predicate can return WARPED_CAUSAL, which
/// the MD-side `obmc_motion_mode_allowed` never does, and
/// `force_integer_mv` demotes it to OBMC.
#[test]
fn tier4_motion_mode_allowed_warp_promotion_and_demotion() {
    let base = |fim: u8, npr: u16, allow_warp: bool| {
        motion_mode_allowed(
            true,
            fim,
            allow_warp,
            TransformationType::Identity,
            npr,
            1,
            9, // BLOCK_32X32
            -1,
            PredictionMode::NewMv as u8,
        )
    };
    assert_eq!(base(0, 1, true), MotionMode::WarpedCausal);
    assert_eq!(base(1, 1, true), MotionMode::ObmcCausal);
    assert_eq!(base(0, 0, true), MotionMode::ObmcCausal);
    assert_eq!(base(0, 1, false), MotionMode::ObmcCausal);
    // No overlappable neighbours: simple translation.
    assert_eq!(
        motion_mode_allowed(
            true,
            0,
            true,
            TransformationType::Identity,
            1,
            0,
            9,
            -1,
            PredictionMode::NewMv as u8
        ),
        MotionMode::SimpleTranslation
    );
    // A global-MV block is excluded when force_integer_mv is 0.
    assert_eq!(
        motion_mode_allowed(
            true,
            0,
            true,
            TransformationType::RotZoom,
            1,
            1,
            9,
            -1,
            PredictionMode::GlobalMv as u8
        ),
        MotionMode::SimpleTranslation
    );
}

/// TIER 4 — the subsampled-plane nudges fire only for a ONE-mi-unit
/// block on the subsampled axis.
#[test]
fn tier4_setup_pred_plane_subsampling_nudges() {
    // No subsampling: the odd mi_row is used as-is.
    let a = setup_pred_plane(1, 1, 64, 64, 128, 3, 3, 0, 0);
    assert_eq!(a.offset, (4 * 3 * 128 + 4 * 3) as usize);
    // Subsampled Y with a 1-mi-tall block and an odd row: nudged down.
    let b = setup_pred_plane(1, 1, 64, 64, 128, 3, 2, 0, 1);
    assert_eq!(b.offset, (((4 * 2) >> 1) * 128 + 4 * 2) as usize);
    // A taller block does NOT get nudged.
    let c = setup_pred_plane(2, 2, 64, 64, 128, 3, 2, 0, 1);
    assert_eq!(c.offset, (((4 * 3) >> 1) * 128 + 4 * 2) as usize);
}

/// TIER 4 — the rate delta can be negative and C carries it through
/// unsigned arithmetic.
#[test]
fn tier4_update_refined_mv_fast_rate() {
    let row = || {
        crate::intrabc::MvComponentCost::from_table(
            (0..super::super::pme::MV_VALS)
                .map(|i| (i as i32 - super::super::pme::MV_MAX).abs())
                .collect(),
        )
    };
    let t = MvCostTable {
        joint_cost: [0, 100, 200, 300],
        comp_cost: [row(), row()],
    };
    let base = 10_000u64;
    // A refinement toward the reference LOWERS the rate.
    let lower = update_refined_mv_fast_rate(base, mv(64, 0), Mv::ZERO, mv(8, 0), Mv::ZERO, &t);
    assert!(lower < base, "a cheaper MV must lower the fast rate");
    // ...and away from it raises it.
    let higher = update_refined_mv_fast_rate(base, mv(8, 0), Mv::ZERO, mv(64, 0), Mv::ZERO, &t);
    assert!(higher > base);
    // An unchanged MV is a no-op.
    assert_eq!(
        update_refined_mv_fast_rate(base, mv(8, 0), Mv::ZERO, mv(8, 0), Mv::ZERO, &t),
        base
    );
}

#[test]
fn tier4_refine_stage_selection() {
    assert_eq!(warp_refine_stage(0), MdStage::Invalid);
    assert_eq!(warp_refine_stage(1), MdStage::Stage1);
    assert_eq!(warp_refine_stage(2), MdStage::Stage3);
    assert_eq!(warp_refine_stage(3), MdStage::Invalid);
    // OBMC pairs TWO levels per stage.
    assert_eq!(obmc_refine_stage(0), MdStage::Invalid);
    assert_eq!(obmc_refine_stage(1), MdStage::Stage1);
    assert_eq!(obmc_refine_stage(2), MdStage::Stage1);
    assert_eq!(obmc_refine_stage(3), MdStage::Stage3);
    assert_eq!(obmc_refine_stage(4), MdStage::Stage3);
    assert_eq!(obmc_refine_stage(5), MdStage::Invalid);
}

fn warp_cand() -> InterCandidate {
    InterCandidate {
        mode: PredictionMode::NewMv,
        motion_mode: MotionMode::WarpedCausal,
        ref_frame: [1, -1],
        ..Default::default()
    }
}

fn snap(c: &InterCandidate) -> RefinementSnapshot {
    RefinementSnapshot {
        mv: c.mv[0],
        pred_mv: c.pred_mv[0],
        drl_index: c.drl_index,
        wm_params_l0: c.wm_params_l0,
        num_proj_ref: c.num_proj_ref,
    }
}

/// TIER 4 — the warp arm rolls back on an UNCHANGED MV as well as on
/// an invalid one, and restores five fields.
#[test]
fn tier4_warp_refinement_rolls_back_on_unchanged_mv() {
    let mut c = warp_cand();
    c.num_proj_ref = 7;
    let s = snap(&c);
    // Valid but unchanged -> rollback.
    let out = opt_non_translation_motion_mode_warp(
        1,
        1,
        true,
        MdStage::Stage1,
        &mut c,
        s,
        |cand| {
            cand.num_proj_ref = 99;
            true
        },
        |_| panic!("wm params must not be derived on the rollback path"),
    );
    assert_eq!(out, RefinementOutcome::RolledBack);
    assert_eq!(c.num_proj_ref, 7, "num_proj_ref is restored too");

    // Valid AND changed -> refined.
    let mut c = warp_cand();
    let s = snap(&c);
    let mut derived = false;
    let out = opt_non_translation_motion_mode_warp(
        1,
        1,
        true,
        MdStage::Stage1,
        &mut c,
        s,
        |cand| {
            cand.mv[0] = mv(16, 16);
            true
        },
        |_| derived = true,
    );
    assert_eq!(out, RefinementOutcome::Refined);
    assert!(derived);
    assert_eq!(c.mv[0], mv(16, 16));

    // Wrong stage -> not applicable, and the refinement never runs.
    let mut c = warp_cand();
    let s = snap(&c);
    let out = opt_non_translation_motion_mode_warp(
        1,
        1,
        true,
        MdStage::Stage3,
        &mut c,
        s,
        |_| panic!("must not refine at the wrong stage"),
        |_| panic!(),
    );
    assert_eq!(out, RefinementOutcome::NotApplicable);

    // refinement_iterations == 0 skips the search and rolls back.
    let mut c = warp_cand();
    let s = snap(&c);
    let out = opt_non_translation_motion_mode_warp(
        1,
        0,
        true,
        MdStage::Stage1,
        &mut c,
        s,
        |_| panic!("must not refine with zero iterations"),
        |_| panic!(),
    );
    assert_eq!(out, RefinementOutcome::RolledBack);
}

/// TIER 4 — the OBMC arm does NOT roll back a valid-but-unchanged
/// refinement.
#[test]
fn tier4_obmc_refinement_does_not_roll_back_unchanged() {
    let mut c = InterCandidate {
        mode: PredictionMode::NewMv,
        motion_mode: MotionMode::ObmcCausal,
        drl_index: 2,
        ..Default::default()
    };
    let s = snap(&c);
    let out = opt_non_translation_motion_mode_obmc(1, true, MdStage::Stage1, &mut c, s, |cand| {
        cand.drl_index = 5;
        true
    });
    assert_eq!(out, RefinementOutcome::NotApplicable);
    assert_eq!(c.drl_index, 5, "a valid-but-unchanged refinement is kept");

    // Invalid -> rollback of the three fields.
    let mut c = InterCandidate {
        mode: PredictionMode::NewMv,
        motion_mode: MotionMode::ObmcCausal,
        drl_index: 2,
        ..Default::default()
    };
    let s = snap(&c);
    let out = opt_non_translation_motion_mode_obmc(3, true, MdStage::Stage3, &mut c, s, |cand| {
        cand.drl_index = 5;
        cand.mv[0] = mv(8, 8);
        false
    });
    assert_eq!(out, RefinementOutcome::RolledBack);
    assert_eq!(c.drl_index, 2);
    assert_eq!(c.mv[0], Mv::ZERO);
}

/// TIER 4 — the face-off's rate terms come from three different
/// places depending on what the writer permits.
#[test]
fn tier4_obmc_face_off_rate_terms() {
    let b1 = [11i32, 22];
    let b3 = [100i32, 200, 300];
    assert_eq!(
        obmc_face_off_rate_terms(MotionMode::SimpleTranslation, &b1, &b3),
        (0, 0)
    );
    assert_eq!(
        obmc_face_off_rate_terms(MotionMode::ObmcCausal, &b1, &b3),
        (11, 22)
    );
    assert_eq!(
        obmc_face_off_rate_terms(MotionMode::WarpedCausal, &b1, &b3),
        (100, 200)
    );
}

/// TIER 4 — the hbd lambda is shifted RIGHT by 4 in the face-off.
#[test]
fn tier4_obmc_face_off_lambda_undoes_the_hbd_shift() {
    assert_eq!(obmc_face_off_lambda(false, 1 << 20, 1234), 1234);
    assert_eq!(obmc_face_off_lambda(true, 1 << 20, 1234), (1 << 20) >> 4);
}

#[test]
fn tier4_obmc_face_off_applies() {
    assert!(obmc_trans_face_off_applies(
        PredictionMode::NewMv,
        MotionMode::SimpleTranslation,
        false,
        true
    ));
    // Not simple translation.
    assert!(!obmc_trans_face_off_applies(
        PredictionMode::NewMv,
        MotionMode::ObmcCausal,
        false,
        true
    ));
    // Inter-intra in use.
    assert!(!obmc_trans_face_off_applies(
        PredictionMode::NewMv,
        MotionMode::SimpleTranslation,
        true,
        true
    ));
    // A compound mode is not single-ref.
    assert!(!obmc_trans_face_off_applies(
        PredictionMode::NewNewMv,
        MotionMode::SimpleTranslation,
        false,
        true
    ));
}

/// TIER 4 — the PD0 fast cost is the raw variance: no lambda, no
/// rate, no subsampling (the SVT_HDR_MODE arm is not compiled).
#[test]
fn tier4_fast_loop_core_pd0_is_the_plain_variance() {
    assert_eq!(fast_loop_core_pd0_cost(12_345), 12_345);
    assert_eq!(fast_loop_core_pd0_cost(0), 0);
}

/// TIER 4 — the PD0 staging loop's buffer index flips only on a WIN,
/// and ties keep the earlier candidate.
#[test]
fn tier4_md_stage_0_pd0_buffer_pingpong() {
    // Strictly improving: every candidate wins, so the index
    // alternates and ends on the parity of the count.
    assert_eq!(md_stage_0_pd0(&[100, 90, 80]), (80, 0));
    assert_eq!(md_stage_0_pd0(&[100, 90]), (90, 1));
    // Only the first wins: the index never flips past 1.
    assert_eq!(md_stage_0_pd0(&[100, 200, 300]), (100, 0));
    // Ties keep the earlier candidate.
    assert_eq!(md_stage_0_pd0(&[100, 100, 100]), (100, 0));
    assert_eq!(md_stage_0_pd0(&[]), (u64::MAX, 0));
}

#[test]
fn tier4_md_stage_3_pd0_subres_step_caps_small_blocks() {
    assert_eq!(md_stage_3_pd0_subres_step(16, 2), 2);
    assert_eq!(md_stage_3_pd0_subres_step(32, 2), 2);
    assert_eq!(md_stage_3_pd0_subres_step(8, 2), 1);
    assert_eq!(md_stage_3_pd0_subres_step(8, 0), 0);
    assert_eq!(md_stage_3_pd0_subres_step(4, 2), 1);
}

/// TIER 4 — the PD0 candidate plan: no intra at 128x128, no inter on
/// an I slice, and the ZZ backup only for a non-I slice that came out
/// empty.
#[test]
fn tier4_generate_md_stage_0_cand_pd0_plan_and_dispatch() {
    let p = generate_md_stage_0_cand_pd0_plan(false, 64, true, false);
    assert!(p.inject_intra && p.inject_inter && p.inject_zz_backup_if_empty);
    assert!(!generate_md_stage_0_cand_pd0_plan(false, 128, true, false).inject_intra);
    assert!(!generate_md_stage_0_cand_pd0_plan(false, 64, false, false).inject_intra);
    assert!(!generate_md_stage_0_cand_pd0_plan(true, 64, true, false).inject_inter);
    assert!(!generate_md_stage_0_cand_pd0_plan(true, 64, true, false).inject_zz_backup_if_empty);

    // The ZZ backup fires only when the other stages produced nothing.
    let mut cands = CandArray::new(16);
    let mut zz_ran = false;
    let n = generate_md_stage_0_cand_pd0(
        generate_md_stage_0_cand_pd0_plan(false, 64, true, false),
        &mut cands,
        |_| {},
        |_| {},
        |c| {
            zz_ran = true;
            c.push(InterCandidate::default());
        },
        |_| {},
    );
    assert!(zz_ran);
    assert_eq!(n, 1);

    // With a candidate already injected it does not.
    let mut cands = CandArray::new(16);
    let mut zz_ran = false;
    let n = generate_md_stage_0_cand_pd0(
        generate_md_stage_0_cand_pd0_plan(false, 64, true, false),
        &mut cands,
        |c| c.push(InterCandidate::default()),
        |_| {},
        |_| zz_ran = true,
        |_| {},
    );
    assert!(!zz_ran);
    assert_eq!(n, 1);
}

/// TIER 4 — the inter-intra mode pick ties toward the LOWER mode, and
/// `ii_wedge_mode == 1` forces the wedge flag on regardless of RD.
#[test]
fn tier4_inter_intra_search_decisions() {
    // Distinct RDs: the minimum wins.
    let r = inter_intra_search(2, |m| i64::from(10 - m), |_| (1000, 3));
    assert_eq!(r.interintra_mode, II_SMOOTH_PRED);
    assert!(!r.use_wedge_interintra, "wedge RD 1000 lost to 7");
    assert_eq!(r.interintra_wedge_index, 3);

    // Ties keep the LOWEST mode.
    let r = inter_intra_search(0, |_| 5, |_| (0, 0));
    assert_eq!(r.interintra_mode, II_DC_PRED);
    // wedge_mode 0 never runs the wedge search, so the flag is off.
    assert!(!r.use_wedge_interintra);

    // wedge_mode 1 forces the flag on even when the wedge RD lost.
    let r = inter_intra_search(1, |_| 5, |_| (i64::MAX, 7));
    assert!(r.use_wedge_interintra);
    assert_eq!(r.interintra_wedge_index, 7);

    // wedge_mode 2 sets it only when the wedge actually won.
    let r = inter_intra_search(2, |_| 5, |_| (4, 1));
    assert!(r.use_wedge_interintra);
    let r = inter_intra_search(2, |_| 5, |_| (6, 1));
    assert!(!r.use_wedge_interintra);
}

/// TIER 4 — `diff10` is pred1 MINUS pred0, and both planes use the
/// BLOCK stride, not the source stride.
#[test]
fn tier4_interintra_wedge_residual_order_and_stride() {
    let bw = 2;
    let bh = 2;
    // A source with a WIDER stride than the block.
    let src = vec![100u8, 101, 255, 255, 110, 111, 255, 255];
    let pred0 = vec![1u8, 2, 3, 4];
    let pred1 = vec![10u8, 20, 30, 40];
    let (residual1, diff10) = interintra_wedge_residuals(&src, 4, &pred0, &pred1, bw, bh);
    assert_eq!(residual1, vec![90, 81, 80, 71]);
    assert_eq!(diff10, vec![9, 18, 27, 36]);
    // Swapping the predictions flips diff10's sign, which is why the
    // order is load-bearing.
    let (_, swapped) = interintra_wedge_residuals(&src, 4, &pred1, &pred0, bw, bh);
    assert_eq!(swapped, vec![-9, -18, -27, -36]);
}
