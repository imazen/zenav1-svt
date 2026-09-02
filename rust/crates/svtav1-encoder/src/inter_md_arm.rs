//! Frame-level state for the INTER branch of mode decision, and the per-block
//! candidate builder the leaf funnel injects through.
//!
//! `docs/INTER-ENCODE-PLAN.md` §1s items 1b, 2, 3 and 6: everything downstream
//! of mode decision is ported and gated, and every island this module reaches
//! (`inter_me_arm`, `inter_mvp`, `port_md::drl`, `port_rd_cost::inter_cost`,
//! `inter_pred_arm`) was ported with no caller. This is the caller.
//!
//! # What lives here and why it is not in `leaf_funnel`
//!
//! The funnel owns candidate EVALUATION — MDS0 -> MDS1 -> MDS3, one intra
//! candidate set at a time. What an inter candidate needs before it can enter
//! that pipeline is a different set of concerns entirely: an open-loop motion
//! search, a reference-MV stack over the MD mode-info grid, a DRL choice, a
//! motion-compensated prediction on three planes and C's `inter_fast_cost`.
//! Keeping them here means `leaf_funnel/inject.rs` gains ONE call rather than
//! six modules' worth of imports, and it means this code is reachable from a
//! test without standing up a funnel.
//!
//! # Scope, stated as a fraction (`docs/WORKING-ON-THIS.md` §NEVER CLAIM
//! FALSE COMPLETION)
//!
//! What is MISSING, first: no compound candidate, no NEAREST/NEAR/GLOBAL
//! candidate (only `NEWMV`), no second reference, no interpolation-filter
//! search (`EIGHTTAP_REGULAR` in both directions), no motion-mode search (no
//! OBMC, no warp), no inter-intra, no predictive-ME refinement of the ME MV,
//! no sub-pel refinement, and no `skip_mode`. C's `inject_inter_candidates`
//! (mode_decision.c:2264) builds all of those. What IS here is the ONE
//! candidate C commits on this campaign's reference cell — `NEWMV` off
//! `LAST_FRAME` at the open-loop MV — priced with C's real
//! `svt_aom_inter_fast_cost`, predicted with C's real convolve, and placed on
//! C's real reference-MV stack.

use crate::inter_me_arm::FrameMe;
use crate::inter_mvp::NONE_FRAME;
use crate::inter_mvp::{InterMvpEnv, setup_ref_mv_list};
use crate::intrabc::TileMiBounds;
use crate::intrabc_mvp::{MvpGrid, MvpMiEntry, derive_block_ctx};
use crate::picture::PaddedRef;
use crate::port_entropy_inter::modes::{MotionMode, TransformationType};
use crate::port_entropy_inter::{InterCdfs, NeighborMi, Neighbors};
use crate::port_md::drl::{ChooseDrlCtx, choose_best_av1_mv_pred};
use crate::port_md::pme::{MV_VALS, MvCostTable};
use crate::port_md::ref_frame_rate::{NeighborRefCounts, RefFrameFacBits};
use crate::port_rd_cost::inter_cost::{
    InterBlock, InterCandidate, InterFacBits, InterFrame, inter_fast_cost,
};
use alloc::vec::Vec;
use svtav1_types::motion::Mv;
use svtav1_types::prediction::PredictionMode;

/// C `LAST_FRAME`.
pub const LAST_FRAME: i8 = 1;

/// The frame-level tables and pictures the inter branch of MD reads.
///
/// Built once per inter frame, shared by every leaf.
pub struct InterMdFrame<'a> {
    /// The DPB reference with C's replicated margins — what the MC indexes.
    pub padded: &'a PaddedRef,
    /// This frame's open-loop motion search.
    pub me: &'a FrameMe,
    /// C `md_rate_estimation_ptr`'s inter tables.
    pub fac: InterFacBits,
    /// C's reference-signalling tables.
    pub ref_fac: RefFrameFacBits,
    /// C `mvjcost` + `mvcost[2]` at the frame's MV precision.
    pub nmv: MvCostTable,
    /// The frame-header/sequence-header fields `inter_fast_cost` reads.
    pub interpolation_filter: u8,
    pub is_motion_mode_switchable: bool,
    pub allow_warped_motion: bool,
    pub force_integer_mv: bool,
    pub allow_high_precision_mv: bool,
    pub enable_dual_filter: bool,
    pub enable_masked_compound: bool,
    pub enable_jnt_comp: bool,
    pub enable_interintra_compound: bool,
    pub reference_mode_is_select: bool,
    pub allow_screen_content_tools: bool,
    pub order_hint: OrderHints,
    /// The MVP environment (global motion, temporal MVs, order hints).
    pub mvp_env: InterMvpEnv<'a>,
    /// mi geometry for the MVP scans.
    pub mi_rows: i32,
    pub mi_cols: i32,
    pub tile: TileMiBounds,
    pub sb_mi_size: i32,
    /// ALIGNED frame dims and the superblock size, for the MC clamp.
    pub frame_w: usize,
    pub frame_h: usize,
    pub sb_size: usize,
    /// C `ppcs->global_motion[ref].wmtype`.
    pub gm_wmtype: [TransformationType; 8],
}

/// The order-hint half of [`InterFrame`], owned so the borrow is local.
#[derive(Clone, Copy, Debug)]
pub struct OrderHints {
    pub enable_order_hint: bool,
    pub order_hint_bits: u32,
    pub cur_order_hint: i32,
    pub ref_order_hint: [i32; 7],
}

impl InterMdFrame<'_> {
    fn cost_frame(&self) -> InterFrame<'_> {
        InterFrame {
            allow_screen_content_tools: self.allow_screen_content_tools,
            // The port writes `skip_mode_present = 0` on every frame it
            // emits, so no candidate can be a skip-mode one and the flag's
            // rate is never paid.
            skip_mode_flag: false,
            interpolation_filter: self.interpolation_filter,
            is_motion_mode_switchable: self.is_motion_mode_switchable,
            force_integer_mv: self.force_integer_mv,
            allow_warped_motion: self.allow_warped_motion,
            enable_dual_filter: self.enable_dual_filter,
            enable_masked_compound: self.enable_masked_compound,
            enable_jnt_comp: self.enable_jnt_comp,
            enable_interintra_compound: self.enable_interintra_compound,
            enable_order_hint: self.order_hint.enable_order_hint,
            order_hint_bits: self.order_hint.order_hint_bits,
            cur_order_hint: self.order_hint.cur_order_hint,
            ref_order_hint: &self.order_hint.ref_order_hint,
            gm_wmtype: &self.gm_wmtype,
        }
    }
}

/// Convert the IntraBC-side MV cost tables to the `port_md` shape.
///
/// The two types differ ONLY in how they clip a component index — see
/// [`MvCostTable`]'s own doc — and both are built by C's single
/// `svt_av1_build_nmv_cost_table` (md_rate_estimation.c:446). Building one
/// from the other means there is one transcription of that function, not two.
#[must_use]
pub fn nmv_cost_table(
    nmvc: &crate::entropy::mv_coding::NmvContext,
    precision: crate::entropy::mv_coding::MvSubpelPrecision,
) -> MvCostTable {
    let t = crate::intrabc::build_nmv_cost_table(nmvc, precision);
    let comp = |i: usize| -> Vec<i32> {
        (0..MV_VALS)
            .map(|v| t.comp_cost[i].cost(v as i32 - crate::intrabc::MV_MAX))
            .collect()
    };
    MvCostTable {
        joint: t.joint_cost,
        comp: [comp(0), comp(1)],
    }
}

/// Build [`InterFacBits`] + [`RefFrameFacBits`] from the live CDFs.
#[must_use]
pub fn build_inter_rates(
    fc: &crate::entropy::context::FrameContext,
    ic: &InterCdfs,
) -> (InterFacBits, RefFrameFacBits) {
    (
        InterFacBits::from_cdfs(fc, ic),
        RefFrameFacBits::from_cdfs(fc, ic),
    )
}

/// One block's INTER candidate, or `None` when the block has no ME result.
pub struct InterCandOut {
    /// The mode-decision payload the funnel carries on its `Cand`.
    pub mode: PredictionMode,
    pub ref_frame: [i8; 2],
    pub mv: [Mv; 2],
    pub pred_mv: [Mv; 2],
    pub drl_index: u8,
    pub interp_filters: u32,
    pub motion_mode: MotionMode,
    /// The motion-compensated prediction, luma then the two chroma planes.
    pub y_pred: Vec<u8>,
    pub u_pred: Vec<u8>,
    pub v_pred: Vec<u8>,
    /// C `cand_bf->fast_luma_rate`.
    pub fast_luma_rate: u32,
}

/// The per-block inputs the caller has and this module does not.
pub struct InterBlockCtx<'a> {
    /// Frame-absolute luma origin and dims.
    pub org_x: usize,
    pub org_y: usize,
    pub bw: usize,
    pub bh: usize,
    /// C `BlockSize` index.
    pub bsize: u8,
    /// The MD mode-info grid, already carrying this block's own partition in
    /// its first cell (C's MVP scan runs against the live mi state).
    pub grid: &'a [MvpMiEntry],
    pub grid_stride: i32,
    /// C `xd->above_mbmi` / `left_mbmi` and their availability flags.
    pub neighbors: Neighbors,
    /// C `blk_ptr->overlappable_neighbors`.
    pub overlappable_neighbors: u32,
    /// C `ctx->is_inter_ctx` (`svt_av1_get_intra_inter_context`).
    pub is_inter_ctx: usize,
    /// Whether the block has a chroma pair; when false the chroma prediction
    /// is not produced.
    pub has_uv: bool,
}

/// Build the block's `NEWMV` candidate off `LAST_FRAME`.
///
/// Returns `None` when the open-loop search has no entry for this geometry,
/// which is a caller-geometry question and not a decision.
#[must_use]
pub fn build_inter_candidate(
    f: &InterMdFrame<'_>,
    b: &InterBlockCtx<'_>,
    lambda: u64,
    luma_distortion: u64,
) -> Option<InterCandOut> {
    // --- 1. The open-loop MV (full pel), as C injects it: `* 8`
    //        (mode_decision.c:2323-2325).
    let mv_fp = f.me.mv_for(b.org_x, b.org_y, b.bsize, 0, 0, 4)?;
    let mv = Mv {
        x: mv_fp.x.saturating_mul(8),
        y: mv_fp.y.saturating_mul(8),
    };

    // --- 2. The reference-MV stack, and the DRL choice over it.
    let ctx = derive_block_ctx(
        (b.org_y / 4) as i32,
        (b.org_x / 4) as i32,
        b.bsize as usize,
        f.mi_rows,
        f.mi_cols,
        f.tile,
        f.sb_mi_size,
    );
    let grid = MvpGrid {
        entries: b.grid,
        stride: b.grid_stride,
        base: (b.org_y / 4) as i32 * b.grid_stride + (b.org_x / 4) as i32,
    };
    // C `svt_aom_generate_av1_mvp_table`'s `gm_mv` for an IDENTITY global
    // motion model is the zero MV; this port signals no global motion.
    let stack = setup_ref_mv_list(&grid, &ctx, &f.mvp_env, LAST_FRAME, [Mv::ZERO; 2]);
    let mut drl_index = 0u8;
    let mut pred_mv = [Mv::ZERO; 2];
    choose_best_av1_mv_pred(
        &ChooseDrlCtx {
            shut_fast_rate: false,
            approx_inter_rate: 0,
            ref_mv_stack: &stack.stack,
            ref_mv_count: stack.count,
            nmv_cost: &f.nmv,
            drl_mode_fac_bits: &f.fac.drl_mode,
        },
        PredictionMode::NewMv,
        mv,
        Mv::ZERO,
        &mut drl_index,
        &mut pred_mv,
    );

    // --- 3. The motion-compensated prediction. C does luma and both chroma
    //        planes in ONE `av1_inter_prediction_light_pd1` call under a
    //        component mask, so this is one call (see `inter_pred_arm`).
    let interp_filters = 0u32; // EIGHTTAP_REGULAR in both directions
    let mut y_pred = alloc::vec![0u8; b.bw * b.bh];
    let (cw, chh) = (b.bw / 2, b.bh / 2);
    let (mut u_pred, mut v_pred) = if b.has_uv {
        (alloc::vec![0u8; cw * chh], alloc::vec![0u8; cw * chh])
    } else {
        (Vec::new(), Vec::new())
    };
    match (b.has_uv, f.padded.uv.as_ref()) {
        (true, Some((refu, refv))) => crate::inter_pred_arm::predict_inter_yuv(
            (&f.padded.y, refu, refv),
            b.org_x,
            b.org_y,
            b.bw,
            b.bh,
            mv,
            interp_filters,
            f.sb_size,
            f.frame_w,
            f.frame_h,
            &mut y_pred,
            b.bw,
            &mut u_pred,
            &mut v_pred,
            cw,
        ),
        _ => crate::inter_pred_arm::predict_inter_luma(
            &f.padded.y,
            b.org_x,
            b.org_y,
            b.bw,
            b.bh,
            mv,
            interp_filters,
            f.sb_size,
            f.frame_w,
            f.frame_h,
            &mut y_pred,
            b.bw,
        ),
    }

    // --- 4. C's real MDS0 rate, `svt_aom_inter_fast_cost` (rd_cost.c:1005).
    // `ref_frame_rate` carries its own two-field `NeighborMi` (only
    // `ref_frame` + `use_intrabc` are read there); this is a projection, not
    // a second neighbour derivation.
    let rr = |n: Option<NeighborMi>| {
        n.map(|m| crate::port_md::ref_frame_rate::NeighborMi {
            ref_frame: m.ref_frame,
            use_intrabc: m.use_intrabc,
        })
    };
    let (rr_above, rr_left) = (
        rr(b.neighbors.above_avail().copied()),
        rr(b.neighbors.left_avail().copied()),
    );
    let counts = NeighborRefCounts::collect(rr_above, rr_left);
    let ref_bits = crate::port_md::ref_frame_rate::estimate_ref_frames_num_bits(
        &[LAST_FRAME],
        &counts,
        rr_above,
        rr_left,
        f.reference_mode_is_select,
        b.bw as u16,
        b.bh as u16,
        &f.ref_fac,
        |rf| [rf, NONE_FRAME],
    );
    let ref_frames_num_bits = ref_bits.first().map_or(0, |&(_, bits)| bits);

    let bsize = svtav1_types::block::BlockSize::from_u8(b.bsize)
        .expect("an injected inter block must have a real BlockSize");
    let cost = inter_fast_cost(
        &f.cost_frame(),
        &InterBlock {
            bsize,
            // The port writes `skip_mode_present = 0`, so the skip-mode
            // context is never read; 0 is C's own initial value.
            skip_mode_ctx: 0,
            is_inter_ctx: b.is_inter_ctx,
            inter_mode_ctx: stack.mode_context,
            ref_mv_count: stack.count,
            ref_mv_stack: &stack.stack,
            ref_frames_num_bits,
            neighbors: &b.neighbors,
            overlappable_neighbors: b.overlappable_neighbors,
            approx_inter_rate: 0,
            // C prices the interpolation filter at MDS0 only when the IFS
            // level says the filter is already decided there; this port runs
            // no filter search, so the filter IS known and is priced.
            ifs_at_mds0: true,
        },
        &InterCandidate {
            mode: PredictionMode::NewMv,
            ref_frame: [LAST_FRAME, NONE_FRAME],
            mv: [mv, Mv::ZERO],
            pred_mv,
            drl_index,
            interp_filters,
            motion_mode: MotionMode::SimpleTranslation,
            num_proj_ref: 0,
            is_interintra_used: false,
            interintra_mode: 0,
            use_wedge_interintra: false,
            interintra_wedge_index: 0,
            comp_group_idx: 0,
            compound_idx: 1,
            interinter_comp_type: svtav1_types::prediction::CompoundType::Average,
            interinter_wedge_index: 0,
            skip_mode_allowed: false,
        },
        lambda,
        luma_distortion,
        Some(&f.nmv),
        &f.fac,
    );

    Some(InterCandOut {
        mode: PredictionMode::NewMv,
        ref_frame: [LAST_FRAME, NONE_FRAME],
        mv: [mv, Mv::ZERO],
        pred_mv,
        drl_index,
        interp_filters,
        motion_mode: MotionMode::SimpleTranslation,
        y_pred,
        u_pred,
        v_pred,
        fast_luma_rate: cost.rate.luma,
    })
}

/// The neighbour pair the inter contexts read, from the MD mode-info grid.
///
/// C reads `xd->above_mbmi` / `left_mbmi` — the mi cell ABOVE the block's
/// top-left and the one to its LEFT — and keeps the availability flags
/// separate from the pointers (`port_entropy_inter::Neighbors`).
#[must_use]
pub fn neighbors_from_grid(
    grid: &[MvpMiEntry],
    stride: i32,
    mi_row: i32,
    mi_col: i32,
    tile: TileMiBounds,
) -> Neighbors {
    let at = |r: i32, c: i32| -> NeighborMi {
        let e = grid[(r * stride + c) as usize];
        NeighborMi {
            mode: e.mode,
            ref_frame: e.ref_frame,
            interp_filters: e.interp_filters,
            use_intrabc: e.use_intrabc,
            skip_mode: false,
            comp_group_idx: 0,
            compound_idx: 0,
            bsize: e.bsize,
        }
    };
    let up = mi_row > tile.mi_row_start;
    let left = mi_col > tile.mi_col_start;
    Neighbors {
        above: up.then(|| at(mi_row - 1, mi_col)),
        left: left.then(|| at(mi_row, mi_col - 1)),
        up_available: up,
        left_available: left,
    }
}
