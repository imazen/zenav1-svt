//! Metric identity, direction, and per-metric seed anchors.
//!
//! A target-quality encode names ONE metric as its constraint
//! ([`MetricKind`]); the judge may still return a whole score bundle per
//! decode, and the extra scores ride along in the result for logging —
//! "more than one metric" costs nothing once the recon exists.
//!
//! Anchors are CONTENT-BLIND per-target median oracle qp, fitted per
//! metric (the S1 pattern in `crate::seed`): enough to put the first
//! trial inside the basin, then the secant step in `search_target_qp`
//! corrects the residual. They are NOT a quality guarantee — only a
//! seed. Fit provenance is recorded per table.
//!
//! Sign convention: the search assumes score non-increasing in qp
//! (coarser quant -> worse perceptual). Distance metrics (butteraugli)
//! invert that; [`MetricKind::sign`] maps them into the convention so
//! one search loop serves both families.

/// Which metric a target names, and the search's scale/direction
/// contract for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricKind {
    /// SSIMULACRA2, 0..100-ish (can go negative on hard content), higher
    /// better. The measured surface of the still-image tune sweeps.
    Ssimulacra2,
    /// zensim, 0..100, higher better — the HDR-target corpus metric.
    Zensim,
    /// CVVDP JOD units, higher better.
    CvvdP,
    /// butteraugli distance, LOWER better (distance, not quality).
    Butteraugli,
    /// Anything else the judge reports by name; anchors fall back to the
    /// midpoint seed and slope bounds to the wide default.
    Custom,
}

impl MetricKind {
    /// +1 for higher-is-better quality metrics, -1 for distance metrics —
    /// multiplied through score AND target so the search's monotone
    /// non-increasing assumption holds for both families.
    #[must_use]
    pub fn sign(self) -> f64 {
        match self {
            Self::Butteraugli => -1.0,
            _ => 1.0,
        }
    }

    /// Prior |dscore/dqp| for the FIRST correction, when only one real
    /// trial exists and no secant can be fitted yet — the shot-2 slope.
    /// Measured medians near the anchored band; `Custom` splits its
    /// (very wide) range geometrically.
    #[must_use]
    pub fn slope_hint(self) -> f64 {
        match self {
            // 42-img still subset, t3 arm: median 1.13 (see anchors note).
            Self::Ssimulacra2 => 1.15,
            // S1 anchor pair slope: (80-70)/(22-13) ~= 1.1.
            Self::Zensim => 1.05,
            Self::CvvdP => 0.08,
            Self::Butteraugli => 0.15,
            Self::Custom => 1.0,
        }
    }

    /// Sane |dscore/dqp| bounds for the secant step — outside this band
    /// the fitted slope is noise (plateau, saturated content) and the
    /// search falls back to bisection for that step.
    #[must_use]
    pub fn slope_range(self) -> (f64, f64) {
        match self {
            // measured on the 42-img still subset, t3 arm: median 1.13,
            // p25/p75 0.87/1.84 near the 65-80 band.
            Self::Ssimulacra2 => (0.2, 6.0),
            Self::Zensim => (0.2, 8.0),
            Self::CvvdP => (0.01, 0.6),
            Self::Butteraugli => (0.01, 1.5),
            Self::Custom => (1e-4, 1e4),
        }
    }

    /// Per-target seed anchors `(target, qp)`, ascending by target.
    /// Empty for unmeasured metrics — callers fall back to midpoint.
    #[must_use]
    pub fn anchors(self) -> &'static [(f64, u8)] {
        match self {
            // Fitted 2026-09-19 on the 42-image k-means imazen-26 subset,
            // tune-IQ (t3) arm, qp{20,32,44}, presets -1..12: per-image
            // oracle qp = inversion of the piecewise-linear ssim2(qp)
            // curve, median across images. Stable across preset bands
            // (research/mid/fast medians differ by <=2 qp) because IQ's
            // tools compress the preset spread — one table serves all
            // still presets. Raw fit in benchmarks/
            // still_image_tune_v1_{zones,backfill,oct}_2026-09-19.tsv.
            Self::Ssimulacra2 => &[(45.0, 44), (55.0, 42), (65.0, 37), (75.0, 31), (85.0, 20)],
            // S1 anchors, fitted on 25,171 era-B hdrgrid cells
            // (benchmarks/zensim_hdr_target_wave_2026-08-27.md) — the
            // original content-blind seed that replaced the midpoint.
            Self::Zensim => &[(70.0, 22), (80.0, 13), (88.0, 5)],
            _ => &[],
        }
    }
}

/// Anchor-table seed qp for `(metric, target)`: linear interpolation
/// between anchors, clamped flat outside the fitted band. `None` for
/// non-finite targets or metrics with no fitted table — the caller then
/// uses the midpoint seed.
#[must_use]
pub fn anchor_qp_start(metric: MetricKind, target: f64) -> Option<u8> {
    if !target.is_finite() {
        return None;
    }
    let anchors = metric.anchors();
    let (first, last) = (anchors.first()?, anchors.last()?);
    if target <= first.0 {
        return Some(first.1);
    }
    if target >= last.0 {
        return Some(last.1);
    }
    for w in anchors.windows(2) {
        let (t0, q0) = w[0];
        let (t1, q1) = w[1];
        if target <= t1 {
            let f = (target - t0) / (t1 - t0);
            let q = f64::from(q0) + f * (f64::from(q1) - f64::from(q0));
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            return Some(q.round().clamp(1.0, 63.0) as u8);
        }
    }
    Some(last.1)
}

/// A named metric score from a judge's per-trial bundle.
pub type MetricScore = (MetricKind, f64);

/// Pull the target metric's score out of a judge bundle. `None` means
/// the judge did not report the named metric for this trial — the
/// caller treats it as a judge failure, never substitutes another.
#[must_use]
pub fn score_of(bundle: &[MetricScore], metric: MetricKind) -> Option<f64> {
    bundle.iter().find(|(m, _)| *m == metric).map(|(_, s)| *s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssim2_anchors_interpolate_and_clamp() {
        assert_eq!(anchor_qp_start(MetricKind::Ssimulacra2, 45.0), Some(44));
        assert_eq!(anchor_qp_start(MetricKind::Ssimulacra2, 85.0), Some(20));
        assert_eq!(anchor_qp_start(MetricKind::Ssimulacra2, 75.0), Some(31));
        // 70 -> midpoint of (65,37)-(75,31): 37 + 0.5*(31-37) = 34
        assert_eq!(anchor_qp_start(MetricKind::Ssimulacra2, 70.0), Some(34));
        // outside the fitted band: flat clamp
        assert_eq!(anchor_qp_start(MetricKind::Ssimulacra2, 95.0), Some(20));
        assert_eq!(anchor_qp_start(MetricKind::Ssimulacra2, 20.0), Some(44));
    }

    #[test]
    fn unmeasured_metrics_fall_back() {
        assert_eq!(anchor_qp_start(MetricKind::CvvdP, 8.0), None);
        assert_eq!(anchor_qp_start(MetricKind::Custom, 1.0), None);
        assert_eq!(anchor_qp_start(MetricKind::Ssimulacra2, f64::NAN), None);
    }

    #[test]
    fn distance_metrics_flip_sign() {
        assert_eq!(MetricKind::Butteraugli.sign(), -1.0);
        assert_eq!(MetricKind::Ssimulacra2.sign(), 1.0);
    }

    #[test]
    fn score_of_picks_only_the_named_metric() {
        let bundle = [(MetricKind::Ssimulacra2, 72.5), (MetricKind::Zensim, 81.0)];
        assert_eq!(score_of(&bundle, MetricKind::Ssimulacra2), Some(72.5));
        assert_eq!(score_of(&bundle, MetricKind::Zensim), Some(81.0));
        assert_eq!(score_of(&bundle, MetricKind::CvvdP), None);
    }
}
