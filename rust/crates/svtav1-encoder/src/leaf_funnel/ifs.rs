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

use super::tx_pipeline::Bd10Rd;
use super::types::{Cand, FunnelCtx, LeafGeom};
use crate::port_enc_mode_config::ctrls::IfsLevel;
use crate::port_rd_cost::inter_cost::{SWITCHABLE, get_switchable_rate, is_interp_needed_md};

/// C `svt_aom_inter_pu_prediction_av1`'s IFS hook (enc_inter_prediction.c
/// :3817-3823) for one MDS3 inter candidate: decide `interp_filters`,
/// rebuild the prediction when the pair changed, add the switchable rate.
///
/// `full_lambda` is C's `full_lambda_md[EB_8_BIT_MD]` (`:2081`, the 8-bit
/// arm); `quantizer` is `y_dequant_qtx[base_q_idx][1]` (`:2027-2029`). When
/// `bd10_rd` is `Some` — the bypass-encdec `hbd_md = 2` bump
/// (product_coding_loop.c:9649) — the search runs at `EB_TEN_BIT` instead:
/// `full_lambda_md[EB_10_BIT_MD] >> (2 * (bd - 8))`, 10-bit trial
/// predictions into a u16 scratch, and `model_rd_for_sb` at bit depth 10
/// (:2081, :2130-2152, :2146).
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
    bd10_rd: &Option<Bd10Rd>,
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
    // 1..3. The video ladder yields those ONLY at preset -1 (the research
    // preset; every non-negative preset gives Mds3 or Off), and there the port
    // skips the search: NOT MODELLED, so preset -1 video codes the filter C's
    // MDS1 search would not have picked (a valid stream; the video gates are
    // decoder-verified, not byte-claimed). This used to be a
    // `debug_assert!(false)` claiming the arm unreachable, which panicked any
    // debug-assertion build at preset -1 (video_paths.rs sweep, 2026-09-26).
    // Porting the arms is plan Phase 3 work.
    match im.search.ifs_level {
        IfsLevel::Mds3 => {}
        IfsLevel::Off => {
            skip("level-off");
            return;
        }
        IfsLevel::Mds0 | IfsLevel::Mds1 | IfsLevel::Mds2 => {
            skip("level-not-modelled");
            return;
        }
    }
    let Cand {
        inter,
        pred,
        pred10,
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
    // enc_inter_prediction.c:2081 — under `hbd_md` (the bypass-encdec MDS3
    // bump, `bd10_rd` = `Some` here) the search divides the 10-bit lambda by
    // `2 * (bd - 8)` and scores 10-bit trials with the 10-bit dequant.
    let (ifs_lambda, ifs_quantizer) = match bd10_rd.as_ref() {
        Some(b) => (
            b.lambda >> (2 * u64::from(b.bd - 8)),
            i16::try_from(b.qt.dequant[1]).expect("y_dequant_qtx is int16_t in C"),
        ),
        None => (full_lambda, quantizer),
    };
    let ctrls = IfsCtrls {
        enable_dual_filter: im.enable_dual_filter,
        smooth_bias: im.ifs.smooth_bias,
        picture_qp: usize::from(im.ifs.picture_qp),
        tx_bias: im.ifs.tx_bias,
        full_lambda: u32::try_from(ifs_lambda).expect("full_lambda_md is a uint32_t in C"),
    };
    let org = ic.interp_filters;
    let flr0 = *flr;
    let padded = im.padded_by_ref[ic.ref_frame[0].max(0) as usize].unwrap_or_else(|| {
        panic!(
            "an inter candidate names reference {} with no DPB picture",
            ic.ref_frame[0]
        )
    });
    // C `has_second_ref` — a compound candidate's second reference, bound the
    // same way `inter_md_arm::predict_and_price` binds it at injection.
    let padded1 = (ic.ref_frame[1] > 0).then(|| {
        im.padded_by_ref[ic.ref_frame[1].max(0) as usize].unwrap_or_else(|| {
            panic!(
                "an inter candidate names reference {} with no DPB picture",
                ic.ref_frame[1]
            )
        })
    });
    // The OBMC blend tail — needed by BOTH the 10-bit trial below (C's
    // `use_precomputed_obmc` arm of the :2130 predict) and the winner
    // rebuild after the search. `av1_is_interp_needed_md` excludes warped
    // and non-translational global motion but NOT OBMC, so an OBMC
    // candidate does reach the search.
    let obmc = ic.motion_mode == crate::port_entropy_inter::modes::MotionMode::ObmcCausal;
    let obmc_spans = obmc.then(|| {
        crate::inter_md_arm::obmc_nb_spans(
            grid, im.mi_cols, im.mi_rows, im.mi_cols, abs_x, abs_y, w, h,
        )
    });
    // C predicts each non-full-pel trial into `ctx->scratch_prediction_ptr`
    // (:2130-2152) and models it from there; the candidate's own prediction
    // is left alone until the winner is known. Under `hbd_md` that scratch
    // is the 16-bit pipeline's plane — the u16 twin here.
    let mut scratch: Vec<u8> = if is_fp || bd10_rd.is_some() {
        Vec::new()
    } else {
        vec![0u8; w * h]
    };
    let mut scratch10: Vec<u16> = if is_fp || bd10_rd.is_none() {
        Vec::new()
    } else {
        vec![0u16; w * h]
    };
    // `block_mi.interinter_comp` → the masked arm's `InterInterCompoundData`
    // + the DIFFWTD `mask_type`, and `dist_wtd_comp_weight_assign`'s outputs
    // — C derives this once per `svt_aom_inter_prediction` call
    // (enc_inter_prediction.c:3253-3264); the trials below pass the same
    // candidate fields.
    let (comp_data, distwtd) = crate::inter_md_arm::compound_md_args(
        im,
        ic.ref_frame,
        ic.interinter_comp_type,
        ic.interinter_mask_type,
        ic.interinter_wedge_index,
        ic.interinter_wedge_sign,
        ic.compound_idx,
    );
    // `ctx->intrapred_buf` — the inter-intra blend's intra predictions,
    // parked on the funnel context at injection (`use_precomputed_ii = 1`
    // in C's own :2137 call; the u16 twin makes the hbd=0 switch the port
    // never needs).
    let ii_preds = fx.ii_preds.as_ref();
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
        if let Some(b) = bd10_rd.as_ref() {
            // :2130-2152 under `hbd_md = 2`: `svt_aom_inter_prediction` at
            // `EB_TEN_BIT` (luma only) into the 16-bit scratch, then
            // `model_rd_for_sb` at `EB_TEN_BIT` (:2146). The ac-bias psy
            // term is `get_svt_psy_full_dist`'s `is_hbd` arm
            // (`svt_psy_distortion_hbd`, the <<2-scaled u16 energy gap).
            let hbd = padded
                .hbd
                .as_ref()
                .expect("the MDS3 bump needs the 10-bit DPB twin");
            if let Some(p1) = padded1 {
                let h1 = p1
                    .hbd
                    .as_ref()
                    .expect("the MDS3 bump needs the 10-bit DPB twin");
                let iw10 = [
                    crate::inter_pred_arm::inter_pred_uses_warp(
                        ic.motion_mode,
                        ic.mode as u8,
                        w,
                        h,
                        &ic.wm_params,
                    ),
                    crate::inter_pred_arm::inter_pred_uses_warp(
                        ic.motion_mode,
                        ic.mode as u8,
                        w,
                        h,
                        &ic.wm_params_l1,
                    ),
                ];
                // C's `av1_inter_prediction` compound arm at `EB_TEN_BIT`
                // — the masked/distwtd metadata is the same candidate's,
                // so a WEDGE/DIFFWTD trial blends what it will code.
                let mut wm0 = ic.wm_params;
                let mut wm1 = ic.wm_params_l1;
                crate::inter_pred_arm::predict_inter_yuv_compound_md_hbd(
                    [(&hbd.y, None), (&h1.y, None)],
                    &mut wm0,
                    &mut wm1,
                    iw10,
                    comp_data,
                    distwtd,
                    &im.wedge_masks,
                    abs_x,
                    abs_y,
                    w,
                    h,
                    bsize as usize,
                    ic.mv,
                    filters,
                    im.sb_size,
                    im.frame_w,
                    im.frame_h,
                    im.bit_depth,
                    &mut scratch10,
                    w,
                    &mut [],
                    &mut [],
                    0,
                );
            } else {
                let mm_base = if obmc {
                    crate::port_entropy_inter::modes::MotionMode::SimpleTranslation
                } else {
                    ic.motion_mode
                };
                let is_wm10 = crate::inter_pred_arm::inter_pred_uses_warp(
                    ic.motion_mode,
                    ic.mode as u8,
                    w,
                    h,
                    &ic.wm_params,
                );
                crate::inter_pred_arm::predict_inter_leaf_hbd(
                    &hbd.y,
                    None,
                    mm_base,
                    is_wm10,
                    ic.wm_params,
                    abs_x,
                    abs_y,
                    w,
                    h,
                    ic.mv[0],
                    filters,
                    im.sb_size,
                    im.frame_w,
                    im.frame_h,
                    im.bit_depth,
                    &mut scratch10,
                    w,
                    &mut [],
                    &mut [],
                    0,
                );
                // `inter_intra_prediction` (enc_inter_prediction.c:3488)
                // — the LUMA arm at `EB_TEN_BIT`, before the OBMC tail.
                // `luma16` is filled for every bd10_rd canvas (its fill
                // gate is `y_recon10`, which `bd10_rd.is_some()` implies).
                if ic.is_interintra_used {
                    let ii = ii_preds
                        .expect("an inter-intra candidate implies the block's intrapred_buf");
                    if !ii.luma16.is_empty() {
                        let n = w * h;
                        let mode = svtav1_dsp::port_interintra::InterIntraMode::ALL
                            [ic.interintra_mode as usize];
                        let mut inter = vec![0u16; n];
                        inter.copy_from_slice(&scratch10);
                        svtav1_dsp::port_interintra::combine_interintra_highbd(
                            &im.ii_masks,
                            &im.wedge_masks,
                            mode,
                            ic.use_wedge_interintra,
                            ic.interintra_wedge_index.max(0) as usize,
                            0,
                            bsize as usize,
                            bsize as usize,
                            &mut scratch10,
                            w,
                            &inter,
                            w,
                            &ii.luma16[ic.interintra_mode as usize * n..],
                            w,
                        );
                    }
                }
                if let Some(spans) = &obmc_spans {
                    crate::obmc_pred_arm::predict_obmc_in_place_hbd(
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
                        im.bit_depth,
                        &mut scratch10,
                        w,
                        &mut [],
                        &mut [],
                        0,
                    );
                }
            }
            let mut sse = svtav1_dsp::hbd::full_distortion_kernel16_bits(
                &b.y_src10, 0, w, &scratch10, 0, w, w, h,
            );
            // `model_rd_for_sb`'s `sse += get_svt_psy_full_dist(..., is_hbd)`
            // (enc_inter_prediction.c:2012-2022): u16 source vs the 10-bit
            // inter prediction, `energy_gap << 2` scaled. GhostRobot-only at
            // this scope (Hybrid3115 keeps its pinned surface).
            if im.ifs.ac_bias_eff != 0.0
                && fx.frame().reference == crate::reference::SvtReference::GhostRobot
            {
                sse += svtav1_dsp::ac_bias::psy_full_dist_hbd(
                    &b.y_src10,
                    0,
                    w,
                    &scratch10,
                    0,
                    w,
                    w,
                    h,
                    im.ifs.ac_bias_eff,
                );
            }
            let (rate, dist) = svtav1_dsp::port_model_rd::model_rd_for_sb(
                &[bsize],
                &[sse],
                0,
                0,
                ifs_quantizer,
                im.bit_depth,
            );
            return IfsCandidateCost {
                switchable_rate,
                rate,
                dist: i64::try_from(dist).expect("model_rd distortion fits int64_t, as in C"),
            };
        }
        // :2130 `svt_aom_inter_prediction` (luma only, PICTURE_BUFFER_DESC_LUMA_MASK).
        // C's call warps each reference by ITS OWN model (`av1_inter_prediction`,
        // enc_inter_prediction.c:3276) — a GLOBAL_GLOBALMV candidate whose IFS
        // gate passed because ONE ref is translational still warps the other.
        if let Some(p1) = padded1 {
            let iw = [
                crate::inter_pred_arm::inter_pred_uses_warp(
                    ic.motion_mode,
                    ic.mode as u8,
                    w,
                    h,
                    &ic.wm_params,
                ),
                crate::inter_pred_arm::inter_pred_uses_warp(
                    ic.motion_mode,
                    ic.mode as u8,
                    w,
                    h,
                    &ic.wm_params_l1,
                ),
            ];
            // C's `av1_inter_prediction` compound arm — masked and
            // distance-weighted candidates blend with the metadata they
            // will code (`comp_data`/`distwtd` above), so the trial scores
            // the prediction the bitstream will produce.
            let mut wm0 = ic.wm_params;
            let mut wm1 = ic.wm_params_l1;
            crate::inter_pred_arm::predict_inter_yuv_compound_md(
                [(&padded.y, None), (&p1.y, None)],
                &mut wm0,
                &mut wm1,
                iw,
                comp_data,
                distwtd,
                &im.wedge_masks,
                abs_x,
                abs_y,
                w,
                h,
                bsize as usize,
                ic.mv,
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
        } else {
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
            // `inter_intra_prediction` (enc_inter_prediction.c:3488) —
            // the LUMA arm; OBMC is unreachable for an II candidate
            // (`motion_mode` is SIMPLE_TRANSLATION by injection).
            if ic.is_interintra_used {
                let ii =
                    ii_preds.expect("an inter-intra candidate implies the block's intrapred_buf");
                let n = w * h;
                let mode =
                    svtav1_dsp::port_interintra::InterIntraMode::ALL[ic.interintra_mode as usize];
                let mut inter = vec![0u8; n];
                inter.copy_from_slice(&scratch);
                svtav1_dsp::port_interintra::combine_interintra(
                    &im.ii_masks,
                    &im.wedge_masks,
                    mode,
                    ic.use_wedge_interintra,
                    ic.interintra_wedge_index.max(0) as usize,
                    0,
                    bsize as usize,
                    bsize as usize,
                    &mut scratch,
                    w,
                    &inter,
                    w,
                    &ii.luma[ic.interintra_mode as usize * n..],
                    w,
                );
            }
        }
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
        if let Some(p1) = padded1 {
            // COMPOUND: never sub-8 (`allow_bipred` rejects width/height
            // 4). Each reference picks warp or convolve by its OWN model
            // (C's per-ref `is_wm`, enc_inter_prediction.c:3276), and the
            // candidate's `interinter_comp` metadata drives ref 1's
            // masked/distance-weighted blend — the same
            // `av1_inter_prediction` compound arm the injection-time
            // prediction now uses.
            let iw = [
                crate::inter_pred_arm::inter_pred_uses_warp(
                    ic.motion_mode,
                    ic.mode as u8,
                    w,
                    h,
                    &ic.wm_params,
                ),
                crate::inter_pred_arm::inter_pred_uses_warp(
                    ic.motion_mode,
                    ic.mode as u8,
                    w,
                    h,
                    &ic.wm_params_l1,
                ),
            ];
            let mut wm0 = ic.wm_params;
            let mut wm1 = ic.wm_params_l1;
            crate::inter_pred_arm::predict_inter_yuv_compound_md(
                [
                    (
                        &padded.y,
                        padded.uv.as_ref().map(|(u, v)| (u, v)).filter(|_| g.has_uv),
                    ),
                    (
                        &p1.y,
                        p1.uv.as_ref().map(|(u, v)| (u, v)).filter(|_| g.has_uv),
                    ),
                ],
                &mut wm0,
                &mut wm1,
                iw,
                comp_data,
                distwtd,
                &im.wedge_masks,
                abs_x,
                abs_y,
                w,
                h,
                bsize as usize,
                ic.mv,
                ic.interp_filters,
                im.sb_size,
                im.frame_w,
                im.frame_h,
                pred,
                w,
                &mut ic.u_pred,
                &mut ic.v_pred,
                cw,
            );
        } else {
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
        }
        // `inter_intra_prediction` (enc_inter_prediction.c:3488-3500 ->
        // inter_prediction.c:2217+): the blend runs inside
        // `av1_inter_prediction` — after the block's own prediction, before
        // OBMC — and covers LUMA + CHROMA here (`component_mask` is full at
        // MDS3). The chroma intra planes come from the same `ii_preds`
        // store — identical to the per-call recompute C does, since the
        // recon they read cannot change inside one block's mode decision.
        if ic.is_interintra_used {
            let ii = ii_preds.expect("an inter-intra candidate implies the block's intrapred_buf");
            let n = w * h;
            let mode =
                svtav1_dsp::port_interintra::InterIntraMode::ALL[ic.interintra_mode as usize];
            let mut inter = vec![0u8; n];
            inter.copy_from_slice(&pred[..n]);
            svtav1_dsp::port_interintra::combine_interintra(
                &im.ii_masks,
                &im.wedge_masks,
                mode,
                ic.use_wedge_interintra,
                ic.interintra_wedge_index.max(0) as usize,
                0,
                bsize as usize,
                bsize as usize,
                pred,
                w,
                &inter,
                w,
                &ii.luma[ic.interintra_mode as usize * n..],
                w,
            );
            if g.has_uv {
                let plane_bsize = svtav1_dsp::port_obmc_data::get_plane_block_size(bsize, 1, 1)
                    .expect("an inter block's chroma has a real plane_bsize")
                    as usize;
                let chh = h.max(8) / 2;
                let cn = cw * chh;
                for (dst, plane_ii) in [(&mut ic.u_pred, &ii.u), (&mut ic.v_pred, &ii.v)] {
                    let mut inter_uv = vec![0u8; cn];
                    inter_uv.copy_from_slice(&dst[..cn]);
                    svtav1_dsp::port_interintra::combine_interintra(
                        &im.ii_masks,
                        &im.wedge_masks,
                        mode,
                        ic.use_wedge_interintra,
                        ic.interintra_wedge_index.max(0) as usize,
                        0,
                        bsize as usize,
                        plane_bsize,
                        dst,
                        cw,
                        &inter_uv,
                        cw,
                        &plane_ii[ic.interintra_mode as usize * cn..],
                        cw,
                    );
                }
            }
        }
        // C re-applies the OBMC blend on EVERY prediction — it is the tail of
        // `svt_aom_inter_prediction` (:3511), not a one-off at injection — so
        // a rebuilt prediction that skipped it would carry unblended edges
        // into the winner's recon. `av1_is_interp_needed_md` (rd_cost.h:71)
        // excludes WARPED_CAUSAL and non-translational global motion but NOT
        // OBMC, so this arm is reachable.
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
        // `obmc_spans` is computed before the search — the 10-bit trial
        // blends the SAME neighbours the 8-bit arm does.
        if let Some(spans) = &obmc_spans {
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
        // The TRUE-10-bit twin of the prediction the u8 arm just rebuilt.
        // `cand.pred10` / `ic.{u,v}_pred10` still hold the INJECTOR's pair
        // (packed 0, EIGHTTAP_REGULAR both ways): the bd10 full-RD loop
        // computes its residual and recon against them, so a pair change
        // here would code `src10 - pred10(old_pair)` against a bitstream
        // that signals `ic.interp_filters` — the decoder adds that
        // residual to a DIFFERENT prediction and every subpel block
        // drifts. C never has two predictions to desynchronize (at hbd_md
        // the 16-bit reference IS the only picture); the port's u8/hbd
        // split makes this twin rebuild necessary. The dispatch mirrors
        // `inter_md_arm::predict_and_price`'s hbd arm — same functions,
        // same order, same OBMC tail.
        if let Some(hbd) = padded.hbd.as_ref() {
            if !pred10.is_empty() {
                let hbd1 = padded1.and_then(|p| p.hbd.as_ref());
                let want_uv10 = g.has_uv
                    && hbd.uv.is_some()
                    && hbd1.is_none_or(|h10| h10.uv.is_some())
                    && !ic.u_pred10.is_empty();
                fn uv_of<'p>(
                    p10: &'p crate::picture::PaddedRefHbd,
                    want: bool,
                ) -> Option<(
                    &'p crate::picture::PaddedPlaneHbd,
                    &'p crate::picture::PaddedPlaneHbd,
                )> {
                    if want {
                        p10.uv.as_ref().map(|(u, v)| (u, v))
                    } else {
                        None
                    }
                }
                if let Some(h1) = hbd1 {
                    let iw10 = [
                        crate::inter_pred_arm::inter_pred_uses_warp(
                            ic.motion_mode,
                            ic.mode as u8,
                            w,
                            h,
                            &ic.wm_params,
                        ),
                        crate::inter_pred_arm::inter_pred_uses_warp(
                            ic.motion_mode,
                            ic.mode as u8,
                            w,
                            h,
                            &ic.wm_params_l1,
                        ),
                    ];
                    // The 10-bit twin of the compound arm above — same
                    // per-ref warp decision, same masked/distwtd
                    // metadata, on the DPB's u16 planes.
                    let mut wm0 = ic.wm_params;
                    let mut wm1 = ic.wm_params_l1;
                    crate::inter_pred_arm::predict_inter_yuv_compound_md_hbd(
                        [
                            (&hbd.y, uv_of(hbd, want_uv10)),
                            (&h1.y, uv_of(h1, want_uv10)),
                        ],
                        &mut wm0,
                        &mut wm1,
                        iw10,
                        comp_data,
                        distwtd,
                        &im.wedge_masks,
                        abs_x,
                        abs_y,
                        w,
                        h,
                        bsize as usize,
                        ic.mv,
                        ic.interp_filters,
                        im.sb_size,
                        im.frame_w,
                        im.frame_h,
                        im.bit_depth,
                        &mut pred10[..],
                        w,
                        &mut ic.u_pred10,
                        &mut ic.v_pred10,
                        cw,
                    );
                } else {
                    // OBMC's base is the plain inter prediction; the blend
                    // is the tail below — same split as the 8-bit arm and
                    // `predict_and_price`'s hbd arm.
                    let mm_base = if obmc {
                        crate::port_entropy_inter::modes::MotionMode::SimpleTranslation
                    } else {
                        ic.motion_mode
                    };
                    let is_wm10 = crate::inter_pred_arm::inter_pred_uses_warp(
                        ic.motion_mode,
                        ic.mode as u8,
                        w,
                        h,
                        &ic.wm_params,
                    );
                    // Sub-8 chroma covers the parent 8x8 and is stitched
                    // from the covered cells' MVs (`inter_chroma_4xn_pred`)
                    // — the same split the u8 arm above makes.
                    crate::inter_pred_arm::predict_inter_leaf_hbd(
                        &hbd.y,
                        uv_of(hbd, want_uv10 && !sub8),
                        mm_base,
                        is_wm10,
                        ic.wm_params,
                        abs_x,
                        abs_y,
                        w,
                        h,
                        ic.mv[0],
                        ic.interp_filters,
                        im.sb_size,
                        im.frame_w,
                        im.frame_h,
                        im.bit_depth,
                        &mut pred10[..],
                        w,
                        &mut ic.u_pred10,
                        &mut ic.v_pred10,
                        cw,
                    );
                    if want_uv10 && sub8 {
                        crate::inter_md_arm::predict_inter_chroma_sub8_hbd(
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
                            im.bit_depth,
                            &mut ic.u_pred10,
                            &mut ic.v_pred10,
                            cw,
                        );
                    }
                }
                // `inter_intra_prediction`'s 10-bit arm — LUMA + CHROMA,
                // before the OBMC blend (C's :3488 sits inside
                // `av1_inter_prediction`, ahead of :3511's OBMC tail).
                if ic.is_interintra_used {
                    let ii = ii_preds
                        .expect("an inter-intra candidate implies the block's intrapred_buf");
                    let mode = svtav1_dsp::port_interintra::InterIntraMode::ALL
                        [ic.interintra_mode as usize];
                    if !ii.luma16.is_empty() {
                        let n = w * h;
                        let mut inter = vec![0u16; n];
                        inter.copy_from_slice(&pred10[..n]);
                        svtav1_dsp::port_interintra::combine_interintra_highbd(
                            &im.ii_masks,
                            &im.wedge_masks,
                            mode,
                            ic.use_wedge_interintra,
                            ic.interintra_wedge_index.max(0) as usize,
                            0,
                            bsize as usize,
                            bsize as usize,
                            &mut pred10[..],
                            w,
                            &inter,
                            w,
                            &ii.luma16[ic.interintra_mode as usize * n..],
                            w,
                        );
                    }
                    if want_uv10 && !ii.u16.is_empty() && !ii.v16.is_empty() {
                        let plane_bsize =
                            svtav1_dsp::port_obmc_data::get_plane_block_size(bsize, 1, 1)
                                .expect("an inter block's chroma has a real plane_bsize")
                                as usize;
                        let chh10 = h.max(8) / 2;
                        let cn = cw * chh10;
                        for (dst, plane_ii) in
                            [(&mut ic.u_pred10, &ii.u16), (&mut ic.v_pred10, &ii.v16)]
                        {
                            let mut inter_uv = vec![0u16; cn];
                            inter_uv.copy_from_slice(&dst[..cn]);
                            svtav1_dsp::port_interintra::combine_interintra_highbd(
                                &im.ii_masks,
                                &im.wedge_masks,
                                mode,
                                ic.use_wedge_interintra,
                                ic.interintra_wedge_index.max(0) as usize,
                                0,
                                bsize as usize,
                                plane_bsize,
                                dst,
                                cw,
                                &inter_uv,
                                cw,
                                &plane_ii[ic.interintra_mode as usize * cn..],
                                cw,
                            );
                        }
                    }
                }
                if let Some(spans) = &obmc_spans {
                    crate::obmc_pred_arm::predict_obmc_in_place_hbd(
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
                        im.bit_depth,
                        &mut pred10[..],
                        w,
                        &mut ic.u_pred10,
                        &mut ic.v_pred10,
                        cw,
                    );
                }
            }
        }
    }
    // :2205-2208 withdraws `skip_mode_allowed` when the IFS result is a
    // non-zero (non-EIGHTTAP_REGULAR packed) filter pair — C opts to use IFS
    // over skip mode, so a searched non-default filter forfeits the
    // skip-mode arbitration at the full-cost stages. `skip_mode` clears
    // with it: in C this withdrawal runs during candidate prep, before ANY
    // `svt_aom_full_cost`, so a candidate reaching the stages always has
    // `skip_mode == false` here. The port's IFS runs inside MDS3 — AFTER
    // MDS1's skip-mode arm — so the flag can already be set; leaving it
    // would commit `skip_mode = 1` on a coefficient-bearing block (the
    // decoder reads no residual for it), which is a tile desync, not a
    // byte divergence. C's assert at mode_decision.c:3741 requires the
    // same pairing (a skm winner's `interp_filters == 0`).
    if ic.skip_mode_allowed && ic.interp_filters != 0 {
        ic.skip_mode_allowed = false;
        ic.skip_mode = false;
    }
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
