//! Per-txb luma prediction at tx depth > 0 (u8 + the bd10 twin).
//!
//! C `av1_intra_luma_prediction` (product_coding_loop.c:4072): predict one txb
//! against neighbour arrays whose interior is this depth's partial recon and
//! whose exterior is the frame recon.
//!
//! Split out of `leaf_funnel/mod.rs` on 2026-08-25. PURE CODE MOVEMENT: every
//! item keeps its name, order and effective visibility (file-private became
//! `pub(super)`, the same scope).

use super::*;

/// Per-txb luma prediction at depth > 0: reads the frame recon for
/// out-of-block neighbors and this depth's partial recon inside the block.
/// Mirrors C `av1_intra_luma_prediction` (product_coding_loop.c:4072):
/// `svt_av1_predict_intra_block` at (row_off, col_off) over the
/// tx-search neighbor arrays (block interior = this depth's recon so
/// far, exterior = frame recon).
#[allow(clippy::too_many_arguments)]
pub(super) fn predict_unit_overlay(
    y_recon: &[u8],
    y_stride: usize,
    blk_x: usize,
    blk_y: usize,
    dep_recon: &[u8],
    blk_w: usize,
    blk_h: usize,
    tx_x: usize,
    tx_y: usize,
    txw: usize,
    txh: usize,
    mode: u8,
    delta: i8,
    fi: u8,
    geom: &UnitGeom,
    edge_filter: bool,
    filt_type: i32,
    dst: &mut [u8],
) {
    if matches!(mode, 3..=8) || (matches!(mode, 1 | 2) && delta != 0) {
        let p_angle = crate::intra_edge::MODE_TO_ANGLE_MAP[mode as usize] + delta as i32 * 3;
        debug_assert!(fi == FI_NONE);
        let g = crate::intra_edge::DrGeom {
            px: blk_x + tx_x,
            py: blk_y + tx_y,
            txw,
            txh,
            mi_row: geom.mi_row,
            mi_col: geom.mi_col,
            bw_px: geom.bw_px,
            bh_px: geom.bh_px,
            row_off: tx_y / 4,
            col_off: tx_x / 4,
            ss: 0,
            frame_w: geom.frame_w,
            frame_h: geom.frame_h,
            sb_mi_size: geom.sb_mi_size,
            tile: geom.tile,
        };
        crate::intra_edge::dr_predict(
            |x, y| {
                if x >= blk_x && x < blk_x + blk_w && y >= blk_y && y < blk_y + blk_h {
                    dep_recon[(y - blk_y) * blk_w + (x - blk_x)]
                } else {
                    y_recon[y * y_stride + x]
                }
            },
            &g,
            p_angle,
            edge_filter,
            filt_type,
            svtav1_types::partition::PartitionType::None,
            dst,
        );
        return;
    }
    // Build a small canvas: (txh + 1) left col + (txw + 1) top row around
    // the txb, sourcing in-block pixels from dep_recon and out-of-block
    // pixels from the frame recon, then run the standard edge extraction
    // on it. Canvas layout: (txh+1) rows x (txw+1) cols, txb at (1, 1).
    let cw_dim = txw + 1;
    let ch_dim = txh + 1;
    let abs_tx_x = blk_x + tx_x;
    let abs_tx_y = blk_y + tx_y;
    let mut canvas = vec![0u8; cw_dim * ch_dim];
    let sample = |x: isize, y: isize| -> u8 {
        // (x, y) absolute plane coords.
        if x < 0 || y < 0 {
            return 128; // never read: extract handles borders
        }
        let (x, y) = (x as usize, y as usize);
        let in_blk_x = x >= blk_x && x < blk_x + blk_w;
        let in_blk_y = y >= blk_y && y < blk_y + blk_h;
        if in_blk_x && in_blk_y {
            dep_recon[(y - blk_y) * blk_w + (x - blk_x)]
        } else {
            let row_len = y_stride;
            let idx = y * y_stride + x.min(row_len - 1);
            if idx < y_recon.len() {
                y_recon[idx]
            } else {
                y_recon[y_recon.len() - row_len + x.min(row_len - 1)]
            }
        }
    };
    // C `n_top_px` / `n_left_px` (`build_intra_predictors` via
    // `svt_av1_predict_intra_block`'s `xr`/`yd`, enc_intra_prediction.c:489-492
    // + :540-543): reference samples past the ALIGNED frame extent are not
    // read — the last real sample is replicated. Spec 7.11.2 writes the same
    // rule as a coordinate clamp (`Min(maxY, ...)`), which is what the two
    // `.min()`s below are: clamping the canvas source coordinate reproduces
    // C's replication exactly, because the last real sample IS the one at the
    // extent. Identical to the unclamped read whenever the txb is inside the
    // extent, i.e. always on a 64-aligned frame.
    let max_x = (geom.frame_w >> geom.ss).saturating_sub(1) as isize;
    let max_y = (geom.frame_h >> geom.ss).saturating_sub(1) as isize;
    // top row (incl. corner) and left col of the canvas
    for cx in 0..cw_dim {
        canvas[cx] = sample(
            (abs_tx_x as isize + cx as isize - 1).min(max_x),
            abs_tx_y as isize - 1,
        );
    }
    for cy in 1..ch_dim {
        canvas[cy * cw_dim] = sample(
            abs_tx_x as isize - 1,
            (abs_tx_y as isize + cy as isize - 1).min(max_y),
        );
    }
    // Predict at canvas coords (1, 1): availability mirrors the absolute
    // position (frame edges) — and, task #96, the TILE edges, which C
    // gates on identically (`mi_row > tile->mi_row_start`). Both origins
    // are 0 for a single-tile encode. C then selects the DC / edge-fill
    // variants on `n_*_px > 0`, which additionally excludes a txb whose own
    // origin is already past the extent (reachable inside a straddling leaf).
    let has_above = abs_tx_y > geom.tile.top_px(geom.ss) && (abs_tx_x as isize) <= max_x;
    let has_left = abs_tx_x > geom.tile.left_px(geom.ss) && (abs_tx_y as isize) <= max_y;
    let above: Vec<u8> = if has_above {
        canvas[1..cw_dim].to_vec()
    } else {
        vec![if has_left { canvas[cw_dim] } else { 127 }; txw]
    };
    let left: Vec<u8> = if has_left {
        (1..ch_dim).map(|cy| canvas[cy * cw_dim]).collect()
    } else {
        vec![if has_above { canvas[1] } else { 129 }; txh]
    };
    let top_left = if has_above && has_left {
        canvas[0]
    } else if has_above {
        canvas[1]
    } else if has_left {
        canvas[cw_dim]
    } else {
        128
    };
    if fi != FI_NONE {
        let mut above_c = vec![0u8; txw + 1];
        above_c[0] = top_left;
        above_c[1..].copy_from_slice(&above);
        svtav1_dsp::intra_pred::predict_filter_intra(dst, txw, &above_c, &left, txw, txh, fi);
        return;
    }
    match mode {
        0 => svtav1_dsp::intra_pred::predict_dc(
            dst, txw, &above, &left, txw, txh, has_above, has_left,
        ),
        1 => svtav1_dsp::intra_pred::predict_v(dst, txw, &above, txw, txh),
        2 => svtav1_dsp::intra_pred::predict_h(dst, txw, &left, txw, txh),
        9 => svtav1_dsp::intra_pred::predict_smooth(dst, txw, &above, &left, txw, txh),
        10 => svtav1_dsp::intra_pred::predict_smooth_v(dst, txw, &above, &left, txh, txh, txw),
        11 => svtav1_dsp::intra_pred::predict_smooth_h(dst, txw, &above, &left, txw, txh),
        12 => svtav1_dsp::intra_pred::predict_paeth(dst, txw, &above, &left, top_left, txw, txh),
        m => unreachable!("funnel mode {m}"),
    }
}

/// bd10 twin of [`predict_unit_overlay`]: predict one deeper-depth txb from
/// the TRUE 10-bit canvas (frame recon outside the block, this depth's 10-bit
/// recon inside).
///
/// Same geometry, same availability, same canvas splice as the u8 form — only
/// the pixel type and the no-neighbour flat fills change, which follow C's
/// `build_intra_predictors_high` (enc_intra_prediction.c:261-374):
/// `{129, 127, 128}` become `{base+1, base-1, base}` with `base = 128 <<
/// (bd - 8)`. That is the same substitution `dr_predict_hbd` already makes.
#[allow(clippy::too_many_arguments)]
pub(super) fn predict_unit_overlay_hbd(
    y_recon10: &[u16],
    y_stride: usize,
    blk_x: usize,
    blk_y: usize,
    dep_recon10: &[u16],
    blk_w: usize,
    blk_h: usize,
    tx_x: usize,
    tx_y: usize,
    txw: usize,
    txh: usize,
    mode: u8,
    delta: i8,
    fi: u8,
    geom: &UnitGeom,
    edge_filter: bool,
    filt_type: i32,
    dst: &mut [u16],
    bd: u8,
) {
    use svtav1_dsp::hbd as hp;
    let base: u16 = 128u16 << (bd - 8);
    if matches!(mode, 3..=8) || (matches!(mode, 1 | 2) && delta != 0) {
        let p_angle = crate::intra_edge::MODE_TO_ANGLE_MAP[mode as usize] + delta as i32 * 3;
        debug_assert!(fi == FI_NONE);
        let g = crate::intra_edge::DrGeom {
            px: blk_x + tx_x,
            py: blk_y + tx_y,
            txw,
            txh,
            mi_row: geom.mi_row,
            mi_col: geom.mi_col,
            bw_px: geom.bw_px,
            bh_px: geom.bh_px,
            row_off: tx_y / 4,
            col_off: tx_x / 4,
            ss: 0,
            frame_w: geom.frame_w,
            frame_h: geom.frame_h,
            sb_mi_size: geom.sb_mi_size,
            tile: geom.tile,
        };
        crate::intra_edge::dr_predict_hbd(
            |x, y| {
                if x >= blk_x && x < blk_x + blk_w && y >= blk_y && y < blk_y + blk_h {
                    dep_recon10[(y - blk_y) * blk_w + (x - blk_x)]
                } else {
                    y_recon10[y * y_stride + x]
                }
            },
            &g,
            p_angle,
            edge_filter,
            filt_type,
            svtav1_types::partition::PartitionType::None,
            dst,
            bd,
        );
        return;
    }
    let cw_dim = txw + 1;
    let ch_dim = txh + 1;
    let abs_tx_x = blk_x + tx_x;
    let abs_tx_y = blk_y + tx_y;
    let mut canvas = vec![0u16; cw_dim * ch_dim];
    let sample = |x: isize, y: isize| -> u16 {
        if x < 0 || y < 0 {
            return base; // never read: the extraction below handles borders
        }
        let (x, y) = (x as usize, y as usize);
        let in_blk_x = x >= blk_x && x < blk_x + blk_w;
        let in_blk_y = y >= blk_y && y < blk_y + blk_h;
        if in_blk_x && in_blk_y {
            dep_recon10[(y - blk_y) * blk_w + (x - blk_x)]
        } else {
            let row_len = y_stride;
            let idx = y * y_stride + x.min(row_len - 1);
            if idx < y_recon10.len() {
                y_recon10[idx]
            } else {
                y_recon10[y_recon10.len() - row_len + x.min(row_len - 1)]
            }
        }
    };
    // C n_top_px/n_left_px as a coordinate clamp — see the u8 twin.
    let max_x = (geom.frame_w >> geom.ss).saturating_sub(1) as isize;
    let max_y = (geom.frame_h >> geom.ss).saturating_sub(1) as isize;
    for cx in 0..cw_dim {
        canvas[cx] = sample(
            (abs_tx_x as isize + cx as isize - 1).min(max_x),
            abs_tx_y as isize - 1,
        );
    }
    for cy in 1..ch_dim {
        canvas[cy * cw_dim] = sample(
            abs_tx_x as isize - 1,
            (abs_tx_y as isize + cy as isize - 1).min(max_y),
        );
    }
    let has_above = abs_tx_y > geom.tile.top_px(geom.ss) && (abs_tx_x as isize) <= max_x;
    let has_left = abs_tx_x > geom.tile.left_px(geom.ss) && (abs_tx_y as isize) <= max_y;
    let above: Vec<u16> = if has_above {
        canvas[1..cw_dim].to_vec()
    } else {
        vec![if has_left { canvas[cw_dim] } else { base - 1 }; txw]
    };
    let left: Vec<u16> = if has_left {
        (1..ch_dim).map(|cy| canvas[cy * cw_dim]).collect()
    } else {
        vec![if has_above { canvas[1] } else { base + 1 }; txh]
    };
    let top_left = if has_above && has_left {
        canvas[0]
    } else if has_above {
        canvas[1]
    } else if has_left {
        canvas[cw_dim]
    } else {
        base
    };
    if fi != FI_NONE {
        let mut above_c = vec![0u16; txw + 1];
        above_c[0] = top_left;
        above_c[1..].copy_from_slice(&above);
        hp::predict_filter_intra_hbd(dst, txw, &above_c, &left, txw, txh, fi, bd);
        return;
    }
    match mode {
        0 => hp::predict_dc_hbd(dst, txw, &above, &left, txw, txh, has_above, has_left, bd),
        1 => hp::predict_v_hbd(dst, txw, &above, txw, txh),
        2 => hp::predict_h_hbd(dst, txw, &left, txw, txh),
        9 => hp::predict_smooth_hbd(dst, txw, &above, &left, txw, txh),
        10 => hp::predict_smooth_v_hbd(dst, txw, &above, &left, txw, txh),
        11 => hp::predict_smooth_h_hbd(dst, txw, &above, &left, txw, txh),
        12 => hp::predict_paeth_hbd(dst, txw, &above, &left, top_left, txw, txh),
        m => unreachable!("funnel bd10 overlay mode {m}"),
    }
}
