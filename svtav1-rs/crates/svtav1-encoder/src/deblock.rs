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

/// Inputs to the full-image deblock-level search (`LPF_PICK_FROM_FULL_IMAGE`).
pub struct DlfSearchInput<'a> {
    /// Source planes (`enhanced_pic`): the encoder input.
    pub y_src: &'a [u8],
    pub u_src: &'a [u8],
    pub v_src: &'a [u8],
    /// Post-encode reconstruction, BEFORE any deblocking.
    pub y_recon: &'a [u8],
    pub u_recon: &'a [u8],
    pub v_recon: &'a [u8],
    pub width: usize,
    pub height: usize,
    pub chroma_420: bool,
    pub geom: &'a DeblockGeom,
    /// `dlf_ctrls.early_exit_convergence` — 0 at allintra presets <= M3
    /// (dlf_level 1), 1 at M4/M5 (dlf_level 2); `svt_aom_set_dlf_controls`
    /// enc_mode_config.c:2235.
    pub early_exit_convergence: i32,
}

/// Sum of squared differences over one plane —
/// `svt_spatial_full_distortion_kernel` as called by
/// `picture_sse_calculations` (deblocking_filter.c:743) with the full
/// aligned plane dims (our frames are 64-aligned by construction).
fn plane_sse(a: &[u8], b: &[u8], w: usize, h: usize) -> i64 {
    debug_assert!(a.len() >= w * h && b.len() >= w * h);
    let mut sum = 0u64;
    for r in 0..h {
        let ar = &a[r * w..r * w + w];
        let br = &b[r * w..r * w + w];
        for (pa, pb) in ar.iter().zip(br.iter()) {
            let d = i64::from(*pa) - i64::from(*pb);
            sum += (d * d) as u64;
        }
    }
    sum as i64
}

/// C `try_filter_frame` (deblocking_filter.c:777) for one plane: apply the
/// candidate level(s) to a scratch copy of the unfiltered recon and return
/// the SSE vs the source. C filters the live recon then restores it from
/// `temp_lf_recon_buffer`; filtering a scratch clone is arithmetically
/// identical (the kernels are pure functions of the pre-filter plane).
#[allow(clippy::too_many_arguments)]
fn try_filter_plane(
    input: &DlfSearchInput,
    scratch: &mut Vec<u8>,
    plane: usize,
    lvl0: u8,
    lvl1: u8,
) -> i64 {
    let (src, recon, w, h, subs) = match plane {
        0 => (input.y_src, input.y_recon, input.width, input.height, 0),
        1 => (
            input.u_src,
            input.u_recon,
            input.width / 2,
            input.height / 2,
            1,
        ),
        _ => (
            input.v_src,
            input.v_recon,
            input.width / 2,
            input.height / 2,
            1,
        ),
    };
    scratch.clear();
    scratch.extend_from_slice(&recon[..w * h]);
    // svt_av1_loop_filter_frame(recon, pcs, plane, plane + 1): zero levels
    // leave the plane untouched (the per-SB walk skips planes whose level
    // is 0 — svt_aom_loop_filter_sb plane gating).
    if lvl0 != 0 || lvl1 != 0 {
        filter_plane(
            scratch, w, w, h, plane, subs, lvl0, lvl1, input.geom,
            0, // sharpness 0 (matched config; sharpness_level = CLIP3(0,7,0))
        );
    }
    plane_sse(src, scratch, w, h)
}

/// C `search_filter_level` (deblocking_filter.c:832) — the
/// LPF_PICK_FROM_FULL_IMAGE hill-climb over candidate levels with real
/// filtering trials, specialized to STILL/KEY frames:
/// - `last_frame_filter_level` is all-zero (nothing writes the FH lf levels
///   before dlf_process on the sb_based_dlf=0 path; instrumented C confirms
///   `SEARCH last=[0,0,0,0] filt_mid=0 step=4` on every tracked cell), so
///   the start level and the plane-0 dir-2 average are 0.
/// - `bias >>= 1` unconditionally: tx_mode is always TX_MODE_SELECT
///   (!= ONLY_4X4) under FTR_COUPLE_VLPD0_TXS_PER_SB.
///
/// Returns (best level, ss_err[0], ss_err[best]) — the zero/best SSEs feed
/// the caller's bookkeeping (pcs->zero_filt_sse / best_filt_sse).
fn search_filter_level(
    input: &DlfSearchInput,
    scratch: &mut Vec<u8>,
    plane: usize,
    dir: i32,
    last_frame_filter_level: [i32; 4],
    conv_th: i32,
) -> (i32, i64, i64) {
    let min_filter_level = 0i32;
    let max_filter_level = MAX_LOOP_FILTER;
    let mut filt_direction = 0i32;

    let lvl = match plane {
        0 => {
            if dir > 1 {
                (last_frame_filter_level[0] + last_frame_filter_level[1] + 1) >> 1
            } else {
                last_frame_filter_level[dir as usize]
            }
        }
        1 => last_frame_filter_level[2],
        _ => last_frame_filter_level[3],
    };
    let mut filt_mid = lvl.clamp(min_filter_level, max_filter_level);
    let mut filter_step = if filt_mid < 16 { 4 } else { filt_mid / 4 };

    // ss_err[level] = -1 means "not evaluated".
    let mut ss_err = [-1i64; (MAX_LOOP_FILTER + 1) as usize];

    // try_filter_frame at filt_mid. For plane 0 with dir == 2 both luma
    // levels take the candidate; chroma planes carry one level.
    let try_level = |input: &DlfSearchInput, scratch: &mut Vec<u8>, level: i32| -> i64 {
        let (l0, l1) = if plane == 0 {
            // dir == 2 on the still path (dir 0/1 would mix in the frame
            // header's other-direction level, which is only exercised by
            // LPF_PICK_FROM_SUBIMAGE — not ported).
            debug_assert_eq!(dir, 2);
            (level as u8, level as u8)
        } else {
            debug_assert_eq!(dir, 0);
            (level as u8, level as u8)
        };
        try_filter_plane(input, scratch, plane, l0, l1)
    };

    let mut best_err = try_level(input, scratch, filt_mid);
    let mut filt_best = filt_mid;
    ss_err[filt_mid as usize] = best_err;

    let mut tot_convergence = 0i32;
    while filter_step > 0 {
        let filt_high = (filt_mid + filter_step).min(max_filter_level);
        let filt_low = (filt_mid - filter_step).max(min_filter_level);

        // Bias against raising the level in favour of lowering it.
        let mut bias = (best_err >> (15 - (filt_mid / 8))) * i64::from(filter_step);
        // tx_mode != ONLY_4X4 (always TX_MODE_SELECT): bias less for large
        // block sizes.
        bias >>= 1;

        if filt_direction <= 0 && filt_low != filt_mid {
            if ss_err[filt_low as usize] < 0 {
                ss_err[filt_low as usize] = try_level(input, scratch, filt_low);
            }
            // Bias toward the lower loop-filter value when close.
            if ss_err[filt_low as usize] < (best_err + bias) {
                if ss_err[filt_low as usize] < best_err {
                    best_err = ss_err[filt_low as usize];
                }
                filt_best = filt_low;
            }
        }
        if filt_direction >= 0 && filt_high != filt_mid {
            if ss_err[filt_high as usize] < 0 {
                ss_err[filt_high as usize] = try_level(input, scratch, filt_high);
            }
            // Raising the level must beat the bias.
            if ss_err[filt_high as usize] < (best_err - bias) {
                best_err = ss_err[filt_high as usize];
                filt_best = filt_high;
            }
        }

        if filt_best == filt_mid {
            tot_convergence += 1;
            if tot_convergence == conv_th {
                filter_step = 0;
            } else {
                filter_step /= 2;
            }
            filt_direction = 0;
        } else {
            filt_direction = if filt_best < filt_mid { -1 } else { 1 };
            filt_mid = filt_best;
        }
    }
    best_err = ss_err[filt_best as usize];

    (filt_best, ss_err[0], best_err)
}

/// C-exact `svt_av1_pick_filter_level(.., LPF_PICK_FROM_FULL_IMAGE)`
/// (deblocking_filter.c:1138) + the dlf_process zero-SSE guard
/// (dlf_process.c:103), for STILL/KEY frames at allintra presets <= M5
/// (`get_dlf_level_allintra` 1/2 -> `sb_based_dlf = 0`,
/// enc_mode_config.c:2214).
///
/// Key-frame facts baked in (each verified on the instrumented library —
/// `DLFDBG SEARCH/STEP/PICK/FINAL/GUARD` captures, docs/captures):
/// - sharpness_level = 0 (config sharpness 0, tune != VQ/IQ/MS_SSIM).
/// - The dlf_avg reference-average block is skipped
///   (`tot_ref_frame_types == 0` — captured `GUARD .. tot_refs=0`).
/// - `me_based_dlf_skip` returns immediately for I_SLICE (do_y = do_uv).
/// - `frame_is_boosted` is true for key frames, so luma ALWAYS searches
///   (never the use_ref_avg_y arm) and chroma searches whenever luma picked
///   a nonzero level (never the use_ref_avg_uv arm).
/// - The dlf_process zero-SSE guard (`zero_filt_sse == -1` recompute) can
///   never fire: the luma search starts at level 0, so ss_err[0] is always
///   evaluated and zero_filt_sse != -1. Not ported (documented no-op).
pub fn pick_filter_levels_full_search(input: &DlfSearchInput) -> LfLevels {
    let mut scratch: Vec<u8> = Vec::with_capacity(input.width * input.height);
    let last = [0i32; 4];

    // Luma: one level for both edge directions (dir = 2).
    let (filt_y, _zero_sse, _best_sse) = search_filter_level(
        input,
        &mut scratch,
        0,
        2,
        last,
        input.early_exit_convergence,
    );

    // Chroma filtering is not allowed when the luma filters are off; when
    // luma is on, key frames search U and V independently (dir = 0).
    let (filt_u, filt_v) = if filt_y == 0 || !input.chroma_420 {
        (0, 0)
    } else {
        let (u, _, _) = search_filter_level(
            input,
            &mut scratch,
            1,
            0,
            last,
            input.early_exit_convergence,
        );
        let (v, _, _) = search_filter_level(
            input,
            &mut scratch,
            2,
            0,
            last,
            input.early_exit_convergence,
        );
        (u, v)
    };

    LfLevels {
        levels: [filt_y as u8, filt_y as u8, filt_u as u8, filt_v as u8],
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
    /// The raw signaled `mbmi->skip_txfm` (any block type) — CDEF's
    /// per-8x8 skip-all test uses this directly (libaom
    /// is_8x8_block_skip, av1/common/cdef.c:29).
    skip: Vec<bool>,
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
            skip: alloc::vec![false; n],
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
                self.skip[i] = skip;
            }
        }
    }

    /// CDEF's per-8x8 skip test: true when all four mi units of the 8x8
    /// whose top-left mi is (mi_row, mi_col) carry the signaled skip flag
    /// (libaom `is_8x8_block_skip`, av1/common/cdef.c:29 — the dlist in
    /// av1_cdef_compute_sb_list excludes exactly these blocks).
    pub fn is_8x8_all_skip(&self, mi_row: usize, mi_col: usize) -> bool {
        for r in mi_row..(mi_row + 2).min(self.mi_rows) {
            for c in mi_col..(mi_col + 2).min(self.mi_cols) {
                if !self.skip[r * self.mi_cols + c] {
                    return false;
                }
            }
        }
        true
    }

    /// True when EVERY 8x8 unit of the frame is all-skip — i.e. every
    /// 64x64 filter block's CDEF dlist is empty. C equivalence: the CDEF
    /// search kernel's `svt_sb_compute_cdef_list` returns 0 for every fb,
    /// so `pcs->skip_cdef_seg[]` is all-1 (cdef_process.c:385-392) and
    /// `finish_cdef_search` runs its strength selection with
    /// `sb_count == 0` (enc_cdef.c:1296-1316).
    pub fn cdef_frame_all_skip(&self) -> bool {
        for r in (0..self.mi_rows).step_by(2) {
            for c in (0..self.mi_cols).step_by(2) {
                if !self.is_8x8_all_skip(r, c) {
                    return false;
                }
            }
        }
        true
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
            // Chroma TX is never smaller than TX_4X4: av1_get_max_uv_txsize
            // floors at 4x4 (a 4-wide/tall luma block shares one 4x4 chroma
            // TX across its 2x2 luma group via is_chroma_reference). Without
            // the floor, bw==4 gives a 2-wide chroma dim and advance == 0 in
            // filter_plane's edge walk -> infinite loop.
            ((bw >> 1).max(4), (bh >> 1).max(4))
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
