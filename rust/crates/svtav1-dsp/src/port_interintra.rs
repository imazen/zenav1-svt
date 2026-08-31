//! Inter-intra compound blending.
//!
//! Ported from `Source/Lib/Codec/inter_prediction.c` (SVT-AV1 v4.2.0):
//! `build_smooth_interintra_mask` (:2233), `init_ii_masks` (:2282),
//! `get_ii_mask` (:2294), `svt_aom_combine_interintra` (:2468) and
//! `svt_aom_combine_interintra_highbd` (:2298).
//!
//! `enable_interintra_compound` is TRUE for every preset <= 8 in this port's
//! own sequence-header derivation (`svtav1-encoder/src/speed_config.rs:221`),
//! so this covers most of the shipping envelope rather than a corner.
//!
//! # `svt_aom_blend_a64_mask` comes from another file
//!
//! `combine_interintra` finishes in `svt_aom_blend_a64_mask_c` /
//! `svt_aom_highbd_blend_a64_mask_c` (`Source/Lib/Codec/blend_a64_mask.c:207`
//! and `:254`), which belong to a different module group. They are ported here
//! as the private [`blend_a64_mask`] / [`highbd_blend_a64_mask`] because
//! nothing else in the port supplies them (`obmc.rs` has only the 1-D
//! `v`/`h`-mask variants, `copy.rs` only a uniform blend). If a blend lane
//! lands a canonical copy, this one should be deleted in favour of it — it is
//! not a second opinion, it is a stand-in with the same C provenance.

use crate::port_masked_compound::aom_blend_a64;
use crate::port_wedge_masks::{BLOCK_SIZES_ALL, WedgeMasks, is_interintra_wedge_used};
use alloc::vec;
use alloc::vec::Vec;

/// `INTERINTRA_MODES` (definitions.h).
pub const INTERINTRA_MODES: usize = 4;
/// `MAX_INTERINTRA_SB_SQUARE` (inter_prediction.h:66) — 32*32.
pub const MAX_INTERINTRA_SB_SQUARE: usize = 32 * 32;
/// `BLOCK_32X32` in the `BlockSize` ordering.
pub const BLOCK_32X32: usize = 9;

/// `InterIntraMode` (definitions.h).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum InterIntraMode {
    /// `II_DC_PRED`
    DcPred = 0,
    /// `II_V_PRED`
    VPred = 1,
    /// `II_H_PRED`
    HPred = 2,
    /// `II_SMOOTH_PRED`
    SmoothPred = 3,
}

impl InterIntraMode {
    /// The four modes in C's enum order.
    pub const ALL: [Self; INTERINTRA_MODES] =
        [Self::DcPred, Self::VPred, Self::HPred, Self::SmoothPred];
}

/// `ii_weights1d` (inter_prediction.c:2217) — `MAX_SB_SIZE` = 128 entries.
pub const II_WEIGHTS_1D: [u8; 128] = [
    60, 58, 56, 54, 52, 50, 48, 47, 45, 44, 42, 41, 39, 38, 37, 35, 34, 33, 32, 31, 30, 29, 28, 27,
    26, 25, 24, 23, 22, 22, 21, 20, 19, 19, 18, 18, 17, 16, 16, 15, 15, 14, 14, 13, 13, 12, 12, 12,
    11, 11, 10, 10, 10, 9, 9, 9, 8, 8, 8, 8, 7, 7, 7, 7, 6, 6, 6, 6, 6, 5, 5, 5, 5, 5, 4, 4, 4, 4,
    4, 4, 4, 4, 3, 3, 3, 3, 3, 3, 3, 3, 3, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
];

/// `ii_size_scales` (inter_prediction.c:2226), indexed by `BlockSize`.
pub const II_SIZE_SCALES: [usize; BLOCK_SIZES_ALL] = [
    32, 16, 16, 16, 8, 8, 8, 4, 4, 4, 2, 2, 2, 1, 1, 1, 8, 8, 4, 4, 2, 2,
];

const BLOCK_W: [usize; BLOCK_SIZES_ALL] = [
    4, 4, 8, 8, 8, 16, 16, 16, 32, 32, 32, 64, 64, 64, 128, 128, 4, 16, 8, 32, 16, 64,
];
const BLOCK_H: [usize; BLOCK_SIZES_ALL] = [
    4, 8, 4, 8, 16, 8, 16, 32, 16, 32, 64, 32, 64, 128, 64, 128, 16, 4, 32, 8, 64, 16,
];
/// `mi_size_wide` — the block width in 4x4 units.
const MI_SIZE_WIDE: [usize; BLOCK_SIZES_ALL] = [
    1, 1, 2, 2, 2, 4, 4, 4, 8, 8, 8, 16, 16, 16, 32, 32, 1, 4, 2, 8, 4, 16,
];
/// `mi_size_high` — the block height in 4x4 units.
const MI_SIZE_HIGH: [usize; BLOCK_SIZES_ALL] = [
    1, 2, 1, 2, 4, 2, 4, 8, 4, 8, 16, 8, 16, 32, 16, 32, 4, 1, 8, 2, 16, 4,
];

#[inline]
fn round_power_of_two(value: i32, n: i32) -> i32 {
    if n == 0 {
        value
    } else {
        (value + (1 << (n - 1))) >> n
    }
}

/// `AOM_BLEND_AVG(v0, v1)` (definitions.h:1275).
#[inline]
fn aom_blend_avg(v0: i32, v1: i32) -> i32 {
    round_power_of_two(v0 + v1, 1)
}

/// `build_smooth_interintra_mask` (inter_prediction.c:2233).
///
/// The four arms read `ii_weights1d` at `i * size_scale` (V), `j * size_scale`
/// (H), `min(i, j) * size_scale` (SMOOTH) and the constant 32 (DC). Note the
/// SMOOTH arm's `(i < j ? i : j)` is a MIN, not the row or column alone.
pub fn build_smooth_interintra_mask(
    mask: &mut [u8],
    stride: usize,
    plane_bsize: usize,
    mode: InterIntraMode,
) {
    let bw = BLOCK_W[plane_bsize];
    let bh = BLOCK_H[plane_bsize];
    let size_scale = II_SIZE_SCALES[plane_bsize];
    match mode {
        InterIntraMode::VPred => {
            for i in 0..bh {
                let v = II_WEIGHTS_1D[i * size_scale];
                mask[i * stride..i * stride + bw].fill(v);
            }
        }
        InterIntraMode::HPred => {
            for i in 0..bh {
                for j in 0..bw {
                    mask[i * stride + j] = II_WEIGHTS_1D[j * size_scale];
                }
            }
        }
        InterIntraMode::SmoothPred => {
            for i in 0..bh {
                for j in 0..bw {
                    mask[i * stride + j] = II_WEIGHTS_1D[i.min(j) * size_scale];
                }
            }
        }
        InterIntraMode::DcPred => {
            for i in 0..bh {
                mask[i * stride..i * stride + bw].fill(32);
            }
        }
    }
}

/// The smooth inter-intra mask table `init_ii_masks` (inter_prediction.c:2282)
/// builds. C keeps it in file-scope arrays; here it is an owned value.
///
/// Inter-intra is allowed for 8x8..32x32 blocks, but masks are generated down
/// to 4x4 **because of chroma** — dropping the sub-8x8 sizes breaks 4:2:0
/// chroma, which is why `init_ii_masks` starts at `BLOCK_4X4`.
pub struct IiMasks {
    data: Vec<u8>,
}

impl Default for IiMasks {
    fn default() -> Self {
        Self::new()
    }
}

impl IiMasks {
    /// `init_ii_masks` (inter_prediction.c:2282). Each mask's stride is its
    /// block width.
    pub fn new() -> Self {
        let mut data = vec![0u8; (BLOCK_32X32 + 1) * INTERINTRA_MODES * MAX_INTERINTRA_SB_SQUARE];
        for bsize in 0..=BLOCK_32X32 {
            let bw = BLOCK_W[bsize];
            for (m, mode) in InterIntraMode::ALL.iter().enumerate() {
                let off = (bsize * INTERINTRA_MODES + m) * MAX_INTERINTRA_SB_SQUARE;
                build_smooth_interintra_mask(
                    &mut data[off..off + MAX_INTERINTRA_SB_SQUARE],
                    bw,
                    bsize,
                    *mode,
                );
            }
        }
        Self { data }
    }

    /// `get_ii_mask` (inter_prediction.c:2294) — mask stride is the block
    /// width. Returns `None` for a bsize with no mask, which is what C's
    /// zero-initialised `smooth_ii_masks` entry (a NULL pointer) means.
    pub fn get(&self, bsize: usize, mode: InterIntraMode) -> Option<&[u8]> {
        if bsize > BLOCK_32X32 {
            return None;
        }
        let off = (bsize * INTERINTRA_MODES + mode as usize) * MAX_INTERINTRA_SB_SQUARE;
        Some(&self.data[off..off + MAX_INTERINTRA_SB_SQUARE])
    }
}

/// `svt_aom_blend_a64_mask_c` (blend_a64_mask.c:207). See the module note: this
/// lives here only because nothing else in the port supplies it.
#[allow(clippy::too_many_arguments)]
pub fn blend_a64_mask(
    dst: &mut [u8],
    dst_stride: usize,
    src0: &[u8],
    src0_stride: usize,
    src1: &[u8],
    src1_stride: usize,
    mask: &[u8],
    mask_stride: usize,
    w: usize,
    h: usize,
    subw: bool,
    subh: bool,
) {
    for i in 0..h {
        for j in 0..w {
            let m = match (subw, subh) {
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
            };
            dst[i * dst_stride + j] = aom_blend_a64(
                m,
                src0[i * src0_stride + j] as i32,
                src1[i * src1_stride + j] as i32,
            ) as u8;
        }
    }
}

/// `svt_aom_highbd_blend_a64_mask_c` (blend_a64_mask.c:254). `bd` is accepted
/// and unused by C — the `AOM_BLEND_A64` result cannot exceed the inputs'
/// range, so no clip is needed.
#[allow(clippy::too_many_arguments)]
pub fn highbd_blend_a64_mask(
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
) {
    for i in 0..h {
        for j in 0..w {
            let m = match (subw, subh) {
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
            };
            dst[i * dst_stride + j] = aom_blend_a64(
                m,
                src0[i * src0_stride + j] as i32,
                src1[i * src1_stride + j] as i32,
            ) as u16;
        }
    }
}

/// `svt_aom_combine_interintra` (inter_prediction.c:2468).
///
/// TRAP, reproduced: on the wedge arm, when
/// `svt_aom_is_interintra_wedge_used(bsize)` is FALSE the function returns
/// having written NOTHING — `comppred` keeps whatever it held. A port that
/// "helpfully" fell through to the smooth arm would differ on exactly the
/// block sizes wedges are not allowed on.
///
/// Note the argument order into the blend: `src0` is the INTRA predictor and
/// `src1` the INTER one, so the mask weights intra.
#[allow(clippy::too_many_arguments)]
pub fn combine_interintra(
    ii_masks: &IiMasks,
    wedge: &WedgeMasks,
    mode: InterIntraMode,
    use_wedge_interintra: bool,
    wedge_index: usize,
    wedge_sign: usize,
    bsize: usize,
    plane_bsize: usize,
    comppred: &mut [u8],
    compstride: usize,
    interpred: &[u8],
    interstride: usize,
    intrapred: &[u8],
    intrastride: usize,
) {
    let bw = BLOCK_W[plane_bsize];
    let bh = BLOCK_H[plane_bsize];

    if use_wedge_interintra {
        if is_interintra_wedge_used(bsize) {
            let mask = wedge.contiguous_soft_mask(wedge_index, wedge_sign, bsize);
            let subw = 2 * MI_SIZE_WIDE[bsize] == bw;
            let subh = 2 * MI_SIZE_HIGH[bsize] == bh;
            blend_a64_mask(
                comppred,
                compstride,
                intrapred,
                intrastride,
                interpred,
                interstride,
                mask,
                BLOCK_W[bsize],
                bw,
                bh,
                subw,
                subh,
            );
        }
        return;
    }

    let mask = ii_masks
        .get(plane_bsize, mode)
        .expect("get_ii_mask has no entry for this plane_bsize");
    blend_a64_mask(
        comppred,
        compstride,
        intrapred,
        intrastride,
        interpred,
        interstride,
        mask,
        bw,
        bw,
        bh,
        false,
        false,
    );
}

/// `svt_aom_combine_interintra_highbd` (inter_prediction.c:2298).
///
/// Same shape as the 8-bit twin — including the write-nothing wedge arm — but
/// note C computes `subh` BEFORE `subw` here and after it in the 8-bit
/// version. That ordering is cosmetic (both are pure reads) and is the kind of
/// difference that tempts a reader into thinking the two arms differ.
#[allow(clippy::too_many_arguments)]
pub fn combine_interintra_highbd(
    ii_masks: &IiMasks,
    wedge: &WedgeMasks,
    mode: InterIntraMode,
    use_wedge_interintra: bool,
    wedge_index: usize,
    wedge_sign: usize,
    bsize: usize,
    plane_bsize: usize,
    comppred: &mut [u16],
    compstride: usize,
    interpred: &[u16],
    interstride: usize,
    intrapred: &[u16],
    intrastride: usize,
) {
    let bw = BLOCK_W[plane_bsize];
    let bh = BLOCK_H[plane_bsize];

    if use_wedge_interintra {
        if is_interintra_wedge_used(bsize) {
            let mask = wedge.contiguous_soft_mask(wedge_index, wedge_sign, bsize);
            let subh = 2 * MI_SIZE_HIGH[bsize] == bh;
            let subw = 2 * MI_SIZE_WIDE[bsize] == bw;
            highbd_blend_a64_mask(
                comppred,
                compstride,
                intrapred,
                intrastride,
                interpred,
                interstride,
                mask,
                BLOCK_W[bsize],
                bw,
                bh,
                subw,
                subh,
            );
        }
        return;
    }

    let mask = ii_masks
        .get(plane_bsize, mode)
        .expect("get_ii_mask has no entry for this plane_bsize");
    highbd_blend_a64_mask(
        comppred,
        compstride,
        intrapred,
        intrastride,
        interpred,
        interstride,
        mask,
        bw,
        bw,
        bh,
        false,
        false,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::port_masked_compound::AOM_BLEND_A64_ROUND_BITS;

    /// `AOM_BLEND_A64` at the two rails is an exact select.
    #[test]
    fn blend_rails_are_selects() {
        assert_eq!(aom_blend_a64(0, 200, 40), 40);
        assert_eq!(aom_blend_a64(1 << AOM_BLEND_A64_ROUND_BITS, 200, 40), 200);
    }

    /// The DC mask is the constant 32 and the V mask is row-constant; the
    /// SMOOTH mask uses `min(i, j)`, not `i` or `j` alone.
    #[test]
    fn smooth_mask_shapes() {
        let m = IiMasks::new();
        // BLOCK_8X8 = 3, bw = bh = 8, size_scale = 16.
        let dc = m.get(3, InterIntraMode::DcPred).unwrap();
        assert!(dc[..64].iter().all(|&v| v == 32));
        let v = m.get(3, InterIntraMode::VPred).unwrap();
        for i in 0..8 {
            assert!(
                v[i * 8..i * 8 + 8]
                    .iter()
                    .all(|&x| x == II_WEIGHTS_1D[i * 16])
            );
        }
        let s = m.get(3, InterIntraMode::SmoothPred).unwrap();
        for i in 0..8 {
            for j in 0..8 {
                assert_eq!(s[i * 8 + j], II_WEIGHTS_1D[i.min(j) * 16]);
            }
        }
    }
}
