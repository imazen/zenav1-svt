use super::*;

/// One injected candidate -> its prediction and C's MDS0 rate.
pub(super) fn predict_and_price(
    f: &InterMdFrame<'_>,
    b: &InterBlockCtx<'_>,
    c: &crate::port_md::inject::InterCandidate,
    inter_mode_ctx: i16,
    stacks: &[crate::inter_mvp::InterMvpStack],
    lambda: u64,
) -> InterCandOut {
    // --- The motion-compensated prediction. C does luma and both chroma
    //     planes in ONE `av1_inter_prediction_light_pd1` call under a
    //     component mask, so this is one call (see `inter_pred_arm`).
    // C `block_mi.interp_filters` at injection: every C injector leaves it
    // at EIGHTTAP_REGULAR in both directions (packed 0), and the filter is
    // decided later by the interpolation-filter search at the stage
    // `ifs_ctrls.level` names — MDS3 on this port's ladders, run by
    // `leaf_funnel::ifs::ifs_at_mds3`. This prediction and the MDS0 rate
    // are therefore C's PRE-search values, exactly as they are in C.
    let interp_filters = 0u32;
    // The two crates carry their own `MotionMode` (the injector's lives in
    // `port_md::predicates`, the writer's and the rate's in
    // `port_entropy_inter::modes`); the discriminants are C's, so this is a
    // re-spelling. The assertion at the call site has already established
    // that only SimpleTranslation reaches here.
    let mm = match c.motion_mode {
        crate::port_md::predicates::MotionMode::SimpleTranslation => MotionMode::SimpleTranslation,
        crate::port_md::predicates::MotionMode::ObmcCausal => MotionMode::ObmcCausal,
        crate::port_md::predicates::MotionMode::WarpedCausal => MotionMode::WarpedCausal,
    };
    let mut y_pred = alloc::vec![0u8; b.bw * b.bh];
    // C `blk_geom->bwidth_uv` = `MAX(4, bwidth >> 1)` (utility.c:274), which
    // at 4:2:0 is the same extent as `get_plane_block_size(bsize, 1, 1)` and
    // as the funnel's own `cw`/`chh`. It was `b.bw / 2`, which is 2 on a
    // 4-wide block — half the chroma the funnel then reads, and the reason a
    // 4xN inter leaf indexed past the end of its own prediction.
    let (cw, chh) = (b.bw.max(8) / 2, b.bh.max(8) / 2);
    let (mut u_pred, mut v_pred) = if b.has_uv {
        (alloc::vec![0u8; cw * chh], alloc::vec![0u8; cw * chh])
    } else {
        (Vec::new(), Vec::new())
    };
    // C `svt_aom_get_ref_pic_buffer(pcs, rf[0])` — the candidate's OWN
    // reference picture. A missing entry is a caller bug: the injector can
    // only produce a reference that was in `ref_frame_type_arr`, and the
    // pipeline fills the table for every entry it puts there.
    let padded = f.padded_by_ref[c.ref_frame[0].max(0) as usize].unwrap_or_else(|| {
        panic!(
            "an inter candidate names reference {} with no DPB picture — \
             `ref_frame_type_arr` and `padded_by_ref` disagree",
            c.ref_frame[0]
        )
    });
    // C `has_second_ref(&mbmi->block_mi)` — spec 7.10.1: `rf[1] >
    // INTRA_FRAME`. An INTER-INTRA candidate's `ref_frame[1]` is
    // INTRA_FRAME (0), which is NOT a second reference — `> NONE_FRAME`
    // would index `padded_by_ref[0]` for a picture that is not a
    // reference at all.
    let is_compound = c.ref_frame[1] > crate::inter_mvp::INTRA_FRAME;
    let padded1 = is_compound.then(|| {
        f.padded_by_ref[c.ref_frame[1].max(0) as usize].unwrap_or_else(|| {
            panic!(
                "an inter candidate names reference {} with no DPB picture — \
                 `ref_frame_type_arr` and `padded_by_ref` disagree",
                c.ref_frame[1]
            )
        })
    });
    // The WARP driver takes a DIFFERENT C path — `av1_inter_prediction`'s
    // `is_wm` arm, not `av1_inter_prediction_light_pd1` — so it is dispatched
    // here rather than flagged inside the translation adapter. See
    // `inter_pred_arm::predict_inter_yuv_warped`.
    //
    // `is_wm` is C's OWN two-term condition, not just the motion mode: a
    // GLOBALMV candidate is injected as SIMPLE_TRANSLATION, and with a model
    // above TRANSLATION the decoder still warps it.
    // `is_wm` is PER REFERENCE in C (`av1_inter_prediction`,
    // enc_inter_prediction.c:3276): ref `i` warps when ITS model is above
    // TRANSLATION. A GLOBAL_GLOBALMV candidate keeps `wm_params_l0`/`l1`
    // and both refs are evaluated against their own model — the
    // single-model check used to route the whole block through the warp
    // leaf with only ref 0's plane, which silently dropped the second
    // reference's prediction (a decoder averages BOTH, so every committed
    // GLOBAL_GLOBALMV block's recon disagreed with the stream).
    let is_wm =
        crate::inter_pred_arm::inter_pred_uses_warp(mm, c.mode as u8, b.bw, b.bh, &c.wm_params_l0);
    let is_wm1 = padded1.is_some()
        && crate::inter_pred_arm::inter_pred_uses_warp(
            mm,
            c.mode as u8,
            b.bw,
            b.bh,
            &c.wm_params_l1,
        );
    // `block_mi.interinter_comp` → the masked arm's `InterInterCompoundData`
    // + the DIFFWTD `mask_type`, and `dist_wtd_comp_weight_assign`'s output
    // for ref 1's convolve — shared with the IFS/MDS3 rebuild via
    // [`compound_md_args`].
    let (comp_data, distwtd) = compound_md_args(
        f,
        c.ref_frame,
        c.interinter_comp_type,
        c.interinter_mask_type,
        c.interinter_wedge_index,
        c.interinter_wedge_sign,
        c.compound_idx,
    );
    if let Some(p1) = padded1 {
        // COMPOUND — C's `av1_inter_prediction` compound arm: each ref
        // picks warp or convolve by its OWN model over one shared
        // CONV_BUF, ref 1 takes the distance-weighted offsets and the
        // masked blend when `interinter_comp.type` is WEDGE/DIFFWTD.
        let mut wm0 = c.wm_params_l0;
        let mut wm1 = c.wm_params_l1;
        crate::inter_pred_arm::predict_inter_yuv_compound_md(
            [
                (
                    &padded.y,
                    padded.uv.as_ref().map(|(u, v)| (u, v)).filter(|_| b.has_uv),
                ),
                (
                    &p1.y,
                    p1.uv.as_ref().map(|(u, v)| (u, v)).filter(|_| b.has_uv),
                ),
            ],
            &mut wm0,
            &mut wm1,
            [is_wm, is_wm1],
            comp_data,
            distwtd,
            &f.wedge_masks,
            b.org_x,
            b.org_y,
            b.bw,
            b.bh,
            b.bsize as usize,
            c.mv,
            interp_filters,
            f.sb_size,
            f.frame_w,
            f.frame_h,
            &mut y_pred,
            b.bw,
            &mut u_pred,
            &mut v_pred,
            cw,
        );
    } else if is_wm {
        let mut wm = c.wm_params_l0;
        crate::inter_pred_arm::predict_inter_yuv_warped(
            (
                &padded.y,
                padded.uv.as_ref().map(|(u, v)| (u, v)).filter(|_| b.has_uv),
            ),
            &mut wm,
            b.org_x,
            b.org_y,
            b.bw,
            b.bh,
            c.mv[0],
            interp_filters,
            f.sb_size,
            f.frame_w,
            f.frame_h,
            &mut y_pred,
            b.bw,
            &mut u_pred,
            &mut v_pred,
            cw,
        );
    } else {
        let sub8 = b.bw < 8 || b.bh < 8;
        match (b.has_uv && !sub8, padded.uv.as_ref()) {
            (true, Some((refu, refv))) => crate::inter_pred_arm::predict_inter_yuv(
                (&padded.y, refu, refv),
                b.org_x,
                b.org_y,
                b.bw,
                b.bh,
                c.mv[0],
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
                &padded.y,
                b.org_x,
                b.org_y,
                b.bw,
                b.bh,
                c.mv[0],
                interp_filters,
                f.sb_size,
                f.frame_w,
                f.frame_h,
                &mut y_pred,
                b.bw,
            ),
        }
        // C `av1_inter_prediction`'s SUB-8 chroma arm: a 4xN / Nx4 block's
        // chroma covers the parent 8x8, so C stitches it from the covered
        // cells' own MVs (`inter_chroma_4xn_pred`) and falls back to this
        // block's MV over the whole area when one of them is intra. Neither
        // is what the luma-shaped `predict_inter_yuv` above does, which is why
        // the sub-8 case is split out of it rather than folded in.
        if b.has_uv && sub8 {
            predict_inter_chroma_sub8(
                &f.padded_by_ref,
                b.grid,
                b.grid_stride,
                b.org_x,
                b.org_y,
                b.bw,
                b.bh,
                c.ref_frame[0],
                c.mv[0],
                interp_filters,
                f.sb_size,
                f.frame_w,
                f.frame_h,
                &mut u_pred,
                &mut v_pred,
                cw,
            );
        }
    }

    // ---- Inter-intra, C `av1_inter_prediction`'s tail ----
    // `inter_intra_prediction` (enc_inter_prediction.c:3468-3478 ->
    // inter_prediction.c:2217+) blends the block's OWN prediction with the
    // precomputed intra prediction under the smooth (or wedge) mask —
    // INSIDE `av1_inter_prediction`, so it runs before the OBMC tail
    // below, per plane. `intrapred_buf` is the block's, which
    // `precompute_intra_pred_for_inter_intra` fills only for an
    // inter-intra-eligible block — an II candidate on a block without it
    // is a wiring bug, not a slow path.
    if c.is_interintra_used {
        let ii = b
            .ii
            .as_ref()
            .expect("an inter-intra candidate reached prediction on a block with no intrapred_buf");
        let n = b.bw * b.bh;
        let mode = svtav1_dsp::port_interintra::InterIntraMode::ALL[c.interintra_mode as usize];
        let mut inter = alloc::vec![0u8; n];
        inter.copy_from_slice(&y_pred);
        svtav1_dsp::port_interintra::combine_interintra(
            &f.ii_masks,
            &f.wedge_masks,
            mode,
            c.use_wedge_interintra,
            c.interintra_wedge_index as usize,
            // C `INTERINTRA_WEDGE_SIGN` — the wedge search is run at a
            // FIXED sign (pick_wedge_fixed_sign), so the blend reads the
            // same sign it was picked at; it is not a coded symbol.
            0,
            b.bsize as usize,
            b.bsize as usize,
            &mut y_pred,
            b.bw,
            &inter,
            b.bw,
            &ii.luma[c.interintra_mode as usize * n..],
            b.bw,
        );
        if b.has_uv {
            // C :2217's per-plane loop — `plane_bsize` is the chroma
            // plane's own block size while `bsize` stays the LUMA one
            // (the wedge mask's sub-sampling test reads it).
            let plane_bsize = svtav1_dsp::port_obmc_data::get_plane_block_size(
                svtav1_types::block::BlockSize::from_u8(b.bsize)
                    .expect("an injected inter block must have a real BlockSize"),
                1,
                1,
            )
            .expect("an inter block's chroma has a real plane_bsize")
                as usize;
            let cn = cw * chh;
            for (pred, plane_ii) in [(&mut u_pred, &ii.u), (&mut v_pred, &ii.v)] {
                let mut inter_uv = alloc::vec![0u8; cn];
                inter_uv.copy_from_slice(pred);
                svtav1_dsp::port_interintra::combine_interintra(
                    &f.ii_masks,
                    &f.wedge_masks,
                    mode,
                    c.use_wedge_interintra,
                    c.interintra_wedge_index as usize,
                    0,
                    b.bsize as usize,
                    plane_bsize,
                    pred,
                    cw,
                    &inter_uv,
                    cw,
                    &plane_ii[c.interintra_mode as usize * cn..],
                    cw,
                );
            }
        }
    }

    // ---- OBMC, C `svt_aom_inter_prediction`'s tail (:3511) ----
    // The blend runs AFTER the block's own prediction and rewrites its edges
    // in place, so it sits here rather than as a third arm of the dispatch
    // above. It is re-applied wherever the prediction is rebuilt -- see
    // `leaf_funnel::ifs`. The spans are hoisted so the 10-bit arm below
    // blends the SAME neighbours the 8-bit one does.
    let obmc_spans = (mm == MotionMode::ObmcCausal).then(|| {
        obmc_nb_spans(
            b.grid,
            b.grid_stride,
            f.mi_rows,
            f.mi_cols,
            b.org_x,
            b.org_y,
            b.bw,
            b.bh,
        )
    });
    if let Some(spans) = &obmc_spans {
        crate::obmc_pred_arm::predict_obmc_in_place(
            &crate::obmc_pred_arm::ObmcCtx {
                padded_by_ref: &f.padded_by_ref,
                above_row: &spans.above[..spans.n_above],
                left_col: &spans.left[..spans.n_left],
                up_available: b.org_y > 0,
                left_available: b.org_x > 0,
                mi_cols: f.mi_cols.max(0) as usize,
                mi_rows: f.mi_rows.max(0) as usize,
                sb_size: f.sb_size,
                frame_w: f.frame_w,
                frame_h: f.frame_h,
                edges: crate::inter_pred_arm::block_mb_edges(
                    b.org_x, b.org_y, b.bw, b.bh, f.frame_w, f.frame_h,
                ),
            },
            svtav1_types::block::BlockSize::from_u8(b.bsize)
                .expect("an injected inter block must have a real BlockSize"),
            b.org_x,
            b.org_y,
            b.bw,
            b.bh,
            &mut y_pred,
            b.bw,
            &mut u_pred,
            &mut v_pred,
            cw,
        );
    }

    // The SAME prediction at true 10 bits, when the DPB carries a 10-bit twin
    // of this reference. C does not do this twice — at `bd > EB_EIGHT_BIT` its
    // reference IS the 16-bit picture and the 8-bit call above does not exist.
    // The port keeps both because its u8 mode-decision stages still read
    // `y_pred`, and the bd10 full-RD funnel reads `y_pred10`.
    let (mut y_pred10, mut u_pred10, mut v_pred10) = (Vec::new(), Vec::new(), Vec::new());
    if let Some(hbd) = padded.hbd.as_ref() {
        y_pred10 = alloc::vec![0u16; b.bw * b.bh];
        let hbd1 = padded1.and_then(|p| p.hbd.as_ref());
        let want_uv = b.has_uv && hbd.uv.is_some() && hbd1.is_none_or(|h| h.uv.is_some());
        if want_uv {
            u_pred10 = alloc::vec![0u16; cw * chh];
            v_pred10 = alloc::vec![0u16; cw * chh];
        }
        if let Some(h1) = hbd1 {
            // The SAME compound arm at 10 bits — `predict_inter_yuv_
            // compound_md`'s `SrcPlanes::Hbd` twin.
            let mut wm0 = c.wm_params_l0;
            let mut wm1 = c.wm_params_l1;
            crate::inter_pred_arm::predict_inter_yuv_compound_md_hbd(
                [
                    (
                        &hbd.y,
                        hbd.uv.as_ref().map(|(u, v)| (u, v)).filter(|_| want_uv),
                    ),
                    (
                        &h1.y,
                        h1.uv.as_ref().map(|(u, v)| (u, v)).filter(|_| want_uv),
                    ),
                ],
                &mut wm0,
                &mut wm1,
                [is_wm, is_wm1],
                comp_data,
                distwtd,
                &f.wedge_masks,
                b.org_x,
                b.org_y,
                b.bw,
                b.bh,
                b.bsize as usize,
                c.mv,
                interp_filters,
                f.sb_size,
                f.frame_w,
                f.frame_h,
                f.bit_depth,
                &mut y_pred10,
                b.bw,
                &mut u_pred10,
                &mut v_pred10,
                cw,
            );
        } else {
            // C's OBMC block predicts with the PLAIN inter path first and
            // blends the neighbours in after — `predict_inter_leaf_hbd`'s
            // OBMC assert guards the committed-leaf post-pass, so the base
            // prediction here is taken as SIMPLE_TRANSLATION and the blend
            // is applied below, exactly as the 8-bit arm does.
            let mm_base = if mm == MotionMode::ObmcCausal {
                MotionMode::SimpleTranslation
            } else {
                mm
            };
            // A sub-8 luma block's chroma covers the parent 8x8 and is
            // stitched from the covered cells' own MVs
            // (`inter_chroma_4xn_pred`), exactly as the 8-bit arm's
            // `predict_inter_chroma_sub8` — the luma-shaped hbd leaf would
            // predict `bw/2 x bh/2` at the block's own origin, which is
            // neither the right area nor the right motion.
            let sub8 = b.bw < 8 || b.bh < 8;
            crate::inter_pred_arm::predict_inter_leaf_hbd(
                &hbd.y,
                if want_uv && !sub8 {
                    hbd.uv.as_ref().map(|(u, v)| (u, v))
                } else {
                    None
                },
                mm_base,
                is_wm,
                c.wm_params_l0,
                b.org_x,
                b.org_y,
                b.bw,
                b.bh,
                c.mv[0],
                interp_filters,
                f.sb_size,
                f.frame_w,
                f.frame_h,
                f.bit_depth,
                &mut y_pred10,
                b.bw,
                &mut u_pred10,
                &mut v_pred10,
                cw,
            );
            if want_uv && sub8 {
                predict_inter_chroma_sub8_hbd(
                    &f.padded_by_ref,
                    b.grid,
                    b.grid_stride,
                    b.org_x,
                    b.org_y,
                    b.bw,
                    b.bh,
                    c.ref_frame[0],
                    c.mv[0],
                    interp_filters,
                    f.sb_size,
                    f.frame_w,
                    f.frame_h,
                    f.bit_depth,
                    &mut u_pred10,
                    &mut v_pred10,
                    cw,
                );
            }
        }
        // The SAME inter-intra blend at 10 bits — `combine_interintra_
        // highbd` on the u16 `pred10` buffers, still inside C's
        // `av1_inter_prediction` tail so the OBMC blend below applies on
        // top of it. `luma16` is empty when the block has no u16 recon
        // canvas (M6-M8 bd10): `pred10` has no consumer there — C's
        // hbd_md=0 produces no u16 inter-intra prediction either — so the
        // blend is skipped rather than faked.
        if c.is_interintra_used {
            let ii = b.ii.as_ref().expect(
                "an inter-intra candidate reached prediction on a block with no intrapred_buf",
            );
            if !ii.luma16.is_empty() {
                let n = b.bw * b.bh;
                let mode =
                    svtav1_dsp::port_interintra::InterIntraMode::ALL[c.interintra_mode as usize];
                let mut inter = alloc::vec![0u16; n];
                inter.copy_from_slice(&y_pred10);
                svtav1_dsp::port_interintra::combine_interintra_highbd(
                    &f.ii_masks,
                    &f.wedge_masks,
                    mode,
                    c.use_wedge_interintra,
                    c.interintra_wedge_index as usize,
                    0,
                    b.bsize as usize,
                    b.bsize as usize,
                    &mut y_pred10,
                    b.bw,
                    &inter,
                    b.bw,
                    &ii.luma16[c.interintra_mode as usize * n..],
                    b.bw,
                );
                if want_uv && !ii.u16.is_empty() && !ii.v16.is_empty() {
                    let plane_bsize = svtav1_dsp::port_obmc_data::get_plane_block_size(
                        svtav1_types::block::BlockSize::from_u8(b.bsize)
                            .expect("an injected inter block must have a real BlockSize"),
                        1,
                        1,
                    )
                    .expect("an inter block's chroma has a real plane_bsize")
                        as usize;
                    let cn = cw * chh;
                    for (pred, plane_ii) in [(&mut u_pred10, &ii.u16), (&mut v_pred10, &ii.v16)] {
                        let mut inter_uv = alloc::vec![0u16; cn];
                        inter_uv.copy_from_slice(pred);
                        svtav1_dsp::port_interintra::combine_interintra_highbd(
                            &f.ii_masks,
                            &f.wedge_masks,
                            mode,
                            c.use_wedge_interintra,
                            c.interintra_wedge_index as usize,
                            0,
                            b.bsize as usize,
                            plane_bsize,
                            pred,
                            cw,
                            &inter_uv,
                            cw,
                            &plane_ii[c.interintra_mode as usize * cn..],
                            cw,
                        );
                    }
                }
            }
        }
        // The SAME OBMC blend at 10 bits (`av1_inter_prediction_obmc` with
        // `is16bit`): the neighbours' predictions are rebuilt from each
        // reference's `hbd` twin and the blend is the u16 hmask/vmask pair.
        if let Some(spans) = &obmc_spans {
            crate::obmc_pred_arm::predict_obmc_in_place_hbd(
                &crate::obmc_pred_arm::ObmcCtx {
                    padded_by_ref: &f.padded_by_ref,
                    above_row: &spans.above[..spans.n_above],
                    left_col: &spans.left[..spans.n_left],
                    up_available: b.org_y > 0,
                    left_available: b.org_x > 0,
                    mi_cols: f.mi_cols.max(0) as usize,
                    mi_rows: f.mi_rows.max(0) as usize,
                    sb_size: f.sb_size,
                    frame_w: f.frame_w,
                    frame_h: f.frame_h,
                    edges: crate::inter_pred_arm::block_mb_edges(
                        b.org_x, b.org_y, b.bw, b.bh, f.frame_w, f.frame_h,
                    ),
                },
                svtav1_types::block::BlockSize::from_u8(b.bsize)
                    .expect("an injected inter block must have a real BlockSize"),
                b.org_x,
                b.org_y,
                b.bw,
                b.bh,
                f.bit_depth,
                &mut y_pred10,
                b.bw,
                &mut u_pred10,
                &mut v_pred10,
                cw,
            );
        }
    }

    // --- C's real MDS0 rate, `svt_aom_inter_fast_cost` (rd_cost.c:1005).
    //
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
    // C indexes `ref_mv_stack` and `ref_frames_num_bits` by the candidate's
    // `MvReferenceFrame` TYPE — `LAST_FRAME` for a single reference, the
    // compound index (8..) for a pair. `av1_ref_frame_type` collapses the
    // candidate's `{rf0, rf1}` pair back to that index.
    let ref_type = crate::inter_mvp::av1_ref_frame_type(c.ref_frame);
    let ref_bits = crate::port_md::ref_frame_rate::estimate_ref_frames_num_bits(
        &[ref_type],
        &counts,
        rr_above,
        rr_left,
        f.reference_mode_is_select,
        b.bw as u16,
        b.bh as u16,
        &f.ref_fac,
        crate::inter_mvp::av1_set_ref_frame,
    );
    let ref_frames_num_bits = ref_bits.first().map_or(0, |&(_, bits)| bits);

    let bsize = svtav1_types::block::BlockSize::from_u8(b.bsize)
        .expect("an injected inter block must have a real BlockSize");
    let stack = &stacks[ref_type.max(0) as usize];
    // C `ctx->skip_mode_ctx` = `av1_get_skip_mode_context(xd)`
    // (`entropy_coding.c:1097`), the same neighbour pair the writer uses.
    // Per-block: priced at MDS0 and re-read by the funnel stages' skip-mode
    // arbitration off the candidate.
    let skip_mode_ctx = crate::port_entropy_inter::modes::skip_mode_context(&b.neighbors);
    let cost = inter_fast_cost(
        &f.cost_frame(),
        &InterBlock {
            bsize,
            skip_mode_ctx,
            is_inter_ctx: b.is_inter_ctx,
            inter_mode_ctx,
            ref_mv_count: stack.count,
            ref_mv_stack: &stack.stack,
            ref_frames_num_bits,
            neighbors: &b.neighbors,
            overlappable_neighbors: b.overlappable_neighbors,
            // C `ctx->approx_inter_rate` — `sig_deriv_enc_dec_default`
            // copies `pcs->approx_inter_rate` (enc_mode_config.c:7906);
            // see the InjectCtx note above.
            approx_inter_rate: f.search.approx_inter_rate,
            // C prices the interpolation filter at MDS0 only when
            // `ctx->ifs_ctrls.level == IFS_MDS0` (rd_cost.c:1179).
            //
            // This was hard-coded TRUE on the reasoning that "this port runs
            // no filter search, so the filter IS known and is priced". The
            // reasoning is about a DIFFERENT gap: C's level here is
            // `IFS_MDS1` or `IFS_MDS3` (`interpolation_search_level` is 2 at
            // MR and 4 above it, never 1), so C does not price the filter at
            // MDS0 either — it prices it after the search it runs and this
            // port does not. Paying it early is not "pricing what C prices
            // later"; it is a DIFFERENT MDS0 ordering. MEASURED 2026-09-02
            // against C's `svt_aom_inter_fast_cost` (`SVT_IFCOST_OUT`) on
            // `uniform 72x72 q20 p8`: 20 to 109 rate units on every inter
            // candidate, on top of the 1207 the inverted `is_inter_ctx`
            // cost. The filter is priced where C prices it — after the MDS3
            // search, `fast_luma_rate += switchable_rate`
            // (enc_inter_prediction.c:2211) — by `leaf_funnel::ifs`.
            ifs_at_mds0: f.search.ifs_at_mds0,
        },
        &InterCandidate {
            mode: c.mode,
            ref_frame: c.ref_frame,
            mv: c.mv,
            pred_mv: c.pred_mv,
            drl_index: c.drl_index,
            interp_filters,
            motion_mode: mm,
            num_proj_ref: u16::from(c.num_proj_ref),
            is_interintra_used: c.is_interintra_used,
            interintra_mode: c.interintra_mode,
            use_wedge_interintra: c.use_wedge_interintra,
            interintra_wedge_index: c.interintra_wedge_index.max(0) as u8,
            comp_group_idx: c.comp_group_idx,
            compound_idx: c.compound_idx,
            interinter_comp_type: match c.interinter_comp_type {
                1 => svtav1_types::prediction::CompoundType::DistWtd,
                2 => svtav1_types::prediction::CompoundType::Wedge,
                3 => svtav1_types::prediction::CompoundType::DiffWtd,
                _ => svtav1_types::prediction::CompoundType::Average,
            },
            interinter_wedge_index: c.interinter_wedge_index.max(0) as u8,
            skip_mode_allowed: c.skip_mode_allowed,
        },
        lambda,
        0,
        Some(&f.nmv),
        &f.fac,
    );

    // The FIELD JOIN against C's `SVT_CINTER_OUT` line, which carries exactly
    // these inputs (`imc=`, `drl=`, `mv0=`, `pmv0=`, `ovl=`, `rf=`) plus the
    // decision C made with them. It exists because the funnel's `NSQDBG CAND`
    // line reports only the FINISHED rate: on `uniform 72x72 q20 p8` frame 1
    // the port priced the 8x8 corner block's NEARESTMV at `flr = 3014` and
    // chose intra where C codes inter, and nothing in the repo could say
    // which of the six inputs to `svt_aom_inter_fast_cost` differed. A total
    // is one number; C's dump has six fields, so print six.
    //
    // Gated on SVTAV1_CANDDBG + SVTAV1_NSQDBG like every other funnel dump.
    #[cfg(feature = "std")]
    if crate::dbgenv::canddbg() && crate::depth_refine::nsqdbg_here(b.org_x, b.org_y) {
        std::eprintln!(
            "NSQDBG ICAND mi=({},{}) {}x{} mode={} rf={},{} mv0={},{} pmv0={},{} drl={} imc={} \
             ovl={} isinterctx={} nb=[{},{}] refmvcnt={} refbits={} flr={}",
            b.org_y / 4,
            b.org_x / 4,
            b.bw,
            b.bh,
            c.mode as u8,
            c.ref_frame[0],
            c.ref_frame[1],
            c.mv[0].y,
            c.mv[0].x,
            c.pred_mv[0].y,
            c.pred_mv[0].x,
            c.drl_index,
            inter_mode_ctx,
            b.overlappable_neighbors,
            b.is_inter_ctx,
            // The two neighbours' `ref_frame[0]`, which is what
            // `svt_av1_get_intra_inter_context` reads: `-9` for "not
            // available". Without them `isinterctx` is a verdict with no
            // premises, and the premise is the MD mi grid.
            b.neighbors.above_avail().map_or(-9, |m| m.ref_frame[0]),
            b.neighbors.left_avail().map_or(-9, |m| m.ref_frame[0]),
            stack.count,
            ref_frames_num_bits,
            cost.rate.luma,
        );
    }

    InterCandOut {
        mode: c.mode,
        ref_frame: c.ref_frame,
        mv: c.mv,
        pred_mv: c.pred_mv,
        drl_index: c.drl_index,
        interp_filters,
        motion_mode: mm,
        y_pred,
        u_pred,
        v_pred,
        y_pred10,
        u_pred10,
        v_pred10,
        wm_params_l0: c.wm_params_l0,
        wm_params_l1: c.wm_params_l1,
        fast_luma_rate: cost.rate.luma,
        fast_cost_rate: cost.charged_rate,
        num_proj_ref: c.num_proj_ref,
        // Stamped by `build_inter_candidates` once the block's
        // `merge_inter_cands` decision is known.
        cand_class: 0,
        comp_group_idx: c.comp_group_idx,
        compound_idx: c.compound_idx,
        interinter_comp_type: c.interinter_comp_type,
        interinter_mask_type: c.interinter_mask_type,
        interinter_wedge_index: c.interinter_wedge_index,
        interinter_wedge_sign: c.interinter_wedge_sign,
        is_interintra_used: c.is_interintra_used,
        interintra_mode: c.interintra_mode,
        use_wedge_interintra: c.use_wedge_interintra,
        interintra_wedge_index: c.interintra_wedge_index,
        skip_mode_allowed: c.skip_mode_allowed,
        skip_mode_ctx: skip_mode_ctx as u8,
    }
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
            skip_mode: e.skip_mode,
            skip: e.skip,
            comp_group_idx: e.comp_group_idx,
            compound_idx: e.compound_idx,
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
