//! The OBMC `wsrc` / `mask` producer.
//!
//! Ported from `Source/Lib/Codec/enc_inter_prediction.c` (SVT-AV1 v4.2.0):
//! `svt_av1_calc_target_weighted_pred_above_c` (:1577),
//! `svt_av1_calc_target_weighted_pred_left_c` (:1605) and
//! `calc_target_weighted_pred` (:1749); plus
//! `svt_av1_skip_u4x4_pred_in_obmc` (inter_prediction.c:2403) and
//! `get_plane_block_size` (common_utils.h:135).
//!
//! # Why this exists
//!
//! `svtav1-encoder/src/inter_me/obmc_search.rs` (the av1me.c OBMC search)
//! takes `wsrc` and `mask` as INPUTS and nothing in the port produced them.
//! These functions are that producer.
//!
//! # `CONFIG_ENABLE_OBMC` is 1, checked not assumed
//!
//! `Source/API/EbConfigMacros.h` defines it twice: 0 at :33 inside
//! `#if RTC_BUILD`, and 1 at :82 in the `#ifndef` default. `RTC_BUILD`
//! defaults to 0 (:25-27), so mainline compiles OBMC and every function here
//! is LIVE. `DISABLE_CHROMA_U8X8_OBMC` is 0 (inter_prediction.h:41), so
//! `svt_av1_skip_u4x4_pred_in_obmc` compiles the one-sided `return dir == 0`
//! arm — the `return 1` arm is DEAD and is not ported.
//!
//! # The arithmetic, and where it is asymmetric
//!
//! The above pass WRITES `wsrc`/`mask` (`wsrc = m1 * tmp`, `mask = m0`); the
//! left pass READS THEM BACK and accumulates
//! (`wsrc = (wsrc >> 6) * m0 + (tmp << 6) * m1`). Between the two,
//! `calc_target_weighted_pred` multiplies both buffers by
//! `AOM_BLEND_A64_MAX_ALPHA`. Running the two passes in the other order, or
//! skipping the scale between them, produces plausible numbers that are wrong
//! everywhere the two neighbours overlap.

use crate::obmc::obmc_mask;
use svtav1_types::block::BlockSize;

/// `AOM_BLEND_A64_MAX_ALPHA` (definitions.h:1269).
pub const AOM_BLEND_A64_MAX_ALPHA: i32 = 64;
/// `AOM_BLEND_A64_ROUND_BITS` (definitions.h:1268).
pub const AOM_BLEND_A64_ROUND_BITS: i32 = 6;
/// `MI_SIZE` — 4 luma samples per mode-info unit.
pub const MI_SIZE: usize = 4;

/// `struct calc_target_weighted_pred_ctxt` (enc_inter_prediction.c), split so
/// the two buffers can be borrowed mutably alongside the read-only neighbour
/// prediction.
pub struct CalcTargetWeightedPredCtxt<'a> {
    /// `mask_buf` — `bw * bh` i32 accumulator.
    pub mask_buf: &'a mut [i32],
    /// `wsrc_buf` — `bw * bh` i32 accumulator.
    pub wsrc_buf: &'a mut [i32],
    /// `tmp` — the neighbour's prediction plane.
    pub tmp: &'a [u8],
    /// `tmp_stride`.
    pub tmp_stride: usize,
    /// `overlap` — the blend depth, in samples.
    pub overlap: usize,
}

/// `svt_av1_calc_target_weighted_pred_above_c` (enc_inter_prediction.c:1577).
///
/// `bw` is `xd->n4_w << MI_SIZE_LOG2`, i.e. the CURRENT block's width — the
/// accumulator stride — not the neighbour's. `nb_mi_width` is the neighbour's
/// width in mode-info units.
pub fn calc_target_weighted_pred_above(
    ctxt: &mut CalcTargetWeightedPredCtxt<'_>,
    bw: usize,
    rel_mi_col: usize,
    nb_mi_width: usize,
) {
    let mask1d = obmc_mask(ctxt.overlap);
    let base = rel_mi_col * MI_SIZE;
    for row in 0..ctxt.overlap {
        let m0 = mask1d[row] as i32;
        let m1 = AOM_BLEND_A64_MAX_ALPHA - m0;
        for col in 0..nb_mi_width * MI_SIZE {
            let i = base + row * bw + col;
            ctxt.wsrc_buf[i] = m1 * ctxt.tmp[base + row * ctxt.tmp_stride + col] as i32;
            ctxt.mask_buf[i] = m0;
        }
    }
}

/// `svt_av1_calc_target_weighted_pred_left_c` (enc_inter_prediction.c:1605).
///
/// Unlike the above pass this one ACCUMULATES: it reads `wsrc`/`mask` back and
/// folds the left neighbour in.
pub fn calc_target_weighted_pred_left(
    ctxt: &mut CalcTargetWeightedPredCtxt<'_>,
    bw: usize,
    rel_mi_row: usize,
    nb_mi_height: usize,
) {
    let mask1d = obmc_mask(ctxt.overlap);
    let base = rel_mi_row * MI_SIZE * bw;
    let tmp_base = rel_mi_row * MI_SIZE * ctxt.tmp_stride;
    for row in 0..nb_mi_height * MI_SIZE {
        for col in 0..ctxt.overlap {
            let m0 = mask1d[col] as i32;
            let m1 = AOM_BLEND_A64_MAX_ALPHA - m0;
            let i = base + row * bw + col;
            let t = ctxt.tmp[tmp_base + row * ctxt.tmp_stride + col] as i32;
            ctxt.wsrc_buf[i] = (ctxt.wsrc_buf[i] >> AOM_BLEND_A64_ROUND_BITS) * m0
                + (t << AOM_BLEND_A64_ROUND_BITS) * m1;
            ctxt.mask_buf[i] = (ctxt.mask_buf[i] >> AOM_BLEND_A64_ROUND_BITS) * m0;
        }
    }
}

/// One overlapping neighbour, as `foreach_overlappable_nb_{above,left}` yields
/// it: its offset from the current block in mode-info units, and its extent.
#[derive(Debug, Clone, Copy)]
pub struct Neighbour {
    /// `rel_mi_col` (above) or `rel_mi_row` (left).
    pub rel_mi: usize,
    /// `nb_mi_width` (above) or `nb_mi_height` (left), in mode-info units.
    pub nb_mi: usize,
}

/// `calc_target_weighted_pred` (enc_inter_prediction.c:1749).
///
/// The neighbour WALK (`foreach_overlappable_nb_above` / `_left`, which read
/// the `MacroBlockD` mi grid) is supplied by the caller as `above` / `left`
/// lists, so this function is the pure arithmetic. C's `up_available` /
/// `left_available` are expressed as an empty list.
///
/// The two `overlap` values are NOT the same: above uses
/// `min(block_size_high[bsize], 64) >> 1` and left
/// `min(block_size_wide[bsize], 64) >> 1`.
#[allow(clippy::too_many_arguments)]
pub fn calc_target_weighted_pred(
    bsize: BlockSize,
    bw: usize,
    bh: usize,
    wsrc_buf: &mut [i32],
    mask_buf: &mut [i32],
    above: &[Neighbour],
    above_plane: &[u8],
    above_stride: usize,
    left: &[Neighbour],
    left_plane: &[u8],
    left_stride: usize,
    src: &[u8],
    src_stride: usize,
) {
    let src_scale = AOM_BLEND_A64_MAX_ALPHA * AOM_BLEND_A64_MAX_ALPHA;

    wsrc_buf[..bw * bh].fill(0);
    mask_buf[..bw * bh].fill(AOM_BLEND_A64_MAX_ALPHA);

    if !above.is_empty() {
        let overlap = (block_size_high(bsize).min(64)) >> 1;
        let mut ctxt = CalcTargetWeightedPredCtxt {
            mask_buf,
            wsrc_buf,
            tmp: above_plane,
            tmp_stride: above_stride,
            overlap,
        };
        for nb in above {
            calc_target_weighted_pred_above(&mut ctxt, bw, nb.rel_mi, nb.nb_mi);
        }
    }

    // The scale between the two passes. Skipping it silently halves the above
    // neighbour's weight wherever the left pass also writes.
    for i in 0..bw * bh {
        wsrc_buf[i] *= AOM_BLEND_A64_MAX_ALPHA;
        mask_buf[i] *= AOM_BLEND_A64_MAX_ALPHA;
    }

    if !left.is_empty() {
        let overlap = (block_size_wide(bsize).min(64)) >> 1;
        let mut ctxt = CalcTargetWeightedPredCtxt {
            mask_buf,
            wsrc_buf,
            tmp: left_plane,
            tmp_stride: left_stride,
            overlap,
        };
        for nb in left {
            calc_target_weighted_pred_left(&mut ctxt, bw, nb.rel_mi, nb.nb_mi);
        }
    }

    for row in 0..bh {
        for col in 0..bw {
            wsrc_buf[row * bw + col] =
                src[row * src_stride + col] as i32 * src_scale - wsrc_buf[row * bw + col];
        }
    }
}

const BLOCK_W: [usize; BlockSize::SIZES_ALL] = [
    4, 4, 8, 8, 8, 16, 16, 16, 32, 32, 32, 64, 64, 64, 128, 128, 4, 16, 8, 32, 16, 64,
];
const BLOCK_H: [usize; BlockSize::SIZES_ALL] = [
    4, 8, 4, 8, 16, 8, 16, 32, 16, 32, 64, 32, 64, 128, 64, 128, 16, 4, 32, 8, 64, 16,
];

/// `block_size_wide[bsize]`.
pub fn block_size_wide(bsize: BlockSize) -> usize {
    BLOCK_W[bsize as usize]
}

/// `block_size_high[bsize]`.
pub fn block_size_high(bsize: BlockSize) -> usize {
    BLOCK_H[bsize as usize]
}

/// `svt_aom_ss_size_lookup[bsize][ss_x][ss_y]` (common_utils.c:239).
/// `None` is C's `BLOCK_INVALID`.
const SS_SIZE_LOOKUP: [[[Option<u8>; 2]; 2]; BlockSize::SIZES_ALL] = [
    [[Some(0), Some(0)], [Some(0), Some(0)]],
    [[Some(1), Some(0)], [None, Some(0)]],
    [[Some(2), None], [Some(0), Some(0)]],
    [[Some(3), Some(2)], [Some(1), Some(0)]],
    [[Some(4), Some(3)], [None, Some(1)]],
    [[Some(5), None], [Some(3), Some(2)]],
    [[Some(6), Some(5)], [Some(4), Some(3)]],
    [[Some(7), Some(6)], [None, Some(4)]],
    [[Some(8), None], [Some(6), Some(5)]],
    [[Some(9), Some(8)], [Some(7), Some(6)]],
    [[Some(10), Some(9)], [None, Some(7)]],
    [[Some(11), None], [Some(9), Some(8)]],
    [[Some(12), Some(11)], [Some(10), Some(9)]],
    [[Some(13), Some(12)], [None, Some(10)]],
    [[Some(14), None], [Some(12), Some(11)]],
    [[Some(15), Some(14)], [Some(13), Some(12)]],
    [[Some(16), Some(1)], [None, Some(1)]],
    [[Some(17), None], [Some(2), Some(2)]],
    [[Some(18), Some(4)], [None, Some(16)]],
    [[Some(19), None], [Some(5), Some(17)]],
    [[Some(20), Some(7)], [None, Some(18)]],
    [[Some(21), None], [Some(8), Some(19)]],
];

/// `get_plane_block_size` (common_utils.h:135). `None` is `BLOCK_INVALID`.
pub fn get_plane_block_size(
    bsize: BlockSize,
    subsampling_x: usize,
    subsampling_y: usize,
) -> Option<BlockSize> {
    SS_SIZE_LOOKUP[bsize as usize][subsampling_x][subsampling_y].and_then(BlockSize::from_u8)
}

/// `svt_av1_skip_u4x4_pred_in_obmc` (inter_prediction.c:2403).
///
/// `DISABLE_CHROMA_U8X8_OBMC` is 0, so the LIVE arm is `return dir == 0` for
/// the three sub-8x8 plane sizes. The `#if` arm (`return 1`) is DEAD and is
/// deliberately not ported — see the module doc.
pub fn skip_u4x4_pred_in_obmc(
    bsize: BlockSize,
    dir: i32,
    subsampling_x: usize,
    subsampling_y: usize,
) -> i32 {
    let plane = get_plane_block_size(bsize, subsampling_x, subsampling_y);
    match plane {
        // BLOCK_4X4 = 0, BLOCK_8X4 = 2, BLOCK_4X8 = 1.
        Some(BlockSize::Block4x4) | Some(BlockSize::Block8x4) | Some(BlockSize::Block4x8) => {
            i32::from(dir == 0)
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// The two passes are asymmetric: above WRITES, left ACCUMULATES. Running
    /// the left pass on a zeroed accumulator must therefore differ from
    /// running it after the above pass.
    #[test]
    fn left_pass_accumulates() {
        let (bw, bh) = (8usize, 8usize);
        let tmp = vec![100u8; bw * bh];
        let mut w1 = vec![0i32; bw * bh];
        let mut m1 = vec![AOM_BLEND_A64_MAX_ALPHA; bw * bh];
        {
            let mut c = CalcTargetWeightedPredCtxt {
                mask_buf: &mut m1,
                wsrc_buf: &mut w1,
                tmp: &tmp,
                tmp_stride: bw,
                overlap: 4,
            };
            calc_target_weighted_pred_left(&mut c, bw, 0, 2);
        }
        let mut w2 = vec![0i32; bw * bh];
        let mut m2 = vec![AOM_BLEND_A64_MAX_ALPHA; bw * bh];
        {
            let mut c = CalcTargetWeightedPredCtxt {
                mask_buf: &mut m2,
                wsrc_buf: &mut w2,
                tmp: &tmp,
                tmp_stride: bw,
                overlap: 4,
            };
            calc_target_weighted_pred_above(&mut c, bw, 0, 2);
            calc_target_weighted_pred_left(&mut c, bw, 0, 2);
        }
        assert_ne!(
            w1, w2,
            "the left pass must read the above pass's output back"
        );
    }

    /// The LIVE arm of skip_u4x4_pred_in_obmc is one-sided.
    #[test]
    fn skip_u4x4_is_one_sided() {
        // 8x8 luma at 4:2:0 -> 4x4 chroma: skipped for dir 0 only.
        assert_eq!(skip_u4x4_pred_in_obmc(BlockSize::Block8x8, 0, 1, 1), 1);
        assert_eq!(skip_u4x4_pred_in_obmc(BlockSize::Block8x8, 1, 1, 1), 0);
        // 16x16 luma at 4:2:0 -> 8x8 chroma: never skipped.
        assert_eq!(skip_u4x4_pred_in_obmc(BlockSize::Block16x16, 0, 1, 1), 0);
        assert_eq!(skip_u4x4_pred_in_obmc(BlockSize::Block16x16, 1, 1, 1), 0);
    }
}
