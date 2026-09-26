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
        vec![(CandClass::Intra, 0, 3), (CandClass::InterMvp, 3, 3)]
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
        vec![CandClass::Intra, CandClass::InterMvp]
    );
    assert_eq!(
        ops.stage2_calls,
        vec![CandClass::Intra, CandClass::InterMvp]
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
