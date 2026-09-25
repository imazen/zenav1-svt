use super::*;

/// The per-picture state `av1_generate_rps_info` reads and writes.
///
/// Mirrors the `PictureParentControlSet` fields in scope for this module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PicParams {
    /// C `pcs->picture_number` — display-order POC.
    pub picture_number: u64,
    /// C `pcs->decode_order`.
    pub decode_order: u64,
    /// C `pcs->slice_type`.
    pub slice_type: SliceType,
    /// C `pcs->frm_hdr.frame_type == KEY_FRAME`.
    pub is_key_frame: bool,
    /// C `frame_is_intra_only(pcs)`.
    pub is_intra_only: bool,
    /// C `pcs->temporal_layer_index`.
    pub temporal_layer_index: u8,
    /// C `pcs->hierarchical_levels`.
    pub hierarchical_levels: u8,
    /// C `pcs->is_overlay`.
    pub is_overlay: bool,
    /// C `svt_aom_is_delayed_intra(pcs)` — whether an intra picture is held
    /// back to join the next mini-GOP; selects `tf_params_per_type[0]` in
    /// `copy_tf_params`. Precomputed by the caller because C's inputs
    /// (`end_of_sequence_flag`, the pre-assignment buffer) live outside
    /// [`PicParams`].
    pub is_delayed_intra: bool,
    /// C `pcs->tf_ctrls` — the `tf_params_per_type` entry `copy_tf_params`
    /// selected for this picture, stamped by [`init_pic_settings`].
    pub tf_ctrls: TfCtrls,
    /// C `pcs->frame_width` — the UNALIGNED source width. `derive_tf_window_params`
    /// compares it pairwise to exclude resolution-change candidates
    /// (`pd_process.c:3881` et seq.).
    pub frame_width: u32,
    /// C `pcs->frame_height`.
    pub frame_height: u32,
    /// C `pcs->filt_to_unfilt_diff` — `~0` from the pcs ctor
    /// (`resource_coordination_process.c:347`), then inherited from
    /// `ctx->filt_to_unfilt_diff` before `mctf_frame` runs
    /// (`pd_process.c:5122`), and finally overwritten by the filter itself
    /// (`temporal_filtering.c:4108`).
    pub filt_to_unfilt_diff: u32,
    /// C `pcs->is_noise_level` — `last_i_noise_levels_log1p_fp16[0] >=
    /// VQ_NOISE_LVL_TH`, stamped on EVERY picture by `mctf_frame`
    /// (`pd_process.c:4240-4241`), TF enabled or not.
    pub is_noise_level: bool,
    /// C `pcs->noise_levels_log1p_fp16[3]` — the Q16 `log1p` noise levels the
    /// temporal filter weights off.
    pub noise_levels_log1p_fp16: [i32; 3],
    /// C `pcs->past_altref_nframes`.
    pub past_altref_nframes: u8,
    /// C `pcs->future_altref_nframes`.
    pub future_altref_nframes: u8,
    /// C `pcs->tf_avg_luma` — the window-mean member `avg_luma`, centre excluded.
    pub tf_avg_luma: u64,
    /// C `pcs->tf_avg_ahd_error`.
    pub tf_avg_ahd_error: i32,
    /// The picture's assembled temporal-filter window — C's
    /// `temp_filt_pcs_list` plus the per-member `tf_ahd_error_to_central` /
    /// `tf_active_region_present` stamps. `Some` only when
    /// `derive_tf_window_params`' list half ran (`tf_ctrls.enabled`).
    pub tf_window: Option<alloc::boxed::Box<TfWindow>>,
    /// C `pcs->pred_struct_ptr->pred_type` — the *picture's* structure, which
    /// can be `LOW_DELAY` inside a `RANDOM_ACCESS` sequence (an incomplete MG).
    pub pred_struct_type: PredStructure,
    /// C `pcs->pred_struct_ptr->pred_struct_entry_count`.
    pub pred_struct_entry_count: u32,
    /// C `pcs->update_type`.
    pub update_type: FrameUpdateType,
    /// C `pcs->layer_depth`.
    pub layer_depth: u8,
    /// C `pcs->frame_offset`.
    pub frame_offset: u64,
    /// C `pcs->is_ref`.
    pub is_ref: bool,
    /// C `pcs->av1_ref_signal`.
    pub rps: Av1RpsNode,
    /// C `pcs->ref_list0_count`.
    pub ref_list0_count: u8,
    /// C `pcs->ref_list1_count`.
    pub ref_list1_count: u8,
    /// C `pcs->ref_list0_count_try`.
    pub ref_list0_count_try: u8,
    /// C `pcs->ref_list1_count_try`.
    pub ref_list1_count_try: u8,
    /// C `pcs->avg_luma` — the input's 1/16-plane mean, `INVALID_LUMA` when
    /// `calc_hist` is off (`pic_analysis_process.c:628`). Stamped by the
    /// caller before [`picture_decision_per_picture`]; `send_picture_out`'s
    /// `get_similar_ref_brightness` compares it against the references'.
    pub avg_luma: u64,
    /// C `pcs->similar_brightness_refs` — stamped by `send_picture_out`
    /// (`pd_process.c:4305`), read by `motion_estimation.c:2231`'s
    /// safe-limit ME prune.
    pub similar_brightness_refs: bool,
    /// C `pcs->ref_order_hint[7]` — the written frame-header field.
    pub ref_order_hint: [u32; INTER_REFS_PER_FRAME],
    /// C `pcs->cur_order_hint`.
    pub cur_order_hint: u32,
    /// C `pcs->frm_hdr.show_frame`.
    pub show_frame: bool,
    /// C `pcs->has_show_existing`.
    pub has_show_existing: bool,
    /// C `pcs->frm_hdr.show_existing_frame` — the DPB slot a
    /// `show_existing_frame` header re-displays. Only meaningful while
    /// [`PicParams::has_show_existing`] is set.
    pub show_existing_frame: u8,
    /// C `pcs->frm_hdr.reference_mode`.
    pub reference_mode: ReferenceMode,
    /// C `pcs->allow_comp_inter_inter`.
    pub allow_comp_inter_inter: bool,
    /// C `pcs->frm_hdr.skip_mode_params`.
    pub skip_mode: SkipModeInfo,
    /// C `pcs->av1_cm->ref_frame_sign_bias[8]`.
    pub ref_frame_sign_bias: [i32; REF_FRAMES],
    /// C `pcs->ref_frame_type_arr` (`MvReferenceFrame`, `MODE_CTX_REF_FRAMES` long).
    pub ref_frame_type_arr: [i8; MODE_CTX_REF_FRAMES],
    /// C `pcs->tot_ref_frame_types`.
    pub tot_ref_frame_types: u8,
    /// C `pcs->transition_present`.
    pub transition_present: u8,
    /// C `pcs->av1_cm->mi_cols`.
    pub mi_cols: u32,
    /// C `pcs->av1_cm->mi_rows`.
    pub mi_rows: u32,
    /// C `pcs->aligned_width`.
    pub aligned_width: u32,
    /// C `pcs->aligned_height`.
    pub aligned_height: u32,
    /// C `pcs->ref_mgmt` — the long-term-reference events the application
    /// queued on this picture (see [`crate::port_ref_mgmt`]).
    pub ref_mgmt: crate::port_ref_mgmt::RefMgmtEvents,
    /// C `pcs->frm_hdr.frame_type == S_FRAME` (see [`crate::port_sframe`]).
    /// Tracked as a flag rather than a full `frame_type` enum because
    /// `is_key_frame` and `slice_type` already carry the other distinctions
    /// this module needs.
    pub is_switch_frame: bool,
    /// C `pcs->frm_hdr.error_resilient_mode` — a written header bit, set by
    /// [`crate::port_sframe::set_sframe_rps`].
    pub error_resilient_mode: bool,
    /// C `pcs->picture_qp`.
    pub picture_qp: u8,
    /// C `pcs->qp_on_the_fly`.
    pub qp_on_the_fly: bool,
    /// C `pcs->sframe_qp_offset`.
    pub sframe_qp_offset: i8,
    /// C `pcs->sframe_ref_pruned`.
    pub sframe_ref_pruned: bool,
    /// C `pcs->dpb_order_hint[REF_FRAMES]` — the per-slot order hints an
    /// error-resilient frame writes in its header.
    pub dpb_order_hint: [u32; REF_FRAMES],
    /// The shadow DPB exactly as `update_ref_poc_array` saw it — snapshotted
    /// inside [`picture_decision_per_picture`] between [`generate_rps_info`]
    /// and [`update_dpb`].
    ///
    /// The bind-time reference queue MUST be built from this snapshot, not
    /// from `ctx.dpb` at bind time: `update_dpb` has already applied THIS
    /// picture's `refresh_frame_mask`, and masks like
    /// `ld_reduce_ref_buffs == 2`'s `0xfd` overwrite the slots the picture's
    /// own references live in — so the post-refresh DPB no longer names them.
    /// C's `ref_pic_list` is the picture manager's own list and outlives the
    /// shadow-DPB refresh; this snapshot is that list's contents for every
    /// POC `ref_poc_array` can name.
    pub ref_queue_dpb: [DpbEntry; REF_FRAMES],
}

impl Default for PicParams {
    fn default() -> Self {
        Self {
            picture_number: 0,
            decode_order: 0,
            slice_type: SliceType::B,
            is_key_frame: false,
            is_intra_only: false,
            temporal_layer_index: 0,
            hierarchical_levels: 0,
            is_overlay: false,
            is_delayed_intra: false,
            tf_ctrls: TfCtrls::default(),
            frame_width: 0,
            frame_height: 0,
            filt_to_unfilt_diff: u32::MAX,
            is_noise_level: false,
            noise_levels_log1p_fp16: [0; 3],
            past_altref_nframes: 0,
            future_altref_nframes: 0,
            tf_avg_luma: 0,
            tf_avg_ahd_error: 0,
            tf_window: None,
            pred_struct_type: PredStructure::LowDelay,
            pred_struct_entry_count: 1,
            update_type: FrameUpdateType::Lf,
            layer_depth: 0,
            frame_offset: 0,
            is_ref: true,
            rps: Av1RpsNode::default(),
            ref_list0_count: 0,
            ref_list1_count: 0,
            ref_list0_count_try: 0,
            ref_list1_count_try: 0,
            avg_luma: INVALID_LUMA,
            similar_brightness_refs: false,
            ref_order_hint: [0; INTER_REFS_PER_FRAME],
            cur_order_hint: 0,
            show_frame: true,
            has_show_existing: false,
            show_existing_frame: 0,
            reference_mode: ReferenceMode::Select,
            allow_comp_inter_inter: false,
            skip_mode: SkipModeInfo::default(),
            ref_frame_sign_bias: [0; REF_FRAMES],
            ref_frame_type_arr: [0; MODE_CTX_REF_FRAMES],
            tot_ref_frame_types: 0,
            transition_present: 0,
            mi_cols: 0,
            mi_rows: 0,
            aligned_width: 0,
            aligned_height: 0,
            ref_mgmt: crate::port_ref_mgmt::RefMgmtEvents::default(),
            is_switch_frame: false,
            error_resilient_mode: false,
            picture_qp: 0,
            qp_on_the_fly: false,
            sframe_qp_offset: 0,
            sframe_ref_pruned: false,
            dpb_order_hint: [0; REF_FRAMES],
            ref_queue_dpb: [DpbEntry::default(); REF_FRAMES],
        }
    }
}

/// C `SkipModeInfo` (`frame_header`): spec 5.9.22 skip-mode params.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkipModeInfo {
    /// C `skip_mode_allowed`.
    pub skip_mode_allowed: i32,
    /// C `skip_mode_flag` — the written header bit.
    pub skip_mode_flag: i32,
    /// C `ref_frame_idx_0`.
    pub ref_frame_idx_0: i32,
    /// C `ref_frame_idx_1`.
    pub ref_frame_idx_1: i32,
}

impl Default for SkipModeInfo {
    fn default() -> Self {
        Self {
            skip_mode_allowed: 0,
            skip_mode_flag: 0,
            ref_frame_idx_0: INVALID_IDX,
            ref_frame_idx_1: INVALID_IDX,
        }
    }
}

/// The `PictureDecisionContext` state the RPS branches carry across frames.
///
/// The toggles are the whole reason this is stateful: a slot assignment that
/// is right for frame N is wrong for frame N+1 unless the toggle advanced
/// exactly as C's did, so this struct must be threaded through the whole
/// sequence rather than rebuilt per picture.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PicDecisionCtx {
    /// C `ctx->lay0_toggle`.
    pub lay0_toggle: u8,
    /// C `ctx->lay1_toggle`.
    pub lay1_toggle: u8,
    /// C `ctx->lay2_toggle`.
    pub lay2_toggle: u8,
    /// C `ctx->dpb[REF_FRAMES]` — the shadow DPB.
    pub dpb: [DpbEntry; REF_FRAMES],
    /// C `ctx->last_long_base_pic`.
    pub last_long_base_pic: u64,
    /// C `ctx->sframe_poc` (0 when no S-frame is pending).
    pub sframe_poc: u64,
    /// C `ctx->cut_short_ra_mg`.
    pub cut_short_ra_mg: u8,
    /// C `ctx->transition_detected`.
    pub transition_detected: i32,
    /// C `ctx->list0_only`.
    pub list0_only: bool,
    /// C `ctx->mini_gop_length[]`.
    pub mini_gop_length: [u32; 8],
    /// C `ctx->mini_gop_start_index[]`.
    pub mini_gop_start_index: [u32; 8],
    /// C `ctx->mini_gop_end_index[]`.
    pub mini_gop_end_index: [u32; 8],
    /// C `ctx->mg_size`.
    pub mg_size: u32,
    /// C `ctx->sframe_due` — an S-frame is owed at the next base-layer frame
    /// (`SFRAME_NEAREST_BASE` in low delay only).
    pub sframe_due: bool,
    /// C `ctx->next_arf_is_s` — a decode-order insert deferred the switch to
    /// the NEXT base-layer frame.
    pub next_arf_is_s: bool,
    /// C `ctx->sframe_hier_lvls` — the pyramid depth the next mini-GOP will
    /// use, lowered so a scheduled S-frame lands on a base-layer picture.
    ///
    /// Trap (`pd_process.c:249` vs `:5407-5409`): the constructor's value is
    /// DEAD — the kernel overwrites it at `picture_number == 0`.
    pub sframe_hier_lvls: i32,
    /// C `ctx->sframe_last_arf`.
    pub sframe_last_arf: u64,
    /// C `ctx->key_poc` — the POC of the last key frame.
    pub key_poc: u64,
    /// C `ctx->ref_order_hint[REF_FRAMES]` — the shadow DPB's per-slot order
    /// hints, published into `PicParams::dpb_order_hint`.
    pub ref_order_hint: [u32; REF_FRAMES],
    /// C `ctx->pic_id_per_dpb_slot` — the application `pic_id` pinned into
    /// each DPB slot, or `None` for a slot the short-term allocator owns.
    /// C stores `0` for "no id"; [`core::num::NonZeroU32`] makes that
    /// sentinel unrepresentable (see [`crate::port_ref_mgmt`]).
    pub pic_id_per_dpb_slot: [Option<core::num::NonZeroU32>; REF_FRAMES],
    /// C `ctx->last_i_noise_levels_log1p_fp16` — the last I slice's luma
    /// noise in Q16 `log1p` form. `derive_tf_window_params` updates it from
    /// the fresh estimate or reuses it when `use_intra_for_noise_est` skips
    /// the estimate (`pd_process.c:3843-3848`).
    pub last_i_noise_levels_log1p_fp16: [i32; 3],
    /// C `ctx->filt_to_unfilt_diff` — the filtered-vs-unfiltered difference
    /// the last I slice's temporal filter measured, inherited by every
    /// following picture's `ref_pics_modulation` until the next I publishes
    /// a new one (`pd_process.c:5107/5122-5125`). `~0` before the first I
    /// slice runs — C's `pcs` ctor value (`resource_coordination_process.c:347`),
    /// and what the delayed-intra slot resets both sides to.
    pub filt_to_unfilt_diff: u32,
    /// C `ctx->tf_motion_direction` — the `-1`/`0`/`1` verdict `mctf_frame`
    /// publishes after filtering (`pd_process.c:4232-4238`; the port's
    /// [`tf_motion_direction`]).
    pub tf_motion_direction: i8,
    /// C `ctx->tf_pic_array` + `ctx->tf_pic_arr_cnt` — the low-delay TF ring
    /// `low_delay_store_tf_pictures` fills and `low_delay_release_tf_pictures`
    /// drains. Dead while low-delay TF is disabled, transcribed anyway.
    pub ld_tf_ring: LowDelayTfRing,
    /// C's `pd_ctx` scene-detector members — `ahd_running_avg`,
    /// `prev_picture_histogram`, `prev_average_intensity_per_region` and
    /// `reset_running_avg`, kept on [`SceneDetectState`].
    pub scene_detect: SceneDetectState,
}

impl PicDecisionCtx {
    /// The state C's `svt_aom_picture_decision_context_ctor` leaves
    /// (`pd_process.c:236-252`).
    ///
    /// The only field that differs from [`Default`] is `transition_detected`,
    /// which C initialises to **-1**, not 0. Nothing this module does reads it
    /// except `init_pic_settings`' `== 1` test, so the two are behaviourally
    /// identical here — it is reproduced so a later consumer that treats 0 as
    /// "no transition yet" does not inherit a value C never had.
    #[must_use]
    pub fn new() -> Self {
        Self {
            transition_detected: -1,
            // `~0`, C's pcs-ctor sentinel (`resource_coordination_process.c:347`).
            // The C context itself is malloc'd and only ever written at
            // `pd_process.c:5107`, which also writes `~0` — so `~0` is the
            // only value either side can carry before the first I slice.
            filt_to_unfilt_diff: u32::MAX,
            ..Self::default()
        }
    }
}

// ---------------------------------------------------------------------------
// Exported C symbols (tier 1 reachable)
// ---------------------------------------------------------------------------
