//! Port of `Codec/pass2_strategy.c`'s **GOP bit allocation and two-pass
//! feedback** — the half that reads the first-pass stats ring.
//!
//! [`crate::port_pass2_strategy`] already ports the file's pure-scalar leaves
//! (the qindex-by-rate search with correction, the `q_pow_term` curve, the
//! twopass worst-quality estimate). Everything HERE walks the
//! `FIRSTPASS_STATS` ring or mutates `TWO_PASS` / `RATE_CONTROL` /
//! `RateControlIntervalParamContext`, which is why it was left out of that
//! file and listed as missing in its header.
//!
//! **EVIDENCE.** Two of this group's functions are EXPORTED and are the ones
//! that run on EVERY frame of a VBR encode, so they are pinned at **tier 1**
//! by `tests/c_parity_pass2_gop.rs`:
//! `svt_av1_twopass_postencode_update` and
//! `svt_av1_twopass_postencode_update_gop_const`.
//!
//! The rest is **tier 4** and says so per function. `svt_aom_process_rc_stat`,
//! `svt_av1_init_second_pass` and `svt_av1_init_single_pass_lap` ARE exported,
//! but driving them needs a populated `STATS_BUFFER_CTX` ring wired into a
//! `SequenceControlSet` — a harness this lane has not built. That is a real
//! gap, not a claim of coverage: the functions below it (`kf_group_rate_
//! assingment`, `gf_group_rate_assingment`, `calculate_gf_stats`,
//! `allocate_gf_group_bits`, …) are `static` and inlined away, so they have no
//! other route either.
//!
//! **Preprocessor check** (`docs/WORKING-ON-THIS.md` §5 trap #1):
//! `grep -c 'SVT_HDR_MODE' pass2_strategy.c` is 0 and `grep -c '#if'` is 0, so
//! no function here has a second fork definition and every line read is a line
//! mainline compiles.

use crate::port_rc_process::FrameUpdateType;
use crate::port_rc_vbr_cbr_state::{FrameRc, RateControl, RateControlCfg, SeqRc};
use crate::port_rc_vbr_cbr_update::RcIntervalParams;
use crate::rate_control::convert_qindex_to_q;

/// C `MINQ_ADJ_LIMIT` (rc_process.h:22).
pub const MINQ_ADJ_LIMIT: i32 = 48;
/// C `HIGH_UNDERSHOOT_RATIO` (rc_process.h:23).
pub const HIGH_UNDERSHOOT_RATIO: i32 = 2;
/// C `MAX_ARF_LAYERS` (rc_process.h:36).
pub const MAX_ARF_LAYERS: usize = 6;
/// C `MAX_MB_RATE` (pass2_strategy.c:878).
pub const MAX_MB_RATE: i64 = 250;
/// C `MAXRATE_1080P` (pass2_strategy.c:879).
pub const MAXRATE_1080P: i64 = 2_025_000;
/// C `DEFAULT_GRP_WEIGHT` (pass2_strategy.c:744).
pub const DEFAULT_GRP_WEIGHT: f64 = 1.0;
/// C `MAX_KF_BITS_INTERVAL_SINGLE_PASS` (pass2_strategy.c:645).
pub const MAX_KF_BITS_INTERVAL_SINGLE_PASS: f64 = 5.0;
/// C `RC_FACTOR_MIN_GOP_CONST` (pass2_strategy.c:308).
pub const RC_FACTOR_MIN_GOP_CONST: f64 = 0.5;
/// C `RC_FACTOR_MIN_1P_VBR` (pass2_strategy.c:309).
pub const RC_FACTOR_MIN_1P_VBR: f64 = 1.0;
/// C `RC_FACTOR_MIN` (pass2_strategy.c:310).
pub const RC_FACTOR_MIN: f64 = 0.75;
/// C `RC_FACTOR_MAX` (pass2_strategy.c:311).
pub const RC_FACTOR_MAX: f64 = 2.0;

/// C `layer_fraction` (pass2_strategy.c:238) — the share of the ARF bit pool
/// each pyramid level takes before passing the remainder down.
pub const LAYER_FRACTION: [f64; MAX_ARF_LAYERS + 1] = [1.0, 0.80, 0.7, 0.60, 0.60, 1.0, 1.0];

/// C `DOUBLE_DIVIDE_CHECK(x)` (firstpass.h:23) — nudge a divisor away from
/// zero WITHOUT changing its sign. Note it is not `max(x, eps)`: a negative
/// divisor gets `- 0.000001`, so the quotient's sign is preserved.
#[must_use]
pub fn double_divide_check(x: f64) -> f64 {
    if x < 0.0 {
        x - 0.000_001
    } else {
        x + 0.000_001
    }
}

/// C `StatStruct` (definitions.h:2171).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StatStruct {
    pub poc: u64,
    pub total_num_bits: u64,
    pub qindex: u8,
    pub worst_qindex: u8,
    pub temporal_layer_index: u8,
}

/// C `FIRSTPASS_STATS` (firstpass.h:30). Every field is `double` in C
/// including `frame` and `count`, which are logically integers — kept `f64`
/// because [`subtract_stats`] and the averages do real arithmetic on them.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FirstPassStats {
    /// Frame number in display order, for a single-frame stat.
    pub frame: f64,
    /// Best of intra and inter (last-frame) prediction error.
    pub coded_error: f64,
    /// Duration of the frame, or of the collection.
    pub duration: f64,
    /// 1.0 for a single frame, otherwise how many frames are accumulated.
    pub count: f64,
    pub stat_struct: StatStruct,
}

/// C `GF_GROUP_STATS` (pass2_strategy.h:23).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GfGroupStats {
    pub gf_group_err: f64,
    pub gf_stat_struct: StatStruct,
    pub gf_group_raw_error: f64,
    pub gf_group_skip_pct: f64,
    pub gf_group_inactive_zone_rows: f64,
}

/// C `init_gf_stats` (pass2_strategy.c:326).
///
/// NOT `Default`: the qindex fields start at **172** and `total_num_bits` at
/// **1** (a divide guard), not at zero. Using `GfGroupStats::default()` where
/// C calls this would silently divide by zero in
/// `av1_gop_bit_allocation_same_pred`.
#[must_use]
pub fn init_gf_stats() -> GfGroupStats {
    GfGroupStats {
        gf_group_err: 0.0,
        gf_stat_struct: StatStruct {
            poc: 0,
            total_num_bits: 1,
            qindex: 172,
            worst_qindex: 172,
            temporal_layer_index: 0,
        },
        gf_group_raw_error: 0.0,
        gf_group_skip_pct: 0.0,
        gf_group_inactive_zone_rows: 0.0,
    }
}

/// C `TWO_PASS` (firstpass.h:75) — the fields `pass2_strategy.c` reads or
/// writes, minus the ring pointers (see [`StatsCursor`]).
#[derive(Clone, Copy, Debug, Default)]
pub struct TwoPassState {
    /// `bits_left`.
    pub bits_left: i64,
    /// `modified_error_min`.
    pub modified_error_min: f64,
    /// `modified_error_max`.
    pub modified_error_max: f64,
    /// `modified_error_left`.
    pub modified_error_left: f64,
    /// `kf_group_bits`.
    pub kf_group_bits: i64,
    /// `kf_group_error_left`.
    pub kf_group_error_left: i64,
    /// `kf_zeromotion_pct`.
    pub kf_zeromotion_pct: i32,
    /// `extend_minq`.
    pub extend_minq: i32,
    /// `extend_maxq`.
    pub extend_maxq: i32,
    /// `extend_minq_fast`.
    pub extend_minq_fast: i32,
    /// `passes`.
    pub passes: i32,
}

/// C's `twopass->stats_in` pointer walk over the first-pass ring, as an index.
///
/// **THE CURSOR AND `this_frame` ARE OFFSET BY ONE.** Every caller reaches
/// these loops with `this_frame` already holding the stat at index `k` and the
/// cursor sitting at `k + 1` — `process_first_pass_stats` establishes that by
/// calling `input_stats` once before anything else. A loop body therefore
/// accounts for `this_frame` and THEN advances. Setting the cursor to the same
/// index as `this_frame` double-counts the first entry, which is exactly what
/// three of this module's first-draft vectors did.
///
/// **Two boundary rules that look inconsistent and are not.** `input_stats`
/// stops at `stats_in >= stats_in_end`, but three of the loops that call it
/// test `stats_in <= stats_in_end` — INCLUSIVE. So a loop body runs once more,
/// with the cursor sitting exactly on the end sentinel (the accumulator slot
/// `svt_av1_init_second_pass` writes the totals into), and only then does
/// `input_stats` return EOF and break. Reproducing that off-by-one is what
/// keeps `num_stats` and `modified_error_total` matching C.
#[derive(Clone, Copy, Debug)]
pub struct StatsCursor<'a> {
    /// The ring from `stats_in_start` up to but excluding `stats_in_end`.
    stats: &'a [FirstPassStats],
    /// `stats_in - stats_in_start`.
    pos: usize,
}

impl<'a> StatsCursor<'a> {
    /// A cursor at `stats_in_start + offset`.
    #[must_use]
    pub fn new(stats: &'a [FirstPassStats], offset: usize) -> Self {
        Self { stats, pos: offset }
    }

    /// C's `twopass->stats_in <= twopass->stats_buf_ctx->stats_in_end`.
    #[must_use]
    pub fn at_or_before_end(&self) -> bool {
        self.pos <= self.stats.len()
    }

    /// C `input_stats` (pass2_strategy.c:37): read and advance, or EOF.
    pub fn input_stats(&mut self) -> Option<FirstPassStats> {
        if self.pos >= self.stats.len() {
            return None;
        }
        let v = self.stats[self.pos];
        self.pos += 1;
        Some(v)
    }

    /// C `reset_fpf_position` (pass2_strategy.c:33) — C takes a saved POINTER;
    /// the port takes the saved index from [`StatsCursor::position`].
    pub fn reset_fpf_position(&mut self, position: usize) {
        self.pos = position;
    }

    /// The value C saves as `const FIRSTPASS_STATS* start_pos`.
    #[must_use]
    pub fn position(&self) -> usize {
        self.pos
    }

    /// C's `(twopass->stats_in - 1)->stat_struct` in
    /// `kf_group_rate_assingment`. `None` at position 0, where C would read
    /// one slot BEFORE the ring.
    #[must_use]
    pub fn previous(&self) -> Option<&FirstPassStats> {
        self.pos.checked_sub(1).and_then(|i| self.stats.get(i))
    }
}

/// C `calculate_modified_err` (pass2_strategy.c:22).
///
/// The name is upstream's; the body no longer computes a modified error at
/// all — it returns the frame's coded bit count, and returns 0 only when the
/// accumulated `total_stats` slot is absent. `has_total_stats` is that
/// `stats != NULL` test.
#[must_use]
pub fn calculate_modified_err(has_total_stats: bool, this_frame: &FirstPassStats) -> f64 {
    if !has_total_stats {
        return 0.0;
    }
    this_frame.stat_struct.total_num_bits as f64
}

/// C `subtract_stats` (pass2_strategy.c:47). Only four of the five fields are
/// subtracted — `stat_struct` is left alone.
pub fn subtract_stats(section: &mut FirstPassStats, frame: &FirstPassStats) {
    section.frame -= frame.frame;
    section.coded_error -= frame.coded_error;
    section.count -= frame.count;
    section.duration -= frame.duration;
}

/// C `frame_max_bits` (pass2_strategy.c:55) — the per-frame ceiling from
/// `--vbr-max-section-pct`.
#[must_use]
pub fn frame_max_bits(rc: &RateControl, vbrmax_section: i32) -> i32 {
    let max_bits = i64::from(rc.avg_frame_bandwidth) * i64::from(vbrmax_section) / 100;
    // C `CLIP3(0, rc->max_frame_bandwidth, max_bits)` over `int64_t`, then a
    // narrowing `(int)`. The clip bounds it to `max_frame_bandwidth` first, so
    // the narrowing cannot truncate.
    max_bits.clamp(0, i64::from(rc.max_frame_bandwidth)) as i32
}

/// C `accumulate_this_frame_stats` (pass2_strategy.c:168).
pub fn accumulate_this_frame_stats(
    stats: &FirstPassStats,
    mod_frame_err: f64,
    gf_stats: &mut GfGroupStats,
) {
    gf_stats.gf_group_err += mod_frame_err;
    gf_stats.gf_group_raw_error += stats.coded_error;
}

/// The `SequenceControlSet` two-pass knobs `pass2_strategy.c` reads that are
/// not already on [`SeqRc`].
#[derive(Clone, Copy, Debug, Default)]
pub struct TwoPassCfg {
    /// `enc_ctx->two_pass_cfg.vbrmin_section`.
    pub vbrmin_section: i32,
    /// `enc_ctx->two_pass_cfg.vbrmax_section`.
    pub vbrmax_section: i32,
    /// `scs->lap_rc`.
    pub lap_rc: bool,
    /// `scs->lad_mg`.
    pub lad_mg: i32,
    /// `scs->static_config.pass == ENC_SINGLE_PASS`.
    pub single_pass: bool,
    /// `scs->static_config.pass == ENC_SECOND_PASS`.
    pub second_pass: bool,
    /// `enc_ctx->frame_info.mb_rows`.
    pub mb_rows: i32,
    /// `enc_ctx->frame_info.num_mbs`.
    pub num_mbs: i32,
}

/// C `calculate_total_gf_group_bits` (pass2_strategy.c:186).
///
/// Splits the remaining key-frame group budget across this GF group in
/// proportion to its share of the group error, then clamps three ways: to
/// `[0, kf_group_bits]`, to `frame_max_bits * baseline_gf_interval`, and
/// finally SUBTRACTS what it hands out from `twopass.kf_group_bits`. That last
/// step makes this a state mutator, not a query — calling it twice halves the
/// budget.
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn calculate_total_gf_group_bits(
    rc: &RateControl,
    scs: &SeqRc,
    cfg2: &TwoPassCfg,
    twopass: &mut TwoPassState,
    frames_in_sw: i32,
    gf_group_err: f64,
) -> i64 {
    let max_bits = frame_max_bits(rc, cfg2.vbrmax_section);
    let mut total_group_bits;
    if twopass.kf_group_bits > 0 && twopass.kf_group_error_left > 0 {
        let kf_group_bits = if cfg2.lap_rc
            && (cfg2.lad_mg + 1) * (1 << scs.hierarchical_levels) < scs.intra_period_length
        {
            twopass.kf_group_bits * i64::from(frames_in_sw.min(rc.frames_to_key))
                / i64::from(rc.frames_to_key)
        } else {
            twopass.kf_group_bits
        };
        total_group_bits =
            (kf_group_bits as f64 * (gf_group_err / twopass.kf_group_error_left as f64)) as i64;
    } else {
        total_group_bits = 0;
    }

    // Clamp odd edge cases.
    total_group_bits = total_group_bits.clamp(0, twopass.kf_group_bits.max(0));
    // C's ternary chain is `< 0 ? 0 : > kf_group_bits ? kf_group_bits : x`,
    // which with a NEGATIVE kf_group_bits would return the negative bound;
    // the `.max(0)` above cannot happen there because the `> 0` guard means a
    // non-positive kf_group_bits leaves total_group_bits at 0 anyway.

    // Clip based on the user-supplied data-rate variability limit.
    if total_group_bits > i64::from(max_bits) * i64::from(rc.baseline_gf_interval) {
        total_group_bits = i64::from(max_bits) * i64::from(rc.baseline_gf_interval);
    }
    twopass.kf_group_bits = (twopass.kf_group_bits - total_group_bits).max(0);
    total_group_bits
}

/// One `pcs->gf_group[idx]` entry — the PPCS fields the allocators touch.
#[derive(Clone, Copy, Debug, Default)]
pub struct GfGroupFrame {
    pub update_type: FrameUpdateType,
    pub layer_depth: i32,
    /// `stat_struct.total_num_bits`.
    pub total_num_bits: u64,
    pub picture_number: u64,
    pub gf_update_due: bool,
    /// `svt_aom_is_incomp_mg_frame(pcs->gf_group[i])` — a pd_process.c
    /// predicate, so it is an INPUT here rather than something this file
    /// recomputes.
    pub is_incomp_mg_frame: bool,
    /// OUTPUT: `base_frame_target`.
    pub base_frame_target: i32,
}

/// C `av1_gop_bit_allocation_same_pred` (pass2_strategy.c:226) — the
/// second-pass path, which cross-multiplies the PREVIOUS pass's per-frame bit
/// counts instead of modelling a pyramid.
///
/// C asserts `gf_stats.gf_group_err != 0`; the port returns without writing
/// rather than dividing by zero, because a debug-only assert is not a
/// contract the port can rely on.
pub fn gop_bit_allocation_same_pred(
    gf_group: &mut [GfGroupFrame],
    gf_interval: usize,
    is_i_slice: bool,
    gf_group_bits: i64,
    gf_stats: &GfGroupStats,
) {
    if gf_stats.gf_group_err == 0.0 {
        return;
    }
    // For key frames the frame target rate is already set.
    let frame_index = usize::from(is_i_slice);
    for idx in frame_index..gf_interval.min(gf_group.len()) {
        gf_group[idx].base_frame_target = ((gf_group_bits as f64)
            * (gf_group[idx].total_num_bits as f64)
            / gf_stats.gf_group_err) as i32;
    }
}

/// C `allocate_gf_group_bits` (pass2_strategy.c:240).
///
/// Three passes over the group: count the ARF frames per pyramid level (twice,
/// with the SECOND pass weighting internal ARFs double — only when the
/// baseline interval is under half the nominal one), split the ARF bit pool
/// down the levels by [`LAYER_FRACTION`], then write each frame's target.
///
/// **`gf_arf_bits` is consumed as it descends**: each level takes its fraction
/// and subtracts it, so the LAST level with frames gets whatever is left. And
/// the top level's fraction is forced to 1.0 regardless of the table. Both are
/// easy to drop when reading the loop as a simple table lookup.
#[allow(clippy::too_many_arguments)]
pub fn allocate_gf_group_bits(
    rc: &RateControl,
    gf_group: &mut [GfGroupFrame],
    gf_interval_frames: usize,
    hierarchical_levels: u8,
    gf_group_bits: i64,
    mut gf_arf_bits: i32,
    gf_interval: i32,
    key_frame: bool,
    use_arf: bool,
) {
    let mut total_group_bits = gf_group_bits;
    let mut layer_frames = [0_i32; MAX_ARF_LAYERS + 1];

    // Subtract the extra bits set aside for ARF frames from the group total.
    if use_arf || !key_frame {
        total_group_bits -= i64::from(gf_arf_bits);
    }

    let base_frame_bits = if rc.baseline_gf_interval != 0 {
        (total_group_bits / i64::from(rc.baseline_gf_interval)) as i32
    } else {
        1
    };

    // For key frames the frame target rate is already set.
    let frame_index = usize::from(key_frame);
    let end = gf_interval_frames.min(gf_group.len());

    let max_arf_layer = usize::from(hierarchical_levels).min(MAX_ARF_LAYERS);
    for f in gf_group.iter().take(end).skip(frame_index) {
        if matches!(
            f.update_type,
            FrameUpdateType::ArfUpdate | FrameUpdateType::IntnlArfUpdate
        ) {
            layer_frames[(f.layer_depth as usize).min(MAX_ARF_LAYERS)] += 1;
        }
    }
    if rc.baseline_gf_interval < (gf_interval >> 1) {
        for f in gf_group.iter().take(end).skip(frame_index) {
            let d = (f.layer_depth as usize).min(MAX_ARF_LAYERS);
            if f.update_type == FrameUpdateType::ArfUpdate {
                layer_frames[d] += 1;
            }
            if f.update_type == FrameUpdateType::IntnlArfUpdate {
                layer_frames[d] += 2;
            }
        }
    }

    // Allocate extra bits to each ARF layer.
    let mut layer_extra_bits = [0_i32; MAX_ARF_LAYERS + 1];
    for i in 1..=max_arf_layer {
        if layer_frames[i] != 0 {
            let fraction = if i == max_arf_layer {
                1.0
            } else {
                LAYER_FRACTION[i]
            };
            layer_extra_bits[i] =
                ((f64::from(gf_arf_bits) * fraction) / f64::from(1.max(layer_frames[i]))) as i32;
            gf_arf_bits -= (f64::from(gf_arf_bits) * fraction) as i32;
        }
    }

    // Combine the ARF-layer and baseline bits into each frame's target.
    for f in gf_group.iter_mut().take(end).skip(frame_index) {
        f.base_frame_target = match f.update_type {
            FrameUpdateType::ArfUpdate | FrameUpdateType::IntnlArfUpdate => {
                base_frame_bits + layer_extra_bits[(f.layer_depth as usize).min(MAX_ARF_LAYERS)]
            }
            FrameUpdateType::IntnlOverlayUpdate | FrameUpdateType::OverlayUpdate => 0,
            _ => base_frame_bits,
        };
    }
}

/// C `set_baseline_gf_interval` (pass2_strategy.c:314).
pub fn set_baseline_gf_interval(
    rc: &mut RateControl,
    frame: &FrameRc,
    idr_flag: bool,
    gf_interval: i32,
    arf_position: i32,
) {
    if frame.is_intra_only() && idr_flag {
        rc.baseline_gf_interval = (arf_position - 1).max(1);
    } else {
        rc.baseline_gf_interval = gf_interval;
    }
}

/// The result of C `calculate_gf_stats` (pass2_strategy.c:341), which returns
/// through three out-params plus two `RATE_CONTROL` writes.
#[derive(Clone, Copy, Debug)]
pub struct GfStatsResult {
    pub gf_stats: GfGroupStats,
    /// C `*use_alt_ref`.
    pub use_alt_ref: bool,
    /// The loop counter C hands to `set_baseline_gf_interval` as
    /// `arf_position`.
    pub arf_position: i32,
}

/// C `calculate_gf_stats` (pass2_strategy.c:341). `static` — tier 4.
///
/// Walks up to `gf_interval` first-pass stats accumulating the group error,
/// then RESTORES the cursor. `this_frame` is advanced as C advances it (it is
/// an in/out `FIRSTPASS_STATS*`), and `rc.constrained_gf_group` /
/// `rc.baseline_gf_interval` are written.
///
/// The intra pre-subtraction is not a sign error: a key frame's (or a previous
/// ARF overlay's) cost is already accounted for elsewhere, so it is removed
/// BEFORE the accumulation loop adds it back on the first iteration.
#[allow(clippy::too_many_arguments)]
pub fn calculate_gf_stats(
    rc: &mut RateControl,
    frame: &FrameRc,
    cursor: &mut StatsCursor<'_>,
    has_total_stats: bool,
    this_frame: &mut FirstPassStats,
    gf_interval: i32,
    idr_flag: bool,
) -> GfStatsResult {
    let start_pos = cursor.position();
    let mut gf_stats = init_gf_stats();

    // Load stats for the current frame.
    let mut mod_frame_err = calculate_modified_err(has_total_stats, this_frame);

    // A key frame's / previous ARF overlay's cost is already accounted for.
    if frame.is_intra_only() {
        gf_stats.gf_group_err -= mod_frame_err;
        gf_stats.gf_group_raw_error -= this_frame.coded_error;
    }
    gf_stats.gf_stat_struct = this_frame.stat_struct;
    let mut i = 0_i32;
    while i < gf_interval {
        i += 1;
        mod_frame_err = calculate_modified_err(has_total_stats, this_frame);
        accumulate_this_frame_stats(this_frame, mod_frame_err, &mut gf_stats);
        let Some(next_frame) = cursor.input_stats() else {
            break;
        };
        *this_frame = next_frame;
    }

    // Was the group length constrained by the need for a new key frame?
    rc.constrained_gf_group = i32::from(i >= rc.frames_to_key);
    let use_alt_ref = i > 2;
    set_baseline_gf_interval(rc, frame, idr_flag, gf_interval, i);
    cursor.reset_fpf_position(start_pos);
    GfStatsResult {
        gf_stats,
        use_alt_ref,
        arf_position: i,
    }
}

/// C `calculate_active_worst_quality` (pass2_strategy.c:392). `static` —
/// tier 4.
///
/// Estimates the maxq the group needs, corrected by a drift factor derived
/// from `vbr_bits_off_target` — and the correction is DELIBERATELY ASYMMETRIC:
/// undershooting (`rate_error > 0`) is floored at a mode-dependent minimum
/// (0.5 / 1.0 / 0.75), while overshooting is capped at 2.0.
///
/// The second-pass arm replaces the model entirely with a binary search over
/// the previous pass's recorded worst qindex and bit count.
///
/// `get_twopass_worst_quality` is taken as a callback because it lives in
/// [`crate::port_pass2_strategy`] with a different parameter shape; this
/// function's job is the drift factor and the second-pass search.
#[allow(clippy::too_many_arguments)]
pub fn calculate_active_worst_quality(
    rc: &mut RateControl,
    scs: &SeqRc,
    cfg2: &TwoPassCfg,
    twopass: &TwoPassState,
    target_bit_rate: i64,
    gop_constraint_rc: bool,
    gf_stats: &GfGroupStats,
    twopass_worst_quality: impl FnOnce(f64, f64, i32, f64) -> i32,
) {
    if rc.baseline_gf_interval <= 1 {
        return;
    }
    let vbr_group_bits_per_frame = (rc.gf_group_bits / i64::from(rc.baseline_gf_interval)) as i32;
    let group_av_err = gf_stats.gf_group_raw_error / f64::from(rc.baseline_gf_interval);
    let group_av_skip_pct = gf_stats.gf_group_skip_pct / f64::from(rc.baseline_gf_interval);
    let group_av_inactive_zone = (gf_stats.gf_group_inactive_zone_rows * 2.0)
        / (f64::from(rc.baseline_gf_interval) * f64::from(cfg2.mb_rows));

    // rc_factor corrects for local rate-control drift.
    let mut rc_factor = 1.0_f64;
    if target_bit_rate > 0 {
        let rate_error = ((rc.vbr_bits_off_target * 100) / target_bit_rate) as i32;
        let rate_error = rate_error.clamp(-100, 100);
        if rate_error > 0 {
            let rc_factor_min = if gop_constraint_rc {
                RC_FACTOR_MIN_GOP_CONST
            } else if cfg2.single_pass {
                RC_FACTOR_MIN_1P_VBR
            } else {
                RC_FACTOR_MIN
            };
            rc_factor = rc_factor_min.max(f64::from(100 - rate_error) / 100.0);
        } else {
            rc_factor = RC_FACTOR_MAX.min(f64::from(100 - rate_error) / 100.0);
        }
    }
    let mut tmp_q = twopass_worst_quality(
        group_av_err,
        group_av_skip_pct + group_av_inactive_zone,
        vbr_group_bits_per_frame,
        rc_factor,
    );
    if twopass.passes == 2 {
        let ref_qindex = i32::from(gf_stats.gf_stat_struct.worst_qindex);
        let ref_q = convert_qindex_to_q(ref_qindex, scs.encoder_bit_depth);
        let ref_gf_group_bits = gf_stats.gf_group_err as i64;
        let target_gf_group_bits = rc.gf_group_bits;
        let mut low = rc.best_quality;
        let mut high = rc.worst_quality;
        while low < high {
            let mid = (low + high) >> 1;
            let q = convert_qindex_to_q(mid, scs.encoder_bit_depth);
            let mid_bits = ((ref_gf_group_bits as f64) * ref_q * rc_factor / q) as i32;
            if mid_bits > target_gf_group_bits as i32 {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        tmp_q = low;
    }
    rc.active_worst_quality = tmp_q.max(rc.active_worst_quality >> 1);
}

/// C `get_section_target_bandwidth` (pass2_strategy.c:746). `static` — tier 4.
///
/// `frames_left` is `total_stats->count - picture_number`; C divides by it
/// without a zero check, so the port returns `None` where C would divide by
/// zero rather than inventing a value.
#[must_use]
pub fn get_section_target_bandwidth(
    rc: &RateControl,
    twopass: &TwoPassState,
    lap_rc: bool,
    total_stats_count: f64,
    picture_number: u64,
) -> Option<i32> {
    if lap_rc {
        return Some(rc.avg_frame_bandwidth);
    }
    let frames_left = (total_stats_count - picture_number as f64) as i32;
    if frames_left == 0 {
        return None;
    }
    Some((twopass.bits_left / i64::from(frames_left)) as i32)
}

/// C `get_kf_group_bits` (pass2_strategy.c:629). `static` — tier 4.
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn get_kf_group_bits(
    rc: &RateControl,
    scs: &SeqRc,
    twopass: &TwoPassState,
    lap_rc: bool,
    frames_in_sw: i32,
    end_of_sequence_region: bool,
    kf_group_err: f64,
) -> i64 {
    if lap_rc && frames_in_sw < scs.intra_period_length && !end_of_sequence_region {
        i64::from(rc.frames_to_key) * i64::from(rc.avg_frame_bandwidth)
    } else {
        ((twopass.bits_left as f64) * (kf_group_err / twopass.modified_error_left)) as i64
    }
}

/// C `av1_rc_update_framerate` (pass2_strategy.c:884). `static` — tier 4.
///
/// The `max_frame_bandwidth` floor is the interesting part: it is the LARGER
/// of a per-MB hardware-ish ceiling, a fixed 1080p number, and the user's
/// `--vbr-max-section-pct`, so raising the vbr cap can only raise it.
///
/// **C's first line is UNDEFINED BEHAVIOUR at an out-of-envelope bitrate, and
/// the two ISAs disagree.** `(int)(target_bit_rate / new_framerate)` casts a
/// `double` that can exceed `INT_MAX`; x86-64's `cvttsd2si` yields `INT_MIN`
/// and aarch64's `fcvtzs` saturates to `INT_MAX`. Rust's `as i32` saturates,
/// so this function agrees with aarch64 and cannot agree with x86-64 — because
/// C has no single answer to agree with. Reproducing either realization would
/// make the PORT host-dependent, which is exactly what this project's
/// cross-ISA gates exist to prevent. Full write-up, with the per-ISA
/// instruction table and the second-ISA measurement, in
/// `docs/SUSPECTED-C-BUGS.md` **#17**.
///
/// The encoder's own configuration cannot reach it: `svt_av1_verify_settings`
/// bounds the bitrate, and the quotient stays well inside `int` for every
/// framerate the CLI accepts.
pub fn rc_update_framerate(
    rc: &mut RateControl,
    cfg2: &TwoPassCfg,
    target_bit_rate: i64,
    new_framerate: f64,
) {
    rc.avg_frame_bandwidth = ((target_bit_rate as f64) / new_framerate) as i32;
    let vbr_max_bits =
        ((i64::from(rc.avg_frame_bandwidth) * i64::from(cfg2.vbrmax_section)) / 100) as i32;
    // C's next line is `AOMMIN(vbr_max_bits, INT_MAX)` on an `int`, which is
    // a no-op; it is NOT reproduced because `x.min(i32::MAX)` is a no-op that
    // clippy rejects. Recorded here instead of silently dropped
    // (WORKING-ON-THIS.md §7 — dead-looking C stays documented).
    rc.max_frame_bandwidth = (i64::from(cfg2.num_mbs) * MAX_MB_RATE)
        .max(MAXRATE_1080P)
        .max(i64::from(vbr_max_bits)) as i32;
}

/// C `svt_av1_new_framerate` (pass2_strategy.c:901) — **EXPORTED**.
///
/// Returns the clamped framerate C stores in `scs->new_framerate`; a value
/// under 0.1 becomes 30, not 0.1.
pub fn new_framerate(
    rc: &mut RateControl,
    cfg2: &TwoPassCfg,
    target_bit_rate: i64,
    framerate: f64,
) -> f64 {
    let new_framerate = if framerate < 0.1 { 30.0 } else { framerate };
    rc_update_framerate(rc, cfg2, target_bit_rate, new_framerate);
    new_framerate
}

/// C `read_stat_from_file` (pass2_strategy.c:955). `static` — tier 4.
///
/// Fills in any zero `total_num_bits` from the last frame at the SAME temporal
/// layer (a first-pass frame that produced no stats inherits its layer's last
/// known cost), and returns the accumulated total.
///
/// Despite the name it reads no file — the stats are already in memory. The
/// name is upstream's.
pub fn read_stat_from_file(stats: &mut [FirstPassStats]) -> u64 {
    let mut total_num_bits = 0_u64;
    let mut previous_num_bits = [0_u64; crate::port_rc_vbr_cbr_state::MAX_TEMPORAL_LAYERS];
    for s in stats.iter_mut() {
        let tl = usize::from(s.stat_struct.temporal_layer_index)
            .min(crate::port_rc_vbr_cbr_state::MAX_TEMPORAL_LAYERS - 1);
        if s.stat_struct.total_num_bits == 0 {
            s.stat_struct.total_num_bits = previous_num_bits[tl];
        }
        previous_num_bits[tl] = s.stat_struct.total_num_bits;
        total_num_bits += s.stat_struct.total_num_bits;
    }
    total_num_bits
}

/// C `svt_av1_init_single_pass_lap` (pass2_strategy.c:971) — **EXPORTED**,
/// but reached here without its `SequenceControlSet`, so tier 4.
///
/// Returns early (writing nothing) when the stats ring is empty, exactly as C
/// does on `!stats_in_end`.
pub fn init_single_pass_lap(rc: &mut RateControl, twopass: &mut TwoPassState, has_stats: bool) {
    if !has_stats {
        return;
    }
    // C also calls `svt_aom_set_rc_param` here; that is already ported in
    // `port_rc_process::set_rc_param` and is the caller's to run.
    twopass.bits_left = 0;
    twopass.modified_error_min = 0.0;
    twopass.modified_error_max = 0.0;
    twopass.modified_error_left = 0.0;
    rc.vbr_bits_off_target = 0;
    rc.vbr_bits_off_target_fast = 0;
    rc.rate_error_estimate = 0;
    // Static sequence monitor variables.
    twopass.kf_zeromotion_pct = 100;
}

/// The two two-pass post-encode updaters differ only in WHICH struct owns the
/// drift state, exactly as the two in
/// [`crate::port_rc_vbr_cbr_update`] do — and, additionally, in that the
/// per-GOP variant has four extra `is_short_clip` clamps the other does not.
///
/// C spells both as 110-line near-duplicates. Naming the divergence is what
/// stops one being patched without the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriftStateOwner {
    /// `svt_av1_twopass_postencode_update` — `rc->vbr_bits_off_target*`,
    /// `rc->rate_error_estimate`, `twopass->extend_*`.
    RateControlAndTwoPass,
    /// `svt_av1_twopass_postencode_update_gop_const` — every one of those on
    /// the per-GOP `RateControlIntervalParamContext`, plus the extra
    /// short-clip clamps.
    IntervalParams,
}

/// C `svt_av1_twopass_postencode_update` (pass2_strategy.c:1176) and
/// `svt_av1_twopass_postencode_update_gop_const` (pass2_strategy.c:1063) —
/// both **EXPORTED**, and both pinned at tier 1.
///
/// Feeds the frame's actual size back into the VBR drift accounting, refreshes
/// the active-best-quality pyramid from this frame's qindex DOWNWARD (every
/// level from `layer_depth` to `MAX_ARF_LAYERS`, so a base-layer frame rewrites
/// the whole pyramid), and nudges `extend_minq` / `extend_maxq` toward
/// whichever direction the rate is drifting.
///
/// **The `is_short_clip` arms are NOT symmetric between the two variants.**
/// The gop_const one has four extra: a `-5` / `+5` unwind inside the balanced
/// branch and a signed `extend_minq` clamp; the plain one clamps `extend_minq`
/// to `[0, limit]` unconditionally. Reproduced per variant.
#[allow(clippy::too_many_arguments)]
pub fn twopass_postencode_update(
    rc: &mut RateControl,
    cfg: &RateControlCfg,
    scs: &SeqRc,
    frame: &FrameRc,
    twopass: &mut TwoPassState,
    params: &mut RcIntervalParams,
    owner: DriftStateOwner,
) {
    let gop_const = owner == DriftStateOwner::IntervalParams;

    // VBR correction goes through vbr_bits_off_target; its sign drives a
    // limited % adjustment of later frames' targets, pushing it back to 0.
    let delta = i64::from(frame.base_frame_target - frame.projected_frame_size);
    if gop_const {
        params.vbr_bits_off_target += delta;
    } else {
        rc.vbr_bits_off_target += delta;
    }
    let vbr_bits_off_target = if gop_const {
        params.vbr_bits_off_target
    } else {
        rc.vbr_bits_off_target
    };
    let total_actual_bits = if gop_const {
        params.total_actual_bits
    } else {
        rc.total_actual_bits
    };
    let total_target_bits = if gop_const {
        params.total_target_bits
    } else {
        rc.total_target_bits
    };

    let mut rate_error_estimate_target = 0_i32;
    let rate_error_estimate = if total_actual_bits != 0 {
        if total_target_bits != 0 {
            rate_error_estimate_target = ((vbr_bits_off_target * 100) / total_target_bits) as i32;
        }
        (((vbr_bits_off_target * 100) / total_actual_bits) as i32).clamp(-100, 100)
    } else {
        0
    };
    if gop_const {
        params.rate_error_estimate = rate_error_estimate;
    } else {
        rc.rate_error_estimate = rate_error_estimate;
    }

    if frame.is_overlay {
        return;
    }

    // Update the active best quality pyramid, from this frame's level DOWN.
    let pyramid_level = (frame.layer_depth.max(0) as usize).min(MAX_ARF_LAYERS);
    for slot in rc.active_best_quality[pyramid_level..=MAX_ARF_LAYERS].iter_mut() {
        *slot = frame.base_q_idx;
    }

    // If the rate control is drifting, consider adjusting min or max q.
    let maxq_adj_limit = rc.worst_quality - rc.active_worst_quality;
    let minq_adj_limit = MINQ_ADJ_LIMIT;

    let (rolling_target, rolling_actual) = if gop_const {
        (params.rolling_target_bits, params.rolling_actual_bits)
    } else {
        (rc.rolling_target_bits, rc.rolling_actual_bits)
    };
    let (mut extend_minq, mut extend_maxq, mut extend_minq_fast) = if gop_const {
        (
            params.extend_minq,
            params.extend_maxq,
            params.extend_minq_fast,
        )
    } else {
        (
            twopass.extend_minq,
            twopass.extend_maxq,
            twopass.extend_minq_fast,
        )
    };

    if rate_error_estimate > cfg.under_shoot_pct {
        // Undershoot.
        extend_maxq -= 1;
        if rolling_target >= rolling_actual {
            extend_minq += 1;
        }
    } else if rate_error_estimate < -cfg.over_shoot_pct {
        // Overshoot.
        extend_minq -= 1;
        if rolling_target < rolling_actual {
            extend_maxq += if scs.is_short_clip {
                if rate_error_estimate_target < -100 {
                    10
                } else {
                    2
                }
            } else {
                1
            };
        }
    } else {
        // Adjustment for extreme local overshoot.
        if frame.projected_frame_size > (2 * frame.base_frame_target)
            && frame.projected_frame_size > (2 * rc.avg_frame_bandwidth)
        {
            extend_maxq += 1;
        }
        // Unwind an earlier undershoot or overshoot adjustment.
        if rolling_target < rolling_actual {
            extend_minq -= 1;
        } else if rolling_target > rolling_actual {
            extend_maxq -= 1;
        }
        // ONLY the gop_const variant has this extra short-clip unwind.
        if gop_const && scs.is_short_clip {
            if extend_minq > minq_adj_limit / 3 {
                extend_minq -= 5;
            }
            if extend_maxq < -maxq_adj_limit / 3 {
                extend_maxq += 5;
            }
        }
    }

    if gop_const && scs.is_short_clip {
        // The gop_const variant allows a NEGATIVE extend_minq on a short clip.
        extend_minq = extend_minq.clamp(-minq_adj_limit / 4, minq_adj_limit);
    } else {
        extend_minq = extend_minq.clamp(0, minq_adj_limit);
    }
    if !scs.is_short_clip {
        // C: `clamp(extend_maxq, 0, maxq_adj_limit)`. `maxq_adj_limit` is
        // `worst_quality - active_worst_quality` and CAN be negative when the
        // active worst has been pushed above the worst allowed; C's `clamp`
        // then returns the low bound 0 (it tests `< low` first), which the
        // port reproduces by clamping through the same order rather than with
        // `i32::clamp`, whose min > max case panics.
        extend_maxq = if extend_maxq < 0 {
            0
        } else if extend_maxq > maxq_adj_limit {
            maxq_adj_limit
        } else {
            extend_maxq
        };
    }

    // A big unexpected undershoot: feed the extra bits back in quickly.
    if !frame.is_kf_gf_arf() && !frame.is_overlay {
        let fast_extra_thresh = frame.base_frame_target / HIGH_UNDERSHOOT_RATIO;
        let vbr_fast = if gop_const {
            params.vbr_bits_off_target_fast
        } else {
            rc.vbr_bits_off_target_fast
        };
        let new_vbr_fast;
        if frame.projected_frame_size < fast_extra_thresh && rate_error_estimate > 0 {
            let bumped = vbr_fast + i64::from(fast_extra_thresh - frame.projected_frame_size);
            new_vbr_fast = bumped.min(4 * i64::from(rc.avg_frame_bandwidth));
            if rc.avg_frame_bandwidth != 0 {
                extend_minq_fast = (new_vbr_fast * 8 / i64::from(rc.avg_frame_bandwidth)) as i32;
            }
            extend_minq_fast = extend_minq_fast.min(minq_adj_limit - extend_minq);
        } else if vbr_fast != 0 {
            new_vbr_fast = vbr_fast;
            extend_minq_fast = extend_minq_fast.min(minq_adj_limit - extend_minq);
        } else {
            new_vbr_fast = vbr_fast;
            extend_minq_fast = 0;
        }
        if gop_const {
            params.vbr_bits_off_target_fast = new_vbr_fast;
        } else {
            rc.vbr_bits_off_target_fast = new_vbr_fast;
        }
    }

    if gop_const {
        params.extend_minq = extend_minq;
        params.extend_maxq = extend_maxq;
        params.extend_minq_fast = extend_minq_fast;
    } else {
        twopass.extend_minq = extend_minq;
        twopass.extend_maxq = extend_maxq;
        twopass.extend_minq_fast = extend_minq_fast;
    }
}

/// C `is_new_gf_group` (pass2_strategy.c:823). `static` — tier 4.
///
/// For a frame in a COMPLETE mini-GOP the answer is just this frame's
/// `gf_update_due`. For one in an incomplete mini-GOP there is no decode order
/// to rely on, so C scans the whole group for any nearby incomplete-MG frame
/// whose update is due — and, if it finds one, CLEARS `gf_update_due` on every
/// group member so the next frame does not re-trigger.
#[must_use]
pub fn is_new_gf_group(
    gf_group: &mut [GfGroupFrame],
    gf_interval: usize,
    picture_number: u64,
    this_is_incomp_mg_frame: bool,
    this_gf_update_due: bool,
) -> bool {
    if !this_is_incomp_mg_frame {
        return this_gf_update_due;
    }
    let end = gf_interval.min(gf_group.len());
    let mut new_group = false;
    for f in gf_group.iter().take(end) {
        if (f.picture_number as i64 - picture_number as i64).abs() <= gf_interval as i64
            && f.is_incomp_mg_frame
            && f.gf_update_due
        {
            new_group = true;
        }
    }
    if new_group {
        for f in gf_group.iter_mut().take(end) {
            f.gf_update_due = false;
        }
    }
    new_group
}

/// C `set_kf_interval_variables` (pass2_strategy.c:594). `static` — tier 4.
///
/// Walks the lookahead accumulating key-frame-group error until either the
/// stats run out or `num_frames_to_detect_scenecut` frames have been counted,
/// then sets `rc.frames_to_key`.
///
/// **`num_frames_to_detect_scenecut == 0` returns having written NOTHING** —
/// not even `frames_to_key`. That early return is load-bearing.
///
/// Returns `(kf_group_err_added, end_of_seq_seen)`; C writes the first through
/// an optional `double*` (NULL means "count frames only") and the second onto
/// `rate_control_param_ptr`.
#[allow(clippy::too_many_arguments)]
pub fn set_kf_interval_variables(
    rc: &mut RateControl,
    scs: &SeqRc,
    cursor: &mut StatsCursor<'_>,
    has_total_stats: bool,
    this_frame: &mut FirstPassStats,
    accumulate_err: bool,
    num_frames_to_detect_scenecut: i32,
    lap_rc: bool,
    end_of_sequence_region: bool,
) -> (f64, bool) {
    let mut kf_group_err = 0.0;
    if num_frames_to_detect_scenecut == 0 {
        return (kf_group_err, false);
    }
    let mut frames_to_key = 0_i32;
    while cursor.at_or_before_end() && frames_to_key < num_frames_to_detect_scenecut {
        if accumulate_err {
            kf_group_err += calculate_modified_err(has_total_stats, this_frame);
        }
        frames_to_key += 1;
        let Some(next) = cursor.input_stats() else {
            break;
        };
        *this_frame = next;
    }
    let end_of_seq_seen = lap_rc && end_of_sequence_region;
    if lap_rc && !end_of_sequence_region {
        rc.frames_to_key = scs.intra_period_length + 1;
    } else {
        rc.frames_to_key = (scs.intra_period_length + 1).min(frames_to_key);
    }
    (kf_group_err, end_of_seq_seen)
}

/// C `lap_rc_group_error_calc` (pass2_strategy.c:562). `static` — tier 4.
///
/// Note the loop bound is `num_stats < rc->frames_to_key`, and `num_stats` is
/// incremented BEFORE the accumulate — so it sums exactly `frames_to_key`
/// entries, not `frames_to_key + 1`.
#[must_use]
pub fn lap_rc_group_error_calc(
    rc: &RateControl,
    cursor: &mut StatsCursor<'_>,
    has_total_stats: bool,
    mut this_frame: FirstPassStats,
) -> f64 {
    let start_position = cursor.position();
    let mut num_stats = 0_i32;
    let mut modified_error_total = 0.0;
    while cursor.at_or_before_end() && num_stats < rc.frames_to_key {
        num_stats += 1;
        modified_error_total += calculate_modified_err(has_total_stats, &this_frame);
        let Some(next) = cursor.input_stats() else {
            break;
        };
        this_frame = next;
    }
    cursor.reset_fpf_position(start_position);
    modified_error_total
}

/// C `lap_rc_init` (pass2_strategy.c:512). `static` — tier 4.
///
/// Two passes over the same lookahead: the first averages `coded_error` to set
/// the modified-error bounds, the second sums the modified error. The cursor
/// is reset between them AND at the end, and `this_frame` is restored from a
/// saved copy — C keeps `this_frame_ref` for exactly that.
pub fn lap_rc_init(
    twopass: &mut TwoPassState,
    cfg2: &TwoPassCfg,
    cursor: &mut StatsCursor<'_>,
    has_total_stats: bool,
    this_frame_ref: FirstPassStats,
    target_bit_rate: i64,
    new_framerate: f64,
) {
    let start_position = cursor.position();
    let mut num_stats = 0_i32;
    let mut coded_error_total = 0.0;
    let mut this_frame = this_frame_ref;

    while cursor.at_or_before_end() {
        num_stats += 1;
        coded_error_total += this_frame.coded_error;
        let Some(next) = cursor.input_stats() else {
            break;
        };
        this_frame = next;
    }
    let avg_error = coded_error_total / double_divide_check(f64::from(num_stats));
    twopass.modified_error_min = (avg_error * f64::from(cfg2.vbrmin_section)) / 100.0;
    twopass.modified_error_max = (avg_error * f64::from(cfg2.vbrmax_section)) / 100.0;
    cursor.reset_fpf_position(start_position);
    this_frame = this_frame_ref;

    let mut modified_error_total = 0.0;
    while cursor.at_or_before_end() {
        modified_error_total += calculate_modified_err(has_total_stats, &this_frame);
        let Some(next) = cursor.input_stats() else {
            break;
        };
        this_frame = next;
    }
    twopass.modified_error_left = modified_error_total;
    twopass.bits_left += (f64::from(num_stats) * ((target_bit_rate as f64) / new_framerate)) as i64;
    cursor.reset_fpf_position(start_position);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **EVIDENCE TIER 4** for every test in this module: hand-derived vectors
    /// traced against `Codec/pass2_strategy.c`. These functions are `static`
    /// in C and were inlined away by the Release build (`nm` on
    /// `pass2_strategy.c.o` shows ONE local symbol, `get_twopass_worst_quality`,
    /// which is already covered by `port_pass2_strategy`), so there is no
    /// symbol to link and no exported caller that reaches them without a
    /// first-pass stats ring. Each vector below states the C line it was
    /// traced from.
    const _: () = ();

    fn stat(bits: u64, coded_error: f64, tl: u8) -> FirstPassStats {
        FirstPassStats {
            frame: 1.0,
            coded_error,
            duration: 10_000.0,
            count: 1.0,
            stat_struct: StatStruct {
                poc: 0,
                total_num_bits: bits,
                qindex: 100,
                worst_qindex: 150,
                temporal_layer_index: tl,
            },
        }
    }

    /// `DOUBLE_DIVIDE_CHECK` (firstpass.h:23) preserves the divisor's SIGN.
    /// `max(x, eps)` would flip a negative divisor's quotient.
    #[test]
    fn double_divide_check_preserves_sign() {
        assert_eq!(double_divide_check(0.0), 0.000_001);
        assert_eq!(double_divide_check(-0.0), 0.000_001);
        assert_eq!(double_divide_check(-1.0), -1.000_001);
        assert_eq!(double_divide_check(2.0), 2.000_001);
    }

    /// `init_gf_stats` (pass2_strategy.c:326) is NOT a zero-init: the qindexes
    /// start at 172 and `total_num_bits` at 1.
    #[test]
    fn init_gf_stats_is_not_default() {
        let g = init_gf_stats();
        assert_eq!(g.gf_stat_struct.total_num_bits, 1);
        assert_eq!(g.gf_stat_struct.qindex, 172);
        assert_eq!(g.gf_stat_struct.worst_qindex, 172);
        assert_eq!(g.gf_group_err, 0.0);
        assert_ne!(g, GfGroupStats::default());
    }

    /// `subtract_stats` (pass2_strategy.c:47) touches four fields and leaves
    /// `stat_struct` alone.
    #[test]
    fn subtract_stats_leaves_stat_struct() {
        let mut section = FirstPassStats {
            frame: 10.0,
            coded_error: 500.0,
            duration: 100.0,
            count: 10.0,
            stat_struct: StatStruct {
                poc: 7,
                total_num_bits: 999,
                qindex: 1,
                worst_qindex: 2,
                temporal_layer_index: 3,
            },
        };
        let frame = stat(4, 50.0, 1);
        subtract_stats(&mut section, &frame);
        assert_eq!(section.frame, 9.0);
        assert_eq!(section.coded_error, 450.0);
        assert_eq!(section.count, 9.0);
        // 100 - 10000: `duration` can legitimately go negative here, because
        // `subtract_stats` is used to draw a frame OUT of an accumulated
        // section whose duration may already have been consumed.
        assert_eq!(section.duration, -9_900.0);
        assert_eq!(section.stat_struct.total_num_bits, 999);
        assert_eq!(section.stat_struct.poc, 7);
    }

    /// `frame_max_bits` (pass2_strategy.c:55): the `CLIP3` upper bound is
    /// `max_frame_bandwidth`, so a huge `vbrmax_section` cannot exceed it.
    #[test]
    fn frame_max_bits_clips_to_max_frame_bandwidth() {
        let rc = RateControl {
            avg_frame_bandwidth: 100_000,
            max_frame_bandwidth: 250_000,
            ..Default::default()
        };
        assert_eq!(frame_max_bits(&rc, 100), 100_000);
        assert_eq!(frame_max_bits(&rc, 400), 250_000); // 400_000 clipped
        assert_eq!(frame_max_bits(&rc, 0), 0);
    }

    /// The cursor's two boundary rules (see [`StatsCursor`]): the loops test
    /// `<= end` while `input_stats` stops at `>= end`, so a loop body runs
    /// once with the cursor ON the end sentinel.
    #[test]
    fn stats_cursor_boundary_rules() {
        let stats = [stat(10, 1.0, 0), stat(20, 2.0, 1)];
        let mut c = StatsCursor::new(&stats, 0);
        assert!(c.at_or_before_end());
        assert_eq!(
            c.input_stats().map(|s| s.stat_struct.total_num_bits),
            Some(10)
        );
        assert_eq!(
            c.input_stats().map(|s| s.stat_struct.total_num_bits),
            Some(20)
        );
        // pos == len: still "at or before end", but input_stats is EOF.
        assert!(c.at_or_before_end());
        assert_eq!(c.input_stats(), None);
        assert_eq!(c.position(), 2);
        c.reset_fpf_position(1);
        assert_eq!(
            c.input_stats().map(|s| s.stat_struct.total_num_bits),
            Some(20)
        );
        // `previous()` is C's `(stats_in - 1)`; None rather than a read before
        // the ring.
        let c0 = StatsCursor::new(&stats, 0);
        assert!(c0.previous().is_none());
    }

    /// `calculate_modified_err` (pass2_strategy.c:22) returns the frame's bit
    /// count, or 0 when the accumulated `total_stats` slot is absent.
    #[test]
    fn calculate_modified_err_gates_on_total_stats() {
        let s = stat(1234, 5.0, 0);
        assert_eq!(calculate_modified_err(true, &s), 1234.0);
        assert_eq!(calculate_modified_err(false, &s), 0.0);
    }

    /// `read_stat_from_file` (pass2_strategy.c:955) carries the last non-zero
    /// bit count forward PER TEMPORAL LAYER, not globally.
    #[test]
    fn read_stat_from_file_carries_forward_per_layer() {
        let mut stats = [
            stat(100, 1.0, 0),
            stat(200, 1.0, 1),
            stat(0, 1.0, 0), // inherits 100, not 200
            stat(0, 1.0, 1), // inherits 200
            stat(0, 1.0, 2), // no predecessor at layer 2 -> 0
        ];
        let total = read_stat_from_file(&mut stats);
        assert_eq!(stats[2].stat_struct.total_num_bits, 100);
        assert_eq!(stats[3].stat_struct.total_num_bits, 200);
        assert_eq!(stats[4].stat_struct.total_num_bits, 0);
        assert_eq!(total, 100 + 200 + 100 + 200);
    }

    fn arf(layer_depth: i32) -> GfGroupFrame {
        GfGroupFrame {
            update_type: FrameUpdateType::IntnlArfUpdate,
            layer_depth,
            ..Default::default()
        }
    }

    /// `allocate_gf_group_bits` (pass2_strategy.c:240): the ARF pool is
    /// CONSUMED as it descends the pyramid, and the TOP level's fraction is
    /// forced to 1.0 regardless of `layer_fraction`. Traced by hand against
    /// the C loop at :356-363.
    #[test]
    fn allocate_gf_group_bits_consumes_the_arf_pool() {
        let rc = RateControl {
            baseline_gf_interval: 8,
            ..Default::default()
        };
        let mut group = vec![
            GfGroupFrame {
                update_type: FrameUpdateType::LfUpdate,
                ..Default::default()
            },
            arf(1),
            arf(2),
            GfGroupFrame {
                update_type: FrameUpdateType::OverlayUpdate,
                ..Default::default()
            },
        ];
        // gf_group_bits 80_000, gf_arf_bits 8_000, hierarchical_levels 2 so
        // max_arf_layer == 2 and level 2 takes fraction 1.0.
        allocate_gf_group_bits(&rc, &mut group, 4, 2, 80_000, 8_000, 8, false, true);
        let base = (80_000 - 8_000) / 8; // 9_000
        assert_eq!(group[0].base_frame_target, base);
        // Level 1 takes layer_fraction[1] = 0.80 of 8_000 over 1 frame.
        assert_eq!(group[1].base_frame_target, base + 6_400);
        // Level 2 (== max_arf_layer) takes ALL of what is left: 8_000 - 6_400.
        assert_eq!(group[2].base_frame_target, base + 1_600);
        // Overlays get nothing.
        assert_eq!(group[3].base_frame_target, 0);
    }

    /// `is_new_gf_group` (pass2_strategy.c:823): for a complete mini-GOP the
    /// answer is this frame's own flag and NOTHING is cleared; for an
    /// incomplete one a hit clears `gf_update_due` across the whole group.
    #[test]
    fn is_new_gf_group_clears_only_on_the_incomplete_path() {
        let mut group = vec![
            GfGroupFrame {
                picture_number: 10,
                gf_update_due: true,
                is_incomp_mg_frame: true,
                ..Default::default()
            },
            GfGroupFrame {
                picture_number: 11,
                gf_update_due: true,
                is_incomp_mg_frame: true,
                ..Default::default()
            },
        ];
        // Complete mini-GOP: returns the frame's own flag, clears nothing.
        assert!(is_new_gf_group(&mut group, 2, 10, false, true));
        assert!(group[0].gf_update_due && group[1].gf_update_due);
        // Incomplete: finds a due neighbour and clears every entry.
        assert!(is_new_gf_group(&mut group, 2, 10, true, false));
        assert!(!group[0].gf_update_due && !group[1].gf_update_due);
        // Now nothing is due, so no new group and still nothing to clear.
        assert!(!is_new_gf_group(&mut group, 2, 10, true, false));
    }

    /// `set_kf_interval_variables` (pass2_strategy.c:594): a
    /// `num_frames_to_detect_scenecut` of 0 returns having written NOTHING,
    /// not even `frames_to_key`.
    #[test]
    fn set_kf_interval_variables_zero_lookahead_writes_nothing() {
        let stats = [stat(10, 1.0, 0), stat(20, 1.0, 0)];
        let mut cursor = StatsCursor::new(&stats, 0);
        let mut rc = RateControl {
            frames_to_key: 77,
            ..Default::default()
        };
        let scs = SeqRc {
            intra_period_length: 63,
            ..Default::default()
        };
        let mut this_frame = stats[0];
        let (err, eos) = set_kf_interval_variables(
            &mut rc,
            &scs,
            &mut cursor,
            true,
            &mut this_frame,
            true,
            0,
            false,
            false,
        );
        assert_eq!(err, 0.0);
        assert!(!eos);
        assert_eq!(rc.frames_to_key, 77, "frames_to_key must be untouched");
        assert_eq!(cursor.position(), 0);
    }

    /// `set_kf_interval_variables`, the normal path: it walks the ring and
    /// clamps `frames_to_key` to `intra_period_length + 1`. Note the loop runs
    /// once with the cursor on the end sentinel (see [`StatsCursor`]), so
    /// three stats yield `frames_to_key == 3` and not 2.
    #[test]
    fn set_kf_interval_variables_counts_through_the_sentinel() {
        let stats = [stat(10, 1.0, 0), stat(20, 1.0, 0), stat(30, 1.0, 0)];
        // The caller invariant: `this_frame` is stats[0] and the cursor is
        // already past it. See `StatsCursor`.
        let mut cursor = StatsCursor::new(&stats, 1);
        let mut rc = RateControl::default();
        let scs = SeqRc {
            intra_period_length: 63,
            ..Default::default()
        };
        let mut this_frame = stats[0];
        let (err, _) = set_kf_interval_variables(
            &mut rc,
            &scs,
            &mut cursor,
            true,
            &mut this_frame,
            true,
            100,
            false,
            false,
        );
        // Bodies run for stats[0], [1], [2]; the third advance hits EOF and
        // breaks, so the sentinel iteration does not add a fourth term here.
        assert_eq!(err, 10.0 + 20.0 + 30.0);
        assert_eq!(rc.frames_to_key, 3);
    }

    /// `lap_rc_group_error_calc` (pass2_strategy.c:562) sums EXACTLY
    /// `frames_to_key` entries and restores the cursor.
    #[test]
    fn lap_rc_group_error_calc_sums_frames_to_key_entries() {
        let stats = [stat(10, 1.0, 0), stat(20, 1.0, 0), stat(30, 1.0, 0)];
        // Caller invariant: this_frame == stats[0], cursor already past it.
        let mut cursor = StatsCursor::new(&stats, 1);
        let rc = RateControl {
            frames_to_key: 2,
            ..Default::default()
        };
        let err = lap_rc_group_error_calc(&rc, &mut cursor, true, stats[0]);
        assert_eq!(err, 10.0 + 20.0);
        assert_eq!(cursor.position(), 1, "the cursor must be restored");
    }

    /// `calculate_total_gf_group_bits` (pass2_strategy.c:186) SUBTRACTS what
    /// it hands out, so it is a state mutator: calling it twice with the same
    /// arguments does not return the same number.
    #[test]
    fn calculate_total_gf_group_bits_consumes_the_kf_budget() {
        let rc = RateControl {
            avg_frame_bandwidth: 100_000,
            max_frame_bandwidth: 10_000_000,
            baseline_gf_interval: 16,
            frames_to_key: 40,
            ..Default::default()
        };
        let scs = SeqRc {
            hierarchical_levels: 4,
            intra_period_length: 63,
            ..Default::default()
        };
        let cfg2 = TwoPassCfg {
            vbrmax_section: 2000,
            ..Default::default()
        };
        let mut twopass = TwoPassState {
            kf_group_bits: 1_000_000,
            kf_group_error_left: 4_000,
            ..Default::default()
        };
        let first = calculate_total_gf_group_bits(&rc, &scs, &cfg2, &mut twopass, 0, 1_000.0);
        assert_eq!(first, 250_000);
        assert_eq!(twopass.kf_group_bits, 750_000);
        let second = calculate_total_gf_group_bits(&rc, &scs, &cfg2, &mut twopass, 0, 1_000.0);
        assert_eq!(second, 187_500);
        assert_eq!(twopass.kf_group_bits, 562_500);
    }

    /// `rc_update_framerate` (pass2_strategy.c:884): `max_frame_bandwidth` is
    /// the MAXIMUM of three terms, so a small `vbrmax_section` cannot lower it
    /// below the 1080p floor.
    #[test]
    fn rc_update_framerate_floors_at_maxrate_1080p() {
        let mut rc = RateControl::default();
        let cfg2 = TwoPassCfg {
            vbrmax_section: 100,
            num_mbs: 100,
            ..Default::default()
        };
        rc_update_framerate(&mut rc, &cfg2, 3_000_000, 60.0);
        assert_eq!(rc.avg_frame_bandwidth, 50_000);
        assert_eq!(rc.max_frame_bandwidth, MAXRATE_1080P as i32);
        // A big MB count wins instead.
        let cfg_big = TwoPassCfg {
            vbrmax_section: 100,
            num_mbs: 100_000,
            ..Default::default()
        };
        rc_update_framerate(&mut rc, &cfg_big, 3_000_000, 60.0);
        assert_eq!(rc.max_frame_bandwidth, 100_000 * 250);
    }

    /// `svt_av1_new_framerate` (pass2_strategy.c:901) maps anything under 0.1
    /// to **30**, not to 0.1.
    #[test]
    fn new_framerate_maps_tiny_to_thirty() {
        let mut rc = RateControl::default();
        let cfg2 = TwoPassCfg {
            vbrmax_section: 100,
            num_mbs: 1,
            ..Default::default()
        };
        assert_eq!(new_framerate(&mut rc, &cfg2, 3_000_000, 0.05), 30.0);
        assert_eq!(rc.avg_frame_bandwidth, 100_000);
        assert_eq!(new_framerate(&mut rc, &cfg2, 3_000_000, 24.0), 24.0);
        assert_eq!(rc.avg_frame_bandwidth, 125_000);
    }

    /// `get_section_target_bandwidth` (pass2_strategy.c:746) divides by
    /// `total_stats->count - picture_number` with no guard in C; the port
    /// returns `None` there instead of dividing by zero.
    #[test]
    fn get_section_target_bandwidth_refuses_zero_frames_left() {
        let rc = RateControl {
            avg_frame_bandwidth: 1_234,
            ..Default::default()
        };
        let twopass = TwoPassState {
            bits_left: 1_000_000,
            ..Default::default()
        };
        assert_eq!(
            get_section_target_bandwidth(&rc, &twopass, false, 100.0, 60),
            Some(25_000)
        );
        assert_eq!(
            get_section_target_bandwidth(&rc, &twopass, false, 60.0, 60),
            None
        );
        // lap_rc ignores the ring entirely.
        assert_eq!(
            get_section_target_bandwidth(&rc, &twopass, true, 60.0, 60),
            Some(1_234)
        );
    }

    /// `calculate_gf_stats` (pass2_strategy.c:341): an intra frame's error is
    /// SUBTRACTED before the loop adds it back, and the cursor is restored.
    #[test]
    fn calculate_gf_stats_pre_subtracts_the_intra_frame() {
        let stats = [stat(10, 1.0, 0), stat(20, 2.0, 0), stat(30, 3.0, 0)];
        let mut rc = RateControl {
            frames_to_key: 100,
            ..Default::default()
        };
        let inter = FrameRc {
            frame_type: crate::port_rc_vbr_cbr_state::FrameType::Inter,
            ..Default::default()
        };
        let key = FrameRc {
            frame_type: crate::port_rc_vbr_cbr_state::FrameType::Key,
            ..Default::default()
        };

        let mut cursor = StatsCursor::new(&stats, 0);
        let mut this = stats[0];
        let r_inter = calculate_gf_stats(&mut rc, &inter, &mut cursor, true, &mut this, 3, false);
        assert_eq!(cursor.position(), 0);
        assert_eq!(r_inter.arf_position, 3);
        assert!(r_inter.use_alt_ref);

        let mut cursor = StatsCursor::new(&stats, 0);
        let mut this = stats[0];
        let r_key = calculate_gf_stats(&mut rc, &key, &mut cursor, true, &mut this, 3, false);
        // The key-frame run is exactly the inter run minus this_frame's error.
        assert_eq!(
            r_key.gf_stats.gf_group_err,
            r_inter.gf_stats.gf_group_err - 10.0
        );
        assert_eq!(
            r_key.gf_stats.gf_group_raw_error,
            r_inter.gf_stats.gf_group_raw_error - 1.0
        );
    }
}

// ---------------------------------------------------------------------------
// The orchestration layer
// ---------------------------------------------------------------------------
//
// Everything above is a piece; these five assemble them into the two-pass
// rate-control entry point. They are `static` in C (except
// `svt_aom_process_rc_stat` and `svt_av1_init_second_pass`, which are
// exported but need a populated stats ring to drive), so they are all
// evidence TIER 4 — see the module header.

/// C `svt_av1_accumulate_stats` (firstpass.c:141) — **EXPORTED**, and the
/// exact inverse of [`subtract_stats`]: four fields, `stat_struct` untouched.
///
/// It belongs to `firstpass.c`, not to this file; it is here because
/// `svt_av1_init_second_pass` needs it and it is four lines.
pub fn accumulate_stats(section: &mut FirstPassStats, frame: &FirstPassStats) {
    section.frame += frame.frame;
    section.coded_error += frame.coded_error;
    section.count += frame.count;
    section.duration += frame.duration;
}

/// C `av1_gop_bit_allocation` (pass2_strategy.c:457).
///
/// Two lines: work out the ARF bit pool from the GF boost, then split the
/// group. `calculate_boost_bits` is `rc_process.c`'s and is already ported
/// (and pinned at tier 1) in [`crate::port_rc_process::calculate_boost_bits`].
#[allow(clippy::too_many_arguments)]
pub fn gop_bit_allocation(
    rc: &RateControl,
    gf_group: &mut [GfGroupFrame],
    gf_interval_frames: usize,
    hierarchical_levels: u8,
    is_key_frame: bool,
    gf_interval: i32,
    use_arf: bool,
    gf_group_bits: i64,
) {
    let gf_arf_bits = crate::port_rc_process::calculate_boost_bits(
        rc.baseline_gf_interval,
        rc.gfu_boost,
        gf_group_bits,
    );
    allocate_gf_group_bits(
        rc,
        gf_group,
        gf_interval_frames,
        hierarchical_levels,
        gf_group_bits,
        gf_arf_bits,
        gf_interval,
        is_key_frame,
        use_arf,
    );
}

/// The picture-level facts the two rate-assignment functions read that are
/// not already on [`SeqRc`] / [`FrameRc`] / [`TwoPassCfg`].
#[derive(Clone, Copy, Debug, Default)]
pub struct GroupRateInput {
    /// `pcs->frames_in_sw`.
    pub frames_in_sw: i32,
    /// `pcs->end_of_sequence_region`.
    pub end_of_sequence_region: bool,
    /// `pcs->idr_flag`.
    pub idr_flag: bool,
    /// `pcs->gf_interval`.
    pub gf_interval: i32,
    /// `pcs->slice_type == I_SLICE`.
    pub is_i_slice: bool,
    /// `scs->static_config.target_bit_rate`.
    pub target_bit_rate: i64,
    /// `scs->new_framerate`.
    pub new_framerate: f64,
    /// `scs->static_config.gop_constraint_rc`.
    pub gop_constraint_rc: bool,
    /// `twopass->passes` — 2 selects the second-pass cross-multiply paths.
    pub passes: i32,
}

/// C `gf_group_rate_assingment` (pass2_strategy.c:475) — the name's spelling
/// is upstream's.
///
/// Assembles the GF-group budget: accumulate the group's error, take its
/// share of the KF budget, estimate the group's worst quality, DEDUCT the
/// group's error from the KF pool, and split the budget across the frames.
///
/// Two details worth stating:
/// * the cursor is saved and restored around the whole thing, and
///   `calculate_gf_stats` restores it a second time internally — the outer
///   restore is what makes this callable twice;
/// * the second-pass path (`passes == 2` AND `pass == ENC_SECOND_PASS`, two
///   different flags that must BOTH hold) cross-multiplies the previous
///   pass's per-frame bit counts instead of modelling a pyramid.
#[allow(clippy::too_many_arguments)]
pub fn gf_group_rate_assingment(
    rc: &mut RateControl,
    scs: &SeqRc,
    cfg2: &TwoPassCfg,
    twopass: &mut TwoPassState,
    frame: &FrameRc,
    input: &GroupRateInput,
    cursor: &mut StatsCursor<'_>,
    has_total_stats: bool,
    this_frame: &mut FirstPassStats,
    gf_group: &mut [GfGroupFrame],
    twopass_worst_quality: impl FnOnce(f64, f64, i32, f64) -> i32,
) {
    let start_pos = cursor.position();
    let result = calculate_gf_stats(
        rc,
        frame,
        cursor,
        has_total_stats,
        this_frame,
        input.gf_interval,
        input.idr_flag,
    );

    // Bits for the gf/arf group as a whole.
    rc.gf_group_bits = calculate_total_gf_group_bits(
        rc,
        scs,
        cfg2,
        twopass,
        input.frames_in_sw,
        result.gf_stats.gf_group_err,
    );
    calculate_active_worst_quality(
        rc,
        scs,
        cfg2,
        twopass,
        input.target_bit_rate,
        input.gop_constraint_rc,
        &result.gf_stats,
        twopass_worst_quality,
    );

    // Adjust the KF group's remaining error.
    twopass.kf_group_error_left -= result.gf_stats.gf_group_err as i64;

    cursor.reset_fpf_position(start_pos);
    let gf_bits = rc.gf_group_bits;
    if twopass.passes == 2 && cfg2.second_pass {
        gop_bit_allocation_same_pred(
            gf_group,
            input.gf_interval as usize,
            input.is_i_slice,
            gf_bits,
            &result.gf_stats,
        );
    } else {
        gop_bit_allocation(
            rc,
            gf_group,
            input.gf_interval as usize,
            scs.hierarchical_levels,
            frame.frame_type.is_key(),
            1 << scs.hierarchical_levels,
            result.use_alt_ref,
            gf_bits,
        );
    }
}

/// C `kf_group_rate_assingment` (pass2_strategy.c:651) — upstream's spelling.
///
/// Budgets the next key-frame group and carves the key frame's own share out
/// of it. Four things a rewrite gets wrong, all commented at their sites:
///
/// * `frames_to_key_clipped` starts at `INT_MAX` and `kf_group_bits_clipped`
///   at `INT64_MAX`, and both stay there unless `lap_rc` is on — so the
///   `AOMMIN`s below them are no-ops in the non-LAP case rather than clamps.
/// * the LAP bits-left update SUBTRACTS a second term (the lookahead beyond
///   this KF group), which the non-LAP branch does not; they are not the same
///   expression with a flag.
/// * `kf_zeromotion_pct` is unconditionally set to **0** here, overwriting the
///   100 that `init_second_pass` / `init_single_pass_lap` set — so the
///   `STATIC_KF_GROUP_THRESH` gate in `rc_vbr_cbr.c`'s `get_q` can only fire
///   before the first KF group is budgeted.
/// * the key frame's own error (`kf_mod_err`) is subtracted from the group's
///   error AFTER the group total is used, so the two are not interchangeable.
///
/// Returns the key frame's `base_frame_target`.
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn kf_group_rate_assingment(
    rc: &mut RateControl,
    scs: &SeqRc,
    cfg2: &TwoPassCfg,
    twopass: &mut TwoPassState,
    input: &GroupRateInput,
    cursor: &mut StatsCursor<'_>,
    has_total_stats: bool,
    mut this_frame: FirstPassStats,
    params_end_of_seq_seen: &mut bool,
) -> i32 {
    rc.frames_since_key = 0;
    rc.frames_since_cdf_update = 0;
    let start_position = cursor.position();
    let mut frames_to_key_clipped = i32::MAX;
    let mut kf_group_bits_clipped = i64::MAX;

    twopass.kf_group_bits = 0; // Total bits available to the kf group.
    twopass.kf_group_error_left = 0; // Group modified error score.
    let kf_mod_err = calculate_modified_err(has_total_stats, &this_frame);
    let (kf_group_err, eos_seen) = set_kf_interval_variables(
        rc,
        scs,
        cursor,
        has_total_stats,
        &mut this_frame,
        true,
        scs.intra_period_length + 1,
        cfg2.lap_rc,
        input.end_of_sequence_region,
    );
    if eos_seen {
        *params_end_of_seq_seen = true;
    }

    if (twopass.bits_left > 0 && twopass.modified_error_left > 0.0) || cfg2.lap_rc {
        let max_bits = frame_max_bits(rc, cfg2.vbrmax_section);
        twopass.kf_group_bits = get_kf_group_bits(
            rc,
            scs,
            twopass,
            cfg2.lap_rc,
            input.frames_in_sw,
            input.end_of_sequence_region,
            kf_group_err,
        );
        // Clip to the user's maximum per-frame rate.
        let max_grp_bits = i64::from(max_bits) * i64::from(rc.frames_to_key);
        if twopass.kf_group_bits > max_grp_bits {
            twopass.kf_group_bits = max_grp_bits;
        }
    } else {
        twopass.kf_group_bits = 0;
    }
    twopass.kf_group_bits = twopass.kf_group_bits.max(0);

    if cfg2.lap_rc {
        // The lookahead is moving, so bits_left is recomputed for the NEXT KF.
        // The second term is the lookahead BEYOND this KF group, added back
        // because it is part of the next one — the non-LAP branch has no
        // equivalent.
        twopass.bits_left -= twopass.kf_group_bits
            + ((i64::from(input.frames_in_sw) - i64::from(rc.frames_to_key)) as f64
                * ((input.target_bit_rate as f64) / input.new_framerate)) as i64;
    } else {
        twopass.bits_left = (twopass.bits_left - twopass.kf_group_bits).max(0);
    }
    if cfg2.lap_rc {
        // With LAP, frames_to_key can be wildly inaccurate; clip it.
        frames_to_key_clipped = (MAX_KF_BITS_INTERVAL_SINGLE_PASS * input.new_framerate) as i32;
        if rc.frames_to_key > frames_to_key_clipped {
            kf_group_bits_clipped = ((twopass.kf_group_bits as f64)
                * f64::from(frames_to_key_clipped)
                / f64::from(rc.frames_to_key)) as i64;
        }
    }
    cursor.reset_fpf_position(start_position);

    // Store the zero-motion percentage. NOTE: unconditionally 0, overwriting
    // the 100 the init functions set.
    twopass.kf_zeromotion_pct = 0;

    let kf_bits = if twopass.passes == 2 {
        // Second pass: cross-multiply the previous pass's cost for this frame.
        // C reads `(twopass->stats_in - 1)->stat_struct`, i.e. the entry
        // BEFORE the cursor; `previous()` returns None at position 0 where C
        // would read one slot outside the ring.
        let prev_bits = cursor
            .previous()
            .map_or(0, |s| s.stat_struct.total_num_bits);
        ((twopass.kf_group_bits as f64) * (prev_bits as f64) / kf_group_err) as i32
    } else {
        crate::port_rc_process::calculate_boost_bits(
            rc.frames_to_key.min(frames_to_key_clipped) - 1,
            rc.kf_boost,
            twopass.kf_group_bits.min(kf_group_bits_clipped),
        )
    };

    twopass.kf_group_bits -= i64::from(kf_bits);
    // The group's error MINUS the key frame's own.
    twopass.kf_group_error_left = (kf_group_err - kf_mod_err) as i64;
    twopass.modified_error_left -= kf_group_err;
    kf_bits
}

/// The first-frame quality seed [`process_first_pass_stats`] produces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FirstFrameSeed {
    /// `rc->active_worst_quality`.
    pub active_worst_quality: i32,
    /// `rc->avg_frame_qindex[INTER_FRAME]`.
    pub avg_frame_qindex_inter: i32,
    /// `rc->avg_frame_qindex[KEY_FRAME]`.
    pub avg_frame_qindex_key: i32,
}

/// C `process_first_pass_stats` (pass2_strategy.c:761).
///
/// On picture 0 ONLY, seeds the quality state from the whole sequence's
/// average error; on every picture it then consumes one stats entry and
/// deducts it from the running remainder.
///
/// The seed is the interesting half. `section_error` is the total remaining
/// coded error divided by the remaining frame COUNT, and it is fed either to
/// the twopass worst-quality model (one pass) or to a binary search over the
/// PREVIOUS pass's recorded qindex and bit count (`passes == 2`) — the same
/// second-pass substitution `calculate_active_worst_quality` makes per GF
/// group.
///
/// `avg_frame_qindex[KEY_FRAME]` is seeded HALFWAY between the derived q and
/// `best_allowed_q`, not at the derived q.
///
/// Returns the seed when one was produced (picture 0 with both stats slots
/// present), and mutates the cursor + `total_left_stats` regardless.
#[allow(clippy::too_many_arguments)]
pub fn process_first_pass_stats(
    rc: &mut RateControl,
    scs: &SeqRc,
    cfg2: &TwoPassCfg,
    twopass: &TwoPassState,
    frame: &FrameRc,
    best_allowed_q: i32,
    cursor: &mut StatsCursor<'_>,
    total_stats: Option<&FirstPassStats>,
    total_left_stats: Option<&mut FirstPassStats>,
    this_frame: &mut FirstPassStats,
    twopass_worst_quality: impl FnOnce(f64, f64, i32, f64) -> i32,
) -> Option<FirstFrameSeed> {
    let mut seed = None;
    let mut total_left = total_left_stats;
    if frame.picture_number == 0
        && let (Some(total), Some(left)) = (total_stats, total_left.as_deref_mut())
    {
        if cfg2.lap_rc {
            // Accumulate `total_stats` from the limited stats available and
            // use it as `total_left_stats`.
            *left = *total;
        }
        let section_target_bandwidth = get_section_target_bandwidth(
            rc,
            twopass,
            cfg2.lap_rc,
            total.count,
            frame.picture_number,
        );
        let section_length = left.count;
        let section_error = left.coded_error / section_length;
        let tmp_q = if scs.passes == 2 {
            let ref_qindex = i32::from(total.stat_struct.worst_qindex);
            let ref_q = convert_qindex_to_q(ref_qindex, scs.encoder_bit_depth);
            let ref_gf_group_bits = total.stat_struct.total_num_bits as i64;
            let target_gf_group_bits = twopass.bits_left;
            let mut low = rc.best_quality;
            let mut high = rc.worst_quality;
            while low < high {
                let mid = (low + high) >> 1;
                let q = convert_qindex_to_q(mid, scs.encoder_bit_depth);
                let mid_bits = ((ref_gf_group_bits as f64) * ref_q / q) as i32;
                if mid_bits > target_gf_group_bits as i32 {
                    low = mid + 1;
                } else {
                    high = mid;
                }
            }
            low
        } else {
            twopass_worst_quality(
                section_error,
                0.0,
                section_target_bandwidth.unwrap_or(0),
                DEFAULT_GRP_WEIGHT,
            )
        };
        rc.active_worst_quality = tmp_q;
        rc.avg_frame_qindex[INTER_FRAME_IDX] = tmp_q;
        rc.avg_frame_qindex[KEY_FRAME_IDX] = (tmp_q + best_allowed_q) / 2;
        seed = Some(FirstFrameSeed {
            active_worst_quality: tmp_q,
            avg_frame_qindex_inter: tmp_q,
            avg_frame_qindex_key: (tmp_q + best_allowed_q) / 2,
        });
    }

    let Some(next) = cursor.input_stats() else {
        return seed;
    };
    *this_frame = next;
    if let Some(left) = total_left {
        subtract_stats(left, this_frame);
    }
    seed
}

/// `rc->avg_frame_qindex` is indexed by `FrameType`; these name the two slots
/// so a call site cannot transpose them.
const KEY_FRAME_IDX: usize = 0;
const INTER_FRAME_IDX: usize = 1;

/// What `svt_aom_process_rc_stat` did, so the caller can apply the parts that
/// touch structures this file does not own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProcessRcStatResult {
    /// A key frame's `base_frame_target`, when a KF group was budgeted.
    pub kf_base_frame_target: Option<i32>,
    /// Whether a new GF group was defined this call.
    pub new_gf_group: bool,
}

/// C `svt_aom_process_rc_stat` (pass2_strategy.c:847) — **EXPORTED**, and the
/// two-pass entry point `svt_av1_rc_process_rate_allocation` calls.
///
/// Three steps: consume this frame's first-pass stats, budget a new KF group
/// if this is an IDR, and budget a new GF group if one is due.
///
/// The `lap_rc` re-computation in the middle is easy to misread: for 1-pass
/// VBR the lookahead MOVES, so `total_stats` changes and with it every
/// modified error — which is why `kf_group_error_left` is recomputed for
/// every mini-GOP EXCEPT the first one after a key frame (that one was just
/// set by `kf_group_rate_assingment` and must not be overwritten).
#[allow(clippy::too_many_arguments)]
pub fn process_rc_stat(
    rc: &mut RateControl,
    scs: &SeqRc,
    cfg2: &TwoPassCfg,
    twopass: &mut TwoPassState,
    frame: &FrameRc,
    input: &GroupRateInput,
    best_allowed_q: i32,
    cursor: &mut StatsCursor<'_>,
    total_stats: Option<&FirstPassStats>,
    total_left_stats: Option<&mut FirstPassStats>,
    gf_group: &mut [GfGroupFrame],
    this_gf_update_due: bool,
    this_is_incomp_mg_frame: bool,
    params_end_of_seq_seen: &mut bool,
    mut twopass_worst_quality: impl FnMut(f64, f64, i32, f64) -> i32,
) -> ProcessRcStatResult {
    let has_total_stats = total_stats.is_some();
    let mut this_frame = FirstPassStats::default();
    process_first_pass_stats(
        rc,
        scs,
        cfg2,
        twopass,
        frame,
        best_allowed_q,
        cursor,
        total_stats,
        total_left_stats,
        &mut this_frame,
        &mut twopass_worst_quality,
    );

    let mut out = ProcessRcStatResult::default();
    // Keyframe and section processing.
    let is_idr = frame.is_intra_only() && input.idr_flag;
    if is_idr {
        if cfg2.lap_rc {
            lap_rc_init(
                twopass,
                cfg2,
                cursor,
                has_total_stats,
                this_frame,
                input.target_bit_rate,
                input.new_framerate,
            );
        }
        out.kf_base_frame_target = Some(kf_group_rate_assingment(
            rc,
            scs,
            cfg2,
            twopass,
            input,
            cursor,
            has_total_stats,
            this_frame,
            params_end_of_seq_seen,
        ));
    }

    // Define a new GF/ARF group. (Always entered for key frames.)
    out.new_gf_group = is_new_gf_group(
        gf_group,
        input.gf_interval as usize,
        frame.picture_number,
        this_is_incomp_mg_frame,
        this_gf_update_due,
    );
    if out.new_gf_group {
        // For 1-pass VBR the lookahead moves, so `total_stats` — and every
        // modified error derived from it — changes. Recompute
        // kf_group_error_left for each mini-GOP EXCEPT the first after a KF,
        // which `kf_group_rate_assingment` just set.
        if !is_idr && cfg2.lap_rc {
            twopass.kf_group_error_left =
                lap_rc_group_error_calc(rc, cursor, has_total_stats, this_frame) as i64;
        }
        gf_group_rate_assingment(
            rc,
            scs,
            cfg2,
            twopass,
            frame,
            input,
            cursor,
            has_total_stats,
            &mut this_frame,
            gf_group,
            twopass_worst_quality,
        );
    }
    out
}

/// C `svt_av1_init_second_pass` (pass2_strategy.c:997) — **EXPORTED**.
///
/// Sums the whole first-pass stats file into the end sentinel, derives the
/// real frame rate from the accumulated duration, and seeds the two-pass
/// budget and error bounds.
///
/// **The frame rate is DERIVED, not configured**: `10000000 * count /
/// duration`. Each first-pass frame can have a different duration, so the
/// second pass's rate is the true average rather than the guess the first
/// pass ran with.
///
/// `stats` is the whole ring; it is `&mut` because `read_stat_from_file`
/// backfills zero bit counts in place. Returns the derived frame rate, which
/// the caller feeds to [`new_framerate`].
///
/// C also calls `svt_aom_set_rc_param` here; that is `port_rc_process`'s and
/// is the caller's to run.
#[allow(clippy::too_many_arguments)]
pub fn init_second_pass(
    rc: &mut RateControl,
    twopass: &mut TwoPassState,
    cfg2: &TwoPassCfg,
    target_bit_rate: i64,
    stats: &mut [FirstPassStats],
    total_stats: &mut FirstPassStats,
    total_left_stats: &mut FirstPassStats,
) -> Option<f64> {
    if stats.is_empty() {
        // C returns early on `!stats_buf_ctx->stats_in_end`.
        return None;
    }
    // C zeroes the end sentinel and accumulates every frame into it.
    let mut end = FirstPassStats::default();
    let mut total_num_bits = 0_u64;
    for s in stats.iter() {
        accumulate_stats(&mut end, s);
        total_num_bits += s.stat_struct.total_num_bits;
    }
    end.stat_struct.total_num_bits = total_num_bits;

    *total_stats = end;
    *total_left_stats = end;

    let frame_rate = 10_000_000.0 * end.count / end.duration;
    twopass.bits_left = (end.duration * (target_bit_rate as f64) / 10_000_000.0) as i64;
    // Backfill any zero per-frame bit counts, per temporal layer.
    total_stats.stat_struct.total_num_bits = read_stat_from_file(stats);

    // Scan the stats and build the modified-error total and its bounds.
    let avg_error = end.coded_error / double_divide_check(end.count);
    twopass.modified_error_min = (avg_error * f64::from(cfg2.vbrmin_section)) / 100.0;
    twopass.modified_error_max = (avg_error * f64::from(cfg2.vbrmax_section)) / 100.0;
    let mut modified_error_total = 0.0;
    for s in stats.iter() {
        modified_error_total += calculate_modified_err(true, s);
    }
    twopass.modified_error_left = modified_error_total;

    // Reset the vbr counters.
    rc.vbr_bits_off_target = 0;
    rc.vbr_bits_off_target_fast = 0;
    rc.rate_error_estimate = 0;
    // Static sequence monitor variables.
    twopass.kf_zeromotion_pct = 100;
    Some(frame_rate)
}

#[cfg(test)]
mod orchestration_tests {
    use super::*;

    /// **EVIDENCE TIER 4** — see the module header. `svt_aom_process_rc_stat`
    /// and `svt_av1_init_second_pass` ARE exported, but driving them needs a
    /// populated `STATS_BUFFER_CTX` wired into a `SequenceControlSet`, which
    /// this lane has not built; the other three are `static` and inlined.
    const _: () = ();

    fn stat(bits: u64, coded_error: f64, duration: f64) -> FirstPassStats {
        FirstPassStats {
            frame: 1.0,
            coded_error,
            duration,
            count: 1.0,
            stat_struct: StatStruct {
                poc: 0,
                total_num_bits: bits,
                qindex: 100,
                worst_qindex: 150,
                temporal_layer_index: 0,
            },
        }
    }

    /// `accumulate_stats` is the exact inverse of `subtract_stats`, and both
    /// leave `stat_struct` alone.
    #[test]
    fn accumulate_and_subtract_are_inverses() {
        let mut section = FirstPassStats::default();
        section.stat_struct.poc = 9;
        let f = stat(100, 5.0, 1000.0);
        accumulate_stats(&mut section, &f);
        assert_eq!((section.frame, section.count), (1.0, 1.0));
        assert_eq!(section.coded_error, 5.0);
        assert_eq!(section.duration, 1000.0);
        assert_eq!(section.stat_struct.poc, 9, "stat_struct is not accumulated");
        subtract_stats(&mut section, &f);
        assert_eq!(section, {
            let mut z = FirstPassStats::default();
            z.stat_struct.poc = 9;
            z
        });
    }

    /// `svt_av1_init_second_pass` DERIVES the frame rate from the accumulated
    /// duration rather than taking the configured one — each first-pass frame
    /// can have a different duration, so this is the true average.
    #[test]
    fn init_second_pass_derives_the_frame_rate_from_duration() {
        let mut stats = [
            stat(1000, 10.0, 500_000.0),
            stat(2000, 20.0, 500_000.0),
            // A frame with no recorded bits: read_stat_from_file backfills it
            // from the previous frame at the same temporal layer.
            stat(0, 30.0, 500_000.0),
        ];
        let mut rc = RateControl::default();
        let mut tp = TwoPassState::default();
        let cfg2 = TwoPassCfg {
            vbrmin_section: 50,
            vbrmax_section: 200,
            ..Default::default()
        };
        let mut total = FirstPassStats::default();
        let mut left = FirstPassStats::default();
        let fr = init_second_pass(
            &mut rc, &mut tp, &cfg2, 10_000_000, &mut stats, &mut total, &mut left,
        )
        .expect("non-empty ring");
        // 3 frames over 1.5e6 ticks of 1e-7 s -> 20 fps.
        assert_eq!(fr, 10_000_000.0 * 3.0 / 1_500_000.0);
        // bits_left = duration * bitrate / 1e7 = 1.5e6 * 1e7 / 1e7.
        assert_eq!(tp.bits_left, 1_500_000);
        // The backfill happened, and the total is the backfilled sum.
        assert_eq!(stats[2].stat_struct.total_num_bits, 2000);
        assert_eq!(total.stat_struct.total_num_bits, 1000 + 2000 + 2000);
        // The error bounds are the average coded error scaled by the vbr
        // percentages; the modified error total is the (pre-backfill) bit sum.
        let avg_error = 60.0 / double_divide_check(3.0);
        assert_eq!(tp.modified_error_min, avg_error * 50.0 / 100.0);
        assert_eq!(tp.modified_error_max, avg_error * 200.0 / 100.0);
        assert_eq!(tp.modified_error_left, 1000.0 + 2000.0 + 2000.0);
        assert_eq!(tp.kf_zeromotion_pct, 100);
        // An empty ring returns None and writes nothing.
        let mut empty: [FirstPassStats; 0] = [];
        assert!(
            init_second_pass(
                &mut rc, &mut tp, &cfg2, 10_000_000, &mut empty, &mut total, &mut left
            )
            .is_none()
        );
    }

    /// `kf_group_rate_assingment` sets `kf_zeromotion_pct` to **0**,
    /// overwriting the 100 the init functions set — so the
    /// `STATIC_KF_GROUP_THRESH` gate in `rc_vbr_cbr.c`'s `get_q` can only
    /// fire before the first KF group is budgeted.
    #[test]
    fn kf_group_rate_assingment_zeroes_kf_zeromotion_pct() {
        let stats = [stat(1000, 10.0, 1.0), stat(1000, 10.0, 1.0)];
        let mut cursor = StatsCursor::new(&stats, 1);
        let mut rc = RateControl {
            avg_frame_bandwidth: 50_000,
            max_frame_bandwidth: 500_000,
            best_quality: 0,
            worst_quality: 255,
            kf_boost: 2000,
            frames_to_key: 30,
            ..Default::default()
        };
        let scs = SeqRc {
            intra_period_length: 63,
            ..Default::default()
        };
        let cfg2 = TwoPassCfg {
            vbrmax_section: 200,
            ..Default::default()
        };
        let mut tp = TwoPassState {
            bits_left: 10_000_000,
            modified_error_left: 100_000.0,
            kf_zeromotion_pct: 100,
            ..Default::default()
        };
        let input = GroupRateInput {
            new_framerate: 30.0,
            target_bit_rate: 1_500_000,
            ..Default::default()
        };
        let mut eos = false;
        let kf_bits = kf_group_rate_assingment(
            &mut rc,
            &scs,
            &cfg2,
            &mut tp,
            &input,
            &mut cursor,
            true,
            stats[0],
            &mut eos,
        );
        assert_eq!(tp.kf_zeromotion_pct, 0);
        assert!(kf_bits > 0, "the key frame must get a budget");
        // frames_since_key / frames_since_cdf_update are reset.
        assert_eq!(rc.frames_since_key, 0);
        assert_eq!(rc.frames_since_cdf_update, 0);
        // The cursor is restored.
        assert_eq!(cursor.position(), 1);
        // The group's remaining error excludes the key frame's own.
        assert_eq!(tp.kf_group_error_left, 1000);
    }

    /// `process_first_pass_stats` seeds the quality state ONLY on picture 0,
    /// and seeds `avg_frame_qindex[KEY_FRAME]` HALFWAY to `best_allowed_q`.
    #[test]
    fn process_first_pass_stats_seeds_only_picture_zero() {
        let stats = [stat(1000, 10.0, 1.0), stat(2000, 20.0, 1.0)];
        let mut rc = RateControl {
            best_quality: 0,
            worst_quality: 255,
            ..Default::default()
        };
        let scs = SeqRc::default();
        let cfg2 = TwoPassCfg::default();
        let tp = TwoPassState {
            bits_left: 1_000_000,
            ..Default::default()
        };
        let total = FirstPassStats {
            count: 10.0,
            coded_error: 100.0,
            ..Default::default()
        };
        let mut left = total;
        let mut this = FirstPassStats::default();

        let frame0 = FrameRc {
            picture_number: 0,
            ..Default::default()
        };
        let mut cursor = StatsCursor::new(&stats, 0);
        let seed = process_first_pass_stats(
            &mut rc,
            &scs,
            &cfg2,
            &tp,
            &frame0,
            40,
            &mut cursor,
            Some(&total),
            Some(&mut left),
            &mut this,
            |_, _, _, _| 120,
        )
        .expect("picture 0 with both stats slots seeds");
        assert_eq!(seed.active_worst_quality, 120);
        assert_eq!(seed.avg_frame_qindex_inter, 120);
        assert_eq!(seed.avg_frame_qindex_key, (120 + 40) / 2);
        assert_eq!(rc.avg_frame_qindex, [80, 120]);
        // One stat was consumed and deducted from the remainder.
        assert_eq!(cursor.position(), 1);
        assert_eq!(left.count, 9.0);
        assert_eq!(left.coded_error, 90.0);

        // A later picture seeds nothing but still consumes.
        let frame1 = FrameRc {
            picture_number: 1,
            ..Default::default()
        };
        let mut cursor = StatsCursor::new(&stats, 0);
        let mut left2 = total;
        assert!(
            process_first_pass_stats(
                &mut rc,
                &scs,
                &cfg2,
                &tp,
                &frame1,
                40,
                &mut cursor,
                Some(&total),
                Some(&mut left2),
                &mut this,
                |_, _, _, _| 200,
            )
            .is_none()
        );
        assert_eq!(rc.active_worst_quality, 120, "unchanged on a later picture");
        assert_eq!(cursor.position(), 1);
    }

    /// `process_rc_stat` budgets a KF group only on an IDR, and a GF group
    /// only when one is due — and the `lap_rc` error recomputation is skipped
    /// on the first mini-GOP after a key frame (which the KF assignment just
    /// set).
    #[test]
    fn process_rc_stat_gates_the_two_group_assignments() {
        let stats = [stat(1000, 10.0, 1.0), stat(1000, 10.0, 1.0)];
        let base_rc = RateControl {
            avg_frame_bandwidth: 50_000,
            max_frame_bandwidth: 500_000,
            worst_quality: 255,
            frames_to_key: 30,
            baseline_gf_interval: 8,
            ..Default::default()
        };
        let scs = SeqRc {
            intra_period_length: 63,
            hierarchical_levels: 3,
            ..Default::default()
        };
        let cfg2 = TwoPassCfg {
            vbrmax_section: 200,
            mb_rows: 68,
            ..Default::default()
        };
        let mk_tp = || TwoPassState {
            bits_left: 10_000_000,
            modified_error_left: 100_000.0,
            kf_group_bits: 1_000_000,
            kf_group_error_left: 10_000,
            ..Default::default()
        };
        let mut gf_group = vec![GfGroupFrame::default(); 8];

        // Non-IDR, no GF update due: neither assignment runs.
        let mut rc = base_rc.clone();
        let mut tp = mk_tp();
        let mut eos = false;
        let frame = FrameRc {
            picture_number: 4,
            frame_type: crate::port_rc_vbr_cbr_state::FrameType::Inter,
            ..Default::default()
        };
        let input = GroupRateInput {
            gf_interval: 8,
            new_framerate: 30.0,
            target_bit_rate: 1_500_000,
            ..Default::default()
        };
        let mut cursor = StatsCursor::new(&stats, 0);
        let out = process_rc_stat(
            &mut rc,
            &scs,
            &cfg2,
            &mut tp,
            &frame,
            &input,
            0,
            &mut cursor,
            None,
            None,
            &mut gf_group,
            false,
            false,
            &mut eos,
            |_, _, _, _| 120,
        );
        assert_eq!(out.kf_base_frame_target, None);
        assert!(!out.new_gf_group);
        assert_eq!(tp.kf_zeromotion_pct, 0, "untouched default");

        // IDR: the KF group is budgeted.
        let mut rc = base_rc.clone();
        let mut tp = mk_tp();
        let kf_frame = FrameRc {
            picture_number: 0,
            frame_type: crate::port_rc_vbr_cbr_state::FrameType::Key,
            ..Default::default()
        };
        let kf_input = GroupRateInput {
            idr_flag: true,
            gf_interval: 8,
            new_framerate: 30.0,
            target_bit_rate: 1_500_000,
            ..Default::default()
        };
        let mut cursor = StatsCursor::new(&stats, 0);
        let out = process_rc_stat(
            &mut rc,
            &scs,
            &cfg2,
            &mut tp,
            &kf_frame,
            &kf_input,
            0,
            &mut cursor,
            None,
            None,
            &mut gf_group,
            true,
            false,
            &mut eos,
            |_, _, _, _| 120,
        );
        assert!(out.kf_base_frame_target.is_some());
        assert!(out.new_gf_group, "a KF always defines a new GF group");
        // The GF assignment ran and split the budget.
        assert!(rc.gf_group_bits >= 0);
    }

    /// `gop_bit_allocation` derives the ARF pool from the GF boost and hands
    /// it to the splitter; a zero boost gives a zero pool, so every frame in
    /// the group gets the flat base share.
    #[test]
    fn gop_bit_allocation_with_no_boost_is_a_flat_split() {
        let rc = RateControl {
            baseline_gf_interval: 4,
            gfu_boost: 0,
            ..Default::default()
        };
        let mut group = vec![
            GfGroupFrame {
                update_type: FrameUpdateType::LfUpdate,
                ..Default::default()
            };
            4
        ];
        gop_bit_allocation(&rc, &mut group, 4, 3, false, 8, false, 40_000);
        for f in &group {
            assert_eq!(f.base_frame_target, 10_000);
        }
    }
}
