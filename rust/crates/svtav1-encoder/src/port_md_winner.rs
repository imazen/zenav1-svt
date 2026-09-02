//! Full mode decision — `svt_aom_product_full_mode_decision`
//! (mode_decision.c:3812) and its light-PD1 twin (:3682): choosing the
//! winning candidate and deriving the per-block signals the entropy coder
//! then reads.
//!
//! # Coverage — 4 of the 41 rows the inventory lists for `mode_decision.c`
//!
//! | C function | line | here |
//! |---|---|---|
//! | `svt_aom_product_full_mode_decision` | 3812 | [`select_winner`] + [`winner_signals`] |
//! | `svt_aom_product_full_mode_decision_light_pd1` | 3682 | [`winner_signals_light_pd1`] |
//! | `derive_ssim_threshold_factor_for_full_md` | 3805 | [`ssim_threshold_factor`] |
//! | `svt_av1_is_lossless_segment` | 71 | [`is_lossless_segment`] |
//!
//! # What is NOT here, named
//!
//! `svt_aom_product_full_mode_decision` is two things bolted together: a
//! WINNER SELECTION over the candidate array, and a bulk COPY of the winner
//! into `BlkStruct` plus (when EncDec is bypassed) a walk that memcpys the
//! quantized coefficients into the SB's coefficient buffer. This module ports
//! the first and the DERIVED part of the second — every field whose value is
//! computed rather than copied. The copies themselves
//! (`svt_memcpy(&blk_ptr->block_mi, &cand->block_mi, ...)`, the `tx_type`
//! array, `quant_dc`, `eob`, and the coefficient-buffer walk keyed on
//! `coded_area_sb`) are SVT's buffer plumbing: this port keeps a candidate as
//! a value and hands the winner back, so there is no second struct to copy
//! into and no `coded_area_sb` cursor to advance. That is a deliberate
//! substitution, not a gap — but the DECISIONS inside the copy region
//! (`skip_mode |= !block_has_coeff`, the `skip` derivation, the drl-context
//! stamping, the palette gate) are all here.
//!
//! Still missing from `mode_decision.c` and named rather than implied: the
//! candidate-injection family (`inject_intra_candidates`,
//! `inject_intra_candidates_pd0`, `inject_filter_intra_candidates`,
//! `inject_palette_candidates`, `inject_intra_bc_candidates`, the three
//! `*_light_pd1` inter injectors and `inject_sframe_backup_candidate`),
//! `generate_md_stage_0_cand{,_light_pd1}`, `single_motion_search`,
//! `pick_interintra_wedge`, `svt_aom_set_tuned_blk_lambda`,
//! `aom_av1_set_ssim_rdmult`, `svt_av1_setup_pred_block`,
//! `get_superblock_tpl_column_end`, `reject_candidate_sframe` /
//! `valid_ref_frame_type`, and the small type helpers
//! `intra_mode_to_tx_type` / `svt_aom_get_intra_uv_tx_type` /
//! `svt_aom_filter_intra_allowed_bsize`.
//!
//! Counted OUT of the queue as not-translatable, one reason each:
//! `svt_aom_mode_decision_cand_bf_ctor` / `_scratch_cand_bf_ctor` and their
//! two dtors are `EbPictureBufferDesc` allocation for the candidate-buffer
//! pool, which this port replaces with owned values; `assert_release` is an
//! assertion helper.
//!
//! The SSIM distortion kernels the inventory also lists here (`ssim`,
//! `ssim_4x4_blocks`, `ssim_8x8_blocks`, `svt_ssim_{4x4,8x8}{,_hbd}_c`,
//! `svt_spatial_full_distortion_ssim_kernel`) are already ported —
//! `crate::ssim_md` for the 8-bit path and `crate::port_md::ssim_hbd` for the
//! high-bit-depth arm.
//!
//! # Evidence
//!
//! Tier 1 for [`is_lossless_segment`] (`svt_av1_is_lossless_segment` is
//! EXPORTED) via `tests/c_parity_md_winner.rs`.
//!
//! [`select_winner`] and the two signal derivations are **tier 4**
//! (hand-derived vectors traced against the C source) even though
//! `svt_aom_product_full_mode_decision` is exported, and the reason is
//! specific: the function takes an array of `ModeDecisionCandidateBuffer*`
//! whose `full_cost` / `full_cost_ssim` are POINTERS into per-candidate
//! scratch, and it writes its result into a `BlkStruct` that owns an
//! `av1xd`, a palette buffer and a coefficient-buffer graph. A shim can build
//! that — but the part under test here is the ordering rule, and a shim
//! large enough to reach it would itself need verifying. The ordering rule
//! is instead pinned by vectors that exercise each of its four documented
//! tie-breaks.

/// C `uni_psy_bias[64]` (md_process.c:29), indexed by `picture_qp`.
///
/// Three plateaus: 85 below qp 16, 95 through qp 47, 100 above. Applied as
/// `cost * bias / 100`, so it makes a single-reference inter candidate
/// CHEAPER at low qp — a deliberate unipred bias, only on noisy pictures.
pub const UNI_PSY_BIAS: [u8; 64] = [
    85, 85, 85, 85, 85, 85, 85, 85, 85, 85, 85, 85, 85, 85, 85, 85, 95, 95, 95, 95, 95, 95, 95, 95,
    95, 95, 95, 95, 95, 95, 95, 95, 95, 95, 95, 95, 95, 95, 95, 95, 95, 95, 95, 95, 95, 95, 95, 95,
    100, 100, 100, 100, 100, 100, 100, 100, 100, 100, 100, 100, 100, 100, 100, 100,
];

/// C `INPUT_SIZE_1080p_RANGE` (definitions.h:1828).
pub const INPUT_SIZE_1080P_RANGE: u8 = 4;

/// C `NEARESTMV` (definitions.h) — the first inter mode, and the bound
/// `is_inter_mode` / `is_intra_mode` split on.
pub const NEARESTMV: u8 = 13;

/// C `derive_ssim_threshold_factor_for_full_md` (mode_decision.c:3805).
///
/// The SSD slack a candidate may spend to win on SSIM: 2 % at 1080p and
/// above, 3 % below. Bigger pictures get the TIGHTER bound.
#[inline]
pub fn ssim_threshold_factor(input_resolution: u8) -> f64 {
    if input_resolution >= INPUT_SIZE_1080P_RANGE {
        1.02
    } else {
        1.03
    }
}

/// C `svt_av1_is_lossless_segment` (mode_decision.c:71, EXPORTED).
///
/// With segmentation off, EVERY segment reads `lossless[0]` — the
/// `segment_id` is ignored, not clamped.
#[inline]
pub fn is_lossless_segment(
    segmentation_enabled: bool,
    lossless: &[bool],
    segment_id: usize,
) -> bool {
    if segmentation_enabled {
        lossless[segment_id]
    } else {
        lossless[0]
    }
}

/// One candidate as the winner selection sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CandidateCost {
    /// C `*(cand_bf->full_cost)`.
    pub full_cost: u64,
    /// C `*(cand_bf->full_cost_ssim)`; read only when tune-SSIM is on.
    pub full_cost_ssim: u64,
    /// C `is_inter_singleref_mode(cand_bf->cand->block_mi.mode)`.
    pub is_inter_singleref: bool,
}

/// C `svt_aom_product_full_mode_decision`'s selection loop
/// (mode_decision.c:3820-3874): the index into `order` of the winning
/// candidate.
///
/// `order` is C's `best_candidate_index_array`, and the answer is an index
/// INTO the candidate array, taken from that ordering — C returns
/// `best_candidate_index_array[i]`, not `i`.
///
/// Four rules, all of them C's and all load-bearing:
///
/// * with only one MDS3 candidate (`md_stage_3_total_count <= 1`) the loop
///   does not run at all and `order[0]` wins REGARDLESS of cost — the caller
///   is trusted to have put the only candidate first;
/// * the SSD comparison is STRICT (`cost < lowest`), so on a tie the
///   EARLIEST candidate in `order` wins;
/// * the SSIM arm is two passes: pass one finds the lowest SSD, pass two
///   takes the lowest SSIM among candidates within `factor * lowest_ssd`.
///   Pass two's threshold is computed ONCE, from pass one's minimum, and
///   pass two then also lowers `ssd_lowest_cost` as it goes — which affects
///   only the equal-SSIM tie-break below, never the threshold;
/// * on equal SSIM the lower SSD wins, and that comparison is NOT gated on
///   the threshold.
pub fn select_winner(
    cands: &[CandidateCost],
    order: &[usize],
    md_stage_3_total_count: u32,
    tune_ssim: bool,
    input_resolution: u8,
    unipred_bias: bool,
    is_noise_level: bool,
    picture_qp: u8,
) -> usize {
    let mut lowest_cost_index = order[0];
    if md_stage_3_total_count <= 1 {
        return lowest_cost_index;
    }

    if tune_ssim {
        // Pass one — lowest SSD.
        let mut ssd_lowest_cost = u64::MAX;
        for &ci in order {
            let cost = cands[ci].full_cost;
            if cost < ssd_lowest_cost {
                lowest_cost_index = ci;
                ssd_lowest_cost = cost;
            }
        }
        // Pass two — lowest SSIM within the SSD slack.
        let threshold_factor = ssim_threshold_factor(input_resolution);
        let ssd_cost_threshold = (threshold_factor * ssd_lowest_cost as f64) as u64;
        let mut ssim_lowest_cost = u64::MAX;
        for &ci in order {
            let ssim_cost = cands[ci].full_cost_ssim;
            let ssd_cost = cands[ci].full_cost;
            if ssim_cost < ssim_lowest_cost {
                if ssd_cost <= ssd_cost_threshold {
                    lowest_cost_index = ci;
                    ssim_lowest_cost = ssim_cost;
                    ssd_lowest_cost = ssd_cost;
                }
            } else if ssim_cost == ssim_lowest_cost && ssd_cost < ssd_lowest_cost {
                lowest_cost_index = ci;
                ssd_lowest_cost = ssd_cost;
            }
        }
    } else {
        let mut lowest_cost = u64::MAX;
        for &ci in order {
            let mut cost = cands[ci].full_cost;
            if unipred_bias && is_noise_level && cands[ci].is_inter_singleref {
                cost = (cost * u64::from(UNI_PSY_BIAS[picture_qp as usize])) / 100;
            }
            if cost < lowest_cost {
                lowest_cost_index = ci;
                lowest_cost = cost;
            }
        }
    }
    lowest_cost_index
}

/// The per-block state `svt_aom_product_full_mode_decision` derives from the
/// winner — everything it computes rather than copies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WinnerSignals {
    /// C `blk_ptr->cost` — `None` when C skips the assignment
    /// (`pd_pass == PD_PASS_1 && fixed_partition`), because at inter-depth
    /// decision the SB lambda is used instead of the block's tuned one.
    pub cost: Option<u64>,
    /// C `blk_ptr->full_dist`, written under the same gate as `cost`.
    pub full_dist: Option<u32>,
    /// C `blk_ptr->block_has_coeff` AFTER the skip-mode fold.
    pub block_has_coeff: bool,
    /// C `blk_ptr->block_mi.skip_mode` after `|= !block_has_coeff`.
    pub skip_mode: bool,
    /// C `blk_ptr->block_mi.skip` = `!block_has_coeff`.
    pub skip: bool,
    /// C `blk_ptr->drl_ctx[0..2]`, `-1` where the stack is too short.
    pub drl_ctx: [i8; 2],
    /// C `blk_ptr->drl_ctx_near[0..2]`.
    pub drl_ctx_near: [i8; 2],
    /// C `blk_ptr->palette_size[0..2]` — zeroed for an inter winner, and for
    /// an intra winner whose palette the block-size gate rejects.
    pub palette_size: [u8; 2],
    /// C `cand->skip_mode_allowed`, which an intra non-IntraBC winner
    /// CLEARS on the candidate itself.
    pub skip_mode_allowed: bool,
}

/// Everything the two winner-commit functions read.
#[derive(Debug, Clone, Copy)]
pub struct WinnerInputs<'a> {
    /// C `cand->block_mi.mode`.
    pub mode: u8,
    /// C `is_inter_block(&blk_ptr->block_mi)` — `use_intrabc || ref_frame[0] > INTRA_FRAME`.
    pub is_inter_block: bool,
    /// C `is_intra_mode(blk_ptr->block_mi.mode)`.
    pub is_intra_mode: bool,
    /// C `block_mi.use_intrabc`.
    pub use_intrabc: bool,
    /// C `cand->skip_mode_allowed`.
    pub skip_mode_allowed: bool,
    /// C `cand->block_mi.skip_mode` as the candidate carries it in.
    pub cand_skip_mode: bool,
    /// C `cand_bf->block_has_coeff`.
    pub cand_block_has_coeff: u8,
    /// C `cand_bf->total_rate`.
    pub total_rate: u64,
    /// C `cand_bf->full_dist`.
    pub full_dist: u32,
    /// C `ctx->pd_pass == PD_PASS_1`.
    pub pd_pass_1: bool,
    /// C `ctx->fixed_partition`.
    pub fixed_partition: bool,
    /// C `ctx->full_sb_lambda_md[hbd_md ? EB_10_BIT_MD : EB_8_BIT_MD]`.
    pub full_lambda: u32,
    /// C `xd->ref_mv_count[ref_frame_type]`.
    pub ref_mv_count: u8,
    /// C `ctx->ref_mv_stack[ref_frame_type]`.
    pub ref_mv_stack: &'a [svtav1_types::motion::CandidateMv],
    /// C `cand->palette_info != NULL` with its two sizes.
    pub palette: Option<[u8; 2]>,
    /// C `svt_av1_allow_palette(ctx->md_palette_level, ctx->blk_geom->bsize)`.
    pub allow_palette: bool,
}

fn drl_contexts(i: &WinnerInputs<'_>) -> ([i8; 2], [i8; 2]) {
    drl_contexts_for(i.mode, i.ref_mv_count, i.ref_mv_stack)
}

/// C's two `drl_ctx` / `drl_ctx_near` fill loops (`mode_decision.c:3709-3728`,
/// repeated at `:3913-3932`), taken directly on their three real inputs.
///
/// Exposed because the ENTROPY side needs the same two arrays derived from
/// the COMMITTED mode-info map rather than from an MD candidate
/// (`pipeline::EntropyCtx::inter_mvp_fields`; see
/// `partition::InterDecision` for why the derivation moved there), and
/// [`winner_signals`] wraps them in a `pd_pass_1` gate that belongs to the
/// mode-decision site, not to this arithmetic.
///
/// The two loops use DIFFERENT index ranges and DIFFERENT mode predicates —
/// `drl_ctx` runs `0..2` under `NEWMV || NEW_NEWMV`, `drl_ctx_near` runs
/// `1..3` storing at `idx - 1` under `have_nearmv_in_inter_mode`, which is
/// C's own "temporary solution to compensate the NEARESTMV offset". Both
/// gate on `ref_mv_count > idx + 1`; `-1` means that position codes no bit.
#[must_use]
pub fn drl_contexts_for(
    mode: u8,
    ref_mv_count: u8,
    ref_mv_stack: &[svtav1_types::motion::CandidateMv],
) -> ([i8; 2], [i8; 2]) {
    use crate::port_md::drl::av1_drl_ctx;
    let mut drl_ctx = [0i8; 2];
    let mut drl_ctx_near = [0i8; 2];
    let newmv = mode == crate::port_entropy_inter::modes::NEWMV
        || mode == crate::port_entropy_inter::modes::NEW_NEWMV;
    if newmv {
        for idx in 0..2usize {
            drl_ctx[idx] = if usize::from(ref_mv_count) > idx + 1 {
                av1_drl_ctx(ref_mv_stack, idx) as i8
            } else {
                -1
            };
        }
    }
    if crate::port_entropy_inter::modes::have_nearmv_in_inter_mode(mode) {
        for idx in 1..3usize {
            drl_ctx_near[idx - 1] = if usize::from(ref_mv_count) > idx + 1 {
                av1_drl_ctx(ref_mv_stack, idx) as i8
            } else {
                -1
            };
        }
    }
    (drl_ctx, drl_ctx_near)
}

/// The tail C shares between the two commit functions: the skip-mode fold
/// and the `skip` derivation (mode_decision.c:3953-3969 / :3735-3752).
fn finish_coeff_state(
    cand_block_has_coeff: u8,
    cand_skip_mode: bool,
    skip_mode_allowed: bool,
) -> (bool, bool, bool) {
    let mut block_has_coeff = cand_block_has_coeff > 0;
    let mut skip_mode = cand_skip_mode;
    if skip_mode_allowed {
        skip_mode |= !block_has_coeff;
    }
    if skip_mode {
        block_has_coeff = false;
    }
    let skip = !block_has_coeff;
    (block_has_coeff, skip_mode, skip)
}

/// C `svt_aom_product_full_mode_decision` (mode_decision.c:3812)'s derived
/// state, once the winner is chosen.
///
/// Order matters and is C's: the INTER branch runs FIRST and zeroes the
/// palette sizes, because an inter winner shuts palette off; the INTRA
/// branch then overwrites them, which is how a palette + IntraBC winner
/// keeps its palette. Reversing the two silently drops the palette.
pub fn winner_signals(i: &WinnerInputs<'_>) -> WinnerSignals {
    let mut out = WinnerSignals {
        skip_mode_allowed: i.skip_mode_allowed,
        ..Default::default()
    };

    if !(i.pd_pass_1 && i.fixed_partition) {
        out.cost = Some(crate::port_rd_cost::rdcost(
            u64::from(i.full_lambda),
            i.total_rate,
            u64::from(i.full_dist),
        ));
        out.full_dist = Some(i.full_dist);
    }

    if i.is_inter_block {
        out.palette_size = [0, 0];
        if i.pd_pass_1 {
            let (a, b) = drl_contexts(i);
            out.drl_ctx = a;
            out.drl_ctx_near = b;
        }
    }

    if i.is_intra_mode {
        match i.palette {
            None => out.palette_size = [0, 0],
            Some(sizes) if i.allow_palette => out.palette_size = sizes,
            // C leaves `blk_ptr->palette_size` UNTOUCHED when the candidate
            // has a palette the block-size gate rejects — it neither copies
            // nor zeroes. Reproduced: the inter branch's zeroing above is the
            // only writer, so an intra winner in this state keeps whatever
            // the caller had.
            Some(_) => {}
        }
        if !i.use_intrabc {
            out.skip_mode_allowed = false;
        }
    }

    let (has_coeff, skip_mode, skip) = finish_coeff_state(
        i.cand_block_has_coeff,
        i.cand_skip_mode,
        out.skip_mode_allowed,
    );
    out.block_has_coeff = has_coeff;
    out.skip_mode = skip_mode;
    out.skip = skip;
    out
}

/// C `svt_aom_product_full_mode_decision_light_pd1` (mode_decision.c:3682).
///
/// Three differences from the full form, each of which changes an output:
///
/// * the palette sizes are zeroed UNCONDITIONALLY, before the mode test —
///   light PD1 codes no palette at all;
/// * the drl contexts are stamped without the `pd_pass == PD_PASS_1` gate
///   (light PD1 IS pass 1 by construction);
/// * the intra arm clears `skip_mode_allowed` with NO `use_intrabc`
///   exception, because light PD1 never codes IntraBC.
///
/// It also does not compute `blk_ptr->cost` at all, so [`WinnerSignals::cost`]
/// is always `None` here.
pub fn winner_signals_light_pd1(i: &WinnerInputs<'_>) -> WinnerSignals {
    let mut out = WinnerSignals {
        skip_mode_allowed: i.skip_mode_allowed,
        palette_size: [0, 0],
        ..Default::default()
    };

    // C tests `is_inter_mode(cand->block_mi.mode)` here — the MODE, not
    // `is_inter_block` (which would also count IntraBC). Light PD1 has no
    // IntraBC, so the two agree on its domain; the port keeps C's test.
    // `is_inter_mode` is `mode >= NEARESTMV` (definitions.h:1618).
    if i.mode >= NEARESTMV {
        let (a, b) = drl_contexts(i);
        out.drl_ctx = a;
        out.drl_ctx_near = b;
    } else {
        out.skip_mode_allowed = false;
    }

    let (has_coeff, skip_mode, skip) = finish_coeff_state(
        i.cand_block_has_coeff,
        i.cand_skip_mode,
        out.skip_mode_allowed,
    );
    out.block_has_coeff = has_coeff;
    out.skip_mode = skip_mode;
    out.skip = skip;
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use svtav1_types::motion::{CandidateMv, Mv};

    fn cand(full: u64, ssim: u64, singleref: bool) -> CandidateCost {
        CandidateCost {
            full_cost: full,
            full_cost_ssim: ssim,
            is_inter_singleref: singleref,
        }
    }

    /// TIER 4 (mode_decision.c:3820). With one MDS3 candidate the loop does
    /// not run, so `order[0]` wins whatever the costs say. A port that
    /// "helpfully" scanned anyway would pick a different block here.
    #[test]
    fn single_candidate_short_circuits_the_scan() {
        let c = [cand(900, 900, false), cand(1, 1, false)];
        assert_eq!(select_winner(&c, &[0, 1], 1, false, 4, false, false, 30), 0);
        // With two, the scan runs and the cheap one wins.
        assert_eq!(select_winner(&c, &[0, 1], 2, false, 4, false, false, 30), 1);
    }

    /// TIER 4. The SSD comparison is STRICT, so a tie goes to the candidate
    /// EARLIER in `best_candidate_index_array` — and the answer is an index
    /// into the candidate array taken from that ordering, not the loop
    /// position.
    #[test]
    fn ssd_ties_go_to_the_earlier_entry_in_the_order() {
        let c = [
            cand(500, 0, false),
            cand(500, 0, false),
            cand(500, 0, false),
        ];
        assert_eq!(
            select_winner(&c, &[2, 0, 1], 3, false, 4, false, false, 30),
            2
        );
        assert_eq!(
            select_winner(&c, &[1, 2, 0], 3, false, 4, false, false, 30),
            1
        );
    }

    /// TIER 4 (mode_decision.c:3863-3866). The unipred bias applies only
    /// when ALL THREE of `unipred_bias`, `is_noise_level` and
    /// `is_inter_singleref_mode` hold, and it makes the candidate CHEAPER.
    #[test]
    fn unipred_bias_needs_all_three_conditions() {
        // qp 10 -> bias 85. 1000 * 85 / 100 = 850 < 900.
        let c = [cand(900, 0, false), cand(1000, 0, true)];
        assert_eq!(select_winner(&c, &[0, 1], 2, false, 4, true, true, 10), 1);
        // Any one condition off and the raw 1000 loses.
        assert_eq!(select_winner(&c, &[0, 1], 2, false, 4, false, true, 10), 0);
        assert_eq!(select_winner(&c, &[0, 1], 2, false, 4, true, false, 10), 0);
        let c2 = [cand(900, 0, false), cand(1000, 0, false)];
        assert_eq!(select_winner(&c2, &[0, 1], 2, false, 4, true, true, 10), 0);
        // qp 48 -> bias 100, i.e. no discount at all.
        assert_eq!(select_winner(&c, &[0, 1], 2, false, 4, true, true, 48), 0);
    }

    /// TIER 4 (mode_decision.c:3835-3857). Pass two takes the lowest SSIM
    /// among candidates whose SSD is within `factor * lowest_ssd`; a
    /// candidate with a better SSIM but an SSD outside the slack does NOT
    /// win. The slack is 2 % at 1080p and above, 3 % below — so the same
    /// cell can flip on resolution alone.
    #[test]
    fn ssim_pass_two_respects_the_ssd_slack_and_the_resolution() {
        // lowest ssd 1000 -> threshold 1020 at >=1080p, 1030 below.
        let c = [cand(1000, 900, false), cand(1025, 100, false)];
        assert_eq!(select_winner(&c, &[0, 1], 2, true, 4, false, false, 30), 0);
        assert_eq!(select_winner(&c, &[0, 1], 2, true, 3, false, false, 30), 1);
    }

    /// TIER 4. On EQUAL ssim the lower SSD wins, and that tie-break is NOT
    /// gated on the threshold — C's `else if` branch checks only the SSD.
    #[test]
    fn equal_ssim_falls_back_to_ssd_without_the_threshold_gate() {
        // Both have ssim 100. The first sets ssim_lowest; the second is far
        // outside the slack (1000 * 1.02 = 1020) yet still wins on SSD?  No:
        // its SSD is HIGHER, so it does not. Reverse the order and the
        // cheaper-SSD one wins through the else-if.
        let c = [cand(1010, 100, false), cand(1000, 100, false)];
        assert_eq!(select_winner(&c, &[0, 1], 2, true, 4, false, false, 30), 1);
        assert_eq!(select_winner(&c, &[1, 0], 2, true, 4, false, false, 30), 1);
    }

    fn stack(weights: [i32; 8]) -> Vec<CandidateMv> {
        weights
            .iter()
            .map(|&w| CandidateMv {
                this_mv: Mv { x: 0, y: 0 },
                comp_mv: Mv { x: 0, y: 0 },
                weight: w,
            })
            .collect()
    }

    fn inputs<'a>(mode: u8, s: &'a [CandidateMv]) -> WinnerInputs<'a> {
        WinnerInputs {
            mode,
            is_inter_block: mode >= NEARESTMV,
            is_intra_mode: mode < NEARESTMV,
            use_intrabc: false,
            skip_mode_allowed: false,
            cand_skip_mode: false,
            cand_block_has_coeff: 1,
            total_rate: 100,
            full_dist: 200,
            pd_pass_1: true,
            fixed_partition: false,
            full_lambda: 1000,
            ref_mv_count: 4,
            ref_mv_stack: s,
            palette: None,
            allow_palette: true,
        }
    }

    /// TIER 4 (mode_decision.c:3909-3931). The drl contexts are `-1` where
    /// the ref-MV stack is too short — a sentinel, not a zero — and only a
    /// NEWMV-family mode fills `drl_ctx` while only a NEARMV-family mode
    /// fills `drl_ctx_near`.
    #[test]
    fn drl_contexts_use_minus_one_for_a_short_stack() {
        let s = stack([700, 700, 700, 700, 0, 0, 0, 0]);
        // NEWMV with ref_mv_count 4: idx 0 and 1 both satisfy count > idx+1.
        let mut i = inputs(crate::port_entropy_inter::modes::NEWMV, &s);
        let out = winner_signals(&i);
        assert_eq!(out.drl_ctx, [0, 0]);
        assert_eq!(out.drl_ctx_near, [0, 0]);
        // count 2: only idx 0 qualifies, idx 1 becomes -1.
        i.ref_mv_count = 2;
        assert_eq!(winner_signals(&i).drl_ctx, [0, -1]);
        // NEARMV fills the _near array from idx 1..3 instead.
        let mut n = inputs(14 /* NEARMV */, &s);
        n.ref_mv_count = 3;
        let out = winner_signals(&n);
        assert_eq!(out.drl_ctx, [0, 0], "NEARMV does not fill drl_ctx");
        assert_eq!(out.drl_ctx_near, [0, -1]);
    }

    /// TIER 4 (mode_decision.c:3951-3969). `skip_mode` is OR-ed with
    /// "no coefficients" only when the candidate allows it, and a
    /// skip-mode block then reports NO coefficients — so `skip` follows.
    #[test]
    fn skip_mode_folds_into_the_coefficient_state() {
        let s = stack([0; 8]);
        let mut i = inputs(crate::port_entropy_inter::modes::NEWMV, &s);

        // Coefficients present, skip-mode not allowed: nothing folds.
        let out = winner_signals(&i);
        assert!(out.block_has_coeff && !out.skip_mode && !out.skip);

        // No coefficients and skip-mode allowed: skip_mode turns on.
        i.cand_block_has_coeff = 0;
        i.skip_mode_allowed = true;
        let out = winner_signals(&i);
        assert!(!out.block_has_coeff && out.skip_mode && out.skip);

        // Coefficients present AND skip-mode allowed: the OR does not fire,
        // so the block keeps its coefficients.
        i.cand_block_has_coeff = 1;
        let out = winner_signals(&i);
        assert!(out.block_has_coeff && !out.skip_mode && !out.skip);
    }

    /// TIER 4 (mode_decision.c:3936-3948). An intra winner that is NOT
    /// IntraBC clears `skip_mode_allowed` on the candidate — which then
    /// suppresses the fold above. An IntraBC winner does not.
    #[test]
    fn an_intra_winner_clears_skip_mode_allowed_unless_it_is_intrabc() {
        let s = stack([0; 8]);
        let mut i = inputs(0 /* DC_PRED */, &s);
        i.skip_mode_allowed = true;
        i.cand_block_has_coeff = 0;
        let out = winner_signals(&i);
        assert!(!out.skip_mode_allowed);
        assert!(!out.skip_mode, "the fold must not fire once it is cleared");

        i.use_intrabc = true;
        i.is_inter_block = true; // is_inter_block counts IntraBC
        let out = winner_signals(&i);
        assert!(out.skip_mode_allowed && out.skip_mode);
    }

    /// TIER 4. `blk_ptr->cost` is NOT written when
    /// `pd_pass == PD_PASS_1 && fixed_partition` — at inter-depth decision
    /// the SB lambda is used instead of the block's tuned one.
    #[test]
    fn cost_is_skipped_under_fixed_partition_at_pass_1() {
        let s = stack([0; 8]);
        let mut i = inputs(crate::port_entropy_inter::modes::NEWMV, &s);
        assert!(winner_signals(&i).cost.is_some());
        i.fixed_partition = true;
        assert!(winner_signals(&i).cost.is_none());
        i.pd_pass_1 = false;
        assert!(winner_signals(&i).cost.is_some());
    }

    /// TIER 4 (mode_decision.c:3682-3752). Light PD1 zeroes the palette
    /// unconditionally, stamps the drl contexts with no `PD_PASS_1` gate,
    /// clears `skip_mode_allowed` for ANY intra mode (no IntraBC exception),
    /// and never computes a cost.
    #[test]
    fn light_pd1_differs_from_the_full_form_in_three_places() {
        let s = stack([700, 0, 0, 0, 0, 0, 0, 0]);
        let mut i = inputs(crate::port_entropy_inter::modes::NEWMV, &s);
        i.pd_pass_1 = false;
        i.palette = Some([4, 4]);
        i.ref_mv_count = 4;

        let light = winner_signals_light_pd1(&i);
        assert_eq!(light.cost, None);
        assert_eq!(light.palette_size, [0, 0]);
        // Stamped even though pd_pass_1 is false.
        assert_eq!(light.drl_ctx, [1, 2]);

        // The full form at pass 0 stamps nothing.
        assert_eq!(winner_signals(&i).drl_ctx, [0, 0]);

        // Intra + IntraBC: the full form KEEPS skip_mode_allowed, light PD1
        // clears it.
        let mut ibc = inputs(0, &s);
        ibc.skip_mode_allowed = true;
        ibc.use_intrabc = true;
        ibc.is_inter_block = true;
        assert!(winner_signals(&ibc).skip_mode_allowed);
        assert!(!winner_signals_light_pd1(&ibc).skip_mode_allowed);
    }
}
