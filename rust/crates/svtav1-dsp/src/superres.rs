//! Super-resolution — the AV1 NORMATIVE horizontal upscale (spec 7.16).
//!
//! Port of SVT-AV1 `super_res.c`: the 64-phase `svt_av1_resize_filter_normative`
//! table, `av1_get_upscale_convolve_step` / `get_upscale_convolve_x0`, the
//! `av1_convolve_horiz_rs_c` kernel, and the `upscale_normative_rect` border
//! handling. Superres encodes a frame at a reduced horizontal width
//! `coded_w = frame_w * 8 / denom` and the DECODER upscales it back with this
//! exact filter — so it is normative: an approximation here is a decode
//! mismatch, not a quality trade. (This module was a 16-phase Q14 stub until
//! the task #6/superres chunk-A port; `tests/c_parity_superres.rs` pins every
//! phase and a denominator sweep against the real C.)
//!
//! Scope: the kernel + geometry. The ENCODER side (denominator selection,
//! source downscale, encoding at `coded_w`, recon upscale before loop
//! restoration, `superres_params()` header syntax) is chunk B — see
//! `rust/docs/superres-port-map.md`. Superres stays OFF by default in C
//! (`superres_mode = SUPERRES_NONE`, enc_settings.c:1095) and in this port.

/// C `RS_SUBPEL_BITS` — 6, i.e. `1 << 6 = 64` filter phases.
pub const RS_SUBPEL_BITS: u32 = 6;
/// C `RS_SUBPEL_MASK`.
pub const RS_SUBPEL_MASK: i32 = (1 << RS_SUBPEL_BITS) - 1;
/// C `RS_SCALE_SUBPEL_BITS` — the 14-bit fixed-point domain the source
/// position accumulator lives in.
pub const RS_SCALE_SUBPEL_BITS: u32 = 14;
/// C `RS_SCALE_SUBPEL_MASK`.
pub const RS_SCALE_SUBPEL_MASK: i32 = (1 << RS_SCALE_SUBPEL_BITS) - 1;
/// C `RS_SCALE_EXTRA_BITS` — the shift from the 14-bit accumulator down to a
/// 6-bit phase index.
pub const RS_SCALE_EXTRA_BITS: u32 = RS_SCALE_SUBPEL_BITS - RS_SUBPEL_BITS;
/// C `RS_SCALE_EXTRA_OFF` — the half-step rounding offset baked into `x0`.
pub const RS_SCALE_EXTRA_OFF: i32 = 1 << (RS_SCALE_EXTRA_BITS - 1);
/// C `UPSCALE_NORMATIVE_TAPS`.
pub const UPSCALE_NORMATIVE_TAPS: usize = 8;
/// C `SCALE_NUMERATOR` — superres numerator; `denom` ranges 9..=16.
pub const SCALE_NUMERATOR: u32 = 8;
/// C `FILTER_BITS` (super_res.c:18).
const FILTER_BITS: i32 = 7;
/// C `upscale_normative_rect`'s `border_cols` = TAPS/2 + 1 = 5. The kernel
/// reaches at most this far outside the tile column on each side.
pub const UPSCALE_BORDER_COLS: usize = UPSCALE_NORMATIVE_TAPS / 2 + 1;

/// C `svt_av1_resize_filter_normative` (super_res.c:20) — 64 phases x 8 taps,
/// verbatim. Pinned phase-by-phase against the real C table by
/// `c_parity_superres::c_normative_filter_table_matches_c`.
pub static RESIZE_FILTER_NORMATIVE: [[i16; UPSCALE_NORMATIVE_TAPS]; 1 << RS_SUBPEL_BITS] = [
    [0, 0, 0, 128, 0, 0, 0, 0],
    [0, 0, -1, 128, 2, -1, 0, 0],
    [0, 1, -3, 127, 4, -2, 1, 0],
    [0, 1, -4, 127, 6, -3, 1, 0],
    [0, 2, -6, 126, 8, -3, 1, 0],
    [0, 2, -7, 125, 11, -4, 1, 0],
    [-1, 2, -8, 125, 13, -5, 2, 0],
    [-1, 3, -9, 124, 15, -6, 2, 0],
    [-1, 3, -10, 123, 18, -6, 2, -1],
    [-1, 3, -11, 122, 20, -7, 3, -1],
    [-1, 4, -12, 121, 22, -8, 3, -1],
    [-1, 4, -13, 120, 25, -9, 3, -1],
    [-1, 4, -14, 118, 28, -9, 3, -1],
    [-1, 4, -15, 117, 30, -10, 4, -1],
    [-1, 5, -16, 116, 32, -11, 4, -1],
    [-1, 5, -16, 114, 35, -12, 4, -1],
    [-1, 5, -17, 112, 38, -12, 4, -1],
    [-1, 5, -18, 111, 40, -13, 5, -1],
    [-1, 5, -18, 109, 43, -14, 5, -1],
    [-1, 6, -19, 107, 45, -14, 5, -1],
    [-1, 6, -19, 105, 48, -15, 5, -1],
    [-1, 6, -19, 103, 51, -16, 5, -1],
    [-1, 6, -20, 101, 53, -16, 6, -1],
    [-1, 6, -20, 99, 56, -17, 6, -1],
    [-1, 6, -20, 97, 58, -17, 6, -1],
    [-1, 6, -20, 95, 61, -18, 6, -1],
    [-2, 7, -20, 93, 64, -18, 6, -2],
    [-2, 7, -20, 91, 66, -19, 6, -1],
    [-2, 7, -20, 88, 69, -19, 6, -1],
    [-2, 7, -20, 86, 71, -19, 6, -1],
    [-2, 7, -20, 84, 74, -20, 7, -2],
    [-2, 7, -20, 81, 76, -20, 7, -1],
    [-2, 7, -20, 79, 79, -20, 7, -2],
    [-1, 7, -20, 76, 81, -20, 7, -2],
    [-2, 7, -20, 74, 84, -20, 7, -2],
    [-1, 6, -19, 71, 86, -20, 7, -2],
    [-1, 6, -19, 69, 88, -20, 7, -2],
    [-1, 6, -19, 66, 91, -20, 7, -2],
    [-2, 6, -18, 64, 93, -20, 7, -2],
    [-1, 6, -18, 61, 95, -20, 6, -1],
    [-1, 6, -17, 58, 97, -20, 6, -1],
    [-1, 6, -17, 56, 99, -20, 6, -1],
    [-1, 6, -16, 53, 101, -20, 6, -1],
    [-1, 5, -16, 51, 103, -19, 6, -1],
    [-1, 5, -15, 48, 105, -19, 6, -1],
    [-1, 5, -14, 45, 107, -19, 6, -1],
    [-1, 5, -14, 43, 109, -18, 5, -1],
    [-1, 5, -13, 40, 111, -18, 5, -1],
    [-1, 4, -12, 38, 112, -17, 5, -1],
    [-1, 4, -12, 35, 114, -16, 5, -1],
    [-1, 4, -11, 32, 116, -16, 5, -1],
    [-1, 4, -10, 30, 117, -15, 4, -1],
    [-1, 3, -9, 28, 118, -14, 4, -1],
    [-1, 3, -9, 25, 120, -13, 4, -1],
    [-1, 3, -8, 22, 121, -12, 4, -1],
    [-1, 3, -7, 20, 122, -11, 3, -1],
    [-1, 2, -6, 18, 123, -10, 3, -1],
    [0, 2, -6, 15, 124, -9, 3, -1],
    [0, 2, -5, 13, 125, -8, 2, -1],
    [0, 1, -4, 11, 125, -7, 2, 0],
    [0, 1, -3, 8, 126, -6, 2, 0],
    [0, 1, -3, 6, 127, -4, 1, 0],
    [0, 1, -2, 4, 127, -3, 1, 0],
    [0, 0, -1, 2, 128, -1, 0, 0],
];

/// C `av1_get_upscale_convolve_step` (super_res.c:76, `static`).
///
/// The per-output-pixel source-position increment in the
/// [`RS_SCALE_SUBPEL_BITS`] domain.
pub fn upscale_convolve_step(in_length: i32, out_length: i32) -> i32 {
    ((in_length << RS_SCALE_SUBPEL_BITS) + out_length / 2) / out_length
}

/// C `get_upscale_convolve_x0` (super_res.c:80, `static`).
///
/// The initial source position, carrying both the centering offset for the
/// scale change and `RS_SCALE_EXTRA_OFF` (the phase-rounding half-step), then
/// wrapped into one source pixel. Getting this wrong shifts the whole
/// upscaled row by a sub-pixel — which is why the stub this replaced could
/// never match C.
pub fn upscale_convolve_x0(in_length: i32, out_length: i32, x_step_qn: i32) -> i32 {
    let err = out_length * x_step_qn - (in_length << RS_SCALE_SUBPEL_BITS);
    let x0 = (-((out_length - in_length) << (RS_SCALE_SUBPEL_BITS - 1)) + out_length / 2)
        / out_length
        + RS_SCALE_EXTRA_OFF
        - err / 2;
    ((x0 as u32) & (RS_SCALE_SUBPEL_MASK as u32)) as i32
}

/// C `calculate_scaled_size_helper` (super_res.c:52) — the DOWNSCALED
/// dimension for a superres denominator (`denom` 9..=16; 8 = no scaling).
/// Clamped to >= min(16, dim) for the spec Appendix-A minimum-size rule.
pub fn scaled_size(dim: u16, denom: u8) -> u16 {
    if denom as u32 != SCALE_NUMERATOR && denom <= 16 {
        let min_dim = core::cmp::min(16u32, u32::from(dim));
        let scaled = (u32::from(dim) * SCALE_NUMERATOR + u32::from(denom) / 2) / u32::from(denom);
        core::cmp::max(scaled, min_dim) as u16
    } else {
        dim
    }
}

/// One tile column's border policy for [`upscale_normative_row`], mirroring
/// C `upscale_normative_rect`'s `pad_left` / `pad_right`.
///
/// C physically overwrites [`UPSCALE_BORDER_COLS`] pixels on each side of the
/// tile column with the edge pixel (and restores them afterwards) when the
/// flag is set; where it is NOT set the kernel reads the REAL neighbouring
/// tile column's pixels straight out of the frame buffer. This port models
/// both without mutating the source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileColPad {
    /// Replicate the leftmost pixel instead of reading the left neighbour
    /// (C: first tile column, or a tile boundary that may not be crossed).
    pub left: bool,
    /// Replicate the rightmost pixel instead of reading the right neighbour.
    pub right: bool,
}

impl TileColPad {
    /// The whole-frame case: one tile column, both edges replicated.
    pub const FRAME: Self = Self {
        left: true,
        right: true,
    };
}

/// The source sample C's kernel reads at tile-column-relative index `i`,
/// applying `upscale_normative_rect`'s border policy.
#[inline]
fn sample(row: &[u8], x0: usize, width: usize, i: i32, pad: TileColPad) -> u8 {
    if i < 0 {
        if pad.left {
            row[x0]
        } else {
            row[(x0 as i32 + i) as usize]
        }
    } else if i as usize >= width {
        if pad.right {
            row[x0 + width - 1]
        } else {
            row[x0 + i as usize]
        }
    } else {
        row[x0 + i as usize]
    }
}

/// C `av1_convolve_horiz_rs_c` (super_res.c:88) for ONE row of ONE tile
/// column, with `upscale_normative_rect`'s border handling folded in.
///
/// `row` is the whole DOWNSCALED source row; the tile column occupies
/// `[src_x0, src_x0 + src_width)`. `dst` receives `dst_width` upscaled
/// samples. `x_step_qn` / `x0_qn` come from [`upscale_convolve_step`] /
/// [`upscale_convolve_x0`] over the PLANE widths (not the tile column's).
///
/// Tap indexing follows C exactly: the kernel is handed `input - 1` and then
/// subtracts `TAPS/2 - 1 = 3`, so tap `k` of output `x` reads source index
/// `(x_qn >> RS_SCALE_SUBPEL_BITS) - 4 + k`.
#[allow(clippy::too_many_arguments)]
pub fn upscale_normative_row(
    row: &[u8],
    src_x0: usize,
    src_width: usize,
    dst: &mut [u8],
    dst_width: usize,
    x_step_qn: i32,
    x0_qn: i32,
    pad: TileColPad,
) {
    debug_assert!(src_width > 0 && dst_width > 0);
    let mut x_qn = x0_qn;
    for x in dst.iter_mut().take(dst_width) {
        let base = x_qn >> RS_SCALE_SUBPEL_BITS;
        let phase = ((x_qn & RS_SCALE_SUBPEL_MASK) >> RS_SCALE_EXTRA_BITS) as usize;
        debug_assert!(phase <= RS_SUBPEL_MASK as usize);
        let filter = &RESIZE_FILTER_NORMATIVE[phase];
        let mut sum: i32 = 0;
        for (k, &tap) in filter.iter().enumerate() {
            let idx = base - (UPSCALE_NORMATIVE_TAPS as i32 / 2) + k as i32;
            // C reaches at most `border_cols` outside the tile column; a
            // wider reach would read pixels C never replicated.
            debug_assert!(
                idx >= -(UPSCALE_BORDER_COLS as i32)
                    && idx < (src_width + UPSCALE_BORDER_COLS) as i32,
                "upscale tap {idx} outside the +/-{UPSCALE_BORDER_COLS} border of a \
                 {src_width}-wide tile column"
            );
            sum += i32::from(sample(row, src_x0, src_width, idx, pad)) * i32::from(tap);
        }
        // C `clip_pixel(ROUND_POWER_OF_TWO(sum, FILTER_BITS))`.
        *x = (((sum + (1 << (FILTER_BITS - 1))) >> FILTER_BITS).clamp(0, 255)) as u8;
        x_qn += x_step_qn;
    }
}

/// Whole-plane normative upscale, single tile column (C
/// `svt_av1_upscale_normative_rows` with `tile_cols == 1`): `src_width` ->
/// `dst_width` horizontally, height unchanged.
///
/// This is the shape the encoder's recon-upscale hook needs (chunk B): after
/// CDEF and before loop restoration, C upscales the reconstruction to the
/// full frame width (`svt_av1_superres_upscale_frame`, cdef_process.c:152).
#[allow(clippy::too_many_arguments)]
pub fn upscale_normative_plane(
    src: &[u8],
    src_stride: usize,
    src_width: usize,
    dst: &mut [u8],
    dst_stride: usize,
    dst_width: usize,
    rows: usize,
) {
    let x_step_qn = upscale_convolve_step(src_width as i32, dst_width as i32);
    let x0_qn = upscale_convolve_x0(src_width as i32, dst_width as i32, x_step_qn);
    for r in 0..rows {
        upscale_normative_row(
            &src[r * src_stride..],
            0,
            src_width,
            &mut dst[r * dst_stride..],
            dst_width,
            x_step_qn,
            x0_qn,
            TileColPad::FRAME,
        );
    }
}
