//! Motion-estimation signal derivation from `Codec/enc_mode_config.c` —
//! chunk C4's parameter surface.
//!
//! `svt_aom_sig_deriv_me` is where every search-area, HME and pruning parameter
//! the real SVT ME uses comes from. The port's existing `motion_est.rs` is
//! homegrown and does NOT read these; this module is the faithful translation
//! the wholesale ME port needs.
//!
//! `svt_aom_sig_deriv_me_tf` matters for a reason that is easy to miss: temporal
//! filtering is ON in the campaign's video config (`kf_tf_strength = 3`,
//! `docs/INTER-ENCODE-PLAN.md` 1b) and TF **rewrites the source pixels before
//! encode**, so the video-mode KEY frame's bytes depend on it.

use super::{ResolutionRange, enc_mode::*};

/// C `SearchArea` (`me_context.h:306`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SearchArea {
    /// `width`
    pub width: u16,
    /// `height`
    pub height: u16,
}

impl SearchArea {
    /// Construct from C's `(SearchArea){w, h}` designator order.
    #[must_use]
    pub const fn new(width: u16, height: u16) -> Self {
        Self { width, height }
    }
}

/// C `SearchAreaMinMax` (`me_context.h:311`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SearchAreaMinMax {
    /// `sa_min`
    pub sa_min: SearchArea,
    /// `sa_max`
    pub sa_max: SearchArea,
}

/// C `MAX_QP_VALUE` (`definitions.h:1662`).
pub const MAX_QP_VALUE: u32 = 63;
/// C `BLOCK_SIZE_64` (`definitions.h:2033`).
pub const BLOCK_SIZE_64: u32 = 64;
/// C `SUB_SAD_SEARCH` (`definitions.h:1819`).
pub const SUB_SAD_SEARCH: u8 = 0;
/// C `FULL_SAD_SEARCH` (`definitions.h:1820`).
pub const FULL_SAD_SEARCH: u8 = 1;

/// C `DIVIDE_AND_ROUND(x, y)` (`Codec/utility.h:96`) — `((x) + ((y) >> 1)) / (y)`.
///
/// NOTE this is NOT a round-half-away-from-zero: it is an unsigned add-then-
/// truncate, so it must be reproduced exactly rather than replaced with a
/// "rounded division" helper.
#[must_use]
pub const fn divide_and_round(x: u32, y: u32) -> u32 {
    (x + (y >> 1)) / y
}

/// C `svt_aom_get_qp_based_th_scaling_factors` (`enc_mode_config.c:25`).
/// EXPORTED. Returns `(q_weight, q_weight_denom)`.
///
/// Above qp 45 the weight switches from linear `qp/63` to
/// `(1.05 - exp(-(max(40, qp) - 35) / 10)) * 10000` truncated to `u32`. The
/// truncation (a C cast, not a round) is load-bearing.
#[must_use]
pub fn get_qp_based_th_scaling_factors(enable_qp_based_th_scaling: bool, qp: u32) -> (u32, u32) {
    if !enable_qp_based_th_scaling {
        return (1, 1);
    }
    // Limit scaling for low QPs to 10/63 to avoid extreme actions.
    let mut q_weight = qp.max(10);
    let mut q_weight_denom = MAX_QP_VALUE;
    if qp >= 46 {
        let ex = -((f64::from(qp).max(40.0)) - 35.0) / 10.0;
        let mut q_weight_int = ex.exp();
        q_weight_int = 1.05 - q_weight_int;
        q_weight_int *= 10000.0;
        q_weight = q_weight_int as u32;
        q_weight_denom = 10000;
    }
    (q_weight, q_weight_denom)
}

// ---------------------------------------------------------------------------
// HME / ME search areas
// ---------------------------------------------------------------------------

/// What C `set_hme_search_params` writes into the `MeContext`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HmeSearchParams {
    /// `num_hme_sa_w`
    pub num_hme_sa_w: u8,
    /// `num_hme_sa_h`
    pub num_hme_sa_h: u8,
    /// `hme_l0_sa`
    pub hme_l0_sa: SearchAreaMinMax,
    /// `hme_l1_sa`
    pub hme_l1_sa: SearchArea,
    /// `hme_l2_sa`
    pub hme_l2_sa: SearchArea,
}

/// C `set_hme_search_params` (`enc_mode_config.c:216`). static — tier 4
/// directly, reached at tier 1 through `svt_aom_sig_deriv_me`.
#[must_use]
pub fn set_hme_search_params(
    enc_mode: i8,
    input_resolution: ResolutionRange,
    rtc_tune: bool,
    hme_qp_based_th_scaling: bool,
    qp: u32,
) -> HmeSearchParams {
    let mut p = HmeSearchParams {
        num_hme_sa_w: 2,
        num_hme_sa_h: 2,
        ..HmeSearchParams::default()
    };

    if enc_mode <= M1 {
        if input_resolution < ResolutionRange::R4k {
            p.hme_l0_sa.sa_min = SearchArea::new(32, 32);
            p.hme_l0_sa.sa_max = SearchArea::new(192, 192);
        } else {
            p.hme_l0_sa.sa_min = SearchArea::new(240, 240);
            p.hme_l0_sa.sa_max = SearchArea::new(480, 480);
        }
    } else if enc_mode <= M5 {
        p.hme_l0_sa.sa_min = SearchArea::new(32, 32);
        p.hme_l0_sa.sa_max = SearchArea::new(192, 192);
    } else if !rtc_tune && enc_mode <= M7 {
        if input_resolution >= ResolutionRange::R4k {
            p.hme_l0_sa.sa_min = SearchArea::new(32, 32);
            p.hme_l0_sa.sa_max = SearchArea::new(192, 192);
        } else {
            p.hme_l0_sa.sa_min = SearchArea::new(16, 16);
            p.hme_l0_sa.sa_max = SearchArea::new(192, 192);
        }
    } else if enc_mode <= M8 {
        p.hme_l0_sa.sa_min = SearchArea::new(16, 16);
        p.hme_l0_sa.sa_max = SearchArea::new(192, 192);
    } else if input_resolution < ResolutionRange::R4k {
        p.hme_l0_sa.sa_min = SearchArea::new(8, 8);
        p.hme_l0_sa.sa_max = SearchArea::new(96, 96);
    } else {
        p.hme_l0_sa.sa_min = SearchArea::new(16, 16);
        p.hme_l0_sa.sa_max = SearchArea::new(96, 96);
    }

    // Modulate the HME search area using qp. NOTE the min floors at 8 and the
    // MAX floors at 96 — different constants, not a shared one.
    let (qw, qwd) = get_qp_based_th_scaling_factors(hme_qp_based_th_scaling, qp);
    let scale =
        |v: u16, floor: u32| -> u16 { floor.max(divide_and_round(u32::from(v) * qw, qwd)) as u16 };
    p.hme_l0_sa.sa_min.width = scale(p.hme_l0_sa.sa_min.width, 8);
    p.hme_l0_sa.sa_min.height = scale(p.hme_l0_sa.sa_min.height, 8);
    p.hme_l0_sa.sa_max.width = scale(p.hme_l0_sa.sa_max.width, 96);
    p.hme_l0_sa.sa_max.height = scale(p.hme_l0_sa.sa_max.height, 96);

    if enc_mode <= MR {
        p.hme_l1_sa = SearchArea::new(16, 16);
        p.hme_l2_sa = SearchArea::new(16, 16);
    } else {
        p.hme_l1_sa = SearchArea::new(8, 3);
        p.hme_l2_sa = SearchArea::new(8, 3);
    }
    p
}

/// C `set_me_search_params` (`enc_mode_config.c:281`). static — tier 4
/// directly, reached at tier 1 through `svt_aom_sig_deriv_me`.
#[must_use]
pub fn set_me_search_params(
    enc_mode: i8,
    input_resolution: ResolutionRange,
    rtc_tune: bool,
    me_qp_based_th_scaling: bool,
    qp: u32,
) -> SearchAreaMinMax {
    let mut sa = SearchAreaMinMax::default();
    if rtc_tune {
        if enc_mode <= M10 {
            if input_resolution < ResolutionRange::R1080p {
                sa.sa_min = SearchArea::new(24, 16);
                sa.sa_max = SearchArea::new(32, 16);
            } else {
                sa.sa_min = SearchArea::new(16, 9);
                sa.sa_max = SearchArea::new(16, 9);
            }
        } else {
            sa.sa_min = SearchArea::new(8, 1);
            sa.sa_max = SearchArea::new(8, 1);
        }
    } else if enc_mode <= MR {
        sa.sa_min = SearchArea::new(96, 96);
        sa.sa_max = SearchArea::new(256, 256);
    } else if enc_mode <= M1 {
        sa.sa_min = SearchArea::new(84, 84);
        sa.sa_max = SearchArea::new(224, 224);
    } else if enc_mode <= M3 {
        sa.sa_min = SearchArea::new(24, 24);
        sa.sa_max = SearchArea::new(88, 88);
    } else if enc_mode <= M4 {
        sa.sa_min = SearchArea::new(24, 12);
        sa.sa_max = SearchArea::new(48, 32);
    } else if enc_mode <= M7 {
        sa.sa_min = SearchArea::new(24, 9);
        sa.sa_max = SearchArea::new(24, 12);
    } else if enc_mode <= M8 {
        sa.sa_min = SearchArea::new(16, 6);
        sa.sa_max = SearchArea::new(24, 12);
    } else if enc_mode <= M11 {
        if input_resolution < ResolutionRange::R720p {
            sa.sa_min = SearchArea::new(12, 4);
            sa.sa_max = SearchArea::new(16, 9);
        } else if input_resolution < ResolutionRange::R1080p {
            sa.sa_min = SearchArea::new(12, 1);
            sa.sa_max = SearchArea::new(16, 7);
        } else if input_resolution < ResolutionRange::R4k {
            sa.sa_min = SearchArea::new(8, 1);
            sa.sa_max = SearchArea::new(8, 7);
        } else {
            sa.sa_min = SearchArea::new(8, 1);
            sa.sa_max = SearchArea::new(8, 1);
        }
    } else {
        sa.sa_min = SearchArea::new(8, 1);
        sa.sa_max = SearchArea::new(8, 1);
    }

    // NOTE the WIDTH floors at 8 and the HEIGHT at 3 — unlike the HME
    // modulation above, which floors min at 8/8 and max at 96/96.
    let (qw, qwd) = get_qp_based_th_scaling_factors(me_qp_based_th_scaling, qp);
    let scale =
        |v: u16, floor: u32| -> u16 { floor.max(divide_and_round(u32::from(v) * qw, qwd)) as u16 };
    sa.sa_min.width = scale(sa.sa_min.width, 8);
    sa.sa_min.height = scale(sa.sa_min.height, 3);
    sa.sa_max.width = scale(sa.sa_max.width, 8);
    sa.sa_max.height = scale(sa.sa_max.height, 3);
    sa
}

// ---------------------------------------------------------------------------
// ME/HME control tables
// ---------------------------------------------------------------------------

/// C `MeHmeRefPruneCtrls` (`me_context.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MeHmeRefPruneCtrls {
    /// `enable_me_hme_ref_pruning`
    pub enable_me_hme_ref_pruning: u8,
    /// `prune_ref_if_hme_sad_dev_bigger_than_th`
    pub prune_ref_if_hme_sad_dev_bigger_than_th: u16,
    /// `prune_ref_if_me_sad_dev_bigger_than_th`
    pub prune_ref_if_me_sad_dev_bigger_than_th: u16,
    /// `zz_sad_th`
    pub zz_sad_th: u32,
    /// `zz_sad_pct`
    pub zz_sad_pct: u32,
    /// `phme_sad_th`
    pub phme_sad_th: u32,
    /// `phme_sad_pct`
    pub phme_sad_pct: u32,
}

/// C `svt_aom_set_me_hme_ref_prune_ctrls` (`enc_mode_config.c:345`). static.
#[must_use]
pub fn set_me_hme_ref_prune_ctrls(prune_level: u8) -> Option<MeHmeRefPruneCtrls> {
    let no_cap = u16::MAX;
    let mut c = MeHmeRefPruneCtrls::default();
    match prune_level {
        0 => {
            c.enable_me_hme_ref_pruning = 0;
            c.prune_ref_if_hme_sad_dev_bigger_than_th = no_cap;
            c.prune_ref_if_me_sad_dev_bigger_than_th = no_cap;
        }
        1 => {
            c.enable_me_hme_ref_pruning = 1;
            c.prune_ref_if_hme_sad_dev_bigger_than_th = 80;
            c.prune_ref_if_me_sad_dev_bigger_than_th = no_cap;
        }
        2 => {
            c.enable_me_hme_ref_pruning = 1;
            c.prune_ref_if_hme_sad_dev_bigger_than_th = 50;
            c.prune_ref_if_me_sad_dev_bigger_than_th = no_cap;
        }
        3 => {
            c.enable_me_hme_ref_pruning = 1;
            c.prune_ref_if_hme_sad_dev_bigger_than_th = 30;
            c.prune_ref_if_me_sad_dev_bigger_than_th = no_cap;
        }
        4 => {
            c.enable_me_hme_ref_pruning = 1;
            c.prune_ref_if_hme_sad_dev_bigger_than_th = 15;
            c.prune_ref_if_me_sad_dev_bigger_than_th = 60;
        }
        5 => {
            c.enable_me_hme_ref_pruning = 1;
            c.prune_ref_if_hme_sad_dev_bigger_than_th = 5;
            c.prune_ref_if_me_sad_dev_bigger_than_th = 60;
        }
        6 => {
            c.enable_me_hme_ref_pruning = 1;
            c.prune_ref_if_hme_sad_dev_bigger_than_th = 5;
            c.prune_ref_if_me_sad_dev_bigger_than_th = 60;
            c.zz_sad_th = 20 * 64 * 64;
            c.zz_sad_pct = 5;
            c.phme_sad_th = 10 * 64 * 64;
            c.phme_sad_pct = 5;
        }
        _ => return None,
    }
    Some(c)
}

/// C `MvBasedSearchAdj` (`me_context.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MvBasedSearchAdj {
    /// `enabled`
    pub enabled: u8,
    /// `nearest_ref_only`
    pub nearest_ref_only: u8,
    /// `mv_size_th`
    pub mv_size_th: u16,
    /// `sa_multiplier`
    pub sa_multiplier: u8,
}

/// C `svt_aom_set_mv_based_sa_ctrls` (`enc_mode_config.c:428`). static.
#[must_use]
pub fn set_mv_based_sa_ctrls(mv_sa_adj_level: u8) -> Option<MvBasedSearchAdj> {
    match mv_sa_adj_level {
        0 => Some(MvBasedSearchAdj::default()),
        1 => Some(MvBasedSearchAdj {
            enabled: 1,
            nearest_ref_only: 0,
            mv_size_th: 25,
            sa_multiplier: 2,
        }),
        2 => Some(MvBasedSearchAdj {
            enabled: 1,
            nearest_ref_only: 1,
            mv_size_th: 25,
            sa_multiplier: 2,
        }),
        _ => None,
    }
}

/// C `MeSrCtrls` (`me_context.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MeSrCtrls {
    /// `enable_me_sr_adjustment`
    pub enable_me_sr_adjustment: u8,
    /// `reduce_me_sr_based_on_mv_length_th`
    pub reduce_me_sr_based_on_mv_length_th: u16,
    /// `stationary_hme_sad_abs_th`
    pub stationary_hme_sad_abs_th: u32,
    /// `stationary_me_sr_divisor`
    pub stationary_me_sr_divisor: u16,
    /// `reduce_me_sr_based_on_hme_sad_abs_th`
    pub reduce_me_sr_based_on_hme_sad_abs_th: u32,
    /// `me_sr_divisor_for_low_hme_sad`
    pub me_sr_divisor_for_low_hme_sad: u16,
    /// `distance_based_hme_resizing`
    pub distance_based_hme_resizing: u8,
}

/// C `svt_aom_set_me_sr_adjustment_ctrls` (`enc_mode_config.c:453`). static.
///
/// The post-switch HME-level fixup is part of the function: when HME level 2 is
/// off the two absolute SAD thresholds are divided by 4 (level 1 on) or 16
/// (level 1 also off). It fires on the level-0 arm too, where the thresholds
/// are the zeroed context values — dividing 0 is a no-op, but the branch is
/// still taken, so the port keeps it unconditional exactly as C has it.
#[must_use]
pub fn set_me_sr_adjustment_ctrls(
    sr_adjustment_level: u8,
    enable_hme_level1_flag: bool,
    enable_hme_level2_flag: bool,
) -> Option<MeSrCtrls> {
    let mut c = MeSrCtrls::default();
    match sr_adjustment_level {
        0 => c.enable_me_sr_adjustment = 0,
        1..=3 => {
            c.enable_me_sr_adjustment = 1;
            c.reduce_me_sr_based_on_mv_length_th = 4;
            c.stationary_hme_sad_abs_th = 12000;
            c.stationary_me_sr_divisor = 8;
            // The ONLY field that separates level 3 from levels 1 and 2.
            c.reduce_me_sr_based_on_hme_sad_abs_th = if sr_adjustment_level == 3 {
                12000
            } else {
                6000
            };
            c.me_sr_divisor_for_low_hme_sad = 8;
            // The ONLY field that separates level 1 from level 2.
            c.distance_based_hme_resizing = u8::from(sr_adjustment_level != 1);
        }
        4 => {
            c.enable_me_sr_adjustment = 2;
            c.reduce_me_sr_based_on_mv_length_th = 16;
            c.stationary_hme_sad_abs_th = 20000;
            c.stationary_me_sr_divisor = 8;
            c.reduce_me_sr_based_on_hme_sad_abs_th = 20000;
            c.me_sr_divisor_for_low_hme_sad = 8;
            c.distance_based_hme_resizing = 1;
        }
        5 => {
            c.enable_me_sr_adjustment = 2;
            c.reduce_me_sr_based_on_mv_length_th = 20;
            c.stationary_hme_sad_abs_th = 24000;
            c.stationary_me_sr_divisor = 8;
            c.reduce_me_sr_based_on_hme_sad_abs_th = 24000;
            c.me_sr_divisor_for_low_hme_sad = 8;
            c.distance_based_hme_resizing = 1;
        }
        _ => return None,
    }
    if !enable_hme_level2_flag {
        let div = if enable_hme_level1_flag { 4 } else { 16 };
        c.stationary_hme_sad_abs_th /= div;
        c.reduce_me_sr_based_on_hme_sad_abs_th /= div;
    }
    Some(c)
}

/// C `Me8x8VarCtrls` (`me_context.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Me8x8VarCtrls {
    /// `enabled`
    pub enabled: u8,
    /// `me_sr_div4_th`
    pub me_sr_div4_th: u32,
    /// `me_sr_div2_th`
    pub me_sr_div2_th: u32,
    /// `me_sr_mult2_th`
    pub me_sr_mult2_th: u32,
}

/// C `svt_aom_set_me_8x8_var_ctrls` (`enc_mode_config.c:526`). static.
///
/// `me_8x8_cost_variance` is read back by `sig_deriv_enc_dec_common` (the LPD1
/// bump) and by PD0 (`parent_cost_bias`), so this leaks into the partition
/// decision.
#[must_use]
pub fn set_me_8x8_var_ctrls(level: u8) -> Option<Me8x8VarCtrls> {
    match level {
        0 => Some(Me8x8VarCtrls::default()),
        1 => Some(Me8x8VarCtrls {
            enabled: 1,
            me_sr_div4_th: 0,
            me_sr_div2_th: 0,
            me_sr_mult2_th: 900_000,
        }),
        2 => Some(Me8x8VarCtrls {
            enabled: 1,
            me_sr_div4_th: 80_000,
            me_sr_div2_th: 150_000,
            me_sr_mult2_th: u32::MAX,
        }),
        _ => None,
    }
}

/// C `PreHmeCtrls` (`me_context.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PreHmeCtrls {
    /// `enable`
    pub enable: u8,
    /// `prehme_sa_cfg[0]` — the VERTICAL shape search region.
    pub prehme_sa_cfg_vert: SearchAreaMinMax,
    /// `prehme_sa_cfg[1]` — the HORIZONTAL shape search region.
    pub prehme_sa_cfg_horz: SearchAreaMinMax,
    /// `skip_search_line`
    pub skip_search_line: u8,
    /// `l1_early_exit`
    pub l1_early_exit: u8,
}

/// C `svt_aom_set_prehme_ctrls` (`enc_mode_config.c:551`). static.
#[must_use]
pub fn set_prehme_ctrls(level: u8) -> Option<PreHmeCtrls> {
    let mut c = PreHmeCtrls::default();
    let mut set = |vmin: (u16, u16), vmax: (u16, u16), hmin: (u16, u16), hmax: (u16, u16)| {
        c.enable = 1;
        c.prehme_sa_cfg_vert.sa_min = SearchArea::new(vmin.0, vmin.1);
        c.prehme_sa_cfg_vert.sa_max = SearchArea::new(vmax.0, vmax.1);
        c.prehme_sa_cfg_horz.sa_min = SearchArea::new(hmin.0, hmin.1);
        c.prehme_sa_cfg_horz.sa_max = SearchArea::new(hmax.0, hmax.1);
    };
    match level {
        0 => return Some(PreHmeCtrls::default()),
        1 => set((8, 144), (8, 496), (144, 3), (496, 3)),
        2 => set((8, 100), (8, 400), (96, 3), (384, 3)),
        3 => set((8, 100), (8, 350), (32, 7), (200, 7)),
        4 => set((8, 100), (8, 350), (32, 7), (128, 7)),
        _ => return None,
    }
    c.skip_search_line = u8::from(level >= 3);
    c.l1_early_exit = u8::from(level == 4);
    Some(c)
}

// ---------------------------------------------------------------------------
// The two exported entry points
// ---------------------------------------------------------------------------

/// The `MeContext` fields `svt_aom_sig_deriv_me` writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MeSignals {
    /// `me_sa`
    pub me_sa: SearchAreaMinMax,
    /// `num_hme_sa_w` / `num_hme_sa_h` / `hme_l0_sa` / `hme_l1_sa` / `hme_l2_sa`
    pub hme: HmeSearchParams,
    /// `enable_hme_flag`
    pub enable_hme_flag: u8,
    /// `enable_hme_level0_flag`
    pub enable_hme_level0_flag: u8,
    /// `enable_hme_level1_flag`
    pub enable_hme_level1_flag: u8,
    /// `enable_hme_level2_flag`
    pub enable_hme_level2_flag: u8,
    /// `hme_search_method`
    pub hme_search_method: u8,
    /// `me_search_method`
    pub me_search_method: u8,
    /// `reduce_hme_l0_sr_th_min`
    pub reduce_hme_l0_sr_th_min: u8,
    /// `reduce_hme_l0_sr_th_max`
    pub reduce_hme_l0_sr_th_max: u8,
    /// `prehme_ctrl`
    pub prehme_ctrl: PreHmeCtrls,
    /// `me_hme_prune_ctrls`
    pub me_hme_prune_ctrls: MeHmeRefPruneCtrls,
    /// `me_sr_adjustment_ctrls`
    pub me_sr_adjustment_ctrls: MeSrCtrls,
    /// `mv_based_sa_adj`
    pub mv_based_sa_adj: MvBasedSearchAdj,
    /// `me_8x8_var_ctrls`
    pub me_8x8_var_ctrls: Me8x8VarCtrls,
    /// `prune_me_candidates_th`
    pub prune_me_candidates_th: u16,
    /// `sc_class_me_boost`
    pub sc_class_me_boost: u8,
    /// `use_best_unipred_cand_only`
    pub use_best_unipred_cand_only: u8,
    /// `me_early_exit_th`
    pub me_early_exit_th: u32,
    /// `me_static_b64_th`
    pub me_static_b64_th: u32,
    /// `me_safe_limit_zz_th`
    pub me_safe_limit_zz_th: u32,
    /// `prev_me_stage_based_exit_th`
    pub prev_me_stage_based_exit_th: u32,
}

/// The inputs `svt_aom_sig_deriv_me` reads off the SCS and PPCS, spelled out.
#[derive(Debug, Clone, Copy)]
pub struct MeDerivInputs {
    /// `pcs->enc_mode`
    pub enc_mode: i8,
    /// `pcs->sc_class5`
    pub sc_class5: u8,
    /// `scs->input_resolution`
    pub input_resolution: ResolutionRange,
    /// `scs->static_config.rtc`
    pub rtc_tune: bool,
    /// `frame_is_boosted(pcs)` — `frame_is_intra_only || update_type is
    /// ARF/GF` (`enc_mode_config.h:103`).
    pub is_base: bool,
    /// `pcs->hierarchical_levels`
    pub hierarchical_levels: u8,
    /// `pcs->enable_hme_flag`
    pub enable_hme_flag: u8,
    /// `pcs->enable_hme_level0_flag`
    pub enable_hme_level0_flag: u8,
    /// `pcs->enable_hme_level1_flag`
    pub enable_hme_level1_flag: u8,
    /// `pcs->enable_hme_level2_flag`
    pub enable_hme_level2_flag: u8,
    /// `pcs->use_best_me_unipred_cand_only`
    pub use_best_me_unipred_cand_only: u8,
    /// `scs->qp_based_th_scaling_ctrls.me_qp_based_th_scaling`
    pub me_qp_based_th_scaling: bool,
    /// `scs->qp_based_th_scaling_ctrls.hme_qp_based_th_scaling`
    pub hme_qp_based_th_scaling: bool,
    /// `scs->static_config.qp`
    pub qp: u32,
    /// `scs->mrp_ctrls.safe_limit_nref`
    pub safe_limit_nref: u8,
    /// `scs->mrp_ctrls.safe_limit_zz_th`
    pub safe_limit_zz_th: u32,
}

/// C `svt_aom_sig_deriv_me` (`enc_mode_config.c:700`). EXPORTED.
#[must_use]
pub fn sig_deriv_me(i: MeDerivInputs) -> MeSignals {
    let enc_mode = i.enc_mode;
    let rtc_tune = i.rtc_tune;
    let use_flat_ipp = rtc_tune && i.hierarchical_levels == 0;

    let mut s = MeSignals {
        me_sa: set_me_search_params(
            enc_mode,
            i.input_resolution,
            rtc_tune,
            i.me_qp_based_th_scaling,
            i.qp,
        ),
        hme: set_hme_search_params(
            enc_mode,
            i.input_resolution,
            rtc_tune,
            i.hme_qp_based_th_scaling,
            i.qp,
        ),
        enable_hme_flag: i.enable_hme_flag,
        enable_hme_level0_flag: i.enable_hme_level0_flag,
        enable_hme_level1_flag: i.enable_hme_level1_flag,
        enable_hme_level2_flag: i.enable_hme_level2_flag,
        hme_search_method: SUB_SAD_SEARCH,
        me_search_method: SUB_SAD_SEARCH,
        reduce_hme_l0_sr_th_min: 0,
        reduce_hme_l0_sr_th_max: 0,
        ..MeSignals::default()
    };

    // Pre-HME level.
    let mut prehme_level = if rtc_tune {
        if enc_mode <= M8 {
            2
        } else if enc_mode <= M10 {
            4
        } else {
            0
        }
    } else if enc_mode <= M7 {
        2
    } else if enc_mode <= M11 {
        4
    } else {
        0
    };
    if i.enable_hme_level1_flag == 0 {
        prehme_level = 0;
    }
    s.prehme_ctrl = set_prehme_ctrls(prehme_level).expect("prehme level in range");

    // HME/ME reference pruning level.
    let me_ref_prune_level = if rtc_tune {
        if use_flat_ipp {
            6
        } else if enc_mode <= M7 {
            if i.is_base { 1 } else { 6 }
        } else {
            6
        }
    } else if enc_mode <= MR {
        0
    } else if enc_mode <= M0 {
        if i.is_base { 1 } else { 4 }
    } else if enc_mode <= M4 {
        if i.is_base { 1 } else { 5 }
    } else if enc_mode <= M8 {
        if i.is_base { 1 } else { 6 }
    } else {
        6
    };
    s.me_hme_prune_ctrls =
        set_me_hme_ref_prune_ctrls(me_ref_prune_level).expect("prune level in range");

    // HME-based ME search-range adjustment.
    let me_sr_adj_lvl = if enc_mode <= M0 { 0 } else { 3 };
    // The threshold fixup inside the helper reads the ME CONTEXT's HME flags,
    // which the block above has already overwritten from the PCS. C's tests are
    // `enable_hme_level2_flag == 0` (fixup applies when level 2 is OFF) and
    // `enable_hme_level1_flag == 1` (an EXACT compare, not "nonzero"); the
    // helper takes "level 2 enabled", so the level-2 flag is passed as `!= 0`.
    s.me_sr_adjustment_ctrls = set_me_sr_adjustment_ctrls(
        me_sr_adj_lvl,
        s.enable_hme_level1_flag == 1,
        s.enable_hme_level2_flag != 0,
    )
    .expect("sr level in range");

    let mv_sa_adj_level = if enc_mode <= M0 { 2 } else { 0 };
    s.mv_based_sa_adj = set_mv_based_sa_ctrls(mv_sa_adj_level).expect("mv sa level in range");

    s.me_8x8_var_ctrls = set_me_8x8_var_ctrls(2).expect("me 8x8 var level 2");

    s.prune_me_candidates_th = if enc_mode <= M1 { 0 } else { 65 };

    // Applies to sc-class1 & sc-class4 scenes.
    s.sc_class_me_boost = if i.sc_class5 != 0 {
        if enc_mode <= M2 {
            1
        } else if enc_mode <= M5 {
            2
        } else {
            0
        }
    } else {
        0
    };

    s.use_best_unipred_cand_only = i.use_best_me_unipred_cand_only;

    s.me_early_exit_th = if rtc_tune {
        BLOCK_SIZE_64 * BLOCK_SIZE_64 * 8
    } else if enc_mode <= M2 {
        0
    } else if enc_mode <= M9 {
        BLOCK_SIZE_64 * BLOCK_SIZE_64 * 8
    } else {
        BLOCK_SIZE_64 * BLOCK_SIZE_64 * 12
    };

    // Complete ME bypass for static 64x64 blocks (rtc only).
    s.me_static_b64_th = if rtc_tune {
        BLOCK_SIZE_64 * BLOCK_SIZE_64
    } else {
        0
    };

    s.me_safe_limit_zz_th = if i.safe_limit_nref == 1 {
        i.safe_limit_zz_th
    } else {
        0
    };
    s.prev_me_stage_based_exit_th = 0;
    s
}

/// The `MeContext` fields C `tf_set_me_hme_params_oq` writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TfMeHmeParams {
    /// `num_hme_sa_w`
    pub num_hme_sa_w: u8,
    /// `num_hme_sa_h`
    pub num_hme_sa_h: u8,
    /// `hme_l0_sa_default_tf` — note this is a DIFFERENT field from
    /// `hme_l0_sa`, which the TF path leaves alone.
    pub hme_l0_sa_default_tf: SearchAreaMinMax,
    /// `hme_l1_sa`
    pub hme_l1_sa: SearchArea,
    /// `hme_l2_sa`
    pub hme_l2_sa: SearchArea,
    /// `me_sa`
    pub me_sa: SearchAreaMinMax,
}

/// C `tf_set_me_hme_params_oq` (`enc_mode_config.c:611`). static.
///
/// Keyed on `pcs->tf_ctrls.hme_me_level`, not on `enc_mode`.
#[must_use]
pub fn tf_set_me_hme_params_oq(
    hme_me_level: u8,
    scs_input_resolution: ResolutionRange,
    qp_opt: bool,
    tf_me_qp_based_th_scaling: bool,
    qp: u32,
) -> Option<TfMeHmeParams> {
    let mut p = TfMeHmeParams {
        num_hme_sa_w: 2,
        num_hme_sa_h: 2,
        ..TfMeHmeParams::default()
    };
    match hme_me_level {
        0 => {
            p.hme_l0_sa_default_tf.sa_min = SearchArea::new(30, 30);
            p.hme_l0_sa_default_tf.sa_max = SearchArea::new(60, 60);
            p.hme_l1_sa = SearchArea::new(16, 16);
            p.hme_l2_sa = SearchArea::new(16, 16);
            p.me_sa.sa_min = SearchArea::new(60, 60);
            p.me_sa.sa_max = SearchArea::new(120, 120);
        }
        1 => {
            p.hme_l0_sa_default_tf.sa_min = SearchArea::new(16, 16);
            p.hme_l0_sa_default_tf.sa_max = SearchArea::new(32, 32);
            p.hme_l1_sa = SearchArea::new(16, 16);
            p.hme_l2_sa = SearchArea::new(16, 16);
            p.me_sa.sa_min = SearchArea::new(16, 16);
            p.me_sa.sa_max = SearchArea::new(32, 32);
        }
        2 => {
            if scs_input_resolution <= ResolutionRange::R360p {
                p.hme_l0_sa_default_tf.sa_min = SearchArea::new(8, 8);
                p.hme_l0_sa_default_tf.sa_max = SearchArea::new(8, 8);
                p.hme_l1_sa = SearchArea::new(8, 8);
            } else if scs_input_resolution <= ResolutionRange::R480p {
                p.hme_l0_sa_default_tf.sa_min = SearchArea::new(8, 8);
                p.hme_l0_sa_default_tf.sa_max = SearchArea::new(16, 16);
                p.hme_l1_sa = SearchArea::new(8, 8);
            } else {
                p.hme_l0_sa_default_tf.sa_min = SearchArea::new(16, 16);
                p.hme_l0_sa_default_tf.sa_max = SearchArea::new(32, 32);
                p.hme_l1_sa = SearchArea::new(16, 16);
            }
            p.hme_l2_sa = SearchArea::new(16, 16);
            p.me_sa.sa_min = SearchArea::new(8, 8);
            p.me_sa.sa_max = SearchArea::new(8, 8);
        }
        3 => {
            p.hme_l0_sa_default_tf.sa_min = SearchArea::new(8, 8);
            p.hme_l0_sa_default_tf.sa_max = SearchArea::new(8, 8);
            p.hme_l1_sa = SearchArea::new(8, 8);
            p.hme_l2_sa = SearchArea::new(8, 8);
            p.me_sa.sa_min = SearchArea::new(8, 8);
            p.me_sa.sa_max = SearchArea::new(8, 8);
        }
        4 => {
            p.hme_l0_sa_default_tf.sa_min = SearchArea::new(4, 4);
            p.hme_l0_sa_default_tf.sa_max = SearchArea::new(4, 4);
            p.hme_l1_sa = SearchArea::new(8, 8);
            p.hme_l2_sa = SearchArea::new(8, 8);
            p.me_sa.sa_min = SearchArea::new(8, 8);
            p.me_sa.sa_max = SearchArea::new(8, 8);
        }
        _ => return None,
    }
    // Modulate ONLY me_sa by qp, and only when tf_ctrls.qp_opt is set. Every
    // floor here is 8 — unlike set_me_search_params, whose height floor is 3.
    if qp_opt {
        let (qw, qwd) = get_qp_based_th_scaling_factors(tf_me_qp_based_th_scaling, qp);
        let scale = |v: u16| -> u16 { 8u32.max(divide_and_round(u32::from(v) * qw, qwd)) as u16 };
        p.me_sa.sa_min.width = scale(p.me_sa.sa_min.width);
        p.me_sa.sa_min.height = scale(p.me_sa.sa_min.height);
        p.me_sa.sa_max.width = scale(p.me_sa.sa_max.width);
        p.me_sa.sa_max.height = scale(p.me_sa.sa_max.height);
    }
    Some(p)
}

/// The `MeContext` fields `svt_aom_sig_deriv_me_tf` writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MeTfSignals {
    /// `tf_set_me_hme_params_oq`'s output
    pub params: TfMeHmeParams,
    /// `enable_hme_flag`
    pub enable_hme_flag: u8,
    /// `enable_hme_level0_flag`
    pub enable_hme_level0_flag: u8,
    /// `enable_hme_level1_flag`
    pub enable_hme_level1_flag: u8,
    /// `enable_hme_level2_flag`
    pub enable_hme_level2_flag: u8,
    /// `hme_search_method`
    pub hme_search_method: u8,
    /// `me_search_method`
    pub me_search_method: u8,
    /// `prehme_ctrl`
    pub prehme_ctrl: PreHmeCtrls,
    /// `me_hme_prune_ctrls`
    pub me_hme_prune_ctrls: MeHmeRefPruneCtrls,
    /// `me_sr_adjustment_ctrls`
    pub me_sr_adjustment_ctrls: MeSrCtrls,
    /// `mv_based_sa_adj`
    pub mv_based_sa_adj: MvBasedSearchAdj,
    /// `me_8x8_var_ctrls`
    pub me_8x8_var_ctrls: Me8x8VarCtrls,
    /// `sc_class_me_boost`
    pub sc_class_me_boost: u8,
    /// `me_early_exit_th`
    pub me_early_exit_th: u32,
    /// `me_safe_limit_zz_th`
    pub me_safe_limit_zz_th: u32,
    /// `reduce_hme_l0_sr_th_min`
    pub reduce_hme_l0_sr_th_min: u8,
    /// `reduce_hme_l0_sr_th_max`
    pub reduce_hme_l0_sr_th_max: u8,
    /// `prev_me_stage_based_exit_th`
    pub prev_me_stage_based_exit_th: u32,
}

/// C `svt_aom_sig_deriv_me_tf` (`enc_mode_config.c:848`). EXPORTED.
///
/// Byte-parity blocker for the video-mode key frame: TF runs before encode and
/// rewrites the source pixels.
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn sig_deriv_me_tf(
    hme_me_level: u8,
    scs_input_resolution: ResolutionRange,
    qp_opt: bool,
    tf_me_qp_based_th_scaling: bool,
    qp: u32,
    tf_enable_hme_flag: u8,
    tf_enable_hme_level0_flag: u8,
    tf_enable_hme_level1_flag: u8,
    tf_enable_hme_level2_flag: u8,
) -> Option<MeTfSignals> {
    let params = tf_set_me_hme_params_oq(
        hme_me_level,
        scs_input_resolution,
        qp_opt,
        tf_me_qp_based_th_scaling,
        qp,
    )?;
    let method = if hme_me_level <= 2 {
        FULL_SAD_SEARCH
    } else {
        SUB_SAD_SEARCH
    };
    let mut s = MeTfSignals {
        params,
        enable_hme_flag: tf_enable_hme_flag,
        enable_hme_level0_flag: tf_enable_hme_level0_flag,
        enable_hme_level1_flag: tf_enable_hme_level1_flag,
        enable_hme_level2_flag: tf_enable_hme_level2_flag,
        hme_search_method: method,
        me_search_method: method,
        ..MeTfSignals::default()
    };
    s.prehme_ctrl = set_prehme_ctrls(0).expect("prehme level 0");
    // Ref pruning and ME SR adjustment are disallowed for TF in
    // motion_estimate_sb(), so both are forced to level 0.
    s.me_hme_prune_ctrls = set_me_hme_ref_prune_ctrls(0).expect("prune level 0");
    s.me_sr_adjustment_ctrls = set_me_sr_adjustment_ctrls(
        0,
        s.enable_hme_level1_flag == 1,
        s.enable_hme_level2_flag != 0,
    )
    .expect("sr level 0");
    s.mv_based_sa_adj = set_mv_based_sa_ctrls(0).expect("mv sa level 0");
    s.me_8x8_var_ctrls = set_me_8x8_var_ctrls(0).expect("me 8x8 var level 0");
    s.sc_class_me_boost = 0;
    let exit_th = if hme_me_level <= 1 {
        0
    } else {
        BLOCK_SIZE_64 * BLOCK_SIZE_64 * 4
    };
    s.me_early_exit_th = exit_th;
    s.me_safe_limit_zz_th = 0;
    s.reduce_hme_l0_sr_th_min = 0;
    s.reduce_hme_l0_sr_th_max = 0;
    s.prev_me_stage_based_exit_th = exit_th;
    Some(s)
}
