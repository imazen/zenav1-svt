//! Differential parity for
//! `svt_aom_sig_deriv_mode_decision_config_default`
//! (`Source/Lib/Codec/enc_mode_config.c:8900`) — the largest function in the
//! file, which assigns EVERY per-picture level the MD path consumes.
//!
//! **Evidence tier 1** (`docs/WORKING-ON-THIS.md` §4): the entry point is
//! EXPORTED and the shim drives the real symbol on a synthetic SCS/PCS/PPCS,
//! dumping 52 picture-level fields.
//!
//! ONE derived value is not compared: `dlf_level`, which feeds
//! `svt_aom_set_dlf_controls` — a table this lane has not ported. The shim
//! therefore sets `enable_dlf_flag = 0`, which is the arm that forces the level
//! to 0, so the deblocking path is held constant rather than left varying and
//! unchecked. `get_dlf_level_default` itself remains ported-but-untested
//! (tier 4, no vectors), and that is stated in the lane report.

use svtav1_cref::sig_deriv as cref;
use svtav1_cref::sig_deriv::{MD_OUT_SLOTS, md_in};
use svtav1_encoder::port_enc_mode_config::md_config as md;
use svtav1_encoder::port_enc_mode_config::md_config::MdConfigInputs;
use svtav1_encoder::port_enc_mode_config::{InputCoeffLvl, ResolutionRange};

// Output slot indices, mirroring the C shim's MD_O_* enum.
const O: [&str; 0] = [];
const MFMV_BIT: usize = 0;
const RDOQ: usize = 1;
const COEFF_SHAVE: usize = 2;
const RATE_EST: usize = 3;
const CDF_MV: usize = 4;
const CDF_SE: usize = 5;
const CDF_COEF: usize = 6;
const CDF_EN: usize = 7;
const FILTER_INTRA: usize = 8;
const ACCURATE_PART_CTX: usize = 9;
const ALLOW_HP_MV: usize = 10;
const WM_LEVEL: usize = 11;
const ALLOW_WM: usize = 12;
const MOTION_MODE_SWITCHABLE: usize = 13;
const OBMC_LEVEL: usize = 14;
const APPROX_RATE: usize = 15;
const SKIP_INTRA: usize = 16;
const INTRA_LEVEL: usize = 17;
const DIST_ANG_INTRA: usize = 18;
const CAND_RED: usize = 19;
const TXT: usize = 20;
const TX_SHORTCUT: usize = 21;
const PD0_BIAS_WEIGHT: usize = 22;
const IFS_LEVEL: usize = 23;
const INTERP_FILTER: usize = 24;
const CHROMA: usize = 25;
const CFL: usize = 26;
const NN_COMB: usize = 27;
const UNI3X3: usize = 28;
const BIPRED3X3: usize = 29;
const INTER_COMP: usize = 30;
const REF_PRUNE: usize = 31;
const SPATIAL_SSE: usize = 32;
const NSQ_GEOM: usize = 33;
const NSQ_SEARCH: usize = 34;
const INTER_INTRA: usize = 35;
const TXS: usize = 36;
const TX_MODE: usize = 37;
const NIC: usize = 38;
const MD_SQ_MV: usize = 39;
const MD_NSQ_MV: usize = 40;
const MD_PME: usize = 41;
const ME_SUBPEL: usize = 42;
const PME_SUBPEL: usize = 43;
const MDS0: usize = 44;
const DISALLOW_4X4: usize = 45;
const BYPASS_ENCDEC: usize = 46;
const PD0_LVL: usize = 47;
const DEPTH_REMOVAL: usize = 48;
const DEPTH_REFINE: usize = 49;
const LPD1_LVL: usize = 50;
const LAMBDA_WEIGHT: usize = 51;

#[derive(Clone, Copy)]
struct Case {
    enc_mode: i8,
    is_ref: bool,
    temporal_layer: u8,
    input_res: ResolutionRange,
    is_islice: bool,
    sc_class5: u8,
    fast_decode: u8,
    hier_levels: u32,
    transition: bool,
    is_highest_layer: bool,
    sq_qp: u32,
    mfmv_enabled: u8,
    error_resilient: bool,
    base_q: i32,
    ref_hp_perc: i16,
    scs_input_res: ResolutionRange,
    frame_is_intra: bool,
    superres: bool,
    resize_enabled: bool,
    seq_qp_mod: u8,
    resize_mode: u8,
    ref_intra_perc: u8,
    rc_stat_gen: u8,
    ref_skip_perc: u8,
    coeff_lvl: InputCoeffLvl,
    ref_l0_try: u32,
    ref_l1_try: u32,
    enable_ii: bool,
    bit_depth: u8,
    segmentation: bool,
    sb_size: u16,
    hbd_md: u8,
    r0_gen: bool,
    r0_milli: i32,
    pcs_temporal_layer: u8,
    tune: u8,
    picture_qp: u32,
    ext_crf_offset: u8,
}

impl Default for Case {
    fn default() -> Self {
        Self {
            enc_mode: 5,
            is_ref: true,
            temporal_layer: 0,
            input_res: ResolutionRange::R1080p,
            is_islice: false,
            sc_class5: 0,
            fast_decode: 0,
            hier_levels: 4,
            transition: false,
            is_highest_layer: false,
            sq_qp: 35,
            mfmv_enabled: 1,
            error_resilient: false,
            base_q: 150,
            ref_hp_perc: 20,
            scs_input_res: ResolutionRange::R1080p,
            frame_is_intra: false,
            superres: false,
            resize_enabled: false,
            seq_qp_mod: 0,
            resize_mode: 0,
            ref_intra_perc: 20,
            rc_stat_gen: 0,
            ref_skip_perc: 20,
            coeff_lvl: InputCoeffLvl::Normal,
            ref_l0_try: 2,
            ref_l1_try: 2,
            enable_ii: true,
            bit_depth: 8,
            segmentation: false,
            sb_size: 64,
            hbd_md: 0,
            r0_gen: false,
            r0_milli: 500,
            pcs_temporal_layer: 0,
            tune: 0,
            picture_qp: 35,
            ext_crf_offset: 0,
        }
    }
}

fn build_input(c: &Case) -> [i32; md_in::COUNT] {
    let _ = O;
    let mut i = [0i32; md_in::COUNT];
    i[md_in::ENC_MODE] = i32::from(c.enc_mode);
    i[md_in::IS_REF] = i32::from(c.is_ref);
    i[md_in::TEMPORAL_LAYER] = i32::from(c.temporal_layer);
    i[md_in::INPUT_RES] = i32::from(c.input_res.as_u8());
    i[md_in::IS_ISLICE] = i32::from(c.is_islice);
    i[md_in::SC_CLASS5] = i32::from(c.sc_class5);
    i[md_in::FAST_DECODE] = i32::from(c.fast_decode);
    i[md_in::HIER_LEVELS] = c.hier_levels as i32;
    i[md_in::TRANSITION] = i32::from(c.transition);
    i[md_in::IS_HIGHEST_LAYER] = i32::from(c.is_highest_layer);
    i[md_in::SQ_QP] = c.sq_qp as i32;
    i[md_in::MFMV_ENABLED] = i32::from(c.mfmv_enabled);
    i[md_in::ERROR_RESILIENT] = i32::from(c.error_resilient);
    i[md_in::BASE_Q] = c.base_q;
    i[md_in::REF_HP_PERC] = i32::from(c.ref_hp_perc);
    i[md_in::SCS_INPUT_RES] = i32::from(c.scs_input_res.as_u8());
    i[md_in::FRAME_IS_INTRA] = i32::from(c.frame_is_intra);
    i[md_in::SUPERRES] = i32::from(c.superres);
    i[md_in::RESIZE_ENABLED] = i32::from(c.resize_enabled);
    i[md_in::SEQ_QP_MOD] = i32::from(c.seq_qp_mod);
    i[md_in::RESIZE_MODE] = i32::from(c.resize_mode);
    i[md_in::REF_INTRA_PERC] = i32::from(c.ref_intra_perc);
    i[md_in::RC_STAT_GEN] = i32::from(c.rc_stat_gen);
    i[md_in::REF_SKIP_PERC] = i32::from(c.ref_skip_perc);
    i[md_in::COEFF_LVL] = c.coeff_lvl as i32;
    i[md_in::REF_L0_TRY] = c.ref_l0_try as i32;
    i[md_in::REF_L1_TRY] = c.ref_l1_try as i32;
    i[md_in::ENABLE_II] = i32::from(c.enable_ii);
    i[md_in::BIT_DEPTH] = i32::from(c.bit_depth);
    i[md_in::SEGMENTATION] = i32::from(c.segmentation);
    i[md_in::SB_SIZE] = i32::from(c.sb_size);
    i[md_in::HBD_MD] = i32::from(c.hbd_md);
    i[md_in::R0_GEN] = i32::from(c.r0_gen);
    i[md_in::R0_MILLI] = c.r0_milli;
    i[md_in::PCS_TEMPORAL_LAYER] = i32::from(c.pcs_temporal_layer);
    i[md_in::TUNE] = i32::from(c.tune);
    i[md_in::PICTURE_QP] = c.picture_qp as i32;
    i[md_in::EXT_CRF_OFFSET] = i32::from(c.ext_crf_offset);
    i
}

fn to_port(c: &Case) -> MdConfigInputs {
    MdConfigInputs {
        enc_mode: c.enc_mode,
        is_ref: c.is_ref,
        temporal_layer_index: c.temporal_layer,
        input_resolution: c.input_res,
        is_islice: c.is_islice,
        sc_class5: c.sc_class5,
        fast_decode: c.fast_decode,
        hierarchical_levels: c.hier_levels,
        transition_present: c.transition,
        is_not_last_layer: !c.is_highest_layer,
        sq_qp: c.sq_qp,
        mfmv_enabled: c.mfmv_enabled,
        error_resilient_mode: c.error_resilient,
        base_q_idx: c.base_q,
        ref_hp_percentage: c.ref_hp_perc,
        scs_input_resolution: c.scs_input_res,
        frame_is_intra: c.frame_is_intra,
        frame_superres_enabled: c.superres,
        frame_resize_enabled: c.resize_enabled,
        seq_qp_mod: c.seq_qp_mod,
        resize_mode: c.resize_mode,
        ref_intra_percentage: c.ref_intra_perc,
        rc_stat_gen_pass_mode: c.rc_stat_gen,
        ref_skip_percentage: c.ref_skip_perc,
        coeff_lvl: c.coeff_lvl,
        ref_list0_count_try: c.ref_l0_try,
        ref_list1_count_try: c.ref_l1_try,
        enable_interintra_compound: c.enable_ii,
        encoder_bit_depth: c.bit_depth,
        segmentation_enabled: c.segmentation,
        super_block_size: c.sb_size,
        hbd_md: c.hbd_md,
        r0_gen: c.r0_gen,
        r0: f64::from(c.r0_milli) / 1000.0,
        pcs_temporal_layer_index: c.pcs_temporal_layer,
        tune: c.tune,
        picture_qp: c.picture_qp,
        extended_crf_qindex_offset: c.ext_crf_offset,
    }
}

fn flatten(s: &md::MdConfigSignals, mfmv_bit: i64) -> [i64; MD_OUT_SLOTS] {
    let mut o = [0i64; MD_OUT_SLOTS];
    o[MFMV_BIT] = mfmv_bit;
    o[RDOQ] = i64::from(s.rdoq_level);
    o[COEFF_SHAVE] = i64::from(s.coeff_shaving_level);
    o[RATE_EST] = i64::from(s.rate_est_level);
    o[CDF_MV] = i64::from(s.cdf_ctrl.update_mv);
    o[CDF_SE] = i64::from(s.cdf_ctrl.update_se);
    o[CDF_COEF] = i64::from(s.cdf_ctrl.update_coef);
    o[CDF_EN] = i64::from(s.cdf_ctrl.enabled);
    o[FILTER_INTRA] = i64::from(s.pic_filter_intra_level);
    o[ACCURATE_PART_CTX] = i64::from(s.use_accurate_part_ctx);
    o[ALLOW_HP_MV] = i64::from(s.allow_high_precision_mv);
    o[WM_LEVEL] = i64::from(s.wm_level);
    o[ALLOW_WM] = i64::from(s.allow_warped_motion);
    o[MOTION_MODE_SWITCHABLE] = i64::from(s.is_motion_mode_switchable);
    o[OBMC_LEVEL] = i64::from(s.pic_obmc_level);
    o[APPROX_RATE] = i64::from(s.approx_inter_rate);
    o[SKIP_INTRA] = i64::from(s.skip_intra);
    o[INTRA_LEVEL] = i64::from(s.intra_level);
    o[DIST_ANG_INTRA] = i64::from(s.dist_based_ang_intra_level);
    o[CAND_RED] = i64::from(s.cand_reduction_level);
    o[TXT] = i64::from(s.txt_level);
    o[TX_SHORTCUT] = i64::from(s.tx_shortcut_level);
    o[PD0_BIAS_WEIGHT] = i64::from(s.pd0_cost_bias_weight);
    o[IFS_LEVEL] = i64::from(s.interpolation_search_level);
    o[INTERP_FILTER] = i64::from(s.interpolation_filter);
    o[CHROMA] = i64::from(s.chroma_level);
    o[CFL] = i64::from(s.cfl_level);
    o[NN_COMB] = i64::from(s.new_nearest_near_comb_injection);
    o[UNI3X3] = i64::from(s.unipred3x3_injection);
    o[BIPRED3X3] = i64::from(s.bipred3x3_injection);
    o[INTER_COMP] = i64::from(s.inter_compound_mode);
    o[REF_PRUNE] = i64::from(s.dist_based_ref_pruning);
    o[SPATIAL_SSE] = i64::from(s.spatial_sse_full_loop_level);
    o[NSQ_GEOM] = i64::from(s.nsq_geom_level);
    o[NSQ_SEARCH] = i64::from(s.nsq_search_level);
    o[INTER_INTRA] = i64::from(s.inter_intra_level);
    o[TXS] = i64::from(s.txs_level);
    o[TX_MODE] = i64::from(s.tx_mode);
    o[NIC] = i64::from(s.nic_level);
    o[MD_SQ_MV] = i64::from(s.md_sq_mv_search_level);
    o[MD_NSQ_MV] = i64::from(s.md_nsq_mv_search_level);
    o[MD_PME] = i64::from(s.md_pme_level);
    o[ME_SUBPEL] = i64::from(s.me_subpel_level);
    o[PME_SUBPEL] = i64::from(s.pme_subpel_level);
    o[MDS0] = i64::from(s.mds0_level);
    o[DISALLOW_4X4] = i64::from(s.pic_disallow_4x4);
    o[BYPASS_ENCDEC] = i64::from(s.pic_bypass_encdec);
    o[PD0_LVL] = i64::from(s.pic_pd0_lvl);
    o[DEPTH_REMOVAL] = i64::from(s.pic_depth_removal_level);
    o[DEPTH_REFINE] = i64::from(s.pic_block_based_depth_refinement_level);
    o[LPD1_LVL] = i64::from(s.pic_lpd1_lvl);
    o[LAMBDA_WEIGHT] = i64::from(s.lambda_weight);
    o
}

fn assert_case(c: &Case, msg: &str) {
    let ours = md::sig_deriv_mode_decision_config_default(to_port(c)).expect("levels in range");
    let theirs = cref::sig_deriv_md_config_default(&build_input(c));
    // The mfmv frame-header BIT is produced by mfmv_controls (ported in
    // `tail`); the level this function derives is fed through it here.
    let mfmv_bit = i64::from(
        svtav1_encoder::port_enc_mode_config::tail::mfmv_controls(
            svtav1_encoder::port_enc_mode_config::tail::MfmvInputs {
                mfmv_level: ours.mfmv_level,
                is_base: c.temporal_layer == 0,
                // The shim leaves scs->tpl at 0.
                tpl: false,
                r0_gen: c.r0_gen,
                r0: f64::from(c.r0_milli) / 1000.0,
                is_b_slice: !c.is_islice,
                ref_list1_count_try: c.ref_l1_try,
                ref_l0_is_mfmv_used: false,
                ref_l1_is_mfmv_used: false,
            },
        )
        .expect("mfmv level in range"),
    );
    assert_eq!(flatten(&ours, mfmv_bit), theirs, "{msg}");
}

#[test]
fn md_config_matches_c_over_the_preset_and_layer_product() {
    let enc_modes: [i8; 15] = [-1, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13];
    for &m in &enc_modes {
        for &islice in &[false, true] {
            for &tl in &[0u8, 1, 3] {
                for &res in &[
                    ResolutionRange::R240p,
                    ResolutionRange::R360p,
                    ResolutionRange::R480p,
                    ResolutionRange::R720p,
                    ResolutionRange::R1080p,
                    ResolutionRange::R4k,
                ] {
                    for &coeff in &[
                        InputCoeffLvl::VLow,
                        InputCoeffLvl::Low,
                        InputCoeffLvl::Normal,
                        InputCoeffLvl::High,
                    ] {
                        for &sc5 in &[0u8, 1] {
                            for &transition in &[false, true] {
                                let c = Case {
                                    enc_mode: m,
                                    is_islice: islice,
                                    temporal_layer: tl,
                                    pcs_temporal_layer: tl,
                                    input_res: res,
                                    coeff_lvl: coeff,
                                    sc_class5: sc5,
                                    transition,
                                    frame_is_intra: islice,
                                    ..Case::default()
                                };
                                assert_case(
                                    &c,
                                    &format!(
                                        "m={m} islice={islice} tl={tl} res={res:?} \
                                         coeff={coeff:?} sc5={sc5} trans={transition}"
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

/// The QP-banded and flag-gated derivations: warped motion, TXS, OBMC, the
/// high-precision-MV frame-header bit and the lambda weight.
#[test]
fn md_config_qp_and_flag_gates_match_c() {
    for &m in &[-1i8, 1, 3, 6, 7, 9, 11, 13] {
        for &sqm in &[0u8, 1, 2, 3] {
            for &qp in &[0u32, 16, 35, 55, 56, 58, 59, 62, 63] {
                for &hier in &[0u32, 2, 3, 4, 5] {
                    for &(intra, err, sr, rz) in &[
                        (false, false, false, false),
                        (true, false, false, false),
                        (false, true, false, false),
                        (false, false, true, false),
                        (false, false, false, true),
                    ] {
                        for &tune in &[0u8, 3] {
                            for &ext in &[0u8, 7] {
                                let c = Case {
                                    enc_mode: m,
                                    seq_qp_mod: sqm,
                                    sq_qp: qp,
                                    picture_qp: qp,
                                    hier_levels: hier,
                                    frame_is_intra: intra,
                                    error_resilient: err,
                                    superres: sr,
                                    resize_enabled: rz,
                                    tune,
                                    ext_crf_offset: ext,
                                    ..Case::default()
                                };
                                assert_case(
                                    &c,
                                    &format!(
                                        "m={m} sqm={sqm} qp={qp} hier={hier} \
                                         intra={intra} err={err} sr={sr} rz={rz} \
                                         tune={tune} ext={ext}"
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

/// The reference-derived gates: skip_intra, the ref-pruning list counts, the
/// interpolation-search skip ladder and the high-precision-MV thresholds.
#[test]
fn md_config_reference_gates_match_c() {
    for &m in &[1i8, 2, 5, 9, 10, 13] {
        for &is_ref in &[false, true] {
            for &intra_perc in &[0u8, 50, 51, 100] {
                for &skip_perc in &[0u8, 29, 30, 31, 49, 50, 51, 84, 85, 86, 99, 100] {
                    for &(l0, l1) in &[(1u32, 1u32), (2, 1), (1, 2), (2, 2)] {
                        for &(bq, hp) in &[
                            (0i32, 0i16),
                            (127, 0),
                            (128, 0),
                            (150, 50),
                            (150, 51),
                            (195, 51),
                            (196, 51),
                        ] {
                            for &sres in &[ResolutionRange::R480p, ResolutionRange::R720p] {
                                // The interpolation-search skip threshold is
                                // indexed BY RESOLUTION (100/100/85/50/30/30/30),
                                // so the picture resolution has to vary here too
                                // or only one entry of that table is measured.
                                for &pres in &[
                                    ResolutionRange::R240p,
                                    ResolutionRange::R360p,
                                    ResolutionRange::R480p,
                                    ResolutionRange::R720p,
                                    ResolutionRange::R1080p,
                                    ResolutionRange::R4k,
                                    ResolutionRange::R8k,
                                ] {
                                    let c = Case {
                                        enc_mode: m,
                                        is_ref,
                                        ref_intra_perc: intra_perc,
                                        ref_skip_perc: skip_perc,
                                        ref_l0_try: l0,
                                        ref_l1_try: l1,
                                        base_q: bq,
                                        ref_hp_perc: hp,
                                        scs_input_res: sres,
                                        input_res: pres,
                                        temporal_layer: 2,
                                        pcs_temporal_layer: 2,
                                        ..Case::default()
                                    };
                                    assert_case(
                                        &c,
                                        &format!(
                                            "m={m} is_ref={is_ref} intra%={intra_perc} \
                                         skip%={skip_perc} refs=({l0},{l1}) bq={bq} \
                                         hp={hp} sres={sres:?} pres={pres:?}"
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

/// The r0 modulation of the depth-refinement level, and the LPD1 eligibility
/// gate (8-bit MD, 4x4 disallowed, 64x64 SB).
#[test]
fn md_config_r0_and_lpd1_gates_match_c() {
    for &m in &[1i8, 3, 6, 7, 9, 10, 11, 13] {
        for &r0_gen in &[false, true] {
            // The r0 thresholds are per-temporal-layer (0.20, 0.30, 0.40,
            // 0.50, ...) plus 0.05 on an I-slice, so the milli values must
            // straddle EACH of them — a single band leaves the others
            // unmeasured.
            for &r0_milli in &[
                0i32, 40, 49, 50, 51, 190, 199, 200, 201, 290, 299, 300, 301, 390, 399, 400, 401,
                490, 499, 500, 501, 990,
            ] {
                for &tl in &[0u8, 1, 2, 5] {
                    for &hbd in &[0u8, 1] {
                        for &sb in &[64u16, 128] {
                            for &islice in &[false, true] {
                                let c = Case {
                                    enc_mode: m,
                                    r0_gen,
                                    r0_milli,
                                    temporal_layer: tl,
                                    pcs_temporal_layer: tl,
                                    hbd_md: hbd,
                                    sb_size: sb,
                                    is_islice: islice,
                                    frame_is_intra: islice,
                                    ..Case::default()
                                };
                                assert_case(
                                    &c,
                                    &format!(
                                        "m={m} r0_gen={r0_gen} r0={r0_milli} tl={tl} \
                                         hbd={hbd} sb={sb} islice={islice}"
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

/// The sequence-level gates: mfmv, interintra, segmentation, bit depth,
/// rc_stat_gen and the resize mode.
#[test]
fn md_config_sequence_gates_match_c() {
    for &m in &[0i8, 3, 8, 11] {
        for &mfmv in &[0u8, 1] {
            for &err in &[false, true] {
                for &ii in &[false, true] {
                    for &seg in &[false, true] {
                        for &bd in &[8u8, 10] {
                            for &rcgen in &[0u8, 1] {
                                for &rm in &[0u8, 1] {
                                    for &fd in &[0u8, 1, 2] {
                                        let c = Case {
                                            enc_mode: m,
                                            mfmv_enabled: mfmv,
                                            error_resilient: err,
                                            enable_ii: ii,
                                            segmentation: seg,
                                            bit_depth: bd,
                                            rc_stat_gen: rcgen,
                                            resize_mode: rm,
                                            fast_decode: fd,
                                            ..Case::default()
                                        };
                                        assert_case(
                                            &c,
                                            &format!(
                                                "m={m} mfmv={mfmv} err={err} ii={ii} \
                                                 seg={seg} bd={bd} rcgen={rcgen} \
                                                 rm={rm} fd={fd}"
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
}

/// Positive controls: the sweeps must not be comparing two constant dumps, and
/// the fields that separate this arm from the ported allintra twin must hold
/// the VIDEO values.
#[test]
fn md_config_positive_controls() {
    let c = Case {
        enc_mode: 5,
        ..Case::default()
    };
    let t = cref::sig_deriv_md_config_default(&build_input(&c));
    // Inter-only tools that the allintra arm never turns on.
    assert_eq!(
        t[MD_NSQ_MV], 2,
        "md_nsq_mv_search_level is 2 on the video arm"
    );
    assert_eq!(t[MD_PME], 3, "M5 selects PME level 3");
    assert_eq!(t[ME_SUBPEL], 4, "M5 selects me_subpel level 4");
    assert_eq!(t[OBMC_LEVEL], 5, "M5 selects OBMC level 5");
    assert_eq!(t[WM_LEVEL], 4, "M5 at 1080p base selects warped level 4");
    assert_eq!(t[ALLOW_WM], 1, "and the frame header allows warped motion");
    assert_eq!(t[MOTION_MODE_SWITCHABLE], 1);
    // The tx_mode frame-header field follows txs_level.
    assert_eq!(
        (t[TXS], t[TX_MODE]),
        (3, 2),
        "M5 base: txs 3 -> TX_MODE_SELECT"
    );
    // A key frame kills warped motion outright.
    let ci = Case {
        frame_is_intra: true,
        is_islice: true,
        ..c
    };
    let ti = cref::sig_deriv_md_config_default(&build_input(&ci));
    assert_eq!((ti[WM_LEVEL], ti[ALLOW_WM]), (0, 0));
    // ...but OBMC survives, so the motion mode stays switchable.
    assert_eq!(ti[OBMC_LEVEL], 5);
    assert_eq!(ti[MOTION_MODE_SWITCHABLE], 1);
    // mfmv is off on an I-slice.
    assert_eq!(ti[MFMV_BIT], 0);
}

/// The warped-motion QP-banding WRAPS: at level 0 it becomes MAX_WARP_LVL
/// rather than staying off. Pin that against C, because it is the one place in
/// this function where a "reduce the level" step can INCREASE it.
#[test]
fn wm_qp_banding_wraps_zero_to_max() {
    // M9 at 4K non-base gives wm_level 0; seq_qp_mod 1 with qp > 55 then
    // wraps it to MAX_WARP_LVL (4).
    let c = Case {
        enc_mode: 7,
        temporal_layer: 3,
        pcs_temporal_layer: 3,
        input_res: ResolutionRange::R4k,
        seq_qp_mod: 1,
        sq_qp: 56,
        ..Case::default()
    };
    let t = cref::sig_deriv_md_config_default(&build_input(&c));
    assert_eq!(
        t[WM_LEVEL], 4,
        "level 0 wraps to MAX_WARP_LVL under QP banding"
    );
    // Without the banding it stays 0.
    let c0 = Case { seq_qp_mod: 0, ..c };
    assert_eq!(
        cref::sig_deriv_md_config_default(&build_input(&c0))[WM_LEVEL],
        0
    );
}

// ---------------------------------------------------------------------------
// The ALLINTRA arm's three RATE ladders — the still side of the fork
// `svtav1_encoder::rate_arm` dispatches on.
//
// Evidence tier 1 for a set that was previously tier 4:
// `quant::rdoq_level_allintra` and `FunnelCfg::for_preset`'s baked
// (coeff_rate_est_lvl, real_coeff_ctx) pair were hand-transcriptions with
// unit tests only. `svt_aom_sig_deriv_mode_decision_config_allintra` IS an
// exported symbol (`nm -g` shows it GLOBAL in both the aarch64 and x86-64
// archives), so the real C ladder is reachable and is what these drive.
// ---------------------------------------------------------------------------

use svtav1_cref::sig_deriv::md_allintra_out as ao;

/// `pcs->rdoq_level` on the allintra arm, over the whole (preset x coeff_lvl)
/// grid, against the real C function.
#[test]
fn allintra_rdoq_ladder_matches_c() {
    for enc_mode in -1i8..=13 {
        for lvl in [
            InputCoeffLvl::VLow,
            InputCoeffLvl::Low,
            InputCoeffLvl::Normal,
            InputCoeffLvl::High,
        ] {
            let c = Case {
                enc_mode,
                coeff_lvl: lvl,
                is_islice: true,
                frame_is_intra: true,
                ..Case::default()
            };
            let t = cref::sig_deriv_md_config_allintra(&build_input(&c));
            // The port's ladder takes a u8 enc_mode (SpeedConfig::preset);
            // ENC_MR is structurally unreachable there, so only 0..=13 are
            // compared against it — but C is still exercised at MR, which
            // pins that the `<= ENC_M5` arm covers it.
            if enc_mode >= 0 {
                let port = svtav1_encoder::quant::rdoq_level_allintra(
                    enc_mode as u8,
                    match lvl {
                        InputCoeffLvl::VLow => svtav1_encoder::quant::CoeffLvl::VLow,
                        InputCoeffLvl::Low => svtav1_encoder::quant::CoeffLvl::Low,
                        InputCoeffLvl::Normal => svtav1_encoder::quant::CoeffLvl::Normal,
                        InputCoeffLvl::High => svtav1_encoder::quant::CoeffLvl::High,
                    },
                );
                assert_eq!(
                    i64::from(port),
                    t[ao::RDOQ],
                    "allintra rdoq_level M{enc_mode} {lvl:?}"
                );
            } else {
                assert_eq!(t[ao::RDOQ], 1, "allintra rdoq_level at ENC_MR");
            }
        }
    }
}

/// `pcs->rate_est_level` on the allintra arm — the ladder whose
/// `set_rate_est_ctrls` row `FunnelCfg::for_preset` bakes.
#[test]
fn allintra_rate_est_ladder_matches_c() {
    for enc_mode in 0i8..=13 {
        let c = Case {
            enc_mode,
            is_islice: true,
            frame_is_intra: true,
            ..Case::default()
        };
        let t = cref::sig_deriv_md_config_allintra(&build_input(&c));
        let want = if enc_mode <= 6 {
            1
        } else if enc_mode <= 8 {
            4
        } else {
            0
        };
        assert_eq!(t[ao::RATE_EST], want, "allintra rate_est_level M{enc_mode}");
    }
}

/// `pcs->cdf_ctrl` on BOTH arms, for a KEY frame — the fork the per-SB CDF
/// chain gate reads. The point is `enabled`: the allintra arm switches CDF
/// adaptation off at M7/M8 and the video arm keeps it on, while at M4..M6 the
/// two arms carry DIFFERENT levels (2 vs 1) yet identical controls, because
/// `set_cdf_controls` forces `update_mv = 0` on an I_SLICE.
#[test]
fn cdf_ctrl_arms_diverge_at_m7_m8_and_coincide_below() {
    for enc_mode in 0i8..=13 {
        let c = Case {
            enc_mode,
            is_islice: true,
            frame_is_intra: true,
            temporal_layer: 0,
            pcs_temporal_layer: 0,
            ..Case::default()
        };
        let a = cref::sig_deriv_md_config_allintra(&build_input(&c));
        let d = cref::sig_deriv_md_config_default(&build_input(&c));
        // update_mv is forced 0 on an I-slice on both arms.
        assert_eq!(a[ao::CDF_MV], 0, "allintra update_mv M{enc_mode}");
        assert_eq!(d[CDF_MV], 0, "video update_mv M{enc_mode}");
        // The allintra arm's `enabled` is exactly `enc_mode <= M6`.
        assert_eq!(
            a[ao::CDF_EN],
            i64::from(enc_mode <= 6),
            "allintra cdf enabled M{enc_mode}"
        );
        // The video arm's is `enc_mode <= M8` for an I-slice.
        assert_eq!(
            d[CDF_EN],
            i64::from(enc_mode <= 8),
            "video cdf enabled M{enc_mode}"
        );
        // Below M7 the two arms produce the SAME controls despite different
        // levels — which is why the still path is byte-neutral at M4..M6.
        if enc_mode <= 6 {
            assert_eq!(
                (a[ao::CDF_MV], a[ao::CDF_SE], a[ao::CDF_COEF], a[ao::CDF_EN]),
                (d[CDF_MV], d[CDF_SE], d[CDF_COEF], d[CDF_EN]),
                "arms must agree on cdf_ctrl at M{enc_mode}"
            );
        }
    }
}
