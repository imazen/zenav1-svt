//! Per-block helpers of `Source/Lib/Codec/product_coding_loop.c` — the ME
//! plumbing, the MV size categories, the reference-pruning table and the
//! LPD0 inter cost.
//!
//! | this module | C |
//! |---|---|
//! | [`derive_me_offsets`] | `:2051-2076` |
//! | [`clip_mv_on_pic_boundary`] | `:2254-2271` |
//! | [`check_spatial_mv_size`] | `:2278-2301` |
//! | [`check_temporal_mv_size`] | `:2307-2323` |
//! | [`determine_best_references`] | `:65-116` |
//! | [`perform_md_reference_pruning`] | `:3004-3092` |
//! | [`compute_lpd0_cost_from_variance`] | `:8247-8259` |
//! | [`lpd0_inter_best_variance`] / [`compute_lpd0_cost_inter`] | `:8267-8341` |
//! | [`is_intra_bordered`] | `:8119-8136` |
//! | [`get_enable_use_best_me`] | `:9310-9341` |
//! | [`ssim_threshold_factor_for_tx_type_search`] | `:4578-4580` |
//! | [`eliminate_candidate_based_on_pme_me_results`] | `mode_decision.c:3407-3416` |
//!
//! # Evidence
//!
//! Every function here is `static` in C with no exported symbol, so this
//! module is **tier 4** throughout — hand-derived vectors traced against
//! the C source (`docs/WORKING-ON-THIS.md` §4). The one exception is
//! [`derive_me_offsets`], whose only arithmetic beyond the SB-128 geometry
//! is `svt_aom_get_me_block_offset`, which IS gated at tier 1 in
//! [`super::predicates`]; this module calls that port rather than
//! re-transcribing it.
//!
//! Two C behaviours reproduced ON PURPOSE, both flagged where they occur:
//! `perform_md_reference_pruning`'s `dev_to_the_best` loop fills only
//! seven of its eight entries, and `check_spatial_mv_size` compares MV
//! components SIGNED against positive thresholds so a large negative MV
//! never raises the category.

use super::predicates::{
    MAX_NUM_OF_REF_PIC_LIST, MeCandidateRef, REF_LIST_MAX_DEPTH, RefPruningState, TOT_INTER_GROUP,
    get_me_block_offset, get_ref_frame_type,
};
use alloc::vec::Vec;
use svtav1_types::motion::Mv;

// ---------------------------------------------------------------------------
// ME offsets (product_coding_loop.c:2051-2076)
// ---------------------------------------------------------------------------

/// What [`derive_me_offsets`] writes onto the MD context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MeOffsets {
    /// C `ctx->me_sb_addr`.
    pub me_sb_addr: u32,
    /// C `ctx->me_block_offset`.
    pub me_block_offset: u32,
    /// C `ctx->me_cand_offset`.
    pub me_cand_offset: u32,
    /// C `ctx->geom_offset_x` / `geom_offset_y` (SB-128 only).
    pub geom_offset_x: u32,
    pub geom_offset_y: u32,
}

/// C `derive_me_offsets` (product_coding_loop.c:2051-2076).
///
/// `sb_index` is C's `ctx->sb_ptr->index`, used only on the SB-64 path.
/// On the SB-128 path the ME address is recomputed from the block origin
/// over the **b64** grid, because ME results are always stored at 64x64
/// granularity even when MD runs on 128x128 superblocks — that is what
/// `geom_offset_x/y` exist to compensate for.
///
/// An off-by-one here silently feeds a NEIGHBOURING block's MVs into
/// injection, which is why the two arms are written out separately rather
/// than folded.
#[allow(clippy::too_many_arguments)]
pub fn derive_me_offsets(
    sb_size_is_128: bool,
    b64_size: u32,
    aligned_width: u32,
    sb_index: u32,
    blk_org_x: u32,
    blk_org_y: u32,
    bsize: u8,
    enable_me_8x8: bool,
    enable_me_16x16: bool,
    max_cand: u32,
) -> MeOffsets {
    let mut out = MeOffsets::default();
    let me_block_offset =
        get_me_block_offset(blk_org_x, blk_org_y, bsize, enable_me_8x8, enable_me_16x16);

    if sb_size_is_128 {
        let me_sb_size = b64_size;
        let me_pic_width_in_sb = aligned_width.div_ceil(b64_size);
        let me_sb_x = blk_org_x / me_sb_size;
        let me_sb_y = blk_org_y / me_sb_size;
        out.me_sb_addr = me_sb_x + me_sb_y * me_pic_width_in_sb;
        out.geom_offset_x = (me_sb_x & 1) * me_sb_size;
        out.geom_offset_y = (me_sb_y & 1) * me_sb_size;
    } else {
        out.me_sb_addr = sb_index;
    }
    out.me_block_offset = me_block_offset;
    out.me_cand_offset = me_block_offset * max_cand;
    out
}

// ---------------------------------------------------------------------------
// MV clipping (product_coding_loop.c:2254-2271)
// ---------------------------------------------------------------------------

/// C `clip_mv_on_pic_boundary` (product_coding_loop.c:2254-2271).
///
/// Clamps an eighth-pel MV so the predicted block stays inside the PADDED
/// reference. Two things are easy to get wrong:
///
/// * The four tests are SEQUENTIAL, not exclusive, and each writes the MV
///   the next one reads. A block wider than the padded picture therefore
///   takes the right-edge clamp and then the left-edge clamp.
/// * The replacements are NOT symmetric: the max side uses
///   `(max_width - blk_org_x) * 8` (no block-size term) while the min side
///   uses `(-blk_org_x - bwidth) * 8` (with it).
pub fn clip_mv_on_pic_boundary(
    blk_org_x: i32,
    blk_org_y: i32,
    bwidth: i32,
    bheight: i32,
    ref_max_width: i32,
    ref_max_height: i32,
    ref_border: i32,
    mvx: &mut i16,
    mvy: &mut i16,
) {
    if blk_org_x + (i32::from(*mvx) >> 3) + bwidth > ref_max_width + ref_border {
        *mvx = ((ref_max_width - blk_org_x) * 8) as i16;
    }
    if blk_org_y + (i32::from(*mvy) >> 3) + bheight > ref_max_height + ref_border {
        *mvy = ((ref_max_height - blk_org_y) * 8) as i16;
    }
    if blk_org_x + (i32::from(*mvx) >> 3) < -ref_border {
        *mvx = ((-blk_org_x - bwidth) * 8) as i16;
    }
    if blk_org_y + (i32::from(*mvy) >> 3) < -ref_border {
        *mvy = ((-blk_org_y - bheight) * 8) as i16;
    }
}

// ---------------------------------------------------------------------------
// MV size categories (product_coding_loop.c:2278-2323)
// ---------------------------------------------------------------------------

/// C `HIGH_SPATIAL_MV_TH` / `MEDIUM_` / `LOW_` (motion_estimation.h:69-71).
pub const HIGH_SPATIAL_MV_TH: i16 = 2048;
pub const MEDIUM_SPATIAL_MV_TH: i16 = 512;
pub const LOW_SPATIAL_MV_TH: i16 = 256;
/// C `MEDIUM_TEMPORAL_MV_TH` / `LOW_TEMPORAL_MV_TH`
/// (motion_estimation.h:66-67).
pub const MEDIUM_TEMPORAL_MV_TH: i32 = 2048;
pub const LOW_TEMPORAL_MV_TH: i32 = 1024;

/// C `check_spatial_mv_size` (product_coding_loop.c:2278-2301).
///
/// **The comparisons are SIGNED against positive thresholds** — C writes
/// `mvp.x > HIGH_SPATIAL_MV_TH`, not `abs(mvp.x) > ...`. A large NEGATIVE
/// MV therefore never raises the category. This looks like a C defect and
/// is reproduced deliberately (`docs/WORKING-ON-THIS.md` §7: a C bug is
/// still the oracle).
///
/// The HIGH arm returns immediately; the MEDIUM and LOW arms only raise
/// the running maximum and keep scanning, so the result is the maximum
/// category over all MVPs and the (shared) ME MV.
pub fn check_spatial_mv_size(mvp_array: &[Mv], me_mv_x: i16, me_mv_y: i16) -> u8 {
    let mut mult = 0u8;
    for mvp in mvp_array {
        if mvp.x > HIGH_SPATIAL_MV_TH
            || mvp.y > HIGH_SPATIAL_MV_TH
            || me_mv_x > HIGH_SPATIAL_MV_TH
            || me_mv_y > HIGH_SPATIAL_MV_TH
        {
            return mult.max(3);
        } else if mvp.x > MEDIUM_SPATIAL_MV_TH
            || mvp.y > MEDIUM_SPATIAL_MV_TH
            || me_mv_x > MEDIUM_SPATIAL_MV_TH
            || me_mv_y > MEDIUM_SPATIAL_MV_TH
        {
            mult = mult.max(2);
        } else if mvp.x > LOW_SPATIAL_MV_TH
            || mvp.y > LOW_SPATIAL_MV_TH
            || me_mv_x > LOW_SPATIAL_MV_TH
            || me_mv_y > LOW_SPATIAL_MV_TH
        {
            mult = mult.max(1);
        }
    }
    mult
}

/// C `INVALID_MV` (definitions.h) as an `as_int`.
pub const INVALID_MV_AS_INT: u32 = 0x8000_8000;

/// C `check_temporal_mv_size` (product_coding_loop.c:2307-2323).
///
/// Unlike its spatial twin this one DOES take `ABS`, and it reads a single
/// TPL entry — the one at `(mi_row >> 1, mi_col >> 1)` in a half-resolution
/// grid. It never reaches 3.
pub fn check_temporal_mv_size(mfmv0: Mv) -> u8 {
    let mut mult = 0u8;
    if mfmv0.as_int() != INVALID_MV_AS_INT {
        let ay = i32::from(mfmv0.y).abs();
        let ax = i32::from(mfmv0.x).abs();
        if ay > MEDIUM_TEMPORAL_MV_TH || ax > MEDIUM_TEMPORAL_MV_TH {
            mult = mult.max(2);
        } else if ay > LOW_TEMPORAL_MV_TH || ax > LOW_TEMPORAL_MV_TH {
            mult = mult.max(1);
        }
    }
    mult
}

/// The TPL index `check_temporal_mv_size` reads:
/// `(mi_row >> 1) * (mi_stride >> 1) + (mi_col >> 1)`.
#[inline]
pub fn temporal_mv_tpl_index(blk_org_x: u32, blk_org_y: u32, mi_stride: u32) -> usize {
    // C `MI_SIZE_LOG2` is 2.
    let mi_row = blk_org_y >> 2;
    let mi_col = blk_org_x >> 2;
    ((mi_row >> 1) * (mi_stride >> 1) + (mi_col >> 1)) as usize
}

// ---------------------------------------------------------------------------
// Reference set (product_coding_loop.c:65-116)
// ---------------------------------------------------------------------------

/// C `LAST_BWD_FRAME` (definitions.h:1412) — the compound ref type for
/// (LAST, BWDREF).
///
/// It is **8**, i.e. exactly `av1_ref_frame_type([LAST, BWDREF])`
/// (`TOTAL_REFS_PER_FRAME + FWD_RF_OFFSET(LAST) + BWD_RF_OFFSET(BWDREF) *
/// FWD_REFS` = 8 + 0 + 0). The equality is asserted in this module's
/// tests rather than assumed, because `determine_best_references`
/// compares against the computed value in one place and appends the
/// constant in another.
pub const LAST_BWD_FRAME: i8 = 8;

/// C `determine_best_references` (product_coding_loop.c:65-116).
///
/// Builds `ctx->ref_frame_type_arr` from the ME candidate array plus the
/// B-slice LAST / BWD / LAST_BWD backfill. **The ORDER is load-bearing**:
/// every later loop (`perform_md_reference_pruning`,
/// `estimate_ref_frames_num_bits`, every injector) iterates this array in
/// index order, so appending the backfill entries in a different order
/// changes downstream behaviour even with an identical set.
///
/// Note C does NOT deduplicate: two ME candidates naming the same
/// reference produce two entries.
pub fn determine_best_references(
    me_cands: &[MeCandidateRef],
    total_me_cnt: usize,
    is_b_slice: bool,
    ref_list0_count_try: bool,
    ref_list1_count_try: bool,
) -> Vec<i8> {
    let mut out: Vec<i8> = Vec::new();
    let mut is_last_added = false;
    let mut is_bwd_added = false;
    let mut is_last_bwd_added = false;

    for cand in me_cands.iter().take(total_me_cnt) {
        match cand.direction {
            0 => {
                out.push(get_ref_frame_type(0, cand.ref_idx_l0) as i8);
                if cand.ref_idx_l0 == 0 {
                    is_last_added = true;
                }
            }
            1 => {
                out.push(get_ref_frame_type(1, cand.ref_idx_l1) as i8);
                if cand.ref_idx_l1 == 0 {
                    is_bwd_added = true;
                }
            }
            2 => {
                let rf0 = get_ref_frame_type(cand.ref0_list, cand.ref_idx_l0) as i8;
                let rf1 = get_ref_frame_type(cand.ref1_list, cand.ref_idx_l1) as i8;
                out.push(crate::inter_mvp::av1_ref_frame_type([rf0, rf1]));
                if rf0 == 1 && rf1 == 5 {
                    is_last_bwd_added = true;
                }
            }
            // C's `else` arm is svt_aom_assert_err("corrupted me results"),
            // which is a no-op in a release build and appends nothing.
            _ => {}
        }
    }

    if is_b_slice {
        if !is_last_added && ref_list0_count_try {
            out.push(1); // LAST_FRAME
        }
        if !is_bwd_added && ref_list1_count_try {
            out.push(5); // BWDREF_FRAME
        }
        if !is_last_bwd_added && ref_list0_count_try && ref_list1_count_try {
            out.push(LAST_BWD_FRAME);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Reference pruning table (product_coding_loop.c:3004-3092)
// ---------------------------------------------------------------------------

/// The `ctx->ref_pruning_ctrls` fields [`perform_md_reference_pruning`]
/// reads.
#[derive(Debug, Clone)]
pub struct RefPruningCtrls {
    pub enabled: bool,
    /// C `max_dev_to_best[TOT_INTER_GROUP]`; `u32::MAX` is C's `(uint32_t)~0`.
    pub max_dev_to_best: [u32; TOT_INTER_GROUP],
    pub check_closest_multiplier: u32,
    pub closest_refs: [bool; TOT_INTER_GROUP],
}

/// C `perform_md_reference_pruning` (product_coding_loop.c:3004-3092),
/// **with the TPL arm omitted** — see below.
///
/// Fills `ctx->ref_filtering_res`, the table
/// [`super::predicates::is_valid_unipred_ref`] and
/// [`super::predicates::is_valid_bipred_ref`] read.
///
/// `early_dist[list * REF_LIST_MAX_DEPTH + ref]` is C's
/// `early_inter_distortion_array`, pre-filled by the caller from
/// `MIN(fp_me_dist, best_fp_mvp_dist)` for every single reference in
/// `ref_frame_type_arr` — the entries no reference visits keep C's
/// `0xFEFEFEFE` memset, which is what `None` means here.
///
/// **TPL scope.** C's `use_tpl_info_offset` arm adds a per-reference
/// offset derived from `get_sb_tpl_inter_stats`. It is gated on
/// `ctx->ref_pruning_ctrls.use_tpl_info_offset && pcs->ppcs->tpl_ctrls.enable`;
/// this port takes `offset_tab` as an INPUT so the arm can be supplied by
/// whoever ports TPL, rather than silently pretending the offsets are
/// always zero.
///
/// **A C quirk reproduced on purpose:** the `dev_to_the_best` fill loop
/// runs `i < num_of_cand_to_sort - 1`, i.e. 0..6, leaving entry **7**
/// (list 1, ref 3) at its `0` initialiser. Zero is below every non-zero
/// `pruning_th`, so that reference is enabled for every group whose
/// threshold is non-zero regardless of its actual distortion. This is
/// almost certainly an upstream off-by-one; byte-identity means
/// reproducing it (`docs/WORKING-ON-THIS.md` §7).
pub fn perform_md_reference_pruning(
    ctrls: &RefPruningCtrls,
    early_dist: &[u32; MAX_NUM_OF_REF_PIC_LIST * REF_LIST_MAX_DEPTH],
    offset_tab: &[[u32; REF_LIST_MAX_DEPTH]; MAX_NUM_OF_REF_PIC_LIST],
    min_dist: u32,
    bwidth: u32,
    bheight: u32,
    picture_qp: u32,
) -> RefPruningState {
    let mut state = RefPruningState {
        enabled: ctrls.enabled,
        closest_refs: ctrls.closest_refs,
        ..Default::default()
    };

    let th = (ctrls.check_closest_multiplier * (bheight * bwidth) * picture_qp) / 24;
    if ctrls.check_closest_multiplier != 0
        && early_dist[0] < th
        && early_dist[REF_LIST_MAX_DEPTH] < th
    {
        for li in 0..MAX_NUM_OF_REF_PIC_LIST {
            for ri in 0..REF_LIST_MAX_DEPTH {
                for gi in 0..TOT_INTER_GROUP {
                    if ri == 0 || ctrls.max_dev_to_best[gi] == u32::MAX {
                        state.do_ref[gi][li][ri] = true;
                    }
                }
            }
        }
        return state;
    }

    let n = MAX_NUM_OF_REF_PIC_LIST * REF_LIST_MAX_DEPTH;
    let mut dev_to_the_best = [0u32; MAX_NUM_OF_REF_PIC_LIST * REF_LIST_MAX_DEPTH];
    // C: `i < num_of_cand_to_sort - 1` — the last entry is NEVER written.
    for (i, dev) in dev_to_the_best.iter_mut().enumerate().take(n - 1) {
        let a = i64::from(early_dist[i].max(1));
        let b = i64::from(min_dist.max(1));
        *dev = ((a - b) * 100 / b) as u32;
    }

    for li in 0..MAX_NUM_OF_REF_PIC_LIST {
        for ri in 0..REF_LIST_MAX_DEPTH {
            for gi in 0..TOT_INTER_GROUP {
                let offset = offset_tab[li][ri];
                let pruning_th = if offset == u32::MAX || ctrls.max_dev_to_best[gi] == 0 {
                    0
                } else if ctrls.max_dev_to_best[gi] == u32::MAX {
                    u32::MAX
                } else {
                    (i64::from(ctrls.max_dev_to_best[gi]) - i64::from(offset)).max(0) as u32
                };
                if dev_to_the_best[li * REF_LIST_MAX_DEPTH + ri] < pruning_th {
                    state.do_ref[gi][li][ri] = true;
                }
            }
        }
    }
    state
}

// ---------------------------------------------------------------------------
// LPD0 cost (product_coding_loop.c:8245-8341)
// ---------------------------------------------------------------------------

/// C `VLPD0_NOISE_SHIFT` (product_coding_loop.c:8245).
pub const VLPD0_NOISE_SHIFT: u32 = 10;

use svtav1_types::math::rd::rdcost_u64 as rdcost;

/// C `compute_lpd0_cost_from_variance` (product_coding_loop.c:8247-8259).
///
/// The distortion is `min(variance / area, lambda >> 10) * area` — a
/// noise floor, not a plain variance. `variance / area` is INTEGER
/// division done BEFORE the clamp and then multiplied back by `area`, so
/// the result is quantised to multiples of `area`; a port that clamped
/// the raw variance would get a different number.
pub fn compute_lpd0_cost_from_variance(
    variance: u32,
    bwidth: u32,
    bheight: u32,
    lambda: u32,
    partition_none_rate: u32,
) -> u64 {
    let area = bwidth * bheight;
    let noise = lambda >> VLPD0_NOISE_SHIFT;
    let var_pp = variance / area;
    let dist = u64::from(var_pp.min(noise)) * u64::from(area);
    rdcost(u64::from(lambda), u64::from(partition_none_rate), dist)
}

/// One ME candidate as [`lpd0_inter_best_variance`] consumes it: the
/// candidate's decoded `(list_idx, ref_idx)` and its full-pel MV.
#[derive(Debug, Clone, Copy)]
pub struct Lpd0MeCand {
    pub list_idx: u8,
    pub ref_idx: u8,
    /// C `me_results->me_mv_array[...]`, FULL-pel (C multiplies by 8).
    pub mv_full_pel: Mv,
}

/// The candidate walk of C `compute_lpd0_cost_inter`
/// (product_coding_loop.c:8267-8341), up to but excluding the variance
/// call.
///
/// Returns the candidates in the order C evaluates them, with their MVs
/// already converted to eighth-pel and clipped
/// ([`clip_mv_on_pic_boundary`]). Splitting the walk from the variance
/// keeps the part that needs no reference pixels testable on its own; the
/// caller feeds each entry to `svt_aom_mefn_ptr[bsize].vf` and keeps the
/// minimum.
///
/// Three C behaviours that are easy to lose:
///
/// * **`BI_PRED` candidates are skipped** and do NOT count toward the cap.
/// * **The cap is `++cand_count > 2`**, i.e. up to THREE candidates are
///   evaluated, and the break happens AFTER the third is scored.
/// * **`list_idx` is the direction itself** (`direction ? l1 : l0` selects
///   the ref index), not a separate field.
pub fn lpd0_inter_candidate_walk(
    me_cands: &[MeCandidateRef],
    total_me_cnt: usize,
    mv_of: impl Fn(u8, u8) -> Mv,
    blk_org_x: i32,
    blk_org_y: i32,
    bwidth: i32,
    bheight: i32,
    ref_max_width: i32,
    ref_max_height: i32,
    ref_border: i32,
) -> Vec<Lpd0MeCand> {
    /// C `BI_PRED`.
    const BI_PRED: u8 = 2;
    let mut out = Vec::new();
    let mut cand_count = 0u8;
    for cand in me_cands.iter().take(total_me_cnt) {
        if cand.direction == BI_PRED {
            continue;
        }
        let list_idx = cand.direction;
        let ref_idx = if cand.direction != 0 {
            cand.ref_idx_l1
        } else {
            cand.ref_idx_l0
        };
        let full = mv_of(list_idx, ref_idx);
        let mut mv_x = full.x.wrapping_mul(8);
        let mut mv_y = full.y.wrapping_mul(8);
        clip_mv_on_pic_boundary(
            blk_org_x,
            blk_org_y,
            bwidth,
            bheight,
            ref_max_width,
            ref_max_height,
            ref_border,
            &mut mv_x,
            &mut mv_y,
        );
        out.push(Lpd0MeCand {
            list_idx,
            ref_idx,
            mv_full_pel: Mv { x: mv_x, y: mv_y },
        });
        cand_count += 1;
        if cand_count > 2 {
            break;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Small gates
// ---------------------------------------------------------------------------

/// C `is_intra_bordered` (product_coding_loop.c:8119-8136).
///
/// Drives `use_neighbouring_mode_ctrls` candidate reduction. Requires
/// BOTH neighbours to exist AND both to be intra; a missing neighbour
/// yields 0, not 1.
#[inline]
pub fn is_intra_bordered(
    has_above: bool,
    has_left: bool,
    above_is_inter: bool,
    left_is_inter: bool,
) -> bool {
    has_above && has_left && !above_is_inter && !left_is_inter
}

/// C `eliminate_candidate_based_on_pme_me_results`
/// (mode_decision.c:3407-3416).
///
/// Sets `dc_cand_only` when the best of the ME and PME distortions is
/// below `dc_only_th * area`, collapsing the whole intra candidate set on
/// an easy inter block.
///
/// The outer guard is `||`, not `&&`: a single VALID distortion is enough
/// to enter, and the `MIN` then picks it because the other is
/// `u32::MAX`. **The threshold multiply is `u32` and CAN overflow** —
/// `dc_only_th * bheight * bwidth` wraps for a large threshold on a
/// 128x128 block. C wraps; the port wraps.
///
/// `dc_cand_only` is only ever RAISED — C never clears it, so the port
/// takes it as `&mut`.
pub fn eliminate_candidate_based_on_pme_me_results(
    md_pme_dist: u32,
    md_me_dist: u32,
    dc_only_th: u32,
    bwidth: u32,
    bheight: u32,
    dc_cand_only: &mut bool,
) {
    if md_pme_dist != u32::MAX || md_me_dist != u32::MAX {
        let th = dc_only_th.wrapping_mul(bheight).wrapping_mul(bwidth);
        if md_pme_dist.min(md_me_dist) < th {
            *dc_cand_only = true;
        }
    }
}

/// C `derive_ssim_threshold_factor_for_tx_type_search`
/// (product_coding_loop.c:4578-4580).
///
/// The TXT-side envelope, distinct from the full-MD twin in
/// [`crate::ssim_md`] (1.02 / 1.03, mode_decision.c:3862) — same shape,
/// different constants, so the two must not be shared.
#[inline]
pub fn ssim_threshold_factor_for_tx_type_search(input_resolution_at_least_1080p: bool) -> f64 {
    if input_resolution_at_least_1080p {
        1.06
    } else {
        1.05
    }
}

/// C `get_enable_use_best_me` (product_coding_loop.c:9310-9341), levels 1
/// and 3 only.
///
/// `use_best_references == 2` needs `get_sb_tpl_inter_stats`, which is
/// TPL machinery this port does not have; that arm returns `None` here
/// rather than a fabricated answer. Levels 1 and 3 need no TPL and are
/// returned as `Some`.
///
/// Level 1's threshold is on the b64's `me_8x8_distortion` (> 45000);
/// level 3 is unconditional. Both require `temporal_layer_index > 0`.
pub fn get_enable_use_best_me(
    use_best_references: u8,
    temporal_layer_index: u32,
    me_8x8_dist: u32,
) -> Option<bool> {
    if use_best_references == 0 || temporal_layer_index == 0 {
        return Some(false);
    }
    match use_best_references {
        1 => Some(me_8x8_dist > 45000),
        2 => None,
        _ => Some(true),
    }
}

/// C's `b64_idx` for the level-1 arm (product_coding_loop.c:9314-9318).
#[inline]
pub fn b64_index(blk_org_x: u32, blk_org_y: u32, aligned_width: u32, b64_size: u32) -> u32 {
    let b64_x = blk_org_x / 64;
    let b64_y = blk_org_y / 64;
    let pic_width_in_b64 = aligned_width.div_ceil(b64_size);
    b64_y * pic_width_in_b64 + b64_x
}

/// Convenience: which `InterCandGroup` indices a filled
/// [`RefPruningState`] enables for a given `(list, ref)`. Present so a
/// caller can assert on the whole row rather than probing one group at a
/// time.
pub fn enabled_groups(state: &RefPruningState, list_idx: usize, ref_idx: usize) -> Vec<usize> {
    (0..TOT_INTER_GROUP)
        .filter(|&g| state.do_ref[g][list_idx][ref_idx])
        .collect()
}

// ---------------------------------------------------------------------------
// TIER 4 — every C function in this module is `static` with no exported
// symbol (`nm -g` checked), so these are hand-derived vectors traced
// against the C source. `derive_me_offsets` leans on the tier-1
// `svt_aom_get_me_block_offset` for the part that has an oracle.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
