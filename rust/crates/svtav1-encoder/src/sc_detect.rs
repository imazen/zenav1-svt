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
    debug_assert!((w == 8 && h == 8) || (w == 16 && h == 16));
    let mut sum: i64 = 0;
    let mut sse: u32 = 0;
    for r in 0..h {
        for c in 0..w {
            let diff = src[r * stride + c] as i32 - 128;
            sum += diff as i64;
            sse = sse.wrapping_add((diff * diff) as u32);
        }
    }
    let var = sse.wrapping_sub((sum * sum / (w as i64 * h as i64)) as u32);
    let log2pels = if w == 8 { 6 } else { 8 };
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
    /// M0 -> 3, M1 -> 4, M2 -> 5, M3 -> 6, M4 -> 7, M5+ -> 0). Recorded for
    /// the IBC vertical; `allow_intrabc` below stays false until the port
    /// codes IBC blocks — signaling a tool the tile never uses would be
    /// legal but C-divergent in a different way, and the FH intrabc bit
    /// also suppresses LF/CDEF/LR params (spec 5.9.11/19/20).
    pub intrabc_level: u8,
    /// FH bit. C: `pcs->intrabc_ctrls.enabled`. Port: false (see above) —
    /// M2-M4 sc_class5 cells stay divergent until the IBC vertical (#71).
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
            None => is_screen_content_antialiasing_aware(y, y_stride, width, height, fast_detection),
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
    let allow_intrabc = false; // IBC unported — see field doc
    ScDerivation {
        classes,
        palette_level,
        intrabc_level,
        allow_intrabc,
        allow_screen_content_tools: palette_level != 0 || allow_intrabc,
    }
}
