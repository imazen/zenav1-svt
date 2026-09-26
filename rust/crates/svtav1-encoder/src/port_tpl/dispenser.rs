use super::*;

/// Per-SB inputs to [`tpl_mc_flow_dispenser_sb_generic`], bundling the C
/// `pcs` / `enc_ctx` / `scs` fields the function reads.
pub struct TplSbCtx<'a> {
    /// C `pcs->enhanced_pic` — the input source plane.
    pub input_pic: TplPic<'a>,
    /// C `pcs->tpl_ctrls`.
    pub tpl_ctrls: crate::port_picstruct::TplControls,
    /// C `pcs->tpl_data`.
    pub tpl_data: &'a TplData,
    /// C `pcs->temporal_layer_index`.
    pub temporal_layer_index: u8,
    /// C `pcs->hierarchical_levels`.
    pub hierarchical_levels: u8,
    /// C `pcs->slice_type`.
    pub slice_type: crate::port_picstruct::SliceType,
    /// C `pcs->update_type` — for the subpel rdmult.
    pub update_type: crate::port_picstruct::FrameUpdateType,
    /// C `scs->static_config.qp` already mapped through
    /// `quantizer_to_qindex` — the qindex the subpel rdmult uses.
    pub qp_qindex: u8,
    /// C `scs->static_config.extended_crf_qindex_offset`.
    pub extended_crf_qindex_offset: i32,
    /// C `pcs->tpl_src_data_ready` — reuse cached src stats (tpl_lad_mg>0
    /// re-runs a frame inside a later window).
    pub tpl_src_data_ready: bool,
    /// C `pcs->enable_me_16x16` — scales `me_mb_offset`.
    pub enable_me_16x16: bool,
    /// C `scs->tpl_lad_mg` — whether `tpl_src_stats_buffer` is live.
    pub tpl_lad_mg: u8,
    /// C `scs->b64_geom[sb_index]` reduced to the SB origin.
    pub sb_origin: (u32, u32),
    /// C `pcs->aligned_width`.
    pub aligned_width: u32,
    /// C `(pcs->aligned_width + 15) >> 4` — the `tpl_src_stats_buffer` stride.
    pub aligned16_width: usize,
    /// C `cm->mi_rows` / `cm->mi_cols`.
    pub mi_rows: i32,
    /// See [`TplSbCtx::mi_rows`].
    pub mi_cols: i32,
    /// C `scs->max_input_luma_width` / `max_input_luma_height` — the
    /// `svt_aom_filter_intra_edge` bounds args.
    pub max_input_luma_width: u32,
    /// See [`TplSbCtx::max_input_luma_width`].
    pub max_input_luma_height: u32,
    /// C `scs->super_block_size` — for [`svtav1_dsp::port_subpel_params::RefGeometry`].
    pub super_block_size: i32,
    /// C `scs->sf_identity`.
    pub sf_identity: svtav1_dsp::port_scale_factors::ScaleFactors,
    /// The `quants_8bit` row for this picture's TPL qindex — C indexes
    /// `scs->enc_ctx->quants_8bit.y_*[qIndex]` per SB call.
    pub quant: crate::quant::QuantTable,
    /// C `enc_ctx->poc_map_idx` — TPL window index -> POC.
    pub poc_map_idx: &'a [u64],
    /// C `pcs->tpl_data.base_pcs->tpl_valid_pic` — which earlier window
    /// frames have a usable TPL recon.
    pub base_tpl_valid_pic: &'a [u8],
}

/// The ME results for one superblock — the `pa_me_data->me_results[sb_index]`
/// subset the dispenser iterates.
pub struct TplMeResults<'a> {
    /// C `me_results->total_me_candidate_index` (indexed by `me_mb_offset`).
    pub total_me_candidate_index: &'a [u8],
    /// C `me_results->me_candidate_array`
    /// (`me_mb_offset * max_cand + cand`).
    pub me_candidate_array: &'a [crate::inter_me::context::MeCandidate],
    /// C `me_results->me_mv_array`
    /// (`me_mb_offset * max_refs + (list ? max_l0 : 0) + ref_idx`), full-pel.
    pub me_mv_array: &'a [Mv],
    /// C `pa_me_data->max_cand`.
    pub max_cand: usize,
    /// C `pa_me_data->max_refs`.
    pub max_refs: usize,
    /// C `pa_me_data->max_l0`.
    pub max_l0: usize,
}

/// C `svt_aom_filter_intra_edge` (intra_prediction.c:2597) restricted to the
/// directional arm — the only mode class TPL ever calls it with, so the
/// `extend_modes` flags are always overridden by the `p_angle` chain. Fixed
/// TX_16X16 geometry. `above`/`left` are the C `above_row`/`left_col`
/// pointers: index `origin` is row sample 0, `origin - 1` the corner.
#[allow(clippy::too_many_arguments)]
pub(super) fn tpl_filter_intra_edge(
    p_angle: i32,
    max_frame_width: u32,
    max_frame_height: u32,
    cu_origin_x: u32,
    cu_origin_y: u32,
    above: &mut [u8],
    left: &mut [u8],
    origin: usize,
) {
    use svtav1_dsp::intra_pred as ip;
    let mb_stride = max_frame_width.div_ceil(16);
    let mb_height = max_frame_height.div_ceil(16);
    let txwpx = 16i32; // TX_16X16
    let txhpx = 16i32;
    let need_right = p_angle < 90;
    let need_bottom = p_angle > 180;
    // C reads extend_modes[mode] first, then the directional arm overrides
    // all three flags — the initial values are dead here, so we keep only
    // the override (the caller only reaches us for directional modes).
    let (need_above, need_left) = if p_angle <= 90 {
        (true, false)
    } else if p_angle < 180 {
        (true, true)
    } else {
        (false, true)
    };
    let need_above_left = true;
    let n_top_px = if cu_origin_y > 0 {
        txwpx.min((mb_stride * 16) as i32 - cu_origin_x as i32 + txwpx)
    } else {
        0
    };
    let n_left_px = if cu_origin_x > 0 {
        txhpx.min((mb_height * 16) as i32 - cu_origin_y as i32 + txhpx)
    } else {
        0
    };

    if p_angle != 90 && p_angle != 180 {
        let ab_le = if need_above_left { 1usize } else { 0 };
        if need_above && need_left && (txwpx + txhpx >= 24) {
            ip::filter_intra_edge_corner(above, left, origin);
        }
        if need_above && n_top_px > 0 {
            let strength = ip::intra_edge_filter_strength(txwpx, txhpx, p_angle - 90, 0);
            let n_px = n_top_px as usize + ab_le + if need_right { txhpx as usize } else { 0 };
            ip::filter_intra_edge(above, origin - ab_le, n_px, strength);
        }
        if need_left && n_left_px > 0 {
            let strength = ip::intra_edge_filter_strength(txhpx, txwpx, p_angle - 180, 0);
            let n_px = n_left_px as usize + ab_le + if need_bottom { txwpx as usize } else { 0 };
            ip::filter_intra_edge(left, origin - ab_le, n_px, strength);
        }
    }
    if need_above && ip::use_intra_edge_upsample(txwpx, txhpx, p_angle - 90, 0) {
        let n_px = txwpx as usize + if need_right { txhpx as usize } else { 0 };
        ip::upsample_intra_edge(above, origin, n_px);
    }
    if need_left && ip::use_intra_edge_upsample(txhpx, txwpx, p_angle - 180, 0) {
        let n_px = txhpx as usize + if need_bottom { txwpx as usize } else { 0 };
        ip::upsample_intra_edge(left, origin, n_px);
    }
}

/// The intra-prediction neighbor layout TPL uses: the C arrays have the
/// corner at `data[MAX_TPL_SIZE - 1]` and `row = data + MAX_TPL_SIZE`.
pub(super) const TPL_NEIGH_SZ: usize = MAX_TPL_SIZE * 4 + 1;

/// C `pcs->tpl_ctrls.pf_shape` (`u8`) -> [`TxCoeffShape`]; C's field can
/// only ever hold DEFAULT/N2/N4 — `svt_aom_set_tpl_params` assigns literals.
pub(super) fn tpl_pf_shape(
    t: &crate::port_picstruct::TplControls,
) -> svtav1_dsp::fwd_txfm_pf::TxCoeffShape {
    use svtav1_dsp::fwd_txfm_pf::TxCoeffShape as S;
    match t.pf_shape {
        crate::port_picstruct::N2_SHAPE => S::N2,
        crate::port_picstruct::N4_SHAPE => S::N4,
        _ => S::Default,
    }
}

/// `(tx_size_wide, tx_size_high)` for the square-subsample TX family TPL
/// uses — the only shapes `TX_SIZE_ARRAY`/`SUB2`/`SUB4` can produce.
pub(super) fn tx_dims(tx: TxSize) -> (usize, usize) {
    match tx {
        TxSize::Tx16x16 => (16, 16),
        TxSize::Tx32x32 => (32, 32),
        TxSize::Tx64x64 => (64, 64),
        TxSize::Tx16x8 => (16, 8),
        TxSize::Tx32x16 => (32, 16),
        TxSize::Tx64x32 => (64, 32),
        TxSize::Tx16x4 => (16, 4),
        TxSize::Tx32x8 => (32, 8),
        TxSize::Tx64x16 => (64, 16),
        other => unreachable!("tpl tx shape {other:?}"),
    }
}

/// u8 -> [`svtav1_types::prediction::PredictionMode`] for the intra modes
/// TPL loops over (`DC_PRED..=PAETH_PRED`). `None` for out-of-range — the
/// callers keep C's behaviour of never asking for one.
pub(super) fn tpl_mode(v: u8) -> Option<svtav1_types::prediction::PredictionMode> {
    use svtav1_types::prediction::PredictionMode as M;
    Some(match v {
        0 => M::DcPred,
        1 => M::VPred,
        2 => M::HPred,
        3 => M::D45Pred,
        4 => M::D135Pred,
        5 => M::D113Pred,
        6 => M::D157Pred,
        7 => M::D203Pred,
        8 => M::D67Pred,
        9 => M::SmoothPred,
        10 => M::SmoothVPred,
        11 => M::SmoothHPred,
        12 => M::PaethPred,
        _ => return None,
    })
}

/// Fill one mode's neighbors + edge filter, predict into `pred`, and score —
/// the shared body of the src-path and recon-path intra searches.
/// `above0`/`left0` carry the [0]=corner convention from
/// [`update_neighbor_samples_open_loop`].
#[allow(clippy::too_many_arguments)]
pub(super) fn tpl_predict_intra(
    mode_u8: u8,
    p_angle: i32,
    max_w: u32,
    max_h: u32,
    mb_origin_x: u32,
    mb_origin_y: u32,
    above0: &[u8],
    left0: &[u8],
    scratch_above: &mut [u8],
    scratch_left: &mut [u8],
    size: usize,
    pred: &mut [u8],
    pred_stride: usize,
) {
    let mode = tpl_mode(mode_u8).expect("TPL iterates DC..=PAETH");
    let (above_row, left_row, corner): (&[u8], &[u8], u8) =
        if crate::intra_open_loop::is_directional_mode(mode) {
            scratch_above.copy_from_slice(above0);
            scratch_left.copy_from_slice(left0);
            // above0 is a `TPL_NEIGH_SZ` slice with the corner at index 0 and
            // the row at 1.. — but C's arrays put the corner at
            // `data[MAX_TPL_SIZE-1]`, row at `data+MAX_TPL_SIZE`. The scratch
            // copies keep the same layout, and `tpl_filter_intra_edge` takes
            // `origin` = the row start index inside the slice.
            tpl_filter_intra_edge(
                p_angle,
                max_w,
                max_h,
                mb_origin_x,
                mb_origin_y,
                scratch_above,
                scratch_left,
                1, // corner at [0], row at [1..]
            );
            (&scratch_above[1..], &scratch_left[1..], scratch_above[0])
        } else {
            (&above0[1..], &left0[1..], above0[0])
        };
    // `2 * size`, not `size`: a directional mode reads the edge EXTENSION
    // (zone 1 up to `w + h` above samples, zone 3 as many left), which C's
    // pointer into the full neighbour buffer provides. Slicing to `size`
    // panicked on the first zone-1/3 angle (random access + aq_mode 2 at
    // 128x128, found 2026-09-26 by svtav1/tests/video_paths.rs).
    let n = crate::intra_open_loop::Neighbours {
        above: &above_row[..2 * size],
        left: &left_row[..2 * size],
        top_left: corner,
        has_left: mb_origin_x > 0,
        has_above: mb_origin_y > 0,
    };
    // C returns EB_ErrorNone even when the dr leaf falls through on a bad
    // angle — unreachable here because `p_angle` comes from
    // `mode_to_angle_map` exactly as in C.
    let _ = crate::intra_open_loop::intra_prediction_open_loop_mb(
        mode,
        p_angle,
        n,
        size,
        size,
        pred,
        pred_stride,
    );
}

/// C `tpl_mc_flow_dispenser_sb_generic` — one SB of the TPL fake-encode.
///
/// `recon` is `enc_ctx->mc_flow_rec_picture_buffer[frame_idx]`; `rec_refs`
/// is the window's recon list. `ref_pics` is the two-list view of
/// `pcs->tpl_data.tpl_ref_ds_ptr_array` (downscaled PA sources) and
/// `me_results` the SB's PA ME output.
#[allow(clippy::too_many_arguments)]
pub fn tpl_mc_flow_dispenser_sb_generic(
    ctx: &TplSbCtx<'_>,
    dispenser_search_level: usize,
    recon: &mut TplPicMut<'_>,
    rec_refs: &[TplPic<'_>],
    ref_pics: &[TplPic<'_>],
    me_results: &TplMeResults<'_>,
    tpl_src_stats_buffer: &mut [TplSrcStats],
    tpl_stats_grid: &mut [TplStats],
) {
    use svtav1_dsp::port_masked_compound::subtract_block;

    let size = SIZE_ARRAY[dispenser_search_level] as usize;
    let blk_start = BLK_START_ARRAY[dispenser_search_level];
    let blk_end = BLK_END_ARRAY[dispenser_search_level];

    let tpl_ctrls = &ctx.tpl_ctrls;
    let tx_size = if tpl_ctrls.subsample_tx == 2 {
        SUB4_TX_SIZE_ARRAY[dispenser_search_level]
    } else if tpl_ctrls.subsample_tx == 1 {
        SUB2_TX_SIZE_ARRAY[dispenser_search_level]
    } else {
        TX_SIZE_ARRAY[dispenser_search_level]
    };
    let subsample_tx = tpl_ctrls.subsample_tx as usize;

    let input_pic = ctx.input_pic;
    let src_stride = input_pic.y_stride;
    let dst_buffer_stride = recon.y_stride;
    let (sb_org_x, sb_org_y) = ctx.sb_origin;

    let disable_intra_pred = tpl_ctrls.disable_intra_pred_nref != 0
        && ctx.temporal_layer_index == ctx.hierarchical_levels;
    let intra_dc_sad_path = tpl_ctrls.use_sad_in_src_search != 0
        && tpl_ctrls.intra_mode_end == crate::port_picstruct::DC_PRED;

    let geom = svtav1_dsp::port_subpel_params::RefGeometry {
        super_block_size: ctx.super_block_size,
        frame_width: input_pic.width as i32,
        frame_height: input_pic.height as i32,
    };

    for blk_index in blk_start..=blk_end {
        let z_blk_index = TPL_BLK_IDX_TAB[0][blk_index as usize] as usize;
        let blk_stats = &CODED_UNIT_STATS[z_blk_index];
        let bsize = blk_stats.size as usize;
        let block_size = match bsize {
            8 => BlockSize::Block8x8,
            16 => BlockSize::Block16x16,
            32 => BlockSize::Block32x32,
            _ => BlockSize::Block64x64,
        };
        let mb_origin_x = sb_org_x + u32::from(blk_stats.org_x);
        let mb_origin_y = sb_org_y + u32::from(blk_stats.org_y);

        // at least half of the block inside
        if mb_origin_x as usize + (size >> 1) > input_pic.width as usize
            || mb_origin_y as usize + (size >> 1) > input_pic.height as usize
        {
            continue;
        }

        let xd = TplXd::new(
            ctx.mi_rows,
            ctx.mi_cols,
            block_size,
            mb_origin_x,
            mb_origin_y,
        );
        let edges = xd.edges();

        let dst_mb_offset =
            recon.origin + mb_origin_y as usize * dst_buffer_stride + mb_origin_x as usize;
        let src_mb = input_pic.origin + mb_origin_x as usize + mb_origin_y as usize * src_stride;

        let mut recon_error: i64 = 1;
        let mut sse: i64 = 1;
        let mut best_ref_poc: u64 = 0;
        let mut best_rf_idx: i32 = -1;
        let mut final_best_mv = Mv::ZERO;
        let mut best_mode: u8 = DC_PRED;
        let mut tpl_stats = TplStats::default();
        let mut best_intra_mode: u8 = DC_PRED;

        let src_stats_idx =
            ((mb_origin_y as usize) >> 4) * ctx.aligned16_width + ((mb_origin_x as usize) >> 4);

        let mut predictor = [0u8; MAX_TPL_SAMPLES_PER_BLOCK * 2];
        let mut src_diff = [0i16; MAX_TPL_SAMPLES_PER_BLOCK];
        let mut coeff = [0i32; MAX_TPL_SAMPLES_PER_BLOCK];
        let mut qcoeff = [0i32; MAX_TPL_SAMPLES_PER_BLOCK];
        let mut dqcoeff = [0i32; MAX_TPL_SAMPLES_PER_BLOCK];
        let mut best_coeff = [0i32; MAX_TPL_SAMPLES_PER_BLOCK];
        let mut compensated_blk = [0u8; MAX_TPL_SAMPLES_PER_BLOCK];

        if !ctx.tpl_src_data_ready {
            let mut best_inter_cost: i64 = i64::MAX;
            let mut best_intra_cost: i64 = i64::MAX;
            if !disable_intra_pred {
                if intra_dc_sad_path {
                    // Fast DC-only + SAD path. C uses the real source
                    // neighbours when the whole block is interior, else the
                    // open-loop edge builder. [0]=corner layout in
                    // `above0`/`left0`.
                    let mut above0 = [0u8; TPL_NEIGH_SZ];
                    let mut left0 = [0u8; TPL_NEIGH_SZ];
                    let mb_inside = mb_origin_x as usize + size <= input_pic.width as usize
                        && mb_origin_y as usize + size <= input_pic.height as usize;
                    if mb_origin_x > 0 && mb_origin_y > 0 && mb_inside {
                        get_neighbor_samples_dc(
                            input_pic.y,
                            src_stride,
                            src_mb,
                            &mut above0,
                            &mut left0,
                            bsize,
                        );
                    } else {
                        // C's `input_pic->y_buffer` is the frame-origin
                        // pointer — `TplPic.y` is the whole padded
                        // allocation, so hand over the origin-shifted slice.
                        update_neighbor_samples_open_loop(
                            true,
                            true,
                            &input_pic.y[input_pic.origin..],
                            src_stride,
                            input_pic.width,
                            input_pic.height,
                            mb_origin_x,
                            mb_origin_y,
                            bsize,
                            bsize,
                            &mut above0,
                            &mut left0,
                        );
                    }
                    let n = crate::intra_open_loop::Neighbours {
                        above: &above0[1..1 + size],
                        left: &left0[1..1 + size],
                        top_left: above0[0],
                        has_left: mb_origin_x > 0,
                        has_above: mb_origin_y > 0,
                    };
                    // tx_size_array[search_level] — the FULL (non-subsampled)
                    // block for prediction, C passes `size` as dst_stride.
                    let _ = crate::intra_open_loop::intra_prediction_open_loop_mb(
                        svtav1_types::prediction::PredictionMode::DcPred,
                        0,
                        n,
                        size,
                        size,
                        &mut predictor,
                        size,
                    );
                    best_intra_cost = i64::from(crate::inter_me::sad::nxm_sad_kernel(
                        &input_pic.y[src_mb..],
                        src_stride,
                        &predictor,
                        size,
                        size,
                        size,
                    ));
                } else {
                    let mut above0 = [0u8; TPL_NEIGH_SZ];
                    let mut left0 = [0u8; TPL_NEIGH_SZ];
                    let mut above = [0u8; TPL_NEIGH_SZ];
                    let mut left = [0u8; TPL_NEIGH_SZ];

                    update_neighbor_samples_open_loop(
                        true,
                        true,
                        &input_pic.y[input_pic.origin..],
                        src_stride,
                        input_pic.width,
                        input_pic.height,
                        mb_origin_x,
                        mb_origin_y,
                        bsize,
                        bsize,
                        &mut above0,
                        &mut left0,
                    );

                    let intra_mode_end = tpl_ctrls.intra_mode_end;
                    for ois_intra_mode in DC_PRED..=intra_mode_end {
                        let mode = tpl_mode(ois_intra_mode).expect("intra_mode_end <= PAETH");
                        let p_angle = if crate::intra_open_loop::is_directional_mode(mode) {
                            crate::intra_edge::MODE_TO_ANGLE_MAP[ois_intra_mode as usize]
                        } else {
                            0
                        };
                        tpl_predict_intra(
                            ois_intra_mode,
                            p_angle,
                            ctx.max_input_luma_width,
                            ctx.max_input_luma_height,
                            mb_origin_x,
                            mb_origin_y,
                            &above0,
                            &left0,
                            &mut above,
                            &mut left,
                            size,
                            &mut predictor,
                            size,
                        );
                        let intra_cost: i64 = if tpl_ctrls.use_sad_in_src_search != 0 {
                            i64::from(crate::inter_me::sad::nxm_sad_kernel(
                                &input_pic.y[src_mb..],
                                src_stride,
                                &predictor,
                                size,
                                size,
                                size,
                            ))
                        } else {
                            subtract_block(
                                size >> subsample_tx,
                                size,
                                &mut src_diff,
                                size << subsample_tx,
                                &input_pic.y[src_mb..],
                                src_stride << subsample_tx,
                                &predictor,
                                size << subsample_tx,
                            );
                            svtav1_dsp::fwd_txfm_pf::wht_fwd_txfm(
                                &src_diff,
                                size << subsample_tx,
                                &mut coeff,
                                tx_size,
                                tpl_pf_shape(tpl_ctrls),
                                8,
                                false,
                            );
                            i64::from(aom_satd(&coeff[..(size * size) >> subsample_tx]))
                                << subsample_tx
                        };
                        if intra_cost < best_intra_cost {
                            best_mode = ois_intra_mode;
                            best_intra_cost = intra_cost;
                            best_intra_mode = ois_intra_mode;
                        }
                    }
                }
            }

            // Inter Src path — score each ME candidate against the
            // reference SOURCE pictures.
            let mut me_mb_offset = TPL_BLK_IDX_TAB[1][blk_index as usize] as usize;
            if !ctx.enable_me_16x16 {
                me_mb_offset = (me_mb_offset - 1) / 4;
            }
            let total_me_cnt = if ctx.slice_type == crate::port_picstruct::SliceType::I
                || me_results.total_me_candidate_index.is_empty()
            {
                // I slices never run ME; an empty result set is the port's
                // "no ME ran" (a ref pyramid no source resolved to), which
                // the emit path also treats as absent rather than searched.
                0
            } else {
                me_results.total_me_candidate_index[me_mb_offset] as usize
            };
            let me_block_results = me_results
                .me_candidate_array
                .get(me_mb_offset * me_results.max_cand..)
                .unwrap_or(&[]);

            for me_cand in me_block_results.iter().take(total_me_cnt) {
                // consider only single refs
                if me_cand.direction() > 1 {
                    continue;
                }
                let list_index = me_cand.direction() as usize;
                let ref_pic_index = if list_index == 0 {
                    me_cand.ref_idx_l0() as usize
                } else {
                    me_cand.ref_idx_l1() as usize
                };
                // exclude this cand if the reference is within the sliding
                // window and does not have valid TPL recon data. C's field
                // is an int8_t where -1 means "out of window"; the index is
                // only read once `> 0` proves it in-window.
                let ref_grp_idx = ctx.tpl_data.ref_tpl_group_idx[list_index][ref_pic_index];
                if ref_grp_idx > 0 && ctx.base_tpl_valid_pic[ref_grp_idx as usize] == 0 {
                    continue;
                }
                let rf_idx = crate::port_picstruct::get_ref_frame_type(
                    list_index as u8,
                    ref_pic_index as u8,
                ) as i32
                    - 1;
                let me_offset = me_mb_offset * me_results.max_refs
                    + if list_index != 0 {
                        me_results.max_l0
                    } else {
                        0
                    }
                    + ref_pic_index;
                let ref_pic =
                    ref_pics[ctx.tpl_data.tpl_ref_ds[list_index][ref_pic_index].pic_index];

                let mut x_curr_mv = i32::from(me_results.me_mv_array[me_offset].x) * 8;
                let mut y_curr_mv = i32::from(me_results.me_mv_array[me_offset].y) * 8;

                if mb_origin_x as i32 + (x_curr_mv >> 3) < -TPL_PAD {
                    x_curr_mv = (-TPL_PAD - mb_origin_x as i32) * 8;
                }
                if mb_origin_x as i32 + bsize as i32 + (x_curr_mv >> 3)
                    > TPL_PAD + ref_pic.max_width as i32 - 1
                {
                    x_curr_mv = (TPL_PAD + ref_pic.max_width as i32
                        - 1
                        - (mb_origin_x as i32 + bsize as i32))
                        * 8;
                }
                if mb_origin_y as i32 + (y_curr_mv >> 3) < -TPL_PAD {
                    y_curr_mv = (-TPL_PAD - mb_origin_y as i32) * 8;
                }
                if mb_origin_y as i32 + bsize as i32 + (y_curr_mv >> 3)
                    > TPL_PAD + ref_pic.max_height as i32 - 1
                {
                    y_curr_mv = (TPL_PAD + ref_pic.max_height as i32
                        - 1
                        - (mb_origin_y as i32 + bsize as i32))
                        * 8;
                }

                let mut best_mv = Mv {
                    x: x_curr_mv as i16,
                    y: y_curr_mv as i16,
                };
                if tpl_ctrls.subpel_depth != crate::port_picstruct::FULL_PEL {
                    tpl_subpel_search(
                        tpl_ctrls,
                        ctx.update_type,
                        ctx.qp_qindex,
                        ctx.extended_crf_qindex_offset,
                        ctx.mi_rows,
                        ctx.mi_cols,
                        &ref_pic,
                        &input_pic,
                        mb_origin_x,
                        mb_origin_y,
                        bsize as u8,
                        &mut best_mv,
                    );
                }
                let ref_origin_index = (ref_pic.origin as i32
                    + mb_origin_x as i32
                    + (best_mv.x as i32 / 8)
                    + (mb_origin_y as i32 + (best_mv.y as i32 / 8)) * ref_pic.y_stride as i32)
                    as usize;

                let subpel_mv = (best_mv.x & 0x7) != 0 || (best_mv.y & 0x7) != 0;
                if subpel_mv {
                    let mut conv_buf = [0u16; MAX_TPL_SAMPLES_PER_BLOCK];
                    let cp = svtav1_dsp::port_convolve::ConvolveParams::no_round(
                        false,
                        MAX_TPL_SIZE,
                        false,
                        8,
                    );
                    let _ = svtav1_dsp::port_enc_make_pred::enc_make_inter_predictor(
                        svtav1_dsp::port_enc_make_pred::SrcPlanes::Lbd(ref_pic.y),
                        ref_pic.origin,
                        ref_pic.y_stride,
                        svtav1_dsp::port_enc_make_pred::DstPlane::Lbd(&mut compensated_blk),
                        size,
                        &mut conv_buf,
                        mb_origin_y as i32,
                        mb_origin_x as i32,
                        svtav1_dsp::port_subpel_params::Mv {
                            x: best_mv.x,
                            y: best_mv.y,
                        },
                        &ctx.sf_identity,
                        &cp,
                        0, // interp_filters
                        None,
                        None,
                        geom,
                        bsize,
                        bsize,
                        &edges,
                        0,
                        0,
                        0,
                        8,
                        false,
                        false,
                    );
                }

                let inter_cost: i64 = if tpl_ctrls.use_sad_in_src_search != 0 {
                    i64::from(crate::inter_me::sad::nxm_sad_kernel(
                        &input_pic.y[src_mb..],
                        src_stride,
                        if subpel_mv {
                            &compensated_blk
                        } else {
                            &ref_pic.y[ref_origin_index..]
                        },
                        if subpel_mv { size } else { ref_pic.y_stride },
                        size,
                        size,
                    ))
                } else {
                    subtract_block(
                        size >> subsample_tx,
                        size,
                        &mut src_diff,
                        size << subsample_tx,
                        &input_pic.y[src_mb..],
                        src_stride << subsample_tx,
                        if subpel_mv {
                            &compensated_blk
                        } else {
                            &ref_pic.y[ref_origin_index..]
                        },
                        (if subpel_mv { size } else { ref_pic.y_stride }) << subsample_tx,
                    );
                    svtav1_dsp::fwd_txfm_pf::wht_fwd_txfm(
                        &src_diff,
                        size << subsample_tx,
                        &mut coeff,
                        tx_size,
                        tpl_pf_shape(tpl_ctrls),
                        8,
                        false,
                    );
                    i64::from(aom_satd(&coeff[..(size * size) >> subsample_tx])) << subsample_tx
                };

                if inter_cost < best_inter_cost {
                    if tpl_ctrls.use_sad_in_src_search == 0 {
                        best_coeff.copy_from_slice(&coeff);
                    }
                    best_ref_poc =
                        ctx.tpl_data.tpl_ref_ds[list_index][ref_pic_index].picture_number;
                    best_rf_idx = rf_idx;
                    best_inter_cost = inter_cost;
                    final_best_mv = best_mv;
                }
            } // rf_idx

            if best_inter_cost < best_intra_cost {
                best_mode = NEWMV;
            }

            if best_mode == NEWMV {
                let mut eob = 0u16;
                if tpl_ctrls.use_sad_in_src_search != 0 {
                    // Re-derive the winning prediction (the SAD path did not
                    // keep its coeff block).
                    let list_index = if best_rf_idx < 4 { 0 } else { 1 };
                    let ref_pic_index = if best_rf_idx >= 4 {
                        best_rf_idx - 4
                    } else {
                        best_rf_idx
                    } as usize;
                    let ref_pic =
                        ref_pics[ctx.tpl_data.tpl_ref_ds[list_index][ref_pic_index].pic_index];
                    let ref_origin_index = (ref_pic.origin as i32
                        + mb_origin_x as i32
                        + (final_best_mv.x as i32 >> 3)
                        + (mb_origin_y as i32 + (final_best_mv.y as i32 >> 3))
                            * ref_pic.y_stride as i32)
                        as usize;
                    let subpel_mv = (final_best_mv.x & 0x7) != 0 || (final_best_mv.y & 0x7) != 0;
                    if subpel_mv {
                        let mut conv_buf = [0u16; MAX_TPL_SAMPLES_PER_BLOCK];
                        let cp = svtav1_dsp::port_convolve::ConvolveParams::no_round(
                            false,
                            MAX_TPL_SIZE,
                            false,
                            8,
                        );
                        let _ = svtav1_dsp::port_enc_make_pred::enc_make_inter_predictor(
                            svtav1_dsp::port_enc_make_pred::SrcPlanes::Lbd(ref_pic.y),
                            ref_pic.origin,
                            ref_pic.y_stride,
                            svtav1_dsp::port_enc_make_pred::DstPlane::Lbd(&mut compensated_blk),
                            size,
                            &mut conv_buf,
                            mb_origin_y as i32,
                            mb_origin_x as i32,
                            svtav1_dsp::port_subpel_params::Mv {
                                x: final_best_mv.x,
                                y: final_best_mv.y,
                            },
                            &ctx.sf_identity,
                            &cp,
                            0, // interp_filters
                            None,
                            None,
                            geom,
                            bsize,
                            bsize,
                            &edges,
                            0,
                            0,
                            0,
                            8,
                            false,
                            false,
                        );
                    }
                    subtract_block(
                        size >> subsample_tx,
                        size,
                        &mut src_diff,
                        size << subsample_tx,
                        &input_pic.y[src_mb..],
                        src_stride << subsample_tx,
                        if subpel_mv {
                            &compensated_blk
                        } else {
                            &ref_pic.y[ref_origin_index..]
                        },
                        (if subpel_mv { size } else { ref_pic.y_stride }) << subsample_tx,
                    );
                    svtav1_dsp::fwd_txfm_pf::wht_fwd_txfm(
                        &src_diff,
                        size << subsample_tx,
                        &mut best_coeff,
                        tx_size,
                        tpl_pf_shape(tpl_ctrls),
                        8,
                        false,
                    );
                }

                get_quantize_error(
                    &ctx.quant,
                    &best_coeff,
                    &mut qcoeff,
                    &mut dqcoeff,
                    tx_size,
                    &mut eob,
                    &mut recon_error,
                    &mut sse,
                );
                let rate_cost: i64 = if tpl_ctrls.compute_rate != 0 {
                    i64::from(rate_estimator(&qcoeff, eob as usize, tx_size))
                } else {
                    0
                };
                tpl_stats.srcrf_rate = (rate_cost << TPL_DEP_COST_SCALE_LOG2) << subsample_tx;
                tpl_stats.srcrf_dist = (recon_error << TPL_DEP_COST_SCALE_LOG2) << subsample_tx;
            }
            if ctx.tpl_lad_mg > 0 {
                // store src based stats
                tpl_src_stats_buffer[src_stats_idx] = TplSrcStats {
                    srcrf_dist: tpl_stats.srcrf_dist,
                    srcrf_rate: tpl_stats.srcrf_rate,
                    mv: final_best_mv,
                    best_rf_idx,
                    ref_frame_poc: best_ref_poc,
                    best_mode,
                    best_intra_mode,
                };
            }
        } else {
            // get src based stats from previously computed data
            let s = tpl_src_stats_buffer[src_stats_idx];
            tpl_stats.srcrf_dist = s.srcrf_dist;
            tpl_stats.srcrf_rate = s.srcrf_rate;
            final_best_mv = s.mv;
            best_rf_idx = s.best_rf_idx;
            best_ref_poc = s.ref_frame_poc;
            best_mode = s.best_mode;
            best_intra_mode = s.best_intra_mode;
        }

        // Recon path — predict from the TPL RECONSTRUCTIONS.
        if best_mode == NEWMV {
            // inter recon with rec_picture as reference pic
            let ref_poc = best_ref_poc;
            let list_index = if best_rf_idx < 4 { 0usize } else { 1 };
            let ref_pic_index = if best_rf_idx >= 4 {
                (best_rf_idx - 4) as usize
            } else {
                best_rf_idx as usize
            };

            let ref_pic: TplPic<'_> = if ctx.tpl_data.ref_in_slide_window[list_index][ref_pic_index]
            {
                let mut ref_frame_idx = 0usize;
                while ref_frame_idx < MAX_TPL_LA_SW && ctx.poc_map_idx[ref_frame_idx] != ref_poc {
                    ref_frame_idx += 1;
                }
                assert!(
                    ref_frame_idx != MAX_TPL_LA_SW,
                    "tpl: in-window ref poc {ref_poc} not in poc_map_idx"
                );
                rec_refs[ref_frame_idx]
            } else {
                ref_pics[ctx.tpl_data.tpl_ref_ds[list_index][ref_pic_index].pic_index]
            };

            let ref_origin_index = (ref_pic.origin as i32
                + mb_origin_x as i32
                + (final_best_mv.x as i32 >> 3)
                + (mb_origin_y as i32 + (final_best_mv.y as i32 >> 3)) * ref_pic.y_stride as i32)
                as usize;
            // REDO COMPENSATION WITH REF PIC (INSTEAD OF REF BEING THE SRC PIC)
            let subpel_mv = (final_best_mv.x & 0x7) != 0 || (final_best_mv.y & 0x7) != 0;
            if subpel_mv {
                let mut conv_buf = [0u16; MAX_TPL_SAMPLES_PER_BLOCK];
                let cp = svtav1_dsp::port_convolve::ConvolveParams::no_round(
                    false,
                    MAX_TPL_SIZE,
                    false,
                    8,
                );
                let _ = svtav1_dsp::port_enc_make_pred::enc_make_inter_predictor(
                    svtav1_dsp::port_enc_make_pred::SrcPlanes::Lbd(ref_pic.y),
                    ref_pic.origin,
                    ref_pic.y_stride,
                    svtav1_dsp::port_enc_make_pred::DstPlane::Lbd(&mut recon.y[dst_mb_offset..]),
                    dst_buffer_stride,
                    &mut conv_buf,
                    mb_origin_y as i32,
                    mb_origin_x as i32,
                    svtav1_dsp::port_subpel_params::Mv {
                        x: final_best_mv.x,
                        y: final_best_mv.y,
                    },
                    &ctx.sf_identity,
                    &cp,
                    0, // interp_filters
                    None,
                    None,
                    geom,
                    bsize,
                    bsize,
                    &edges,
                    0,
                    0,
                    0,
                    8,
                    false,
                    false,
                );
            } else {
                for i in 0..size {
                    let src_row = ref_origin_index + i * ref_pic.y_stride;
                    let dst_row = dst_mb_offset + i * dst_buffer_stride;
                    recon.y[dst_row..dst_row + size]
                        .copy_from_slice(&ref_pic.y[src_row..src_row + size]);
                }
            }
        } else {
            // intra recon — neighbours come from the TPL RECON buffer.
            let mut above0 = [0u8; TPL_NEIGH_SZ];
            let mut left0 = [0u8; TPL_NEIGH_SZ];
            let mut above = [0u8; TPL_NEIGH_SZ];
            let mut left = [0u8; TPL_NEIGH_SZ];

            if intra_dc_sad_path {
                let mb_inside = mb_origin_x as usize + size <= input_pic.width as usize
                    && mb_origin_y as usize + size <= input_pic.height as usize;
                if mb_origin_x > 0 && mb_origin_y > 0 && mb_inside {
                    get_neighbor_samples_dc(
                        recon.y,
                        dst_buffer_stride,
                        dst_mb_offset,
                        &mut above0,
                        &mut left0,
                        bsize,
                    );
                } else {
                    // `recon.y` is the whole padded allocation — C's
                    // `recon_pic->y_buffer` is the frame-origin pointer, so
                    // pass the origin-shifted slice.
                    update_neighbor_samples_open_loop(
                        true,
                        true,
                        &recon.y[recon.origin..],
                        dst_buffer_stride,
                        input_pic.width,
                        input_pic.height,
                        mb_origin_x,
                        mb_origin_y,
                        size,
                        size,
                        &mut above0,
                        &mut left0,
                    );
                    if crate::dbgenv::blkdbg() && mb_origin_y == 0 {
                        eprintln!(
                            "  NDBG x={} y={} after-openloop corner={} a0={} l0={} l1={}",
                            mb_origin_x, mb_origin_y, above0[0], above0[1], left0[0], left0[1]
                        );
                    }
                }
                let n = crate::intra_open_loop::Neighbours {
                    above: &above0[1..1 + size],
                    left: &left0[1..1 + size],
                    top_left: above0[0],
                    has_left: mb_origin_x > 0,
                    has_above: mb_origin_y > 0,
                };
                let pred_rc = crate::intra_open_loop::intra_prediction_open_loop_mb(
                    svtav1_types::prediction::PredictionMode::DcPred,
                    0,
                    n,
                    size,
                    size,
                    &mut recon.y[dst_mb_offset..],
                    dst_buffer_stride,
                );
                if crate::dbgenv::blkdbg() && mb_origin_y == 0 {
                    eprintln!(
                        "  PDBG x={} y={} rc={:?} p0={} p15={} p240={}",
                        mb_origin_x,
                        mb_origin_y,
                        pred_rc,
                        recon.y[dst_mb_offset],
                        recon.y[dst_mb_offset + 15],
                        recon.y[dst_mb_offset + 15 * dst_buffer_stride],
                    );
                }
            } else {
                update_neighbor_samples_open_loop(
                    true,
                    true,
                    &recon.y[recon.origin..],
                    dst_buffer_stride,
                    input_pic.width,
                    input_pic.height,
                    mb_origin_x,
                    mb_origin_y,
                    size,
                    size,
                    &mut above0,
                    &mut left0,
                );
                let p_angle = if crate::intra_open_loop::is_directional_mode(
                    tpl_mode(best_intra_mode).expect("stored intra mode"),
                ) {
                    crate::intra_edge::MODE_TO_ANGLE_MAP[best_intra_mode as usize]
                } else {
                    0
                };
                tpl_predict_intra(
                    best_intra_mode,
                    p_angle,
                    ctx.max_input_luma_width,
                    ctx.max_input_luma_height,
                    mb_origin_x,
                    mb_origin_y,
                    &above0,
                    &left0,
                    &mut above,
                    &mut left,
                    size,
                    // predict straight into the recon buffer — C's dst_buffer
                    &mut recon.y[dst_mb_offset..],
                    dst_buffer_stride,
                );
            }

            if crate::dbgenv::blkdbg() && mb_origin_x < 32 && mb_origin_y == 0 {
                let mut psum = 0u64;
                for r in 0..size {
                    for c in 0..size {
                        psum += u64::from(recon.y[dst_mb_offset + r * dst_buffer_stride + c])
                            * (r * size + c + 1) as u64;
                    }
                }
                eprintln!(
                    "  PREDBG x={} y={} corner={} above0={} above1={} above15={} leftm1={} left0={} left14={} psum={} dcsad={} bim={} dip={} size={} bsize={}",
                    mb_origin_x,
                    mb_origin_y,
                    above0[0],
                    above0[1],
                    above0[2],
                    above0[16],
                    left0[0],
                    left0[1],
                    left0[15],
                    psum,
                    intra_dc_sad_path,
                    best_intra_mode,
                    disable_intra_pred,
                    size,
                    bsize,
                );
            }
        }

        subtract_block(
            size >> subsample_tx,
            size,
            &mut src_diff,
            size << subsample_tx,
            &input_pic.y[src_mb..],
            src_stride << subsample_tx,
            &recon.y[dst_mb_offset..],
            dst_buffer_stride << subsample_tx,
        );
        svtav1_dsp::fwd_txfm_pf::wht_fwd_txfm(
            &src_diff,
            size << subsample_tx,
            &mut coeff,
            tx_size,
            tpl_pf_shape(tpl_ctrls),
            8,
            false,
        );

        let mut eob = 0u16;
        get_quantize_error(
            &ctx.quant,
            &coeff,
            &mut qcoeff,
            &mut dqcoeff,
            tx_size,
            &mut eob,
            &mut recon_error,
            &mut sse,
        );
        let rate_cost: i64 = if tpl_ctrls.compute_rate != 0 {
            i64::from(rate_estimator(&qcoeff, eob as usize, tx_size))
        } else {
            0
        };

        if !disable_intra_pred || ctx.tpl_data.is_ref {
            if eob != 0 {
                let _ = svtav1_dsp::txfm_dispatch::inv_transform_recon8bit_in_place(
                    &dqcoeff,
                    size,
                    &mut recon.y[dst_mb_offset..],
                    dst_buffer_stride << subsample_tx,
                    tx_size,
                    TxType::DctDct,
                    u32::from(eob),
                    false,
                );
                // populate the subsampled rows with a copy of the neighbour.
                // `copy_within`, not a `[u8; MAX_TPL_SIZE]` staging array: the
                // TPL block is 16 wide on small frames, and `try_into` of a
                // 16-sample row into a 32-array panicked (130x98, RA + aq_mode
                // 2, presets 9 and 12; found 2026-09-26 by video_paths.rs).
                if subsample_tx == 2 {
                    for i in (0..size).step_by(4) {
                        for k in 1..=3usize {
                            let src_row = dst_mb_offset + i * dst_buffer_stride;
                            let dst_row = dst_mb_offset + (i + k) * dst_buffer_stride;
                            recon.y.copy_within(src_row..src_row + size, dst_row);
                        }
                    }
                } else if subsample_tx == 1 {
                    for i in (0..size).step_by(2) {
                        let src_row = dst_mb_offset + i * dst_buffer_stride;
                        let dst_row = dst_mb_offset + (i + 1) * dst_buffer_stride;
                        recon.y.copy_within(src_row..src_row + size, dst_row);
                    }
                }
            }
        }

        tpl_stats.recrf_dist = (recon_error << TPL_DEP_COST_SCALE_LOG2) << subsample_tx;
        tpl_stats.recrf_rate = (rate_cost << TPL_DEP_COST_SCALE_LOG2) << subsample_tx;
        if best_mode != NEWMV {
            tpl_stats.srcrf_dist = (recon_error << TPL_DEP_COST_SCALE_LOG2) << subsample_tx;
            tpl_stats.srcrf_rate = (rate_cost << TPL_DEP_COST_SCALE_LOG2) << subsample_tx;
        }

        tpl_stats.recrf_dist = tpl_stats.srcrf_dist.max(tpl_stats.recrf_dist);
        tpl_stats.recrf_rate = tpl_stats.srcrf_rate.max(tpl_stats.recrf_rate);
        if ctx.tpl_data.tpl_slice_type != crate::port_picstruct::SliceType::I && best_rf_idx != -1 {
            tpl_stats.mv = final_best_mv;
            tpl_stats.ref_frame_poc = best_ref_poc;
        }

        if crate::dbgenv::blkdbg() {
            let mut rsum = 0u64;
            for r in 0..size {
                for c in 0..size {
                    rsum += u64::from(recon.y[dst_mb_offset + r * dst_buffer_stride + c])
                        * (r * size + c + 1) as u64;
                }
            }
            let mut ssum = 0u64;
            for r in 0..size {
                for c in 0..size {
                    ssum += u64::from(input_pic.y[src_mb + r * src_stride + c])
                        * (r * size + c + 1) as u64;
                }
            }
            eprintln!(
                "  BLKDBG x={} y={} sz={} src={} rec={} mode={} brf={} err={} rsum={} ssum={}",
                mb_origin_x,
                mb_origin_y,
                size,
                tpl_stats.srcrf_dist,
                tpl_stats.recrf_dist,
                best_mode as i32,
                best_rf_idx,
                recon_error,
                rsum,
                ssum
            );
        }

        // Motion flow dependency dispenser.
        result_model_store(
            &mut tpl_stats,
            tpl_stats_grid,
            tpl_ctrls.synth_blk_size,
            ctx.aligned_width,
            mb_origin_x,
            mb_origin_y,
            size as u32,
        );
    }
}

// =============================================================================
// tpl_prep_info + tpl_regular_setup_me_refs (src_ops_process.c:117-184)
// =============================================================================

/// C `RDDIV_BITS` (rd_cost.h:34).
pub(super) const RDDIV_BITS: u32 = 7;
/// C `AV1_PROB_COST_SHIFT`.
pub(super) const AV1_PROB_COST_SHIFT: u32 = 9;

/// The fields one `pcs->tpl_group[i]` picture contributes to
/// [`tpl_prep_info`]/[`tpl_regular_setup_me_refs`].
#[derive(Debug, Clone)]
pub struct TplPrepPic {
    /// C `pcs_tpl->slice_type`.
    pub slice_type: crate::port_picstruct::SliceType,
    /// C `pcs_tpl->temporal_layer_index`.
    pub temporal_layer_index: u8,
    /// C `pcs_tpl->is_ref`.
    pub is_ref: bool,
    /// C `pcs_tpl->decode_order`.
    pub decode_order: i32,
    /// C `pcs_tpl->picture_number` (POC).
    pub picture_number: u64,
    /// C `ref_list0_count_try` / `ref_list1_count_try`.
    pub ref_list_count_try: [u8; 2],
    /// C `ref_pic_poc_array[list][idx]`.
    pub ref_pic_poc: [[u64; REF_LIST_MAX_DEPTH]; 2],
    /// For each ref, the index of its `EbPaReferenceObject->input_padded_pic`
    /// in the caller's `ref_pics` table (C stores the pointer; the port
    /// carries the index).
    pub ref_pic_index: [[usize; REF_LIST_MAX_DEPTH]; 2],
    /// For each ref, `EbPaReferenceObject->picture_number`.
    pub ref_poc_ds: [[u64; REF_LIST_MAX_DEPTH]; 2],
}

/// C `tpl_regular_setup_me_refs`: for each non-I picture's two ref lists,
/// record the ref count, mark window membership, and fill `tpl_ref_ds` with
/// (poc, ref-picture index).
pub fn tpl_regular_setup_me_refs(group_pocs: &[u64], pic: &TplPrepPic, tpl_data: &mut TplData) {
    for list_index in 0..2usize {
        let ref_list_count = pic.ref_list_count_try[list_index];
        if list_index == 0 {
            tpl_data.tpl_ref0_count = ref_list_count;
        } else {
            tpl_data.tpl_ref1_count = ref_list_count;
        }

        for ref_idx in 0..ref_list_count as usize {
            let ref_poc = pic.ref_pic_poc[list_index][ref_idx];

            tpl_data.ref_tpl_group_idx[list_index][ref_idx] = -1;
            for (j, &poc) in group_pocs.iter().enumerate() {
                if ref_poc == poc {
                    tpl_data.ref_in_slide_window[list_index][ref_idx] = true;
                    tpl_data.ref_tpl_group_idx[list_index][ref_idx] = j as i32;
                    break;
                }
            }

            tpl_data.tpl_ref_ds[list_index][ref_idx] = TplRefDs {
                picture_number: pic.ref_poc_ds[list_index][ref_idx],
                pic_index: pic.ref_pic_index[list_index][ref_idx],
            };
        }
    }
}

/// C `tpl_prep_info`: zero each picture's ref tables, copy its
/// slice/layer/ref/decode-order fields, then set up ME refs for non-I
/// pictures. `group_pocs` is the window's `tpl_group[i]->picture_number`
/// list; `pics` is the same group in the same order, and the returned
/// `tpl_data` vector is 1:1 with it.
pub fn tpl_prep_info(group_pocs: &[u64], pics: &[TplPrepPic]) -> Vec<TplData> {
    assert_eq!(group_pocs.len(), pics.len());
    pics.iter()
        .map(|pic| {
            let mut tpl_data = TplData {
                tpl_slice_type: pic.slice_type,
                tpl_temporal_layer_index: pic.temporal_layer_index,
                is_ref: pic.is_ref,
                tpl_decode_order: pic.decode_order,
                ..Default::default()
            };
            if tpl_data.tpl_slice_type != crate::port_picstruct::SliceType::I {
                tpl_regular_setup_me_refs(group_pocs, pic, &mut tpl_data);
            }
            tpl_data
        })
        .collect()
}

// =============================================================================
// generate_lambda_scaling_factor (src_ops_process.c:184-232)
// =============================================================================
