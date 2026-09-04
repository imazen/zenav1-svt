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

use crate::entropy::coeff_c as cc;
use crate::entropy::context::FrameContext;

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
// The 2026-08-25 round moved out everything that is NOT the funnel walk -- the
// data model (`types`), the CfL machinery (`cfl`), the commit step (`commit`),
// tx geometry (`tx_geom`), the depth > 0 predictor (`overlay`), the tx-type
// search (`txt`), the chroma detector (`detect`), the unit tests (`tests`) --
// and then split the walk itself into its stages: `inject` (candidate
// injection + the MDS0 fast loop), `nic` (the per-class staging between every
// pair of MD stages), `chroma` (one full-loop chroma evaluation, shared by
// injection and MDS3), and `mds3` (the independent-uv search + the last full
// loop).
//
// What is left HERE is the walk's spine: `evaluate_leaf` derives the per-leaf
// carriers (`LeafGeom`, `ChromaCtx`, `LeafBd10`, `PalFlagRates`), calls the
// stages in order -- `inject` -> `nic` -> `mds1` -> `nic` -> `mds3` -- and
// picks the winner. Reading it top to bottom is reading C's `md_encode_block`
// staging, which is what this file should have been all along.

use crate::quant::TX_SCALE_TAB;

mod cfl;
mod chroma;
mod coeff_rate;
mod commit;
mod detect;
mod ifs;
mod inject;
mod mds1;
mod mds3;
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
    // C `ctx->is_inter_ctx` (`svt_av1_get_intra_inter_context`,
    // entropy_coding.c:1127) over the MD mode-info grid's neighbour pair.
    // Zero on a KEY frame, where the grid is absent and no `intra_inter`
    // symbol exists — which is what keeps the still path byte-neutral.
    let is_inter_ctx = match fx.inter {
        Some(im) => {
            let nb = crate::inter_md_arm::neighbors_from_grid(
                fx.ibc_mvp
                    .as_deref()
                    .expect("the MD mi grid is allocated whenever the inter arm is armed"),
                im.mi_cols,
                (abs_y / 4) as i32,
                (abs_x / 4) as i32,
                im.tile,
            );
            // `port_entropy_inter`'s transcription, which handles
            // AVAILABILITY — see the same change in `inject.rs`. Collapsing
            // "not available" into "intra" and reading an inverted table
            // cost 1207 rate units per inter candidate, measured against C.
            crate::port_entropy_inter::intra_inter_context(&nb)
        }
        None => 0,
    };
    let skip_ctx = if fx.frame.cfg.real_coeff_ctx {
        fx.ectx.skip_ctx(abs_x, abs_y)
    } else {
        0
    };
    let fi_allowed_bsize = w <= 32 && h <= 32;
    let bsize_idx = crate::entropy::context::block_size_index(w, h);
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
    let allow_pal = crate::entropy::context::allow_palette(cfg.allow_sct, w, h);
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
            allow: allow_pal,
            mode_ctx,
            y_no: if allow_pal {
                rates.palette_y_no[crate::entropy::context::palette_bsize_ctx(w, h)][mode_ctx]
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

    // The independent-uv table. Written by whichever stage C builds it in --
    // injection when `ind_uv_last_mds == 0` (M0/M1, so every candidate's fast
    // cost prices its final uv pair), MDS3 otherwise -- hence the `&mut`
    // threading rather than a return value from either.
    let mut ind_uv: Option<[(u8, i8); 13]> = None;

    // The per-leaf carriers the stages read. Each is derived once here and is
    // constant for the leaf; see their docs in [`types`] and [`chroma`].
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
        is_inter_ctx,
        skip_ctx,
        blk_crop,
        aligned_dims,
    };
    let bd10 = LeafBd10 {
        active: bd10_funnel,
        blk_y_src10: &blk_y_src10,
        lambda_fast: lambda_bd10_fast,
        rd: &bd10_rd,
    };
    // -- Candidate injection + MDS0 -- see [`inject`].
    // C `generate_md_stage_0_cand`: regular intra, filter-intra, palette, then
    // IntraBC, each scored with the Hadamard SATD fast cost. The returned order
    // is C's PROCESSING order and the MDS0 pool below depends on it.
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
    let staging = nic::stage_mds0_to_mds1(&cands, cfg, frame.cli_qp, frame.non_i_slice);
    let order = &staging.order;
    let n1 = order.len();

    // -- MDS1: luma-only full loop -- see [`mds1`].
    mds1::run_mds1(
        fx,
        &geom,
        &bd10_rd,
        &qt,
        lambda,
        y_src,
        y_src_stride,
        y_src_off,
        y_recon,
        y_stride,
        &mut cands,
        order,
        n1,
    );

    // -- MDS1 -> MDS3 staging: per-class full-cost sort + the two prunes --
    // C `sort_full_cost_based_candidates` + `post_mds1_nic_pruning` +
    // `post_mds2_nic_pruning`. See [`nic`].
    let staging3 = nic::stage_mds1_to_mds3(&cands, cfg, &staging);
    let order1 = staging3.order1;
    let n3 = staging3.n3;

    // -- MDS3 + the independent-chroma search -- see [`mds3`].
    mds3::run_mds3(
        fx,
        &geom,
        &cx,
        &bd10_rd,
        pal,
        &qt,
        lambda,
        y_src,
        y_src_stride,
        y_src_off,
        y_recon,
        y_stride,
        sb_is_lvl6,
        &mut cands,
        &order1,
        n3,
        &mut ind_uv,
    );

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
        // This is the ONLY read of `y_recon_d0` in the tree, and
        // `eval_candidate` builds it for EVERY candidate of every leaf —
        // measured, and left that way on purpose: eliding it for the
        // candidates nobody reads is a REGRESSION, see
        // benchmarks/mds3d0_null_2026-09-03.meta.
        //
        // It can also be EMPTY here. MEASURED 2026-09-03 by asserting
        // non-emptiness: `avif::tests::lossless_is_qp0_on_420_and_refused_on_mono`
        // trips it. A coded-lossless 8x8 leaf takes `start_depth = 1` (C
        // `get_start_end_tx_depth`'s "force TX_4X4 for 8x8"), so the
        // `depth == 0` arm that fills `y_recon_d0` never runs and `gate_y` is
        // an empty slice. `depth_refine`'s quad-dist gate indexes `gate_y`
        // directly and would panic on it; it does not today because that gate
        // is not reached for those blocks. UNFIXED and pre-existing — noted
        // here so the next reader does not have to rediscover it.
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
                frame.rdoq_allintra_rd_mult,
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
