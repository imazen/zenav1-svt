//! `level -> controls` tables from `Codec/enc_mode_config.c`.
//!
//! Each function here mirrors one C `set_*_controls` / `svt_aom_set_*` switch
//! and returns the struct C would have written into
//! `ModeDecisionContext` / `PictureParentControlSet`. The port previously
//! carried the RESOLVED allintra values for a few of these inlined as per-preset
//! constants; these are the tables themselves, so the video (`_default`) arm
//! can reach the levels the still path never selects.
//!
//! Where a C `default:` arm is `assert(0)` the Rust returns `None`, so an
//! out-of-range level is a caller error rather than a silently plausible
//! control set. Where C's `default:` writes a real value (`set_obmc_controls`
//! disables OBMC), the port reproduces that value.

use super::ResolutionRange;

/// C `TOT_INTER_GROUP` (`md_process.h:79`) — the length of the per-group
/// arrays in [`RefPruningControls`].
pub const TOT_INTER_GROUP: usize = 11;

/// C `InterCandGroup` (`md_process.h:65`) — the index space of
/// [`RefPruningControls::max_dev_to_best`] and `closest_refs`.
pub mod inter_cand_group {
    /// `PA_ME_GROUP`
    pub const PA_ME: usize = 0;
    /// `UNI_3x3_GROUP`
    pub const UNI_3X3: usize = 1;
    /// `BI_3x3_GROUP`
    pub const BI_3X3: usize = 2;
    /// `NRST_NEW_NEAR_GROUP`
    pub const NRST_NEW_NEAR: usize = 3;
    /// `NRST_NEAR_GROUP`
    pub const NRST_NEAR: usize = 4;
    /// `PRED_ME_GROUP`
    pub const PRED_ME: usize = 5;
    /// `GLOBAL_GROUP`
    pub const GLOBAL: usize = 6;
    /// `WARP_GROUP`
    pub const WARP: usize = 7;
    /// `OBMC_GROUP`
    pub const OBMC: usize = 8;
    /// `INTER_INTRA_GROUP`
    pub const INTER_INTRA: usize = 9;
    /// `INTER_COMP_GROUP`
    pub const INTER_COMP: usize = 10;
}

/// C `MAX_WARP_LVL` (`enc_mode_config.c:4395`).
pub const MAX_WARP_LVL: u8 = 4;

/// C `MIN_SIGNED_VALUE` (`Codec/utility.h:164`) — `~0 - (int)(~0u >> 1)`.
pub const MIN_SIGNED_VALUE: i32 = i32::MIN;
/// C `MAX_SIGNED_VALUE` (`Codec/utility.h:165`) — `(int)(~0u >> 1)`.
pub const MAX_SIGNED_VALUE: i32 = i32::MAX;

// ---------------------------------------------------------------------------
// Warped motion
// ---------------------------------------------------------------------------

/// C `WmCtrls` (`md_process.h:619`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WmCtrls {
    /// `enabled`
    pub enabled: u8,
    /// `use_wm_for_mvp`
    pub use_wm_for_mvp: u8,
    /// `refinement_iterations`
    pub refinement_iterations: u8,
    /// `refine_diag`
    pub refine_diag: u8,
    /// `refine_level`
    pub refine_level: u8,
    /// `lower_band_th`
    pub lower_band_th: u16,
    /// `upper_band_th`
    pub upper_band_th: u16,
    /// `shut_approx_if_not_mds0`
    pub shut_approx_if_not_mds0: u8,
}

/// C `svt_aom_set_wm_controls` (`enc_mode_config.c:4397`). EXPORTED.
///
/// NOTE level 0 writes ONLY `enabled`; every other field keeps whatever the
/// context already held. The port models that by starting from
/// `WmCtrls::default()` (a zeroed context, which is what C's `calloc`'d
/// `ModeDecisionContext` supplies before the first call).
#[must_use]
pub fn set_wm_controls(wm_level: u8) -> Option<WmCtrls> {
    let mut c = WmCtrls::default();
    match wm_level {
        0 => c.enabled = 0,
        1 => {
            c.enabled = 1;
            c.use_wm_for_mvp = 1;
            c.refinement_iterations = 16;
            c.refine_diag = 1;
            c.refine_level = 0;
            c.lower_band_th = 0;
            c.upper_band_th = u16::MAX;
            c.shut_approx_if_not_mds0 = 0;
        }
        2 => {
            c.enabled = 1;
            c.use_wm_for_mvp = 1;
            c.refinement_iterations = 8;
            c.refine_diag = 0;
            c.refine_level = 1;
            c.lower_band_th = 0;
            c.upper_band_th = u16::MAX;
            c.shut_approx_if_not_mds0 = 1;
        }
        3 => {
            c.enabled = 1;
            c.use_wm_for_mvp = 1;
            c.refinement_iterations = 8;
            c.refine_diag = 0;
            c.refine_level = 1;
            c.lower_band_th = 1 << 10;
            c.upper_band_th = u16::MAX;
            c.shut_approx_if_not_mds0 = 1;
        }
        MAX_WARP_LVL => {
            c.enabled = 1;
            c.use_wm_for_mvp = 0;
            c.refinement_iterations = 0;
            c.refine_diag = 0;
            c.refine_level = 1;
            c.lower_band_th = 0;
            c.upper_band_th = u16::MAX;
            c.shut_approx_if_not_mds0 = 0;
        }
        _ => return None,
    }
    Some(c)
}

// ---------------------------------------------------------------------------
// Bi-predictive 3x3 refinement
// ---------------------------------------------------------------------------

/// C `Bipred3x3Controls` (`md_process.h:190`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Bipred3x3Controls {
    /// `enabled`
    pub enabled: u8,
    /// `search_diag`
    pub search_diag: u8,
    /// `use_best_list`
    pub use_best_list: u8,
    /// `use_l0_l1_dev`
    pub use_l0_l1_dev: u8,
}

/// C `svt_aom_set_bipred3x3_controls` (`enc_mode_config.c:5869`). EXPORTED.
#[must_use]
pub fn set_bipred3x3_controls(bipred3x3_injection: u8) -> Option<Bipred3x3Controls> {
    let mut c = Bipred3x3Controls::default();
    match bipred3x3_injection {
        0 => c.enabled = 0,
        1 => {
            c.enabled = 1;
            c.search_diag = 1;
            c.use_best_list = 0;
            c.use_l0_l1_dev = u8::MAX;
        }
        2 => {
            c.enabled = 1;
            c.search_diag = 0;
            c.use_best_list = 0;
            c.use_l0_l1_dev = u8::MAX;
        }
        3 => {
            c.enabled = 1;
            c.search_diag = 0;
            c.use_best_list = 1;
            c.use_l0_l1_dev = u8::MAX;
        }
        4 => {
            c.enabled = 1;
            c.search_diag = 0;
            c.use_best_list = 1;
            c.use_l0_l1_dev = 20;
        }
        _ => return None,
    }
    Some(c)
}

// ---------------------------------------------------------------------------
// Distance-based reference pruning
// ---------------------------------------------------------------------------

/// C `RefPruningControls` (`md_process.h:202`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefPruningControls {
    /// `enabled`
    pub enabled: u8,
    /// `max_dev_to_best[TOT_INTER_GROUP]`
    pub max_dev_to_best: [u32; TOT_INTER_GROUP],
    /// `use_tpl_info_offset`
    pub use_tpl_info_offset: u8,
    /// `check_closest_multiplier`
    pub check_closest_multiplier: u8,
    /// `closest_refs[TOT_INTER_GROUP]`
    pub closest_refs: [u8; TOT_INTER_GROUP],
}

impl Default for RefPruningControls {
    fn default() -> Self {
        Self {
            enabled: 0,
            max_dev_to_best: [0; TOT_INTER_GROUP],
            use_tpl_info_offset: 0,
            check_closest_multiplier: 0,
            closest_refs: [0; TOT_INTER_GROUP],
        }
    }
}

/// C `svt_aom_set_dist_based_ref_pruning_controls` (`enc_mode_config.c:5906`).
/// EXPORTED.
///
/// The C body writes each group's entry by name; the arrays below are in the
/// `InterCandGroup` declaration order (`md_process.h:65`), so index 0 is
/// `PA_ME_GROUP` and index 10 is `INTER_COMP_GROUP`. `u32::MAX` is C's
/// `(uint32_t)~0`, i.e. "no cap".
#[must_use]
pub fn set_dist_based_ref_pruning_controls(level: u8) -> Option<RefPruningControls> {
    const NO_CAP: u32 = u32::MAX;
    let mut c = RefPruningControls::default();
    // Every enabled level sets closest_refs to all-ones.
    let all_closest = [1u8; TOT_INTER_GROUP];
    match level {
        0 => {
            c.enabled = 0;
            return Some(c);
        }
        1 => {
            c.enabled = 1;
            c.max_dev_to_best = [NO_CAP; TOT_INTER_GROUP];
            c.use_tpl_info_offset = 0;
            c.check_closest_multiplier = 0;
        }
        2 => {
            c.enabled = 1;
            c.max_dev_to_best = [
                NO_CAP, NO_CAP, NO_CAP, NO_CAP, NO_CAP, 150, NO_CAP, NO_CAP, NO_CAP, NO_CAP, NO_CAP,
            ];
            c.use_tpl_info_offset = 0;
            c.check_closest_multiplier = 0;
        }
        3 => {
            c.enabled = 1;
            c.max_dev_to_best = [
                30, 30, 30, NO_CAP, NO_CAP, 60, NO_CAP, NO_CAP, NO_CAP, 30, 30,
            ];
            c.use_tpl_info_offset = 0;
            c.check_closest_multiplier = 0;
        }
        4 => {
            c.enabled = 1;
            c.max_dev_to_best = [30, 30, 30, 30, 30, 30, NO_CAP, NO_CAP, NO_CAP, 30, 30];
            c.use_tpl_info_offset = 20;
            c.check_closest_multiplier = 1;
        }
        // Levels 5 and 6 have identical bodies in v4.2.0; kept as separate arms
        // so an upstream edit to one lands where it belongs.
        5 | 6 => {
            c.enabled = 1;
            c.max_dev_to_best = [30, 0, 30, 30, 30, 30, NO_CAP, NO_CAP, NO_CAP, 30, 0];
            c.use_tpl_info_offset = 20;
            c.check_closest_multiplier = 1;
        }
        7 => {
            c.enabled = 1;
            c.max_dev_to_best = [10, 0, 10, 10, 10, 10, NO_CAP, NO_CAP, 10, 10, 0];
            c.use_tpl_info_offset = 20;
            c.check_closest_multiplier = 1;
        }
        8 => {
            c.enabled = 1;
            c.max_dev_to_best = [0, 0, 0, 0, 0, 0, NO_CAP, 0, 0, 0, 0];
            c.use_tpl_info_offset = 20;
            c.check_closest_multiplier = 1;
        }
        _ => return None,
    }
    c.closest_refs = all_closest;
    Some(c)
}

// ---------------------------------------------------------------------------
// Predictive-ME search in MD
// ---------------------------------------------------------------------------

/// C `DistortionType` (`definitions.h:892`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum DistortionType {
    /// `SAD`
    #[default]
    Sad = 0,
    /// `VAR`
    Var = 1,
    /// `SSD`
    Ssd = 2,
}

/// C `MdPmeCtrls` (`md_process.h:354`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MdPmeCtrls {
    /// `enabled`
    pub enabled: u8,
    /// `dist_type`
    pub dist_type: DistortionType,
    /// `early_check_mv_th_multiplier`
    pub early_check_mv_th_multiplier: i32,
    /// `full_pel_search_width`
    pub full_pel_search_width: u8,
    /// `full_pel_search_height`
    pub full_pel_search_height: u8,
    /// `pre_fp_pme_to_me_cost_th`
    pub pre_fp_pme_to_me_cost_th: i32,
    /// `pre_fp_pme_to_me_mv_th`
    pub pre_fp_pme_to_me_mv_th: i32,
    /// `post_fp_pme_to_me_cost_th`
    pub post_fp_pme_to_me_cost_th: i32,
    /// `post_fp_pme_to_me_mv_th`
    pub post_fp_pme_to_me_mv_th: i32,
    /// `enable_psad`
    pub enable_psad: u8,
    /// `sa_q_weight`
    pub sa_q_weight: u8,
}

/// C `svt_aom_md_pme_search_controls` (`enc_mode_config.c:3310`). EXPORTED.
#[must_use]
pub fn md_pme_search_controls(md_pme_level: u8) -> Option<MdPmeCtrls> {
    let mut c = MdPmeCtrls::default();
    match md_pme_level {
        0 => c.enabled = 0,
        1 | 2 => {
            c.enabled = 1;
            c.dist_type = DistortionType::Var;
            c.full_pel_search_width = 9;
            c.full_pel_search_height = 9;
            c.early_check_mv_th_multiplier = MIN_SIGNED_VALUE;
            c.pre_fp_pme_to_me_cost_th = MAX_SIGNED_VALUE;
            c.pre_fp_pme_to_me_mv_th = MIN_SIGNED_VALUE;
            c.post_fp_pme_to_me_cost_th = MAX_SIGNED_VALUE;
            c.post_fp_pme_to_me_mv_th = MIN_SIGNED_VALUE;
            c.enable_psad = 0;
            // The ONLY difference between levels 1 and 2.
            c.sa_q_weight = u8::from(md_pme_level == 2);
        }
        3 => {
            c.enabled = 1;
            c.dist_type = DistortionType::Var;
            c.full_pel_search_width = 9;
            c.full_pel_search_height = 7;
            c.early_check_mv_th_multiplier = MIN_SIGNED_VALUE;
            c.pre_fp_pme_to_me_cost_th = MAX_SIGNED_VALUE;
            c.pre_fp_pme_to_me_mv_th = 16;
            c.post_fp_pme_to_me_cost_th = 50;
            c.post_fp_pme_to_me_mv_th = MIN_SIGNED_VALUE;
            c.enable_psad = 0;
            c.sa_q_weight = 1;
        }
        4 | 5 => {
            c.enabled = 1;
            c.dist_type = DistortionType::Sad;
            c.full_pel_search_width = 7;
            c.full_pel_search_height = 5;
            // The ONLY difference between levels 4 and 5.
            c.early_check_mv_th_multiplier = if md_pme_level == 5 {
                64
            } else {
                MIN_SIGNED_VALUE
            };
            c.pre_fp_pme_to_me_cost_th = 25;
            c.pre_fp_pme_to_me_mv_th = 16;
            c.post_fp_pme_to_me_cost_th = 50;
            c.post_fp_pme_to_me_mv_th = 32;
            c.enable_psad = 1;
            c.sa_q_weight = 1;
        }
        _ => return None,
    }
    Some(c)
}

// ---------------------------------------------------------------------------
// Global motion
// ---------------------------------------------------------------------------

/// C `TransformationType` (`definitions.h:1755`).
pub mod transformation_type {
    /// `IDENTITY`
    pub const IDENTITY: u8 = 0;
    /// `TRANSLATION`
    pub const TRANSLATION: u8 = 1;
    /// `ROTZOOM`
    pub const ROTZOOM: u8 = 2;
    /// `AFFINE`
    pub const AFFINE: u8 = 3;
}

/// C `GM_LEVEL` (`definitions.h:256`) — the GM downsampling mode.
pub mod gm_downsample {
    /// `GM_FULL`
    pub const FULL: u8 = 0;
}

/// C `CorrespondenceMethod` (`pcs.h:504`).
///
/// Modelled as a `u8` rather than a Rust enum ON PURPOSE: the enum's values
/// start at 1, and `set_gm_controls`'s level-0 arm never assigns the field, so
/// a `calloc`'d context leaves it at **0** — a value outside the enum that C's
/// post-switch `< CORNERS` test then reads. A Rust enum could not hold it.
pub mod correspondence_method {
    /// `MV_32x32`
    pub const MV_32X32: u8 = 1;
    /// `MV_16x16`
    pub const MV_16X16: u8 = 2;
    /// `MV_8x8`
    pub const MV_8X8: u8 = 3;
    /// `CORNERS`
    pub const CORNERS: u8 = 4;
}

/// C `GmControls` (`pcs.h:512`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GmControls {
    /// `enabled`
    pub enabled: u8,
    /// `identiy_exit` (C's spelling)
    pub identiy_exit: u8,
    /// `search_start_model`
    pub search_start_model: u8,
    /// `search_end_model`
    pub search_end_model: u8,
    /// `skip_identity`
    pub skip_identity: u8,
    /// `bypass_based_on_me`
    pub bypass_based_on_me: u8,
    /// `params_refinement_steps`
    pub params_refinement_steps: u8,
    /// `downsample_level`
    pub downsample_level: u8,
    /// `corners`
    pub corners: u8,
    /// `chess_rfn`
    pub chess_rfn: u8,
    /// `match_sz`
    pub match_sz: u8,
    /// `inj_psq_glb`
    pub inj_psq_glb: bool,
    /// `pp_enabled`
    pub pp_enabled: bool,
    /// `ref_idx0_only`
    pub ref_idx0_only: bool,
    /// `rfn_early_exit`
    pub rfn_early_exit: u8,
    /// `correspondence_method` — see [`correspondence_method`] for the values;
    /// 0 is the zeroed-context value C's level-0 arm leaves in place.
    pub correspondence_method: u8,
}

/// C `svt_aom_set_gm_controls` (`enc_mode_config.c:2491`). EXPORTED.
///
/// Level 0 writes ONLY `enabled` and `pp_enabled`, so the other fields keep the
/// context's prior contents — modelled from a zeroed `GmControls`, which is
/// what a `calloc`'d PPCS carries. The post-switch `correspondence_method <
/// CORNERS` clamp of `pp_enabled` is applied here too: a zeroed
/// `correspondence_method` (0) is BELOW `CORNERS`, so on the level-0 arm C
/// clears `pp_enabled` a second time.
#[must_use]
pub fn set_gm_controls(gm_level: u8, input_resolution: ResolutionRange) -> Option<GmControls> {
    let mut c = GmControls::default();
    // The MV-based correspondence method for levels 3 and 4.
    let mv_method = if input_resolution <= ResolutionRange::R480p {
        correspondence_method::MV_8X8
    } else if input_resolution <= ResolutionRange::R1080p {
        correspondence_method::MV_16X16
    } else {
        correspondence_method::MV_32X32
    };
    match gm_level {
        0 => {
            // C writes ONLY these two; every other field (correspondence_method
            // included) keeps the zeroed context value.
            c.enabled = 0;
            c.pp_enabled = false;
        }
        1 => {
            c.enabled = 1;
            c.identiy_exit = 0;
            c.search_start_model = transformation_type::TRANSLATION;
            c.search_end_model = transformation_type::AFFINE;
            c.skip_identity = 0;
            c.bypass_based_on_me = 0;
            c.params_refinement_steps = 5;
            c.downsample_level = gm_downsample::FULL;
            c.corners = 4;
            c.chess_rfn = 0;
            c.match_sz = 13;
            c.inj_psq_glb = false;
            c.pp_enabled = false;
            c.ref_idx0_only = false;
            c.rfn_early_exit = 0;
            c.correspondence_method = correspondence_method::CORNERS;
        }
        2 => {
            c.enabled = 1;
            c.identiy_exit = 1;
            c.search_start_model = transformation_type::TRANSLATION;
            c.search_end_model = transformation_type::ROTZOOM;
            c.skip_identity = 0;
            c.bypass_based_on_me = 0;
            c.params_refinement_steps = 5;
            c.downsample_level = gm_downsample::FULL;
            c.corners = 2;
            c.chess_rfn = 0;
            c.match_sz = 7;
            c.inj_psq_glb = false;
            c.pp_enabled = true;
            c.ref_idx0_only = false;
            c.rfn_early_exit = 0;
            c.correspondence_method = correspondence_method::CORNERS;
        }
        3 | 4 => {
            c.enabled = 1;
            c.identiy_exit = 1;
            c.search_start_model = transformation_type::TRANSLATION;
            c.search_end_model = transformation_type::ROTZOOM;
            // The ONLY difference between levels 3 and 4.
            c.skip_identity = u8::from(gm_level == 4);
            c.bypass_based_on_me = 1;
            c.params_refinement_steps = 5;
            c.downsample_level = gm_downsample::FULL;
            c.corners = 2;
            c.chess_rfn = 1;
            c.match_sz = 7;
            c.inj_psq_glb = true;
            c.pp_enabled = false;
            c.ref_idx0_only = true;
            c.rfn_early_exit = 1;
            c.correspondence_method = mv_method;
        }
        _ => return None,
    }
    // C: MV-based correspondence relies on ME info, unavailable in the
    // pre-processor stage. This ALSO fires on the level-0 arm, where the
    // zeroed correspondence_method (0) is below CORNERS.
    if c.correspondence_method < correspondence_method::CORNERS {
        c.pp_enabled = false;
    }
    // C asserts an odd match_sz for every enabled level.
    debug_assert!(gm_level == 0 || (c.match_sz & 1) == 1);
    Some(c)
}

// ---------------------------------------------------------------------------
// OBMC
// ---------------------------------------------------------------------------

/// C `ObmcControls` (`md_process.h:123`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ObmcControls {
    /// `enabled`
    pub enabled: u8,
    /// `max_blk_size_to_refine`
    pub max_blk_size_to_refine: u8,
    /// `max_blk_size`
    pub max_blk_size: u8,
    /// `refine_level`
    pub refine_level: u8,
    /// `trans_face_off`
    pub trans_face_off: u8,
    /// `fpel_search_range`
    pub fpel_search_range: u8,
    /// `fpel_search_diag`
    pub fpel_search_diag: u8,
}

/// C `set_obmc_controls` (`enc_mode_config.c:2878`). static — tier 4.
///
/// C's `default:` arm is NOT `assert(0)` here: it disables OBMC, so this
/// returns a value for every `u8` rather than `Option`.
#[must_use]
pub fn set_obmc_controls(obmc_mode: u8) -> ObmcControls {
    let mut c = ObmcControls::default();
    match obmc_mode {
        1 => {
            c.enabled = 1;
            c.max_blk_size_to_refine = 128;
            c.max_blk_size = 128;
            c.refine_level = 0;
            c.trans_face_off = 0;
            c.fpel_search_range = 16;
            c.fpel_search_diag = 1;
        }
        2 => {
            c.enabled = 1;
            c.max_blk_size_to_refine = 64;
            c.max_blk_size = 128;
            c.refine_level = 1;
            c.trans_face_off = 0;
            c.fpel_search_range = 16;
            c.fpel_search_diag = 1;
        }
        3 => {
            c.enabled = 1;
            c.max_blk_size_to_refine = 32;
            c.max_blk_size = 128;
            c.refine_level = 1;
            c.trans_face_off = 0;
            c.fpel_search_range = 8;
            c.fpel_search_diag = 0;
        }
        4 => {
            c.enabled = 1;
            c.max_blk_size_to_refine = 32;
            c.max_blk_size = 128;
            c.refine_level = 1;
            c.trans_face_off = 1;
            c.fpel_search_range = 16;
            c.fpel_search_diag = 1;
        }
        5 => {
            c.enabled = 1;
            c.max_blk_size_to_refine = 32;
            c.max_blk_size = 32;
            c.refine_level = 4;
            c.trans_face_off = 1;
            c.fpel_search_range = 8;
            c.fpel_search_diag = 0;
        }
        6 => {
            c.enabled = 1;
            c.max_blk_size_to_refine = 16;
            c.max_blk_size = 16;
            c.refine_level = 4;
            c.trans_face_off = 1;
            c.fpel_search_range = 8;
            c.fpel_search_diag = 0;
        }
        // C: `case 0` and `default` both just clear `enabled`.
        _ => c.enabled = 0,
    }
    c
}

// ---------------------------------------------------------------------------
// Inter-inter compound
// ---------------------------------------------------------------------------

/// C `MD_COMP_TYPE` (`definitions.h:1285`).
pub mod md_comp {
    /// `MD_COMP_AVG`
    pub const AVG: u8 = 0;
    /// `MD_COMP_DIST`
    pub const DIST: u8 = 1;
    /// `MD_COMP_DIFF0`
    pub const DIFF0: u8 = 2;
    /// `MD_COMP_WEDGE`
    pub const WEDGE: u8 = 3;
    /// `MD_COMP_TYPES` — used as a COUNT by `set_inter_comp_controls`.
    pub const TYPES: u8 = 4;
}

/// C `InterCompCtrls` (`md_process.h:82`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InterCompCtrls {
    /// `tot_comp_types`
    pub tot_comp_types: u8,
    /// `do_me`
    pub do_me: bool,
    /// `do_pme`
    pub do_pme: bool,
    /// `do_nearest_nearest`
    pub do_nearest_nearest: bool,
    /// `do_near_near`
    pub do_near_near: bool,
    /// `do_nearest_near_new`
    pub do_nearest_near_new: bool,
    /// `do_3x3_bi`
    pub do_3x3_bi: bool,
    /// `do_global`
    pub do_global: bool,
    /// `pred0_to_pred1_mult`
    pub pred0_to_pred1_mult: u8,
    /// `max_mv_length`
    pub max_mv_length: u16,
    /// `skip_on_ref_info`
    pub skip_on_ref_info: bool,
    /// `use_rate`
    pub use_rate: bool,
    /// `no_sym_dist`
    pub no_sym_dist: bool,
}

/// C `set_inter_comp_controls` (`enc_mode_config.c:2589`). static — tier 4.
///
/// Level 0 is "AVG only": it sets `tot_comp_types = MD_COMP_DIST` (1) and
/// leaves the five `skip_on_ref_info`..`no_sym_dist` fields untouched, so the
/// port returns the zeroed values a `calloc`'d context holds.
#[must_use]
pub fn set_inter_comp_controls(inter_comp_mode: u8) -> Option<InterCompCtrls> {
    let mut c = InterCompCtrls::default();
    match inter_comp_mode {
        0 => {
            c.tot_comp_types = md_comp::DIST;
            c.do_nearest_nearest = false;
            c.do_near_near = false;
            c.do_me = false;
            c.do_pme = false;
            c.do_nearest_near_new = false;
            c.do_3x3_bi = false;
            c.do_global = false;
        }
        1 => {
            c.tot_comp_types = md_comp::TYPES;
            c.do_nearest_nearest = true;
            c.do_near_near = true;
            c.do_me = true;
            c.do_pme = true;
            c.do_nearest_near_new = true;
            c.do_3x3_bi = true;
            c.do_global = true;
            c.skip_on_ref_info = false;
            c.use_rate = true;
            c.pred0_to_pred1_mult = 0;
            c.max_mv_length = 0;
            c.no_sym_dist = false;
        }
        2 => {
            c.tot_comp_types = md_comp::TYPES;
            c.do_nearest_nearest = true;
            c.do_near_near = true;
            c.do_me = true;
            c.do_pme = true;
            c.do_nearest_near_new = false;
            c.do_3x3_bi = false;
            c.do_global = true;
            c.skip_on_ref_info = false;
            c.use_rate = false;
            c.pred0_to_pred1_mult = 1;
            c.max_mv_length = 0;
            c.no_sym_dist = false;
        }
        3 => {
            c.tot_comp_types = md_comp::TYPES;
            c.do_nearest_nearest = true;
            c.do_near_near = true;
            c.do_me = true;
            c.do_pme = false;
            c.do_nearest_near_new = false;
            c.do_3x3_bi = false;
            c.do_global = true;
            c.skip_on_ref_info = true;
            c.use_rate = false;
            c.pred0_to_pred1_mult = 1;
            c.max_mv_length = 0;
            c.no_sym_dist = true;
        }
        4 => {
            c.tot_comp_types = md_comp::TYPES;
            c.do_nearest_nearest = true;
            c.do_near_near = true;
            c.do_me = true;
            c.do_pme = false;
            c.do_nearest_near_new = false;
            c.do_3x3_bi = false;
            c.do_global = true;
            c.skip_on_ref_info = true;
            c.use_rate = false;
            c.pred0_to_pred1_mult = 4;
            c.max_mv_length = 32;
            c.no_sym_dist = true;
        }
        _ => return None,
    }
    Some(c)
}

// ---------------------------------------------------------------------------
// Inter-intra
// ---------------------------------------------------------------------------

/// C `InterIntraCompCtrls` (`md_process.h:113`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InterIntraCompCtrls {
    /// `enabled`
    pub enabled: u8,
    /// `use_rd_model`
    pub use_rd_model: u8,
    /// `wedge_mode_sq`
    pub wedge_mode_sq: u8,
    /// `wedge_mode_nsq`
    pub wedge_mode_nsq: u8,
}

/// C `set_inter_intra_ctrls` (`enc_mode_config.c:5385`). static — tier 4.
#[must_use]
pub fn set_inter_intra_ctrls(inter_intra_level: u8) -> Option<InterIntraCompCtrls> {
    match inter_intra_level {
        0 => Some(InterIntraCompCtrls {
            enabled: 0,
            use_rd_model: 0,
            wedge_mode_sq: 0,
            wedge_mode_nsq: 0,
        }),
        1 => Some(InterIntraCompCtrls {
            enabled: 1,
            use_rd_model: 1,
            wedge_mode_sq: 1,
            wedge_mode_nsq: 1,
        }),
        2 => Some(InterIntraCompCtrls {
            enabled: 1,
            use_rd_model: 0,
            wedge_mode_sq: 0,
            wedge_mode_nsq: 2,
        }),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Interpolation-filter search
// ---------------------------------------------------------------------------

/// C `IfsLevel` (`definitions.h:877`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum IfsLevel {
    /// `IFS_OFF`
    #[default]
    Off = 0,
    /// `IFS_MDS0`
    Mds0 = 1,
    /// `IFS_MDS1`
    Mds1 = 2,
    /// `IFS_MDS2`
    Mds2 = 3,
    /// `IFS_MDS3`
    Mds3 = 4,
}

/// C `set_interpolation_search_level_ctrls` (`enc_mode_config.c:4069`).
/// static — tier 4.
///
/// The allintra arm passes 0 unconditionally; the video arm passes
/// `pcs->interpolation_search_level`, and the chosen `interp_filter` is coded
/// per inter block, so a wrong level here is bit-affecting.
#[must_use]
pub fn set_interpolation_search_level_ctrls(interpolation_search_level: u8) -> Option<IfsLevel> {
    match interpolation_search_level {
        0 => Some(IfsLevel::Off),
        1 => Some(IfsLevel::Mds0),
        2 => Some(IfsLevel::Mds1),
        3 => Some(IfsLevel::Mds2),
        4 => Some(IfsLevel::Mds3),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Deblocking loop filter
// ---------------------------------------------------------------------------

/// C `DlfCtrls` (`Codec/pcs.h:603`).
///
/// The two fields that decide WHICH level-picker runs:
/// * `enabled == 0` — no pick and no apply; the frame header codes
///   `loop_filter_level[..] = 0`.
/// * `sb_based_dlf != 0` — `enc_dec_process.c:3132` runs
///   `svt_av1_pick_filter_level(.., LPF_PICK_FROM_Q)`, the closed form
///   (`svt_av1_pick_filter_level_by_q`, `deblocking_filter.c:1055`).
/// * `sb_based_dlf == 0` — `dlf_process.c:97` runs
///   `svt_av1_pick_filter_level(.., LPF_PICK_FROM_FULL_IMAGE)`, the real
///   per-level SSE search.
///
/// The remaining fields tune that search. On an I\_SLICE
/// (`tot_ref_frame_types == 0`) `dlf_avg`, `use_ref_avg_y` and
/// `use_ref_avg_uv` are all structurally inert — every branch that reads them
/// is guarded on having reference frames (`deblocking_filter.c:1180`, `:1221`,
/// `:1264`) — and `me_based_dlf_skip` returns immediately for an I\_SLICE
/// (`:967`), which makes `zero_filter_strength_lvl` and `prev_dlf_dist_th`
/// inert too. So on a key frame only `enabled`, `sb_based_dlf` and
/// `early_exit_convergence` are read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DlfCtrls {
    /// `enabled`
    pub enabled: u8,
    /// `sb_based_dlf`
    pub sb_based_dlf: u8,
    /// `dlf_avg` — start the search from the ref-average level instead of 0.
    pub dlf_avg: bool,
    /// `use_ref_avg_y`
    pub use_ref_avg_y: bool,
    /// `use_ref_avg_uv`
    pub use_ref_avg_uv: bool,
    /// `early_exit_convergence` — 0 = off, else exit on the Nth convergence.
    pub early_exit_convergence: u8,
    /// `zero_filter_strength_lvl`
    pub zero_filter_strength_lvl: u8,
    /// `prev_dlf_dist_th`
    pub prev_dlf_dist_th: u16,
}

/// C `svt_aom_set_dlf_controls` (`enc_mode_config.c:1561`).
///
/// `static` despite the `svt_aom_` prefix, so this is **evidence tier 4**
/// (hand-derived vectors traced against the C source) — there is no exported
/// symbol to run a differential against.
///
/// C's `default:` arm is `assert(0)`, so an out-of-range level returns `None`
/// rather than a silently plausible control set.
#[must_use]
pub fn set_dlf_controls(dlf_level: u8) -> Option<DlfCtrls> {
    let mut c = DlfCtrls::default();
    match dlf_level {
        0 => {
            c.enabled = 0;
            c.sb_based_dlf = 0;
            c.dlf_avg = false;
            c.use_ref_avg_y = false;
            c.use_ref_avg_uv = false;
            c.early_exit_convergence = 0;
            c.zero_filter_strength_lvl = 0;
            c.prev_dlf_dist_th = 0;
        }
        1 => {
            c.enabled = 1;
            c.sb_based_dlf = 0;
            c.dlf_avg = false;
            c.use_ref_avg_y = false;
            c.use_ref_avg_uv = false;
            c.early_exit_convergence = 0;
            c.zero_filter_strength_lvl = 0;
            c.prev_dlf_dist_th = 0;
        }
        2 => {
            c.enabled = 1;
            c.sb_based_dlf = 0;
            c.dlf_avg = true;
            c.use_ref_avg_y = false;
            c.use_ref_avg_uv = true;
            c.early_exit_convergence = 1;
            c.zero_filter_strength_lvl = 0;
            c.prev_dlf_dist_th = 0;
        }
        3 => {
            c.enabled = 1;
            c.sb_based_dlf = 0;
            c.dlf_avg = true;
            c.use_ref_avg_y = true;
            c.use_ref_avg_uv = true;
            c.early_exit_convergence = 1;
            c.zero_filter_strength_lvl = 0;
            c.prev_dlf_dist_th = 0;
        }
        4 => {
            c.enabled = 1;
            c.sb_based_dlf = 0;
            c.dlf_avg = true;
            c.use_ref_avg_y = true;
            c.use_ref_avg_uv = true;
            c.early_exit_convergence = 1;
            c.zero_filter_strength_lvl = 2;
            c.prev_dlf_dist_th = 10;
        }
        5 => {
            c.enabled = 1;
            c.sb_based_dlf = 1;
            c.dlf_avg = false;
            c.use_ref_avg_y = false;
            c.use_ref_avg_uv = false;
            c.early_exit_convergence = 0;
            c.zero_filter_strength_lvl = 1;
            c.prev_dlf_dist_th = 0;
        }
        6 => {
            c.enabled = 1;
            c.sb_based_dlf = 1;
            c.dlf_avg = false;
            c.use_ref_avg_y = false;
            c.use_ref_avg_uv = false;
            c.early_exit_convergence = 0;
            c.zero_filter_strength_lvl = 2;
            c.prev_dlf_dist_th = 0;
        }
        7 => {
            c.enabled = 1;
            c.sb_based_dlf = 1;
            c.dlf_avg = false;
            c.use_ref_avg_y = false;
            c.use_ref_avg_uv = false;
            c.early_exit_convergence = 0;
            c.zero_filter_strength_lvl = 3;
            c.prev_dlf_dist_th = 0;
        }
        _ => return None,
    }
    Some(c)
}
