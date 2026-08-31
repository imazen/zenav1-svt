//! Reference bindings for `Codec/rc_process.c` — the rate-control rate model,
//! the qdelta-by-rate search and the boost helpers (lane `wp-ratecontrol`).
//!
//! Nearly all of this surface is **directly linkable**: `nm -g` on
//! `Bin/Release/libSvtAv1Enc.a` shows `svt_av1_rc_bits_per_mb`,
//! `svt_av1_get_cqp_kf_boost_from_r0`, `svt_av1_get_gfu_boost_from_r0_lap`,
//! `svt_av1_calculate_boost_bits` and every one of the file's const tables as
//! defined (`T`/`S`) symbols, so they are bound here with no C wrapper —
//! evidence tier 1 in `WORKING-ON-THIS.md` §4 with nothing transcribed in
//! between. Only `svt_av1_compute_qdelta_by_rate` (which takes a
//! `RATE_CONTROL*`) goes through `shims/rc_shims.c`.
//!
//! The const tables are exposed as functions rather than `pub static`s so a
//! caller cannot accidentally hold a reference into the C image; each read is
//! a copy out of the linked object.

use std::os::raw::c_int;

unsafe extern "C" {
    // ---- exported, bound with no shim ----
    fn svt_av1_rc_bits_per_mb(
        frame_type: c_int,
        qindex: c_int,
        correction_factor: f64,
        bit_depth: c_int,
        is_screen_content_type: c_int,
    ) -> c_int;
    fn svt_av1_get_cqp_kf_boost_from_r0(
        r0: f64,
        frames_to_key: c_int,
        input_resolution: c_int,
    ) -> c_int;
    fn svt_av1_get_gfu_boost_from_r0_lap(
        min_factor: f64,
        max_factor: f64,
        r0: f64,
        frames_to_key: c_int,
    ) -> c_int;
    fn svt_av1_calculate_boost_bits(
        frame_count: c_int,
        boost: c_int,
        total_group_bits: i64,
    ) -> c_int;

    // ---- exported const tables (rc_process.c:38-48) ----
    static svt_av1_non_base_qindex_weight_ref: [c_int; 6];
    static svt_av1_non_base_qindex_weight_wq: [c_int; 6];
    static svt_av1_tpl_hl_islice_div_factor: [f64; 6];
    static svt_av1_tpl_hl_base_frame_div_factor: [f64; 6];
    static svt_av1_r0_weight: [f64; 3];
    static svt_av1_rate_factor_deltas: [f64; 6];
    /// `rate_factor_level` is a plain C enum, i.e. `int`.
    static svt_av1_rate_factor_levels: [c_int; 7];

    // ---- via shims/rc_shims.c ----
    fn ref_rc_compute_qdelta_by_rate(
        best_quality: c_int,
        worst_quality: c_int,
        frame_type: c_int,
        qindex: c_int,
        rate_target_ratio: f64,
        bit_depth: c_int,
        is_screen_content_type: c_int,
    ) -> i32;
    fn ref_rc_convert_qindex_to_q(qindex: c_int, bit_depth: c_int) -> f64;
}

/// Reference `svt_av1_rc_bits_per_mb` (rc_process.c:255).
///
/// `frame_type` is `FrameType`: 0 == `KEY_FRAME`, 1 == `INTER_FRAME`.
/// `bit_depth` is the numeric `EbBitDepth` (8 / 10 / 12).
#[must_use]
pub fn rc_bits_per_mb(
    frame_type: i32,
    qindex: i32,
    correction_factor: f64,
    bit_depth: i32,
    is_screen_content_type: i32,
) -> i32 {
    unsafe {
        svt_av1_rc_bits_per_mb(
            frame_type,
            qindex,
            correction_factor,
            bit_depth,
            is_screen_content_type,
        )
    }
}

/// Reference `svt_av1_compute_qdelta_by_rate` (rc_process.c:290), driven
/// through `ref_rc_compute_qdelta_by_rate` with a stack `RATE_CONTROL` whose
/// only read fields (`best_quality`, `worst_quality`) are set from the args.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn compute_qdelta_by_rate(
    best_quality: i32,
    worst_quality: i32,
    frame_type: i32,
    qindex: i32,
    rate_target_ratio: f64,
    bit_depth: i32,
    is_screen_content_type: i32,
) -> i32 {
    unsafe {
        ref_rc_compute_qdelta_by_rate(
            best_quality,
            worst_quality,
            frame_type,
            qindex,
            rate_target_ratio,
            bit_depth,
            is_screen_content_type,
        )
    }
}

/// Reference `svt_av1_convert_qindex_to_q` (rc_process.c:185).
#[must_use]
pub fn convert_qindex_to_q(qindex: i32, bit_depth: i32) -> f64 {
    unsafe { ref_rc_convert_qindex_to_q(qindex, bit_depth) }
}

/// Reference `svt_av1_get_cqp_kf_boost_from_r0` (rc_process.c:230).
/// `input_resolution` is `ResolutionRange` (0 == 240p … 6 == 8K).
#[must_use]
pub fn get_cqp_kf_boost_from_r0(r0: f64, frames_to_key: i32, input_resolution: i32) -> i32 {
    unsafe { svt_av1_get_cqp_kf_boost_from_r0(r0, frames_to_key, input_resolution) }
}

/// Reference `svt_av1_get_gfu_boost_from_r0_lap` (rc_process.c:246).
#[must_use]
pub fn get_gfu_boost_from_r0_lap(
    min_factor: f64,
    max_factor: f64,
    r0: f64,
    frames_to_key: i32,
) -> i32 {
    unsafe { svt_av1_get_gfu_boost_from_r0_lap(min_factor, max_factor, r0, frames_to_key) }
}

/// Reference `svt_av1_calculate_boost_bits` (rc_process.c:638).
#[must_use]
pub fn calculate_boost_bits(frame_count: i32, boost: i32, total_group_bits: i64) -> i32 {
    unsafe { svt_av1_calculate_boost_bits(frame_count, boost, total_group_bits) }
}

/// Reference `svt_av1_non_base_qindex_weight_ref` (rc_process.c:38).
#[must_use]
pub fn non_base_qindex_weight_ref() -> [i32; 6] {
    unsafe { svt_av1_non_base_qindex_weight_ref }
}

/// Reference `svt_av1_non_base_qindex_weight_wq` (rc_process.c:40).
#[must_use]
pub fn non_base_qindex_weight_wq() -> [i32; 6] {
    unsafe { svt_av1_non_base_qindex_weight_wq }
}

/// Reference `svt_av1_tpl_hl_islice_div_factor` (rc_process.c:42).
#[must_use]
pub fn tpl_hl_islice_div_factor() -> [f64; 6] {
    unsafe { svt_av1_tpl_hl_islice_div_factor }
}

/// Reference `svt_av1_tpl_hl_base_frame_div_factor` (rc_process.c:43).
#[must_use]
pub fn tpl_hl_base_frame_div_factor() -> [f64; 6] {
    unsafe { svt_av1_tpl_hl_base_frame_div_factor }
}

/// Reference `svt_av1_r0_weight` (rc_process.c:45).
#[must_use]
pub fn r0_weight() -> [f64; 3] {
    unsafe { svt_av1_r0_weight }
}

/// Reference `svt_av1_rate_factor_deltas` (rc_process.c:299).
#[must_use]
pub fn rate_factor_deltas() -> [f64; 6] {
    unsafe { svt_av1_rate_factor_deltas }
}

/// Reference `svt_av1_rate_factor_levels` (rc_process.c:308), indexed by
/// `SvtAv1FrameUpdateType` and valued as `rate_factor_level`.
#[must_use]
pub fn rate_factor_levels() -> [i32; 7] {
    unsafe { svt_av1_rate_factor_levels }
}

// ---------------------------------------------------------------------------
// `svt_aom_set_rc_param` (pass2_strategy.c:906) and `svt_av1_rc_init`
// (rc_process.c:495) — both EXPORTED, both taking a `SequenceControlSet*`.
//
// The shim `calloc`s a real SequenceControlSet + EncodeContext per call and
// drives the real symbol; these structs are the flattened in/out parameter
// blocks. Field order MUST match `shims/rc_shims.c` exactly.
// ---------------------------------------------------------------------------

/// Flattened inputs to `svt_aom_set_rc_param` — the fields the C function
/// actually reads off `SequenceControlSet`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SetRcParamIn {
    pub first_pass_downsample: i32,
    pub max_input_luma_width: u32,
    pub max_input_luma_height: u32,
    pub encoder_bit_depth: i32,
    pub vbr_min_section_pct: i32,
    pub vbr_max_section_pct: i32,
    pub rate_control_mode: i32,
    pub min_qp_allowed: i32,
    pub max_qp_allowed: i32,
    pub gop_constraint_rc: i32,
    pub over_shoot_pct: i32,
    pub under_shoot_pct: i32,
    pub maximum_buffer_size_ms: i64,
    pub starting_buffer_level_ms: i64,
    pub optimal_buffer_level_ms: i64,
    pub max_intra_bitrate_pct: u32,
    pub max_inter_bitrate_pct: u32,
    pub sframe_dist: i32,
    pub sframe_mode: i32,
}

/// Flattened outputs of `svt_aom_set_rc_param`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SetRcParamOut {
    pub frame_width: i32,
    pub frame_height: i32,
    pub mb_rows: i32,
    pub mb_cols: i32,
    pub num_mbs: i32,
    pub bit_depth: i32,
    pub vbrmin_section: i32,
    pub vbrmax_section: i32,
    pub mode: i32,
    pub best_allowed_q: i32,
    pub worst_allowed_q: i32,
    pub over_shoot_pct: i32,
    pub under_shoot_pct: i32,
    pub maximum_buffer_size_ms: i64,
    pub starting_buffer_level_ms: i64,
    pub optimal_buffer_level_ms: i64,
    pub max_intra_bitrate_pct: u32,
    pub max_inter_bitrate_pct: u32,
    pub sframe_dist: i32,
    pub sframe_mode: i32,
}

/// Flattened inputs to `svt_av1_rc_init`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct RcInitIn {
    pub mode: i32,
    pub best_allowed_q: i32,
    pub worst_allowed_q: i32,
    pub starting_buffer_level: i64,
    pub avg_frame_bandwidth: i32,
    pub hierarchical_levels: i32,
    pub frame_rate_numerator: i32,
    pub frame_rate_denominator: i32,
}

/// Flattened outputs of `svt_av1_rc_init`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RcInitOut {
    pub avg_frame_qindex_key: i32,
    pub avg_frame_qindex_inter: i32,
    pub last_q_key: i32,
    pub last_q_inter: i32,
    pub buffer_level: i64,
    pub bits_off_target: i64,
    pub rolling_target_bits: i32,
    pub rolling_actual_bits: i32,
    pub total_actual_bits: i64,
    pub total_target_bits: i64,
    pub frames_since_key: i32,
    pub frames_since_cdf_update: i32,
    pub this_key_frame_forced: i32,
    pub rate_correction_factors: [f64; 7],
    pub baseline_gf_interval: i32,
    pub worst_quality: i32,
    pub best_quality: i32,
    pub cur_avg_base_me_dist: u32,
    pub prev_avg_base_me_dist: u32,
    pub avg_frame_low_motion: i32,
}

unsafe extern "C" {
    fn ref_rc_set_rc_param(input: *const SetRcParamIn, out: *mut SetRcParamOut);
    fn ref_rc_init(input: *const RcInitIn, out: *mut RcInitOut);
}

/// Drive the real `svt_aom_set_rc_param` on a per-call `SequenceControlSet`.
#[must_use]
pub fn set_rc_param(input: &SetRcParamIn) -> SetRcParamOut {
    let mut out = SetRcParamOut::default();
    unsafe { ref_rc_set_rc_param(input, &mut out) };
    out
}

/// Drive the real `svt_av1_rc_init` on a per-call `SequenceControlSet`.
///
/// SAFETY NOTE FOR CALLERS: when `mode != AOM_Q` the C function calls
/// `svt_av1_new_framerate` -> `av1_rc_update_framerate`, which reads further
/// `static_config` fields off the zeroed control set. `frame_rate_numerator` /
/// `frame_rate_denominator` are therefore exposed on [`RcInitIn`]; pass
/// non-zero values on any non-AOM_Q call.
#[must_use]
pub fn rc_init(input: &RcInitIn) -> RcInitOut {
    let mut out = RcInitOut::default();
    unsafe { ref_rc_init(input, &mut out) };
    out
}
