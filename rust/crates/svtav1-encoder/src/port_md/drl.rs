//! DRL selection for NEWMV candidates — `svt_aom_choose_best_av1_mv_pred`
//! and the context it prices with.
//!
//! | this module | C |
//! |---|---|
//! | [`av1_drl_ctx`] | `rd_cost.h:85-89` |
//! | [`mv_bit_cost`] | `rd_cost.c:70-78` (`svt_av1_mv_bit_cost`) |
//! | [`mv_bit_cost_light`] | `rd_cost.c:59-65` |
//! | [`choose_best_av1_mv_pred`] | `mode_decision.c:527-617` (EXPORTED) |
//!
//! # Why this matters more than its size suggests
//!
//! `choose_best_av1_mv_pred` picks the DRL index AND the `pred_mv` that
//! every NEWMV candidate is both PRICED against (the MV rate in RD) and
//! WRITTEN against (`blk_ptr->predmv`, which the writer differences the
//! coded MV from). A wrong answer here is simultaneously an RD error and
//! a bitstream error.
//!
//! # Evidence
//!
//! Tier 1 — `tests/c_parity_md_drl.rs` drives the EXPORTED
//! `svt_aom_choose_best_av1_mv_pred` over randomized ref-MV stacks.
//! `av1_drl_ctx` (`static INLINE`) and `svt_av1_mv_bit_cost`
//! (not exported) are reached through it.
//!
//! # Cost tables
//!
//! [`mv_bit_cost`] is written over [`super::pme::MvCostTable`] rather than
//! [`crate::inter_mv_code::mv_bit_cost`]'s `NmvRate`, for the reason that
//! type documents: C's `mv_cost` (rd_cost.c:53) and `svt_mv_cost`
//! (mcomp.h:138) are the SAME lookup with the SAME `CLIP3(MV_LOW, MV_UPP)`
//! clip, and `MvCostTable` carries that clip exactly. The two are
//! numerically identical everywhere a legal candidate can reach; keeping
//! one type here also keeps this lane out of another lane's file.

use super::pme::MvCostTable;
use super::predicates::get_max_drl_index;
use crate::inter_mvp::{DrlMvPred, InterMvpStack, get_av1_mv_pred_drl};
use svtav1_types::motion::{CandidateMv, MAX_REF_MV_STACK_SIZE, Mv};
use svtav1_types::prediction::PredictionMode;

/// C `REF_CAT_LEVEL` (definitions.h:1365).
pub const REF_CAT_LEVEL: i32 = 640;
/// C `DRL_MODE_CONTEXTS` (definitions.h:1343).
pub const DRL_MODE_CONTEXTS: usize = 3;
/// C `MV_COST_WEIGHT` (mode_decision.c:519).
pub const MV_COST_WEIGHT: i32 = 108;

/// C `av1_drl_ctx` (rd_cost.h:85-89).
///
/// Reads `ref_mv_stack[ref_idx]` AND `[ref_idx + 1]`, so the caller must
/// guarantee `ref_idx + 1` is in range — C does not check, and the
/// stack it is handed is always `MAX_REF_MV_STACK_SIZE` long with the
/// unfilled tail zeroed (weight 0 < `REF_CAT_LEVEL`, which is why a
/// short stack still yields a defined context).
#[inline]
pub fn av1_drl_ctx(ref_mv_stack: &[CandidateMv], ref_idx: usize) -> u8 {
    if ref_mv_stack[ref_idx].weight >= REF_CAT_LEVEL {
        if ref_mv_stack[ref_idx + 1].weight >= REF_CAT_LEVEL {
            0
        } else {
            1
        }
    } else if ref_mv_stack[ref_idx + 1].weight < REF_CAT_LEVEL {
        2
    } else {
        0
    }
}

/// C `svt_av1_mv_bit_cost` (rd_cost.c:70-78) over a [`MvCostTable`].
#[inline]
pub fn mv_bit_cost(mv: Mv, ref_mv: Mv, table: &MvCostTable, weight: i32) -> i32 {
    let diff = Mv {
        x: mv.x.wrapping_sub(ref_mv.x),
        y: mv.y.wrapping_sub(ref_mv.y),
    };
    // C: ROUND_POWER_OF_TWO(mv_cost(...) * weight, RDDIV_BITS = 7).
    let v = table.mv_cost(diff) * weight;
    (v + (1 << 6)) >> 7
}

/// C `svt_av1_mv_bit_cost_light` (rd_cost.c:59-65) — the
/// `approx_inter_rate` fast path, table-independent.
#[inline]
pub fn mv_bit_cost_light(mv: Mv, ref_mv: Mv) -> i32 {
    const FACTOR: i32 = 50;
    let absdx = (i32::from(mv.x) - i32::from(ref_mv.x)).abs();
    let absdy = (i32::from(mv.y) - i32::from(ref_mv.y)).abs();
    1296 + FACTOR * (absdx + absdy)
}

/// The MD-context fields `choose_best_av1_mv_pred` reads.
pub struct ChooseDrlCtx<'a> {
    /// C `ctx->shut_fast_rate` — when set, C returns WITHOUT writing
    /// either output.
    pub shut_fast_rate: bool,
    /// C `ctx->approx_inter_rate`. `> 1` short-circuits to DRL 0; `== 1`
    /// selects the light MV cost inside the loop.
    pub approx_inter_rate: u8,
    /// C `ctx->ref_mv_stack[ref_frame]`.
    pub ref_mv_stack: &'a [CandidateMv; MAX_REF_MV_STACK_SIZE],
    /// C `blk_ptr->av1xd->ref_mv_count[ref_frame]`.
    pub ref_mv_count: u8,
    /// C `ctx->md_rate_est_ctx->nmv_vec_cost` + `nmvcoststack`.
    pub nmv_cost: &'a MvCostTable,
    /// C `ctx->md_rate_est_ctx->drl_mode_fac_bits`.
    pub drl_mode_fac_bits: &'a [[i32; 2]; DRL_MODE_CONTEXTS],
}

/// C `svt_aom_choose_best_av1_mv_pred` (mode_decision.c:527-617, EXPORTED).
///
/// `best_drl_index` and `best_pred_mv` are `&mut` because C leaves them
/// UNTOUCHED on the `shut_fast_rate` early return — the caller keeps
/// whatever was there. Returning a value would invent a result C never
/// produces.
///
/// Three details a paraphrase loses:
///
/// * **`approx_inter_rate > 1` and `approx_inter_rate == 1` are different
///   branches.** `> 1` returns DRL 0 with the stack's slot-0 MVs and
///   never enters the loop; `== 1` runs the whole loop but prices with
///   [`mv_bit_cost_light`].
/// * **`max_drl_index == 1` is a separate early path**, not the loop with
///   one iteration: it does NOT consult `get_av1_mv_pred_drl` and takes
///   slot 0's `this_mv`/`comp_mv` directly, which for a compound NEWMV
///   differs from what the loop's `ref_mv` would be.
/// * **The DRL-signalling rate loop `break`s on the FIRST `idx` where
///   `ref_mv_count > idx + 1`**, and only when `drli == idx`. So at most
///   two `drl_mode_fac_bits` terms are added, and which ones depends on
///   `drli`, not on the mode.
pub fn choose_best_av1_mv_pred(
    ctx: &ChooseDrlCtx<'_>,
    mode: PredictionMode,
    mv0: Mv,
    mv1: Mv,
    best_drl_index: &mut u8,
    best_pred_mv: &mut [Mv; 2],
) {
    if ctx.shut_fast_rate {
        return;
    }
    if ctx.approx_inter_rate > 1 {
        *best_drl_index = 0;
        best_pred_mv[0] = ctx.ref_mv_stack[0].this_mv;
        best_pred_mv[1] = ctx.ref_mv_stack[0].comp_mv;
        return;
    }

    let is_compound = crate::inter_mv_code::is_inter_compound_mode(mode);
    let max_drl_index = get_max_drl_index(ctx.ref_mv_count, mode);

    if max_drl_index == 1 {
        *best_drl_index = 0;
        best_pred_mv[0] = ctx.ref_mv_stack[0].this_mv;
        best_pred_mv[1] = ctx.ref_mv_stack[0].comp_mv;
        return;
    }

    // C carries `nearestmv`/`nearmv` across loop iterations: `nearestmv`
    // is zero-initialised at declaration and `nearmv` is UNINITIALISED,
    // but every branch of get_av1_mv_pred_drl that a reachable mode takes
    // writes what it later reads. The port threads the previous
    // iteration's values in, which is what C's stack slots hold.
    let mut carried = DrlMvPred {
        nearestmv: [Mv::ZERO; 2],
        nearmv: [Mv::ZERO; 2],
        ref_mv: [Mv::ZERO; 2],
    };
    let stack = InterMvpStack {
        stack: *ctx.ref_mv_stack,
        count: ctx.ref_mv_count,
        mode_context: 0,
        mv_ref0: [Mv::ZERO; 64],
    };

    let mut best_mv_cost: u32 = 0xFFFF_FFFF;
    for drli in 0..max_drl_index {
        let pred = get_av1_mv_pred_drl(&stack, is_compound, mode as u8, usize::from(drli), carried);
        carried = pred;
        let ref_mv = pred.ref_mv;

        let mut mv_rate: u32 = if ctx.approx_inter_rate != 0 {
            mv_bit_cost_light(mv0, ref_mv[0]) as u32
        } else {
            mv_bit_cost(mv0, ref_mv[0], ctx.nmv_cost, MV_COST_WEIGHT) as u32
        };

        if is_compound {
            mv_rate = mv_rate.wrapping_add(if ctx.approx_inter_rate != 0 {
                mv_bit_cost_light(mv1, ref_mv[1]) as u32
            } else {
                mv_bit_cost(mv1, ref_mv[1], ctx.nmv_cost, MV_COST_WEIGHT) as u32
            });
        }

        let new_mv = mode == PredictionMode::NewMv || mode == PredictionMode::NewNewMv;
        if new_mv {
            for idx in 0..2usize {
                if usize::from(ctx.ref_mv_count) > idx + 1 {
                    let drl_1_ctx = av1_drl_ctx(ctx.ref_mv_stack, idx);
                    mv_rate = mv_rate.wrapping_add(
                        ctx.drl_mode_fac_bits[usize::from(drl_1_ctx)]
                            [usize::from(usize::from(drli) != idx)] as u32,
                    );
                    if usize::from(drli) == idx {
                        break;
                    }
                }
            }
        }

        if mv_rate < best_mv_cost {
            best_mv_cost = mv_rate;
            *best_drl_index = drli;
            best_pred_mv[0] = ref_mv[0];
            best_pred_mv[1] = ref_mv[1];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmv(w: i32) -> CandidateMv {
        CandidateMv {
            this_mv: Mv::ZERO,
            comp_mv: Mv::ZERO,
            weight: w,
        }
    }

    /// TIER 4 — `av1_drl_ctx` is `static INLINE` in rd_cost.h. It is also
    /// reached at tier 1 through `svt_aom_choose_best_av1_mv_pred`; these
    /// vectors pin the four-way table directly because the tier-1 test
    /// only observes it through a cost comparison.
    #[test]
    fn tier4_av1_drl_ctx_truth_table() {
        let hi = REF_CAT_LEVEL;
        let lo = REF_CAT_LEVEL - 1;
        // [i] >= LEVEL, [i+1] >= LEVEL -> 0
        assert_eq!(av1_drl_ctx(&[cmv(hi), cmv(hi)], 0), 0);
        // [i] >= LEVEL, [i+1] <  LEVEL -> 1
        assert_eq!(av1_drl_ctx(&[cmv(hi), cmv(lo)], 0), 1);
        // [i] <  LEVEL, [i+1] <  LEVEL -> 2
        assert_eq!(av1_drl_ctx(&[cmv(lo), cmv(lo)], 0), 2);
        // [i] <  LEVEL, [i+1] >= LEVEL -> 0 (NOT 2 — the nested ternary's
        // else branch collapses back to 0)
        assert_eq!(av1_drl_ctx(&[cmv(lo), cmv(hi)], 0), 0);
    }

    /// TIER 4 — `svt_av1_mv_bit_cost_light` (rd_cost.c:59).
    #[test]
    fn tier4_mv_bit_cost_light_formula() {
        assert_eq!(mv_bit_cost_light(Mv::ZERO, Mv::ZERO), 1296);
        assert_eq!(
            mv_bit_cost_light(Mv { x: 3, y: -4 }, Mv::ZERO),
            1296 + 50 * 7
        );
    }
}
