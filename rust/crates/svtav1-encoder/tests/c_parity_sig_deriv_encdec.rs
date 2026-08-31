//! Differential parity for `svt_aom_sig_deriv_enc_dec_default` and the twelve
//! file-`static` PD1 tables it drives (`Source/Lib/Codec/enc_mode_config.c`).
//!
//! **Evidence tier 1** (`docs/WORKING-ON-THIS.md` §4).
//! `svt_aom_sig_deriv_enc_dec_default(pcs, ctx)` is EXPORTED and reaches each
//! `static` table by passing one picture-level level into it, so driving that
//! entry point on a synthetic `PictureControlSet` and reading the resulting
//! `ModeDecisionContext` back gates all of them against the real symbol —
//! `set_subres_controls`, `set_pf_controls`,
//! `set_spatial_sse_full_loop_level`, `set_tx_shortcut_ctrls`,
//! `set_coeff_shaving_controls`, `set_depth_early_exit_ctrls`,
//! `set_skip_sub_depth_ctrls`, `md_nsq_motion_search_controls`,
//! `md_sq_motion_search_controls`, `md_subpel_me_controls`,
//! `md_subpel_pme_controls` and `set_obmc_controls` /
//! `set_inter_comp_controls` / `set_inter_intra_ctrls` /
//! `set_interpolation_search_level_ctrls` (the last four also carry tier-4
//! traced vectors in `c_parity_sig_deriv_ctrls.rs`; here they get tier 1).
//!
//! What is deliberately NOT compared, and why: the twelve control structs
//! written by tables this lane has not ported (`nsq_search_ctrls`,
//! `nic_ctrls`, `cand_reduction_ctrls`, `txt_ctrls`, `uv_ctrls`, `cfl_ctrls`,
//! `rdoq_ctrls`, `txs_ctrls`, `filter_intra_ctrls`, `rate_est_ctrls`,
//! `intra_ctrls`, `mds0_ctrls`). Their picture levels ARE varied in the sweep,
//! so a port that accidentally depended on one would still be caught.

use svtav1_cref::sig_deriv as cref;
use svtav1_cref::sig_deriv::{ED_OUT_SLOTS, ed_in};
use svtav1_encoder::port_enc_mode_config::encdec;
use svtav1_encoder::port_enc_mode_config::encdec::EncDecDefaultInputs;

// Output slot indices, mirroring the C shim's `ED_O_*` enum. A drift is caught
// by the slot-count assertion inside `cref::sig_deriv_enc_dec_default`.
const O_SUBRES_STEP: usize = 0;
const O_SUBRES_DEV: usize = 1;
const O_PF_SHAPE: usize = 2;
const O_SSSE_LEVEL: usize = 3;
const O_TXSC: usize = 4; // 4 fields
const O_SHAVE: usize = 8; // 4 fields
const O_DEE: usize = 12; // 2 fields
const O_SSD: usize = 14; // 4 fields
const O_NSQME: usize = 18; // 5 fields
const O_SQME: usize = 23; // 22 fields
const O_SPME: usize = 45; // 13 fields
const O_SPPME: usize = 58; // 13 fields
const O_OBMC: usize = 71; // 7 fields
const O_II: usize = 78; // 4 fields
const O_IC: usize = 82; // 13 fields
const O_IFS_LEVEL: usize = 95;
const O_GM_INJ: usize = 96;
const O_NN_INJ: usize = 97;
const O_NNC_INJ: usize = 98;
const O_UNI3X3_INJ: usize = 99;
const O_ALLOW_IBC: usize = 100;
const O_PALETTE_LVL: usize = 101;
const O_APPROX_RATE: usize = 102;
const O_SHUT_FAST_RATE: usize = 103;
const O_MDS0_HADAMARD: usize = 104;
const O_PARENT_COST_BIAS: usize = 105;
const O_TUNE_SSIM: usize = 106;
const O_UV_MODE: usize = 107;

fn flatten(s: &encdec::EncDecDefaultSignals) -> [i64; ED_OUT_SLOTS] {
    let mut o = [0i64; ED_OUT_SLOTS];
    o[O_SUBRES_STEP] = i64::from(s.subres.step);
    o[O_SUBRES_DEV] = i64::from(s.subres.odd_to_even_deviation_th);
    o[O_PF_SHAPE] = s.pf.pf_shape as i64;
    o[O_SSSE_LEVEL] = s.spatial_sse.level as i64;
    o[O_TXSC] = i64::from(s.tx_shortcut.bypass_tx_th);
    o[O_TXSC + 1] = i64::from(s.tx_shortcut.apply_pf_on_coeffs);
    o[O_TXSC + 2] = i64::from(s.tx_shortcut.chroma_detector_level);
    o[O_TXSC + 3] = i64::from(s.tx_shortcut.use_mds3_shortcuts_th);
    o[O_SHAVE] = i64::from(s.coeff_shaving.enabled);
    o[O_SHAVE + 1] = i64::from(s.coeff_shaving.level_threshold);
    o[O_SHAVE + 2] = i64::from(s.coeff_shaving.zero_gap_threshold);
    o[O_SHAVE + 3] = i64::from(s.coeff_shaving.rd_zero_strength);
    o[O_DEE] = i64::from(s.depth_early_exit.split_cost_th);
    o[O_DEE + 1] = i64::from(s.depth_early_exit.early_exit_th);
    o[O_SSD] = i64::from(s.skip_sub_depth.enabled);
    o[O_SSD + 1] = i64::from(s.skip_sub_depth.max_size);
    o[O_SSD + 2] = i64::from(s.skip_sub_depth.quad_deviation_th);
    o[O_SSD + 3] = i64::from(s.skip_sub_depth.coeff_perc);
    o[O_NSQME] = i64::from(s.md_nsq_me.enabled);
    o[O_NSQME + 1] = s.md_nsq_me.dist_type as i64;
    o[O_NSQME + 2] = i64::from(s.md_nsq_me.full_pel_search_width);
    o[O_NSQME + 3] = i64::from(s.md_nsq_me.full_pel_search_height);
    o[O_NSQME + 4] = i64::from(s.md_nsq_me.enable_psad);
    let q = &s.md_sq_me;
    o[O_SQME] = i64::from(q.enabled);
    o[O_SQME + 1] = q.dist_type as i64;
    o[O_SQME + 2] = i64::from(q.pame_distortion_th);
    o[O_SQME + 3] = i64::from(q.sprs_lev0_enabled);
    o[O_SQME + 4] = i64::from(q.sprs_lev0_step);
    o[O_SQME + 5] = i64::from(q.sprs_lev0_w);
    o[O_SQME + 6] = i64::from(q.sprs_lev0_h);
    o[O_SQME + 7] = i64::from(q.max_sprs_lev0_w);
    o[O_SQME + 8] = i64::from(q.max_sprs_lev0_h);
    o[O_SQME + 9] = i64::from(q.sprs_lev0_multiplier);
    o[O_SQME + 10] = i64::from(q.sprs_lev1_enabled);
    o[O_SQME + 11] = i64::from(q.sprs_lev1_step);
    o[O_SQME + 12] = i64::from(q.sprs_lev1_w);
    o[O_SQME + 13] = i64::from(q.sprs_lev1_h);
    o[O_SQME + 14] = i64::from(q.max_sprs_lev1_w);
    o[O_SQME + 15] = i64::from(q.max_sprs_lev1_h);
    o[O_SQME + 16] = i64::from(q.sprs_lev1_multiplier);
    o[O_SQME + 17] = i64::from(q.sprs_lev2_enabled);
    o[O_SQME + 18] = i64::from(q.sprs_lev2_step);
    o[O_SQME + 19] = i64::from(q.sprs_lev2_w);
    o[O_SQME + 20] = i64::from(q.sprs_lev2_h);
    o[O_SQME + 21] = i64::from(q.enable_psad);
    for (base, c) in [(O_SPME, &s.md_subpel_me), (O_SPPME, &s.md_subpel_pme)] {
        o[base] = i64::from(c.enabled);
        o[base + 1] = i64::from(c.subpel_search_type);
        o[base + 2] = i64::from(c.max_precision);
        o[base + 3] = i64::from(c.subpel_search_method);
        o[base + 4] = i64::from(c.subpel_iters_per_step);
        o[base + 5] = i64::from(c.pred_variance_th);
        o[base + 6] = i64::from(c.abs_th_mult);
        o[base + 7] = i64::from(c.round_dev_th);
        o[base + 8] = i64::from(c.skip_diag_refinement);
        o[base + 9] = i64::from(c.min_blk_sz);
        o[base + 10] = i64::from(c.mvp_th);
        o[base + 11] = i64::from(c.hp_mv_th);
        o[base + 12] = i64::from(c.bias_fp);
    }
    o[O_OBMC] = i64::from(s.obmc.enabled);
    o[O_OBMC + 1] = i64::from(s.obmc.max_blk_size_to_refine);
    o[O_OBMC + 2] = i64::from(s.obmc.max_blk_size);
    o[O_OBMC + 3] = i64::from(s.obmc.refine_level);
    o[O_OBMC + 4] = i64::from(s.obmc.trans_face_off);
    o[O_OBMC + 5] = i64::from(s.obmc.fpel_search_range);
    o[O_OBMC + 6] = i64::from(s.obmc.fpel_search_diag);
    o[O_II] = i64::from(s.inter_intra.enabled);
    o[O_II + 1] = i64::from(s.inter_intra.use_rd_model);
    o[O_II + 2] = i64::from(s.inter_intra.wedge_mode_sq);
    o[O_II + 3] = i64::from(s.inter_intra.wedge_mode_nsq);
    let ic = &s.inter_comp;
    o[O_IC] = i64::from(ic.tot_comp_types);
    o[O_IC + 1] = i64::from(ic.do_me);
    o[O_IC + 2] = i64::from(ic.do_pme);
    o[O_IC + 3] = i64::from(ic.do_nearest_nearest);
    o[O_IC + 4] = i64::from(ic.do_near_near);
    o[O_IC + 5] = i64::from(ic.do_nearest_near_new);
    o[O_IC + 6] = i64::from(ic.do_3x3_bi);
    o[O_IC + 7] = i64::from(ic.do_global);
    o[O_IC + 8] = i64::from(ic.pred0_to_pred1_mult);
    o[O_IC + 9] = i64::from(ic.max_mv_length);
    o[O_IC + 10] = i64::from(ic.skip_on_ref_info);
    o[O_IC + 11] = i64::from(ic.use_rate);
    o[O_IC + 12] = i64::from(ic.no_sym_dist);
    o[O_IFS_LEVEL] = s.ifs_level as i64;
    o[O_GM_INJ] = i64::from(s.global_mv_injection);
    o[O_NN_INJ] = i64::from(s.new_nearest_injection);
    o[O_NNC_INJ] = i64::from(s.new_nearest_near_comb_injection);
    o[O_UNI3X3_INJ] = i64::from(s.unipred3x3_injection);
    o[O_ALLOW_IBC] = i64::from(s.md_allow_intrabc);
    o[O_PALETTE_LVL] = i64::from(s.md_palette_level);
    o[O_APPROX_RATE] = i64::from(s.approx_inter_rate);
    o[O_SHUT_FAST_RATE] = i64::from(s.shut_fast_rate);
    o[O_MDS0_HADAMARD] = i64::from(s.mds0_use_hadamard_sb);
    o[O_PARENT_COST_BIAS] = i64::from(s.parent_cost_bias);
    o[O_TUNE_SSIM] = i64::from(s.tune_ssim_level);
    o
}

/// Slots this lane does NOT model, zeroed on the port side before comparing so
/// the assertion reports only ported fields. Currently just `uv_mode`, which
/// `svt_aom_set_chroma_controls` produces.
fn mask_unported(mut c: [i64; ED_OUT_SLOTS]) -> [i64; ED_OUT_SLOTS] {
    c[O_UV_MODE] = 0;
    c
}

struct Case {
    enc_mode: i8,
    is_islice: bool,
    update_type: i32,
    levels: Levels,
}

#[derive(Clone, Copy)]
struct Levels {
    tx_shortcut: u8,
    ifs: u8,
    wm: u8,
    bipred3x3: u8,
    inter_comp: u8,
    ref_prune: u8,
    spatial_sse: u8,
    coeff_shave: u8,
    obmc: u8,
    inter_intra: u8,
    md_sq_mv: u8,
    md_nsq_mv: u8,
    md_pme: u8,
    me_subpel: u8,
    pme_subpel: u8,
}

fn build_input(c: &Case, noise: i32) -> [i32; ed_in::COUNT] {
    let mut i = [0i32; ed_in::COUNT];
    i[ed_in::ENC_MODE] = i32::from(c.enc_mode);
    i[ed_in::IS_ISLICE] = i32::from(c.is_islice);
    i[ed_in::UPDATE_TYPE] = c.update_type;
    let l = c.levels;
    i[ed_in::TX_SHORTCUT] = i32::from(l.tx_shortcut);
    i[ed_in::IFS] = i32::from(l.ifs);
    i[ed_in::WM] = i32::from(l.wm);
    i[ed_in::BIPRED3X3] = i32::from(l.bipred3x3);
    i[ed_in::INTER_COMP] = i32::from(l.inter_comp);
    i[ed_in::REF_PRUNE] = i32::from(l.ref_prune);
    i[ed_in::SPATIAL_SSE] = i32::from(l.spatial_sse);
    i[ed_in::COEFF_SHAVE] = i32::from(l.coeff_shave);
    i[ed_in::OBMC] = i32::from(l.obmc);
    i[ed_in::INTER_INTRA] = i32::from(l.inter_intra);
    i[ed_in::MD_SQ_MV] = i32::from(l.md_sq_mv);
    i[ed_in::MD_NSQ_MV] = i32::from(l.md_nsq_mv);
    i[ed_in::MD_PME] = i32::from(l.md_pme);
    i[ed_in::ME_SUBPEL] = i32::from(l.me_subpel);
    i[ed_in::PME_SUBPEL] = i32::from(l.pme_subpel);
    // Direct copies the port reproduces.
    i[ed_in::UNIPRED3X3] = 1;
    i[ed_in::NN_COMB] = 1;
    i[ed_in::APPROX_INTER_RATE] = 1;
    i[ed_in::ALLOW_INTRABC] = 1;
    i[ed_in::PALETTE_LEVEL] = 3;
    i[ed_in::GM_ENABLED] = 1;
    // Levels of tables this lane did NOT port. They are varied (`noise`) so a
    // port that accidentally depended on one would diverge.
    i[ed_in::NSQ_SEARCH] = 1 + (noise % 3);
    i[ed_in::NIC] = 1 + (noise % 5);
    i[ed_in::CAND_RED] = noise % 4;
    i[ed_in::TXT] = noise % 3;
    i[ed_in::CHROMA] = 1 + (noise % 4);
    i[ed_in::CFL] = noise % 2;
    i[ed_in::RDOQ] = noise % 3;
    i[ed_in::TXS] = noise % 3;
    i[ed_in::FILTER_INTRA] = noise % 2;
    i[ed_in::RATE_EST] = noise % 3;
    i[ed_in::INTRA] = 1 + (noise % 5);
    i[ed_in::DIST_ANG_INTRA] = noise % 3;
    i[ed_in::MDS0] = noise % 3;
    i[ed_in::ME_8X8_DIST] = 1000 * noise;
    i[ed_in::ME_8X8_VAR] = 500 * noise;
    i[ed_in::PICTURE_QP] = 20 + noise;
    i[ed_in::REF_SKIP_PERC] = (10 * noise) % 100;
    i
}

fn to_port_inputs(c: &Case) -> EncDecDefaultInputs {
    let l = c.levels;
    EncDecDefaultInputs {
        enc_mode: c.enc_mode,
        is_islice: c.is_islice,
        // C `frame_is_leaf` is `update_type == SVT_AV1_LF_UPDATE` (== 1).
        is_leaf: c.update_type == 1,
        tx_shortcut_level: l.tx_shortcut,
        interpolation_search_level: l.ifs,
        wm_level: l.wm,
        bipred3x3_injection: l.bipred3x3,
        unipred3x3_injection: 1,
        new_nearest_near_comb_injection: 1,
        inter_compound_mode: l.inter_comp,
        dist_based_ref_pruning: l.ref_prune,
        spatial_sse_full_loop_level: l.spatial_sse,
        coeff_shaving_level: l.coeff_shave,
        pic_obmc_level: l.obmc,
        inter_intra_level: l.inter_intra,
        md_sq_mv_search_level: l.md_sq_mv,
        md_nsq_mv_search_level: l.md_nsq_mv,
        md_pme_level: l.md_pme,
        me_subpel_level: l.me_subpel,
        pme_subpel_level: l.pme_subpel,
        approx_inter_rate: 1,
        allow_intrabc: 1,
        palette_level: 3,
        gm_enabled: 1,
    }
}

/// The full level domain of every table this lane ported, walked
/// independently: each table's levels are swept while the others cycle, so
/// every level of every ported table is reached, and the cross product stays
/// tractable.
#[test]
fn sig_deriv_enc_dec_default_matches_c() {
    let enc_modes: [i8; 15] = [-1, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13];
    let mut n = 0i32;
    for &m in &enc_modes {
        for &islice in &[false, true] {
            // update_type: 0 = KF, 1 = LF (leaf), 3 = ARF.
            for &ut in &[0i32, 1, 3] {
                for tx_shortcut in 0u8..=3 {
                    for ifs in 0u8..=4 {
                        for spatial_sse in 0u8..=3 {
                            for coeff_shave in 0u8..=2 {
                                for md_sq_mv in 0u8..=4 {
                                    for md_nsq_mv in 0u8..=2 {
                                        n = n.wrapping_add(1);
                                        let case = Case {
                                            enc_mode: m,
                                            is_islice: islice,
                                            update_type: ut,
                                            levels: Levels {
                                                tx_shortcut,
                                                ifs,
                                                wm: (n % 5) as u8,
                                                bipred3x3: (n % 5) as u8,
                                                inter_comp: (n % 5) as u8,
                                                ref_prune: (n % 9) as u8,
                                                spatial_sse,
                                                coeff_shave,
                                                obmc: (n % 7) as u8,
                                                inter_intra: (n % 3) as u8,
                                                md_sq_mv,
                                                md_nsq_mv,
                                                md_pme: (n % 6) as u8,
                                                me_subpel: (n % 11) as u8,
                                                pme_subpel: (n % 5) as u8,
                                            },
                                        };
                                        let ours = encdec::sig_deriv_enc_dec_default(
                                            to_port_inputs(&case),
                                        )
                                        .expect("all levels in range");
                                        let theirs = cref::sig_deriv_enc_dec_default(&build_input(
                                            &case,
                                            n.abs() % 97 + 1,
                                        ));
                                        assert_eq!(
                                            flatten(&ours),
                                            mask_unported(theirs),
                                            "enc_mode={m} islice={islice} update_type={ut} \
                                             tx_shortcut={tx_shortcut} ifs={ifs} \
                                             spatial_sse={spatial_sse} \
                                             coeff_shave={coeff_shave} md_sq_mv={md_sq_mv} \
                                             md_nsq_mv={md_nsq_mv} n={n}"
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
}

/// The subpel tables have the widest level domains (0..=10 for ME, 0..=4 for
/// PME) and the most partial-write arms, so they get an exhaustive pass of
/// their own rather than relying on the modular cycling above.
#[test]
fn subpel_tables_match_c_at_every_level() {
    for me_subpel in 0u8..=10 {
        for pme_subpel in 0u8..=4 {
            let case = Case {
                enc_mode: 5,
                is_islice: false,
                update_type: 1,
                levels: Levels {
                    tx_shortcut: 1,
                    ifs: 1,
                    wm: 1,
                    bipred3x3: 1,
                    inter_comp: 1,
                    ref_prune: 1,
                    spatial_sse: 1,
                    coeff_shave: 1,
                    obmc: 1,
                    inter_intra: 1,
                    md_sq_mv: 1,
                    md_nsq_mv: 1,
                    md_pme: 1,
                    me_subpel,
                    pme_subpel,
                },
            };
            let ours = encdec::sig_deriv_enc_dec_default(to_port_inputs(&case)).expect("in range");
            let theirs = cref::sig_deriv_enc_dec_default(&build_input(&case, 7));
            assert_eq!(
                flatten(&ours),
                mask_unported(theirs),
                "me_subpel={me_subpel} pme_subpel={pme_subpel}"
            );
        }
    }
}

/// Positive controls: the sweep must not be comparing two all-zero dumps, and
/// the fields that separate the video arm from the ported allintra twin must
/// hold the video values.
#[test]
fn enc_dec_default_positive_controls() {
    let case = Case {
        enc_mode: 5,
        is_islice: false,
        update_type: 1,
        levels: Levels {
            tx_shortcut: 3,
            ifs: 4,
            wm: 1,
            bipred3x3: 4,
            inter_comp: 4,
            ref_prune: 8,
            spatial_sse: 3,
            coeff_shave: 1,
            obmc: 6,
            inter_intra: 2,
            md_sq_mv: 1,
            md_nsq_mv: 1,
            md_pme: 5,
            me_subpel: 6,
            pme_subpel: 4,
        },
    };
    let c = cref::sig_deriv_enc_dec_default(&build_input(&case, 3));
    // mds0_use_hadamard_sb is FALSE on the video arm (the allintra twin sets it).
    assert_eq!(c[O_MDS0_HADAMARD], 0, "video arm shuts Hadamard at MDS0");
    // parent_cost_bias is 995 here.
    assert_eq!(c[O_PARENT_COST_BIAS], 995);
    // subres is forced to level 0 and pf to level 1 (DEFAULT_SHAPE == 0).
    assert_eq!((c[O_SUBRES_STEP], c[O_SUBRES_DEV]), (0, 0));
    assert_eq!(c[O_PF_SHAPE], 0);
    // The enc_mode-derived levels: M5 > M1 so skip_sub_depth is level 2
    // (coeff_perc 25), and M5 <= M6 so depth_early_exit is level 1
    // (split 50, exit 0).
    assert_eq!((c[O_DEE], c[O_DEE + 1]), (50, 0));
    assert_eq!(c[O_SSD + 3], 25, "M5 takes skip_sub_depth level 2");
    // Non-trivial table output reached.
    assert_eq!(c[O_OBMC + 1], 16, "obmc level 6 refines up to 16x16");
    assert_eq!(c[O_SPME + 12], 110, "me_subpel level 6 biases fp by 110");
}

/// The two derived levels are the ONLY thing `enc_mode` changes on this arm,
/// and their preset boundaries differ from the allintra twin: M6 for the
/// depth-early-exit level and M1 for the sub-depth skip level.
#[test]
fn enc_mode_derived_levels_boundaries() {
    for m in -1i8..=13 {
        let d = encdec::enc_dec_default_derived_levels(m);
        assert_eq!(
            d.depth_early_exit_lvl,
            if m <= 6 { 1 } else { 2 },
            "depth_early_exit boundary at M6, enc_mode={m}"
        );
        assert_eq!(
            d.skip_sub_depth_lvl,
            if m <= 1 { 1 } else { 2 },
            "skip_sub_depth boundary at M1, enc_mode={m}"
        );
    }
}

/// The two ctx flags derived from tables this lane did not port, checked
/// against their C predicates directly.
#[test]
fn derived_flags_match_their_c_predicates() {
    use encdec::chroma_mode;
    assert!(encdec::blk_skip_decision(chroma_mode::FULL));
    assert!(encdec::blk_skip_decision(chroma_mode::FAST));
    assert!(!encdec::blk_skip_decision(chroma_mode::BLIND));
    assert!(encdec::redundant_blk(true));
    assert!(!encdec::redundant_blk(false));
}

/// Out-of-range levels: C `assert(0)`s; the port refuses instead of inventing
/// a plausible control set.
#[test]
fn out_of_range_levels_are_refused() {
    let mut base = to_port_inputs(&Case {
        enc_mode: 5,
        is_islice: false,
        update_type: 1,
        levels: Levels {
            tx_shortcut: 0,
            ifs: 0,
            wm: 0,
            bipred3x3: 0,
            inter_comp: 0,
            ref_prune: 0,
            spatial_sse: 0,
            coeff_shave: 0,
            obmc: 0,
            inter_intra: 0,
            md_sq_mv: 0,
            md_nsq_mv: 0,
            md_pme: 0,
            me_subpel: 0,
            pme_subpel: 0,
        },
    });
    assert!(encdec::sig_deriv_enc_dec_default(base).is_some());
    base.me_subpel_level = 11;
    assert!(encdec::sig_deriv_enc_dec_default(base).is_none());
    base.me_subpel_level = 0;
    base.pme_subpel_level = 5;
    assert!(encdec::sig_deriv_enc_dec_default(base).is_none());
    base.pme_subpel_level = 0;
    base.tx_shortcut_level = 4;
    assert!(encdec::sig_deriv_enc_dec_default(base).is_none());
}
