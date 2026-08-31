//! The masked-compound search: which wedge, which sign, which DIFFWTD mask.
//!
//! Ported from `Source/Lib/Codec/enc_inter_prediction.c` (SVT-AV1 v4.2.0):
//! `pick_wedge` (:424), `pick_wedge_fixed_sign` (:489),
//! `pick_interinter_wedge` (:523), `pick_interinter_seg` (:538) and
//! `pick_interinter_mask` (:583).
//!
//! Everything these choose — `compound_type`, `wedge_index`, `wedge_sign`,
//! `mask_type` — is a coded syntax element, so a wrong choice desyncs the
//! bitstream even when every kernel below is bit-exact.
//!
//! # Evidence, stated per function
//!
//! `pick_wedge_fixed_sign` is exported and IS gated at tier 1, on the
//! `use_rd_model = 0` arm (see `c_parity_port_wedge_search.rs`). The other
//! four are `static` and reach the encoder's `PictureControlSet` /
//! `ModeDecisionContext` for a lambda, a rate-model switch and a rate table,
//! which a shim cannot synthesise without building most of the encoder. They
//! carry TIER 4 evidence for their control flow.
//!
//! That control flow is thin, though: every quantity it compares comes from a
//! primitive that IS tier-1 gated elsewhere in this port —
//! `svt_aom_subtract_block` / `_highbd_`, `svt_aom_sum_squares_i16`,
//! `svt_av1_wedge_compute_delta_squares`, `svt_av1_wedge_sign_from_residuals`,
//! `svt_av1_wedge_sse_from_residuals`, `svt_aom_get_contiguous_soft_mask`,
//! `svt_av1_build_compound_diffwtd_mask` / `_highbd_`, and
//! `model_rd_with_curvfit` + `RDCOST`. What is tier 4 here is the loop and the
//! tie-break, not the arithmetic.
//!
//! # The tie-break is strict `<`
//!
//! Every loop keeps the FIRST index that beats the running best (`rd < best_rd`
//! with `best_rd` starting at `INT64_MAX`). Relaxing it to `<=` picks the LAST
//! of any tie — a different, still-plausible syntax element.

use crate::port_masked_compound::{
    DiffwtdMaskType, build_compound_diffwtd_mask, build_compound_diffwtd_mask_highbd,
    highbd_subtract_block, subtract_block, sum_squares_i16, wedge_compute_delta_squares,
    wedge_sign_from_residuals, wedge_sse_from_residuals,
};
use crate::port_model_rd::{NUM_PELS_LOG2_LOOKUP, model_rd_with_curvfit, rdcost};
use crate::port_wedge_masks::{WedgeMasks, get_wedge_bits_lookup};
use alloc::vec;
use svtav1_types::block::BlockSize;

/// `WEDGE_WEIGHT_BITS`.
const WEDGE_WEIGHT_BITS: u32 = 6;

const BLOCK_W: [usize; BlockSize::SIZES_ALL] = [
    4, 4, 8, 8, 8, 16, 16, 16, 32, 32, 32, 64, 64, 64, 128, 128, 4, 16, 8, 32, 16, 64,
];
const BLOCK_H: [usize; BlockSize::SIZES_ALL] = [
    4, 8, 4, 8, 16, 8, 16, 32, 16, 32, 64, 32, 64, 128, 64, 128, 16, 4, 32, 8, 64, 16,
];

/// The encoder state the searches read, gathered so the functions stay pure.
#[derive(Debug, Clone, Copy)]
pub struct SearchCtx {
    /// `ctx->hbd_md != EB_8_BIT_MD` — selects the 10-bit source/subtract path.
    pub hbd: bool,
    /// `full_lambda` (`ctx->full_lambda_md[...]`).
    pub full_lambda: u32,
    /// `ctx->inter_comp_ctrls.use_rate` (wedge and seg pickers), or
    /// `ctx->inter_intra_comp_ctrls.use_rd_model` (`pick_wedge_fixed_sign`).
    pub use_rate: bool,
    /// `dequants->y_dequant_qtx[base_q_idx][1]`, which `model_rd_with_curvfit`
    /// reaches through the PCS.
    pub quantizer: i16,
}

/// `pick_wedge_fixed_sign` (enc_inter_prediction.c:489) — the reduced loop at
/// one sign, and the arm the fast presets take.
///
/// Returns `(best_rd, best_wedge_index)`. `wedge_idx_fac_bits` is
/// `ctx->md_rate_est_ctx->wedge_idx_fac_bits[bsize]`, added to the rate only
/// on the rate-model arm.
pub fn pick_wedge_fixed_sign(
    wedge: &WedgeMasks,
    ctx: &SearchCtx,
    bsize: BlockSize,
    residual1: &[i16],
    diff10: &[i16],
    wedge_sign: usize,
    wedge_idx_fac_bits: &[i32],
) -> (i64, i32) {
    let bw = BLOCK_W[bsize as usize];
    let bh = BLOCK_H[bsize as usize];
    let n = bw * bh;
    debug_assert!(n >= 64);
    let wedge_types = 1usize << get_wedge_bits_lookup(bsize as usize);
    // `bd_round` is 0 in C, so ROUND_POWER_OF_TWO(sse, 0) is the identity.
    let mut best_rd = i64::MAX;
    let mut best_wedge_index = -1i32;
    for wedge_index in 0..wedge_types {
        let mask = wedge.contiguous_soft_mask(wedge_index, wedge_sign, bsize as usize);
        let sse = wedge_sse_from_residuals(residual1, diff10, mask, n);
        let mut rd = sse as i64;
        if ctx.use_rate {
            let (mut rate, dist) =
                model_rd_with_curvfit(bsize, sse as i64, n as i32, ctx.quantizer, ctx.full_lambda);
            rate += wedge_idx_fac_bits[wedge_index];
            rd = rdcost(ctx.full_lambda, rate as i64, dist);
        }
        if rd < best_rd {
            best_wedge_index = wedge_index as i32;
            best_rd = rd;
        }
    }
    (best_rd, best_wedge_index)
}

/// `pick_wedge` (enc_inter_prediction.c:424) — the full loop over both signs,
/// via `svt_av1_wedge_sign_from_residuals`.
///
/// Returns `(wedge_sign, wedge_index)`. `src` is the source plane at the
/// block's origin (8-bit) and `src_hbd` the 10-bit one; exactly one is used,
/// per `ctx.hbd`.
///
/// TRAP: `ds` ALIASES `residual0` — C writes
/// `svt_av1_wedge_compute_delta_squares(ds, residual0, residual1, N)` with
/// `ds == residual0`, so the deltas overwrite the residual in place, and
/// `sign_limit` must be computed BEFORE that call. Reordering the two silently
/// changes every sign decision.
#[allow(clippy::too_many_arguments)]
pub fn pick_wedge(
    wedge: &WedgeMasks,
    ctx: &SearchCtx,
    bsize: BlockSize,
    src: &[u8],
    src_hbd: &[u16],
    src_stride: usize,
    p0: &[u8],
    p0_hbd: &[u16],
    residual1: &[i16],
    diff10: &[i16],
) -> (usize, i32) {
    let bw = BLOCK_W[bsize as usize];
    let bh = BLOCK_H[bsize as usize];
    let n = bw * bh;
    debug_assert!(n >= 64);
    let wedge_types = 1usize << get_wedge_bits_lookup(bsize as usize);

    let mut residual0 = vec![0i16; n];
    if ctx.hbd {
        highbd_subtract_block(bh, bw, &mut residual0, bw, src_hbd, src_stride, p0_hbd, bw);
    } else {
        subtract_block(bh, bw, &mut residual0, bw, src, src_stride, p0, bw);
    }

    // Computed BEFORE the in-place delta-squares overwrite.
    let sign_limit = ((sum_squares_i16(&residual0, n) as i64)
        - (sum_squares_i16(residual1, n) as i64))
        * (1i64 << WEDGE_WEIGHT_BITS)
        / 2;

    let ds_src = residual0.clone();
    wedge_compute_delta_squares(&mut residual0, &ds_src, residual1, n);
    let ds = residual0;

    let mut best_rd = i64::MAX;
    let mut best_wedge_index = -1i32;
    let mut best_wedge_sign = 0usize;
    for wedge_index in 0..wedge_types {
        let mask0 = wedge.contiguous_soft_mask(wedge_index, 0, bsize as usize);
        let wedge_sign = usize::from(wedge_sign_from_residuals(&ds, mask0, n, sign_limit));
        let mask = wedge.contiguous_soft_mask(wedge_index, wedge_sign, bsize as usize);
        let sse = wedge_sse_from_residuals(residual1, diff10, mask, n);
        let mut rd = sse as i64;
        if ctx.use_rate {
            let (rate, dist) =
                model_rd_with_curvfit(bsize, sse as i64, n as i32, ctx.quantizer, ctx.full_lambda);
            rd = rdcost(ctx.full_lambda, rate as i64, dist);
        }
        if rd < best_rd {
            best_wedge_index = wedge_index as i32;
            best_wedge_sign = wedge_sign;
            best_rd = rd;
        }
    }
    (best_wedge_sign, best_wedge_index)
}

/// `pick_interinter_seg` (enc_inter_prediction.c:538) — chooses the DIFFWTD
/// `mask_type`.
///
/// `N` here is `1 << eb_num_pels_log2_lookup[bsize]`, where `pick_wedge` in
/// the same file uses `bw * bh`. MEASURED, correcting what this comment first
/// claimed: those two are EQUAL for all 22 block sizes, including the 4:1
/// shapes — `eb_num_pels_log2_lookup` is the exact pel count, not a rounded
/// one. The difference is cosmetic, and
/// `seg_pel_count_equals_bw_times_bh` pins that so nobody "fixes" one to
/// match the other on the assumption they diverge. The table form is kept
/// because it is what C writes.
#[allow(clippy::too_many_arguments)]
pub fn pick_interinter_seg(
    ctx: &SearchCtx,
    bsize: BlockSize,
    p0: &[u8],
    p1: &[u8],
    p0_hbd: &[u16],
    p1_hbd: &[u16],
    residual1: &[i16],
    diff10: &[i16],
) -> DiffwtdMaskType {
    let bw = BLOCK_W[bsize as usize];
    let bh = BLOCK_H[bsize as usize];
    let n = 1usize << NUM_PELS_LOG2_LOOKUP[bsize as usize];
    let mut best_rd = i64::MAX;
    let mut best_mask_type = DiffwtdMaskType::D38;

    for cur in [DiffwtdMaskType::D38, DiffwtdMaskType::D38Inv] {
        let mut temp_mask = vec![0u8; bw * bh];
        if ctx.hbd {
            build_compound_diffwtd_mask_highbd(
                &mut temp_mask,
                cur,
                p0_hbd,
                bw,
                p1_hbd,
                bw,
                bh,
                bw,
                10,
            );
        } else {
            build_compound_diffwtd_mask(&mut temp_mask, cur, p0, bw, p1, bw, bh, bw);
        }
        let sse = wedge_sse_from_residuals(residual1, diff10, &temp_mask, n);
        let rd0 = if ctx.use_rate {
            let (rate, dist) =
                model_rd_with_curvfit(bsize, sse as i64, n as i32, ctx.quantizer, ctx.full_lambda);
            rdcost(ctx.full_lambda, rate as i64, dist)
        } else {
            sse as i64
        };
        if rd0 < best_rd {
            best_mask_type = cur;
            best_rd = rd0;
        }
    }
    best_mask_type
}

/// What `pick_interinter_mask` writes back into the `InterInterCompoundData`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickedMask {
    /// `pick_interinter_wedge` -> `(wedge_sign, wedge_index)`.
    Wedge {
        /// `interinter_comp->wedge_sign`.
        sign: usize,
        /// `interinter_comp->wedge_index`.
        index: i32,
    },
    /// `pick_interinter_seg` -> `mask_type`.
    Seg(DiffwtdMaskType),
}

/// `pick_interinter_wedge` (enc_inter_prediction.c:523) — a thin wrapper that
/// writes `pick_wedge`'s two outputs into the compound data.
#[allow(clippy::too_many_arguments)]
pub fn pick_interinter_wedge(
    wedge: &WedgeMasks,
    ctx: &SearchCtx,
    bsize: BlockSize,
    src: &[u8],
    src_hbd: &[u16],
    src_stride: usize,
    p0: &[u8],
    p0_hbd: &[u16],
    residual1: &[i16],
    diff10: &[i16],
) -> PickedMask {
    let (sign, index) = pick_wedge(
        wedge, ctx, bsize, src, src_hbd, src_stride, p0, p0_hbd, residual1, diff10,
    );
    PickedMask::Wedge { sign, index }
}

/// `pick_interinter_mask` (enc_inter_prediction.c:583) — dispatches on the
/// compound type. C `assert(0)`s on anything but WEDGE or DIFFWTD; this
/// returns `None` rather than guessing.
#[allow(clippy::too_many_arguments)]
pub fn pick_interinter_mask(
    wedge: &WedgeMasks,
    ctx: &SearchCtx,
    compound_type: crate::port_masked_compound::CompoundType,
    bsize: BlockSize,
    src: &[u8],
    src_hbd: &[u16],
    src_stride: usize,
    p0: &[u8],
    p1: &[u8],
    p0_hbd: &[u16],
    p1_hbd: &[u16],
    residual1: &[i16],
    diff10: &[i16],
) -> Option<PickedMask> {
    use crate::port_masked_compound::CompoundType;
    match compound_type {
        CompoundType::Wedge => Some(pick_interinter_wedge(
            wedge, ctx, bsize, src, src_hbd, src_stride, p0, p0_hbd, residual1, diff10,
        )),
        CompoundType::DiffWtd => Some(PickedMask::Seg(pick_interinter_seg(
            ctx, bsize, p0, p1, p0_hbd, p1_hbd, residual1, diff10,
        ))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::port_masked_compound::CompoundType;

    /// `pick_interinter_seg` uses `1 << eb_num_pels_log2_lookup[bsize]` while
    /// `pick_wedge` uses `bw * bh`. MEASURED: they are equal for all 22 block
    /// sizes, so the difference is cosmetic — this cell exists so that stays
    /// true if either table moves.
    #[test]
    fn seg_pel_count_equals_bw_times_bh() {
        for b in 0..BlockSize::SIZES_ALL {
            assert_eq!(
                1usize << NUM_PELS_LOG2_LOOKUP[b],
                BLOCK_W[b] * BLOCK_H[b],
                "pel count disagrees at block size {b}"
            );
        }
    }

    /// A non-masked compound type is refused, not guessed at.
    #[test]
    fn non_masked_compound_is_refused() {
        let wedge = WedgeMasks::new();
        let ctx = SearchCtx {
            hbd: false,
            full_lambda: 100,
            use_rate: false,
            quantizer: 64,
        };
        let z8 = alloc::vec![0u8; 64 * 64];
        let z16 = alloc::vec![0u16; 64 * 64];
        let zi = alloc::vec![0i16; 64 * 64];
        assert!(
            pick_interinter_mask(
                &wedge,
                &ctx,
                CompoundType::Average,
                BlockSize::Block8x8,
                &z8,
                &z16,
                8,
                &z8,
                &z8,
                &z16,
                &z16,
                &zi,
                &zi,
            )
            .is_none()
        );
    }
}
