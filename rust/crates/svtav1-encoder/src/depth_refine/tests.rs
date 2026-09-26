use super::*;

#[test]
fn asymmetric_children_cover_the_parent_without_overlap() {
    for size in [16, 32, 64, 128] {
        for shape in [
            PartitionType::HorzA,
            PartitionType::HorzB,
            PartitionType::VertA,
            PartitionType::VertB,
        ] {
            let children = shape_children(size, shape);
            assert_eq!(children.len(), 3);
            let mut coverage = vec![0u8; size * size];
            for (x, y, w, h) in children {
                assert!(x + w <= size && y + h <= size);
                for row in y..y + h {
                    for col in x..x + w {
                        coverage[row * size + col] += 1;
                    }
                }
            }
            assert!(coverage.iter().all(|&n| n == 1), "{size} {shape:?}");
        }
    }
}

#[test]
fn research_shapes_obey_size_and_edge_constraints() {
    let cfg = NsqCfg::for_arm(crate::sc_detect::ScArm::Allintra, -1, 40);
    assert_eq!(
        shapes_for_size(16, &cfg),
        &[
            PartitionType::None,
            PartitionType::Horz,
            PartitionType::Vert,
            PartitionType::Horz4,
            PartitionType::Vert4,
            PartitionType::HorzA,
            PartitionType::HorzB,
            PartitionType::VertA,
            PartitionType::VertB
        ]
    );
    assert_eq!(shapes_for_size(8, &cfg).len(), 3);
    assert_eq!(shapes_for_size(4, &cfg), &[PartitionType::None]);
    let large = shapes_for_size(128, &cfg);
    assert_eq!(large.len(), 7);
    assert!(!large.contains(&PartitionType::Horz4));
    assert!(!large.contains(&PartitionType::Vert4));
    assert_eq!(
        shapes_at_edge(64, &cfg, true, false, true),
        &[PartitionType::Horz]
    );
    assert_eq!(
        shapes_at_edge(64, &cfg, true, true, false),
        &[PartitionType::Vert]
    );
    assert!(shapes_at_edge(64, &cfg, true, false, false).is_empty());
}

#[test]
fn nsq_cfg_matches_instrumented_captures() {
    // NSQCFG rows (docs/captures/nsq_m2m3/): M3 levels 19/18/16 at
    // qp 20/40/55, M2 levels 17/16/14 — post-tail values (dev - 5).
    let c = NsqCfg::for_arm(crate::sc_detect::ScArm::Allintra, 3, 20);
    assert!(c.enabled && c.allow_hv4 && c.psq_txs);
    assert_eq!(
        (c.sq_weight, c.hv_weight, c.max_part0_to_part1_dev),
        (90, 75, 75)
    );
    assert_eq!((c.nsq_split_cost_th, c.lower_depth_split_cost_th), (35, 20));
    assert_eq!((c.h_vs_v_split_rate_th, c.non_hv_split_rate_th), (85, 70));
    assert_eq!((c.rate_th_offset_lte16, c.component_multiple_th), (15, 5));
    let c = NsqCfg::for_arm(crate::sc_detect::ScArm::Allintra, 3, 40);
    assert_eq!((c.max_part0_to_part1_dev, c.nsq_split_cost_th), (70, 40));
    assert_eq!((c.h_vs_v_split_rate_th, c.non_hv_split_rate_th), (80, 70));
    assert!(c.psq_txs);
    let c = NsqCfg::for_arm(crate::sc_detect::ScArm::Allintra, 3, 55);
    assert_eq!(
        (c.max_part0_to_part1_dev, c.component_multiple_th),
        (45, 15)
    );
    assert!(!c.psq_txs); // level 16
    let c = NsqCfg::for_arm(crate::sc_detect::ScArm::Allintra, 2, 20);
    assert!(c.psq_txs); // level 17
    assert_eq!((c.max_part0_to_part1_dev, c.rate_th_offset_lte16), (45, 15));
    let c = NsqCfg::for_arm(crate::sc_detect::ScArm::Allintra, 2, 40);
    assert!(!c.psq_txs); // level 16
    assert_eq!(c.max_part0_to_part1_dev, 45);
    let c = NsqCfg::for_arm(crate::sc_detect::ScArm::Allintra, 2, 55);
    assert_eq!((c.max_part0_to_part1_dev, c.component_multiple_th), (0, 20));
    assert_eq!((c.sq_weight, c.hv_weight), (95, 100));
    // Presets >= 4: search off.
    assert!(!NsqCfg::for_arm(crate::sc_detect::ScArm::Allintra, 4, 40).enabled);
}

#[test]
fn dr_ctrls_match_capture() {
    // M5DBG CFG enc_mode=4: dr_s1=15 dr_e1=15 dr_maxmult=10
    // dr_bandmod=0 dr_lowsplit=20 dr_splitrate=10 dr_limitpd0=1
    // dr_unavail=2 (docs/captures/m0m5_config_dlf.txt line 14).
    let m4 = DrCtrls::for_preset(4);
    assert!(m4.adaptive);
    assert_eq!((m4.s1_th, m4.e1_th), (15, 15));
    assert_eq!(m4.parent_max_cost_mult, 10);
    assert!(!m4.band_mod);
    assert_eq!((m4.lower_split_th, m4.split_rate_th), (20, 10));
    assert_eq!((m4.limit_to_pd0, m4.unavail_mode), (1, 2));
    // enc_mode=5: dr_s1=10 dr_e1=10 dr_maxmult=0 dr_bandmod=1
    // dr_maxcostmult=400 dr_bands=4 dr_lowsplit=100 dr_splitrate=5
    // dr_unavail=0.
    let m5 = DrCtrls::for_preset(5);
    assert!(m5.adaptive);
    assert_eq!((m5.s1_th, m5.e1_th), (10, 10));
    assert_eq!(m5.parent_max_cost_mult, 0);
    assert!(m5.band_mod);
    assert_eq!((m5.max_cost_multiplier, m5.max_band_cnt), (400, 4));
    assert_eq!(m5.decrement_per_band, [i64::MAX, i64::MAX, 10, 5]);
    assert_eq!((m5.lower_split_th, m5.split_rate_th), (100, 5));
    assert_eq!((m5.limit_to_pd0, m5.unavail_mode), (1, 0));
    // M6+ collapses to PRED_PART_ONLY.
    assert!(!DrCtrls::for_preset(6).adaptive);
}

#[test]
fn dr_ctrls_sc_class5_level_mapping() {
    // C allintra derivation (enc_mode_config.c:10067-10090), verified
    // against the instrumented `ctx->depth_refinement_ctrls` dump:
    //   graph (sc_class5):  p0/p1 -> lvl1, p2 -> lvl5, p3/p4 -> lvl6, p5 -> lvl9
    //   codec_wiki (!sc):   p0..p4 -> lvl6, p5 -> lvl9
    // sc_class5 M0/M1 = level 1: s1=e1=200, s2=e2=0 (NOT the sentinel),
    // split_rate_th=0, limit=0.
    for p in [0i8, 1] {
        let c = DrCtrls::for_preset_sc(p, true);
        assert!(c.adaptive);
        assert_eq!((c.s1_th, c.e1_th), (200, 200));
        assert_eq!((c.s2_th, c.e2_th), (0, 0), "level 1 s2/e2 are literal 0");
        assert_eq!((c.split_rate_th, c.limit_to_pd0), (0, 0));
    }
    // sc_class5 M2 = level 5: s1=e1=30, s2=e2=sentinel, limit=2, lower=10.
    let c = DrCtrls::for_preset_sc(2, true);
    assert!(c.adaptive);
    assert_eq!((c.s1_th, c.e1_th), (30, 30));
    assert_eq!((c.s2_th, c.e2_th), (i64::MIN, i64::MIN));
    assert_eq!((c.lower_split_th, c.limit_to_pd0), (10, 2));
    // sc_class5 M3/M4 = level 6, same as the !sc row.
    for p in [3i8, 4] {
        let sc = DrCtrls::for_preset_sc(p, true);
        assert_eq!((sc.s1_th, sc.e1_th), (15, 15));
        assert_eq!((sc.limit_to_pd0, sc.lower_split_th), (1, 20));
    }
    // sc_class5 M5 = level 9 (same band-modulated ctrls as !sc M5).
    let sc5 = DrCtrls::for_preset_sc(5, true);
    assert!(sc5.band_mod);
    assert_eq!((sc5.s1_th, sc5.e1_th), (10, 10));
    // !sc_class5 keeps the pre-fix per-preset row for every preset, so the
    // whole non-screen envelope (every mainline gate) is byte-identical.
    for p in 0..=6i8 {
        let a = DrCtrls::for_preset_sc(p, false);
        let b = DrCtrls::for_preset(p);
        assert_eq!(
            (a.s1_th, a.e1_th, a.adaptive),
            (b.s1_th, b.e1_th, b.adaptive)
        );
        assert_eq!(
            (a.limit_to_pd0, a.split_rate_th),
            (b.limit_to_pd0, b.split_rate_th)
        );
    }
    // The screen and non-screen rows differ exactly at M0/M1/M2.
    for p in [0i8, 1, 2] {
        assert_ne!(
            DrCtrls::for_preset_sc(p, true).e1_th,
            DrCtrls::for_preset_sc(p, false).e1_th,
            "sc_class5 must lower e1 at M{p}"
        );
    }
}

/// The identity-harness gradient content (identity_run/content.rs) at 64x64.
fn gradient64() -> alloc::vec::Vec<u8> {
    let (w, h) = (64usize, 64usize);
    let mut y = alloc::vec![0u8; w * h];
    for r in 0..h {
        for c in 0..w {
            y[r * w + c] = (((r * 255) / h) ^ ((c * 3) & 0x3f)) as u8;
        }
    }
    y
}

/// Refined-scan shape pins vs the instrumented M5DBG WIN dumps
/// (docs/captures/m0m5_config_dlf.txt, gradient 64x64 preset 5):
/// - q20/q40: PD0 tree = 64 SPLIT + 4x32 NONE; 16x16 evaluations
///   appear ONLY under the (32,0) quadrant (the child-deviation gate
///   admits the sub-depth for quadrant 1, rejects 0/2/3), and there
///   is NO 64x64 WIN row (the parent depth is not admitted).
/// - q55: PD0 tree = single 64x64 NONE and the WIN dump has ONLY the
///   64x64 row — no 32x32 evaluations (e_depth 0 for the root leaf).
#[test]
fn m5_gradient64_scan_matches_capture() {
    let y = gradient64();
    let ctrls = DrCtrls::for_preset(5);
    for (qp, qindex, lambda) in [(20u32, 80u8, 25650u64), (40, 160, 248207)] {
        let tables = crate::pd0::build_m6_pd0_tables(qindex);
        let eval = crate::pd0::pd0_pick_sb_partition_m6_eval(
            &y,
            64,
            0,
            0,
            qp,
            qindex,
            crate::pd0::frame_lambda_weight(qp, false, 0),
            &tables,
            8,
            1,
            crate::pd0::Pd0Mode::Lvl1,
            1000,
            false,
            true,
            64,
            64,
            0,
            0,
            None,
            64,
            None,
            // These are KEY-frame scans (no reference exists in a unit test).
            None,
            None,
            None,
            None,
            0.0,
        );
        assert!(eval.split, "q{qp}: PD0 splits the 64");
        let scan = build_refined_scan(&eval, &ctrls, lambda, &tables);
        assert!(!scan.test_this, "q{qp}: no 64x64 parent-depth eval");
        assert!(scan.split_flag);
        let ch = scan.children.as_ref().unwrap();
        assert!(ch.iter().all(|c| c.test_this), "q{qp}: all 32s evaluated");
        assert!(
            !ch[0].split_flag && ch[1].split_flag && !ch[2].split_flag && !ch[3].split_flag,
            "q{qp}: 16x16 depth admitted only under (32,0)"
        );
    }
    // q55: 64x64 NONE, no deeper evals.
    let tables = crate::pd0::build_m6_pd0_tables(220);
    let eval = crate::pd0::pd0_pick_sb_partition_m6_eval(
        &y,
        64,
        0,
        0,
        55,
        220,
        crate::pd0::frame_lambda_weight(55, false, 0),
        &tables,
        8,
        1,
        crate::pd0::Pd0Mode::Lvl1,
        1000,
        false,
        true,
        64,
        64,
        0,
        0,
        None,
        64,
        None,
        // These are KEY-frame scans (no reference exists in a unit test).
        None,
        None,
        None,
        None,
        0.0,
    );
    assert!(!eval.split);
    let scan = build_refined_scan(&eval, &ctrls, 1527856, &tables);
    assert!(scan.test_this && !scan.split_flag && scan.children.is_none());
}

/// M4 (dr level 6) on the same content: the wider e1 threshold (15
/// vs 10) and the M4 leaf funnel own the g128-q20 SB0 (0,32) 32x32
/// -> 4x16 flip the differ chased (byte-identical after this port);
/// at 64x64 the admissions stay quadrant-1-only like M5 (pinned so
/// gate drift is caught without the harness).
#[test]
fn m4_gradient64_scan_shape() {
    let y = gradient64();
    let ctrls = DrCtrls::for_preset(4);
    let tables = crate::pd0::build_m6_pd0_tables(80);
    let eval = crate::pd0::pd0_pick_sb_partition_m6_eval(
        &y,
        64,
        0,
        0,
        20,
        80,
        crate::pd0::frame_lambda_weight(20, false, 0),
        &tables,
        8,
        1,
        crate::pd0::Pd0Mode::Lvl1,
        1000,
        false,
        true,
        64,
        64,
        0,
        0,
        None,
        64,
        None,
        // These are KEY-frame scans (no reference exists in a unit test).
        None,
        None,
        None,
        None,
        0.0,
    );
    assert!(eval.split);
    let scan = build_refined_scan(&eval, &ctrls, 25650, &tables);
    assert!(!scan.test_this && scan.split_flag);
    let ch = scan.children.as_ref().unwrap();
    assert!(ch.iter().all(|c| c.test_this));
    assert!(!ch[0].split_flag && ch[1].split_flag && !ch[2].split_flag && !ch[3].split_flag);
}

#[test]
fn pred_part_only_scan_equals_pd0_tree() {
    // A PRED_PART_ONLY refinement must mark exactly the PD0 leaves.
    let eval = Pd0Eval {
        root_det: None,
        sq: 64,
        tested: true,
        sq_tested: true,
        cost: 100,
        split: true,
        off: false,
        children: Some(Box::new([
            Pd0Eval {
                root_det: None,
                sq: 32,
                tested: true,
                sq_tested: true,
                cost: 25,
                split: false,
                off: false,
                children: None,
            },
            Pd0Eval {
                root_det: None,
                sq: 32,
                tested: true,
                sq_tested: true,
                cost: 25,
                split: false,
                off: false,
                children: None,
            },
            Pd0Eval {
                root_det: None,
                sq: 32,
                tested: true,
                sq_tested: true,
                cost: 25,
                split: false,
                off: false,
                children: None,
            },
            Pd0Eval {
                root_det: None,
                sq: 32,
                tested: true,
                sq_tested: true,
                cost: 25,
                split: false,
                off: false,
                children: None,
            },
        ])),
    };
    let ctrls = DrCtrls::for_preset(6);
    let tables = crate::pd0::build_m6_pd0_tables(160);
    let scan = build_refined_scan(&eval, &ctrls, 248207, &tables);
    assert!(!scan.test_this && scan.split_flag);
    for c in scan.children.as_ref().unwrap().iter() {
        assert!(c.test_this && !c.split_flag && c.children.is_none());
    }
}

/// `max_sq_size` must follow `static_config.max_tx_size`
/// (enc_dec_process.c:1814-1817), not the literal 64 it was hardcoded to.
///
/// C caps `max_sq_size` to 32 when `max_tx_size == 32`, which the port
/// already derives at tune IQ with qp <= 45 (`hdr_mode.rs`) and threads
/// into every PD0 entry. With the cap missing, a 32x32 node was allowed
/// `s_depth = -1` (test a 64x64 parent) where C forces 0, admitting a
/// shallower depth C never tests.
///
/// ANTI-VACUITY: this asserts the two `max_tx_size` values produce
/// DIFFERENT scans. With the old hardcoded 64 both arms are identical and
/// the final assert fails.
#[test]
fn max_sq_size_follows_max_tx_size() {
    // A 32x32 PD0 leaf that was tested and not split: `set_start_end_depth`
    // may hand it a parent depth unless `sq == max_sq`.
    let leaf32 = |cost: u64| Pd0Eval {
        root_det: None,
        sq: 32,
        tested: true,
        sq_tested: true,
        cost,
        split: false,
        off: false,
        children: None,
    };
    let eval = Pd0Eval {
        root_det: None,
        sq: 64,
        tested: true,
        sq_tested: true,
        cost: 100,
        split: true,
        off: false,
        children: Some(Box::new([leaf32(25), leaf32(25), leaf32(25), leaf32(25)])),
    };
    // A preset whose refinement mode is ADAPTIVE (so s_depth can be nonzero
    // at all) -- presets 0-5 per DrCtrls::for_preset.
    let ctrls = DrCtrls::for_preset(4);
    let tables = crate::pd0::build_m6_pd0_tables(160);

    let scan64 = build_refined_scan_at(
        &eval,
        &ctrls,
        248207,
        &tables,
        0,
        0,
        None,
        64,
        64,
        None,
        true,
        crate::quant::CoeffLvl::Normal,
        ctrls.disallow_4x4,
        false,
        crate::port_enc_mode_config::common::DepthRemovalCtrls::default(),
    );
    let scan32 = build_refined_scan_at(
        &eval,
        &ctrls,
        248207,
        &tables,
        0,
        0,
        None,
        32,
        64,
        None,
        true,
        crate::quant::CoeffLvl::Normal,
        ctrls.disallow_4x4,
        false,
        crate::port_enc_mode_config::common::DepthRemovalCtrls::default(),
    );

    // At max_tx_size 32 the 32x32 nodes ARE the max square, so C forces
    // s_depth = 0 -- they must not request their 64x64 parent.
    let parent_tested = |sc: &RefScan| sc.test_this;
    assert!(
        parent_tested(&scan64) != parent_tested(&scan32),
        "max_tx_size must change whether the 64x64 parent is admitted \
             (64 -> {}, 32 -> {}); if these agree, the max_sq cap is not wired",
        parent_tested(&scan64),
        parent_tested(&scan32)
    );
    assert!(
        !parent_tested(&scan32),
        "at max_tx_size 32 a 32x32 node is the max square: C forces s_depth = 0"
    );
}

/// C `set_start_end_depth`'s per-SB depth-removal clamp
/// (enc_dec_process.c:1799-1813): `disallow_below_*` caps `e_depth` at
/// the granularity it names, BEFORE adaptive narrowing. This is the gate
/// that was missing: `DepthRemovalResult` was computed per SB for the
/// PD0 entries but never reached `RefineEnv`, so a 16x16 leaf under
/// `disallow_below_16x16` still tested its 8x8 children (vidyo1 256x256
/// p6 frame 1, SB at org (128,0)).
#[test]
fn depth_removal_clamps_e_depth() {
    let ctrls = DrCtrls::for_level(5, false);
    let tables = crate::pd0::build_m6_pd0_tables(160);
    // Cost in the range the real cell had (~75M): low enough that
    // `band_mod`/`split_rate_th` stay quiet, so the e_depth answer
    // reflects the clamp under test.
    let leaf = |sq: usize| Pd0Eval {
        root_det: None,
        sq,
        tested: true,
        sq_tested: true,
        cost: 75_000_000,
        split: false,
        off: false,
        children: None,
    };
    let env = |dr: crate::port_enc_mode_config::common::DepthRemovalCtrls| RefineEnv {
        ctrls: &ctrls,
        disallow_4x4: false,
        disallow_8x8: false,
        depth_removal: dr,
        lambda: 248207,
        tables: &tables,
        max_pd0: 32,
        min_pd0: 16,
        max_sq: 64,
        sb_sq: 64,
        ref_min_max_sq: None,
        is_islice: false,
        coeff_lvl: crate::quant::CoeffLvl::Normal,
    };
    let dr = |b64: u8, b32: u8, b16: u8| crate::port_enc_mode_config::common::DepthRemovalCtrls {
        enabled: 1,
        disallow_below_64x64: b64,
        disallow_below_32x32: b32,
        disallow_below_16x16: b16,
        disallow_4x4: 0,
    };

    // Disabled: the leaf's e_depth survives the unavail-mode fallback.
    let e = set_start_end_depth(&env(dr(0, 0, 0)), &leaf(16), None, 0, 0).1;
    assert!(
        e > 0,
        "sanity: without depth removal the children stay live"
    );

    // disallow_below_16x16: sq<=16 -> 0; 32 -> <=1; 64 -> <=2.
    assert_eq!(
        set_start_end_depth(&env(dr(0, 0, 1)), &leaf(16), None, 0, 0).1,
        0
    );
    assert_eq!(
        set_start_end_depth(&env(dr(0, 0, 1)), &leaf(32), None, 0, 0).1,
        1
    );
    // disallow_below_32x32: sq<=32 -> 0.
    assert_eq!(
        set_start_end_depth(&env(dr(0, 1, 0)), &leaf(32), None, 0, 0).1,
        0
    );
    assert_eq!(
        set_start_end_depth(&env(dr(0, 1, 1)), &leaf(32), None, 0, 0).1,
        0
    );
    // disallow_below_64x64 takes precedence over the 32/16 flags
    // (C's if/else-if chain) and zeroes a 64x64 node.
    assert_eq!(
        set_start_end_depth(&env(dr(1, 1, 1)), &leaf(64), None, 0, 0).1,
        0
    );
    // Below-16 alone still lets the 64x64 node admit a sub-depth
    // (coeff_lvl_mod caps at 1 on this ctrls row).
    assert_eq!(
        set_start_end_depth(&env(dr(0, 0, 1)), &leaf(64), None, 0, 0).1,
        1
    );
}
