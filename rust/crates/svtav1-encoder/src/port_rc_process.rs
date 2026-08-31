//! Port of `Codec/rc_process.c` — the rate-control rate model, the
//! qdelta-by-rate search, the RD-multiplier frame-type arms and the boost
//! helpers (lane `wp-ratecontrol` of the wholesale-port campaign).
//!
//! **Why this file exists.** The CQP/CRF qindex path already ported in
//! [`crate::rate_control`] covers the KEY frame. Every *inter* frame in
//! AOM_Q/CRF mode goes through `adjust_active_best_and_worst_quality`
//! (rc_crf_cqp.c:168-186), which is guarded by `if (!frame_is_intra_only)`
//! and calls `svt_av1_frame_type_qdelta` -> [`compute_qdelta_by_rate`] on
//! every one of them. The returned delta moves `active_worst_quality` and
//! therefore `base_q_idx`, so without it every inter frame is encoded at the
//! wrong qindex and the whole frame diverges.
//!
//! **Evidence.** Everything in this file that C exports is pinned at tier 1
//! (`WORKING-ON-THIS.md` §4) against the real symbol in
//! `Bin/Release/libSvtAv1Enc.a` by `tests/c_parity_rc_process.rs`:
//! `svt_av1_rc_bits_per_mb`, `svt_av1_compute_qdelta_by_rate`,
//! `svt_av1_get_cqp_kf_boost_from_r0`, `svt_av1_get_gfu_boost_from_r0_lap`,
//! `svt_av1_calculate_boost_bits`, and all seven const tables (which are
//! exported data symbols, so the table contents are compared against the
//! linked object rather than against a second transcription).
//! [`find_qindex_by_rate`] is `static` in C with no exported symbol, but it
//! is the entire second half of `compute_qdelta_by_rate`, so the tier-1
//! differential on that function drives it on every cell — a stronger
//! statement than a hand-derived vector suite would be.
//!
//! **Preprocessor check.** `grep -c '#if' rc_process.c` is 0 (positive
//! control: `rc_crf_cqp.c` is 17). There is no `#if TUNE_*` arm in this
//! file, so every line read here is a line mainline compiles.

use crate::rate_control::convert_qindex_to_q;

/// C `FrameType` (definitions.h:1605).
pub const KEY_FRAME: i32 = 0;
/// C `FrameType` (definitions.h:1606).
pub const INTER_FRAME: i32 = 1;

/// C `rate_factor_level` (rc_process.h:38).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum RateFactorLevel {
    InterNormal = 0,
    InterLow = 1,
    InterHigh = 2,
    GfArfLow = 3,
    GfArfStd = 4,
    KfStd = 5,
}

/// C `SvtAv1FrameUpdateType` (EbSvtAv1Enc.h:183).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum FrameUpdateType {
    KfUpdate = 0,
    LfUpdate = 1,
    GfUpdate = 2,
    ArfUpdate = 3,
    OverlayUpdate = 4,
    IntnlOverlayUpdate = 5,
    IntnlArfUpdate = 6,
}

// ---------------------------------------------------------------------------
// The const tables (rc_process.c:38-48)
// ---------------------------------------------------------------------------
//
// These are `extern const` data symbols consumed by `rc_crf_cqp.c`'s
// `crf_qindex_calc`; C's own regex-shaped inventories miss them because they
// are not function definitions. Each is pinned against the exported symbol.

/// C `svt_av1_non_base_qindex_weight_ref` (rc_process.c:38): the weight of the
/// reference frame's qindex in a non-base-layer frame's qindex blend.
pub const NON_BASE_QINDEX_WEIGHT_REF: [i32; 6] = [100, 100, 100, 100, 100, 100];

/// C `svt_av1_non_base_qindex_weight_wq` (rc_process.c:40): the weight of the
/// worst quality in the same blend. Note entry 2 is 300, not 100 — the table
/// is deliberately non-uniform and reading it as "all 100s" is easy.
pub const NON_BASE_QINDEX_WEIGHT_WQ: [i32; 6] = [100, 100, 300, 100, 100, 100];

/// C `svt_av1_tpl_hl_islice_div_factor` (rc_process.c:42).
pub const TPL_HL_ISLICE_DIV_FACTOR: [f64; 6] = [1.0, 2.0, 2.0, 1.0, 1.0, 0.7];

/// C `svt_av1_tpl_hl_base_frame_div_factor` (rc_process.c:43).
pub const TPL_HL_BASE_FRAME_DIV_FACTOR: [f64; 6] = [1.0, 3.0, 3.0, 2.0, 1.0, 1.0];

/// C `svt_av1_r0_weight` (rc_process.c:45): `[I_SLICE, BASE, NON-BASE]`.
pub const R0_WEIGHT: [f64; 3] = [0.75, 0.9, 1.0];

/// C `svt_av1_rate_factor_deltas` (rc_process.c:299), indexed by
/// [`RateFactorLevel`].
pub const RATE_FACTOR_DELTAS: [f64; 6] = [1.00, 1.00, 1.00, 1.50, 2.00, 2.00];

/// C `svt_av1_rate_factor_levels` (rc_process.c:308), indexed by
/// [`FrameUpdateType`], valued as [`RateFactorLevel`].
pub const RATE_FACTOR_LEVELS: [RateFactorLevel; 7] = [
    RateFactorLevel::KfStd,       // KF_UPDATE
    RateFactorLevel::InterNormal, // LF_UPDATE
    RateFactorLevel::GfArfStd,    // GF_UPDATE
    RateFactorLevel::GfArfStd,    // ARF_UPDATE
    RateFactorLevel::InterNormal, // OVERLAY_UPDATE
    RateFactorLevel::InterNormal, // INTNL_OVERLAY_UPDATE
    RateFactorLevel::GfArfLow,    // INTNL_ARF_UPDATE
];

/// C `R0_MIN_DIVISOR` (rc_process.c:228): r0 can legitimately be exactly 0 for
/// a zero-distortion frame, and `factor / 0.0` cast to `int` is UB in C
/// (UBSan: float-cast-overflow), so C floors the divisor here. The port keeps
/// the floor because the boost VALUE at r0 == 0 is defined by it.
const R0_MIN_DIVISOR: f64 = 1e-6;

// ---------------------------------------------------------------------------
// The rate model
// ---------------------------------------------------------------------------

/// C `svt_av1_rc_bits_per_mb` (rc_process.c:255).
///
/// The bit-rate model underneath every qdelta-by-rate decision: a fixed
/// enumerator per (frame type, screen-content) scaled by the correction factor
/// and divided by the "old Q" value of the qindex.
///
/// The C body ends in `(int)(enumerator * correction_factor / q)`, a C cast
/// that truncates toward zero; `as i32` in Rust is a saturating cast, which
/// differs from C only where the value overflows `int`. It cannot here: `q`
/// is at least `AC_QLOOKUP[0] / 4 = 1.0` at 8-bit (and larger at higher bit
/// depths), so with C's own `correction_factor <= MAX_BPB_FACTOR` (1.5) the
/// numerator is at most 2.1e6.
///
/// `frame_type` is [`KEY_FRAME`] / [`INTER_FRAME`]; `bit_depth` is the numeric
/// `EbBitDepth` (8 or 10 — the port's envelope; C also handles 12).
#[must_use]
pub fn rc_bits_per_mb(
    frame_type: i32,
    qindex: i32,
    correction_factor: f64,
    bit_depth: u8,
    is_screen_content_type: bool,
) -> i32 {
    let q = convert_qindex_to_q(qindex, bit_depth);
    let enumerator: f64 = if is_screen_content_type {
        if frame_type == KEY_FRAME {
            1_000_000.0
        } else {
            750_000.0
        }
    } else if frame_type == KEY_FRAME {
        1_400_000.0
    } else {
        1_000_000.0
    };
    (enumerator * correction_factor / q) as i32
}

/// C `find_qindex_by_rate` (rc_process.c:270). **`static` in C — no exported
/// symbol** (tier 4 on its own), but it is the whole second half of
/// [`compute_qdelta_by_rate`], which IS exported and is pinned at tier 1, so
/// every cell of that differential drives this search too.
///
/// A binary search for the smallest qindex in `[best_qindex, worst_qindex]`
/// whose modelled bits/mb is `<= desired_bits_per_mb`. C's `(low + high) >> 1`
/// is reproduced exactly: with `low <= high <= 255` there is no overflow, and
/// an arithmetic shift on a non-negative value is the same as a divide, but
/// the shape is kept because the search's landing point depends on the
/// rounding of the midpoint.
#[must_use]
pub fn find_qindex_by_rate(
    desired_bits_per_mb: i32,
    bit_depth: u8,
    frame_type: i32,
    is_screen_content_type: bool,
    best_qindex: i32,
    worst_qindex: i32,
) -> i32 {
    debug_assert!(best_qindex <= worst_qindex);
    let mut low = best_qindex;
    let mut high = worst_qindex;
    while low < high {
        let mid = (low + high) >> 1;
        let mid_bits_per_mb =
            rc_bits_per_mb(frame_type, mid, 1.0, bit_depth, is_screen_content_type);
        if mid_bits_per_mb > desired_bits_per_mb {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    low
}

/// C `svt_av1_compute_qdelta_by_rate` (rc_process.c:290) — **the inter
/// unblocker for this group**.
///
/// `rc_crf_cqp.c`'s `adjust_active_best_and_worst_quality` (:170-178) calls
/// `svt_av1_frame_type_qdelta` -> this on EVERY non-intra frame in AOM_Q/CRF
/// mode, and adds the result to `active_worst_quality`.
///
/// C takes a `RATE_CONTROL*` but reads exactly two fields off it —
/// `rc->best_quality` and `rc->worst_quality`, as the search bounds — so the
/// port takes them as arguments. Those two come from
/// `svt_av1_rc_init` copying `rc_cfg.best_allowed_q` / `worst_allowed_q`,
/// which `svt_aom_set_rc_param` sets to
/// `quantizer_to_qindex[min_qp_allowed]` / `[max_qp_allowed]`.
///
/// `(int)(rate_target_ratio * base_bits_per_mb)` is C's truncating cast; the
/// callers' ratios are the 1.0/1.5/2.0 of [`RATE_FACTOR_DELTAS`] and
/// `base_bits_per_mb` is bounded by the model above, so `as i32` cannot
/// saturate where C would wrap.
#[must_use]
pub fn compute_qdelta_by_rate(
    best_quality: i32,
    worst_quality: i32,
    frame_type: i32,
    qindex: i32,
    rate_target_ratio: f64,
    bit_depth: u8,
    is_screen_content_type: bool,
) -> i32 {
    // Look up the current projected bits per block for the base index.
    let base_bits_per_mb =
        rc_bits_per_mb(frame_type, qindex, 1.0, bit_depth, is_screen_content_type);
    // Find the target bits per mb based on the base value and given ratio.
    let target_bits_per_mb = (rate_target_ratio * f64::from(base_bits_per_mb)) as i32;
    let target_index = find_qindex_by_rate(
        target_bits_per_mb,
        bit_depth,
        frame_type,
        is_screen_content_type,
        best_quality,
        worst_quality,
    );
    target_index - qindex
}

/// C `svt_av1_frame_type_qdelta` (rc_crf_cqp.c:157). `static` in C.
///
/// Kept here rather than in the `rc_crf_cqp` port because it is a two-line
/// wrapper over [`compute_qdelta_by_rate`] and [`RATE_FACTOR_DELTAS`], both of
/// which live in `rc_process.c`.
///
/// The `GF_ARF_LOW` arm reads `rate_factor -= (0 - 2) * 0.1` in C — i.e.
/// `+= 0.2`, written that way because upstream folded a removed variable into
/// a literal. 1.5 + 0.2 = 1.7, then `AOMMAX(.., 1.0)` is a no-op. Transcribed
/// as written; the `AOMMAX` stays because removing it would be a judgement
/// about a value this function does not own.
#[must_use]
pub fn frame_type_qdelta(
    best_quality: i32,
    worst_quality: i32,
    rf_lvl: RateFactorLevel,
    q: i32,
    bit_depth: u8,
    sc_content_detected: bool,
) -> i32 {
    let frame_type = if rf_lvl == RateFactorLevel::KfStd {
        KEY_FRAME
    } else {
        INTER_FRAME
    };
    let mut rate_factor = RATE_FACTOR_DELTAS[rf_lvl as usize];
    if rf_lvl == RateFactorLevel::GfArfLow {
        rate_factor -= (0.0 - 2.0) * 0.1;
        rate_factor = rate_factor.max(1.0);
    }
    compute_qdelta_by_rate(
        best_quality,
        worst_quality,
        frame_type,
        q,
        rate_factor,
        bit_depth,
        sc_content_detected,
    )
}

// ---------------------------------------------------------------------------
// RD multiplier frame-type arms (rc_process.c:347-361)
// ---------------------------------------------------------------------------
//
// The port previously hardcoded ONLY the KF arm inline (`(3.3 + 0.0015*q)`
// at pd0.rs). The other two arms are what every inter frame uses.

/// C `def_inter_rd_multiplier` (rc_process.c:347). **`static` in C** — but it
/// is one of the three arms of the EXPORTED
/// `svt_aom_compute_rd_mult_based_on_qindex`, so
/// [`compute_rd_mult_based_on_qindex`] pins it at tier 1 for every
/// non-KF/GF/ARF update type.
#[must_use]
pub fn def_inter_rd_multiplier(qindex: f64) -> f64 {
    3.2 + 0.0015 * qindex
}

/// C `def_arf_rd_multiplier` (rc_process.c:354). `static`; same tier-1 route
/// as [`def_inter_rd_multiplier`], via `GF_UPDATE` / `ARF_UPDATE`.
#[must_use]
pub fn def_arf_rd_multiplier(qindex: f64) -> f64 {
    3.25 + 0.0015 * qindex
}

/// C `def_kf_rd_multiplier` (rc_process.c:361). `static`; same tier-1 route,
/// via `KF_UPDATE`. This is the arm the port already had inline.
#[must_use]
pub fn def_kf_rd_multiplier(qindex: f64) -> f64 {
    3.3 + 0.0015 * qindex
}

/// C `svt_aom_compute_rd_mult_based_on_qindex` (rc_process.c:365), EXPORTED.
///
/// Generalises what the port had as a KF-only inline expression to all seven
/// `SvtAv1FrameUpdateType`s. Note C's `int q = svt_aom_dc_quant_qtx(...)` —
/// the DC quantizer is taken as an **integer**, and the multiplier arms are
/// then evaluated on that integer, so the `q * q` term is exact.
#[must_use]
pub fn compute_rd_mult_based_on_qindex(
    bit_depth: u8,
    update_type: FrameUpdateType,
    qindex: i32,
) -> i32 {
    let q = f64::from(dc_quant_qtx_int(qindex, bit_depth));
    let rdmult: i64 = match update_type {
        FrameUpdateType::KfUpdate => (def_kf_rd_multiplier(q) * q * q) as i64,
        FrameUpdateType::GfUpdate | FrameUpdateType::ArfUpdate => {
            (def_arf_rd_multiplier(q) * q * q) as i64
        }
        _ => (def_inter_rd_multiplier(q) * q * q) as i64,
    };
    let rdmult = match bit_depth {
        8 => rdmult,
        10 => round_power_of_two_i64(rdmult, 4),
        12 => round_power_of_two_i64(rdmult, 8),
        other => panic!("compute_rd_mult_based_on_qindex: unsupported bit depth {other}"),
    };
    if rdmult > 0 {
        rdmult.min(i64::from(i32::MAX)) as i32
    } else {
        1
    }
}

/// C `ROUND_POWER_OF_TWO` on an `int64_t`.
fn round_power_of_two_i64(value: i64, n: u32) -> i64 {
    (value + (1 << (n - 1))) >> n
}

/// C's `svt_aom_dc_quant_qtx` result as the **integer** the RD-multiplier
/// arms consume. Delegates to the port's existing DC quantizer tables.
fn dc_quant_qtx_int(qindex: i32, bit_depth: u8) -> i32 {
    let i = qindex.clamp(0, 255) as usize;
    match bit_depth {
        8 => i32::from(svtav1_dsp::quant_tables::DC_QLOOKUP_8[i]),
        10 => i32::from(crate::bd10::DC_QLOOKUP_10[i]),
        other => panic!("dc_quant_qtx_int: unsupported bit depth {other}"),
    }
}

// ---------------------------------------------------------------------------
// Boost helpers (rc_process.c:230-252, 638)
// ---------------------------------------------------------------------------

/// C `svt_av1_get_cqp_kf_boost_from_r0` (rc_process.c:230), EXPORTED.
///
/// Called at rc_crf_cqp.c:243 for every key frame under TPL.
///
/// REACHABILITY, stated honestly per `WORKING-ON-THIS.md` §7: its only
/// consumer is `rc->kf_boost`, which is read only inside
/// `svt_aom_crf_assign_max_rate`, gated on `max_bit_rate != 0`
/// (rc_crf_cqp.c:533). At the current oracle's `max_bit_rate = 0` this value
/// is byte-inert. It is translated anyway — dead-looking C stays translated —
/// and it is pinned at tier 1 because the symbol is exported.
///
/// `frames_to_key == -1` means "not available"; C then uses the midpoint of
/// the clamp range rather than `sqrt(-1)`.
#[must_use]
pub fn get_cqp_kf_boost_from_r0(r0: f64, frames_to_key: i32, input_resolution: i32) -> i32 {
    let r0 = aom_max(r0, R0_MIN_DIVISOR);
    let factor = if frames_to_key == -1 {
        (10.0 + 4.0) / 2.0
    } else {
        // C's `AOMMIN` then `AOMMAX`, in that order — NOT `clamp`. On this
        // input they agree, but the macro order is the transcription and
        // `clamp` additionally panics when max < min, which C never does.
        aom_max(aom_min(f64::from(frames_to_key).sqrt(), 10.0), 4.0)
    };
    // `INPUT_SIZE_720p_RANGE` == 3 (definitions.h:1827).
    let scaled = if input_resolution <= 3 {
        3.0 * (75.0 + 17.0 * factor) / r0
    } else {
        4.0 * (75.0 + 17.0 * factor) / r0
    };
    // C's `rint` is round-half-to-even under the default rounding mode;
    // Rust's `round()` is round-half-away-from-zero, which differs on exact
    // .5 values. `round_ties_even` is the matching primitive.
    scaled.round_ties_even() as i32
}

/// C `svt_av1_get_gfu_boost_from_r0_lap` (rc_process.c:246), EXPORTED.
///
/// Called unconditionally on the non-intra arm of `crf_qindex_calc`
/// (rc_crf_cqp.c:268) once TPL is on — i.e. every inter frame in video-mode
/// CRF. Same honest caveat as [`get_cqp_kf_boost_from_r0`]: its consumer
/// `rc->gfu_boost` is read only under `max_bit_rate != 0`, so it is byte-inert
/// in the current envelope. Translated per §7, pinned at tier 1 for free.
///
/// Note C does NOT special-case `frames_to_key == -1` here (unlike the KF
/// twin), so a negative `frames_to_key` yields `sqrt` of a negative, i.e. NaN,
/// and both `AOMMIN`/`AOMMAX` propagate it. The port reproduces C's ternary
/// `AOMMIN`/`AOMMAX` shape exactly rather than Rust's `f64::min`/`max`, which
/// have the OPPOSITE NaN behaviour (they return the non-NaN operand). See
/// [`aom_min`] / [`aom_max`].
#[must_use]
pub fn get_gfu_boost_from_r0_lap(
    min_factor: f64,
    max_factor: f64,
    r0: f64,
    frames_to_key: i32,
) -> i32 {
    let r0 = r0.max(R0_MIN_DIVISOR);
    let mut factor = f64::from(frames_to_key).sqrt();
    factor = aom_min(factor, max_factor);
    factor = aom_max(factor, min_factor);
    factor = 200.0 + 10.0 * factor;
    (factor / r0).round_ties_even() as i32
}

/// C `AOMMIN(a, b)` == `((a) < (b) ? (a) : (b))`. When `a` is NaN the
/// comparison is false and `b` is returned; Rust's `f64::min` would return
/// `b` too, but when `b` is NaN C returns NaN and `f64::min` returns `a`.
/// The macro shape is what this reproduces.
#[must_use]
pub fn aom_min(a: f64, b: f64) -> f64 {
    if a < b { a } else { b }
}

/// C `AOMMAX(a, b)` == `((a) > (b) ? (a) : (b))`. Same NaN reasoning as
/// [`aom_min`].
#[must_use]
pub fn aom_max(a: f64, b: f64) -> f64 {
    if a > b { a } else { b }
}

/// C `svt_av1_calculate_boost_bits` (rc_process.c:638), EXPORTED.
///
/// Consumes the two boosts above inside `svt_aom_crf_assign_max_rate`
/// (rc_crf_cqp.c:76, 94), which is reachable only under capped CRF
/// (`max_bit_rate != 0`). Pure integer arithmetic; pinned at tier 1.
///
/// The `boost > 1023` block divides BOTH `boost` and `allocation_chunks` by
/// `boost >> 10` using C integer division, which is truncating; `i32`
/// division in Rust truncates the same way for the non-negative values this
/// branch can see (`boost > 1023`).
#[must_use]
pub fn calculate_boost_bits(frame_count: i32, mut boost: i32, total_group_bits: i64) -> i32 {
    // return 0 for invalid inputs (could arise e.g. through rounding errors)
    if boost == 0 || total_group_bits <= 0 {
        return 0;
    }
    if frame_count <= 0 {
        return total_group_bits.min(i64::from(i32::MAX)) as i32;
    }
    let mut allocation_chunks = frame_count * 100 + boost;
    // Prevent overflow.
    if boost > 1023 {
        let divisor = boost >> 10;
        boost /= divisor;
        allocation_chunks /= divisor;
    }
    let bits = (i64::from(boost) * total_group_bits) / i64::from(allocation_chunks);
    // C: AOMMAX((int)(...), 0) — the cast to int happens BEFORE the max, so a
    // value that overflows `int` wraps in C and is then compared against 0.
    // `total_group_bits` here is a group's bit budget, orders of magnitude
    // below the point where `boost * total_group_bits / allocation_chunks`
    // could exceed INT_MAX (boost <= allocation_chunks by construction once
    // frame_count >= 1, so the quotient is at most total_group_bits).
    (bits as i32).max(0)
}
