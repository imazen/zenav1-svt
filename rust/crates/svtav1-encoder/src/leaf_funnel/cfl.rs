//! Chroma-from-luma: AC subsampling, the alpha ladder, and the RD alpha pick.
//!
//! C `cfl_prediction` / `md_cfl_rd_pick_alpha` (product_coding_loop.c:3750).
//! Reached only when the chroma-complexity detector arms
//! ([`super::chroma_detector_fires`]), which is why flat-chroma content never
//! executes a line of it.
//!
//! Split out of `leaf_funnel/mod.rs` on 2026-08-25. PURE CODE MOVEMENT: every
//! item keeps its name, order and effective visibility (file-private became
//! `pub(super)`, the same scope).

use super::*;

/// C `MAX_MODE_COST` (coding_unit.h:37) — the RD-cost sentinel for
/// "not set" used by md_cfl_rd_pick_alpha / cfl_prediction.
pub(super) const MAX_MODE_COST: u64 = 13754408443200 * 8;

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
pub(super) fn cfl_ac_subsample(
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
pub(super) fn cfl_ac_subsample_hbd(
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
    use crate::entropy::context::{cfl_sign_u, cfl_sign_v};
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
pub(super) fn plane_sign_to_joint_sign(plane: usize, a: usize, b: usize) -> u8 {
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
pub(super) fn md_cfl_rd_pick_alpha(
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
    // ONE prediction buffer for the whole alpha search, not one per trial.
    // `cfl_predict_lbd` writes every byte of it (`dst_stride == width == cw`
    // over all `chh` rows), so a reused buffer cannot carry a stale value from
    // the previous alpha. `callcount_realimg_2026-09-04` item B named this
    // site: 211,831 callocs on photo_cid p2, one per alpha trial, against
    // 20,754 calls to this function.
    let mut cfl_pred = vec![0u8; cw * chh];
    let plane_cost = |plane: usize, alpha_q3: i32| -> (u64, i32) {
        let (src, dc, tsc, dsc) = if plane == 0 {
            (u_src, u_dc, cb_tsc, cb_dsc)
        } else {
            (v_src, v_dc, cr_tsc, cr_dsc)
        };
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
///
/// # The tier arms below are a LAYOUT effect, and the mechanism they were built
/// to test is REFUTED — do not cite them as evidence for it
///
/// `archmage/docs/PERFORMANCE.md`'s rule is "enter `#[arcane]` once from
/// non-SIMD code and put the loop INSIDE": the alpha loop here was wrapped so
/// `plane_cost` — which calls `cfl_predict_lbd` and `tx_unit` — would inline
/// into one `#[target_feature]` region per tier and stop crossing the boundary
/// per alpha. **It does not inline.** In the built binary
/// `md_cfl_rd_pick_alpha::{closure#0}` is still its own out-of-line symbol
/// (654.1 M inclusive Ir on photo_cid p2, 2.368 G on photo_clic), so the
/// boundary is crossed exactly as often as before and `tx_unit` never enters
/// the AVX2 region. The instruction count says so too: this shape is
/// Ir-**WORSE** than the plain call (photo_clic 45,185,097,146 ->
/// 45,191,716,532, the CFL subtree 2,445,276,387 -> 2,450,070,566).
///
/// It is here because the campaign decides on WALL CLOCK, and on wall clock it
/// wins on four of five cells with non-overlapping spans (r7900x, 21 paired
/// rounds each, quiet box, every row byte-identical): photo_clic 512 p2
/// **1.010x**, photo_cid 512 p2 **1.008x**, gradient 512 p2 1.005x, gradient
/// 256 p2 1.002x, gradient 512 p6 1.001x (span straddles 1.0 — a null). That
/// is a code-layout / register-allocation effect of the extra frame, the exact
/// mirror of `benchmarks/cfl_branchfree_2026-09-05.meta`'s rejected
/// variant (fewer instructions, slower everywhere) and reported the same way.
/// **And it is NOT what the inlining model predicts** — C reaches its own
/// kernel through the RTCD FUNCTION POINTER `svt_cfl_predict_lbd`
/// (`Codec/common_dsp_rtcd.h:73`), i.e. an indirect out-of-line call per alpha
/// per plane, and still costs 99 Ir a call. Record:
/// `benchmarks/cfl_simd_kernel_2026-09-05.meta`.
pub(super) fn md_cfl_alpha_search(
    plane_cost: impl FnMut(usize, i32) -> (u64, i32),
    rates: &MdRates,
    lambda: u64,
    luma_mode: usize,
    itr_th: u8,
) -> (u8, u8, u64) {
    archmage::incant!(
        md_cfl_alpha_search_impl(plane_cost, rates, lambda, luma_mode, itr_th),
        [v3, neon, scalar]
    )
}

fn md_cfl_alpha_search_impl_scalar(
    _t: archmage::prelude::ScalarToken,
    plane_cost: impl FnMut(usize, i32) -> (u64, i32),
    rates: &MdRates,
    lambda: u64,
    luma_mode: usize,
    itr_th: u8,
) -> (u8, u8, u64) {
    md_cfl_alpha_search_core(plane_cost, rates, lambda, luma_mode, itr_th)
}

#[cfg(target_arch = "x86_64")]
#[archmage::arcane]
fn md_cfl_alpha_search_impl_v3(
    _t: archmage::prelude::Desktop64,
    plane_cost: impl FnMut(usize, i32) -> (u64, i32),
    rates: &MdRates,
    lambda: u64,
    luma_mode: usize,
    itr_th: u8,
) -> (u8, u8, u64) {
    md_cfl_alpha_search_core(plane_cost, rates, lambda, luma_mode, itr_th)
}

#[cfg(target_arch = "aarch64")]
#[archmage::arcane]
fn md_cfl_alpha_search_impl_neon(
    _t: archmage::prelude::NeonToken,
    plane_cost: impl FnMut(usize, i32) -> (u64, i32),
    rates: &MdRates,
    lambda: u64,
    luma_mode: usize,
    itr_th: u8,
) -> (u8, u8, u64) {
    md_cfl_alpha_search_core(plane_cost, rates, lambda, luma_mode, itr_th)
}

#[inline(always)]
fn md_cfl_alpha_search_core(
    mut plane_cost: impl FnMut(usize, i32) -> (u64, i32),
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
pub(super) const UV_CFL_PRED_IDX: usize = 13;
