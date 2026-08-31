//! Differential parity for `svt_aom_sig_deriv_multi_processes_default`
//! (`Source/Lib/Codec/enc_mode_config.c:1973`) — the picture-level tool
//! derivation for EVERY video-mode picture, the key frame included.
//!
//! **Evidence tier 1** (`docs/WORKING-ON-THIS.md` §4): the entry point is
//! EXPORTED and the shim drives the real symbol on a synthetic SCS/PPCS.
//!
//! This is the queue's highest-value entry for chunk C1a
//! (`docs/INTER-ENCODE-PLAN.md`): without it a `SVT_AVIF=0` key frame runs on
//! the allintra tool set and diverges before the first tile byte.
//!
//! NOT compared, because the tables behind them are not ported:
//! `pcs->intrabc_ctrls` beyond `enabled` (which is what `frm_hdr->allow_intrabc`
//! reads), `pcs->palette_ctrls`, `pcs->cdef_search_ctrls` and
//! `cm->wn_filter_ctrls`. The LEVELS feeding all four ARE compared — the
//! palette level and the CDEF search level directly (C stores both on the
//! PCS), and the Wiener level through `enable_restoration`, which is
//! `wn > 0 || sg > 0` and so pins `wn` wherever `sg` is 0.

use svtav1_cref::sig_deriv as cref;
use svtav1_cref::sig_deriv::{mp_in, mp_out};
use svtav1_encoder::port_enc_mode_config::ResolutionRange;
use svtav1_encoder::port_enc_mode_config::multi_processes as mp;
use svtav1_encoder::port_enc_mode_config::multi_processes::MultiProcessesInputs;

#[derive(Clone, Copy)]
struct Case {
    enc_mode: i8,
    is_islice: bool,
    temporal_layer: u8,
    input_res: ResolutionRange,
    fast_decode: u8,
    sc_class5: u8,
    is_highest_layer: bool,
    tf_hme_level: u8,
    enable_intrabc: bool,
    seq_cdef_level: u8,
    cfg_cdef_level: i32,
    seq_enable_restoration: bool,
    init_luma_w: u16,
    init_luma_h: u16,
    encoder_bit_depth: u32,
    cfg_hbd_mds: i32,
}

impl Default for Case {
    fn default() -> Self {
        Self {
            enc_mode: 5,
            is_islice: false,
            temporal_layer: 0,
            input_res: ResolutionRange::R1080p,
            fast_decode: 0,
            sc_class5: 0,
            is_highest_layer: false,
            tf_hme_level: 0,
            enable_intrabc: true,
            seq_cdef_level: 1,
            cfg_cdef_level: -1,
            seq_enable_restoration: true,
            init_luma_w: 1920,
            init_luma_h: 1080,
            encoder_bit_depth: 8,
            cfg_hbd_mds: -1,
        }
    }
}

fn build_input(c: &Case) -> [i32; mp_in::COUNT] {
    let mut i = [0i32; mp_in::COUNT];
    i[mp_in::ENC_MODE] = i32::from(c.enc_mode);
    i[mp_in::IS_ISLICE] = i32::from(c.is_islice);
    i[mp_in::TEMPORAL_LAYER] = i32::from(c.temporal_layer);
    i[mp_in::INPUT_RES] = i32::from(c.input_res.as_u8());
    i[mp_in::FAST_DECODE] = i32::from(c.fast_decode);
    i[mp_in::SC_CLASS5] = i32::from(c.sc_class5);
    i[mp_in::IS_HIGHEST_LAYER] = i32::from(c.is_highest_layer);
    i[mp_in::TF_HME_LEVEL] = i32::from(c.tf_hme_level);
    i[mp_in::ENABLE_INTRABC] = i32::from(c.enable_intrabc);
    i[mp_in::SEQ_CDEF_LEVEL] = i32::from(c.seq_cdef_level);
    i[mp_in::CFG_CDEF_LEVEL] = c.cfg_cdef_level;
    i[mp_in::SEQ_ENABLE_RESTORATION] = i32::from(c.seq_enable_restoration);
    i[mp_in::INIT_LUMA_W] = i32::from(c.init_luma_w);
    i[mp_in::INIT_LUMA_H] = i32::from(c.init_luma_h);
    i[mp_in::ENCODER_BIT_DEPTH] = c.encoder_bit_depth as i32;
    i[mp_in::CFG_HBD_MDS] = c.cfg_hbd_mds;
    i
}

fn to_port(c: &Case) -> MultiProcessesInputs {
    MultiProcessesInputs {
        enc_mode: c.enc_mode,
        is_islice: c.is_islice,
        is_base: c.temporal_layer == 0,
        input_resolution: c.input_res,
        fast_decode: c.fast_decode,
        sc_class5: c.sc_class5,
        is_not_last_layer: !c.is_highest_layer,
        tf_hme_me_level: c.tf_hme_level,
        enable_intrabc: c.enable_intrabc,
        seq_cdef_level: c.seq_cdef_level,
        config_cdef_level: c.cfg_cdef_level,
        seq_enable_restoration: c.seq_enable_restoration,
        max_initial_luma_width: c.init_luma_w,
        max_initial_luma_height: c.init_luma_h,
        encoder_bit_depth: c.encoder_bit_depth,
        config_hbd_mds: c.cfg_hbd_mds,
        gm_super_res_off: true,
    }
}

fn assert_case(c: &Case, msg: &str) {
    let o = mp::sig_deriv_multi_processes_default(to_port(c)).expect("levels in range");
    let t = cref::sig_deriv_multi_processes_default(&build_input(c));

    let g = &o.gm;
    assert_eq!(
        [
            i64::from(g.enabled),
            i64::from(g.identiy_exit),
            i64::from(g.search_start_model),
            i64::from(g.search_end_model),
            i64::from(g.skip_identity),
            i64::from(g.bypass_based_on_me),
            i64::from(g.params_refinement_steps),
            i64::from(g.downsample_level),
            i64::from(g.corners),
            i64::from(g.chess_rfn),
            i64::from(g.match_sz),
            i64::from(g.inj_psq_glb),
            i64::from(g.pp_enabled),
            i64::from(g.ref_idx0_only),
            i64::from(g.rfn_early_exit),
            i64::from(g.correspondence_method),
        ],
        t[mp_out::GM..mp_out::GM + 16],
        "gm_ctrls {msg}"
    );
    assert_eq!(
        [
            i64::from(o.enable_hme_flag),
            i64::from(o.enable_hme_level0_flag),
            i64::from(o.enable_hme_level1_flag),
            i64::from(o.enable_hme_level2_flag),
            i64::from(o.tf_enable_hme_flag),
            i64::from(o.tf_enable_hme_level0_flag),
            i64::from(o.tf_enable_hme_level1_flag),
            i64::from(o.tf_enable_hme_level2_flag),
        ],
        [
            t[mp_out::HME],
            t[mp_out::HME_L0],
            t[mp_out::HME_L1],
            t[mp_out::HME_L2],
            t[mp_out::TF_HME],
            t[mp_out::TF_HME_L0],
            t[mp_out::TF_HME_L1],
            t[mp_out::TF_HME_L2],
        ],
        "hme flags {msg}"
    );
    assert_eq!(
        [
            i64::from(o.multi_pass_pd_level),
            i64::from(o.allow_intrabc),
            i64::from(o.palette_level),
            i64::from(o.allow_screen_content_tools),
            i64::from(o.cdef_level),
        ],
        [
            t[mp_out::MULTI_PASS_PD],
            t[mp_out::ALLOW_INTRABC],
            t[mp_out::PALETTE_LEVEL],
            t[mp_out::ALLOW_SC_TOOLS],
            t[mp_out::CDEF_LEVEL],
        ],
        "tool levels (intrabc level {}) {msg}",
        o.intrabc_level
    );
    assert_eq!(
        [
            i64::from(o.cdef_recon.zero_fs_cost_bias),
            i64::from(o.cdef_recon.zero_filter_strength_lvl),
            i64::from(o.cdef_recon.prev_cdef_dist_th),
        ],
        t[mp_out::CDEF_RECON..mp_out::CDEF_RECON + 3],
        "cdef_recon_ctrls (level {}) {msg}",
        o.cdef_recon_level
    );
    let s = &o.sg_filter;
    assert_eq!(
        [
            i64::from(s.enabled),
            i64::from(s.start_ep[0]),
            i64::from(s.start_ep[1]),
            i64::from(s.end_ep[0]),
            i64::from(s.end_ep[1]),
            i64::from(s.ep_inc[0]),
            i64::from(s.ep_inc[1]),
            i64::from(s.refine[0]),
            i64::from(s.refine[1]),
            i64::from(s.use_chroma),
        ],
        t[mp_out::SG..mp_out::SG + 10],
        "sg_filter_ctrls (level {}) {msg}",
        o.sg_filter_level
    );
    assert_eq!(
        [
            i64::from(o.enable_restoration),
            i64::from(o.frame_end_cdf_update_mode),
            i64::from(o.hbd_md),
            i64::from(o.max_can_count),
            i64::from(o.use_best_me_unipred_cand_only),
        ],
        [
            t[mp_out::ENABLE_RESTORATION],
            t[mp_out::FRAME_END_CDF],
            t[mp_out::HBD_MD],
            t[mp_out::MAX_CAN_COUNT],
            t[mp_out::BEST_UNIPRED],
        ],
        "picture scalars (wn level {}) {msg}",
        o.wn_filter_level
    );
}

#[test]
fn multi_processes_matches_c_over_the_preset_and_flag_product() {
    let enc_modes: [i8; 15] = [-1, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13];
    for &m in &enc_modes {
        for &islice in &[false, true] {
            for &tl in &[0u8, 1, 4] {
                for &sc5 in &[0u8, 1] {
                    for &highest in &[false, true] {
                        for &fd in &[0u8, 1, 2] {
                            for &res in &[
                                ResolutionRange::R240p,
                                ResolutionRange::R360p,
                                ResolutionRange::R1080p,
                                ResolutionRange::R8k,
                            ] {
                                let c = Case {
                                    enc_mode: m,
                                    is_islice: islice,
                                    temporal_layer: tl,
                                    sc_class5: sc5,
                                    is_highest_layer: highest,
                                    fast_decode: fd,
                                    input_res: res,
                                    ..Case::default()
                                };
                                assert_case(
                                    &c,
                                    &format!(
                                        "m={m} islice={islice} tl={tl} sc5={sc5} \
                                         highest={highest} fd={fd} res={res:?}"
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

/// The sequence/config gates: cdef on/off and overridden, restoration on/off,
/// intrabc on/off, and the initial-resolution bucket the restoration levels
/// read (which is the SEQUENCE's initial size, not the picture's).
#[test]
fn multi_processes_sequence_gates_match_c() {
    for &m in &[-1i8, 0, 3, 4, 8, 9, 13] {
        for &seq_cdef in &[0u8, 1] {
            for &cfg_cdef in &[-1i32, 0, 2, 6] {
                for &seq_lr in &[false, true] {
                    for &ibc in &[false, true] {
                        for &(w, h) in &[
                            (176u16, 144u16),
                            (640, 360),
                            (1920, 1080),
                            (3840, 2160),
                            // 8K: >= INPUT_SIZE_8K_TH turns both filters off.
                            (7680, 4320),
                            (65535, 1600),
                        ] {
                            for &sc5 in &[0u8, 1] {
                                let c = Case {
                                    enc_mode: m,
                                    seq_cdef_level: seq_cdef,
                                    cfg_cdef_level: cfg_cdef,
                                    seq_enable_restoration: seq_lr,
                                    enable_intrabc: ibc,
                                    init_luma_w: w,
                                    init_luma_h: h,
                                    sc_class5: sc5,
                                    is_islice: true,
                                    ..Case::default()
                                };
                                assert_case(
                                    &c,
                                    &format!(
                                        "m={m} seq_cdef={seq_cdef} cfg_cdef={cfg_cdef} \
                                         seq_lr={seq_lr} ibc={ibc} init={w}x{h} sc5={sc5}"
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

/// `hbd_md`, its bit-depth gate and its config override.
#[test]
fn multi_processes_hbd_md_matches_c() {
    for &m in &[-1i8, 0, 5, 6, 13] {
        for &bd in &[8u32, 10] {
            for &cfg in &[-1i32, 0, 1, 2] {
                for &islice in &[false, true] {
                    for &tl in &[0u8, 2] {
                        let c = Case {
                            enc_mode: m,
                            encoder_bit_depth: bd,
                            cfg_hbd_mds: cfg,
                            is_islice: islice,
                            temporal_layer: tl,
                            ..Case::default()
                        };
                        assert_case(
                            &c,
                            &format!("m={m} bd={bd} cfg={cfg} islice={islice} tl={tl}"),
                        );
                    }
                }
            }
        }
    }
}

/// The TF HME flag switch, over its whole legal domain, and the refusal above
/// it (C would `assert(0)`).
#[test]
fn multi_processes_tf_hme_levels_match_c() {
    for level in 0u8..=4 {
        let c = Case {
            tf_hme_level: level,
            ..Case::default()
        };
        assert_case(&c, &format!("tf_hme_level={level}"));
    }
    let bad = Case {
        tf_hme_level: 5,
        ..Case::default()
    };
    assert!(mp::sig_deriv_multi_processes_default(to_port(&bad)).is_none());
}

/// Positive controls, so the sweeps cannot pass on two constant dumps.
#[test]
fn multi_processes_positive_controls() {
    // Screen content on an I-slice at M3 must turn IntraBC and palette ON and
    // therefore allow_screen_content_tools; and CDEF search must then be
    // FORCED OFF, because allow_intrabc gates it.
    let sc = Case {
        enc_mode: 3,
        is_islice: true,
        sc_class5: 1,
        ..Case::default()
    };
    let t = cref::sig_deriv_multi_processes_default(&build_input(&sc));
    assert_eq!(
        t[mp_out::ALLOW_INTRABC],
        1,
        "M3 screen I-slice enables IntraBC"
    );
    assert_eq!(t[mp_out::PALETTE_LEVEL], 5, "and palette level 5");
    assert_eq!(t[mp_out::ALLOW_SC_TOOLS], 1);
    assert_eq!(
        t[mp_out::CDEF_LEVEL],
        0,
        "IntraBC forces the CDEF search off"
    );

    // Non-screen content: all three off, CDEF search derived from the preset.
    let ns = Case { sc_class5: 0, ..sc };
    let tn = cref::sig_deriv_multi_processes_default(&build_input(&ns));
    assert_eq!(tn[mp_out::ALLOW_INTRABC], 0);
    assert_eq!(tn[mp_out::PALETTE_LEVEL], 0);
    assert_eq!(tn[mp_out::ALLOW_SC_TOOLS], 0);
    assert_eq!(tn[mp_out::CDEF_LEVEL], 5, "M3 derives CDEF search level 5");

    // SGR IS live in VIDEO mode at presets 0..3 — the finding this lane
    // recorded. Pin it against C directly.
    let p3 = Case {
        enc_mode: 3,
        is_islice: false,
        sc_class5: 0,
        ..Case::default()
    };
    let p4 = Case { enc_mode: 4, ..p3 };
    assert_eq!(
        cref::sig_deriv_multi_processes_default(&build_input(&p3))[mp_out::SG],
        1,
        "SGR is enabled at video preset 3"
    );
    assert_eq!(
        cref::sig_deriv_multi_processes_default(&build_input(&p4))[mp_out::SG],
        0,
        "and disabled at preset 4"
    );
    // Restoration stays on at p4 through Wiener alone.
    assert_eq!(
        cref::sig_deriv_multi_processes_default(&build_input(&p4))[mp_out::ENABLE_RESTORATION],
        1,
        "Wiener keeps restoration on at preset 4"
    );
    // On the HIGHEST temporal layer Wiener is off, so with SGR off too the
    // whole of restoration is off — which is what pins the Wiener level here.
    let p4_leaf = Case {
        is_highest_layer: true,
        ..p4
    };
    assert_eq!(
        cref::sig_deriv_multi_processes_default(&build_input(&p4_leaf))[mp_out::ENABLE_RESTORATION],
        0,
        "Wiener is off on the highest layer"
    );

    // max_can_count is a real preset ladder, not a constant.
    let m0 = Case {
        enc_mode: 0,
        ..Case::default()
    };
    let m13 = Case {
        enc_mode: 13,
        ..Case::default()
    };
    assert_eq!(
        cref::sig_deriv_multi_processes_default(&build_input(&m0))[mp_out::MAX_CAN_COUNT],
        1225
    );
    assert_eq!(
        cref::sig_deriv_multi_processes_default(&build_input(&m13))[mp_out::MAX_CAN_COUNT],
        80
    );
}
