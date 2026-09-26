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
pub(crate) fn max_block_cap_active(arm: ScArm, preset: i8, full_sb: bool) -> bool {
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
pub(crate) fn disallow_4x4(arm: ScArm, preset: i8) -> bool {
    let m = i8::try_from(crate::rate_arm::eff_enc_mode(arm, preset)).unwrap_or(i8::MAX);
    match arm {
        ScArm::Allintra => leaf::get_disallow_4x4_allintra(m),
        ScArm::Video { .. } => leaf::get_disallow_4x4_default(m),
    }
}

/// `pcs->nsq_geom_level` for this arm — the level itself, so callers that
/// need `allow_HV4` / `min_nsq_block_size` (not just `enabled`) can ask.
#[must_use]
pub(crate) fn nsq_geom_level(
    arm: ScArm,
    preset: i8,
    reference: crate::reference::SvtReference,
) -> u8 {
    let m = i8::try_from(preset).unwrap_or(i8::MAX);
    match arm {
        ScArm::Allintra => leaf::get_nsq_geom_level_allintra(m),
        ScArm::Video { .. } => {
            leaf::get_nsq_geom_level_default(m, VIDEO_ISLICE_COEFF_LVL, reference)
        }
    }
}

/// `ctx->nsq_geom_ctrls.enabled` — whether NSQ shapes exist at all.
///
/// This is the predicate a ONE-FALSE boundary node consults: with geometry on
/// it keeps its single injected edge shape, with geometry off it force-splits.
#[must_use]
pub(crate) fn nsq_geom_enabled(
    arm: ScArm,
    preset: i8,
    reference: crate::reference::SvtReference,
) -> bool {
    nsq_geom_level(arm, preset, reference) != 0
}

/// `svt_aom_set_nsq_geom_ctrls` (`:8180`) — the `(allow_HV4, min_nsq_block_size)`
/// pair the funnel's `shapes_for_size` consumes, per geom level.
///
/// Level 1 additionally enables asymmetric shapes. `NsqCfg` carries that
/// flag separately and searches their geometry, pruning and redundant blocks.
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
///   MR, 1 everywhere else.
#[must_use]
pub(crate) fn nsq_qp_based_th_scaling(arm: ScArm, preset: i8) -> bool {
    match arm {
        ScArm::Allintra => preset > 3,
        ScArm::Video { .. } => preset > -1,
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
#[cfg(test)]
pub(crate) fn nsq_search_level(arm: ScArm, preset: i8, cli_qp: u32) -> u8 {
    nsq_search_level_with_coeff(
        arm,
        preset,
        cli_qp,
        crate::quant::CoeffLvl::Normal,
        0,
        crate::reference::SvtReference::Hybrid3115,
    )
}

/// The `quant::CoeffLvl` -> `InputCoeffLvl` bridge — the `InputCoeffLvl`
/// enum carries C's `INVALID_LVL` too, which a derived level never is.
pub(crate) fn input_coeff_lvl(level: crate::quant::CoeffLvl) -> InputCoeffLvl {
    match level {
        crate::quant::CoeffLvl::VLow => InputCoeffLvl::VLow,
        crate::quant::CoeffLvl::Low => InputCoeffLvl::Low,
        crate::quant::CoeffLvl::Normal => InputCoeffLvl::Normal,
        crate::quant::CoeffLvl::High => InputCoeffLvl::High,
    }
}

pub(crate) fn nsq_search_level_with_coeff(
    arm: ScArm,
    preset: i8,
    cli_qp: u32,
    coeff_level: crate::quant::CoeffLvl,
    // C `pcs->temporal_layer_index` — `get_nsq_search_level_default` reads it
    // for the M0 `is_base` row and the r0 modulation table
    // (enc_mode_config.c:8258-8290). 0 on a flat GOP's every picture.
    temporal_layer: u8,
    reference: crate::reference::SvtReference,
) -> u8 {
    let coeff = input_coeff_lvl(coeff_level);
    let m = i8::try_from(preset).unwrap_or(i8::MAX);
    match arm {
        // Research mode reads the actual picture coefficient class here.
        ScArm::Allintra => leaf::get_nsq_search_level_allintra(m, cli_qp, coeff, SEQ_QP_MOD),
        ScArm::Video { is_islice } => leaf::get_nsq_search_level_default(
            m,
            // C `derive_inter_coeff_level` runs only on non-I-slices
            // (md_config_process.c:898-903): a video I-slice keeps
            // `INVALID_LVL`, which every consumer's equality tests treat as
            // NORMAL (`VIDEO_ISLICE_COEFF_LVL`). Inter frames pass the real
            // `pcs->coeff_lvl` — the level offsets at :8296-8300 are live.
            if is_islice {
                VIDEO_ISLICE_COEFF_LVL
            } else {
                coeff
            },
            cli_qp,
            temporal_layer,
            /*r0_gen=*/ false,
            /*r0=*/ 0.0,
            is_islice,
            temporal_layer,
            SEQ_QP_MOD,
            reference,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The predicate `pipeline.rs` carried inline before this module existed,
    /// kept VERBATIM as the regression oracle for the still path.
    fn old_flattened_cap(preset: i8, full_sb: bool) -> bool {
        preset >= 8 && full_sb
    }

    /// Ditto for the NSQ-geometry predicate (two sites, same expression).
    fn old_flattened_geom_enabled(preset: i8) -> bool {
        preset <= 6
    }

    /// Ditto for `NsqCfg::for_preset_qp`'s level derivation — the base table
    /// plus the seq-qp-mod offsets, transcribed from the function as it stood.
    fn old_flattened_search_level(preset: i8, cli_qp: u32) -> u8 {
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
        for preset in 0i8..=13 {
            for full_sb in [false, true] {
                assert_eq!(
                    max_block_cap_active(ScArm::Allintra, preset, full_sb),
                    old_flattened_cap(preset, full_sb),
                    "max-block cap p{preset} full_sb={full_sb}"
                );
            }
            assert_eq!(
                nsq_geom_enabled(
                    ScArm::Allintra,
                    preset,
                    crate::reference::SvtReference::Hybrid3115
                ),
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
        for preset in 0i8..=3 {
            assert_eq!(
                nsq_geom_level(
                    ScArm::Allintra,
                    preset,
                    crate::reference::SvtReference::Hybrid3115
                ),
                2
            );
            assert_eq!(nsq_geom_shape_ctrls(2), (true, 0));
        }
        for preset in 4i8..=6 {
            assert_eq!(
                nsq_geom_level(
                    ScArm::Allintra,
                    preset,
                    crate::reference::SvtReference::Hybrid3115
                ),
                3
            );
        }
        for preset in 7i8..=13 {
            assert_eq!(
                nsq_geom_level(
                    ScArm::Allintra,
                    preset,
                    crate::reference::SvtReference::Hybrid3115
                ),
                0
            );
        }
    }

    /// Where the video arm actually departs from the still one — the whole
    /// point of the chunk, recorded so a future edit that flattens it back is
    /// a test failure rather than a silent regression.
    #[test]
    fn video_arm_departs_where_expected() {
        let v = ScArm::Video { is_islice: true };
        // max_block_size: the still arm caps at M8+, the video arm never caps.
        for preset in 0i8..=13 {
            assert!(
                !max_block_cap_active(v, preset, true),
                "video cap p{preset}"
            );
        }
        assert!(max_block_cap_active(ScArm::Allintra, 8, true));

        // NSQ geometry: the still arm switches OFF above M6, the video arm
        // never does (`get_nsq_geom_level_default` returns 1/2/3 only).
        for preset in 0i8..=13 {
            assert!(
                nsq_geom_enabled(v, preset, crate::reference::SvtReference::Hybrid3115),
                "video geom p{preset}"
            );
        }
        assert!(!nsq_geom_enabled(
            ScArm::Allintra,
            7,
            crate::reference::SvtReference::Hybrid3115
        ));

        // NSQ search: the still arm is OFF from M4 up at EVERY qp (the
        // allintra base table is 0 there and the offsets short-circuit on 0),
        // while the video arm keeps searching.
        for preset in 4i8..=13 {
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
        for preset in 4i8..=6 {
            assert_ne!(nsq_search_level(v, preset, 40), 0, "video search p{preset}");
        }
        // ...and at q55, where no offset applies, it extends to the top.
        for preset in 4i8..=13 {
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
            assert_eq!(nsq_search_level(v, preset as i8, 40), *want, "p{preset}");
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
            assert_eq!(nsq_search_level(v, preset as i8, 55), *want, "p{preset}");
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
/// `(pd0_level, coeff_rate_est_lvl, use_accurate_part_ctx, subres_step)`.
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
///   Sourced from the ported `sig_deriv_enc_dec_pd0` output rather than a
///   second transcription of the same ladder.
/// * `subres_step` — `ctx->subres_ctrls.step` from the same call's
///   `subres_level` ladder (`:7322-7357`): 0 below `PD0_LVL_3` / on partial
///   SBs / with 4x4 allowed, the `cost_64x64 < compute_subres_th` check at
///   LVL_3-4, the `disallow_8x8 || depth-removal` test at LVL_5+ on non-leaf
///   pictures, and a flat 2 at LVL_5+ on a leaf picture.
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
///
/// The frame-level inputs to `pd0_detector` that are not per-superblock data,
/// bundled so `encode_tile_rows` takes one argument.
///
/// `l{0,1}_sb_intra` is `ref_pic_ptr_array[REF_LIST_{0,1}][0]`'s
/// `EbReferenceObject::sb_intra` — `Some` only while C's three guards admit
/// the reference (enc_dec_process.c:2144-2168): a nonzero
/// `ref_list{0,1}_count_try`, a same-size reference
/// (`svt_aom_is_ref_same_size`, enc_mode_config.c:2857), and
/// `tmp_layer_idx <= temporal_layer_index`. The CALLER folds all three into
/// the `Option`, so `None` here is exactly C's `l{0,1}_refs == 0`.
/// The usable colocated reference's per-SB statistics — the `Some`/`None` on
/// [`Pd0DetFrame::l0`]/[`Pd0DetFrame::l1`] folds in C's `count_try` +
/// `is_ref_same_size` + `tmp_layer_idx <= temporal_layer` gates, so a present
/// entry is exactly C's "`l{0,1}_refs == 1`" arm of both `pd0_detector` and
/// `lpd1_detector_*`.
#[derive(Clone, Copy, Default)]
pub(crate) struct RefSbStats<'a> {
    /// Whether the list's index-0 reference is usable
    /// (`svt_aom_is_ref_same_size`, enc_mode_config.c:2857, with the
    /// `count_try` and `tmp_layer_idx <= temporal_layer` gates folded in) —
    /// `svt_aom_sig_deriv_enc_dec_light_pd1_default`'s `is_ref_l0_avail`.
    /// `false` on the `Default` the caller returns when no ref qualifies.
    pub avail: bool,
    /// `ref_obj->sb_intra` — `pd0_detector`'s `use_ref_info` and both
    /// `lpd1_detector_*` read it.
    pub sb_intra: Option<&'a [u8]>,
    /// `ref_obj->sb_skip` — `lpd1_detector_skip_pd0`'s score only.
    pub sb_skip: Option<&'a [u8]>,
    /// `ref_obj->sb_me_64x64_dist` — `lpd1_detector_skip_pd0`'s score only.
    pub me_64x64_dist: Option<&'a [u32]>,
    /// `ref_obj->sb_me_8x8_cost_var` — `lpd1_detector_skip_pd0`'s score only.
    pub me_8x8_cost_var: Option<&'a [u32]>,
    /// `ref_obj->sb_64x64_mvp` — `svt_aom_sig_deriv_enc_dec_light_pd1_default`
    /// reads it (enc_mode_config.c:7403/7603); not consulted by either
    /// detector.
    pub mvp_64x64: Option<&'a [u8]>,
    /// `ref_obj->slice_type == I_SLICE` — `lpd1_detector_skip_pd0`'s score
    /// adds a flat 10 for an intra reference.
    pub is_islice: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct Pd0DetFrame<'a> {
    /// C `ppcs->transition_present == 1`.
    pub transition_present: bool,
    /// C `!frame_is_leaf(ppcs)` — `ppcs->update_type != LF_UPDATE`
    /// (enc_mode_config.h:113). TRUE on a KEY frame (`KF_UPDATE` is not a
    /// leaf), false on a flat low-delay GOP's inter frames — which is why a
    /// key frame's `PD0_LVL_5` takes the `is_not_last_layer` arm of the
    /// subres ladder while an inter leaf goes straight to level 2.
    pub is_not_last_layer: bool,
    /// C `pcs->ref_intra_percentage` (`get_ref_intra_percentage`,
    /// rc_process.c:66).
    pub ref_intra_percentage: u8,
    /// List 0's usable colocated reference, per [`RefSbStats`].
    pub l0: RefSbStats<'a>,
    /// List 1's.
    pub l1: RefSbStats<'a>,
}

/// The `svt_aom_sig_deriv_enc_dec_pd0` (enc_mode_config.c:7207) inputs that
/// [`Pd0SbInput`] does not already carry — everything the per-superblock
/// `subres_level` ladder and `rate_est_level` derivation read beyond the
/// detector's own fields.
#[derive(Clone, Copy, Default)]
pub(crate) struct Pd0SigDerivInput {
    /// `!frame_is_leaf(ppcs)` — [`Pd0DetFrame::is_not_last_layer`].
    pub is_not_last_layer: bool,
    /// `ctx->pic_pred_depth_only` — level-10 depth refinement
    /// (`PD0_DEPTH_PRED_PART_ONLY`).
    pub pic_pred_depth_only: bool,
    /// `ctx->disallow_4x4` as `set_depth_removal_level_controls` left it for
    /// THIS superblock — it can only set the pic-level flag, never clear it.
    /// The pic value (`pic_disallow_4x4`) on a key frame, where the
    /// depth-removal call does not run.
    pub disallow_4x4: bool,
    /// `ctx->disallow_8x8` — `svt_aom_get_disallow_8x8_default` on the video
    /// arm (enc_mode_config.c:7122).
    pub disallow_8x8: bool,
    /// `ppcs->b64_geom[ctx->sb_index].is_complete_b64` — measured against the
    /// ALIGNED frame dims (`b64_geom_init` takes `pcs->aligned_width`,
    /// pcs.c:1490), NOT the SB-extent-padded canvas.
    pub b64_is_complete: bool,
    /// `ctx->depth_removal_ctrls` for this superblock; all-zero on a key
    /// frame (`set_depth_removal_level_controls` returns `enabled = 0` on an
    /// I_SLICE, so nothing was accumulated).
    pub depth_removal: crate::port_enc_mode_config::common::DepthRemovalCtrls,
    /// `ctx->fast_lambda_md[EB_8_BIT_MD]` — this superblock's
    /// `av1_lambda_assign_md` output. Consulted only by the
    /// `pd0_level <= PD0_LVL_4` non-islice arm's `cost_64x64 <
    /// compute_subres_th` check; a key frame's islice arm never reaches it.
    pub fast_lambda_8bit: u32,
    /// `ppcs->me_8x8_distortion[ctx->sb_index]` — read only by the
    /// `PD0_LVL_6` `parent_cost_bias` arm.
    pub me_8x8_distortion: u32,
    /// `ppcs->frm_hdr.quantization_params.base_q_idx` — same.
    pub base_q_idx: u32,
    /// `scs->super_block_size`.
    pub super_block_size: u32,
}

#[must_use]
pub(crate) fn video_pd0_params(
    enc_mode: i8,
    cli_qp: u32,
    luma_pixels: usize,
    // The frame's `pcs->coeff_lvl` as `crate::quant::derive_inter_coeff_level`
    // derived it — real only on an INTER frame. On a video I-slice C leaves it
    // at `INVALID_LVL`, which lands on `set_pic_pd0_lvl_default`'s `else` arms
    // — NOT the `NORMAL_LVL` arms, so the `VIDEO_ISLICE_COEFF_LVL` stand-in
    // that is sound in the NSQ ladders is wrong here.
    coeff_lvl: InputCoeffLvl,
    // C `ppcs->temporal_layer_index` — `set_pic_pd0_lvl_default`'s `is_base`
    // (enc_mode_config.c:8594). 0 on a flat GOP's every picture.
    temporal_layer: u8,
    // C `pcs->hbd_md != 0` — `set_pd0_ctrls` (enc_mode_config.c:5415) reads
    // `ctx->hbd_md` BEFORE the PD0 pass forces it to 0, so a nonzero value
    // short-circuits `pd0_level = PD0_LVL_0` and the `pd0_level > PD0_LVL_0`
    // gate (enc_dec_process.c:2957) skips `pd0_detector` entirely: the
    // ladder and the per-SB demote below never run on such a frame.
    hbd_md: bool,
    sb: &crate::port_pd0_detector::Pd0SbInput,
    sig: &Pd0SigDerivInput,
    // `(pd0_level, coeff_rate_est_lvl, use_accurate_part_ctx, subres_step,
    // parent_cost_bias)` — the last is `ctx->parent_cost_bias`, off 1000
    // only for the inter PD0_LVL_6 arm that consumes it.
) -> (u8, u8, bool, u32, u32) {
    let m = enc_mode;
    let is_islice = sb.slice_type_is_intra;
    let pic_pd0_lvl = leaf::set_pic_pd0_lvl_default(
        m,
        // `ppcs->temporal_layer_index == 0` (enc_mode_config.c:8594).
        temporal_layer == 0,
        is_islice,
        false,
        if is_islice {
            InputCoeffLvl::Invalid
        } else {
            coeff_lvl
        },
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
    // The detector is PER-SUPERBLOCK: `sb` carries this SB's own
    // `ref_obj->{sb_intra}` view (an all-intra L0 reference — a key frame —
    // walks `use_ref_info` down before any ME threshold is consulted; an
    // inter reference demotes only the superblocks that reference
    // intra-coded ones), its `ppcs->me_*` statistics, and the already-coded
    // left/top neighbours' `sb_intra`/`sb_skip`.
    //
    // THE RESULT IS A `Pd0Level`, NOT AN `lpd0_lvl`. The two numberings
    // differ above 4 (`lpd0_lvl` 5 AND 6 both mean `PD0_LVL_5`; 7 AND 8 both
    // mean `PD0_LVL_6`), and every consumer — here and in `crate::pd0` —
    // reads it as the LEVEL.
    //
    // `hbd_md` arm: `set_pd0_ctrls` (enc_mode_config.c:5415-5418) runs
    // `if (ctx->hbd_md) { pd0_level = PD0_LVL_0; return; }` — the picture
    // ladder's value never reaches `pd0_ctrls`, and `pd0_detector`'s
    // `pd0_level > PD0_LVL_0` gate (enc_dec_process.c:2957) keeps the
    // detector off, so the resolved level is a flat 0.
    let pic_pd0_lvl = if hbd_md {
        crate::port_pd0_detector::Pd0Level::Lvl0 as u8
    } else {
        let ctrls = crate::port_pd0_detector::pd0_ctrls_for_level(pic_pd0_lvl);
        crate::port_pd0_detector::pd0_detector(&ctrls, sb) as u8
    };
    // `svt_aom_sig_deriv_enc_dec_pd0` (enc_mode_config.c:7207) — the per-SB
    // signal ladder C runs right after the detector. Consumed here:
    // `rate_est.coeff_rate_est_lvl` (PD0's own coeff-rate level, below) and
    // `subres.step` (the residual sub-sampling step `Pd0Ctx` prices with).
    // The rest of `Pd0Signals` C stores on the context for PD1 consumers
    // that are not ported on this path yet.
    //
    // `pcs->rate_est_level` is a flat 1 on the video arm
    // (`crate::rate_arm::rate_est_level`), `pd0_cost_bias_weight` is 0 through
    // M12 (enc_mode_config.c:9654: `enc_mode <= ENC_M12 ? 0 : 600` — read
    // only by the LVL_6 `parent_cost_bias` arm this port does not consume),
    // `rtc_tune`/`allintra` are false on the video arm by definition, and
    // `hbd_md` arrives as the `hbd_md` parameter — applied above as the
    // `set_pd0_ctrls` force to `PD0_LVL_0` (the level `sig_deriv` resolves
    // the rest of the signals under), and as `pcs_hbd_md` for
    // `pd0_use_src_samples`.
    let signals = crate::port_enc_mode_config::pd0::sig_deriv_enc_dec_pd0(
        crate::port_enc_mode_config::pd0::Pd0Inputs {
            pd0_level: pic_pd0_lvl,
            is_islice,
            allintra: false,
            rtc_tune: false,
            is_not_last_layer: sig.is_not_last_layer,
            enc_mode,
            transition_present: sb.transition_present,
            pic_pred_depth_only: sig.pic_pred_depth_only,
            // `ctx->hbd_md` is already forced to 0 when
            // `svt_aom_sig_deriv_enc_dec_pd0` runs (enc_dec_process.c:2965-2977
            // window), so the `fast_lambda` select reads the 8-bit lambda even
            // on a 10-bit frame. `pcs->hbd_md` is NOT forced — it stays at the
            // frame's derived value, which is what `pd0_use_src_samples`
            // (:7309) reads.
            ctx_hbd_md: false,
            pcs_hbd_md: hbd_md,
            fast_lambda_8bit: sig.fast_lambda_8bit,
            fast_lambda_10bit: 0,
            me_64x64_distortion: sb.me_64x64_distortion,
            me_8x8_cost_variance: sb.me_8x8_cost_variance,
            me_8x8_distortion: sig.me_8x8_distortion,
            base_q_idx: sig.base_q_idx,
            pd0_cost_bias_weight: if enc_mode > 12 { 600 } else { 0 },
            rate_est_level: 1,
            disallow_4x4: sig.disallow_4x4,
            disallow_8x8: sig.disallow_8x8,
            depth_removal_enabled: sig.depth_removal.enabled != 0,
            disallow_below_16x16: sig.depth_removal.disallow_below_16x16 != 0,
            disallow_below_32x32: sig.depth_removal.disallow_below_32x32 != 0,
            disallow_below_64x64: sig.depth_removal.disallow_below_64x64 != 0,
            b64_is_complete: sig.b64_is_complete,
            super_block_size: sig.super_block_size,
        },
    );
    // `None` is C's `assert(0)` arm — an out-of-domain derived level, which
    // the in-range `pd0_level` here can never produce (every level 0..=6 maps
    // inside each table's domain). Fall back to the zero step/level C's
    // zeroed context would hold rather than panic on a proven-unreachable arm.
    let (coeff_rate_est_lvl, subres_step, parent_cost_bias) = signals.map_or((0, 0, 1000), |s| {
        (
            s.rate_est.coeff_rate_est_lvl,
            u32::from(s.subres.step),
            u32::from(s.parent_cost_bias),
        )
    });
    (
        pic_pd0_lvl,
        coeff_rate_est_lvl,
        enc_mode <= 8,
        subres_step,
        parent_cost_bias,
    )
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
    // The RESOLVED per-superblock `Pd0Level` — `set_pic_pd0_lvl_default` ->
    // `set_pd0_ctrls` -> `pd0_detector`, all run by [`video_pd0_params`]
    // per superblock because the detector demotes per superblock
    // (`md_ctx->pd0_ctrls.pd0_level` is per-SB in C, not per picture).
    pic_pd0_lvl: u8,
    pred_depth_only: bool,
) -> (crate::pd0::Pd0Mode, u128, Option<u8>) {
    match arm {
        ScArm::Allintra => (crate::pd0::Pd0Mode::Lvl1, 1000, None),
        ScArm::Video { .. } => {
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
    use super::{Pd0SigDerivInput, SEQ_QP_MOD, video_pd0_params};
    use crate::port_enc_mode_config::{InputCoeffLvl, ResolutionRange, leaf};
    use crate::port_pd0_detector::Pd0SbInput;

    /// The raw ladder value these tests are ABOUT, so a change in
    /// `set_pic_pd0_lvl_default` cannot make them pass vacuously (§5's
    /// positive-control rule: prove the input is what you think it is).
    ///
    /// `coeff_lvl` is `Invalid` — the value `pcs->coeff_lvl` really holds on
    /// a video I-slice (`md_config_process.c:898`), NOT `VIDEO_ISLICE_COEFF_LVL`:
    /// in this ladder `INVALID_LVL` takes the `else` arms, which are not the
    /// `NORMAL_LVL` arms.
    fn raw_ladder(enc_mode: i8, cli_qp: u32, luma_pixels: u32) -> u8 {
        leaf::set_pic_pd0_lvl_default(
            enc_mode,
            true,
            true,
            false,
            crate::port_enc_mode_config::InputCoeffLvl::Invalid,
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
        // Positive control for the OTHER side of the resolution class: a
        // 560^2 key frame at M10/INVALID reaches `lpd0_lvl` 7 (`MIN(7, 5+2)`
        // on the <=360p `else` arm), which IS `PD0_LVL_6` — the level that
        // must not survive an I_SLICE.
        assert_eq!(
            raw_ladder(10, 32, 560 * 560),
            7,
            "the ladder no longer reaches lpd0_lvl 7 here — this test's premise is gone, not satisfied"
        );
        let (level, coeff_rate_est_lvl, ..) = video_pd0_params(
            10,
            32,
            560 * 560,
            // `pcs->coeff_lvl` on a video I-slice is INVALID_LVL; the
            // function resolves that itself, so this argument is inert here.
            crate::port_enc_mode_config::InputCoeffLvl::Invalid,
            0,
            false,
            &Pd0SbInput {
                slice_type_is_intra: true,
                ..Pd0SbInput::default()
            },
            &Pd0SigDerivInput::default(),
        );
        // C `pd0_detector` (enc_dec_process.c:2413): VERY_LIGHT_PD0 supports
        // INTER compensation only, so an I_SLICE steps down to `PD0_LVL_5`.
        // That is also what makes C's own closing assert at :2517 hold.
        assert_eq!(level, 5, "an I_SLICE must never run PD0_LVL_6");
        // `pd0_level > PD0_LVL_4` -> PD0 rate_est_level 0 -> coeff lvl 0.
        assert_eq!(coeff_rate_est_lvl, 0);
    }

    /// The >480p side of the same cell: `INVALID_LVL` takes the M10 ladder's
    /// `else` arm, `MIN(7, 3+2)` = 5 — a DIFFERENT `lpd0_lvl` that resolves
    /// to the same `PD0_LVL_5` without needing the demote at all.
    #[test]
    fn the_480p_side_of_the_boundary_starts_at_lvl5_directly() {
        assert_eq!(raw_ladder(10, 32, 568 * 568), 5);
        let (level, ..) = video_pd0_params(
            10,
            32,
            568 * 568,
            crate::port_enc_mode_config::InputCoeffLvl::Invalid,
            0,
            false,
            &Pd0SbInput {
                slice_type_is_intra: true,
                ..Pd0SbInput::default()
            },
            &Pd0SigDerivInput::default(),
        );
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
        assert_eq!(
            video_pd0_params(
                6,
                40,
                64 * 64,
                crate::port_enc_mode_config::InputCoeffLvl::Invalid,
                0,
                false,
                &Pd0SbInput {
                    slice_type_is_intra: true,
                    ..Pd0SbInput::default()
                },
                &Pd0SigDerivInput::default()
            )
            .0,
            3
        );
        // M8 at 240p, CLI qp 40 -> qp_band 2 -> offset 1: `MIN(8, 3 + 1)` = 4.
        assert_eq!(raw_ladder(8, 40, 64 * 64), 4);
        assert_eq!(
            video_pd0_params(
                8,
                40,
                64 * 64,
                crate::port_enc_mode_config::InputCoeffLvl::Invalid,
                0,
                false,
                &Pd0SbInput {
                    slice_type_is_intra: true,
                    ..Pd0SbInput::default()
                },
                &Pd0SigDerivInput::default()
            )
            .0,
            4
        );
    }

    /// C `set_pd0_ctrls` (enc_mode_config.c:5415): `if (ctx->hbd_md) {
    /// pd0_level = PD0_LVL_0; return; }` — a 10-bit frame bypasses the
    /// ladder AND the detector, so a video I-slice resolves a flat 0 where
    /// the same SB at 8-bit resolves 3.
    #[test]
    fn hbd_md_forces_pd0_lvl0_regardless_of_ladder() {
        // Positive control: the non-hbd arm still resolves the ladder's 3.
        assert_eq!(raw_ladder(6, 40, 64 * 64), 3);
        let sb = Pd0SbInput {
            slice_type_is_intra: true,
            ..Pd0SbInput::default()
        };
        let sig = Pd0SigDerivInput::default();
        let bd10 = video_pd0_params(6, 40, 64 * 64, InputCoeffLvl::Invalid, 0, true, &sb, &sig).0;
        let bd8 = video_pd0_params(6, 40, 64 * 64, InputCoeffLvl::Invalid, 0, false, &sb, &sig).0;
        assert_eq!(bd10, 0, "hbd_md must force PD0_LVL_0");
        assert_eq!(bd8, 3, "the 8-bit arm keeps the ladder level");
    }
}
