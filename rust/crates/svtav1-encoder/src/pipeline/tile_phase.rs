use super::*;

impl EncodePipeline {
    #[inline(always)]
    pub(super) fn decide_and_encode_tiles(
        hdr: &crate::hdr_mode::HdrForkConfig,
        speed_config: &SpeedConfig,
        bit_depth: u8,
        true_width: u32,
        true_height: u32,
        thread_count: usize,
        reference: crate::reference::SvtReference,
        enhancements: crate::enhancements::ZenEnhancements,
        chroma_format: Option<svtav1_types::chroma::ChromaFormat>,
        last_recon10_final: &mut Option<(Vec<u16>, Vec<u16>, Vec<u16>)>,
        last_recon10_y: &mut Option<Vec<u16>>,
        last_recon10_uv: &mut Option<(Vec<u16>, Vec<u16>)>,
        fs: &FrameShape<'_>,
        hbd_source: &Option<HbdSource>,
        stale_vars: Option<Vec<crate::pd0::SbVariance>>,
        hbd_used: &mut bool,
        hbd_used_flag: core::sync::atomic::AtomicBool,
        stop: &almost_enough::StopToken,
        md_lambda_base_update_type: Option<crate::port_rc_process::FrameUpdateType>,
        md_lambda_factor_update_type: crate::port_rc_process::FrameUpdateType,
        md_alt_lambda_factors: bool,
        lambda_mod_intra: i64,
        encode_input: &Vec<u8>,
        sb_input: &[u8],
        in_stride: usize,
        sb_chroma_owned: &Option<(Vec<u8>, Vec<u8>)>,
        hbd_sb_owned: Option<(Vec<u16>, Vec<u16>, Vec<u16>)>,
        tpl_adjusted_qp: u8,
        picture_qp: u8,
        lw_bump: u32,
        tpl_rdmult: Option<alloc::sync::Arc<crate::port_md_lambda::TplRdmult>>,
        primary_ref_cdfs: &Option<alloc::sync::Arc<crate::port_frame_cdf::FrameCdfs>>,
        c_quant: &Option<alloc::sync::Arc<crate::quant::CodingQuantCfg>>,
        recon: &mut Vec<u8>,
        lambda: u64,
        ref_frame_data: Option<&[u8]>,
        mv_map_stride: usize,
        mv_map: Vec<svtav1_types::motion::Mv>,
        ac_bias_eff: f64,
        ssim_rdmult: Option<crate::tune::SsimRdmult>,
        sharp_tx_active: bool,
        inter_syntax_state: &Option<InterSyntaxState>,
        inter_mvp_env: &Option<crate::partition::InterMdEnv>,
        inter_md_frame: Option<crate::inter_md_arm::InterMdFrame<'_>>,
        sb_inter_lambda: Option<Vec<crate::pd0::SbInterLambda>>,
        pd0_dr_res: Option<Vec<crate::port_enc_mode_config::common::DepthRemovalResult>>,
        pd0_min_sq: Option<Vec<u8>>,
        pd0_det_frame: crate::part_arm::Pd0DetFrame<'_>,
        lpd1_frame: Option<Lpd1FrameIn>,
        ref_min_max_sq: Option<(Vec<u8>, Vec<u8>)>,
    ) -> Result<
        (
            Vec<crate::partition::PartitionTree>,
            Vec<u8>,
            Vec<u8>,
            usize,
            usize,
            usize,
            usize,
            core::cell::RefCell<Option<CodedAreaAcc>>,
            core::cell::RefCell<Option<crate::port_frame_cdf::FrameCdfs>>,
        ),
        whereat::prelude::At<EncodeError>,
    > {
        let FrameShape {
            chroma,
            is_key,
            sc_arm,
            temporal_layer,
            frame_hier,
            w,
            h,
            fmt,
            ss_x,
            ss_y,
            acw,
            ach,
            filter_chroma,
            sc_derivation,
            frame_tx_mode_select,
            base_qindex,
            coded_lossless,
            delta_q_plan,
            md_sb_qindex,
            sb_size,
            sb_cols,
            sb_rows,
            ref_padded_luma,
            tile_grid,
            chroma_deltas,
            qindex_u,
            qindex_v,
            qm_levels,
            seq_tools,
        } = *fs;
        let tile_recons = encode_tile_rows(
            encode_input,
            sb_input,
            in_stride,
            w,
            h,
            sb_size,
            sb_cols,
            sb_rows,
            tile_grid,
            base_qindex,
            qindex_u,
            qindex_v,
            ac_bias_eff,
            md_sb_qindex.map(|p| p.sb_qindex.as_slice()),
            // `frm_hdr.delta_q_params.delta_q_present` — the SIGNALLING
            // side. `md_sb_qindex` exists under `r0_delta_qp_md` even when
            // nothing is signalled, so it cannot proxy this flag.
            delta_q_plan.is_some(),
            tpl_rdmult.clone(),
            (chroma_deltas.u_ac, chroma_deltas.v_ac),
            sharp_tx_active,
            if hdr.is_fork() {
                hdr.noise_norm_strength
            } else {
                0
            },
            qm_levels,
            if hdr.is_fork() { hdr.tx_bias } else { 0 },
            hdr.is_fork() && hdr.complex_hvs == 1,
            // Share the frame's resolved detection with every tile.
            sc_derivation,
            sc_arm,
            hdr.is_fork() && hdr.alt_ssim_tuning,
            hdr.is_fork() && hdr.alt_lambda_factors,
            if hdr.tune == crate::tune::TUNE_IQ {
                Some(crate::tune::iq_lambda_weight(picture_qp as u32))
            } else if speed_config.preset == -1 {
                Some(0)
            } else {
                None
            },
            ssim_rdmult.as_ref(),
            base_qindex,
            frame_tx_mode_select,
            tpl_adjusted_qp,
            picture_qp,
            lw_bump,
            hdr.tune == crate::tune::TUNE_IQ,
            hdr.sharpness,
            lambda,
            &speed_config,
            ref_frame_data,
            crate::port_picstruct::is_highest_layer(temporal_layer, frame_hier),
            temporal_layer,
            // The padded twin of the plane above, from the SAME DPB slot.
            ref_padded_luma,
            inter_md_frame.as_ref(),
            inter_syntax_state.as_ref(),
            inter_mvp_env.as_ref(),
            pd0_min_sq.as_deref(),
            pd0_dr_res.as_deref(),
            // `ref_obj_l0->{sb_min_sq_size,sb_max_sq_size}` for the
            // use_ref_info refinement arm — LAST ref (see `last_ref_slot`),
            // `None` on a key frame.
            ref_min_max_sq
                .as_ref()
                .map(|(mn, mx)| (mn.as_slice(), mx.as_slice())),
            sb_inter_lambda.as_deref(),
            primary_ref_cdfs.as_deref(),
            &mv_map,
            mv_map_stride,
            // "chroma present AND the C-parity surface" — `filter_chroma`,
            // so the 4:2:0-only funnel never arms at 4:4:4.
            filter_chroma,
            chroma_format.unwrap_or(svtav1_types::chroma::ChromaFormat::Yuv420),
            c_quant.clone(),
            sb_chroma_owned
                .as_ref()
                .map(|(u, v)| (u.as_slice(), v.as_slice())),
            bit_depth,
            // Task #6 chunk 1: the native 10-bit source for the bd10 MD
            // funnel (`None` on every u8 path).
            // The SB-extent-padded twins when the frame has a partial SB (see
            // `hbd_sb_owned`), else the aligned planes — identical on every
            // 64-aligned frame.
            match hbd_sb_owned.as_ref() {
                Some((y, u, v)) => Some((y.as_slice(), u.as_slice(), v.as_slice())),
                None => hbd_source
                    .as_ref()
                    .map(|h| (h.y.as_slice(), h.u.as_slice(), h.v.as_slice())),
            },
            &hbd_used_flag,
            // Superres chunk B.4: C's stale full-res variance array.
            stale_vars.as_deref(),
            hdr.max_tx_size,
            coded_lossless,
            enhancements.contains(crate::enhancements::ZenEnhancement::DeepSearch),
            reference,
            thread_count,
            pd0_det_frame,
            lpd1_frame,
            stop,
        )?;
        *hbd_used |= hbd_used_flag.load(core::sync::atomic::Ordering::Relaxed);
        let mut tree_slots: Vec<Option<crate::partition::PartitionTree>> =
            (0..sb_cols * sb_rows).map(|_| None).collect();
        let mut sb_enc_rdoq: Vec<bool> =
            vec![c_quant.as_ref().map_or(false, |q| q.rdoq_level != 0); sb_cols * sb_rows];
        let mut canvas10: Option<(Vec<u16>, Vec<u16>, Vec<u16>)> = tile_recons
            .first()
            .and_then(|t| t.2.as_ref())
            .map(|_| -> crate::EncodeResult<(Vec<u16>, Vec<u16>, Vec<u16>)> {
                Ok((
                    svtav1_types::try_vec![0u16; w * h]?,
                    svtav1_types::try_vec![0u16; acw * ach]?,
                    svtav1_types::try_vec![0u16; acw * ach]?,
                ))
            })
            .transpose()?;
        fill_canvas10(
            w,
            h,
            ss_x,
            ss_y,
            acw,
            sb_size,
            tile_grid,
            &tile_recons,
            &mut canvas10,
        );
        for (tile_idx, (tile_recon, tile_trees, _canvas10, tile_rdoq)) in
            tile_recons.into_iter().enumerate()
        {
            let (tile_sb_row_start, tile_sb_row_end) =
                tile_grid.row_span(tile_idx / tile_grid.tile_cols);
            let (tile_sb_col_start, tile_sb_col_end) =
                tile_grid.col_span(tile_idx % tile_grid.tile_cols);
            let mut tile_trees = tile_trees.into_iter();
            let mut tile_rdoq = tile_rdoq.into_iter();
            for sb_row in tile_sb_row_start..tile_sb_row_end {
                // Feature 1: byte-inert cooperative-cancellation check (no-op
                // for the default `Unstoppable` token — `may_stop()` is false).
                if stop.may_stop() {
                    stop.check()
                        .map_err(EncodeError::from)
                        .map_err(whereat::at)?;
                }
                for sb_col in tile_sb_col_start..tile_sb_col_end {
                    crate::stop_check(stop)?;
                    tree_slots[sb_row * sb_cols + sb_col] = tile_trees.next();
                    if let Some(en) = tile_rdoq.next() {
                        sb_enc_rdoq[sb_row * sb_cols + sb_col] = en;
                    }
                }
            }
            let mut offset = 0;
            for sb_row in tile_sb_row_start..tile_sb_row_end {
                // Feature 1: byte-inert cooperative-cancellation check.
                if stop.may_stop() {
                    stop.check()
                        .map_err(EncodeError::from)
                        .map_err(whereat::at)?;
                }
                for sb_col in tile_sb_col_start..tile_sb_col_end {
                    crate::stop_check(stop)?;
                    let x0 = sb_col * sb_size;
                    let y0 = sb_row * sb_size;
                    let cur_w = sb_size.min(w - x0);
                    let cur_h = sb_size.min(h - y0);
                    for r in 0..cur_h {
                        recon[(y0 + r) * w + x0..(y0 + r) * w + x0 + cur_w].copy_from_slice(
                            &tile_recon[offset + r * cur_w..offset + (r + 1) * cur_w],
                        );
                    }
                    offset += cur_w * cur_h;
                }
            }
        }
        let mut all_trees: Vec<crate::partition::PartitionTree> = tree_slots
            .into_iter()
            .map(|t| t.expect("every SB is covered by exactly one tile"))
            .collect();
        let sb_min_sq_sizes: Vec<u8> = all_trees
            .iter()
            .map(|t| u8::try_from(t.min_sq_size(sb_size)).unwrap_or(u8::MAX))
            .collect();
        let sb_max_sq_sizes: Vec<u8> = all_trees
            .iter()
            .map(|t| u8::try_from(t.max_sq_size(sb_size)).unwrap_or(u8::MAX))
            .collect();
        crate::stop_check(stop)?;
        *last_recon10_y = None;
        *last_recon10_uv = None;
        *last_recon10_final = None;
        if let Some((cy, cu, cv)) = canvas10 {
            *last_recon10_y = Some(cy);
            *last_recon10_uv = Some((cu, cv));
        }
        Self::bd10_post_pass(
            bit_depth,
            speed_config.preset,
            hdr.sharpness,
            hdr.tune,
            &mut (*last_recon10_y),
            &mut (*last_recon10_uv),
            chroma,
            hbd_source,
            hbd_used,
            is_key,
            sc_arm,
            md_lambda_base_update_type,
            md_lambda_factor_update_type,
            md_alt_lambda_factors,
            lambda_mod_intra,
            w,
            h,
            fmt,
            acw,
            ach,
            sb_input,
            in_stride,
            sb_chroma_owned,
            hbd_sb_owned,
            base_qindex,
            picture_qp,
            lw_bump,
            coded_lossless,
            primary_ref_cdfs,
            c_quant,
            sb_size,
            sb_cols,
            tile_grid,
            qindex_u,
            qindex_v,
            qm_levels,
            seq_tools,
            inter_md_frame,
            sb_inter_lambda,
            sb_enc_rdoq,
            &mut all_trees,
        )?;
        crate::stop_check(stop)?;
        crate::stop_check(stop)?;
        let cw = acw;
        let ext_cbuf = fmt.chroma_width(w.div_ceil(sb_size) * sb_size)
            * fmt.chroma_height(h.div_ceil(sb_size) * sb_size);
        #[cfg(feature = "std")]
        if crate::dbgenv::dump_tree() {
            for (sb_idx, tree) in all_trees.iter().enumerate() {
                let bx = (sb_idx % sb_cols) * sb_size;
                let by = (sb_idx / sb_cols) * sb_size;
                dump_tree_leaves(tree, bx, by);
            }
        }
        let lr_true_w = true_width as usize;
        let lr_true_h = true_height as usize;
        let frame_coded_area: core::cell::RefCell<Option<CodedAreaAcc>> =
            core::cell::RefCell::new(None);
        let walk_end_cdfs: core::cell::RefCell<Option<crate::port_frame_cdf::FrameCdfs>> =
            core::cell::RefCell::new(None);
        Ok((
            all_trees,
            sb_min_sq_sizes,
            sb_max_sq_sizes,
            cw,
            ext_cbuf,
            lr_true_w,
            lr_true_h,
            frame_coded_area,
            walk_end_cdfs,
        ))
    }
}
