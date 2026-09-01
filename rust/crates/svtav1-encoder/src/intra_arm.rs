//! The `scs->allintra` fork for the two INTRA-candidate ladders the leaf
//! funnel consumes: `pcs->pic_filter_intra_level` and the
//! `(intra_level, dist_based_ang_intra_level)` pair.
//!
//! Both are assigned by the arm pair
//! `svt_aom_sig_deriv_mode_decision_config_{allintra,default}`
//! (`enc_mode_config.c:9935/:9952` and `:8952/:9033`), dispatched on
//! `scs->allintra` (`md_config_process.c:924-930`), and both reach the funnel
//! through a control table — `set_filter_intra_ctrls` (`:6235`) and
//! `set_intra_ctrls` (`:6535`).
//!
//! What the arms disagree about on the only video picture this port encodes
//! today (a KEY frame: `is_islice`, `is_base`, `transition_present == 0`):
//!
//! | preset | fi allintra | fi video | intra_level allintra | intra_level video |
//! |---|---|---|---|---|
//! | 0 | 1 | 1 | 1 | 1 |
//! | 1 | 2 | 1 | 1 | 1 |
//! | 2 | 2 | 2 | 1 | 1 |
//! | 3..=4 | 2 | 2 | 1 | 1 |
//! | 5 | 2 | 2 | 2 | 1 |
//! | 6 | 2 | **0** | 6 | **2** |
//! | 7 | 0 | 0 | 7 | 2 |
//! | 8..=11 | 0 | 0 | 7 / 8 | 4 |
//! | 12..=13 | 0 | 0 | 8 (clamped M9) | 9 (clamped M11 -> 4) |
//!
//! The M6 row is the one the inter campaign is standing on: a video-mode key
//! frame at preset 6 searches the M5-shaped intra set (PAETH mode end, angular
//! level 2) with filter-intra OFF, where the still path searches the M6 set
//! (SMOOTH mode end, angular level 4) with a FILTER_DC candidate injected.
//!
//! `dist_based_ang_intra_level` is 0 on BOTH arms for every I-slice
//! (`svt_aom_get_intra_mode_levels_default` keys its non-zero rows on
//! `is_islice ? 0 : ...` / `is_base ? 0 : ...`, and the allintra twin returns
//! a literal 0 at every preset), so `set_intra_ctrls`' second switch always
//! lands on case 0 — every `skip_angular_delta*_th` is -1, i.e. the
//! distance-based angular skip is inert. The port's video envelope is
//! key-frames-only, so a non-zero value is unreachable; [`intra_mode_levels`]
//! returns it anyway and [`apply`] asserts it, so the day inter pictures
//! arrive this stops being silent instead of being wrong.
//!
//! # Evidence
//!
//! Tier 1 on both arms, and neither ladder nor either control table is
//! re-transcribed here:
//!
//! - **video** — `svt_aom_sig_deriv_mode_decision_config_default` is EXPORTED
//!   and `tests/c_parity_sig_deriv_md_config.rs` already drives the real
//!   symbol, reading back `pcs->pic_filter_intra_level`, `pcs->intra_level`
//!   and `pcs->dist_based_ang_intra_level`. This module calls the same
//!   [`md_config::get_filter_intra_level_default`] /
//!   [`leaf::get_intra_mode_levels_default`] that function's body calls.
//! - **allintra** — the twin entry point is likewise EXPORTED, and since
//!   2026-09-01 `ref_sig_deriv_md_config_allintra` reads back the SAME slot
//!   set, so `arms_agree_with_c_on_both_intra_ladders` drives both arms from
//!   one input population.
//! - the level -> controls mapping is pinned entry-for-entry against
//!   `FunnelCfg::for_preset`'s baked allintra values by
//!   `allintra_flattening_matches_the_ladder`, which is what makes the still
//!   path byte-neutral BY CONSTRUCTION and not merely by measurement.

use crate::leaf_funnel::FunnelCfg;
use crate::port_enc_mode_config::{leaf, md_config};
use crate::sc_detect::ScArm;

/// `pcs->pic_filter_intra_level` for this arm. `enc_mode` must already be
/// [`crate::rate_arm::eff_enc_mode`]-clamped.
#[must_use]
pub(crate) fn filter_intra_level(arm: ScArm, enc_mode: u8) -> u8 {
    let m = i8::try_from(enc_mode).unwrap_or(i8::MAX);
    match arm {
        // C `get_filter_intra_level_allintra` (:8790): 1 at M0, 2 through M6,
        // 0 above. Not ported as a standalone function anywhere else — the
        // still path had it flattened into `seq_tools_for_preset` (for the SH
        // bit) and into `FunnelCfg::for_preset` (for the candidate). This is
        // the ladder itself; the flattening pin below is the regression
        // oracle.
        ScArm::Allintra => {
            if m <= 0 {
                1
            } else if m <= 6 {
                2
            } else {
                0
            }
        }
        ScArm::Video { .. } => md_config::get_filter_intra_level_default(m),
    }
}

/// `set_filter_intra_ctrls` (`enc_mode_config.c:6235`) as the
/// `(enabled, max_filter_intra_mode)` pair `FunnelCfg` carries.
///
/// `FILTER_DC_PRED` is 0 and `FILTER_PAETH_PRED` is 4 (`FilterIntraMode`,
/// `EbAv1Structs.h`), which is exactly `FunnelCfg::fi_max`'s domain.
///
/// # Panics
/// On a level outside 0..=2 — C `assert(0)`s there.
#[must_use]
pub(crate) fn filter_intra_ctrls(level: u8) -> (bool, u8) {
    match level {
        0 => (false, 0),
        1 => (true, 4),
        2 => (true, 0),
        _ => panic!("filter_intra level {level} outside C's switch"),
    }
}

/// `(intra_level, dist_based_ang_intra_level)` for this arm. `enc_mode` must
/// already be [`crate::rate_arm::eff_enc_mode`]-clamped.
#[must_use]
pub(crate) fn intra_mode_levels(
    arm: ScArm,
    enc_mode: u8,
    is_islice: bool,
    is_base: bool,
    transition_present: bool,
) -> (u8, u8) {
    let m = i8::try_from(enc_mode).unwrap_or(i8::MAX);
    match arm {
        ScArm::Allintra => {
            let (lvl, ang) = leaf::get_intra_mode_levels_allintra(m);
            (lvl as u8, ang as u8)
        }
        ScArm::Video { .. } => {
            let (lvl, ang) = leaf::get_intra_mode_levels_default(
                m,
                is_islice,
                is_base,
                i32::from(transition_present),
            );
            (lvl as u8, ang as u8)
        }
    }
}

/// `set_intra_ctrls` (`enc_mode_config.c:6535`) as the four values `FunnelCfg`
/// carries: `(intra_mode_end, angular_pred_level, prune_using_best_mode,
/// prune_using_edge_info)`.
///
/// The TPL refinement inside cases 2/3/4/5 is NOT modelled: it is gated on
/// `ppcs->tpl_ctrls.enable` and reads `get_sb_tpl_intra_stats`, and the port
/// has no TPL. Case 2 — the one a video-mode M6 key frame lands on — takes it
/// only when TPL is enabled AND `tpl_ctrls.intra_mode_end == PAETH_PRED`, and
/// with no TPL dispenser the stats getter returns false anyway. This is named
/// here rather than left implicit: when TPL lands, cases 2..=5 grow a per-SB
/// override and this function's signature has to grow with them.
///
/// # Panics
/// On a level outside C's switch (it `assert(0)`s there).
#[must_use]
pub(crate) fn intra_ctrls(level: u8) -> (u8, u8, bool, bool) {
    // C `angular_pred_level[MAX_INTRA_LEVEL]` (enc_mode_config.c:22).
    const ANGULAR_PRED_LEVEL: [u8; 10] = [0, 1, 2, 2, 3, 4, 4, 4, 4, 0];
    // PredictionMode indices: DC 0, SMOOTH 9, SMOOTH_H 11, PAETH 12.
    let (mode_end, prune_best, prune_edge) = match level {
        0 => (0u8, false, false),
        1 | 2 | 3 => (12u8, false, false),
        4 => (11u8, false, false),
        5 | 6 => (9u8, false, false),
        7 => (9u8, true, false),
        8 => (9u8, true, true),
        9 => (0u8, false, false),
        _ => panic!("intra_level {level} outside C's switch"),
    };
    (
        mode_end,
        ANGULAR_PRED_LEVEL[level as usize],
        prune_best,
        prune_edge,
    )
}

/// C `scs->seq_header.enable_intra_edge_filter`
/// (`svt_aom_sig_deriv_pre_analysis_scs`, `enc_mode_config.c:2807-2821`).
///
/// This is a SEQUENCE-header bit, so it is decoder-visible: with it set, a
/// conforming decoder edge-filters and upsamples every directional prediction
/// whose `p_angle != 90/180`. The encoder MUST predict the same way.
///
/// The two arms could hardly differ more:
///
/// - **video** (`:2820`): a literal 1, at every preset. C's own comment says
///   "for non-still-image or non-all-intra configurations, keep edge filter
///   always ON".
/// - **allintra** (`:2815`): on only when the angular-refinement pruning is
///   active — `dist_based_ang_intra_level >= 1 || angular_pred_level[
///   intra_level] == 2 || == 3`. Walking
///   `svt_aom_get_intra_mode_levels_allintra` through
///   `angular_pred_level` (`:22`) lands in {2, 3} at intra_level 2 ONLY, which
///   is preset 5 — the value `speed_config` previously hardcoded.
///
/// It is derived HERE, once, and both the sequence header and the leaf
/// funnel's prediction read this one function, because the bug this fixes was
/// exactly the two disagreeing.
#[must_use]
pub(crate) fn intra_edge_filter(arm: ScArm, enc_mode: u8) -> bool {
    match arm {
        ScArm::Video { .. } => true,
        ScArm::Allintra => {
            // On this arm the ladder ignores is_islice / is_base entirely.
            let (intra_level, dist_ang) = intra_mode_levels(arm, enc_mode, true, true, false);
            const ANGULAR_PRED_LEVEL: [u8; 10] = [0, 1, 2, 2, 3, 4, 4, 4, 4, 0];
            let ang = ANGULAR_PRED_LEVEL[intra_level as usize];
            dist_ang >= 1 || ang == 2 || ang == 3
        }
    }
}

/// Stamp both ladders' results onto a [`FunnelCfg`], replacing the values
/// `FunnelCfg::for_preset` baked from the allintra arm.
///
/// On the allintra arm this is a no-op by construction (the pin below proves
/// it entry-for-entry); on the video arm it is the whole point.
pub(crate) fn apply(cfg: &mut FunnelCfg, arm: ScArm, enc_mode: u8, is_islice: bool, is_base: bool) {
    let (fi_on, fi_max) = filter_intra_ctrls(filter_intra_level(arm, enc_mode));
    cfg.filter_intra = fi_on;
    cfg.fi_max = fi_max;

    let (intra_level, dist_ang) = intra_mode_levels(arm, enc_mode, is_islice, is_base, false);
    debug_assert_eq!(
        dist_ang, 0,
        "dist_based_ang_intra_level != 0 needs set_intra_ctrls' skip_angular_delta*_th ported"
    );
    let (mode_end, angular, prune_best, prune_edge) = intra_ctrls(intra_level);
    cfg.mode_end = mode_end;
    cfg.angular_level = angular;
    cfg.prune_best_mode = prune_best;
    cfg.dc_only_gate = prune_edge;

    // The SH bit the decoder reads, applied to the funnel's own prediction.
    cfg.edge_filter = intra_edge_filter(arm, enc_mode);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The still path is byte-neutral BY CONSTRUCTION: for every preset the
    /// allintra arm reproduces exactly what `FunnelCfg::for_preset` baked.
    ///
    /// This is the same shape as `rate_arm::allintra_flattening_matches_the_ladder`
    /// and `part_arm::allintra_flattening_matches_*` — the baked table is the
    /// regression oracle, so a divergence fails here rather than in a 1,100-cell
    /// sweep.
    #[test]
    fn allintra_flattening_matches_the_ladder() {
        for preset in 0u8..=13 {
            let baked = FunnelCfg::for_preset(preset);
            let mut walked = baked;
            let eff = crate::rate_arm::eff_enc_mode(ScArm::Allintra, preset);
            apply(&mut walked, ScArm::Allintra, eff, true, true);
            assert_eq!(
                (
                    baked.filter_intra,
                    baked.fi_max,
                    baked.mode_end,
                    baked.angular_level,
                    baked.prune_best_mode,
                    baked.dc_only_gate
                ),
                (
                    walked.filter_intra,
                    walked.fi_max,
                    walked.mode_end,
                    walked.angular_level,
                    walked.prune_best_mode,
                    walked.dc_only_gate
                ),
                "allintra intra ladder vs FunnelCfg::for_preset at M{preset}"
            );
        }
    }

    /// The M6 key-frame row the inter campaign stands on, spelled out so a
    /// regression names itself.
    #[test]
    fn video_m6_key_frame_drops_filter_intra_and_takes_the_m5_intra_set() {
        let arm = ScArm::Video { is_islice: true };
        assert_eq!(filter_intra_level(arm, 6), 0);
        assert_eq!(intra_mode_levels(arm, 6, true, true, false), (2, 0));
        let (mode_end, angular, prune_best, prune_edge) = intra_ctrls(2);
        assert_eq!(
            (mode_end, angular, prune_best, prune_edge),
            (12, 2, false, false)
        );
        // ... which is exactly the still path's M5 shape.
        let m5 = FunnelCfg::for_preset(5);
        assert_eq!((m5.mode_end, m5.angular_level), (mode_end, angular));
    }
}
