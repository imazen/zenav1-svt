//! CDEF: frame-level strength/damping selection and (further down, once the
//! application port lands) the decoder-exact frame filter pass.
//!
//! Strength selection: C-exact port of the closed-form QP predictor
//! `svt_pick_cdef_from_qp` (Source/Lib/Codec/enc_cdef.c:849) specialized to
//! its intra branch (`frame_type == KEY_FRAME`), i.e. the C encoder's
//! `use_qp_strength` fast path (enc_cdef.c:1143), which signals
//! `cdef_bits = 0` with a single strength pair — NOT the C default preset
//! policy (the RDO `svt_av1_cdef_search`), which lands with decision parity.
//! Damping: `CDEF_DAMPING_FROM_QP` = `3 + (base_q_idx >> 6)`
//! (enc_cdef.c:923, also inlined at enc_cdef.c:1154/1256/1443).

use svtav1_dsp::quant_tables::AC_QLOOKUP_8;

/// Frame-level CDEF parameters as signaled in `cdef_params()` (spec 5.9.19)
/// with `cdef_bits = 0`: one packed 6-bit strength per plane type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CdefFrameParams {
    /// `cdef_damping`: 3..=6 (`cdef_damping_minus_3` is a 2-bit field).
    pub damping: u8,
    /// Packed luma strength: `pri * 4 + sec_signaled` (0..=63).
    pub y_strength: u8,
    /// Packed chroma strength (ignored / not signaled for monochrome).
    pub uv_strength: u8,
}

impl Default for CdefFrameParams {
    /// All-zero strengths with the minimum legal damping — the decoder's
    /// `do_cdef` gate (libaom decodeframe.c:5417) is then false and the
    /// CDEF pass is skipped entirely on both sides.
    fn default() -> Self {
        Self {
            damping: 3,
            y_strength: 0,
            uv_strength: 0,
        }
    }
}

impl CdefFrameParams {
    /// True when a conforming decoder will run the CDEF frame pass: libaom
    /// `do_cdef = cdef_bits || cdef_strengths[0] || cdef_uv_strengths[0]`
    /// (decodeframe.c:5417; we always signal cdef_bits = 0). Monochrome
    /// streams never signal uv_strength, so the decoder sees 0 there.
    pub fn any(&self, monochrome: bool) -> bool {
        self.y_strength != 0 || (!monochrome && self.uv_strength != 0)
    }

    /// The `[damping, y_strength, uv_strength]` triple the frame-header
    /// writer takes.
    pub fn signal(&self) -> [u8; 3] {
        [self.damping, self.y_strength, self.uv_strength]
    }
}

/// C-exact port of `svt_pick_cdef_from_qp` (enc_cdef.c:849), intra branch
/// (`is_screen_content = 0`, `frame_type == KEY_FRAME`), plus the
/// `CDEF_DAMPING_FROM_QP` damping derivation (enc_cdef.c:923).
///
/// `q = svt_aom_ac_quant_qtx(base_q_idx, 0, 8) >> 0` is the 8-bit AC step;
/// the strength fits are evaluated in f32 exactly as C does (float
/// constants, left-associated sum, `roundf` = round-half-away-from-zero,
/// which is `f32::round`), then clamped to the 4-/2-bit field ranges and
/// packed `f1 * CDEF_SEC_STRENGTHS + f2`.
///
/// Firing profile on the AC table (C-verified in tests +
/// tests/c_parity_cdef_pick.rs): zero at very low qindex (<= ~50, CDEF
/// hurts near-lossless), luma pri kicks in around the qindex-60 knee
/// (y = 4 at qindex 63/80), growing to y = 9/17/43 at qindex 128/172/220
/// and saturating at 63 (pri 15 / sec field 3) at qindex 255.
pub fn pick_cdef_params_key_frame(qindex: u8) -> CdefFrameParams {
    let q = AC_QLOOKUP_8[qindex as usize] as i32 as f32;

    // enc_cdef.c:880-888 (Intra branch), verbatim constants.
    let y_f1 =
        (q * q * 0.000_003_373_197_4_f32 + q * 0.008_070_594_f32 + 0.018_763_4_f32).round() as i32;
    let y_f2 = (q * q * 0.000_002_916_734_3_f32 + q * 0.002_779_862_4_f32 + 0.007_940_5_f32).round()
        as i32;
    let uv_f1 = (q * q * -0.000_013_079_099_5_f32 + q * 0.012_892_405_f32 - 0.007_483_88_f32)
        .round() as i32;
    let uv_f2 = (q * q * 0.000_003_265_178_3_f32 + q * 0.000_355_201_83_f32 + 0.002_280_92_f32)
        .round() as i32;

    // "Clamp to AV1 limits" (enc_cdef.c:891-895).
    let y_f1 = y_f1.clamp(0, 15);
    let y_f2 = y_f2.clamp(0, 3);
    let uv_f1 = uv_f1.clamp(0, 15);
    let uv_f2 = uv_f2.clamp(0, 3);

    CdefFrameParams {
        damping: 3 + (qindex >> 6),
        y_strength: (y_f1 * 4 + y_f2) as u8,
        uv_strength: (uv_f1 * 4 + uv_f2) as u8,
    }
}

/// Whether C's allintra path runs the CDEF RDO *search* (vs the
/// `use_qp_strength` fast path) at this preset.
///
/// C: `svt_aom_sig_deriv_multi_processes_allintra` cdef derivation
/// (enc_mode_config.c:3543-3600, `fast_decode == 0` branch — our configs):
/// MR -> level 1, M0 -> 2, M1..M3 -> 3, M4..M5 -> 5, M6 -> 7, M7+ ->
/// level 10 (OPT_CDEF_PRI_ONLY=1 && FIX_RTC_M13=1, EbDebugMacros.h:59/114).
/// `set_cdef_search_controls`: levels 1..=7 all set
/// `use_qp_strength = false` (search); level 10 sets it true
/// (enc_mode_config.c:1688-1692). So in the u8 preset domain: search for
/// presets 0..=6, qp fast path for 7+ (MR = -1 is unreachable).
pub fn allintra_preset_uses_cdef_search(preset: u8) -> bool {
    preset <= 6
}

/// The C-exact `finish_cdef_search` outcome for a frame in which EVERY
/// 64x64 filter block is CDEF-skipped (`sb_count == 0`) at a search
/// preset — the still/allintra <=M6 case for all-skip content.
///
/// Trace of enc_cdef.c:1129-1449 with `use_qp_strength = false`,
/// `use_reference_cdef_fs = 0` (I_SLICE: case-7 sets it from
/// `is_not_highest_layer`, which is TRUE for a KEY/KF_UPDATE frame —
/// `frame_is_leaf` = LF_UPDATE only, enc_mode_config.h:192 — and
/// `update_cdef_filters_on_ref_info` only mutates non-I slices,
/// md_config_process.c:709-711) and `sb_count == 0`:
///
/// - `joint_strength_search_dual` with 0 filter blocks leaves
///   `best_lev0/1 = {0}` and returns tot_mse 0 (svt_search_one_dual's
///   accumulators stay zero; the argmin lands on (0,0),
///   enc_cdef.c:740-811).
/// - The nb_strength_bits loop then minimizes pure rate:
///   `total_bits = sb_count*i + (1<<i)*CDEF_STRENGTH_BITS*2` — strictly
///   increasing in i at sb_count 0, so `cdef_bits = 0` wins
///   (enc_cdef.c:1368-1388).
/// - The final strength remap `filter_map[best_lev] = pf_gi[0] = 0`
///   (enc_mode_config.c:16), so `cdef_y_strength[0] =
///   cdef_uv_strength[0] = 0`.
/// - `cdef_damping = 3 + (base_q_idx >> 6)` (enc_cdef.c:1446) — same
///   formula as the qp path.
///
/// The outcome is independent of lambda and zero_fs_cost_bias (both only
/// touch per-fb mse rows; there are none). This is the ONLY piece of the
/// RDO search ported so far: frames with any non-skip filter block at a
/// search preset still take the qp fast path and hence still diverge
/// from C's searched strengths (gap 2a, narrowed).
pub fn pick_cdef_params_all_skip_search(qindex: u8) -> CdefFrameParams {
    CdefFrameParams {
        damping: 3 + (qindex >> 6),
        y_strength: 0,
        uv_strength: 0,
    }
}

// ---------------------------------------------------------------------------
// Decoder-exact frame application
// ---------------------------------------------------------------------------

use crate::deblock::DeblockGeom;
use alloc::vec::Vec;
use svtav1_dsp::cdef as k;

/// Evidence counters from one CDEF frame pass.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CdefStats {
    /// Pixels covered by a filter invocation with nonzero effective
    /// strength (adjusted primary or secondary) — "CDEF did real work".
    pub filtered_px: u64,
    /// Subset of `filtered_px` whose value actually changed.
    pub changed_px: u64,
}

/// Apply the CDEF frame pass to the (already deblocked) output
/// reconstruction, exactly as a conforming decoder does — the authority is
/// libaom `av1_cdef_frame` (av1/common/cdef.c:467; what aomdec runs
/// single-threaded), with SVT's `svt_av1_cdef_frame` (cdef_process.c)
/// carrying the same structure. Specialized to our signaled configuration:
/// 8-bit, 64x64 superblocks (CDEF unit == SB, so the 128x128 sub-unit
/// indexing collapses), `cdef_bits = 0` (every unit uses strength set 0),
/// single tile, 8-aligned frame dims (see the alignment note below).
///
/// EQUIVALENCE TO THE C LINEBUFFER/COLBUF DISCIPLINE: `av1_cdef_frame`
/// assembles each fb's padded 16-bit source from (a) the deblocked frame
/// for the fb itself + not-yet-filtered right/bottom neighbors, (b)
/// `top_linebuf`/`bot_linebuf` (rows saved from the deblocked frame BEFORE
/// the rows above got CDEF-filtered), (c) `colbuf` (the left fb's right
/// edge saved pre-CDEF), and (d) CDEF_VERY_LARGE fills outside the frame.
/// In single-threaded raster order every one of those sources is the
/// post-deblock, PRE-CDEF value of the pixel — the machinery exists only to
/// avoid a full-frame copy (and for MT row sync). We keep an explicit
/// pre-CDEF snapshot instead, so the buffer build reduces to: in-frame ->
/// snapshot pixel, out-of-frame -> CDEF_VERY_LARGE. Region-by-region proof
/// against cdef_prepare_fb (all 13 copy/fill sites) in the port notes of
/// this commit; the recon-parity gate (encoder recon == aomdec, byte-exact,
/// 216 streams with CDEF firing) is the binary judge.
///
/// The per-64x64 skip rule: a unit whose 8x8s are all-skip gets no dlist
/// entries; a fully-skip unit also never had a cdef_idx transmitted
/// (`mbmi->cdef_strength == -1` early-out in libaom cdef_fb_col) — with
/// cdef_bits = 0 both conditions coincide with "dlist empty" (a strength is
/// transmitted iff some block has skip = 0 iff some 8x8 is non-skip).
///
/// Frame dims must be multiples of 8 (asserted): partial 64x64 fbs are
/// handled exactly like C (`nvb = min(16, mi_rows - 16*fbr)` mi units), and
/// 8-alignment guarantees no partial 8x8 blocks exist, sidestepping the
/// C mi-grid over-read semantics for 4px tails (which today's pipeline
/// cannot code anyway — the partition writer has no partial-SB syntax).
pub fn apply_cdef_frame(
    y: &mut [u8],
    u: &mut [u8],
    v: &mut [u8],
    width: usize,
    height: usize,
    chroma_420: bool,
    geom: &DeblockGeom,
    params: &CdefFrameParams,
) -> CdefStats {
    let mut stats = CdefStats::default();
    // Decoder's frame-level gate (libaom decodeframe.c:5417 do_cdef): with
    // cdef_bits = 0 and all strengths 0 the pass does not run at all.
    if !params.any(!chroma_420) {
        return stats;
    }
    assert!(width % 8 == 0 && height % 8 == 0, "8-aligned frames only");

    // Strength decomposition (libaom cdef_fb_col, av1/common/cdef.c:323):
    // level = strength / 4, sec = strength % 4, sec 3 decodes as 4.
    let lvl_y = (params.y_strength / 4) as i32;
    let mut sec_y = (params.y_strength % 4) as i32;
    sec_y += i32::from(sec_y == 3);
    let (lvl_uv, sec_uv) = if chroma_420 {
        let l = (params.uv_strength / 4) as i32;
        let mut s = (params.uv_strength % 4) as i32;
        s += i32::from(s == 3);
        (l, s)
    } else {
        (0, 0)
    };
    let zero_y = lvl_y == 0 && sec_y == 0;
    let zero_uv = lvl_uv == 0 && sec_uv == 0;
    let damping = params.damping as i32; // + coeff_shift (=0 at 8-bit)

    let nvfb = height.div_ceil(64);
    let nhfb = width.div_ceil(64);

    // Pre-CDEF (post-deblock) snapshot per plane — see the doc comment.
    let pre_y: Vec<u8> = y.to_vec();
    let (pre_u, pre_v): (Vec<u8>, Vec<u8>) = if chroma_420 && !zero_uv {
        (u.to_vec(), v.to_vec())
    } else {
        (Vec::new(), Vec::new())
    };

    let mut src = alloc::vec![0u16; k::CDEF_INBUF_SIZE];
    let mut dir = [[0i32; 8]; 8];
    let mut var = [[0i32; 8]; 8];

    for fbr in 0..nvfb {
        let vsize = 64.min(height - fbr * 64); // nvb << 2 in C mi units
        for fbc in 0..nhfb {
            let hsize = 64.min(width - fbc * 64);
            // dlist: non-skip 8x8s of this (possibly partial) 64x64 unit,
            // raster order (av1_cdef_compute_sb_list).
            let mut dlist: Vec<(usize, usize)> = Vec::with_capacity(64);
            for by in 0..vsize / 8 {
                for bx in 0..hsize / 8 {
                    if !geom.is_8x8_all_skip(fbr * 16 + by * 2, fbc * 16 + bx * 2) {
                        dlist.push((by, bx));
                    }
                }
            }
            if dlist.is_empty() {
                continue;
            }

            // ---- Luma (pli 0): always prepared — the direction search
            // runs here even at zero luma strength because chroma reuses
            // dir[][] (libaom cdef_fb_col never skips plane 0).
            build_src(
                &mut src,
                &pre_y,
                width,
                height,
                fbr * 64,
                fbc * 64,
                vsize,
                hsize,
            );
            let base = k::CDEF_VBORDER * k::CDEF_BSTRIDE + k::CDEF_HBORDER;
            for &(by, bx) in &dlist {
                let (d, vr) = k::cdef_find_dir(
                    &src[base + by * 8 * k::CDEF_BSTRIDE + bx * 8..],
                    k::CDEF_BSTRIDE,
                    0,
                );
                dir[by][bx] = d as i32;
                var[by][bx] = vr;
            }
            if !zero_y {
                for &(by, bx) in &dlist {
                    // pli 0: variance-adjusted primary strength.
                    let t = k::adjust_strength(lvl_y, var[by][bx]);
                    if t == 0 && sec_y == 0 {
                        // libaom dispatches the enable-nothing variant,
                        // which writes x back — a no-op on our buffer.
                        continue;
                    }
                    let px = fbc * 64 + bx * 8;
                    let py = fbr * 64 + by * 8;
                    let doff = py * width + px;
                    filter_and_count(
                        y,
                        doff,
                        width,
                        &src,
                        base + by * 8 * k::CDEF_BSTRIDE + bx * 8,
                        t,
                        sec_y,
                        if lvl_y != 0 { dir[by][bx] } else { 0 },
                        damping,
                        k::BLOCK_8X8,
                        &mut stats,
                    );
                }
            }

            // ---- Chroma (pli 1/2), 4:2:0: 4x4 blocks, luma dirs, no
            // adjust_strength, damping - 1 (libaom av1_cdef_filter_fb:
            // damping += coeff_shift - (pli != AOM_PLANE_Y)).
            if chroma_420 && !zero_uv {
                let (cw, ch) = (width / 2, height / 2);
                for (pre_c, buf) in [(&pre_u, &mut *u), (&pre_v, &mut *v)] {
                    build_src(
                        &mut src,
                        pre_c,
                        cw,
                        ch,
                        fbr * 32,
                        fbc * 32,
                        vsize / 2,
                        hsize / 2,
                    );
                    for &(by, bx) in &dlist {
                        let px = fbc * 32 + bx * 4;
                        let py = fbr * 32 + by * 4;
                        filter_and_count(
                            buf,
                            py * cw + px,
                            cw,
                            &src,
                            base + by * 4 * k::CDEF_BSTRIDE + bx * 4,
                            lvl_uv,
                            sec_uv,
                            if lvl_uv != 0 { dir[by][bx] } else { 0 },
                            damping - 1,
                            k::BLOCK_4X4,
                            &mut stats,
                        );
                    }
                }
            }
        }
    }
    stats
}

/// Build one plane's padded fb source: `src[r][c]` = snapshot pixel when
/// (plane_y0 + r - VBORDER, plane_x0 + c - HBORDER) is inside the plane,
/// else CDEF_VERY_LARGE — exactly what cdef_prepare_fb assembles for a
/// 64-aligned frame (see apply_cdef_frame docs).
fn build_src(
    src: &mut [u16],
    pre: &[u8],
    plane_w: usize,
    plane_h: usize,
    y0: usize,
    x0: usize,
    vsize: usize,
    hsize: usize,
) {
    for r in 0..(vsize + 2 * k::CDEF_VBORDER) {
        let gy = y0 as isize + r as isize - k::CDEF_VBORDER as isize;
        let row = &mut src[r * k::CDEF_BSTRIDE..r * k::CDEF_BSTRIDE + hsize + 2 * k::CDEF_HBORDER];
        if gy < 0 || gy >= plane_h as isize {
            row.fill(k::CDEF_VERY_LARGE);
            continue;
        }
        let gy = gy as usize;
        for (c, out) in row.iter_mut().enumerate() {
            let gx = x0 as isize + c as isize - k::CDEF_HBORDER as isize;
            *out = if gx < 0 || gx >= plane_w as isize {
                k::CDEF_VERY_LARGE
            } else {
                pre[gy * plane_w + gx as usize] as u16
            };
        }
    }
}

/// Run the C-exact block kernel into `buf` and account evidence counters.
#[allow(clippy::too_many_arguments)]
fn filter_and_count(
    buf: &mut [u8],
    doff: usize,
    dstride: usize,
    src: &[u16],
    ioff: usize,
    pri: i32,
    sec: i32,
    dir: i32,
    damping: i32,
    bsize: i32,
    stats: &mut CdefStats,
) {
    let dim = if bsize == k::BLOCK_8X8 { 8 } else { 4 };
    let mut before = [0u8; 64];
    for r in 0..dim {
        before[r * dim..r * dim + dim]
            .copy_from_slice(&buf[doff + r * dstride..doff + r * dstride + dim]);
    }
    k::cdef_filter_block(
        buf, doff, dstride, src, ioff, pri, sec, dir, damping, damping, bsize, 0, 1,
    );
    stats.filtered_px += (dim * dim) as u64;
    for r in 0..dim {
        for c in 0..dim {
            if buf[doff + r * dstride + c] != before[r * dim + c] {
                stats.changed_px += 1;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// CDEF RDO strength search (svt_av1_cdef_search + finish_cdef_search)
// ---------------------------------------------------------------------------

/// Outcome of the live-block CDEF search.
pub enum CdefSearchPick {
    /// Every 64x64 filter block is all-skip (`sb_count == 0`): the caller
    /// takes `pick_cdef_params_all_skip_search` (already C-exact).
    AllSkip,
    /// A single strength pair won (`cdef_bits = 0`) — the fully supported
    /// case; all six tracked identity cells land here.
    Picked(CdefFrameParams),
    /// The RD search preferred `cdef_bits > 0` (multiple strength pairs).
    /// The tile writer has no per-SB `cdef_idx` emission yet, so the
    /// caller falls back to the qp fast path (self-consistent stream,
    /// documented divergence from C — see docs/IDENTITY-STATUS.md).
    MultiStrength,
}

/// The level-7 candidate strengths: `first_pass = {pf_gi[0], pf_gi[15]}`,
/// `second_pass = first_pass + 2` (enc_mode_config.c:1579-1607) — i.e.
/// {(pri 0, sec 0), (pri 15, sec 0), (pri 0, sec 2), (pri 15, sec 2)}.
const L7_FS: [i32; 4] = [0, 60, 2, 62];
/// Chroma candidates: only the first-pass pair is evaluated
/// (`default_second_pass_fs_uv = -1`); the second-pass rows carry the
/// `default_mse_uv * 64` sentinel (cdef_process.c:494/569).
const L7_UV_VALID: [bool; 4] = [true, true, false, false];
const DEFAULT_MSE_UV: u64 = 1_040_400; // enc_cdef.c / cdef_process.c:240

/// C `RDCOST` (identical macro to rd_cost.h).
#[inline]
fn rdc(lambda: u64, rate: u64, dist: u64) -> u64 {
    ((rate * lambda + 256) >> 9) + (dist << 7)
}

/// `svt_search_one_dual_c` (enc_cdef.c:740), specialized to
/// `start_gi = 0, end_gi = 4`.
fn search_one_dual(
    lev0: &mut [i32; 8],
    lev1: &mut [i32; 8],
    nb_strengths: usize,
    mse: &[[[u64; 4]; 2]],
) -> u64 {
    let mut tot_mse = [[0u64; 4]; 4];
    for row in mse {
        let mut best_mse = 1u64 << 63;
        for gi in 0..nb_strengths {
            let curr = row[0][lev0[gi] as usize] + row[1][lev1[gi] as usize];
            if curr < best_mse {
                best_mse = curr;
            }
        }
        for (j, tj) in tot_mse.iter_mut().enumerate() {
            for (kk, t) in tj.iter_mut().enumerate() {
                let curr = row[0][j] + row[1][kk];
                *t += curr.min(best_mse);
            }
        }
    }
    let mut best_tot = 1u64 << 63;
    let (mut best_id0, mut best_id1) = (0usize, 0usize);
    for (j, tj) in tot_mse.iter().enumerate() {
        for (kk, &t) in tj.iter().enumerate() {
            if t < best_tot {
                best_tot = t;
                best_id0 = j;
                best_id1 = kk;
            }
        }
    }
    lev0[nb_strengths] = best_id0 as i32;
    lev1[nb_strengths] = best_id1 as i32;
    best_tot
}

/// `joint_strength_search_dual` (enc_cdef.c:813): greedy + 4*nb rotation
/// refinement.
fn joint_strength_search_dual(
    best_lev0: &mut [i32; 8],
    best_lev1: &mut [i32; 8],
    nb_strengths: usize,
    mse: &[[[u64; 4]; 2]],
) -> u64 {
    let mut best_tot = 1u64 << 63;
    for i in 0..nb_strengths {
        best_tot = search_one_dual(best_lev0, best_lev1, i, mse);
    }
    for _ in 0..4 * nb_strengths {
        for j in 0..nb_strengths - 1 {
            best_lev0[j] = best_lev0[j + 1];
            best_lev1[j] = best_lev1[j + 1];
        }
        best_tot = search_one_dual(best_lev0, best_lev1, nb_strengths - 1, mse);
    }
    best_tot
}

/// The `finish_cdef_search` RD half (enc_cdef.c:1063-1127) over already
/// computed per-fb mse rows (zero_fs_cost_bias = 0 at allintra <= M7 —
/// cdef_recon_level 0 — so no mse scaling). Returns (cdef_bits,
/// y_strength, uv_strength) in the SEARCH-INDEX domain for bits = 0, or
/// the bits count when > 0.
fn finish_cdef_rd(mse: &[[[u64; 4]; 2]], qindex: u8) -> (usize, usize, usize) {
    let sb_count = mse.len();
    debug_assert!(sb_count > 0);
    let lambda = crate::pd0::kf_full_lambda_8bit_unweighted(qindex) as u64;
    let mut best_cost = 1u64 << 63;
    let mut best_bits = 0usize;
    let mut best_pair = (0usize, 0usize);
    for i in 0..=3usize {
        let nb = 1usize << i;
        let mut lev0 = [0i32; 8];
        let mut lev1 = [0i32; 8];
        let tot = joint_strength_search_dual(&mut lev0, &mut lev1, nb, mse);
        // CDEF_STRENGTH_BITS = 6, two planes; av1_cost_literal = n << 9.
        let total_bits = (sb_count * i + nb * 6 * 2) as u64;
        let cost = rdc(lambda, total_bits << 9, tot * 16);
        if cost < best_cost {
            best_cost = cost;
            best_bits = i;
            best_pair = (lev0[0] as usize, lev1[0] as usize);
        }
    }
    (best_bits, best_pair.0, best_pair.1)
}

/// One fb's packed filter pass: C `svt_cdef_filter_fb` with `dstride = 0`
/// (search mode): each dlist block lands at `bi << (bsizex + bsizey)` with
/// row stride `1 << bsizex`, rows stepped by `subsampling` (unfiltered rows
/// keep stale bytes exactly like C's tmp_dst — the dist reads the same
/// subsampled rows only).
#[allow(clippy::too_many_arguments)]
fn filter_fb_packed(
    tmp: &mut [u8],
    src_pad: &[u16],
    dlist: &[(usize, usize)],
    strength: i32,
    damping: i32,
    pli: usize,
    subsampling: usize,
    dir: &mut [[i32; 8]; 8],
    var: &mut [[i32; 8]; 8],
    dirinit: &mut bool,
) {
    let pri_strength = strength / 4;
    let sec = strength % 4;
    let sec_strength = sec + i32::from(sec == 3);
    let damping = damping - i32::from(pli != 0);
    let (bsizex, bsizey, bsize) = if pli == 0 {
        (3usize, 3usize, k::BLOCK_8X8)
    } else {
        (2, 2, k::BLOCK_4X4)
    };
    let base = k::CDEF_VBORDER * k::CDEF_BSTRIDE + k::CDEF_HBORDER;

    if strength == 0 {
        // Copy path (svt_cdef_filter_fb, cdef.c:336-364).
        for (bi, &(by, bx)) in dlist.iter().enumerate() {
            let ioff = base + ((by << bsizey) * k::CDEF_BSTRIDE) + (bx << bsizex);
            let doff = bi << (bsizex + bsizey);
            let mut iy = 0usize;
            while iy < (1 << bsizey) {
                for ix in 0..(1 << bsizex) {
                    tmp[doff + (iy << bsizex) + ix] =
                        src_pad[ioff + iy * k::CDEF_BSTRIDE + ix] as u8;
                }
                iy += subsampling;
            }
        }
        return;
    }

    if pli == 0 && !*dirinit {
        for &(by, bx) in dlist {
            let (d, vr) = k::cdef_find_dir(
                &src_pad[base + by * 8 * k::CDEF_BSTRIDE + bx * 8..],
                k::CDEF_BSTRIDE,
                0,
            );
            dir[by][bx] = d as i32;
            var[by][bx] = vr;
        }
        *dirinit = true;
    }

    for (bi, &(by, bx)) in dlist.iter().enumerate() {
        let t = if pli != 0 {
            pri_strength
        } else {
            k::adjust_strength(pri_strength, var[by][bx])
        };
        let doff = bi << (bsizex + bsizey);
        let ioff = base + ((by << bsizey) * k::CDEF_BSTRIDE) + (bx << bsizex);
        k::cdef_filter_block(
            tmp,
            doff,
            1 << bsizex,
            src_pad,
            ioff,
            t,
            sec_strength,
            if pri_strength != 0 { dir[by][bx] } else { 0 },
            damping,
            damping,
            bsize,
            0,
            subsampling,
        );
    }
}

/// `svt_aom_compute_cdef_dist_8bit_c` (enc_cdef.c): SSE between the SOURCE
/// plane and the packed filtered blocks over the subsampled rows.
fn dist_packed(
    tmp: &[u8],
    src_plane: &[u8],
    plane_w: usize,
    fb_y0: usize,
    fb_x0: usize,
    dlist: &[(usize, usize)],
    luma: bool,
    subsampling: usize,
) -> u64 {
    let dim = if luma { 8usize } else { 4 };
    let mut sum = 0u64;
    for (bi, &(by, bx)) in dlist.iter().enumerate() {
        let s0 = (fb_y0 + by * dim) * plane_w + fb_x0 + bx * dim;
        let p0 = bi * dim * dim;
        let mut i = 0usize;
        while i < dim {
            for j in 0..dim {
                let e = src_plane[s0 + i * plane_w + j] as i32 - tmp[p0 + i * dim + j] as i32;
                sum += (e * e) as u64;
            }
            i += subsampling;
        }
    }
    sum
}

/// The full still-frame CDEF strength search at allintra search presets
/// with the level-7 controls (M6; levels 5/3 share the machinery but wider
/// strength sets — unported): per 64x64 filter block, filter the
/// POST-DEBLOCK recon at each candidate strength and measure SSE against
/// the source over every-4th luma row (chroma 4x4 blocks cap the
/// subsampling at 1), then run the C RD pick over signal-bit counts.
///
/// C references: svt_av1_cdef_search (cdef_process.c:300-640, damping
/// `3 + (qindex >> 6)`, `mse *= subsampling_factor`, V accumulates into
/// the joint uv row) + finish_cdef_search (enc_cdef.c:925-1127).
#[allow(clippy::too_many_arguments)]
pub fn cdef_search_still_level7(
    recon_y: &[u8],
    recon_u: &[u8],
    recon_v: &[u8],
    src_y: &[u8],
    src_u: &[u8],
    src_v: &[u8],
    width: usize,
    height: usize,
    chroma_420: bool,
    geom: &DeblockGeom,
    qindex: u8,
) -> CdefSearchPick {
    assert!(width % 8 == 0 && height % 8 == 0, "8-aligned frames only");
    let damping = 3 + (qindex as i32 >> 6);
    let nvfb = height.div_ceil(64);
    let nhfb = width.div_ceil(64);
    let sub_y = 4usize; // BLOCK_8X8: min(subsampling_factor=4, 4)
    let sub_uv = 1usize; // BLOCK_4X4: min(4, 1)

    let mut mse: Vec<[[u64; 4]; 2]> = Vec::new();
    let mut src_pad = alloc::vec![0u16; k::CDEF_INBUF_SIZE];
    let mut tmp = alloc::vec![0u8; 64 * 64];
    let mut dlist: Vec<(usize, usize)> = Vec::with_capacity(64);

    for fbr in 0..nvfb {
        let vsize = 64.min(height - fbr * 64);
        for fbc in 0..nhfb {
            let hsize = 64.min(width - fbc * 64);
            dlist.clear();
            for by in 0..vsize / 8 {
                for bx in 0..hsize / 8 {
                    if !geom.is_8x8_all_skip(fbr * 16 + by * 2, fbc * 16 + bx * 2) {
                        dlist.push((by, bx));
                    }
                }
            }
            if dlist.is_empty() {
                continue;
            }
            let mut row = [[0u64; 4]; 2];
            let mut dir = [[0i32; 8]; 8];
            let mut var = [[0i32; 8]; 8];
            let mut dirinit = false;

            // ---- Luma
            build_src(
                &mut src_pad,
                recon_y,
                width,
                height,
                fbr * 64,
                fbc * 64,
                vsize,
                hsize,
            );
            for (gi, &fs) in L7_FS.iter().enumerate() {
                filter_fb_packed(
                    &mut tmp,
                    &src_pad,
                    &dlist,
                    fs,
                    damping,
                    0,
                    sub_y,
                    &mut dir,
                    &mut var,
                    &mut dirinit,
                );
                let d = dist_packed(&tmp, src_y, width, fbr * 64, fbc * 64, &dlist, true, sub_y);
                row[0][gi] = d * sub_y as u64;
            }

            // ---- Chroma: U then V ACCUMULATE into the joint uv row.
            if chroma_420 {
                let (cw, ch) = (width / 2, height / 2);
                for gi in 0..4 {
                    if !L7_UV_VALID[gi] {
                        row[1][gi] = DEFAULT_MSE_UV * 64;
                    }
                }
                for (rec_c, src_c) in [(recon_u, src_u), (recon_v, src_v)] {
                    build_src(
                        &mut src_pad,
                        rec_c,
                        cw,
                        ch,
                        fbr * 32,
                        fbc * 32,
                        vsize / 2,
                        hsize / 2,
                    );
                    for (gi, &fs) in L7_FS.iter().enumerate() {
                        if !L7_UV_VALID[gi] {
                            continue;
                        }
                        filter_fb_packed(
                            &mut tmp,
                            &src_pad,
                            &dlist,
                            fs,
                            damping,
                            1,
                            sub_uv,
                            &mut dir,
                            &mut var,
                            &mut dirinit,
                        );
                        let d =
                            dist_packed(&tmp, src_c, cw, fbr * 32, fbc * 32, &dlist, false, sub_uv);
                        row[1][gi] += d * sub_uv as u64;
                    }
                }
            } else {
                // Monochrome: the C search still runs pli 0 only; the uv
                // rows stay zero, making uv candidate 0 free like C's
                // num_planes=1 loop bound.
            }
            mse.push(row);
        }
    }

    if mse.is_empty() {
        return CdefSearchPick::AllSkip;
    }
    let (bits, y_idx, uv_idx) = finish_cdef_rd(&mse, qindex);
    if bits > 0 {
        return CdefSearchPick::MultiStrength;
    }
    CdefSearchPick::Picked(CdefFrameParams {
        damping: (3 + (qindex >> 6)) as u8,
        y_strength: L7_FS[y_idx] as u8,
        uv_strength: L7_FS[uv_idx] as u8,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// C-verified anchors (values cross-checked bit-exact against the C
    /// float evaluation for ALL qindexes by
    /// tests/c_parity_cdef_pick.rs; these pin representative points):
    /// q(255): ac=1828 -> y = 63 (pri 15, sec field 3), uv = 3 (uv_f1's
    /// quadratic goes negative and clamps to 0 — C's own fit);
    /// q(128): ac=176 -> y = 9, uv = 8; q(30): ac=37 -> all zero.
    #[test]
    fn strength_formula_anchors() {
        let p255 = pick_cdef_params_key_frame(255);
        assert_eq!(
            (p255.damping, p255.y_strength, p255.uv_strength),
            (6, 63, 3)
        );
        let p128 = pick_cdef_params_key_frame(128);
        assert_eq!((p128.damping, p128.y_strength, p128.uv_strength), (5, 9, 8));
        // Very low q: everything zero (CDEF off near-lossless).
        let p30 = pick_cdef_params_key_frame(30);
        assert_eq!((p30.y_strength, p30.uv_strength), (0, 0));
        assert_eq!(p30.damping, 3);
        assert!(!p30.any(true) && !p30.any(false));
    }

    /// The full recon-parity matrix qindexes {80,128,172,220,255} must all
    /// produce nonzero luma strengths (non-vacuous gate coverage; C-verified
    /// values y = 4/9/17/43/63) and legal field ranges everywhere.
    #[test]
    fn firing_profile_and_ranges() {
        for q in 0..=255u16 {
            let p = pick_cdef_params_key_frame(q as u8);
            assert!((3..=6).contains(&p.damping), "damping range at {q}");
            assert!(p.y_strength <= 63 && p.uv_strength <= 63);
        }
        // Zero below the knee (near-lossless protection)...
        assert_eq!(pick_cdef_params_key_frame(50).y_strength, 0);
        // ...firing across the entire gate matrix.
        for (q, want_y) in [(80u8, 4u8), (128, 9), (172, 17), (220, 43), (255, 63)] {
            assert_eq!(
                pick_cdef_params_key_frame(q).y_strength,
                want_y,
                "luma CDEF strength at qindex {q}"
            );
        }
        assert!(pick_cdef_params_key_frame(80).uv_strength != 0);
    }

    /// The finish_cdef_search RD pick pinned against the instrumented C
    /// captures (SVT_M6DBG CDEFMSE/CDEFBITS/CDEFPICK, gradient p6 cells,
    /// docs/IDENTITY-STATUS.md M6 chunk).
    #[test]
    fn finish_rd_matches_c_captures() {
        // g64 q55 (qindex 220): pick y search-index 2 (strength 2 =
        // pri 0 / sec 2), uv index 0, cdef_bits 0.
        let m55 = [[
            [885_020u64, 900_992, 875_920, 892_836],
            [0, 0, 66_585_600, 66_585_600],
        ]];
        assert_eq!(finish_cdef_rd(&m55, 220), (0, 2, 0));
        // g64 q40 (qindex 160): pick y index 3 (strength 62 = pri 15 /
        // sec 2).
        let m40 = [[
            [271_716u64, 257_812, 260_308, 251_848],
            [0, 0, 66_585_600, 66_585_600],
        ]];
        assert_eq!(finish_cdef_rd(&m40, 160), (0, 3, 0));
        // g128 q20 (qindex 80), 4 filter blocks: pick y index 2
        // (strength 2), uv 0, bits 0 (CDEFPICK y=[2]).
        let uvrow = [0u64, 0, 66_585_600, 66_585_600];
        let m20 = [
            [[51_440u64, 48_480, 47_460, 49_720], uvrow],
            [[49_580, 46_280, 45_176, 47_436], uvrow],
            [[52_756, 49_508, 48_144, 49_800], uvrow],
            [[52_028, 48_140, 46_548, 48_664], uvrow],
        ];
        assert_eq!(finish_cdef_rd(&m20, 80), (0, 2, 0));
    }

    /// Damping steps exactly at the C breakpoints.
    #[test]
    fn damping_from_qp_breakpoints() {
        assert_eq!(pick_cdef_params_key_frame(0).damping, 3);
        assert_eq!(pick_cdef_params_key_frame(63).damping, 3);
        assert_eq!(pick_cdef_params_key_frame(64).damping, 4);
        assert_eq!(pick_cdef_params_key_frame(127).damping, 4);
        assert_eq!(pick_cdef_params_key_frame(128).damping, 5);
        assert_eq!(pick_cdef_params_key_frame(191).damping, 5);
        assert_eq!(pick_cdef_params_key_frame(192).damping, 6);
    }
}
