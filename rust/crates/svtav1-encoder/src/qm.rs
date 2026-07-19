//! [SVT_HDR_MODE] AV1 quantization matrices (QM) — fork default ON
//! (`enable_qm=1`, luma levels 6..10, chroma 8..15; mainline default OFF).
//!
//! Pieces (all mirroring the C hybrid):
//! * [`aom_get_qmlevel`] — linear qindex→level map (md_config_process.c:179,
//!   used for the default tune=PSNR branch of `svt_av1_qm_init`).
//! * [`still_get_qmlevel`] — the still-image polynomial (TUNE_IQ /
//!   TUNE_MS_SSIM branch, md_config_process.c:185).
//! * [`qm_slices`] — (wt, iwt) matrix slices per (level, plane-class,
//!   tx size), reproducing the `gqmatrix`/`giqmatrix` pointer init
//!   (md_config_process.c:211-237): per level and plane class the flat
//!   3344-entry blob concatenates one matrix per SELF-adjusted tx size in
//!   TX_SIZES_ALL order; non-self-adjusted sizes alias their adjusted
//!   size's matrix; level 15 (NUM_QM_LEVELS-1) has no matrices.
//! * [`quantize_b_qm`] / [`quantize_fp_qm`] — the QM branches of the C
//!   quantize kernels (full_loop.c `svt_aom_quantize_b_c` /
//!   `quantize_fp_helper_c` with non-NULL qm/iqm), differentially tested
//!   against the exported C functions WITH the transcribed tables
//!   (tests/c_parity_qm.rs) — validating kernels and tables together.
//! * [`dqv_qm`] — the trellis/noise-norm `get_dqv` iwt adjustment
//!   (full_loop.c:741).
//!
//! QM applies only to 2D transforms (`IS_2D_TRANSFORM(tx_type)` =
//! `tx_type < IDTX(9)`); IDTX/1D transforms use identity (level 15).

use crate::qm_tables::{IWT_MATRIX_REF, WT_MATRIX_REF};

/// `AOM_QM_BITS` — unity weight is `1 << 5 = 32`.
pub const AOM_QM_BITS: i32 = 5;
/// `NUM_QM_LEVELS` (1 << QM_LEVEL_BITS); level 15 = identity (no matrices).
pub const NUM_QM_LEVELS: usize = 16;

/// `av1_get_adjusted_tx_size` (common_utils.h:100) as an index map over
/// TX_SIZES_ALL: 64X64/64X32/32X64→32X32, 64X16→32X16, 16X64→16X32.
pub const ADJUSTED_TX_SIZE: [usize; 19] = [
    0, 1, 2, 3, 3, // 4X4 8X8 16X16 32X32 64X64→32X32
    5, 6, 7, 8, 9, 10, // 4X8 8X4 8X16 16X8 16X32 32X16
    3, 3, // 32X64→32X32 64X32→32X32
    13, 14, 15, 16, // 4X16 16X4 8X32 32X8
    9, 10, // 16X64→16X32 64X16→32X16
];

/// Offset of each tx size's matrix inside the flat per-plane blob —
/// the `current` accumulator of the C init loop, walking TX_SIZES_ALL in
/// order and advancing by `tx_size_2d` only at self-adjusted sizes.
pub const QM_OFFSET: [usize; 19] = [
    0, 16, 80, 336, 336, // 4X4 8X8 16X16 32X32 (64X64 aliases 32X32)
    1360, 1392, 1424, 1552, 1680, 2192, // 4X8 8X4 8X16 16X8 16X32 32X16
    336, 336, // 32X64 64X32 alias 32X32
    2704, 2768, 2832, 3088, // 4X16 16X4 8X32 32X8
    1680, 2192, // 16X64 aliases 16X32, 64X16 aliases 32X16
];

/// `tx_size_2d` of the ADJUSTED size (matrix length at each index).
pub const QM_SIZE_2D: [usize; 19] = [
    16, 64, 256, 1024, 1024, 32, 32, 128, 128, 512, 512, 1024, 1024, 64, 64, 256, 256, 512, 512,
];

/// C `aom_get_qmlevel` (md_config_process.c:179): linear map of
/// qindex(0..255) into level(first..last).
pub fn aom_get_qmlevel(qindex: i32, first: i32, last: i32) -> i32 {
    (first + (qindex * (last + 1 - first)) / 256).clamp(first, last)
}

/// C `svt_av1_still_get_qmlevel` (md_config_process.c:185): degree-7
/// polynomial tuned for still images (TUNE_IQ / TUNE_MS_SSIM).
pub fn still_get_qmlevel(qindex: i32, min: i32, max: i32) -> i32 {
    const COEFFS: [f64; 8] = [
        1.10464272e-14,
        -9.78597634e-12,
        3.46261763e-09,
        -6.26759877e-07,
        6.10876647e-05,
        -3.04942759e-03,
        4.79930113e-02,
        9.86922373e+00,
    ];
    let mut result = 0.0f64;
    let mut q_power = 1.0f64;
    for coeff_idx in (0..8).rev() {
        result += COEFFS[coeff_idx] * q_power;
        q_power *= f64::from(qindex);
    }
    // C round(): half away from zero.
    (result.round() as i32).clamp(min, max)
}

/// (wt, iwt) slices for (qm level, chroma?, C TxSize index), or None when
/// the level is identity (15) — callers then use the non-QM kernels.
/// The C plane class is `c >= 1` — U and V share the chroma tables.
pub fn qm_slices(level: usize, is_chroma: bool, c_tx: usize) -> Option<(&'static [u8], &'static [u8])> {
    if level >= NUM_QM_LEVELS - 1 {
        return None;
    }
    let p = usize::from(is_chroma);
    let off = QM_OFFSET[c_tx];
    let n = QM_SIZE_2D[c_tx];
    Some((
        &WT_MATRIX_REF[level][p][off..off + n],
        &IWT_MATRIX_REF[level][p][off..off + n],
    ))
}

/// C `get_dqv` (full_loop.c:741): per-position dequant with the inverse
/// weight applied — used by the trellis and noise normalization.
#[inline]
pub fn dqv_qm(dequant: &[i32; 2], coeff_idx: usize, iwt: Option<&[u8]>) -> i32 {
    let iw = iwt.map_or(1 << AOM_QM_BITS, |m| i32::from(m[coeff_idx]));
    (dequant[usize::from(coeff_idx != 0)] * iw + (1 << (AOM_QM_BITS - 1))) >> AOM_QM_BITS
}

/// C `svt_aom_quantize_b_c` with non-NULL qm/iqm (full_loop.c:30).
/// `wt`/`iwt` are raster-indexed matrix slices from [`qm_slices`].
#[allow(clippy::too_many_arguments)]
pub fn quantize_b_qm(
    coeffs: &[i32],
    scan: &[u16],
    t: &crate::quant::QuantTable,
    log_scale: i32,
    wt: &[u8],
    iwt: &[u8],
    qcoeff: &mut [i32],
    dqcoeff: &mut [i32],
) -> u16 {
    let n_coeffs = scan.len();
    let zbins = [
        (t.zbin[0] + ((1 << log_scale) >> 1)) >> log_scale,
        (t.zbin[1] + ((1 << log_scale) >> 1)) >> log_scale,
    ];
    qcoeff[..coeffs.len()].fill(0);
    dqcoeff[..coeffs.len()].fill(0);

    // Pre-scan pass (weighted zbin dead zone).
    let mut non_zero_count = n_coeffs;
    for i in (0..n_coeffs).rev() {
        let rc = scan[i] as usize;
        let w = i32::from(wt[rc]);
        let coeff = coeffs[rc] * w;
        let iz = usize::from(rc != 0);
        if coeff < zbins[iz] * (1 << AOM_QM_BITS) && coeff > -zbins[iz] * (1 << AOM_QM_BITS) {
            non_zero_count -= 1;
        } else {
            break;
        }
    }

    let mut eob: i64 = -1;
    for i in 0..non_zero_count {
        let rc = scan[i] as usize;
        let coeff = coeffs[rc];
        let iz = usize::from(rc != 0);
        let coeff_sign: i32 = if coeff < 0 { -1 } else { 0 };
        let abs_coeff = (coeff ^ coeff_sign) - coeff_sign;
        let w = i64::from(wt[rc]);
        if i64::from(abs_coeff) * w >= i64::from(zbins[iz]) << AOM_QM_BITS {
            let round = (t.round[iz] + ((1 << log_scale) >> 1)) >> log_scale;
            let mut tmp = i64::from((abs_coeff + round).clamp(i16::MIN as i32, i16::MAX as i32));
            tmp *= w;
            let tmp32 = ((((tmp * t.quant[iz] as i64) >> 16) + tmp) * t.quant_shift[iz] as i64
                >> (16 - log_scale + AOM_QM_BITS)) as i32;
            qcoeff[rc] = (tmp32 ^ coeff_sign) - coeff_sign;
            let dequant =
                (t.dequant[iz] * i32::from(iwt[rc]) + (1 << (AOM_QM_BITS - 1))) >> AOM_QM_BITS;
            let abs_dq = ((tmp32 as i64 * dequant as i64) >> log_scale) as i32;
            dqcoeff[rc] = (abs_dq ^ coeff_sign) - coeff_sign;
            if tmp32 != 0 {
                eob = i as i64;
            }
        }
    }
    (eob + 1) as u16
}

/// C `quantize_fp_helper_c` QM branch (full_loop.c:257).
#[allow(clippy::too_many_arguments)]
pub fn quantize_fp_qm(
    coeffs: &[i32],
    scan: &[u16],
    t: &crate::quant::QuantTable,
    log_scale: i32,
    wt: &[u8],
    iwt: &[u8],
    qcoeff: &mut [i32],
    dqcoeff: &mut [i32],
) -> u16 {
    let n_coeffs = scan.len();
    let rounding = [
        (t.round_fp[0] + ((1 << log_scale) >> 1)) >> log_scale,
        (t.round_fp[1] + ((1 << log_scale) >> 1)) >> log_scale,
    ];
    qcoeff[..coeffs.len()].fill(0);
    dqcoeff[..coeffs.len()].fill(0);

    let mut eob: i64 = -1;
    for i in 0..n_coeffs {
        let rc = scan[i] as usize;
        let coeff = coeffs[rc];
        let iz = usize::from(rc != 0);
        let w = i64::from(wt[rc]);
        let iw = i32::from(iwt[rc]);
        let dequant = (t.dequant[iz] * iw + (1 << (AOM_QM_BITS - 1))) >> AOM_QM_BITS;
        let coeff_sign: i32 = if coeff < 0 { -1 } else { 0 };
        let abs_coeff = i64::from((coeff ^ coeff_sign) - coeff_sign);
        let mut tmp32: i32 = 0;
        if abs_coeff * w >= i64::from(t.dequant[iz]) << (AOM_QM_BITS - (1 + log_scale)) {
            let a = (abs_coeff + i64::from(rounding[iz])).clamp(i16::MIN as i64, i16::MAX as i64);
            tmp32 = ((a * w * t.quant_fp[iz] as i64) >> (16 - log_scale + AOM_QM_BITS)) as i32;
            qcoeff[rc] = (tmp32 ^ coeff_sign) - coeff_sign;
            let abs_dq = ((tmp32 as i64 * dequant as i64) >> log_scale) as i32;
            dqcoeff[rc] = (abs_dq ^ coeff_sign) - coeff_sign;
        }
        if tmp32 != 0 {
            eob = i as i64;
        }
    }
    (eob + 1) as u16
}

/// C `highbd_quantize_fp_helper_c` **QM arm** (full_loop.c:342-366) — the
/// bd>8 FP quantize with quantization matrices.
///
/// Differs from the bd8 [`quantize_fp_qm`] in exactly one place, the same
/// bd8-only INT16 clamp that separates [`crate::quant::quantize_fp_hbd`] from
/// [`crate::quant::quantize_fp`]: C's 8-bit arm does
/// `abs_coeff = clamp64(abs_coeff + rounding, INT16_MIN, INT16_MAX)`
/// (full_loop.c:271-272) while the highbd arm keeps full width in an `int64_t`
/// `tmp` (full_loop.c:355-356). At 10-bit the coefficients genuinely exceed
/// INT16, so the clamp is the whole difference.
///
/// The `if (abs_qcoeff)` eob guard also differs from the bd8 arm: highbd sets
/// `eob = i` only for a nonzero qcoeff, whereas the bd8 QM arm writes
/// `qcoeff_ptr[rc]` unconditionally inside the threshold branch and then tests
/// `tmp32`. Both end up recording the last nonzero, but highbd leaves
/// `qcoeff/dqcoeff` at the memset 0 for a zero result, which is what the
/// `else` clause here reproduces.
#[allow(clippy::too_many_arguments)]
pub fn quantize_fp_hbd_qm(
    coeffs: &[i32],
    scan: &[u16],
    t: &crate::quant::QuantTable,
    log_scale: i32,
    wt: &[u8],
    iwt: &[u8],
    qcoeff: &mut [i32],
    dqcoeff: &mut [i32],
) -> u16 {
    let shift = 16 - log_scale;
    qcoeff[..coeffs.len()].fill(0);
    dqcoeff[..coeffs.len()].fill(0);

    let mut eob: i64 = -1;
    for (i, &sc) in scan.iter().enumerate() {
        let rc = sc as usize;
        let coeff = coeffs[rc];
        let iz = usize::from(rc != 0);
        let w = i64::from(wt[rc]);
        let iw = i32::from(iwt[rc]);
        let dequant = (t.dequant[iz] * iw + (1 << (AOM_QM_BITS - 1))) >> AOM_QM_BITS;
        let coeff_sign: i32 = if coeff < 0 { -1 } else { 0 };
        let abs_coeff = i64::from((coeff ^ coeff_sign) - coeff_sign);
        if abs_coeff * w >= i64::from(t.dequant[iz]) << (AOM_QM_BITS - (1 + log_scale)) {
            // NO INT16 clamp (highbd path) — C full_loop.c:355-356.
            let tmp = abs_coeff
                + i64::from((t.round_fp[iz] + ((1 << log_scale) >> 1)) >> log_scale);
            let abs_qcoeff = ((tmp * i64::from(t.quant_fp[iz]) * w) >> (shift + AOM_QM_BITS)) as i32;
            qcoeff[rc] = (abs_qcoeff ^ coeff_sign) - coeff_sign;
            let abs_dq = ((i64::from(abs_qcoeff) * i64::from(dequant)) >> log_scale) as i32;
            dqcoeff[rc] = (abs_dq ^ coeff_sign) - coeff_sign;
            if abs_qcoeff != 0 {
                eob = i as i64;
            }
        } else {
            qcoeff[rc] = 0;
            dqcoeff[rc] = 0;
        }
    }
    (eob + 1) as u16
}

/// C `svt_aom_highbd_quantize_b_c` **QM arm** (full_loop.c:85-136) — the bd>8
/// dead-zone (b) quantize with quantization matrices, used by the bd10 RDOQ
/// level-0 path.
///
/// Unlike [`crate::quant::quantize_b_hbd`] (which may collapse C's `idx_arr`
/// to a contiguous prefix because the no-QM dead-zone test is monotone in scan
/// order) this reproduces C's **two-pass index collection verbatim**. With a
/// matrix the prescan test is `coeff * wt` vs `zbin << AOM_QM_BITS`, and `wt`
/// varies per coefficient position — so passing/failing is no longer a
/// trailing-run property and the prefix shortcut would select a different set.
/// C's quantization pass then processes exactly the collected indices with NO
/// further guard, so the collected set IS the semantics.
#[allow(clippy::too_many_arguments)]
pub fn quantize_b_hbd_qm(
    coeffs: &[i32],
    scan: &[u16],
    t: &crate::quant::QuantTable,
    log_scale: i32,
    wt: &[u8],
    iwt: &[u8],
    qcoeff: &mut [i32],
    dqcoeff: &mut [i32],
) -> u16 {
    let n_coeffs = scan.len();
    qcoeff[..coeffs.len()].fill(0);
    dqcoeff[..coeffs.len()].fill(0);

    let zbins = [
        (t.zbin[0] + ((1 << log_scale) >> 1)) >> log_scale,
        (t.zbin[1] + ((1 << log_scale) >> 1)) >> log_scale,
    ];

    // Pre-scan pass (C idx_arr[4096]; the packed txb here is at most 32x32
    // = 1024 coefficients after the 64-dim fold).
    debug_assert!(n_coeffs <= 1024, "packed txb exceeds the idx_arr bound");
    let mut idx_arr = [0u16; 1024];
    let mut idx = 0usize;
    for (i, &sc) in scan.iter().enumerate() {
        let rc = sc as usize;
        let iz = usize::from(rc != 0);
        let coeff = i64::from(coeffs[rc]) * i64::from(wt[rc]);
        let zb = i64::from(zbins[iz]) << AOM_QM_BITS;
        if coeff >= zb || coeff <= -zb {
            idx_arr[idx] = i as u16;
            idx += 1;
        }
    }

    // Quantization pass: only the collected indices, no further guard.
    let mut eob: i64 = -1;
    for &si in idx_arr.iter().take(idx) {
        let i = si as usize;
        let rc = scan[i] as usize;
        let coeff = coeffs[rc];
        let iz = usize::from(rc != 0);
        let coeff_sign: i32 = if coeff < 0 { -1 } else { 0 };
        let w = i64::from(wt[rc]);
        let iw = i32::from(iwt[rc]);
        let abs_coeff = i64::from((coeff ^ coeff_sign) - coeff_sign);
        // NO INT16 clamp (highbd path) — C full_loop.c:122-124.
        let tmp1 = abs_coeff + i64::from((t.round[iz] + ((1 << log_scale) >> 1)) >> log_scale);
        let tmpw = tmp1 * w;
        let tmp2 = ((tmpw * i64::from(t.quant[iz])) >> 16) + tmpw;
        let abs_qcoeff =
            ((tmp2 * i64::from(t.quant_shift[iz])) >> (16 - log_scale + AOM_QM_BITS)) as i32;
        qcoeff[rc] = (abs_qcoeff ^ coeff_sign) - coeff_sign;
        let dequant = (t.dequant[iz] * iw + (1 << (AOM_QM_BITS - 1))) >> AOM_QM_BITS;
        let abs_dq = ((i64::from(abs_qcoeff) * i64::from(dequant)) >> log_scale) as i32;
        dqcoeff[rc] = (abs_dq ^ coeff_sign) - coeff_sign;
        if abs_qcoeff != 0 {
            eob = i as i64;
        }
    }
    (eob + 1) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_reproduce_c_init_walk() {
        // Recompute the C loop: current += tx_size_2d[t] at self-adjusted t.
        let mut current = 0usize;
        let mut off = [0usize; 19];
        for t in 0..19 {
            if ADJUSTED_TX_SIZE[t] == t {
                off[t] = current;
                current += QM_SIZE_2D[t];
            }
        }
        assert_eq!(current, crate::qm_tables::QM_TOTAL_SIZE);
        for t in 0..19 {
            assert_eq!(QM_OFFSET[t], off[ADJUSTED_TX_SIZE[t]], "tx {t}");
        }
    }

    #[test]
    fn qmlevel_maps() {
        // Linear: fork luma envelope 6..10.
        assert_eq!(aom_get_qmlevel(0, 6, 10), 6);
        assert_eq!(aom_get_qmlevel(255, 6, 10), 10);
        // The C expression before clamping at qindex 128: 6 + 128*5/256 = 8.
        assert_eq!(aom_get_qmlevel(128, 6, 10), 8);
        // Still polynomial endpoints: value(0) = 9.869... -> 10 (in 6..15).
        assert_eq!(still_get_qmlevel(0, 0, 15), 10);
        // Monotone-ish decrease toward high qindex; clamps at min.
        assert!(still_get_qmlevel(255, 0, 15) <= still_get_qmlevel(0, 0, 15));
    }

    #[test]
    fn identity_level_has_no_matrices() {
        assert!(qm_slices(15, false, 0).is_none());
        let (w, iw) = qm_slices(0, false, 0).unwrap();
        assert_eq!(w.len(), 16);
        assert_eq!(iw.len(), 16);
        // First 4x4 luma level-0 weights start 32,24,14,11 (q_matrices.h).
        assert_eq!(&w[..4], &[32, 24, 14, 11]);
    }
}
