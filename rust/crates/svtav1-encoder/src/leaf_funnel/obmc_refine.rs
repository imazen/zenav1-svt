//! The OBMC MV refinement.
//!
//! C `opt_non_translation_motion_mode`'s OBMC arm
//! (product_coding_loop.c:6798-6824) driving `svt_aom_obmc_motion_refinement`
//! (mode_decision.c:2183-2286) and, inside it, `single_motion_search`
//! (:2069-2181). All three are ported —
//! [`crate::port_md::motion_mode::opt_non_translation_motion_mode_obmc`],
//! [`crate::port_md::mv_refine::obmc_motion_refinement`] and
//! [`crate::port_md::mv_refine::single_motion_search_plan`] — and had no
//! caller. This is the adapter, the twin of [`super::warp_refine`].
//!
//! # Which stage, and why it is not always the same one
//!
//! `obmc_refine_stage(refine_level)` maps the ladder onto an MD stage: levels
//! 1 and 2 refine at STAGE 1, levels 3 and 4 at STAGE 3. Level 0 refines at
//! INJECTION instead, which is a different call site
//! (`inj_non_simple_modes` -> `InjectHooks::obmc_motion_refinement`).
//!
//! `set_obmc_controls` gives `refine_level` 0 at obmc_level 1 (preset MR), 1
//! at obmc_levels 2 and 3 (presets 0 and 1) and 4 at levels 5 and 6. Since
//! `benchmarks/obmc_census_2026-09-10.meta` measured C selecting OBMC ONLY at
//! obmc_levels 1 and 3, the reachable refinements are the INJECTION one and
//! the STAGE 1 one; the stage-3 arm is wired because the ladder can express it.
//!
//! # What it searches against
//!
//! Not the source: `wsrc`/`mask`, the source with the neighbours' predictions
//! blended out (`calc_target_weighted_pred`). C builds that once per block
//! behind `obmc_weighted_pred_ready`;
//! [`crate::obmc_pred_arm::with_obmc_weighted_pred`] is that, over the same
//! thread-local scratch the blend uses.

use super::*;

/// Run C's OBMC MV refinement over one candidate, in place.
///
/// A no-op unless the candidate is a `NEWMV` `OBMC_CAUSAL` one whose
/// `obmc_ctrls.refine_level` selects `stage`.
#[allow(clippy::too_many_arguments)]
pub(super) fn refine_at_stage(
    fx: &FunnelCtx<'_>,
    g: &LeafGeom,
    stage: crate::port_md::motion_mode::MdStage,
    full_lambda: u64,
    y_src: &[u8],
    y_src_stride: usize,
    y_src_off: usize,
    stacks: &[crate::inter_mvp::InterMvpStack],
    cand: &mut Cand,
) {
    use crate::port_md::motion_mode::RefinementSnapshot;
    use crate::port_md::predicates::MotionMode;

    let Some(im) = fx.inter else {
        return;
    };
    let Some(ic) = cand.inter.as_deref_mut() else {
        return;
    };
    if ic.motion_mode != crate::port_entropy_inter::modes::MotionMode::ObmcCausal {
        return;
    }
    let ctrls = crate::port_enc_mode_config::ctrls::set_obmc_controls(im.pic_obmc_level);
    if ctrls.enabled == 0 {
        return;
    }
    let padded = im.padded_by_ref[ic.ref_frame[0].max(0) as usize].unwrap_or_else(|| {
        panic!(
            "an OBMC candidate names reference {} with no DPB picture",
            ic.ref_frame[0]
        )
    });
    let grid = fx
        .ibc_mvp
        .as_deref()
        .expect("the MD mi grid is allocated whenever the inter arm is armed");

    let (w, h) = (g.w, g.h);
    let bsize = svtav1_types::block::BlockSize::from_u8(crate::entropy::context::block_size_index(
        w, h,
    ) as u8)
    .expect("a leaf's dims are a BLOCK_SIZE");

    let mut proj = crate::port_md::inject::InterCandidate {
        mode: ic.mode,
        ref_frame: ic.ref_frame,
        mv: ic.mv,
        pred_mv: ic.pred_mv,
        drl_index: ic.drl_index,
        motion_mode: MotionMode::ObmcCausal,
        ..Default::default()
    };
    let snapshot = RefinementSnapshot {
        mv: ic.mv[0],
        pred_mv: ic.pred_mv[0],
        drl_index: ic.drl_index,
        // C's OBMC arm restores THREE fields, not five; the warp pair is
        // carried because the snapshot type is shared with the warp arm.
        wm_params_l0: svtav1_types::motion::WarpedMotionParams::default(),
        num_proj_ref: 0,
    };

    let spans = crate::inter_md_arm::obmc_nb_spans(
        grid, im.mi_cols, im.mi_rows, im.mi_cols, g.abs_x, g.abs_y, w, h,
    );
    let obmc_ctx = crate::obmc_pred_arm::ObmcCtx {
        padded_by_ref: &im.padded_by_ref,
        above_row: &spans.above[..spans.n_above],
        left_col: &spans.left[..spans.n_left],
        up_available: g.abs_y > 0,
        left_available: g.abs_x > 0,
        mi_cols: im.mi_cols.max(0) as usize,
        mi_rows: im.mi_rows.max(0) as usize,
        sb_size: im.sb_size,
        frame_w: im.frame_w,
        frame_h: im.frame_h,
        edges: crate::inter_pred_arm::block_mb_edges(
            g.abs_x, g.abs_y, w, h, im.frame_w, im.frame_h,
        ),
    };

    let stack = &stacks[ic.ref_frame[0].max(0) as usize];
    let drl = crate::port_md::drl::ChooseDrlCtx {
        shut_fast_rate: false,
        approx_inter_rate: 0,
        ref_mv_stack: &stack.stack,
        ref_mv_count: stack.count,
        nmv_cost: &im.nmv,
        drl_mode_fac_bits: &im.fac.drl_mode,
    };

    let outcome = crate::port_md::motion_mode::opt_non_translation_motion_mode_obmc(
        ctrls.refine_level,
        // C `ctx->pd_pass == PD_PASS_1`; the leaf funnel IS PD1.
        true,
        stage,
        &mut proj,
        snapshot,
        |c| {
            let r = crate::port_md::mv_refine::obmc_motion_refinement(
                w as u16,
                h as u16,
                ctrls.max_blk_size_to_refine,
                // C `ctx->corrupted_mv_check`.
                true,
                &drl,
                c.mode,
                c.mv[0],
                |cand_mv| {
                    run_single_motion_search(
                        &obmc_ctx,
                        &ctrls,
                        im,
                        padded,
                        bsize,
                        g.abs_x,
                        g.abs_y,
                        w,
                        h,
                        full_lambda,
                        y_src,
                        y_src_stride,
                        y_src_off,
                        cand_mv,
                        c.pred_mv[0],
                    )
                },
            );
            if !r.skipped {
                c.mv[0] = r.best_mv;
                c.drl_index = r.drl_index;
                c.pred_mv = r.pred_mv;
            }
            r.valid
        },
    );
    if outcome != crate::port_md::motion_mode::RefinementOutcome::Refined {
        // C restores the snapshot itself on an invalid refinement, and does
        // NOTHING on a valid-but-unchanged one — either way the candidate and
        // its prediction are still the ones MDS0 built.
        return;
    }

    // C `update_refined_mv_fast_rate` (product_coding_loop.c:6741): the fast
    // luma rate carries the OLD MV's bit cost.
    let old_rate = crate::intrabc::mv_bit_cost(
        snapshot.mv,
        snapshot.pred_mv,
        &im.nmv,
        crate::inter_mv_code::MV_COST_WEIGHT,
    );
    let new_rate = crate::intrabc::mv_bit_cost(
        proj.mv[0],
        proj.pred_mv[0],
        &im.nmv,
        crate::inter_mv_code::MV_COST_WEIGHT,
    );
    cand.flr = (cand.flr as i64 + i64::from(new_rate) - i64::from(old_rate)).max(0) as u64;

    let ic = cand.inter.as_deref_mut().expect("checked above");
    ic.mv = proj.mv;
    ic.pred_mv = proj.pred_mv;
    ic.drl_index = proj.drl_index;

    // C `cand_bf->valid_luma_pred = 0`, which makes `full_loop_core` re-run
    // the inter prediction before the residual. The port has no lazy flag, so
    // it rebuilds here — and for OBMC that means the translation AND the
    // blend, because the blend is the tail of the same C function.
    //
    // Skipping this is not a quality difference, it is wrong pixels: the MV
    // that gets CODED is the refined one, so a prediction built from the old
    // MV is one no decoder reproduces. MEASURED on vidyo3 256x256 p0: 653 luma
    // samples diverged from dav1d with this rebuild missing.
    let cw = g.w.max(8) / 2;
    let chh = g.h.max(8) / 2;
    let want_uv = g.has_uv && padded.uv.is_some();
    let (mut u_pred, mut v_pred) = if want_uv {
        (alloc::vec![0u8; cw * chh], alloc::vec![0u8; cw * chh])
    } else {
        (alloc::vec::Vec::new(), alloc::vec::Vec::new())
    };
    let mut y_pred = alloc::vec![0u8; w * h];
    let sub8 = w < 8 || h < 8;
    match (want_uv && !sub8, padded.uv.as_ref()) {
        (true, Some((refu, refv))) => crate::inter_pred_arm::predict_inter_yuv(
            (&padded.y, refu, refv),
            g.abs_x,
            g.abs_y,
            w,
            h,
            ic.mv[0],
            ic.interp_filters,
            im.sb_size,
            im.frame_w,
            im.frame_h,
            &mut y_pred,
            w,
            &mut u_pred,
            &mut v_pred,
            cw,
        ),
        _ => crate::inter_pred_arm::predict_inter_luma(
            &padded.y,
            g.abs_x,
            g.abs_y,
            w,
            h,
            ic.mv[0],
            ic.interp_filters,
            im.sb_size,
            im.frame_w,
            im.frame_h,
            &mut y_pred,
            w,
        ),
    }
    if want_uv && sub8 {
        crate::inter_md_arm::predict_inter_chroma_sub8(
            &im.padded_by_ref,
            grid,
            im.mi_cols,
            g.abs_x,
            g.abs_y,
            w,
            h,
            ic.ref_frame[0],
            ic.mv[0],
            ic.interp_filters,
            im.sb_size,
            im.frame_w,
            im.frame_h,
            &mut u_pred,
            &mut v_pred,
            cw,
        );
    }
    crate::obmc_pred_arm::predict_obmc_in_place(
        &obmc_ctx,
        bsize,
        g.abs_x,
        g.abs_y,
        w,
        h,
        &mut y_pred,
        w,
        &mut u_pred,
        &mut v_pred,
        cw,
    );
    cand.pred.clear();
    cand.pred.extend_from_slice(&y_pred);
    ic.u_pred = u_pred;
    ic.v_pred = v_pred;
}

/// C `single_motion_search` (mode_decision.c:2069-2181) for the OBMC arm.
///
/// Returns the refined MV in EIGHTH-pel, which is the precision C leaves
/// `x->best_mv` in — including on the two `else` branches, where the `>> 3`
/// and the `* 8` are what keep it there.
#[allow(clippy::too_many_arguments)]
fn run_single_motion_search(
    obmc_ctx: &crate::obmc_pred_arm::ObmcCtx<'_>,
    ctrls: &crate::port_enc_mode_config::ctrls::ObmcControls,
    im: &crate::inter_md_arm::InterMdFrame<'_>,
    padded: &crate::picture::PaddedRef,
    bsize: svtav1_types::block::BlockSize,
    abs_x: usize,
    abs_y: usize,
    w: usize,
    h: usize,
    full_lambda: u64,
    y_src: &[u8],
    y_src_stride: usize,
    y_src_off: usize,
    cand_mv: svtav1_types::motion::Mv,
    pred_mv: svtav1_types::motion::Mv,
) -> svtav1_types::motion::Mv {
    use crate::inter_me::obmc_search::{
        ObmcSearch, find_best_obmc_sub_pixel_tree_up, obmc_full_pixel_search,
    };
    use crate::port_md::mv_refine::{single_motion_search_fallback_mv, single_motion_search_plan};
    use svtav1_types::motion::Mv;

    let plan = single_motion_search_plan(i32::from(ctrls.refine_level));
    let fallback = single_motion_search_fallback_mv(plan, cand_mv);
    if !plan.do_full_refine && !plan.do_frac_refine {
        return fallback;
    }

    // C `single_motion_search`'s MV limits (:2101-2106), from the block's own
    // mi position — `AOM_INTERP_EXTEND` past each frame edge.
    const AOM_INTERP_EXTEND: i32 = 4;
    let (mi_row, mi_col) = ((abs_y / 4) as i32, (abs_x / 4) as i32);
    let (mi_w, mi_h) = ((w / 4) as i32, (h / 4) as i32);
    let base_limits = svtav1_types::motion::FullMvLimits {
        row_min: -(((mi_row + mi_h) * 4) + AOM_INTERP_EXTEND),
        col_min: -(((mi_col + mi_w) * 4) + AOM_INTERP_EXTEND),
        row_max: (im.mi_rows - mi_row) * 4 + AOM_INTERP_EXTEND,
        col_max: (im.mi_cols - mi_col) * 4 + AOM_INTERP_EXTEND,
    };

    // C `x->errorperbit = full_lambda >> RD_EPB_SHIFT`, floored at 1.
    let errorperbit =
        crate::port_md::mv_refine::error_per_bit(u32::try_from(full_lambda).unwrap_or(u32::MAX));
    // C `x->sadperbit16 = svt_aom_get_sad_per_bit(base_q_idx, 0)`.
    let sadpb = crate::port_md::pme::get_sad_per_bit(usize::from(im.search.base_q_idx), false);

    let refined = crate::obmc_pred_arm::with_obmc_weighted_pred(
        obmc_ctx,
        bsize,
        abs_x,
        abs_y,
        w,
        h,
        &y_src[y_src_off..],
        y_src_stride,
        |wsrc, mask| {
            // C `svt_av1_setup_pred_block`: the reference block at the
            // block's own origin, addressed FULL-PEL from there.
            let pre_base = (padded.y.origin + abs_y * padded.y.stride + abs_x) as i64;
            let mut s = ObmcSearch {
                pre: &padded.y.buf,
                pre_base,
                pre_stride: padded.y.stride,
                wsrc,
                mask,
                w,
                h,
                mv_limits: base_limits,
                approx_inter_rate: false,
                mv_cost: &im.nmv,
                errorperbit,
            };
            let mut best = Mv::ZERO;
            if plan.do_full_refine {
                // C narrows the limits around the PREDICTED mv for the
                // full-pel pass only, then restores them (:2118, :2136).
                let mut narrowed = base_limits;
                crate::intrabc::set_mv_search_range(&mut narrowed, pred_mv);
                s.mv_limits = narrowed;
                let mvp_full = Mv {
                    x: cand_mv.x >> 3,
                    y: cand_mv.y >> 3,
                };
                obmc_full_pixel_search(
                    &s,
                    mvp_full,
                    sadpb,
                    pred_mv,
                    &mut best,
                    i32::from(ctrls.fpel_search_range),
                    ctrls.fpel_search_diag != 0,
                );
                s.mv_limits = base_limits;
            } else {
                best = Mv {
                    x: cand_mv.x >> 3,
                    y: cand_mv.y >> 3,
                };
            }
            if plan.do_frac_refine {
                find_best_obmc_sub_pixel_tree_up(
                    &s,
                    &mut best,
                    pred_mv,
                    im.allow_high_precision_mv,
                    errorperbit,
                    i32::from(plan.subpel_force_stop),
                    i32::from(plan.subpel_iters_per_step),
                    i32::from(plan.subpel_use_8_taps),
                );
            } else {
                best = Mv {
                    x: best.x.wrapping_mul(8),
                    y: best.y.wrapping_mul(8),
                };
            }
            best
        },
    );
    // `None` means the block had no overlapping neighbour, which C cannot
    // reach from an OBMC candidate (`motion_mode_allowed` needs one).
    refined.unwrap_or(fallback)
}
