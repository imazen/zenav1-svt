//! Motion-mode refinement, inter-intra, and the PD0 staging loop.
//!
//! | this module | C |
//! |---|---|
//! | [`generate_md_stage_0_cand_pd0`] | `mode_decision.c:3494-3521` (EXPORTED) |
//! | [`fast_loop_core_pd0_cost`] | `product_coding_loop.c:964-1005` |
//! | [`md_stage_0_pd0`] | `product_coding_loop.c:1507-1523` |
//! | [`md_stage_3_pd0_subres_step`] | `product_coding_loop.c:7100-7115` |
//! | [`update_refined_mv_fast_rate`] | `product_coding_loop.c:6741-6755` |
//! | [`warp_refine_stage`] / [`obmc_refine_stage`] / [`opt_non_translation_motion_mode`] | `product_coding_loop.c:6757-6825` |
//! | [`obmc_trans_face_off`] | `product_coding_loop.c:1068-1173` |
//! | [`motion_mode_allowed`] | `entropy_coding.c:1159-1195` |
//! | [`setup_pred_plane`] | `mode_decision.c:2013-2030` |
//! | [`inter_intra_search`] | `mode_decision.c:326-492` |
//! | [`pick_interintra_wedge`] | `mode_decision.c:297-323` |
//!
//! # A `#if` that had to be checked, not assumed
//!
//! `fast_loop_core_pd0` has an `#if SVT_HDR_MODE` arm computing a
//! spatial-full-distortion over SUBSAMPLED rows and an `#else` arm
//! computing the plain variance — **different distortions**.
//! `SVT_HDR_MODE` defaults to **0** (`EbDebugMacros.h:53-54`), so the
//! `#else` is what mainline compiles, and that is what
//! [`fast_loop_core_pd0_cost`] ports. The fork arm is named in the doc
//! and deliberately NOT implemented, because implementing the wrong arm
//! is exactly the failure this group's brief warns about.
//!
//! # Evidence
//!
//! **Tier 4** for everything except `generate_md_stage_0_cand_pd0`,
//! which IS exported — but its body is four calls into `static`
//! injectors (`inject_intra_candidates_pd0`,
//! [`super::inject::inject_inter_candidates_pd0`],
//! [`super::inject::inject_zz_backup_candidate`],
//! `reject_candidate_sframe`), so driving the export would measure those
//! rather than the dispatch. What is portable and testable is the
//! DISPATCH, and that is what [`generate_md_stage_0_cand_pd0`] is: a
//! decision function returning which stages run, with the injectors as
//! caller-supplied closures.
//!
//! The pixel-domain calls (`svt_aom_inter_prediction`, `svt_aom_sse`,
//! `pick_wedge_fixed_sign`, `svt_aom_inter_pu_prediction_av1_obmc`) are
//! [`super::inject::InjectHooks`]-style parameters, for the same reason
//! stated there: a caller without them must say so.

use super::inject::{CandArray, InterCandidate};
use super::pme::MvCostTable;
use super::predicates::{MotionMode, is_global_mv_block, is_motion_variation_allowed_bsize};
use svtav1_types::motion::{Mv, TransformationType, WarpedMotionParams};
use svtav1_types::prediction::PredictionMode;

/// C `MV_COST_WEIGHT` (mode_decision.c:519).
pub const MV_COST_WEIGHT: i32 = 108;

// ---------------------------------------------------------------------------
// motion_mode_allowed (entropy_coding.c:1159-1195)
// ---------------------------------------------------------------------------

/// C `svt_aom_motion_mode_allowed` (entropy_coding.c:1159-1195).
///
/// The WRITER-side predicate, distinct from
/// [`super::predicates::obmc_motion_mode_allowed`]: this one has no
/// `obmc_ctrls` at all (it is what the bitstream permits, not what the
/// encoder chose to search) and it can return `WARPED_CAUSAL`, which the
/// MD-side one never does.
///
/// `num_proj_ref >= 1` plus `allow_warped_motion` is what promotes
/// OBMC_CAUSAL to WARPED_CAUSAL — and `force_integer_mv` demotes it back,
/// because a warp cannot be expressed at integer MV precision.
#[allow(clippy::too_many_arguments)]
pub fn motion_mode_allowed(
    is_motion_mode_switchable: bool,
    force_integer_mv: u8,
    allow_warped_motion: bool,
    gm_wmtype: TransformationType,
    num_proj_ref: u16,
    overlappable_neighbors: u32,
    bsize: u8,
    rf1: i8,
    mode: u8,
) -> MotionMode {
    if !is_motion_mode_switchable {
        return MotionMode::SimpleTranslation;
    }
    if force_integer_mv == 0 && is_global_mv_block(mode, bsize, gm_wmtype) {
        return MotionMode::SimpleTranslation;
    }
    if is_motion_variation_allowed_bsize(bsize)
        && super::predicates::is_inter_singleref_mode(mode)
        && rf1 != 0
        && !(rf1 > 0)
    {
        if overlappable_neighbors == 0 {
            return MotionMode::SimpleTranslation;
        }
        if allow_warped_motion && num_proj_ref >= 1 {
            if force_integer_mv != 0 {
                return MotionMode::ObmcCausal;
            }
            return MotionMode::WarpedCausal;
        }
        return MotionMode::ObmcCausal;
    }
    MotionMode::SimpleTranslation
}

// ---------------------------------------------------------------------------
// setup_pred_plane (mode_decision.c:2013-2030)
// ---------------------------------------------------------------------------

/// C `Buf2D` as [`setup_pred_plane`] fills it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Buf2D {
    /// C `buf` as an OFFSET from `buf0`, because a Rust port cannot carry
    /// an interior pointer.
    pub offset: usize,
    pub width: i32,
    pub height: i32,
    pub stride: i32,
}

/// C `setup_pred_plane` (mode_decision.c:2013-2030).
///
/// The two `mi_row -= 1` / `mi_col -= 1` nudges fire only for a
/// SUBSAMPLED plane whose block is ONE mi unit tall/wide — that is the
/// 4xN / Nx4 chroma-pairing rule, and getting it wrong reads the wrong
/// reference block. `mi_size_high[bsize] == 1` means a 4-pixel-tall
/// block.
#[allow(clippy::too_many_arguments)]
pub fn setup_pred_plane(
    mi_size_wide: i32,
    mi_size_high: i32,
    width: i32,
    height: i32,
    stride: i32,
    mi_row: i32,
    mi_col: i32,
    subsampling_x: i32,
    subsampling_y: i32,
) -> Buf2D {
    /// C `MI_SIZE`.
    const MI_SIZE: i32 = 4;
    let mut mi_row = mi_row;
    let mut mi_col = mi_col;
    if subsampling_y != 0 && (mi_row & 1) != 0 && mi_size_high == 1 {
        mi_row -= 1;
    }
    if subsampling_x != 0 && (mi_col & 1) != 0 && mi_size_wide == 1 {
        mi_col -= 1;
    }
    let x = (MI_SIZE * mi_col) >> subsampling_x;
    let y = (MI_SIZE * mi_row) >> subsampling_y;
    Buf2D {
        offset: (y * stride + x) as usize,
        width,
        height,
        stride,
    }
}

// ---------------------------------------------------------------------------
// update_refined_mv_fast_rate + opt_non_translation_motion_mode
// ---------------------------------------------------------------------------

/// C `update_refined_mv_fast_rate` (product_coding_loop.c:6741-6755).
///
/// Re-prices a candidate's fast rate after a motion-mode refinement moved
/// its MV: `fast_luma_rate += refined_rate - default_rate`. Skipping it
/// leaves the PRE-refinement rate in the cost and mis-orders MDS1/MDS3.
///
/// Both rates use `svt_av1_mv_bit_cost` at `MV_COST_WEIGHT`, i.e. the
/// same lookup [`super::drl::mv_bit_cost`] ports.
pub fn update_refined_mv_fast_rate(
    fast_luma_rate: u64,
    default_mv: Mv,
    default_ref_mv: Mv,
    refined_mv: Mv,
    refined_ref_mv: Mv,
    table: &MvCostTable,
) -> u64 {
    let default_rate = super::drl::mv_bit_cost(default_mv, default_ref_mv, table, MV_COST_WEIGHT);
    let refined_rate = super::drl::mv_bit_cost(refined_mv, refined_ref_mv, table, MV_COST_WEIGHT);
    // C computes in uint64_t with int32_t operands, so the difference can
    // legitimately be negative and wraps through the unsigned add.
    fast_luma_rate
        .wrapping_add(refined_rate as u64)
        .wrapping_sub(default_rate as u64)
}

/// C `MdStage` (definitions.h:796): the stage a refinement is scheduled
/// at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MdStage {
    Stage0,
    Stage1,
    Stage2,
    Stage3,
    Invalid,
}

/// C's warp-refinement stage selection
/// (product_coding_loop.c:6758-6760): level 1 -> MDS1, level 2 -> MDS3,
/// anything else -> never.
#[inline]
pub fn warp_refine_stage(wm_refine_level: u8) -> MdStage {
    match wm_refine_level {
        1 => MdStage::Stage1,
        2 => MdStage::Stage3,
        _ => MdStage::Invalid,
    }
}

/// C's OBMC-refinement stage selection
/// (product_coding_loop.c:6798-6800): levels 1 AND 2 -> MDS1, levels 3
/// AND 4 -> MDS3, anything else (including 0) -> never.
///
/// Note the pairing: unlike warp, TWO levels map to each stage.
#[inline]
pub fn obmc_refine_stage(obmc_refine_level: u8) -> MdStage {
    match obmc_refine_level {
        1 | 2 => MdStage::Stage1,
        3 | 4 => MdStage::Stage3,
        _ => MdStage::Invalid,
    }
}

/// The candidate state a refinement may change, and which C snapshots so
/// it can be rolled back.
#[derive(Debug, Clone, Copy)]
pub struct RefinementSnapshot {
    pub mv: Mv,
    pub pred_mv: Mv,
    pub drl_index: u8,
    pub wm_params_l0: WarpedMotionParams,
    pub num_proj_ref: u8,
}

/// What [`opt_non_translation_motion_mode`] decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefinementOutcome {
    /// The stage/mode/level combination did not select this candidate.
    NotApplicable,
    /// The refinement ran and CHANGED the MV: the fast rate is re-priced
    /// and `valid_luma_pred` is cleared.
    Refined,
    /// The refinement ran but was invalid, or left the MV unchanged: C
    /// restores the snapshot and proceeds with the original candidate.
    RolledBack,
}

/// C `opt_non_translation_motion_mode`'s WARP arm
/// (product_coding_loop.c:6762-6796).
///
/// **The rollback fires on "unchanged MV" as well as on "invalid"** —
/// and it restores FIVE fields (mv, pred_mv, wm_params, num_proj_ref,
/// drl_index), not just the MV. A port that only checked validity would
/// leave a refined-but-identical candidate with its `wm_params` clobbered
/// by the search.
///
/// `refinement_iterations == 0` skips the search and treats the mode as
/// VALID (C's `? ... : 1`), so the "changed MV" test still runs and, with
/// the MV necessarily unchanged, takes the rollback path.
#[allow(clippy::too_many_arguments)]
pub fn opt_non_translation_motion_mode_warp(
    wm_refine_level: u8,
    wm_refinement_iterations: u8,
    pd_pass_is_1: bool,
    md_stage: MdStage,
    cand: &mut InterCandidate,
    snapshot: RefinementSnapshot,
    refine: impl FnOnce(&mut InterCandidate) -> bool,
    derive_wm_params: impl FnOnce(&mut InterCandidate),
) -> RefinementOutcome {
    let stage = warp_refine_stage(wm_refine_level);
    if stage == MdStage::Invalid
        || !pd_pass_is_1
        || md_stage != stage
        || cand.motion_mode != MotionMode::WarpedCausal
        || cand.mode != PredictionMode::NewMv
    {
        return RefinementOutcome::NotApplicable;
    }
    let motion_mode_valid = if wm_refinement_iterations != 0 {
        refine(cand)
    } else {
        true
    };
    if motion_mode_valid && snapshot.mv.as_int() != cand.mv[0].as_int() {
        derive_wm_params(cand);
        RefinementOutcome::Refined
    } else {
        cand.mv[0] = snapshot.mv;
        cand.pred_mv[0] = snapshot.pred_mv;
        cand.wm_params_l0 = snapshot.wm_params_l0;
        cand.num_proj_ref = snapshot.num_proj_ref;
        cand.drl_index = snapshot.drl_index;
        RefinementOutcome::RolledBack
    }
}

/// C `opt_non_translation_motion_mode`'s OBMC arm
/// (product_coding_loop.c:6798-6824).
///
/// The shape differs from the warp arm in TWO ways C makes easy to miss:
/// the refinement is unconditional (there is no `refinement_iterations`
/// guard), and an "unchanged MV" with a VALID refinement does NOT roll
/// back — it simply does nothing. Only an INVALID refinement restores
/// the snapshot, and it restores three fields, not five.
#[allow(clippy::too_many_arguments)]
pub fn opt_non_translation_motion_mode_obmc(
    obmc_refine_level: u8,
    pd_pass_is_1: bool,
    md_stage: MdStage,
    cand: &mut InterCandidate,
    snapshot: RefinementSnapshot,
    refine: impl FnOnce(&mut InterCandidate) -> bool,
) -> RefinementOutcome {
    let stage = obmc_refine_stage(obmc_refine_level);
    if stage == MdStage::Invalid
        || !pd_pass_is_1
        || md_stage != stage
        || cand.motion_mode != MotionMode::ObmcCausal
        || cand.mode != PredictionMode::NewMv
    {
        return RefinementOutcome::NotApplicable;
    }
    if refine(cand) {
        if snapshot.mv.as_int() != cand.mv[0].as_int() {
            RefinementOutcome::Refined
        } else {
            // Valid but unchanged: C does NOTHING here, it does not roll
            // back.
            RefinementOutcome::NotApplicable
        }
    } else {
        cand.mv[0] = snapshot.mv;
        cand.pred_mv[0] = snapshot.pred_mv;
        cand.drl_index = snapshot.drl_index;
        RefinementOutcome::RolledBack
    }
}

// ---------------------------------------------------------------------------
// obmc_trans_face_off (product_coding_loop.c:1068-1173)
// ---------------------------------------------------------------------------

/// C's OBMC-vs-translation rate delta
/// (product_coding_loop.c:1092-1113).
///
/// The two motion-mode rate terms come from DIFFERENT tables depending on
/// what `svt_aom_motion_mode_allowed` says is available:
/// `motion_mode_fac_bits1` (the two-symbol table) when only OBMC is
/// allowed, `motion_mode_fac_bits` (the three-symbol one) when warp is
/// too, and ZERO when only simple translation is.
///
/// Returns `(translation_bits, obmc_bits)`.
pub fn obmc_face_off_rate_terms(
    last_motion_mode_allowed: MotionMode,
    motion_mode_fac_bits1: &[i32; 2],
    motion_mode_fac_bits: &[i32; 3],
) -> (i32, i32) {
    match last_motion_mode_allowed {
        MotionMode::SimpleTranslation => (0, 0),
        MotionMode::ObmcCausal => (motion_mode_fac_bits1[0], motion_mode_fac_bits1[1]),
        MotionMode::WarpedCausal => (
            motion_mode_fac_bits[MotionMode::SimpleTranslation as usize],
            motion_mode_fac_bits[MotionMode::ObmcCausal as usize],
        ),
    }
}

/// C `obmc_trans_face_off`'s eligibility test
/// (product_coding_loop.c:1078-1087).
///
/// Note it drives `svt_aom_obmc_motion_mode_allowed` with
/// **`situation = 2`**, which is precisely the value that BYPASSES the
/// `trans_face_off` early return in that predicate — the face-off is the
/// thing the early return was deferring to.
///
/// It also forces `rf1 = NONE_FRAME` into the query regardless of the
/// candidate's actual second reference, after having already required
/// `is_inter_singleref_mode`.
#[allow(clippy::too_many_arguments)]
pub fn obmc_trans_face_off_applies(
    cand_mode: PredictionMode,
    cand_motion_mode: MotionMode,
    cand_is_interintra_used: bool,
    is_obmc_allowed_situation2: bool,
) -> bool {
    super::predicates::is_inter_singleref_mode(cand_mode as u8)
        && is_obmc_allowed_situation2
        && cand_motion_mode == MotionMode::SimpleTranslation
        && !cand_is_interintra_used
}

/// C `RDCOST(RM, R, D)` (rd_cost.h:36).
#[inline]
pub fn rdcost(lambda: u64, rate: u64, dist: u64) -> u64 {
    ((rate * lambda + (1 << 8)) >> 9) + (dist << 7)
}

/// C's full-lambda selection inside `obmc_trans_face_off`
/// (product_coding_loop.c:1074).
///
/// **The 10-bit lambda is shifted RIGHT by 4** here, undoing the left
/// shift it normally carries, because the 10-bit variance function
/// already rescales its output into the 8-bit range. Using the unshifted
/// lambda would over-weight the rate by 16x on the hbd path.
#[inline]
pub fn obmc_face_off_lambda(hbd_md: bool, full_lambda_10: u32, full_lambda_8: u32) -> u32 {
    if hbd_md {
        full_lambda_10 >> 4
    } else {
        full_lambda_8
    }
}

/// C's VAR-arm cost in `obmc_trans_face_off`
/// (product_coding_loop.c:1163-1168).
///
/// The variance is shifted LEFT by 4 before the RDCOST because full
/// lambda expects a squared metric at that scale — the same shift the
/// full loop applies to SSE.
#[inline]
pub fn obmc_face_off_var_cost(
    full_lambda: u32,
    fast_luma_rate: u64,
    fast_chroma_rate: u64,
    luma_variance: u64,
) -> u64 {
    rdcost(
        u64::from(full_lambda),
        fast_luma_rate + fast_chroma_rate,
        luma_variance << 4,
    )
}

// ---------------------------------------------------------------------------
// PD0 staging (product_coding_loop.c + mode_decision.c:3494)
// ---------------------------------------------------------------------------

/// C `fast_loop_core_pd0` (product_coding_loop.c:964-1005), MAINLINE arm.
///
/// `SVT_HDR_MODE` is **0** by default (`EbDebugMacros.h:53-54`), so the
/// compiled body is the `#else`: `fast_cost = vf(pred, src)`, the plain
/// variance, with NO lambda, NO rate and NO subsampling. The
/// `#if SVT_HDR_MODE` arm computes a spatial full distortion over
/// HALF the rows and doubles it — a different number — and is
/// deliberately not implemented here.
#[inline]
pub fn fast_loop_core_pd0_cost(luma_variance: u32) -> u64 {
    u64::from(luma_variance)
}

/// C `md_stage_0_pd0` (product_coding_loop.c:1507-1523).
///
/// Two buffers, ping-ponged. **The buffer index flips only when a
/// candidate WINS**, so a run of losing candidates all reuse the same
/// scratch buffer and the winner's buffer is never overwritten. Returns
/// `(best_cost, best_buffer_idx)`.
///
/// The comparison is `<`, so a tie keeps the EARLIER candidate.
pub fn md_stage_0_pd0(costs: &[u64]) -> (u64, usize) {
    let mut best_cost = u64::MAX;
    let mut best_idx = 0usize;
    let mut cand_buff_idx = 0usize;
    for &cost in costs {
        if cost < best_cost {
            best_cost = cost;
            best_idx = cand_buff_idx;
            cand_buff_idx = 1 - cand_buff_idx;
        }
    }
    (best_cost, best_idx)
}

/// C `md_stage_3_pd0`'s residual-subsampling step
/// (product_coding_loop.c:7105-7106).
///
/// A block below 16x16 caps the step at 1, because there is no 8x2
/// transform. The three C asserts that follow are reproduced as
/// `debug_assert!`s in the caller's contract, stated here: step 2
/// requires `sq_size >= 16`, step 1 requires `sq_size >= 8`, and any
/// non-zero step is incompatible with 4x4 blocks.
#[inline]
pub fn md_stage_3_pd0_subres_step(sq_size: u32, subres_step: u8) -> u8 {
    if sq_size >= 16 {
        subres_step
    } else {
        subres_step.min(1)
    }
}

/// Which stages C `generate_md_stage_0_cand_pd0`
/// (mode_decision.c:3494-3521) runs, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pd0CandPlan {
    pub inject_intra: bool,
    pub inject_inter: bool,
    /// Only decidable AFTER the first two ran, because it is gated on the
    /// resulting count being zero.
    pub inject_zz_backup_if_empty: bool,
    pub reject_sframe: bool,
}

/// C `generate_md_stage_0_cand_pd0` (mode_decision.c:3494-3521,
/// EXPORTED).
///
/// **PD0 is not optional**: `multi_pass_pd_level` is `MULTI_PASS_PD_ON`
/// at all three assignment sites in `enc_mode_config.c`, and PD0 is
/// skipped only when depth removal collapses to a single depth
/// (`enc_dec_process.c:2947`). So this dispatch is live at low presets,
/// not just high ones.
///
/// The intra gate is `sq_size < 128 && intra_ctrls.enable_intra` — a
/// 128x128 block gets NO intra candidate in PD0 at all.
///
/// The ZZ backup fires only for a non-I slice whose count came out ZERO,
/// which is the case the inter pruning can produce and the I-slice path
/// structurally cannot (DC is always injected there).
pub fn generate_md_stage_0_cand_pd0_plan(
    is_i_slice: bool,
    sq_size: u32,
    enable_intra: bool,
    sframe_ref_pruned: bool,
) -> Pd0CandPlan {
    Pd0CandPlan {
        inject_intra: sq_size < 128 && enable_intra,
        inject_inter: !is_i_slice,
        inject_zz_backup_if_empty: !is_i_slice,
        reject_sframe: sframe_ref_pruned,
    }
}

/// C `generate_md_stage_0_cand_pd0` (mode_decision.c:3494-3521) driven
/// end to end.
///
/// The injectors are closures because two of the four
/// (`inject_intra_candidates_pd0`, `reject_candidate_sframe`) belong to
/// the intra lane and to the s-frame lane respectively; the inter half is
/// [`super::inject::inject_inter_candidates_pd0`].
pub fn generate_md_stage_0_cand_pd0(
    plan: Pd0CandPlan,
    cands: &mut CandArray,
    mut inject_intra: impl FnMut(&mut CandArray),
    mut inject_inter: impl FnMut(&mut CandArray),
    mut inject_zz: impl FnMut(&mut CandArray),
    mut reject_sframe: impl FnMut(&mut CandArray),
) -> usize {
    if plan.inject_intra {
        inject_intra(cands);
    }
    if plan.inject_inter {
        inject_inter(cands);
    }
    if plan.inject_zz_backup_if_empty && cands.count() == 0 {
        inject_zz(cands);
    }
    if plan.reject_sframe {
        reject_sframe(cands);
    }
    cands.count()
}

// ---------------------------------------------------------------------------
// inter_intra_search (mode_decision.c:326-492)
// ---------------------------------------------------------------------------

/// C `INTERINTRA_MODES` (definitions.h:1257).
pub const INTERINTRA_MODES: usize = 4;

/// C `InterIntraMode` (definitions.h:1257).
pub const II_DC_PRED: u8 = 0;
pub const II_V_PRED: u8 = 1;
pub const II_H_PRED: u8 = 2;
pub const II_SMOOTH_PRED: u8 = 3;

/// The result of C `inter_intra_search` (mode_decision.c:326-492): the
/// three candidate fields it writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterIntraResult {
    pub interintra_mode: u8,
    pub use_wedge_interintra: bool,
    pub interintra_wedge_index: i8,
}

/// C `inter_intra_search`'s mode loop and wedge decision
/// (mode_decision.c:414-492).
///
/// `mode_rd(mode)` supplies C's per-mode RD: `RDCOST(lambda, rate + rmode,
/// dist)` when `ii_wedge_mode` is on, and a plain SSE otherwise — that
/// branch belongs to the caller because only it has the prediction
/// buffers.
///
/// Two decisions this makes:
///
/// * **The best mode is chosen with `<`,** so a tie keeps the LOWER
///   `InterIntraMode` (DC before V before H before SMOOTH).
/// * **`use_wedge_interintra` is set when `ii_wedge_mode == 1` OR the
///   wedge RD beat the non-wedge RD.** At mode 1 the wedge flag is
///   unconditional because [`super::inject::inj_non_simple_modes`]
///   injects the non-wedge variant as a SEPARATE candidate; at mode 2
///   only the winner is kept.
pub fn inter_intra_search(
    ii_wedge_mode: u8,
    mut mode_rd: impl FnMut(u8) -> i64,
    pick_wedge: impl FnOnce(u8) -> (i64, i8),
) -> InterIntraResult {
    let mut best_rd = i64::MAX;
    let mut best_mode = INTERINTRA_MODES as u8; // C's INTERINTRA_MODES sentinel
    for j in 0..INTERINTRA_MODES as u8 {
        let rd = mode_rd(j);
        if rd < best_rd {
            best_rd = rd;
            best_mode = j;
        }
    }

    let mut wedge_index = 0i8;
    let mut best_rd_wedge = i64::MAX;
    if ii_wedge_mode != 0 {
        let (rd, idx) = pick_wedge(best_mode);
        best_rd_wedge = rd;
        wedge_index = idx;
    }

    InterIntraResult {
        interintra_mode: best_mode,
        use_wedge_interintra: ii_wedge_mode == 1 || best_rd_wedge < best_rd,
        interintra_wedge_index: wedge_index,
    }
}

/// C `pick_interintra_wedge` (mode_decision.c:297-323), the part that is
/// not `pick_wedge_fixed_sign`.
///
/// It builds two residual planes — `residual1 = src - pred1` and
/// `diff10 = pred1 - pred0` — at the BLOCK stride (`bw`), not the source
/// stride, and hands them to the wedge search. The order of the two
/// subtractions is load-bearing: `diff10` is pred1 MINUS pred0, so
/// swapping the two predictions flips the sign of every wedge score.
///
/// `hbd` selects `svt_aom_highbd_subtract_block` at `EB_TEN_BIT`; both
/// arms write `int16_t`.
pub fn interintra_wedge_residuals(
    src: &[u8],
    src_stride: usize,
    pred0: &[u8],
    pred1: &[u8],
    bw: usize,
    bh: usize,
) -> (Vec<i16>, Vec<i16>) {
    let mut residual1 = vec![0i16; bw * bh];
    let mut diff10 = vec![0i16; bw * bh];
    for y in 0..bh {
        for x in 0..bw {
            let s = i16::from(src[y * src_stride + x]);
            let p1 = i16::from(pred1[y * bw + x]);
            let p0 = i16::from(pred0[y * bw + x]);
            residual1[y * bw + x] = s - p1;
            diff10[y * bw + x] = p1 - p0;
        }
    }
    (residual1, diff10)
}

#[cfg(test)]
mod tests {
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
        let t = MvCostTable {
            joint: [0, 100, 200, 300],
            comp: [
                (0..super::super::pme::MV_VALS)
                    .map(|i| (i as i32 - super::super::pme::MV_MAX).abs())
                    .collect(),
                (0..super::super::pme::MV_VALS)
                    .map(|i| (i as i32 - super::super::pme::MV_MAX).abs())
                    .collect(),
            ],
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
        let out =
            opt_non_translation_motion_mode_obmc(1, true, MdStage::Stage1, &mut c, s, |cand| {
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
        let out =
            opt_non_translation_motion_mode_obmc(3, true, MdStage::Stage3, &mut c, s, |cand| {
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
        assert!(
            !generate_md_stage_0_cand_pd0_plan(true, 64, true, false).inject_zz_backup_if_empty
        );

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
}
