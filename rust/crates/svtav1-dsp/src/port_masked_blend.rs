//! Applying the wedge / DIFFWTD mask in the CONV_BUF (no-round) domain —
//! the actual pixels of every masked-compound block.
//!
//! Ported from `Source/Lib/Codec/inter_prediction.c` (SVT-AV1 v4.2.0):
//! `av1_get_compound_type_mask` (:2332) and
//! `svt_aom_build_masked_compound_no_round` (:2347); plus
//! `svt_aom_lowbd_blend_a64_d16_mask_c` (blend_a64_mask.c) and
//! `svt_aom_highbd_blend_a64_d16_mask_c`, which are the two kernels it
//! dispatches to.
//!
//! # `d16` is not the same blend as `blend_a64_mask`
//!
//! [`crate::port_interintra::blend_a64_mask`] blends two 8-bit PLANES and
//! rounds by `AOM_BLEND_A64_ROUND_BITS` alone. These blend two `CONV_BUF_TYPE`
//! (16-bit, `round_1`-domain) intermediates: after the alpha blend they
//! subtract the same `round_offset` the `jnt_convolve` kernels added and then
//! shift by `round_bits`. Feeding CONV_BUF values to the plane blend, or plane
//! values to this one, is off by that offset everywhere.
//!
//! # The subsampling flags are DERIVED, not passed
//!
//! `svt_aom_build_masked_compound_no_round` computes
//! `subh = (2 << mi_size_high_log2[bsize]) == h` and the `subw` twin from the
//! `w`/`h` it is handed — i.e. it infers 4:2:0 chroma from the block being
//! half the luma size. C's own comment says this "may be refactored to pass in
//! subsampling factors directly"; until it is, the inference is the contract.

use crate::port_convolve::ConvolveParams;
use crate::port_masked_compound::{
    AOM_BLEND_A64_MAX_ALPHA, AOM_BLEND_A64_ROUND_BITS, CompoundType,
};
use crate::port_wedge_masks::{BLOCK_SIZES_ALL, WedgeMasks};

/// `FILTER_BITS`.
const FILTER_BITS: i32 = 7;

const BLOCK_W: [usize; BLOCK_SIZES_ALL] = [
    4, 4, 8, 8, 8, 16, 16, 16, 32, 32, 32, 64, 64, 64, 128, 128, 4, 16, 8, 32, 16, 64,
];
/// `mi_size_wide_log2[bsize]` — log2 of the width in 4x4 units.
const MI_SIZE_WIDE_LOG2: [u32; BLOCK_SIZES_ALL] = [
    0, 0, 1, 1, 1, 2, 2, 2, 3, 3, 3, 4, 4, 4, 5, 5, 0, 2, 1, 3, 2, 4,
];
/// `mi_size_high_log2[bsize]`.
const MI_SIZE_HIGH_LOG2: [u32; BLOCK_SIZES_ALL] = [
    0, 1, 0, 1, 2, 1, 2, 3, 2, 3, 4, 3, 4, 5, 4, 5, 2, 0, 3, 1, 4, 2,
];

#[inline]
fn round_power_of_two(value: i32, n: i32) -> i32 {
    if n == 0 {
        value
    } else {
        (value + (1 << (n - 1))) >> n
    }
}

/// `AOM_BLEND_AVG(v0, v1)`.
#[inline]
fn aom_blend_avg(v0: i32, v1: i32) -> i32 {
    round_power_of_two(v0 + v1, 1)
}

/// The mask value at output `(i, j)` for a given `(subw, subh)`.
#[inline]
fn mask_at(mask: &[u8], mask_stride: usize, i: usize, j: usize, subw: bool, subh: bool) -> i32 {
    match (subw, subh) {
        (false, false) => mask[i * mask_stride + j] as i32,
        (true, true) => round_power_of_two(
            mask[(2 * i) * mask_stride + 2 * j] as i32
                + mask[(2 * i + 1) * mask_stride + 2 * j] as i32
                + mask[(2 * i) * mask_stride + 2 * j + 1] as i32
                + mask[(2 * i + 1) * mask_stride + 2 * j + 1] as i32,
            2,
        ),
        (true, false) => aom_blend_avg(
            mask[i * mask_stride + 2 * j] as i32,
            mask[i * mask_stride + 2 * j + 1] as i32,
        ),
        (false, true) => aom_blend_avg(
            mask[(2 * i) * mask_stride + j] as i32,
            mask[(2 * i + 1) * mask_stride + j] as i32,
        ),
    }
}

/// `svt_aom_lowbd_blend_a64_d16_mask_c` (blend_a64_mask.c:113).
#[allow(clippy::too_many_arguments)]
pub fn lowbd_blend_a64_d16_mask(
    dst: &mut [u8],
    dst_stride: usize,
    src0: &[u16],
    src0_stride: usize,
    src1: &[u16],
    src1_stride: usize,
    mask: &[u8],
    mask_stride: usize,
    w: usize,
    h: usize,
    subw: bool,
    subh: bool,
    conv_params: &ConvolveParams,
) {
    let bd = 8i32;
    let offset_bits = bd + 2 * FILTER_BITS - conv_params.round_0;
    let round_offset =
        (1 << (offset_bits - conv_params.round_1)) + (1 << (offset_bits - conv_params.round_1 - 1));
    let round_bits = 2 * FILTER_BITS - conv_params.round_0 - conv_params.round_1;

    for i in 0..h {
        for j in 0..w {
            let m = mask_at(mask, mask_stride, i, j, subw, subh);
            let mut res = (m * src0[i * src0_stride + j] as i32
                + (AOM_BLEND_A64_MAX_ALPHA - m) * src1[i * src1_stride + j] as i32)
                >> AOM_BLEND_A64_ROUND_BITS;
            res -= round_offset;
            dst[i * dst_stride + j] = round_power_of_two(res, round_bits).clamp(0, 255) as u8;
        }
    }
}

/// `svt_aom_highbd_blend_a64_d16_mask_c` (blend_a64_mask.c:296).
///
/// C's saturation value comes from a `switch (bd)` that falls through to 255
/// for anything other than 10 or 12 — so an out-of-range `bd` saturates at
/// 8-bit rather than being rejected. That default is reproduced.
#[allow(clippy::too_many_arguments)]
pub fn highbd_blend_a64_d16_mask(
    dst: &mut [u16],
    dst_stride: usize,
    src0: &[u16],
    src0_stride: usize,
    src1: &[u16],
    src1_stride: usize,
    mask: &[u8],
    mask_stride: usize,
    w: usize,
    h: usize,
    subw: bool,
    subh: bool,
    conv_params: &ConvolveParams,
    bd: i32,
) {
    let offset_bits = bd + 2 * FILTER_BITS - conv_params.round_0;
    let round_offset =
        (1 << (offset_bits - conv_params.round_1)) + (1 << (offset_bits - conv_params.round_1 - 1));
    let round_bits = 2 * FILTER_BITS - conv_params.round_0 - conv_params.round_1;
    let saturation_value: i32 = match bd {
        10 => 1023,
        12 => 4095,
        _ => 255,
    };

    for i in 0..h {
        for j in 0..w {
            let m = mask_at(mask, mask_stride, i, j, subw, subh);
            let mut res = (m * src0[i * src0_stride + j] as i32
                + (AOM_BLEND_A64_MAX_ALPHA - m) * src1[i * src1_stride + j] as i32)
                >> AOM_BLEND_A64_ROUND_BITS;
            res -= round_offset;
            let v = round_power_of_two(res, round_bits).max(0);
            dst[i * dst_stride + j] = v.min(saturation_value) as u16;
        }
    }
}

/// `InterInterCompoundData` (definitions.h:1302).
#[derive(Debug, Clone, Copy)]
pub struct InterInterCompoundData {
    /// `type` — must be WEDGE or DIFFWTD here.
    pub compound_type: CompoundType,
    /// `wedge_index`.
    pub wedge_index: usize,
    /// `wedge_sign`.
    pub wedge_sign: usize,
}

/// `av1_get_compound_type_mask` (inter_prediction.c:2332) — WEDGE reads the
/// per-(index, sign, bsize) wedge mask; DIFFWTD reads the caller's `seg_mask`.
/// C `assert`s the type is masked and returns NULL otherwise.
pub fn get_compound_type_mask<'a>(
    comp: &InterInterCompoundData,
    seg_mask: &'a [u8],
    wedge: &'a WedgeMasks,
    bsize: usize,
) -> &'a [u8] {
    match comp.compound_type {
        CompoundType::Wedge => wedge.contiguous_soft_mask(comp.wedge_index, comp.wedge_sign, bsize),
        CompoundType::DiffWtd => seg_mask,
        other => panic!("av1_get_compound_type_mask on a non-masked compound type {other:?}"),
    }
}

/// `svt_aom_build_masked_compound_no_round` (inter_prediction.c:2347), 8-bit
/// destination.
#[allow(clippy::too_many_arguments)]
pub fn build_masked_compound_no_round(
    dst: &mut [u8],
    dst_stride: usize,
    src0: &[u16],
    src0_stride: usize,
    src1: &[u16],
    src1_stride: usize,
    comp: &InterInterCompoundData,
    seg_mask: &[u8],
    wedge: &WedgeMasks,
    bsize: usize,
    h: usize,
    w: usize,
    conv_params: &ConvolveParams,
) {
    let subh = (2usize << MI_SIZE_HIGH_LOG2[bsize]) == h;
    let subw = (2usize << MI_SIZE_WIDE_LOG2[bsize]) == w;
    let mask = get_compound_type_mask(comp, seg_mask, wedge, bsize);
    lowbd_blend_a64_d16_mask(
        dst,
        dst_stride,
        src0,
        src0_stride,
        src1,
        src1_stride,
        mask,
        BLOCK_W[bsize],
        w,
        h,
        subw,
        subh,
        conv_params,
    );
}

/// `svt_aom_build_masked_compound_no_round` (inter_prediction.c:2347), 16-bit
/// destination (`is_16bit`).
#[allow(clippy::too_many_arguments)]
pub fn build_masked_compound_no_round_hbd(
    dst: &mut [u16],
    dst_stride: usize,
    src0: &[u16],
    src0_stride: usize,
    src1: &[u16],
    src1_stride: usize,
    comp: &InterInterCompoundData,
    seg_mask: &[u8],
    wedge: &WedgeMasks,
    bsize: usize,
    h: usize,
    w: usize,
    conv_params: &ConvolveParams,
    bd: i32,
) {
    let subh = (2usize << MI_SIZE_HIGH_LOG2[bsize]) == h;
    let subw = (2usize << MI_SIZE_WIDE_LOG2[bsize]) == w;
    let mask = get_compound_type_mask(comp, seg_mask, wedge, bsize);
    highbd_blend_a64_d16_mask(
        dst,
        dst_stride,
        src0,
        src0_stride,
        src1,
        src1_stride,
        mask,
        BLOCK_W[bsize],
        w,
        h,
        subw,
        subh,
        conv_params,
        bd,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The subsampling flags are inferred from `w`/`h` against the block size:
    /// full size means no subsampling, half means 4:2:0 chroma.
    #[test]
    fn subsampling_is_inferred_from_the_block_size() {
        // BLOCK_16X16 = 6: mi_size_wide_log2 = 2, so 2 << 2 = 8 == w means subw.
        assert_eq!(2usize << MI_SIZE_WIDE_LOG2[6], 8);
        assert_eq!(2usize << MI_SIZE_HIGH_LOG2[6], 8);
        // The luma call passes w = h = 16, so neither flag is set.
        assert_ne!(2usize << MI_SIZE_WIDE_LOG2[6], 16);
    }

    /// `mi_size_*_log2` must be log2 of the block dimension in 4x4 units for
    /// every block size, or the inference above is wrong somewhere.
    #[test]
    fn mi_size_log2_tables_are_consistent() {
        const BLOCK_H: [usize; BLOCK_SIZES_ALL] = [
            4, 8, 4, 8, 16, 8, 16, 32, 16, 32, 64, 32, 64, 128, 64, 128, 16, 4, 32, 8, 64, 16,
        ];
        for b in 0..BLOCK_SIZES_ALL {
            assert_eq!(
                1usize << MI_SIZE_WIDE_LOG2[b],
                BLOCK_W[b] / 4,
                "wide log2 at {b}"
            );
            assert_eq!(
                1usize << MI_SIZE_HIGH_LOG2[b],
                BLOCK_H[b] / 4,
                "high log2 at {b}"
            );
        }
    }
}
