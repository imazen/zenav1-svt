//! Port of the scalar core of `Codec/rc_vbr_cbr.c` — the active-quality
//! interpolation over `rc_tables.h`'s minq tables, its two reverse searches,
//! the qindex-by-q binary search, and the small rate/target clamps.
//!
//! **SCOPE, stated first because the missing part is larger than the present
//! part.** `rc_vbr_cbr.c` is 1925 lines and ~54 functions. This file ports
//! **11 of them**. What is NOT here: every function that reads or mutates a
//! `PictureParentControlSet` / `RATE_CONTROL` beyond the two or three scalar
//! fields these take —
//! `av1_calc_pframe_target_size_one_pass_cbr`, `svt_aom_reset_update_frame_target`,
//! `calc_active_worst_quality_no_stats_cbr`, `adjust_q_cbr`,
//! `get_rate_correction_factor`, `set_rate_correction_factor`,
//! `set_gf_interval_update_onepass_rt`, `dynamic_resize_one_pass_cbr`,
//! `svt_aom_dynamic_resize_decision`, `av1_calc_iframe_target_size_one_pass_cbr`,
//! `svt_aom_one_pass_rt_rate_alloc`, `set_rc_buffer_sizes`,
//! `process_tpl_stats_frame_kf_gfu_boost`, `vbr_rate_correction`,
//! `av1_set_target_rate`, `restore_param`, `store_param`,
//! `svt_av1_rc_process_rate_allocation`, `cyclic_refresh_init`,
//! `compute_cr_deltaq`, `cyclic_refresh_compute_cr_qdeltas`,
//! `rc_pick_q_and_bounds_no_stats_cbr`, `av1_frame_type_qdelta_org`,
//! `get_active_best_quality`, `get_q`, `rc_pick_q_and_bounds`,
//! `find_min_ref_base_q_idx`, `svt_av1_rc_calc_qindex_rate_control`,
//! `av1_rc_update_rate_correction_factors`, `update_buffer_level`,
//! `svt_av1_rc_postencode_update{,_gop_const}`, `av1_get_compression_ratio`,
//! `get_regulated_q_undershoot`, `av1_rc_compute_frame_size_bounds`,
//! `recode_loop_update_q`, and the rest.
//!
//! **EVIDENCE.** The eighteen `rc_tables.h` tables and the four BOOST
//! thresholds are pinned at TIER 1 — they are `static const` / `#define` in a
//! header with no exported symbol, so `shims/rc_shims.c` indexes the REAL C
//! arrays and reads the REAL macros, and the port's copies (extracted with a
//! parser, never hand-typed) are compared entry for entry. The FUNCTIONS here
//! are all `static` in C with no exported symbol, so they are TIER 4:
//! hand-derived vectors traced against the source. [`find_qindex`] is the
//! exception in spirit — it is pure arithmetic over
//! `svt_av1_convert_qindex_to_q`, which IS exported and IS pinned at tier 1 in
//! `c_parity_rc_process.rs`, so its only unpinned content is the search shape.
//!
//! None of this is reachable in the CQP/CRF envelope the port ships
//! (`rc_cfg.mode == AOM_Q` skips `svt_av1_rc_calc_qindex_rate_control`
//! entirely). It is translated because the directive is to leave nothing
//! untranslated, and `WORKING-ON-THIS.md` §7 says dead-looking C stays
//! translated with its reachability written down. This paragraph is that
//! writing-down.

use crate::rate_control::{compute_qdelta, convert_qindex_to_q};

/// C's `rc_tables.h` minq tables.
pub mod minq_tables {
    include!("port_rc_vbr_tables.rs");
}

/// C `BOOST_KF_LOW` (rc_process.h:62).
pub const BOOST_KF_LOW: i32 = 400;
/// C `BOOST_KF_HIGH` (rc_process.h:61).
pub const BOOST_KF_HIGH: i32 = 5000;
/// C `BOOST_GF_LOW_TPL_LA` (rc_process.h:60).
pub const BOOST_GF_LOW_TPL_LA: i32 = 300;
/// C `BOOST_GF_HIGH_TPL_LA` (rc_process.h:59).
pub const BOOST_GF_HIGH_TPL_LA: i32 = 2400;

/// Which minq family a call site wants; the second half of C's
/// `ASSIGN_MINQ_TABLE(bit_depth, name)` selection, which keys off the C
/// VARIABLE NAME as well as the bit depth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MinqFamily {
    KfLowMotionCqp,
    KfHighMotion,
    ArfgfLowMotion,
    ArfgfHighMotion,
    Inter,
    Rtc,
}

/// C `ASSIGN_MINQ_TABLE(bit_depth, name)` (rc_tables.h:14).
///
/// THE MACRO SELECTS ON THE VARIABLE NAME, not only on the bit depth: a call
/// site that writes `ASSIGN_MINQ_TABLE(bd, kf_low_motion_minq_cqp)` and
/// `ASSIGN_MINQ_TABLE(bd, kf_high_motion_minq)` gets two DIFFERENT tables from
/// what looks like the same macro twice. Splitting the selection into an
/// explicit family plus a bit depth is what keeps that visible.
///
/// C's macro sets the pointer to `NULL` for a bit depth outside {8, 10, 12}
/// and the caller then dereferences it; the port panics instead of
/// reproducing that.
#[must_use]
pub fn assign_minq_table(bit_depth: u8, family: MinqFamily) -> &'static [i32; 256] {
    use minq_tables as t;
    match (family, bit_depth) {
        (MinqFamily::KfLowMotionCqp, 8) => &t::KF_LOW_MOTION_MINQ_CQP_8,
        (MinqFamily::KfLowMotionCqp, 10) => &t::KF_LOW_MOTION_MINQ_CQP_10,
        (MinqFamily::KfLowMotionCqp, 12) => &t::KF_LOW_MOTION_MINQ_CQP_12,
        (MinqFamily::KfHighMotion, 8) => &t::KF_HIGH_MOTION_MINQ_8,
        (MinqFamily::KfHighMotion, 10) => &t::KF_HIGH_MOTION_MINQ_10,
        (MinqFamily::KfHighMotion, 12) => &t::KF_HIGH_MOTION_MINQ_12,
        (MinqFamily::ArfgfLowMotion, 8) => &t::ARFGF_LOW_MOTION_MINQ_8,
        (MinqFamily::ArfgfLowMotion, 10) => &t::ARFGF_LOW_MOTION_MINQ_10,
        (MinqFamily::ArfgfLowMotion, 12) => &t::ARFGF_LOW_MOTION_MINQ_12,
        (MinqFamily::ArfgfHighMotion, 8) => &t::ARFGF_HIGH_MOTION_MINQ_8,
        (MinqFamily::ArfgfHighMotion, 10) => &t::ARFGF_HIGH_MOTION_MINQ_10,
        (MinqFamily::ArfgfHighMotion, 12) => &t::ARFGF_HIGH_MOTION_MINQ_12,
        (MinqFamily::Inter, 8) => &t::INTER_MINQ_8,
        (MinqFamily::Inter, 10) => &t::INTER_MINQ_10,
        (MinqFamily::Inter, 12) => &t::INTER_MINQ_12,
        (MinqFamily::Rtc, 8) => &t::RTC_MINQ_8,
        (MinqFamily::Rtc, 10) => &t::RTC_MINQ_10,
        (MinqFamily::Rtc, 12) => &t::RTC_MINQ_12,
        (_, other) => panic!("assign_minq_table: unsupported bit depth {other}"),
    }
}

/// C `get_active_quality` (rc_vbr_cbr.c:30). `static` — tier 4.
///
/// Interpolates between the low- and high-motion minq tables by where `boost`
/// sits in `[low, high]`. Outside the range it saturates to one table.
///
/// THE INTERPOLATION IS ANCHORED ON `low_motion_minq`, and `offset` counts
/// DOWN from `high`: `boost == high` gives `offset == 0` and therefore the
/// low-motion value, `boost == low` gives `offset == gap` and therefore the
/// high-motion value. A higher boost picks the LOWER-motion (better) table —
/// the naming makes it easy to read backwards.
///
/// `(offset * qdiff + (gap >> 1)) / gap` is C integer division, which
/// TRUNCATES TOWARD ZERO. `qdiff` can be negative (the high-motion table is
/// not uniformly above the low-motion one at every qindex), and then the
/// `+ (gap >> 1)` rounding bias plus truncation is NOT symmetric round-half-up.
/// Reproduced as written.
#[must_use]
pub fn get_active_quality(
    q: usize,
    boost: i32,
    low: i32,
    high: i32,
    low_motion_minq: &[i32; 256],
    high_motion_minq: &[i32; 256],
) -> i32 {
    if boost > high {
        return low_motion_minq[q];
    }
    if boost < low {
        return high_motion_minq[q];
    }
    let gap = high - low;
    let offset = high - boost;
    let qdiff = high_motion_minq[q] - low_motion_minq[q];
    let adjustment = (offset * qdiff + (gap >> 1)) / gap;
    low_motion_minq[q] + adjustment
}

/// C `get_kf_active_quality_tpl` (rc_vbr_cbr.c:47). `static` — tier 4.
#[must_use]
pub fn get_kf_active_quality_tpl(kf_boost: i32, q: usize, bit_depth: u8) -> i32 {
    get_active_quality(
        q,
        kf_boost,
        BOOST_KF_LOW,
        BOOST_KF_HIGH,
        assign_minq_table(bit_depth, MinqFamily::KfLowMotionCqp),
        assign_minq_table(bit_depth, MinqFamily::KfHighMotion),
    )
}

/// C `get_gf_active_quality_tpl_la` (rc_vbr_cbr.c:56). `static` — tier 4.
#[must_use]
pub fn get_gf_active_quality_tpl_la(gfu_boost: i32, q: usize, bit_depth: u8) -> i32 {
    get_active_quality(
        q,
        gfu_boost,
        BOOST_GF_LOW_TPL_LA,
        BOOST_GF_HIGH_TPL_LA,
        assign_minq_table(bit_depth, MinqFamily::ArfgfLowMotion),
        assign_minq_table(bit_depth, MinqFamily::ArfgfHighMotion),
    )
}

/// C `get_gf_high_motion_quality` (rc_vbr_cbr.c:65). `static` — tier 4.
#[must_use]
pub fn get_gf_high_motion_quality(q: usize, bit_depth: u8) -> i32 {
    assign_minq_table(bit_depth, MinqFamily::ArfgfHighMotion)[q]
}

/// The shared body of C's `get_kf_q_tpl` (rc_vbr_cbr.c:1641) and `get_gfu_q_tpl`
/// (:1666) — "the functionality is the reverse of get_*_active_quality", i.e.
/// walk `q` until the interpolated active quality lands near the target.
/// Both `static` — tier 4.
///
/// THE LOOP HAS TWO EXIT CONDITIONS AND ONLY ONE IS THE OBVIOUS ONE. C:
/// `while (abs(target - active) > 4 && abs(target - active) <= prev_dif)`.
/// `prev_dif` is computed ONCE, BEFORE the loop, and is never updated inside
/// it — so the second clause is not "stopped improving", it is "still no worse
/// than the very first difference". That also means the loop can run a long
/// time while oscillating, and can exit on the first iteration if the step
/// overshoots past the initial difference.
///
/// **C CAN LOOP FOREVER HERE, AND THE PORT REFUSES TO** (`WORKING-ON-THIS.md`
/// §6 — a suspected C defect, not a port bug). Because `prev_dif` never
/// updates, `abs(target - active) <= prev_dif` stays TRUE whenever the active
/// quality stops changing — which it does as soon as `q` leaves the table's
/// 0..=255 domain, since every further step then reads the same saturated end.
/// With an unreachable target C therefore walks `q` without bound, indexing
/// the array out of range on every iteration.
///
/// MEASURED, not argued: the port's first faithful transcription of this loop
/// panicked on `get_kf_q_tpl(200, 1000, 100_000, 8)` with an integer overflow
/// after ~4 s of walking. That IS the runaway, observed.
///
/// So the port keeps C's loop verbatim and adds ONE extra exit: the walk stops
/// at the edge of the qindex domain. Inside the domain — every input with a
/// target the tables can actually produce — the two are identical. Outside it
/// C's behaviour is an out-of-bounds read, so there is no defined value to
/// reproduce.
fn reverse_active_quality(
    start_q: i32,
    boost: i32,
    target_active_quality: i32,
    low: i32,
    high: i32,
    low_motion_minq: &[i32; 256],
    high_motion_minq: &[i32; 256],
) -> i32 {
    let idx = |q: i32| q.clamp(0, 255) as usize;
    let mut q = start_q.clamp(0, 255);
    let mut active_quality =
        get_active_quality(idx(q), boost, low, high, low_motion_minq, high_motion_minq);
    // C computes `prev_dif` ONCE, before the loop, and never updates it.
    let prev_dif = (target_active_quality - active_quality).abs();
    while (target_active_quality - active_quality).abs() > 4
        && (target_active_quality - active_quality).abs() <= prev_dif
    {
        let next = if active_quality > target_active_quality {
            q - 1
        } else {
            q + 1
        };
        // The added exit — see the doc comment. C has no bound here.
        if !(0..=255).contains(&next) {
            break;
        }
        q = next;
        active_quality =
            get_active_quality(idx(q), boost, low, high, low_motion_minq, high_motion_minq);
    }
    q
}

/// C `get_kf_q_tpl` (rc_vbr_cbr.c:1641). `static` — tier 4.
/// `start_q` is C's `rc->active_worst_quality`.
#[must_use]
pub fn get_kf_q_tpl(start_q: i32, kf_boost: i32, target_active_quality: i32, bit_depth: u8) -> i32 {
    reverse_active_quality(
        start_q,
        kf_boost,
        target_active_quality,
        BOOST_KF_LOW,
        BOOST_KF_HIGH,
        assign_minq_table(bit_depth, MinqFamily::KfLowMotionCqp),
        assign_minq_table(bit_depth, MinqFamily::KfHighMotion),
    )
}

/// C `get_gfu_q_tpl` (rc_vbr_cbr.c:1666). `static` — tier 4.
#[must_use]
pub fn get_gfu_q_tpl(
    start_q: i32,
    gfu_boost: i32,
    target_active_quality: i32,
    bit_depth: u8,
) -> i32 {
    reverse_active_quality(
        start_q,
        gfu_boost,
        target_active_quality,
        BOOST_GF_LOW_TPL_LA,
        BOOST_GF_HIGH_TPL_LA,
        assign_minq_table(bit_depth, MinqFamily::ArfgfLowMotion),
        assign_minq_table(bit_depth, MinqFamily::ArfgfHighMotion),
    )
}

/// C `av1_find_qindex` (rc_vbr_cbr.c:1772). `static` — tier 4, but its only
/// unpinned content is the SEARCH SHAPE: the value it compares,
/// `svt_av1_convert_qindex_to_q`, is exported and pinned at tier 1 in
/// `c_parity_rc_process.rs`.
///
/// This is the sibling of `find_qindex_by_rate` (rc_process.c:270) with the
/// comparison INVERTED: that one searches downward on bits (higher qindex ->
/// fewer bits, so it moves `low` up when the value is too HIGH), this one
/// searches upward on q (higher qindex -> larger q, so it moves `low` up when
/// the value is too LOW). Copying one into the other flips the result.
#[must_use]
pub fn find_qindex(desired_q: f64, bit_depth: u8, best_qindex: i32, worst_qindex: i32) -> i32 {
    debug_assert!(best_qindex <= worst_qindex);
    let mut low = best_qindex;
    let mut high = worst_qindex;
    while low < high {
        let mid = (low + high) >> 1;
        let mid_q = convert_qindex_to_q(mid, bit_depth);
        if mid_q < desired_q {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    low
}

/// C `get_bits_per_mb` (rc_vbr_cbr.c:208). `static` — tier 4, and a pure
/// forward to the exported `svt_av1_rc_bits_per_mb` (pinned at tier 1 in
/// `c_parity_rc_process.rs`) with the PPCS fields unpacked.
#[must_use]
pub fn get_bits_per_mb(
    frame_type: i32,
    sc_class1: bool,
    bit_depth: u8,
    correction_factor: f64,
    q: i32,
) -> i32 {
    crate::port_rc_process::rc_bits_per_mb(frame_type, q, correction_factor, bit_depth, sc_class1)
}

/// C `av1_rc_clamp_iframe_target_size` (rc_vbr_cbr.c:518). `static` — tier 4.
///
/// The `max_intra_bitrate_pct` clamp is SKIPPED ENTIRELY when the pct is 0 —
/// it is not treated as "0% of the bandwidth". The `max_frame_bandwidth`
/// clamp then applies unconditionally.
///
/// `rc->avg_frame_bandwidth * max_intra_bitrate_pct` is `int * unsigned int`
/// in C, so the int is converted to unsigned and the product is computed in
/// `unsigned int`, THEN `/ 100`, THEN assigned back to an `int`. For the
/// non-negative bandwidths this can see that is the same as i64 arithmetic
/// until the product exceeds 2^32, where C wraps; the port reproduces the
/// unsigned wrap with `u32`.
#[must_use]
pub fn clamp_iframe_target_size(
    avg_frame_bandwidth: i32,
    max_frame_bandwidth: i32,
    max_intra_bitrate_pct: u32,
    mut target: i32,
) -> i32 {
    if max_intra_bitrate_pct != 0 {
        // C: `int max_rate = rc->avg_frame_bandwidth * rc_cfg->max_intra_bitrate_pct / 100;`
        // — the multiply happens in `unsigned int` because of the usual
        // arithmetic conversions, so it wraps rather than saturating.
        let max_rate =
            ((avg_frame_bandwidth as u32).wrapping_mul(max_intra_bitrate_pct) / 100) as i32;
        target = target.min(max_rate);
    }
    if target > max_frame_bandwidth {
        target = max_frame_bandwidth;
    }
    target
}

/// C's `adjust_q_cbr` content-change arm (rc_vbr_cbr.c:186-196). `static` —
/// tier 4. Extracted as its own function because it is the only part of
/// `adjust_q_cbr` that is pure arithmetic; the rest of that function reads a
/// `max_delta_per_layer` table plus eight `RATE_CONTROL` fields and is listed
/// as NOT PORTED in this module's header.
///
/// `q_adj_factor = 1.0 + 0.5 * tanh(4.0 * delta)` — `tanh` is a libm call, so
/// this value is host-libm-dependent in exactly the way
/// `WORKING-ON-THIS.md` §5c describes. It is NOT reachable in the port's
/// envelope (CBR only), but if it ever becomes reachable it belongs in
/// `tools/fp_cross_isa.sh`'s transcendental list.
#[must_use]
pub fn cbr_content_change_qdelta(
    cur_avg_base_me_dist: u32,
    prev_avg_base_me_dist: u32,
    q: i32,
    bit_depth: u8,
) -> i32 {
    let delta = f64::from(cur_avg_base_me_dist) / f64::from(prev_avg_base_me_dist) - 1.0;
    let q_adj_factor = 1.0 + 0.5 * (4.0 * delta).tanh();
    let q_val = convert_qindex_to_q(q, bit_depth);
    compute_qdelta(q_val, q_val * q_adj_factor, bit_depth)
}
