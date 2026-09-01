//! Port of the **stateful** half of `Codec/rc_vbr_cbr.c`: the `RATE_CONTROL`
//! model itself, the leaky-bucket target sizing, the rate-correction-factor
//! feedback loop and the qindex regulator built on top of them.
//!
//! [`crate::port_rc_vbr_cbr`] already ports the file's pure-scalar leaves (the
//! minq interpolation and its two reverse searches). Everything there takes
//! plain numbers. Everything HERE reads or writes `RATE_CONTROL` /
//! `RateControlCfg` / `PictureParentControlSet`, which is why it was left out
//! of that file and listed as missing in its header.
//!
//! **What this file replaces, and what it does not.** C threads the state
//! through three pointers — `ppcs->scs->enc_ctx->rc`, `->rc_cfg`, and the PPCS
//! itself — and guards two of the accesses with mutexes (`rc->rc_mutex`,
//! `enc_ctx->frame_updated_mutex`) because the C encoder runs rate control
//! concurrently with several pipeline stages. The port does NOT translate the
//! mutexes: the state is an owned [`RateControl`] passed as `&mut`, so the
//! aliasing C needs a lock to forbid is a compile error here. That is a
//! deliberate substitution of Rust's ownership for C's locking, not an
//! omission — the arithmetic under the lock is reproduced exactly.
//!
//! The three C pointers become three explicit parameters:
//! * [`RateControl`] — the `RATE_CONTROL` fields this file touches.
//! * [`RateControlCfg`] — `EncodeContext::rc_cfg` (encoder.h).
//! * [`FrameRc`] + [`SeqRc`] — the `PictureParentControlSet` and
//!   `SequenceControlSet` fields it reads. Both are narrow views over structs
//!   with hundreds of members; each field carries its C name so the mapping is
//!   checkable.
//!
//! **EVIDENCE.** `av1_rc_regulate_q`,
//! `av1_rc_update_rate_correction_factors` and
//! `calc_active_worst_quality_no_stats_cbr` survive in
//! `cbuild-static/.../rc_vbr_cbr.c.o` as local (`t`) symbols, so they are
//! reachable at **tier 1** through the `--globalize-symbol` promotion the
//! build script already performs for `pd_process.c`
//! (`docs/WORKING-ON-THIS.md` §4); `tests/c_parity_rc_vbr_cbr_state.rs` drives
//! them. The rest of this file — every function LLVM inlined away, and every
//! `void` state mutator — is **tier 4**: hand-derived vectors traced against
//! the C source, labelled as such on each test.
//!
//! **Preprocessor check** (trap #1 in `docs/WORKING-ON-THIS.md` §5):
//! `grep -c '#if' rc_vbr_cbr.c` is 1, and the single hit is
//! `#if DEBUG_RC_CAP_LOG` inside `rc_process.h`'s struct, not in this file's
//! bodies. No function here has a second `SVT_HDR_MODE` definition
//! (`grep -c 'SVT_HDR_MODE' rc_vbr_cbr.c` == 0), so every line read is a line
//! mainline compiles.

use crate::port_pass2_strategy::fclamp;
use crate::port_rc_process::{FrameUpdateType, INTER_FRAME, KEY_FRAME, rc_bits_per_mb};
use crate::rate_control::{compute_qdelta, convert_qindex_to_q};

// ---------------------------------------------------------------------------
// Constants (rc_process.h / definitions.h)
// ---------------------------------------------------------------------------

/// C `BPER_MB_NORMBITS` (rc_process.h:26) — bits-per-MB are carried in Q9.
pub const BPER_MB_NORMBITS: u32 = 9;
/// C `FRAME_OVERHEAD_BITS` (rc_process.h:28).
pub const FRAME_OVERHEAD_BITS: i32 = 200;
/// C `MIN_BPB_FACTOR` (rc_process.h:56).
pub const MIN_BPB_FACTOR: f64 = 0.005;
/// C `MAX_BPB_FACTOR` (rc_process.h:57). Note it is written `50` (an `int`) in
/// C and used in a `double` context, so it is 50.0 — not the 1.5 that libaom
/// uses for the same-named macro.
pub const MAX_BPB_FACTOR: f64 = 50.0;
/// C `MAX_GF_INTERVAL` (rc_process.h:35).
pub const MAX_GF_INTERVAL: i32 = 32;
/// C `DEFAULT_GF_BOOST_RT` (rc_vbr_cbr.c:362).
pub const DEFAULT_GF_BOOST_RT: i32 = 2000;
/// C `DEFAULT_KF_BOOST_RT` (rc_vbr_cbr.c:361).
pub const DEFAULT_KF_BOOST_RT: i32 = 2300;
/// C `MAX_TEMPORAL_LAYERS` (EbSvtAv1.h:59).
pub const MAX_TEMPORAL_LAYERS: usize = 6;
/// C `MAX_HIERARCHICAL_LEVEL` (EbSvtAv1Enc.h:34).
pub const MAX_HIERARCHICAL_LEVEL: usize = 6;

// ---------------------------------------------------------------------------
// Enums, replacing C's magic integers
// ---------------------------------------------------------------------------

/// C `enum aom_rc_mode` (encoder.h:32). The discriminants are C's, because
/// `rc_cfg.mode` is compared against them across several files.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum AomRcMode {
    /// `AOM_VBR` — variable bitrate, the two-pass / lookahead path.
    #[default]
    Vbr = 0,
    /// `AOM_CBR` — constant bitrate, the low-delay path.
    Cbr = 1,
    /// `AOM_Q` — constant quality (CQP/CRF). This file's entry points are not
    /// reached in this mode; `svt_av1_rc_process_rate_allocation` is only
    /// called for VBR/CBR.
    Q = 2,
}

/// C `FrameType` (definitions.h:1605). Only [`FrameType::Key`] is ever tested
/// for by name in `rc_vbr_cbr.c`; the other three exist so a caller cannot
/// smuggle an out-of-range `int` in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum FrameType {
    #[default]
    Key = 0,
    Inter = 1,
    IntraOnly = 2,
    Switch = 3,
}

impl FrameType {
    /// C `frm_hdr.frame_type == KEY_FRAME`.
    #[must_use]
    pub fn is_key(self) -> bool {
        matches!(self, FrameType::Key)
    }

    /// C `frame_is_intra_only` (entropy_coding.h:60) — KEY **or** INTRA_ONLY.
    /// Reading this as "is a key frame" is the easy mistake; `INTRA_ONLY_FRAME`
    /// also selects the intra arm of every branch in this file.
    #[must_use]
    pub fn is_intra_only(self) -> bool {
        matches!(self, FrameType::Key | FrameType::IntraOnly)
    }

    /// The `FrameType` argument the rate model
    /// ([`crate::port_rc_process::rc_bits_per_mb`]) takes. C passes
    /// `frm_hdr.frame_type` straight through and the model only compares it
    /// against `KEY_FRAME`, so INTRA_ONLY / SWITCH take the inter arm there.
    #[must_use]
    pub fn as_rate_model_arg(self) -> i32 {
        if self.is_key() {
            KEY_FRAME
        } else {
            INTER_FRAME
        }
    }
}

/// C `RESIZE_STATE` (definitions.h) — the dynamic-resize ladder position.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum ResizeState {
    #[default]
    Orig = 0,
    ThreeQuarter = 1,
    OneHalf = 2,
}

/// C `RESIZE_ACTION` — the transition chosen by
/// [`dynamic_resize_one_pass_cbr`](super::port_rc_vbr_cbr_qpick). Modelled as
/// an `Option`-free enum with an explicit "no change" arm because C's
/// `NO_RESIZE` is load-bearing (it is the "do not touch the RC" signal).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ResizeAction {
    #[default]
    NoResize,
    DownOneHalf,
    DownThreeFour,
    UpThreeFour,
    UpOrig,
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// C `RateControlCfg` (encoder.h:44) — the subset `rc_vbr_cbr.c` reads.
///
/// `max_intra_bitrate_pct` / `max_inter_bitrate_pct` / `min_cr` are
/// `unsigned int` in C and are kept `u32` here **on purpose**: they multiply a
/// signed `avg_frame_bandwidth`, and C's usual arithmetic conversions make the
/// whole product unsigned. See [`clamp_iframe_target_size`].
#[derive(Clone, Copy, Debug)]
pub struct RateControlCfg {
    /// C `rc_cfg.mode`.
    pub mode: AomRcMode,
    /// C `rc_cfg.starting_buffer_level_ms`.
    pub starting_buffer_level_ms: i64,
    /// C `rc_cfg.optimal_buffer_level_ms`.
    pub optimal_buffer_level_ms: i64,
    /// C `rc_cfg.maximum_buffer_size_ms`.
    pub maximum_buffer_size_ms: i64,
    /// C `rc_cfg.max_intra_bitrate_pct` (default 300).
    pub max_intra_bitrate_pct: u32,
    /// C `rc_cfg.max_inter_bitrate_pct` (default 0 == disabled).
    pub max_inter_bitrate_pct: u32,
    /// C `rc_cfg.min_cr` — target minimum compression ratio × 100.
    pub min_cr: u32,
    /// C `rc_cfg.under_shoot_pct`, clamped to `0..=100` by
    /// `svt_av1_verify_settings` (enc_settings.c:105).
    pub under_shoot_pct: i32,
    /// C `rc_cfg.over_shoot_pct`, clamped to `0..=100` (enc_settings.c:95).
    pub over_shoot_pct: i32,
}

impl Default for RateControlCfg {
    /// The values `svt_aom_set_rc_param` + `enc_handle.c:4602-4614` install for
    /// a default VBR configure: undershoot 25, overshoot 25, intra cap 300 %,
    /// no inter cap, no min-CR.
    fn default() -> Self {
        Self {
            mode: AomRcMode::Vbr,
            starting_buffer_level_ms: 0,
            optimal_buffer_level_ms: 0,
            maximum_buffer_size_ms: 0,
            max_intra_bitrate_pct: 300,
            max_inter_bitrate_pct: 0,
            min_cr: 0,
            under_shoot_pct: 25,
            over_shoot_pct: 25,
        }
    }
}

/// C `RATE_CONTROL` (rc_process.h:108) — the fields `rc_vbr_cbr.c` reads or
/// writes. The C struct has ~50 members; the ones left out here are owned by
/// `rc_process.c` / `pass2_strategy.c` / `rc_rtc_cbr.c` and are named in those
/// ports.
///
/// Widths match C exactly: the buffer quantities are `int64_t` because they
/// hold bits over a whole GOP, the per-frame bandwidths are `int`, and
/// `cur/prev_avg_base_me_dist` are `uint32_t` (their comparison against 0 in
/// `adjust_q_cbr` is therefore a "is it set" test, not a sign test).
#[derive(Clone, Debug, Default)]
pub struct RateControl {
    /// `rc->avg_frame_bandwidth` — average target frame size, in bits.
    pub avg_frame_bandwidth: i32,
    /// `rc->prev_avg_frame_bandwidth`.
    pub prev_avg_frame_bandwidth: i32,
    /// `rc->max_frame_bandwidth` — the burst cap for one frame.
    pub max_frame_bandwidth: i32,
    /// `rc->starting_buffer_level`.
    pub starting_buffer_level: i64,
    /// `rc->optimal_buffer_level`.
    pub optimal_buffer_level: i64,
    /// `rc->maximum_buffer_size`.
    pub maximum_buffer_size: i64,
    /// `rc->buffer_level`.
    pub buffer_level: i64,
    /// `rc->bits_off_target`.
    pub bits_off_target: i64,
    /// `rc->vbr_bits_off_target`.
    pub vbr_bits_off_target: i64,
    /// `rc->vbr_bits_off_target_fast`.
    pub vbr_bits_off_target_fast: i64,
    /// `rc->rate_correction_factors[]`, indexed either by
    /// `rate_factor_level` (non-VBR) or by `temporal_layer_index + 1` (VBR) —
    /// which is why it is 7 wide (`MAX_TEMPORAL_LAYERS + 1`) rather than
    /// `RATE_FACTOR_LEVELS`.
    pub rate_correction_factors: [f64; MAX_TEMPORAL_LAYERS + 1],
    /// `rc->avg_frame_qindex[FRAME_TYPES]`, indexed `[KEY_FRAME, INTER_FRAME]`.
    pub avg_frame_qindex: [i32; 2],
    /// `rc->last_q[FRAME_TYPES]` — the *q value* (not qindex) last used.
    pub last_q: [i32; 2],
    /// `rc->worst_quality` — the highest qindex RC may pick.
    pub worst_quality: i32,
    /// `rc->best_quality` — the lowest qindex RC may pick.
    pub best_quality: i32,
    /// `rc->active_worst_quality`.
    pub active_worst_quality: i32,
    /// `rc->active_best_quality[MAX_ARF_LAYERS + 1]`.
    pub active_best_quality: [i32; 7],
    /// `rc->last_boosted_qindex`.
    pub last_boosted_qindex: i32,
    /// `rc->kf_boost`.
    pub kf_boost: i32,
    /// `rc->gfu_boost`.
    pub gfu_boost: i32,
    /// `rc->arf_q`.
    pub arf_q: i32,
    /// `rc->baseline_gf_interval`.
    pub baseline_gf_interval: i32,
    /// `rc->constrained_gf_group`.
    pub constrained_gf_group: i32,
    /// `rc->frames_to_key`.
    pub frames_to_key: i32,
    /// `rc->frames_since_key`.
    pub frames_since_key: i32,
    /// `rc->frames_since_cdf_update`.
    pub frames_since_cdf_update: i32,
    /// `rc->this_key_frame_forced`.
    pub this_key_frame_forced: bool,
    /// `rc->rolling_target_bits`.
    pub rolling_target_bits: i32,
    /// `rc->rolling_actual_bits`.
    pub rolling_actual_bits: i32,
    /// `rc->total_actual_bits`.
    pub total_actual_bits: i64,
    /// `rc->total_target_bits`.
    pub total_target_bits: i64,
    /// `rc->avg_frame_low_motion`.
    pub avg_frame_low_motion: i32,
    /// `rc->rate_error_estimate` — the VBR drift, in percent, clamped to
    /// `[-100, 100]`. Written by `pass2_strategy.c`'s two-pass post-encode
    /// update and read by its `calculate_active_worst_quality`.
    pub rate_error_estimate: i32,
    /// `rc->gf_group_bits` — the bit budget for the current GF group.
    pub gf_group_bits: i64,
    /// `rc->rc_1_frame` / `rc_2_frame`: -1 undershot, +1 overshoot, 0 unset.
    pub rc_1_frame: i32,
    /// See [`RateControl::rc_1_frame`].
    pub rc_2_frame: i32,
    /// `rc->q_1_frame` — the qindex used one frame ago.
    pub q_1_frame: i32,
    /// `rc->q_2_frame` — two frames ago.
    pub q_2_frame: i32,
    /// `rc->percent_refresh_adjustment` (cyclic refresh).
    pub percent_refresh_adjustment: i32,
    /// `rc->rate_ratio_qdelta_adjustment` (cyclic refresh).
    pub rate_ratio_qdelta_adjustment: f64,
    /// `rc->resize_state`.
    pub resize_state: ResizeState,
    /// `rc->resize_avg_qp`.
    pub resize_avg_qp: i32,
    /// `rc->resize_buffer_underflow`.
    pub resize_buffer_underflow: i32,
    /// `rc->resize_count`.
    pub resize_count: i32,
    /// `rc->cur_avg_base_me_dist`.
    pub cur_avg_base_me_dist: u32,
    /// `rc->prev_avg_base_me_dist`.
    pub prev_avg_base_me_dist: u32,
    /// `enc_ctx->frame_updated`. It lives on the `EncodeContext` in C behind
    /// `frame_updated_mutex`, but it is read and written only by this file's
    /// functions, so it is carried with the rest of the RC state.
    pub frame_updated: i32,
}

/// C `CyclicRefresh` (pcs.h:654) — the fields the rate loop reads.
#[derive(Clone, Copy, Debug, Default)]
pub struct CyclicRefresh {
    /// `cr->percent_refresh`.
    pub percent_refresh: i32,
    /// `cr->max_qdelta_perc`.
    pub max_qdelta_perc: i32,
    /// `cr->sb_start`.
    pub sb_start: u32,
    /// `cr->sb_end`.
    pub sb_end: u32,
    /// `cr->rate_ratio_qdelta`.
    pub rate_ratio_qdelta: f64,
    /// `cr->rate_ratio_qdelta_seg2`.
    pub rate_ratio_qdelta_seg2: f64,
    /// `cr->apply_cyclic_refresh`.
    pub apply_cyclic_refresh: bool,
    /// `cr->rate_boost_fac`.
    pub rate_boost_fac: i32,
    /// `cr->qindex_delta[3]`.
    pub qindex_delta: [i32; 3],
    /// `cr->actual_num_seg1_sbs`.
    pub actual_num_seg1_sbs: i32,
    /// `cr->actual_num_seg2_sbs`.
    pub actual_num_seg2_sbs: i32,
}

/// C `SequenceControlSet` fields read by `rc_vbr_cbr.c`.
#[derive(Clone, Copy, Debug)]
pub struct SeqRc {
    /// `scs->static_config.encoder_bit_depth` (8 / 10 / 12).
    pub encoder_bit_depth: u8,
    /// `scs->static_config.target_bit_rate`, in bits per second.
    pub target_bit_rate: i64,
    /// `scs->new_framerate`.
    pub new_framerate: f64,
    /// `scs->static_config.intra_period_length`. `-1` means "infinite".
    pub intra_period_length: i32,
    /// `scs->static_config.hierarchical_levels`.
    pub hierarchical_levels: u8,
    /// `scs->max_input_luma_width`.
    pub max_input_luma_width: i32,
    /// `scs->max_input_luma_height`.
    pub max_input_luma_height: i32,
    /// `scs->static_config.min_qp_allowed` in the CLI 0..63 domain.
    pub min_qp_allowed: u8,
    /// `scs->static_config.max_qp_allowed` in the CLI 0..63 domain.
    pub max_qp_allowed: u8,
    /// `scs->static_config.rtc`.
    pub rtc: bool,
    /// `scs->static_config.gop_constraint_rc`.
    pub gop_constraint_rc: bool,
    /// `scs->is_short_clip`.
    pub is_short_clip: bool,
    /// `scs->super_block_size`.
    pub super_block_size: u16,
    /// `scs->sb_total_count`.
    pub sb_total_count: u16,
    /// `scs->seq_header.seq_profile` (0 MAIN / 1 HIGH / 2 PROFESSIONAL).
    pub seq_profile: u8,
    /// `enc_ctx->recode_tolerance`, asserted `<= 100` by C.
    pub recode_tolerance: i32,
    /// `scs->input_resolution` (`ResolutionRange`), the key-frame boost's
    /// resolution bucket.
    pub input_resolution: i32,
    /// `scs->static_config.qp_scale_compress_strength_unused` — the MAINLINE
    /// spelling. The `SVT_HDR_MODE` fork replaces this `uint8_t` with a
    /// `double qp_scale_compress_strength` and a different formula; see
    /// `port_rc_vbr_cbr_qpick::qp_scale_weight_mainline`.
    pub qp_scale_compress_strength: u8,
    /// `scs->passes`.
    pub passes: i32,
}

impl Default for SeqRc {
    fn default() -> Self {
        Self {
            encoder_bit_depth: 8,
            target_bit_rate: 0,
            new_framerate: 60.0,
            intra_period_length: -1,
            hierarchical_levels: 0,
            max_input_luma_width: 0,
            max_input_luma_height: 0,
            min_qp_allowed: 1,
            max_qp_allowed: 63,
            rtc: false,
            gop_constraint_rc: false,
            is_short_clip: false,
            super_block_size: 64,
            sb_total_count: 0,
            seq_profile: 0,
            recode_tolerance: 25,
            input_resolution: 0,
            qp_scale_compress_strength: 0,
            passes: 1,
        }
    }
}

/// C `PictureParentControlSet` fields read or written by `rc_vbr_cbr.c`.
///
/// `this_frame_target`, `base_frame_target`, `projected_frame_size`,
/// `top_index` and `bottom_index` are OUTPUTS in C (written back onto the
/// PPCS); they are ordinary fields here and the functions that set them take
/// `&mut FrameRc`.
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameRc {
    /// `ppcs->picture_number`.
    pub picture_number: u64,
    /// `ppcs->frame_offset`.
    pub frame_offset: u64,
    /// `ppcs->frm_hdr.frame_type`.
    pub frame_type: FrameType,
    /// `ppcs->frm_hdr.showable_frame`.
    pub showable_frame: bool,
    /// `ppcs->frm_hdr.quantization_params.base_q_idx`.
    pub base_q_idx: i32,
    /// `ppcs->update_type`.
    pub update_type: FrameUpdateType,
    /// `ppcs->is_overlay`.
    pub is_overlay: bool,
    /// `ppcs->is_ref`.
    pub is_ref: bool,
    /// `ppcs->sc_class1` — screen-content class 1.
    pub sc_class1: bool,
    /// `ppcs->scene_change_flag`.
    pub scene_change_flag: bool,
    /// `ppcs->transition_present`. `int8_t` in C; `-1` means "not computed"
    /// and is NOT the same as `0` at the one site that reads it
    /// (`adjust_active_best_and_worst_quality_org` tests `!= 1`).
    pub transition_present: i8,
    /// `ppcs->temporal_layer_index`.
    pub temporal_layer_index: u8,
    /// `ppcs->hierarchical_levels`.
    pub hierarchical_levels: u8,
    /// `ppcs->layer_depth`.
    pub layer_depth: i32,
    /// `ppcs->av1_cm->frm_size.frame_width`.
    pub frame_width: i32,
    /// `ppcs->av1_cm->frm_size.frame_height`.
    pub frame_height: i32,
    /// `ppcs->av1_cm->frm_size.superres_upscaled_width`.
    pub superres_upscaled_width: i32,
    /// `ppcs->frame_width` — the *source* width, which differs from
    /// `av1_cm->frm_size.frame_width` while a dynamic resize is pending.
    pub src_frame_width: i32,
    /// `ppcs->frame_height`, see [`FrameRc::src_frame_width`].
    pub src_frame_height: i32,
    /// `ppcs->this_frame_target` (bits).
    pub this_frame_target: i32,
    /// `ppcs->base_frame_target` (bits).
    pub base_frame_target: i32,
    /// `ppcs->projected_frame_size` (bits).
    pub projected_frame_size: i32,
    /// `ppcs->max_frame_size` (bits) — the capped-CRF budget.
    pub max_frame_size: i32,
    /// `ppcs->top_index`.
    pub top_index: i32,
    /// `ppcs->bottom_index`.
    pub bottom_index: i32,
    /// `ppcs->loop_count` — recode iteration.
    pub loop_count: i32,
    /// `ppcs->b64_total_count` (`uint16_t` in C).
    pub b64_total_count: u16,
    /// `ppcs->r0`.
    pub r0: f64,
    /// `ppcs->rc_reset_flag`.
    pub rc_reset_flag: bool,
}

impl FrameRc {
    /// C `frame_is_intra_only(ppcs)`.
    #[must_use]
    pub fn is_intra_only(&self) -> bool {
        self.frame_type.is_intra_only()
    }

    /// C `svt_aom_frame_is_kf_gf_arf` (rc_process.c:56).
    #[must_use]
    pub fn is_kf_gf_arf(&self) -> bool {
        self.is_intra_only()
            || self.update_type == FrameUpdateType::ArfUpdate
            || self.update_type == FrameUpdateType::GfUpdate
    }

    /// C's repeated `update_type == GF_UPDATE || update_type == ARF_UPDATE`.
    #[must_use]
    pub fn is_gf_or_arf(&self) -> bool {
        matches!(
            self.update_type,
            FrameUpdateType::GfUpdate | FrameUpdateType::ArfUpdate
        )
    }

    /// C's `is_intrnl_arf` / `is_intrl_arf_boost`.
    #[must_use]
    pub fn is_internal_arf(&self) -> bool {
        self.update_type == FrameUpdateType::IntnlArfUpdate
    }
}

// ---------------------------------------------------------------------------
// qindex clamping
// ---------------------------------------------------------------------------

/// C `quantizer_to_qindex` (`rc_process.c`), re-exported from the CQP port so
/// there is exactly one copy of the table in the crate.
#[must_use]
fn quantizer_to_qindex(qp: u8) -> i32 {
    i32::from(crate::rate_control::qp_to_qindex(qp))
}

/// C `clamp_qindex` (rc_vbr_cbr.c:21). Marked `NOINLINE` in C.
///
/// C returns `uint8_t` from a `CLIP3` over `int`, so a negative `qindex`
/// clamps up to `qmin` before the narrowing — the narrowing itself never
/// truncates. The port returns `i32` because every caller immediately stores
/// it back into an `int` qindex; the value is identical.
///
/// Written as C's `CLIP3` (utility.h:101) rather than `i32::clamp`: `clamp`
/// PANICS when `min > max`, while `CLIP3` returns `max`. `min_qp_allowed <=
/// max_qp_allowed` is enforced by `svt_av1_verify_settings`, so the case does
/// not arise — but a panic on a misconfigured caller would be a behaviour
/// change this function does not own.
#[must_use]
pub fn clamp_qindex(scs: &SeqRc, qindex: i32) -> i32 {
    let qmin = quantizer_to_qindex(scs.min_qp_allowed);
    let qmax = quantizer_to_qindex(scs.max_qp_allowed);
    if qindex < qmin {
        qmin
    } else if qindex > qmax {
        qmax
    } else {
        qindex
    }
}

// ---------------------------------------------------------------------------
// Buffer sizing and target-size derivation
// ---------------------------------------------------------------------------

/// C `set_rc_buffer_sizes` (rc_vbr_cbr.c:591).
///
/// A zero `optimal_buffer_level_ms` / `maximum_buffer_size_ms` means "one
/// eighth of a second of bitrate", NOT zero — the `== 0` arms are the defaults,
/// not a disable.
pub fn set_rc_buffer_sizes(rc: &mut RateControl, cfg: &RateControlCfg, scs: &SeqRc) {
    let bandwidth = scs.target_bit_rate;
    rc.starting_buffer_level = cfg.starting_buffer_level_ms * bandwidth / 1000;
    rc.optimal_buffer_level = if cfg.optimal_buffer_level_ms == 0 {
        bandwidth / 8
    } else {
        cfg.optimal_buffer_level_ms * bandwidth / 1000
    };
    rc.maximum_buffer_size = if cfg.maximum_buffer_size_ms == 0 {
        bandwidth / 8
    } else {
        cfg.maximum_buffer_size_ms * bandwidth / 1000
    };
}

/// C `av1_calc_pframe_target_size_one_pass_cbr` (rc_vbr_cbr.c:71).
///
/// Two integer-arithmetic details that a natural Rust rewrite would get wrong:
///
/// * `AOMMIN(diff / one_pct_bits, rc_cfg->under_shoot_pct)` compares an
///   `int64_t` against an `int`, so the *int* is promoted and the min is taken
///   in 64-bit before the `(int)` cast. Done in `i64` here for that reason.
/// * `target * pct / 200` is `int` × `int` in C. With both `under_shoot_pct`
///   and `over_shoot_pct` clamped to `0..=100` by `svt_av1_verify_settings`
///   (enc_settings.c:95/105) the product overflows `int` only above
///   ~21 Mbit *per frame*; `wrapping_mul` reproduces what clang and gcc
///   actually emit there rather than panicking in debug.
#[must_use]
pub fn calc_pframe_target_size_one_pass_cbr(rc: &RateControl, cfg: &RateControlCfg) -> i32 {
    let diff = rc.optimal_buffer_level - rc.buffer_level;
    let one_pct_bits = 1 + rc.optimal_buffer_level / 100;
    let min_frame_target = (rc.avg_frame_bandwidth >> 4).max(FRAME_OVERHEAD_BITS);
    let mut target = rc.avg_frame_bandwidth;

    if diff > 0 {
        // Lower the target bandwidth for this frame.
        let pct_low = (diff / one_pct_bits).min(i64::from(cfg.under_shoot_pct)) as i32;
        target -= target.wrapping_mul(pct_low) / 200;
    } else if diff < 0 {
        // Increase the target bandwidth for this frame.
        let pct_high = (-diff / one_pct_bits).min(i64::from(cfg.over_shoot_pct)) as i32;
        target += target.wrapping_mul(pct_high) / 200;
    }
    if cfg.max_inter_bitrate_pct != 0 {
        // C: `int max_rate = rc->avg_frame_bandwidth * rc_cfg->max_inter_bitrate_pct / 100;`
        // `max_inter_bitrate_pct` is `unsigned int`, so the signed
        // `avg_frame_bandwidth` is converted to `unsigned` and the whole
        // expression is UNSIGNED. That is not a nicety: at 300 % of a 10 Mbit
        // frame the signed product would overflow, while the unsigned one is
        // well-defined and the `/ 100` brings it back in range.
        let max_rate =
            (rc.avg_frame_bandwidth as u32).wrapping_mul(cfg.max_inter_bitrate_pct) / 100;
        target = target.min(max_rate as i32);
    }
    min_frame_target.max(target)
}

/// C `svt_aom_reset_update_frame_target` (rc_vbr_cbr.c:97).
pub fn reset_update_frame_target(rc: &mut RateControl, cfg: &RateControlCfg, frame: &mut FrameRc) {
    rc.buffer_level = rc.optimal_buffer_level;
    rc.bits_off_target = rc.optimal_buffer_level;
    frame.this_frame_target = calc_pframe_target_size_one_pass_cbr(rc, cfg);
}

/// C `av1_rc_clamp_iframe_target_size` (rc_vbr_cbr.c:518).
///
/// Same unsigned-promotion note as [`calc_pframe_target_size_one_pass_cbr`]:
/// `max_intra_bitrate_pct` is `unsigned int` and defaults to **300**, so the
/// signed product would overflow at high bitrates and the unsigned one does
/// not.
#[must_use]
pub fn clamp_iframe_target_size(rc: &RateControl, cfg: &RateControlCfg, mut target: i32) -> i32 {
    if cfg.max_intra_bitrate_pct != 0 {
        let max_rate =
            (rc.avg_frame_bandwidth as u32).wrapping_mul(cfg.max_intra_bitrate_pct) / 100;
        target = target.min(max_rate as i32);
    }
    if target > rc.max_frame_bandwidth {
        target = rc.max_frame_bandwidth;
    }
    target
}

/// C `av1_calc_iframe_target_size_one_pass_cbr` (rc_vbr_cbr.c:534).
///
/// The first picture gets half the *starting* buffer level; every later key
/// frame gets `(16 + kf_boost) / 16` of the average frame bandwidth, with the
/// boost ramped in over the first half-second after a key frame.
#[must_use]
pub fn calc_iframe_target_size_one_pass_cbr(
    rc: &RateControl,
    cfg: &RateControlCfg,
    scs: &SeqRc,
    frame: &FrameRc,
) -> i32 {
    let target = if frame.picture_number == 0 {
        // C: `((rc->starting_buffer_level / 2) > INT_MAX) ? INT_MAX : (int)(...)`
        let half = rc.starting_buffer_level / 2;
        if half > i64::from(i32::MAX) {
            i32::MAX
        } else {
            half as i32
        }
    } else {
        let framerate = scs.new_framerate;
        // C `AOMMAX(kf_boost, (int)(2 * framerate - 16))` with kf_boost = 32.
        let mut kf_boost = 32.max((2.0 * framerate - 16.0) as i32);
        if f64::from(rc.frames_since_key) < framerate / 2.0 {
            // C: `(int)(kf_boost * rc->frames_since_key / (framerate / 2))`.
            // `kf_boost * rc->frames_since_key` is INT arithmetic; only the
            // division is floating point, so the product truncates first.
            kf_boost =
                (f64::from(kf_boost.wrapping_mul(rc.frames_since_key)) / (framerate / 2.0)) as i32;
        }
        ((16 + kf_boost).wrapping_mul(rc.avg_frame_bandwidth)) >> 4
    };
    clamp_iframe_target_size(rc, cfg, target)
}

/// C `set_gf_interval_update_onepass_rt` (rc_vbr_cbr.c:363).
///
/// Returns whether a GF update happened, and mutates the interval/boost when
/// it did. C returns `int`; a `bool` is the same value with the meaning
/// visible.
pub fn set_gf_interval_update_onepass_rt(rc: &mut RateControl, frame: &FrameRc) -> bool {
    if !frame.frame_offset.is_multiple_of(MAX_GF_INTERVAL as u64) {
        return false;
    }
    rc.baseline_gf_interval = MAX_GF_INTERVAL;
    if rc.baseline_gf_interval > rc.frames_to_key {
        rc.baseline_gf_interval = rc.frames_to_key;
    }
    rc.gfu_boost = DEFAULT_GF_BOOST_RT;
    rc.constrained_gf_group = i32::from(rc.baseline_gf_interval >= rc.frames_to_key);
    true
}

/// C `update_buffer_level` (rc_vbr_cbr.c:1474) — the leaky bucket.
///
/// A non-showable frame is pure overhead: it costs bits and earns no drain
/// interval. Note the clip is applied to `bits_off_target` and `buffer_level`
/// is then *assigned* from it, so the two are equal after every call.
pub fn update_buffer_level(rc: &mut RateControl, frame: &FrameRc, encoded_frame_size: i32) {
    if !frame.showable_frame {
        rc.bits_off_target -= i64::from(encoded_frame_size);
    } else {
        rc.bits_off_target += i64::from(rc.avg_frame_bandwidth - encoded_frame_size);
    }
    rc.bits_off_target = rc.bits_off_target.min(rc.maximum_buffer_size);
    rc.buffer_level = rc.bits_off_target;
}

/// C `av1_get_compression_ratio` (rc_vbr_cbr.c:1689).
///
/// `encoded_frame_size` is a `size_t` **in bytes** at the one call site
/// (`recode_loop_update_q` passes `projected_frame_size >> 3`). The
/// `> 129 ? - 128 : 1` step removes a fixed header allowance and floors the
/// denominator at 1 so the division cannot blow up.
#[must_use]
pub fn get_compression_ratio(scs: &SeqRc, frame: &FrameRc, encoded_frame_size: u64) -> f64 {
    let luma_pic_size = frame.superres_upscaled_width * frame.frame_height;
    // C `MAIN_PROFILE == 0`, `HIGH_PROFILE == 1`, anything else PROFESSIONAL.
    let pic_size_profile_factor = match scs.seq_profile {
        0 => 15,
        1 => 30,
        _ => 36,
    };
    let encoded = if encoded_frame_size > 129 {
        encoded_frame_size - 128
    } else {
        1
    };
    let uncompressed_frame_size = (luma_pic_size.wrapping_mul(pic_size_profile_factor)) >> 3;
    f64::from(uncompressed_frame_size) / encoded as f64
}

/// The under/over-shoot window `recode_loop_test` compares against.
/// C returns it through two out-params; a named pair reads better and cannot
/// be swapped at a call site.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameSizeBounds {
    /// C `*frame_under_shoot_limit`.
    pub under_shoot: i32,
    /// C `*frame_over_shoot_limit`.
    pub over_shoot: i32,
}

/// C `av1_rc_compute_frame_size_bounds` (rc_vbr_cbr.c:1700).
///
/// In `AOM_Q` the bounds are relative to `ppcs->max_frame_size` (capped CRF)
/// and the under-shoot limit is 0 on the first pass of the recode loop; in
/// VBR/CBR they are relative to the frame target and capped by
/// `max_frame_bandwidth`.
#[must_use]
pub fn compute_frame_size_bounds(
    rc: &RateControl,
    cfg: &RateControlCfg,
    scs: &SeqRc,
    frame: &FrameRc,
    frame_target: i32,
) -> FrameSizeBounds {
    if cfg.mode == AomRcMode::Q {
        let tolerance =
            100.max(i64::from(scs.recode_tolerance) * i64::from(frame.max_frame_size) / 100) as i32;
        FrameSizeBounds {
            under_shoot: if frame.loop_count != 0 {
                (frame.max_frame_size - tolerance).max(0)
            } else {
                0
            },
            // C `AOMMIN(ppcs->max_frame_size + tolerance, INT_MAX)`: the add is
            // plain `int`, and the `AOMMIN` against `INT_MAX` is therefore a
            // no-op that cannot undo an overflow. `wrapping_add` is what the
            // compiler emits; `saturating_add` would be a different function.
            over_shoot: frame.max_frame_size.wrapping_add(tolerance),
        }
    } else {
        debug_assert!(scs.recode_tolerance <= 100);
        let tolerance =
            100.max(i64::from(scs.recode_tolerance) * i64::from(frame_target) / 100) as i32;
        FrameSizeBounds {
            under_shoot: (frame_target - tolerance).max(0),
            over_shoot: frame_target
                .wrapping_add(tolerance)
                .min(rc.max_frame_bandwidth),
        }
    }
}

/// C `av1_estimate_bits_at_q` (rc_vbr_cbr.c:1348).
#[must_use]
pub fn estimate_bits_at_q(
    frame_type: FrameType,
    q: i32,
    mbs: i32,
    correction_factor: f64,
    bit_depth: u8,
    is_screen_content_type: bool,
) -> i32 {
    let bpm = rc_bits_per_mb(
        frame_type.as_rate_model_arg(),
        q,
        correction_factor,
        bit_depth,
        is_screen_content_type,
    );
    // C: `AOMMAX(FRAME_OVERHEAD_BITS, (int)((uint64_t)bpm * mbs) >> BPER_MB_NORMBITS)`.
    // The multiply is UNSIGNED 64-bit and the `(int)` cast happens BEFORE the
    // shift, so a product above 2^31 becomes a negative `int` and is then
    // arithmetic-shifted. Reproduced exactly rather than "fixed".
    let product = (bpm as u64).wrapping_mul(mbs as u64);
    let as_int = product as i32;
    FRAME_OVERHEAD_BITS.max(as_int >> BPER_MB_NORMBITS)
}

/// C `av1_find_qindex` (rc_vbr_cbr.c:1772).
///
/// The smallest qindex in `[best, worst]` whose q value is `>= desired_q`.
/// Distinct from [`crate::port_rc_process::find_qindex_by_rate`], which
/// searches on modelled *bits*, and from
/// [`crate::port_rc_vbr_cbr::find_qindex`], which is the same search expressed
/// over the same helper — this one is kept beside its caller because the two
/// files' copies could drift.
#[must_use]
pub fn find_qindex(desired_q: f64, bit_depth: u8, best_qindex: i32, worst_qindex: i32) -> i32 {
    debug_assert!(best_qindex <= worst_qindex);
    let mut low = best_qindex;
    let mut high = worst_qindex;
    while low < high {
        let mid = (low + high) >> 1;
        if convert_qindex_to_q(mid, bit_depth) < desired_q {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    low
}

// ---------------------------------------------------------------------------
// Rate correction factors
// ---------------------------------------------------------------------------

/// Which slot of `rc->rate_correction_factors` this frame uses.
///
/// C computes the index inline in BOTH [`get_rate_correction_factor`] and
/// [`set_rate_correction_factor`], with the two copies required to agree — a
/// getter/setter pair that silently disagreed would corrupt the loop with no
/// symptom other than bad rate control. Factoring the selection out makes the
/// agreement structural.
#[must_use]
fn rate_correction_factor_index(cfg: &RateControlCfg, frame: &FrameRc) -> usize {
    if cfg.mode == AomRcMode::Vbr {
        // C `rate_factor_level rf_lvl = frame_type == KEY_FRAME ? 0 : temporal_layer_index + 1`.
        // Note this is NOT a `rate_factor_level` value despite the type: it is
        // a temporal-layer index shifted by one, which is why the array is
        // `MAX_TEMPORAL_LAYERS + 1` wide rather than `RATE_FACTOR_LEVELS`.
        if frame.frame_type.is_key() {
            0
        } else {
            usize::from(frame.temporal_layer_index) + 1
        }
    } else if frame.frame_type.is_key() {
        5 // KF_STD
    } else if frame.is_gf_or_arf() && !frame.is_overlay && cfg.mode != AomRcMode::Cbr {
        4 // GF_ARF_STD
    } else {
        0 // INTER_NORMAL
    }
}

/// C `get_rate_correction_factor` (rc_vbr_cbr.c:256).
///
/// `width`/`height` are the dimensions the *rate* is being computed for, which
/// during a dynamic resize differ from the frame's own coded size; the ratio
/// rescales the stored factor into that domain.
#[must_use]
pub fn get_rate_correction_factor(
    rc: &RateControl,
    cfg: &RateControlCfg,
    frame: &FrameRc,
    width: i32,
    height: i32,
) -> f64 {
    let idx = rate_correction_factor_index(cfg, frame);
    let mut rcf = rc.rate_correction_factors[idx];
    rcf *= f64::from(frame.frame_width * frame.frame_height) / f64::from(width * height);
    fclamp(rcf, MIN_BPB_FACTOR, MAX_BPB_FACTOR)
}

/// C `set_rate_correction_factor` (rc_vbr_cbr.c:280) — the inverse rescale of
/// [`get_rate_correction_factor`], then the same clamp, then the store.
pub fn set_rate_correction_factor(
    rc: &mut RateControl,
    cfg: &RateControlCfg,
    frame: &FrameRc,
    mut factor: f64,
    width: i32,
    height: i32,
) {
    // Normalize RCF to account for the size-dependent scaling factor.
    factor /= f64::from(frame.frame_width * frame.frame_height) / f64::from(width * height);
    factor = fclamp(factor, MIN_BPB_FACTOR, MAX_BPB_FACTOR);
    let idx = rate_correction_factor_index(cfg, frame);
    rc.rate_correction_factors[idx] = factor;
}

/// C `get_bits_per_mb` (rc_vbr_cbr.c:208).
#[must_use]
pub fn get_bits_per_mb(scs: &SeqRc, frame: &FrameRc, correction_factor: f64, q: i32) -> i32 {
    rc_bits_per_mb(
        frame.frame_type.as_rate_model_arg(),
        q,
        correction_factor,
        scs.encoder_bit_depth,
        frame.sc_class1,
    )
}

/// C `find_closest_qindex_by_rate` (rc_vbr_cbr.c:217).
///
/// Unlike `rc_process.c`'s `find_qindex_by_rate`, which returns the first
/// qindex at or under the target rate, this one then looks at `q - 1` too and
/// returns whichever of the two lands *closer* to the desired rate.
///
/// C's `INT_MAX` sentinel for "no candidate" is replaced by `Option`: the two
/// `curr_bit_diff == INT_MAX` paths mean "the search hit `worst_qindex` and
/// still overshot" and "there is no `q - 1`", and both make the comparison
/// `<=` against `INT_MAX` trivially true or false in ways that are easy to
/// misread as arithmetic. `None` compares as greater than every `Some`, which
/// is exactly `INT_MAX`'s role here.
#[must_use]
pub fn find_closest_qindex_by_rate(
    desired_bits_per_mb: i32,
    scs: &SeqRc,
    frame: &FrameRc,
    correction_factor: f64,
    best_qindex: i32,
    worst_qindex: i32,
) -> i32 {
    debug_assert!(best_qindex <= worst_qindex);
    let mut low = best_qindex;
    let mut high = worst_qindex;
    while low < high {
        let mid = (low + high) >> 1;
        if get_bits_per_mb(scs, frame, correction_factor, mid) > desired_bits_per_mb {
            low = mid + 1;
        } else {
            high = mid;
        }
    }

    let curr_q = low;
    let curr_bits_per_mb = get_bits_per_mb(scs, frame, correction_factor, curr_q);
    let curr_bit_diff =
        (curr_bits_per_mb <= desired_bits_per_mb).then(|| desired_bits_per_mb - curr_bits_per_mb);

    let prev_q = curr_q - 1;
    let prev_bit_diff = if curr_bit_diff.is_none() || curr_q == best_qindex {
        None
    } else {
        Some(get_bits_per_mb(scs, frame, correction_factor, prev_q) - desired_bits_per_mb)
    };

    // C: `(curr_bit_diff <= prev_bit_diff) ? curr_q : prev_q`, with INT_MAX
    // standing in for "absent". `Option`'s derived ordering puts `None` above
    // every `Some`, so this is the same predicate.
    match (curr_bit_diff, prev_bit_diff) {
        (Some(c), Some(p)) if c > p => prev_q,
        (Some(_), None) | (Some(_), Some(_)) => curr_q,
        // curr absent: `INT_MAX <= prev` only when prev is also absent.
        (None, None) => curr_q,
        (None, Some(_)) => prev_q,
    }
}

/// C `max_delta_per_layer` (rc_vbr_cbr.c:165), indexed
/// `[hierarchical_levels][temporal_layer_index]`.
///
/// **The trailing zeros are real.** C declares
/// `[MAX_HIERARCHICAL_LEVEL][MAX_TEMPORAL_LAYERS]` and initialises rows of
/// 1..6 entries, so the unwritten tail of each row is zero-filled by the
/// standard. A frame whose `temporal_layer_index` exceeds its
/// `hierarchical_levels` therefore gets `max_delta == 0`, and
/// `adjust_q_cbr`'s `max_delta_down = AOMMIN(0, AOMMAX(1, ...)) == 0` pins the
/// qindex to the previous frame's. Writing the rows short would have been a
/// silent behaviour change.
pub const MAX_DELTA_PER_LAYER: [[i32; MAX_TEMPORAL_LAYERS]; MAX_HIERARCHICAL_LEVEL] = [
    [60, 0, 0, 0, 0, 0],
    [60, 5, 0, 0, 0, 0],
    [60, 20, 2, 0, 0, 0],
    [60, 20, 10, 2, 0, 0],
    [60, 20, 10, 5, 2, 0],
    [60, 30, 20, 10, 5, 2],
];

/// C `adjust_q_cbr` (rc_vbr_cbr.c:168).
///
/// Two clamps: a content-change nudge downwards when the base-layer ME
/// distortion is falling and the buffer is healthy, and a hard limit on how
/// far q may drop below the previous frame's (anti-resonance).
#[must_use]
pub fn adjust_q_cbr(rc: &RateControl, scs: &SeqRc, frame: &FrameRc, mut q: i32) -> i32 {
    let max_delta = MAX_DELTA_PER_LAYER[usize::from(frame.hierarchical_levels)]
        [usize::from(frame.temporal_layer_index)];
    let max_delta_down = if frame.sc_class1 {
        max_delta.min(1.max(rc.q_1_frame / 2))
    } else {
        max_delta.min(1.max(rc.q_1_frame / 3))
    };
    // C compares an `int` `abs(...)` against a `double`, so the left side is
    // converted to `double`; kept in `f64` for that reason.
    let change_avg_frame_bandwidth =
        f64::from((rc.avg_frame_bandwidth - rc.prev_avg_frame_bandwidth).abs())
            > 0.1 * f64::from(rc.avg_frame_bandwidth);
    // If resolution changes or avg_frame_bandwidth significantly changed,
    // then set this flag to indicate change in target bits per macroblock.
    let change_target_bits_mb = change_avg_frame_bandwidth;

    if !frame.frame_type.is_key() && rc.frames_since_key > 1 && !change_target_bits_mb {
        // Adjust Q based on source content change.
        if frame.temporal_layer_index == 0
            && rc.prev_avg_base_me_dist > 0
            && rc.frames_since_key > 5
            && rc.cur_avg_base_me_dist > 0
        {
            let delta =
                f64::from(rc.cur_avg_base_me_dist) / f64::from(rc.prev_avg_base_me_dist) - 1.0;
            // Push Q downwards if content change is decreasing and the buffer
            // is at least a quarter of optimal, and only from a high q.
            if delta < 0.0
                && rc.buffer_level > (rc.optimal_buffer_level >> 2)
                && q > (rc.worst_quality >> 1)
            {
                let bit_depth = scs.encoder_bit_depth;
                let q_adj_factor = 1.0 + 0.5 * (4.0 * delta).tanh();
                let q_val = convert_qindex_to_q(q, bit_depth);
                q += compute_qdelta(q_val, q_val * q_adj_factor, bit_depth);
            }
        }
        // Make sure q is between oscillating Qs to prevent resonance.
        if rc.q_1_frame - q > max_delta_down {
            q = rc.q_1_frame - max_delta_down;
        }
    }
    q.min(rc.worst_quality).max(rc.best_quality)
}

/// C `av1_rc_regulate_q` (rc_vbr_cbr.c:307) — the qindex that the rate model
/// says will hit `ppcs->this_frame_target`.
///
/// `((uint64_t)this_frame_target << 9) / MBs` is C's own widening: the shift
/// is done in 64-bit so a large target cannot lose its top bits, and the
/// result is then narrowed by a `(int)` cast. A negative `this_frame_target`
/// would become a huge `uint64_t` in C; that is reproduced (`as u64`) rather
/// than guarded, because the callers never produce one and a guard would be an
/// unverifiable behaviour change.
#[must_use]
pub fn regulate_q(
    rc: &RateControl,
    cfg: &RateControlCfg,
    scs: &SeqRc,
    frame: &FrameRc,
    active_best_quality: i32,
    active_worst_quality: i32,
    width: i32,
    height: i32,
) -> i32 {
    let mbs = ((width + 15) / 16) * ((height + 15) / 16);
    let correction_factor = get_rate_correction_factor(rc, cfg, frame, width, height);
    let target_bits_per_mb =
        (((frame.this_frame_target as u64) << BPER_MB_NORMBITS) / (mbs as u64)) as i32;

    let q = find_closest_qindex_by_rate(
        target_bits_per_mb,
        scs,
        frame,
        correction_factor,
        active_best_quality,
        active_worst_quality,
    );
    if cfg.mode == AomRcMode::Cbr {
        return adjust_q_cbr(rc, scs, frame, q);
    }
    q
}

/// C `av1_rc_update_rate_correction_factors` (rc_vbr_cbr.c:1354) — the
/// feedback half of the loop: compare what the model predicted for the qindex
/// that was actually used against what the frame actually cost, and move the
/// stored correction factor a damped fraction of the way.
///
/// Overlay frames return early and update nothing.
pub fn update_rate_correction_factors(
    rc: &mut RateControl,
    cfg: &RateControlCfg,
    scs: &SeqRc,
    frame: &FrameRc,
    cr: &CyclicRefresh,
    width: i32,
    height: i32,
) {
    // Do not update the rate factors for arf overlay frames.
    if frame.is_overlay {
        return;
    }

    let frame_type = frame.frame_type;
    let bit_depth = scs.encoder_bit_depth;
    let base_q_idx = frame.base_q_idx;
    let mut rate_correction_factor = get_rate_correction_factor(rc, cfg, frame, width, height);
    let mbs = ((width + 15) / 16) * ((height + 15) / 16);

    // Work out how big we would have expected the frame to be at this Q given
    // the current correction factor. Stay in double to avoid int overflow.
    let projected_size_based_on_q = if cr.apply_cyclic_refresh {
        let weight_segment1 = f64::from(cr.actual_num_seg1_sbs) / f64::from(frame.b64_total_count);
        let weight_segment2 = f64::from(cr.actual_num_seg2_sbs) / f64::from(frame.b64_total_count);
        let at = |q: i32| {
            f64::from(estimate_bits_at_q(
                frame_type,
                q,
                mbs,
                rate_correction_factor,
                bit_depth,
                frame.sc_class1,
            ))
        };
        ((1.0 - weight_segment1 - weight_segment2) * at(base_q_idx)
            + weight_segment1 * at(base_q_idx + cr.qindex_delta[1])
            + weight_segment2 * at(base_q_idx + cr.qindex_delta[2]))
        .round() as i32
    } else {
        estimate_bits_at_q(
            frame_type,
            base_q_idx,
            mbs,
            rate_correction_factor,
            bit_depth,
            frame.sc_class1,
        )
    };

    // Work out a size correction factor, as a percentage.
    let mut correction_factor = 100_i32;
    if projected_size_based_on_q > FRAME_OVERHEAD_BITS {
        correction_factor = (100 * i64::from(frame.projected_frame_size)
            / i64::from(projected_size_based_on_q)) as i32;
    }
    // Clamp correction factor to prevent anything too extreme.
    correction_factor = correction_factor.max(25);
    rc.q_2_frame = rc.q_1_frame;
    rc.q_1_frame = base_q_idx;
    rc.rc_2_frame = rc.rc_1_frame;
    rc.rc_1_frame = if correction_factor > 110 {
        -1
    } else if correction_factor < 90 {
        1
    } else {
        0
    };

    // Decide how heavily to dampen the adjustment.
    // `correction_factor > 0` is always true after the `.max(25)` above; the
    // `else` arms are dead in C too and are kept so the two files stay
    // line-comparable (WORKING-ON-THIS.md §7).
    let adjustment_limit = if cfg.mode == AomRcMode::Cbr {
        if correction_factor > 0 {
            let l = (0.01 * f64::from(correction_factor)).log10().abs().min(0.5);
            if frame.sc_class1 {
                0.25 + 0.5 * l
            } else {
                0.25 + 0.75 * l
            }
        } else {
            0.75
        }
    } else if correction_factor > 0 {
        0.25 + 0.5 * (0.01 * f64::from(correction_factor)).log10().abs().min(1.0)
    } else {
        0.75
    };

    // Adjustment to delta Q and number of blocks updated in cyclic refresh
    // based on over/undershoot of the target in the current frame.
    if cr.apply_cyclic_refresh {
        if correction_factor > 125 {
            rc.percent_refresh_adjustment = (rc.percent_refresh_adjustment - 1).max(-5);
            // C `AOMMAX(x - 0.05, -0.0)` is `a > b ? a : b`, so an `x - 0.05`
            // of exactly `-0.0` or `0.0` yields `-0.0` (the `>` is false).
            // `f64::max` is documented to return EITHER zero in that tie, so
            // the ternary is written out rather than delegated.
            let adj = rc.rate_ratio_qdelta_adjustment - 0.05;
            rc.rate_ratio_qdelta_adjustment = if adj > -0.0 { adj } else { -0.0 };
        } else if correction_factor < 50 {
            rc.percent_refresh_adjustment = (rc.percent_refresh_adjustment + 1).min(5);
            let adj = rc.rate_ratio_qdelta_adjustment + 0.05;
            rc.rate_ratio_qdelta_adjustment = if adj < 0.25 { adj } else { 0.25 };
        }
    }

    if correction_factor > 101 {
        // We are not already at the worst allowable quality.
        correction_factor =
            (100.0 + (f64::from(correction_factor) - 100.0) * adjustment_limit) as i32;
        rate_correction_factor = rate_correction_factor * f64::from(correction_factor) / 100.0;
        if rate_correction_factor > MAX_BPB_FACTOR {
            rate_correction_factor = MAX_BPB_FACTOR;
        }
    } else if correction_factor < 99 {
        // We are not already at the best allowable quality.
        let mut tmp_corr_fac = 100.0 / f64::from(correction_factor);
        tmp_corr_fac = 1.0 + ((tmp_corr_fac - 1.0) * adjustment_limit);
        tmp_corr_fac = 1.0 / tmp_corr_fac;
        correction_factor = (100.0 * tmp_corr_fac) as i32;
        rate_correction_factor = rate_correction_factor * f64::from(correction_factor) / 100.0;
        if rate_correction_factor < MIN_BPB_FACTOR {
            rate_correction_factor = MIN_BPB_FACTOR;
        }
    }

    set_rate_correction_factor(rc, cfg, frame, rate_correction_factor, width, height);
}

/// C `recode_loop_test` (rc_vbr_cbr.c:1755).
///
/// `recode_loop` is `EncodeContext::recode_loop`; the two thresholds C names
/// are `ALLOW_RECODE_KFMAXBW` and `ALLOW_RECODE`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
#[repr(i32)]
pub enum RecodeLoop {
    /// `DISALLOW_RECODE` — never recode.
    #[default]
    Disallow = 0,
    /// `ALLOW_RECODE_KFMAXBW` — recode KF/ARF/GF, or any frame over the max
    /// bandwidth.
    AllowKfMaxBw = 1,
    /// `ALLOW_RECODE_KFARFGF`.
    AllowKfArfGf = 2,
    /// `ALLOW_RECODE` — recode any frame.
    Allow = 3,
    /// `ALLOW_RECODE_DEFAULT` — the "not yet resolved" sentinel
    /// (definitions.h:328). It is 4, so it satisfies
    /// `>= ALLOW_RECODE_KFMAXBW` but not `== ALLOW_RECODE`; the ordering of
    /// this enum reproduces that.
    AllowDefault = 4,
}

/// C `recode_loop_test` (rc_vbr_cbr.c:1755).
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn recode_loop_test(
    rc: &RateControl,
    recode_loop: RecodeLoop,
    frame: &FrameRc,
    high_limit: i32,
    low_limit: i32,
    q: i32,
    maxq: i32,
    minq: i32,
) -> bool {
    let frame_is_kfgfarf = frame.is_kf_gf_arf();
    if frame.projected_frame_size >= rc.max_frame_bandwidth
        || recode_loop == RecodeLoop::Allow
        || (frame_is_kfgfarf && recode_loop >= RecodeLoop::AllowKfMaxBw)
    {
        return (frame.projected_frame_size > high_limit && q < maxq)
            || (frame.projected_frame_size < low_limit && q > minq);
    }
    false
}

// ---------------------------------------------------------------------------
// CBR active-worst-quality, the recode regulators, and the resize reset
// ---------------------------------------------------------------------------

/// C `calc_active_worst_quality_no_stats_cbr` (rc_vbr_cbr.c:107).
///
/// Three buffer regimes, in order: above optimal (walk the worst quality
/// *down* from 5/4 of the ambient q, in proportion to buffer headroom), between
/// optimal and the critical level at 1/8 optimal (walk it *up* from ambient
/// toward `worst_quality`), and below critical (pin it at `worst_quality`).
/// A key frame short-circuits to `worst_quality` before any of that.
///
/// The `frame_updated < 4` arm folds the *key* frame's average qindex into the
/// ambient q for the first few frames after a key frame, so `frame_updated` is
/// load-bearing state, not a counter for logging. C reads it under
/// `enc_ctx->frame_updated_mutex`; the port owns it in [`RateControl`].
///
/// `buff_lvl_step` is `int64_t` in C and the two `adjustment` expressions are
/// `int64_t` divisions narrowed by an `(int)` cast, so the truncation happens
/// AFTER the division, not before.
#[must_use]
pub fn calc_active_worst_quality_no_stats_cbr(rc: &RateControl, frame: &FrameRc) -> i32 {
    if frame.frame_type.is_key() {
        return rc.worst_quality;
    }
    let critical_level = rc.optimal_buffer_level >> 3;
    let mut adjustment = 0_i32;

    let mut ambient_qp = if rc.frame_updated < 4 {
        rc.avg_frame_qindex[INTER_FRAME as usize].min(rc.avg_frame_qindex[KEY_FRAME as usize])
    } else {
        rc.avg_frame_qindex[INTER_FRAME as usize]
    };
    ambient_qp = rc.worst_quality.min(ambient_qp);

    let mut active_worst_quality;
    if rc.buffer_level > rc.optimal_buffer_level {
        active_worst_quality = rc.worst_quality.min(ambient_qp * 5 / 4);
        // Adjust down. Maximum limit for down adjustment, ~30%.
        let max_adjustment_down = active_worst_quality / 3;
        if max_adjustment_down != 0 {
            let buff_lvl_step =
                (rc.maximum_buffer_size - rc.optimal_buffer_level) / i64::from(max_adjustment_down);
            if buff_lvl_step != 0 {
                adjustment = ((rc.buffer_level - rc.optimal_buffer_level) / buff_lvl_step) as i32;
            }
            active_worst_quality -= adjustment;
        }
    } else if rc.buffer_level > critical_level {
        active_worst_quality = rc.worst_quality.min(ambient_qp);
        // Adjust up from ambient Q.
        if critical_level != 0 {
            let buff_lvl_step = rc.optimal_buffer_level - critical_level;
            if buff_lvl_step != 0 {
                adjustment = (i64::from(rc.worst_quality - ambient_qp)
                    * (rc.optimal_buffer_level - rc.buffer_level)
                    / buff_lvl_step) as i32;
            }
            active_worst_quality += adjustment;
        }
    } else {
        // Set to worst_quality if buffer is below critical level.
        active_worst_quality = rc.worst_quality;
    }
    active_worst_quality
}

/// C `get_regulated_q_overshoot` (rc_vbr_cbr.c:1719).
///
/// Re-runs the rate-correction feedback and re-regulates until the regulated q
/// is no longer below `q_low`, capped at ten retries. Each retry mutates the
/// stored correction factor, so the loop CONVERGES rather than spinning — the
/// retry cap is a guard, not the normal exit.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn get_regulated_q_overshoot(
    rc: &mut RateControl,
    cfg: &RateControlCfg,
    scs: &SeqRc,
    frame: &FrameRc,
    cr: &CyclicRefresh,
    q_low: i32,
    q_high: i32,
    top_index: i32,
    bottom_index: i32,
) -> i32 {
    let (width, height) = (frame.frame_width, frame.frame_height);
    update_rate_correction_factors(rc, cfg, scs, frame, cr, width, height);
    let mut q_regulated = regulate_q(
        rc,
        cfg,
        scs,
        frame,
        bottom_index,
        q_high.max(top_index),
        width,
        height,
    );
    let mut retries = 0;
    while q_regulated < q_low && retries < 10 {
        update_rate_correction_factors(rc, cfg, scs, frame, cr, width, height);
        q_regulated = regulate_q(
            rc,
            cfg,
            scs,
            frame,
            bottom_index,
            q_high.max(top_index),
            width,
            height,
        );
        retries += 1;
    }
    q_regulated
}

/// C `get_regulated_q_undershoot` (rc_vbr_cbr.c:1737). The mirror of
/// [`get_regulated_q_overshoot`]: it regulates against `top_index` directly
/// (no `max` with `q_high`) and retries while the result is ABOVE `q_high`.
#[must_use]
pub fn get_regulated_q_undershoot(
    rc: &mut RateControl,
    cfg: &RateControlCfg,
    scs: &SeqRc,
    frame: &FrameRc,
    cr: &CyclicRefresh,
    q_high: i32,
    top_index: i32,
    bottom_index: i32,
) -> i32 {
    let (width, height) = (frame.frame_width, frame.frame_height);
    update_rate_correction_factors(rc, cfg, scs, frame, cr, width, height);
    let mut q_regulated = regulate_q(rc, cfg, scs, frame, bottom_index, top_index, width, height);
    let mut retries = 0;
    while q_regulated > q_high && retries < 10 {
        update_rate_correction_factors(rc, cfg, scs, frame, cr, width, height);
        q_regulated = regulate_q(rc, cfg, scs, frame, bottom_index, top_index, width, height);
        retries += 1;
    }
    q_regulated
}

/// C `svt_av1_resize_reset_rc` (rc_vbr_cbr.c:324) — EXPORTED.
///
/// After a dynamic-resize transition the buffer is refilled to optimal, the
/// inter average qindex is pushed toward `worst_quality` in proportion to how
/// much the picture GREW, and then the rate-correction factor is nudged by
/// whether the newly regulated qindex looks too high or too low for the new
/// size. `tot_scale_change` is `new_area / prev_area`, so `> 1.0` means the
/// resize went UP.
///
/// The three nudges are separate `if`s, not `else if`s: at
/// `tot_scale_change >= 1.0` both of the second block's tests can fire, and
/// then the factor is multiplied by `0.8 * 1.5`. Reproduced as written.
#[allow(clippy::too_many_arguments)]
pub fn resize_reset_rc(
    rc: &mut RateControl,
    cfg: &RateControlCfg,
    scs: &SeqRc,
    frame: &mut FrameRc,
    resize_width: i32,
    resize_height: i32,
    prev_width: i32,
    prev_height: i32,
) {
    let tot_scale_change = f64::from(resize_width.wrapping_mul(resize_height))
        / f64::from(prev_width.wrapping_mul(prev_height));
    // Reset buffer level to optimal, update target size.
    reset_update_frame_target(rc, cfg, frame);
    if tot_scale_change > 4.0 {
        rc.avg_frame_qindex[INTER_FRAME as usize] = rc.worst_quality;
    } else if tot_scale_change > 1.0 {
        rc.avg_frame_qindex[INTER_FRAME as usize] =
            (rc.avg_frame_qindex[INTER_FRAME as usize] + rc.worst_quality) >> 1;
    }
    let active_worst_quality = calc_active_worst_quality_no_stats_cbr(rc, frame);
    let qindex = regulate_q(
        rc,
        cfg,
        scs,
        frame,
        rc.best_quality,
        active_worst_quality,
        resize_width,
        resize_height,
    );
    // Resized DOWN and the projected q is near the worst allowed: the frame
    // can probably afford a lower q, so relax the correction factor.
    if tot_scale_change < 1.0 && qindex > 90 * rc.worst_quality / 100 {
        rc.rate_correction_factors[0] *= 0.85;
    }
    if tot_scale_change >= 1.0 {
        if tot_scale_change < 4.0 && qindex > 130 * rc.last_q[INTER_FRAME as usize] / 100 {
            rc.rate_correction_factors[0] *= 0.8;
        }
        if qindex <= 120 * rc.last_q[INTER_FRAME as usize] / 100 {
            rc.rate_correction_factors[0] *= 1.5;
        }
    }
}
