use super::*;

impl EncodePipeline {
    pub(super) fn entropy_walk(
        preset: i8,
        superres_denom: Option<u8>,
        chroma_format: Option<svtav1_types::chroma::ChromaFormat>,
        bit_depth: u8,
        chroma: Option<(&[u8], &[u8])>,
        stop: &almost_enough::StopToken,
        is_key: bool,
        sc_arm: crate::sc_detect::ScArm,
        pic_decision: &Option<crate::port_picstruct::PicParams>,
        w: usize,
        h: usize,
        n: usize,
        sb_chroma_owned: &Option<(Vec<u8>, Vec<u8>)>,
        sc_derivation: crate::sc_detect::ScDerivation,
        frame_tx_mode_select: bool,
        base_qindex: u8,
        delta_q_plan: Option<&crate::sb_qindex::SbQindexPlan>,
        md_sb_qindex: Option<&crate::sb_qindex::SbQindexPlan>,
        primary_ref_cdfs: &Option<alloc::sync::Arc<crate::port_frame_cdf::FrameCdfs>>,
        c_quant: &Option<alloc::sync::Arc<crate::quant::CodingQuantCfg>>,
        sb_size: usize,
        sb_cols: usize,
        sb_rows: usize,
        ref_padded_luma: Option<&crate::picture::PaddedRef>,
        tile_grid: crate::entropy::obu::TileGrid,
        chroma_deltas: crate::chroma_q::ChromaQDeltas,
        qindex_u: u8,
        qindex_v: u8,
        delta_q_res_signal: Option<u8>,
        qm_levels: [u8; 3],
        seq_tools: crate::entropy::obu::SeqTools,
        md_config_signals: Option<crate::port_enc_mode_config::md_config::MdConfigSignals>,
        inter_syntax_state: &Option<InterSyntaxState>,
        inter_ref_frame_side: [i8; 8],
        inter_mvp_env: &Option<crate::partition::InterMdEnv>,
        all_trees: &[crate::partition::PartitionTree],
        cw: usize,
        ext_cbuf: usize,
        lr_true_w: usize,
        lr_true_h: usize,
        frame_coded_area: &core::cell::RefCell<Option<CodedAreaAcc>>,
        walk_end_cdfs: &core::cell::RefCell<Option<crate::port_frame_cdf::FrameCdfs>>,
        lr: Option<&crate::restoration::FrameRestInfo>,
        cdef_walk: Option<&crate::cdef::CdefPick>,
        recon_only: bool,
    ) -> Result<
        (Vec<u8>, crate::deblock::DeblockGeom, Vec<u8>, Vec<u8>, u8),
        whereat::prelude::At<EncodeError>,
    > {
        let (mut u_recon, mut v_recon) = if chroma.is_some() {
            (
                svtav1_types::try_vec![128u8; ext_cbuf]?,
                svtav1_types::try_vec![128u8; ext_cbuf]?,
            )
        } else {
            (Vec::new(), Vec::new())
        };
        // Per-4x4 block/TX/skip geometry for the deblocking edge walk,
        // recorded in coding order (== the decoder's parse order).
        // SHARED across every tile (absolute-position indexed,
        // deblock.rs): deblock/CDEF/LR apply post-tile-merge at frame
        // scope, unaffected by tile-row boundaries, so this — like
        // u_recon/v_recon above — is allocated ONCE and each tile's
        // walk below only ever writes its own rows into it.
        let mut deblock_geom = crate::deblock::DeblockGeom::new(w, h, lr_true_w, lr_true_h);
        // Mode/skip context tracking at 4x4 granularity — frame-wide
        // sizing (not tile-height): block coords (bx, by) passed to
        // encode_partition_tree are ABSOLUTE frame positions, so a
        // fresh EntropyCtx sized to the whole frame keeps those
        // indices valid across every tile while still giving the
        // C-exact "above unavailable at tile top" reset (a fresh
        // EntropyCtx starts every array at its unavailable/default
        // state — exactly entropy_coding_reset_neighbor_arrays,
        // ec_process.c:60-67).
        let w4 = w.div_ceil(4);
        let h4 = h.div_ceil(4);

        debug_assert_eq!(
            all_trees.len(),
            sb_cols * sb_rows,
            "tree count {} != SB count {}x{}={}",
            all_trees.len(),
            sb_cols,
            sb_rows,
            sb_cols * sb_rows,
        );

        // One independent entropy walk PER TILE ROW (task #86): C
        // resets every tile to a fresh FrameContext (`primary_ref_
        // frame == PRIMARY_REF_NONE` always holds for KEY frames) and
        // fresh neighbor-context arrays before its own arithmetic
        // coder starts (`reset_entropy_coding_picture`,
        // ec_process.c:72-117) — mirrored here by constructing fresh
        // writer/frame_ctx/coeff_fc/ectx/lr_refs per tile_idx.
        //
        // Task #96: and per tile COLUMN too. The tile group's tile
        // order is raster over the grid (row-major), which is the
        // order a decoder consumes the size-prefixed payloads in.
        let mut tile_bitstreams: Vec<Vec<u8>> = Vec::with_capacity(tile_grid.num_tiles());
        for tile_idx in 0..tile_grid.num_tiles() {
            // Feature 1: byte-inert cooperative-cancellation check, once per
            // tile of each entropy re-walk (this closure runs up to 3x).
            if stop.may_stop() {
                stop.check()
                    .map_err(EncodeError::from)
                    .map_err(whereat::at)?;
            }
            let (tile_sb_row_start, tile_sb_row_end) =
                tile_grid.row_span(tile_idx / tile_grid.tile_cols);
            let (tile_sb_col_start, tile_sb_col_end) =
                tile_grid.col_span(tile_idx % tile_grid.tile_cols);

            let mut writer = crate::entropy::writer::AomWriter::new(n + 256);
            // CDF updates enabled — matches the frame header's disable_cdf_update=0.
            //
            // C `reset_entropy_coding_picture` (ec_process.c:101-112) does
            // this per TILE, and so does this loop: with
            // `primary_ref_frame != PRIMARY_REF_NONE` every tile starts
            // from the SAME restored reference context (not from the
            // previous tile's end state), which is what makes tiles
            // independently decodable.
            let (mut frame_ctx, mut coeff_fc) = match primary_ref_cdfs.as_ref() {
                Some(prev) => (prev.fc.clone(), prev.coeff.clone()),
                // C-exact coefficient CDFs for the base_q_idx bucket
                // (svt_av1_default_coef_probs semantics) — qindex domain.
                None => (
                    crate::entropy::context::FrameContext::new_default(),
                    crate::entropy::coeff_c::CoeffFc::default_for_qindex(base_qindex),
                ),
            };
            let mut ectx = EntropyCtx::new(
                w4,
                h4,
                seq_tools.enable_filter_intra,
                // The SAME bit the frame header writes — see
                // `EntropyCtx::tx_mode_select`.
                frame_tx_mode_select,
                sc_derivation.allow_screen_content_tools,
                bit_depth,
                chroma_format.unwrap_or(svtav1_types::chroma::ChromaFormat::Yuv420),
            );
            // IBC chunk 1: arm the per-block use_intrabc flag coding
            // (C write_intrabc_info gate) from the same sc derivation
            // that set the FH bit — signaling and coding MUST agree or
            // the stream is undecodable.
            ectx.allow_intrabc = sc_derivation.allow_intrabc;
            // The frame-level inter syntax the pack's inter arm reads
            // (docs/INTER-ENCODE-PLAN.md §1s item 7). `None` on a key
            // frame, where the arm is unreachable.
            ectx.inter_syntax = inter_syntax_state.clone();
            // C `update_b`'s accumulators, armed on the VIDEO arm only —
            // C's own gate is `!pcs->scs->allintra` (coding_loop.c:1603),
            // a SEQUENCE flag, so both frame types of a video encode
            // accumulate and no allintra cell does. That is what keeps
            // the still envelope untouched by construction.
            ectx.coded_area = matches!(sc_arm, crate::sc_detect::ScArm::Video { .. }).then(|| {
                CodedAreaAcc::new(
                    // C `frm_hdr->allow_high_precision_mv`, from the
                    // same derivation the header signals. FALSE on a
                    // key frame, where `inter_syntax_state` is `None`
                    // and no block carries an MV anyway.
                    inter_syntax_state
                        .as_ref()
                        .is_some_and(|st| st.allow_high_precision_mv),
                    sb_size,
                    w.div_ceil(sb_size),
                    h.div_ceil(sb_size),
                    h.div_ceil(4) as i32,
                    w.div_ceil(4) as i32,
                    // C `coding_loop.c:1748`:
                    // `scs->mfmv_enabled && slice_type != I_SLICE &&
                    //  ppcs->is_ref`. `mfmv_enabled` is
                    // `svt_aom_set_mfmv_config`'s sequence flag, which
                    // `md_config_inputs` already derives as
                    // `enc_mode <= ENC_M10`; `is_ref` is the picture
                    // decision's. On a key frame `md_config_signals`
                    // is `None`, which is C's `I_SLICE` arm.
                    md_config_signals.is_some()
                        && preset <= 10
                        && pic_decision.as_ref().is_some_and(|p| p.is_ref),
                    inter_ref_frame_side,
                )
            });
            if let Some(env) = inter_mvp_env.clone() {
                ectx.arm_inter_mvp(env);
            }
            // Task #86: this tile's own top row — gates "above"
            // availability in tx_size_ctx and (via chroma_pass's
            // encode_chroma_block_dc calls below) chroma prediction.
            ectx.tile_top_px = tile_sb_row_start * sb_size;
            // Task #96: ditto for this tile's own left column.
            ectx.tile_left_px = tile_sb_col_start * sb_size;
            // Same rect in LUMA mi, ends included, for the MD
            // prediction path. Ends are clamped to the frame exactly
            // like C's av1_tile_set_{col,row}
            // (`AOMMIN(mi_col_end, cm->mi_params.mi_cols)`).
            ectx.tile_mi = crate::intra_edge::TileMi {
                mi_row_start: tile_sb_row_start * sb_size / 4,
                mi_row_end: (tile_sb_row_end * sb_size / 4).min(h4),
                mi_col_start: tile_sb_col_start * sb_size / 4,
                mi_col_end: (tile_sb_col_end * sb_size / 4).min(w4),
            };
            // [SVT_HDR_MODE] arm per-SB delta-q: prev starts at the FH base
            // (C prev_qindex tile-init); uniform plan = every SB at base.
            if let Some(res) = delta_q_res_signal {
                ectx.delta_q_state = Some((res, i32::from(base_qindex), sb_size));
                ectx.delta_q_sb_qindex = i32::from(base_qindex);
            }
            let mut chroma_pass = sb_chroma_owned.as_ref().map(|(u_src, v_src)| ChromaPass {
                u_src: u_src.as_slice(),
                v_src: v_src.as_slice(),
                u_recon: &mut u_recon,
                v_recon: &mut v_recon,
                stride: cw,
                qindex_u,
                qindex_v,
                qm_u: qm_levels[1],
                qm_v: qm_levels[2],
                c_quant: c_quant.as_deref(),
                ref_uv: ref_padded_luma
                    .and_then(|p| p.uv.as_ref())
                    .map(|(u, v)| (u, v)),
                sb_size,
                frame_w: w,
                frame_h: h,
            });
            // LR tap references reset at the tile start (C
            // svt_av1_reset_loop_restoration, ec_process.c:199).
            let mut lr_refs = crate::restoration::LrWalkRefs::default();
            let mut prev_sb_row = usize::MAX;

            for sb_row in tile_sb_row_start..tile_sb_row_end {
                // Feature 1: byte-inert cooperative-cancellation check, once
                // per SB row of the entropy walk.
                if stop.may_stop() {
                    stop.check()
                        .map_err(EncodeError::from)
                        .map_err(whereat::at)?;
                }
                for sb_col in tile_sb_col_start..tile_sb_col_end {
                    crate::stop_check(stop)?;
                    let sb_idx = sb_row * sb_cols + sb_col;
                    let tree = &all_trees[sb_idx];
                    // Per-SB delta-q / TPL: the SB's qindex drives the
                    // delta symbol (only when `delta_q_present` armed the
                    // state), the chroma dequant, and (via the search,
                    // which used the same map) the coded coefficients.
                    // `md_sb_qindex` is the QUANT map — live under
                    // `r0_delta_qp_md` even when nothing is signalled.
                    if let Some(plan) = md_sb_qindex {
                        let sbq = i32::from(plan.sb_qindex[sb_idx]);
                        if delta_q_plan.is_some() {
                            ectx.delta_q_sb_qindex = sbq;
                        }
                        if let Some(cp) = chroma_pass.as_mut() {
                            cp.qindex_u = (sbq + i32::from(chroma_deltas.u_ac)).clamp(0, 255) as u8;
                            cp.qindex_v = (sbq + i32::from(chroma_deltas.v_ac)).clamp(0, 255) as u8;
                        }
                    }
                    let bx = sb_col * sb_size;
                    let by = sb_row * sb_size;

                    // Reset left partition context at the start of each SB row,
                    // matching rav1d's per-tile-row left context reset.
                    if sb_row != prev_sb_row {
                        ectx.reset_left_for_sb_row();
                        prev_sb_row = sb_row;
                    }

                    // Arm the per-SB cdef_idx emission (C write_cdef resets
                    // cdef_transmitted at the SB's top-left, then the first
                    // non-skip block emits `cdef_bits` literal bits). 64x64
                    // SBs: one filter block per SB.
                    // C write_cdef resets `cdef_transmitted[4]` at the
                    // SB top-left, then each 64x64 quadrant's first
                    // non-skip block emits its own literal. The strength
                    // is read off the B64 grid (C's mbmi at
                    // `(mi & ~15)`), which is what `fb_idx` is indexed
                    // by — NOT by the SB grid. At SB64 the two grids
                    // coincide and only quadrant 0 is ever used, so this
                    // reduces exactly to the previous
                    // `fb_idx[sb_row * nhfb + sb_col]`.
                    ectx.cdef_sb = cdef_walk.and_then(|p| {
                        (p.bits > 0).then(|| {
                            let fb_per_sb = sb_size / 64;
                            let mut strengths = [0u8; 4];
                            for (q, st) in strengths.iter_mut().enumerate() {
                                let fbc = sb_col * fb_per_sb + (q & 1);
                                let fbr = sb_row * fb_per_sb + (q >> 1);
                                // Off-frame quadrants of a partial SB
                                // code nothing, so their slot is never
                                // read; 0 keeps the lookup total.
                                *st = p
                                    .fb_idx
                                    .get(fbr * p.nhfb + fbc)
                                    .copied()
                                    .filter(|_| fbc < p.nhfb)
                                    .unwrap_or(0);
                            }
                            CdefSbState {
                                bits: p.bits,
                                strengths,
                                transmitted: [false; 4],
                                sb128: sb_size == 128,
                            }
                        })
                    });

                    // Loop-restoration coefficients for every RU cornered in
                    // this SB — BEFORE the SB's partition tree, matching the
                    // decoder's read order.
                    if let Some(info) = lr {
                        crate::restoration::write_lr_for_sb(
                            &mut writer,
                            &mut frame_ctx,
                            info,
                            &mut lr_refs,
                            (by / 4) as i32,
                            (bx / 4) as i32,
                            (sb_size / 4) as i32,
                            // TRUE dims: the RU grid / corner computation is
                            // coded off the coded frame size, not the aligned
                            // grid (byte-neutral when 8-aligned).
                            lr_true_w,
                            lr_true_h,
                            chroma.is_none(),
                            superres_denom,
                        );
                    }

                    encode_partition_tree(
                        tree,
                        &mut writer,
                        &mut frame_ctx,
                        &mut coeff_fc,
                        base_qindex,
                        &mut ectx,
                        is_key,
                        bx,
                        by,
                        &mut chroma_pass,
                        &mut deblock_geom,
                        recon_only,
                    );
                }
            }

            tile_bitstreams.push(writer.done().to_vec());
            // C `enc_dec_process.c:3166-3170`: each EncDec context's
            // coded-area totals are summed into the picture under
            // `pcs->intra_mutex`. One tile per context here.
            // The recon-only walk keeps NO coded-area / CDF state: its
            // sums would be re-added by the bit-producing walk that
            // follows, inflating `intra_area`/`skip_area`/`hp_area` past
            // C's single `update_b` pass. (This is also the latent fix
            // for the pre-split walks double-merging on re-walk frames —
            // only ONE bit-producing walk now runs per frame.)
            if !recon_only && let Some(acc) = ectx.coded_area.as_ref() {
                let mut slot = frame_coded_area.borrow_mut();
                match slot.as_mut() {
                    Some(f) => f.merge(acc),
                    None => *slot = Some(acc.clone()),
                }
            }
            // See `walk_end_cdfs`: overwritten per tile AND per walk, so
            // it ends holding the last tile of the last walk — C's own
            // "last tile wins" save order.
            if !recon_only {
                *walk_end_cdfs.borrow_mut() = Some(crate::port_frame_cdf::FrameCdfs {
                    fc: frame_ctx,
                    coeff: coeff_fc,
                });
            }
        }

        // Shared derivation for the frame header's tile_info() trailer
        // AND the tile group's size prefixes — computed once from the
        // real per-tile byte lengths so the two can never disagree
        // (see tile_size_bytes_minus_1_for's doc comment).
        let non_last_lens: Vec<usize> = tile_bitstreams[..tile_bitstreams.len().saturating_sub(1)]
            .iter()
            .map(|t| t.len())
            .collect();
        let tile_size_bytes_minus_1 =
            crate::entropy::obu::tile_size_bytes_minus_1_for(&non_last_lens);

        Ok((
            crate::entropy::obu::build_tile_group_multi(&tile_bitstreams, tile_size_bytes_minus_1),
            deblock_geom,
            u_recon,
            v_recon,
            tile_size_bytes_minus_1,
        ))
    }
}
