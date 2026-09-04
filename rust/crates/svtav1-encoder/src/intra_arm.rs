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

/// `pcs->spatial_sse_full_loop_level` for this arm. `enc_mode` must already be
/// [`crate::rate_arm::eff_enc_mode`]-clamped.
///
/// The allintra arm is the LITERAL 3 at `enc_mode_config.c:10010` — no ladder
/// at all — which is why the still path has always run MDS1 in the frequency
/// domain. The video arm (`:9161-9165`) is `enc_mode <= ENC_M2 ? 1 : 3`, and
/// level 1 is `SSSE_MDS1`: spatial SSE from MD stage 1 onward.
#[must_use]
pub(crate) fn spatial_sse_full_loop_level(arm: ScArm, enc_mode: u8) -> u8 {
    match arm {
        ScArm::Allintra => 3,
        ScArm::Video { .. } => {
            if enc_mode <= 2 {
                1
            } else {
                3
            }
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

    // `ctx->mds_do_spatial_sse` at MDS1 — `spatial_sse_ctrls.level <=
    // SSSE_MDS1` (product_coding_loop.c:7025), with `SSSE_MDS1` the FIRST
    // enum value (`definitions.h:886`), so the test is level == SSSE_MDS1.
    cfg.spatial_sse_mds1 = matches!(
        crate::port_enc_mode_config::encdec::set_spatial_sse_full_loop_level(
            spatial_sse_full_loop_level(arm, enc_mode),
        )
        .expect("spatial_sse level outside C's switch")
        .level,
        crate::port_enc_mode_config::encdec::SpatialSseLevel::Mds1
    );

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
    /// `spatial_sse_full_loop_level` — the ONE-ROW fork at M0..M2 that makes
    /// the video arm run MDS1 in the SPATIAL domain. Anti-vacuity: the M3 row
    /// must agree on both arms, or the sweep below would pass trivially.
    #[test]
    fn spatial_sse_ladder_forks_only_at_m0_to_m2() {
        let vid = ScArm::Video { is_islice: true };
        for preset in 0u8..=13 {
            let eff_a = crate::rate_arm::eff_enc_mode(ScArm::Allintra, preset);
            let eff_v = crate::rate_arm::eff_enc_mode(vid, preset);
            assert_eq!(
                spatial_sse_full_loop_level(ScArm::Allintra, eff_a),
                3,
                "allintra is the literal 3 at M{preset}"
            );
            let want = if preset <= 2 { 1 } else { 3 };
            assert_eq!(
                spatial_sse_full_loop_level(vid, eff_v),
                want,
                "video ladder at M{preset}"
            );
        }
        // The arms DISAGREE at M0..M2 and AGREE from M3 — the shape the
        // 128x128 video matrix measured (p0/p1/p2 diff, p3+ identical).
        for preset in 0u8..=2 {
            assert_ne!(
                spatial_sse_full_loop_level(ScArm::Allintra, preset),
                spatial_sse_full_loop_level(vid, preset)
            );
        }
        for preset in 3u8..=8 {
            assert_eq!(
                spatial_sse_full_loop_level(ScArm::Allintra, preset),
                spatial_sse_full_loop_level(vid, preset)
            );
        }
    }

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

/// C `uv_mode_nfl_count`'s BASE, before the `uv_nic_scaling_num / 16` scale
/// (`search_best_independent_uv_mode`, product_coding_loop.c:7693-7696):
///
/// ```c
/// unsigned int uv_mode_nfl_count = pcs->scs->allintra ? ppcs->is_highest_layer ? 16 : 32
///     : pcs->slice_type == I_SLICE                    ? 64
///     : !ppcs->is_highest_layer                       ? 32
///                                                     : 16;
/// ```
///
/// The funnel carried the allintra arm's 32 alone (every picture it encoded
/// was a still), which on a VIDEO-arm key frame halves C's full-loop count:
/// C tests every injected uv candidate (61 at `intra_mode_end` PAETH, under
/// the 64 cap) where 32 keeps only the first 32 of a flat-chroma SAD tie, so
/// UV_SMOOTH*/UV_PAETH never reach the full loop and the per-luma table
/// resolves luma PAETH to UV_DC. Measured on `video_key_matrix` `gradient p0`
/// and `screenrep p0` (72x88 q40): both cells' first divergent block in
/// coding order priced PAETH_PRED +1310 and DC+FILTER_PAETH -1315 rate units
/// against C from MDS0 on, exactly the uv-mode rate under the coded luma mode
/// (`docs/INTER-ENCODE-PLAN.md` §1z³⁸).
///
/// `is_highest_layer` is C's `(temporal_layer_index == hierarchical_levels)
/// && hierarchical_levels != 0` (pd_process.c:5560) — false for every key
/// frame and for every picture of a flat GOP.
#[must_use]
pub(crate) fn ind_uv_nfl_base(arm: ScArm, is_highest_layer: bool) -> u16 {
    match arm {
        ScArm::Allintra => {
            if is_highest_layer {
                16
            } else {
                32
            }
        }
        ScArm::Video { is_islice: true } => 64,
        ScArm::Video { is_islice: false } => {
            if is_highest_layer {
                16
            } else {
                32
            }
        }
    }
}

#[cfg(test)]
mod ind_uv_nfl_tests {
    use super::*;

    /// The four arms of product_coding_loop.c:7693-7696, as C's literals.
    #[test]
    fn nfl_base_matches_c_ladder() {
        assert_eq!(ind_uv_nfl_base(ScArm::Allintra, false), 32);
        assert_eq!(ind_uv_nfl_base(ScArm::Allintra, true), 16);
        assert_eq!(ind_uv_nfl_base(ScArm::Video { is_islice: true }, false), 64);
        assert_eq!(ind_uv_nfl_base(ScArm::Video { is_islice: true }, true), 64);
        assert_eq!(
            ind_uv_nfl_base(ScArm::Video { is_islice: false }, false),
            32
        );
        assert_eq!(ind_uv_nfl_base(ScArm::Video { is_islice: false }, true), 16);
    }

    /// The count the funnel actually uses at M0 (uv_nic 16) and M1 (uv_nic
    /// 8): a video key frame runs 64 / 32 full-loop uv candidates where the
    /// allintra still runs 32 / 16 — i.e. every one of the 61 injected at
    /// M0, and the `uv_mode_total_count` MIN in C (:7698) is what caps it.
    #[test]
    fn video_key_frame_doubles_the_still_count() {
        for (uv_nic, still, video_kf) in [(16u64, 32u64, 64u64), (8, 16, 32)] {
            let scale = |base: u16| (u64::from(base) * uv_nic + 8) / 16;
            assert_eq!(scale(ind_uv_nfl_base(ScArm::Allintra, false)), still);
            assert_eq!(
                scale(ind_uv_nfl_base(ScArm::Video { is_islice: true }, false)),
                video_kf
            );
        }
    }
}
