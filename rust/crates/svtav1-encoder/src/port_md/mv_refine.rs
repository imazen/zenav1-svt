//! The MD motion-mode MV refinements — `svt_aom_wm_motion_refinement`,
//! `svt_aom_obmc_motion_refinement` and the `single_motion_search` that
//! drives the OBMC one.
//!
//! | this module | C |
//! |---|---|
//! | [`WM_NEIGHBORS`] / [`wm_motion_refinement`] | `mode_decision.c:1873-2011` (EXPORTED) |
//! | [`single_motion_search_plan`] | `mode_decision.c:2069-2181` |
//! | [`obmc_motion_refinement`] | `mode_decision.c:2183-2286` (EXPORTED) |
//!
//! # Why these matter
//!
//! `crate::inter_me::obmc_search` already ports
//! `svt_av1_obmc_full_pixel_search` and
//! `svt_av1_find_best_obmc_sub_pixel_tree_up`, and `svtav1-dsp`'s warp
//! kernels are ported and C-gated — and **nothing in the encoder calls
//! any of them**. These two refinements are what call them and write the
//! refined MV back onto the candidate, so without them those kernels stay
//! unreachable and no WARPED_CAUSAL / OBMC_CAUSAL candidate ever moves
//! off its injected MV.
//!
//! # Evidence
//!
//! **Tier 4.** Both refinements ARE exported, but their bodies are
//! prediction loops over `svt_aom_inter_prediction` /
//! `calc_target_weighted_pred` and the OBMC search kernels — driving the
//! export would require a full reference picture, a populated
//! `ModeDecisionContext` with OBMC prediction buffers, and the neighbour
//! recon arrays. What is portable is the SEARCH GEOMETRY and the
//! bookkeeping, and those are ported here with the prediction/variance
//! taken as a closure, exactly as [`super::inject::InjectHooks`] does.
//! The pieces WITH an oracle are called rather than re-transcribed:
//! [`super::drl::choose_best_av1_mv_pred`] (tier 1),
//! [`super::predicates::is_valid_mv_diff`], and
//! [`super::pme::get_sad_per_bit`] (tier 1).
//!
//! # A trap this port does NOT step in
//!
//! `crate::inter_me::obmc_search` records that `mode_decision.c:2148`
//! passes **`USE_8_TAPS`** to `svt_av1_find_best_obmc_sub_pixel_tree_up`,
//! so the UPSAMPLED sub-pel branch is the live one — not the cheap
//! `osvf` one. [`single_motion_search_plan`] carries that as a named
//! field so the caller cannot pick the other by default.

use super::drl::{ChooseDrlCtx, choose_best_av1_mv_pred};
use super::predicates::is_valid_mv_diff;
use svtav1_types::motion::Mv;
use svtav1_types::prediction::PredictionMode;

/// C `neighbors[9]` (mode_decision.c:1875-1876): the centre first, then
/// the four axis neighbours, then the four diagonals.
///
/// The ORDER is load-bearing twice over: the centre is searched only on
/// the first iteration (`i = iter ? 1 : 0`), and `refine_diag` truncates
/// the list at **5**, which is exactly the centre plus the four axis
/// positions.
pub const WM_NEIGHBORS: [(i16, i16); 9] = [
    (0, 0),
    (-1, 0),
    (0, 1),
    (1, 0),
    (0, -1),
    (1, -1),
    (1, 1),
    (-1, 1),
    (-1, -1),
];

/// C `RD_EPB_SHIFT` (restoration.h:342).
pub const RD_EPB_SHIFT: u32 = 6;

/// C's `error_per_bit` derivation, shared by both refinements
/// (mode_decision.c:1883-1884 and :2110-2111).
///
/// `full_lambda >> 6`, then `+= (x == 0)` — a floor of 1 written as an
/// increment, so a lambda below 64 gives exactly 1.
#[inline]
pub fn error_per_bit(full_lambda: u32) -> i32 {
    let mut e = (full_lambda >> RD_EPB_SHIFT) as i32;
    e += i32::from(e == 0);
    e
}

/// The MD-context fields [`wm_motion_refinement`] reads.
pub struct WmRefineCtx<'a> {
    /// C `ctx->wm_ctrls.refinement_iterations`.
    pub refinement_iterations: u8,
    /// C `ctx->wm_ctrls.refine_diag` — false truncates the neighbour list
    /// at 5.
    pub refine_diag: bool,
    /// C `pcs->ppcs->frm_hdr.allow_high_precision_mv`; false makes the
    /// step 2 eighth-pel units instead of 1.
    pub allow_high_precision_mv: bool,
    /// C `ctx->approx_inter_rate`.
    pub approx_inter_rate: u8,
    /// C `ctx->corrupted_mv_check`.
    pub corrupted_mv_check: bool,
    /// C's `error_per_bit` = [`error_per_bit`] of the 8-bit full lambda.
    /// Carried on the context so the caller states the lambda once.
    pub error_per_bit: i32,
    /// The DRL context [`choose_best_av1_mv_pred`] needs.
    pub drl: ChooseDrlCtx<'a>,
}

/// What [`wm_motion_refinement`] produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WmRefineResult {
    pub best_mv: Mv,
    pub drl_index: u8,
    pub pred_mv: [Mv; 2],
    /// C's return value: 1 when the final MV is valid.
    pub valid: bool,
    /// How many distinct positions the search actually evaluated —
    /// exposed so a test can assert the walk, not just its answer.
    pub positions_checked: usize,
}

/// C `svt_aom_wm_motion_refinement` (mode_decision.c:1873-2011,
/// EXPORTED).
///
/// `evaluate(test_mv)` performs C's warp-parameter derivation plus
/// prediction plus variance: it returns `None` when
/// `svt_aom_warped_motion_parameters` rejects the MV (C `continue`s), and
/// `Some(var)` otherwise. The MV RATE is added here, not by the closure,
/// because it is `svt_aom_mv_err_cost{,_light}` over the same tables the
/// DRL pick uses.
///
/// Five details a paraphrase loses:
///
/// * **The centre is searched only on iteration 0** (`i = iter ? 1 : 0`).
/// * **`refine_diag == false` truncates at 5**, giving centre + four
///   axis neighbours.
/// * **The step is `1 << mv_prec_shift`** where the shift is
///   `allow_high_precision_mv ? 0 : 1`.
/// * **A rejected warp still consumes a `mv_record` slot** — C writes
///   the record BEFORE calling `warped_motion_parameters`, so a position
///   that fails validation is never retried in a later iteration.
/// * **The loop breaks when an iteration does not move the centre**, and
///   `prev_mv` is the PREVIOUS centre, so the dedup skips the previous
///   centre explicitly as well as anything in the record.
pub fn wm_motion_refinement(
    ctx: &WmRefineCtx<'_>,
    cand_mv: Mv,
    cand_pred_mv: Mv,
    mode: PredictionMode,
    mut evaluate: impl FnMut(Mv) -> Option<i32>,
) -> WmRefineResult {
    let mv_prec_shift = u32::from(!ctx.allow_high_precision_mv);
    let mut best_cost = i32::MAX;
    let mut search_centre_mv = cand_mv;
    let mut best_mv = cand_mv;
    let mut prev_mv = cand_mv;
    let ref_mv = cand_pred_mv;

    // C's `uint32_t mv_record[256]` with an unbounded `tot_checked_pos`;
    // the port uses a Vec so an overrun is impossible rather than
    // silently corrupting the stack.
    let mut mv_record: Vec<u32> = Vec::new();

    for iter in 0..usize::from(ctx.refinement_iterations) {
        let start = usize::from(iter != 0);
        let end = if ctx.refine_diag { 9 } else { 5 };
        for &(nx, ny) in WM_NEIGHBORS.iter().take(end).skip(start) {
            let test_mv = Mv {
                x: search_centre_mv
                    .x
                    .wrapping_add(nx.wrapping_mul(1 << mv_prec_shift)),
                y: search_centre_mv
                    .y
                    .wrapping_add(ny.wrapping_mul(1 << mv_prec_shift)),
            };
            if iter != 0 {
                if prev_mv.as_int() == test_mv.as_int() {
                    continue;
                }
                if mv_record.contains(&test_mv.as_int()) {
                    continue;
                }
            }
            // C records the position BEFORE validating the warp.
            mv_record.push(test_mv.as_int());
            let Some(var) = evaluate(test_mv) else {
                continue;
            };
            // C: `svt_aom_mv_err_cost{,_light}(&test_mv, &ref_mv, ...)`
            // — the SSD-domain search cost (av1me.c), the ENTROPY arm of
            // the one `mv_err_cost` body, over the same nmv tables the DRL
            // pick uses.
            let rate = if ctx.approx_inter_rate != 0 {
                super::drl::mv_bit_cost_light(test_mv, ref_mv)
            } else {
                crate::intrabc::mv_err_cost(test_mv, ref_mv, ctx.drl.nmv_cost, ctx.error_per_bit)
            };
            let cost = var.saturating_add(rate);
            if cost < best_cost {
                best_mv = test_mv;
                best_cost = cost;
            }
        }
        prev_mv = search_centre_mv;
        search_centre_mv = best_mv;
        if prev_mv.as_int() == best_mv.as_int() {
            break;
        }
    }

    let mut drl_index = 0u8;
    let mut pred_mv = [Mv::ZERO; 2];
    choose_best_av1_mv_pred(
        &ctx.drl,
        mode,
        best_mv,
        Mv::ZERO,
        &mut drl_index,
        &mut pred_mv,
    );

    let valid = !ctx.corrupted_mv_check || is_valid_mv_diff(pred_mv, best_mv, best_mv, false);
    WmRefineResult {
        best_mv,
        drl_index,
        pred_mv,
        valid,
        positions_checked: mv_record.len(),
    }
}

/// C `single_motion_search`'s refine-level dispatch
/// (mode_decision.c:2072-2087).
///
/// Levels 0, 1 and 3 do BOTH a full-pel and a fractional refinement;
/// levels 2 and 4 do only the fractional one; anything else does
/// NEITHER — and C's `default: break` leaves both flags false, so an
/// unknown level silently skips the whole search rather than asserting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SingleMotionSearchPlan {
    pub do_full_refine: bool,
    pub do_frac_refine: bool,
    /// C passes `USE_8_TAPS` at mode_decision.c:2148, so the UPSAMPLED
    /// sub-pel branch is the live one. Carried explicitly because the
    /// cheap `osvf` branch exists and picking it would silently change
    /// every refined OBMC MV.
    pub subpel_use_8_taps: bool,
    /// C `mv.subpel_force_stop` — a literal 0 at the call site.
    pub subpel_force_stop: u8,
    /// C `mv.subpel_iters_per_step` — a literal 2 at the call site.
    pub subpel_iters_per_step: u8,
}

/// C `single_motion_search` (mode_decision.c:2069-2181), the parts that
/// are not the two search kernels.
///
/// **The `else` branches are not no-ops.** Without the full-pel refine,
/// `best_mv` is the predicted MV shifted RIGHT by 3 (to full pel);
/// without the fractional refine, it is then multiplied by 8 back to
/// eighth-pel. Skipping either leaves the MV in the wrong precision.
pub fn single_motion_search_plan(refine_level: i32) -> SingleMotionSearchPlan {
    let (full, frac) = match refine_level {
        0 | 1 | 3 => (true, true),
        2 | 4 => (false, true),
        _ => (false, false),
    };
    SingleMotionSearchPlan {
        do_full_refine: full,
        do_frac_refine: frac,
        subpel_use_8_taps: true,
        subpel_force_stop: 0,
        subpel_iters_per_step: 2,
    }
}

/// C `single_motion_search`'s non-refined MV paths
/// (mode_decision.c:2137-2139 and :2172-2175).
///
/// Returns the MV the search would produce with the given plan and no
/// kernels: the `>> 3` when the full-pel search is skipped, and the `* 8`
/// when the fractional one is.
pub fn single_motion_search_fallback_mv(plan: SingleMotionSearchPlan, best_pred_mv: Mv) -> Mv {
    let mut mv = if plan.do_full_refine {
        best_pred_mv
    } else {
        Mv {
            x: best_pred_mv.x >> 3,
            y: best_pred_mv.y >> 3,
        }
    };
    if !plan.do_frac_refine {
        mv = Mv {
            x: mv.x.wrapping_mul(8),
            y: mv.y.wrapping_mul(8),
        };
    }
    mv
}

/// C `single_motion_search`'s MV-limit derivation
/// (mode_decision.c:2101-2106) — identical arithmetic to
/// [`super::md_search::subpel_mv_limits`], but derived from
/// `mb_to_top_edge` / `mb_to_left_edge` rather than from `mi_row` /
/// `mi_col` directly.
///
/// `mi_row = -mb_to_top_edge / (8 * MI_SIZE)` — an integer division of a
/// NEGATIVE numerator in C, which truncates toward zero; the port keeps
/// that by dividing after negating.
#[inline]
pub fn mi_row_col_from_edges(mb_to_top_edge: i32, mb_to_left_edge: i32) -> (i32, i32) {
    const MI_SIZE: i32 = 4;
    (
        (-mb_to_top_edge) / (8 * MI_SIZE),
        (-mb_to_left_edge) / (8 * MI_SIZE),
    )
}

/// C `svt_aom_obmc_motion_refinement`'s block-size gate
/// (mode_decision.c:2184-2188).
///
/// **A block too large to refine returns 1 (VALID), not 0** — the
/// candidate is kept with its unrefined MV. Returning 0 would drop it.
#[inline]
pub fn obmc_refinement_skipped_as_valid(
    bwidth: u16,
    bheight: u16,
    max_blk_size_to_refine: u8,
) -> bool {
    bwidth > u16::from(max_blk_size_to_refine) || bheight > u16::from(max_blk_size_to_refine)
}

/// What [`obmc_motion_refinement`] produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObmcRefineResult {
    pub best_mv: Mv,
    pub drl_index: u8,
    pub pred_mv: [Mv; 2],
    pub valid: bool,
    /// True when the block-size gate short-circuited the whole thing.
    pub skipped: bool,
}

/// C `svt_aom_obmc_motion_refinement` (mode_decision.c:2183-2286,
/// EXPORTED), everything but the two search kernels and the OBMC
/// weighted-prediction precompute.
///
/// `search(best_mv) -> Mv` stands for `single_motion_search`, which needs
/// the OBMC full-pel and sub-pel kernels
/// ([`crate::inter_me::obmc_search`]) plus the target weighted
/// prediction; the caller supplies it. Everything around it — the size
/// gate, the DRL re-pick and the MV-validity check — is here.
pub fn obmc_motion_refinement(
    bwidth: u16,
    bheight: u16,
    max_blk_size_to_refine: u8,
    corrupted_mv_check: bool,
    drl: &ChooseDrlCtx<'_>,
    mode: PredictionMode,
    cand_mv: Mv,
    search: impl FnOnce(Mv) -> Mv,
) -> ObmcRefineResult {
    if obmc_refinement_skipped_as_valid(bwidth, bheight, max_blk_size_to_refine) {
        return ObmcRefineResult {
            best_mv: cand_mv,
            drl_index: 0,
            pred_mv: [Mv::ZERO; 2],
            valid: true,
            skipped: true,
        };
    }
    let best_mv = search(cand_mv);
    let mut drl_index = 0u8;
    let mut pred_mv = [Mv::ZERO; 2];
    choose_best_av1_mv_pred(drl, mode, best_mv, Mv::ZERO, &mut drl_index, &mut pred_mv);
    let valid = !corrupted_mv_check || is_valid_mv_diff(pred_mv, best_mv, best_mv, false);
    ObmcRefineResult {
        best_mv,
        drl_index,
        pred_mv,
        valid,
        skipped: false,
    }
}

// ---------------------------------------------------------------------------
// TIER 4 — both refinements are exported, but their bodies are prediction
// loops that need a reference picture and populated OBMC buffers. These
// vectors pin the SEARCH GEOMETRY and the bookkeeping, which is what is
// portable; the pieces with an oracle (choose_best_av1_mv_pred,
// is_valid_mv_diff, get_sad_per_bit, mv_err_cost) are called, not
// re-transcribed.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
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
}
