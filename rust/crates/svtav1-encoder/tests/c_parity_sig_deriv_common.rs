//! Differential parity for `svt_aom_sig_deriv_enc_dec_common`
//! (`Source/Lib/Codec/enc_mode_config.c:7086`) and the largest table in the
//! file, `set_depth_removal_level_controls` (`:2965`).
//!
//! **Evidence tier 1** (`docs/WORKING-ON-THIS.md` §4): the entry point is
//! EXPORTED and the shim drives the real symbol on a synthetic SCS/PCS/ctx.
//!
//! This spine is called by ALL THREE arms, allintra included
//! (`product_coding_loop.c:10867` is downstream of it), so its absence was a
//! STILL-path gap that the current byte gates cannot see — the port hardcodes
//! the values it would produce.
//!
//! What is NOT compared, and why: the control structs written by tables this
//! lane has not ported — `depth_refinement_ctrls` beyond its `mode`,
//! `pd0_ctrls`, `lpd1_ctrls` beyond `pd1_level`, and `nsq_geom_ctrls`. The
//! LPD1 LEVEL that this function derives IS compared, through
//! `lpd1_ctrls.pd1_level`, which `set_lpd1_ctrls` stores verbatim.

use svtav1_cref::sig_deriv as cref;
use svtav1_cref::sig_deriv::{cm_in, cm_out};
use svtav1_encoder::port_enc_mode_config::common;
use svtav1_encoder::port_enc_mode_config::common::{CommonInputs, DepthRemovalInputs};

/// C `quantizer_to_qindex[64]` (`Codec/md_process.c:20`) — the port needs it to
/// reproduce the SB delta-qp modulation.
const QUANTIZER_TO_QINDEX: [u8; 64] = [
    0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44, 48, 52, 56, 60, 64, 68, 72, 76, 80, 84, 88, 92,
    96, 100, 104, 108, 112, 116, 120, 124, 128, 132, 136, 140, 144, 148, 152, 156, 160, 164, 168,
    172, 176, 180, 184, 188, 192, 196, 200, 204, 208, 212, 216, 220, 224, 228, 232, 236, 240, 244,
    249, 255,
];

#[derive(Clone, Copy)]
struct Case {
    enc_mode: i8,
    rtc: bool,
    allintra: bool,
    is_leaf: bool,
    is_base: bool,
    depth_refine_lvl: u8,
    b64_w: u16,
    b64_h: u16,
    pic_disallow_4x4: bool,
    pic_lpd1_lvl: i32,
    sb_qindex: i32,
    base_q: i32,
    is_islice: bool,
    me8_var: i32,
    qp_index: i32,
    max_tx_size: u32,
    dr_level: u8,
    lambda8: u32,
    delta_q_present: bool,
    r0_delta_qp: bool,
    picture_qp: i32,
    dist64: u32,
    dist32: u32,
    dist16: u32,
    dist8: u32,
    sb_geom_w: u16,
    sb_geom_h: u16,
    ref_avail: bool,
    ref_min_sq_size: u8,
    sb_variance: u16,
    cap_qp_scaling: bool,
    static_qp: u32,
}

impl Default for Case {
    fn default() -> Self {
        Self {
            enc_mode: 5,
            rtc: false,
            allintra: false,
            is_leaf: false,
            is_base: true,
            depth_refine_lvl: 1,
            b64_w: 64,
            b64_h: 64,
            pic_disallow_4x4: false,
            pic_lpd1_lvl: 0,
            sb_qindex: 128,
            base_q: 128,
            is_islice: false,
            me8_var: 1500,
            qp_index: 100,
            max_tx_size: 64,
            dr_level: 5,
            lambda8: 1000,
            delta_q_present: false,
            r0_delta_qp: false,
            picture_qp: 32,
            dist64: 400_000,
            dist32: 200_000,
            dist16: 100_000,
            dist8: 60_000,
            sb_geom_w: 64,
            sb_geom_h: 64,
            ref_avail: false,
            ref_min_sq_size: 0,
            sb_variance: 5000,
            cap_qp_scaling: false,
            static_qp: 35,
        }
    }
}

fn build_input(c: &Case) -> [i32; cm_in::COUNT] {
    let mut i = [0i32; cm_in::COUNT];
    i[cm_in::ENC_MODE] = i32::from(c.enc_mode);
    i[cm_in::RTC] = i32::from(c.rtc);
    i[cm_in::ALLINTRA] = i32::from(c.allintra);
    i[cm_in::UPDATE_TYPE] = if c.is_leaf { 1 } else { 0 };
    i[cm_in::IS_BASE] = i32::from(c.is_base);
    i[cm_in::DEPTH_REFINE_LVL] = i32::from(c.depth_refine_lvl);
    i[cm_in::B64_W] = i32::from(c.b64_w);
    i[cm_in::B64_H] = i32::from(c.b64_h);
    i[cm_in::PIC_DISALLOW_4X4] = i32::from(c.pic_disallow_4x4);
    i[cm_in::SB_SIZE] = 64;
    i[cm_in::PIC_LPD1_LVL] = c.pic_lpd1_lvl;
    i[cm_in::SB_QINDEX] = c.sb_qindex;
    i[cm_in::BASE_Q] = c.base_q;
    i[cm_in::IS_ISLICE] = i32::from(c.is_islice);
    i[cm_in::ME8_VAR] = c.me8_var;
    i[cm_in::QP_INDEX] = c.qp_index;
    i[cm_in::MAX_TX_SIZE] = c.max_tx_size as i32;
    i[cm_in::DR_LEVEL] = i32::from(c.dr_level);
    i[cm_in::LAMBDA8] = c.lambda8 as i32;
    i[cm_in::DELTA_Q_PRESENT] = i32::from(c.delta_q_present);
    i[cm_in::R0_DELTA_QP] = i32::from(c.r0_delta_qp);
    i[cm_in::PIC_QINDEX] = i32::from(QUANTIZER_TO_QINDEX[c.picture_qp as usize]);
    i[cm_in::PICTURE_QP] = c.picture_qp;
    i[cm_in::DIST64] = c.dist64 as i32;
    i[cm_in::DIST32] = c.dist32 as i32;
    i[cm_in::DIST16] = c.dist16 as i32;
    i[cm_in::DIST8] = c.dist8 as i32;
    i[cm_in::SB_GEOM_W] = i32::from(c.sb_geom_w);
    i[cm_in::SB_GEOM_H] = i32::from(c.sb_geom_h);
    i[cm_in::REF_AVAIL] = i32::from(c.ref_avail);
    i[cm_in::REF_MIN_SQ_SIZE] = i32::from(c.ref_min_sq_size);
    i[cm_in::SB_VARIANCE] = i32::from(c.sb_variance);
    i[cm_in::CAP_QP_SCALING] = i32::from(c.cap_qp_scaling);
    i[cm_in::STATIC_QP] = c.static_qp as i32;
    i
}

fn to_port(c: &Case) -> CommonInputs {
    CommonInputs {
        enc_mode: c.enc_mode,
        rtc_tune: c.rtc,
        allintra: c.allintra,
        is_not_last_layer: !c.is_leaf,
        is_base: c.is_base,
        pic_block_based_depth_refinement_level: c.depth_refine_lvl,
        b64_width: c.b64_w,
        b64_height: c.b64_h,
        pic_disallow_4x4: c.pic_disallow_4x4,
        super_block_size: 64,
        pic_lpd1_lvl: c.pic_lpd1_lvl,
        sb_qindex: c.sb_qindex,
        base_q_idx: c.base_q,
        is_islice: c.is_islice,
        me_8x8_cost_variance: c.me8_var,
        qp_index: c.qp_index,
        max_tx_size: c.max_tx_size,
        sb_geom_width: u32::from(c.sb_geom_w),
        sb_geom_height: u32::from(c.sb_geom_h),
        cap_max_size_qp_based_th_scaling: c.cap_qp_scaling,
        static_qp: c.static_qp,
        sb_variance: c.sb_variance,
        depth_removal: DepthRemovalInputs {
            level: c.dr_level,
            is_islice: c.is_islice,
            fast_lambda_8bit: c.lambda8,
            delta_q_present: c.delta_q_present,
            r0_delta_qp_md: c.r0_delta_qp,
            sb_qindex: c.sb_qindex,
            picture_qindex: i32::from(QUANTIZER_TO_QINDEX[c.picture_qp as usize]),
            picture_qp: c.picture_qp,
            dist_64: c.dist64,
            dist_32: c.dist32,
            dist_16: c.dist16,
            dist_8: c.dist8,
            me_8x8_cost_variance: c.me8_var as u32,
            sb_width: c.sb_geom_w,
            sb_height: c.sb_geom_h,
            // `disallow_4x4_in` is overwritten by the entry point.
            disallow_4x4_in: false,
            // C's reference arm runs only when !rtc AND the picture is not an
            // I-slice AND a same-size, same-POC reference exists.
            ref_sb_min_sq_size: if !c.rtc && !c.is_islice && c.ref_avail {
                Some(c.ref_min_sq_size)
            } else {
                None
            },
        },
    }
}

fn assert_case(c: &Case, msg: &str) {
    let ours = common::sig_deriv_enc_dec_common(to_port(c)).expect("levels in range");
    let theirs = cref::sig_deriv_enc_dec_common(&build_input(c));
    let got = [
        i64::from(ours.depth_refinement_mode),
        i64::from(ours.pred_depth_only),
        i64::from(ours.pred_depth_only),
        i64::from(ours.depth_removal.enabled),
        i64::from(ours.depth_removal.disallow_below_64x64),
        i64::from(ours.depth_removal.disallow_below_32x32),
        i64::from(ours.depth_removal.disallow_below_16x16),
        i64::from(ours.depth_removal.disallow_4x4),
        i64::from(ours.disallow_8x8),
        i64::from(ours.disallow_4x4),
        i64::from(ours.max_block_size),
        i64::from(ours.pd1_lvl_refinement),
        i64::from(ours.lpd1_pd1_level),
    ];
    let want = [
        theirs[cm_out::DEPTH_REFINE_MODE],
        theirs[cm_out::PRED_DEPTH_ONLY],
        theirs[cm_out::PIC_PRED_DEPTH_ONLY],
        theirs[cm_out::DR_ENABLED],
        theirs[cm_out::DR_B64],
        theirs[cm_out::DR_B32],
        theirs[cm_out::DR_B16],
        theirs[cm_out::DR_4X4],
        theirs[cm_out::DISALLOW_8X8],
        theirs[cm_out::DISALLOW_4X4],
        theirs[cm_out::MAX_BLOCK_SIZE],
        theirs[cm_out::PD1_LVL_REFINEMENT],
        theirs[cm_out::LPD1_PD1_LEVEL],
    ];
    assert_eq!(got, want, "{msg}");

    // The full lpd1_ctrls table: seven rows x nine fields.
    for (r, row) in ours.lpd1.rows.iter().enumerate() {
        let base = cm_out::LPD1_ROWS + r * 9;
        assert_eq!(
            [
                i64::from(row.use_lpd1_detector),
                i64::from(row.use_ref_info),
                i64::from(row.cost_th_dist),
                i64::from(row.cost_th_rate),
                i64::from(row.nz_coeff_th),
                i64::from(row.max_mv_length),
                i64::from(row.me_8x8_cost_variance_th),
                i64::from(row.skip_pd0_edge_dist_th),
                i64::from(row.skip_pd0_me_shift),
            ],
            [
                theirs[base],
                theirs[base + 1],
                theirs[base + 2],
                theirs[base + 3],
                theirs[base + 4],
                theirs[base + 5],
                theirs[base + 6],
                theirs[base + 7],
                theirs[base + 8],
            ],
            "lpd1_ctrls row {r} {msg}"
        );
    }
}

#[test]
fn common_matches_c_over_the_arm_and_flag_product() {
    let enc_modes: [i8; 7] = [-1, 0, 5, 8, 9, 10, 13];
    for &m in &enc_modes {
        for &rtc in &[false, true] {
            for &allintra in &[false, true] {
                for &is_leaf in &[false, true] {
                    for &is_base in &[false, true] {
                        for &is_islice in &[false, true] {
                            for &max_tx in &[32u32, 64] {
                                for &d4 in &[false, true] {
                                    let c = Case {
                                        enc_mode: m,
                                        rtc,
                                        allintra,
                                        is_leaf,
                                        is_base,
                                        is_islice,
                                        max_tx_size: max_tx,
                                        pic_disallow_4x4: d4,
                                        ..Case::default()
                                    };
                                    assert_case(
                                        &c,
                                        &format!(
                                            "m={m} rtc={rtc} allintra={allintra} leaf={is_leaf} \
                                             base={is_base} islice={is_islice} maxtx={max_tx} \
                                             d4={d4}"
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

/// Every depth-removal level, over the ME distortions and variance bands that
/// select its three disallow flags.
#[test]
fn depth_removal_levels_match_c() {
    for dr_level in 0u8..=15 {
        for &lambda in &[1u32, 100, 5000, 100_000] {
            for &(d64, d32, d16, d8) in &[
                (0u32, 0u32, 0u32, 0u32),
                (100, 100, 100, 100),
                (400_000, 200_000, 100_000, 60_000),
                (400_000, 399_000, 398_000, 397_000),
                (1_000_000, 10_000, 5_000, 1_000),
                (10_000, 1_000_000, 500_000, 250_000),
            ] {
                for &var in &[0i32, 10_000, 24_999, 25_000, 49_999, 50_000, 1_000_000] {
                    for &qp in &[0i32, 20, 32, 52, 63] {
                        let c = Case {
                            dr_level,
                            lambda8: lambda,
                            dist64: d64,
                            dist32: d32,
                            dist16: d16,
                            dist8: d8,
                            me8_var: var,
                            picture_qp: qp,
                            ..Case::default()
                        };
                        assert_case(
                            &c,
                            &format!(
                                "dr={dr_level} lambda={lambda} dists=({d64},{d32},{d16},{d8}) \
                                 var={var} qp={qp}"
                            ),
                        );
                    }
                }
            }
        }
    }
}

/// The SB-delta-qp modulation subtracts 1..4 from the level; sweep the four
/// bands plus both gating flags.
#[test]
fn depth_removal_delta_qp_modulation_matches_c() {
    for dr_level in 0u8..=15 {
        for &dq in &[false, true] {
            for &r0 in &[false, true] {
                for &diff in &[-40i32, -13, -12, -11, -7, -6, -5, -4, -3, -2, -1, 0, 1, 40] {
                    let picture_qp = 32;
                    let pic_qindex = i32::from(QUANTIZER_TO_QINDEX[picture_qp as usize]);
                    let c = Case {
                        dr_level,
                        delta_q_present: dq,
                        r0_delta_qp: r0,
                        picture_qp,
                        sb_qindex: pic_qindex + diff,
                        ..Case::default()
                    };
                    assert_case(&c, &format!("dr={dr_level} dq={dq} r0={r0} diff={diff}"));
                }
            }
        }
    }
}

/// The SB-geometry gates on the three disallow flags, and
/// `dimensions_require_8x8` behind the 16x16 one.
#[test]
fn depth_removal_sb_geometry_gates_match_c() {
    for w in [8u16, 16, 24, 32, 40, 48, 56, 64] {
        for h in [8u16, 16, 24, 32, 40, 48, 56, 64] {
            for &dr_level in &[5u8, 8, 12, 15] {
                let c = Case {
                    dr_level,
                    sb_geom_w: w,
                    sb_geom_h: h,
                    b64_w: w,
                    b64_h: h,
                    ..Case::default()
                };
                assert_case(&c, &format!("w={w} h={h} dr={dr_level}"));
            }
        }
    }
}

/// The reference-frame `sb_min_sq_size` adjustment, which bumps the two dev
/// thresholds. It is skipped entirely under rtc and on an I-slice.
#[test]
fn depth_removal_reference_adjustment_matches_c() {
    for &avail in &[false, true] {
        for &size in &[0u8, 8, 16, 31, 32, 63, 64, 128] {
            for &rtc in &[false, true] {
                for &islice in &[false, true] {
                    for &dr_level in &[6u8, 10, 15] {
                        // The adjustment moves dev_16x16_to_8x8_th by +20 (or
                        // dev_32x32_to_16x16_th by +5), and those thresholds
                        // are then shifted and qp-scaled — so the distortions
                        // must put the measured deviation NEAR the threshold or
                        // the +20 is unobservable. These d16/d8 and d32/d16
                        // ratios straddle it.
                        for &(d64, d32, d16, d8) in &[
                            (400_000u32, 200_000u32, 130_000u32, 100_000u32),
                            (400_000, 200_000, 133_000, 100_000),
                            (400_000, 200_000, 136_000, 100_000),
                            (400_000, 200_000, 140_000, 100_000),
                            (400_000, 130_000, 100_000, 90_000),
                            (400_000, 136_000, 100_000, 90_000),
                            (400_000, 200_000, 100_000, 60_000),
                            // Exact threshold landings, computed from the C:
                            // dev_16x16_to_8x8 = (d16-d8)*1000/d8, and the
                            // sb_min_sq_size >= 64 bump takes the dr-6 dev_16
                            // threshold from (50<<2)=200 to ((50+20)<<2)=280.
                            // A deviation of exactly 280 is BELOW 284 but not
                            // below 280, so it separates +20 from +21.
                            (400_000, 200_000, 128_000, 100_000), // dev_16 == 280
                            (400_000, 200_000, 126_000, 100_000), // dev_16 == 260 (the >=32 bump)
                            // dev_32x32_to_16x16 == 30 separates the +5 bump
                            // (threshold 30, not below) from +6 (31, below).
                            (400_000, 103_000, 100_000, 100_000),
                        ] {
                            let c = Case {
                                dr_level,
                                ref_avail: avail,
                                ref_min_sq_size: size,
                                rtc,
                                is_islice: islice,
                                dist64: d64,
                                dist32: d32,
                                dist16: d16,
                                dist8: d8,
                                ..Case::default()
                            };
                            assert_case(
                                &c,
                                &format!(
                                    "avail={avail} size={size} rtc={rtc} islice={islice} \
                                     dr={dr_level} dists=({d64},{d32},{d16},{d8})"
                                ),
                            );
                        }
                    }
                }
            }
        }
    }
}

/// The LPD1 level derivation: three arms, and the M8 boundary inside the third
/// where the threshold switches from `3 * qp_index` to a flat 3000.
#[test]
fn lpd1_level_derivation_matches_c() {
    for &m in &[-1i8, 5, 8, 9, 10, 11, 12, 13] {
        for pic_lpd1 in 0i32..=5 {
            for &rtc in &[false, true] {
                for &islice in &[false, true] {
                    for &is_base in &[false, true] {
                        for &qp_index in &[0i32, 100, 255] {
                            for &var in &[0i32, 299, 300, 3000, 3001, 100_000] {
                                for &(sbq, bq) in &[(100i32, 128i32), (128, 128), (200, 128)] {
                                    let c = Case {
                                        enc_mode: m,
                                        pic_lpd1_lvl: pic_lpd1,
                                        rtc,
                                        is_islice: islice,
                                        is_base,
                                        qp_index,
                                        me8_var: var,
                                        sb_qindex: sbq,
                                        base_q: bq,
                                        ..Case::default()
                                    };
                                    assert_case(
                                        &c,
                                        &format!(
                                            "m={m} lpd1={pic_lpd1} rtc={rtc} islice={islice} \
                                             base={is_base} qpidx={qp_index} var={var} \
                                             sbq={sbq} bq={bq}"
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

/// Positive controls, so the sweeps above cannot pass on two constant dumps.
#[test]
fn common_positive_controls() {
    // A level-15 depth removal with tiny 16x16/8x8 distortions must actually
    // disallow something.
    let c = Case {
        dr_level: 15,
        lambda8: 100_000,
        dist64: 10,
        dist32: 10,
        dist16: 10,
        dist8: 10,
        me8_var: 0,
        ..Case::default()
    };
    let t = cref::sig_deriv_enc_dec_common(&build_input(&c));
    assert_eq!(t[cm_out::DR_ENABLED], 1);
    assert_eq!(
        t[cm_out::DR_B16],
        1,
        "level 15 must disallow below 16x16 here"
    );
    assert_eq!(t[cm_out::DISALLOW_4X4], 1, "and set ctx->disallow_4x4");

    // An I-slice disables depth removal outright, whatever the level.
    let ci = Case {
        is_islice: true,
        ..c
    };
    let ti = cref::sig_deriv_enc_dec_common(&build_input(&ci));
    assert_eq!(ti[cm_out::DR_ENABLED], 0, "I-slice disables depth removal");

    // depth_refinement level 10 is the only one that yields PRED_PART_ONLY,
    // which is what sets pred_depth_only.
    for lvl in 0u8..=10 {
        let cl = Case {
            depth_refine_lvl: lvl,
            ..Case::default()
        };
        let tl = cref::sig_deriv_enc_dec_common(&build_input(&cl));
        assert_eq!(
            tl[cm_out::PRED_DEPTH_ONLY],
            i64::from(lvl == 10),
            "pred_depth_only at depth_refine level {lvl}"
        );
    }

    // max_tx_size 32 must clear disallow_below_64x64 even when the table set it.
    let c64 = Case {
        dr_level: 15,
        lambda8: 100_000,
        dist64: 10,
        dist32: 10,
        dist16: 10,
        dist8: 10,
        max_tx_size: 64,
        ..Case::default()
    };
    let c32 = Case {
        max_tx_size: 32,
        ..c64
    };
    let t64 = cref::sig_deriv_enc_dec_common(&build_input(&c64));
    let t32 = cref::sig_deriv_enc_dec_common(&build_input(&c32));
    assert_eq!(
        t64[cm_out::DR_B64],
        1,
        "level 15 disallows below 64x64 here"
    );
    assert_eq!(t32[cm_out::DR_B64], 0, "max_tx_size 32 clears it");
}

/// `ctx->max_block_size` takes a DIFFERENT function per arm, and the still
/// (allintra) and rtc arms both cap it to half the SB above a qp-scaled
/// variance threshold while the video arm never caps at all. Sweep all three.
#[test]
fn max_block_size_arms_match_c() {
    for &m in &[-1i8, 5, 7, 8, 9, 13] {
        for &variance in &[0u16, 7499, 7500, 7501, 30_000, u16::MAX] {
            for &me_var in &[0i32, 49_999, 50_000, 50_001, 1_000_000] {
                for &qp_scaling in &[false, true] {
                    for &qp in &[0u32, 20, 45, 46, 63] {
                        for &(w, h) in &[(64u16, 64u16), (32, 64), (64, 32), (8, 8)] {
                            for &(allintra, rtc) in &[(false, false), (true, false), (false, true)]
                            {
                                for &islice in &[false, true] {
                                    let c = Case {
                                        enc_mode: m,
                                        allintra,
                                        rtc,
                                        is_islice: islice,
                                        sb_variance: variance,
                                        me8_var: me_var,
                                        cap_qp_scaling: qp_scaling,
                                        static_qp: qp,
                                        sb_geom_w: w,
                                        sb_geom_h: h,
                                        b64_w: w,
                                        b64_h: h,
                                        ..Case::default()
                                    };
                                    assert_case(
                                        &c,
                                        &format!(
                                            "m={m} var={variance} me_var={me_var} \
                                             qp_scaling={qp_scaling} qp={qp} {w}x{h} \
                                             allintra={allintra} rtc={rtc} islice={islice}"
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

/// Positive control for the max-block-size sweep: at M13 with a high variance
/// the allintra and rtc arms must HALVE the SB while the video arm does not.
#[test]
fn max_block_size_positive_control() {
    let base = Case {
        enc_mode: 13,
        sb_variance: u16::MAX,
        me8_var: 1_000_000,
        sb_geom_w: 64,
        sb_geom_h: 64,
        b64_w: 64,
        b64_h: 64,
        is_islice: false,
        ..Case::default()
    };
    let video = cref::sig_deriv_enc_dec_common(&build_input(&base));
    assert_eq!(
        video[cm_out::MAX_BLOCK_SIZE],
        64,
        "the video arm never caps"
    );
    let ai = Case {
        allintra: true,
        ..base
    };
    assert_eq!(
        cref::sig_deriv_enc_dec_common(&build_input(&ai))[cm_out::MAX_BLOCK_SIZE],
        32,
        "the allintra arm halves on high pixel variance"
    );
    let rtc = Case { rtc: true, ..base };
    assert_eq!(
        cref::sig_deriv_enc_dec_common(&build_input(&rtc))[cm_out::MAX_BLOCK_SIZE],
        32,
        "the rtc arm halves on high ME variance"
    );
}

/// Every `lpd1_lvl` the derivation can produce, driven through
/// `set_lpd1_ctrls` from the exported entry point. This is the ~336-line table
/// in full — nine fields x seven rows x nine levels — and it is the widest
/// single table in the file after depth removal.
#[test]
fn lpd1_ctrls_table_matches_c_at_every_level() {
    for pic_lpd1 in 0i32..=8 {
        // enc_mode 11 takes the third LPD1 arm, where the picture level is
        // bumped by the ME-variance test; drive both the bumped and unbumped
        // paths so every derived level 0..=8 is reached.
        for &m in &[5i8, 11] {
            for &var in &[0i32, 100_000] {
                for &islice in &[false, true] {
                    let c = Case {
                        enc_mode: m,
                        pic_lpd1_lvl: pic_lpd1,
                        me8_var: var,
                        is_islice: islice,
                        qp_index: 100,
                        ..Case::default()
                    };
                    assert_case(
                        &c,
                        &format!("lpd1={pic_lpd1} m={m} var={var} islice={islice}"),
                    );
                }
            }
        }
    }
}

/// Positive control for the table sweep: a level must actually write the rows
/// up to its `pd1_level` and leave the rest zeroed.
#[test]
fn lpd1_ctrls_writes_only_the_rows_below_its_level() {
    let c = Case {
        enc_mode: 5,
        pic_lpd1_lvl: 5,
        ..Case::default()
    };
    let t = cref::sig_deriv_enc_dec_common(&build_input(&c));
    assert_eq!(t[cm_out::LPD1_PD1_LEVEL], 3, "lpd1_lvl 5 -> LPD1_LVL_3");
    // Rows 0..=3 are written (detector on), rows 4..6 are not.
    for r in 0..=3 {
        assert_eq!(t[cm_out::LPD1_ROWS + r * 9], 1, "row {r} detector on");
    }
    for r in 4..7 {
        assert_eq!(t[cm_out::LPD1_ROWS + r * 9], 0, "row {r} untouched");
    }
    // And a non-trivial value inside: row 0's cost_th_dist is 256 << 10.
    assert_eq!(t[cm_out::LPD1_ROWS + 2], 256 << 10);
}

/// The LPD1 level -> `pd1_level` mapping is NOT the identity; pin every entry
/// so a future edit to `set_lpd1_ctrls` shows up here rather than as a silent
/// off-by-one in the LPD1 path.
#[test]
fn lpd1_pd1_level_mapping_is_pinned() {
    let expect: [(u8, i8); 9] = [
        (0, -1), // REGULAR_PD1
        (1, 0),
        (2, 0),
        (3, 0),
        (4, 1),
        (5, 3), // LPD1_LVL_2 is skipped
        (6, 4),
        (7, 5),
        (8, 6),
    ];
    for &(lvl, want) in &expect {
        assert_eq!(common::lpd1_pd1_level(lvl), Some(want), "lpd1_lvl={lvl}");
    }
    assert!(common::lpd1_pd1_level(9).is_none());
}
