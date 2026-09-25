use super::*;

/// C `generate_lambda_scaling_factor` — per-16x16 (or 32x32) grid, combine
/// `recrf_dist` against the MC-dependency delta into `c = 1.2 + rk/r0`.
///
/// `tpl_stats` is the flat `pa_me_data->tpl_stats` grid at
/// `mi_cols_sr >> tpl_synth_size_offset` stride; `tpl_rdmult_scaling_factors`
/// is the `num_cols x num_rows` output.
#[allow(clippy::too_many_arguments)]
pub fn generate_lambda_scaling_factor(
    synth_blk_size: u8,
    mi_rows: i32,
    enhanced_unscaled_width: u32,
    base_rdmult: i64,
    mc_dep_cost_base: i64,
    r0: f64,
    tpl_stats: &[TplStats],
    tpl_rdmult_scaling_factors: &mut [f64],
) {
    let tpl_synth_size_offset: i32 = if synth_blk_size == 8 {
        1
    } else if synth_blk_size == 16 {
        2
    } else {
        3
    };
    let step = 1i32 << tpl_synth_size_offset;
    let mi_cols_sr = ((enhanced_unscaled_width as i32 + 15) / 16) << 2;
    // BLOCK_32X32 at synth 32 else BLOCK_16X16 — 8x8 shares the 16 grid.
    let (num_mi_w, num_mi_h) = if synth_blk_size == 32 { (8, 8) } else { (4, 4) };
    let num_cols = (mi_cols_sr + num_mi_w - 1) / num_mi_w;
    let num_rows = (mi_rows + num_mi_h - 1) / num_mi_h;
    let stride = mi_cols_sr >> tpl_synth_size_offset;
    let c = 1.2f64;

    for row in 0..num_rows {
        for col in 0..num_cols {
            let mut recrf_dist_sum: i64 = 0;
            let mut mc_dep_delta_sum: i64 = 0;
            let index = (row * num_cols + col) as usize;
            let mut mi_row = row * num_mi_h;
            while mi_row < (row + 1) * num_mi_h {
                let mut mi_col = col * num_mi_w;
                while mi_col < (col + 1) * num_mi_w {
                    if mi_row < mi_rows && mi_col < mi_cols_sr {
                        let index1 = ((mi_row >> tpl_synth_size_offset) * stride
                            + (mi_col >> tpl_synth_size_offset))
                            as usize;
                        let st = &tpl_stats[index1];
                        let mc_dep_delta = rdcost_tpl(base_rdmult, st.mc_dep_rate, st.mc_dep_dist);
                        recrf_dist_sum += st.recrf_dist;
                        mc_dep_delta_sum += mc_dep_delta;
                    }
                    mi_col += step;
                }
                mi_row += step;
            }
            let mut scaling_factors = c;
            if mc_dep_cost_base != 0 && recrf_dist_sum > 0 {
                let rk = ((recrf_dist_sum << RDDIV_BITS) as f64)
                    / (((recrf_dist_sum << RDDIV_BITS) + mc_dep_delta_sum) as f64);
                scaling_factors += rk / r0;
            }
            tpl_rdmult_scaling_factors[index] = scaling_factors;
        }
    }
}

// =============================================================================
// tpl_mc_flow_dispenser (src_ops_process.c:1338-1411)
// =============================================================================

/// C's `quantizer_to_qindex[static_config.qp] + extended_crf_qindex_offset`,
/// clamped to MAXQ, then the optional `enable_tpl_qps` per-temporal-layer
/// delta — the TPL picture's `qIndex`.
pub fn tpl_qindex(
    qp: u8,
    extended_crf_qindex_offset: i32,
    enable_tpl_qps: bool,
    tpl_slice_type: crate::port_picstruct::SliceType,
    hierarchical_levels: u8,
    tpl_temporal_layer_index: u8,
) -> i32 {
    let mut qindex = i32::from(crate::rate_control::QUANTIZER_TO_QINDEX[qp.min(63) as usize])
        + extended_crf_qindex_offset;
    qindex = qindex.min(crate::port_rc_vbr_cbr_qpick::MAXQ);

    if enable_tpl_qps {
        #[rustfmt::skip]
        const DELTA_RATE_NEW: [[f64; 6]; 7] = [
            [1.0, 1.0, 1.0, 1.0, 1.0, 1.0], // 1L
            [0.6, 1.0, 1.0, 1.0, 1.0, 1.0], // 2L
            [0.6, 0.8, 1.0, 1.0, 1.0, 1.0], // 3L
            [0.6, 0.8, 0.9, 1.0, 1.0, 1.0], // 4L
            [0.35, 0.6, 0.8, 0.9, 1.0, 1.0], //5L
            [0.35, 0.6, 0.8, 0.9, 0.95, 1.0], //6L
            [0.0, 0.0, 0.0, 0.0, 0.0, 0.0], // C indexes [hier][tl]; row 6 never read
        ];
        let q_val = crate::rate_control::convert_qindex_to_q(qindex, 8);
        let delta_qindex = if tpl_slice_type == crate::port_picstruct::SliceType::I {
            crate::rate_control::compute_qdelta(q_val, q_val * 0.25, 8)
        } else {
            crate::rate_control::compute_qdelta(
                q_val,
                q_val
                    * DELTA_RATE_NEW[hierarchical_levels as usize]
                        [tpl_temporal_layer_index as usize],
                8,
            )
        };
        qindex += delta_qindex;
    }
    qindex
}

/// C `tpl_mc_flow_dispenser`: derive this picture's TPL `qIndex`, iterate
/// all SBs (full 64x64 SBs run at `tpl_ctrls.dispenser_search_level`, edge
/// SBs at level 0), then pad the completed recon picture.
///
/// `ctx` is a fn(&sb_index) -> &TplSbCtx adapter the caller uses to bind the
/// per-SB `b64_geom` origin; `sb_geom` gives each SB's (origin, is_full_64)
/// — the port keeps the geometry outside the ctx so the same ctx serves
/// every SB.
#[allow(clippy::too_many_arguments)]
pub fn tpl_mc_flow_dispenser<'a>(
    qp: u8,
    extended_crf_qindex_offset: i32,
    sb_geom: &[(u32, u32, bool)], // (org_x, org_y, is_full_64x64)
    make_ctx: impl Fn(usize) -> TplSbCtx<'a>,
    recon: &mut TplPicMut<'_>,
    rec_refs: &[TplPic<'a>],
    ref_pics: &[TplPic<'a>],
    me_results: impl Fn(usize) -> TplMeResults<'a>,
    tpl_src_stats_buffer: &mut [TplSrcStats],
    tpl_stats_grid: &mut [TplStats],
) -> i32 {
    // The ctx for sb0 carries the frame-level fields; qIndex derivation is
    // per-PICTURE in C, so read them once here.
    let ctx0 = make_ctx(0);
    let qindex = tpl_qindex(
        qp,
        extended_crf_qindex_offset,
        ctx0.tpl_ctrls.enable_tpl_qps != 0,
        ctx0.tpl_data.tpl_slice_type,
        ctx0.hierarchical_levels,
        ctx0.tpl_data.tpl_temporal_layer_index,
    );
    let rc_update_type = match ctx0.update_type {
        crate::port_picstruct::FrameUpdateType::Kf => {
            crate::port_rc_process::FrameUpdateType::KfUpdate
        }
        crate::port_picstruct::FrameUpdateType::Lf => {
            crate::port_rc_process::FrameUpdateType::LfUpdate
        }
        crate::port_picstruct::FrameUpdateType::Gf => {
            crate::port_rc_process::FrameUpdateType::GfUpdate
        }
        crate::port_picstruct::FrameUpdateType::Arf => {
            crate::port_rc_process::FrameUpdateType::ArfUpdate
        }
        crate::port_picstruct::FrameUpdateType::Overlay => {
            crate::port_rc_process::FrameUpdateType::OverlayUpdate
        }
        crate::port_picstruct::FrameUpdateType::IntnlOverlay => {
            crate::port_rc_process::FrameUpdateType::IntnlOverlayUpdate
        }
        crate::port_picstruct::FrameUpdateType::IntnlArf => {
            crate::port_rc_process::FrameUpdateType::IntnlArfUpdate
        }
    };
    let base_rdmult =
        crate::port_rc_process::compute_rd_mult_based_on_qindex(8, rc_update_type, qindex)
            / TPL_RDMULT_SCALING_FACTOR;

    for (sb_index, &(org_x, org_y, is_full)) in sb_geom.iter().enumerate() {
        let mut ctx = make_ctx(sb_index);
        ctx.sb_origin = (org_x, org_y);
        // C re-derives the quant table per SB from the same qIndex — the
        // caller pre-binds it in ctx.quant.
        let level = if is_full {
            ctx.tpl_ctrls.dispenser_search_level as usize
        } else {
            0
        };
        tpl_mc_flow_dispenser_sb_generic(
            &ctx,
            level,
            recon,
            rec_refs,
            ref_pics,
            &me_results(sb_index),
            tpl_src_stats_buffer,
            tpl_stats_grid,
        );
    }

    // padding current recon picture
    crate::port_preanalysis::generate_padding(
        recon.y,
        recon.origin,
        recon.y_stride,
        recon.width as usize,
        recon.height as usize,
        recon.border,
        recon.border,
    );
    base_rdmult
}

// =============================================================================
// tpl_mc_flow_synthesizer + generate_r0beta (src_ops_process.c:1409-1690)
// =============================================================================

/// Per-picture view the synthesizer mutates — C's
/// `pcs_array[i]->{picture_number, aligned_width, av1_cm, pa_me_data->tpl_stats}`.
pub struct TplSynthPic<'a> {
    /// C `pcs->picture_number` — matched against `TplStats::ref_frame_poc`.
    pub picture_number: u64,
    /// C `pcs->aligned_width` (superres-scaled width, like everywhere in TPL).
    pub aligned_width: u32,
    /// C `av1_cm->mi_rows` / `mi_cols` — bounds for the dependency walk.
    pub mi_rows: i32,
    pub mi_cols: i32,
    /// C `pa_me_data->tpl_stats` — flat grid at `(aligned_width+15)/16 << (2-shift)`
    /// stride.
    pub stats: &'a mut [TplStats],
}

/// C `round_floor` (src_ops_process.c:1441) — floor division that rounds
/// negatives DOWN (away from zero toward -inf), unlike `>>`.
pub(super) fn round_floor(ref_pos: i32, bsize_pix: i32) -> i32 {
    if ref_pos < 0 {
        -(1 + (-ref_pos - 1) / bsize_pix)
    } else {
        ref_pos / bsize_pix
    }
}

/// C `get_overlap_area` (src_ops_process.c:1410) — overlap of the `block`-th
/// quadrant of the grid cell at (`grid_pos_*`) with the predicted region at
/// (`ref_pos_*`), `bw`x`bh` pixels each.
pub(super) fn get_overlap_area(
    grid_pos_row: i32,
    grid_pos_col: i32,
    ref_pos_row: i32,
    ref_pos_col: i32,
    block: i32,
    bw: i32,
    bh: i32,
) -> i32 {
    let (width, height) = match block {
        0 => (
            grid_pos_col + bw - ref_pos_col,
            grid_pos_row + bh - ref_pos_row,
        ),
        1 => (
            ref_pos_col + bw - grid_pos_col,
            grid_pos_row + bh - ref_pos_row,
        ),
        2 => (
            grid_pos_col + bw - ref_pos_col,
            ref_pos_row + bh - grid_pos_row,
        ),
        _ => (
            ref_pos_col + bw - grid_pos_col,
            ref_pos_row + bh - grid_pos_row,
        ),
    };
    width * height
}

/// C `delta_rate_cost` (src_ops_process.c:1452) — dependency-rate cost in
/// `TPL_DEP_COST_SCALE_LOG2 + AV1_PROB_COST_SHIFT` fixed point.
pub(super) fn delta_rate_cost(
    delta_rate: i64,
    recrf_dist: i64,
    srcrf_dist: i64,
    pix_num: i64,
) -> i64 {
    let beta = srcrf_dist as f64 / recrf_dist as f64;
    if srcrf_dist <= 128 {
        return delta_rate;
    }
    let dr =
        (delta_rate >> (TPL_DEP_COST_SCALE_LOG2 + AV1_PROB_COST_SHIFT)) as f64 / pix_num as f64;
    let log_den = beta.ln() / 2.0f64.ln() + 2.0 * dr;
    let rate_cost = if log_den > 10.0f64.ln() / 2.0f64.ln() {
        let rc = ((1.0f64 / beta).ln() * pix_num as f64) / 2.0f64.ln() / 2.0;
        (rc as i64) << (TPL_DEP_COST_SCALE_LOG2 + AV1_PROB_COST_SHIFT)
    } else {
        let num = 2.0f64.powf(log_den);
        let den = num * beta + (1.0 - beta) * beta;
        let rc = (pix_num as f64 * (num / den).ln()) / 2.0f64.ln() / 2.0;
        (rc as i64) << (TPL_DEP_COST_SCALE_LOG2 + AV1_PROB_COST_SHIFT)
    };
    rate_cost
}

/// C `tpl_model_update_b` (src_ops_process.c:1484) — propagate one
/// synth-block's `(cur_dep + mc_dep)` into the 4 quadrants of the reference
/// picture's stats grid it overlaps.
pub(super) fn tpl_model_update_b(
    ref_pic: &mut TplSynthPic<'_>,
    synth_blk_size: u8,
    compute_rate: bool,
    tpl_stats: &TplStats,
    mi_row: i32,
    mi_col: i32,
    bsize: BlockSize,
) {
    // C `get_fullmv_from_mv` — subpel to fullpel, arithmetic shift.
    let ref_pos_row = mi_row * 4 + i32::from(tpl_stats.mv.y >> 3);
    let ref_pos_col = mi_col * 4 + i32::from(tpl_stats.mv.x >> 3);

    let mi_height = svtav1_dsp::port_obmc_pred::MI_SIZE_HIGH[bsize as usize] as i32;
    let mi_width = svtav1_dsp::port_obmc_pred::MI_SIZE_WIDE[bsize as usize] as i32;
    let (bw, bh) = (mi_width * 4, mi_height * 4);
    let pix_num = (bw * bh) as i64;
    let shift: i32 = if synth_blk_size == 8 {
        1
    } else if synth_blk_size == 16 {
        2
    } else {
        3
    };
    let mi_cols_sr = ((ref_pic.aligned_width as i32 + 15) / 16) << 2;

    let grid_pos_row_base = round_floor(ref_pos_row, bh) * bh;
    let grid_pos_col_base = round_floor(ref_pos_col, bw) * bw;

    let cur_dep_dist = tpl_stats.recrf_dist - tpl_stats.srcrf_dist;
    // C divides by `recrf_dist` unconditionally — stats are floored at 1 by
    // `result_model_store`, so it is never zero on this path.
    let mc_dep_dist = tpl_stats.mc_dep_dist * (tpl_stats.recrf_dist - tpl_stats.srcrf_dist)
        / tpl_stats.recrf_dist;
    let delta_rate = tpl_stats.recrf_rate - tpl_stats.srcrf_rate;
    let mc_dep_rate = if compute_rate {
        delta_rate_cost(
            tpl_stats.mc_dep_rate,
            tpl_stats.recrf_dist,
            tpl_stats.srcrf_dist,
            pix_num,
        )
    } else {
        0
    };

    for block in 0..4 {
        let grid_pos_row = grid_pos_row_base + bh * (block >> 1);
        let grid_pos_col = grid_pos_col_base + bw * (block & 0x01);
        if grid_pos_row >= 0
            && grid_pos_row < ref_pic.mi_rows * 4
            && grid_pos_col >= 0
            && grid_pos_col < ref_pic.mi_cols * 4
        {
            let overlap_area = get_overlap_area(
                grid_pos_row,
                grid_pos_col,
                ref_pos_row,
                ref_pos_col,
                block,
                bw,
                bh,
            );
            let ref_mi_row = round_floor(grid_pos_row, bh) * mi_height;
            let ref_mi_col = round_floor(grid_pos_col, bw) * mi_width;
            let step = 1i32 << shift;
            let mut idy = 0;
            while idy < mi_height {
                let mut idx = 0;
                while idx < mi_width {
                    let cell = (((ref_mi_row + idy) >> shift) * (mi_cols_sr >> shift)
                        + ((ref_mi_col + idx) >> shift)) as usize;
                    let st = &mut ref_pic.stats[cell];
                    st.mc_dep_dist +=
                        ((cur_dep_dist + mc_dep_dist) * overlap_area as i64) / pix_num;
                    st.mc_dep_rate += ((delta_rate + mc_dep_rate) * overlap_area as i64) / pix_num;
                    idx += step;
                }
                idy += step;
            }
        }
    }
}

/// C `tpl_model_update` (src_ops_process.c:1543) — walk one outer block's
/// synth cells, find each cell's reference picture in the window by
/// `ref_frame_poc`, and propagate. `i` is deliberately NOT reset between
/// cells — C's forward-only scan.
pub(super) fn tpl_model_update(
    pics: &mut [TplSynthPic<'_>],
    frame_idx: usize,
    mi_row: i32,
    mi_col: i32,
    bsize: BlockSize,
    frames_in_sw: usize,
    synth_blk_size: u8,
    compute_rate: bool,
) {
    let mi_height = svtav1_dsp::port_obmc_pred::MI_SIZE_HIGH[bsize as usize] as i32;
    let mi_width = svtav1_dsp::port_obmc_pred::MI_SIZE_WIDE[bsize as usize] as i32;
    let block_size = if synth_blk_size == 8 {
        BlockSize::Block8x8
    } else if synth_blk_size == 16 {
        BlockSize::Block16x16
    } else {
        BlockSize::Block32x32
    };
    let shift: i32 = if synth_blk_size == 8 {
        1
    } else if synth_blk_size == 16 {
        2
    } else {
        3
    };
    let step = 1i32 << shift;
    let mi_cols_sr = ((pics[frame_idx].aligned_width as i32 + 15) / 16) << 2;

    // Copy the cells out first so the per-ref mutable borrows don't alias the
    // current picture's grid.
    let mut cells: alloc::vec::Vec<TplStats> = alloc::vec::Vec::new();
    let mut idy = 0;
    while idy < mi_height {
        let mut idx = 0;
        while idx < mi_width {
            let cell = ((((mi_row + idy) >> shift) * (mi_cols_sr >> shift))
                + ((mi_col + idx) >> shift)) as usize;
            cells.push(pics[frame_idx].stats[cell]);
            idx += step;
        }
        idy += step;
    }

    let mut i = 0usize;
    let mut cell_iter = cells.iter();
    idy = 0;
    while idy < mi_height {
        let mut idx = 0;
        while idx < mi_width {
            let tpl_stats = *cell_iter.next().unwrap();
            while i < frames_in_sw && pics[i].picture_number != tpl_stats.ref_frame_poc {
                i += 1;
            }
            if i < frames_in_sw {
                tpl_model_update_b(
                    &mut pics[i],
                    synth_blk_size,
                    compute_rate,
                    &tpl_stats,
                    mi_row + idy,
                    mi_col + idx,
                    block_size,
                );
            }
            idx += step;
        }
        idy += step;
    }
}

/// C `tpl_mc_flow_synthesizer` (src_ops_process.c:1575) — sweep the whole
/// frame at the OUTER block size (32x32 for synth 32, else 16x16).
pub fn tpl_mc_flow_synthesizer(
    pics: &mut [TplSynthPic<'_>],
    frame_idx: usize,
    frames_in_sw: usize,
    synth_blk_size: u8,
    compute_rate: bool,
) {
    let bsize = if synth_blk_size == 32 {
        BlockSize::Block32x32
    } else {
        BlockSize::Block16x16
    };
    let mi_height = svtav1_dsp::port_obmc_pred::MI_SIZE_HIGH[bsize as usize] as i32;
    let mi_width = svtav1_dsp::port_obmc_pred::MI_SIZE_WIDE[bsize as usize] as i32;
    let mi_rows = pics[frame_idx].mi_rows;
    let mi_cols = pics[frame_idx].mi_cols;
    let mut mi_row = 0;
    while mi_row < mi_rows {
        let mut mi_col = 0;
        while mi_col < mi_cols {
            tpl_model_update(
                pics,
                frame_idx,
                mi_row,
                mi_col,
                bsize,
                frames_in_sw,
                synth_blk_size,
                compute_rate,
            );
            mi_col += mi_width;
        }
        mi_row += mi_height;
    }
}

/// C `coded_to_superres_mi` (resize.h:71).
pub(super) fn coded_to_superres_mi(mi_col: i32, denom: u8) -> i32 {
    (mi_col * denom as i32 + 4) / 8
}

/// Output of C `svt_aom_generate_r0beta` (src_ops_process.c:1592-1687).
pub struct R0Beta {
    /// C `pcs->r0` — valid only when `tpl_is_valid`.
    pub r0: f64,
    /// C `pcs->tpl_is_valid`.
    pub tpl_is_valid: bool,
}

/// C `svt_aom_generate_r0beta` — frame-level `r0`, then
/// `generate_lambda_scaling_factor`, then per-SB `tpl_beta`.
///
/// `tpl_stats` is the synth-grid for THIS picture; `sb_geom` is each SB's
/// `(org_x, org_y)` in pixels; `tpl_beta` is the per-SB output (indexed
/// `sb_y * picture_sb_width + sb_x`).
#[allow(clippy::too_many_arguments)]
pub fn generate_r0beta(
    synth_blk_size: u8,
    sb_size: u32,
    aligned_width: u32,
    aligned_height: u32,
    enhanced_unscaled_width: u32,
    enhanced_unscaled_height: u32,
    superres_denom: u8,
    cm_mi_rows: i32,
    base_rdmult: i64,
    tpl_stats: &[TplStats],
    sb_geom: &[(u32, u32)],
    tpl_rdmult_scaling_factors: &mut [f64],
    tpl_beta: &mut [f64],
) -> R0Beta {
    let shift: i32 = if synth_blk_size == 8 {
        1
    } else if synth_blk_size == 16 {
        2
    } else {
        3
    };
    let step = 1i32 << shift;
    let col_step_sr = coded_to_superres_mi(step, superres_denom);
    // Super-res UPSCALED size.
    let mi_cols_sr = ((enhanced_unscaled_width as i32 + 15) / 16) << 2;
    let mi_rows = ((enhanced_unscaled_height as i32 + 15) / 16) << 2;

    let mut recrf_dist_base_sum: i64 = 0;
    let mut mc_dep_delta_base_sum: i64 = 0;
    let mut count: i64 = 0;
    let mut max_dist: i64 = 0;

    let mut row = 0;
    while row < cm_mi_rows {
        let mut col = 0;
        while col < mi_cols_sr {
            let st = &tpl_stats[((row >> shift) * (mi_cols_sr >> shift) + (col >> shift)) as usize];
            let mc_dep_delta = rdcost_tpl(base_rdmult, st.mc_dep_rate, st.mc_dep_dist);
            recrf_dist_base_sum += st.recrf_dist;
            mc_dep_delta_base_sum += mc_dep_delta;
            count += 1;
            if mc_dep_delta > max_dist {
                max_dist = mc_dep_delta;
            }
            col += col_step_sr;
        }
        row += step;
    }

    let mc_dep_cost_base = (recrf_dist_base_sum << RDDIV_BITS) + mc_dep_delta_base_sum;
    #[cfg(feature = "std")]
    if std::env::var_os("SVTAV1_R0DBG").is_some() {
        std::eprintln!(
            "R0DBG recrf={} mcdep={} count={} rdmult={}",
            recrf_dist_base_sum,
            mc_dep_delta_base_sum,
            count,
            base_rdmult,
        );
    }
    let (r0, tpl_is_valid) = if mc_dep_cost_base != 0 {
        let mut r0 = ((recrf_dist_base_sum << RDDIV_BITS) as f64) / (mc_dep_cost_base as f64);
        if max_dist > (mc_dep_delta_base_sum / count) * 100
            && max_dist > (mc_dep_delta_base_sum * 9 / 10)
        {
            r0 = 1.0;
        }
        (r0, true)
    } else {
        (0.0, false)
    };

    generate_lambda_scaling_factor(
        synth_blk_size,
        cm_mi_rows,
        enhanced_unscaled_width,
        base_rdmult,
        mc_dep_cost_base,
        r0,
        tpl_stats,
        tpl_rdmult_scaling_factors,
    );

    // Per-SB `tpl_beta` — C src_ops_process.c:1644-1684. Superres scale-down
    // uses the scaled width (`aligned_width`), as in C.
    let sb_mi_sz = (sb_size >> 2) as i32;
    let picture_sb_width = aligned_width.div_ceil(sb_size);
    let picture_sb_height = aligned_height.div_ceil(sb_size);
    let mi_high = sb_mi_sz;
    let mi_wide = sb_mi_sz;
    for sb_y in 0..picture_sb_height {
        for sb_x in 0..picture_sb_width {
            let (org_x, org_y) = sb_geom[(sb_y * picture_sb_width + sb_x) as usize];
            let mi_row = (org_y >> 2) as i32;
            let mi_col = (org_x >> 2) as i32;
            let mut recrf_dist_sum: i64 = 0;
            let mut mc_dep_delta_sum: i64 = 0;
            let mi_col_sr = coded_to_superres_mi(mi_col, superres_denom);
            let mi_col_end_sr = coded_to_superres_mi(mi_col + mi_wide, superres_denom);
            let row_step = step;

            let mut row = mi_row;
            while row < mi_row + mi_high {
                let mut col = mi_col_sr;
                while col < mi_col_end_sr {
                    if row < mi_rows && col < mi_cols_sr {
                        let index =
                            ((row >> shift) * (mi_cols_sr >> shift) + (col >> shift)) as usize;
                        let st = &tpl_stats[index];
                        let mc_dep_delta = rdcost_tpl(base_rdmult, st.mc_dep_rate, st.mc_dep_dist);
                        recrf_dist_sum += st.recrf_dist;
                        mc_dep_delta_sum += mc_dep_delta;
                    }
                    col += col_step_sr;
                }
                row += row_step;
            }
            let mut beta = 1.0f64;
            if recrf_dist_sum > 0 {
                let rk = ((recrf_dist_sum << RDDIV_BITS) as f64)
                    / (((recrf_dist_sum << RDDIV_BITS) + mc_dep_delta_sum) as f64);
                beta = r0 / rk;
            }
            tpl_beta[(sb_y * picture_sb_width + sb_x) as usize] = beta;
        }
    }

    R0Beta { r0, tpl_is_valid }
}

// =============================================================================
// tpl_mc_flow (src_ops_process.c:1793-1975)
// =============================================================================

/// Per-window-frame state the flow owns — the `tpl_group` member's
/// `pa_me_data` plus the `tpl_valid_pic`/`tpl_src_data_ready` flags.
///
/// The recon plane is deliberately NOT here: it lives in the parallel
/// `recons` array so the driver can hand the dispenser a mutable view of the
/// current frame's recon AND immutable views of the completed in-window
/// references (`enc_ctx->mc_flow_rec_picture_buffer`) at the same time.
pub struct TplWindowFrame<'a> {
    /// C `pcs->picture_number` — matched by the synthesizer's poc search and
    /// recorded into `poc_map_idx`.
    pub picture_number: u64,
    /// C `pcs->tpl_valid_pic[frame_idx]` — set by `store_extended_group`.
    pub valid: bool,
    /// C `pcs->tpl_data` — the per-frame ref mapping the dispenser reads.
    pub tpl_data: TplData,
    /// C `pa_me_data->tpl_stats` — the synth grid (zeroed each pass).
    pub stats: &'a mut [TplStats],
    /// C `pa_me_data->tpl_src_stats_buffer` — the 16-px src-stats grid.
    pub src_stats: &'a mut [TplSrcStats],
    /// C `pcs->aligned_width` (superres-scaled).
    pub aligned_width: u32,
    /// C `av1_cm->mi_rows` / `mi_cols`.
    pub mi_rows: i32,
    /// See [`TplWindowFrame::mi_rows`].
    pub mi_cols: i32,
    /// C `pcs->tpl_src_data_ready` — out flag set when `tpl_lad_mg > 0` and
    /// the frame was dispensed.
    pub tpl_src_data_ready: bool,
}

/// Arguments handed to the caller's per-frame dispenser closure — everything
/// C's `tpl_mc_flow` loop body set up before calling
/// `tpl_mc_flow_dispenser`.
pub struct TplDispenseArgs<'a, 'b> {
    /// `frame_idx`.
    pub frame_idx: usize,
    /// The frame being dispensed.
    pub frame: &'a mut TplWindowFrame<'b>,
    /// Its recon buffer (`mc_flow_rec_picture_buffer[frame_idx]`).
    pub recon: &'a mut TplPicMut<'b>,
    /// `mc_flow_rec_picture_buffer` for the WHOLE window, indexed by
    /// `frame_idx` exactly like `tpl_ref_ds[..].pic_index` — slot
    /// `frame_idx` itself is a placeholder that is never read (a frame can
    /// never reference itself).
    pub ref_pics: &'a [TplPic<'a>],
    /// `enc_ctx->poc_map_idx` — window index to POC, filled through
    /// `frame_idx` inclusive (C writes each entry just before dispensing).
    pub poc_map_idx: &'a [u64],
}

/// C `tpl_mc_flow` (src_ops_process.c:1793-1975), minus the resource-pool
/// plumbing that has no safe-Rust analogue:
///
/// * `svt_get_empty_object`/`tpl_ref_list`/`svt_release_object` manage C's
///   recon-buffer pool; the port's caller owns `recons` directly.
/// * `init_tpl_segments` + `assign_tpl_segments` are the multi-threaded SB
///   scheduler; the port's dispenser walks all SBs serially.
/// * `svt_wait_cond_var(&tpl_group[i]->me_ready)` is the PA-ME barrier — the
///   caller runs after ME results exist by construction.
/// * `EB_DELETE(non_tf_input)` / `release_pa_reference_objects` are lifetime
///   management.
///
/// What IS preserved, in C order: `frames_in_sw` clamp, per-frame stats-grid
/// zero, `poc_map_idx` fill, valid-gated dispenser calls, `tpl_src_data_ready`
/// marking, and the reverse-order synthesizer sweep.
///
/// The caller must only invoke this when C would:
/// `tpl_ctrls.enable && !frame_superres_enabled && temporal_layer_index == 0`
/// (`svt_aom_source_based_operations_kernel_iter`) and
/// `tpl_group[0].tpl_data.tpl_temporal_layer_index == 0` (the `tpl_mc_flow`
/// gate). Returns `frames_in_sw`.
pub fn tpl_mc_flow<'a>(
    window: &mut [TplWindowFrame<'a>],
    recons: &mut [TplPicMut<'a>],
    synth_blk_size: u8,
    compute_rate: bool,
    tpl_lad_mg: u8,
    mut dispense: impl FnMut(TplDispenseArgs<'_, '_>) -> i32,
) -> usize {
    let frames_in_sw = MAX_TPL_LA_SW.min(window.len());
    debug_assert_eq!(recons.len(), window.len());

    // Grid geometry — C derives it from `enhanced_pic` dims + synth size.
    // `picture_width_in_mb` is the tpl_stats row stride.
    let picture_width_in_mb = |w: u32| -> usize {
        let mb = w.div_ceil(16) as usize;
        match synth_blk_size {
            8 => mb << 1,
            32 => w.div_ceil(32) as usize,
            _ => mb,
        }
    };

    let mut poc_map_idx: alloc::vec::Vec<u64> = alloc::vec::Vec::with_capacity(frames_in_sw);

    // TPL main frame loop — forward so in-window ref recons are complete.
    for frame_idx in 0..frames_in_sw {
        poc_map_idx.push(window[frame_idx].picture_number);

        // Zero this frame's tpl_stats grid — C memsets `picture_height_in_mb
        // * picture_width_in_mb` cells, which is the whole grid.
        let row_len = picture_width_in_mb(window[frame_idx].aligned_width);
        debug_assert!(window[frame_idx].stats.len() >= row_len);
        for st in window[frame_idx].stats.iter_mut() {
            *st = TplStats::default();
        }

        let tpl_on = window[frame_idx].valid;
        if tpl_on {
            // Split `recons` so slot frame_idx is mutable while the rest are
            // shared views — the C code aliases these freely via
            // `mc_flow_rec_picture_buffer`.
            let n_recons = recons.len();
            let (left, right) = recons.split_at_mut(frame_idx);
            let (cur, right) = right.split_at_mut(1);
            let mut ref_pics: alloc::vec::Vec<TplPic<'_>> =
                alloc::vec::Vec::with_capacity(n_recons);
            for r in left.iter() {
                ref_pics.push(TplPic {
                    y: r.y,
                    y_stride: r.y_stride,
                    width: r.width,
                    height: r.height,
                    max_width: r.width,
                    max_height: r.height,
                    origin: r.origin,
                });
            }
            // Placeholder for the self slot — `pic_index == frame_idx` can
            // never be produced by `tpl_prep_info`.
            ref_pics.push(TplPic {
                y: &[],
                y_stride: 0,
                width: 0,
                height: 0,
                max_width: 0,
                max_height: 0,
                origin: 0,
            });
            for r in right.iter() {
                ref_pics.push(TplPic {
                    y: r.y,
                    y_stride: r.y_stride,
                    width: r.width,
                    height: r.height,
                    max_width: r.width,
                    max_height: r.height,
                    origin: r.origin,
                });
            }
            let (_, right) = window.split_at_mut(frame_idx);
            let frame = &mut right[0];
            if crate::dbgenv::blkdbg() {
                eprintln!(
                    "RUN poc={} tl={}",
                    frame.picture_number, frame.tpl_data.tpl_temporal_layer_index
                );
            }
            dispense(TplDispenseArgs {
                frame_idx,
                frame,
                recon: &mut cur[0],
                ref_pics: &ref_pics,
                poc_map_idx: &poc_map_idx,
            });
        }

        if tpl_lad_mg > 0 && tpl_on {
            window[frame_idx].tpl_src_data_ready = true;
        }
    }

    // Synthesizer — REVERSE order over valid pics, propagating each frame's
    // dependency deltas onto its references' grids.
    {
        let valid: alloc::vec::Vec<bool> = window[..frames_in_sw].iter().map(|w| w.valid).collect();
        let mut synth: alloc::vec::Vec<TplSynthPic<'_>> = window[..frames_in_sw]
            .iter_mut()
            .map(|w| TplSynthPic {
                picture_number: w.picture_number,
                aligned_width: w.aligned_width,
                mi_rows: w.mi_rows,
                mi_cols: w.mi_cols,
                stats: &mut *w.stats,
            })
            .collect();
        for frame_idx in (0..frames_in_sw).rev() {
            if valid[frame_idx] {
                tpl_mc_flow_synthesizer(
                    &mut synth,
                    frame_idx,
                    frames_in_sw,
                    synth_blk_size,
                    compute_rate,
                );
            }
        }
    }

    frames_in_sw
}

/// The TPL results a picture carries into its own encode — C's
/// `pcs->tpl_ctrls`, the four `r0_*` flags, `pcs->r0`,
/// `pcs->tpl_is_valid`, `pa_me_data->tpl_beta`/`tpl_rdmult_scaling_factors`,
/// `tpl_group_size`/`used_tpl_frame_num`, plus the member's open-loop ME
/// results (`pa_me_data->me_results`) which the TPL dispenser computes and
/// the picture's own encode reuses verbatim.
///
/// Produced per released mini-GOP by `EncodePipeline::run_tpl_stage` — the
/// pipeline wiring that replaces `initial_rc_process.c`'s
/// `store_extended_group`/`set_tpl_group`/`set_tpl_params`,
/// `src_ops_process.c`'s `tpl_prep_info`/`tpl_mc_flow`, and
/// `rc_init_frame_stats`' `svt_aom_generate_r0beta`, at `tpl_lad_mg == 0`
/// (the only lookahead shape the one-mini-GOP RA buffer can express).
pub struct FrameTplIn {
    /// `pcs->tpl_ctrls` — this picture's own copy, from `set_tpl_group` +
    /// `set_tpl_params` on its `PicParams`.
    pub tpl_ctrls: crate::port_picstruct::TplControls,
    /// `pcs->synth_blk_size` for the GROUP BASE that produced this
    /// picture's stats — the synthesizer block size `tpl_mc_flow` used
    /// (the base's `tpl_ctrls.synth_blk_size`, not necessarily this
    /// picture's own).
    pub synth_blk_size: u8,
    /// `pcs->r0_gen`/`r0_qps`/`r0_delta_qp_md`/`r0_delta_qp_quant`
    /// (`initial_rc_process.c:733-762`).
    pub flags: crate::rate_control::R0Flags,
    /// `pcs->r0` — the group's geometric-mean base factor; `0` when
    /// `!tpl_is_valid`.
    pub r0: f64,
    /// `pcs->tpl_is_valid` — `svt_aom_generate_r0beta`'s validity verdict.
    pub tpl_is_valid: bool,
    /// `pcs->tpl_group_size` — nonzero only on the group base picture.
    pub tpl_group_size: u32,
    /// `pcs->used_tpl_frame_num` — nonzero only on the group base.
    pub used_tpl_frame_num: u32,
    /// `pa_me_data->tpl_beta` — per-superblock beta (indexed by SB index).
    pub tpl_beta: alloc::vec::Vec<f64>,
    /// `pa_me_data->tpl_rdmult_scaling_factors` — the PRE-
    /// `sb_setup_lambda` grid; the SB loop folds it through
    /// [`crate::sb_qindex::sb_setup_lambda`] before use.
    pub tpl_rdmult_scaling_factors: alloc::vec::Vec<f64>,
    /// The member's open-loop ME results — the same `FrameMe` the
    /// picture's own encode computes against the same reference planes,
    /// reused so PA ME runs once per picture (`pa_me` equivalent).
    /// `None` when the member ran no open-loop ME (I slice, or a
    /// reference pyramid the search needs was missing).
    pub frame_me: Option<crate::inter_me_arm::FrameMe>,
}
