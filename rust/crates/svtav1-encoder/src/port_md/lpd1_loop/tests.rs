use super::*;

/// Tier 4: hand-derived against the C lines named in each comment. All
/// three functions are `static` in `product_coding_loop.c`.
fn plane(data: &[u8], stride: usize) -> Plane<'_> {
    Plane::new(data, stride)
}

/// `:1552` — the toggle is INSIDE the improvement branch. With costs
/// 10, 20, 5 the winner must survive the middle candidate.
#[test]
fn the_buffer_pingpong_protects_the_running_winner() {
    let costs = [10u64, 20, 5];
    let mut seen: Vec<(usize, usize)> = Vec::new();
    let r = md_stage_0_light_pd1(costs.len(), |c, b| {
        seen.push((c, b));
        costs[c]
    });
    // cand 0 -> buffer 0, improves, toggle to 1.
    // cand 1 -> buffer 1, does NOT improve, no toggle (buffer 0 keeps
    //           the winner).
    // cand 2 -> buffer 1 again, improves, best_idx 1.
    assert_eq!(seen, vec![(0, 0), (1, 1), (2, 1)]);
    assert_eq!(r.best_buffer_idx, 1);
    assert_eq!(r.best_cost, 5);
    assert_eq!(r.best_cand_idx, Some(2));
}

/// An unconditional toggle clobbers the winner as soon as TWO
/// candidates in a row fail to improve: the second of them lands back
/// on the winner's buffer. Stated as its own case because the
/// difference is invisible on any sequence that keeps improving, and
/// the first draft of this test used one that did (10, 20, 5) and
/// passed under both readings.
#[test]
fn an_unconditional_toggle_would_clobber_the_winner() {
    let costs = [10u64, 20, 30];
    let mut buffers = [u64::MAX; 2];
    let r = md_stage_0_light_pd1(costs.len(), |c, b| {
        buffers[b] = costs[c];
        costs[c]
    });
    assert_eq!(buffers[r.best_buffer_idx], r.best_cost);
    // The alternate reading, simulated:
    let mut naive_buffers = [u64::MAX; 2];
    let mut naive_best = u64::MAX;
    let mut naive_idx = 0usize;
    for (c, &cost) in costs.iter().enumerate() {
        let b = c % 2;
        naive_buffers[b] = cost;
        if cost < naive_best {
            naive_best = cost;
            naive_idx = b;
        }
    }
    assert_eq!(naive_idx, 0, "the naive walk's winner is in buffer 0");
    assert_ne!(
        naive_buffers[naive_idx], naive_best,
        "and that buffer no longer holds the winner"
    );
}

/// Every candidate improving means the toggle runs every time.
#[test]
fn a_monotone_improving_sequence_alternates_buffers() {
    let costs = [40u64, 30, 20, 10];
    let mut seen = Vec::new();
    let r = md_stage_0_light_pd1(costs.len(), |c, b| {
        seen.push(b);
        costs[c]
    });
    assert_eq!(seen, vec![0, 1, 0, 1]);
    assert_eq!(r.best_buffer_idx, 1);
    assert_eq!(r.best_cand_idx, Some(3));
}

/// No candidates at all leaves the sentinel in place (C never enters
/// the loop, and `mds0_best_cost` stays `(uint64_t)~0`).
#[test]
fn an_empty_candidate_list_keeps_the_sentinel() {
    let r = md_stage_0_light_pd1(0, |_, _| unreachable!());
    assert_eq!(r.best_cost, u64::MAX);
    assert_eq!(r.best_cand_idx, None);
}

/// `:1022-1023` — `dc_only_th` is the threshold for a NON-DC mode.
#[test]
fn the_dc_and_non_dc_thresholds_are_the_other_way_round() {
    let elim = CandEliminationCtrls {
        enabled: true,
        dc_only_th: 10,
        skip_dc_th: 100,
    };
    let best = Some(Mds0Best {
        cost: 1,
        luma_fast_dist: 500,
    });
    // area 64: non-DC th 640 > 500 -> eliminated; DC th 6400 > 500 too.
    assert!(intra_candidate_eliminated(&elim, best, true, false, (8, 8)));
    assert!(intra_candidate_eliminated(&elim, best, true, true, (8, 8)));
    // area 16: non-DC th 160 < 500 -> kept; DC th 1600 > 500 -> dropped.
    assert!(!intra_candidate_eliminated(
        &elim,
        best,
        true,
        false,
        (4, 4)
    ));
    assert!(intra_candidate_eliminated(&elim, best, true, true, (4, 4)));
}

/// `:1018` — inter candidates and a disabled control never eliminate,
/// and neither does the first candidate (no best yet).
#[test]
fn the_intra_elimination_needs_all_three_preconditions() {
    let elim = CandEliminationCtrls {
        enabled: true,
        dc_only_th: 1_000_000,
        skip_dc_th: 1_000_000,
    };
    let best = Some(Mds0Best {
        cost: 1,
        luma_fast_dist: 1,
    });
    assert!(intra_candidate_eliminated(&elim, best, true, false, (8, 8)));
    assert!(!intra_candidate_eliminated(
        &elim,
        best,
        false,
        false,
        (8, 8)
    ));
    let off = CandEliminationCtrls {
        enabled: false,
        ..elim
    };
    assert!(!intra_candidate_eliminated(&off, best, true, false, (8, 8)));
    assert!(!intra_candidate_eliminated(
        &elim,
        None,
        true,
        false,
        (8, 8)
    ));
}

/// `:1044-1046` — the stored distortion is the RAW variance and
/// `full_dist` is the SSE, which are different numbers.
#[test]
fn the_stored_distortion_is_unshifted_and_full_dist_is_the_sse() {
    // 4x4: prediction constant 100, source constant 110 -> every diff
    // is -10, so sse = 16 * 100 = 1600 and variance = 1600 - (-160)^2/16
    // = 1600 - 1600 = 0.
    let pred = [100u8; 16];
    let src = [110u8; 16];
    let out = fast_loop_core_light_pd1(
        None,
        (4, 4),
        0,
        true,
        plane(&src, 4),
        plane(&pred, 4),
        |_| unreachable!("shut_fast_rate skips the rate function"),
    );
    let FastLoopOutcome::Scored(s) = out else {
        panic!("must score");
    };
    assert_eq!(s.luma_fast_dist, 0, "constant offset has zero variance");
    assert_eq!(s.full_dist, 1600, "but a real SSE");
    assert_eq!(s.fast_cost, 0, "shut_fast_rate -> cost is the shifted dist");
}

/// `:1058-1061` — `shut_fast_rate` makes the cost the SHIFTED
/// distortion directly, with no `RDCOST` wrapper.
#[test]
fn shut_fast_rate_uses_the_shifted_distortion_as_the_cost() {
    // 2x2 with a checkerboard: pred {0,255,255,0}, src all 0.
    let pred = [0u8, 255, 255, 0];
    let src = [0u8; 4];
    let out = fast_loop_core_light_pd1(
        None,
        (2, 2),
        123,
        true,
        plane(&src, 2),
        plane(&pred, 2),
        |_| unreachable!(),
    );
    let FastLoopOutcome::Scored(s) = out else {
        panic!("must score");
    };
    assert_eq!(s.fast_cost, s.luma_fast_dist << 4);
    assert_ne!(s.fast_cost, 0, "the control must not be vacuous");
}

/// `:1049-1054` — the distortion-only cost is compared against the
/// running best TOTAL, and the eliminated candidate reports the
/// sentinel.
#[test]
fn a_candidate_whose_distortion_alone_loses_is_abandoned() {
    let pred = [0u8, 255, 255, 0];
    let src = [0u8; 4];
    let best = Some(Mds0Best {
        cost: 1,
        luma_fast_dist: 0,
    });
    let out = fast_loop_core_light_pd1(
        best,
        (2, 2),
        0,
        false,
        plane(&src, 2),
        plane(&pred, 2),
        |_| unreachable!("an eliminated candidate is never priced"),
    );
    assert_eq!(
        out,
        FastLoopOutcome::Eliminated {
            reason: EliminationReason::DistortionAloneExceedsTheBest
        }
    );
    assert_eq!(out.cost(), MAX_MODE_COST);
    // A generous best keeps it.
    let generous = Some(Mds0Best {
        cost: u64::MAX / 2,
        luma_fast_dist: 0,
    });
    let kept = fast_loop_core_light_pd1(
        generous,
        (2, 2),
        0,
        false,
        plane(&src, 2),
        plane(&pred, 2),
        |d| (d + 7, 3, 0),
    );
    let FastLoopOutcome::Scored(s) = kept else {
        panic!("must score");
    };
    assert_eq!(s.fast_cost, (s.luma_fast_dist << 4) + 7);
    assert_eq!(s.fast_luma_rate, 3);
}

/// `:7127-7130` — the RDOQ switches are cleared ONLY under
/// `bypass_encdec`.
#[test]
fn mds3_clears_the_rdoq_switches_only_when_encdec_is_bypassed() {
    let bypassed = md_stage_3_light_pd1_settings(true, true, true);
    assert!(!bypassed.rdoq_skip_uv && !bypassed.rdoq_dct_dct_only);
    let normal = md_stage_3_light_pd1_settings(false, true, true);
    assert!(normal.rdoq_skip_uv && normal.rdoq_dct_dct_only);
    // The rest is unconditional.
    for s in [bypassed, normal] {
        assert!(s.mds_do_chroma && s.uv_intra_comp_only && s.mds_do_rdoq);
        assert_eq!(s.mds_fast_coeff_est_level, 1);
        assert_eq!(s.mds_subres_step, 0);
    }
}
