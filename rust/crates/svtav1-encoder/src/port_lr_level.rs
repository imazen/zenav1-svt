//! Loop-restoration LEVEL derivation — the VIDEO-mode arm.
//!
//! # The gap this closes
//!
//! `restoration.rs::wn_filter_ctrls_allintra` ports
//! `svt_aom_get_wn_filter_level_allintra` + `svt_aom_set_wn_filter_ctrls`, and
//! `svt_aom_get_sg_filter_level_allintra` is 0 for every representable preset,
//! so the port has only ever had the ALL-INTRA level table.
//!
//! That is not the only selector. `pd_process.c:4935-4938` is
//!
//! ```text
//! allintra ? svt_aom_sig_deriv_multi_processes_allintra(..)
//!          : rtc_tune ? ..._rtc(..)
//!                     : ..._default(..)
//! ```
//!
//! and `scs->allintra` is set only when `intra_period_length == 0 || avif`
//! (`enc_handle.c:518`). Every video-mode frame — which is every frame of the
//! inter campaign — therefore takes the `_default` derivations, and those give
//! DIFFERENT levels:
//!
//! * Wiener (`svt_aom_get_wn_filter_level_default`, `enc_mode_config.c:1357`):
//!   `is_not_last_layer ? 4 : 0` at `<= M3`, `is_not_last_layer ? 5 : 0` at
//!   `<= M8`, else 0. Levels 4 and 5 differ ONLY in `use_chroma` (1 vs 0), so a
//!   video-mode frame at preset 4..8 runs **luma-only** Wiener where the
//!   all-intra table runs luma+chroma — and a LAST-LAYER frame gets no Wiener
//!   at all. That is a byte difference on a one-frame video-mode cell with no
//!   new algorithm required.
//! * SGR (`svt_aom_get_sg_filter_level_default`, `:1402`): 3 at `<= M3`, 0
//!   above (1 at `<= ENC_MR`, unreachable). Level 3 turns on the whole SGR
//!   search — see `svtav1_dsp::port_sgr`'s module doc.
//!
//! # Evidence: TIER 4, and here is exactly why
//!
//! `svt_aom_get_wn_filter_level_default`, `svt_aom_get_sg_filter_level_default`,
//! `svt_aom_set_wn_filter_ctrls` and `svt_aom_set_sg_filter_ctrls` are all
//! `static` in `enc_mode_config.c` — `nm` on the built oracle shows no symbol
//! for any of them. The only exported driver is
//! `svt_aom_sig_deriv_multi_processes_default`, which needs a fully-built
//! `SequenceControlSet` + `PictureControlSet` + `Av1Common` before it will run;
//! that is a real shim, not a thin adapter, and it is not built here. So these
//! are HAND-DERIVED FROM THE C SOURCE (`WORKING-ON-THIS.md` §4 tier 4) and the
//! tests below are transcription checks, not differentials. Anyone raising this
//! to tier 1 should add the `sig_deriv_multi_processes_default` shim rather
//! than trusting these tables.
//!
//! Neither C function is under `#if TUNE_*` or `SVT_HDR_MODE`; both were read
//! from the mainline `#else`-free bodies.

/// `WnFilterCtrls` (`enc_mode_config.c:1234`, all six fields).
///
/// This is a superset of `restoration.rs::WnFilterCtrls`, which omits
/// `use_prev_frame_coeffs` because no all-intra level sets it. Level 6 (video
/// mode) does, so the field is carried here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WnFilterCtrlsFull {
    pub enabled: bool,
    pub use_chroma: bool,
    /// 1 -> 7x7 luma taps, 2 -> 5x5 luma taps (chroma is always 5x5).
    pub filter_tap_lvl: u8,
    pub use_refinement: bool,
    pub max_one_refinement_step: bool,
    pub use_prev_frame_coeffs: bool,
}

/// `SgFilterCtrls` (`enc_mode_config.c:1295`).
///
/// `start_ep` / `end_ep` / `ep_inc` are per SEARCH LANE (index 0 and 1), NOT
/// per SGR radius — lane 0 sweeps `ep` coarsely over the whole range and lane 1
/// sweeps a narrow window. `refine` is the +-1 `xqd` refinement, per lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SgFilterCtrls {
    pub enabled: bool,
    pub use_chroma: bool,
    pub start_ep: [u8; 2],
    pub end_ep: [u8; 2],
    pub ep_inc: [u8; 2],
    pub refine: [bool; 2],
}

/// `INPUT_SIZE_8K_RANGE` — the resolution class at and above which both
/// restoration filters are force-disabled for memory reasons.
///
/// **CORRECTED 2026-09-01: this was 5 and 5 is `INPUT_SIZE_4K_RANGE`.** The C
/// enum (`definitions.h:1824-1831`) is 240p 0 / 360p 1 / 480p 2 / 720p 3 /
/// 1080p 4 / 4K 5 / 8K 6, which `port_enc_mode_config::ResolutionRange` and
/// `port_picstruct::INPUT_SIZE_360P_RANGE` already carry correctly. The wrong
/// pair was byte-inert only because this module was UNWIRED; at 4K it would
/// have killed both filters C keeps on, and under `fast_decode` it would have
/// killed SGR at 360p where C keeps it.
pub const INPUT_SIZE_8K_RANGE: u8 = 6;
/// `INPUT_SIZE_360p_RANGE` (`definitions.h:1825`). Was 0 — see above.
pub const INPUT_SIZE_360P_RANGE: u8 = 1;

/// Port of `svt_aom_get_wn_filter_level_default` (`enc_mode_config.c:1357`).
///
/// `is_not_last_layer` is the load-bearing argument the all-intra variant does
/// not have: on the highest temporal layer Wiener is OFF entirely.
pub fn wn_filter_level_default(enc_mode: u8, input_resolution: u8, is_not_last_layer: bool) -> u8 {
    let mut lvl = if enc_mode <= 3 {
        if is_not_last_layer { 4 } else { 0 }
    } else if enc_mode <= 8 {
        if is_not_last_layer { 5 } else { 0 }
    } else {
        0
    };
    if input_resolution >= INPUT_SIZE_8K_RANGE {
        lvl = 0;
    }
    lvl
}

/// Port of `svt_aom_get_wn_filter_level_allintra` (`enc_mode_config.c:1386`)
/// — kept here beside the video-mode arm so the two can be compared at a
/// glance. Values match `restoration.rs::wn_filter_ctrls_allintra`.
///
/// **CORRECTED 2026-09-01: this used to take an `input_resolution` and apply
/// the 8K force-off.** C's all-intra variant takes `EncMode` ALONE and has no
/// resolution clause — only the `_default` and `_rtc` variants do. Carrying
/// the clause here would have disabled Wiener at 8K all-intra, where C keeps
/// it on. Inert until now because nothing called this function.
#[must_use]
pub fn wn_filter_level_allintra(enc_mode: u8) -> u8 {
    if enc_mode <= 3 {
        3
    } else if enc_mode <= 6 {
        4
    } else {
        0
    }
}

/// Port of `svt_aom_get_sg_filter_level_default` (`enc_mode_config.c:1402`).
///
/// The `enc_mode <= ENC_MR` arm (level 1) is unreachable from this port —
/// `ENC_MR` is -1 and the preset is a `u8` — so it is written as the
/// `enc_mode <= 3 -> 3` arm plus a documented note, not as a branch that can
/// never be taken.
pub fn sg_filter_level_default(enc_mode: u8, input_resolution: u8, fast_decode: bool) -> u8 {
    // C: `if (enc_mode <= ENC_MR) 1;` — ENC_MR is -1, structurally unreachable
    // from a u8 preset (CLAUDE.md envelope guard 5).
    let mut lvl = if enc_mode <= 3 { 3 } else { 0 };
    if input_resolution >= INPUT_SIZE_8K_RANGE
        || (fast_decode && input_resolution > INPUT_SIZE_360P_RANGE)
    {
        lvl = 0;
    }
    lvl
}

/// Port of `svt_aom_get_sg_filter_level_rtc` (`enc_mode_config.c:1420`) —
/// unconditionally 0. Kept because a "level function that always returns 0" is
/// exactly the kind of thing a later reader deletes as dead and then
/// re-derives wrongly.
pub fn sg_filter_level_rtc(_input_resolution: u8, _fast_decode: bool) -> u8 {
    0
}

/// Port of `svt_aom_set_wn_filter_ctrls` (`enc_mode_config.c:1234`).
///
/// Level 0 leaves every field but `enabled` UNTOUCHED in C (the `case 0:` arm
/// sets only `ctrls->enabled = 0`). The port returns a fully-defaulted struct
/// instead; that is observably identical because every consumer gates on
/// `enabled` first, and it avoids carrying stale fields.
pub fn set_wn_filter_ctrls(wn_filter_lvl: u8) -> WnFilterCtrlsFull {
    match wn_filter_lvl {
        0 => WnFilterCtrlsFull::default(),
        1 => WnFilterCtrlsFull {
            enabled: true,
            use_chroma: true,
            filter_tap_lvl: 1,
            use_refinement: true,
            max_one_refinement_step: false,
            use_prev_frame_coeffs: false,
        },
        2 => WnFilterCtrlsFull {
            enabled: true,
            use_chroma: true,
            filter_tap_lvl: 1,
            use_refinement: true,
            max_one_refinement_step: true,
            use_prev_frame_coeffs: false,
        },
        3 => WnFilterCtrlsFull {
            enabled: true,
            use_chroma: true,
            filter_tap_lvl: 2,
            use_refinement: true,
            max_one_refinement_step: true,
            use_prev_frame_coeffs: false,
        },
        4 => WnFilterCtrlsFull {
            enabled: true,
            use_chroma: true,
            filter_tap_lvl: 2,
            use_refinement: false,
            max_one_refinement_step: true,
            use_prev_frame_coeffs: false,
        },
        5 => WnFilterCtrlsFull {
            enabled: true,
            use_chroma: false,
            filter_tap_lvl: 2,
            use_refinement: false,
            max_one_refinement_step: true,
            use_prev_frame_coeffs: false,
        },
        6 => WnFilterCtrlsFull {
            enabled: true,
            use_chroma: false,
            filter_tap_lvl: 2,
            use_refinement: false,
            max_one_refinement_step: true,
            use_prev_frame_coeffs: true,
        },
        other => panic!("unknown wn_filter_lvl {other}"),
    }
}

/// Port of `svt_aom_set_sg_filter_ctrls` (`enc_mode_config.c:1295`).
pub fn set_sg_filter_ctrls(sg_filter_lvl: u8) -> SgFilterCtrls {
    match sg_filter_lvl {
        0 => SgFilterCtrls::default(),
        1 => SgFilterCtrls {
            enabled: true,
            use_chroma: true,
            start_ep: [0, 0],
            end_ep: [16, 16],
            ep_inc: [1, 1],
            refine: [true, true],
        },
        2 => SgFilterCtrls {
            enabled: true,
            use_chroma: true,
            start_ep: [0, 4],
            end_ep: [16, 5],
            ep_inc: [1, 1],
            refine: [true, false],
        },
        3 => SgFilterCtrls {
            enabled: true,
            use_chroma: true,
            start_ep: [0, 4],
            end_ep: [16, 5],
            ep_inc: [8, 1],
            refine: [true, false],
        },
        4 => SgFilterCtrls {
            enabled: true,
            use_chroma: false,
            start_ep: [0, 4],
            end_ep: [16, 5],
            ep_inc: [8, 1],
            refine: [true, false],
        },
        other => panic!("unknown sg_filter_lvl {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of this module: the video-mode Wiener level is NOT the
    /// all-intra one, and the difference is `use_chroma` at presets 4..8 plus a
    /// hard off on the last temporal layer.
    #[test]
    fn video_mode_wiener_differs_from_allintra_where_the_gap_was_claimed() {
        // Preset 4..=6: allintra level 4 (chroma ON) vs default level 5
        // (chroma OFF) on a non-last layer.
        for preset in 4..=6u8 {
            let ai = set_wn_filter_ctrls(wn_filter_level_allintra(preset));
            let vid = set_wn_filter_ctrls(wn_filter_level_default(preset, 1, true));
            assert!(ai.use_chroma, "allintra preset {preset} should use chroma");
            assert!(
                !vid.use_chroma,
                "video-mode preset {preset} must be LUMA-ONLY Wiener"
            );
            assert!(ai.enabled && vid.enabled);
        }
        // Presets 7..=8: allintra is OFF entirely, video mode is ON (level 5).
        for preset in 7..=8u8 {
            assert_eq!(wn_filter_level_allintra(preset), 0);
            assert_eq!(wn_filter_level_default(preset, 1, true), 5);
        }
        // Last temporal layer: video-mode Wiener is off at every preset.
        for preset in 0..=13u8 {
            assert_eq!(
                wn_filter_level_default(preset, 1, false),
                0,
                "preset {preset} last-layer must disable Wiener"
            );
        }
    }

    #[test]
    fn video_mode_sgr_is_on_at_presets_0_to_3_only() {
        for preset in 0..=3u8 {
            let c = set_sg_filter_ctrls(sg_filter_level_default(preset, 1, false));
            assert!(c.enabled, "preset {preset} should enable SGR in video mode");
            assert!(c.use_chroma);
            assert_eq!(c.start_ep, [0, 4]);
            assert_eq!(c.end_ep, [16, 5]);
            assert_eq!(c.ep_inc, [8, 1]);
            assert_eq!(c.refine, [true, false]);
        }
        for preset in 4..=13u8 {
            assert_eq!(sg_filter_level_default(preset, 1, false), 0);
        }
    }

    #[test]
    fn eight_k_and_fast_decode_force_both_filters_off() {
        for preset in 0..=13u8 {
            assert_eq!(
                wn_filter_level_default(preset, INPUT_SIZE_8K_RANGE, true),
                0
            );
            assert_eq!(
                sg_filter_level_default(preset, INPUT_SIZE_8K_RANGE, false),
                0
            );
            // fast_decode disables SGR at anything above 360p, but NOT at 360p
            // and below — C's condition is
            // `fast_decode && !(input_resolution <= INPUT_SIZE_360p_RANGE)`.
            // 2 is 480p; this assert used to pass 1 while 1 was miscoded as
            // "above 360p", which is exactly the constant corrected above.
            assert_eq!(sg_filter_level_default(preset, 2, true), 0);
        }
        for preset in 0..=3u8 {
            assert_eq!(
                sg_filter_level_default(preset, INPUT_SIZE_360P_RANGE, true),
                3,
                "fast_decode must NOT disable SGR at 360p and below"
            );
        }
    }

    /// The `ep` sweep the search will walk at level 3: lane 0 is
    /// `0, 8, 16` (start 0, end 16, inc 8 — C's loop is `ep < end_ep`, so 16 is
    /// NOT visited) and lane 1 is `4` only.
    #[test]
    fn level_three_sweep_shape() {
        let c = set_sg_filter_ctrls(3);
        let lane0: Vec<u8> = (c.start_ep[0]..c.end_ep[0])
            .step_by(c.ep_inc[0] as usize)
            .collect();
        let lane1: Vec<u8> = (c.start_ep[1]..c.end_ep[1])
            .step_by(c.ep_inc[1] as usize)
            .collect();
        assert_eq!(lane0, vec![0, 8]);
        assert_eq!(lane1, vec![4]);
    }
}
