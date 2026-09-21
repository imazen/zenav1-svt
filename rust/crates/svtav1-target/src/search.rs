//! Bracketed qp search toward a quality-score target.
//!
//! Score is assumed MONOTONE NON-INCREASING in qp (higher qp = coarser
//! quantization = lower perceptual score) — the AV1 CQP direction. The
//! search brackets the target, bisecting in qp, spending at most
//! `max_encodes` trials; it returns the best trial seen (never an
//! un-encoded interpolation).

/// How each post-seed trial qp is chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepPolicy {
    /// Midpoint of the shrinking bracket — the classic census bisection.
    Bisect,
    /// Secant (linear-model) correction: `qp += (target - score)/slope`,
    /// slope fitted from the two most recent trials. The two-shot path:
    /// shot 1 from an anchor seed, shot 2 lands on the curve's local
    /// linearization. Falls back to bisection when the fitted slope is
    /// outside [`TargetOptions::slope_range`] (plateau / saturated
    /// content) or the step escapes the bracket.
    Secant,
}

/// Options for [`search_target_qp`].
#[derive(Debug, Clone, Copy)]
pub struct TargetOptions {
    /// Inclusive qp bounds (AV1 CQP domain, e.g. 1..=63 at the CLI scale
    /// the pipeline consumes).
    pub min_qp: u8,
    pub max_qp: u8,
    /// Stop early when a trial lands within `target ± tolerance`.
    /// `0.0` spends the full budget (census mode).
    pub tolerance: f64,
    /// Hard cap on trials (encode→judge cycles). The census k.
    /// `2` = the canonical one/two-shot budget: seed + one correction.
    pub max_encodes: u8,
    /// First trial qp. `None` = midpoint of the bounds (the content-blind
    /// control). The S1 anchor seed is the wired default source since
    /// 2026-08-28 via [`TargetOptions::seeded`] (`crate::seed`); the
    /// per-metric table is `crate::metric::anchor_qp_start`.
    pub qp_start: Option<u8>,
    /// Post-seed step policy; see [`StepPolicy`].
    pub step: StepPolicy,
    /// Sane |dscore/dqp| band for [`StepPolicy::Secant`] — the fitted
    /// slope's absolute value must sit inside or the step degrades to a
    /// bisection. Seed from [`crate::metric::MetricKind::slope_range`].
    pub slope_range: (f64, f64),
    /// Prior |dscore/dqp| used for the shot-2 correction when only one
    /// trial exists (no secant fittable yet). Seed from
    /// [`crate::metric::MetricKind::slope_hint`]; `None` = bisect shot 2.
    pub slope_hint: Option<f64>,
}

impl Default for TargetOptions {
    fn default() -> Self {
        Self {
            min_qp: 1,
            max_qp: 63,
            tolerance: 0.5,
            max_encodes: 3,
            qp_start: None,
            step: StepPolicy::Bisect,
            slope_range: (1e-4, 1e4),
            slope_hint: None,
        }
    }
}

/// Outcome of a search: the best (closest-scoring) trial actually encoded.
#[derive(Debug, Clone, Copy)]
pub struct TargetSearchResult {
    pub qp: u8,
    pub score: f64,
    /// Trials spent (= encodes performed).
    pub encodes_used: u8,
    /// `true` iff some trial landed inside the tolerance band.
    pub converged: bool,
}

/// Bracketed search: trial the seed, then bisect toward the target within
/// the shrinking qp bracket. `trial(qp)` performs one encode→judge cycle
/// and returns the achieved score; errors abort the search.
pub fn search_target_qp<E, Err>(
    target: f64,
    options: &TargetOptions,
    mut trial: E,
) -> Result<TargetSearchResult, Err>
where
    E: FnMut(u8) -> Result<f64, Err>,
{
    let mut lo = options.min_qp.min(options.max_qp);
    let mut hi = options.max_qp.max(options.min_qp);
    let budget = options.max_encodes.max(1);
    let mut qp = options
        .qp_start
        .unwrap_or_else(|| lo + (hi - lo) / 2)
        .clamp(lo, hi);
    let mut best: Option<TargetSearchResult> = None;
    let mut used = 0u8;
    // Most recent two trials (newest last) — the secant's slope fit.
    let mut history: Vec<(u8, f64)> = Vec::with_capacity(budget as usize);
    while used < budget {
        let score = trial(qp)?;
        used += 1;
        history.push((qp, score));
        let better = best.is_none_or(|b| (score - target).abs() < (b.score - target).abs());
        if better {
            best = Some(TargetSearchResult {
                qp,
                score,
                encodes_used: used,
                converged: (score - target).abs() <= options.tolerance,
            });
        }
        if (score - target).abs() <= options.tolerance && options.tolerance > 0.0 {
            break;
        }
        // Monotone non-increasing in qp: score too HIGH -> need coarser
        // (raise qp); too LOW -> need finer (lower qp).
        if score > target {
            lo = qp.saturating_add(1).min(hi);
        } else {
            hi = qp.saturating_sub(1).max(lo);
        }
        if lo >= hi && used < budget {
            qp = lo;
            if best.is_some_and(|b| b.qp == qp) {
                break; // bracket collapsed onto an already-tried qp
            }
            continue;
        }
        qp = next_trial_qp(qp, target, lo, hi, &history, options);
        // Never re-encode a qp already trialled — degenerate to bisect.
        if history.iter().any(|(q, _)| *q == qp) {
            qp = lo + (hi - lo) / 2;
        }
    }
    let mut r = best.expect("budget >= 1 guarantees at least one trial");
    r.encodes_used = used;
    Ok(r)
}

/// Pick the next qp after a non-converged trial. [`StepPolicy::Secant`]
/// linearizes the score curve through the two most recent trials and
/// steps to the target's x-intercept; the step must land inside the
/// bracket with a sane fitted slope or it degrades to the midpoint.
fn next_trial_qp(
    _qp: u8,
    target: f64,
    lo: u8,
    hi: u8,
    history: &[(u8, f64)],
    options: &TargetOptions,
) -> u8 {
    if options.step == StepPolicy::Secant {
        // Two-point secant if we have one, else the per-metric prior
        // slope for the very first correction (the two-shot step).
        let fitted = if history.len() >= 2 {
            let (q1, s1) = history[history.len() - 1];
            let (q0, s0) = history[history.len() - 2];
            (q1 != q0).then(|| {
                (
                    f64::from(q1),
                    s1,
                    (s1 - s0) / (f64::from(q1) - f64::from(q0)),
                )
            })
        } else {
            let (q1, s1) = history[history.len() - 1];
            options.slope_hint.map(|h| (f64::from(q1), s1, -h.abs()))
        };
        if let Some((q1, s1, slope)) = fitted {
            let (smin, smax) = options.slope_range;
            // Expect a NEGATIVE slope (score falls as qp rises); a flat or
            // wrong-sign fit means the local model is useless.
            let sane = slope.is_finite() && slope < 0.0 && (-slope) >= smin && (-slope) <= smax;
            if sane {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let step = (q1 + (target - s1) / slope).round();
                if step > f64::from(lo) && step < f64::from(hi) {
                    return step as u8;
                }
            }
        }
    }
    lo + (hi - lo) / 2
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthetic monotone curve: score = 100 - 1.4*qp (plus a plateau tail).
    fn curve(qp: u8) -> f64 {
        (100.0 - 1.4 * f64::from(qp)).max(5.0)
    }

    #[test]
    fn converges_on_a_monotone_curve() {
        let r = search_target_qp::<_, ()>(
            80.0,
            &TargetOptions {
                tolerance: 1.0,
                max_encodes: 6,
                ..Default::default()
            },
            |qp| Ok(curve(qp)),
        )
        .unwrap();
        assert!(r.converged, "{r:?}");
        assert!((r.score - 80.0).abs() <= 1.0, "{r:?}");
        // score 80 => qp ~14.3
        assert!((13..=16).contains(&r.qp), "{r:?}");
    }

    #[test]
    fn census_mode_spends_the_full_budget_and_returns_best() {
        let mut n = 0u8;
        let r = search_target_qp::<_, ()>(
            70.0,
            &TargetOptions {
                tolerance: 0.0,
                max_encodes: 3,
                ..Default::default()
            },
            |qp| {
                n += 1;
                Ok(curve(qp))
            },
        )
        .unwrap();
        assert_eq!(n, 3);
        assert_eq!(r.encodes_used, 3);
        // best of the 3 trials, not the last
        assert!((r.score - 70.0).abs() <= (curve(32) - 70.0).abs());
    }

    #[test]
    fn unreachable_target_returns_closest_edge() {
        let r = search_target_qp::<_, ()>(
            200.0,
            &TargetOptions {
                tolerance: 0.0,
                max_encodes: 4,
                ..Default::default()
            },
            |qp| Ok(curve(qp)),
        )
        .unwrap();
        assert!(!r.converged);
        assert!(
            r.qp <= 8,
            "should walk toward the fine edge (bisection from 32 reaches ~4 in 4 trials): {r:?}"
        );
    }

    #[test]
    fn secant_two_shot_converges_on_linear_curve() {
        // Anchor-seeded + secant on a perfectly linear curve: shot 2's
        // slope fit is exact, so it must land on the oracle qp within
        // rounding — the "two-shot" claim made concrete.
        let r = search_target_qp::<_, ()>(
            80.0,
            &TargetOptions {
                tolerance: 0.5,
                max_encodes: 2,
                qp_start: Some(30), // deliberately off-basin
                step: StepPolicy::Secant,
                slope_range: (0.5, 4.0),
                slope_hint: Some(1.4), // the synthetic curve's true slope
                ..Default::default()
            },
            |qp| Ok(curve(qp)),
        )
        .unwrap();
        assert!(r.converged, "{r:?}");
        assert!((r.score - 80.0).abs() <= 0.5, "{r:?}");
        assert!(r.encodes_used <= 2, "{r:?}");
        assert!((13..=16).contains(&r.qp), "{r:?}");
    }

    #[test]
    fn secant_degrades_to_bisect_on_flat_plateau() {
        // Slope ~0 (plateau) is outside the sane band -> bisect, still
        // bounded, still returns a real trial.
        let flat = |qp: u8| Ok(if qp < 40 { 90.0 } else { 10.0 });
        let r = search_target_qp::<_, ()>(
            50.0,
            &TargetOptions {
                tolerance: 0.0,
                max_encodes: 4,
                qp_start: Some(10),
                step: StepPolicy::Secant,
                slope_range: (0.5, 4.0),
                ..Default::default()
            },
            flat,
        )
        .unwrap();
        // A step function never hits 50; best = nearest edge, budget spent.
        assert_eq!(r.encodes_used, 4);
        assert!(r.score == 90.0 || r.score == 10.0);
    }

    #[test]
    fn secant_never_reencodes_a_trialled_qp() {
        let mut seen = std::collections::HashSet::new();
        let _ = search_target_qp::<_, ()>(
            80.0,
            &TargetOptions {
                tolerance: 0.0,
                max_encodes: 5,
                qp_start: Some(30),
                step: StepPolicy::Secant,
                slope_range: (0.5, 4.0),
                ..Default::default()
            },
            |qp| {
                assert!(seen.insert(qp), "qp {qp} re-trialled");
                Ok(curve(qp))
            },
        )
        .unwrap();
    }

    #[test]
    fn errors_abort() {
        let e = search_target_qp(
            80.0,
            &TargetOptions::default(),
            |_qp| -> Result<f64, &'static str> { Err("encode failed") },
        );
        assert_eq!(e.unwrap_err(), "encode failed");
    }
}
