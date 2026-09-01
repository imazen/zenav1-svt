//! The CONV_BUF-domain DIFFWTD mask builder.
//!
//! Ported from `Source/Lib/C_DEFAULT/inter_prediction_c.c` (SVT-AV1 v4.2.0):
//! `diffwtd_mask_d16` (:16) and `svt_av1_build_compound_diffwtd_mask_d16_c`
//! (:30).
//!
//! # Why this is NOT the same function as the one in `port_masked_compound`
//!
//! [`crate::port_masked_compound::build_compound_diffwtd_mask`] and its
//! `_highbd` twin (inter_prediction.c:154 / :139) take **pixels** and derive
//! `diff` with a plain right shift by `bd - 8`. This one takes the
//! **CONV_BUF** (`uint16_t`, the `round_1`-domain compound intermediate) and
//! derives `diff` with a ROUNDING shift by
//! `2*FILTER_BITS - round_0 - round_1 + (bd - 8)`. Truncating vs rounding is
//! a real difference at every odd LSB, so neither can stand in for the other.
//!
//! The pixel-domain pair is dead on the compound path: `pick_interinter_seg`
//! (enc_inter_prediction.c) picks `mask_type` from the two ranked
//! **predictions**, but the mask that is actually blended into the block —
//! and therefore the one whose value reaches the bitstream's reconstruction —
//! is built here, from the two CONV_BUFs, at
//! `enc_inter_prediction.c:169` (`av1_make_masked_inter_predictor`) and
//! `:1681` (`av1_make_masked_warp_inter_predictor`), immediately before
//! `svt_aom_build_masked_compound_no_round`.
//!
//! # Evidence
//!
//! TIER 1 — `svt_av1_build_compound_diffwtd_mask_d16_c` is an exported symbol
//! (`nm`: `T`), gated in `tests/c_parity_port_diffwtd_d16.rs` against both the
//! scalar `_c` entry and the RTCD-dispatched pointer.
//!
//! # Integer widths
//!
//! C computes `abs(src0[..] - src1[..])` on two `uint16_t` values that the
//! usual arithmetic conversions promote to `int`, so the subtraction is
//! **signed 32-bit** and cannot wrap for any `u16` input. This port uses
//! `i32` for the same reason, and `u16::abs_diff` — which is the same value
//! without the promotion round-trip.

use crate::port_convolve::{ConvolveParams, FILTER_BITS};
use crate::port_masked_compound::{AOM_BLEND_A64_MAX_ALPHA, DIFF_FACTOR, DiffwtdMaskType};

/// `ROUND_POWER_OF_TWO` (definitions.h) on a non-negative `i32`.
///
/// `n == 0` is `(value + 0) >> 0`, which C's macro also yields
/// (`(1 << 0) >> 1 == 0`); it is reachable in principle with
/// single-prediction rounding params at 8-bit, though every live caller of
/// this module passes compound params.
#[inline]
fn round_power_of_two(value: i32, n: i32) -> i32 {
    debug_assert!((0..31).contains(&n));
    if n == 0 {
        value
    } else {
        (value + (1 << (n - 1))) >> n
    }
}

/// The shift `diffwtd_mask_d16` applies to `|src0 - src1|`, and the one place
/// its three inputs are combined.
///
/// C recomputes this inline as
/// `2 * FILTER_BITS - conv_params->round_0 - conv_params->round_1 + (bd - 8)`.
#[inline]
pub fn d16_diff_round(conv_params: &ConvolveParams, bd: i32) -> i32 {
    2 * FILTER_BITS - conv_params.round_0 - conv_params.round_1 + (bd - 8)
}

/// `diffwtd_mask_d16` (C_DEFAULT/inter_prediction_c.c:16).
///
/// `mask` is written densely at stride `w` — C indexes it `mask[i * w + j]`,
/// NOT at a caller-supplied stride, so the row chunking below is the same
/// addressing and not a simplification.
fn diffwtd_mask_d16(
    mask: &mut [u8],
    which_inverse: bool,
    mask_base: i32,
    src0: &[u16],
    src0_stride: usize,
    src1: &[u16],
    src1_stride: usize,
    h: usize,
    w: usize,
    conv_params: &ConvolveParams,
    bd: i32,
) {
    let round = d16_diff_round(conv_params, bd);
    for (i, out_row) in mask.chunks_exact_mut(w).take(h).enumerate() {
        let r0 = &src0[i * src0_stride..][..w];
        let r1 = &src1[i * src1_stride..][..w];
        for ((m, &a), &b) in out_row.iter_mut().zip(r0).zip(r1) {
            let diff = round_power_of_two(a.abs_diff(b) as i32, round);
            let v = (mask_base + diff / DIFF_FACTOR).clamp(0, AOM_BLEND_A64_MAX_ALPHA);
            *m = if which_inverse {
                (AOM_BLEND_A64_MAX_ALPHA - v) as u8
            } else {
                v as u8
            };
        }
    }
}

/// `svt_av1_build_compound_diffwtd_mask_d16_c`
/// (C_DEFAULT/inter_prediction_c.c:30).
///
/// C's `switch` has a `default: assert(0)` arm for the two mask types AV1
/// does not define; [`DiffwtdMaskType`] has exactly the two live values, so
/// the unrepresentable arm is gone rather than translated into a panic.
/// Both live types share `mask_base = 38` and differ only in the inversion.
#[allow(clippy::too_many_arguments)]
pub fn build_compound_diffwtd_mask_d16(
    mask: &mut [u8],
    mask_type: DiffwtdMaskType,
    src0: &[u16],
    src0_stride: usize,
    src1: &[u16],
    src1_stride: usize,
    h: usize,
    w: usize,
    conv_params: &ConvolveParams,
    bd: i32,
) {
    diffwtd_mask_d16(
        mask,
        mask_type == DiffwtdMaskType::D38Inv,
        38,
        src0,
        src0_stride,
        src1,
        src1_stride,
        h,
        w,
        conv_params,
        bd,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_matches_the_documented_compound_values() {
        // 8-bit compound: round_0 = 3, round_1 = COMPOUND_ROUND1_BITS = 7.
        let cp8 = ConvolveParams::no_round(false, 0, true, 8);
        assert_eq!((cp8.round_0, cp8.round_1), (3, 7));
        assert_eq!(d16_diff_round(&cp8, 8), 4);
        // 10-bit compound: same rounds, +2 from (bd - 8).
        let cp10 = ConvolveParams::no_round(false, 0, true, 10);
        assert_eq!((cp10.round_0, cp10.round_1), (3, 7));
        assert_eq!(d16_diff_round(&cp10, 10), 6);
    }

    #[test]
    fn inverse_is_the_alpha_complement_of_the_forward_mask() {
        let cp = ConvolveParams::no_round(false, 0, true, 8);
        let (w, h) = (8usize, 4usize);
        let src0: alloc::vec::Vec<u16> = (0..w * h).map(|i| (i * 137 % 4096) as u16).collect();
        let src1: alloc::vec::Vec<u16> = (0..w * h).map(|i| (i * 991 % 4096) as u16).collect();
        let mut fwd = alloc::vec![0u8; w * h];
        let mut inv = alloc::vec![0u8; w * h];
        build_compound_diffwtd_mask_d16(
            &mut fwd,
            DiffwtdMaskType::D38,
            &src0,
            w,
            &src1,
            w,
            h,
            w,
            &cp,
            8,
        );
        build_compound_diffwtd_mask_d16(
            &mut inv,
            DiffwtdMaskType::D38Inv,
            &src0,
            w,
            &src1,
            w,
            h,
            w,
            &cp,
            8,
        );
        for (f, i) in fwd.iter().zip(&inv) {
            assert_eq!(*f as i32 + *i as i32, AOM_BLEND_A64_MAX_ALPHA);
        }
    }
}
