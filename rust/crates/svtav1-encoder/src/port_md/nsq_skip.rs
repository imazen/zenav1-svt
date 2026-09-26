//! The NSQ-shape skip gates of `Source/Lib/Codec/product_coding_loop.c` —
//! the general form, with the inter modes and the AB shapes live.
//!
//! | this module | C |
//! |---|---|
//! | [`Part`] | `Part` (definitions.h:950-961) |
//! | [`skip_by_split_rate`] | `update_skip_nsq_based_on_split_rate` `:9710` |
//! | [`skip_by_sq_recon_dist`] | `update_skip_nsq_based_on_sq_recon_dist` `:9847` |
//! | [`skip_by_shapes`] | `update_skip_nsq_shapes` `:9982` |
//! | [`skip_by_sq_txs`] | `update_skip_nsq_based_on_sq_txs` `:10063` |
//! | [`skip_processing_nsq_block`] | `get_skip_processing_nsq_block` `:10352` |
//! | [`eval_sub_depth_skip_cond1`] | `:10370` |
//! | [`faster_md_settings_nsq`] | `:10401` |
//!
//! # What is NOT already in the port
//!
//! [`crate::depth_refine`] carries four of these specialised to the
//! all-intra funnel. That specialisation stays; this is the general form,
//! and the difference is inter-shaped in three ways:
//!
//! * **`skip_by_sq_recon_dist`'s mode table has an INTER half.** C keys the
//!   threshold on the parent square's prediction mode (`:9867-9895`):
//!   `NEWMV`/`NEW_NEWMV` SHRINK it to 75%, `NEAREST_NEARESTMV`/`NEAR_NEARMV`
//!   double it alongside DC/H/V, and `GLOBALMV`/`GLOBAL_GLOBALMV` shift it
//!   left 2 alongside the directional intra modes. An intra-only port sees
//!   the `default:` arm for all of those — and the 75% arm has no intra
//!   member at all, so it is unreachable without inter.
//! * **The AB shapes.** `PART_HA`/`HB`/`VA`/`VB` are geometry-off at the
//!   still-image presets, so the intra port's shape matches cover only
//!   `H`/`H4`/`V`/`V4`. They are in every shape test here, and
//!   [`skip_by_shapes`]'s `AGGRESSIVE_OFFSET_1` arm exists ONLY for
//!   `PART_HA`/`PART_VB` and has therefore never run.
//! * **[`faster_md_settings_nsq`] is inter-only by construction** — C calls
//!   it under `slice_type != I_SLICE` (`:10933`), so the intra port
//!   documents it as dead rather than implementing it.
//!
//! # Evidence
//!
//! **Tier 4 throughout** — every function here is `static` in C with no
//! exported symbol (`docs/WORKING-ON-THIS.md` §4). The vectors in the test
//! block are hand-derived and traced against the C source, and each one
//! names the C line it came from.
//!
//! # Reachability
//!
//! Nothing calls this yet — the public entry point still refuses inter
//! frames (`docs/WORKING-ON-THIS.md` §7).

use svtav1_types::prediction::PredictionMode;

/// C `Part` — unified: the single definition lives in `svtav1_types::partition`.
pub use svtav1_types::partition::Part;

/// C `NsqSearchCtrls` (md_process.h), the fields these gates read.
///
/// `sq_weight` uses `(uint32_t)~0` as its disabled sentinel, so it is an
/// [`Option`]; the rest use plain 0 for off, which C tests directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NsqSearchCtrls {
    /// `sq_weight` — `None` is C's `(uint32_t)~0`.
    pub sq_weight: Option<u32>,
    /// `hv_weight`
    pub hv_weight: u32,
    /// `max_part0_to_part1_dev` — 0 is off.
    pub max_part0_to_part1_dev: u32,
    /// `nsq_split_cost_th` — 0 is off.
    pub nsq_split_cost_th: u32,
    /// `H_vs_V_split_rate_th` — 0 is off.
    pub h_vs_v_split_rate_th: u32,
    /// `non_HV_split_rate_th` — 0 is off.
    pub non_hv_split_rate_th: u32,
    /// `lower_depth_split_cost_th` — 0 is off.
    pub lower_depth_split_cost_th: u32,
    /// `rate_th_offset_lte16`
    pub rate_th_offset_lte16: u32,
    /// `component_multiple_th` — 0 is off.
    pub component_multiple_th: u64,
    /// `sub_depth_block_lvl` — 0 is off.
    pub sub_depth_block_lvl: u8,
}

/// C `NsqPsqTxsCtrls` (md_process.h), read by [`skip_by_sq_txs`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NsqPsqTxsCtrls {
    pub enabled: bool,
    /// `hv_to_sq_th`
    pub hv_to_sq_th: u32,
    /// `h_to_v_th`
    pub h_to_v_th: u32,
}

/// C `SkipSubDepthCtrls`' two fields [`eval_sub_depth_skip_cond1`] reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkipSubDepthCtrls {
    /// `quad_deviation_th` — C stores this as a `float` and compares a
    /// `float` standard deviation against it, so it is `f32` here.
    pub quad_deviation_th: f32,
    /// `coeff_perc`
    pub coeff_perc: u32,
}

/// C `CONSERVATIVE_OFFSET_0` (definitions.h:269).
pub const CONSERVATIVE_OFFSET_0: i32 = 5;
/// C `AGGRESSIVE_OFFSET_1` (definitions.h:272). NEGATIVE — it makes the
/// weight SMALLER, i.e. the skip more likely, which is why C casts
/// `sq_weight` to `int32_t` before adding it.
pub const AGGRESSIVE_OFFSET_1: i32 = -10;

use svtav1_types::math::rd::rdcost_u64 as rdcost;

// ---------------------------------------------------------------------------
// update_skip_nsq_based_on_split_rate (:9710-9845)
// ---------------------------------------------------------------------------

/// What the parent square contributed, as far as these gates are concerned.
#[derive(Debug, Clone, Copy)]
pub struct ParentSquare {
    /// C `sq_blk_ptr->cost`.
    pub cost: u64,
    /// C `sq_blk_ptr->total_rate`.
    pub total_rate: u64,
    /// C `sq_blk_ptr->full_dist`.
    pub full_dist: u64,
    /// C `sq_blk_ptr->cnt_nz_coeff`.
    pub cnt_nz_coeff: u32,
    /// C `sq_blk_ptr->block_mi.mode`.
    pub mode: PredictionMode,
}

/// C `update_skip_nsq_based_on_split_rate` (`:9710-9845`).
///
/// `partition_rate` is C's `svt_aom_partition_rate_cost(.., p, left_ctx,
/// above_ctx)` for this node with all arguments but the partition type
/// fixed — every one of the five sub-gates calls it with a different
/// partition, and nothing else varies.
///
/// The `sq_size <= 16` adjustments are NOT uniform: the first threshold is
/// REDUCED by the offset (floored at 1, `:9732`) while the other three are
/// INCREASED by it (`:9752`, `:9786`, `:9816`). Folding them into one helper
/// is how a transcription silently inverts three of the four.
pub fn skip_by_split_rate(
    ctrls: &NsqSearchCtrls,
    shape: Part,
    sq: &ParentSquare,
    sq_size: usize,
    best_partition: Part,
    split_flag: bool,
    full_lambda: u64,
    partition_rate: impl Fn(Part) -> u64,
) -> bool {
    if shape == Part::N {
        return false;
    }

    if ctrls.nsq_split_cost_th != 0 {
        let th = if sq_size <= 16 {
            u64::from(
                ctrls
                    .nsq_split_cost_th
                    .saturating_sub(ctrls.rate_th_offset_lte16)
                    .max(1),
            )
        } else {
            u64::from(ctrls.nsq_split_cost_th)
        };
        let part_cost = rdcost(full_lambda, partition_rate(shape), 0);
        if part_cost * 1000 > sq.cost * th {
            return true;
        }
    }

    if ctrls.h_vs_v_split_rate_th != 0 && matches!(shape, Part::H | Part::V) {
        let th = u64::from(if sq_size <= 16 {
            ctrls.h_vs_v_split_rate_th + ctrls.rate_th_offset_lte16
        } else {
            ctrls.h_vs_v_split_rate_th
        });
        let h_cost = rdcost(full_lambda, partition_rate(Part::H), 0);
        let v_cost = rdcost(full_lambda, partition_rate(Part::V), 0);
        // Only the two plain rect shapes reach here, and each is compared
        // against the OTHER one.
        let (mine, theirs) = if shape == Part::H {
            (h_cost, v_cost)
        } else {
            (v_cost, h_cost)
        };
        if mine * th > theirs * 100 {
            return true;
        }
    }

    if ctrls.non_hv_split_rate_th != 0 && !matches!(shape, Part::H | Part::V) {
        let th = u64::from(if sq_size <= 16 {
            ctrls.non_hv_split_rate_th + ctrls.rate_th_offset_lte16
        } else {
            ctrls.non_hv_split_rate_th
        });
        let part_cost = rdcost(full_lambda, partition_rate(shape), 0);
        let best_cost = rdcost(full_lambda, partition_rate(best_partition), 0);
        if part_cost * th > best_cost * 100 {
            return true;
        }
    }

    if ctrls.lower_depth_split_cost_th != 0 && split_flag {
        let th = u64::from(if sq_size <= 16 {
            ctrls.lower_depth_split_cost_th + ctrls.rate_th_offset_lte16
        } else {
            ctrls.lower_depth_split_cost_th
        });
        let split_cost = rdcost(full_lambda, partition_rate(Part::S), 0);
        if split_cost * 10000 < sq.cost * th {
            return true;
        }
    }

    if ctrls.component_multiple_th != 0 {
        let rate_cost = rdcost(full_lambda, sq.total_rate, 0);
        let dist_cost = rdcost(full_lambda, 0, sq.full_dist);
        if rate_cost.max(dist_cost) > ctrls.component_multiple_th * rate_cost.min(dist_cost) {
            return true;
        }
    }

    false
}

// ---------------------------------------------------------------------------
// update_skip_nsq_based_on_sq_recon_dist (:9847-9968)
// ---------------------------------------------------------------------------

/// C's parent-mode threshold modulation (`:9867-9895`).
///
/// The `* 75 / 100` arm is INTER-ONLY (`NEWMV`, `NEW_NEWMV`) and therefore
/// unreachable on an intra frame; the `* 2` and `<< 2` arms each gained
/// inter members that an intra-only table would drop into `default`.
///
/// **This is the REFERENCE copy of C's switch.** `depth_refine`'s live NSQ
/// funnel has its own transcription (`DepthWalk::nsq_dev_by_parent_mode`,
/// which takes C's `block_mi.mode` as a raw `u8`); the two are PINNED to each
/// other over all 25 modes by
/// `crate::depth_refine::nsq_mode_table_tests::depth_refine_table_agrees_with_port_md_nsq_skip`,
/// per `docs/WORKING-ON-THIS.md` §4's rule for a second transcription. Change
/// one and the pin fails.
pub(crate) fn modulate_by_parent_mode(dev: u32, mode: PredictionMode) -> u32 {
    use PredictionMode as M;
    match mode {
        M::NewMv | M::NewNewMv => (dev * 75) / 100,
        M::DcPred | M::HPred | M::VPred | M::NearestNearestMv | M::NearNearMv => dev * 2,
        M::D45Pred
        | M::D135Pred
        | M::D113Pred
        | M::D157Pred
        | M::D203Pred
        | M::D67Pred
        | M::SmoothPred
        | M::SmoothHPred
        | M::SmoothVPred
        | M::PaethPred
        | M::GlobalMv
        | M::GlobalGlobalMv => dev << 2,
        _ => dev,
    }
}

/// C's `(ABS(a - b) * 100) / MIN(a, b)` deviation (`:9917` and friends),
/// on values C has already floored at 1 so the division is safe.
#[inline]
fn pct_deviation(a: u64, b: u64) -> u32 {
    ((a.abs_diff(b) * 100) / a.min(b)) as u32
}

/// C `update_skip_nsq_based_on_sq_recon_dist` (`:9847-9968`).
///
/// `rec_dist_per_quadrant` is C's `ctx->rec_dist_per_quadrant[0..4]` in
/// raster order (top-left, top-right, bottom-left, bottom-right).
///
/// Two transcription hazards, both live:
///
/// * The final threshold assignment (`:9925-9929` and its V twin) OVERWRITES
///   the accumulated value with `dist_cost_ratio` when the ratio exceeds
///   `max_ratio` — it is not a clamp, and `modulated_th` is only used in the
///   middle band. `modulated_th` itself underflows for a ratio below
///   `min_ratio`, which is harmless ONLY because that band takes the `0`
///   arm; the port computes it as an `Option` so the dead value can never
///   be read.
/// * The horizontal arm groups quadrants as `(q0+q1)` vs `(q2+q3)` and the
///   vertical arm as `(q0+q2)` vs `(q1+q3)`; the inner quadrant deviations
///   pair `(q0,q1)`/`(q2,q3)` and `(q0,q2)`/`(q1,q3)` respectively. Swapping
///   either pairing gives a plausible number on most content.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn skip_by_sq_recon_dist(
    ctrls: &NsqSearchCtrls,
    shape: Part,
    sq: &ParentSquare,
    rec_dist_per_quadrant: [u64; 4],
    full_lambda: u64,
) -> bool {
    if shape == Part::N || ctrls.max_part0_to_part1_dev == 0 || sq.cost == 0 {
        return false;
    }

    let dist = rdcost(full_lambda, 0, sq.full_dist);
    let dist_cost_ratio = (dist * 100) / sq.cost;
    const MIN_RATIO: u64 = 50;
    const MAX_RATIO: u64 = 100;
    // C evaluates this unconditionally and underflows below MIN_RATIO; the
    // only band that READS it is `MIN_RATIO < ratio <= MAX_RATIO`.
    let modulated_th = dist_cost_ratio
        .checked_sub(MIN_RATIO)
        .map(|over| (100 * over) / (MAX_RATIO - MIN_RATIO));

    let base = modulate_by_parent_mode(ctrls.max_part0_to_part1_dev, sq.mode);
    let q = rec_dist_per_quadrant.map(|d| d.max(1));

    // The two arms are the same shape with different quadrant pairings and
    // different mode exceptions, so they share one closure.
    let arm = |mut th: u32, group_a: u64, group_b: u64, inner_a: u32, inner_b: u32| -> bool {
        th += ((u64::from(th) * u64::from(inner_a.min(inner_b))) / 100) as u32;
        let th = if dist_cost_ratio <= MIN_RATIO {
            0
        } else if dist_cost_ratio <= MAX_RATIO {
            ((u64::from(th) * modulated_th.expect("ratio > MIN_RATIO in this band")) / 100) as u32
        } else {
            dist_cost_ratio as u32
        };
        pct_deviation(group_a, group_b) < th
    };

    if shape.is_horizontal() {
        // `:9899-9904`: the H path is hurt by a vertically-structured
        // parent, so V / D67 / D113 / D45 / D135 relax it and H kills it.
        use PredictionMode as M;
        let th = match sq.mode {
            M::VPred | M::D67Pred | M::D113Pred | M::D45Pred | M::D135Pred => base << 2,
            M::HPred => 0,
            _ => base,
        };
        if arm(
            th,
            q[0] + q[1],
            q[2] + q[3],
            pct_deviation(q[0], q[1]),
            pct_deviation(q[2], q[3]),
        ) {
            return true;
        }
    }

    if shape.is_vertical() {
        use PredictionMode as M;
        let th = match sq.mode {
            M::HPred | M::D157Pred | M::D203Pred | M::D45Pred | M::D135Pred => base << 2,
            M::VPred => 0,
            _ => base,
        };
        if arm(
            th,
            q[0] + q[2],
            q[1] + q[3],
            pct_deviation(q[0], q[2]),
            pct_deviation(q[1], q[3]),
        ) {
            return true;
        }
    }

    false
}

// ---------------------------------------------------------------------------
// update_skip_nsq_shapes (:9982-10061)
// ---------------------------------------------------------------------------

/// One rect partition's two halves as [`skip_by_shapes`] sees them.
#[derive(Debug, Clone, Copy)]
pub struct RectHalves {
    /// `block_data[PART_H|V][0]->cost` and `[1]->cost`.
    pub costs: [u64; 2],
    /// `block_data[PART_H|V][0]->block_has_coeff` and `[1]`.
    pub has_coeff: [bool; 2],
}

/// C `update_skip_nsq_shapes` (`:9982-10061`).
///
/// Skips an AB or 4-way shape when the corresponding rect partition already
/// costs more than a weighted fraction of the square — and, failing that,
/// when it costs more than a weighted fraction of the OTHER rect.
///
/// `h`/`v` are `None` when C's `tested_blk[PART_H|V][0..2]` are not all set;
/// C then skips the whole block, which is not the same as deciding "do not
/// skip" for the H/V cross-check that follows.
///
/// The `AGGRESSIVE_OFFSET_1` arm (`:10002-10009`) applies to `PART_HA` from
/// the FIRST half's coefficients and to `PART_HB` from the SECOND's — an
/// asymmetry that mirrors which half each shape actually subdivides, and one
/// that has never run in this port because the AB shapes are geometry-off at
/// every still-image preset.
#[must_use]
pub fn skip_by_shapes(
    ctrls: &NsqSearchCtrls,
    shape: Part,
    sq_cost: Option<u64>,
    h: Option<RectHalves>,
    v: Option<RectHalves>,
) -> bool {
    let Some(sq_weight) = ctrls.sq_weight else {
        return false;
    };
    let mut sq_weight = sq_weight as i32;
    if matches!(shape, Part::H4 | Part::V4) {
        sq_weight += CONSERVATIVE_OFFSET_0;
    }

    // The H family reads H as "mine" and V as "the other"; the V family
    // mirrors it. Everything else is identical, so it is written once.
    let (mine, other, aggressive_half) = match shape {
        Part::Ha => (h, v, Some(0usize)),
        Part::Hb => (h, v, Some(1usize)),
        Part::H4 => (h, v, None),
        Part::Va => (v, h, Some(0usize)),
        Part::Vb => (v, h, Some(1usize)),
        Part::V4 => (v, h, None),
        _ => return false,
    };

    let (Some(sq_cost), Some(mine)) = (sq_cost, mine) else {
        return false;
    };
    if let Some(half) = aggressive_half
        && !mine.has_coeff[half]
    {
        sq_weight += AGGRESSIVE_OFFSET_1;
    }
    // C keeps `sq_weight` in a `uint32_t` and only casts to `int32_t` for
    // the addition, so a weight driven below zero by the aggressive offset
    // wraps to a colossal unsigned value and the gate never fires. Every
    // shipped `sq_weight` is far above 10; the saturation here is the
    // reachable half of that behaviour, stated rather than hidden.
    let sq_weight = u64::from(sq_weight.max(0) as u32);

    let my_cost = mine.costs[0] + mine.costs[1];
    if my_cost > (sq_cost * sq_weight) / 100 {
        return true;
    }
    let Some(other) = other else {
        return false;
    };
    let other_cost = other.costs[0] + other.costs[1];
    my_cost > (other_cost * u64::from(ctrls.hv_weight)) / 100
}

// ---------------------------------------------------------------------------
// update_skip_nsq_based_on_sq_txs (:10063-10099)
// ---------------------------------------------------------------------------

/// C `update_skip_nsq_based_on_sq_txs` (`:10063-10099`).
///
/// `min_nz_hv` is `(ctx->min_nz_h, ctx->min_nz_v)` — the minimum nonzero
/// coefficient counts the parent square's non-normative H and V transform
/// splits produced. `None` is C's pair of `(uint16_t)~0` sentinels, meaning
/// the parent kept no coefficients so the split was never measured.
///
/// Both counts are DOUBLED before comparison (`:10077-10078`): they are
/// per-half minima and the square's `cnt_nz_coeff` covers the whole block.
#[must_use]
pub fn skip_by_sq_txs(
    ctrls: &NsqPsqTxsCtrls,
    shape: Part,
    sq: &ParentSquare,
    min_nz_hv: Option<(u16, u16)>,
) -> bool {
    if shape == Part::N || !ctrls.enabled {
        return false;
    }
    let Some((nz_h, nz_v)) = min_nz_hv else {
        return false;
    };
    let cnt_h_best = u64::from(nz_h) << 1;
    let cnt_v_best = u64::from(nz_v) << 1;
    let cnt_nz = u64::from(sq.cnt_nz_coeff);
    let hv_to_sq = (cnt_nz * u64::from(ctrls.hv_to_sq_th)) / 100;
    let h_to_v = (cnt_nz * u64::from(ctrls.h_to_v_th)) / 100;

    if cnt_h_best >= hv_to_sq && cnt_v_best >= hv_to_sq {
        return true;
    }
    if shape.is_horizontal() && cnt_v_best <= cnt_h_best && cnt_h_best >= h_to_v {
        return true;
    }
    if shape.is_vertical() && cnt_h_best <= cnt_v_best && cnt_v_best >= h_to_v {
        return true;
    }
    false
}

// ---------------------------------------------------------------------------
// get_skip_processing_nsq_block (:10352-10368)
// ---------------------------------------------------------------------------

/// Which gate fired, so a caller (or a bisect harness) can say why a shape
/// was dropped instead of only that it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NsqSkipReason {
    SplitRate,
    SqTxs,
    SqReconDist,
    Shapes,
}

/// C `get_skip_processing_nsq_block` (`:10352-10368`).
///
/// The ORDER is C's and is load-bearing for anything that reports a reason:
/// split-rate, then TX-split counts, then recon distortion, then relative
/// shape cost.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn skip_processing_nsq_block(
    ctrls: &NsqSearchCtrls,
    txs_ctrls: &NsqPsqTxsCtrls,
    shape: Part,
    sq: &ParentSquare,
    sq_size: usize,
    best_partition: Part,
    split_flag: bool,
    full_lambda: u64,
    rec_dist_per_quadrant: [u64; 4],
    min_nz_hv: Option<(u16, u16)>,
    h: Option<RectHalves>,
    v: Option<RectHalves>,
    partition_rate: impl Fn(Part) -> u64,
) -> Option<NsqSkipReason> {
    if skip_by_split_rate(
        ctrls,
        shape,
        sq,
        sq_size,
        best_partition,
        split_flag,
        full_lambda,
        partition_rate,
    ) {
        return Some(NsqSkipReason::SplitRate);
    }
    if skip_by_sq_txs(txs_ctrls, shape, sq, min_nz_hv) {
        return Some(NsqSkipReason::SqTxs);
    }
    if skip_by_sq_recon_dist(ctrls, shape, sq, rec_dist_per_quadrant, full_lambda) {
        return Some(NsqSkipReason::SqReconDist);
    }
    if skip_by_shapes(ctrls, shape, Some(sq.cost), h, v) {
        return Some(NsqSkipReason::Shapes);
    }
    None
}

// ---------------------------------------------------------------------------
// eval_sub_depth_skip_cond1 (:10370-10398)
// ---------------------------------------------------------------------------

/// C `eval_sub_depth_skip_cond1` (`:10370-10398`).
///
/// True when the four quadrant distortions are UNIFORM (low standard
/// deviation) and the block coded few coefficients — a block with nothing
/// interesting in any quadrant.
///
/// **The arithmetic is `float`, not `double`, and that is the contract.**
/// C accumulates `sum` and `sum1` in `float`, divides by an `(float)n`, and
/// takes `sqrtf`. Only the squaring detours through `double` (`pow(x, 2)`
/// returns `double` and is immediately assigned into a `float`), which
/// rounds the same as an `f32` multiply for these magnitudes but is
/// transcribed as written. Promoting the accumulators to `f64` would change
/// the comparison against `quad_deviation_th` on near-ties.
#[must_use]
pub fn eval_sub_depth_skip_cond1(
    ctrls: &SkipSubDepthCtrls,
    rec_dist_per_quadrant: [u64; 4],
    cnt_nz_coeff: u32,
    sq_size: usize,
) -> bool {
    let n = 4usize;
    let mut sum = 0f32;
    for q in rec_dist_per_quadrant {
        sum += q as f32;
    }
    let average = sum / n as f32;
    let mut sum1 = 0f32;
    for q in rec_dist_per_quadrant {
        sum1 += ((f64::from(q as f32 - average)).powi(2)) as f32;
    }
    let std_deviation = (sum1 / n as f32).sqrt();

    let total_samples = (sq_size * sq_size) as u32;
    let coeff_perc = (cnt_nz_coeff * 100) / total_samples;

    std_deviation < ctrls.quad_deviation_th && coeff_perc < ctrls.coeff_perc
}

// ---------------------------------------------------------------------------
// faster_md_settings_nsq (:10401-10422)
// ---------------------------------------------------------------------------

/// The settings [`faster_md_settings_nsq`] may change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NsqFasterSettings {
    /// C `ctx->global_mv_injection`.
    pub global_mv_injection: bool,
    /// The (possibly tightened) search controls.
    pub ctrls: NsqSearchCtrls,
    /// C `ctx->params_status` — set when anything above changed, so the
    /// next square restores the per-picture derivation.
    pub params_status: bool,
}

/// C `faster_md_settings_nsq` (`:10401-10422`). **Inter-only** — C guards
/// the call site with `slice_type != I_SLICE` (`:10933`).
///
/// Two independent tightenings:
///
/// * Global-motion injection is switched OFF for the NSQ shapes when the
///   parent square did NOT pick a global-MV mode — there is no reason to
///   price a global candidate under a parent that rejected one.
/// * At `sub_depth_block_lvl` on a PD1 CHILD block, four thresholds are
///   pulled toward "skip more": `sq_weight` and `nsq_split_cost_th` are
///   capped (`MIN`) while `H_vs_V` and `non_HV` are floored (`MAX`). The
///   opposite direction on the two pairs is deliberate — a bigger
///   `sq_weight` skips LESS and a bigger `H_vs_V_split_rate_th` skips MORE.
#[must_use]
pub fn faster_md_settings_nsq(
    gm_enabled_with_inj_psq_glb: bool,
    parent_square_mode: Option<PredictionMode>,
    is_pd_pass_1: bool,
    is_child: bool,
    mut ctrls: NsqSearchCtrls,
) -> NsqFasterSettings {
    let mut global_mv_injection = true;
    let mut params_status = false;

    if gm_enabled_with_inj_psq_glb
        && let Some(mode) = parent_square_mode
        && !matches!(
            mode,
            PredictionMode::GlobalGlobalMv | PredictionMode::GlobalMv
        )
    {
        params_status = true;
        global_mv_injection = false;
    }

    if ctrls.sub_depth_block_lvl != 0 && is_pd_pass_1 && is_child {
        ctrls.sq_weight = Some(ctrls.sq_weight.map_or(85, |w| w.min(85)));
        ctrls.nsq_split_cost_th = ctrls.nsq_split_cost_th.min(60);
        ctrls.h_vs_v_split_rate_th = ctrls.h_vs_v_split_rate_th.max(60);
        ctrls.non_hv_split_rate_th = ctrls.non_hv_split_rate_th.max(60);
        params_status = true;
    }

    NsqFasterSettings {
        global_mv_injection,
        ctrls,
        params_status,
    }
}

#[cfg(test)]
mod tests;

// ---------------------------------------------------------------------------
// update_redundant, predicate half (:10267-10294)
// ---------------------------------------------------------------------------

/// C `update_redundant`'s shape-matching rule (`:10275-10294`).
///
/// Some NSQ shapes' FIRST sub-block is geometrically identical to a
/// sub-block of a shape already tested, so its whole mode decision can be
/// copied instead of re-derived. `Some((shape, nsi))` names the source.
///
/// The rest of `update_redundant` (`:10296-10349`) copies the block data,
/// the recon and the coefficients — buffer plumbing this port structures
/// differently, and not translated here.
///
/// **`PART_VA`'s source is `PART_HA`, not `PART_V`.** That looks like a
/// typo and is not: `HA` splits the TOP half and `VA` splits the LEFT half,
/// so both of their first sub-blocks are the same top-left quarter square.
/// `HB`/`VB`, by contrast, take the full-width / full-height rect from
/// `H`/`V`. Also note `PART_HA` itself has NO source — nothing tested
/// earlier shares its first sub-block.
///
/// Squares are excluded even when they would match (`:10275`), and C says
/// why: an SQ block carries per-quadrant recon statistics
/// (`rec_dist_per_quadrant`) that are computed from samples, not copied, so
/// reusing the record would leave later decisions reading data that was
/// never produced.
#[must_use]
pub fn redundant_shape_source(
    shape: Part,
    nsi: usize,
    tested: impl Fn(Part) -> bool,
) -> Option<Part> {
    if shape == Part::N || nsi != 0 {
        return None;
    }
    let source = match shape {
        Part::Hb => Part::H,
        Part::Vb => Part::V,
        Part::Va => Part::Ha,
        _ => return None,
    };
    tested(source).then_some(source)
}

#[cfg(test)]
mod redundant_tests {
    use super::*;

    /// `:10283-10289` — three shapes have a source, and VA's is HA.
    #[test]
    fn the_redundancy_map_pairs_va_with_ha() {
        let all = |_: Part| true;
        assert_eq!(redundant_shape_source(Part::Hb, 0, all), Some(Part::H));
        assert_eq!(redundant_shape_source(Part::Vb, 0, all), Some(Part::V));
        assert_eq!(
            redundant_shape_source(Part::Va, 0, all),
            Some(Part::Ha),
            "VA's first sub-block is HA's, not V's"
        );
        for s in [Part::N, Part::H, Part::V, Part::H4, Part::V4, Part::Ha] {
            assert_eq!(redundant_shape_source(s, 0, all), None, "{s:?}");
        }
    }

    /// `:10283` — only the FIRST sub-block of a shape can be redundant.
    #[test]
    fn only_the_first_sub_block_is_redundant() {
        let all = |_: Part| true;
        assert_eq!(redundant_shape_source(Part::Hb, 1, all), None);
    }

    /// `:10283` — an untested source yields nothing.
    #[test]
    fn an_untested_source_is_not_a_source() {
        assert_eq!(redundant_shape_source(Part::Hb, 0, |_| false), None);
        // And the map is consulted by SOURCE, not by the shape under test.
        assert_eq!(
            redundant_shape_source(Part::Va, 0, |p| p == Part::V),
            None,
            "V being tested does not make VA redundant"
        );
    }
}
