//! Prediction helpers.
//!
//! Split out of `leaf_funnel.rs` on 2026-08-16 (11,247 lines).
//! PURE CODE MOVEMENT: every item keeps its name, order and effective
//! visibility (file-private became `pub(super)`, the same scope).

use super::*;

// ---------------------------------------------------------------------------
// Prediction helpers
// ---------------------------------------------------------------------------

/// Per-unit geometry the directional predictor needs beyond the plane
/// coords: the CODED BLOCK's luma mi position/dims (availability tables),
/// the plane subsampling, and the LUMA frame dims.
#[derive(Clone, Copy)]
pub(crate) struct UnitGeom {
    pub mi_row: usize,
    pub mi_col: usize,
    pub bw_px: usize,
    pub bh_px: usize,
    pub ss: usize,
    pub frame_w: usize,
    pub frame_h: usize,
    /// C `seq_header.sb_mi_size` — 16 (SB64) or 32 (SB128). See
    /// [`FunnelFrame::sb_mi_size`].
    pub sb_mi_size: usize,
    /// Task #96: the current TILE's bounds in LUMA mi units. Every
    /// neighbour-availability test in the MD prediction path is
    /// tile-scoped in C; see [`crate::intra_edge::TileMi`].
    /// `TileMi::whole_frame(..)` is the single-tile default and reproduces
    /// the previous behaviour exactly.
    pub tile: crate::intra_edge::TileMi,
}

/// Predict one intra mode (any of the 13 C modes + angle delta, or
/// FILTER_DC) for a whole prediction unit at absolute plane coords,
/// reading the live recon plane with the C edge-fill rules
/// (`svt_av1_intra_prediction` -> `build_intra_predictors`).
///
/// Non-directional modes and V/H at delta 0 (p_angle exactly 90/180 —
/// the decoder's edge filter skips them) use the extract_neighbors fills;
/// all other directional predictions run `intra_edge::dr_predict`, which
/// applies the SH-gated corner/edge filters + upsampling
/// (`edge_filter`, `filt_type` = C `get_filt_type`).
#[allow(clippy::too_many_arguments)]
pub(super) fn predict_unit(
    recon: &[u8],
    stride: usize,
    abs_x: usize,
    abs_y: usize,
    w: usize,
    h: usize,
    mode: u8,
    delta: i8,
    fi_mode: u8,
    geom: &UnitGeom,
    edge_filter: bool,
    filt_type: i32,
    dst: &mut [u8],
) {
    use svtav1_dsp::intra_pred as ip;
    if matches!(mode, 3..=8) || (matches!(mode, 1 | 2) && delta != 0) {
        let p_angle = crate::intra_edge::MODE_TO_ANGLE_MAP[mode as usize] + delta as i32 * 3;
        debug_assert!(fi_mode == FI_NONE);
        let g = crate::intra_edge::DrGeom {
            px: abs_x,
            py: abs_y,
            txw: w,
            txh: h,
            mi_row: geom.mi_row,
            mi_col: geom.mi_col,
            bw_px: geom.bw_px,
            bh_px: geom.bh_px,
            row_off: 0,
            col_off: 0,
            ss: geom.ss,
            frame_w: geom.frame_w,
            frame_h: geom.frame_h,
            sb_mi_size: geom.sb_mi_size,
            tile: geom.tile,
        };
        crate::intra_edge::dr_predict(
            |x, y| recon[y * stride + x],
            &g,
            p_angle,
            edge_filter,
            filt_type,
            svtav1_types::partition::PartitionType::None,
            dst,
        );
        return;
    }
    // Task #96: tile-scoped neighbour availability. `geom.tile` is the
    // whole frame for a single-tile encode, where `tile_top/left` are 0
    // and this is bit-for-bit `extract_neighbors`.
    let (above, left, top_left, has_above, has_left) = crate::partition::extract_neighbors_tiled(
        recon,
        stride,
        abs_x,
        abs_y,
        w,
        h,
        geom.tile.top_px(geom.ss),
        geom.tile.left_px(geom.ss),
        // C n_top_px/n_left_px: this plane's ALIGNED extent, so a block
        // straddling a partial superblock replicates the frame edge instead
        // of reading recon a conforming decoder never produces.
        geom.frame_w >> geom.ss,
        geom.frame_h >> geom.ss,
    );
    if fi_mode != FI_NONE {
        let mut above_c = vec![0u8; w + 1];
        above_c[0] = if has_above && has_left {
            top_left
        } else if has_above {
            above[0]
        } else if has_left {
            left[0]
        } else {
            128
        };
        above_c[1..].copy_from_slice(&above);
        ip::predict_filter_intra(dst, w, &above_c, &left, w, h, fi_mode);
        return;
    }
    match mode {
        0 => ip::predict_dc(dst, w, &above, &left, w, h, has_above, has_left),
        1 => ip::predict_v(dst, w, &above, w, h),
        2 => ip::predict_h(dst, w, &left, w, h),
        9 => ip::predict_smooth(dst, w, &above, &left, w, h),
        10 => ip::predict_smooth_v(dst, w, &above, &left, h, h, w),
        11 => ip::predict_smooth_h(dst, w, &above, &left, w, h),
        12 => ip::predict_paeth(dst, w, &above, &left, top_left, w, h),
        m => unreachable!("funnel mode {m}"),
    }
}

/// C `hadamard_path` (product_coding_loop.c:1187): residual over square
/// tiles of `MIN(TX_32X32, eb_max_txsize_lookup[bsize])` — the largest
/// square TX fitting the block (its MIN dimension), capped at 32 — aom
/// Hadamard per tile, SATD accumulated (raster tile order).
pub(super) fn hadamard_satd(
    src: &[u8],
    src_stride: usize,
    src_off: usize,
    pred: &[u8],
    w: usize,
    h: usize,
) -> u64 {
    let tx = w.min(h).min(32);
    let mut satd: u64 = 0;
    let mut res = vec![0i16; tx * tx];
    let mut coeff = vec![0i32; tx * tx];
    for ty in (0..h).step_by(tx) {
        for tx_x in (0..w).step_by(tx) {
            for r in 0..tx {
                let srow = src_off + (ty + r) * src_stride + tx_x;
                let prow = (ty + r) * w + tx_x;
                for c in 0..tx {
                    res[r * tx + c] = src[srow + c] as i16 - pred[prow + c] as i16;
                }
            }
            match tx {
                4 => svtav1_dsp::hadamard::aom_hadamard_4x4(&res, tx, &mut coeff),
                8 => svtav1_dsp::hadamard::aom_hadamard_8x8(&res, tx, &mut coeff),
                16 => svtav1_dsp::hadamard::aom_hadamard_16x16(&res, tx, &mut coeff),
                32 => svtav1_dsp::hadamard::aom_hadamard_32x32(&res, tx, &mut coeff),
                _ => unreachable!("hadamard tile {tx}"),
            }
            satd += svtav1_dsp::hadamard::aom_satd(&coeff) as u64;
        }
    }
    satd
}

/// bd10 mirror of [`hadamard_satd`]: 10-bit residual (`src << 2` minus the
/// 10-bit `pred`) over the same square-tile Hadamard/SATD accumulation. Used
/// ONLY by the bd10 luma mode funnel (task #94, `evaluate_leaf`'s MDS0 fast
/// loop, gated on the bd10 recon canvas). The transform/SATD kernels are
/// bit-depth-independent (i16 residual, i32 coeffs) — only the source scale
/// (`<< 2` from the MSB-truncated u8 the harness feeds) and the u16 `pred`
/// differ. The residual range (−1023..1020) fits i16 exactly.
/// 10-bit twin of [`hadamard_satd`]: the residual is `src10 - pred10` at true
/// depth. `src` is the block-local 10-bit source (task #6 chunk 1 — real u16
/// samples on a native-HBD encode, the `u8 << 2` widening otherwise; the
/// widening used to live in this loop, so the arithmetic is unchanged).
pub(super) fn hadamard_satd_hbd(
    src: &[u16],
    src_stride: usize,
    src_off: usize,
    pred: &[u16],
    w: usize,
    h: usize,
) -> u64 {
    let tx = w.min(h).min(32);
    let mut satd: u64 = 0;
    let mut res = vec![0i16; tx * tx];
    let mut coeff = vec![0i32; tx * tx];
    for ty in (0..h).step_by(tx) {
        for tx_x in (0..w).step_by(tx) {
            for r in 0..tx {
                let srow = src_off + (ty + r) * src_stride + tx_x;
                let prow = (ty + r) * w + tx_x;
                for c in 0..tx {
                    res[r * tx + c] = src[srow + c] as i16 - pred[prow + c] as i16;
                }
            }
            match tx {
                4 => svtav1_dsp::hadamard::aom_hadamard_4x4(&res, tx, &mut coeff),
                8 => svtav1_dsp::hadamard::aom_hadamard_8x8(&res, tx, &mut coeff),
                16 => svtav1_dsp::hadamard::aom_hadamard_16x16(&res, tx, &mut coeff),
                32 => svtav1_dsp::hadamard::aom_hadamard_32x32(&res, tx, &mut coeff),
                _ => unreachable!("hadamard tile {tx}"),
            }
            satd += svtav1_dsp::hadamard::aom_satd(&coeff) as u64;
        }
    }
    satd
}

/// Is a presence-only debug env var set? Cached, because every caller sits on
/// a per-block or per-txb path where a `getenv` per call would be a real
/// regression. One relaxed atomic load when off.
#[cfg(feature = "std")]
pub(super) fn dbg_on(cell: &'static std::sync::OnceLock<bool>, var: &str) -> bool {
    *cell.get_or_init(|| std::env::var_os(var).is_some())
}

/// The `"x,y"` block-pin debug vars (`SVTAV1_CEDGE_XY`, `SVTAV1_QLEV_XY`),
/// parsed once. `Some((x, y))` selects a single block ORIGIN to dump; these
/// dumps are per-txb verbose, so pinning is what keeps them usable.
#[cfg(feature = "std")]
pub(super) fn dbg_xy(
    cell: &'static std::sync::OnceLock<Option<(usize, usize)>>,
    var: &str,
) -> Option<(usize, usize)> {
    *cell.get_or_init(|| {
        let s = std::env::var(var).ok()?;
        let (a, b) = s.split_once(',')?;
        Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
    })
}
