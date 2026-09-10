//! The MDS1 warped-motion MV refinement.
//!
//! C `opt_non_translation_motion_mode`'s WARP arm
//! (product_coding_loop.c:6757-6796) driving `svt_aom_wm_motion_refinement`
//! (mode_decision.c:1873-2011). Both are ported —
//! [`crate::port_md::motion_mode::opt_non_translation_motion_mode_warp`] and
//! [`crate::port_md::mv_refine::wm_motion_refinement`] — and had no caller.
//! This module is the adapter: it supplies the two closures they take, out of
//! the block-scoped inputs [`crate::inter_md_arm::WarpRefineBlock`] carries and
//! the frame-scoped ones on `fx.inter`.
//!
//! # Why it must run at MDS1 and not at injection
//!
//! C defers it deliberately: only MDS0 SURVIVORS pay for it. Running it at
//! injection would change the MDS0 fast cost — the refined MV prices
//! differently — and therefore the MDS1 survivor set, which is a byte
//! difference, not an optimisation.
//!
//! # What it measured
//!
//! `benchmarks/warped_motion_2026-09-10.meta` joined the port's per-block
//! decisions to C's on vidyo3 256x256 preset 6: of 29 blocks where both chose
//! WARPED_CAUSAL, 19 already had the same MV and 10 differed — every one of
//! them by a single step in one component, which is exactly the signature of
//! the missing refinement.

use super::*;

/// Run C's MDS1 warp refinement over one candidate, in place.
///
/// A no-op unless the candidate is a `NEWMV` `WARPED_CAUSAL` one and
/// `wm_ctrls.refine_level == 1`, which is what selects MD stage 1.
pub(super) fn refine_at_mds1(
    fx: &FunnelCtx<'_>,
    g: &LeafGeom,
    blk: &crate::inter_md_arm::WarpRefineBlock,
    full_lambda: u64,
    y_src: &[u8],
    y_src_stride: usize,
    y_src_off: usize,
    cand: &mut Cand,
) {
    use crate::port_md::motion_mode::{MdStage, RefinementOutcome, RefinementSnapshot};
    use crate::port_md::predicates::MotionMode;

    if !blk.enabled {
        return;
    }
    let Some(im) = fx.inter else {
        return;
    };
    let Some(ic) = cand.inter.as_deref_mut() else {
        return;
    };
    if ic.motion_mode != crate::port_entropy_inter::modes::MotionMode::WarpedCausal {
        return;
    }
    let padded = im.padded_by_ref[ic.ref_frame[0].max(0) as usize].unwrap_or_else(|| {
        panic!(
            "a warped candidate names reference {} with no DPB picture",
            ic.ref_frame[0]
        )
    });

    // The port carries the candidate's inter payload on `InterCand`; C's
    // refinement operates on a `ModeDecisionCandidate`. Project into the
    // shape the ported driver takes, run it, and project back — rather than
    // re-implementing the driver against a second candidate type.
    let mut proj = crate::port_md::inject::InterCandidate {
        mode: ic.mode,
        ref_frame: ic.ref_frame,
        mv: ic.mv,
        pred_mv: ic.pred_mv,
        drl_index: ic.drl_index,
        motion_mode: MotionMode::WarpedCausal,
        num_proj_ref: ic.num_proj_ref,
        wm_params_l0: ic.wm_params,
        ..Default::default()
    };
    let snapshot = RefinementSnapshot {
        mv: proj.mv[0],
        pred_mv: proj.pred_mv[0],
        drl_index: proj.drl_index,
        wm_params_l0: proj.wm_params_l0,
        num_proj_ref: proj.num_proj_ref,
    };

    let (w, h) = (g.w, g.h);
    let filters = ic.interp_filters;
    // C `ctx->scratch_prediction_ptr` — one luma-only buffer the search
    // predicts into; the candidate's own prediction is untouched until the
    // winner is known.
    let mut scratch = alloc::vec![0u8; w * h];
    let stack = &blk.stacks[ic.ref_frame[0].max(0) as usize];
    let drl_ctx = crate::port_md::drl::ChooseDrlCtx {
        shut_fast_rate: false,
        approx_inter_rate: 0,
        ref_mv_stack: &stack.stack,
        ref_mv_count: blk.ref_mv_count[ic.ref_frame[0].max(0) as usize],
        nmv_cost: &im.nmv,
        drl_mode_fac_bits: &im.fac.drl_mode,
    };
    // C `error_per_bit = full_lambda >> RD_EPB_SHIFT; error_per_bit += (error_per_bit == 0)`
    // — the `+= (x == 0)` is a MAX(1), spelled as C spells it.
    let epb =
        (u32::try_from(full_lambda).unwrap_or(u32::MAX) >> crate::intrabc::RD_EPB_SHIFT) as i32;
    let refine_ctx = crate::port_md::mv_refine::WmRefineCtx {
        refinement_iterations: blk.refinement_iterations,
        refine_diag: blk.refine_diag,
        allow_high_precision_mv: im.allow_high_precision_mv,
        approx_inter_rate: 0,
        corrupted_mv_check: true,
        error_per_bit: epb + i32::from(epb == 0),
        drl: drl_ctx,
    };

    let outcome = crate::port_md::motion_mode::opt_non_translation_motion_mode_warp(
        blk.refine_level,
        blk.refinement_iterations,
        // C `ctx->pd_pass == PD_PASS_1`. The leaf funnel IS the full PD1 pass;
        // PD0 is `crate::pd0` and never reaches here.
        true,
        MdStage::Stage1,
        &mut proj,
        snapshot,
        |c| {
            let mut wm = svtav1_types::motion::WarpedMotionParams::default();
            let r = crate::port_md::mv_refine::wm_motion_refinement(
                &refine_ctx,
                c.mv[0],
                c.pred_mv[0],
                c.mode,
                |test_mv| {
                    // C: `svt_aom_warped_motion_parameters` -> `continue` when
                    // it rejects, else predict LUMA ONLY and take the variance.
                    wm = svtav1_types::motion::WarpedMotionParams {
                        wm_type: svtav1_types::motion::TransformationType::Affine,
                        ..Default::default()
                    };
                    blk.warp_params_for(
                        c.ref_frame[0],
                        test_mv,
                        blk.shut_approx_if_not_mds0,
                        &mut wm,
                    )?;
                    let mut m = wm;
                    crate::inter_pred_arm::predict_inter_yuv_warped(
                        (&padded.y, None),
                        &mut m,
                        g.abs_x,
                        g.abs_y,
                        w,
                        h,
                        test_mv,
                        filters,
                        im.sb_size,
                        im.frame_w,
                        im.frame_h,
                        &mut scratch,
                        w,
                        &mut [],
                        &mut [],
                        0,
                    );
                    // C `fn_ptr->vf(pred, pred_stride, src, src_stride, &sse)`
                    // — the block-size variance, NOT the SSE.
                    Some(svtav1_dsp::variance::variance_diff(
                        &scratch,
                        w,
                        &y_src[y_src_off..],
                        y_src_stride,
                        w,
                        h,
                    ) as i32)
                },
            );
            c.mv[0] = r.best_mv;
            c.drl_index = r.drl_index;
            c.pred_mv = r.pred_mv;
            r.valid
        },
        |c| {
            // C re-derives the model for the CHOSEN MV with the shortcuts
            // off: `svt_aom_warped_motion_parameters(..., 0, 0, 1)` — both
            // band thresholds zero and `shut_approx` set, "because this call is
            // not part of a search".
            let mut wm = svtav1_types::motion::WarpedMotionParams {
                wm_type: svtav1_types::motion::TransformationType::Affine,
                ..Default::default()
            };
            // The band thresholds are forced to ZERO and `shut_approx` set,
            // which is C's own comment: "this call is not part of a search, so
            // disable the shortcuts". Only the four fields the derivation
            // reads are needed, so this is a projection of `blk`, not a clone
            // (the `stacks` vector is deliberately left empty).
            let final_blk = crate::inter_md_arm::WarpRefineBlock {
                lower_band_th: 0,
                upper_band_th: 0,
                samples: blk.samples,
                mi_row: blk.mi_row,
                mi_col: blk.mi_col,
                bsize: blk.bsize,
                bwidth: blk.bwidth,
                bheight: blk.bheight,
                ..Default::default()
            };
            if let Some(n) = final_blk.warp_params_for(c.ref_frame[0], c.mv[0], true, &mut wm) {
                c.num_proj_ref = n;
                c.wm_params_l0 = wm;
            }
        },
    );

    if outcome != RefinementOutcome::Refined {
        // C restores the snapshot itself (the ported arm does), so the
        // candidate is already back to what it was and its prediction is still
        // valid. Nothing to write back.
        return;
    }

    // C `update_refined_mv_fast_rate` (product_coding_loop.c:6741): the fast
    // luma rate carries the OLD MV's bit cost, so swap it for the new one
    // rather than re-pricing the whole candidate.
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
    ic.num_proj_ref = proj.num_proj_ref;
    ic.wm_params = proj.wm_params_l0;

    // C `cand_bf->valid_luma_pred = 0`, which makes `full_loop_core` re-run
    // the whole inter prediction before the residual. The port has no lazy
    // flag, so it rebuilds here — luma AND chroma, because MDS3's chroma loop
    // reads `ic.u_pred` / `ic.v_pred`.
    let (cw, chh) = (g.w / 2, g.h / 2);
    let want_uv = g.has_uv && padded.uv.is_some();
    let mut wm = ic.wm_params;
    let mut y_pred = alloc::vec![0u8; g.w * g.h];
    let (mut u_pred, mut v_pred) = if want_uv {
        (alloc::vec![0u8; cw * chh], alloc::vec![0u8; cw * chh])
    } else {
        (alloc::vec::Vec::new(), alloc::vec::Vec::new())
    };
    crate::inter_pred_arm::predict_inter_yuv_warped(
        (
            &padded.y,
            padded.uv.as_ref().map(|(u, v)| (u, v)).filter(|_| want_uv),
        ),
        &mut wm,
        g.abs_x,
        g.abs_y,
        g.w,
        g.h,
        ic.mv[0],
        ic.interp_filters,
        im.sb_size,
        im.frame_w,
        im.frame_h,
        &mut y_pred,
        g.w,
        &mut u_pred,
        &mut v_pred,
        cw,
    );
    ic.u_pred = u_pred;
    ic.v_pred = v_pred;
    cand.pred = crate::vecpool::PoolVec::from_slice(&y_pred);
}
