//! The 10/12-bit (highbd) AV1 inter reconstruction MC kernels.
//!
//! Ported from `Source/Lib/Codec/inter_prediction.c` (SVT-AV1 v4.2.0):
//! `svt_av1_highbd_convolve_2d_copy_sr_c` (:713),
//! `svt_av1_highbd_convolve_x_sr_c` (:731),
//! `svt_av1_highbd_convolve_y_sr_c` (:758),
//! `svt_av1_highbd_convolve_2d_sr_c` (:784),
//! `svt_av1_highbd_jnt_convolve_x_c` (:905),
//! `svt_av1_highbd_jnt_convolve_y_c` (:950),
//! `svt_av1_highbd_jnt_convolve_2d_copy_c` (:995) and
//! `svt_av1_highbd_jnt_convolve_2d_c` (:1034).
//!
//! Nothing in `svtav1-dsp/src/hbd.rs` is a convolve — that module is intra /
//! loop-filter / CDEF / distortion — so the port ships 10-bit everywhere else
//! (`bd10.rs`, `c_parity_bd10_quant`) but had no 10-bit MC at all.
//!
//! # Where the highbd kernels differ from their 8-bit twins
//!
//! They are NOT the 8-bit bodies with `bd` substituted for the literal 8:
//!
//! * `svt_av1_convolve_2d_sr_c` truncates its vertical result to `int16_t`
//!   before the final shift; `svt_av1_highbd_convolve_2d_sr_c` keeps it in
//!   `int32_t`. MEASURED 2026-08-31: that difference is **inert** across the
//!   whole valid bit-depth range. At bd 10 the vertical `res` lands in
//!   [-1536, 2560] and at bd 12 in roughly [-6144, 10240] — both well inside
//!   `int16_t` — so a port that truncated here would still agree with C.
//!   Mutating the port to truncate does NOT fail
//!   `highbd_convolve_2d_sr_matches_c`. The faithful `int32_t` is kept anyway
//!   (WORKING-ON-THIS.md §7: dead-looking C stays translated, with its
//!   reachability written down) — do not "simplify" it to match the 8-bit
//!   twin, and do not cite it as a behavioural difference.
//! * `svt_av1_highbd_convolve_2d_sr_c`'s horizontal stage stores
//!   `(ConvBufType)ROUND_POWER_OF_TWO(...)` — a **u16** cast — into an
//!   `int16_t` array, i.e. a bit-reinterpretation. The 8-bit twin casts to
//!   `int16_t` directly. Both wrap the same bits, and both are modelled by
//!   storing the low 16 bits.
//! * `svt_av1_highbd_jnt_convolve_2d_c`'s vertical stage has **no** `bits`
//!   pre-scale at all (its 8-bit twin has none either, but the `_x` / `_y`
//!   highbd arms keep the same `round_1` / `round_0` asymmetry as the 8-bit
//!   ones).
//! * The `_x` arm's `bits = FILTER_BITS - round_1`, the `_y` arm's
//!   `bits = FILTER_BITS - round_0` — the same upstream asymmetry as 8-bit.

use crate::port_convolve::{
    ConvolveParams, DIST_PRECISION_BITS, FILTER_BITS, FilterParams, SUBPEL_TAPS,
};

/// `clip_pixel_highbd(val, bd)`.
#[inline]
fn clip_pixel_highbd(val: i32, bd: i32) -> u16 {
    val.clamp(0, (1 << bd) - 1) as u16
}

#[inline]
fn round_power_of_two(value: i32, n: i32) -> i32 {
    if n == 0 {
        value
    } else {
        (value + (1 << (n - 1))) >> n
    }
}

/// A 16-bit source view whose logical origin sits `origin` elements into
/// `data`, so the kernels can read the taps that precede it.
#[derive(Clone, Copy)]
pub struct SrcView16<'a> {
    data: &'a [u16],
    origin: usize,
    stride: usize,
}

impl<'a> SrcView16<'a> {
    /// Wrap `data` with its logical (0, 0) at `origin`.
    pub fn new(data: &'a [u16], origin: usize, stride: usize) -> Self {
        Self {
            data,
            origin,
            stride,
        }
    }

    #[inline]
    fn at(&self, y: i32, x: i32) -> i32 {
        let idx = self.origin as isize + y as isize * self.stride as isize + x as isize;
        self.data[idx as usize] as i32
    }
}

/// `svt_av1_highbd_convolve_2d_copy_sr_c` (inter_prediction.c:713) — the
/// 10-bit whole-pel path, i.e. the 10-bit PD0 kernel.
pub fn highbd_convolve_2d_copy_sr(
    src: SrcView16<'_>,
    dst: &mut [u16],
    dst_stride: usize,
    w: usize,
    h: usize,
) {
    for y in 0..h {
        for x in 0..w {
            dst[y * dst_stride + x] = src.at(y as i32, x as i32) as u16;
        }
    }
}

/// `svt_av1_highbd_convolve_x_sr_c` (inter_prediction.c:731).
pub fn highbd_convolve_x_sr(
    src: SrcView16<'_>,
    dst: &mut [u16],
    dst_stride: usize,
    w: usize,
    h: usize,
    filter_x: &FilterParams,
    subpel_x_q4: i32,
    conv_params: &ConvolveParams,
    bd: i32,
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
            dst[y * dst_stride + x] = clip_pixel_highbd(round_power_of_two(res, bits), bd);
        }
    }
}

/// `svt_av1_highbd_convolve_y_sr_c` (inter_prediction.c:758). `conv_params` is
/// unused by C (a single `FILTER_BITS` shift), so it is not a parameter here.
pub fn highbd_convolve_y_sr(
    src: SrcView16<'_>,
    dst: &mut [u16],
    dst_stride: usize,
    w: usize,
    h: usize,
    filter_y: &FilterParams,
    subpel_y_q4: i32,
    bd: i32,
) {
    let fo_vert = (SUBPEL_TAPS / 2 - 1) as i32;
    let y_filter = filter_y.subpel_kernel(subpel_y_q4);
    for y in 0..h {
        for x in 0..w {
            let mut res = 0i32;
            for k in 0..SUBPEL_TAPS {
                res += y_filter[k] as i32 * src.at(y as i32 - fo_vert + k as i32, x as i32);
            }
            dst[y * dst_stride + x] = clip_pixel_highbd(round_power_of_two(res, FILTER_BITS), bd);
        }
    }
}

/// `svt_av1_highbd_convolve_2d_sr_c` (inter_prediction.c:784).
///
/// The vertical result stays in `int32_t` here — unlike the 8-bit twin, which
/// truncates to `int16_t` first.
pub fn highbd_convolve_2d_sr(
    src: SrcView16<'_>,
    dst: &mut [u16],
    dst_stride: usize,
    w: usize,
    h: usize,
    filter_x: &FilterParams,
    filter_y: &FilterParams,
    subpel_x_q4: i32,
    subpel_y_q4: i32,
    conv_params: &ConvolveParams,
    bd: i32,
) {
    let im_h = h + SUBPEL_TAPS - 1;
    let im_stride = w;
    let fo_vert = (SUBPEL_TAPS / 2 - 1) as i32;
    let fo_horiz = (SUBPEL_TAPS / 2 - 1) as i32;
    let bits = FILTER_BITS * 2 - conv_params.round_0 - conv_params.round_1;

    let mut im_block = alloc::vec![0i16; im_h * im_stride];
    let x_filter = filter_x.subpel_kernel(subpel_x_q4);
    for y in 0..im_h {
        for x in 0..w {
            let mut sum = 1i32 << (bd + FILTER_BITS - 1);
            for k in 0..SUBPEL_TAPS {
                sum +=
                    x_filter[k] as i32 * src.at(y as i32 - fo_vert, x as i32 - fo_horiz + k as i32);
            }
            // C: `(ConvBufType)ROUND_POWER_OF_TWO(...)` stored into an
            // `int16_t` array — the low 16 bits, reinterpreted signed.
            im_block[y * im_stride + x] =
                round_power_of_two(sum, conv_params.round_0) as u16 as i16;
        }
    }

    let y_filter = filter_y.subpel_kernel(subpel_y_q4);
    let offset_bits = bd + 2 * FILTER_BITS - conv_params.round_0;
    for y in 0..h {
        for x in 0..w {
            let mut sum = 1i32 << offset_bits;
            for k in 0..SUBPEL_TAPS {
                sum += y_filter[k] as i32 * im_block[(y + k) * im_stride + x] as i32;
            }
            let res = round_power_of_two(sum, conv_params.round_1)
                - ((1 << (offset_bits - conv_params.round_1))
                    + (1 << (offset_bits - conv_params.round_1 - 1)));
            dst[y * dst_stride + x] = clip_pixel_highbd(round_power_of_two(res, bits), bd);
        }
    }
}

/// The compound blend tail shared by the four highbd `jnt_convolve` kernels.
#[inline]
fn jnt_average_hbd(
    conv_buf_val: u16,
    res: i32,
    round_offset: i32,
    round_bits: i32,
    conv_params: &ConvolveParams,
    bd: i32,
) -> u16 {
    let mut tmp = conv_buf_val as i32;
    if conv_params.use_jnt_comp_avg {
        tmp = tmp * conv_params.fwd_offset + res * conv_params.bck_offset;
        tmp >>= DIST_PRECISION_BITS;
    } else {
        tmp += res;
        tmp >>= 1;
    }
    tmp -= round_offset;
    clip_pixel_highbd(round_power_of_two(tmp, round_bits), bd)
}

/// `svt_av1_highbd_jnt_convolve_x_c` (inter_prediction.c:905).
/// `bits = FILTER_BITS - round_1`.
pub fn highbd_jnt_convolve_x(
    src: SrcView16<'_>,
    dst: &mut [u16],
    dst_stride: usize,
    conv_buf: &mut [u16],
    w: usize,
    h: usize,
    filter_x: &FilterParams,
    subpel_x_q4: i32,
    conv_params: &ConvolveParams,
    bd: i32,
) {
    let cb_stride = conv_params.dst_stride;
    let fo_horiz = (SUBPEL_TAPS / 2 - 1) as i32;
    let bits = FILTER_BITS - conv_params.round_1;
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
                dst[y * dst_stride + x] = jnt_average_hbd(
                    conv_buf[y * cb_stride + x],
                    res,
                    round_offset,
                    round_bits,
                    conv_params,
                    bd,
                );
            } else {
                conv_buf[y * cb_stride + x] = res as u16;
            }
        }
    }
}

/// `svt_av1_highbd_jnt_convolve_y_c` (inter_prediction.c:950).
/// `bits = FILTER_BITS - round_0`.
pub fn highbd_jnt_convolve_y(
    src: SrcView16<'_>,
    dst: &mut [u16],
    dst_stride: usize,
    conv_buf: &mut [u16],
    w: usize,
    h: usize,
    filter_y: &FilterParams,
    subpel_y_q4: i32,
    conv_params: &ConvolveParams,
    bd: i32,
) {
    let cb_stride = conv_params.dst_stride;
    let fo_vert = (SUBPEL_TAPS / 2 - 1) as i32;
    let bits = FILTER_BITS - conv_params.round_0;
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
                dst[y * dst_stride + x] = jnt_average_hbd(
                    conv_buf[y * cb_stride + x],
                    res,
                    round_offset,
                    round_bits,
                    conv_params,
                    bd,
                );
            } else {
                conv_buf[y * cb_stride + x] = res as u16;
            }
        }
    }
}

/// `svt_av1_highbd_jnt_convolve_2d_copy_c` (inter_prediction.c:995).
///
/// `res` is a `ConvBufType` (u16), so the shift and the `round_offset` add both
/// wrap at 16 bits before `do_average` re-widens.
pub fn highbd_jnt_convolve_2d_copy(
    src: SrcView16<'_>,
    dst: &mut [u16],
    dst_stride: usize,
    conv_buf: &mut [u16],
    w: usize,
    h: usize,
    conv_params: &ConvolveParams,
    bd: i32,
) {
    let cb_stride = conv_params.dst_stride;
    let bits = FILTER_BITS * 2 - conv_params.round_1 - conv_params.round_0;
    let offset_bits = bd + 2 * FILTER_BITS - conv_params.round_0;
    let round_offset =
        (1 << (offset_bits - conv_params.round_1)) + (1 << (offset_bits - conv_params.round_1 - 1));

    for y in 0..h {
        for x in 0..w {
            let mut res = (src.at(y as i32, x as i32) as u16).wrapping_shl(bits as u32);
            res = res.wrapping_add(round_offset as u16);
            if conv_params.do_average {
                dst[y * dst_stride + x] = jnt_average_hbd(
                    conv_buf[y * cb_stride + x],
                    res as i32,
                    round_offset,
                    bits,
                    conv_params,
                    bd,
                );
            } else {
                conv_buf[y * cb_stride + x] = res;
            }
        }
    }
}

/// `svt_av1_highbd_jnt_convolve_2d_c` (inter_prediction.c:1034).
///
/// The vertical stage has NO `bits` pre-scale; it stores
/// `(ConvBufType)ROUND_POWER_OF_TWO(sum, round_1)` straight into the CONV_BUF.
pub fn highbd_jnt_convolve_2d(
    src: SrcView16<'_>,
    dst: &mut [u16],
    dst_stride: usize,
    conv_buf: &mut [u16],
    w: usize,
    h: usize,
    filter_x: &FilterParams,
    filter_y: &FilterParams,
    subpel_x_q4: i32,
    subpel_y_q4: i32,
    conv_params: &ConvolveParams,
    bd: i32,
) {
    let cb_stride = conv_params.dst_stride;
    let im_h = h + SUBPEL_TAPS - 1;
    let im_stride = w;
    let fo_vert = (SUBPEL_TAPS / 2 - 1) as i32;
    let fo_horiz = (SUBPEL_TAPS / 2 - 1) as i32;
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
                dst[y * dst_stride + x] = jnt_average_hbd(
                    conv_buf[y * cb_stride + x],
                    res as i32,
                    round_offset,
                    round_bits,
                    conv_params,
                    bd,
                );
            } else {
                conv_buf[y * cb_stride + x] = res;
            }
        }
    }
}

/// The `svt_aom_convolveHbd[subX][subY][bi]` dispatch table
/// (inter_prediction.c:1094) as an enum, so `svt_highbd_inter_predictor`'s
/// kernel choice is expressible without function pointers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HbdKernel {
    /// `[0][0][0]` — `svt_av1_highbd_convolve_2d_copy_sr`.
    Copy2d,
    /// `[0][0][1]` — `svt_av1_highbd_jnt_convolve_2d_copy`.
    JntCopy2d,
    /// `[0][1][0]` — `svt_av1_highbd_convolve_y_sr`.
    Y,
    /// `[0][1][1]` — `svt_av1_highbd_jnt_convolve_y`.
    JntY,
    /// `[1][0][0]` — `svt_av1_highbd_convolve_x_sr`.
    X,
    /// `[1][0][1]` — `svt_av1_highbd_jnt_convolve_x`.
    JntX,
    /// `[1][1][0]` — `svt_av1_highbd_convolve_2d_sr`.
    Sr2d,
    /// `[1][1][1]` — `svt_av1_highbd_jnt_convolve_2d`.
    Jnt2d,
}

/// Index `svt_aom_convolveHbd` the way the predictors do: `[subpel_x != 0]`,
/// `[subpel_y != 0]`, `[is_compound]`.
pub fn hbd_kernel_for(sub_x: bool, sub_y: bool, is_compound: bool) -> HbdKernel {
    match (sub_x, sub_y, is_compound) {
        (false, false, false) => HbdKernel::Copy2d,
        (false, false, true) => HbdKernel::JntCopy2d,
        (false, true, false) => HbdKernel::Y,
        (false, true, true) => HbdKernel::JntY,
        (true, false, false) => HbdKernel::X,
        (true, false, true) => HbdKernel::JntX,
        (true, true, false) => HbdKernel::Sr2d,
        (true, true, true) => HbdKernel::Jnt2d,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::port_convolve::COMPOUND_ROUND1_BITS;

    /// The 10-bit compound rounds are the same 3/7 as 8-bit; the
    /// `intbufrange` correction does not fire until bd 12. Guards the
    /// assumption `round_offset` rests on.
    #[test]
    fn hbd_conv_params() {
        let c10 = ConvolveParams::no_round(false, 64, true, 10);
        assert_eq!((c10.round_0, c10.round_1), (3, COMPOUND_ROUND1_BITS));
        let s10 = ConvolveParams::single(false, 10);
        assert_eq!((s10.round_0, s10.round_1), (3, 11));
    }

    /// The dispatch table's index order — `[subX][subY][bi]` — transposed by
    /// accident is exactly the "assumed index order" trap.
    #[test]
    fn dispatch_table_index_order() {
        assert_eq!(hbd_kernel_for(false, true, false), HbdKernel::Y);
        assert_eq!(hbd_kernel_for(true, false, false), HbdKernel::X);
        assert_eq!(hbd_kernel_for(false, false, true), HbdKernel::JntCopy2d);
        assert_eq!(hbd_kernel_for(true, true, true), HbdKernel::Jnt2d);
    }
}
