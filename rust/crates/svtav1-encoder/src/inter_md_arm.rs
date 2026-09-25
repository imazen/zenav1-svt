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

/// C `LAST_FRAME`.
pub const LAST_FRAME: i8 = 1;

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

/// Build this block's INTER candidate set, exactly as C composes it.
///
/// One reference's precomputed local-warp neighbour samples — C's
/// `ctx->wm_sample_info[ref]`, filled once per block by
/// `svt_aom_init_wm_samples` and read by every candidate's warp derivation.
#[derive(Clone, Copy)]
pub struct WarpSamples {
    pub n: u8,
    pub pts: [[i32; 2]; crate::inter_mvp::LEAST_SQUARES_SAMPLES_MAX],
    pub pts_inref: [[i32; 2]; crate::inter_mvp::LEAST_SQUARES_SAMPLES_MAX],
}

impl Default for WarpSamples {
    fn default() -> Self {
        WarpSamples {
            n: 0,
            pts: [[0; 2]; crate::inter_mvp::LEAST_SQUARES_SAMPLES_MAX],
            pts_inref: [[0; 2]; crate::inter_mvp::LEAST_SQUARES_SAMPLES_MAX],
        }
    }
}

/// Everything the MDS1 warp MV refinement needs that is PER BLOCK.
///
/// C keeps all of it on `ModeDecisionContext` and `svt_aom_wm_motion_refinement`
/// reads it there. The port builds the candidate list in this module and runs
/// MDS1 in the leaf funnel, so the block-scoped half has to travel; the
/// frame-scoped half (`nmv`, `fac.drl_mode`, `allow_high_precision_mv`) stays
/// on [`InterMdFrame`] and is read through `fx.inter`.
pub struct WarpRefineBlock {
    /// The per-reference MV stacks `svt_aom_generate_av1_mvp_table` built for
    /// this block.
    ///
    /// The OBMC MV refinement re-picks the DRL index against them
    /// (`svt_aom_choose_best_av1_mv_pred`, mode_decision.c:2272), so it needs
    /// the SAME stack the injector priced the candidate against — rebuilding
    /// it in the funnel would be a second derivation that could disagree.
    pub mvp_stacks: alloc::vec::Vec<crate::inter_mvp::InterMvpStack>,

    /// False when this block can produce no warped candidate at all, which
    /// makes the refinement a no-op without the caller having to know why.
    pub enabled: bool,
    /// C `ctx->wm_ctrls`, the fields the refinement reads.
    pub refinement_iterations: u8,
    pub refine_diag: bool,
    pub shut_approx_if_not_mds0: bool,
    pub lower_band_th: u16,
    pub upper_band_th: u16,
    /// C `svt_aom_set_wm_controls`'s `refine_level`: 1 -> MDS1, 2 -> MDS3.
    pub refine_level: u8,
    /// C `ctx->wm_sample_info[ref]`.
    pub samples: [WarpSamples; 8],
    /// C `ctx->ref_mv_stack[ref]` and `xd->ref_mv_count[ref]`, for the DRL
    /// re-pick the refinement ends with.
    pub stacks: alloc::vec::Vec<crate::inter_mvp::InterMvpStack>,
    pub ref_mv_count: [u8; crate::inter_mvp::MODE_CTX_REF_FRAMES],
    pub mi_row: i32,
    pub mi_col: i32,
    pub bsize: svtav1_types::block::BlockSize,
    pub bwidth: usize,
    pub bheight: usize,
}

impl Default for WarpRefineBlock {
    fn default() -> Self {
        WarpRefineBlock {
            mvp_stacks: alloc::vec::Vec::new(),
            enabled: false,
            refinement_iterations: 0,
            refine_diag: false,
            shut_approx_if_not_mds0: false,
            lower_band_th: 0,
            upper_band_th: 0,
            refine_level: 0,
            samples: Default::default(),
            stacks: alloc::vec::Vec::new(),
            ref_mv_count: [0; crate::inter_mvp::MODE_CTX_REF_FRAMES],
            mi_row: 0,
            mi_col: 0,
            bsize: svtav1_types::block::BlockSize::Block8x8,
            bwidth: 0,
            bheight: 0,
        }
    }
}

impl WarpRefineBlock {
    /// C `svt_aom_warped_motion_parameters` (adaptive_mv_pred.c:1776) for an
    /// arbitrary test MV — the SAME derivation the injector's hook runs, which
    /// is why it lives here rather than being duplicated in the funnel.
    ///
    /// `shut_approx` skips the band gate; C passes
    /// `ctx->wm_ctrls.shut_approx_if_not_mds0` inside the search and a literal
    /// 1 for the final re-derivation, with `lower`/`upper` forced to 0 there
    /// ("this call is not part of a search, so disable the shortcuts").
    #[must_use]
    pub fn warp_params_for(
        &self,
        ref_frame: i8,
        mv: svtav1_types::motion::Mv,
        shut_approx: bool,
        wm: &mut svtav1_types::motion::WarpedMotionParams,
    ) -> Option<u8> {
        use svtav1_dsp::port_warp::{find_projection, select_samples};
        if self.bwidth < 8 || self.bheight < 8 {
            return None;
        }
        let s = &self.samples[ref_frame.max(0) as usize];
        if s.n == 0 {
            return None;
        }
        let mut pts = [0i32; 2 * crate::inter_mvp::LEAST_SQUARES_SAMPLES_MAX];
        let mut pts_inref = [0i32; 2 * crate::inter_mvp::LEAST_SQUARES_SAMPLES_MAX];
        for i in 0..usize::from(s.n) {
            pts[2 * i] = s.pts[i][0];
            pts[2 * i + 1] = s.pts[i][1];
            pts_inref[2 * i] = s.pts_inref[i][0];
            pts_inref[2 * i + 1] = s.pts_inref[i][1];
        }
        let mut nsamples = s.n;
        if nsamples > 1 {
            nsamples = select_samples(
                mv,
                &mut pts,
                &mut pts_inref,
                usize::from(nsamples),
                self.bsize,
            );
        }
        let mut apply = !find_projection(
            usize::from(nsamples),
            &pts,
            &pts_inref,
            self.bsize,
            mv,
            wm,
            self.mi_row,
            self.mi_col,
        );
        if apply && !shut_approx {
            let (a, b) = (i32::from(wm.alpha).abs(), i32::from(wm.beta).abs());
            let (g, d) = (i32::from(wm.gamma).abs(), i32::from(wm.delta).abs());
            let lo = i32::from(self.lower_band_th);
            let hi = i32::from(self.upper_band_th);
            if a + b < lo && g + d < lo {
                apply = false;
            }
            if 4 * a + 7 * b > hi && 4 * g + 4 * d > hi {
                apply = false;
            }
        }
        // C assigns `*num_samples` regardless of the projection's success, so
        // the count is returned even when the model is rejected.
        if apply { Some(nsamples) } else { None }
    }
}

/// `ctx->cmp_store`'s OUTPUT — the masked-compound preparation
/// `calc_pred_masked_compound` fills and `search_compound_diff_wedge`
/// reads (C's `ctx->pred0`/`pred1`/`residual1`/`diff10`).
///
/// C's store is a four-entry MV-keyed cache per list
/// (`port_compound_prep::cmp_store_lookup` is that policy); the
/// predictors are deterministic per (ref, MV), so this port keeps only
/// the live pair's buffers and pays a second uniprediction on a repeated
/// MV rather than holding eight slots of scratch per block. The search
/// output is identical either way.
#[derive(Default)]
struct CmpStore {
    /// The 8-bit domain buffers — used whenever the remap
    /// (`hbd_md == EB_DUAL_BIT_MD ? 8 : hbd_md`, enc_inter_prediction.c:3538)
    /// lands on 8 bits, which is every reachable case.
    pred0_8: Vec<u8>,
    pred1_8: Vec<u8>,
    residual1_8: Vec<i16>,
    diff10_8: Vec<i16>,
    /// The `hbd_md == EB_10_BIT_MD` twins — wired for faithfulness, dead
    /// on the shipped ladder (`hbd_md` is 0 on inter frames).
    pred0_16: Vec<u16>,
    pred1_16: Vec<u16>,
    residual1_16: Vec<i16>,
    diff10_16: Vec<i16>,
    /// The post-remap search depth the preparation ran at; `None` until
    /// `calc_pred_masked_compound` has filled the store.
    hbd: Option<bool>,
}

/// The injector's [`InjectHooks`] with the WARP derivation, the
/// inter-intra search, and the masked-compound preparation/pick wired.
///
/// `blk` is the block-scoped warp-refinement state; `f`/`b` carry the
/// frame and block inputs the three pixel searches read; `ii_ctrls` and
/// `comp_ctrls` are the SAME control objects `InjectCtx` holds (C's
/// `ctx->inter_intra_comp_ctrls` / `inter_comp_ctrls`); `cmp` is the
/// `calc_pred_masked_compound` → `search_compound_diff_wedge` buffer
/// bridge.
struct WarpHooks<'a> {
    blk: &'a WarpRefineBlock,
    f: &'a InterMdFrame<'a>,
    b: &'a InterBlockCtx<'a>,
    ii_ctrls: crate::port_md::inject::InterIntraCompCtrls,
    comp_ctrls: crate::port_md::inject::InterCompCtrls,
    cmp: CmpStore,
}

/// The injector's `MotionMode` (port_md::predicates) as the prediction/
/// rate crate's (`port_entropy_inter::modes`) — same discriminants.
fn map_mm(m: crate::port_md::predicates::MotionMode) -> MotionMode {
    match m {
        crate::port_md::predicates::MotionMode::SimpleTranslation => MotionMode::SimpleTranslation,
        crate::port_md::predicates::MotionMode::ObmcCausal => MotionMode::ObmcCausal,
        crate::port_md::predicates::MotionMode::WarpedCausal => MotionMode::WarpedCausal,
    }
}

/// The single-reference luma prediction C's `svt_aom_inter_prediction`
/// makes for a unipred candidate at the 8-bit domain — the warp leaf when
/// `is_wm`, the plain convolve otherwise, `interp_filters` 0.
///
/// This is what `inter_intra_search` blends against (mode_decision.c:357)
/// and what `calc_pred_masked_compound` fills `pred0`/`pred1` with
/// (:3579/:3630) — the `unipred_override` shape: no compound, no
/// inter-intra, no OBMC.
#[allow(clippy::too_many_arguments)]
fn unipred_luma8(
    p: &crate::picture::PaddedPlane,
    mut wm: svtav1_types::motion::WarpedMotionParams,
    is_wm: bool,
    org_x: usize,
    org_y: usize,
    bw: usize,
    bh: usize,
    mv: Mv,
    sb_size: usize,
    frame_w: usize,
    frame_h: usize,
    dst: &mut [u8],
) {
    if is_wm {
        crate::inter_pred_arm::predict_inter_yuv_warped(
            (p, None),
            &mut wm,
            org_x,
            org_y,
            bw,
            bh,
            mv,
            0,
            sb_size,
            frame_w,
            frame_h,
            dst,
            bw,
            &mut [],
            &mut [],
            0,
        );
    } else {
        crate::inter_pred_arm::predict_inter_luma(
            p, org_x, org_y, bw, bh, mv, 0, sb_size, frame_w, frame_h, dst, bw,
        );
    }
}

/// [`unipred_luma8`] at true 10 bits — `predict_inter_leaf_hbd`'s luma-
/// only form, for the `hbd_md` search domains.
#[allow(clippy::too_many_arguments)]
fn unipred_luma16(
    p: &crate::picture::PaddedPlaneHbd,
    motion_mode: MotionMode,
    is_wm: bool,
    wm: svtav1_types::motion::WarpedMotionParams,
    org_x: usize,
    org_y: usize,
    bw: usize,
    bh: usize,
    mv: Mv,
    sb_size: usize,
    frame_w: usize,
    frame_h: usize,
    bit_depth: u8,
    dst: &mut [u16],
) {
    crate::inter_pred_arm::predict_inter_leaf_hbd(
        p,
        None,
        motion_mode,
        is_wm,
        wm,
        org_x,
        org_y,
        bw,
        bh,
        mv,
        0,
        sb_size,
        frame_w,
        frame_h,
        bit_depth,
        dst,
        bw,
        &mut [],
        &mut [],
        0,
    );
}

impl crate::port_md::inject::InjectHooks for WarpHooks<'_> {
    /// C `inter_intra_search` (mode_decision.c:326-484): the regular
    /// inter prediction blended against each of the four
    /// `intrapred_buf` modes, priced by SSE or the RD model, then the
    /// `ii_wedge_mode` wedge search on the winner. The candidate keeps
    /// the strict-`<` winners; `use_wedge_interintra` follows C's
    /// mode-1/mode-2 rule exactly.
    fn inter_intra_search(&mut self, cand: &mut crate::port_md::inject::InterCandidate) {
        use svtav1_dsp::port_interintra::{
            InterIntraMode, combine_interintra, combine_interintra_highbd,
        };
        use svtav1_dsp::port_masked_compound::{
            highbd_sse, highbd_subtract_block, sse, subtract_block,
        };
        use svtav1_dsp::port_wedge_search::{SearchCtx, pick_wedge_fixed_sign};

        let f = self.f;
        let b = self.b;
        let Some(ii) = b.ii.as_ref() else {
            // `precompute_intra_pred_for_inter_intra` never ran for this
            // block — the funnel's gate and the injector's
            // `is_interintra_allowed` should agree, but refuse rather
            // than blend an empty table.
            debug_assert!(
                false,
                "inter_intra_search without precomputed intrapred_buf"
            );
            return;
        };
        // C reads `ctx->hbd_md` DIRECTLY here (mode_decision.c:333) —
        // no EB_DUAL_BIT_MD remap — so a DUAL block searches at 10 bits.
        let hbd = f.hbd_md != 0;
        let full_lambda = if hbd { b.full_lambda10 } else { b.full_lambda8 };
        let (bw, bh) = (b.bw, b.bh);
        let n = bw * bh;
        let bsize = svtav1_types::block::BlockSize::from_u8(b.bsize)
            .expect("an injected inter block must have a real BlockSize");
        let padded = f.padded_by_ref[cand.ref_frame[0].max(0) as usize].unwrap_or_else(|| {
            panic!(
                "an inter candidate names reference {} with no DPB picture",
                cand.ref_frame[0]
            )
        });
        let mm = map_mm(cand.motion_mode);
        let is_wm = crate::inter_pred_arm::inter_pred_uses_warp(
            mm,
            cand.mode as u8,
            bw,
            bh,
            &cand.wm_params_l0,
        );
        // C `svt_aom_inter_prediction` into `tmp_buf` — the plain
        // single-reference prediction (`interp_filters` 0,
        // `is_interintra_used` 0 on the copy the injector hands us).
        let mut inter8: Vec<u8> = Vec::new();
        let mut inter16: Vec<u16> = Vec::new();
        if hbd {
            inter16 = alloc::vec![0u16; n];
            let h = padded
                .hbd
                .as_ref()
                .expect("hbd_md inter-intra search needs the 10-bit DPB twin");
            unipred_luma16(
                &h.y,
                mm,
                is_wm,
                cand.wm_params_l0,
                b.org_x,
                b.org_y,
                bw,
                bh,
                cand.mv[0],
                f.sb_size,
                f.frame_w,
                f.frame_h,
                f.bit_depth,
                &mut inter16,
            );
        } else {
            inter8 = alloc::vec![0u8; n];
            unipred_luma8(
                &padded.y,
                cand.wm_params_l0,
                is_wm,
                b.org_x,
                b.org_y,
                bw,
                bh,
                cand.mv[0],
                f.sb_size,
                f.frame_w,
                f.frame_h,
                &mut inter8,
            );
        }
        let src8_off = b.org_y * f.src_stride + b.org_x;
        let bsize_group =
            crate::port_entropy_inter::modes::SIZE_GROUP_LOOKUP[bsize as usize] as usize;
        // The four-way mode loop (mode_decision.c:382-461): blend, score,
        // keep the strict-`<` best. `best_interintra_rd` is INT64_MAX
        // init, so the first mode always lands.
        let mut best_rd = i64::MAX;
        let mut best_mode = svtav1_dsp::port_interintra::INTERINTRA_MODES as u8;
        let mut blend: Vec<u8> = Vec::new();
        let mut blend16: Vec<u16> = Vec::new();
        if hbd {
            blend16 = alloc::vec![0u16; n];
        } else {
            blend = alloc::vec![0u8; n];
        }
        for (j, mode) in InterIntraMode::ALL.iter().enumerate() {
            let rmode = i64::from(f.fac.inter_intra_mode[bsize_group][j]);
            if hbd {
                combine_interintra_highbd(
                    &f.ii_masks,
                    &f.wedge_masks,
                    *mode,
                    false,
                    0,
                    0,
                    bsize as usize,
                    bsize as usize,
                    &mut blend16,
                    bw,
                    &inter16,
                    bw,
                    &ii.luma16[j * n..],
                    bw,
                );
            } else {
                combine_interintra(
                    &f.ii_masks,
                    &f.wedge_masks,
                    *mode,
                    false,
                    0,
                    0,
                    bsize as usize,
                    bsize as usize,
                    &mut blend,
                    bw,
                    &inter8,
                    bw,
                    &ii.luma[j * n..],
                    bw,
                );
            }
            let rd: i64 = if self.ii_ctrls.use_rd_model {
                // `src_stride` is consumed by the LIVE arm only — the
                // 10-bit source is `blk_y_src10`, a block-local `bw`-
                // strided canvas (C's `input_frame16bit` crop), while the
                // 8-bit source is the frame-strided `enhanced_pic`.
                let (rate_sum, dist_sum) = svtav1_dsp::port_model_rd::model_rd_for_sb_with_curvfit(
                    bsize,
                    bw,
                    bh,
                    &f.src[src8_off..],
                    if hbd { bw } else { f.src_stride },
                    b.y_src10,
                    &blend,
                    bw,
                    &blend16,
                    0,
                    0,
                    hbd,
                    b.quantizer,
                    full_lambda,
                    None,
                );
                crate::port_rd_cost::rdcost(
                    u64::from(full_lambda),
                    (i64::from(rate_sum) + rmode).max(0) as u64,
                    dist_sum.max(0) as u64,
                ) as i64
            } else if hbd {
                highbd_sse(b.y_src10, bw, &blend16, bw, bw, bh) as i64
            } else {
                sse(&f.src[src8_off..], f.src_stride, &blend, bw, bw, bh) as i64
            };
            if rd < best_rd {
                best_rd = rd;
                best_mode = j as u8;
                cand.interintra_mode = j as u8;
            }
        }
        // The wedge arm (mode_decision.c:463-484): residuals against the
        // SMOOTH-winner's intra pred and the same inter pred, then
        // `pick_wedge_fixed_sign` at sign 0 (`INTERINTRA_WEDGE_SIGN`).
        let ii_wedge_mode = if b.is_part_n {
            self.ii_ctrls.wedge_mode_sq
        } else {
            self.ii_ctrls.wedge_mode_nsq
        };
        let mut best_rd_wedge = i64::MAX;
        if ii_wedge_mode != 0 {
            let mut residual1 = alloc::vec![0i16; n];
            let mut diff10 = alloc::vec![0i16; n];
            if hbd {
                highbd_subtract_block(bh, bw, &mut residual1, bw, b.y_src10, bw, &inter16, bw);
                highbd_subtract_block(
                    bh,
                    bw,
                    &mut diff10,
                    bw,
                    &inter16,
                    bw,
                    &ii.luma16[best_mode as usize * n..],
                    bw,
                );
            } else {
                subtract_block(
                    bh,
                    bw,
                    &mut residual1,
                    bw,
                    &f.src[src8_off..],
                    f.src_stride,
                    &inter8,
                    bw,
                );
                subtract_block(
                    bh,
                    bw,
                    &mut diff10,
                    bw,
                    &inter8,
                    bw,
                    &ii.luma[best_mode as usize * n..],
                    bw,
                );
            }
            let ctx = SearchCtx {
                hbd,
                full_lambda,
                // `pick_wedge_fixed_sign` reads
                // `inter_intra_comp_ctrls.use_rd_model` on this arm — see
                // `SearchCtx::use_rate`'s doc.
                use_rate: self.ii_ctrls.use_rd_model,
                quantizer: b.quantizer,
            };
            let (rd_wedge, wedge_index) = pick_wedge_fixed_sign(
                &f.wedge_masks,
                &ctx,
                bsize,
                &residual1,
                &diff10,
                0,
                &f.fac.wedge_idx[bsize as usize],
            );
            cand.interintra_wedge_index = wedge_index as i8;
            best_rd_wedge = rd_wedge;
        }
        // C's exact rule (mode_decision.c:481-483): mode 1 always wedges;
        // mode 2 wedges only when it beats the smooth winner.
        cand.use_wedge_interintra = ii_wedge_mode == 1 || best_rd_wedge < best_rd;
    }

    /// C `svt_aom_wm_motion_refinement` (mode_decision.c:1873-2011) at
    /// INJECTION time — `inj_non_simple_modes` calls it only for `NEWMV`
    /// clones when `refinement_iterations != 0 && refine_level == 0`
    /// (wm_level 1). The same diamond search the MDS1 lane drives through
    /// [`crate::port_md::mv_refine::wm_motion_refinement`], differing in two
    /// inputs C passes literally at this call site (mode_decision.c:925):
    /// `shut_approx` is 0 (the band gate is live, though level 1's band is
    /// 0/~0) and the rate reference is the candidate's injection-time
    /// `pred_mv`.
    fn wm_motion_refinement(&mut self, cand: &mut crate::port_md::inject::InterCandidate) -> bool {
        let blk = self.blk;
        let f = self.f;
        let b = self.b;
        let rf = cand.ref_frame[0].max(0) as usize;
        let stack = &blk.stacks[rf];
        // C `error_per_bit = full_lambda_md[EB_8_BIT_MD] >> RD_EPB_SHIFT`
        // with the `+= (x == 0)` MAX(1) (mode_decision.c:1884-1886).
        let epb = (b.full_lambda8 >> crate::intrabc::RD_EPB_SHIFT) as i32;
        let refine_ctx = crate::port_md::mv_refine::WmRefineCtx {
            refinement_iterations: blk.refinement_iterations,
            refine_diag: blk.refine_diag,
            allow_high_precision_mv: f.allow_high_precision_mv,
            approx_inter_rate: f.search.approx_inter_rate,
            corrupted_mv_check: true,
            error_per_bit: epb + i32::from(epb == 0),
            drl: crate::port_md::drl::ChooseDrlCtx {
                shut_fast_rate: false,
                approx_inter_rate: f.search.approx_inter_rate,
                ref_mv_stack: &stack.stack,
                ref_mv_count: blk.ref_mv_count[rf],
                nmv_cost: &f.nmv,
                drl_mode_fac_bits: &f.fac.drl_mode,
            },
        };
        let (bw, bh) = (b.bw, b.bh);
        // C `ctx->scratch_prediction_ptr` — luma only; the candidate's own
        // prediction is untouched until a winner exists.
        let mut scratch = crate::vecpool::dirty_pool::<u8>(bw * bh);
        let padded = f.padded_by_ref[rf]
            .unwrap_or_else(|| panic!("warp-refined candidate names ref {rf} with no DPB picture"));
        let src_off = b.org_y * f.src_stride + b.org_x;
        let mut wm = svtav1_types::motion::WarpedMotionParams::default();
        let r = crate::port_md::mv_refine::wm_motion_refinement(
            &refine_ctx,
            cand.mv[0],
            cand.pred_mv[0],
            cand.mode,
            |test_mv| {
                wm = svtav1_types::motion::WarpedMotionParams {
                    wm_type: svtav1_types::motion::TransformationType::Affine,
                    ..Default::default()
                };
                // `shut_approx` is a literal 0 at C's injection call
                // (mode_decision.c:925) — the band gate is live.
                blk.warp_params_for(cand.ref_frame[0], test_mv, false, &mut wm)?;
                unipred_luma8(
                    &padded.y,
                    wm,
                    true,
                    b.org_x,
                    b.org_y,
                    bw,
                    bh,
                    test_mv,
                    f.sb_size,
                    f.frame_w,
                    f.frame_h,
                    &mut scratch,
                );
                // C `fn_ptr->vf(pred, src, &sse)` — variance, not SSE.
                Some(svtav1_dsp::variance::variance_diff(
                    &scratch,
                    bw,
                    &f.src[src_off..],
                    f.src_stride,
                    bw,
                    bh,
                ) as i32)
            },
        );
        cand.mv[0] = r.best_mv;
        cand.drl_index = r.drl_index;
        // C copies back `best_pred_mv[0]` only (mode_decision.c:2000) —
        // `pred_mv[1]` keeps whatever the cloned simple candidate carried.
        cand.pred_mv[0] = r.pred_mv[0];
        r.valid
    }

    /// C `svt_aom_warped_motion_parameters` (adaptive_mv_pred.c:1776).
    ///
    /// `shut_approx` is FALSE at the injection call site (`mode_decision.c:936`
    /// passes a literal 0), so the band gate below is live — and at wm_level 3
    /// `lower_band_th` is `1 << 10`, which is what rejects a warp too weak to
    /// be worth its own motion mode.
    fn warped_motion_parameters(
        &mut self,
        cand: &mut crate::port_md::inject::InterCandidate,
    ) -> bool {
        // C assigns `*num_samples = 0` on every early return, so a rejected
        // candidate leaves the count zeroed rather than stale.
        cand.num_proj_ref = 0;
        match self.blk.warp_params_for(
            cand.ref_frame[0],
            cand.mv[0],
            // `shut_approx` is a literal 0 at C's injection call site
            // (mode_decision.c:936), so the band gate is LIVE here -- at
            // wm_level 3 `lower_band_th` is 1 << 10, which is what rejects a
            // warp too weak to be worth its own motion mode.
            false,
            &mut cand.wm_params_l0,
        ) {
            Some(n) => {
                // The count is `block_mi.num_proj_ref`, which the WRITER reads
                // to pick the motion-mode ALPHABET. Getting it wrong is an
                // arithmetic-coder desync, not a quality choice.
                cand.num_proj_ref = n;
                true
            }
            None => false,
        }
    }

    /// OBMC is not wired. The injector never asks: `obmc_ctrls` is
    /// `Default::default()` (disabled) on this arm.
    ///
    /// THE JUSTIFICATION THAT USED TO BE HERE WAS AN OVER-READ. It said "the
    /// census that motivated the warp wiring found C selecting OBMC on ZERO
    /// blocks at these presets" — true of presets 6 and 8, which is all that
    /// census measured, and read ever since as a fact about OBMC.
    /// `benchmarks/obmc_census_2026-09-10.meta` asked the question at the
    /// presets global motion made reachable: C codes OBMC on **22.5 % of every
    /// coded inter block at preset 0** (987 of 4382 over twelve real-video
    /// cells), 22.2 % at MR and 27.0 % at preset 1, and EXACTLY ZERO at preset
    /// 2 and above. The cutoff is `svt_aom_get_obmc_level`'s level 3 -> 5 step.
    ///
    /// So this is a CORRECTNESS gap at presets -1/0/1, not RD in a wider
    /// envelope. `tools/motion_mode_census.sh` re-measures it.
    fn obmc_motion_refinement(
        &mut self,
        _cand: &mut crate::port_md::inject::InterCandidate,
    ) -> bool {
        true
    }

    /// C `svt_aom_calc_pred_masked_compound`
    /// (enc_inter_prediction.c:3535-3703): the two unipred predictors
    /// for the candidate's ref pair, the `pred0_to_pred1_mult` SAD early
    /// exit, then `residual1 = src - pred1` and `diff10 = pred1 - pred0`
    /// into the store `search_compound_diff_wedge` reads. A `true`
    /// return is C's `exit_compound_prep`, which makes `inj_comp_modes`
    /// inject nothing for the pair.
    fn calc_pred_masked_compound(&mut self, cand: &crate::port_md::inject::InterCandidate) -> bool {
        use svtav1_dsp::port_compound_prep::{
            compound_residuals, compound_residuals_hbd, exit_compound_prep,
        };

        let f = self.f;
        let b = self.b;
        // C :3538 — EB_DUAL_BIT_MD remaps to the 8-bit domain; only
        // EB_10_BIT_MD (1) prepares at true depth. On the shipped ladder
        // `hbd_md` is 0, so the 8-bit arm is the live one.
        let hbd = f.hbd_md == 1;
        self.cmp.hbd = Some(hbd);
        let (bw, bh) = (b.bw, b.bh);
        let n = bw * bh;
        // The unipred override (C :3573/:3624): `mode` is NEWMV, or
        // GLOBALMV when the candidate is GLOBAL_GLOBALMV — which is what
        // makes `inter_pred_uses_warp` fire on the model that candidate
        // carries — and `interp_filters`/`is_interintra_used` forced 0.
        let mode = if cand.mode == PredictionMode::GlobalGlobalMv {
            PredictionMode::GlobalMv
        } else {
            PredictionMode::NewMv
        };
        let mm = map_mm(cand.motion_mode);
        for i in 0..2 {
            let rf = cand.ref_frame[i];
            let padded = f.padded_by_ref[rf.max(0) as usize].unwrap_or_else(|| {
                panic!("a compound candidate names reference {rf} with no DPB picture")
            });
            let wm = if i == 0 {
                cand.wm_params_l0
            } else {
                cand.wm_params_l1
            };
            let is_wm = crate::inter_pred_arm::inter_pred_uses_warp(mm, mode as u8, bw, bh, &wm);
            if hbd {
                let h = padded
                    .hbd
                    .as_ref()
                    .expect("hbd_md masked-compound prep needs the 10-bit DPB twin");
                let dst = if i == 0 {
                    self.cmp.pred0_16 = alloc::vec![0u16; n];
                    &mut self.cmp.pred0_16
                } else {
                    self.cmp.pred1_16 = alloc::vec![0u16; n];
                    &mut self.cmp.pred1_16
                };
                unipred_luma16(
                    &h.y,
                    mm,
                    is_wm,
                    wm,
                    b.org_x,
                    b.org_y,
                    bw,
                    bh,
                    cand.mv[i],
                    f.sb_size,
                    f.frame_w,
                    f.frame_h,
                    f.bit_depth,
                    dst,
                );
            } else {
                let dst = if i == 0 {
                    self.cmp.pred0_8 = alloc::vec![0u8; n];
                    &mut self.cmp.pred0_8
                } else {
                    self.cmp.pred1_8 = alloc::vec![0u8; n];
                    &mut self.cmp.pred1_8
                };
                unipred_luma8(
                    &padded.y, wm, is_wm, b.org_x, b.org_y, bw, bh, cand.mv[i], f.sb_size,
                    f.frame_w, f.frame_h, dst,
                );
            }
        }
        // C :3662-3671 — the per-pixel SAD budget: too-similar predictors
        // have nothing for a mask to separate.
        let dist = if hbd {
            svtav1_dsp::hbd::highbd_sad_kernel(
                &self.cmp.pred0_16,
                bw,
                &self.cmp.pred1_16,
                bw,
                bw,
                bh,
            )
        } else {
            svtav1_dsp::sad::sad(&self.cmp.pred0_8, bw, &self.cmp.pred1_8, bw, bw, bh)
        };
        if exit_compound_prep(dist, bw, bh, u32::from(self.comp_ctrls.pred0_to_pred1_mult)) {
            return true;
        }
        // C :3675/:3696 — `residual1 = src - pred1`, `diff10 = pred1 -
        // pred0`, at the SAME depth the predictors ran at.
        if hbd {
            self.cmp.residual1_16 = alloc::vec![0i16; n];
            self.cmp.diff10_16 = alloc::vec![0i16; n];
            compound_residuals_hbd(
                &mut self.cmp.residual1_16,
                &mut self.cmp.diff10_16,
                b.y_src10,
                bw,
                &self.cmp.pred0_16,
                &self.cmp.pred1_16,
                bw,
                bh,
            );
        } else {
            self.cmp.residual1_8 = alloc::vec![0i16; n];
            self.cmp.diff10_8 = alloc::vec![0i16; n];
            let src8_off = b.org_y * f.src_stride + b.org_x;
            compound_residuals(
                &mut self.cmp.residual1_8,
                &mut self.cmp.diff10_8,
                &f.src[src8_off..],
                f.src_stride,
                &self.cmp.pred0_8,
                &self.cmp.pred1_8,
                bw,
                bh,
            );
        }
        false
    }

    /// C `svt_aom_search_compound_diff_wedge`
    /// (enc_inter_prediction.c:3705-3710) — `pick_interinter_mask` over
    /// the store `calc_pred_masked_compound` just filled, writing the
    /// CODED `wedge_index`/`wedge_sign` (WEDGE) or `mask_type`
    /// (DIFFWTD) on the candidate.
    fn search_compound_diff_wedge(&mut self, cand: &mut crate::port_md::inject::InterCandidate) {
        use svtav1_dsp::port_masked_compound::CompoundType;
        use svtav1_dsp::port_wedge_search::{PickedMask, SearchCtx, pick_interinter_mask};

        let f = self.f;
        let b = self.b;
        let bsize = svtav1_types::block::BlockSize::from_u8(b.bsize)
            .expect("an injected inter block must have a real BlockSize");
        let comp_type = match cand.interinter_comp_type {
            t if t == CompoundType::Wedge as u8 => CompoundType::Wedge,
            t if t == CompoundType::DiffWtd as u8 => CompoundType::DiffWtd,
            _ => return,
        };
        // The pair contract: `determine_compound_mode` reaches this only
        // inside `inj_comp_modes`, after `calc_pred_masked_compound` ran
        // for the same ref pair.
        let Some(hbd) = self.cmp.hbd else {
            debug_assert!(
                false,
                "search_compound_diff_wedge before calc_pred_masked_compound"
            );
            return;
        };
        let ctx = SearchCtx {
            hbd,
            full_lambda: if hbd { b.full_lambda10 } else { b.full_lambda8 },
            use_rate: self.comp_ctrls.use_rate,
            quantizer: b.quantizer,
        };
        let src8_off = b.org_y * f.src_stride + b.org_x;
        let empty8: &[u8] = &[];
        let empty16: &[u16] = &[];
        let (src8, src16, src_stride, p0_8, p1_8, p0_16, p1_16, r1, d10) = if hbd {
            (
                empty8,
                b.y_src10,
                b.bw,
                empty8,
                empty8,
                &self.cmp.pred0_16[..],
                &self.cmp.pred1_16[..],
                &self.cmp.residual1_16[..],
                &self.cmp.diff10_16[..],
            )
        } else {
            (
                &f.src[src8_off..],
                empty16,
                f.src_stride,
                &self.cmp.pred0_8[..],
                &self.cmp.pred1_8[..],
                empty16,
                empty16,
                &self.cmp.residual1_8[..],
                &self.cmp.diff10_8[..],
            )
        };
        match pick_interinter_mask(
            &f.wedge_masks,
            &ctx,
            comp_type,
            bsize,
            src8,
            src16,
            src_stride,
            p0_8,
            p1_8,
            p0_16,
            p1_16,
            r1,
            d10,
        ) {
            Some(PickedMask::Wedge { sign, index }) => {
                cand.interinter_wedge_sign = sign != 0;
                cand.interinter_wedge_index = index as i8;
            }
            Some(PickedMask::Seg(mask_type)) => {
                cand.interinter_mask_type = mask_type as u8;
            }
            // C `assert(0)`s on a non-masked type; `pick_interinter_mask`
            // returns `None` there, which cannot happen on this dispatch.
            None => {}
        }
    }
}

/// The block-setup half of C's `md_product_coding_loop`: the per-reference
/// MVP stacks and the ME/PME searches (`svt_aom_generate_av1_mvp_table` ->
/// `read_refine_me_mvs` -> `pme_search`, product_coding_loop.c:9393-9447).
///
/// C runs this BEFORE `generate_md_stage_0_cand`, so `ctx->md_me_dist` /
/// `md_pme_dist` already exist when `inject_intra_candidates` resolves
/// `dc_cand_only_flag` via `eliminate_candidate_based_on_pme_me_results`
/// (mode_decision.c:3576-3579). The funnel needs the same ordering: this
/// runs before the intra candidate set is fixed, and
/// [`build_inter_candidates`] consumes the result rather than re-searching.
pub struct BlockPrelude<'a> {
    /// C `ctx->ref_mv_stack[MODE_CTX_REF_FRAMES]`.
    pub stacks: Vec<crate::inter_mvp::InterMvpStack>,
    /// C `ctx->ref_mv_count[MODE_CTX_REF_FRAMES]`.
    pub ref_mv_count: [u8; crate::inter_mvp::MODE_CTX_REF_FRAMES],
    /// The search output — `md_me_dist()`/`md_pme_dist()` included.
    pub search: crate::inter_search_arm::BlockSearchOut,
    /// C `ctx->ref_frame_type_arr[0..tot_ref_frame_types]` at INJECTION
    /// time — `determine_best_references`' rebuild of the picture-level
    /// list from this block's own ME candidates when `use_best_me` fired,
    /// else the picture-level list unchanged.
    pub ref_arr: alloc::borrow::Cow<'a, [i8]>,
    /// This block's ME candidates — `determine_best_references` consumed
    /// them in `block_prelude`; `build_inter_candidates` hands the same
    /// slice to the injectors.
    pub me_cands: Vec<crate::port_md::predicates::MeCandidateRef>,
}

/// Build [`BlockPrelude`] for one block (see its doc for C's ordering).
///
/// `light` is `Some((sig, is_intra_bordered))` on the light-PD1 lane — the
/// per-SB `svt_aom_sig_deriv_enc_dec_light_pd1_default` signals and
/// `ctx->is_intra_bordered` already gated on
/// `use_neighbouring_mode_ctrls.enabled` (product_coding_loop.c:9115). It
/// swaps C's `read_refine_me_mvs` + `pme_search` for
/// `read_refine_me_mvs_light_pd1` (:2737) — a strictly smaller search with
/// its own seeding and skip gates.
pub fn block_prelude<'a>(
    f: &InterMdFrame<'a>,
    b: &mut InterBlockCtx<'_>,
    lambda: u64,
    fast_lambda: u32,
    // C `ctx->is_intra_bordered` (product_coding_loop.c:9417) — the
    // `use_neighbouring_mode_ctrls.enabled ? is_intra_bordered(ctx) : 0`
    // product, computed by the caller. The regular lane resolves
    // `ctx->updated_enable_pme` off it (:9418-9422); the light lane
    // carries it inside `light`'s tuple for `run_block_searches_light`.
    is_intra_bordered: bool,
    light: Option<(
        &crate::port_enc_mode_config::light_pd1::LightPd1Signals,
        bool,
    )>,
) -> BlockPrelude<'a> {
    use crate::port_md::predicates::MeCandidateRef;
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
    // C `svt_aom_generate_av1_mvp_table`'s `gm_mv`
    // (adaptive_mv_pred.c:1372-1394): the block-centre projection of THIS
    // reference's global-motion model. It is the zero MV for an IDENTITY
    // model, which is what this used to hardcode; with the search's real
    // models threaded through `InterMdFrame::global_motion` it is the value
    // `setup_ref_mv_list` substitutes for a GLOBALMV neighbour, so a wrong
    // one desyncs the DRL against the decoder's own scan.
    // C's `ctx->ref_mv_stack[MODE_CTX_REF_FRAMES]` — indexed by REFERENCE
    // TYPE, so a compound pair (LAST_LAST2 = 20, LAST_BWD = 8) has its own
    // entry, and `inject_mvp_candidates_ii`'s `ref_mv_stack[ref_pair]` reads
    // the stack that pair's constituents built together.
    let mut stacks = alloc::vec![
        crate::inter_mvp::InterMvpStack::default();
        crate::inter_mvp::MODE_CTX_REF_FRAMES
    ];
    let mut ref_mv_count = [0u8; crate::inter_mvp::MODE_CTX_REF_FRAMES];
    // C's `ctx->sb64_sq_no4xn_geom` is set in the MD block setup, so it is a
    // property of THIS block, not of the picture.
    let mvp_env = f.mvp_env.for_block(f.sb_size, b.bw as usize, b.bh as usize);
    // --- C's ME candidate array for this block, verbatim: the injectors
    //     read each candidate's own `direction` and resolve it to a
    //     reference frame (`mode_decision.c:2320-2326`), and
    //     `determine_best_references` below rebuilds the ref list from it.
    let me_cands: Vec<MeCandidateRef> = f
        .me
        .cands_for(b.org_x, b.org_y, b.bsize)
        .iter()
        .map(|c| MeCandidateRef {
            direction: c.direction(),
            ref_idx_l0: c.ref_idx_l0(),
            ref_idx_l1: c.ref_idx_l1(),
            ref0_list: c.ref0_list(),
            ref1_list: c.ref1_list(),
        })
        .collect();
    // C `determine_best_references` (product_coding_loop.c:65-116), run at
    // the top of `md_encode_block`/`md_encode_block_light_pd1` when the
    // lane's own gate says so — the light lane's is `use_best_references
    // == 3 && temporal_layer_index > 0` (:9074), the regular lane's is
    // `get_enable_use_best_me` (:9379), which levels 1 and 3 also resolve
    // without TPL. In both, `ctx->ref_frame_type_arr` is REBUILT per block
    // from this block's own ME candidate array — a reference ME never
    // searched (`do_ref == 0`) drops out entirely — and that list, not
    // `ppcs`' picture-level one, then drives the MVP table, the searches,
    // and every injector below. `use_best_references == 2` needs
    // `get_sb_tpl_inter_stats` (TPL, unported); `None` folds to false,
    // which is C's own result whenever `tpl_ctrls.enable` is 0.
    let use_best_me = !f.sframe_ref_pruned
        && if light.is_some() {
            f.use_best_references == 3 && f.temporal_layer_index > 0
        } else {
            crate::port_md::coding_loop::get_enable_use_best_me(
                f.use_best_references,
                u32::from(f.temporal_layer_index),
                f.me
                    .per_b64
                    .get((b.org_y / 64) * f.me.b64_cols + (b.org_x / 64))
                    .map_or(0, |o| o.me_8x8_distortion),
            )
            .unwrap_or(false)
        };
    let block_ref_arr: alloc::borrow::Cow<'_, [i8]> = if use_best_me {
        alloc::borrow::Cow::Owned(
            crate::port_md::coding_loop::determine_best_references(
                &me_cands,
                me_cands.len(),
                // `pcs->slice_type == B_SLICE` — this frame is inter (the
                // funnel only builds `InterMdFrame` on non-I slices) and the
                // port's `SliceType` makes every inter frame B.
                true,
                f.ref_list0_count_try,
                f.ref_list1_count_try,
            ),
        )
    } else {
        alloc::borrow::Cow::Borrowed(f.ref_frame_type_arr)
    };
    // C `svt_aom_generate_av1_mvp_table` (product_coding_loop.c:9393 ->
    // adaptive_mv_pred.c:1329; the light lane's `!shut_fast_rate`-guarded
    // call is at :9114): ONE driver over `ref_frame_type_arr`, single AND
    // compound entries, with the `mv_ref0` scratch shared across the loop
    // the way C shares its local — the `symteric_refs` shortcut reads what
    // the LAST pass left in it. `shut_fast_rate` is false on every lane
    // this reaches, so the guard is a transcription, not a fork — it keeps
    // C's skip reachable rather than baking in today's value.
    if light.is_none_or(|(sig, _)| !sig.shut_fast_rate) {
        for (&rt, st) in block_ref_arr
            .iter()
            .zip(crate::inter_mvp::generate_av1_mvp_table(
                &grid,
                &ctx,
                &mvp_env,
                b.bsize as usize,
                &block_ref_arr,
            ))
        {
            ref_mv_count[rt.max(0) as usize] = st.count;
            stacks[rt.max(0) as usize] = st;
        }
    }

    // --- C's per-block MD motion searches, in C's own order. The regular
    //     lane runs `build_single_ref_mvp_array` -> `read_refine_me_mvs` ->
    //     `pme_search` (product_coding_loop.c:9425-9447); the light lane
    //     runs `read_refine_me_mvs_light_pd1` (:2737) instead — no MVP
    //     array, no PME, no ref pruning — which is what
    //     [`crate::inter_search_arm::run_block_searches_light`] ports. See
    //     [`crate::inter_search_arm`] for why the reference set and PME are
    //     one mechanism.
    let search_in = crate::inter_search_arm::BlockSearchIn {
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
        // The BLOCK-level list `determine_best_references` produced (or the
        // picture-level one when its gate is off) — what C's
        // `ctx->ref_frame_type_arr` holds at this point in
        // `md_encode_block`.
        ref_frame_type_arr: &block_ref_arr,
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
        // C `ctx->updated_enable_pme` (product_coding_loop.c:9418-9422):
        // `md_pme_ctrls.enabled`, zeroed when this block's
        // `is_intra_bordered && use_neighbouring_mode_ctrls.enabled`. The
        // caller's `is_intra_bordered` is already the `enabled`-gated
        // product, so the conjunction collapses to `!is_intra_bordered`.
        // Skipping `pme_search` here is what keeps an intra-bordered
        // block's `inject_pme_candidates` from adding candidates C never
        // had — MEASURED on `vidyo1 256x256 p8` frame 1, mi=(10,8),
        // where the port's `sb_pme_mv` (20,6) injected three extra
        // NEWMV/NEWNEWMV candidates C's `updated_enable_pme = 0`
        // suppressed (`SVT_INJCFG_OUT`: `ibord=1 uepme=0`).
        updated_enable_pme: f.search.md_pme_enabled && !is_intra_bordered,
    };
    let search = if let Some((sig, sig_ibord)) = light {
        crate::inter_search_arm::run_block_searches_light(
            &f.search,
            &search_in,
            &crate::inter_search_arm::LightSearchSig {
                md_subpel_me: &sig.md_subpel_me,
                is_intra_bordered: sig_ibord,
                use_neighbouring_mode_enabled: sig.cand_reduction.use_neighbouring_mode_enabled
                    != 0,
                shut_fast_rate: sig.shut_fast_rate,
                approx_inter_rate: sig.approx_inter_rate,
            },
        )
    } else {
        crate::inter_search_arm::run_block_searches(&f.search, &search_in)
    };
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
    BlockPrelude {
        stacks,
        ref_mv_count,
        search,
        ref_arr: block_ref_arr,
        me_cands,
    }
}

/// `port_md::inject::inject_inter_candidates` (C `mode_decision.c:2836`)
/// decides WHICH candidates exist; this fills its `InjectCtx` and turns each
/// one into a motion-compensated prediction plus C's real
/// `svt_aom_inter_fast_cost`. The returned order is the injector's, which is
/// load-bearing — each stage sees the injected-MV log the previous ones
/// filled, so `NEARESTMV` at the same MV suppresses the `NEWMV` duplicate.
///
/// `prelude` is this block's [`block_prelude`] output — C runs the MVP/ME
/// searches at block setup, BEFORE the intra candidate set is decided
/// (`eliminate_candidate_based_on_pme_me_results` reads `md_me_dist`), so the
/// caller owns when they run.
#[must_use]
pub fn build_inter_candidates(
    f: &InterMdFrame<'_>,
    b: &mut InterBlockCtx<'_>,
    lambda: u64,
    // C `nic_pruning_ctrls->merge_inter_cands_mult` — the
    // `generate_md_stage_0_cand_light_pd1` class-merge control
    // (mode_decision.c:3638-3643).
    merge_inter_cands_mult: u8,
    prelude: BlockPrelude<'_>,
    warp_out: &mut WarpRefineBlock,
    // C `ctx->is_intra_bordered` — the
    // `use_neighbouring_mode_ctrls.enabled ? is_intra_bordered(ctx) : 0`
    // product BOTH lanes compute (product_coding_loop.c:9115 / :9451).
    is_intra_bordered: bool,
    // `Some(sig)` on the light-PD1 lane — the per-SB
    // `svt_aom_sig_deriv_enc_dec_light_pd1_default` output. When the per-SB
    // `pd1_level` is above `REGULAR_PD1` the inter candidate set is the
    // STRICT subset `inject_inter_candidates_light_pd1` emits (MVP +
    // ME-NEWMV only — no global, no bipred-3x3, no unipred-3x3, no PME, no
    // non-simple/compound expansion; mode_decision.c:3526-3562), and the
    // cand-reduction controls and `approx_inter_rate` come from the sig,
    // not the picture-level rows.
    light: Option<&crate::port_enc_mode_config::light_pd1::LightPd1Signals>,
) -> Vec<InterCandOut> {
    use crate::port_md::inject::{
        CandArray, InjectCtx, WmCtrls, inject_inter_candidates, inject_inter_candidates_light_pd1,
    };
    use crate::port_md::predicates::InjectedMvLog;

    let BlockPrelude {
        stacks,
        ref_mv_count,
        search,
        ref_arr,
        me_cands,
    } = prelude;
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

    // C `merge_inter_cands` (mode_decision.c:3638-3643), computed once per
    // block in `generate_md_stage_0_cand_light_pd1`: when the best
    // post-subpel ME/PME variance per pixel is under the nic-level
    // threshold, EVERY inter candidate is CAND_CLASS_2 — the MVP and MV
    // lanes share one MDS0 pool instead of competing for separate caps.
    let merge_inter_cands = merge_inter_cands_mult != u8::MAX && {
        // C `uint16_t th = (mult * (63 - scs->static_config.qp)) >> 1`
        // and `(MIN(md_me_dist, md_pme_dist) / (bw * bh)) < th` — C's
        // integer division, with `th` widened for the compare.
        let th = (u32::from(merge_inter_cands_mult) * 63u32.saturating_sub(f.search.cli_qp)) >> 1;
        let me_d = search.md_me_dist();
        let pme_d = search.md_pme_dist();
        let r = me_d.min(pme_d) / ((b.bw * b.bh) as u32) < th;
        #[cfg(feature = "std")]
        if crate::dbgenv::mrgdbg() {
            std::eprintln!(
                "MRGDBG blk=({},{}) {}x{} mult={} th={} me_d={} pme_d={} -> {r}",
                b.org_x,
                b.org_y,
                b.bw,
                b.bh,
                merge_inter_cands_mult,
                th,
                me_d,
                pme_d
            );
        }
        r
    };

    let me_totals = [me_cands.len() as u8];
    let sb_me_mv = search.sb_me_mv;

    // C `ctx->ref_pruning_ctrls` + `ctx->ref_filtering_res`, computed per
    // block by `inter_search_arm::run_block_searches`
    // (`perform_md_reference_pruning`, product_coding_loop.c:9441). A
    // `default()` here silently made every ref valid — which let a pruned
    // LAST2's PME MV in and let it edge a real C candidate out of MDS1's
    // survivor set.
    let ref_pruning = search.ref_pruning.clone();
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
    //
    // The SAMPLES are kept, not only the count: C's
    // `svt_aom_init_wm_samples` precomputes both once per block into
    // `ctx->wm_sample_info[ref]`, and `svt_aom_warped_motion_parameters`
    // reads them for EVERY candidate MV rather than re-scanning. Discarding
    // them here would have meant a second, partial transcription of the same
    // four-way scan at every warp derivation.
    let mut wm_sample_num = [0u8; 8];
    let mut wm_samples: [WarpSamples; 8] = Default::default();
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
            let (n, pts, pts_inref) = crate::inter_mvp::find_warp_samples(&grid, &ctx, rf[0]);
            let slot = rf[0].max(0) as usize;
            wm_sample_num[slot] = n;
            wm_samples[slot] = WarpSamples { n, pts, pts_inref };
        }
    }
    // C `ctx->wm_ctrls` = `svt_aom_set_wm_controls(pcs->wm_level)`
    // (enc_mode_config.c:4397). The port derived `wm_level` in the wired
    // picture-level sig-deriv all along and threw it away here.
    //
    // MEASURED (benchmarks/c_motion_mode_census_2026-09-10.meta): C codes 88
    // of 1158 inter blocks WARPED_CAUSAL over the 24-cell video gate, all at
    // preset 6 and none at preset 8 -- which is exactly what this derivation
    // predicts (`wm_level` is 3 at M6 and 0 at M8 for a flat GOP at <= 720p).
    let wmc = crate::port_enc_mode_config::ctrls::set_wm_controls(f.wm_level)
        .expect("set_wm_controls answers None only for a level outside C's switch");
    // `refine_level == 0` (wm_level 1) means C refines the MV AT INJECTION
    // (`svt_aom_wm_motion_refinement` inside `inj_non_simple_modes`), but ONLY
    // on `NEWMV` clones (mode_decision.c:924-926) — non-NEWMV clones skip the
    // refinement and go straight to `svt_aom_warped_motion_parameters`. The
    // refinement is wired (`port_md::mv_refine::wm_motion_refinement`), so the
    // controls reach the injector unmodified at every level.
    let wm_enabled = wmc.enabled != 0;
    // C `ctx->cand_reduction_ctrls` — the light lane reads the per-SB
    // `sig.cand_reduction` (its `cand_reduction_level` is raised above the
    // picture's), the regular lane the picture-level row.
    let cand_red = light.map_or(&f.cand_reduction, |s| &s.cand_reduction);
    let inj = InjectCtx {
        bsize: b.bsize,
        bwidth: b.bw as u16,
        bheight: b.bh as u16,
        blk_org_x: b.org_x as u32,
        blk_org_y: b.org_y as u32,
        // C `ctx->shape == PART_N` — the inter-intra ctrls pick their
        // wedge mode between `wedge_mode_sq` and `wedge_mode_nsq` on it
        // (mode_decision.c:468). It was hardcoded true, which sent every
        // NSQ shape down the square path.
        shape_is_part_n: b.is_part_n,
        // C `frm_hdr->reference_mode == SINGLE_REFERENCE` — the value
        // `allow_bipred` reads. `REFERENCE_SELECT` frames (every inter
        // frame of a complete mini-GOP) get bipred, which is what the
        // compound entries of `ref_frame_type_arr` are for.
        reference_mode_is_single: !f.reference_mode_is_select,
        allow_high_precision_mv: f.allow_high_precision_mv,
        is_motion_mode_switchable: f.is_motion_mode_switchable,
        force_integer_mv: u8::from(f.force_integer_mv),
        // C `frm_hdr->skip_mode_params.skip_mode_flag`. The injector reads it
        // in its NEAREST_NEAREST arm to mark a COMPOUND candidate
        // `skip_mode_allowed`.
        skip_mode_flag: f.skip_mode_flag,
        // C `frm_hdr->skip_mode_params.ref_frame_idx_{0,1}` — the pair the
        // NEAREST_NEARESTMV arm compares a candidate's refs against, derived
        // by `setup_skip_mode_allowed` at picture decision.
        skip_mode_ref_frame_idx_0: f.skip_mode_ref_frame_idx_0,
        skip_mode_ref_frame_idx_1: f.skip_mode_ref_frame_idx_1,
        is_lossless_segment: false,
        // The same block-level list the MVP table and searches used —
        // `determine_best_references`' rebuild when `use_best_me` fired in
        // `block_prelude`, else the picture-level list.
        ref_frame_type_arr: &ref_arr,
        global_motion: &f.global_motion,
        // C `pcs->ppcs->gm_ctrls.skip_identity`, which
        // `svt_aom_set_gm_controls` sets ONLY at gm_level 4: with it set and a
        // reference's model IDENTITY, `inject_global_candidates` `continue`s.
        // This was hardcoded `true`, which suppressed the GLOBALMV candidate C
        // injects at every other level.
        gm_skip_identity: f.gm_skip_identity,
        // C `ctx->global_mv_injection = ppcs->gm_ctrls.enabled`
        // (enc_mode_config.c:7847/:7964). It was hardcoded `true`, which
        // injected GLOBALMV candidates at presets (>= M5) where C's gm
        // level is 0 and the whole section is skipped.
        global_mv_injection: f.gm_enabled,
        wm_sample_num: &wm_sample_num,
        ref_mv_stack: &stacks,
        ref_mv_count: &ref_mv_count,
        nmv_cost: &f.nmv,
        drl_mode_fac_bits: &f.fac.drl_mode,
        // C `ctx->shut_fast_rate` — false on both lanes this reaches
        // (enc_mode_config.c:7565 light / :7908 regular), read off the sig
        // anyway so the transcription survives a level that sets it.
        shut_fast_rate: light.map_or(false, |s| s.shut_fast_rate),
        // C `ctx->approx_inter_rate` — the regular arm writes it from
        // `pcs->approx_inter_rate` (`sig_deriv_enc_dec_default`,
        // enc_mode_config.c:7906); the light-PD1 arm's `MAX(1, ·)` differs
        // exactly where `pcs` is 0, so the sig's own field reads here.
        approx_inter_rate: light.map_or(f.search.approx_inter_rate, |s| s.approx_inter_rate),
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
        // C `ctx->cand_reduction_ctrls.redundant_cand_ctrls`. `score_th` is 0
        // at levels 0..3, i.e. everywhere this port's `cand_reduction_level`
        // can land, so this is inert TODAY and would not be if a level 4+
        // ever became reachable.
        redundant_cand_ctrls: crate::port_md::predicates::RedundantCandCtrls {
            score_th: cand_red.redundant_cand_ctrls.score_th,
            mag_th: cand_red.redundant_cand_ctrls.mag_th,
        },
        // C `set_inter_comp_controls(ctx, pcs->inter_compound_mode)`
        // (enc_mode_config.c:7856/:7973) — `get_inter_compound_level`'s
        // ladder: 0 = "AVG only" (MD_COMP_DIST, every `do_*` off) at
        // M3+, 3 at M0, 4 at M1..M2.
        inter_comp_ctrls: crate::port_enc_mode_config::ctrls::set_inter_comp_controls(
            f.inter_compound_mode,
        )
        .map(crate::port_md::inject::InterCompCtrls::from)
        .unwrap_or_default(),
        // C `set_inter_intra_ctrls(ctx->inter_intra_comp_ctrls,
        // pcs->inter_intra_level)` (mode_decision.c:420-470 / enc_mode_
        // config.c:5385) — level 2 on the reachable ladder: enabled, no
        // RD model, wedge search on NSQ shapes only.
        inter_intra_comp_ctrls: crate::port_enc_mode_config::ctrls::set_inter_intra_ctrls(
            f.inter_intra_level,
        )
        .map(|c| crate::port_md::inject::InterIntraCompCtrls {
            enabled: c.enabled != 0,
            use_rd_model: c.use_rd_model != 0,
            wedge_mode_sq: c.wedge_mode_sq,
            wedge_mode_nsq: c.wedge_mode_nsq,
        })
        .unwrap_or_default(),
        wm_ctrls: WmCtrls {
            enabled: wm_enabled,
            use_wm_for_mvp: wm_enabled && wmc.use_wm_for_mvp != 0,
            refinement_iterations: wmc.refinement_iterations,
            refine_level: wmc.refine_level,
        },
        // C `ctx->obmc_ctrls`, as `svt_aom_set_obmc_controls` expands
        // `pcs->ppcs->pic_obmc_level`. The injector reads `refine_level` (which
        // MD stage refines the MV, if any) and `enabled`.
        obmc_ctrls: {
            let c = crate::port_enc_mode_config::ctrls::set_obmc_controls(f.pic_obmc_level);
            crate::port_md::inject::ObmcCtrls {
                enabled: c.enabled != 0,
                max_blk_size: c.max_blk_size,
                trans_face_off: c.trans_face_off != 0,
                refine_level: c.refine_level,
            }
        },
        // C `ctx->cand_reduction_ctrls.near_count_ctrls`, and the ONE field
        // of that struct this envelope is not inert in: it is
        // `{enabled 1, near_count 3, near_near_count 3}` at every level the
        // default arm reaches, which is up to three `NEARMV` candidates per
        // single reference. See the module header for the measurement.
        near_count_ctrls: crate::port_md::inject::NearCountCtrls {
            enabled: cand_red.near_count_ctrls.enabled != 0,
            near_count: cand_red.near_count_ctrls.near_count,
            near_near_count: cand_red.near_count_ctrls.near_near_count,
        },
        bipred3x3_ctrls: Default::default(),
        unipred3x3_injection: 0,
        // C `ctx->new_nearest_injection` — 1 in both sig derivations this
        // lane reaches (enc_mode_config.c:7570/:7848/:7965); the light sig's
        // copy reads here so a level that clears it stays reachable.
        new_nearest_injection: light.map_or(true, |s| s.new_nearest_injection != 0),
        new_nearest_near_comb_injection: 0,
        inject_new_me: true,
        inject_new_pme: true,
        // The same per-block `ctx->updated_enable_pme` resolution
        // `search_in` got in `block_prelude` — the injector and the
        // search read ONE ctx field in C.
        updated_enable_pme: f.search.md_pme_enabled && !is_intra_bordered,
        // C `ctx->cand_reduction_ctrls.reduce_unipred_candidates` — 0 at
        // levels 0..2, so inert on this envelope for the same reason.
        reduce_unipred_candidates: cand_red.reduce_unipred_candidates,
        // C `ctx->cand_reduction_ctrls.use_neighbouring_mode_ctrls.enabled`,
        // which is 1 from level 2 up — read in conjunction with the now-real
        // `is_intra_bordered` below.
        use_neighbouring_mode_ctrls_enabled: cand_red.use_neighbouring_mode_enabled != 0,
        lpd1_mvp_best_me_list: cand_red.lpd1_mvp_best_me_list != 0,
        is_intra_bordered,
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

    // C allocates `ctx->fast_cand_array` to `pcs->ppcs->max_can_count`
    // (md_process.c:386) and `INC_MD_CAND_CNT` saturates at the same
    // value — the two must agree or a saturated push leaves
    // `inj_comp_modes` reading a stale last candidate.
    let mut cands = CandArray::new(usize::from(f.max_can_count));
    let mut log = InjectedMvLog::default();
    // The block-scoped half of what the MDS1 refinement needs, filled here
    // because this is where the samples, the stacks and the derived controls
    // already exist. `enabled` false makes the refinement a no-op without the
    // funnel having to know why.
    *warp_out = WarpRefineBlock {
        mvp_stacks: core::mem::take(&mut warp_out.mvp_stacks),
        enabled: wm_enabled,
        refinement_iterations: wmc.refinement_iterations,
        refine_diag: wmc.refine_diag != 0,
        shut_approx_if_not_mds0: wmc.shut_approx_if_not_mds0 != 0,
        lower_band_th: wmc.lower_band_th,
        upper_band_th: wmc.upper_band_th,
        refine_level: wmc.refine_level,
        samples: wm_samples,
        stacks: stacks.clone(),
        ref_mv_count,
        mi_row: (b.org_y / 4) as i32,
        mi_col: (b.org_x / 4) as i32,
        bsize: svtav1_types::block::BlockSize::from_u8(b.bsize)
            .expect("an injected inter block must have a real BlockSize"),
        bwidth: b.bw,
        bheight: b.bh,
    };
    // Hand the block's MV stacks to the funnel: the OBMC refinement's DRL
    // re-pick must use the SAME stack the injector priced against.
    warp_out.mvp_stacks.clear();
    warp_out.mvp_stacks.extend_from_slice(&stacks);
    if light.is_some() {
        // C `inject_inter_candidates_light_pd1` takes no warp hooks — the
        // light path never refines an MV at injection (`read_refine_me_mvs_
        // light_pd1` runs the light refine, not the MDS1 warp lane).
        inject_inter_candidates_light_pd1(&inj, &mut cands, &mut log);
    } else {
        let mut hooks = WarpHooks {
            blk: warp_out,
            f,
            b,
            ii_ctrls: inj.inter_intra_comp_ctrls,
            comp_ctrls: inj.inter_comp_ctrls,
            cmp: CmpStore::default(),
        };
        inject_inter_candidates(&inj, &mut cands, &mut log, &mut hooks);
    }

    let mut out = Vec::new();
    for c in cands.as_slice() {
        // Every motion mode C's regular lane can stamp is predictable
        // (`predict_inter_yuv_warped` / the OBMC blend tail), and
        // inter-intra compounds are predicted by the `combine_interintra`
        // tail in `predict_and_price`. The assertion stays because a
        // silently dropped candidate is a mode decision nobody made.
        assert!(
            matches!(
                c.motion_mode,
                crate::port_md::predicates::MotionMode::SimpleTranslation
                    | crate::port_md::predicates::MotionMode::WarpedCausal
                    | crate::port_md::predicates::MotionMode::ObmcCausal
            ),
            "the inter candidate set produced a candidate this port cannot PREDICT \
             (motion_mode {:?}, interintra {}, ref_frame {:?}). Its control was supposed \
             to be off — see `inter_md_arm`'s header. Refusing rather than dropping it, \
             because a silently dropped candidate is a mode decision nobody made.",
            c.motion_mode,
            c.is_interintra_used,
            c.ref_frame,
        );
        // C `blk_ptr->inter_mode_ctx[ref_frame_type]` — the mode context of
        // the candidate's OWN reference TYPE, which for a compound pair is
        // the compound index (>= 8), not `ref_frame[0]`.
        let imc =
            stacks[crate::inter_mvp::av1_ref_frame_type(c.ref_frame).max(0) as usize].mode_context;
        let mut o = predict_and_price(f, b, c, imc, &stacks, lambda);
        // C `cand->cand_class` (mode_decision.c:3662-3669): `NEWMV` /
        // `NEW_NEWMV` — or ANY inter candidate when `merge_inter_cands`
        // fired — is class 2; the remaining inter modes are class 1.
        o.cand_class = if merge_inter_cands
            || matches!(o.mode, PredictionMode::NewMv | PredictionMode::NewNewMv)
        {
            2
        } else {
            1
        };
        out.push(o);
    }
    out
}

/// C's SUB-8 chroma arm of `av1_inter_prediction` (`enc_inter_prediction.c:3374`),
/// for one committed-or-candidate 4xN / Nx4 block.
///
/// A sub-8 luma block's chroma covers the PARENT 8x8, so C does not predict it
/// with one MV: `inter_chroma_4xn_pred` stitches it from the covered mode-info
/// cells' own references, MVs and filters, and falls back to this block's own
/// MV over the whole area when one of those cells is INTRA.
///
/// It lives here rather than in `inter_pred_arm` because it needs both the mi
/// grid and the per-reference DPB table, and it is shared by MDS0's candidate
/// prediction and the MDS3 interpolation-filter rebuild — which used to size
/// chroma as `w / 2` and so wrote a 4xN block's chroma at the wrong stride.
#[allow(clippy::too_many_arguments)]
pub(crate) fn predict_inter_chroma_sub8(
    padded_by_ref: &[Option<&crate::picture::PaddedRef>; 8],
    grid: &[crate::intrabc_mvp::MvpMiEntry],
    grid_stride: i32,
    org_x: usize,
    org_y: usize,
    bw: usize,
    bh: usize,
    ref_frame: i8,
    mv: Mv,
    interp_filters: u32,
    sb_size: usize,
    frame_w: usize,
    frame_h: usize,
    u_out: &mut [u8],
    v_out: &mut [u8],
    uv_stride: usize,
) {
    let padded = padded_by_ref[ref_frame.max(0) as usize]
        .expect("a sub-8 inter block names a reference with no DPB picture");
    let Some((refu, refv)) = padded.uv.as_ref() else {
        return;
    };
    let mut refs_by_frame: [Option<(&crate::picture::PaddedPlane, _)>; 8] = [None; 8];
    for (i, slot) in refs_by_frame.iter_mut().enumerate() {
        *slot = padded_by_ref[i].and_then(|p| p.uv.as_ref().map(|(u, v)| (u, v)));
    }
    // C fills `xd->mi[0]` from the block being predicted (:3036-3043); every
    // other covered cell comes from the mi grid. Only the cells C's
    // `row_start` / `col_start` walk reaches are read — a (-1, *) cell for a
    // block that is not 4 HIGH would index above the frame on the first mi row.
    let mut mis = [[crate::inter_pred_arm::Sub8ChromaMi::default(); 2]; 2];
    let row_start: i32 = if bh == 4 { -1 } else { 0 };
    let col_start: i32 = if bw == 4 { -1 } else { 0 };
    for dr in row_start..=0 {
        for dc in col_start..=0 {
            mis[(dr + 1) as usize][(dc + 1) as usize] = if dr == 0 && dc == 0 {
                crate::inter_pred_arm::Sub8ChromaMi {
                    is_inter: true,
                    ref_frame,
                    mv,
                    interp_filters,
                }
            } else {
                let idx = ((org_y / 4) as i32 + dr) * grid_stride + (org_x / 4) as i32 + dc;
                let e = &grid[idx as usize];
                crate::inter_pred_arm::Sub8ChromaMi {
                    is_inter: e.use_intrabc || e.ref_frame[0] > 0,
                    ref_frame: e.ref_frame[0],
                    mv: e.mv[0],
                    interp_filters: e.interp_filters,
                }
            };
        }
    }
    let stitched = crate::inter_pred_arm::predict_inter_chroma_sub8x8(
        &refs_by_frame,
        &mis,
        org_x,
        org_y,
        bw,
        bh,
        sb_size,
        frame_w,
        frame_h,
        u_out,
        v_out,
        uv_stride,
    );
    if !stitched {
        crate::inter_pred_arm::predict_inter_chroma_whole(
            refu,
            refv,
            org_x,
            org_y,
            bw,
            bh,
            mv,
            interp_filters,
            sb_size,
            frame_w,
            frame_h,
            u_out,
            v_out,
            uv_stride,
            1,
            1,
        );
    }
}

/// The 10-bit twin of [`predict_inter_chroma_sub8`]: same covered-cell walk
/// (`inter_chroma_4xn_pred` reads `is16bit` and runs the identical walk on
/// the 16-bit picture), sourcing each cell's reference from the `hbd` DPB
/// twin instead of the 8-bit plane.
#[allow(clippy::too_many_arguments)]
pub(crate) fn predict_inter_chroma_sub8_hbd(
    padded_by_ref: &[Option<&crate::picture::PaddedRef>; 8],
    grid: &[crate::intrabc_mvp::MvpMiEntry],
    grid_stride: i32,
    org_x: usize,
    org_y: usize,
    bw: usize,
    bh: usize,
    ref_frame: i8,
    mv: Mv,
    interp_filters: u32,
    sb_size: usize,
    frame_w: usize,
    frame_h: usize,
    bit_depth: u8,
    u_out: &mut [u16],
    v_out: &mut [u16],
    uv_stride: usize,
) {
    let padded = padded_by_ref[ref_frame.max(0) as usize]
        .expect("a sub-8 inter block names a reference with no DPB picture");
    let Some(hbd) = padded.hbd.as_ref() else {
        return;
    };
    let Some((refu, refv)) = hbd.uv.as_ref() else {
        return;
    };
    let mut refs_by_frame: [Option<(
        &crate::picture::PaddedPlaneHbd,
        &crate::picture::PaddedPlaneHbd,
    )>; 8] = [None; 8];
    for (i, slot) in refs_by_frame.iter_mut().enumerate() {
        *slot = padded_by_ref[i]
            .and_then(|p| p.hbd.as_ref())
            .and_then(|h| h.uv.as_ref().map(|(u, v)| (u, v)));
    }
    let mut mis = [[crate::inter_pred_arm::Sub8ChromaMi::default(); 2]; 2];
    let row_start: i32 = if bh == 4 { -1 } else { 0 };
    let col_start: i32 = if bw == 4 { -1 } else { 0 };
    for dr in row_start..=0 {
        for dc in col_start..=0 {
            mis[(dr + 1) as usize][(dc + 1) as usize] = if dr == 0 && dc == 0 {
                crate::inter_pred_arm::Sub8ChromaMi {
                    is_inter: true,
                    ref_frame,
                    mv,
                    interp_filters,
                }
            } else {
                let idx = ((org_y / 4) as i32 + dr) * grid_stride + (org_x / 4) as i32 + dc;
                let e = &grid[idx as usize];
                crate::inter_pred_arm::Sub8ChromaMi {
                    is_inter: e.use_intrabc || e.ref_frame[0] > 0,
                    ref_frame: e.ref_frame[0],
                    mv: e.mv[0],
                    interp_filters: e.interp_filters,
                }
            };
        }
    }
    let stitched = crate::inter_pred_arm::predict_inter_chroma_sub8x8_hbd(
        &refs_by_frame,
        &mis,
        org_x,
        org_y,
        bw,
        bh,
        sb_size,
        frame_w,
        frame_h,
        bit_depth,
        u_out,
        v_out,
        uv_stride,
    );
    if !stitched {
        crate::inter_pred_arm::predict_inter_chroma_whole_hbd(
            refu,
            refv,
            org_x,
            org_y,
            bw,
            bh,
            mv,
            interp_filters,
            sb_size,
            frame_w,
            frame_h,
            bit_depth,
            u_out,
            v_out,
            uv_stride,
            1,
            1,
        );
    }
}

/// The ABOVE row and LEFT column the OBMC walk reads, projected out of the mi
/// grid into caller-owned arrays.
///
/// C reads `xd->mi` in place; this is the same span, copied so the borrow is
/// local. Both are bounded by one superblock edge in mi units plus the cell
/// the 4-wide pairing rule reaches past it, so neither allocates.
pub(crate) struct ObmcNbSpans {
    pub above: [crate::obmc_pred_arm::ObmcNbCell; OBMC_NB_SPAN],
    pub n_above: usize,
    pub left: [crate::obmc_pred_arm::ObmcNbCell; OBMC_NB_SPAN],
    pub n_left: usize,
}

/// One superblock edge in mi units (128 / 4) plus the pairing cell.
pub(crate) const OBMC_NB_SPAN: usize = 33;

const OBMC_NB_INTRA: crate::obmc_pred_arm::ObmcNbCell = crate::obmc_pred_arm::ObmcNbCell {
    bsize: svtav1_types::block::BlockSize::Block4x4,
    overlappable: false,
    ref_frame: 0,
    mv: Mv::ZERO,
    interp_filters: 0,
};

/// Project the two mi spans the OBMC walks read.
///
/// `mi_row == 0` has no ABOVE row and `mi_col == 0` no LEFT column; C gates
/// those with `xd->up_available` / `left_available`, which the caller passes
/// on rather than inferring from an empty span.
pub(crate) fn obmc_nb_spans(
    grid: &[crate::intrabc_mvp::MvpMiEntry],
    grid_stride: i32,
    mi_rows: i32,
    mi_cols: i32,
    org_x: usize,
    org_y: usize,
    bw: usize,
    bh: usize,
) -> ObmcNbSpans {
    let (mi_row, mi_col) = ((org_y / 4) as i32, (org_x / 4) as i32);
    let (n4_w, n4_h) = (bw / 4, bh / 4);
    let mut out = ObmcNbSpans {
        above: [OBMC_NB_INTRA; OBMC_NB_SPAN],
        n_above: 0,
        left: [OBMC_NB_INTRA; OBMC_NB_SPAN],
        n_left: 0,
    };
    let cell = |idx: i32| -> crate::obmc_pred_arm::ObmcNbCell {
        let e = &grid[idx as usize];
        crate::obmc_pred_arm::ObmcNbCell {
            bsize: svtav1_types::block::BlockSize::from_u8(e.bsize)
                .unwrap_or(svtav1_types::block::BlockSize::Block4x4),
            // C `is_neighbor_overlappable`: `ref_frame[0] > INTRA_FRAME`.
            // IntraBC is NOT overlappable — its `ref_frame[0]` is INTRA_FRAME.
            overlappable: e.ref_frame[0] > 0,
            ref_frame: e.ref_frame[0],
            mv: e.mv[0],
            interp_filters: e.interp_filters,
        }
    };
    if mi_row > 0 {
        // One past `n4_w`, because the pairing rule can read `idx + 1`.
        let n = ((n4_w + 1).min(OBMC_NB_SPAN)).min((mi_cols - mi_col).max(0) as usize + 1);
        for k in 0..n {
            let c = mi_col + k as i32;
            if c >= mi_cols {
                break;
            }
            out.above[k] = cell((mi_row - 1) * grid_stride + c);
            out.n_above = k + 1;
        }
    }
    if mi_col > 0 {
        let n = ((n4_h + 1).min(OBMC_NB_SPAN)).min((mi_rows - mi_row).max(0) as usize + 1);
        for k in 0..n {
            let r = mi_row + k as i32;
            if r >= mi_rows {
                break;
            }
            out.left[k] = cell(r * grid_stride + mi_col - 1);
            out.n_left = k + 1;
        }
    }
    out
}

/// `block_mi.interinter_comp` → the masked arm's `InterInterCompoundData`
/// + the DIFFWTD `mask_type`, and `dist_wtd_comp_weight_assign`'s outputs
/// for ref 1's convolve (enc_inter_prediction.c:3253-3264 — ref 0 is
/// `bck_frame_index`, ref 1 `fwd_frame_index`, `order_idx` 0). Shared by
/// the inject-time prediction and the IFS/MDS3 rebuild: C's driver runs
/// this setup once per `svt_aom_inter_prediction` call, so each port call
/// site re-derives it from the same candidate fields.
#[allow(clippy::type_complexity)]
pub(crate) fn compound_md_args(
    f: &InterMdFrame<'_>,
    ref_frame: [i8; 2],
    interinter_comp_type: u8,
    interinter_mask_type: u8,
    interinter_wedge_index: i8,
    interinter_wedge_sign: bool,
    compound_idx: u8,
) -> (
    Option<(
        svtav1_dsp::port_masked_blend::InterInterCompoundData,
        svtav1_dsp::port_masked_compound::DiffwtdMaskType,
    )>,
    Option<svtav1_dsp::port_scale_factors::DistWtdWeights>,
) {
    use svtav1_dsp::port_masked_compound::{CompoundType, DiffwtdMaskType};
    let is_compound = ref_frame[1] > crate::inter_mvp::INTRA_FRAME;
    let comp_type = match interinter_comp_type {
        0 => CompoundType::Average,
        1 => CompoundType::DistWtd,
        2 => CompoundType::Wedge,
        3 => CompoundType::DiffWtd,
        t => panic!("an injected compound candidate carries interinter_comp_type {t}"),
    };
    let masked =
        is_compound && svtav1_dsp::port_masked_compound::is_masked_compound_type(comp_type);
    let comp_data = masked.then(|| {
        (
            svtav1_dsp::port_masked_blend::InterInterCompoundData {
                compound_type: comp_type,
                wedge_index: interinter_wedge_index.max(0) as usize,
                wedge_sign: usize::from(interinter_wedge_sign),
            },
            match interinter_mask_type {
                0 => DiffwtdMaskType::D38,
                1 => DiffwtdMaskType::D38Inv,
                t => panic!("an injected DIFFWTD candidate carries mask_type {t}"),
            },
        )
    });
    // C's call is inside the `is_compound` arm — a single-reference or
    // inter-intra candidate never reaches the weight assign.
    let distwtd = is_compound.then(|| {
        svtav1_dsp::port_scale_factors::dist_wtd_comp_weight_assign(
            f.order_hint.enable_order_hint,
            f.order_hint.order_hint_bits as i32,
            f.order_hint.cur_order_hint,
            f.order_hint.ref_order_hint[(ref_frame[0] - 1) as usize],
            f.order_hint.ref_order_hint[(ref_frame[1] - 1) as usize],
            i32::from(compound_idx),
            0,
            true,
            svtav1_dsp::port_scale_factors::DistWtdWeights {
                fwd_offset: 0,
                bck_offset: 0,
                use_dist_wtd_comp_avg: 0,
            },
        )
    });
    (comp_data, distwtd)
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
    // C `block_mi.interp_filters` at injection: every C injector leaves it
    // at EIGHTTAP_REGULAR in both directions (packed 0), and the filter is
    // decided later by the interpolation-filter search at the stage
    // `ifs_ctrls.level` names — MDS3 on this port's ladders, run by
    // `leaf_funnel::ifs::ifs_at_mds3`. This prediction and the MDS0 rate
    // are therefore C's PRE-search values, exactly as they are in C.
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
    // C `blk_geom->bwidth_uv` = `MAX(4, bwidth >> 1)` (utility.c:274), which
    // at 4:2:0 is the same extent as `get_plane_block_size(bsize, 1, 1)` and
    // as the funnel's own `cw`/`chh`. It was `b.bw / 2`, which is 2 on a
    // 4-wide block — half the chroma the funnel then reads, and the reason a
    // 4xN inter leaf indexed past the end of its own prediction.
    let (cw, chh) = (b.bw.max(8) / 2, b.bh.max(8) / 2);
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
    // C `has_second_ref(&mbmi->block_mi)` — spec 7.10.1: `rf[1] >
    // INTRA_FRAME`. An INTER-INTRA candidate's `ref_frame[1]` is
    // INTRA_FRAME (0), which is NOT a second reference — `> NONE_FRAME`
    // would index `padded_by_ref[0]` for a picture that is not a
    // reference at all.
    let is_compound = c.ref_frame[1] > crate::inter_mvp::INTRA_FRAME;
    let padded1 = is_compound.then(|| {
        f.padded_by_ref[c.ref_frame[1].max(0) as usize].unwrap_or_else(|| {
            panic!(
                "an inter candidate names reference {} with no DPB picture — \
                 `ref_frame_type_arr` and `padded_by_ref` disagree",
                c.ref_frame[1]
            )
        })
    });
    // The WARP driver takes a DIFFERENT C path — `av1_inter_prediction`'s
    // `is_wm` arm, not `av1_inter_prediction_light_pd1` — so it is dispatched
    // here rather than flagged inside the translation adapter. See
    // `inter_pred_arm::predict_inter_yuv_warped`.
    //
    // `is_wm` is C's OWN two-term condition, not just the motion mode: a
    // GLOBALMV candidate is injected as SIMPLE_TRANSLATION, and with a model
    // above TRANSLATION the decoder still warps it.
    // `is_wm` is PER REFERENCE in C (`av1_inter_prediction`,
    // enc_inter_prediction.c:3276): ref `i` warps when ITS model is above
    // TRANSLATION. A GLOBAL_GLOBALMV candidate keeps `wm_params_l0`/`l1`
    // and both refs are evaluated against their own model — the
    // single-model check used to route the whole block through the warp
    // leaf with only ref 0's plane, which silently dropped the second
    // reference's prediction (a decoder averages BOTH, so every committed
    // GLOBAL_GLOBALMV block's recon disagreed with the stream).
    let is_wm =
        crate::inter_pred_arm::inter_pred_uses_warp(mm, c.mode as u8, b.bw, b.bh, &c.wm_params_l0);
    let is_wm1 = padded1.is_some()
        && crate::inter_pred_arm::inter_pred_uses_warp(
            mm,
            c.mode as u8,
            b.bw,
            b.bh,
            &c.wm_params_l1,
        );
    // `block_mi.interinter_comp` → the masked arm's `InterInterCompoundData`
    // + the DIFFWTD `mask_type`, and `dist_wtd_comp_weight_assign`'s output
    // for ref 1's convolve — shared with the IFS/MDS3 rebuild via
    // [`compound_md_args`].
    let (comp_data, distwtd) = compound_md_args(
        f,
        c.ref_frame,
        c.interinter_comp_type,
        c.interinter_mask_type,
        c.interinter_wedge_index,
        c.interinter_wedge_sign,
        c.compound_idx,
    );
    if let Some(p1) = padded1 {
        // COMPOUND — C's `av1_inter_prediction` compound arm: each ref
        // picks warp or convolve by its OWN model over one shared
        // CONV_BUF, ref 1 takes the distance-weighted offsets and the
        // masked blend when `interinter_comp.type` is WEDGE/DIFFWTD.
        let mut wm0 = c.wm_params_l0;
        let mut wm1 = c.wm_params_l1;
        crate::inter_pred_arm::predict_inter_yuv_compound_md(
            [
                (
                    &padded.y,
                    padded.uv.as_ref().map(|(u, v)| (u, v)).filter(|_| b.has_uv),
                ),
                (
                    &p1.y,
                    p1.uv.as_ref().map(|(u, v)| (u, v)).filter(|_| b.has_uv),
                ),
            ],
            &mut wm0,
            &mut wm1,
            [is_wm, is_wm1],
            comp_data,
            distwtd,
            &f.wedge_masks,
            b.org_x,
            b.org_y,
            b.bw,
            b.bh,
            b.bsize as usize,
            c.mv,
            interp_filters,
            f.sb_size,
            f.frame_w,
            f.frame_h,
            &mut y_pred,
            b.bw,
            &mut u_pred,
            &mut v_pred,
            cw,
        );
    } else if is_wm {
        let mut wm = c.wm_params_l0;
        crate::inter_pred_arm::predict_inter_yuv_warped(
            (
                &padded.y,
                padded.uv.as_ref().map(|(u, v)| (u, v)).filter(|_| b.has_uv),
            ),
            &mut wm,
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
        );
    } else {
        let sub8 = b.bw < 8 || b.bh < 8;
        match (b.has_uv && !sub8, padded.uv.as_ref()) {
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
        // C `av1_inter_prediction`'s SUB-8 chroma arm: a 4xN / Nx4 block's
        // chroma covers the parent 8x8, so C stitches it from the covered
        // cells' own MVs (`inter_chroma_4xn_pred`) and falls back to this
        // block's MV over the whole area when one of them is intra. Neither
        // is what the luma-shaped `predict_inter_yuv` above does, which is why
        // the sub-8 case is split out of it rather than folded in.
        if b.has_uv && sub8 {
            predict_inter_chroma_sub8(
                &f.padded_by_ref,
                b.grid,
                b.grid_stride,
                b.org_x,
                b.org_y,
                b.bw,
                b.bh,
                c.ref_frame[0],
                c.mv[0],
                interp_filters,
                f.sb_size,
                f.frame_w,
                f.frame_h,
                &mut u_pred,
                &mut v_pred,
                cw,
            );
        }
    }

    // ---- Inter-intra, C `av1_inter_prediction`'s tail ----
    // `inter_intra_prediction` (enc_inter_prediction.c:3468-3478 ->
    // inter_prediction.c:2217+) blends the block's OWN prediction with the
    // precomputed intra prediction under the smooth (or wedge) mask —
    // INSIDE `av1_inter_prediction`, so it runs before the OBMC tail
    // below, per plane. `intrapred_buf` is the block's, which
    // `precompute_intra_pred_for_inter_intra` fills only for an
    // inter-intra-eligible block — an II candidate on a block without it
    // is a wiring bug, not a slow path.
    if c.is_interintra_used {
        let ii = b
            .ii
            .as_ref()
            .expect("an inter-intra candidate reached prediction on a block with no intrapred_buf");
        let n = b.bw * b.bh;
        let mode = svtav1_dsp::port_interintra::InterIntraMode::ALL[c.interintra_mode as usize];
        let mut inter = alloc::vec![0u8; n];
        inter.copy_from_slice(&y_pred);
        svtav1_dsp::port_interintra::combine_interintra(
            &f.ii_masks,
            &f.wedge_masks,
            mode,
            c.use_wedge_interintra,
            c.interintra_wedge_index as usize,
            // C `INTERINTRA_WEDGE_SIGN` — the wedge search is run at a
            // FIXED sign (pick_wedge_fixed_sign), so the blend reads the
            // same sign it was picked at; it is not a coded symbol.
            0,
            b.bsize as usize,
            b.bsize as usize,
            &mut y_pred,
            b.bw,
            &inter,
            b.bw,
            &ii.luma[c.interintra_mode as usize * n..],
            b.bw,
        );
        if b.has_uv {
            // C :2217's per-plane loop — `plane_bsize` is the chroma
            // plane's own block size while `bsize` stays the LUMA one
            // (the wedge mask's sub-sampling test reads it).
            let plane_bsize = svtav1_dsp::port_obmc_data::get_plane_block_size(
                svtav1_types::block::BlockSize::from_u8(b.bsize)
                    .expect("an injected inter block must have a real BlockSize"),
                1,
                1,
            )
            .expect("an inter block's chroma has a real plane_bsize")
                as usize;
            let cn = cw * chh;
            for (pred, plane_ii) in [(&mut u_pred, &ii.u), (&mut v_pred, &ii.v)] {
                let mut inter_uv = alloc::vec![0u8; cn];
                inter_uv.copy_from_slice(pred);
                svtav1_dsp::port_interintra::combine_interintra(
                    &f.ii_masks,
                    &f.wedge_masks,
                    mode,
                    c.use_wedge_interintra,
                    c.interintra_wedge_index as usize,
                    0,
                    b.bsize as usize,
                    plane_bsize,
                    pred,
                    cw,
                    &inter_uv,
                    cw,
                    &plane_ii[c.interintra_mode as usize * cn..],
                    cw,
                );
            }
        }
    }

    // ---- OBMC, C `svt_aom_inter_prediction`'s tail (:3511) ----
    // The blend runs AFTER the block's own prediction and rewrites its edges
    // in place, so it sits here rather than as a third arm of the dispatch
    // above. It is re-applied wherever the prediction is rebuilt -- see
    // `leaf_funnel::ifs`. The spans are hoisted so the 10-bit arm below
    // blends the SAME neighbours the 8-bit one does.
    let obmc_spans = (mm == MotionMode::ObmcCausal).then(|| {
        obmc_nb_spans(
            b.grid,
            b.grid_stride,
            f.mi_rows,
            f.mi_cols,
            b.org_x,
            b.org_y,
            b.bw,
            b.bh,
        )
    });
    if let Some(spans) = &obmc_spans {
        crate::obmc_pred_arm::predict_obmc_in_place(
            &crate::obmc_pred_arm::ObmcCtx {
                padded_by_ref: &f.padded_by_ref,
                above_row: &spans.above[..spans.n_above],
                left_col: &spans.left[..spans.n_left],
                up_available: b.org_y > 0,
                left_available: b.org_x > 0,
                mi_cols: f.mi_cols.max(0) as usize,
                mi_rows: f.mi_rows.max(0) as usize,
                sb_size: f.sb_size,
                frame_w: f.frame_w,
                frame_h: f.frame_h,
                edges: crate::inter_pred_arm::block_mb_edges(
                    b.org_x, b.org_y, b.bw, b.bh, f.frame_w, f.frame_h,
                ),
            },
            svtav1_types::block::BlockSize::from_u8(b.bsize)
                .expect("an injected inter block must have a real BlockSize"),
            b.org_x,
            b.org_y,
            b.bw,
            b.bh,
            &mut y_pred,
            b.bw,
            &mut u_pred,
            &mut v_pred,
            cw,
        );
    }

    // The SAME prediction at true 10 bits, when the DPB carries a 10-bit twin
    // of this reference. C does not do this twice — at `bd > EB_EIGHT_BIT` its
    // reference IS the 16-bit picture and the 8-bit call above does not exist.
    // The port keeps both because its u8 mode-decision stages still read
    // `y_pred`, and the bd10 full-RD funnel reads `y_pred10`.
    let (mut y_pred10, mut u_pred10, mut v_pred10) = (Vec::new(), Vec::new(), Vec::new());
    if let Some(hbd) = padded.hbd.as_ref() {
        y_pred10 = alloc::vec![0u16; b.bw * b.bh];
        let hbd1 = padded1.and_then(|p| p.hbd.as_ref());
        let want_uv = b.has_uv && hbd.uv.is_some() && hbd1.is_none_or(|h| h.uv.is_some());
        if want_uv {
            u_pred10 = alloc::vec![0u16; cw * chh];
            v_pred10 = alloc::vec![0u16; cw * chh];
        }
        if let Some(h1) = hbd1 {
            // The SAME compound arm at 10 bits — `predict_inter_yuv_
            // compound_md`'s `SrcPlanes::Hbd` twin.
            let mut wm0 = c.wm_params_l0;
            let mut wm1 = c.wm_params_l1;
            crate::inter_pred_arm::predict_inter_yuv_compound_md_hbd(
                [
                    (
                        &hbd.y,
                        hbd.uv.as_ref().map(|(u, v)| (u, v)).filter(|_| want_uv),
                    ),
                    (
                        &h1.y,
                        h1.uv.as_ref().map(|(u, v)| (u, v)).filter(|_| want_uv),
                    ),
                ],
                &mut wm0,
                &mut wm1,
                [is_wm, is_wm1],
                comp_data,
                distwtd,
                &f.wedge_masks,
                b.org_x,
                b.org_y,
                b.bw,
                b.bh,
                b.bsize as usize,
                c.mv,
                interp_filters,
                f.sb_size,
                f.frame_w,
                f.frame_h,
                f.bit_depth,
                &mut y_pred10,
                b.bw,
                &mut u_pred10,
                &mut v_pred10,
                cw,
            );
        } else {
            // C's OBMC block predicts with the PLAIN inter path first and
            // blends the neighbours in after — `predict_inter_leaf_hbd`'s
            // OBMC assert guards the committed-leaf post-pass, so the base
            // prediction here is taken as SIMPLE_TRANSLATION and the blend
            // is applied below, exactly as the 8-bit arm does.
            let mm_base = if mm == MotionMode::ObmcCausal {
                MotionMode::SimpleTranslation
            } else {
                mm
            };
            // A sub-8 luma block's chroma covers the parent 8x8 and is
            // stitched from the covered cells' own MVs
            // (`inter_chroma_4xn_pred`), exactly as the 8-bit arm's
            // `predict_inter_chroma_sub8` — the luma-shaped hbd leaf would
            // predict `bw/2 x bh/2` at the block's own origin, which is
            // neither the right area nor the right motion.
            let sub8 = b.bw < 8 || b.bh < 8;
            crate::inter_pred_arm::predict_inter_leaf_hbd(
                &hbd.y,
                if want_uv && !sub8 {
                    hbd.uv.as_ref().map(|(u, v)| (u, v))
                } else {
                    None
                },
                mm_base,
                is_wm,
                c.wm_params_l0,
                b.org_x,
                b.org_y,
                b.bw,
                b.bh,
                c.mv[0],
                interp_filters,
                f.sb_size,
                f.frame_w,
                f.frame_h,
                f.bit_depth,
                &mut y_pred10,
                b.bw,
                &mut u_pred10,
                &mut v_pred10,
                cw,
            );
            if want_uv && sub8 {
                predict_inter_chroma_sub8_hbd(
                    &f.padded_by_ref,
                    b.grid,
                    b.grid_stride,
                    b.org_x,
                    b.org_y,
                    b.bw,
                    b.bh,
                    c.ref_frame[0],
                    c.mv[0],
                    interp_filters,
                    f.sb_size,
                    f.frame_w,
                    f.frame_h,
                    f.bit_depth,
                    &mut u_pred10,
                    &mut v_pred10,
                    cw,
                );
            }
        }
        // The SAME inter-intra blend at 10 bits — `combine_interintra_
        // highbd` on the u16 `pred10` buffers, still inside C's
        // `av1_inter_prediction` tail so the OBMC blend below applies on
        // top of it. `luma16` is empty when the block has no u16 recon
        // canvas (M6-M8 bd10): `pred10` has no consumer there — C's
        // hbd_md=0 produces no u16 inter-intra prediction either — so the
        // blend is skipped rather than faked.
        if c.is_interintra_used {
            let ii = b.ii.as_ref().expect(
                "an inter-intra candidate reached prediction on a block with no intrapred_buf",
            );
            if !ii.luma16.is_empty() {
                let n = b.bw * b.bh;
                let mode =
                    svtav1_dsp::port_interintra::InterIntraMode::ALL[c.interintra_mode as usize];
                let mut inter = alloc::vec![0u16; n];
                inter.copy_from_slice(&y_pred10);
                svtav1_dsp::port_interintra::combine_interintra_highbd(
                    &f.ii_masks,
                    &f.wedge_masks,
                    mode,
                    c.use_wedge_interintra,
                    c.interintra_wedge_index as usize,
                    0,
                    b.bsize as usize,
                    b.bsize as usize,
                    &mut y_pred10,
                    b.bw,
                    &inter,
                    b.bw,
                    &ii.luma16[c.interintra_mode as usize * n..],
                    b.bw,
                );
                if want_uv && !ii.u16.is_empty() && !ii.v16.is_empty() {
                    let plane_bsize = svtav1_dsp::port_obmc_data::get_plane_block_size(
                        svtav1_types::block::BlockSize::from_u8(b.bsize)
                            .expect("an injected inter block must have a real BlockSize"),
                        1,
                        1,
                    )
                    .expect("an inter block's chroma has a real plane_bsize")
                        as usize;
                    let cn = cw * chh;
                    for (pred, plane_ii) in [(&mut u_pred10, &ii.u16), (&mut v_pred10, &ii.v16)] {
                        let mut inter_uv = alloc::vec![0u16; cn];
                        inter_uv.copy_from_slice(pred);
                        svtav1_dsp::port_interintra::combine_interintra_highbd(
                            &f.ii_masks,
                            &f.wedge_masks,
                            mode,
                            c.use_wedge_interintra,
                            c.interintra_wedge_index as usize,
                            0,
                            b.bsize as usize,
                            plane_bsize,
                            pred,
                            cw,
                            &inter_uv,
                            cw,
                            &plane_ii[c.interintra_mode as usize * cn..],
                            cw,
                        );
                    }
                }
            }
        }
        // The SAME OBMC blend at 10 bits (`av1_inter_prediction_obmc` with
        // `is16bit`): the neighbours' predictions are rebuilt from each
        // reference's `hbd` twin and the blend is the u16 hmask/vmask pair.
        if let Some(spans) = &obmc_spans {
            crate::obmc_pred_arm::predict_obmc_in_place_hbd(
                &crate::obmc_pred_arm::ObmcCtx {
                    padded_by_ref: &f.padded_by_ref,
                    above_row: &spans.above[..spans.n_above],
                    left_col: &spans.left[..spans.n_left],
                    up_available: b.org_y > 0,
                    left_available: b.org_x > 0,
                    mi_cols: f.mi_cols.max(0) as usize,
                    mi_rows: f.mi_rows.max(0) as usize,
                    sb_size: f.sb_size,
                    frame_w: f.frame_w,
                    frame_h: f.frame_h,
                    edges: crate::inter_pred_arm::block_mb_edges(
                        b.org_x, b.org_y, b.bw, b.bh, f.frame_w, f.frame_h,
                    ),
                },
                svtav1_types::block::BlockSize::from_u8(b.bsize)
                    .expect("an injected inter block must have a real BlockSize"),
                b.org_x,
                b.org_y,
                b.bw,
                b.bh,
                f.bit_depth,
                &mut y_pred10,
                b.bw,
                &mut u_pred10,
                &mut v_pred10,
                cw,
            );
        }
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
    // C indexes `ref_mv_stack` and `ref_frames_num_bits` by the candidate's
    // `MvReferenceFrame` TYPE — `LAST_FRAME` for a single reference, the
    // compound index (8..) for a pair. `av1_ref_frame_type` collapses the
    // candidate's `{rf0, rf1}` pair back to that index.
    let ref_type = crate::inter_mvp::av1_ref_frame_type(c.ref_frame);
    let ref_bits = crate::port_md::ref_frame_rate::estimate_ref_frames_num_bits(
        &[ref_type],
        &counts,
        rr_above,
        rr_left,
        f.reference_mode_is_select,
        b.bw as u16,
        b.bh as u16,
        &f.ref_fac,
        crate::inter_mvp::av1_set_ref_frame,
    );
    let ref_frames_num_bits = ref_bits.first().map_or(0, |&(_, bits)| bits);

    let bsize = svtav1_types::block::BlockSize::from_u8(b.bsize)
        .expect("an injected inter block must have a real BlockSize");
    let stack = &stacks[ref_type.max(0) as usize];
    // C `ctx->skip_mode_ctx` = `av1_get_skip_mode_context(xd)`
    // (`entropy_coding.c:1097`), the same neighbour pair the writer uses.
    // Per-block: priced at MDS0 and re-read by the funnel stages' skip-mode
    // arbitration off the candidate.
    let skip_mode_ctx = crate::port_entropy_inter::modes::skip_mode_context(&b.neighbors);
    let cost = inter_fast_cost(
        &f.cost_frame(),
        &InterBlock {
            bsize,
            skip_mode_ctx,
            is_inter_ctx: b.is_inter_ctx,
            inter_mode_ctx,
            ref_mv_count: stack.count,
            ref_mv_stack: &stack.stack,
            ref_frames_num_bits,
            neighbors: &b.neighbors,
            overlappable_neighbors: b.overlappable_neighbors,
            // C `ctx->approx_inter_rate` — `sig_deriv_enc_dec_default`
            // copies `pcs->approx_inter_rate` (enc_mode_config.c:7906);
            // see the InjectCtx note above.
            approx_inter_rate: f.search.approx_inter_rate,
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
            // cost. The filter is priced where C prices it — after the MDS3
            // search, `fast_luma_rate += switchable_rate`
            // (enc_inter_prediction.c:2211) — by `leaf_funnel::ifs`.
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
            is_interintra_used: c.is_interintra_used,
            interintra_mode: c.interintra_mode,
            use_wedge_interintra: c.use_wedge_interintra,
            interintra_wedge_index: c.interintra_wedge_index.max(0) as u8,
            comp_group_idx: c.comp_group_idx,
            compound_idx: c.compound_idx,
            interinter_comp_type: match c.interinter_comp_type {
                1 => svtav1_types::prediction::CompoundType::DistWtd,
                2 => svtav1_types::prediction::CompoundType::Wedge,
                3 => svtav1_types::prediction::CompoundType::DiffWtd,
                _ => svtav1_types::prediction::CompoundType::Average,
            },
            interinter_wedge_index: c.interinter_wedge_index.max(0) as u8,
            skip_mode_allowed: c.skip_mode_allowed,
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
        y_pred10,
        u_pred10,
        v_pred10,
        wm_params_l0: c.wm_params_l0,
        wm_params_l1: c.wm_params_l1,
        fast_luma_rate: cost.rate.luma,
        fast_cost_rate: cost.charged_rate,
        num_proj_ref: c.num_proj_ref,
        // Stamped by `build_inter_candidates` once the block's
        // `merge_inter_cands` decision is known.
        cand_class: 0,
        comp_group_idx: c.comp_group_idx,
        compound_idx: c.compound_idx,
        interinter_comp_type: c.interinter_comp_type,
        interinter_mask_type: c.interinter_mask_type,
        interinter_wedge_index: c.interinter_wedge_index,
        interinter_wedge_sign: c.interinter_wedge_sign,
        is_interintra_used: c.is_interintra_used,
        interintra_mode: c.interintra_mode,
        use_wedge_interintra: c.use_wedge_interintra,
        interintra_wedge_index: c.interintra_wedge_index,
        skip_mode_allowed: c.skip_mode_allowed,
        skip_mode_ctx: skip_mode_ctx as u8,
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
            skip_mode: e.skip_mode,
            skip: e.skip,
            comp_group_idx: e.comp_group_idx,
            compound_idx: e.compound_idx,
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
