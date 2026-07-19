//! Variance Boost — the fork-side (SVT_HDR_MODE=1) per-SB delta-q boost.
//!
//! Port of the C hybrid's `rc_aq.c` fork region (svt-av1-hdr "Chromedome"
//! semantics): double-precision 8x8 variances, octile sampling with 1:2:1
//! smoothing, four boost curves — 0 default, 1 low/medium-contrast,
//! 2 still-picture (CID22/SSIMULACRA2-tuned), **3 PQ/HDR** with dark-region
//! attenuation — and the curve-dependent qstep→qindex-delta conversion.
//!
//! STATUS: the math is fully ported and unit-pinned here; the per-SB
//! delta-q WIRING (frame `delta_q_present=1`, per-SB qindex signaling in
//! the tile syntax + its rate estimation) is the tracked long pole — see
//! docs/HDR-ON-4.2.md. Until it lands, HdrFork-mode encodes cannot yet
//! byte-match the MODE1 oracle whenever variance boost changes any SB's
//! qindex (i.e. virtually always, since the fork defaults it ON).
//!
//! The helpers `convert_qindex_to_q_fp8` / `compute_qdelta_fp` mirror the
//! EXPORTED C functions of the same names (common to both modes) and are
//! C-parity-tested against the linked reference library.

use svtav1_dsp::quant_tables::AC_QLOOKUP_8;

pub const MINQ: i32 = 0;
pub const MAXQ: i32 = 255;

const VAR_BOOST_MAX_PQ_DELTAQ_RANGE: i32 = 120;
const VAR_BOOST_MAX_PQ_QSTEP_RATIO_BOOST: f64 = 14.0;
const VAR_BOOST_MAX_DELTAQ_RANGE: i32 = 80;
const VAR_BOOST_MAX_QSTEP_RATIO_BOOST: f64 = 8.0;

const SUBBLOCKS_IN_SB: usize = 64;
const SUBBLOCKS_IN_OCTILE: usize = SUBBLOCKS_IN_SB / 8;

/// C `svt_av1_convert_qindex_to_q_fp8` (rc_aq.c:24): AC dequant at the
/// qindex, scaled to fp8 "real Q" units. 8-bit only in this port (the C
/// 10/12-bit variants shift by 4/3 on their own tables).
#[inline]
pub fn convert_qindex_to_q_fp8(qindex: i32) -> i32 {
    let q = qindex.clamp(0, 255) as usize;
    i32::from(AC_QLOOKUP_8[q]) << 6
}

/// C `svt_av1_compute_qdelta_fp` (rc_aq.c:39): linear scan mapping fp8 Q
/// values back to qindex, returning `target_index - start_index`.
pub fn compute_qdelta_fp(qstart_fp8: i32, qtarget_fp8: i32) -> i32 {
    let mut start_index = MAXQ;
    for i in MINQ..MAXQ {
        start_index = i;
        if convert_qindex_to_q_fp8(i) >= qstart_fp8 {
            break;
        }
    }
    let mut target_index = MAXQ;
    for i in MINQ..MAXQ {
        target_index = i;
        if convert_qindex_to_q_fp8(i) >= qtarget_fp8 {
            break;
        }
    }
    target_index - start_index
}

/// C fork `av1_get_deltaq_sb_variance_boost` (rc_aq.c, SVT_HDR_MODE=1).
///
/// * `base_q_idx` — frame base qindex.
/// * `mean` — SB mean luma sum (ppcs->mean units; PQ dark attenuation input).
/// * `variances_8x8` — the 64 double-precision 8x8 variances
///   (`variance[ME_TIER_ZERO_PU_8x8_0..+64]`).
/// * `variance_64x64` — `variance[ME_TIER_ZERO_PU_64x64]` (block-protect input).
/// * `strength` 1..=4, `octile` 1..=8, `curve` 0..=3.
///
/// Returns the qindex boost (subtract from base qindex), clamped to the
/// curve's max delta-q range.
#[allow(clippy::too_many_arguments)]
pub fn deltaq_sb_variance_boost(
    base_q_idx: u8,
    mean: u64,
    variances_8x8: &[f64; 64],
    variance_64x64: f64,
    strength: u8,
    octile: u8,
    curve: u8,
) -> i32 {
    debug_assert!((1..=4).contains(&strength));
    debug_assert!((1..=8).contains(&octile));

    // qsort with the C comparator: total order on doubles via </>.
    let mut ordered = *variances_8x8;
    ordered.sort_by(|a, b| a.partial_cmp(b).expect("variance NaN"));

    let mid_idx = octile as usize * SUBBLOCKS_IN_OCTILE - 1;
    let low_idx = (SUBBLOCKS_IN_OCTILE - 1).max(mid_idx - SUBBLOCKS_IN_OCTILE);
    let upp_idx = (SUBBLOCKS_IN_SB - 1).min(mid_idx + SUBBLOCKS_IN_OCTILE);

    // 1:2:1 smoothing across the neighbouring octile representatives.
    let mut variance = (ordered[low_idx] + 2.0 * ordered[mid_idx] + ordered[upp_idx]) / 4.0;

    // clip minimum variance to 1, or 0.25 on the PQ curve
    let min_var = if curve == 3 { 0.25 } else { 1.0 };
    if variance < min_var {
        variance = min_var;
    }

    const STRENGTHS: [f64; 5] = [0.0, 0.4, 0.8, 1.2, 1.8];
    const STRENGTHS_PQ: [f64; 5] = [0.0, 0.65, 1.1, 1.6, 2.5];
    let s = strength as usize;

    let mut qstep_ratio = match curve {
        1 => 0.25 * f64::from(strength) * (-variance.log2() + 8.0) + 1.0,
        2 => 0.15 * f64::from(strength) * (-variance.log2() + 10.0) + 1.0,
        3 => 1.018f64.powf(STRENGTHS_PQ[s] * (-10.0 * variance.log2() + 80.0)),
        _ => 1.018f64.powf(STRENGTHS[s] * (-10.0 * variance.log2() + 80.0)),
    };

    // PQ curve dark-bias adjustment: attenuate the boost the darker the
    // block, unless the SB is mixed-variance ("protected").
    if curve == 3 && mean <= 25000 {
        let m = mean as f64;
        let dark_attenuation_ratio = 0.25 + 0.75 * ((m * m) / (25000.0 * 25000.0));
        let should_protect_block = variance_64x64 > 256.0;
        if !should_protect_block {
            qstep_ratio = (qstep_ratio - 1.0) * dark_attenuation_ratio + 1.0;
        }
    }

    let max_ratio = if curve == 3 {
        VAR_BOOST_MAX_PQ_QSTEP_RATIO_BOOST
    } else {
        VAR_BOOST_MAX_QSTEP_RATIO_BOOST
    };
    qstep_ratio = qstep_ratio.clamp(1.0, max_ratio);

    let base_q = convert_qindex_to_q_fp8(i32::from(base_q_idx));
    let target_q = (f64::from(base_q) / qstep_ratio) as i32;

    let qdelta = compute_qdelta_fp(base_q, target_q);
    let boost = match curve {
        2 => (i32::from(base_q_idx) + 544) * -qdelta / (255 + 1024),
        3 => (i32::from(base_q_idx) + 2000) * -qdelta / (255 + 2000),
        _ => (i32::from(base_q_idx) + 200) * -qdelta / (255 + 200),
    };

    let max_range = if curve == 3 {
        VAR_BOOST_MAX_PQ_DELTAQ_RANGE
    } else {
        VAR_BOOST_MAX_DELTAQ_RANGE
    };
    boost.min(max_range)
}

/// Fork double-precision 8x8 variance for one block (`SVT_VAR_STORE` /
/// `svt_aom_get_mean_and_perpixel_variance` semantics on the producer
/// side): `(sum_sq - mean*mean) / (1 << VARIANCE_PRECISION)` in f64,
/// where mainline stores `(…) >> VARIANCE_PRECISION` as u16.
/// `VARIANCE_PRECISION` = 16 (C definitions.h).
#[inline]
pub fn fork_variance_store(sum_sq_minus_meansq: u64) -> f64 {
    sum_sq_minus_meansq as f64 / f64::from(1u32 << 16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qindex_to_q_fp8_endpoints() {
        // AC_QLOOKUP_8[0]=4 -> 256; [255]=1828 -> 116992 (table-derived).
        assert_eq!(convert_qindex_to_q_fp8(0), i32::from(AC_QLOOKUP_8[0]) << 6);
        assert_eq!(
            convert_qindex_to_q_fp8(255),
            i32::from(AC_QLOOKUP_8[255]) << 6
        );
    }

    #[test]
    fn qdelta_fp_roundtrip_zero() {
        for q in [0, 32, 128, 249, 255] {
            let fp8 = convert_qindex_to_q_fp8(q);
            assert_eq!(compute_qdelta_fp(fp8, fp8), 0, "q={q}");
        }
    }

    /// Formula pins: flat SB (all variances 1.0) at strength 2 / octile 5,
    /// base 255. Curve 0: qstep = 1.018^(0.8*80) = 1.018^64 ≈ 3.132 —
    /// matches the C comment table ("1 -> 3.132"). The boost must be
    /// positive and within range for every curve.
    #[test]
    fn flat_sb_boosts_within_range() {
        let vars = [1.0f64; 64];
        for curve in 0..=3u8 {
            let b = deltaq_sb_variance_boost(255, 30000, &vars, 1.0, 2, 5, curve);
            let cap = if curve == 3 { 120 } else { 80 };
            assert!(b > 0 && b <= cap, "curve {curve}: boost {b}");
        }
        // High-variance SB: no boost on any curve.
        let vars = [4096.0f64; 64];
        for curve in 0..=3u8 {
            let b = deltaq_sb_variance_boost(255, 30000, &vars, 4096.0, 2, 5, curve);
            assert_eq!(b, 0, "curve {curve}");
        }
    }

    /// PQ dark attenuation: a dark flat SB (mean small) must get a SMALLER
    /// boost than a bright one on curve 3, and be unaffected on curve 0;
    /// a mixed-variance SB (var64 > 256) is protected from attenuation.
    #[test]
    fn pq_dark_attenuation() {
        let vars = [1.0f64; 64];
        let bright = deltaq_sb_variance_boost(255, 26000, &vars, 1.0, 2, 5, 3);
        let dark = deltaq_sb_variance_boost(255, 1000, &vars, 1.0, 2, 5, 3);
        assert!(dark < bright, "dark {dark} !< bright {bright}");
        let protected = deltaq_sb_variance_boost(255, 1000, &vars, 300.0, 2, 5, 3);
        assert!(protected >= bright, "protect {protected} < {bright}");
        let c0_bright = deltaq_sb_variance_boost(255, 26000, &vars, 1.0, 2, 5, 0);
        let c0_dark = deltaq_sb_variance_boost(255, 1000, &vars, 1.0, 2, 5, 0);
        assert_eq!(c0_bright, c0_dark);
    }

    /// C comment table for strength 2 @ base 255 (curve 0):
    /// variance 256 -> qstep 1 (no boost); the octile smoothing with a
    /// uniform array reduces to the raw value, so pin the qstep ratios by
    /// recomputing the closed form at the comment's variance points.
    #[test]
    fn curve0_comment_table_qsteps() {
        for (var, expect) in [(256.0, 1.0), (64.0, 1.330), (16.0, 1.769), (4.0, 2.354), (1.0, 3.132)] {
            let q = 1.018f64.powf(0.8 * (-10.0 * (var as f64).log2() + 80.0));
            let q = q.clamp(1.0, 8.0);
            assert!(
                (q - expect).abs() < 0.002,
                "var {var}: qstep {q} vs C comment {expect}"
            );
        }
    }
}
