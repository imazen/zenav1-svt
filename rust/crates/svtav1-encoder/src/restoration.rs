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
    PixelRect, RESTORATION_UNITSIZE_MAX, RESTORE_NONE, RESTORE_SGRPROJ, RESTORE_SWITCHABLE,
    RESTORE_WIENER, RestUnitParams, StripeBoundaries, StripeBoundariesT, TileLimits,
    WIENER_FILT_TAP0_MAXV, WIENER_FILT_TAP0_MINV, WIENER_FILT_TAP1_MAXV, WIENER_FILT_TAP1_MINV,
    WIENER_FILT_TAP2_MAXV, WIENER_FILT_TAP2_MINV, WIENER_WIN, WIENER_WIN_CHROMA, WienerInfo,
    alloc_stripe_boundaries_t, compute_score, compute_stats, extend_frame, finalize_sym_filter,
    foreach_rest_unit_in_tile, loop_restoration_filter_unit, loop_restoration_filter_unit_hbd,
    save_tile_row_boundary_lines, sse_region, wiener_decompose_sep_sym,
};

/// `SVTAV1_LR_DBG` per-unit/per-step search dump (mirrors the sibling-C
/// `SVT_LR_OUT` instrument format: LRNONE/LRWNSOLVE/LRWNSCORE/LRWNSEG/LRSTEP
/// lines to stderr). Off = zero cost (a OnceLock bool).
#[cfg(feature = "std")]
fn lr_dbg_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("SVTAV1_LR_DBG").is_some())
}
#[cfg(not(feature = "std"))]
fn lr_dbg_on() -> bool {
    false
}
macro_rules! lr_dbg {
    ($($t:tt)*) => {
        #[cfg(feature = "std")]
        if lr_dbg_on() {
            eprintln!($($t)*);
        }
    };
}

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

impl From<crate::port_lr_level::WnFilterCtrlsFull> for WnFilterCtrls {
    /// The VIDEO arm's `svt_aom_set_wn_filter_ctrls` result, narrowed to the
    /// five fields the search consumes.
    ///
    /// `use_prev_frame_coeffs` is dropped because it is set by LEVEL 6 alone,
    /// and no level function this port can reach returns 6 (`_default` gives
    /// 4 or 5, `_allintra` 3 or 4, `_rtc` 0) — the assert below is the
    /// positive control for that claim rather than a comment asserting it.
    fn from(full: crate::port_lr_level::WnFilterCtrlsFull) -> Self {
        debug_assert!(
            !full.use_prev_frame_coeffs,
            "wn level 6 (use_prev_frame_coeffs) is unreachable from every ported ladder"
        );
        WnFilterCtrls {
            enabled: full.enabled,
            use_chroma: full.use_chroma,
            filter_tap_lvl: full.filter_tap_lvl,
            use_refinement: full.use_refinement,
            max_one_refinement_step: full.max_one_refinement_step,
        }
    }
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

/// Per-restoration-unit outcome (C `RestorationUnitInfo` + the search's
/// per-unit choice).
#[derive(Clone, Copy, Debug)]
pub struct RestUnit {
    pub rtype: u8,
    pub wiener: WienerInfo,
    /// The unit's SGR parameters. Meaningful only when `rtype ==
    /// RESTORE_SGRPROJ`; C carries `wiener_info` and `sgrproj_info` in one
    /// union-free struct the same way (`copy_unit_info`,
    /// restoration_pick.c:1220, writes exactly one of them per unit).
    pub sgrproj: crate::port_sgr_search::SgrprojInfo,
}

impl RestUnit {
    /// A NONE unit with both payloads at the C defaults.
    fn none() -> Self {
        RestUnit {
            rtype: RESTORE_NONE,
            wiener: WienerInfo::default(),
            sgrproj: crate::port_sgr_search::SgrprojInfo::c_default(),
        }
    }

    /// This unit as the `RestorationUnitInfo` the DSP filter dispatches on.
    fn params(&self) -> RestUnitParams {
        RestUnitParams {
            rtype: self.rtype,
            wiener: self.wiener,
            sgr_ep: self.sgrproj.ep as usize,
            sgr_xqd: self.sgrproj.xqd,
        }
    }
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
            units: alloc::vec![RestUnit::none(); (hunits * vunits) as usize],
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
pub struct PaddedPlaneT<T> {
    pub data: alloc::vec::Vec<T>,
    pub stride: usize,
    pub origin: usize,
    pub w: usize,
    pub h: usize,
}

/// The 8-bit plane (unchanged name for every existing caller).
pub type PaddedPlane = PaddedPlaneT<u8>;

pub const PLANE_BORDER: usize = 4;

impl<T: Copy + Default> PaddedPlaneT<T> {
    /// Copy a tight `w x h` plane into padded storage (borders zero until
    /// `extend()` replicates them).
    pub fn from_tight(src: &[T], w: usize, h: usize) -> Self {
        Self::from_strided(src, w, w, h)
    }

    /// Copy the top-left `w x h` window of a plane stored at `src_stride`
    /// into padded storage. The recon canvases are sized on the ALIGNED
    /// (mi-grid) dims while loop restoration works on the TRUE frame extent
    /// (C `whole_frame_rect` reads `frm_size`, which pcs.c:1337 sets to
    /// `picture_width - non_m8_pad_w`), so the two differ by up to 7 px and
    /// the window has to be taken at the canvas stride.
    pub fn from_strided(src: &[T], src_stride: usize, w: usize, h: usize) -> Self {
        let stride = w + 2 * PLANE_BORDER;
        let mut data = alloc::vec![T::default(); stride * (h + 2 * PLANE_BORDER)];
        let origin = PLANE_BORDER * stride + PLANE_BORDER;
        for y in 0..h {
            data[origin + y * stride..origin + y * stride + w]
                .copy_from_slice(&src[y * src_stride..y * src_stride + w]);
        }
        PaddedPlaneT {
            data,
            stride,
            origin,
            w,
            h,
        }
    }

    fn empty(w: usize, h: usize) -> Self {
        let stride = w + 2 * PLANE_BORDER;
        PaddedPlaneT {
            data: alloc::vec![T::default(); stride * (h + 2 * PLANE_BORDER)],
            stride,
            origin: PLANE_BORDER * stride + PLANE_BORDER,
            w,
            h,
        }
    }

    /// Copy the crop back into the top-left `w x h` window of a buffer
    /// stored at `dst_stride`. Columns/rows of `dst` outside the window are
    /// left untouched — C filters only the `whole_frame_rect` extent too, so
    /// the aligned padding keeps its post-CDEF content.
    fn copy_crop_to_strided(&self, dst: &mut [T], dst_stride: usize) {
        for y in 0..self.h {
            dst[y * dst_stride..y * dst_stride + self.w].copy_from_slice(
                &self.data[self.origin + y * self.stride..self.origin + y * self.stride + self.w],
            );
        }
    }
}

/// The four bit-depth-dependent kernels the Wiener SEARCH calls. C selects
/// between two whole families on `cm->use_highbitdepth` (restoration_pick.c:
/// 1243) while the surrounding decision logic — the per-unit iteration, the
/// tap solve, the refinement hill-climb, the per-unit and frame-level RD — is
/// one body shared by both. This trait keeps that split: exactly the kernels
/// are per-depth, the logic below is written once.
pub trait LrPixel: Copy + Default {
    /// `sse_restoration_unit` (restoration_pick.c:48) at this depth.
    #[allow(clippy::too_many_arguments)]
    fn sse_region(
        a: &[Self],
        a_origin: usize,
        a_stride: usize,
        b: &[Self],
        b_origin: usize,
        b_stride: usize,
        width: usize,
        height: usize,
    ) -> i64;

    /// `svt_av1_compute_stats{,_highbd}` (restoration_pick.c:652 / :692).
    #[allow(clippy::too_many_arguments)]
    fn compute_stats(
        wiener_win: usize,
        dgd: &[Self],
        dgd_origin: usize,
        dgd_stride: usize,
        src: &[Self],
        src_origin: usize,
        src_stride: usize,
        h_start: i32,
        h_end: i32,
        v_start: i32,
        v_end: i32,
        m: &mut [i64],
        h: &mut [i64],
        bit_depth: u8,
    );

    /// `svt_av1_loop_restoration_filter_unit` at `need_boundaries = 0` —
    /// the search arm (`use_boundaries_in_rest_search = 0`).
    #[allow(clippy::too_many_arguments)]
    fn filter_unit_search(
        limits: &TileLimits,
        rui: &RestUnitParams,
        rect: &PixelRect,
        ss: i32,
        data: &mut [Self],
        data_origin: usize,
        stride: usize,
        dst: &mut [Self],
        dst_origin: usize,
        dst_stride: usize,
        bit_depth: u8,
    );

    /// `svt_av1_loop_restoration_filter_unit` at `need_boundaries = 1` — the
    /// decoder-exact APPLY arm (`svt_av1_loop_restoration_filter_frame`,
    /// restoration.c:1154, `highbd = cm->use_highbitdepth`). Issue #13: the
    /// u16 instantiation is what lets the 10-bit canvas receive the filter
    /// the 10-bit search signalled.
    #[allow(clippy::too_many_arguments)]
    fn filter_unit_apply(
        limits: &TileLimits,
        rui: &RestUnitParams,
        rsb: &StripeBoundariesT<Self>,
        rect: &PixelRect,
        ss: i32,
        data: &mut [Self],
        data_origin: usize,
        stride: usize,
        dst: &mut [Self],
        dst_origin: usize,
        dst_stride: usize,
        bit_depth: u8,
    );

    /// `search_sgrproj_seg`'s sweep half (restoration_pick.c:1237) — the
    /// per-unit `(ep, xqd)` pick. The bit-depth split is the same one C makes
    /// with `highbd`: it selects which `pixel_proj_error` kernel runs.
    /// 4:2:0 is the port's only chroma format, so `subsampling_{x,y}` are
    /// both 1 for a chroma plane.
    #[allow(clippy::too_many_arguments)]
    fn sgr_search_unit(
        dgd: &[Self],
        dgd_origin: usize,
        width: i32,
        height: i32,
        dgd_stride: usize,
        src: &[Self],
        src_origin: usize,
        src_stride: usize,
        bit_depth: u8,
        plane: usize,
        ctrls: &crate::port_lr_level::SgFilterCtrls,
    ) -> crate::port_sgr_search::SgrprojInfo;
}

impl LrPixel for u8 {
    fn sse_region(
        a: &[u8],
        a_origin: usize,
        a_stride: usize,
        b: &[u8],
        b_origin: usize,
        b_stride: usize,
        width: usize,
        height: usize,
    ) -> i64 {
        sse_region(a, a_origin, a_stride, b, b_origin, b_stride, width, height)
    }

    fn compute_stats(
        wiener_win: usize,
        dgd: &[u8],
        dgd_origin: usize,
        dgd_stride: usize,
        src: &[u8],
        src_origin: usize,
        src_stride: usize,
        h_start: i32,
        h_end: i32,
        v_start: i32,
        v_end: i32,
        m: &mut [i64],
        h: &mut [i64],
        _bit_depth: u8,
    ) {
        compute_stats(
            wiener_win, dgd, dgd_origin, dgd_stride, src, src_origin, src_stride, h_start, h_end,
            v_start, v_end, m, h,
        );
    }

    fn filter_unit_search(
        limits: &TileLimits,
        rui: &RestUnitParams,
        rect: &PixelRect,
        ss: i32,
        data: &mut [u8],
        data_origin: usize,
        stride: usize,
        dst: &mut [u8],
        dst_origin: usize,
        dst_stride: usize,
        _bit_depth: u8,
    ) {
        // `need_boundaries = false` -> the stripe-boundary save/restore never
        // runs, so the (empty) buffers are never read and `data` is not
        // modified. Byte-identical to the previous direct call.
        let empty_bounds = StripeBoundaries::default();
        loop_restoration_filter_unit(
            false,
            limits,
            rui,
            &empty_bounds,
            rect,
            0,
            ss,
            ss,
            data,
            data_origin,
            stride,
            dst,
            dst_origin,
            dst_stride,
        );
    }

    fn filter_unit_apply(
        limits: &TileLimits,
        rui: &RestUnitParams,
        rsb: &StripeBoundariesT<u8>,
        rect: &PixelRect,
        ss: i32,
        data: &mut [u8],
        data_origin: usize,
        stride: usize,
        dst: &mut [u8],
        dst_origin: usize,
        dst_stride: usize,
        _bit_depth: u8,
    ) {
        loop_restoration_filter_unit(
            true,
            limits,
            rui,
            rsb,
            rect,
            0, // tile_stripe0 (single tile row)
            ss,
            ss,
            data,
            data_origin,
            stride,
            dst,
            dst_origin,
            dst_stride,
        );
    }

    fn sgr_search_unit(
        dgd: &[u8],
        dgd_origin: usize,
        width: i32,
        height: i32,
        dgd_stride: usize,
        src: &[u8],
        src_origin: usize,
        src_stride: usize,
        bit_depth: u8,
        plane: usize,
        ctrls: &crate::port_lr_level::SgFilterCtrls,
    ) -> crate::port_sgr_search::SgrprojInfo {
        crate::port_sgr_search::search_sgrproj_unit(
            svtav1_dsp::port_sgr::SgrSrc::Lowbd(dgd),
            dgd_origin,
            width,
            height,
            dgd_stride,
            &crate::port_sgr_search::ProjPlanes::Lowbd { src, dat: dgd },
            src_origin,
            src_stride,
            i32::from(bit_depth),
            plane,
            true,
            true,
            ctrls,
        )
    }
}

impl LrPixel for u16 {
    fn sse_region(
        a: &[u16],
        a_origin: usize,
        a_stride: usize,
        b: &[u16],
        b_origin: usize,
        b_stride: usize,
        width: usize,
        height: usize,
    ) -> i64 {
        svtav1_dsp::restoration::sse_region_hbd(
            a, a_origin, a_stride, b, b_origin, b_stride, width, height,
        )
    }

    fn compute_stats(
        wiener_win: usize,
        dgd: &[u16],
        dgd_origin: usize,
        dgd_stride: usize,
        src: &[u16],
        src_origin: usize,
        src_stride: usize,
        h_start: i32,
        h_end: i32,
        v_start: i32,
        v_end: i32,
        m: &mut [i64],
        h: &mut [i64],
        bit_depth: u8,
    ) {
        svtav1_dsp::restoration::compute_stats_hbd(
            wiener_win, dgd, dgd_origin, dgd_stride, src, src_origin, src_stride, h_start, h_end,
            v_start, v_end, m, h, bit_depth,
        );
    }

    fn filter_unit_search(
        limits: &TileLimits,
        rui: &RestUnitParams,
        rect: &PixelRect,
        ss: i32,
        data: &mut [u16],
        data_origin: usize,
        stride: usize,
        dst: &mut [u16],
        dst_origin: usize,
        dst_stride: usize,
        bit_depth: u8,
    ) {
        svtav1_dsp::restoration::loop_restoration_filter_unit_search_hbd(
            limits,
            rui,
            rect,
            0,
            ss,
            ss,
            data,
            data_origin,
            stride,
            dst,
            dst_origin,
            dst_stride,
            bit_depth as i32,
        );
    }

    fn filter_unit_apply(
        limits: &TileLimits,
        rui: &RestUnitParams,
        rsb: &StripeBoundariesT<u16>,
        rect: &PixelRect,
        ss: i32,
        data: &mut [u16],
        data_origin: usize,
        stride: usize,
        dst: &mut [u16],
        dst_origin: usize,
        dst_stride: usize,
        bit_depth: u8,
    ) {
        loop_restoration_filter_unit_hbd(
            true,
            limits,
            rui,
            rsb,
            rect,
            0, // tile_stripe0 (single tile row)
            ss,
            ss,
            data,
            data_origin,
            stride,
            dst,
            dst_origin,
            dst_stride,
            bit_depth as i32,
        );
    }

    fn sgr_search_unit(
        dgd: &[u16],
        dgd_origin: usize,
        width: i32,
        height: i32,
        dgd_stride: usize,
        src: &[u16],
        src_origin: usize,
        src_stride: usize,
        bit_depth: u8,
        plane: usize,
        ctrls: &crate::port_lr_level::SgFilterCtrls,
    ) -> crate::port_sgr_search::SgrprojInfo {
        crate::port_sgr_search::search_sgrproj_unit(
            svtav1_dsp::port_sgr::SgrSrc::Highbd(dgd),
            dgd_origin,
            width,
            height,
            dgd_stride,
            &crate::port_sgr_search::ProjPlanes::Highbd { src, dat: dgd },
            src_origin,
            src_stride,
            i32::from(bit_depth),
            plane,
            true,
            true,
            ctrls,
        )
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
    let icdf0 = crate::entropy::context::FrameContext::new_default().wiener_restore_cdf[0] as u32;
    let p0 = 32768 - icdf0;
    let p1 = icdf0;
    [
        crate::entropy::context::av1_cost_symbol(p0) as i64,
        crate::entropy::context::av1_cost_symbol(p1) as i64,
    ]
}

/// `sgrproj_restore` flag costs from the default CDF (AOM_CDF2(16855)) —
/// C `svt_aom_get_syntax_rate_from_cdf(sgrproj_restore_fac_bits,
/// fc->sgrproj_restore_cdf, NULL)`, the SGR twin of
/// [`wiener_restore_cost`].
fn sgrproj_restore_cost() -> [i64; 2] {
    let icdf0 = crate::entropy::context::FrameContext::new_default().sgrproj_restore_cdf[0] as u32;
    [
        crate::entropy::context::av1_cost_symbol(32768 - icdf0) as i64,
        crate::entropy::context::av1_cost_symbol(icdf0) as i64,
    ]
}

/// `switchable_restore` symbol costs from the default CDF
/// (AOM_CDF3(9413, 22581)) — C `x->switchable_restore_cost[3]`.
fn switchable_restore_cost() -> [i64; 3] {
    let cdf = crate::entropy::context::FrameContext::new_default().switchable_restore_cdf;
    // ICDF storage: cdf[i] = 32768 - CDF_i. p(sym i) = cdf[i-1] - cdf[i]
    // with cdf[-1] = 32768.
    let p0 = 32768 - cdf[0] as u32;
    let p1 = (cdf[0] - cdf[1]) as u32;
    let p2 = cdf[1] as u32;
    [
        crate::entropy::context::av1_cost_symbol(p0) as i64,
        crate::entropy::context::av1_cost_symbol(p1) as i64,
        crate::entropy::context::av1_cost_symbol(p2) as i64,
    ]
}

/// C `RDCOST_DBL` (restoration.h:344): rate in 1/512-bit units (already
/// `>> 4`-ed by the callers), double math.
fn rdcost_dbl(rdmult: i64, rate: i64, dist: i64) -> f64 {
    (rate as f64 * rdmult as f64) / (1u32 << 9) as f64 + dist as f64 * (1u32 << 7) as f64
}

/// One plane's per-unit search results (C `RestUnitSearchInfo` slice).
struct UnitSearch {
    /// C `rusi->sse[]`, indexed by `RESTORE_NONE`/`_WIENER`/`_SGRPROJ`.
    /// `i64::MAX` in the WIENER slot is the `compute_score > 0` revert (C's
    /// `INT64_MAX` sentinel); the SGRPROJ slot keeps `i64::MAX` when the SGR
    /// walk did not run for this plane, and that unit is then never admitted
    /// (the sgrproj/switchable frame walks are gated on the same predicate C
    /// gates its `foreach_rest_unit_in_frame_seg` call on).
    sse: [i64; 3],
    wiener: WienerInfo,
    sgrproj: crate::port_sgr_search::SgrprojInfo,
}

/// C `AV1_PROB_COST_SHIFT` (md_rate_estimation.h:29).
const AV1_PROB_COST_SHIFT: i32 = 9;
/// C `RESTORE_TYPES` (definitions.h) — the four frame restoration types.
const RESTORE_TYPES: u8 = 4;

/// C `try_restoration_unit_seg` (restoration_pick.c:123) at
/// `use_boundaries_in_rest_search = 0`: filter the unit (no stripe-boundary
/// overwrites) from the extended dgd into the trial buffer, then SSE vs the
/// source over the unit rect.
#[allow(clippy::too_many_arguments)]
fn try_restoration_unit<P: LrPixel>(
    dgd: &mut PaddedPlaneT<P>,
    trial: &mut PaddedPlaneT<P>,
    src: &[P],
    src_stride: usize,
    limits: &TileLimits,
    rect: &PixelRect,
    ss: i32,
    rui: &RestUnitParams,
    bit_depth: u8,
) -> i64 {
    P::filter_unit_search(
        limits,
        rui,
        rect,
        ss,
        &mut dgd.data,
        dgd.origin,
        dgd.stride,
        &mut trial.data,
        trial.origin,
        trial.stride,
        bit_depth,
    );
    P::sse_region(
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
fn finer_tile_search_wiener<P: LrPixel>(
    ctrls: &WnFilterCtrls,
    dgd: &mut PaddedPlaneT<P>,
    trial: &mut PaddedPlaneT<P>,
    src: &[P],
    src_stride: usize,
    limits: &TileLimits,
    rect: &PixelRect,
    ss: i32,
    wiener: &mut WienerInfo,
    wiener_win: usize,
    bit_depth: u8,
) -> i64 {
    let plane_off = (WIENER_WIN - wiener_win) >> 1;
    let mut err = try_restoration_unit(
        dgd,
        trial,
        src,
        src_stride,
        limits,
        rect,
        ss,
        &RestUnitParams::wiener(*wiener),
        bit_depth,
    );
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
                            dgd,
                            trial,
                            src,
                            src_stride,
                            limits,
                            rect,
                            ss,
                            &RestUnitParams::wiener(*wiener),
                            bit_depth,
                        );
                        lr_dbg!(
                            "LRSTEP f={} d=- p={p} s={s} err2={err2} err={err} acc={}",
                            if pass == 0 { 'h' } else { 'v' },
                            i32::from(err2 <= err)
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
                            dgd,
                            trial,
                            src,
                            src_stride,
                            limits,
                            rect,
                            ss,
                            &RestUnitParams::wiener(*wiener),
                            bit_depth,
                        );
                        lr_dbg!(
                            "LRSTEP f={} d=+ p={p} s={s} err2={err2} err={err} acc={}",
                            if pass == 0 { 'h' } else { 'v' },
                            i32::from(err2 <= err)
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
) -> crate::EncodeResult<FrameRestInfo> {
    search_restoration_still_bd(
        ctrls,
        &crate::port_lr_level::SgFilterCtrls::default(),
        src_y,
        src_u,
        src_v,
        recon_y,
        recon_u,
        recon_v,
        w,
        h,
        has_chroma,
        rdmult,
        8,
    )
}

/// [`search_restoration_still`] at an explicit bit depth. C runs ONE
/// `restoration_seg_search` body and picks the kernel family per
/// `cm->use_highbitdepth` (restoration_pick.c:1243); the same split here —
/// the decision logic is this single generic body, only the four kernels in
/// [`LrPixel`] differ. `bit_depth` reaches `compute_stats` (the
/// `bit_depth_divider`) and the unit filter (`clip_pixel_highbd`); it is
/// inert on the u8 instantiation.
///
/// `rdmult` is C's `x->rdmult` = `pic_full_lambda[bit_depth == EB_TEN_BIT ?
/// EB_10_BIT_MD : EB_8_BIT_MD]` (enc_dec_process.c:3246) — the CALLER's
/// responsibility to pass at the matching depth
/// (`pd0::kf_full_lambda_bd10_pic` at bd10).
#[allow(clippy::too_many_arguments)]
pub fn search_restoration_still_bd<P: LrPixel>(
    wn_ctrls: &WnFilterCtrls,
    sg_ctrls: &crate::port_lr_level::SgFilterCtrls,
    src_y: &[P],
    src_u: &[P],
    src_v: &[P],
    recon_y: &[P],
    recon_u: &[P],
    recon_v: &[P],
    w: usize,
    h: usize,
    has_chroma: bool,
    rdmult: i64,
    bit_depth: u8,
) -> crate::EncodeResult<FrameRestInfo> {
    debug_assert!(wn_ctrls.enabled || sg_ctrls.enabled);
    let wn_luma = if wn_ctrls.filter_tap_lvl == 1 {
        WIENER_WIN
    } else {
        WIENER_WIN_CHROMA
    };
    let wiener_restore_cost = wiener_restore_cost();
    let sgrproj_restore_cost = sgrproj_restore_cost();
    let switchable_restore_cost = switchable_restore_cost();

    // set_restoration_unit_size (pcs.c:30): 256 for all planes (s = 0).
    let unit_size = RESTORATION_UNITSIZE_MAX;

    // C `plane_end` (restoration_pick.c:1573): PLANE_V iff EITHER filter is
    // enabled with chroma. `has_chroma` is the port's monochrome guard.
    let plane_end = if has_chroma
        && ((wn_ctrls.enabled && wn_ctrls.use_chroma) || (sg_ctrls.enabled && sg_ctrls.use_chroma))
    {
        2
    } else {
        0
    };
    let mut planes = alloc::vec::Vec::new();

    for plane in 0..3usize {
        let is_uv = plane > 0;
        let ss = i32::from(is_uv);
        // C whole_frame_rect (restoration.c:58-59): the plane rect is the
        // TRUE luma dims for Y and ROUND_POWER_OF_TWO (= CEILING (x+1)>>1) for
        // chroma. `w`/`h` here are the TRUE dims (the caller feeds tight
        // true/ceil buffers extracted from the aligned-strided recon so the
        // search touches only the true region + edge replication, exactly as
        // C's extend_frame does — task #95 goal 1, odd true dims). For even
        // (8-aligned) true dims ceiling == floor, so every existing cell is
        // byte-neutral.
        let (pw, ph) = if is_uv {
            (w.div_ceil(2), h.div_ceil(2))
        } else {
            (w, h)
        };
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
        let wiener_win = if plane == 0 {
            wn_luma
        } else {
            WIENER_WIN_CHROMA
        };
        let rect = plane_rect(pw as i32, ph as i32);

        // svt_extend_frame(dgd, ..) with RESTORATION_BORDER+1(+pad16) horz /
        // RESTORATION_BORDER vert — values beyond +-3 never affect results,
        // our PLANE_BORDER=4 covers every touched byte.
        let mut dgd = PaddedPlaneT::<P>::from_tight(recon, pw, ph);
        extend_frame(&mut dgd.data, dgd.origin, pw, ph, dgd.stride, 4, 3);
        let mut trial = PaddedPlaneT::<P>::empty(pw, ph);

        // ---- search phase (`restoration_seg_search`, restoration_pick.c:1474)
        // Per unit: the unfiltered SSE, then the Wiener solve and the SGR
        // sweep, each gated per plane exactly as C's three
        // `foreach_rest_unit_in_frame_seg` calls are.
        let nunits = (hunits * vunits) as usize;
        let mut units: alloc::vec::Vec<UnitSearch> = svtav1_types::try_with_capacity![nunits]?;
        for _ in 0..nunits {
            units.push(UnitSearch {
                sse: [0, i64::MAX, i64::MAX],
                wiener: WienerInfo::default(),
                sgrproj: crate::port_sgr_search::SgrprojInfo::c_default(),
            });
        }

        foreach_rest_unit_in_tile(&rect, hunits, unit_size, ss, |limits, unit_idx| {
            // search_norestore_seg: SSE of the unfiltered recon vs source.
            units[unit_idx as usize].sse[RESTORE_NONE as usize] = P::sse_region(
                src,
                (limits.v_start as usize) * pw + limits.h_start as usize,
                pw,
                recon,
                (limits.v_start as usize) * pw + limits.h_start as usize,
                pw,
                (limits.h_end - limits.h_start) as usize,
                (limits.v_end - limits.v_start) as usize,
            );
            lr_dbg!(
                "LRNONE plane={plane} unit={unit_idx} lim=[{},{},{},{}] sse={}",
                limits.h_start,
                limits.h_end,
                limits.v_start,
                limits.v_end,
                units[unit_idx as usize].sse[RESTORE_NONE as usize]
            );
        });

        // C: `if (cm->wn_filter_ctrls.enabled && (!plane || use_chroma))`.
        if wn_ctrls.enabled && (plane == 0 || wn_ctrls.use_chroma) {
            foreach_rest_unit_in_tile(&rect, hunits, unit_size, ss, |limits, unit_idx| {
                // search_wiener_seg.
                let win2 = wiener_win * wiener_win;
                let mut m = [0i64; WIENER_WIN * WIENER_WIN];
                let mut hh = alloc::vec![0i64; win2 * win2];
                P::compute_stats(
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
                    bit_depth,
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

                #[cfg(feature = "std")]
                if lr_dbg_on() {
                    let msum = m.iter().fold(0u64, |a, &v| a.wrapping_add(v as u64));
                    let hsum = hh.iter().fold(0u64, |a, &v| a.wrapping_add(v as u64));
                    eprintln!(
                        "LRWNSOLVE plane={plane} unit={unit_idx} win={wiener_win} lim=[{},{},{},{}] \
                         M0={} M1={} Msum={msum} Hsum={hsum} vd={:?} hd={:?} v={:?} h={:?}",
                        limits.h_start,
                        limits.h_end,
                        limits.v_start,
                        limits.v_end,
                        m[0],
                        m[1],
                        &vd[..],
                        &hd[..],
                        &wi.vfilter[..7],
                        &wi.hfilter[..7]
                    );
                }
                let score = compute_score(wiener_win, &m, &hh, &wi.vfilter, &wi.hfilter);
                lr_dbg!("LRWNSCORE plane={plane} unit={unit_idx} score={score}");
                if score > 0 {
                    units[unit_idx as usize].sse[RESTORE_WIENER as usize] = i64::MAX;
                    return;
                }
                let sse = finer_tile_search_wiener(
                    wn_ctrls, &mut dgd, &mut trial, src, pw, limits, &rect, ss, &mut wi,
                    wiener_win, bit_depth,
                );
                lr_dbg!(
                    "LRWNSEG plane={plane} unit={unit_idx} sse_wn={sse} v={:?} h={:?}",
                    &wi.vfilter[..7],
                    &wi.hfilter[..7]
                );
                units[unit_idx as usize].sse[RESTORE_WIENER as usize] = sse;
                units[unit_idx as usize].wiener = wi;
            });
        }

        // C: `if (cm->sg_filter_ctrls.enabled && (!plane || use_chroma))` —
        // `search_sgrproj_seg` (restoration_pick.c:1237). Unreachable on the
        // all-intra arm (`sg_filter_lvl = 0` at every representable preset);
        // live in VIDEO mode at presets 0..3.
        if sg_ctrls.enabled && (plane == 0 || sg_ctrls.use_chroma) {
            foreach_rest_unit_in_tile(&rect, hunits, unit_size, ss, |limits, unit_idx| {
                let sgr = P::sgr_search_unit(
                    &dgd.data,
                    dgd.origin + limits.v_start as usize * dgd.stride + limits.h_start as usize,
                    limits.h_end - limits.h_start,
                    limits.v_end - limits.v_start,
                    dgd.stride,
                    src,
                    limits.v_start as usize * pw + limits.h_start as usize,
                    pw,
                    bit_depth,
                    plane,
                    sg_ctrls,
                );
                let sse = try_restoration_unit(
                    &mut dgd,
                    &mut trial,
                    src,
                    pw,
                    limits,
                    &rect,
                    ss,
                    &RestUnitParams::sgrproj(sgr.ep as usize, sgr.xqd),
                    bit_depth,
                );
                lr_dbg!(
                    "LRSGSEG plane={plane} unit={unit_idx} ep={} xqd={:?} sse_sg={sse}",
                    sgr.ep,
                    sgr.xqd
                );
                units[unit_idx as usize].sse[RESTORE_SGRPROJ as usize] = sse;
                units[unit_idx as usize].sgrproj = sgr;
            });
        }

        // ---- finish phase (`rest_finish_search`, restoration_pick.c:1561) ----
        //
        // C runs `search_rest_type_finish` once per candidate frame type in
        // ORDER (NONE, WIENER, SGRPROJ, SWITCHABLE) and takes the argmin with
        // `r == 0 || cost < best_cost` — NONE first, ties to the earlier type.
        // SWITCHABLE reads the per-unit verdicts the WIENER and SGRPROJ walks
        // left behind, which is why the walks must run in that order and why
        // a candidate whose own walk chose NONE is SKIPPED there rather than
        // re-priced (`rusi->best_rtype[r-1] == RESTORE_NONE -> continue`).
        //
        // `force_restore_type_d` (restoration_pick.c:1565): with only one
        // filter enabled, the argmin is restricted to {NONE, that filter}.
        let force_restore_type = match (wn_ctrls.enabled, sg_ctrls.enabled) {
            (true, true) => RESTORE_TYPES,
            (true, false) => RESTORE_WIENER,
            (false, true) => RESTORE_SGRPROJ,
            (false, false) => RESTORE_NONE,
        };
        // `num_rtypes = (plane_ntiles > 1) ? RESTORE_TYPES : RESTORE_SWITCHABLE_TYPES`
        // — a plane with a SINGLE restoration unit never considers
        // RESTORE_SWITCHABLE (there is nothing to switch between).
        let num_rtypes = if nunits > 1 {
            RESTORE_TYPES
        } else {
            RESTORE_SWITCHABLE
        };

        // Per-unit verdict of each finish walk — C `rusi->best_rtype[r - 1]`,
        // zero-initialised (= RESTORE_NONE) for a walk that never ran.
        let mut best_rtype_wiener = alloc::vec![RESTORE_NONE; nunits];
        let mut best_rtype_sgr = alloc::vec![RESTORE_NONE; nunits];

        let mut best_cost = 0.0f64;
        let mut best_rtype = RESTORE_NONE;
        let mut best_picks = alloc::vec![RESTORE_NONE; nunits];

        for r in 0..num_rtypes {
            if force_restore_type != RESTORE_TYPES && r != RESTORE_NONE && r != force_restore_type {
                continue;
            }
            if plane > 0
                && ((r == RESTORE_WIENER && !wn_ctrls.use_chroma)
                    || (r == RESTORE_SGRPROJ && !sg_ctrls.use_chroma))
            {
                continue;
            }

            // `reset_rsc` + `rsc_on_tile`: the accumulators and BOTH filter
            // references restart at the C defaults for every walk.
            let mut bits_frame = 0i64;
            let mut sse_frame = 0i64;
            let mut ref_wiener = WienerInfo::default();
            let mut ref_sgr = crate::port_sgr_search::SgrprojInfo::c_default();
            let mut picks = alloc::vec![RESTORE_NONE; nunits];

            for (idx, u) in units.iter().enumerate() {
                match r {
                    RESTORE_NONE => {
                        // search_norestore_finish: no bits at all.
                        sse_frame += u.sse[RESTORE_NONE as usize];
                    }
                    RESTORE_WIENER => {
                        if u.sse[RESTORE_WIENER as usize] == i64::MAX {
                            bits_frame += wiener_restore_cost[0];
                            sse_frame += u.sse[RESTORE_NONE as usize];
                            best_rtype_wiener[idx] = RESTORE_NONE;
                            continue;
                        }
                        let cnt = crate::entropy::lr::count_wiener_bits(
                            wiener_win,
                            &u.wiener.vfilter,
                            &u.wiener.hfilter,
                            &ref_wiener.vfilter,
                            &ref_wiener.hfilter,
                        ) as i64;
                        let bits_wiener = wiener_restore_cost[1] + (cnt << AV1_PROB_COST_SHIFT);
                        let bits_none = wiener_restore_cost[0];
                        let cost_none =
                            rdcost_dbl(rdmult, bits_none >> 4, u.sse[RESTORE_NONE as usize]);
                        let cost_wiener =
                            rdcost_dbl(rdmult, bits_wiener >> 4, u.sse[RESTORE_WIENER as usize]);
                        if cost_wiener < cost_none {
                            picks[idx] = RESTORE_WIENER;
                            best_rtype_wiener[idx] = RESTORE_WIENER;
                            bits_frame += bits_wiener;
                            sse_frame += u.sse[RESTORE_WIENER as usize];
                            ref_wiener = u.wiener;
                        } else {
                            best_rtype_wiener[idx] = RESTORE_NONE;
                            bits_frame += bits_none;
                            sse_frame += u.sse[RESTORE_NONE as usize];
                        }
                    }
                    RESTORE_SGRPROJ => {
                        let f = crate::port_sgr_search::sgrproj_finish_decision(
                            rdmult,
                            sgrproj_restore_cost,
                            &u.sgrproj,
                            &ref_sgr,
                            u.sse[RESTORE_NONE as usize],
                            u.sse[RESTORE_SGRPROJ as usize],
                        );
                        bits_frame += f.bits;
                        sse_frame += f.sse;
                        if f.chose_sgr {
                            picks[idx] = RESTORE_SGRPROJ;
                            best_rtype_sgr[idx] = RESTORE_SGRPROJ;
                            ref_sgr = u.sgrproj;
                        } else {
                            best_rtype_sgr[idx] = RESTORE_NONE;
                        }
                    }
                    _ => {
                        // search_switchable. NOTE the wiener window here is
                        // plane-based (7-tap luma) regardless of
                        // `filter_tap_lvl` — C's own asymmetry with
                        // search_wiener_finish (docs/SUSPECTED-C-BUGS.md #7).
                        let sw_win = if plane == 0 {
                            WIENER_WIN
                        } else {
                            WIENER_WIN_CHROMA
                        };
                        let coeff_pcost_wiener = crate::entropy::lr::count_wiener_bits(
                            sw_win,
                            &u.wiener.vfilter,
                            &u.wiener.hfilter,
                            &ref_wiener.vfilter,
                            &ref_wiener.hfilter,
                        );
                        let coeff_pcost_sgr =
                            crate::port_sgr_search::count_sgrproj_bits(&u.sgrproj, &ref_sgr);
                        let choice = crate::port_sgr_search::switchable_decision(
                            rdmult,
                            switchable_restore_cost,
                            [
                                u.sse[RESTORE_NONE as usize],
                                u.sse[RESTORE_WIENER as usize],
                                u.sse[RESTORE_SGRPROJ as usize],
                            ],
                            best_rtype_wiener[idx] == RESTORE_WIENER,
                            coeff_pcost_wiener,
                            best_rtype_sgr[idx] == RESTORE_SGRPROJ,
                            coeff_pcost_sgr,
                        );
                        picks[idx] = choice.best_rtype as u8;
                        bits_frame += choice.bits;
                        sse_frame += choice.sse;
                        if choice.best_rtype == crate::port_sgr_search::RESTORE_WIENER {
                            ref_wiener = u.wiener;
                        }
                        if choice.best_rtype == crate::port_sgr_search::RESTORE_SGRPROJ {
                            ref_sgr = u.sgrproj;
                        }
                    }
                }
            }

            let cost = rdcost_dbl(rdmult, bits_frame >> 4, sse_frame);
            lr_dbg!("LRFINISH plane={plane} r={r} bits={bits_frame} sse={sse_frame} cost={cost}");
            if r == RESTORE_NONE || cost < best_cost {
                best_cost = cost;
                best_rtype = r;
                best_picks = picks;
            }
        }

        let frame_rtype = best_rtype;
        let mut out_units: alloc::vec::Vec<RestUnit> = svtav1_types::try_with_capacity![nunits]?;
        for (idx, u) in units.iter().enumerate() {
            // copy_unit_info (restoration_pick.c:1220): only when the frame
            // type is not NONE does the unit carry a per-unit type.
            out_units.push(RestUnit {
                rtype: if frame_rtype == RESTORE_NONE {
                    RESTORE_NONE
                } else {
                    best_picks[idx]
                },
                wiener: u.wiener,
                sgrproj: u.sgrproj,
            });
        }
        planes.push(PlaneRest {
            frame_rtype,
            unit_size,
            hunits,
            vunits,
            units: out_units,
        });
    }

    Ok(FrameRestInfo { planes })
}

/// Build the stripe-boundary line buffers exactly like the C pipeline:
/// after-deblock (pre-CDEF) pass + after-CDEF pass per plane.
/// `pre_cdef_*` = post-deblock planes, `post_cdef_*` = final CDEF'd planes.
///
/// `w`/`h` are the TRUE (coded) luma dims — C
/// `svt_av1_loop_restoration_save_boundary_lines` (restoration.c:1665) passes
/// `frame->crop_widths/crop_heights` as the extent and `frame->strides` as the
/// stride, and `svt_aom_save_tile_row_boundary_lines` bounds its stripe walk by
/// `whole_frame_rect(&cm->frm_size, ..)`, which is the coded (pre-8-alignment)
/// size, CEILING for chroma. `stride_y`/`stride_uv` are the ALIGNED canvas
/// strides the planes are actually stored at. Equal for an 8-aligned frame.
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
    stride_y: usize,
    stride_uv: usize,
    has_chroma: bool,
) -> alloc::vec::Vec<StripeBoundaries> {
    save_lr_boundaries_bd::<u8>(
        pre_y, pre_u, pre_v, post_y, post_u, post_v, w, h, stride_y, stride_uv, has_chroma,
    )
}

/// [`save_lr_boundaries`] at any [`LrPixel`] type — the `u16` instantiation
/// saves the 10-bit post-deblock / post-CDEF context lines for the 10-bit
/// apply (issue #13). C's `svt_aom_save_tile_row_boundary_lines` takes
/// `use_highbd` and only rescales byte counts with it, so the walk is one body.
#[allow(clippy::too_many_arguments)]
pub fn save_lr_boundaries_bd<P: LrPixel>(
    pre_y: &[P],
    pre_u: &[P],
    pre_v: &[P],
    post_y: &[P],
    post_u: &[P],
    post_v: &[P],
    w: usize,
    h: usize,
    stride_y: usize,
    stride_uv: usize,
    has_chroma: bool,
) -> alloc::vec::Vec<StripeBoundariesT<P>> {
    let mut out = alloc::vec::Vec::new();
    for plane in 0..3usize {
        let is_uv = plane > 0;
        let ss = i32::from(is_uv);
        // C whole_frame_rect: ROUND_POWER_OF_TWO (= CEILING) for chroma.
        let (pw, ph) = if is_uv {
            (w.div_ceil(2), h.div_ceil(2))
        } else {
            (w, h)
        };
        let stride = if is_uv { stride_uv } else { stride_y };
        let mut bnd = alloc_stripe_boundaries_t::<P>(w as i32, h as i32, ss);
        if is_uv && !has_chroma {
            out.push(bnd);
            continue;
        }
        let (pre, post) = match plane {
            0 => (pre_y, post_y),
            1 => (pre_u, post_u),
            _ => (pre_v, post_v),
        };
        save_tile_row_boundary_lines(pre, 0, stride, pw as i32, ph as i32, ss, false, &mut bnd);
        save_tile_row_boundary_lines(post, 0, stride, pw as i32, ph as i32, ss, true, &mut bnd);
        out.push(bnd);
    }
    out
}

/// C `svt_av1_loop_restoration_filter_frame` (restoration.c:1154): apply
/// the signaled restoration to the final recon planes in place (the output
/// copy — prediction sources are untouched by the caller's contract).
///
/// `w`/`h` are the TRUE (coded) luma dims and MUST be the same extent the
/// search sized the RU grid from: C runs both `svt_av1_alloc_restoration_struct`
/// (which sets `horz_units_per_tile` / `units_per_tile`) and this walk off ONE
/// `whole_frame_rect(&cm->frm_size, ..)` (restoration.c:51, 81, 1281), so the
/// unit count and the unit walk cannot disagree. `stride_y`/`stride_uv` are the
/// ALIGNED canvas strides the recon planes are stored at; the window outside
/// the true extent keeps its post-CDEF content, exactly as in C where the rect
/// stops at the coded size.
#[allow(clippy::too_many_arguments)]
pub fn apply_restoration_frame(
    recon_y: &mut [u8],
    recon_u: &mut [u8],
    recon_v: &mut [u8],
    w: usize,
    h: usize,
    stride_y: usize,
    stride_uv: usize,
    has_chroma: bool,
    info: &FrameRestInfo,
    boundaries: &[StripeBoundaries],
) {
    apply_restoration_frame_bd::<u8>(
        recon_y, recon_u, recon_v, w, h, stride_y, stride_uv, has_chroma, info, boundaries, 8,
    );
}

/// [`apply_restoration_frame`] at any [`LrPixel`] type. The `u16`
/// instantiation is the 10-bit apply (issue #13): C runs ONE
/// `svt_av1_loop_restoration_filter_frame` body at `highbd =
/// cm->use_highbitdepth` and only the per-unit convolve differs, so the
/// extend / unit walk / copy-back are this single generic body and the
/// per-depth kernel is [`LrPixel::filter_unit_apply`]. `bit_depth` reaches
/// the highbd convolve's rounding offsets and clamp; inert on `u8`.
#[allow(clippy::too_many_arguments)]
pub fn apply_restoration_frame_bd<P: LrPixel>(
    recon_y: &mut [P],
    recon_u: &mut [P],
    recon_v: &mut [P],
    w: usize,
    h: usize,
    stride_y: usize,
    stride_uv: usize,
    has_chroma: bool,
    info: &FrameRestInfo,
    boundaries: &[StripeBoundariesT<P>],
    bit_depth: u8,
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
        // C whole_frame_rect: ROUND_POWER_OF_TWO (= CEILING) for chroma.
        let (pw, ph) = if is_uv {
            (w.div_ceil(2), h.div_ceil(2))
        } else {
            (w, h)
        };
        let stride = if is_uv { stride_uv } else { stride_y };
        let recon: &mut [P] = match plane {
            0 => recon_y,
            1 => recon_u,
            _ => recon_v,
        };
        let mut data = PaddedPlaneT::<P>::from_strided(recon, stride, pw, ph);
        extend_frame(&mut data.data, data.origin, pw, ph, data.stride, 3, 3);
        let mut dst = PaddedPlaneT::<P>::empty(pw, ph);
        let rect = plane_rect(pw as i32, ph as i32);
        foreach_rest_unit_in_tile(&rect, pr.hunits, pr.unit_size, ss, |limits, unit_idx| {
            let u = &pr.units[unit_idx as usize];
            P::filter_unit_apply(
                limits,
                &u.params(),
                &boundaries[plane],
                &rect,
                ss,
                &mut data.data,
                data.origin,
                data.stride,
                &mut dst.data,
                dst.origin,
                dst.stride,
                bit_depth,
            );
        });
        dst.copy_crop_to_strided(recon, stride);
    }
}

/// C `svt_av1_loop_restoration_corners_in_sb` (restoration.c:1410) —
/// which restoration units have their top-left corner inside this
/// superblock (single tile). Returns `(rcol0, rcol1, rrow0, rrow1)` when
/// non-empty. `mi_*` are 4x4 luma units; `sb_mi` the SB span in mi (16 for
/// 64px SBs).
///
/// SUPERRES (chunk B.5): restoration units live on the UPSCALED frame while
/// superblocks are coded at the reduced width, so `frame_w` must be the
/// UPSCALED width and `sr_denom` the `SuperresDenom`. C then scales the
/// mi->pixel numerator by the denominator and the divisor by
/// `SCALE_NUMERATOR` (restoration.c:1457-1462):
/// `u = D * MI_SIZE * m / N`. `None` = unscaled = the pre-superres arithmetic
/// exactly (`mi_to_num_x = mi_size_x`, `denom_x = size`).
/// C `SCALE_NUMERATOR` (definitions.h:1451) — the superres numerator.
const SCALE_NUMERATOR: i32 = 8;

pub fn corners_in_sb(
    pr: &PlaneRest,
    is_uv: bool,
    mi_row: i32,
    mi_col: i32,
    sb_mi: i32,
    frame_w: usize,
    frame_h: usize,
    sr_denom: Option<u8>,
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
    // C `mi_to_num_x` / `denom_x` (restoration.c:1459-1465): under superres the
    // horizontal mapping carries the denominator; the vertical one never does
    // (superres is horizontal only).
    let (mi_to_num_x, denom_x) = match sr_denom {
        Some(d) => (mi_size_x * i32::from(d), size * SCALE_NUMERATOR),
        None => (mi_size_x, size),
    };
    let (rnd_x, rnd_y) = (denom_x - 1, size - 1);
    let rcol0 = (mi_col * mi_to_num_x + rnd_x) / denom_x;
    let rrow0 = (mi_row * mi_size_y + rnd_y) / size;
    let rcol1 = (((mi_col + sb_mi) * mi_to_num_x + rnd_x) / denom_x).min(horz_units);
    let rrow1 = (((mi_row + sb_mi) * mi_size_y + rnd_y) / size).min(vert_units);
    (rcol0 < rcol1 && rrow0 < rrow1).then_some((rcol0, rcol1, rrow0, rrow1))
}

/// Per-tile LR reference state for the entropy walk — C
/// `EntropyCodingContext.wiener_info[3]`, reset to the default filter at
/// the first SB of each tile (`svt_av1_reset_loop_restoration`,
/// ec_process.c:199; decoder mirror `av1_reset_loop_restoration`).
#[derive(Clone, Debug)]
pub struct LrWalkRefs {
    pub wiener: [WienerInfo; 3],
    /// The SGR twin — C `EntropyCodingContext.sgrproj_info[3]`, reset by the
    /// same `svt_av1_reset_loop_restoration` call.
    pub sgrproj: [crate::port_sgr_search::SgrprojInfo; 3],
}

impl Default for LrWalkRefs {
    /// `svt_av1_reset_loop_restoration` (entropy_coding.c:4019): BOTH
    /// references start at their C defaults, which for SGR is the range
    /// midpoint, NOT zero.
    fn default() -> Self {
        LrWalkRefs {
            wiener: [WienerInfo::default(); 3],
            sgrproj: [crate::port_sgr_search::SgrprojInfo::c_default(); 3],
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
    w: &mut crate::entropy::writer::AomWriter,
    fc: &mut crate::entropy::context::FrameContext,
    info: &FrameRestInfo,
    refs: &mut LrWalkRefs,
    mi_row: i32,
    mi_col: i32,
    sb_mi: i32,
    frame_w: usize,
    frame_h: usize,
    monochrome: bool,
    // Superres chunk B.5: `SuperresDenom` when the frame is scaled (then
    // `frame_w` is the UPSCALED width), `None` otherwise.
    sr_denom: Option<u8>,
) {
    let num_planes = if monochrome { 1 } else { 3 };
    for plane in 0..num_planes {
        let pr = &info.planes[plane];
        let Some((rcol0, rcol1, rrow0, rrow1)) = corners_in_sb(
            pr,
            plane > 0,
            mi_row,
            mi_col,
            sb_mi,
            frame_w,
            frame_h,
            sr_denom,
        ) else {
            continue;
        };
        for rrow in rrow0..rrow1 {
            for rcol in rcol0..rcol1 {
                let runit = (rcol + rrow * pr.hunits) as usize;
                let u = &pr.units[runit];
                let win = if plane > 0 {
                    WIENER_WIN_CHROMA
                } else {
                    WIENER_WIN
                };
                match pr.frame_rtype {
                    RESTORE_WIENER => {
                        let used = u.rtype != RESTORE_NONE;
                        w.write_symbol(usize::from(used), &mut fc.wiener_restore_cdf, 2);
                        if used {
                            let r = &mut refs.wiener[plane];
                            crate::entropy::lr::write_wiener_filter(
                                w,
                                win,
                                &u.wiener.vfilter,
                                &u.wiener.hfilter,
                                &mut r.vfilter,
                                &mut r.hfilter,
                            );
                        }
                    }
                    RESTORE_SGRPROJ => {
                        let used = u.rtype != RESTORE_NONE;
                        w.write_symbol(usize::from(used), &mut fc.sgrproj_restore_cdf, 2);
                        if used {
                            crate::entropy::lr::write_sgrproj_filter(
                                w,
                                &u.sgrproj,
                                &mut refs.sgrproj[plane],
                            );
                        }
                    }
                    RESTORE_SWITCHABLE => {
                        w.write_symbol(
                            u.rtype as usize,
                            &mut fc.switchable_restore_cdf,
                            crate::port_sgr_search::RESTORE_SWITCHABLE_TYPES,
                        );
                        match u.rtype {
                            RESTORE_WIENER => {
                                let r = &mut refs.wiener[plane];
                                crate::entropy::lr::write_wiener_filter(
                                    w,
                                    win,
                                    &u.wiener.vfilter,
                                    &u.wiener.hfilter,
                                    &mut r.vfilter,
                                    &mut r.hfilter,
                                );
                            }
                            RESTORE_SGRPROJ => {
                                crate::entropy::lr::write_sgrproj_filter(
                                    w,
                                    &u.sgrproj,
                                    &mut refs.sgrproj[plane],
                                );
                            }
                            _ => debug_assert_eq!(u.rtype, RESTORE_NONE),
                        }
                    }
                    _ => debug_assert!(false, "frame_rtype {} has no writer", pr.frame_rtype),
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

#[cfg(test)]
mod superres_lr_geom_tests {
    use super::*;

    fn pr(unit_size: i32) -> PlaneRest {
        PlaneRest {
            frame_rtype: RESTORE_WIENER,
            unit_size,
            hunits: 0,
            vunits: 0,
            units: alloc::vec::Vec::new(),
        }
    }

    /// C `svt_av1_loop_restoration_corners_in_sb` (restoration.c:1410) with
    /// the superres arms transcribed verbatim — the port must agree with it
    /// for every superres denominator, SB position and plane.
    ///
    /// EVIDENCE TIER: hand-transcribed formula, not an FFI call. The C symbol
    /// IS exported (`nm -g libSvtAv1Enc.a | grep corners_in_sb`), but it takes
    /// an `Av1Common*` whose `child_pcs->rst_info` must be built by hand;
    /// shimming it the way `c_parity_intrabc_mvp` shims its context is the
    /// upgrade path when superres chunk B.5 wires the rest of the LR path.
    fn c_reference(
        unit_size: i32,
        is_uv: bool,
        mi_row: i32,
        mi_col: i32,
        sb_mi: i32,
        upscaled_w: usize,
        frame_h: usize,
        sr_denom: Option<u8>,
    ) -> Option<(i32, i32, i32, i32)> {
        const SCALE_NUMERATOR: i32 = 8;
        let ss = i32::from(is_uv);
        // C `whole_frame_rect` (restoration.c:51): the LR tile rect is the
        // UPSCALED width, ROUND_POWER_OF_TWO'd for chroma.
        let tile_w = (upscaled_w as i32 + ss) >> ss;
        let tile_h = (frame_h as i32 + ss) >> ss;
        let horz_units = svtav1_dsp::restoration::count_units_in_tile(unit_size, tile_w);
        let vert_units = svtav1_dsp::restoration::count_units_in_tile(unit_size, tile_h);
        let (mi_size_x, mi_size_y) = (4 >> ss, 4 >> ss);
        let unscaled = sr_denom.is_none();
        let mi_to_num_x = if unscaled {
            mi_size_x
        } else {
            mi_size_x * i32::from(sr_denom.unwrap())
        };
        let denom_x = if unscaled {
            unit_size
        } else {
            unit_size * SCALE_NUMERATOR
        };
        let (rnd_x, rnd_y) = (denom_x - 1, unit_size - 1);
        let rcol0 = (mi_col * mi_to_num_x + rnd_x) / denom_x;
        let rrow0 = (mi_row * mi_size_y + rnd_y) / unit_size;
        let rcol1 = (((mi_col + sb_mi) * mi_to_num_x + rnd_x) / denom_x).min(horz_units);
        let rrow1 = (((mi_row + sb_mi) * mi_size_y + rnd_y) / unit_size).min(vert_units);
        (rcol0 < rcol1 && rrow0 < rrow1).then_some((rcol0, rcol1, rrow0, rrow1))
    }

    #[test]
    fn corners_in_sb_matches_c_across_superres_denominators() {
        for &unit_size in &[64i32, 128, 256] {
            for &upscaled_w in &[128usize, 256, 512] {
                let frame_h = 128usize;
                for denom in [None, Some(9u8), Some(12), Some(16)] {
                    for is_uv in [false, true] {
                        for sb in 0..(upscaled_w / 64) {
                            let mi_col = (sb * 16) as i32;
                            for mi_row in [0i32, 16, 32] {
                                let got = corners_in_sb(
                                    &pr(unit_size),
                                    is_uv,
                                    mi_row,
                                    mi_col,
                                    16,
                                    upscaled_w,
                                    frame_h,
                                    denom,
                                );
                                let want = c_reference(
                                    unit_size, is_uv, mi_row, mi_col, 16, upscaled_w, frame_h,
                                    denom,
                                );
                                assert_eq!(
                                    got, want,
                                    "unit {unit_size} w {upscaled_w} denom {denom:?} uv {is_uv} \
                                     mi ({mi_row},{mi_col})"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    /// ANTI-VACUITY: the superres arm must actually change the mapping —
    /// otherwise the test above would pass on a port that ignored `sr_denom`.
    #[test]
    fn superres_shifts_the_restoration_unit_mapping() {
        let (unit_size, upscaled_w, frame_h) = (64i32, 512usize, 128usize);
        let mut differing = 0;
        for sb in 0..(upscaled_w / 64) {
            let mi_col = (sb * 16) as i32;
            let unscaled = corners_in_sb(
                &pr(unit_size),
                false,
                0,
                mi_col,
                16,
                upscaled_w,
                frame_h,
                None,
            );
            let scaled = corners_in_sb(
                &pr(unit_size),
                false,
                0,
                mi_col,
                16,
                upscaled_w,
                frame_h,
                Some(16),
            );
            if unscaled != scaled {
                differing += 1;
            }
        }
        assert!(
            differing > 0,
            "denominator 16 must remap at least one superblock's restoration units"
        );
    }
}
