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

use archmage::prelude::*;
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

    let x_filter = filter_x.subpel_kernel(subpel_x_q4);
    #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
    if w >= 16 {
        if let Some(token) = X64V4Token::summon() {
            convolve_2d_sr_v4(
                token,
                src,
                dst,
                dst_stride,
                w,
                h,
                x_filter,
                filter_y.subpel_kernel(subpel_y_q4),
                conv_params.round_0,
                conv_params.round_1,
                bits,
            );
            return;
        }
    }
    #[cfg(target_arch = "x86_64")]
    if let Some(token) = X64V3Token::summon() {
        convolve_2d_sr_v3(
            token,
            src,
            dst,
            dst_stride,
            w,
            h,
            x_filter,
            filter_y.subpel_kernel(subpel_y_q4),
            conv_params.round_0,
            conv_params.round_1,
            bits,
        );
        return;
    }

    // Horizontal pass into the 16-bit intermediate. Note the vertical offset
    // uses `fo_vert`, matching `src_horiz = src - fo_vert * src_stride`.
    let mut im_block = alloc::vec![0i16; im_h * im_stride];
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

/// One horizontal-pass intermediate pixel of [`convolve_2d_sr`].
#[inline(always)]
fn convolve_2d_sr_hpx(
    src: SrcView<'_>,
    y: usize,
    x: usize,
    x_filter: &InterpKernel,
    fo_vert: i32,
    fo_horiz: i32,
    round_0: i32,
) -> i16 {
    let mut sum = 1i32 << (8 + FILTER_BITS - 1);
    for k in 0..SUBPEL_TAPS {
        sum += x_filter[k] as i32 * src.at(y as i32 - fo_vert, x as i32 - fo_horiz + k as i32);
    }
    round_power_of_two(sum, round_0) as i16
}

/// One output pixel of [`convolve_2d_sr`]'s vertical pass.
#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn convolve_2d_sr_vpx(
    im_block: &[i16],
    im_stride: usize,
    y: usize,
    x: usize,
    y_filter: &InterpKernel,
    offset_bits: i32,
    round_1: i32,
    bits: i32,
) -> u8 {
    let mut sum = 1i32 << offset_bits;
    for k in 0..SUBPEL_TAPS {
        sum += y_filter[k] as i32 * im_block[(y + k) * im_stride + x] as i32;
    }
    let res = (round_power_of_two(sum, round_1)
        - ((1 << (offset_bits - round_1)) + (1 << (offset_bits - round_1 - 1))))
        as i16;
    clip_pixel_8(round_power_of_two(res as i32, bits))
}

/// x86-64 v3 arm of [`convolve_2d_sr`]. Horizontal pass: same 8-wide
/// shifted-tap trick as [`convolve_x_sr_v3`], storing the `as i16`
/// truncation through `to_array` (the truncation is load-bearing — values
/// always fit, but the semantics are C's `int16_t` store, not saturation).
/// Vertical pass: each tap row is a contiguous `i16x8` load, products
/// widened to `i32x4` pairs (|im| can exceed what an `i16` product holds),
/// the final `as i16` + second rounding per lane.
#[cfg(target_arch = "x86_64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn convolve_2d_sr_v3(
    token: X64V3Token,
    src: SrcView<'_>,
    dst: &mut [u8],
    dst_stride: usize,
    w: usize,
    h: usize,
    x_filter: &InterpKernel,
    y_filter: &InterpKernel,
    round_0: i32,
    round_1: i32,
    bits: i32,
) {
    use magetypes::simd::generic::{i16x8, i32x4, u8x16};
    let im_h = h + SUBPEL_TAPS - 1;
    let fo_vert = (SUBPEL_TAPS / 2 - 1) as isize;
    let fo_horiz = (SUBPEL_TAPS / 2 - 1) as isize;
    let len = src.data.len() as isize;
    let origin = src.origin as isize;
    let stride = src.stride as isize;
    let offset_bits = 8 + 2 * FILTER_BITS - round_0;

    let mut im_block = alloc::vec![0i16; im_h * w];
    let cfs_x: [i16x8<_>; SUBPEL_TAPS] = core::array::from_fn(|i| i16x8::splat(token, x_filter[i]));
    // C adds `1 << (bd + FILTER_BITS - 1)` into the dot-product sum and THEN
    // `round_power_of_two(.., round_0)` contributes its own `1 << (r0 - 1)`.
    let off0 = i32x4::splat(
        token,
        (1 << (8 + FILTER_BITS - 1)) + if round_0 > 0 { 1 << (round_0 - 1) } else { 0 },
    );
    for y in 0..im_h {
        let row = origin + (y as isize - fo_vert) * stride;
        let x0 = (fo_horiz - row).max(0).min(w as isize) as usize;
        let x1 = ((len - 16 - 7 + fo_horiz - row).min(w as isize)).max(x0 as isize) as usize;
        let imrow = &mut im_block[y * w..y * w + w];
        for x in 0..x0 {
            imrow[x] = convolve_2d_sr_hpx(
                src,
                y,
                x,
                x_filter,
                fo_vert as i32,
                fo_horiz as i32,
                round_0,
            );
        }
        let mut x = x0;
        while x + 8 <= x1 {
            let mut lo = i32x4::splat(token, 0);
            let mut hi = i32x4::splat(token, 0);
            for (k, cf) in cfs_x.iter().enumerate() {
                let i = (row + x as isize - fo_horiz + k as isize) as usize;
                let v = u8x16::load(token, src.data[i..i + 16].try_into().unwrap())
                    .widen_low()
                    .bitcast_i16x8();
                let p = v * *cf;
                lo += p.widen_low();
                hi += p.widen_high();
            }
            let rl = (lo + off0)
                .shr_arithmetic_uniform(round_0 as u32)
                .to_array();
            let rh = (hi + off0)
                .shr_arithmetic_uniform(round_0 as u32)
                .to_array();
            for j in 0..4 {
                imrow[x + j] = rl[j] as i16;
                imrow[x + 4 + j] = rh[j] as i16;
            }
            x += 8;
        }
        for x in x..w {
            imrow[x] = convolve_2d_sr_hpx(
                src,
                y,
                x,
                x_filter,
                fo_vert as i32,
                fo_horiz as i32,
                round_0,
            );
        }
    }

    // Vertical pass — the i16 intermediate means tap loads need no widening
    // from u8, but products widen to i32 (|im| is ~14-bit).
    let cfs_y: [i32x4<_>; SUBPEL_TAPS] =
        core::array::from_fn(|i| i32x4::splat(token, y_filter[i] as i32));
    let offv = i32x4::splat(
        token,
        (1 << offset_bits) + if round_1 > 0 { 1 << (round_1 - 1) } else { 0 },
    );
    let corr = (1 << (offset_bits - round_1)) + (1 << (offset_bits - round_1 - 1));
    for y in 0..h {
        let drow = &mut dst[y * dst_stride..y * dst_stride + w];
        let mut x = 0usize;
        while x + 8 <= w {
            let mut lo = i32x4::splat(token, 0);
            let mut hi = i32x4::splat(token, 0);
            for (k, cf) in cfs_y.iter().enumerate() {
                let v = i16x8::load(
                    token,
                    im_block[(y + k) * w + x..(y + k) * w + x + 8]
                        .try_into()
                        .unwrap(),
                );
                lo += v.widen_low() * *cf;
                hi += v.widen_high() * *cf;
            }
            let rl = (lo + offv)
                .shr_arithmetic_uniform(round_1 as u32)
                .to_array();
            let rh = (hi + offv)
                .shr_arithmetic_uniform(round_1 as u32)
                .to_array();
            for j in 0..4 {
                let res = (rl[j] - corr) as i16;
                drow[x + j] = clip_pixel_8(round_power_of_two(res as i32, bits));
                let res = (rh[j] - corr) as i16;
                drow[x + 4 + j] = clip_pixel_8(round_power_of_two(res as i32, bits));
            }
            x += 8;
        }
        for x in x..w {
            drow[x] = convolve_2d_sr_vpx(&im_block, w, y, x, y_filter, offset_bits, round_1, bits);
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
    #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
    if w >= 16 {
        if let Some(token) = X64V4Token::summon() {
            convolve_y_sr_v4(token, src, dst, dst_stride, w, h, y_filter);
            return;
        }
    }
    #[cfg(target_arch = "x86_64")]
    if let Some(token) = X64V3Token::summon() {
        convolve_y_sr_v3(token, src, dst, dst_stride, w, h, y_filter);
        return;
    }
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

/// x86-64 v3 arm of [`convolve_y_sr`]: 8 columns at a time. The tap rows are
/// contiguous in `x`, so each of the 8 taps is one `u8x16` load widened to
/// `i16x8` — the `i16` product `src * coeff` is exact (|coeff| <= 128 and
/// src <= 255 keep |product| <= 32640), and the two `i32x4` widened halves
/// accumulate the tap sum. Round-shift, then the saturating narrows ARE
/// `clip_pixel_8`.
///
/// Each tap load reads 16 bytes where 8 are needed, so columns whose window
/// would leave `data` (and the last partial chunk) take the scalar pixel.
#[cfg(target_arch = "x86_64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn convolve_y_sr_v3(
    token: X64V3Token,
    src: SrcView<'_>,
    dst: &mut [u8],
    dst_stride: usize,
    w: usize,
    h: usize,
    y_filter: &InterpKernel,
) {
    use magetypes::simd::generic::{i16x8, i32x4, u8x16};
    let fo = (SUBPEL_TAPS / 2 - 1) as isize;
    let len = src.data.len() as isize;
    let origin = src.origin as isize;
    let stride = src.stride as isize;
    let cfs: [i16x8<_>; SUBPEL_TAPS] = core::array::from_fn(|i| i16x8::splat(token, y_filter[i]));
    for y in 0..h {
        // First tap row y - fo, last y - fo + 7.
        let row0 = origin + (y as isize - fo) * stride;
        let row7 = row0 + 7 * stride;
        // Column window: every tap row's `x .. x + 16` load must stay inside
        // `data`.
        let x0 = (-row0).max(0).min(w as isize) as usize;
        let x1 = ((len - 16 - row7).min(w as isize)).max(x0 as isize) as usize;
        let drow = &mut dst[y * dst_stride..y * dst_stride + w];
        for x in 0..x0 {
            let mut res = 0i32;
            for k in 0..SUBPEL_TAPS {
                res += y_filter[k] as i32 * src.at(y as i32 - fo as i32 + k as i32, x as i32);
            }
            drow[x] = clip_pixel_8(round_power_of_two(res, FILTER_BITS));
        }
        let mut x = x0;
        while x + 8 <= x1 {
            let mut lo = i32x4::splat(token, 0);
            let mut hi = i32x4::splat(token, 0);
            for (k, cf) in cfs.iter().enumerate() {
                let i = (row0 + k as isize * stride + x as isize) as usize;
                let v = u8x16::load(token, src.data[i..i + 16].try_into().unwrap())
                    .widen_low()
                    .bitcast_i16x8();
                let p = v * *cf;
                lo += p.widen_low();
                hi += p.widen_high();
            }
            let res_lo = (lo + i32x4::splat(token, 64)).shr_arithmetic_uniform(7);
            let res_hi = (hi + i32x4::splat(token, 64)).shr_arithmetic_uniform(7);
            let packed = res_lo
                .narrow_saturating_i16(res_hi)
                .narrow_saturating_u8(i16x8::splat(token, 0));
            let mut tmp = [0u8; 16];
            packed.store(&mut tmp);
            drow[x..x + 8].copy_from_slice(&tmp[..8]);
            x += 8;
        }
        for x in x..w {
            let mut res = 0i32;
            for k in 0..SUBPEL_TAPS {
                res += y_filter[k] as i32 * src.at(y as i32 - fo as i32 + k as i32, x as i32);
            }
            drow[x] = clip_pixel_8(round_power_of_two(res, FILTER_BITS));
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
    #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
    if w >= 16 {
        if let Some(token) = X64V4Token::summon() {
            convolve_x_sr_v4(
                token,
                src,
                dst,
                dst_stride,
                w,
                h,
                x_filter,
                conv_params.round_0,
                bits,
            );
            return;
        }
    }
    #[cfg(target_arch = "x86_64")]
    if let Some(token) = X64V3Token::summon() {
        convolve_x_sr_v3(
            token,
            src,
            dst,
            dst_stride,
            w,
            h,
            x_filter,
            conv_params.round_0,
            bits,
        );
        return;
    }
    convolve_x_sr_scalar(
        src,
        dst,
        dst_stride,
        w,
        h,
        x_filter,
        fo_horiz,
        conv_params.round_0,
        bits,
    );
}

/// One output pixel of [`convolve_x_sr`]: the 8-tap horizontal dot and the
/// `round_0` then `bits` two-step rounding.
#[inline(always)]
fn convolve_x_sr_px(
    src: SrcView<'_>,
    y: usize,
    x: usize,
    x_filter: &InterpKernel,
    fo_horiz: i32,
    round_0: i32,
    bits: i32,
) -> u8 {
    let mut res = 0i32;
    for k in 0..SUBPEL_TAPS {
        res += x_filter[k] as i32 * src.at(y as i32, x as i32 - fo_horiz + k as i32);
    }
    res = round_power_of_two(res, round_0);
    clip_pixel_8(round_power_of_two(res, bits))
}

/// Scalar body of [`convolve_x_sr`], verbatim C.
#[allow(clippy::too_many_arguments)]
fn convolve_x_sr_scalar(
    src: SrcView<'_>,
    dst: &mut [u8],
    dst_stride: usize,
    w: usize,
    h: usize,
    x_filter: &InterpKernel,
    fo_horiz: i32,
    round_0: i32,
    bits: i32,
) {
    for y in 0..h {
        for x in 0..w {
            dst[y * dst_stride + x] =
                convolve_x_sr_px(src, y, x, x_filter, fo_horiz, round_0, bits);
        }
    }
}

/// x86-64 v3 arm of [`convolve_x_sr`]: 8 output pixels at a time. For tap
/// `k`, output `x + i` reads `src[x - fo + k + i]` — contiguous in `i` — so
/// one shifted `u8x16` load per tap covers the whole group. Each `i16`
/// product `src * coeff` is exact (|coeff| <= 128, src <= 255), the two
/// `i32x4` widened halves accumulate the 8-tap sum, and the saturating
/// narrows ARE `clip_pixel_8` after the `round_0`/`bits` two-step rounding.
///
/// The loads reach `x - fo + 7 + 15`, so column groups whose window would
/// leave `data` take the scalar pixel.
#[cfg(target_arch = "x86_64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn convolve_x_sr_v3(
    token: X64V3Token,
    src: SrcView<'_>,
    dst: &mut [u8],
    dst_stride: usize,
    w: usize,
    h: usize,
    x_filter: &InterpKernel,
    round_0: i32,
    bits: i32,
) {
    use magetypes::simd::generic::{i16x8, i32x4, u8x16};
    let cfs: [i16x8<_>; SUBPEL_TAPS] = core::array::from_fn(|i| i16x8::splat(token, x_filter[i]));
    let fo = (SUBPEL_TAPS / 2 - 1) as isize;
    let len = src.data.len() as isize;
    let origin = src.origin as isize;
    let stride = src.stride as isize;
    let r0_off = i32x4::splat(token, 1 << (round_0 - 1));
    let b_off = i32x4::splat(token, 1 << (bits - 1));
    for y in 0..h {
        let row = origin + y as isize * stride;
        // Vector range: every tap load `row + x - fo + k .. + 16`, k <= 7,
        // must stay inside `data`.
        let x0 = (fo - row).max(0).min(w as isize) as usize;
        let x1 = ((len - 16 - 7 + fo - row).min(w as isize)).max(x0 as isize) as usize;
        let drow = &mut dst[y * dst_stride..y * dst_stride + w];
        for x in 0..x0 {
            drow[x] = convolve_x_sr_px(src, y, x, x_filter, fo as i32, round_0, bits);
        }
        let mut x = x0;
        while x + 8 <= x1 {
            let mut lo = i32x4::splat(token, 0);
            let mut hi = i32x4::splat(token, 0);
            for (k, cf) in cfs.iter().enumerate() {
                let i = (row + x as isize - fo + k as isize) as usize;
                let v = u8x16::load(token, src.data[i..i + 16].try_into().unwrap())
                    .widen_low()
                    .bitcast_i16x8();
                let p = v * *cf;
                lo += p.widen_low();
                hi += p.widen_high();
            }
            let res_lo = ((lo + r0_off).shr_arithmetic_uniform(round_0 as u32) + b_off)
                .shr_arithmetic_uniform(bits as u32);
            let res_hi = ((hi + r0_off).shr_arithmetic_uniform(round_0 as u32) + b_off)
                .shr_arithmetic_uniform(bits as u32);
            let packed = res_lo
                .narrow_saturating_i16(res_hi)
                .narrow_saturating_u8(i16x8::splat(token, 0));
            let mut tmp = [0u8; 16];
            packed.store(&mut tmp);
            drow[x..x + 8].copy_from_slice(&tmp[..8]);
            x += 8;
        }
        for x in x..w {
            drow[x] = convolve_x_sr_px(src, y, x, x_filter, fo as i32, round_0, bits);
        }
    }
}

// --- AVX-512 (`_v4`) arms ---
//
// Same lanewise pipelines as the `_v3` arms at 32 output pixels per
// iteration: each tap is a 32-byte `_mm256_loadu_si256` widened by
// `_mm512_cvtepu8_epi16`, an exact `i16` `mullo`, then both halves widened
// to `i32x16` for the tap-sum accumulator. Rounding uses per-lane `sra`
// (the shift amounts are runtime values, so `srav`), and the final
// `clip_pixel_8` is an ORDERED narrow — `vpmovsdw`/`vpmovuswb` truncate in
// lane order, so no `packs` lane fixup is ever needed.
//
// The public dispatchers only send `w >= 16` here: narrower blocks keep the
// `_v3` arm. Each kernel runs a 32-px zmm loop, a 16-px rung (xmm load ->
// ymm widen -> a single i32x16 accumulator), then a scalar tail.

/// Narrow two `i32x16` partial results (pixels `x..x+16` and `x+16..x+32`)
/// to `u8x32` with `clip_pixel_8` semantics — ordered, no lane interleave.
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
#[rite]
fn pack_u8x32_v4(_token: X64V4Token, lo: __m512i, hi: __m512i) -> __m256i {
    let t16 = _mm512_inserti64x4::<1>(
        _mm512_castsi256_si512(_mm512_cvtsepi32_epi16(lo)),
        _mm512_cvtsepi32_epi16(hi),
    );
    _mm512_cvtusepi16_epi8(_mm512_max_epi16(t16, _mm512_setzero_si512()))
}

/// Narrow one `i32x16` partial result (pixels `x..x+16`) to `u8x16` with
/// `clip_pixel_8` semantics — ordered.
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
#[rite]
fn pack_u8x16_v4(_token: X64V4Token, lo: __m512i) -> __m128i {
    let t16 = _mm512_cvtsepi32_epi16(lo);
    _mm256_cvtusepi16_epi8(_mm256_max_epi16(t16, _mm256_setzero_si256()))
}

/// Truncate two `i32x16` partial results to `i16x32` — the `as i16` store
/// the 2D horizontal pass writes into `im_block`.
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
#[rite]
fn trunc_i16x32_v4(_token: X64V4Token, lo: __m512i, hi: __m512i) -> __m512i {
    _mm512_inserti64x4::<1>(
        _mm512_castsi256_si512(_mm512_cvtepi32_epi16(lo)),
        _mm512_cvtepi32_epi16(hi),
    )
}

/// x86-64 v4 arm of [`convolve_x_sr`]: 32 output pixels per iteration.
/// Only reached for `w >= 32` (see the dispatcher), so the tail after the
/// 32-wide loop is scalar.
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn convolve_x_sr_v4(
    token: X64V4Token,
    src: SrcView<'_>,
    dst: &mut [u8],
    dst_stride: usize,
    w: usize,
    h: usize,
    x_filter: &InterpKernel,
    round_0: i32,
    bits: i32,
) {
    let cfs: [__m512i; SUBPEL_TAPS] = core::array::from_fn(|i| _mm512_set1_epi16(x_filter[i]));
    let fo = (SUBPEL_TAPS / 2 - 1) as isize;
    let len = src.data.len() as isize;
    let origin = src.origin as isize;
    let stride = src.stride as isize;
    let r0_off = _mm512_set1_epi32(1 << (round_0 - 1));
    // `round_power_of_two(v, 0) = v` — bits can be 0 for single prediction.
    let b_off = _mm512_set1_epi32(if bits > 0 { 1 << (bits - 1) } else { 0 });
    let r0v = _mm512_set1_epi32(round_0);
    let bv = _mm512_set1_epi32(bits.max(0));
    for y in 0..h {
        let row = origin + y as isize * stride;
        // Vector range: every tap load `row + x - fo + k .. + V`, k <= 7,
        // must stay inside `data`; the two rungs have separate bounds.
        let x0 = (fo - row).max(0).min(w as isize) as usize;
        let x1_32 = ((len - 32 - 7 + fo - row).min(w as isize)).max(x0 as isize) as usize;
        let x1_16 = ((len - 16 - 7 + fo - row).min(w as isize)).max(x0 as isize) as usize;
        let drow = &mut dst[y * dst_stride..y * dst_stride + w];
        for x in 0..x0 {
            drow[x] = convolve_x_sr_px(src, y, x, x_filter, fo as i32, round_0, bits);
        }
        let mut x = x0;
        while x + 32 <= x1_32 {
            let mut lo = _mm512_setzero_si512();
            let mut hi = _mm512_setzero_si512();
            for (k, cf) in cfs.iter().enumerate() {
                let i = (row + x as isize - fo + k as isize) as usize;
                let s: &[u8; 32] = src.data[i..i + 32].try_into().unwrap();
                let v = _mm512_cvtepu8_epi16(_mm256_loadu_si256(s));
                let p = _mm512_mullo_epi16(v, *cf);
                lo = _mm512_add_epi32(lo, _mm512_cvtepi16_epi32(_mm512_castsi512_si256(p)));
                hi = _mm512_add_epi32(hi, _mm512_cvtepi16_epi32(_mm512_extracti64x4_epi64::<1>(p)));
            }
            let res_lo = _mm512_srav_epi32(
                _mm512_add_epi32(_mm512_srav_epi32(_mm512_add_epi32(lo, r0_off), r0v), b_off),
                bv,
            );
            let res_hi = _mm512_srav_epi32(
                _mm512_add_epi32(_mm512_srav_epi32(_mm512_add_epi32(hi, r0_off), r0v), b_off),
                bv,
            );
            let px = pack_u8x32_v4(token, res_lo, res_hi);
            let out: &mut [u8; 32] = (&mut drow[x..x + 32]).try_into().unwrap();
            _mm256_storeu_si256(out, px);
            x += 32;
        }
        while x + 16 <= x1_16 {
            let mut acc = _mm512_setzero_si512();
            for (k, cf) in cfs.iter().enumerate() {
                let i = (row + x as isize - fo + k as isize) as usize;
                let s: &[u8; 16] = src.data[i..i + 16].try_into().unwrap();
                let v = _mm256_cvtepu8_epi16(_mm_loadu_si128(s));
                let p = _mm512_cvtepi16_epi32(_mm256_mullo_epi16(v, _mm512_castsi512_si256(*cf)));
                acc = _mm512_add_epi32(acc, p);
            }
            let res = _mm512_srav_epi32(
                _mm512_add_epi32(_mm512_srav_epi32(_mm512_add_epi32(acc, r0_off), r0v), b_off),
                bv,
            );
            let px = pack_u8x16_v4(token, res);
            let out: &mut [u8; 16] = (&mut drow[x..x + 16]).try_into().unwrap();
            _mm_storeu_si128(out, px);
            x += 16;
        }
        for x in x..w {
            drow[x] = convolve_x_sr_px(src, y, x, x_filter, fo as i32, round_0, bits);
        }
    }
}

/// x86-64 v4 arm of [`convolve_y_sr`]: 32 columns at a time, tap rows are
/// contiguous 32-byte loads. Only reached for `w >= 32`.
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
#[arcane]
fn convolve_y_sr_v4(
    token: X64V4Token,
    src: SrcView<'_>,
    dst: &mut [u8],
    dst_stride: usize,
    w: usize,
    h: usize,
    y_filter: &InterpKernel,
) {
    let fo = (SUBPEL_TAPS / 2 - 1) as isize;
    let len = src.data.len() as isize;
    let origin = src.origin as isize;
    let stride = src.stride as isize;
    let cfs: [__m512i; SUBPEL_TAPS] = core::array::from_fn(|i| _mm512_set1_epi16(y_filter[i]));
    let off64 = _mm512_set1_epi32(64);
    let seven = _mm512_set1_epi32(7);
    for y in 0..h {
        let row0 = origin + (y as isize - fo) * stride;
        let row7 = row0 + 7 * stride;
        // Column window: every tap row's `x .. x + V` load must stay inside
        // `data`; the two rungs have separate bounds.
        let x0 = (-row0).max(0).min(w as isize) as usize;
        let x1_32 = ((len - 32 - row7).min(w as isize)).max(x0 as isize) as usize;
        let x1_16 = ((len - 16 - row7).min(w as isize)).max(x0 as isize) as usize;
        let drow = &mut dst[y * dst_stride..y * dst_stride + w];
        for x in 0..x0 {
            let mut res = 0i32;
            for k in 0..SUBPEL_TAPS {
                res += y_filter[k] as i32 * src.at(y as i32 - fo as i32 + k as i32, x as i32);
            }
            drow[x] = clip_pixel_8(round_power_of_two(res, FILTER_BITS));
        }
        let mut x = x0;
        while x + 32 <= x1_32 {
            let mut lo = _mm512_setzero_si512();
            let mut hi = _mm512_setzero_si512();
            for (k, cf) in cfs.iter().enumerate() {
                let i = (row0 + k as isize * stride + x as isize) as usize;
                let s: &[u8; 32] = src.data[i..i + 32].try_into().unwrap();
                let v = _mm512_cvtepu8_epi16(_mm256_loadu_si256(s));
                let p = _mm512_mullo_epi16(v, *cf);
                lo = _mm512_add_epi32(lo, _mm512_cvtepi16_epi32(_mm512_castsi512_si256(p)));
                hi = _mm512_add_epi32(hi, _mm512_cvtepi16_epi32(_mm512_extracti64x4_epi64::<1>(p)));
            }
            let res_lo = _mm512_srav_epi32(_mm512_add_epi32(lo, off64), seven);
            let res_hi = _mm512_srav_epi32(_mm512_add_epi32(hi, off64), seven);
            let px = pack_u8x32_v4(token, res_lo, res_hi);
            let out: &mut [u8; 32] = (&mut drow[x..x + 32]).try_into().unwrap();
            _mm256_storeu_si256(out, px);
            x += 32;
        }
        while x + 16 <= x1_16 {
            let mut acc = _mm512_setzero_si512();
            for (k, cf) in cfs.iter().enumerate() {
                let i = (row0 + k as isize * stride + x as isize) as usize;
                let s: &[u8; 16] = src.data[i..i + 16].try_into().unwrap();
                let v = _mm256_cvtepu8_epi16(_mm_loadu_si128(s));
                let p = _mm512_cvtepi16_epi32(_mm256_mullo_epi16(v, _mm512_castsi512_si256(*cf)));
                acc = _mm512_add_epi32(acc, p);
            }
            let res = _mm512_srav_epi32(_mm512_add_epi32(acc, off64), seven);
            let px = pack_u8x16_v4(token, res);
            let out: &mut [u8; 16] = (&mut drow[x..x + 16]).try_into().unwrap();
            _mm_storeu_si128(out, px);
            x += 16;
        }
        for x in x..w {
            let mut res = 0i32;
            for k in 0..SUBPEL_TAPS {
                res += y_filter[k] as i32 * src.at(y as i32 - fo as i32 + k as i32, x as i32);
            }
            drow[x] = clip_pixel_8(round_power_of_two(res, FILTER_BITS));
        }
    }
}

/// x86-64 v4 arm of [`convolve_2d_sr`]: both passes at 32 columns per
/// iteration. Only reached for `w >= 32`. The h-pass writes `i16x32` via
/// `vpmovdw` (the `as i16` truncation), and the whole intermediate block is
//  committed before the v-pass reads it — the passes never form a
/// per-row store->load handoff.
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn convolve_2d_sr_v4(
    token: X64V4Token,
    src: SrcView<'_>,
    dst: &mut [u8],
    dst_stride: usize,
    w: usize,
    h: usize,
    x_filter: &InterpKernel,
    y_filter: &InterpKernel,
    round_0: i32,
    round_1: i32,
    bits: i32,
) {
    let im_h = h + SUBPEL_TAPS - 1;
    let fo_vert = (SUBPEL_TAPS / 2 - 1) as isize;
    let fo_horiz = (SUBPEL_TAPS / 2 - 1) as isize;
    let len = src.data.len() as isize;
    let origin = src.origin as isize;
    let stride = src.stride as isize;
    let offset_bits = 8 + 2 * FILTER_BITS - round_0;

    let mut im_block = alloc::vec![0i16; im_h * w];
    let cfs_x: [__m512i; SUBPEL_TAPS] = core::array::from_fn(|i| _mm512_set1_epi16(x_filter[i]));
    let off0 = _mm512_set1_epi32(
        (1 << (8 + FILTER_BITS - 1)) + if round_0 > 0 { 1 << (round_0 - 1) } else { 0 },
    );
    let r0v = _mm512_set1_epi32(round_0);
    for y in 0..im_h {
        let row = origin + (y as isize - fo_vert) * stride;
        let x0 = (fo_horiz - row).max(0).min(w as isize) as usize;
        let x1_32 = ((len - 32 - 7 + fo_horiz - row).min(w as isize)).max(x0 as isize) as usize;
        let x1_16 = ((len - 16 - 7 + fo_horiz - row).min(w as isize)).max(x0 as isize) as usize;
        let imrow = &mut im_block[y * w..y * w + w];
        for x in 0..x0 {
            imrow[x] = convolve_2d_sr_hpx(
                src,
                y,
                x,
                x_filter,
                fo_vert as i32,
                fo_horiz as i32,
                round_0,
            );
        }
        let mut x = x0;
        while x + 32 <= x1_32 {
            let mut lo = _mm512_setzero_si512();
            let mut hi = _mm512_setzero_si512();
            for (k, cf) in cfs_x.iter().enumerate() {
                let i = (row + x as isize - fo_horiz + k as isize) as usize;
                let s: &[u8; 32] = src.data[i..i + 32].try_into().unwrap();
                let v = _mm512_cvtepu8_epi16(_mm256_loadu_si256(s));
                let p = _mm512_mullo_epi16(v, *cf);
                lo = _mm512_add_epi32(lo, _mm512_cvtepi16_epi32(_mm512_castsi512_si256(p)));
                hi = _mm512_add_epi32(hi, _mm512_cvtepi16_epi32(_mm512_extracti64x4_epi64::<1>(p)));
            }
            let rl = _mm512_srav_epi32(_mm512_add_epi32(lo, off0), r0v);
            let rh = _mm512_srav_epi32(_mm512_add_epi32(hi, off0), r0v);
            let t = trunc_i16x32_v4(token, rl, rh);
            let out: &mut [i16; 32] = (&mut imrow[x..x + 32]).try_into().unwrap();
            _mm512_storeu_si512(out, t);
            x += 32;
        }
        while x + 16 <= x1_16 {
            let mut acc = _mm512_setzero_si512();
            for (k, cf) in cfs_x.iter().enumerate() {
                let i = (row + x as isize - fo_horiz + k as isize) as usize;
                let s: &[u8; 16] = src.data[i..i + 16].try_into().unwrap();
                let v = _mm256_cvtepu8_epi16(_mm_loadu_si128(s));
                let p = _mm512_cvtepi16_epi32(_mm256_mullo_epi16(v, _mm512_castsi512_si256(*cf)));
                acc = _mm512_add_epi32(acc, p);
            }
            let r = _mm512_srav_epi32(_mm512_add_epi32(acc, off0), r0v);
            let t16 = _mm512_cvtepi32_epi16(r);
            let out: &mut [i16; 16] = (&mut imrow[x..x + 16]).try_into().unwrap();
            _mm256_storeu_si256(out, t16);
            x += 16;
        }
        for x in x..w {
            imrow[x] = convolve_2d_sr_hpx(
                src,
                y,
                x,
                x_filter,
                fo_vert as i32,
                fo_horiz as i32,
                round_0,
            );
        }
    }

    // Vertical pass — i16 tap loads widened to i32; |im| is ~14-bit so the
    // product needs the width.
    let cfs_y: [__m512i; SUBPEL_TAPS] =
        core::array::from_fn(|i| _mm512_set1_epi32(y_filter[i] as i32));
    let offv =
        _mm512_set1_epi32((1 << offset_bits) + if round_1 > 0 { 1 << (round_1 - 1) } else { 0 });
    let corrv =
        _mm512_set1_epi32((1 << (offset_bits - round_1)) + (1 << (offset_bits - round_1 - 1)));
    let r1v = _mm512_set1_epi32(round_1);
    // `round_power_of_two(v, 0) = v` — bits is 0 for single prediction.
    let bv = _mm512_set1_epi32(bits.max(0));
    let b_off = _mm512_set1_epi32(if bits > 0 { 1 << (bits - 1) } else { 0 });
    for y in 0..h {
        let drow = &mut dst[y * dst_stride..y * dst_stride + w];
        let mut x = 0usize;
        // `(acc + offv) >> round_1 - corr`, truncated `as i16`, then the
        // second `round_power_of_two(res, bits)` — all at i32, matching the
        // scalar arm's re-widened i16.
        let el = |acc: __m512i| -> __m512i {
            let t = _mm512_sub_epi32(_mm512_srav_epi32(_mm512_add_epi32(acc, offv), r1v), corrv);
            // `as i16` then `as i32`: truncate to 16 bits and sign-extend.
            let t16 = _mm512_cvtepi16_epi32(_mm512_cvtepi32_epi16(t));
            _mm512_srav_epi32(_mm512_add_epi32(t16, b_off), bv)
        };
        while x + 32 <= w {
            let mut lo = _mm512_setzero_si512();
            let mut hi = _mm512_setzero_si512();
            for (k, cf) in cfs_y.iter().enumerate() {
                let s: &[i16; 32] = im_block[(y + k) * w + x..(y + k) * w + x + 32]
                    .try_into()
                    .unwrap();
                let v = _mm512_loadu_si512(s);
                lo = _mm512_add_epi32(
                    lo,
                    _mm512_mullo_epi32(_mm512_cvtepi16_epi32(_mm512_castsi512_si256(v)), *cf),
                );
                hi = _mm512_add_epi32(
                    hi,
                    _mm512_mullo_epi32(
                        _mm512_cvtepi16_epi32(_mm512_extracti64x4_epi64::<1>(v)),
                        *cf,
                    ),
                );
            }
            let res_lo = el(lo);
            let res_hi = el(hi);
            let px = pack_u8x32_v4(token, res_lo, res_hi);
            let out: &mut [u8; 32] = (&mut drow[x..x + 32]).try_into().unwrap();
            _mm256_storeu_si256(out, px);
            x += 32;
        }
        while x + 16 <= w {
            let mut acc = _mm512_setzero_si512();
            for (k, cf) in cfs_y.iter().enumerate() {
                let s: &[i16; 16] = im_block[(y + k) * w + x..(y + k) * w + x + 16]
                    .try_into()
                    .unwrap();
                let v = _mm512_cvtepi16_epi32(_mm256_loadu_si256(s));
                acc = _mm512_add_epi32(acc, _mm512_mullo_epi32(v, *cf));
            }
            let res = el(acc);
            let px = pack_u8x16_v4(token, res);
            let out: &mut [u8; 16] = (&mut drow[x..x + 16]).try_into().unwrap();
            _mm_storeu_si128(out, px);
            x += 16;
        }
        for x in x..w {
            drow[x] = convolve_2d_sr_vpx(&im_block, w, y, x, y_filter, offset_bits, round_1, bits);
        }
    }
}

// --- AVX-512 (`_v4`) arms of the compound (`jnt`) family ---
//
// Same tap-load shape as the `_sr` v4 arms; only the epilogue differs: where
// `_sr` clips to `u8`, `jnt` either stores the `ConvBufType` (u16) `res` into
// `conv_buf` or folds it into the previous prediction through `jnt_average`.
// Reached only for `w >= 16` (16-px and 32-px rungs, scalar tails) and only
// when `conv_buf`/`dst` geometry is row-contiguous the way the scalar body
// indexes it (`dst_stride >= w`, `conv_params.dst_stride >= w`).

/// The `jnt_average`/`ConvBufType` tail for one 16-lane group.
///
/// `res` is the i32x16 value the scalar body calls `res`. `TRUNC_RES` selects
/// whether the average sees the `as u16` truncation first: `jnt_convolve_2d`
/// and `jnt_convolve_2d_copy` truncate `res` to `ConvBufType` BEFORE
/// `jnt_average` re-widens it; `jnt_convolve_x`/`_y` pass the untruncated i32.
/// The `!do_average` store truncates to u16 either way (`res as u16`).
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
#[rite]
#[allow(clippy::too_many_arguments)]
fn jnt_epilogue16_v4<const TRUNC_RES: bool>(
    token: X64V4Token,
    res: __m512i,
    conv_row: &mut [u16],
    dst_row: &mut [u8],
    x: usize,
    cp: &ConvolveParams,
    round_offset: i32,
    round_bits: i32,
) {
    let res_u16 = _mm512_cvtepi32_epi16(res);
    if !cp.do_average {
        let slot: &mut [u16; 16] = (&mut conv_row[x..x + 16]).try_into().unwrap();
        _mm256_storeu_si256(slot, res_u16);
        return;
    }
    let cbv: &[u16; 16] = conv_row[x..x + 16].try_into().unwrap();
    let cb32 = _mm512_cvtepu16_epi32(_mm256_loadu_si256(cbv));
    let res32 = if TRUNC_RES {
        _mm512_cvtepu16_epi32(res_u16)
    } else {
        res
    };
    let tmp = if cp.use_jnt_comp_avg {
        _mm512_srai_epi32::<{ DIST_PRECISION_BITS as u32 }>(_mm512_add_epi32(
            _mm512_mullo_epi32(cb32, _mm512_set1_epi32(cp.fwd_offset)),
            _mm512_mullo_epi32(res32, _mm512_set1_epi32(cp.bck_offset)),
        ))
    } else {
        _mm512_srai_epi32::<1>(_mm512_add_epi32(cb32, res32))
    };
    let tmp = _mm512_sub_epi32(tmp, _mm512_set1_epi32(round_offset));
    // clip_pixel_8(round_power_of_two(tmp, round_bits)) — pack_u8x16_v4's
    // saturating narrow after max(0) IS that clamp.
    let rb_off = _mm512_set1_epi32(if round_bits > 0 {
        1 << (round_bits - 1)
    } else {
        0
    });
    let v = _mm512_srav_epi32(
        _mm512_add_epi32(tmp, rb_off),
        _mm512_set1_epi32(round_bits.max(0)),
    );
    let px = pack_u8x16_v4(token, v);
    let out: &mut [u8; 16] = (&mut dst_row[x..x + 16]).try_into().unwrap();
    _mm_storeu_si128(out, px);
}

/// 32-lane twin of [`jnt_epilogue16_v4`].
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
#[rite]
#[allow(clippy::too_many_arguments)]
fn jnt_epilogue32_v4<const TRUNC_RES: bool>(
    token: X64V4Token,
    res_lo: __m512i,
    res_hi: __m512i,
    conv_row: &mut [u16],
    dst_row: &mut [u8],
    x: usize,
    cp: &ConvolveParams,
    round_offset: i32,
    round_bits: i32,
) {
    let lo16 = _mm512_cvtepi32_epi16(res_lo);
    let hi16 = _mm512_cvtepi32_epi16(res_hi);
    if !cp.do_average {
        let res16x32 = _mm512_inserti64x4::<1>(_mm512_castsi256_si512(lo16), hi16);
        let slot: &mut [u16; 32] = (&mut conv_row[x..x + 32]).try_into().unwrap();
        _mm512_storeu_si512(slot, res16x32);
        return;
    }
    let rb_off = _mm512_set1_epi32(if round_bits > 0 {
        1 << (round_bits - 1)
    } else {
        0
    });
    let rbv = _mm512_set1_epi32(round_bits.max(0));
    let roff = _mm512_set1_epi32(round_offset);
    let fwd = _mm512_set1_epi32(cp.fwd_offset);
    let bck = _mm512_set1_epi32(cp.bck_offset);
    let avg = |res: __m512i, res16: __m256i, off: usize| -> __m512i {
        let cbv: &[u16; 16] = conv_row[x + off..x + off + 16].try_into().unwrap();
        let cb32 = _mm512_cvtepu16_epi32(_mm256_loadu_si256(cbv));
        let res32 = if TRUNC_RES {
            _mm512_cvtepu16_epi32(res16)
        } else {
            res
        };
        let tmp = if cp.use_jnt_comp_avg {
            _mm512_srai_epi32::<{ DIST_PRECISION_BITS as u32 }>(_mm512_add_epi32(
                _mm512_mullo_epi32(cb32, fwd),
                _mm512_mullo_epi32(res32, bck),
            ))
        } else {
            _mm512_srai_epi32::<1>(_mm512_add_epi32(cb32, res32))
        };
        _mm512_srav_epi32(_mm512_add_epi32(_mm512_sub_epi32(tmp, roff), rb_off), rbv)
    };
    let px = pack_u8x32_v4(token, avg(res_lo, lo16, 0), avg(res_hi, hi16, 16));
    let out: &mut [u8; 32] = (&mut dst_row[x..x + 32]).try_into().unwrap();
    _mm256_storeu_si256(out, px);
}

/// x86-64 v4 arm of [`jnt_convolve_x`]: the `convolve_x_sr_v4` tap pipeline
/// feeding the compound epilogue. `bits = FILTER_BITS - round_1` — the
/// upstream asymmetry is kept.
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn jnt_convolve_x_v4(
    token: X64V4Token,
    src: SrcView<'_>,
    dst: &mut [u8],
    dst_stride: usize,
    conv_buf: &mut [u16],
    w: usize,
    h: usize,
    x_filter: &InterpKernel,
    cp: &ConvolveParams,
) {
    let cb_stride = cp.dst_stride;
    let fo = (SUBPEL_TAPS / 2 - 1) as isize;
    let len = src.data.len() as isize;
    let origin = src.origin as isize;
    let stride = src.stride as isize;
    let bits = FILTER_BITS - cp.round_1;
    let offset_bits = 8 + 2 * FILTER_BITS - cp.round_0;
    let round_offset = (1 << (offset_bits - cp.round_1)) + (1 << (offset_bits - cp.round_1 - 1));
    let round_bits = 2 * FILTER_BITS - cp.round_0 - cp.round_1;
    let cfs: [__m512i; SUBPEL_TAPS] = core::array::from_fn(|i| _mm512_set1_epi16(x_filter[i]));
    let r0_off = _mm512_set1_epi32(if cp.round_0 > 0 {
        1 << (cp.round_0 - 1)
    } else {
        0
    });
    let r0v = _mm512_set1_epi32(cp.round_0);
    let scale = _mm512_set1_epi32(1 << bits);
    let roff = _mm512_set1_epi32(round_offset);
    for y in 0..h {
        let row = origin + y as isize * stride;
        let x0 = (fo - row).max(0).min(w as isize) as usize;
        let x1_32 = ((len - 32 - 7 + fo - row).min(w as isize)).max(x0 as isize) as usize;
        let x1_16 = ((len - 16 - 7 + fo - row).min(w as isize)).max(x0 as isize) as usize;
        // res = (1 << bits) * round_power_of_two(acc, round_0) + round_offset
        let fin = |acc: __m512i| -> __m512i {
            let r = _mm512_srav_epi32(_mm512_add_epi32(acc, r0_off), r0v);
            _mm512_add_epi32(_mm512_mullo_epi32(r, scale), roff)
        };
        for x in 0..x0 {
            let mut res = 0i32;
            for k in 0..SUBPEL_TAPS {
                res += x_filter[k] as i32 * src.at(y as i32, x as i32 - fo as i32 + k as i32);
            }
            let res = (1 << bits) * round_power_of_two(res, cp.round_0) + round_offset;
            if cp.do_average {
                dst[y * dst_stride + x] = jnt_average(
                    conv_buf[y * cb_stride + x],
                    res,
                    round_offset,
                    round_bits,
                    cp,
                );
            } else {
                conv_buf[y * cb_stride + x] = res as u16;
            }
        }
        let mut x = x0;
        while x + 32 <= x1_32 {
            let mut lo = _mm512_setzero_si512();
            let mut hi = _mm512_setzero_si512();
            for (k, cf) in cfs.iter().enumerate() {
                let i = (row + x as isize - fo + k as isize) as usize;
                let s: &[u8; 32] = src.data[i..i + 32].try_into().unwrap();
                let v = _mm512_cvtepu8_epi16(_mm256_loadu_si256(s));
                let p = _mm512_mullo_epi16(v, *cf);
                lo = _mm512_add_epi32(lo, _mm512_cvtepi16_epi32(_mm512_castsi512_si256(p)));
                hi = _mm512_add_epi32(hi, _mm512_cvtepi16_epi32(_mm512_extracti64x4_epi64::<1>(p)));
            }
            jnt_epilogue32_v4::<false>(
                token,
                fin(lo),
                fin(hi),
                &mut conv_buf[y * cb_stride..],
                &mut dst[y * dst_stride..],
                x,
                cp,
                round_offset,
                round_bits,
            );
            x += 32;
        }
        while x + 16 <= x1_16 {
            let mut acc = _mm512_setzero_si512();
            for (k, cf) in cfs.iter().enumerate() {
                let i = (row + x as isize - fo + k as isize) as usize;
                let s: &[u8; 16] = src.data[i..i + 16].try_into().unwrap();
                let v = _mm256_cvtepu8_epi16(_mm_loadu_si128(s));
                let p = _mm512_cvtepi16_epi32(_mm256_mullo_epi16(v, _mm512_castsi512_si256(*cf)));
                acc = _mm512_add_epi32(acc, p);
            }
            jnt_epilogue16_v4::<false>(
                token,
                fin(acc),
                &mut conv_buf[y * cb_stride..],
                &mut dst[y * dst_stride..],
                x,
                cp,
                round_offset,
                round_bits,
            );
            x += 16;
        }
        for x in x..w {
            let mut res = 0i32;
            for k in 0..SUBPEL_TAPS {
                res += x_filter[k] as i32 * src.at(y as i32, x as i32 - fo as i32 + k as i32);
            }
            let res = (1 << bits) * round_power_of_two(res, cp.round_0) + round_offset;
            if cp.do_average {
                dst[y * dst_stride + x] = jnt_average(
                    conv_buf[y * cb_stride + x],
                    res,
                    round_offset,
                    round_bits,
                    cp,
                );
            } else {
                conv_buf[y * cb_stride + x] = res as u16;
            }
        }
    }
}

/// x86-64 v4 arm of [`jnt_convolve_y`]: the `convolve_y_sr_v4` tap pipeline
/// feeding the compound epilogue. `bits = FILTER_BITS - round_0` here (the
/// `_x` twin uses `round_1`) — upstream asymmetry kept verbatim.
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn jnt_convolve_y_v4(
    token: X64V4Token,
    src: SrcView<'_>,
    dst: &mut [u8],
    dst_stride: usize,
    conv_buf: &mut [u16],
    w: usize,
    h: usize,
    y_filter: &InterpKernel,
    cp: &ConvolveParams,
) {
    let cb_stride = cp.dst_stride;
    let fo = (SUBPEL_TAPS / 2 - 1) as isize;
    let len = src.data.len() as isize;
    let origin = src.origin as isize;
    let stride = src.stride as isize;
    let bits = FILTER_BITS - cp.round_0;
    let offset_bits = 8 + 2 * FILTER_BITS - cp.round_0;
    let round_offset = (1 << (offset_bits - cp.round_1)) + (1 << (offset_bits - cp.round_1 - 1));
    let round_bits = 2 * FILTER_BITS - cp.round_0 - cp.round_1;
    let cfs: [__m512i; SUBPEL_TAPS] = core::array::from_fn(|i| _mm512_set1_epi16(y_filter[i]));
    let scale = _mm512_set1_epi32(1 << bits);
    let r1_off = _mm512_set1_epi32(if cp.round_1 > 0 {
        1 << (cp.round_1 - 1)
    } else {
        0
    });
    let r1v = _mm512_set1_epi32(cp.round_1);
    let roff = _mm512_set1_epi32(round_offset);
    // res = round_power_of_two(acc * (1 << bits), round_1) + round_offset
    let fin = |acc: __m512i| -> __m512i {
        let t = _mm512_mullo_epi32(acc, scale);
        _mm512_add_epi32(_mm512_srav_epi32(_mm512_add_epi32(t, r1_off), r1v), roff)
    };
    for y in 0..h {
        let row0 = origin + (y as isize - fo) * stride;
        let row7 = row0 + 7 * stride;
        let x0 = (-row0).max(0).min(w as isize) as usize;
        let x1_32 = ((len - 32 - row7).min(w as isize)).max(x0 as isize) as usize;
        let x1_16 = ((len - 16 - row7).min(w as isize)).max(x0 as isize) as usize;
        for x in 0..x0 {
            let mut res = 0i32;
            for k in 0..SUBPEL_TAPS {
                res += y_filter[k] as i32 * src.at(y as i32 - fo as i32 + k as i32, x as i32);
            }
            let res = round_power_of_two(res * (1 << bits), cp.round_1) + round_offset;
            if cp.do_average {
                dst[y * dst_stride + x] = jnt_average(
                    conv_buf[y * cb_stride + x],
                    res,
                    round_offset,
                    round_bits,
                    cp,
                );
            } else {
                conv_buf[y * cb_stride + x] = res as u16;
            }
        }
        let mut x = x0;
        while x + 32 <= x1_32 {
            let mut lo = _mm512_setzero_si512();
            let mut hi = _mm512_setzero_si512();
            for (k, cf) in cfs.iter().enumerate() {
                let i = (row0 + k as isize * stride + x as isize) as usize;
                let s: &[u8; 32] = src.data[i..i + 32].try_into().unwrap();
                let v = _mm512_cvtepu8_epi16(_mm256_loadu_si256(s));
                let p = _mm512_mullo_epi16(v, *cf);
                lo = _mm512_add_epi32(lo, _mm512_cvtepi16_epi32(_mm512_castsi512_si256(p)));
                hi = _mm512_add_epi32(hi, _mm512_cvtepi16_epi32(_mm512_extracti64x4_epi64::<1>(p)));
            }
            jnt_epilogue32_v4::<false>(
                token,
                fin(lo),
                fin(hi),
                &mut conv_buf[y * cb_stride..],
                &mut dst[y * dst_stride..],
                x,
                cp,
                round_offset,
                round_bits,
            );
            x += 32;
        }
        while x + 16 <= x1_16 {
            let mut acc = _mm512_setzero_si512();
            for (k, cf) in cfs.iter().enumerate() {
                let i = (row0 + k as isize * stride + x as isize) as usize;
                let s: &[u8; 16] = src.data[i..i + 16].try_into().unwrap();
                let v = _mm256_cvtepu8_epi16(_mm_loadu_si128(s));
                let p = _mm512_cvtepi16_epi32(_mm256_mullo_epi16(v, _mm512_castsi512_si256(*cf)));
                acc = _mm512_add_epi32(acc, p);
            }
            jnt_epilogue16_v4::<false>(
                token,
                fin(acc),
                &mut conv_buf[y * cb_stride..],
                &mut dst[y * dst_stride..],
                x,
                cp,
                round_offset,
                round_bits,
            );
            x += 16;
        }
        for x in x..w {
            let mut res = 0i32;
            for k in 0..SUBPEL_TAPS {
                res += y_filter[k] as i32 * src.at(y as i32 - fo as i32 + k as i32, x as i32);
            }
            let res = round_power_of_two(res * (1 << bits), cp.round_1) + round_offset;
            if cp.do_average {
                dst[y * dst_stride + x] = jnt_average(
                    conv_buf[y * cb_stride + x],
                    res,
                    round_offset,
                    round_bits,
                    cp,
                );
            } else {
                conv_buf[y * cb_stride + x] = res as u16;
            }
        }
    }
}

/// x86-64 v4 arm of [`jnt_convolve_2d`]: the `convolve_2d_sr_v4` horizontal
/// pass (identical arithmetic — `1 << (bd + FILTER_BITS - 1)` seed, `round_0`
/// shift, `as i16` store into `im_block`), then a vertical pass whose `res`
/// goes through `round_1` into the compound epilogue.
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn jnt_convolve_2d_v4(
    token: X64V4Token,
    src: SrcView<'_>,
    dst: &mut [u8],
    dst_stride: usize,
    conv_buf: &mut [u16],
    w: usize,
    h: usize,
    x_filter: &InterpKernel,
    y_filter: &InterpKernel,
    cp: &ConvolveParams,
) {
    let cb_stride = cp.dst_stride;
    let im_h = h + SUBPEL_TAPS - 1;
    let fo_vert = (SUBPEL_TAPS / 2 - 1) as isize;
    let fo_horiz = (SUBPEL_TAPS / 2 - 1) as isize;
    let len = src.data.len() as isize;
    let origin = src.origin as isize;
    let stride = src.stride as isize;
    let offset_bits = 8 + 2 * FILTER_BITS - cp.round_0;
    let round_offset = (1 << (offset_bits - cp.round_1)) + (1 << (offset_bits - cp.round_1 - 1));
    let round_bits = 2 * FILTER_BITS - cp.round_0 - cp.round_1;

    let mut im_block = alloc::vec![0i16; im_h * w];
    let cfs_x: [__m512i; SUBPEL_TAPS] = core::array::from_fn(|i| _mm512_set1_epi16(x_filter[i]));
    let off0 = _mm512_set1_epi32(
        (1 << (8 + FILTER_BITS - 1))
            + if cp.round_0 > 0 {
                1 << (cp.round_0 - 1)
            } else {
                0
            },
    );
    let r0v = _mm512_set1_epi32(cp.round_0);
    for y in 0..im_h {
        let row = origin + (y as isize - fo_vert) * stride;
        let x0 = (fo_horiz - row).max(0).min(w as isize) as usize;
        let x1_32 = ((len - 32 - 7 + fo_horiz - row).min(w as isize)).max(x0 as isize) as usize;
        let x1_16 = ((len - 16 - 7 + fo_horiz - row).min(w as isize)).max(x0 as isize) as usize;
        let imrow = &mut im_block[y * w..y * w + w];
        for x in 0..x0 {
            let mut sum = 1i32 << (8 + FILTER_BITS - 1);
            for k in 0..SUBPEL_TAPS {
                sum += x_filter[k] as i32
                    * src.at(
                        y as i32 - fo_vert as i32,
                        x as i32 - fo_horiz as i32 + k as i32,
                    );
            }
            imrow[x] = round_power_of_two(sum, cp.round_0) as i16;
        }
        let mut x = x0;
        while x + 32 <= x1_32 {
            let mut lo = _mm512_setzero_si512();
            let mut hi = _mm512_setzero_si512();
            for (k, cf) in cfs_x.iter().enumerate() {
                let i = (row + x as isize - fo_horiz + k as isize) as usize;
                let s: &[u8; 32] = src.data[i..i + 32].try_into().unwrap();
                let v = _mm512_cvtepu8_epi16(_mm256_loadu_si256(s));
                let p = _mm512_mullo_epi16(v, *cf);
                lo = _mm512_add_epi32(lo, _mm512_cvtepi16_epi32(_mm512_castsi512_si256(p)));
                hi = _mm512_add_epi32(hi, _mm512_cvtepi16_epi32(_mm512_extracti64x4_epi64::<1>(p)));
            }
            let rl = _mm512_srav_epi32(_mm512_add_epi32(lo, off0), r0v);
            let rh = _mm512_srav_epi32(_mm512_add_epi32(hi, off0), r0v);
            let t = trunc_i16x32_v4(token, rl, rh);
            let out: &mut [i16; 32] = (&mut imrow[x..x + 32]).try_into().unwrap();
            _mm512_storeu_si512(out, t);
            x += 32;
        }
        while x + 16 <= x1_16 {
            let mut acc = _mm512_setzero_si512();
            for (k, cf) in cfs_x.iter().enumerate() {
                let i = (row + x as isize - fo_horiz + k as isize) as usize;
                let s: &[u8; 16] = src.data[i..i + 16].try_into().unwrap();
                let v = _mm256_cvtepu8_epi16(_mm_loadu_si128(s));
                let p = _mm512_cvtepi16_epi32(_mm256_mullo_epi16(v, _mm512_castsi512_si256(*cf)));
                acc = _mm512_add_epi32(acc, p);
            }
            let r = _mm512_srav_epi32(_mm512_add_epi32(acc, off0), r0v);
            let t16 = _mm512_cvtepi32_epi16(r);
            let out: &mut [i16; 16] = (&mut imrow[x..x + 16]).try_into().unwrap();
            _mm256_storeu_si256(out, t16);
            x += 16;
        }
        for x in x..w {
            let mut sum = 1i32 << (8 + FILTER_BITS - 1);
            for k in 0..SUBPEL_TAPS {
                sum += x_filter[k] as i32
                    * src.at(
                        y as i32 - fo_vert as i32,
                        x as i32 - fo_horiz as i32 + k as i32,
                    );
            }
            imrow[x] = round_power_of_two(sum, cp.round_0) as i16;
        }
    }

    // Vertical pass — same i16 tap loads as `convolve_2d_sr_v4`, but the seed
    // is `1 << offset_bits` inside the sum and `res` truncates to u16 (the
    // ConvBufType store semantics `jnt_epilogue`'s TRUNC_RES models).
    let cfs_y: [__m512i; SUBPEL_TAPS] =
        core::array::from_fn(|i| _mm512_set1_epi32(y_filter[i] as i32));
    let seed = _mm512_set1_epi32(1 << offset_bits);
    let r1_off = _mm512_set1_epi32(if cp.round_1 > 0 {
        1 << (cp.round_1 - 1)
    } else {
        0
    });
    let r1v = _mm512_set1_epi32(cp.round_1);
    for y in 0..h {
        let mut x = 0usize;
        while x + 32 <= w {
            let mut lo = seed;
            let mut hi = seed;
            for (k, cf) in cfs_y.iter().enumerate() {
                let s: &[i16; 32] = im_block[(y + k) * w + x..(y + k) * w + x + 32]
                    .try_into()
                    .unwrap();
                let v = _mm512_loadu_si512(s);
                lo = _mm512_add_epi32(
                    lo,
                    _mm512_mullo_epi32(_mm512_cvtepi16_epi32(_mm512_castsi512_si256(v)), *cf),
                );
                hi = _mm512_add_epi32(
                    hi,
                    _mm512_mullo_epi32(
                        _mm512_cvtepi16_epi32(_mm512_extracti64x4_epi64::<1>(v)),
                        *cf,
                    ),
                );
            }
            let fin =
                |acc: __m512i| -> __m512i { _mm512_srav_epi32(_mm512_add_epi32(acc, r1_off), r1v) };
            jnt_epilogue32_v4::<true>(
                token,
                fin(lo),
                fin(hi),
                &mut conv_buf[y * cb_stride..],
                &mut dst[y * dst_stride..],
                x,
                cp,
                round_offset,
                round_bits,
            );
            x += 32;
        }
        while x + 16 <= w {
            let mut acc = seed;
            for (k, cf) in cfs_y.iter().enumerate() {
                let s: &[i16; 16] = im_block[(y + k) * w + x..(y + k) * w + x + 16]
                    .try_into()
                    .unwrap();
                let v = _mm512_cvtepi16_epi32(_mm256_loadu_si256(s));
                acc = _mm512_add_epi32(acc, _mm512_mullo_epi32(v, *cf));
            }
            let res = _mm512_srav_epi32(_mm512_add_epi32(acc, r1_off), r1v);
            jnt_epilogue16_v4::<true>(
                token,
                res,
                &mut conv_buf[y * cb_stride..],
                &mut dst[y * dst_stride..],
                x,
                cp,
                round_offset,
                round_bits,
            );
            x += 16;
        }
        for x in x..w {
            let mut sum = 1i32 << offset_bits;
            for k in 0..SUBPEL_TAPS {
                sum += y_filter[k] as i32 * im_block[(y + k) * w + x] as i32;
            }
            let res = round_power_of_two(sum, cp.round_1) as u16;
            if cp.do_average {
                dst[y * dst_stride + x] = jnt_average(
                    conv_buf[y * cb_stride + x],
                    res as i32,
                    round_offset,
                    round_bits,
                    cp,
                );
            } else {
                conv_buf[y * cb_stride + x] = res;
            }
        }
    }
}

/// x86-64 v4 arm of [`jnt_convolve_2d_copy`]: `(src << bits) + round_offset`
/// with `ConvBufType` (u16) wrap — `mullo_epi16`/`add_epi16` wrap in 16-bit
/// lanes, matching the scalar `u16` arithmetic exactly.
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn jnt_convolve_2d_copy_v4(
    token: X64V4Token,
    src: SrcView<'_>,
    dst: &mut [u8],
    dst_stride: usize,
    conv_buf: &mut [u16],
    w: usize,
    h: usize,
    cp: &ConvolveParams,
) {
    let cb_stride = cp.dst_stride;
    let len = src.data.len() as isize;
    let origin = src.origin as isize;
    let stride = src.stride as isize;
    let bits = FILTER_BITS * 2 - cp.round_1 - cp.round_0;
    let offset_bits = 8 + 2 * FILTER_BITS - cp.round_0;
    let round_offset = (1 << (offset_bits - cp.round_1)) + (1 << (offset_bits - cp.round_1 - 1));
    let round_bits = 2 * FILTER_BITS - cp.round_0 - cp.round_1;
    let shl = _mm512_set1_epi16((1u16 << bits) as i16);
    let roff16 = _mm512_set1_epi16(round_offset as u16 as i16);
    for y in 0..h {
        let row = origin + y as isize * stride;
        let x1_32 = ((len - 32 - row).min(w as isize)).max(0) as usize;
        let x1_16 = ((len - 16 - row).min(w as isize)).max(0) as usize;
        let mut x = 0usize;
        while x + 32 <= x1_32 {
            let i = (row + x as isize) as usize;
            let s: &[u8; 32] = src.data[i..i + 32].try_into().unwrap();
            let v = _mm512_add_epi16(
                _mm512_mullo_epi16(_mm512_cvtepu8_epi16(_mm256_loadu_si256(s)), shl),
                roff16,
            );
            if !cp.do_average {
                let slot: &mut [u16; 32] = (&mut conv_buf
                    [y * cb_stride + x..y * cb_stride + x + 32])
                    .try_into()
                    .unwrap();
                _mm512_storeu_si512(slot, v);
            } else {
                // u16 lanes widened to i32 — the same value the scalar body
                // holds in `res` (already a u16).
                let lo = _mm512_cvtepu16_epi32(_mm512_castsi512_si256(v));
                let hi = _mm512_cvtepu16_epi32(_mm512_extracti64x4_epi64::<1>(v));
                jnt_epilogue32_v4::<true>(
                    token,
                    lo,
                    hi,
                    &mut conv_buf[y * cb_stride..],
                    &mut dst[y * dst_stride..],
                    x,
                    cp,
                    round_offset,
                    round_bits,
                );
            }
            x += 32;
        }
        while x + 16 <= x1_16 {
            let i = (row + x as isize) as usize;
            let s: &[u8; 16] = src.data[i..i + 16].try_into().unwrap();
            let v = _mm256_add_epi16(
                _mm256_mullo_epi16(
                    _mm256_cvtepu8_epi16(_mm_loadu_si128(s)),
                    _mm512_castsi512_si256(shl),
                ),
                _mm512_castsi512_si256(roff16),
            );
            if !cp.do_average {
                let slot: &mut [u16; 16] = (&mut conv_buf
                    [y * cb_stride + x..y * cb_stride + x + 16])
                    .try_into()
                    .unwrap();
                _mm256_storeu_si256(slot, v);
            } else {
                let res = _mm512_cvtepu16_epi32(v);
                jnt_epilogue16_v4::<true>(
                    token,
                    res,
                    &mut conv_buf[y * cb_stride..],
                    &mut dst[y * dst_stride..],
                    x,
                    cp,
                    round_offset,
                    round_bits,
                );
            }
            x += 16;
        }
        for x in x..w {
            let mut res = (src.at(y as i32, x as i32) as u16) << bits;
            res = res.wrapping_add(round_offset as u16);
            if cp.do_average {
                dst[y * dst_stride + x] = jnt_average(
                    conv_buf[y * cb_stride + x],
                    res as i32,
                    round_offset,
                    round_bits,
                    cp,
                );
            } else {
                conv_buf[y * cb_stride + x] = res;
            }
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
        let i = (src.origin as isize + y as isize * src.stride as isize) as usize;
        dst[y * dst_stride..y * dst_stride + w].copy_from_slice(&src.data[i..i + w]);
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
    #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
    if w >= 16
        && conv_params.dst_stride >= w
        && conv_buf.len() >= conv_params.dst_stride * h
        && (!conv_params.do_average || dst.len() >= (h - 1) * dst_stride + w)
    {
        if let Some(token) = X64V4Token::summon() {
            jnt_convolve_2d_v4(
                token,
                src,
                dst,
                dst_stride,
                conv_buf,
                w,
                h,
                filter_x.subpel_kernel(subpel_x_q4),
                filter_y.subpel_kernel(subpel_y_q4),
                conv_params,
            );
            return;
        }
    }
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
    #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
    if w >= 16
        && conv_params.dst_stride >= w
        && conv_buf.len() >= conv_params.dst_stride * h
        && (!conv_params.do_average || dst.len() >= (h - 1) * dst_stride + w)
    {
        if let Some(token) = X64V4Token::summon() {
            jnt_convolve_y_v4(
                token,
                src,
                dst,
                dst_stride,
                conv_buf,
                w,
                h,
                filter_y.subpel_kernel(subpel_y_q4),
                conv_params,
            );
            return;
        }
    }
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
    #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
    if w >= 16
        && conv_params.dst_stride >= w
        && conv_buf.len() >= conv_params.dst_stride * h
        && (!conv_params.do_average || dst.len() >= (h - 1) * dst_stride + w)
    {
        if let Some(token) = X64V4Token::summon() {
            jnt_convolve_x_v4(
                token,
                src,
                dst,
                dst_stride,
                conv_buf,
                w,
                h,
                filter_x.subpel_kernel(subpel_x_q4),
                conv_params,
            );
            return;
        }
    }
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
    #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
    if w >= 16
        && conv_params.dst_stride >= w
        && conv_buf.len() >= conv_params.dst_stride * h
        && (!conv_params.do_average || dst.len() >= (h - 1) * dst_stride + w)
    {
        if let Some(token) = X64V4Token::summon() {
            jnt_convolve_2d_copy_v4(token, src, dst, dst_stride, conv_buf, w, h, conv_params);
            return;
        }
    }
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

    /// Positive witness that the jnt `_v4` arms are EXERCISED and exact, not
    /// just compiled — the c_parity tests pass either way since the scalar
    /// fallback is also correct. The oracle is the scalar body transcribed
    /// per kernel; each `w` covers the 32-px rung, the 16-px rung, and the
    /// scalar tail (w = 56 -> 32 + 16 + 8).
    #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
    #[test]
    fn jnt_v4_arms_match_scalar_when_summoned() {
        extern crate std;
        use alloc::vec;
        use alloc::vec::Vec;
        let Some(v4) = X64V4Token::summon() else {
            std::eprintln!("X64V4Token unavailable — jnt _v4 arms NOT exercised on this host");
            return;
        };
        let fp = interp_filter_params_with_block_size(InterpFilterKind::EightTapRegular, 32);
        let mk_cp = |do_average: bool, use_jnt: bool| {
            let mut cp = ConvolveParams::no_round(do_average, 64, true, 8);
            cp.use_jnt_comp_avg = use_jnt;
            cp.fwd_offset = 9;
            cp.bck_offset = 7;
            cp
        };
        let stride = 80usize;
        let src: Vec<u8> = (0..stride * 80)
            .map(|i| ((i * 37 + 11) % 251) as u8)
            .collect();
        let view = SrcView::new(&src, 8 * stride + 8, stride);
        let bd = 8i32;

        for &(w, h) in &[(16usize, 8usize), (32, 8), (56, 8), (64, 16)] {
            for cp in [mk_cp(false, false), mk_cp(true, false), mk_cp(true, true)] {
                let cb_stride = cp.dst_stride;
                // --- scalar oracles (verbatim transcriptions) ---
                let off = bd + 2 * FILTER_BITS - cp.round_0;
                let roff = (1 << (off - cp.round_1)) + (1 << (off - cp.round_1 - 1));
                let rbits = 2 * FILTER_BITS - cp.round_0 - cp.round_1;
                let mut cb0 = vec![0u16; cb_stride * h];
                let mut d0 = vec![0u8; w * h];
                let mut cb1 = cb0.clone();
                let mut d1 = d0.clone();

                // jnt_convolve_x
                let xf = fp.subpel_kernel(5);
                let bits = FILTER_BITS - cp.round_1;
                for y in 0..h {
                    for x in 0..w {
                        let mut res = 0i32;
                        for k in 0..SUBPEL_TAPS {
                            res += xf[k] as i32 * view.at(y as i32, x as i32 - 3 + k as i32);
                        }
                        let res = (1 << bits) * round_power_of_two(res, cp.round_0) + roff;
                        if cp.do_average {
                            d0[y * w + x] =
                                jnt_average(cb0[y * cb_stride + x], res, roff, rbits, &cp);
                        } else {
                            cb0[y * cb_stride + x] = res as u16;
                        }
                    }
                }
                jnt_convolve_x_v4(v4, view, &mut d1, w, &mut cb1, w, h, xf, &cp);
                assert_eq!(cb0, cb1, "jnt_x_v4 conv_buf {w}x{h} avg={}", cp.do_average);
                assert_eq!(d0, d1, "jnt_x_v4 dst {w}x{h} avg={}", cp.do_average);

                // jnt_convolve_y
                let yf = fp.subpel_kernel(9);
                let bits = FILTER_BITS - cp.round_0;
                let mut cb0 = vec![0u16; cb_stride * h];
                let mut d0 = vec![0u8; w * h];
                let mut cb1 = cb0.clone();
                let mut d1 = d0.clone();
                for y in 0..h {
                    for x in 0..w {
                        let mut res = 0i32;
                        for k in 0..SUBPEL_TAPS {
                            res += yf[k] as i32 * view.at(y as i32 - 3 + k as i32, x as i32);
                        }
                        let res = round_power_of_two(res * (1 << bits), cp.round_1) + roff;
                        if cp.do_average {
                            d0[y * w + x] =
                                jnt_average(cb0[y * cb_stride + x], res, roff, rbits, &cp);
                        } else {
                            cb0[y * cb_stride + x] = res as u16;
                        }
                    }
                }
                jnt_convolve_y_v4(v4, view, &mut d1, w, &mut cb1, w, h, yf, &cp);
                assert_eq!(cb0, cb1, "jnt_y_v4 conv_buf {w}x{h} avg={}", cp.do_average);
                assert_eq!(d0, d1, "jnt_y_v4 dst {w}x{h} avg={}", cp.do_average);

                // jnt_convolve_2d
                let mut cb0 = vec![0u16; cb_stride * h];
                let mut d0 = vec![0u8; w * h];
                let mut cb1 = cb0.clone();
                let mut d1 = d0.clone();
                let im_h = h + SUBPEL_TAPS - 1;
                let mut im = vec![0i16; im_h * w];
                for y in 0..im_h {
                    for x in 0..w {
                        let mut sum = 1i32 << (bd + FILTER_BITS - 1);
                        for k in 0..SUBPEL_TAPS {
                            sum += xf[k] as i32 * view.at(y as i32 - 3, x as i32 - 3 + k as i32);
                        }
                        im[y * w + x] = round_power_of_two(sum, cp.round_0) as i16;
                    }
                }
                for y in 0..h {
                    for x in 0..w {
                        let mut sum = 1i32 << off;
                        for k in 0..SUBPEL_TAPS {
                            sum += yf[k] as i32 * im[(y + k) * w + x] as i32;
                        }
                        let res = round_power_of_two(sum, cp.round_1) as u16;
                        if cp.do_average {
                            d0[y * w + x] =
                                jnt_average(cb0[y * cb_stride + x], res as i32, roff, rbits, &cp);
                        } else {
                            cb0[y * cb_stride + x] = res;
                        }
                    }
                }
                jnt_convolve_2d_v4(v4, view, &mut d1, w, &mut cb1, w, h, xf, yf, &cp);
                assert_eq!(cb0, cb1, "jnt_2d_v4 conv_buf {w}x{h} avg={}", cp.do_average);
                assert_eq!(d0, d1, "jnt_2d_v4 dst {w}x{h} avg={}", cp.do_average);

                // jnt_convolve_2d_copy
                let mut cb0 = vec![0u16; cb_stride * h];
                let mut d0 = vec![0u8; w * h];
                let mut cb1 = cb0.clone();
                let mut d1 = d0.clone();
                let bits = FILTER_BITS * 2 - cp.round_1 - cp.round_0;
                for y in 0..h {
                    for x in 0..w {
                        let res = ((view.at(y as i32, x as i32) as u16) << bits)
                            .wrapping_add(roff as u16);
                        if cp.do_average {
                            d0[y * w + x] =
                                jnt_average(cb0[y * cb_stride + x], res as i32, roff, rbits, &cp);
                        } else {
                            cb0[y * cb_stride + x] = res;
                        }
                    }
                }
                jnt_convolve_2d_copy_v4(v4, view, &mut d1, w, &mut cb1, w, h, &cp);
                assert_eq!(
                    cb0, cb1,
                    "jnt_copy_v4 conv_buf {w}x{h} avg={}",
                    cp.do_average
                );
                assert_eq!(d0, d1, "jnt_copy_v4 dst {w}x{h} avg={}", cp.do_average);
            }
        }
    }
}
