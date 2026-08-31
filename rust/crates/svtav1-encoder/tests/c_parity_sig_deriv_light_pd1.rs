//! Differential parity for `svt_aom_sig_deriv_enc_dec_light_pd1_default`
//! (`Source/Lib/Codec/enc_mode_config.c:7378`) — the whole light-PD1 signal
//! set.
//!
//! **Evidence tier 1** (`docs/WORKING-ON-THIS.md` §4): the entry point is
//! EXPORTED and the shim drives the real symbol.
//!
//! `pic_lpd1_lvl` is nonzero for non-base inter pictures at M7..M13
//! (`enc_mode_config.c:9407-9420`), presets the port supports, so a
//! high-preset video GOP takes THIS path rather than
//! `svt_aom_sig_deriv_enc_dec_default`.
//!
//! Two derived levels are not directly observable — `rdoq_level` and
//! `intra_level` feed tables this lane has not ported. `intra_level` is
//! validated the same way the PD0 test does it, by pushing the port's level
//! through C's own `set_intra_ctrls` from a second exported entry point;
//! `rdoq_level` is left unverified and is called out in its own test.

use svtav1_cref::sig_deriv as cref;
use svtav1_cref::sig_deriv::{lp_in, lp_out};
use svtav1_encoder::port_enc_mode_config::ResolutionRange;
use svtav1_encoder::port_enc_mode_config::light_pd1;
use svtav1_encoder::port_enc_mode_config::light_pd1::LightPd1Inputs;

#[derive(Clone, Copy)]
struct Case {
    lpd1_level: i8,
    enc_mode: i8,
    input_res: ResolutionRange,
    is_b_slice: bool,
    picture_qp: u32,
    ref_l0_avail: bool,
    ref_l1_avail: bool,
    ref_l1_try: u32,
    me8_var: u32,
    me64_dist: u32,
    l0_skip: u8,
    l1_skip: u8,
    l0_mvp: u8,
    l1_mvp: u8,
    ref_skip_perc: u8,
    cand_red: u8,
    rdoq: u8,
    coeff_shave: u8,
    me_subpel: u8,
    rate_est: u8,
    approx_rate: u8,
    intra: u8,
    ref_l0_try: u32,
    best_unipred: u8,
    rtc: bool,
    hier_levels: u8,
    is_leaf: bool,
}

impl Default for Case {
    fn default() -> Self {
        Self {
            lpd1_level: 0,
            enc_mode: 9,
            input_res: ResolutionRange::R1080p,
            is_b_slice: true,
            picture_qp: 32,
            ref_l0_avail: true,
            ref_l1_avail: true,
            ref_l1_try: 1,
            me8_var: 5000,
            me64_dist: 20_000,
            l0_skip: 0,
            l1_skip: 0,
            l0_mvp: 0,
            l1_mvp: 0,
            ref_skip_perc: 20,
            cand_red: 1,
            rdoq: 1,
            coeff_shave: 1,
            me_subpel: 1,
            rate_est: 1,
            approx_rate: 1,
            intra: 1,
            ref_l0_try: 1,
            best_unipred: 1,
            rtc: false,
            hier_levels: 4,
            is_leaf: false,
        }
    }
}

fn build_input(c: &Case) -> [i32; lp_in::COUNT] {
    let mut i = [0i32; lp_in::COUNT];
    i[lp_in::LPD1_LEVEL] = i32::from(c.lpd1_level);
    i[lp_in::ENC_MODE] = i32::from(c.enc_mode);
    i[lp_in::INPUT_RES] = i32::from(c.input_res.as_u8());
    i[lp_in::IS_B_SLICE] = i32::from(c.is_b_slice);
    i[lp_in::PICTURE_QP] = c.picture_qp as i32;
    i[lp_in::REF_L0_AVAIL] = i32::from(c.ref_l0_avail);
    i[lp_in::REF_L1_AVAIL] = i32::from(c.ref_l1_avail);
    i[lp_in::REF_L1_TRY] = c.ref_l1_try as i32;
    i[lp_in::ME8_VAR] = c.me8_var as i32;
    i[lp_in::ME64_DIST] = c.me64_dist as i32;
    i[lp_in::L0_SKIP] = i32::from(c.l0_skip);
    i[lp_in::L1_SKIP] = i32::from(c.l1_skip);
    i[lp_in::L0_MVP] = i32::from(c.l0_mvp);
    i[lp_in::L1_MVP] = i32::from(c.l1_mvp);
    i[lp_in::REF_SKIP_PERC] = i32::from(c.ref_skip_perc);
    i[lp_in::CAND_RED] = i32::from(c.cand_red);
    i[lp_in::RDOQ] = i32::from(c.rdoq);
    i[lp_in::COEFF_SHAVE] = i32::from(c.coeff_shave);
    i[lp_in::ME_SUBPEL] = i32::from(c.me_subpel);
    i[lp_in::RATE_EST] = i32::from(c.rate_est);
    i[lp_in::APPROX_RATE] = i32::from(c.approx_rate);
    i[lp_in::INTRA] = i32::from(c.intra);
    i[lp_in::REF_L0_TRY] = c.ref_l0_try as i32;
    i[lp_in::BEST_UNIPRED] = i32::from(c.best_unipred);
    i[lp_in::RTC] = i32::from(c.rtc);
    i[lp_in::HIER_LEVELS] = i32::from(c.hier_levels);
    i[lp_in::UPDATE_TYPE] = if c.is_leaf { 1 } else { 0 };
    i
}

fn to_port(c: &Case) -> LightPd1Inputs {
    LightPd1Inputs {
        lpd1_level: c.lpd1_level,
        enc_mode: c.enc_mode,
        input_resolution: c.input_res,
        is_b_slice: c.is_b_slice,
        picture_qp: c.picture_qp,
        // With reference scaling ON (the shim sets is_not_scaled = 0),
        // svt_aom_is_ref_same_size requires a B-slice AND a present,
        // same-size reference.
        is_ref_l0_avail: c.is_b_slice && c.ref_l0_avail,
        is_ref_l1_avail: c.is_b_slice && c.ref_l1_avail,
        ref_list1_count_try: c.ref_l1_try,
        me_8x8_cost_variance: c.me8_var,
        me_64x64_distortion: c.me64_dist,
        l0_sb_skip: c.l0_skip,
        l1_sb_skip: c.l1_skip,
        l0_sb_64x64_mvp: c.l0_mvp,
        l1_sb_64x64_mvp: c.l1_mvp,
        ref_skip_percentage: c.ref_skip_perc,
        cand_reduction_level: c.cand_red,
        rdoq_level: c.rdoq,
        coeff_shaving_level: c.coeff_shave,
        me_subpel_level: c.me_subpel,
        rate_est_level: c.rate_est,
        approx_inter_rate: c.approx_rate,
        intra_level: c.intra,
        ref_list0_count_try: c.ref_l0_try,
        use_best_me_unipred_cand_only: c.best_unipred,
        use_flat_ipp: c.rtc && c.hier_levels == 0,
        is_not_last_layer: !c.is_leaf,
    }
}

fn assert_case(c: &Case, msg: &str) {
    let o = light_pd1::sig_deriv_enc_dec_light_pd1_default(to_port(c)).expect("levels in range");
    let t = cref::sig_deriv_light_pd1_default(&build_input(c));

    assert_eq!(
        i64::from(o.lpd1_globalmv_bypass_th),
        t[lp_out::GLOBALMV_TH],
        "globalmv_bypass_th {msg}"
    );
    let cr = &o.cand_reduction;
    assert_eq!(
        [
            i64::from(cr.redundant_cand_ctrls.score_th),
            i64::from(cr.redundant_cand_ctrls.mag_th),
            i64::from(cr.near_count_ctrls.enabled),
            i64::from(cr.near_count_ctrls.near_count),
            i64::from(cr.near_count_ctrls.near_near_count),
            i64::from(cr.lpd1_mvp_best_me_list),
            i64::from(cr.use_neighbouring_mode_enabled),
            i64::from(cr.cand_elimination_ctrls.enabled),
            i64::from(cr.cand_elimination_ctrls.dc_only_th),
            i64::from(cr.cand_elimination_ctrls.skip_dc_th),
            i64::from(cr.reduce_unipred_candidates),
        ],
        t[lp_out::CAND_RED..lp_out::CAND_RED + 11],
        "cand_reduction_ctrls {msg}"
    );
    assert_eq!(
        [
            i64::from(o.coeff_shaving.enabled),
            i64::from(o.coeff_shaving.level_threshold),
            i64::from(o.coeff_shaving.zero_gap_threshold),
            i64::from(o.coeff_shaving.rd_zero_strength),
        ],
        t[lp_out::COEFF_SHAVE..lp_out::COEFF_SHAVE + 4],
        "coeff_shaving_ctrls {msg}"
    );
    let s = &o.md_subpel_me;
    assert_eq!(
        [
            i64::from(s.enabled),
            i64::from(s.subpel_search_type),
            i64::from(s.max_precision),
            i64::from(s.subpel_search_method),
            i64::from(s.subpel_iters_per_step),
            i64::from(s.pred_variance_th),
            i64::from(s.abs_th_mult),
            i64::from(s.round_dev_th),
            i64::from(s.skip_diag_refinement),
            i64::from(s.min_blk_sz),
            i64::from(s.mvp_th),
            i64::from(s.hp_mv_th),
            i64::from(s.bias_fp),
        ],
        t[lp_out::SUBPEL_ME..lp_out::SUBPEL_ME + 13],
        "md_subpel_me_ctrls (derived level {}) {msg}",
        o.me_subpel_level
    );
    assert_eq!(
        [
            i64::from(o.lpd1_tx_skip_decision.skip_tx_score_th),
            i64::from(o.lpd1_tx_skip_decision.dist_energy_th),
            i64::from(o.lpd1_tx_skip_decision.rd_skip_th),
        ],
        t[lp_out::TX_SKIP..lp_out::TX_SKIP + 3],
        "lpd1_tx_skip_decision (derived level {}) {msg}",
        o.lpd1_tx_skip_decision_level
    );
    assert_eq!(
        [
            i64::from(o.lpd1_tx.zero_y_coeff_exit),
            i64::from(o.lpd1_tx.chroma_detector_level),
            i64::from(o.lpd1_tx.use_uv_shortcuts_on_y_coeffs),
            i64::from(o.lpd1_tx.use_mds3_shortcuts_th),
        ],
        t[lp_out::LPD1_TX..lp_out::LPD1_TX + 4],
        "lpd1_tx_ctrls (derived level {}) {msg}",
        o.lpd1_tx_level
    );
    assert_eq!(
        i64::from(o.lpd1_blk_skip_luma_rd_pct),
        t[lp_out::BLK_SKIP_LUMA_PCT],
        "lpd1_blk_skip_luma_rd_pct {msg}"
    );
    assert_eq!(
        i64::from(o.lpd1_chroma_skip_energy_th),
        t[lp_out::CHROMA_SKIP_ENERGY],
        "lpd1_chroma_skip_energy_th {msg}"
    );
    assert_eq!(
        [
            i64::from(o.rate_est.update_skip_ctx_dc_sign_ctx),
            i64::from(o.rate_est.update_skip_coeff_ctx),
            i64::from(o.rate_est.coeff_rate_est_lvl),
            i64::from(o.rate_est.lpd0_qp_offset),
            i64::from(o.rate_est.pd0_fast_coeff_est_level),
        ],
        t[lp_out::RATE_EST..lp_out::RATE_EST + 5],
        "rate_est_ctrls (derived level {}) {msg}",
        o.rate_est_level
    );
    assert_eq!(
        [
            i64::from(o.approx_inter_rate),
            o.pf.pf_shape as i64,
            i64::from(o.shut_fast_rate),
            i64::from(o.uv_enabled),
            i64::from(o.uv_mode),
            i64::from(o.md_disallow_nsq_search),
            i64::from(o.new_nearest_injection),
            i64::from(o.blk_skip_decision),
            i64::from(o.subres_odd_to_even_deviation_th),
        ],
        [
            t[lp_out::APPROX_RATE],
            t[lp_out::PF_SHAPE],
            t[lp_out::SHUT_FAST_RATE],
            t[lp_out::UV_EN],
            t[lp_out::UV_MODE],
            t[lp_out::NSQ_OFF],
            t[lp_out::NN_INJ],
            t[lp_out::BLK_SKIP_DEC],
            t[lp_out::SUBRES_DEV],
        ],
        "scalar assignments {msg}"
    );
    assert_eq!(
        [
            i64::from(o.inter_intra.enabled),
            i64::from(o.inter_intra.use_rd_model),
            i64::from(o.inter_intra.wedge_mode_sq),
            i64::from(o.inter_intra.wedge_mode_nsq),
        ],
        t[lp_out::INTER_INTRA..lp_out::INTER_INTRA + 4],
        "inter_intra_comp_ctrls {msg}"
    );

    // The derived intra_level, validated through C's own set_intra_ctrls.
    // Light-PD1 always passes dist_based_ang_intra_level = 2, like PD0 does.
    let expect_ic = cref::set_intra_ctrls_at_level(o.intra_level, 2, !c.is_b_slice);
    assert_eq!(
        expect_ic,
        t[lp_out::INTRA_CTRLS..lp_out::INTRA_CTRLS + 8],
        "intra_level {} {msg}",
        o.intra_level
    );
}

#[test]
fn light_pd1_matches_c_over_the_level_and_reference_product() {
    for lpd1_level in -1i8..=6 {
        for &m in &[5i8, 8, 9, 13] {
            for &res in &[
                ResolutionRange::R240p,
                ResolutionRange::R480p,
                ResolutionRange::R720p,
                ResolutionRange::R1080p,
                ResolutionRange::R4k,
            ] {
                for &l0_avail in &[false, true] {
                    for &(l0s, l1s, l0m, l1m) in &[
                        (0u8, 0u8, 0u8, 0u8),
                        (1, 1, 0, 0),
                        (0, 0, 1, 1),
                        (1, 0, 1, 0),
                    ] {
                        for &perc in &[0u8, 35, 36, 50, 51, 100] {
                            let c = Case {
                                lpd1_level,
                                enc_mode: m,
                                input_res: res,
                                ref_l0_avail: l0_avail,
                                l0_skip: l0s,
                                l1_skip: l1s,
                                l0_mvp: l0m,
                                l1_mvp: l1m,
                                ref_skip_perc: perc,
                                ..Case::default()
                            };
                            assert_case(
                                &c,
                                &format!(
                                    "lpd1={lpd1_level} m={m} res={res:?} l0={l0_avail} \
                                     flags=({l0s},{l1s},{l0m},{l1m}) perc={perc}"
                                ),
                            );
                        }
                    }
                }
            }
        }
    }
}

/// The three static-SB predicates compare the ME variance and distortion
/// against multiples of `picture_qp` (200x, 800x, 100x); sweep across each
/// crossing.
#[test]
fn light_pd1_static_sb_thresholds_match_c() {
    for &qp in &[1u32, 10, 32, 63] {
        for &mult in &[0u32, 99, 100, 101, 199, 200, 201, 799, 800, 801, 5000] {
            for &lpd1_level in &[0i8, 2, 3, 6] {
                for &(l0s, l1s, perc) in &[(1u8, 1u8, 40u8), (1, 1, 60), (0, 0, 60)] {
                    let c = Case {
                        lpd1_level,
                        picture_qp: qp,
                        me8_var: mult * qp,
                        me64_dist: mult * qp,
                        l0_skip: l0s,
                        l1_skip: l1s,
                        l0_mvp: l0s,
                        l1_mvp: l1s,
                        ref_skip_perc: perc,
                        ..Case::default()
                    };
                    assert_case(
                        &c,
                        &format!(
                            "qp={qp} mult={mult} lpd1={lpd1_level} skips=({l0s},{l1s},{perc})"
                        ),
                    );
                }
            }
        }
    }
}

/// Every picture-level input the derivations clamp against.
#[test]
fn light_pd1_picture_level_clamps_match_c() {
    for &lpd1_level in &[0i8, 1, 3, 5, 6] {
        for cand_red in 0u8..=6 {
            for rdoq in 0u8..=2 {
                for coeff_shave in 0u8..=2 {
                    for me_subpel in 0u8..=10 {
                        for rate_est in 0u8..=4 {
                            for &approx in &[0u8, 1, 2] {
                                for &intra in &[0u8, 1, 6, 9] {
                                    let c = Case {
                                        lpd1_level,
                                        cand_red,
                                        rdoq,
                                        coeff_shave,
                                        me_subpel,
                                        rate_est,
                                        approx_rate: approx,
                                        intra,
                                        ..Case::default()
                                    };
                                    assert_case(
                                        &c,
                                        &format!(
                                            "lpd1={lpd1_level} cr={cand_red} rdoq={rdoq} \
                                             cs={coeff_shave} sp={me_subpel} re={rate_est} \
                                             ar={approx} intra={intra}"
                                        ),
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Positive controls so the sweeps cannot pass on constant dumps.
#[test]
fn light_pd1_positive_controls() {
    // The two post-hoc rate-est overrides at the end of the function force
    // both skip-context flags to 0 even at a level that sets them to 1.
    let c = Case {
        lpd1_level: 0,
        rate_est: 1,
        ..Case::default()
    };
    let t = cref::sig_deriv_light_pd1_default(&build_input(&c));
    assert_eq!(
        t[lp_out::RATE_EST],
        0,
        "update_skip_ctx_dc_sign_ctx forced 0"
    );
    assert_eq!(t[lp_out::RATE_EST + 1], 0, "update_skip_coeff_ctx forced 0");
    // Level 4 of set_rate_est_ctrls is what the LPD1_LVL_0 arm selects, and
    // its coeff_rate_est_lvl is 2 — proving the override did not zero the
    // whole struct.
    assert_eq!(t[lp_out::RATE_EST + 2], 2, "coeff_rate_est_lvl survives");

    // Unlike the PD0 path, light-PD1 leaves shut_fast_rate FALSE.
    assert_eq!(t[lp_out::SHUT_FAST_RATE], 0);
    // Chroma is forced to CHROMA_MODE_1 (fast) with uv enabled.
    assert_eq!((t[lp_out::UV_EN], t[lp_out::UV_MODE]), (1, 1));

    // approx_inter_rate is MAX(1, pcs value), not a copy.
    for (pic, want) in [(0i32, 1i64), (1, 1), (2, 2)] {
        let ca = Case {
            approx_rate: pic as u8,
            ..Case::default()
        };
        assert_eq!(
            cref::sig_deriv_light_pd1_default(&build_input(&ca))[lp_out::APPROX_RATE],
            want,
            "approx_inter_rate MAX(1, {pic})"
        );
    }

    // The LPD1_LVL_2 boundary flips lpd1_blk_skip_luma_rd_pct 0 -> 90.
    for (lvl, want) in [(0i8, 0i64), (2, 0), (3, 90), (6, 90)] {
        let cl = Case {
            lpd1_level: lvl,
            ..Case::default()
        };
        assert_eq!(
            cref::sig_deriv_light_pd1_default(&build_input(&cl))[lp_out::BLK_SKIP_LUMA_PCT],
            want,
            "blk_skip_luma_rd_pct at LPD1_LVL_{lvl}"
        );
    }
}

/// HONEST GAP, stated rather than left implied: the `rdoq_level` this function
/// derives feeds `set_rdoq_controls`, a `static` this lane has NOT ported, and
/// nothing else in the dumped context depends on it — so the derivation is
/// translated but UNVERIFIED against C. Everything it can be checked against
/// is checked; this test records what is not.
#[test]
fn rdoq_level_derivation_is_translated_but_unverified() {
    // The port's ladder, pinned against the C source at enc_mode_config.c:7443
    // so a future edit is visible even without a differential.
    let probe = |m: i8, lpd1: i8, pic_rdoq: u8| -> u8 {
        light_pd1::sig_deriv_enc_dec_light_pd1_default(to_port(&Case {
            enc_mode: m,
            lpd1_level: lpd1,
            rdoq: pic_rdoq,
            ..Case::default()
        }))
        .expect("in range")
        .rdoq_level
    };
    // A zero picture level disables it outright.
    assert_eq!(probe(9, 0, 0), 0);
    // <= M8: level 1 up to LPD1_LVL_4, then 0.
    assert_eq!(probe(8, 4, 1), 1);
    assert_eq!(probe(8, 5, 1), 0);
    // > M8: 4 at LPD1_LVL_0, 5 up to LPD1_LVL_4, then 0.
    assert_eq!(probe(9, 0, 1), 4);
    assert_eq!(probe(9, 1, 1), 5);
    assert_eq!(probe(9, 4, 1), 5);
    assert_eq!(probe(9, 5, 1), 0);
    // The MAX against the picture level only raises, never lowers.
    assert_eq!(probe(8, 4, 3), 3);
    assert_eq!(probe(9, 0, 6), 6);
}

/// Direct probe of the `me_subpel_level` zeroing predicate, which needs the ME
/// variance and distortion to land EXACTLY on `200 * picture_qp` to separate
/// `<` from `<=`-shaped transcription errors.
#[test]
fn light_pd1_me_subpel_zeroing_lands_on_the_threshold() {
    // The predicate is a conjunction of the SAME threshold on two different
    // quantities, so the two must be varied INDEPENDENTLY — with both equal, a
    // wrong constant on one line is masked by the other.
    let case = |var_mult: u32, dist_mult: u32, qp: u32| Case {
        lpd1_level: 3,
        picture_qp: qp,
        me8_var: var_mult * qp,
        me64_dist: dist_mult * qp,
        l0_mvp: 1,
        l1_mvp: 1,
        ..Case::default()
    };
    let probe = |var_mult: u32, dist_mult: u32, qp: u32| -> u8 {
        light_pd1::sig_deriv_enc_dec_light_pd1_default(to_port(&case(var_mult, dist_mult, qp)))
            .expect("in range")
            .me_subpel_level
    };
    // 1080p at LPD1_LVL_3 selects 9 unless the static-SB test fires.
    // Variance exactly ON the threshold, distortion well below it: only the
    // variance line decides.
    assert_eq!(probe(200, 100, 32), 9, "200*qp is NOT below 200*qp");
    assert_eq!(probe(199, 100, 32), 0, "199*qp IS below, so sub-pel is off");
    // ...and the mirror, isolating the distortion line.
    assert_eq!(probe(100, 200, 32), 9);
    assert_eq!(probe(100, 199, 32), 0);
    // Every one of those must also agree with C.
    for &(v, d) in &[
        (198u32, 100u32),
        (199, 100),
        (200, 100),
        (201, 100),
        (100, 198),
        (100, 199),
        (100, 200),
        (100, 201),
        (200, 200),
    ] {
        assert_case(
            &case(v, d, 32),
            &format!("me_subpel threshold var={v} dist={d}"),
        );
    }
}
