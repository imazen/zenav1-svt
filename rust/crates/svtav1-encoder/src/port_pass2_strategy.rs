//! Port of the scalar core of `Codec/pass2_strategy.c` — the two-pass
//! bit-allocation math.
//!
//! **COVERAGE, missing first.** `pass2_strategy.c` is 1270 lines and 29
//! functions. This file ports **8**; [`crate::port_rc_process`] already owns
//! `svt_aom_set_rc_param` (:906), `svt_av1_new_framerate` (:901) and
//! `av1_rc_update_framerate` (:880), for 11 of 29 across the two files.
//!
//! NOT PORTED (18): `reset_fpf_position`, `input_stats`, `subtract_stats`
//! (all three walk a `FIRSTPASS_STATS*` cursor into a stats file),
//! `accumulate_this_frame_stats`, `calculate_total_gf_group_bits`,
//! `set_baseline_gf_interval`, `init_gf_stats`, `calculate_active_worst_quality`,
//! `gf_group_rate_assingment`, `lap_rc_init`, `lap_rc_group_error_calc`,
//! `get_kf_group_bits`, `kf_group_rate_assingment`,
//! `get_section_target_bandwidth`, `process_first_pass_stats`,
//! `is_new_gf_group`, `svt_aom_process_rc_stat`, `read_stat_from_file`,
//! `svt_av1_init_single_pass_lap`, `svt_av1_init_second_pass`,
//! `svt_av1_twopass_postencode_update{,_gop_const}` — every one of them reads a
//! `TWO_PASS` stats cursor, a `PictureParentControlSet`, or both.
//!
//! **EVIDENCE.** `q_pow_term` is `static const` in the .c file (not even a
//! header), so there is no symbol and no shim can reach it; it and every
//! function here are **TIER 4**, hand-derived vectors traced against the
//! source. The one lever that raises the floor:
//! [`find_qindex_by_rate_with_correction`] is built on
//! `svt_av1_convert_qindex_to_q`, which IS exported and IS pinned at tier 1 in
//! `c_parity_rc_process.rs`, so its q ladder is not a transcription.
//!
//! **REACHABILITY: none in the port's envelope.** Everything here is on the
//! two-pass / LAP path; `svt_aom_set_rc_param` sets `AOM_Q` for CQP/CRF and
//! `resource_coordination_process.c:1074-1078` takes the single-pass branch.
//! Translated per `WORKING-ON-THIS.md` §7 with the reachability written down.

use crate::rate_control::convert_qindex_to_q;

/// C `BPER_MB_NORMBITS` (rc_process.h:26).
pub const BPER_MB_NORMBITS: u32 = 9;

/// C `ERR_DIVISOR` (pass2_strategy.c:61).
pub const ERR_DIVISOR: f64 = 96.0;

/// C `q_pow_term[(QINDEX_RANGE >> 5) + 1]` (pass2_strategy.c:59).
///
/// **THE `+ 1` IN THE LENGTH IS LOAD-BEARING.** [`calc_correction_factor`]
/// reads both `q_pow_term[q >> 5]` and `q_pow_term[(q >> 5) + 1]`, and at
/// `q == 255` that is index 7 and index 8 — so the table needs 9 entries for
/// a 256-entry qindex range and the last one exists only to be the upper
/// interpolation endpoint. Sizing it `256 >> 5 == 8` reads out of bounds at
/// the top of the ladder.
pub const Q_POW_TERM: [f64; 9] = [0.65, 0.70, 0.75, 0.80, 0.85, 0.90, 0.95, 0.95, 0.95];

/// C `fclamp` (definitions.h:717).
#[must_use]
pub fn fclamp(value: f64, low: f64, high: f64) -> f64 {
    if value < low {
        low
    } else if value > high {
        high
    } else {
        value
    }
}

/// C `frame_max_bits` (pass2_strategy.c:55). `static` — tier 4.
///
/// `(int64_t)rc->avg_frame_bandwidth * vbrmax_section / 100`, then
/// `CLIP3(0, rc->max_frame_bandwidth, max_bits)`. The CLIP3 runs on the
/// **`int64_t`** value and the result is cast to `int` afterwards, so the
/// upper bound is what prevents the narrowing from wrapping — the clamp is
/// load-bearing, not cosmetic.
#[must_use]
pub fn frame_max_bits(
    avg_frame_bandwidth: i32,
    max_frame_bandwidth: i32,
    vbrmax_section: i32,
) -> i32 {
    let max_bits = i64::from(avg_frame_bandwidth) * i64::from(vbrmax_section) / 100;
    max_bits.clamp(0, i64::from(max_frame_bandwidth)) as i32
}

/// C `calc_correction_factor` (pass2_strategy.c:63). `static` — tier 4.
///
/// `pow(err_per_mb / 96, power_term)` clamped to `[0.05, 5.0]`, where
/// `power_term` linearly interpolates [`Q_POW_TERM`] between `q >> 5` and the
/// next entry by `(q % 32) / 32`.
///
/// TWO THINGS THAT READ WRONG:
/// * the interpolation weight is `q % 32`, i.e. 0..=31 over a divisor of 32,
///   so the top of each segment reaches only 31/32 of the way to the next
///   entry — it never actually lands on `q_pow_term[index + 1]`. That is why
///   the duplicated 0.95 tail is harmless and why "it's a mean of the two
///   endpoints" is wrong.
/// * `pow` is a libm call, so this value is host-libm-dependent exactly as
///   `WORKING-ON-THIS.md` §5c describes. It is unreachable in the port's
///   envelope; if it ever becomes reachable it belongs in
///   `tools/fp_cross_isa.sh`'s transcendental list.
///
/// C asserts `error_term >= 0.0`; the port keeps that as a `debug_assert`
/// because a negative base with a fractional exponent is NaN in both
/// languages and silently producing NaN would be worse than the assert.
#[must_use]
pub fn calc_correction_factor(err_per_mb: f64, q: i32) -> f64 {
    let error_term = err_per_mb / ERR_DIVISOR;
    let index = (q >> 5) as usize;
    let power_term = Q_POW_TERM[index]
        + ((Q_POW_TERM[index + 1] - Q_POW_TERM[index]) * f64::from(q % 32)) / 32.0;
    debug_assert!(error_term >= 0.0);
    fclamp(error_term.powf(power_term), 0.05, 5.0)
}

/// C `qbpm_enumerator` (pass2_strategy.c:72). `static` — tier 4.
///
/// `1250000 + ((300000 * AOMMIN(75, AOMMAX(rate_err_tol - 25, 0))) / 75)`.
/// The inner clamp is on `rate_err_tol - 25`, NOT on `rate_err_tol`, so the
/// enumerator is flat at 1,250,000 for every tolerance up to 25 and saturates
/// at 1,550,000 from 100 upward.
#[must_use]
pub fn qbpm_enumerator(rate_err_tol: i32) -> i32 {
    // C: `AOMMIN(75, AOMMAX(rate_err_tol - 25, 0))` — the max runs first.
    1_250_000 + ((300_000 * (rate_err_tol - 25).clamp(0, 75)) / 75)
}

/// C `find_qindex_by_rate_with_correction` (pass2_strategy.c:78). `static` —
/// tier 4, but its q ladder is `svt_av1_convert_qindex_to_q`, which is
/// exported and tier-1 pinned in `c_parity_rc_process.rs`.
///
/// The same binary search as `find_qindex_by_rate` (rc_process.c:270) with the
/// modelled bits replaced by
/// `(int)((qbpm_enumerator(tol) * correction * group_weight) / q)`. Note the
/// enumerator is recomputed INSIDE the loop from a value that does not change
/// — faithful, and worth not "optimising" out, because hoisting it changes
/// nothing but makes a future diff against C harder to read.
#[must_use]
pub fn find_qindex_by_rate_with_correction(
    desired_bits_per_mb: i32,
    bit_depth: u8,
    error_per_mb: f64,
    group_weight_factor: f64,
    rate_err_tol: i32,
    best_qindex: i32,
    worst_qindex: i32,
) -> i32 {
    debug_assert!(best_qindex <= worst_qindex);
    let mut low = best_qindex;
    let mut high = worst_qindex;
    while low < high {
        let mid = (low + high) >> 1;
        let mid_factor = calc_correction_factor(error_per_mb, mid);
        let q = convert_qindex_to_q(mid, bit_depth);
        let enumerator = qbpm_enumerator(rate_err_tol);
        let mid_bits_per_mb =
            ((f64::from(enumerator) * mid_factor * group_weight_factor) / q) as i32;
        if mid_bits_per_mb > desired_bits_per_mb {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    low
}

/// The MB grid `get_twopass_worst_quality` (pass2_strategy.c:120) derives.
///
/// **THIS IS NOT `svt_aom_set_rc_param`'S GRID, and the difference is a real
/// off-by-one.** `set_rc_param` computes `((w + 15) / 16) << 1` on the
/// downsample arm — ceil-divide, THEN double. This function computes
/// `2 * (w + 16 - 1) / 16` — double the NUMERATOR, then divide, because C's
/// `*` and `/` are left-associative and equal precedence. For `w = 17` the
/// first gives 4 and the second gives `(2*32)/16 == 4`… but for `w = 25` the
/// first gives `2 * 2 = 4` and the second gives `2*40/16 == 5`. Two functions
/// in the same file compute "the MB count" two different ways; both are
/// transcribed as written and neither is normalised to the other.
#[must_use]
pub fn twopass_mb_grid(
    first_pass_downsample: bool,
    max_input_luma_width: u32,
    max_input_luma_height: u32,
) -> (u32, u32) {
    if first_pass_downsample {
        // C: `2 * (w + 16 - 1) / 16` — left-to-right, so the DOUBLING is
        // inside the division. NOT `2 * w.div_ceil(16)`; see the doc comment.
        (
            (2 * (max_input_luma_width + 16 - 1)) / 16,
            (2 * (max_input_luma_height + 16 - 1)) / 16,
        )
    } else {
        (
            max_input_luma_width.div_ceil(16),
            max_input_luma_height.div_ceil(16),
        )
    }
}

/// C `get_twopass_worst_quality` (pass2_strategy.c:120). `static` — tier 4,
/// with the `SequenceControlSet` / `RATE_CONTROL` fields unpacked into
/// arguments.
///
/// `section_target_bandwidth <= 0` returns `worst_quality` unchanged — the
/// early-out is on the TARGET, before any MB math, so a zero-bandwidth section
/// never reaches the pow(). HONEST LIMIT: at exactly 0 that early-out is not
/// observable from the return value, because the fall-through would ask the
/// search for 0 bits/mb and the search saturates to `worst_quality` too. The
/// `<=` is transcribed from C; the test says so rather than claiming a
/// distinction it cannot make.
///
/// `active_mbs = AOMMAX(1, num_mbs - (int)(num_mbs * inactive_zone))` — the
/// multiply is `int * double`, so it is done in double and truncated toward
/// zero by the `(int)` cast, then subtracted. `target_norm_bits_per_mb` shifts
/// by [`BPER_MB_NORMBITS`] in `uint64_t` BEFORE dividing, so the shift cannot
/// lose the low bits; the port keeps the u64.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn get_twopass_worst_quality(
    first_pass_downsample: bool,
    max_input_luma_width: u32,
    max_input_luma_height: u32,
    encoder_bit_depth: u8,
    section_err: f64,
    inactive_zone: f64,
    section_target_bandwidth: i32,
    group_weight_factor: f64,
    under_shoot_pct: i32,
    over_shoot_pct: i32,
    best_quality: i32,
    worst_quality: i32,
) -> i32 {
    let (mb_cols, mb_rows) = twopass_mb_grid(
        first_pass_downsample,
        max_input_luma_width,
        max_input_luma_height,
    );
    let inactive_zone = fclamp(inactive_zone, 0.0, 1.0);
    if section_target_bandwidth <= 0 {
        return worst_quality; // Highest value allowed
    }
    let num_mbs = (mb_cols * mb_rows) as i32;
    let active_mbs = (num_mbs - (f64::from(num_mbs) * inactive_zone) as i32).max(1);
    let av_err_per_mb = section_err / f64::from(active_mbs);
    let target_norm_bits_per_mb =
        (((section_target_bandwidth as u64) << BPER_MB_NORMBITS) / active_mbs as u64) as i32;
    let rate_err_tol = under_shoot_pct.min(over_shoot_pct);
    find_qindex_by_rate_with_correction(
        target_norm_bits_per_mb,
        encoder_bit_depth,
        av_err_per_mb,
        group_weight_factor,
        rate_err_tol,
        best_quality,
        worst_quality,
    )
}

/// C `calculate_modified_err` (pass2_strategy.c:23). `static` — tier 4.
///
/// The whole body is `stats == NULL ? 0 : this_frame->stat_struct.total_num_bits`
/// — the name says "modified error" and the value is a raw bit count, because
/// the modification upstream libaom does was removed here. Keep the name; it
/// is what every call site reads.
#[must_use]
pub fn calculate_modified_err(has_total_stats: bool, total_num_bits: u64) -> f64 {
    if has_total_stats {
        total_num_bits as f64
    } else {
        0.0
    }
}
