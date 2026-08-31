//! The MD-level motion searches of `Source/Lib/Codec/product_coding_loop.c`.
//!
//! | this module | C |
//! |---|---|
//! | [`md_full_pel_search`] | `:1914-2049` |
//! | [`md_full_pel_search_large_lbd`] | `:1830-1912` |
//! | [`md_sq_motion_search`] | `:2329-2510` |
//! | [`md_nsq_motion_search`] | `:2080-2252` |
//! | [`md_subpel_search_fixed_stage`] | `:2634-2731` |
//! | [`subpel_mv_limits`] | `:2547-2557` (the `md_subpel_search` limit derivation) |
//! | [`build_single_ref_mvp_array`] | `:3097-3187` |
//! | [`pme_search`] | `:3197-3372` |
//! | [`read_refine_me_mvs`] | `:2815-2936` |
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
        let mut acc = 0u32;
        for y in 0..self.bheight {
            for x in 0..self.bwidth {
                let s = self.src[input_origin_index + y * self.src_stride + x];
                let rr = self.ref_plane[base + y * self.ref_stride + x];
                acc += u32::from(s.abs_diff(rr));
            }
        }
        acc
    }
}

impl DistortionSource for PlaneDistortion<'_> {
    fn variance(&mut self, ref_origin_index: i32, input_origin_index: usize) -> u32 {
        // C `aom_variance<W>x<H>_c`: sum of squares minus (sum^2 / n).
        let base = self.ref_at(ref_origin_index);
        let mut sum: i64 = 0;
        let mut sse: i64 = 0;
        for y in 0..self.bheight {
            for x in 0..self.bwidth {
                let s = i64::from(self.src[input_origin_index + y * self.src_stride + x]);
                let rr = i64::from(self.ref_plane[base + y * self.ref_stride + x]);
                let d = rr - s;
                sum += d;
                sse += d * d;
            }
        }
        let n = (self.bwidth * self.bheight) as i64;
        (sse - (sum * sum) / n) as u32
    }

    fn subpel_variance(
        &mut self,
        _ref_origin_index: i32,
        _subx: i32,
        _suby: i32,
        _input_origin_index: usize,
    ) -> u32 {
        // The sub-pel variance needs the AV1 bilinear filters, which live
        // in the DSP layer; this plain-plane implementation deliberately
        // does not fake them.
        unimplemented!("subpel_variance requires the interpolation filters")
    }

    fn sad(&mut self, ref_origin_index: i32, input_origin_index: usize) -> u32 {
        self.sum_abs_diff(ref_origin_index, input_origin_index)
    }

    fn ssd(&mut self, ref_origin_index: i32, input_origin_index: usize) -> u32 {
        let base = self.ref_at(ref_origin_index);
        let mut acc: u64 = 0;
        for y in 0..self.bheight {
            for x in 0..self.bwidth {
                let s = i64::from(self.src[input_origin_index + y * self.src_stride + x]);
                let rr = i64::from(self.ref_plane[base + y * self.ref_stride + x]);
                acc += ((rr - s) * (rr - s)) as u64;
            }
        }
        acc as u32
    }

    fn variance_vs_flat(&mut self, _ref_origin_index: i32) -> u32 {
        unimplemented!("variance_vs_flat requires svt_aom_eb_av1_var_offs")
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
// TIER 4 — every C function ported here is `static` with no exported
// symbol (`nm -g`). These are hand-derived vectors traced against the C
// source. The pieces WITH an oracle (pme_sad_loop_kernel,
// fp_mv_err_cost, get_max_drl_index, clip_mv_on_pic_boundary) are called,
// not re-transcribed, so their tier-1 coverage carries through.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::port_md::pme::{MvCostTable, MvCostType};

    fn mv(x: i16, y: i16) -> Mv {
        Mv { x, y }
    }

    fn zero_cost() -> MvCostTable {
        MvCostTable {
            joint: [0; 4],
            comp: [
                vec![0i32; crate::port_md::pme::MV_VALS],
                vec![0i32; crate::port_md::pme::MV_VALS],
            ],
        }
    }

    fn params(t: &MvCostTable) -> MvCostParams<'_> {
        MvCostParams {
            ref_mv: Mv::ZERO,
            full_ref_mv: Mv::ZERO,
            mv_cost_type: MvCostType::None,
            mv_cost_tables: Some(t),
            error_per_bit: 0,
            early_exit_th: 0,
            sad_per_bit: 0,
        }
    }

    /// A distortion that is a pure function of the reference index, so a
    /// test can state exactly which positions were visited and which one
    /// won without needing pixels.
    struct Probe {
        /// Every `ref_origin_index` the search asked about, in order.
        visited: Vec<i32>,
        /// The index whose cost is 0; everything else costs 1000.
        target: i32,
        /// Feeds `subpel_variance` / `variance_vs_flat`.
        subpel: Vec<((i32, i32, i32), u32)>,
        flat: u32,
    }

    impl Probe {
        fn new(target: i32) -> Self {
            Self {
                visited: Vec::new(),
                target,
                subpel: Vec::new(),
                flat: 0,
            }
        }
        fn cost(&mut self, idx: i32) -> u32 {
            self.visited.push(idx);
            if idx == self.target { 0 } else { 1000 }
        }
    }

    impl PmeSadLoop for Probe {
        fn run_pme_sad_loop(
            &mut self,
            _p: &MvCostParams<'_>,
            _input_origin_index: usize,
            ref_origin_index: i32,
            search_area_width: i32,
            search_area_height: i32,
            _start_x: i32,
            _start_y: i32,
            _search_step: i32,
            _mvx: i16,
            _mvy: i16,
            _best: &mut PmeBest,
        ) {
            // Record that the 8-aligned half ran, and with what extent.
            self.visited.push(-1_000_000 - ref_origin_index);
            self.visited.push(search_area_width);
            self.visited.push(search_area_height);
        }
    }

    impl DistortionSource for Probe {
        fn variance(&mut self, ref_origin_index: i32, _i: usize) -> u32 {
            self.cost(ref_origin_index)
        }
        fn subpel_variance(&mut self, idx: i32, subx: i32, suby: i32, _i: usize) -> u32 {
            self.visited.push(idx);
            for &(k, v) in &self.subpel {
                if k == (idx, subx, suby) {
                    return v;
                }
            }
            1000
        }
        fn sad(&mut self, ref_origin_index: i32, _i: usize) -> u32 {
            self.cost(ref_origin_index)
        }
        fn ssd(&mut self, ref_origin_index: i32, _i: usize) -> u32 {
            self.cost(ref_origin_index)
        }
        fn variance_vs_flat(&mut self, _idx: i32) -> u32 {
            self.flat
        }
    }

    fn fp_ctx() -> FullPelCtx {
        FullPelCtx {
            blk_org_x: 32,
            blk_org_y: 32,
            bwidth: 16,
            bheight: 16,
            enable_psad: false,
            hbd_md: false,
            sprs_lev0_start_x: 0,
            sprs_lev0_end_x: 0,
            sprs_lev0_start_y: 0,
            sprs_lev0_end_y: 0,
        }
    }

    fn geom() -> RefPicGeom {
        RefPicGeom {
            border: 64,
            max_width: 320,
            max_height: 240,
            y_stride: 448,
        }
    }

    /// TIER 4 — the search-area clamp is asymmetric: the start sides drop
    /// only the origin plus the MV, the end sides also drop the block
    /// dimension.
    #[test]
    fn tier4_clamp_search_window_is_asymmetric() {
        let ctx = fp_ctx();
        let r = geom();
        let mut w = SearchWindow {
            start_x: -1000,
            end_x: 1000,
            start_y: -1000,
            end_y: 1000,
            sparse_search_step: 1,
            is_sprs_lev0_performed: false,
        };
        clamp_search_window(&ctx, &r, 0, 0, &mut w);
        assert_eq!(w.start_x, (-r.border + 1) - ctx.blk_org_x);
        assert_eq!(
            w.end_x,
            (r.border + r.max_width - 1) - (ctx.blk_org_x + ctx.bwidth)
        );
        assert_eq!(w.start_y, (-r.border + 1) - ctx.blk_org_y);
        assert_eq!(
            w.end_y,
            (r.border + r.max_height - 1) - (ctx.blk_org_y + ctx.bheight)
        );

        // The MV shifts the whole window, in FULL-pel units (>> 3).
        let mut w2 = SearchWindow {
            start_x: -1000,
            end_x: 0,
            start_y: 0,
            end_y: 0,
            sparse_search_step: 1,
            is_sprs_lev0_performed: false,
        };
        clamp_search_window(&ctx, &r, 80, 0, &mut w2);
        assert_eq!(w2.start_x, (-r.border + 1) - (ctx.blk_org_x + 10));

        // A window already inside is untouched.
        let mut w3 = SearchWindow {
            start_x: -2,
            end_x: 2,
            start_y: -2,
            end_y: 2,
            sparse_search_step: 1,
            is_sprs_lev0_performed: false,
        };
        clamp_search_window(&ctx, &r, 0, 0, &mut w3);
        assert_eq!((w3.start_x, w3.end_x, w3.start_y, w3.end_y), (-2, 2, -2, 2));
    }

    /// TIER 4 — the scan order is x-OUTER, y-INNER, and both bounds are
    /// INCLUSIVE.
    #[test]
    fn tier4_md_full_pel_search_scan_order_and_inclusive_bounds() {
        let ctx = fp_ctx();
        let r = geom();
        let t = zero_cost();
        let p = params(&t);
        let mut probe = Probe::new(i32::MIN);
        let mut best = PmeBest {
            cost: u32::MAX,
            mvx: 0,
            mvy: 0,
        };
        md_full_pel_search(
            &ctx,
            &r,
            &mut probe,
            &p,
            0,
            DistortionType::Sad,
            0,
            0,
            SearchWindow {
                start_x: -1,
                end_x: 1,
                start_y: -1,
                end_y: 1,
                sparse_search_step: 1,
                is_sprs_lev0_performed: false,
            },
            &mut best,
        );
        // 3x3 inclusive = 9 positions.
        assert_eq!(probe.visited.len(), 9);
        let base = ctx.blk_org_x + ctx.blk_org_y * r.y_stride as i32;
        let at = |dx: i32, dy: i32| base + dx + dy * r.y_stride as i32;
        // x outer, y inner: (-1,-1), (-1,0), (-1,1), (0,-1), ...
        assert_eq!(probe.visited[0], at(-1, -1));
        assert_eq!(probe.visited[1], at(-1, 0));
        assert_eq!(probe.visited[2], at(-1, 1));
        assert_eq!(probe.visited[3], at(0, -1));
    }

    /// TIER 4 — the winning MV is the search offset x8 added to the
    /// centre, and a tie keeps the EARLIER position (`<`, not `<=`).
    #[test]
    fn tier4_md_full_pel_search_best_mv_and_tie_break() {
        let ctx = fp_ctx();
        let r = geom();
        let t = zero_cost();
        let p = params(&t);
        // Centre MV (16, -8) is full-pel (2, -1); the winner sits at
        // search offset (+1, +1) from that centre.
        let target = (ctx.blk_org_x + 2 + 1) + (ctx.blk_org_y - 1 + 1) * r.y_stride as i32;
        let mut probe = Probe::new(target);
        let mut best = PmeBest {
            cost: u32::MAX,
            mvx: 0,
            mvy: 0,
        };
        md_full_pel_search(
            &ctx,
            &r,
            &mut probe,
            &p,
            0,
            DistortionType::Var,
            16,
            -8,
            SearchWindow {
                start_x: -1,
                end_x: 1,
                start_y: -1,
                end_y: 1,
                sparse_search_step: 1,
                is_sprs_lev0_performed: false,
            },
            &mut best,
        );
        // The centre MV is (16, -8) = full-pel (2, -1); the winner is at
        // offset (+1, +1) relative to that centre.
        assert_eq!(best.cost, 0);
        assert_eq!(best.mvx, 16 + 8);
        assert_eq!(best.mvy, -8 + 8);

        // All-equal costs: the FIRST position scanned wins.
        let mut probe = Probe::new(i32::MIN);
        let mut best = PmeBest {
            cost: u32::MAX,
            mvx: 0,
            mvy: 0,
        };
        md_full_pel_search(
            &ctx,
            &r,
            &mut probe,
            &p,
            0,
            DistortionType::Var,
            0,
            0,
            SearchWindow {
                start_x: -1,
                end_x: 1,
                start_y: -1,
                end_y: 1,
                sparse_search_step: 1,
                is_sprs_lev0_performed: false,
            },
            &mut best,
        );
        assert_eq!((best.mvx, best.mvy), (-8, -8));
    }

    /// TIER 4 — the sparse level-1 skip fires only for
    /// `sparse_search_step == 2`, inside the recorded level-0 window, and
    /// only on positions that are multiples of 4 in BOTH axes.
    #[test]
    fn tier4_md_full_pel_search_sparse_level1_skip() {
        let mut ctx = fp_ctx();
        ctx.sprs_lev0_start_x = -8;
        ctx.sprs_lev0_end_x = 8;
        ctx.sprs_lev0_start_y = -8;
        ctx.sprs_lev0_end_y = 8;
        let r = geom();
        let t = zero_cost();
        let p = params(&t);
        let w = SearchWindow {
            start_x: -4,
            end_x: 4,
            start_y: -4,
            end_y: 4,
            sparse_search_step: 2,
            is_sprs_lev0_performed: true,
        };
        let mut probe = Probe::new(i32::MIN);
        let mut best = PmeBest {
            cost: u32::MAX,
            mvx: 0,
            mvy: 0,
        };
        md_full_pel_search(
            &ctx,
            &r,
            &mut probe,
            &p,
            0,
            DistortionType::Sad,
            0,
            0,
            w,
            &mut best,
        );
        // 5x5 lattice at step 2 = 25 positions; the 9 with both
        // coordinates in {-4, 0, 4} are skipped.
        assert_eq!(probe.visited.len(), 25 - 9);

        // Same window with the flag off: nothing is skipped.
        let mut probe = Probe::new(i32::MIN);
        let mut best = PmeBest {
            cost: u32::MAX,
            mvx: 0,
            mvy: 0,
        };
        let mut w2 = w;
        w2.is_sprs_lev0_performed = false;
        md_full_pel_search(
            &ctx,
            &r,
            &mut probe,
            &p,
            0,
            DistortionType::Sad,
            0,
            0,
            w2,
            &mut best,
        );
        assert_eq!(probe.visited.len(), 25);

        // Step 1 never skips, even with the flag on.
        let mut probe = Probe::new(i32::MIN);
        let mut best = PmeBest {
            cost: u32::MAX,
            mvx: 0,
            mvy: 0,
        };
        let mut w3 = w;
        w3.sparse_search_step = 1;
        md_full_pel_search(
            &ctx,
            &r,
            &mut probe,
            &p,
            0,
            DistortionType::Sad,
            0,
            0,
            w3,
            &mut best,
        );
        assert_eq!(probe.visited.len(), 81);
    }

    /// TIER 4 — the psad dispatch: SAD + enable_psad + 8-bit + a CLAMPED
    /// width >= 7. The threshold is 7, not 8.
    #[test]
    fn tier4_md_full_pel_search_psad_dispatch_threshold() {
        let mut ctx = fp_ctx();
        ctx.enable_psad = true;
        let r = geom();
        let t = zero_cost();
        let p = params(&t);
        let run = |w: i32, dist_type, hbd: bool| {
            let mut c = ctx;
            c.hbd_md = hbd;
            let mut probe = Probe::new(i32::MIN);
            let mut best = PmeBest {
                cost: u32::MAX,
                mvx: 0,
                mvy: 0,
            };
            md_full_pel_search(
                &c,
                &r,
                &mut probe,
                &p,
                0,
                dist_type,
                0,
                0,
                SearchWindow {
                    start_x: 0,
                    end_x: w,
                    start_y: 0,
                    end_y: 0,
                    sparse_search_step: 1,
                    is_sprs_lev0_performed: false,
                },
                &mut best,
            );
            // The Probe records a large negative marker when the
            // 8-aligned kernel ran.
            probe.visited.iter().any(|&v| v < -100_000)
        };
        assert!(!run(6, DistortionType::Sad, false), "width 6 stays generic");
        assert!(run(7, DistortionType::Sad, false), "width 7 takes mpsad");
        assert!(run(8, DistortionType::Sad, false));
        // Not SAD, or hbd, and it never dispatches.
        assert!(!run(16, DistortionType::Var, false));
        assert!(!run(16, DistortionType::Sad, true));
    }

    /// TIER 4 — the mpsad variant rounds the x extent UP to a multiple of
    /// 8, so it does NOT search the same positions as the generic path.
    #[test]
    fn tier4_large_lbd_rounds_x_extent_up_to_a_multiple_of_eight() {
        let mut ctx = fp_ctx();
        ctx.enable_psad = true;
        let r = geom();
        let t = zero_cost();
        let p = params(&t);
        let mut probe = Probe::new(i32::MIN);
        let mut best = PmeBest {
            cost: u32::MAX,
            mvx: 0,
            mvy: 0,
        };
        // Requested width 9 (start 0, end 9) -> 9 % 8 = 1, remain = 7,
        // rounded end 16, so search_area_width = 16 and the tail is empty.
        md_full_pel_search(
            &ctx,
            &r,
            &mut probe,
            &p,
            0,
            DistortionType::Sad,
            0,
            0,
            SearchWindow {
                start_x: 0,
                end_x: 9,
                start_y: 0,
                end_y: 0,
                sparse_search_step: 1,
                is_sprs_lev0_performed: false,
            },
            &mut best,
        );
        // Probe records [marker, width, height] for the 8-aligned half.
        let marker = probe.visited.iter().position(|&v| v < -100_000).unwrap();
        assert_eq!(probe.visited[marker + 1], 16, "x extent rounded up to 16");
        assert_eq!(probe.visited[marker + 2], 1, "height is end - start + 1");
        // Nothing else was scanned: the tail is empty when the rounded
        // width is already a multiple of 8.
        assert_eq!(probe.visited.len(), 3);
    }

    /// TIER 4 — `sparse_extent`'s cap applies to the pre-multiplier
    /// product and the half-extent is floored to the step.
    #[test]
    fn tier4_sparse_extent_cap_and_step_alignment() {
        // 20 * 3 * 2 = 120, capped at 64, * 100 / 100 = 64, half 32,
        // floored to a multiple of 8 -> 32.
        assert_eq!(sparse_extent(100, 20, 3, 2, 64, 8), 32);
        // The multiplier scales AFTER the cap.
        assert_eq!(sparse_extent(50, 20, 3, 2, 64, 8), 16);
        // The step floor bites: 30 / 2 = 15, floored to a multiple of 4.
        assert_eq!(sparse_extent(100, 30, 1, 1, 1000, 4), 12);
    }

    /// TIER 4 — the level-1 nudge pushes a 4-aligned bound OUT by 2.
    #[test]
    fn tier4_nudge_sprs_lev1() {
        assert_eq!(nudge_sprs_lev1(-8, 8), (-10, 10));
        assert_eq!(nudge_sprs_lev1(-6, 6), (-6, 6));
        assert_eq!(nudge_sprs_lev1(0, 0), (-2, 2));
    }

    /// TIER 4 — the high-motion detector's threshold comparison, and that
    /// the two categories come from DIFFERENT functions.
    #[test]
    fn tier4_sq_search_area_multiplier() {
        let ctrls = MdSqMeCtrls {
            pame_distortion_th: 10,
            ..Default::default()
        };
        // Blocks above 64 never trigger.
        assert_eq!(
            sq_search_area_multiplier(&ctrls, 128, 16, 16, u32::MAX, false, &[], 0, 0, Mv::ZERO),
            0
        );
        // Below the threshold: 0.
        assert_eq!(
            sq_search_area_multiplier(&ctrls, 32, 16, 16, 10 * 256, false, &[], 0, 0, Mv::ZERO),
            0
        );
        // Above it with an INTER reference: the temporal (absolute)
        // category.
        assert_eq!(
            sq_search_area_multiplier(
                &ctrls,
                32,
                16,
                16,
                10 * 256 + 1,
                false,
                &[],
                0,
                0,
                mv(0, -3000)
            ),
            2
        );
        // Above it with an INTRA/key reference: the spatial (signed)
        // category, which ignores the same negative magnitude.
        assert_eq!(
            sq_search_area_multiplier(
                &ctrls,
                32,
                16,
                16,
                10 * 256 + 1,
                true,
                &[mv(0, -3000)],
                0,
                0,
                mv(0, -3000)
            ),
            0
        );
        assert_eq!(
            sq_search_area_multiplier(
                &ctrls,
                32,
                16,
                16,
                10 * 256 + 1,
                true,
                &[mv(0, 3000)],
                0,
                0,
                Mv::ZERO
            ),
            3
        );
    }

    /// TIER 4 — the NSQ MVC list: SQ MV first, deduped sub-block MVs,
    /// then a zero MV only if absent.
    #[test]
    fn tier4_nsq_mvc_list() {
        let l = nsq_mvc_list(mv(8, 8), &[mv(16, 16), mv(8, 8), mv(24, 24)]);
        assert_eq!(l, vec![mv(8, 8), mv(16, 16), mv(24, 24), Mv::ZERO]);
        // A list already containing (0,0) does not get a second one.
        let l = nsq_mvc_list(Mv::ZERO, &[mv(8, 0)]);
        assert_eq!(l, vec![Mv::ZERO, mv(8, 0)]);
        // The cap is 6 entries.
        let many: Vec<Mv> = (1..12).map(|i| mv(i * 8, 0)).collect();
        assert_eq!(
            nsq_mvc_list(Mv::ZERO, &many).len(),
            MAX_MD_NSQ_SEARCH_MVC_CNT
        );
    }

    /// TIER 4 — `(v + 4) & ~7` rounds to nearest full pel with ties UP,
    /// which is NOT truncation toward zero.
    #[test]
    fn tier4_round_to_full_pel_ties_up() {
        assert_eq!(round_to_full_pel(0), 0);
        assert_eq!(round_to_full_pel(3), 0);
        assert_eq!(round_to_full_pel(4), 8);
        assert_eq!(round_to_full_pel(-4), 0);
        assert_eq!(round_to_full_pel(-5), -8);
        assert_eq!(round_to_full_pel(-8), -8);
    }

    /// TIER 4 — the fixed-stage sub-pel search's three exits and its bias
    /// rule.
    #[test]
    fn tier4_md_subpel_search_fixed_stage_exits() {
        let ctrls = MdSubpelCtrls {
            abs_th_mult: 1,
            max_precision: 2, // > QUARTER_PEL: no quarter-pel stage
            ..Default::default()
        };
        // The integer baseline is already below th_normalizer
        // (16 * 16 * 1 = 256), so C returns WITHOUT writing me_mv.
        let mut probe = Probe::new(i32::MIN);
        probe.subpel.clear();
        let mut m = mv(24, -16);
        // Probe::variance returns 1000 unless the index is the target;
        // make the baseline the target so it costs 0.
        let base = 32 + 3 + (32 - 2) * 448;
        probe.target = base;
        let got =
            md_subpel_search_fixed_stage(&ctrls, &mut probe, 32, 32, 16, 16, 8, 448, 0, &mut m);
        assert_eq!(got, 0);
        assert_eq!(m, mv(24, -16), "the early exit leaves me_mv untouched");

        // With a high baseline the half-pel stage runs: four probes.
        let mut probe = Probe::new(i32::MIN);
        let mut m = mv(24, -16);
        let got =
            md_subpel_search_fixed_stage(&ctrls, &mut probe, 32, 32, 16, 16, 8, 448, 0, &mut m);
        assert_eq!(got, 1000);
        // 1 integer + 4 half-pel probes.
        assert_eq!(probe.visited.len(), 5);
        // No improvement, so the MV is unchanged (best_dx/dy stay 0).
        assert_eq!(m, mv(24, -16));

        // max_precision <= QUARTER_PEL adds four more probes.
        let ctrls_q = MdSubpelCtrls {
            max_precision: QUARTER_PEL,
            ..ctrls
        };
        let mut probe = Probe::new(i32::MIN);
        let mut m = mv(24, -16);
        md_subpel_search_fixed_stage(&ctrls_q, &mut probe, 32, 32, 16, 16, 8, 448, 0, &mut m);
        assert_eq!(probe.visited.len(), 9);
    }

    /// TIER 4 — `pred_variance_th`'s flat-reference exit is normalised
    /// per pixel by `ROUND_POWER_OF_TWO(var, num_pels_log2)`.
    #[test]
    fn tier4_md_subpel_fixed_stage_pred_variance_exit() {
        let ctrls = MdSubpelCtrls {
            abs_th_mult: 0, // never exit on th_normalizer
            pred_variance_th: 100,
            max_precision: 2,
            ..Default::default()
        };
        let mut probe = Probe::new(i32::MIN);
        // 256x256 block would be log2 8; var 25000 >> 8 = 98 (rounded),
        // below 100 -> exit before any sub-pel probe.
        probe.flat = 25_000;
        let mut m = mv(0, 0);
        md_subpel_search_fixed_stage(&ctrls, &mut probe, 0, 0, 16, 16, 8, 448, 0, &mut m);
        assert_eq!(probe.visited.len(), 1, "only the integer baseline ran");

        // A higher flat variance keeps the search going.
        let mut probe = Probe::new(i32::MIN);
        probe.flat = 26_000;
        let mut m = mv(0, 0);
        md_subpel_search_fixed_stage(&ctrls, &mut probe, 0, 0, 16, 16, 8, 448, 0, &mut m);
        assert_eq!(probe.visited.len(), 5);
    }

    /// TIER 4 — the MVP list: `shut_fast_rate` gives ONE ZERO MVP, not
    /// the stack's nearest.
    #[test]
    fn tier4_build_single_ref_mvp_list() {
        let r = geom();
        let stack = [mv(4, 4), mv(12, 12), mv(20, 20), mv(28, 28)];
        assert_eq!(
            build_single_ref_mvp_list(true, &stack, 4, 32, 32, 16, 16, &r),
            vec![Mv::ZERO]
        );

        // ref_mv_count 4 -> max_drl_index for NEARMV is 3.
        let l = build_single_ref_mvp_list(false, &stack, 4, 32, 32, 16, 16, &r);
        assert_eq!(l.len(), 4);
        assert_eq!(l[0], mv(8, 8), "nearest rounded to full pel");
        assert_eq!(l[1], mv(16, 16));

        // ref_mv_count 0 -> max_drl_index 1, so NEAREST plus one NEAR.
        let l = build_single_ref_mvp_list(false, &stack, 0, 32, 32, 16, 16, &r);
        assert_eq!(l.len(), 2);

        // Duplicates after rounding collapse.
        let dup = [mv(4, 4), mv(5, 5), mv(6, 6), mv(7, 7)];
        let l = build_single_ref_mvp_list(false, &dup, 4, 32, 32, 16, 16, &r);
        assert_eq!(l, vec![mv(8, 8)]);
    }

    /// TIER 4 — the best-MVP pick breaks ties toward the EARLIER index.
    #[test]
    fn tier4_best_mvp_by_distortion_ties_to_the_first() {
        let mut probe = Probe::new(i32::MIN); // every cost is 1000
        let mvps = [mv(0, 0), mv(8, 0), mv(16, 0)];
        let (idx, cost) = best_mvp_by_distortion(&mvps, &mut probe, 0, 0, 448, 0);
        assert_eq!((idx, cost), (0, 1000));

        // A unique minimum wins wherever it is.
        let mut probe = Probe::new(2);
        let (idx, cost) = best_mvp_by_distortion(&mvps, &mut probe, 0, 0, 448, 0);
        assert_eq!((idx, cost), (2, 0));
    }

    /// TIER 4 — the PME extents floor at 3 AFTER the rounding division.
    #[test]
    fn tier4_pme_search_extents_floor_is_three() {
        assert_eq!(pme_search_extents(16, 8, false, 1, 100), (16, 8));
        // 16 * 63 / 63 = 16.
        assert_eq!(pme_search_extents(16, 8, true, 63, 63), (16, 8));
        // A tiny weight collapses to the floor, not to 0.
        assert_eq!(pme_search_extents(16, 8, true, 1, 10_000), (3, 3));
    }

    /// TIER 4 — the ME-vs-MVP direction check is a sign PRODUCT, so a
    /// zero ME component never counts as different.
    #[test]
    fn tier4_pme_me_mv_differs_from_mvps() {
        // th = ((640*480) >> 17) * 10 / 10 = 2.
        let (w, h, mult) = (640u32, 480u32, 10i32);
        assert!(pme_me_mv_differs_from_mvps(
            &[mv(16, 0)],
            mv(-16, 0),
            w,
            h,
            mult
        ));
        // Same sign -> not different.
        assert!(!pme_me_mv_differs_from_mvps(
            &[mv(16, 0)],
            mv(16, 0),
            w,
            h,
            mult
        ));
        // A zero ME component: the product is 0, which is not < 0.
        assert!(!pme_me_mv_differs_from_mvps(
            &[mv(16, 0)],
            mv(0, 0),
            w,
            h,
            mult
        ));
        // An MVP below the magnitude threshold is ignored entirely.
        assert!(!pme_me_mv_differs_from_mvps(
            &[mv(2, 0)],
            mv(-16, 0),
            w,
            h,
            mult
        ));
    }

    #[test]
    fn tier4_pme_cost_dev_and_bail() {
        // Both clamp to 1 before the division.
        assert_eq!(pme_to_me_cost_dev(0, 0), 0);
        assert_eq!(pme_to_me_cost_dev(200, 100), 100);
        assert_eq!(pme_to_me_cost_dev(50, 100), -50);

        // Close in MV: bail regardless of cost.
        assert!(pme_bails_to_me(mv(8, 8), mv(10, 10), 4, -1000, 1000));
        // Far in MV but the cost deviation reaches the threshold: bail.
        assert!(pme_bails_to_me(mv(8, 8), mv(80, 80), 4, 50, 50));
        // Far and cheap: run the search.
        assert!(!pme_bails_to_me(mv(8, 8), mv(80, 80), 4, 49, 50));
    }

    /// TIER 4 — the subpel MV limits are asymmetric between the min and
    /// max sides.
    #[test]
    fn tier4_subpel_mv_limits() {
        let (row_min, row_max, col_min, col_max) = subpel_mv_limits(4, 8, 4, 4, 64, 64);
        assert_eq!(row_min, -(((4 + 4) * 4) + 4));
        assert_eq!(col_min, -(((8 + 4) * 4) + 4));
        assert_eq!(row_max, (64 - 4) * 4 + 4);
        assert_eq!(col_max, (64 - 8) * 4 + 4);
    }

    /// TIER 4 — the ME centre inherits the SQ MV for NSQ blocks, EXCEPT
    /// 64x128 / 128x64.
    #[test]
    fn tier4_me_mv_center_inheritance() {
        let sq = mv(20, -20);
        let raw = mv(3, -3);
        // NSQ, parent available, not the excluded sizes -> inherit,
        // rounded.
        assert_eq!(
            me_mv_center(true, 32, 16, false, false, false, sq, raw),
            mv(24, -16)
        );
        // The excluded shapes fall back to the raw ME MV x8.
        assert_eq!(
            me_mv_center(true, 64, 128, true, false, false, sq, raw),
            mv(24, -24)
        );
        // A square block does not inherit.
        assert_eq!(
            me_mv_center(true, 32, 32, false, false, false, sq, raw),
            mv(24, -24)
        );
        // A 4x4 whose parent was tested DOES inherit even though it is
        // square.
        assert_eq!(
            me_mv_center(false, 4, 4, false, true, true, sq, raw),
            mv(24, -16)
        );
        // ...but only when the parent was tested.
        assert_eq!(
            me_mv_center(false, 4, 4, false, true, false, sq, raw),
            mv(24, -24)
        );
    }
}
