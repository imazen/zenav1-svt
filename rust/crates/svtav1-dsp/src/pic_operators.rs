//! Port of the pixel-domain kernels in `Codec/pic_operators.c`.
//!
//! These are the residual and distortion primitives the mode-decision and
//! encode loops run on every candidate. C states each one as a
//! pointer-walking `while` loop over `(area_width, area_height)` with an
//! explicit stride; this port states the same arithmetic as a strided row
//! walk over slices, which removes the pointer arithmetic without changing a
//! single operation.
//!
//! Integer semantics carried over deliberately (they are part of the
//! contract, not incidental):
//!
//! * The residual kernels store `((int16_t)input) - ((int16_t)pred)` into an
//!   `int16_t`. For 8-bit inputs the difference is in `[-255, 255]` and the
//!   narrowing is exact. For the 16-bit kernel C performs TWO
//!   implementation-defined narrowings (`(int16_t)` on each `uint16_t`
//!   operand, then again on the store); clang/gcc both wrap, so this port
//!   spells them `as i16` / `wrapping_sub`, which is that same wrap and is
//!   defined in Rust. Inside the encoder's own envelope (10/12-bit samples,
//!   `<= 4095`) every narrowing is the identity, so the wrap is unreachable
//!   in production and only observable to a differential that feeds
//!   full-range `u16`. `c_parity_pic_operators.rs` feeds exactly that.
//! * The distortion kernels square an `i64` difference and accumulate into
//!   `uint64_t`. Two different overflow rules meet here and the port matches
//!   each one deliberately:
//!   - The ACCUMULATION is `uint64_t +=`, which is modular in C, not UB, so
//!     the port uses `wrapping_add` rather than a wider type that would
//!     disagree on a saturated 64x64 block.
//!   - The SQUARE is `(int64_t)d * (int64_t)d`. Both operands come from
//!     `int32_t` coefficients, so `|d|` can reach `2^32` and the product can
//!     exceed `i64::MAX` — signed overflow, i.e. UB in C, wrapping in every
//!     compiler that actually builds it. The port spells it `wrapping_mul`:
//!     identical to C wherever C is defined, and defined (never a panic)
//!     where C is not. Real AV1 coefficients are bounded far below that, so
//!     the wrap is unreachable from the encoder; it exists so no input can
//!     make a codec kernel panic.
//!
//! Evidence: tier 1. `crates/svtav1-dsp/tests/c_parity_pic_operators.rs`
//! drives the real exported `svt_residual_kernel8bit_c`,
//! `svt_residual_kernel16bit_c`, `svt_full_distortion_kernel32_bits_c`,
//! `svt_full_distortion_kernel_cbf_zero32_bits_c` and
//! `svt_aom_picture_full_distortion32_bits_single` through
//! `svtav1-cref`.

/// C's `uint64_t distortion_result[DIST_CALC_TOTAL]` out-parameter, named.
///
/// `DIST_CALC_RESIDUAL` = 0, `DIST_CALC_PREDICTION` = 1
/// (`EbDefinitions.h`). Returning a struct instead of filling a
/// caller-supplied 2-element array is the whole of the shape change; both
/// fields carry C's values unmodified.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FullDistortion {
    /// `DIST_CALC_RESIDUAL`: SSE between the coefficients and the
    /// dequantized/reconstructed coefficients.
    pub residual: u64,
    /// `DIST_CALC_PREDICTION`: energy of the coefficients themselves (the
    /// distortion that would result from coding nothing).
    pub prediction: u64,
}

/// Rows of a strided plane, as `area_width`-long slices.
///
/// One bounds check per row instead of one per sample: `input[r * stride..]`
/// is checked once, then `[..width]` fixes the length so the inner loop
/// indexes a slice the optimizer knows the extent of.
#[inline]
fn rows<T>(buf: &[T], stride: usize, width: usize, height: usize) -> impl Iterator<Item = &[T]> {
    (0..height).map(move |r| &buf[r * stride..][..width])
}

/// C `svt_residual_kernel8bit_c` (pic_operators.c:52-70).
///
/// `residual[c] = input[c] - pred[c]` over an `area_width x area_height`
/// window, each plane on its own stride.
pub fn residual_kernel_8bit(
    input: &[u8],
    input_stride: usize,
    pred: &[u8],
    pred_stride: usize,
    residual: &mut [i16],
    residual_stride: usize,
    area_width: usize,
    area_height: usize,
) {
    for (r, (i_row, p_row)) in rows(input, input_stride, area_width, area_height)
        .zip(rows(pred, pred_stride, area_width, area_height))
        .enumerate()
    {
        let out = &mut residual[r * residual_stride..][..area_width];
        for ((o, &i), &p) in out.iter_mut().zip(i_row).zip(p_row) {
            *o = i16::from(i) - i16::from(p);
        }
    }
}

/// C `svt_residual_kernel16bit_c` (pic_operators.c:27-45).
///
/// Same shape as [`residual_kernel_8bit`] on 16-bit planes. See the module
/// doc for why the two narrowings are spelled as wrapping casts.
pub fn residual_kernel_16bit(
    input: &[u16],
    input_stride: usize,
    pred: &[u16],
    pred_stride: usize,
    residual: &mut [i16],
    residual_stride: usize,
    area_width: usize,
    area_height: usize,
) {
    for (r, (i_row, p_row)) in rows(input, input_stride, area_width, area_height)
        .zip(rows(pred, pred_stride, area_width, area_height))
        .enumerate()
    {
        let out = &mut residual[r * residual_stride..][..area_width];
        for ((o, &i), &p) in out.iter_mut().zip(i_row).zip(p_row) {
            *o = (i as i16).wrapping_sub(p as i16);
        }
    }
}

/// C `svt_full_distortion_kernel32_bits_c` (pic_operators.c:77-97).
///
/// Frequency-domain distortion: `residual` is the SSE between `coeff` and
/// `recon_coeff`, `prediction` is the energy of `coeff`. Both planes share
/// one `stride`, exactly as C does (it advances both pointers by `stride`).
pub fn full_distortion_kernel32_bits(
    coeff: &[i32],
    recon_coeff: &[i32],
    stride: usize,
    area_width: usize,
    area_height: usize,
) -> FullDistortion {
    let mut dist = FullDistortion::default();
    for (c_row, r_row) in rows(coeff, stride, area_width, area_height).zip(rows(
        recon_coeff,
        stride,
        area_width,
        area_height,
    )) {
        for (&c, &r) in c_row.iter().zip(r_row) {
            let d = i64::from(c) - i64::from(r);
            dist.residual = dist.residual.wrapping_add(d.wrapping_mul(d) as u64);
            let c = i64::from(c);
            dist.prediction = dist.prediction.wrapping_add(c.wrapping_mul(c) as u64);
        }
    }
    dist
}

/// C `svt_full_distortion_kernel_cbf_zero32_bits_c` (pic_operators.c:128-146).
///
/// The "code nothing" case: both fields carry the same coefficient energy,
/// because zeroing the block makes the residual distortion equal to it.
pub fn full_distortion_kernel_cbf_zero32_bits(
    coeff: &[i32],
    coeff_stride: usize,
    area_width: usize,
    area_height: usize,
) -> FullDistortion {
    let mut prediction: u64 = 0;
    for c_row in rows(coeff, coeff_stride, area_width, area_height) {
        for &c in c_row {
            let c = i64::from(c);
            prediction = prediction.wrapping_add(c.wrapping_mul(c) as u64);
        }
    }
    FullDistortion {
        residual: prediction,
        prediction,
    }
}

/// C `svt_aom_picture_full_distortion32_bits_single` (pic_operators.c:149-160).
///
/// Selects between the two kernels above on whether the block has any
/// non-zero coefficient. C takes a `uint32_t cnt_nz_coeff` and tests it for
/// truth; the port takes the boolean that test computes, since no caller
/// uses the count for anything else.
pub fn picture_full_distortion32_bits_single(
    coeff: &[i32],
    recon_coeff: &[i32],
    stride: usize,
    bwidth: usize,
    bheight: usize,
    has_nz_coeff: bool,
) -> FullDistortion {
    if has_nz_coeff {
        full_distortion_kernel32_bits(coeff, recon_coeff, stride, bwidth, bheight)
    } else {
        full_distortion_kernel_cbf_zero32_bits(coeff, stride, bwidth, bheight)
    }
}

/// C `svt_spatial_full_distortion_kernel_c`
/// (`C_DEFAULT/picture_operators_c.c:55-73`) — the 8-bit spatial SSE the
/// `svt_spatial_full_distortion_kernel` RTCD slot dispatches to, and the
/// kernel `svt_spatial_full_distortion_kernel_facade` wraps on the
/// `hbd_md == false` arm. Its 16-bit twin is
/// [`crate::hbd::full_distortion_kernel16_bits`].
///
/// C's `recon_offset` is `int32_t` and every call site passes a
/// non-negative block origin, so this port takes `usize`.
pub fn spatial_full_distortion_kernel(
    input: &[u8],
    input_offset: usize,
    input_stride: usize,
    recon: &[u8],
    recon_offset: usize,
    recon_stride: usize,
    area_width: usize,
    area_height: usize,
) -> u64 {
    let mut spatial_distortion: u64 = 0;
    for r in 0..area_height {
        let i_row = &input[input_offset + r * input_stride..][..area_width];
        let r_row = &recon[recon_offset + r * recon_stride..][..area_width];
        for (&i, &c) in i_row.iter().zip(r_row) {
            let d = i64::from(i) - i64::from(c);
            spatial_distortion = spatial_distortion.wrapping_add(d.wrapping_mul(d) as u64);
        }
    }
    spatial_distortion
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    #[test]
    fn residual_8bit_is_plain_difference_on_strided_planes() {
        // 3x2 window inside 5-wide planes, residual on its own stride 4.
        let input: Vec<u8> = (0..10u8).collect();
        let pred: Vec<u8> = (0..10u8).map(|v| v.wrapping_mul(2)).collect();
        let mut res = vec![0i16; 8];
        residual_kernel_8bit(&input, 5, &pred, 5, &mut res, 4, 3, 2);
        assert_eq!(&res[..3], &[0, -1, -2]);
        assert_eq!(&res[4..7], &[-5, -6, -7]);
        // Untouched columns stay zero (the kernel writes area_width only).
        assert_eq!(res[3], 0);
        assert_eq!(res[7], 0);
    }

    #[test]
    fn residual_16bit_wraps_like_c_outside_the_encoder_envelope() {
        // 40000 as int16_t is -25536; 100 stays 100. C narrows both operands
        // and then the store, so the result is the wrapping difference.
        let input = [40_000u16, 1023];
        let pred = [100u16, 0];
        let mut res = [0i16; 2];
        residual_kernel_16bit(&input, 2, &pred, 2, &mut res, 2, 2, 1);
        assert_eq!(res[0], (40_000u16 as i16).wrapping_sub(100));
        // Inside the envelope (<= 4095) it is the plain difference.
        assert_eq!(res[1], 1023);
    }

    #[test]
    fn cbf_zero_duplicates_the_coefficient_energy() {
        let coeff = [3i32, -4, 0, 12];
        let d = full_distortion_kernel_cbf_zero32_bits(&coeff, 2, 2, 2);
        assert_eq!(d.residual, 9 + 16 + 144);
        assert_eq!(d.prediction, d.residual);
    }

    #[test]
    fn single_selects_the_kernel_on_the_nonzero_flag() {
        let coeff = [10i32, -10, 5, 5];
        let recon = [8i32, -8, 5, 4];
        let nz = picture_full_distortion32_bits_single(&coeff, &recon, 2, 2, 2, true);
        assert_eq!(nz.residual, 4 + 4 + 0 + 1);
        assert_eq!(nz.prediction, 100 + 100 + 25 + 25);
        let zero = picture_full_distortion32_bits_single(&coeff, &recon, 2, 2, 2, false);
        assert_eq!(zero.residual, nz.prediction);
        assert_eq!(zero.prediction, nz.prediction);
    }

    #[test]
    fn spatial_kernel_honours_both_offsets_and_strides() {
        let input: Vec<u8> = (0..32u8).collect();
        let recon: Vec<u8> = (0..32u8).map(|v| v.saturating_sub(1)).collect();
        // 2x2 window at offset 9 (input) / 8 (recon), strides 8 and 8.
        let d = spatial_full_distortion_kernel(&input, 9, 8, &recon, 8, 8, 2, 2);
        // input 9,10 / 17,18  vs recon 7,8 / 15,16 -> each diff 2
        assert_eq!(d, 4 * 4);
    }
}
