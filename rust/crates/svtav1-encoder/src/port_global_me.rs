//! The FRAME-LEVEL half of `Codec/global_me.c` — the derivation that decides
//! whether C's global-motion search runs at all, and how many references it
//! runs over.
//!
//! # Why this file exists, and what it deliberately does NOT do
//!
//! [`crate::port_global_motion`], [`crate::port_ransac`] and
//! [`crate::port_gm_correspondence`] together port the per-reference SEARCH
//! (`compute_global_motion`'s correspondence -> RANSAC -> refine -> convert
//! chain). Nothing ported the function ABOVE them,
//! `svt_aom_global_motion_estimation` (`global_me.c:137`), which is where C
//! decides whether to call `compute_global_motion` even once. Without it the
//! pipeline could only ask "is `gm_level` non-zero?", which is a question about
//! the PRESET, not about this frame — and answering an inter frame at preset
//! <= 4 with "C codes a model here" is wrong on every cell measured so far.
//!
//! MEASURED 2026-09-05 with the `SVT_GM_OUT` interposer on C's own
//! `svt_aom_global_motion_estimation` (`tools/capture_c_trace/wrap_recon.c`),
//! 2-frame low-delay P at preset 2, q40:
//!
//! | cell | `avg_me_sad` | `is_gm_on` |
//! |---|---|---|
//! | gradient / diag / screen, 64..512 | 0 | 0 |
//! | `crop:` CID22 photo 256 / 512, shift 3 / 13 / 37 | 0 | 0 |
//! | `crop:` CID22 photo 256 / 512 **with a 33/32 zoom** | 2 | **1** (ROTZOOM) |
//!
//! The gate is `average_me_sad = sum(rc_me_distortion) / (w*h)`, an INTEGER
//! divide: a pure translation is exactly what open-loop ME finds, so the
//! residual SAD floors the average to 0 and C skips the search entirely. Every
//! reference then keeps the IDENTITY model this port already writes. That is a
//! DERIVED result, not an assumption, and it is what lets the pipeline encode
//! those cells instead of refusing them.
//!
//! The SEARCH itself is still not wired: when this derivation says C would run
//! one, the pipeline refuses. Lifting that needs `compute_global_motion` +
//! `svt_aom_gm_get_params_cost` and byte-parity evidence for the model coding,
//! neither of which exists yet.
//!
//! # Evidence: TIER 4, joined against a frame-level C dump
//!
//! `svt_aom_global_motion_estimation` is exported but takes a
//! `PictureParentControlSet*` whose `pa_me_data`, `rc_me_distortion`,
//! `rc_me_allow_gm`, `b64_geom` and `gm_ctrls` would all have to be assembled
//! for a shim — the same reason [`crate::port_gm_correspondence`] is tier 4.
//! So this is hand-derived from the C source and joined against C's own
//! `GMFRAME` line (`avg_me_sad`, `total_gm_sbs`, `ds`, `is_gm_on`,
//! `ctrls=...`) by `tools/gm_join_gate.sh`, which is a per-cell differential
//! against the real encoder rather than a self-written expectation.

use crate::port_enc_mode_config::ctrls::GmControls;

/// C `GM_LEVEL` (`definitions.h:256`) — the downsampling mode `gm_ctrls`
/// carries and `svt_aom_global_motion_estimation` resolves.
pub mod gm_level {
    /// `GM_FULL` — search at full resolution.
    pub const FULL: u8 = 0;
    /// `GM_DOWN` — 1/2 in each dimension.
    pub const DOWN: u8 = 1;
    /// `GM_DOWN16` — 1/4 in each dimension.
    pub const DOWN16: u8 = 2;
    /// `GM_ADAPT_0` — FULL or DOWN by `average_me_sad`.
    pub const ADAPT_0: u8 = 3;
    /// `GM_ADAPT_1` — DOWN or DOWN16 by `average_me_sad`.
    pub const ADAPT_1: u8 = 4;
}

/// C's normalized distortion thresholds (`global_me.c:25-27`).
const GMV_ME_SAD_TH_0: u32 = 1;
const GMV_ME_SAD_TH_1: u32 = 5;
const GMV_ME_SAD_TH_2: u32 = 10;

/// The picture-level inputs `svt_aom_global_motion_estimation` reads before it
/// decides anything.
#[derive(Debug, Clone, Copy)]
pub struct GmEstimationInputs<'a> {
    /// `pcs->gm_ctrls`, from
    /// [`crate::port_enc_mode_config::ctrls::set_gm_controls`].
    pub gm_ctrls: GmControls,
    /// `pcs->rc_me_distortion[0..b64_total_count]`.
    pub rc_me_distortion: &'a [u32],
    /// `pcs->rc_me_allow_gm[0..b64_total_count]`.
    pub rc_me_allow_gm: &'a [u8],
    /// `input_pic->width` — C's `pcs->enhanced_pic` (`me_process.c:136`), i.e.
    /// the SOURCE picture, not an SB-aligned one.
    pub input_width: u32,
    /// `input_pic->height`.
    pub input_height: u32,
    /// `pcs->ref_list0_count_try`.
    pub ref_list0_count_try: u32,
    /// `pcs->ref_list1_count_try`.
    pub ref_list1_count_try: u32,
    /// `pcs->temporal_layer_index`.
    pub temporal_layer_index: u8,
    /// `pcs->gm_pp_detected` — the pre-processor's verdict. Only read when
    /// `gm_ctrls.pp_enabled`, which no level this port can express sets (level
    /// 2 does, and level 2 needs `enc_mode <= ENC_MR`).
    pub gm_pp_detected: bool,
}

/// What C's frame-level derivation resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GmEstimation {
    /// `average_me_sad` (`global_me.c:157`) — an INTEGER divide, and the whole
    /// reason the search usually does not run.
    pub average_me_sad: u32,
    /// `total_gm_sbs` (`:155`).
    pub total_gm_sbs: u32,
    /// `global_motion_estimation_level` AFTER the `bypass_based_on_me` clamp
    /// (`:184-188`). 0 means every reference keeps IDENTITY.
    pub estimation_level: u8,
    /// `pcs->gm_downsample_level` (`:175-181`).
    pub downsample_level: u8,
    /// How many `compute_global_motion` calls C would make — the sum over both
    /// lists of the capped `num_of_ref_pic_to_search`.
    ///
    /// This is an UPPER BOUND on the calls that actually happen: `identiy_exit`
    /// can break out of the list loop after list 0 (`:279-287`), which needs
    /// the search's own result and so cannot be known here. Zero means C
    /// provably calls it not once.
    pub max_searches: u32,
}

impl GmEstimation {
    /// True when C's search cannot run, so every reference keeps the IDENTITY
    /// model `svt_aom_global_motion_estimation` initialised it to (`:143-151`)
    /// and `pcs->is_gm_on` stays 0.
    #[must_use]
    pub const fn all_identity(&self) -> bool {
        self.estimation_level == 0 || self.max_searches == 0
    }
}

/// Port of the derivation half of `svt_aom_global_motion_estimation`
/// (`global_me.c:137-190`), plus `me_process.c:266`'s enable gate.
///
/// Returns `None` when C would not enter the function at all — `gm_ctrls.
/// enabled == 0`, or `pp_enabled` without `gm_pp_detected` (`me_process.c:266`)
/// — in which case `is_global_motion` is memset false and every model is
/// IDENTITY, exactly as when the search is skipped.
///
/// # Panics
///
/// Never: the two slices are read over `min(len)` and the divisor is checked.
#[must_use]
pub fn global_motion_estimation(i: &GmEstimationInputs<'_>) -> Option<GmEstimation> {
    // `me_process.c:266`. C's `else` arm memsets `is_global_motion` false, so
    // a `None` here and an all-IDENTITY `Some` mean the same thing to a caller
    // that only asks `all_identity()` — they are distinguished so a reader can
    // tell "GM is off for this preset" from "GM ran and found nothing".
    if i.gm_ctrls.enabled == 0 || (i.gm_ctrls.pp_enabled && !i.gm_pp_detected) {
        return None;
    }
    // `:152-156`. C sums over `b64_total_count`; the two arrays are that long
    // by construction (`pcs.c:1301`), and the shorter of the two bounds the
    // loop here rather than trusting a length that came from a caller.
    let n = i.rc_me_distortion.len().min(i.rc_me_allow_gm.len());
    let mut total_me_sad: u32 = 0;
    let mut total_gm_sbs: u32 = 0;
    for b in 0..n {
        // C accumulates into `uint32_t`, so the wrap is C's own arithmetic.
        total_me_sad = total_me_sad.wrapping_add(i.rc_me_distortion[b]);
        total_gm_sbs = total_gm_sbs.wrapping_add(u32::from(i.rc_me_allow_gm[b]));
    }
    // `:157`. An integer divide by the SOURCE picture's pixel count.
    let pixels = i.input_width.saturating_mul(i.input_height);
    // C divides unconditionally; a zero-pixel picture cannot reach the encoder,
    // and `checked_div` keeps that fact from becoming a panic if it ever does.
    let average_me_sad = total_me_sad.checked_div(pixels).unwrap_or(0);

    // `:161-172`.
    let mut estimation_level = if average_me_sad < GMV_ME_SAD_TH_0 {
        0
    } else if average_me_sad < GMV_ME_SAD_TH_1 {
        1
    } else if average_me_sad < GMV_ME_SAD_TH_2 {
        2
    } else {
        3
    };

    // `:175-181`.
    let downsample_level = if i.gm_ctrls.downsample_level == gm_level::ADAPT_0 {
        if average_me_sad < GMV_ME_SAD_TH_1 {
            gm_level::DOWN
        } else {
            gm_level::FULL
        }
    } else if i.gm_ctrls.downsample_level == gm_level::ADAPT_1 {
        if average_me_sad < GMV_ME_SAD_TH_2 {
            gm_level::DOWN16
        } else {
            gm_level::DOWN
        }
    } else {
        i.gm_ctrls.downsample_level
    };

    // `:183-188`. NB `b64_total_count >> 1`, so a single-b64 picture can never
    // trip this (`total_gm_sbs < 0` is false for an unsigned count).
    if i.gm_ctrls.bypass_based_on_me != 0 {
        let n32 = u32::try_from(n).unwrap_or(u32::MAX);
        if total_gm_sbs < (n32 >> 1) {
            estimation_level = 0;
        }
    }

    // `:209-222` + `:224-226`, the per-list reference-count caps.
    let mut max_searches = 0u32;
    if estimation_level != 0 {
        for list_index in 0..2u32 {
            let mut num = if list_index == 0 {
                i.ref_list0_count_try
            } else {
                i.ref_list1_count_try
            };
            if estimation_level == 1 {
                num = num.min(1);
            } else if estimation_level == 2 {
                num = num.min(2);
            }
            if i.temporal_layer_index > 0 && i.gm_ctrls.ref_idx0_only {
                num = num.min(1);
            }
            max_searches += num;
        }
    }

    Some(GmEstimation {
        average_me_sad,
        total_gm_sbs,
        estimation_level,
        downsample_level,
        max_searches,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::port_enc_mode_config::{ResolutionRange, ctrls::set_gm_controls};

    fn lvl4() -> GmControls {
        set_gm_controls(4, ResolutionRange::R240p).unwrap()
    }

    /// The measured campaign cell: one b64, a residual small enough that the
    /// integer divide floors to zero, so C's search does not run.
    #[test]
    fn zero_average_me_sad_skips_the_search() {
        let d = [1234u32];
        let g = [1u8];
        let out = global_motion_estimation(&GmEstimationInputs {
            gm_ctrls: lvl4(),
            rc_me_distortion: &d,
            rc_me_allow_gm: &g,
            input_width: 64,
            input_height: 64,
            ref_list0_count_try: 1,
            ref_list1_count_try: 1,
            temporal_layer_index: 0,
            gm_pp_detected: false,
        })
        .unwrap();
        assert_eq!(out.average_me_sad, 0);
        assert_eq!(out.estimation_level, 0);
        assert_eq!(out.max_searches, 0);
        assert!(out.all_identity());
    }

    /// The measured zoom cell's shape: `avg_me_sad = 2` puts C at level 1 and
    /// it searches one reference per list.
    #[test]
    fn nonzero_average_me_sad_runs_the_search() {
        // 64 b64s of a 512x512 picture; 2 * 512 * 512 / 64 per b64 gives an
        // average of exactly 2.
        let d = [2 * 512 * 512 / 64u32; 64];
        let g = [1u8; 64];
        let out = global_motion_estimation(&GmEstimationInputs {
            gm_ctrls: lvl4(),
            rc_me_distortion: &d,
            rc_me_allow_gm: &g,
            input_width: 512,
            input_height: 512,
            ref_list0_count_try: 1,
            ref_list1_count_try: 1,
            temporal_layer_index: 0,
            gm_pp_detected: false,
        })
        .unwrap();
        assert_eq!(out.average_me_sad, 2);
        assert_eq!(out.estimation_level, 1);
        assert_eq!(out.max_searches, 2);
        assert!(!out.all_identity());
    }

    /// `bypass_based_on_me` forces level 0 when fewer than half the b64s voted
    /// for global motion — and it is ON at every level this port can express.
    #[test]
    fn bypass_based_on_me_clamps_to_zero() {
        let d = [100_000u32; 64];
        let mut g = [0u8; 64];
        g[..10].fill(1);
        let out = global_motion_estimation(&GmEstimationInputs {
            gm_ctrls: lvl4(),
            rc_me_distortion: &d,
            rc_me_allow_gm: &g,
            input_width: 512,
            input_height: 512,
            ref_list0_count_try: 1,
            ref_list1_count_try: 1,
            temporal_layer_index: 0,
            gm_pp_detected: false,
        })
        .unwrap();
        assert_eq!(out.total_gm_sbs, 10);
        assert_eq!(out.estimation_level, 0);
        assert!(out.all_identity());
    }

    /// Level 0 controls mean C never enters the function.
    #[test]
    fn disabled_controls_return_none() {
        let ctrls = set_gm_controls(0, ResolutionRange::R240p).unwrap();
        assert!(
            global_motion_estimation(&GmEstimationInputs {
                gm_ctrls: ctrls,
                rc_me_distortion: &[1_000_000; 64],
                rc_me_allow_gm: &[1; 64],
                input_width: 512,
                input_height: 512,
                ref_list0_count_try: 1,
                ref_list1_count_try: 1,
                temporal_layer_index: 0,
                gm_pp_detected: false,
            })
            .is_none()
        );
    }
}
