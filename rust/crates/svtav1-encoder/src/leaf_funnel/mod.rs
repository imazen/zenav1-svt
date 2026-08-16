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
// Module layout (split 2026-08-16)
// --------------------------------------------------------------------------
//
// This file was 11,247 lines; the port map had wanted it split since July.
// PURE CODE MOVEMENT -- the glob re-exports keep every existing
// `crate::leaf_funnel::X` path resolving unchanged, so byte-identity of the
// encoder is the acceptance test rather than a reading of the diff.

use crate::quant::TX_SCALE_TAB;

mod coeff_rate;
mod predict;
mod rate_tables;
mod tx_pipeline;
// A glob re-export CAPS the visibility of what it re-exports. `pub(crate) use`
// therefore silently demoted the genuinely-`pub` `build_md_rates` and broke the
// integration tests that reach it from outside the crate; a blanket `pub use`
// then warned on the three modules that export nothing public. So: crate-scoped
// globs for the internals, and one explicit `pub` re-export for the item that
// really is part of the crate's surface.
pub(crate) use coeff_rate::*;
pub(crate) use predict::*;
pub use rate_tables::build_md_rates;
pub(crate) use rate_tables::*;
pub(crate) use tx_pipeline::*;

// ---------------------------------------------------------------------------
// The funnel
// ---------------------------------------------------------------------------

/// C `intra_luma_to_chroma` (mode_decision.c:42) — identity mapping.
#[inline]
fn uv_from_y(mode: u8) -> u8 {
    mode
}

/// C `MAX_MODE_COST` (coding_unit.h:37) — the RD-cost sentinel for
/// "not set" used by md_cfl_rd_pick_alpha / cfl_prediction.
const MAX_MODE_COST: u64 = 13754408443200 * 8;

/// CfL AC luma subsampling with C's chroma-PAIR geometry
/// (`compute_cfl_ac_components`, product_coding_loop.c:3750). C subsamples
/// `cfl_temp_luma_recon` at the ROUND_UV (8-aligned) origin over
/// `max(w,8) x max(h,8)` — i.e. the whole chroma-reference PAIR for a sub-8
/// block (an 8x4/4x8/4x4 chroma-ref block's chroma covers the 8x8 pair, so
/// its CfL luma is the pair, not just the block). `cfl_temp_luma_recon`
/// accumulates every block's recon in the SB, so the pair holds the already-
/// committed sibling(s) plus this block. Here `y_recon` carries the committed
/// siblings (the walk commits child N before evaluating child N+1) and
/// `best_recon` is this block's (uncommitted) winning-depth luma recon.
///
/// For `w >= 8 && h >= 8` the pair reduces to the block itself → identical to
/// subsampling `best_recon` directly (fast path, zero change for >=8 blocks).
fn cfl_ac_subsample(
    y_recon: &[u8],
    y_stride: usize,
    best_recon: &[u8],
    abs_x: usize,
    abs_y: usize,
    w: usize,
    h: usize,
    pred_buf_q3: &mut [i16],
) {
    if w >= 8 && h >= 8 {
        svtav1_dsp::intra_pred::cfl_luma_subsampling_420(best_recon, w, pred_buf_q3, w, h);
        return;
    }
    // Sub-8 chroma-ref: assemble the max(w,8) x max(h,8) pair at the
    // ROUND_UV origin from the committed frame recon, then overlay this
    // block's uncommitted recon (== C's cfl_temp_luma_recon state).
    let luma_w = w.max(8);
    let luma_h = h.max(8);
    let pair_x = abs_x & !7;
    let pair_y = abs_y & !7;
    let off_x = abs_x - pair_x;
    let off_y = abs_y - pair_y;
    let mut pair = alloc::vec![0u8; luma_w * luma_h];
    for r in 0..luma_h {
        let src = (pair_y + r) * y_stride + pair_x;
        pair[r * luma_w..r * luma_w + luma_w].copy_from_slice(&y_recon[src..src + luma_w]);
    }
    for r in 0..h {
        let db = (off_y + r) * luma_w + off_x;
        pair[db..db + w].copy_from_slice(&best_recon[r * w..r * w + w]);
    }
    svtav1_dsp::intra_pred::cfl_luma_subsampling_420(&pair, luma_w, pred_buf_q3, luma_w, luma_h);
}

/// 10-bit twin of [`cfl_ac_subsample`]. C `compute_cfl_ac_components`
/// (product_coding_loop.c:3683) branches on `hbd_md` only to pick
/// `svt_cfl_luma_subsampling_420_hbd` over the lbd kernel and to read
/// `cfl_temp_luma_recon16bit` over `cfl_temp_luma_recon` — the geometry
/// (ROUND_UV pair origin, the uncommitted-block overlay) is identical, so this
/// mirrors the u8 body exactly with the pixel type swapped. The resulting
/// `pred_buf_q3` is ~4x the 8-bit one at bd10, which is correct and required:
/// it is added to a 10-bit DC base inside `cfl_predict_hbd`.
fn cfl_ac_subsample_hbd(
    y_recon10: &[u16],
    y_stride: usize,
    best_recon10: &[u16],
    abs_x: usize,
    abs_y: usize,
    w: usize,
    h: usize,
    pred_buf_q3: &mut [i16],
) {
    if w >= 8 && h >= 8 {
        svtav1_dsp::hbd::cfl_luma_subsampling_420_hbd(best_recon10, w, pred_buf_q3, w, h);
        return;
    }
    let luma_w = w.max(8);
    let luma_h = h.max(8);
    let pair_x = abs_x & !7;
    let pair_y = abs_y & !7;
    let off_x = abs_x - pair_x;
    let off_y = abs_y - pair_y;
    let mut pair = alloc::vec![0u16; luma_w * luma_h];
    for r in 0..luma_h {
        let src = (pair_y + r) * y_stride + pair_x;
        pair[r * luma_w..r * luma_w + luma_w].copy_from_slice(&y_recon10[src..src + luma_w]);
    }
    for r in 0..h {
        let db = (off_y + r) * luma_w + off_x;
        pair[db..db + w].copy_from_slice(&best_recon10[r * w..r * w + w]);
    }
    svtav1_dsp::hbd::cfl_luma_subsampling_420_hbd(&pair, luma_w, pred_buf_q3, luma_w, luma_h);
}

/// CfL AC luma for the bd10 re-encode post-pass (`compute_cfl_ac_components`,
/// product_coding_loop.c:3683 + `svt_subtract_average`). The in-search twin
/// [`cfl_ac_subsample_hbd`] has to overlay the block's *uncommitted* recon onto
/// the frame; in the post-pass the luma re-encode has already walked the whole
/// frame, so the ROUND_UV pair is read straight out of the committed 10-bit
/// luma recon. For `w >= 8 && h >= 8` (`abs_x`/`abs_y` are then 8-aligned) the
/// pair IS the block, so the two agree by construction.
pub(crate) fn cfl_ac_from_frame_recon_hbd(
    y_recon10: &[u16],
    y_stride: usize,
    abs_x: usize,
    abs_y: usize,
    w: usize,
    h: usize,
    cw: usize,
    chh: usize,
    pred_buf_q3: &mut [i16],
) {
    let luma_w = w.max(8);
    let luma_h = h.max(8);
    let pair_x = abs_x & !7;
    let pair_y = abs_y & !7;
    let mut pair = alloc::vec![0u16; luma_w * luma_h];
    for r in 0..luma_h {
        let src = (pair_y + r) * y_stride + pair_x;
        pair[r * luma_w..r * luma_w + luma_w].copy_from_slice(&y_recon10[src..src + luma_w]);
    }
    svtav1_dsp::hbd::cfl_luma_subsampling_420_hbd(&pair, luma_w, pred_buf_q3, luma_w, luma_h);
    svtav1_dsp::intra_pred::cfl_subtract_average(pred_buf_q3, cw, chh);
}

/// C `cfl_idx_to_alpha` (intra_prediction.h:134): signed Q3 alpha for a
/// (idx, joint_sign, plane). plane 0 = Cb (U), 1 = Cr (V).
#[inline]
pub(crate) fn cfl_idx_to_alpha(alpha_idx: u8, joint_sign: u8, plane: usize) -> i32 {
    use svtav1_entropy::context::{cfl_sign_u, cfl_sign_v};
    let js = joint_sign as usize;
    let alpha_sign = if plane == 0 {
        cfl_sign_u(js)
    } else {
        cfl_sign_v(js)
    };
    if alpha_sign == 0 {
        // CFL_SIGN_ZERO
        return 0;
    }
    let abs_alpha = if plane == 0 {
        (alpha_idx >> 4) as i32 // CFL_IDX_U
    } else {
        (alpha_idx & 15) as i32 // CFL_IDX_V
    };
    if alpha_sign == 2 {
        abs_alpha + 1 // CFL_SIGN_POS
    } else {
        -abs_alpha - 1 // CFL_SIGN_NEG
    }
}

/// C `PLANE_SIGN_TO_JOINT_SIGN(plane, a, b)` (product_coding_loop.c:3612):
/// `plane == U ? a*CFL_SIGNS + b - 1 : b*CFL_SIGNS + a - 1`.
#[inline]
fn plane_sign_to_joint_sign(plane: usize, a: usize, b: usize) -> u8 {
    let js = if plane == 0 {
        a * 3 + b - 1
    } else {
        b * 3 + a - 1
    };
    js as u8
}

/// C `md_cfl_rd_pick_alpha` (product_coding_loop.c:3615). Searches the CfL
/// alpha (magnitude + joint sign) that minimises the two-plane RD, using
/// `av1_cost_calc_cfl`'s per-(plane, alpha) cost = (CfL residual TX/quant
/// SSD, coeff bits). Returns `(cfl_alpha_idx, cfl_alpha_signs, best_rd)`
/// where `best_rd` includes the UV_CFL_PRED mode rate (`mode_rd`) so it is
/// directly comparable to `non_cfl_cost`. `pred_buf_q3` is the AC luma
/// (from compute_cfl_ac_components); `u_dc`/`v_dc` the DC chroma base.
#[allow(clippy::too_many_arguments)]
fn md_cfl_rd_pick_alpha(
    pred_buf_q3: &[i16],
    u_dc: &[u8],
    v_dc: &[u8],
    u_src: &[u8],
    v_src: &[u8],
    c_stride: usize,
    c_off: usize,
    cw: usize,
    chh: usize,
    // This block's chroma cropped-TX distortion extent (`frame_geom::
    // cropped_tx_dims_uv`). Inert here — `av1_cost_calc_cfl` scores the
    // TRANSFORM domain — but threaded so the chroma TX calls all name the
    // same C quantity.
    uv_crop: (usize, usize),
    cb_tsc: usize,
    cb_dsc: usize,
    cr_tsc: usize,
    cr_dsc: usize,
    qt_u: &QuantTable,
    qt_v: &QuantTable,
    frame: &FunnelFrame,
    rates: &MdRates,
    do_rdoq: bool,
    lambda: u64,
    luma_mode: usize,
    itr_th: u8,
) -> (u8, u8, u64) {
    // Per-(plane, alpha_q3) CfL cost: CfL-predict the plane from the DC
    // base + AC luma, TX/quant/recon the residual (same path the non-CFL
    // chroma uses), return (SSD residual distortion, coeff bits). Mirrors
    // av1_cost_calc_cfl (product_coding_loop.c:3445) for one component.
    let plane_cost = |plane: usize, alpha_q3: i32| -> (u64, i32) {
        let (src, dc, tsc, dsc) = if plane == 0 {
            (u_src, u_dc, cb_tsc, cb_dsc)
        } else {
            (v_src, v_dc, cr_tsc, cr_dsc)
        };
        let mut cfl_pred = vec![0u8; cw * chh];
        svtav1_dsp::intra_pred::cfl_predict_lbd(
            pred_buf_q3,
            dc,
            cw,
            &mut cfl_pred,
            cw,
            alpha_q3,
            cw,
            chh,
        );
        // C `av1_cost_calc_cfl` costs each alpha via svt_aom_full_loop_uv with
        // is_full_loop=0 -> TRANSFORM-domain distortion, NOT the spatial SSE
        // that feeds the final block RD. spatial_dist=false mirrors that.
        let out = tx_unit(
            src,
            c_stride,
            c_off,
            &cfl_pred,
            cw,
            0,
            cw,
            chh,
            0,
            1,
            tsc,
            dsc,
            0,
            if plane == 0 { qt_u } else { qt_v },
            frame,
            rates,
            do_rdoq,
            false,
            uv_crop,
            // R1: this closure returns `(out.dist, out.bits)` and nothing else
            // — the recon is unread. C's `av1_cost_calc_cfl` reaches
            // `svt_aom_full_loop_uv` with `is_full_loop = 0`, so the
            // `if (is_full_loop && ctx->mds_do_spatial_sse)` inverse-transform
            // gate at full_loop.c:2313 is false for every alpha it tries.
            false,
            RateMode::Exact,
        );
        (out.dist, out.bits)
    };

    md_cfl_alpha_search(plane_cost, rates, lambda, luma_mode, itr_th)
}

/// The bit-depth-INDEPENDENT driver of C `md_cfl_rd_pick_alpha`
/// (product_coding_loop.c:3547): the `plane x pn_sign x magnitude` alpha
/// search, the `itr_th` early exit and the joint-sign bookkeeping. Everything
/// depth-specific lives in `plane_cost(plane, alpha_q3) -> (dist, coeff_bits)`
/// — C's `av1_cost_calc_cfl` for one component, which is the ONLY place the
/// pixel type, the quant table and the CfL predictor enter. Splitting here is
/// what lets the u8 and bd10 arms share one provably-identical search.
fn md_cfl_alpha_search(
    plane_cost: impl Fn(usize, i32) -> (u64, i32),
    rates: &MdRates,
    lambda: u64,
    luma_mode: usize,
    itr_th: u8,
) -> (u8, u8, u64) {
    let mode_rd = rdcost(lambda, rates.uv[1][luma_mode][UV_CFL_PRED_IDX] as u64, 0);
    let mut best_rd = MAX_MODE_COST;
    let mut best_rd_uv = [[MAX_MODE_COST; 2]; 8]; // [joint_sign][plane]
    let mut best_c = [[0u8; 2]; 8];
    let mut best_joint_sign = 0u8;
    let mut best_joint_sign_found = false;

    // Alpha-zero pass: seed best_rd_uv for the joint signs with a zero
    // component in this plane (CFL_SIGN_ZERO,{NEG,POS}).
    for plane in 0..2 {
        let jsn = plane_sign_to_joint_sign(plane, 0, 1); // ZERO, NEG
        let alpha0 = cfl_idx_to_alpha(0, jsn, plane); // == 0
        let (dist, cbits) = plane_cost(plane, alpha0);
        let arate_neg = rates.cfl_alpha_fac_bits[jsn as usize][plane][0] as u64;
        best_rd_uv[jsn as usize][plane] = rdcost(lambda, cbits as u64 + arate_neg, dist);
        let jsp = plane_sign_to_joint_sign(plane, 0, 2); // ZERO, POS
        let arate_pos = rates.cfl_alpha_fac_bits[jsp as usize][plane][0] as u64;
        best_rd_uv[jsp as usize][plane] = rdcost(lambda, cbits as u64 + arate_pos, dist);
    }

    // Main search over plane, sign, magnitude c (with the itr_th early exit).
    for plane in 0..2 {
        for pn_sign in 1..3usize {
            // NEG=1, POS=2
            let mut progress = 0u8;
            for c in 0..16usize {
                let mut flag = 0u8;
                if c as u8 > itr_th && progress < c as u8 {
                    break;
                }
                let mut dist = 0u64;
                let mut cbits = 0i32;
                for i in 0..3usize {
                    // CFL_SIGNS
                    let joint_sign = plane_sign_to_joint_sign(plane, pn_sign, i);
                    if i == 0 {
                        let idx = ((c << 4) + c) as u8;
                        let alpha = cfl_idx_to_alpha(idx, joint_sign, plane);
                        let (d, b) = plane_cost(plane, alpha);
                        dist = d;
                        cbits = b;
                    }
                    let arate = rates.cfl_alpha_fac_bits[joint_sign as usize][plane][c] as u64;
                    let this_rd = rdcost(lambda, cbits as u64 + arate, dist);
                    if this_rd >= best_rd_uv[joint_sign as usize][plane] {
                        continue;
                    }
                    best_rd_uv[joint_sign as usize][plane] = this_rd;
                    best_c[joint_sign as usize][plane] = c as u8;
                    flag = itr_th;
                    let other = 1 - plane;
                    if best_rd_uv[joint_sign as usize][other] == MAX_MODE_COST {
                        continue;
                    }
                    let combined = this_rd + mode_rd + best_rd_uv[joint_sign as usize][other];
                    if combined >= best_rd {
                        continue;
                    }
                    best_rd = combined;
                    best_joint_sign = joint_sign;
                    best_joint_sign_found = true;
                }
                progress += flag;
            }
        }
    }

    let (mut cfl_idx, mut cfl_signs) = (0u8, 0u8);
    if best_rd != MAX_MODE_COST {
        let mut ind = 0u8;
        if best_joint_sign_found {
            let u = best_c[best_joint_sign as usize][0];
            let v = best_c[best_joint_sign as usize][1];
            ind = (u << 4) + v;
        }
        cfl_idx = ind;
        cfl_signs = best_joint_sign;
    }
    (cfl_idx, cfl_signs, best_rd)
}

/// C `UV_CFL_PRED` chroma-mode index.
const UV_CFL_PRED_IDX: usize = 13;

/// C `fimode_to_intradir` (common_utils.c:33).
pub(crate) const FIMODE_TO_INTRADIR: [u8; 5] = [0, 1, 2, 6, 0];
/// C `fimode_to_intramode` (definitions.h:1301) — differs from INTRADIR in the
/// last entry: FILTER_PAETH maps to PAETH (12), not DC. C uses THIS table for
/// the injection-time uv/uv_delta assignment; the tx/ext-tx rate paths use
/// INTRADIR (common_utils.c:33 via rd_cost.c:135).
pub(crate) const FIMODE_TO_INTRAMODE: [u8; 5] = [0, 1, 2, 6, 12];

/// One funnel candidate's evolving state.
struct Cand {
    mode: u8,
    /// Luma angle delta (directional modes only; C ANGLE_STEP units).
    delta: i8,
    fi: u8,
    uv: u8,
    /// Chroma angle delta (= luma delta at injection; rewritten by the
    /// ind-uv MDS3 update at chroma_level 4).
    uv_delta: i8,
    /// Whole-block depth-0 luma prediction (w x h).
    pred: Vec<u8>,
    /// The SAME prediction at TRUE 10 bits, from the bd10 recon canvas
    /// (task #94). MDS0 already computes this to score the fast cost and
    /// used to throw it away; MDS1/MDS3 need it as their depth-0 predictor.
    /// Empty unless the bd10 full-RD funnel is active.
    pred10: Vec<u16>,
    flr: u64,
    fcr: u64,
    fast_cost: u64,
    // MDS1:
    full_cost: u64,
    /// [SVT_HDR_MODE] parallel SSIM full cost (only when frame.tune_ssim).
    mds3_cost_ssim: u64,
    mds1_has_coeff: bool,
    // MDS3 winner data:
    tx_depth: u8,
    txb_q: Vec<Vec<i32>>,
    txb_eob: Vec<u16>,
    txb_cul: Vec<u8>,
    txb_type: Vec<u8>,
    y_recon: Vec<u8>,
    /// The winner's TRUE 10-bit LUMA recon (w*h), from the winning tx depth
    /// of the bd10 MDS3 loop. Empty unless the bd10 full-RD funnel is active.
    y_recon10: Vec<u16>,
    /// The winner's TRUE 10-bit chroma recon (cw*chh each), produced by the
    /// bd10 chroma full loop. `commit_leaf` writes them into the bd10 chroma
    /// canvases so the NEXT block predicts chroma from 10-bit neighbours —
    /// the same sequential coupling `y_recon10` closes for luma. Empty
    /// unless the bd10 full-RD funnel is active.
    u_recon10: Vec<u16>,
    v_recon10: Vec<u16>,
    /// The tx_depth-0 luma recon (C's shared `cand_bf->recon` state after the
    /// TX loop — deeper depths reconstruct in aux buffers and are never
    /// copied back, so the quad-dist gates measure THIS, not `y_recon`).
    y_recon_d0: Vec<u8>,
    y_bits: u64,
    y_dist: u64,
    u_q: Vec<i32>,
    v_q: Vec<i32>,
    u_eob: u16,
    v_eob: u16,
    u_cul: u8,
    v_cul: u8,
    u_recon: Vec<u8>,
    v_recon: Vec<u8>,
    /// CfL alpha idx/signs when the MDS3 chroma decision picked
    /// UV_CFL_PRED (uv == 13); both 0 otherwise (C block_mi.cfl_alpha_*).
    cfl_alpha_idx: u8,
    cfl_alpha_signs: u8,
    /// Luma palette candidate payload (colors, full-size idx map) — Some
    /// only for candidates injected by `inject_palette_candidates`
    /// (mode == DC, fi == NONE). The prediction is map->colors
    /// SUBSTITUTION (position-only, no neighbor edges) at every stage.
    palette: Option<(Vec<u16>, Vec<u8>)>,
    /// IntraBC candidate payload `(dv, pred_dv)` (IBC chunk 7/8) — Some
    /// only for candidates injected by the IBC lane
    /// (`inject_intra_bc_candidates`): the winning eighth-pel DV +
    /// `ref_mv_stack[INTRA_FRAME][0].this_mv` (the dv_ref the writer's
    /// `svt_av1_encode_dv` diffs against). The candidate's other fields
    /// follow `build_intra_bc_candidate`: mode DC (0), uv DC (0), fi
    /// NONE, deltas 0 — an IBC cand is `is_inter`-classified everywhere
    /// (tx set, tx_size vartx coding, no CfL / no ind-uv rewrite).
    ibc: Option<(svtav1_types::motion::Mv, svtav1_types::motion::Mv)>,
    mds3_cost: u64,
    block_has_coeff: bool,
    /// C `blk_ptr->total_rate` / `full_dist` (svt_aom_full_cost writeback)
    /// — read by the NSQ component-multiple / recon-dist gates.
    total_rate: u64,
    full_dist: u64,
}

/// The chosen leaf coding, consumed by the fixed-tree walk + the entropy
/// pass.
pub struct LeafChoice {
    pub mode: u8,
    /// Luma angle delta (0 for non-directional modes).
    pub angle_delta: i8,
    pub fi_mode: u8,
    pub uv_mode: u8,
    /// Chroma angle delta (0 unless the ind-uv search picked one).
    pub uv_angle_delta: i8,
    pub tx_depth: u8,
    /// Per-txb packed quantized levels (1 txb at depth 0, 4 at depth 1),
    /// in raster txb order.
    pub txb_qcoeffs: Vec<Vec<i32>>,
    pub txb_eobs: Vec<u16>,
    /// Per-txb C TxType indices (winner of the per-txb TXT search).
    pub txb_tx_types: Vec<u8>,
    pub u_qcoeffs: Vec<i32>,
    pub v_qcoeffs: Vec<i32>,
    pub u_eob: u16,
    pub v_eob: u16,
    /// The winner's reconstructed chroma blocks (cw x ch rasters) — the
    /// entropy walk copies these into its chroma planes so the walk's
    /// recon evolution is byte-identical to the decision phase's.
    pub u_recon: Vec<u8>,
    pub v_recon: Vec<u8>,
    /// CfL alpha idx/signs for a UV_CFL_PRED (uv_mode == 13) leaf; the
    /// entropy writer emits `write_cfl_alphas` from these. 0/0 otherwise.
    pub cfl_alpha_idx: u8,
    pub cfl_alpha_signs: u8,
    /// Winning palette payload (colors, full-size idx map) — Some iff the
    /// palette candidate won this leaf; flows into BlockDecision.palette.
    pub palette: Option<(Vec<u16>, Vec<u8>)>,
    /// IBC chunk 8: `(dv, dv_ref)` — Some iff the IntraBC candidate won
    /// this leaf; flows into BlockDecision (chunk 9) for the pack's
    /// `write_intrabc_info` + var-tx tx_size writer.
    pub ibc: Option<(svtav1_types::motion::Mv, svtav1_types::motion::Mv)>,
}

/// Per-frame/SB mutable funnel context threaded through the fixed tree.
/// Native 10-bit (u16) SOURCE planes for the bd10 funnel — task #6 chunk 1.
///
/// `Some` on [`FunnelCtx::src10`] exactly when the caller entered through
/// [`crate::pipeline::EncodePipeline::try_encode_frame_420_hbd`]: the planes
/// carry the REAL 10-bit samples instead of the `u8 << 2` widening every bd10
/// stage used before. Frame-strided over the ALIGNED frame (the bd10 envelope
/// is 64-aligned-gated, so `y_stride` equals the funnel's `y_src`/`y_recon`
/// stride and a block at `(abs_x, abs_y)` indexes identically in both).
#[derive(Clone, Copy)]
pub(crate) struct FunnelSrc10<'a> {
    pub y: &'a [u16],
    pub y_stride: usize,
    /// Chroma planes at `c_stride` (== the u8 `u_src`/`v_src` layout). Empty
    /// on a monochrome frame (which never builds a funnel today).
    pub u: &'a [u16],
    pub v: &'a [u16],
    pub c_stride: usize,
}

pub(crate) struct FunnelCtx<'a> {
    pub u_src: &'a [u8],
    pub v_src: &'a [u8],
    /// Native 10-bit source planes (task #6 chunk 1). `None` on every u8
    /// entry point AND on a bd10 encode of an 8-bit source, where the bd10
    /// stages keep widening `u8 << 2` exactly as before — so every existing
    /// gate cell is byte-unchanged.
    pub src10: Option<FunnelSrc10<'a>>,
    pub u_recon: &'a mut [u8],
    pub v_recon: &'a mut [u8],
    pub c_stride: usize,
    pub ectx: &'a mut crate::pipeline::EntropyCtx,
    pub rates: &'a MdRates,
    pub frame: &'a FunnelFrame,
    /// bd10 LUMA mode-decision recon canvas (task #94, the u16 mode funnel):
    /// the TRUE 10-bit reconstruction of every committed block, frame-strided
    /// (== the u8 `y_recon` canvas dims/stride). `Some` ONLY for complete-SB
    /// eff-M9 (preset ≥ 9) bd10 frames; `None` (bd8, and every other bd10
    /// preset/partial-SB) leaves the funnel byte-IDENTICAL. When present,
    /// `evaluate_leaf`'s MDS0 fast loop predicts each candidate at 10-bit from
    /// this canvas and scores the 10-bit SATD (so the mode survivor is C's
    /// bd10 winner, not the u8 winner — the DC↔SMOOTH flips on diagonal-edge
    /// content), and `commit_leaf` writes the winner's 10-bit recon back for
    /// the next block's neighbours. The coded LEVELS come from the post-pass
    /// `bd10_reencode_luma`, which reads these bd10-decided modes.
    pub y_recon10: Option<&'a mut [u16]>,
    /// bd10 CHROMA mode-decision recon canvases — the chroma twins of
    /// `y_recon10`, chroma-strided (`c_stride`). `Some` exactly when
    /// `full_rd10` is set: the MDS3 chroma full loop predicts from them so
    /// the joint (luma + chroma) block RD is entirely 10-bit.
    pub u_recon10: Option<&'a mut [u16]>,
    pub v_recon10: Option<&'a mut [u16]>,
    /// Run the FULL-RD stages (MDS1 + MDS3, luma AND chroma) at bd10.
    ///
    /// `y_recon10` alone only fixes MDS0, which is sufficient at eff-M9
    /// (`nic_counts == (1,1,1)` -> the fast survivor IS the coded mode) but
    /// NOT below it: at M6 `nic_counts == (6,6,6)`, several candidates reach
    /// MDS1/MDS3 and the full-RD compare picks the winner. Widening only the
    /// MDS0 funnel to M6..M8 was measured to close ZERO cells
    /// (docs/bd10-port-map.md "MEASURED NEGATIVE"), which is what this flag
    /// exists to fix. Requires `y_recon10`/`u_recon10`/`v_recon10` to be set.
    pub full_rd10: bool,
    /// IBC chunk 8: frame-level IntraBC search state (hash table, site
    /// config, search cost tables, ctrls, tile/mi geometry). `None`
    /// unless `cfg.allow_intrabc` — every IBC path is unreachable then.
    pub ibc: Option<&'a IbcFrameState>,
    /// The MD mode-info grid the INTRA_FRAME MVP scans read (C
    /// `pcs->mi_grid_base` as MD stamps it): one entry per 4x4 mi cell,
    /// frame-wide, stamped by [`commit_leaf`] per mid-walk commit exactly
    /// like C's `svt_aom_update_mi_map` (product_coding_loop.c:670) — and
    /// NOT restored by the NSQ walk's node snapshots (C never restores the
    /// mi map between shapes; losing shapes' stamps linger until
    /// overwritten, so this lives OUTSIDE `EntropyCtx`). `None` unless
    /// `cfg.allow_intrabc`.
    pub ibc_mvp: Option<&'a mut alloc::vec::Vec<crate::intrabc_mvp::MvpMiEntry>>,
    /// Per-leaf IBC gate input, set by the partition/NSQ walk before each
    /// `evaluate_leaf` call (the C `ctx->shape` + `pc_tree` state the
    /// `do_intra_bc` gate reads, mode_decision.c:3597-3616).
    pub ibc_gate: IbcGateInput,
}

/// C `BlockSize` enum index from pixel dims (definitions.h block order) —
/// the MVP block-ctx derivation consumes the C index.
pub(crate) fn c_bsize_index(w: usize, h: usize) -> usize {
    match (w, h) {
        (4, 4) => 0,
        (4, 8) => 1,
        (8, 4) => 2,
        (8, 8) => 3,
        (8, 16) => 4,
        (16, 8) => 5,
        (16, 16) => 6,
        (16, 32) => 7,
        (32, 16) => 8,
        (32, 32) => 9,
        (32, 64) => 10,
        (64, 32) => 11,
        (64, 64) => 12,
        (64, 128) => 13,
        (128, 64) => 14,
        (128, 128) => 15,
        (4, 16) => 16,
        (16, 4) => 17,
        (8, 32) => 18,
        (32, 8) => 19,
        (16, 64) => 20,
        (64, 16) => 21,
        _ => panic!("no C BlockSize for {w}x{h}"),
    }
}

/// The per-leaf inputs of the IBC injection gate + the current block's
/// live partition (C `pc_tree->partition` on the current mbmi — read by
/// `has_top_right`'s VERT_A case via the CURRENT mi cell).
#[derive(Clone, Copy, Debug)]
pub(crate) struct IbcGateInput {
    /// C PartitionType of the shape under evaluation (NONE=0, HORZ=1,
    /// VERT=2, SPLIT=3, HORZ_A=4, HORZ_B=5, VERT_A=6, VERT_B=7,
    /// HORZ_4=8, VERT_4=9).
    pub partition: u8,
    /// `ctx->shape == PART_N`.
    pub is_part_n: bool,
    /// The node's PART_N (square) winner: `(tested, used_intrabc)` — C
    /// `pc_tree->tested_blk[PART_N][0]` +
    /// `block_data[PART_N][0]->block_mi.use_intrabc`.
    pub sibling_n0: (bool, bool),
}

impl Default for IbcGateInput {
    /// The fixed-tree default: PART_N (square leaves; the gate always
    /// allows — b4 gating is off at every allintra IBC level).
    fn default() -> Self {
        Self {
            partition: 0,
            is_part_n: true,
            sibling_n0: (false, false),
        }
    }
}

/// Frame-constant IntraBC search state (IBC chunk 8) — everything
/// `intra_bc_search` + the MVP build need beyond the funnel context.
/// Built once per frame in the pipeline when `allow_intrabc`.
pub struct IbcFrameState {
    /// Per-level controls with the one-shot QP mesh rescale applied
    /// (md_config_process.c:956-969).
    pub ctrls: crate::intrabc::IbcCtrls,
    /// The frame source hash table (`generate_ibc_data`).
    pub hash: crate::intrabc_hash::HashTable,
    /// Diamond site config (per-frame, source stride baked).
    pub sites: crate::intrabc::SearchSiteConfig,
    /// SEARCH-time mv cost tables: C `md_rate_est_ctx->nmv_vec_cost` /
    /// `nmvcoststack` — built from `fc->nmvc` at precision
    /// `allow_high_precision_mv` (= 0 = LOW on a KEY frame, i.e. WITH
    /// fractional-bit costs; svt_aom_estimate_mv_rate). Frame-constant
    /// (update_mv forced 0 on I-slices). Distinct from the RD-time
    /// `FunnelFrame::dv_tables` (ndvc at MV_SUBPEL_NONE).
    pub search_tables: crate::intrabc::MvCostTables,
    /// `svt_aom_get_sad_per_bit(base_q_idx, 0)` (mode_decision.c:3010).
    pub sad_per_bit: i32,
    /// `full_lambda >> RD_EPB_SHIFT`, min 1 (mode_decision.c:3011-3012).
    pub error_per_bit: i32,
    pub mi_rows: i32,
    pub mi_cols: i32,
    pub tile: crate::intrabc::TileMiBounds,
    pub sb_mi_size: i32,
    pub sb_size_log2_mi: u32,
    pub sb_size_px: i32,
    /// `pcs->pic_disallow_4x4` — gates the 4x4 hash size out of the table.
    pub disallow_4x4: bool,
}

/// One evaluated (not yet committed) PART_N funnel decision — the C
/// `md_encode_block` output before `md_update_all_neighbour_arrays`
/// commits it. The PD1 depth walk evaluates parent and child depths and
/// only commits the depth that wins the inter-depth compare.
pub(crate) struct LeafEval {
    pub abs_x: usize,
    pub abs_y: usize,
    pub w: usize,
    pub h: usize,
    /// C `ctx->has_uv` (is_chroma_reference) + the chroma PAIR geometry
    /// (bsize_uv dims at the ROUND_UV origin) — sub-8 NSQ children only
    /// deviate from (x/2, y/2, w/2, h/2).
    has_uv: bool,
    ccx: usize,
    ccy: usize,
    cw: usize,
    chh: usize,
    win: Cand,
    /// The shared `cand_bf->recon` state the quad-dist gates measure
    /// (skip-sub-depth cond1 + the NSQ recon-dist gates): bypass_encdec=0
    /// -> the winner rebuild (== winner final recon+chroma); bypass=1 ->
    /// the LAST MDS3 candidate's depth-0 luma recon + its chroma (the
    /// rebuild is redirected away and never reaches the shared buffer).
    gate_y: Vec<u8>,
    gate_u: Vec<u8>,
    gate_v: Vec<u8>,
    /// C `cand_bf->residual` content at `non_normative_txs` time: ALL
    /// MDS3 candidates share ONE residual workspace (verified by buffer-
    /// pointer instrumentation — docs/captures/nsq_m2m3), so the buffer
    /// holds the LAST MDS3-processed candidate's whole-block DEPTH-0
    /// residual (the depth-1/2 trials write the per-depth scratch
    /// buffers, init_tx_cand_bf copies OUT of this one).
    psq_resid: Vec<i32>,
    /// bd10 twin of `psq_resid` (task #94, root #2): the LAST MDS3 candidate's
    /// whole-block depth-0 residual at TRUE 10 bits (`src10 - last.pred10`).
    /// C's `non_normative_txs` (product_coding_loop.c:9180) transforms +
    /// quantizes this at `EB_TEN_BIT` (Q10 tables, `svt_aom_highbd_quantize_b`)
    /// to derive `min_nz_h`/`min_nz_v` — the counts the `skip_by_sq_txs` NSQ
    /// gate reads. Deciding that gate on the bd8 residual + Q8 quant flips
    /// which NSQ shapes are pruned (H-vs-V), so the port over/under-splits at
    /// bd10. Empty on the u8 path (bd8 keeps `psq_resid`, byte-unchanged).
    psq_resid10: Vec<i32>,
    /// bd10 mode funnel (task #94): the winner's TRUE 10-bit recon (w×h
    /// raster), reconstructed by `evaluate_leaf` from the bd10 canvas when
    /// `FunnelCtx::y_recon10` is `Some`. `commit_leaf` writes it back into the
    /// canvas for the next block's neighbour prediction. Empty on the u8 path.
    win_recon10: Vec<u16>,
    /// The winner's TRUE 10-bit CHROMA recon (cw*chh each) — the chroma twins
    /// of `win_recon10`, written into the bd10 chroma canvases by
    /// `commit_leaf`. Empty unless the bd10 full-RD funnel is active.
    win_u_recon10: Vec<u16>,
    win_v_recon10: Vec<u16>,
}

impl LeafEval {
    /// The winner's MDS3 full cost (C `blk_ptr->cost` before the
    /// partition-rate term the depth walk adds).
    pub(crate) fn block_cost(&self) -> u64 {
        self.win.mds3_cost
    }

    /// IBC chunk 8: whether the winner is an IntraBC candidate — the C
    /// `block_data[PART_N][0]->block_mi.use_intrabc` the NSQ parent gate
    /// reads (mode_decision.c:3608-3612).
    pub(crate) fn used_ibc(&self) -> bool {
        self.win.ibc.is_some()
    }

    /// C `cnt_nz_coeff` (sum of the winner's luma txb eobs,
    /// product_coding_loop.c:7166-7168).
    pub(crate) fn cnt_nz_coeff(&self) -> u32 {
        self.win.txb_eob.iter().map(|&e| e as u32).sum()
    }

    /// C `blk_ptr->total_rate` (the winner's full rate) and `full_dist`
    /// — inputs to the NSQ component-multiple gate.
    pub(crate) fn total_rate(&self) -> u64 {
        self.win.total_rate
    }

    pub(crate) fn full_dist(&self) -> u64 {
        self.win.full_dist
    }

    /// Winner luma mode (C `block_mi.mode`) — the NSQ recon-dist gate's
    /// modulation input.
    pub(crate) fn mode(&self) -> u8 {
        self.win.mode
    }

    /// Winner tx_depth (diagnostic; only read by the std-gated NSQDBG dumps).
    #[cfg(feature = "std")]
    pub(crate) fn tx_depth(&self) -> u8 {
        self.win.tx_depth
    }

    /// Winner uv_mode (diagnostic — 13 == UV_CFL_PRED; std-gated NSQDBG only).
    #[cfg(feature = "std")]
    pub(crate) fn uv_mode(&self) -> u8 {
        self.win.uv
    }

    pub(crate) fn block_has_coeff(&self) -> bool {
        self.win.block_has_coeff
    }

    /// NSQDBG only: winner per-txb tx types / luma eobs as "a,b,c" strings,
    /// plus chroma eobs — joined against C's CLEAF dump to catch coeff-level
    /// (tx_type/RDOQ) divergence that mode/uv/txd comparison misses.
    /// std-only (returns `String`; only consumed by the std-gated NSQDBG dumps).
    #[cfg(feature = "std")]
    pub(crate) fn dbg_txb_types(&self) -> String {
        let v: Vec<String> = self.win.txb_type.iter().map(|t| t.to_string()).collect();
        v.join(",")
    }

    #[cfg(feature = "std")]
    pub(crate) fn dbg_txb_eobs(&self) -> String {
        let v: Vec<String> = self.win.txb_eob.iter().map(|e| e.to_string()).collect();
        v.join(",")
    }

    #[cfg(feature = "std")]
    pub(crate) fn dbg_uv_eobs(&self) -> (u16, u16) {
        (self.win.u_eob, self.win.v_eob)
    }

    /// NSQDBG only: the winner's filter-intra mode (0 == FI off/none for
    /// non-DC winners; distinguishes FILTER_* candidates from plain DC).
    #[cfg(feature = "std")]
    pub(crate) fn dbg_fi(&self) -> u8 {
        self.win.fi
    }

    /// NSQDBG only: the winner's luma + chroma angle deltas.
    #[cfg(feature = "std")]
    pub(crate) fn dbg_deltas(&self) -> (i8, i8) {
        (self.win.delta, self.win.uv_delta)
    }

    /// NSQDBG only: the winner's per-txb quantized DC levels.
    #[cfg(feature = "std")]
    pub(crate) fn dbg_qdcs(&self) -> String {
        let v: Vec<String> = self.win.txb_q.iter().map(|q| q[0].to_string()).collect();
        v.join(",")
    }

    /// NSQDBG only: the winner's whole-block depth-0 luma prediction.
    #[cfg(feature = "std")]
    pub(crate) fn dbg_pred(&self) -> &[u8] {
        &self.win.pred
    }

    /// The quad-dist gate recon planes (see the `gate_y` field doc).
    pub(crate) fn gate_y(&self) -> &[u8] {
        &self.gate_y
    }

    pub(crate) fn gate_uv(&self) -> (&[u8], &[u8]) {
        (&self.gate_u, &self.gate_v)
    }

    /// bd10 (task #94, root #2): the 10-bit twin of [`gate_y`](Self::gate_y) for
    /// the NSQ recon-dist gate. C's `calc_scr_to_recon_dist_per_quadrant`
    /// (product_coding_loop.c:8065) reads `cand_bf->recon` through
    /// `svt_full_distortion_kernel16_bits` at `hbd_md`, i.e. the 10-bit recon —
    /// while `gate_y` is the MSB-truncated u8 proxy. At bypass_encdec=0
    /// (preset <= 3) `cand_bf->recon` is the winner's final (winning-depth)
    /// recon, whose bd10 twin is exactly `win_recon10`. Empty on the u8 path.
    pub(crate) fn win_recon10(&self) -> &[u16] {
        &self.win_recon10
    }

    /// bd10 twin of [`gate_uv`](Self::gate_uv) — the winner's 10-bit chroma
    /// recon (chroma has no tx-depth split, so the winner recon is unambiguous).
    /// Empty unless the bd10 chroma full loop ran.
    pub(crate) fn win_uv_recon10(&self) -> (&[u16], &[u16]) {
        (&self.win_u_recon10, &self.win_v_recon10)
    }

    /// The shared MDS3 residual-workspace state (C `cand_bf->residual`,
    /// consumed by the psq gate): the LAST MDS3 candidate's depth-0
    /// residual.
    pub(crate) fn psq_resid(&self) -> &[i32] {
        &self.psq_resid
    }

    /// The bd10 twin of [`psq_resid`](Self::psq_resid): the last MDS3
    /// candidate's depth-0 residual at TRUE 10 bits. Empty on the u8 path.
    pub(crate) fn psq_resid10(&self) -> &[i32] {
        &self.psq_resid10
    }

    /// Winner luma recon (w x h raster).
    ///
    /// Kept alongside the used `psq_resid`/`psq_resid10` family so the
    /// winner's buffers have a uniform read surface; no live caller today.
    #[allow(dead_code)]
    pub(crate) fn y_recon(&self) -> &[u8] {
        &self.win.y_recon
    }

    /// Winner chroma recons ((size/2)^2 rasters). See [`y_recon`](Self::y_recon)
    /// for why this has no live caller.
    #[allow(dead_code)]

    /// The walk/entropy-pass view of the winner.
    ///
    /// Takes `self` and MOVES the winner's seven owned buffers into the
    /// `LeafChoice` rather than cloning them. They were cloned only because
    /// this used to run BEFORE `commit_leaf`, which borrows the `LeafEval`;
    /// `commit_leaf` reads none of `to_choice`'s output and `to_choice` reads
    /// none of `commit_leaf`'s side effects, so both callers simply commit
    /// first and convert second. Same bytes, seven fewer allocate+memcpy+free
    /// round trips per coded block.
    pub(crate) fn into_choice(self) -> LeafChoice {
        let cand = self.win;
        LeafChoice {
            mode: cand.mode,
            angle_delta: cand.delta,
            fi_mode: cand.fi,
            uv_mode: cand.uv,
            uv_angle_delta: cand.uv_delta,
            tx_depth: cand.tx_depth,
            txb_qcoeffs: cand.txb_q,
            txb_eobs: cand.txb_eob,
            txb_tx_types: cand.txb_type,
            u_qcoeffs: cand.u_q,
            v_qcoeffs: cand.v_q,
            u_eob: cand.u_eob,
            v_eob: cand.v_eob,
            u_recon: cand.u_recon,
            v_recon: cand.v_recon,
            cfl_alpha_idx: cand.cfl_alpha_idx,
            cfl_alpha_signs: cand.cfl_alpha_signs,
            palette: cand.palette,
            ibc: cand.ibc,
        }
    }
}

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

    // One full-loop chroma evaluation of a (uv_mode, uv_delta) pair —
    // the shared body of `search_best_mds3_uv_mode`'s full loop and
    // MDS3's `svt_aom_full_loop_uv` (identical settings: rdoq per frame
    // policy, spatial SSE, real contexts).
    // bd10 FULL-RD chroma (task #94): the 10-bit twin of `chroma_eval`. C's
    // `svt_aom_full_loop_uv` reaches the same facades at both depths — the
    // spatial chroma distortion is `svt_full_distortion_kernel16_bits` at
    // hbd_md != 0 (pic_operators.c:257) — so only the pixel type, the quant
    // table and the lambda move. This matters because the MDS3 block cost is
    // JOINT (luma + chroma): with the luma terms at 10 bits and chroma left at
    // 8, chroma would be ~16x under-weighted and every uv-follows-luma mode
    // flip would be decided on luma alone.
    let chroma_eval10 =
        |fx: &FunnelCtx<'_>, b: &Bd10Rd, uv: u8, uv_delta: i8| -> (TxUnitOutHbd, TxUnitOutHbd) {
            let mut u_pred = vec![0u16; cw * chh];
            let mut v_pred = vec![0u16; cw * chh];
            let c_off10 = ccy * fx.c_stride + ccx;
            predict_unit_hbd(
                fx.u_recon10.as_deref().unwrap(),
                fx.c_stride,
                ccx,
                ccy,
                cw,
                chh,
                uv,
                uv_delta,
                FI_NONE,
                &uv_geom,
                cfg.edge_filter,
                filt_type_uv,
                &mut u_pred,
                b.bd,
            );
            predict_unit_hbd(
                fx.v_recon10.as_deref().unwrap(),
                fx.c_stride,
                ccx,
                ccy,
                cw,
                chh,
                uv,
                uv_delta,
                FI_NONE,
                &uv_geom,
                cfg.edge_filter,
                filt_type_uv,
                &mut v_pred,
                b.bd,
            );
            let _ = c_off10;
            let tt = uv_tx_type(uv, cw, chh);
            let rd = |plane_dir: usize| TxRdArgs {
                spatial_dist: true, // MDS3 chroma is the spatial SSE (<<4)
                intra_dir: plane_dir,
                coeff_rate_est_lvl: cfg.coeff_rate_est_lvl,
                tx_bias: frame.tx_bias,
                crop: uv_crop,
            };
            let u_out = tx_unit_hbd(
                &b.u_src10,
                cw,
                0,
                &u_pred,
                cw,
                0,
                cw,
                chh,
                tt,
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
                Some(&rd(0)),
            );
            let v_out = tx_unit_hbd(
                &b.v_src10,
                cw,
                0,
                &v_pred,
                cw,
                0,
                cw,
                chh,
                tt,
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
                Some(&rd(0)),
            );
            (u_out, v_out)
        };
    // The IntraBC twin of `chroma_eval10`: an IBC candidate's chroma is the DV
    // copy / half-pel bilinear from the chroma recon (NOT an intra uv mode), so
    // the bd10 arm cannot reuse `chroma_eval10` -- that would score the
    // candidate against a prediction it does not use. The tx-type rule is the
    // INTER one the u8 arm already applies (the luma winner's txb-0 type when
    // the chroma ext set allows it, else DCT; tx_type_search,
    // product_coding_loop.c:5087-5096).
    let chroma_eval10_ibc = |fx: &FunnelCtx<'_>,
                             b: &Bd10Rd,
                             dv: svtav1_types::motion::Mv,
                             tt: usize|
     -> (TxUnitOutHbd, TxUnitOutHbd) {
        let mut u_pred = vec![0u16; cw * chh];
        let mut v_pred = vec![0u16; cw * chh];
        let frame_ch = frame.frame_h_px / 2;
        crate::intrabc_pred::predict_intrabc_chroma(
            fx.u_recon10.as_deref().unwrap(),
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
            fx.v_recon10.as_deref().unwrap(),
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
        let rd = |plane_dir: usize| TxRdArgs {
            spatial_dist: true,
            intra_dir: plane_dir,
            coeff_rate_est_lvl: cfg.coeff_rate_est_lvl,
            tx_bias: frame.tx_bias,
            crop: uv_crop,
        };
        let u_out = tx_unit_hbd(
            &b.u_src10,
            cw,
            0,
            &u_pred,
            cw,
            0,
            cw,
            chh,
            tt,
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
            Some(&rd(0)),
        );
        let v_out = tx_unit_hbd(
            &b.v_src10,
            cw,
            0,
            &v_pred,
            cw,
            0,
            cw,
            chh,
            tt,
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
            Some(&rd(0)),
        );
        (u_out, v_out)
    };
    let chroma_eval = |fx: &FunnelCtx<'_>, uv: u8, uv_delta: i8| -> (TxUnitOut, TxUnitOut) {
        let mut u_pred = vec![0u8; cw * chh];
        let mut v_pred = vec![0u8; cw * chh];
        predict_unit(
            fx.u_recon,
            fx.c_stride,
            ccx,
            ccy,
            cw,
            chh,
            uv,
            uv_delta,
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
            uv,
            uv_delta,
            FI_NONE,
            &uv_geom,
            cfg.edge_filter,
            filt_type_uv,
            &mut v_pred,
        );
        let tt = uv_tx_type(uv, cw, chh);
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
        (u_out, v_out)
    };

    // No-palette flag pricing for this leaf (C svt_aom_allow_palette on the
    // LUMA bsize; both dims <= 64 and not 4x4/4x8/8x4).
    let allow_pal = svtav1_entropy::context::allow_palette(cfg.allow_sct, w, h);
    // C svt_aom_get_palette_mode_ctx (rd_cost.c:583): neighbor palette-mode
    // ctx (above+left count of palette-coded neighbours, 0..=2), read from
    // the MD decision grid (stamped by commit_leaf in coding order). 0 until
    // a palette candidate wins a neighbour => byte-identical for non-screen
    // content, where no leaf ever carries a palette.
    let pal_mode_ctx = fx.ectx.palette_neighbor_ctx(abs_x, abs_y);
    let pal_y_no = if allow_pal {
        rates.palette_y_no[svtav1_entropy::context::palette_bsize_ctx(w, h)][pal_mode_ctx] as u64
    } else {
        0
    };
    // Regular (y-palette-off) candidates price the [0] row; the palette
    // candidate prices the [1] row (use_palette_y=1) via pal_uv_no_y1 below.
    let pal_uv_no = if allow_pal {
        rates.palette_uv_no[0] as u64
    } else {
        0
    };
    let pal_uv_no_y1 = if allow_pal {
        rates.palette_uv_no[1] as u64
    } else {
        0
    };

    let mut ind_uv: Option<[(u8, i8); 13]> = None;
    // C: at ind_uv_last_mds == 0 (the M0/M1 chroma config) the independent
    // uv search runs BEFORE MDS0 (product_coding_loop.c:9260, ind_uv_avail=1
    // at injection) so every candidate's MDS0 fast cost prices its FINAL uv
    // pair — which drives the NIC survivor order. The table itself is
    // candidate-independent, so building it here is timing-exact.
    if has_uv && let Some(ind_uv_independent) = cfg.ind_uv_independent {
        // C `search_best_independent_uv_mode` (product_coding_loop.c:7778),
        // chroma_level 1/2 (ind_uv_last_mds 0/1): a FULL independent uv
        // search over ALL uv modes, not just the survivors' uv-follows-luma
        // modes. `perform_ind_uv_search_last_mds` (:7899) is true whenever
        // an intra candidate survived (skip_ind_uv_if_only_dc = 0 here, and
        // the inter-vs-intra arm is I-slice-dead) — so it always runs for
        // our intra blocks.
        let uv_nic = ind_uv_independent as u64;

        // 1. Inject ALL uv modes DC..mode_end with angle deltas, in the C
        //    uv_mode-then-delta order (:7807-7849): angular_pred_level >= 4
        //    skips D45..D67; directional modes get 7 deltas (-3..3) when
        //    use_angle_delta && level <= 2, else 1; |1|/|2| are dropped at
        //    level >= 2 (all inert for M0/M1 at angular_pred_level 1).
        let mut uv_cands: Vec<(u8, i8)> = Vec::new();
        for uvm in 0u8..=cfg.mode_end {
            let directional = matches!(uvm, 1..=8);
            if directional && ((cfg.angular_level >= 4 && uvm >= 3) || cfg.angular_level == 0) {
                continue;
            }
            let ndelta = if use_angle && directional && cfg.angular_level <= 2 {
                7
            } else {
                1
            };
            for k in 0..ndelta {
                let d: i8 = if ndelta == 1 { 0 } else { k as i8 - 3 };
                if cfg.angular_level >= 2 && matches!(d, -2 | -1 | 1 | 2) {
                    continue;
                }
                uv_cands.push((uvm, d));
            }
        }

        // 2. Fast loop: SAD (u + v) per candidate, NO rate at this stage
        //    (product_coding_loop.c:7604-7674). C's `mds0_dist_type` is
        //    zero-initialized = SAD (never assigned in `Source/Lib`), so BOTH
        //    bit depths score plain SAD — bd8 `svt_nxm_sad_kernel`, bd10
        //    `sad_16b_kernel` — NOT the `vf` variance. The sort order (which
        //    candidates enter the full loop) is decided HERE, so the metric
        //    must match C's SAD or a different candidate SET is admitted.
        // bd10 (task #94, root #1): C runs this fast loop at `hbd_md` too — the
        // 10-bit prediction scored by `sad_16b_kernel` on the 10-bit source.
        let mut u_pred = alloc::vec![0u8; cw * chh];
        let mut v_pred = alloc::vec![0u8; cw * chh];
        let mut u_pred10 = alloc::vec![0u16; cw * chh];
        let mut v_pred10 = alloc::vec![0u16; cw * chh];
        let mut fast: Vec<(u64, usize)> = Vec::with_capacity(uv_cands.len());
        for (idx, &(uvm, uvd)) in uv_cands.iter().enumerate() {
            // Both bit depths score SAD (`mds0_dist_type` default 0 = SAD);
            // it is the fast-loop sort key below.
            let fast_dist = match bd10_rd.as_ref() {
                Some(b) => {
                    predict_unit_hbd(
                        fx.u_recon10.as_deref().unwrap(),
                        fx.c_stride,
                        ccx,
                        ccy,
                        cw,
                        chh,
                        uvm,
                        uvd,
                        FI_NONE,
                        &uv_geom,
                        cfg.edge_filter,
                        filt_type_uv,
                        &mut u_pred10,
                        b.bd,
                    );
                    predict_unit_hbd(
                        fx.v_recon10.as_deref().unwrap(),
                        fx.c_stride,
                        ccx,
                        ccy,
                        cw,
                        chh,
                        uvm,
                        uvd,
                        FI_NONE,
                        &uv_geom,
                        cfg.edge_filter,
                        filt_type_uv,
                        &mut v_pred10,
                        b.bd,
                    );
                    residual_sad_hbd(&b.u_src10, cw, 0, 0, &u_pred10, cw, chh)
                        + residual_sad_hbd(&b.v_src10, cw, 0, 0, &v_pred10, cw, chh)
                }
                None => {
                    predict_unit(
                        fx.u_recon,
                        fx.c_stride,
                        ccx,
                        ccy,
                        cw,
                        chh,
                        uvm,
                        uvd,
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
                        uvm,
                        uvd,
                        FI_NONE,
                        &uv_geom,
                        cfg.edge_filter,
                        filt_type_uv,
                        &mut v_pred,
                    );
                    residual_sad(fx.u_src, fx.c_stride, ccx, ccy, &u_pred, cw, chh)
                        + residual_sad(fx.v_src, fx.c_stride, ccx, ccy, &v_pred, cw, chh)
                }
            };
            fast.push((fast_dist, idx));
        }

        // 3. Sort by fast cost. C `sort_fast_cost_based_candidates`
        //    (product_coding_loop.c:1415, called by the ind-uv search at
        //    :7680) is a swap-on-`<` selection sort:
        //    `for i { for j>i { if cost[j] < cost[i] swap(i,j) } }`. It is NOT
        //    stable — a swap displaces the element at `i` down to `j`, so
        //    equal-cost candidates do NOT keep injection order, and which of a
        //    SAD tie group (e.g. the three `cbd=96` D45 deltas) lands inside
        //    `nfl` is decided by this exact ordering. BOTH bit depths must
        //    replicate C bit-for-bit. (The bd8 arm briefly kept a stable
        //    `sort_by_key`, believed byte-inert from the then-green gates —
        //    WRONG on real content: flat-chroma SAD tie groups straddle the
        //    nfl cut constantly, admitting a different full-loop SET. Two
        //    independent witnesses, same day:
        //    - CID22 1200348 512x512 q32 p0 at org=(192,128) 32x32 — C fully
        //      evaluates (V,-3) but never (V,0); the stable port did the
        //      opposite, flipping the coded chroma angle delta and cascading
        //      into every later chroma DC base in SB(1,1)+.
        //    - codec_wiki 512^2 p0 q32 (16x16 at mi(4,24)) — C's exchange
        //      order kept UV_SMOOTH inside the 32-survivor cut where the
        //      stable order kept an extra D113 delta, so the whole ind-uv
        //      table and every MDS0 fast cost pricing it diverged.)
        {
            let n = fast.len();
            for i in 0..n.saturating_sub(1) {
                for j in (i + 1)..n {
                    if fast[j].0 < fast[i].0 {
                        fast.swap(i, j);
                    }
                }
            }
        }

        // 4. Full-loop count: allintra path -> base is_highest_layer ? 16
        //    : 32 (:7919). Under OPT_USE_HL0_FLAT a still KF (temporal layer
        //    0, hierarchical_levels 0) has is_highest_layer = FALSE
        //    (pd_process.c:6212: `(tli == hl) && hl != 0`), so base = 32;
        //    scaled by uv_nic_scaling_num/16, min 1 (:7919-7925). UV_DC is
        //    always tested (:7927-7947); it is injected first (sorted index
        //    0 on the flat-chroma tie) so it is already within the first
        //    nfl, but the explicit force is kept for content where DC sorts
        //    late. -> nfl = 16 at M1 (uv_nic 8), 32 at M0 (uv_nic 16).
        let mut nfl = div_round(32 * uv_nic, 16).max(1) as usize;
        nfl = nfl.min(uv_cands.len()).max(1);
        let mut set: Vec<(u8, i8)> = fast.iter().take(nfl).map(|&(_, i)| uv_cands[i]).collect();
        if !set.iter().any(|&(m, _)| m == 0) {
            set.push((0, 0));
        }

        // 5. Full loop: coeff_rate + SSD distortion per uv candidate
        //    (:7949-8003).
        let mut uv_rd: Vec<(u8, i8, u64, u64)> = Vec::with_capacity(set.len());
        for &(uvm, uvd) in &set {
            // bd10 (root #1): the full loop is `svt_aom_full_loop_uv` at
            // `hbd_md` (product_coding_loop.c:7523 full_lambda, 10-bit pred/
            // residual/distortion), same as the mds3-uv fix. bd8 keeps the u8
            // `chroma_eval` (the `None` arm is the original code).
            let (bits, dist) = match bd10_rd.as_ref() {
                Some(b) => {
                    let (u_out, v_out) = chroma_eval10(fx, b, uvm, uvd);
                    (
                        u_out.bits as u64 + v_out.bits as u64,
                        u_out.dist + v_out.dist,
                    )
                }
                None => {
                    let (u_out, v_out) = chroma_eval(fx, uvm, uvd);
                    (
                        u_out.bits as u64 + v_out.bits as u64,
                        u_out.dist + v_out.dist,
                    )
                }
            };
            uv_rd.push((uvm, uvd, bits, dist));
            #[cfg(feature = "std")]
            if crate::dbgenv::nsqdbg() && crate::depth_refine::nsqdbg_here(abs_x, abs_y) {
                eprintln!(
                    "NSQDBG UVRD mi=({},{}) {}x{} uv={uvm} uvd={uvd} bits={bits} dist={dist}",
                    abs_y / 4,
                    abs_x / 4,
                    w,
                    h,
                );
            }
        }

        // 6. Per luma mode: best uv by RD with the uv rate conditioned on
        //    the (real) luma mode (:8005-8039). All luma modes DC..mode_end
        //    get an entry (no directional skip at angular_pred_level 1); the
        //    rewrite below reads only the surviving luma modes.
        // bd10 (root #1): C prices this compare with the SAME full_lambda the
        // 10-bit full loop used (`full_lambda_md[EB_10_BIT_MD]`, :7523/:7994),
        // matching the 10-bit `uv_rd` above; bd8 keeps the u8 `lambda`.
        let uv_lambda = bd10_rd.as_ref().map_or(lambda, |b| b.lambda);
        let mut table = [(0u8, 0i8); 13];
        for luma in 0..=(cfg.mode_end as usize) {
            let mut best_cost = u64::MAX;
            for &(uvm, uvd, bits, dist) in &uv_rd {
                let mut fcr2 = rates.uv[cfl_allowed][luma][uvm as usize] as u64;
                if use_angle && matches!(uvm, 1..=8) {
                    fcr2 += rates.angle[uvm as usize - 1][(3 + uvd) as usize] as u64;
                }
                if uvm == 0 {
                    fcr2 += pal_uv_no; // rd_cost.c:514 (inside uv fast rate)
                }
                let cost = rdcost(uv_lambda, bits + fcr2, dist);
                if cost < best_cost {
                    best_cost = cost;
                    table[luma] = (uvm, uvd);
                }
            }
        }
        ind_uv = Some(table);
    }
    #[cfg(feature = "std")]
    if crate::dbgenv::nsqdbg()
        && crate::depth_refine::nsqdbg_here(abs_x, abs_y)
        && let Some(t) = &ind_uv
    {
        eprintln!(
            "NSQDBG UVTAB mi=({},{}) {}x{} t={:?}",
            abs_y / 4,
            abs_x / 4,
            w,
            h,
            t
        );
    }
    let fi_elig = cfg.filter_intra && fi_allowed_bsize;
    let mut cand_modes: Vec<(u8, i8, u8)> = Vec::new();
    if dc_only {
        // eff-M9 dc_cand_only injection: exactly {DC_PRED}, no filter-intra.
        cand_modes.push((0, 0, FI_NONE));
    } else {
        for mode in 0..=cfg.mode_end {
            let directional = matches!(mode, 1..=8);
            // directional_mode_skip_mask at angular_pred_level >= 4 masks
            // D45_PRED (3) .. D67_PRED (8) — V/H stay
            // (inject_intra_candidates, mode_decision.c:3246-3250).
            if matches!(mode, 3..=8) && cfg.angular_level >= 4 {
                continue;
            }
            if directional && cfg.angular_level <= 2 && use_angle {
                for d in -3i8..=3 {
                    if cfg.angular_level >= 2 && matches!(d, -2 | -1 | 1 | 2) {
                        continue;
                    }
                    cand_modes.push((mode, d, FI_NONE));
                }
            } else {
                cand_modes.push((mode, 0, FI_NONE));
            }
        }
    }
    if fi_elig && !dc_only {
        // Inject FILTER_DC_PRED..max_filter_intra_mode (each is a DC_PRED
        // block carrying filter_intra_mode 0..N). fi_max 0 = FILTER_DC only
        // (M1..M6); fi_max 4 = all five filter-intra modes (M0, filter_intra
        // level 1). inject_filter_intra_candidates, mode_decision.c:3318-3330.
        for fi_mode in 0..=cfg.fi_max {
            cand_modes.push((0, 0, fi_mode));
        }
    }

    // C `mds0_use_hadamard_blk` (product_coding_loop.c:9473):
    //
    //     ctx->mds0_use_hadamard_blk =
    //         ctx->mds0_use_hadamard_sb && fast_candidate_total_count > 1;
    //
    // `mds0_use_hadamard_sb` is true on the all-intra path
    // (enc_mode_config.c:8148, svt_aom_sig_deriv_enc_dec_allintra), so the live
    // term is the injected-candidate count. When it is 1, C's `fast_loop_core`
    // takes the VARIANCE arm (:1296-1306) instead of `hadamard_path` (:1283) —
    // both then shift by 4 and feed the same fast cost. At preset >= 9 the
    // `dc_only` gate injects exactly {DC_PRED}, so C runs NO Hadamard there at
    // all: profiling C at 512x512 and 1024x1024 preset 10 found ZERO samples in
    // any hadamard/satd symbol across 7,126 and 19,073 samples respectively,
    // while svt_aom_variance*_neon_dotprod appeared in both
    // (benchmarks/perf_class_attrib_2026-08-13.meta). The port was computing the
    // Hadamard SATD unconditionally — 4.8 % (512^2) / 5.1 % (1024^2) of its
    // whole frame at p10.
    //
    // `fast_candidate_total_count` in C counts EVERY injected candidate, and C
    // injects all of them before `md_stage_0` runs. This funnel interleaves
    // injection with evaluation, so the palette and intra-BC candidate counts
    // are not knowable here (the palette count is an output of the k-means
    // search below). The count is therefore OVER-approximated: whenever palette
    // or IBC injection can run at all, the Hadamard arm is kept. That direction
    // is byte-safe by domination — it can only preserve the pre-existing
    // behaviour on blocks where C would have used variance, which is exactly
    // what shipped and passed 168/168 byte identity before this change; it can
    // never take the variance arm on a block where C takes the Hadamard one.
    let palette_can_inject =
        svtav1_entropy::context::allow_palette(cfg.allow_sct, w, h) && cfg.palette_level > 0;
    let mds0_use_hadamard = cand_modes.len() > 1 || palette_can_inject || cfg.allow_intrabc;

    let mut cands: Vec<Cand> = Vec::with_capacity(cand_modes.len());
    // MDS0 with `prune_using_best_mode` (product_coding_loop.c:1680-1737):
    // candidates are evaluated in injection order; the running best REGULAR
    // (class-0, non-filter-intra) mode by fast cost is tracked and used to
    // SKIP later candidates — H when V is currently best, SMOOTH when DC is
    // still best. Skipped candidates never get a fast cost (never enter the
    // pool). At M6 (prune off) every candidate is evaluated, identical to
    // the original funnel.
    let mut best_reg_cost = u64::MAX;
    let mut best_reg_mode: i32 = -1;
    for &(mode, delta, fi) in &cand_modes {
        if cfg.prune_best_mode && fi == FI_NONE {
            // intra_mode_end SMOOTH >= H_PRED, so the gate is armed.
            if mode == 2 && best_reg_mode == 1 {
                continue; // V better than DC -> skip H
            }
            if mode == 9 && best_reg_mode == 0 {
                continue; // DC still best -> skip SMOOTH
            }
        }
        // C injection (inject_intra_candidates / inject_filter_intra_candidates,
        // mode_decision.c:3286-3292): uv = ind_uv_avail ? best_uv_mode[map]
        // : intra_luma_to_chroma[map], angle_uv = ind_uv_avail ?
        // best_uv_angle[map] : angle_y — with map = fimode_to_intramode[fi]
        // for FILTER candidates (their coded luma mode is DC, but the chroma
        // follows the fi-mapped DIRECTION). ind_uv_avail at injection is 1
        // exactly for the ind_uv_last_mds==0 (independent) presets, whose
        // table was built above; the ind_uv_mds3 presets stay on the
        // luma_to_chroma mapping here and rewrite at MDS3 (C :7063).
        let map_mode = if fi != FI_NONE {
            FIMODE_TO_INTRAMODE[fi as usize]
        } else {
            mode
        };
        // At ind_uv_last_mds==1 (M1) the C search hasn't run yet at
        // injection time (`ind_uv_avail` = 0, site :9477 is pre-MDS3), so
        // candidates inject uv-follows-luma and only the MDS3 rewrite
        // applies the table.
        let (uv, uv_delta) = match &ind_uv {
            Some(tbl) if !cfg.ind_uv_last_mds1 => tbl[map_mode as usize],
            _ => (uv_from_y(map_mode), if fi != FI_NONE { 0 } else { delta }),
        };
        let mut pred = vec![0u8; w * h];
        predict_unit(
            y_recon,
            y_stride,
            abs_x,
            abs_y,
            w,
            h,
            mode,
            delta,
            fi,
            &y_geom,
            cfg.edge_filter,
            filt_type_y,
            &mut pred,
        );
        // [SVT_HDR_MODE] complex-hvs: plain whole-block spatial SSD, no
        // shift (C fast_loop_core SSD arm). SATD path shifts << 4 below.
        // PORT-NOTE(unverified): fork mds0 SSD fast cost vs C — verify by
        // a C-side fast_loop_core dump once the C hybrid carries the
        // fork's set_mds0_controls case 3 (the hybrid currently assert(0)s
        // on mds0_level 3; see docs/HDR-ON-4.2.md complex-hvs row).
        let satd = if frame.mds0_ssd {
            let mut sse: u64 = 0;
            for r in 0..h {
                let srow = y_src_off + r * y_src_stride;
                for c in 0..w {
                    let d = i64::from(y_src[srow + c]) - i64::from(pred[r * w + c]);
                    sse += (d * d) as u64;
                }
            }
            sse
        } else if mds0_use_hadamard {
            hadamard_satd(y_src, y_src_stride, y_src_off, &pred, w, h)
        } else {
            // C fast_loop_core's variance arm (product_coding_loop.c:1296-1302):
            // `fn_ptr->vf(pred, pred_stride, src, src_stride, &sse)` with
            // `fn_ptr = &svt_aom_mefn_ptr[bsize]`, i.e. svt_aom_variance{W}x{H}.
            // Argument order is (pred, src); the metric is symmetric in the two
            // buffers (sse is, and only sum^2 is used), so this matches.
            u64::from(svtav1_dsp::variance::variance_diff(
                &pred,
                w,
                &y_src[y_src_off..],
                y_src_stride,
                w,
                h,
            ))
        };

        let mut flr = rates.kf_y[above_ctx][left_ctx][mode as usize] as u64;
        if use_angle && matches!(mode, 1..=8) {
            flr += rates.angle[mode as usize - 1][(3 + delta) as usize] as u64;
        }
        if fi_elig && mode == 0 {
            flr += rates.fi_flag[bsize_idx][usize::from(fi != FI_NONE)] as u64;
            if fi != FI_NONE {
                flr += rates.fi_mode[fi as usize] as u64;
            }
        }
        // No-palette y flag (rd_cost.c:579-585): every DC-coded candidate
        // (fi included) prices palette_ymode_fac_bits[bctx][mode_ctx][0]
        // (via pal_y_no, computed above with the neighbour mode ctx) when
        // allow_palette. pal_y_no is 0 when palette is disallowed.
        if mode == 0 {
            flr += pal_y_no;
        }
        // No-intrabc flag (rd_cost.c:629-631, IBC chunk 3): on an IBC frame
        // EVERY non-IBC candidate's luma rate carries intrabc_fac_bits[0]
        // (the use_intrabc=0 flag the writer codes per block). 0-cost
        // structurally when !allow_intrabc (the C fill is gated the same).
        if cfg.allow_intrabc {
            flr += rates.intrabc_fac_bits[0] as u64;
        }
        let mut fcr = if has_uv {
            rates.uv[cfl_allowed][mode as usize][uv as usize] as u64
        } else {
            // C fast cost: chroma_rate only when ctx->has_uv
            // (av1_intra_fast_cost, rd_cost.c:619).
            0
        };
        if has_uv && use_angle && matches!(uv, 1..=8) {
            fcr += rates.angle[uv as usize - 1][(3 + uv_delta) as usize] as u64;
        }
        if has_uv && uv == 0 {
            fcr += pal_uv_no; // rd_cost.c:514 (inside uv fast rate)
        }
        // bd10 mode funnel (task #94): when the bd10 recon canvas is present,
        // score this candidate's MDS0 fast cost at TRUE 10-bit — predict from
        // the 10-bit canvas, SATD the 10-bit residual (`y_src<<2 - pred10`),
        // with the bd10 fast lambda. This re-orders the survivor (C's bd10
        // winner). The rate (flr+fcr) is bit-depth-independent. The u8 `pred`
        // and `satd` above are still computed (MDS1/MDS3 reuse `cand.pred`);
        // only the fast COST switches. `None` (bd8) is the exact u8 path.
        // Diagnostic-only (read by the std-gated NSQDBG PFAST dump below).
        #[cfg(feature = "std")]
        let mut dbg_satd10: u64 = 0;
        #[cfg(feature = "std")]
        let mut dbg_pred0: u16 = 0;
        // The 10-bit prediction is RETAINED (`cand.pred10`) — MDS1/MDS3 need it
        // as their depth-0 predictor, exactly as they reuse the u8 `cand.pred`.
        // It used to be dropped here because only MDS0 ran at bd10.
        let mut pred10: Vec<u16> = Vec::new();
        let fast_cost = match fx.y_recon10.as_deref() {
            Some(canvas10) => {
                pred10 = vec![0u16; w * h];
                predict_unit_hbd(
                    canvas10,
                    y_stride,
                    abs_x,
                    abs_y,
                    w,
                    h,
                    mode,
                    delta,
                    fi,
                    &y_geom,
                    cfg.edge_filter,
                    filt_type_y,
                    &mut pred10,
                    frame.bit_depth,
                );
                let satd10 = hadamard_satd_hbd(&blk_y_src10, w, 0, &pred10, w, h);
                #[cfg(feature = "std")]
                {
                    dbg_satd10 = satd10;
                    dbg_pred0 = pred10[0];
                }
                rdcost(lambda_bd10_fast, flr + fcr, satd10 << 4)
            }
            None => rdcost(
                lambda,
                flr + fcr,
                if frame.mds0_ssd { satd } else { satd << 4 },
            ),
        };
        #[cfg(feature = "std")]
        if crate::dbgenv::canddbg() && crate::depth_refine::nsqdbg_here(abs_x, abs_y) {
            eprintln!(
                "NSQDBG PFAST mi=({},{}) {}x{} mode={} fi={} delta={} uv={} uvd={} flr={} fcr={} satd={} satd10={} pred10_0={} fast={}",
                abs_y / 4,
                abs_x / 4,
                w,
                h,
                mode,
                fi,
                delta,
                uv,
                uv_delta,
                flr,
                fcr,
                satd,
                dbg_satd10,
                dbg_pred0,
                fast_cost,
            );
        }
        // C updates best_reg_intra_mode after fast_loop_core for regular
        // class-0 candidates when prune is armed (line 1727).
        if cfg.prune_best_mode && fi == FI_NONE && fast_cost < best_reg_cost {
            best_reg_cost = fast_cost;
            best_reg_mode = mode as i32;
        }
        cands.push(Cand {
            mode,
            delta,
            fi,
            uv,
            uv_delta,
            pred,
            pred10,
            flr,
            fcr,
            fast_cost,
            full_cost: u64::MAX,
            mds3_cost_ssim: u64::MAX,
            mds1_has_coeff: false,
            tx_depth: 0,
            txb_q: Vec::new(),
            txb_eob: Vec::new(),
            txb_cul: Vec::new(),
            txb_type: Vec::new(),
            y_recon: Vec::new(),
            y_recon10: Vec::new(),
            u_recon10: Vec::new(),
            v_recon10: Vec::new(),
            y_recon_d0: Vec::new(),
            y_bits: 0,
            y_dist: 0,
            u_q: Vec::new(),
            v_q: Vec::new(),
            u_eob: 0,
            v_eob: 0,
            u_cul: 0,
            v_cul: 0,
            u_recon: Vec::new(),
            v_recon: Vec::new(),
            cfl_alpha_idx: 0,
            cfl_alpha_signs: 0,
            palette: None,
            ibc: None,
            mds3_cost: u64::MAX,
            block_has_coeff: false,
            total_rate: 0,
            full_dist: 0,
        });
    }
    // ---- inject_palette_candidates (mode_decision.c:3356-3406) ----
    // C order: regular+fi intra first, palette after (IBC would follow).
    // PORT-NOTE(unverified): C classes palette CAND_CLASS_3 with its own
    // MDS lanes/pool + class dist-to-cost th 50 (enc_mode_config.c:6775);
    // this funnel is single-class, so palette candidates share the one
    // pool — near-tie survivor sets can differ from C. Verify on the
    // EPICA cells; if a cell diverges on survivor membership, split the
    // pool per class. Neighbor state (mode ctx `pal_mode_ctx` + color cache
    // `pal_cache`) is read from the MD decision grid (stamped by commit_leaf
    // in coding order); both are 0/empty for blocks with no palette
    // neighbours — always true for non-screen content — so those stay
    // byte-identical to the pre-neighbour stub.
    // PALETTE AT 10 BITS (task #94 / #71). C has ONE palette search
    // parameterized by `is16bit` (palette.c:391-399): it reads
    // `pcs->input_frame16bit` instead of `enhanced_pic`, swaps
    // `svt_av1_count_colors` for `svt_av1_count_colors_highbd`, clips centroids
    // with `clip_pixel_highbd` (:310-312), widens the cache-snap threshold by
    // `<< (bit_depth - 8)` (:265), and codes the colour literals at
    // `encoder_bit_depth` (entropy_coding.c:4369, rd_cost.c:600).
    //
    // This funnel used to gate palette injection OUT at bd10 entirely
    // (`!bd10_funnel`), because a surviving palette candidate reached
    // `tx_unit_hbd` with only a u8 prediction and panicked. That was a graceful
    // stand-in for a crash, but its parity cost was never measured: since
    // `bd10_funnel` is true for EVERY 64-aligned bd10 4:2:0 frame at every
    // preset, the port offered ZERO palette candidates where C codes palette
    // blocks, so those leaves resolved to ordinary intra. MEASURED on the
    // production corpus (benchmarks/imazen26_sweep_2026-07-24_summary.tsv):
    // preset 6 bd8 = 515/515 byte-identical but preset 6 bd10 = 380/515, and
    // the 135 failing cells are EXACTLY the eight screen-detecting content
    // classes — the whole M6 bd10 gap was this gate. (At M6 IBC is already off,
    // so M6 bd10 is a pure palette divergence; M0 adds IBC on top.)
    //
    // Now the search runs at the real depth and the candidate carries BOTH
    // predictions: `pred` (u8, for the MDS1/MDS3 u8 stages) and `pred10` (u16,
    // what `tx_unit_hbd` needs). Palette prediction is a position-only colour
    // substitution with no neighbour edges (enc_intra_prediction.c:631-651), so
    // the 10-bit form is the same index map through the 10-bit colours — no new
    // predictor kernel is required, which is why this is a small change rather
    // than an hbd-predictor port.
    //
    // C's `eval_intrabc` narrowing scope (mode_decision.c:3587-3594): the
    // palette-hint coupling reads whether the palette injection RAN for
    // this block and whether it produced any candidate.
    let palette_ran =
        svtav1_entropy::context::allow_palette(cfg.allow_sct, w, h) && cfg.palette_level > 0;
    let cands_before_palette = cands.len();
    if palette_ran {
        let ctrls = crate::palette::PaletteCtrls::for_level(cfg.palette_level);
        let bctx = svtav1_entropy::context::palette_bsize_ctx(w, h);
        // Neighbour palette color cache (C svt_get_palette_cache_y): merged
        // above+left palette colours, feeding BOTH the k-means centroid snap
        // (optimize_palette_colors, opt_colors=TRUE) INSIDE the search AND
        // the cache-aware color cost below. Empty => bit-identical search +
        // cost (the n_cache==0 fast paths in index_color_cache /
        // optimize_palette_colors / palette_color_cost_y).
        let pal_cache = crate::pipeline::palette_cache(&*fx.ectx, abs_x, abs_y);
        // C svt_aom_write_uniform_cost (entropy_coding.c:4308):
        // truncated-binary literal bits << AV1_PROB_COST_SHIFT(9).
        let uniform_cost = |n: usize, v: u8| -> u64 {
            let l = usize::BITS - n.leading_zeros(); // get_unsigned_bits
            if l == 0 {
                return 0;
            }
            let m = (1usize << l) - n;
            let bits = if (v as usize) < m { l - 1 } else { l };
            (bits as u64) << 9
        };
        // The funnel receives the source as (plane, stride, block offset);
        // decompose the offset back to plane coords for the search.
        // C picks the source plane by `is16bit = ctx->hbd_md > 0`
        // (palette.c:391-399). `blk_y_src10` is this block's 10-bit luma at
        // stride `w` (the real u16 samples when the caller entered through a
        // `*_hbd` entry point, else the `u8 << 2` widening), so the hbd search
        // reads exactly what C's `input_frame16bit` would give it.
        //
        // C SEARCHES over the IN-FRAME part of the block, not the whole block:
        // `search_palette_luma` (palette.c:401-403) takes its `rows`/`cols`
        // from `svt_aom_get_block_dimensions`' `rows_within_bounds` /
        // `cols_within_bounds` (palette.c:217-245), and feeds exactly those to
        // `svt_av1_count_colors` (:409-411), to the `data[]` / `lb` / `ub` fill
        // (:427-439) and to `av1_calc_indices` (:323) — the index map is then
        // edge-REPLICATED out to the nominal block (`extend_palette_color_map`,
        // :324). Passing the full block instead lets the padded rows/columns
        // beyond the picture edge vote in the colour histogram, the
        // dominant-colour scan, the k-means seed range `[lb, ub]` and every
        // k-means iteration — so a straddling block gets DIFFERENT palette
        // colours than C's, and the colour literals desync the bitstream from
        // C's (issue #15).
        //
        // The RATE side (`map_rows`/`map_cols` below) and the PACK side
        // (`pipeline.rs`, `write_palette_map_tokens`) already cropped; only the
        // SEARCH did not, so the three sites disagreed with each other about
        // which block a palette candidate describes.
        //
        // Identical to `w`/`h` on every 64-aligned frame, where nothing
        // straddles — which is why this was invisible to every gate until the
        // unaligned real-content scan crossed the two axes.
        let pal_rows = h.min(frame.frame_h_px.saturating_sub(abs_y));
        let pal_cols = w.min(frame.frame_w_px.saturating_sub(abs_x));
        let pal_cands = if bd10_funnel {
            crate::palette::search_palette_luma_hbd(
                &blk_y_src10,
                w,
                pal_rows,
                pal_cols,
                w,
                h,
                &ctrls,
                &pal_cache,
                frame.base_qindex,
                u32::from(frame.bit_depth),
            )
        } else {
            crate::palette::search_palette_luma(
                y_src,
                y_src_stride,
                y_src_off % y_src_stride,
                y_src_off / y_src_stride,
                pal_rows,
                pal_cols,
                w,
                h,
                &ctrls,
                &pal_cache,
                frame.base_qindex,
            )
        };
        for pc in pal_cands {
            let n = pc.colors.len();
            // Substitution prediction (enc_intra_prediction.c:631-651): a
            // position-only colour lookup, no neighbour edges. At bd10 the
            // colours are 10-bit, so `pred10` is the authoritative prediction
            // and the u8 `pred` is its MSB-truncated twin, kept because the
            // MDS1/MDS3 u8 stages and `commit_leaf` still read `cand.pred`.
            let mut pred = vec![0u8; w * h];
            let mut pred10: Vec<u16> = if bd10_funnel {
                vec![0u16; w * h]
            } else {
                Vec::new()
            };
            let shift = u32::from(frame.bit_depth - 8);
            for (o, &idx) in pc.idx_map.iter().enumerate().take(w * h) {
                let c = pc.colors[idx as usize];
                if bd10_funnel {
                    pred10[o] = c;
                    pred[o] = (c >> shift) as u8;
                } else {
                    pred[o] = c as u8;
                }
            }
            // MDS0 fast distortion at the real depth, mirroring the regular
            // candidates' bd10 arm above (10-bit SATD against the 10-bit
            // source, u8 SATD otherwise).
            let satd = if bd10_funnel {
                hadamard_satd_hbd(&blk_y_src10, w, 0, &pred10, w, h)
            } else {
                hadamard_satd(y_src, y_src_stride, y_src_off, &pred, w, h)
            };
            // Luma rate: DC mode + fi-off flag (fi eligible blocks price it
            // for every DC candidate) + the palette slice (rd_cost.c:579-605
            // use_palette=1 arm): ymode YES + size + (0,0) uniform + colors
            // + map tokens.
            let r_mode = rates.kf_y[above_ctx][left_ctx][0] as u64;
            // C prices NO filter-intra flag on a palette candidate:
            // svt_aom_filter_intra_allowed (mode_decision.c:106) returns 0
            // whenever palette_size > 0, so the use_filter_intra syntax is
            // never written for a palette block (rd_cost.c pals the DC-mode
            // + palette rate only). The port was adding fi_flag[bsize][0]
            // here, over-pricing every palette candidate by that flag cost
            // (measured 1053 at EPICA 8x8) — a real, agent-verified rate
            // divergence vs C. Palette candidates get zero fi bits.
            let r_fi = 0u64;
            let _ = fi_elig; // (fi eligibility is a DC-candidate concept)
            let r_yes = rates.palette_y_yes[bctx][pal_mode_ctx] as u64;
            let r_size = rates.palette_ysize[bctx][n - 2] as u64;
            let r_uniform = uniform_cost(n, pc.idx_map[0]);
            // Colors (C svt_av1_palette_color_cost_y, palette.c:143-152):
            // one flag bit per neighbour-cache entry (n_cache) + delta-code
            // only the out-of-cache colours; av1_cost_literal shifts the
            // whole total by 9. index_color_cache splits pc.colors on the
            // neighbour cache — at n_cache==0 out == pc.colors, so this is
            // bit-identical to the former empty-cache all-colours cost.
            let mut pal_found = alloc::vec![false; pal_cache.len()];
            let mut pal_out = alloc::vec![0u16; pc.colors.len()];
            let n_out = crate::palette::index_color_cache(
                &pal_cache,
                &pc.colors,
                &mut pal_found,
                &mut pal_out,
            );
            // C passes `scs->static_config.encoder_bit_depth` here
            // (`svt_av1_palette_color_cost_y`, rd_cost.c:600) — the same width
            // the WRITER uses (entropy_coding.c:4369). A hardcoded 8 would
            // under-price every 10-bit palette candidate's colours by 2 bits
            // for the first literal (and shift the whole delta ladder), biasing
            // the palette-vs-regular RD tie.
            let r_colors = ((pal_cache.len() as u64)
                + crate::palette::delta_encode_bits(
                    &pal_out[..n_out],
                    u32::from(frame.bit_depth),
                    1,
                ) as u64)
                << 9;
            let mut map_bits = 0u64;
            // C prices the map over the IN-FRAME part of the block, not the
            // whole block: `get_palette_params_rate` (palette.c:569-580) fills
            // `params->rows` / `params->cols` from `svt_aom_get_block_dimensions`
            // -- the same `rows_within_bounds` / `cols_within_bounds` the PACK
            // side uses (entropy_coding.c:5083). Both sides must agree, or a
            // straddling palette block is priced over rows the writer never
            // emits and the RD tie moves.
            //
            // Identical to `w`/`h` unless the block straddles the aligned
            // extent, which only happens on a partial SB. Same numbers the
            // SEARCH above uses — ONE definition, because the search, the rate
            // and the pack disagreeing about the block's in-frame extent is
            // exactly the defect issue #15 turned out to be.
            let (map_rows, map_cols) = (pal_rows, pal_cols);
            crate::palette::color_map_wavefront(
                &pc.idx_map,
                w, // stride: the FULL block width, only the traversal shrinks
                map_rows,
                map_cols,
                n,
                |_i, _j, ctx, idx| {
                    map_bits += rates.palette_ycolor[n - 2][ctx][idx as usize] as u64;
                },
            );
            // Palette candidates flow through the same svt_aom_intra_fast_cost
            // else-arm tail as regular intra — the no-intrabc flag charge
            // (rd_cost.c:629-631) applies to them identically (IBC chunk 3).
            let r_ibc_no = if cfg.allow_intrabc {
                rates.intrabc_fac_bits[0] as u64
            } else {
                0
            };
            let flr = r_mode + r_fi + r_yes + r_size + r_uniform + r_colors + map_bits + r_ibc_no;
            #[cfg(feature = "std")]
            if crate::dbgenv::palbrk() && crate::depth_refine::nsqdbg_here(abs_x, abs_y) {
                eprintln!(
                    "NSQDBG PALBRK mi=({},{}) n={} mode={} fi={} yes={} size={} uniform={} colors={} map={} (63tok? map/512={})",
                    abs_y / 4,
                    abs_x / 4,
                    n,
                    r_mode,
                    r_fi,
                    r_yes,
                    r_size,
                    r_uniform,
                    r_colors,
                    map_bits,
                    map_bits / 512,
                );
                eprintln!(
                    "NSQDBG PALDATA mi=({},{}) n={} colors={:?} idxmap={:?}",
                    abs_y / 4,
                    abs_x / 4,
                    n,
                    pc.colors,
                    pc.idx_map,
                );
            }
            // Chroma: DC (palette-uv unsupported) with the y-palette-ON uv
            // flag row. C prices palette_uv_mode_fac_bits[1][0] here
            // (rd_cost.c:514-521, use_palette_y=1 because this candidate has a
            // luma palette). This is the ONLY leaf-funnel site that takes the
            // [1] row; every regular candidate keeps pal_uv_no ([0]). The port
            // formerly priced [0][0] here too, under-costing the palette
            // candidate's chroma flag (icdf 307 vs the correct 11280) and
            // biasing the palette-vs-regular RD tie toward palette — a #71
            // over-picking contributor (agent-confirmed via the triage drill).
            let (uv, uv_delta) = match &ind_uv {
                Some(tbl) if !cfg.ind_uv_last_mds1 => tbl[0],
                _ => (0u8, 0i8),
            };
            let mut fcr = if has_uv {
                rates.uv[cfl_allowed][0][uv as usize] as u64
            } else {
                0
            };
            if has_uv && use_angle && matches!(uv, 1..=8) {
                fcr += rates.angle[uv as usize - 1][(3 + uv_delta) as usize] as u64;
            }
            if has_uv && uv == 0 {
                fcr += pal_uv_no_y1; // [1][0]: this candidate's luma palette is on
            }
            let fast_cost = rdcost(
                lambda,
                flr + fcr,
                if frame.mds0_ssd { satd } else { satd << 4 },
            );
            #[cfg(feature = "std")]
            if crate::dbgenv::canddbg() && crate::depth_refine::nsqdbg_here(abs_x, abs_y) {
                eprintln!(
                    "NSQDBG PFAST mi=({},{}) {}x{} PAL n={} flr={} fcr={} satd={} fast={}",
                    abs_y / 4,
                    abs_x / 4,
                    w,
                    h,
                    n,
                    flr,
                    fcr,
                    satd,
                    fast_cost,
                );
            }
            cands.push(Cand {
                mds3_cost_ssim: u64::MAX,
                mode: 0,
                delta: 0,
                fi: FI_NONE,
                uv,
                uv_delta,
                pred,
                // The 10-bit substitution prediction (empty at bd8). This is
                // what `tx_unit_hbd` residuals against; it used to be
                // unconditionally empty, which is why a palette candidate
                // reaching the bd10 full-RD stage panicked and why palette was
                // gated out of the bd10 funnel entirely.
                pred10,
                flr,
                fcr,
                fast_cost,
                full_cost: u64::MAX,
                mds1_has_coeff: false,
                tx_depth: 0,
                txb_q: Vec::new(),
                txb_eob: Vec::new(),
                txb_cul: Vec::new(),
                txb_type: Vec::new(),
                y_recon: Vec::new(),
                y_recon10: Vec::new(),
                u_recon10: Vec::new(),
                v_recon10: Vec::new(),
                y_recon_d0: Vec::new(),
                y_bits: 0,
                y_dist: 0,
                u_q: Vec::new(),
                v_q: Vec::new(),
                u_eob: 0,
                v_eob: 0,
                u_cul: 0,
                v_cul: 0,
                u_recon: Vec::new(),
                v_recon: Vec::new(),
                cfl_alpha_idx: 0,
                cfl_alpha_signs: 0,
                palette: Some((pc.colors, pc.idx_map)),
                ibc: None,
                mds3_cost: u64::MAX,
                block_has_coeff: false,
                total_rate: 0,
                full_dist: 0,
            });
        }
    }

    // ---- inject_intra_bc_candidates (IBC chunk 8; mode_decision.c
    //      :3596-3618 gate + :3127-3163 injection + :2976-3126 search) ----
    // IBC AT 10 BITS (task #94 / #71). Formerly `&& !bd10_funnel`, on the
    // grounds that the IBC predictor was u8-only: at bd10 the frame header
    // still carried allow_intrabc while every block coded use_intrabc=0.
    // Decodable, but a guaranteed divergence from C wherever C picks a DV --
    // and the cost is far larger than the palette one it shipped alongside.
    // MEASURED on the gb82-sc screen corpus (512x512 centre crops, q20,
    // port vs real C at bd10, IBC gated out):
    //     terminal  p2  C 7611 B   port 13338 B   +75.2%
    //     terminal  p3  C 7889 B   port 13584 B   +72.2%
    //     windows95 p3  C 13398 B  port 13810 B    +3.1%
    // At bd8 the same cells code 890 / 362 IntraBC blocks, so this is the
    // whole of the delta on copy-friendly content.
    //
    // The DV SEARCH already ran at 8 bits and stays there -- that is C's own
    // asymmetry, not a shortcut (the search reads the source plane, the
    // predictor reads the recon; map SS A.6), and it is the arm the
    // c_parity_intrabc_search / _hash / _mvp differentials pin. What was
    // missing is only the COMPENSATION at 10 bits, which is now generic over
    // `ReconSample` (see intrabc_pred.rs: the bilinear closed forms are
    // provably identical for bd <= 10).
    if cfg.allow_intrabc
        && let (Some(ibc), Some(dvt)) = (fx.ibc, frame.dv_tables.as_ref())
    {
        let gate = fx.ibc_gate;
        let do_ibc = crate::intrabc::do_intra_bc_gate(
            &ibc.ctrls,
            palette_ran,
            (cands.len() - cands_before_palette) as u32,
            gate.is_part_n,
            w.max(h) as i32, // sq_size: only the (allintra-off) b4 gate reads it
            (false, false),  // parent_n0: b4_parent_gating is off at every level
            gate.sibling_n0,
        );
        if do_ibc {
            let mi_row = (abs_y / 4) as i32;
            let mi_col = (abs_x / 4) as i32;
            let grid_stride = ibc.mi_cols;
            let base = mi_row * grid_stride + mi_col;
            // C's MVP scan runs against the live mi state where the
            // CURRENT cell carries the block's own partition (the
            // `has_top_right` VERT_A read) — stamp it before building
            // the stack (commit will overwrite the cell either way).
            let mvp = fx.ibc_mvp.as_deref_mut().expect("ibc_mvp with ibc state");
            mvp[base as usize].partition = gate.partition;
            let stack = {
                let grid = crate::intrabc_mvp::MvpGrid {
                    entries: mvp,
                    stride: grid_stride,
                    base,
                };
                let bctx = crate::intrabc_mvp::derive_block_ctx(
                    mi_row,
                    mi_col,
                    c_bsize_index(w, h),
                    ibc.mi_rows,
                    ibc.mi_cols,
                    ibc.tile,
                    ibc.sb_mi_size,
                );
                crate::intrabc_mvp::generate_mvp_table_intra_frame(&grid, &bctx)
            };
            // dv_ref = nearest/near coercion + find_ref_dv fallback
            // (mode_decision.c:3019-3033); C stamps it back onto
            // ref_mv_stack[INTRA_FRAME][0].this_mv = cand->pred_mv[0].
            let dv_ref =
                crate::intrabc_mvp::compose_dv_ref(&stack, ibc.tile, ibc.sb_mi_size, mi_row);
            // Per-block hash query (square + size-gated), the bucket
            // fetched once and offered to both directions.
            let hash_eligible = crate::intrabc::hash_search_eligible(
                w as i32,
                h as i32,
                ibc.ctrls.max_block_size_hash,
            );
            let (bucket_entries, hv2) = if hash_eligible {
                let mut bufs = crate::intrabc_hash::BlockHashBuffers::default();
                let (hv1, hv2) = crate::intrabc_hash::get_block_hash_value(
                    &y_src[abs_y * y_src_stride + abs_x..],
                    y_src_stride,
                    w,
                    &mut bufs,
                );
                (
                    ibc.hash
                        .bucket(hv1)
                        .iter()
                        .map(|e| crate::intrabc::BlockHashEntry {
                            x: i32::from(e.x),
                            y: i32::from(e.y),
                            hash_value2: e.hash_value2,
                        })
                        .collect::<Vec<_>>(),
                    hv2,
                )
            } else {
                (Vec::new(), 0)
            };
            let buckets: [Option<&[crate::intrabc::BlockHashEntry]>; 2] = if hash_eligible {
                [Some(&bucket_entries), Some(&bucket_entries)]
            } else {
                [None, None]
            };
            let dvs = crate::intrabc::intra_bc_search(
                y_src, // SOURCE pixels (A.3 fact 1), frame-origin absolute
                y_src_stride,
                w as i32,
                h as i32,
                (w / 4) as i32,
                (h / 4) as i32,
                mi_row,
                mi_col,
                ibc.mi_rows,
                ibc.mi_cols,
                ibc.sb_mi_size,
                ibc.sb_size_log2_mi,
                ibc.sb_size_px,
                ibc.tile,
                dv_ref,
                &ibc.sites,
                &ibc.ctrls,
                ibc.sad_per_bit,
                ibc.error_per_bit,
                false, // approx_inter_rate: structurally 0 on allintra
                &ibc.search_tables,
                buckets,
                hv2,
            );
            // Diagnostic (SVTAV1_IBCDBG): what the DV search actually
            // returned for this block. Without it a "C codes IntraBC here
            // and the port does not" verdict cannot distinguish "the
            // search found no DV" from "it found one and the RD lost".
            #[cfg(feature = "std")]
            if crate::dbgenv::ibcdbg() && crate::depth_refine::nsqdbg_here(abs_x, abs_y) {
                eprintln!(
                    "NSQDBG IBCSEARCH mi=({},{}) {}x{} hash_elig={} bucket={} dv_ref=({},{}) ndv={} dvs={:?}",
                    abs_y / 4,
                    abs_x / 4,
                    w,
                    h,
                    hash_eligible,
                    bucket_entries.len(),
                    dv_ref.y,
                    dv_ref.x,
                    dvs.len(),
                    dvs,
                );
            }
            for dv in dvs {
                // Prediction: the RECON-domain block copy (the ONE
                // search-vs-predict asymmetry — map §A.6).
                let mut pred = vec![0u8; w * h];
                crate::intrabc_pred::predict_intrabc_luma(
                    y_recon, y_stride, abs_x, abs_y, w, h, dv, &mut pred,
                );
                // The SAME block copy on the 10-bit canvas. This is the
                // prediction `tx_unit_hbd` residuals against; leaving it
                // empty is what made an IBC candidate unrepresentable at
                // bd10 (and is why the injection was gated out).
                let mut pred10: Vec<u16> = Vec::new();
                if bd10_funnel {
                    pred10 = vec![0u16; w * h];
                    crate::intrabc_pred::predict_intrabc_luma(
                        fx.y_recon10.as_deref().unwrap(),
                        y_stride,
                        abs_x,
                        abs_y,
                        w,
                        h,
                        dv,
                        &mut pred10,
                    );
                }
                let satd = if bd10_funnel {
                    // Score MDS0 at the real depth, like every other
                    // candidate's bd10 arm above.
                    hadamard_satd_hbd(&blk_y_src10, w, 0, &pred10, w, h)
                } else if frame.mds0_ssd {
                    let mut sse: u64 = 0;
                    for r in 0..h {
                        let srow = y_src_off + r * y_src_stride;
                        for c in 0..w {
                            let d = i64::from(y_src[srow + c]) - i64::from(pred[r * w + c]);
                            sse += (d * d) as u64;
                        }
                    }
                    sse
                } else {
                    hadamard_satd(y_src, y_src_stride, y_src_off, &pred, w, h)
                };
                // svt_aom_intra_fast_cost use_intrabc arm (rd_cost.c
                // :531-545): rate = mv_bit_cost(dv, pred_dv, dv tables,
                // MV_COST_WEIGHT_SUB) + intrabc_fac_bits[1]; chroma 0.
                let (flr32, _) = crate::intrabc::intrabc_fast_cost_rates(
                    dv,
                    dv_ref,
                    dvt,
                    &rates.intrabc_fac_bits,
                );
                let flr = u64::from(flr32);
                let fast_cost = if bd10_funnel {
                    rdcost(lambda_bd10_fast, flr, satd << 4)
                } else {
                    rdcost(lambda, flr, if frame.mds0_ssd { satd } else { satd << 4 })
                };
                cands.push(Cand {
                    mode: 0, // DC_PRED (the coded neighbour-visible mode)
                    delta: 0,
                    fi: FI_NONE,
                    uv: 0, // UV_DC_PRED
                    uv_delta: 0,
                    pred,
                    pred10,
                    flr,
                    fcr: 0,
                    fast_cost,
                    full_cost: u64::MAX,
                    mds3_cost_ssim: u64::MAX,
                    mds1_has_coeff: false,
                    tx_depth: 0,
                    txb_q: Vec::new(),
                    txb_eob: Vec::new(),
                    txb_cul: Vec::new(),
                    txb_type: Vec::new(),
                    y_recon: Vec::new(),
                    y_recon10: Vec::new(),
                    u_recon10: Vec::new(),
                    v_recon10: Vec::new(),
                    y_recon_d0: Vec::new(),
                    y_bits: 0,
                    y_dist: 0,
                    u_q: Vec::new(),
                    v_q: Vec::new(),
                    u_eob: 0,
                    v_eob: 0,
                    u_cul: 0,
                    v_cul: 0,
                    u_recon: Vec::new(),
                    v_recon: Vec::new(),
                    cfl_alpha_idx: 0,
                    cfl_alpha_signs: 0,
                    palette: None,
                    ibc: Some((dv, dv_ref)),
                    mds3_cost: u64::MAX,
                    block_has_coeff: false,
                    total_rate: 0,
                    full_dist: 0,
                });
            }
        }
    }

    let ncand = cands.len();

    // -- MDS0 -> MDS1 MEMBERSHIP: C's replacement POOL, not a sort. --
    // md_stage_0 keeps candidates in max_buffers = md_stage_1_count + 1
    // slots (product_coding_loop.c:9342): the first max_buffers candidates
    // fill slots in PROCESSING order; every later candidate OVERWRITES the
    // current worst slot, where the victim scan is a FIRST-argmax with
    // strict `>` (:1692-1699) — so when two candidates TIE on fast cost at
    // the pool boundary, the EARLIER-processed one is the victim and the
    // LATER-processed one survives. After the last candidate the current
    // victim is discarded (cost set to MAX, :1708). A stable
    // sort + take(n1) keeps the EARLIER tied candidate instead — one
    // swapped survivor flips the whole SB downstream (1624307 q32 p2
    // mi(66,108): (mode5,d-1) vs (mode5,d+3) tied at fast 19175060; C
    // carries d+3, the sort carried d-1, the mds3 uv table then lost its
    // uv=2 row and tbl[SMOOTH] flipped H->SMOOTH).
    // NOTE: ties BETWEEN adjacent same-mode deltas share our injection
    // order with C; cross-mode/cross-iteration ties additionally depend on
    // C's two-iteration MDS0 order (regulars, then angular+fi, :1600) —
    // refine if a cell ever demands it.
    let (nic1, nic2, nic3) = nic_counts(frame.cli_qp, cfg.nic_num);
    // C runs md_stage_0's replacement pool PER CANDIDATE CLASS
    // (svt_aom_set_nics gives each class its own mds1_count, product_
    // coding_loop.c:1358; the pool + argmax-victim loop runs once per
    // cand_class_it, :9330-9360). On the allintra I-slice only two intra
    // classes are live: CAND_CLASS_0 (regular + fi intra) and
    // CAND_CLASS_3 (palette), and MD_STAGE_NICS gives BOTH base 64
    // (definitions.h:811), so each lane keeps up to `nic1` survivors and
    // MDS1/MDS3 evaluate the UNION (construct_best_sorted_arrays_md_
    // stage_3, :1455). A single shared pool let palette candidates
    // (huge SATD advantage on screen content) flood out the regular
    // survivors — EPICA p6 coded 2064 palette blocks vs C's 178. The
    // per-class dist-to-cost prune (product_coding_loop.c:1309) is INERT
    // here: allintra mds0_level == 0 (enc_mode_config.c:10042) sets
    // pruning_method_th = 0, so no class-th cut runs.
    let lane_pool = |lane: &[usize], cands: &[Cand], cap: usize| -> Vec<usize> {
        if lane.len() < cap {
            return lane.to_vec();
        }
        let argmax_first = |pool: &[usize]| -> usize {
            let mut vi = 0usize;
            let mut vc = cands[pool[0]].fast_cost;
            for (i, &ci) in pool.iter().enumerate().skip(1) {
                if cands[ci].fast_cost > vc {
                    vi = i;
                    vc = cands[ci].fast_cost;
                }
            }
            vi
        };
        let mut pool: Vec<usize> = Vec::with_capacity(cap);
        let mut victim = 0usize;
        for &ci in lane {
            if pool.len() < cap {
                pool.push(ci);
                if pool.len() == cap {
                    victim = argmax_first(&pool);
                }
            } else {
                pool[victim] = ci;
                victim = argmax_first(&pool);
            }
        }
        if pool.len() == cap {
            pool.remove(victim);
        }
        pool
    };
    // Class-partition preserving injection (processing) order within each
    // lane — the argmax-victim tie rule depends on it (the MDS0 pool
    // fix, 1624307). Regular (C0) then palette (C3), matching C's class
    // iteration order in construct_best_sorted_arrays.
    let has_palette_lane = cands.iter().any(|c| c.palette.is_some());

    // -- post_mds0_nic_pruning (product_coding_loop.c:7819) --
    let (qw, qwd) = qp_scale_factors(frame.cli_qp);
    // nic_level 1 (M0) sets mds1_cand_base_th_intra = (uint64_t)~0 (no mds1
    // cand pruning); the qp-scaled threshold stays saturated so the loop
    // below never prunes (guard avoids the base*qw overflow).
    let mds1_cand_th = if cfg.mds1_cand_base_th == u64::MAX {
        u64::MAX
    } else {
        div_round(cfg.mds1_cand_base_th * qw, qwd)
    };
    // C runs the intra dev-threshold prune PER CLASS (`for cidx`, :7840),
    // each relative to that class's OWN best fast cost (`cand_buff[cidx]
    // [0]`, :7845/:7868) — never the global best. The inter-class
    // (class_th) block :7847-7862 is inert on the I-slice: mds1_class_th
    // == ~0 (:7826) forces band_idx 0 (:7859), so no class is zeroed or
    // band-reduced. Running this prune over the sorted UNION with the
    // global best (as a single shared pool did) let palette — whose
    // screen-content fast cost sits far below any regular mode — prune
    // out every regular candidate (EPICA p6: 2064 palette blocks vs C's
    // 178, and every port-only block's ONLY MDS1 survivors were palette).
    // Prune each lane against its own class-best, then union + sort.
    let dev_prune = |sorted: &[usize], cands: &[Cand]| -> usize {
        if sorted.is_empty() {
            return 0;
        }
        let best = cands[sorted[0]].fast_cost;
        let mut count = 1usize;
        if best > 0 {
            while count < sorted.len() {
                let dev = (cands[sorted[count]].fast_cost - best) * 100 / best;
                // C: `mds1_cand_th / (rank ? rank * cand_count : 1)`
                // (product_coding_loop.c:7869) — rank 0 (M4 nic case 5)
                // means the raw threshold, NOT a zero divisor.
                let div = if cfg.mds1_rank_factor != 0 {
                    cfg.mds1_rank_factor * count as u64
                } else {
                    1
                };
                if dev >= mds1_cand_th / div {
                    break;
                }
                count += 1;
            }
        }
        count
    };
    // C `sort_fast_cost_based_candidates` (product_coding_loop.c:1415) over
    // each class's surviving pool. MUST be the C exchange sort, not a stable
    // sort: on exact fast-cost ties the two differ (see [`c_exchange_sort_by`]),
    // and the pool arrangement entering it is C's buffer arrangement
    // (lane_pool), so the tie order here is the one C's MDS1 walks.
    let sort_lane = |mut lane: Vec<usize>, cands: &[Cand]| -> Vec<usize> {
        c_exchange_sort_by(&mut lane, |i| cands[i].fast_cost);
        lane
    };
    // IBC chunk 8: C classes IntraBC CAND_CLASS_4 (mode_decision.c:3659)
    // — its own MDS0 pool + per-class prunes, exactly like palette's C3.
    // The class NIC bases are all 64 on I-slices (MD_STAGE_NICS,
    // definitions.h:811-813: {64, 0, 0, 64, 64}) so every lane shares the
    // same `cap` derivation; with <= 2 IBC candidates the C4 pool never
    // overflows in practice. Union order = class order (C0, C3, C4 —
    // construct_best_sorted_arrays), stable-sorted by fast cost.
    let has_ibc_lane = cands.iter().any(|c| c.ibc.is_some());
    // Multi-lane: `seg` carries the per-class segment lengths (k0, k3, k4)
    // of the CLASS-CONCATENATED `order` — C's cand_buff_indices structure.
    // C never merges the classes into one cost-sorted list: MDS1 evaluates
    // each class's own fast-sorted survivors (md_stage_1 per target_class),
    // and every later union (construct_best_sorted_arrays_md_stage_3,
    // :1454) is a pure concatenation in class order C0, C3, C4. The
    // previous union `sort_by_key(fast_cost)` matched C on all DISTINCT
    // costs but flipped cross-class tie/order corners (winner-scan ties,
    // uv_list order, mds1-best identity) — the screen multi-lane pins.
    let (order, seg): (Vec<usize>, Option<(usize, usize, usize)>) =
        if has_palette_lane || has_ibc_lane {
            let cap = (ncand as u32).min(nic1).max(1) as usize + 1;
            let lane0: Vec<usize> = (0..ncand)
                .filter(|&i| cands[i].palette.is_none() && cands[i].ibc.is_none())
                .collect();
            let lane3: Vec<usize> = (0..ncand).filter(|&i| cands[i].palette.is_some()).collect();
            let lane4: Vec<usize> = (0..ncand).filter(|&i| cands[i].ibc.is_some()).collect();
            // Per-class MDS0 replacement pool -> sort -> per-class dev-prune.
            let s0 = sort_lane(lane_pool(&lane0, &cands, cap), &cands);
            let s3 = sort_lane(lane_pool(&lane3, &cands, cap), &cands);
            let s4 = sort_lane(lane_pool(&lane4, &cands, cap), &cands);
            let k0 = dev_prune(&s0, &cands);
            let k3 = dev_prune(&s3, &cands);
            let k4 = dev_prune(&s4, &cands);
            // MDS1 evaluates the per-class survivors, class-concatenated in
            // class order (C0, C3, C4) — NOT cost-merged.
            let mut u: Vec<usize> = s0[..k0].to_vec();
            u.extend_from_slice(&s3[..k3]);
            u.extend_from_slice(&s4[..k4]);
            (u, Some((k0, k3, k4)))
        } else {
            // Single-class fast path (no palette candidates) — byte-identical
            // to the prior single-pool behaviour: pool -> sort -> dev-prune.
            let cap = (ncand as u32).min(nic1) as usize + 1;
            let all: Vec<usize> = (0..ncand).collect();
            let s = sort_lane(lane_pool(&all, &cands, cap), &cands);
            let k = dev_prune(&s, &cands);
            (s[..k].to_vec(), None)
        };
    // C mds0_best (:9518-9524): strict `<` over the per-class sorted heads
    // in class order (the head survives every dev-prune, count >= 1). On
    // the single-class path this is order[0]; on the multi-lane concat it
    // must be scanned (the concat head is C0's head, not the global min).
    let mds0_best_idx = match seg {
        Some((k0, k3, _)) => {
            let mut bi = order[0];
            let mut bc = u64::MAX;
            for head in [order.first(), order.get(k0), order.get(k0 + k3)]
                .into_iter()
                .flatten()
            {
                if cands[*head].fast_cost < bc {
                    bc = cands[*head].fast_cost;
                    bi = *head;
                }
            }
            bi
        }
        None => order[0],
    };
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

    // -- Sort survivors by full cost --
    // C `sort_full_cost_based_candidates` (product_coding_loop.c:1438, the
    // post-MDS1 :9561 sort). Same exchange-sort tie semantics as the fast
    // sort: on an exact full-cost TIE the survivor set into MDS3 depends on
    // it. Measured on clic 8426ed... 512^2 bd10 p6 q5, blk (472,208) 8x8:
    // MDS1 costs {DC+fi 2709194, SMOOTH 2710447, DC 2710447} in fast order
    // [SMOOTH, DC, DC+fi] — C's i=0/j=2 swap moves SMOOTH BEHIND the tied
    // DC, so C's MDS3 pair is {DC+fi, DC} while a stable sort keeps SMOOTH
    // -> the port coded SMOOTH and desynced the whole tail of the frame
    // (305 tree flips downstream of one tie).
    // Multi-lane: C sorts PER CLASS (`sort_full_cost_based_candidates(ctx,
    // md_stage_1_count[cidx], cand_buff_indices[cidx])` inside the per-class
    // MDS1 loop, :9560-9564) — never across the union. The class segments
    // stay contiguous; the mds1 best is the strict-`<` scan over the class
    // heads in class order (:9565-9569) — on a cross-class exact full-cost
    // tie the EARLIER class keeps the best (identity feeds the rank-staging
    // `mds0_best_idx == mds1_best_idx` compare and the class +3 arm).
    let mut order1: Vec<usize> = order[..n1].to_vec();
    let mds1_best_idx = match seg {
        Some((k0, k3, _)) => {
            let (a, rest) = order1.split_at_mut(k0);
            let (b, c) = rest.split_at_mut(k3);
            c_exchange_sort_by(a, |i| cands[i].full_cost);
            c_exchange_sort_by(b, |i| cands[i].full_cost);
            c_exchange_sort_by(c, |i| cands[i].full_cost);
            let mut bi = order1[0];
            let mut bc = u64::MAX;
            for head in [order1.first(), order1.get(k0), order1.get(k0 + k3)]
                .into_iter()
                .flatten()
            {
                if cands[*head].full_cost < bc {
                    bc = cands[*head].full_cost;
                    bi = *head;
                }
            }
            bi
        }
        None => {
            c_exchange_sort_by(&mut order1, |i| cands[i].full_cost);
            order1[0]
        }
    };

    // -- post_mds1_nic_pruning (:7885) + post_mds2_nic_pruning (:7961) --
    // BOTH run PER CANDIDATE CLASS in C (`for cidx`, :7903/:7969), each
    // dev-threshold relative to that class's OWN best full_cost
    // (cand_buff[cidx][0]). Running them over the sorted UNION with the
    // global best (as the single block below did) prunes the regular
    // (DC/dir) candidates out before MDS3 whenever a palette candidate's
    // lower full cost sets `best` — the MDS1/MDS3 sibling of the MDS0
    // dev-prune fix (ba58a3ec2). Without this DC never reaches MDS3, so
    // palette wins by default even though C's DC MDS3 (residual coded)
    // beats it. The post_mds1 inter-class (mds2_class_th) block IS inert on
    // the I-slice (forced ~0, :7897) — but the post_mds2 inter-class
    // (mds3_class_th) block is NOT (:7978-7979 re-floors it to
    // MAX(25, scaled*mult) for I_SLICE); that one is applied per lane below
    // (the #71 palette under-pick root: it zeroes the regular class when its
    // best cost deviates too far from the palette global best). Only the
    // palette (multi-class) path takes the per-lane branch; the single-class
    // path is byte-identical to before (best == global best => inert).
    let mds2_cand_th = div_round(cfg.mds2_cand_base_th * qw, qwd);
    let mds3_cand_th = div_round(cfg.mds3_cand_base_th * qw, qwd);
    // Inter-class MDS3 threshold (post_mds2_nic_pruning, :7975-7979). This
    // funnel is always the allintra KEY (I_SLICE), so the I-slice re-floor
    // MAX(25, scaled*i_mds3_class_th_mult) always applies. u64::MAX == the
    // `(uint64_t)~0` disabled sentinel (never set on palette-active presets).
    let mds3_class_th = if cfg.mds3_class_th == u64::MAX {
        u64::MAX
    } else {
        25u64.max(div_round(cfg.mds3_class_th * qw, qwd) * cfg.i_mds3_class_th_mult)
    };
    // C `best_md_stage_cost` at post_mds2: MDS2 is bypassed on this funnel
    // (no MD_STAGE_2 full loop), so it stays the MDS1 GLOBAL best
    // (product_coding_loop.c:9580-9585) — the overall cheapest MDS1 full cost.
    let global_best = cands[mds1_best_idx].full_cost;
    // Class id for the rank-staging compare: 0 regular, 3 palette, 4 IBC.
    let class_of = |c: &Cand| -> u8 {
        if c.ibc.is_some() {
            4
        } else if c.palette.is_some() {
            3
        } else {
            0
        }
    };
    let n3;
    if let Some((k0s, k3s, _)) = seg {
        let mds1_best_class = class_of(&cands[mds1_best_idx]);
        // post_mds1 (n2) then post_mds2 (n3) for one class lane, each
        // against that lane's own best. Returns the post_mds2 survivor
        // count. `cands`/`cfg`/thresholds captured by ref; no `order1`
        // capture (lanes are copied index lists).
        let prune_lane = |lane: &[usize]| -> usize {
            if lane.is_empty() {
                return 0;
            }
            let best = cands[lane[0]].full_cost;
            // post_mds1 -> n2
            let mut n2 = lane.len().min(nic2 as usize);
            if best > 0 && 1 < n2 {
                // C rank staging (:7934-7939): +3 when this lane is NOT
                // the MDS1-best class, else +2 when the MDS0 and MDS1
                // winners coincide (only if the base factor is nonzero).
                let lane_class = class_of(&cands[lane[0]]);
                let mut rank_factor = cfg.mds2_rank_factor;
                if rank_factor != 0 {
                    if lane_class != mds1_best_class {
                        rank_factor += 3;
                    } else if mds0_best_idx == mds1_best_idx {
                        rank_factor += 2;
                    }
                }
                let mut count = 1usize;
                let mut prev_dev = (cands[lane[count]].full_cost - best) * 100 / best;
                let mut dev = prev_dev;
                while (cfg.mds2_rel_dev_th == 0 || dev <= prev_dev + cfg.mds2_rel_dev_th)
                    && dev
                        < mds2_cand_th
                            / (if rank_factor != 0 {
                                rank_factor * count as u64
                            } else {
                                1
                            })
                {
                    count += 1;
                    if count >= n2 {
                        break;
                    }
                    prev_dev = dev;
                    dev = (cands[lane[count]].full_cost - best) * 100 / best;
                }
                n2 = count;
            }
            // post_mds2 -> n3. C: md_stage_3_count = min(md_stage_2_count,
            // nic3_base) (product_coding_loop.c:9589), then post_mds2 prunes.
            let mut n3l = n2.min(nic3 as usize);
            if n3l == 0 {
                return 0; // C guard :7986 md_stage_3_count[cidx] > 0
            }
            // INTER-CLASS prune (:7993-8008): zero a class whose best full
            // cost deviates >= mds3_class_th% from the GLOBAL best (`continue`
            // skips its intra prune), else band-reduce the count. `best` is
            // this lane's best; on the single-class path best == global_best
            // so this whole block is skipped (byte-inert). The zeroing arm is
            // the #71 fix: the regular lane (best 455607) vs the palette
            // global best (295193) gives dev 54 >= 50 at q5/p6, dropping DC
            // from MDS3 so palette (the C winner) is no longer beaten.
            if mds3_class_th != u64::MAX && best != 0 && global_best != 0 && best != global_best {
                if mds3_class_th == 0 {
                    return 0; // C :7994-7996 md_stage_3_count=0; continue
                }
                let dev = (best - global_best) * 100 / global_best;
                if dev != 0 {
                    if dev >= mds3_class_th {
                        return 0; // C :8000-8002 md_stage_3_count=0; continue
                    }
                    if cfg.mds3_band_cnt >= 3 && n3l > 1 {
                        // C :8004-8007 band reduce (DIVIDE_AND_ROUND).
                        let band_idx = dev * (cfg.mds3_band_cnt as u64 - 1) / mds3_class_th;
                        n3l = div_round(n3l as u64, band_idx + 1) as usize;
                    }
                }
            }
            // INTRA-CLASS prune (mds3_cand_th, :8011-8019): C floors cand_count
            // at 1, so a band-reduced 0 is lifted back to 1 here (only the
            // inter-class `continue` above yields a true 0).
            if best > 0 {
                let mut count = 1usize;
                while count < n3l {
                    let dev = (cands[lane[count]].full_cost - best) * 100 / best;
                    if dev >= mds3_cand_th {
                        break;
                    }
                    count += 1;
                }
                n3l = count;
            }
            n3l
        };
        // The class segments are contiguous in `order1` (per-class sorted
        // above) — C's cand_buff_indices[cidx] arrays.
        let lane0: Vec<usize> = order1[..k0s].to_vec();
        let lane3: Vec<usize> = order1[k0s..k0s + k3s].to_vec();
        let lane4: Vec<usize> = order1[k0s + k3s..].to_vec();
        let k0 = prune_lane(&lane0);
        let k3 = prune_lane(&lane3);
        let k4 = prune_lane(&lane4);
        // MDS3 evaluates the class-CONCATENATED survivors in class order —
        // C `construct_best_sorted_arrays_md_stage_3` (:1454) does NOT
        // re-sort the union; the winner scan's strict-`<` therefore breaks
        // cross-class full-cost ties toward the earlier class (C0 intra
        // beats palette/IBC on an exact tie), and the ind-uv uv_list /
        // MDS3 evaluation order follow the same concatenation.
        let mut u: Vec<usize> = lane0[..k0].to_vec();
        u.extend_from_slice(&lane3[..k3]);
        u.extend_from_slice(&lane4[..k4]);
        n3 = u.len();
        order1 = u;
    } else {
        // Single-class fast path — byte-identical to the prior union prune.
        let mut n2 = (n1 as u32).min(nic2) as usize;
        {
            let best = cands[order1[0]].full_cost;
            let mut count = 1usize;
            if best > 0 && count < n2 {
                // C rank staging (product_coding_loop.c:8158-8166): only
                // when the config factor is nonzero — same class (the
                // inter-class +3 arm is dead: single intra class == the
                // mds1 best class), +2 when MDS0 and MDS1 winners coincide.
                let mut rank_factor = cfg.mds2_rank_factor;
                if rank_factor != 0 && mds0_best_idx == mds1_best_idx {
                    rank_factor += 2;
                }
                let mut prev_dev = (cands[order1[count]].full_cost - best) * 100 / best;
                let mut dev = prev_dev;
                while (cfg.mds2_rel_dev_th == 0 || dev <= prev_dev + cfg.mds2_rel_dev_th)
                    && dev
                        < mds2_cand_th
                            / (if rank_factor != 0 {
                                rank_factor * count as u64
                            } else {
                                1
                            })
                {
                    count += 1;
                    if count >= n2 {
                        break;
                    }
                    prev_dev = dev;
                    dev = (cands[order1[count]].full_cost - best) * 100 / best;
                }
                n2 = count;
            }
        }
        let mut n3v = (n2 as u32).min(nic3) as usize;
        {
            let best = cands[order1[0]].full_cost;
            let mut count = 1usize;
            if best > 0 {
                while count < n3v {
                    let dev = (cands[order1[count]].full_cost - best) * 100 / best;
                    if dev >= mds3_cand_th {
                        break;
                    }
                    count += 1;
                }
                n3v = count;
            }
        }
        n3 = n3v;
    }

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
                    let (u_out, v_out) = chroma_eval10(fx, b, uvm, uvd);
                    (
                        u_out.bits as u64 + v_out.bits as u64,
                        u_out.dist + v_out.dist,
                    )
                }
                None => {
                    let (u_out, v_out) = chroma_eval(fx, uvm, uvd);
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
            chroma_eval(fx, cand.uv, cand.uv_delta)
        } else {
            (TxUnitOut::absent(), TxUnitOut::absent())
        };
        // bd10 chroma full loop — the decision terms for this candidate.
        let mut uv_out10 = match (&bd10_rd, has_uv) {
            (Some(b), true) => Some(match (cand.ibc, ibc_uv_tt) {
                // IBC: the DV copy at 10 bits, with the inter tx-type rule.
                (Some((dv, _)), Some(tt)) => chroma_eval10_ibc(fx, b, dv, tt),
                _ => chroma_eval10(fx, b, cand.uv, cand.uv_delta),
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
                    let (u2, v2) = chroma_eval(fx, arb_uv, arb_uvd);
                    u_out = u2;
                    v_out = v2;
                    // bd10: the 10-bit chroma decision terms follow the re-key
                    // (C re-runs the ind-uv-best chroma at hbd_md in
                    // check_best_indepedant_cfl :3957-3995). Only fires at M0
                    // (FILTER candidate, no :7063 pre-rewrite); the mds3 configs
                    // pre-rewrote so this branch is a no-op there.
                    if let Some(b) = bd10_rd.as_ref() {
                        uv_out10 = Some(chroma_eval10(fx, b, arb_uv, arb_uvd));
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

/// Commit an evaluated winner — C `md_update_all_neighbour_arrays` (+ the
/// MD recon plane writes `copy_recon_md` feeds): luma recon into
/// `y_recon`, chroma into the funnel's decision planes, mode/skip/uv
/// rows, chosen-tx txfm dims, per-txb + chroma coefficient contexts.
/// Every array write spans exactly the block, so re-committing a parent
/// block after its children were committed overwrites them completely
/// (the C winner-overwrite in `test_split_partition`).
// `clippy::manual_checked_ops` post-dates the 1.89 MSRV floor's clippy, so the
// allow has to tolerate being unknown there (`cargo +1.89 clippy` otherwise
// reports `unknown lint` at this line).
#[allow(unknown_lints, clippy::manual_checked_ops)] // the `> 0` guard scopes a whole block, not a single
// division; `checked_div` cannot express it without restructuring hot RD control flow
pub(crate) fn commit_leaf(
    fx: &mut FunnelCtx<'_>,
    y_recon: &mut [u8],
    y_stride: usize,
    ev: &LeafEval,
    // IBC chunk 8: the C PartitionType stamped onto the mi map with this
    // block (C `svt_aom_update_mi_map(pcs, ctx, pc_tree->partition, ...)`,
    // product_coding_loop.c:670 — the currently-evaluated shape during the
    // NSQ walk, the winning shape at the final re-stamp :10696). Dead when
    // the frame has no IBC state (the map is None).
    partition: u8,
) {
    let (abs_x, abs_y) = (ev.abs_x, ev.abs_y);
    let (w, h) = (ev.w, ev.h);
    // IBC chunk 8: stamp the MD mi map (C svt_aom_update_mi_map) — the
    // INTRA_FRAME MVP scans read these entries. Stamped at every mid-walk
    // commit and NEVER restored by node snapshots, mirroring C (losing
    // shapes' stamps linger until overwritten).
    if let Some(mvp) = fx.ibc_mvp.as_deref_mut() {
        let stride = fx.ibc.map(|i| i.mi_cols as usize).unwrap_or(0);
        if stride > 0 {
            let entry = crate::intrabc_mvp::MvpMiEntry {
                bsize: c_bsize_index(w, h) as u8,
                mode: ev.win.mode,
                use_intrabc: ev.win.ibc.is_some(),
                ref_frame: [0, -1], // {INTRA_FRAME, NONE_FRAME}
                mv: [
                    ev.win.ibc.map(|(dv, _)| dv).unwrap_or_default(),
                    svtav1_types::motion::Mv::default(),
                ],
                partition,
            };
            let (mi_x, mi_y) = (abs_x / 4, abs_y / 4);
            for my in mi_y..(mi_y + h / 4).min(mvp.len() / stride) {
                for cell in mvp
                    [my * stride + mi_x..(my * stride + mi_x + w / 4).min((my + 1) * stride)]
                    .iter_mut()
                {
                    *cell = entry;
                }
            }
        }
    }
    let (ccx, ccy, cw, chh) = (ev.ccx, ev.ccy, ev.cw, ev.chh);
    let cand = &ev.win;
    // Task #95 (both-partial p6 mode flip): a boundary block whose recon
    // STRADDLES past the aligned width writes columns `abs_x..abs_x+w` into a
    // row of stride `y_stride` (= the aligned width). When `abs_x + w >
    // y_stride`, the off-aligned columns spill past the row boundary and — the
    // recon buffer being SB-extent-sized but aligned-strided — WRAP into the
    // NEXT row's low columns, silently corrupting an already-committed
    // neighbour SB's recon that a later SB then reads as its intra-prediction
    // reference (e.g. an aligned-72 frame's SB(0,1) VERT 32x64 at x64..96 wraps
    // cols 72..96 into the next row's cols 0..24, flat-filling SB(0,0)'s
    // row-63 V_PRED reference → V mispredicts → DC wins → byte divergence).
    // C's recon buffer has an SB-extent stride so the straddle lands in place;
    // the off-aligned columns are never READ by any in-frame block (nothing
    // predicts, deblocks, or outputs past the aligned extent), so clipping the
    // write to the row boundary matches C's readable recon exactly and is
    // byte-neutral where nothing straddles (`abs_x + w <= y_stride`).
    let wr = w.min(y_stride.saturating_sub(abs_x));
    for r in 0..h {
        let dst = (abs_y + r) * y_stride + abs_x;
        y_recon[dst..dst + wr].copy_from_slice(&cand.y_recon[r * w..r * w + wr]);
    }
    // bd10 mode funnel (task #94): write the winner's 10-bit recon into the
    // bd10 canvas for the next block's neighbour prediction (same straddle clip
    // as the u8 recon above). `None` on the u8 path — byte-neutral for bd8.
    if let Some(canvas10) = fx.y_recon10.as_deref_mut() {
        for r in 0..h {
            let dst = (abs_y + r) * y_stride + abs_x;
            canvas10[dst..dst + wr].copy_from_slice(&ev.win_recon10[r * w..r * w + wr]);
        }
    }
    if ev.has_uv {
        // Same straddle clip on chroma (c_stride = the aligned chroma width).
        let cwr = cw.min(fx.c_stride.saturating_sub(ccx));
        for r in 0..chh {
            let dst = (ccy + r) * fx.c_stride + ccx;
            fx.u_recon[dst..dst + cwr].copy_from_slice(&cand.u_recon[r * cw..r * cw + cwr]);
            fx.v_recon[dst..dst + cwr].copy_from_slice(&cand.v_recon[r * cw..r * cw + cwr]);
        }
        // bd10 FULL-RD chroma canvases — the chroma twin of the luma write
        // above, closing the same sequential coupling for chroma prediction.
        if !ev.win_u_recon10.is_empty() {
            let c_stride = fx.c_stride;
            for (canvas, src) in [
                (fx.u_recon10.as_deref_mut(), &ev.win_u_recon10),
                (fx.v_recon10.as_deref_mut(), &ev.win_v_recon10),
            ] {
                let canvas = canvas.expect("bd10 full-RD requires both chroma canvases");
                for r in 0..chh {
                    let dst = (ccy + r) * c_stride + ccx;
                    canvas[dst..dst + cwr].copy_from_slice(&src[r * cw..r * cw + cwr]);
                }
            }
        }
        // SVTAV1_CEDGE: the committed winner's recon EDGES, mirroring the C
        // `--wrap svt_aom_update_mi_map` `CEDGE` dump (blk_ptr->neigh_top/
        // left_recon_16bit = the block's bottom row / right column). Joining
        // the two bisects an MD recon drift to its first divergent block, and
        // separates a LUMA root (lyb/lyr, which also feeds CfL's AC) from a
        // CHROMA one (cu/cv, whose average IS the next block's DC base).
        #[cfg(feature = "std")]
        if !ev.win_u_recon10.is_empty() && {
            static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
            dbg_on(&ON, "SVTAV1_CEDGE")
        } {
            let lyb: u64 = ev.win_recon10[(h - 1) * w..h * w]
                .iter()
                .map(|&s| u64::from(s))
                .sum();
            let lyr: u64 = (0..h)
                .map(|r| u64::from(ev.win_recon10[r * w + w - 1]))
                .sum();
            let col = |v: &[u16]| {
                (0..chh)
                    .map(|r| v[r * cw + cw - 1].to_string())
                    .collect::<alloc::vec::Vec<_>>()
                    .join(",")
            };
            // Raw luma edges for one pinned block (SVTAV1_CEDGE_XY="x,y") —
            // which SAMPLES differ localises a divergence to one TX unit.
            static XY: std::sync::OnceLock<Option<(usize, usize)>> = std::sync::OnceLock::new();
            let raw = (dbg_xy(&XY, "SVTAV1_CEDGE_XY") == Some((abs_x, abs_y))).then(|| {
                let j = |it: alloc::vec::Vec<u16>| {
                    it.iter()
                        .map(|v| v.to_string())
                        .collect::<alloc::vec::Vec<_>>()
                        .join(",")
                };
                alloc::format!(
                    " lyB={} lyR={}",
                    j(ev.win_recon10[(h - 1) * w..h * w].to_vec()),
                    j((0..h).map(|r| ev.win_recon10[r * w + w - 1]).collect())
                )
            });
            eprintln!(
                "CEDGE org=({abs_x},{abs_y}) {w}x{h} lyb={lyb} lyr={lyr} uvr={cw}x{chh} cu={} cv={}{}",
                col(&ev.win_u_recon10),
                col(&ev.win_v_recon10),
                raw.unwrap_or_default()
            );
        }
    }
    let skip = !cand.block_has_coeff;
    fx.ectx
        .record_block(abs_x, abs_y, w, h, cand.mode, cand.uv, skip);
    // IBC chunk 9 (Root 6 twin, MD side): stamp the inter-neighbour dims
    // — the funnel's tx_size_ctx reads them for the C is_inter override.
    fx.ectx
        .record_inter_dims(abs_x, abs_y, w, h, cand.ibc.is_some());
    // MD-time palette neighbour state (C mbmi->palette_mode_info, stamped for
    // EVERY committed winner in coding order — mirrors the pack walk's
    // record_palette + the record_block above). Read back by the NEXT
    // block's evaluate_leaf via palette_cache (colour cache / centroid snap)
    // and palette_neighbor_ctx (mode-flag ctx). None for a non-palette
    // winner => neighbour state stays empty, so non-screen content (no
    // palette winner) is byte-identical.
    fx.ectx.record_palette(
        abs_x,
        abs_y,
        w,
        h,
        cand.palette
            .as_ref()
            .map(|(colors, _idx)| colors.as_slice()),
    );
    // MD partition-context bytes (mode_decision_update_neighbor_arrays,
    // product_coding_loop.c:179-192: partition_context_lookup[bsize]
    // written over the block span — per-DIMENSION levels for rect NSQ
    // children). Consumed by the depth walk's partition rates
    // (update_part_neighs); inert for the fixed-tree paths (nothing
    // reads the decision ectx's partition bytes there).
    fx.ectx.update_partition_ctx_leaf(abs_x, abs_y, w, h);
    // set_txfm_ctxs with the CHOSEN tx dims (mode_decision_update:246-256)
    // — the skip && is_inter arm stores the BLOCK dims instead (IntraBC
    // skip winners; entropy_coding.c:4620-4624).
    let (txw, txh) = txb_dims_at_depth(w, h, cand.tx_depth);
    if cand.ibc.is_some() && skip {
        fx.ectx.record_txfm_dims(abs_x, abs_y, w, h, w, h);
    } else {
        fx.ectx.record_txfm_dims(abs_x, abs_y, w, h, txw, txh);
    }
    // Per-txb luma cul bytes; chroma culs over the chroma span. The
    // winner's txb arrays are stored in the SEARCH walk order — the
    // inter z-order (txb_org_inter) for IntraBC winners, raster for
    // intra — so the index -> position mapping must match, or a depth-2
    // IBC winner's culs land on the wrong cells and every later block's
    // coeff contexts (pack vs decode) desync.
    let cols = w / txw;
    for (txb, &cul) in cand.txb_cul.iter().enumerate() {
        let (tx_x, tx_y) = if cand.ibc.is_some() {
            txb_org_inter(w, h, cand.tx_depth, txb)
        } else {
            ((txb % cols) * txw, (txb / cols) * txh)
        };
        fx.ectx
            .record_coeff(abs_x + tx_x, abs_y + tx_y, txw, txh, cul);
    }
    if ev.has_uv {
        fx.ectx.record_coeff_uv(0, ccx, ccy, cw, chh, cand.u_cul);
        fx.ectx.record_coeff_uv(1, ccx, ccy, cw, chh, cand.v_cul);
    }
}

/// C `get_end_tx_depth` (product_coding_loop.c:4171) clamped by
/// `intra_class_max_depth_sq` / `_nsq` (get_start_end_tx_depth :6973;
/// shape == PART_N <=> w == h — HVA/HVB shapes with square children are
/// geometry-disabled at every funnel preset).
fn end_tx_depth(w: usize, h: usize, cfg: &FunnelCfg) -> u8 {
    let base: u8 = match (w, h) {
        // 2-depth blocks (the bsize list at :4173-4176).
        (64, 64) | (32, 32) | (16, 16) => 2,
        (64, 32) | (32, 64) | (32, 16) | (16, 32) | (16, 8) | (8, 16) => 2,
        (64, 16) | (16, 64) | (32, 8) | (8, 32) | (16, 4) | (4, 16) => 2,
        (8, 8) => 1,
        _ => 0, // 8x4, 4x8, 4x4
    };
    let cap = if w == h {
        cfg.txs_max_sq
    } else {
        cfg.txs_max_nsq
    };
    base.min(cap)
}

/// The INTER-class twin of [`end_tx_depth`]: same bsize base, capped by
/// `txs_ctrls.inter_class_max_depth_sq/nsq`. NOT what C applies to
/// IntraBC (C's clamp is mode-keyed -> intra caps; see the pinned KNOWN
/// GAP note at the depth-loop call site) — kept as the port's IBC cap so
/// every emitted stream stays within the proven depth<=1 pack chain.
fn end_tx_depth_inter(w: usize, h: usize, cfg: &FunnelCfg) -> u8 {
    let base: u8 = match (w, h) {
        (64, 64) | (32, 32) | (16, 16) => 2,
        (64, 32) | (32, 64) | (32, 16) | (16, 32) | (16, 8) | (8, 16) => 2,
        (64, 16) | (16, 64) | (32, 8) | (8, 32) | (16, 4) | (4, 16) => 2,
        (8, 8) => 1,
        _ => 0,
    };
    let cap = if w == h {
        cfg.txs_inter_max_sq
    } else {
        cfg.txs_inter_max_nsq
    };
    base.min(cap)
}

/// Per-txb origin at a depth for an INTER-classified (IntraBC) block —
/// C `tx_org[bsize][is_inter=1][depth][txb]` (transforms.c:48). Depths
/// 0/1 equal the intra raster; at depth 2 the inter rows are the
/// RECURSIVE var-tx z-order — depth-1 parents in raster, the 2x2
/// sub-txbs raster within each parent (verified against the C table:
/// exactly 6 (bsize, depth-2) cells differ from the intra raster —
/// 16X8/16X16/32X16/32X32/64X32/64X64; vertical rects coincide).
/// Currently unreachable at the IBC presets (inter depth caps <= 1) but
/// kept exact for when deeper inter caps arrive.
pub(crate) fn txb_org_inter(w: usize, h: usize, depth: u8, txb: usize) -> (usize, usize) {
    let (txw, txh) = txb_dims_at_depth(w, h, depth);
    if depth < 2 {
        let cols = w / txw;
        return ((txb % cols) * txw, (txb / cols) * txh);
    }
    // Parent (depth-1) geometry.
    let (pw, ph) = txb_dims_at_depth(w, h, 1);
    let sub_per_parent = (pw / txw) * (ph / txh);
    let parent = txb / sub_per_parent;
    let within = txb % sub_per_parent;
    let pcols = w / pw;
    let (px, py) = ((parent % pcols) * pw, (parent / pcols) * ph);
    let scols = pw / txw;
    (px + (within % scols) * txw, py + (within / scols) * txh)
}

/// C `bsize_to_tx_size_cat`: category of the block's max tx size chain —
/// `TXSIZE_SQR_UP` of the max rect TX (== the larger block dim as a
/// square), minus TX_8X8, capped at MAX_TX_CATS-1. 4x8/8x4 -> TX_8X8 ->
/// cat 0; 4x16/16x4 -> TX_16X16 -> cat 1.
fn tx_size_cat(w: usize, h: usize) -> usize {
    match w.max(h) {
        4 | 8 => 0,
        16 => 1,
        32 => 2,
        _ => 3, // 64 (TX_64X64 -> cat 3)
    }
}

/// C `block_signals_txsize` (rd_cost.c:1508): `bsize > BLOCK_4X4`. Every block
/// EXCEPT the 4x4 codes a tx_size symbol; for the 4x4 `svt_aom_tx_size_bits`
/// (rd_cost.c:1761) returns 0. The RD of a 4x4 leaf must therefore carry NO
/// tx_size rate — the port previously added `tx_size[cat 0][ctx][0]` (~365 rate
/// units) unconditionally, inflating every 4x4's cost and wrongly keeping an
/// 8x8 where C splits it to four 4x4 (first real-content M2/M3 partition flip).
fn block_signals_txsize(w: usize, h: usize) -> bool {
    !(w == 4 && h == 4)
}

/// C `tx_depth_to_tx_size[depth][bsize]` (common_utils.c:95) — the TX
/// dims at a given depth — plus the txb count/raster geometry
/// (`tx_blocks_per_depth` / the intra `tx_org` rows, transforms.c:48;
/// pinned against the instrumented dump in the tests below). Positions
/// are plain raster: x fastest, `w/txw` columns.
pub(crate) fn txb_dims_at_depth(w: usize, h: usize, depth: u8) -> (usize, usize) {
    let (mut tw, mut th) = (w.min(64), h.min(64));
    for _ in 0..depth {
        (tw, th) = sub_tx_dims(tw, th);
    }
    (tw, th)
}

/// C `sub_tx_size_map` chain expressed on dims: square TXs halve both
/// dims (min 4); 2:1 rects halve the long dim; 4:1 rects halve the long
/// dim (64x16 -> 32x16 -> 16x16 per the table).
fn sub_tx_dims(tw: usize, th: usize) -> (usize, usize) {
    if tw == th {
        ((tw / 2).max(4), (th / 2).max(4))
    } else if tw > th {
        (tw / 2, th)
    } else {
        (tw, th / 2)
    }
}

/// C `non_normative_txs` (product_coding_loop.c:9641): re-transform the
/// shared MDS3 residual workspace (`cand_bf->residual` = the LAST MDS3
/// candidate's whole-block depth-0 residual — every MDS3 candidate
/// full-loops through ONE pixel workspace at staging mode 1; pointer-
/// instrumented) with the two half-height TXs (H split) and the two
/// half-width TXs (V split), DCT_DCT + `svt_aom_quantize_inv_quantize_
/// light` (plain quantize_b, y tables, full_loop.c:1253), and return
/// the min eob per split direction. `None` when the winner kept no
/// coefficients (C leaves the ~0 sentinels, so the psq gate can't
/// fire).
pub(crate) fn min_nz_hv(
    ev: &LeafEval,
    qindex: u8,
    qm_level_y: u8,
    bit_depth: u8,
) -> Option<(u16, u16)> {
    if !ev.block_has_coeff() {
        return None;
    }
    let (w, h) = (ev.w, ev.h);
    debug_assert!(w == h && w >= 8, "psq gate runs on SQ blocks only");
    // bd10 (task #94, root #2): C's `non_normative_txs` transforms + quantizes
    // this residual at `EB_TEN_BIT` — Q10 tables + `svt_aom_highbd_quantize_b`
    // (full_loop.c:1288). Deciding the H/V nz counts on the bd8 residual + Q8
    // quant flips the `skip_by_sq_txs` gate. bd8 keeps `build_quant_table` +
    // `psq_resid` + `quantize_b`, so it is byte-unchanged by construction.
    let bd10 = bit_depth > 8 && !ev.psq_resid10().is_empty();
    let mut qt = if bd10 {
        crate::quant::build_quant_table_bd(qindex, bit_depth)
    } else {
        crate::quant::build_quant_table(qindex)
    };
    // C's light quantize applies the PLANE_Y QM here too (full_loop.c:1282).
    qt.qm_level = qm_level_y;
    let resid = if bd10 {
        ev.psq_resid10()
    } else {
        ev.psq_resid()
    };
    debug_assert_eq!(resid.len(), w * h);

    let half_eob = |ox: usize, oy: usize, tw: usize, th: usize| -> u16 {
        let n = tw * th;
        let c_tx = cc::tx_size_from_dims(tw, th);
        let mut residual = vec![0i32; n];
        for r in 0..th {
            let rrow = (oy + r) * w + ox;
            residual[r * tw..(r + 1) * tw].copy_from_slice(&resid[rrow..rrow + tw]);
        }
        let mut coeffs = vec![0i32; n];
        let ok = svtav1_dsp::txfm_dispatch::fwd_txfm2d_dispatch(
            &residual,
            &mut coeffs,
            tw,
            rs_tx_size(tw, th),
            TX_TYPE_FROM_C[cc::DCT_DCT],
        );
        debug_assert!(ok, "psq fwd txfm {tw}x{th}");
        // 64-dim fold (the 64x32/32x64 halves of a 64x64 block).
        let (pw, ph) = (tw.min(32), th.min(32));
        let packed = if tw > 32 || th > 32 {
            let mut v = vec![0i32; pw * ph];
            for r in 0..ph {
                v[r * pw..(r + 1) * pw].copy_from_slice(&coeffs[r * tw..r * tw + pw]);
            }
            v
        } else {
            coeffs
        };
        let scan = svtav1_entropy::scan_tables::scan(
            c_tx,
            svtav1_entropy::scan_tables::TX_TYPE_TO_SCAN_INDEX[cc::DCT_DCT] as usize,
        );
        let mut qcoeff = vec![0i32; pw * ph];
        let mut dqcoeff = vec![0i32; pw * ph];
        match if qt.qm_level < 15 {
            crate::qm::qm_slices(usize::from(qt.qm_level), false, c_tx)
        } else {
            None
        } {
            Some((wt, iwt)) if bd10 => crate::qm::quantize_b_hbd_qm(
                &packed,
                scan,
                &qt,
                TX_SCALE_TAB[c_tx],
                wt,
                iwt,
                &mut qcoeff,
                &mut dqcoeff,
            ),
            Some((wt, iwt)) => crate::qm::quantize_b_qm(
                &packed,
                scan,
                &qt,
                TX_SCALE_TAB[c_tx],
                wt,
                iwt,
                &mut qcoeff,
                &mut dqcoeff,
            ),
            None if bd10 => crate::quant::quantize_b_hbd(
                &packed,
                scan,
                &qt,
                TX_SCALE_TAB[c_tx],
                &mut qcoeff,
                &mut dqcoeff,
            ),
            None => crate::quant::quantize_b(
                &packed,
                scan,
                &qt,
                TX_SCALE_TAB[c_tx],
                &mut qcoeff,
                &mut dqcoeff,
            ),
        }
    };

    let mut nz_h = u16::MAX;
    for part in 0..2usize {
        nz_h = nz_h.min(half_eob(0, part * (h / 2), w, h / 2));
    }
    let mut nz_v = u16::MAX;
    for part in 0..2usize {
        nz_v = nz_v.min(half_eob(part * (w / 2), 0, w / 2, h));
    }
    Some((nz_h, nz_v))
}

/// Chroma tx type: C `svt_aom_get_intra_uv_tx_type`
/// (mode_decision.c:2991) = `g_intra_mode_to_tx_type[uv_mode]` clamped to
/// DCT_DCT when the chroma tx size's intra ext set doesn't carry the
/// type (32x32 chroma is DCT-only; the WIN dumps' ttuv fields pin the
/// mapping). The uv tx type affects the SCAN + coeff coding only when
/// eob > 0.
pub(crate) fn uv_tx_type(uv: u8, cw: usize, chh: usize) -> usize {
    /// C `g_intra_mode_to_tx_type[INTRA_MODES]` (DCT=0, ADST_DCT=1,
    /// DCT_ADST=2, ADST_ADST=3).
    const MODE_TO_TX: [usize; 13] = [0, 1, 2, 0, 3, 1, 2, 2, 1, 3, 1, 2, 3];
    // UV_CFL_PRED (13): C forces transform_type_uv = DCT_DCT
    // (product_coding_loop.c:3789); the decoder derives DCT_DCT for CfL too.
    if uv as usize == UV_CFL_PRED_IDX {
        return cc::DCT_DCT;
    }
    let t = MODE_TO_TX[uv as usize];
    // DCT-only tx sizes (>= 32 in either dim).
    if cw >= 32 || chh >= 32 {
        cc::DCT_DCT
    } else {
        t
    }
}

/// Per-txb luma prediction at depth > 0: reads the frame recon for
/// out-of-block neighbors and this depth's partial recon inside the block.
/// Mirrors C `av1_intra_luma_prediction` (product_coding_loop.c:4072):
/// `svt_av1_predict_intra_block` at (row_off, col_off) over the
/// tx-search neighbor arrays (block interior = this depth's recon so
/// far, exterior = frame recon).
#[allow(clippy::too_many_arguments)]
fn predict_unit_overlay(
    y_recon: &[u8],
    y_stride: usize,
    blk_x: usize,
    blk_y: usize,
    dep_recon: &[u8],
    blk_w: usize,
    blk_h: usize,
    tx_x: usize,
    tx_y: usize,
    txw: usize,
    txh: usize,
    mode: u8,
    delta: i8,
    fi: u8,
    geom: &UnitGeom,
    edge_filter: bool,
    filt_type: i32,
    dst: &mut [u8],
) {
    if matches!(mode, 3..=8) || (matches!(mode, 1 | 2) && delta != 0) {
        let p_angle = crate::intra_edge::MODE_TO_ANGLE_MAP[mode as usize] + delta as i32 * 3;
        debug_assert!(fi == FI_NONE);
        let g = crate::intra_edge::DrGeom {
            px: blk_x + tx_x,
            py: blk_y + tx_y,
            txw,
            txh,
            mi_row: geom.mi_row,
            mi_col: geom.mi_col,
            bw_px: geom.bw_px,
            bh_px: geom.bh_px,
            row_off: tx_y / 4,
            col_off: tx_x / 4,
            ss: 0,
            frame_w: geom.frame_w,
            frame_h: geom.frame_h,
            sb_mi_size: geom.sb_mi_size,
            tile: geom.tile,
        };
        crate::intra_edge::dr_predict(
            |x, y| {
                if x >= blk_x && x < blk_x + blk_w && y >= blk_y && y < blk_y + blk_h {
                    dep_recon[(y - blk_y) * blk_w + (x - blk_x)]
                } else {
                    y_recon[y * y_stride + x]
                }
            },
            &g,
            p_angle,
            edge_filter,
            filt_type,
            svtav1_types::partition::PartitionType::None,
            dst,
        );
        return;
    }
    // Build a small canvas: (txh + 1) left col + (txw + 1) top row around
    // the txb, sourcing in-block pixels from dep_recon and out-of-block
    // pixels from the frame recon, then run the standard edge extraction
    // on it. Canvas layout: (txh+1) rows x (txw+1) cols, txb at (1, 1).
    let cw_dim = txw + 1;
    let ch_dim = txh + 1;
    let abs_tx_x = blk_x + tx_x;
    let abs_tx_y = blk_y + tx_y;
    let mut canvas = vec![0u8; cw_dim * ch_dim];
    let sample = |x: isize, y: isize| -> u8 {
        // (x, y) absolute plane coords.
        if x < 0 || y < 0 {
            return 128; // never read: extract handles borders
        }
        let (x, y) = (x as usize, y as usize);
        let in_blk_x = x >= blk_x && x < blk_x + blk_w;
        let in_blk_y = y >= blk_y && y < blk_y + blk_h;
        if in_blk_x && in_blk_y {
            dep_recon[(y - blk_y) * blk_w + (x - blk_x)]
        } else {
            let row_len = y_stride;
            let idx = y * y_stride + x.min(row_len - 1);
            if idx < y_recon.len() {
                y_recon[idx]
            } else {
                y_recon[y_recon.len() - row_len + x.min(row_len - 1)]
            }
        }
    };
    // C `n_top_px` / `n_left_px` (`build_intra_predictors` via
    // `svt_av1_predict_intra_block`'s `xr`/`yd`, enc_intra_prediction.c:489-492
    // + :540-543): reference samples past the ALIGNED frame extent are not
    // read — the last real sample is replicated. Spec 7.11.2 writes the same
    // rule as a coordinate clamp (`Min(maxY, ...)`), which is what the two
    // `.min()`s below are: clamping the canvas source coordinate reproduces
    // C's replication exactly, because the last real sample IS the one at the
    // extent. Identical to the unclamped read whenever the txb is inside the
    // extent, i.e. always on a 64-aligned frame.
    let max_x = (geom.frame_w >> geom.ss).saturating_sub(1) as isize;
    let max_y = (geom.frame_h >> geom.ss).saturating_sub(1) as isize;
    // top row (incl. corner) and left col of the canvas
    for cx in 0..cw_dim {
        canvas[cx] = sample(
            (abs_tx_x as isize + cx as isize - 1).min(max_x),
            abs_tx_y as isize - 1,
        );
    }
    for cy in 1..ch_dim {
        canvas[cy * cw_dim] = sample(
            abs_tx_x as isize - 1,
            (abs_tx_y as isize + cy as isize - 1).min(max_y),
        );
    }
    // Predict at canvas coords (1, 1): availability mirrors the absolute
    // position (frame edges) — and, task #96, the TILE edges, which C
    // gates on identically (`mi_row > tile->mi_row_start`). Both origins
    // are 0 for a single-tile encode. C then selects the DC / edge-fill
    // variants on `n_*_px > 0`, which additionally excludes a txb whose own
    // origin is already past the extent (reachable inside a straddling leaf).
    let has_above = abs_tx_y > geom.tile.top_px(geom.ss) && (abs_tx_x as isize) <= max_x;
    let has_left = abs_tx_x > geom.tile.left_px(geom.ss) && (abs_tx_y as isize) <= max_y;
    let above: Vec<u8> = if has_above {
        canvas[1..cw_dim].to_vec()
    } else {
        vec![if has_left { canvas[cw_dim] } else { 127 }; txw]
    };
    let left: Vec<u8> = if has_left {
        (1..ch_dim).map(|cy| canvas[cy * cw_dim]).collect()
    } else {
        vec![if has_above { canvas[1] } else { 129 }; txh]
    };
    let top_left = if has_above && has_left {
        canvas[0]
    } else if has_above {
        canvas[1]
    } else if has_left {
        canvas[cw_dim]
    } else {
        128
    };
    if fi != FI_NONE {
        let mut above_c = vec![0u8; txw + 1];
        above_c[0] = top_left;
        above_c[1..].copy_from_slice(&above);
        svtav1_dsp::intra_pred::predict_filter_intra(dst, txw, &above_c, &left, txw, txh, fi);
        return;
    }
    match mode {
        0 => svtav1_dsp::intra_pred::predict_dc(
            dst, txw, &above, &left, txw, txh, has_above, has_left,
        ),
        1 => svtav1_dsp::intra_pred::predict_v(dst, txw, &above, txw, txh),
        2 => svtav1_dsp::intra_pred::predict_h(dst, txw, &left, txw, txh),
        9 => svtav1_dsp::intra_pred::predict_smooth(dst, txw, &above, &left, txw, txh),
        10 => svtav1_dsp::intra_pred::predict_smooth_v(dst, txw, &above, &left, txh, txh, txw),
        11 => svtav1_dsp::intra_pred::predict_smooth_h(dst, txw, &above, &left, txw, txh),
        12 => svtav1_dsp::intra_pred::predict_paeth(dst, txw, &above, &left, top_left, txw, txh),
        m => unreachable!("funnel mode {m}"),
    }
}

/// bd10 twin of [`predict_unit_overlay`]: predict one deeper-depth txb from
/// the TRUE 10-bit canvas (frame recon outside the block, this depth's 10-bit
/// recon inside).
///
/// Same geometry, same availability, same canvas splice as the u8 form — only
/// the pixel type and the no-neighbour flat fills change, which follow C's
/// `build_intra_predictors_high` (enc_intra_prediction.c:261-374):
/// `{129, 127, 128}` become `{base+1, base-1, base}` with `base = 128 <<
/// (bd - 8)`. That is the same substitution `dr_predict_hbd` already makes.
#[allow(clippy::too_many_arguments)]
fn predict_unit_overlay_hbd(
    y_recon10: &[u16],
    y_stride: usize,
    blk_x: usize,
    blk_y: usize,
    dep_recon10: &[u16],
    blk_w: usize,
    blk_h: usize,
    tx_x: usize,
    tx_y: usize,
    txw: usize,
    txh: usize,
    mode: u8,
    delta: i8,
    fi: u8,
    geom: &UnitGeom,
    edge_filter: bool,
    filt_type: i32,
    dst: &mut [u16],
    bd: u8,
) {
    use svtav1_dsp::hbd as hp;
    let base: u16 = 128u16 << (bd - 8);
    if matches!(mode, 3..=8) || (matches!(mode, 1 | 2) && delta != 0) {
        let p_angle = crate::intra_edge::MODE_TO_ANGLE_MAP[mode as usize] + delta as i32 * 3;
        debug_assert!(fi == FI_NONE);
        let g = crate::intra_edge::DrGeom {
            px: blk_x + tx_x,
            py: blk_y + tx_y,
            txw,
            txh,
            mi_row: geom.mi_row,
            mi_col: geom.mi_col,
            bw_px: geom.bw_px,
            bh_px: geom.bh_px,
            row_off: tx_y / 4,
            col_off: tx_x / 4,
            ss: 0,
            frame_w: geom.frame_w,
            frame_h: geom.frame_h,
            sb_mi_size: geom.sb_mi_size,
            tile: geom.tile,
        };
        crate::intra_edge::dr_predict_hbd(
            |x, y| {
                if x >= blk_x && x < blk_x + blk_w && y >= blk_y && y < blk_y + blk_h {
                    dep_recon10[(y - blk_y) * blk_w + (x - blk_x)]
                } else {
                    y_recon10[y * y_stride + x]
                }
            },
            &g,
            p_angle,
            edge_filter,
            filt_type,
            svtav1_types::partition::PartitionType::None,
            dst,
            bd,
        );
        return;
    }
    let cw_dim = txw + 1;
    let ch_dim = txh + 1;
    let abs_tx_x = blk_x + tx_x;
    let abs_tx_y = blk_y + tx_y;
    let mut canvas = vec![0u16; cw_dim * ch_dim];
    let sample = |x: isize, y: isize| -> u16 {
        if x < 0 || y < 0 {
            return base; // never read: the extraction below handles borders
        }
        let (x, y) = (x as usize, y as usize);
        let in_blk_x = x >= blk_x && x < blk_x + blk_w;
        let in_blk_y = y >= blk_y && y < blk_y + blk_h;
        if in_blk_x && in_blk_y {
            dep_recon10[(y - blk_y) * blk_w + (x - blk_x)]
        } else {
            let row_len = y_stride;
            let idx = y * y_stride + x.min(row_len - 1);
            if idx < y_recon10.len() {
                y_recon10[idx]
            } else {
                y_recon10[y_recon10.len() - row_len + x.min(row_len - 1)]
            }
        }
    };
    // C n_top_px/n_left_px as a coordinate clamp — see the u8 twin.
    let max_x = (geom.frame_w >> geom.ss).saturating_sub(1) as isize;
    let max_y = (geom.frame_h >> geom.ss).saturating_sub(1) as isize;
    for cx in 0..cw_dim {
        canvas[cx] = sample(
            (abs_tx_x as isize + cx as isize - 1).min(max_x),
            abs_tx_y as isize - 1,
        );
    }
    for cy in 1..ch_dim {
        canvas[cy * cw_dim] = sample(
            abs_tx_x as isize - 1,
            (abs_tx_y as isize + cy as isize - 1).min(max_y),
        );
    }
    let has_above = abs_tx_y > geom.tile.top_px(geom.ss) && (abs_tx_x as isize) <= max_x;
    let has_left = abs_tx_x > geom.tile.left_px(geom.ss) && (abs_tx_y as isize) <= max_y;
    let above: Vec<u16> = if has_above {
        canvas[1..cw_dim].to_vec()
    } else {
        vec![if has_left { canvas[cw_dim] } else { base - 1 }; txw]
    };
    let left: Vec<u16> = if has_left {
        (1..ch_dim).map(|cy| canvas[cy * cw_dim]).collect()
    } else {
        vec![if has_above { canvas[1] } else { base + 1 }; txh]
    };
    let top_left = if has_above && has_left {
        canvas[0]
    } else if has_above {
        canvas[1]
    } else if has_left {
        canvas[cw_dim]
    } else {
        base
    };
    if fi != FI_NONE {
        let mut above_c = vec![0u16; txw + 1];
        above_c[0] = top_left;
        above_c[1..].copy_from_slice(&above);
        hp::predict_filter_intra_hbd(dst, txw, &above_c, &left, txw, txh, fi, bd);
        return;
    }
    match mode {
        0 => hp::predict_dc_hbd(dst, txw, &above, &left, txw, txh, has_above, has_left, bd),
        1 => hp::predict_v_hbd(dst, txw, &above, txw, txh),
        2 => hp::predict_h_hbd(dst, txw, &left, txw, txh),
        9 => hp::predict_smooth_hbd(dst, txw, &above, &left, txw, txh),
        10 => hp::predict_smooth_v_hbd(dst, txw, &above, &left, txw, txh),
        11 => hp::predict_smooth_h_hbd(dst, txw, &above, &left, txw, txh),
        12 => hp::predict_paeth_hbd(dst, txw, &above, &left, top_left, txw, txh),
        m => unreachable!("funnel bd10 overlay mode {m}"),
    }
}

/// The 10-bit inputs one [`txt_search`] txb needs to run at true depth.
struct Bd10Txb<'a> {
    /// Block-local 10-bit source and this txb's origin inside it.
    src10: &'a [u16],
    src10_stride: usize,
    src10_off: usize,
    /// This txb's 10-bit prediction (txw*txh at stride txw).
    pred10: &'a [u16],
    qt: &'a QuantTable,
    lambda: u64,
    bd: u8,
}

/// txb skip / dc sign contexts from TX-local (block-span) overlay arrays.
/// `spans` are the block's above/left coeff-byte slices (4x4 units);
/// txb at (tx_x, tx_y) within the block, `tx` square dims.
fn txb_ctx_from_spans(
    above_span: &[u8],
    left_span: &[u8],
    tx_x: usize,
    tx_y: usize,
    txw: usize,
    txh: usize,
    block_eq_tx: bool,
) -> (usize, usize) {
    // A txb of a leaf that STRADDLES the aligned frame edge (partial-SB path,
    // task #95) can sit entirely past the frame extent — its 4x4 origin then
    // exceeds the block's coeff-context span, which `above_coeff_span` /
    // `left_coeff_span` already clip to the in-frame extent. Clamp the START so
    // the slice is empty rather than panicking (start > end). An empty span is
    // exactly what `get_txb_ctx` treats as "no coded neighbour" (== a 0xFF /
    // INVALID entry -> zero contribution), which is the context of an off-frame
    // neighbour — so this is byte-neutral for every in-frame txb (start <= len,
    // clamp is a no-op) and gives the off-frame txb the unavailable-neighbour
    // context. C reads its SB-extent-padded neighbour arrays here; the off-frame
    // cells were never coded, so C's contribution is likewise zero.
    let a0 = (tx_x / 4).min(above_span.len());
    let l0 = (tx_y / 4).min(left_span.len());
    let a = &above_span[a0..(a0 + txw / 4).min(above_span.len())];
    let l = &left_span[l0..(l0 + txh / 4).min(left_span.len())];
    cc::get_txb_ctx(0, a, l, block_eq_tx, false)
}

/// `SVTAV1_TXT_XY="x,y"` per-candidate TXT-search dump tag (block org +
/// txb identity), mirroring the sibling-C `SVT_TXT_OUT` instrument lines.
#[derive(Clone, Copy)]
struct TxtDbg {
    abs_x: usize,
    abs_y: usize,
    tx_x: usize,
    tx_y: usize,
    mode: u8,
    fi: u8,
}

/// TXT search for one luma txb (`tx_type_search`, product_coding_loop.c:
/// 4660): DCT-only above 16x16 intra (ext-tx set), otherwise the intra
/// tx-type groups with SATD early exit + rate-cost gate. Returns the best
/// type's unit output.
///
/// `crop` is this txb's cropped-TX distortion extent — C computes it ONCE
/// per txb at :4664-4665 (and identically in `perform_dct_dct_tx` at
/// :5752-5754), i.e. OUTSIDE the tx-type loop, so every candidate type is
/// scored over the same in-frame region. Passed in rather than derived
/// here because the caller owns the absolute txb origin.
#[allow(clippy::too_many_arguments)]
fn txt_search(
    src: &[u8],
    src_stride: usize,
    src_off: usize,
    pred: &[u8],
    w: usize,
    h: usize,
    crop: (usize, usize),
    depth: u8,
    txb_skip_ctx: usize,
    dc_sign_ctx: usize,
    intra_dir: usize,
    qt: &QuantTable,
    frame: &FunnelFrame,
    rates: &MdRates,
    do_rdoq: bool,
    lambda: u64,
    bd10: Option<&Bd10Txb<'_>>,
    dbg: Option<TxtDbg>,
    rate_mode: RateMode,
) -> (TxUnitOut, Option<TxUnitOutHbd>, usize) {
    macro_rules! txt_dbg {
        ($($t:tt)*) => {
            #[cfg(feature = "std")]
            if let Some(d) = &dbg {
                eprint!(
                    "PTXT org=({},{}) tx=({},{}) {w}x{h} d={depth} mode={} fi={} ",
                    d.abs_x, d.abs_y, d.tx_x, d.tx_y, d.mode, d.fi
                );
                eprintln!($($t)*);
            }
        };
    }
    let c_tx = cc::tx_size_from_dims(w, h);
    // IBC chunk 7: the INTER_TXT_DIR sentinel marks an IntraBC txb — the
    // whole search then runs over the INTER ext-tx machinery
    // (tx_type_search's `is_inter`, product_coding_loop.c:4597-4601).
    let is_inter = intra_dir == INTER_TXT_DIR;
    // search_dct_dct_only (product_coding_loop.c:4601): txt disabled
    // (eff-M9 txt_level 0 -> !mds_do_txt), dims > 32, a single-type ext
    // set, or ext set index 0.
    let only_dct = !frame.cfg.txt_on
        || w > 32
        || h > 32
        || cc::ext_tx_types(c_tx, is_inter, false) == 1
        || cc::ext_tx_set(c_tx, is_inter, false) == 0;
    // get_tx_type_group (product_coding_loop.c:4358): per-preset intra
    // group counts (M6 txt_level 8: ge16 4 / lt16 5; M5 txt_level 3:
    // 6 / 6 — the dump's txt_ge16/txt_lt16); depth-1 offset 3 (min 1).
    // INTER groups: at every IBC preset (M0-M4, txt_level 2/3) the C
    // inter group counts EQUAL the intra ones (both MAX=6/6,
    // set_txt_controls cases 2-3, enc_mode_config.c:3927-3955), so the
    // intra config fields are reused; presets >= M5 have allow_intrabc=0
    // so the inter arm is unreachable there.
    let mut groups: i32 = if only_dct {
        1
    } else if w >= 16 && h >= 16 {
        frame.cfg.txt_group_ge16
    } else {
        frame.cfg.txt_group_lt16
    };
    if depth == 1 && !only_dct {
        groups = (groups - frame.cfg.txt_d1_off).max(1);
    } else if depth == 2 && !only_dct {
        groups = (groups - frame.cfg.txt_d2_off).max(1);
    }

    const TX_TYPE_GROUPS: [&[usize]; 6] = [
        &[cc::DCT_DCT],
        &[10, 11], // V_DCT, H_DCT
        &[3],      // ADST_ADST
        &[1, 2],   // ADST_DCT, DCT_ADST
        &[6, 9],   // FLIPADST_FLIPADST, IDTX
        &[4, 5, 7, 8, 12, 13, 14, 15],
    ];

    let set_type = cc::ext_tx_set_type(c_tx, is_inter, false);
    // qp-scaled SATD early-exit th (satd_th_q_weight = 1; intra th 10 at
    // M6, 15 at M5 — txt_satd_intra in the dumps). INTER th: equal to the
    // intra th at every IBC preset (M0-M3: 20/20, M4: 15/15 —
    // set_txt_controls cases 2-3), so the intra field is reused (same
    // reasoning as the group counts above).
    let (qw, qwd) = qp_scale_factors(frame.cli_qp);
    let satd_th = if only_dct {
        0
    } else {
        div_round(frame.cfg.txt_satd_th * qw, qwd)
    } as i64;

    // C's level-0 closed form replaces `out.bits`, which also feeds the
    // per-tx-type `cost` compare below. That is inert only when exactly ONE
    // type is evaluated (`only_dct`), where `best` is assigned on the sole
    // iteration whatever the cost. `coeff_rate_est_lvl == 0` implies
    // `txt_on == false` (`FunnelCfg::for_preset`'s `_ =>` arm sets both), which
    // implies `only_dct`, so the demotion below never fires today — it is a
    // structural guard so a future preset table cannot make the search
    // rate-sensitive behind our back.
    let rate_mode = if only_dct { rate_mode } else { RateMode::Exact };
    debug_assert!(only_dct || rate_mode == RateMode::Exact);

    let mut best: Option<TxUnitOut> = None;
    // The bd10 twin of the SELECTED type (not of the u8-best type): when the
    // bd10 context is present the winner is chosen by the 10-bit cost, so both
    // outputs must come from the same tx_type.
    let mut best10: Option<TxUnitOutHbd> = None;
    let mut best_type = cc::DCT_DCT;
    let mut best_cost = u64::MAX;
    let mut dct_cost = u64::MAX;
    let mut best_satd = i64::MAX;

    'groups: for g in 0..groups as usize {
        for &tx_type in TX_TYPE_GROUPS[g] {
            if only_dct && tx_type != cc::DCT_DCT {
                continue;
            }
            if tx_type != cc::DCT_DCT {
                if AV1_EXT_TX_USED[set_type][tx_type] == 0 {
                    continue;
                }
                // txt_rate_cost_th (100 at M6, 250 at M5): skip types
                // whose signalling rate alone exceeds the DCT cost
                // fraction (product_coding_loop.c:4710-4716).
                //
                // The lambda here MUST be the same one that produced
                // `dct_cost`, or the gate compares two different scales. C
                // uses ONE `full_lambda` for both — `ctx->hbd_md ?
                // full_lambda_md[EB_10_BIT_MD] : full_lambda_md[EB_8_BIT_MD]`
                // (:4590) — in the gate at :4714 AND in the cost at :4944.
                // The port had the cost on the bd10 lambda but the gate on the
                // u8 one, so at bd10 the left side stayed 8-bit-scaled while
                // `dct_cost` was 10-bit-scaled: the gate under-fired and the
                // port evaluated (and sometimes picked) tx types C prunes
                // before it ever quantizes them. `bd10.map_or` mirrors
                // `lambda3`, so bd8 is byte-unchanged by construction.
                let gate_lambda = bd10.map_or(lambda, |b| b.lambda);
                let tx_type_rate = rates.txt_rate(c_tx, intra_dir, tx_type) as u64;
                if dct_cost != u64::MAX
                    && rdcost(gate_lambda, tx_type_rate, 0) * 1000
                        > dct_cost * frame.cfg.txt_rate_th
                {
                    txt_dbg!(
                        "cand txt={tx_type} SKIP rate_gate rate={tx_type_rate} dctcost={dct_cost}"
                    );
                    continue;
                }
                txt_dbg!(
                    "cand txt={tx_type} rate_gate_pass rate={tx_type_rate} dctcost={dct_cost}"
                );
            }
            let out = tx_unit(
                src,
                src_stride,
                src_off,
                pred,
                w,
                0,
                w,
                h,
                tx_type,
                0,
                txb_skip_ctx,
                dc_sign_ctx,
                intra_dir,
                qt,
                frame,
                rates,
                do_rdoq,
                true, // MDS3 spatial dist
                crop,
                true,
                rate_mode,
            );
            // bd10 FULL-RD (task #94): the same TX unit at true depth. Every
            // gate around it (group order, ext-tx set, the rate-cost th, the
            // SATD early exit, the non-signalable-eob rule) is bit-depth
            // INDEPENDENT — only the residual, the quant table, the lambda and
            // the distortion move — so the search structure is shared and only
            // the COST source switches.
            let out10 = bd10.map(|b| {
                tx_unit_hbd(
                    b.src10,
                    b.src10_stride,
                    b.src10_off,
                    b.pred10,
                    w,
                    0,
                    w,
                    h,
                    tx_type,
                    0,
                    txb_skip_ctx,
                    dc_sign_ctx,
                    b.qt,
                    frame.rdoq_level,
                    b.lambda,
                    frame.sharpness,
                    rates,
                    do_rdoq,
                    b.bd,
                    b.qt.qm_level,
                    Some(&TxRdArgs {
                        spatial_dist: true, // MDS3
                        intra_dir,
                        coeff_rate_est_lvl: frame.cfg.coeff_rate_est_lvl,
                        tx_bias: frame.tx_bias,
                        crop,
                    }),
                )
            });
            // SATD early exit between transform and quantize in C; we
            // apply it post-hoc on the transform coefficients via a
            // dedicated pass only when the th is armed.
            if satd_th > 0 {
                let satd = match bd10 {
                    Some(b) => txb_coeff_satd_hbd(
                        b.src10,
                        b.src10_stride,
                        b.src10_off,
                        b.pred10,
                        w,
                        h,
                        tx_type,
                    ),
                    None => txb_coeff_satd(src, src_stride, src_off, pred, w, h, tx_type),
                };
                txt_dbg!(
                    "cand txt={tx_type} satd={satd} best_satd={best_satd} skip={}",
                    i32::from(satd >= best_satd && (satd - best_satd) * 100 > best_satd * satd_th)
                );
                if satd < best_satd {
                    best_satd = satd;
                } else if (satd - best_satd) * 100 > best_satd * satd_th {
                    continue;
                }
            }
            // A non-DCT type with no coefficients is not signalable.
            let dec_eob = out10.as_ref().map_or(out.eob, |o| o.eob);
            txt_dbg!("cand txt={tx_type} eob={dec_eob}");
            if dec_eob == 0 && tx_type != cc::DCT_DCT {
                txt_dbg!("cand txt={tx_type} SKIP eob0");
                continue;
            }
            let cost = match (&out10, bd10) {
                (Some(o), Some(b)) => rdcost(b.lambda, o.bits as u64, o.dist),
                _ => rdcost(lambda, out.bits as u64, out.dist),
            };
            #[cfg(feature = "std")]
            if dbg.is_some() {
                let (b_, d_) = match &out10 {
                    Some(o) => (o.bits, o.dist),
                    None => (out.bits, out.dist),
                };
                txt_dbg!(
                    "cand txt={tx_type} bits={b_} dist={d_} cost={cost} best={best_cost}{}",
                    if cost < best_cost { " NEW_BEST" } else { "" }
                );
            }
            if cost < best_cost {
                best_cost = cost;
                best_type = tx_type;
                if tx_type == cc::DCT_DCT {
                    dct_cost = cost;
                }
                best = Some(out);
                best10 = out10;
            } else if tx_type == cc::DCT_DCT {
                dct_cost = cost;
            }
            if only_dct {
                break 'groups;
            }
        }
    }
    txt_dbg!("WINNER txt={best_type} cost={best_cost}");
    (best.expect("DCT_DCT always evaluated"), best10, best_type)
}

/// bd10 twin of [`txb_coeff_satd`] — the forward-transform coefficient SATD on
/// the 10-bit residual. The transform is bit-depth-independent; only the
/// residual's source/prediction type changes.
fn txb_coeff_satd_hbd(
    src: &[u16],
    src_stride: usize,
    src_off: usize,
    pred: &[u16],
    w: usize,
    h: usize,
    tx_type: usize,
) -> i64 {
    let n = w * h;
    let mut residual = vec![0i32; n];
    for r in 0..h {
        let srow = src_off + r * src_stride;
        let prow = r * w;
        for c in 0..w {
            residual[r * w + c] = i32::from(src[srow + c]) - i32::from(pred[prow + c]);
        }
    }
    let mut coeffs = vec![0i32; n];
    svtav1_dsp::txfm_dispatch::fwd_txfm2d_dispatch(
        &residual,
        &mut coeffs,
        w,
        rs_tx_size(w, h),
        TX_TYPE_FROM_C[tx_type],
    );
    let mut satd: i64 = 0;
    for &c in &coeffs {
        satd += c.unsigned_abs() as i64;
    }
    satd
}

/// SATD of the forward-transform coefficients (C computes it inline on
/// `ctx->tx_coeffs` right after svt_aom_estimate_transform).
fn txb_coeff_satd(
    src: &[u8],
    src_stride: usize,
    src_off: usize,
    pred: &[u8],
    w: usize,
    h: usize,
    tx_type: usize,
) -> i64 {
    let n = w * h;
    let mut residual = vec![0i32; n];
    for r in 0..h {
        let srow = src_off + r * src_stride;
        let prow = r * w;
        for c in 0..w {
            residual[r * w + c] = src[srow + c] as i32 - pred[prow + c] as i32;
        }
    }
    let mut coeffs = vec![0i32; n];
    svtav1_dsp::txfm_dispatch::fwd_txfm2d_dispatch(
        &residual,
        &mut coeffs,
        w,
        rs_tx_size(w, h),
        TX_TYPE_FROM_C[tx_type],
    );
    let mut satd: i64 = 0;
    for &c in &coeffs {
        satd += c.unsigned_abs() as i64;
    }
    satd
}

/// C `chroma_complexity_check_pred` (product_coding_loop.c:6095), exact:
/// subsampled SADs of the candidate's luma/chroma predictions vs their
/// sources; the CFL gate (`cfl_complexity == COMPONENT_CHROMA`) arms when
/// either chroma SAD exceeds 2x the luma SAD over the chroma-sized
/// region. (The use_var arm only raises chroma_complexity, which has no
/// funnel-visible effect at M6 — tx shortcuts are level 0.)
#[allow(clippy::too_many_arguments)]
fn chroma_detector_fires(
    y_src: &[u8],
    y_stride: usize,
    y_off: usize,
    y_pred: &[u8],
    y_pred_stride: usize,
    u_src: &[u8],
    v_src: &[u8],
    u_pred: &[u8],
    v_pred: &[u8],
    c_stride: usize,
    c_off: usize,
    cw: usize,
    chh: usize,
) -> bool {
    let shift = if chh > 8 {
        2usize
    } else if chh > 4 {
        1
    } else {
        0
    };
    let rows = chh >> shift;
    let sad =
        |a: &[u8], a_off: usize, a_stride: usize, b: &[u8], b_off: usize, b_stride: usize| -> u32 {
            let mut s = 0u32;
            for r in 0..rows {
                let ar = a_off + r * (a_stride << shift);
                let br = b_off + r * (b_stride << shift);
                for c in 0..cw {
                    s += (a[ar + c] as i32 - b[br + c] as i32).unsigned_abs();
                }
            }
            s
        };
    let y_dist = sad(y_src, y_off, y_stride, y_pred, 0, y_pred_stride) << 1;
    let cb_dist = sad(u_src, c_off, c_stride, u_pred, 0, cw);
    let cr_dist = sad(v_src, c_off, c_stride, v_pred, 0, cw);
    cb_dist > y_dist || cr_dist > y_dist
}

/// bd10 twin of [`chroma_detector_fires`]: C's `hbd_md` arm of
/// `chroma_complexity_check_pred` (product_coding_loop.c:6048-6072) runs
/// `sad_16b_kernel` over the **10-bit** source and the **10-bit candidate
/// prediction**, with the identical subsample shift and `cb > 2*y ||
/// cr > 2*y` test.
///
/// This is NOT redundant with the u8 form under the harness's `src10 =
/// src8 << 2` ingestion. The SOURCE scales exactly x4, so it cancels in the
/// ratio — but the PREDICTION does not: intra prediction rounds internally
/// (DC averaging, smooth weighting, paeth), so `pred10 != pred8 << 2` in
/// general. The three SADs therefore scale by slightly different factors and
/// the comparison flips on near-ties — and this test is a CfL GATE, so a flip
/// does not perturb a cost, it decides whether CfL is evaluated at all.
///
/// The sources stay `u8` + `shift`: the 10-bit source IS `src8 << shift` by
/// construction (the same ingestion `Bd10Rd`'s `y_src10`/`u_src10`/`v_src10`
/// use), so widening it here would allocate a frame-sized buffer per
/// candidate to no numerical effect.
#[allow(clippy::too_many_arguments)]
fn chroma_detector_fires_hbd(
    y_src: &[u8],
    y_src_stride: usize,
    y_src_off: usize,
    y_pred10: &[u16],
    y_pred10_stride: usize,
    u_src: &[u8],
    v_src: &[u8],
    u_pred10: &[u16],
    v_pred10: &[u16],
    c_stride: usize,
    c_off: usize,
    cw: usize,
    chh: usize,
    shift10: u32,
) -> bool {
    let shift = if chh > 8 {
        2usize
    } else if chh > 4 {
        1
    } else {
        0
    };
    let rows = chh >> shift;
    let sad = |a: &[u8],
               a_off: usize,
               a_stride: usize,
               b: &[u16],
               b_off: usize,
               b_stride: usize|
     -> u32 {
        let mut s = 0u32;
        for r in 0..rows {
            let ar = a_off + r * (a_stride << shift);
            let br = b_off + r * (b_stride << shift);
            for c in 0..cw {
                s += ((i32::from(a[ar + c]) << shift10) - i32::from(b[br + c])).unsigned_abs();
            }
        }
        s
    };
    let y_dist = sad(y_src, y_src_off, y_src_stride, y_pred10, 0, y_pred10_stride) << 1;
    let cb_dist = sad(u_src, c_off, c_stride, u_pred10, 0, cw);
    let cr_dist = sad(v_src, c_off, c_stride, v_pred10, 0, cw);
    cb_dist > y_dist || cr_dist > y_dist
}

/// C `chroma_complexity_check_pred` variance arm (product_coding_loop.c:6172,
/// `use_var == 1`): sets `cfl_complexity = COMPONENT_CHROMA` when either
/// chroma plane's per-pixel source variance exceeds `cplx_th`. Variance is
/// `svt_aom_varianceWxH_c` against a flat-128 reference (== variance around
/// the block mean), then `ROUND_POWER_OF_TWO(var, log2(cw*chh))`.
fn chroma_var_arm_fires(
    u_src: &[u8],
    v_src: &[u8],
    c_stride: usize,
    c_off: usize,
    cw: usize,
    chh: usize,
    cplx_th: u32,
) -> bool {
    let block_var = |src: &[u8]| -> u32 {
        let mut sum: i64 = 0;
        let mut sse: i64 = 0;
        for r in 0..chh {
            let row = c_off + r * c_stride;
            for c in 0..cw {
                let diff = src[row + c] as i64 - 128;
                sum += diff;
                sse += diff * diff;
            }
        }
        let n = (cw * chh) as i64;
        // svt_aom_varianceWxH_c: *sse - (uint32)((int64)sum*sum / (w*h)).
        let var = (sse - (sum * sum) / n) as u32;
        // block_var = ROUND_POWER_OF_TWO(var, log2(cw*chh)).
        let log2n = n.trailing_zeros();
        (var + (1 << (log2n - 1))) >> log2n
    };
    block_var(u_src) > cplx_th || block_var(v_src) > cplx_th
}

#[cfg(test)]
mod tests {
    use super::*;

    /// IBC chunk 7: the inter txb origins must reproduce the C
    /// `tx_org[bsize][is_inter=1]` rows exactly. Depths 0/1 equal the
    /// intra raster everywhere; depth 2 is the var-tx z-order on exactly
    /// 6 bsizes (values extracted from transforms.c:48 during the chunk-7
    /// landing — the 16X8/16X16 rows locked verbatim here, the others by
    /// the parent-major rule those two pin).
    #[test]
    fn inter_txb_origins_match_c_tx_org() {
        // Depth 0/1: identical to the intra raster for every dim pair.
        for &(w, h) in &[(64, 64), (32, 16), (16, 8), (8, 8), (16, 64)] {
            for depth in 0..=1u8 {
                let (txw, txh) = txb_dims_at_depth(w, h, depth);
                let cols = w / txw;
                let n = cols * (h / txh);
                for txb in 0..n {
                    assert_eq!(
                        txb_org_inter(w, h, depth, txb),
                        ((txb % cols) * txw, (txb / cols) * txh),
                        "{w}x{h} d{depth} txb{txb}"
                    );
                }
            }
        }
        // Depth 2, BLOCK_16X8 (C inter row):
        // {0,0},{4,0},{0,4},{4,4},{8,0},{12,0},{8,4},{12,4}.
        let c_16x8: [(usize, usize); 8] = [
            (0, 0),
            (4, 0),
            (0, 4),
            (4, 4),
            (8, 0),
            (12, 0),
            (8, 4),
            (12, 4),
        ];
        for (i, &xy) in c_16x8.iter().enumerate() {
            assert_eq!(txb_org_inter(16, 8, 2, i), xy, "16x8 d2 txb{i}");
        }
        // Depth 2, BLOCK_16X16 (C inter row).
        let c_16x16: [(usize, usize); 16] = [
            (0, 0),
            (4, 0),
            (0, 4),
            (4, 4),
            (8, 0),
            (12, 0),
            (8, 4),
            (12, 4),
            (0, 8),
            (4, 8),
            (0, 12),
            (4, 12),
            (8, 8),
            (12, 8),
            (8, 12),
            (12, 12),
        ];
        for (i, &xy) in c_16x16.iter().enumerate() {
            assert_eq!(txb_org_inter(16, 16, 2, i), xy, "16x16 d2 txb{i}");
        }
        // Vertical rects coincide with the raster even at depth 2
        // (verified against the C table): 8X16 d2.
        let (txw, txh) = txb_dims_at_depth(8, 16, 2);
        let cols = 8 / txw;
        for txb in 0..(cols * (16 / txh)) {
            assert_eq!(
                txb_org_inter(8, 16, 2, txb),
                ((txb % cols) * txw, (txb / cols) * txh),
                "8x16 d2 txb{txb}"
            );
        }
    }

    /// IBC chunk 7: `MdRates::txt_rate` INTER arm — the sentinel routes to
    /// `inter_ext_tx` rows with the inter set indexing; the intra arm is
    /// untouched (same inputs give the pre-chunk value).
    #[test]
    fn txt_rate_inter_sentinel_routes_to_inter_rows() {
        let fc = FrameContext::new_default();
        let cfc = cc::CoeffFc::default_for_qindex(60);
        let rates = build_md_rates(&fc, &cfc);
        // 8x8: intra set DTT4_IDTX_1DDCT (7 types), inter set ALL16.
        let tx = cc::TX_8X8;
        let intra_dct = rates.txt_rate(tx, 0, cc::DCT_DCT);
        let inter_dct = rates.txt_rate(tx, INTER_TXT_DIR, cc::DCT_DCT);
        // Both nonzero (multi-type sets), from DIFFERENT tables.
        assert!(intra_dct > 0 && inter_dct > 0);
        let set_inter = cc::ext_tx_set_type(tx, true, false);
        let eset_inter = cc::EXT_TX_SET_INDEX[1][set_inter] as usize;
        let sym = cc::AV1_EXT_TX_IND[set_inter][cc::DCT_DCT];
        assert_eq!(
            inter_dct,
            rates.inter_ext_tx[eset_inter * 4 + cc::TXSIZE_SQR_MAP[tx]][sym]
        );
        // 32x32: intra DCT-only (rate 0); inter DCT_IDTX (2 types, nonzero).
        assert_eq!(rates.txt_rate(cc::TX_32X32, 0, cc::DCT_DCT), 0);
        assert!(rates.txt_rate(cc::TX_32X32, INTER_TXT_DIR, cc::DCT_DCT) > 0);
    }

    /// SHIPPED-C QUIRK, second half: a tx type OUTSIDE the queried row's ext
    /// set costs ZERO, because C's `{intra,inter}_tx_type_fac_bits` are
    /// TX-TYPE-indexed and `svt_aom_get_syntax_rate_from_cdf(...,
    /// av1_ext_tx_inv[set])` (md_rate_estimation.c:225-243) only ever writes
    /// the set's own members — every other entry keeps its zero init.
    ///
    /// Reachable in exactly one place: the IntraBC coeff cost, whose
    /// `cost_dir` remap in `cost_coeffs_txb` (mirroring
    /// `svt_av1_cost_coeffs_txb`'s `is_inter = is_inter_mode(mode)` WITHOUT
    /// `|| use_intrabc`, rd_cost.c:392) reads the INTRA row for a tx type that
    /// came from the INTER search set. This port's tables are SYMBOL-indexed,
    /// so without the membership guard `AV1_EXT_TX_IND[set][out_of_set_type]`
    /// returns its own 0 filler and the query silently prices SYMBOL 0.
    ///
    /// Witness (measured, gb82-sc graph.png 512x512 q63 preset 2): block
    /// mi(8,80), a 32x32 IntraBC leaf, luma txb (16,0) 16x16. C prices V_DCT
    /// at 0 for a txb cost of 2808; symbol 0 of that row is IDTX and costs
    /// 2489 more, taking the port to 5297 and flipping the per-txb TXT winner
    /// to DCT_DCT/eob=0 where C codes V_DCT/eob=1.
    #[test]
    fn txt_rate_out_of_set_type_costs_zero_like_c() {
        let fc = FrameContext::new_default();
        let cfc = cc::CoeffFc::default_for_qindex(255);
        let rates = build_md_rates(&fc, &cfc);
        // The witnessed geometry: 16x16 on the INTRA row (the IntraBC
        // `cost_dir` remap sends intra_dir = DC_PRED = 0). TxType ids 10/11 =
        // V_DCT / H_DCT — both members of the INTER 16x16 set
        // (DTT9_IDTX_1DDCT) and neither a member of the INTRA one
        // (DTT4_IDTX).
        let tx = cc::TX_16X16;
        let intra_set = cc::ext_tx_set_type(tx, false, false);
        for t in [10usize, 11usize] {
            assert_eq!(
                AV1_EXT_TX_USED[intra_set][t], 0,
                "precondition: tx type {t} must be outside the intra 16x16 set"
            );
            assert_eq!(
                rates.txt_rate(tx, 0, t),
                0,
                "out-of-set tx type {t} must cost 0 on the intra row (C never \
                 populates that entry), not symbol 0's rate"
            );
        }
        // Anti-vacuity: symbol 0 of that very row IS expensive, so the guard
        // is doing real work rather than agreeing with an all-zero table.
        let eset = cc::EXT_TX_SET_INDEX[0][intra_set] as usize;
        let row = (eset * 4 + cc::TXSIZE_SQR_MAP[tx]) * 13;
        assert!(
            rates.intra_ext_tx[row][0] > 0,
            "symbol 0 of the intra 16x16 DC row must be nonzero"
        );
        // In-set types on the same row still price normally.
        assert!(rates.txt_rate(tx, 0, cc::DCT_DCT) > 0);
    }

    /// bd10 ind_uv fast metric: [`residual_sad_hbd`] is the 16-bit SAD C sorts
    /// the `search_best_independent_uv_mode` candidates by when `mds0_dist_type
    /// != VAR` (product_coding_loop.c:7658, `sad_16b_kernel`) — the default,
    /// since `mds0_dist_type` is never assigned in the C tree (0 = SAD). Pin it
    /// BIT-EXACT to the real `svt_aom_sad_16b_kernel_c` across the chroma sizes
    /// the uv search reaches, over randomized 10-bit content. Using variance
    /// here (the mainline LUMA mds0 metric) mis-orders the SET on non-flat
    /// recon and drops UV_PAETH from the survivors where C keeps it.
    #[test]
    fn residual_sad_hbd_matches_c_sad_16b_kernel() {
        // Deterministic xorshift so the test needs no rng dependency.
        let mut s: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        };
        for &(w, h) in &[
            (4usize, 4usize),
            (8, 8),
            (16, 16),
            (32, 32),
            (4, 8),
            (8, 4),
            (16, 8),
            (8, 16),
        ] {
            for _ in 0..300 {
                let n = w * h;
                let src: Vec<u16> = (0..n).map(|_| (next() % 1024) as u16).collect();
                let pred: Vec<u16> = (0..n).map(|_| (next() % 1024) as u16).collect();
                let port = residual_sad_hbd(&src, w, 0, 0, &pred, w, h);
                let c = svtav1_cref::sad_16b_kernel(&src, w, &pred, w, w, h) as u64;
                assert_eq!(port, c, "sad_16b mismatch at {w}x{h}: port={port} c={c}");
            }
        }
    }

    /// The bd10 CfL AC luma has TWO producers that must agree: the in-search
    /// [`cfl_ac_subsample_hbd`], which overlays the block's *uncommitted*
    /// winner recon onto the frame's ROUND_UV pair, and the re-encode
    /// post-pass's [`cfl_ac_from_frame_recon_hbd`], which reads the pair
    /// straight out of the committed frame recon. They are only allowed to
    /// differ before the block is committed; once the frame recon HOLDS the
    /// block (exactly the post-pass's situation, since `bd10_reencode_luma`
    /// walks the whole frame before chroma starts) they must be identical.
    /// This pins that invariant, which is what lets the post-pass reproduce
    /// the search's CfL prediction and hence lets `bd10_tree_supported` admit
    /// UV_CFL_PRED leaves at all.
    #[test]
    fn cfl_ac_producers_agree_once_block_is_committed() {
        use svtav1_dsp::intra_pred::CFL_BUF_LINE;
        let stride = 64usize;
        // Deterministic pseudo-random 10-bit frame recon.
        let mut frame = alloc::vec![0u16; stride * stride];
        let mut s: u32 = 0x1234_5678;
        for px in frame.iter_mut() {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            *px = ((s >> 13) & 0x3ff) as u16;
        }
        // (block x, y, w, h) — the >=8 fast path, a 4-wide and a 4-high
        // sub-8 chroma-ref pair, and an off-origin 8x8.
        // Legal AV1 leaf geometries only: an N-wide block is N-aligned in x
        // (and N-high in y), so the ROUND_UV pair origin `abs & !7` never
        // splits the block across the pair. The >=8 fast path, then the two
        // sub-8 chroma-ref shapes (4xN at an 8-aligned x, Nx4 at an 8-aligned
        // x with an odd 4-row offset).
        for &(bx, by, w, h) in &[(8, 8, 8, 8), (16, 24, 16, 16), (8, 8, 4, 8), (8, 12, 8, 4)] {
            let cw = w.max(8) / 2;
            let chh = h.max(8) / 2;
            // The block's own recon, as the search carries it (`best_recon10`).
            let mut blk = alloc::vec![0u16; w * h];
            for r in 0..h {
                let src = (by + r) * stride + bx;
                blk[r * w..(r + 1) * w].copy_from_slice(&frame[src..src + w]);
            }
            let mut a = alloc::vec![0i16; CFL_BUF_LINE * chh.max(1)];
            cfl_ac_subsample_hbd(&frame, stride, &blk, bx, by, w, h, &mut a);
            svtav1_dsp::intra_pred::cfl_subtract_average(&mut a, cw, chh);
            let mut b = alloc::vec![0i16; CFL_BUF_LINE * chh.max(1)];
            cfl_ac_from_frame_recon_hbd(&frame, stride, bx, by, w, h, cw, chh, &mut b);
            assert_eq!(a, b, "CfL AC producers disagree for {w}x{h} at ({bx},{by})");
            // Non-degenerate: a flat AC would make the comparison vacuous.
            assert!(
                a[..cw].iter().any(|&v| v != a[0]),
                "AC row is constant for {w}x{h} — test content is degenerate"
            );
        }
    }

    /// `cfl_idx_to_alpha` round-trips the packed `(u << 4) + v` index and the
    /// joint sign exactly as C's `cfl_idx_to_alpha` (intra_prediction.h:134);
    /// the re-encode post-pass re-derives both plane alphas from the leaf's
    /// stored `cfl_alpha_idx`/`cfl_alpha_signs`, so a mis-unpack there would
    /// silently mispredict chroma on every CfL leaf.
    #[test]
    fn cfl_idx_to_alpha_unpacks_both_planes() {
        // joint_sign 6 decodes to (signU = POS, signV = NEG) via C's
        // CFL_SIGN_U/V ((js+1)/3, (js+1)%3). The magnitude index c maps to
        // |alpha| = c + 1, so c=1 POS is +2 and c=2 NEG is -3 — cross-checked
        // against a real C `md_cfl_rd_pick_alpha` dump, where idx=2/sgn=6
        // evaluated alpha +1 on U (c=0) and -3 on V (c=2).
        assert_eq!(cfl_idx_to_alpha((1 << 4) + 2, 6, 0), 2); // u c=1, POS
        assert_eq!(cfl_idx_to_alpha((1 << 4) + 2, 6, 1), -3); // v c=2, NEG
        assert_eq!(cfl_idx_to_alpha(2, 6, 0), 1); // u c=0, POS
        assert_eq!(cfl_idx_to_alpha(2, 6, 1), -3); // v c=2, NEG
        // CFL_SIGN_ZERO on a plane forces alpha 0 regardless of magnitude.
        let js = plane_sign_to_joint_sign(0, 0, 1); // (ZERO, NEG)
        assert_eq!(cfl_idx_to_alpha((7 << 4) + 7, js, 0), 0);
    }

    /// Instrumented-capture pins: `M6FNL NICS c0` lines — mds1/2/3
    /// counts at CLI qp 20/40/55 (M6 nic level 6, nums 6/6/6, base
    /// 24/12/6 q-scaled).
    ///
    /// (These docs + a stray duplicate `#[test]` were left attached to the
    /// CfL producers test by the 977136df8 splice; relocated here, where the
    /// test they describe lives.)
    #[test]
    fn nic_counts_match_c() {
        // M6 (nic level 6): nums 6/6/6.
        assert_eq!(nic_counts(20, (6, 6, 6)), (8, 4, 2));
        assert_eq!(nic_counts(40, (6, 6, 6)), (15, 8, 4));
        assert_eq!(nic_counts(55, (6, 6, 6)), (22, 11, 5));
        // M8 (nic level 11 -> scaling level 15 -> nums 0/0/0): the min-1
        // floor (scaling num == 0) pins every stage to 1 at all tracked qps.
        assert_eq!(nic_counts(20, (0, 0, 0)), (1, 1, 1));
        assert_eq!(nic_counts(40, (0, 0, 0)), (1, 1, 1));
        assert_eq!(nic_counts(55, (0, 0, 0)), (1, 1, 1));
    }

    /// RDCOST identity from the captured g64 q55 MDS3 rows: the DC
    /// candidate's full cost decomposition
    /// (rate 547+273+176560+112+112+1280+26, dist 10963760).
    #[test]
    fn rdcost_matches_capture() {
        assert_eq!(rdcost(1527856, 178910, 10963760), 1937245493);
        // H row: rate 181608, dist 10996528 -> 1949490882.
        assert_eq!(rdcost(1527856, 181608, 10996528), 1949490882);
        // MDS0 fast cost, DC @ q55: rate 820, satd 204088 << 4.
        assert_eq!(rdcost(1527856, 820, 204088 << 4), 420419181);
    }

    /// Mode/uv/fi/tx_size rate pins from the M6FNL MDS0/FLC dumps
    /// (default contexts, coeff tables at the respective qindexes).
    #[test]
    fn md_rates_match_c_captures() {
        let fc = svtav1_entropy::context::FrameContext::new_default();
        let cfc = svtav1_entropy::coeff_c::CoeffFc::default_for_qindex(220);
        let r = build_md_rates(&fc, &cfc);
        // kf y mode at ctx (0,0): DC 547, SMOOTH 1556 (q55 64x64 flr).
        assert_eq!(r.kf_y[0][0][0], 547);
        assert_eq!(r.kf_y[0][0][9], 1556);
        // V/H flr include the angle0 symbol: 2874 / 2555.
        assert_eq!(r.kf_y[0][0][1] + r.angle[0][3], 2874);
        assert_eq!(r.kf_y[0][0][2] + r.angle[1][3], 2555);
        // uv fcr rows: 64x64 (CFL-disallowed) DC 273, V 1033, H 1009;
        // 32x32 (CFL-allowed) DC 845, SMOOTH 1362.
        assert_eq!(r.uv[0][0][0], 273);
        assert_eq!(r.uv[0][1][1] + r.angle[0][3], 1033);
        assert_eq!(r.uv[0][2][2] + r.angle[1][3], 1009);
        assert_eq!(r.uv[1][0][0], 845);
        assert_eq!(r.uv[1][9][9], 1362);
        // filter-intra at 32x32 (bsize_idx 9): flag-off 281 (DC flr
        // 828 - 547), flag-on + FILTER_DC mode = 1803 (FI flr 2350).
        assert_eq!(r.fi_flag[9][0], 281);
        assert_eq!(r.fi_flag[9][1] + r.fi_mode[0], 1803);
        // skip=0 at ctx 0: 26.
        assert_eq!(r.skip[0][0], 26);
        // tx_size bits: 64x64 ctx0 depth0/1 = 1280/1292; 32x32 ctx0
        // depth0 = 683 (q40 FLC nsk_txsz).
        assert_eq!(r.tx_size[3][0][0], 1280);
        assert_eq!(r.tx_size[3][0][1], 1292);
        assert_eq!(r.tx_size[2][0][0], 683);
    }

    /// FunnelCfg::for_preset(5) pins vs the instrumented M5DBG CFG
    /// enc_mode=5 dump (docs/captures/m0m5_config_dlf.txt): intra_level 2
    /// -> mode_end PAETH / ang 2; fi_max 0 (FILTER_DC only); nic 6 with
    /// M6's pruning ths; txt 6/6 satd 15 rate 250; chroma_level 4
    /// (ind-uv MDS3); SH edge filter.
    #[test]
    fn m5_cfg_matches_capture() {
        let c = FunnelCfg::for_preset(5);
        assert_eq!(c.mode_end, 12);
        assert_eq!(c.angular_level, 2);
        assert!(c.filter_intra && !c.prune_best_mode);
        assert_eq!(c.nic_num, (6, 6, 6));
        assert_eq!(
            (c.mds1_cand_base_th, c.mds1_rank_factor, c.mds2_cand_base_th),
            (1200, 3, 15)
        );
        assert_eq!((c.mds2_rel_dev_th, c.mds3_cand_base_th), (5, 15));
        assert_eq!((c.txt_group_lt16, c.txt_group_ge16), (6, 6));
        assert_eq!((c.txt_satd_th, c.txt_rate_th), (15, 250));
        assert!(c.real_coeff_ctx && c.txs_on && c.txt_on);
        assert!(c.ind_uv_mds3 && c.edge_filter && !c.dc_only_gate);
        assert_eq!(c.mds2_rank_factor, 1);
        // M6 keeps the original shape (regression pin for the shared tail).
        let m6 = FunnelCfg::for_preset(6);
        assert_eq!(m6.mode_end, 9);
        assert_eq!(m6.angular_level, 4);
        assert_eq!((m6.txt_group_lt16, m6.txt_group_ge16), (5, 4));
        assert_eq!((m6.txt_satd_th, m6.txt_rate_th), (10, 100));
        assert!(!m6.ind_uv_mds3 && !m6.edge_filter);
        assert_eq!(m6.mds2_rank_factor, 1);
    }

    /// FunnelCfg::for_preset(4) pins vs the instrumented M5DBG CFG
    /// enc_mode=4 dump (docs/captures/m0m5_config_dlf.txt line 14):
    /// intra_level 1 -> mode_end PAETH / angular_pred_level 1 (ALL 7
    /// deltas); SH edge filter OFF (ang 1 not in {2,3}); nic case 5 —
    /// scal 6, mds1 1200/rank 0, mds2 20/rank 0/rel-dev 0, mds3 15;
    /// txt/txs/rdoq/chroma identical to M5.
    #[test]
    fn m4_cfg_matches_capture() {
        let c = FunnelCfg::for_preset(4);
        assert_eq!(c.mode_end, 12);
        assert_eq!(c.angular_level, 1);
        assert!(c.filter_intra && !c.prune_best_mode);
        assert_eq!(c.nic_num, (6, 6, 6));
        assert_eq!(
            (c.mds1_cand_base_th, c.mds1_rank_factor, c.mds2_cand_base_th),
            (1200, 0, 20)
        );
        assert_eq!((c.mds2_rank_factor, c.mds2_rel_dev_th), (0, 0));
        assert_eq!(c.mds3_cand_base_th, 15);
        assert_eq!((c.txt_group_lt16, c.txt_group_ge16), (6, 6));
        assert_eq!((c.txt_satd_th, c.txt_rate_th), (15, 250));
        assert!(c.real_coeff_ctx && c.txs_on && c.txt_on);
        assert!(c.ind_uv_mds3 && !c.edge_filter && !c.dc_only_gate);
    }

    /// M4 candidate enumeration (angular_pred_level 1): every directional
    /// mode carries all 7 deltas in counter order -3..+3
    /// (mode_decision.c:3259-3271 — the |1|/|2| skip only arms at
    /// level >= 2), non-directionals one entry each, FILTER_DC last:
    /// 13 modes + 8 x 6 extra deltas = 61 regular + 1 filter-intra.
    #[test]
    fn m4_candidate_set_shape() {
        let cfg = FunnelCfg::for_preset(4);
        let mut n = 0usize;
        let mut first_dir_deltas: Vec<i8> = Vec::new();
        for mode in 0..=cfg.mode_end {
            let directional = matches!(mode, 1..=8);
            if matches!(mode, 3..=8) && cfg.angular_level >= 4 {
                continue;
            }
            if directional && cfg.angular_level <= 2 {
                for d in -3i8..=3 {
                    if cfg.angular_level >= 2 && matches!(d, -2 | -1 | 1 | 2) {
                        continue;
                    }
                    if mode == 1 {
                        first_dir_deltas.push(d);
                    }
                    n += 1;
                }
            } else {
                n += 1;
            }
        }
        assert_eq!(n, 61);
        assert_eq!(first_dir_deltas, alloc::vec![-3, -2, -1, 0, 1, 2, 3]);
    }

    /// The chroma tx type derivation confirmed by the WIN dumps
    /// (ttuv 0/1/2/3 for DC/V/H/SMOOTH; DCT-only at >= 32) + the full
    /// g_intra_mode_to_tx_type rows the M5 ind-uv modes reach.
    #[test]
    fn txb_geometry_matches_c_tables() {
        // Pinned against the instrumented tx_org/tx_blocks_per_depth/
        // tx_depth_to_tx_size dump (intra rows; docs/captures/nsq_m2m3
        // provenance): (w, h, depth) -> (txw, txh).
        const CASES: [(usize, usize, u8, usize, usize); 16] = [
            (64, 64, 1, 32, 32),
            (64, 64, 2, 16, 16),
            (32, 32, 2, 8, 8),
            (16, 16, 2, 4, 4),
            (64, 32, 0, 64, 32),
            (64, 32, 1, 32, 32),
            (64, 32, 2, 16, 16),
            (32, 64, 2, 16, 16),
            (64, 16, 1, 32, 16),
            (64, 16, 2, 16, 16),
            (16, 64, 2, 16, 16),
            (32, 8, 1, 16, 8),
            (32, 8, 2, 8, 8),
            (16, 8, 2, 4, 4),
            (4, 16, 1, 4, 8),
            (4, 16, 2, 4, 4),
        ];
        for &(w, h, d, tw, th) in &CASES {
            assert_eq!(txb_dims_at_depth(w, h, d), (tw, th), "{w}x{h} d{d}");
        }
    }

    #[test]
    fn m2_m3_funnel_cfg_matches_capture() {
        // M5DBG CFG enc_mode=2/3 rows (docs/captures/m0m5_config_dlf.txt
        // lines 12-13): txt satd 20, groups 6/6, rate 250; txs 2/2 with
        // d1/d2 offsets 0; M2 nic case 3 (scal 12, mds1 1200/rank 0,
        // mds2 30/rank 0/rel 0, mds3 25); M3 nic case 5 == M4.
        for p in [2u8, 3] {
            let c = FunnelCfg::for_preset(p);
            assert_eq!(c.txt_satd_th, 20, "p{p}");
            assert_eq!((c.txt_group_lt16, c.txt_group_ge16), (6, 6));
            assert_eq!(c.txt_rate_th, 250);
            assert_eq!((c.txs_max_sq, c.txs_max_nsq), (2, 2));
            assert_eq!((c.txt_d1_off, c.txt_d2_off), (0, 0));
            assert_eq!(c.mode_end, 12);
            assert_eq!(c.angular_level, 1);
            assert!(c.ind_uv_mds3);
            assert_eq!(c.mds1_rank_factor, 0);
            assert_eq!(c.mds2_rank_factor, 0);
            assert_eq!(c.mds2_rel_dev_th, 0);
        }
        let m2 = FunnelCfg::for_preset(2);
        assert_eq!(m2.nic_num, (12, 12, 12));
        assert_eq!(m2.mds2_cand_base_th, 30);
        assert_eq!(m2.mds3_cand_base_th, 25);
        let m3 = FunnelCfg::for_preset(3);
        assert_eq!(m3.nic_num, (6, 6, 6));
        assert_eq!(m3.mds2_cand_base_th, 20);
        assert_eq!(m3.mds3_cand_base_th, 15);
        // M4 (txs level 3) unchanged by the M2/M3 additions.
        let m4 = FunnelCfg::for_preset(4);
        assert_eq!((m4.txs_max_sq, m4.txs_max_nsq), (1, 0));
        assert_eq!((m4.txt_d1_off, m4.txt_d2_off), (3, 3));
        assert_eq!(m4.txt_satd_th, 15);
    }

    #[test]
    fn uv_tx_type_matches_c() {
        // SMOOTH_V -> ADST_DCT, SMOOTH_H -> DCT_ADST, PAETH -> ADST_ADST,
        // D45 -> DCT_DCT, D135 -> ADST_ADST (mode_decision.c:2991 table).
        assert_eq!(uv_tx_type(10, 16, 16), 1);
        assert_eq!(uv_tx_type(11, 16, 16), 2);
        assert_eq!(uv_tx_type(12, 16, 16), 3);
        assert_eq!(uv_tx_type(3, 16, 16), 0);
        assert_eq!(uv_tx_type(4, 16, 16), 3);
    }

    #[test]
    fn uv_tx_type_m6_subset_matches_c() {
        assert_eq!(uv_tx_type(0, 16, 16), 0);
        assert_eq!(uv_tx_type(1, 16, 16), 1);
        assert_eq!(uv_tx_type(2, 16, 16), 2);
        assert_eq!(uv_tx_type(9, 16, 16), 3);
        assert_eq!(uv_tx_type(2, 32, 32), 0); // 64x64 luma -> DCT only
    }

    /// A minimal mainline (non-fork) [`FunnelFrame`] for the TX-unit tests:
    /// every fork knob off, so `tx_unit`'s spatial arm is the plain SSE << 4
    /// that C's `svt_spatial_full_distortion_kernel_facade` produces at
    /// `tx_bias == 0 && ac_bias == 0`.
    fn test_frame(base_qindex: u8, frame_w_px: usize, frame_h_px: usize) -> FunnelFrame {
        FunnelFrame {
            sb_mi_size: 16,
            lambda: 100_000,
            cli_qp: 32,
            rdoq_level: 0,
            base_qindex,
            bit_depth: 8,
            qindex_u: base_qindex,
            qindex_v: base_qindex,
            ac_bias_eff: 0.0,
            sharpness: 0,
            sharp_tx_active: false,
            noise_norm_strength: 0,
            qm_levels: [15, 15, 15],
            mds0_ssd: false,
            tune_ssim: false,
            tune_ssim_threshold: 1.03,
            tx_bias: 0,
            dv_tables: None,
            frame_h_px,
            frame_w_px,
            cfg: FunnelCfg::for_preset(6),
        }
    }

    /// Task #95 (b)+(c) — the CROPPED-TX RD distortion, differentially pinned
    /// to the REAL exported C kernel.
    ///
    /// C computes a boundary TX block's SPATIAL distortion only over the part
    /// inside the ALIGNED frame: `cropped_tx_width`/`cropped_tx_height`
    /// (product_coding_loop.c:4664-4665, re-derived identically in
    /// `perform_dct_dct_tx` at :5752-5754) and `cropped_tx_width_uv`/`_height_uv`
    /// (full_loop.c:2228-2232), which it then passes straight into
    /// `svt_spatial_full_distortion_kernel_facade` (:4818/:4846, :5781/:5809,
    /// full_loop.c:2376/:2405) — the FULL tx stride, the CROPPED area.
    ///
    /// That facade IS exported and is what `cref::spatial_facade` drives, so
    /// this is the strongest evidence tier: the port's `tx_unit` distortion is
    /// compared against the real C kernel invoked with the C-derived cropped
    /// dims, on a straddling geometry taken from a real partial-superblock
    /// frame (aligned 96x80: an SB(1,0) 16x32 txb at (32,64) hangs 16 rows
    /// past the bottom; an SB(0,1) 32x16 txb at (64,32) hangs 8 columns past
    /// the right — cf. `edge_flags_match_c_rule_on_the_96x80_milestone`).
    ///
    /// ANTI-VACUITY is asserted in the test itself: the same C kernel over the
    /// FULL tx dims must give a DIFFERENT number, which is exactly what the
    /// port produced before the crop was wired.
    /// The crop reaches THREE consumers in `tx_unit`'s spatial arm: the plain
    /// SSE, `tx_bias::facade_bias`, and `ac_bias::psy_full_dist`. The
    /// differential above runs at `tx_bias == 0 && ac_bias_eff == 0.0` —
    /// exactly the configuration where the latter two are no-ops — so it
    /// covered one of the three. Flagged by the adversarial pass.
    ///
    /// This drives the psy consumer against the REAL exported C kernel
    /// (`svt_cref::psy_distortion` -> `get_svt_psy_full_dist`'s inner
    /// distortion, which C calls with `cropped_tx_width/height` and the FULL
    /// recon stride at product_coding_loop.c:4834/:4862 and :5803/:5831).
    ///
    /// ANTI-VACUITY: asserts the cropped and FULL-dims results differ, so a
    /// regression that passed full dims to the psy call would fail here.
    #[test]
    fn cropped_psy_distortion_matches_c_on_a_straddling_txb() {
        use crate::frame_geom::{FrameDims, cropped_tx_dims};
        let dims = FrameDims::new(96, 80);
        let stride = 128usize;
        let mut src = alloc::vec![0u8; stride * stride];
        let mut s: u32 = 0x9e37_79b9;
        for px in src.iter_mut() {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *px = (s >> 20) as u8;
        }
        // Bottom straddle: a 16x32 txb at (32,64) keeps 80-64 = 16 of 32 rows.
        let (tx_x, tx_y, w, h) = (32usize, 64usize, 16usize, 32usize);
        let (crop_w, crop_h) = cropped_tx_dims(&dims, tx_x, tx_y, w, h);
        assert_eq!((crop_w, crop_h), (16, 16), "the txb must actually straddle");

        let src_off = tx_y * stride + tx_x;
        let mut recon = alloc::vec![0u8; w * h];
        for (i, r) in recon.iter_mut().enumerate() {
            *r = src[src_off + (i / w) * stride + (i % w)].wrapping_add(23);
        }

        // C: cropped area, FULL recon stride.
        let c_cropped = svtav1_cref::psy_distortion(
            &src[src_off..],
            stride as u32,
            &recon,
            w as u32,
            crop_w as u32,
            crop_h as u32,
        );
        let c_full = svtav1_cref::psy_distortion(
            &src[src_off..],
            stride as u32,
            &recon,
            w as u32,
            w as u32,
            h as u32,
        );
        assert_ne!(
            c_cropped, c_full,
            "ANTI-VACUITY: cropped and full psy distortion must differ on a \
             straddling txb, else this test cannot detect the wrong dims"
        );

        let port = svtav1_dsp::ac_bias::psy_full_dist(
            &src, src_off, stride, &recon, 0, w, crop_w, crop_h, 1.0,
        );
        // psy_full_dist folds C's `llrint(psy * ac_bias)`; at ac_bias 1.0 that
        // is the kernel value itself.
        assert_eq!(
            port, c_cropped,
            "port psy_full_dist at the cropped dims must equal the real C kernel"
        );
    }

    #[test]
    fn cropped_tx_distortion_matches_c_spatial_facade() {
        use crate::frame_geom::{FrameDims, cropped_tx_dims, cropped_tx_dims_uv};
        // Aligned 96x80 partial-SB frame (the #95 milestone geometry).
        let dims = FrameDims::new(96, 80);
        assert_eq!((dims.aligned_w, dims.aligned_h), (96, 80));
        let frame = test_frame(120, dims.aligned_w, dims.aligned_h);
        let fc = FrameContext::new_default();
        let cfc = cc::CoeffFc::default_for_qindex(frame.base_qindex);
        let rates = build_md_rates(&fc, &cfc);
        let qt = crate::quant::build_quant_table(frame.base_qindex);

        // Source plane at the SB extent (128x128) — a partial SB's straddling
        // rows/cols live in the edge-replicated pad, exactly as the encoder
        // lays it out (`frame_geom::pad_input_plane`).
        let stride = 128usize;
        let mut src = alloc::vec![0u8; stride * stride];
        let mut s: u32 = 0x2545_f491;
        for px in src.iter_mut() {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *px = (s >> 20) as u8;
        }

        // (tx origin x, y, tx w, h, plane_type) — a bottom straddle, a right
        // straddle, and a fully-interior control.
        let cases: [(usize, usize, usize, usize, usize); 3] = [
            (32, 64, 16, 32, 0), // bottom: 80 - 64 = 16 of 32 rows in frame
            (64, 32, 32, 16, 0), // right:  96 - 64 = 32 -> full; use 40 below
            (16, 16, 16, 16, 0), // interior control (crop == full)
        ];
        let mut saw_crop = false;
        for &(tx_x, tx_y, w, h, plane_type) in &cases {
            let crop = cropped_tx_dims(&dims, tx_x, tx_y, w, h);
            let src_off = tx_y * stride + tx_x;
            // A prediction that is NOT the source, so the SSE is non-trivial
            // and the cropped and uncropped sums genuinely differ.
            let mut pred = alloc::vec![0u8; w * h];
            for (i, p) in pred.iter_mut().enumerate() {
                *p = src[src_off + (i / w) * stride + (i % w)].wrapping_add(17);
            }
            let out = tx_unit(
                &src,
                stride,
                src_off,
                &pred,
                w,
                0,
                w,
                h,
                cc::DCT_DCT,
                plane_type,
                0,
                0,
                0,
                &qt,
                &frame,
                &rates,
                false, /* do_rdoq */
                true,  /* spatial_dist */
                crop,
                true,
                RateMode::Exact,
            );
            // The REAL C facade, cropped area + full recon stride — the exact
            // call C makes at product_coding_loop.c:4839-4853.
            let c_cropped = svtav1_cref::spatial_facade(
                &src[src_off..],
                stride as u32,
                &out.recon,
                w as u32,
                crop.0 as u32,
                crop.1 as u32,
                0,     // DC_PRED
                0,     // UV_DC_PRED
                false, // is_chroma
                false, // is_interintra
                0,     // comp_type
                0,     // temporal_layer_index (still = layer 0)
                0.0,   // ac_bias off
                0,     // tx_bias off (mainline)
            );
            assert_eq!(
                out.dist,
                c_cropped << 4,
                "cropped spatial dist mismatch at tx ({tx_x},{tx_y}) {w}x{h}"
            );
            // ANTI-VACUITY: on a straddling txb the UNcropped kernel — what
            // the port computed before this wiring — must differ.
            let c_full = svtav1_cref::spatial_facade(
                &src[src_off..],
                stride as u32,
                &out.recon,
                w as u32,
                w as u32,
                h as u32,
                0,
                0,
                false,
                false,
                0,
                0,
                0.0,
                0,
            );
            if crop != (w, h) {
                saw_crop = true;
                assert_ne!(
                    c_full, c_cropped,
                    "straddling txb ({tx_x},{tx_y}) {w}x{h} must be crop-sensitive"
                );
                assert_ne!(
                    out.dist,
                    c_full << 4,
                    "tx_unit must NOT price the out-of-frame part of ({tx_x},{tx_y}) {w}x{h}"
                );
            } else {
                assert_eq!(c_full, c_cropped, "interior control must be crop-inert");
            }
        }
        assert!(saw_crop, "no straddling case exercised — test is vacuous");

        // ---- CHROMA (full_loop.c:2228-2232) ----
        // The chroma crop is taken in the CHROMA domain from the ROUND_UV
        // origin: `(aligned_w >> 1) - (ROUND_UV(luma_x) >> 1)`. Aligned 96x80
        // -> chroma 48x40; a 16x16 chroma txb at (32,32) hangs 8 rows past the
        // chroma bottom.
        let (ccx, ccy, cw, chh) = (32usize, 32usize, 16usize, 16usize);
        let uv_crop = cropped_tx_dims_uv(&dims, ccx, ccy, cw, chh);
        assert_eq!(uv_crop, (16, 8));
        let c_off = ccy * stride + ccx;
        let mut uv_pred = alloc::vec![0u8; cw * chh];
        for (i, p) in uv_pred.iter_mut().enumerate() {
            *p = src[c_off + (i / cw) * stride + (i % cw)].wrapping_sub(23);
        }
        let uv_out = tx_unit(
            &src,
            stride,
            c_off,
            &uv_pred,
            cw,
            0,
            cw,
            chh,
            cc::DCT_DCT,
            1,
            0,
            0,
            0,
            &qt,
            &frame,
            &rates,
            false,
            true,
            uv_crop,
            true,
            RateMode::Exact,
        );
        let uv_c_cropped = svtav1_cref::spatial_facade(
            &src[c_off..],
            stride as u32,
            &uv_out.recon,
            cw as u32,
            uv_crop.0 as u32,
            uv_crop.1 as u32,
            0,
            0,
            true, // is_chroma
            false,
            0,
            0,
            0.0,
            0,
        );
        assert_eq!(
            uv_out.dist,
            uv_c_cropped << 4,
            "chroma cropped dist mismatch"
        );
        let uv_c_full = svtav1_cref::spatial_facade(
            &src[c_off..],
            stride as u32,
            &uv_out.recon,
            cw as u32,
            cw as u32,
            chh as u32,
            0,
            0,
            true,
            false,
            0,
            0,
            0.0,
            0,
        );
        assert_ne!(
            uv_c_full, uv_c_cropped,
            "chroma case must be crop-sensitive"
        );
        assert_ne!(
            uv_out.dist,
            uv_c_full << 4,
            "chroma must not price out-of-frame rows"
        );
    }

    /// The cropped-TX bound itself, as hand-derived from the C expressions
    /// (product_coding_loop.c:4664-4665 luma, full_loop.c:2228-2232 chroma).
    /// Complements the differential above, which pins the CONSUMPTION.
    #[test]
    fn cropped_tx_dims_match_the_c_expressions() {
        use crate::frame_geom::{FrameDims, cropped_tx_dims, cropped_tx_dims_uv};
        let d = FrameDims::new(96, 80);
        // Interior: no crop.
        assert_eq!(cropped_tx_dims(&d, 0, 0, 64, 64), (64, 64));
        assert_eq!(cropped_tx_dims(&d, 32, 32, 16, 16), (16, 16));
        // Bottom straddle: MIN(32, 80 - 64) = 16 rows.
        assert_eq!(cropped_tx_dims(&d, 32, 64, 16, 32), (16, 16));
        // Right straddle: MIN(32, 96 - 80) = 16 cols.
        assert_eq!(cropped_tx_dims(&d, 80, 0, 32, 32), (16, 32));
        // Both.
        assert_eq!(cropped_tx_dims(&d, 80, 64, 32, 32), (16, 16));
        // Chroma: bound is (aligned >> 1) - chroma_origin, chroma dims 48x40.
        assert_eq!(cropped_tx_dims_uv(&d, 0, 0, 32, 32), (32, 32));
        assert_eq!(cropped_tx_dims_uv(&d, 32, 32, 16, 16), (16, 8));
        assert_eq!(cropped_tx_dims_uv(&d, 40, 32, 8, 8), (8, 8));
        assert_eq!(cropped_tx_dims_uv(&d, 32, 24, 16, 16), (16, 16));
        // 64-aligned frame: BOTH crops are the identity at every geometry —
        // the byte-neutrality guarantee for every full-SB gate cell.
        let full = FrameDims::new(128, 128);
        for &(x, y, w, h) in &[
            (0usize, 0usize, 64usize, 64usize),
            (64, 64, 64, 64),
            (96, 112, 32, 16),
        ] {
            assert_eq!(cropped_tx_dims(&full, x, y, w, h), (w, h));
        }
        for &(x, y, w, h) in &[(0usize, 0usize, 32usize, 32usize), (32, 48, 32, 16)] {
            assert_eq!(cropped_tx_dims_uv(&full, x, y, w, h), (w, h));
        }
    }
}
