//! Wiener loop-restoration search + decoder-exact frame application.
//!
//! C sources (SVT-AV1 v4.2.0-rc):
//! - Controls: `svt_aom_get_wn_filter_level_allintra` (enc_mode_config.c:1928
//!   — M0..M3 -> 3, M4..M6 -> 4, else 0) + `svt_aom_set_wn_filter_ctrls`
//!   (:1758). `sg_filter_lvl = 0` for every representable allintra preset
//!   (`svt_aom_get_sg_filter_level_allintra`, :2000 — level 1 requires
//!   ENC_MR = -1), so sgrproj is NEVER searched and `rest_finish_search`
//!   force-types WIENER-vs-NONE only (restoration_pick.c:1565).
//! - Search: `restoration_seg_search` (restoration_pick.c:1474) —
//!   `svt_extend_frame(dgd, .., RESTORATION_BORDER+1+align16_pad,
//!   RESTORATION_BORDER)` then per-unit `search_norestore_seg` (:1432) and
//!   `search_wiener_seg` (:1306): compute_stats -> wiener_decompose_sep_sym
//!   -> finalize_sym_filter -> compute_score>0 revert ->
//!   `finer_tile_search_wiener_seg` (:1041; refinement per
//!   `wn_filter_ctrls.use_refinement`) where `try_restoration_unit_seg`
//!   (:123) filters with `need_boundaries = use_boundaries_in_rest_search`
//!   = **0** (enc_handle.c:4483) and SSEs vs the source.
//! - Finish: `rest_finish_search` (:1561) — per plane, frame-level RD over
//!   {NONE, WIENER}: `search_rest_type_finish` (:1458) resets {sse, bits}
//!   and the reference filters (`rsc_on_tile`, :85), walks units with
//!   `search_norestore_finish` (:1444 — NO bits) / `search_wiener_finish`
//!   (:1383 — wiener_restore flag cost + `count_wiener_bits` at the SEARCH
//!   window, RDCOST_DBL with `x->rdmult` = the unweighted kf lambda,
//!   enc_dec_process.c:3512), frame cost `RDCOST_DBL(rdmult, bits>>4,
//!   sse)`, strict-< argmin with NONE first.
//! - Application: `svt_av1_loop_restoration_filter_frame` (restoration.c:
//!   1154) — per non-NONE plane: `svt_extend_frame(.., 3, 3)`, per-unit
//!   `filter_unit` WITH boundaries into a dst buffer, then plane copy-back.
//!   Boundaries: `svt_av1_loop_restoration_save_boundary_lines` after
//!   deblock (dlf_process.c:134, after_cdef=0) and after CDEF
//!   (cdef_process.c:707, after_cdef=1).
//!
//! Instrumented ground truth (scratch build, SVT_LRDBG dumps, OBUs verified
//! byte-identical to baseline): docs/captures/gradient_*_p6.lrdbg.txt —
//! pinned in the unit tests below.

use svtav1_dsp::restoration::{
    alloc_stripe_boundaries, compute_score, compute_stats, extend_frame, finalize_sym_filter,
    foreach_rest_unit_in_tile, loop_restoration_filter_unit, save_tile_row_boundary_lines,
    sse_region, wiener_decompose_sep_sym, PixelRect, StripeBoundaries, TileLimits, WienerInfo,
    RESTORATION_UNITSIZE_MAX, RESTORE_NONE, RESTORE_WIENER, WIENER_FILT_TAP0_MAXV,
    WIENER_FILT_TAP0_MINV, WIENER_FILT_TAP1_MAXV, WIENER_FILT_TAP1_MINV, WIENER_FILT_TAP2_MAXV,
    WIENER_FILT_TAP2_MINV, WIENER_WIN, WIENER_WIN_CHROMA,
};

/// C `WnFilterCtrls` (the fields the still path consumes).
#[derive(Clone, Copy, Debug)]
pub struct WnFilterCtrls {
    pub enabled: bool,
    pub use_chroma: bool,
    /// 1 -> 7x7 luma taps, 2 -> 5x5 luma taps (chroma is always 5x5).
    pub filter_tap_lvl: u8,
    pub use_refinement: bool,
    pub max_one_refinement_step: bool,
}

/// C `svt_aom_get_wn_filter_level_allintra` + `svt_aom_set_wn_filter_ctrls`
/// (enc_mode_config.c:1928 / :1758): level 3 for presets <= 3, level 4 for
/// 4..=6, disabled above.
pub fn wn_filter_ctrls_allintra(preset: u8) -> WnFilterCtrls {
    if preset <= 3 {
        WnFilterCtrls {
            enabled: true,
            use_chroma: true,
            filter_tap_lvl: 2,
            use_refinement: true,
            max_one_refinement_step: true,
        }
    } else if preset <= 6 {
        WnFilterCtrls {
            enabled: true,
            use_chroma: true,
            filter_tap_lvl: 2,
            use_refinement: false,
            max_one_refinement_step: true,
        }
    } else {
        WnFilterCtrls {
            enabled: false,
            use_chroma: false,
            filter_tap_lvl: 2,
            use_refinement: false,
            max_one_refinement_step: true,
        }
    }
}

/// Per-restoration-unit outcome.
#[derive(Clone, Copy, Debug)]
pub struct RestUnit {
    pub rtype: u8,
    pub wiener: WienerInfo,
}

/// Per-plane restoration info (C `RestorationInfo`).
#[derive(Clone, Debug)]
pub struct PlaneRest {
    pub frame_rtype: u8,
    pub unit_size: i32,
    pub hunits: i32,
    pub vunits: i32,
    pub units: alloc::vec::Vec<RestUnit>,
}

impl PlaneRest {
    fn none(unit_size: i32, hunits: i32, vunits: i32) -> Self {
        PlaneRest {
            frame_rtype: RESTORE_NONE,
            unit_size,
            hunits,
            vunits,
            units: alloc::vec![
                RestUnit { rtype: RESTORE_NONE, wiener: WienerInfo::default() };
                (hunits * vunits) as usize
            ],
        }
    }
}

/// Frame restoration info for all planes.
#[derive(Clone, Debug)]
pub struct FrameRestInfo {
    pub planes: alloc::vec::Vec<PlaneRest>,
}

impl FrameRestInfo {
    pub fn any_non_none(&self) -> bool {
        self.planes.iter().any(|p| p.frame_rtype != RESTORE_NONE)
    }
}

/// A plane padded with a 4-pixel border on every side (>= the 3+1 the
/// search extend uses horizontally and >= every read/write the stripe
/// machinery performs: setup touches columns h_start-4 .. h_end+4 and rows
/// v_start-3 .. v_end+2; the convolve reads 3/3/3/4).
pub struct PaddedPlane {
    pub data: alloc::vec::Vec<u8>,
    pub stride: usize,
    pub origin: usize,
    pub w: usize,
    pub h: usize,
}

pub const PLANE_BORDER: usize = 4;

impl PaddedPlane {
    /// Copy a tight `w x h` plane into padded storage (borders zero until
    /// `extend()` replicates them).
    pub fn from_tight(src: &[u8], w: usize, h: usize) -> Self {
        let stride = w + 2 * PLANE_BORDER;
        let mut data = alloc::vec![0u8; stride * (h + 2 * PLANE_BORDER)];
        let origin = PLANE_BORDER * stride + PLANE_BORDER;
        for y in 0..h {
            data[origin + y * stride..origin + y * stride + w]
                .copy_from_slice(&src[y * w..y * w + w]);
        }
        PaddedPlane {
            data,
            stride,
            origin,
            w,
            h,
        }
    }

    fn empty(w: usize, h: usize) -> Self {
        let stride = w + 2 * PLANE_BORDER;
        PaddedPlane {
            data: alloc::vec![0u8; stride * (h + 2 * PLANE_BORDER)],
            stride,
            origin: PLANE_BORDER * stride + PLANE_BORDER,
            w,
            h,
        }
    }

    /// Copy the crop back into a tight buffer.
    fn copy_crop_to(&self, dst: &mut [u8]) {
        for y in 0..self.h {
            dst[y * self.w..y * self.w + self.w].copy_from_slice(
                &self.data[self.origin + y * self.stride..self.origin + y * self.stride + self.w],
            );
        }
    }
}

/// `whole_frame_rect` for a plane (restoration.c:51): no superres, so the
/// plane rect is just its cropped dimensions.
fn plane_rect(pw: i32, ph: i32) -> PixelRect {
    PixelRect {
        left: 0,
        top: 0,
        right: pw,
        bottom: ph,
    }
}

/// wiener_restore flag costs from the default CDF (AOM_CDF2(11570)):
/// C `svt_aom_get_syntax_rate_from_cdf(wiener_restore_fac_bits,
/// fc->wiener_restore_cdf, NULL)` (md_rate_estimation.c:250) over the
/// pic-level (default) frame context — instrumented: [768, 320].
fn wiener_restore_cost() -> [i64; 2] {
    let icdf0 = svtav1_entropy::context::FrameContext::new_default().wiener_restore_cdf[0] as u32;
    let p0 = 32768 - icdf0;
    let p1 = icdf0;
    [
        svtav1_entropy::context::av1_cost_symbol(p0) as i64,
        svtav1_entropy::context::av1_cost_symbol(p1) as i64,
    ]
}

/// C `RDCOST_DBL` (restoration.h:344): rate in 1/512-bit units (already
/// `>> 4`-ed by the callers), double math.
fn rdcost_dbl(rdmult: i64, rate: i64, dist: i64) -> f64 {
    (rate as f64 * rdmult as f64) / (1u32 << 9) as f64 + dist as f64 * (1u32 << 7) as f64
}

/// One plane's per-unit search results (C `RestUnitSearchInfo` slice).
struct UnitSearch {
    sse_none: i64,
    /// i64::MAX == the compute_score>0 revert (C INT64_MAX sentinel).
    sse_wiener: i64,
    wiener: WienerInfo,
}

/// C `try_restoration_unit_seg` (restoration_pick.c:123) at
/// `use_boundaries_in_rest_search = 0`: filter the unit (no stripe-boundary
/// overwrites) from the extended dgd into the trial buffer, then SSE vs the
/// source over the unit rect.
#[allow(clippy::too_many_arguments)]
fn try_restoration_unit(
    dgd: &mut PaddedPlane,
    trial: &mut PaddedPlane,
    src: &[u8],
    src_stride: usize,
    limits: &TileLimits,
    rect: &PixelRect,
    ss: i32,
    wiener: &WienerInfo,
) -> i64 {
    let empty_bounds = StripeBoundaries::default();
    loop_restoration_filter_unit(
        false,
        limits,
        RESTORE_WIENER,
        wiener,
        &empty_bounds,
        rect,
        0,
        ss,
        ss,
        &mut dgd.data,
        dgd.origin,
        dgd.stride,
        &mut trial.data,
        trial.origin,
        trial.stride,
    );
    sse_region(
        src,
        (limits.v_start as usize) * src_stride + limits.h_start as usize,
        src_stride,
        &trial.data,
        trial.origin + (limits.v_start as usize) * trial.stride + limits.h_start as usize,
        trial.stride,
        (limits.h_end - limits.h_start) as usize,
        (limits.v_end - limits.v_start) as usize,
    )
}

/// C `finer_tile_search_wiener_seg` (restoration_pick.c:1041): base SSE via
/// try_restoration_unit, then (when `use_refinement`) the +-step tap hill
/// climb over hfilter then vfilter, taps plane_off..WIENER_HALFWIN.
#[allow(clippy::too_many_arguments)]
fn finer_tile_search_wiener(
    ctrls: &WnFilterCtrls,
    dgd: &mut PaddedPlane,
    trial: &mut PaddedPlane,
    src: &[u8],
    src_stride: usize,
    limits: &TileLimits,
    rect: &PixelRect,
    ss: i32,
    wiener: &mut WienerInfo,
    wiener_win: usize,
) -> i64 {
    let plane_off = (WIENER_WIN - wiener_win) >> 1;
    let mut err = try_restoration_unit(dgd, trial, src, src_stride, limits, rect, ss, wiener);
    if !ctrls.use_refinement {
        return err;
    }
    let start_step = 4i32;
    let end_step = if ctrls.max_one_refinement_step { 4 } else { 1 };
    let tap_min = [
        WIENER_FILT_TAP0_MINV as i16,
        WIENER_FILT_TAP1_MINV as i16,
        WIENER_FILT_TAP2_MINV as i16,
    ];
    let tap_max = [
        WIENER_FILT_TAP0_MAXV as i16,
        WIENER_FILT_TAP1_MAXV as i16,
        WIENER_FILT_TAP2_MAXV as i16,
    ];
    let halfwin = WIENER_WIN >> 1; // 3

    let mut s = start_step;
    while s >= end_step {
        // hfilter pass, then vfilter pass — C order.
        for pass in 0..2 {
            for p in plane_off..halfwin {
                let mut skip = false;
                // minus direction
                loop {
                    let f = if pass == 0 {
                        &mut wiener.hfilter
                    } else {
                        &mut wiener.vfilter
                    };
                    if f[p] - s as i16 >= tap_min[p] {
                        f[p] -= s as i16;
                        f[WIENER_WIN - p - 1] -= s as i16;
                        f[halfwin] += 2 * s as i16;
                        let err2 = try_restoration_unit(
                            dgd, trial, src, src_stride, limits, rect, ss, wiener,
                        );
                        if err2 > err {
                            let f = if pass == 0 {
                                &mut wiener.hfilter
                            } else {
                                &mut wiener.vfilter
                            };
                            f[p] += s as i16;
                            f[WIENER_WIN - p - 1] += s as i16;
                            f[halfwin] -= 2 * s as i16;
                        } else {
                            err = err2;
                            skip = true;
                            if s == start_step && !ctrls.max_one_refinement_step {
                                continue;
                            }
                        }
                    }
                    break;
                }
                if skip {
                    break;
                }
                // plus direction
                loop {
                    let f = if pass == 0 {
                        &mut wiener.hfilter
                    } else {
                        &mut wiener.vfilter
                    };
                    if f[p] + s as i16 <= tap_max[p] {
                        f[p] += s as i16;
                        f[WIENER_WIN - p - 1] += s as i16;
                        f[halfwin] -= 2 * s as i16;
                        let err2 = try_restoration_unit(
                            dgd, trial, src, src_stride, limits, rect, ss, wiener,
                        );
                        if err2 > err {
                            let f = if pass == 0 {
                                &mut wiener.hfilter
                            } else {
                                &mut wiener.vfilter
                            };
                            f[p] -= s as i16;
                            f[WIENER_WIN - p - 1] -= s as i16;
                            f[halfwin] += 2 * s as i16;
                        } else {
                            err = err2;
                            if s == start_step && !ctrls.max_one_refinement_step {
                                continue;
                            }
                        }
                    }
                    break;
                }
            }
        }
        s >>= 1;
    }
    err
}

/// The full C-exact still-frame restoration search. `recon_*` are the
/// POST-CDEF planes, `src_*` the source planes (tight buffers, stride = the
/// plane width). `rdmult` is `x->rdmult` = the unweighted kf lambda.
///
/// Returns per-plane frame types + per-unit picks with C's exact RD.
#[allow(clippy::too_many_arguments)]
pub fn search_restoration_still(
    ctrls: &WnFilterCtrls,
    src_y: &[u8],
    src_u: &[u8],
    src_v: &[u8],
    recon_y: &[u8],
    recon_u: &[u8],
    recon_v: &[u8],
    w: usize,
    h: usize,
    has_chroma: bool,
    rdmult: i64,
) -> FrameRestInfo {
    debug_assert!(ctrls.enabled);
    let wn_luma = if ctrls.filter_tap_lvl == 1 {
        WIENER_WIN
    } else {
        WIENER_WIN_CHROMA
    };
    let restore_cost = wiener_restore_cost();

    // set_restoration_unit_size (pcs.c:30): 256 for all planes (s = 0).
    let unit_size = RESTORATION_UNITSIZE_MAX;

    let plane_end = if has_chroma && ctrls.use_chroma { 2 } else { 0 };
    let mut planes = alloc::vec::Vec::new();

    for plane in 0..3usize {
        let is_uv = plane > 0;
        let ss = i32::from(is_uv);
        let (pw, ph) = if is_uv { (w / 2, h / 2) } else { (w, h) };
        let hunits = svtav1_dsp::restoration::count_units_in_tile(unit_size, pw as i32);
        let vunits = svtav1_dsp::restoration::count_units_in_tile(unit_size, ph as i32);

        if plane > plane_end {
            planes.push(PlaneRest::none(unit_size, hunits, vunits));
            continue;
        }
        let (src, recon) = match plane {
            0 => (src_y, recon_y),
            1 => (src_u, recon_u),
            _ => (src_v, recon_v),
        };
        let wiener_win = if plane == 0 { wn_luma } else { WIENER_WIN_CHROMA };
        let rect = plane_rect(pw as i32, ph as i32);

        // svt_extend_frame(dgd, ..) with RESTORATION_BORDER+1(+pad16) horz /
        // RESTORATION_BORDER vert — values beyond +-3 never affect results,
        // our PLANE_BORDER=4 covers every touched byte.
        let mut dgd = PaddedPlane::from_tight(recon, pw, ph);
        extend_frame(&mut dgd.data, dgd.origin, pw, ph, dgd.stride, 4, 3);
        let mut trial = PaddedPlane::empty(pw, ph);

        // ---- search phase (per-unit sse_none + wiener solve/SSE) ----
        let nunits = (hunits * vunits) as usize;
        let mut units: alloc::vec::Vec<UnitSearch> = alloc::vec::Vec::with_capacity(nunits);
        for _ in 0..nunits {
            units.push(UnitSearch {
                sse_none: 0,
                sse_wiener: i64::MAX,
                wiener: WienerInfo::default(),
            });
        }

        foreach_rest_unit_in_tile(&rect, hunits, unit_size, ss, |limits, unit_idx| {
            // search_norestore_seg: SSE of the unfiltered recon vs source.
            units[unit_idx as usize].sse_none = sse_region(
                src,
                (limits.v_start as usize) * pw + limits.h_start as usize,
                pw,
                recon,
                (limits.v_start as usize) * pw + limits.h_start as usize,
                pw,
                (limits.h_end - limits.h_start) as usize,
                (limits.v_end - limits.v_start) as usize,
            );
        });

        foreach_rest_unit_in_tile(&rect, hunits, unit_size, ss, |limits, unit_idx| {
            // search_wiener_seg.
            let win2 = wiener_win * wiener_win;
            let mut m = [0i64; WIENER_WIN * WIENER_WIN];
            let mut hh = alloc::vec![0i64; win2 * win2];
            compute_stats(
                wiener_win,
                &dgd.data,
                dgd.origin,
                dgd.stride,
                src,
                0,
                pw,
                limits.h_start,
                limits.h_end,
                limits.v_start,
                limits.v_end,
                &mut m,
                &mut hh,
            );
            let mut vd = [0i32; WIENER_WIN];
            let mut hd = [0i32; WIENER_WIN];
            wiener_decompose_sep_sym(wiener_win, &m, &hh, &mut vd, &mut hd);
            let mut wi = WienerInfo {
                vfilter: [0; 8],
                hfilter: [0; 8],
            };
            finalize_sym_filter(wiener_win, &vd, &mut wi.vfilter);
            finalize_sym_filter(wiener_win, &hd, &mut wi.hfilter);

            if compute_score(wiener_win, &m, &hh, &wi.vfilter, &wi.hfilter) > 0 {
                units[unit_idx as usize].sse_wiener = i64::MAX;
                return;
            }
            let sse = finer_tile_search_wiener(
                ctrls, &mut dgd, &mut trial, src, pw, limits, &rect, ss, &mut wi, wiener_win,
            );
            units[unit_idx as usize].sse_wiener = sse;
            units[unit_idx as usize].wiener = wi;
        });

        // ---- finish phase: frame-level {NONE, WIENER} RD ----
        // r = RESTORE_NONE walk: bits stay 0 (search_norestore_finish).
        let mut sse_frame_none = 0i64;
        for u in &units {
            sse_frame_none += u.sse_none;
        }
        let cost_none_frame = rdcost_dbl(rdmult, 0, sse_frame_none);

        // r = RESTORE_WIENER walk (search_wiener_finish per unit, reference
        // chaining from set_default_wiener).
        let mut bits_frame = 0i64;
        let mut sse_frame = 0i64;
        let mut ref_wiener = WienerInfo::default();
        let mut unit_picks = alloc::vec![RESTORE_NONE; nunits];
        for (idx, u) in units.iter().enumerate() {
            if u.sse_wiener == i64::MAX {
                bits_frame += restore_cost[0];
                sse_frame += u.sse_none;
                continue;
            }
            let cnt = svtav1_entropy::lr::count_wiener_bits(
                wiener_win,
                &u.wiener.vfilter,
                &u.wiener.hfilter,
                &ref_wiener.vfilter,
                &ref_wiener.hfilter,
            ) as i64;
            // AV1_PROB_COST_SHIFT = 9.
            let bits_wiener = restore_cost[1] + (cnt << 9);
            let bits_none = restore_cost[0];
            let cost_none = rdcost_dbl(rdmult, bits_none >> 4, u.sse_none);
            let cost_wiener = rdcost_dbl(rdmult, bits_wiener >> 4, u.sse_wiener);
            if cost_wiener < cost_none {
                unit_picks[idx] = RESTORE_WIENER;
                bits_frame += bits_wiener;
                sse_frame += u.sse_wiener;
                ref_wiener = u.wiener;
            } else {
                unit_picks[idx] = RESTORE_NONE;
                bits_frame += bits_none;
                sse_frame += u.sse_none;
            }
        }
        let cost_wiener_frame = rdcost_dbl(rdmult, bits_frame >> 4, sse_frame);

        // rest_finish_search argmin: NONE first, strict <.
        let frame_rtype = if cost_wiener_frame < cost_none_frame {
            RESTORE_WIENER
        } else {
            RESTORE_NONE
        };

        let mut out_units = alloc::vec::Vec::with_capacity(nunits);
        for (idx, u) in units.iter().enumerate() {
            if frame_rtype == RESTORE_WIENER {
                // copy_unit_info: unit rtype = the per-unit pick.
                out_units.push(RestUnit {
                    rtype: unit_picks[idx],
                    wiener: u.wiener,
                });
            } else {
                out_units.push(RestUnit {
                    rtype: RESTORE_NONE,
                    wiener: u.wiener,
                });
            }
        }
        planes.push(PlaneRest {
            frame_rtype,
            unit_size,
            hunits,
            vunits,
            units: out_units,
        });
    }

    FrameRestInfo { planes }
}

/// Build the stripe-boundary line buffers exactly like the C pipeline:
/// after-deblock (pre-CDEF) pass + after-CDEF pass per plane.
/// `pre_cdef_*` = post-deblock planes, `post_cdef_*` = final CDEF'd planes.
#[allow(clippy::too_many_arguments)]
pub fn save_lr_boundaries(
    pre_y: &[u8],
    pre_u: &[u8],
    pre_v: &[u8],
    post_y: &[u8],
    post_u: &[u8],
    post_v: &[u8],
    w: usize,
    h: usize,
    has_chroma: bool,
) -> alloc::vec::Vec<StripeBoundaries> {
    let mut out = alloc::vec::Vec::new();
    for plane in 0..3usize {
        let is_uv = plane > 0;
        let ss = i32::from(is_uv);
        let (pw, ph) = if is_uv { (w / 2, h / 2) } else { (w, h) };
        let mut bnd = alloc_stripe_boundaries(w as i32, h as i32, ss);
        if is_uv && !has_chroma {
            out.push(bnd);
            continue;
        }
        let (pre, post) = match plane {
            0 => (pre_y, post_y),
            1 => (pre_u, post_u),
            _ => (pre_v, post_v),
        };
        save_tile_row_boundary_lines(pre, 0, pw, pw as i32, ph as i32, ss, false, &mut bnd);
        save_tile_row_boundary_lines(post, 0, pw, pw as i32, ph as i32, ss, true, &mut bnd);
        out.push(bnd);
    }
    out
}

/// C `svt_av1_loop_restoration_filter_frame` (restoration.c:1154): apply
/// the signaled restoration to the final recon planes in place (the output
/// copy — prediction sources are untouched by the caller's contract).
#[allow(clippy::too_many_arguments)]
pub fn apply_restoration_frame(
    recon_y: &mut [u8],
    recon_u: &mut [u8],
    recon_v: &mut [u8],
    w: usize,
    h: usize,
    has_chroma: bool,
    info: &FrameRestInfo,
    boundaries: &[StripeBoundaries],
) {
    for plane in 0..3usize {
        let pr = &info.planes[plane];
        if pr.frame_rtype == RESTORE_NONE {
            continue;
        }
        let is_uv = plane > 0;
        if is_uv && !has_chroma {
            continue;
        }
        let ss = i32::from(is_uv);
        let (pw, ph) = if is_uv { (w / 2, h / 2) } else { (w, h) };
        let recon: &mut [u8] = match plane {
            0 => recon_y,
            1 => recon_u,
            _ => recon_v,
        };
        let mut data = PaddedPlane::from_tight(recon, pw, ph);
        extend_frame(&mut data.data, data.origin, pw, ph, data.stride, 3, 3);
        let mut dst = PaddedPlane::empty(pw, ph);
        let rect = plane_rect(pw as i32, ph as i32);
        foreach_rest_unit_in_tile(&rect, pr.hunits, pr.unit_size, ss, |limits, unit_idx| {
            let u = &pr.units[unit_idx as usize];
            loop_restoration_filter_unit(
                true,
                limits,
                u.rtype,
                &u.wiener,
                &boundaries[plane],
                &rect,
                0, // tile_stripe0 (single tile row)
                ss,
                ss,
                &mut data.data,
                data.origin,
                data.stride,
                &mut dst.data,
                dst.origin,
                dst.stride,
            );
        });
        dst.copy_crop_to(recon);
    }
}

/// C `svt_av1_loop_restoration_corners_in_sb` (restoration.c:1410) —
/// which restoration units have their top-left corner inside this
/// superblock (no superres, single tile). Returns `(rcol0, rcol1, rrow0,
/// rrow1)` when non-empty. `mi_*` are 4x4 luma units; `sb_mi` the SB span
/// in mi (16 for 64px SBs).
pub fn corners_in_sb(
    pr: &PlaneRest,
    is_uv: bool,
    mi_row: i32,
    mi_col: i32,
    sb_mi: i32,
    frame_w: usize,
    frame_h: usize,
) -> Option<(i32, i32, i32, i32)> {
    if pr.frame_rtype == RESTORE_NONE {
        return None;
    }
    let ss = i32::from(is_uv);
    let tile_w = (frame_w as i32 + ss) >> ss;
    let tile_h = (frame_h as i32 + ss) >> ss;
    let size = pr.unit_size;
    let horz_units = svtav1_dsp::restoration::count_units_in_tile(size, tile_w);
    let vert_units = svtav1_dsp::restoration::count_units_in_tile(size, tile_h);
    // MI_SIZE = 4 luma px; one mi spans 4 >> ss plane px.
    let mi_size_x = 4 >> ss;
    let mi_size_y = 4 >> ss;
    let rnd = size - 1;
    let rcol0 = (mi_col * mi_size_x + rnd) / size;
    let rrow0 = (mi_row * mi_size_y + rnd) / size;
    let rcol1 = (((mi_col + sb_mi) * mi_size_x + rnd) / size).min(horz_units);
    let rrow1 = (((mi_row + sb_mi) * mi_size_y + rnd) / size).min(vert_units);
    (rcol0 < rcol1 && rrow0 < rrow1).then_some((rcol0, rcol1, rrow0, rrow1))
}

/// Per-tile LR reference state for the entropy walk — C
/// `EntropyCodingContext.wiener_info[3]`, reset to the default filter at
/// the first SB of each tile (`svt_av1_reset_loop_restoration`,
/// ec_process.c:199; decoder mirror `av1_reset_loop_restoration`).
#[derive(Clone, Debug)]
pub struct LrWalkRefs {
    pub wiener: [WienerInfo; 3],
}

impl Default for LrWalkRefs {
    fn default() -> Self {
        LrWalkRefs {
            wiener: [WienerInfo::default(); 3],
        }
    }
}

/// C `loop_restoration_write_sb_coeffs` over every RU cornered in this SB
/// (the write_modes_sb plane/unit loop, entropy_coding.c:5500-5521):
/// for a RESTORE_WIENER frame type, one `wiener_restore` flag per RU plus
/// the taps when set. The WRITER's window is plane-based (7-tap luma,
/// 5-tap chroma — entropy_coding.c:4160) even when the search solved 5-tap
/// luma: TAP0 is then coded as 0.
#[allow(clippy::too_many_arguments)]
pub fn write_lr_for_sb(
    w: &mut svtav1_entropy::writer::AomWriter,
    fc: &mut svtav1_entropy::context::FrameContext,
    info: &FrameRestInfo,
    refs: &mut LrWalkRefs,
    mi_row: i32,
    mi_col: i32,
    sb_mi: i32,
    frame_w: usize,
    frame_h: usize,
    monochrome: bool,
) {
    let num_planes = if monochrome { 1 } else { 3 };
    for plane in 0..num_planes {
        let pr = &info.planes[plane];
        let Some((rcol0, rcol1, rrow0, rrow1)) =
            corners_in_sb(pr, plane > 0, mi_row, mi_col, sb_mi, frame_w, frame_h)
        else {
            continue;
        };
        debug_assert_eq!(
            pr.frame_rtype, RESTORE_WIENER,
            "only WIENER frame types are searched/signaled (sg_filter_lvl = 0)"
        );
        for rrow in rrow0..rrow1 {
            for rcol in rcol0..rcol1 {
                let runit = (rcol + rrow * pr.hunits) as usize;
                let u = &pr.units[runit];
                let used = u.rtype != RESTORE_NONE;
                w.write_symbol(usize::from(used), &mut fc.wiener_restore_cdf, 2);
                if used {
                    let win = if plane > 0 { WIENER_WIN_CHROMA } else { WIENER_WIN };
                    let r = &mut refs.wiener[plane];
                    svtav1_entropy::lr::write_wiener_filter(
                        w,
                        win,
                        &u.wiener.vfilter,
                        &u.wiener.hfilter,
                        &mut r.vfilter,
                        &mut r.hfilter,
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// wiener_restore flag costs from the default CDF: the instrumented
    /// captures show bits_none = 768 and bits_wn - (count << 9) = 320 on
    /// every cell.
    #[test]
    fn restore_costs_match_instrumented_c() {
        assert_eq!(wiener_restore_cost(), [768, 320]);
    }

    /// RDCOST_DBL against captured values: g64 q40 unit RD —
    /// cost_none 26642064.625 (bits 768, sse 207986, rdmult 211804) and
    /// cost_wn 26499258.34375 (bits 11072, sse 204789).
    #[test]
    fn rdcost_dbl_matches_instrumented_c() {
        assert_eq!(rdcost_dbl(211804, 768 >> 4, 207986), 26642064.625);
        assert_eq!(rdcost_dbl(211804, 11072 >> 4, 204789), 26499258.34375);
        // g64 q55: NONE wins at the unit level.
        assert_eq!(rdcost_dbl(1303771, 768 >> 4, 671191), 86034676.53125);
        assert_eq!(rdcost_dbl(1303771, 13120 >> 4, 670249), 87879942.7421875);
    }

    /// M6 controls: presets 4..=6 -> level 4 (no refinement), <=3 -> level
    /// 3 (refinement, one step), >=7 disabled.
    #[test]
    fn allintra_ctrls_match_c() {
        let c6 = wn_filter_ctrls_allintra(6);
        assert!(c6.enabled && c6.use_chroma && c6.filter_tap_lvl == 2 && !c6.use_refinement);
        let c3 = wn_filter_ctrls_allintra(3);
        assert!(c3.enabled && c3.use_refinement && c3.max_one_refinement_step);
        assert!(!wn_filter_ctrls_allintra(7).enabled);
        assert!(!wn_filter_ctrls_allintra(13).enabled);
    }
}
