//! [SVT_HDR_MODE] tune policies (`--tune 0..5`) — the per-tune RD/config
//! deltas reachable on the still/allintra path, making fork mode a full
//! pareto encoder (VQ / PSNR / SSIM / IQ / MS_SSIM / FILM_GRAIN).
//!
//! Still-reachable effects wired from C (hybrid = v4.2-final + fork gates):
//! * **SSIM/IQ/MS_SSIM**: per-16x16 `ssim_rdmult_scaling_factors`
//!   (src_ops_process.c `aom_av1_set_mb_ssim_rdmult_scaling`, incl. the
//!   `alt_ssim_tuning` perceptual-variance variant) scaling the block
//!   lambda by the geometric mean of covered factors
//!   (`aom_av1_set_ssim_rdmult`, mode_decision.c:4117 — no I-slice gate).
//! * **IQ**: still-picture `lambda_weight` curve (enc_mode_config.c:13513),
//!   the constant chroma boost (rc_crf_cqp.c TUNE_IQ arm), the
//!   still-image QM level polynomial (qm.rs `still_get_qmlevel`).
//! * **SSIM**: the pow(x,1.4)/9 chroma boost (fork rc arm).
//! * **IQ/MS_SSIM**: the LF sharpness cap ladder (deblocking_filter.c:1160
//!   — qindex<=112 -> 7, <=160 -> 1, else 0, MIN'd onto the level).
//! * **VQ/FILM_GRAIN**: KEY-frame LF sharpness +2 (min 7).
//!
//! NOT modeled (documented): tune=SSIM/IQ `tune_ssim_level` LVL_3 is
//! `!I_SLICE`-gated in C — never reachable on stills (alt_ssim_tuning's
//! LVL_1 is, and is ported separately); TUNE_VQ's `vq_ctrls` sharpness
//! machinery is video-focused (PORT-NOTE below); TUNE_VMAF is
//! mainline-v4.2-only (the fork replaces slot 5 with FILM_GRAIN).

/// Fork tune values (definitions.h:1919, fork enum — slot 5 is
/// FILM_GRAIN in the fork; mainline v4.2's slot 5 is VMAF, not modeled).
pub const TUNE_VQ: u8 = 0;
pub const TUNE_PSNR: u8 = 1;
pub const TUNE_SSIM: u8 = 2;
pub const TUNE_IQ: u8 = 3;
pub const TUNE_MS_SSIM: u8 = 4;
pub const TUNE_FILM_GRAIN: u8 = 5;

/// Does this tune run the per-block SSIM rdmult scaling?
pub fn tune_uses_ssim_rdmult(tune: u8) -> bool {
    matches!(tune, TUNE_SSIM | TUNE_IQ | TUNE_MS_SSIM)
}

/// C `svt_aom_get_perpixel_variance` (src_ops_process.c:2138): the
/// integer block variance (aom vf semantics: `sse - ((sum*sum) >> log2N)`
/// over the 128-offset diffs) rounded to per-pixel.
fn perpixel_variance(buf: &[u8], stride: usize, n: usize) -> u32 {
    let log2n = (n * n).trailing_zeros();
    let mut sum: i64 = 0;
    let mut sse: i64 = 0;
    for r in 0..n {
        for c in 0..n {
            let d = i64::from(buf[r * stride + c]) - 128;
            sum += d;
            sse += d * d;
        }
    }
    let var = sse - ((sum * sum) >> log2n);
    (((var as u64) + (1 << (log2n - 1))) >> log2n) as u32
}

/// C `svt_aom_get_perceptual_perpixel_variance` (src_ops_process.c:2173):
/// two-pass mean/variance + the mid-gray parabola boost
/// `var + (var*weight)/sqrtf(var+1)` (f32 division, truncated).
fn perceptual_perpixel_variance(buf: &[u8], stride: usize, n: usize) -> u32 {
    let log2n = (n * n).trailing_zeros();
    let mut sum: u64 = 0;
    for r in 0..n {
        for c in 0..n {
            sum += u64::from(buf[r * stride + c]);
        }
    }
    let mean = ((sum + (1 << (log2n - 1))) >> log2n) as i32;
    let mut sse: u64 = 0;
    for r in 0..n {
        for c in 0..n {
            let d = i64::from(buf[r * stride + c]) - i64::from(mean);
            sse += (d * d) as u64;
        }
    }
    let var = ((sse + (1 << (log2n - 1))) >> log2n) as u32;
    let centered_mean = mean - 128;
    let weight_numerator = 128 * 128 - centered_mean * centered_mean;
    let weight = (weight_numerator * 256) / (128 * 128);
    // C: `var + ((var * weight) / sqrtf(var + 1.))` — int product, f32
    // divide, f32 sum, truncating store to unsigned.
    let boost = (var as i64 * i64::from(weight)) as f32 / ((var as f32) + 1.0).sqrt();
    (var as f32 + boost) as u32
}

/// Per-16x16 SSIM rdmult scaling factors for the frame luma (C
/// `aom_av1_set_mb_ssim_rdmult_scaling`). `alt` = the fork's
/// `alt_ssim_tuning` multi-scale perceptual-variance blend (4x4 + 8x8 +
/// 16x16 weighted 0.25/0.5/0.25 of `num_of_var`) which also SKIPS the
/// final geometric-mean normalization.
pub fn ssim_rdmult_factors(
    luma: &[u8],
    stride: usize,
    width: usize,
    height: usize,
    alt: bool,
) -> (alloc::vec::Vec<f64>, usize, usize) {
    let mi_cols = width.div_ceil(4);
    let mi_rows = height.div_ceil(4);
    let num_cols = mi_cols.div_ceil(4);
    let num_rows = mi_rows.div_ceil(4);
    let mut factors = alloc::vec![0.0f64; num_cols * num_rows];
    let mut log_sum = 0.0f64;
    const FACTOR_A: f64 = 67.035434;
    const FACTOR_B: f64 = -0.0021489;
    const FACTOR_C: f64 = 17.492222;

    for row in 0..num_rows {
        for col in 0..num_cols {
            let mut var = 0.0f64;
            let mut num_of_var = 0.0f64;
            if alt {
                // 4x4 sweep (weight 0.015625 each = 0.25 of the total).
                let mut mi_r = row * 4;
                while mi_r < mi_rows && mi_r < (row + 1) * 4 {
                    let mut mi_c = col * 4;
                    while mi_c < mi_cols && mi_c < (col + 1) * 4 {
                        let off = (mi_r * 4) * stride + mi_c * 4;
                        var += f64::from(perceptual_perpixel_variance(&luma[off..], stride, 4));
                        num_of_var += 0.015625;
                        mi_c += 1;
                    }
                    mi_r += 1;
                }
                // 8x8 sweep (weight 0.125 each = 0.5 of the total).
                let mut mi_r = row * 4;
                while mi_r < mi_rows && mi_r < (row + 1) * 4 {
                    let mut mi_c = col * 4;
                    while mi_c < mi_cols && mi_c < (col + 1) * 4 {
                        let off = (mi_r * 4) * stride + mi_c * 4;
                        var += f64::from(perceptual_perpixel_variance(&luma[off..], stride, 8));
                        num_of_var += 0.125;
                        mi_c += 2;
                    }
                    mi_r += 2;
                }
                // The 16x16 itself (0.25 of the total).
                let off = (row * 16) * stride + col * 16;
                var += f64::from(perceptual_perpixel_variance(&luma[off..], stride, 16));
                num_of_var += 0.25;
            } else {
                // Plain 8x8 sweep, unit weights.
                let mut mi_r = row * 4;
                while mi_r < mi_rows && mi_r < (row + 1) * 4 {
                    let mut mi_c = col * 4;
                    while mi_c < mi_cols && mi_c < (col + 1) * 4 {
                        let off = (mi_r * 4) * stride + mi_c * 4;
                        var += f64::from(perpixel_variance(&luma[off..], stride, 8));
                        num_of_var += 1.0;
                        mi_c += 2;
                    }
                    mi_r += 2;
                }
            }
            var /= num_of_var;
            let v = FACTOR_A * (1.0 - (FACTOR_B * var).exp()) + FACTOR_C;
            debug_assert!(v > 17.0 && v < 85.0);
            factors[row * num_cols + col] = v;
            log_sum += v.ln();
        }
    }
    if !alt {
        let norm = (log_sum / (num_rows * num_cols) as f64).exp();
        for f in factors.iter_mut() {
            *f /= norm;
        }
    }
    (factors, num_cols, num_rows)
}

/// C `aom_av1_set_ssim_rdmult` (mode_decision.c:4117): geometric mean of
/// the 16x16 factors a block covers. `mi_row/mi_col` = the block's mi
/// coords, `bw_mi/bh_mi` = block dims in mi units. Note C's index quirk
/// (row from mi_row/num_mi_W, col from mi_col/num_mi_H — both 4, benign).
pub fn ssim_scale_for_block(
    factors: &[f64],
    num_cols: usize,
    num_rows: usize,
    mi_row: usize,
    mi_col: usize,
    bw_mi: usize,
    bh_mi: usize,
) -> f64 {
    let num_bcols = bw_mi.div_ceil(4);
    let num_brows = bh_mi.div_ceil(4);
    let mut num_of_mi = 0.0f64;
    let mut geom = 1.0f64;
    let mut row = mi_row / 4;
    while row < num_rows && row < mi_row / 4 + num_brows {
        let mut col = mi_col / 4;
        while col < num_cols && col < mi_col / 4 + num_bcols {
            geom *= factors[row * num_cols + col];
            num_of_mi += 1.0;
            col += 1;
        }
        row += 1;
    }
    geom.powf(1.0 / num_of_mi)
}

/// C TUNE_IQ still-picture `lambda_weight` (enc_mode_config.c:13513):
/// `CLIP3(0, 72, MIN(qp*4, (63-qp)*3)) + 128`.
pub fn iq_lambda_weight(picture_qp: u32) -> u32 {
    let qp = picture_qp as i32;
    (qp * 4).min((63 - qp) * 3).clamp(0, 72) as u32 + 128
}

/// Per-tune LF sharpness (deblocking_filter.c:1157, KEY frames): VQ and
/// FILM_GRAIN get `min(7, sharpness+2)`; IQ and MS_SSIM cap by qindex
/// (<=112 -> 7, <=160 -> 1, else 0); other tunes pass through.
pub fn lf_sharpness_for_tune(sharpness: u8, tune: u8, base_qindex: u8) -> u8 {
    match tune {
        TUNE_VQ | TUNE_FILM_GRAIN => (sharpness + 2).min(7),
        TUNE_IQ | TUNE_MS_SSIM => {
            let max = if base_qindex <= 112 {
                7
            } else if base_qindex <= 160 {
                1
            } else {
                0
            };
            sharpness.min(max)
        }
        _ => sharpness,
    }
}

/// Fork rc chroma-boost tune arms (rc_crf_cqp.c:565, SVT_HDR_MODE path) —
/// the delta applied to `chroma_qindex` BEFORE the tune-independent
/// boosts (which chroma_q.rs already carries). `adj` = the pre-adjustment
/// chroma qindex (C `chroma_qindex_adjustment`).
pub fn tune_chroma_boost(tune: u8, adj: i32) -> i32 {
    match tune {
        TUNE_SSIM => {
            let t2 = (adj - 48).max(0) as f64;
            // C: rint(pow(t2, 1.4) / 9.0) — round-half-to-even.
            let v = (t2.powf(1.4) / 9.0).round_ties_even() as i32;
            v.clamp(0, 12)
        }
        TUNE_IQ => (adj / 2 - 14).clamp(0, 12),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iq_lambda_curve_endpoints() {
        assert_eq!(iq_lambda_weight(0), 128);
        assert_eq!(iq_lambda_weight(18), 200); // 18*4=72 cap
        assert_eq!(iq_lambda_weight(39), 200); // (63-39)*3=72 cap
        assert_eq!(iq_lambda_weight(63), 128);
        assert_eq!(iq_lambda_weight(30), 200);
        assert_eq!(iq_lambda_weight(10), 168);
    }

    #[test]
    fn lf_ladder() {
        assert_eq!(lf_sharpness_for_tune(1, TUNE_VQ, 100), 3);
        assert_eq!(lf_sharpness_for_tune(7, TUNE_FILM_GRAIN, 100), 7);
        assert_eq!(lf_sharpness_for_tune(7, TUNE_IQ, 100), 7);
        assert_eq!(lf_sharpness_for_tune(7, TUNE_IQ, 130), 1);
        assert_eq!(lf_sharpness_for_tune(7, TUNE_MS_SSIM, 200), 0);
        assert_eq!(lf_sharpness_for_tune(1, TUNE_PSNR, 100), 1);
    }

    #[test]
    fn chroma_boost_arms() {
        assert_eq!(tune_chroma_boost(TUNE_IQ, 60), 12); // 60/2-14=16 -> clip 12
        assert_eq!(tune_chroma_boost(TUNE_IQ, 20), 0); // 20/2-14=-4 -> 0
        assert_eq!(tune_chroma_boost(TUNE_SSIM, 40), 0); // t2=0
        assert!(tune_chroma_boost(TUNE_SSIM, 120) > 0);
        assert_eq!(tune_chroma_boost(TUNE_PSNR, 200), 0);
    }

    #[test]
    fn factors_normalized_geomean_one() {
        // Non-alt path: geometric mean of factors is 1 after normalization.
        let luma: alloc::vec::Vec<u8> =
            (0..128 * 128).map(|i| ((i * 7) % 256) as u8).collect();
        let (f, nc, nr) = ssim_rdmult_factors(&luma, 128, 128, 128, false);
        assert_eq!((nc, nr), (8, 8));
        let g: f64 = f.iter().map(|v| v.ln()).sum::<f64>() / f.len() as f64;
        assert!(g.abs() < 1e-9, "geomean(log) = {g}");
        for &v in &f {
            assert!(v > 0.0);
        }
    }
}
