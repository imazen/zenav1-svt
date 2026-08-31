//! The AV1 inter *reconstruction* motion-compensation kernels.
//!
//! Ported from `Source/Lib/Codec/inter_prediction.c` (SVT-AV1 v4.2.0):
//! `svt_av1_convolve_2d_sr_c` (:329), `svt_av1_convolve_x_sr_c` (:402),
//! `svt_av1_convolve_y_sr_c` (:374), `svt_av1_convolve_2d_copy_sr_c` (:431),
//! and the compound (`jnt`) family `svt_av1_jnt_convolve_2d_c` (:526),
//! `svt_av1_jnt_convolve_y_c` (:584), `svt_av1_jnt_convolve_x_c` (:629),
//! `svt_av1_jnt_convolve_2d_copy_c` (:674).
//!
//! # This is NOT [`crate::inter_pred`]
//!
//! `inter_pred.rs` ports `svt_aom_convolve8_horiz_c` / `svt_aom_convolve8_vert_c`
//! — the single-pass `clip_pixel(ROUND_POWER_OF_TWO(sum, 7))` kernels that
//! `svt_aom_upsampled_pred_c` uses for motion-estimation sub-pel refinement.
//! Its `convolve_2d` composes two of those through a **u8** intermediate.
//!
//! The kernels here are the ones every reconstructed inter block goes through:
//! a **16-bit** intermediate, a `round_0`/`round_1` split (`ROUND0_BITS = 3`,
//! `round_1 = 2*FILTER_BITS - round_0 = 11` for single prediction,
//! `COMPOUND_ROUND1_BITS = 7` for compound), and an `offset_bits` bias that is
//! added before the first shift and subtracted after the second. The two
//! rounding contracts do not agree, so one cannot stand in for the other.
//!
//! # Faithfulness notes (each is a place a "cleaned up" port diverges)
//!
//! * `svt_av1_jnt_convolve_x_c` computes `bits = FILTER_BITS - round_1` while
//!   `svt_av1_jnt_convolve_y_c` computes `bits = FILTER_BITS - round_0`. That
//!   asymmetry is upstream (it matches libaom) and is reproduced verbatim.
//! * `ConvBufType` is `uint16_t`. `jnt_convolve_2d_copy`'s `res` is a
//!   `ConvBufType`, so `src << bits` and `res += round_offset` both wrap at 16
//!   bits before `do_average` widens them again; `jnt_convolve_y` likewise
//!   truncates its `res` on the store. Both wraps are modelled with `u16`.
//! * `svt_av1_convolve_2d_sr_c`'s vertical stage stores into an `int16_t res`
//!   after subtracting the offset — the truncation to `i16` is load-bearing.
//! * The horizontal pass of the 2D kernels reads `src - fo_vert * src_stride`
//!   (note: `fo_vert`, from the *vertical* filter) and runs for
//!   `im_h = h + taps - 1` rows.

use svtav1_types::tables::interp::{
    BILINEAR_FILTERS, InterpKernel, SUB_PEL_FILTERS_8, SUB_PEL_FILTERS_8SHARP,
    SUB_PEL_FILTERS_8SMOOTH,
};

/// `FILTER_BITS` (definitions.h:456).
pub const FILTER_BITS: i32 = 7;
/// `ROUND0_BITS` (convolve.h:22).
pub const ROUND0_BITS: i32 = 3;
/// `COMPOUND_ROUND1_BITS` (convolve.h:23).
pub const COMPOUND_ROUND1_BITS: i32 = 7;
/// `DIST_PRECISION_BITS` (definitions.h:451).
pub const DIST_PRECISION_BITS: i32 = 4;
/// `SUBPEL_MASK` (definitions.h:458).
pub const SUBPEL_MASK: i32 = 15;
/// `SUBPEL_TAPS` (definitions.h:460) — every entry of
/// `av1_interp_filter_params_list` and `av1_interp_4tap` uses this tap count,
/// including the "4-tap" tables (whose outer taps are zero).
pub const SUBPEL_TAPS: usize = 8;

/// `sub_pel_filters_4` (inter_prediction.c:254) — the narrow-block regular /
/// sharp kernel. Not in `svtav1_types::tables::interp`, which carries only the
/// four `av1_interp_filter_params_list` entries.
pub const SUB_PEL_FILTERS_4: [InterpKernel; 16] = [
    [0, 0, 0, 128, 0, 0, 0, 0],
    [0, 0, -4, 126, 8, -2, 0, 0],
    [0, 0, -8, 122, 18, -4, 0, 0],
    [0, 0, -10, 116, 28, -6, 0, 0],
    [0, 0, -12, 110, 38, -8, 0, 0],
    [0, 0, -12, 102, 48, -10, 0, 0],
    [0, 0, -14, 94, 58, -10, 0, 0],
    [0, 0, -12, 84, 66, -10, 0, 0],
    [0, 0, -12, 76, 76, -12, 0, 0],
    [0, 0, -10, 66, 84, -12, 0, 0],
    [0, 0, -10, 58, 94, -14, 0, 0],
    [0, 0, -10, 48, 102, -12, 0, 0],
    [0, 0, -8, 38, 110, -12, 0, 0],
    [0, 0, -6, 28, 116, -10, 0, 0],
    [0, 0, -4, 18, 122, -8, 0, 0],
    [0, 0, -2, 8, 126, -4, 0, 0],
];

/// `sub_pel_filters_4smooth` (inter_prediction.c:1177).
pub const SUB_PEL_FILTERS_4SMOOTH: [InterpKernel; 16] = [
    [0, 0, 0, 128, 0, 0, 0, 0],
    [0, 0, 30, 62, 34, 2, 0, 0],
    [0, 0, 26, 62, 36, 4, 0, 0],
    [0, 0, 22, 62, 40, 4, 0, 0],
    [0, 0, 20, 60, 42, 6, 0, 0],
    [0, 0, 18, 58, 44, 8, 0, 0],
    [0, 0, 16, 56, 46, 10, 0, 0],
    [0, 0, 14, 54, 48, 12, 0, 0],
    [0, 0, 12, 52, 52, 12, 0, 0],
    [0, 0, 12, 48, 54, 14, 0, 0],
    [0, 0, 10, 46, 56, 16, 0, 0],
    [0, 0, 8, 44, 58, 18, 0, 0],
    [0, 0, 6, 42, 60, 20, 0, 0],
    [0, 0, 4, 40, 62, 22, 0, 0],
    [0, 0, 4, 36, 62, 26, 0, 0],
    [0, 0, 2, 34, 62, 30, 0, 0],
];

/// `InterpFilter` (definitions.h) — the switchable filter set plus BILINEAR.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum InterpFilterKind {
    /// `EIGHTTAP_REGULAR`
    EightTapRegular = 0,
    /// `EIGHTTAP_SMOOTH`
    EightTapSmooth = 1,
    /// `MULTITAP_SHARP`
    MultiTapSharp = 2,
    /// `BILINEAR`
    Bilinear = 3,
}

/// `InterpFilterParams` (filter.h) reduced to what the kernels read: the
/// 16-phase kernel table. `taps` is `SUBPEL_TAPS` for every entry of both
/// `av1_interp_filter_params_list` and `av1_interp_4tap`, so it is not a field.
#[derive(Clone, Copy, Debug)]
pub struct FilterParams {
    /// The 16 sub-pel phases, 8 taps each.
    pub kernels: &'static [InterpKernel; 16],
}

impl FilterParams {
    /// `av1_get_interp_filter_subpel_kernel` (filter.h:77).
    #[inline]
    pub fn subpel_kernel(&self, subpel: i32) -> &'static InterpKernel {
        &self.kernels[(subpel & SUBPEL_MASK) as usize]
    }
}

/// `av1_interp_filter_params_list[f]` (inter_prediction.h:80).
pub fn interp_filter_params_list(f: InterpFilterKind) -> FilterParams {
    let kernels = match f {
        InterpFilterKind::EightTapRegular => &SUB_PEL_FILTERS_8,
        InterpFilterKind::EightTapSmooth => &SUB_PEL_FILTERS_8SMOOTH,
        InterpFilterKind::MultiTapSharp => &SUB_PEL_FILTERS_8SHARP,
        InterpFilterKind::Bilinear => &BILINEAR_FILTERS,
    };
    FilterParams { kernels }
}

/// `av1_get_interp_filter_params_with_block_size` (inter_prediction.h:128).
///
/// The `w <= 4` narrow-block substitution: REGULAR and SHARP both fall back to
/// `av1_interp_4tap[0]` (`sub_pel_filters_4`) — note SHARP maps to the
/// *regular* 4-tap table, not a sharp one — and SMOOTH to `av1_interp_4tap[1]`.
/// BILINEAR is never substituted.
pub fn interp_filter_params_with_block_size(f: InterpFilterKind, w: i32) -> FilterParams {
    if w <= 4 && (f == InterpFilterKind::MultiTapSharp || f == InterpFilterKind::EightTapRegular) {
        FilterParams {
            kernels: &SUB_PEL_FILTERS_4,
        }
    } else if w <= 4 && f == InterpFilterKind::EightTapSmooth {
        FilterParams {
            kernels: &SUB_PEL_FILTERS_4SMOOTH,
        }
    } else {
        interp_filter_params_list(f)
    }
}

/// `ConvolveParams` (definitions.h:681), minus the `dst` pointer — the compound
/// intermediate buffer is passed alongside as a slice so this stays safe Rust.
#[derive(Clone, Copy, Debug)]
pub struct ConvolveParams {
    /// `do_average`: blend into the existing CONV_BUF instead of writing it.
    pub do_average: bool,
    /// Row stride of the CONV_BUF (`dst_stride`).
    pub dst_stride: usize,
    /// First-stage right shift.
    pub round_0: i32,
    /// Second-stage right shift.
    pub round_1: i32,
    /// Whether this is a compound (two-reference) prediction.
    pub is_compound: bool,
    /// Use the distance-weighted average rather than a plain mean.
    pub use_jnt_comp_avg: bool,
    /// Forward weight (Q4), from `svt_av1_dist_wtd_comp_weight_assign`.
    pub fwd_offset: i32,
    /// Backward weight (Q4).
    pub bck_offset: i32,
}

impl ConvolveParams {
    /// `get_conv_params_no_round` (convolve.h:41).
    ///
    /// The `intbufrange > 16` correction is reproduced: it fires for `bd = 12`
    /// (`12 + 7 - 3 + 2 = 18`), never for 8 or 10.
    pub fn no_round(do_average: bool, dst_stride: usize, is_compound: bool, bd: i32) -> Self {
        let mut round_0 = ROUND0_BITS;
        let mut round_1 = if is_compound {
            COMPOUND_ROUND1_BITS
        } else {
            2 * FILTER_BITS - round_0
        };
        let intbufrange = bd + FILTER_BITS - round_0 + 2;
        if intbufrange > 16 {
            round_0 += intbufrange - 16;
            if !is_compound {
                round_1 -= intbufrange - 16;
            }
        }
        Self {
            do_average,
            dst_stride,
            round_0,
            round_1,
            is_compound,
            use_jnt_comp_avg: false,
            fwd_offset: 0,
            bck_offset: 0,
        }
    }

    /// `get_conv_params` (convolve.h:68) — single prediction, no CONV_BUF.
    pub fn single(do_average: bool, bd: i32) -> Self {
        Self::no_round(do_average, 0, false, bd)
    }
}

/// `ROUND_POWER_OF_TWO(value, n)` — round-half-up on a signed value.
#[inline]
fn round_power_of_two(value: i32, n: i32) -> i32 {
    if n == 0 {
        value
    } else {
        (value + (1 << (n - 1))) >> n
    }
}

/// `clip_pixel_highbd(val, bd)` for `bd = 8`.
#[inline]
fn clip_pixel_8(val: i32) -> u8 {
    val.clamp(0, 255) as u8
}

/// A source view whose logical origin sits `origin` elements into `data`, so a
/// kernel can read the `fo_horiz`/`fo_vert` taps that precede it without
/// negative indexing.
///
/// Every kernel below reads `src[y * stride + x - fo_horiz + k]` for
/// `y` from `-fo_vert`, exactly as C does off a raw pointer.
#[derive(Clone, Copy)]
pub struct SrcView<'a> {
    data: &'a [u8],
    origin: usize,
    stride: usize,
}

impl<'a> SrcView<'a> {
    /// Wrap `data` with its logical (0, 0) at `origin`.
    pub fn new(data: &'a [u8], origin: usize, stride: usize) -> Self {
        Self {
            data,
            origin,
            stride,
        }
    }

    #[inline]
    pub(crate) fn at(&self, y: i32, x: i32) -> i32 {
        let idx = self.origin as isize + y as isize * self.stride as isize + x as isize;
        self.data[idx as usize] as i32
    }
}

/// `svt_av1_convolve_2d_sr_c` (inter_prediction.c:329).
pub fn convolve_2d_sr(
    src: SrcView<'_>,
    dst: &mut [u8],
    dst_stride: usize,
    w: usize,
    h: usize,
    filter_x: &FilterParams,
    filter_y: &FilterParams,
    subpel_x_q4: i32,
    subpel_y_q4: i32,
    conv_params: &ConvolveParams,
) {
    let im_h = h + SUBPEL_TAPS - 1;
    let im_stride = w;
    let fo_vert = (SUBPEL_TAPS / 2 - 1) as i32;
    let fo_horiz = (SUBPEL_TAPS / 2 - 1) as i32;
    let bd = 8i32;
    let bits = FILTER_BITS * 2 - conv_params.round_0 - conv_params.round_1;

    // Horizontal pass into the 16-bit intermediate. Note the vertical offset
    // uses `fo_vert`, matching `src_horiz = src - fo_vert * src_stride`.
    let mut im_block = alloc::vec![0i16; im_h * im_stride];
    let x_filter = filter_x.subpel_kernel(subpel_x_q4);
    for y in 0..im_h {
        for x in 0..w {
            let mut sum = 1i32 << (bd + FILTER_BITS - 1);
            for k in 0..SUBPEL_TAPS {
                sum +=
                    x_filter[k] as i32 * src.at(y as i32 - fo_vert, x as i32 - fo_horiz + k as i32);
            }
            im_block[y * im_stride + x] = round_power_of_two(sum, conv_params.round_0) as i16;
        }
    }

    // Vertical pass.
    let y_filter = filter_y.subpel_kernel(subpel_y_q4);
    let offset_bits = bd + 2 * FILTER_BITS - conv_params.round_0;
    for y in 0..h {
        for x in 0..w {
            let mut sum = 1i32 << offset_bits;
            for k in 0..SUBPEL_TAPS {
                // `src_vert = im_block + fo_vert * im_stride`, indexed at
                // `(y - fo_vert + k)`, i.e. `im_block[(y + k) * im_stride]`.
                sum += y_filter[k] as i32 * im_block[(y + k) * im_stride + x] as i32;
            }
            // C truncates to `int16_t res` here; the wrap is reproduced.
            let res = (round_power_of_two(sum, conv_params.round_1)
                - ((1 << (offset_bits - conv_params.round_1))
                    + (1 << (offset_bits - conv_params.round_1 - 1)))) as i16;
            dst[y * dst_stride + x] = clip_pixel_8(round_power_of_two(res as i32, bits));
        }
    }
}

/// `svt_av1_convolve_y_sr_c` (inter_prediction.c:374). `subpel_x_q4` and
/// `conv_params` are unused by C and unused here.
pub fn convolve_y_sr(
    src: SrcView<'_>,
    dst: &mut [u8],
    dst_stride: usize,
    w: usize,
    h: usize,
    filter_y: &FilterParams,
    subpel_y_q4: i32,
) {
    let fo_vert = (SUBPEL_TAPS / 2 - 1) as i32;
    let y_filter = filter_y.subpel_kernel(subpel_y_q4);
    for y in 0..h {
        for x in 0..w {
            let mut res = 0i32;
            for k in 0..SUBPEL_TAPS {
                res += y_filter[k] as i32 * src.at(y as i32 - fo_vert + k as i32, x as i32);
            }
            dst[y * dst_stride + x] = clip_pixel_8(round_power_of_two(res, FILTER_BITS));
        }
    }
}

/// `svt_av1_convolve_x_sr_c` (inter_prediction.c:402).
///
/// Two shifts: `round_0` then `bits = FILTER_BITS - round_0`. With the default
/// single-prediction params (`round_0 = 3`) that is 3 then 4, which is NOT the
/// same as one shift by 7 — the intermediate rounding differs.
pub fn convolve_x_sr(
    src: SrcView<'_>,
    dst: &mut [u8],
    dst_stride: usize,
    w: usize,
    h: usize,
    filter_x: &FilterParams,
    subpel_x_q4: i32,
    conv_params: &ConvolveParams,
) {
    let fo_horiz = (SUBPEL_TAPS / 2 - 1) as i32;
    let bits = FILTER_BITS - conv_params.round_0;
    let x_filter = filter_x.subpel_kernel(subpel_x_q4);
    for y in 0..h {
        for x in 0..w {
            let mut res = 0i32;
            for k in 0..SUBPEL_TAPS {
                res += x_filter[k] as i32 * src.at(y as i32, x as i32 - fo_horiz + k as i32);
            }
            res = round_power_of_two(res, conv_params.round_0);
            dst[y * dst_stride + x] = clip_pixel_8(round_power_of_two(res, bits));
        }
    }
}

/// `svt_av1_convolve_2d_copy_sr_c` (inter_prediction.c:431) — the whole-pel
/// path, and the ONLY MC kernel `svt_inter_predictor_pd0` reaches.
pub fn convolve_2d_copy_sr(
    src: SrcView<'_>,
    dst: &mut [u8],
    dst_stride: usize,
    w: usize,
    h: usize,
) {
    for y in 0..h {
        for x in 0..w {
            dst[y * dst_stride + x] = src.at(y as i32, x as i32) as u8;
        }
    }
}

/// The compound blend tail shared by all four `jnt_convolve` kernels: fold
/// `res` into the CONV_BUF value and emit an 8-bit pixel.
#[inline]
fn jnt_average(
    conv_buf_val: u16,
    res: i32,
    round_offset: i32,
    round_bits: i32,
    conv_params: &ConvolveParams,
) -> u8 {
    let mut tmp = conv_buf_val as i32;
    if conv_params.use_jnt_comp_avg {
        tmp = tmp * conv_params.fwd_offset + res * conv_params.bck_offset;
        tmp >>= DIST_PRECISION_BITS;
    } else {
        tmp += res;
        tmp >>= 1;
    }
    tmp -= round_offset;
    clip_pixel_8(round_power_of_two(tmp, round_bits))
}

/// `svt_av1_jnt_convolve_2d_c` (inter_prediction.c:526).
///
/// `conv_buf` is C's `conv_params->dst` (the `CONV_BUF_TYPE` intermediate):
/// read when `do_average`, written otherwise. `dst` is only written when
/// `do_average`.
pub fn jnt_convolve_2d(
    src: SrcView<'_>,
    dst: &mut [u8],
    dst_stride: usize,
    conv_buf: &mut [u16],
    w: usize,
    h: usize,
    filter_x: &FilterParams,
    filter_y: &FilterParams,
    subpel_x_q4: i32,
    subpel_y_q4: i32,
    conv_params: &ConvolveParams,
) {
    let cb_stride = conv_params.dst_stride;
    let im_h = h + SUBPEL_TAPS - 1;
    let im_stride = w;
    let fo_vert = (SUBPEL_TAPS / 2 - 1) as i32;
    let fo_horiz = (SUBPEL_TAPS / 2 - 1) as i32;
    let bd = 8i32;
    let round_bits = 2 * FILTER_BITS - conv_params.round_0 - conv_params.round_1;

    let mut im_block = alloc::vec![0i16; im_h * im_stride];
    let x_filter = filter_x.subpel_kernel(subpel_x_q4);
    for y in 0..im_h {
        for x in 0..w {
            let mut sum = 1i32 << (bd + FILTER_BITS - 1);
            for k in 0..SUBPEL_TAPS {
                sum +=
                    x_filter[k] as i32 * src.at(y as i32 - fo_vert, x as i32 - fo_horiz + k as i32);
            }
            im_block[y * im_stride + x] = round_power_of_two(sum, conv_params.round_0) as i16;
        }
    }

    let y_filter = filter_y.subpel_kernel(subpel_y_q4);
    let offset_bits = bd + 2 * FILTER_BITS - conv_params.round_0;
    let round_offset =
        (1 << (offset_bits - conv_params.round_1)) + (1 << (offset_bits - conv_params.round_1 - 1));
    for y in 0..h {
        for x in 0..w {
            let mut sum = 1i32 << offset_bits;
            for k in 0..SUBPEL_TAPS {
                sum += y_filter[k] as i32 * im_block[(y + k) * im_stride + x] as i32;
            }
            let res = round_power_of_two(sum, conv_params.round_1) as u16;
            if conv_params.do_average {
                dst[y * dst_stride + x] = jnt_average(
                    conv_buf[y * cb_stride + x],
                    res as i32,
                    round_offset,
                    round_bits,
                    conv_params,
                );
            } else {
                conv_buf[y * cb_stride + x] = res;
            }
        }
    }
}

/// `svt_av1_jnt_convolve_y_c` (inter_prediction.c:584).
///
/// `bits = FILTER_BITS - round_0` here; the `_x` twin uses `round_1`.
pub fn jnt_convolve_y(
    src: SrcView<'_>,
    dst: &mut [u8],
    dst_stride: usize,
    conv_buf: &mut [u16],
    w: usize,
    h: usize,
    filter_y: &FilterParams,
    subpel_y_q4: i32,
    conv_params: &ConvolveParams,
) {
    let cb_stride = conv_params.dst_stride;
    let fo_vert = (SUBPEL_TAPS / 2 - 1) as i32;
    let bits = FILTER_BITS - conv_params.round_0;
    let bd = 8i32;
    let offset_bits = bd + 2 * FILTER_BITS - conv_params.round_0;
    let round_offset =
        (1 << (offset_bits - conv_params.round_1)) + (1 << (offset_bits - conv_params.round_1 - 1));
    let round_bits = 2 * FILTER_BITS - conv_params.round_0 - conv_params.round_1;

    let y_filter = filter_y.subpel_kernel(subpel_y_q4);
    for y in 0..h {
        for x in 0..w {
            let mut res = 0i32;
            for k in 0..SUBPEL_TAPS {
                res += y_filter[k] as i32 * src.at(y as i32 - fo_vert + k as i32, x as i32);
            }
            res *= 1 << bits;
            res = round_power_of_two(res, conv_params.round_1) + round_offset;

            if conv_params.do_average {
                dst[y * dst_stride + x] = jnt_average(
                    conv_buf[y * cb_stride + x],
                    res,
                    round_offset,
                    round_bits,
                    conv_params,
                );
            } else {
                // C stores through `(ConvBufType)res` — a u16 truncation.
                conv_buf[y * cb_stride + x] = res as u16;
            }
        }
    }
}

/// `svt_av1_jnt_convolve_x_c` (inter_prediction.c:629).
///
/// `bits = FILTER_BITS - round_1` (the `_y` twin uses `round_0`) — upstream
/// asymmetry, reproduced verbatim.
pub fn jnt_convolve_x(
    src: SrcView<'_>,
    dst: &mut [u8],
    dst_stride: usize,
    conv_buf: &mut [u16],
    w: usize,
    h: usize,
    filter_x: &FilterParams,
    subpel_x_q4: i32,
    conv_params: &ConvolveParams,
) {
    let cb_stride = conv_params.dst_stride;
    let fo_horiz = (SUBPEL_TAPS / 2 - 1) as i32;
    let bits = FILTER_BITS - conv_params.round_1;
    let bd = 8i32;
    let offset_bits = bd + 2 * FILTER_BITS - conv_params.round_0;
    let round_offset =
        (1 << (offset_bits - conv_params.round_1)) + (1 << (offset_bits - conv_params.round_1 - 1));
    let round_bits = 2 * FILTER_BITS - conv_params.round_0 - conv_params.round_1;

    let x_filter = filter_x.subpel_kernel(subpel_x_q4);
    for y in 0..h {
        for x in 0..w {
            let mut res = 0i32;
            for k in 0..SUBPEL_TAPS {
                res += x_filter[k] as i32 * src.at(y as i32, x as i32 - fo_horiz + k as i32);
            }
            res = (1 << bits) * round_power_of_two(res, conv_params.round_0);
            res += round_offset;

            if conv_params.do_average {
                dst[y * dst_stride + x] = jnt_average(
                    conv_buf[y * cb_stride + x],
                    res,
                    round_offset,
                    round_bits,
                    conv_params,
                );
            } else {
                conv_buf[y * cb_stride + x] = res as u16;
            }
        }
    }
}

/// `svt_av1_jnt_convolve_2d_copy_c` (inter_prediction.c:674).
///
/// C's `res` is a `ConvBufType` (u16), so `src << bits` and the `round_offset`
/// add both wrap at 16 bits before `do_average` re-widens. With the compound
/// defaults (`round_0 = 3`, `round_1 = 7`) `bits = 4` and `round_offset` is
/// `(1 << 11) + (1 << 10) = 3072`, so `255 << 4 = 4080` plus 3072 stays inside
/// 16 bits — but the wrap is modelled rather than assumed away.
pub fn jnt_convolve_2d_copy(
    src: SrcView<'_>,
    dst: &mut [u8],
    dst_stride: usize,
    conv_buf: &mut [u16],
    w: usize,
    h: usize,
    conv_params: &ConvolveParams,
) {
    let cb_stride = conv_params.dst_stride;
    let bits = FILTER_BITS * 2 - conv_params.round_1 - conv_params.round_0;
    let bd = 8i32;
    let offset_bits = bd + 2 * FILTER_BITS - conv_params.round_0;
    let round_offset =
        (1 << (offset_bits - conv_params.round_1)) + (1 << (offset_bits - conv_params.round_1 - 1));

    for y in 0..h {
        for x in 0..w {
            let mut res = (src.at(y as i32, x as i32) as u16) << bits;
            res = res.wrapping_add(round_offset as u16);
            if conv_params.do_average {
                dst[y * dst_stride + x] = jnt_average(
                    conv_buf[y * cb_stride + x],
                    res as i32,
                    round_offset,
                    bits,
                    conv_params,
                );
            } else {
                conv_buf[y * cb_stride + x] = res;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `get_conv_params_no_round`'s documented arms, checked against the C
    /// constants: single prediction is 3/11, compound is 3/7, and the
    /// `intbufrange` correction fires only at bd 12.
    #[test]
    fn conv_params_rounds() {
        let single = ConvolveParams::single(false, 8);
        assert_eq!((single.round_0, single.round_1), (3, 11));
        let compound = ConvolveParams::no_round(false, 64, true, 8);
        assert_eq!((compound.round_0, compound.round_1), (3, 7));
        let bd10 = ConvolveParams::single(false, 10);
        assert_eq!((bd10.round_0, bd10.round_1), (3, 11));
        // bd 12: intbufrange = 12 + 7 - 3 + 2 = 18 > 16, so +2 / -2.
        let bd12 = ConvolveParams::single(false, 12);
        assert_eq!((bd12.round_0, bd12.round_1), (5, 9));
        let bd12c = ConvolveParams::no_round(false, 64, true, 12);
        assert_eq!((bd12c.round_0, bd12c.round_1), (5, 7));
    }

    /// The narrow-block substitution maps SHARP onto the *regular* 4-tap
    /// table, which is the arm a "sharp -> sharp" assumption gets wrong.
    #[test]
    fn narrow_block_filter_substitution() {
        let sharp4 = interp_filter_params_with_block_size(InterpFilterKind::MultiTapSharp, 4);
        assert_eq!(sharp4.kernels[1], SUB_PEL_FILTERS_4[1]);
        let sharp8 = interp_filter_params_with_block_size(InterpFilterKind::MultiTapSharp, 8);
        assert_eq!(sharp8.kernels[1], SUB_PEL_FILTERS_8SHARP[1]);
        let smooth4 = interp_filter_params_with_block_size(InterpFilterKind::EightTapSmooth, 4);
        assert_eq!(smooth4.kernels[1], SUB_PEL_FILTERS_4SMOOTH[1]);
        // BILINEAR is never substituted.
        let bil4 = interp_filter_params_with_block_size(InterpFilterKind::Bilinear, 4);
        assert_eq!(bil4.kernels[1], BILINEAR_FILTERS[1]);
    }

    /// Both 4-tap tables normalize to 128 like the 8-tap ones.
    #[test]
    fn four_tap_tables_sum_to_128() {
        for (name, t) in [
            ("sub_pel_filters_4", &SUB_PEL_FILTERS_4),
            ("sub_pel_filters_4smooth", &SUB_PEL_FILTERS_4SMOOTH),
        ] {
            for (phase, k) in t.iter().enumerate() {
                let s: i32 = k.iter().map(|&v| v as i32).sum();
                assert_eq!(s, 128, "{name} phase {phase} sums to {s}");
            }
        }
    }
}
