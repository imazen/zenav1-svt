//! The `scs->allintra` fork for the three PARTITION-SEARCH ladders.
//!
//! `enc_mode_config.c` carries an `_allintra` / `_rtc` / `_default` triple for
//! `ctx->max_block_size`, `pcs->nsq_geom_level` and `pcs->nsq_search_level`,
//! dispatched on `scs->allintra` (`:7127`, `md_config_process.c:924-930`). The
//! still envelope this port shipped first flattened the **allintra** arm into
//! three inline predicates in `pipeline.rs`:
//!
//! | flattened predicate | C it stood for |
//! |---|---|
//! | `preset >= 8 && full_sb` | `get_max_block_size_allintra` (`:7042`) |
//! | `preset <= 6` | `svt_aom_get_nsq_geom_level_allintra` (`:8240`) != 0 |
//! | `NsqCfg::for_preset_qp`'s base table | `svt_aom_get_nsq_search_level_allintra` (`:8363`) |
//!
//! This module replaces the flattening with a call into the tier-1-gated
//! ladders in [`crate::port_enc_mode_config`], and adds the VIDEO arm beside
//! it. The still path is byte-neutral **by construction** — the allintra arm
//! evaluates the same ladder the flattening was transcribed from — and the
//! `allintra_flattening_matches_*` tests below pin that entry-for-entry over
//! the whole preset x qp grid, with the old inline predicates kept verbatim as
//! the regression oracle.
//!
//! # Evidence
//!
//! The three ladders are EXPORTED C symbols and are already gated at tier 1
//! (`tests/c_parity_sig_deriv_leaf.rs`, `tests/c_parity_sig_deriv_common.rs`).
//! Nothing here re-transcribes them; this module is wiring plus the
//! flattening pins, so its own tier is that of the functions it calls.

use crate::port_enc_mode_config::enc_mode::M7;
use crate::port_enc_mode_config::{InputCoeffLvl, leaf};
use crate::sc_detect::ScArm;

/// C `scs->seq_qp_mod`, set unconditionally to 2 at `enc_handle.c:3994`.
/// Not arm-dependent — the still path's flattened offsets already assumed it.
pub(crate) const SEQ_QP_MOD: u8 = 2;

/// The `coeff_lvl` the port hands the VIDEO-arm ladders.
///
/// C leaves `pcs->coeff_lvl` at `INVALID_LVL` (`~0`, `definitions.h:288`) for a
/// video-mode **I-slice**: `md_config_process.c:898-902` runs
/// `derive_intra_coeff_level` only when `scs->allintra`, and
/// `derive_inter_coeff_level` only when `!rtc && slice_type != I_SLICE`. A
/// video KEY frame — the only video picture this port encodes today — falls
/// through both.
///
/// Every consumer in these two ladders tests `coeff_lvl` by EQUALITY against
/// `HIGH_LVL` or against `VLOW_LVL | LOW_LVL` (`:8216`, `:8254`), and
/// `INVALID_LVL` equals none of them, so it behaves exactly as `NORMAL_LVL`
/// there. `nsq_levels_treat_invalid_coeff_lvl_as_normal` in
/// `tests/c_parity_sig_deriv_leaf.rs` pins that against the real C symbols
/// rather than leaving it as a reading of the source.
pub(crate) const VIDEO_ISLICE_COEFF_LVL: InputCoeffLvl = InputCoeffLvl::Normal;

/// Whether the 64x64-variance cap of `ctx->max_block_size` can fire at all.
///
/// `pd0_pick_sb_partition_m6_eval` takes this as a boolean and applies the
/// variance compare itself ([`crate::pd0::max_block_size_allintra`]), because
/// the threshold is only ever finite on one arm at one preset band:
///
/// - **allintra** (`get_max_block_size_allintra`, `:7042`): `base_var_th_cap`
///   is `(uint16_t)~0` through M7 — a `u16` variance can never exceed it — and
///   7500 at M8+. Incomplete edge SBs return `super_block_size` uncapped.
/// - **video** (`get_max_block_size_default`, `:6991`): `ctx->max_block_size =
///   scs->super_block_size`. No cap, at any preset, ever.
///
/// `full_sb` is C's `sb_geom->width >= sb_size && sb_geom->height >= sb_size`.
#[must_use]
pub(crate) fn max_block_cap_active(arm: ScArm, preset: u8, full_sb: bool) -> bool {
    if !full_sb {
        return false;
    }
    match arm {
        ScArm::Allintra => i8::try_from(preset).is_ok_and(|m| m > M7),
        ScArm::Video { .. } => false,
    }
}

/// `ctx->disallow_4x4` for this arm — the ONE-preset fork at M3.
///
/// - **allintra** (`svt_aom_get_disallow_4x4_allintra`, `:8181`):
///   `enc_mode > M3`.
/// - **video** (`svt_aom_get_disallow_4x4_default`, `:8169`):
///   `enc_mode > M2`.
///
/// So at CLI preset 3 — and ONLY there, since M0..M2 allow 4x4 on both arms
/// and M4+ forbid it on both — a video-mode frame codes no 4x4 block where a
/// still one does. The port ran the allintra rule (`preset >= 4`) on both
/// arms, and at p3 that is the whole of `diag 72x88 q40`'s 22.257 %.
///
/// `preset` is clamped per [`crate::rate_arm::eff_enc_mode`] first, as C does
/// once in `svt_av1_enc_set_parameter`. Both clamps land above M3, so the
/// clamp cannot change this predicate today; it is applied because reading a
/// ladder at an unclamped `enc_mode` is the defect §1n names, not because a
/// cell needs it.
#[must_use]
pub(crate) fn disallow_4x4(arm: ScArm, preset: u8) -> bool {
    let m = i8::try_from(crate::rate_arm::eff_enc_mode(arm, preset)).unwrap_or(i8::MAX);
    match arm {
        ScArm::Allintra => leaf::get_disallow_4x4_allintra(m),
        ScArm::Video { .. } => leaf::get_disallow_4x4_default(m),
    }
}

/// `pcs->nsq_geom_level` for this arm — the level itself, so callers that
/// need `allow_HV4` / `min_nsq_block_size` (not just `enabled`) can ask.
#[must_use]
pub(crate) fn nsq_geom_level(arm: ScArm, preset: u8) -> u8 {
    let m = i8::try_from(preset).unwrap_or(i8::MAX);
    match arm {
        ScArm::Allintra => leaf::get_nsq_geom_level_allintra(m),
        ScArm::Video { .. } => leaf::get_nsq_geom_level_default(m, VIDEO_ISLICE_COEFF_LVL),
    }
}

/// `ctx->nsq_geom_ctrls.enabled` — whether NSQ shapes exist at all.
///
/// This is the predicate a ONE-FALSE boundary node consults: with geometry on
/// it keeps its single injected edge shape, with geometry off it force-splits.
#[must_use]
pub(crate) fn nsq_geom_enabled(arm: ScArm, preset: u8) -> bool {
    nsq_geom_level(arm, preset) != 0
}

/// `svt_aom_set_nsq_geom_ctrls` (`:8180`) — the `(allow_HV4, min_nsq_block_size)`
/// pair the funnel's `shapes_for_size` consumes, per geom level.
///
/// Level 1 also sets `allow_HVA_HVB = 1`, which the funnel cannot search: it
/// has no HorzA/HorzB/VertA/VertB candidate and `shape_children` is
/// `unreachable!` on them. Level 1 is reachable ONLY on the video arm at
/// preset 0 (`get_nsq_geom_level_default`, non-HIGH coeff_lvl), so that one
/// cell searches level 2's shape set. Named, not silent — see
/// `docs/nsq-port-map.md`.
#[must_use]
pub(crate) fn nsq_geom_shape_ctrls(level: u8) -> (bool, usize) {
    match level {
        0 => (false, 0),
        1 | 2 => (true, 0),
        3 => (false, 8),
        _ => (false, 16),
    }
}

/// `scs->qp_based_th_scaling_ctrls.nsq_qp_based_th_scaling` for this arm —
/// whether `set_nsq_search_ctrls`'s tail scales `component_multiple_th`,
/// `nsq_split_cost_th` and the `max_part0_to_part1_dev` offset by the qp
/// weight (`enc_mode_config.c:7110-7121`).
///
/// - **allintra** (`set_qp_based_th_scaling_ctrls_all_intra`,
///   `enc_handle.c:3838-3895`): 0 through M3, 1 from M4 up. Only presets
///   0..=3 ever reach `set_nsq_search_ctrls` on this arm (the ladder returns
///   0 from M4), so the still path is always unscaled — which is what the
///   flattened tail assumed.
/// - **video** (`set_qp_based_th_scaling_ctrls_default`, `:3788-3817`): 0 at
///   MR, 1 everywhere else. `MR` is unreachable from a `u8` preset, so it is
///   always 1 here.
#[must_use]
pub(crate) fn nsq_qp_based_th_scaling(arm: ScArm, preset: u8) -> bool {
    match arm {
        ScArm::Allintra => preset > 3,
        ScArm::Video { .. } => true,
    }
}

/// `pcs->nsq_search_level` for this arm.
///
/// The video arm's r0 modulation (`:8280-8288`) is inert for every
/// configuration this port encodes: `r0_gen` is set from `pcs->tpl_ctrls.enable`
/// (`initial_rc_process.c:734-744`), and `get_tpl` (`enc_handle.c:3665`)
/// returns 0 whenever `pred_structure == LOW_DELAY` — which is the GOP shape
/// the inter harness and the port's only multi-frame envelope use. So `r0_gen`
/// is passed as `false` and `r0` as 0.0, and the modulation branch is never
/// entered. If a RANDOM_ACCESS envelope is ever wired, this is the input to
/// revisit first.
#[must_use]
pub(crate) fn nsq_search_level(arm: ScArm, preset: u8, cli_qp: u32) -> u8 {
    let m = i8::try_from(preset).unwrap_or(i8::MAX);
    match arm {
        // `coeff_lvl` cannot matter on this arm: its only use is the
        // `enc_mode <= ENC_MR` clause, and MR is -1, structurally unreachable
        // from a `u8` preset (`rust/CLAUDE.md` envelope guard 5).
        ScArm::Allintra => {
            leaf::get_nsq_search_level_allintra(m, cli_qp, InputCoeffLvl::Normal, SEQ_QP_MOD)
        }
        ScArm::Video { is_islice } => leaf::get_nsq_search_level_default(
            m,
            VIDEO_ISLICE_COEFF_LVL,
            cli_qp,
            /*ppcs_temporal_layer_index=*/ 0,
            /*r0_gen=*/ false,
            /*r0=*/ 0.0,
            is_islice,
            /*temporal_layer_index=*/ 0,
            SEQ_QP_MOD,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The predicate `pipeline.rs` carried inline before this module existed,
    /// kept VERBATIM as the regression oracle for the still path.
    fn old_flattened_cap(preset: u8, full_sb: bool) -> bool {
        preset >= 8 && full_sb
    }

    /// Ditto for the NSQ-geometry predicate (two sites, same expression).
    fn old_flattened_geom_enabled(preset: u8) -> bool {
        preset <= 6
    }

    /// Ditto for `NsqCfg::for_preset_qp`'s level derivation — the base table
    /// plus the seq-qp-mod offsets, transcribed from the function as it stood.
    fn old_flattened_search_level(preset: u8, cli_qp: u32) -> u8 {
        let base: i32 = match preset {
            0 => 3,
            1 => 10,
            2 => 14,
            3 => 16,
            _ => 0,
        };
        if base == 0 {
            return 0;
        }
        let mut level = base;
        if cli_qp <= 39 {
            level = if level + 3 > 19 { 0 } else { level + 3 };
        } else if cli_qp <= 45 {
            level = if level + 2 > 19 { 0 } else { level + 2 };
        } else if cli_qp <= 48 {
            level = if level + 1 > 19 { 0 } else { level + 1 };
        } else if cli_qp > 59 {
            level = (level - 1).max(1);
        }
        level as u8
    }

    #[test]
    fn allintra_flattening_matches_the_ladder() {
        for preset in 0u8..=13 {
            for full_sb in [false, true] {
                assert_eq!(
                    max_block_cap_active(ScArm::Allintra, preset, full_sb),
                    old_flattened_cap(preset, full_sb),
                    "max-block cap p{preset} full_sb={full_sb}"
                );
            }
            assert_eq!(
                nsq_geom_enabled(ScArm::Allintra, preset),
                old_flattened_geom_enabled(preset),
                "nsq geom p{preset}"
            );
            // The flattened tail assumed factors of 1/1 unconditionally. That
            // is only sound where the still path actually builds an NsqCfg —
            // presets 0..=3, the band where the allintra search ladder is
            // non-zero. Pin BOTH halves: the flag is off there, and the ladder
            // is off wherever the flag is on.
            if nsq_search_level(ScArm::Allintra, preset, 40) != 0 {
                assert!(
                    !nsq_qp_based_th_scaling(ScArm::Allintra, preset),
                    "still tail must stay unscaled at p{preset}"
                );
            }
            for cli_qp in 0u32..=63 {
                assert_eq!(
                    nsq_search_level(ScArm::Allintra, preset, cli_qp),
                    old_flattened_search_level(preset, cli_qp),
                    "nsq search p{preset} q{cli_qp}"
                );
            }
        }
    }

    /// The allintra geom levels the flattening implied: 2 at presets 0..=3
    /// (`allow_HV4 = 1`, `min_nsq = 0` — exactly the pair `NsqCfg` hardcoded),
    /// 3 at 4..=6, 0 above.
    #[test]
    fn allintra_geom_ctrls_match_the_hardcoded_pair() {
        for preset in 0u8..=3 {
            assert_eq!(nsq_geom_level(ScArm::Allintra, preset), 2);
            assert_eq!(nsq_geom_shape_ctrls(2), (true, 0));
        }
        for preset in 4u8..=6 {
            assert_eq!(nsq_geom_level(ScArm::Allintra, preset), 3);
        }
        for preset in 7u8..=13 {
            assert_eq!(nsq_geom_level(ScArm::Allintra, preset), 0);
        }
    }

    /// Where the video arm actually departs from the still one — the whole
    /// point of the chunk, recorded so a future edit that flattens it back is
    /// a test failure rather than a silent regression.
    #[test]
    fn video_arm_departs_where_expected() {
        let v = ScArm::Video { is_islice: true };
        // max_block_size: the still arm caps at M8+, the video arm never caps.
        for preset in 0u8..=13 {
            assert!(
                !max_block_cap_active(v, preset, true),
                "video cap p{preset}"
            );
        }
        assert!(max_block_cap_active(ScArm::Allintra, 8, true));

        // NSQ geometry: the still arm switches OFF above M6, the video arm
        // never does (`get_nsq_geom_level_default` returns 1/2/3 only).
        for preset in 0u8..=13 {
            assert!(nsq_geom_enabled(v, preset), "video geom p{preset}");
        }
        assert!(!nsq_geom_enabled(ScArm::Allintra, 7));

        // NSQ search: the still arm is OFF from M4 up at EVERY qp (the
        // allintra base table is 0 there and the offsets short-circuit on 0),
        // while the video arm keeps searching.
        for preset in 4u8..=13 {
            for qp in 0u32..=63 {
                assert_eq!(
                    nsq_search_level(ScArm::Allintra, preset, qp),
                    0,
                    "still search p{preset} q{qp}"
                );
            }
        }
        // At q40 the video arm's `qp <= 43` offset is +2, which pushes M7's
        // base 18 and M8+'s 19 over 19 and back to 0 — NSQ search off. That is
        // C's own saturation rule (`level + 2 > 19 ? 0 : ...`), not a port
        // shortcut, so the departure shows at 4..=6 here...
        for preset in 4u8..=6 {
            assert_ne!(nsq_search_level(v, preset, 40), 0, "video search p{preset}");
        }
        // ...and at q55, where no offset applies, it extends to the top.
        for preset in 4u8..=13 {
            assert_ne!(
                nsq_search_level(v, preset, 55),
                0,
                "video search p{preset} q55"
            );
        }
    }

    /// The video ladder resolved at cli qp 40 (seq_qp_mod 2), spelled out so a
    /// change to either the base table or the offset arm shows up as a diff in
    /// a table rather than in behaviour only.
    ///
    /// Base (`svt_aom_get_nsq_search_level_default`, :8254): M0 2
    /// (`temporal_layer_index == 0` -> `is_base`), M1..M2 7, M3 9, M4 12,
    /// M5..M6 15, M7 18, M8+ 19. Then the seq-qp offset: M0..M6 take the
    /// `qp <= 45` arm (+2), M7+ the `qp <= 43` arm (also +2 at 40) — and the
    /// `level + 2 > 19 ? 0` saturation turns M7's 18 and M8+'s 19 into NSQ
    /// search OFF.
    #[test]
    fn video_search_levels_at_q40() {
        let v = ScArm::Video { is_islice: true };
        let expect = [4u8, 9, 9, 11, 14, 17, 17, 0, 0, 0, 0, 0, 0, 0];
        for (preset, want) in expect.iter().enumerate() {
            assert_eq!(nsq_search_level(v, preset as u8, 40), *want, "p{preset}");
        }
    }

    /// The same ladder at cli qp 55, where NO seq-qp offset applies (55 is
    /// above every `<=` bound and not `> 56`), so the base table shows
    /// through unmodified.
    #[test]
    fn video_search_levels_at_q55() {
        let v = ScArm::Video { is_islice: true };
        let expect = [2u8, 7, 7, 9, 12, 15, 15, 18, 19, 19, 19, 19, 19, 19];
        for (preset, want) in expect.iter().enumerate() {
            assert_eq!(nsq_search_level(v, preset as u8, 55), *want, "p{preset}");
        }
    }

    #[test]
    fn video_search_level_qp_arm_split_at_q44() {
        let v = ScArm::Video { is_islice: true };
        // M6 takes the `qp <= 45` arm: 15 + 2 = 17.
        assert_eq!(nsq_search_level(v, 6, 44), 17);
        // M7 takes the `qp <= 43` arm, so 44 falls through to `qp <= 48`:
        // 18 + 1 = 19.
        assert_eq!(nsq_search_level(v, 7, 44), 19);
    }
}

/// The VIDEO arm's PD0 configuration, as
/// `crate::pd0::pd0_pick_sb_partition_video` takes it:
/// `(pd0_level, coeff_rate_est_lvl, use_accurate_part_ctx)`.
///
/// The FIRST element is a resolved C `Pd0Level` (0..=6), not `pcs->pic_pd0_lvl`
/// (0..=8) — this function runs the whole chain
/// `set_pic_pd0_lvl_default` -> `set_pd0_ctrls` -> `pd0_detector`, for both
/// slice types. See the body for why the I_SLICE arm is NOT the identity.
///
/// * `pic_pd0_lvl` — `set_pic_pd0_lvl_default` (`enc_mode_config.c:8592`),
///   already ported and tier-1 gated as
///   [`crate::port_enc_mode_config::leaf::set_pic_pd0_lvl_default`]. At 240p
///   with C's unconditional `seq_qp_mod = 2` it is a flat 3 for M3..M7 and
///   `4 + ldp0_lvl_offset[qp_band]` from M8 up — 6 at CLI qp <= 27, 5 at
///   28..=39 and 40..=43, 4 above. **`seq_qp_mod` is load-bearing here**: at
///   the harness default of 0 the same call returns 4 at M9..M13, which is a
///   different PD0 level, so a probe that leaves it 0 measures a
///   configuration C never ships.
/// * `coeff_rate_est_lvl` — PD0's own `rate_est_level`
///   (`svt_aom_sig_deriv_enc_dec_pd0`, `:7355`) is 2 for `pd0_level <=
///   PD0_LVL_3`, 4 at PD0_LVL_4 and 0 above, raised to `MAX(that,
///   pcs->rate_est_level)` when non-zero — and `pcs->rate_est_level` is a flat
///   1 on the video arm. `set_rate_est_ctrls` then maps 0 -> 0, 2 -> 1, 4 -> 2.
/// * `use_accurate_part_ctx` — `enc_mode <= M8` (`:8955` / `:9937`).
///
/// `enc_mode` must already be [`crate::rate_arm::eff_enc_mode`]-clamped.
/// Which VIDEO picture the PD0 level is being derived for.
///
/// The distinction is not cosmetic: C's `pd0_detector`
/// (`enc_dec_process.c:2406`) gates EVERY one of its tests on
/// `slice_type != I_SLICE`, so on a key frame the picture level IS the
/// superblock level, and on an inter frame the ladder can step down several
/// levels before any search runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VideoPic {
    /// C `slice_type == I_SLICE`. The detector is a no-op below `PD0_LVL_6`.
    IntraSlice,
    /// A non-I slice whose list-0 reference is an I_SLICE — which is every
    /// inter frame the port's low-delay-P envelope can produce, because the
    /// only reference is the key frame.
    ///
    /// `ref_obj_l0->sb_intra[sb]` is then 1 for EVERY superblock (a key frame
    /// codes only intra blocks), so `use_ref_info` decides the whole ladder
    /// and no per-SB ME datum is read — see
    /// `port_pd0_detector::tests::an_all_intra_l0_reference_walks_every_level_down_to_lvl3`.
    ///
    /// NOT COVERED, and it is a real gap rather than an impossibility: an
    /// inter frame whose reference is itself an inter frame. C reads that
    /// reference's per-SB `sb_intra` (`coding_loop.c:1606`, set when any block
    /// in the SB is intra), which this port does not carry on its DPB entry.
    /// `InterOnInterRef` is deliberately ABSENT from this enum so that adding
    /// it is a compile error at every call site rather than a silent wrong
    /// level.
    InterOnIntraRef,
}

#[must_use]
pub(crate) fn video_pd0_params(
    enc_mode: u8,
    cli_qp: u32,
    luma_pixels: usize,
    pic: VideoPic,
) -> (u8, u8, bool) {
    let m = i8::try_from(enc_mode).unwrap_or(i8::MAX);
    let is_islice = pic == VideoPic::IntraSlice;
    let pic_pd0_lvl = leaf::set_pic_pd0_lvl_default(
        m,
        // Every video picture this port encodes is at temporal_layer_index 0.
        true,
        is_islice,
        false,
        VIDEO_ISLICE_COEFF_LVL,
        crate::port_enc_mode_config::ResolutionRange::from_luma_area(
            u32::try_from(luma_pixels).unwrap_or(u32::MAX),
        ),
        cli_qp,
        SEQ_QP_MOD,
        64,
    );
    // C `set_pd0_ctrls` then `pd0_detector`: `svt_aom_mode_decision_kernel`
    // runs the detector (enc_dec_process.c:2957) BEFORE
    // `svt_aom_sig_deriv_enc_dec_pd0` (:2977), so what PD0 searches with is
    // always the POST-detector level, on BOTH slice types.
    //
    // CORRECTED 2026-09-02. This used to take the identity on an I_SLICE,
    // justified by "every test is gated off there". That is true of C's
    // tests 2-4 and FALSE of test 1: `pd0_detector`'s first branch is gated
    // ON `slice_type == I_SLICE` (or `transition_present`) and demotes
    // `PD0_LVL_6`, because VERY_LIGHT_PD0 does INTER compensation only.
    // `set_pic_pd0_lvl_default` reaches `lpd0_lvl` 7 (= `PD0_LVL_6`) on a KEY
    // frame from 480p up, so the identity handed `crate::pd0` a level it has
    // no block cost for and the port PANICKED on 4 of the completion scan's
    // 64 video cells (568/576/1024/2048 square at preset 10, frame 0 — an
    // ordinary still-image configuration). C's own closing
    // `assert(IMPLIES(I_SLICE, pd0_level < PD0_LVL_6))` (`:2517`) holds
    // BECAUSE of that demote, not because the ladder never assigns the level.
    //
    // On an inter frame with an all-intra L0 reference the `use_ref_info`
    // arms walk down before any ME threshold is consulted, which is why no
    // per-SB input is needed on either arm.
    //
    // THE RESULT IS A `Pd0Level`, NOT AN `lpd0_lvl`. The two numberings
    // differ above 4 (`lpd0_lvl` 5 AND 6 both mean `PD0_LVL_5`; 7 AND 8 both
    // mean `PD0_LVL_6`), and every consumer — here and in `crate::pd0` —
    // reads it as the LEVEL.
    let pic_pd0_lvl = {
        let ctrls = crate::port_pd0_detector::pd0_ctrls_for_level(pic_pd0_lvl);
        crate::port_pd0_detector::pd0_detector(
            &ctrls,
            &crate::port_pd0_detector::Pd0SbInput {
                slice_type_is_intra: is_islice,
                // C reads `ref_obj_l0->sb_intra[sb]`, which an I_SLICE has no
                // reference for; `RefSbInfo::default()` is C's `l0_refs == 0`.
                ref_l0: if is_islice {
                    crate::port_pd0_detector::RefSbInfo::default()
                } else {
                    crate::port_pd0_detector::RefSbInfo { was_intra: Some(1) }
                },
                ..crate::port_pd0_detector::Pd0SbInput::default()
            },
        ) as u8
    };
    let pd0_rate_est_level = if pic_pd0_lvl <= 3 {
        2
    } else if pic_pd0_lvl == 4 {
        4
    } else {
        0
    };
    // `pcs->rate_est_level` is 1 at every preset on the video arm
    // (`crate::rate_arm::rate_est_level`), so the MAX only ever raises a 0,
    // which the `if (rate_est_level)` guard already excludes.
    let pd0_rate_est_level = if pd0_rate_est_level == 0 {
        0
    } else {
        pd0_rate_est_level.max(1)
    };
    let coeff_rate_est_lvl = match pd0_rate_est_level {
        0 => 0,
        2 => 1,
        4 => 2,
        other => unreachable!("PD0 rate_est_level {other} outside set_rate_est_ctrls' PD0 rows"),
    };
    (pic_pd0_lvl, coeff_rate_est_lvl, enc_mode <= 8)
}

/// The PD0 block-encode model, depth-early-exit threshold and PD0's OWN
/// coefficient-rate level for the REFINEMENT path
/// (`pd0_pick_sb_partition_m6_eval`, CLI presets 0..=8), as
/// `(mode, depth_early_exit_th, pd0_coeff_rate_est_lvl)`.
///
/// The fixed-tree path (preset >= 9) has its own entry point,
/// [`crate::pd0::pd0_pick_sb_partition_video`], because there the level,
/// the max block size and the NSQ geometry ALL fork; here only the level
/// does — `max_block_cap_active` is already false for both arms on this
/// path and `nsq_geom_enabled` is already arm-dispatched at the call sites.
///
/// `pd0_coeff_rate_est_lvl` is `None` for the allintra arm, meaning "keep the
/// frame-level `FunnelCfg::coeff_rate_est_lvl` the call site already passes".
/// C derives PD0's rate level from `pd0_level` and NOT from the frame's
/// (`svt_aom_sig_deriv_enc_dec_pd0`, enc_mode_config.c:7358-7366:
/// `pd0_level <= PD0_LVL_3 -> 2`, `<= PD0_LVL_4 -> 4`, else 0, then
/// `MAX(that, pcs->rate_est_level)`), and `set_rate_est_ctrls` maps
/// `2 -> coeff_rate_est_lvl 1` and `4 -> 2`. On the allintra arm the two
/// happen to agree at every preset this path serves, which is why the frame
/// value was correct there and is left alone.
///
/// `pred_depth_only` is C's `ctx->pic_pred_depth_only`
/// (`enc_mode_config.c:7095`: `depth_refinement_ctrls.mode ==
/// PD0_DEPTH_PRED_PART_ONLY`, i.e. depth-refinement level 10). It is what
/// picks `depth_early_exit_lvl` 1 over 2 (`:7229-7233`), so a level > LVL_1
/// with pred-depth-only takes `early_exit_th` 0 — which `Pd0Ctx::pick` spells
/// as `th = 1000` — rather than 900. MEASURED on the video arm at M8 through
/// C's own `SVT_PD0CFG_OUT` dump: `gradient 72x88 q40 p8` reports
/// `lvl=4 subres=1 exit_th=0 rate_lvl=2 pred_only=1`, and the sc_class5
/// contents at the same preset take depth-refinement level 6, so THEY get
/// `pred_only=0` and the 900 threshold.
///
/// **Not fully ported, and it returns the pre-existing allintra model rather
/// than a guess:** the video arm's `pic_pd0_lvl` is 0 at M0..M2 and 1 at M3
/// (`set_pic_pd0_lvl_default`), i.e. PD0_LVL_0 / PD0_LVL_1. PD0_LVL_1 IS the
/// allintra model, so M3 is exact; PD0_LVL_0's block cost differs from
/// `Pd0Mode::Lvl0` (which is the bd10 forcing, where `pcs->rate_est_level` is
/// 0 and the closed form applies, while a video frame's is 1 and C would
/// price the real coeff rate), so M0..M2 keep today's behaviour and are
/// listed as open in `docs/INTER-ENCODE-PLAN.md` §1f.
#[must_use]
pub(crate) fn refined_pd0_model(
    arm: ScArm,
    enc_mode: u8,
    cli_qp: u32,
    luma_pixels: usize,
    pred_depth_only: bool,
    pic: VideoPic,
) -> (crate::pd0::Pd0Mode, u128, Option<u8>) {
    match arm {
        ScArm::Allintra => (crate::pd0::Pd0Mode::Lvl1, 1000, None),
        ScArm::Video { .. } => {
            let (pic_pd0_lvl, _, _) = video_pd0_params(enc_mode, cli_qp, luma_pixels, pic);
            // `set_depth_early_exit_ctrls` (enc_mode_config.c:7229-7233).
            let th: u128 = if pic_pd0_lvl <= 1 || pred_depth_only {
                1000
            } else {
                900
            };
            match pic_pd0_lvl {
                3 => (crate::pd0::Pd0Mode::Lvl3, th, Some(1)),
                4 => (crate::pd0::Pd0Mode::Lvl4, th, Some(2)),
                // The unported block costs: `PD0_LVL_0..PD0_LVL_2`, and
                // `PD0_LVL_5`/`PD0_LVL_6`, which reach this fallback only
                // from the frame-level call at `pipeline.rs`'s PD0 setup —
                // the per-SB `>= M8` branch has its own entry point
                // (`pd0_pick_sb_partition_video_eval`) with a real LVL_5
                // model. `th` deliberately goes back to 1000, the
                // pre-existing value, because the model returned with it is
                // LVL_1's — pairing LVL_1's block cost with LVL_5's threshold
                // would be a third thing that is neither arm.
                _ => (crate::pd0::Pd0Mode::Lvl1, 1000, None),
            }
        }
    }
}

#[cfg(test)]
mod video_pd0_level_tests {
    use super::{SEQ_QP_MOD, VIDEO_ISLICE_COEFF_LVL, VideoPic, video_pd0_params};
    use crate::port_enc_mode_config::{ResolutionRange, leaf};

    /// The raw ladder value these tests are ABOUT, so a change in
    /// `set_pic_pd0_lvl_default` cannot make them pass vacuously (§5's
    /// positive-control rule: prove the input is what you think it is).
    fn raw_ladder(enc_mode: i8, cli_qp: u32, luma_pixels: u32) -> u8 {
        leaf::set_pic_pd0_lvl_default(
            enc_mode,
            true,
            true,
            false,
            VIDEO_ISLICE_COEFF_LVL,
            ResolutionRange::from_luma_area(luma_pixels),
            cli_qp,
            SEQ_QP_MOD,
            64,
        )
    }

    /// REGRESSION, 2026-09-02. `video_pd0_params` took the identity on an
    /// I_SLICE, which skipped `pd0_detector`'s FIRST test — the one branch of
    /// that function gated ON `slice_type == I_SLICE` rather than off it.
    ///
    /// OBSERVED BEFORE: `video_pd0_params(10, 32, 568*568, IntraSlice).0` was
    /// **7**, and `crate::pd0::video_pd0_mode` panicked on it — "video
    /// pic_pd0_lvl 7 selects a PD0 level this port has no block cost for" —
    /// on the KEY frame, i.e. frame 0 never reached disk. Four of the 64
    /// cells of `tools/inter_completion_scan.sh` (568/576/1024/2048 square at
    /// preset 10) crashed there.
    ///
    /// AFTER: 5 (`PD0_LVL_5`), and `gradient 568 568 32 10` frame 0 is
    /// byte-identical to C at 45 385 B.
    #[test]
    fn a_key_frame_above_360p_is_demoted_out_of_very_light_pd0() {
        // Positive control: the ladder really does hand out `lpd0_lvl` 7 here.
        // M10, R480p (568^2 = 322 624 >= 314 880), NORMAL coeff, CLI qp 32 ->
        // qp_band 1 -> `ldp0_lvl_offset[1]` = 2 with `seq_qp_mod` 2, so
        // `MIN(MAX_PD0_LVL, 5 + 2)` = 7. `set_pd0_ctrls` case 7 is
        // `PD0_LVL_6`.
        assert_eq!(
            raw_ladder(10, 32, 568 * 568),
            7,
            "the ladder no longer reaches lpd0_lvl 7 here — this test's premise is gone, not satisfied"
        );
        let (level, coeff_rate_est_lvl, _) =
            video_pd0_params(10, 32, 568 * 568, VideoPic::IntraSlice);
        // C `pd0_detector` (enc_dec_process.c:2413): VERY_LIGHT_PD0 supports
        // INTER compensation only, so an I_SLICE steps down to `PD0_LVL_5`.
        // That is also what makes C's own closing assert at :2517 hold.
        assert_eq!(level, 5, "an I_SLICE must never run PD0_LVL_6");
        // `pd0_level > PD0_LVL_4` -> PD0 rate_est_level 0 -> coeff lvl 0.
        assert_eq!(coeff_rate_est_lvl, 0);
    }

    /// The other side of the resolution class boundary, so the cell above
    /// cannot pass by demoting everything: 560^2 = 313 600 < 314 880 is
    /// R360p, where the M10 ladder gives `lpd0_lvl` 6 — a DIFFERENT number
    /// that resolves to the same `PD0_LVL_5`, and did so before this fix too.
    #[test]
    fn the_360p_side_of_the_boundary_reaches_lvl5_by_a_different_route() {
        assert_eq!(raw_ladder(10, 32, 560 * 560), 6);
        let (level, _, _) = video_pd0_params(10, 32, 560 * 560, VideoPic::IntraSlice);
        assert_eq!(level, 5);
    }

    /// The levels that are NOT supposed to move must not: `pd0_detector`'s
    /// tests 2-4 are all gated on `slice_type != I_SLICE`, so below
    /// `PD0_LVL_6` a key frame keeps the picture level. This is what makes
    /// the fix byte-neutral everywhere it was not crashing.
    #[test]
    fn a_key_frame_below_very_light_pd0_keeps_its_picture_level() {
        // M6 at 240p: the flat `3` row of `set_pic_pd0_lvl_default`.
        assert_eq!(raw_ladder(6, 40, 64 * 64), 3);
        assert_eq!(video_pd0_params(6, 40, 64 * 64, VideoPic::IntraSlice).0, 3);
        // M8 at 240p, CLI qp 40 -> qp_band 2 -> offset 1: `MIN(8, 3 + 1)` = 4.
        assert_eq!(raw_ladder(8, 40, 64 * 64), 4);
        assert_eq!(video_pd0_params(8, 40, 64 * 64, VideoPic::IntraSlice).0, 4);
    }
}
