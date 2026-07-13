//! Deblocking: filter-level selection, frame geometry, and decoder-exact
//! frame application.
//!
//! Level picker: C-exact port of the closed-form q-based selection
//! (`svt_av1_pick_filter_level_by_q`, Source/Lib/Codec/deblocking_filter.c:1026,
//! 8-bit KEY_FRAME specialization — see [`pick_filter_levels_key_frame`]).
//!
//! Frame application: ports the DECODER's edge walk (libaom
//! av1/common/av1_loopfilter.c set_lpf_parameters +
//! av1_filter_block_plane_vert/_horz; SVT's deblocking_filter.c carries the
//! same code) over the per-4x4 geometry recorded during the entropy walk.

use alloc::vec::Vec;
use svtav1_dsp::loop_filter as lf;

/// AV1 MAX_LOOP_FILTER.
const MAX_LOOP_FILTER: i32 = 63;

/// Frame loop-filter levels as signaled in the frame header
/// (spec 5.9.11): `[0]` = luma vertical edges, `[1]` = luma horizontal
/// edges, `[2]` = U plane, `[3]` = V plane (both edge directions).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LfLevels {
    pub levels: [u8; 4],
}

impl LfLevels {
    /// True when any plane has a nonzero level (filtering happens at all).
    pub fn any(&self) -> bool {
        // Chroma filtering requires luma levels nonzero (the decoder's
        // plane loop breaks out entirely when both luma levels are 0 —
        // svt_aom_loop_filter_sb / libaom check_planes_to_loop_filter).
        self.levels[0] != 0 || self.levels[1] != 0
    }
}

/// C-exact port of `svt_av1_pick_filter_level_by_q`
/// (deblocking_filter.c:1026) for 8-bit KEY_FRAME pictures.
///
/// The C function reduces to a closed form for still/key frames:
/// - `me_based_dlf_skip` returns immediately for I_SLICE (do_y = do_uv =
///   true), so no zero-forcing applies.
/// - The min-ref-filter-level scan never runs (key frames reference
///   nothing), leaving `min_ref_filter_level[*] = MAX_LOOP_FILTER` (truthy),
///   so every level takes the `clamp(filt_guess, 0, 63)` arm.
/// - 8-bit KEY_FRAME: `filt_guess = ROUND_POWER_OF_TWO(q * 17563 - 421574,
///   18)` with `q = svt_aom_ac_quant_qtx(qindex, 0, 8-bit)` (the AC step
///   table), a linear fit of the searched level ("Keyframes: filt_guess =
///   q * 0.06699 - 1.60817" per the C comment).
/// - `filt_guess_chroma = filt_guess / 2` (C truncating integer division,
///   applied BEFORE clamping), then each level clamps to [0, 63].
///
/// Returns `[filt_guess, filt_guess, chroma, chroma]` for
/// `[level0, level1, U, V]`.
pub fn pick_filter_levels_key_frame(qindex: u8) -> LfLevels {
    let q = svtav1_dsp::quant_tables::AC_QLOOKUP_8[qindex as usize] as i32;
    // ROUND_POWER_OF_TWO(v, 18) on a possibly-negative value: C computes
    // (v + (1 << 17)) >> 18 with an arithmetic shift; Rust's i32 >> is
    // arithmetic too.
    let filt_guess = (q * 17563 - 421574 + (1 << 17)) >> 18;
    let filt_guess_chroma = filt_guess / 2;
    let y = filt_guess.clamp(0, MAX_LOOP_FILTER) as u8;
    let uv = filt_guess_chroma.clamp(0, MAX_LOOP_FILTER) as u8;
    LfLevels {
        levels: [y, y, uv, uv],
    }
}

/// Per-4x4 (mode-info unit) frame geometry for deblocking, recorded during
/// the entropy walk — the same data a decoder derives from parsed
/// partitions/modes: which coded block covers each mi, the block dims
/// (single-TX-per-block invariant: TX dims == block dims for luma, and
/// (bw/2, bh/2) for 4:2:0 chroma via av1_get_max_uv_txsize), and the
/// signaled skip for inter blocks (mbmi->skip_txfm && is_inter_block).
pub struct DeblockGeom {
    /// Luma mi columns (frame_width / 4).
    pub mi_cols: usize,
    /// Luma mi rows (frame_height / 4).
    pub mi_rows: usize,
    /// Covering block's identity per mi (origin mi index; the decoder's
    /// equivalent is the shared MB_MODE_INFO pointer in mi_grid_base —
    /// `pu_edge = mi_prev != mbmi` compares these).
    block_id: Vec<u32>,
    /// Covering block's width in pixels.
    bw: Vec<u8>,
    /// Covering block's height in pixels.
    bh: Vec<u8>,
    /// `mbmi->skip_txfm && is_inter_block(mbmi)`: intra blocks are never
    /// "skipped" for deblocking purposes.
    skip_inter: Vec<bool>,
}

impl DeblockGeom {
    /// Frame dims must be mi-aligned (the pipeline is 64-aligned).
    pub fn new(width: usize, height: usize) -> Self {
        assert!(width % 4 == 0 && height % 4 == 0);
        let mi_cols = width / 4;
        let mi_rows = height / 4;
        let n = mi_cols * mi_rows;
        Self {
            mi_cols,
            mi_rows,
            block_id: alloc::vec![u32::MAX; n],
            bw: alloc::vec![0; n],
            bh: alloc::vec![0; n],
            skip_inter: alloc::vec![false; n],
        }
    }

    /// Record one coded block (in coding order, like the decoder fills
    /// mi_grid_base while parsing).
    pub fn record_block(
        &mut self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        is_inter: bool,
        skip: bool,
    ) {
        debug_assert!(w.is_power_of_two() || w == 4 * (w / 4)); // 4..=64 incl. rects
        let id = ((y / 4) * self.mi_cols + (x / 4)) as u32;
        let skip_inter = is_inter && skip;
        for my in y / 4..((y + h) / 4).min(self.mi_rows) {
            for mx in x / 4..((x + w) / 4).min(self.mi_cols) {
                let i = my * self.mi_cols + mx;
                self.block_id[i] = id;
                self.bw[i] = w as u8;
                self.bh[i] = h as u8;
                self.skip_inter[i] = skip_inter;
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EdgeDir {
    Vert,
    Horz,
}

/// Port of the decoder's `set_lpf_parameters` (libaom
/// av1/common/av1_loopfilter.c:224; SVT deblocking_filter.c:217),
/// specialized to our signaled configuration: delta_lf_present=0,
/// segmentation off, mode_ref_delta_enabled=0 — so the filter level is
/// UNIFORM per (plane, direction) and `get_filter_level` returns `level`
/// for every block (av1_loop_filter_frame_init memsets lfi->lvl).
///
/// (x, y) are PLANE-pixel coords of the 4x4 being visited; `level` is that
/// uniform level. Returns (advance in plane-mi units, Some(filter_length)
/// when this edge filters).
#[allow(clippy::too_many_arguments)]
fn lpf_params(
    geom: &DeblockGeom,
    dir: EdgeDir,
    x: usize,
    y: usize,
    plane: usize,
    ss: usize,
    level: u8,
) -> (usize, Option<u8>) {
    // Chroma maps to the bottom/right mi of the co-located 8x8 luma block
    // (the `scale | ...` trick in C).
    let mi_row = ss | ((y << ss) >> 2);
    let mi_col = ss | ((x << ss) >> 2);
    let idx = mi_row * geom.mi_cols + mi_col;

    // Plane transform dims: single TX per block; chroma TX is the plane
    // block size (av1_get_max_uv_txsize of (bw/2, bh/2), never 64-dim).
    let tx_dims = |i: usize| -> (usize, usize) {
        let (bw, bh) = (geom.bw[i] as usize, geom.bh[i] as usize);
        debug_assert!(bw >= 4 && bh >= 4, "uncovered mi {i}");
        if plane == 0 {
            (bw, bh)
        } else {
            (bw >> 1, bh >> 1)
        }
    };
    let (tw, th) = tx_dims(idx);
    let ts_dim = if dir == EdgeDir::Vert { tw } else { th };
    let advance = ts_dim >> 2;

    let coord = if dir == EdgeDir::Vert { x } else { y };
    // Not a transform edge: no filtering, advance across the TX like C
    // (cannot happen when advancing by whole TX dims from an aligned
    // start — TX dims equal block dims and AV1 blocks are size-aligned —
    // kept for C-faithfulness).
    if coord & (ts_dim - 1) != 0 {
        return (advance, None);
    }
    // Frame/plane boundary edges are never filtered.
    if coord == 0 {
        return (advance, None);
    }

    let prev_idx = match dir {
        EdgeDir::Vert => idx - (1 << ss),
        EdgeDir::Horz => idx - (geom.mi_cols << ss),
    };
    let (ptw, pth) = tx_dims(prev_idx);
    let pv_dim = if dir == EdgeDir::Vert { ptw } else { pth };

    // Uniform level: curr_level == pv_lvl == level (mode_ref_delta_enabled
    // = 0, no segments, no delta_lf — see doc comment).
    let curr_skipped = geom.skip_inter[idx];
    let pv_skip = geom.skip_inter[prev_idx];
    // Block (prediction-unit) edge: different covering blocks. The decoder
    // compares mi_grid pointers (libaom set_one_param_for_line_luma:699);
    // the older mask form tests the same thing for aligned partitions.
    let pu_edge = geom.block_id[prev_idx] != geom.block_id[idx];

    // "if the current and the previous blocks are skipped, deblock the
    // edge if the edge belongs to a PU's edge only."
    if level != 0 && (!pv_skip || !curr_skipped || pu_edge) {
        let min_dim = ts_dim.min(pv_dim);
        // C: TX_4X4 >= min_ts -> 4; plane != 0 -> 6; TX_8X8 -> 8; else 14
        // (equivalently libaom tx_dim_to_filter_length {4,8,14,14,14}).
        let len = if min_dim == 4 {
            4
        } else if plane != 0 {
            6
        } else if min_dim == 8 {
            8
        } else {
            14
        };
        (advance, Some(len))
    } else {
        (advance, None)
    }
}

/// Filter every deblocking edge of one plane, decoder-exactly.
///
/// Pass order: ALL vertical edges, then ALL horizontal edges. The decoder
/// (libaom loop_filter_rows, thread_common.c:467) interleaves per
/// 128px-tall stripe (vert stripe k -> horz stripe k -> vert stripe k+1
/// ...), which is equivalent: vertical filtering only touches its own
/// 4-row band, and horizontal filtering of an edge only writes/reads
/// within [edge-7, edge + below-TX-height), which never crosses the
/// stripe's bottom SB boundary (the filter length is capped by the
/// adjacent TX dims). Within a pass the orders coincide: vertical edges
/// left-to-right per row band, horizontal edges top-to-bottom per column
/// band, bands independent.
#[allow(clippy::too_many_arguments)]
fn filter_plane(
    buf: &mut [u8],
    stride: usize,
    plane_w: usize,
    plane_h: usize,
    plane: usize,
    ss: usize,
    level_vert: u8,
    level_horz: u8,
    geom: &DeblockGeom,
    sharpness: u8,
) {
    let mi_cols_p = plane_w / 4;
    let mi_rows_p = plane_h / 4;

    // Vertical edges (svt_av1_filter_block_plane_vert / libaom
    // av1_filter_block_plane_vert, full-frame extent).
    let tv = lf::lf_thresholds(level_vert, sharpness);
    for my in 0..mi_rows_p {
        let mut mx = 0;
        while mx < mi_cols_p {
            let (adv, filter) =
                lpf_params(geom, EdgeDir::Vert, mx * 4, my * 4, plane, ss, level_vert);
            if let Some(len) = filter {
                let off = (my * 4) * stride + mx * 4;
                match len {
                    4 => lf::lpf_vertical_4(buf, off, stride, tv),
                    6 => lf::lpf_vertical_6(buf, off, stride, tv),
                    8 => lf::lpf_vertical_8(buf, off, stride, tv),
                    _ => lf::lpf_vertical_14(buf, off, stride, tv),
                }
            }
            mx += adv;
        }
    }

    // Horizontal edges.
    let th = lf::lf_thresholds(level_horz, sharpness);
    for mx in 0..mi_cols_p {
        let mut my = 0;
        while my < mi_rows_p {
            let (adv, filter) =
                lpf_params(geom, EdgeDir::Horz, mx * 4, my * 4, plane, ss, level_horz);
            if let Some(len) = filter {
                let off = (my * 4) * stride + mx * 4;
                match len {
                    4 => lf::lpf_horizontal_4(buf, off, stride, th),
                    6 => lf::lpf_horizontal_6(buf, off, stride, th),
                    8 => lf::lpf_horizontal_8(buf, off, stride, th),
                    _ => lf::lpf_horizontal_14(buf, off, stride, th),
                }
            }
            my += adv;
        }
    }
}

/// Apply the AV1 deblocking filter to a reconstructed frame with the
/// signaled `levels`, exactly as a conforming decoder will (spec 7.14;
/// libaom av1_loop_filter_frame). Must run on the OUTPUT copy of the
/// reconstruction only — intra prediction reads unfiltered pixels.
///
/// `u`/`v` are (w/2 x h/2) planes at stride w/2 when `chroma_420`; ignored
/// otherwise. `sharpness` must match the signaled loop_filter_sharpness.
#[allow(clippy::too_many_arguments)]
pub fn apply_deblock_frame(
    y: &mut [u8],
    u: &mut [u8],
    v: &mut [u8],
    width: usize,
    height: usize,
    chroma_420: bool,
    geom: &DeblockGeom,
    lv: &LfLevels,
    sharpness: u8,
) {
    debug_assert_eq!(geom.mi_cols, width / 4);
    debug_assert_eq!(geom.mi_rows, height / 4);
    let l = lv.levels;
    // Plane gating exactly like svt_aom_loop_filter_sb / libaom
    // check_planes_to_loop_filter: both luma levels zero disables ALL
    // planes; chroma planes need their own level nonzero.
    if l[0] == 0 && l[1] == 0 {
        return;
    }
    filter_plane(y, width, width, height, 0, 0, l[0], l[1], geom, sharpness);
    if chroma_420 {
        let (cw, ch) = (width / 2, height / 2);
        if l[2] != 0 {
            filter_plane(u, cw, cw, ch, 1, 1, l[2], l[2], geom, sharpness);
        }
        if l[3] != 0 {
            filter_plane(v, cw, cw, ch, 2, 1, l[3], l[3], geom, sharpness);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-computed values of the C closed form (AC step table x the
    /// KEY_FRAME fit): q_step(30) = 34 -> (597142 - 421574 + 131072) >> 18
    /// = 1; q_step(50) = 54 -> 657900 >> 18 = 2; q_step(63) = 70 ->
    /// 938908 >> 18 = 3. Chroma = y / 2 (truncated before clamp).
    #[test]
    fn key_frame_levels_match_c_formula() {
        assert_eq!(pick_filter_levels_key_frame(30).levels, [1, 1, 0, 0]);
        assert_eq!(pick_filter_levels_key_frame(50).levels, [2, 2, 1, 1]);
        assert_eq!(pick_filter_levels_key_frame(63).levels, [3, 3, 1, 1]);
        // qindex 0: q_step = 4 -> 70252 - 421574 = negative -> clamps to 0.
        assert_eq!(pick_filter_levels_key_frame(0).levels, [0, 0, 0, 0]);
        // Top of the table: q_step(255) = 1828 -> (32105164 - 421574 +
        // 131072) >> 18 = 121 -> clamps to 63; chroma 121/2 = 60.
        assert_eq!(pick_filter_levels_key_frame(255).levels, [63, 63, 60, 60]);
    }

    #[test]
    fn any_requires_luma() {
        assert!(
            !LfLevels {
                levels: [0, 0, 1, 1]
            }
            .any()
        );
        assert!(
            LfLevels {
                levels: [1, 0, 0, 0]
            }
            .any()
        );
        assert!(LfLevels::default() == LfLevels { levels: [0; 4] });
    }
}
