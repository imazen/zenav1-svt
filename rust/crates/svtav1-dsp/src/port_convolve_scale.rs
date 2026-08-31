//! The scaled-reference motion-compensation kernels.
//!
//! Ported from `Source/Lib/Codec/inter_prediction.c` (SVT-AV1 v4.2.0):
//! `svt_av1_convolve_2d_scale_c` (:448) and
//! `svt_av1_highbd_convolve_2d_scale_c` (:828).
//!
//! # This is the port `c_parity_scale.rs` is waiting for
//!
//! `svtav1-dsp/src/scale.rs::scaled_prediction` is a homegrown two-pass filter
//! through a **u8** intermediate driven by a Q14 `ScaleFactors`, and
//! `crates/svtav1-dsp/tests/c_parity_scale.rs` pins its divergence from the C
//! kernel with an `assert_ne!` plus a comment saying to flip it to `assert_eq!`
//! "when scale.rs is ported". The functions here are that port, with the C
//! contract (`SCALE_SUBPEL_BITS = 10` phase domain, `InterpFilterParams`, a
//! 16-bit intermediate and the ROUND0/ROUND1 offset scheme) rather than
//! scale.rs's. `scale.rs` is deliberately NOT edited — re-pointing its callers
//! at these and flipping that pin is a separate, caller-side change.
//!
//! # What the scaled kernels do differently from the unscaled ones
//!
//! Each output column advances a phase accumulator by `x_step_qn` instead of
//! reusing one `subpel_x_q4`, so the SOURCE position and the FILTER PHASE both
//! move per pixel: `src_x = src_horiz[x_qn >> SCALE_SUBPEL_BITS]` and
//! `x_filter_idx = (x_qn & SCALE_SUBPEL_MASK) >> SCALE_EXTRA_BITS`. The
//! intermediate is `im_h = (((h - 1) * y_step_qn + subpel_y_qn) >> 10) + taps`
//! rows tall — a function of the vertical step, not `h + taps - 1`.
//!
//! And the vertical loop is COLUMN-MAJOR (`for x { for y }`), with `src_vert`
//! incremented once per column. Transposing it to row-major changes nothing
//! arithmetically but makes the `src_vert++` walk easy to get wrong; the port
//! keeps C's order and indexes explicitly.

use crate::port_convolve::{
    ConvolveParams, DIST_PRECISION_BITS, FILTER_BITS, FilterParams, SUBPEL_TAPS, SrcView,
};
use crate::port_convolve_hbd::SrcView16;

/// `SCALE_SUBPEL_BITS` (definitions.h:462).
pub const SCALE_SUBPEL_BITS: i32 = 10;
/// `SCALE_SUBPEL_MASK` (definitions.h:464).
pub const SCALE_SUBPEL_MASK: i32 = (1 << SCALE_SUBPEL_BITS) - 1;
/// `SCALE_EXTRA_BITS` (definitions.h:465).
pub const SCALE_EXTRA_BITS: i32 = SCALE_SUBPEL_BITS - 4;

#[inline]
fn round_power_of_two(value: i32, n: i32) -> i32 {
    if n == 0 {
        value
    } else {
        (value + (1 << (n - 1))) >> n
    }
}

#[inline]
fn clip_pixel_8(val: i32) -> u8 {
    val.clamp(0, 255) as u8
}

#[inline]
fn clip_pixel_highbd(val: i32, bd: i32) -> u16 {
    val.clamp(0, (1 << bd) - 1) as u16
}

/// The number of intermediate rows the horizontal pass must produce.
pub fn scale_im_h(h: usize, subpel_y_qn: i32, y_step_qn: i32) -> usize {
    (((h as i32 - 1) * y_step_qn + subpel_y_qn) >> SCALE_SUBPEL_BITS) as usize + SUBPEL_TAPS
}

/// `svt_av1_convolve_2d_scale_c` (inter_prediction.c:448).
///
/// `conv_buf` is `conv_params->dst`; it is only touched on the compound arms.
#[allow(clippy::too_many_arguments)]
pub fn convolve_2d_scale(
    src: SrcView<'_>,
    dst: &mut [u8],
    dst_stride: usize,
    conv_buf: &mut [u16],
    w: usize,
    h: usize,
    filter_x: &FilterParams,
    filter_y: &FilterParams,
    subpel_x_qn: i32,
    x_step_qn: i32,
    subpel_y_qn: i32,
    y_step_qn: i32,
    conv_params: &ConvolveParams,
) {
    let im_h = scale_im_h(h, subpel_y_qn, y_step_qn);
    let im_stride = w;
    let fo_vert = (SUBPEL_TAPS / 2 - 1) as i32;
    let fo_horiz = (SUBPEL_TAPS / 2 - 1) as i32;
    let bd = 8i32;
    let bits = FILTER_BITS * 2 - conv_params.round_0 - conv_params.round_1;
    let dst16_stride = conv_params.dst_stride;

    // Horizontal pass. `src_horiz` starts `fo_vert` rows ABOVE the block —
    // the VERTICAL front offset, applied to the horizontal pass's rows.
    let mut im_block = alloc::vec![0i16; im_h * im_stride];
    for y in 0..im_h {
        let mut x_qn = subpel_x_qn;
        for x in 0..w {
            let src_x = x_qn >> SCALE_SUBPEL_BITS;
            let x_filter_idx = (x_qn & SCALE_SUBPEL_MASK) >> SCALE_EXTRA_BITS;
            let x_filter = filter_x.subpel_kernel(x_filter_idx);
            let mut sum = 1i32 << (bd + FILTER_BITS - 1);
            for k in 0..SUBPEL_TAPS {
                sum += x_filter[k] as i32 * src.at(y as i32 - fo_vert, src_x + k as i32 - fo_horiz);
            }
            im_block[y * im_stride + x] = round_power_of_two(sum, conv_params.round_0) as i16;
            x_qn += x_step_qn;
        }
    }

    // Vertical pass, column-major as in C.
    let offset_bits = bd + 2 * FILTER_BITS - conv_params.round_0;
    let round_offset =
        (1 << (offset_bits - conv_params.round_1)) + (1 << (offset_bits - conv_params.round_1 - 1));
    for x in 0..w {
        let mut y_qn = subpel_y_qn;
        for y in 0..h {
            let src_y = (y_qn >> SCALE_SUBPEL_BITS) as usize;
            let y_filter_idx = (y_qn & SCALE_SUBPEL_MASK) >> SCALE_EXTRA_BITS;
            let y_filter = filter_y.subpel_kernel(y_filter_idx);
            let mut sum = 1i32 << offset_bits;
            for k in 0..SUBPEL_TAPS {
                // `src_vert = im_block + fo_vert * im_stride` then
                // `src_y[(k - fo_vert) * im_stride]`, i.e. row `src_y + k`.
                sum += y_filter[k] as i32 * im_block[(src_y + k) * im_stride + x] as i32;
            }
            // C stores through a CONV_BUF_TYPE (u16) here.
            let res = round_power_of_two(sum, conv_params.round_1) as u16;
            if conv_params.is_compound {
                if conv_params.do_average {
                    let mut tmp = conv_buf[y * dst16_stride + x] as i32;
                    if conv_params.use_jnt_comp_avg {
                        tmp = tmp * conv_params.fwd_offset + res as i32 * conv_params.bck_offset;
                        tmp >>= DIST_PRECISION_BITS;
                    } else {
                        tmp += res as i32;
                        tmp >>= 1;
                    }
                    tmp -= round_offset;
                    dst[y * dst_stride + x] = clip_pixel_8(round_power_of_two(tmp, bits));
                } else {
                    conv_buf[y * dst16_stride + x] = res;
                }
            } else {
                let tmp = res as i32 - round_offset;
                dst[y * dst_stride + x] = clip_pixel_8(round_power_of_two(tmp, bits));
            }
            y_qn += y_step_qn;
        }
    }
}

/// `svt_av1_highbd_convolve_2d_scale_c` (inter_prediction.c:828).
#[allow(clippy::too_many_arguments)]
pub fn highbd_convolve_2d_scale(
    src: SrcView16<'_>,
    dst: &mut [u16],
    dst_stride: usize,
    conv_buf: &mut [u16],
    w: usize,
    h: usize,
    filter_x: &FilterParams,
    filter_y: &FilterParams,
    subpel_x_qn: i32,
    x_step_qn: i32,
    subpel_y_qn: i32,
    y_step_qn: i32,
    conv_params: &ConvolveParams,
    bd: i32,
) {
    let im_h = scale_im_h(h, subpel_y_qn, y_step_qn);
    let im_stride = w;
    let fo_vert = (SUBPEL_TAPS / 2 - 1) as i32;
    let fo_horiz = (SUBPEL_TAPS / 2 - 1) as i32;
    let bits = FILTER_BITS * 2 - conv_params.round_0 - conv_params.round_1;
    let dst16_stride = conv_params.dst_stride;

    let mut im_block = alloc::vec![0i16; im_h * im_stride];
    for y in 0..im_h {
        let mut x_qn = subpel_x_qn;
        for x in 0..w {
            let src_x = x_qn >> SCALE_SUBPEL_BITS;
            let x_filter_idx = (x_qn & SCALE_SUBPEL_MASK) >> SCALE_EXTRA_BITS;
            let x_filter = filter_x.subpel_kernel(x_filter_idx);
            let mut sum = 1i32 << (bd + FILTER_BITS - 1);
            for k in 0..SUBPEL_TAPS {
                sum += x_filter[k] as i32 * src.at(y as i32 - fo_vert, src_x + k as i32 - fo_horiz);
            }
            im_block[y * im_stride + x] = round_power_of_two(sum, conv_params.round_0) as i16;
            x_qn += x_step_qn;
        }
    }

    let offset_bits = bd + 2 * FILTER_BITS - conv_params.round_0;
    let round_offset =
        (1 << (offset_bits - conv_params.round_1)) + (1 << (offset_bits - conv_params.round_1 - 1));
    for x in 0..w {
        let mut y_qn = subpel_y_qn;
        for y in 0..h {
            let src_y = (y_qn >> SCALE_SUBPEL_BITS) as usize;
            let y_filter_idx = (y_qn & SCALE_SUBPEL_MASK) >> SCALE_EXTRA_BITS;
            let y_filter = filter_y.subpel_kernel(y_filter_idx);
            let mut sum = 1i32 << offset_bits;
            for k in 0..SUBPEL_TAPS {
                sum += y_filter[k] as i32 * im_block[(src_y + k) * im_stride + x] as i32;
            }
            let res = round_power_of_two(sum, conv_params.round_1) as u16;
            if conv_params.is_compound {
                if conv_params.do_average {
                    let mut tmp = conv_buf[y * dst16_stride + x] as i32;
                    if conv_params.use_jnt_comp_avg {
                        tmp = tmp * conv_params.fwd_offset + res as i32 * conv_params.bck_offset;
                        tmp >>= DIST_PRECISION_BITS;
                    } else {
                        tmp += res as i32;
                        tmp >>= 1;
                    }
                    tmp -= round_offset;
                    dst[y * dst_stride + x] = clip_pixel_highbd(round_power_of_two(tmp, bits), bd);
                } else {
                    conv_buf[y * dst16_stride + x] = res;
                }
            } else {
                let tmp = res as i32 - round_offset;
                dst[y * dst_stride + x] = clip_pixel_highbd(round_power_of_two(tmp, bits), bd);
            }
            y_qn += y_step_qn;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `im_h` follows the VERTICAL STEP, not the block height. At 1:1 (step
    /// 1024, phase 0) it collapses to `h - 1 + taps`, the unscaled kernel's
    /// `h + taps - 1`; at 2:1 it is nearly twice that.
    #[test]
    fn intermediate_height_follows_the_step() {
        assert_eq!(scale_im_h(16, 0, 1024), 16 - 1 + 8);
        assert_eq!(scale_im_h(16, 0, 2048), 30 + 8);
        assert_eq!(scale_im_h(16, 512, 1024), 15 + 8);
    }
}
