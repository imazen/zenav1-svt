//! The MD-level motion searches of `Source/Lib/Codec/product_coding_loop.c`.
//!
//! | this module | C |
//! |---|---|
//! | [`md_full_pel_search`] | `:1914-2049` |
//! | [`md_full_pel_search_large_lbd`] | `:1830-1912` |
//! | [`md_subpel_search_fixed_stage`] | `:2634-2731` |
//! | [`md_subpel_search`] | `:2520-2630` |
//! | [`subpel_mv_limits`] | `:2547-2557` (its limit derivation) |
//! | [`md_nsq_motion_search`] | `:2080-2252` |
//! | [`pme_search_for_ref`] | `:3216-3364` (`pme_search`'s per-reference BODY; the loop over `ref_frame_type_arr` is the caller's) |
//! | [`build_single_ref_mvp_list`] | `:3097-3187` (`build_single_ref_mvp_array`) |
//!
//! Two C functions are represented here by their PIECES rather than by a
//! driver. This table once listed FOUR of them as if they were ported, all
//! four as broken intra-doc links; corrected 2026-09-02, and the other two
//! (`md_nsq_motion_search`, `pme_search`) were ported the same day.
//!
//! | C driver | line | what IS here |
//! |---|---|---|
//! | `md_sq_motion_search` | `:2329-2510` | [`MdSqMeCtrls`], [`sparse_extent`], [`sq_search_area_multiplier`], [`nudge_sprs_lev1`] — and the DRIVER is deliberately absent: `pcs->md_sq_mv_search_level` is 0 unconditionally at all three of its derivation sites (`enc_mode_config.c:9200`, `:9753`, `:10033`), so it never runs |
//! | `read_refine_me_mvs` | `:2815-2936` | [`me_mv_center`] + [`refine_me_mv_for_ref`] (its per-reference BODY; the loop over `ref_frame_type_arr` is the caller's, because each iteration needs a different reference picture and MVP stack) |
//!
//! What is left between this module and C's inter reference set is WIRING,
//! not translation: the `ref_frame_type_arr` loop in
//! [`crate::inter_md_arm`], turning `inject_new_pme` /
//! `updated_enable_pme` on, and widening
//! [`crate::inter_pred_arm`]'s adapter to pass TWO MVs and two reference
//! planes — the compound prediction itself is already executable
//! (`svtav1_dsp::port_pd_pred::av1_inter_prediction_light_pd1` takes an
//! `mvs` slice and compounds when it has two).
//! `docs/INTER-ENCODE-PLAN.md` §1z¹⁴ says why those have to land together
//! rather than one at a time.
//!
//! # The shape of this port
//!
//! Every function here is a SEARCH: a geometry (which positions are
//! visited, in what order, with what clamping and what early exits) laid
//! over a per-position DISTORTION. The geometry is what changes the MV
//! and therefore the bitstream, and it is fully transcribed here. The
//! distortion is a handful of pixel kernels (`vf`, `svf`,
//! `svt_nxm_sad_kernel`, `svt_spatial_full_distortion_kernel`,
//! `sad_16b_kernel`) that live in the DSP layer; they arrive as a
//! [`DistortionSource`] so this module can be exercised, and reasoned
//! about, without a reference picture — and so a caller that has one
//! cannot accidentally get a different geometry.
//!
//! # Evidence
//!
//! **Tier 4** throughout: every C function listed above is `static` with
//! no exported symbol (`nm -g`). The pieces that DO have an oracle are
//! called rather than re-transcribed —
//! [`super::pme::pme_sad_loop_kernel`] and
//! [`super::pme::fp_mv_err_cost`] (tier 1),
//! [`super::pme::init_mv_cost_params`],
//! [`super::coding_loop::clip_mv_on_pic_boundary`],
//! [`super::predicates::get_max_drl_index`] (tier 1) and
//! [`super::drl::choose_best_av1_mv_pred`] (tier 1).

use alloc::{vec, vec::Vec};
use super::coding_loop::{check_spatial_mv_size, check_temporal_mv_size, clip_mv_on_pic_boundary};
use super::pme::{MvCostParams, PmeBest, fp_mv_err_cost, pme_sad_loop_kernel};
use super::predicates::get_max_drl_index;
use svtav1_types::motion::Mv;
use svtav1_types::prediction::PredictionMode;

/// C `DistortionType` (definitions.h): the metric
/// [`md_full_pel_search`] scores a position with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DistortionType {
    Sad = 0,
    Var = 1,
    Ssd = 2,
}

/// The reference picture's geometry as the searches read it.
#[derive(Debug, Clone, Copy)]
pub struct RefPicGeom {
    pub border: i32,
    pub max_width: i32,
    pub max_height: i32,
    pub y_stride: usize,
}

/// The 8-aligned half of [`md_full_pel_search_large_lbd`] needs the whole
/// plane, not a per-position index, so it is a separate provided method:
/// implementors that have the plane override it, and the default routes
/// through [`super::pme::pme_sad_loop_kernel`] on the slices they expose.
pub trait PmeSadLoop {
    /// C `svt_pme_sad_loop_kernel(mv_cost_params, src + input_origin_index,
    /// src_stride, ref + ref_origin_index, ref_stride, bheight, bwidth, ...)`.
    #[allow(clippy::too_many_arguments)]
    fn run_pme_sad_loop(
        &mut self,
        mv_cost_params: &MvCostParams<'_>,
        input_origin_index: usize,
        ref_origin_index: i32,
        search_area_width: i32,
        search_area_height: i32,
        start_x: i32,
        start_y: i32,
        search_step: i32,
        mvx: i16,
        mvy: i16,
        best: &mut PmeBest,
    );
}

/// The per-position pixel distortion the searches score with.
///
/// Implementors supply C's `vf` (variance), `svf` (sub-pel variance),
/// `svt_nxm_sad_kernel` / `sad_16b_kernel` and
/// `svt_spatial_full_distortion_kernel`. Every method takes an index into
/// the reference plane exactly as C computes it, so an implementor cannot
/// silently change the geometry.
pub trait DistortionSource: PmeSadLoop {
    /// C `fn_ptr->vf(ref + ref_origin_index, ref_stride, src + input_origin_index, src_stride, &sse)`.
    fn variance(&mut self, ref_origin_index: i32, input_origin_index: usize) -> u32;
    /// C `fn_ptr->svf(...)` at eighth-pel offsets `(subx, suby)`.
    fn subpel_variance(
        &mut self,
        ref_origin_index: i32,
        subx: i32,
        suby: i32,
        input_origin_index: usize,
    ) -> u32;
    /// C `svt_nxm_sad_kernel` (8-bit) / `sad_16b_kernel` (hbd).
    fn sad(&mut self, ref_origin_index: i32, input_origin_index: usize) -> u32;
    /// C `svt_spatial_full_distortion_kernel` / `svt_full_distortion_kernel16_bits`.
    fn ssd(&mut self, ref_origin_index: i32, input_origin_index: usize) -> u32;
    /// C's `fn_ptr->vf(pred, stride, svt_aom_eb_av1_var_offs, 0, &sse)` —
    /// the flat-reference variance `md_subpel_search_fixed_stage` uses for
    /// its `pred_variance_th` early exit.
    fn variance_vs_flat(&mut self, ref_origin_index: i32) -> u32;
}

// ---------------------------------------------------------------------------
// md_full_pel_search (product_coding_loop.c:1914-2049)
// ---------------------------------------------------------------------------

/// The block/plane geometry `md_full_pel_search` needs.
#[derive(Debug, Clone, Copy)]
pub struct FullPelCtx {
    pub blk_org_x: i32,
    pub blk_org_y: i32,
    pub bwidth: i32,
    pub bheight: i32,
    /// C `ctx->enable_psad`.
    pub enable_psad: bool,
    /// C's `hbd_md`; the large-LBD variant is 8-bit only.
    pub hbd_md: bool,
    /// C `ctx->sprs_lev0_start_x` .. `_end_y`.
    pub sprs_lev0_start_x: i32,
    pub sprs_lev0_end_x: i32,
    pub sprs_lev0_start_y: i32,
    pub sprs_lev0_end_y: i32,
}

/// The search window `md_full_pel_search` walks, in full-pel offsets
/// relative to `(mvx >> 3, mvy >> 3)`.
#[derive(Debug, Clone, Copy)]
pub struct SearchWindow {
    pub start_x: i32,
    pub end_x: i32,
    pub start_y: i32,
    pub end_y: i32,
    pub sparse_search_step: i32,
    /// C `is_sprs_lev0_performed`.
    pub is_sprs_lev0_performed: bool,
}

/// C's search-area adjustment (product_coding_loop.c:1928-1948): clamp
/// the window so every scanned position keeps the block inside the padded
/// reference.
///
/// The four clamps are asymmetric in the same way
/// [`clip_mv_on_pic_boundary`]'s are: the start sides subtract only the
/// origin plus the MV, the end sides subtract the origin, the MV AND the
/// block dimension.
pub fn clamp_search_window(
    ctx: &FullPelCtx,
    r: &RefPicGeom,
    mvx: i16,
    mvy: i16,
    w: &mut SearchWindow,
) {
    let mvx_fp = i32::from(mvx) >> 3;
    let mvy_fp = i32::from(mvy) >> 3;
    if ctx.blk_org_x + mvx_fp + w.start_x < -r.border + 1 {
        w.start_x = (-r.border + 1) - (ctx.blk_org_x + mvx_fp);
    }
    if ctx.blk_org_x + ctx.bwidth + mvx_fp + w.end_x > r.border + r.max_width - 1 {
        w.end_x = (r.border + r.max_width - 1) - (ctx.blk_org_x + ctx.bwidth + mvx_fp);
    }
    if ctx.blk_org_y + mvy_fp + w.start_y < -r.border + 1 {
        w.start_y = (-r.border + 1) - (ctx.blk_org_y + mvy_fp);
    }
    if ctx.blk_org_y + ctx.bheight + mvy_fp + w.end_y > r.border + r.max_height - 1 {
        w.end_y = (r.border + r.max_height - 1) - (ctx.blk_org_y + ctx.bheight + mvy_fp);
    }
}

/// C `md_full_pel_search` (product_coding_loop.c:1914-2049).
///
/// Four things a paraphrase loses:
///
/// * **The window is clamped BEFORE the psad dispatch**, so whether the
///   `md_full_pel_search_large_lbd` variant is taken depends on the
///   CLAMPED width, not the requested one.
/// * **The psad dispatch is `>= 7`, not `>= 8`**, even though the variant
///   it calls rounds the x extent up to a multiple of 8.
/// * **The `x` loop is OUTER and `y` INNER** — the opposite of the PME
///   kernel — so on a cost tie the smaller `x` wins.
/// * **The sparse level-1 skip** re-tests positions in absolute MV space
///   (`refinement_pos + (mv >> 3)` against `ctx->sprs_lev0_*`) and only
///   skips those that are also on the level-0 lattice
///   (`% 4 == 0` in BOTH axes).
#[allow(clippy::too_many_arguments)]
pub fn md_full_pel_search(
    ctx: &FullPelCtx,
    r: &RefPicGeom,
    dist: &mut impl DistortionSource,
    mv_cost_params: &MvCostParams<'_>,
    input_origin_index: usize,
    dist_type: DistortionType,
    mvx: i16,
    mvy: i16,
    window: SearchWindow,
    best: &mut PmeBest,
) {
    let mut w = window;
    clamp_search_window(ctx, r, mvx, mvy, &mut w);

    if dist_type == DistortionType::Sad
        && ctx.enable_psad
        && !ctx.hbd_md
        && (w.end_x - w.start_x) >= 7
    {
        md_full_pel_search_large_lbd(
            ctx,
            r,
            dist,
            mv_cost_params,
            input_origin_index,
            mvx,
            mvy,
            &w,
            best,
        );
        return;
    }

    let mvx_fp = i32::from(mvx) >> 3;
    let mvy_fp = i32::from(mvy) >> 3;
    let step = w.sparse_search_step.max(1);
    let mut px = w.start_x;
    while px <= w.end_x {
        let mut py = w.start_y;
        while py <= w.end_y {
            if w.sparse_search_step == 2
                && w.is_sprs_lev0_performed
                && (px + mvx_fp) >= ctx.sprs_lev0_start_x
                && (px + mvx_fp) <= ctx.sprs_lev0_end_x
                && (py + mvy_fp) >= ctx.sprs_lev0_start_y
                && (py + mvy_fp) <= ctx.sprs_lev0_end_y
                && px % 4 == 0
                && py % 4 == 0
            {
                py += step;
                continue;
            }
            let ref_origin_index =
                (ctx.blk_org_x + mvx_fp + px) + (ctx.blk_org_y + mvy_fp + py) * r.y_stride as i32;
            let mut cost = match dist_type {
                DistortionType::Var => dist.variance(ref_origin_index, input_origin_index),
                DistortionType::Ssd => dist.ssd(ref_origin_index, input_origin_index),
                DistortionType::Sad => dist.sad(ref_origin_index, input_origin_index),
            };
            let cand = Mv {
                x: (i32::from(mvx) + px * 8) as i16,
                y: (i32::from(mvy) + py * 8) as i16,
            };
            cost = cost.wrapping_add(fp_mv_err_cost(cand, mv_cost_params) as u32);
            if cost < best.cost {
                best.mvx = cand.x;
                best.mvy = cand.y;
                best.cost = cost;
            }
            py += step;
        }
        px += step;
    }
}

/// C `md_full_pel_search_large_lbd` (product_coding_loop.c:1830-1912).
///
/// The 8-column `mpsad` variant. **It does NOT search the same positions
/// as the generic path**: it rounds the x extent UP to a multiple of 8
/// (`remain_search_area`), hands the 8-aligned part to
/// [`super::pme::pme_sad_loop_kernel`] (whose column walk is the 8-wide
/// ratchet), and sweeps only the ragged tail with a plain SAD. Picking
/// the wrong variant changes the MV.
///
/// Note C computes `search_area_width` AFTER the round-up, so the tail
/// loop's `refinement_pos_x` starts at `start_x + (width & ~7)` — which
/// can exceed the ORIGINAL `end_x` and is then bounded by the
/// rounded-up one.
#[allow(clippy::too_many_arguments)]
pub fn md_full_pel_search_large_lbd(
    ctx: &FullPelCtx,
    r: &RefPicGeom,
    dist: &mut impl DistortionSource,
    mv_cost_params: &MvCostParams<'_>,
    input_origin_index: usize,
    mvx: i16,
    mvy: i16,
    w: &SearchWindow,
    best: &mut PmeBest,
) {
    let mvx_fp = i32::from(mvx) >> 3;
    let mvy_fp = i32::from(mvy) >> 3;
    let ref_origin_index = (ctx.blk_org_x + mvx_fp + w.start_x)
        + (ctx.blk_org_y + mvy_fp + w.start_y) * r.y_stride as i32;

    let mut remain = 8 - ((w.end_x - w.start_x) % 8);
    if remain == 8 {
        remain = 0;
    }
    let end_x = w.end_x.max(w.end_x + remain);
    let search_area_width = end_x - w.start_x;
    let search_area_height = w.end_y - w.start_y + 1;
    debug_assert_eq!(search_area_width & 7, 0);

    if search_area_width & !7i32 != 0 {
        // The kernel's own caller-side slice is the ref plane offset by
        // `ref_origin_index`; the port hands the index through the
        // DistortionSource's plane instead, so the shape is preserved by
        // `pme_sad_loop_kernel`'s own `ref_row_base` walk.
        dist.run_pme_sad_loop(
            mv_cost_params,
            input_origin_index,
            ref_origin_index,
            search_area_width & !7i32,
            search_area_height,
            w.start_x,
            w.start_y,
            w.sparse_search_step,
            mvx,
            mvy,
            best,
        );
    }

    if search_area_width & 7 != 0 {
        let mut py = w.start_y;
        while py <= w.end_y {
            let mut px = w.start_x + (search_area_width & !7i32);
            while px <= end_x {
                let idx = (ctx.blk_org_x + mvx_fp + px)
                    + (ctx.blk_org_y + mvy_fp + py) * r.y_stride as i32;
                let mut cost = dist.sad(idx, input_origin_index);
                let cand = Mv {
                    x: (i32::from(mvx) + px * 8) as i16,
                    y: (i32::from(mvy) + py * 8) as i16,
                };
                cost = cost.wrapping_add(fp_mv_err_cost(cand, mv_cost_params) as u32);
                if cost < best.cost {
                    best.mvx = cand.x;
                    best.mvy = cand.y;
                    best.cost = cost;
                }
                px += 1;
            }
            py += w.sparse_search_step.max(1);
        }
    }
}

/// A [`DistortionSource`] over real 8-bit planes, which is what the
/// encoder will hand in.
pub struct PlaneDistortion<'a> {
    pub src: &'a [u8],
    pub src_stride: usize,
    /// The reference plane, indexed from its (0,0); `ref_origin_index` may
    /// be negative into the border, so the slice must include it and
    /// `ref_org` is the index of pixel (0,0).
    pub ref_plane: &'a [u8],
    pub ref_org: usize,
    pub ref_stride: usize,
    pub bwidth: usize,
    pub bheight: usize,
}

impl PlaneDistortion<'_> {
    #[inline]
    fn ref_at(&self, idx: i32) -> usize {
        (self.ref_org as i64 + i64::from(idx)) as usize
    }

    fn sum_abs_diff(&self, ref_origin_index: i32, input_origin_index: usize) -> u32 {
        let base = self.ref_at(ref_origin_index);
        svtav1_dsp::me_sad::block_sad(
            &self.src[input_origin_index..],
            self.src_stride,
            &self.ref_plane[base..],
            self.ref_stride,
            self.bwidth,
            self.bheight,
        )
    }
}

impl DistortionSource for PlaneDistortion<'_> {
    fn variance(&mut self, ref_origin_index: i32, input_origin_index: usize) -> u32 {
        // C `aom_variance<W>x<H>_c`: sum of squares minus (sum^2 / n). The
        // accumulation is `me_sad::block_sum_sse` (SIMD, exact); the operand
        // order is C's, `d = ref - src`.
        let base = self.ref_at(ref_origin_index);
        let (sum, sse) = svtav1_dsp::me_sad::block_sum_sse(
            &self.ref_plane[base..],
            self.ref_stride,
            &self.src[input_origin_index..],
            self.src_stride,
            self.bwidth,
            self.bheight,
        );
        let sum = i64::from(sum);
        let n = (self.bwidth * self.bheight) as i64;
        (i64::from(sse) - (sum * sum) / n) as u32
    }

    fn subpel_variance(
        &mut self,
        ref_origin_index: i32,
        subx: i32,
        suby: i32,
        input_origin_index: usize,
    ) -> u32 {
        // C `fn_ptr->svf` — `svt_aom_sub_pixel_variance{W}x{H}_c` (bilinear).
        let base = self.ref_at(ref_origin_index);
        svtav1_dsp::subpel_variance::sub_pixel_variance(
            self.ref_plane,
            base,
            self.ref_stride,
            subx as usize,
            suby as usize,
            self.src,
            input_origin_index,
            self.src_stride,
            self.bwidth,
            self.bheight,
        )
        .0
    }

    fn sad(&mut self, ref_origin_index: i32, input_origin_index: usize) -> u32 {
        self.sum_abs_diff(ref_origin_index, input_origin_index)
    }

    fn ssd(&mut self, ref_origin_index: i32, input_origin_index: usize) -> u32 {
        let base = self.ref_at(ref_origin_index);
        svtav1_dsp::me_sad::block_sum_sse(
            &self.ref_plane[base..],
            self.ref_stride,
            &self.src[input_origin_index..],
            self.src_stride,
            self.bwidth,
            self.bheight,
        )
        .1
    }

    fn variance_vs_flat(&mut self, ref_origin_index: i32) -> u32 {
        // C `fn_ptr->vf(ref + idx, ref_stride, svt_aom_eb_av1_var_offs, 0,
        // &sse)` — the second operand's stride is 0, so every row reads the
        // same 128-valued row.
        let base = self.ref_at(ref_origin_index);
        svtav1_dsp::subpel_variance::variance_diff_sse(
            self.ref_plane,
            base,
            self.ref_stride,
            &crate::md_subpel::EB_AV1_VAR_OFFS,
            0,
            0,
            self.bwidth,
            self.bheight,
        )
        .0
    }
}

impl PmeSadLoop for PlaneDistortion<'_> {
    fn run_pme_sad_loop(
        &mut self,
        mv_cost_params: &MvCostParams<'_>,
        input_origin_index: usize,
        ref_origin_index: i32,
        search_area_width: i32,
        search_area_height: i32,
        start_x: i32,
        start_y: i32,
        search_step: i32,
        mvx: i16,
        mvy: i16,
        best: &mut PmeBest,
    ) {
        let base = self.ref_at(ref_origin_index);
        pme_sad_loop_kernel(
            mv_cost_params,
            &self.src[input_origin_index..],
            self.src_stride,
            &self.ref_plane[base..],
            self.ref_stride,
            self.bheight,
            self.bwidth,
            best,
            start_x as i16,
            start_y as i16,
            search_area_width as i16,
            search_area_height as i16,
            search_step as i16,
            mvx,
            mvy,
        );
    }
}

// ---------------------------------------------------------------------------
// md_sq_motion_search (product_coding_loop.c:2329-2510)
// ---------------------------------------------------------------------------

/// C `MdSqMotionSearchCtrls`, the fields the search reads.
#[derive(Debug, Clone, Copy, Default)]
pub struct MdSqMeCtrls {
    pub enabled: bool,
    pub enable_psad: bool,
    pub dist_type_var: bool,
    pub pame_distortion_th: u32,
    pub sprs_lev0_enabled: bool,
    pub sprs_lev0_step: u8,
    pub sprs_lev0_w: u16,
    pub sprs_lev0_h: u16,
    pub max_sprs_lev0_w: u16,
    pub max_sprs_lev0_h: u16,
    pub sprs_lev0_multiplier: u16,
    pub sprs_lev1_enabled: bool,
    pub sprs_lev1_step: u8,
    pub sprs_lev1_w: u16,
    pub sprs_lev1_h: u16,
    pub max_sprs_lev1_w: u16,
    pub max_sprs_lev1_h: u16,
    pub sprs_lev1_multiplier: u16,
    pub sprs_lev2_enabled: bool,
    pub sprs_lev2_step: u8,
    pub sprs_lev2_w: u16,
    pub sprs_lev2_h: u16,
}

/// C's sparse-search extent derivation, shared by levels 0 and 1
/// (product_coding_loop.c:2396-2400 and :2432-2436).
///
/// `(multiplier * MIN(w * search_area_multiplier * dist, max_w)) / 100`,
/// then the half-extent is rounded DOWN to a multiple of the step so the
/// window is step-aligned.
pub fn sparse_extent(
    multiplier: u16,
    w: u16,
    search_area_multiplier: u8,
    dist: u16,
    max_w: u16,
    step: u8,
) -> i32 {
    let scaled = u32::from(w) * u32::from(search_area_multiplier) * u32::from(dist);
    let capped = scaled.min(u32::from(max_w));
    let ext = (u32::from(multiplier) * capped) / 100;
    let step = i32::from(step).max(1);
    ((ext as i32 >> 1) / step) * step
}

/// C `md_sq_motion_search`'s high-motion detector
/// (product_coding_loop.c:2368-2384).
///
/// Returns the `search_area_multiplier`: 0 when the ME distortion is
/// already good enough (or the block is bigger than 64), otherwise the
/// TEMPORAL category for an inter reference and the SPATIAL one for an
/// intra / key reference. **The two categories are computed by different
/// functions with different semantics** — see
/// [`check_spatial_mv_size`] (signed comparisons) vs
/// [`check_temporal_mv_size`] (absolute).
///
/// The RDCOST comparison C writes reduces to comparing the two
/// distortions at the same rate, so the port compares them directly and
/// says so rather than carrying a lambda that cancels.
#[allow(clippy::too_many_arguments)]
pub fn sq_search_area_multiplier(
    ctrls: &MdSqMeCtrls,
    sq_size: u32,
    bwidth: u32,
    bheight: u32,
    pa_me_cost: u32,
    ref_is_intra_or_key: bool,
    mvp_array: &[Mv],
    me_mv_x: i16,
    me_mv_y: i16,
    mfmv0: Mv,
) -> u8 {
    if sq_size > 64 {
        return 0;
    }
    // C: RDCOST(l, 16, a) > RDCOST(l, 16, b) with the same lambda and
    // rate on both sides, so the rate term cancels exactly.
    let th = u64::from(ctrls.pame_distortion_th) * u64::from(bwidth) * u64::from(bheight);
    if u64::from(pa_me_cost) <= th {
        return 0;
    }
    if ref_is_intra_or_key {
        check_spatial_mv_size(mvp_array, me_mv_x, me_mv_y)
    } else {
        check_temporal_mv_size(mfmv0)
    }
}

/// C's level-1 window nudge (product_coding_loop.c:2438-2445): a
/// start/end that lands on a multiple of 4 is pushed OUT by 2, so the
/// level-1 lattice never coincides with level 0's at the window edges.
#[inline]
pub fn nudge_sprs_lev1(start: i32, end: i32) -> (i32, i32) {
    (
        if start % 4 == 0 { start - 2 } else { start },
        if end % 4 == 0 { end + 2 } else { end },
    )
}

// ---------------------------------------------------------------------------
// md_nsq_motion_search (product_coding_loop.c:2080-2252)
// ---------------------------------------------------------------------------

/// C `MAX_MD_NSQ_SARCH_MVC_CNT` (product_coding_loop.c:2078).
pub const MAX_MD_NSQ_SEARCH_MVC_CNT: usize = 6;

/// C's MVC-list construction for the NSQ search
/// (product_coding_loop.c:2086-2148).
///
/// The list starts with the SQ MV, appends the sub-block MVs that pass
/// the geometry + ME-presence filter (deduped), and ALWAYS considers a
/// zero MV last.
///
/// **Two C quirks reproduced:** the dedup compares
/// `mvc_x_array[mvc_count]` — the slot being written, not the value just
/// computed — which is the same thing only because the value was stored
/// there first; and the trailing zero-MV is written at `[mvc_count]`
/// BEFORE the dedup loop reads it, so a list already containing (0,0)
/// leaves the count unchanged and the zero entry is simply not counted.
///
/// `sub_block_mvs` is the already-filtered set of candidate sub-block
/// MVs in C's `block_index` order; the geometry filter itself
/// (`partition_width` / `pu_search_index_map`) is ME-table machinery
/// supplied by the caller.
pub fn nsq_mvc_list(sq_mv: Mv, sub_block_mvs: &[Mv]) -> Vec<Mv> {
    let mut list: Vec<Mv> = Vec::with_capacity(MAX_MD_NSQ_SEARCH_MVC_CNT);
    list.push(sq_mv);
    for &m in sub_block_mvs {
        if list.len() >= MAX_MD_NSQ_SEARCH_MVC_CNT {
            break;
        }
        if !list.iter().any(|e| e.x == m.x && e.y == m.y) {
            list.push(m);
        }
    }
    // C then writes (0,0) at [mvc_count] and counts it only if absent.
    if !list.iter().any(|e| e.x == 0 && e.y == 0) && list.len() < MAX_MD_NSQ_SEARCH_MVC_CNT {
        list.push(Mv::ZERO);
    }
    list
}

/// C's "round-up the search center to the closest integer"
/// (product_coding_loop.c:2160-2161): `(v + 4) & ~0x07`.
///
/// This is a round-to-nearest-full-pel in eighth-pel units with ties
/// going UP, and it is applied to the MVC array IN PLACE before each
/// search — a port that rounded toward zero would search from a
/// different centre.
#[inline]
pub fn round_to_full_pel(v: i16) -> i16 {
    (v.wrapping_add(4)) & !0x07
}

/// C `md_nsq_motion_search` (product_coding_loop.c:2080-2252) — the
/// NSQ-shape full-pel refinement, assembled from [`nsq_mvc_list`] and four
/// [`md_full_pel_search`] passes.
///
/// `me_mv` is EIGHTH-PEL in and out. `sub_block_mvs` is the geometry- and
/// ME-presence-filtered sub-block candidate set in C's `block_index` order
/// (already multiplied by 8), which the caller supplies because the filter
/// itself is ME-table machinery, not search geometry — see
/// [`nsq_mvc_list`].
///
/// Four details that decide the MV:
///
/// * **The MVC entries are rounded IN PLACE, and the first search centre
///   is read BEFORE the rounding.** C seeds `search_center_mv` from
///   `mvc_array[0]` unrounded with cost `~0`, then rounds each entry
///   inside the evaluation loop. The seed is therefore dead — any
///   evaluation beats `~0` — but it is transcribed rather than
///   simplified away.
/// * **Each MVC entry is evaluated with a ZERO window** (`0,0,0,0`,
///   step 1), i.e. one position each; the search proper only starts
///   afterwards.
/// * **The three refinement passes are a 4 -> 2 -> 1 step ladder** with
///   windows `±(full_pel_search_{width,height} >> 1)`, `±2` and `±1`, each
///   starting from the previous pass's winner and sharing ONE running
///   `best_search_cost`.
/// * **The refinement result is taken only if it BEATS the MVC winner**
///   (`best_search_cost < search_center_cost`); on a tie the MVC winner
///   stands.
///
/// Evidence tier 4 — `md_nsq_motion_search` is `static` with no exported
/// symbol. Its two leaves are the already-ported [`nsq_mvc_list`] and
/// [`md_full_pel_search`], and the per-position distortion arrives through
/// [`DistortionSource`] so the geometry can be asserted without pixels.
#[allow(clippy::too_many_arguments)]
pub fn md_nsq_motion_search(
    ctx: &FullPelCtx,
    r: &RefPicGeom,
    dist: &mut impl DistortionSource,
    mv_cost_params: &MvCostParams<'_>,
    input_origin_index: usize,
    dist_type: DistortionType,
    full_pel_search_width: u8,
    full_pel_search_height: u8,
    sq_mv: Mv,
    sub_block_mvs: &[Mv],
    me_mv: &mut Mv,
) {
    let mut mvc = nsq_mvc_list(sq_mv, sub_block_mvs);

    // C: seeded from the UNROUNDED first entry, with cost ~0.
    let mut center = PmeBest {
        cost: u32::MAX,
        mvx: mvc[0].x,
        mvy: mvc[0].y,
    };
    let zero_window = SearchWindow {
        start_x: 0,
        end_x: 0,
        start_y: 0,
        end_y: 0,
        sparse_search_step: 1,
        is_sprs_lev0_performed: false,
    };
    #[cfg(feature = "std")]
    let dbg_nsq = crate::dbgenv::nsqsrch();
    #[cfg(feature = "std")]
    if dbg_nsq {
        std::eprint!(
            "NSQSRCH org=({},{}) {}x{} sqmv=({},{}) rawmvc=",
            ctx.blk_org_x,
            ctx.blk_org_y,
            ctx.bwidth,
            ctx.bheight,
            sq_mv.y,
            sq_mv.x
        );
        for m in &mvc {
            std::eprint!("({},{})", m.y, m.x);
        }
        std::eprintln!();
    }
    for m in &mut mvc {
        m.x = round_to_full_pel(m.x);
        m.y = round_to_full_pel(m.y);
        md_full_pel_search(
            ctx,
            r,
            dist,
            mv_cost_params,
            input_origin_index,
            dist_type,
            m.x,
            m.y,
            zero_window,
            &mut center,
        );
        #[cfg(feature = "std")]
        if dbg_nsq {
            std::eprintln!(
                "  mvc({},{}) -> center=({},{}) cost={}",
                m.y,
                m.x,
                center.mvy,
                center.mvx,
                center.cost
            );
        }
    }

    me_mv.x = center.mvx;
    me_mv.y = center.mvy;

    // C initialises the refinement best to `(int16_t)~0` = -1 with cost
    // `~0`; the first pass overwrites both.
    let mut best = PmeBest {
        cost: u32::MAX,
        mvx: -1,
        mvy: -1,
    };
    let ladder = [
        (
            i32::from(full_pel_search_width) >> 1,
            i32::from(full_pel_search_height) >> 1,
            4i32,
        ),
        (2, 2, 2),
        (1, 1, 1),
    ];
    let (mut sx, mut sy) = (center.mvx, center.mvy);
    for (wx, wy, step) in ladder {
        md_full_pel_search(
            ctx,
            r,
            dist,
            mv_cost_params,
            input_origin_index,
            dist_type,
            sx,
            sy,
            SearchWindow {
                start_x: -wx,
                end_x: wx,
                start_y: -wy,
                end_y: wy,
                sparse_search_step: step,
                is_sprs_lev0_performed: false,
            },
            &mut best,
        );
        #[cfg(feature = "std")]
        if dbg_nsq {
            std::eprintln!(
                "  ladder w{}x{} step{} from({},{}) -> best=({},{}) cost={}",
                wx,
                wy,
                step,
                sy,
                sx,
                best.mvy,
                best.mvx,
                best.cost
            );
        }
        sx = best.mvx;
        sy = best.mvy;
    }

    if best.cost < center.cost {
        me_mv.x = best.mvx;
        me_mv.y = best.mvy;
    }
}

// ---------------------------------------------------------------------------
// md_subpel_search_fixed_stage (product_coding_loop.c:2634-2731)
// ---------------------------------------------------------------------------

/// C `hpel_dx` / `hpel_dy` (product_coding_loop.c:2652-2653).
pub const HPEL_DX: [i8; 4] = [4, -4, 0, 0];
pub const HPEL_DY: [i8; 4] = [0, 0, 4, -4];
/// C `qpel_dx` / `qpel_dy` (product_coding_loop.c:2701-2702).
pub const QPEL_DX: [i8; 4] = [2, -2, 0, 0];
pub const QPEL_DY: [i8; 4] = [0, 0, 2, -2];

/// C `MdSubPelSearchCtrls`, the fields the fixed-stage search reads.
#[derive(Debug, Clone, Copy, Default)]
pub struct MdSubpelCtrls {
    pub enabled: bool,
    /// C `max_precision`; `QUARTER_PEL` is 1 and the quarter-pel stage
    /// runs when `max_precision <= QUARTER_PEL`.
    pub max_precision: u8,
    pub abs_th_mult: u32,
    pub pred_variance_th: u32,
    pub bias_fp: u16,
    pub min_blk_sz: u16,
    /// C `subpel_search_method == SUBPEL_FIXED_STAGE_SEARCH`.
    pub fixed_stage: bool,
    pub subpel_iters_per_step: u8,
    pub skip_diag_refinement: u8,
}

/// C `QUARTER_PEL` (definitions.h).
pub const QUARTER_PEL: u8 = 1;

/// C `md_subpel_search_fixed_stage` (product_coding_loop.c:2634-2731).
///
/// Four details that decide the MV:
///
/// * **The early exits return WITHOUT writing `me_mv`** on the
///   integer-pel baseline, so a block that exits there keeps its full-pel
///   MV — but the half-pel and quarter-pel early exits DO write it first.
/// * **`bias_fp` is applied only while the current best is still the
///   integer position** (`best_dx == 0 && best_dy == 0`), and the biased
///   value is used ONLY for the comparison: `best_var` is assigned the
///   UNBIASED `var`.
/// * **The quarter-pel offsets are relative to the winning half-pel
///   offset** (`best_dx + qpel_dx[i]`), not to the integer position.
/// * **`pred_variance_th`'s flat-reference variance is normalised by
///   `ROUND_POWER_OF_TWO(var, num_pels_log2)`**, i.e. per-pixel, before
///   the comparison.
#[allow(clippy::too_many_arguments)]
pub fn md_subpel_search_fixed_stage(
    ctrls: &MdSubpelCtrls,
    dist: &mut impl DistortionSource,
    blk_org_x: i32,
    blk_org_y: i32,
    bwidth: u32,
    bheight: u32,
    num_pels_log2: u32,
    ref_stride: usize,
    input_origin_index: usize,
    me_mv: &mut Mv,
) -> u32 {
    let mv_x_fp = i32::from(me_mv.x);
    let mv_y_fp = i32::from(me_mv.y);
    let th_normalizer = bwidth * bheight * ctrls.abs_th_mult;

    let idx_of = |dx: i32, dy: i32| -> i32 {
        let fp_x = blk_org_x + ((mv_x_fp + dx) >> 3);
        let fp_y = blk_org_y + ((mv_y_fp + dy) >> 3);
        fp_x + fp_y * ref_stride as i32
    };

    let mut best_var = dist.variance(idx_of(0, 0), input_origin_index);
    let mut best_dx = 0i32;
    let mut best_dy = 0i32;
    if best_var < th_normalizer {
        return best_var;
    }
    if ctrls.pred_variance_th != 0 {
        let var = dist.variance_vs_flat(idx_of(0, 0));
        // C ROUND_POWER_OF_TWO(var, num_pels_log2).
        let block_var = if num_pels_log2 == 0 {
            var
        } else {
            (var + (1 << (num_pels_log2 - 1))) >> num_pels_log2
        };
        if block_var < ctrls.pred_variance_th {
            return best_var;
        }
    }

    for i in 0..4usize {
        let dx = i32::from(HPEL_DX[i]);
        let dy = i32::from(HPEL_DY[i]);
        let subx = (mv_x_fp + dx) & 7;
        let suby = (mv_y_fp + dy) & 7;
        let var = dist.subpel_variance(idx_of(dx, dy), subx, suby, input_origin_index);
        let biased = if ctrls.bias_fp != 0 && best_dx == 0 && best_dy == 0 {
            ((u64::from(var) * u64::from(ctrls.bias_fp)) / 100) as u32
        } else {
            var
        };
        if biased < best_var {
            best_var = var;
            best_dx = dx;
            best_dy = dy;
            if best_var < th_normalizer {
                me_mv.x = (mv_x_fp + best_dx) as i16;
                me_mv.y = (mv_y_fp + best_dy) as i16;
                return best_var;
            }
        }
    }

    if ctrls.max_precision <= QUARTER_PEL {
        for i in 0..4usize {
            let tot_dx = best_dx + i32::from(QPEL_DX[i]);
            let tot_dy = best_dy + i32::from(QPEL_DY[i]);
            let subx = (mv_x_fp + tot_dx) & 7;
            let suby = (mv_y_fp + tot_dy) & 7;
            let var = dist.subpel_variance(idx_of(tot_dx, tot_dy), subx, suby, input_origin_index);
            let biased = if ctrls.bias_fp != 0 && best_dx == 0 && best_dy == 0 {
                ((u64::from(var) * u64::from(ctrls.bias_fp)) / 100) as u32
            } else {
                var
            };
            if biased < best_var {
                best_var = var;
                best_dx = tot_dx;
                best_dy = tot_dy;
                if best_var < th_normalizer {
                    me_mv.x = (mv_x_fp + best_dx) as i16;
                    me_mv.y = (mv_y_fp + best_dy) as i16;
                    return best_var;
                }
            }
        }
    }
    me_mv.x = (mv_x_fp + best_dx) as i16;
    me_mv.y = (mv_y_fp + best_dy) as i16;
    best_var
}

/// C `md_subpel_search`'s MV-limit derivation
/// (product_coding_loop.c:2547-2557).
///
/// `AOM_INTERP_EXTEND` is 4. Note the MIN sides use `mi_row + mi_height`
/// (the block's FAR edge) while the MAX sides use `mi_rows - mi_row` (the
/// picture's far edge minus the block's NEAR edge) — the asymmetry is C's.
pub fn subpel_mv_limits(
    mi_row: i32,
    mi_col: i32,
    mi_width: i32,
    mi_height: i32,
    mi_rows: i32,
    mi_cols: i32,
) -> (i32, i32, i32, i32) {
    /// C `AOM_INTERP_EXTEND`.
    const AOM_INTERP_EXTEND: i32 = 4;
    /// C `MI_SIZE`.
    const MI_SIZE: i32 = 4;
    let row_min = -(((mi_row + mi_height) * MI_SIZE) + AOM_INTERP_EXTEND);
    let col_min = -(((mi_col + mi_width) * MI_SIZE) + AOM_INTERP_EXTEND);
    let row_max = (mi_rows - mi_row) * MI_SIZE + AOM_INTERP_EXTEND;
    let col_max = (mi_cols - mi_col) * MI_SIZE + AOM_INTERP_EXTEND;
    (row_min, row_max, col_min, col_max)
}

// ---------------------------------------------------------------------------
// md_subpel_search (product_coding_loop.c:2520-2630)
// ---------------------------------------------------------------------------

/// The pieces of C's `md_subpel_search` that its caller owns: the block's
/// mi geometry, the picture's, and the reference/source planes.
///
/// The two plane fields mirror C's `ms_buffers`: `src` is the SOURCE
/// picture (`pcs->ppcs->enhanced_pic`, NOT a recon) at this block's origin,
/// and `ref_alloc` is the padded REFERENCE allocation with `ref_base` the
/// index of the block's own (0, 0) inside it — the same shape
/// [`crate::md_subpel::SubpelSearchVarParams`] takes, because that is what
/// the negative offsets `get_buf_from_mv` produces require.
#[derive(Debug, Clone, Copy)]
pub struct SubpelBlockGeom {
    /// C `xd->mi_row` / `xd->mi_col`.
    pub mi_row: i32,
    pub mi_col: i32,
    /// C `mi_size_wide[bsize]` / `mi_size_high[bsize]`.
    pub mi_width: i32,
    pub mi_height: i32,
    /// C `cm->mi_rows` / `cm->mi_cols`.
    pub mi_rows: i32,
    pub mi_cols: i32,
    /// C `block_size_wide[bsize]` / `block_size_high[bsize]`.
    pub bwidth: usize,
    pub bheight: usize,
    /// C `ctx->blk_geom->sq_size` — the SQUARE size the block came from,
    /// which for an NSQ shape is its parent's, NOT `max(bwidth, bheight)`.
    /// `svt_init_mv_cost_params` reads it for `early_exit_th`.
    pub sq_size: u16,
}

/// C `md_subpel_search` (product_coding_loop.c:2520-2630) — the SETUP
/// wrapper around `mcomp.c`'s two tree searches.
///
/// It owns no geometry of its own: everything it does is derive the MV
/// limits, the MV-cost parameters and the variance parameters, then
/// dispatch to [`crate::md_subpel::find_best_sub_pixel_tree`] or
/// [`crate::md_subpel::find_best_sub_pixel_tree_pruned`]. `me_mv` is
/// updated in place with the refined EIGHTH-PEL MV and the return value is
/// C's `besterr`.
///
/// Three things a rewrite gets wrong, all transcribed here:
///
/// * **The MV-limit chain is three steps, and the middle one narrows the
///   FULL-PEL set in place.** [`subpel_mv_limits`] gives the block/picture
///   rectangle, `svt_av1_set_mv_search_range` intersects it with the
///   `±MAX_FULL_PEL_VAL` window around `ref_mv`, and only then does
///   `svt_av1_set_subpel_mv_search_range` scale to eighth pel. Skipping
///   the middle step leaves a search range up to 1023 full pels wider.
/// * **The two limit helpers take their four components in DIFFERENT
///   orders** — [`subpel_mv_limits`] returns `(row_min, row_max, col_min,
///   col_max)` and
///   [`crate::md_subpel::set_subpel_mv_search_range`] takes `(col_min,
///   col_max, row_min, row_max)`.
/// * **The start MV is a full-pel ROUND-TRIP, not the input MV.** C takes
///   `me_mv >> 3` and then `get_mv_from_fullmv` (`<< 3`), so any
///   fractional part of the incoming MV is DISCARDED before the search
///   starts. (C's own comment says it should use `get_fullmv_from_mv`,
///   which rounds; it does not, and the truncation is the behaviour.)
///
/// **`md_ctx` is `Some` in C, always.** `md_subpel_search` passes
/// `ctx` to the tree search unconditionally, and the PRUNED variant writes
/// `ctx->fp_me_dist[list][ref] = besterr` there when
/// `search_stage == SPEL_ME` (mcomp.c:616-618) — a value
/// `read_refine_me_mvs` and `pme_search` both READ afterwards. Passing
/// `None` reproduces C's `ictx == NULL` arm, which no call site in
/// `product_coding_loop.c` takes; a caller wiring the ME/PME drivers must
/// pass `Some` or it silently loses that write. (The UNPRUNED variant
/// casts `ictx` without a null check, so `None` there is C's undefined
/// behaviour, not a supported arm.)
///
/// Evidence tier 4 — `md_subpel_search` is `static` with no exported
/// symbol. Its leaves are not: the two tree searches are C's exported
/// `svt_av1_find_best_sub_pixel_tree{,_pruned}` and are called here rather
/// than re-transcribed.
#[allow(clippy::too_many_arguments)]
pub fn md_subpel_search(
    search_stage: i32,
    ctrls: &crate::port_enc_mode_config::encdec::MdSubPelSearchCtrls,
    geom: SubpelBlockGeom,
    bsize: svtav1_types::block::BlockSize,
    list_idx: usize,
    ref_idx: usize,
    allow_high_precision_mv: bool,
    ref_mv: Mv,
    base_q_idx: usize,
    full_lambda: u32,
    // C `ctx->md_subpel_me_ctrls.skip_diag_refinement` — see the note at
    // the `mv_cost_type` assignment; it is the ME controls' field even on
    // a PME call.
    me_skip_diag_refinement: u8,
    mv_cost_tables: Option<&crate::intrabc::MvCostTables>,
    src: &[u8],
    src_base: usize,
    src_stride: usize,
    ref_alloc: &[u8],
    ref_base: i64,
    ref_stride: usize,
    mut md_ctx: Option<&mut crate::md_subpel::SubpelMdContext>,
    me_mv: &mut Mv,
) -> u32 {
    use crate::md_subpel::{
        SubpelSearchParams, SubpelSearchVarParams, find_best_sub_pixel_tree,
        find_best_sub_pixel_tree_pruned, set_subpel_mv_search_range,
    };

    // C `mv_limits` (:2547-2557), then `svt_av1_set_mv_search_range`
    // (:2558) narrowing it in place, then the eighth-pel conversion.
    let (row_min, row_max, col_min, col_max) = subpel_mv_limits(
        geom.mi_row,
        geom.mi_col,
        geom.mi_width,
        geom.mi_height,
        geom.mi_rows,
        geom.mi_cols,
    );
    let mut full = svtav1_types::motion::FullMvLimits {
        col_min,
        col_max,
        row_min,
        row_max,
    };
    crate::intrabc::set_mv_search_range(&mut full, ref_mv);
    let mv_limits = set_subpel_mv_search_range(
        (full.col_min, full.col_max, full.row_min, full.row_max),
        ref_mv,
    );

    let ms = SubpelSearchParams {
        allow_hp: allow_high_precision_mv,
        forced_stop: i32::from(ctrls.max_precision),
        iters_per_step: ctrls.subpel_iters_per_step,
        pred_variance_th: ctrls.pred_variance_th,
        abs_th_mult: ctrls.abs_th_mult,
        round_dev_th: ctrls.round_dev_th,
        skip_diag_refinement: ctrls.skip_diag_refinement,
        search_stage,
        list_idx,
        ref_idx,
        mv_limits,
    };

    // C `svt_init_mv_cost_params` (:2560-2565), restricted to the members
    // `mcomp.c` actually reads (`md_subpel::MvCostParams` documents which
    // two it drops). The 8-BIT lambda, because the variance this search
    // minimises is computed at 8 bits.
    //
    // NOTE `base_q_idx` is unread here for exactly that reason: it feeds
    // `sad_per_bit`, and no function in `mcomp.c` reads `sad_per_bit`. It
    // stays in the signature because C passes it and a caller wiring the
    // PME/ME drivers must not have to rediscover that.
    let _ = base_q_idx;
    // **C reads the ME controls here, not `ctrls`**
    // (`svt_init_mv_cost_params`, product_coding_loop.c:1906:
    // `ctx->md_subpel_me_ctrls.skip_diag_refinement >= 3`). So a PME
    // call, which passes `md_subpel_pme_ctrls` as `ctrls`, still takes
    // its MV-cost TYPE from the ME controls. That is why this is a
    // separate parameter instead of `ctrls.skip_diag_refinement`.
    let mv_cost_params = super::pme::init_mv_cost_params(
        ref_mv,
        geom.sq_size,
        me_skip_diag_refinement,
        full_lambda,
        mv_cost_tables,
    );

    let var_params = SubpelSearchVarParams {
        src,
        src_base,
        src_stride,
        ref_alloc,
        ref_base,
        ref_stride,
        w: geom.bwidth,
        h: geom.bheight,
        bias_fp: ctrls.bias_fp,
        subpel_search_type: i32::from(ctrls.subpel_search_type),
    };

    // C `best_mv = me_mv >> 3` then `get_mv_from_fullmv` — a TRUNCATING
    // round trip, see the doc note above.
    let start_mv = Mv {
        x: (me_mv.x >> 3).wrapping_mul(8),
        y: (me_mv.y >> 3).wrapping_mul(8),
    };

    let (besterr, st) = if ctrls.subpel_search_method
        == crate::port_enc_mode_config::encdec::subpel_search_method::SUBPEL_TREE
    {
        find_best_sub_pixel_tree(
            md_ctx.as_deref_mut(),
            &ms,
            &var_params,
            &mv_cost_params,
            start_mv,
            bsize,
        )
    } else {
        find_best_sub_pixel_tree_pruned(
            md_ctx.as_deref_mut(),
            &ms,
            &var_params,
            &mv_cost_params,
            start_mv,
            bsize,
        )
    };
    *me_mv = st.best_mv;
    // C hands the caller `*distortion` (the winner's raw prediction error)
    // alongside `bestmv`; the port's callers read it through `md_ctx`.
    if let Some(c) = md_ctx {
        c.final_distortion = st.distortion;
    }
    besterr
}

// ---------------------------------------------------------------------------
// build_single_ref_mvp_array (product_coding_loop.c:3097-3187)
// ---------------------------------------------------------------------------

/// C `MAX_MVP_CANIDATES` — the NEAREST plus up to `max_drl_index` NEARs.
pub const MAX_MVP_CANDIDATES: usize = 4;

/// C `build_single_ref_mvp_array` (product_coding_loop.c:3097-3187), the
/// MVP-list half.
///
/// `shut_fast_rate` short-circuits to a single ZERO MVP — not to the
/// stack's nearest, which is what a "skip the rate" reading would
/// suggest.
///
/// Each MVP is rounded to full pel with [`round_to_full_pel`] and then
/// clipped to the padded reference BEFORE the dedup, so two stack entries
/// that clip to the same position collapse to one.
#[allow(clippy::too_many_arguments)]
pub fn build_single_ref_mvp_list(
    shut_fast_rate: bool,
    stack_this_mv: &[Mv],
    ref_mv_count: u8,
    blk_org_x: i32,
    blk_org_y: i32,
    bwidth: i32,
    bheight: i32,
    r: &RefPicGeom,
) -> Vec<Mv> {
    if shut_fast_rate {
        return vec![Mv::ZERO];
    }
    let mut out: Vec<Mv> = Vec::with_capacity(MAX_MVP_CANDIDATES);

    let mut nearest = Mv {
        x: round_to_full_pel(stack_this_mv[0].x),
        y: round_to_full_pel(stack_this_mv[0].y),
    };
    clip_mv_on_pic_boundary(
        blk_org_x,
        blk_org_y,
        bwidth,
        bheight,
        r.max_width,
        r.max_height,
        r.border,
        &mut nearest.x,
        &mut nearest.y,
    );
    out.push(nearest);

    let max_drl_index = get_max_drl_index(ref_mv_count, PredictionMode::NearMv);
    for drli in 0..usize::from(max_drl_index) {
        let src = stack_this_mv[1 + drli];
        let mut nearmv = Mv {
            x: round_to_full_pel(src.x),
            y: round_to_full_pel(src.y),
        };
        clip_mv_on_pic_boundary(
            blk_org_x,
            blk_org_y,
            bwidth,
            bheight,
            r.max_width,
            r.max_height,
            r.border,
            &mut nearmv.x,
            &mut nearmv.y,
        );
        if !out.iter().any(|m| m.as_int() == nearmv.as_int()) {
            out.push(nearmv);
        }
    }
    out
}

/// The second half of `build_single_ref_mvp_array`
/// (product_coding_loop.c:3169-3186): the best MVP by variance.
///
/// Ties go to the EARLIER index (`<`, not `<=`), which is why the list's
/// order matters.
pub fn best_mvp_by_distortion(
    mvps: &[Mv],
    dist: &mut impl DistortionSource,
    blk_org_x: i32,
    blk_org_y: i32,
    ref_stride: usize,
    input_origin_index: usize,
) -> (usize, u32) {
    let mut best_idx = 0usize;
    let mut best_cost = u32::MAX;
    for (i, m) in mvps.iter().enumerate() {
        let idx = (blk_org_x + (i32::from(m.x) >> 3))
            + (blk_org_y + (i32::from(m.y) >> 3)) * ref_stride as i32;
        let cost = dist.variance(idx, input_origin_index);
        if cost < best_cost {
            best_idx = i;
            best_cost = cost;
        }
    }
    (best_idx, best_cost)
}

// ---------------------------------------------------------------------------
// pme_search (product_coding_loop.c:3197-3372)
// ---------------------------------------------------------------------------

/// C `MdPmeCtrls`, the fields `pme_search` reads.
#[derive(Debug, Clone, Copy, Default)]
pub struct MdPmeCtrls {
    pub enabled: bool,
    pub full_pel_search_width: u8,
    pub full_pel_search_height: u8,
    pub sa_q_weight: bool,
    pub enable_psad: bool,
    /// C `early_check_mv_th_multiplier`; `MIN_SIGNED_VALUE` (i16::MIN
    /// here) turns the early check OFF.
    pub early_check_mv_th_multiplier: i32,
    pub pre_fp_pme_to_me_mv_th: i32,
    pub pre_fp_pme_to_me_cost_th: i64,
    pub post_fp_pme_to_me_mv_th: i32,
    pub post_fp_pme_to_me_cost_th: i64,
}

/// C's `MIN_SIGNED_VALUE` as `pme_search` compares it.
pub const PME_EARLY_CHECK_OFF: i32 = i32::MIN;

/// C's qp-scaled PME search extents (product_coding_loop.c:3203-3211).
///
/// **The floor is 3, applied AFTER the rounding division** — a
/// `q_weight` small enough to zero the extent still leaves a 3-wide
/// search, not a skipped one.
pub fn pme_search_extents(
    full_pel_search_width: u8,
    full_pel_search_height: u8,
    sa_q_weight: bool,
    q_weight: u32,
    q_weight_denom: u32,
) -> (u8, u8) {
    if !sa_q_weight {
        return (full_pel_search_width, full_pel_search_height);
    }
    let dr = |v: u8| -> u8 {
        let n = u32::from(v) * q_weight;
        let d = q_weight_denom.max(1);
        let scaled = (n + (d >> 1)) / d;
        scaled.max(3) as u8
    };
    (dr(full_pel_search_width), dr(full_pel_search_height))
}

/// C's early MVP-vs-ME direction check (product_coding_loop.c:3255-3283).
///
/// Returns `true` when the ME MV points the OTHER WAY from at least one
/// sufficiently-large MVP component, which is the condition for running
/// the PME search at all. The threshold is
/// `((width * height) >> 17) * multiplier / 10` — a per-resolution
/// figure, and the sign test is a PRODUCT (`me * mvp < 0`), so a zero ME
/// component never counts as different.
pub fn pme_me_mv_differs_from_mvps(
    mvps: &[Mv],
    fp_me_mv: Mv,
    pic_width: u32,
    pic_height: u32,
    multiplier: i32,
) -> bool {
    let mv_th = ((((pic_width * pic_height) >> 17) as i64) * i64::from(multiplier)) / 10;
    for mvp in mvps {
        if i64::from(mvp.x.abs()) > mv_th && (i32::from(fp_me_mv.x) * i32::from(mvp.x)) < 0 {
            return true;
        }
        if i64::from(mvp.y.abs()) > mv_th && (i32::from(fp_me_mv.y) * i32::from(mvp.y)) < 0 {
            return true;
        }
    }
    false
}

/// C's PME-vs-ME cost deviation (product_coding_loop.c:3295-3296 and
/// :3336-3337): `((MAX(a,1) - MAX(b,1)) * 100) / MAX(b,1)`.
#[inline]
pub fn pme_to_me_cost_dev(pme_cost: u32, me_cost: u32) -> i64 {
    let a = i64::from(pme_cost.max(1));
    let b = i64::from(me_cost.max(1));
    ((a - b) * 100) / b
}

/// C's "close enough to the ME MV, or far enough in cost" bail
/// (product_coding_loop.c:3297-3305 in the PRE-full-pel form and
/// :3338-3346 in the POST one). Both arms share this shape; the
/// thresholds differ.
#[inline]
pub fn pme_bails_to_me(
    fp_me_mv: Mv,
    candidate: Mv,
    mv_th: i32,
    cost_dev: i64,
    cost_th: i64,
) -> bool {
    let dx = (i32::from(fp_me_mv.x) - i32::from(candidate.x)).abs();
    let dy = (i32::from(fp_me_mv.y) - i32::from(candidate.y)).abs();
    (dx <= mv_th && dy <= mv_th) || cost_dev >= cost_th
}

// ---------------------------------------------------------------------------
// read_refine_me_mvs (product_coding_loop.c:2815-2936)
// ---------------------------------------------------------------------------

/// C's ME-MV centre selection inside `read_refine_me_mvs`
/// (product_coding_loop.c:2851-2869).
///
/// An NSQ block (or a 4x4 whose parent was tested) INHERITS the square
/// block's MV, rounded to full pel; everything else reads the raw ME
/// array and multiplies by 8. **BLOCK_64X128 and BLOCK_128X64 are
/// excluded from the inheritance** because their second halves do not
/// share ME results with the 128x128 parent.
///
/// `bsize_is_64x128_or_128x64` and `parent_tested` are the two C
/// conditions the caller resolves; keeping them as parameters is what
/// makes the exclusion visible instead of buried in a geometry lookup.
#[allow(clippy::too_many_arguments)]
pub fn me_mv_center(
    blk_avail_sqi: bool,
    bwidth: u16,
    bheight: u16,
    bsize_is_64x128_or_128x64: bool,
    bsize_is_4x4: bool,
    parent_tested: bool,
    sq_sb_me_mv: Mv,
    raw_me_mv_full_pel: Mv,
) -> Mv {
    let b_w_ne_h = bwidth != bheight;
    if (blk_avail_sqi && b_w_ne_h && !bsize_is_64x128_or_128x64) || (bsize_is_4x4 && parent_tested)
    {
        Mv {
            x: round_to_full_pel(sq_sb_me_mv.x),
            y: round_to_full_pel(sq_sb_me_mv.y),
        }
    } else {
        Mv {
            x: raw_me_mv_full_pel.x.wrapping_mul(8),
            y: raw_me_mv_full_pel.y.wrapping_mul(8),
        }
    }
}

// ---------------------------------------------------------------------------
// read_refine_me_mvs (product_coding_loop.c:2815-2936)
// ---------------------------------------------------------------------------

/// Everything C's `read_refine_me_mvs` reads out of `ModeDecisionContext`
/// and the block geometry for ONE `(list_idx, ref_idx)` pair.
#[derive(Debug, Clone, Copy)]
pub struct RefineMeIn {
    /// C `pc_tree->tested_blk[PART_N][0]`.
    pub blk_avail_sqi: bool,
    /// C `ctx->blk_geom->bsize == BLOCK_64X128 || == BLOCK_128X64`.
    pub bsize_is_64x128_or_128x64: bool,
    /// C `ctx->blk_geom->bsize == BLOCK_4X4`.
    pub bsize_is_4x4: bool,
    /// C `pc_tree->parent->tested_blk[PART_N][0]`.
    pub parent_tested: bool,
    /// C `ctx->sq_sb_me_mv[list][ref]` — the SQUARE block's MV, which an
    /// NSQ shape inherits.
    pub sq_sb_me_mv: Mv,
    /// C `me_results->me_mv_array[...]` for this pair, in FULL PEL.
    pub raw_me_mv_full_pel: Mv,
    /// C `ctx->md_nsq_me_ctrls.enabled`.
    pub md_nsq_me_enabled: bool,
    /// C `ctx->md_subpel_me_ctrls.enabled`.
    pub do_subpel: bool,
    /// C `ctx->md_subpel_me_ctrls.subpel_search_method ==
    /// SUBPEL_FIXED_STAGE_SEARCH`. **This body does not branch on it** —
    /// the caller does, when it builds the `subpel` closure, because the
    /// two searches take different buffers
    /// ([`md_subpel_search_fixed_stage`] wants a `DistortionSource`,
    /// [`md_subpel_search`] wants planes). It is carried so C's condition
    /// stays visible at the call site instead of being re-derived there.
    pub subpel_fixed_stage: bool,
    /// C `ctx->updated_enable_pme || ctx->ref_pruning_ctrls.enabled` — the
    /// only reason the full-pel ME cost is computed when subpel is OFF.
    pub needs_fp_me_dist: bool,
    /// C `ctx->shape == PART_N`.
    pub shape_is_part_n: bool,
}

/// What C's `read_refine_me_mvs` writes for one `(list_idx, ref_idx)`.
#[derive(Debug, Clone, Copy, Default)]
pub struct RefineMeOut {
    /// C `ctx->fp_me_mv[list][ref]`.
    pub fp_me_mv: Mv,
    /// C `ctx->sub_me_mv[list][ref]`.
    pub sub_me_mv: Mv,
    /// C `ctx->sb_me_mv[list][ref]` — the same MV, clipped AGAIN.
    pub sb_me_mv: Mv,
    /// C `ctx->sq_sb_me_mv[list][ref]`, written only on a square shape.
    pub sq_sb_me_mv: Option<Mv>,
    /// C `ctx->post_subpel_me_mv_cost[list][ref]`; `u32::MAX` when subpel
    /// did not run, which is C's `(int32_t)~0` initialisation.
    pub post_subpel_me_mv_cost: u32,
    /// C `ctx->fp_me_dist[list][ref]`, written ONLY on the
    /// no-subpel-but-someone-needs-it arm.
    pub fp_me_dist: Option<u32>,
}

/// C `read_refine_me_mvs`' per-reference body (product_coding_loop.c:2851-2934).
///
/// The enclosing loop is over `ref_frame_type_arr`, skipping compound
/// entries and pairs with no ME data (`svt_aom_is_me_data_present`); the
/// caller owns that loop because each iteration needs a DIFFERENT reference
/// picture and a different MVP stack, neither of which this module models.
///
/// **Caller contract for `ref_mv`.** Between the clip and the searches C sets
/// `ctx->ref_mv` from
/// `svt_aom_choose_best_av1_mv_pred(ref_pair, NEWMV, me_mv, 0)`, and that
/// is the MV every cost in both searches is measured against. There is no
/// `ref_mv` parameter here because it reaches the searches through
/// `mv_cost_params.ref_mv` — the caller MUST build `mv_cost_params` (and
/// the `subpel` closure's own params) from that value, not from the ME MV.
/// The clip runs FIRST, so `choose_best_av1_mv_pred` sees the clipped
/// centre.
///
/// Four things transcribed that a rewrite loses:
///
/// * **The centre MV is clipped BEFORE `choose_best_av1_mv_pred`, and the
///   result is clipped AGAIN into `sb_me_mv` afterwards** — `sub_me_mv`
///   keeps the UNCLIPPED post-search value while `sb_me_mv` gets the
///   clipped one, so the two can differ.
/// * **`md_sq_motion_search` is DEAD.** C guards it with
///   `ctx->md_sq_me_ctrls.enabled`, and `pcs->md_sq_mv_search_level` is 0
///   unconditionally at all three of its derivation sites
///   (`enc_mode_config.c:9200`, `:9753`, `:10033`). The square arm of C's
///   `if (b_w_ne_h) { nsq } else if (md_sq_me_enabled) { sq }` therefore
///   never runs, and this port has no parameter for it.
/// * **`post_subpel_me_mv_cost` is set to `~0` BEFORE the subpel search**,
///   so a block that skips subpel leaves the sentinel rather than a stale
///   cost.
/// * **The full-pel ME cost is computed only when subpel is off AND
///   somebody needs it** (`updated_enable_pme || ref_pruning_ctrls.enabled`)
///   — it is not a fallback, it is a conditional side-effect.
///
/// Evidence tier 4 — `read_refine_me_mvs` is `static` with no exported
/// symbol. Every leaf it calls is already ported: [`me_mv_center`],
/// [`clip_mv_on_pic_boundary`], [`md_nsq_motion_search`],
/// [`md_subpel_search`], [`md_subpel_search_fixed_stage`] and
/// [`super::pme::fp_mv_err_cost`].
#[allow(clippy::too_many_arguments)]
pub fn refine_me_mv_for_ref(
    inp: RefineMeIn,
    fp_ctx: &FullPelCtx,
    r: &RefPicGeom,
    nsq: Option<(DistortionType, u8, u8, &[Mv])>,
    subpel: Option<&mut dyn FnMut(&mut Mv) -> u32>,
    fp_dist: Option<&mut dyn FnMut(Mv) -> u32>,
    dist: &mut impl DistortionSource,
    mv_cost_params: &MvCostParams<'_>,
    input_origin_index: usize,
) -> RefineMeOut {
    let mut me_mv = me_mv_center(
        inp.blk_avail_sqi,
        fp_ctx.bwidth as u16,
        fp_ctx.bheight as u16,
        inp.bsize_is_64x128_or_128x64,
        inp.bsize_is_4x4,
        inp.parent_tested,
        inp.sq_sb_me_mv,
        inp.raw_me_mv_full_pel,
    );
    clip_mv_on_pic_boundary(
        fp_ctx.blk_org_x,
        fp_ctx.blk_org_y,
        fp_ctx.bwidth,
        fp_ctx.bheight,
        r.max_width,
        r.max_height,
        r.border,
        &mut me_mv.x,
        &mut me_mv.y,
    );
    let b_w_ne_h = fp_ctx.bwidth != fp_ctx.bheight;
    if b_w_ne_h
        && inp.md_nsq_me_enabled
        && let Some((dist_type, w, h, sub_block_mvs)) = nsq
    {
        md_nsq_motion_search(
            fp_ctx,
            r,
            dist,
            mv_cost_params,
            input_origin_index,
            dist_type,
            w,
            h,
            me_mv,
            sub_block_mvs,
            &mut me_mv,
        );
    }

    let mut out = RefineMeOut {
        // C `(int32_t)~0`.
        post_subpel_me_mv_cost: u32::MAX,
        fp_me_mv: me_mv,
        ..RefineMeOut::default()
    };

    if inp.do_subpel {
        if let Some(f) = subpel {
            out.post_subpel_me_mv_cost = f(&mut me_mv);
        }
    } else if inp.needs_fp_me_dist
        && let Some(f) = fp_dist
    {
        out.fp_me_dist = Some(f(me_mv));
    }

    out.sub_me_mv = me_mv;
    let mut sb = me_mv;
    clip_mv_on_pic_boundary(
        fp_ctx.blk_org_x,
        fp_ctx.blk_org_y,
        fp_ctx.bwidth,
        fp_ctx.bheight,
        r.max_width,
        r.max_height,
        r.border,
        &mut sb.x,
        &mut sb.y,
    );
    out.sb_me_mv = sb;
    if inp.shape_is_part_n {
        out.sq_sb_me_mv = Some(sb);
    }
    out
}

// ---------------------------------------------------------------------------
// pme_search (product_coding_loop.c:3197-3372)
// ---------------------------------------------------------------------------

/// What C's `pme_search` writes for one `(list_idx, ref_idx)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PmeSearchOut {
    /// C `ctx->valid_pme_mv[list][ref]`.
    pub valid: bool,
    /// C `ctx->best_pme_mv[list][ref]`, EIGHTH PEL.
    pub best_pme_mv: Mv,
    /// C `ctx->pme_res[list][ref].dist`.
    pub dist: u32,
    /// Which of C's four exits produced the result. Not a C field — C
    /// distinguishes them only by control flow, and a caller (or a test)
    /// that cannot tell "PME ran" from "PME handed back the ME MV" cannot
    /// check the thing that matters.
    pub exit: PmeExit,
}

/// Where [`pme_search_for_ref`] left, in C's own order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PmeExit {
    /// `continue` before anything ran: `pme_ref0_only` skipped a farther
    /// reference, or `svt_aom_is_valid_unipred_ref` refused it. C leaves
    /// `valid_pme_mv` at the 0 it just wrote.
    Skipped,
    /// The early MVP-vs-ME direction check found the ME MV agrees with
    /// every MVP, so C adopts the ME MV (`:3305-3310`).
    EarlyMvpCheck,
    /// The PRE-full-pel deviation check bailed to the ME MV (`:3324-3330`).
    PreFullPel,
    /// The POST-full-pel deviation check bailed to the ME MV (`:3348-3354`).
    PostFullPel,
    /// The search ran to the end and its own MV was taken.
    Searched,
}

/// C `pme_search`'s per-reference body (product_coding_loop.c:3216-3364),
/// single-reference arm.
///
/// As with [`refine_me_mv_for_ref`], the loop over `ref_frame_type_arr` is
/// the caller's — each iteration needs a different reference picture, MVP
/// list and MVP stack. C's compound arm is not here because
/// `pme_search`'s `rf[1] == NONE_FRAME` guard means a compound entry does
/// nothing at all.
///
/// **Three of the four exits hand back the ME MV, not a searched one**, and
/// they are the common case on a well-behaved GOP — which is exactly why
/// [`PmeSearchOut::exit`] exists: `valid_pme_mv = 1` says nothing about
/// whether a search happened, and a caller that reads only `best_pme_mv`
/// cannot tell a PME result from an ME echo.
///
/// Details transcribed:
///
/// * **All three bail-outs write the ME values, not the MVP** —
///   `dist = post_subpel_me_mv_cost`, `best_pme_mv = sub_me_mv`.
/// * **`me_mv_cost` is `fp_me_dist`, the FULL-PEL cost**, while the value
///   the bail-outs store is the SUB-pel one. C mixes the two on purpose.
/// * **The full-pel search is centred on the best MVP, not on the ME MV**,
///   and its window is `±(full_pel_search_{width,height} >> 1)` at step 1
///   — the qp modulation of those extents is [`pme_search_extents`], which
///   the caller applies.
/// * **`ctx->ref_mv` is set from `choose_best_av1_mv_pred(best_mvp)`**
///   before the search, so the MV rate inside the search is measured
///   against the MVP — the caller must build `mv_cost_params` from that,
///   the same contract [`refine_me_mv_for_ref`] documents.
///
/// Evidence tier 4 — `pme_search` is `static` with no exported symbol.
#[allow(clippy::too_many_arguments)]
pub fn pme_search_for_ref(
    ctrls: &MdPmeCtrls,
    fp_ctx: &FullPelCtx,
    r: &RefPicGeom,
    dist: &mut impl DistortionSource,
    mv_cost_params: &MvCostParams<'_>,
    input_origin_index: usize,
    // C `ctx->md_pme_ctrls.dist_type`. `MdPmeCtrls` here carries the
    // fields the PREDICATES read; the distortion metric is a parameter so
    // this body does not need a second copy of that table.
    dist_type: DistortionType,
    /* C's two `continue`-before-anything gates, already evaluated: */
    skipped: bool,
    me_data_present: bool,
    /* the ME state `read_refine_me_mvs` left: */
    fp_me_mv: Mv,
    sub_me_mv: Mv,
    fp_me_dist: u32,
    post_subpel_me_mv_cost: u32,
    /* the MVP list and the qp-modulated extents: */
    mvps: &[Mv],
    full_pel_search_width: u8,
    full_pel_search_height: u8,
    pic_width: u32,
    pic_height: u32,
    subpel: Option<&mut dyn FnMut(&mut Mv) -> u32>,
) -> PmeSearchOut {
    let bail = |exit: PmeExit| PmeSearchOut {
        valid: true,
        best_pme_mv: sub_me_mv,
        dist: post_subpel_me_mv_cost,
        exit,
    };
    if skipped {
        return PmeSearchOut {
            valid: false,
            best_pme_mv: Mv::ZERO,
            dist: u32::MAX,
            exit: PmeExit::Skipped,
        };
    }

    let mut me_mv_cost = u32::MAX;
    if me_data_present {
        if ctrls.early_check_mv_th_multiplier != PME_EARLY_CHECK_OFF
            && !pme_me_mv_differs_from_mvps(
                mvps,
                fp_me_mv,
                pic_width,
                pic_height,
                ctrls.early_check_mv_th_multiplier,
            )
        {
            return bail(PmeExit::EarlyMvpCheck);
        }
        me_mv_cost = fp_me_dist;
    }

    // Step 1: the best MVP by DISTORTION (C `:3316-3317`).
    let (best_idx, best_mvp_cost) = best_mvp_by_distortion(
        mvps,
        dist,
        fp_ctx.blk_org_x,
        fp_ctx.blk_org_y,
        r.y_stride,
        input_origin_index,
    );
    let best_mvp = mvps[best_idx];

    if me_data_present
        && pme_bails_to_me(
            fp_me_mv,
            best_mvp,
            ctrls.pre_fp_pme_to_me_mv_th,
            pme_to_me_cost_dev(best_mvp_cost, me_mv_cost),
            ctrls.pre_fp_pme_to_me_cost_th,
        )
    {
        return bail(PmeExit::PreFullPel);
    }

    let mut search = PmeBest {
        cost: u32::MAX,
        mvx: 0,
        mvy: 0,
    };
    let mut psad_ctx = *fp_ctx;
    psad_ctx.enable_psad = ctrls.enable_psad;
    md_full_pel_search(
        &psad_ctx,
        r,
        dist,
        mv_cost_params,
        input_origin_index,
        dist_type,
        best_mvp.x,
        best_mvp.y,
        SearchWindow {
            start_x: -(i32::from(full_pel_search_width) >> 1),
            end_x: i32::from(full_pel_search_width) >> 1,
            start_y: -(i32::from(full_pel_search_height) >> 1),
            end_y: i32::from(full_pel_search_height) >> 1,
            sparse_search_step: 1,
            is_sprs_lev0_performed: false,
        },
        &mut search,
    );
    let mut best_search_mv = Mv {
        x: search.mvx,
        y: search.mvy,
    };

    if me_data_present
        && pme_bails_to_me(
            fp_me_mv,
            best_search_mv,
            ctrls.post_fp_pme_to_me_mv_th,
            pme_to_me_cost_dev(search.cost, me_mv_cost),
            ctrls.post_fp_pme_to_me_cost_th,
        )
    {
        return bail(PmeExit::PostFullPel);
    }

    // C leaves `post_subpel_pme_mv_cost` at `~0` when the subpel search is
    // disabled, and stores THAT into `pme_res.dist`.
    let mut d = u32::MAX;
    if let Some(f) = subpel {
        d = f(&mut best_search_mv);
    }
    PmeSearchOut {
        valid: true,
        best_pme_mv: best_search_mv,
        dist: d,
        exit: PmeExit::Searched,
    }
}

// ---------------------------------------------------------------------------
// TIER 4 — every C function ported here is `static` with no exported
// symbol (`nm -g`). These are hand-derived vectors traced against the C
// source. The pieces WITH an oracle (pme_sad_loop_kernel,
// fp_mv_err_cost, get_max_drl_index, clip_mv_on_pic_boundary) are called,
// not re-transcribed, so their tier-1 coverage carries through.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
