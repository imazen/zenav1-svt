use super::super::pme::MvCostTable;
use super::*;
use svtav1_types::motion::{CandidateMv, MAX_REF_MV_STACK_SIZE};

fn mv(x: i16, y: i16) -> Mv {
    Mv { x, y }
}

fn zero_table() -> MvCostTable {
    MvCostTable::zeroed()
}

struct DrlFixture {
    stack: [CandidateMv; MAX_REF_MV_STACK_SIZE],
    table: MvCostTable,
    fac: [[i32; 2]; 3],
}

impl DrlFixture {
    fn new() -> Self {
        Self {
            stack: [CandidateMv::default(); MAX_REF_MV_STACK_SIZE],
            table: zero_table(),
            fac: [[0; 2]; 3],
        }
    }
    fn ctx(&self) -> ChooseDrlCtx<'_> {
        ChooseDrlCtx {
            shut_fast_rate: false,
            approx_inter_rate: 0,
            ref_mv_stack: &self.stack,
            ref_mv_count: 1,
            nmv_cost: &self.table,
            drl_mode_fac_bits: &self.fac,
        }
    }
}

/// TIER 4 — `error_per_bit` floors at 1 via an increment, so any
/// lambda below 64 gives exactly 1.
#[test]
fn tier4_error_per_bit_floors_at_one() {
    assert_eq!(error_per_bit(0), 1);
    assert_eq!(error_per_bit(63), 1);
    assert_eq!(error_per_bit(64), 1);
    assert_eq!(error_per_bit(128), 2);
    assert_eq!(error_per_bit(1 << 20), 1 << 14);
}

/// TIER 4 — the neighbour list's ORDER: centre first, then the four
/// axis positions, then the four diagonals, so truncating at 5 is
/// exactly "no diagonals".
#[test]
fn tier4_wm_neighbor_order() {
    assert_eq!(WM_NEIGHBORS[0], (0, 0));
    for &(x, y) in &WM_NEIGHBORS[1..5] {
        assert!(x == 0 || y == 0, "positions 1..5 must be axis-aligned");
    }
    for &(x, y) in &WM_NEIGHBORS[5..9] {
        assert!(x != 0 && y != 0, "positions 5..9 must be diagonals");
    }
}

/// TIER 4 — the centre is searched only on the first iteration, the
/// step scales with MV precision, and `refine_diag` truncates at 5.
#[test]
fn tier4_wm_refinement_walk() {
    let f = DrlFixture::new();
    let base = |iters: u8, diag: bool, hp: bool| WmRefineCtx {
        refinement_iterations: iters,
        refine_diag: diag,
        allow_high_precision_mv: hp,
        approx_inter_rate: 1, // the light cost: no table needed
        corrupted_mv_check: false,
        error_per_bit: 1,
        drl: f.ctx(),
    };

    // One iteration, no diagonals: centre + four axis positions.
    let mut seen: Vec<Mv> = Vec::new();
    let r = wm_motion_refinement(
        &base(1, false, false),
        mv(64, 64),
        Mv::ZERO,
        PredictionMode::NewMv,
        |m| {
            seen.push(m);
            Some(1000)
        },
    );
    assert_eq!(seen.len(), 5);
    assert_eq!(r.positions_checked, 5);
    assert_eq!(seen[0], mv(64, 64), "the centre is searched first");
    // Quarter-pel precision: the step is 2 eighth-pel units.
    assert_eq!(seen[1], mv(62, 64));

    // Eighth-pel precision: the step is 1.
    let mut seen: Vec<Mv> = Vec::new();
    wm_motion_refinement(
        &base(1, false, true),
        mv(64, 64),
        Mv::ZERO,
        PredictionMode::NewMv,
        |m| {
            seen.push(m);
            Some(1000)
        },
    );
    assert_eq!(seen[1], mv(63, 64));

    // With diagonals: nine positions.
    let mut seen: Vec<Mv> = Vec::new();
    wm_motion_refinement(
        &base(1, true, false),
        mv(64, 64),
        Mv::ZERO,
        PredictionMode::NewMv,
        |m| {
            seen.push(m);
            Some(1000)
        },
    );
    assert_eq!(seen.len(), 9);

    // Zero iterations: nothing is searched and the MV is unchanged.
    let r = wm_motion_refinement(
        &base(0, true, false),
        mv(64, 64),
        Mv::ZERO,
        PredictionMode::NewMv,
        |_| panic!("must not evaluate with zero iterations"),
    );
    assert_eq!(r.best_mv, mv(64, 64));
    assert_eq!(r.positions_checked, 0);
}

/// TIER 4 — the second iteration skips the centre AND anything
/// already recorded, and the loop stops once the centre stops moving.
#[test]
fn tier4_wm_refinement_dedup_and_early_break() {
    let f = DrlFixture::new();
    let ctx = WmRefineCtx {
        refinement_iterations: 4,
        refine_diag: false,
        allow_high_precision_mv: true,
        approx_inter_rate: 1,
        corrupted_mv_check: false,
        error_per_bit: 1,
        drl: f.ctx(),
    };
    // Every position costs the same, so the centre never moves and
    // the loop breaks after iteration 0 — five evaluations, not 20.
    let mut count = 0usize;
    let r = wm_motion_refinement(&ctx, mv(0, 0), Mv::ZERO, PredictionMode::NewMv, |_| {
        count += 1;
        Some(1000)
    });
    assert_eq!(count, 5);
    assert_eq!(r.best_mv, mv(0, 0));

    // A closure that rewards moving left keeps the search going and
    // never re-evaluates a recorded position. The reward has to beat
    // the MV rate the search adds on top (the light cost is
    // 1296 + 50 * L1), which is itself the point: a small distortion
    // gradient does NOT move the MV.
    let mut seen: Vec<Mv> = Vec::new();
    let r = wm_motion_refinement(&ctx, mv(0, 0), Mv::ZERO, PredictionMode::NewMv, |m| {
        seen.push(m);
        Some(1_000_000 + 1000 * i32::from(m.x))
    });
    let mut sorted: Vec<u32> = seen.iter().map(|m| m.as_int()).collect();
    let before = sorted.len();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), before, "no position is evaluated twice");
    assert!(r.best_mv.x < 0, "the search moved toward the cheaper side");
}

/// TIER 4 — a position whose warp is REJECTED still consumes a
/// record slot, so it is never retried in a later iteration.
#[test]
fn tier4_wm_refinement_records_rejected_positions() {
    let f = DrlFixture::new();
    let ctx = WmRefineCtx {
        refinement_iterations: 3,
        refine_diag: false,
        allow_high_precision_mv: true,
        approx_inter_rate: 1,
        corrupted_mv_check: false,
        error_per_bit: 1,
        drl: f.ctx(),
    };
    let mut seen: Vec<Mv> = Vec::new();
    let r = wm_motion_refinement(&ctx, mv(0, 0), Mv::ZERO, PredictionMode::NewMv, |m| {
        seen.push(m);
        // Reject everything: no position ever wins.
        None
    });
    // Iteration 0 evaluates 5; the centre does not move, so the loop
    // breaks — every one of those 5 is recorded even though all were
    // rejected.
    assert_eq!(seen.len(), 5);
    assert_eq!(r.positions_checked, 5);
    assert_eq!(r.best_mv, mv(0, 0));
}

/// TIER 4 — `corrupted_mv_check` decides the return value, and it is
/// checked AFTER the DRL re-pick.
#[test]
fn tier4_wm_refinement_validity_check() {
    let f = DrlFixture::new();
    let ctx = WmRefineCtx {
        refinement_iterations: 1,
        refine_diag: false,
        allow_high_precision_mv: true,
        approx_inter_rate: 1,
        corrupted_mv_check: true,
        error_per_bit: 1,
        drl: f.ctx(),
    };
    let r = wm_motion_refinement(&ctx, mv(8, 8), Mv::ZERO, PredictionMode::NewMv, |_| Some(0));
    assert!(r.valid);
    // With the check OFF the answer is unconditionally valid.
    let ctx_off = WmRefineCtx {
        corrupted_mv_check: false,
        ..ctx
    };
    assert!(
        wm_motion_refinement(&ctx_off, mv(8, 8), Mv::ZERO, PredictionMode::NewMv, |_| {
            Some(0)
        })
        .valid
    );
}

/// TIER 4 — the refine-level dispatch, including that an unknown
/// level does NEITHER search rather than asserting.
#[test]
fn tier4_single_motion_search_plan() {
    for lvl in [0, 1, 3] {
        let p = single_motion_search_plan(lvl);
        assert!(p.do_full_refine && p.do_frac_refine, "level {lvl}");
    }
    for lvl in [2, 4] {
        let p = single_motion_search_plan(lvl);
        assert!(!p.do_full_refine && p.do_frac_refine, "level {lvl}");
    }
    for lvl in [5, -1, 99] {
        let p = single_motion_search_plan(lvl);
        assert!(!p.do_full_refine && !p.do_frac_refine, "level {lvl}");
    }
    // The sub-pel call site's literals.
    let p = single_motion_search_plan(0);
    assert!(
        p.subpel_use_8_taps,
        "mode_decision.c:2148 passes USE_8_TAPS"
    );
    assert_eq!(p.subpel_force_stop, 0);
    assert_eq!(p.subpel_iters_per_step, 2);
}

/// TIER 4 — the two `else` branches are precision conversions, not
/// no-ops.
#[test]
fn tier4_single_motion_search_fallback_mv() {
    let pred = mv(72, -72);
    // Both refines on: the MV is handed to the kernels unchanged.
    assert_eq!(
        single_motion_search_fallback_mv(single_motion_search_plan(0), pred),
        pred
    );
    // Full-pel skipped, fractional on: >> 3 only.
    assert_eq!(
        single_motion_search_fallback_mv(single_motion_search_plan(2), pred),
        mv(9, -9)
    );
    // Neither: >> 3 then * 8, which is NOT the identity for a
    // non-multiple of 8.
    assert_eq!(
        single_motion_search_fallback_mv(single_motion_search_plan(5), mv(70, -70)),
        mv(64, -72)
    );
}

/// TIER 4 — `mi_row`/`mi_col` come from the NEGATED edges, and C's
/// integer division truncates toward zero.
#[test]
fn tier4_mi_row_col_from_edges() {
    // mb_to_top_edge = -((mi_row * MI_SIZE) * 8) for mi_row = 5.
    assert_eq!(mi_row_col_from_edges(-(5 * 4 * 8), -(3 * 4 * 8)), (5, 3));
    assert_eq!(mi_row_col_from_edges(0, 0), (0, 0));
}

/// TIER 4 — a block too large to refine returns VALID with its
/// original MV; returning invalid would DROP the candidate.
#[test]
fn tier4_obmc_refinement_size_gate_returns_valid() {
    let f = DrlFixture::new();
    let drl = f.ctx();
    let r = obmc_motion_refinement(
        64,
        64,
        32,
        true,
        &drl,
        PredictionMode::NewMv,
        mv(8, 8),
        |_| panic!("must not search a block above the refine cap"),
    );
    assert!(r.skipped);
    assert!(r.valid, "an unrefinable block is KEPT, not dropped");
    assert_eq!(r.best_mv, mv(8, 8));

    // Within the cap the search runs and the DRL is re-picked.
    let r = obmc_motion_refinement(
        16,
        16,
        32,
        false,
        &drl,
        PredictionMode::NewMv,
        mv(8, 8),
        |_| mv(16, 16),
    );
    assert!(!r.skipped);
    assert_eq!(r.best_mv, mv(16, 16));
    assert!(r.valid);

    // Either dimension above the cap trips the gate.
    assert!(obmc_refinement_skipped_as_valid(64, 16, 32));
    assert!(obmc_refinement_skipped_as_valid(16, 64, 32));
    assert!(!obmc_refinement_skipped_as_valid(32, 32, 32));
}
