use super::*;
use svtav1_types::motion::CandidateMv;

fn mv(x: i16, y: i16) -> Mv {
    Mv { x, y }
}

fn me(direction: u8, l0: u8, l1: u8, r0l: u8, r1l: u8) -> MeCandidateRef {
    MeCandidateRef {
        direction,
        ref_idx_l0: l0,
        ref_idx_l1: l1,
        ref0_list: r0l,
        ref1_list: r1l,
    }
}

fn zero_table() -> MvCostTable {
    MvCostTable::zeroed()
}

fn stack_with(count: u8, mvs: &[(i16, i16, i16, i16)]) -> InterMvpStack {
    let mut s = InterMvpStack {
        count,
        ..Default::default()
    };
    for (i, &(x, y, cx, cy)) in mvs.iter().enumerate() {
        s.stack[i] = CandidateMv {
            this_mv: mv(x, y),
            comp_mv: mv(cx, cy),
            weight: 700,
        };
    }
    s
}

/// The whole fixed world a test case needs; every field mirrors a C
/// context field and the defaults are the "everything off" state.
struct World {
    refs: Vec<i8>,
    gm: [WarpedMotionParams; 8],
    wm_num: [u8; 8],
    stack: Vec<InterMvpStack>,
    count: Vec<u8>,
    nmv: MvCostTable,
    fac: [[i32; 2]; 3],
    me_cands: Vec<MeCandidateRef>,
    me_totals: Vec<u8>,
    sb_me_mv: [[Mv; 4]; 2],
    cost: [[u32; 4]; 2],
    valid_pme: [[bool; 4]; 2],
    best_pme: [[Mv; 4]; 2],
    pruning: RefPruningState,
}

impl World {
    fn new() -> Self {
        // Pruning disabled: every reference is admissible, which is
        // the state that isolates the injector logic under test.
        let pruning = RefPruningState::default();
        Self {
            refs: vec![1],
            gm: [WarpedMotionParams::default(); 8],
            wm_num: [0; 8],
            stack: vec![stack_with(2, &[(4, 8, -4, -8), (12, 16, -12, -16)]); 29],
            count: vec![2; 29],
            nmv: zero_table(),
            fac: [[0; 2]; 3],
            me_cands: vec![],
            me_totals: vec![0],
            sb_me_mv: [[Mv::ZERO; 4]; 2],
            cost: [[0; 4]; 2],
            valid_pme: [[false; 4]; 2],
            best_pme: [[Mv::ZERO; 4]; 2],
            pruning,
        }
    }

    fn ctx(&self) -> InjectCtx<'_> {
        InjectCtx {
            bsize: 9, // BLOCK_32X32
            bwidth: 32,
            bheight: 32,
            blk_org_x: 0,
            blk_org_y: 0,
            shape_is_part_n: true,
            reference_mode_is_single: false,
            allow_high_precision_mv: false,
            is_motion_mode_switchable: false,
            force_integer_mv: 0,
            skip_mode_flag: false,
            skip_mode_ref_frame_idx_0: -1,
            skip_mode_ref_frame_idx_1: -1,
            is_lossless_segment: false,
            ref_frame_type_arr: &self.refs,
            global_motion: &self.gm,
            gm_skip_identity: false,
            wm_sample_num: &self.wm_num,
            ref_mv_stack: &self.stack,
            ref_mv_count: &self.count,
            nmv_cost: &self.nmv,
            drl_mode_fac_bits: &self.fac,
            shut_fast_rate: false,
            approx_inter_rate: 0,
            total_me_cnt: self.me_cands.len(),
            me_cands: &self.me_cands,
            me_totals: &self.me_totals,
            me_block_offset: 0,
            sb_me_mv: &self.sb_me_mv,
            post_subpel_me_mv_cost: &self.cost,
            valid_pme_mv: &self.valid_pme,
            best_pme_mv: &self.best_pme,
            ref_pruning: &self.pruning,
            corrupted_mv_check: false,
            redundant_cand_ctrls: RedundantCandCtrls::default(),
            inter_comp_ctrls: InterCompCtrls::default(),
            inter_intra_comp_ctrls: InterIntraCompCtrls::default(),
            wm_ctrls: WmCtrls::default(),
            obmc_ctrls: ObmcCtrls::default(),
            near_count_ctrls: NearCountCtrls::default(),
            bipred3x3_ctrls: Bipred3x3Ctrls::default(),
            unipred3x3_injection: 0,
            new_nearest_injection: true,
            new_nearest_near_comb_injection: 1,
            inject_new_me: true,
            global_mv_injection: false,
            inject_new_pme: false,
            updated_enable_pme: false,
            reduce_unipred_candidates: 0,
            use_neighbouring_mode_ctrls_enabled: false,
            lpd1_mvp_best_me_list: false,
            is_intra_bordered: false,
            has_overlappable_candidates: false,
            allow_warped_motion: false,
            left_available: false,
            up_available: false,
            left_mi: None,
            above_mi: None,
        }
    }
}

/// TIER 4 — `INC_MD_CAND_CNT` is `if (cnt + 1 < max) cnt++;`, so the
/// count SATURATES at `max - 1` and the next write overwrites the
/// last slot rather than growing the array.
#[test]
fn tier4_inc_md_cand_cnt_saturates_one_below_max() {
    let mut a = CandArray::new(3);
    for i in 0..5u8 {
        a.push(InterCandidate {
            drl_index: i,
            ..Default::default()
        });
    }
    assert_eq!(a.count(), 2, "count stops at max_can_count - 1");
    assert_eq!(a.overflow_events, 3);
    // C keeps WRITING at the un-incremented index (2) while
    // `cand_array[count - 1]` still points at index 1. So after an
    // overflow the "previously injected candidate" that
    // inj_non_simple_modes / inj_comp_modes clone from is NOT the one
    // just written — candidates 2, 3 and 4 all landed in slot 2 and
    // are invisible to `last()`.
    assert_eq!(a.last().unwrap().drl_index, 1);
    assert_eq!(a.as_slice().len(), 2);
}

/// TIER 4 — `determine_compound_mode` writes CODED syntax.
#[test]
fn tier4_determine_compound_mode_syntax_values() {
    let mut h = NoRefinement;
    let check = |t: u8, h: &mut NoRefinement| {
        let mut c = InterCandidate::default();
        determine_compound_mode(&mut c, t, h);
        (
            c.comp_group_idx,
            c.compound_idx,
            c.interinter_comp_type,
            c.interinter_mask_type,
        )
    };
    assert_eq!(check(0, &mut h), (0, 1, 0, 0)); // AVG
    assert_eq!(check(1, &mut h), (0, 0, 1, 0)); // DIST
    // The MD-side order is DIFFWTD-then-WEDGE while the AV1 enum is
    // WEDGE=2 / DIFFWTD=3 (definitions.h:1259) — the LUT is NOT the
    // identity, and an identity LUT would code a searched DIFFWTD as
    // WEDGE and vice versa.
    assert_eq!(check(2, &mut h), (1, 1, 3, 55)); // DIFF0 -> COMPOUND_DIFFWTD, mask_type 55
    assert_eq!(check(3, &mut h), (1, 1, 2, 0)); // WEDGE -> COMPOUND_WEDGE
}

/// TIER 4 — `TO_AV1_COMPOUND_LUT` mirrors C's
/// `{COMPOUND_AVERAGE, COMPOUND_DISTWTD, COMPOUND_DIFFWTD, COMPOUND_WEDGE}`
/// (mode_decision.c:494) against the enum `AVERAGE=0, DISTWTD=1, WEDGE=2,
/// DIFFWTD=3` (definitions.h:1259): the values are `[0, 1, 3, 2]`.
/// A `svtav1_dsp` `CompoundType::from` round-trip must land on the same
/// masked type the search picked — this is the wiring
/// `build_masked_compound_no_round_matches_c` could not see at MD level.
#[test]
fn tier4_compound_lut_is_not_identity() {
    use svtav1_dsp::port_masked_compound::CompoundType;
    assert_eq!(TO_AV1_COMPOUND_LUT[0], CompoundType::Average as u8);
    assert_eq!(TO_AV1_COMPOUND_LUT[1], CompoundType::DistWtd as u8);
    assert_eq!(TO_AV1_COMPOUND_LUT[2], CompoundType::DiffWtd as u8);
    assert_eq!(TO_AV1_COMPOUND_LUT[3], CompoundType::Wedge as u8);
}

/// TIER 4 — `allow_bipred` (AV1 spec 5.11.25): BOTH dimensions must
/// exceed 4, and `SINGLE_REFERENCE` disables it outright.
#[test]
fn tier4_allow_bipred_gate() {
    let w = World::new();
    let mut c = w.ctx();
    assert!(c.allow_bipred());
    c.bwidth = 4;
    assert!(!c.allow_bipred());
    c.bwidth = 32;
    c.bheight = 4;
    assert!(!c.allow_bipred());
    c.bheight = 32;
    c.reference_mode_is_single = true;
    assert!(!c.allow_bipred());
}

/// TIER 4 — the NEAR loop is capped to ZERO unless
/// `near_count_ctrls.enabled`: C initialises `cap_max_drl_index = 0`
/// and only assigns inside the `if`. A port that used
/// `max_drl_index` directly would inject NEAR candidates C never
/// does.
#[test]
fn tier4_mvp_near_loop_is_zero_without_near_count_ctrls() {
    let mut w = World::new();
    w.stack = vec![
        stack_with(
            4,
            &[(4, 8, 0, 0), (12, 16, 0, 0), (20, 24, 0, 0), (28, 32, 0, 0)]
        );
        29
    ];
    w.count = vec![4; 29];
    let mut h = NoRefinement;

    // Control OFF: only the NEAREST candidate.
    let ctx = w.ctx();
    let mut cands = CandArray::new(64);
    let mut log = InjectedMvLog::default();
    inject_mvp_candidates_ii(&ctx, &mut cands, &mut log, &mut h, true);
    assert_eq!(cands.count(), 1);
    assert_eq!(cands.as_slice()[0].mode, PredictionMode::NearestMv);

    // Control ON with near_count 2: NEAREST + two NEARs.
    let mut ctx = w.ctx();
    ctx.near_count_ctrls = NearCountCtrls {
        enabled: true,
        near_count: 2,
        near_near_count: 2,
    };
    let mut cands = CandArray::new(64);
    let mut log = InjectedMvLog::default();
    inject_mvp_candidates_ii(&ctx, &mut cands, &mut log, &mut h, true);
    assert_eq!(cands.count(), 3);
    assert_eq!(cands.as_slice()[1].mode, PredictionMode::NearMv);
    assert_eq!(cands.as_slice()[1].drl_index, 0);
    assert_eq!(cands.as_slice()[2].drl_index, 1);
}

/// TIER 4 — the injected-MV log dedups within a stage. Two identical
/// stack entries yield ONE candidate.
#[test]
fn tier4_mvp_dedups_identical_mvs() {
    let mut w = World::new();
    // slot 0 and slot 1 (the first NEAR) carry the same MV.
    w.stack = vec![stack_with(4, &[(4, 8, 0, 0), (4, 8, 0, 0), (4, 8, 0, 0), (4, 8, 0, 0)]); 29];
    w.count = vec![4; 29];
    let mut ctx = w.ctx();
    ctx.near_count_ctrls = NearCountCtrls {
        enabled: true,
        near_count: 3,
        near_near_count: 0,
    };
    let mut h = NoRefinement;
    let mut cands = CandArray::new(64);
    let mut log = InjectedMvLog::default();
    inject_mvp_candidates_ii(&ctx, &mut cands, &mut log, &mut h, true);
    assert_eq!(cands.count(), 1, "the NEAR duplicates are deduped away");
}

/// TIER 4 — `inject_new_candidates` turns ME candidates into NEWMV,
/// and `reduce_unipred_candidates` drops uni-pred ones only when
/// `total_me_cnt > 3`.
#[test]
fn tier4_inject_new_candidates_and_the_unipred_reduction() {
    let mut w = World::new();
    w.me_cands = vec![me(0, 0, 0, 0, 0), me(0, 1, 0, 0, 0)];
    w.me_totals = vec![2];
    w.sb_me_mv[0][0] = mv(8, 8);
    w.sb_me_mv[0][1] = mv(16, 16);
    let ctx = w.ctx();
    let mut h = NoRefinement;
    let mut cands = CandArray::new(64);
    let mut log = InjectedMvLog::default();
    inject_new_candidates(&ctx, &mut cands, &mut log, &mut h, true);
    assert_eq!(cands.count(), 2);
    assert_eq!(cands.as_slice()[0].mode, PredictionMode::NewMv);
    assert_eq!(cands.as_slice()[0].mv[0], mv(8, 8));
    assert_eq!(cands.as_slice()[0].ref_frame, [1, NONE_FRAME]);
    assert_eq!(cands.as_slice()[1].ref_frame, [2, NONE_FRAME]);

    // reduce_unipred_candidates with only 2 ME candidates: the
    // `total_me_cnt > 3` guard is NOT met, so nothing is dropped.
    let mut ctx = w.ctx();
    ctx.reduce_unipred_candidates = 1;
    let mut cands = CandArray::new(64);
    let mut log = InjectedMvLog::default();
    inject_new_candidates(&ctx, &mut cands, &mut log, &mut h, true);
    assert_eq!(cands.count(), 2);

    // With 4 uni-pred ME candidates it fires and drops them all.
    let mut w4 = World::new();
    w4.me_cands = vec![
        me(0, 0, 0, 0, 0),
        me(0, 1, 0, 0, 0),
        me(0, 2, 0, 0, 0),
        me(0, 3, 0, 0, 0),
    ];
    w4.me_totals = vec![4];
    for i in 0..4 {
        w4.sb_me_mv[0][i] = mv(8 * (i as i16 + 1), 0);
    }
    let mut ctx = w4.ctx();
    ctx.reduce_unipred_candidates = 1;
    let mut cands = CandArray::new(64);
    let mut log = InjectedMvLog::default();
    inject_new_candidates(&ctx, &mut cands, &mut log, &mut h, true);
    assert_eq!(cands.count(), 0);
}

/// TIER 4 — the uni-pred 3x3 step is `<< !allow_high_precision_mv`:
/// 2 eighth-pel units at quarter-pel precision, 1 at eighth-pel.
#[test]
fn tier4_unipred_3x3_step_depends_on_mv_precision() {
    let mut w = World::new();
    w.me_cands = vec![me(0, 0, 0, 0, 0)];
    w.me_totals = vec![1];
    w.sb_me_mv[0][0] = mv(0, 0);
    let mut h = NoRefinement;

    // Quarter-pel: the first position (-1, 0) becomes (-2, 0).
    let mut ctx = w.ctx();
    ctx.unipred3x3_injection = 1;
    ctx.allow_high_precision_mv = false;
    let mut cands = CandArray::new(64);
    let mut log = InjectedMvLog::default();
    unipred_3x3_candidates_injection(&ctx, &mut cands, &mut log, &mut h);
    assert_eq!(cands.count(), 8);
    assert_eq!(cands.as_slice()[0].mv[0], mv(-2, 0));

    // Eighth-pel: (-1, 0).
    let mut ctx = w.ctx();
    ctx.unipred3x3_injection = 1;
    ctx.allow_high_precision_mv = true;
    let mut cands = CandArray::new(64);
    let mut log = InjectedMvLog::default();
    unipred_3x3_candidates_injection(&ctx, &mut cands, &mut log, &mut h);
    assert_eq!(cands.as_slice()[0].mv[0], mv(-1, 0));

    // Level >= 2 keeps only the four allow_refinement_flag positions.
    let mut ctx = w.ctx();
    ctx.unipred3x3_injection = 2;
    let mut cands = CandArray::new(64);
    let mut log = InjectedMvLog::default();
    unipred_3x3_candidates_injection(&ctx, &mut cands, &mut log, &mut h);
    assert_eq!(cands.count(), 4);
}

/// TIER 4 — `inj_comp_modes` has five early returns; this pins the
/// MV-length cap and the `no_sym_dist` skip, and that the loop runs
/// `MD_COMP_DIST .. tot_comp_types`.
#[test]
fn tier4_inj_comp_modes_variants_and_gates() {
    let mut w = World::new();
    w.pruning.enabled = false;
    let mut h = NoRefinement;

    let base = InterCandidate {
        mode: PredictionMode::NewNewMv,
        ref_frame: [1, 5],
        mv: [mv(8, 8), mv(-8, -8)],
        ..Default::default()
    };

    // tot_comp_types = 4 on a 32x32 block (wedge params present):
    // DIST, DIFF0, WEDGE.
    let mut ctx = w.ctx();
    ctx.inter_comp_ctrls.tot_comp_types = 4;
    let mut cands = CandArray::new(64);
    cands.push(base);
    inj_comp_modes(&ctx, &mut cands, &mut h);
    assert_eq!(cands.count(), 4);
    // MD types DIST/DIFF0/WEDGE map through the non-identity LUT to
    // AV1 values DISTWTD=1 / DIFFWTD=3 / WEDGE=2.
    assert_eq!(cands.as_slice()[1].interinter_comp_type, 1);
    assert_eq!(cands.as_slice()[2].interinter_comp_type, 3);
    assert_eq!(cands.as_slice()[3].interinter_comp_type, 2);

    // tot_comp_types == MD_COMP_DIST (1) is an EQUALITY early return.
    let mut ctx = w.ctx();
    ctx.inter_comp_ctrls.tot_comp_types = 1;
    let mut cands = CandArray::new(64);
    cands.push(base);
    inj_comp_modes(&ctx, &mut cands, &mut h);
    assert_eq!(cands.count(), 1);

    // The MV-length cap.
    let mut ctx = w.ctx();
    ctx.inter_comp_ctrls.tot_comp_types = 4;
    ctx.inter_comp_ctrls.max_mv_length = 4;
    let mut cands = CandArray::new(64);
    cands.push(base);
    inj_comp_modes(&ctx, &mut cands, &mut h);
    assert_eq!(cands.count(), 1);

    // no_sym_dist skips DIST when BOTH ref indices are 0 (LAST+BWD).
    let mut ctx = w.ctx();
    ctx.inter_comp_ctrls.tot_comp_types = 4;
    ctx.inter_comp_ctrls.no_sym_dist = true;
    let mut cands = CandArray::new(64);
    cands.push(base);
    inj_comp_modes(&ctx, &mut cands, &mut h);
    assert_eq!(cands.count(), 3);
    // First injected is DIFF0 (MD type 2) -> COMPOUND_DIFFWTD = 3.
    assert_eq!(cands.as_slice()[1].interinter_comp_type, 3);
}

/// TIER 4 — `calc_pred_masked_compound` returning non-zero aborts the
/// whole thing, injecting NOTHING.
#[test]
fn tier4_inj_comp_modes_masked_compound_abort() {
    struct Abort;
    impl InjectHooks for Abort {
        fn inter_intra_search(&mut self, _c: &mut InterCandidate) {}
        fn wm_motion_refinement(&mut self, _c: &mut InterCandidate) -> bool {
            true
        }
        fn warped_motion_parameters(&mut self, _c: &mut InterCandidate) -> bool {
            true
        }
        fn obmc_motion_refinement(&mut self, _c: &mut InterCandidate) -> bool {
            true
        }
        fn calc_pred_masked_compound(&mut self, _c: &InterCandidate) -> bool {
            true
        }
        fn search_compound_diff_wedge(&mut self, _c: &mut InterCandidate) {}
    }
    let w = World::new();
    let mut ctx = w.ctx();
    ctx.inter_comp_ctrls.tot_comp_types = 4;
    let mut cands = CandArray::new(64);
    cands.push(InterCandidate {
        mode: PredictionMode::NewNewMv,
        ref_frame: [1, 5],
        ..Default::default()
    });
    inj_comp_modes(&ctx, &mut cands, &mut Abort);
    assert_eq!(cands.count(), 1);
}

/// TIER 4 — `skip_compound_on_ref_types`. With NEITHER neighbour
/// available the answer is "do not skip"; with one available and no
/// match it is "skip".
#[test]
fn tier4_skip_compound_on_ref_types_shape() {
    let w = World::new();
    let rf = [1i8, 5];

    // Control off: never skips.
    let ctx = w.ctx();
    assert!(!skip_compound_on_ref_types(&ctx, rf));

    // Same list for both refs: always skips.
    let mut ctx = w.ctx();
    ctx.inter_comp_ctrls.skip_on_ref_info = true;
    assert!(skip_compound_on_ref_types(&ctx, [1, 2]));

    // No neighbours: does NOT skip.
    assert!(!skip_compound_on_ref_types(&ctx, rf));

    // A left neighbour that used neither ref: skips.
    let mut ctx = w.ctx();
    ctx.inter_comp_ctrls.skip_on_ref_info = true;
    ctx.left_available = true;
    ctx.left_mi = Some((PredictionMode::NewMv, [3, NONE_FRAME]));
    assert!(skip_compound_on_ref_types(&ctx, rf));

    // A left neighbour that used ONE of them: does not skip.
    ctx.left_mi = Some((PredictionMode::NewMv, [5, NONE_FRAME]));
    assert!(!skip_compound_on_ref_types(&ctx, rf));

    // A compound neighbour must match BOTH, not either.
    ctx.left_mi = Some((PredictionMode::NewNewMv, [1, 6]));
    assert!(skip_compound_on_ref_types(&ctx, rf));
    ctx.left_mi = Some((PredictionMode::NewNewMv, [1, 5]));
    assert!(!skip_compound_on_ref_types(&ctx, rf));
}

/// TIER 4 — `inj_non_simple_modes`'s inter-intra arm injects TWO
/// candidates when `ii_wedge_mode == 1`.
#[test]
fn tier4_inj_non_simple_modes_interintra_wedge_mode_1_injects_two() {
    let w = World::new();
    let mut h = NoRefinement;
    let mut ctx = w.ctx();
    ctx.inter_intra_comp_ctrls = InterIntraCompCtrls {
        enabled: true,
        wedge_mode_sq: 1,
        wedge_mode_nsq: 0,
        use_rd_model: false,
    };
    let mut cands = CandArray::new(64);
    cands.push(InterCandidate {
        mode: PredictionMode::NewMv,
        ref_frame: [1, NONE_FRAME],
        ..Default::default()
    });
    inj_non_simple_modes(&ctx, &mut cands, &mut h, true, false, false);
    assert_eq!(cands.count(), 3);
    assert!(cands.as_slice()[1].is_interintra_used);
    assert_eq!(cands.as_slice()[1].ref_frame[1], INTRA_FRAME);
    assert!(cands.as_slice()[2].is_interintra_used);
    assert!(!cands.as_slice()[2].use_wedge_interintra);

    // wedge_mode 2 injects only the searched one.
    ctx.inter_intra_comp_ctrls.wedge_mode_sq = 2;
    let mut cands = CandArray::new(64);
    cands.push(InterCandidate {
        mode: PredictionMode::NewMv,
        ref_frame: [1, NONE_FRAME],
        ..Default::default()
    });
    inj_non_simple_modes(&ctx, &mut cands, &mut h, true, false, false);
    assert_eq!(cands.count(), 2);

    // `enable_ii = false` suppresses it entirely, even when allowed.
    ctx.inter_intra_comp_ctrls.wedge_mode_sq = 1;
    let mut cands = CandArray::new(64);
    cands.push(InterCandidate {
        mode: PredictionMode::NewMv,
        ref_frame: [1, NONE_FRAME],
        ..Default::default()
    });
    inj_non_simple_modes(&ctx, &mut cands, &mut h, false, false, false);
    assert_eq!(cands.count(), 1);
}

/// TIER 4 — a failing refinement DROPS the motion-mode candidate.
#[test]
fn tier4_inj_non_simple_modes_refinement_failure_drops_the_candidate() {
    struct NoValidMv;
    impl InjectHooks for NoValidMv {
        fn inter_intra_search(&mut self, _c: &mut InterCandidate) {}
        fn wm_motion_refinement(&mut self, _c: &mut InterCandidate) -> bool {
            false
        }
        fn warped_motion_parameters(&mut self, _c: &mut InterCandidate) -> bool {
            true
        }
        fn obmc_motion_refinement(&mut self, _c: &mut InterCandidate) -> bool {
            false
        }
        fn calc_pred_masked_compound(&mut self, _c: &InterCandidate) -> bool {
            false
        }
        fn search_compound_diff_wedge(&mut self, _c: &mut InterCandidate) {}
    }
    let w = World::new();
    let mut ctx = w.ctx();
    ctx.allow_warped_motion = true;
    ctx.has_overlappable_candidates = true;
    ctx.wm_ctrls = WmCtrls {
        enabled: true,
        use_wm_for_mvp: true,
        refinement_iterations: 1,
        refine_level: 0,
    };
    ctx.is_motion_mode_switchable = true;
    ctx.obmc_ctrls = ObmcCtrls {
        enabled: true,
        max_blk_size: 128,
        trans_face_off: false,
        refine_level: 0,
    };
    let mut cands = CandArray::new(64);
    cands.push(InterCandidate {
        mode: PredictionMode::NewMv,
        ref_frame: [1, NONE_FRAME],
        ..Default::default()
    });
    inj_non_simple_modes(&ctx, &mut cands, &mut NoValidMv, false, true, true);
    assert_eq!(cands.count(), 1, "both refinements failed, nothing added");

    // With refinement succeeding, both arms inject.
    let mut cands = CandArray::new(64);
    cands.push(InterCandidate {
        mode: PredictionMode::NewMv,
        ref_frame: [1, NONE_FRAME],
        ..Default::default()
    });
    inj_non_simple_modes(&ctx, &mut cands, &mut NoRefinement, false, true, true);
    assert_eq!(cands.count(), 3);
    assert_eq!(cands.as_slice()[1].motion_mode, MotionMode::WarpedCausal);
    assert_eq!(cands.as_slice()[2].motion_mode, MotionMode::ObmcCausal);
}

/// TIER 4 — `inject_global_candidates` skips IDENTITY warps under
/// `gm_skip_identity` and emits GLOBALMV otherwise.
#[test]
fn tier4_inject_global_candidates() {
    let mut w = World::new();
    w.gm[1].wm_type = TransformationType::Translation;
    w.gm[1].wmmat[0] = 1 << 13;
    w.gm[1].wmmat[1] = 1 << 13;
    let mut h = NoRefinement;

    let ctx = w.ctx();
    let mut cands = CandArray::new(64);
    let mut log = InjectedMvLog::default();
    inject_global_candidates(&ctx, &mut cands, &mut log, &mut h, true);
    assert_eq!(cands.count(), 1);
    assert_eq!(cands.as_slice()[0].mode, PredictionMode::GlobalMv);
    assert_eq!(
        cands.as_slice()[0].wm_params_l0.wm_type,
        TransformationType::Translation
    );

    // An IDENTITY warp under skip_identity is skipped.
    let mut w2 = World::new();
    let mut ctx = w2.ctx();
    ctx.gm_skip_identity = true;
    let mut cands = CandArray::new(64);
    let mut log = InjectedMvLog::default();
    inject_global_candidates(&ctx, &mut cands, &mut log, &mut h, true);
    assert_eq!(cands.count(), 0);
    // Without skip_identity it is injected with a zero MV.
    w2.gm[1].wm_type = TransformationType::Identity;
    let ctx = w2.ctx();
    let mut cands = CandArray::new(64);
    let mut log = InjectedMvLog::default();
    inject_global_candidates(&ctx, &mut cands, &mut log, &mut h, true);
    assert_eq!(cands.count(), 1);
    assert_eq!(cands.as_slice()[0].mv[0], Mv::ZERO);
}

/// TIER 4 — PME injection is gated on `valid_pme_mv`, not on the
/// reference-pruning table.
#[test]
fn tier4_inject_pme_candidates_gated_on_valid_flag() {
    let mut w = World::new();
    w.best_pme[0][0] = mv(24, -24);
    let mut h = NoRefinement;

    let ctx = w.ctx();
    let mut cands = CandArray::new(64);
    let mut log = InjectedMvLog::default();
    inject_pme_candidates(&ctx, &mut cands, &mut log, &mut h, true);
    assert_eq!(cands.count(), 0);

    w.valid_pme[0][0] = true;
    let ctx = w.ctx();
    let mut cands = CandArray::new(64);
    let mut log = InjectedMvLog::default();
    inject_pme_candidates(&ctx, &mut cands, &mut log, &mut h, true);
    assert_eq!(cands.count(), 1);
    assert_eq!(cands.as_slice()[0].mv[0], mv(24, -24));
    assert_eq!(cands.as_slice()[0].mode, PredictionMode::NewMv);
}

/// TIER 4 — the ZZ backup always produces a zero-MV LAST NEWMV.
#[test]
fn tier4_inject_zz_backup_candidate() {
    let w = World::new();
    let ctx = w.ctx();
    let mut cands = CandArray::new(64);
    inject_zz_backup_candidate(&ctx, &mut cands);
    assert_eq!(cands.count(), 1);
    let c = cands.as_slice()[0];
    assert_eq!(c.mode, PredictionMode::NewMv);
    assert_eq!(c.mv[0], Mv::ZERO);
    assert_eq!(c.ref_frame, [1, NONE_FRAME]);
    assert_eq!(c.transform_type_y, 0);
}

/// TIER 4 — PD0's injector has no dedup, no DRL, no motion modes, and
/// its cap is `cand_total_cnt > 2` checked AFTER each push, so up to
/// three candidates survive.
#[test]
fn tier4_inject_new_candidates_pd0_cap_and_bipred_gate() {
    let mut w = World::new();
    w.me_cands = vec![
        me(0, 0, 0, 0, 0),
        me(0, 1, 0, 0, 0),
        me(0, 2, 0, 0, 0),
        me(0, 3, 0, 0, 0),
    ];
    w.me_totals = vec![4];
    let me_mv_array: Vec<Mv> = (0..8).map(|i| mv(i as i16, -(i as i16))).collect();
    let ctx = w.ctx();
    let mut h = NoRefinement;
    let mut cands = CandArray::new(64);
    inject_new_candidates_pd0(&ctx, &mut cands, &mut h, &me_mv_array, 0, 4, 2, false, true);
    assert_eq!(cands.count(), 3);
    // MVs are the raw ME array x 8.
    assert_eq!(cands.as_slice()[0].mv[0], mv(0, 0));
    assert_eq!(cands.as_slice()[1].mv[0], mv(8, -8));
    assert_eq!(cands.as_slice()[2].mv[0], mv(16, -16));

    // LVL_6 drops BI_PRED candidates outright.
    let mut w2 = World::new();
    w2.me_cands = vec![me(2, 0, 0, 0, 1)];
    w2.me_totals = vec![1];
    let ctx = w2.ctx();
    let mut cands = CandArray::new(64);
    inject_new_candidates_pd0(&ctx, &mut cands, &mut h, &me_mv_array, 0, 4, 2, true, true);
    assert_eq!(cands.count(), 0);
    let mut cands = CandArray::new(64);
    inject_new_candidates_pd0(&ctx, &mut cands, &mut h, &me_mv_array, 0, 4, 2, false, true);
    assert_eq!(cands.count(), 1);
    assert_eq!(cands.as_slice()[0].mode, PredictionMode::NewNewMv);
}

/// TIER 4 — the top-level dispatcher's ORDER, and that each stage's
/// gate really turns it off.
#[test]
fn tier4_inject_inter_candidates_stage_order_and_gates() {
    let mut w = World::new();
    w.me_cands = vec![me(0, 0, 0, 0, 0)];
    w.me_totals = vec![1];
    w.sb_me_mv[0][0] = mv(64, 64);
    w.gm[1].wm_type = TransformationType::Translation;
    w.gm[1].wmmat[0] = 1 << 14;
    w.gm[1].wmmat[1] = 1 << 14;
    let mut h = NoRefinement;

    let mut ctx = w.ctx();
    ctx.global_mv_injection = true;
    ctx.unipred3x3_injection = 2;
    let mut cands = CandArray::new(128);
    let mut log = InjectedMvLog::default();
    inject_inter_candidates(&ctx, &mut cands, &mut log, &mut h);
    let modes: Vec<PredictionMode> = cands.as_slice().iter().map(|c| c.mode).collect();
    // MVP NEAREST, then ME NEWMV, then GLOBALMV, then the 3x3
    // refinements — C's dispatch order.
    assert_eq!(modes[0], PredictionMode::NearestMv);
    assert_eq!(modes[1], PredictionMode::NewMv);
    assert_eq!(modes[2], PredictionMode::GlobalMv);
    assert!(modes.len() > 3);
    assert!(modes[3..].iter().all(|&m| m == PredictionMode::NewMv));

    // Every stage gate off -> nothing at all.
    let mut ctx = w.ctx();
    ctx.new_nearest_injection = false;
    ctx.new_nearest_near_comb_injection = 0;
    ctx.inject_new_me = false;
    ctx.global_mv_injection = false;
    ctx.unipred3x3_injection = 0;
    ctx.inject_new_pme = false;
    let mut cands = CandArray::new(128);
    let mut log = InjectedMvLog::default();
    inject_inter_candidates(&ctx, &mut cands, &mut log, &mut h);
    assert_eq!(cands.count(), 0);

    // is_intra_bordered + use_neighbouring_mode_ctrls suppresses the
    // MVP stage specifically.
    let mut ctx = w.ctx();
    ctx.inject_new_me = false;
    ctx.is_intra_bordered = true;
    ctx.use_neighbouring_mode_ctrls_enabled = true;
    let mut cands = CandArray::new(128);
    let mut log = InjectedMvLog::default();
    inject_inter_candidates(&ctx, &mut cands, &mut log, &mut h);
    assert_eq!(cands.count(), 0);
}
