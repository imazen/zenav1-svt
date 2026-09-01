//! Port of `Codec/rc_vbr_cbr.c`'s **post-encode and recode** half, plus the
//! rate-allocation entry point and the dynamic-resize decision.
//!
//! Chunk 1 ([`crate::port_rc_vbr_cbr_state`]) is the model, chunk 2
//! ([`crate::port_rc_vbr_cbr_qpick`]) is the qindex decision; this is what
//! happens AFTER a frame is coded — feed the actual size back into the rate
//! model, roll the averages forward, decide whether to recode at a different
//! q, and pick the next frame's target.
//!
//! **EVIDENCE.** Four of this group's functions are EXPORTED, so they are
//! reachable at **tier 1** with no symbol promotion, and
//! `tests/c_parity_rc_vbr_cbr_update.rs` drives them:
//! `svt_av1_rc_postencode_update`, `svt_av1_rc_postencode_update_gop_const`,
//! `recode_loop_update_q` and `svt_aom_dynamic_resize_decision`. Each drags
//! several inlined-away statics in with it — `update_buffer_level`,
//! `av1_rc_update_rate_correction_factors`, `av1_rc_compute_frame_size_bounds`,
//! `recode_loop_test`, `av1_find_qindex`, `av1_get_compression_ratio`,
//! `get_regulated_q_overshoot`/`_undershoot`, `dynamic_resize_one_pass_cbr`
//! and `set_gf_interval_update_onepass_rt`.
//!
//! Two are NOT pinned and say so:
//! `svt_aom_one_pass_rt_rate_alloc` is `static` and inlined (it is reached
//! only through `svt_av1_rc_process_rate_allocation`, whose other arm needs a
//! populated two-pass stats buffer this lane has not built yet), and
//! `restore_param` / `store_param` / `restore_two_pass_param` are stats-queue
//! plumbing — see their doc comments for which parts are translated and which
//! are replaced by design.

use crate::port_rc_process::{INTER_FRAME, KEY_FRAME};
use crate::port_rc_vbr_cbr::{get_gfu_q_tpl, get_kf_q_tpl};
use crate::port_rc_vbr_cbr_qpick::TplCtrlsRc;
use crate::port_rc_vbr_cbr_state::{
    self as st, AomRcMode, CyclicRefresh, FrameRc, FrameType, RateControl, RateControlCfg,
    RecodeLoop, ResizeState, SeqRc,
};
use crate::rate_control::convert_qindex_to_q;

/// C `AV1_PROB_COST_SHIFT` (md_rate_estimation.h:29).
pub const AV1_PROB_COST_SHIFT: u32 = 9;
/// C `SCALE_NUMERATOR` (definitions.h:1451).
pub const SCALE_NUMERATOR: u8 = 8;
/// C `SCALE_THREE_QUATER` (definitions.h:1457) — spelled as C spells it.
pub const SCALE_THREE_QUATER: u8 = 17;
/// C `SCALE_DENOMINATOR_MAX` (definitions.h:1456).
pub const SCALE_DENOMINATOR_MAX: u8 = 16;

/// C `RateControlIntervalParamContext` (encode_context.h:46) — the fields
/// `rc_vbr_cbr.c` reads or writes. This is the PER-GOP mirror of several
/// `RATE_CONTROL` fields; `gop_constraint_rc` decides which copy is
/// authoritative, which is why both exist.
#[derive(Clone, Copy, Debug, Default)]
pub struct RcIntervalParams {
    /// `first_poc`.
    pub first_poc: u64,
    /// `kf_group_bits`.
    pub kf_group_bits: i64,
    /// `kf_group_error_left`.
    pub kf_group_error_left: i64,
    /// `end_of_seq_seen`.
    pub end_of_seq_seen: bool,
    /// `vbr_bits_off_target`.
    pub vbr_bits_off_target: i64,
    /// `vbr_bits_off_target_fast`.
    pub vbr_bits_off_target_fast: i64,
    /// `rolling_target_bits`.
    pub rolling_target_bits: i32,
    /// `rolling_actual_bits`.
    pub rolling_actual_bits: i32,
    /// `rate_error_estimate`.
    pub rate_error_estimate: i32,
    /// `total_actual_bits`.
    pub total_actual_bits: i64,
    /// `total_target_bits`.
    pub total_target_bits: i64,
    /// `extend_minq`.
    pub extend_minq: i32,
    /// `extend_maxq`.
    pub extend_maxq: i32,
    /// `extend_minq_fast`.
    pub extend_minq_fast: i32,
}

/// C `ResizePendingParams` (rc_process.h:103) — what the dynamic-resize
/// decision publishes for picture decision to act on next frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResizePendingParams {
    pub resize_state: ResizeState,
    pub resize_denom: u8,
}

/// C `ROUND_POWER_OF_TWO(value, n)` for the 32-bit `int` sites in this file.
///
/// The rounding is `(v + (1 << (n-1))) >> n`, an ARITHMETIC shift, so a
/// negative value rounds toward negative infinity rather than toward zero.
/// Both call sites here feed it a sum of non-negative qindexes, but the shape
/// is kept because `>> 2` on a negative `int` is what C does and `/ 4` is not.
#[must_use]
fn round_power_of_two(value: i32, n: u32) -> i32 {
    (value + (1 << (n - 1))) >> n
}

/// C `ROUND_POWER_OF_TWO_64(value, n)`.
#[must_use]
fn round_power_of_two_64(value: i64, n: u32) -> i64 {
    (value + (1 << (n - 1))) >> n
}

/// The two post-encode updaters differ ONLY in where the rolling and total
/// bit counters live: [`postencode_update`] keeps them on `RATE_CONTROL`,
/// [`postencode_update_gop_const`] keeps them on the per-GOP
/// [`RcIntervalParams`] — and the latter also skips the `avg_frame_low_motion`
/// update entirely.
///
/// C spells this as two near-identical 60-line functions with the divergence
/// buried at the bottom; naming it here is what stops a reader from assuming
/// they are the same and patching only one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RollingBitsOwner {
    /// `svt_av1_rc_postencode_update` — `rc->rolling_*`, `rc->total_*`, and
    /// `rc->avg_frame_low_motion` is updated.
    RateControl,
    /// `svt_av1_rc_postencode_update_gop_const` — `rc_param_ptr->rolling_*`
    /// and `->total_*`, and NO `avg_frame_low_motion` update.
    IntervalParams,
}

#[allow(clippy::too_many_arguments)]
fn postencode_update_common(
    rc: &mut RateControl,
    cfg: &RateControlCfg,
    scs: &SeqRc,
    frame: &mut FrameRc,
    cr: &CyclicRefresh,
    params: &mut RcIntervalParams,
    total_num_bits: u64,
    avg_cnt_zeromv: u64,
    owner: RollingBitsOwner,
) {
    let width = frame.frame_width;
    let height = frame.frame_height;
    let is_intrnl_arf = frame.is_internal_arf();
    let qindex = frame.base_q_idx;

    // Update rate control heuristics.
    frame.projected_frame_size = total_num_bits as i32;
    // Post encode loop adjustment of Q prediction.
    st::update_rate_correction_factors(rc, cfg, scs, frame, cr, width, height);

    // Keep a record of the last Q and the ambient average Q.
    if frame.frame_type.is_key() {
        rc.avg_frame_qindex[KEY_FRAME as usize] =
            round_power_of_two(3 * rc.avg_frame_qindex[KEY_FRAME as usize] + qindex, 2);
        // NOTE: `last_q` stores the Q VALUE, not the qindex — the `(int32_t)`
        // cast truncates `svt_av1_convert_qindex_to_q` toward zero.
        rc.last_q[KEY_FRAME as usize] = convert_qindex_to_q(qindex, scs.encoder_bit_depth) as i32;
        rc.frame_updated = 0;
    } else {
        rc.frame_updated += 1;
        if !frame.is_overlay && !(frame.is_gf_or_arf() || is_intrnl_arf) {
            rc.avg_frame_qindex[INTER_FRAME as usize] =
                round_power_of_two(3 * rc.avg_frame_qindex[INTER_FRAME as usize] + qindex, 2);
            rc.last_q[INTER_FRAME as usize] =
                convert_qindex_to_q(qindex, scs.encoder_bit_depth) as i32;
        }
    }

    // Keep a record of the last boosted (KF/GF/ARF) Q value, used to hold
    // quality steady across forced key frames.
    if qindex < rc.last_boosted_qindex
        || frame.frame_type.is_key()
        || (rc.constrained_gf_group == 0
            && (frame.update_type == crate::port_rc_process::FrameUpdateType::ArfUpdate
                || is_intrnl_arf
                || (frame.update_type == crate::port_rc_process::FrameUpdateType::GfUpdate
                    && !frame.is_overlay)))
    {
        rc.last_boosted_qindex = qindex;
    }
    st::update_buffer_level(rc, frame, frame.projected_frame_size);
    rc.prev_avg_frame_bandwidth = rc.avg_frame_bandwidth;

    // Rolling monitors of over/underspend, used to regulate min and max Q in
    // two-pass.
    if !frame.frame_type.is_key() {
        let (rolling_target, rolling_actual) = match owner {
            RollingBitsOwner::RateControl => (rc.rolling_target_bits, rc.rolling_actual_bits),
            RollingBitsOwner::IntervalParams => {
                (params.rolling_target_bits, params.rolling_actual_bits)
            }
        };
        let new_target = round_power_of_two_64(
            i64::from(rolling_target) * 3 + i64::from(frame.this_frame_target),
            2,
        ) as i32;
        let new_actual = round_power_of_two_64(
            i64::from(rolling_actual) * 3 + i64::from(frame.projected_frame_size),
            2,
        ) as i32;
        match owner {
            RollingBitsOwner::RateControl => {
                rc.rolling_target_bits = new_target;
                rc.rolling_actual_bits = new_actual;
            }
            RollingBitsOwner::IntervalParams => {
                params.rolling_target_bits = new_target;
                params.rolling_actual_bits = new_actual;
            }
        }
    }

    if owner == RollingBitsOwner::RateControl {
        // C: `(rc->avg_frame_low_motion == 0) ? child->avg_cnt_zeromv
        //      : (3 * rc->avg_frame_low_motion + child->avg_cnt_zeromv) / 4`.
        // Zero means "not yet measured", so the first frame ADOPTS rather than
        // averaging against a zero. Absent from the gop_const variant.
        rc.avg_frame_low_motion = if rc.avg_frame_low_motion == 0 {
            avg_cnt_zeromv as i32
        } else {
            ((3 * u64::from(rc.avg_frame_low_motion as u32) + avg_cnt_zeromv) / 4) as i32
        };
    }

    // Actual bits spent.
    let showable_bits = if frame.showable_frame {
        i64::from(rc.avg_frame_bandwidth)
    } else {
        0
    };
    match owner {
        RollingBitsOwner::RateControl => {
            rc.total_actual_bits += i64::from(frame.projected_frame_size);
            rc.total_target_bits += showable_bits;
        }
        RollingBitsOwner::IntervalParams => {
            params.total_actual_bits += i64::from(frame.projected_frame_size);
            params.total_target_bits += showable_bits;
        }
    }

    if frame.frame_type.is_key() {
        rc.frames_since_key = 0;
        rc.frames_since_cdf_update = 0;
    }
}

/// C `svt_av1_rc_postencode_update` (rc_vbr_cbr.c:1562) — **EXPORTED**.
///
/// `total_num_bits` is `ppcs->total_num_bits`, the coded size of the frame in
/// bits; `avg_cnt_zeromv` is `ppcs->child_pcs->avg_cnt_zeromv`.
pub fn postencode_update(
    rc: &mut RateControl,
    cfg: &RateControlCfg,
    scs: &SeqRc,
    frame: &mut FrameRc,
    cr: &CyclicRefresh,
    total_num_bits: u64,
    avg_cnt_zeromv: u64,
) {
    // The gop_const variant's per-GOP counters are unused on this path; a
    // scratch value keeps the shared body to one copy.
    let mut unused = RcIntervalParams::default();
    postencode_update_common(
        rc,
        cfg,
        scs,
        frame,
        cr,
        &mut unused,
        total_num_bits,
        avg_cnt_zeromv,
        RollingBitsOwner::RateControl,
    );
}

/// C `svt_av1_rc_postencode_update_gop_const` (rc_vbr_cbr.c:1494) —
/// **EXPORTED**.
///
/// Identical to [`postencode_update`] except that the rolling and total bit
/// counters live on the per-GOP [`RcIntervalParams`], and
/// `rc.avg_frame_low_motion` is NOT updated. (C duplicates the whole 60-line
/// body to express that.)
pub fn postencode_update_gop_const(
    rc: &mut RateControl,
    cfg: &RateControlCfg,
    scs: &SeqRc,
    frame: &mut FrameRc,
    cr: &CyclicRefresh,
    params: &mut RcIntervalParams,
    total_num_bits: u64,
) {
    postencode_update_common(
        rc,
        cfg,
        scs,
        frame,
        cr,
        params,
        total_num_bits,
        0,
        RollingBitsOwner::IntervalParams,
    );
}

// ---------------------------------------------------------------------------
// The recode loop
// ---------------------------------------------------------------------------

/// The mutable recode-loop state C threads through `recode_loop_update_q` as
/// seven `int*` out-params plus a `bool*`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RecodeState {
    /// C `*q` — the qindex to use for the next attempt.
    pub q: i32,
    /// C `*q_low` — the lower bracket.
    pub q_low: i32,
    /// C `*q_high` — the upper bracket.
    pub q_high: i32,
    /// C `*undershoot_seen`.
    pub undershoot_seen: bool,
    /// C `*overshoot_seen`.
    pub overshoot_seen: bool,
    /// C `*low_cr_seen`.
    pub low_cr_seen: bool,
    /// C `*loop` — whether to encode this frame again. An OUTPUT only.
    pub loop_again: bool,
}

/// C `recode_loop_update_q` (rc_vbr_cbr.c:1793) — **EXPORTED**.
///
/// Decides whether the frame just coded missed its size bracket badly enough
/// to be worth re-encoding, and if so at what q. Four things happen in order,
/// and the early returns matter:
///
/// 1. `projected_frame_size` is re-derived from the entropy coder's running
///    rate (`pcs_total_rate`), or zeroed when no dummy pack is needed.
/// 2. `ALLOW_RECODE_KFMAXBW` returns immediately for a non-key frame.
/// 3. A min-compression-ratio miss forces q UP and returns.
/// 4. The over/undershoot bisection, then a final clamp; `*loop` is set from
///    whether q actually moved.
///
/// `capped_crf_reencode` (rc_crf_cqp.c) is taken as a callback for the same
/// file-boundary reason as the cyclic-refresh setup in chunk 2 — it belongs to
/// the CQP port, not to this file.
#[allow(clippy::too_many_arguments)]
pub fn recode_loop_update_q(
    rc: &mut RateControl,
    cfg: &RateControlCfg,
    scs: &SeqRc,
    frame: &mut FrameRc,
    cr: &CyclicRefresh,
    state: &mut RecodeState,
    recode_loop: RecodeLoop,
    max_bit_rate: u64,
    pcs_total_rate: u64,
    top_index: i32,
    bottom_index: i32,
    loop_count: i32,
    capped_crf_reencode: impl FnOnce(&mut i32),
) {
    let do_dummy_pack = (recode_loop >= RecodeLoop::AllowKfMaxBw
        && !(cfg.mode == AomRcMode::Q && max_bit_rate == 0))
        || cfg.min_cr > 0;
    frame.projected_frame_size = if do_dummy_pack {
        // C: `(int)((pcs_total_rate + (1 << (SHIFT-1))) >> SHIFT) + (KEY ? 13 : 0)`
        (((pcs_total_rate + (1 << (AV1_PROB_COST_SHIFT - 1))) >> AV1_PROB_COST_SHIFT) as i32)
            + i32::from(frame.frame_type.is_key()) * 13
    } else {
        0
    };
    state.loop_again = false;
    if recode_loop == RecodeLoop::AllowKfMaxBw && !frame.frame_type.is_key() {
        // Skip re-encode for inter frames when `-recode-loop 1` is set.
        return;
    }

    if cfg.min_cr > 0 {
        let compression_ratio =
            st::get_compression_ratio(scs, frame, (frame.projected_frame_size >> 3) as u64);
        let target_cr = f64::from(cfg.min_cr) / 100.0;
        if compression_ratio < target_cr {
            state.low_cr_seen = true;
            if state.q < rc.worst_quality {
                let cr_ratio = target_cr / compression_ratio;
                // C: `AOMMAX(*q + 1, (int)(*q * cr_ratio * cr_ratio))`.
                let projected_q =
                    (state.q + 1).max((f64::from(state.q) * cr_ratio * cr_ratio) as i32);
                state.q = projected_q.min(state.q + 32).min(rc.worst_quality);
                state.q_low = state.q.max(state.q_low);
                state.q_high = state.q.max(state.q_high);
                state.loop_again = true;
            }
        }
        if state.low_cr_seen {
            return;
        }
    }

    // Capped CRF: update the active worst quality.
    if cfg.mode == AomRcMode::Q && max_bit_rate != 0 {
        if frame.temporal_layer_index > 0 {
            return;
        }
        capped_crf_reencode(&mut state.q);
    }

    let last_q = state.q;
    let bounds = st::compute_frame_size_bounds(rc, cfg, scs, frame, frame.this_frame_target);
    let frame_over_shoot_limit = if bounds.over_shoot == 0 {
        1
    } else {
        bounds.over_shoot
    };

    if st::recode_loop_test(
        rc,
        recode_loop,
        frame,
        frame_over_shoot_limit,
        bounds.under_shoot,
        state.q,
        state.q_high.max(top_index),
        bottom_index,
    ) {
        let width = frame.frame_width;
        let height = frame.frame_height;
        if frame.projected_frame_size > frame.this_frame_target {
            // Frame is too large.
            if state.q == state.q_high && frame.projected_frame_size >= rc.max_frame_bandwidth {
                let q_val_high_current = convert_qindex_to_q(state.q_high, scs.encoder_bit_depth);
                let q_val_high_new = q_val_high_current
                    * (f64::from(frame.projected_frame_size) / f64::from(rc.max_frame_bandwidth));
                state.q_high = st::find_qindex(
                    q_val_high_new,
                    scs.encoder_bit_depth,
                    rc.best_quality,
                    rc.worst_quality,
                );
            }
            // Raise q_low to at least the current value.
            state.q_low = (state.q + 1).min(state.q_high);

            if state.undershoot_seen
                || loop_count > 2
                || (loop_count == 2 && !frame.is_intra_only())
            {
                st::update_rate_correction_factors(rc, cfg, scs, frame, cr, width, height);
                state.q = (state.q_high + state.q_low + 1) / 2;
            } else if loop_count == 2 && frame.is_intra_only() {
                let q_mid = (state.q_high + state.q_low + 1) / 2;
                let q_regulated = st::get_regulated_q_overshoot(
                    rc,
                    cfg,
                    scs,
                    frame,
                    cr,
                    state.q_low,
                    state.q_high,
                    top_index,
                    bottom_index,
                );
                // q in between q_mid and q_regulated, for a smooth transition
                // between loop_count < 2 and loop_count > 2.
                state.q = (q_mid + q_regulated + 1) / 2;
            } else {
                state.q = st::get_regulated_q_overshoot(
                    rc,
                    cfg,
                    scs,
                    frame,
                    cr,
                    state.q_low,
                    state.q_high,
                    top_index,
                    bottom_index,
                );
            }
            state.overshoot_seen = true;
        } else {
            // Frame is too small.
            state.q_high = (state.q - 1).max(state.q_low);

            if state.overshoot_seen || loop_count > 2 || (loop_count == 2 && !frame.is_intra_only())
            {
                st::update_rate_correction_factors(rc, cfg, scs, frame, cr, width, height);
                // NOTE: no `+ 1` on this side — the overshoot arm rounds up
                // and the undershoot arm rounds down.
                state.q = (state.q_high + state.q_low) / 2;
            } else if loop_count == 2 && frame.is_intra_only() {
                let q_mid = (state.q_high + state.q_low) / 2;
                let q_regulated = st::get_regulated_q_undershoot(
                    rc,
                    cfg,
                    scs,
                    frame,
                    cr,
                    state.q_high,
                    top_index,
                    bottom_index,
                );
                state.q = (q_mid + q_regulated) / 2;
            } else {
                state.q = st::get_regulated_q_undershoot(
                    rc,
                    cfg,
                    scs,
                    frame,
                    cr,
                    state.q_high,
                    top_index,
                    bottom_index,
                );
            }
            state.undershoot_seen = true;
        }

        // Clamp Q to the bracket.
        state.q = state.q.clamp(state.q_low.min(state.q_high), state.q_high);
    }

    state.q = st::clamp_qindex(scs, state.q);
    state.loop_again = state.q != last_q;

    // Capped CRF: update the active worst quality from the final qindex.
    if cfg.mode == AomRcMode::Q && max_bit_rate != 0 && !state.loop_again && frame.loop_count > 0 {
        rc.active_worst_quality = if frame.frame_type.is_key() {
            // C branches on `ppcs->slice_type == I_SLICE`; a key frame is the
            // only thing that reaches this in the port's envelope, and the
            // caller passes the slice type through the frame type.
            get_kf_q_tpl(
                rc.active_worst_quality,
                rc.kf_boost,
                state.q,
                scs.encoder_bit_depth,
            )
        } else {
            get_gfu_q_tpl(
                rc.active_worst_quality,
                rc.gfu_boost,
                state.q,
                scs.encoder_bit_depth,
            )
        };
        rc.active_worst_quality = st::clamp_qindex(scs, rc.active_worst_quality);
    }
}

// ---------------------------------------------------------------------------
// Dynamic resize
// ---------------------------------------------------------------------------

/// C `RESIZE_ACTION` values `dynamic_resize_one_pass_cbr` can choose.
pub use crate::port_rc_vbr_cbr_state::ResizeAction;

/// C `dynamic_resize_one_pass_cbr` (rc_vbr_cbr.c:382).
///
/// A windowed vote: over `max(60, 3 * framerate)` frames, count how often the
/// client buffer underflowed, then step the resolution ladder down when more
/// than a quarter of the window underflowed, or back up when the average QP is
/// comfortably below the worst allowed.
///
/// TWO THINGS THAT ARE EASY TO GET WRONG AND ARE REPRODUCED AS WRITTEN:
///
/// * **The underflow test is INVERTED under `--rtc`.** `rc_rtc_cbr.c` models
///   the client buffer with an inverted leaky bucket (it accumulates
///   `encoded - bandwidth` and grows under starvation), so the same 30 %
///   margin is expressed as `buffer_level > maximum - 30 % of headroom` there
///   and `buffer_level < 30 % of optimal` here. C carries both and picks on
///   `static_config.rtc`.
/// * **The scale change is relative to the PREVIOUS resize state, not to the
///   original size**, and both are derived from `scs->max_input_luma_*` rather
///   than from the current frame — otherwise a 3/4 -> 1/2 transition reports
///   the wrong magnitude and the post-resize regulation overshoots.
///
/// Returns the action taken; `rc.resize_state` is updated in place.
pub fn dynamic_resize_one_pass_cbr(
    rc: &mut RateControl,
    cfg: &RateControlCfg,
    scs: &SeqRc,
    frame: &mut FrameRc,
    one_half_only: bool,
) -> ResizeAction {
    let mut resize_action = ResizeAction::NoResize;
    let prev_resize_state = rc.resize_state;
    let avg_qp_thr1 = 70_i32;
    let avg_qp_thr2 = 50_i32;
    // Don't allow a resized frame below 160x90; resize in steps of 3/4.
    let min_width = (160 * 4) / 3;
    let min_height = (90 * 4) / 3;
    let mut down_size_on = true;

    // Step 1: don't resize on a key frame; reset the counters there.
    if frame.frame_type.is_key() {
        rc.resize_avg_qp = 0;
        rc.resize_count = 0;
        rc.resize_buffer_underflow = 0;
        return ResizeAction::NoResize;
    }

    // Step 2: no resizing down below the size limit.
    if frame.src_frame_width * frame.src_frame_height < min_width * min_height {
        down_size_on = false;
    }

    // Step 3: the windowed vote. Samples close to a key frame or a scene
    // change are ignored, because QP is usually high right after both.
    if f64::from(rc.frames_since_key) > scs.new_framerate && !frame.scene_change_flag {
        let window = 60.max((3.0 * scs.new_framerate) as i32);
        rc.resize_avg_qp += rc.last_q[INTER_FRAME as usize];
        let buffer_underflow = if scs.rtc {
            rc.buffer_level
                > rc.maximum_buffer_size
                    - 30 * (rc.maximum_buffer_size - rc.optimal_buffer_level) / 100
        } else {
            rc.buffer_level < 30 * rc.optimal_buffer_level / 100
        };
        if buffer_underflow {
            rc.resize_buffer_underflow += 1;
        }
        rc.resize_count += 1;
        if rc.resize_count >= window {
            let avg_qp = rc.resize_avg_qp / rc.resize_count;
            if rc.resize_buffer_underflow > (rc.resize_count >> 2) && down_size_on {
                if rc.resize_state == ResizeState::ThreeQuarter {
                    resize_action = ResizeAction::DownOneHalf;
                    rc.resize_state = ResizeState::OneHalf;
                } else if rc.resize_state == ResizeState::Orig {
                    let next = if one_half_only {
                        ResizeState::OneHalf
                    } else {
                        ResizeState::ThreeQuarter
                    };
                    resize_action = if one_half_only {
                        ResizeAction::DownOneHalf
                    } else {
                        ResizeAction::DownThreeFour
                    };
                    rc.resize_state = next;
                }
            } else if rc.resize_state != ResizeState::Orig
                && avg_qp < avg_qp_thr1 * rc.worst_quality / 100
            {
                if rc.resize_state == ResizeState::ThreeQuarter
                    || avg_qp < avg_qp_thr2 * rc.worst_quality / 100
                    || one_half_only
                {
                    resize_action = ResizeAction::UpOrig;
                    rc.resize_state = ResizeState::Orig;
                } else if rc.resize_state == ResizeState::OneHalf {
                    resize_action = ResizeAction::UpThreeFour;
                    rc.resize_state = ResizeState::ThreeQuarter;
                }
            }
            // Reset for the next window measurement.
            rc.resize_avg_qp = 0;
            rc.resize_count = 0;
            rc.resize_buffer_underflow = 0;
        }
    }

    // Step 4: on a transition, reset the rate control for the new size.
    if resize_action != ResizeAction::NoResize {
        let orig_w = scs.max_input_luma_width;
        let orig_h = scs.max_input_luma_height;
        let dims = |s: ResizeState| match s {
            ResizeState::Orig => (orig_w, orig_h),
            ResizeState::ThreeQuarter => (orig_w * 3 / 4, orig_h * 3 / 4),
            ResizeState::OneHalf => (orig_w / 2, orig_h / 2),
        };
        let (new_w, new_h) = dims(rc.resize_state);
        let (prev_w, prev_h) = dims(prev_resize_state);
        st::resize_reset_rc(rc, cfg, scs, frame, new_w, new_h, prev_w, prev_h);
    }
    resize_action
}

/// C `svt_aom_dynamic_resize_decision` (rc_vbr_cbr.c:497) — **EXPORTED**.
///
/// Runs the decision and publishes it to `scs->resize_pending_params` so
/// picture decision applies it to the NEXT input picture. libaom's real-time
/// caller always passes `one_half_only = 1` (the 3/4 rung is unused by the RT
/// path), and C hardcodes that for parity — so the `THREE_QUARTER` arm of the
/// denominator switch below is unreachable from here and is kept only because
/// `dynamic_resize_one_pass_cbr` can reach it from a different caller.
pub fn dynamic_resize_decision(
    rc: &mut RateControl,
    cfg: &RateControlCfg,
    scs: &SeqRc,
    frame: &mut FrameRc,
    pending: &mut ResizePendingParams,
) {
    dynamic_resize_one_pass_cbr(rc, cfg, scs, frame, /* one_half_only */ true);
    if rc.resize_state != pending.resize_state {
        pending.resize_denom = match rc.resize_state {
            ResizeState::Orig => SCALE_NUMERATOR,
            ResizeState::ThreeQuarter => SCALE_THREE_QUATER,
            ResizeState::OneHalf => SCALE_DENOMINATOR_MAX,
        };
        pending.resize_state = rc.resize_state;
    }
}

// ---------------------------------------------------------------------------
// Rate allocation entry
// ---------------------------------------------------------------------------

/// C `svt_aom_one_pass_rt_rate_alloc` (rc_vbr_cbr.c:553). `static` in C and
/// inlined away — evidence tier 4.
///
/// Sets the frame's bit target for the CBR path: a key frame gets the boosted
/// intra target, everything else the buffer-corrected P target.
///
/// `resize` selects which of the two resize behaviours C runs — the dynamic
/// decision (only for `RESIZE_DYNAMIC` + single pass + LOW_DELAY) or the
/// straight reset on `rc_reset_flag`. Modelling it as an enum keeps the
/// three-condition gate at the CALLER, where the config lives, rather than
/// pulling three more `static_config` fields into [`SeqRc`] for one branch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RtResizeMode {
    /// Neither branch applies.
    #[default]
    None,
    /// `resize_mode == RESIZE_DYNAMIC && pass == ENC_SINGLE_PASS &&
    /// pred_structure == LOW_DELAY`.
    Dynamic,
    /// `ppcs->rc_reset_flag`, with the render size and the configured size.
    ResetTo {
        render_width: i32,
        render_height: i32,
    },
}

/// C `svt_aom_one_pass_rt_rate_alloc` (rc_vbr_cbr.c:553).
pub fn one_pass_rt_rate_alloc(
    rc: &mut RateControl,
    cfg: &RateControlCfg,
    scs: &SeqRc,
    frame: &mut FrameRc,
    pending: &mut ResizePendingParams,
    resize: RtResizeMode,
) {
    // Set frame type.
    if frame.is_intra_only() {
        rc.kf_boost = st::DEFAULT_KF_BOOST_RT;
        rc.this_key_frame_forced = frame.picture_number != 0 && rc.frames_to_key == 0;
        rc.frames_to_key = scs.intra_period_length + 1;
    }

    match resize {
        RtResizeMode::Dynamic => dynamic_resize_decision(rc, cfg, scs, frame, pending),
        RtResizeMode::ResetTo {
            render_width,
            render_height,
        } => st::resize_reset_rc(
            rc,
            cfg,
            scs,
            frame,
            render_width,
            render_height,
            scs.max_input_luma_width,
            scs.max_input_luma_height,
        ),
        RtResizeMode::None => {}
    }

    // Set the GF interval and update flag.
    st::set_gf_interval_update_onepass_rt(rc, frame);

    // Set target size.
    let target = if frame.frame_type.is_key() {
        st::calc_iframe_target_size_one_pass_cbr(rc, cfg, scs, frame)
    } else {
        st::calc_pframe_target_size_one_pass_cbr(rc, cfg)
    };
    frame.this_frame_target = target;
    frame.base_frame_target = target;
}

/// C `store_param` (rc_vbr_cbr.c:766).
pub fn store_param(params: &mut RcIntervalParams, kf_group_bits: i64, kf_group_error_left: i64) {
    params.kf_group_bits = kf_group_bits;
    params.kf_group_error_left = kf_group_error_left;
}

/// The `TWO_PASS` fields `restore_param` writes back.
#[derive(Clone, Copy, Debug, Default)]
pub struct TwoPassRestored {
    pub kf_group_bits: i64,
    pub kf_group_error_left: i64,
    pub extend_minq: i32,
    pub extend_maxq: i32,
    pub extend_minq_fast: i32,
}

/// C `restore_param` (rc_vbr_cbr.c:725) — the ARITHMETIC half.
///
/// **What is translated:** the `frames_since_key` / `frames_to_key` derivation
/// from the decode order and the last IDR, the `key_max` narrowing against the
/// last first-pass stats frame, and the end-of-sequence `kf_group_bits`
/// rescale (which is one-shot — it sets `end_of_seq_seen`).
///
/// **What is NOT, and why:** C's first act is to re-point
/// `twopass->stats_in` and `stats_buf_ctx->stats_in_end` into the first-pass
/// stats ring by pointer arithmetic on `stats_in_start + ppcs->stats_in_offset`.
/// That is buffer plumbing, not a computation — the port's two-pass reader
/// owns its own cursor and this function has nothing to say about it. Same for
/// `restore_two_pass_param`'s accumulate loop, which walks the same ring; the
/// per-frame accumulation itself is `svt_av1_accumulate_stats` and belongs to
/// the `pass2_strategy.c` port.
///
/// `lap_rc` and `enable_dec_order` gate several of these; both are passed
/// explicitly rather than folded into [`SeqRc`], because they are two-pass
/// configuration and no other function in this lane reads them.
#[allow(clippy::too_many_arguments)]
pub fn restore_param(
    rc: &mut RateControl,
    params: &mut RcIntervalParams,
    twopass: &mut TwoPassRestored,
    scs: &SeqRc,
    frame: &FrameRc,
    lap_rc: bool,
    lad_mg: i32,
    decode_order: u64,
    last_idr_picture: u64,
    end_of_sequence_region: bool,
    last_first_pass_frame: i64,
) {
    if scs.gop_constraint_rc && params.first_poc == frame.picture_number {
        params.rolling_target_bits = rc.avg_frame_bandwidth;
        params.rolling_actual_bits = rc.avg_frame_bandwidth;
    }

    // restore_two_pass_param's scalar half.
    twopass.kf_group_bits = params.kf_group_bits;
    twopass.kf_group_error_left = params.kf_group_error_left;
    if scs.gop_constraint_rc {
        twopass.extend_minq = params.extend_minq;
        twopass.extend_maxq = params.extend_maxq;
        twopass.extend_minq_fast = params.extend_minq_fast;
        rc.vbr_bits_off_target = params.vbr_bits_off_target;
        rc.vbr_bits_off_target_fast = params.vbr_bits_off_target_fast;
        rc.rolling_target_bits = params.rolling_target_bits;
        rc.rolling_actual_bits = params.rolling_actual_bits;
        rc.total_actual_bits = params.total_actual_bits;
        rc.total_target_bits = params.total_target_bits;
    }

    let mut key_max = scs.intra_period_length + 1;
    // C: `(int)(stats_in_end[-1].frame - ppcs->last_idr_picture + 1)` — the
    // last first-pass stats frame, relative to this GOP's IDR.
    let last_frame_diff = last_first_pass_frame - last_idr_picture as i64 + 1;
    if lap_rc {
        if i32::from(scs.hierarchical_levels) != i32::from(frame.hierarchical_levels)
            || end_of_sequence_region
        {
            key_max = key_max.min(last_frame_diff as i32);
        }
    } else {
        key_max = key_max.min(last_frame_diff as i32);
    }

    rc.frames_since_key = (decode_order - last_idr_picture) as i32;
    rc.frames_to_key = key_max - rc.frames_since_key;
    // For the last minigop of a sequence the lookahead may not have been long
    // enough to see the GOP end when kf_group_bits was computed; once the end
    // IS in the lookahead, rescale the remaining budget to the true distance.
    if lap_rc
        && (scs.intra_period_length + 1) != rc.frames_since_key
        && (lad_mg + 1) * (1 << scs.hierarchical_levels) < scs.intra_period_length
        && (i32::from(scs.hierarchical_levels) != i32::from(frame.hierarchical_levels)
            || end_of_sequence_region)
        && !params.end_of_seq_seen
    {
        twopass.kf_group_bits = i64::from(rc.frames_to_key) * twopass.kf_group_bits
            / i64::from(scs.intra_period_length + 1 - rc.frames_since_key);
        params.end_of_seq_seen = true;
    }
}

/// C `svt_av1_rc_process_rate_allocation` (rc_vbr_cbr.c:779) — **EXPORTED**.
///
/// The rate-control entry point for a VBR/CBR frame: initialise the buffers on
/// the first picture, refresh the TPL-derived boosts, then take the CBR path
/// (a direct target computation) or the VBR path (restore the two-pass state,
/// consume the first-pass stats, correct the target, store the state back).
///
/// `process_rc_stat` is the `pass2_strategy.c` step and is taken as a callback
/// so the two ports keep their file boundary; `rc_init` likewise belongs to
/// `rc_process.c` and is already ported there.
#[allow(clippy::too_many_arguments)]
pub fn process_rate_allocation(
    rc: &mut RateControl,
    cfg: &RateControlCfg,
    scs: &SeqRc,
    frame: &mut FrameRc,
    tpl: &TplCtrlsRc,
    pending: &mut ResizePendingParams,
    resize: RtResizeMode,
    seq_param_changed: bool,
    mut rc_init: impl FnMut(&mut RateControl),
    process_rc_stat: impl FnOnce(&mut RateControl, &mut FrameRc),
    set_target_rate: impl FnOnce(&mut RateControl, &mut FrameRc),
) {
    if frame.picture_number == 0 || seq_param_changed {
        st::set_rc_buffer_sizes(rc, cfg, scs);
        rc_init(rc);
    }

    let update_type = frame.update_type;
    use crate::port_rc_process::FrameUpdateType as U;
    if tpl.enable
        && frame.r0 != 0.0
        && matches!(update_type, U::KfUpdate | U::GfUpdate | U::ArfUpdate)
    {
        crate::port_rc_vbr_cbr_qpick::process_tpl_stats_frame_kf_gfu_boost(rc, scs, frame, tpl);
    }

    if cfg.mode == AomRcMode::Cbr {
        one_pass_rt_rate_alloc(rc, cfg, scs, frame, pending, resize);
    } else {
        // C: restore_param -> svt_aom_process_rc_stat -> av1_set_target_rate
        // -> store_param, all under `enc_ctx->stat_file_mutex`. The mutex is
        // not translated (see the module header of `port_rc_vbr_cbr_state`);
        // the three steps are the caller's to supply because two of them live
        // in `pass2_strategy.c`.
        process_rc_stat(rc, frame);
        set_target_rate(rc, frame);
    }
}

/// C `frame_is_intra_only` applied to a [`FrameType`], re-exported so callers
/// of this module do not have to reach into chunk 1 for it.
#[must_use]
pub fn frame_is_intra_only(frame_type: FrameType) -> bool {
    frame_type.is_intra_only()
}
