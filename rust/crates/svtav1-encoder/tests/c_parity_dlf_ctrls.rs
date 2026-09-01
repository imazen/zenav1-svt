//! Differential parity for the DEBLOCK signal derivation of
//! `Source/Lib/Codec/enc_mode_config.c`: the `get_dlf_level_*` ladders
//! (`:1466` default, `:1540` allintra), `dlf_level_modulation` (`:1442`) and
//! the `svt_aom_set_dlf_controls` table (`:1561`).
//!
//! **Evidence tier 1** (`docs/WORKING-ON-THIS.md` §4). All four C functions are
//! file-`static` — the `svt_aom_` prefix on `set_dlf_controls`
//! notwithstanding — so none can be called directly. But the EXPORTED
//! `svt_aom_sig_deriv_mode_decision_config_{default,allintra}` reach all four
//! and leave the result in `ppcs->dlf_ctrls`, which `shims/dlf_shims.c` reads
//! back. So this drives the REAL C ladder and the REAL C controls table.
//!
//! This supersedes the tier-4 claim in `tests/sig_deriv_dlf_traced.rs`: that
//! file's header said the levels "move to tier 1 when `svt_aom_set_dlf_controls`
//! is ported", which was half right. Porting the table is what makes the
//! composite OBSERVABLE (the level scalar is never stored in C), and the eight
//! control fields are distinct for each of the eight levels, so
//! `ctrls == set_dlf_controls(level)` pins the level too.
//!
//! The call-site rules are part of what is compared, because they are what the
//! encoder actually executes:
//!   * `if (scs->static_config.enable_dlf_flag && frm_hdr->allow_intrabc == 0)`
//!     — otherwise the level is 0.
//!   * `enable_dlf_flag == 2` re-derives at `AOMMAX(ENC_MR, enc_mode - 3)`.
//!   * the `default` arm's `is_base` argument is
//!     **`pcs->temporal_layer_index == 0`**, not the function's own `is_base`
//!     local (which is `ppcs->temporal_layer_index == 0`) — two different
//!     fields, and the shim carries both.

use svtav1_cref::dlf as cref;
use svtav1_cref::dlf::{dlf_in, dlf_out};
use svtav1_encoder::port_enc_mode_config::ctrls::{DlfCtrls, set_dlf_controls};
use svtav1_encoder::port_enc_mode_config::leaf;
use svtav1_encoder::port_enc_mode_config::{InputCoeffLvl, ResolutionRange};

/// C `ENC_MR`.
const MR: i8 = -1;

#[derive(Clone, Copy, Debug)]
struct Case {
    enc_mode: i8,
    fast_decode: u8,
    input_res: ResolutionRange,
    is_highest_layer: bool,
    /// `pcs->temporal_layer_index` — the `is_base` argument.
    pcs_temporal_layer: u8,
    coeff_lvl: InputCoeffLvl,
    ref_skip_perc: u8,
    enable_dlf_flag: u8,
    allow_intrabc: bool,
    is_islice: bool,
}

impl Default for Case {
    fn default() -> Self {
        Self {
            enc_mode: 5,
            fast_decode: 0,
            input_res: ResolutionRange::R1080p,
            is_highest_layer: false,
            pcs_temporal_layer: 0,
            coeff_lvl: InputCoeffLvl::Normal,
            ref_skip_perc: 20,
            enable_dlf_flag: 1,
            allow_intrabc: false,
            is_islice: false,
        }
    }
}

/// The non-dlf slots are held at the md-config differential's defaults; every
/// one of them is dereferenced by the surrounding
/// `svt_aom_sig_deriv_mode_decision_config_*` body, which runs in full.
fn build_input(c: &Case) -> [i32; dlf_in::COUNT] {
    let mut i = [0i32; dlf_in::COUNT];
    i[dlf_in::ENC_MODE] = i32::from(c.enc_mode);
    i[dlf_in::IS_REF] = 1;
    i[dlf_in::TEMPORAL_LAYER] = 0;
    i[dlf_in::INPUT_RES] = i32::from(c.input_res.as_u8());
    i[dlf_in::IS_ISLICE] = i32::from(c.is_islice);
    i[dlf_in::SC_CLASS5] = 0;
    i[dlf_in::FAST_DECODE] = i32::from(c.fast_decode);
    i[dlf_in::HIER_LEVELS] = 4;
    i[dlf_in::TRANSITION] = 0;
    i[dlf_in::IS_HIGHEST_LAYER] = i32::from(c.is_highest_layer);
    i[dlf_in::SQ_QP] = 35;
    i[dlf_in::MFMV_ENABLED] = 1;
    i[dlf_in::ERROR_RESILIENT] = 0;
    i[dlf_in::BASE_Q] = 150;
    i[dlf_in::REF_HP_PERC] = 20;
    i[dlf_in::SCS_INPUT_RES] = i32::from(c.input_res.as_u8());
    i[dlf_in::FRAME_IS_INTRA] = i32::from(c.is_islice);
    i[dlf_in::SUPERRES] = 0;
    i[dlf_in::RESIZE_ENABLED] = 0;
    i[dlf_in::SEQ_QP_MOD] = 0;
    i[dlf_in::RESIZE_MODE] = 0;
    i[dlf_in::REF_INTRA_PERC] = 20;
    i[dlf_in::RC_STAT_GEN] = 0;
    i[dlf_in::REF_SKIP_PERC] = i32::from(c.ref_skip_perc);
    i[dlf_in::COEFF_LVL] = c.coeff_lvl as i32;
    i[dlf_in::REF_L0_TRY] = 2;
    i[dlf_in::REF_L1_TRY] = 2;
    i[dlf_in::ENABLE_II] = 1;
    i[dlf_in::BIT_DEPTH] = 8;
    i[dlf_in::SEGMENTATION] = 0;
    i[dlf_in::SB_SIZE] = 64;
    i[dlf_in::HBD_MD] = 0;
    i[dlf_in::R0_GEN] = 0;
    i[dlf_in::R0_MILLI] = 500;
    i[dlf_in::PCS_TEMPORAL_LAYER] = i32::from(c.pcs_temporal_layer);
    i[dlf_in::TUNE] = 0;
    i[dlf_in::PICTURE_QP] = 35;
    i[dlf_in::EXT_CRF_OFFSET] = 0;
    i[dlf_in::ENABLE_DLF_FLAG] = i32::from(c.enable_dlf_flag);
    i[dlf_in::ALLOW_INTRABC] = i32::from(c.allow_intrabc);
    i
}

fn c_ctrls(out: &[i64; dlf_out::COUNT]) -> DlfCtrls {
    DlfCtrls {
        enabled: out[dlf_out::ENABLED] as u8,
        sb_based_dlf: out[dlf_out::SB_BASED] as u8,
        dlf_avg: out[dlf_out::AVG] != 0,
        use_ref_avg_y: out[dlf_out::USE_REF_AVG_Y] != 0,
        use_ref_avg_uv: out[dlf_out::USE_REF_AVG_UV] != 0,
        early_exit_convergence: out[dlf_out::EARLY_EXIT] as u8,
        zero_filter_strength_lvl: out[dlf_out::ZERO_FILT_STRENGTH] as u8,
        prev_dlf_dist_th: out[dlf_out::PREV_DIST_TH] as u16,
    }
}

/// C's call-site `dlf_enc_mode` adjustment (`enc_mode_config.c:9472-9477`,
/// identical at `:10120-10125`).
fn dlf_enc_mode(enable_dlf_flag: u8, enc_mode: i8) -> i8 {
    if enable_dlf_flag == 2 {
        MR.max(enc_mode - 3)
    } else {
        enc_mode
    }
}

fn port_level_default(c: &Case) -> u8 {
    if c.enable_dlf_flag == 0 || c.allow_intrabc {
        return 0;
    }
    leaf::get_dlf_level_default(
        dlf_enc_mode(c.enable_dlf_flag, c.enc_mode),
        u8::from(!c.is_highest_layer),
        c.fast_decode,
        c.input_res,
        c.pcs_temporal_layer == 0,
        c.coeff_lvl,
        c.ref_skip_perc,
    )
}

fn port_level_allintra(c: &Case) -> u8 {
    if c.enable_dlf_flag == 0 || c.allow_intrabc {
        return 0;
    }
    leaf::get_dlf_level_allintra(
        dlf_enc_mode(c.enable_dlf_flag, c.enc_mode),
        c.fast_decode,
        c.input_res,
    )
}

fn check(c: &Case, video: bool) {
    let inp = build_input(c);
    let out = if video {
        cref::dlf_ctrls_default(&inp)
    } else {
        cref::dlf_ctrls_allintra(&inp)
    };
    let level = if video {
        port_level_default(c)
    } else {
        port_level_allintra(c)
    };
    let port = set_dlf_controls(level)
        .unwrap_or_else(|| panic!("port produced out-of-range dlf level {level} for {c:?}"));
    assert_eq!(
        c_ctrls(&out),
        port,
        "dlf ctrls mismatch ({}) for {c:?} (port level {level})",
        if video {
            "default/video"
        } else {
            "allintra/still"
        }
    );
}

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

/// The whole VIDEO ladder: every preset x fast-decode arm x resolution x
/// layer position, on a BASE picture (where `dlf_level_modulation` does not
/// run).
#[test]
fn dlf_ctrls_default_ladder_matches_c() {
    for &m in &ENC_MODES {
        for &fd in &[0u8, 1, 2] {
            for &res in &RESOLUTIONS {
                for &highest in &[false, true] {
                    for &coeff in &[
                        InputCoeffLvl::VLow,
                        InputCoeffLvl::Low,
                        InputCoeffLvl::Normal,
                        InputCoeffLvl::High,
                    ] {
                        check(
                            &Case {
                                enc_mode: m,
                                fast_decode: fd,
                                input_res: res,
                                is_highest_layer: highest,
                                pcs_temporal_layer: 0,
                                coeff_lvl: coeff,
                                ..Case::default()
                            },
                            true,
                        );
                    }
                }
            }
        }
    }
}

/// `dlf_level_modulation` — reached only when `pcs->temporal_layer_index != 0`.
/// The `ref_skip_percentage` grid straddles every threshold in the C body
/// (25, 50, 75, 95).
#[test]
fn dlf_ctrls_default_modulation_matches_c() {
    for &m in &ENC_MODES {
        for &fd in &[0u8, 2] {
            for &skip in &[0u8, 24, 25, 26, 49, 50, 51, 74, 75, 76, 94, 95, 96, 100] {
                for &highest in &[false, true] {
                    for &coeff in &[InputCoeffLvl::Normal, InputCoeffLvl::High] {
                        check(
                            &Case {
                                enc_mode: m,
                                fast_decode: fd,
                                input_res: ResolutionRange::R1080p,
                                is_highest_layer: highest,
                                pcs_temporal_layer: 2,
                                coeff_lvl: coeff,
                                ref_skip_perc: skip,
                                ..Case::default()
                            },
                            true,
                        );
                    }
                }
            }
        }
    }
}

/// The STILL ladder — the arm the 280/280 still envelope rides on.
#[test]
fn dlf_ctrls_allintra_ladder_matches_c() {
    for &m in &ENC_MODES {
        for &fd in &[0u8, 1, 2] {
            for &res in &RESOLUTIONS {
                for &islice in &[false, true] {
                    check(
                        &Case {
                            enc_mode: m,
                            fast_decode: fd,
                            input_res: res,
                            is_islice: islice,
                            ..Case::default()
                        },
                        false,
                    );
                }
            }
        }
    }
}

/// The two call-site guards: `enable_dlf_flag` (0 = off, 2 = derive three
/// presets lower) and `frm_hdr->allow_intrabc`.
#[test]
fn dlf_ctrls_call_site_guards_match_c() {
    for &m in &ENC_MODES {
        for &flag in &[0u8, 1, 2] {
            for &ibc in &[false, true] {
                for &video in &[false, true] {
                    check(
                        &Case {
                            enc_mode: m,
                            enable_dlf_flag: flag,
                            allow_intrabc: ibc,
                            ..Case::default()
                        },
                        video,
                    );
                }
            }
        }
    }
}

/// The exact cell the inter campaign's first video-mode key frame lands on:
/// preset 6, fast-decode 0, 64x64 (240p range), base layer, key frame. C picks
/// level 3, whose `sb_based_dlf = 0` selects the full-image SSE search — NOT
/// the by-q closed form the still arm uses at the same preset (level 5).
#[test]
fn dlf_ctrls_p6_video_key_frame_is_search_not_closed_form() {
    let c = Case {
        enc_mode: 6,
        fast_decode: 0,
        input_res: ResolutionRange::R240p,
        is_highest_layer: false,
        pcs_temporal_layer: 0,
        is_islice: true,
        ..Case::default()
    };
    check(&c, true);
    check(&c, false);

    let video = set_dlf_controls(port_level_default(&c)).unwrap();
    let still = set_dlf_controls(port_level_allintra(&c)).unwrap();
    assert_eq!(port_level_default(&c), 3, "video arm level at M6/base");
    assert_eq!(port_level_allintra(&c), 5, "still arm level at M6");
    assert_eq!(video.enabled, 1);
    assert_eq!(video.sb_based_dlf, 0, "video M6 must take the SSE search");
    assert_eq!(video.early_exit_convergence, 1);
    assert_eq!(still.sb_based_dlf, 1, "still M6 must take the closed form");
}
