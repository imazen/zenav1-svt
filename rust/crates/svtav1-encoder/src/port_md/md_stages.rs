//! The MD-stage driver of `Source/Lib/Codec/product_coding_loop.c` — the
//! class loop that runs MDS0..MDS3, sorts each class's survivors and tracks
//! the per-stage winners (`md_encode_block`, `:9459-9640`).
//!
//! This is the consumer [`super::nic_prune`] was written for. On its own
//! that module is a set of prunes with nothing to prune; here they sit in
//! C's order, with C's buffer accounting and C's best-cost resets.
//!
//! # What is translated, and what is a parameter
//!
//! Everything that DECIDES: the buffer arithmetic, the per-class ordering,
//! the winner scans and their tie-breaks, the three prune call sites, the
//! `best_md_stage_cost` resets, and the two `perform_mds1 == 0` shortcuts.
//!
//! The four operations the loop sequences — `md_stage_0` (candidate
//! generation + fast cost), `md_stage_1`, `md_stage_2`, and the cost
//! lookups — are [`MdStageOps`]. They are the RD machinery, not this C
//! file's arithmetic, and the port already has them elsewhere.
//!
//! # Evidence
//!
//! **Tier 4** — the driver is the body of `md_encode_block`, which is
//! `static` in C with no exported symbol (`docs/WORKING-ON-THIS.md` §4).
//! [`super::nic_prune::sort_full_cost_based_candidates`], which this calls,
//! IS tier 1.
//!
//! # Reachability
//!
//! Nothing calls this yet — the public entry point still refuses inter
//! frames (`docs/WORKING-ON-THIS.md` §7).

use super::nic_prune::{
    self, CAND_CLASS_TOTAL, CandClass, Mds0Prune, NicPruningCtrls, StageWinner,
};

/// The RD operations the stage loop sequences. Each is a whole subsystem in
/// C; here they are the caller's.
pub trait MdStageOps {
    /// C `md_stage_0` (`:9494-9503`): generate this class's candidates into
    /// `buffer_start_idx .. buffer_start_idx + buffer_count` and leave a
    /// fast cost in every one of them.
    fn run_md_stage_0(&mut self, class: CandClass, buffer_start_idx: u32, buffer_count: u32);
    /// C `md_stage_1` (`:9557`) over this class's surviving buffers.
    fn run_md_stage_1(&mut self, class: CandClass);
    /// C `md_stage_2` (`:9601`) over this class's surviving buffers.
    fn run_md_stage_2(&mut self, class: CandClass);
    /// `*cand_bf_ptr_array[buffer_idx]->fast_cost`.
    fn fast_cost(&self, buffer_idx: u32) -> u64;
    /// `*cand_bf_ptr_array[buffer_idx]->full_cost`.
    fn full_cost(&self, buffer_idx: u32) -> u64;
    /// `cand_bf_ptr_array[buffer_idx]->luma_fast_dist`, read once for the
    /// MDS3 TX-shortcut detector (`:9531`).
    fn luma_fast_dist(&self, buffer_idx: u32) -> u64;
}

/// The per-class stage counts the driver reads and rewrites — C's
/// `ctx->md_stage_N_count[]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StageCounts {
    pub stage0: [u32; CAND_CLASS_TOTAL],
    pub stage1: [u32; CAND_CLASS_TOTAL],
    pub stage2: [u32; CAND_CLASS_TOTAL],
    pub stage3: [u32; CAND_CLASS_TOTAL],
}

/// The staging switches the driver consults, gathered so the signature does
/// not grow a dozen booleans.
#[derive(Debug, Clone, Copy)]
pub struct MdStageConfig {
    /// C `ctx->bypass_md_stage_1` (from [`super::nics::set_md_stage_counts`]).
    pub bypass_md_stage_1: bool,
    /// C `ctx->bypass_md_stage_2`.
    pub bypass_md_stage_2: bool,
    /// C `pcs->slice_type == I_SLICE`.
    pub is_i_slice: bool,
    /// C `svt_aom_get_qp_based_th_scaling_factors(..)` — `(weight, denom)`.
    pub qp_scale: (u32, u32),
    /// C `ctx->max_nics`, the total candidate-buffer capacity.
    pub max_nics: u32,
    /// C `ctx->tx_shortcut_ctrls.use_mds3_shortcuts_th` — 0 is off.
    pub use_mds3_shortcuts_th: u32,
    /// C `ctx->mds0_use_hadamard_blk`.
    pub mds0_use_hadamard_blk: bool,
    /// C `ctx->qp_index`.
    pub qp_index: u32,
    /// The block's `(width, height)`.
    pub block: (usize, usize),
}

/// Everything the driver decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MdStageResult {
    /// C `ctx->cand_buff_indices[class]`, each already sorted for the last
    /// stage that touched it.
    pub cand_buff_indices: [Vec<u32>; CAND_CLASS_TOTAL],
    /// The counts after all three prunes.
    pub counts: StageCounts,
    /// C `ctx->md_stage_{1,2,3}_total_count`.
    pub totals: (u32, u32, u32),
    /// C `ctx->mds0_best_idx` / `mds0_best_class_it`.
    pub mds0_best: StageWinner,
    /// C `ctx->mds1_best_idx` / `mds1_best_class_it`.
    pub mds1_best: StageWinner,
    /// C `ctx->best_candidate_index_array[0 .. md_stage_3_total_count]`.
    pub best_candidate_index_array: Vec<u32>,
    /// C `ctx->perform_mds1`.
    pub perform_mds1: bool,
    /// C `ctx->use_tx_shortcuts_mds3`.
    pub use_tx_shortcuts_mds3: bool,
}

/// Raised instead of C's `svt_aom_assert_err` at `:9502`.
///
/// C aborts the encoder there ("not enough cand buffers"). A typed error is
/// the port's equivalent of the refusal discipline in
/// `docs/WORKING-ON-THIS.md` §6: an over-subscribed buffer pool is a
/// configuration bug, and emitting a stream from it would be emitting one
/// from candidates that were never scored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotEnoughCandBuffers {
    pub required: u32,
    pub max_nics: u32,
}

/// C `md_encode_block`'s stage loop (`:9459-9640`).
///
/// # Errors
///
/// [`NotEnoughCandBuffers`] when the per-class buffer runs exceed
/// `max_nics`, which is C's `svt_aom_assert_err` at `:9502`.
pub fn run_md_stages<O: MdStageOps>(
    ops: &mut O,
    ctrls: &NicPruningCtrls,
    cfg: &MdStageConfig,
    mut counts: StageCounts,
) -> Result<MdStageResult, NotEnoughCandBuffers> {
    // ---- MDS0 ------------------------------------------------------
    let mut cand_buff_indices: [Vec<u32>; CAND_CLASS_TOTAL] = Default::default();
    let mut buffer_start_idx = 0u32;
    let mut buffer_total_count = 0u32;
    let mut best_md_stage_cost = u64::MAX;
    let mut mds0_best = StageWinner {
        idx: 0,
        class: CandClass::Intra,
    };

    for class in CandClass::ALL {
        let c = class.index();
        // `:9486-9487` — a later stage can never keep more candidates than
        // the one before it.
        counts.stage1[c] = counts.stage0[c].min(counts.stage1[c]);
        if counts.stage0[c] == 0 || counts.stage1[c] == 0 {
            continue;
        }
        // `:9491` — one SPARE buffer per class: MDS0's replacement pool
        // needs somewhere to put the candidate it is about to reject.
        let buffer_count = counts.stage1[c] + 1;
        buffer_total_count += buffer_count;
        if buffer_total_count > cfg.max_nics {
            return Err(NotEnoughCandBuffers {
                required: buffer_total_count,
                max_nics: cfg.max_nics,
            });
        }
        ops.run_md_stage_0(class, buffer_start_idx, buffer_count);

        // `:9505-9517` — with a single survivor C does NOT sort; it picks
        // the cheaper of the two buffers directly. That is not the same
        // tie-break: this comparison is `fast[start] < fast[start + 1]`, so
        // an exact tie takes `start + 1`, while the exchange sort's strict
        // `<` leaves `start` in front on a tie. One swapped survivor changes
        // the whole block downstream, so the special case is transcribed
        // rather than folded into the sort.
        cand_buff_indices[c] = if counts.stage1[c] == 1 {
            let a = ops.fast_cost(buffer_start_idx);
            let b = ops.fast_cost(buffer_start_idx + 1);
            vec![if a < b {
                buffer_start_idx
            } else {
                buffer_start_idx + 1
            }]
        } else {
            // `:9518-9523` — sorted over `count + 1` buffers, not
            // `buffer_count`; C's comment says `buffer_count_for_curr_class`
            // can be wrong when MDS0 ran multiple iterations.
            nic_prune::sort_fast_cost_based_candidates(
                buffer_start_idx,
                (counts.stage1[c] + 1) as usize,
                |i| ops.fast_cost(i),
            )
        };

        let head = cand_buff_indices[c][0];
        let head_cost = ops.fast_cost(head);
        // Strict `<`, so a cross-class tie keeps the EARLIER class.
        if head_cost < best_md_stage_cost {
            best_md_stage_cost = head_cost;
            mds0_best = StageWinner { idx: head, class };
        }
        buffer_start_idx += buffer_count;
    }

    // `post_mds0_nic_pruning` reads each class's sorted candidate costs.
    let fast_costs: Vec<Vec<u64>> = cand_buff_indices
        .iter()
        .map(|v| v.iter().map(|&i| ops.fast_cost(i)).collect())
        .collect();
    let fast_slices: [&[u64]; CAND_CLASS_TOTAL] = [
        &fast_costs[0],
        &fast_costs[1],
        &fast_costs[2],
        &fast_costs[3],
        &fast_costs[4],
    ];
    let stage0 = counts.stage0;
    let Mds0Prune {
        total: total1,
        perform_mds1,
    } = nic_prune::post_mds0_nic_pruning(
        ctrls,
        cfg.qp_scale,
        cfg.is_i_slice,
        &fast_slices,
        &stage0,
        &mut counts.stage1,
        best_md_stage_cost,
    );

    // `:9527-9534` — the MDS3 TX-shortcut detector, live ONLY when MDS1 was
    // skipped (there is no MDS1 information to use instead) and only when
    // MDS0 scored with variance rather than the Hadamard SATD.
    let mut use_tx_shortcuts_mds3 = false;
    if !perform_mds1 && cfg.use_mds3_shortcuts_th != 0 && !cfg.mds0_use_hadamard_blk {
        let dist = ops.luma_fast_dist(mds0_best.idx);
        let th_normalizer = (cfg.block.0 * cfg.block.1) as u64 * u64::from(cfg.qp_index);
        use_tx_shortcuts_mds3 = 100 * dist < u64::from(cfg.use_mds3_shortcuts_th) * th_normalizer;
    }
    debug_assert!(
        perform_mds1 || total1 == 1,
        "C asserts IMPLIES(!perform_mds1, md_stage_1_total_count == 1) at :9539"
    );

    // ---- MDS1 ------------------------------------------------------
    // `:9543-9545` — the best cost is reset ONLY when MDS1 will actually
    // run. If MDS1 is bypassed the full costs were never computed, so the
    // post-MDS1 prune has to keep comparing MDS0's fast costs.
    if !cfg.bypass_md_stage_1 {
        best_md_stage_cost = u64::MAX;
    }
    let mut mds1_best = mds0_best;
    for class in CandClass::ALL {
        let c = class.index();
        counts.stage2[c] = counts.stage1[c].min(counts.stage2[c]);
        if !perform_mds1 {
            // `:9571-9573` — with no MDS1 the winner is MDS0's, copied once
            // per class iteration in C and idempotent.
            mds1_best = mds0_best;
            continue;
        }
        if cfg.bypass_md_stage_1 || counts.stage1[c] == 0 || counts.stage2[c] == 0 {
            continue;
        }
        ops.run_md_stage_1(class);
        if counts.stage1[c] != 0 {
            let n = counts.stage1[c] as usize;
            nic_prune::sort_full_cost_based_candidates(&mut cand_buff_indices[c][..n], |i| {
                ops.full_cost(i)
            });
        }
        let head = cand_buff_indices[c][0];
        let head_cost = ops.full_cost(head);
        if head_cost < best_md_stage_cost {
            best_md_stage_cost = head_cost;
            mds1_best = StageWinner { idx: head, class };
        }
    }

    let mut total2 = 0u32;
    if perform_mds1 {
        let full_costs: Vec<Vec<u64>> = cand_buff_indices
            .iter()
            .map(|v| v.iter().map(|&i| ops.full_cost(i)).collect())
            .collect();
        let full_slices: [&[u64]; CAND_CLASS_TOTAL] = [
            &full_costs[0],
            &full_costs[1],
            &full_costs[2],
            &full_costs[3],
            &full_costs[4],
        ];
        let stage1 = counts.stage1;
        total2 = nic_prune::post_mds1_nic_pruning(
            ctrls,
            cfg.qp_scale,
            cfg.is_i_slice,
            &full_slices,
            &stage1,
            &mut counts.stage2,
            best_md_stage_cost,
            mds0_best,
            mds1_best,
        );
    }

    // ---- MDS2 ------------------------------------------------------
    if !cfg.bypass_md_stage_2 {
        best_md_stage_cost = u64::MAX;
    }
    for class in CandClass::ALL {
        let c = class.index();
        counts.stage3[c] = counts.stage2[c].min(counts.stage3[c]);
        if !perform_mds1 || cfg.bypass_md_stage_2 {
            continue;
        }
        if counts.stage2[c] == 0 || counts.stage3[c] == 0 {
            continue;
        }
        ops.run_md_stage_2(class);
        if counts.stage2[c] != 0 {
            let n = counts.stage2[c] as usize;
            nic_prune::sort_full_cost_based_candidates(&mut cand_buff_indices[c][..n], |i| {
                ops.full_cost(i)
            });
        }
        // `:9612` — a plain MIN here, with no winner index: MDS2 refines
        // costs but never re-elects a winner.
        best_md_stage_cost = best_md_stage_cost.min(ops.full_cost(cand_buff_indices[c][0]));
    }

    // ---- MDS3 selection --------------------------------------------
    let (total3, best_candidate_index_array) = if perform_mds1 {
        let full_costs: Vec<Vec<u64>> = cand_buff_indices
            .iter()
            .map(|v| v.iter().map(|&i| ops.full_cost(i)).collect())
            .collect();
        let full_slices: [&[u64]; CAND_CLASS_TOTAL] = [
            &full_costs[0],
            &full_costs[1],
            &full_costs[2],
            &full_costs[3],
            &full_costs[4],
        ];
        let stage2 = counts.stage2;
        let t = nic_prune::post_mds2_nic_pruning(
            ctrls,
            cfg.qp_scale,
            cfg.is_i_slice,
            &full_slices,
            &stage2,
            &mut counts.stage3,
            best_md_stage_cost,
        );
        let buffers: [&[u32]; CAND_CLASS_TOTAL] = [
            &cand_buff_indices[0],
            &cand_buff_indices[1],
            &cand_buff_indices[2],
            &cand_buff_indices[3],
            &cand_buff_indices[4],
        ];
        (
            t,
            nic_prune::construct_best_sorted_arrays_md_stage_3(&counts.stage3, &buffers),
        )
    } else {
        // `:9617-9619` — one candidate, taken from the MDS1-best CLASS's
        // head. Not from `mds1_best.idx` directly: the two agree here, but
        // C indexes the class array, and that is what a later stage reads.
        (1, vec![cand_buff_indices[mds1_best.class.index()][0]])
    };
    debug_assert!(total3 > 0, "C asserts md_stage_3_total_count > 0 at :9621");

    Ok(MdStageResult {
        cand_buff_indices,
        counts,
        totals: (total1, total2, total3),
        mds0_best,
        mds1_best,
        best_candidate_index_array,
        perform_mds1,
        use_tx_shortcuts_mds3,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pool of buffers whose costs the test writes directly — the RD
    /// machinery C runs is exactly what this port does not own.
    struct FakeOps {
        fast: Vec<u64>,
        full: Vec<u64>,
        dist: Vec<u64>,
        stage0_calls: Vec<(CandClass, u32, u32)>,
        stage1_calls: Vec<CandClass>,
        stage2_calls: Vec<CandClass>,
    }

    impl FakeOps {
        fn new(fast: Vec<u64>, full: Vec<u64>) -> Self {
            let n = fast.len();
            FakeOps {
                fast,
                full,
                dist: vec![0; n],
                stage0_calls: Vec::new(),
                stage1_calls: Vec::new(),
                stage2_calls: Vec::new(),
            }
        }
    }

    impl MdStageOps for FakeOps {
        fn run_md_stage_0(&mut self, class: CandClass, start: u32, count: u32) {
            self.stage0_calls.push((class, start, count));
        }
        fn run_md_stage_1(&mut self, class: CandClass) {
            self.stage1_calls.push(class);
        }
        fn run_md_stage_2(&mut self, class: CandClass) {
            self.stage2_calls.push(class);
        }
        fn fast_cost(&self, i: u32) -> u64 {
            self.fast[i as usize]
        }
        fn full_cost(&self, i: u32) -> u64 {
            self.full[i as usize]
        }
        fn luma_fast_dist(&self, i: u32) -> u64 {
            self.dist[i as usize]
        }
    }

    fn open_ctrls() -> NicPruningCtrls {
        NicPruningCtrls {
            mds1_class_th: None,
            mds1_band_cnt: 0,
            mds2_class_th: None,
            mds2_band_cnt: 0,
            mds3_class_th: None,
            i_mds3_class_th_mult: 1,
            mds3_band_cnt: 0,
            mds1_cand_base_th_intra: None,
            mds1_cand_base_th_inter: None,
            mds1_cand_th_rank_factor: 0,
            mds2_cand_base_th: None,
            mds2_cand_th_rank_factor: 0,
            mds2_relative_dev_th: 0,
            mds3_cand_base_th: None,
            enable_skipping_mds1: false,
            merge_inter_cands_mult: 0,
        }
    }

    fn cfg() -> MdStageConfig {
        MdStageConfig {
            bypass_md_stage_1: false,
            bypass_md_stage_2: false,
            is_i_slice: false,
            qp_scale: (1, 1),
            max_nics: 64,
            use_mds3_shortcuts_th: 0,
            mds0_use_hadamard_blk: false,
            qp_index: 100,
            block: (16, 16),
        }
    }

    fn counts_for(stage0: [u32; 5], n: u32) -> StageCounts {
        StageCounts {
            stage0,
            stage1: [n; 5],
            stage2: [n; 5],
            stage3: [n; 5],
        }
    }

    /// `:9491` — each class gets `stage1_count + 1` buffers, and the runs
    /// are consecutive.
    #[test]
    fn each_class_gets_its_own_consecutive_buffer_run_plus_a_spare() {
        let fast = vec![100u64; 32];
        let full = vec![100u64; 32];
        let mut ops = FakeOps::new(fast, full);
        let counts = counts_for([2, 3, 0, 0, 0], 2);
        let r = run_md_stages(&mut ops, &open_ctrls(), &cfg(), counts).unwrap();
        assert_eq!(
            ops.stage0_calls,
            vec![(CandClass::Intra, 0, 3), (CandClass::InterNew, 3, 3)]
        );
        assert_eq!(
            r.cand_buff_indices[2],
            Vec::<u32>::new(),
            "class 2 has none"
        );
    }

    /// `:9502` — over-subscribing the pool is a typed error, not a wrong
    /// stream.
    #[test]
    fn an_oversubscribed_buffer_pool_is_refused() {
        let mut ops = FakeOps::new(vec![0; 64], vec![0; 64]);
        let mut c = cfg();
        c.max_nics = 5;
        let err = run_md_stages(&mut ops, &open_ctrls(), &c, counts_for([4; 5], 4)).unwrap_err();
        assert_eq!(err.max_nics, 5);
        assert!(err.required > 5);
    }

    /// `:9505-9511` — the single-survivor shortcut breaks a tie the OTHER
    /// way from the exchange sort. Both buffers cost 50; C's `<` picks
    /// `start + 1`.
    #[test]
    fn the_single_survivor_shortcut_breaks_a_tie_toward_the_second_buffer() {
        let mut ops = FakeOps::new(vec![50, 50, 9, 9], vec![50, 50, 9, 9]);
        let counts = StageCounts {
            stage0: [1, 0, 0, 0, 0],
            stage1: [1, 0, 0, 0, 0],
            stage2: [1, 0, 0, 0, 0],
            stage3: [1, 0, 0, 0, 0],
        };
        let r = run_md_stages(&mut ops, &open_ctrls(), &cfg(), counts).unwrap();
        assert_eq!(r.cand_buff_indices[0], vec![1]);
        // The sort path, with the same two costs, would have kept 0. Shown
        // rather than asserted about the driver, so the difference is
        // visible in one place.
        let sorted = nic_prune::sort_fast_cost_based_candidates(0, 2, |i| [50u64, 50][i as usize]);
        assert_eq!(sorted[0], 0);
    }

    /// `:9515` — with two distinct costs the shortcut picks the cheaper.
    #[test]
    fn the_single_survivor_shortcut_picks_the_cheaper_buffer() {
        let mut ops = FakeOps::new(vec![9, 50, 0, 0], vec![9, 50, 0, 0]);
        let counts = StageCounts {
            stage0: [1, 0, 0, 0, 0],
            stage1: [1, 0, 0, 0, 0],
            stage2: [1, 0, 0, 0, 0],
            stage3: [1, 0, 0, 0, 0],
        };
        let r = run_md_stages(&mut ops, &open_ctrls(), &cfg(), counts).unwrap();
        assert_eq!(r.cand_buff_indices[0], vec![0]);
    }

    /// `:9520` — a cross-class tie on the MDS0 head keeps the EARLIER
    /// class, because the scan uses a strict `<`.
    #[test]
    fn a_cross_class_tie_keeps_the_earlier_class() {
        // Class 0 buffers 0..3, class 1 buffers 3..6; heads tie at 10.
        let fast = vec![10, 20, 30, 10, 20, 30, 0, 0];
        let mut ops = FakeOps::new(fast.clone(), fast);
        let r = run_md_stages(
            &mut ops,
            &open_ctrls(),
            &cfg(),
            counts_for([2, 2, 0, 0, 0], 2),
        )
        .unwrap();
        assert_eq!(r.mds0_best.class, CandClass::Intra);
        assert_eq!(r.mds0_best.idx, 0);
    }

    /// `:9543-9545` and `:9581-9583` — the best cost is reset only when the
    /// stage that would refill it actually runs.
    #[test]
    fn a_bypassed_stage_keeps_the_previous_best_cost() {
        let fast = vec![10, 20, 30, 40, 0, 0, 0, 0];
        let full = vec![99, 99, 99, 99, 0, 0, 0, 0];
        let mut c = cfg();
        c.bypass_md_stage_1 = true;
        let mut ops = FakeOps::new(fast, full);
        let r = run_md_stages(&mut ops, &open_ctrls(), &c, counts_for([2, 0, 0, 0, 0], 2)).unwrap();
        // MDS1 never ran, so no full costs were consulted for the winner.
        assert!(ops.stage1_calls.is_empty());
        assert_eq!(r.mds1_best, r.mds0_best);
    }

    /// `:9571-9573` — with MDS1 skipped the MDS1 winner IS the MDS0
    /// winner, and MDS3 evaluates exactly one candidate taken from that
    /// class's head.
    #[test]
    fn skipping_mds1_sends_one_candidate_to_mds3() {
        let mut ctrls = open_ctrls();
        ctrls.enable_skipping_mds1 = true;
        let fast = vec![10, 20, 0, 0];
        let mut ops = FakeOps::new(fast.clone(), fast);
        let counts = StageCounts {
            stage0: [1, 0, 0, 0, 0],
            stage1: [1, 0, 0, 0, 0],
            stage2: [1, 0, 0, 0, 0],
            stage3: [1, 0, 0, 0, 0],
        };
        let r = run_md_stages(&mut ops, &ctrls, &cfg(), counts).unwrap();
        assert!(!r.perform_mds1);
        assert!(ops.stage1_calls.is_empty() && ops.stage2_calls.is_empty());
        assert_eq!(r.totals.2, 1);
        assert_eq!(
            r.best_candidate_index_array,
            vec![r.cand_buff_indices[0][0]]
        );
    }

    /// `:9527-9534` — the MDS3 shortcut detector runs ONLY when MDS1 was
    /// skipped, and never under the Hadamard MDS0.
    #[test]
    fn the_mds3_shortcut_detector_needs_a_skipped_mds1_and_no_hadamard() {
        let mut ctrls = open_ctrls();
        ctrls.enable_skipping_mds1 = true;
        let counts = StageCounts {
            stage0: [1, 0, 0, 0, 0],
            stage1: [1, 0, 0, 0, 0],
            stage2: [1, 0, 0, 0, 0],
            stage3: [1, 0, 0, 0, 0],
        };
        let mut c = cfg();
        c.use_mds3_shortcuts_th = 30;
        // dist 0 -> 0 < 30 * 16*16*100 -> armed.
        let mut ops = FakeOps::new(vec![10, 20, 0, 0], vec![10, 20, 0, 0]);
        let r = run_md_stages(&mut ops, &ctrls, &c, counts).unwrap();
        assert!(r.use_tx_shortcuts_mds3);
        // Hadamard MDS0 disarms it.
        let mut c_had = c;
        c_had.mds0_use_hadamard_blk = true;
        let mut ops2 = FakeOps::new(vec![10, 20, 0, 0], vec![10, 20, 0, 0]);
        assert!(
            !run_md_stages(&mut ops2, &ctrls, &c_had, counts)
                .unwrap()
                .use_tx_shortcuts_mds3
        );
        // And it never arms when MDS1 runs.
        let mut ops3 = FakeOps::new(vec![10, 20, 0, 0], vec![10, 20, 0, 0]);
        assert!(
            !run_md_stages(&mut ops3, &open_ctrls(), &c, counts)
                .unwrap()
                .use_tx_shortcuts_mds3
        );
    }

    /// `:9486`, `:9578`, `:9594` — each stage's count is clamped by the
    /// previous one, so a class starved at MDS0 cannot revive later.
    #[test]
    fn a_stage_never_keeps_more_candidates_than_the_stage_before_it() {
        let fast = vec![10, 20, 30, 40, 50, 60, 70, 80];
        let mut ops = FakeOps::new(fast.clone(), fast);
        let counts = StageCounts {
            stage0: [1, 0, 0, 0, 0],
            stage1: [4, 0, 0, 0, 0],
            stage2: [4, 0, 0, 0, 0],
            stage3: [4, 0, 0, 0, 0],
        };
        let r = run_md_stages(&mut ops, &open_ctrls(), &cfg(), counts).unwrap();
        assert_eq!(r.counts.stage1[0], 1);
        assert_eq!(r.counts.stage2[0], 1);
        assert_eq!(r.counts.stage3[0], 1);
    }

    /// The MDS3 union is the class concatenation, in class order.
    #[test]
    fn the_mds3_union_is_the_class_concatenation() {
        let fast = vec![10, 11, 12, 20, 21, 22, 0, 0];
        let mut ops = FakeOps::new(fast.clone(), fast);
        let r = run_md_stages(
            &mut ops,
            &open_ctrls(),
            &cfg(),
            counts_for([2, 2, 0, 0, 0], 2),
        )
        .unwrap();
        assert_eq!(r.best_candidate_index_array, vec![0, 1, 3, 4]);
        assert_eq!(r.totals.2, 4);
    }

    /// MDS1 and MDS2 run per class, in class order, and only for classes
    /// that still have candidates.
    #[test]
    fn the_full_loops_run_per_class_in_class_order() {
        let fast = vec![10, 11, 12, 20, 21, 22, 0, 0];
        let full = vec![12, 11, 10, 22, 21, 20, 0, 0];
        let mut ops = FakeOps::new(fast, full);
        let _ = run_md_stages(
            &mut ops,
            &open_ctrls(),
            &cfg(),
            counts_for([2, 2, 0, 0, 0], 2),
        )
        .unwrap();
        assert_eq!(
            ops.stage1_calls,
            vec![CandClass::Intra, CandClass::InterNew]
        );
        assert_eq!(
            ops.stage2_calls,
            vec![CandClass::Intra, CandClass::InterNew]
        );
    }

    /// The MDS1 sort really re-orders by FULL cost — the survivors entering
    /// MDS3 are not simply the fast-cost order.
    #[test]
    fn the_mds1_sort_reorders_by_full_cost() {
        // Fast order 0, 1; full order 1, 0.
        let fast = vec![10, 20, 99, 0];
        let full = vec![20, 10, 99, 0];
        let mut ops = FakeOps::new(fast, full);
        let counts = StageCounts {
            stage0: [2, 0, 0, 0, 0],
            stage1: [2, 0, 0, 0, 0],
            stage2: [2, 0, 0, 0, 0],
            stage3: [2, 0, 0, 0, 0],
        };
        let r = run_md_stages(&mut ops, &open_ctrls(), &cfg(), counts).unwrap();
        assert_eq!(r.cand_buff_indices[0][0], 1);
        assert_eq!(r.mds1_best.idx, 1);
        assert_eq!(r.mds0_best.idx, 0, "MDS0 still elected buffer 0");
    }
}
