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
//! * `inter_comp_ctrls` / bipred — this module's `ref_frame_type_arr` carries
//!   ONE entry, so no compound candidate can be built. The PREDICTION is not
//!   the obstacle: `svtav1_dsp::port_pd_pred::av1_inter_prediction_light_pd1`
//!   takes an `mvs` SLICE and runs the `jnt_convolve` compound path whenever
//!   it has two (`port_pd_pred.rs:240`); it is
//!   [`crate::inter_pred_arm`]'s ADAPTER that narrows it to one MV, on
//!   purpose, because no candidate here is compound. Widening the reference
//!   set therefore has to widen the adapter with it. **This used to say "no second reference EXISTS in the port's
//!   low-delay-P reference set", and that was MEASURED FALSE on 2026-09-02:**
//!   C reports `ref_frame_type_arr = [LAST, BWDREF, LAST_BWD]`,
//!   `reference_select = 1`, and it CODES `rf=5` (BWDREF) on the 128-wide
//!   cells (`SVT_INJCFG_OUT` / `SVT_CINTER_OUT`). The second reference is
//!   real; this module does not model it yet, and
//!   `docs/INTER-ENCODE-PLAN.md` §1z¹⁴ says why the fix is atomic with PME.
//! * `wm_ctrls` (warped motion) and `obmc_ctrls` — the DSP is ported
//!   (`svtav1_dsp::obmc`, the warp family) and the PREDICTION drivers are
//!   not wired, so a warped or OBMC candidate could not be predicted. The
//!   injector would produce them; the ctrls are off and the module ASSERTS
//!   none arrive rather than dropping them silently.
//! * `inter_intra_comp_ctrls` — same shape.
//! * `unipred3x3_injection`, `bipred3x3_ctrls`, `inject_new_pme` — the 3x3
//!   refinement and the predictive-ME search are unported.
//! * `near_count_ctrls` — C caps the NEAR DRL loop to ZERO unless this
//!   control is enabled (it REPLACES `max_drl_index`, it does not refine
//!   it), so `NEARMV` is absent exactly the way C makes it absent.
//! * The ME MV handed to `sb_me_mv` is the OPEN-LOOP one; C's
//!   `read_refine_me_mvs` sub-pel refinement is unported, so this is the
//!   value C's refinement STARTS from.
//!
//! What that leaves live is `NEARESTMV` and `NEWMV` off `LAST_FRAME` — where
//! C, on the same cells, has three reference types and a PME candidate per
//! type (§1z¹⁴) — with
//! C's own injection ORDER (MVP before NEW) and C's own
//! `mv_is_already_injected` dedup — which is what makes the port pick
//! `NEARESTMV` alone on flat content, as C does.

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
    pub padded: &'a PaddedRef,
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
    b: &InterBlockCtx<'_>,
    lambda: u64,
) -> Vec<InterCandOut> {
    use crate::port_md::inject::{
        CandArray, InjectCtx, NoRefinement, WmCtrls, inject_inter_candidates,
    };
    use crate::port_md::predicates::{InjectedMvLog, MeCandidateRef, RefPruningState};

    // --- The reference-MV stack, per reference type. Only LAST_FRAME is
    //     populated: the port's low-delay-P reference set has one entry, and
    //     `ref_frame_type_arr` below says so.
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
    let last_stack = setup_ref_mv_list(&grid, &ctx, &f.mvp_env, LAST_FRAME, [Mv::ZERO; 2]);
    let mut stacks = alloc::vec![crate::inter_mvp::InterMvpStack::default(); 8];
    let mut ref_mv_count = [0u8; 8];
    stacks[LAST_FRAME as usize] = last_stack;
    ref_mv_count[LAST_FRAME as usize] = stacks[LAST_FRAME as usize].count;
    let inter_mode_ctx = stacks[LAST_FRAME as usize].mode_context;

    // --- The open-loop ME MV, as C stores it: full-pel `* 8`
    //     (mode_decision.c:2323-2325). `sb_me_mv` is C's REFINED value; the
    //     refinement (`read_refine_me_mvs`) is unported, so this is the value
    //     it starts from.
    let mut sb_me_mv = [[Mv::ZERO; 4]; 2];
    let mut me_cands: Vec<MeCandidateRef> = Vec::new();
    //
    //     C indexes `me_mv_array` by the ME CANDIDATE's own `direction`
    //     (`mode_decision.c:2320-2326`), and on a flat low-delay-P GOP that
    //     candidate is usually LIST 1's — see
    //     [`crate::inter_me_arm::FrameMe::cand_mv_for`] for the measurement.
    //     The DIRECTION is deliberately NOT propagated: `ref_frame_type_arr`
    //     below carries `LAST_FRAME` alone, so a direction-1 candidate would
    //     resolve to `BWDREF_FRAME` and be dropped by the injector. This port
    //     models one reference and takes that candidate's MV against it; the
    //     second reference is a separate chunk.
    if let Some((_dir, mv_fp)) = f.me.cand_mv_for(b.org_x, b.org_y, b.bsize, 0) {
        sb_me_mv[0][0] = Mv {
            x: mv_fp.x.saturating_mul(8),
            y: mv_fp.y.saturating_mul(8),
        };
        me_cands.push(MeCandidateRef {
            direction: 0,
            ref_idx_l0: 0,
            ref_idx_l1: 0,
            ref0_list: 0,
            ref1_list: 0,
        });
    }
    let me_totals = [me_cands.len() as u8];

    let gm = [svtav1_types::motion::WarpedMotionParams::default(); 8];
    let ref_pruning = RefPruningState::default();
    let ref_frame_type_arr = [LAST_FRAME];
    let wm_sample_num = [0u8; 8];
    let inj = InjectCtx {
        bsize: b.bsize,
        bwidth: b.bw as u16,
        bheight: b.bh as u16,
        blk_org_x: b.org_x as u32,
        blk_org_y: b.org_y as u32,
        shape_is_part_n: true,
        reference_mode_is_single: !f.reference_mode_is_select,
        allow_high_precision_mv: f.allow_high_precision_mv,
        is_motion_mode_switchable: f.is_motion_mode_switchable,
        force_integer_mv: u8::from(f.force_integer_mv),
        // The port writes `skip_mode_present = 0` on every frame it emits.
        skip_mode_flag: false,
        skip_mode_ref_frame_idx_0: -1,
        skip_mode_ref_frame_idx_1: -1,
        is_lossless_segment: false,
        ref_frame_type_arr: &ref_frame_type_arr,
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
        post_subpel_me_mv_cost: &[[0u32; 4]; 2],
        valid_pme_mv: &[[false; 4]; 2],
        best_pme_mv: &[[Mv::ZERO; 4]; 2],
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
        inject_new_pme: false,
        updated_enable_pme: false,
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
        out.push(predict_and_price(f, b, c, inter_mode_ctx, &stacks, lambda));
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
    match (b.has_uv, f.padded.uv.as_ref()) {
        (true, Some((refu, refv))) => crate::inter_pred_arm::predict_inter_yuv(
            (&f.padded.y, refu, refv),
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
            &f.padded.y,
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
            // C prices the interpolation filter at MDS0 only when the IFS
            // level says the filter is already decided there; this port runs
            // no filter search, so the filter IS known and is priced.
            ifs_at_mds0: true,
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
