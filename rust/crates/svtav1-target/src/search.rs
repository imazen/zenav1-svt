//! Bracketed qp search toward a quality-score target.
//!
//! Score is assumed MONOTONE NON-INCREASING in qp (higher qp = coarser
//! quantization = lower perceptual score) — the AV1 CQP direction. The
//! search brackets the target, bisecting in qp, spending at most
//! `max_encodes` trials; it returns the best trial seen (never an
//! un-encoded interpolation).

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
    pub max_encodes: u8,
    /// First trial qp. `None` = midpoint of the bounds (the content-blind
    /// control). The S1 anchor seed is the wired default source since
    /// 2026-08-28 via [`TargetOptions::seeded`] (`crate::seed`).
    pub qp_start: Option<u8>,
}

impl Default for TargetOptions {
    fn default() -> Self {
        Self {
            min_qp: 1,
            max_qp: 63,
            tolerance: 0.5,
            max_encodes: 3,
            qp_start: None,
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
    while used < budget {
        let score = trial(qp)?;
        used += 1;
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
        qp = lo + (hi - lo) / 2;
    }
    let mut r = best.expect("budget >= 1 guarantees at least one trial");
    r.encodes_used = used;
    Ok(r)
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
    fn errors_abort() {
        let e = search_target_qp(
            80.0,
            &TargetOptions::default(),
            |_qp| -> Result<f64, &'static str> { Err("encode failed") },
        );
        assert_eq!(e.unwrap_err(), "encode failed");
    }
}
