//! The interpolation-filter search at MDS3 — the funnel's call site for
//! [`svtav1_dsp::port_ifs::interpolation_filter_search`].
//!
//! C runs `interpolation_filter_search` (enc_inter_prediction.c:2058) from
//! `svt_aom_inter_pu_prediction_av1` (:3803), the inter arm of
//! `product_prediction_fun_table`, under `ctx->mds_do_ifs &&
//! frm_hdr.interpolation_filter == SWITCHABLE && !use_intrabc &&
//! av1_is_interp_needed_md(..)` (:3817-3820). `mds_do_ifs` is set per stage
//! from `ifs_ctrls.level` (product_coding_loop.c:1648, :7029, :7043, :7148).
//! On the video ladder the level is `interpolation_search_level` 4 =
//! `IFS_MDS3` for every preset the port accepts (enc_mode_config.c:9083-9098:
//! 2 needs `ENC_MR`, which is -1 and below the port's unsigned preset; 0
//! only above M8 on a non-base picture with a high `ref_skip_percentage`), so
//! the search runs ONCE per MDS3 candidate, inside `full_loop_core`'s
//! prediction call (:6848-6853), before the transform loop. That is where
//! [`ifs_at_mds3`] is called from (`mds3::eval_candidate`).
//!
//! What the search decides is `block_mi.interp_filters` — WRITTEN to the
//! bitstream per inter block (`write_mb_interp_filter`) — and the
//! `switchable_rate` it adds to `fast_luma_rate` (:2211), which the MDS3
//! full cost then carries. Both halves are transcribed here.
//!
//! # Evidence (docs/INTER-ENCODE-PLAN.md §1z³⁶)
//!
//! The C function is `static` and takes the whole MD context, so there is
//! no tier-1 shim for the search itself. Its inputs are pinned separately —
//! `svt_aom_get_switchable_rate` (exported) in `tests/c_parity_rd_cost.rs`,
//! `model_rd_from_sse` in `svtav1-dsp/tests/c_parity_port_model_rd.rs` — and
//! the decision structure in `port_ifs`'s own tests. The search AS CALLED is
//! joined against C's per-candidate `SVT_IFS_OUT` interposer on the exported
//! caller by `tools/ifs_join_gate.sh`: same candidate (origin, size, mode,
//! MV), same full-pel verdict, same filter pair after the call, same rate
//! added.
//!
//! MEASURED on the 96-cell grid, frame 1, C side (2026-09-04): 367 MDS3
//! candidates reach the search, all 367 with a FULL-PEL MV, and C keeps
//! `EIGHTTAP_REGULAR` on every one — the port's former constant was
//! byte-correct there, and what it lacked was the RATE. The sub-pel arm
//! (predict with each pair, `model_rd_for_sb`) is transcribed below but no
//! cell on this envelope reaches it; it is stated as unverified, not as
//! verified.

use alloc::vec;
use alloc::vec::Vec;

use svtav1_dsp::port_ifs::{self, IfsCandidateCost, IfsCtrls};
use svtav1_types::block::BlockSize;

use super::types::{Cand, FunnelCtx, LeafGeom};
use crate::port_enc_mode_config::ctrls::IfsLevel;
use crate::port_rd_cost::inter_cost::{SWITCHABLE, get_switchable_rate, is_interp_needed_md};

/// C `svt_aom_inter_pu_prediction_av1`'s IFS hook (enc_inter_prediction.c
/// :3817-3823) for one MDS3 inter candidate: decide `interp_filters`,
/// rebuild the prediction when the pair changed, add the switchable rate.
///
/// `full_lambda` is C's `full_lambda_md[EB_8_BIT_MD]` (`:2081`, the 8-bit
/// arm — the port's inter path is 8-bit); `quantizer` is
/// `y_dequant_qtx[base_q_idx][1]` (`:2027-2029`).
#[allow(clippy::too_many_arguments)]
pub(super) fn ifs_at_mds3(
    fx: &mut FunnelCtx<'_>,
    g: &LeafGeom,
    full_lambda: u64,
    y_src: &[u8],
    y_src_stride: usize,
    y_src_off: usize,
    quantizer: i16,
    cand: &mut Cand,
) {
    #[cfg(feature = "std")]
    let skip = |why: &str| {
        if crate::dbgenv::ifsdbg() {
            std::eprintln!(
                "IFSDBG SKIP why={why} org=({},{}) {}x{}",
                g.abs_x,
                g.abs_y,
                g.w,
                g.h
            );
        }
    };
    #[cfg(not(feature = "std"))]
    let skip = |_why: &str| {};
    let Some(im) = fx.inter else {
        skip("no-inter-frame");
        return;
    };
    // C :7148 — `mds_do_ifs` at MDS3. The `IFS_MDS1`/`IFS_MDS2`-with-bypass
    // arms of that predicate, and `IFS_MDS0`, need `interpolation_search_level`
    // 1..3, which the video ladder never yields for a non-negative preset
    // (module header) and the allintra ladder never applies to an inter
    // block. Unreachable, and NOT modelled.
    match im.search.ifs_level {
        IfsLevel::Mds3 => {}
        IfsLevel::Off => {
            skip("level-off");
            return;
        }
        IfsLevel::Mds0 | IfsLevel::Mds1 | IfsLevel::Mds2 => {
            skip("level-unreachable");
            debug_assert!(
                false,
                "IFS level {:?} is unreachable on this port's ladders",
                im.search.ifs_level
            );
            return;
        }
    }
    let Cand {
        inter,
        pred,
        flr,
        ibc,
        ..
    } = cand;
    let Some(ic) = inter.as_deref_mut() else {
        skip("not-inter-cand");
        return;
    };
    // :3818-3819 — the header must say SWITCHABLE; IntraBC is always
    // BILINEAR and never searched.
    if ibc.is_some() {
        skip("intrabc");
        return;
    }
    if im.interpolation_filter != SWITCHABLE {
        skip("header-not-switchable");
        return;
    }
    let (w, h, abs_x, abs_y) = (g.w, g.h, g.abs_x, g.abs_y);
    let bsize = BlockSize::from_u8(crate::entropy::context::block_size_index(w, h) as u8)
        .expect("a leaf's dims are a BLOCK_SIZE");
    // :3820 `av1_is_interp_needed_md` — WARPED_CAUSAL and non-translational
    // global motion code no filter.
    if !is_interp_needed_md(ic.motion_mode, ic.mode, bsize, ic.ref_frame, &im.gm_wmtype) {
        skip("interp-not-needed");
        return;
    }
    let grid = fx
        .ibc_mvp
        .as_deref()
        .expect("the MD mi grid is allocated whenever the inter arm is armed");
    let neighbors = crate::inter_md_arm::neighbors_from_grid(
        grid,
        im.mi_cols,
        (abs_y / 4) as i32,
        (abs_x / 4) as i32,
        im.tile,
    );
    // :2077-2080. `is_not_scaled` is always true on this port: a reference
    // is never resampled (superres refuses an inter frame).
    let is_fp = port_ifs::is_full_pel(
        (i32::from(ic.mv[0].x), i32::from(ic.mv[0].y)),
        (ic.ref_frame[1] > 0).then(|| (i32::from(ic.mv[1].x), i32::from(ic.mv[1].y))),
        true,
    );
    let ctrls = IfsCtrls {
        enable_dual_filter: im.enable_dual_filter,
        smooth_bias: im.ifs.smooth_bias,
        picture_qp: usize::from(im.ifs.picture_qp),
        tx_bias: im.ifs.tx_bias,
        full_lambda: u32::try_from(full_lambda).expect("full_lambda_md is a uint32_t in C"),
    };
    let org = ic.interp_filters;
    let flr0 = *flr;
    let padded = im.padded_by_ref[ic.ref_frame[0].max(0) as usize].unwrap_or_else(|| {
        panic!(
            "an inter candidate names reference {} with no DPB picture",
            ic.ref_frame[0]
        )
    });
    // C predicts each non-full-pel trial into `ctx->scratch_prediction_ptr`
    // (:2130-2152) and models it from there; the candidate's own prediction
    // is left alone until the winner is known.
    let mut scratch: Vec<u8> = if is_fp { Vec::new() } else { vec![0u8; w * h] };
    let res = port_ifs::interpolation_filter_search(&ctrls, org, is_fp, |_, filters| {
        // :2107 `svt_aom_get_switchable_rate`.
        let switchable_rate = get_switchable_rate(
            im.interpolation_filter,
            ic.ref_frame,
            filters,
            &neighbors,
            im.enable_dual_filter,
            &im.fac,
        );
        if is_fp {
            // :2111-2112 — a full-pel MV is filter-independent; rate only.
            return IfsCandidateCost {
                switchable_rate,
                rate: 0,
                dist: 0,
            };
        }
        // :2130 `svt_aom_inter_prediction` (luma only, PICTURE_BUFFER_DESC_LUMA_MASK).
        crate::inter_pred_arm::predict_inter_luma(
            &padded.y,
            abs_x,
            abs_y,
            w,
            h,
            ic.mv[0],
            filters,
            im.sb_size,
            im.frame_w,
            im.frame_h,
            &mut scratch,
            w,
        );
        // :1977-2040 `model_rd_for_sb`, PLANE_Y..PLANE_Y: spatial SSE (+ the
        // psy term when the effective ac bias is on) through
        // `model_rd_from_sse` at the frame's AC dequant.
        let mut sse = svtav1_dsp::pic_operators::spatial_full_distortion_kernel(
            y_src,
            y_src_off,
            y_src_stride,
            &scratch,
            0,
            w,
            w,
            h,
        );
        if im.ifs.ac_bias_eff != 0.0 {
            sse += svtav1_dsp::ac_bias::psy_full_dist(
                y_src,
                y_src_off,
                y_src_stride,
                &scratch,
                0,
                w,
                w,
                h,
                im.ifs.ac_bias_eff,
            );
        }
        let (rate, dist) =
            svtav1_dsp::port_model_rd::model_rd_for_sb(&[bsize], &[sse], 0, 0, quantizer, 8);
        IfsCandidateCost {
            switchable_rate,
            rate,
            dist: i64::try_from(dist).expect("model_rd distortion fits int64_t, as in C"),
        }
    });
    ic.interp_filters = res.best_filters;
    // CHROMA IS STALE WHENEVER THE PAIR CHANGED, even when luma is not.
    //
    // The search predicts LUMA ONLY, so the chroma buffers still hold the
    // prediction made with the filters the injector set (packed 0,
    // EIGHTTAP_REGULAR both ways) no matter which pair wins.
    // `invalidates_luma_pred` answers a different question -- whether the
    // LUMA buffer happens to hold the winning pair's prediction because it
    // was tried last -- and gating the rebuild on it alone leaves chroma
    // predicted with a filter the bitstream does not name.
    //
    // MEASURED (vidyo3 256x256 p6 qp34 frames=2, benchmarks/
    // chroma_subpel_inter_2026-09-10.meta): block (16,160) 16x16 picked
    // best_filters=0x10001 (SMOOTH/SMOOTH) over was=0x0 with
    // invalidates_luma_pred=false, so nothing was rebuilt and the recon kept
    // REGULAR chroma while the header said SMOOTH.
    //
    // It stayed invisible because it needs the luma phase to be ZERO -- an
    // integer luma MV applies no filter at all, so luma is right either way --
    // while the chroma phase is non-zero. At 4:2:0 that is exactly an mv
    // component that is an ODD MULTIPLE OF 8: integer in luma, half-pel in
    // chroma.
    //
    // Rebuilding when only the pair changed is safe for luma: with
    // `invalidates_luma_pred` false the buffer already holds this pair's
    // prediction, so re-running it is idempotent.
    let chroma_pred_is_stale = g.has_uv && res.best_filters != org;
    if res.invalidates_luma_pred || chroma_pred_is_stale {
        // :2200-2202 `valid_luma_pred = false` -> the prediction call that
        // follows the search (:3838) rebuilds luma and, with `mds_do_chroma`
        // at MDS3, both chroma planes with the new pair. The port carries
        // chroma with luma (§1s item 6), so both are rebuilt here.
        // C `blk_geom->bwidth_uv` is `MAX(4, bwidth >> 1)`, and a sub-8 block's
        // chroma is stitched from the covered cells rather than predicted from
        // this block's MV — see `inter_md_arm::predict_inter_chroma_sub8`. This
        // arm used to rebuild every block's chroma as `w / 2` at stride
        // `w / 2`, which on a 4xN leaf wrote the right samples into the wrong
        // places inside a buffer the funnel reads at stride `max(w, 8) / 2`.
        let sub8 = w < 8 || h < 8;
        let cw = w.max(8) / 2;
        match (g.has_uv && !sub8, padded.uv.as_ref()) {
            (true, Some((refu, refv))) => crate::inter_pred_arm::predict_inter_yuv(
                (&padded.y, refu, refv),
                abs_x,
                abs_y,
                w,
                h,
                ic.mv[0],
                ic.interp_filters,
                im.sb_size,
                im.frame_w,
                im.frame_h,
                pred,
                w,
                &mut ic.u_pred,
                &mut ic.v_pred,
                cw,
            ),
            _ => crate::inter_pred_arm::predict_inter_luma(
                &padded.y,
                abs_x,
                abs_y,
                w,
                h,
                ic.mv[0],
                ic.interp_filters,
                im.sb_size,
                im.frame_w,
                im.frame_h,
                pred,
                w,
            ),
        }
        // C re-applies the OBMC blend on EVERY prediction — it is the tail of
        // `svt_aom_inter_prediction` (:3511), not a one-off at injection — so
        // a rebuilt prediction that skipped it would carry unblended edges
        // into the winner's recon. `av1_is_interp_needed_md` (rd_cost.h:71)
        // excludes WARPED_CAUSAL and non-translational global motion but NOT
        // OBMC, so this arm is reachable.
        let obmc = ic.motion_mode == crate::port_entropy_inter::modes::MotionMode::ObmcCausal;
        if g.has_uv && sub8 {
            crate::inter_md_arm::predict_inter_chroma_sub8(
                &im.padded_by_ref,
                grid,
                im.mi_cols,
                abs_x,
                abs_y,
                w,
                h,
                ic.ref_frame[0],
                ic.mv[0],
                ic.interp_filters,
                im.sb_size,
                im.frame_w,
                im.frame_h,
                &mut ic.u_pred,
                &mut ic.v_pred,
                cw,
            );
        }
        if obmc {
            let spans = crate::inter_md_arm::obmc_nb_spans(
                grid, im.mi_cols, im.mi_rows, im.mi_cols, abs_x, abs_y, w, h,
            );
            crate::obmc_pred_arm::predict_obmc_in_place(
                &crate::obmc_pred_arm::ObmcCtx {
                    padded_by_ref: &im.padded_by_ref,
                    above_row: &spans.above[..spans.n_above],
                    left_col: &spans.left[..spans.n_left],
                    up_available: abs_y > 0,
                    left_available: abs_x > 0,
                    mi_cols: im.mi_cols.max(0) as usize,
                    mi_rows: im.mi_rows.max(0) as usize,
                    sb_size: im.sb_size,
                    frame_w: im.frame_w,
                    frame_h: im.frame_h,
                    edges: crate::inter_pred_arm::block_mb_edges(
                        abs_x, abs_y, w, h, im.frame_w, im.frame_h,
                    ),
                },
                bsize,
                abs_x,
                abs_y,
                w,
                h,
                pred,
                w,
                &mut ic.u_pred,
                &mut ic.v_pred,
                cw,
            );
        }
    }
    // :2205-2208 withdraws `skip_mode_allowed` when the pair is non-zero.
    // The port injects no skip-mode candidate (`skip_mode_flag` needs two
    // references; this is the single-reference low-delay shape), so there is
    // nothing to withdraw.
    // :2211 `fast_luma_rate += switchable_rate`.
    *flr +=
        u64::try_from(res.switchable_rate).expect("a switchable rate is a non-negative bit count");
    #[cfg(feature = "std")]
    if crate::dbgenv::ifsdbg() {
        std::eprintln!(
            "IFSDBG sl={} org=({},{}) {}x{} mode={} rf={},{} mv0={},{} fp={} interp={:#x}->{:#x} rs={} flr={}->{}",
            u8::from(fx.frame.non_i_slice),
            abs_x,
            abs_y,
            w,
            h,
            ic.mode as u8,
            ic.ref_frame[0],
            ic.ref_frame[1],
            ic.mv[0].y,
            ic.mv[0].x,
            u8::from(is_fp),
            org,
            ic.interp_filters,
            res.switchable_rate,
            flr0,
            *flr,
        );
    }
}
