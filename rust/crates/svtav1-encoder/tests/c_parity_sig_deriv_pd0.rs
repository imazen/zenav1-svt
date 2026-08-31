//! Differential parity for `svt_aom_sig_deriv_enc_dec_pd0`
//! (`Source/Lib/Codec/enc_mode_config.c:7207`) — the per-SB PD0 signal set that
//! ALL THREE arms (allintra included) call.
//!
//! **Evidence tier 1** (`docs/WORKING-ON-THIS.md` §4): the entry point is
//! EXPORTED and the shim drives the real symbol on a synthetic SCS/PCS/ctx.
//!
//! One derived value cannot be read back directly: `intra_level` is consumed by
//! `set_intra_ctrls`, a `static` this lane has NOT ported, and C never stores
//! the level itself. It is validated instead by feeding the PORT's derived
//! level through C's OWN `set_intra_ctrls` — reached via the second exported
//! entry point `svt_aom_sig_deriv_enc_dec_default`, whose `pcs->intra_level` is
//! a direct input — and comparing the two resulting `intra_ctrls` structs. That
//! is still tier 1 (both sides are the real C table); its one caveat is that
//! `set_intra_ctrls` would have to be injective for the check to pin the level
//! exactly, and a separate test shows the levels it maps distinctly.

use svtav1_cref::sig_deriv as cref;
use svtav1_cref::sig_deriv::{pd0_in, pd0_out};
use svtav1_encoder::port_enc_mode_config::pd0;
use svtav1_encoder::port_enc_mode_config::pd0::Pd0Inputs;

#[derive(Clone, Copy)]
struct Case {
    pd0_level: u8,
    is_islice: bool,
    allintra: bool,
    rtc: bool,
    is_leaf: bool,
    enc_mode: i8,
    transition: bool,
    pred_depth_only: bool,
    ctx_hbd: bool,
    pcs_hbd: bool,
    lambda8: u32,
    lambda10: u32,
    me64_dist: u32,
    me8_var: u32,
    me8_dist: u32,
    base_q: u32,
    bias_weight: u32,
    rate_est: u8,
    disallow_4x4: bool,
    disallow_8x8: bool,
    dr_enabled: bool,
    dr_b16: bool,
    dr_b32: bool,
    dr_b64: bool,
    b64_complete: bool,
}

impl Default for Case {
    fn default() -> Self {
        Self {
            pd0_level: 0,
            is_islice: false,
            allintra: false,
            rtc: false,
            is_leaf: false,
            enc_mode: 5,
            transition: false,
            pred_depth_only: false,
            ctx_hbd: false,
            pcs_hbd: false,
            lambda8: 1000,
            lambda10: 4000,
            me64_dist: 50_000,
            me8_var: 1500,
            me8_dist: 3000,
            base_q: 128,
            bias_weight: 0,
            rate_est: 0,
            disallow_4x4: true,
            disallow_8x8: false,
            dr_enabled: false,
            dr_b16: false,
            dr_b32: false,
            dr_b64: false,
            b64_complete: true,
        }
    }
}

fn build_input(c: &Case) -> [i32; pd0_in::COUNT] {
    let mut i = [0i32; pd0_in::COUNT];
    i[pd0_in::LEVEL] = i32::from(c.pd0_level);
    i[pd0_in::IS_ISLICE] = i32::from(c.is_islice);
    i[pd0_in::ALLINTRA] = i32::from(c.allintra);
    i[pd0_in::RTC] = i32::from(c.rtc);
    // C `frame_is_leaf` is `update_type == SVT_AV1_LF_UPDATE` (== 1).
    i[pd0_in::UPDATE_TYPE] = if c.is_leaf { 1 } else { 0 };
    i[pd0_in::ENC_MODE] = i32::from(c.enc_mode);
    i[pd0_in::TRANSITION] = i32::from(c.transition);
    i[pd0_in::PRED_DEPTH_ONLY] = i32::from(c.pred_depth_only);
    i[pd0_in::CTX_HBD] = i32::from(c.ctx_hbd);
    i[pd0_in::PCS_HBD] = i32::from(c.pcs_hbd);
    i[pd0_in::LAMBDA8] = c.lambda8 as i32;
    i[pd0_in::LAMBDA10] = c.lambda10 as i32;
    i[pd0_in::ME64_DIST] = c.me64_dist as i32;
    i[pd0_in::ME8_VAR] = c.me8_var as i32;
    i[pd0_in::ME8_DIST] = c.me8_dist as i32;
    i[pd0_in::BASE_Q] = c.base_q as i32;
    i[pd0_in::BIAS_WEIGHT] = c.bias_weight as i32;
    i[pd0_in::RATE_EST] = i32::from(c.rate_est);
    i[pd0_in::DISALLOW_4X4] = i32::from(c.disallow_4x4);
    i[pd0_in::DISALLOW_8X8] = i32::from(c.disallow_8x8);
    i[pd0_in::DR_ENABLED] = i32::from(c.dr_enabled);
    i[pd0_in::DR_B16] = i32::from(c.dr_b16);
    i[pd0_in::DR_B32] = i32::from(c.dr_b32);
    i[pd0_in::DR_B64] = i32::from(c.dr_b64);
    i[pd0_in::B64_COMPLETE] = i32::from(c.b64_complete);
    i[pd0_in::SB_SIZE] = 64;
    i
}

fn to_port(c: &Case) -> Pd0Inputs {
    Pd0Inputs {
        pd0_level: c.pd0_level,
        is_islice: c.is_islice,
        allintra: c.allintra,
        rtc_tune: c.rtc,
        is_not_last_layer: !c.is_leaf,
        enc_mode: c.enc_mode,
        transition_present: c.transition,
        pic_pred_depth_only: c.pred_depth_only,
        ctx_hbd_md: c.ctx_hbd,
        pcs_hbd_md: c.pcs_hbd,
        fast_lambda_8bit: c.lambda8,
        fast_lambda_10bit: c.lambda10,
        me_64x64_distortion: c.me64_dist,
        me_8x8_cost_variance: c.me8_var,
        me_8x8_distortion: c.me8_dist,
        base_q_idx: c.base_q,
        pd0_cost_bias_weight: c.bias_weight,
        rate_est_level: c.rate_est,
        disallow_4x4: c.disallow_4x4,
        disallow_8x8: c.disallow_8x8,
        depth_removal_enabled: c.dr_enabled,
        disallow_below_16x16: c.dr_b16,
        disallow_below_32x32: c.dr_b32,
        disallow_below_64x64: c.dr_b64,
        b64_is_complete: c.b64_complete,
        super_block_size: 64,
    }
}

/// Compare the fields the port models. `intra_ctrls` and `uv_mode` come from
/// unported tables and are checked separately.
fn assert_case(c: &Case, ctx_msg: &str) {
    let ours = pd0::sig_deriv_enc_dec_pd0(to_port(c)).expect("derived levels in range");
    let theirs = cref::sig_deriv_enc_dec_pd0(&build_input(c));

    assert_eq!(
        i64::from(ours.md_disallow_nsq_search),
        theirs[pd0_out::NSQ_OFF],
        "md_disallow_nsq_search {ctx_msg}"
    );
    assert_eq!(
        i64::from(ours.shut_fast_rate),
        theirs[pd0_out::SHUT_FAST_RATE],
        "shut_fast_rate {ctx_msg}"
    );
    assert_eq!(
        (
            i64::from(ours.depth_early_exit.split_cost_th),
            i64::from(ours.depth_early_exit.early_exit_th)
        ),
        (theirs[pd0_out::DEE_SPLIT], theirs[pd0_out::DEE_EXIT]),
        "depth_early_exit {ctx_msg}"
    );
    assert_eq!(
        i64::from(ours.parent_cost_bias),
        theirs[pd0_out::PARENT_BIAS],
        "parent_cost_bias {ctx_msg}"
    );
    assert_eq!(
        i64::from(ours.pd0_use_src_samples),
        theirs[pd0_out::USE_SRC],
        "pd0_use_src_samples {ctx_msg}"
    );
    assert_eq!(
        ours.pf.pf_shape as i64,
        theirs[pd0_out::PF_SHAPE],
        "pf_shape {ctx_msg}"
    );
    assert_eq!(
        (
            i64::from(ours.subres.step),
            i64::from(ours.subres.odd_to_even_deviation_th)
        ),
        (theirs[pd0_out::SUBRES_STEP], theirs[pd0_out::SUBRES_DEV]),
        "subres {ctx_msg}"
    );
    assert_eq!(
        i64::from(ours.approx_inter_rate),
        theirs[pd0_out::APPROX_RATE],
        "approx_inter_rate {ctx_msg}"
    );
    let r = pd0_out::RATE_EST;
    assert_eq!(
        [
            i64::from(ours.rate_est.update_skip_ctx_dc_sign_ctx),
            i64::from(ours.rate_est.update_skip_coeff_ctx),
            i64::from(ours.rate_est.coeff_rate_est_lvl),
            i64::from(ours.rate_est.lpd0_qp_offset),
            i64::from(ours.rate_est.pd0_fast_coeff_est_level),
        ],
        [
            theirs[r],
            theirs[r + 1],
            theirs[r + 2],
            theirs[r + 3],
            theirs[r + 4]
        ],
        "rate_est_ctrls {ctx_msg}"
    );

    // The derived intra_level, validated through C's own set_intra_ctrls.
    // PD0 always passes dist_based_ang_intra_level = 2.
    let expect_ic = cref::set_intra_ctrls_at_level(ours.intra_level, 2, c.is_islice);
    let ic = pd0_out::INTRA_CTRLS;
    assert_eq!(
        expect_ic,
        [
            theirs[ic],
            theirs[ic + 1],
            theirs[ic + 2],
            theirs[ic + 3],
            theirs[ic + 4],
            theirs[ic + 5],
            theirs[ic + 6],
            theirs[ic + 7]
        ],
        "intra_level {} {ctx_msg}",
        ours.intra_level
    );
}

#[test]
fn pd0_matches_c_over_the_level_and_flag_product() {
    let enc_modes: [i8; 6] = [-1, 0, 5, 9, 10, 13];
    for pd0_level in 0u8..=6 {
        for &enc_mode in &enc_modes {
            for &is_islice in &[false, true] {
                for &allintra in &[false, true] {
                    for &rtc in &[false, true] {
                        for &is_leaf in &[false, true] {
                            for &transition in &[false, true] {
                                for &pred_depth_only in &[false, true] {
                                    let c = Case {
                                        pd0_level,
                                        enc_mode,
                                        is_islice,
                                        allintra,
                                        rtc,
                                        is_leaf,
                                        transition,
                                        pred_depth_only,
                                        ..Case::default()
                                    };
                                    assert_case(
                                        &c,
                                        &format!(
                                            "lvl={pd0_level} m={enc_mode} islice={is_islice} \
                                             allintra={allintra} rtc={rtc} leaf={is_leaf} \
                                             trans={transition} pdo={pred_depth_only}"
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

/// The intra-level and subres decisions both compare an `RDCOST` against a
/// threshold; sweep the distortion and lambda across the crossing point.
#[test]
fn pd0_rdcost_thresholds_match_c() {
    for &lvl in &[0u8, 1, 2, 3, 4, 5] {
        for &lambda in &[0u32, 1, 100, 1000, 10_000, 100_000] {
            for &dist in &[0u32, 1, 1000, 50_000, 500_000, 5_000_000, u32::MAX / 2] {
                for &hbd in &[false, true] {
                    for &is_islice in &[false, true] {
                        let c = Case {
                            pd0_level: lvl,
                            lambda8: lambda,
                            lambda10: lambda.wrapping_mul(3),
                            me64_dist: dist,
                            ctx_hbd: hbd,
                            is_islice,
                            ..Case::default()
                        };
                        assert_case(
                            &c,
                            &format!(
                                "lvl={lvl} lambda={lambda} dist={dist} hbd={hbd} \
                                 islice={is_islice}"
                            ),
                        );
                    }
                }
            }
        }
    }
}

/// `parent_cost_bias` at PD0_LVL_6 is the densest arithmetic on this path:
/// a base_q interpolation plus a three-band ME-variance offset, optionally
/// scaled by a weight derived from a distortion ratio, then clamped.
#[test]
fn pd0_parent_cost_bias_matches_c() {
    for &base_q in &[0u32, 1, 63, 127, 128, 200, 254, 255] {
        for &me8_var in &[0u32, 500, 501, 1000, 1001, 2000, 2001, 100_000] {
            for &bias_weight in &[0u32, 1, 512, 700, 1024, 2000] {
                for &(me64, me8) in &[
                    (0u32, 0u32),
                    (0, 1000),
                    (1000, 0),
                    (16_000, 1000),
                    (1000, 16_000),
                    (500_000, 3000),
                ] {
                    for &allintra in &[false, true] {
                        let c = Case {
                            pd0_level: 6,
                            base_q,
                            me8_var,
                            bias_weight,
                            me64_dist: me64,
                            me8_dist: me8,
                            allintra,
                            ..Case::default()
                        };
                        assert_case(
                            &c,
                            &format!(
                                "base_q={base_q} var={me8_var} w={bias_weight} \
                                 me64={me64} me8={me8} allintra={allintra}"
                            ),
                        );
                    }
                }
            }
        }
    }
}

/// The subres ladder at PD0_LVL_3..5 depends on four context flags that the
/// still path never varies.
#[test]
fn pd0_subres_ladder_matches_c() {
    for &lvl in &[3u8, 4, 5] {
        for &d4 in &[false, true] {
            for &d8 in &[false, true] {
                for &complete in &[false, true] {
                    for &dr_en in &[false, true] {
                        for &(b16, b32, b64) in &[
                            (false, false, false),
                            (true, false, false),
                            (false, true, false),
                            (false, false, true),
                        ] {
                            for &is_leaf in &[false, true] {
                                let c = Case {
                                    pd0_level: lvl,
                                    disallow_4x4: d4,
                                    disallow_8x8: d8,
                                    b64_complete: complete,
                                    dr_enabled: dr_en,
                                    dr_b16: b16,
                                    dr_b32: b32,
                                    dr_b64: b64,
                                    is_leaf,
                                    ..Case::default()
                                };
                                assert_case(
                                    &c,
                                    &format!(
                                        "lvl={lvl} d4={d4} d8={d8} complete={complete} \
                                         dr={dr_en} ({b16},{b32},{b64}) leaf={is_leaf}"
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

/// `rate_est_level` is clamped upward by the picture level; sweep both.
#[test]
fn pd0_rate_est_level_matches_c() {
    for lvl in 0u8..=6 {
        for pic_rate_est in 0u8..=4 {
            let c = Case {
                pd0_level: lvl,
                rate_est: pic_rate_est,
                ..Case::default()
            };
            assert_case(&c, &format!("lvl={lvl} pic_rate_est={pic_rate_est}"));
        }
    }
}

/// Positive controls: PD0_LVL_6 must return EARLY, leaving pf/subres/rate_est
/// and approx_inter_rate at the context's zeroed values, and the non-6 levels
/// must not.
#[test]
fn pd0_early_return_positive_control() {
    let c6 = Case {
        pd0_level: 6,
        ..Case::default()
    };
    let t6 = cref::sig_deriv_enc_dec_pd0(&build_input(&c6));
    assert_eq!(
        t6[pd0_out::APPROX_RATE],
        0,
        "LVL_6 returns before approx_inter_rate"
    );
    assert_eq!(
        t6[pd0_out::PF_SHAPE],
        0,
        "LVL_6 returns before set_pf_controls"
    );
    assert_eq!(
        t6[pd0_out::RATE_EST + 3],
        0,
        "LVL_6 leaves lpd0_qp_offset zeroed"
    );
    assert!(
        pd0::sig_deriv_enc_dec_pd0(to_port(&c6))
            .expect("in range")
            .returned_early
    );

    let c5 = Case {
        pd0_level: 5,
        rate_est: 1,
        ..Case::default()
    };
    let t5 = cref::sig_deriv_enc_dec_pd0(&build_input(&c5));
    assert_eq!(
        t5[pd0_out::APPROX_RATE],
        1,
        "LVL_5 reaches approx_inter_rate"
    );
    // set_pf_controls(ctx, 1) is DEFAULT_SHAPE == 0, so use the rate_est
    // struct as the "we got past the early return" witness: level 0 there
    // writes lpd0_qp_offset = 8, which a zeroed context does not have.
    assert_eq!(
        t5[pd0_out::RATE_EST + 3],
        8,
        "LVL_5 reaches set_rate_est_ctrls"
    );
    assert!(
        !pd0::sig_deriv_enc_dec_pd0(to_port(&c5))
            .expect("in range")
            .returned_early
    );
}

/// The intra-level cross-check is only as sharp as `set_intra_ctrls`'s
/// injectivity. Show which levels it maps to distinct control structs, so the
/// strength of the check above is on the record rather than assumed.
#[test]
fn set_intra_ctrls_separates_the_levels_pd0_can_derive() {
    // The levels sig_deriv_enc_dec_pd0 can produce: 0, 1, 8 and
    // MAX_INTRA_LEVEL-1 == 9.
    let levels = [0u8, 1, 8, 9];
    let mut seen: Vec<(u8, [i64; 8])> = Vec::new();
    for &l in &levels {
        // An I-slice would trip C's `intra_level > 0` assert at level 0.
        let ctrls = cref::set_intra_ctrls_at_level(l, 2, false);
        for (prev_l, prev) in &seen {
            assert_ne!(
                *prev, ctrls,
                "levels {prev_l} and {l} produce identical intra_ctrls, so the \
                 pd0 intra-level cross-check cannot distinguish them"
            );
        }
        seen.push((l, ctrls));
    }
}
