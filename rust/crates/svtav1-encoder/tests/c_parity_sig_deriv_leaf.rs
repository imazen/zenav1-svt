//! Differential parity for the leaf per-preset getters of
//! `Source/Lib/Codec/enc_mode_config.c`.
//!
//! **Evidence tier 1** (`docs/WORKING-ON-THIS.md` §4): every assertion below
//! compares the Rust port against the REAL exported C symbol linked out of
//! `Bin/Release/libSvtAv1Enc.a` through `svtav1-cref`. Nothing here compares a
//! transcription against a second transcription.
//!
//! The sweeps are exhaustive over the whole input domain wherever the domain
//! is small enough to enumerate (enc_mode -1..=13 x resolution 0..=6 x
//! coeff_lvl 0..=3 x the boolean flags), so a preset-boundary transcription
//! error cannot hide between sampled points.

use svtav1_cref::sig_deriv as cref;
use svtav1_encoder::port_enc_mode_config::leaf;
use svtav1_encoder::port_enc_mode_config::{InputCoeffLvl, ResolutionRange};

/// Every `EncMode` the C enum spans, MR (-1) included.
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

const COEFF_LVLS: [InputCoeffLvl; 4] = [
    InputCoeffLvl::VLow,
    InputCoeffLvl::Low,
    InputCoeffLvl::Normal,
    InputCoeffLvl::High,
];

#[test]
fn enable_me_8x8_matches_c() {
    for &m in &ENC_MODES {
        for &r in &RESOLUTIONS {
            for &rtc in &[false, true] {
                assert_eq!(
                    leaf::get_enable_me_8x8(m, r, rtc),
                    cref::get_enable_me_8x8(m, r.as_u8(), rtc),
                    "enc_mode={m} res={r:?} rtc={rtc}"
                );
            }
        }
    }
}

#[test]
fn enable_me_16x16_matches_c() {
    for &m in &ENC_MODES {
        assert_eq!(
            leaf::get_enable_me_16x16(m),
            cref::get_enable_me_16x16(m),
            "enc_mode={m}"
        );
    }
}

#[test]
fn gm_levels_match_c() {
    for &m in &ENC_MODES {
        for &sro in &[false, true] {
            assert_eq!(
                leaf::get_gm_core_level(m, sro),
                cref::get_gm_core_level(m, sro),
                "core enc_mode={m} super_res_off={sro}"
            );
            for &isl in &[false, true] {
                assert_eq!(
                    leaf::derive_gm_level(m, isl, sro),
                    cref::derive_gm_level(m, isl, sro),
                    "derive enc_mode={m} is_islice={isl} super_res_off={sro}"
                );
            }
        }
    }
}

#[test]
fn max_can_count_matches_c() {
    for &m in &ENC_MODES {
        for &rtc in &[false, true] {
            assert_eq!(
                leaf::get_max_can_count(m, rtc),
                cref::get_max_can_count(m, rtc),
                "enc_mode={m} rtc={rtc}"
            );
        }
    }
}

#[test]
fn disallow_8x8_matches_c() {
    assert_eq!(
        leaf::get_disallow_8x8_default(),
        cref::get_disallow_8x8_default()
    );
    assert_eq!(
        leaf::get_disallow_8x8_allintra(),
        cref::get_disallow_8x8_allintra()
    );
    // The rtc arm walks `dimensions_require_8x8`, whose loop over
    // 32/16/8 leftovers is the part a transcription can get wrong; sweep every
    // 8-aligned dimension pair through two SB periods plus the odd sizes in
    // between.
    for &m in &ENC_MODES {
        for w in (0u16..=256).step_by(8) {
            for h in [
                0u16, 8, 16, 24, 40, 48, 56, 64, 72, 96, 120, 128, 136, 192, 200,
            ] {
                assert_eq!(
                    leaf::get_disallow_8x8_rtc(m, w, h),
                    cref::get_disallow_8x8_rtc(m, w, h),
                    "enc_mode={m} w={w} h={h}"
                );
            }
        }
    }
}

#[test]
fn nsq_geom_levels_match_c() {
    for &m in &ENC_MODES {
        assert_eq!(
            leaf::get_nsq_geom_level_allintra(m),
            cref::get_nsq_geom_level_allintra(m),
            "allintra enc_mode={m}"
        );
        for &c in &COEFF_LVLS {
            assert_eq!(
                leaf::get_nsq_geom_level_default(m, c),
                cref::get_nsq_geom_level_default(m, c as u8),
                "default enc_mode={m} coeff={c:?}"
            );
        }
    }
    assert_eq!(
        leaf::get_nsq_geom_level_rtc(),
        cref::get_nsq_geom_level_rtc()
    );
}

/// QP values that straddle every bound the `seq_qp_mod` ladders test
/// (39/43/45/48/56/59/60), plus the ends.
const QPS: [u32; 16] = [
    0, 20, 27, 38, 39, 40, 42, 43, 44, 45, 46, 48, 49, 56, 57, 63,
];

#[test]
fn nsq_search_level_default_matches_c() {
    for &m in &ENC_MODES {
        for &c in &COEFF_LVLS {
            for &qp in &QPS {
                for &sqm in &[0u8, 1, 2, 3] {
                    for &tli in &[0u8, 1, 5] {
                        for &(r0_gen, r0) in &[(false, 0.0f64), (true, 0.01), (true, 0.9)] {
                            for &isl in &[false, true] {
                                let ours = leaf::get_nsq_search_level_default(
                                    m, c, qp, tli, r0_gen, r0, isl, tli, sqm,
                                );
                                let theirs = cref::get_nsq_search_level_default(
                                    m, c as u8, qp, tli, r0_gen, r0, isl, tli, sqm,
                                );
                                assert_eq!(
                                    ours, theirs,
                                    "enc_mode={m} coeff={c:?} qp={qp} seq_qp_mod={sqm} \
                                     tli={tli} r0_gen={r0_gen} r0={r0} is_islice={isl}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn nsq_search_level_rtc_and_allintra_match_c() {
    for &c in &COEFF_LVLS {
        for &qp in &QPS {
            for &sqm in &[0u8, 1, 2, 3] {
                assert_eq!(
                    leaf::get_nsq_search_level_rtc(c, qp, sqm),
                    cref::get_nsq_search_level_rtc(c as u8, qp, sqm),
                    "rtc coeff={c:?} qp={qp} seq_qp_mod={sqm}"
                );
                for &m in &ENC_MODES {
                    assert_eq!(
                        leaf::get_nsq_search_level_allintra(m, qp, c, sqm),
                        cref::get_nsq_search_level_allintra(m, qp, c as u8, sqm),
                        "allintra enc_mode={m} coeff={c:?} qp={qp} seq_qp_mod={sqm}"
                    );
                }
            }
        }
    }
}

#[test]
fn nic_levels_match_c() {
    for &m in &ENC_MODES {
        assert_eq!(
            leaf::get_nic_level_rtc(m),
            cref::get_nic_level_rtc(m),
            "rtc enc_mode={m}"
        );
        assert_eq!(
            leaf::get_nic_level_allintra(m),
            cref::get_nic_level_allintra(m),
            "allintra enc_mode={m}"
        );
        for &b in &[false, true] {
            assert_eq!(
                leaf::get_nic_level_default(m, b),
                cref::get_nic_level_default(m, b),
                "default enc_mode={m} is_base={b}"
            );
        }
    }
}

#[test]
fn intra_mode_levels_match_c() {
    for &m in &ENC_MODES {
        assert_eq!(
            leaf::get_intra_mode_levels_allintra(m),
            cref::get_intra_mode_levels_allintra(m),
            "allintra enc_mode={m}"
        );
        for &isl in &[false, true] {
            for &tp in &[0i32, 1] {
                for &ufi in &[false, true] {
                    assert_eq!(
                        leaf::get_intra_mode_levels_rtc(m, isl, tp, ufi),
                        cref::get_intra_mode_levels_rtc(m, isl, tp, ufi),
                        "rtc enc_mode={m} is_islice={isl} tp={tp} flat_ipp={ufi}"
                    );
                }
                for &b in &[false, true] {
                    assert_eq!(
                        leaf::get_intra_mode_levels_default(m, isl, b, tp),
                        cref::get_intra_mode_levels_default(m, isl, b, tp),
                        "default enc_mode={m} is_islice={isl} is_base={b} tp={tp}"
                    );
                }
            }
        }
    }
}

#[test]
fn bypass_encdec_matches_c() {
    for &m in &ENC_MODES {
        assert_eq!(
            leaf::get_bypass_encdec_allintra(m),
            cref::get_bypass_encdec_allintra(m),
            "allintra enc_mode={m}"
        );
        for &bd in &[8u8, 10] {
            assert_eq!(
                leaf::get_bypass_encdec_default(m, bd),
                cref::get_bypass_encdec_default(m, bd),
                "default enc_mode={m} bd={bd}"
            );
            assert_eq!(
                leaf::get_bypass_encdec_rtc(m, bd),
                cref::get_bypass_encdec_rtc(m, bd),
                "rtc enc_mode={m} bd={bd}"
            );
        }
    }
}

#[test]
fn update_cdf_levels_match_c() {
    for &m in &ENC_MODES {
        assert_eq!(
            leaf::get_update_cdf_level_allintra(m),
            cref::get_update_cdf_level_allintra(m),
            "allintra enc_mode={m}"
        );
        for &isl in &[false, true] {
            assert_eq!(
                leaf::get_update_cdf_level_rtc(m, isl),
                cref::get_update_cdf_level_rtc(m, isl),
                "rtc enc_mode={m} is_islice={isl}"
            );
            for &b in &[false, true] {
                assert_eq!(
                    leaf::get_update_cdf_level_default(m, isl, b),
                    cref::get_update_cdf_level_default(m, isl, b),
                    "default enc_mode={m} is_islice={isl} is_base={b}"
                );
            }
        }
    }
}

#[test]
fn chroma_levels_match_c() {
    for &m in &ENC_MODES {
        assert_eq!(
            leaf::get_chroma_level_rtc(m),
            cref::get_chroma_level_rtc(m),
            "rtc enc_mode={m}"
        );
        assert_eq!(
            leaf::get_chroma_level_allintra(m),
            cref::get_chroma_level_allintra(m),
            "allintra enc_mode={m}"
        );
        for &isl in &[false, true] {
            assert_eq!(
                leaf::get_chroma_level_default(m, isl),
                cref::get_chroma_level_default(m, isl),
                "default enc_mode={m} is_islice={isl}"
            );
        }
    }
}

/// Tier-1 reach onto the file-`static` `svt_aom_get_sg_filter_level_*`:
/// `svt_aom_get_enable_sg_*` is the exported wrapper, so a disagreement in the
/// static's preset ladder shows up here as a differing boolean.
///
/// SCOPE FINDING, from the C source rather than assumed: the `_default` arm
/// returns 3 at `enc_mode <= ENC_M3`, so SGR IS live in video mode at presets
/// 0..3 — `rust/CLAUDE.md` envelope guard 5 is an allintra-only statement.
#[test]
fn enable_sg_matches_c() {
    for &m in &ENC_MODES {
        assert_eq!(
            leaf::get_enable_sg_allintra(m),
            cref::get_enable_sg_allintra(m),
            "allintra enc_mode={m}"
        );
        for &r in &RESOLUTIONS {
            for &fd in &[0u8, 1, 2, 3] {
                assert_eq!(
                    leaf::get_enable_sg_default(m, r.as_u8(), fd),
                    cref::get_enable_sg_default(m, r.as_u8(), fd),
                    "default enc_mode={m} res={r:?} fast_decode={fd}"
                );
                assert_eq!(
                    leaf::get_enable_sg_rtc(r.as_u8(), fd),
                    cref::get_enable_sg_rtc(r.as_u8(), fd),
                    "rtc res={r:?} fast_decode={fd}"
                );
            }
        }
    }
}

/// The port's `get_enable_sg_default` at presets 0..3 must be nonzero for the
/// non-8K resolutions — a positive control proving the previous test is not
/// passing because both sides are uniformly zero.
#[test]
fn sg_is_live_in_video_mode_at_low_presets() {
    assert_eq!(
        leaf::get_enable_sg_default(3, ResolutionRange::R1080p.as_u8(), 0),
        1
    );
    assert_eq!(
        leaf::get_enable_sg_default(4, ResolutionRange::R1080p.as_u8(), 0),
        0
    );
    // 8K and fast-decode both force it off.
    assert_eq!(
        leaf::get_enable_sg_default(3, ResolutionRange::R8k.as_u8(), 0),
        0
    );
    assert_eq!(
        leaf::get_enable_sg_default(3, ResolutionRange::R1080p.as_u8(), 1),
        0
    );
}

#[test]
fn inter_compound_level_matches_c() {
    for &m in &ENC_MODES {
        assert_eq!(
            leaf::get_inter_compound_level(m),
            cref::inter_compound_level(m),
            "enc_mode={m}"
        );
    }
}

#[test]
fn obmc_level_matches_c() {
    for &m in &ENC_MODES {
        for qp in 0u32..=63 {
            for &sqm in &[0u8, 1, 2, 3] {
                assert_eq!(
                    leaf::get_obmc_level(m, qp, sqm),
                    cref::get_obmc_level(m, qp, sqm),
                    "enc_mode={m} qp={qp} seq_qp_mod={sqm}"
                );
            }
        }
    }
}

#[test]
fn mfmv_config_matches_c() {
    for &m in &ENC_MODES {
        for &rtc in &[false, true] {
            for &cfg in &[-1i32, 0, 1] {
                assert_eq!(
                    leaf::set_mfmv_config(m, rtc, cfg),
                    cref::set_mfmv_config(m, rtc, cfg),
                    "enc_mode={m} rtc={rtc} cfg={cfg}"
                );
            }
        }
    }
}

#[test]
fn pre_analysis_pcs_matches_c() {
    // Luma areas that land in each resolution bucket, plus the exact
    // thresholds so an off-by-one in the bucket boundary is caught.
    // Widths/heights are `uint16_t` in C (`sequence_control_set.h:113-114`),
    // so the bucket boundaries are probed with representable pairs whose
    // PRODUCT straddles each threshold (0x28500, 0x4CE00, ... 0x140A000).
    let dims: [(u16, u16); 16] = [
        (16, 16),
        (352, 288),
        (640, 360),
        (854, 480),
        (1280, 720),
        (1920, 1080),
        (3840, 2160),
        (7680, 4320),
        (256, 645),   // 165120 == INPUT_SIZE_240p_TH
        (256, 644),   // one bucket below it
        (256, 1230),  // 314880 == INPUT_SIZE_360p_TH
        (256, 2580),  // 660480 == INPUT_SIZE_480p_TH
        (256, 5850),  // 1497600 == INPUT_SIZE_720p_TH
        (256, 21330), // 5460480 == INPUT_SIZE_1080p_TH
        (512, 41040), // 21012480 == INPUT_SIZE_4K_TH
        (512, 41039), // one bucket below it
    ];
    for &m in &ENC_MODES {
        for &(w, h) in &dims {
            for &rtc in &[false, true] {
                let ours = leaf::sig_deriv_pre_analysis_pcs(m, w, h, rtc);
                let theirs = cref::sig_deriv_pre_analysis_pcs(m, w, h, rtc);
                assert_eq!(
                    [
                        ours.enable_me_16x16,
                        ours.enable_me_8x8,
                        ours.enable_hme_flag,
                        ours.enable_hme_level0_flag,
                        ours.enable_hme_level1_flag,
                        ours.enable_hme_level2_flag,
                        ours.tf_enable_hme_flag,
                        ours.tf_enable_hme_level0_flag,
                        ours.tf_enable_hme_level1_flag,
                        ours.tf_enable_hme_level2_flag,
                    ],
                    theirs,
                    "enc_mode={m} {w}x{h} rtc={rtc}"
                );
            }
        }
    }
}

#[test]
fn is_ref_same_size_matches_c() {
    for &ins in &[false, true] {
        for &b in &[false, true] {
            for &present in &[false, true] {
                for &(rw, rh) in &[(64u16, 64u16), (32, 64), (64, 32)] {
                    let ours = leaf::is_ref_same_size(ins, b, present, rw, rh, 64, 64);
                    let theirs = cref::is_ref_same_size(ins, b, present, rw, rh, 64, 64);
                    assert_eq!(
                        ours, theirs,
                        "not_scaled={ins} b_slice={b} present={present} ref={rw}x{rh}"
                    );
                }
            }
        }
    }
}

/// A positive control for the `is_ref_same_size` sweep: with scaling ON, a
/// B-slice and a present same-size reference, C must return true, and with a
/// differently-sized reference it must return false. Without this, the sweep
/// above would pass if both sides always returned the same constant.
#[test]
fn is_ref_same_size_positive_control() {
    assert!(cref::is_ref_same_size(false, true, true, 64, 64, 64, 64));
    assert!(!cref::is_ref_same_size(false, true, true, 32, 64, 64, 64));
    assert!(!cref::is_ref_same_size(false, false, true, 64, 64, 64, 64));
}

/// The premise `part_arm::VIDEO_ISLICE_COEFF_LVL` rests on, pinned against the
/// real C symbols rather than left as a reading of the source.
///
/// C leaves `pcs->coeff_lvl` at `INVALID_LVL` (`~0` = -1) for a video-mode
/// I-slice: `md_config_process.c:898-902` runs `derive_intra_coeff_level` only
/// under `scs->allintra`, and `derive_inter_coeff_level` only when
/// `!rtc && slice_type != I_SLICE`. A video KEY frame — the only video picture
/// this port encodes today — matches neither, so both `_default` ladders see
/// `INVALID_LVL`.
///
/// Both ladders test `coeff_lvl` only by equality against `HIGH_LVL` or
/// against `VLOW_LVL | LOW_LVL`, so `INVALID_LVL` must behave exactly as
/// `NORMAL_LVL`. If upstream ever adds an ordering comparison, this test goes
/// red and the port's `NORMAL` stand-in stops being sound.
#[test]
fn nsq_levels_treat_invalid_coeff_lvl_as_normal() {
    let normal = InputCoeffLvl::Normal as u8;
    for &m in &ENC_MODES {
        let invalid = cref::get_nsq_geom_level_default_raw(m, cref::INVALID_COEFF_LVL);
        assert_eq!(
            invalid,
            cref::get_nsq_geom_level_default(m, normal),
            "geom enc_mode={m}: INVALID_LVL must equal NORMAL_LVL"
        );
        // ... and the port's own arm helper reproduces it.
        assert_eq!(
            invalid,
            leaf::get_nsq_geom_level_default(m, InputCoeffLvl::Normal),
            "geom enc_mode={m}: port vs C at INVALID_LVL"
        );
    }
    for &m in &ENC_MODES {
        for &qp in &QPS {
            for &sqm in &[0u8, 1, 2, 3] {
                for &isl in &[false, true] {
                    let invalid = cref::get_nsq_search_level_default_raw(
                        m,
                        cref::INVALID_COEFF_LVL,
                        qp,
                        0,
                        false,
                        0.0,
                        isl,
                        0,
                        sqm,
                    );
                    assert_eq!(
                        invalid,
                        cref::get_nsq_search_level_default(
                            m, normal, qp, 0, false, 0.0, isl, 0, sqm
                        ),
                        "search enc_mode={m} qp={qp} sqm={sqm} isl={isl}: \
                         INVALID_LVL must equal NORMAL_LVL"
                    );
                    assert_eq!(
                        invalid,
                        leaf::get_nsq_search_level_default(
                            m,
                            InputCoeffLvl::Normal,
                            qp,
                            0,
                            false,
                            0.0,
                            isl,
                            0,
                            sqm,
                        ),
                        "search enc_mode={m} qp={qp} sqm={sqm} isl={isl}: port vs C at INVALID_LVL"
                    );
                }
            }
        }
    }
}

/// A positive control for the test above: `HIGH_LVL` must NOT agree with
/// `NORMAL_LVL`, or the assertions would pass on a constant function and prove
/// nothing (`docs/WORKING-ON-THIS.md` §5 — prove the probe fires).
#[test]
fn invalid_coeff_lvl_probe_has_a_positive_control() {
    let mut geom_differs = false;
    let mut search_differs = false;
    for &m in &ENC_MODES {
        if cref::get_nsq_geom_level_default(m, InputCoeffLvl::High as u8)
            != cref::get_nsq_geom_level_default(m, InputCoeffLvl::Normal as u8)
        {
            geom_differs = true;
        }
        for &qp in &QPS {
            let hi = cref::get_nsq_search_level_default(
                m,
                InputCoeffLvl::High as u8,
                qp,
                0,
                false,
                0.0,
                true,
                0,
                2,
            );
            let no = cref::get_nsq_search_level_default(
                m,
                InputCoeffLvl::Normal as u8,
                qp,
                0,
                false,
                0.0,
                true,
                0,
                2,
            );
            if hi != no {
                search_differs = true;
            }
        }
    }
    assert!(geom_differs, "geom ladder ignores coeff_lvl entirely");
    assert!(search_differs, "search ladder ignores coeff_lvl entirely");
}
