use super::*;

/// C `get_neighbor_samples_dc` (src_ops_process.c:359-376) — copy the
/// block's real above/left neighbours from an already-reconstructed buffer.
///
/// `above`/`left` carry the C convention: index 0 is the corner
/// (`above0_row[-1]` / `left0_col[-1]`).
pub fn get_neighbor_samples_dc(
    src: &[u8],
    src_stride: usize,
    origin: usize,
    above: &mut [u8],
    left: &mut [u8],
    bsize: usize,
) {
    // top left
    above[0] = src[origin - src_stride - 1];
    left[0] = above[0];
    // top
    above[1..1 + bsize].copy_from_slice(&src[origin - src_stride..origin - src_stride + bsize]);
    // left
    let mut read = origin - 1;
    for l in left.iter_mut().take(1 + bsize).skip(1) {
        *l = src[read];
        read += src_stride;
    }
}

/// C `svt_aom_update_neighbor_samples_array_open_loop_mb`
/// (enc_intra_prediction.c:731-816) on a flat buffer: the C version reads
/// `input_ptr->y_buffer`; passing `(buf, stride, width, height)` makes this
/// one signature serve both the source and the `_recon` variant
/// (enc_intra_prediction.c:818-927 — byte-identical except it takes the
/// buffer + dims directly rather than through `EbPictureBufferDesc`).
#[allow(clippy::too_many_arguments)]
pub fn update_neighbor_samples_open_loop(
    use_top_right_bottom_left: bool,
    update_top_neighbor: bool,
    buf: &[u8],
    stride: usize,
    width: u32,
    height: u32,
    src_origin_x: u32,
    src_origin_y: u32,
    bwidth: usize,
    bheight: usize,
    above_ref: &mut [u8],
    left_ref: &mut [u8],
) {
    let block_width_neigh = if use_top_right_bottom_left {
        bwidth << 1
    } else {
        bwidth
    };
    let block_height_neigh = if use_top_right_bottom_left {
        bheight << 1
    } else {
        bheight
    };
    let origin = src_origin_y as usize * stride + src_origin_x as usize;

    // C initialises above to 127 and left to 129 INCLUDING the corner slot.
    for v in above_ref.iter_mut().take(block_width_neigh + 1) {
        *v = 127;
    }
    for v in left_ref.iter_mut().take(block_height_neigh + 1) {
        *v = 129;
    }

    // Upper left sample. C writes it into above_ref[0]/left_ref[0] then
    // bumps both pointers — same as the [0]=corner, [1..]=row convention.
    if src_origin_x != 0 && src_origin_y != 0 {
        let t = buf[origin - stride - 1];
        above_ref[0] = t;
        left_ref[0] = t;
    } else {
        above_ref[0] = 128;
        left_ref[0] = 128;
    }

    // Left column.
    let mut count = block_width_neigh;
    if src_origin_x != 0 {
        let mut read = origin - 1;
        if src_origin_y == 0 {
            // C writes the top-row pixel into left_ref[-1]... i.e. [0].
            left_ref[0] = buf[read];
        }
        count = if src_origin_y + count as u32 > height {
            count - ((src_origin_y + count as u32) - height) as usize
        } else {
            count
        };
        // C writes left_ref[1..=count].
        for l in left_ref.iter_mut().take(1 + count).skip(1) {
            *l = buf[read];
            read += stride;
        }
        if use_top_right_bottom_left {
            // pad unknown left-bottom pixels with value at (-1, -15):
            // left_ref[bheight+1 ..= 2*bheight] = left_ref[bheight].
            let v = left_ref[bheight];
            for l in left_ref
                .iter_mut()
                .take(1 + block_height_neigh)
                .skip(1 + bheight)
            {
                *l = v;
            }
        }
    } else if src_origin_y != 0 {
        count = if src_origin_y + count as u32 > height {
            count - ((src_origin_y + count as u32) - height) as usize
        } else {
            count
        };
        // C: EB_MEMSET(left_ref - 1, src_ptr[-stride], count + 1) — the corner
        // AND the first `count` left entries all take the above row's pixel;
        // *(above_ref - 1) = same. With [0]=corner that is:
        let t = buf[origin - stride];
        for l in left_ref.iter_mut().take(1 + count) {
            *l = t;
        }
        above_ref[0] = t;
    }

    // Top row.
    let mut count = block_width_neigh;
    if src_origin_y != 0 {
        if update_top_neighbor {
            let read = origin - stride;
            count = if src_origin_x + count as u32 > width {
                count - ((src_origin_x + count as u32) - width) as usize
            } else {
                count
            };
            above_ref[1..1 + count].copy_from_slice(&buf[read..read + count]);
        }
        // pad unknown top-right pixels with value at (15, -1)
        if use_top_right_bottom_left && src_origin_x != 0 {
            let v = above_ref[bwidth];
            for a in above_ref
                .iter_mut()
                .take(1 + block_width_neigh)
                .skip(1 + bwidth)
            {
                *a = v;
            }
        }
    } else if src_origin_x != 0 {
        count = if src_origin_x + count as u32 > width {
            count - ((src_origin_x + count as u32) - width) as usize
        } else {
            count
        };
        // C: EB_MEMSET(above_ref - 1, *(left_ref - count), count + 1) —
        // `left_ref` has already advanced to base+1+block_width_neigh (the
        // left column fill ran first, src_origin_x != 0), so the value is
        // `left[1 + block_width_neigh - count]`: the LEFT sample `count`
        // rows up from the fill's end — the corner plus `count` above
        // entries all take it.
        let v = left_ref[1 + block_width_neigh - count];
        for a in above_ref.iter_mut().take(1 + count) {
            *a = v;
        }
    }
}

// =============================================================================
// init_xd_tpl + MV cost params (src_ops_process.c:384-419)
// =============================================================================

/// The `MacroBlockD` fields TPL uses (`init_xd_tpl`, src_ops_process.c:404).
#[derive(Debug, Clone, Copy)]
pub struct TplXd {
    /// `xd->mb_to_top_edge`.
    pub to_top: i32,
    /// `xd->mb_to_bottom_edge`.
    pub to_bottom: i32,
    /// `xd->mb_to_left_edge`.
    pub to_left: i32,
    /// `xd->mb_to_right_edge`.
    pub to_right: i32,
    /// `xd->mi_row`.
    pub mi_row: i32,
    /// `xd->mi_col`.
    pub mi_col: i32,
}

impl TplXd {
    /// `init_xd_tpl` — `mi_row`/`mi_col` are the block's position, the edges
    /// are `(0,0)`-relative distance-to-frame-edge in pixels.
    pub fn new(
        mi_rows: i32,
        mi_cols: i32,
        block_size: BlockSize,
        mb_origin_x: u32,
        mb_origin_y: u32,
    ) -> Self {
        const MI_SIZE: i32 = 4;
        let bw = svtav1_dsp::port_obmc_pred::MI_SIZE_WIDE[block_size as usize] as i32;
        let bh = svtav1_dsp::port_obmc_pred::MI_SIZE_HIGH[block_size as usize] as i32;
        let mi_row = (mb_origin_y >> 2) as i32;
        let mi_col = (mb_origin_x >> 2) as i32;
        Self {
            to_top: -((mi_row * MI_SIZE) * 8),
            to_bottom: ((mi_rows - bh - mi_row) * MI_SIZE) * 8,
            to_left: -((mi_col * MI_SIZE) * 8),
            to_right: ((mi_cols - bw - mi_col) * MI_SIZE) * 8,
            mi_row,
            mi_col,
        }
    }

    /// The [`svtav1_dsp::port_subpel_params::MbEdges`] view.
    pub fn edges(&self) -> svtav1_dsp::port_subpel_params::MbEdges {
        svtav1_dsp::port_subpel_params::MbEdges {
            to_left: self.to_left,
            to_right: self.to_right,
            to_top: self.to_top,
            to_bottom: self.to_bottom,
        }
    }
}

/// C `svt_tpl_init_mv_cost_params` (src_ops_process.c:384-404) — TPL always
/// uses `MV_COST_NONE` (no rate update inside the dispenser). C also writes
/// `full_ref_mv` and `sad_per_bit`, which no `mcomp.c` consumer reads —
/// the port's [`crate::md_subpel::MvCostParams`] omits both on purpose.
pub fn tpl_init_mv_cost_params(ref_mv: Mv, rdmult: u32) -> crate::md_subpel::MvCostParams<'static> {
    crate::md_subpel::MvCostParams {
        ref_mv,
        mv_cost_type: crate::md_subpel::MvCostType::None,
        tables: None,
        // `AOMMAX(rdmult >> RD_EPB_SHIFT, 1)` — RD_EPB_SHIFT = 6.
        error_per_bit: (rdmult >> 6).max(1) as i32,
        early_exit_th: 0,
    }
}

/// C `tpl_subpel_search` (src_ops_process.c:419-516) — refine the ME MV to
/// sub-pel via `find_best_sub_pixel_tree_pruned`, always `USE_4_TAPS`.
#[allow(clippy::too_many_arguments)]
pub fn tpl_subpel_search(
    tpl_ctrls: &crate::port_picstruct::TplControls,
    update_type: crate::port_picstruct::FrameUpdateType,
    qp_qindex: u8,
    extended_crf_qindex_offset: i32,
    mi_rows: i32,
    mi_cols: i32,
    ref_pic: &TplPic<'_>,
    input_pic: &TplPic<'_>,
    mb_origin_x: u32,
    mb_origin_y: u32,
    bsize: u8,
    best_mv: &mut Mv,
) {
    let block_size = match bsize {
        8 => BlockSize::Block8x8,
        16 => BlockSize::Block16x16,
        32 => BlockSize::Block32x32,
        _ => BlockSize::Block64x64,
    };
    let xd = TplXd::new(mi_rows, mi_cols, block_size, mb_origin_x, mb_origin_y);
    let ref_mv = Mv::ZERO;

    // Full-pel limits, then sub-pel.
    let mi_width = svtav1_dsp::port_obmc_pred::MI_SIZE_WIDE[block_size as usize] as i32;
    let mi_height = svtav1_dsp::port_obmc_pred::MI_SIZE_HIGH[block_size as usize] as i32;
    const AOM_INTERP_EXTEND: i32 = 4;
    const MI_SIZE: i32 = 4;
    let mut full_limits = svtav1_types::motion::FullMvLimits {
        col_min: -(((xd.mi_col + mi_width) * MI_SIZE) + AOM_INTERP_EXTEND),
        col_max: (mi_cols - xd.mi_col) * MI_SIZE + AOM_INTERP_EXTEND,
        row_min: -(((xd.mi_row + mi_height) * MI_SIZE) + AOM_INTERP_EXTEND),
        row_max: (mi_rows - xd.mi_row) * MI_SIZE + AOM_INTERP_EXTEND,
    };
    crate::intrabc::set_mv_search_range(&mut full_limits, ref_mv);
    let mv_limits = crate::md_subpel::set_subpel_mv_search_range(
        (
            full_limits.col_min,
            full_limits.col_max,
            full_limits.row_min,
            full_limits.row_max,
        ),
        ref_mv,
    );

    // MV cost params — C keys the qindex off static_config.qp through
    // quantizer_to_qindex + the extended CRF offset, clamps to MAXQ, and
    // scales rdmult by TPL_RDMULT_SCALING_FACTOR.
    let qindex = i32::from(qp_qindex) + extended_crf_qindex_offset;
    let qindex = qindex.min(crate::port_rc_vbr_cbr_qpick::MAXQ);
    let rc_update_type = match update_type {
        crate::port_picstruct::FrameUpdateType::Kf => crate::port_rc_process::FrameUpdateType::Kf,
        crate::port_picstruct::FrameUpdateType::Lf => crate::port_rc_process::FrameUpdateType::Lf,
        crate::port_picstruct::FrameUpdateType::Gf => crate::port_rc_process::FrameUpdateType::Gf,
        crate::port_picstruct::FrameUpdateType::Arf => crate::port_rc_process::FrameUpdateType::Arf,
        crate::port_picstruct::FrameUpdateType::Overlay => {
            crate::port_rc_process::FrameUpdateType::Overlay
        }
        crate::port_picstruct::FrameUpdateType::IntnlOverlay => {
            crate::port_rc_process::FrameUpdateType::IntnlOverlay
        }
        crate::port_picstruct::FrameUpdateType::IntnlArf => {
            crate::port_rc_process::FrameUpdateType::IntnlArf
        }
    };
    let rdmult = crate::port_rc_process::compute_rd_mult_based_on_qindex(8, rc_update_type, qindex)
        / TPL_RDMULT_SCALING_FACTOR;
    let mv_cost_params = tpl_init_mv_cost_params(ref_mv, rdmult.max(0) as u32);

    let (w, h) = (
        svtav1_dsp::port_obmc_data::block_size_wide(block_size),
        svtav1_dsp::port_obmc_data::block_size_high(block_size),
    );
    let ref_origin =
        ref_pic.origin + mb_origin_x as usize + mb_origin_y as usize * ref_pic.y_stride;
    let src_origin =
        input_pic.origin + mb_origin_x as usize + mb_origin_y as usize * input_pic.y_stride;

    let var_params = crate::md_subpel::SubpelSearchVarParams {
        src: input_pic.y,
        src_base: src_origin,
        src_stride: input_pic.y_stride,
        ref_alloc: ref_pic.y,
        ref_base: ref_origin as i64,
        ref_stride: ref_pic.y_stride,
        w,
        h,
        bias_fp: 0,
        subpel_search_type: crate::inter_me::obmc_search::USE_4_TAPS,
    };
    let ms_params = crate::md_subpel::SubpelSearchParams {
        allow_hp: false, // pcs->frm_hdr.allow_high_precision_mv is not set yet
        forced_stop: i32::from(tpl_ctrls.subpel_depth),
        iters_per_step: 2,
        pred_variance_th: 0,
        abs_th_mult: 0,
        round_dev_th: i32::MAX,
        skip_diag_refinement: tpl_ctrls.subpel_diag_refinement,
        search_stage: crate::md_subpel::SPEL_ME,
        list_idx: 0,
        ref_idx: 0,
        mv_limits,
    };

    // C takes best_mv >> 3 as the full-pel start (TODO in C says this should
    // be get_fullmv_from_mv), then get_mv_from_fullmv shifts it back — a
    // floor, not the rounding `to_fullpel` does, so keep the shift.
    let start_mv = Mv {
        x: best_mv.x >> 3,
        y: best_mv.y >> 3,
    }
    .to_subpel();
    let (_besterr, st) = crate::md_subpel::find_best_sub_pixel_tree_pruned(
        None,
        &ms_params,
        &var_params,
        &mv_cost_params,
        start_mv,
        block_size,
    );
    *best_mv = st.best_mv;
}

// =============================================================================
// get_quantize_error + rate_estimator (src_ops_process.c:232-273)
// =============================================================================

/// C `svt_av1_block_error` (`aom_dsp_rtcd`): `error = Σ(coeff-dqcoeff)²`,
/// `ssz = Σcoeff²` over `pix_num` raster entries.
pub(super) fn av1_block_error(coeff: &[i32], dqcoeff: &[i32], pix_num: usize) -> (i64, i64) {
    let mut error = 0i64;
    let mut sqcoeff = 0i64;
    for i in 0..pix_num {
        let diff = i64::from(coeff[i]) - i64::from(dqcoeff[i]);
        error += diff * diff;
        sqcoeff += i64::from(coeff[i]) * i64::from(coeff[i]);
    }
    (error, sqcoeff)
}

/// C `get_quantize_error` (src_ops_process.c:232-257): FP-quantize `coeff`
/// with the TPL quant table, then `recon_error`/`sse` are the block errors
/// shifted down 2 (0 at TX_32X32) and floored at 1.
#[allow(clippy::too_many_arguments)]
pub fn get_quantize_error(
    quant: &crate::quant::QuantTable,
    coeff: &[i32],
    qcoeff: &mut [i32],
    dqcoeff: &mut [i32],
    tx_size: TxSize,
    eob: &mut u16,
    recon_error: &mut i64,
    sse: &mut i64,
) {
    let scan = crate::entropy::scan_tables::scan(tx_size as usize, 0);
    // `1 << num_pels_log2_lookup[txsize_to_bsize[tx_size]]` — every TPL tx
    // is `size x (size >> subsample)` so the pixel count is w*h directly.
    let (txw, txh) = tx_dims(tx_size);
    let pix_num = txw * txh;
    let shift: i64 = if tx_size == TxSize::Tx32x32 { 0 } else { 2 };

    // C `svt_av1_quantize_fp` — `quantize_fp_helper_c` `(void)`-ignores its
    // `zbin_ptr` and `quant_shift_ptr`, so the `mb_plane.zbin_qtx`/
    // `quant_shift_qtx` rows C loads are inert here.
    *eob = crate::quant::quantize_fp(coeff, scan, quant, 0, qcoeff, dqcoeff);

    let (err, sq) = av1_block_error(coeff, dqcoeff, pix_num);
    *recon_error = (err >> shift).max(1);
    *sse = (sq >> shift).max(1);
}

/// C `rate_estimator` (src_ops_process.c:258-272): `eob + 1` plus
/// `floor(log2(level+1)) + (level > 0)` per coded coeff, in
/// `AV1_PROB_COST_SHIFT` fixed point.
pub fn rate_estimator(qcoeff: &[i32], eob: usize, tx_size: TxSize) -> i32 {
    let scan = crate::entropy::scan_tables::scan(tx_size as usize, 0);
    const AV1_PROB_COST_SHIFT: i32 = 9;
    let mut rate_cost = eob as i32 + 1;
    for idx in 0..eob {
        let abs_level = qcoeff[scan[idx] as usize].abs();
        // `svt_log2f` = get_msb = floor(log2(x)).
        rate_cost += msb(abs_level as u32 + 1) + i32::from(abs_level > 0);
    }
    rate_cost << AV1_PROB_COST_SHIFT
}

/// C `svt_log2f` / `get_msb` (definitions.h:613): position of the top bit.
pub(super) fn msb(x: u32) -> i32 {
    31 - x.leading_zeros() as i32
}

// =============================================================================
// result_model_store (src_ops_process.c:273-352)
// =============================================================================

/// C `result_model_store`: clamp stats to >= 1 and write them into the
/// `tpl_stats` grid at `synth_blk_size` granularity (32/16/8 — the 8x8 arm
/// also normalizes a 32- or 16-sized result down).
pub fn result_model_store(
    tpl_stats: &mut TplStats,
    tpl_stats_grid: &mut [TplStats],
    synth_blk_size: u8,
    aligned_width: u32,
    mb_origin_x: u32,
    mb_origin_y: u32,
    size: u32,
) {
    tpl_stats.srcrf_dist = tpl_stats.srcrf_dist.max(1);
    tpl_stats.recrf_dist = tpl_stats.recrf_dist.max(1);
    tpl_stats.srcrf_rate = tpl_stats.srcrf_rate.max(1);
    tpl_stats.recrf_rate = tpl_stats.recrf_rate.max(1);

    if synth_blk_size == 32 {
        let stride = (aligned_width as usize + 31) / 32;
        let idx = (mb_origin_y as usize >> 5) * stride + (mb_origin_x as usize >> 5);
        tpl_stats_grid[idx] = *tpl_stats;
    } else if synth_blk_size == 16 {
        let stride = (aligned_width as usize + 15) / 16;
        let idx = (mb_origin_y as usize >> 4) * stride + (mb_origin_x as usize >> 4);
        if size == 32 {
            // normalize based on the block size 16x16
            tpl_stats.srcrf_dist = (tpl_stats.srcrf_dist / 4).max(1);
            tpl_stats.recrf_dist = (tpl_stats.recrf_dist / 4).max(1);
            tpl_stats.srcrf_rate = (tpl_stats.srcrf_rate / 4).max(1);
            tpl_stats.recrf_rate = (tpl_stats.recrf_rate / 4).max(1);
            tpl_stats_grid[idx] = *tpl_stats;
            tpl_stats_grid[idx + 1] = *tpl_stats;
            tpl_stats_grid[idx + stride] = *tpl_stats;
            tpl_stats_grid[idx + stride + 1] = *tpl_stats;
        } else if size == 16 {
            tpl_stats_grid[idx] = *tpl_stats;
        }
    } else {
        // small resolution: 16x16 data duplicated on an 8x8 grid
        let stride = ((aligned_width as usize + 15) / 16) << 1;
        let idx = (mb_origin_y as usize >> 3) * stride + (mb_origin_x as usize >> 3);
        if size == 32 {
            tpl_stats.srcrf_dist = (tpl_stats.srcrf_dist / 16).max(1);
            tpl_stats.recrf_dist = (tpl_stats.recrf_dist / 16).max(1);
            tpl_stats.srcrf_rate = (tpl_stats.srcrf_rate / 16).max(1);
            tpl_stats.recrf_rate = (tpl_stats.recrf_rate / 16).max(1);
            for r in 0..4usize {
                for c in 0..4usize {
                    tpl_stats_grid[idx + r * stride + c] = *tpl_stats;
                }
            }
        } else if size == 16 {
            tpl_stats.srcrf_dist = (tpl_stats.srcrf_dist / 4).max(1);
            tpl_stats.recrf_dist = (tpl_stats.recrf_dist / 4).max(1);
            tpl_stats.srcrf_rate = (tpl_stats.srcrf_rate / 4).max(1);
            tpl_stats.recrf_rate = (tpl_stats.recrf_rate / 4).max(1);
            for r in 0..2usize {
                for c in 0..2usize {
                    tpl_stats_grid[idx + r * stride + c] = *tpl_stats;
                }
            }
        }
    }
}

// =============================================================================
// tpl_mc_flow_dispenser_sb_generic (src_ops_process.c:517-1202)
// =============================================================================
