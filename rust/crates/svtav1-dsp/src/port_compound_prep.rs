//! Preparing the two unmasked predictors the masked-compound search ranks.
//!
//! Ported from `Source/Lib/Codec/enc_inter_prediction.c` (SVT-AV1 v4.2.0):
//! `svt_aom_calc_pred_masked_compound` (:3535) and
//! `svt_aom_search_compound_diff_wedge` (:3705).
//!
//! `mode_decision.c:1064` calls the first to build `pred0` / `pred1`,
//! `residual1` and `diff10`, then the second to turn them into
//! `compound_type` / `wedge_index` / `wedge_sign` / `mask_type` — all
//! bitstream-visible.
//!
//! # Evidence
//!
//! TIER 4. Both are exported, but their arguments are a `PictureControlSet`, a
//! `ModeDecisionContext` (with its four-entry compound predictor CACHE) and a
//! `ModeDecisionCandidate`, and the body calls `svt_aom_inter_prediction` on
//! the encoder's scratch buffers. What is ported is the CACHE policy, the
//! early-exit test and the two residual derivations; the MC is
//! [`crate::port_inter_predictor`] (tier 1) and the two `subtract_block`s are
//! tier 1 (`c_parity_port_masked_compound.rs`).

use crate::port_masked_compound::{highbd_subtract_block, subtract_block};

/// `ctx->cmp_store` — the per-list cache of already-built unipred predictors,
/// keyed by MV. C fixes the capacity at 4 and `svt_aom_assert_err`s on
/// overflow.
pub const CMP_STORE_CAPACITY: usize = 4;

/// The result of a cache lookup: which slot to use, and whether it already
/// holds the prediction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CmpStoreSlot {
    /// Index into `pred{0,1}_buf`.
    pub index: usize,
    /// True when the MV was already present, so the MC can be skipped.
    pub found: bool,
}

/// The MV-keyed lookup C does over `cmp_store.pred{0,1}_mv[0..cnt]`.
///
/// TRAP: the key is the packed `Mv::as_int` — BOTH components at once — so two
/// MVs that share a component do NOT collide. On a miss the slot is appended
/// and `cnt` incremented; C asserts `cnt < 4` BEFORE the append, so a fifth
/// distinct MV is a hard error rather than an eviction. This port returns
/// `None` there instead of overflowing.
pub fn cmp_store_lookup(mvs: &mut alloc::vec::Vec<u32>, mv: u32) -> Option<CmpStoreSlot> {
    if let Some(i) = mvs.iter().position(|&m| m == mv) {
        return Some(CmpStoreSlot {
            index: i,
            found: true,
        });
    }
    if mvs.len() >= CMP_STORE_CAPACITY {
        return None;
    }
    mvs.push(mv);
    Some(CmpStoreSlot {
        index: mvs.len() - 1,
        found: false,
    })
}

/// How C rewrites the candidate before each unipred MC pass
/// (:3573 for list 0, :3624 for list 1).
///
/// The two are NOT symmetric: the list-1 pass ALSO copies `mv[1]` into `mv[0]`
/// and `ref_frame[1]` into `ref_frame[0]` before clearing `ref_frame[1]`,
/// because `svt_aom_inter_prediction` always predicts from list 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnipredOverride {
    /// `block_mi.mv[0]` for the pass.
    pub mv0: u32,
    /// `block_mi.ref_frame[0]`.
    pub ref0: i8,
    /// `block_mi.ref_frame[1]`, always NONE_FRAME here.
    pub ref1: i8,
    /// `block_mi.mode`: GLOBALMV when the candidate was GLOBAL_GLOBALMV, else
    /// NEWMV.
    pub is_global: bool,
    /// `block_mi.is_interintra_used`, forced to 0.
    pub is_interintra_used: bool,
    /// `block_mi.interp_filters`, forced to 0.
    pub interp_filters: u32,
}

/// `NONE_FRAME`.
pub const NONE_FRAME: i8 = -1;

/// The list-0 override (:3573).
pub fn unipred_override_l0(mv0: u32, ref0: i8, was_global_global: bool) -> UnipredOverride {
    UnipredOverride {
        mv0,
        ref0,
        ref1: NONE_FRAME,
        is_global: was_global_global,
        is_interintra_used: false,
        interp_filters: 0,
    }
}

/// The list-1 override (:3624) — note `mv[1]` and `ref_frame[1]` are MOVED
/// into slot 0.
pub fn unipred_override_l1(mv1: u32, ref1: i8, was_global_global: bool) -> UnipredOverride {
    UnipredOverride {
        mv0: mv1,
        ref0: ref1,
        ref1: NONE_FRAME,
        is_global: was_global_global,
        is_interintra_used: false,
        interp_filters: 0,
    }
}

/// The early exit at :3665.
///
/// When the two predictors are too similar there is nothing for a mask to
/// separate, so the whole masked-compound search is skipped. The threshold is
/// `bheight * bwidth * pred0_to_pred1_mult` — a per-pixel SAD budget, so the
/// multiplier is compared against the MEAN absolute difference, not a total.
pub fn exit_compound_prep(
    pred0_to_pred1_dist: u32,
    bwidth: usize,
    bheight: usize,
    mult: u32,
) -> bool {
    pred0_to_pred1_dist < (bheight as u32 * bwidth as u32 * mult)
}

/// The two residual derivations at :3675 / :3696, 8-bit.
///
/// TRAP: `residual1` is `src - pred1` and `diff10` is `pred1 - pred0` — the
/// SECOND predictor is the reference for both, and `diff10`'s operand order is
/// (pred1, pred0) despite the name reading "1 minus 0" only if you read it
/// that way. Swapping either pair flips the sign of every wedge decision.
#[allow(clippy::too_many_arguments)]
pub fn compound_residuals(
    residual1: &mut [i16],
    diff10: &mut [i16],
    src: &[u8],
    src_stride: usize,
    pred0: &[u8],
    pred1: &[u8],
    bwidth: usize,
    bheight: usize,
) {
    subtract_block(
        bheight, bwidth, residual1, bwidth, src, src_stride, pred1, bwidth,
    );
    subtract_block(
        bheight, bwidth, diff10, bwidth, pred1, bwidth, pred0, bwidth,
    );
}

/// The 10-bit twin of [`compound_residuals`] (:3675).
#[allow(clippy::too_many_arguments)]
pub fn compound_residuals_hbd(
    residual1: &mut [i16],
    diff10: &mut [i16],
    src: &[u16],
    src_stride: usize,
    pred0: &[u16],
    pred1: &[u16],
    bwidth: usize,
    bheight: usize,
) {
    highbd_subtract_block(
        bheight, bwidth, residual1, bwidth, src, src_stride, pred1, bwidth,
    );
    highbd_subtract_block(
        bheight, bwidth, diff10, bwidth, pred1, bwidth, pred0, bwidth,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// The cache is keyed on the PACKED MV, holds four entries, and refuses a
    /// fifth rather than evicting.
    #[test]
    fn cmp_store_is_a_four_entry_mv_cache() {
        let mut mvs = vec![];
        assert_eq!(
            cmp_store_lookup(&mut mvs, 0x0001_0002),
            Some(CmpStoreSlot {
                index: 0,
                found: false
            })
        );
        assert_eq!(
            cmp_store_lookup(&mut mvs, 0x0001_0002),
            Some(CmpStoreSlot {
                index: 0,
                found: true
            })
        );
        // Sharing one component is NOT a hit.
        assert_eq!(
            cmp_store_lookup(&mut mvs, 0x0001_0003),
            Some(CmpStoreSlot {
                index: 1,
                found: false
            })
        );
        cmp_store_lookup(&mut mvs, 3).unwrap();
        cmp_store_lookup(&mut mvs, 4).unwrap();
        assert_eq!(mvs.len(), 4);
        assert_eq!(
            cmp_store_lookup(&mut mvs, 5),
            None,
            "the fifth MV must be refused"
        );
        // An existing MV still hits when the store is full.
        assert_eq!(
            cmp_store_lookup(&mut mvs, 4),
            Some(CmpStoreSlot {
                index: 3,
                found: true
            })
        );
    }

    /// The list-1 override MOVES mv[1]/ref_frame[1] into slot 0.
    #[test]
    fn list1_override_moves_the_second_reference_into_slot_zero() {
        let l0 = unipred_override_l0(0x1111, 1, false);
        assert_eq!((l0.mv0, l0.ref0, l0.ref1), (0x1111, 1, NONE_FRAME));
        let l1 = unipred_override_l1(0x2222, 5, true);
        assert_eq!((l1.mv0, l1.ref0, l1.ref1), (0x2222, 5, NONE_FRAME));
        assert!(l1.is_global);
        assert!(!l1.is_interintra_used);
        assert_eq!(l1.interp_filters, 0);
    }

    /// The early-exit threshold scales with the block AREA, so the multiplier
    /// is a per-pixel SAD budget.
    #[test]
    fn early_exit_threshold_is_per_pixel() {
        // 8x8 with mult 2: exits below 128.
        assert!(exit_compound_prep(127, 8, 8, 2));
        assert!(!exit_compound_prep(128, 8, 8, 2));
        // 16x16 with the same mult: the same MEAN difference does not exit.
        assert!(!exit_compound_prep(128 * 4, 16, 16, 2));
        // mult 0 never exits.
        assert!(!exit_compound_prep(0, 8, 8, 0));
    }

    /// `residual1 = src - pred1` and `diff10 = pred1 - pred0`.
    #[test]
    fn residual_operand_order() {
        let src = vec![200u8; 16];
        let p0 = vec![50u8; 16];
        let p1 = vec![120u8; 16];
        let mut r1 = vec![0i16; 16];
        let mut d10 = vec![0i16; 16];
        compound_residuals(&mut r1, &mut d10, &src, 4, &p0, &p1, 4, 4);
        assert!(r1.iter().all(|&v| v == 80), "residual1 must be src - pred1");
        assert!(d10.iter().all(|&v| v == 70), "diff10 must be pred1 - pred0");
    }
}
