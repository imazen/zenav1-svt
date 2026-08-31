//! Differential parity for the ME signal derivation of
//! `Source/Lib/Codec/enc_mode_config.c` — chunk C4's parameter surface.
//!
//! **Evidence tier 1** throughout (`docs/WORKING-ON-THIS.md` §4). The two entry
//! points `svt_aom_sig_deriv_me` and `svt_aom_sig_deriv_me_tf` are EXPORTED, and
//! between them they call every `static` ME helper in the file —
//! `set_me_search_params`, `set_hme_search_params`, `svt_aom_set_prehme_ctrls`,
//! `svt_aom_set_me_hme_ref_prune_ctrls`, `svt_aom_set_me_sr_adjustment_ctrls`,
//! `svt_aom_set_mv_based_sa_ctrls`, `svt_aom_set_me_8x8_var_ctrls` and
//! `tf_set_me_hme_params_oq`. Driving the entry point on a synthetic
//! SCS/PPCS and reading the whole `MeContext` back therefore gates all eight
//! statics at tier 1, instead of eight hand-derived vector sets at tier 4.
//!
//! The C-side dump layout carries a compile-time assertion on its slot count
//! (`me_out_slot_count_check` in `shims/sigderiv_shims.c`), verified to FAIL the
//! build when the expected count is wrong — so the shim cannot silently write
//! past the Rust array.

use svtav1_cref::sig_deriv as cref;
use svtav1_cref::sig_deriv::{ME_OUT_SLOTS, me_slot as s};
use svtav1_encoder::port_enc_mode_config::ResolutionRange;
use svtav1_encoder::port_enc_mode_config::me;

const ENC_MODES: [i8; 15] = [-1, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13];

const RESOLUTIONS: [ResolutionRange; 7] = [
    ResolutionRange::R240p,
    ResolutionRange::R360p,
    ResolutionRange::R480p,
    ResolutionRange::R720p,
    ResolutionRange::R1080p,
    ResolutionRange::R4k,
    ResolutionRange::R8k,
];

/// QPs straddling the linear/exponential switch at 46 and the low clamp at 10.
const QPS: [u32; 10] = [0, 5, 10, 11, 30, 45, 46, 47, 55, 63];

fn flatten(sig: &me::MeSignals) -> [u32; ME_OUT_SLOTS] {
    let mut o = [0u32; ME_OUT_SLOTS];
    o[s::SA_MIN_W] = u32::from(sig.me_sa.sa_min.width);
    o[s::SA_MIN_H] = u32::from(sig.me_sa.sa_min.height);
    o[s::SA_MAX_W] = u32::from(sig.me_sa.sa_max.width);
    o[s::SA_MAX_H] = u32::from(sig.me_sa.sa_max.height);
    o[s::NUM_HME_W] = u32::from(sig.hme.num_hme_sa_w);
    o[s::NUM_HME_H] = u32::from(sig.hme.num_hme_sa_h);
    o[s::HME_L0_MIN_W] = u32::from(sig.hme.hme_l0_sa.sa_min.width);
    o[s::HME_L0_MIN_H] = u32::from(sig.hme.hme_l0_sa.sa_min.height);
    o[s::HME_L0_MAX_W] = u32::from(sig.hme.hme_l0_sa.sa_max.width);
    o[s::HME_L0_MAX_H] = u32::from(sig.hme.hme_l0_sa.sa_max.height);
    o[s::HME_L1_W] = u32::from(sig.hme.hme_l1_sa.width);
    o[s::HME_L1_H] = u32::from(sig.hme.hme_l1_sa.height);
    o[s::HME_L2_W] = u32::from(sig.hme.hme_l2_sa.width);
    o[s::HME_L2_H] = u32::from(sig.hme.hme_l2_sa.height);
    o[s::EN_HME] = u32::from(sig.enable_hme_flag);
    o[s::EN_HME_L0] = u32::from(sig.enable_hme_level0_flag);
    o[s::EN_HME_L1] = u32::from(sig.enable_hme_level1_flag);
    o[s::EN_HME_L2] = u32::from(sig.enable_hme_level2_flag);
    o[s::HME_METHOD] = u32::from(sig.hme_search_method);
    o[s::ME_METHOD] = u32::from(sig.me_search_method);
    o[s::RED_HME_MIN] = u32::from(sig.reduce_hme_l0_sr_th_min);
    o[s::RED_HME_MAX] = u32::from(sig.reduce_hme_l0_sr_th_max);
    o[s::PREHME_EN] = u32::from(sig.prehme_ctrl.enable);
    o[s::PREHME_V_MIN_W] = u32::from(sig.prehme_ctrl.prehme_sa_cfg_vert.sa_min.width);
    o[s::PREHME_V_MIN_H] = u32::from(sig.prehme_ctrl.prehme_sa_cfg_vert.sa_min.height);
    o[s::PREHME_V_MAX_W] = u32::from(sig.prehme_ctrl.prehme_sa_cfg_vert.sa_max.width);
    o[s::PREHME_V_MAX_H] = u32::from(sig.prehme_ctrl.prehme_sa_cfg_vert.sa_max.height);
    o[s::PREHME_H_MIN_W] = u32::from(sig.prehme_ctrl.prehme_sa_cfg_horz.sa_min.width);
    o[s::PREHME_H_MIN_H] = u32::from(sig.prehme_ctrl.prehme_sa_cfg_horz.sa_min.height);
    o[s::PREHME_H_MAX_W] = u32::from(sig.prehme_ctrl.prehme_sa_cfg_horz.sa_max.width);
    o[s::PREHME_H_MAX_H] = u32::from(sig.prehme_ctrl.prehme_sa_cfg_horz.sa_max.height);
    o[s::PREHME_SKIP_LINE] = u32::from(sig.prehme_ctrl.skip_search_line);
    o[s::PREHME_L1_EXIT] = u32::from(sig.prehme_ctrl.l1_early_exit);
    o[s::PRUNE_EN] = u32::from(sig.me_hme_prune_ctrls.enable_me_hme_ref_pruning);
    o[s::PRUNE_HME_DEV] = u32::from(
        sig.me_hme_prune_ctrls
            .prune_ref_if_hme_sad_dev_bigger_than_th,
    );
    o[s::PRUNE_ME_DEV] = u32::from(
        sig.me_hme_prune_ctrls
            .prune_ref_if_me_sad_dev_bigger_than_th,
    );
    o[s::PRUNE_ZZ_TH] = sig.me_hme_prune_ctrls.zz_sad_th;
    o[s::PRUNE_ZZ_PCT] = sig.me_hme_prune_ctrls.zz_sad_pct;
    o[s::PRUNE_PHME_TH] = sig.me_hme_prune_ctrls.phme_sad_th;
    o[s::PRUNE_PHME_PCT] = sig.me_hme_prune_ctrls.phme_sad_pct;
    o[s::SR_EN] = u32::from(sig.me_sr_adjustment_ctrls.enable_me_sr_adjustment);
    o[s::SR_MV_LEN_TH] = u32::from(
        sig.me_sr_adjustment_ctrls
            .reduce_me_sr_based_on_mv_length_th,
    );
    o[s::SR_STAT_TH] = sig.me_sr_adjustment_ctrls.stationary_hme_sad_abs_th;
    o[s::SR_STAT_DIV] = u32::from(sig.me_sr_adjustment_ctrls.stationary_me_sr_divisor);
    o[s::SR_RED_TH] = sig
        .me_sr_adjustment_ctrls
        .reduce_me_sr_based_on_hme_sad_abs_th;
    o[s::SR_LOW_DIV] = u32::from(sig.me_sr_adjustment_ctrls.me_sr_divisor_for_low_hme_sad);
    o[s::SR_DIST_RESIZE] = u32::from(sig.me_sr_adjustment_ctrls.distance_based_hme_resizing);
    o[s::MVSA_EN] = u32::from(sig.mv_based_sa_adj.enabled);
    o[s::MVSA_NEAREST] = u32::from(sig.mv_based_sa_adj.nearest_ref_only);
    o[s::MVSA_MV_TH] = u32::from(sig.mv_based_sa_adj.mv_size_th);
    o[s::MVSA_MULT] = u32::from(sig.mv_based_sa_adj.sa_multiplier);
    o[s::VAR_EN] = u32::from(sig.me_8x8_var_ctrls.enabled);
    o[s::VAR_DIV4] = sig.me_8x8_var_ctrls.me_sr_div4_th;
    o[s::VAR_DIV2] = sig.me_8x8_var_ctrls.me_sr_div2_th;
    o[s::VAR_MULT2] = sig.me_8x8_var_ctrls.me_sr_mult2_th;
    o[s::PRUNE_CAND_TH] = u32::from(sig.prune_me_candidates_th);
    o[s::SC_BOOST] = u32::from(sig.sc_class_me_boost);
    o[s::BEST_UNIPRED] = u32::from(sig.use_best_unipred_cand_only);
    o[s::EARLY_EXIT] = sig.me_early_exit_th;
    o[s::STATIC_B64] = sig.me_static_b64_th;
    o[s::SAFE_ZZ] = sig.me_safe_limit_zz_th;
    o[s::PREV_STAGE] = sig.prev_me_stage_based_exit_th;
    o
}

fn flatten_tf(sig: &me::MeTfSignals) -> [u32; ME_OUT_SLOTS] {
    let mut o = [0u32; ME_OUT_SLOTS];
    o[s::SA_MIN_W] = u32::from(sig.params.me_sa.sa_min.width);
    o[s::SA_MIN_H] = u32::from(sig.params.me_sa.sa_min.height);
    o[s::SA_MAX_W] = u32::from(sig.params.me_sa.sa_max.width);
    o[s::SA_MAX_H] = u32::from(sig.params.me_sa.sa_max.height);
    o[s::NUM_HME_W] = u32::from(sig.params.num_hme_sa_w);
    o[s::NUM_HME_H] = u32::from(sig.params.num_hme_sa_h);
    o[s::HME_L0_MIN_W] = u32::from(sig.params.hme_l0_sa_default_tf.sa_min.width);
    o[s::HME_L0_MIN_H] = u32::from(sig.params.hme_l0_sa_default_tf.sa_min.height);
    o[s::HME_L0_MAX_W] = u32::from(sig.params.hme_l0_sa_default_tf.sa_max.width);
    o[s::HME_L0_MAX_H] = u32::from(sig.params.hme_l0_sa_default_tf.sa_max.height);
    o[s::HME_L1_W] = u32::from(sig.params.hme_l1_sa.width);
    o[s::HME_L1_H] = u32::from(sig.params.hme_l1_sa.height);
    o[s::HME_L2_W] = u32::from(sig.params.hme_l2_sa.width);
    o[s::HME_L2_H] = u32::from(sig.params.hme_l2_sa.height);
    o[s::EN_HME] = u32::from(sig.enable_hme_flag);
    o[s::EN_HME_L0] = u32::from(sig.enable_hme_level0_flag);
    o[s::EN_HME_L1] = u32::from(sig.enable_hme_level1_flag);
    o[s::EN_HME_L2] = u32::from(sig.enable_hme_level2_flag);
    o[s::HME_METHOD] = u32::from(sig.hme_search_method);
    o[s::ME_METHOD] = u32::from(sig.me_search_method);
    o[s::RED_HME_MIN] = u32::from(sig.reduce_hme_l0_sr_th_min);
    o[s::RED_HME_MAX] = u32::from(sig.reduce_hme_l0_sr_th_max);
    o[s::PREHME_EN] = u32::from(sig.prehme_ctrl.enable);
    o[s::PREHME_V_MIN_W] = u32::from(sig.prehme_ctrl.prehme_sa_cfg_vert.sa_min.width);
    o[s::PREHME_V_MIN_H] = u32::from(sig.prehme_ctrl.prehme_sa_cfg_vert.sa_min.height);
    o[s::PREHME_V_MAX_W] = u32::from(sig.prehme_ctrl.prehme_sa_cfg_vert.sa_max.width);
    o[s::PREHME_V_MAX_H] = u32::from(sig.prehme_ctrl.prehme_sa_cfg_vert.sa_max.height);
    o[s::PREHME_H_MIN_W] = u32::from(sig.prehme_ctrl.prehme_sa_cfg_horz.sa_min.width);
    o[s::PREHME_H_MIN_H] = u32::from(sig.prehme_ctrl.prehme_sa_cfg_horz.sa_min.height);
    o[s::PREHME_H_MAX_W] = u32::from(sig.prehme_ctrl.prehme_sa_cfg_horz.sa_max.width);
    o[s::PREHME_H_MAX_H] = u32::from(sig.prehme_ctrl.prehme_sa_cfg_horz.sa_max.height);
    o[s::PREHME_SKIP_LINE] = u32::from(sig.prehme_ctrl.skip_search_line);
    o[s::PREHME_L1_EXIT] = u32::from(sig.prehme_ctrl.l1_early_exit);
    o[s::PRUNE_EN] = u32::from(sig.me_hme_prune_ctrls.enable_me_hme_ref_pruning);
    o[s::PRUNE_HME_DEV] = u32::from(
        sig.me_hme_prune_ctrls
            .prune_ref_if_hme_sad_dev_bigger_than_th,
    );
    o[s::PRUNE_ME_DEV] = u32::from(
        sig.me_hme_prune_ctrls
            .prune_ref_if_me_sad_dev_bigger_than_th,
    );
    o[s::PRUNE_ZZ_TH] = sig.me_hme_prune_ctrls.zz_sad_th;
    o[s::PRUNE_ZZ_PCT] = sig.me_hme_prune_ctrls.zz_sad_pct;
    o[s::PRUNE_PHME_TH] = sig.me_hme_prune_ctrls.phme_sad_th;
    o[s::PRUNE_PHME_PCT] = sig.me_hme_prune_ctrls.phme_sad_pct;
    o[s::SR_EN] = u32::from(sig.me_sr_adjustment_ctrls.enable_me_sr_adjustment);
    o[s::SR_MV_LEN_TH] = u32::from(
        sig.me_sr_adjustment_ctrls
            .reduce_me_sr_based_on_mv_length_th,
    );
    o[s::SR_STAT_TH] = sig.me_sr_adjustment_ctrls.stationary_hme_sad_abs_th;
    o[s::SR_STAT_DIV] = u32::from(sig.me_sr_adjustment_ctrls.stationary_me_sr_divisor);
    o[s::SR_RED_TH] = sig
        .me_sr_adjustment_ctrls
        .reduce_me_sr_based_on_hme_sad_abs_th;
    o[s::SR_LOW_DIV] = u32::from(sig.me_sr_adjustment_ctrls.me_sr_divisor_for_low_hme_sad);
    o[s::SR_DIST_RESIZE] = u32::from(sig.me_sr_adjustment_ctrls.distance_based_hme_resizing);
    o[s::MVSA_EN] = u32::from(sig.mv_based_sa_adj.enabled);
    o[s::MVSA_NEAREST] = u32::from(sig.mv_based_sa_adj.nearest_ref_only);
    o[s::MVSA_MV_TH] = u32::from(sig.mv_based_sa_adj.mv_size_th);
    o[s::MVSA_MULT] = u32::from(sig.mv_based_sa_adj.sa_multiplier);
    o[s::VAR_EN] = u32::from(sig.me_8x8_var_ctrls.enabled);
    o[s::VAR_DIV4] = sig.me_8x8_var_ctrls.me_sr_div4_th;
    o[s::VAR_DIV2] = sig.me_8x8_var_ctrls.me_sr_div2_th;
    o[s::VAR_MULT2] = sig.me_8x8_var_ctrls.me_sr_mult2_th;
    // The TF shim reports these three as 0 (they are not part of the TF path).
    o[s::PRUNE_CAND_TH] = 0;
    o[s::SC_BOOST] = u32::from(sig.sc_class_me_boost);
    o[s::BEST_UNIPRED] = 0;
    o[s::EARLY_EXIT] = sig.me_early_exit_th;
    o[s::STATIC_B64] = 0;
    o[s::SAFE_ZZ] = sig.me_safe_limit_zz_th;
    o[s::PREV_STAGE] = sig.prev_me_stage_based_exit_th;
    o
}

/// The qp-scaling helper on its own, over the whole qp domain and both
/// enable states. It is exercised through the search-area sweeps too, but a
/// direct check localizes a `exp()`/truncation divergence instead of leaving
/// it to surface as a wrong search area.
#[test]
fn qp_based_th_scaling_factors_are_the_c_formula() {
    // `svt_aom_get_qp_based_th_scaling_factors` is exported, but its result is
    // only observable through a caller; both callers are swept below. This
    // test pins the two structural facts of the formula against the C source:
    // the low clamp at 10/63 and the switch to the exponential form at qp 46.
    assert_eq!(me::get_qp_based_th_scaling_factors(false, 40), (1, 1));
    assert_eq!(me::get_qp_based_th_scaling_factors(true, 0), (10, 63));
    assert_eq!(me::get_qp_based_th_scaling_factors(true, 10), (10, 63));
    assert_eq!(me::get_qp_based_th_scaling_factors(true, 45), (45, 63));
    let (w, d) = me::get_qp_based_th_scaling_factors(true, 46);
    assert_eq!(d, 10000);
    // (1.05 - exp(-(46-35)/10)) * 10000 = 7171.289... -> 7171 after the C cast
    // to uint32_t, which TRUNCATES rather than rounding.
    assert_eq!(w, 7171);
}

#[test]
fn sig_deriv_me_matches_c() {
    for &m in &ENC_MODES {
        for &r in &RESOLUTIONS {
            for &rtc in &[false, true] {
                for &is_base in &[false, true] {
                    for &hl in &[0u8, 4] {
                        for &qp in &QPS {
                            for &scaling in &[false, true] {
                                for &(l1, l2) in &[(1u8, 1u8), (1, 0), (0, 0)] {
                                    let args = cref::MeArgs {
                                        enc_mode: m,
                                        sc_class5: 1,
                                        input_resolution: r.as_u8(),
                                        rtc,
                                        is_base,
                                        hierarchical_levels: hl,
                                        en_hme: 1,
                                        en_hme_l0: 1,
                                        en_hme_l1: l1,
                                        en_hme_l2: l2,
                                        use_best_unipred: 1,
                                        me_qp_scaling: scaling,
                                        hme_qp_scaling: scaling,
                                        qp,
                                        safe_limit_nref: 1,
                                        safe_limit_zz_th: 4242,
                                    };
                                    let ours = me::sig_deriv_me(me::MeDerivInputs {
                                        enc_mode: m,
                                        sc_class5: 1,
                                        input_resolution: r,
                                        rtc_tune: rtc,
                                        is_base,
                                        hierarchical_levels: hl,
                                        enable_hme_flag: 1,
                                        enable_hme_level0_flag: 1,
                                        enable_hme_level1_flag: l1,
                                        enable_hme_level2_flag: l2,
                                        use_best_me_unipred_cand_only: 1,
                                        me_qp_based_th_scaling: scaling,
                                        hme_qp_based_th_scaling: scaling,
                                        qp,
                                        safe_limit_nref: 1,
                                        safe_limit_zz_th: 4242,
                                    });
                                    assert_eq!(
                                        flatten(&ours),
                                        cref::sig_deriv_me(args),
                                        "enc_mode={m} res={r:?} rtc={rtc} is_base={is_base} \
                                         hl={hl} qp={qp} scaling={scaling} hme_l1={l1} hme_l2={l2}"
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

/// `sc_class5 == 0` takes the other arm of the ME boost, and
/// `safe_limit_nref != 1` the other arm of `me_safe_limit_zz_th`; sweep both so
/// neither branch is left unmeasured.
#[test]
fn sig_deriv_me_other_arms_match_c() {
    for &m in &ENC_MODES {
        for &sc5 in &[0u8, 1] {
            for &nref in &[0u8, 1, 2] {
                for &ubu in &[0u8, 1] {
                    let args = cref::MeArgs {
                        enc_mode: m,
                        sc_class5: sc5,
                        input_resolution: ResolutionRange::R1080p.as_u8(),
                        rtc: false,
                        is_base: true,
                        hierarchical_levels: 4,
                        en_hme: 1,
                        en_hme_l0: 1,
                        en_hme_l1: 1,
                        en_hme_l2: 1,
                        use_best_unipred: ubu,
                        me_qp_scaling: false,
                        hme_qp_scaling: false,
                        qp: 35,
                        safe_limit_nref: nref,
                        safe_limit_zz_th: 777,
                    };
                    let ours = me::sig_deriv_me(me::MeDerivInputs {
                        enc_mode: m,
                        sc_class5: sc5,
                        input_resolution: ResolutionRange::R1080p,
                        rtc_tune: false,
                        is_base: true,
                        hierarchical_levels: 4,
                        enable_hme_flag: 1,
                        enable_hme_level0_flag: 1,
                        enable_hme_level1_flag: 1,
                        enable_hme_level2_flag: 1,
                        use_best_me_unipred_cand_only: ubu,
                        me_qp_based_th_scaling: false,
                        hme_qp_based_th_scaling: false,
                        qp: 35,
                        safe_limit_nref: nref,
                        safe_limit_zz_th: 777,
                    });
                    assert_eq!(
                        flatten(&ours),
                        cref::sig_deriv_me(args),
                        "enc_mode={m} sc5={sc5} nref={nref} ubu={ubu}"
                    );
                }
            }
        }
    }
}

#[test]
fn sig_deriv_me_tf_matches_c() {
    for level in 0u8..=4 {
        for &r in &RESOLUTIONS {
            for &qp_opt in &[false, true] {
                for &scaling in &[false, true] {
                    for &qp in &QPS {
                        for &(f, l0, l1, l2) in &[(1u8, 1u8, 1u8, 1u8), (1, 1, 1, 0), (0, 0, 0, 0)]
                        {
                            let ours =
                                me::sig_deriv_me_tf(level, r, qp_opt, scaling, qp, f, l0, l1, l2)
                                    .expect("hme_me_level in range");
                            let theirs = cref::sig_deriv_me_tf(
                                level,
                                r.as_u8(),
                                qp_opt,
                                scaling,
                                qp,
                                f,
                                l0,
                                l1,
                                l2,
                            );
                            assert_eq!(
                                flatten_tf(&ours),
                                theirs,
                                "hme_me_level={level} res={r:?} qp_opt={qp_opt} \
                                 scaling={scaling} qp={qp} flags=({f},{l0},{l1},{l2})"
                            );
                        }
                    }
                }
            }
        }
    }
    assert!(
        me::sig_deriv_me_tf(5, ResolutionRange::R1080p, false, false, 35, 1, 1, 1, 1).is_none()
    );
}

/// Positive controls proving the sweeps above are not comparing two all-zero
/// dumps: at p0 the ME search area is large and pre-HME is on; at p13 it
/// collapses to 8x1 and pre-HME is off.
#[test]
fn sig_deriv_me_positive_controls() {
    let base = |m: i8| cref::MeArgs {
        enc_mode: m,
        sc_class5: 0,
        input_resolution: ResolutionRange::R1080p.as_u8(),
        rtc: false,
        is_base: true,
        hierarchical_levels: 4,
        en_hme: 1,
        en_hme_l0: 1,
        en_hme_l1: 1,
        en_hme_l2: 1,
        use_best_unipred: 0,
        me_qp_scaling: false,
        hme_qp_scaling: false,
        qp: 35,
        safe_limit_nref: 0,
        safe_limit_zz_th: 0,
    };
    let p0 = cref::sig_deriv_me(base(0));
    assert_eq!((p0[s::SA_MIN_W], p0[s::SA_MIN_H]), (84, 84), "p0 me_sa min");
    assert_eq!(p0[s::PREHME_EN], 1, "p0 pre-HME on");
    assert_eq!(p0[s::PRUNE_CAND_TH], 0, "p0 prunes no ME candidates");

    let p13 = cref::sig_deriv_me(base(13));
    assert_eq!(
        (p13[s::SA_MIN_W], p13[s::SA_MIN_H]),
        (8, 3),
        "p13 me_sa min"
    );
    assert_eq!(p13[s::PREHME_EN], 0, "p13 pre-HME off");
    assert_eq!(p13[s::PRUNE_CAND_TH], 65, "p13 prunes ME candidates");

    // TF: level 0 must give the widest search area, level 4 the narrowest.
    let tf0 = cref::sig_deriv_me_tf(
        0,
        ResolutionRange::R1080p.as_u8(),
        false,
        false,
        35,
        1,
        1,
        1,
        1,
    );
    assert_eq!((tf0[s::SA_MIN_W], tf0[s::SA_MAX_W]), (60, 120));
    assert_eq!(tf0[s::HME_METHOD], u32::from(me::FULL_SAD_SEARCH));
    let tf4 = cref::sig_deriv_me_tf(
        4,
        ResolutionRange::R1080p.as_u8(),
        false,
        false,
        35,
        1,
        1,
        1,
        1,
    );
    assert_eq!((tf4[s::SA_MIN_W], tf4[s::SA_MAX_W]), (8, 8));
    assert_eq!(tf4[s::HME_METHOD], u32::from(me::SUB_SAD_SEARCH));
}
