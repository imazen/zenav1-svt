use super::*;

fn ctrls(score_th: i32, dist_th: u16, rd_th: u16) -> Lpd1TxSkipDecisionCtrls {
    Lpd1TxSkipDecisionCtrls {
        skip_tx_score_th: score_th,
        dist_energy_th: dist_th,
        rd_skip_th: rd_th,
    }
}

fn flat_rd() -> SkipRdInputs {
    SkipRdInputs {
        full_lambda: 0,
        fast_luma_rate: 0,
        skip_fac_bits_coded: 0,
        skip_fac_bits_skip: 0,
        full_dist: 0,
    }
}

/// Tier 4: traced against `product_coding_loop.c:6331-6337`.
#[test]
fn intra_and_disabled_thresholds_always_transform() {
    let c = ctrls(125, 30, 100);
    assert!(should_perform_tx(
        false,
        &c,
        0,
        16,
        16,
        100,
        None,
        flat_rd(),
        false
    ));
    let off = ctrls(0, 30, 100);
    assert!(should_perform_tx(
        true,
        &off,
        0,
        16,
        16,
        100,
        None,
        flat_rd(),
        false
    ));
}

/// The neighbour bonuses are additive, and both-flags scores 50.
#[test]
fn neighbour_bonuses_stack_to_fifty() {
    // dist gate passes (0 < anything), rd gate off, qp >= 128 so no bias.
    let c = ctrls(1000, 30, 0);
    let score_for = |n: Option<Neighbours>| {
        // Recover the score by sweeping the threshold: the function
        // returns `score < th`. The sweep starts at 1 because a
        // threshold of 0 short-circuits to `true` (`:6335`).
        (1..=1000i32)
            .find(|&th| {
                should_perform_tx(true, &ctrls(th, 30, 0), 0, 16, 16, 200, n, flat_rd(), false)
            })
            .unwrap()
    };
    let _ = c;
    assert_eq!(score_for(None), 51, "50 + 1 for the strict `<`");
    assert_eq!(
        score_for(Some(Neighbours {
            both_skip: true,
            both_nearest: false
        })),
        71
    );
    assert_eq!(
        score_for(Some(Neighbours {
            both_skip: false,
            both_nearest: true
        })),
        66
    );
    assert_eq!(
        score_for(Some(Neighbours {
            both_skip: true,
            both_nearest: true
        })),
        101,
        "50 + 20 + 15 + 15"
    );
}

/// The QP bias can drive the score NEGATIVE — the case an unsigned
/// score would wrap on, inverting the decision.
#[test]
fn low_qp_luma_dominant_bias_can_go_negative() {
    // dist_energy_th 0 makes the dist gate `x < 0`, always false, so
    // the score enters the bias at 0. rd gate off. score -> -200.
    let c = ctrls(-100, 0, 0);
    // score = -200 < -100 -> skip the transform.
    assert!(should_perform_tx(
        true,
        &c,
        1000,
        16,
        16,
        10,
        None,
        flat_rd(),
        true
    ));
    // With the same threshold but a non-luma-dominant input the bias is
    // only -20, which is NOT below -100.
    assert!(!should_perform_tx(
        true,
        &c,
        1000,
        16,
        16,
        10,
        None,
        flat_rd(),
        false
    ));
}

#[test]
fn skip_luma_rd_prefers_skip_when_the_margin_holds() {
    // non_skip dist 1000, skip dist 1200, no rate. pct 100 -> compare
    // 1200*128*100 vs 1000*128*100: skip loses.
    assert_eq!(
        blk_skip_luma_rd(0, 0, 0, 0, 1000, 1200, 100),
        SkipLumaOutcome::KeepResidual
    );
    // pct 50 halves the skip cost -> 1200*128*50 < 1000*128*100.
    assert_eq!(
        blk_skip_luma_rd(0, 0, 0, 0, 1000, 1200, 50),
        SkipLumaOutcome::CommitSkip
    );
}

fn planes<'a>(y: &'a [u8], u: &'a [u8], v: &'a [u8], stride: usize) -> BlockPlanes<'a> {
    BlockPlanes {
        y: Plane::new(y, stride),
        u: Plane::new(u, stride),
        v: Plane::new(v, stride),
    }
}

#[test]
fn chroma_energy_skip_never_reintroduces_a_dropped_plane() {
    let geom = UvGeom {
        bwidth_uv: 4,
        bheight_uv: 4,
        bsize_uv: 0,
    };
    let zeros = [0u8; 64];
    let ones = [255u8; 64];
    // Cr is already dropped (component == Cb): even though the V planes
    // differ wildly, only Cb is measured — and it matches, so it passes.
    let input = planes(&zeros, &zeros, &zeros, 4);
    let pred = planes(&zeros, &zeros, &ones, 4);
    assert_eq!(
        chroma_energy_skip(ComponentType::Cb, geom, input, pred, 8, true),
        ChromaEnergyOutcome::DropBothChroma
    );
    // With both planes in the set the V mismatch keeps Cr alive.
    assert_eq!(
        chroma_energy_skip(ComponentType::Chroma, geom, input, pred, 8, true),
        ChromaEnergyOutcome::DropCb
    );
}

#[test]
fn chroma_energy_skip_commits_a_block_skip_only_without_luma_coeffs() {
    let geom = UvGeom {
        bwidth_uv: 4,
        bheight_uv: 4,
        bsize_uv: 0,
    };
    let zeros = [0u8; 64];
    let p = planes(&zeros, &zeros, &zeros, 4);
    assert_eq!(
        chroma_energy_skip(ComponentType::Chroma, geom, p, p, 8, false),
        ChromaEnergyOutcome::BlockSkip
    );
    assert_eq!(
        chroma_energy_skip(ComponentType::Chroma, geom, p, p, 8, true),
        ChromaEnergyOutcome::DropBothChroma
    );
}

#[test]
fn merge_promotes_but_never_demotes() {
    assert_eq!(
        merge_component(ComponentType::Cr, ComponentType::Cb),
        ComponentType::Chroma
    );
    assert_eq!(
        merge_component(ComponentType::Cb, ComponentType::Cb),
        ComponentType::Cb
    );
    assert_eq!(
        merge_component(ComponentType::Luma, ComponentType::Chroma),
        ComponentType::Chroma
    );
}

#[test]
fn globalmv_bypass_needs_every_precondition() {
    let ok = |th, i_slice, sq, refmvs, gm, mv, cost| {
        globalmv_bypass_allowed(i_slice, th, sq, refmvs, gm, mv, cost, 16, 16)
    };
    assert!(ok(10, false, true, false, true, true, 100));
    assert!(!ok(10, true, true, false, true, true, 100), "I slice");
    assert!(!ok(0, false, true, false, true, true, 100), "th 0");
    assert!(!ok(10, false, false, false, true, true, 100), "non-square");
    assert!(!ok(10, false, true, true, true, true, 100), "ref frame mvs");
    assert!(!ok(10, false, true, false, false, true, 100), "warped gm");
    assert!(
        !ok(10, false, true, false, true, false, 100),
        "nonzero me mv"
    );
    // 10 * 256 = 2560; the sentinel and anything at/above the bound fail.
    assert!(!ok(10, false, true, false, true, true, 2560));
    assert!(ok(10, false, true, false, true, true, 2559));
    assert!(!ok(10, false, true, false, true, true, u32::MAX));
}

#[test]
fn variance_detector_threshold_is_level_keyed() {
    let geom = UvGeom {
        bwidth_uv: 8,
        bheight_uv: 8,
        bsize_uv: 0, // 4x4 log2 pels = 4; deliberately small so the
                     // normalisation leaves a large per-pixel value
    };
    // A checkerboard of 0 / 255 against the flat-128 reference has a
    // large variance; a constant-128 block has none.
    let mut checker = [0u8; 64];
    for (i, p) in checker.iter_mut().enumerate() {
        *p = if i % 2 == 0 { 0 } else { 255 };
    }
    let flat = [128u8; 64];
    let busy = planes(&flat, &checker, &checker, 8);
    let calm = planes(&flat, &flat, &flat, 8);
    assert_eq!(
        chroma_complexity_check_variance(0, geom, busy),
        Some(ComponentType::Chroma)
    );
    assert_eq!(chroma_complexity_check_variance(0, geom, calm), None);
    // Mixed: Cb busy, Cr calm.
    let mixed = BlockPlanes {
        y: Plane::new(&flat, 8),
        u: Plane::new(&checker, 8),
        v: Plane::new(&flat, 8),
    };
    assert_eq!(
        chroma_complexity_check_variance(0, geom, mixed),
        Some(ComponentType::Cb)
    );
}
