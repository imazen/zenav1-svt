use super::*;

/// C `get_similar_ref_brightness` (`pd_process.c:4251-4267`) — EXPORTED.
///
/// Produces `pcs->similar_brightness_refs`, read by `motion_estimation.c:2231`
/// to take the safe-limit ME path. `avg_luma` of `INVALID_LUMA` on EITHER
/// reference disables the test entirely.
///
/// The luma means are `uint64_t` in C and the comparison casts BOTH sides to
/// `int` before subtracting, which is reproduced here.
#[must_use]
pub fn get_similar_ref_brightness(
    slice_type: SliceType,
    hierarchical_levels: u8,
    ref_list1_count_try: u8,
    ref0_avg_luma: u64,
    ref1_avg_luma: u64,
    cur_avg_luma: u64,
) -> bool {
    if slice_type == SliceType::B
        && hierarchical_levels > 0
        && ref_list1_count_try > 0
        && ref0_avg_luma != INVALID_LUMA
        && ref1_avg_luma != INVALID_LUMA
    {
        const LUMA_TH: i32 = 5;
        let cur = cur_avg_luma as i32;
        return (ref0_avg_luma as i32 - cur).abs() < LUMA_TH
            && (ref1_avg_luma as i32 - cur).abs() < LUMA_TH;
    }
    false
}

/// C `send_picture_out`'s reference-count adjustment block
/// (`pd_process.c:4276-4313`) — static. Only this block is in scope; the fifo
/// posting around it is buffer plumbing.
///
/// Two independent limiters, in C's order:
/// 1. the RTC early-HME prune, which drops LAST2 (flat) or LAST3
///    (hierarchical base) when it is at least `early_hme_l0_prune_th` percent
///    worse than LAST — and then RE-RUNS `set_all_ref_frame_type`, because the
///    candidate set is derived from the try counts;
/// 2. `safe_limit_nref == 2`, which caps both lists at 1 when the references
///    have similar brightness.
///
/// Trap, and C flags it itself with a TODO: limiter 2 lowers the try counts
/// AFTER `set_all_ref_frame_type` has already run, so `ref_frame_type_arr`
/// keeps candidates for references MD will not enumerate. That is reproduced,
/// not fixed — byte-identity means reproducing it (`WORKING-ON-THIS.md` §7).
///
/// `hme_dist` returns `(last_dist, other_dist)` for the pair the current
/// hierarchy compares; it is `None` when the prune does not apply.
pub fn send_picture_out_ref_counts(
    pic: &mut PicParams,
    seq: &SeqPicParams,
    hme_dist: Option<(u64, u64)>,
    similar_brightness_refs: bool,
) {
    let mrp = &seq.mrp_ctrls;
    if seq.rtc && mrp.early_hme_l0_prune_th != 0 && pic.ref_list0_count_try > 1 {
        // `cap` is the count the prune drops to: 1 for the flat structure
        // (LAST2 gone) and 2 for the hierarchical base (LAST3 gone).
        let cap = if pic.hierarchical_levels == 0 {
            Some(1u8)
        } else if pic.temporal_layer_index == 0 && pic.ref_list0_count_try >= 3 {
            Some(2u8)
        } else {
            None
        };
        if let (Some(cap), Some((last_dist, other_dist))) = (cap, hme_dist)
            && other_dist * 100 >= last_dist * u64::from(mrp.early_hme_l0_prune_th)
        {
            pic.ref_list0_count_try = pic.ref_list0_count_try.min(cap);
            let (arr, tot) = set_all_ref_frame_type(pic);
            pic.ref_frame_type_arr = arr;
            pic.tot_ref_frame_types = tot;
        }
    }

    if mrp.safe_limit_nref == 2
        && pic.slice_type == SliceType::B
        && pic.hierarchical_levels > 0
        && pic.temporal_layer_index >= pic.hierarchical_levels - 1
        && similar_brightness_refs
    {
        // C's own TODO: these run AFTER set_all_ref_frame_type.
        pic.ref_list0_count_try = pic.ref_list0_count_try.min(1);
        pic.ref_list1_count_try = pic.ref_list1_count_try.min(1);
    }
}

/// Which entry of `scs->tf_params_per_type[]` a picture takes, or `Disabled`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TfParamsChoice {
    /// C `pcs->tf_ctrls.enabled = 0`.
    Disabled,
    /// C `scs->tf_params_per_type[0]` — the delayed-intra entry.
    DelayedIntra,
    /// C `scs->tf_params_per_type[1]` — BASE.
    Base,
    /// C `scs->tf_params_per_type[2]` — L1.
    L1,
}

/// C `copy_tf_params` (`pd_process.c:4468-4497`) — static.
///
/// The gate that decides whether temporal filtering runs at all, and which
/// parameter set it uses.
///
/// **MEASURED, and the negative result must be reproduced rather than
/// assumed:** in `LOW_DELAY` `tf_level` is forced to 0 before any preset logic
/// (`enc_handle.c:3339-3343`), so `tf_params_per_type[1]` is itself disabled
/// and the correct port yields TF OFF for every low-delay picture — including
/// the base-layer inter pictures this function nominally maps to entry 1.
/// This function returns [`TfParamsChoice::Base`] for those pictures because
/// that is what C selects; the disabling happens one level up, in the
/// parameter table.
///
/// The `IS_SFRAME_FLEXIBLE_INSERT` guard that can force `enabled = 0` when
/// `ctx->tf_pic_arr_cnt == 0` is not ported (S-frames are outside the
/// envelope) and is named here rather than dropped.
#[must_use]
pub fn copy_tf_params(
    seq_pred_structure: PredStructure,
    slice_type: SliceType,
    is_key_frame: bool,
    temporal_layer_index: u8,
    hierarchical_levels: u8,
    is_overlay: bool,
    enable_tf_key: bool,
    is_delayed_intra: bool,
) -> TfParamsChoice {
    if seq_pred_structure == PredStructure::LowDelay {
        return if slice_type != SliceType::I && temporal_layer_index == 0 {
            TfParamsChoice::Base
        } else {
            TfParamsChoice::Disabled
        };
    }
    // No TF for overlays, for a key frame with enable_tf_key off, or for the
    // highest layer (which matters at 2L).
    if (is_key_frame && !enable_tf_key) || is_overlay || temporal_layer_index == hierarchical_levels
    {
        TfParamsChoice::Disabled
    } else if is_delayed_intra {
        TfParamsChoice::DelayedIntra
    } else if temporal_layer_index == 0 {
        TfParamsChoice::Base
    } else if temporal_layer_index == 1 {
        TfParamsChoice::L1
    } else {
        TfParamsChoice::Disabled
    }
}

// ---------------------------------------------------------------------------
// Histogram-based scene detection — pd_process.c:55-84, 256-378, 4682-4719,
// 5192-5215
// ---------------------------------------------------------------------------
//
// Reachability, measured rather than inferred. `static_config.scene_change_detection`
// is force-zeroed with a warning (`enc_settings.c:839-843`), which makes it
// tempting to call the whole detector dead. It is NOT:
// `vq_ctrls.sharpness_ctrls.scene_transition` is set to 1 in BOTH arms of
// `derive_vq_params` (`enc_handle.c:3282, 3291`) and only zeroed for
// LOW_DELAY (`3324-3326`), so the transition path runs in random access and
// its output becomes `pcs->transition_present`. `scs->calc_hist` follows the
// same shape (`enc_handle.c:1353` makes it 1 whenever any TF type is
// enabled), so `calc_ahd_pd` — and hence `pcs->ahd_error`, read at
// `motion_estimation.c:1245` — is live there too.

/// C `HISTOGRAM_NUMBER_OF_BINS` (`pcs.h:39`).
pub const HISTOGRAM_NUMBER_OF_BINS: usize = 256;
/// C `MAX_NUMBER_OF_REGIONS_IN_WIDTH` (`pcs.h:40`).
pub const MAX_NUMBER_OF_REGIONS_IN_WIDTH: usize = 4;
/// C `MAX_NUMBER_OF_REGIONS_IN_HEIGHT` (`pcs.h:41`).
pub const MAX_NUMBER_OF_REGIONS_IN_HEIGHT: usize = 4;
/// C `FLASH_TH` (`pd_process.c:170`).
pub const FLASH_TH: u8 = 5;
/// C `FADE_TH` (`pd_process.c:171`).
pub const FADE_TH: u8 = 3;
/// C `SCENE_TH` (`pd_process.c:172`).
pub const SCENE_TH: u32 = 3000;

/// C `NUM64x64INPIC(w, h)` (`pd_process.c:173`).
///
/// The macro is `((w * h) >> (svt_log2f(BLOCK_SIZE_64) << 1))`, i.e.
/// `(w * h) >> 12` — `log2(64) == 6`, doubled is 12. Written out rather than
/// re-deriving the shift at each call site.
#[inline]
#[must_use]
pub fn num_64x64_in_pic(w: u32, h: u32) -> u32 {
    (w.wrapping_mul(h)) >> 12
}

/// A per-region luma histogram plane, as `pcs->picture_histogram` holds it.
pub type RegionHistograms = [[[u32; HISTOGRAM_NUMBER_OF_BINS]; MAX_NUMBER_OF_REGIONS_IN_HEIGHT];
    MAX_NUMBER_OF_REGIONS_IN_WIDTH];
/// `pcs->average_intensity_per_region` (`pcs.h:852`).
///
/// It is `uint64_t[4][4]`, NOT `uint8_t[4][4]` — the values are 8-bit luma
/// means but the storage is 64-bit, and `scene_transition_detector` narrows
/// each one with an explicit `(int16_t)` cast before subtracting. Modelling it
/// as `u8` would silently change what an out-of-range value does.
pub type RegionIntensities =
    [[u64; MAX_NUMBER_OF_REGIONS_IN_HEIGHT]; MAX_NUMBER_OF_REGIONS_IN_WIDTH];

/// C `calc_ahd` (`pd_process.c:55-84`) — static.
///
/// Accumulated histogram difference between two pictures, plus a count of the
/// regions whose own difference exceeds their pixel count. Fills
/// `tf_ahd_error_to_central`, which `temporal_filtering.c:2879` uses to drop
/// dissimilar pictures from the filter window.
///
/// Returns `(ahd, active_region_cnt_increment)`. C takes `active_region_cnt`
/// as an in/out pointer and only ever INCREMENTS it, so the caller adds.
///
/// Note the region size: `ref_pcs->enhanced_pic->{width,height}` divided by
/// the region counts, with NO remainder handling — unlike
/// [`scene_transition_detector`], which does add the remainder (and does it in
/// a way that accumulates; see there).
#[must_use]
pub fn calc_ahd(
    input_hist: &RegionHistograms,
    ref_hist: &RegionHistograms,
    ref_width: u32,
    ref_height: u32,
    regions_per_width: usize,
    regions_per_height: usize,
) -> (u32, u8) {
    let region_width = ref_width / regions_per_width as u32;
    let region_height = ref_height / regions_per_height as u32;
    let mut ahd: u32 = 0;
    let mut active_region_cnt: u8 = 0;
    for w in 0..regions_per_width {
        for h in 0..regions_per_height {
            let mut ahd_per_region: u32 = 0;
            for bin in 0..HISTOGRAM_NUMBER_OF_BINS {
                ahd_per_region = ahd_per_region.wrapping_add(
                    (input_hist[w][h][bin] as i32 - ref_hist[w][h][bin] as i32).unsigned_abs(),
                );
            }
            ahd = ahd.wrapping_add(ahd_per_region);
            if ahd_per_region > region_width.wrapping_mul(region_height) {
                active_region_cnt = active_region_cnt.wrapping_add(1);
            }
        }
    }
    (ahd, active_region_cnt)
}

/// C `calc_ahd_pd` (`pd_process.c:5192-5215`) — static.
///
/// Fills `pcs->ahd_error`, read by `motion_estimation.c:1245` as an ME gating
/// threshold. Live whenever `scs->calc_hist` is set, which is whenever any TF
/// type is enabled — i.e. in the default random-access config.
///
/// Unlike [`calc_ahd`] this one sums a single running total with no per-region
/// bookkeeping and no region-size test.
#[must_use]
pub fn calc_ahd_pd(
    cur_hist: &RegionHistograms,
    prev_hist: &RegionHistograms,
    regions_per_width: usize,
    regions_per_height: usize,
) -> u32 {
    let mut ahd: u32 = 0;
    for w in 0..regions_per_width {
        for h in 0..regions_per_height {
            for bin in 0..HISTOGRAM_NUMBER_OF_BINS {
                ahd = ahd.wrapping_add(
                    (cur_hist[w][h][bin] as i32 - prev_hist[w][h][bin] as i32).unsigned_abs(),
                );
            }
        }
    }
    ahd
}

/// The cross-picture detector state `scene_transition_detector` carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneDetectState {
    /// C `ctx->ahd_running_avg[w][h]`.
    pub ahd_running_avg: [[u32; MAX_NUMBER_OF_REGIONS_IN_HEIGHT]; MAX_NUMBER_OF_REGIONS_IN_WIDTH],
    /// C `ctx->reset_running_avg`.
    pub reset_running_avg: bool,
    /// C `ctx->prev_picture_histogram`.
    pub prev_picture_histogram: alloc::boxed::Box<RegionHistograms>,
    /// C `ctx->prev_average_intensity_per_region`.
    pub prev_average_intensity_per_region: RegionIntensities,
}

impl Default for SceneDetectState {
    fn default() -> Self {
        Self {
            ahd_running_avg: [[0; MAX_NUMBER_OF_REGIONS_IN_HEIGHT]; MAX_NUMBER_OF_REGIONS_IN_WIDTH],
            // C's ctor zeroes the context, so reset_running_avg starts FALSE
            // and the first picture therefore folds its ahd into a zero
            // running average rather than seeding it. That asymmetry is C's.
            reset_running_avg: false,
            prev_picture_histogram: alloc::boxed::Box::new(
                [[[0u32; HISTOGRAM_NUMBER_OF_BINS]; MAX_NUMBER_OF_REGIONS_IN_HEIGHT];
                    MAX_NUMBER_OF_REGIONS_IN_WIDTH],
            ),
            prev_average_intensity_per_region: [[0; MAX_NUMBER_OF_REGIONS_IN_HEIGHT];
                MAX_NUMBER_OF_REGIONS_IN_WIDTH],
        }
    }
}

/// C `scene_transition_detector` (`pd_process.c:256-378`) — static.
///
/// Its output becomes `pcs->transition_present` via `init_pic_settings`, which
/// sharpness-tuned MD reads. Random access only.
///
/// **THE TRAP, and it is a real C quirk that must be reproduced, not tidied.**
/// `region_width` and `region_height` are declared OUTSIDE the region loops
/// and updated inside with `region_width += region_width_offset;`. The offsets
/// are non-zero only on the last row/column, so:
/// * `region_height` grows by the height remainder on EVERY last-height
///   iteration, i.e. once per width column, and the growth persists into the
///   next column;
/// * `region_width` grows by the width remainder on every iteration of the
///   FINAL width column, i.e. `regions_per_height` times.
///
/// `region_threshold` is computed from those accumulating values, so the
/// threshold is different in later regions than in earlier ones even for
/// identically sized regions. Transcribed literally.
///
/// Returns whether a scene change was detected; updates
/// `state.ahd_running_avg` and `state.reset_running_avg` in place.
#[must_use]
pub fn scene_transition_detector(
    state: &mut SceneDetectState,
    current_hist: &RegionHistograms,
    current_intensity: &RegionIntensities,
    future_intensity: &RegionIntensities,
    picture_width: u32,
    picture_height: u32,
    regions_per_width: usize,
    regions_per_height: usize,
) -> bool {
    let mut is_abrupt_change_count: u32 = 0;
    let mut is_scene_change_count: u32 = 0;

    // C: (uint32_t)(((float)((w_regions * h_regions) * 50) / 100) + 0.5)
    let region_count_threshold =
        ((((regions_per_width * regions_per_height) * 50) as f32 / 100.0) + 0.5) as u32;

    // Declared OUTSIDE the loops in C and mutated inside — see the doc.
    let mut region_width = picture_width / regions_per_width as u32;
    let mut region_height = picture_height / regions_per_height as u32;

    for w in 0..regions_per_width {
        for h in 0..regions_per_height {
            let mut is_abrupt_change = false;
            let mut is_scene_change = false;

            let mut ahd: u32 = 0;

            let region_width_offset = if w == regions_per_width - 1 {
                picture_width.wrapping_sub(regions_per_width as u32 * region_width)
            } else {
                0
            };
            let region_height_offset = if h == regions_per_height - 1 {
                picture_height.wrapping_sub(regions_per_height as u32 * region_height)
            } else {
                0
            };
            region_width = region_width.wrapping_add(region_width_offset);
            region_height = region_height.wrapping_add(region_height_offset);

            let region_threshold =
                SCENE_TH.wrapping_mul(num_64x64_in_pic(region_width, region_height));

            for bin in 0..HISTOGRAM_NUMBER_OF_BINS {
                ahd = ahd.wrapping_add(
                    (current_hist[w][h][bin] as i32
                        - state.prev_picture_histogram[w][h][bin] as i32)
                        .unsigned_abs(),
                );
            }

            if state.reset_running_avg {
                state.ahd_running_avg[w][h] = ahd;
            }

            let ahd_error = (state.ahd_running_avg[w][h] as i32 - ahd as i32).unsigned_abs();

            if ahd_error > region_threshold && ahd >= ahd_error {
                is_abrupt_change = true;
            }
            if is_abrupt_change {
                // Average intensity differences, all narrowed to uint8_t by C
                // AFTER the abs — a difference above 255 wraps, which is
                // reproduced with the same truncation.
                // C: `(uint8_t)ABS((int16_t)a - (int16_t)b)`. Each 64-bit
                // value is TRUNCATED to int16 first, the subtraction happens
                // in `int` after integer promotion, and the absolute value is
                // then truncated to uint8. All three narrowings are
                // reproduced exactly; collapsing them changes the result for
                // any value outside 0..=255.
                let aid = |a: u64, b: u64| -> u8 {
                    let d = i32::from(a as i16) - i32::from(b as i16);
                    d.unsigned_abs() as u8
                };
                let prev = state.prev_average_intensity_per_region[w][h];
                let aid_future_past = aid(future_intensity[w][h], prev);
                let aid_future_present = aid(future_intensity[w][h], current_intensity[w][h]);
                let aid_present_past = aid(current_intensity[w][h], prev);

                if aid_future_past < FLASH_TH
                    && aid_future_present >= FLASH_TH
                    && aid_present_past >= FLASH_TH
                {
                    // A flash, not a scene change.
                } else if aid_future_present < FADE_TH && aid_present_past < FADE_TH {
                    // A fade, not a scene change.
                } else {
                    is_scene_change = true;
                }
            } else {
                state.ahd_running_avg[w][h] = (3u32
                    .wrapping_mul(state.ahd_running_avg[w][h])
                    .wrapping_add(ahd))
                    / 4;
            }
            is_abrupt_change_count += u32::from(is_abrupt_change);
            is_scene_change_count += u32::from(is_scene_change);
        }
    }

    state.reset_running_avg = is_abrupt_change_count >= region_count_threshold;
    is_scene_change_count >= region_count_threshold
}

/// What `perform_scene_change_detection` decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SceneChangeOutcome {
    /// C `pcs->scene_change_flag`.
    pub scene_change_flag: bool,
    /// C `pcs->cra_flag` after the update.
    pub cra_flag: bool,
    /// C `ctx->transition_detected`.
    pub transition_detected: i32,
    /// C `ctx->is_scene_change_detected`.
    pub is_scene_change_detected: bool,
}

/// C's `scs->scd_delay` derivation (`enc_handle.c:4005-4038`).
///
/// The number of FUTURE pictures `check_window_availability` requires in the
/// reorder queue before it lets a picture through picture decision — and
/// therefore before `perform_scene_change_detection` may run for it. The
/// detector itself only reads `pd_window[2]` (the immediate future), but the
/// availability gate scans `pd_window[2 .. 2+scd_delay)` and stalls the
/// release until all of them are present and none carries EOS.
#[must_use]
pub fn derive_scd_delay(
    intra_period_is_zero: bool,
    tf_params_per_type: &[TfCtrls; 3],
    scene_transition_armed: bool,
    lap_rc: bool,
) -> u32 {
    // `scd_delay_islice`: only the "non-delayed intra" shape
    // (`intra_period_length == 0`) can put an I slice's TF window into the
    // lookahead.
    let scd_delay_islice = if intra_period_is_zero && tf_params_per_type[0].enabled {
        u32::from(tf_params_per_type[0].num_future_pics.saturating_add(
            if tf_params_per_type[0].modulate_pics != 0 {
                TF_MAX_EXTENSION as u8
            } else {
                0
            },
        ))
        .min(u32::from(tf_params_per_type[0].max_num_future_pics))
    } else {
        0
    };
    let scd_delay_base = if tf_params_per_type[1].enabled {
        u32::from(tf_params_per_type[1].num_future_pics.saturating_add(
            if tf_params_per_type[1].modulate_pics != 0 {
                TF_MAX_EXTENSION as u8
            } else {
                0
            },
        ))
        .min(u32::from(tf_params_per_type[1].max_num_future_pics))
    } else {
        0
    };
    let mut delay = scd_delay_islice.max(scd_delay_base);
    // `enc_handle.c:4036-4038`: SCD (force-zeroed but transcribed), the
    // sharpness scene-transition arm, and lookahead RC all pin the floor at
    // two future pictures.
    if scene_transition_armed || lap_rc {
        delay = delay.max(2);
    }
    delay
}

/// C `perform_scene_change_detection` (`pd_process.c:4682-4700`) — static.
///
/// Which of the two detector calls fires is the whole content of this
/// function, and both are gated on settings measured above:
/// * `static_config.scene_change_detection` is force-zeroed
///   (`enc_settings.c:839-843`), so the first arm is dead in mainline;
/// * `vq_ctrls.sharpness_ctrls.scene_transition` is 1 in both arms of
///   `derive_vq_params` and zeroed only for LOW_DELAY, so the SECOND arm is
///   live in random access.
///
/// The second arm also only runs while `transition_detected` is -1 or 0 — once
/// a transition is latched at 1 it stays until `init_pic_settings` consumes it
/// at a base-layer picture.
///
/// `run_detector` is the [`scene_transition_detector`] result, computed by the
/// caller because it needs the picture window; `None` means the caller
/// determined neither arm fires.
#[must_use]
pub fn perform_scene_change_detection(
    scene_change_detection_enabled: bool,
    sharpness_scene_transition: bool,
    transition_detected_in: i32,
    cra_flag_in: bool,
    run_detector: impl FnOnce() -> bool,
) -> SceneChangeOutcome {
    let mut transition_detected = transition_detected_in;
    let scene_change_flag = if scene_change_detection_enabled {
        run_detector()
    } else {
        if sharpness_scene_transition
            && (transition_detected_in == -1 || transition_detected_in == 0)
        {
            transition_detected = i32::from(run_detector());
        }
        false
    };
    SceneChangeOutcome {
        scene_change_flag,
        cra_flag: if scene_change_flag { true } else { cra_flag_in },
        transition_detected,
        is_scene_change_detected: scene_change_flag,
    }
}

/// C `copy_histograms` (`pd_process.c:4703-4719`) — static.
///
/// Carries the current picture's histogram forward for the next input
/// picture. Without it [`calc_ahd_pd`] and [`scene_transition_detector`] read
/// zeros and every downstream threshold decision flips.
///
/// Trap: the loops run over `MAX_NUMBER_OF_REGIONS_IN_{WIDTH,HEIGHT}` (4 and
/// 4), NOT over `scs->picture_analysis_number_of_regions_per_*`. So regions
/// the detector never reads are copied anyway — reproduced, because a port
/// that copied only the active regions would leave stale data in the rest and
/// diverge the moment the region count changes.
pub fn copy_histograms(
    state: &mut SceneDetectState,
    picture_histogram: &RegionHistograms,
    average_intensity_per_region: &RegionIntensities,
) {
    for w in 0..MAX_NUMBER_OF_REGIONS_IN_WIDTH {
        for h in 0..MAX_NUMBER_OF_REGIONS_IN_HEIGHT {
            state.prev_picture_histogram[w][h] = picture_histogram[w][h];
            state.prev_average_intensity_per_region[w][h] = average_intensity_per_region[w][h];
        }
    }
}

// ---------------------------------------------------------------------------
// Dynamic-GOP detector — pd_process.c:403-758
// ---------------------------------------------------------------------------
//
// Reachability, measured (`enc_handle.c:4294-4300`): `scs->enable_dg` is 0 for
// VBR, CBR, >= 4K, non-RANDOM_ACCESS and multi-pass, and 1 otherwise — so this
// whole detector is ON BY DEFAULT for single-pass CQP/CRF random access below
// 4K. It is not an exotic knob, and a one-SAD difference here flips the
// mini-GOP SIZE, i.e. the temporal layer of every frame in it.

/// C `FULL_SAD_SEARCH` (`definitions.h:1821`).
pub const FULL_SAD_SEARCH: u8 = 1;
/// C `INPUT_SIZE_360p_RANGE` (`definitions.h:1825`).
pub const INPUT_SIZE_360P_RANGE: u8 = 1;
/// C `HIGH_DIST_TH` (`pd_process.c:684`) — `16 * 16 * 18`.
pub const HIGH_DIST_TH: u64 = 16 * 16 * 18;
/// C `LOW_DIST_TH` (`pd_process.c:685`) — `16 * 16 * 2`.
pub const LOW_DIST_TH: u64 = 16 * 16 * 2;

/// A borrowed view of a sixteenth-downsampled luma plane, as
/// `EbPictureBufferDesc` presents one to the detector.
///
/// `y_buffer` is the interior origin: the search can address NEGATIVE offsets
/// (up to `border - 1` pixels left/up), so the backing allocation must extend
/// `border` pixels in every direction and `origin` is where (0, 0) sits.
#[derive(Debug, Clone, Copy)]
pub struct DsPlane<'a> {
    /// The whole padded allocation.
    pub data: &'a [u8],
    /// Index of pixel (0, 0) within `data`.
    pub origin: usize,
    /// C `y_stride`.
    pub stride: usize,
    /// C `width` (the un-padded picture width).
    pub width: u16,
    /// C `height`.
    pub height: u16,
    /// C `border` — padding on each side.
    pub border: u16,
}

/// C `early_hme_b64` (`pd_process.c:403-491`) — static.
///
/// The per-64x64 search kernel underneath the dynamic-GOP detector. Needs a
/// bit-exact port because a one-SAD difference flips the GOP-size decision.
///
/// Returns `(best_sad, sr_center)`.
///
/// Traps transcribed literally:
/// * `sa_width` is rounded UP to a multiple of 8 on entry, then clamped
///   against the picture, then rounded DOWN to a multiple of 8 — but only when
///   it is at least 8 (`sa_width < 8 ? sa_width : sa_width & ~7`).
/// * the left-edge correction `sa_width = sa_width - (-pad_width - (org_x +
///   sa_origin_x))` is evaluated AFTER `sa_origin_x` has already been
///   reassigned on the line above, so the parenthesised term is identically
///   zero and the width is unchanged. That is C's code, not a transcription
///   slip; a "fixed" version would clamp differently.
/// * `best_sad` is doubled for the non-`FULL_SAD_SEARCH` method because only
///   every other line was summed, and the centre is scaled by 4 because the
///   search ran at sixteenth (quarter-per-axis) resolution.
///
/// # Panics
///
/// Panics if the search region falls outside `plane.data`; that is a caller
/// error in the padding setup, and reading the wrong memory would silently
/// produce a wrong SAD.
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn early_hme_b64(
    src: &[u8],
    src_stride: usize,
    hme_search_method: u8,
    org_x: i16,
    org_y: i16,
    block_width: u32,
    block_height: u32,
    sa_width_in: i16,
    sa_height_in: i16,
    ref_plane: &DsPlane<'_>,
) -> (u64, svtav1_types::motion::Mv) {
    // Round the search width up to a multiple of 8: the SAD kernel costs the
    // same for widths 1..8.
    let mut sa_width = (sa_width_in + 7) & !0x07;
    let mut sa_height = sa_height_in;
    let pad_width = ref_plane.border as i16 - 1;
    let pad_height = ref_plane.border as i16 - 1;

    let mut sa_origin_x = -(sa_width >> 1);
    let mut sa_origin_y = -(sa_height >> 1);

    // Left edge. NOTE: C reassigns sa_origin_x first, so the width adjustment
    // that follows subtracts zero. Reproduced exactly.
    if org_x + sa_origin_x < -pad_width {
        sa_origin_x = -pad_width - org_x;
        sa_width -= -pad_width - (org_x + sa_origin_x);
    }
    // Right edge.
    if org_x + sa_origin_x > ref_plane.width as i16 - 1 {
        sa_origin_x -= (org_x + sa_origin_x) - (ref_plane.width as i16 - 1);
    }
    if org_x + sa_origin_x + sa_width > ref_plane.width as i16 {
        sa_width = 1.max(sa_width - ((org_x + sa_origin_x + sa_width) - ref_plane.width as i16));
    }
    // Round DOWN to a multiple of 8, but only at 8 or more.
    sa_width = if sa_width < 8 {
        sa_width
    } else {
        sa_width & !0x07
    };

    // Top edge — same shape as the left edge, same zero-subtraction.
    if org_y + sa_origin_y < -pad_height {
        sa_origin_y = -pad_height - org_y;
        sa_height -= -pad_height - (org_y + sa_origin_y);
    }
    if org_y + sa_origin_y > ref_plane.height as i16 - 1 {
        sa_origin_y -= (org_y + sa_origin_y) - (ref_plane.height as i16 - 1);
    }
    if org_y + sa_origin_y + sa_height > ref_plane.height as i16 {
        sa_height =
            1.max(sa_height - ((org_y + sa_origin_y + sa_height) - ref_plane.height as i16));
    }

    let x_top_left = org_x + sa_origin_x;
    let y_top_left = org_y + sa_origin_y;
    let search_region_index =
        i64::from(x_top_left) + i64::from(y_top_left) * ref_plane.stride as i64;

    let full = hme_search_method == FULL_SAD_SEARCH;
    let r = crate::inter_me::sad::sad_loop_kernel(
        src,
        if full { src_stride } else { src_stride * 2 },
        ref_plane.data,
        ref_plane.origin as i64 + search_region_index,
        if full {
            ref_plane.stride
        } else {
            ref_plane.stride * 2
        },
        if full {
            block_height as usize
        } else {
            (block_height >> 1) as usize
        },
        block_width as usize,
        ref_plane.stride,
        0,
        sa_width,
        sa_height,
    );

    let best_sad = if full { r.best_sad } else { r.best_sad * 2 };
    let mut mv = svtav1_types::motion::Mv { x: 0, y: 0 };
    // Operating on 1/4 resolution per axis, hence the x4.
    mv.x = (r.x_search_center + sa_origin_x) * 4;
    mv.y = (r.y_search_center + sa_origin_y) * 4;
    (best_sad, mv)
}

/// C `DGDetectorMetrics` (`pcs.h:710-716`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DgDetectorMetrics {
    /// C `tot_dist`.
    pub tot_dist: u64,
    /// C `tot_cplx`.
    pub tot_cplx: u32,
    /// C `tot_active`.
    pub tot_active: u32,
    /// C `sum_in_vectors`.
    pub sum_in_vectors: i32,
    /// C `seg_completed`.
    pub seg_completed: u16,
}

/// C `dg_detector_hme_level0` (`pd_process.c:532-629`) — EXPORTED.
///
/// Segment-level entry to the dynamic-GOP HME. Accumulates distortion,
/// complexity, activity and an inward/outward motion-vector balance over the
/// segment's 64x64 blocks.
///
/// Traps:
/// * `hme_level0_sad` and `sr_center` are declared OUTSIDE the block loop and
///   passed by pointer to [`early_hme_b64`], which OVERWRITES both, so no
///   value carries between blocks — but the initial `~0` seed does reach the
///   first call. `sad_loop_kernel` ignores its incoming value, so this is
///   inert; it is preserved so a reader is not tempted to "fix" it.
/// * the `sum_in_vectors` update SKIPS the middle row and column exactly
///   (`< n/2` and `> n/2`, never `==`), so an odd block count leaves one row
///   and one column contributing nothing.
///
/// `metrics` is accumulated in place, matching C's mutex-guarded shared
/// struct.
#[allow(clippy::too_many_arguments)]
pub fn dg_detector_hme_level0(
    metrics: &mut DgDetectorMetrics,
    src_plane: &DsPlane<'_>,
    ref_plane: &DsPlane<'_>,
    input_resolution: u8,
    aligned_width: u32,
    aligned_height: u32,
    b64_size: u32,
    seg_idx: u32,
    me_segments_column_count: u32,
    me_segments_row_count: u32,
) {
    let (sa_width, sa_height) = if input_resolution <= INPUT_SIZE_360P_RANGE {
        (16i16, 16i16)
    } else if input_resolution <= INPUT_SIZE_480P_RANGE {
        (64, 64)
    } else {
        (128, 128)
    };

    let pic_width_in_b64 = aligned_width.div_ceil(b64_size);
    let pic_height_in_b64 = aligned_height.div_ceil(b64_size);

    // SEGMENT_CONVERT_IDX_TO_XY(seg_idx, x, y, me_segments_column_count)
    let y_seg_idx = seg_idx / me_segments_column_count;
    let x_seg_idx = seg_idx - y_seg_idx * me_segments_column_count;
    let x_b64_start = (x_seg_idx * pic_width_in_b64) / me_segments_column_count;
    let x_b64_end = ((x_seg_idx + 1) * pic_width_in_b64) / me_segments_column_count;
    let y_b64_start = (y_seg_idx * pic_height_in_b64) / me_segments_row_count;
    let y_b64_end = ((y_seg_idx + 1) * pic_height_in_b64) / me_segments_row_count;

    for y_b64_idx in y_b64_start..y_b64_end {
        for x_b64_idx in x_b64_start..x_b64_end {
            let b64_origin_x = x_b64_idx * 64;
            let b64_origin_y = y_b64_idx * 64;
            let buffer_index =
                (b64_origin_y >> 2) as usize * src_plane.stride + (b64_origin_x >> 2) as usize;

            let (sad, sr_center) = early_hme_b64(
                &src_plane.data[src_plane.origin + buffer_index..],
                src_plane.stride,
                FULL_SAD_SEARCH,
                (b64_origin_x as i16) >> 2,
                (b64_origin_y as i16) >> 2,
                16,
                16,
                sa_width,
                sa_height,
                ref_plane,
            );

            metrics.tot_dist += sad;
            metrics.tot_cplx += u32::from(sad > (16 * 16 * 30));
            metrics.tot_active += u32::from(sr_center.x.abs() > 0 || sr_center.y.abs() > 0);

            // Row balance: the MIDDLE row is skipped (never `==`).
            if y_b64_idx < pic_height_in_b64 / 2 {
                if sr_center.y > 0 {
                    metrics.sum_in_vectors -= 1;
                } else if sr_center.y < 0 {
                    metrics.sum_in_vectors += 1;
                }
            } else if y_b64_idx > pic_height_in_b64 / 2 {
                if sr_center.y > 0 {
                    metrics.sum_in_vectors += 1;
                } else if sr_center.y < 0 {
                    metrics.sum_in_vectors -= 1;
                }
            }
            // Column balance: same shape, same skipped middle.
            if x_b64_idx < pic_width_in_b64 / 2 {
                if sr_center.x > 0 {
                    metrics.sum_in_vectors -= 1;
                } else if sr_center.x < 0 {
                    metrics.sum_in_vectors += 1;
                }
            } else if x_b64_idx > pic_width_in_b64 / 2 {
                if sr_center.x > 0 {
                    metrics.sum_in_vectors += 1;
                } else if sr_center.x < 0 {
                    metrics.sum_in_vectors -= 1;
                }
            }
        }
    }
    metrics.seg_completed += 1;
}

/// The per-pair result `early_hme` leaves on the picture-decision context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EarlyHmeResult {
    /// C `ctx->mv_in_out_count`.
    pub mv_in_out_count: i16,
    /// C `ctx->norm_dist`.
    pub norm_dist: u64,
    /// C `ctx->perc_cplx`.
    pub perc_cplx: u8,
    /// C `ctx->perc_active`.
    pub perc_active: u8,
}

/// C `early_hme`'s reduction (`pd_process.c:669-684`) — static.
///
/// Turns the accumulated [`DgDetectorMetrics`] into the four per-pair numbers
/// `calc_mini_gop_activity` consumes. The segment dispatch around it is
/// threading this port replaces; the caller runs
/// [`dg_detector_hme_level0`] over its segments and passes the totals.
///
/// Trap: the block counts here use a HARDCODED 64
/// (`(aligned_width + 63) / 64`), not `scs->b64_size` as
/// `dg_detector_hme_level0` does. The two agree at the default b64_size of 64
/// and would not at 128 — reproduced rather than unified.
#[must_use]
pub fn early_hme_reduce(
    metrics: &DgDetectorMetrics,
    aligned_width: u32,
    aligned_height: u32,
) -> EarlyHmeResult {
    let pic_width_in_b64 = aligned_width.div_ceil(64);
    let pic_height_in_b64 = aligned_height.div_ceil(64);
    let blocks = i64::from(pic_height_in_b64 * pic_width_in_b64);
    EarlyHmeResult {
        mv_in_out_count: (i64::from(metrics.sum_in_vectors) * 100 / blocks) as i16,
        norm_dist: metrics.tot_dist / (blocks as u64),
        perc_cplx: ((u64::from(metrics.tot_cplx) * 100) / blocks as u64) as u8,
        perc_active: ((u64::from(metrics.tot_active) * 100) / blocks as u64) as u8,
    }
}

/// C `calc_mini_gop_activity` (`pd_process.c:686-712`) — static.
///
/// The 6L-vs-5L split decision. Returns `true` when the top layer should be
/// re-activated (i.e. SPLIT into the two sub layers); the caller then sets
/// `activity[top] = true` and `activity[sub0] = activity[sub1] = false`.
///
/// Traps:
/// * `bias` is 25 when the previous mini-GOP in this GOP was 5L and this is
///   not the first mini-GOP, else 75 — a hysteresis toward staying at 6L.
/// * `top_layer_mv_in_out_count` is `(void)`-cast UNUSED in C. It is accepted
///   here for call-site fidelity and deliberately not read.
/// * the CALL SITE passes the two sub-layer mv counts in the order
///   (end-mid, mid-start) into parameters named `..._count1` and `..._count2`,
///   so `count1` belongs to `sub_layer_idx1` and `count2` to `sub_layer_idx0`
///   — the names are inverted relative to the indices. `cond3` takes a MIN and
///   a MAX so the order does not change the result, which is presumably why it
///   was never noticed.
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn calc_mini_gop_activity(
    mini_gop_cnt_per_gop: u32,
    previous_mini_gop_hierarchical_levels: u32,
    top_layer_dist: u64,
    top_layer_perc_active: u8,
    top_layer_perc_cplx: u8,
    sub_layer_dist0: u64,
    sub_layer0_perc_active: u8,
    sub_layer0_perc_cplx: u8,
    sub_layer_dist1: u64,
    sub_layer1_perc_active: u8,
    sub_layer1_perc_cplx: u8,
    _top_layer_mv_in_out_count: i16,
    sub_layer_mv_in_out_count1: i16,
    sub_layer_mv_in_out_count2: i16,
) -> bool {
    let bias: u64 = if mini_gop_cnt_per_gop > 1 && previous_mini_gop_hierarchical_levels == 5 {
        25
    } else {
        75
    };

    let cond1 = top_layer_perc_active >= 95
        && !(sub_layer0_perc_active >= 95 && sub_layer1_perc_active < 75)
        && !(sub_layer0_perc_active < 75 && sub_layer1_perc_active >= 95);
    let cond2 = top_layer_dist > LOW_DIST_TH
        && sub_layer_dist0 < HIGH_DIST_TH
        && sub_layer_dist1 < HIGH_DIST_TH
        && top_layer_perc_cplx > 0
        && sub_layer0_perc_cplx < 25
        && sub_layer1_perc_cplx < 25
        && ((sub_layer_dist0 + sub_layer_dist1) / 2) < ((bias * top_layer_dist) / 100);
    let cond3 = sub_layer_mv_in_out_count1.min(sub_layer_mv_in_out_count2) > 40
        && sub_layer_mv_in_out_count1.max(sub_layer_mv_in_out_count2) > 55;

    cond1 && (cond2 || cond3)
}

/// C `eval_sub_mini_gop` (`pd_process.c:713-758`) — static.
///
/// Runs three `early_hme` passes — (end, start), (end, mid), (mid, start) —
/// and commits the split. The caller supplies the three reductions because the
/// HME itself needs picture buffers; this is the decision half.
///
/// Argument-order trap, and it is the reason this wrapper exists at all: the
/// C call maps `(end,start)` to the TOP layer, `(mid,start)` to sub layer 0
/// and `(end,mid)` to sub layer 1 — NOT in the order the three passes are run.
/// Getting that mapping wrong silently swaps the two sub layers' statistics.
#[must_use]
pub fn eval_sub_mini_gop(
    mini_gop_cnt_per_gop: u32,
    previous_mini_gop_hierarchical_levels: u32,
    end_start: EarlyHmeResult,
    end_mid: EarlyHmeResult,
    mid_start: EarlyHmeResult,
) -> bool {
    calc_mini_gop_activity(
        mini_gop_cnt_per_gop,
        previous_mini_gop_hierarchical_levels,
        // top layer <- (end, start)
        end_start.norm_dist,
        end_start.perc_active,
        end_start.perc_cplx,
        // sub layer 0 <- (mid, start)
        mid_start.norm_dist,
        mid_start.perc_active,
        mid_start.perc_cplx,
        // sub layer 1 <- (end, mid)
        end_mid.norm_dist,
        end_mid.perc_active,
        end_mid.perc_cplx,
        end_start.mv_in_out_count,
        end_mid.mv_in_out_count,
        mid_start.mv_in_out_count,
    )
}

/// Apply [`eval_sub_mini_gop`]'s verdict to the activity array, as C does at
/// `pd_process.c:707-711`.
pub fn commit_sub_mini_gop_split(
    map: &mut MiniGopMap,
    split: bool,
    top_layer_idx: usize,
    sub_layer_idx0: usize,
    sub_layer_idx1: usize,
) {
    if split {
        map.activity[top_layer_idx] = true;
        map.activity[sub_layer_idx0] = false;
        map.activity[sub_layer_idx1] = false;
    }
}

// ---------------------------------------------------------------------------
// Temporal-filter window — pd_process.c:3642-4250
// ---------------------------------------------------------------------------
//
// Reachability, measured: in LOW_DELAY `tf_level` is forced to 0 before any
// preset logic (`enc_handle.c:3339-3343`), so this whole group is dead for the
// campaign's first cell. It is ON BY DEFAULT in random access (tf_level 5 at
// M3-M7), where the temporal filter REWRITES THE SOURCE PIXELS of base-layer
// frames — so no random-access frame can be byte-identical without it.

/// C `ALTREF_MAX_NFRAMES` (`definitions.h:338`).
pub const ALTREF_MAX_NFRAMES: usize = 33;
/// C `TF_MAX_EXTENSION` (`definitions.h:340`).
pub const TF_MAX_EXTENSION: i32 = 6;
/// C `TF_MAX_BASE_REF_PICS` (`definitions.h:341`).
pub const TF_MAX_BASE_REF_PICS: u8 = 7;
/// C `TF_MAX_L1_REF_PICS_6L` (`definitions.h:342`).
pub const TF_MAX_L1_REF_PICS_6L: u8 = 2;
/// C `TF_MAX_L1_REF_PICS_SUB_6L` (`definitions.h:343`).
pub const TF_MAX_L1_REF_PICS_SUB_6L: u8 = 1;
/// C `VQ_NOISE_LVL_TH` (`definitions.h:83`).
pub const VQ_NOISE_LVL_TH: i32 = 15000;

/// C `svt_aom_tf_max_ref_per_struct` (`enc_handle.c:2506-2519`) — EXPORTED.
///
/// The per-side cap on temporal-filter reference pictures.
///
/// Two traps: `direction` is `(void)`-cast UNUSED (past and future share a
/// cap), and the `type` encoding is 0 = I_SLICE, 1 = BASE, 2 = L1 — the
/// I_SLICE arm is `1 << hierarchical_levels`, which is the ONLY arm that grows
/// with the hierarchy.
#[must_use]
pub fn tf_max_ref_per_struct(hierarchical_levels: u32, ty: u8, _direction: bool) -> u8 {
    if ty == 0 {
        // C computes `1 << hierarchical_levels` into a uint8_t, so a hierarchy
        // of 8 or more wraps. Reproduced with a wrapping shift.
        (1u32 << hierarchical_levels) as u8
    } else if ty == 1 {
        TF_MAX_BASE_REF_PICS
    } else if hierarchical_levels < 5 {
        TF_MAX_L1_REF_PICS_SUB_6L
    } else {
        TF_MAX_L1_REF_PICS_6L
    }
}
