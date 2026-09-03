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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(i32)]
pub enum FrameUpdateType {
    #[default]
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

// ---------------------------------------------------------------------------
// Sequence-level RC setup: `svt_aom_set_rc_param` + `svt_av1_rc_init`
// ---------------------------------------------------------------------------
//
// These two run once per sequence and produce `rc.best_quality` /
// `rc.worst_quality` — the clamp bounds inside `compute_qdelta_by_rate` above,
// and the `AOMMAX`/`clamp` bounds in `adjust_active_best_and_worst_quality`.
// Neither had a Rust counterpart: the port's inventory matched
// `svt_av1_rc_init` against the STRING "svt_av1_rc_init_sb_qindex" inside two
// doc comments (sb_qindex.rs:145, :265) and reported it ported. It was not.

/// C `enum aom_rc_mode` (encoder.h:32).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum AomRcMode {
    Vbr = 0,
    Cbr = 1,
    Q = 2,
}

/// C `SvtAv1RcMode` (EbSvtAv1Enc.h:177) — the CONFIG-side enum, whose numbering
/// is NOT `AomRcMode`'s. `set_rc_param` is the translation between them, and
/// mixing them up silently selects the wrong rate-control mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum SvtAv1RcMode {
    CqpOrCrf = 0,
    Vbr = 1,
    Cbr = 2,
}

/// The fields of `SequenceControlSet` that C's `svt_aom_set_rc_param` reads.
#[derive(Clone, Copy, Debug, Default)]
pub struct SetRcParamInput {
    pub first_pass_downsample: bool,
    pub max_input_luma_width: u32,
    pub max_input_luma_height: u32,
    pub encoder_bit_depth: i32,
    pub vbr_min_section_pct: i32,
    pub vbr_max_section_pct: i32,
    /// A [`SvtAv1RcMode`] discriminant.
    pub rate_control_mode: i32,
    pub min_qp_allowed: i32,
    pub max_qp_allowed: i32,
    pub gop_constraint_rc: bool,
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

/// The `EncodeContext` fields C's `svt_aom_set_rc_param` writes: `frame_info`,
/// `two_pass_cfg`, `rc_cfg` and `sf_cfg`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SetRcParamOutput {
    pub frame_width: i32,
    pub frame_height: i32,
    pub mb_rows: i32,
    pub mb_cols: i32,
    pub num_mbs: i32,
    pub bit_depth: i32,
    pub vbrmin_section: i32,
    pub vbrmax_section: i32,
    /// An [`AomRcMode`] discriminant.
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

/// C `svt_aom_set_rc_param` (pass2_strategy.c:906), EXPORTED.
///
/// REACHED IN THE PORT'S EXACT CONFIGURATION: resource_coordination_process.c
/// :1074-1078 takes this branch when `pass == ENC_SINGLE_PASS && !lap_rc`, and
/// `lap_rc` is 1 only for VBR single-pass (enc_handle.c:4623-4627).
///
/// Its `best_allowed_q` / `worst_allowed_q` become `rc->best_quality` /
/// `rc->worst_quality` via [`rc_init`], and those are the clamp bounds inside
/// [`compute_qdelta_by_rate`] — so the inter qindex path is wrong without it.
///
/// INDEX-ORDER NOTE, since this is exactly the class of thing that gets
/// inferred wrongly: on the `first_pass_downsample` arm the MB counts are
/// `((w + 15) / 16) << 1` — the ceiling division happens on the ORIGINAL
/// width and the RESULT is doubled. That is not the same as
/// `(2w + 15) / 16` whenever `w % 16` is in 1..=8, so a "double the width
/// first" reading is wrong on most widths.
#[must_use]
pub fn set_rc_param(input: &SetRcParamInput) -> SetRcParamOutput {
    let mut out = SetRcParamOutput::default();
    let w = input.max_input_luma_width as i32;
    let h = input.max_input_luma_height as i32;
    if input.first_pass_downsample {
        out.frame_width = w << 1;
        out.frame_height = h << 1;
        out.mb_cols = ((w + 16 - 1) / 16) << 1;
        out.mb_rows = ((h + 16 - 1) / 16) << 1;
    } else {
        out.frame_width = w;
        out.frame_height = h;
        out.mb_cols = (w + 16 - 1) / 16;
        out.mb_rows = (h + 16 - 1) / 16;
    }
    out.num_mbs = out.mb_cols * out.mb_rows;
    out.bit_depth = input.encoder_bit_depth;
    out.vbrmin_section = input.vbr_min_section_pct;
    out.vbrmax_section = input.vbr_max_section_pct;
    out.mode = if input.rate_control_mode == SvtAv1RcMode::Vbr as i32 {
        AomRcMode::Vbr as i32
    } else if input.rate_control_mode == SvtAv1RcMode::Cbr as i32 {
        AomRcMode::Cbr as i32
    } else {
        AomRcMode::Q as i32
    };
    out.best_allowed_q = i32::from(quantizer_to_qindex(input.min_qp_allowed));
    out.worst_allowed_q = i32::from(quantizer_to_qindex(input.max_qp_allowed));
    if input.gop_constraint_rc {
        out.over_shoot_pct = 0;
        out.under_shoot_pct = 0;
    } else {
        out.over_shoot_pct = input.over_shoot_pct;
        out.under_shoot_pct = input.under_shoot_pct;
    }
    let is_vbr = out.mode == AomRcMode::Vbr as i32;
    out.maximum_buffer_size_ms = if is_vbr {
        240_000
    } else {
        input.maximum_buffer_size_ms
    };
    out.starting_buffer_level_ms = if is_vbr {
        60_000
    } else {
        input.starting_buffer_level_ms
    };
    out.optimal_buffer_level_ms = if is_vbr {
        60_000
    } else {
        input.optimal_buffer_level_ms
    };
    out.max_intra_bitrate_pct = input.max_intra_bitrate_pct;
    out.max_inter_bitrate_pct = input.max_inter_bitrate_pct;
    out.sframe_dist = input.sframe_dist;
    out.sframe_mode = input.sframe_mode;
    out
}

/// C's `quantizer_to_qindex[qp]`. Delegates to the port's existing table so
/// there is exactly one copy; the differential on [`set_rc_param`] pins it,
/// because C indexes its own table on the same `min_qp_allowed` / `max_qp_allowed`.
fn quantizer_to_qindex(qp: i32) -> u8 {
    crate::rate_control::QUANTIZER_TO_QINDEX[qp.clamp(0, 63) as usize]
}

/// The `RATE_CONTROL` fields C's `svt_av1_rc_init` reads before writing.
#[derive(Clone, Copy, Debug, Default)]
pub struct RcInitInput {
    /// An [`AomRcMode`] discriminant (i.e. `rc_cfg.mode`, the OUTPUT of
    /// [`set_rc_param`] — not the config-side [`SvtAv1RcMode`]).
    pub mode: i32,
    pub best_allowed_q: i32,
    pub worst_allowed_q: i32,
    pub starting_buffer_level: i64,
    pub avg_frame_bandwidth: i32,
    pub hierarchical_levels: i32,
}

/// The `RATE_CONTROL` fields C's `svt_av1_rc_init` writes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RcInitOutput {
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

/// C `svt_av1_rc_init` (rc_process.c:495), EXPORTED.
///
/// **This function was recorded as "ported" and is not.** The port's inventory
/// matched it by name against the string `svt_av1_rc_init_sb_qindex` inside
/// doc comments at `sb_qindex.rs:145` and `:265`; there was no implementation.
/// rc_crf_cqp.c:487 calls it at `picture_number == 0` under TPL.
///
/// It produces `rc->best_quality` / `rc->worst_quality` (the
/// [`compute_qdelta_by_rate`] clamp) and the two counters that make the
/// CQP/CRF CDF story work — see [`RcInitOutput::frames_since_key`].
///
/// LOAD-BEARING NON-EXECUTION, do NOT "fix" it: in CQP/CRF mode
/// `svt_aom_update_rc_counts` never runs (rc_process.c:791 is on the
/// non-AOM_Q arm; packetization_process.c:602 is under
/// `if (scs->static_config.rate_control_mode)` and CQP_OR_CRF == 0), so
/// `frames_since_key` stays at this seed of 8 and `frames_since_cdf_update`
/// stays 0 for the WHOLE sequence regardless of frame count.
/// `should_disable_cdf_update` (enc_mode_config.c:9484-9501) reads exactly
/// those two and therefore evaluates `8 >= 30 && 0 < 8` == false forever, so
/// `disable_cdf_update` is always 0 in this envelope. A port that helpfully
/// increments the counters diverges from C on long GOPs at fast presets.
///
/// INDEX TRAP: `rate_correction_factors` is sized `MAX_TEMPORAL_LAYERS + 1`
/// (7) but the non-CBR override writes `[KF_STD]`, and `KF_STD` is a
/// [`RateFactorLevel`] == 5, not a temporal-layer index. Two different index
/// spaces share one array.
///
/// NOT PORTED HERE (and deliberately): the `mode != AOM_Q` tail calls
/// `svt_av1_new_framerate` -> `av1_rc_update_framerate`, which belongs to
/// `rc_vbr_cbr.c`'s surface and computes `avg_frame_bandwidth` /
/// `max_frame_bandwidth` from the target bit rate. This function returns the
/// AOM_Q-complete result; a VBR/CBR caller must run that step itself. Stated
/// rather than silently omitted.
#[must_use]
pub fn rc_init(input: &RcInitInput) -> RcInitOutput {
    let cbr = input.mode == AomRcMode::Cbr as i32;
    let seed = if cbr {
        input.worst_allowed_q
    } else {
        (input.worst_allowed_q + input.best_allowed_q) / 2
    };
    let mut rate_correction_factors = [0.7f64; 7];
    if !cbr {
        rate_correction_factors[RateFactorLevel::KfStd as usize] = 1.0;
    }
    RcInitOutput {
        avg_frame_qindex_key: seed,
        avg_frame_qindex_inter: seed,
        last_q_key: seed,
        last_q_inter: seed,
        buffer_level: input.starting_buffer_level,
        bits_off_target: input.starting_buffer_level,
        rolling_target_bits: input.avg_frame_bandwidth,
        rolling_actual_bits: input.avg_frame_bandwidth,
        total_actual_bits: 0,
        total_target_bits: 0,
        // "Sensible default for first frame" — C's own comment. See the
        // load-bearing-non-execution note above: in CQP/CRF this 8 is the
        // value for the entire sequence.
        frames_since_key: 8,
        frames_since_cdf_update: 0,
        this_key_frame_forced: 0,
        rate_correction_factors,
        baseline_gf_interval: 1 << input.hierarchical_levels,
        worst_quality: input.worst_allowed_q,
        best_quality: input.best_allowed_q,
        cur_avg_base_me_dist: 0,
        prev_avg_base_me_dist: 0,
        avg_frame_low_motion: 0,
    }
}

// ---------------------------------------------------------------------------
// The MD lambda chain (rc_process.c:398-489)
// ---------------------------------------------------------------------------
//
// `av1_lambda_assign_md` (md_process.c:724-728) sets `fast_lambda_md[0]`/`[1]`
// from `svt_aom_compute_fast_lambda` on EVERY frame, and MDS0 candidate
// pruning uses `fast_lambda_md[1]` — so a wrong frame-type factor changes the
// candidate set, not just a cost. The port had only the KF arm, inline, at
// five sites in `pd0.rs`; this is the general chain.

/// C's `av1_lambda_mode_decision{8,10,12}_bit_sad` tables — see the module
/// docs there for why they are extracted rather than bound.
pub mod lambda_tables {
    include!("port_rc_lambda_tables.rs");
}

/// C `rd_frame_type_factor[2][SVT_AV1_FRAME_UPDATE_TYPES]` (rc_process.c:398).
///
/// **THE FIRST INDEX IS A BOOLEAN** — `bit_depth != EB_EIGHT_BIT` — not a
/// bit-depth ordinal. Row 0 is 8-bit; row 1 is 10- AND 12-bit. Reading it as
/// `[bit_depth]` would index out of range or silently pick row 1 for 8-bit.
pub const RD_FRAME_TYPE_FACTOR: [[i32; 7]; 2] = [
    [150, 180, 150, 150, 180, 180, 150],
    [128, 144, 128, 128, 144, 144, 128],
];

/// C `rd_frame_type_factor_alt[SVT_AV1_FRAME_UPDATE_TYPES]`
/// (rc_process.c:400) — used when `static_config.alt_lambda_factors` is set,
/// and NOT bit-depth-indexed at all.
pub const RD_FRAME_TYPE_FACTOR_ALT: [i32; 7] = [140, 180, 128, 140, 164, 164, 140];

/// C `RTC_KF_LAMBDA_BOOST` (rc_process.c:401).
pub const RTC_KF_LAMBDA_BOOST: i64 = 100;

/// The PCS/PPCS/SCS fields C's `update_lambda` (rc_process.c:404) reads.
#[derive(Clone, Copy, Debug, Default)]
pub struct LambdaContext {
    /// `ppcs->frm_hdr.frame_type`; [`KEY_FRAME`] == 0.
    pub frame_type: i32,
    pub temporal_layer_index: u8,
    /// `ppcs->hierarchical_levels`, used as the MAX temporal layer.
    pub hierarchical_levels: u8,
    /// `ppcs->update_type`, read by [`compute_rd_mult`] (not by
    /// `update_lambda`, which derives its OWN `gf_update_type`).
    pub update_type: FrameUpdateType,
    pub alt_lambda_factors: bool,
    pub rtc: bool,
    pub stats_based_sb_lambda_modulation: bool,
    pub base_q_idx: i32,
    pub delta_q_present: bool,
    pub r0_delta_qp_md: bool,
    /// `scs->static_config.lambda_scale_factors`, indexed by
    /// [`FrameUpdateType`]. 128 is the identity.
    pub lambda_scale_factors: [i32; 7],
}

/// C `update_lambda` (rc_process.c:404). **`static` in C** — no exported
/// symbol — but every one of its inputs is on [`LambdaContext`], and the two
/// EXPORTED functions that wrap it ([`compute_rd_mult`] and
/// [`compute_fast_lambda`]) are pinned at tier 1 over a sweep of all of them,
/// which drives all four branches of the `stats_based` block.
///
/// THE TRAP HERE, spelled out because it is precisely the "assume the index is
/// what it looks like" failure: `gf_update_type` is **derived inside this
/// function** from `frame_type` and the temporal layer — it is NOT
/// `ppcs->update_type`, which the caller also has and which
/// [`compute_rd_mult`] uses for a DIFFERENT lookup. The two disagree (an
/// INTNL_ARF frame's `update_type` and its `gf_update_type` can differ), so
/// substituting one for the other is a silent lambda error.
///
/// And `rd_frame_type_factor`'s first index is the BOOLEAN
/// `bit_depth != EB_EIGHT_BIT`. See [`RD_FRAME_TYPE_FACTOR`].
#[must_use]
pub fn update_lambda(
    ctx: &LambdaContext,
    q_index: u8,
    me_q_index: u8,
    bit_depth: u8,
    mut rdmult: i64,
) -> u32 {
    let temporal_layer_index = ctx.temporal_layer_index;
    let max_temporal_layer = ctx.hierarchical_levels;

    // Update rdmult based on the frame's position in the miniGOP.
    let gf_update_type = if ctx.frame_type == KEY_FRAME {
        FrameUpdateType::KfUpdate
    } else if temporal_layer_index == 0 {
        FrameUpdateType::ArfUpdate
    } else if temporal_layer_index < max_temporal_layer {
        FrameUpdateType::IntnlArfUpdate
    } else {
        FrameUpdateType::LfUpdate
    };

    if ctx.alt_lambda_factors {
        rdmult = (rdmult * i64::from(RD_FRAME_TYPE_FACTOR_ALT[gf_update_type as usize])) >> 7;
    } else {
        // First index: the BOOLEAN `bit_depth != EB_EIGHT_BIT`.
        let hbd = usize::from(bit_depth != 8);
        rdmult = (rdmult * i64::from(RD_FRAME_TYPE_FACTOR[hbd][gf_update_type as usize])) >> 7;
    }

    if ctx.rtc && ctx.frame_type == KEY_FRAME {
        rdmult = (rdmult * RTC_KF_LAMBDA_BOOST) >> 7;
    }

    if ctx.stats_based_sb_lambda_modulation {
        let mut factor: i64 = 128;
        if ctx.rtc {
            let qdiff = i32::from(me_q_index) - ctx.base_q_idx;
            if qdiff < 0 {
                factor = if qdiff <= -4 { 100 } else { 115 };
            }
        } else if ctx.delta_q_present || ctx.r0_delta_qp_md {
            // NOTE this arm uses `q_index`, the other two use `me_q_index`.
            let qdiff = i32::from(q_index) - ctx.base_q_idx;
            if qdiff < 0 {
                factor = if qdiff <= -8 { 90 } else { 115 };
            } else if qdiff > 0 {
                factor = if qdiff <= 8 { 135 } else { 150 };
            }
        } else {
            let qdiff = i32::from(me_q_index) - ctx.base_q_idx;
            if qdiff < 0 {
                factor = if qdiff <= -4 { 100 } else { 115 };
            } else if qdiff > 0 {
                factor = if qdiff <= 4 { 135 } else { 150 };
            }
        }
        rdmult = (rdmult * factor) >> 7;
    }
    // C's `return (uint32_t)rdmult` — a C truncating conversion, which for a
    // negative or >2^32 value wraps. `as u32` in Rust is the same wrapping
    // conversion for `i64`, so this is the faithful spelling.
    rdmult as u32
}

/// C `svt_aom_compute_rd_mult` (rc_process.c:456), EXPORTED.
///
/// Note C's comment and code: the initial rdmult base uses `q_index`, NEVER
/// `me_q_index`; `me_q_index` only reaches [`update_lambda`]'s stats-based
/// factor.
#[must_use]
pub fn compute_rd_mult(ctx: &LambdaContext, q_index: u8, me_q_index: u8, bit_depth: u8) -> u32 {
    let rdmult = i64::from(compute_rd_mult_based_on_qindex(
        bit_depth,
        ctx.update_type,
        i32::from(q_index),
    ));
    update_lambda(ctx, q_index, me_q_index, bit_depth, rdmult)
}

/// C `svt_aom_compute_fast_lambda` (rc_process.c:461), EXPORTED — queue
/// item #6.
///
/// `av1_lambda_assign_md` (md_process.c:724-728) sets `fast_lambda_md[0]` and
/// `[1]` from this on every frame; MDS0 candidate pruning reads
/// `fast_lambda_md[1]`, so the frame-type factor changes the CANDIDATE SET.
///
/// C's ternary is `bit_depth == EB_EIGHT_BIT ? 8bit_sad : 10bit_sad` — the
/// **12-bit table is not reachable from this function**, so a 12-bit call
/// takes the 10-bit table. Transcribed as written; it looks like an upstream
/// slip but a C bug is still the oracle (`docs/SUSPECTED-C-BUGS.md` is where
/// that belongs, not a "fix" here).
#[must_use]
pub fn compute_fast_lambda(ctx: &LambdaContext, q_index: u8, me_q_index: u8, bit_depth: u8) -> u32 {
    let rdmult: i64 = if bit_depth == 8 {
        i64::from(lambda_tables::LAMBDA_MODE_DECISION_8BIT_SAD[q_index as usize])
    } else {
        i64::from(lambda_tables::LAMBDA_MODE_DECISION_10BIT_SAD[q_index as usize])
    };
    update_lambda(ctx, q_index, me_q_index, bit_depth, rdmult)
}

/// C `svt_aom_lambda_assign` (rc_process.c:469), EXPORTED. Returns
/// `(fast_lambda, full_lambda)`.
///
/// `multiply_lambda` is 10-bit-ONLY in C: the `*= 16` / `*= 4` pair sits
/// inside the `EB_TEN_BIT` arm, so passing `true` at 8- or 12-bit does
/// nothing. And `fast_lambda` here is the RAW table entry — it does NOT go
/// through [`update_lambda`], unlike [`compute_fast_lambda`]'s result. Those
/// are two different "fast lambdas" and conflating them is easy.
///
/// **12-BIT IS INCOMPLETE, and says so by panicking rather than by returning
/// a plausible number** (`WORKING-ON-THIS.md` §6). The 12-bit `fast_lambda`
/// is available — [`lambda_tables::LAMBDA_MODE_DECISION_12BIT_SAD`] is
/// pinned entry-for-entry — but `full_lambda` needs
/// `svt_aom_dc_quant_qtx(_, _, EB_TWELVE_BIT)`, and the port has no 12-bit DC
/// quantizer table (the same reason `rate_control::convert_qindex_to_q`
/// panics at 12). So the differential sweeps 8 and 10 only; 12-bit
/// `lambda_assign` is NOT ported and is listed as missing.
#[must_use]
pub fn lambda_assign(
    ctx: &LambdaContext,
    bit_depth: u8,
    qp_index: u8,
    multiply_lambda: bool,
) -> (u32, u32) {
    let q = qp_index as usize;
    let (mut fast_lambda, mut full_lambda) = match bit_depth {
        8 => (
            lambda_tables::LAMBDA_MODE_DECISION_8BIT_SAD[q],
            compute_rd_mult(ctx, qp_index, qp_index, bit_depth),
        ),
        10 => {
            let full = compute_rd_mult(ctx, qp_index, qp_index, bit_depth);
            let fast = lambda_tables::LAMBDA_MODE_DECISION_10BIT_SAD[q];
            if multiply_lambda {
                (fast.wrapping_mul(4), full.wrapping_mul(16))
            } else {
                (fast, full)
            }
        }
        12 => (
            lambda_tables::LAMBDA_MODE_DECISION_12BIT_SAD[q],
            compute_rd_mult(ctx, qp_index, qp_index, bit_depth),
        ),
        other => panic!("lambda_assign: unsupported bit depth {other}"),
    };
    // NM: To be done: tune lambda based on the picture type and layer.
    let scale_factor = i64::from(ctx.lambda_scale_factors[ctx.update_type as usize]);
    full_lambda = ((i64::from(full_lambda) * scale_factor) >> 7) as u32;
    fast_lambda = ((i64::from(fast_lambda) * scale_factor) >> 7) as u32;
    (fast_lambda, full_lambda)
}

// ---------------------------------------------------------------------------
// The reference-frame percentages and the per-frame RC entry
// (rc_process.c:61-142, :604-632)
// ---------------------------------------------------------------------------
//
// EVIDENCE TIER 4 FOR THIS SECTION, and it is stated once here rather than
// implied: `get_ref_obj`, `get_ref_intra_percentage`, `get_ref_skip_percentage`,
// `get_ref_hp_percentage` and `rc_init_frame_stats` are ALL `static` in
// `rc_process.c` with NO exported symbol (checked with `nm -g` on
// `Bin/Release/libSvtAv1Enc.a`, whose positive controls
// `svt_av1_rc_bits_per_mb` / `svt_aom_compute_rd_mult` are present, so a
// "not exported" verdict there is trustworthy). They also cannot be reached
// through any exported wrapper without standing up the whole rate-control
// thread and its fifos. So they are pinned by HAND-DERIVED VECTORS TRACED
// AGAINST THE C SOURCE — the weakest tier in `WORKING-ON-THIS.md` §4 — and
// the tests spell out the arithmetic per vector rather than re-running the
// port's own algorithm.
//
// These are inter-only (each returns a constant on an I_SLICE), so they are
// invisible to the still envelope and silently wrong the moment inter frames
// exist. Their consumers: `ref_intra_percentage` at md_process.c:738,
// rc_crf_cqp.c:332, enc_dec_process.c:2466, enc_mode_config.c:6653/9027;
// `ref_skip_percentage` at eight sites in enc_mode_config.c and
// md_config_process.c:972 (preset-level TOOL GATING — a wrong value changes
// the tool set and therefore the bitstream); `ref_hp_percentage` at
// enc_mode_config.c:8962 and :9580 against HIGH_PRECISION_REF_PERC_TH, which
// decides `allow_high_precision_mv` — a frame-header bit AND an MV-coding
// change.

/// C `SliceType` (definitions.h:1890). **`B_SLICE` is 0 and `I_SLICE` is 1** —
/// the opposite of the ordering most people assume, and every helper below
/// branches on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum SliceType {
    B = 0,
    I = 1,
}

/// The `EbReferenceObject` fields the three percentage helpers read
/// (reference_object.h:31-33). All three coded-area fields are **`uint8_t`**
/// in C; that matters below.
#[derive(Clone, Copy, Debug)]
pub struct RefObjStats {
    pub slice_type: SliceType,
    pub intra_coded_area: u8,
    pub skip_coded_area: u8,
    pub hp_coded_area: u8,
}

/// C `get_ref_obj` (rc_process.c:61): `pcs->ref_pic_ptr_array[list][idx]->object_ptr`.
///
/// A one-line DPB accessor that #7-#9 are built on. Each RC file has its own
/// `static` copy of it in C; the port needs exactly one. Returns `None` when
/// the slot is empty, which C cannot express — C would dereference a null
/// `object_ptr`, so `None` is the port refusing rather than reproducing UB.
#[must_use]
pub fn get_ref_obj(
    ref_pic_array: &[[Option<RefObjStats>; 2]],
    ref_list: usize,
    idx: usize,
) -> Option<&RefObjStats> {
    ref_pic_array.get(idx)?[ref_list].as_ref()
}

/// C `get_ref_intra_percentage` (rc_process.c:66). **`static` — tier 4.**
///
/// Sets `pcs->ref_intra_percentage`: the mean intra-coded area of the two
/// nearest reference frames, skipping any that is itself an I_SLICE.
///
/// THE OVERFLOW IS REAL AND IS REPRODUCED: `iperc` is `uint8_t` in C, so
/// `iperc += ref_obj_l1->intra_coded_area` WRAPS at 256 before the divide.
/// Two references at 200% each (which the field's 0-100 contract forbids but
/// its type permits) give C `(200 + 200) mod 256 = 144`, then `144 / 2 = 72`.
/// `wrapping_add` is the faithful spelling; a `u16` accumulator would be a
/// "fix" that diverges.
#[must_use]
pub fn get_ref_intra_percentage(
    slice_type: SliceType,
    ref_list1_count_try: u8,
    ref_l0: Option<&RefObjStats>,
    ref_l1: Option<&RefObjStats>,
) -> u8 {
    if slice_type == SliceType::I {
        return 100;
    }
    let mut iperc: u8 = 0;
    let mut ref_cnt: u8 = 0;
    if let Some(l0) = ref_l0
        && l0.slice_type != SliceType::I
    {
        iperc = l0.intra_coded_area;
        ref_cnt += 1;
    }
    // C's guard is `pcs->slice_type == B_SLICE && pcs->ppcs->ref_list1_count_try`.
    if slice_type == SliceType::B
        && ref_list1_count_try != 0
        && let Some(l1) = ref_l1
        && l1.slice_type != SliceType::I
    {
        iperc = iperc.wrapping_add(l1.intra_coded_area);
        ref_cnt += 1;
    }
    // C: `if (ref_cnt) { *intra_perc = iperc / ref_cnt; } else { *intra_perc = 0; }`
    iperc.checked_div(ref_cnt).unwrap_or(0)
}

/// C `get_ref_skip_percentage` (rc_process.c:96). **`static` — tier 4.**
///
/// ASYMMETRY WITH THE INTRA TWIN, which is exactly the sort of thing an
/// "obviously the same shape" reading gets wrong: this function has NO
/// `ref_cnt`. On the two-reference branch it adds `0` for an I_SLICE L1 and
/// then **halves unconditionally**, so an inter frame whose L1 reference is
/// intra reports HALF the L0 skip area, not the L0 skip area. And `skip_perc`
/// is `uint8_t`, so the add wraps before the shift.
#[must_use]
pub fn get_ref_skip_percentage(
    slice_type: SliceType,
    ref_list1_count_try: u8,
    ref_l0: Option<&RefObjStats>,
    ref_l1: Option<&RefObjStats>,
) -> u8 {
    if slice_type == SliceType::I {
        return 0;
    }
    let mut skip_perc: u8 = 0;
    if let Some(l0) = ref_l0 {
        skip_perc = skip_perc.wrapping_add(if l0.slice_type == SliceType::I {
            0
        } else {
            l0.skip_coded_area
        });
    }
    if slice_type == SliceType::B && ref_list1_count_try != 0 {
        if let Some(l1) = ref_l1 {
            skip_perc = skip_perc.wrapping_add(if l1.slice_type == SliceType::I {
                0
            } else {
                l1.skip_coded_area
            });
        }
        // Unconditional in this branch — see the doc comment. (The `if let`
        // above cannot be merged into the outer `if`: C's `>>= 1` is OUTSIDE
        // it, and merging would skip the halve whenever L1 is absent.)
        skip_perc >>= 1;
    }
    skip_perc
}

/// C `get_ref_hp_percentage` (rc_process.c:118). **`static` — tier 4.**
///
/// Sets `pcs->ref_hp_percentage`, read at enc_mode_config.c:8962 and :9580
/// against `HIGH_PRECISION_REF_PERC_TH` to decide `allow_high_precision_mv` —
/// a frame-header bit and an MV-coding change, so this one is directly on the
/// MV-entropy critical path.
///
/// TWO SIGN TRAPS:
/// 1. `hp_coded_area` is **`uint8_t`** in `EbReferenceObject` but C assigns it
///    to an **`int8_t`** local. A stored value of 200 therefore becomes -56 —
///    negative, but NOT the -1 sentinel — and flows into the average as -56.
///    The port reproduces the narrowing with `as i8`.
/// 2. `-1` is the "no usable reference" sentinel AND a legal `int8_t` value,
///    so a reference whose `hp_coded_area` is 255 is indistinguishable from
///    an absent one. Reproduced, not disambiguated.
///
/// `(hp_perc_l0 + hp_perc_l1) >> 1` is an arithmetic shift on the `int`
/// promotion, i.e. floor division: `(-3 + 0) >> 1 == -2`, not -1.
#[must_use]
pub fn get_ref_hp_percentage(
    slice_type: SliceType,
    ref_list1_count_try: u8,
    ref_l0: Option<&RefObjStats>,
    ref_l1: Option<&RefObjStats>,
) -> i16 {
    if slice_type == SliceType::I {
        return -1;
    }
    let hp_perc_l0: i8 = match ref_l0 {
        Some(l0) if l0.slice_type != SliceType::I => l0.hp_coded_area as i8,
        _ => -1,
    };
    let mut hp_perc_l1: i8 = -1;
    if slice_type == SliceType::B
        && ref_list1_count_try != 0
        && let Some(l1) = ref_l1
    {
        hp_perc_l1 = if l1.slice_type == SliceType::I {
            -1
        } else {
            l1.hp_coded_area as i8
        };
    }
    if hp_perc_l0 == -1 && hp_perc_l1 == -1 {
        -1
    } else if hp_perc_l1 == -1 {
        i16::from(hp_perc_l0)
    } else if hp_perc_l0 == -1 {
        i16::from(hp_perc_l1)
    } else {
        ((i32::from(hp_perc_l0) + i32::from(hp_perc_l1)) >> 1) as i16
    }
}

/// C `MAX_RATE_AVG_PERIOD` (rc_process.h:50) ==
/// `CODED_FRAMES_STAT_QUEUE_MAX_DEPTH >> 1` == `2000 >> 1`.
pub const MAX_RATE_AVG_PERIOD: u64 = 1000;

/// The inputs `rc_init_frame_stats` reads.
#[derive(Clone, Copy, Debug)]
pub struct FrameStatsInput<'a> {
    pub slice_type: SliceType,
    pub ref_list1_count_try: u8,
    pub ref_l0: Option<&'a RefObjStats>,
    pub ref_l1: Option<&'a RefObjStats>,
    /// `scs->passes`.
    pub passes: u32,
    /// `scs->static_config.max_bit_rate`.
    pub max_bit_rate: u64,
    /// `scs->twopass.stats_buf_ctx->total_stats->count`, read only when
    /// `passes > 1 && max_bit_rate != 0`.
    pub total_stats_count: u64,
    /// `ppcs->me_64x64_distortion[0..b64_total_count]`.
    pub me_64x64_distortion: &'a [u32],
}

/// What `rc_init_frame_stats` writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameStatsOutput {
    pub ref_intra_percentage: u8,
    pub ref_skip_percentage: u8,
    pub ref_hp_percentage: i16,
    pub rate_average_periodin_frames: u64,
    /// `None` on an I_SLICE, where C leaves both ME-distortion fields alone.
    pub avg_base_me_dist: Option<u32>,
}

/// C `rc_init_frame_stats` (rc_process.c:604). **`static` — tier 4.**
///
/// The unconditional per-frame RC entry (called at rc_process.c:836, which
/// runs for AOM_Q too). Without it none of the three percentages above are
/// ever populated.
///
/// NOT COVERED HERE, named rather than omitted: the `svt_aom_generate_r0beta`
/// call on the `ppcs->r0_gen` arm (src_ops_process.c:1592) and the cyclic
/// refresh init. Those belong to other files' surfaces; this function returns
/// everything `rc_process.c` itself computes.
///
/// THE ME-DISTORTION ARM IS I_SLICE-GUARDED: on an I_SLICE C does not touch
/// `cur_avg_base_me_dist` OR `prev_avg_base_me_dist`, so the previous frame's
/// values persist — hence `Option` rather than a 0. And the average is
/// `uint64 sum / b64_total_count` truncated into a `uint32`; a zero
/// `b64_total_count` would divide by zero in C, so the port returns `None`
/// for an empty slice rather than reproducing UB.
#[must_use]
pub fn rc_init_frame_stats(input: &FrameStatsInput<'_>) -> FrameStatsOutput {
    let ref_intra_percentage = get_ref_intra_percentage(
        input.slice_type,
        input.ref_list1_count_try,
        input.ref_l0,
        input.ref_l1,
    );
    let ref_skip_percentage = get_ref_skip_percentage(
        input.slice_type,
        input.ref_list1_count_try,
        input.ref_l0,
        input.ref_l1,
    );
    let ref_hp_percentage = get_ref_hp_percentage(
        input.slice_type,
        input.ref_list1_count_try,
        input.ref_l0,
        input.ref_l1,
    );

    let mut rate_average_periodin_frames = if input.passes > 1 && input.max_bit_rate != 0 {
        input.total_stats_count
    } else {
        60
    };
    rate_average_periodin_frames = rate_average_periodin_frames.min(MAX_RATE_AVG_PERIOD);

    let avg_base_me_dist = if input.slice_type != SliceType::I {
        let n = input.me_64x64_distortion.len() as u64;
        let sum: u64 = input
            .me_64x64_distortion
            .iter()
            .fold(0u64, |a, &d| a.wrapping_add(u64::from(d)));
        // C divides by `ppcs->b64_total_count` unguarded; `checked_div`
        // returns None for an empty slice instead of reproducing that UB.
        sum.checked_div(n).map(|avg| avg as u32)
    } else {
        None
    };

    FrameStatsOutput {
        ref_intra_percentage,
        ref_skip_percentage,
        ref_hp_percentage,
        rate_average_periodin_frames,
        avg_base_me_dist,
    }
}

/// C `svt_aom_frame_is_kf_gf_arf` (rc_process.c:56), EXPORTED but taking a
/// `PictureParentControlSet*`; ported here from its two-line body because its
/// only inputs are `frame_is_intra_only(ppcs)` and `ppcs->update_type`.
///
/// Tier 4 as written (no differential — the exported symbol needs a synthetic
/// PPCS this lane did not build one for), and its logic is a two-term
/// disjunction over an enum, which is why that is acceptable here and would
/// not be for anything with arithmetic in it.
#[must_use]
pub fn frame_is_kf_gf_arf(is_intra_only: bool, update_type: FrameUpdateType) -> bool {
    is_intra_only
        || update_type == FrameUpdateType::ArfUpdate
        || update_type == FrameUpdateType::GfUpdate
}

/// C `svt_aom_update_rc_counts` (rc_process.c:564), EXPORTED but taking a
/// `PictureParentControlSet*` and MUTATING the shared `RATE_CONTROL`.
///
/// **THIS IS TRANSLATED AND MUST NOT BE CALLED IN CQP/CRF**
/// (`WORKING-ON-THIS.md` §7: dead-looking C stays translated, with its
/// reachability written down). Both C call sites were checked:
/// rc_process.c:791 is inside the non-AOM_Q arm, and
/// packetization_process.c:602 is under
/// `if (scs->static_config.rate_control_mode)` where `CQP_OR_CRF == 0`.
/// So in the port's envelope it never runs, `frames_since_key` stays at
/// [`rc_init`]'s seed of 8 and `frames_since_cdf_update` stays 0 forever —
/// see [`rc_init`] for why that makes `disable_cdf_update` permanently 0.
///
/// Returns the updated `(frames_since_key, frames_to_key, frames_since_cdf_update)`.
#[must_use]
pub fn update_rc_counts(
    showable_frame: bool,
    disable_cdf_update: bool,
    frames_since_key: i32,
    frames_to_key: i32,
    frames_since_cdf_update: i32,
) -> (i32, i32, i32) {
    if !showable_frame {
        return (frames_since_key, frames_to_key, frames_since_cdf_update);
    }
    let frames_since_key = frames_since_key + 1;
    let frames_to_key = frames_to_key - 1;
    // "Reset whenever the CDF is updated for the current frame" — C's comment.
    let frames_since_cdf_update = if !disable_cdf_update {
        0
    } else {
        frames_since_cdf_update + 1
    };
    (frames_since_key, frames_to_key, frames_since_cdf_update)
}

// ---------------------------------------------------------------------------
// Frame-rate -> bandwidth, and the small clamps/resets
// (pass2_strategy.c:880-903, rc_process.c:34-54, :552-592, :735)
// ---------------------------------------------------------------------------

/// C `MAX_MB_RATE` (pass2_strategy.c:878): the per-16x16-MB bit budget the
/// 1080p-and-below decode-HW baseline assumes.
pub const MAX_MB_RATE: i64 = 250;

/// C `MAXRATE_1080P` (pass2_strategy.c:879) == `(1920 * 1080 / (16 * 16)) * 250`.
pub const MAXRATE_1080P: i64 = 2_025_000;

/// What `av1_rc_update_framerate` / `svt_av1_new_framerate` produce.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FramerateBandwidth {
    /// `scs->new_framerate`.
    pub new_framerate: f64,
    /// `rc->avg_frame_bandwidth`.
    pub avg_frame_bandwidth: i32,
    /// `rc->max_frame_bandwidth`.
    pub max_frame_bandwidth: i32,
}

/// C `svt_av1_new_framerate` (pass2_strategy.c:900), EXPORTED, plus the
/// `static` `av1_rc_update_framerate` (:880) it calls unconditionally.
///
/// **This closes the gap [`rc_init`] names.** `rc_init`'s `mode != AOM_Q` tail
/// calls exactly this; the port previously returned the AOM_Q-complete result
/// and told the caller to run this step itself. Now the step exists.
///
/// THE FLOOR IS ON THE INPUT, NOT THE OUTPUT: `framerate < 0.1` is replaced by
/// **30**, not by 0.1 — a sub-0.1 fps input jumps to 30 fps, which is a
/// 300x change in `avg_frame_bandwidth`, not a clamp.
///
/// C's arithmetic, kept in its own types: `avg_frame_bandwidth` is
/// `(int)(uint32 target_bit_rate / double new_framerate)` — a truncating
/// double-to-int conversion; `vbr_max_bits` is computed in `int64_t` and then
/// narrowed by `(int)` BEFORE the `AOMMIN(_, INT_MAX)`, so that AOMMIN can
/// never fire and a genuinely huge product has already wrapped. Reproduced
/// with a wrapping narrow, because that wrap is the C behaviour a bitstream
/// would depend on.
#[must_use]
pub fn new_framerate(
    target_bit_rate: u32,
    num_mbs: i32,
    vbrmax_section: i32,
    framerate: f64,
) -> FramerateBandwidth {
    let new_framerate = if framerate < 0.1 { 30.0 } else { framerate };
    let avg_frame_bandwidth = (f64::from(target_bit_rate) / new_framerate) as i32;
    // C: `(int)(((int64_t)rc->avg_frame_bandwidth * vbrmax_section) / 100)`
    // then `AOMMIN(_, INT_MAX)` — the cast to int already happened, so the
    // AOMMIN is a no-op on a value that has already been narrowed.
    let vbr_max_bits_i64 = (i64::from(avg_frame_bandwidth) * i64::from(vbrmax_section)) / 100;
    // The `(int)` narrow happens BEFORE C's `AOMMIN(_, INT_MAX)`, so that
    // AOMMIN is a no-op and a large product has already wrapped. `as i32` is
    // the wrapping narrow; reproducing the wrap is the point.
    let vbr_max_bits = vbr_max_bits_i64 as i32;
    // `MBs * MAX_MB_RATE` and both AOMMAXes are plain `int` arithmetic in C,
    // so the comparison chain stays in i32 (and the multiply wraps there too).
    let mb_rate = num_mbs.wrapping_mul(MAX_MB_RATE as i32);
    let max_frame_bandwidth = aom_max_i32(aom_max_i32(mb_rate, MAXRATE_1080P as i32), vbr_max_bits);
    FramerateBandwidth {
        new_framerate,
        avg_frame_bandwidth,
        max_frame_bandwidth,
    }
}

/// C `AOMMAX` on `int` — the macro shape, kept for symmetry with [`aom_max`].
fn aom_max_i32(a: i32, b: i32) -> i32 {
    if a > b { a } else { b }
}

/// C `clamp_qp` (rc_process.c:50). `static` — tier 4.
///
/// `CLIP3(min_val, max_val, a)` (utility.h:101) is
/// `a < min ? min : (a > max ? max : a)` — **min first, then max, then the
/// value**, which is the opposite argument order from most clamp helpers and
/// is why this is spelled out rather than mapped onto `i32::clamp`. When
/// `min > max` C returns `min` and `i32::clamp` PANICS, so the two are not
/// interchangeable.
#[must_use]
pub fn clamp_qp(min_qp_allowed: i32, max_qp_allowed: i32, qp: i32) -> u8 {
    clip3(min_qp_allowed, max_qp_allowed, qp) as u8
}

/// C `clamp_qindex` (rc_vbr_cbr.c:21). `static` — tier 4. Same clamp as
/// [`clamp_qp`] but the bounds go through `quantizer_to_qindex` first, so it
/// clamps in the QINDEX domain rather than the QP domain.
#[must_use]
pub fn clamp_qindex(min_qp_allowed: i32, max_qp_allowed: i32, qindex: i32) -> u8 {
    let qmin = i32::from(quantizer_to_qindex(min_qp_allowed));
    let qmax = i32::from(quantizer_to_qindex(max_qp_allowed));
    clip3(qmin, qmax, qindex) as u8
}

/// C `CLIP3` (utility.h:101). Note it is NOT `clamp`: with `min > max` it
/// returns `min`, where `i32::clamp` panics.
#[must_use]
pub fn clip3(min_val: i32, max_val: i32, a: i32) -> i32 {
    if a < min_val {
        min_val
    } else if a > max_val {
        max_val
    } else {
        a
    }
}

/// C `use_rtc_cbr_path` (rc_process.c:34). `static` — tier 4.
/// `rc_cfg.mode == AOM_CBR && scs->static_config.rtc`.
#[must_use]
pub fn use_rtc_cbr_path(mode: i32, rtc: bool) -> bool {
    mode == AomRcMode::Cbr as i32 && rtc
}

/// The `RateControlIntervalParamContext` fields C's `rc_param_reset`
/// (rc_process.c:552) writes. `static` — tier 4.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RcIntervalParams {
    /// C sets this to **-1**, not 0 — the "no interval" sentinel.
    pub size: i64,
    pub processed_frame_number: u32,
    pub vbr_bits_off_target: i64,
    pub vbr_bits_off_target_fast: i64,
    pub rate_error_estimate: i32,
    pub total_actual_bits: i64,
    pub total_target_bits: i64,
    pub extend_minq: i32,
    pub extend_maxq: i32,
    pub extend_minq_fast: i32,
}

impl RcIntervalParams {
    /// C `rc_param_reset` (rc_process.c:552).
    #[must_use]
    pub fn reset() -> Self {
        Self {
            size: -1,
            processed_frame_number: 0,
            vbr_bits_off_target: 0,
            vbr_bits_off_target_fast: 0,
            rate_error_estimate: 0,
            total_actual_bits: 0,
            total_target_bits: 0,
            extend_minq: 0,
            extend_maxq: 0,
            extend_minq_fast: 0,
        }
    }
}

/// The PPCS fields C's `reset_rc_param` (rc_process.c:589) zeroes. EXPORTED in
/// C, but its whole body is three stores into a `PictureParentControlSet`, so
/// there is nothing for a differential to compare that this type does not
/// already state. Tier 4.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PpcsRcParams {
    pub loop_count: u32,
    pub overshoot_seen: u8,
    pub undershoot_seen: u8,
}

/// C `reset_rc_param` (rc_process.c:589).
#[must_use]
pub fn reset_rc_param() -> PpcsRcParams {
    PpcsRcParams::default()
}

/// Which steps C's `generate_sb_qindex` (rc_process.c:735) runs, given the
/// frame's delta-q config. `static` — tier 4.
///
/// This is the SECOND false-"ported" row: the port's inventory matched
/// `generate_sb_qindex` against the string `svt_av1_rc_init_sb_qindex` inside
/// comments at `sb_qindex.rs:144`/`:264` and `variance_boost_recon.rs:10`.
/// The first callee's logic does exist in [`crate::sb_qindex`]; this wrapper
/// did not.
///
/// NOT PORTED, named rather than omitted: `svt_av1_generate_b64_me_qindex_map`
/// (**rc_aq.c**:656) has no Rust counterpart and rc_aq.c is another module
/// group's file, so it is not written here. `svt_av1_rc_init_sb_qindex`
/// (rc_aq.c:871) and `svt_av1_normalize_sb_delta_q` (rc_aq.c:830) are both
/// already ported in [`crate::sb_qindex`]. What this function contributes is
/// only the CONTROL FLOW that rc_process.c owns — in particular that the
/// normalize step is gated on `delta_q_present && delta_q_res != 1`, so
/// `delta_q_res == 1` skips it even with delta-q on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SbQindexSteps {
    /// `svt_av1_rc_init_sb_qindex` — unconditional.
    pub init_sb_qindex: bool,
    /// `svt_av1_normalize_sb_delta_q`.
    pub normalize_sb_delta_q: bool,
    /// `svt_av1_generate_b64_me_qindex_map` — NOT PORTED, see above.
    pub generate_b64_me_qindex_map: bool,
}

/// C `generate_sb_qindex` (rc_process.c:735) — its step selection.
#[must_use]
pub fn generate_sb_qindex_steps(
    delta_q_present: bool,
    delta_q_res: u8,
    stats_based_sb_lambda_modulation: bool,
) -> SbQindexSteps {
    SbQindexSteps {
        init_sb_qindex: true,
        normalize_sb_delta_q: delta_q_present && delta_q_res != 1,
        generate_b64_me_qindex_map: stats_based_sb_lambda_modulation,
    }
}

// ---------------------------------------------------------------------------
// update_lambda's own gf_update_type (rc_process.c:406)
// ---------------------------------------------------------------------------

/// C `update_lambda`'s own `gf_update_type` (rc_process.c:406-410) — the
/// `rd_frame_type_factor` row's selector, derived from the FRAME TYPE and the
/// temporal layer rather than from `ppcs->update_type`.
#[must_use]
pub fn lambda_gf_update_type(
    is_key: bool,
    hierarchical_levels: u8,
    temporal_layer_index: u8,
) -> FrameUpdateType {
    if is_key {
        FrameUpdateType::KfUpdate
    } else if temporal_layer_index == 0 {
        FrameUpdateType::ArfUpdate
    } else if temporal_layer_index < hierarchical_levels {
        FrameUpdateType::IntnlArfUpdate
    } else {
        FrameUpdateType::LfUpdate
    }
}

/// C `svt_av1_generate_b64_me_qindex_map` (`rc_aq.c:656`) — a per-b64 qindex
/// derived from the open-loop ME COST VARIANCE, for LAMBDA MODULATION ONLY.
///
/// C's own header comment says "to be used for lambda modulation only; not at
/// Q/Q-1": nothing quantizes with this value, and it never reaches the frame
/// header. Its only consumer is
/// [`crate::port_md_rate_estimation::get_me_qindex`], whose result is
/// `update_lambda`'s `me_q_index` — the `qdiff` in the
/// `stats_based_sb_lambda_modulation` factor, which is what makes C's
/// `fast_lambda_md` and `full_lambda_md` PER-SUPERBLOCK.
///
/// **Byte-inert on every I-slice by construction.** C takes the `else` branch
/// there and writes `base_q_idx` into every entry, so `qdiff` is 0 and the
/// factor is the identity 128. That is what lets this be wired without moving
/// a still or a key frame.
///
/// The offsets: `min_offset` / `max_offset` are `-8` / `+8` at every temporal
/// layer (C spells them as per-layer arrays that happen to be uniform, kept
/// that shape here), and the clip is `base_q_idx ± (9 * 4 - 1)` = ±35, which
/// an 8-wide offset cannot reach except against the 1 / MAXQ ends.
///
/// **The multiply happens in C's `int`, not in 64 bits.** `min_offset[tl]` and
/// `diff_dist` are both `int`, so `min_offset[tl] * diff_dist` is a 32-bit
/// product that is only then widened for the division by the `int64_t`
/// denominator. Reproduced with `wrapping_mul` on `i32` rather than promoted,
/// because a 32-bit overflow there is C's behaviour and not ours to fix.
///
/// **No division by zero is reachable**, and it is worth saying why rather
/// than guarding: the `diff_dist < 0` arm divides by `min_dist - avg_dist`,
/// and `diff_dist < 0` means `mev[b] < avg`, which forces `min <= mev < avg`;
/// symmetrically for the `> 0` arm. When every value is equal, `diff_dist` is
/// 0 and neither arm runs.
///
/// **Evidence tier 2** for the RESULT, on `diag 72x72 q40 p6` frame 1: this
/// map's four outputs put `update_lambda`'s factor at 100 / 100 / 100 / 150,
/// and C's own `SVT_PD0CFG_OUT` reports `fastlam` 5182 / 5182 / 5182 / 7773
/// against a pre-factor 6633 — both reproduced exactly. Tier 4 for the
/// arithmetic itself (C's function is `void` and takes a `PictureControlSet`,
/// so there is nothing to shim a differential against).
/// Full derivation: `benchmarks/pd0_depth_removal_join_2026-09-02.md`.
#[must_use]
pub fn generate_b64_me_qindex_map(
    me_8x8_cost_variance: &[u32],
    base_q_idx: i32,
    is_islice: bool,
) -> alloc::vec::Vec<u8> {
    let n = me_8x8_cost_variance.len();
    // C clamps the WRITTEN value to `1..=MAXQ` only inside the non-I arm; the
    // I arm writes `base_q_idx` raw, which is already a valid qindex.
    let base = base_q_idx.clamp(0, 255) as u8;
    if is_islice || n == 0 {
        return alloc::vec![base; n];
    }

    let mut sum: i64 = 0;
    let mut min_dist: i64 = i64::MAX;
    let mut max_dist: i64 = 0;
    for &v in me_8x8_cost_variance {
        let v = i64::from(v);
        sum += v;
        min_dist = min_dist.min(v);
        max_dist = max_dist.max(v);
    }
    let avg_dist = sum / n as i64;

    const MIN_OFFSET: i32 = -8;
    const MAX_OFFSET: i32 = 8;
    let min_q_idx = (base_q_idx - 9 * 4 + 1).max(1);
    let max_q_idx = (base_q_idx + 9 * 4 - 1).min(255);

    me_8x8_cost_variance
        .iter()
        .map(|&v| {
            // C: `int diff_dist = (int)(mev - avg_dist);`
            let diff_dist = (i64::from(v) - avg_dist) as i32;
            let offset: i32 = if diff_dist < 0 {
                (i64::from(MIN_OFFSET.wrapping_mul(diff_dist)) / (min_dist - avg_dist)) as i32
            } else if diff_dist > 0 {
                (i64::from(MAX_OFFSET.wrapping_mul(diff_dist)) / (max_dist - avg_dist)) as i32
            } else {
                0
            };
            (base_q_idx + offset).clamp(min_q_idx, max_q_idx) as u8
        })
        .collect()
}

#[cfg(test)]
mod frame_update_type_tests {
    use super::*;

    /// `update_lambda`'s selector is NOT `ppcs->update_type`, and the two
    /// DISAGREE on the port's own low-delay P envelope: `set_frame_update_type`
    /// (`port_picstruct`, pd_process.c:4591) gives `Lf` for frame 1 while this
    /// gives `Arf`. That disagreement is the whole reason both exist — see
    /// `pd0::inter_full_lambda_8bit`.
    #[test]
    fn low_delay_p_frame_1_is_arf_for_the_factor_while_the_picture_is_lf() {
        assert_eq!(lambda_gf_update_type(true, 0, 0), FrameUpdateType::KfUpdate);
        assert_eq!(
            lambda_gf_update_type(false, 0, 0),
            FrameUpdateType::ArfUpdate
        );

        // The picture's own type, from the already-ported `set_frame_update_type`.
        let mut pic = crate::port_picstruct::PicParams {
            is_key_frame: false,
            hierarchical_levels: 0,
            temporal_layer_index: 0,
            frame_offset: 1,
            ..Default::default()
        };
        crate::port_picstruct::set_frame_update_type(&mut pic);
        assert_eq!(pic.update_type, crate::port_picstruct::FrameUpdateType::Lf);
    }

    /// C's `gf_update_type` ladder itself: the LAST-layer arm is `<`, not
    /// `<=`, so `temporal_layer_index == hierarchical_levels` is `Lf`.
    #[test]
    fn the_hierarchical_ladder_matches_update_lambdas_own_comparisons() {
        assert_eq!(
            lambda_gf_update_type(false, 3, 0),
            FrameUpdateType::ArfUpdate
        );
        assert_eq!(
            lambda_gf_update_type(false, 3, 2),
            FrameUpdateType::IntnlArfUpdate
        );
        assert_eq!(
            lambda_gf_update_type(false, 3, 3),
            FrameUpdateType::LfUpdate
        );
    }

    /// **C's OWN per-superblock lambdas, read off `SVT_PD0CFG_OUT`'s
    /// `fastlam=` field on `diag 72x72 q40 p6` frame 1** (evidence tier 2 —
    /// the real encoder, not a transcription):
    ///
    /// ```text
    /// PD0CFG sb=0 org=(0,0)   islice=0 ... fastlam=5182 ... mev=0
    /// PD0CFG sb=1 org=(64,0)  islice=0 ... fastlam=5182 ... mev=0
    /// PD0CFG sb=2 org=(0,64)  islice=0 ... fastlam=5182 ... mev=0
    /// PD0CFG sb=3 org=(64,64) islice=0 ... fastlam=7773 ... mev=1341553
    /// ```
    ///
    /// with `base_q_idx = 160`, and the port's pre-factor value 6633 (its own
    /// `PD0DR fastlam=` before this map existed). This test drives the map and
    /// `update_lambda`'s factor block and asserts BOTH of C's numbers.
    ///
    /// It is the cell to re-run if this ever moves:
    /// `benchmarks/pd0_depth_removal_join_2026-09-02.md`.
    #[test]
    fn the_per_sb_lambda_matches_cs_measured_values_on_diag_72x72_q40_p6() {
        // The four `me_8x8_cost_variance` values, which the port's own
        // `PD0DR mev=` already reproduced exactly before this map existed.
        let mev = [0u32, 0, 0, 1_341_553];
        let map = generate_b64_me_qindex_map(&mev, 160, false);
        assert_eq!(
            map,
            [152, 152, 152, 168],
            "offset -8 on the three zero-variance b64s and +8 on the outlier"
        );

        let ctx = LambdaContext {
            frame_type: 1, // not KEY_FRAME
            temporal_layer_index: 0,
            hierarchical_levels: 0,
            update_type: FrameUpdateType::LfUpdate,
            alt_lambda_factors: false,
            rtc: false,
            stats_based_sb_lambda_modulation: true,
            base_q_idx: 160,
            delta_q_present: false,
            r0_delta_qp_md: false,
            lambda_scale_factors: [128; 7],
        };
        // ORDER MATTERS AND IS NOT THE OBVIOUS ONE. `update_lambda` applies
        // the per-SB factor and RETURNS; `av1_lambda_assign_md`
        // (md_process.c:747) then multiplies by `pcs->lambda_weight`, which is
        // 150 on this cell. So the port's reported flat 6633 is
        // POST-lambda_weight — `compute_fast_lambda` alone gives 5661 — and an
        // implementation that folded the weight in first would land on the
        // same two numbers here only by luck of the flooring.
        let weight = |v: u32| ((u64::from(v) * 150) >> 7) as u32;
        let off = LambdaContext {
            stats_based_sb_lambda_modulation: false,
            ..ctx
        };
        assert_eq!(compute_fast_lambda(&off, 160, 160, 8), 5661, "pre-weight");
        assert_eq!(
            weight(compute_fast_lambda(&off, 160, 160, 8)),
            6633,
            "the flat value the port reported before this map existed"
        );
        assert_eq!(
            weight(compute_fast_lambda(&ctx, 160, map[0], 8)),
            5182,
            "C's SB 0/1/2 — factor 100"
        );
        assert_eq!(
            weight(compute_fast_lambda(&ctx, 160, map[3], 8)),
            7773,
            "C's SB 3 — factor 150"
        );
    }

    /// An I_SLICE writes `base_q_idx` everywhere, which is what makes wiring
    /// this byte-inert for every still and every key frame — the factor is
    /// then the identity 128 rather than 100 or 150.
    #[test]
    fn an_i_slice_map_is_flat_and_the_factor_is_the_identity() {
        let mev = [0u32, 7, 1_341_553, 42];
        assert_eq!(generate_b64_me_qindex_map(&mev, 160, true), [160; 4]);

        let ctx = LambdaContext {
            frame_type: 0, // KEY_FRAME
            temporal_layer_index: 0,
            hierarchical_levels: 0,
            update_type: FrameUpdateType::KfUpdate,
            alt_lambda_factors: false,
            rtc: false,
            stats_based_sb_lambda_modulation: true,
            base_q_idx: 160,
            delta_q_present: false,
            r0_delta_qp_md: false,
            lambda_scale_factors: [128; 7],
        };
        let off = LambdaContext {
            stats_based_sb_lambda_modulation: false,
            ..ctx
        };
        assert_eq!(
            compute_fast_lambda(&ctx, 160, 160, 8),
            compute_fast_lambda(&off, 160, 160, 8),
            "qdiff 0 -> factor 128 -> the modulation is a no-op"
        );
    }

    /// The two boundaries of the arm this port takes — C's FINAL `else`, whose
    /// thresholds are +-4 with a LOW factor of 100. They are NOT the
    /// `delta_q_present` arm's +-8 / 90, which `pd0::inter_full_lambda_8bit`
    /// transcribed instead (see the benchmark note); a test that only checked
    /// the extremes would pass against either.
    #[test]
    fn the_me_q_index_arm_uses_plus_minus_four_and_a_low_factor_of_one_hundred() {
        let ctx = LambdaContext {
            frame_type: 1,
            temporal_layer_index: 0,
            hierarchical_levels: 0,
            update_type: FrameUpdateType::LfUpdate,
            alt_lambda_factors: false,
            rtc: false,
            stats_based_sb_lambda_modulation: true,
            base_q_idx: 160,
            delta_q_present: false,
            r0_delta_qp_md: false,
            lambda_scale_factors: [128; 7],
        };
        let f = |me: u8| compute_fast_lambda(&ctx, 160, me, 8);
        // `LAMBDA_MODE_DECISION_8BIT_SAD[160]` through the frame-type factor,
        // i.e. the value with the modulation off (asserted in the sibling
        // test above), before `lambda_weight`.
        let base = 5661i64;
        let with = |factor: i64| ((base * factor) >> 7) as u32;
        assert_eq!(f(156), with(100), "qdiff -4 is <= -4 -> 100, not 115");
        assert_eq!(f(157), with(115), "qdiff -3 -> 115");
        assert_eq!(f(160), with(128), "qdiff 0 -> identity");
        assert_eq!(f(163), with(135), "qdiff +3 -> 135");
        assert_eq!(f(164), with(135), "qdiff +4 is <= 4 -> 135, not 150");
        assert_eq!(f(165), with(150), "qdiff +5 -> 150");
    }

    /// A map over values that are all equal takes NEITHER division arm, which
    /// is why no divide-by-zero guard is needed. Asserted rather than argued
    /// because the denominators (`min - avg`, `max - avg`) are both zero here.
    #[test]
    fn a_uniform_variance_map_divides_by_nothing() {
        assert_eq!(
            generate_b64_me_qindex_map(&[7, 7, 7, 7], 100, false),
            [100; 4]
        );
        assert_eq!(generate_b64_me_qindex_map(&[0, 0], 100, false), [100; 2]);
    }
}
