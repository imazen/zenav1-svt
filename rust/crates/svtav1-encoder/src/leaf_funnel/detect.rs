//! Coefficient SATD and the chroma-complexity detector that gates CfL.
//!
//! C's chroma detector (product_coding_loop.c:6095) decides whether a block's
//! chroma is complex enough to be worth evaluating CfL for; on flat-chroma
//! content it never fires, which is what keeps [`super::cfl`] dead there.
//!
//! Split out of `leaf_funnel/mod.rs` on 2026-08-25. PURE CODE MOVEMENT: every
//! item keeps its name, order and effective visibility (file-private became
//! `pub(super)`, the same scope).

use super::*;

/// bd10 twin of [`txb_coeff_satd`] — the forward-transform coefficient SATD on
/// the 10-bit residual. The transform is bit-depth-independent; only the
/// residual's source/prediction type changes.
pub(super) fn txb_coeff_satd_hbd(
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
pub(super) fn txb_coeff_satd(
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
pub(super) fn chroma_detector_fires(
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
pub(super) fn chroma_detector_fires_hbd(
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
pub(super) fn chroma_var_arm_fires(
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
