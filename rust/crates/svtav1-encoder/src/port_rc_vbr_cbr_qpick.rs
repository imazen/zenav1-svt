//! Port of `Codec/rc_vbr_cbr.c`'s **qindex decision** — the chain that turns a
//! frame target plus the reference DPB into `frm_hdr.quantization_params
//! .base_q_idx` for a VBR or CBR frame.
//!
//! [`crate::port_rc_vbr_cbr_state`] ports the machinery underneath this
//! (the rate model, the buffer, the correction-factor loop, the regulator);
//! this file is the decision layer on top: the CBR and VBR
//! `rc_pick_q_and_bounds` variants, the active-best/worst derivations they
//! call, the reference-qindex limiting, and the cyclic-refresh setup.
//!
//! **Why this is the inter unblocker for the lane.** `svt_av1_rc_calc_qindex_
//! rate_control` is the ONLY writer of `base_q_idx` in VBR/CBR mode. Without
//! it a multi-frame encode has no per-frame quantizer at all, so every frame
//! after the first is coded at the wrong q and nothing downstream can be
//! byte-compared. Everything else in the rate-control group feeds this.
//!
//! **EVIDENCE — read this before trusting a green run.** Every function here
//! is `static` in C **and was inlined away by the Release build**: `nm` on
//! `cbuild-static/.../rc_vbr_cbr.c.o` lists eight local symbols and not one of
//! them is in this file's set. `calc_active_worst_quality_no_stats_cbr` is the
//! nearest miss — it HAS a symbol, but LLVM specialized its ABI, so it is not
//! callable either (see `link_globalized_rc_vbr_statics` in
//! `svtav1-cref/build.rs`). So:
//!
//! * The three entry points C EXPORTS —
//!   `svt_av1_rc_calc_qindex_rate_control`,
//!   `svt_av1_rc_process_rate_allocation` and
//!   `svt_av1_rc_postencode_update` — are the only tier-1 route to this code,
//!   and each needs a fully populated `PictureControlSet` with a live DPB to
//!   drive. That harness is NOT built yet; it is the next chunk of this lane.
//! * Until it is, **everything in this file is evidence TIER 4**
//!   (`docs/WORKING-ON-THIS.md` §4): hand-derived vectors traced against the C
//!   source. The tests say so individually. The leaves it calls
//!   ([`crate::port_rc_vbr_cbr_state::regulate_q`],
//!   [`crate::port_rc_process::compute_qdelta_by_rate`],
//!   [`crate::port_rc_vbr_cbr::get_gf_active_quality_tpl_la`]) ARE tier 1, so
//!   what is unpinned here is the control flow and the constants, not the
//!   arithmetic underneath.
//!
//! Stating that plainly is the point: a transcribed oracle agreeing with
//! transcribed code proves only that both were transcribed the same way.
//!
//! **Preprocessor check.** `grep -c 'SVT_HDR_MODE' rc_vbr_cbr.c` is 0, so no
//! function here has a second fork definition. One macro it uses DOES have
//! two arms — `SVT_QP_SCALE_WEIGHT` (definitions.h:249/252) — and the
//! MAINLINE arm is the table lookup, not the fork's linear formula; see
//! [`qp_scale_weight_mainline`].

use crate::port_rc_process::{
    self, FrameUpdateType, INTER_FRAME, KEY_FRAME, NON_BASE_QINDEX_WEIGHT_REF,
    NON_BASE_QINDEX_WEIGHT_WQ, R0_WEIGHT, RATE_FACTOR_DELTAS, RATE_FACTOR_LEVELS, RateFactorLevel,
    SliceType,
};
use crate::port_rc_vbr_cbr::{self as leaves, MinqFamily};
use crate::port_rc_vbr_cbr_state::{
    self as st, AomRcMode, CyclicRefresh, FrameRc, RateControl, RateControlCfg, SeqRc,
};
use crate::rate_control::{compute_qdelta, convert_qindex_to_q, q_index_from_qstep_ratio};

/// C `MAXQ` (definitions.h:1658).
pub const MAXQ: i32 = 255;
/// C `MAX_ARF_LAYERS` (rc_process.h:36).
pub const MAX_ARF_LAYERS: usize = 6;
/// C `STATIC_KF_GROUP_THRESH` (rc_process.h:33).
pub const STATIC_KF_GROUP_THRESH: i32 = 99;
/// C `MIN_BOOST_COMBINE_FACTOR` (rc_vbr_cbr.c:598).
pub const MIN_BOOST_COMBINE_FACTOR: f64 = 4.0;
/// C `MAX_GFUBOOST_FACTOR` (rc_process.h:54).
pub const MAX_GFUBOOST_FACTOR: f64 = 10.0;
/// C `QFACTOR` (rc_vbr_cbr.c:1000) — the base-layer qdelta ratio applied when
/// the intra period is short.
pub const QFACTOR: f64 = 1.1;
/// C `CR_MAX_RATE_TARGET_RATIO` (rc_vbr_cbr.c:987).
pub const CR_MAX_RATE_TARGET_RATIO: f64 = 4.0;
/// C `VBR_PCT_ADJUSTMENT_LIMIT` (rc_vbr_cbr.c:647).
pub const VBR_PCT_ADJUSTMENT_LIMIT: i64 = 50;

/// C `svt_av1_qp_scale_compress_weight` (rc_process.c:48).
pub const QP_SCALE_COMPRESS_WEIGHT: [f64; 4] = [1.0, 1.125, 1.25, 1.375];

/// C `SVT_QP_SCALE_WEIGHT` — **the MAINLINE arm** (definitions.h:252), which
/// indexes [`QP_SCALE_COMPRESS_WEIGHT`] with a `uint8_t`
/// `qp_scale_compress_strength_unused`. The `SVT_HDR_MODE` arm at :249 is a
/// linear `1.0 + strength * 0.125` over a `double` field and is NOT what a
/// mainline build compiles.
///
/// The two agree on the CLI's 0..=3 domain, which is exactly why citing the
/// wrong one is easy and harmless right up until someone widens the range.
/// C indexes without a bounds check; the port clamps, because reading past a
/// 4-entry table is not behaviour worth reproducing.
#[must_use]
pub fn qp_scale_weight_mainline(strength: u8) -> f64 {
    QP_SCALE_COMPRESS_WEIGHT[(strength as usize).min(3)]
}

/// C `SVT_QP_SCALE_ON` — mainline arm: a non-zero `uint8_t` strength.
#[must_use]
pub fn qp_scale_on_mainline(strength: u8) -> bool {
    strength != 0
}

// ---------------------------------------------------------------------------
// The reference-picture view
// ---------------------------------------------------------------------------

/// One DPB slot as `rc_vbr_cbr.c` sees it.
///
/// C reaches the same facts through two different paths and the port keeps
/// both fields rather than assuming they agree: `slice_type` / `ref_poc` /
/// `tmp_layer_idx` / `r0` come off the `EbReferenceObject`
/// (`pcs->ref_pic_ptr_array[list][i]->object_ptr`), while `base_q_idx` and
/// `pcs_slice_type` are the PCS's own mirrors
/// (`pcs->ref_base_q_idx[list][i]`, `pcs->ref_slice_type[list][i]`).
/// `calc_active_best_quality_no_stats_cbr` reads the OBJECT's slice type and
/// `find_min_ref_base_q_idx` reads the PCS's, in the same file, on the same
/// frame — collapsing them into one field would be a guess.
#[derive(Clone, Copy, Debug)]
pub struct RefPicRc {
    /// `ref_obj->tmp_layer_idx`.
    pub tmp_layer_idx: u8,
    /// `ref_obj->slice_type`.
    pub slice_type: SliceType,
    /// `pcs->ref_slice_type[list][i]`.
    pub pcs_slice_type: SliceType,
    /// `ref_obj->ref_poc`.
    pub ref_poc: u64,
    /// `pcs->ref_base_q_idx[list][i]`.
    pub base_q_idx: u8,
    /// `pcs->ref_pic_r0[list][i]`.
    pub pcs_r0: f64,
    /// `ref_obj->r0`.
    pub obj_r0: f64,
}

/// Both reference lists plus the counts C actually loops to.
///
/// The counts are separate from the slice lengths on purpose: C indexes slot 0
/// of list 0 UNCONDITIONALLY in `calc_active_best_quality_no_stats_cbr` (it is
/// only called for non-intra frames, which always have one) but loops
/// `1..ref_list0_count_try` for the rest. A single truncated slice would
/// conflate "present in the DPB" with "eligible to be searched".
#[derive(Clone, Copy, Debug)]
pub struct RefLists<'a> {
    /// `pcs->ref_pic_ptr_array[REF_LIST_0]`.
    pub l0: &'a [RefPicRc],
    /// `pcs->ref_pic_ptr_array[REF_LIST_1]`.
    pub l1: &'a [RefPicRc],
    /// `ppcs->ref_list0_count_try`.
    pub l0_count_try: usize,
    /// `ppcs->ref_list1_count_try`.
    pub l1_count_try: usize,
}

impl RefLists<'_> {
    /// C `get_ref_obj(pcs, list, idx)` (rc_vbr_cbr.c:27). `None` where C would
    /// dereference an empty DPB slot — the port refuses rather than
    /// reproducing the null deref.
    #[must_use]
    fn get(&self, list: usize, idx: usize) -> Option<&RefPicRc> {
        match list {
            0 => self.l0.get(idx),
            _ => self.l1.get(idx),
        }
    }
}

/// C's `RefList` enum, so a call site cannot pass the wrong integer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum RefList {
    L0 = 0,
    L1 = 1,
}

// ---------------------------------------------------------------------------
// Two-pass and TPL state this file reads
// ---------------------------------------------------------------------------

/// The `TWO_PASS` fields `rc_vbr_cbr.c` reads (`scs->twopass`).
#[derive(Clone, Copy, Debug, Default)]
pub struct TwoPassRc {
    /// `twopass->extend_minq`.
    pub extend_minq: i32,
    /// `twopass->extend_maxq`.
    pub extend_maxq: i32,
    /// `twopass->extend_minq_fast`.
    pub extend_minq_fast: i32,
    /// `twopass->kf_zeromotion_pct`.
    pub kf_zeromotion_pct: i32,
    /// `twopass->stats_buf_ctx->total_stats->count`, or 0 when
    /// `total_stats` is null — C's own `!= NULL ? count : 0`.
    pub total_stats_count: i32,
}

/// The `TplControls` fields the boost derivation reads.
#[derive(Clone, Copy, Debug, Default)]
pub struct TplCtrlsRc {
    /// `ppcs->tpl_ctrls.enable`.
    pub enable: bool,
    /// `ppcs->tpl_ctrls.r0_adjust_factor`. Zero means "do not adjust", and is
    /// also the divisor guard — C only divides when it is non-zero.
    pub r0_adjust_factor: f64,
}

// ---------------------------------------------------------------------------
// Boost derivation and target-rate correction
// ---------------------------------------------------------------------------

/// C `process_tpl_stats_frame_kf_gfu_boost` (rc_vbr_cbr.c:611).
///
/// **`ppcs->r0` is MUTATED, and on a key frame it is divided TWICE** — once by
/// `r0_adjust_factor` and again by the islice GOP factor. `frame.r0` is
/// therefore `&mut` and the caller must not re-derive it afterwards.
///
/// The inter arm and the key arm are separate `if`s, not an if/else: a key
/// frame is `frame_is_intra_only`, so it skips the first block and runs the
/// second. An INTRA_ONLY frame runs NEITHER (it is intra-only but not
/// `KEY_FRAME`), which is easy to miss.
pub fn process_tpl_stats_frame_kf_gfu_boost(
    rc: &mut RateControl,
    scs: &SeqRc,
    frame: &mut FrameRc,
    tpl: &TplCtrlsRc,
) {
    let hl = usize::from(frame.hierarchical_levels);
    if !frame.is_intra_only() {
        if tpl.r0_adjust_factor != 0.0 {
            frame.r0 /= tpl.r0_adjust_factor;
            // Further scale r0 based on the GOP structure.
            frame.r0 /= port_rc_process::TPL_HL_BASE_FRAME_DIV_FACTOR[hl];
        }
        rc.gfu_boost = port_rc_process::get_gfu_boost_from_r0_lap(
            MIN_BOOST_COMBINE_FACTOR,
            MAX_GFUBOOST_FACTOR,
            frame.r0,
            rc.frames_to_key,
        );
    }

    if frame.frame_type.is_key() {
        if tpl.r0_adjust_factor != 0.0 {
            frame.r0 /= tpl.r0_adjust_factor;
        }
        // Scale r0 based on the GOP structure.
        frame.r0 /= port_rc_process::TPL_HL_ISLICE_DIV_FACTOR[hl];

        // when frames_to_key is not available, i.e. in 1-pass encoding
        rc.kf_boost = port_rc_process::get_cqp_kf_boost_from_r0(
            frame.r0,
            rc.frames_to_key,
            scs.input_resolution,
        );
        let max_boost = 10_000; // ppcs->used_tpl_frame_num * KB;
        rc.kf_boost = rc.kf_boost.min(max_boost);

        rc.gfu_boost = port_rc_process::get_gfu_boost_from_r0_lap(
            MIN_BOOST_COMBINE_FACTOR,
            MAX_GFUBOOST_FACTOR,
            frame.r0,
            rc.frames_to_key,
        );
    }
}

/// C `vbr_rate_correction` (rc_vbr_cbr.c:649).
///
/// Spends (or claws back) up to `VBR_PCT_ADJUSTMENT_LIMIT` percent of the
/// frame target to close the running VBR error, then hands out a slice of the
/// "fast" undershoot pool — but never to a KF/GF/ARF or an overlay, because
/// those already carry a boost.
///
/// `rc.vbr_bits_off_target_fast` is DECREMENTED by what it hands out, so this
/// is a state mutator despite reading like a pure adjustment.
pub fn vbr_rate_correction(
    rc: &mut RateControl,
    twopass: &TwoPassRc,
    frame: &FrameRc,
    this_frame_target: &mut i32,
) {
    let vbr_bits_off_target = rc.vbr_bits_off_target;
    let stats_count = twopass.total_stats_count;
    // C: `AOMMIN(16, stats_count - (int)pcs->picture_number)` — the picture
    // number is narrowed to `int` FIRST, so a stream past 2^31 frames wraps
    // here exactly as C does.
    let frame_window = 16.min(stats_count - (frame.picture_number as i32));
    const { assert!(VBR_PCT_ADJUSTMENT_LIMIT <= 100) };
    if frame_window > 0 {
        let max_delta = (vbr_bits_off_target / i64::from(frame_window))
            .abs()
            .min(i64::from(*this_frame_target) * VBR_PCT_ADJUSTMENT_LIMIT / 100)
            as i32;
        // vbr_bits_off_target > 0 means we have extra bits to spend;
        // < 0 means we are currently overshooting.
        *this_frame_target += if vbr_bits_off_target >= 0 {
            max_delta
        } else {
            -max_delta
        };
    }

    // Fast redistribution of bits arising from massive local undershoot.
    // Not for kf, arf, gf or overlay frames.
    if !frame.is_kf_gf_arf() && !frame.is_overlay && rc.vbr_bits_off_target_fast != 0 {
        let one_frame_bits = rc.avg_frame_bandwidth.max(*this_frame_target);
        let mut fast_extra_bits = rc.vbr_bits_off_target_fast.min(i64::from(one_frame_bits));
        fast_extra_bits =
            fast_extra_bits.min(i64::from(one_frame_bits / 8).max(rc.vbr_bits_off_target_fast / 8));
        *this_frame_target += fast_extra_bits as i32;
        rc.vbr_bits_off_target_fast -= fast_extra_bits;
    }
}

/// C `av1_set_target_rate` (rc_vbr_cbr.c:678).
pub fn set_target_rate(rc: &mut RateControl, twopass: &TwoPassRc, frame: &mut FrameRc) {
    let mut target_rate = frame.base_frame_target;
    vbr_rate_correction(rc, twopass, frame, &mut target_rate);
    frame.this_frame_target = target_rate;
}

// ---------------------------------------------------------------------------
// Active best quality
// ---------------------------------------------------------------------------

/// C `calc_active_best_quality_no_stats_cbr` (rc_vbr_cbr.c:807).
///
/// Two entirely different derivations behind one name:
///
/// * **Intra** — the kf minq curve at the running average KEY qindex, nudged
///   down by a quarter of a q-step for pictures at or below CIF. The very
///   first picture (`frame_offset == 0`) skips all of it and keeps
///   `rc->best_quality`.
/// * **Inter** — inherit from the reference DPB. It picks the "best" reference
///   by a three-way preference (lower temporal layer; same layer but
///   temporally closer; anything at all if the incumbent was an I_SLICE),
///   sets `rc->arf_q` to that reference's qindex minus 30, looks that up in
///   the RTC minq table, then averages toward `active_worst_quality` once per
///   temporal layer of separation.
///
/// `rc.arf_q` is an OUTPUT (`&mut RateControl`), not a scratch value.
///
/// Returns `None` when the inter arm has no list-0 slot 0 — C would
/// dereference a null `object_ptr` there.
#[must_use]
pub fn calc_active_best_quality_no_stats_cbr(
    rc: &mut RateControl,
    scs: &SeqRc,
    frame: &FrameRc,
    refs: &RefLists<'_>,
    active_worst_quality: i32,
    width: i32,
    height: i32,
) -> Option<i32> {
    let bit_depth = scs.encoder_bit_depth;
    let rtc_minq = leaves::assign_minq_table(bit_depth, MinqFamily::Rtc);

    if frame.is_intra_only() {
        let mut active_best_quality = rc.best_quality;
        if frame.frame_offset > 0 {
            // Not the first frame of a one-pass encode, and kf_boost is set.
            let mut q_adj_factor = 1.0_f64;
            active_best_quality = leaves::get_kf_active_quality_tpl(
                rc.kf_boost,
                rc.avg_frame_qindex[KEY_FRAME as usize] as usize,
                bit_depth,
            );
            // Allow somewhat lower kf minq with small image formats.
            if width * height <= 352 * 288 {
                q_adj_factor -= 0.25;
            }
            let q_val = convert_qindex_to_q(active_best_quality, bit_depth);
            active_best_quality += compute_qdelta(q_val, q_val * q_adj_factor, bit_depth);
        }
        return Some(active_best_quality);
    }

    // Inherit qp from the reference qps.
    let first = refs.get(RefList::L0 as usize, 0)?;
    let mut ref_base_q_idx = first.base_q_idx;
    let mut max_tmp_layer = first.tmp_layer_idx;
    // C: `int dist = abs((int)pcs->picture_number - (int)ref_obj_l0->ref_poc);`
    // BOTH operands are narrowed to `int` BEFORE the subtraction, so the
    // difference is computed in 32 bits. Reproduced with `as i32` +
    // `wrapping_sub`; `unsigned_abs` then also defines the `abs(INT_MIN)` case
    // that is UB in C.
    let mut dist = (frame.picture_number as i32)
        .wrapping_sub(first.ref_poc as i32)
        .unsigned_abs();
    let mut best_is_islice = first.slice_type == SliceType::I;

    // C runs the same three-way preference over the rest of list 0 and then
    // over all of list 1; written once here so the two cannot drift.
    let mut consider = |r: &RefPicRc| {
        if r.slice_type == SliceType::I {
            return;
        }
        let d = (frame.picture_number as i32)
            .wrapping_sub(r.ref_poc as i32)
            .unsigned_abs();
        if r.tmp_layer_idx < max_tmp_layer
            || (r.tmp_layer_idx == max_tmp_layer && d < dist)
            || best_is_islice
        {
            ref_base_q_idx = r.base_q_idx;
            max_tmp_layer = r.tmp_layer_idx;
            dist = d;
            best_is_islice = false;
        }
    };
    for i in 1..refs.l0_count_try {
        if let Some(r) = refs.get(RefList::L0 as usize, i) {
            consider(r);
        }
    }
    for i in 0..refs.l1_count_try {
        if let Some(r) = refs.get(RefList::L1 as usize, i) {
            consider(r);
        }
    }

    let ref_tmp_layer = max_tmp_layer;
    rc.arf_q = 0.max(i32::from(ref_base_q_idx) - 30);
    let mut active_best_quality = rtc_minq[rc.arf_q as usize];
    let q = active_worst_quality;
    // C: `int8_t tmp_layer_delta = (int8_t)temporal_layer_index - (int8_t)ref_tmp_layer;`
    // The subtraction is `int` and the result narrows to `int8_t`; both
    // operands are 0..=5 so the narrowing cannot bite, but the loop is
    // `while (tmp_layer_delta > 0)` and a NEGATIVE delta (a reference from a
    // HIGHER temporal layer) correctly runs zero times.
    let mut tmp_layer_delta = i32::from(frame.temporal_layer_index) - i32::from(ref_tmp_layer);
    while tmp_layer_delta > 0 {
        active_best_quality = (active_best_quality + q + 1) / 2;
        tmp_layer_delta -= 1;
    }
    Some(active_best_quality)
}

/// C `get_active_best_quality` (rc_vbr_cbr.c:1081) — the VBR (two-pass) arm.
///
/// A leaf or overlay frame just reads the inter minq curve at
/// `active_worst_quality`. A GF/ARF frame instead interpolates the arfgf
/// curves at the lower of `active_worst_quality` and the running inter
/// average, then walks the boost back toward `min_boost` by `arf_boost_factor`
/// — 1.3 (i.e. a SMALLER boost) when the nearest reference was an I_SLICE and
/// its r0 is at least 0.08 above this frame's. An internal ARF then averages
/// toward `active_worst_quality` once per pyramid level.
#[must_use]
pub fn get_active_best_quality(
    rc: &RateControl,
    scs: &SeqRc,
    frame: &FrameRc,
    refs: &RefLists<'_>,
    active_worst_quality: i32,
) -> i32 {
    let bit_depth = scs.encoder_bit_depth;
    let is_intrl_arf_boost = frame.is_internal_arf();
    let inter_minq = leaves::assign_minq_table(bit_depth, MinqFamily::Inter);
    let is_leaf_frame = !(frame.is_gf_or_arf() || is_intrl_arf_boost);

    if is_leaf_frame || frame.is_overlay {
        return inter_minq[active_worst_quality as usize];
    }

    // Use the lower of active_worst_quality and the recent average Q as the
    // basis for the GF/ARF best-Q limit, unless the last frame was a key frame.
    let mut q = active_worst_quality;
    if rc.frames_since_key > 1 && rc.avg_frame_qindex[INTER_FRAME as usize] < active_worst_quality {
        q = rc.avg_frame_qindex[INTER_FRAME as usize];
    }
    let mut active_best_quality =
        leaves::get_gf_active_quality_tpl_la(rc.gfu_boost, q as usize, bit_depth);
    let min_boost = leaves::get_gf_high_motion_quality(q as usize, bit_depth);
    let boost = min_boost - active_best_quality;

    let l0_first = refs.get(RefList::L0 as usize, 0);
    let arf_boost_factor = match l0_first {
        Some(r) if r.pcs_slice_type == SliceType::I && r.pcs_r0 - frame.r0 >= 0.08 => 1.3,
        _ => 1.0,
    };
    // C's `(int)(boost * arf_boost_factor)` truncates toward zero; `boost` can
    // be negative (the high-motion curve is not uniformly above the low-motion
    // one), and truncation toward zero is NOT floor for a negative product.
    active_best_quality = min_boost - ((f64::from(boost) * arf_boost_factor) as i32);
    if !is_intrl_arf_boost {
        return active_best_quality;
    }

    let mut this_height = frame.layer_depth;
    while this_height > 1 {
        active_best_quality = (active_best_quality + active_worst_quality + 1) / 2;
        this_height -= 1;
    }
    active_best_quality
}

/// C `av1_frame_type_qdelta_org` (rc_vbr_cbr.c:1066).
///
/// Distinct from [`crate::port_rc_process::frame_type_qdelta`] (which is
/// `rc_crf_cqp.c`'s copy) in one place that matters: this one subtracts
/// `(layer_depth - 2) * 0.1` from the `GF_ARF_LOW` rate factor using the
/// frame's REAL `layer_depth`, where the CQP copy has the same line with the
/// variable already folded to a literal `0`. Same shape, different value.
#[must_use]
pub fn frame_type_qdelta_org(rc: &RateControl, frame: &FrameRc, q: i32, bit_depth: u8) -> i32 {
    let rf_lvl = RATE_FACTOR_LEVELS[frame.update_type as usize];
    let frame_type = if rf_lvl == RateFactorLevel::KfStd {
        KEY_FRAME
    } else {
        INTER_FRAME
    };
    let mut rate_factor = RATE_FACTOR_DELTAS[rf_lvl as usize];
    if rf_lvl == RateFactorLevel::GfArfLow {
        rate_factor -= f64::from(frame.layer_depth - 2) * 0.1;
        rate_factor = rate_factor.max(1.0);
    }
    port_rc_process::compute_qdelta_by_rate(
        rc.best_quality,
        rc.worst_quality,
        frame_type,
        q,
        rate_factor,
        bit_depth,
        frame.sc_class1,
    )
}

/// The `(active_worst, active_best)` pair that C passes around as two
/// `int*` out-params.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActiveQuality {
    pub best: i32,
    pub worst: i32,
}

/// C `adjust_active_best_and_worst_quality_org` (rc_vbr_cbr.c:1127).
///
/// Widens the `[best, worst]` window by the two-pass minq/maxq extensions —
/// by the FULL amount for a frame the two-pass logic considers important
/// (intra, or a reference under `gop_constraint_rc`, or a low temporal layer
/// on a short clip, or any reference on a long one) and by HALF for the rest —
/// then applies the frame-type qdelta to the worst end and re-clamps.
///
/// `transition_present == 1` skips the extension entirely; note the field is
/// `int8_t` with `-1` meaning "not computed", so `!= 1` is true for BOTH the
/// unset and the not-a-transition cases.
#[must_use]
pub fn adjust_active_best_and_worst_quality_org(
    rc: &RateControl,
    scs: &SeqRc,
    twopass: &TwoPassRc,
    frame: &FrameRc,
    active: ActiveQuality,
) -> ActiveQuality {
    let bit_depth = scs.encoder_bit_depth;
    let mut active_best_quality = active.best;
    let mut active_worst_quality = active.worst;

    if frame.transition_present != 1 {
        if frame.is_intra_only()
            || (scs.gop_constraint_rc && frame.is_ref)
            || (frame.temporal_layer_index < 2 && scs.is_short_clip)
            || (frame.is_ref && !scs.is_short_clip)
        {
            active_best_quality -= twopass.extend_minq + twopass.extend_minq_fast;
            active_worst_quality += twopass.extend_maxq / 2;
        } else {
            active_best_quality -= (twopass.extend_minq + twopass.extend_minq_fast) / 2;
            active_worst_quality += twopass.extend_maxq;
        }
    }
    // Static forced key frames' Q restrictions are dealt with elsewhere.
    let qdelta = frame_type_qdelta_org(rc, frame, active_worst_quality, bit_depth);

    active_worst_quality = (active_worst_quality + qdelta).max(active_best_quality);
    active_best_quality = active_best_quality.clamp(rc.best_quality, rc.worst_quality);
    active_worst_quality = active_worst_quality.clamp(active_best_quality, rc.worst_quality);

    ActiveQuality {
        best: active_best_quality,
        worst: active_worst_quality,
    }
}

/// C `get_q` (rc_vbr_cbr.c:1158).
///
/// A static (slide-show-like) key-frame group with more than one frame to go
/// takes `active_best_quality` outright; everything else asks the rate
/// regulator and then floors the answer at `active_best_quality`.
///
/// The `q > active_worst_quality` arm only pulls q back down when the frame is
/// NOT already targeting the max allowed rate — at the cap, overshooting the
/// worst quality is the intended behaviour.
#[must_use]
pub fn get_q(
    rc: &RateControl,
    cfg: &RateControlCfg,
    scs: &SeqRc,
    twopass: &TwoPassRc,
    frame: &FrameRc,
    active_worst_quality: i32,
    active_best_quality: i32,
) -> i32 {
    if frame.is_intra_only()
        && twopass.kf_zeromotion_pct >= STATIC_KF_GROUP_THRESH
        && rc.frames_to_key > 1
    {
        return active_best_quality;
    }
    let mut q = st::regulate_q(
        rc,
        cfg,
        scs,
        frame,
        active_best_quality,
        active_worst_quality,
        frame.frame_width,
        frame.frame_height,
    );
    if q > active_worst_quality {
        // Special case when we are targeting the max allowed rate.
        if frame.this_frame_target < rc.max_frame_bandwidth {
            q = active_worst_quality;
        }
    }
    q.max(active_best_quality)
}

// ---------------------------------------------------------------------------
// Cyclic refresh
// ---------------------------------------------------------------------------

/// C `cyclic_refresh_init` (rc_vbr_cbr.c:895).
///
/// Decides whether this frame applies cyclic refresh at all, and if so which
/// slice of superblocks it refreshes and how hard. `cr_sb_end` is the
/// ENCODE-CONTEXT-level cursor that walks the frame across successive
/// pictures, so it is `&mut` here — the function is a state machine, not a
/// pure config.
///
/// Every disable is a separate `if`, so the reasons compose: an I_SLICE, a
/// non-base temporal layer, a non-64 superblock size, an average qindex
/// outside `[best_quality + 4 .. 118*255/128]`, too little low motion, or a
/// non-positive refresh percentage all switch it off independently.
pub fn cyclic_refresh_init(
    rc: &mut RateControl,
    scs: &SeqRc,
    frame: &FrameRc,
    slice_type: SliceType,
    cr_sb_end: &mut u32,
    cr: &mut CyclicRefresh,
) {
    // Reset the adaptive elements for intra-only frames and scene changes.
    if slice_type == SliceType::I {
        rc.percent_refresh_adjustment = 5;
        rc.rate_ratio_qdelta_adjustment = 0.25;
    }

    cr.percent_refresh = 20 + rc.percent_refresh_adjustment;
    if frame.sc_class1 {
        cr.percent_refresh += 5;
    }

    cr.apply_cyclic_refresh = slice_type != SliceType::I && frame.temporal_layer_index == 0;
    if scs.super_block_size != 64 {
        cr.apply_cyclic_refresh = false;
    }

    let qp_thresh = 16.max(rc.best_quality + 4);
    // C: `int qp_max_thresh = 118 * MAXQ >> 7;` — `*` binds tighter than
    // `>>` in C too, so this is (118 * 255) >> 7 == 235, not 118 * (255 >> 7).
    let qp_max_thresh = (118 * MAXQ) >> 7;
    if rc.avg_frame_qindex[INTER_FRAME as usize] > qp_max_thresh {
        cr.apply_cyclic_refresh = false;
    }
    if rc.avg_frame_qindex[INTER_FRAME as usize] < qp_thresh {
        cr.apply_cyclic_refresh = false;
    }
    // C: `if (rc->avg_frame_low_motion && rc->avg_frame_low_motion < 50)`.
    // Zero means "not measured yet" and does NOT disable — reading this as
    // `< 50` alone would switch refresh off on the first frame of every GOP.
    if rc.avg_frame_low_motion != 0 && rc.avg_frame_low_motion < 50 {
        cr.apply_cyclic_refresh = false;
    }
    if cr.percent_refresh <= 0 {
        cr.apply_cyclic_refresh = false;
    }
    if !cr.apply_cyclic_refresh {
        return;
    }

    let sb_cnt = u32::from(scs.sb_total_count);
    cr.sb_start = *cr_sb_end;
    cr.sb_end = (cr.sb_start + sb_cnt * (cr.percent_refresh as u32) / 100).min(sb_cnt);
    *cr_sb_end = if cr.sb_end >= sb_cnt { 0 } else { cr.sb_end };

    cr.max_qdelta_perc = 60;

    // Use a larger delta-qp for the first few refresh cycles after a key frame
    // (or a scene change). For screen content the boost decays with distance
    // from the scene change and is suppressed further if either of the last
    // two frames overshot.
    if !frame.sc_class1 {
        cr.rate_ratio_qdelta = if rc.frames_since_key
            < 4 * (1 << scs.hierarchical_levels) * 100 / cr.percent_refresh
        {
            1.50
        } else {
            1.15
        };
        cr.rate_ratio_qdelta += rc.rate_ratio_qdelta_adjustment;
        cr.rate_boost_fac = 15;
    } else {
        // C: `AOMMIN(0.75, (rc->frames_since_key / 10) * 0.1)` — the `/ 10` is
        // INTEGER division, so the factor steps in tenths every ten frames
        // rather than rising smoothly.
        let distance_from_sc_factor = 0.75_f64.min(f64::from(rc.frames_since_key / 10) * 0.1);
        cr.rate_ratio_qdelta = 2.25 + rc.rate_ratio_qdelta_adjustment - distance_from_sc_factor;
        if rc.rc_1_frame < 0 || rc.rc_2_frame < 0 {
            cr.rate_ratio_qdelta -= 0.25;
        }
        cr.rate_boost_fac = 10;
    }
}

/// C `compute_cr_deltaq` (rc_vbr_cbr.c:978).
///
/// The qdelta that would hit `rate_ratio_qdelta` times the base rate, floored
/// at `-max_qdelta_perc` percent of the base qindex so a segment can never be
/// pushed arbitrarily far below the frame.
#[must_use]
pub fn compute_cr_deltaq(
    rc: &RateControl,
    scs: &SeqRc,
    frame: &FrameRc,
    max_qdelta_perc: i32,
    q: i32,
    rate_ratio_qdelta: f64,
) -> i32 {
    let deltaq = port_rc_process::compute_qdelta_by_rate(
        rc.best_quality,
        rc.worst_quality,
        INTER_FRAME,
        q,
        rate_ratio_qdelta,
        scs.encoder_bit_depth,
        frame.sc_class1,
    );
    deltaq.max(-(max_qdelta_perc * q / 100))
}

/// C `cyclic_refresh_compute_cr_qdeltas` (rc_vbr_cbr.c:989).
///
/// Segment 0 is always the frame qindex; segments 1 and 2 get the two ratios,
/// with segment 2's capped at [`CR_MAX_RATE_TARGET_RATIO`].
pub fn cyclic_refresh_compute_cr_qdeltas(
    rc: &RateControl,
    scs: &SeqRc,
    frame: &FrameRc,
    cr: &mut CyclicRefresh,
    base_q_idx: i32,
) {
    let rate_ratio_qdelta = cr.rate_ratio_qdelta;
    let rate_ratio_qdelta_seg2 = CR_MAX_RATE_TARGET_RATIO.min(cr.rate_ratio_qdelta_seg2);
    let max_qdelta_perc = cr.max_qdelta_perc;
    cr.qindex_delta[0] = 0;
    cr.qindex_delta[1] = compute_cr_deltaq(
        rc,
        scs,
        frame,
        max_qdelta_perc,
        base_q_idx,
        rate_ratio_qdelta,
    );
    cr.qindex_delta[2] = compute_cr_deltaq(
        rc,
        scs,
        frame,
        max_qdelta_perc,
        base_q_idx,
        rate_ratio_qdelta_seg2,
    );
}

// ---------------------------------------------------------------------------
// The two rc_pick_q_and_bounds variants
// ---------------------------------------------------------------------------

/// C `rc_pick_q_and_bounds_no_stats_cbr` (rc_vbr_cbr.c:1002) — the CBR arm.
///
/// `frame.top_index` / `frame.bottom_index` are OUTPUTS (C writes them onto
/// the PPCS for the recode loop to read), which is why `frame` is `&mut`.
///
/// The forced-key-frame case reuses `rc->last_boosted_qindex` verbatim rather
/// than regulating, to keep quality continuous across a forced KF.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn rc_pick_q_and_bounds_no_stats_cbr(
    rc: &mut RateControl,
    cfg: &RateControlCfg,
    scs: &SeqRc,
    frame: &mut FrameRc,
    refs: &RefLists<'_>,
    slice_type: SliceType,
) -> Option<i32> {
    debug_assert_eq!(cfg.mode, AomRcMode::Cbr);
    let bit_depth = scs.encoder_bit_depth;
    let width = frame.frame_width;
    let height = frame.frame_height;
    let mut active_worst_quality = st::calc_active_worst_quality_no_stats_cbr(rc, frame);
    let mut active_best_quality = calc_active_best_quality_no_stats_cbr(
        rc,
        scs,
        frame,
        refs,
        active_worst_quality,
        width,
        height,
    )?;

    // Clip the active best and worst quality values to limits.
    active_best_quality = active_best_quality.clamp(rc.best_quality, rc.worst_quality);
    active_worst_quality = active_worst_quality.clamp(active_best_quality, rc.worst_quality);

    frame.top_index = active_worst_quality;
    frame.bottom_index = active_best_quality;

    // Limit the Q range for the adaptive loop.
    if frame.frame_type.is_key() && !rc.this_key_frame_forced && frame.frame_offset != 0 {
        let qdelta = port_rc_process::compute_qdelta_by_rate(
            rc.best_quality,
            rc.worst_quality,
            frame.frame_type.as_rate_model_arg(),
            active_worst_quality,
            2.0,
            bit_depth,
            frame.sc_class1,
        );
        frame.top_index = active_worst_quality + qdelta;
        frame.top_index = frame.top_index.max(frame.bottom_index);
    }

    let mut q;
    if frame.frame_type.is_key() && rc.this_key_frame_forced {
        q = rc.last_boosted_qindex;
    } else {
        q = st::regulate_q(
            rc,
            cfg,
            scs,
            frame,
            active_best_quality,
            active_worst_quality,
            width,
            height,
        );
        if q > frame.top_index {
            // Special case when we are targeting the max allowed rate.
            if frame.this_frame_target >= rc.max_frame_bandwidth {
                frame.top_index = q;
            } else {
                q = frame.top_index;
            }
        }
    }
    if frame.update_type == FrameUpdateType::ArfUpdate {
        rc.arf_q = q;
    }
    let ip = scs.intra_period_length;
    // If short intra refresh.
    if ip > -1 && ip < 256 {
        if slice_type == SliceType::I {
            let q1 = if frame.picture_number == 0 {
                q + 20
            } else {
                rc.q_1_frame
            };
            q = (q + q1) / 2;
        } else if frame.temporal_layer_index == 0 {
            let qdelta = port_rc_process::compute_qdelta_by_rate(
                rc.best_quality,
                rc.worst_quality,
                frame.frame_type.as_rate_model_arg(),
                active_worst_quality,
                QFACTOR,
                bit_depth,
                frame.sc_class1,
            );
            q += qdelta;
        }
    }
    Some(q)
}

/// C `rc_pick_q_and_bounds` (rc_vbr_cbr.c:1187) — the VBR (second-pass) arm.
///
/// Base-layer frames derive `active_best_quality` from `r0` through the qstep
/// ratio rather than from a minq curve; non-base frames either fall through to
/// [`get_active_best_quality`] (pyramid level 0/1, or past `MAX_ARF_LAYERS`)
/// or blend the PARENT LEVEL's stored `active_best_quality` with
/// `active_worst_quality` by the two weight tables.
///
/// `rc.active_best_quality[]` is read here and written by the caller
/// ([`rc_calc_qindex_rate_control`]); `rc.arf_q` is written here.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn rc_pick_q_and_bounds(
    rc: &mut RateControl,
    cfg: &RateControlCfg,
    scs: &SeqRc,
    twopass: &TwoPassRc,
    frame: &mut FrameRc,
    refs: &RefLists<'_>,
) -> i32 {
    debug_assert_eq!(cfg.mode, AomRcMode::Vbr);
    // C initialises `active_best_quality = 0` and both arms below assign it
    // before use; declared without the dead store here.
    let mut active_best_quality;
    let mut active_worst_quality = rc.active_worst_quality;
    let is_intrl_arf_boost = frame.is_internal_arf();
    let hierarchical_levels = usize::from(frame.hierarchical_levels);

    if frame.temporal_layer_index == 0 {
        // C: `unsigned r0_weight_idx = !frame_is_intra_only(ppcs);` — so this
        // is 0 for intra and 1 for base-layer inter. The THIRD entry of
        // `svt_av1_r0_weight` is unreachable from here.
        let r0_weight_idx = usize::from(!frame.is_intra_only());
        let weight = R0_WEIGHT[r0_weight_idx];
        let mut qstep_ratio =
            frame.r0.sqrt() * weight * qp_scale_weight_mainline(scs.qp_scale_compress_strength);
        if qp_scale_on_mainline(scs.qp_scale_compress_strength) {
            // Clamp qstep_ratio so it cannot get past the weight value.
            qstep_ratio = qstep_ratio.min(weight);
        }
        let mut qindex_from_qstep_ratio =
            q_index_from_qstep_ratio(rc.active_worst_quality, qstep_ratio, scs.encoder_bit_depth);
        if frame.sc_class1 && scs.passes == 1 && frame.is_intra_only() {
            qindex_from_qstep_ratio /= 2;
        }
        if !frame.is_intra_only() {
            rc.arf_q = qindex_from_qstep_ratio;
        }
        active_best_quality =
            qindex_from_qstep_ratio.clamp(rc.best_quality, rc.active_worst_quality);
        active_worst_quality = (active_best_quality + (3 * active_worst_quality) + 2) / 4;
    } else {
        let pyramid_level = frame.layer_depth;
        if pyramid_level <= 1 || pyramid_level > MAX_ARF_LAYERS as i32 {
            active_best_quality =
                get_active_best_quality(rc, scs, frame, refs, active_worst_quality);
        } else {
            let parent = rc.active_best_quality[(pyramid_level - 1) as usize] + 1;
            let w1 = NON_BASE_QINDEX_WEIGHT_REF[hierarchical_levels];
            let w2 = NON_BASE_QINDEX_WEIGHT_WQ[hierarchical_levels];
            active_best_quality =
                (w1 * parent + (w2 * active_worst_quality) + ((w1 + w2) / 2)) / (w1 + w2);
        }
        // For alt-ref and GF frames (internal ARFs included) adjust the worst
        // allowed quality too, so hard sections do not clamp ARF and leaf
        // frames at the same Q — the TPL model assumes Q drops with ARF level.
        if !frame.is_overlay && (frame.is_gf_or_arf() || is_intrl_arf_boost) {
            active_worst_quality = (active_best_quality + (3 * active_worst_quality) + 2) / 4;
        }
    }
    let adjusted = adjust_active_best_and_worst_quality_org(
        rc,
        scs,
        twopass,
        frame,
        ActiveQuality {
            best: active_best_quality,
            worst: active_worst_quality,
        },
    );
    active_best_quality = adjusted.best;
    active_worst_quality = adjusted.worst;

    let q = get_q(
        rc,
        cfg,
        scs,
        twopass,
        frame,
        active_worst_quality,
        active_best_quality,
    );

    // Special case when we are targeting the max allowed rate.
    if frame.this_frame_target >= rc.max_frame_bandwidth && q > active_worst_quality {
        active_worst_quality = q;
    }
    frame.top_index = active_worst_quality;
    frame.bottom_index = active_best_quality;

    if frame.update_type == FrameUpdateType::ArfUpdate {
        rc.arf_q = q;
    }
    q
}

/// C `find_min_ref_base_q_idx` (rc_vbr_cbr.c:1262). Marked `NOINLINE` in C.
///
/// The lowest `base_q_idx` among references that are BOTH non-intra and from a
/// strictly lower temporal layer than this frame. C returns `-1` when there is
/// none; the port returns `Option` and the caller reproduces the `-1` where C
/// feeds it into a `MAX`.
#[must_use]
pub fn find_min_ref_base_q_idx(
    refs: &RefLists<'_>,
    list: RefList,
    temporal_layer_index: u8,
) -> Option<i32> {
    let cnt = match list {
        RefList::L0 => refs.l0_count_try,
        RefList::L1 => refs.l1_count_try,
    };
    let mut best: Option<i32> = None;
    for i in 0..cnt {
        let Some(r) = refs.get(list as usize, i) else {
            continue;
        };
        let pic_used = r.tmp_layer_idx < temporal_layer_index;
        if r.pcs_slice_type != SliceType::I && pic_used {
            let v = i32::from(r.base_q_idx);
            best = Some(best.map_or(v, |b: i32| b.min(v)));
        }
    }
    best
}

/// C's `-1` spelling of [`find_min_ref_base_q_idx`], for the call sites that
/// feed it straight into a `MAX`.
#[must_use]
fn min_ref_base_q_idx_or_minus_one(
    refs: &RefLists<'_>,
    list: RefList,
    temporal_layer_index: u8,
) -> i32 {
    find_min_ref_base_q_idx(refs, list, temporal_layer_index).unwrap_or(-1)
}

/// The per-frame ME distortion sums `rc_calc_qindex_rate_control`'s VBR
/// reference-limit arm compares.
///
/// C sums `ref_obj_l0->sb_me_64x64_dist[i]` against
/// `ppcs->me_64x64_distortion[i]` over `pcs->b64_total_count` blocks. Passing
/// the two slices keeps the summation in the port rather than asking a caller
/// for a pre-reduced number that could have been reduced differently.
#[derive(Clone, Copy, Debug)]
pub struct MeDistortion<'a> {
    /// `ref_obj_l0->sb_me_64x64_dist`.
    pub ref_l0: &'a [u32],
    /// `ppcs->me_64x64_distortion`.
    pub cur: &'a [u32],
}

/// C `svt_av1_rc_calc_qindex_rate_control` (rc_vbr_cbr.c:1281) — **EXPORTED**,
/// and the only writer of `base_q_idx` in VBR/CBR.
///
/// Picks q with the mode's `rc_pick_q_and_bounds` variant, clamps it, then
/// applies ONE of four mutually exclusive reference-qindex floors:
///
/// | condition | floor |
/// |---|---|
/// | non-base temporal layer | `min_ref_q - (gop_constraint_rc ? 8 : 0)` |
/// | base layer, CBR | `min_ref_q - 16` |
/// | base layer, VBR, non-transition, non-I | `ref_q - 25*4` (or `- 6*4` when this frame is much harder than its reference) |
/// | otherwise | none |
///
/// Then, in CBR only, it sets up cyclic refresh — via the
/// `aq_cyclic_refresh_setup` callback, because C's
/// `svt_aom_cyclic_refresh_setup` belongs to `Codec/rc_aq.c` rather than to
/// this file, and it may switch `apply_cyclic_refresh` back OFF.
///
/// Returns the new `base_q_idx`; the caller stores it into the frame header.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn rc_calc_qindex_rate_control(
    rc: &mut RateControl,
    cfg: &RateControlCfg,
    scs: &SeqRc,
    twopass: &TwoPassRc,
    frame: &mut FrameRc,
    refs: &RefLists<'_>,
    slice_type: SliceType,
    me_dist: Option<MeDistortion<'_>>,
    cr_sb_end: &mut u32,
    cr: &mut CyclicRefresh,
    aq_cyclic_refresh_setup: impl FnOnce(&mut CyclicRefresh),
) -> Option<i32> {
    let mut new_qindex = if cfg.mode == AomRcMode::Cbr {
        rc_pick_q_and_bounds_no_stats_cbr(rc, cfg, scs, frame, refs, slice_type)?
    } else {
        rc_pick_q_and_bounds(rc, cfg, scs, twopass, frame, refs)
    };
    new_qindex = st::clamp_qindex(scs, new_qindex);

    // Limit the qindex based on the qindex of the reference frames.
    let tli = frame.temporal_layer_index;
    if tli != 0 {
        let l0 = min_ref_base_q_idx_or_minus_one(refs, RefList::L0, tli);
        let l1 = min_ref_base_q_idx_or_minus_one(refs, RefList::L1, tli);
        let ref_base_q_idx = l0.max(l1);
        let limit = if scs.gop_constraint_rc { 2 } else { 0 };
        new_qindex = new_qindex.max(ref_base_q_idx - limit * 4);
    } else if cfg.mode == AomRcMode::Cbr {
        let l0 = min_ref_base_q_idx_or_minus_one(refs, RefList::L0, tli);
        let l1 = min_ref_base_q_idx_or_minus_one(refs, RefList::L1, tli);
        let ref_base_q_idx = l0.max(l1);
        let limit = 4;
        new_qindex = new_qindex.max(ref_base_q_idx - limit * 4);
    } else if frame.transition_present != 1 && slice_type != SliceType::I && !scs.gop_constraint_rc
    {
        let mut cur_dist = 0_u64;
        let mut ref_dist = 0_u64;
        // C dereferences `get_ref_obj(pcs, REF_LIST_0, 0)` unconditionally
        // here and indexes two `uint32_t*` arrays; `None` for either is the
        // port refusing rather than reproducing a null deref, and it leaves
        // `new_qindex` at its clamped value.
        if let (Some(d), Some(l0)) = (me_dist, refs.get(RefList::L0 as usize, 0)) {
            let n = usize::from(frame.b64_total_count);
            for i in 0..n {
                ref_dist += u64::from(d.ref_l0[i]);
                cur_dist += u64::from(d.cur[i]);
            }
            // C: `if (cur_dist > 3 * ref_dist || (ppcs->r0 - ref_obj_l0->r0 > 0))`.
            // Note the second test reads the REFERENCE OBJECT's r0, while
            // `get_active_best_quality` reads the PCS mirror `ref_pic_r0` for
            // an analogous comparison — hence both fields on `RefPicRc`.
            let mut limit = 25;
            if cur_dist > 3 * ref_dist || (frame.r0 - l0.obj_r0 > 0.0) {
                limit = 6;
            }
            let mut ref_base_q_idx = 0;
            if l0.pcs_slice_type != SliceType::I {
                ref_base_q_idx = i32::from(l0.base_q_idx);
            }
            // C guards this on `ppcs->ref_list1_count_try` and then reads
            // `pcs->ref_slice_type[REF_LIST_1][0]` — a plain array, so an empty
            // DPB slot still reads a value. The port additionally requires the
            // slot to exist.
            if slice_type == SliceType::B
                && refs.l1_count_try != 0
                && let Some(l1) = refs.get(RefList::L1 as usize, 0)
                && l1.pcs_slice_type != SliceType::I
            {
                ref_base_q_idx = ref_base_q_idx.max(i32::from(l1.base_q_idx));
            }
            new_qindex = new_qindex.max(ref_base_q_idx - limit * 4);
        }
    }

    new_qindex = st::clamp_qindex(scs, new_qindex);

    if cfg.mode == AomRcMode::Cbr {
        // CR is not used in the qindex derivation, so compute it all here.
        cyclic_refresh_init(rc, scs, frame, slice_type, cr_sb_end, cr);
        if cr.apply_cyclic_refresh {
            // C calls `svt_aom_cyclic_refresh_setup` here. That function lives
            // in `Codec/rc_aq.c`, NOT in this file, and it both fills
            // `rate_ratio_qdelta_seg2` / `actual_num_seg*` and can switch
            // `apply_cyclic_refresh` back OFF when its motion gate rejects
            // every superblock in the band. Taking it as a callback keeps the
            // file boundary honest — inlining someone else's port here is how
            // two lanes end up with two copies that drift.
            aq_cyclic_refresh_setup(cr);
        }
        if cr.apply_cyclic_refresh {
            cyclic_refresh_compute_cr_qdeltas(rc, scs, frame, cr, new_qindex);
        }
    }

    frame.base_q_idx = new_qindex;
    Some(new_qindex)
}
