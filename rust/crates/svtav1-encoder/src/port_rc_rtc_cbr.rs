//! Port of the pure-scalar helpers of `Codec/rc_rtc_cbr.c` — the real-time
//! CBR rate controller.
//!
//! **COVERAGE, missing first.** `rc_rtc_cbr.c` is 721 lines and 24 functions.
//! This ports **3**: `find_closest_arg` (:73), `normalize_factors` (:175) and
//! `index2tl` (:186), the only three whose whole input is scalars or a plain
//! array. NOT ported: `clamp_qindex` (:22 — the same body already lives in
//! [`crate::port_rc_process::clamp_qindex`]), `get_ref_obj` (:28, ditto),
//! `get_min_ref_base_q_idx`, `av1_estimate_bits_at_qindex`, `eval_block_bits`,
//! `rtc_compute_cr_deltaq`, `rtc_cyclic_refresh_compute_cr_qdeltas`,
//! `av1_estimate_frame_size`, `eval_frame_size`, `calc_pframe_target_size`,
//! `cr_select_sbs`, `rtc_cyclic_refresh_init`, `get_rcf_index`,
//! `rtc_get_rate_correction_factor`, `rtc_set_rate_correction_factor`,
//! `calculate_qindex`, `svt_av1_rc_calc_qindex_rtc_cbr`,
//! `rtc_update_rate_correction_factors`, `rtc_update_buffer_level`,
//! `svt_av1_rc_recode_decision_rtc_cbr`, `svt_av1_rc_postencode_update_rtc_cbr`
//! — 21 functions, every one of which reads a `PictureControlSet`,
//! `PictureParentControlSet` or `CyclicRefresh`.
//!
//! **EVIDENCE: TIER 4** for all three. Every function in this file is `static`
//! in C with no exported symbol; `nm -g` on `Bin/Release/libSvtAv1Enc.a`
//! exports only `svt_av1_rc_calc_qindex_rtc_cbr`,
//! `svt_av1_rc_postencode_update_rtc_cbr` and
//! `svt_av1_rc_recode_decision_rtc_cbr` from this file, none of which is here.
//! The expected values in `tests/rc_rtc_cbr_scalars.rs` are literals derived by
//! hand from the C statements quoted beside them.
//!
//! **REACHABILITY: none in the port's envelope.** The whole file is behind
//! `use_rtc_cbr_path` (rc_process.c:34) — `rc_cfg.mode == AOM_CBR &&
//! static_config.rtc` — and CQP/CRF sets `AOM_Q`. Translated because the
//! directive is to leave nothing untranslated, with the reachability written
//! down here rather than assumed (`WORKING-ON-THIS.md` §7).

/// C `find_closest_arg` (rc_rtc_cbr.c:73). `static` — tier 4.
///
/// A generic binary search over an integer argument for the value of `eval`
/// closest to `target`. C's comment says "eval() must be monotonically
/// DECREASING with arg", which is the opposite direction from
/// `find_qindex_by_rate`'s implicit contract — hence the `mid_val > target`
/// test moving `lo` UP.
///
/// THE SECOND HALF IS THE PART THAT GETS DROPPED. After the binary search
/// lands on the first arg whose value is `<= target`, C looks back ONE step
/// and takes the neighbour when it is strictly closer:
/// `if (fabs(prev_val - target) < fabs(curr_val - target)) curr_arg = lo - 1;`
/// A plain lower-bound search returns `lo` and is wrong by one on roughly half
/// its inputs. The tie goes to `curr_arg` (`<`, not `<=`).
///
/// `eval` is called AGAIN for both `lo - 1` and `lo` in that step — C does not
/// reuse `mid_val` — so an `eval` with side effects would see the extra calls.
/// The port keeps the same call pattern.
pub fn find_closest_arg<F: FnMut(i32) -> f64>(
    target: f64,
    min_arg: i32,
    max_arg: i32,
    mut eval: F,
) -> i32 {
    let mut lo_arg = min_arg;
    let mut hi_arg = max_arg;
    while lo_arg < hi_arg {
        let mid_arg = (lo_arg + hi_arg) >> 1;
        let mid_val = eval(mid_arg);
        if mid_val > target {
            lo_arg = mid_arg + 1;
        } else {
            hi_arg = mid_arg;
        }
    }
    let mut curr_arg = lo_arg;
    if curr_arg > min_arg {
        let prev_val = eval(lo_arg - 1);
        let curr_val = eval(lo_arg);
        if (prev_val - target).abs() < (curr_val - target).abs() {
            curr_arg = lo_arg - 1;
        }
    }
    curr_arg
}

/// C `normalize_factors` (rc_rtc_cbr.c:175). `static` — tier 4.
///
/// Divides `src[i_start..i_end]` by a weighted average and writes the result
/// to `dst` over the same range.
///
/// **The weights are `1 << max(k - i_start - 1, 0)` — note the `- 1`**, so the
/// first TWO entries both get weight 1 and the doubling only starts at the
/// third: 1, 1, 2, 4, 8, ... For a range of length `n >= 1` those sum to
/// `1 + (2^(n-1) - 1) = 2^(n-1)`, which is exactly C's divisor
/// `1 << max(i_end - i_start - 1, 0)`. So it IS a weighted mean, and the
/// off-by-one in the exponent is what makes the two halves line up — "fix" the
/// `- 1` in either place alone and it stops being one.
///
/// C writes `dst` and reads `src` as separate pointers; every call site passes
/// two different arrays, so the port takes two slices and does not alias them.
/// Indices outside `[i_start, i_end)` are left untouched, exactly as C leaves
/// them.
pub fn normalize_factors(dst: &mut [f64], src: &[f64], i_start: usize, i_end: usize) {
    let mut sum = 0.0f64;
    for k in i_start..i_end {
        let shift = (k as i64 - i_start as i64 - 1).max(0) as u32;
        sum += src[k] * f64::from(1u32 << shift);
    }
    let avg_shift = (i_end as i64 - i_start as i64 - 1).max(0) as u32;
    let avg_factor = sum / f64::from(1u32 << avg_shift);
    for k in i_start..i_end {
        dst[k] = src[k] / avg_factor;
    }
}

/// C `index2tl` (rc_rtc_cbr.c:186). `static` — tier 4.
///
/// `return index ? levels - get_msb(index ^ (index - 1)) : 0;`
///
/// `index ^ (index - 1)` is the classic "low bit plus everything below it"
/// mask, so `get_msb` of it is the index of the lowest SET bit — i.e.
/// `index.trailing_zeros()`. The port spells it as `trailing_zeros` and says
/// why, because `get_msb(x ^ (x-1))` reads like a highest-bit operation and
/// is not one.
///
/// `get_msb` (definitions.h:617) asserts `n != 0` and is undefined at 0; the
/// `index ? ... : 0` guard is what keeps it out of that case, since
/// `index ^ (index - 1)` is 0 only when `index` is 0. The port takes `u32` so
/// the negative-index question cannot arise.
#[must_use]
pub fn index2tl(index: u32, levels: i32) -> i32 {
    if index == 0 {
        0
    } else {
        levels - index.trailing_zeros() as i32
    }
}
