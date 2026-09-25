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
//! * `unipred3x3_injection`, `bipred3x3_ctrls` — the 3x3 refinements are
//!   unported. `inject_new_pme` / `updated_enable_pme` are ON:
//!   [`crate::inter_search_arm`] runs C's `build_single_ref_mvp_array` ->
//!   `read_refine_me_mvs` -> `pme_search` chain per reference.
//! * `md_nsq_motion_search` — not called in THIS module, and **that no longer
//!   means an NSQ block takes the square path**. [`crate::inter_search_arm`]
//!   builds its MVC list (`nsq_sub_block_mvs`) and passes it into
//!   `refine_me_mv_for_ref` under `b_w_ne_h && md_nsq_me_enabled`, and it
//!   seeds from the square parent's `sq_sb_me_mv` through the `SqMeState`
//!   this module threads. The search RUNS.
//!
//!   The entry here used to say the opposite and to quote **94 of the 259
//!   coded inter blocks on the then-55 F1DIFF cells**
//!   (`tools/inter_cinter_census.sh`, 2026-09-02) as its reach. That census
//!   predates the wiring and §1z²⁶; it is kept for provenance and is a
//!   measurement of a state that no longer holds.
//!
//!   MEASURED 2026-09-03 on `diag 72x72 q55 p6`, the cell that reading would
//!   have been used to explain: C's own `SVT_SUBPEL_OUT` at the one block
//!   that still diverges (`org=(64,32)`, a `BLOCK_16X32`) reports
//!   `start=(32,8) best=(32,8)` — **the port's ME MV exactly** — with
//!   `nsqme=1` confirmed from C's `SVT_INJCFG_OUT`. C does not code it
//!   because NEARMV's COST wins, not because its search found something
//!   else. The residual is a cost comparison, and the instrument for it is
//!   `SVT_FULLCOST_OUT`, not the ME:
//!   `benchmarks/f1diff_q55_localization_2026-09-03.md`.
//!
//!   What IS still unported here: this port keeps ONE `SqMeState` slot, not
//!   a node chain, so C's `BLOCK_4X4`-off-`parent->tested_blk` seed arm
//!   (`product_coding_loop.c:2860`) has no counterpart. Unreachable at the
//!   presets measured (`shapes_for_size` returns `N_ONLY` at size 4) and
//!   unported, not proven inert.
//!
//! The compound/inter-intra entries that USED to head this list have
//! landed: `ref_frame_type_arr` carries C's compound entries (`LAST_BWD`,
//! `LAST_LAST2`), `reference_mode_is_select` reads the real picture
//! decision, and `inj_comp_modes` injects C's DIST/DIFF0/WEDGE variants
//! with `calc_pred_masked_compound` + `search_compound_diff_wedge` behind
//! [`WarpHooks`]. [`crate::inter_pred_arm::predict_inter_yuv_compound_md`]
//! (and the `_hbd` twin) are C's full `av1_inter_prediction` compound arm
//! — per-reference warp, `dist_wtd_comp_weight_assign` offsets on ref 1's
//! convolve, and the masked blend through the private CONV_BUF — and
//! `inter_intra_search` plus the `IiPreds` precompute reproduce
//! `precompute_intra_pred_for_inter_intra` + `inter_intra_prediction` on
//! every path that predicts (injection, IFS, MDS3, winner rebuild).
//!
//! `near_count_ctrls` WAS on that list, and its entry was WRONG. It read "C
//! caps the NEAR DRL loop to ZERO unless this control is enabled (it REPLACES
//! `max_drl_index`, it does not refine it), so `NEARMV` is absent exactly the
//! way C makes it absent" — a correct reading of C's `enabled == 0` arm
//! (`mode_decision.c:1377-1381`) and a wrong conclusion, because `enabled` is
//! **1 in all seven arms** of `set_cand_reduction_ctrls`
//! (`enc_mode_config.c:4113/4138/4163/4193/4224/4255/4290`) and the video
//! arm's `pcs->cand_reduction_level` is 0, 1 or 2 (`:9039-9050`) — every one
//! of which carries `near_count = 3`. So C injects up to three `NEARMV`
//! candidates per single reference on every frame this port can encode, and
//! this module injected none. MEASURED on `diag 72x72 q40 p6` frame 1: at
//! `mi=(8,16)` C's `SVT_IFCOST_OUT` carries `mode=14` at
//! `fast_luma_rate = 2845` and CODES it, while this module's best was `NEWMV`
//! at 4187 with the SAME MV `(24,0)`. The control is derived now
//! ([`InterMdFrame::cand_reduction`]); full record
//! `benchmarks/inter_near_candidate_2026-09-03.md`.
//!
//! What that leaves live is `NEARESTMV`, `NEARMV` and `NEWMV` off
//! `LAST_FRAME` and `BWDREF_FRAME`, plus one PME `NEWMV` per reference — with
//! C's own injection ORDER (MVP before NEW before PME) and C's own
//! `mv_is_already_injected` dedup.

use crate::inter_me_arm::FrameMe;
use crate::inter_mvp::InterMvpEnv;
use crate::inter_mvp::NONE_FRAME;
use crate::intrabc::TileMiBounds;
use crate::intrabc_mvp::{MvpGrid, MvpMiEntry, derive_block_ctx};
use crate::picture::PaddedRef;
use crate::port_entropy_inter::modes::{MotionMode, TransformationType};
use crate::port_entropy_inter::{InterCdfs, NeighborMi, Neighbors};
use crate::port_md::pme::MvCostTable;
use crate::port_md::ref_frame_rate::{NeighborRefCounts, RefFrameFacBits};
use crate::port_rd_cost::inter_cost::{
    InterBlock, InterCandidate, InterFacBits, InterFrame, inter_fast_cost,
};
use alloc::vec::Vec;
use svtav1_types::motion::Mv;
use svtav1_types::prediction::PredictionMode;

pub use svtav1_types::reference::LAST_FRAME;

/// The frame-level tables and pictures the inter branch of MD reads.
///
/// Built once per inter frame, shared by every leaf.
/// What `interpolation_filter_search` (enc_inter_prediction.c:2058) reads
/// from the picture besides the sequence/frame-header fields
/// [`InterMdFrame`] already carries. Built once per frame in `pipeline.rs`.
#[derive(Debug, Clone, Copy)]
pub struct IfsFrameKnobs {
    /// `scs->vq_ctrls.sharpness_ctrls.ifs && pcs->ppcs->is_noise_level`
    /// (`:2166`). The first term is `tune::sharpness_ifs`; the second is
    /// `PicParams::is_noise_level` (always 0 on low delay — C's `last_i`
    /// carry never updates there). The pipeline refuses a frame where the
    /// pair would be 1, so admitted frames always read 0 — same as C.
    pub smooth_bias: bool,
    /// `scs->static_config.tx_bias > 0` (`:2173`).
    pub tx_bias: bool,
    /// `pcs->ppcs->picture_qp`, the index into `ifs_smooth_bias`.
    pub picture_qp: u8,
    /// `get_effective_ac_bias(ac_bias, slice_type == I_SLICE,
    /// temporal_layer_index)` as `model_rd_for_sb` (`:1990`) evaluates it
    /// for THIS picture — an inter frame on layer 0 (the port's video mode
    /// is `hier_levels 0`), so `ac_bias * 0.6`, not the `* 0.3` I-slice arm
    /// `FunnelCfg::ac_bias_eff` carries.
    pub ac_bias_eff: f64,
}

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
    /// C `pcs->wm_level` — the warped-motion ladder, derived by the wired
    /// picture-level `sig_deriv_mode_decision_config_default` and, until
    /// 2026-09-10, thrown away here.
    pub wm_level: u8,
    /// The sequence bit depth. 8 on every u8 encode; 10 is what makes
    /// [`InterCandOut::y_pred10`] reachable, and the highbd convolve reads it
    /// as C's `bd` (the rounding and the clamp both depend on it).
    pub bit_depth: u8,
    /// C `ppcs->global_motion[ref].wmtype`.
    pub gm_wmtype: [TransformationType; 8],
    /// C `pcs->ppcs->global_motion[TOTAL_REFS_PER_FRAME]` — the models
    /// `svt_aom_global_motion_estimation` fitted, as
    /// `set_global_motion_field` published them. The injector prices GLOBALMV
    /// against these and `gm_mv_candidates_for` projects them per block.
    pub global_motion: [svtav1_types::motion::WarpedMotionParams; 8],
    /// C `pcs->ppcs->gm_ctrls.skip_identity` (`set_gm_controls`, level 4 only).
    pub gm_skip_identity: bool,
    /// C `pcs->ppcs->gm_ctrls.enabled` — the value
    /// `svt_aom_sig_deriv_enc_dec_*` assigns to `ctx->global_mv_injection`
    /// (enc_mode_config.c:7847/:7964). 0 on every level-0 preset (enc_mode
    /// > M4), where `svt_aom_inject_inter_candidates` skips
    /// `inject_global_candidates` entirely.
    pub gm_enabled: bool,
    /// C `pcs->inter_compound_mode` (`get_inter_compound_level`,
    /// enc_mode_config.c:8757) — the level `set_inter_comp_controls`
    /// expands into the injector's `inter_comp_ctrls`. 0 ("AVG only") at
    /// M3+, which still permits the NEAREST_NEAREST/NEAR_NEAR/NEW_NEW
    /// compound candidates `inject_*` builds under `allow_bipred`.
    pub inter_compound_mode: u8,
    /// C `pcs->ppcs->pic_obmc_level` (`svt_aom_get_obmc_level`,
    /// enc_mode_config.c:8815) — the ladder `set_obmc_controls` expands.
    ///
    /// It was `Default::default()` (disabled) until 2026-09-11 on the strength
    /// of a census that only ran at presets 6 and 8. C codes OBMC on 22.5 % of
    /// every coded inter block at preset 0; see
    /// `benchmarks/obmc_census_2026-09-10.meta`.
    pub pic_obmc_level: u8,
    /// The frame-level knobs of the MDS3 interpolation-filter search
    /// (`leaf_funnel::ifs`) that are not already header fields above.
    pub ifs: IfsFrameKnobs,
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
    /// C `av1_lambda_assign_md`'s LAMBDA_MOD_INTRA arm (md_process.c:730-745)
    /// — 138 when `stats_based_sb_lambda_modulation && temporal_layer_index
    /// > 0 && ref_intra_percentage < (alt ? 65 : 50)`, else the 128
    /// identity. Frame-level; applied inside `update_lambda`'s output before
    /// `lambda_weight`, for both `full_lambda_md` and `fast_lambda_md`.
    pub lambda_mod_intra: i64,
    /// C `frm_hdr->skip_mode_params.skip_mode_flag`
    /// (`pd_process.c:4958` = `skip_mode_allowed`), the frame bit the MDS0
    /// rate reads.
    pub skip_mode_flag: bool,
    /// C `frm_hdr->skip_mode_params.ref_frame_idx_{0,1}` — the pair the
    /// injector's NEAREST_NEARESTMV arm compares a candidate's refs against
    /// (mode_decision.c:1590-1594). `INVALID_IDX` (-1) when the frame does
    /// not allow skip mode.
    pub skip_mode_ref_frame_idx_0: i8,
    pub skip_mode_ref_frame_idx_1: i8,
    /// C `ctx->cand_reduction_ctrls`, as
    /// `svt_aom_sig_deriv_enc_dec_default` sets it from
    /// `pcs->cand_reduction_level` (`enc_mode_config.c:7826`).
    ///
    /// The injector reads four of its fields; see the module header for what
    /// each one does here and which are inert on this envelope.
    pub cand_reduction: crate::port_enc_mode_config::encdec::CandReductionCtrls,
    /// C `pcs->inter_intra_level` (`svt_aom_get_inter_intra_level`,
    /// enc_mode_config.c:8819) — the level `set_inter_intra_ctrls`
    /// (enc_mode_config.c:5385) expands into the injector's
    /// `inter_intra_comp_ctrls`. 0 = off; 2 is the only level the shipped
    /// presets reach (wedge search on NSQ shapes only, no RD model).
    pub inter_intra_level: u8,
    /// C `pcs->hbd_md` as the candidate injector sees it — the SEARCH
    /// depth of the inter-intra and masked-compound hooks
    /// (`inter_intra_search`, mode_decision.c:333;
    /// `calc_pred_masked_compound`, enc_inter_prediction.c:3538, which
    /// remaps EB_DUAL_BIT_MD to 8-bit). On the shipped ladder
    /// `enable_hbd_mode_decision` is `DEFAULT`, which maps to
    /// `EB_DUAL_BIT_MD` on I-slices and `EB_8_BIT_MD` on inter frames
    /// (enc_handle.c:4518) — so this is 0 everywhere the inter arm runs.
    /// The field is carried anyway: the hook bodies read it the way C
    /// reads `ctx->hbd_md`, so the day a level arms 10-bit MD the wiring
    /// is already the transcription.
    pub hbd_md: u8,
    /// The `smooth_interintra_mask` tables — `inter_intra_prediction`'s
    /// per-(bsize, mode) blend weights (`init_ii_masks`,
    /// inter_prediction.c:2294). Frame-owned: C keeps them on the MD
    /// context; the port builds them once per inter frame here.
    pub ii_masks: svtav1_dsp::port_interintra::IiMasks,
    /// The `wedge_params` master tables (`av1_init_wedge_masks`,
    /// reconinter.c) — read by the inter-intra wedge search, the
    /// masked-compound wedge pick, and the predict-time WEDGE blend.
    pub wedge_masks: svtav1_dsp::port_wedge_masks::WedgeMasks,
    /// C `scs->mrp_ctrls.use_best_references` — the level
    /// `get_enable_use_best_me` (product_coding_loop.c:9310-9341) reads to
    /// decide whether this block's `ref_frame_type_arr` is rebuilt by
    /// `determine_best_references` from its own ME candidate array.
    pub use_best_references: u8,
    /// C `pcs->temporal_layer_index` — the `> 0` gate of
    /// `get_enable_use_best_me`.
    pub temporal_layer_index: u8,
    /// C `pcs->ppcs->ref_list{0,1}_count_try != 0` — the backfill gates of
    /// `determine_best_references` (product_coding_loop.c:104-114).
    pub ref_list0_count_try: bool,
    pub ref_list1_count_try: bool,
    /// C `pcs->ppcs->sframe_ref_pruned` — when set, C skips
    /// `determine_best_references` entirely (product_coding_loop.c:9379).
    pub sframe_ref_pruned: bool,
    /// C `pcs->ppcs->max_can_count` — `svt_aom_get_max_can_count`
    /// (enc_mode_config.c:1921). `INC_MD_CAND_CNT` caps
    /// `fast_cand_array` at this and C allocates the array to exactly
    /// this size (md_process.c:386), so the injection port must take the
    /// same value: a smaller cap saturates mid-injection, leaves
    /// `inj_comp_modes` reading a stale last candidate, and silently
    /// drops candidates C evaluates. MEASURED at preset 0: a hardcoded
    /// 64 overflowed once GM armed (1225 is C's value there) and the
    /// stale last cand was a single-ref NEWMV.
    pub max_can_count: u16,
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
            // C `frm_hdr->skip_mode_params.skip_mode_flag`
            // (`pd_process.c:4958` = `skip_mode_allowed`). It gates the
            // skip-mode RATE `inter_fast_cost` adds to every candidate of a
            // compound-capable block, so a constant false under-priced every
            // block of a frame that signals the bit.
            skip_mode_flag: self.skip_mode_flag,
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

/// C `svt_av1_build_nmv_cost_table` (md_rate_estimation.c:446) for the MD
/// arm — the one transcription, [`crate::intrabc::build_nmv_cost_table`].
/// (`port_md`'s table type is that same type since 2026-09-04; this used to
/// re-pack it into a second shape that differed only at an unreachable
/// clip.)
#[must_use]
pub fn nmv_cost_table(
    nmvc: &crate::entropy::mv_coding::NmvContext,
    precision: crate::entropy::mv_coding::MvSubpelPrecision,
) -> MvCostTable {
    crate::intrabc::build_nmv_cost_table(nmvc, precision)
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
    /// THE SAME PREDICTION AT TRUE 10 BITS — C's `bd > EB_EIGHT_BIT` arm of
    /// `svt_inter_predictor_light_pd1`, run against the 10-bit reference the
    /// DPB carries ([`crate::picture::PaddedRef::hbd`]).
    ///
    /// EMPTY unless this frame's reference has a 10-bit twin. The bd10 full-RD
    /// funnel residuals every candidate against `Cand::pred10`; an inter
    /// candidate had no producer for it at all, so the funnel indexed an empty
    /// slice and panicked, which is what kept 10-bit VIDEO unreachable while
    /// 10-bit stills worked.
    pub y_pred10: Vec<u16>,
    pub u_pred10: Vec<u16>,
    pub v_pred10: Vec<u16>,
    /// C `cand->wm_params_l0` — the local-warp affine model when
    /// `motion_mode == WarpedCausal`, reference 0's GLOBAL model for a
    /// GLOBALMV / GLOBAL_GLOBALMV candidate; the reconstruction and the
    /// writer both need it, and the model IS the motion on the warp path
    /// (the MV is unused by the warp kernel).
    pub wm_params_l0: svtav1_types::motion::WarpedMotionParams,
    /// C `cand->wm_params_l1` — reference 1's model for a compound
    /// candidate. `av1_inter_prediction` warps EACH reference by its own
    /// model, so a GLOBAL_GLOBALMV rebuild needs both.
    pub wm_params_l1: svtav1_types::motion::WarpedMotionParams,
    /// C `cand_bf->fast_luma_rate`.
    pub fast_luma_rate: u32,
    /// The rate C's `*cand_bf->fast_cost` was charged at — the skip-mode
    /// rate when `skip_mode_rate < luma_rate` fired in `finish`
    /// (rd_cost.c:997-1003), `fast_luma_rate` otherwise. C's `fast_loop_core`
    /// assigns the returned RDCOST wholesale; this port's funnel owns the
    /// distortion and re-forms the cost as `rdcost(lambda, rate, satd)`, so
    /// without this the skip-mode candidate is priced at its full mode rate
    /// and dies in the MDS0 replacement pool (gradient 16x16 q40 p6 poc4:
    /// C charged 1536 → fcost 3230888, the port charged 4025 → 4420902).
    pub fast_cost_rate: u32,
    /// C `cand->block_mi.num_proj_ref` — the warped-motion SAMPLE COUNT, which
    /// the WRITER needs because it decides the motion-mode ALPHABET
    /// (`docs/INTER-ENCODE-PLAN.md` §1z¹⁸). It is carried even though this
    /// port never selects warped motion: the symbol is written by every inter
    /// block, whatever the search does.
    pub num_proj_ref: u8,
    /// C `cand->cand_class` for the inter classes — 2 for `NEWMV`/
    /// `NEW_NEWMV` and for EVERY inter candidate when `merge_inter_cands`
    /// fired on the block, 1 for the other "MVP Prediction" modes
    /// (mode_decision.c:3662-3669). Stamped by `build_inter_candidates`
    /// after `predict_and_price`; the NIC lane reads it unchanged.
    pub cand_class: u8,
    /// C `cand->block_mi.comp_group_idx` / `compound_idx` /
    /// `interinter_comp.type` — CODED symbols `determine_compound_mode` set
    /// at injection. All zero (`COMPOUND_AVERAGE` group A) on a
    /// single-reference candidate, which never reaches the coder.
    pub comp_group_idx: u8,
    pub compound_idx: u8,
    pub interinter_comp_type: u8,
    /// C `block_mi.interinter_comp.mask_type` / `wedge_index` /
    /// `wedge_sign` — the mask `search_compound_diff_wedge` selected for a
    /// `COMPOUND_DIFFWTD` / `COMPOUND_WEDGE` candidate. The rebuild and the
    /// writer both need them: the DIFFWTD mask is re-derived from the two
    /// convolution buffers at predict time while `mask_type` is coded, and
    /// the WEDGE pair IS the coded syntax.
    pub interinter_mask_type: u8,
    pub interinter_wedge_index: i8,
    pub interinter_wedge_sign: bool,
    /// C `block_mi.is_interintra_used` / `interintra_mode` /
    /// `use_wedge_interintra` / `interintra_wedge_index` — the
    /// `inter_intra_search` result for an inter-intra candidate. `ref_frame[1]
    /// == INTRA_FRAME` is the same fact (`inj_non_simple_modes` stamps both);
    /// the fields are what the intra-predictor blend and the writer read.
    pub is_interintra_used: bool,
    pub interintra_mode: u8,
    pub use_wedge_interintra: bool,
    pub interintra_wedge_index: i8,
    /// C `cand->skip_mode_allowed` — the injector's NEAREST_NEARESTMV arm
    /// sets it when the candidate's ref pair IS the frame's skip-mode pair
    /// (mode_decision.c:1590-1595); `svt_aom_full_cost` arbitrates the
    /// skip-mode symbol cost against the normal mode cost at every stage.
    pub skip_mode_allowed: bool,
    /// C `ctx->skip_mode_ctx` — `av1_get_skip_mode_context(xd)`, a per-BLOCK
    /// value carried on each candidate so the funnel stages can price the
    /// skip-mode symbol without re-deriving the neighbour pair.
    pub skip_mode_ctx: u8,
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
    /// C `ctx->shape == PART_N` — the pick between
    /// `inter_intra_comp_ctrls.wedge_mode_sq` and `wedge_mode_nsq`
    /// (mode_decision.c:468). The funnel's `ibc_gate.is_part_n`.
    pub is_part_n: bool,
    /// The block-local 10-bit luma source (`bw * bh` u16s, stride `bw`)
    /// — `LeafBd10::blk_y_src10`. The `hbd_md` inter-intra and masked-
    /// compound searches score against it. Empty when the leaf's bd10
    /// plumbing is off — which is every shipped inter config today,
    /// since `hbd_md` is 0 there.
    pub y_src10: &'a [u16],
    /// The block's precomputed inter-intra intra predictions — C's
    /// `ctx->intrapred_buf` (`precompute_intra_pred_for_inter_intra`,
    /// enc_intra_prediction.c:657). `None` when the block is not
    /// inter-intra eligible; the `inter_intra_search` hook refuses in
    /// that case rather than blending an empty table. A borrow: the
    /// buffers live on `FunnelCtx::ii_preds` so the IFS/MDS3 rebuild
    /// reaches them after injection returns (C's `ctx->intrapred_buf`
    /// likewise outlives `generate_md_stage_0_cand`).
    pub ii: Option<&'a IiPreds>,
    /// C `full_lambda_md[EB_8_BIT_MD]` for this superblock — the lambda
    /// `inter_intra_search`'s model-RD arm and the wedge/mask searches'
    /// `use_rate` arms price with at the 8-bit (or DUAL-remapped) search
    /// depth.
    pub full_lambda8: u32,
    /// C `full_lambda_md[EB_10_BIT_MD]` — read only when the search depth
    /// is 10 bits (`hbd_md != 0` for inter-intra, `hbd_md == 1` for the
    /// masked-compound prep).
    pub full_lambda10: u32,
    /// C `dequants->y_dequant_qtx[base_q_idx][1]` — the AC dequant
    /// `model_rd_*` divides by inside the `use_rate`/`use_rd_model`
    /// arms. Inert at every level the shipped ladder reaches (both
    /// flags are 0 below level 1) but plumbed faithfully.
    pub quantizer: i16,
}

/// The inter-intra intra predictions for one block — the port's
/// `ctx->intrapred_buf[INTERINTRA_MODES]` (`precompute_intra_pred_for_
/// inter_intra`, enc_intra_prediction.c:657).
///
/// C precomputes LUMA only and derives chroma inside
/// `inter_intra_prediction` on every call (enc_inter_prediction.c:2217,
/// the `use_precomputed_intra && !plane` gate). The port computes all
/// three planes up front: the recon neighbours the predictors read
/// cannot change between this block's injection and its arbitration, so
/// the values are the same — and the blend tail lives in
/// [`predict_and_price`], which cannot reach the funnel's `predict_unit`.
///
/// Each plane buffer is `INTERINTRA_MODES` (4) contiguous `w * h`
/// blocks — `buf[mode * w * h ..]`; the chroma planes use the block's
/// `get_plane_block_size(bsize, 1, 1)` dims.
pub struct IiPreds {
    /// `intrapred_buf[j]` luma, 8-bit MD domain — `4 * bw * bh`.
    pub luma: Vec<u8>,
    /// The chroma intra predictions, `4 * cw * chh` each — empty when the
    /// block has no chroma.
    pub u: Vec<u8>,
    pub v: Vec<u8>,
    /// The 10-bit twins, populated only when the leaf's bd10 plumbing is
    /// on (C's `use_precomputed_intra` 16-bit arm). Empty otherwise.
    pub luma16: Vec<u16>,
    pub u16: Vec<u16>,
    pub v16: Vec<u16>,
}

/// C `BlockModeInfo::mode` as a `PredictionMode`.
///
/// The mode-info grid stores C's raw `u8` (that is what
/// `svt_aom_update_mi_map` writes) and `InjectCtx` wants the enum, so the
/// mapping lives here rather than as a new public constructor on the shared
/// types crate. `None` is a value outside AV1's 25 modes, which is a caller
/// bug rather than a neighbour state.
pub(crate) fn mode_from_u8(v: u8) -> Option<PredictionMode> {
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

mod warp;
pub use warp::*;
mod prelude;
pub use prelude::*;
mod chroma_obmc;
pub(crate) use chroma_obmc::*;

mod price;
pub use price::*;
