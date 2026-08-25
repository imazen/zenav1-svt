//! C-exact leaf intra-mode decision funnel (allintra presets 4..=10,
//! still/PD1 fixed-tree path).
//!
//! Per-preset configuration lives in [`FunnelCfg::for_preset`]; the M5
//! extension (mode_end PAETH, angular deltas {-3,0,+3}, SH-gated edge-
//! filtered directional prediction, independent-uv at MDS3, txt 6/6
//! satd 15 rate 250) and the M4 extension (intra_level 1: ALL 7 angle
//! deltas, unfiltered prediction — SH bit 0; nic case 5: rank factors
//! 0, mds2 base 20, rel-dev off) are documented against their C cites
//! there and in docs/IDENTITY-STATUS.md 2026-07-14. The staging
//! skeleton below is the M6 baseline the other presets specialize:
//!
//! Ports the REGULAR-PD1 `md_encode_block` staging for the allintra M6
//! configuration, verified against instrumented-library captures
//! (docs/captures/gradient_*_p6.m6fnl.txt; every constant below carries
//! its C cite):
//!
//! - Candidates (`generate_md_stage_0_cand`, mode_decision.c:3621):
//!   intra_level 6 (enc_mode_config.c:6907 M6 row; set_intra_ctrls case 6
//!   :8574) => mode_end SMOOTH, angular_pred_level 4 (D45.. masked, no
//!   angle deltas), no prune flags — injection order DC, V, H, SMOOTH —
//!   plus FILTER_DC_PRED for blocks <= 32x32 (filter_intra level 2,
//!   :8045; svt_aom_filter_intra_allowed_bsize mode_decision.c:102).
//!   `is_dc_only_safe` is dead at M6 (prune_using_edge_info == 0).
//! - MDS0 (`fast_loop_core`, product_coding_loop.c:1258): whole-block
//!   luma prediction, Hadamard SATD (`hadamard_path` :1187 — 32x32-capped
//!   tiles, `mds0_use_hadamard_sb = true` for allintra PD1,
//!   enc_mode_config.c:11408), fast cost = RDCOST(lambda, flr + fcr,
//!   satd << 4) with `svt_aom_intra_fast_cost` rates (rd_cost.c:526).
//! - NIC (nic_level 6, svt_aom_get_nic_level_allintra:5999):
//!   scaling level 6 => stage nums 6/6/6 over I-slice class-0 base 64
//!   (MD_STAGE_NICS), qp-scaled (svt_aom_set_nics,
//!   product_coding_loop.c:1347); pruning ths mds1 1200/rank 3,
//!   mds2 15/rank 1/dev 5, mds3 15 (set_nic_controls case 6:6209),
//!   qp-scaled via svt_aom_get_qp_based_th_scaling_factors.
//! - MDS1 (`md_stage_1` :7269, staging mode 1): luma-only full loop at
//!   tx_depth 0, DCT_DCT, `quantize_b` (mds_do_rdoq = false —
//!   svt_aom_quantize_inv_quantize full_loop.c:1754), FREQ-domain SSE
//!   (spatial level 3 = SSSE_MDS3 only), real txb/dc-sign contexts
//!   (rate_est_level 1 => update_skip_ctx_dc_sign_ctx = 1), full cost =
//!   svt_aom_full_cost with zero chroma terms.
//! - MDS3 (`md_stage_3` :7397): TXS depths 0..1 (txs_level 3 intra sq
//!   max depth 1, prev_depth_coeff_exit 1), per-txb TXT search
//!   (`tx_type_search` :4660 — groups 4 (>=16x16) / 5 (<16x16) intra,
//!   SATD early-exit th 10 qp-scaled, rate th 100, depth-1 group offset
//!   3), RDOQ per the frame policy with REAL contexts, spatial SSE << 4,
//!   CHROMA full loop (CHROMA_MODE_1: uv follows luma;
//!   `svt_aom_full_loop_uv` full_loop.c:2161) with the
//!   chroma-complexity detector (:6095) gating CFL (cfl level 4,
//!   cplx_th 10 — CFL is only *evaluated* when the detector fires;
//!   flat-chroma content never fires it; if it fires we currently keep
//!   the non-CFL uv mode, documented as a residual gap), full cost =
//!   `svt_aom_full_cost` (rd_cost.c:1357).
//! - Winner: lowest full cost, first-in-order ties
//!   (`svt_aom_product_full_mode_decision`, mode_decision.c:3869).

use alloc::vec;
use alloc::vec::Vec;

use svtav1_entropy::coeff_c as cc;
use svtav1_entropy::context::FrameContext;

use crate::quant::{CoeffCostTables, QuantTable};

/// FILTER_INTRA_MODES = "no filter intra" sentinel (C definitions.h:1339).
pub const FI_NONE: u8 = 5;

// --------------------------------------------------------------------------
// Module layout (split 2026-08-16, extended 2026-08-25)
// --------------------------------------------------------------------------
//
// This file was 11,247 lines; the port map had wanted it split since July.
// PURE CODE MOVEMENT -- the glob re-exports keep every existing
// `crate::leaf_funnel::X` path resolving unchanged, so byte-identity of the
// encoder is the acceptance test rather than a reading of the diff.
//
// The 2026-08-25 round moved out everything that is NOT the funnel walk: the
// data model (`types`), the CfL machinery (`cfl`), the commit step (`commit`),
// tx geometry (`tx_geom`), the depth > 0 predictor (`overlay`), the tx-type
// search (`txt`), the chroma detector (`detect`), and the unit tests
// (`tests`). What is left here is `evaluate_leaf` and the two `decide_leaf`
// entry points it serves -- i.e. the walk itself.

use crate::quant::TX_SCALE_TAB;

mod cfl;
mod chroma;
mod coeff_rate;
mod commit;
mod detect;
mod inject;
mod nic;
mod overlay;
mod predict;
mod rate_tables;
mod tx_geom;
mod tx_pipeline;
mod txt;
mod types;

#[cfg(test)]
mod tests;
// A glob re-export CAPS the visibility of what it re-exports. `pub(crate) use`
// therefore silently demoted the genuinely-`pub` `build_md_rates` and broke the
// integration tests that reach it from outside the crate; a blanket `pub use`
// then warned on the three modules that export nothing public. So: crate-scoped
// globs for the internals, and one explicit `pub` re-export for the item that
// really is part of the crate's surface.
pub(crate) use cfl::*;
pub(crate) use coeff_rate::*;
pub(crate) use commit::*;
// `detect`, `overlay` and `txt` export nothing above `pub(super)` -- nothing
// outside `leaf_funnel` calls them -- so these are plain imports. A
// `pub(crate)` glob over them would warn (it re-exports nothing), which is the
// same visibility trap as `build_md_rates` above, seen from the other side.
use detect::*;
use overlay::*;
pub(crate) use predict::*;
pub use rate_tables::build_md_rates;
pub(crate) use rate_tables::*;
pub(crate) use tx_geom::*;
pub(crate) use tx_pipeline::*;
use txt::*;
// Same rule as `build_md_rates` above: these two are genuinely `pub` (the AVIF
// surface and the pipeline's IBC frame state reach them from outside the
// crate), so they need an explicit re-export the glob cannot cap.
pub(crate) use types::*;
pub use types::{IbcFrameState, LeafChoice};

// ---------------------------------------------------------------------------
// The funnel
// ---------------------------------------------------------------------------

/// C `intra_luma_to_chroma` (mode_decision.c:42) — identity mapping.
#[inline]
fn uv_from_y(mode: u8) -> u8 {
    mode
}

/// C `fimode_to_intradir` (common_utils.c:33).
pub(crate) const FIMODE_TO_INTRADIR: [u8; 5] = [0, 1, 2, 6, 0];
/// C `fimode_to_intramode` (definitions.h:1301) — differs from INTRADIR in the
/// last entry: FILTER_PAETH maps to PAETH (12), not DC. C uses THIS table for
/// the injection-time uv/uv_delta assignment; the tx/ext-tx rate paths use
/// INTRADIR (common_utils.c:33 via rd_cost.c:135).
pub(crate) const FIMODE_TO_INTRAMODE: [u8; 5] = [0, 1, 2, 6, 12];

/// The partition value the fixed-tree decide paths stamp at commit: the
/// caller-set per-leaf gate partition (PART_N default).
fn fx_partition_for_commit(fx: &FunnelCtx<'_>) -> u8 {
    fx.ibc_gate.partition
}

/// Decide one PART_N leaf of the fixed tree — the full MDS0/MDS1/MDS3
/// funnel — and commit the winner (luma recon into `y_recon`, chroma into
/// the funnel's decision planes, all neighbor context updates).
#[allow(clippy::too_many_arguments)]
pub(crate) fn decide_leaf(
    fx: &mut FunnelCtx<'_>,
    y_src: &[u8],
    y_src_stride: usize,
    y_src_off: usize,
    y_recon: &mut [u8],
    y_stride: usize,
    abs_x: usize,
    abs_y: usize,
    size: usize,
    dc_only: bool,
    // eff-M9 per-SB TXS gate: the SB stayed at PD0_LVL_6 (undemoted). Only
    // consulted when the config's `txs_lvl6_gate` is set (eff-M9); ignored
    // at M0..M8 where TXS is uniform.
    sb_is_lvl6: bool,
) -> LeafChoice {
    decide_leaf_rect(
        fx,
        y_src,
        y_src_stride,
        y_src_off,
        y_recon,
        y_stride,
        abs_x,
        abs_y,
        size,
        size,
        dc_only,
        sb_is_lvl6,
    )
}

/// Non-square variant of [`decide_leaf`] — evaluate + commit a `w x h` block
/// (`evaluate_leaf`/`commit_leaf` are already dimension-general, exercised by
/// the M4/M5 NSQ depth-refine walk). Used by the partial-SB partition edge
/// coding (task #95 chunk 2): an incomplete node coded as PARTITION_HORZ /
/// PARTITION_VERT codes its single in-frame `size x (size/2)` (or
/// `(size/2) x size`) block through this path.
#[allow(clippy::too_many_arguments)]
pub(crate) fn decide_leaf_rect(
    fx: &mut FunnelCtx<'_>,
    y_src: &[u8],
    y_src_stride: usize,
    y_src_off: usize,
    y_recon: &mut [u8],
    y_stride: usize,
    abs_x: usize,
    abs_y: usize,
    w: usize,
    h: usize,
    dc_only: bool,
    sb_is_lvl6: bool,
) -> LeafChoice {
    let ev = evaluate_leaf(
        fx,
        y_src,
        y_src_stride,
        y_src_off,
        y_recon,
        y_stride,
        abs_x,
        abs_y,
        w,
        h,
        dc_only,
        sb_is_lvl6,
    );
    commit_leaf(fx, y_recon, y_stride, &ev, fx_partition_for_commit(fx));
    ev.into_choice()
}

/// Evaluate one PART_N block through the funnel WITHOUT committing —
/// C `md_encode_block` (the neighbour arrays / MD recon planes are
/// untouched; the caller commits the winning depth via [`commit_leaf`]).
#[allow(clippy::too_many_arguments)]
// `clippy::manual_checked_ops` post-dates the 1.89 MSRV floor's clippy, so the
// allow has to tolerate being unknown there (`cargo +1.89 clippy` otherwise
// reports `unknown lint` at this line).
#[allow(unknown_lints, clippy::manual_checked_ops)]
// the `> 0` guard scopes a whole block, not a single
// division; `checked_div` cannot express it without restructuring hot RD control flow
pub(crate) fn evaluate_leaf(
    fx: &mut FunnelCtx<'_>,
    y_src: &[u8],
    y_src_stride: usize,
    y_src_off: usize,
    y_recon: &[u8],
    y_stride: usize,
    abs_x: usize,
    abs_y: usize,
    w: usize,
    h: usize,
    // eff-M9: `is_dc_only_safe` fired for this block -> C's dc_cand_only
    // injection restricts the candidate list to {DC_PRED}
    // (mode_decision.c:3633). Always false at M6/M7/M8 (gate dead).
    dc_only: bool,
    // eff-M9 per-SB TXS gate: the SB stayed at PD0_LVL_6 (the pd0 detector
    // did not demote it to PD0_LVL_5). Consulted only when the config's
    // `txs_lvl6_gate` is set.
    sb_is_lvl6: bool,
) -> LeafEval {
    let frame = fx.frame;
    let rates = fx.rates;
    let lambda = frame.lambda;
    let mut qt = crate::quant::build_quant_table(frame.base_qindex);
    qt.qm_level = frame.qm_levels[0];
    // Per-plane chroma tables (== qt when the FH chroma deltas are 0).
    let mut qt_u = crate::quant::build_quant_table(frame.qindex_u);
    qt_u.qm_level = frame.qm_levels[1];
    let mut qt_v = crate::quant::build_quant_table(frame.qindex_v);
    qt_v.qm_level = frame.qm_levels[2];

    // bd10 LUMA mode funnel (task #94): when the bd10 recon canvas is present
    // (complete-SB eff-M9 bd10 — gated at construction) the MDS0 mode decision
    // must be made at TRUE 10-bit, not on the MSB-truncated u8 recon (which
    // scales `satd` exactly ×4 on `sample<<2` content and cannot flip the
    // survivor). C decides the mode at bd10; the ~+20/px hbd-predictor recon
    // divergence feeds a different prediction into DC↔SMOOTH near-ties. When
    // `bd10_funnel` is false (bd8, every other preset/partial-SB) NONE of the
    // bd10 branches below run and the path is byte-IDENTICAL.
    let bd10_funnel = fx.y_recon10.is_some();
    let (lambda_bd10_full, lambda_bd10_fast) = if bd10_funnel {
        // Full bd10 MD lambda (C full_lambda_md[1] = compute_rd_mult(10bit)×16,
        // md_process.c:753) — used for the winner-recon RDOQ.
        let lf = u64::from(crate::pd0::kf_full_lambda_bd10(
            frame.base_qindex,
            frame.cli_qp,
        ));
        // MDS0 fast cost lambda. C's fast loop calls `av1_intra_fast_cost(...,
        // fast_lambda_md[1], satd<<4)`, and the port's `rdcost(λ, rate, satd<<4)`
        // has the IDENTICAL structure (`(rate*λ+256)>>9 + (satd<<4)<<7`) — so the
        // port's fast lambda must be C's `fast_lambda_md[1]` EXACTLY. Verified vs
        // the real C interposer (SVT_FASTCOST_OUT lam=): it is `kf_full_lambda_
        // bd10 / 16` (the value BEFORE md_process.c's `full_lambda_md[1] *= 16`;
        // integer-exact since `*16` adds no low bits) — 22505@q20, 94716@q32,
        // 2053848@q55 all match. (This is a bd10-specific coincidence of the
        // rdmult-vs-SAD tables at ×16-vs-×4; the u8 path keeps frame.lambda.)
        (lf, lf / 16)
    } else {
        (0, 0)
    };
    // Task #6 chunk 1 — the block-local 10-bit LUMA source every bd10 stage
    // reads (MDS0 SATD, the MDS1/MDS3 `Bd10Rd` inputs, the `psq_resid10`
    // twin, and the eff-M9 winner re-encode). When the caller supplied a
    // NATIVE 10-bit source (`try_encode_frame_420_hbd`) these are the real
    // u16 samples, so the low 2 bits reach the mode decision AND the coded
    // levels; otherwise it is the identical `u8 << (bd - 8)` widening those
    // sites did inline before, i.e. byte-unchanged for every existing cell.
    // Built once per leaf instead of four times per leaf/candidate.
    let shift10 = (frame.bit_depth - 8) as u32;
    let blk_y_src10: Vec<u16> = if bd10_funnel {
        let mut blk = vec![0u16; w * h];
        match fx.src10.as_ref() {
            Some(s10) => {
                debug_assert!(
                    s10.y.len() >= (abs_y + h - 1) * s10.y_stride + abs_x + w,
                    "hbd luma plane must cover the aligned frame"
                );
                for r in 0..h {
                    let srow = (abs_y + r) * s10.y_stride + abs_x;
                    blk[r * w..(r + 1) * w].copy_from_slice(&s10.y[srow..srow + w]);
                }
            }
            None => {
                for r in 0..h {
                    let srow = y_src_off + r * y_src_stride;
                    for c in 0..w {
                        blk[r * w + c] = u16::from(y_src[srow + c]) << shift10;
                    }
                }
            }
        }
        blk
    } else {
        Vec::new()
    };

    // -- Block-level contexts (svt_aom_coding_loop_context_generation) --
    // Intra-mode and tx-size contexts are always neighbour-derived; the
    // skip_coeff context is only real when `update_skip_coeff_ctx` is set
    // (rate_est_level 1 at M6). M7/M8 (rate_est_level 4) price it at ctx 0.
    let above_ctx = fx.ectx.above_mode_ctx(abs_x);
    let left_ctx = fx.ectx.left_mode_ctx(abs_y);
    let skip_ctx = if fx.frame.cfg.real_coeff_ctx {
        fx.ectx.skip_ctx(abs_x, abs_y)
    } else {
        0
    };
    let fi_allowed_bsize = w <= 32 && h <= 32;
    let bsize_idx = svtav1_entropy::context::block_size_index(w, h);
    let cfl_allowed = usize::from(w <= 32 && h <= 32);
    let use_angle = !matches!((w, h), (4, 4) | (4, 8) | (8, 4));
    // C `is_chroma_reference(mi_row, mi_col, bsize, 1, 1)`
    // (common_utils.h:315): sub-8 blocks carry chroma only at odd mi in
    // the sub-8 dimension; the chroma block then covers the PAIR
    // (bsize_uv dims = max(dim,8)/2 at the ROUND_UV origin).
    let has_uv = ((abs_y / 4) % 2 == 1 || (h / 4).is_multiple_of(2))
        && ((abs_x / 4) % 2 == 1 || (w / 4).is_multiple_of(2));

    // Block geometry for the directional predictor (availability tables +
    // frame-edge clamps) and the per-block C `get_filt_type` inputs (the
    // above/left CODED-BLOCK modes' smoothness, per plane).
    // The ALIGNED luma extent, NOT the recon buffer's shape. `y_recon.len() /
    // y_stride` was wrong on a partial superblock: the recon working buffers
    // keep the aligned STRIDE but are sized to the SB-extent PRODUCT
    // (`pipeline.rs`, "SB extent (task #95 chunk 2)"), so at 96x88 that
    // division yields 170 rows, `dr_predict` derives `mi_rows = 44`, and the
    // frame-edge clamp `yd` it exists to compute never fires. C takes the same
    // quantity from `mb_to_bottom_edge` (`svt_aom_init_xd`,
    // adaptive_mv_pred.c:1055 -> `enc_intra_prediction.c:492`), i.e. the
    // ALIGNED extent. Byte-neutral on any frame where the two agree — every
    // 64-aligned one.
    let y_geom = UnitGeom {
        mi_row: abs_y >> 2,
        mi_col: abs_x >> 2,
        bw_px: w,
        bh_px: h,
        ss: 0,
        frame_w: frame.frame_w_px,
        frame_h: frame.frame_h_px,
        sb_mi_size: fx.frame.sb_mi_size,
        // Task #96: the tile this SB belongs to. The per-tile walk stamps
        // it on the funnel's own EntropyCtx (`fun_ectx`), so the MD
        // prediction sees the SAME boundaries the coded symbols do.
        // Whole-frame for a single-tile encode -> byte-identical.
        tile: fx.ectx.tile_mi,
    };
    // Chroma prediction geometry: for sub-8 chroma-ref blocks the unit
    // is the PAIR (C predicts the ROUND_UV-anchored bsize_uv block), so
    // the mi origin and luma dims are the pair's — the child's odd mi
    // would desync the plane coords from the availability tables.
    let uv_geom = UnitGeom {
        mi_row: ((abs_y >> 3) << 3) >> 2,
        mi_col: ((abs_x >> 3) << 3) >> 2,
        bw_px: w.max(8),
        bh_px: h.max(8),
        ss: 1,
        ..y_geom
    };
    let filt_type_y = fx.ectx.filt_type_y(abs_x, abs_y);
    let filt_type_uv = fx.ectx.filt_type_uv(abs_x, abs_y);
    // Chroma pair geometry (C blk_geom bsize_uv + ROUND_UV origins).
    let cw = w.max(8) / 2;
    let chh = h.max(8) / 2;
    let ccx = ((abs_x >> 3) << 3) / 2 + if w >= 8 { (abs_x % 8) / 2 } else { 0 };
    let ccy = ((abs_y >> 3) << 3) / 2 + if h >= 8 { (abs_y % 8) / 2 } else { 0 };

    // ---- cropped-TX RD distortion bound (task #95 chunk 2 (b)+(c)) ----
    // C prices the SPATIAL distortion of a TX block only over the part that
    // lies inside the ALIGNED frame (`cropped_tx_width`/`_height`,
    // product_coding_loop.c:4664; `cropped_tx_width_uv`/`_height_uv`,
    // full_loop.c:2228). That matters ONLY on a partial superblock, where a
    // coded block may straddle the aligned extent; on a 64-aligned frame both
    // crops are the identity and every distortion expression is unchanged.
    //
    // The ALIGNED frame extent the bound is taken against. `FrameDims::new`
    // would re-derive it from TRUE dims; the funnel already carries the
    // aligned values, so construct them directly (aligned-of-aligned is a
    // fixed point — the dims are multiples of 8).
    let aligned_dims = crate::frame_geom::FrameDims {
        true_w: frame.frame_w_px,
        true_h: frame.frame_h_px,
        aligned_w: frame.frame_w_px,
        aligned_h: frame.frame_h_px,
    };
    // The CHROMA crop is candidate-independent (one chroma txb per block:
    // `tu_count` is 1 at every tx_depth on the chroma path, full_loop.c:2221,
    // so `txb_origin` is always (0,0) and `ccx`/`ccy` — which already ARE
    // `ROUND_UV(luma) >> 1` — are the C origins), so it is computed once here
    // and shared by every chroma tx_unit call below.
    let uv_crop = crate::frame_geom::cropped_tx_dims_uv(&aligned_dims, ccx, ccy, cw, chh);
    // The whole-block LUMA crop — the depth-0 txb's crop, which is what the
    // MDS1 luma full loop (one txb at the block origin) computes. MDS1 scores
    // the FREQ-domain distortion, where the crop is inert (C's coefficient
    // -domain facade takes the full tx dims); it is threaded anyway so every
    // luma call site names the same C quantity, and so a future spatial MDS1
    // (C's `!is_inter && tx_depth` arm) is already correct.
    let blk_crop = crate::frame_geom::cropped_tx_dims(&aligned_dims, abs_x, abs_y, w, h);

    // bd10 FULL-RD (task #94, MODE axis): the MDS1/MDS3 inputs at true depth.
    // Built once per leaf; `None` on every u8 path AND on bd10 leaves where
    // only the MDS0 funnel is enabled, so both stay byte-identical.
    let bd10_rd: Option<Bd10Rd> = if bd10_funnel && fx.full_rd10 {
        let shift = shift10;
        // Task #6 chunk 1: the shared block-local 10-bit luma source (real u16
        // when the caller supplied a native HBD source, else the same
        // `u8 << shift` widening this site did inline).
        let y_src10 = blk_y_src10.clone();
        let mut qt10 = crate::quant::build_quant_table_bd(frame.base_qindex, frame.bit_depth);
        qt10.qm_level = frame.qm_levels[0];
        let mut qt_u10 = crate::quant::build_quant_table_bd(frame.qindex_u, frame.bit_depth);
        qt_u10.qm_level = frame.qm_levels[1];
        let mut qt_v10 = crate::quant::build_quant_table_bd(frame.qindex_v, frame.bit_depth);
        qt_v10.qm_level = frame.qm_levels[2];
        // Block-local 10-bit chroma sources at stride cw (empty when the block
        // carries no chroma — C skips every chroma stage on !has_uv).
        let (mut u_src10, mut v_src10) = (Vec::new(), Vec::new());
        if has_uv {
            let c_off = ccy * fx.c_stride + ccx;
            u_src10 = vec![0u16; cw * chh];
            v_src10 = vec![0u16; cw * chh];
            match fx.src10.as_ref() {
                // Task #6 chunk 1: real 10-bit chroma samples (same strided
                // layout as the u8 `u_src`/`v_src`).
                Some(s10) => {
                    let c_off10 = ccy * s10.c_stride + ccx;
                    for r in 0..chh {
                        let srow = c_off10 + r * s10.c_stride;
                        u_src10[r * cw..(r + 1) * cw].copy_from_slice(&s10.u[srow..srow + cw]);
                        v_src10[r * cw..(r + 1) * cw].copy_from_slice(&s10.v[srow..srow + cw]);
                    }
                }
                None => {
                    for r in 0..chh {
                        let srow = c_off + r * fx.c_stride;
                        for c in 0..cw {
                            u_src10[r * cw + c] = u16::from(fx.u_src[srow + c]) << shift;
                            v_src10[r * cw + c] = u16::from(fx.v_src[srow + c]) << shift;
                        }
                    }
                }
            }
        }
        Some(Bd10Rd {
            y_src10,
            u_src10,
            v_src10,
            qt: qt10,
            qt_u: qt_u10,
            qt_v: qt_v10,
            lambda: lambda_bd10_full,
            bd: frame.bit_depth,
        })
    } else {
        None
    };

    // -- Candidate injection + MDS0 --
    // C order (`generate_md_stage_0_cand`): regular intra modes DC ..
    // intra_mode_end with the angular-delta inner loop in counter order
    // (-3..3, level >= 2 keeping {-3, 0, +3}; inject_intra_candidates,
    // mode_decision.c:3254-3271), then filter-intra
    // (inject_filter_intra_candidates — FILTER_DC only at fi level 2).
    let cfg = frame.cfg;
    let do_rdoq = frame.rdoq_level > 0;
    // Chroma txb contexts (real at rate_est_level 1; candidate-independent
    // — the neighbour bytes don't change during this block's search).
    let (cb_tsc, cb_dsc) = if cfg.real_coeff_ctx {
        let (a, l) = fx.ectx.coeff_neighbors_uv(0, ccx, ccy, cw, chh);
        cc::get_txb_ctx(1, a, l, true, false)
    } else {
        (0, 0)
    };
    let (cr_tsc, cr_dsc) = if cfg.real_coeff_ctx {
        let (a, l) = fx.ectx.coeff_neighbors_uv(1, ccx, ccy, cw, chh);
        cc::get_txb_ctx(1, a, l, true, false)
    } else {
        (0, 0)
    };

    // The per-leaf chroma context every chroma evaluation reads. All of it is
    // candidate-INDEPENDENT (pair geometry from the ROUND_UV origin; neighbour
    // txb contexts read once -- the neighbouring bytes cannot change during
    // this block's own search), which is what lets the three arms in [`chroma`]
    // be free functions over one shared value instead of closures capturing
    // fourteen locals across the whole funnel body.
    let cx = chroma::ChromaCtx {
        cw,
        chh,
        ccx,
        ccy,
        uv_geom,
        filt_type_uv,
        uv_crop,
        cb_tsc,
        cb_dsc,
        cr_tsc,
        cr_dsc,
        do_rdoq,
        qt_u,
        qt_v,
    };

    // No-palette flag pricing for this leaf (C svt_aom_allow_palette on the
    // LUMA bsize; both dims <= 64 and not 4x4/4x8/8x4).
    let allow_pal = svtav1_entropy::context::allow_palette(cfg.allow_sct, w, h);
    // C svt_aom_get_palette_mode_ctx (rd_cost.c:583): neighbor palette-mode
    // ctx (above+left count of palette-coded neighbours, 0..=2), read from
    // the MD decision grid (stamped by commit_leaf in coding order). 0 until
    // a palette candidate wins a neighbour => byte-identical for non-screen
    // content, where no leaf ever carries a palette.
    // Regular (y-palette-off) candidates price the [0] row; the palette
    // candidate prices the [1] row (use_palette_y=1) via `uv_no_y1`.
    let pal = {
        let mode_ctx = fx.ectx.palette_neighbor_ctx(abs_x, abs_y);
        PalFlagRates {
            mode_ctx,
            y_no: if allow_pal {
                rates.palette_y_no[svtav1_entropy::context::palette_bsize_ctx(w, h)][mode_ctx]
                    as u64
            } else {
                0
            },
            uv_no: if allow_pal {
                rates.palette_uv_no[0] as u64
            } else {
                0
            },
            uv_no_y1: if allow_pal {
                rates.palette_uv_no[1] as u64
            } else {
                0
            },
        }
    };
    let (pal_uv_no, pal_uv_no_y1) = (pal.uv_no, pal.uv_no_y1);

    let mut ind_uv: Option<[(u8, i8); 13]> = None;
    // -- Candidate injection + MDS0 -- see [`inject`].
    // C `generate_md_stage_0_cand`: regular intra, filter-intra, palette, then
    // IntraBC, each scored with the Hadamard SATD fast cost. The returned order
    // is C's PROCESSING order and the MDS0 pool below depends on it.
    let geom = LeafGeom {
        w,
        h,
        abs_x,
        abs_y,
        has_uv,
        y_geom,
        filt_type_y,
        bsize_idx,
        cfl_allowed,
        use_angle,
        fi_allowed_bsize,
        above_ctx,
        left_ctx,
    };
    let bd10 = LeafBd10 {
        active: bd10_funnel,
        blk_y_src10: &blk_y_src10,
        lambda_fast: lambda_bd10_fast,
        rd: &bd10_rd,
    };
    let mut cands = inject::inject_candidates(
        fx,
        &geom,
        &cx,
        bd10,
        pal,
        lambda,
        y_src,
        y_src_stride,
        y_src_off,
        y_recon,
        y_stride,
        dc_only,
        &mut ind_uv,
    );

    // -- MDS0 -> MDS1 staging: replacement pool, per-class sort, dev-prune --
    // C `md_stage_0` + `sort_fast_cost_based_candidates` +
    // `post_mds0_nic_pruning`, all per candidate class. See [`nic`].
    let staging = nic::stage_mds0_to_mds1(&cands, cfg, frame.cli_qp);
    let order = &staging.order;
    let n1 = order.len();

    // -- MDS1: luma-only full loop (freq dist, quantize_b, DCT, depth 0) --
    for &ci in order.iter().take(n1) {
        let cand = &mut cands[ci];
        let (txb_skip_ctx, dc_sign_ctx) = if cfg.real_coeff_ctx {
            let (above, left) = fx.ectx.coeff_neighbors(abs_x, abs_y, w, h);
            cc::get_txb_ctx(0, above, left, true, false)
        } else {
            (0, 0)
        };
        // The intra dir feeding the ext-tx-type rate row: C prices FILTER
        // candidates at the fi-MAPPED direction (fimode_to_intradir; rd_cost.c
        // :135) at EVERY stage. MDS3's txt_search already mapped it — MDS1
        // didn't, under-pricing fi=V/H/D157 coeff rates by the row delta
        // (g128 q20 p0 16x4@(2,0): C ycb higher by exactly 630/684/736 for
        // fi=1/2/3 with bit-equal dists; fi=0/4 map to DC and matched).
        let intra_dir = if cand.ibc.is_some() {
            // IBC chunk 7: inter-classified — the coeff cost's tx-type
            // rate reads the INTER rows (av1_txt_rate_est is_inter arm).
            INTER_TXT_DIR
        } else if cand.fi != FI_NONE {
            FIMODE_TO_INTRADIR[cand.fi as usize] as usize
        } else {
            cand.mode as usize
        };
        let out = tx_unit(
            y_src,
            y_src_stride,
            y_src_off,
            &cand.pred,
            w,
            0,
            w,
            h,
            cc::DCT_DCT,
            0,
            txb_skip_ctx,
            dc_sign_ctx,
            intra_dir,
            &qt,
            frame,
            rates,
            false, // no RDOQ at MDS1
            false, // freq-domain dist
            blk_crop,
            // R1: MDS1's reconstruction is UNREAD. The loop body below takes
            // only `out.eob / .bits / .dist` (and `out10`'s twins); grepping
            // the whole MDS1 loop for `out.` finds exactly that one line. C
            // agrees structurally: its inverse-transform gate is
            // `mds_do_spatial_sse || (!is_inter && tx_depth)`
            // (product_coding_loop.c:4783-4784), all-intra pins
            // `spatial_sse_full_loop_level = 3` (SSSE_MDS3,
            // enc_mode_config.c:10010) so `mds_do_spatial_sse` is FALSE at
            // MDS1 (:7025), and MDS1 evaluates tx_depth 0 — both disjuncts
            // false, so C inverts nothing here at any all-intra preset.
            false,
            RateMode::Exact,
        );
        // bd10 FULL-RD (task #94): C's MDS1 at hbd_md != 0 runs the SAME
        // luma-only full loop on 10-bit pixels — 10-bit residual, bd10 quant
        // table, bd10 lambda, and the bit-depth-INDEPENDENT freq-domain
        // distortion (svt_aom_picture_full_distortion32_bits_single). Deciding
        // it at 8 bits picks C's bd8 winner; below eff-M9 several candidates
        // survive to MDS3, so this ordering + the pruning below is binding.
        // The u8 `out` above still runs — nothing downstream of MDS1 reads it,
        // but keeping it keeps the bd8 expression untouched and the two
        // domains directly comparable under SVTAV1_CANDDBG.
        let out10 = bd10_rd.as_ref().map(|b| {
            tx_unit_hbd(
                &b.y_src10,
                w,
                0,
                &cand.pred10,
                w,
                0,
                w,
                h,
                cc::DCT_DCT,
                0,
                txb_skip_ctx,
                dc_sign_ctx,
                &b.qt,
                frame.rdoq_level,
                b.lambda,
                frame.sharpness,
                rates,
                false, // no RDOQ at MDS1 (mirrors the u8 call)
                b.bd,
                b.qt.qm_level,
                Some(&TxRdArgs {
                    spatial_dist: false, // MDS1 = freq-domain residual
                    intra_dir,
                    coeff_rate_est_lvl: cfg.coeff_rate_est_lvl,
                    tx_bias: frame.tx_bias,
                    crop: blk_crop,
                }),
            )
        });
        let (dec_eob, dec_bits, dec_dist, dec_lambda) = match &out10 {
            Some(o) => (
                o.eob,
                o.bits as u64,
                o.dist,
                bd10_rd.as_ref().unwrap().lambda,
            ),
            None => (out.eob, out.bits as u64, out.dist, lambda),
        };
        let has = dec_eob > 0;
        let tsz_cat = tx_size_cat(w, h);
        let tsz_ctx = fx.ectx.tx_size_ctx(abs_x, abs_y, w, h);
        // C: 4x4 codes no tx_size symbol (block_signals_txsize == bsize > 4x4).
        // IBC (inter-classified): tx_size codes via the var-tx walk when the
        // block has coeffs, and ZERO bits when skip (svt_aom_tx_size_bits'
        // `!(is_inter_tx && skip)` gate) — svt_aom_full_cost prices exactly
        // that pair at MDS1 too.
        let coeff_rate = if cand.ibc.is_some() {
            let vartx_bits = if has && block_signals_txsize(w, h) {
                crate::vartx::tx_size_bits_vartx(
                    &rates.txfm_partition_fac_bits,
                    fx.ectx.txfm_above_span(abs_x, w),
                    fx.ectx.txfm_left_span(abs_y, h),
                    w,
                    h,
                    0, // MDS1 evaluates depth 0
                    abs_y,
                    frame.frame_h_px,
                )
            } else {
                0
            };
            if has {
                dec_bits + vartx_bits + rates.skip[skip_ctx][0] as u64
            } else {
                rates.skip[skip_ctx][1] as u64
            }
        } else {
            let tx_size_bits = if block_signals_txsize(w, h) {
                rates.tx_size[tsz_cat][tsz_ctx][0] as u64
            } else {
                0
            };
            if has {
                dec_bits + tx_size_bits + rates.skip[skip_ctx][0] as u64
            } else {
                rates.skip[skip_ctx][1] as u64 + tx_size_bits
            }
        };
        cand.mds1_has_coeff = has;
        cand.full_cost = rdcost(dec_lambda, cand.flr + cand.fcr + coeff_rate, dec_dist);
        #[cfg(feature = "std")]
        if crate::dbgenv::canddbg() && crate::depth_refine::nsqdbg_here(abs_x, abs_y) {
            eprintln!(
                "NSQDBG PMDS1 mi=({},{}) {}x{} mode={} fi={} delta={} uv={} coeff_rate={} dist={} full={}",
                abs_y / 4,
                abs_x / 4,
                w,
                h,
                cand.mode,
                cand.fi,
                cand.delta,
                cand.uv,
                coeff_rate,
                dec_dist,
                cand.full_cost,
            );
        }
    }

    // -- MDS1 -> MDS3 staging: per-class full-cost sort + the two prunes --
    // C `sort_full_cost_based_candidates` + `post_mds1_nic_pruning` +
    // `post_mds2_nic_pruning`. See [`nic`].
    let staging3 = nic::stage_mds1_to_mds3(&cands, cfg, &staging);
    let order1 = staging3.order1;
    let n3 = staging3.n3;

    // -- MDS3: full loop with TXS + TXT + RDOQ + spatial SSE + chroma --
    // txs_level 0 (M8) -> depth 0 only; else get_end_tx_depth clamped by
    // the config's intra sq/nsq max depths. At eff-M9 the enable is per-SB
    // (txs_lvl6_gate): C only bumps txs on for SBs the pd0 detector left at
    // PD0_LVL_6 (undemoted); demoted PD0_LVL_5 SBs keep TXS off (depth 0).
    let txs_active = cfg.txs_on && (!cfg.txs_lvl6_gate || sb_is_lvl6);
    // C `get_start_end_tx_depth` (product_coding_loop.c:6710-6717):
    //
    //     // end_tx_depth set to zero for blocks which go beyond the picture
    //     // boundaries
    //     if (blk_org_x + bwidth <= aligned_width &&
    //         blk_org_y + bheight <= aligned_height)
    //         *end_tx_depth = get_end_tx_depth(bsize);
    //     else
    //         *end_tx_depth = 0;
    //
    // A leaf that STRADDLES the aligned frame edge is searched at tx depth 0
    // only. The port had no boundary term and searched a depth C never tests.
    //
    // MEASURED reachability (2026-08-03, `gradient {80,104,72}x88 q55`, one
    // straddling leaf per frame):
    //   p6  leaf (0,64) 64x32   txs_active=true   end_tx_depth would be 0 -> no-op
    //   p7  leaf (32,64) 32x32  txs_active=true   end_tx_depth would be 1 -> LIVE
    //   p8  leaf (32,64) 32x32  txs_active=false  end_tx_depth would be 0 -> no-op
    // So this is a real divergence at preset 7. It is byte-INERT on the 48
    // partial-SB cells swept ({80x88,104x88,72x88,96x80,88x72,120x104,72x120,
    // 104x72} x p{6,7,8} x q{32,55}) — it changes the searched depth set without
    // flipping any cell's verdict there — but "inert on what we measured" is not
    // "unreachable", and the p7 arm is exercised.
    let in_frame = abs_x + w <= frame.frame_w_px && abs_y + h <= frame.frame_h_px;
    let end_depth = if txs_active && in_frame {
        end_tx_depth(w, h, &cfg)
    } else {
        0
    };
    let tsz_cat = tx_size_cat(w, h);
    let tsz_ctx = fx.ectx.tx_size_ctx(abs_x, abs_y, w, h);

    // -- Independent chroma search before MDS3 (chroma_level 4:
    //    `search_best_mds3_uv_mode`, product_coding_loop.c:7301, invoked at
    //    :9625-9637 when `perform_ind_uv_search_last_mds` (:1472-1504)
    //    returns true. Produces best_uv[(luma mode)] -> (uv mode, uv delta);
    //    `update_intra_chroma_mode` (:7063) then rewrites each MDS3
    //    candidate before its full loop. --
    //
    // The gate has TWO arms, and the second one is live here (issue #15):
    //
    //  a) `mds3_intra_count` (:1478-1487) counts the MDS3 survivors that are
    //     NOT inter-classified and — with `skip_ind_uv_if_only_dc = 1`, which
    //     is chroma_level 4's setting (enc_mode_config.c:4373) — whose
    //     injected (uv-follows-luma) uv mode is not UV_DC.
    //  b) the `inter_vs_intra_cost_th` arm (:1498-1501) then ZEROES that count
    //     when `best_inter_cost * th < best_intra_cost * 100`, th = 100 at
    //     chroma_level 4 (enc_mode_config.c:4372) — i.e. when the best
    //     inter-classified candidate's MDS1 full cost beats every intra
    //     candidate's.
    //
    // Arm (b) was previously commented here as "never fires on I-slices,
    // MAX_MODE_COST * 100 does not overflow and dwarfs any intra cost". The
    // overflow half is right (MAX_MODE_COST = 13754408443200 * 8,
    // coding_unit.h:37, so * 100 is ~1.1e16, far under 2^64) but the
    // conclusion was WRONG: `is_inter` here is
    // `is_inter_mode(mode) || use_intrabc` (:1479-1481), so on a SCREEN-CONTENT
    // I-slice a winning IntraBC candidate makes `best_inter_cost` an ordinary
    // finite cost and the arm fires. MEASURED on `terminal` 188x256 (the last
    // two divergent cells of tools/unaligned_identity_scan.sh): at p2 q55
    // mi=(50,42) C's MDS1 best intra = 97_762_561 vs best IntraBC = 84_376_537,
    // and at p4 q12 mi=(46,46) 163_691 vs 148_994 — the arm fires in both, C
    // sets `ind_uv_avail = 0` (confirmed directly by the
    // `svt_aom_get_intra_uv_fast_rate` interposer, `indavail=0`), every MDS3
    // candidate keeps its uv-follows-luma pair, and C codes uv=D113/-1 resp.
    // UV_CFL where the port's table said UV_DC.
    //
    // C's `is_inter` for both the count and the two cost minima is
    // `is_inter_mode(block_mi.mode) || block_mi.use_intrabc`; the port has no
    // inter modes on this all-intra path, so IntraBC is the whole of it.
    const IND_UV_INTER_VS_INTRA_TH: u64 = 100; // chroma_level 4, enc_mode_config.c:4372
    let ind_uv_gate = cfg.ind_uv_mds3 && has_uv && {
        let mut intra_count = 0usize;
        let mut best_intra = u64::MAX;
        let mut best_inter = u64::MAX;
        for &ci in order1.iter().take(n3) {
            let c = &cands[ci];
            if c.ibc.is_some() {
                best_inter = best_inter.min(c.full_cost);
            } else {
                if c.uv != 0 {
                    intra_count += 1;
                }
                best_intra = best_intra.min(c.full_cost);
            }
        }
        // C SEEDS both minima with MAX_MODE_COST and only ever lowers them, so
        // an absent class — or a class whose every candidate costs more than
        // the seed — compares as that CONSTANT, not as "infinity". Clamping
        // the u64::MAX-seeded minima to it reproduces both cases exactly.
        // With no IntraBC candidate this makes the arm inert as the old
        // comment assumed: 1.1e16 is not < (an intra cost) * 100.
        const MAX_MODE_COST: u64 = 13_754_408_443_200 * 8; // coding_unit.h:37
        let best_intra = best_intra.min(MAX_MODE_COST);
        let best_inter = best_inter.min(MAX_MODE_COST);
        if best_inter * IND_UV_INTER_VS_INTRA_TH < best_intra * 100 {
            intra_count = 0;
        }
        intra_count > 0
    };
    if ind_uv_gate {
        // Distinct (uv, uv_delta) pairs of the MDS3 survivors, in
        // survivor order, excluding UV_DC; then UV_DC (delta 0) last.
        let mut tested = [[false; 7]; 13];
        let mut uv_list: Vec<(u8, i8)> = Vec::new();
        for &ci in order1.iter().take(n3) {
            let (uvm, uvd) = (cands[ci].uv, cands[ci].uv_delta);
            if uvm == 0 || tested[uvm as usize][(3 + uvd) as usize] {
                continue;
            }
            tested[uvm as usize][(3 + uvd) as usize] = true;
            uv_list.push((uvm, uvd));
        }
        uv_list.push((0, 0));

        // Full loop per uv candidate: coeff_rate + SSD distortion
        // (DIST_CALC_RESIDUAL — both planes summed).
        //
        // bd10 FULL-RD (task #94): C runs search_best_mds3_uv_mode ENTIRELY at
        // hbd_md — `full_lambda = full_lambda_md[hbd_md ? EB_10_BIT_MD :
        // EB_8_BIT_MD]` (product_coding_loop.c:7307) with 10-bit prediction/
        // residual (:7397/:7415/:7429) and the 10-bit full-loop distortion
        // (svt_aom_full_loop_uv, :7443). Deciding the uv mode on the u8
        // `chroma_eval` + u8 `lambda` flips near-ties: on 1001682 q12 p5 block
        // (0,0) the port picked UV_V_PRED where C picks UV_DC_PRED. Use the
        // 10-bit twin at bd10; bd8 keeps `chroma_eval` and is byte-unchanged.
        let mut uv_rd: Vec<(u64, u64)> = Vec::with_capacity(uv_list.len());
        for &(uvm, uvd) in &uv_list {
            let (bits, dist) = match bd10_rd.as_ref() {
                Some(b) => {
                    let (u_out, v_out) = chroma::eval_uv_hbd(&cx, fx, b, uvm, uvd);
                    (
                        u_out.bits as u64 + v_out.bits as u64,
                        u_out.dist + v_out.dist,
                    )
                }
                None => {
                    let (u_out, v_out) = chroma::eval_uv(&cx, fx, uvm, uvd);
                    (
                        u_out.bits as u64 + v_out.bits as u64,
                        u_out.dist + v_out.dist,
                    )
                }
            };
            uv_rd.push((bits, dist));
        }

        // Per distinct surviving luma mode (survivor order), pick the
        // lowest-cost uv pair (strict less, list order on ties). At bd10 the
        // compare uses the SAME 10-bit lambda C prices this search with
        // (`full_lambda_md[EB_10_BIT_MD]`, :7307/:7491), matching the 10-bit
        // `uv_rd` above; bd8 takes the `None` arm and keeps the u8 `lambda`.
        let uv_lambda = bd10_rd.as_ref().map_or(lambda, |b| b.lambda);
        let mut table = [(0u8, 0i8); 13];
        let mut mode_seen = [false; 13];
        for &ci in order1.iter().take(n3) {
            // C search_best_mds3_uv_mode skips inter-classified candidates
            // (product_coding_loop.c:7335 — an IntraBC cand keeps UV_DC and
            // never seeds a per-luma-mode table row).
            if cands[ci].ibc.is_some() {
                continue;
            }
            let luma = cands[ci].mode as usize;
            if mode_seen[luma] {
                continue;
            }
            mode_seen[luma] = true;
            let mut best_cost = u64::MAX;
            for (k, &(uvm, uvd)) in uv_list.iter().enumerate() {
                let mut fcr2 = rates.uv[cfl_allowed][luma][uvm as usize] as u64;
                if use_angle && matches!(uvm, 1..=8) {
                    fcr2 += rates.angle[uvm as usize - 1][(3 + uvd) as usize] as u64;
                }
                if uvm == 0 {
                    fcr2 += pal_uv_no; // rd_cost.c:514 (inside uv fast rate)
                }
                let (bits, dist) = uv_rd[k];
                let cost = rdcost(uv_lambda, bits + fcr2, dist);
                #[cfg(feature = "std")]
                if crate::dbgenv::canddbg() && crate::depth_refine::nsqdbg_here(abs_x, abs_y) {
                    eprintln!(
                        "NSQDBG UVTAB2 mi=({},{}) luma={luma} uv={uvm} uvd={uvd} bits={bits} dist={dist} fcr={fcr2} cost={cost}",
                        abs_y / 4,
                        abs_x / 4,
                    );
                }
                if cost < best_cost {
                    best_cost = cost;
                    table[luma] = (uvm, uvd);
                }
            }
        }
        ind_uv = Some(table);
    }

    // bd10 FULL-RD (task #94): every MDS3 rdcost — the depth compare, the txb
    // early exits and the final block cost — must use the SAME lambda domain
    // as the distortion it is comparing. C uses `full_lambda_md[hbd_md ? 1 : 0]`
    // throughout (md_process.c:753), so one substitution covers all of them.
    let lambda3 = bd10_rd.as_ref().map_or(lambda, |b| b.lambda);
    for &ci in order1.iter().take(n3) {
        // `update_intra_chroma_mode`: rewrite the candidate's chroma from
        // the ind-uv table (fast chroma rate recomputed for the luma
        // mode + new uv pair — same formula as injection, so an
        // unconditional recompute is C-identical).
        // C gates the rewrite on `ind_uv_avail && ind_uv_last_mds` (:7063)
        // — it runs for last_mds 1 (M1) and 2 (M2/M3) but NOT for
        // last_mds 0 (M0), whose candidates were already injected FROM the
        // table and keep it. (The earlier "A/B proved rewrite needed for
        // both configs" note toggled M0+M1 together; the q40-64 breakage
        // came from the M1 cells, where C does rewrite.)
        if let Some(tbl) = &ind_uv {
            // C update_intra_chroma_mode skips inter-classified candidates
            // (:7077 `!is_inter` gate) — an IntraBC cand keeps UV_DC.
            if (cfg.ind_uv_last_mds1 || cfg.ind_uv_mds3) && cands[ci].ibc.is_none() {
                // The rewrite keys on the CODED luma mode (`cand->block_mi.mode`
                // in update_intra_chroma_mode — DC for FILTER candidates), NOT
                // the fi-mapped direction. A/B-verified (g64 p0): mapping the
                // key broke q40.
                let (uvm, uvd) = tbl[cands[ci].mode as usize];
                let c = &mut cands[ci];
                c.uv = uvm;
                c.uv_delta = uvd;
                let mut fcr = rates.uv[cfl_allowed][c.mode as usize][uvm as usize] as u64;
                if use_angle && matches!(uvm, 1..=8) {
                    fcr += rates.angle[uvm as usize - 1][(3 + uvd) as usize] as u64;
                }
                if uvm == 0 {
                    // rd_cost.c:515-521 — the UV_DC palette-flag row is keyed on
                    // `use_palette_y = cand->palette_info && palette_size[0] > 0`
                    // read off the REAL candidate, and C's recompute here is
                    // `svt_aom_get_intra_uv_fast_rate(pcs, ctx, cand_bf, 1)` on
                    // that same candidate (update_intra_chroma_mode,
                    // product_coding_loop.c:7095). So a LUMA-PALETTE candidate
                    // pays the [1] row, not the [0] row every regular candidate
                    // pays — the same distinction the injection site (:4596)
                    // already makes. C's rewrite is conditional (only when the uv
                    // pair actually changed, :7084); when it does NOT fire the
                    // candidate keeps the fast_chroma_rate injection gave it,
                    // which for a palette candidate is ALSO the [1] row — so the
                    // port's unconditional recompute is C-identical only if it
                    // uses the same row. Charging [0] here undid :4596 for every
                    // ind_uv_last_mds preset (M1..M5), under-costing a palette
                    // candidate's chroma flag and biasing the palette-vs-regular
                    // RD tie toward palette (#71 over-picking). MEASURED
                    // 2026-08-04: flips `screen 64 64 63 1` (C 64B, port 71->64B)
                    // and `screen 128 128 63 1` (C 185B, port 193->185B) to byte
                    // MATCH — both KNOWN_DIFF pins of tools/identity_full_8bit.sh,
                    // promoted in this commit — and moves NO other cell of the
                    // 976-cell synthetic+dims scoreboard.
                    fcr += if c.palette.is_some() {
                        pal_uv_no_y1
                    } else {
                        pal_uv_no
                    };
                }
                c.fcr = fcr;
            }
        }
        // ---- Luma: TX depth loop ----
        // KNOWN GAP (pinned, screen-IBC grind 2026-07-23): C's TXS depth
        // CLASS clamp keys on `is_intra_mode(cand->block_mi.mode)`
        // (get_start_end_tx_depth, product_coding_loop.c:6728-6733) — an
        // IntraBC candidate keeps mode DC_PRED, so C searches IBC txs
        // depths under the INTRA caps (2/2 at p0..p3 txs_level 2), NOT the
        // inter caps (1/1) used here. Widening the port to the intra caps
        // was MEASURED to flip windows95_p0_q20 to a byte MATCH, but any
        // cell where the port then PICKS an IBC depth-2 winner emits a
        // stream the decode oracle reads differently (16 cells
        // SELF-DESYNC: the depth-2 inter var-tx pack chain — txfm
        // partition ctx / per-txb syntax — disagrees with the oracle's
        // z-order transform_tree read somewhere past the first nonzero
        // txb, and C streams never code depth-2 IBC on this corpus to
        // arbitrate). Until that chain is proven, keep the inter caps:
        // every emitted stream stays self-consistent (depth <= 1, where
        // the inter z-order == raster).
        // MEASURED 2026-08-04 (CID22 1028637 512x512 crop q32 p0, mi(36,16)
        // 16x16): widening this to the intra caps is NECESSARY but NOT
        // SUFFICIENT for the block C codes as IntraBC at tx_depth 2. With the
        // inter cap the port's IBC candidate is capped at depth 1, picks depth
        // 0 and costs 17_683_025; with the intra cap it reaches depth 2 and
        // costs 16_821_993 — still beaten by the SMOOTH_H intra candidate at
        // 16_371_896, so the block stays intra either way. At depth 2 the IBC
        // candidate is CHEAPER in rate than that winner (68_090 vs 69_158, in
        // 1/512-bit units) and loses purely on distortion (33_264 vs 28_208) —
        // with the SAME DV as C (search returns exactly dv_ref = (0,-1024),
        // which is why C codes MV_JOINT_ZERO) and an identical recon to copy
        // from. So the second gap is in the DEPTH-2 var-tx handling of an
        // inter-classified candidate, i.e. the same unproven chain the
        // paragraph above pins for the PACK side. This image finally supplies
        // cells where C itself codes depth-2 IntraBC (q32 p0 mi(36,16) 16x16;
        // q48 p2 mi(16,0) 32x32) — which is the arbitration the earlier grind
        // said the corpus could not provide.
        //
        //
        // THE ARBITRATION CASE NOW EXISTS (found 2026-08-04 with the
        // `SVTAV1_TXDEPTH_XY` / C `SVT_TXDEPTH_XY` depth-cost pair, on a
        // real-C drill): gb82-sc **graph.png 512x512 q63 preset 0**, block
        // mi(8,80) — a 32x32 IntraBC leaf at (320,32), dv (0,-2496). C's
        // stream codes it at **tx_depth 2**, so the depth-2 IBC var-tx pack
        // chain finally has a byte-verified oracle. MEASURED depth costs,
        // BIT-IDENTICAL on the depths both sides search:
        //     d=0  ycb 6790  txsz 814   dist 13648528  cost 1972278747  (both)
        //     d=1  ycb 16003 txsz 1308  dist  8029920  cost 1540665092  (both)
        //     d=2  ycb 31118 txsz 2316  dist  4069248  cost 1511340117  (C only)
        // so the port loses this cell purely by never SEARCHING d=2 — every
        // term it does compute already matches C exactly. C's coded syntax
        // for that block (from the in-source op trace, first ops after the
        // block marker) is 5 txfm_partition flags all = 1 (one at 32x32 +
        // four at 16x16) — i.e. a UNIFORM 8x8 tiling, the shape the port's
        // uniform-`depth` model already represents — then per-txb
        // all_zero / tx_type over the 16-type inter set (`CDF nsyms=16`) /
        // `eob_pt_64` (`CDF nsyms=7`). Widen the caps against THAT stream.
        let cand_end_depth = if cands[ci].ibc.is_some() {
            if txs_active {
                end_tx_depth_inter(w, h, &cfg)
            } else {
                0
            }
        } else {
            end_depth
        };
        let mut best_depth = 0u8;
        let mut best_cost = u64::MAX;
        let mut best_bits: u64 = 0;
        let mut best_dist: u64 = 0;
        let mut best_txb_q: Vec<Vec<i32>> = Vec::new();
        let mut best_txb_eob: Vec<u16> = Vec::new();
        let mut best_txb_cul: Vec<u8> = Vec::new();
        let mut best_txb_type: Vec<u8> = Vec::new();
        let mut best_recon: Vec<u8> = Vec::new();
        // The winning depth's TRUE 10-bit luma recon (bd10 full-RD only) —
        // the 10-bit twin of `best_recon`.
        let mut best_recon10: Vec<u16> = Vec::new();
        // The winning depth's luma PREDICTION, i.e. C `cand_bf->pred->y_buffer`
        // as it stands once the TX loop returns. NOT the same as `cand.pred`
        // (the MDS0 whole-block pred) whenever the winning depth > 0 — see the
        // detector call below for why the difference is observable.
        let mut best_pred: Vec<u8> = Vec::new();
        // The bd10 twin of `best_pred` — C's `cand_bf->pred->y_buffer` at
        // `hbd_md`, which is what `chroma_complexity_check_pred`'s SAD arm
        // reads (product_coding_loop.c:6049). Empty on every u8 path.
        let mut best_pred10: Vec<u16> = Vec::new();
        // The tx_depth-0 (whole-block-pred) recon, kept regardless of which
        // depth wins. C's `cand_bf->recon` is the SHARED ctx temp buffer:
        // deeper depths reconstruct into the AUX tx-depth buffers and
        // update_tx_cand_bf copies pred/coeffs/eob back but NEVER the recon —
        // so after the TX loop the shared recon still holds the DEPTH-0
        // recon, and that is what `calc_scr_to_recon_dist_per_quadrant`
        // (skip-sub-depth cond1 + the NSQ recon-dist gates) measures.
        // Proven on 1147124 q20 p4 (76,96): C fill luma quads sum 971<<4 ==
        // C's OWN depth-0 dist 15536, while the winning depth-1 dist is
        // 11904 (== this port's winner recon SSE).
        let mut d0_recon: Vec<u8> = Vec::new();
        let mut best_coeff_count = u32::MAX;

        for depth in 0..=cand_end_depth {
            // prev_depth_coeff_exit_th (1 at txs_level <=4; 100 at eff-M9
            // txs_level 5): skip a deeper depth when the best depth so far
            // kept fewer than the threshold's worth of non-zero coeffs.
            if best_coeff_count < cfg.txs_prev_depth_exit {
                continue;
            }
            // C tx geometry at this depth (tx_depth_to_tx_size /
            // tx_blocks_per_depth / the intra tx_org raster).
            let (txw, txh) = txb_dims_at_depth(w, h, depth);
            let cols = w / txw;
            let txbs = cols * (h / txh);
            // TX-local dc_sign/cul overlay (tx_reset_neighbor_arrays).
            let mut loc_above = fx.ectx.above_coeff_span(abs_x, w).to_vec();
            let mut loc_left = fx.ectx.left_coeff_span(abs_y, h).to_vec();
            let mut dep_bits: u64 = 0;
            let mut dep_dist: u64 = 0;
            let mut dep_q: Vec<Vec<i32>> = Vec::with_capacity(txbs);
            let mut dep_eob: Vec<u16> = Vec::with_capacity(txbs);
            let mut dep_cul: Vec<u8> = Vec::with_capacity(txbs);
            let mut dep_type: Vec<u8> = Vec::with_capacity(txbs);
            let mut dep_recon = vec![0u8; w * h];
            // This depth's assembled whole-block luma prediction (see
            // `best_pred`); mirrors what C leaves in `cand_bf->pred->y_buffer`.
            let mut dep_pred = vec![0u8; w * h];
            // Its 10-bit twin, assembled from the same per-txb predictions.
            let mut dep_pred10 = if bd10_rd.is_some() {
                vec![0u16; w * h]
            } else {
                Vec::new()
            };
            let mut dep_has_coeff = false;
            let mut aborted = false;
            // bd10 FULL-RD (task #94): the depth's 10-bit recon, which the
            // NEXT txb of a deeper depth predicts from (the same intra-block
            // sequential coupling the u8 `dep_recon` carries). `dep_dist` /
            // `dep_bits` above accumulate the 10-bit terms when active, so the
            // depth compare — and therefore tx_depth — is decided at bd10.
            let mut dep_recon10 = if bd10_rd.is_some() {
                vec![0u16; w * h]
            } else {
                Vec::new()
            };

            for txb in 0..txbs {
                let cand = &cands[ci];
                // Inter (IntraBC) txbs walk the C tx_org is_inter=1 rows
                // (z-order at depth 2); intra keeps the plain raster.
                let (tx_x, tx_y) = if cand.ibc.is_some() {
                    txb_org_inter(w, h, depth, txb)
                } else {
                    ((txb % cols) * txw, (txb / cols) * txh)
                };
                // Per-txb prediction: depth 0 reuses the MDS0 pred;
                // depth > 0 predicts from the live canvas (frame recon
                // outside the block, this depth's recon inside).
                let mut txb_pred = vec![0u8; txw * txh];
                if depth == 0 {
                    txb_pred.copy_from_slice(&cand.pred);
                } else if cand.palette.is_some() || cand.ibc.is_some() {
                    // Palette: position-only substitution. IntraBC: C
                    // computes the INTER residual once from the block-level
                    // prediction and never re-predicts per txb (the
                    // `if (!is_inter)` skip, product_coding_loop.c:5325) —
                    // a deeper-depth txb pred is the slice of the DV copy.
                    // Palette prediction is position-only substitution
                    // (enc_intra_prediction.c:640-651 runs per tx block
                    // over the SAME map — no neighbor edges), so a
                    // deeper-depth txb pred is just the slice of the
                    // whole-block substitution already in cand.pred.
                    for r in 0..txh {
                        let src0 = (tx_y + r) * w + tx_x;
                        txb_pred[r * txw..(r + 1) * txw]
                            .copy_from_slice(&cand.pred[src0..src0 + txw]);
                    }
                } else {
                    // Overlay canvas: temporarily splice this depth's
                    // reconstructed txbs into the frame recon.
                    predict_unit_overlay(
                        y_recon,
                        y_stride,
                        abs_x,
                        abs_y,
                        &dep_recon,
                        w,
                        h,
                        tx_x,
                        tx_y,
                        txw,
                        txh,
                        cand.mode,
                        cand.delta,
                        cand.fi,
                        &y_geom,
                        cfg.edge_filter,
                        filt_type_y,
                        &mut txb_pred,
                    );
                }
                // Accumulate this depth's whole-block prediction. At depth 0
                // txbs == 1, so this reproduces `cand.pred` exactly.
                for r in 0..txh {
                    let dst = (tx_y + r) * w + tx_x;
                    dep_pred[dst..dst + txw].copy_from_slice(&txb_pred[r * txw..(r + 1) * txw]);
                }
                // The SAME per-txb prediction at 10 bits, by the same three
                // rules: depth 0 reuses the MDS0 10-bit whole-block pred;
                // palette is position-only substitution (no neighbour edges),
                // so a deeper txb is a slice of it; otherwise predict from the
                // 10-bit overlay canvas.
                let mut txb_pred10: Vec<u16> = Vec::new();
                if bd10_rd.is_some() {
                    txb_pred10 = vec![0u16; txw * txh];
                    if depth == 0 {
                        txb_pred10.copy_from_slice(&cand.pred10);
                    } else if cand.palette.is_some() || cand.ibc.is_some() {
                        for r in 0..txh {
                            let src0 = (tx_y + r) * w + tx_x;
                            txb_pred10[r * txw..(r + 1) * txw]
                                .copy_from_slice(&cand.pred10[src0..src0 + txw]);
                        }
                    } else {
                        predict_unit_overlay_hbd(
                            fx.y_recon10.as_deref().unwrap(),
                            y_stride,
                            abs_x,
                            abs_y,
                            &dep_recon10,
                            w,
                            h,
                            tx_x,
                            tx_y,
                            txw,
                            txh,
                            cand.mode,
                            cand.delta,
                            cand.fi,
                            &y_geom,
                            cfg.edge_filter,
                            filt_type_y,
                            &mut txb_pred10,
                            frame.bit_depth,
                        );
                    }
                    // Accumulate this depth's whole-block 10-bit prediction,
                    // exactly as `dep_pred` does for u8 — C writes both
                    // through the same `cand_bf->pred->y_buffer`.
                    for r in 0..txh {
                        let dst = (tx_y + r) * w + tx_x;
                        dep_pred10[dst..dst + txw]
                            .copy_from_slice(&txb_pred10[r * txw..(r + 1) * txw]);
                    }
                }
                // Per-txb contexts from the TX-local overlay (real at M6;
                // 0/0 at M7/M8 where update_skip_ctx_dc_sign_ctx == 0, so
                // cul_level never accumulates — full_loop.c:1880).
                let (tsc, dsc) = if cfg.real_coeff_ctx {
                    txb_ctx_from_spans(&loc_above, &loc_left, tx_x, tx_y, txw, txh, depth == 0)
                } else {
                    (0, 0)
                };
                // TXT search over this txb. IntraBC txbs carry the
                // INTER_TXT_DIR sentinel: the inter ext-tx set + the
                // inter tx-type rate rows (tx_type_search is_inter).
                let intra_dir = if cand.ibc.is_some() {
                    INTER_TXT_DIR
                } else if cand.fi != FI_NONE {
                    FIMODE_TO_INTRADIR[cand.fi as usize] as usize
                } else {
                    cand.mode as usize
                };
                let bd10_txb = bd10_rd.as_ref().map(|b| Bd10Txb {
                    src10: &b.y_src10,
                    src10_stride: w,
                    src10_off: tx_y * w + tx_x,
                    pred10: &txb_pred10,
                    qt: &b.qt,
                    lambda: b.lambda,
                    bd: b.bd,
                });
                #[cfg(feature = "std")]
                let txt_dbg_tag = {
                    static XY: std::sync::OnceLock<Option<(usize, usize)>> =
                        std::sync::OnceLock::new();
                    (dbg_xy(&XY, "SVTAV1_TXT_XY") == Some((abs_x, abs_y))).then_some(TxtDbg {
                        abs_x,
                        abs_y,
                        tx_x,
                        tx_y,
                        mode: cand.mode,
                        fi: cand.fi,
                    })
                };
                #[cfg(not(feature = "std"))]
                let txt_dbg_tag = None;
                // C `cropped_tx_width`/`cropped_tx_height` for THIS txb
                // (product_coding_loop.c:4664-4665 / :5752-5754): the tx
                // origin is the block origin plus the txb offset, and the
                // bound is the ALIGNED frame extent. Identity on a
                // 64-aligned frame.
                let txb_crop = crate::frame_geom::cropped_tx_dims(
                    &aligned_dims,
                    abs_x + tx_x,
                    abs_y + tx_y,
                    txw,
                    txh,
                );
                let (out, out10, txt) = txt_search(
                    y_src,
                    y_src_stride,
                    y_src_off + tx_y * y_src_stride + tx_x,
                    &txb_pred,
                    txw,
                    txh,
                    txb_crop,
                    depth,
                    tsc,
                    dsc,
                    intra_dir,
                    &qt,
                    frame,
                    rates,
                    do_rdoq,
                    lambda,
                    bd10_txb.as_ref(),
                    txt_dbg_tag,
                    // R2: on this branch the exact coefficient rate is
                    // COMPUTED AND THEN OVERWRITTEN by the closed form below
                    // (`txb_bits`, :6230-ish). C never computes it — its rate
                    // tiers are an `if / else if / else` and only the taken arm
                    // runs (product_coding_loop.c:5540-5564). Producing the
                    // closed form inside `tx_unit` yields the SAME `bits`
                    // arithmetic, so this is not a deadness claim.
                    if cfg.coeff_rate_est_lvl == 0 && end_depth > 0 {
                        RateMode::Lvl0Closed
                    } else {
                        RateMode::Exact
                    },
                );
                // SVTAV1_QLEV_XY="x,y": per-txb winner (tx_type, eob, levels)
                // at one pinned block, to join against the C `--wrap
                // svt_aom_quantize_inv_quantize` QLEV dump.
                #[cfg(feature = "std")]
                if let Some(o) = &out10 {
                    static XY: std::sync::OnceLock<Option<(usize, usize)>> =
                        std::sync::OnceLock::new();
                    if dbg_xy(&XY, "SVTAV1_QLEV_XY") == Some((abs_x, abs_y)) {
                        let nz: alloc::vec::Vec<_> = o
                            .qcoeff
                            .iter()
                            .enumerate()
                            .filter(|&(_, &v)| v != 0)
                            .map(|(i, v)| alloc::format!("{i}:{v}"))
                            .collect();
                        eprintln!(
                            "PQLEV org=({abs_x},{abs_y}) d={depth} tx=({tx_x},{tx_y}) {txw}x{txh} txt={txt} eob={} nz=[{}]",
                            o.eob,
                            nz.join(",")
                        );
                    }
                }
                // The decision terms: 10-bit when the bd10 full-RD is active.
                let (dec_eob, dec_bits_raw, dec_dist, dec_cul) = match &out10 {
                    Some(o) => (o.eob, o.bits, o.dist, o.cul),
                    None => (out.eob, out.bits, out.dist, out.cul),
                };
                // eff-M9 (coeff_rate_est_lvl 0) prices the luma coeff RATE in
                // the RD compare with the fast per-txb approximation from C
                // `tx_type_search` (product_coding_loop.c:4976), NOT the real
                // cost_coeffs_txb: th = (txw*txh)>>6; eob<th ? 6000+eob*1000
                // : 3000+eob*100. The real bits still drove RDOQ/eob inside
                // `tx_unit` (unchanged). Gated on end_depth>0 == C's
                // perform_tx_partitioning path; end_depth==0 blocks go through
                // perform_dct_dct_tx and keep the funnel's estimate (their
                // single-candidate decision is rate-invariant).
                let txb_bits = if cfg.coeff_rate_est_lvl == 0 && end_depth > 0 {
                    let th = (txw * txh) >> 6;
                    if (dec_eob as usize) < th {
                        6000 + dec_eob as u64 * 1000
                    } else {
                        3000 + dec_eob as u64 * 100
                    }
                } else {
                    dec_bits_raw as u64
                };
                dep_bits += txb_bits;
                dep_dist += dec_dist;
                dep_has_coeff |= dec_eob > 0;
                // tx_update_neighbor_arrays: cul byte over the txb span. Clamp
                // the START to the span length (partial-SB straddle: an
                // off-frame txb's 4x4 origin exceeds the in-frame-clipped span)
                // so the range is empty rather than start>end. No in-frame cell
                // reads an off-frame txb's cul, so skipping the write matches C;
                // byte-neutral for every in-frame txb (start <= len).
                let a0 = (tx_x / 4).min(loc_above.len());
                let a1 = (a0 + txw / 4).min(loc_above.len());
                for v in loc_above[a0..a1].iter_mut() {
                    *v = dec_cul;
                }
                let l0 = (tx_y / 4).min(loc_left.len());
                let l1 = (l0 + txh / 4).min(loc_left.len());
                for v in loc_left[l0..l1].iter_mut() {
                    *v = dec_cul;
                }
                for r in 0..txh {
                    let dst = (tx_y + r) * w + tx_x;
                    dep_recon[dst..dst + txw].copy_from_slice(&out.recon[r * txw..(r + 1) * txw]);
                }
                if let Some(o) = &out10 {
                    for r in 0..txh {
                        let dst = (tx_y + r) * w + tx_x;
                        dep_recon10[dst..dst + txw]
                            .copy_from_slice(&o.recon[r * txw..(r + 1) * txw]);
                    }
                }

                // The CODED levels. With the bd10 full-RD active these come from
                // the 10-bit quantize/RDOQ — which is what C codes, and which
                // (unlike the level-only re-encode post-pass) carries this
                // txb's REAL txb_skip/dc_sign contexts into the trellis. Both
                // forms are the same packed (32-capped) pw*ph layout the
                // entropy walk re-expands (partition.rs funnel_block_decision).
                dep_q.push(match out10 {
                    Some(o) => o.qcoeff,
                    None => out.qcoeff,
                });
                dep_eob.push(dec_eob);
                dep_cul.push(dec_cul);
                dep_type.push(txt as u8);

                // C txb loop early exit: current accumulated cost already
                // above the best depth cost.
                if rdcost(lambda3, dep_bits, dep_dist) > best_cost {
                    aborted = true;
                    break;
                }
                // C quadrant early-abort (txs_ctrls.quadrant_th_sf,
                // product_coding_loop.c:5437): for a deeper depth, if the
                // accumulated cost (incl. this depth's full tx_size bits)
                // already exceeds its proportional share of the best depth
                // cost, drop the depth. `svt_aom_get_tx_size_bits` for intra
                // == the tx_size rate at (cat, ctx, depth) (skip/has-coeff
                // only gate the inter path).
                if cfg.txs_quadrant_sf != 0 && depth > 0 {
                    let normlized = ((txb as u64 + 1) * best_cost) / txbs as u64;
                    let tsb = if cands[ci].ibc.is_some() {
                        // Inert at the IBC presets (quadrant_sf == 0 at
                        // txs_level 2/3) — kept faithful to
                        // svt_aom_get_tx_size_bits' inter arm regardless.
                        if dep_has_coeff && block_signals_txsize(w, h) {
                            crate::vartx::tx_size_bits_vartx(
                                &rates.txfm_partition_fac_bits,
                                fx.ectx.txfm_above_span(abs_x, w),
                                fx.ectx.txfm_left_span(abs_y, h),
                                w,
                                h,
                                depth,
                                abs_y,
                                frame.frame_h_px,
                            )
                        } else {
                            0
                        }
                    } else {
                        rates.tx_size[tsz_cat][tsz_ctx][depth as usize] as u64
                    };
                    let cost_tmp = rdcost(lambda3, dep_bits + tsb, dep_dist);
                    if cost_tmp * 100 > normlized * cfg.txs_quadrant_sf {
                        aborted = true;
                        break;
                    }
                }
            }
            if aborted && depth > 0 {
                continue;
            }
            // C: 4x4 codes no tx_size symbol (block_signals_txsize == bsize > 4x4).
            // IntraBC (inter-classified): svt_aom_get_tx_size_bits prices the
            // var-tx walk when the depth kept coeffs, 0 bits when skip
            // (`!(is_inter_tx && skip)`).
            let tx_size_bits = if cands[ci].ibc.is_some() {
                if dep_has_coeff && block_signals_txsize(w, h) {
                    crate::vartx::tx_size_bits_vartx(
                        &rates.txfm_partition_fac_bits,
                        fx.ectx.txfm_above_span(abs_x, w),
                        fx.ectx.txfm_left_span(abs_y, h),
                        w,
                        h,
                        depth,
                        abs_y,
                        frame.frame_h_px,
                    )
                } else {
                    0
                }
            } else if block_signals_txsize(w, h) {
                rates.tx_size[tsz_cat][tsz_ctx][depth as usize] as u64
            } else {
                0
            };
            let cost = rdcost(lambda3, dep_bits + tx_size_bits, dep_dist);
            // SVTAV1_TXDEPTH_XY="x,y": per-tx_depth RD terms at one pinned
            // block ORIGIN — the port counterpart of the C
            // `perform_tx_partitioning` depth compare
            // (product_coding_loop.c:5425-5432), so a tx-depth flip can be
            // attributed to the coeff rate, the tx_size rate or the
            // distortion without re-deriving any of them.
            #[cfg(feature = "std")]
            {
                static XY: std::sync::OnceLock<Option<(usize, usize)>> = std::sync::OnceLock::new();
                if dbg_xy(&XY, "SVTAV1_TXDEPTH_XY") == Some((abs_x, abs_y)) {
                    eprintln!(
                        "PTXDEPTH org=({abs_x},{abs_y}) {w}x{h} d={depth} ibc={} mode={} ycb={dep_bits} txsz={tx_size_bits} dist={dep_dist} cost={cost} best={best_cost}",
                        u8::from(cands[ci].ibc.is_some()),
                        cands[ci].mode,
                    );
                }
            }
            // Depth 0 never aborts (the abort guard is `depth > 0`), so this
            // is always populated for every candidate that reaches MDS3.
            if depth == 0 {
                d0_recon = dep_recon.clone();
            }
            if cost < best_cost {
                best_cost = cost;
                best_depth = depth;
                best_bits = dep_bits;
                best_dist = dep_dist;
                best_txb_q = dep_q;
                best_txb_eob = dep_eob.clone();
                best_txb_cul = dep_cul;
                best_txb_type = dep_type;
                best_recon = dep_recon;
                best_recon10 = core::mem::take(&mut dep_recon10);
                best_pred = dep_pred;
                best_pred10 = core::mem::take(&mut dep_pred10);
                best_coeff_count = dep_eob.iter().map(|&e| e as u32).sum();
                let _ = dep_has_coeff;
            }
        }

        // ---- Chroma full loop (uv per candidate: follows-luma at
        //      CHROMA_MODE_1, or the ind-uv table pick at chroma_level 4)
        //      + the complexity detector (CFL gate; see below) ----
        //      Skipped entirely for non-chroma-ref blocks (C gates every
        //      chroma stage on ctx->has_uv).
        let cand = &cands[ci];
        // The INTER chroma tx type the IBC arm derives below, so the bd10 twin
        // can use the SAME one instead of re-deriving it from the intra rule.
        let mut ibc_uv_tt: Option<usize> = None;
        let (mut u_out, mut v_out) = if has_uv && let Some((dv, _)) = cand.ibc {
            // IBC chunk 7: IntraBC chroma — the DV copy / half-pel bilinear
            // from the chroma recon canvases (enc_inter_prediction chroma
            // arm, sf_identity), with the INTER chroma tx type rule: the
            // luma winner's txb-0 type when the chroma ext set allows it,
            // else DCT (tx_type_search, product_coding_loop.c:5087-5096).
            // No CfL, no ind-uv, no detector (all intra-only).
            let mut u_pred = vec![0u8; cw * chh];
            let mut v_pred = vec![0u8; cw * chh];
            let frame_ch = frame.frame_h_px / 2;
            crate::intrabc_pred::predict_intrabc_chroma(
                fx.u_recon,
                fx.c_stride,
                ccx,
                ccy,
                cw,
                chh,
                fx.c_stride,
                frame_ch,
                dv,
                &mut u_pred,
            );
            crate::intrabc_pred::predict_intrabc_chroma(
                fx.v_recon,
                fx.c_stride,
                ccx,
                ccy,
                cw,
                chh,
                fx.c_stride,
                frame_ch,
                dv,
                &mut v_pred,
            );
            let luma_tt = best_txb_type.first().copied().unwrap_or(0) as usize;
            let uv_tx = cc::adjusted_tx_size(cc::tx_size_from_dims(cw, chh));
            let uv_set = cc::ext_tx_set_type(uv_tx, true, false);
            let tt = if AV1_EXT_TX_USED[uv_set][luma_tt] != 0 {
                luma_tt
            } else {
                cc::DCT_DCT
            };
            let u_out = tx_unit(
                fx.u_src,
                fx.c_stride,
                ccy * fx.c_stride + ccx,
                &u_pred,
                cw,
                0,
                cw,
                chh,
                tt,
                1,
                cb_tsc,
                cb_dsc,
                0,
                &qt_u,
                frame,
                rates,
                do_rdoq,
                true,
                uv_crop,
                true,
                RateMode::Exact,
            );
            let v_out = tx_unit(
                fx.v_src,
                fx.c_stride,
                ccy * fx.c_stride + ccx,
                &v_pred,
                cw,
                0,
                cw,
                chh,
                tt,
                1,
                cr_tsc,
                cr_dsc,
                0,
                &qt_v,
                frame,
                rates,
                do_rdoq,
                true,
                uv_crop,
                true,
                RateMode::Exact,
            );
            ibc_uv_tt = Some(tt);
            (u_out, v_out)
        } else if has_uv {
            chroma::eval_uv(&cx, fx, cand.uv, cand.uv_delta)
        } else {
            (TxUnitOut::absent(), TxUnitOut::absent())
        };
        // bd10 chroma full loop — the decision terms for this candidate.
        let mut uv_out10 = match (&bd10_rd, has_uv) {
            (Some(b), true) => Some(match (cand.ibc, ibc_uv_tt) {
                // IBC: the DV copy at 10 bits, with the inter tx-type rule.
                (Some((dv, _)), Some(tt)) => chroma::eval_uv_ibc_hbd(&cx, fx, b, dv, tt),
                _ => chroma::eval_uv_hbd(&cx, fx, b, cand.uv, cand.uv_delta),
            }),
            // !has_uv: C runs NO chroma stage, so every chroma term is exactly
            // zero at either depth (TxUnitOut::absent()'s contract).
            _ => None,
        };
        // CfL override state, applied at the mutable-borrow writeback below.
        let mut uv_mode_final = cand.uv;
        let mut uv_delta_final = cand.uv_delta;
        let mut fcr_final = cand.fcr;
        let mut cfl_idx_final = 0u8;
        let mut cfl_signs_final = 0u8;
        // IntraBC candidates: no chroma detector, no CfL, no uv rewrite —
        // C excludes inter-classified candidates from every chroma search
        // (search_best_mds3_uv_mode :7335, the CfL arm :6932-equivalent).
        if has_uv && cand.ibc.is_none() {
            // Chroma complexity detector (chroma_complexity_check_pred,
            // product_coding_loop.c:6095), use_var=1: cfl_complexity ==
            // COMPONENT_CHROMA iff the SAD arm (cb/cr pred SAD > 2x luma
            // pred SAD) OR the variance arm (per-pixel source variance >
            // cplx_th) fires. Uses the candidate's uv PREDICTION.
            let mut u_pred = vec![0u8; cw * chh];
            let mut v_pred = vec![0u8; cw * chh];
            predict_unit(
                fx.u_recon,
                fx.c_stride,
                ccx,
                ccy,
                cw,
                chh,
                cand.uv,
                cand.uv_delta,
                FI_NONE,
                &uv_geom,
                cfg.edge_filter,
                filt_type_uv,
                &mut u_pred,
            );
            predict_unit(
                fx.v_recon,
                fx.c_stride,
                ccx,
                ccy,
                cw,
                chh,
                cand.uv,
                cand.uv_delta,
                FI_NONE,
                &uv_geom,
                cfg.edge_filter,
                filt_type_uv,
                &mut v_pred,
            );
            let c_off = ccy * fx.c_stride + ccx;
            // LUMA reference for the detector's SAD: C reads
            // `cand_buffer->pred->y_buffer` (product_coding_loop.c:6106), and
            // by the time the detector runs (:7178) the luma TX loop (:7139)
            // has already returned. What that leaves in the buffer depends on
            // the winning tx_depth:
            //   - depth 0: the TX loop re-predicts only `if (ctx->tx_depth)`
            //     (:5393-5395) and at depth 0 `tx_cand_bf == cand_bf`
            //     (:5363-5365), so the buffer still holds the MDS0 whole-block
            //     prediction == `cand.pred`.
            //   - depth > 0: each txb is re-predicted from RECON neighbours
            //     into a SEPARATE scratch buffer (`ctx->cand_bf_tx_depth_1/2`),
            //     and on winning, `update_tx_cand_bf` (:5269, called :5487)
            //     memcpy's that scratch pred back over the full
            //     bheight x bwidth of `cand_bf->pred->y_buffer`.
            // So the detector's luma SAD is against the WINNING DEPTH's
            // prediction, not the MDS0 one. Passing `cand.pred` here made the
            // port's `y_dist` diverge on every candidate whose winning depth
            // was > 0 (measured: 1040/7323 records on 258947 q40 p3, and zero
            // mismatches at depth 0), flipping `sad_arm` — and hence whether
            // CfL is evaluated at all — on 22 of them.
            // At bd10 C runs this SAD arm on the 10-bit source and the 10-bit
            // candidate prediction (:6048-6072), which does NOT reduce to the
            // u8 arm — see `chroma_detector_fires_hbd`. The chroma predictions
            // are the same (uv, uv_delta) pair `u_pred`/`v_pred` above, at 10
            // bits; the luma one is `best_pred10`, the winning depth's 10-bit
            // prediction.
            let sad_arm = match &bd10_rd {
                Some(b) => {
                    let mut u_p10d = vec![0u16; cw * chh];
                    let mut v_p10d = vec![0u16; cw * chh];
                    for (plane_recon, dst) in [
                        (fx.u_recon10.as_deref().unwrap(), &mut u_p10d),
                        (fx.v_recon10.as_deref().unwrap(), &mut v_p10d),
                    ] {
                        predict_unit_hbd(
                            plane_recon,
                            fx.c_stride,
                            ccx,
                            ccy,
                            cw,
                            chh,
                            cand.uv,
                            cand.uv_delta,
                            FI_NONE,
                            &uv_geom,
                            cfg.edge_filter,
                            filt_type_uv,
                            dst,
                            b.bd,
                        );
                    }
                    chroma_detector_fires_hbd(
                        y_src,
                        y_src_stride,
                        y_src_off,
                        &best_pred10,
                        w,
                        fx.u_src,
                        fx.v_src,
                        &u_p10d,
                        &v_p10d,
                        fx.c_stride,
                        c_off,
                        cw,
                        chh,
                        u32::from(b.bd - 8),
                    )
                }
                None => chroma_detector_fires(
                    y_src,
                    y_src_stride,
                    y_src_off,
                    &best_pred,
                    w,
                    fx.u_src,
                    fx.v_src,
                    &u_pred,
                    &v_pred,
                    fx.c_stride,
                    c_off,
                    cw,
                    chh,
                ),
            };
            // M6 cfl_level 4 -> cplx_th 10. Both detector arms use it: the
            // caller gates CfL on cfl_complexity == COMPONENT_CHROMA when
            // cplx_th != 0 (product_coding_loop.c:7183).
            let var_arm = cfg.cfl_cplx_th != 0
                && chroma_var_arm_fires(
                    fx.u_src,
                    fx.v_src,
                    fx.c_stride,
                    c_off,
                    cw,
                    chh,
                    cfg.cfl_cplx_th,
                );
            // cplx_th 0 (cfl_level 1/2, M0) BYPASSES the detector — CfL is
            // always evaluated (C :7183 `!cplx_th`); otherwise gate on either
            // detector arm (SAD 2x-luma or per-pixel variance > cplx_th).
            let cfl_would_run = cfg.cfl_cplx_th == 0 || sad_arm || var_arm;
            // Two CfL decision paths, both C `cfl_prediction`
            // (product_coding_loop.c:3795), gated identically on
            // `cfl_ctrls.enabled` + detector + intra + MDS3 + MAX(dims)<=32
            // (:7183-7193) — NO ind_uv gate there. They differ only in the
            // CfL-vs-non-CfL COMPARISON:
            //  - uv-follows-luma (!ind_uv_avail, M6): non_cfl_cost via
            //    full_loop_uv is_full_loop=0 (TRANSFORM domain) vs cfl_rd
            //    (transform) — the freq decision below.
            //  - independent-uv (ind_uv_avail, M0..M5): CfL forwarded, then
            //    `check_best_indepedant_cfl` (:3964, called :7237) compares
            //    `cfl_uv_cost` vs `best_uv_cost[mode]` — BOTH via full_loop_uv
            //    is_full_loop=1 (SPATIAL @ SSSE_MDS3 for allintra), the
            //    spatial decision in the else-if below.
            // C `ctx->ind_uv_avail` is PER-BLOCK RUNTIME state, not a preset
            // constant: it is reset to 0 for every block (:9931) and set to 1
            // only when the independent-uv search actually RUNS — gated at
            // :10165 on `uv_mode == CHROMA_MODE_0 && ind_uv_last_mds &&
            // sq_size < 128 && has_uv && perform_ind_uv_search_last_mds(...)`.
            // That predicate (:1470) counts MDS3 intra candidates as
            // `!is_inter && (!skip_ind_uv_if_only_dc || uv_mode != UV_DC_PRED)`
            // and returns `count > 0`; at M2..M5 (chroma_level 4,
            // enc_mode_config.c:5781) `skip_ind_uv_if_only_dc = 1`, so when
            // EVERY MDS3 candidate is UV_DC the search is skipped and
            // ind_uv_avail stays 0. C then reaches `if (cfl_performed) { if
            // (ctx->ind_uv_avail) check_best_indepedant_cfl(...) }` (:7258)
            // with a FALSE ind_uv_avail, so no `check_best_indepedant_cfl`
            // revert runs and CfL is decided by the uv-follows-luma
            // TRANSFORM-domain compare inside `cfl_prediction` instead of the
            // ind-uv SPATIAL compare. `ind_uv` above is Some iff that same
            // search ran (its `any(uv != 0)` gate IS
            // perform_ind_uv_search_last_mds for skip_ind_uv_if_only_dc = 1,
            // and the M0/M1 independent branch always runs) — so it is
            // exactly `ind_uv_avail`. Keying the two CfL paths off the preset
            // flags instead made the port take the SPATIAL path on the 263/7323
            // blocks where C has ind_uv_avail == 0, picking CfL where C keeps DC.
            let cfl_uv_follows = ind_uv.is_none();
            let cfl_ind_uv = ind_uv.is_some();
            // The uv-follows-luma arm below runs at BOTH depths (task: bd10
            // CfL). Under the bd10 full-RD the DECISION terms — the non-CfL
            // chroma cost, every per-alpha CfL cost, and hence the winning
            // alpha — are all computed at 10 bits (`cfl_predict_hbd` +
            // `tx_unit_hbd` + the bd10 quant tables + `full_lambda_md[
            // EB_10_BIT_MD]`), exactly as C does when `hbd_md != 0`. The u8
            // chroma buffers then FOLLOW that decision, which is the same
            // model the rest of the bd10 funnel uses (10-bit costs decide,
            // u8 buffers are carried for the pre-filter searches).
            //
            // The `cfl_ind_uv` arm (M0..M5) is still 8-bit only: its decision
            // is `check_best_indepedant_cfl`'s SPATIAL compare against
            // `best_uv_cost[mode]`, which needs the whole independent-uv
            // search at 10 bits, not just the CfL side. So it stays gated on
            // `bd10_rd.is_none()` below — at p0..p5 no bd10 leaf can be CfL,
            // which keeps `bd10_tree_supported` (widened to admit CfL) in
            // lockstep with what the search can actually produce.
            let cfl_gate = cfg.cfl_enabled && cfl_would_run && w <= 32 && h <= 32;
            if cfl_gate && cfl_uv_follows {
                // ---- cfl_prediction (product_coding_loop.c:3795) ----
                // non_cfl_cost = RDCOST(coeff_bits + uv fast rate, dist) over
                // the non-CFL chroma. C recomputes it with svt_aom_full_loop_uv
                // is_full_loop=0 -> TRANSFORM-domain distortion (product_coding
                // _loop.c:3800-3860), which is NOT the spatial SSE u_out/v_out
                // carry (those feed the final block RD). Re-run the non-CFL
                // chroma TX with spatial_dist=false to get the matching freq
                // distortion; coeffs/bits are unchanged by the dist domain so
                // the rate stays u_out/v_out.bits. cand.fcr is the uv fast rate
                // on the uv-follows-luma path.
                let nc_tt = uv_tx_type(cand.uv, cw, chh);
                let u_nc = tx_unit(
                    fx.u_src,
                    fx.c_stride,
                    c_off,
                    &u_pred,
                    cw,
                    0,
                    cw,
                    chh,
                    nc_tt,
                    1,
                    cb_tsc,
                    cb_dsc,
                    0,
                    &qt_u,
                    frame,
                    rates,
                    do_rdoq,
                    false,
                    uv_crop,
                    // R1: only `.dist` is read (the `non_cfl_cost` rdcost
                    // below takes its RATE from `u_out`/`v_out`). C's
                    // `cfl_prediction` recomputes this cost through
                    // `svt_aom_full_loop_uv` with `is_full_loop = 0`
                    // (product_coding_loop.c:3800-3860), which never enters
                    // the `is_full_loop && mds_do_spatial_sse` inverse
                    // transform at full_loop.c:2313.
                    false,
                    RateMode::Exact,
                );
                let v_nc = tx_unit(
                    fx.v_src,
                    fx.c_stride,
                    c_off,
                    &v_pred,
                    cw,
                    0,
                    cw,
                    chh,
                    nc_tt,
                    1,
                    cr_tsc,
                    cr_dsc,
                    0,
                    &qt_v,
                    frame,
                    rates,
                    do_rdoq,
                    false,
                    uv_crop,
                    // R1: only `.dist` is read (the `non_cfl_cost` rdcost
                    // below takes its RATE from `u_out`/`v_out`). C's
                    // `cfl_prediction` recomputes this cost through
                    // `svt_aom_full_loop_uv` with `is_full_loop = 0`
                    // (product_coding_loop.c:3800-3860), which never enters
                    // the `is_full_loop && mds_do_spatial_sse` inverse
                    // transform at full_loop.c:2313.
                    false,
                    RateMode::Exact,
                );
                let non_cfl_cost = rdcost(
                    lambda,
                    u_out.bits as u64 + v_out.bits as u64 + cand.fcr,
                    u_nc.dist + v_nc.dist,
                );
                // compute_cfl_ac_components: subsample the winning luma recon
                // (whole block, origin 0) and subtract its DC.
                let mut pred_buf_q3 = vec![0i16; svtav1_dsp::intra_pred::CFL_BUF_LINE * chh.max(1)];
                cfl_ac_subsample(
                    y_recon,
                    y_stride,
                    &best_recon,
                    abs_x,
                    abs_y,
                    w,
                    h,
                    &mut pred_buf_q3,
                );
                svtav1_dsp::intra_pred::cfl_subtract_average(&mut pred_buf_q3, cw, chh);
                // CfL base is the DC chroma prediction (C regenerates it when
                // the non-CFL uv mode != DC).
                let mut u_dc = vec![0u8; cw * chh];
                let mut v_dc = vec![0u8; cw * chh];
                predict_unit(
                    fx.u_recon,
                    fx.c_stride,
                    ccx,
                    ccy,
                    cw,
                    chh,
                    0,
                    0,
                    FI_NONE,
                    &uv_geom,
                    cfg.edge_filter,
                    filt_type_uv,
                    &mut u_dc,
                );
                predict_unit(
                    fx.v_recon,
                    fx.c_stride,
                    ccx,
                    ccy,
                    cw,
                    chh,
                    0,
                    0,
                    FI_NONE,
                    &uv_geom,
                    cfg.edge_filter,
                    filt_type_uv,
                    &mut v_dc,
                );
                // bd10 decision depth: the 10-bit AC luma (subsampled from the
                // 10-bit WINNING luma recon, C `compute_cfl_ac_components` at
                // `hbd_md != 0`) and the 10-bit DC chroma base. Hoisted out of
                // the compare below because the chosen-alpha chroma TX needs
                // them again once CfL wins.
                let cfl10: Option<(Vec<i16>, Vec<u16>, Vec<u16>)> = bd10_rd.as_ref().map(|b| {
                    let mut ac10 = vec![0i16; svtav1_dsp::intra_pred::CFL_BUF_LINE * chh.max(1)];
                    cfl_ac_subsample_hbd(
                        fx.y_recon10.as_deref().unwrap(),
                        y_stride,
                        &best_recon10,
                        abs_x,
                        abs_y,
                        w,
                        h,
                        &mut ac10,
                    );
                    svtav1_dsp::intra_pred::cfl_subtract_average(&mut ac10, cw, chh);
                    let mut u_dc10 = vec![0u16; cw * chh];
                    let mut v_dc10 = vec![0u16; cw * chh];
                    for (plane_recon, dst) in [
                        (fx.u_recon10.as_deref().unwrap(), &mut u_dc10),
                        (fx.v_recon10.as_deref().unwrap(), &mut v_dc10),
                    ] {
                        predict_unit_hbd(
                            plane_recon,
                            fx.c_stride,
                            ccx,
                            ccy,
                            cw,
                            chh,
                            0, // UV_DC_PRED — CfL's base
                            0,
                            FI_NONE,
                            &uv_geom,
                            cfg.edge_filter,
                            filt_type_uv,
                            dst,
                            b.bd,
                        );
                    }
                    (ac10, u_dc10, v_dc10)
                });
                // SVTAV1_UVDC: the bd10 CfL DC base, one line per (block,
                // candidate). Mirrors the C `--wrap svt_aom_full_loop_uv`
                // `pu=/pv=` readout (cand_bf->pred origin at the CfL-search
                // calls), which is the only externally observable handle on
                // C's chroma recon NEIGHBOUR state — `cfl_prediction` and
                // friends are static and cannot be wrapped. Constant per
                // (block, plane), so joining the two dumps on `org` bisects a
                // chroma neighbour-recon drift to its first divergent block.
                #[cfg(feature = "std")]
                if let Some((_, u_dc10, v_dc10)) = cfl10.as_ref() {
                    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
                    if dbg_on(&ON, "SVTAV1_UVDC") {
                        eprintln!(
                            "UVDC org=({abs_x},{abs_y}) {w}x{h} udc={} vdc={}",
                            u_dc10[0], v_dc10[0]
                        );
                    }
                }
                // The spatial-run chroma coeff bits at the decision depth — the
                // rate half of `non_cfl_cost`. Read out before the compare so
                // `uv_out10` is free to be replaced when CfL wins.
                let uv10_bits: u64 = uv_out10
                    .as_ref()
                    .map_or(0, |(u10, v10)| u10.bits as u64 + v10.bits as u64);
                // C `av1_cost_calc_cfl` for one component at hbd: CfL-predict
                // from the 10-bit DC base + AC luma, then TX/quant with the
                // bd10 table and take the TRANSFORM-domain distortion
                // (`svt_aom_full_loop_uv` is_full_loop=0).
                let plane_cost10 = |plane: usize, alpha_q3: i32| -> (u64, i32) {
                    let b = bd10_rd.as_ref().unwrap();
                    let (ac10, u_dc10, v_dc10) = cfl10.as_ref().unwrap();
                    let (src, dc, tsc, dsc, qt) = if plane == 0 {
                        (&b.u_src10, u_dc10, cb_tsc, cb_dsc, &b.qt_u)
                    } else {
                        (&b.v_src10, v_dc10, cr_tsc, cr_dsc, &b.qt_v)
                    };
                    let mut cfl_pred = vec![0u16; cw * chh];
                    svtav1_dsp::hbd::cfl_predict_hbd(
                        ac10,
                        dc,
                        cw,
                        &mut cfl_pred,
                        cw,
                        alpha_q3,
                        b.bd,
                        cw,
                        chh,
                    );
                    let o = tx_unit_hbd(
                        src,
                        cw,
                        0,
                        &cfl_pred,
                        cw,
                        0,
                        cw,
                        chh,
                        0,
                        1,
                        tsc,
                        dsc,
                        qt,
                        frame.rdoq_level,
                        b.lambda,
                        frame.sharpness,
                        rates,
                        do_rdoq,
                        b.bd,
                        qt.qm_level,
                        Some(&TxRdArgs {
                            spatial_dist: false,
                            intra_dir: 0,
                            coeff_rate_est_lvl: cfg.coeff_rate_est_lvl,
                            tx_bias: frame.tx_bias,
                            crop: uv_crop,
                        }),
                    );
                    (o.dist, o.bits)
                };
                // The alpha search AND the CfL-vs-non-CfL compare both run at
                // the decision depth. Mixing them (an 8-bit CfL cost against a
                // 10-bit non-CfL cost, or vice versa) is a ~16x scale error and
                // decides every block wrongly — which is why the two costs are
                // produced by the same `b.lambda` / bd10-quant pair here.
                let (cfl_idx, cfl_signs, cfl_rd, cfl_cmp_cost) = match &bd10_rd {
                    Some(b) => {
                        // non_cfl_cost at 10 bits: same expression as the u8 one
                        // above (spatial-run coeff bits + uv fast rate, against
                        // the freq-domain re-run's distortion).
                        let mut u_p10 = vec![0u16; cw * chh];
                        let mut v_p10 = vec![0u16; cw * chh];
                        for (plane_recon, dst) in [
                            (fx.u_recon10.as_deref().unwrap(), &mut u_p10),
                            (fx.v_recon10.as_deref().unwrap(), &mut v_p10),
                        ] {
                            predict_unit_hbd(
                                plane_recon,
                                fx.c_stride,
                                ccx,
                                ccy,
                                cw,
                                chh,
                                cand.uv,
                                cand.uv_delta,
                                FI_NONE,
                                &uv_geom,
                                cfg.edge_filter,
                                filt_type_uv,
                                dst,
                                b.bd,
                            );
                        }
                        let freq10 =
                            |src: &[u16], pred: &[u16], tsc: usize, dsc: usize, qt: &QuantTable| {
                                tx_unit_hbd(
                                    src,
                                    cw,
                                    0,
                                    pred,
                                    cw,
                                    0,
                                    cw,
                                    chh,
                                    nc_tt,
                                    1,
                                    tsc,
                                    dsc,
                                    qt,
                                    frame.rdoq_level,
                                    b.lambda,
                                    frame.sharpness,
                                    rates,
                                    do_rdoq,
                                    b.bd,
                                    qt.qm_level,
                                    Some(&TxRdArgs {
                                        spatial_dist: false,
                                        intra_dir: 0,
                                        coeff_rate_est_lvl: cfg.coeff_rate_est_lvl,
                                        tx_bias: frame.tx_bias,
                                        crop: uv_crop,
                                    }),
                                )
                            };
                        let u_nc10 = freq10(&b.u_src10, &u_p10, cb_tsc, cb_dsc, &b.qt_u);
                        let v_nc10 = freq10(&b.v_src10, &v_p10, cr_tsc, cr_dsc, &b.qt_v);
                        let nc10 =
                            rdcost(b.lambda, uv10_bits + cand.fcr, u_nc10.dist + v_nc10.dist);
                        let (i, s, rd) = md_cfl_alpha_search(
                            plane_cost10,
                            rates,
                            b.lambda,
                            cand.mode as usize,
                            cfg.cfl_itr_th,
                        );
                        (i, s, rd, nc10)
                    }
                    None => {
                        let (i, s, rd) = md_cfl_rd_pick_alpha(
                            &pred_buf_q3,
                            &u_dc,
                            &v_dc,
                            fx.u_src,
                            fx.v_src,
                            fx.c_stride,
                            c_off,
                            cw,
                            chh,
                            uv_crop,
                            cb_tsc,
                            cb_dsc,
                            cr_tsc,
                            cr_dsc,
                            &qt_u,
                            &qt_v,
                            frame,
                            rates,
                            do_rdoq,
                            lambda,
                            cand.mode as usize,
                            cfg.cfl_itr_th,
                        );
                        (i, s, rd, non_cfl_cost)
                    }
                };
                if cfl_rd != MAX_MODE_COST && cfl_rd < cfl_cmp_cost {
                    // CfL wins: redo chroma with the winning alpha (DCT_DCT)
                    // for the full TX path, and swap in the CFL mode + rate.
                    let alpha_cb = cfl_idx_to_alpha(cfl_idx, cfl_signs, 0);
                    let alpha_cr = cfl_idx_to_alpha(cfl_idx, cfl_signs, 1);
                    let mut u_cfl = vec![0u8; cw * chh];
                    let mut v_cfl = vec![0u8; cw * chh];
                    svtav1_dsp::intra_pred::cfl_predict_lbd(
                        &pred_buf_q3,
                        &u_dc,
                        cw,
                        &mut u_cfl,
                        cw,
                        alpha_cb,
                        cw,
                        chh,
                    );
                    svtav1_dsp::intra_pred::cfl_predict_lbd(
                        &pred_buf_q3,
                        &v_dc,
                        cw,
                        &mut v_cfl,
                        cw,
                        alpha_cr,
                        cw,
                        chh,
                    );
                    u_out = tx_unit(
                        fx.u_src,
                        fx.c_stride,
                        c_off,
                        &u_cfl,
                        cw,
                        0,
                        cw,
                        chh,
                        0,
                        1,
                        cb_tsc,
                        cb_dsc,
                        0,
                        &qt_u,
                        frame,
                        rates,
                        do_rdoq,
                        true,
                        uv_crop,
                        true,
                        RateMode::Exact,
                    );
                    v_out = tx_unit(
                        fx.v_src,
                        fx.c_stride,
                        c_off,
                        &v_cfl,
                        cw,
                        0,
                        cw,
                        chh,
                        0,
                        1,
                        cr_tsc,
                        cr_dsc,
                        0,
                        &qt_v,
                        frame,
                        rates,
                        do_rdoq,
                        true,
                        uv_crop,
                        true,
                        RateMode::Exact,
                    );
                    // bd10: the coded chroma at the decision depth. C runs the
                    // SAME chosen-alpha `svt_cfl_predict_hbd` + full TX here
                    // (cfl_prediction :3860-3878), so `uv_out10` — which is
                    // what the block cost, the coded levels and the neighbour
                    // culs are taken from at bd10 — must be rebuilt with the
                    // CfL prediction, not left on the non-CfL chroma.
                    if let (Some(b), Some((ac10, u_dc10, v_dc10))) = (&bd10_rd, &cfl10) {
                        let rd10 = TxRdArgs {
                            spatial_dist: true, // MDS3 chroma is the spatial SSE
                            intra_dir: 0,
                            coeff_rate_est_lvl: cfg.coeff_rate_est_lvl,
                            tx_bias: frame.tx_bias,
                            crop: uv_crop,
                        };
                        let mut u_cfl10 = vec![0u16; cw * chh];
                        let mut v_cfl10 = vec![0u16; cw * chh];
                        svtav1_dsp::hbd::cfl_predict_hbd(
                            ac10,
                            u_dc10,
                            cw,
                            &mut u_cfl10,
                            cw,
                            alpha_cb,
                            b.bd,
                            cw,
                            chh,
                        );
                        svtav1_dsp::hbd::cfl_predict_hbd(
                            ac10,
                            v_dc10,
                            cw,
                            &mut v_cfl10,
                            cw,
                            alpha_cr,
                            b.bd,
                            cw,
                            chh,
                        );
                        let u10 = tx_unit_hbd(
                            &b.u_src10,
                            cw,
                            0,
                            &u_cfl10,
                            cw,
                            0,
                            cw,
                            chh,
                            0,
                            1,
                            cb_tsc,
                            cb_dsc,
                            &b.qt_u,
                            frame.rdoq_level,
                            b.lambda,
                            frame.sharpness,
                            rates,
                            do_rdoq,
                            b.bd,
                            b.qt_u.qm_level,
                            Some(&rd10),
                        );
                        let v10 = tx_unit_hbd(
                            &b.v_src10,
                            cw,
                            0,
                            &v_cfl10,
                            cw,
                            0,
                            cw,
                            chh,
                            0,
                            1,
                            cr_tsc,
                            cr_dsc,
                            &b.qt_v,
                            frame.rdoq_level,
                            b.lambda,
                            frame.sharpness,
                            rates,
                            do_rdoq,
                            b.bd,
                            b.qt_v.qm_level,
                            Some(&rd10),
                        );
                        uv_out10 = Some((u10, v10));
                    }
                    uv_mode_final = UV_CFL_PRED_IDX as u8;
                    cfl_idx_final = cfl_idx;
                    cfl_signs_final = cfl_signs;
                    // Updated uv fast rate (get_intra_uv_fast_rate,
                    // use_accurate_cfl=1): UV_CFL_PRED mode bits + alpha bits.
                    fcr_final = rates.uv[cfl_allowed][cand.mode as usize][UV_CFL_PRED_IDX] as u64
                        + rates.cfl_alpha_fac_bits[cfl_signs as usize][0][(cfl_idx >> 4) as usize]
                            as u64
                        + rates.cfl_alpha_fac_bits[cfl_signs as usize][1][(cfl_idx & 15) as usize]
                            as u64;
                }
            } else if cfl_gate && cfl_ind_uv {
                // C independent-uv CfL: cfl_prediction (ind_uv_avail branch,
                // product_coding_loop.c:3888) forwards CfL, then
                // check_best_indepedant_cfl (:3830, called :6875) keeps the
                // non-CfL uv mode iff best_uv_cost[mode] < cfl_uv_cost —
                // where best_uv_cost/best_uv_mode are keyed on the CODED
                // luma mode (DC for FILTER candidates), NOT the candidate's
                // injected uv. At M0 (ind_uv_last_mds==0, no :7063
                // pre-rewrite) a FILTER candidate arrives here still
                // carrying tbl[fimode_to_intramode[fi]]; C discards that
                // eval entirely and arbitrates CfL against the coded-mode
                // row, assigning best_uv_mode[coded] on a non-CfL win. So:
                // re-key the candidate to the coded-mode row before the
                // compare (a no-op for M1/M2/M3, whose pre-MDS3 rewrite
                // already applied it). Both costs are SPATIAL SSE
                // (full_loop_uv is_full_loop=1 @ SSSE_MDS3), unlike the
                // uv-follows-luma freq decision above.
                let (arb_uv, arb_uvd) = ind_uv.as_ref().unwrap()[cand.mode as usize];
                if (cand.uv, cand.uv_delta) != (arb_uv, arb_uvd) {
                    let (u2, v2) = chroma::eval_uv(&cx, fx, arb_uv, arb_uvd);
                    u_out = u2;
                    v_out = v2;
                    // bd10: the 10-bit chroma decision terms follow the re-key
                    // (C re-runs the ind-uv-best chroma at hbd_md in
                    // check_best_indepedant_cfl :3957-3995). Only fires at M0
                    // (FILTER candidate, no :7063 pre-rewrite); the mds3 configs
                    // pre-rewrote so this branch is a no-op there.
                    if let Some(b) = bd10_rd.as_ref() {
                        uv_out10 = Some(chroma::eval_uv_hbd(&cx, fx, b, arb_uv, arb_uvd));
                    }
                    uv_mode_final = arb_uv;
                    uv_delta_final = arb_uvd;
                    let mut f = rates.uv[cfl_allowed][cand.mode as usize][arb_uv as usize] as u64;
                    if use_angle && matches!(arb_uv, 1..=8) {
                        f += rates.angle[arb_uv as usize - 1][(3 + arb_uvd) as usize] as u64;
                    }
                    if arb_uv == 0 {
                        f += pal_uv_no; // rd_cost.c:514 (inside uv fast rate)
                    }
                    fcr_final = f;
                }
                // compute_cfl_ac_components (u8): subsample the winning luma
                // recon; the DC chroma base. Shared by both depths — at bd10
                // the u8 chroma canvas still follows the CfL decision (carried
                // for the pre-filter searches), so it is rebuilt from these.
                let mut pred_buf_q3 = vec![0i16; svtav1_dsp::intra_pred::CFL_BUF_LINE * chh.max(1)];
                cfl_ac_subsample(
                    y_recon,
                    y_stride,
                    &best_recon,
                    abs_x,
                    abs_y,
                    w,
                    h,
                    &mut pred_buf_q3,
                );
                svtav1_dsp::intra_pred::cfl_subtract_average(&mut pred_buf_q3, cw, chh);
                // CfL base is the DC chroma prediction (C regenerates DC pred
                // when the non-CFL uv mode != DC — we always compute it fresh).
                let mut u_dc = vec![0u8; cw * chh];
                let mut v_dc = vec![0u8; cw * chh];
                predict_unit(
                    fx.u_recon,
                    fx.c_stride,
                    ccx,
                    ccy,
                    cw,
                    chh,
                    0,
                    0,
                    FI_NONE,
                    &uv_geom,
                    cfg.edge_filter,
                    filt_type_uv,
                    &mut u_dc,
                );
                predict_unit(
                    fx.v_recon,
                    fx.c_stride,
                    ccx,
                    ccy,
                    cw,
                    chh,
                    0,
                    0,
                    FI_NONE,
                    &uv_geom,
                    cfg.edge_filter,
                    filt_type_uv,
                    &mut v_dc,
                );
                // check_best_indepedant_cfl (product_coding_loop.c:3893): CfL vs
                // the best non-CfL uv, BOTH in the MDS3 SPATIAL SSE domain,
                // priced with `full_lambda_md[hbd_md ? EB_10_BIT_MD :
                // EB_8_BIT_MD]` (:3899). At bd10 C runs the whole arbitration at
                // 10 bits (hbd prediction / residual / full-loop). The port ran
                // it u8-only (this branch was `&& bd10_rd.is_none()`), so no
                // bd10 leaf below p6 could ever pick CfL while C does — the
                // block (16,80) divergence on 1001682 q12 p5.
                // The TABLE-side uv fast rate for the arbitration. C compares
                // against `ctx->best_uv_cost[mode]`, which the independent
                // chroma search built with `svt_aom_get_intra_uv_fast_rate(
                // pcs, ctx, cand_bf, 0)` over its OWN buffers
                // (product_coding_loop.c:7484) — candidates that carry
                // `palette_info == NULL`, so their UV_DC row is priced with
                // `palette_uv_mode_fac_bits[0][0]` (rd_cost.c:514-521).
                // `ind_palette_cost_diff` (:3912-3925) is precisely what
                // converts that [0] row to this candidate's [1] row.
                //
                // `fcr_final` is the CANDIDATE's fast_chroma_rate, and a
                // luma-palette candidate's already carries the [1] row (built
                // at :4596 — C does the same, get_intra_uv_fast_rate sees the
                // real palette_info). Feeding it in here AND adding
                // ind_pal_diff counts the [1]-[0] delta TWICE, which pushed
                // the non-CfL side above CfL on an otherwise-matching block.
                // Rebuild the palette-free row instead — the same expression
                // used for every non-palette candidate's fcr (:4261, :5668,
                // :6849), so this is a no-op wherever no luma palette is in
                // play.
                let fcr_ind = {
                    let mut f =
                        rates.uv[cfl_allowed][cand.mode as usize][uv_mode_final as usize] as u64;
                    if use_angle && matches!(uv_mode_final, 1..=8) {
                        f += rates.angle[uv_mode_final as usize - 1][(3 + uv_delta_final) as usize]
                            as u64;
                    }
                    if uv_mode_final == 0 {
                        f += pal_uv_no; // rd_cost.c:514 (inside uv fast rate)
                    }
                    f
                };
                match &bd10_rd {
                    None => {
                        let best_uv_cost = rdcost(
                            lambda,
                            u_out.bits as u64 + v_out.bits as u64 + fcr_ind,
                            u_out.dist + v_out.dist,
                        );
                        // Alpha search: md_cfl_rd_pick_alpha (transform domain,
                        // spatial_dist=false internally), same call as M6.
                        let (cfl_idx, cfl_signs, cfl_rd) = md_cfl_rd_pick_alpha(
                            &pred_buf_q3,
                            &u_dc,
                            &v_dc,
                            fx.u_src,
                            fx.v_src,
                            fx.c_stride,
                            c_off,
                            cw,
                            chh,
                            uv_crop,
                            cb_tsc,
                            cb_dsc,
                            cr_tsc,
                            cr_dsc,
                            &qt_u,
                            &qt_v,
                            frame,
                            rates,
                            do_rdoq,
                            lambda,
                            cand.mode as usize,
                            cfg.cfl_itr_th,
                        );
                        if cfl_rd != MAX_MODE_COST {
                            // cfl_uv_cost: the chosen-alpha CfL chroma TX in the
                            // MDS3 SPATIAL domain + the accurate CfL uv fast rate.
                            let alpha_cb = cfl_idx_to_alpha(cfl_idx, cfl_signs, 0);
                            let alpha_cr = cfl_idx_to_alpha(cfl_idx, cfl_signs, 1);
                            let mut u_cfl = vec![0u8; cw * chh];
                            let mut v_cfl = vec![0u8; cw * chh];
                            svtav1_dsp::intra_pred::cfl_predict_lbd(
                                &pred_buf_q3,
                                &u_dc,
                                cw,
                                &mut u_cfl,
                                cw,
                                alpha_cb,
                                cw,
                                chh,
                            );
                            svtav1_dsp::intra_pred::cfl_predict_lbd(
                                &pred_buf_q3,
                                &v_dc,
                                cw,
                                &mut v_cfl,
                                cw,
                                alpha_cr,
                                cw,
                                chh,
                            );
                            let u_cfl_out = tx_unit(
                                fx.u_src,
                                fx.c_stride,
                                c_off,
                                &u_cfl,
                                cw,
                                0,
                                cw,
                                chh,
                                0,
                                1,
                                cb_tsc,
                                cb_dsc,
                                0,
                                &qt_u,
                                frame,
                                rates,
                                do_rdoq,
                                true,
                                uv_crop,
                                true,
                                RateMode::Exact,
                            );
                            let v_cfl_out = tx_unit(
                                fx.v_src,
                                fx.c_stride,
                                c_off,
                                &v_cfl,
                                cw,
                                0,
                                cw,
                                chh,
                                0,
                                1,
                                cr_tsc,
                                cr_dsc,
                                0,
                                &qt_v,
                                frame,
                                rates,
                                do_rdoq,
                                true,
                                uv_crop,
                                true,
                                RateMode::Exact,
                            );
                            let cfl_fast_rate =
                                rates.uv[cfl_allowed][cand.mode as usize][UV_CFL_PRED_IDX] as u64
                                    + rates.cfl_alpha_fac_bits[cfl_signs as usize][0]
                                        [(cfl_idx >> 4) as usize]
                                        as u64
                                    + rates.cfl_alpha_fac_bits[cfl_signs as usize][1]
                                        [(cfl_idx & 15) as usize]
                                        as u64;
                            let cfl_uv_cost = rdcost(
                                lambda,
                                u_cfl_out.bits as u64 + v_cfl_out.bits as u64 + cfl_fast_rate,
                                u_cfl_out.dist + v_cfl_out.dist,
                            );
                            #[cfg(feature = "std")]
                            if crate::dbgenv::nsqdbg()
                                && crate::depth_refine::nsqdbg_here(abs_x, abs_y)
                            {
                                eprintln!(
                                    "NSQDBG CFLARB mi=({},{}) {}x{} m={} arb=({},{}) ncb={}+{}+{} ncd={}+{} nc={} cflrd={} idx={} sgn={} cb={}+{}+{} cd={}+{} cfl={} udc={} vdc={}",
                                    abs_y / 4,
                                    abs_x / 4,
                                    w,
                                    h,
                                    cand.mode,
                                    uv_mode_final,
                                    uv_delta_final,
                                    u_out.bits,
                                    v_out.bits,
                                    fcr_ind,
                                    u_out.dist,
                                    v_out.dist,
                                    best_uv_cost,
                                    cfl_rd,
                                    cfl_idx,
                                    cfl_signs,
                                    u_cfl_out.bits,
                                    v_cfl_out.bits,
                                    cfl_fast_rate,
                                    u_cfl_out.dist,
                                    v_cfl_out.dist,
                                    cfl_uv_cost,
                                    u_dc[0],
                                    v_dc[0]
                                );
                            }
                            // C `check_best_indepedant_cfl` reverts to non-CfL
                            // iff `best_uv_cost < cfl_uv_cost` (:3927-3928) —
                            // i.e. CfL is KEPT unless strictly beaten, so CfL
                            // wins exact ties (the bd10 arm below always had
                            // this right; the old `cfl < best` here kept
                            // non-CfL on ties — witnessed flipping CID22
                            // 5739122 q5 p0 at mi(31,80) 8x4, where both
                            // sides' terms are identical and nc == cfl ==
                            // 130518 exactly: C codes CfL, the port coded H).
                            //
                            // ind_palette_cost_diff (C :3849-3863): the ind-uv
                            // table priced its UV_DC row with palette_uv_mode
                            // _fac_bits[0][0] (its injected candidates carry
                            // palette_info = NULL), but THIS candidate's coded
                            // DC row pays the [use_palette_y=1][0] context —
                            // add the row delta to the table side of the
                            // compare for a luma-palette candidate. Witnessed:
                            // windows95_p4_q20 mi(40,32) 16x16 pal=6 — all
                            // four coeff/dist terms byte-match C (nc 6916533
                            // vs cfl 6916944) yet C codes CfL because its DC
                            // side carries +[1][0]-[0][0]; the port kept DC.
                            let ind_pal_diff: i64 =
                                if uv_mode_final == 0 && allow_pal && cand.palette.is_some() {
                                    rdcost(lambda, pal_uv_no_y1, 0) as i64
                                        - rdcost(lambda, pal_uv_no, 0) as i64
                                } else {
                                    0
                                };
                            let best_uv_adj =
                                (best_uv_cost as i64).saturating_add(ind_pal_diff) as u64;
                            // NOT `best_uv_adj >= cfl_uv_cost` (what clippy <=1.89's
                            // nonminimal_bool asks for; current stable no longer does):
                            // the C predicate at product_coding_loop.c:3928 is
                            // `best_uv_cost + ind_palette_cost_diff < cfl_uv_cost` =
                            // REVERT to non-CfL, and this arm is its negation. The tie
                            // case documented above turned on getting that inversion
                            // exactly right, so the shape stays visible.
                            #[allow(clippy::nonminimal_bool)]
                            if !(best_uv_adj < cfl_uv_cost) {
                                u_out = u_cfl_out;
                                v_out = v_cfl_out;
                                uv_mode_final = UV_CFL_PRED_IDX as u8;
                                cfl_idx_final = cfl_idx;
                                cfl_signs_final = cfl_signs;
                                fcr_final = cfl_fast_rate;
                            }
                        } else {
                            #[cfg(feature = "std")]
                            if crate::dbgenv::nsqdbg()
                                && crate::depth_refine::nsqdbg_here(abs_x, abs_y)
                            {
                                eprintln!(
                                    "NSQDBG CFLARB mi=({},{}) {}x{} m={} ALPHA-REJECT",
                                    abs_y / 4,
                                    abs_x / 4,
                                    w,
                                    h,
                                    cand.mode
                                );
                            }
                        }
                    }
                    Some(b) => {
                        // bd10 arbitration: 10-bit AC/DC, hbd alpha search, hbd
                        // SPATIAL cfl_uv_cost, all priced with `b.lambda` ==
                        // full_lambda_md[EB_10_BIT_MD]. `best_uv_cost` is the
                        // 10-bit non-CfL uv cost — the same value C's
                        // search_best_mds3_uv_mode stored in best_uv_cost[mode]
                        // (spatial SSE, from `uv_out10`); scope the borrow so
                        // `uv_out10` is free to be replaced on a CfL win.
                        let best_uv_cost = {
                            let (u10b, v10b) = uv_out10.as_ref().unwrap();
                            rdcost(
                                b.lambda,
                                // `fcr_ind`, not `fcr_final` — the table-side
                                // palette-free row; see the bd8 arm's note.
                                u10b.bits as u64 + v10b.bits as u64 + fcr_ind,
                                u10b.dist + v10b.dist,
                            )
                        };
                        // compute_cfl_ac_components at hbd: AC from the winning
                        // 10-bit luma recon + the 10-bit DC chroma base.
                        let mut ac10 =
                            vec![0i16; svtav1_dsp::intra_pred::CFL_BUF_LINE * chh.max(1)];
                        cfl_ac_subsample_hbd(
                            fx.y_recon10.as_deref().unwrap(),
                            y_stride,
                            &best_recon10,
                            abs_x,
                            abs_y,
                            w,
                            h,
                            &mut ac10,
                        );
                        svtav1_dsp::intra_pred::cfl_subtract_average(&mut ac10, cw, chh);
                        let mut u_dc10 = vec![0u16; cw * chh];
                        let mut v_dc10 = vec![0u16; cw * chh];
                        for (plane_recon, dst) in [
                            (fx.u_recon10.as_deref().unwrap(), &mut u_dc10),
                            (fx.v_recon10.as_deref().unwrap(), &mut v_dc10),
                        ] {
                            predict_unit_hbd(
                                plane_recon,
                                fx.c_stride,
                                ccx,
                                ccy,
                                cw,
                                chh,
                                0,
                                0,
                                FI_NONE,
                                &uv_geom,
                                cfg.edge_filter,
                                filt_type_uv,
                                dst,
                                b.bd,
                            );
                        }
                        // av1_cost_calc_cfl at hbd, TRANSFORM domain (is_full_
                        // loop=0) — the alpha search's per-plane cost.
                        let plane_cost10 = |plane: usize, alpha_q3: i32| -> (u64, i32) {
                            let (src, dc, tsc, dsc, qt) = if plane == 0 {
                                (&b.u_src10, &u_dc10, cb_tsc, cb_dsc, &b.qt_u)
                            } else {
                                (&b.v_src10, &v_dc10, cr_tsc, cr_dsc, &b.qt_v)
                            };
                            let mut cfl_pred = vec![0u16; cw * chh];
                            svtav1_dsp::hbd::cfl_predict_hbd(
                                &ac10,
                                dc,
                                cw,
                                &mut cfl_pred,
                                cw,
                                alpha_q3,
                                b.bd,
                                cw,
                                chh,
                            );
                            let o = tx_unit_hbd(
                                src,
                                cw,
                                0,
                                &cfl_pred,
                                cw,
                                0,
                                cw,
                                chh,
                                0,
                                1,
                                tsc,
                                dsc,
                                qt,
                                frame.rdoq_level,
                                b.lambda,
                                frame.sharpness,
                                rates,
                                do_rdoq,
                                b.bd,
                                qt.qm_level,
                                Some(&TxRdArgs {
                                    spatial_dist: false,
                                    intra_dir: 0,
                                    coeff_rate_est_lvl: cfg.coeff_rate_est_lvl,
                                    tx_bias: frame.tx_bias,
                                    crop: uv_crop,
                                }),
                            );
                            (o.dist, o.bits)
                        };
                        let (cfl_idx, cfl_signs, cfl_rd) = md_cfl_alpha_search(
                            plane_cost10,
                            rates,
                            b.lambda,
                            cand.mode as usize,
                            cfg.cfl_itr_th,
                        );
                        if cfl_rd != MAX_MODE_COST {
                            let alpha_cb = cfl_idx_to_alpha(cfl_idx, cfl_signs, 0);
                            let alpha_cr = cfl_idx_to_alpha(cfl_idx, cfl_signs, 1);
                            // cfl_uv_cost at 10 bits: the chosen-alpha CfL chroma
                            // re-run in the MDS3 SPATIAL domain (full_loop_uv
                            // is_full_loop=1), matching check_best_indepedant_cfl.
                            let rd10 = TxRdArgs {
                                spatial_dist: true,
                                intra_dir: 0,
                                coeff_rate_est_lvl: cfg.coeff_rate_est_lvl,
                                tx_bias: frame.tx_bias,
                                crop: uv_crop,
                            };
                            let mut u_cfl10 = vec![0u16; cw * chh];
                            let mut v_cfl10 = vec![0u16; cw * chh];
                            svtav1_dsp::hbd::cfl_predict_hbd(
                                &ac10,
                                &u_dc10,
                                cw,
                                &mut u_cfl10,
                                cw,
                                alpha_cb,
                                b.bd,
                                cw,
                                chh,
                            );
                            svtav1_dsp::hbd::cfl_predict_hbd(
                                &ac10,
                                &v_dc10,
                                cw,
                                &mut v_cfl10,
                                cw,
                                alpha_cr,
                                b.bd,
                                cw,
                                chh,
                            );
                            let u10 = tx_unit_hbd(
                                &b.u_src10,
                                cw,
                                0,
                                &u_cfl10,
                                cw,
                                0,
                                cw,
                                chh,
                                0,
                                1,
                                cb_tsc,
                                cb_dsc,
                                &b.qt_u,
                                frame.rdoq_level,
                                b.lambda,
                                frame.sharpness,
                                rates,
                                do_rdoq,
                                b.bd,
                                b.qt_u.qm_level,
                                Some(&rd10),
                            );
                            let v10 = tx_unit_hbd(
                                &b.v_src10,
                                cw,
                                0,
                                &v_cfl10,
                                cw,
                                0,
                                cw,
                                chh,
                                0,
                                1,
                                cr_tsc,
                                cr_dsc,
                                &b.qt_v,
                                frame.rdoq_level,
                                b.lambda,
                                frame.sharpness,
                                rates,
                                do_rdoq,
                                b.bd,
                                b.qt_v.qm_level,
                                Some(&rd10),
                            );
                            let cfl_fast_rate =
                                rates.uv[cfl_allowed][cand.mode as usize][UV_CFL_PRED_IDX] as u64
                                    + rates.cfl_alpha_fac_bits[cfl_signs as usize][0]
                                        [(cfl_idx >> 4) as usize]
                                        as u64
                                    + rates.cfl_alpha_fac_bits[cfl_signs as usize][1]
                                        [(cfl_idx & 15) as usize]
                                        as u64;
                            let cfl_uv_cost = rdcost(
                                b.lambda,
                                u10.bits as u64 + v10.bits as u64 + cfl_fast_rate,
                                u10.dist + v10.dist,
                            );
                            // C `check_best_indepedant_cfl` reverts to non-CfL iff
                            // `best_uv_cost < cfl_uv_cost` (:3927) — i.e. CfL is
                            // KEPT unless strictly beaten, so CfL wins exact ties.
                            // ind_palette_cost_diff (:3849-3863) — see the bd8
                            // arm above: a luma-palette candidate's DC row pays
                            // the [1][0] palette-flag context the table priced
                            // as [0][0]; priced with this arm's 10-bit lambda.
                            let ind_pal_diff: i64 =
                                if uv_mode_final == 0 && allow_pal && cand.palette.is_some() {
                                    rdcost(b.lambda, pal_uv_no_y1, 0) as i64
                                        - rdcost(b.lambda, pal_uv_no, 0) as i64
                                } else {
                                    0
                                };
                            let best_uv_adj =
                                (best_uv_cost as i64).saturating_add(ind_pal_diff) as u64;
                            // NOT `best_uv_adj >= cfl_uv_cost` (what clippy <=1.89's
                            // nonminimal_bool asks for; current stable no longer does):
                            // the C predicate at product_coding_loop.c:3928 is
                            // `best_uv_cost + ind_palette_cost_diff < cfl_uv_cost` =
                            // REVERT to non-CfL, and this arm is its negation. The tie
                            // case documented above turned on getting that inversion
                            // exactly right, so the shape stays visible.
                            #[allow(clippy::nonminimal_bool)]
                            if !(best_uv_adj < cfl_uv_cost) {
                                // u8 chroma canvas follows the decision (the
                                // pre-filter searches read it at bd10).
                                let mut u_cfl = vec![0u8; cw * chh];
                                let mut v_cfl = vec![0u8; cw * chh];
                                svtav1_dsp::intra_pred::cfl_predict_lbd(
                                    &pred_buf_q3,
                                    &u_dc,
                                    cw,
                                    &mut u_cfl,
                                    cw,
                                    alpha_cb,
                                    cw,
                                    chh,
                                );
                                svtav1_dsp::intra_pred::cfl_predict_lbd(
                                    &pred_buf_q3,
                                    &v_dc,
                                    cw,
                                    &mut v_cfl,
                                    cw,
                                    alpha_cr,
                                    cw,
                                    chh,
                                );
                                u_out = tx_unit(
                                    fx.u_src,
                                    fx.c_stride,
                                    c_off,
                                    &u_cfl,
                                    cw,
                                    0,
                                    cw,
                                    chh,
                                    0,
                                    1,
                                    cb_tsc,
                                    cb_dsc,
                                    0,
                                    &qt_u,
                                    frame,
                                    rates,
                                    do_rdoq,
                                    true,
                                    uv_crop,
                                    true,
                                    RateMode::Exact,
                                );
                                v_out = tx_unit(
                                    fx.v_src,
                                    fx.c_stride,
                                    c_off,
                                    &v_cfl,
                                    cw,
                                    0,
                                    cw,
                                    chh,
                                    0,
                                    1,
                                    cr_tsc,
                                    cr_dsc,
                                    0,
                                    &qt_v,
                                    frame,
                                    rates,
                                    do_rdoq,
                                    true,
                                    uv_crop,
                                    true,
                                    RateMode::Exact,
                                );
                                uv_out10 = Some((u10, v10));
                                uv_mode_final = UV_CFL_PRED_IDX as u8;
                                cfl_idx_final = cfl_idx;
                                cfl_signs_final = cfl_signs;
                                fcr_final = cfl_fast_rate;
                            }
                        }
                    }
                }
            }
        }

        // ---- svt_aom_full_cost (rd_cost.c:1357) ----
        // bd10 FULL-RD: the chroma eob/bits/dist that enter the block cost come
        // from the 10-bit chroma loop when it ran (the luma terms already do,
        // via `best_bits` / `best_dist`).
        let (uv_eob10, u_bits10, v_bits10, uv_dist10) = match &uv_out10 {
            Some((u, v)) => (
                (u.eob, v.eob),
                u.bits as u64,
                v.bits as u64,
                u.dist + v.dist,
            ),
            None => (
                (u_out.eob, v_out.eob),
                u_out.bits as u64,
                v_out.bits as u64,
                u_out.dist + v_out.dist,
            ),
        };
        let block_has_coeff = best_coeff_count > 0 || uv_eob10.0 > 0 || uv_eob10.1 > 0;
        // C: 4x4 codes no tx_size symbol (block_signals_txsize == bsize > 4x4).
        // IntraBC: svt_aom_full_cost prices non_skip_tx_size_bits = the
        // var-tx walk (block_has_coeff) and skip_tx_size_bits = 0
        // (rd_cost.c:1367-1377 + the `!(is_inter_tx && skip)` gate).
        let tx_size_bits_final = if cand.ibc.is_some() {
            if block_has_coeff && block_signals_txsize(w, h) {
                crate::vartx::tx_size_bits_vartx(
                    &rates.txfm_partition_fac_bits,
                    fx.ectx.txfm_above_span(abs_x, w),
                    fx.ectx.txfm_left_span(abs_y, h),
                    w,
                    h,
                    best_depth,
                    abs_y,
                    frame.frame_h_px,
                )
            } else {
                0
            }
        } else if block_signals_txsize(w, h) {
            rates.tx_size[tsz_cat][tsz_ctx][best_depth as usize] as u64
        } else {
            0
        };
        // Chroma coeff rate. M6 (coeff_rate_est_lvl 1) prices the real
        // cost_coeffs_txb / cost_skip_txb (already in u_out.bits/v_out.bits):
        // C `skip_chroma_rate_est` returns false immediately at lvl 1, so the
        // caller runs the full estimate into a zeroed accumulator — clean.
        //
        // M7/M8 (lvl 2) + eff-M9 (lvl 0) go through C `skip_chroma_rate_est`
        // (full_loop.c:1922, th = (tx_w_uv * tx_h_uv) >> 6) — which we must
        // replicate byte-for-byte INCLUDING an order-dependent CB double-count.
        // skip_chroma_rate_est writes the CB approximation STRAIGHT INTO the
        // `*cb_coeff_bits` accumulator when `cb_eob < th`, then (lvl 2)
        // `return false` at the CR check when `cr_eob >= th` WITHOUT clearing
        // the CB write; the caller (svt_aom_full_loop_uv, full_loop.c:2636-2661)
        // then does `*cb_coeff_bits += cb_txb_coeff_bits` (the full estimate).
        // So in the `cb_eob < th && cr_eob >= th` case ONLY, CB is priced as
        // approx + full. CR never double-counts (CB is checked first; a `>= th`
        // CB `return false`s before the CR branch writes anything). At lvl 0 the
        // function never returns false — each plane gets `1500+eob*50` for
        // eob >= th — so it stays a clean per-plane approximation.
        // Instrumented C 2026-07-15: SB(224,192) q40 p7 H_PRED chroma
        // cb = 4500 approx + 6246 full = 10746, cr = 12848 (DC candidate cb
        // clean: cb_eob=6 >= th so CB returns before leaking). Pricing CB
        // clean (6246) undercharged the H candidate ~4500 and flipped the
        // leaf y_mode from C's DC to our H.
        let (u_bits, v_bits) = if cfg.real_coeff_ctx {
            (u_bits10, v_bits10)
        } else {
            let lvl = cfg.coeff_rate_est_lvl;
            let th = ((cw * chh) >> 6) as u16;
            let approx = |eob: u16| -> u64 {
                if eob == 0 {
                    0
                } else if eob < th {
                    3000 + eob as u64 * 500
                } else {
                    1500 + eob as u64 * 50 // lvl-0 `eob >= th` fallback
                }
            };
            let mut cb_leak = 0u64;
            let mut cr_leak = 0u64;
            let mut need_full = false;
            // CB branch of skip_chroma_rate_est (checked first).
            if uv_eob10.0 < th || lvl == 0 {
                cb_leak = approx(uv_eob10.0);
            } else {
                need_full = true; // lvl-2, cb_eob >= th -> return false (nothing leaked)
            }
            // CR branch — only reached when CB didn't already force full.
            if !need_full {
                if uv_eob10.1 < th || lvl == 0 {
                    cr_leak = approx(uv_eob10.1);
                } else {
                    need_full = true; // lvl-2, cr_eob >= th -> return false (CB leak stays)
                }
            }
            if need_full {
                // Caller runs the full estimate and ADDS it to the accumulator.
                (cb_leak + u_bits10, cr_leak + v_bits10)
            } else {
                (cb_leak, cr_leak)
            }
        };
        let coeff_rate = if block_has_coeff {
            best_bits + u_bits + v_bits + tx_size_bits_final + rates.skip[skip_ctx][0] as u64
        } else {
            rates.skip[skip_ctx][1] as u64 + tx_size_bits_final
        };
        let dist = best_dist + uv_dist10;
        // fcr_final == cand.fcr unless CfL was selected above (then the
        // UV_CFL_PRED mode + alpha rate replaces the non-CFL uv fast rate).
        let full = rdcost(lambda3, cand.flr + fcr_final + coeff_rate, dist);
        #[cfg(feature = "std")]
        if crate::dbgenv::canddbg() && crate::depth_refine::nsqdbg_here(abs_x, abs_y) {
            eprintln!(
                "NSQDBG CAND mi=({},{}) {}x{} ci={} mode={} fi={} delta={} uv={} ibc={} txd={} enddepth={} flr={} fcr={} coeff_rate={} dist={} full={}",
                abs_y / 4,
                abs_x / 4,
                w,
                h,
                ci,
                cand.mode,
                cand.fi,
                cand.delta,
                uv_mode_final,
                u8::from(cand.ibc.is_some()),
                best_depth,
                cand_end_depth,
                cand.flr,
                fcr_final,
                coeff_rate,
                dist,
                full,
            );
        }

        let cand = &mut cands[ci];
        cand.mds3_cost = full;
        cand.total_rate = cand.flr + fcr_final + coeff_rate;
        cand.full_dist = dist;
        cand.uv = uv_mode_final;
        cand.uv_delta = uv_delta_final;
        cand.fcr = fcr_final;
        cand.cfl_alpha_idx = cfl_idx_final;
        cand.cfl_alpha_signs = cfl_signs_final;
        cand.tx_depth = best_depth;
        cand.txb_q = best_txb_q;
        cand.txb_eob = best_txb_eob;
        cand.txb_cul = best_txb_cul;
        cand.txb_type = best_txb_type;
        cand.y_recon = best_recon;
        cand.y_recon_d0 = d0_recon;
        cand.y_bits = best_bits;
        cand.y_dist = best_dist;
        // Chroma coded levels / eobs / neighbour culs — 10-bit when the bd10
        // chroma full loop ran, for the same reason as luma above.
        match &uv_out10 {
            Some((u10, v10)) => {
                cand.u_q = u10.qcoeff.clone();
                cand.v_q = v10.qcoeff.clone();
                cand.u_eob = u10.eob;
                cand.v_eob = v10.eob;
                cand.u_cul = u10.cul;
                cand.v_cul = v10.cul;
                // The stored u8 chroma recon must REPRESENT the coded levels,
                // because the post-filter searches (CDEF / Wiener-LR) read it.
                // At bd10 the true recon is 10-bit and those searches are still
                // 8-bit (the open FH axis), so the u8 proxy is the truncated
                // 10-bit recon — exactly the convention the level-only chroma
                // re-encode post-pass established (`bd10_reencode_chroma_plane`
                // returns `recon10 >> (bd - 8)` and overwrites chroma_dec with
                // it). Keeping the u8-quantizer recon here instead would leave
                // the recon inconsistent with the levels actually coded.
                let sh = (frame.bit_depth - 8) as u32;
                cand.u_recon = u10
                    .recon
                    .iter()
                    .map(|&s| (s >> sh).min(255) as u8)
                    .collect();
                cand.v_recon = v10
                    .recon
                    .iter()
                    .map(|&s| (s >> sh).min(255) as u8)
                    .collect();
            }
            None => {
                cand.u_q = u_out.qcoeff;
                cand.v_q = v_out.qcoeff;
                cand.u_eob = u_out.eob;
                cand.v_eob = v_out.eob;
                cand.u_cul = u_out.cul;
                cand.v_cul = v_out.cul;
                // bd8 (and any bd10 leaf whose chroma loop did not run): the
                // u8-quantizer recon IS the coded recon.
                //
                // This pair USED TO SIT AFTER the match, unconditionally — which
                // silently overwrote the Some-arm's truncated-10-bit assignment
                // above and made that whole branch (and its justification
                // comment) dead code. The consequence was not local: `u_recon` /
                // `v_recon` feed the entropy-walk chroma plane (pipeline.rs), the
                // frame u8 chroma canvas that the CDEF / Wiener-LR / deblock
                // searches read (`commit_leaf`), and the NSQ quad-dist gate. So a
                // bd10 full-RD frame ran all of those against a recon that did
                // not correspond to the levels it actually coded. The chroma
                // re-encode post-pass that would have repaired it does not run
                // here either — `bd10_postpass_runs = !bd10_full_rd`.
                cand.u_recon = u_out.recon;
                cand.v_recon = v_out.recon;
            }
        }
        if let Some((u10, v10)) = uv_out10.take() {
            cand.u_recon10 = u10.recon;
            cand.v_recon10 = v10.recon;
        }
        cand.y_recon10 = core::mem::take(&mut best_recon10);
        cand.block_has_coeff = block_has_coeff;
        // [SVT_HDR_MODE] alt-ssim-tuning: the parallel SSIM full cost —
        // same lambda and total rate, block-SSIM distortion on the FINAL
        // per-plane recon (C accumulates DIST_SSIM per txb with cropped
        // dims; whole-block equals the per-txb sum whenever the 8x8/4x4
        // tiling aligns with txb boundaries, which holds for the funnel's
        // square/half tx shapes).
        // PORT-NOTE(unverified): fork alt-ssim full_cost_ssim vs C — needs
        // a C-side MD dump with alt_ssim_tuning=1 (tune_ssim_level LVL_1).
        if frame.tune_ssim {
            let cand = &cands[ci];
            let mut ssim_dist = crate::ssim_md::spatial_full_distortion_ssim(
                y_src,
                y_src_off,
                y_src_stride,
                &cand.y_recon,
                0,
                w,
                w,
                h,
                frame.ac_bias_eff,
            );
            if !cand.u_recon.is_empty() {
                let c_off = ccy * fx.c_stride + ccx;
                ssim_dist += crate::ssim_md::spatial_full_distortion_ssim(
                    fx.u_src,
                    c_off,
                    fx.c_stride,
                    &cand.u_recon,
                    0,
                    cw,
                    cw,
                    chh,
                    frame.ac_bias_eff,
                );
                ssim_dist += crate::ssim_md::spatial_full_distortion_ssim(
                    fx.v_src,
                    c_off,
                    fx.c_stride,
                    &cand.v_recon,
                    0,
                    cw,
                    cw,
                    chh,
                    frame.ac_bias_eff,
                );
            }
            let total_rate = cand.total_rate;
            cands[ci].mds3_cost_ssim = rdcost(lambda, total_rate, ssim_dist);
        }
    }

    // -- svt_aom_product_full_mode_decision: lowest cost, first wins --
    let mut win = order1[0];
    let mut win_cost = cands[order1[0]].mds3_cost;
    for &ci in order1.iter().take(n3).skip(1) {
        if cands[ci].mds3_cost < win_cost {
            win_cost = cands[ci].mds3_cost;
            win = ci;
        }
    }
    // [SVT_HDR_MODE] alt-ssim-tuning pass two (mode_decision.c:3892-3915):
    // among candidates whose SSD cost is within threshold x best, pick the
    // lowest SSIM cost (ties -> lower SSD cost).
    if frame.tune_ssim {
        let ssd_cost_threshold = (frame.tune_ssim_threshold * win_cost as f64) as u64;
        let mut ssim_lowest = u64::MAX;
        let mut ssd_at_win = win_cost;
        for &ci in order1.iter().take(n3) {
            let ssim_cost = cands[ci].mds3_cost_ssim;
            let ssd_cost = cands[ci].mds3_cost;
            if ssim_cost < ssim_lowest {
                if ssd_cost <= ssd_cost_threshold {
                    win = ci;
                    ssim_lowest = ssim_cost;
                    ssd_at_win = ssd_cost;
                }
            } else if ssim_cost == ssim_lowest && ssd_cost < ssd_at_win {
                win = ci;
                ssd_at_win = ssd_cost;
            }
        }
    }
    // The shared MDS3 residual workspace after the loop: the LAST
    // processed candidate's (order1[n3-1]) whole-block depth-0 residual.
    let mut psq_resid = vec![0i32; w * h];
    // bd10 twin (task #94, root #2): the SAME last-candidate residual at TRUE
    // 10 bits (`src10 - last.pred10`), consumed by `min_nz_hv` at bd10. Built
    // only when the last candidate carries a 10-bit prediction (== bd10 funnel
    // active); empty on the u8 path, so bd8 stays byte-identical.
    let mut psq_resid10: Vec<i32> = Vec::new();
    {
        let last = &cands[order1[n3 - 1]];
        for r in 0..h {
            let srow = y_src_off + r * y_src_stride;
            for c in 0..w {
                psq_resid[r * w + c] = y_src[srow + c] as i32 - last.pred[r * w + c] as i32;
            }
        }
        if !last.pred10.is_empty() {
            // Task #6 chunk 1: `blk_y_src10` is the real u16 source on a
            // native-HBD encode, and the same `u8 << (bd - 8)` widening this
            // loop did inline otherwise.
            debug_assert_eq!(blk_y_src10.len(), w * h);
            psq_resid10 = vec![0i32; w * h];
            for i in 0..w * h {
                psq_resid10[i] = blk_y_src10[i] as i32 - last.pred10[i] as i32;
            }
        }
    }

    // The shared cand_bf->recon state at gate time (see the gate_y field
    // doc): winner rebuild at bypass=0; last MDS3 candidate's depth-0 luma
    // + chroma at bypass=1. Proven on 1147124 q20 p4 (76,96): C's fill luma
    // quads sum to its OWN depth-0 dist (971<<4 == 15536), not the winning
    // depth-1 recon's (744<<4).
    let (gate_y, gate_u, gate_v) = if cfg.bypass_encdec {
        let last = &cands[order1[n3 - 1]];
        (
            last.y_recon_d0.clone(),
            last.u_recon.clone(),
            last.v_recon.clone(),
        )
    } else {
        let wc = &cands[win];
        (wc.y_recon.clone(), wc.u_recon.clone(), wc.v_recon.clone())
    };

    // bd10 mode funnel (task #94): reconstruct the winner at TRUE 10-bit for
    // the next block's neighbour prediction (`commit_leaf` writes this into the
    // canvas). Mirrors the post-pass `bd10_reencode_node` leaf body
    // (predict_unit_hbd + tx_unit_hbd, bd10 quant table + full bd10 lambda +
    // the frame RDOQ level), so the canvas == C's true bd10 recon and the
    // post-pass (which recomputes the coded LEVELS from these bd10 modes)
    // produces the same recon. eff-M9 winners are DC-family / tx_depth 0 / DCT
    // (no directional/fi/CfL — angular_level 4, filter_intra off), all handled
    // by predict_unit_hbd + tx_unit_hbd. Empty on the u8 path.
    // With the bd10 FULL-RD active the winner's 10-bit recon already exists —
    // it is the winning tx DEPTH's recon from the MDS3 loop, so it is correct
    // for tx_depth > 0 too. The re-predict below is the MDS0-only (eff-M9)
    // path, which is depth-0 by construction there.
    if !cands[win].y_recon10.is_empty() {
        let wr = core::mem::take(&mut cands[win].y_recon10);
        let (wu, wv) = (
            core::mem::take(&mut cands[win].u_recon10),
            core::mem::take(&mut cands[win].v_recon10),
        );
        return LeafEval {
            abs_x,
            abs_y,
            w,
            h,
            has_uv,
            ccx,
            ccy,
            cw,
            chh,
            win: cands.swap_remove(win),
            gate_y,
            gate_u,
            gate_v,
            psq_resid,
            psq_resid10,
            win_recon10: wr,
            win_u_recon10: wu,
            win_v_recon10: wv,
        };
    }
    let win_recon10 = match fx.y_recon10.as_deref() {
        Some(canvas10) => {
            let wc = &cands[win];
            let mut pred10 = vec![0u16; w * h];
            predict_unit_hbd(
                canvas10,
                y_stride,
                abs_x,
                abs_y,
                w,
                h,
                wc.mode,
                wc.delta,
                wc.fi,
                &y_geom,
                cfg.edge_filter,
                filt_type_y,
                &mut pred10,
                frame.bit_depth,
            );
            // Task #6 chunk 1: real u16 source on a native-HBD encode; the
            // identical `u8 << 2` widening this site did inline otherwise.
            let blk_src10 = blk_y_src10.clone();
            let tx_type = wc.txb_type.first().copied().unwrap_or(0) as usize;
            let qt10 = crate::quant::build_quant_table_bd(frame.base_qindex, frame.bit_depth);
            let out = tx_unit_hbd(
                &blk_src10,
                w,
                0,
                &pred10,
                w,
                0,
                w,
                h,
                tx_type,
                0,
                0,
                0,
                &qt10,
                frame.rdoq_level,
                lambda_bd10_full,
                0,
                rates,
                frame.rdoq_level != 0,
                frame.bit_depth,
                frame.qm_levels[0],
                None,
            );
            out.recon
        }
        None => Vec::new(),
    };

    LeafEval {
        abs_x,
        abs_y,
        w,
        h,
        has_uv,
        ccx,
        ccy,
        cw,
        chh,
        win: cands.swap_remove(win),
        gate_y,
        gate_u,
        gate_v,
        psq_resid,
        psq_resid10,
        win_recon10,
        win_u_recon10: Vec::new(),
        win_v_recon10: Vec::new(),
    }
}
