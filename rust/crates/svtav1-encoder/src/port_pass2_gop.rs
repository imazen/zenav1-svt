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
/// cross-ISA gates exist to prevent. Full write-up, including the CI cell it
/// currently reddens, in `docs/SUSPECTED-C-BUGS.md` #25.
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
