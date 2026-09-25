//! The `EncodePipeline` ↔ ported rate-control driver — the C-shaped wiring
//! `svt_av1_rc_process_input` (rc_process.c) and
//! `rc_process_packetization_feedback` (packetization→RC feedback) provide.
//!
//! What C does once per picture under VBR/CBR:
//!
//! 1. `svt_av1_rc_process_rate_allocation` — init buffers on frame 0, TPL
//!    boost when enabled, then `one_pass_rt_rate_alloc` (CBR) or the
//!    two-pass stat/target pair (VBR).
//! 2. `svt_av1_rc_calc_qindex_rate_control` — pick `base_q_idx` (CBR also
//!    arms cyclic refresh via `svt_aom_cyclic_refresh_setup`, `rc_aq.c`).
//! 3. `svt_av1_rc_init_sb_qindex` — CBR either runs
//!    `cyclic_sb_qp_assignment` (`delta_q_present = 1`, per-SB qindexes)
//!    or pins every SB to `base_q_idx`; CBR is mutually exclusive with the
//!    variance/TPL AQ arms there (rc_aq.c:874-883).
//! 4. After packetization, `svt_av1_rc_postencode_update` +
//!    `svt_aom_update_rc_counts` consume the coded bit count and the
//!    zero-MV area percentage.
//!
//! The encoder-side state (`RATE_CONTROL`, `RateControlCfg`, the `scs` RC
//! subset, `CyclicRefresh`, the interval/resize/two-pass state) is
//! [`RcVbrCbr`], built once from [`RcConfig`] the way
//! `svt_aom_set_rc_param` (pass2_strategy.c:907) and
//! `set_param_based_on_input` (enc_handle.c:3901) build C's.
//!
//! Envelope: C itself refuses CBR outside LOW_DELAY (enc_settings.c:157)
//! and VBR outside RANDOM_ACCESS (enc_settings.c:177). This module's first
//! consumer is LD+CBR; the VBR stat path (`process_rc_stat` /
//! `av1_set_target_rate` / `twopass_postencode_update`, pass2_strategy.c)
//! stays unwired until first-pass statistics exist.

use crate::port_rc_process::{self, RcInitInput, SliceType};
use crate::port_rc_vbr_cbr_qpick::{RefLists, RefPicRc, TwoPassRc};
use crate::port_rc_vbr_cbr_state::{
    AomRcMode, CyclicRefresh, FrameRc, FrameType, RateControl, RateControlCfg, SeqRc,
};
use crate::port_rc_vbr_cbr_update::{RcIntervalParams, ResizePendingParams, RtResizeMode};

/// The encoder-owned C rate-control state — `enc_ctx->rc`, `enc_ctx->rc_cfg`,
/// the `scs`/`frame_info` fields RC reads, `enc_ctx->rate_control_param`, the
/// cyclic-refresh block, and `twopass`. `Option` on the pipeline: built on
/// the first VBR/CBR frame, never populated under CQP/CRF.
#[derive(Clone, Debug)]
pub struct RcVbrCbr {
    /// C `enc_ctx->rc` (`RATE_CONTROL`).
    pub rc: RateControl,
    /// C `enc_ctx->rc_cfg` (`RateControlCfg`).
    pub cfg: RateControlCfg,
    /// C `scs` + `frame_info` fields the ported RC functions read.
    pub scs: SeqRc,
    /// C `ppcs->cyclic_refresh` — per-sequence in the port because C's
    /// `cr_sb_end` band cursor persists across frames exactly like this does
    /// (`svt_aom_cyclic_refresh_setup` resets the per-frame fields).
    pub cr: CyclicRefresh,
    /// C `rc->cr_sb_end` — the rolling band position `cyclic_refresh_init`
    /// reads and advances.
    pub cr_sb_end: u32,
    /// C `ppcs->rate_control_param_ptr` (`RateControlIntervalParamContext`).
    pub rc_param: RcIntervalParams,
    /// C `enc_ctx->resize_pending_params`.
    pub resize_pending: ResizePendingParams,
    /// C `enc_ctx->twopass` — all zeros for one-pass CBR.
    pub twopass: TwoPassRc,
    /// C `enc_ctx->two_pass_cfg.vbrmax_section`
    /// (`static_config.vbr_max_section_pct`, default 2000).
    pub vbrmax_section: i32,
    /// C `frame_info.num_mbs` — 16x16 MB count on the padded input dims.
    pub num_mbs: i32,
}

/// C `scs->new_framerate`'s input — the CLI fps turned into the double
/// `set_rc_param`/`svt_av1_new_framerate` compute with.
fn frame_rate(framerate: f64) -> f64 {
    // C's app resolves `--fps` into numerator/denominator and the library
    // divides them here; the port's `RcConfig::framerate` is already that
    // quotient.
    framerate
}

/// The `RateControlCfg` half of `svt_aom_set_rc_param`
/// (pass2_strategy.c:926-948) plus the `enc_handle.c:4597-4614` static-config
/// resolution that feeds it.
///
/// `buffer_size_ms` is C's `maximum_buffer_size_ms`; starting/optimal take
/// C's defaults (600/600, enc_settings.c:1061-1062) clamped below `maximum`
/// by C's own verify_settings rule (:76-92).
fn rc_cfg(mode: AomRcMode, rc_config: &crate::rate_control::RcConfig) -> RateControlCfg {
    let maximum = i64::from(rc_config.buffer_size_ms);
    let default_level = 600i64.min((maximum - 1).max(0));
    RateControlCfg {
        mode,
        starting_buffer_level_ms: default_level,
        optimal_buffer_level_ms: default_level,
        maximum_buffer_size_ms: maximum,
        // enc_settings.c:1057-1058 defaults.
        max_intra_bitrate_pct: 300,
        max_inter_bitrate_pct: 0,
        min_cr: 0,
        // enc_handle.c:4604-4613 — under_shoot defaults to 50 for VBR, 25
        // otherwise; over_shoot to 25.
        under_shoot_pct: if mode == AomRcMode::Vbr { 50 } else { 25 },
        over_shoot_pct: 25,
    }
}

/// The `SeqRc` half — `scs`/`frame_info` fields the ported functions read,
/// populated the way `enc_handle.c` + `svt_aom_set_rc_param` leave them.
#[allow(clippy::too_many_arguments)]
fn seq_rc(
    rc_config: &crate::rate_control::RcConfig,
    mode: AomRcMode,
    width: u32,
    height: u32,
    bit_depth: u8,
    intra_period: i32,
    hierarchical_levels: u8,
    sb_size: u16,
    sb_total_count: u16,
) -> SeqRc {
    // C `scs->static_config.min_qp_allowed` (enc_handle.c:4597-4599):
    // lossless → MIN_QP_VALUE; MIN_QP_AUTO → 4 when a rate-control mode is
    // selected, MIN_QP_VALUE (0) otherwise; the port's CBR arm always has a
    // rate-control mode, so 4.
    let min_qp_allowed = if mode == AomRcMode::Q { 0 } else { 4 };
    // `max_qp_allowed` — the CLI default 63.
    let max_qp_allowed = 63;
    SeqRc {
        encoder_bit_depth: bit_depth,
        // `RcConfig::target_bitrate` is kbps; `static_config.target_bit_rate`
        // is bits per second.
        target_bit_rate: i64::from(rc_config.target_bitrate) * 1000,
        new_framerate: frame_rate(rc_config.framerate),
        intra_period_length: intra_period,
        hierarchical_levels,
        max_input_luma_width: width as i32,
        max_input_luma_height: height as i32,
        min_qp_allowed,
        max_qp_allowed,
        rtc: false,
        gop_constraint_rc: false,
        // `scs->is_short_clip = gop_constraint_rc ? 1 : 0`
        // (enc_handle.c:4360).
        is_short_clip: false,
        super_block_size: sb_size,
        sb_total_count,
        seq_profile: 0,
        // `enc_ctx->recode_tolerance = 25` (encode_context.c:121).
        recode_tolerance: 25,
        input_resolution: i32::from(
            crate::port_enc_mode_config::ResolutionRange::from_luma_area(width * height) as u8,
        ),
        qp_scale_compress_strength: 0,
        passes: 1,
    }
}

/// C `quantizer_to_qindex[qp]` — `set_rc_param`'s best/worst resolution.
fn quantizer_to_qindex(qp: i32) -> i32 {
    i32::from(crate::rate_control::QUANTIZER_TO_QINDEX[qp.clamp(0, 63) as usize])
}

impl RcVbrCbr {
    /// Build the encoder-side RC state for a CBR sequence — `RcMode::Cbr`,
    /// LOW_DELAY only (C refuses CBR under RANDOM_ACCESS at
    /// enc_settings.c:157, and the port enforces the same envelope at the
    /// config gate).
    #[allow(clippy::too_many_arguments)]
    pub fn new_cbr(
        rc_config: &crate::rate_control::RcConfig,
        width: u32,
        height: u32,
        bit_depth: u8,
        intra_period: i32,
        hierarchical_levels: u8,
        sb_size: u16,
        sb_cols: u16,
        sb_rows: u16,
    ) -> Self {
        let cfg = rc_cfg(AomRcMode::Cbr, rc_config);
        let sb_total_count = sb_cols * sb_rows;
        let mut scs = seq_rc(
            rc_config,
            AomRcMode::Cbr,
            width,
            height,
            bit_depth,
            intra_period,
            hierarchical_levels,
            sb_size,
            sb_total_count,
        );
        let num_mbs = ((width as i32 + 15) / 16) * ((height as i32 + 15) / 16);
        // C `svt_av1_new_framerate`'s `scs->new_framerate` half is applied
        // HERE, at build: `av1_rc_update_framerate`'s `rc->avg_frame_bandwidth`
        // side must NOT land early — `svt_av1_rc_init` seeds
        // `rolling_{target,actual}_bits` from the STALE bandwidth (0 on first
        // init) before this call rewrites it (rc_process.c:509-541). The
        // `RateControl` half lands inside the rc_init callback; see
        // [`Self::frame_bandwidth`].
        scs.new_framerate = port_rc_process::new_framerate(
            scs.target_bit_rate as u32,
            num_mbs,
            2000,
            frame_rate(scs.new_framerate),
        )
        .new_framerate;
        Self {
            rc: RateControl::default(),
            cfg,
            scs,
            cr: CyclicRefresh::default(),
            cr_sb_end: 0,
            rc_param: RcIntervalParams::default(),
            resize_pending: ResizePendingParams::default(),
            twopass: TwoPassRc::default(),
            // `static_config.vbr_max_section_pct` default (enc_settings.c:1053).
            vbrmax_section: 2000,
            num_mbs,
        }
    }

    /// `av1_rc_update_framerate` (pass2_strategy.c:880) — constant for a
    /// fixed-framerate sequence; recomputed on each call anyway because the
    /// inputs are cheap and `svt_av1_new_framerate` may fire again on a
    /// sequence-parameter change.
    #[must_use]
    pub fn frame_bandwidth(&self) -> port_rc_process::FramerateBandwidth {
        port_rc_process::new_framerate(
            self.scs.target_bit_rate as u32,
            self.num_mbs,
            self.vbrmax_section,
            self.scs.new_framerate,
        )
    }

    /// `quantizer_to_qindex[min_qp_allowed]` / `[max_qp_allowed]` — the
    /// `best_allowed_q`/`worst_allowed_q` pair `svt_aom_set_rc_param`
    /// resolves at :929-930.
    #[must_use]
    pub fn best_worst_allowed_q(&self) -> (i32, i32) {
        (
            quantizer_to_qindex(i32::from(self.scs.min_qp_allowed)),
            quantizer_to_qindex(i32::from(self.scs.max_qp_allowed)),
        )
    }
}

/// C `svt_av1_rc_init` (rc_process.c:495), the callback shape
/// [`crate::port_rc_vbr_cbr_update::process_rate_allocation`] expects: an
/// `RcInitInput` built from `rc`'s CURRENT values (C reads the same fields —
/// `rolling_*` seeds from the stale `avg_frame_bandwidth`), then the
/// `RcInitOutput` applied back, then the `mode != AOM_Q`
/// `svt_av1_new_framerate` tail (`bw`, precomputed by
/// [`RcVbrCbr::frame_bandwidth`] because the callback borrows only
/// `&mut RateControl`).
pub fn apply_rc_init(
    rc: &mut RateControl,
    mode: AomRcMode,
    best_allowed_q: i32,
    worst_allowed_q: i32,
    hierarchical_levels: i32,
    bw: port_rc_process::FramerateBandwidth,
) {
    let out = port_rc_process::rc_init(&RcInitInput {
        mode: mode as i32,
        best_allowed_q,
        worst_allowed_q,
        starting_buffer_level: rc.starting_buffer_level,
        avg_frame_bandwidth: rc.avg_frame_bandwidth,
        hierarchical_levels,
    });
    rc.avg_frame_qindex = [out.avg_frame_qindex_key, out.avg_frame_qindex_inter];
    rc.last_q = [out.last_q_key, out.last_q_inter];
    rc.buffer_level = out.buffer_level;
    rc.bits_off_target = out.bits_off_target;
    rc.rolling_target_bits = out.rolling_target_bits;
    rc.rolling_actual_bits = out.rolling_actual_bits;
    rc.total_actual_bits = out.total_actual_bits;
    rc.total_target_bits = out.total_target_bits;
    rc.frames_since_key = out.frames_since_key;
    rc.frames_since_cdf_update = out.frames_since_cdf_update;
    rc.this_key_frame_forced = out.this_key_frame_forced != 0;
    rc.rate_correction_factors = out.rate_correction_factors;
    rc.baseline_gf_interval = out.baseline_gf_interval;
    rc.worst_quality = out.worst_quality;
    rc.best_quality = out.best_quality;
    rc.cur_avg_base_me_dist = out.cur_avg_base_me_dist;
    rc.prev_avg_base_me_dist = out.prev_avg_base_me_dist;
    rc.avg_frame_low_motion = out.avg_frame_low_motion;
    // `svt_av1_new_framerate`'s `rc` half — after the seeds, exactly like C.
    rc.avg_frame_bandwidth = bw.avg_frame_bandwidth;
    rc.max_frame_bandwidth = bw.max_frame_bandwidth;
}

/// C `ppcs->update_type` — one [`FrameUpdateType`] now (the picture-struct
/// and RC spellings were unified); kept as a named pass-through for the
/// call sites that document this crossing.
#[must_use]
pub fn map_update_type(
    u: crate::port_picstruct::FrameUpdateType,
) -> port_rc_process::FrameUpdateType {
    u
}

/// The [`FrameRc`] the ported RC functions see for one picture — the PPCS
/// fields C reads, taken from the pipeline's picture-decision output.
#[must_use]
pub fn frame_rc(
    pic: Option<&crate::port_picstruct::PicParams>,
    is_key: bool,
    display_order: u64,
    frame_width: u32,
    frame_height: u32,
    upscaled_width: u32,
    b64_total_count: u16,
    hierarchical_levels: u8,
    sc_class1: bool,
) -> FrameRc {
    FrameRc {
        picture_number: display_order,
        frame_offset: pic.map_or(display_order, |p| p.frame_offset),
        frame_type: if is_key {
            FrameType::Key
        } else {
            FrameType::Inter
        },
        // `ppcs->frm_hdr.show_frame || ppcs->is_overlay` — `showable_frame`
        // in C is `!ppcs->is_overlay`; the port's LD envelope has no hidden
        // pictures.
        showable_frame: pic.map_or(true, |p| p.show_frame && !p.is_overlay),
        base_q_idx: 0,
        update_type: pic.map_or(port_rc_process::FrameUpdateType::Kf, |p| {
            map_update_type(p.update_type)
        }),
        is_overlay: pic.is_some_and(|p| p.is_overlay),
        is_ref: pic.map_or(true, |p| p.is_ref),
        sc_class1,
        // `pcs->scene_change_flag` — 0 while the scene-transition detector is
        // unported (open item).
        scene_change_flag: false,
        transition_present: pic.map_or(0, |p| p.transition_present as i8),
        temporal_layer_index: pic.map_or(0, |p| p.temporal_layer_index),
        hierarchical_levels: pic.map_or(hierarchical_levels, |p| p.hierarchical_levels),
        layer_depth: i32::from(pic.map_or(0, |p| p.layer_depth)),
        frame_width: frame_width as i32,
        frame_height: frame_height as i32,
        // `ppcs->enhanced_unscaled_pic->width` — the upscaled (render) width.
        superres_upscaled_width: upscaled_width as i32,
        src_frame_width: frame_width as i32,
        src_frame_height: frame_height as i32,
        this_frame_target: 0,
        base_frame_target: 0,
        projected_frame_size: 0,
        max_frame_size: 0,
        top_index: 0,
        bottom_index: 0,
        loop_count: 0,
        b64_total_count,
        r0: 0.0,
        rc_reset_flag: false,
    }
}

/// One DPB slot as a [`RefPicRc`] — `EbReferenceObject` + the PCS mirrors
/// (`ref_slice_type`/`ref_base_q_idx`/`ref_pic_r0`). For the LD envelope the
/// object and PCS fields carry the same values; both are kept because C's
/// different readers use different halves.
#[must_use]
pub fn ref_pic_rc(rf: &crate::picture::ReferenceFrame) -> RefPicRc {
    let slice_type = if rf.is_islice {
        SliceType::I
    } else {
        SliceType::B
    };
    RefPicRc {
        tmp_layer_idx: rf.temporal_layer,
        slice_type,
        pcs_slice_type: slice_type,
        ref_poc: rf.display_order,
        base_q_idx: rf.base_q_idx,
        pcs_r0: rf.r0,
        obj_r0: rf.r0,
    }
}

/// Build both reference lists for `rc_calc_qindex_rate_control` — C's
/// `pcs->ref_pic_ptr_array[2][4]`, resolved from `pic_decision`'s RPS against
/// the DPB. Slots with no picture map to nothing (C would deref the null —
/// `RefLists::get` refuses instead); `*_count_try` are the counts C loops to.
#[must_use]
pub fn ref_lists<'a>(
    pic: &crate::port_picstruct::PicParams,
    dpb: &crate::picture::DecodedPictureBuffer,
    l0: &'a [RefPicRc],
    l1: &'a [RefPicRc],
) -> RefLists<'a> {
    let _ = (pic, dpb);
    RefLists {
        l0,
        l1,
        l0_count_try: usize::from(pic.ref_list0_count_try),
        l1_count_try: usize::from(pic.ref_list1_count_try),
    }
}

/// C `svt_aom_is_pic_skipped(ppcs)` — a picture the packetization path does
/// not rate-count (`is_pic_skipped` is the alt-ref/overlay flag on LD; always
/// false in this envelope but kept as a named check for parity with the C
/// condition sites).
#[must_use]
pub fn is_pic_skipped(frame: &FrameRc) -> bool {
    frame.is_overlay
}

/// The CBR resize mode — `static_config.resize_mode == RESIZE_NONE` under the
/// port's refusal of dynamic resize; the enum still carries the other arms
/// because `one_pass_rt_rate_alloc` pattern-matches on it.
pub const RESIZE_MODE_NONE: RtResizeMode = RtResizeMode::None;
