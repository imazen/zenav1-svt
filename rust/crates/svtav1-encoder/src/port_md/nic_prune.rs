//! The MD-stage candidate STAGING of `Source/Lib/Codec/product_coding_loop.c`:
//! how many candidates of each class survive MDS0 -> MDS1 -> MDS2 -> MDS3, and
//! in what order.
//!
//! | this module | C |
//! |---|---|
//! | [`sort_fast_cost_based_candidates`] | `:1415-1436` (`static`) |
//! | [`sort_full_cost_based_candidates`] | `:1438-1452` (**EXPORTED**) |
//! | [`construct_best_sorted_arrays_md_stage_3`] | `:1454-1466` (`static`) |
//! | [`post_mds0_nic_pruning`] | `:7819-7882` (`static`) |
//! | [`post_mds1_nic_pruning`] | `:7885-7960` (`static`) |
//! | [`post_mds2_nic_pruning`] | `:7963-8023` (`static`) |
//!
//! # Why this exists when `leaf_funnel::nic` already prunes
//!
//! [`crate::leaf_funnel::nic`] carries the SAME C functions, and as of
//! 2026-09-03 it carries them for BOTH slice types — the three differences
//! this header used to list as reasons the funnel "cannot serve an inter
//! frame" are the three defects §1z33 fixed there, not a division of labour:
//!
//! * `mds1_class_th` and `mds2_class_th` are forced to the disabled sentinel
//!   on an I-slice (`:7826`, `:7897`) and are LIVE on a P/B slice, where the
//!   class-kill (`dev >= class_th` -> the whole class drops to zero
//!   candidates) and the band reduction both fire.
//! * `post_mds0` splits its candidate threshold by class
//!   (`mds1_cand_base_th_intra` vs `..._inter`, `:7841`).
//! * `post_mds2`'s re-floor `MAX(25, scaled * i_mds3_class_th_mult)`
//!   (`:7978`) is I-slice-ONLY.
//!
//! What is still only here is the SHAPE: this is the general five-class form
//! with an explicit `CandClass` and no funnel types in its signature, pure
//! (costs in, counts out) and tier-1 on its one exported C function. The
//! funnel's copy is a lane-indexed specialisation inside `evaluate_leaf`.
//!
//! **Two implementations of one C function is a standing hazard** — the
//! inverted [`CandClass`] naming this module carried until 2026-09-03 is
//! exactly what it looks like when they drift. Whichever survives, they must
//! not both be edited independently again.
//!
//! # Evidence
//!
//! [`sort_full_cost_based_candidates`] is **tier 1** —
//! `tests/c_parity_pcl_nic.rs` drives the real exported
//! `sort_full_cost_based_candidates` through `shims/pcl_shims.c`. The other
//! five functions are `static` in C with no symbol and are **tier 4**:
//! hand-derived vectors traced against the C source
//! (`docs/WORKING-ON-THIS.md` section 4).
//!
//! # Reachability
//!
//! **Nothing calls this.** Re-verified 2026-09-03: the only references to
//! `nic_prune` outside itself are [`super::md_stages`] (which nothing calls
//! either) and `tests/c_parity_pcl_nic.rs`. The live NIC staging on every
//! path this encoder takes is `leaf_funnel::nic`, reached from
//! `leaf_funnel::evaluate_leaf`. Per `docs/WORKING-ON-THIS.md` section 7 a
//! faithful translation with no caller stays translated and states its
//! reachability — this is that statement, and it is also the reason the
//! inter-frame class prunes had to be fixed in the funnel: a correct port
//! sitting beside the live path fixes nothing.

/// C `CAND_CLASS_TOTAL` (definitions.h:792).
pub const CAND_CLASS_TOTAL: usize = 5;

/// C `CandClass` (definitions.h:787-793).
///
/// C names these `CAND_CLASS_0..4` and comments only that classes 0/3/4 are
/// "intra". What they actually hold is fixed by the injector loop
/// (`mode_decision.c:3645-3671`): 0 regular intra, **1 the MVP inter modes —
/// every inter mode EXCEPT `NEWMV` / `NEW_NEWMV` — and 2 `NEWMV` /
/// `NEW_NEWMV`** (C's own comments there read "MV Prediction" for class 2 and
/// "MVP Prediction" for class 1), 3 palette, 4 IntraBC. The numeric identity
/// is load-bearing (`MD_STAGE_NICS` is indexed by it), hence `#[repr(u8)]`
/// and the explicit discriminants rather than a re-ordered "nicer" enum.
///
/// CORRECTED 2026-09-03: this enum previously named 1 `InterNew` and 2
/// `InterOther`, i.e. the two inter classes the wrong way round. The
/// discriminants were right and nothing calls this module, so it never
/// produced a wrong number — but `leaf_funnel::nic::lane_of` (which IS live)
/// disagreed with it, and one of the two had to be wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum CandClass {
    /// `CAND_CLASS_0` — regular intra.
    Intra = 0,
    /// `CAND_CLASS_1` — the MVP inter modes (everything but `NEWMV` /
    /// `NEW_NEWMV`).
    InterMvp = 1,
    /// `CAND_CLASS_2` — `NEWMV` / `NEW_NEWMV`, plus every inter mode when
    /// C's `merge_inter_cands` fires.
    InterNew = 2,
    /// `CAND_CLASS_3` — palette.
    Palette = 3,
    /// `CAND_CLASS_4` — IntraBC.
    IntraBc = 4,
}

impl CandClass {
    /// Class iteration order, which is C's `for (cidx = CAND_CLASS_0; ...)`
    /// and is load-bearing for [`construct_best_sorted_arrays_md_stage_3`].
    pub const ALL: [CandClass; CAND_CLASS_TOTAL] = [
        CandClass::Intra,
        CandClass::InterMvp,
        CandClass::InterNew,
        CandClass::Palette,
        CandClass::IntraBc,
    ];

    /// C `is_intra_class` (definitions.h:805-807).
    #[must_use]
    #[inline]
    pub fn is_intra(self) -> bool {
        matches!(
            self,
            CandClass::Intra | CandClass::Palette | CandClass::IntraBc
        )
    }

    /// Index into the per-class arrays.
    #[must_use]
    #[inline]
    pub fn index(self) -> usize {
        self as usize
    }
}

/// C `NicPruningCtrls` (md_process.h:459-514).
///
/// Every `uint64_t` field C can set to the `(uint64_t)~0` "disabled"
/// sentinel is an [`Option`] here — that is what the sentinel means and the
/// comparisons in C are all against it. The arithmetic inside the prunes
/// still runs on `u64::MAX` for a `None` (see [`Threshold::raw`]), because C
/// does not branch around the divisions: it divides *by* the saturated
/// threshold and relies on the quotient being large. Collapsing `None` to
/// "no limit" would agree with C on every reachable input but is not the
/// same expression, so it is not what this does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NicPruningCtrls {
    /// `mds1_class_th` — class threshold after MDS0. Ignored on intra frames.
    pub mds1_class_th: Option<u64>,
    /// `mds1_band_cnt`
    pub mds1_band_cnt: u8,
    /// `mds2_class_th` — class threshold after MDS1. Ignored on intra frames.
    pub mds2_class_th: Option<u64>,
    /// `mds2_band_cnt`
    pub mds2_band_cnt: u8,
    /// `mds3_class_th` — class threshold after MDS2. LIVE on intra frames.
    pub mds3_class_th: Option<u64>,
    /// `i_mds3_class_th_mult` — multiplier applied to `mds3_class_th` on
    /// intra frames only.
    pub i_mds3_class_th_mult: u8,
    /// `mds3_band_cnt`
    pub mds3_band_cnt: u8,
    /// `mds1_cand_base_th_intra`
    pub mds1_cand_base_th_intra: Option<u64>,
    /// `mds1_cand_base_th_inter`
    pub mds1_cand_base_th_inter: Option<u64>,
    /// `mds1_cand_th_rank_factor` — 0 is off.
    pub mds1_cand_th_rank_factor: u16,
    /// `mds2_cand_base_th`
    pub mds2_cand_base_th: Option<u64>,
    /// `mds2_cand_th_rank_factor` — 0 is off.
    pub mds2_cand_th_rank_factor: u16,
    /// `mds2_relative_dev_th` — 0 is off.
    pub mds2_relative_dev_th: u16,
    /// `mds3_cand_base_th`
    pub mds3_cand_base_th: Option<u64>,
    /// `enable_skipping_mds1`
    pub enable_skipping_mds1: bool,
    /// `merge_inter_cands_mult` — read by candidate injection, not by these
    /// prunes; carried so the struct mirrors C's.
    pub merge_inter_cands_mult: u8,
}

/// A `NicPruningCtrls` threshold resolved for one prune: the qp-scaled value,
/// or the saturated sentinel when C left it disabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Threshold(Option<u64>);

impl Threshold {
    /// C's `(x == (uint64_t)~0) ? x : DIVIDE_AND_ROUND(x * q_weight, q_denom)`.
    fn scaled(base: Option<u64>, q_weight: u32, q_denom: u32) -> Self {
        Threshold(base.map(|b| div_round(b * u64::from(q_weight), u64::from(q_denom))))
    }

    /// The value C's arithmetic actually uses: `(uint64_t)~0` when disabled.
    #[inline]
    fn raw(self) -> u64 {
        self.0.unwrap_or(u64::MAX)
    }

    /// C's `!= (uint64_t)~0` test.
    #[inline]
    fn is_enabled(self) -> bool {
        self.0.is_some()
    }
}

/// C `DIVIDE_AND_ROUND` (utility.h:96): `(x + (y >> 1)) / y`.
///
/// Kept private to this module rather than shared with
/// [`super::nics`]: the two files are edited by different lanes and a
/// three-line arithmetic helper is not worth the coupling.
#[inline]
fn div_round(x: u64, y: u64) -> u64 {
    (x + (y >> 1)) / y
}

/// C's `mdsx_cand_th / (rank_factor ? rank_factor * cand_count : 1)`
/// (`:7869`, `:7945`).
///
/// C computes the divisor as `uint16 * uint32` -> `uint32`; with
/// `cand_count <= MD_STAGE_NICS` (64) and the shipped rank factors
/// (single digits) that product cannot approach `u32::MAX`, so widening it
/// to `u64` here is value-identical and removes the only overflow question.
#[inline]
fn rank_scaled(th: u64, rank_factor: u16, cand_count: u32) -> u64 {
    let divisor = if rank_factor != 0 {
        u64::from(rank_factor) * u64::from(cand_count)
    } else {
        1
    };
    th / divisor
}

/// The band reduction shared by all three prunes (`:7858`, `:7921`, `:8004`).
///
/// C casts `dev * (band_cnt - 1) / class_th` to `uint8_t`. The cast cannot
/// truncate at this call site: the caller has already taken the
/// `dev >= class_th` exit, so the quotient is `< band_cnt - 1 <= 254`.
#[inline]
fn band_reduce(count: u32, dev: u64, band_cnt: u8, class_th: u64) -> u32 {
    if band_cnt >= 3 && count > 1 {
        let band_idx = dev * u64::from(band_cnt - 1) / class_th;
        div_round(u64::from(count), band_idx + 1) as u32
    } else {
        count
    }
}

/// The class-level deviation prune shared by the three stages
/// (`:7847-7863`, `:7910-7926`, `:7993-8008`).
///
/// `None` means "kill this class" (C's `count = 0; continue;`).
#[inline]
fn class_prune(
    count: u32,
    class_best: u64,
    stage_best: u64,
    class_th: u64,
    band_cnt: u8,
) -> Option<u32> {
    if class_best == 0 || stage_best == 0 || class_best == stage_best {
        return Some(count);
    }
    if class_th == 0 {
        return None;
    }
    let dev = ((class_best - stage_best) * 100) / stage_best;
    if dev == 0 {
        return Some(count);
    }
    if dev >= class_th {
        return None;
    }
    Some(band_reduce(count, dev, band_cnt, class_th))
}

/// C `sort_fast_cost_based_candidates` (`:1415-1436`).
///
/// Fills `cand_buff_indices[0..count]` with `start_idx .. start_idx+count`
/// and exchange-sorts them by fast cost. Returned rather than written
/// through an out-pointer.
///
/// **The sort algorithm is part of the contract.** C's doubly-nested
/// `for i { for j > i { if cost[j] < cost[i] swap } }` is NOT a stable sort:
/// on an exact cost tie it can move a later element ahead of an earlier one.
/// A `sort_by_key` agrees on every distinct-cost input and disagrees on ties,
/// and a tie here changes which candidate reaches MDS3 — the same class of
/// divergence `leaf_funnel::nic` documents costing 305 downstream tree flips.
#[must_use]
pub fn sort_fast_cost_based_candidates(
    start_idx: u32,
    count: usize,
    fast_cost: impl Fn(u32) -> u64,
) -> Vec<u32> {
    let mut indices: Vec<u32> = (0..count as u32).map(|k| start_idx + k).collect();
    exchange_sort_by(&mut indices, &fast_cost);
    indices
}

/// C `sort_full_cost_based_candidates` (`:1438-1452`, EXPORTED).
///
/// Sorts `indices` in place by full cost with the same exchange sort as
/// [`sort_fast_cost_based_candidates`]; see there for why the algorithm and
/// not just the ordering is the contract.
pub fn sort_full_cost_based_candidates(indices: &mut [u32], full_cost: impl Fn(u32) -> u64) {
    exchange_sort_by(indices, &full_cost);
}

/// C's exchange sort, the shape both `sort_*_cost_based_candidates` use.
fn exchange_sort_by(indices: &mut [u32], cost: &impl Fn(u32) -> u64) {
    let n = indices.len();
    for i in 0..n.saturating_sub(1) {
        for j in (i + 1)..n {
            if cost(indices[j]) < cost(indices[i]) {
                indices.swap(i, j);
            }
        }
    }
}

/// C `construct_best_sorted_arrays_md_stage_3` (`:1454-1466`).
///
/// The union MDS3 evaluates: each class's surviving buffer indices,
/// CONCATENATED in class order. C does not re-sort across classes, so a
/// cross-class cost tie is broken toward the lower class by the later
/// strict-`<` winner scan.
#[must_use]
pub fn construct_best_sorted_arrays_md_stage_3(
    stage3_count: &[u32; CAND_CLASS_TOTAL],
    cand_buff_indices: &[&[u32]; CAND_CLASS_TOTAL],
) -> Vec<u32> {
    let mut out = Vec::with_capacity(stage3_count.iter().map(|&c| c as usize).sum());
    for class in CandClass::ALL {
        let n = stage3_count[class.index()] as usize;
        out.extend_from_slice(&cand_buff_indices[class.index()][..n]);
    }
    out
}

/// What [`post_mds0_nic_pruning`] decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mds0Prune {
    /// C `ctx->md_stage_1_total_count`.
    pub total: u32,
    /// C `ctx->perform_mds1` — cleared when MDS1 would run on one candidate.
    pub perform_mds1: bool,
}

/// C `post_mds0_nic_pruning` (`:7819-7882`).
///
/// `sorted_fast_costs[c]` is class `c`'s candidate buffer AFTER
/// [`sort_fast_cost_based_candidates`] — i.e. C's
/// `*cand_bf_ptr_array[cand_buff_indices[c][k]]->fast_cost` for
/// `k = 0, 1, ...`, ascending. `stage1_count` is updated in place, which is
/// what C does to `ctx->md_stage_1_count`.
///
/// `best_md_stage_cost` is the MDS0 GLOBAL best (the cheapest class head),
/// not the class's own best; the two are compared and that is the whole
/// point of the class prune.
///
/// C accumulates `md_stage_1_total_count` with `+=` onto a field the driver
/// zeroed at `:9463`, and its two `continue`s skip an `+= 0`; the total
/// returned here is therefore the same value.
// The divisions below are guarded by a `class_best != 0` test that scopes a
// whole block, not one expression, so `checked_div` cannot express them
// without restructuring hot RD control flow — the same call
// `leaf_funnel::nic` makes. `clippy::manual_checked_ops` post-dates the 1.89
// MSRV floor's clippy, so the allow must tolerate being unknown there.
#[allow(unknown_lints, clippy::manual_checked_ops)]
pub fn post_mds0_nic_pruning(
    ctrls: &NicPruningCtrls,
    (q_weight, q_denom): (u32, u32),
    is_i_slice: bool,
    sorted_fast_costs: &[&[u64]; CAND_CLASS_TOTAL],
    stage0_count: &[u32; CAND_CLASS_TOTAL],
    stage1_count: &mut [u32; CAND_CLASS_TOTAL],
    best_md_stage_cost: u64,
) -> Mds0Prune {
    // C `:7826` — an I-slice forces the class threshold to the disabled
    // sentinel regardless of the config value.
    let class_th = if is_i_slice {
        Threshold(None)
    } else {
        Threshold::scaled(ctrls.mds1_class_th, q_weight, q_denom)
    };
    let cand_th_intra = Threshold::scaled(ctrls.mds1_cand_base_th_intra, q_weight, q_denom);
    let cand_th_inter = Threshold::scaled(ctrls.mds1_cand_base_th_inter, q_weight, q_denom);

    let mut total = 0u32;
    for class in CandClass::ALL {
        let c = class.index();
        let cand_th = if class.is_intra() {
            cand_th_intra
        } else {
            cand_th_inter
        };
        if (cand_th.is_enabled() || class_th.is_enabled())
            && stage0_count[c] > 0
            && stage1_count[c] > 0
        {
            let costs = sorted_fast_costs[c];
            debug_assert!(
                costs.windows(2).all(|w| w[0] <= w[1]),
                "post_mds0 reads a fast-cost-SORTED class buffer"
            );
            let class_best = costs[0];
            match class_prune(
                stage1_count[c],
                class_best,
                best_md_stage_cost,
                class_th.raw(),
                ctrls.mds1_band_cnt,
            ) {
                None => {
                    stage1_count[c] = 0;
                    continue;
                }
                Some(reduced) => stage1_count[c] = reduced,
            }
            // Per-candidate prune (`:7865-7873`): keep candidates whose
            // deviation from the class best stays under a threshold that
            // TIGHTENS with rank.
            let mut cand_count = 1u32;
            if class_best != 0 {
                while cand_count < stage1_count[c] {
                    let dev = (costs[cand_count as usize] - class_best) * 100 / class_best;
                    if dev >= rank_scaled(cand_th.raw(), ctrls.mds1_cand_th_rank_factor, cand_count)
                    {
                        break;
                    }
                    cand_count += 1;
                }
            }
            stage1_count[c] = cand_count;
        }
        total += stage1_count[c];
    }

    Mds0Prune {
        total,
        // C `:7879` — MDS1 on a single candidate buys nothing.
        perform_mds1: !(ctrls.enable_skipping_mds1 && total == 1),
    }
}

/// Which candidate won a stage, and in which class — C's
/// `mds0_best_idx` / `mds0_best_class_it` pair (and the MDS1 twin).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StageWinner {
    /// Buffer index of the winning candidate.
    pub idx: u32,
    /// Class it belongs to.
    pub class: CandClass,
}

/// C `post_mds1_nic_pruning` (`:7885-7960`).
///
/// `sorted_full_costs[c]` is class `c`'s buffer after
/// [`sort_full_cost_based_candidates`]. `mds0_best` / `mds1_best` feed the
/// rank-factor staging at `:7934-7939`, which is the one place these prunes
/// look at candidate IDENTITY rather than cost.
pub fn post_mds1_nic_pruning(
    ctrls: &NicPruningCtrls,
    (q_weight, q_denom): (u32, u32),
    is_i_slice: bool,
    sorted_full_costs: &[&[u64]; CAND_CLASS_TOTAL],
    stage1_count: &[u32; CAND_CLASS_TOTAL],
    stage2_count: &mut [u32; CAND_CLASS_TOTAL],
    best_md_stage_cost: u64,
    mds0_best: StageWinner,
    mds1_best: StageWinner,
) -> u32 {
    let cand_th = Threshold::scaled(ctrls.mds2_cand_base_th, q_weight, q_denom);
    // C `:7897` — as at MDS0, an I-slice disables the class prune.
    let class_th = if is_i_slice {
        Threshold(None)
    } else {
        Threshold::scaled(ctrls.mds2_class_th, q_weight, q_denom)
    };

    let mut total = 0u32;
    for class in CandClass::ALL {
        let c = class.index();
        if (cand_th.is_enabled() || class_th.is_enabled())
            && stage1_count[c] > 0
            && stage2_count[c] > 0
        {
            let costs = sorted_full_costs[c];
            debug_assert!(
                costs.windows(2).all(|w| w[0] <= w[1]),
                "post_mds1 reads a full-cost-SORTED class buffer"
            );
            let class_best = costs[0];
            match class_prune(
                stage2_count[c],
                class_best,
                best_md_stage_cost,
                class_th.raw(),
                ctrls.mds2_band_cnt,
            ) {
                None => {
                    stage2_count[c] = 0;
                    continue;
                }
                Some(reduced) => stage2_count[c] = reduced,
            }
            // C guards the candidate prune with `count > 0` here and not at
            // the other two stages (`:7929`). The guard cannot fail — the
            // band reduction floors at 1 — but it is transcribed rather than
            // dropped.
            if stage2_count[c] > 0 {
                let mut cand_count = 1u32;
                if class_best != 0 && cand_count < stage2_count[c] {
                    // C `:7934-7939`: the rank factor is made HARSHER for a
                    // class that did not win MDS1, and slightly harsher for
                    // the winning class when MDS0 and MDS1 agreed on the
                    // candidate. Both arms are skipped when the base factor
                    // is off.
                    let mut rank_factor = ctrls.mds2_cand_th_rank_factor;
                    if rank_factor != 0 {
                        if class != mds1_best.class {
                            rank_factor += 3;
                        } else if mds0_best.idx == mds1_best.idx {
                            rank_factor += 2;
                        }
                    }
                    let dev_of = |k: u32| (costs[k as usize] - class_best) * 100 / class_best;
                    let mut dev = dev_of(cand_count);
                    let mut prev_dev = dev;
                    while (ctrls.mds2_relative_dev_th == 0
                        || dev <= prev_dev + u64::from(ctrls.mds2_relative_dev_th))
                        && dev < rank_scaled(cand_th.raw(), rank_factor, cand_count)
                    {
                        cand_count += 1;
                        if cand_count >= stage2_count[c] {
                            break;
                        }
                        prev_dev = dev;
                        dev = dev_of(cand_count);
                    }
                }
                stage2_count[c] = cand_count;
            }
        }
        total += stage2_count[c];
    }
    total
}

/// C `post_mds2_nic_pruning` (`:7963-8023`).
///
/// The one prune whose class threshold is LIVE on an intra frame: instead of
/// being disabled, `:7978` re-floors it to
/// `MAX(25, scaled * i_mds3_class_th_mult)`.
#[allow(unknown_lints, clippy::manual_checked_ops)]
pub fn post_mds2_nic_pruning(
    ctrls: &NicPruningCtrls,
    (q_weight, q_denom): (u32, u32),
    is_i_slice: bool,
    sorted_full_costs: &[&[u64]; CAND_CLASS_TOTAL],
    stage2_count: &[u32; CAND_CLASS_TOTAL],
    stage3_count: &mut [u32; CAND_CLASS_TOTAL],
    best_md_stage_cost: u64,
) -> u32 {
    let cand_th = Threshold::scaled(ctrls.mds3_cand_base_th, q_weight, q_denom);
    let mut class_th = Threshold::scaled(ctrls.mds3_class_th, q_weight, q_denom);
    if is_i_slice && let Threshold(Some(v)) = class_th {
        class_th = Threshold(Some(25.max(v * u64::from(ctrls.i_mds3_class_th_mult))));
    }

    let mut total = 0u32;
    for class in CandClass::ALL {
        let c = class.index();
        // C `:7986-7988`: the MDS2 count gates the prune, the MDS3 count is
        // what gets pruned — "to preserve the onion ring".
        if (cand_th.is_enabled() || class_th.is_enabled())
            && stage2_count[c] > 0
            && stage3_count[c] > 0
        {
            let costs = sorted_full_costs[c];
            debug_assert!(
                costs.windows(2).all(|w| w[0] <= w[1]),
                "post_mds2 reads a full-cost-SORTED class buffer"
            );
            let class_best = costs[0];
            match class_prune(
                stage3_count[c],
                class_best,
                best_md_stage_cost,
                class_th.raw(),
                ctrls.mds3_band_cnt,
            ) {
                None => {
                    stage3_count[c] = 0;
                    continue;
                }
                Some(reduced) => stage3_count[c] = reduced,
            }
            // No rank factor at this stage — a flat threshold (`:8011-8019`).
            let mut cand_count = 1u32;
            if class_best != 0 {
                while cand_count < stage3_count[c] {
                    let dev = (costs[cand_count as usize] - class_best) * 100 / class_best;
                    if dev >= cand_th.raw() {
                        break;
                    }
                    cand_count += 1;
                }
            }
            stage3_count[c] = cand_count;
        }
        total += stage3_count[c];
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tier 4 vectors: every assertion below was traced by hand against
    /// `product_coding_loop.c`. The functions are `static` in C.
    fn ctrls() -> NicPruningCtrls {
        NicPruningCtrls {
            mds1_class_th: Some(100),
            mds1_band_cnt: 3,
            mds2_class_th: Some(50),
            mds2_band_cnt: 3,
            mds3_class_th: Some(50),
            i_mds3_class_th_mult: 1,
            mds3_band_cnt: 3,
            mds1_cand_base_th_intra: Some(100),
            mds1_cand_base_th_inter: Some(200),
            mds1_cand_th_rank_factor: 0,
            mds2_cand_base_th: Some(100),
            mds2_cand_th_rank_factor: 0,
            mds2_relative_dev_th: 0,
            mds3_cand_base_th: Some(100),
            enable_skipping_mds1: true,
            merge_inter_cands_mult: 0,
        }
    }

    #[test]
    fn exchange_sort_differs_from_a_stable_sort_on_a_tie() {
        // costs indexed by buffer id: 0 -> 10, 1 -> 5, 2 -> 5.
        let costs = [10u64, 5, 5];
        let got = sort_fast_cost_based_candidates(0, 3, |i| costs[i as usize]);
        // C's i=0 pass swaps in buffer 1, then compares j=2 (5) against 5:
        // strict `<` fails, so 2 stays behind. i=1 pass compares 0 (10)
        // against 2 (5) and swaps.
        assert_eq!(got, vec![1, 2, 0]);
        // A stable sort_by_key would give [1, 2, 0] here too; the difference
        // shows when the tie straddles the leading element.
        let costs2 = [5u64, 7, 5];
        let got2 = sort_fast_cost_based_candidates(0, 3, |i| costs2[i as usize]);
        let mut stable: Vec<u32> = vec![0, 1, 2];
        stable.sort_by_key(|&i| costs2[i as usize]);
        assert_eq!(got2, vec![0, 2, 1]);
        assert_eq!(stable, vec![0, 2, 1]);
    }

    #[test]
    fn class_threshold_is_disabled_on_an_i_slice_and_live_otherwise() {
        // Class 1 (inter) sits 200% above the global best -> dev 200 >= 100,
        // which kills the class on a P slice and does nothing on an I slice.
        let c0 = [100u64];
        let c1 = [300u64];
        let empty: &[u64] = &[];
        let costs = [&c0[..], &c1[..], empty, empty, empty];
        let s0 = [1u32, 1, 0, 0, 0];

        let mut s1 = [1u32, 1, 0, 0, 0];
        let out = post_mds0_nic_pruning(&ctrls(), (1, 1), false, &costs, &s0, &mut s1, 100);
        assert_eq!(s1, [1, 0, 0, 0, 0]);
        assert_eq!(out.total, 1);
        // Exactly one survivor and skipping enabled -> MDS1 is skipped.
        assert!(!out.perform_mds1);

        let mut s1i = [1u32, 1, 0, 0, 0];
        let outi = post_mds0_nic_pruning(&ctrls(), (1, 1), true, &costs, &s0, &mut s1i, 100);
        assert_eq!(s1i, [1, 1, 0, 0, 0]);
        assert_eq!(outi.total, 2);
        assert!(outi.perform_mds1);
    }

    #[test]
    fn intra_and_inter_classes_take_different_candidate_thresholds() {
        // Second candidate deviates 150% from its class best: under the
        // intra threshold (100) it is dropped, under the inter one (200) kept.
        let intra = [100u64, 250];
        let inter = [100u64, 250];
        let empty: &[u64] = &[];
        let costs = [&intra[..], &inter[..], empty, empty, empty];
        let s0 = [2u32, 2, 0, 0, 0];
        let mut s1 = [2u32, 2, 0, 0, 0];
        // best_md_stage_cost == both class bests, so the class prune is inert.
        post_mds0_nic_pruning(&ctrls(), (1, 1), false, &costs, &s0, &mut s1, 100);
        assert_eq!(s1[0], 1, "intra class prunes at 100");
        assert_eq!(s1[1], 2, "inter class keeps at 200");
    }

    #[test]
    fn band_reduction_halves_the_count_in_the_middle_band() {
        // dev 40 with class_th 100 and band_cnt 3 -> band_idx 0 -> unchanged;
        // dev 60 -> band_idx 1 -> DIVIDE_AND_ROUND(4, 2) = 2.
        assert_eq!(band_reduce(4, 40, 3, 100), 4);
        assert_eq!(band_reduce(4, 60, 3, 100), 2);
        // band_cnt < 3 disables it entirely, and a single candidate is never
        // reduced.
        assert_eq!(band_reduce(4, 60, 2, 100), 4);
        assert_eq!(band_reduce(1, 60, 3, 100), 1);
    }

    #[test]
    fn mds3_class_threshold_is_refloored_on_an_i_slice() {
        // mds3_class_th 50 * mult 4 = 200 on an I slice; a class deviating
        // 100% survives there and dies on a P slice.
        let mut c = ctrls();
        c.i_mds3_class_th_mult = 4;
        let c0 = [100u64];
        let c3 = [200u64];
        let empty: &[u64] = &[];
        let costs = [&c0[..], empty, empty, &c3[..], empty];
        let s2 = [1u32, 0, 0, 1, 0];

        let mut s3 = [1u32, 0, 0, 1, 0];
        assert_eq!(
            post_mds2_nic_pruning(&c, (1, 1), true, &costs, &s2, &mut s3, 100),
            2
        );
        let mut s3p = [1u32, 0, 0, 1, 0];
        assert_eq!(
            post_mds2_nic_pruning(&c, (1, 1), false, &costs, &s2, &mut s3p, 100),
            1
        );
        assert_eq!(s3p[3], 0);
    }

    #[test]
    fn mds3_class_threshold_floors_at_25_on_an_i_slice() {
        // scaled 5 * mult 1 = 5, floored to 25: a class 10% off the best
        // survives even though the unfloored threshold would kill it.
        let mut c = ctrls();
        c.mds3_class_th = Some(5);
        let c0 = [100u64];
        let c3 = [110u64];
        let empty: &[u64] = &[];
        let costs = [&c0[..], empty, empty, &c3[..], empty];
        let s2 = [1u32, 0, 0, 1, 0];
        let mut s3 = [1u32, 0, 0, 1, 0];
        assert_eq!(
            post_mds2_nic_pruning(&c, (1, 1), true, &costs, &s2, &mut s3, 100),
            2
        );
    }

    #[test]
    fn mds1_rank_factor_staging_penalises_a_losing_class() {
        // cand_th 100, rank factor 1. Candidate 1 deviates 60%.
        // Winning class: rank 1 + 2 (mds0 == mds1 winner) = 3 -> 100/3 = 33,
        //   60 >= 33 -> dropped.
        // With the rank factor off the raw 100 applies -> kept.
        let mut c = ctrls();
        c.mds2_cand_th_rank_factor = 1;
        let c0 = [100u64, 160];
        let empty: &[u64] = &[];
        let costs = [&c0[..], empty, empty, empty, empty];
        let s1 = [2u32, 0, 0, 0, 0];
        let w = StageWinner {
            idx: 0,
            class: CandClass::Intra,
        };
        let mut s2 = [2u32, 0, 0, 0, 0];
        post_mds1_nic_pruning(&c, (1, 1), false, &costs, &s1, &mut s2, 100, w, w);
        assert_eq!(s2[0], 1);

        let mut c_off = c;
        c_off.mds2_cand_th_rank_factor = 0;
        let mut s2b = [2u32, 0, 0, 0, 0];
        post_mds1_nic_pruning(&c_off, (1, 1), false, &costs, &s1, &mut s2b, 100, w, w);
        assert_eq!(s2b[0], 2);
    }

    #[test]
    fn relative_deviation_threshold_stops_a_jump() {
        // devs 10, 20, 90. With mds2_relative_dev_th = 15 the third
        // candidate's jump (90 > 20 + 15) stops the walk even though 90 is
        // still under the flat threshold of 100.
        let mut c = ctrls();
        c.mds2_relative_dev_th = 15;
        let c0 = [100u64, 110, 120, 190];
        let empty: &[u64] = &[];
        let costs = [&c0[..], empty, empty, empty, empty];
        let s1 = [4u32, 0, 0, 0, 0];
        let w = StageWinner {
            idx: 0,
            class: CandClass::Intra,
        };
        let mut s2 = [4u32, 0, 0, 0, 0];
        post_mds1_nic_pruning(&c, (1, 1), false, &costs, &s1, &mut s2, 100, w, w);
        assert_eq!(s2[0], 3);
    }

    #[test]
    fn union_is_class_concatenated_not_cost_sorted() {
        let i0 = [7u32, 8];
        let i1 = [1u32];
        let empty: &[u32] = &[];
        let buffers = [&i0[..], &i1[..], empty, empty, empty];
        let counts = [2u32, 1, 0, 0, 0];
        assert_eq!(
            construct_best_sorted_arrays_md_stage_3(&counts, &buffers),
            vec![7, 8, 1]
        );
    }

    #[test]
    fn disabled_thresholds_leave_every_count_untouched() {
        let mut c = ctrls();
        c.mds1_class_th = None;
        c.mds1_cand_base_th_intra = None;
        c.mds1_cand_base_th_inter = None;
        let c0 = [100u64, 100_000];
        let empty: &[u64] = &[];
        let costs = [&c0[..], empty, empty, empty, empty];
        let s0 = [2u32, 0, 0, 0, 0];
        let mut s1 = [2u32, 0, 0, 0, 0];
        let out = post_mds0_nic_pruning(&c, (1, 1), false, &costs, &s0, &mut s1, 1);
        assert_eq!(s1[0], 2);
        assert_eq!(out.total, 2);
    }
}
