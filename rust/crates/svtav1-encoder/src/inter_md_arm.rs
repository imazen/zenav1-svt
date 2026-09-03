//! Frame-level state for the INTER branch of mode decision, and the per-block
//! candidate builder the leaf funnel injects through.
//!
//! `docs/INTER-ENCODE-PLAN.md` §1s items 1b, 2, 3 and 6: everything downstream
//! of mode decision is ported and gated, and every island this module reaches
//! (`inter_me_arm`, `inter_mvp`, `port_md::drl`, `port_rd_cost::inter_cost`,
//! `inter_pred_arm`) was ported with no caller. This is the caller.
//!
//! # What lives here and why it is not in `leaf_funnel`
//!
//! The funnel owns candidate EVALUATION — MDS0 -> MDS1 -> MDS3, one intra
//! candidate set at a time. What an inter candidate needs before it can enter
//! that pipeline is a different set of concerns entirely: an open-loop motion
//! search, a reference-MV stack over the MD mode-info grid, a DRL choice, a
//! motion-compensated prediction on three planes and C's `inter_fast_cost`.
//! Keeping them here means `leaf_funnel/inject.rs` gains ONE call rather than
//! six modules' worth of imports, and it means this code is reachable from a
//! test without standing up a funnel.
//!
//! # Scope, stated as a fraction (`docs/WORKING-ON-THIS.md` §NEVER CLAIM
//! FALSE COMPLETION)
//!
//! The candidate SET is C's own: `port_md::inject::inject_inter_candidates`,
//! a transcription of `mode_decision.c:2836-2921`, is what builds it — this
//! module fills the `InjectCtx` it takes and turns each candidate it returns
//! into a prediction and an `svt_aom_inter_fast_cost`. Nothing about which
//! candidates exist is re-decided here.
//!
//! What is MISSING is therefore a list of CONTROLS this module hands the
//! injector as OFF, and each one is a separate unported search rather than a
//! shortcut in the composition:
//!
//! * `inter_comp_ctrls` / bipred — C's `ref_frame_type_arr` on these frames
//!   is `[LAST, BWDREF, LAST_BWD]` with `reference_select = 1`
//!   (`SVT_INJCFG_OUT`, 2026-09-02). This module now carries the two SINGLE
//!   entries and DROPS `LAST_BWD`, and it hands the injector
//!   `reference_mode_is_single = true` so `allow_bipred` is false.
//!   **Those two suppressions are ONE missing feature, not two choices:**
//!   dropping the compound entry alone would still let
//!   `inject_new_candidates` build a `NEW_NEWMV` out of C's BI_PRED ME
//!   candidate — which exists, measured `dir=2` on `gradient 64x64 q40 p8`
//!   — and the assertion below would then refuse a candidate C really does
//!   inject.
//!   **And on the inter campaign's 96-cell grid it is worth ZERO coded
//!   blocks.** MEASURED 2026-09-02 (`tools/inter_cinter_census.sh`, plan
//!   §1z¹⁶): across 340 coded inter blocks C codes `rf[1] != NONE` on
//!   none of them, on the 40 cells that already match AND on the 55 that
//!   do not. Unsuppressing compound is still the right work for a grid
//!   that reaches it; it is not the next byte.
//!   The PREDICTION is not the obstacle:
//!   `svtav1_dsp::port_pd_pred::av1_inter_prediction_light_pd1` takes an
//!   `mvs` SLICE and runs the `jnt_convolve` compound path whenever it has
//!   two (`port_pd_pred.rs:240`); it is [`crate::inter_pred_arm`]'s ADAPTER
//!   that narrows it to one MV. Unsuppressing bipred therefore means
//!   widening that adapter, plus the `interinter_comp_type` / `compound_idx`
//!   half of the rate — a separate chunk.
//! * `wm_ctrls` (warped motion) and `obmc_ctrls` — the DSP is ported
//!   (`svtav1_dsp::obmc`, the warp family) and the PREDICTION drivers are
//!   not wired, so a warped or OBMC candidate could not be predicted. The
//!   injector would produce them; the ctrls are off and the module ASSERTS
//!   none arrive rather than dropping them silently.
//! * `inter_intra_comp_ctrls` — same shape.
//! * `unipred3x3_injection`, `bipred3x3_ctrls` — the 3x3 refinements are
//!   unported. `inject_new_pme` / `updated_enable_pme` are now ON:
//!   [`crate::inter_search_arm`] runs C's `build_single_ref_mvp_array` ->
//!   `read_refine_me_mvs` -> `pme_search` chain per reference.
//! * `near_count_ctrls` — C caps the NEAR DRL loop to ZERO unless this
//!   control is enabled (it REPLACES `max_drl_index`, it does not refine
//!   it), so `NEARMV` is absent exactly the way C makes it absent.
//! * `md_nsq_motion_search` — PORTED but NOT CALLED here. C runs it inside
//!   `read_refine_me_mvs` for every NSQ shape (`bwidth != bheight`), seeded
//!   from the square parent's `sq_sb_me_mv` and from a geometry-filtered
//!   set of sub-block ME MVs. Both are ME-TABLE machinery this module does
//!   not carry (there is no `pc_tree->tested_blk` mirror and no
//!   `pu_search_index_map` walk), so an NSQ block here takes the square
//!   path: `raw_me_mv * 8`, then sub-pel. Say what that makes untestable —
//!   on any cell whose winner is an NSQ shape this module's ME MV can
//!   differ from C's, and no assertion in this file can see it.
//!   MEASURED 2026-09-02: that is **94 of the 259 coded inter blocks on
//!   the 55 F1DIFF cells** (`tools/inter_cinter_census.sh`), against 12 of
//!   77 on the 40 that match — the widest measured reach of any control
//!   listed here. And it is TWO divergences, not one: C's
//!   `read_refine_me_mvs` also seeds an NSQ block from
//!   `(sq_sb_me_mv + 4) & ~7` instead of `raw_me_mv * 8` whenever the
//!   square parent was tested (`product_coding_loop.c:2857-2862`), before
//!   `md_nsq_motion_search` runs at all.
//!
//! What that leaves live is `NEARESTMV` and `NEWMV` off `LAST_FRAME` and
//! `BWDREF_FRAME`, plus one PME `NEWMV` per reference — with C's own
//! injection ORDER (MVP before NEW before PME) and C's own
//! `mv_is_already_injected` dedup.

use crate::inter_me_arm::FrameMe;
use crate::inter_mvp::NONE_FRAME;
use crate::inter_mvp::{InterMvpEnv, setup_ref_mv_list};
use crate::intrabc::TileMiBounds;
use crate::intrabc_mvp::{MvpGrid, MvpMiEntry, derive_block_ctx};
use crate::picture::PaddedRef;
use crate::port_entropy_inter::modes::{MotionMode, TransformationType};
use crate::port_entropy_inter::{InterCdfs, NeighborMi, Neighbors};
use crate::port_md::pme::{MV_VALS, MvCostTable};
use crate::port_md::ref_frame_rate::{NeighborRefCounts, RefFrameFacBits};
use crate::port_rd_cost::inter_cost::{
    InterBlock, InterCandidate, InterFacBits, InterFrame, inter_fast_cost,
};
use alloc::vec::Vec;
use svtav1_types::motion::Mv;
use svtav1_types::prediction::PredictionMode;

/// C `LAST_FRAME`.
pub const LAST_FRAME: i8 = 1;

/// The frame-level tables and pictures the inter branch of MD reads.
///
/// Built once per inter frame, shared by every leaf.
pub struct InterMdFrame<'a> {
    /// The DPB reference with C's replicated margins — what the MC indexes.
    ///
    /// This is `LAST_FRAME`'s. [`Self::padded_by_ref`] is the per-reference
    /// table the multi-reference path uses; the two agree on `LAST_FRAME`.
    pub padded: &'a PaddedRef,
    /// The padded DPB picture per `MvReferenceFrame` (index 1..=7), `None`
    /// for a reference this frame does not signal.
    ///
    /// **C maps LAST to DPB slot 0 and BWDREF to slot 3** and on this GOP
    /// both hold frame 0 (C's own `MEL1` line reports `l1ref == l0ref`).
    /// The pipeline ASSERTS that rather than assuming it — see
    /// `pipeline.rs`'s construction — because the day a real GOP puts a
    /// different picture in slot 3, a table that silently aliased the two
    /// would predict from the wrong picture with no failing test.
    pub padded_by_ref: [Option<&'a PaddedRef>; 8],
    /// C `pcs->ppcs->enhanced_pic` luma plane and its stride — the MD
    /// SOURCE the motion searches score against (not a recon).
    pub src: &'a [u8],
    pub src_stride: usize,
    /// C `ctx->ref_frame_type_arr[0..tot_ref_frame_types]`, restricted to
    /// the SINGLE-reference entries (see this module's header).
    pub ref_frame_type_arr: &'a [i8],
    /// The frame-constant halves of C's MD search context.
    pub search: crate::inter_search_arm::SearchFrameCfg,
    /// C `md_rate_est_ctx->nmv_vec_cost` / `nmvcoststack` in the shape
    /// [`crate::md_subpel`] wants. [`Self::nmv`] is the same tables in the
    /// shape `port_md` wants; both are built from one
    /// `svt_av1_build_nmv_cost_table` transcription (see
    /// [`nmv_cost_table`]).
    pub search_tables: crate::intrabc::MvCostTables,
    /// This frame's open-loop motion search.
    pub me: &'a FrameMe,
    /// C `md_rate_estimation_ptr`'s inter tables.
    pub fac: InterFacBits,
    /// C's reference-signalling tables.
    pub ref_fac: RefFrameFacBits,
    /// C `mvjcost` + `mvcost[2]` at the frame's MV precision.
    pub nmv: MvCostTable,
    /// The frame-header/sequence-header fields `inter_fast_cost` reads.
    pub interpolation_filter: u8,
    pub is_motion_mode_switchable: bool,
    pub allow_warped_motion: bool,
    pub force_integer_mv: bool,
    pub allow_high_precision_mv: bool,
    pub enable_dual_filter: bool,
    pub enable_masked_compound: bool,
    pub enable_jnt_comp: bool,
    pub enable_interintra_compound: bool,
    pub reference_mode_is_select: bool,
    pub allow_screen_content_tools: bool,
    pub order_hint: OrderHints,
    /// The MVP environment (global motion, temporal MVs, order hints).
    pub mvp_env: InterMvpEnv<'a>,
    /// mi geometry for the MVP scans.
    pub mi_rows: i32,
    pub mi_cols: i32,
    pub tile: TileMiBounds,
    pub sb_mi_size: i32,
    /// ALIGNED frame dims and the superblock size, for the MC clamp.
    pub frame_w: usize,
    pub frame_h: usize,
    pub sb_size: usize,
    /// C `ppcs->global_motion[ref].wmtype`.
    pub gm_wmtype: [TransformationType; 8],
    /// C `ppcs->update_type` — the rdmult BASE selector of
    /// `av1_lambda_assign_md`'s chain. Carried here so PD0 can build the SAME
    /// `full_sb_lambda_md[EB_8_BIT_MD]` the funnel's `c_quant` already has,
    /// rather than re-deriving the picture state per superblock.
    pub base_update_type: crate::port_rc_process::FrameUpdateType,
    /// C `update_lambda`'s own `gf_update_type` — the frame-type FACTOR row.
    /// It DISAGREES with [`Self::base_update_type`] on a flat low-delay P GOP;
    /// see `pd0::inter_full_lambda_8bit`.
    pub factor_update_type: crate::port_rc_process::FrameUpdateType,
    /// [SVT_HDR_MODE] `static_config.alt_lambda_factors`.
    pub alt_lambda_factors: bool,
}

/// The order-hint half of [`InterFrame`], owned so the borrow is local.
#[derive(Clone, Copy, Debug)]
pub struct OrderHints {
    pub enable_order_hint: bool,
    pub order_hint_bits: u32,
    pub cur_order_hint: i32,
    pub ref_order_hint: [i32; 7],
}

impl InterMdFrame<'_> {
    fn cost_frame(&self) -> InterFrame<'_> {
        InterFrame {
            allow_screen_content_tools: self.allow_screen_content_tools,
            // The port writes `skip_mode_present = 0` on every frame it
            // emits, so no candidate can be a skip-mode one and the flag's
            // rate is never paid.
            skip_mode_flag: false,
            interpolation_filter: self.interpolation_filter,
            is_motion_mode_switchable: self.is_motion_mode_switchable,
            force_integer_mv: self.force_integer_mv,
            allow_warped_motion: self.allow_warped_motion,
            enable_dual_filter: self.enable_dual_filter,
            enable_masked_compound: self.enable_masked_compound,
            enable_jnt_comp: self.enable_jnt_comp,
            enable_interintra_compound: self.enable_interintra_compound,
            enable_order_hint: self.order_hint.enable_order_hint,
            order_hint_bits: self.order_hint.order_hint_bits,
            cur_order_hint: self.order_hint.cur_order_hint,
            ref_order_hint: &self.order_hint.ref_order_hint,
            gm_wmtype: &self.gm_wmtype,
        }
    }
}

/// Convert the IntraBC-side MV cost tables to the `port_md` shape.
///
/// The two types differ ONLY in how they clip a component index — see
/// [`MvCostTable`]'s own doc — and both are built by C's single
/// `svt_av1_build_nmv_cost_table` (md_rate_estimation.c:446). Building one
/// from the other means there is one transcription of that function, not two.
#[must_use]
pub fn nmv_cost_table(
    nmvc: &crate::entropy::mv_coding::NmvContext,
    precision: crate::entropy::mv_coding::MvSubpelPrecision,
) -> MvCostTable {
    let t = crate::intrabc::build_nmv_cost_table(nmvc, precision);
    let comp = |i: usize| -> Vec<i32> {
        (0..MV_VALS)
            .map(|v| t.comp_cost[i].cost(v as i32 - crate::intrabc::MV_MAX))
            .collect()
    };
    MvCostTable {
        joint: t.joint_cost,
        comp: [comp(0), comp(1)],
    }
}

/// Build [`InterFacBits`] + [`RefFrameFacBits`] from the live CDFs.
#[must_use]
pub fn build_inter_rates(
    fc: &crate::entropy::context::FrameContext,
    ic: &InterCdfs,
) -> (InterFacBits, RefFrameFacBits) {
    (
        InterFacBits::from_cdfs(fc, ic),
        RefFrameFacBits::from_cdfs(fc, ic),
    )
}

/// One block's INTER candidate, or `None` when the block has no ME result.
pub struct InterCandOut {
    /// The mode-decision payload the funnel carries on its `Cand`.
    pub mode: PredictionMode,
    pub ref_frame: [i8; 2],
    pub mv: [Mv; 2],
    pub pred_mv: [Mv; 2],
    pub drl_index: u8,
    pub interp_filters: u32,
    pub motion_mode: MotionMode,
    /// The motion-compensated prediction, luma then the two chroma planes.
    pub y_pred: Vec<u8>,
    pub u_pred: Vec<u8>,
    pub v_pred: Vec<u8>,
    /// C `cand_bf->fast_luma_rate`.
    pub fast_luma_rate: u32,
    /// C `cand->block_mi.num_proj_ref` — the warped-motion SAMPLE COUNT, which
    /// the WRITER needs because it decides the motion-mode ALPHABET
    /// (`docs/INTER-ENCODE-PLAN.md` §1z¹⁸). It is carried even though this
    /// port never selects warped motion: the symbol is written by every inter
    /// block, whatever the search does.
    pub num_proj_ref: u8,
}

/// The per-block inputs the caller has and this module does not.
pub struct InterBlockCtx<'a> {
    /// Frame-absolute luma origin and dims.
    pub org_x: usize,
    pub org_y: usize,
    pub bw: usize,
    pub bh: usize,
    /// C `BlockSize` index.
    pub bsize: u8,
    /// The MD mode-info grid, already carrying this block's own partition in
    /// its first cell (C's MVP scan runs against the live mi state).
    pub grid: &'a [MvpMiEntry],
    pub grid_stride: i32,
    /// C `xd->above_mbmi` / `left_mbmi` and their availability flags.
    pub neighbors: Neighbors,
    /// C `blk_ptr->overlappable_neighbors`.
    pub overlappable_neighbors: u32,
    /// C `ctx->is_inter_ctx` (`svt_av1_get_intra_inter_context`).
    pub is_inter_ctx: usize,
    /// Whether the block has a chroma pair; when false the chroma prediction
    /// is not produced.
    pub has_uv: bool,
    /// C `ctx->sq_sb_me_mv` + `pc_tree->tested_blk[PART_N][0]` — see
    /// [`crate::inter_search_arm::SqMeState`]. It is the CALLER's state
    /// because C's is: one slot on the mode-decision context, written by
    /// every square block's own search and read by the NSQ shapes that
    /// follow it at the same node.
    pub sq_me: Option<&'a mut crate::inter_search_arm::SqMeState>,
}

/// C `BlockModeInfo::mode` as a `PredictionMode`.
///
/// The mode-info grid stores C's raw `u8` (that is what
/// `svt_aom_update_mi_map` writes) and `InjectCtx` wants the enum, so the
/// mapping lives here rather than as a new public constructor on the shared
/// types crate. `None` is a value outside AV1's 25 modes, which is a caller
/// bug rather than a neighbour state.
fn mode_from_u8(v: u8) -> Option<PredictionMode> {
    use PredictionMode as M;
    Some(match v {
        0 => M::DcPred,
        1 => M::VPred,
        2 => M::HPred,
        3 => M::D45Pred,
        4 => M::D135Pred,
        5 => M::D113Pred,
        6 => M::D157Pred,
        7 => M::D203Pred,
        8 => M::D67Pred,
        9 => M::SmoothPred,
        10 => M::SmoothVPred,
        11 => M::SmoothHPred,
        12 => M::PaethPred,
        13 => M::NearestMv,
        14 => M::NearMv,
        15 => M::GlobalMv,
        16 => M::NewMv,
        17 => M::NearestNearestMv,
        18 => M::NearNearMv,
        19 => M::NearestNewMv,
        20 => M::NewNearestMv,
        21 => M::NearNewMv,
        22 => M::NewNearMv,
        23 => M::GlobalGlobalMv,
        24 => M::NewNewMv,
        _ => return None,
    })
}

/// Build this block's INTER candidate set, exactly as C composes it.
///
/// `port_md::inject::inject_inter_candidates` (C `mode_decision.c:2836`)
/// decides WHICH candidates exist; this fills its `InjectCtx` and turns each
/// one into a motion-compensated prediction plus C's real
/// `svt_aom_inter_fast_cost`. The returned order is the injector's, which is
/// load-bearing — each stage sees the injected-MV log the previous ones
/// filled, so `NEARESTMV` at the same MV suppresses the `NEWMV` duplicate.
#[must_use]
pub fn build_inter_candidates(
    f: &InterMdFrame<'_>,
    b: &mut InterBlockCtx<'_>,
    lambda: u64,
    fast_lambda: u32,
) -> Vec<InterCandOut> {
    use crate::port_md::inject::{
        CandArray, InjectCtx, NoRefinement, WmCtrls, inject_inter_candidates,
    };
    use crate::port_md::predicates::{InjectedMvLog, MeCandidateRef, RefPruningState};

    // --- The reference-MV stack, PER REFERENCE TYPE. C calls
    //     `svt_aom_generate_av1_mvp_table(ctx, ..., ctx->ref_frame_type_arr,
    //     ctx->tot_ref_frame_types, pcs)` (product_coding_loop.c:9393), i.e.
    //     one stack per entry — not one for LAST.
    let ctx = derive_block_ctx(
        (b.org_y / 4) as i32,
        (b.org_x / 4) as i32,
        b.bsize as usize,
        f.mi_rows,
        f.mi_cols,
        f.tile,
        f.sb_mi_size,
    );
    let grid = MvpGrid {
        entries: b.grid,
        stride: b.grid_stride,
        base: (b.org_y / 4) as i32 * b.grid_stride + (b.org_x / 4) as i32,
    };
    // C `svt_aom_generate_av1_mvp_table`'s `gm_mv` for an IDENTITY global
    // motion model is the zero MV; this port signals no global motion.
    let mut stacks = alloc::vec![crate::inter_mvp::InterMvpStack::default(); 8];
    let mut ref_mv_count = [0u8; 8];
    for &rt in f.ref_frame_type_arr {
        let i = rt.max(0) as usize;
        stacks[i] = setup_ref_mv_list(&grid, &ctx, &f.mvp_env, rt, [Mv::ZERO; 2]);
        ref_mv_count[i] = stacks[i].count;
    }

    // --- C's per-block MD motion searches, in C's own order:
    //     `build_single_ref_mvp_array` -> `read_refine_me_mvs` ->
    //     `pme_search` (product_coding_loop.c:9425-9447). See
    //     [`crate::inter_search_arm`] for why the reference set and PME are
    //     one mechanism.
    let search = crate::inter_search_arm::run_block_searches(
        &f.search,
        &crate::inter_search_arm::BlockSearchIn {
            // C `ctx->full_lambda_md[0]` / `fast_lambda_md[0]` as
            // `svt_aom_mode_decision_configure_sb` set them for THIS
            // superblock. `lambda` is the funnel's own per-SB MD lambda,
            // which is the SAME quantity the search used to re-derive at
            // frame level -- one value, one derivation.
            full_lambda_8bit: u32::try_from(lambda).unwrap_or(u32::MAX),
            fast_lambda_8bit: fast_lambda,
            org_x: b.org_x,
            org_y: b.org_y,
            bw: b.bw,
            bh: b.bh,
            bsize: b.bsize,
            // C `blk_geom->sq_size` — the SQUARE this shape came from. The
            // funnel has no NSQ parent link here, so a square block's own
            // size is used; for an NSQ shape that is the larger side, which
            // is what `svt_init_mv_cost_params`' `early_exit_th` reads.
            sq_size: b.bw.max(b.bh) as u16,
            mi_rows: f.mi_rows,
            mi_cols: f.mi_cols,
            src: f.src,
            src_stride: f.src_stride,
            ref_frame_type_arr: f.ref_frame_type_arr,
            padded_by_ref: &f.padded_by_ref,
            stacks: &stacks,
            ref_mv_count: &ref_mv_count,
            nmv: &f.nmv,
            drl_mode_fac_bits: &f.fac.drl_mode,
            search_tables: &f.search_tables,
            me: f.me,
            // C `ctx->sq_sb_me_mv` + `pc_tree->tested_blk[PART_N][0]`, which
            // live ACROSS blocks. `None` here means the caller has no
            // square-parent state, which makes every shape take C's
            // `me_mv_array` seed — the behaviour this module had before the
            // state existed. The funnel supplies it.
            sq_me: b.sq_me.as_deref().copied(),
        },
    );
    // C `if (ctx->shape == PART_N) ctx->sq_sb_me_mv = ctx->sb_me_mv`
    // (product_coding_loop.c:2932-2934), and the `tested_blk[PART_N][0]` that
    // guards its reader. The write is HERE and not in `inter_search_arm`
    // because the state is the caller's — C's is one slot on the
    // mode-decision context, and the funnel is what owns the block walk that
    // gives it its meaning.
    if search.is_square_shape
        && let Some(q) = b.sq_me.as_deref_mut()
    {
        q.record_square(b.org_x, b.org_y, b.bw, search.sb_me_mv);
    }

    // --- C's ME candidate array for this block, verbatim: the injectors
    //     read each candidate's own `direction` and resolve it to a
    //     reference frame (`mode_decision.c:2320-2326`).
    let me_cands: Vec<MeCandidateRef> =
        f.me.cands_for(b.org_x, b.org_y, b.bsize)
            .iter()
            .map(|c| MeCandidateRef {
                direction: c.direction(),
                ref_idx_l0: c.ref_idx_l0(),
                ref_idx_l1: c.ref_idx_l1(),
                ref0_list: c.ref0_list(),
                ref1_list: c.ref1_list(),
            })
            .collect();
    let me_totals = [me_cands.len() as u8];
    let sb_me_mv = search.sb_me_mv;

    let gm = [svtav1_types::motion::WarpedMotionParams::default(); 8];
    let ref_pruning = RefPruningState::default();
    // C `svt_aom_init_wm_samples` (adaptive_mv_pred.c:1752) -> the injector's
    // `num_proj_ref`. This was `[0u8; 8]`, and a zero here is not a
    // conservative default: `motion_mode_allowed` promotes a block to
    // WARPED_CAUSAL — and with it the THREE-symbol MOTION_MODES alphabet
    // instead of the two-symbol OBMC one — exactly when this count is >= 1
    // and the frame allows warped motion. The DECODER runs the same scan, so
    // a wrong count is an arithmetic-coder DESYNC, not a quality choice:
    // `docs/INTER-ENCODE-PLAN.md` §1z¹⁸ measured `aomdec` REJECTING 22 of the
    // campaign's 96 cells for this, every one at the preset where
    // `allow_warped_motion` is 1.
    //
    // C's three-part gate is reproduced exactly; the `else` arm zeroes every
    // entry, which is what the old constant happened to be right about on the
    // frames where the gate is false.
    let mut wm_sample_num = [0u8; 8];
    if f.allow_warped_motion
        && crate::port_entropy_inter::modes::is_motion_variation_allowed_bsize(
            svtav1_types::block::BlockSize::from_u8(b.bsize)
                .expect("an injected inter block must have a real BlockSize"),
        )
        && b.overlappable_neighbors != 0
    {
        for &rt in f.ref_frame_type_arr {
            let rf = crate::inter_mvp::av1_set_ref_frame(rt);
            if rf[1] != NONE_FRAME {
                continue;
            }
            let (n, _pts, _pts_inref) = crate::inter_mvp::find_warp_samples(&grid, &ctx, rf[0]);
            wm_sample_num[rf[0].max(0) as usize] = n;
        }
    }
    let inj = InjectCtx {
        bsize: b.bsize,
        bwidth: b.bw as u16,
        bheight: b.bh as u16,
        blk_org_x: b.org_x as u32,
        blk_org_y: b.org_y as u32,
        shape_is_part_n: true,
        // NOT C's value (`reference_select` is 1 on these frames, so C's
        // `reference_mode_is_single` is 0). This is the bipred suppression
        // the module header names: `inter_pred_arm` has no two-reference
        // path, and `allow_bipred` is the single switch that keeps every
        // compound injector — MVP-ii, ME's BI_PRED entry and PME's — from
        // producing a candidate this port could not predict.
        reference_mode_is_single: true,
        allow_high_precision_mv: f.allow_high_precision_mv,
        is_motion_mode_switchable: f.is_motion_mode_switchable,
        force_integer_mv: u8::from(f.force_integer_mv),
        // The port writes `skip_mode_present = 0` on every frame it emits.
        skip_mode_flag: false,
        skip_mode_ref_frame_idx_0: -1,
        skip_mode_ref_frame_idx_1: -1,
        is_lossless_segment: false,
        ref_frame_type_arr: f.ref_frame_type_arr,
        global_motion: &gm,
        // C `gm_ctrls.skip_identity`: with it set and every model IDENTITY,
        // `inject_global_candidates` `continue`s — which is why no GLOBALMV
        // candidate appears even though the injector runs.
        gm_skip_identity: true,
        wm_sample_num: &wm_sample_num,
        ref_mv_stack: &stacks,
        ref_mv_count: &ref_mv_count,
        nmv_cost: &f.nmv,
        drl_mode_fac_bits: &f.fac.drl_mode,
        shut_fast_rate: false,
        approx_inter_rate: 0,
        total_me_cnt: me_cands.len(),
        me_cands: &me_cands,
        me_totals: &me_totals,
        me_block_offset: 0,
        sb_me_mv: &sb_me_mv,
        post_subpel_me_mv_cost: &search.post_subpel_me_mv_cost,
        valid_pme_mv: &search.valid_pme_mv,
        best_pme_mv: &search.best_pme_mv,
        ref_pruning: &ref_pruning,
        // C `ctx->corrupted_mv_check`: the `is_valid_mv_diff` guard. On with
        // a real cost table, which is what this module supplies.
        corrupted_mv_check: true,
        redundant_cand_ctrls: Default::default(),
        // Every one of these OFF controls is an unported search, named in
        // this module's header. They are not a smaller candidate set chosen
        // here — they are the inputs that make C's own injector produce the
        // smaller set, and the assertion below refuses anything they should
        // have suppressed.
        inter_comp_ctrls: Default::default(),
        inter_intra_comp_ctrls: Default::default(),
        wm_ctrls: WmCtrls::default(),
        obmc_ctrls: Default::default(),
        near_count_ctrls: Default::default(),
        bipred3x3_ctrls: Default::default(),
        unipred3x3_injection: 0,
        new_nearest_injection: true,
        new_nearest_near_comb_injection: 0,
        inject_new_me: true,
        global_mv_injection: true,
        inject_new_pme: true,
        updated_enable_pme: f.search.updated_enable_pme,
        reduce_unipred_candidates: 0,
        use_neighbouring_mode_ctrls_enabled: false,
        is_intra_bordered: false,
        has_overlappable_candidates: b.overlappable_neighbors != 0,
        allow_warped_motion: f.allow_warped_motion,
        left_available: b.neighbors.left_available,
        up_available: b.neighbors.up_available,
        left_mi: b
            .neighbors
            .left_avail()
            .and_then(|m| mode_from_u8(m.mode).map(|md| (md, m.ref_frame))),
        above_mi: b
            .neighbors
            .above_avail()
            .and_then(|m| mode_from_u8(m.mode).map(|md| (md, m.ref_frame))),
    };

    let mut cands = CandArray::new(64);
    let mut log = InjectedMvLog::default();
    inject_inter_candidates(&inj, &mut cands, &mut log, &mut NoRefinement);

    let mut out = Vec::new();
    for c in cands.as_slice() {
        assert!(
            c.motion_mode == crate::port_md::predicates::MotionMode::SimpleTranslation
                && !c.is_interintra_used
                && c.ref_frame[1] == NONE_FRAME,
            "the inter candidate set produced a candidate this port cannot PREDICT \
             (motion_mode {:?}, interintra {}, ref_frame {:?}). Its control was supposed \
             to be off — see `inter_md_arm`'s header. Refusing rather than dropping it, \
             because a silently dropped candidate is a mode decision nobody made.",
            c.motion_mode,
            c.is_interintra_used,
            c.ref_frame,
        );
        // C `blk_ptr->inter_mode_ctx[ref_frame_type]` — the mode context of
        // the candidate's OWN reference, not LAST's.
        let imc = stacks[c.ref_frame[0].max(0) as usize].mode_context;
        out.push(predict_and_price(f, b, c, imc, &stacks, lambda));
    }
    out
}

/// One injected candidate -> its prediction and C's MDS0 rate.
fn predict_and_price(
    f: &InterMdFrame<'_>,
    b: &InterBlockCtx<'_>,
    c: &crate::port_md::inject::InterCandidate,
    inter_mode_ctx: i16,
    stacks: &[crate::inter_mvp::InterMvpStack],
    lambda: u64,
) -> InterCandOut {
    // --- The motion-compensated prediction. C does luma and both chroma
    //     planes in ONE `av1_inter_prediction_light_pd1` call under a
    //     component mask, so this is one call (see `inter_pred_arm`).
    // C `block_mi.interp_filters` — this port runs no interpolation-filter
    // search, so every candidate is EIGHTTAP_REGULAR in both directions,
    // which is the packed value 0. `InterCandidate` (the injector's) carries
    // no filter field for the same reason C's injectors never set one: the
    // filter is decided later, by the IFS search this port does not have.
    let interp_filters = 0u32;
    // The two crates carry their own `MotionMode` (the injector's lives in
    // `port_md::predicates`, the writer's and the rate's in
    // `port_entropy_inter::modes`); the discriminants are C's, so this is a
    // re-spelling. The assertion at the call site has already established
    // that only SimpleTranslation reaches here.
    let mm = match c.motion_mode {
        crate::port_md::predicates::MotionMode::SimpleTranslation => MotionMode::SimpleTranslation,
        crate::port_md::predicates::MotionMode::ObmcCausal => MotionMode::ObmcCausal,
        crate::port_md::predicates::MotionMode::WarpedCausal => MotionMode::WarpedCausal,
    };
    let mut y_pred = alloc::vec![0u8; b.bw * b.bh];
    let (cw, chh) = (b.bw / 2, b.bh / 2);
    let (mut u_pred, mut v_pred) = if b.has_uv {
        (alloc::vec![0u8; cw * chh], alloc::vec![0u8; cw * chh])
    } else {
        (Vec::new(), Vec::new())
    };
    // C `svt_aom_get_ref_pic_buffer(pcs, rf[0])` — the candidate's OWN
    // reference picture. A missing entry is a caller bug: the injector can
    // only produce a reference that was in `ref_frame_type_arr`, and the
    // pipeline fills the table for every entry it puts there.
    let padded = f.padded_by_ref[c.ref_frame[0].max(0) as usize].unwrap_or_else(|| {
        panic!(
            "an inter candidate names reference {} with no DPB picture — \
             `ref_frame_type_arr` and `padded_by_ref` disagree",
            c.ref_frame[0]
        )
    });
    match (b.has_uv, padded.uv.as_ref()) {
        (true, Some((refu, refv))) => crate::inter_pred_arm::predict_inter_yuv(
            (&padded.y, refu, refv),
            b.org_x,
            b.org_y,
            b.bw,
            b.bh,
            c.mv[0],
            interp_filters,
            f.sb_size,
            f.frame_w,
            f.frame_h,
            &mut y_pred,
            b.bw,
            &mut u_pred,
            &mut v_pred,
            cw,
        ),
        _ => crate::inter_pred_arm::predict_inter_luma(
            &padded.y,
            b.org_x,
            b.org_y,
            b.bw,
            b.bh,
            c.mv[0],
            interp_filters,
            f.sb_size,
            f.frame_w,
            f.frame_h,
            &mut y_pred,
            b.bw,
        ),
    }

    // --- C's real MDS0 rate, `svt_aom_inter_fast_cost` (rd_cost.c:1005).
    //
    // `ref_frame_rate` carries its own two-field `NeighborMi` (only
    // `ref_frame` + `use_intrabc` are read there); this is a projection, not
    // a second neighbour derivation.
    let rr = |n: Option<NeighborMi>| {
        n.map(|m| crate::port_md::ref_frame_rate::NeighborMi {
            ref_frame: m.ref_frame,
            use_intrabc: m.use_intrabc,
        })
    };
    let (rr_above, rr_left) = (
        rr(b.neighbors.above_avail().copied()),
        rr(b.neighbors.left_avail().copied()),
    );
    let counts = NeighborRefCounts::collect(rr_above, rr_left);
    let ref_bits = crate::port_md::ref_frame_rate::estimate_ref_frames_num_bits(
        &[c.ref_frame[0]],
        &counts,
        rr_above,
        rr_left,
        f.reference_mode_is_select,
        b.bw as u16,
        b.bh as u16,
        &f.ref_fac,
        |rf| [rf, NONE_FRAME],
    );
    let ref_frames_num_bits = ref_bits.first().map_or(0, |&(_, bits)| bits);

    let bsize = svtav1_types::block::BlockSize::from_u8(b.bsize)
        .expect("an injected inter block must have a real BlockSize");
    let stack = &stacks[c.ref_frame[0].max(0) as usize];
    let cost = inter_fast_cost(
        &f.cost_frame(),
        &InterBlock {
            bsize,
            // The port writes `skip_mode_present = 0`, so the skip-mode
            // context is never read; 0 is C's own initial value.
            skip_mode_ctx: 0,
            is_inter_ctx: b.is_inter_ctx,
            inter_mode_ctx,
            ref_mv_count: stack.count,
            ref_mv_stack: &stack.stack,
            ref_frames_num_bits,
            neighbors: &b.neighbors,
            overlappable_neighbors: b.overlappable_neighbors,
            approx_inter_rate: 0,
            // C prices the interpolation filter at MDS0 only when
            // `ctx->ifs_ctrls.level == IFS_MDS0` (rd_cost.c:1179).
            //
            // This was hard-coded TRUE on the reasoning that "this port runs
            // no filter search, so the filter IS known and is priced". The
            // reasoning is about a DIFFERENT gap: C's level here is
            // `IFS_MDS1` or `IFS_MDS3` (`interpolation_search_level` is 2 at
            // MR and 4 above it, never 1), so C does not price the filter at
            // MDS0 either — it prices it after the search it runs and this
            // port does not. Paying it early is not "pricing what C prices
            // later"; it is a DIFFERENT MDS0 ordering. MEASURED 2026-09-02
            // against C's `svt_aom_inter_fast_cost` (`SVT_IFCOST_OUT`) on
            // `uniform 72x72 q20 p8`: 20 to 109 rate units on every inter
            // candidate, on top of the 1207 the inverted `is_inter_ctx`
            // cost. That the port never prices the filter AT ALL is the
            // still-open IFS gap, named in this module's header.
            ifs_at_mds0: f.search.ifs_at_mds0,
        },
        &InterCandidate {
            mode: c.mode,
            ref_frame: c.ref_frame,
            mv: c.mv,
            pred_mv: c.pred_mv,
            drl_index: c.drl_index,
            interp_filters,
            motion_mode: mm,
            num_proj_ref: u16::from(c.num_proj_ref),
            is_interintra_used: false,
            interintra_mode: 0,
            use_wedge_interintra: false,
            interintra_wedge_index: 0,
            comp_group_idx: 0,
            compound_idx: 1,
            interinter_comp_type: svtav1_types::prediction::CompoundType::Average,
            interinter_wedge_index: 0,
            skip_mode_allowed: false,
        },
        lambda,
        0,
        Some(&f.nmv),
        &f.fac,
    );

    // The FIELD JOIN against C's `SVT_CINTER_OUT` line, which carries exactly
    // these inputs (`imc=`, `drl=`, `mv0=`, `pmv0=`, `ovl=`, `rf=`) plus the
    // decision C made with them. It exists because the funnel's `NSQDBG CAND`
    // line reports only the FINISHED rate: on `uniform 72x72 q20 p8` frame 1
    // the port priced the 8x8 corner block's NEARESTMV at `flr = 3014` and
    // chose intra where C codes inter, and nothing in the repo could say
    // which of the six inputs to `svt_aom_inter_fast_cost` differed. A total
    // is one number; C's dump has six fields, so print six.
    //
    // Gated on SVTAV1_CANDDBG + SVTAV1_NSQDBG like every other funnel dump.
    #[cfg(feature = "std")]
    if crate::dbgenv::canddbg() && crate::depth_refine::nsqdbg_here(b.org_x, b.org_y) {
        std::eprintln!(
            "NSQDBG ICAND mi=({},{}) {}x{} mode={} rf={},{} mv0={},{} pmv0={},{} drl={} imc={} \
             ovl={} isinterctx={} nb=[{},{}] refmvcnt={} refbits={} flr={}",
            b.org_y / 4,
            b.org_x / 4,
            b.bw,
            b.bh,
            c.mode as u8,
            c.ref_frame[0],
            c.ref_frame[1],
            c.mv[0].y,
            c.mv[0].x,
            c.pred_mv[0].y,
            c.pred_mv[0].x,
            c.drl_index,
            inter_mode_ctx,
            b.overlappable_neighbors,
            b.is_inter_ctx,
            // The two neighbours' `ref_frame[0]`, which is what
            // `svt_av1_get_intra_inter_context` reads: `-9` for "not
            // available". Without them `isinterctx` is a verdict with no
            // premises, and the premise is the MD mi grid.
            b.neighbors.above_avail().map_or(-9, |m| m.ref_frame[0]),
            b.neighbors.left_avail().map_or(-9, |m| m.ref_frame[0]),
            stack.count,
            ref_frames_num_bits,
            cost.rate.luma,
        );
    }

    InterCandOut {
        mode: c.mode,
        ref_frame: c.ref_frame,
        mv: c.mv,
        pred_mv: c.pred_mv,
        drl_index: c.drl_index,
        interp_filters,
        motion_mode: mm,
        y_pred,
        u_pred,
        v_pred,
        fast_luma_rate: cost.rate.luma,
        num_proj_ref: c.num_proj_ref,
    }
}

/// The neighbour pair the inter contexts read, from the MD mode-info grid.
///
/// C reads `xd->above_mbmi` / `left_mbmi` — the mi cell ABOVE the block's
/// top-left and the one to its LEFT — and keeps the availability flags
/// separate from the pointers (`port_entropy_inter::Neighbors`).
#[must_use]
pub fn neighbors_from_grid(
    grid: &[MvpMiEntry],
    stride: i32,
    mi_row: i32,
    mi_col: i32,
    tile: TileMiBounds,
) -> Neighbors {
    let at = |r: i32, c: i32| -> NeighborMi {
        let e = grid[(r * stride + c) as usize];
        NeighborMi {
            mode: e.mode,
            ref_frame: e.ref_frame,
            interp_filters: e.interp_filters,
            use_intrabc: e.use_intrabc,
            skip_mode: false,
            comp_group_idx: 0,
            compound_idx: 0,
            bsize: e.bsize,
        }
    };
    let up = mi_row > tile.mi_row_start;
    let left = mi_col > tile.mi_col_start;
    Neighbors {
        above: up.then(|| at(mi_row - 1, mi_col)),
        left: left.then(|| at(mi_row, mi_col - 1)),
        up_available: up,
        left_available: left,
    }
}
