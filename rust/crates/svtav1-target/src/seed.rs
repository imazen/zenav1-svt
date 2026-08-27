//! S1 one-shot qp seed — the fitted content-blind-per-target anchors that
//! replace the midpoint seed for zensim-targeted encodes.
//!
//! Fit + census: `benchmarks/zensim_hdr_target_wave_2026-08-27.md` (SEED ARM
//! S1/S2) — anchors are per-target median oracle qp over 25,171 era-B
//! hdrgrid cells (842 non-census renditions, census scenes excluded), frozen
//! gates passed at 4-5× margin: census k2 median |err| 17.638 → 3.306
//! (±2 hits 1→11), k3 7.431 → 1.513 (9→20), t88 26.68 → ~1. S2's terciles
//! lose the registered fewer-consts tie-break. **User-approved default
//! wiring 2026-08-28** (AskUserQuestion; the propose-only rule was lifted
//! for this head by an explicit yes).
//!
//! Generalization beyond the three fitted targets: linear interpolation
//! between anchors, clamped flat outside [70, 88] — a seed only needs the
//! basin; the bracketed search corrects the residual either way.

use crate::search::TargetOptions;

/// The fitted (target, qp) anchors, ascending by target.
pub const S1_ANCHORS: [(f64, u8); 3] = [(70.0, 22), (80.0, 13), (88.0, 5)];

/// S1 seed qp for `target`: anchor interpolation (see module docs).
/// `None` on a non-finite target — callers keep the midpoint seed.
#[must_use]
pub fn s1_qp_start(target: f64) -> Option<u8> {
    if !target.is_finite() {
        return None;
    }
    let (t_lo, q_lo) = S1_ANCHORS[0];
    let (t_hi, q_hi) = S1_ANCHORS[S1_ANCHORS.len() - 1];
    if target <= t_lo {
        return Some(q_lo);
    }
    if target >= t_hi {
        return Some(q_hi);
    }
    for w in S1_ANCHORS.windows(2) {
        let (t0, q0) = w[0];
        let (t1, q1) = w[1];
        if target <= t1 {
            let f = (target - t0) / (t1 - t0);
            let q = f64::from(q0) + f * (f64::from(q1) - f64::from(q0));
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            return Some(q.round().clamp(1.0, 63.0) as u8);
        }
    }
    Some(q_hi)
}

impl TargetOptions {
    /// The canonical options for a zensim-targeted encode at `target`:
    /// defaults plus the S1 seed as `qp_start`. Plain [`Default`] stays
    /// midpoint-seeded for callers that bring their own seed.
    #[must_use]
    pub fn seeded(target: f64) -> Self {
        Self {
            qp_start: s1_qp_start(target),
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchors_are_exact() {
        assert_eq!(s1_qp_start(70.0), Some(22));
        assert_eq!(s1_qp_start(80.0), Some(13));
        assert_eq!(s1_qp_start(88.0), Some(5));
    }

    #[test]
    fn interpolation_and_clamps() {
        assert_eq!(s1_qp_start(75.0), Some(18)); // (22+13)/2 = 17.5 rounds up
        assert_eq!(s1_qp_start(84.0), Some(9));  // 13 + 0.5*(5-13) = 9
        assert_eq!(s1_qp_start(60.0), Some(22)); // clamp-flat below band
        assert_eq!(s1_qp_start(95.0), Some(5));  // clamp-flat above band
        assert_eq!(s1_qp_start(f64::NAN), None);
    }

    #[test]
    fn seeded_options_carry_the_anchor() {
        let o = TargetOptions::seeded(80.0);
        assert_eq!(o.qp_start, Some(13));
        assert_eq!(o.max_encodes, TargetOptions::default().max_encodes);
    }
}
