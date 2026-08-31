//! Wholesale port of `Source/Lib/Codec/pic_analysis_process.c` — the
//! pre-analysis picture operations that run BEFORE motion estimation.
//!
//! Why this file exists: the port consumes quarter/sixteenth luma planes in
//! `inter_me::hme` but nothing in the port ever *produced* them — there was no
//! downsampler in the tree at all (2026-08-31). Every HME MV was therefore
//! searched against pixels the port cannot build. This module ports the
//! producer chain end to end:
//!
//! ```text
//!   svt_aom_pad_input_pictures            (min-blk pad, then the 68px border)
//!     -> svt_aom_downsample_filtering_input_picture   (1/4 and 1/16 luma)
//!          -> svt_aom_downsample_2d_c  +  svt_aom_generate_padding
//!   svt_aom_gathering_picture_statistics  (the calc_hist / calculate_variance gates)
//!     -> sub_sample_luma_generate_pixel_intensity_histogram_bins
//!          -> calculate_histogram
//!   svt_aom_is_input_luma_dominant        (live only when !allintra)
//! ```
//!
//! ## Plane representation
//!
//! C works on a pointer that sits in the MIDDLE of an allocation: the active
//! picture origin, with `border` pixels of slack on every side. The padding
//! routines write at NEGATIVE offsets from it. A Rust `&mut [u8]` starting at
//! the origin cannot express that, so every routine that pads takes the whole
//! buffer plus an explicit `origin` byte offset — the exact C pointer
//! arithmetic, bounds-checked. Routines that only ever read/write forward
//! (`downsample_2d`, `calculate_histogram`) take a slice starting at the
//! origin, because C never steps behind it there (`decim_step` is even, so
//! `half_decim_step >= 1` and `input_samples - input_stride` is still inside
//! the active area).
//!
//! Evidence: every function here is gated at tier 1
//! (`docs/WORKING-ON-THIS.md` §4) against the real exported C symbol through
//! `svtav1-cref` — see `tests/c_parity_preanalysis.rs`.

/// `svt_aom_downsample_2d_c` (pic_analysis_process.c:134).
///
/// 2x2 0-phase averaging decimator: `(a + b + c + d + 2) >> 2` over the pair
/// of samples at `(x-1, y-1) .. (x, y)`, sampled on a `decim_step` lattice
/// starting at `half_decim_step`. Note it reads the row ABOVE and the column
/// LEFT of the lattice point — that is what "0-phase" means here, and getting
/// it wrong shifts the whole downsampled plane by half a pixel.
///
/// `input` starts at the C `input_samples` pointer; `decim` at `decim_samples`.
/// The first row C touches is `half_decim_step - 1` and the first column is
/// `half_decim_step - 1`, both >= 0 for every even `decim_step` (the only
/// values the encoder uses are 2 and 4), so no negative indexing is possible.
pub fn downsample_2d(
    input: &[u8],
    input_stride: usize,
    input_area_width: usize,
    input_area_height: usize,
    decim: &mut [u8],
    decim_stride: usize,
    decim_step: usize,
) {
    assert!(decim_step >= 2, "decim_step must be at least 2");
    let half = decim_step >> 1;
    assert!(half >= 1, "decim_step must be even (half-step underflows)");

    let mut out_row = 0usize;
    let mut vertical_index = half;
    while vertical_index < input_area_height {
        // C: `prev_input_line = input_samples - input_stride`, where
        // `input_samples` sits on row `vertical_index`.
        let cur = vertical_index * input_stride;
        let prev = cur - input_stride;
        let dst = out_row * decim_stride;

        let mut horizontal_index = half;
        let mut decim_horizontal_index = 0usize;
        while horizontal_index < input_area_width {
            let sum = u32::from(input[prev + horizontal_index - 1])
                + u32::from(input[prev + horizontal_index])
                + u32::from(input[cur + horizontal_index - 1])
                + u32::from(input[cur + horizontal_index]);
            decim[dst + decim_horizontal_index] = ((sum + 2) >> 2) as u8;
            horizontal_index += decim_step;
            decim_horizontal_index += 1;
        }
        vertical_index += decim_step;
        out_row += 1;
    }
}

/// `calculate_histogram` (pic_analysis_process.c:170).
///
/// n-bin histogram over a `decim_step` lattice. `histogram` is indexed by the
/// raw 8-bit sample so it must hold 256 entries; C's caller pre-seeds every
/// one of them to 1 via `svt_initialize_buffer_32bits(p, 64, 0, 1)` — that
/// `64` is a count of 128-BIT GROUPS (`count128 * 4 + count32` u32 slots =
/// 256), not a bin count. Reading it as 64 bins is an easy and wrong
/// conclusion; `sub_sample_luma_generate_pixel_intensity_histogram_bins` below
/// seeds all 256.
///
/// `sum` ACCUMULATES (C takes `uint64_t*` and does `*sum += ...` without
/// zeroing), so the caller owns initialisation.
pub fn calculate_histogram(
    input_samples: &[u8],
    input_area_width: usize,
    input_area_height: usize,
    stride: usize,
    decim_step: usize,
    histogram: &mut [u32; 256],
    sum: &mut u64,
) {
    assert!(decim_step >= 1);
    let mut row_base = 0usize;
    let mut vertical_index = 0usize;
    while vertical_index < input_area_height {
        let mut horizontal_index = 0usize;
        while horizontal_index < input_area_width {
            let v = input_samples[row_base + horizontal_index];
            histogram[usize::from(v)] += 1;
            *sum += u64::from(v);
            horizontal_index += decim_step;
        }
        row_base += stride * decim_step;
        vertical_index += decim_step;
    }
}

/// `svt_aom_generate_padding` (pic_operators.c:434).
///
/// Horizontal edge-replicate first, then vertical replicate of the ALREADY
/// horizontally padded top and bottom rows.
///
/// Two faithfulness details that a "reasonable" implementation gets wrong:
/// * the vertical copy length is `src_stride`, not `width + 2 * pad_width` —
///   so it copies whatever trails the right padding out to the end of the
///   stride as well;
/// * the vertical copy starts at `src_pic - padding_width`, i.e. it carries
///   the left padding it just wrote.
///
/// `buf` is the whole allocation and `origin` is the byte offset of the C
/// `src_pic` pointer within it.
pub fn generate_padding(
    buf: &mut [u8],
    origin: usize,
    src_stride: usize,
    original_src_width: usize,
    original_src_height: usize,
    padding_width: usize,
    padding_height: usize,
) {
    assert!(original_src_width > 0 && original_src_height > 0);
    let row_bytes = src_stride;

    // Horizontal padding: extend each active row left and right.
    for y in 0..original_src_height {
        let row = origin + y * src_stride;
        let left_pixel = buf[row];
        let right_pixel = buf[row + original_src_width - 1];
        buf[row - padding_width..row].fill(left_pixel);
        buf[row + original_src_width..row + original_src_width + padding_width].fill(right_pixel);
    }

    // Vertical padding: replicate the fully padded first and last rows.
    let top_src_row = origin - padding_width;
    let bottom_src_row = origin - padding_width + (original_src_height - 1) * src_stride;
    for y in 0..padding_height {
        let top_dst_row = top_src_row - (y + 1) * src_stride;
        let bottom_dst_row = bottom_src_row + (y + 1) * src_stride;
        buf.copy_within(top_src_row..top_src_row + row_bytes, top_dst_row);
        buf.copy_within(bottom_src_row..bottom_src_row + row_bytes, bottom_dst_row);
    }
}

/// `pad_input_picture` (pic_operators.c:561).
///
/// Right-then-bottom padding used to reach a multiple of the minimum block
/// size. Unlike `generate_padding` this only ever writes FORWARD of the
/// origin, so it takes a slice starting there.
///
/// The bottom copy length is `original_src_width + pad_right` (the row as
/// widened by the right pass), not the stride.
pub fn pad_input_picture(
    src: &mut [u8],
    src_stride: usize,
    original_src_width: usize,
    original_src_height: usize,
    pad_right: usize,
    pad_bottom: usize,
) {
    if pad_right > 0 {
        for y in 0..original_src_height {
            let row = y * src_stride;
            let last = src[row + original_src_width - 1];
            src[row + original_src_width..row + original_src_width + pad_right].fill(last);
        }
    }
    if pad_bottom > 0 {
        let last_row = (original_src_height - 1) * src_stride;
        let len = original_src_width + pad_right;
        for y in 0..pad_bottom {
            let dst = last_row + (y + 1) * src_stride;
            src.copy_within(last_row..last_row + len, dst);
        }
    }
}

// ---------------------------------------------------------------------------
// svt_aom_is_input_luma_dominant (pic_analysis_process.c:1441)
// ---------------------------------------------------------------------------

/// Sample lattice step over the CHROMA planes.
const FRAME_LUMA_DOMINANT_SAMPLE_STEP: usize = 8;
const FRAME_LUMA_DOMINANT_CORE_THR: u32 = 16;
const FRAME_LUMA_DOMINANT_TAIL_THR: u32 = 18;
const FRAME_LUMA_DOMINANT_MIN_CORE_PCT: u32 = 85;
const FRAME_LUMA_DOMINANT_MIN_TAIL_PCT: u32 = 95;
const FRAME_LUMA_DOMINANT_NEUTRAL_THR: i32 = 6;
const FRAME_LUMA_DOMINANT_UV_DIFF_THR: i32 = 4;
const FRAME_LUMA_DOMINANT_MIN_NEUTRAL_PCT: u32 = 75;

/// `svt_aom_is_input_luma_dominant` (pic_analysis_process.c:1441).
///
/// True when the frame's chroma sits close enough to neutral that the encoder
/// treats it as luma-dominant. `scs->detect_luma_dominant_input` is set
/// exactly when `!allintra` (enc_handle.c:4367-4371), so this is DEAD on the
/// still path and live for every video-mode frame; the flag feeds LPD1 tx-skip
/// scoring (product_coding_loop.c:6402-6406), so a wrong answer changes coded
/// blocks.
///
/// Faithfulness notes:
/// * the chroma dimensions are `width >> 1` / `height >> 1` UNCONDITIONALLY —
///   C does not consult `subsampling_x/y` here, it only rejects `EB_YUV400`;
/// * the two threshold comparisons are on the SQUARED magnitude against the
///   squared threshold, and both counters are incremented independently (a
///   sample inside the core is also inside the tail);
/// * the final predicate is `core AND (tail OR neutral)`, not `core AND tail`.
///
/// `u_plane`/`v_plane` start at their plane origins. `color_format` uses C's
/// `EbColorFormat` numbering (`EB_YUV400 = 0`).
pub fn is_input_luma_dominant(
    color_format: u32,
    width: usize,
    height: usize,
    u_plane: &[u8],
    u_stride: usize,
    v_plane: &[u8],
    v_stride: usize,
) -> bool {
    // EB_YUV400 == 0; C also rejects null u/v buffers, which an empty slice
    // stands in for here.
    if color_format == 0 || u_plane.is_empty() || v_plane.is_empty() {
        return false;
    }

    let uv_w = width >> 1;
    let uv_h = height >> 1;
    if uv_w == 0 || uv_h == 0 {
        return false;
    }

    let mut sample_cnt = 0u32;
    let mut core_cnt = 0u32;
    let mut tail_cnt = 0u32;
    let mut neutral_cnt = 0u32;

    let core_thr_sq = FRAME_LUMA_DOMINANT_CORE_THR * FRAME_LUMA_DOMINANT_CORE_THR;
    let tail_thr_sq = FRAME_LUMA_DOMINANT_TAIL_THR * FRAME_LUMA_DOMINANT_TAIL_THR;

    let mut y = 0usize;
    while y < uv_h {
        let ub = y * u_stride;
        let vb = y * v_stride;
        let mut x = 0usize;
        while x < uv_w {
            let du = i32::from(u_plane[ub + x]) - 128;
            let dv = i32::from(v_plane[vb + x]) - 128;
            let uv = i32::from(u_plane[ub + x]) - i32::from(v_plane[vb + x]);
            let chroma_mag_sq = (du * du + dv * dv) as u32;

            if chroma_mag_sq <= core_thr_sq {
                core_cnt += 1;
            }
            if chroma_mag_sq <= tail_thr_sq {
                tail_cnt += 1;
            }
            if du.abs() <= FRAME_LUMA_DOMINANT_NEUTRAL_THR
                && dv.abs() <= FRAME_LUMA_DOMINANT_NEUTRAL_THR
                && uv.abs() <= FRAME_LUMA_DOMINANT_UV_DIFF_THR
            {
                neutral_cnt += 1;
            }

            sample_cnt += 1;
            x += FRAME_LUMA_DOMINANT_SAMPLE_STEP;
        }
        y += FRAME_LUMA_DOMINANT_SAMPLE_STEP;
    }

    sample_cnt != 0
        && core_cnt * 100 >= sample_cnt * FRAME_LUMA_DOMINANT_MIN_CORE_PCT
        && (tail_cnt * 100 >= sample_cnt * FRAME_LUMA_DOMINANT_MIN_TAIL_PCT
            || neutral_cnt * 100 >= sample_cnt * FRAME_LUMA_DOMINANT_MIN_NEUTRAL_PCT)
}

// ---------------------------------------------------------------------------
// The picture drivers: pic_analysis_process.c:1499 / :1550 / :746
// ---------------------------------------------------------------------------

/// One luma/chroma plane as C sees it: an allocation with the active picture
/// at `origin` and `border` pixels of slack on every side.
///
/// This mirrors the `EbPictureBufferDesc` fields the pre-analysis functions
/// actually read — nothing more. `origin` is the byte offset of C's
/// `*_buffer` pointer within `buf`.
pub struct Plane<'a> {
    pub buf: &'a mut [u8],
    pub origin: usize,
    pub stride: usize,
    pub width: usize,
    pub height: usize,
    pub border: usize,
}

/// The six HME enables `svt_aom_downsample_filtering_input_picture` reads off
/// the `PictureParentControlSet`.
///
/// MEASURED (enc_mode_config.c:1987-1999): the encoder sets
/// `enable_hme_flag = enable_hme_level0_flag = enable_hme_level1_flag = 1`
/// unconditionally, so the LIVE route is 2x-then-2x — the sixteenth plane is
/// built from the QUARTER plane, never by the direct 4x arm. The two routes
/// give different sixteenth pixels; porting only the direct-4x arm would
/// produce plausible-looking planes and wrong MVs everywhere.
#[derive(Debug, Clone, Copy, Default)]
pub struct HmeEnables {
    pub enable_hme: bool,
    pub tf_enable_hme: bool,
    pub enable_hme_level0: bool,
    pub tf_enable_hme_level0: bool,
    pub enable_hme_level1: bool,
    pub tf_enable_hme_level1: bool,
}

/// `svt_aom_downsample_filtering_input_picture` (pic_analysis_process.c:1499).
///
/// Fills the 1/4 and 1/16 luma planes HME level 1 / level 0 search, padding
/// each to its own border afterwards. Called only when `!scs->allintra`, i.e.
/// it is newly live exactly in video mode.
///
/// Note the level-1 gate gets checked TWICE: once to decide whether to build
/// the quarter plane at all, and again inside the level-0 branch to decide
/// which source the sixteenth plane comes from.
pub fn downsample_filtering_input_picture(
    flags: &HmeEnables,
    input_padded: &Plane<'_>,
    quarter: &mut Plane<'_>,
    sixteenth: &mut Plane<'_>,
) {
    if !(flags.enable_hme || flags.tf_enable_hme) {
        return;
    }
    let level1 = flags.enable_hme_level1 || flags.tf_enable_hme_level1;

    if level1 {
        downsample_2d(
            &input_padded.buf[input_padded.origin..],
            input_padded.stride,
            input_padded.width,
            input_padded.height,
            &mut quarter.buf[quarter.origin..],
            quarter.stride,
            2,
        );
        generate_padding(
            quarter.buf,
            quarter.origin,
            quarter.stride,
            quarter.width,
            quarter.height,
            quarter.border,
            quarter.border,
        );
    }

    if flags.enable_hme_level0 || flags.tf_enable_hme_level0 {
        if level1 {
            // The LIVE route: 2x of the already-decimated quarter plane.
            let q_origin = quarter.origin;
            let q_stride = quarter.stride;
            let q_w = quarter.width;
            let q_h = quarter.height;
            downsample_2d(
                &quarter.buf[q_origin..],
                q_stride,
                q_w,
                q_h,
                &mut sixteenth.buf[sixteenth.origin..],
                sixteenth.stride,
                2,
            );
        } else {
            downsample_2d(
                &input_padded.buf[input_padded.origin..],
                input_padded.stride,
                input_padded.width,
                input_padded.height,
                &mut sixteenth.buf[sixteenth.origin..],
                sixteenth.stride,
                4,
            );
        }
        generate_padding(
            sixteenth.buf,
            sixteenth.origin,
            sixteenth.stride,
            sixteenth.width,
            sixteenth.height,
            sixteenth.border,
            sixteenth.border,
        );
    }
}

/// `svt_aom_pad_picture_to_multiple_of_min_blk_size_dimensions`
/// (pic_analysis_process.c:746), 8-bit planes only.
///
/// Right/bottom edge-replicate so a non-8-aligned input (the C comment's
/// example is 426x240 -> 432x240) reaches a multiple of the minimum block
/// size. `pad_right` / `pad_bottom` come from the SequenceControlSet.
///
/// Faithfulness trap: the chroma subsampling here is derived from
/// `input_pic->color_format`, NOT from `scs->subsampling_x/y` — the sibling
/// `pad_input_pictures` below uses the scs fields for the same purpose. The
/// two agree for 4:2:0 but the source of truth differs, so transcribe each
/// from its own site.
///
/// The chroma active dimensions round UP:
/// `(width + subsampling_x - pad_right) >> subsampling_x`.
pub fn pad_picture_to_multiple_of_min_blk_size_dimensions<'p>(
    color_format: u32,
    pad_right: usize,
    pad_bottom: usize,
    y: &mut Plane<'_>,
    u: Option<&mut Plane<'p>>,
    v: Option<&mut Plane<'p>>,
) {
    // EB_YUV444 == 3, EB_YUV422 == 2.
    let subsampling_x = usize::from(color_format != 3);
    let subsampling_y = usize::from(color_format < 2);

    // C reads `input_pic->width` / `->height` for BOTH luma and chroma: those
    // fields hold the LUMA dimensions, and the chroma sizes are derived from
    // them by the shift below. A chroma plane's own width/height is never
    // consulted — using it here is a silent wrong-size pad.
    let luma_w = y.width;
    let luma_h = y.height;
    let y_origin = y.origin;
    let y_stride = y.stride;
    pad_input_picture(
        &mut y.buf[y_origin..],
        y_stride,
        luma_w - pad_right,
        luma_h - pad_bottom,
        pad_right,
        pad_bottom,
    );

    for plane in [u, v].into_iter().flatten() {
        let origin = plane.origin;
        let stride = plane.stride;
        let w = (luma_w + subsampling_x - pad_right) >> subsampling_x;
        let h = (luma_h + subsampling_y - pad_bottom) >> subsampling_y;
        pad_input_picture(
            &mut plane.buf[origin..],
            stride,
            w,
            h,
            pad_right >> subsampling_x,
            pad_bottom >> subsampling_y,
        );
    }
}

/// `svt_aom_pad_input_pictures` (pic_analysis_process.c:1550), 8-bit path.
///
/// Min-block padding first, then the full border edge-replicate (68 px for
/// luma) across Y/U/V. The border is what ME/HME reads when a search window
/// leaves the frame — the port's `frame_geom::pad_input_plane` reproduces only
/// the SB-extent replicate, which is a strict subset sized for the still-path
/// variance walk.
///
/// The chroma planes are padded with `scs->subsampling_x/y`, applied to
/// `input_pic->width/height` (the LUMA dimensions) and to `input_pic->border`.
/// The C comment records why this is safe: the picture is already 8px aligned
/// by the call above, so `>> 1` cannot lose a pixel.
///
/// 10-bit `*_bit_inc` compressed-plane padding is NOT ported here; it is the
/// `svt_aom_generate_padding_compressed_10bit` path and belongs with the
/// bd10 chunk.
pub fn pad_input_pictures<'p>(
    subsampling_x: usize,
    subsampling_y: usize,
    pad_right: usize,
    pad_bottom: usize,
    color_format: u32,
    y: &mut Plane<'_>,
    mut u: Option<&mut Plane<'p>>,
    mut v: Option<&mut Plane<'p>>,
) {
    pad_picture_to_multiple_of_min_blk_size_dimensions(
        color_format,
        pad_right,
        pad_bottom,
        y,
        u.as_deref_mut(),
        v.as_deref_mut(),
    );

    generate_padding(
        y.buf, y.origin, y.stride, y.width, y.height, y.border, y.border,
    );

    // Chroma uses the LUMA width/height/border shifted down — not the chroma
    // plane's own `width`/`height` fields.
    let cw = y.width >> subsampling_x;
    let ch = y.height >> subsampling_y;
    let cbx = y.border >> subsampling_x;
    let cby = y.border >> subsampling_y;
    for plane in [u, v].into_iter().flatten() {
        generate_padding(plane.buf, plane.origin, plane.stride, cw, ch, cbx, cby);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 0-phase lattice: with `decim_step == 2` output `(0,0)` averages the
    /// input's top-left 2x2 block. Hand-derived from the C source (this is a
    /// shape check; the bit-exact gate is `tests/c_parity_preanalysis.rs`).
    #[test]
    fn downsample_2d_step2_averages_top_left_quad() {
        let input: Vec<u8> = (0..16u8).collect(); // 4x4, stride 4
        let mut out = vec![0u8; 4];
        downsample_2d(&input, 4, 4, 4, &mut out, 2, 2);
        // (0 + 1 + 4 + 5 + 2) >> 2 == 3
        assert_eq!(out[0], 3);
        // (2 + 3 + 6 + 7 + 2) >> 2 == 5
        assert_eq!(out[1], 5);
        // (8 + 9 + 12 + 13 + 2) >> 2 == 11
        assert_eq!(out[2], 11);
        assert_eq!(out[3], 13);
    }

    #[test]
    fn histogram_sum_accumulates_into_caller_value() {
        let input = vec![7u8; 16];
        let mut hist = [0u32; 256];
        let mut sum = 100u64;
        calculate_histogram(&input, 4, 4, 4, 1, &mut hist, &mut sum);
        assert_eq!(hist[7], 16);
        assert_eq!(sum, 100 + 7 * 16);
    }

    #[test]
    fn generate_padding_replicates_edges() {
        // 4x2 active area inside a 2-pixel border, stride 8.
        let stride = 8usize;
        let border = 2usize;
        let w = 4usize;
        let h = 2usize;
        let mut buf = vec![0u8; stride * (h + 2 * border)];
        let origin = border * stride + border;
        for y in 0..h {
            for x in 0..w {
                buf[origin + y * stride + x] = (y * 4 + x + 1) as u8;
            }
        }
        generate_padding(&mut buf, origin, stride, w, h, border, border);
        // Left/right replicate on row 0: 1 1 | 1 2 3 4 | 4 4
        assert_eq!(&buf[origin - 2..origin + w + 2], &[1, 1, 1, 2, 3, 4, 4, 4]);
        // Top rows are copies of the padded row 0, starting at origin-border.
        let top = origin - border - stride;
        assert_eq!(
            &buf[top..top + 8],
            &buf[origin - border..origin - border + 8]
        );
    }

    #[test]
    fn pad_input_picture_extends_right_then_bottom() {
        let stride = 6usize;
        let mut buf = vec![0u8; stride * 4];
        // 4x2 active, pad to 6x4.
        for y in 0..2 {
            for x in 0..4 {
                buf[y * stride + x] = (y * 4 + x + 1) as u8;
            }
        }
        pad_input_picture(&mut buf, stride, 4, 2, 2, 2);
        assert_eq!(&buf[0..6], &[1, 2, 3, 4, 4, 4]);
        assert_eq!(&buf[stride..stride + 6], &[5, 6, 7, 8, 8, 8]);
        assert_eq!(&buf[2 * stride..2 * stride + 6], &[5, 6, 7, 8, 8, 8]);
        assert_eq!(&buf[3 * stride..3 * stride + 6], &[5, 6, 7, 8, 8, 8]);
    }
}
