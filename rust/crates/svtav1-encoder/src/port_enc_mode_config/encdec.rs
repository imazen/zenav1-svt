//! The PD1 per-SB `level -> controls` tables of `Codec/enc_mode_config.c` that
//! `svt_aom_sig_deriv_enc_dec_default` drives.
//!
//! Every function here is file-`static` in C, but each is reached from the
//! EXPORTED `svt_aom_sig_deriv_enc_dec_default(pcs, ctx)` by setting one
//! picture-level field on the PCS — so `c_parity_sig_deriv_encdec.rs` gates
//! them at **tier 1** by driving that entry point on a synthetic
//! PictureControlSet and reading the resulting `ModeDecisionContext` back,
//! rather than hand-deriving a dozen vector sets at tier 4.

use super::ctrls::{DistortionType, MAX_SIGNED_VALUE};

// ---------------------------------------------------------------------------
// Small enums
// ---------------------------------------------------------------------------

/// C `TxCoeffShape` (`definitions.h:2061`) — the partial-frequency shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum TxCoeffShape {
    /// `DEFAULT_SHAPE` — the whole transform block.
    #[default]
    Default = 0,
    /// `N2_SHAPE`
    N2 = 1,
    /// `N4_SHAPE`
    N4 = 2,
    /// `ONLY_DC_SHAPE`
    OnlyDc = 3,
}

/// C `SpatialSseLevel` (`definitions.h:885`).
///
/// NOTE the declaration order: `SSSE_MDS1` is **0** and `SSSE_OFF` is **3** —
/// "off" is the LAST value, not the first, so a zeroed context means "spatial
/// SSE from MDS1 onward", not "off".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum SpatialSseLevel {
    /// `SSSE_MDS1`
    #[default]
    Mds1 = 0,
    /// `SSSE_MDS2`
    Mds2 = 1,
    /// `SSSE_MDS3`
    Mds3 = 2,
    /// `SSSE_OFF`
    Off = 3,
}

/// C `SUBPEL_SEARCH_TYPE` (`definitions.h:855`).
pub mod subpel_search_type {
    /// `USE_2_TAPS_ORIG` — used by temporal filtering.
    pub const USE_2_TAPS_ORIG: u8 = 0;
    /// `USE_2_TAPS`
    pub const USE_2_TAPS: u8 = 1;
    /// `USE_4_TAPS`
    pub const USE_4_TAPS: u8 = 2;
    /// `USE_8_TAPS`
    pub const USE_8_TAPS: u8 = 3;
}

/// C `SUBPEL_SEARCH_METHODS` (`definitions.h:862`).
pub mod subpel_search_method {
    /// `SUBPEL_TREE`
    pub const SUBPEL_TREE: u8 = 0;
    /// `SUBPEL_TREE_PRUNED`
    pub const SUBPEL_TREE_PRUNED: u8 = 1;
    /// `SUBPEL_FIXED_STAGE_SEARCH`
    pub const SUBPEL_FIXED_STAGE_SEARCH: u8 = 2;
}

/// C `SUBPEL_FORCE_STOP` (`definitions.h:868`).
pub mod subpel_force_stop {
    /// `EIGHTH_PEL`
    pub const EIGHTH_PEL: u8 = 0;
    /// `QUARTER_PEL`
    pub const QUARTER_PEL: u8 = 1;
    /// `HALF_PEL`
    pub const HALF_PEL: u8 = 2;
    /// `FULL_PEL`
    pub const FULL_PEL: u8 = 3;
}

// ---------------------------------------------------------------------------
// Sub-resolution / partial-frequency
// ---------------------------------------------------------------------------

/// C `SubresCtrls` (`md_process.h:278`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SubresCtrls {
    /// `step` — residual sub-sampling step, 0 is off.
    pub step: u8,
    /// `odd_to_even_deviation_th`
    pub odd_to_even_deviation_th: u8,
}

/// C `set_subres_controls` (`enc_mode_config.c:3388`). static.
///
/// The deviation threshold is derived from the RESULTING step, not from the
/// level — a distinction that matters if a future level maps to step 0.
#[must_use]
pub fn set_subres_controls(subres_level: u8) -> Option<SubresCtrls> {
    let step = match subres_level {
        0 => 0,
        1 => 1,
        2 => 2,
        _ => return None,
    };
    Some(SubresCtrls {
        step,
        odd_to_even_deviation_th: if step == 0 { 0 } else { 5 },
    })
}

/// C `PfCtrls` (`md_process.h:287`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PfCtrls {
    /// `pf_shape`
    pub pf_shape: TxCoeffShape,
}

/// C `set_pf_controls` (`enc_mode_config.c:3413`). static.
#[must_use]
pub fn set_pf_controls(pf_level: u8) -> Option<PfCtrls> {
    let pf_shape = match pf_level {
        0 => TxCoeffShape::OnlyDc,
        1 => TxCoeffShape::Default,
        2 => TxCoeffShape::N2,
        3 => TxCoeffShape::N4,
        _ => return None,
    };
    Some(PfCtrls { pf_shape })
}

// ---------------------------------------------------------------------------
// Spatial SSE
// ---------------------------------------------------------------------------

/// C `SpatialSSECtrls` (`md_process.h:667`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SpatialSseCtrls {
    /// `level`
    pub level: SpatialSseLevel,
}

/// C `set_spatial_sse_full_loop_level` (`enc_mode_config.c:6256`). static.
#[must_use]
pub fn set_spatial_sse_full_loop_level(level: u8) -> Option<SpatialSseCtrls> {
    let level = match level {
        0 => SpatialSseLevel::Off,
        1 => SpatialSseLevel::Mds1,
        2 => SpatialSseLevel::Mds2,
        3 => SpatialSseLevel::Mds3,
        _ => return None,
    };
    Some(SpatialSseCtrls { level })
}

// ---------------------------------------------------------------------------
// TX shortcut / coefficient shaving
// ---------------------------------------------------------------------------

/// C `TxShortcutCtrls` (`md_process.h:791`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TxShortcutCtrls {
    /// `bypass_tx_th`
    pub bypass_tx_th: u32,
    /// `apply_pf_on_coeffs`
    pub apply_pf_on_coeffs: u8,
    /// `chroma_detector_level`
    pub chroma_detector_level: u8,
    /// `use_mds3_shortcuts_th`
    pub use_mds3_shortcuts_th: u32,
}

/// C `set_tx_shortcut_ctrls` (`enc_mode_config.c:6722`). static.
///
/// C's trailing block is an `assert` only (the chroma detector must be on for
/// reference frames at low presets); it writes nothing, so the port carries it
/// as a `debug_assert` on the same predicate.
#[must_use]
pub fn set_tx_shortcut_ctrls(
    tx_shortcut_level: u8,
    is_not_leaf: bool,
    enc_mode: i8,
) -> Option<TxShortcutCtrls> {
    let c = match tx_shortcut_level {
        0 => TxShortcutCtrls {
            bypass_tx_th: 0,
            apply_pf_on_coeffs: 0,
            use_mds3_shortcuts_th: 0,
            chroma_detector_level: 0,
        },
        1 => TxShortcutCtrls {
            bypass_tx_th: 4,
            apply_pf_on_coeffs: 1,
            use_mds3_shortcuts_th: 0,
            chroma_detector_level: 1,
        },
        2 => TxShortcutCtrls {
            bypass_tx_th: 1,
            apply_pf_on_coeffs: 1,
            use_mds3_shortcuts_th: 0,
            chroma_detector_level: 1,
        },
        3 => TxShortcutCtrls {
            bypass_tx_th: 1,
            apply_pf_on_coeffs: 1,
            use_mds3_shortcuts_th: 10,
            chroma_detector_level: 1,
        },
        _ => return None,
    };
    debug_assert!(
        tx_shortcut_level == 0
            || !is_not_leaf
            || enc_mode > super::enc_mode::M9
            || c.chroma_detector_level != 0,
        "chroma detector must be on for ref frames at low presets"
    );
    Some(c)
}

/// C `CoeffShavingCtrls` (`md_process.h:443`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CoeffShavingCtrls {
    /// `enabled`
    pub enabled: u8,
    /// `level_threshold`
    pub level_threshold: i32,
    /// `zero_gap_threshold`
    pub zero_gap_threshold: i32,
    /// `rd_zero_strength`
    pub rd_zero_strength: i32,
}

/// C `set_coeff_shaving_controls` (`enc_mode_config.c:3841`). static.
#[must_use]
pub fn set_coeff_shaving_controls(coeff_shaving_level: u8) -> Option<CoeffShavingCtrls> {
    match coeff_shaving_level {
        0 => Some(CoeffShavingCtrls::default()),
        1 => Some(CoeffShavingCtrls {
            enabled: 1,
            level_threshold: 1,
            zero_gap_threshold: 8,
            rd_zero_strength: 32,
        }),
        2 => Some(CoeffShavingCtrls {
            enabled: 1,
            level_threshold: 1,
            zero_gap_threshold: 8,
            rd_zero_strength: 0,
        }),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Depth early exit / sub-depth skip
// ---------------------------------------------------------------------------

/// C `DepthEarlyExitCtrls` (`md_process.h:586`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DepthEarlyExitCtrls {
    /// `split_cost_th`
    pub split_cost_th: u16,
    /// `early_exit_th`
    pub early_exit_th: u16,
}

/// C `set_depth_early_exit_ctrls` (`enc_mode_config.c:7182`). static.
///
/// The preset threshold that selects the level differs between the arms:
/// allintra uses `<= ENC_M7`, the video arm `<= ENC_M6`.
#[must_use]
pub fn set_depth_early_exit_ctrls(early_exit_level: u8) -> Option<DepthEarlyExitCtrls> {
    match early_exit_level {
        0 => Some(DepthEarlyExitCtrls {
            split_cost_th: 0,
            early_exit_th: 0,
        }),
        1 => Some(DepthEarlyExitCtrls {
            split_cost_th: 50,
            early_exit_th: 0,
        }),
        2 => Some(DepthEarlyExitCtrls {
            split_cost_th: 50,
            early_exit_th: 900,
        }),
        _ => None,
    }
}

/// C `SkipSubDepthCtrls` (`md_process.h:825`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SkipSubDepthCtrls {
    /// `enabled`
    pub enabled: u8,
    /// `max_size`
    pub max_size: u8,
    /// `quad_deviation_th`
    pub quad_deviation_th: i32,
    /// `coeff_perc`
    pub coeff_perc: u8,
}

/// C `set_skip_sub_depth_ctrls` (`enc_mode_config.c:6787`). static.
///
/// The preset boundary that picks the level differs between the arms:
/// allintra is `<= ENC_M7`, the video arm `<= ENC_M1` — so a video encode at
/// M2..M7 takes a level the still path never selects.
#[must_use]
pub fn set_skip_sub_depth_ctrls(skip_sub_depth_lvl: u8) -> Option<SkipSubDepthCtrls> {
    match skip_sub_depth_lvl {
        0 => Some(SkipSubDepthCtrls::default()),
        1 => Some(SkipSubDepthCtrls {
            enabled: 1,
            max_size: 16,
            quad_deviation_th: 250,
            coeff_perc: 15,
        }),
        2 => Some(SkipSubDepthCtrls {
            enabled: 1,
            max_size: 16,
            quad_deviation_th: 250,
            coeff_perc: 25,
        }),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// LPD1 transform controls
// ---------------------------------------------------------------------------

/// C `Lpd1TxSkipDecisionCtrls` (`md_process.h:731`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Lpd1TxSkipDecisionCtrls {
    /// `skip_tx_score_th`
    pub skip_tx_score_th: i32,
    /// `dist_energy_th`
    pub dist_energy_th: u16,
    /// `rd_skip_th`
    pub rd_skip_th: u16,
}

/// C `set_lpd1_tx_skip_decision_ctrls` (`enc_mode_config.c:6300`). static.
///
/// Level 0 writes ONLY `skip_tx_score_th`; the other two keep the context's
/// prior values, modelled here from a zeroed struct.
#[must_use]
pub fn set_lpd1_tx_skip_decision_ctrls(level: u8) -> Option<Lpd1TxSkipDecisionCtrls> {
    match level {
        0 => Some(Lpd1TxSkipDecisionCtrls::default()),
        1 => Some(Lpd1TxSkipDecisionCtrls {
            skip_tx_score_th: 125,
            dist_energy_th: 30,
            rd_skip_th: 100,
        }),
        2 => Some(Lpd1TxSkipDecisionCtrls {
            skip_tx_score_th: 125,
            dist_energy_th: 30,
            rd_skip_th: 200,
        }),
        3 => Some(Lpd1TxSkipDecisionCtrls {
            skip_tx_score_th: 70,
            dist_energy_th: 30,
            rd_skip_th: 200,
        }),
        4 => Some(Lpd1TxSkipDecisionCtrls {
            skip_tx_score_th: 50,
            dist_energy_th: 30,
            rd_skip_th: 200,
        }),
        5 => Some(Lpd1TxSkipDecisionCtrls {
            skip_tx_score_th: 50,
            dist_energy_th: 40,
            rd_skip_th: 200,
        }),
        _ => None,
    }
}

/// C `Lpd1TxCtrls` (`md_process.h:743`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Lpd1TxCtrls {
    /// `zero_y_coeff_exit`
    pub zero_y_coeff_exit: u8,
    /// `chroma_detector_level`
    pub chroma_detector_level: u8,
    /// `use_uv_shortcuts_on_y_coeffs`
    pub use_uv_shortcuts_on_y_coeffs: u8,
    /// `use_mds3_shortcuts_th`
    pub use_mds3_shortcuts_th: u32,
}

/// C `set_lpd1_tx_ctrls` (`enc_mode_config.c:6338`). static.
///
/// Levels 1..6 differ ONLY in `chroma_detector_level` (1, 1, 2, 3, 4, 0) — note
/// level 6 goes back to 0, it is not a monotone ladder.
#[must_use]
pub fn set_lpd1_tx_ctrls(lpd1_tx_level: u8) -> Option<Lpd1TxCtrls> {
    let chroma_detector_level = match lpd1_tx_level {
        0 => return Some(Lpd1TxCtrls::default()),
        1 | 2 => 1,
        3 => 2,
        4 => 3,
        5 => 4,
        6 => 0,
        _ => return None,
    };
    Some(Lpd1TxCtrls {
        zero_y_coeff_exit: 1,
        chroma_detector_level,
        use_uv_shortcuts_on_y_coeffs: 1,
        use_mds3_shortcuts_th: 30,
    })
}

// ---------------------------------------------------------------------------
// MD motion search controls
// ---------------------------------------------------------------------------

/// C `MdNsqMotionSearchCtrls` (`md_process.h:291`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MdNsqMotionSearchCtrls {
    /// `enabled`
    pub enabled: u8,
    /// `dist_type`
    pub dist_type: DistortionType,
    /// `full_pel_search_width`
    pub full_pel_search_width: u8,
    /// `full_pel_search_height`
    pub full_pel_search_height: u8,
    /// `enable_psad`
    pub enable_psad: u8,
}

/// C `md_nsq_motion_search_controls` (`enc_mode_config.c:3283`). static.
#[must_use]
pub fn md_nsq_motion_search_controls(level: u8) -> Option<MdNsqMotionSearchCtrls> {
    match level {
        0 => Some(MdNsqMotionSearchCtrls::default()),
        1 => Some(MdNsqMotionSearchCtrls {
            enabled: 1,
            dist_type: DistortionType::Var,
            full_pel_search_width: 32,
            full_pel_search_height: 16,
            enable_psad: 1,
        }),
        2 => Some(MdNsqMotionSearchCtrls {
            enabled: 1,
            dist_type: DistortionType::Var,
            full_pel_search_width: 16,
            full_pel_search_height: 8,
            enable_psad: 1,
        }),
        _ => None,
    }
}

/// C `MdSqMotionSearchCtrls` (`md_process.h:304`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MdSqMotionSearchCtrls {
    /// `enabled`
    pub enabled: u8,
    /// `dist_type`
    pub dist_type: DistortionType,
    /// `pame_distortion_th`
    pub pame_distortion_th: u16,
    /// `sprs_lev0_enabled`
    pub sprs_lev0_enabled: u8,
    /// `sprs_lev0_step`
    pub sprs_lev0_step: u8,
    /// `sprs_lev0_w`
    pub sprs_lev0_w: u16,
    /// `sprs_lev0_h`
    pub sprs_lev0_h: u16,
    /// `max_sprs_lev0_w`
    pub max_sprs_lev0_w: u16,
    /// `max_sprs_lev0_h`
    pub max_sprs_lev0_h: u16,
    /// `sprs_lev0_multiplier`
    pub sprs_lev0_multiplier: i16,
    /// `sprs_lev1_enabled`
    pub sprs_lev1_enabled: u8,
    /// `sprs_lev1_step`
    pub sprs_lev1_step: u8,
    /// `sprs_lev1_w`
    pub sprs_lev1_w: u16,
    /// `sprs_lev1_h`
    pub sprs_lev1_h: u16,
    /// `max_sprs_lev1_w`
    pub max_sprs_lev1_w: u16,
    /// `max_sprs_lev1_h`
    pub max_sprs_lev1_h: u16,
    /// `sprs_lev1_multiplier`
    pub sprs_lev1_multiplier: i16,
    /// `sprs_lev2_enabled`
    pub sprs_lev2_enabled: u8,
    /// `sprs_lev2_step`
    pub sprs_lev2_step: u8,
    /// `sprs_lev2_w`
    pub sprs_lev2_w: u16,
    /// `sprs_lev2_h`
    pub sprs_lev2_h: u16,
    /// `enable_psad`
    pub enable_psad: u8,
}

/// C `md_sq_motion_search_controls` (`enc_mode_config.c:3438`). static.
///
/// Levels 1..4 are IDENTICAL except for the two sparse-search multipliers
/// (500, 400, 300, 100) — everything else repeats verbatim in the C.
#[must_use]
pub fn md_sq_motion_search_controls(level: u8) -> Option<MdSqMotionSearchCtrls> {
    let multiplier: i16 = match level {
        0 => return Some(MdSqMotionSearchCtrls::default()),
        1 => 500,
        2 => 400,
        3 => 300,
        4 => 100,
        _ => return None,
    };
    Some(MdSqMotionSearchCtrls {
        enabled: 1,
        dist_type: DistortionType::Sad,
        pame_distortion_th: 10,
        sprs_lev0_enabled: 1,
        sprs_lev0_step: 4,
        sprs_lev0_w: 15,
        sprs_lev0_h: 15,
        max_sprs_lev0_w: 150,
        max_sprs_lev0_h: 150,
        sprs_lev0_multiplier: multiplier,
        sprs_lev1_enabled: 1,
        sprs_lev1_step: 2,
        sprs_lev1_w: 4,
        sprs_lev1_h: 4,
        max_sprs_lev1_w: 50,
        max_sprs_lev1_h: 50,
        sprs_lev1_multiplier: multiplier,
        sprs_lev2_enabled: 1,
        sprs_lev2_step: 1,
        sprs_lev2_w: 3,
        sprs_lev2_h: 3,
        enable_psad: 1,
    })
}

/// C `MdSubPelSearchCtrls` (`md_process.h:384`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MdSubPelSearchCtrls {
    /// `enabled`
    pub enabled: u8,
    /// `subpel_search_type`
    pub subpel_search_type: u8,
    /// `max_precision`
    pub max_precision: u8,
    /// `subpel_search_method`
    pub subpel_search_method: u8,
    /// `subpel_iters_per_step`
    pub subpel_iters_per_step: i32,
    /// `pred_variance_th`
    pub pred_variance_th: i32,
    /// `abs_th_mult`
    pub abs_th_mult: u8,
    /// `round_dev_th`
    pub round_dev_th: i32,
    /// `skip_diag_refinement`
    pub skip_diag_refinement: u8,
    /// `min_blk_sz`
    pub min_blk_sz: u8,
    /// `mvp_th`
    pub mvp_th: u8,
    /// `hp_mv_th`
    pub hp_mv_th: i32,
    /// `bias_fp`
    pub bias_fp: i32,
}

/// C `md_subpel_me_controls` (`enc_mode_config.c:3563`). static.
///
/// Levels 7..10 take the `SUBPEL_FIXED_STAGE_SEARCH` path and DELIBERATELY
/// leave six fields unwritten (`subpel_search_type`, `subpel_iters_per_step`,
/// `round_dev_th`, `skip_diag_refinement`, `mvp_th`, `hp_mv_th`) — C's own
/// comment says so. Those keep the context's prior values, which the port
/// models as the zeroed struct.
#[must_use]
pub fn md_subpel_me_controls(level: u8) -> Option<MdSubPelSearchCtrls> {
    use subpel_force_stop::{EIGHTH_PEL, QUARTER_PEL};
    use subpel_search_method::{SUBPEL_FIXED_STAGE_SEARCH, SUBPEL_TREE, SUBPEL_TREE_PRUNED};
    use subpel_search_type::{USE_4_TAPS, USE_8_TAPS};

    let mut c = MdSubPelSearchCtrls::default();
    match level {
        0 => {
            c.enabled = 0;
            return Some(c);
        }
        1..=6 => {
            c.enabled = 1;
            c.subpel_search_type = if level == 1 { USE_8_TAPS } else { USE_4_TAPS };
            c.subpel_iters_per_step = if level == 6 { 1 } else { 2 };
            c.max_precision = if level == 6 { QUARTER_PEL } else { EIGHTH_PEL };
            c.subpel_search_method = if level <= 3 {
                SUBPEL_TREE
            } else {
                SUBPEL_TREE_PRUNED
            };
            c.pred_variance_th = 0;
            c.abs_th_mult = 0;
            c.round_dev_th = MAX_SIGNED_VALUE;
            c.skip_diag_refinement = if level == 6 { 3 } else { 0 };
            c.min_blk_sz = if level == 1 { 0 } else { 4 };
            c.mvp_th = match level {
                1 => 0,
                6 => 12,
                _ => 18,
            };
            c.hp_mv_th = if level == 1 { MAX_SIGNED_VALUE } else { 32 };
            c.bias_fp = match level {
                1 | 2 => 0,
                3 | 4 => 104,
                _ => 110,
            };
        }
        7..=10 => {
            // The fixed-stage path; six fields are intentionally left at the
            // context's prior (zeroed) values.
            c.enabled = 1;
            c.max_precision = QUARTER_PEL;
            c.subpel_search_method = SUBPEL_FIXED_STAGE_SEARCH;
            c.pred_variance_th = if level >= 9 { 100 } else { 0 };
            c.abs_th_mult = match level {
                7 => 0,
                8 => 10,
                9 => 20,
                _ => 25,
            };
            c.min_blk_sz = 4;
            c.bias_fp = 110;
        }
        _ => return None,
    }
    Some(c)
}

/// C `md_subpel_pme_controls` (`enc_mode_config.c:3709`). static.
///
/// Unlike the ME variant, every level here uses `USE_8_TAPS` and leaves
/// `skip_diag_refinement` unwritten, and levels 1..4 differ only in
/// `subpel_search_method`, `max_precision` and `bias_fp`.
#[must_use]
pub fn md_subpel_pme_controls(level: u8) -> Option<MdSubPelSearchCtrls> {
    use subpel_force_stop::{EIGHTH_PEL, HALF_PEL};
    use subpel_search_method::{SUBPEL_TREE, SUBPEL_TREE_PRUNED};
    use subpel_search_type::USE_8_TAPS;

    let mut c = MdSubPelSearchCtrls::default();
    match level {
        0 => {
            c.enabled = 0;
            return Some(c);
        }
        1..=4 => {
            c.enabled = 1;
            c.subpel_search_type = USE_8_TAPS;
            c.subpel_iters_per_step = 2;
            c.max_precision = if level == 4 { HALF_PEL } else { EIGHTH_PEL };
            c.subpel_search_method = if level == 1 {
                SUBPEL_TREE
            } else {
                SUBPEL_TREE_PRUNED
            };
            c.pred_variance_th = 0;
            c.abs_th_mult = 0;
            c.round_dev_th = MAX_SIGNED_VALUE;
            c.min_blk_sz = 0;
            c.mvp_th = 0;
            c.hp_mv_th = 0;
            c.bias_fp = match level {
                1 => 0,
                2 => 104,
                _ => 110,
            };
        }
        _ => return None,
    }
    Some(c)
}

// ---------------------------------------------------------------------------
// PD1 level selection inside svt_aom_sig_deriv_enc_dec_default
// ---------------------------------------------------------------------------

/// The two levels `svt_aom_sig_deriv_enc_dec_default` derives from `enc_mode`
/// alone (`enc_mode_config.c:7876` and `:7920`) rather than reading off the PCS.
///
/// Both preset boundaries differ from the allintra twin: allintra uses
/// `<= ENC_M7` for the depth-early-exit level and `<= ENC_M7` for the
/// sub-depth-skip level, the video arm `<= ENC_M6` and `<= ENC_M1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncDecDefaultDerivedLevels {
    /// `depth_early_exit_lvl`
    pub depth_early_exit_lvl: u8,
    /// `skip_sub_depth_lvl`
    pub skip_sub_depth_lvl: u8,
}

/// The `enc_mode`-derived levels of C `svt_aom_sig_deriv_enc_dec_default`
/// (`enc_mode_config.c:7815`).
#[must_use]
pub fn enc_dec_default_derived_levels(enc_mode: i8) -> EncDecDefaultDerivedLevels {
    EncDecDefaultDerivedLevels {
        depth_early_exit_lvl: if enc_mode <= super::enc_mode::M6 {
            1
        } else {
            2
        },
        skip_sub_depth_lvl: if enc_mode <= super::enc_mode::M1 {
            1
        } else {
            2
        },
    }
}

/// C `get_max_block_size_default` (`enc_mode_config.c:6991`). static.
///
/// The video arm applies NO cap: `ctx->max_block_size = scs->super_block_size`.
/// (The rtc arm is the one that caps by preset.)
#[must_use]
pub fn get_max_block_size_default(super_block_size: u32) -> u32 {
    super_block_size
}

// ---------------------------------------------------------------------------
// svt_aom_sig_deriv_enc_dec_default
// ---------------------------------------------------------------------------

use super::ctrls::{
    Bipred3x3Controls, IfsLevel, InterCompCtrls, InterIntraCompCtrls, MdPmeCtrls, ObmcControls,
    RefPruningControls, WmCtrls, md_pme_search_controls, set_bipred3x3_controls,
    set_dist_based_ref_pruning_controls, set_inter_comp_controls, set_inter_intra_ctrls,
    set_interpolation_search_level_ctrls, set_obmc_controls, set_wm_controls,
};

/// C `ChromaLevel` (`definitions.h:2068`) — `ctx->uv_ctrls.uv_mode`.
pub mod chroma_mode {
    /// `CHROMA_MODE_0` — full chroma search at MD.
    pub const FULL: u8 = 0;
    /// `CHROMA_MODE_1` — fast chroma search at MD.
    pub const FAST: u8 = 1;
    /// `CHROMA_MODE_2` — chroma blind at MD.
    pub const BLIND: u8 = 2;
}

/// C `SSIM_LVL_0` — the value `svt_aom_sig_deriv_enc_dec_default` always
/// writes into `ctx->tune_ssim_level`.
pub const SSIM_LVL_0: u8 = 0;

/// The picture-level levels `svt_aom_sig_deriv_enc_dec_default` reads off the
/// PCS/PPCS. Every one of these is a signal some OTHER derivation set; this
/// function only routes them into per-SB controls.
#[derive(Debug, Clone, Copy, Default)]
pub struct EncDecDefaultInputs {
    /// `pcs->enc_mode`
    pub enc_mode: i8,
    /// `pcs->slice_type == I_SLICE`
    pub is_islice: bool,
    /// `frame_is_leaf(ppcs)` — `update_type == SVT_AV1_LF_UPDATE`.
    pub is_leaf: bool,
    /// `pcs->tx_shortcut_level`
    pub tx_shortcut_level: u8,
    /// `pcs->interpolation_search_level`
    pub interpolation_search_level: u8,
    /// `pcs->wm_level`
    pub wm_level: u8,
    /// `pcs->bipred3x3_injection`
    pub bipred3x3_injection: u8,
    /// `pcs->unipred3x3_injection`
    pub unipred3x3_injection: u8,
    /// `pcs->new_nearest_near_comb_injection`
    pub new_nearest_near_comb_injection: u8,
    /// `pcs->inter_compound_mode`
    pub inter_compound_mode: u8,
    /// `pcs->dist_based_ref_pruning`
    pub dist_based_ref_pruning: u8,
    /// `pcs->spatial_sse_full_loop_level`
    pub spatial_sse_full_loop_level: u8,
    /// `pcs->coeff_shaving_level`
    pub coeff_shaving_level: u8,
    /// `ppcs->pic_obmc_level`
    pub pic_obmc_level: u8,
    /// `pcs->inter_intra_level`
    pub inter_intra_level: u8,
    /// `pcs->md_sq_mv_search_level`
    pub md_sq_mv_search_level: u8,
    /// `pcs->md_nsq_mv_search_level`
    pub md_nsq_mv_search_level: u8,
    /// `pcs->md_pme_level`
    pub md_pme_level: u8,
    /// `pcs->me_subpel_level`
    pub me_subpel_level: u8,
    /// `pcs->pme_subpel_level`
    pub pme_subpel_level: u8,
    /// `pcs->approx_inter_rate`
    pub approx_inter_rate: u8,
    /// `ppcs->frm_hdr.allow_intrabc`
    pub allow_intrabc: u8,
    /// `ppcs->palette_level`
    pub palette_level: u8,
    /// `ppcs->gm_ctrls.enabled`
    pub gm_enabled: u8,
}

/// The subset of `ModeDecisionContext` that
/// `svt_aom_sig_deriv_enc_dec_default` writes and this lane has ported.
///
/// Deliberately EXCLUDED, each because it comes from a table this lane has not
/// ported and would otherwise be a guess rather than a translation:
/// `nsq_search_ctrls`, `nic_ctrls`, `cand_reduction_ctrls`, `txt_ctrls`,
/// `uv_ctrls`, `cfl_ctrls`, `rdoq_ctrls`, `txs_ctrls`, `filter_intra_ctrls`,
/// `rate_est_ctrls`, `intra_ctrls`, `mds0_ctrls`. `blk_skip_decision` and
/// `redundant_blk` are derived from two of those (`uv_ctrls.uv_mode` and
/// `nsq_geom_ctrls.allow_HVA_HVB`), so they are exposed as functions taking
/// those inputs rather than being folded in here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncDecDefaultSignals {
    /// `ctx->subres_ctrls` — always level 0 on this arm.
    pub subres: SubresCtrls,
    /// `ctx->pf_ctrls` — always level 1 on this arm.
    pub pf: PfCtrls,
    /// `ctx->spatial_sse_ctrls`
    pub spatial_sse: SpatialSseCtrls,
    /// `ctx->tx_shortcut_ctrls`
    pub tx_shortcut: TxShortcutCtrls,
    /// `ctx->coeff_shaving_ctrls`
    pub coeff_shaving: CoeffShavingCtrls,
    /// `ctx->depth_early_exit_ctrls`
    pub depth_early_exit: DepthEarlyExitCtrls,
    /// `ctx->skip_sub_depth_ctrls`
    pub skip_sub_depth: SkipSubDepthCtrls,
    /// `ctx->md_nsq_me_ctrls`
    pub md_nsq_me: MdNsqMotionSearchCtrls,
    /// `ctx->md_sq_me_ctrls`
    pub md_sq_me: MdSqMotionSearchCtrls,
    /// `ctx->md_subpel_me_ctrls`
    pub md_subpel_me: MdSubPelSearchCtrls,
    /// `ctx->md_subpel_pme_ctrls`
    pub md_subpel_pme: MdSubPelSearchCtrls,
    /// `ctx->obmc_ctrls`
    pub obmc: ObmcControls,
    /// `ctx->inter_intra_comp_ctrls`
    pub inter_intra: InterIntraCompCtrls,
    /// `ctx->inter_comp_ctrls`
    pub inter_comp: InterCompCtrls,
    /// `ctx->ifs_ctrls.level`
    pub ifs_level: IfsLevel,
    /// `ctx->wm_ctrls`
    pub wm: WmCtrls,
    /// `ctx->bipred3x3_ctrls`
    pub bipred3x3: Bipred3x3Controls,
    /// `ctx->ref_pruning_ctrls`
    pub ref_pruning: RefPruningControls,
    /// `ctx->md_pme_ctrls`
    pub md_pme: MdPmeCtrls,
    /// `ctx->global_mv_injection` — copied from `ppcs->gm_ctrls.enabled`.
    pub global_mv_injection: u8,
    /// `ctx->new_nearest_injection` — hardcoded 1 on this arm.
    pub new_nearest_injection: u8,
    /// `ctx->new_nearest_near_comb_injection`
    pub new_nearest_near_comb_injection: u8,
    /// `ctx->unipred3x3_injection`
    pub unipred3x3_injection: u8,
    /// `ctx->md_allow_intrabc`
    pub md_allow_intrabc: u8,
    /// `ctx->md_palette_level`
    pub md_palette_level: u8,
    /// `ctx->approx_inter_rate`
    pub approx_inter_rate: u8,
    /// `ctx->shut_fast_rate` — hardcoded false on this arm.
    pub shut_fast_rate: bool,
    /// `ctx->mds0_use_hadamard_sb` — hardcoded FALSE on the video arm; the
    /// allintra twin sets it true, which is one of the ~15 places the two arms
    /// diverge.
    pub mds0_use_hadamard_sb: bool,
    /// `ctx->parent_cost_bias` — hardcoded 995 on this arm.
    pub parent_cost_bias: u16,
    /// `ctx->tune_ssim_level`
    pub tune_ssim_level: u8,
}

/// C `svt_aom_sig_deriv_enc_dec_default` (`enc_mode_config.c:7815`). EXPORTED.
///
/// Returns `None` when any input level is outside the range its C table
/// accepts (where C would `assert(0)`).
#[must_use]
pub fn sig_deriv_enc_dec_default(i: EncDecDefaultInputs) -> Option<EncDecDefaultSignals> {
    let derived = enc_dec_default_derived_levels(i.enc_mode);
    Some(EncDecDefaultSignals {
        // C calls set_subres_controls(ctx, 0) and set_pf_controls(ctx, 1)
        // unconditionally on this arm.
        subres: set_subres_controls(0)?,
        pf: set_pf_controls(1)?,
        spatial_sse: set_spatial_sse_full_loop_level(i.spatial_sse_full_loop_level)?,
        tx_shortcut: set_tx_shortcut_ctrls(i.tx_shortcut_level, !i.is_leaf, i.enc_mode)?,
        coeff_shaving: set_coeff_shaving_controls(i.coeff_shaving_level)?,
        depth_early_exit: set_depth_early_exit_ctrls(derived.depth_early_exit_lvl)?,
        skip_sub_depth: set_skip_sub_depth_ctrls(derived.skip_sub_depth_lvl)?,
        md_nsq_me: md_nsq_motion_search_controls(i.md_nsq_mv_search_level)?,
        md_sq_me: md_sq_motion_search_controls(i.md_sq_mv_search_level)?,
        md_subpel_me: md_subpel_me_controls(i.me_subpel_level)?,
        md_subpel_pme: md_subpel_pme_controls(i.pme_subpel_level)?,
        obmc: set_obmc_controls(i.pic_obmc_level),
        inter_intra: set_inter_intra_ctrls(i.inter_intra_level)?,
        inter_comp: set_inter_comp_controls(i.inter_compound_mode)?,
        ifs_level: set_interpolation_search_level_ctrls(i.interpolation_search_level)?,
        wm: set_wm_controls(i.wm_level)?,
        bipred3x3: set_bipred3x3_controls(i.bipred3x3_injection)?,
        ref_pruning: set_dist_based_ref_pruning_controls(i.dist_based_ref_pruning)?,
        md_pme: md_pme_search_controls(i.md_pme_level)?,
        global_mv_injection: i.gm_enabled,
        new_nearest_injection: 1,
        new_nearest_near_comb_injection: i.new_nearest_near_comb_injection,
        unipred3x3_injection: i.unipred3x3_injection,
        md_allow_intrabc: i.allow_intrabc,
        md_palette_level: i.palette_level,
        approx_inter_rate: i.approx_inter_rate,
        shut_fast_rate: false,
        mds0_use_hadamard_sb: false,
        parent_cost_bias: 995,
        tune_ssim_level: SSIM_LVL_0,
    })
}

/// C `ctx->blk_skip_decision` (`enc_mode_config.c:7859`).
///
/// Takes `uv_mode` as an input because it comes from
/// `svt_aom_set_chroma_controls`, a table this lane has NOT ported.
#[must_use]
pub fn blk_skip_decision(uv_mode: u8) -> bool {
    uv_mode <= chroma_mode::FAST
}

/// C `ctx->redundant_blk` (`enc_mode_config.c:7871`).
///
/// Takes `allow_hva_hvb` as an input because it comes from
/// `svt_aom_set_nsq_geom_ctrls`, called by `svt_aom_sig_deriv_enc_dec_common`
/// and not ported in this module.
#[must_use]
pub fn redundant_blk(allow_hva_hvb: bool) -> bool {
    allow_hva_hvb
}
