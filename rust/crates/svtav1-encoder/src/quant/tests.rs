use super::*;

/// The bd8 arm of [`build_quant_table_bd`] must be byte-identical to the
/// legacy [`build_quant_table`] at every qindex (additive-neutrality
/// guarantee: the bd8 path never changes because the new bd-aware builder
/// reproduces the old one exactly for bd==8).
#[test]
fn build_quant_table_bd8_matches_legacy() {
    for q in 0u8..=255 {
        assert_eq!(
            build_quant_table_bd(q, 8),
            build_quant_table(q),
            "bd8 quant table mismatch at qindex {q}"
        );
    }
}

/// bd10 dequant (`t.dequant`) must equal the FFI-verified
/// `crate::bd10::{dc,ac}_qlookup_10` — the coefficient dequant the u16 MD
/// path folds. Spot-checks the wiring (the tables themselves are pinned
/// full-range by tests/c_parity_bd10_quant.rs).
#[test]
fn build_quant_table_bd10_dequant_matches_tables() {
    for &q in &[0u8, 1, 20, 40, 55, 128, 200, 255] {
        let t = build_quant_table_bd(q, 10);
        assert_eq!(
            t.dequant[0],
            crate::bd10::dc_qlookup_10(q) as i32,
            "dc q={q}"
        );
        assert_eq!(
            t.dequant[1],
            crate::bd10::ac_qlookup_10(q) as i32,
            "ac q={q}"
        );
    }
}

/// `quantize_fp_hbd` (bd10 FP quantize) must equal `quantize_fp` for
/// coefficients that fit in INT16 (where the bd8 clamp is a no-op), and
/// must DIVERGE for coefficients exceeding INT16 — the highbd path keeps
/// the full value where the 8-bit path clamps to 32767 (C
/// highbd_quantize_fp_helper_c:382 vs quantize_fp_helper_c:245). This is
/// the bug that made the first bd10 cell's dark-corner DC level diverge.
#[test]
fn quantize_fp_hbd_matches_fp_below_int16_diverges_above() {
    let t = build_quant_table_bd(160, 10);
    let scan: Vec<u16> = (0..16u16).collect();
    // (a) small coeffs (|c| < 32767): the clamp never fires -> identical.
    let small: Vec<i32> = vec![
        1000, -2000, 300, 0, 500, -100, 42, 7, 0, 0, 0, 0, 0, 0, 0, 0,
    ];
    let (mut qa, mut da) = (vec![0i32; 16], vec![0i32; 16]);
    let (mut qb, mut db) = (vec![0i32; 16], vec![0i32; 16]);
    let ea = quantize_fp(&small, &scan, &t, 1, &mut qa, &mut da);
    let eb = quantize_fp_hbd(&small, &scan, &t, 1, &mut qb, &mut db);
    assert_eq!((ea, &qa, &da), (eb, &qb, &db), "hbd==fp below INT16");
    // (b) a DC coefficient beyond INT16 (bd10 dark-corner magnitude): the
    // 8-bit clamp truncates to 32767 -> a SMALLER level than the highbd
    // full-value path.
    let big: Vec<i32> = vec![90000, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    let (mut qc, mut dc) = (vec![0i32; 16], vec![0i32; 16]);
    let (mut qd, mut dd) = (vec![0i32; 16], vec![0i32; 16]);
    quantize_fp(&big, &scan, &t, 1, &mut qc, &mut dc);
    quantize_fp_hbd(&big, &scan, &t, 1, &mut qd, &mut dd);
    assert!(
        qd[0].abs() > qc[0].abs(),
        "hbd DC level {} must exceed clamped bd8 level {}",
        qd[0],
        qc[0]
    );
}

/// Instrumented-library capture (qindex 220, g64 q55 QIQ tables line):
/// dc zbin=326 round=195 quant=-1255 qshift=128 qfp=125 rfp=261 deq=522
/// ac zbin=583 round=349 quant=-29571 qshift=128 qfp=70 rfp=466 deq=933
#[test]
fn quant_table_matches_c_qindex220() {
    let t = build_quant_table(220);
    assert_eq!(t.zbin, [326, 583]);
    assert_eq!(t.round, [195, 349]);
    assert_eq!(t.quant, [-1255, -29571]);
    assert_eq!(t.quant_shift, [128, 128]);
    assert_eq!(t.quant_fp, [125, 70]);
    assert_eq!(t.round_fp, [261, 466]);
    assert_eq!(t.dequant, [522, 933]);
}

/// OPTB capture: rdmult=1054880 @ lambda=248207 (q40) and
/// rdmult=6493388 @ lambda=1527856 (q55), luma.
#[test]
fn rdmult_matches_c() {
    assert_eq!(rdoq_rdmult(248207, 0), 1054880);
    assert_eq!(rdoq_rdmult(1527856, 0), 6493388);
    assert_eq!(rdoq_rdmult(25650, 0), 109013); // (25650*17+2)>>2 = 436052>>2
}

/// `plane_rd_mult[allintra || rtc][is_inter][plane_type]`, full_loop.c:994
/// (the MAINLINE `#else` table; `TUNE_CHROMA_SSIM` is 0 unless
/// `SVT_HDR_MODE`, EbDebugMacros.h:70).
///
/// EVIDENCE TIER 4 (`docs/WORKING-ON-THIS.md` §4): the table is a
/// file-static const and `svt_av1_optimize_b` is `static`, so there is no
/// exported symbol to drive. The value that matters — the video arm's
/// chroma 20 — is separately pinned at tier 2 by the byte-identity cell
/// `video-key-rdoq-plane-rd-mult-p6-64x64` in `tools/regression_spotcheck.sh`.
#[test]
fn plane_rd_mult_matches_c() {
    // video (neither allintra nor rtc): {{17, 20}, {16, 20}}
    assert_eq!(plane_rd_mult(false, false, 0), 17);
    assert_eq!(plane_rd_mult(false, false, 1), 20);
    assert_eq!(plane_rd_mult(false, true, 0), 16);
    assert_eq!(plane_rd_mult(false, true, 1), 20);
    // allintra or rtc: {{17, 13}, {16, 10}}
    assert_eq!(plane_rd_mult(true, false, 0), 17);
    assert_eq!(plane_rd_mult(true, false, 1), 13);
    assert_eq!(plane_rd_mult(true, true, 0), 16);
    assert_eq!(plane_rd_mult(true, true, 1), 10);
    // The rdmult formula reads the arm: same lambda, chroma, both arms.
    // (248207 * 13 + 2) >> 2 = 806673; (248207 * 20 + 2) >> 2 = 1241035.
    assert_eq!(
        rdoq_rdmult_full(248207, 1, 0, false, false, true, false),
        806673
    );
    assert_eq!(
        rdoq_rdmult_full(248207, 1, 0, false, false, false, false),
        1241035
    );
    // The is_inter axis: video intra luma 17 vs inter luma 16
    // (C `pred_mode >= NEARESTMV`). (248207 * 16 + 2) >> 2 = 992828.
    assert_eq!(
        rdoq_rdmult_full(248207, 0, 0, false, false, false, true),
        992828
    );
    assert_eq!(
        rdoq_rdmult_full(248207, 0, 0, false, false, false, false),
        1054880
    );
    // Chroma is inter-axis-invariant (20 on both video rows).
    assert_eq!(
        rdoq_rdmult_full(248207, 1, 0, false, false, false, true),
        rdoq_rdmult_full(248207, 1, 0, false, false, false, false)
    );
    // Luma is arm-invariant — the reason the video-arm defect showed up
    // as a chroma-only divergence.
    assert_eq!(
        rdoq_rdmult_full(248207, 0, 0, false, false, true, false),
        rdoq_rdmult_full(248207, 0, 0, false, false, false, false)
    );
}

/// COEFFLVL captures: g64 pav=5425 -> HIGH/NORMAL/NORMAL at qp
/// 20/40/55; g128 pav=1483 -> LOW at qp 20. Thresholds 42/85/255.
#[test]
fn coeff_lvl_matches_c() {
    assert_eq!(derive_intra_coeff_level(5425, 20, 64, 64), CoeffLvl::High);
    assert_eq!(derive_intra_coeff_level(5425, 40, 64, 64), CoeffLvl::Normal);
    assert_eq!(derive_intra_coeff_level(5425, 55, 64, 64), CoeffLvl::Normal);
    assert_eq!(derive_intra_coeff_level(1483, 20, 128, 128), CoeffLvl::Low);
    // Boundary: cmplx == 255 is NOT > 255 -> NORMAL, 256 -> HIGH.
    assert_eq!(derive_intra_coeff_level(255, 1, 64, 64), CoeffLvl::Normal);
    assert_eq!(derive_intra_coeff_level(256, 1, 64, 64), CoeffLvl::High);
}

#[test]
fn rdoq_policy_matches_c() {
    assert_eq!(rdoq_level_allintra(9, CoeffLvl::High), 0);
    assert_eq!(rdoq_level_allintra(9, CoeffLvl::Normal), 3);
    assert_eq!(rdoq_level_allintra(9, CoeffLvl::Low), 2);
    assert_eq!(rdoq_level_allintra(9, CoeffLvl::VLow), 2);
    assert_eq!(rdoq_level_allintra(5, CoeffLvl::High), 1);
    assert_eq!(rdoq_cutoffs(2), (80, 100));
    assert_eq!(rdoq_cutoffs(3), (60, 100));
}

/// The q/dq mirror must be the decoder's ((q * dqv) >> log_scale) at
/// every position after fp + optimize, so the reconstruction the
/// encoder builds is exactly what the decoder will build.
#[test]
fn optimize_keeps_dequant_mirror() {
    // Synthetic 16x16 (c_tx_size 2, log_scale 0) residual spectrum.
    let n = 256usize;
    let mut tcoeffs = alloc::vec![0i32; n];
    let mut s = 0x1234_5678u32;
    for (i, c) in tcoeffs.iter_mut().enumerate() {
        s = s.wrapping_mul(1103515245).wrapping_add(12345);
        let mag = (s >> 20) as i32 % 900;
        *c = if s & 1 == 0 { mag } else { -mag } / (1 + i as i32 / 16);
    }
    let scan = crate::entropy::scan_tables::scan(2, 0);
    let cfg = CodingQuantCfg::new(3, 248207, 160);
    let mut q = alloc::vec![0i32; n];
    let mut dq = alloc::vec![0i32; n];
    let eob = quantize_inv_quantize_still(
        &cfg, &tcoeffs, &mut q, &mut dq, scan, 160, 2, 0, 0, 256, false, 15,
    );
    let t = build_quant_table(160);
    // The decoder's dequant is `(q * dqv) >> log_scale`; this cell is
    // c_tx_size 2 (16x16), whose log_scale is 0. Kept as a named binding
    // so the shift stays visible in the mirror formula.
    let log_scale = 0u32;
    for i in 0..n {
        let expect = ((q[i].unsigned_abs() as i64 * t.dequant[usize::from(i != 0)] as i64)
            >> log_scale) as i32;
        assert_eq!(dq[i].abs(), expect, "dq mirror at {i}");
        assert_eq!(dq[i] < 0, q[i] < 0 && q[i] != 0, "sign at {i}");
    }
    if eob > 0 {
        assert_ne!(
            q[scan[eob as usize - 1] as usize],
            0,
            "eob-1 must be nonzero"
        );
    }
}

/// The inverse-scan eob (SIMD, every dispatch tier) equals the reverse scan
/// walk for every generated scan, on all-zero, single-coefficient and random
/// sparse blocks.
#[test]
fn eob_from_iscan_matches_reverse_walk_for_every_scan() {
    use archmage::testing::{CompileTimePolicy, for_each_token_permutation};
    let mut cases: Vec<(&'static [u16], Vec<i32>)> = Vec::new();
    let mut st = 0x2545_F491_4F6C_DD1D_u64;
    for ts in 0..19 {
        for class in 0..3 {
            let scan = crate::entropy::scan_tables::scan(ts, class);
            let n = scan.len();
            assert!(crate::entropy::scan_tables::iscan_for(scan).is_some());
            cases.push((scan, vec![0; n]));
            for &pos in &[0, 1, n / 2, n - 1] {
                let mut q = vec![0; n];
                q[pos] = -3;
                cases.push((scan, q));
            }
            for density in [1u64, 8, 64] {
                let q = (0..n)
                    .map(|_| {
                        st ^= st << 13;
                        st ^= st >> 7;
                        st ^= st << 17;
                        if st % 256 < density { (st >> 20) as i32 % 9 - 4 } else { 0 }
                    })
                    .collect();
                cases.push((scan, q));
            }
        }
    }
    let report = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_| {
        for (scan, q) in &cases {
            assert_eq!(eob_from_qcoeff(scan, q), eob_by_walk(scan, q), "n={}", scan.len());
        }
    });
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert!(report.permutations_run >= 2, "{}", report.permutations_run);
}

/// C's scan-prefix cul level equals the raster sum on every generated scan,
/// including saturating and negative-DC blocks.
#[test]
fn cul_level_scan_form_matches_raster_form() {
    use crate::leaf_funnel::{compute_cul_level, compute_cul_level_scan};
    let mut st = 0x9E37_79B9_7F4A_7C15_u64;
    for ts in 0..19 {
        for class in 0..3 {
            let scan = crate::entropy::scan_tables::scan(ts, class);
            let n = scan.len();
            for density in [0u64, 2, 16, 128] {
                for big in [false, true] {
                    let q: Vec<i32> = (0..n)
                        .map(|_| {
                            st ^= st << 13;
                            st ^= st >> 7;
                            st ^= st << 17;
                            if st % 256 < density {
                                let m = if big { 200 } else { 3 };
                                (st >> 20) as i32 % (2 * m + 1) - m
                            } else {
                                0
                            }
                        })
                        .collect();
                    let eob = eob_by_walk(scan, &q);
                    assert_eq!(
                        compute_cul_level_scan(&q, scan, eob),
                        compute_cul_level(&q),
                        "ts={ts} class={class} density={density} big={big}"
                    );
                }
            }
        }
    }
}
