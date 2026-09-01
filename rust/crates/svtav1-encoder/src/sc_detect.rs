//! Screen-content detection — port of the AA-aware detector
//! (`svt_aom_is_screen_content_antialiasing_aware`,
//! pic_analysis_process.c:1207) plus its leaf primitives.
//!
//! This is the `--scm 3` detector: the allintra default for enc_mode <= M7
//! (enc_handle.c:4514-4527; TUNE_IQ forces it at every preset, :4738-4752).
//! It classifies the (padded, 8-bit) luma plane into palette / intrabc /
//! photo blocks over a 16x16 and an 8x8 grid and derives `sc_class0..5`;
//! `sc_class5` gates the whole allintra screen-content vertical (palette
//! level, intrabc level, allow_screen_content_tools, CDEF qp-strength,
//! depth refinement). Port map: docs/sc-detection-port-map.md.
//!
//! Bit-exactness notes (each traced in C):
//! - Input is the PADDED (multiple-of-8, edge-replicated) 8-bit luma plane;
//!   at 10-bit input C reads the 8-bit MSB plane (truncation, not rounding).
//! - Loop bounds are `r + blk_h <= height`: partial edge blocks are
//!   SKIPPED (after padding, only possible for the 16x16 pass when a
//!   dimension is an odd multiple of 8).
//! - `fast_detection` (enc_mode >= ENC_M3, enc_handle.c:4257) changes the
//!   VISITED SET (checkerboard: odd block-rows start at `blk_w`, step
//!   `2*blk_w`) and scales every counter x2 afterwards.
//! - `find_dominant_value` keeps the FIRST scan-order value to reach the
//!   max count (strict `>` compare) — ties do not replace.

/// C `svt_av1_count_colors_with_threshold` (pic_analysis_process.c:911).
/// Returns `(within_threshold, num_colors)`; on early exit (over the
/// threshold) `num_colors` is `threshold + 1` and the flag is `false`.
pub fn count_colors_with_threshold(
    src: &[u8],
    stride: usize,
    rows: usize,
    cols: usize,
    num_colors_threshold: i32,
) -> (bool, i32) {
    let mut has_color = [false; 256];
    let mut num_colors: i32 = 0;
    for r in 0..rows {
        for c in 0..cols {
            let v = src[r * stride + c] as usize;
            if !has_color[v] {
                has_color[v] = true;
                num_colors += 1;
                if num_colors > num_colors_threshold {
                    return (false, num_colors);
                }
            }
        }
    }
    (true, num_colors)
}

/// C `svt_av1_find_dominant_value` (pic_analysis_process.c:986): histogram
/// argmax with first-to-reach-max tie semantics (strict `>`).
pub fn find_dominant_value(src: &[u8], stride: usize, rows: usize, cols: usize) -> u8 {
    let mut value_count = [0u32; 256];
    let mut dominant_value_count = 0u32;
    let mut dominant_value = 0u8;
    for r in 0..rows {
        for c in 0..cols {
            let value = src[r * stride + c];
            let cnt = &mut value_count[value as usize];
            *cnt += 1;
            if *cnt > dominant_value_count {
                dominant_value = value;
                dominant_value_count = *cnt;
            }
        }
    }
    dominant_value
}

/// C `svt_av1_dilate_block` (pic_analysis_process.c:1024): copy the block,
/// then extend every ORIGINAL occurrence of the dominant value into its 8
/// neighbours (reads `src`, writes `dilated` — not iterative).
pub fn dilate_block(
    src: &[u8],
    src_stride: usize,
    dilated: &mut [u8],
    dilated_stride: usize,
    rows: usize,
    cols: usize,
) {
    let dominant_value = find_dominant_value(src, src_stride, rows, cols);
    for r in 0..rows {
        for c in 0..cols {
            dilated[r * dilated_stride + c] = src[r * src_stride + c];
        }
    }
    for r in 0..rows {
        for c in 0..cols {
            let value = src[r * src_stride + c];
            if value != dominant_value {
                continue;
            }
            let r0 = r > 0;
            let r1 = r != rows - 1;
            let c0 = c > 0;
            let c1 = c != cols - 1;
            if r0 {
                dilated[(r - 1) * dilated_stride + c] = value;
            }
            if r1 {
                dilated[(r + 1) * dilated_stride + c] = value;
            }
            if c0 {
                dilated[r * dilated_stride + (c - 1)] = value;
            }
            if c1 {
                dilated[r * dilated_stride + (c + 1)] = value;
            }
            if r0 && c0 {
                dilated[(r - 1) * dilated_stride + (c - 1)] = value;
            }
            if r0 && c1 {
                dilated[(r - 1) * dilated_stride + (c + 1)] = value;
            }
            if r1 && c0 {
                dilated[(r + 1) * dilated_stride + (c - 1)] = value;
            }
            if r1 && c1 {
                dilated[(r + 1) * dilated_stride + (c + 1)] = value;
            }
        }
    }
}

/// C `svt_av1_get_sby_perpixel_variance` (pic_analysis_process.c:944):
/// `fn_ptr->vf(src, stride, all-128 const buf, b_stride=0, &sse)` reduces
/// to plain block variance vs the constant 128 (variance_c,
/// C_DEFAULT/variance.c:141): `sse - (u32)((i64)sum*sum / (w*h))`, then
/// `ROUND_POWER_OF_TWO(var, log2pels)` (8x8 -> 6, 16x16 -> 8).
pub fn sby_perpixel_variance(src: &[u8], stride: usize, w: usize, h: usize) -> u32 {
    debug_assert!(w == h, "only the square detector block sizes are used");
    sby_perpixel_variance_normalized(src, stride, w, w)
}

/// The same C function with the variance WINDOW and the NORMALISING block
/// size chosen independently.
///
/// C passes both through one `AomVarianceFnPtr*` and one `BlockSize`, and they
/// are only the same when the caller keeps them in step — which
/// `svt_aom_is_screen_content` does not (see [`is_screen_content`]). Splitting
/// them here is what lets that call be expressed without a second copy of the
/// variance loop.
///
/// `side` is the window; `norm_side` picks the shift,
/// `eb_num_pels_log2_lookup[BLOCK_norm_side x norm_side]` (8 -> 6, 16 -> 8).
pub fn sby_perpixel_variance_normalized(
    src: &[u8],
    stride: usize,
    side: usize,
    norm_side: usize,
) -> u32 {
    debug_assert!(side == 8 || side == 16);
    debug_assert!(norm_side == 8 || norm_side == 16);
    let mut sum: i64 = 0;
    let mut sse: u32 = 0;
    for r in 0..side {
        for c in 0..side {
            let diff = src[r * stride + c] as i32 - 128;
            sum += diff as i64;
            sse = sse.wrapping_add((diff * diff) as u32);
        }
    }
    // C's `variance_c`: `sse - (uint32_t)((int64_t)sum * sum / (w * h))`,
    // divided by the WINDOW's pixel count, then rounded by the NORMALISER's.
    let var = sse.wrapping_sub((sum * sum / (side as i64 * side as i64)) as u32);
    let log2pels = if norm_side == 8 { 6 } else { 8 };
    (var + (1 << (log2pels - 1))) >> log2pels
}

/// One grid pass of `svt_aom_sc_AA_collect_counts`
/// (pic_analysis_process.c:1088).
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScAaCounts {
    pub count_photo: i64,
    pub count_palette: i64,
    pub count_intrabc: i64,
    pub region_palette: [i32; 4],
    pub region_intrabc: [i32; 4],
    pub region_photo: [i32; 4],
}

#[allow(clippy::too_many_arguments)]
pub fn sc_aa_collect_counts(
    y: &[u8],
    y_stride: usize,
    width: usize,
    height: usize,
    blk_w: usize,
    blk_h: usize,
    complex_initial_color_thresh: i32,
    simple_color_thresh: i32,
    complex_final_color_thresh: i32,
    var_thresh: u32,
    fast_detection: bool,
) -> ScAaCounts {
    let mut out = ScAaCounts::default();
    let multiplier: usize = if fast_detection { 2 } else { 1 };
    let mut dilated = alloc::vec![0u8; blk_w * blk_h];

    let mut r = 0usize;
    while r + blk_h <= height {
        let initial_col = if fast_detection && (r / blk_h) % 2 == 1 {
            blk_w
        } else {
            0
        };
        let mut c = initial_col;
        while c + blk_w <= width {
            let w2 = width >> 1;
            let h2 = height >> 1;
            let region_id = if r >= h2 { 2 } else { 0 } + if c >= w2 { 1 } else { 0 };
            let src = &y[r * y_stride + c..];

            let mut is_palette = false;
            let mut is_photo = false;
            let mut is_intrabc = false;

            let (ok, number_of_colors) = count_colors_with_threshold(
                src,
                y_stride,
                blk_h,
                blk_w,
                complex_initial_color_thresh,
            );
            if ok && number_of_colors > 1 {
                if number_of_colors <= simple_color_thresh {
                    is_palette = true;
                    let var = sby_perpixel_variance(src, y_stride, blk_w, blk_h);
                    if var > var_thresh {
                        is_intrabc = true;
                    }
                } else {
                    dilate_block(src, y_stride, &mut dilated, blk_w, blk_h, blk_w);
                    let (ok2, _) = count_colors_with_threshold(
                        &dilated,
                        blk_w,
                        blk_h,
                        blk_w,
                        complex_final_color_thresh,
                    );
                    if ok2 {
                        let var = sby_perpixel_variance(src, y_stride, blk_w, blk_h);
                        if var > var_thresh {
                            is_palette = true;
                            is_intrabc = true;
                        }
                    }
                }
            } else if number_of_colors > complex_initial_color_thresh {
                is_photo = true;
            }

            if is_palette {
                out.count_palette += 1;
                out.region_palette[region_id] += 1;
            }
            if is_intrabc {
                out.count_intrabc += 1;
                out.region_intrabc[region_id] += 1;
            }
            if is_photo {
                out.count_photo += 1;
                out.region_photo[region_id] += 1;
            }
            c += blk_w * multiplier;
        }
        r += blk_h;
    }

    if fast_detection {
        let m = multiplier as i64;
        out.count_photo *= m;
        out.count_palette *= m;
        out.count_intrabc *= m;
        for i in 0..4 {
            out.region_photo[i] *= multiplier as i32;
            out.region_palette[i] *= multiplier as i32;
            out.region_intrabc[i] *= multiplier as i32;
        }
    }
    out
}

/// The six frame-level screen-content classes.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScClasses {
    pub sc_class0: bool,
    pub sc_class1: bool,
    pub sc_class2: bool,
    pub sc_class3: bool,
    pub sc_class4: bool,
    pub sc_class5: bool,
}

/// C `svt_aom_is_screen_content_antialiasing_aware`
/// (pic_analysis_process.c:1207). `y` is the padded 8-bit luma plane;
/// `fast_detection` = `scs->fast_aa_aware_screen_detection_mode`
/// (enc_mode >= ENC_M3).
pub fn is_screen_content_antialiasing_aware(
    y: &[u8],
    y_stride: usize,
    width: usize,
    height: usize,
    fast_detection: bool,
) -> ScClasses {
    const BLK_AREA16: i64 = 256;
    const BLK_AREA8: i64 = 64;
    // Experimentally-selected C thresholds (pic_analysis_process.c:1228-1236).
    const SIMPLE_COLOR_THRESH: i32 = 4;
    const COMPLEX_INITIAL_COLOR_THRESH: i32 = 40;
    const COMPLEX_FINAL_COLOR_THRESH: i32 = 6;
    const VAR_THRESH: u32 = 5;
    // 8x8-pass-only (:1277-1278).
    const COMPLEX_FINAL_COLOR_THRESH_8: i32 = 8;
    const VAR_THRESH_8: u32 = 50;

    let area = width as i64 * height as i64;

    let c16 = sc_aa_collect_counts(
        y,
        y_stride,
        width,
        height,
        16,
        16,
        COMPLEX_INITIAL_COLOR_THRESH,
        SIMPLE_COLOR_THRESH,
        COMPLEX_FINAL_COLOR_THRESH,
        VAR_THRESH,
        fast_detection,
    );
    let c8 = sc_aa_collect_counts(
        y,
        y_stride,
        width,
        height,
        8,
        8,
        COMPLEX_INITIAL_COLOR_THRESH,
        SIMPLE_COLOR_THRESH,
        COMPLEX_FINAL_COLOR_THRESH_8,
        VAR_THRESH_8,
        fast_detection,
    );

    let mut out = ScClasses::default();
    // Photo-like blocks penalized at 1/16th the weight of a palettizable one.
    out.sc_class0 = (c16.count_palette - c16.count_photo / 16) * BLK_AREA16 * 10 > area;
    out.sc_class1 =
        out.sc_class0 && (c16.count_intrabc - c16.count_photo / 16) * BLK_AREA16 * 12 > area;
    out.sc_class2 = out.sc_class1
        || (c16.count_palette * BLK_AREA16 * 15 > area * 4
            && c16.count_intrabc * BLK_AREA16 * 30 > area);
    out.sc_class3 = out.sc_class1
        || (c16.count_palette * BLK_AREA16 * 8 > area
            && c16.count_intrabc * BLK_AREA16 * 50 > area);

    let region_area = area >> 2;
    let mut pass = 0;
    for i in 0..4 {
        if c8.region_palette[i] as i64 * BLK_AREA8 * 10 > region_area
            && c8.region_intrabc[i] as i64 * BLK_AREA8 * 25 > region_area
        {
            pass += 1;
        }
    }
    out.sc_class4 = pass >= 3 && c8.count_palette * BLK_AREA8 * 5 > area;
    out.sc_class5 = pass >= 3
        && c8.count_palette * BLK_AREA8 * 10 > area
        && c8.count_intrabc * BLK_AREA8 * 23 > area;
    out
}

/// Per-picture screen-content derivation for the allintra still path —
/// the detection slice of `svt_aom_sig_deriv_multi_processes_allintra`
/// (enc_mode_config.c:2337-2393) plus the scm-mode rule
/// (enc_handle.c:4514-4527).
#[derive(Default, Clone, Copy, Debug)]
pub struct ScDerivation {
    pub classes: ScClasses,
    /// C `pcs->palette_level` (enc_mode_config.c:2374-2390, sc_class5-gated:
    /// M0-M2 -> 2, M3 -> 3, M4-M5 -> 4, M6 -> 5, M7 -> 7, M8+ -> 0).
    pub palette_level: u8,
    /// C's intrabc level table value (:2346-2370, sc_class5-gated: MR -> 1,
    /// M0 -> 3, M1 -> 4, M2 -> 5, M3 -> 6, M4 -> 7, M5+ -> 0). Feeds
    /// `IbcCtrls::for_level` — the search controls AND the FH gate below.
    pub intrabc_level: u8,
    /// FH bit. C: `pcs->intrabc_ctrls.enabled` (enc_mode_config.c:2371) =
    /// `IbcCtrls::for_level(intrabc_level).enabled` (IBC chunk 1 flipped
    /// this live; it had been hardcoded false while IBC was unported).
    /// True on sc_class5 frames at presets <= 4. Setting it also
    /// suppresses the LF/CDEF/LR FH param blocks (spec 5.9.11/19/20,
    /// obu.rs) and kills the DLF/CDEF searches + LR execution
    /// (enc_mode_config.c:10118 / :2397; rest_process.c:262 — pipeline.rs).
    pub allow_intrabc: bool,
    /// FH bit. C: `(palette_level || allow_intrabc) ? 1 : 0` (:2393).
    pub allow_screen_content_tools: bool,
}

/// Edge-replicate a luma plane to multiples of 8 in both dimensions
/// (C `pad_picture_to_multiple_of_min_blk_size_dimensions` →
/// `pad_input_picture`, pic_operators.c:393; MIN_BLOCK_SIZE = 8). Returns
/// `None` when already aligned (use the original plane).
pub fn pad_to_multiple_of_8(
    y: &[u8],
    y_stride: usize,
    width: usize,
    height: usize,
) -> Option<(alloc::vec::Vec<u8>, usize, usize, usize)> {
    let pw = (width + 7) & !7;
    let ph = (height + 7) & !7;
    if pw == width && ph == height {
        return None;
    }
    let mut out = alloc::vec::Vec::with_capacity(pw * ph);
    for r in 0..ph {
        let sr = r.min(height - 1);
        let row = &y[sr * y_stride..sr * y_stride + width];
        out.extend_from_slice(row);
        let edge = row[width - 1];
        out.resize(out.len() + (pw - width), edge);
    }
    Some((out, pw, pw, ph))
}

/// `preset` is the still/allintra enc_mode. `y` is the SOURCE luma plane
/// (8-bit; the detector never sees the 10-bit LSBs — C reads the MSB
/// plane).
pub fn derive_allintra_sc(
    preset: u8,
    y: &[u8],
    y_stride: usize,
    width: usize,
    height: usize,
) -> ScDerivation {
    // scm mode (enc_handle.c:4514-4527): the CLI default (2) is overridden
    // for allintra — <= M7 auto-detects with the AA-aware detector (3),
    // M8+ forces detection off (0). (User-forced 0/1 and TUNE_IQ are not
    // exposed by this encoder's config surface yet.)
    let classes = if preset <= 7 {
        let fast_detection = preset >= 3; // enc_handle.c:4257
        match pad_to_multiple_of_8(y, y_stride, width, height) {
            Some((padded, ps, pw, ph)) => {
                is_screen_content_antialiasing_aware(&padded, ps, pw, ph, fast_detection)
            }
            None => {
                is_screen_content_antialiasing_aware(y, y_stride, width, height, fast_detection)
            }
        }
    } else {
        ScClasses::default()
    };

    let palette_level = if classes.sc_class5 {
        match preset {
            0..=2 => 2,
            3 => 3,
            4..=5 => 4,
            6 => 5,
            7 => 7,
            _ => 0,
        }
    } else {
        0
    };
    let intrabc_level = if classes.sc_class5 {
        match preset {
            0 => 3,
            1 => 4,
            2 => 5,
            3 => 6,
            4 => 7,
            _ => 0, // MR (=preset "-1") -> 1 is unreachable here
        }
    } else {
        0
    };
    // C: `set_intrabc_level(pcs, intrabc_level); frm_hdr->allow_intrabc =
    // pcs->intrabc_ctrls.enabled;` (enc_mode_config.c:2370-2371). The CLI
    // `enable_intrabc` toggle defaults ON (enc_settings.c:1065) and is not
    // exposed by this port's surface, so the level table above already
    // encodes the whole gate (sc_class5 && preset <= 4 => level != 0).
    let allow_intrabc = crate::intrabc::IbcCtrls::for_level(intrabc_level).enabled;
    ScDerivation {
        classes,
        palette_level,
        intrabc_level,
        allow_intrabc,
        allow_screen_content_tools: palette_level != 0 || allow_intrabc,
    }
}

// ---------------------------------------------------------------------------
// The `--scm 2` detector
// ---------------------------------------------------------------------------
//
// REACHABILITY — measured, and the answer is "never", which is why it is
// documented rather than assumed. `screen_content_mode` defaults to 2
// (enc_settings.c:1064), but `enc_handle.c:4638-4674` REMAPS it before the
// encoder ever reads it, on all three arms (allintra / rtc / else):
//
//     user <= 1                      -> passes through (0 or 1)
//     user >= 2 and enc_mode <= M7/M8 -> 3
//     otherwise                       -> 0
//
// The value 2 is never stored, so the `case 2:` arms in pd_process.c:4783 and
// pic_analysis_process.c:2018 — the only two callers of
// `svt_aom_is_screen_content` — are unreachable in v4.2.0 and the LIVE
// detector is always the AA-aware one above. Translated anyway per
// `docs/WORKING-ON-THIS.md` §7; one edit to that remap re-arms it, and it is
// gated at tier 1 so it will not rot.
//
// It differs from the AA-aware detector in more than thresholds: it has no
// dilation pass, no photo class, no per-region accounting, and it never writes
// `sc_class5` (which is the bit the whole allintra screen-content vertical
// hangs off, so a re-armed mode 2 would leave palette/IntraBC off).

/// C `is_valid_palette_nb_colors` (pic_analysis_process.c:957).
///
/// True when the block has more than one colour and no more than
/// `nb_colors_threshold` of them. Distinct from
/// [`count_colors_with_threshold`], which reports "not over the threshold"
/// and does not reject a single-colour block.
pub fn is_valid_palette_nb_colors(
    src: &[u8],
    stride: usize,
    rows: usize,
    cols: usize,
    nb_colors_threshold: i32,
) -> bool {
    let mut has_color = [false; 256];
    let mut nb_colors: i32 = 0;
    for r in 0..rows {
        for c in 0..cols {
            let v = src[r * stride + c] as usize;
            if !has_color[v] {
                has_color[v] = true;
                nb_colors += 1;
                if nb_colors > nb_colors_threshold {
                    return false;
                }
            }
        }
    }
    nb_colors > 1
}

/// One grid pass of [`is_screen_content`]: count the blocks that palettize,
/// and of those, the ones whose per-pixel variance clears `var_thresh`.
///
/// `var_side` is the side of the VARIANCE window, which is not always `blk`:
/// see the note on [`is_screen_content`] about C's un-rebound `fn_ptr`.
#[allow(clippy::too_many_arguments)]
fn scm2_counts(
    y: &[u8],
    y_stride: usize,
    width: usize,
    height: usize,
    blk: usize,
    var_side: usize,
    color_thresh: i32,
    var_thresh: u32,
) -> (i64, i64) {
    let (mut counts_1, mut counts_2) = (0i64, 0i64);
    let mut r = 0;
    while r + blk <= height {
        let mut c = 0;
        while c + blk <= width {
            let src = &y[r * y_stride + c..];
            if is_valid_palette_nb_colors(src, y_stride, blk, blk, color_thresh) {
                counts_1 += 1;
                if sby_perpixel_variance_normalized(src, y_stride, var_side, blk) > var_thresh {
                    counts_2 += 1;
                }
            }
            c += blk;
        }
        r += blk;
    }
    (counts_1, counts_2)
}

/// C `svt_aom_is_screen_content` (pic_analysis_process.c:1355) — the
/// `--scm 2` detector. See the reachability note above: v4.2.0 never selects
/// it.
///
/// `y` is the padded 8-bit luma plane and must carry EIGHT extra rows and
/// columns past `width` x `height` — see the `fn_ptr` note below. `sc_class5`
/// is left `false` because C never assigns it here.
///
/// ## Two upstream defects reproduced on purpose
///
/// 1. **The 8x8 pass measures a 16x16 variance.** C binds
///    `const AomVarianceFnPtr* fn_ptr = &svt_aom_mefn_ptr[BLOCK_16X16]` at
///    :1367 and never rebinds it, then passes that same `fn_ptr` to
///    `svt_av1_get_sby_perpixel_variance(..., BLOCK_8X8)` at :1417. So the 8x8
///    pass reads a 16x16 window through `variance16x16` and normalises the
///    result by 64 instead of 256 — a per-pixel variance four times too large,
///    over a window four times too big, straddling three neighbouring blocks.
///    That is also where the 8-row/8-column read past the frame comes from.
///    Confirmed at tier 1: `sby_perpixel_variance_mixed(.., Blk16x16, Blk8x8)`
///    against the real C symbol reproduces it exactly, and the whole-detector
///    differential agrees on every plane in the suite once it is reproduced.
///    Found by the differential, not by reading — the port was written with
///    matched block sizes first and disagreed with C on a mixed plane.
/// 2. **Width and height are swapped** at both call sites:
///    `is_valid_palette_nb_colors(src, stride, blk_w, blk_h, thresh)` against
///    parameters `(.., rows, cols, ..)`. Both passes are square, so it is
///    inert; it is spelled out because a future non-square pass would not be.
///
/// Per `docs/WORKING-ON-THIS.md` §7 a C bug is still the oracle. Recorded in
/// `docs/SUSPECTED-C-BUGS.md`.
pub fn is_screen_content(y: &[u8], y_stride: usize, width: usize, height: usize) -> ScClasses {
    assert!(
        width + 8 <= y_stride && y.len() >= (height + 7) * y_stride + width + 8,
        "the scm-2 detector reads 8 rows/cols past {width}x{height}; supply the border"
    );
    let area = width as i64 * height as i64;

    // 16x16 pass: colour threshold 4, variance threshold 0 (:1358-1362).
    let (counts_1, counts_2) = scm2_counts(y, y_stride, width, height, 16, 16, 4, 0);
    let blk_area16: i64 = 16 * 16;

    let mut out = ScClasses {
        sc_class0: counts_1 * blk_area16 * 10 > area,
        ..Default::default()
    };
    // IntraBC forces the loop filters off, so it takes the stricter rule.
    out.sc_class1 = out.sc_class0 && counts_2 * blk_area16 * 12 > area;
    out.sc_class2 = out.sc_class1
        || (counts_1 * blk_area16 * 10 > area * 4 && counts_2 * blk_area16 * 30 > area);
    out.sc_class3 =
        out.sc_class1 || (counts_1 * blk_area16 * 8 > area && counts_2 * blk_area16 * 50 > area);

    // 8x8 pass: same colour threshold, variance threshold 16 (:1400-1408) —
    // and the 16x16 variance window C's un-rebound `fn_ptr` forces.
    let (counts_1, counts_2) = scm2_counts(y, y_stride, width, height, 8, 16, 4, 16);
    let blk_area8: i64 = 8 * 8;
    out.sc_class4 = counts_1 * blk_area8 * 18 > area && counts_2 * blk_area8 * 20 > area;
    out
}
