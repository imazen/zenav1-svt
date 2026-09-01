//! Source RESIZE (downscale) — the encoder-side half of super-resolution.
//!
//! Port of SVT-AV1 `resize.c`: `svt_av1_interpolate_core_c`,
//! `svt_av1_down2_symeven_c` / `down2_symodd`, `choose_interp_filter` with its
//! four band-limited filter banks, `resize_multistep`, and
//! `svt_av1_resize_plane_horizontal` (the shape superres uses — width changes,
//! height does not).
//!
//! Superres encodes the frame at `coded_w = frame_w * 8 / denom`; C produces
//! that downscaled SOURCE with these filters. Unlike the UPSCALE
//! ([`crate::superres`]) this side is NOT normative — the decoder never runs
//! it — but byte-parity with C requires reproducing it exactly, because a
//! different downscale means a different source and therefore a different
//! bitstream. Pinned against the real C by `tests/c_parity_resize.rs`.
//!
//! Scope: 8-bit, horizontal-only (`height2 == height`), which is exactly what
//! superres needs. The vertical/2D `svt_av1_resize_plane` (reference scaling,
//! not superres) and the highbd twins are not ported — see
//! `rust/docs/superres-port-map.md`.

use alloc::vec;
use alloc::vec::Vec;

use crate::superres::{
    RESIZE_FILTER_NORMATIVE, RS_SCALE_EXTRA_BITS, RS_SCALE_EXTRA_OFF, RS_SCALE_SUBPEL_BITS,
    RS_SUBPEL_BITS, RS_SUBPEL_MASK,
};

/// C `SUBPEL_TAPS` (filter.h:27).
pub const SUBPEL_TAPS: usize = 8;
/// C `FILTER_BITS`.
const FILTER_BITS: i32 = 7;

/// C `svt_aom_av1_down2_symeven_half_filter` (resize.c:32) — the 2:1
/// even-symmetric half filter (denominator 16 takes one down2 step).
pub static DOWN2_SYMEVEN_HALF_FILTER: [i16; 4] = [56, 12, -3, -1];
/// C `av1_down2_symodd_half_filter` (resize.c:33).
pub static DOWN2_SYMODD_HALF_FILTER: [i16; 4] = [64, 35, 0, -3];

/// C `svt_aom_av1_filteredinterp_filters875` (resize.c:114).
pub static FILTEREDINTERP_FILTERS875: [[i16; SUBPEL_TAPS]; 1 << RS_SUBPEL_BITS] = [
    [3, -8, 13, 112, 13, -8, 3, 0],
    [2, -7, 12, 112, 15, -8, 3, -1],
    [3, -7, 10, 112, 17, -9, 3, -1],
    [2, -6, 8, 112, 19, -9, 3, -1],
    [2, -6, 7, 112, 21, -10, 3, -1],
    [2, -5, 6, 111, 22, -10, 3, -1],
    [2, -5, 4, 111, 24, -10, 3, -1],
    [2, -4, 3, 110, 26, -11, 3, -1],
    [2, -4, 1, 110, 28, -11, 3, -1],
    [2, -4, 0, 109, 30, -12, 4, -1],
    [1, -3, -1, 108, 32, -12, 4, -1],
    [1, -3, -2, 108, 34, -13, 4, -1],
    [1, -2, -4, 107, 36, -13, 4, -1],
    [1, -2, -5, 106, 38, -13, 4, -1],
    [1, -1, -6, 105, 40, -14, 4, -1],
    [1, -1, -7, 104, 42, -14, 4, -1],
    [1, -1, -7, 103, 44, -15, 4, -1],
    [1, 0, -8, 101, 46, -15, 4, -1],
    [1, 0, -9, 100, 48, -15, 4, -1],
    [1, 0, -10, 99, 50, -15, 4, -1],
    [1, 1, -11, 97, 53, -16, 4, -1],
    [0, 1, -11, 96, 55, -16, 4, -1],
    [0, 1, -12, 95, 57, -16, 4, -1],
    [0, 2, -13, 93, 59, -16, 4, -1],
    [0, 2, -13, 91, 61, -16, 4, -1],
    [0, 2, -14, 90, 63, -16, 4, -1],
    [0, 2, -14, 88, 65, -16, 4, -1],
    [0, 2, -15, 86, 67, -16, 4, 0],
    [0, 3, -15, 84, 69, -17, 4, 0],
    [0, 3, -16, 83, 71, -17, 4, 0],
    [0, 3, -16, 81, 73, -16, 3, 0],
    [0, 3, -16, 79, 75, -16, 3, 0],
    [0, 3, -16, 77, 77, -16, 3, 0],
    [0, 3, -16, 75, 79, -16, 3, 0],
    [0, 3, -16, 73, 81, -16, 3, 0],
    [0, 4, -17, 71, 83, -16, 3, 0],
    [0, 4, -17, 69, 84, -15, 3, 0],
    [0, 4, -16, 67, 86, -15, 2, 0],
    [-1, 4, -16, 65, 88, -14, 2, 0],
    [-1, 4, -16, 63, 90, -14, 2, 0],
    [-1, 4, -16, 61, 91, -13, 2, 0],
    [-1, 4, -16, 59, 93, -13, 2, 0],
    [-1, 4, -16, 57, 95, -12, 1, 0],
    [-1, 4, -16, 55, 96, -11, 1, 0],
    [-1, 4, -16, 53, 97, -11, 1, 1],
    [-1, 4, -15, 50, 99, -10, 0, 1],
    [-1, 4, -15, 48, 100, -9, 0, 1],
    [-1, 4, -15, 46, 101, -8, 0, 1],
    [-1, 4, -15, 44, 103, -7, -1, 1],
    [-1, 4, -14, 42, 104, -7, -1, 1],
    [-1, 4, -14, 40, 105, -6, -1, 1],
    [-1, 4, -13, 38, 106, -5, -2, 1],
    [-1, 4, -13, 36, 107, -4, -2, 1],
    [-1, 4, -13, 34, 108, -2, -3, 1],
    [-1, 4, -12, 32, 108, -1, -3, 1],
    [-1, 4, -12, 30, 109, 0, -4, 2],
    [-1, 3, -11, 28, 110, 1, -4, 2],
    [-1, 3, -11, 26, 110, 3, -4, 2],
    [-1, 3, -10, 24, 111, 4, -5, 2],
    [-1, 3, -10, 22, 111, 6, -5, 2],
    [-1, 3, -10, 21, 112, 7, -6, 2],
    [-1, 3, -9, 19, 112, 8, -6, 2],
    [-1, 3, -9, 17, 112, 10, -7, 3],
    [-1, 3, -8, 15, 112, 12, -7, 2],
];

/// C `svt_aom_av1_filteredinterp_filters750` (resize.c:88).
pub static FILTEREDINTERP_FILTERS750: [[i16; SUBPEL_TAPS]; 1 << RS_SUBPEL_BITS] = [
    [2, -11, 25, 96, 25, -11, 2, 0],
    [2, -11, 24, 96, 26, -11, 2, 0],
    [2, -11, 22, 96, 28, -11, 2, 0],
    [2, -10, 21, 96, 29, -12, 2, 0],
    [2, -10, 19, 96, 31, -12, 2, 0],
    [2, -10, 18, 95, 32, -11, 2, 0],
    [2, -10, 17, 95, 34, -12, 2, 0],
    [2, -9, 15, 95, 35, -12, 2, 0],
    [2, -9, 14, 94, 37, -12, 2, 0],
    [2, -9, 13, 94, 38, -12, 2, 0],
    [2, -8, 12, 93, 40, -12, 1, 0],
    [2, -8, 11, 93, 41, -12, 1, 0],
    [2, -8, 9, 92, 43, -12, 1, 1],
    [2, -8, 8, 92, 44, -12, 1, 1],
    [2, -7, 7, 91, 46, -12, 1, 0],
    [2, -7, 6, 90, 47, -12, 1, 1],
    [2, -7, 5, 90, 49, -12, 1, 0],
    [2, -6, 4, 89, 50, -12, 1, 0],
    [2, -6, 3, 88, 52, -12, 0, 1],
    [2, -6, 2, 87, 54, -12, 0, 1],
    [2, -5, 1, 86, 55, -12, 0, 1],
    [2, -5, 0, 85, 57, -12, 0, 1],
    [2, -5, -1, 84, 58, -11, 0, 1],
    [2, -5, -2, 83, 60, -11, 0, 1],
    [2, -4, -2, 82, 61, -11, -1, 1],
    [1, -4, -3, 81, 63, -10, -1, 1],
    [2, -4, -4, 80, 64, -10, -1, 1],
    [1, -4, -4, 79, 66, -10, -1, 1],
    [1, -3, -5, 77, 67, -9, -1, 1],
    [1, -3, -6, 76, 69, -9, -1, 1],
    [1, -3, -6, 75, 70, -8, -2, 1],
    [1, -2, -7, 74, 71, -8, -2, 1],
    [1, -2, -7, 72, 72, -7, -2, 1],
    [1, -2, -8, 71, 74, -7, -2, 1],
    [1, -2, -8, 70, 75, -6, -3, 1],
    [1, -1, -9, 69, 76, -6, -3, 1],
    [1, -1, -9, 67, 77, -5, -3, 1],
    [1, -1, -10, 66, 79, -4, -4, 1],
    [1, -1, -10, 64, 80, -4, -4, 2],
    [1, -1, -10, 63, 81, -3, -4, 1],
    [1, -1, -11, 61, 82, -2, -4, 2],
    [1, 0, -11, 60, 83, -2, -5, 2],
    [1, 0, -11, 58, 84, -1, -5, 2],
    [1, 0, -12, 57, 85, 0, -5, 2],
    [1, 0, -12, 55, 86, 1, -5, 2],
    [1, 0, -12, 54, 87, 2, -6, 2],
    [1, 0, -12, 52, 88, 3, -6, 2],
    [0, 1, -12, 50, 89, 4, -6, 2],
    [0, 1, -12, 49, 90, 5, -7, 2],
    [1, 1, -12, 47, 90, 6, -7, 2],
    [0, 1, -12, 46, 91, 7, -7, 2],
    [1, 1, -12, 44, 92, 8, -8, 2],
    [1, 1, -12, 43, 92, 9, -8, 2],
    [0, 1, -12, 41, 93, 11, -8, 2],
    [0, 1, -12, 40, 93, 12, -8, 2],
    [0, 2, -12, 38, 94, 13, -9, 2],
    [0, 2, -12, 37, 94, 14, -9, 2],
    [0, 2, -12, 35, 95, 15, -9, 2],
    [0, 2, -12, 34, 95, 17, -10, 2],
    [0, 2, -11, 32, 95, 18, -10, 2],
    [0, 2, -12, 31, 96, 19, -10, 2],
    [0, 2, -12, 29, 96, 21, -10, 2],
    [0, 2, -11, 28, 96, 22, -11, 2],
    [0, 2, -11, 26, 96, 24, -11, 2],
];

/// C `svt_aom_av1_filteredinterp_filters625` (resize.c:62).
pub static FILTEREDINTERP_FILTERS625: [[i16; SUBPEL_TAPS]; 1 << RS_SUBPEL_BITS] = [
    [-1, -8, 33, 80, 33, -8, -1, 0],
    [-1, -8, 31, 80, 34, -8, -1, 1],
    [-1, -8, 30, 80, 35, -8, -1, 1],
    [-1, -8, 29, 80, 36, -7, -2, 1],
    [-1, -8, 28, 80, 37, -7, -2, 1],
    [-1, -8, 27, 80, 38, -7, -2, 1],
    [0, -8, 26, 79, 39, -7, -2, 1],
    [0, -8, 25, 79, 40, -7, -2, 1],
    [0, -8, 24, 79, 41, -7, -2, 1],
    [0, -8, 23, 78, 42, -6, -2, 1],
    [0, -8, 22, 78, 43, -6, -2, 1],
    [0, -8, 21, 78, 44, -6, -2, 1],
    [0, -8, 20, 78, 45, -5, -3, 1],
    [0, -8, 19, 77, 47, -5, -3, 1],
    [0, -8, 18, 77, 48, -5, -3, 1],
    [0, -8, 17, 77, 49, -5, -3, 1],
    [0, -8, 16, 76, 50, -4, -3, 1],
    [0, -8, 15, 76, 51, -4, -3, 1],
    [0, -8, 15, 75, 52, -3, -4, 1],
    [0, -7, 14, 74, 53, -3, -4, 1],
    [0, -7, 13, 74, 54, -3, -4, 1],
    [0, -7, 12, 73, 55, -2, -4, 1],
    [0, -7, 11, 73, 56, -2, -4, 1],
    [0, -7, 10, 72, 57, -1, -4, 1],
    [1, -7, 10, 71, 58, -1, -5, 1],
    [0, -7, 9, 71, 59, 0, -5, 1],
    [1, -7, 8, 70, 60, 0, -5, 1],
    [1, -7, 7, 69, 61, 1, -5, 1],
    [1, -6, 6, 68, 62, 1, -5, 1],
    [0, -6, 6, 68, 62, 2, -5, 1],
    [1, -6, 5, 67, 63, 2, -5, 1],
    [1, -6, 5, 66, 64, 3, -6, 1],
    [1, -6, 4, 65, 65, 4, -6, 1],
    [1, -6, 3, 64, 66, 5, -6, 1],
    [1, -5, 2, 63, 67, 5, -6, 1],
    [1, -5, 2, 62, 68, 6, -6, 0],
    [1, -5, 1, 62, 68, 6, -6, 1],
    [1, -5, 1, 61, 69, 7, -7, 1],
    [1, -5, 0, 60, 70, 8, -7, 1],
    [1, -5, 0, 59, 71, 9, -7, 0],
    [1, -5, -1, 58, 71, 10, -7, 1],
    [1, -4, -1, 57, 72, 10, -7, 0],
    [1, -4, -2, 56, 73, 11, -7, 0],
    [1, -4, -2, 55, 73, 12, -7, 0],
    [1, -4, -3, 54, 74, 13, -7, 0],
    [1, -4, -3, 53, 74, 14, -7, 0],
    [1, -4, -3, 52, 75, 15, -8, 0],
    [1, -3, -4, 51, 76, 15, -8, 0],
    [1, -3, -4, 50, 76, 16, -8, 0],
    [1, -3, -5, 49, 77, 17, -8, 0],
    [1, -3, -5, 48, 77, 18, -8, 0],
    [1, -3, -5, 47, 77, 19, -8, 0],
    [1, -3, -5, 45, 78, 20, -8, 0],
    [1, -2, -6, 44, 78, 21, -8, 0],
    [1, -2, -6, 43, 78, 22, -8, 0],
    [1, -2, -6, 42, 78, 23, -8, 0],
    [1, -2, -7, 41, 79, 24, -8, 0],
    [1, -2, -7, 40, 79, 25, -8, 0],
    [1, -2, -7, 39, 79, 26, -8, 0],
    [1, -2, -7, 38, 80, 27, -8, -1],
    [1, -2, -7, 37, 80, 28, -8, -1],
    [1, -2, -7, 36, 80, 29, -8, -1],
    [1, -1, -8, 35, 80, 30, -8, -1],
    [1, -1, -8, 34, 80, 31, -8, -1],
];

/// C `svt_aom_av1_filteredinterp_filters500` (resize.c:36).
pub static FILTEREDINTERP_FILTERS500: [[i16; SUBPEL_TAPS]; 1 << RS_SUBPEL_BITS] = [
    [-3, 0, 35, 64, 35, 0, -3, 0],
    [-3, 0, 34, 64, 36, 0, -3, 0],
    [-3, -1, 34, 64, 36, 1, -3, 0],
    [-3, -1, 33, 64, 37, 1, -3, 0],
    [-3, -1, 32, 64, 38, 1, -3, 0],
    [-3, -1, 31, 64, 39, 1, -3, 0],
    [-3, -1, 31, 63, 39, 2, -3, 0],
    [-2, -2, 30, 63, 40, 2, -3, 0],
    [-2, -2, 29, 63, 41, 2, -3, 0],
    [-2, -2, 29, 63, 41, 3, -4, 0],
    [-2, -2, 28, 63, 42, 3, -4, 0],
    [-2, -2, 27, 63, 43, 3, -4, 0],
    [-2, -3, 27, 63, 43, 4, -4, 0],
    [-2, -3, 26, 62, 44, 5, -4, 0],
    [-2, -3, 25, 62, 45, 5, -4, 0],
    [-2, -3, 25, 62, 45, 5, -4, 0],
    [-2, -3, 24, 62, 46, 5, -4, 0],
    [-2, -3, 23, 61, 47, 6, -4, 0],
    [-2, -3, 23, 61, 47, 6, -4, 0],
    [-2, -3, 22, 61, 48, 7, -4, -1],
    [-2, -3, 21, 60, 49, 7, -4, 0],
    [-1, -4, 20, 60, 49, 8, -4, 0],
    [-1, -4, 20, 60, 50, 8, -4, -1],
    [-1, -4, 19, 59, 51, 9, -4, -1],
    [-1, -4, 19, 59, 51, 9, -4, -1],
    [-1, -4, 18, 58, 52, 10, -4, -1],
    [-1, -4, 17, 58, 52, 11, -4, -1],
    [-1, -4, 16, 58, 53, 11, -4, -1],
    [-1, -4, 16, 57, 53, 12, -4, -1],
    [-1, -4, 15, 57, 54, 12, -4, -1],
    [-1, -4, 15, 56, 54, 13, -4, -1],
    [-1, -4, 14, 56, 55, 13, -4, -1],
    [-1, -4, 14, 55, 55, 14, -4, -1],
    [-1, -4, 13, 55, 56, 14, -4, -1],
    [-1, -4, 13, 54, 56, 15, -4, -1],
    [-1, -4, 12, 54, 57, 15, -4, -1],
    [-1, -4, 12, 53, 57, 16, -4, -1],
    [-1, -4, 11, 53, 58, 16, -4, -1],
    [-1, -4, 11, 52, 58, 17, -4, -1],
    [-1, -4, 10, 52, 58, 18, -4, -1],
    [-1, -4, 9, 51, 59, 19, -4, -1],
    [-1, -4, 9, 51, 59, 19, -4, -1],
    [-1, -4, 8, 50, 60, 20, -4, -1],
    [0, -4, 8, 49, 60, 20, -4, -1],
    [0, -4, 7, 49, 60, 21, -3, -2],
    [-1, -4, 7, 48, 61, 22, -3, -2],
    [0, -4, 6, 47, 61, 23, -3, -2],
    [0, -4, 6, 47, 61, 23, -3, -2],
    [0, -4, 5, 46, 62, 24, -3, -2],
    [0, -4, 5, 45, 62, 25, -3, -2],
    [0, -4, 5, 45, 62, 25, -3, -2],
    [0, -4, 5, 44, 62, 26, -3, -2],
    [0, -4, 4, 43, 63, 27, -3, -2],
    [0, -4, 3, 43, 63, 27, -2, -2],
    [0, -4, 3, 42, 63, 28, -2, -2],
    [0, -4, 3, 41, 63, 29, -2, -2],
    [0, -3, 2, 41, 63, 29, -2, -2],
    [0, -3, 2, 40, 63, 30, -2, -2],
    [0, -3, 2, 39, 63, 31, -1, -3],
    [0, -3, 1, 39, 64, 31, -1, -3],
    [0, -3, 1, 38, 64, 32, -1, -3],
    [0, -3, 1, 37, 64, 33, -1, -3],
    [0, -3, 1, 36, 64, 34, -1, -3],
    [0, -3, 0, 36, 64, 34, 0, -3],
];

/// C `choose_interp_filter` (resize.c:272) — the filter bank for a given
/// scale ratio. `filteredinterp_filters1000` is `#define`d to the normative
/// upscale table (resize.h:75), so the 1:1-or-upscale bank IS
/// [`RESIZE_FILTER_NORMATIVE`].
pub fn choose_interp_filter(
    in_length: i32,
    out_length: i32,
) -> &'static [[i16; SUBPEL_TAPS]; 1 << RS_SUBPEL_BITS] {
    let out_length16 = out_length * 16;
    if out_length16 >= in_length * 16 {
        &RESIZE_FILTER_NORMATIVE
    } else if out_length16 >= in_length * 13 {
        &FILTEREDINTERP_FILTERS875
    } else if out_length16 >= in_length * 11 {
        &FILTEREDINTERP_FILTERS750
    } else if out_length16 >= in_length * 9 {
        &FILTEREDINTERP_FILTERS625
    } else {
        &FILTEREDINTERP_FILTERS500
    }
}

#[inline]
fn clip_pixel(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// C `svt_av1_interpolate_core_c` (resize.c:287) — polyphase resample of one
/// 1-D line from `in_length` to `out_length`.
///
/// C splits the output into initial / middle / end parts so the middle can skip
/// the edge clamps; this port runs ONE loop with the full clamp, which is
/// bit-identical (C computes `x1`/`x2` precisely so that the middle part's taps
/// are already in range — the clamp is a no-op there) and cannot silently
/// under- or over-read. The `c_parity_resize` sweep covers both regimes,
/// including the `x1 > x2` short-input case.
pub fn interpolate_core(
    input: &[u8],
    in_length: usize,
    output: &mut [u8],
    out_length: usize,
    filters: &[[i16; SUBPEL_TAPS]; 1 << RS_SUBPEL_BITS],
) {
    debug_assert!(in_length > 0 && out_length > 0);
    let (inl, outl) = (in_length as i32, out_length as i32);
    let delta = (((in_length as u32) << RS_SCALE_SUBPEL_BITS) as i32 + outl / 2) / outl;
    let offset = if inl > outl {
        (((inl - outl) << (RS_SCALE_SUBPEL_BITS - 1)) + outl / 2) / outl
    } else {
        -((((outl - inl) << (RS_SCALE_SUBPEL_BITS - 1)) + outl / 2) / outl)
    };
    let mut y = offset + RS_SCALE_EXTRA_OFF;
    for out in output.iter_mut().take(out_length) {
        let int_pel = y >> RS_SCALE_SUBPEL_BITS;
        let sub_pel = ((y >> RS_SCALE_EXTRA_BITS) & RS_SUBPEL_MASK) as usize;
        let filter = &filters[sub_pel];
        let mut sum: i32 = 0;
        for (k, &tap) in filter.iter().enumerate() {
            let pk = int_pel - SUBPEL_TAPS as i32 / 2 + 1 + k as i32;
            let idx = pk.clamp(0, inl - 1) as usize;
            sum += i32::from(tap) * i32::from(input[idx]);
        }
        *out = clip_pixel((sum + (1 << (FILTER_BITS - 1))) >> FILTER_BITS);
        y += delta;
    }
}

/// C `svt_av1_down2_symeven_c` (resize.c:170) — exact 2:1 decimation with the
/// even-symmetric half filter. Same initial/middle/end -> single-clamped-loop
/// equivalence as [`interpolate_core`].
pub fn down2_symeven(input: &[u8], length: usize, output: &mut [u8]) {
    let filter = &DOWN2_SYMEVEN_HALF_FILTER;
    let len = length as i32;
    let mut o = 0usize;
    let mut i = 0i32;
    while i < len {
        let mut sum: i32 = 1 << (FILTER_BITS - 1);
        for (j, &tap) in filter.iter().enumerate() {
            let a = input[(i - j as i32).max(0) as usize];
            let b = input[(i + 1 + j as i32).min(len - 1) as usize];
            sum += (i32::from(a) + i32::from(b)) * i32::from(tap);
        }
        output[o] = clip_pixel(sum >> FILTER_BITS);
        o += 1;
        i += 2;
    }
}

/// C `down2_symodd` (resize.c:220, `static`) — 2:1 decimation with the
/// odd-symmetric half filter (odd input length).
pub fn down2_symodd(input: &[u8], length: usize, output: &mut [u8]) {
    let filter = &DOWN2_SYMODD_HALF_FILTER;
    let len = length as i32;
    let mut o = 0usize;
    let mut i = 0i32;
    while i < len {
        let mut sum: i32 =
            (1 << (FILTER_BITS - 1)) + i32::from(input[i as usize]) * i32::from(filter[0]);
        for (j, &tap) in filter.iter().enumerate().skip(1) {
            let a = input[(i - j as i32).max(0) as usize];
            let b = input[(i + j as i32).min(len - 1) as usize];
            sum += (i32::from(a) + i32::from(b)) * i32::from(tap);
        }
        output[o] = clip_pixel(sum >> FILTER_BITS);
        o += 1;
        i += 2;
    }
}

/// C `get_down2_length` (resize.c:147, `static`).
pub fn down2_length(mut length: usize, steps: usize) -> usize {
    for _ in 0..steps {
        length = (length + 1) >> 1;
    }
    length
}

/// C `get_down2_steps` (resize.c:154, `static`) — how many exact 2:1 steps fit
/// before the polyphase interpolate finishes the job. Non-zero only at
/// denominator 16 (one step) for the superres ratios.
pub fn down2_steps(mut in_length: usize, out_length: usize) -> usize {
    let mut steps = 0usize;
    loop {
        let proj = down2_length(in_length, 1);
        if proj < out_length {
            break;
        }
        steps += 1;
        in_length = proj;
        if in_length == 1 {
            break;
        }
    }
    steps
}

/// C `resize_multistep` (resize.c:366, `static`) — one 1-D line, `length` ->
/// `olength`: exact 2:1 steps while they fit, then the polyphase interpolate.
pub fn resize_multistep(input: &[u8], length: usize, output: &mut [u8], olength: usize) {
    if length == olength {
        output[..length].copy_from_slice(&input[..length]);
        return;
    }
    let steps = down2_steps(length, olength);
    if steps == 0 {
        // denom 9..15
        interpolate_core(
            input,
            length,
            output,
            olength,
            choose_interp_filter(length as i32, olength as i32),
        );
        return;
    }
    // denom 16: one exact 2:1 step, then interpolate if anything remains.
    let mut cur: Vec<u8> = input[..length].to_vec();
    let mut filtered_length = length;
    for _ in 0..steps {
        let proj = down2_length(filtered_length, 1);
        let mut next = vec![0u8; proj];
        if filtered_length & 1 != 0 {
            down2_symodd(&cur, filtered_length, &mut next);
        } else {
            down2_symeven(&cur, filtered_length, &mut next);
        }
        cur = next;
        filtered_length = proj;
    }
    if filtered_length == olength {
        output[..olength].copy_from_slice(&cur[..olength]);
    } else {
        interpolate_core(
            &cur,
            filtered_length,
            output,
            olength,
            choose_interp_filter(filtered_length as i32, olength as i32),
        );
    }
}

/// C `svt_av1_resize_plane_horizontal` (resize.c:464) — resize a plane
/// horizontally only (`height2 == height`), which is the superres source
/// downscale: `width` -> `width2` at unchanged height.
#[allow(clippy::too_many_arguments)]
pub fn resize_plane_horizontal(
    input: &[u8],
    height: usize,
    width: usize,
    in_stride: usize,
    output: &mut [u8],
    width2: usize,
    out_stride: usize,
) {
    debug_assert!(width > 0 && height > 0 && width2 > 0);
    for r in 0..height {
        resize_multistep(
            &input[r * in_stride..],
            width,
            &mut output[r * out_stride..],
            width2,
        );
    }
}

/// C `fill_col_to_arr` (`resize.c:413`) — static.
///
/// Gather one strided column into a contiguous scratch array, so the same
/// 1-D `resize_multistep` that does rows can do columns.
pub fn fill_col_to_arr(img: &[u8], stride: usize, len: usize, arr: &mut [u8]) {
    for (i, dst) in arr.iter_mut().take(len).enumerate() {
        *dst = img[i * stride];
    }
}

/// C `fill_arr_to_col` (`resize.c:404`) — static. The inverse scatter.
pub fn fill_arr_to_col(img: &mut [u8], stride: usize, len: usize, arr: &[u8]) {
    for (i, src) in arr.iter().take(len).enumerate() {
        img[i * stride] = *src;
    }
}

/// C `svt_av1_resize_plane_c` (`resize.c:422`) — the TWO-dimensional plane
/// resize.
///
/// Frame resize (`--resize-mode`) scales both dimensions, unlike superres,
/// which is horizontal-only — which is why the port previously had
/// [`resize_plane_horizontal`] and not this.
///
/// The shape is C's: every row is resized `width` -> `width2` into an
/// intermediate of stride `width2`, and then every one of the `width2`
/// columns is gathered into a scratch array, resized `height` -> `height2` by
/// the SAME 1-D `resize_multistep`, and scattered back. Doing the horizontal
/// pass first is not an optimisation — it is what makes the vertical pass read
/// already-horizontally-filtered samples, and swapping the order changes the
/// output.
///
/// C returns `EB_ErrorInsufficientResources` when any of its four scratch
/// allocations fails; the equivalent here is that the caller supplies the
/// output and this allocates its own scratch through `Vec`, so the only
/// failure mode C has is one Rust does not express at this layer.
///
/// # Panics
///
/// If `output` is too small for `height2` rows at `out_stride`, or `input` too
/// small for `height` rows at `in_stride`.
#[allow(clippy::too_many_arguments)]
pub fn resize_plane(
    input: &[u8],
    height: usize,
    width: usize,
    in_stride: usize,
    output: &mut [u8],
    height2: usize,
    width2: usize,
    out_stride: usize,
) {
    assert!(width > 0 && height > 0 && width2 > 0 && height2 > 0);
    assert!(input.len() >= (height - 1) * in_stride + width);
    assert!(output.len() >= (height2 - 1) * out_stride + width2);

    let mut intbuf = alloc::vec![0u8; width2 * height];
    let mut arrbuf = alloc::vec![0u8; height];
    let mut arrbuf2 = alloc::vec![0u8; height2];

    for r in 0..height {
        resize_multistep(
            &input[r * in_stride..],
            width,
            &mut intbuf[r * width2..],
            width2,
        );
    }
    for c in 0..width2 {
        fill_col_to_arr(&intbuf[c..], width2, height, &mut arrbuf);
        resize_multistep(&arrbuf, height, &mut arrbuf2, height2);
        fill_arr_to_col(&mut output[c..], out_stride, height2, &arrbuf2);
    }
}
