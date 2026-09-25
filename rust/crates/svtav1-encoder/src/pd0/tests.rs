use super::*;

/// The identity-harness gradient content (identity_run.rs).
fn gradient64() -> Vec<u8> {
    let (w, h) = (64usize, 64usize);
    let mut y = vec![0u8; w * h];
    for r in 0..h {
        for c in 0..w {
            y[r * w + c] = (((r * 255) / h) ^ ((c * 3) & 0x3f)) as u8;
        }
    }
    y
}

/// C variance map for gradient-64, captured from the instrumented
/// library (MDBG sb_var, docs/IDENTITY-STATUS.md 2026-07-13).
const C_GRADIENT64_VARS: [u16; 85] = [
    5425, 1343, 1353, 1733, 1893, 336, 341, 340, 338, 645, 773, 837, 901, 645, 773, 837, 901, 645,
    773, 837, 901, 79, 163, 395, 83, 79, 487, 155, 83, 197, 503, 181, 325, 357, 171, 469, 229, 197,
    1099, 1717, 325, 1573, 1047, 661, 1957, 197, 503, 181, 325, 357, 171, 469, 229, 197, 1099,
    1717, 325, 1573, 1047, 661, 1957, 197, 503, 181, 325, 357, 171, 469, 229, 197, 1099, 1717, 325,
    1573, 1047, 661, 1957, 197, 503, 181, 325, 357, 171, 469, 229,
];

#[test]
fn variance_map_matches_c() {
    let y = gradient64();
    let v = compute_b64_variance(&y, 64, 0, 0);
    assert_eq!(v.0, C_GRADIENT64_VARS);
}

/// The dispatched stats kernel must equal the scalar reference under every
/// token permutation, over varied content and SB origins/strides.
#[test]
fn b64_stats_all_tiers_match_scalar() {
    use archmage::testing::{CompileTimePolicy, for_each_token_permutation};
    let st = ScalarToken::summon().unwrap();
    let report = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_| {
        for (w, h, stride, ox, oy) in [
            (64usize, 64usize, 64usize, 0usize, 0usize),
            (128, 64, 128, 64, 0),
            (64, 128, 72, 0, 64),
            (128, 128, 136, 32, 24),
        ] {
            let src: Vec<u8> = (0..stride * (h + 8))
                .map(|i| (i as u32 * 61 + (i / stride) as u32 * 17 + 3) as u8)
                .collect();
            let mut m_ref = [0u64; 64];
            let mut s_ref = [0u64; 64];
            b64_stats_impl_scalar(st, &src, stride, ox, oy, &mut m_ref, &mut s_ref);
            let mut m_got = [0u64; 64];
            let mut s_got = [0u64; 64];
            incant!(
                b64_stats_impl(&src, stride, ox, oy, &mut m_got, &mut s_got),
                [v3, neon, scalar]
            );
            assert_eq!(m_got, m_ref, "mean8 {w}x{h}s{stride}@({ox},{oy})");
            assert_eq!(s_got, s_ref, "msq8 {w}x{h}s{stride}@({ox},{oy})");
        }
    });
    assert!(report.warnings.is_empty());
}

#[test]
fn qp_scaling_factors_match_c() {
    // Linear branch (MDBG qw prints) + the exp branch at qp 55
    // (9146/10000, from the observed var cap and detector behavior).
    assert_eq!(qp_th_scaling_factors(20), (20, 63));
    assert_eq!(qp_th_scaling_factors(40), (40, 63));
    assert_eq!(qp_th_scaling_factors(55), (9146, 10000));
}

#[test]
fn lambda_matches_c() {
    // MDBG split_enter lambda prints: qindex 80/160/220 (CLI qp
    // 20/40/55 through quantizer_to_qindex).
    assert_eq!(kf_full_lambda_8bit(80, 20), 25650);
    assert_eq!(kf_full_lambda_8bit(160, 40), 248207);
    assert_eq!(kf_full_lambda_8bit(220, 55), 1527856);
}

#[test]
fn rate_constants_match_c() {
    // MDBG pd0_cand: skip_fac_bits[0][0]=26, partition_fac_bits[0][NONE]=400;
    // split_enter above_split_rate (post-double): 2390@64, 2930@32, 4040@16.
    assert_eq!(skip0_bits(), 26);
    assert_eq!(partition_none_bits_ctx0(), 400);
    assert_eq!(2 * partition_split_bits(64), 2390);
    assert_eq!(2 * partition_split_bits(32), 2930);
    assert_eq!(2 * partition_split_bits(16), 4040);
}

#[test]
fn max_block_size_and_detector_match_c() {
    let y = gradient64();
    let v = compute_b64_variance(&y, 64, 0, 0);
    // MDBG: 64x64 depth excluded at q20/q40 (max 32), included at q55.
    assert_eq!(max_block_size_allintra(v.0[0], 20), 32);
    assert_eq!(max_block_size_allintra(v.0[0], 40), 32);
    assert_eq!(max_block_size_allintra(v.0[0], 55), 64);
    // MDBG: pd0_level 6 at q20, demoted to 5 at q40/q55.
    assert!(!pd0_detector_allintra_demotes(&v, 20));
    assert!(pd0_detector_allintra_demotes(&v, 40));
    assert!(pd0_detector_allintra_demotes(&v, 55));
    // Uniform content: all-zero variance map always demotes.
    let u = vec![128u8; 64 * 64];
    let vu = compute_b64_variance(&u, 64, 0, 0);
    assert_eq!(vu.0, [0u16; 85]);
    assert!(pd0_detector_allintra_demotes(&vu, 40));
    assert_eq!(max_block_size_allintra(0, 20), 64);
}

#[test]
fn lvl0_block_costs_match_c() {
    // C `svt_aom_full_cost_pd0` per-block RD, gradient-64 q20 (qindex 80),
    // captured from the REAL library's SVT_PD0COST_OUT wrap at bd10 (PD0
    // runs at 8-bit, hbd_md forced 0). Closed-form coeff rate (5000 +
    // 100*eob, coeff_rate_est_lvl 0), qindex+8 quant (lpd0_qp_offset 8),
    // subres OFF, 8-bit lambda 25650.
    let y = gradient64();
    let mut ctx = Pd0Ctx {
        src: &y,
        stride: 64,
        sb_x: 0,
        sb_y: 0,
        aligned_w: 64,
        aligned_h: 64,
        vars: compute_b64_variance(&y, 64, 0, 0),
        qp: 20,
        qindex: 80,
        qm_level: 15,
        lambda: kf_full_lambda_8bit(80, 20) as u64,
        mode: Pd0Mode::Lvl0,
        coeff_rate_est_lvl: 0,
        ac_bias_eff: 0.0,
        lvl1: None,
        max_sq: 32,
        min_sq: 8,
        is_subres_safe: 0, // subres off
        subres_step: 0,
        ires_factor: 0,
        accurate_part_ctx: true,
        depth_early_exit_th: 1000,
        parent_cost_bias: 1000,
        nsq_enabled: false,
        tile_top: 0,
        tile_left: 0,
        recon_canvas: None,
        inter: None,
        pending_recon: None,
        scratch: Pd0Scratch::default(),
        root_det: Pd0RootDet {
            rd_cost: 0,
            blk: None,
        },
    };
    assert_eq!(ctx.lambda, 25650);
    // (sq, org_x, org_y, C full_cost)
    for (sq, ox, oy, cost) in [
        (32usize, 0usize, 0usize, 26185862u64),
        (16, 0, 0, 8396609),
        (8, 0, 0, 2143413),
        (8, 8, 0, 1990844),
        (8, 0, 8, 2225589),
        (8, 8, 8, 2168757),
        (16, 16, 0, 6559425),
        (16, 0, 16, 8443329),
        (16, 16, 16, 8792001),
        (32, 32, 0, 28871046),
        (32, 0, 32, 22111622),
        (32, 32, 32, 22521222),
    ] {
        assert_eq!(ctx.lvl0_block_cost(sq, ox, oy), cost, "sq={sq} ({ox},{oy})");
    }
}

#[test]
fn lvl0_gradient64_tree_matches_c() {
    // C bd10 CTREE (svt_aom_update_mi_map wrap): gradient-64 q20 p10 codes
    // 4x BLOCK_32X32 PARTITION_NONE (the LVL_0 full-RD keeps the 32x32
    // parent where the LVL_6 heuristic over-splits to 16x 16x16). The
    // 64x64 force-splits (var64 5425 > qp-scaled cap -> max_sq 32).
    let y = gradient64();
    let tree = pd0_pick_sb_partition_lvl0(
        &y,
        64,
        0,
        0,
        20,
        80,
        frame_lambda_weight(20, false, 0),
        15,
        0,
        64,
        64,
        None,
        64,
        None,
        0.0,
    );
    assert_eq!(tree.leaf_sizes(), vec![32, 32, 32, 32]);
    // q40 / q55 keep the same 4x32 shape here (the parent still wins);
    // q55's 64x64 is IN the depth set (max_sq 64) and PARENT wins outright
    // -> a single 64x64 leaf.
    let t55 = pd0_pick_sb_partition_lvl0(
        &y,
        64,
        0,
        0,
        55,
        220,
        frame_lambda_weight(55, false, 0),
        15,
        0,
        64,
        64,
        None,
        64,
        None,
        0.0,
    );
    assert_eq!(t55.leaf_sizes(), vec![64]);
}

#[test]
fn lvl6_costs_match_c() {
    // MDBG vlpd0cost lines, gradient-64 q20 (PD0_LVL_6).
    let y = gradient64();
    let v = compute_b64_variance(&y, 64, 0, 0);
    for (sq, ox, oy, cost) in [
        (32usize, 0usize, 0usize, 1382u64),
        (16, 0, 0, 294),
        (8, 0, 0, 87),
        (8, 8, 0, 89),
        (8, 0, 8, 89),
        (8, 8, 8, 89),
        (16, 16, 0, 294),
        (16, 0, 16, 313),
        (16, 16, 16, 320),
        (32, 32, 0, 1382),
    ] {
        assert_eq!(
            lvl6_cost_allintra(&v, sq, ox, oy, 20),
            cost,
            "sq={sq} ({ox},{oy})"
        );
    }
}

#[test]
fn lvl5_block_costs_match_c_q40() {
    // MDBG pd0_full_cost / tx_pd0_out, gradient-64 q40 (qindex 160,
    // PD0_LVL_5, subres forced off: no 64x64 block in the depth set).
    let y = gradient64();
    let mut ctx = Pd0Ctx {
        src: &y,
        stride: 64,
        sb_x: 0,
        sb_y: 0,
        aligned_w: 64,
        aligned_h: 64,
        vars: compute_b64_variance(&y, 64, 0, 0),
        qp: 40,
        qindex: 160,
        qm_level: 15,
        lambda: kf_full_lambda_8bit(160, 40) as u64,
        mode: Pd0Mode::Lvl5,
        coeff_rate_est_lvl: 0,
        ac_bias_eff: 0.0,
        lvl1: None,
        max_sq: 32,
        min_sq: 8,
        is_subres_safe: 255,
        subres_step: 1,
        ires_factor: 0,
        accurate_part_ctx: true,
        depth_early_exit_th: 1000,
        parent_cost_bias: 1000,
        nsq_enabled: false,
        tile_top: 0,
        tile_left: 0,
        recon_canvas: None,
        inter: None,
        pending_recon: None,
        scratch: Pd0Scratch::default(),
        root_det: Pd0RootDet {
            rd_cost: 0,
            blk: None,
        },
    };
    for (sq, ox, oy, cost) in [
        (32usize, 0usize, 0usize, 187677438u64),
        (16, 0, 0, 48981821),
        (8, 0, 0, 9695714),
        (8, 8, 0, 11371661),
        (8, 0, 8, 16542374),
        (8, 8, 8, 20538852),
        (16, 16, 0, 41852989),
        (32, 32, 0, 190877950),
        (32, 0, 32, 181407102),
        (32, 32, 32, 183892222),
        (16, 48, 16, 53455823),
    ] {
        assert_eq!(ctx.lvl5_block_cost(sq, ox, oy), cost, "sq={sq} ({ox},{oy})");
    }
}

#[test]
fn lvl5_block_costs_match_c_q55_with_subres() {
    // MDBG, gradient-64 q55 (qindex 220): the 64x64 block runs the
    // odd/even check (safe=1) and everything uses subres step 1.
    let y = gradient64();
    let mut ctx = Pd0Ctx {
        src: &y,
        stride: 64,
        sb_x: 0,
        sb_y: 0,
        aligned_w: 64,
        aligned_h: 64,
        vars: compute_b64_variance(&y, 64, 0, 0),
        qp: 55,
        qindex: 220,
        qm_level: 15,
        lambda: kf_full_lambda_8bit(220, 55) as u64,
        mode: Pd0Mode::Lvl5,
        coeff_rate_est_lvl: 0,
        ac_bias_eff: 0.0,
        lvl1: None,
        max_sq: 64,
        min_sq: 8,
        is_subres_safe: 255,
        subres_step: 1,
        ires_factor: 0,
        accurate_part_ctx: true,
        depth_early_exit_th: 1000,
        parent_cost_bias: 1000,
        nsq_enabled: false,
        tile_top: 0,
        tile_left: 0,
        recon_canvas: None,
        inter: None,
        pending_recon: None,
        scratch: Pd0Scratch::default(),
        root_det: Pd0RootDet {
            rd_cost: 0,
            blk: None,
        },
    };
    assert_eq!(ctx.lvl5_block_cost(64, 0, 0), 1708208432);
    assert_eq!(
        ctx.is_subres_safe, 1,
        "64x64 DC pred must pass the odd/even check"
    );
    for (sq, ox, oy, cost) in [
        (32usize, 0usize, 0usize, 522128378u64),
        (16, 0, 0, 137213980),
        (16, 16, 0, 135635996),
        (16, 0, 16, 232128024),
        (16, 16, 16, 194500372),
        (32, 32, 0, 594523898),
        (32, 0, 32, 475114621),
        (32, 32, 32, 469165693),
    ] {
        assert_eq!(ctx.lvl5_block_cost(sq, ox, oy), cost, "sq={sq} ({ox},{oy})");
    }
}

#[test]
fn dc_only_safe_matches_c() {
    // Instrumented-C capture (SVT_MDBG2 cand_gen prints, gradient-64,
    // 2026-07-13): q40 32x32 leaves and q20 16x16 leaves all print
    // dc_only=1 safe=1 (candidate set = {DC}); the q55/q40 64x64
    // prints safe=0 (var 5425 >= 2000).
    let y = gradient64();
    let v = compute_b64_variance(&y, 64, 0, 0);
    assert!(!is_dc_only_safe(&v, 64, 0, 0), "64x64: var 5425 >= 2000");
    for (ox, oy) in [(0usize, 0usize), (32, 0), (0, 32), (32, 32)] {
        assert!(is_dc_only_safe(&v, 32, ox, oy), "32x32 ({ox},{oy})");
    }
    for by in 0..4 {
        for bx in 0..4 {
            assert!(
                is_dc_only_safe(&v, 16, bx * 16, by * 16),
                "16x16 ({bx},{by})"
            );
        }
    }
    // 8x8: blk_var < 2000 only (all gradient 8x8 vars are 79..1957).
    for by in 0..8 {
        for bx in 0..8 {
            assert!(is_dc_only_safe(&v, 8, bx * 8, by * 8), "8x8 ({bx},{by})");
        }
    }
    // 4x4 has no variance data: C early-exits with 0.
    assert!(!is_dc_only_safe(&v, 4, 0, 0));
    // Uniform content: zero variance everywhere -> always DC-only.
    let u = vec![128u8; 64 * 64];
    let vu = compute_b64_variance(&u, 64, 0, 0);
    for sq in [64usize, 32, 16, 8] {
        assert!(is_dc_only_safe(&vu, sq, 0, 0), "uniform sq={sq}");
    }
}

#[test]
fn gradient64_trees_match_c() {
    let y = gradient64();
    // q20 (qindex 80): LVL_6, max 32 -> forced SPLIT at 64, every 32
    // SPLITs again, 16x16 leaves everywhere (C stream: op0 SPLIT,
    // op1 SPLIT, op2 NONE...).
    let t20 = pd0_pick_sb_partition(
        &y,
        64,
        0,
        0,
        20,
        80,
        frame_lambda_weight(20, false, 0),
        0,
        64,
        64,
        None,
        64,
        None,
        0.0,
    );
    assert_eq!(t20.leaf_sizes(), vec![16; 16]);
    // q40 (qindex 160): LVL_5, max 32 -> forced SPLIT at 64, all four
    // 32x32 keep PARENT (C: op0 SPLIT, op1 NONE).
    let t40 = pd0_pick_sb_partition(
        &y,
        64,
        0,
        0,
        40,
        160,
        frame_lambda_weight(40, false, 0),
        0,
        64,
        64,
        None,
        64,
        None,
        0.0,
    );
    assert_eq!(t40.leaf_sizes(), vec![32; 4]);
    // q55 (qindex 220): LVL_5, 64 in set and PARENT wins outright.
    let t55 = pd0_pick_sb_partition(
        &y,
        64,
        0,
        0,
        55,
        220,
        frame_lambda_weight(55, false, 0),
        0,
        64,
        64,
        None,
        64,
        None,
        0.0,
    );
    assert_eq!(t55, Pd0Tree::Leaf(64));
    // Uniform: LVL_5 with zero residual everywhere -> 64x64 NONE.
    let u = vec![128u8; 64 * 64];
    let tu = pd0_pick_sb_partition(
        &u,
        64,
        0,
        0,
        40,
        160,
        frame_lambda_weight(40, false, 0),
        0,
        64,
        64,
        None,
        64,
        None,
        0.0,
    );
    assert_eq!(tu, Pd0Tree::Leaf(64));
}

/// PD0_LVL_1 per-block costs pinned from the instrumented M6 run
/// (SVT_M6DBG PD0BLK lines, gradient-64, docs/IDENTITY-STATUS.md M6
/// chunk). Single-SB frame -> default tables, exactly like C's SB 0.
#[test]
fn lvl1_block_costs_match_c() {
    let y = gradient64();
    let tables = build_m6_pd0_tables(220);
    let mut ctx = Pd0Ctx {
        src: &y,
        stride: 64,
        sb_x: 0,
        sb_y: 0,
        aligned_w: 64,
        aligned_h: 64,
        vars: compute_b64_variance(&y, 64, 0, 0),
        qp: 55,
        qindex: 220,
        qm_level: 15,
        lambda: kf_full_lambda_8bit(220, 55) as u64,
        mode: Pd0Mode::Lvl1,
        coeff_rate_est_lvl: 1,
        ac_bias_eff: 0.0,
        lvl1: Some(&tables),
        max_sq: 64,
        min_sq: 8,
        is_subres_safe: 255,
        subres_step: 0,
        ires_factor: 0,
        accurate_part_ctx: true,
        depth_early_exit_th: 1000,
        parent_cost_bias: 1000,
        nsq_enabled: true,
        tile_top: 0,
        tile_left: 0,
        recon_canvas: None,
        inter: None,
        pending_recon: None,
        scratch: Pd0Scratch::default(),
        root_det: Pd0RootDet {
            rd_cost: 0,
            blk: None,
        },
    };
    for (sq, ox, oy, cost) in [
        (64usize, 0usize, 0usize, 1791569177u64),
        (32, 0, 0, 526486441),
        (32, 32, 0, 572301943),
        (16, 0, 0, 146206469),
        (8, 0, 0, 44014180),
        (8, 8, 0, 35188942),
        (8, 0, 8, 37535984),
        (8, 8, 8, 60514499),
    ] {
        assert_eq!(
            ctx.lvl1_block_cost(sq, ox, oy),
            cost,
            "q55 sq={sq} ({ox},{oy})"
        );
    }

    let tables40 = build_m6_pd0_tables(160);
    let mut ctx40 = Pd0Ctx {
        src: &y,
        stride: 64,
        sb_x: 0,
        sb_y: 0,
        aligned_w: 64,
        aligned_h: 64,
        vars: compute_b64_variance(&y, 64, 0, 0),
        qp: 40,
        qindex: 160,
        qm_level: 15,
        lambda: kf_full_lambda_8bit(160, 40) as u64,
        mode: Pd0Mode::Lvl1,
        coeff_rate_est_lvl: 1,
        ac_bias_eff: 0.0,
        lvl1: Some(&tables40),
        max_sq: 64,
        min_sq: 8,
        is_subres_safe: 255,
        subres_step: 0,
        ires_factor: 0,
        accurate_part_ctx: true,
        depth_early_exit_th: 1000,
        parent_cost_bias: 1000,
        nsq_enabled: true,
        tile_top: 0,
        tile_left: 0,
        recon_canvas: None,
        inter: None,
        pending_recon: None,
        scratch: Pd0Scratch::default(),
        root_det: Pd0RootDet {
            rd_cost: 0,
            blk: None,
        },
    };
    for (sq, ox, oy, cost) in [
        (64usize, 0usize, 0usize, 1176293547u64),
        (32, 0, 0, 230378290),
        (16, 0, 0, 62496975),
        (8, 0, 0, 16077204),
    ] {
        assert_eq!(
            ctx40.lvl1_block_cost(sq, ox, oy),
            cost,
            "q40 sq={sq} ({ox},{oy})"
        );
    }

    let tables20 = build_m6_pd0_tables(80);
    let mut ctx20 = Pd0Ctx {
        src: &y,
        stride: 64,
        sb_x: 0,
        sb_y: 0,
        aligned_w: 64,
        aligned_h: 64,
        vars: compute_b64_variance(&y, 64, 0, 0),
        qp: 20,
        qindex: 80,
        qm_level: 15,
        lambda: kf_full_lambda_8bit(80, 20) as u64,
        mode: Pd0Mode::Lvl1,
        coeff_rate_est_lvl: 1,
        ac_bias_eff: 0.0,
        lvl1: Some(&tables20),
        max_sq: 64,
        min_sq: 8,
        is_subres_safe: 255,
        subres_step: 0,
        ires_factor: 0,
        accurate_part_ctx: true,
        depth_early_exit_th: 1000,
        parent_cost_bias: 1000,
        nsq_enabled: true,
        tile_top: 0,
        tile_left: 0,
        recon_canvas: None,
        inter: None,
        pending_recon: None,
        scratch: Pd0Scratch::default(),
        root_det: Pd0RootDet {
            rd_cost: 0,
            blk: None,
        },
    };
    for (sq, ox, oy, cost) in [
        (64usize, 0usize, 0usize, 903280295u64),
        (32, 0, 0, 51245980),
        (16, 0, 0, 14528276),
        (8, 0, 0, 3483565),
        (8, 8, 0, 3484388),
    ] {
        assert_eq!(
            ctx20.lvl1_block_cost(sq, ox, oy),
            cost,
            "q20 sq={sq} ({ox},{oy})"
        );
    }
}

/// M6 PD0 trees for the gradient-64 identity cells (instrumented
/// PD0CMP verdicts): q20/q40 -> 64 SPLIT + four 32x32 PARENT (q20 is
/// SHALLOWER than the eff-M9 16x16 tree), q55 -> single 64x64.
#[test]
fn m6_gradient64_trees_match_c() {
    let y = gradient64();
    let t20 = pd0_pick_sb_partition_m6(
        &y,
        64,
        0,
        0,
        20,
        80,
        frame_lambda_weight(20, false, 0),
        &build_m6_pd0_tables(80),
        1,
        true,
        64,
        64,
        None,
        64,
        0.0,
    );
    assert_eq!(t20.leaf_sizes(), vec![32; 4]);
    let t40 = pd0_pick_sb_partition_m6(
        &y,
        64,
        0,
        0,
        40,
        160,
        frame_lambda_weight(40, false, 0),
        &build_m6_pd0_tables(160),
        1,
        true,
        64,
        64,
        None,
        64,
        0.0,
    );
    assert_eq!(t40.leaf_sizes(), vec![32; 4]);
    let t55 = pd0_pick_sb_partition_m6(
        &y,
        64,
        0,
        0,
        55,
        220,
        frame_lambda_weight(55, false, 0),
        &build_m6_pd0_tables(220),
        1,
        true,
        64,
        64,
        None,
        64,
        0.0,
    );
    assert_eq!(t55, Pd0Tree::Leaf(64));
    // Uniform content: exact DC prediction, zero residual -> 64 NONE
    // (keeps every uniform p6 identity cell byte-identical).
    let u = vec![128u8; 64 * 64];
    let tu = pd0_pick_sb_partition_m6(
        &u,
        64,
        0,
        0,
        40,
        160,
        frame_lambda_weight(40, false, 0),
        &build_m6_pd0_tables(160),
        1,
        true,
        64,
        64,
        None,
        64,
        0.0,
    );
    assert_eq!(tu, Pd0Tree::Leaf(64));
}
