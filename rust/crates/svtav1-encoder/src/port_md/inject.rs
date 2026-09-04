//! Inter-candidate injection — the regular-PD1 and PD0 candidate
//! generators of `Source/Lib/Codec/mode_decision.c`.
//!
//! This is the seam the whole inter port has been waiting on: open-loop
//! ME ([`crate::inter_me`]), the reference-MV stack
//! ([`crate::inter_mvp`]) and the MV rate model
//! ([`crate::inter_mv_code`]) are all ported, and nothing in the encoder
//! reads any of them, because the layer that turns them into MD
//! candidates had no counterpart. Without it every inter block resolves
//! to intra.
//!
//! | this module | C |
//! |---|---|
//! | [`inject_inter_candidates`] | `:2867-2960` (`svt_aom_inject_inter_candidates`) |
//! | [`inject_mvp_candidates_ii`] | `:1482-1655` |
//! | [`inject_new_nearest_new_comb_candidates`] | `:1657-1870` |
//! | [`inject_new_candidates`] | `:2479-2601` |
//! | [`inject_global_candidates`] | `:2603-2721` |
//! | [`inject_pme_candidates`] | `:2723-2821` |
//! | [`unipred_3x3_candidates_injection`] | `:1084-1163` |
//! | [`bipred_3x3_candidates_injection`] | `:1165-2011` |
//! | [`inject_zz_backup_candidate`] | `:3314-3346` |
//! | [`inj_non_simple_modes`] | `:865-975` |
//! | [`inj_comp_modes`] | `:1027-1082` |
//! | [`determine_compound_mode`] | `:496-521` |
//! | [`skip_compound_on_ref_types`] | `:980-1025` |
//! | [`inject_inter_candidates_pd0`] | `:2823-2834` |
//! | [`inject_new_candidates_pd0`] | `:2293-2370` |
//!
//! # Evidence
//!
//! Every function here is `static` in C (`svt_aom_inject_inter_candidates`
//! carries the `svt_aom_` prefix and is nevertheless `static` — verified
//! with `nm -g`, per this group's documented name trap), so this module is
//! **tier 4**: hand-derived vectors traced against the C source
//! (`docs/WORKING-ON-THIS.md` §4). The predicates the injectors consult —
//! `svt_aom_is_valid_unipred_ref`, `svt_aom_get_max_drl_index`,
//! `svt_get_ref_frame_type`, `svt_is_interintra_allowed`,
//! `svt_aom_is_me_data_present`, `svt_aom_obmc_motion_mode_allowed`,
//! `svt_aom_choose_best_av1_mv_pred` — ARE tier 1 in
//! [`super::predicates`] and [`super::drl`], and this module calls those
//! ports rather than re-transcribing them.
//!
//! # A `MIN` that is a no-op, transcribed as a plain group
//!
//! Four call sites write `MIN(TOT_INTER_GROUP - 1, <GROUP>)`
//! (`PA_ME_GROUP`, `UNI_3x3_GROUP`, `NRST_NEAR_GROUP`,
//! `NRST_NEW_NEAR_GROUP`). Every one of those enum values is 0, 1, 4 or 3
//! and `TOT_INTER_GROUP - 1` is 10, so the clamp never bites; this port
//! passes the group directly and says so here rather than carrying dead
//! arithmetic.
//!
//! # What is delegated, and why that is stated rather than stubbed
//!
//! Five C calls inside these injectors operate on reference PIXELS
//! (`inter_intra_search`, `svt_aom_wm_motion_refinement`,
//! `svt_aom_warped_motion_parameters`, `svt_aom_obmc_motion_refinement`,
//! `svt_aom_calc_pred_masked_compound` / `svt_aom_search_compound_diff_wedge`).
//! They are taken as an [`InjectHooks`] implementation rather than
//! stubbed to a constant, so a caller that has the prediction buffers
//! gets C's behaviour and a caller that does not is forced to say what it
//! substituted. [`NoRefinement`] is the explicit "no pixels available"
//! implementation and is what the tier-4 tests drive.

use super::drl::{ChooseDrlCtx, choose_best_av1_mv_pred};
use super::pme::MvCostTable;
use super::predicates::{
    InjectedMvLog, InterCandGroup, MeCandidateRef, MotionMode, MotionModeCtx, RedundantCandCtrls,
    RefPruningState, get_max_drl_index, get_ref_frame_type, get_tot_comp_types_bsize,
    is_interintra_allowed, is_me_data_present, is_valid_bipred_ref, is_valid_mv_diff,
    is_valid_unipred_ref, mv_is_already_injected, obmc_motion_mode_allowed,
    warped_motion_mode_allowed,
};
use crate::inter_mvp::{
    DrlMvPred, InterMvpStack, av1_ref_frame_type, av1_set_ref_frame, get_av1_mv_pred_drl,
    get_list_idx, get_ref_frame_idx,
};
use svtav1_types::motion::{Mv, TransformationType, WarpedMotionParams};
use svtav1_types::prediction::PredictionMode;

/// C `NONE_FRAME` (definitions.h): the "no second reference" sentinel.
pub const NONE_FRAME: i8 = -1;
/// C `INTRA_FRAME`.
pub const INTRA_FRAME: i8 = 0;
/// C `LAST_FRAME`.
pub const LAST_FRAME: i8 = 1;
/// C `BI_PRED` — the `MeCandidate::direction` value for bi-prediction.
pub const BI_PRED: u8 = 2;
/// C `BIPRED_3x3_REFINMENT_POSITIONS` (mode_decision.c:807).
pub const BIPRED_3X3_REFINEMENT_POSITIONS: usize = 8;

/// C `allow_refinement_flag` (mode_decision.c:809).
pub const ALLOW_REFINEMENT_FLAG: [i8; BIPRED_3X3_REFINEMENT_POSITIONS] = [1, 0, 1, 0, 1, 0, 1, 0];
/// C `bipred_3x3_x_pos` (mode_decision.c:810).
pub const BIPRED_3X3_X_POS: [i8; BIPRED_3X3_REFINEMENT_POSITIONS] = [-1, -1, 0, 1, 1, 1, 0, -1];
/// C `bipred_3x3_y_pos` (mode_decision.c:811).
pub const BIPRED_3X3_Y_POS: [i8; BIPRED_3X3_REFINEMENT_POSITIONS] = [0, 1, 1, 1, 0, -1, -1, -1];

/// C `to_av1_compound_lut` (mode_decision.c:494): `MD_COMP_TYPE` ->
/// `COMPOUND_TYPE`.
pub const TO_AV1_COMPOUND_LUT: [u8; 4] = [
    0, // COMPOUND_AVERAGE
    1, // COMPOUND_DISTWTD
    2, // COMPOUND_DIFFWTD
    3, // COMPOUND_WEDGE
];

// ---------------------------------------------------------------------------
// The candidate record
// ---------------------------------------------------------------------------

/// C `ModeDecisionCandidate` restricted to the fields the inter injectors
/// write.
///
/// C memcpy's a WHOLE `ModeDecisionCandidate` when it clones a candidate
/// ([`inj_non_simple_modes`], [`inj_comp_modes`]), so anything the
/// original carried survives into the clone; `Clone` here has the same
/// effect for the fields present.
#[derive(Debug, Clone, Copy)]
pub struct InterCandidate {
    pub mode: PredictionMode,
    pub motion_mode: MotionMode,
    pub is_interintra_used: bool,
    /// C `block_mi.interintra_mode`.
    pub interintra_mode: u8,
    /// C `block_mi.use_wedge_interintra`.
    pub use_wedge_interintra: bool,
    /// C `block_mi.interintra_wedge_index`.
    pub interintra_wedge_index: i8,
    pub use_intrabc: bool,
    pub skip_mode_allowed: bool,
    /// C `block_mi.mv[2]`.
    pub mv: [Mv; 2],
    /// C `pred_mv[2]` — what the writer differences the coded MV from.
    pub pred_mv: [Mv; 2],
    pub drl_index: u8,
    /// C `block_mi.ref_frame[2]`.
    pub ref_frame: [i8; 2],
    /// C `block_mi.num_proj_ref`.
    pub num_proj_ref: u8,
    pub wm_params_l0: WarpedMotionParams,
    pub wm_params_l1: WarpedMotionParams,
    /// C `block_mi.comp_group_idx` — a CODED symbol.
    pub comp_group_idx: u8,
    /// C `block_mi.compound_idx` — a CODED symbol.
    pub compound_idx: u8,
    /// C `block_mi.interinter_comp.type`.
    pub interinter_comp_type: u8,
    /// C `block_mi.interinter_comp.mask_type`.
    pub interinter_mask_type: u8,
    /// C `transform_type[0]` / `transform_type_uv`, written only by
    /// [`inject_zz_backup_candidate`].
    pub transform_type_y: u8,
    pub transform_type_uv: u8,
}

impl Default for InterCandidate {
    fn default() -> Self {
        Self {
            mode: PredictionMode::NewMv,
            motion_mode: MotionMode::SimpleTranslation,
            is_interintra_used: false,
            interintra_mode: 0,
            use_wedge_interintra: false,
            interintra_wedge_index: 0,
            use_intrabc: false,
            skip_mode_allowed: false,
            mv: [Mv::ZERO; 2],
            pred_mv: [Mv::ZERO; 2],
            drl_index: 0,
            ref_frame: [NONE_FRAME; 2],
            num_proj_ref: 0,
            wm_params_l0: WarpedMotionParams::default(),
            wm_params_l1: WarpedMotionParams::default(),
            comp_group_idx: 0,
            compound_idx: 0,
            interinter_comp_type: 0,
            interinter_mask_type: 0,
            transform_type_y: 0,
            transform_type_uv: 0,
        }
    }
}

/// C's `ctx->fast_cand_array` + the `cand_total_cnt` running index, with
/// `INC_MD_CAND_CNT`'s exact saturation behaviour.
#[derive(Debug, Clone)]
pub struct CandArray {
    slots: Vec<InterCandidate>,
    count: usize,
    /// C `pcs->ppcs->max_can_count`.
    max_can_count: usize,
    /// How many times `INC_MD_CAND_CNT` hit its ceiling. C emits
    /// `SVT_ERROR("Mode decision candidate count exceeded")` there and
    /// keeps going, which silently makes the NEXT write overwrite the
    /// last slot; counting it makes that observable instead of silent.
    pub overflow_events: usize,
}

impl CandArray {
    pub fn new(max_can_count: usize) -> Self {
        Self {
            slots: vec![InterCandidate::default(); max_can_count.max(1)],
            count: 0,
            max_can_count,
            overflow_events: 0,
        }
    }

    /// The index C would write the next candidate at.
    #[inline]
    pub fn count(&self) -> usize {
        self.count
    }

    /// C `&cand_array[cand_total_cnt]` followed by `INC_MD_CAND_CNT`.
    ///
    /// **C's macro is `if (cnt + 1 < max) cnt++; else SVT_ERROR(...)`** —
    /// note `cnt + 1 < max`, not `<=`, so the LAST slot is never reached
    /// by a successful increment, and on the failing branch the count
    /// does not move so the next candidate overwrites this one.
    pub fn push(&mut self, cand: InterCandidate) {
        if self.count < self.slots.len() {
            self.slots[self.count] = cand;
        }
        if self.count + 1 < self.max_can_count {
            self.count += 1;
        } else {
            self.overflow_events += 1;
        }
    }

    /// C `&ctx->fast_cand_array[*total_cand_count - 1]` — the candidate
    /// [`inj_non_simple_modes`] and [`inj_comp_modes`] clone from.
    pub fn last(&self) -> Option<&InterCandidate> {
        self.count.checked_sub(1).map(|i| &self.slots[i])
    }

    /// The candidates written so far, in injection order.
    pub fn as_slice(&self) -> &[InterCandidate] {
        &self.slots[..self.count]
    }
}

// ---------------------------------------------------------------------------
// The pixel-domain hooks
// ---------------------------------------------------------------------------

/// The five C calls inside these injectors that need reference pixels.
///
/// Taken as a trait rather than stubbed so a caller cannot silently get a
/// different candidate set than C without saying so.
pub trait InjectHooks {
    /// C `inter_intra_search` (mode_decision.c:326): fills
    /// `interintra_mode`, `use_wedge_interintra` and
    /// `interintra_wedge_index` on the candidate.
    fn inter_intra_search(&mut self, cand: &mut InterCandidate);

    /// C `svt_aom_wm_motion_refinement` (mode_decision.c:1873): refines
    /// the MV and returns whether a valid MV was found.
    fn wm_motion_refinement(&mut self, cand: &mut InterCandidate) -> bool;

    /// C `svt_aom_warped_motion_parameters`: derives `wm_params_l0` and
    /// `num_proj_ref`, returning validity.
    fn warped_motion_parameters(&mut self, cand: &mut InterCandidate) -> bool;

    /// C `svt_aom_obmc_motion_refinement` (mode_decision.c:2183).
    fn obmc_motion_refinement(&mut self, cand: &mut InterCandidate) -> bool;

    /// C `svt_aom_calc_pred_masked_compound`: a NON-zero return makes
    /// [`inj_comp_modes`] return WITHOUT injecting anything.
    fn calc_pred_masked_compound(&mut self, cand: &InterCandidate) -> bool;

    /// C `svt_aom_search_compound_diff_wedge`, called from
    /// [`determine_compound_mode`] for DIFF0 and WEDGE.
    fn search_compound_diff_wedge(&mut self, cand: &mut InterCandidate);
}

/// The explicit "no reference pixels available" hooks: no inter-intra
/// search, no refinement (every motion-mode candidate is accepted as
/// valid, which is C's behaviour when the refinement is switched OFF),
/// and no compound mask search.
///
/// This is NOT a claim that it matches C when the pixel searches run —
/// it is what a caller without prediction buffers must opt into
/// explicitly.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoRefinement;

impl InjectHooks for NoRefinement {
    fn inter_intra_search(&mut self, _cand: &mut InterCandidate) {}
    fn wm_motion_refinement(&mut self, _cand: &mut InterCandidate) -> bool {
        true
    }
    fn warped_motion_parameters(&mut self, _cand: &mut InterCandidate) -> bool {
        true
    }
    fn obmc_motion_refinement(&mut self, _cand: &mut InterCandidate) -> bool {
        true
    }
    fn calc_pred_masked_compound(&mut self, _cand: &InterCandidate) -> bool {
        false
    }
    fn search_compound_diff_wedge(&mut self, _cand: &mut InterCandidate) {}
}

// ---------------------------------------------------------------------------
// The control structs the injectors read
// ---------------------------------------------------------------------------

/// C `InterCompCtrls` (md_process.h:83), the fields these injectors read.
#[derive(Debug, Clone, Copy, Default)]
pub struct InterCompCtrls {
    pub tot_comp_types: u8,
    pub do_me: bool,
    pub do_pme: bool,
    pub do_nearest_nearest: bool,
    pub do_near_near: bool,
    pub do_nearest_near_new: bool,
    pub do_3x3_bi: bool,
    pub do_global: bool,
    pub skip_on_ref_info: bool,
    pub no_sym_dist: bool,
    pub max_mv_length: u16,
}

/// C `InterIntraCompCtrls`.
#[derive(Debug, Clone, Copy, Default)]
pub struct InterIntraCompCtrls {
    pub enabled: bool,
    pub wedge_mode_sq: u8,
    pub wedge_mode_nsq: u8,
}

/// C `WmCtrls`, the fields these injectors read.
#[derive(Debug, Clone, Copy, Default)]
pub struct WmCtrls {
    pub enabled: bool,
    pub use_wm_for_mvp: bool,
    pub refinement_iterations: u8,
    pub refine_level: u8,
}

/// C `ObmcControls`, the fields these injectors read.
#[derive(Debug, Clone, Copy, Default)]
pub struct ObmcCtrls {
    pub enabled: bool,
    pub max_blk_size: u8,
    pub trans_face_off: bool,
    pub refine_level: u8,
}

/// C `NearCountCtrls` (`ctx->cand_reduction_ctrls.near_count_ctrls`).
#[derive(Debug, Clone, Copy, Default)]
pub struct NearCountCtrls {
    pub enabled: bool,
    pub near_count: u8,
    pub near_near_count: u8,
}

/// C `Bipred3x3Controls`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Bipred3x3Ctrls {
    pub enabled: bool,
    pub search_diag: bool,
    /// C `use_l0_l1_dev`; `0xFF` is C's `(uint8_t)~0` "off" sentinel.
    pub use_l0_l1_dev: u8,
    pub use_best_list: bool,
}

/// The whole MD-context slice these injectors read.
///
/// Field names mirror C's so a reader can grep the C for any of them.
pub struct InjectCtx<'a> {
    // --- geometry / frame header ---
    /// C `ctx->blk_geom->bsize`.
    pub bsize: u8,
    pub bwidth: u16,
    pub bheight: u16,
    pub blk_org_x: u32,
    pub blk_org_y: u32,
    /// C `ctx->shape == PART_N`.
    pub shape_is_part_n: bool,
    /// C `frm_hdr->reference_mode == SINGLE_REFERENCE`.
    pub reference_mode_is_single: bool,
    pub allow_high_precision_mv: bool,
    pub is_motion_mode_switchable: bool,
    pub force_integer_mv: u8,
    /// C `frm_hdr->skip_mode_params.skip_mode_flag` and its two refs.
    pub skip_mode_flag: bool,
    pub skip_mode_ref_frame_idx_0: i8,
    pub skip_mode_ref_frame_idx_1: i8,
    /// C `svt_av1_is_lossless_segment(pcs, blk_ptr->segment_id)`.
    pub is_lossless_segment: bool,

    // --- reference set ---
    /// C `ctx->ref_frame_type_arr` / `tot_ref_frame_types`, in order.
    pub ref_frame_type_arr: &'a [i8],
    /// C `pcs->ppcs->global_motion[TOTAL_REFS_PER_FRAME]`.
    pub global_motion: &'a [WarpedMotionParams; 8],
    /// C `pcs->ppcs->gm_ctrls.skip_identity`.
    pub gm_skip_identity: bool,
    /// C `ctx->wm_sample_info[frame_type].num`.
    pub wm_sample_num: &'a [u8; 8],

    // --- MVP / DRL ---
    /// C `ctx->ref_mv_stack[MODE_CTX_REF_FRAMES]`, indexed by reference
    /// type. Each entry's `count` MUST equal the matching
    /// [`InjectCtx::ref_mv_count`] entry: C's
    /// `svt_aom_get_av1_mv_pred_drl` reads `xd->ref_mv_count[ref_frame]`
    /// while this port's [`get_av1_mv_pred_drl`] reads `stack.count`, and
    /// they are the same number in C.
    pub ref_mv_stack: &'a [InterMvpStack],
    /// C `blk_ptr->av1xd->ref_mv_count[MODE_CTX_REF_FRAMES]`.
    pub ref_mv_count: &'a [u8],
    pub nmv_cost: &'a MvCostTable,
    pub drl_mode_fac_bits: &'a [[i32; 2]; 3],
    pub shut_fast_rate: bool,
    pub approx_inter_rate: u8,

    // --- ME data ---
    /// C `me_results->total_me_candidate_index[me_block_offset]`.
    pub total_me_cnt: usize,
    /// C `&me_results->me_candidate_array[me_cand_offset]`.
    pub me_cands: &'a [MeCandidateRef],
    /// C `me_results->total_me_candidate_index`, needed by
    /// `svt_aom_is_me_data_present`.
    pub me_totals: &'a [u8],
    pub me_block_offset: usize,
    /// C `ctx->sb_me_mv[list][ref]`.
    pub sb_me_mv: &'a [[Mv; 4]; 2],
    /// C `ctx->post_subpel_me_mv_cost[list][ref]`.
    pub post_subpel_me_mv_cost: &'a [[u32; 4]; 2],
    /// C `ctx->valid_pme_mv[list][ref]` / `best_pme_mv[list][ref]`.
    pub valid_pme_mv: &'a [[bool; 4]; 2],
    pub best_pme_mv: &'a [[Mv; 4]; 2],

    // --- gates ---
    pub ref_pruning: &'a RefPruningState,
    pub corrupted_mv_check: bool,
    pub redundant_cand_ctrls: RedundantCandCtrls,
    pub inter_comp_ctrls: InterCompCtrls,
    pub inter_intra_comp_ctrls: InterIntraCompCtrls,
    pub wm_ctrls: WmCtrls,
    pub obmc_ctrls: ObmcCtrls,
    pub near_count_ctrls: NearCountCtrls,
    pub bipred3x3_ctrls: Bipred3x3Ctrls,
    pub unipred3x3_injection: u8,
    pub new_nearest_injection: bool,
    pub new_nearest_near_comb_injection: u8,
    pub inject_new_me: bool,
    pub global_mv_injection: bool,
    pub inject_new_pme: bool,
    pub updated_enable_pme: bool,
    pub reduce_unipred_candidates: u8,
    pub use_neighbouring_mode_ctrls_enabled: bool,
    pub is_intra_bordered: bool,
    /// C `blk_ptr->overlappable_neighbors != 0`.
    pub has_overlappable_candidates: bool,
    pub allow_warped_motion: bool,
    /// C `xd->left_available` / `up_available` + the two neighbour
    /// `block_mi`s `skip_compound_on_ref_types` reads.
    pub left_available: bool,
    pub up_available: bool,
    pub left_mi: Option<(PredictionMode, [i8; 2])>,
    pub above_mi: Option<(PredictionMode, [i8; 2])>,
}

impl InjectCtx<'_> {
    /// C's `allow_bipred` (mode_decision.c:2871-2875 and its twins):
    /// bi-prediction needs BOTH dimensions above 4 and a frame-header
    /// reference mode that is not `SINGLE_REFERENCE` (AV1 spec 5.11.25).
    #[inline]
    pub fn allow_bipred(&self) -> bool {
        !(self.reference_mode_is_single || self.bwidth == 4 || self.bheight == 4)
    }

    fn motion_mode_ctx(&self) -> MotionModeCtx {
        MotionModeCtx {
            trans_face_off: self.obmc_ctrls.trans_face_off,
            obmc_enabled: self.obmc_ctrls.enabled,
            obmc_max_blk_size: self.obmc_ctrls.max_blk_size,
            is_motion_mode_switchable: self.is_motion_mode_switchable,
            force_integer_mv: self.force_integer_mv,
            has_overlappable_candidates: self.has_overlappable_candidates,
            allow_warped_motion: self.allow_warped_motion,
            wm_enabled: self.wm_ctrls.enabled,
            blk_width: self.bwidth,
            blk_height: self.bheight,
        }
    }

    fn choose_drl(&self, ref_frame: i8, mode: PredictionMode, mv0: Mv, mv1: Mv) -> (u8, [Mv; 2]) {
        let stack = &self.ref_mv_stack[ref_frame.max(0) as usize];
        let count = self.ref_mv_count[ref_frame.max(0) as usize];
        let ctx = ChooseDrlCtx {
            shut_fast_rate: self.shut_fast_rate,
            approx_inter_rate: self.approx_inter_rate,
            ref_mv_stack: &stack.stack,
            ref_mv_count: count,
            nmv_cost: self.nmv_cost,
            drl_mode_fac_bits: self.drl_mode_fac_bits,
        };
        let mut drl = 0u8;
        let mut pred = [Mv::ZERO; 2];
        choose_best_av1_mv_pred(&ctx, mode, mv0, mv1, &mut drl, &mut pred);
        (drl, pred)
    }
}

// ---------------------------------------------------------------------------
// determine_compound_mode / skip_compound_on_ref_types / inj_comp_modes
// ---------------------------------------------------------------------------

/// C `determine_compound_mode` (mode_decision.c:496-521).
///
/// These are CODED syntax elements, not RD-only state: `comp_group_idx`
/// and `compound_idx` are written into the bitstream, so this mapping
/// must match bit for bit.
pub fn determine_compound_mode(
    cand: &mut InterCandidate,
    cur_type: u8,
    hooks: &mut impl InjectHooks,
) {
    cand.interinter_comp_type = TO_AV1_COMPOUND_LUT[cur_type as usize];
    match cur_type {
        0 => {
            // MD_COMP_AVG
            cand.comp_group_idx = 0;
            cand.compound_idx = 1;
        }
        1 => {
            // MD_COMP_DIST
            cand.comp_group_idx = 0;
            cand.compound_idx = 0;
        }
        2 => {
            // MD_COMP_DIFF0
            cand.comp_group_idx = 1;
            cand.compound_idx = 1;
            cand.interinter_mask_type = 55;
            hooks.search_compound_diff_wedge(cand);
        }
        _ => {
            // MD_COMP_WEDGE
            cand.comp_group_idx = 1;
            cand.compound_idx = 1;
            hooks.search_compound_diff_wedge(cand);
        }
    }
}

/// C `skip_compound_on_ref_types` (mode_decision.c:980-1025).
///
/// Note the shape: with NEITHER neighbour available C returns **false**
/// (do not skip), and with a neighbour available it returns false as soon
/// as one of them selected a matching reference. Only a block with at
/// least one available neighbour, none of which matched, skips.
///
/// The single-ref match is `ref_frame[0] == rf[0] || == rf[1]`; the
/// compound match is `ref_frame[0] == rf[0] && ref_frame[1] == rf[1]` —
/// an OR and an AND, not the same test twice.
pub fn skip_compound_on_ref_types(ctx: &InjectCtx<'_>, rf: [i8; 2]) -> bool {
    if !ctx.inter_comp_ctrls.skip_on_ref_info {
        return false;
    }
    if get_list_idx(rf[0]) == get_list_idx(rf[1]) {
        return true;
    }
    if !ctx.left_available && !ctx.up_available {
        return false;
    }
    let matches = |mi: Option<(PredictionMode, [i8; 2])>| -> bool {
        match mi {
            None => false,
            Some((mode, nrf)) => {
                (crate::inter_mv_code::is_inter_singleref_mode(mode)
                    && (nrf[0] == rf[0] || nrf[0] == rf[1]))
                    || (crate::inter_mv_code::is_inter_compound_mode(mode)
                        && nrf[0] == rf[0]
                        && nrf[1] == rf[1])
            }
        }
    };
    if ctx.left_available && matches(ctx.left_mi) {
        return false;
    }
    if ctx.up_available && matches(ctx.above_mi) {
        return false;
    }
    true
}

/// C `inj_comp_modes` (mode_decision.c:1027-1082).
///
/// Clones the previously-injected `MD_COMP_AVG` candidate into its
/// DIST / DIFF / WEDGE variants. Five separate early returns, in C's
/// order: the block-size cap, the compound reference-pruning gate, the
/// neighbour-info skip, the MV-length cap, and the masked-compound
/// precompute.
pub fn inj_comp_modes(ctx: &InjectCtx<'_>, cands: &mut CandArray, hooks: &mut impl InjectHooks) {
    let Some(avg_cand) = cands.last().copied() else {
        return;
    };

    let tot_comp_types = get_tot_comp_types_bsize(ctx.inter_comp_ctrls.tot_comp_types, ctx.bsize);
    // C: `if (tot_comp_types == MD_COMP_DIST) return;` — an EQUALITY
    // test on 1, not `<=`, so a tot_comp_types of 0 (AVG only) falls
    // through into a loop that runs zero times anyway.
    if tot_comp_types == 1 {
        return;
    }

    let ref_idx_0 = get_ref_frame_idx(avg_cand.ref_frame[0]);
    let ref_idx_1 = get_ref_frame_idx(avg_cand.ref_frame[1]);
    let list_idx_0 = get_list_idx(avg_cand.ref_frame[0]);
    let list_idx_1 = get_list_idx(avg_cand.ref_frame[1]);
    if !is_valid_bipred_ref(
        ctx.ref_pruning,
        InterCandGroup::InterComp,
        list_idx_0,
        ref_idx_0,
        list_idx_1,
        ref_idx_1,
    ) {
        return;
    }
    if skip_compound_on_ref_types(ctx, avg_cand.ref_frame) {
        return;
    }
    if ctx.inter_comp_ctrls.max_mv_length != 0 {
        let m = i32::from(ctx.inter_comp_ctrls.max_mv_length);
        if i32::from(avg_cand.mv[0].x).abs() > m
            || i32::from(avg_cand.mv[0].y).abs() > m
            || i32::from(avg_cand.mv[1].x).abs() > m
            || i32::from(avg_cand.mv[1].y).abs() > m
        {
            return;
        }
    }
    if tot_comp_types > 1 && hooks.calc_pred_masked_compound(&avg_cand) {
        return;
    }

    // C: `for (cur_type = MD_COMP_DIST; cur_type < tot_comp_types; ...)`.
    for cur_type in 1..tot_comp_types {
        if ctx.inter_comp_ctrls.no_sym_dist && cur_type == 1 && ref_idx_0 == 0 && ref_idx_1 == 0 {
            continue;
        }
        let mut cand = avg_cand;
        cand.skip_mode_allowed = false;
        determine_compound_mode(&mut cand, cur_type, hooks);
        cands.push(cand);
    }
}

// ---------------------------------------------------------------------------
// inj_non_simple_modes
// ---------------------------------------------------------------------------

/// C `inj_non_simple_modes` (mode_decision.c:865-975).
///
/// Clones the previously-injected simple-translation candidate into its
/// inter-intra, warped and OBMC variants. This is where EVERY
/// non-`SIMPLE_TRANSLATION` motion mode and every inter-intra candidate
/// enters the list.
///
/// Two details worth stating:
///
/// * **The `ii_wedge_mode == 1` arm injects a SECOND inter-intra
///   candidate** — the non-wedge one — cloned from the ORIGINAL simple
///   translation candidate, not from the one `inter_intra_search` just
///   modified, and carrying only the searched `interintra_mode` forward.
/// * **Warp and OBMC are inside `#if CONFIG_ENABLE_OBMC`, which is 1**
///   in this build (EbConfigMacros.h:82; RTC_BUILD is 0). The `#else`
///   arm merely `UNUSED`s the two flags.
pub fn inj_non_simple_modes(
    ctx: &InjectCtx<'_>,
    cands: &mut CandArray,
    hooks: &mut impl InjectHooks,
    enable_ii: bool,
    enable_wm: bool,
    enable_obmc: bool,
) {
    let Some(simple) = cands.last().copied() else {
        return;
    };
    debug_assert_eq!(simple.ref_frame[1], NONE_FRAME);
    let list_idx = get_list_idx(simple.ref_frame[0]);
    let ref_idx = get_ref_frame_idx(simple.ref_frame[0]);

    // ---- INTER-INTRA ----
    let is_ii_allowed = is_valid_unipred_ref(
        ctx.ref_pruning,
        InterCandGroup::InterIntra,
        list_idx,
        ref_idx,
    ) && is_interintra_allowed(
        ctx.inter_intra_comp_ctrls.enabled,
        ctx.bsize,
        simple.mode as u8,
        simple.ref_frame,
    );
    if enable_ii && is_ii_allowed {
        let mut cand = simple;
        hooks.inter_intra_search(&mut cand);
        cand.is_interintra_used = true;
        cand.ref_frame[1] = INTRA_FRAME;
        let ii_mode = cand.interintra_mode;
        cands.push(cand);

        let ii_wedge_mode = if ctx.shape_is_part_n {
            ctx.inter_intra_comp_ctrls.wedge_mode_sq
        } else {
            ctx.inter_intra_comp_ctrls.wedge_mode_nsq
        };
        if ii_wedge_mode == 1 {
            let mut cand = simple;
            cand.is_interintra_used = true;
            cand.ref_frame[1] = INTRA_FRAME;
            cand.interintra_mode = ii_mode;
            cand.use_wedge_interintra = false;
            cands.push(cand);
        }
    }

    // ---- WARP ----
    let is_warp_allowed = warped_motion_mode_allowed(&ctx.motion_mode_ctx())
        && is_valid_unipred_ref(ctx.ref_pruning, InterCandGroup::Warp, list_idx, ref_idx);
    if enable_wm && is_warp_allowed {
        let mut cand = simple;
        cand.is_interintra_used = false;
        cand.motion_mode = MotionMode::WarpedCausal;
        cand.wm_params_l0.wm_type = TransformationType::Affine;

        let mut motion_mode_valid = true;
        if cand.mode == PredictionMode::NewMv
            && ctx.wm_ctrls.refinement_iterations != 0
            && ctx.wm_ctrls.refine_level == 0
        {
            motion_mode_valid = hooks.wm_motion_refinement(&mut cand);
        }
        if motion_mode_valid {
            motion_mode_valid = hooks.warped_motion_parameters(&mut cand);
        }
        if motion_mode_valid {
            cands.push(cand);
        }
    }

    // ---- OBMC ----
    let is_obmc_allowed =
        is_valid_unipred_ref(ctx.ref_pruning, InterCandGroup::Obmc, list_idx, ref_idx)
            && obmc_motion_mode_allowed(
                &ctx.motion_mode_ctx(),
                ctx.bsize,
                0,
                ctx.global_motion[simple.ref_frame[0].max(0) as usize].wm_type,
                simple.ref_frame[0],
                simple.ref_frame[1],
                simple.mode as u8,
            ) == MotionMode::ObmcCausal;
    if enable_obmc && is_obmc_allowed {
        let mut cand = simple;
        cand.is_interintra_used = false;
        cand.motion_mode = MotionMode::ObmcCausal;
        let mut motion_mode_valid = true;
        if cand.mode == PredictionMode::NewMv && ctx.obmc_ctrls.refine_level == 0 {
            debug_assert_eq!(cand.ref_frame[1], NONE_FRAME);
            motion_mode_valid = hooks.obmc_motion_refinement(&mut cand);
        }
        if motion_mode_valid {
            cands.push(cand);
        }
    }
}

// ---------------------------------------------------------------------------
// The injectors
// ---------------------------------------------------------------------------

fn already_injected(ctx: &InjectCtx<'_>, log: &InjectedMvLog, mv0: Mv, mv1: Mv, rt: i8) -> bool {
    // C: `ctx->injected_mv_count == 0 || mv_is_already_injected(...) == false`
    // — the count-zero short circuit skips the CORRUPTED-MV CHECK too, so
    // the very first candidate is never rejected for an out-of-range MV.
    if log.count() == 0 {
        return false;
    }
    mv_is_already_injected(
        log,
        ctx.redundant_cand_ctrls,
        ctx.corrupted_mv_check,
        mv0,
        mv1,
        rt as u8,
        av1_set_ref_frame(rt),
    )
}

/// C `inject_mvp_candidates_ii` (mode_decision.c:1482-1655).
///
/// NEAREST / NEAR uni-pred and NEAREST_NEAREST / NEAR_NEAR compound
/// candidates walked out of the ref-MV stack over the DRL range. The
/// single largest source of inter candidates.
///
/// **The NEAR loops are capped to ZERO unless `near_count_ctrls.enabled`**:
/// C initialises `cap_max_drl_index = 0` and only assigns
/// `MIN(near_count, max_drl_index)` inside the `if`. So with the control
/// off, no NEAR candidate is injected at all — the cap is not a
/// refinement of `max_drl_index`, it REPLACES it.
pub fn inject_mvp_candidates_ii(
    ctx: &InjectCtx<'_>,
    cands: &mut CandArray,
    log: &mut InjectedMvLog,
    hooks: &mut impl InjectHooks,
    allow_bipred: bool,
) {
    for &ref_pair in ctx.ref_frame_type_arr {
        let rf = av1_set_ref_frame(ref_pair);
        if rf[1] == NONE_FRAME {
            let frame_type = rf[0];
            let list_idx = get_list_idx(rf[0]);
            let ref_idx = get_ref_frame_idx(rf[0]);
            if !is_valid_unipred_ref(ctx.ref_pruning, InterCandGroup::NrstNear, list_idx, ref_idx) {
                continue;
            }
            let stack = &ctx.ref_mv_stack[frame_type.max(0) as usize];

            // NEAREST
            let to_inj_mv = stack.stack[0].this_mv;
            if !already_injected(ctx, log, to_inj_mv, to_inj_mv, frame_type) {
                let mut cand = InterCandidate {
                    mode: PredictionMode::NearestMv,
                    motion_mode: MotionMode::SimpleTranslation,
                    use_intrabc: false,
                    skip_mode_allowed: false,
                    drl_index: 0,
                    ref_frame: rf,
                    is_interintra_used: false,
                    num_proj_ref: ctx.wm_sample_num[frame_type.max(0) as usize],
                    ..Default::default()
                };
                cand.mv[0] = to_inj_mv;
                cands.push(cand);
                inj_non_simple_modes(ctx, cands, hooks, true, ctx.wm_ctrls.use_wm_for_mvp, true);
                log.push([to_inj_mv, Mv::ZERO], frame_type as u8);
            }

            // NEAR
            let max_drl_index = get_max_drl_index(
                ctx.ref_mv_count[frame_type.max(0) as usize],
                PredictionMode::NearMv,
            );
            let cap = if ctx.near_count_ctrls.enabled {
                ctx.near_count_ctrls.near_count.min(max_drl_index)
            } else {
                0
            };
            let mut carried = DrlMvPred::default();
            for drli in 0..cap {
                let pred = get_av1_mv_pred_drl(
                    stack,
                    false,
                    PredictionMode::NearMv as u8,
                    usize::from(drli),
                    carried,
                );
                carried = pred;
                let to_inj_mv = pred.nearmv[0];
                if already_injected(ctx, log, to_inj_mv, to_inj_mv, frame_type) {
                    continue;
                }
                let mut cand = InterCandidate {
                    mode: PredictionMode::NearMv,
                    motion_mode: MotionMode::SimpleTranslation,
                    use_intrabc: false,
                    skip_mode_allowed: false,
                    drl_index: drli,
                    ref_frame: rf,
                    is_interintra_used: false,
                    num_proj_ref: ctx.wm_sample_num[frame_type.max(0) as usize],
                    ..Default::default()
                };
                cand.mv[0] = to_inj_mv;
                cands.push(cand);
                inj_non_simple_modes(ctx, cands, hooks, true, ctx.wm_ctrls.use_wm_for_mvp, true);
                log.push([to_inj_mv, Mv::ZERO], frame_type as u8);
            }
        } else if allow_bipred {
            let ref_idx_0 = get_ref_frame_idx(rf[0]);
            let ref_idx_1 = get_ref_frame_idx(rf[1]);
            let list_idx_0 = get_list_idx(rf[0]);
            let list_idx_1 = get_list_idx(rf[1]);
            if !is_valid_bipred_ref(
                ctx.ref_pruning,
                InterCandGroup::NrstNear,
                list_idx_0,
                ref_idx_0,
                list_idx_1,
                ref_idx_1,
            ) {
                continue;
            }
            let stack = &ctx.ref_mv_stack[ref_pair.max(0) as usize];

            // NEAREST_NEAREST
            let to_inj_mv0 = stack.stack[0].this_mv;
            let to_inj_mv1 = stack.stack[0].comp_mv;
            if !already_injected(ctx, log, to_inj_mv0, to_inj_mv1, ref_pair) {
                let is_skip_mode = !ctx.is_lossless_segment
                    && ctx.skip_mode_flag
                    && rf[0] == ctx.skip_mode_ref_frame_idx_0
                    && rf[1] == ctx.skip_mode_ref_frame_idx_1;
                let mut cand = InterCandidate {
                    mode: PredictionMode::NearestNearestMv,
                    motion_mode: MotionMode::SimpleTranslation,
                    is_interintra_used: false,
                    use_intrabc: false,
                    skip_mode_allowed: is_skip_mode,
                    drl_index: 0,
                    ref_frame: rf,
                    ..Default::default()
                };
                cand.mv = [to_inj_mv0, to_inj_mv1];
                determine_compound_mode(&mut cand, 0, hooks);
                cands.push(cand);
                if ctx.inter_comp_ctrls.do_nearest_nearest {
                    inj_comp_modes(ctx, cands, hooks);
                }
                log.push([to_inj_mv0, to_inj_mv1], ref_pair as u8);
            }

            // NEAR_NEAR
            let max_drl_index = get_max_drl_index(
                ctx.ref_mv_count[ref_pair.max(0) as usize],
                PredictionMode::NearNearMv,
            );
            let cap = if ctx.near_count_ctrls.enabled {
                ctx.near_count_ctrls.near_near_count.min(max_drl_index)
            } else {
                0
            };
            let mut carried = DrlMvPred::default();
            for drli in 0..cap {
                let pred = get_av1_mv_pred_drl(
                    stack,
                    true,
                    PredictionMode::NearNearMv as u8,
                    usize::from(drli),
                    carried,
                );
                carried = pred;
                let to_inj_mv0 = pred.nearmv[0];
                let to_inj_mv1 = pred.nearmv[1];
                if already_injected(ctx, log, to_inj_mv0, to_inj_mv1, ref_pair) {
                    continue;
                }
                let mut cand = InterCandidate {
                    mode: PredictionMode::NearNearMv,
                    motion_mode: MotionMode::SimpleTranslation,
                    is_interintra_used: false,
                    use_intrabc: false,
                    skip_mode_allowed: false,
                    drl_index: drli,
                    ref_frame: rf,
                    ..Default::default()
                };
                cand.mv = [to_inj_mv0, to_inj_mv1];
                determine_compound_mode(&mut cand, 0, hooks);
                cands.push(cand);
                if ctx.inter_comp_ctrls.do_near_near {
                    inj_comp_modes(ctx, cands, hooks);
                }
                log.push([to_inj_mv0, to_inj_mv1], ref_pair as u8);
            }
        }
    }
}

/// C `inject_new_nearest_new_comb_candidates` (mode_decision.c:1657-1870).
///
/// NEAREST_NEWMV / NEW_NEARESTMV / NEAR_NEWMV / NEW_NEARMV — exactly the
/// four modes whose DRL and MV-write predicates disagree (documented in
/// [`crate::inter_mv_code`]), so omitting them removes half the compound
/// inter mode space.
///
/// **`new_nearest_near_comb_injection >= 2` stops after the first two
/// modes** (`continue`, not `break` — the outer reference loop keeps
/// going).
///
/// Note the asymmetric `is_me_data_present` list arguments C uses: the
/// NEW_NEARESTMV and NEW_NEARMV probes pass a LITERAL 0 for the list,
/// not `list_idx_0`. Transcribed as written.
pub fn inject_new_nearest_new_comb_candidates(
    ctx: &InjectCtx<'_>,
    cands: &mut CandArray,
    log: &mut InjectedMvLog,
    hooks: &mut impl InjectHooks,
) {
    for &ref_pair in ctx.ref_frame_type_arr {
        let rf = av1_set_ref_frame(ref_pair);
        if rf[1] == NONE_FRAME {
            continue;
        }
        let ref_idx_0 = get_ref_frame_idx(rf[0]);
        let ref_idx_1 = get_ref_frame_idx(rf[1]);
        let list_idx_0 = get_list_idx(rf[0]);
        let list_idx_1 = get_list_idx(rf[1]);
        if !is_valid_unipred_ref(
            ctx.ref_pruning,
            InterCandGroup::NrstNewNear,
            list_idx_0,
            ref_idx_0,
        ) || !is_valid_unipred_ref(
            ctx.ref_pruning,
            InterCandGroup::NrstNewNear,
            list_idx_1,
            ref_idx_1,
        ) {
            continue;
        }
        let stack = &ctx.ref_mv_stack[ref_pair.max(0) as usize];
        let me_present = |list: usize, r: usize| {
            is_me_data_present(
                ctx.me_block_offset,
                ctx.me_totals,
                ctx.me_cands,
                list as u8,
                r as u8,
            )
        };

        // NEAREST_NEWMV
        let to_inj_mv0 = stack.stack[0].this_mv;
        let to_inj_mv1 = ctx.sb_me_mv[list_idx_1][ref_idx_1];
        if !already_injected(ctx, log, to_inj_mv0, to_inj_mv1, ref_pair)
            && me_present(get_list_idx(rf[1]), ref_idx_1)
        {
            let pred = get_av1_mv_pred_drl(
                stack,
                true,
                PredictionMode::NearestNewMv as u8,
                0,
                DrlMvPred::default(),
            );
            let mut cand = InterCandidate {
                mode: PredictionMode::NearestNewMv,
                motion_mode: MotionMode::SimpleTranslation,
                is_interintra_used: false,
                use_intrabc: false,
                skip_mode_allowed: false,
                drl_index: 0,
                ref_frame: rf,
                ..Default::default()
            };
            cand.mv = [to_inj_mv0, to_inj_mv1];
            cand.pred_mv[1] = pred.ref_mv[1];
            determine_compound_mode(&mut cand, 0, hooks);
            cands.push(cand);
            if ctx.inter_comp_ctrls.do_nearest_near_new {
                inj_comp_modes(ctx, cands, hooks);
            }
            log.push([to_inj_mv0, to_inj_mv1], ref_pair as u8);
        }

        // NEW_NEARESTMV
        let to_inj_mv0 = ctx.sb_me_mv[list_idx_0][ref_idx_0];
        let to_inj_mv1 = stack.stack[0].comp_mv;
        if !already_injected(ctx, log, to_inj_mv0, to_inj_mv1, ref_pair) && me_present(0, ref_idx_0)
        {
            let pred = get_av1_mv_pred_drl(
                stack,
                true,
                PredictionMode::NewNearestMv as u8,
                0,
                DrlMvPred::default(),
            );
            let mut cand = InterCandidate {
                mode: PredictionMode::NewNearestMv,
                motion_mode: MotionMode::SimpleTranslation,
                is_interintra_used: false,
                use_intrabc: false,
                skip_mode_allowed: false,
                drl_index: 0,
                ref_frame: rf,
                ..Default::default()
            };
            cand.mv = [to_inj_mv0, to_inj_mv1];
            cand.pred_mv[0] = pred.ref_mv[0];
            determine_compound_mode(&mut cand, 0, hooks);
            cands.push(cand);
            if ctx.inter_comp_ctrls.do_nearest_near_new {
                inj_comp_modes(ctx, cands, hooks);
            }
            log.push([to_inj_mv0, to_inj_mv1], ref_pair as u8);
        }

        if ctx.new_nearest_near_comb_injection >= 2 {
            continue;
        }

        // NEW_NEARMV
        let max_drl_index = get_max_drl_index(
            ctx.ref_mv_count[ref_pair.max(0) as usize],
            PredictionMode::NewNearMv,
        );
        let mut carried = DrlMvPred::default();
        for drli in 0..max_drl_index {
            let pred = get_av1_mv_pred_drl(
                stack,
                true,
                PredictionMode::NewNearMv as u8,
                usize::from(drli),
                carried,
            );
            carried = pred;
            let to_inj_mv0 = ctx.sb_me_mv[list_idx_0][ref_idx_0];
            let to_inj_mv1 = pred.nearmv[1];
            if already_injected(ctx, log, to_inj_mv0, to_inj_mv1, ref_pair)
                || !me_present(0, ref_idx_0)
            {
                continue;
            }
            let mut cand = InterCandidate {
                mode: PredictionMode::NewNearMv,
                motion_mode: MotionMode::SimpleTranslation,
                is_interintra_used: false,
                use_intrabc: false,
                skip_mode_allowed: false,
                drl_index: drli,
                ref_frame: rf,
                ..Default::default()
            };
            cand.mv = [to_inj_mv0, to_inj_mv1];
            cand.pred_mv[0] = pred.ref_mv[0];
            determine_compound_mode(&mut cand, 0, hooks);
            cands.push(cand);
            if ctx.inter_comp_ctrls.do_nearest_near_new {
                inj_comp_modes(ctx, cands, hooks);
            }
            log.push([to_inj_mv0, to_inj_mv1], ref_pair as u8);
        }

        // NEAR_NEWMV
        let max_drl_index = get_max_drl_index(
            ctx.ref_mv_count[ref_pair.max(0) as usize],
            PredictionMode::NearNewMv,
        );
        let mut carried = DrlMvPred::default();
        for drli in 0..max_drl_index {
            let pred = get_av1_mv_pred_drl(
                stack,
                true,
                PredictionMode::NearNewMv as u8,
                usize::from(drli),
                carried,
            );
            carried = pred;
            let to_inj_mv0 = pred.nearmv[0];
            let to_inj_mv1 = ctx.sb_me_mv[list_idx_1][ref_idx_1];
            if already_injected(ctx, log, to_inj_mv0, to_inj_mv1, ref_pair)
                || !me_present(list_idx_1, ref_idx_1)
            {
                continue;
            }
            let mut cand = InterCandidate {
                mode: PredictionMode::NearNewMv,
                motion_mode: MotionMode::SimpleTranslation,
                is_interintra_used: false,
                use_intrabc: false,
                skip_mode_allowed: false,
                drl_index: drli,
                ref_frame: rf,
                ..Default::default()
            };
            cand.mv = [to_inj_mv0, to_inj_mv1];
            cand.pred_mv[1] = pred.ref_mv[1];
            determine_compound_mode(&mut cand, 0, hooks);
            cands.push(cand);
            if ctx.inter_comp_ctrls.do_nearest_near_new {
                inj_comp_modes(ctx, cands, hooks);
            }
            log.push([to_inj_mv0, to_inj_mv1], ref_pair as u8);
        }
    }
}

/// C `inject_new_candidates` (mode_decision.c:2479-2601).
///
/// Turns the ported ME's `me_candidate_array` into NEWMV / NEW_NEWMV
/// candidates. This is the seam where [`crate::inter_me`] becomes visible
/// to mode decision; nothing else consumes it.
pub fn inject_new_candidates(
    ctx: &InjectCtx<'_>,
    cands: &mut CandArray,
    log: &mut InjectedMvLog,
    hooks: &mut impl InjectHooks,
    allow_bipred: bool,
) {
    for me_cand in ctx.me_cands.iter().take(ctx.total_me_cnt) {
        let inter_direction = me_cand.direction;
        let list0_ref_index = me_cand.ref_idx_l0;
        let list1_ref_index = me_cand.ref_idx_l1;

        if ctx.reduce_unipred_candidates != 0 && ctx.total_me_cnt > 3 && inter_direction != 2 {
            continue;
        }

        if inter_direction < BI_PRED {
            let list_idx = usize::from(inter_direction);
            let ref_idx = usize::from(if list_idx == 0 {
                list0_ref_index
            } else {
                list1_ref_index
            });
            if !is_valid_unipred_ref(ctx.ref_pruning, InterCandGroup::PaMe, list_idx, ref_idx) {
                continue;
            }
            let to_inj_mv = ctx.sb_me_mv[list_idx][ref_idx];
            let rt = get_ref_frame_type(list_idx as u8, ref_idx as u8) as i8;
            if already_injected(ctx, log, to_inj_mv, to_inj_mv, rt) {
                continue;
            }
            let (drl_index, best_pred_mv) =
                ctx.choose_drl(rt, PredictionMode::NewMv, to_inj_mv, Mv::ZERO);
            if ctx.corrupted_mv_check
                && !is_valid_mv_diff(best_pred_mv, to_inj_mv, to_inj_mv, false)
            {
                continue;
            }
            let mut cand = InterCandidate {
                mode: PredictionMode::NewMv,
                motion_mode: MotionMode::SimpleTranslation,
                is_interintra_used: false,
                use_intrabc: false,
                skip_mode_allowed: false,
                drl_index,
                ref_frame: [rt, NONE_FRAME],
                num_proj_ref: ctx.wm_sample_num[rt.max(0) as usize],
                ..Default::default()
            };
            cand.mv[0] = to_inj_mv;
            cand.pred_mv[0] = best_pred_mv[0];
            cands.push(cand);
            inj_non_simple_modes(ctx, cands, hooks, true, true, true);
            log.push([to_inj_mv, Mv::ZERO], rt as u8);
        } else if allow_bipred
            && !(ctx.is_intra_bordered && ctx.use_neighbouring_mode_ctrls_enabled)
        {
            if !is_valid_bipred_ref(
                ctx.ref_pruning,
                InterCandGroup::PaMe,
                usize::from(me_cand.ref0_list),
                usize::from(list0_ref_index),
                usize::from(me_cand.ref1_list),
                usize::from(list1_ref_index),
            ) {
                continue;
            }
            let to_inj_mv0 =
                ctx.sb_me_mv[usize::from(me_cand.ref0_list)][usize::from(list0_ref_index)];
            let to_inj_mv1 =
                ctx.sb_me_mv[usize::from(me_cand.ref1_list)][usize::from(list1_ref_index)];
            let rf = [
                get_ref_frame_type(me_cand.ref0_list, list0_ref_index) as i8,
                get_ref_frame_type(me_cand.ref1_list, list1_ref_index) as i8,
            ];
            let rt = av1_ref_frame_type(rf);
            if already_injected(ctx, log, to_inj_mv0, to_inj_mv1, rt) {
                continue;
            }
            let (drl_index, best_pred_mv) =
                ctx.choose_drl(rt, PredictionMode::NewNewMv, to_inj_mv0, to_inj_mv1);
            if ctx.corrupted_mv_check
                && !is_valid_mv_diff(best_pred_mv, to_inj_mv0, to_inj_mv1, true)
            {
                continue;
            }
            let mut cand = InterCandidate {
                mode: PredictionMode::NewNewMv,
                motion_mode: MotionMode::SimpleTranslation,
                is_interintra_used: false,
                use_intrabc: false,
                skip_mode_allowed: false,
                drl_index,
                ref_frame: rf,
                ..Default::default()
            };
            cand.mv = [to_inj_mv0, to_inj_mv1];
            cand.pred_mv = best_pred_mv;
            determine_compound_mode(&mut cand, 0, hooks);
            cands.push(cand);
            if ctx.inter_comp_ctrls.do_me {
                inj_comp_modes(ctx, cands, hooks);
            }
            log.push([to_inj_mv0, to_inj_mv1], rt as u8);
        }
    }
}

/// C `inject_global_candidates` (mode_decision.c:2603-2721).
///
/// GLOBALMV / GLOBAL_GLOBALMV from the frame's warp params.
/// [`crate::inter_mvp::gm_get_motion_vector_enc`] is already ported; this
/// is the only thing that turns it into a candidate.
///
/// **The compound arm's pruning failure is a `return`, not a `continue`**
/// — C abandons the WHOLE reference loop when
/// `is_valid_bipred_ref` rejects a pair, while the uni-pred arm
/// `continue`s. Transcribed as written.
pub fn inject_global_candidates(
    ctx: &InjectCtx<'_>,
    cands: &mut CandArray,
    log: &mut InjectedMvLog,
    hooks: &mut impl InjectHooks,
    allow_bipred: bool,
) {
    let mi_row = ctx.blk_org_y >> 2;
    let mi_col = ctx.blk_org_x >> 2;
    let gm_mv = |p: &WarpedMotionParams| {
        crate::inter_mvp::gm_get_motion_vector_enc(
            p,
            ctx.allow_high_precision_mv,
            ctx.bsize as usize,
            mi_col as i32,
            mi_row as i32,
            false,
        )
    };

    for &ref_pair in ctx.ref_frame_type_arr {
        let rf = av1_set_ref_frame(ref_pair);
        if rf[1] == NONE_FRAME {
            let frame_type = rf[0];
            let list_idx = get_list_idx(rf[0]);
            let ref_idx = get_ref_frame_idx(rf[0]);
            if !is_valid_unipred_ref(ctx.ref_pruning, InterCandGroup::Global, list_idx, ref_idx) {
                continue;
            }
            let gm_params = ctx.global_motion[frame_type.max(0) as usize];
            if ctx.gm_skip_identity && gm_params.wm_type == TransformationType::Identity {
                continue;
            }
            let to_inj_mv = gm_mv(&gm_params);
            let mut cand = InterCandidate {
                mode: PredictionMode::GlobalMv,
                motion_mode: MotionMode::SimpleTranslation,
                is_interintra_used: false,
                wm_params_l0: gm_params,
                wm_params_l1: gm_params,
                use_intrabc: false,
                skip_mode_allowed: false,
                drl_index: 0,
                ref_frame: rf,
                num_proj_ref: ctx.wm_sample_num[frame_type.max(0) as usize],
                ..Default::default()
            };
            cand.mv[0] = to_inj_mv;
            cands.push(cand);
            inj_non_simple_modes(ctx, cands, hooks, true, false, false);
            log.push([to_inj_mv, Mv::ZERO], frame_type as u8);
        } else if allow_bipred {
            let ref_idx_0 = get_ref_frame_idx(rf[0]);
            let ref_idx_1 = get_ref_frame_idx(rf[1]);
            let list_idx_0 = get_list_idx(rf[0]);
            let list_idx_1 = get_list_idx(rf[1]);
            if !is_valid_bipred_ref(
                ctx.ref_pruning,
                InterCandGroup::Global,
                list_idx_0,
                ref_idx_0,
                list_idx_1,
                ref_idx_1,
            ) {
                // C: `return`, abandoning the whole loop.
                return;
            }
            let gm0 = ctx.global_motion
                [get_ref_frame_type(list_idx_0 as u8, ref_idx_0 as u8).max(0) as usize];
            let gm1 = ctx.global_motion
                [get_ref_frame_type(list_idx_1 as u8, ref_idx_1 as u8).max(0) as usize];
            if ctx.gm_skip_identity
                && (gm0.wm_type == TransformationType::Identity
                    || gm1.wm_type == TransformationType::Identity)
            {
                continue;
            }
            let to_inj_mv0 = gm_mv(&gm0);
            let to_inj_mv1 = gm_mv(&gm1);
            let rt = av1_ref_frame_type(rf);
            let mut cand = InterCandidate {
                use_intrabc: false,
                skip_mode_allowed: false,
                mode: PredictionMode::GlobalGlobalMv,
                motion_mode: MotionMode::SimpleTranslation,
                wm_params_l0: gm0,
                wm_params_l1: gm1,
                is_interintra_used: false,
                drl_index: 0,
                ref_frame: rf,
                ..Default::default()
            };
            cand.mv = [to_inj_mv0, to_inj_mv1];
            determine_compound_mode(&mut cand, 0, hooks);
            cands.push(cand);
            if ctx.inter_comp_ctrls.do_global {
                inj_comp_modes(ctx, cands, hooks);
            }
            log.push([to_inj_mv0, to_inj_mv1], rt as u8);
        }
    }
}

/// C `inject_pme_candidates` (mode_decision.c:2723-2821): the consumer
/// half of predictive ME.
///
/// Unlike every other injector, this one is gated on
/// `ctx->valid_pme_mv[list][ref]` rather than on the reference-pruning
/// table — a PME MV that was not found is simply absent.
pub fn inject_pme_candidates(
    ctx: &InjectCtx<'_>,
    cands: &mut CandArray,
    log: &mut InjectedMvLog,
    hooks: &mut impl InjectHooks,
    allow_bipred: bool,
) {
    for &ref_pair in ctx.ref_frame_type_arr {
        let rf = av1_set_ref_frame(ref_pair);
        if rf[1] == NONE_FRAME {
            let frame_type = rf[0];
            let list_idx = get_list_idx(rf[0]);
            let ref_idx = get_ref_frame_idx(rf[0]);
            if !ctx.valid_pme_mv[list_idx][ref_idx] {
                continue;
            }
            let to_inj_mv = ctx.best_pme_mv[list_idx][ref_idx];
            if already_injected(ctx, log, to_inj_mv, to_inj_mv, frame_type) {
                continue;
            }
            let (drl_index, best_pred_mv) =
                ctx.choose_drl(frame_type, PredictionMode::NewMv, to_inj_mv, Mv::ZERO);
            if ctx.corrupted_mv_check
                && !is_valid_mv_diff(best_pred_mv, to_inj_mv, to_inj_mv, false)
            {
                continue;
            }
            let mut cand = InterCandidate {
                use_intrabc: false,
                skip_mode_allowed: false,
                mode: PredictionMode::NewMv,
                motion_mode: MotionMode::SimpleTranslation,
                is_interintra_used: false,
                drl_index,
                ref_frame: rf,
                num_proj_ref: ctx.wm_sample_num[frame_type.max(0) as usize],
                ..Default::default()
            };
            cand.mv[0] = to_inj_mv;
            cand.pred_mv[0] = best_pred_mv[0];
            cands.push(cand);
            inj_non_simple_modes(ctx, cands, hooks, true, true, true);
            log.push([to_inj_mv, Mv::ZERO], frame_type as u8);
        } else if allow_bipred {
            let ref_idx_0 = get_ref_frame_idx(rf[0]);
            let ref_idx_1 = get_ref_frame_idx(rf[1]);
            let list_idx_0 = get_list_idx(rf[0]);
            let list_idx_1 = get_list_idx(rf[1]);
            if !ctx.valid_pme_mv[list_idx_0][ref_idx_0] || !ctx.valid_pme_mv[list_idx_1][ref_idx_1]
            {
                continue;
            }
            let to_inj_mv0 = ctx.best_pme_mv[list_idx_0][ref_idx_0];
            let to_inj_mv1 = ctx.best_pme_mv[list_idx_1][ref_idx_1];
            let rt = av1_ref_frame_type([
                get_ref_frame_type(list_idx_0 as u8, ref_idx_0 as u8) as i8,
                get_ref_frame_type(list_idx_1 as u8, ref_idx_1 as u8) as i8,
            ]);
            if already_injected(ctx, log, to_inj_mv0, to_inj_mv1, rt) {
                continue;
            }
            let (drl_index, best_pred_mv) =
                ctx.choose_drl(rt, PredictionMode::NewNewMv, to_inj_mv0, to_inj_mv1);
            if ctx.corrupted_mv_check
                && !is_valid_mv_diff(best_pred_mv, to_inj_mv0, to_inj_mv1, true)
            {
                continue;
            }
            let mut cand = InterCandidate {
                use_intrabc: false,
                skip_mode_allowed: false,
                drl_index,
                mode: PredictionMode::NewNewMv,
                motion_mode: MotionMode::SimpleTranslation,
                is_interintra_used: false,
                ref_frame: rf,
                ..Default::default()
            };
            cand.mv = [to_inj_mv0, to_inj_mv1];
            cand.pred_mv = best_pred_mv;
            determine_compound_mode(&mut cand, 0, hooks);
            cands.push(cand);
            if ctx.inter_comp_ctrls.do_pme {
                inj_comp_modes(ctx, cands, hooks);
            }
            log.push([to_inj_mv0, to_inj_mv1], rt as u8);
        }
    }
}

/// C `unipred_3x3_candidates_injection` (mode_decision.c:1084-1163).
///
/// The +-1 refinement candidates around the best uni-pred MV. The step is
/// `<< !allow_high_precision_mv`, i.e. **2 eighth-pel units at quarter-pel
/// precision and 1 at eighth-pel** — not a fixed offset.
///
/// `unipred3x3_injection >= 2` restricts the eight positions to the four
/// with `allow_refinement_flag == 1` (the axis-aligned ones).
pub fn unipred_3x3_candidates_injection(
    ctx: &InjectCtx<'_>,
    cands: &mut CandArray,
    log: &mut InjectedMvLog,
    hooks: &mut impl InjectHooks,
) {
    let shift = u32::from(!ctx.allow_high_precision_mv);
    for me_cand in ctx.me_cands.iter().take(ctx.total_me_cnt) {
        if me_cand.direction == BI_PRED {
            continue;
        }
        let list_idx = usize::from(me_cand.direction);
        let ref_idx = usize::from(if list_idx == 0 {
            me_cand.ref_idx_l0
        } else {
            me_cand.ref_idx_l1
        });
        if !is_valid_unipred_ref(ctx.ref_pruning, InterCandGroup::Uni3x3, list_idx, ref_idx) {
            continue;
        }
        for pos in 0..BIPRED_3X3_REFINEMENT_POSITIONS {
            if ctx.unipred3x3_injection >= 2 && ALLOW_REFINEMENT_FLAG[pos] == 0 {
                continue;
            }
            let base = ctx.sb_me_mv[list_idx][ref_idx];
            let to_inj_mv = Mv {
                x: base
                    .x
                    .wrapping_add(i16::from(BIPRED_3X3_X_POS[pos]) << shift),
                y: base
                    .y
                    .wrapping_add(i16::from(BIPRED_3X3_Y_POS[pos]) << shift),
            };
            let rt = get_ref_frame_type(list_idx as u8, ref_idx as u8) as i8;
            if already_injected(ctx, log, to_inj_mv, to_inj_mv, rt) {
                continue;
            }
            let (drl_index, best_pred_mv) =
                ctx.choose_drl(rt, PredictionMode::NewMv, to_inj_mv, Mv::ZERO);
            if ctx.corrupted_mv_check
                && !is_valid_mv_diff(best_pred_mv, to_inj_mv, to_inj_mv, false)
            {
                continue;
            }
            let mut cand = InterCandidate {
                use_intrabc: false,
                skip_mode_allowed: false,
                mode: PredictionMode::NewMv,
                motion_mode: MotionMode::SimpleTranslation,
                is_interintra_used: false,
                drl_index,
                ref_frame: [rt, NONE_FRAME],
                num_proj_ref: ctx.wm_sample_num[rt.max(0) as usize],
                ..Default::default()
            };
            cand.mv[0] = to_inj_mv;
            cand.pred_mv[0] = best_pred_mv[0];
            cands.push(cand);
            // OBMC and WM run their own refinement around the ME MV, so
            // they are NOT injected here — this whole function already is
            // a refinement search.
            inj_non_simple_modes(ctx, cands, hooks, true, false, false);
            log.push([to_inj_mv, Mv::ZERO], rt as u8);
        }
    }
}

/// C `bipred_3x3_candidates_injection` (mode_decision.c:1165-2011).
///
/// The same +-1 refinement for compound candidates, in two halves: the
/// first perturbs `mv1` around a fixed `mv0`, the second perturbs `mv0`
/// around a fixed `mv1`.
///
/// **`use_l0_l1_dev`'s failure is a `return`**, abandoning the whole ME
/// candidate loop rather than skipping this candidate. Transcribed as
/// written.
pub fn bipred_3x3_candidates_injection(
    ctx: &InjectCtx<'_>,
    cands: &mut CandArray,
    log: &mut InjectedMvLog,
    hooks: &mut impl InjectHooks,
) {
    let shift = u32::from(!ctx.allow_high_precision_mv);
    for me_cand in ctx.me_cands.iter().take(ctx.total_me_cnt) {
        if me_cand.direction < BI_PRED {
            continue;
        }
        let ref0_list = usize::from(me_cand.ref0_list);
        let ref1_list = usize::from(me_cand.ref1_list);
        let l0 = usize::from(me_cand.ref_idx_l0);
        let l1 = usize::from(me_cand.ref_idx_l1);
        if !is_valid_bipred_ref(
            ctx.ref_pruning,
            InterCandGroup::Bi3x3,
            ref0_list,
            l0,
            ref1_list,
            l1,
        ) {
            continue;
        }

        let diff = (ctx.post_subpel_me_mv_cost[ref0_list][l0] as i64
            - ctx.post_subpel_me_mv_cost[ref1_list][l1] as i64)
            * 100;
        if ctx.bipred3x3_ctrls.use_l0_l1_dev != 0xFF {
            let bound = i64::from(ctx.bipred3x3_ctrls.use_l0_l1_dev)
                * ctx.post_subpel_me_mv_cost[ref0_list][l0] as i64;
            if diff.abs() > bound {
                // C: `return`, not `continue`.
                return;
            }
        }
        let best_list: i8 = if ctx.bipred3x3_ctrls.use_best_list {
            if diff > 0 {
                ref1_list as i8
            } else {
                ref0_list as i8
            }
        } else {
            -1
        };

        let rf = [
            get_ref_frame_type(me_cand.ref0_list, me_cand.ref_idx_l0) as i8,
            get_ref_frame_type(me_cand.ref1_list, me_cand.ref_idx_l1) as i8,
        ];
        let rt = av1_ref_frame_type(rf);

        let mut half = |perturb_mv1: bool, cands: &mut CandArray, log: &mut InjectedMvLog| {
            for pos in 0..BIPRED_3X3_REFINEMENT_POSITIONS {
                if !ctx.bipred3x3_ctrls.search_diag && ALLOW_REFINEMENT_FLAG[pos] == 0 {
                    continue;
                }
                let dx = i16::from(BIPRED_3X3_X_POS[pos]) << shift;
                let dy = i16::from(BIPRED_3X3_Y_POS[pos]) << shift;
                let base0 = ctx.sb_me_mv[ref0_list][l0];
                let base1 = ctx.sb_me_mv[ref1_list][l1];
                let (to_inj_mv0, to_inj_mv1) = if perturb_mv1 {
                    (
                        base0,
                        Mv {
                            x: base1.x.wrapping_add(dx),
                            y: base1.y.wrapping_add(dy),
                        },
                    )
                } else {
                    (
                        Mv {
                            x: base0.x.wrapping_add(dx),
                            y: base0.y.wrapping_add(dy),
                        },
                        base1,
                    )
                };
                if already_injected(ctx, log, to_inj_mv0, to_inj_mv1, rt) {
                    continue;
                }
                let (drl_index, best_pred_mv) =
                    ctx.choose_drl(rt, PredictionMode::NewNewMv, to_inj_mv0, to_inj_mv1);
                if ctx.corrupted_mv_check
                    && !is_valid_mv_diff(best_pred_mv, to_inj_mv0, to_inj_mv1, true)
                {
                    continue;
                }
                let mut cand = InterCandidate {
                    use_intrabc: false,
                    skip_mode_allowed: false,
                    drl_index,
                    mode: PredictionMode::NewNewMv,
                    motion_mode: MotionMode::SimpleTranslation,
                    is_interintra_used: false,
                    ref_frame: rf,
                    ..Default::default()
                };
                cand.mv = [to_inj_mv0, to_inj_mv1];
                cand.pred_mv = best_pred_mv;
                determine_compound_mode(&mut cand, 0, hooks);
                cands.push(cand);
                if ctx.inter_comp_ctrls.do_3x3_bi {
                    inj_comp_modes(ctx, cands, hooks);
                }
                log.push([to_inj_mv0, to_inj_mv1], rt as u8);
            }
        };

        if best_list == -1 || best_list == ref0_list as i8 {
            half(true, cands, log);
        }
        if best_list == -1 || best_list == ref1_list as i8 {
            half(false, cands, log);
        }
    }
}

/// C `inject_zz_backup_candidate` (mode_decision.c:3314-3346).
///
/// The zero-MV LAST NEWMV fallback injected when the candidate list would
/// otherwise be empty. Without it an aggressively-pruned block has no
/// candidate and MD has no defined winner.
///
/// Note C writes `cand_array[cand_total_cnt].drl_index = 0` BEFORE
/// calling `svt_aom_choose_best_av1_mv_pred` and passes a pointer to that
/// field, so on the `shut_fast_rate` path (where the function writes
/// nothing) the DRL index is the 0 that was just stored.
pub fn inject_zz_backup_candidate(ctx: &InjectCtx<'_>, cands: &mut CandArray) {
    let rt = get_ref_frame_type(0, 0) as i8;
    let (drl_index, best_pred_mv) = {
        let stack = &ctx.ref_mv_stack[rt.max(0) as usize];
        let count = ctx.ref_mv_count[rt.max(0) as usize];
        let dctx = ChooseDrlCtx {
            shut_fast_rate: ctx.shut_fast_rate,
            approx_inter_rate: ctx.approx_inter_rate,
            ref_mv_stack: &stack.stack,
            ref_mv_count: count,
            nmv_cost: ctx.nmv_cost,
            drl_mode_fac_bits: ctx.drl_mode_fac_bits,
        };
        let mut drl = 0u8;
        let mut pred = [Mv::ZERO; 2];
        choose_best_av1_mv_pred(
            &dctx,
            PredictionMode::NewMv,
            Mv::ZERO,
            Mv::ZERO,
            &mut drl,
            &mut pred,
        );
        (drl, pred)
    };
    if ctx.corrupted_mv_check && !is_valid_mv_diff(best_pred_mv, Mv::ZERO, Mv::ZERO, false) {
        return;
    }
    let mut cand = InterCandidate {
        use_intrabc: false,
        skip_mode_allowed: false,
        mode: PredictionMode::NewMv,
        motion_mode: MotionMode::SimpleTranslation,
        ref_frame: [rt, NONE_FRAME],
        transform_type_y: 0, // DCT_DCT
        transform_type_uv: 0,
        is_interintra_used: false,
        drl_index,
        num_proj_ref: ctx.wm_sample_num[rt.max(0) as usize],
        ..Default::default()
    };
    cand.mv[0] = Mv::ZERO;
    cand.pred_mv[0] = best_pred_mv[0];
    cands.push(cand);
}

/// C `svt_aom_inject_inter_candidates` (mode_decision.c:2867-2960).
///
/// **`static` despite the `svt_aom_` prefix** — verified with `nm -g`,
/// which is why this group's inventory warns against inferring linkage
/// from the name.
///
/// The dispatch order is load-bearing: MVP -> NEW/NEAREST combos -> ME
/// NEW -> global -> bipred 3x3 -> unipred 3x3 -> PME. Each stage sees the
/// injected-MV log the previous ones filled, so reordering changes the
/// dedup outcome even with identical inputs.
///
/// C also calls `svt_av1_count_overlappable_neighbors`,
/// `svt_aom_init_wm_samples` and (under `obmc_ctrls.refine_level == 0`)
/// `svt_aom_precompute_obmc_data` before the injectors. Those write
/// `ctx->blk_ptr->overlappable_neighbors`, `ctx->wm_sample_info` and the
/// OBMC prediction buffers respectively; here they are INPUTS on
/// [`InjectCtx`] rather than steps, because they are neighbour-array and
/// pixel machinery that belongs outside this module.
pub fn inject_inter_candidates(
    ctx: &InjectCtx<'_>,
    cands: &mut CandArray,
    log: &mut InjectedMvLog,
    hooks: &mut impl InjectHooks,
) {
    let allow_bipred = ctx.allow_bipred();

    if ctx.new_nearest_injection
        && !(ctx.is_intra_bordered && ctx.use_neighbouring_mode_ctrls_enabled)
    {
        inject_mvp_candidates_ii(ctx, cands, log, hooks, allow_bipred);
    }
    if ctx.new_nearest_near_comb_injection != 0 && allow_bipred {
        inject_new_nearest_new_comb_candidates(ctx, cands, log, hooks);
    }
    if ctx.inject_new_me {
        inject_new_candidates(ctx, cands, log, hooks, allow_bipred);
    }
    if ctx.global_mv_injection {
        inject_global_candidates(ctx, cands, log, hooks, allow_bipred);
    }
    if ctx.bipred3x3_ctrls.enabled && allow_bipred {
        bipred_3x3_candidates_injection(ctx, cands, log, hooks);
    }
    if ctx.unipred3x3_injection != 0 {
        unipred_3x3_candidates_injection(ctx, cands, log, hooks);
    }
    if ctx.inject_new_pme && ctx.updated_enable_pme {
        inject_pme_candidates(ctx, cands, log, hooks, allow_bipred);
    }
}

// ---------------------------------------------------------------------------
// PD0
// ---------------------------------------------------------------------------

/// C `inject_new_candidates_pd0` (mode_decision.c:2293-2370).
///
/// PD0's reduced NEWMV injector: no dedup, no DRL, no motion modes, and a
/// hard cap of `cand_total_cnt > 2` checked AFTER each injection.
///
/// **`pd0_level == PD0_LVL_6` skips BI_PRED entirely** (a separate gate
/// from `allow_bipred`), and the MV comes straight from
/// `me_mv_array[me_block_offset * max_refs + (dir ? max_l0 : 0) + ref]`
/// multiplied by 8 — no `sb_me_mv` refinement, because PD0 runs before
/// any of it.
#[allow(clippy::too_many_arguments)]
pub fn inject_new_candidates_pd0(
    ctx: &InjectCtx<'_>,
    cands: &mut CandArray,
    hooks: &mut impl InjectHooks,
    me_mv_array: &[Mv],
    me_block_offset: usize,
    max_refs: usize,
    max_l0: usize,
    pd0_level_is_lvl6: bool,
    allow_bipred: bool,
) {
    for me_cand in ctx.me_cands.iter().take(ctx.total_me_cnt) {
        let dir = me_cand.direction;
        if pd0_level_is_lvl6 && dir == BI_PRED {
            continue;
        }
        if dir < BI_PRED {
            let list_idx = dir;
            let ref_idx = usize::from(if dir != 0 {
                me_cand.ref_idx_l1
            } else {
                me_cand.ref_idx_l0
            });
            let idx = me_block_offset * max_refs + if dir != 0 { max_l0 } else { 0 } + ref_idx;
            let m = me_mv_array[idx];
            let mut cand = InterCandidate {
                mode: PredictionMode::NewMv,
                ref_frame: [
                    get_ref_frame_type(list_idx, ref_idx as u8) as i8,
                    NONE_FRAME,
                ],
                ..Default::default()
            };
            cand.mv[0] = Mv {
                x: m.x.wrapping_mul(8),
                y: m.y.wrapping_mul(8),
            };
            cands.push(cand);
            if cands.count() > 2 {
                break;
            }
        } else if allow_bipred {
            let off0 = me_block_offset * max_refs
                + if me_cand.ref0_list > 0 { max_l0 } else { 0 }
                + usize::from(me_cand.ref_idx_l0);
            let off1 = me_block_offset * max_refs
                + if me_cand.ref1_list > 0 { max_l0 } else { 0 }
                + usize::from(me_cand.ref_idx_l1);
            let m0 = me_mv_array[off0];
            let m1 = me_mv_array[off1];
            let rf = [
                get_ref_frame_type(me_cand.ref0_list, me_cand.ref_idx_l0) as i8,
                get_ref_frame_type(me_cand.ref1_list, me_cand.ref_idx_l1) as i8,
            ];
            let mut cand = InterCandidate {
                mode: PredictionMode::NewNewMv,
                ref_frame: rf,
                ..Default::default()
            };
            cand.mv = [
                Mv {
                    x: m0.x.wrapping_mul(8),
                    y: m0.y.wrapping_mul(8),
                },
                Mv {
                    x: m1.x.wrapping_mul(8),
                    y: m1.y.wrapping_mul(8),
                },
            ];
            determine_compound_mode(&mut cand, 0, hooks);
            cands.push(cand);
            if cands.count() > 2 {
                break;
            }
        }
    }
}

/// C `inject_inter_candidates_pd0` (mode_decision.c:2823-2834): PD0's
/// inter injection entry. It derives `allow_bipred` exactly as the PD1
/// entry does and forwards to [`inject_new_candidates_pd0`].
#[allow(clippy::too_many_arguments)]
pub fn inject_inter_candidates_pd0(
    ctx: &InjectCtx<'_>,
    cands: &mut CandArray,
    hooks: &mut impl InjectHooks,
    me_mv_array: &[Mv],
    me_block_offset: usize,
    max_refs: usize,
    max_l0: usize,
    pd0_level_is_lvl6: bool,
) {
    let allow_bipred = ctx.allow_bipred();
    inject_new_candidates_pd0(
        ctx,
        cands,
        hooks,
        me_mv_array,
        me_block_offset,
        max_refs,
        max_l0,
        pd0_level_is_lvl6,
        allow_bipred,
    );
}

// ---------------------------------------------------------------------------
// TIER 4 — every injector here is `static` in C (including
// `svt_aom_inject_inter_candidates`, whose prefix is misleading; `nm -g`
// is the authority). These are hand-derived vectors traced against the C
// source. The predicates they call are separately gated at TIER 1 in
// `super::predicates` / `super::drl`.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
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
        assert_eq!(check(2, &mut h), (1, 1, 2, 55)); // DIFF0 — mask_type 55
        assert_eq!(check(3, &mut h), (1, 1, 3, 0)); // WEDGE
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
        w.stack =
            vec![stack_with(4, &[(4, 8, 0, 0), (4, 8, 0, 0), (4, 8, 0, 0), (4, 8, 0, 0)]); 29];
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
        assert_eq!(cands.as_slice()[1].interinter_comp_type, 1);
        assert_eq!(cands.as_slice()[2].interinter_comp_type, 2);
        assert_eq!(cands.as_slice()[3].interinter_comp_type, 3);

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
        assert_eq!(cands.as_slice()[1].interinter_comp_type, 2);
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
}
