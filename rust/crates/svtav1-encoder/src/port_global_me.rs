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

use svtav1_types::motion::{TransformationType, WarpedMotionParams};

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

// ---------------------------------------------------------------------------
// `Codec/global_me_cost.c`
// ---------------------------------------------------------------------------

/// C `AV1_PROB_COST_SHIFT` (`md_rate_estimation.h:29`).
const AV1_PROB_COST_SHIFT: u32 = 9;

/// Port of `svt_aom_gm_get_params_cost` (`global_me_cost.c:24`). EXPORTED.
///
/// The bit cost of coding `gm` against `ref_gm`, in C's `1 << 9` fixed-point
/// rate units — the quantity `compute_global_motion` hands
/// `svt_av1_refine_integerized_param` as `params_cost` and
/// `svt_av1_is_enough_erroradvantage` as the second half of its product test.
/// It is a pure COUNT of the syntax
/// [`crate::port_entropy_inter::gm::write_global_motion_params`] writes, and it
/// reuses that file's `count_signed_primitive_refsubexpfin` so the two cannot
/// drift.
///
/// C's `switch` falls through AFFINE -> ROTZOOM -> TRANSLATION -> IDENTITY, so
/// a ROTZOOM pays wmmat[2..4] plus the two translation terms and an AFFINE
/// pays wmmat[2..6] plus them.
#[must_use]
pub fn gm_get_params_cost(
    gm: &WarpedMotionParams,
    ref_gm: &WarpedMotionParams,
    allow_hp: bool,
) -> i32 {
    use crate::port_entropy_inter::gm::{
        GM_ABS_TRANS_BITS, GM_ABS_TRANS_ONLY_BITS, GM_ALPHA_MAX, GM_ALPHA_PREC_BITS,
        GM_ALPHA_PREC_DIFF, GM_TRANS_ONLY_PREC_DIFF, GM_TRANS_PREC_DIFF, SUBEXPFIN_K,
        count_signed_primitive_refsubexpfin as count,
    };
    let mut params_cost = 0i32;
    let ty = gm.wm_type;
    if ty == TransformationType::RotZoom || ty == TransformationType::Affine {
        params_cost += count(
            GM_ALPHA_MAX + 1,
            SUBEXPFIN_K,
            ((ref_gm.wmmat[2] >> GM_ALPHA_PREC_DIFF) - (1 << GM_ALPHA_PREC_BITS)) as i16,
            ((gm.wmmat[2] >> GM_ALPHA_PREC_DIFF) - (1 << GM_ALPHA_PREC_BITS)) as i16,
        );
        params_cost += count(
            GM_ALPHA_MAX + 1,
            SUBEXPFIN_K,
            (ref_gm.wmmat[3] >> GM_ALPHA_PREC_DIFF) as i16,
            (gm.wmmat[3] >> GM_ALPHA_PREC_DIFF) as i16,
        );
        if ty == TransformationType::Affine {
            params_cost += count(
                GM_ALPHA_MAX + 1,
                SUBEXPFIN_K,
                (ref_gm.wmmat[4] >> GM_ALPHA_PREC_DIFF) as i16,
                (gm.wmmat[4] >> GM_ALPHA_PREC_DIFF) as i16,
            );
            params_cost += count(
                GM_ALPHA_MAX + 1,
                SUBEXPFIN_K,
                ((ref_gm.wmmat[5] >> GM_ALPHA_PREC_DIFF) - (1 << GM_ALPHA_PREC_BITS)) as i16,
                ((gm.wmmat[5] >> GM_ALPHA_PREC_DIFF) - (1 << GM_ALPHA_PREC_BITS)) as i16,
            );
        }
    }
    if matches!(
        ty,
        TransformationType::Translation | TransformationType::RotZoom | TransformationType::Affine
    ) {
        let is_trans = ty == TransformationType::Translation;
        let trans_bits = if is_trans {
            GM_ABS_TRANS_ONLY_BITS - i32::from(!allow_hp)
        } else {
            GM_ABS_TRANS_BITS
        };
        let trans_prec_diff = if is_trans {
            GM_TRANS_ONLY_PREC_DIFF + u32::from(!allow_hp)
        } else {
            GM_TRANS_PREC_DIFF
        };
        let n = u16::try_from((1i32 << trans_bits) + 1).unwrap_or(u16::MAX);
        params_cost += count(
            n,
            SUBEXPFIN_K,
            (ref_gm.wmmat[0] >> trans_prec_diff) as i16,
            (gm.wmmat[0] >> trans_prec_diff) as i16,
        );
        params_cost += count(
            n,
            SUBEXPFIN_K,
            (ref_gm.wmmat[1] >> trans_prec_diff) as i16,
            (gm.wmmat[1] >> trans_prec_diff) as i16,
        );
    }
    params_cost << AV1_PROB_COST_SHIFT
}

// ---------------------------------------------------------------------------
// `compute_global_motion` — the per-reference search driver
// ---------------------------------------------------------------------------

/// C `RANSAC_NUM_MOTIONS` (`global_motion.h:24`) — 1 in this tree, so the
/// "best of N motions" loop C writes has exactly one iteration.
pub const RANSAC_NUM_MOTIONS: usize = 1;

/// One luma plane as `compute_global_motion` reads it: `y_buffer` (already at
/// the picture ORIGIN, not at the padded allocation's start), the stride, and
/// the `width`/`height` C passes alongside.
#[derive(Debug, Clone, Copy)]
pub struct GmPlane<'a> {
    /// C `pic->y_buffer` — index 0 is pixel (0, 0).
    pub buf: &'a [u8],
    /// C `pic->y_stride`.
    pub stride: usize,
    /// C `pic->width`.
    pub width: u32,
    /// C `pic->height`.
    pub height: u32,
}

/// Port of the static `compute_global_motion` (`global_me.c:320`) for the
/// MV-correspondence arm, which is the only one this port can reach.
///
/// `src` is C's `input_pic` (`pcs->enhanced_pic`, `me_process.c:136`) and `rf`
/// is `ref_object->input_padded_pic`. MEASURED with the `SVT_GMSEARCH_OUT`
/// interposer on `crop:` CID22 photo 256x256 with a 33/32 zoom: C passes
/// `r=256x256/400 d=256x256/400` — the same dims and the same stride for both,
/// i.e. the TRUE picture dims and each buffer's own padded stride. The warp and
/// the SAD both address from the origin and stay inside `width x height`, so a
/// tightly-packed plane and a bordered one give the same answer as long as the
/// pixels agree; the stride is carried anyway rather than assumed.
///
/// `sf` (C's downsample scale factor) is 1 at every level this port can express
/// — `gm_downsample_level` is `GM_FULL` for `gm_level` 3/4 — so
/// `svt_aom_upscale_wm_params` is a no-op and is not ported. A caller that ever
/// reaches an adaptive downsample level must port it first; this asserts
/// instead of silently skipping it.
///
/// Returns the model C would leave in `pcs->global_motion_estimation[list][ref]`.
///
/// # Panics
///
/// If `sf != 1` (see above), or if the controls name the CORNERS correspondence
/// method, which is unported (`crate::port_gm_correspondence`).
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
#[must_use]
pub fn compute_global_motion(
    ctrls: &GmControls,
    me: &crate::port_gm_correspondence::MeResultsView<'_>,
    geom: &crate::port_gm_correspondence::GmPictureGeometry,
    src: GmPlane<'_>,
    rf: GmPlane<'_>,
    sf: u8,
    chess_refn: bool,
    allow_high_precision_mv: bool,
    list_idx: u8,
    ref_idx: u8,
    mut trace: Option<&mut dyn FnMut(core::fmt::Arguments<'_>)>,
) -> WarpedMotionParams {
    use crate::port_global_motion::{
        GM_ERRORADV_TR_0, GmRefineCtrls, convert_model_to_params, is_enough_erroradvantage,
        refine_integerized_param,
    };
    use crate::port_gm_correspondence::{CorrespondenceMethod, gm_compute_correspondence};
    use crate::port_ransac::{RansacModel, determine_gm_params};

    assert_eq!(
        sf, 1,
        "svt_aom_upscale_wm_params is unported (sf must be 1)"
    );

    let default_wm = WarpedMotionParams::default();
    let mut global_motion = default_wm;

    // `:334` — the whole-frame SAD between the reference and the source. A zero
    // means the two pictures are identical and C returns IDENTITY immediately;
    // it is also the denominator of every `erroradvantage` ratio below, so a
    // zero would be a divide by zero.
    if let Some(t) = trace.as_deref_mut() {
        t(format_args!(
            "GMSRC list={list_idx} ref={ref_idx} r={}x{}/{}[{}] d={}x{}/{}[{}]",
            rf.width,
            rf.height,
            rf.stride,
            rf.buf.len(),
            src.width,
            src.height,
            src.stride,
            src.buf.len(),
        ));
    }
    let ref_sad_error = svtav1_dsp::sad::sad(
        rf.buf,
        rf.stride,
        src.buf,
        src.stride,
        src.width as usize,
        src.height as usize,
    );
    if let Some(t) = trace.as_deref_mut() {
        t(format_args!(
            "GMSAD list={list_idx} ref={ref_idx} pic_sad={ref_sad_error}"
        ));
    }
    if ref_sad_error == 0 {
        return global_motion;
    }

    let method = match ctrls.correspondence_method {
        1 => CorrespondenceMethod::Mv32x32,
        2 => CorrespondenceMethod::Mv16x16,
        3 => CorrespondenceMethod::Mv8x8,
        // 0 is the zeroed-context value the level-0 arm leaves behind, and 4 is
        // CORNERS. Neither can reach a search: level 0 means `enabled == 0`.
        other => panic!("unreachable correspondence_method {other} in a GM search"),
    };
    let correspondences = gm_compute_correspondence(me, geom, method, list_idx, ref_idx)
        .expect("the CORNERS correspondence arm is unported and unreachable at gm_level 3/4");

    let refine_ctrls = GmRefineCtrls {
        rfn_early_exit: ctrls.rfn_early_exit != 0,
    };
    // `:377` — C asserts `search_start_model > IDENTITY`, `search_end_model <=
    // AFFINE` and start <= end. Level 3/4 give TRANSLATION..ROTZOOM.
    for model_u8 in ctrls.search_start_model..=ctrls.search_end_model {
        let model = match model_u8 {
            1 => RansacModel::Translation,
            2 => RansacModel::RotZoom,
            3 => RansacModel::Affine,
            other => panic!("search model {other} is outside C's TRANSLATION..AFFINE"),
        };
        let wm_model = match model_u8 {
            1 => TransformationType::Translation,
            2 => TransformationType::RotZoom,
            _ => TransformationType::Affine,
        };
        let mut best_warp_error = i64::MAX;

        let motions = determine_gm_params(model, &correspondences, RANSAC_NUM_MOTIONS);
        for m in &motions {
            if m.num_inliers == 0 {
                continue;
            }
            let mut tmp = convert_model_to_params(&[
                m.params[0],
                m.params[1],
                m.params[2],
                m.params[3],
                m.params[4],
                m.params[5],
            ]);
            // `svt_aom_upscale_wm_params(&tmp, sf)` — a no-op at sf == 1.
            if tmp.wm_type == TransformationType::Identity {
                continue;
            }
            let params_cost = gm_get_params_cost(&tmp, &default_wm, allow_high_precision_mv);
            let cand_type = tmp.wm_type;
            let warp_error = refine_integerized_param(
                &refine_ctrls,
                &mut tmp,
                cand_type,
                rf.buf,
                rf.width as i32,
                rf.height as i32,
                rf.stride,
                src.buf,
                src.width as i32,
                src.height as i32,
                src.stride,
                i32::from(ctrls.params_refinement_steps),
                chess_refn,
                best_warp_error,
                ref_sad_error,
                params_cost,
            );
            if let Some(t) = trace.as_deref_mut() {
                t(format_args!(
                    "GMREFINE list={list_idx} ref={ref_idx} wmtype={} cost={params_cost} \
                     pic_sad={ref_sad_error} -> err={warp_error} out={:?},{:?}",
                    wm_model as u8, tmp.wm_type as u8, tmp.wmmat
                ));
            }
            if warp_error < best_warp_error {
                best_warp_error = warp_error;
                global_motion = tmp;
            }
        }

        // `:421` — the shear check AND the "did refinement demote the type?"
        // check. `svt_get_shear_params` writes alpha/beta/gamma/delta back into
        // the model, which is why it takes `&mut`.
        if !svtav1_dsp::port_warp::get_shear_params(&mut global_motion)
            || global_motion.wm_type != wm_model
        {
            global_motion = default_wm;
        }
        if global_motion.wm_type == TransformationType::Identity {
            continue;
        }
        // `:432` — the error advantage has to beat BOTH thresholds, measured
        // against the FINAL model's own params cost, not the candidate's.
        let cost = gm_get_params_cost(&global_motion, &default_wm, allow_high_precision_mv);
        if !is_enough_erroradvantage(
            best_warp_error as f64 / f64::from(ref_sad_error),
            cost,
            GM_ERRORADV_TR_0,
        ) {
            global_motion = default_wm;
        }
        if global_motion.wm_type != TransformationType::Identity {
            break;
        }
    }
    global_motion
}

/// C `MAX_NUM_OF_REF_PIC_LIST` (`definitions.h:2048`).
pub const MAX_NUM_OF_REF_PIC_LIST: usize = 2;
/// C `REF_LIST_MAX_DEPTH` (`EbSvtAv1Enc.h:35`).
pub const REF_LIST_MAX_DEPTH: usize = 4;

/// `pcs->global_motion_estimation[][]` + `pcs->is_global_motion[][]` +
/// `pcs->is_gm_on`, as `svt_aom_global_motion_estimation` leaves them.
#[derive(Debug, Clone, Copy)]
pub struct GmModels {
    /// `pcs->global_motion_estimation[list][ref]`.
    pub models: [[WarpedMotionParams; REF_LIST_MAX_DEPTH]; MAX_NUM_OF_REF_PIC_LIST],
    /// `pcs->is_global_motion[list][ref]`.
    pub is_global_motion: [[bool; REF_LIST_MAX_DEPTH]; MAX_NUM_OF_REF_PIC_LIST],
    /// `pcs->is_gm_on`.
    pub is_gm_on: bool,
}

impl Default for GmModels {
    fn default() -> Self {
        Self {
            models: [[WarpedMotionParams::default(); REF_LIST_MAX_DEPTH]; MAX_NUM_OF_REF_PIC_LIST],
            is_global_motion: [[false; REF_LIST_MAX_DEPTH]; MAX_NUM_OF_REF_PIC_LIST],
            is_gm_on: false,
        }
    }
}

/// The reference-plane lookup `svt_aom_global_motion_estimation` performs
/// through `pcs->ref_pa_pic_ptr_array[list][ref]` (`global_me.c:227`).
///
/// `None` means the port has no PA reference for that slot, which is a
/// harness gap rather than a picture-decision outcome — the caller must refuse
/// rather than silently treat it as IDENTITY.
pub type RefPlaneLookup<'a> = dyn Fn(usize, usize) -> Option<GmPlane<'a>> + 'a;

/// Port of the SEARCH half of `svt_aom_global_motion_estimation`
/// (`global_me.c:190-300`) — the per-list reference loop, the `identiy_exit`
/// break and the final `is_gm_on` reduction.
///
/// `est` is [`global_motion_estimation`]'s output for this frame; when it says
/// `all_identity()` the loops do not run at all and every model stays IDENTITY,
/// which is C's own initialisation (`:143-151`).
///
/// `ref_list0_count` / `ref_list1_count` are the UNCAPPED counts C's final
/// `is_gm_on` reduction walks (`:291`), which are NOT the `_try` counts the
/// search loop caps (`:210`). C reads both and they differ whenever
/// `update_count_try` capped a list.
///
/// # Errors
///
/// The reference lookup returning `None` for a slot the search needs.
#[allow(clippy::too_many_arguments)]
pub fn global_motion_search(
    est: &GmEstimation,
    ctrls: &GmControls,
    me: &crate::port_gm_correspondence::MeResultsView<'_>,
    geom: &crate::port_gm_correspondence::GmPictureGeometry,
    src: GmPlane<'_>,
    ref_plane: &RefPlaneLookup<'_>,
    allow_high_precision_mv: bool,
    temporal_layer_index: u8,
    ref_counts_try: [u32; MAX_NUM_OF_REF_PIC_LIST],
    ref_counts: [u32; MAX_NUM_OF_REF_PIC_LIST],
    mut trace: Option<&mut dyn FnMut(core::fmt::Arguments<'_>)>,
) -> Result<GmModels, GmSearchError> {
    let mut out = GmModels::default();
    if est.estimation_level != 0 {
        // `:243-257` — at GM_FULL the detection and refinement planes are the
        // same buffers and `chess_refn` is the control's own value. The two
        // downsampled arms pick the quarter/sixteenth pyramids and force
        // `chess_refn = 0` (GM_DOWN16) — unreachable here because
        // `set_gm_controls` only ever assigns `GM_FULL`, and asserted so.
        if est.downsample_level != gm_level::FULL {
            return Err(GmSearchError::DownsampledUnported(est.downsample_level));
        }
        let chess_refn = ctrls.chess_rfn != 0;
        for list_index in 0..MAX_NUM_OF_REF_PIC_LIST {
            let mut num = ref_counts_try[list_index];
            if est.estimation_level == 1 {
                num = num.min(1);
            } else if est.estimation_level == 2 {
                num = num.min(2);
            }
            if temporal_layer_index > 0 && ctrls.ref_idx0_only {
                num = num.min(1);
            }
            for ref_pic_index in 0..(num as usize).min(REF_LIST_MAX_DEPTH) {
                let rf = ref_plane(list_index, ref_pic_index)
                    .ok_or(GmSearchError::MissingReference(list_index, ref_pic_index))?;
                out.models[list_index][ref_pic_index] = compute_global_motion(
                    ctrls,
                    me,
                    geom,
                    src,
                    rf,
                    /*sf=*/ 1,
                    chess_refn,
                    allow_high_precision_mv,
                    u8::try_from(list_index).unwrap_or(0),
                    u8::try_from(ref_pic_index).unwrap_or(0),
                    trace
                        .as_deref_mut()
                        .map(|t| t as &mut dyn FnMut(core::fmt::Arguments<'_>)),
                );
            }
            // `:279-287` — list 0's FIRST reference decides whether list 1 is
            // searched at all.
            if ctrls.identiy_exit != 0
                && list_index == 0
                && out.models[0][0].wm_type == TransformationType::Identity
            {
                break;
            }
        }
    }
    // `:290-303`. Note the UNCAPPED counts.
    for list_index in 0..MAX_NUM_OF_REF_PIC_LIST {
        let n = (ref_counts[list_index] as usize).min(REF_LIST_MAX_DEPTH);
        for ref_pic_index in 0..n {
            if out.models[list_index][ref_pic_index].wm_type != TransformationType::Identity {
                out.is_global_motion[list_index][ref_pic_index] = true;
                out.is_gm_on = true;
            }
        }
    }
    Ok(out)
}

/// Why [`global_motion_search`] could not run C's search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GmSearchError {
    /// `pcs->gm_downsample_level` resolved to something other than `GM_FULL`;
    /// `svt_aom_upscale_wm_params` and the quarter/sixteenth pyramids are not
    /// ported. Unreachable while `set_gm_controls` only assigns `GM_FULL`.
    DownsampledUnported(u8),
    /// No PA reference plane for `(list, ref)`.
    MissingReference(usize, usize),
}
