//! Encode-to-target: the codec-owned trial cell around
//! [`crate::search::search_target_qp`].
//!
//! The metric judge is INJECTED (a [`MetricJudge`] over the decoder-exact
//! reconstruction) — the crate owns encoding + recon + search; the
//! harness owns judge wiring, so a judge swap never touches this crate.
//!
//! `configure` is the tune/enhancement hook: it runs after the pipeline's
//! baseline is built and before the frame is encoded, so every trial
//! inherits `hdr.tune` (IQ), `enhancements` (`StillImageTune`), QM and
//! variance-boost overrides — whatever the caller sets. That is what
//! makes "target-quality under tune IQ" expressible: without it the
//! search could only encode default-tune trials.

use zenav1_svt::encoder::pipeline::EncodePipeline;
use zenav1_svt::encoder::rate_control::{RcConfig, RcMode};

use crate::metric::{MetricKind, MetricScore, anchor_qp_start, score_of};
use crate::search::{StepPolicy, TargetOptions, TargetSearchResult, search_target_qp};

/// Decoder-exact reconstruction planes of one trial — the pixels a
/// conforming decoder (or an AVIF writer) would reproduce.
#[derive(Debug)]
pub enum TrialRecon {
    /// 8-bit path (`try_encode_frame_420` -> `last_recon`), ALIGNED
    /// strides (luma `aligned_w`, chroma `aligned_w/2`).
    Bd8 { y: Vec<u8>, u: Vec<u8>, v: Vec<u8> },
    /// 10-bit path (`try_encode_frame_420_hbd` -> `last_recon10_final`),
    /// same aligned strides.
    Bd10 {
        y: Vec<u16>,
        u: Vec<u16>,
        v: Vec<u16>,
    },
}

/// One trial's output — everything a metric judge needs to score the
/// decoded frame without touching the encoder again.
#[derive(Debug)]
pub struct TrialOutput {
    /// Decoder-exact recon (bit depth per variant).
    pub recon: TrialRecon,
    /// Aligned luma stride (chroma stride is half).
    pub aligned_w: usize,
    /// OBU payload bytes (the stream an AVIF writer would ship).
    pub bytes: Vec<u8>,
    /// The qp this trial encoded at.
    pub qp: u8,
}

/// Judge contract for the multi-metric path: score the decoded trial
/// and return every metric it knows. The search consumes only the
/// [`TargetSpec`]'s named metric; the rest ride along as side-channel
/// scores in [`TargetEncodeResult::scores`] — one decode, many metrics.
///
/// A single-metric judge returns a one-element bundle. A missing bundle
/// entry for the named metric means the judge did not achieve a score —
/// the trial counts but can never win (the search never silently
/// substitutes another metric).
pub trait MetricJudge<E> {
    /// Score `out` and return `(metric, value)` pairs on each metric's
    /// own scale (higher-better for quality metrics; raw distance for
    /// `MetricKind::Butteraugli` — the search applies its `sign`).
    fn score(&mut self, out: &TrialOutput) -> Result<Vec<MetricScore>, E>;
}

/// Blanket impl: any closure `FnMut(&TrialOutput) -> Result<Vec<..>, E>`
/// is a judge — the ergonomic shape for harnesses shelling out to
/// `zenmetrics score --metric …` per side metric.
impl<E, F> MetricJudge<E> for F
where
    F: FnMut(&TrialOutput) -> Result<Vec<MetricScore>, E>,
{
    fn score(&mut self, out: &TrialOutput) -> Result<Vec<MetricScore>, E> {
        self(out)
    }
}

/// The full target contract for the trial drivers.
///
/// Seed provenance: `options.qp_start` is THE content-conditioned
/// injection point. Its sources, strongest to weakest, form the same
/// fallback chain the sibling pickers use (zenavif `auto_tune`):
/// a zenanalyze-feature-conditioned predictor (baked ZNPR over
/// `feat_*` + target + preset — the planned step once a
/// features×qp×metric training sidecar exists) → per-metric anchor
/// tables ([`crate::metric::anchor_qp_start`], fitted content-blind
/// medians) → `None` = midpoint. This crate stays model-free by
/// design: a predictor is a CALLER-SIDE input into `qp_start`, never
/// a crate dependency, so an unbaked/missing model degrades to
/// anchors exactly like `ModelNotBaked` does in zenavif.
#[derive(Debug, Clone)]
pub struct TargetSpec {
    /// The constrained metric (its scale defines `value`/tolerance).
    pub metric: MetricKind,
    /// Target value on `metric`'s own scale (pre-`sign`).
    pub value: f64,
    /// Search behaviour: bounds, tolerance, budget, seed, step policy.
    pub options: TargetOptions,
}

impl TargetSpec {
    /// One/two-shot SSIMULACRA2 target: per-metric anchor seed + secant
    /// correction, `max_encodes = 2` — the canonical answer to "target
    /// quality" on the still-image surface. The anchor puts shot 1 in
    /// the basin (fitted on the tune-IQ still corpus; see
    /// [`crate::metric`]); the secant corrects the residual on shot 2.
    #[must_use]
    pub fn ssim2(value: f64) -> Self {
        Self {
            metric: MetricKind::Ssimulacra2,
            value,
            options: TargetOptions {
                max_encodes: 2,
                qp_start: anchor_qp_start(MetricKind::Ssimulacra2, value),
                step: StepPolicy::Secant,
                slope_range: MetricKind::Ssimulacra2.slope_range(),
                slope_hint: Some(MetricKind::Ssimulacra2.slope_hint()),
                ..Default::default()
            },
        }
    }

    /// zensim target seeded by the S1 anchors (fitted on the era-B
    /// hdrgrid corpus — [`crate::seed`]); same two-shot budget.
    #[must_use]
    pub fn zensim(value: f64) -> Self {
        Self {
            metric: MetricKind::Zensim,
            value,
            options: TargetOptions {
                max_encodes: 2,
                qp_start: anchor_qp_start(MetricKind::Zensim, value),
                step: StepPolicy::Secant,
                slope_range: MetricKind::Zensim.slope_range(),
                slope_hint: Some(MetricKind::Zensim.slope_hint()),
                ..Default::default()
            },
        }
    }

    /// A custom metric with no fitted anchors: midpoint seed, bisect —
    /// the honest pre-anchored behaviour. `configure` the options for a
    /// bigger budget or a hand seed.
    #[must_use]
    pub fn custom(metric: MetricKind, value: f64) -> Self {
        Self {
            metric,
            value,
            options: TargetOptions {
                slope_range: metric.slope_range(),
                ..Default::default()
            },
        }
    }
}

/// Outcome of `encode_to_target*`: the winning trial's stream + recon
/// plus every score the judge reported for it.
#[derive(Debug)]
pub struct TargetEncodeResult {
    /// Best trial's decoder-exact recon.
    pub recon: TrialRecon,
    /// Aligned luma stride (chroma half).
    pub aligned_w: usize,
    /// Best trial's OBU bytes.
    pub bytes: Vec<u8>,
    /// Search outcome (qp, achieved target-metric score on its own
    /// scale, encodes used, convergence flag).
    pub search: TargetSearchResult,
    /// The winning trial's full score bundle — target metric plus
    /// side-channel metrics from the same decode.
    pub scores: Vec<MetricScore>,
}

/// Errors from the `encode_to_target*` drivers.
#[derive(Debug)]
pub enum TargetError<E> {
    /// `EncodePipeline` refused/failed (the `EncodeError` Debug payload).
    Encode(String),
    /// A recon field the path requires came back `None` — outside the
    /// recon envelope, loud by design.
    ReconMissing { qp: u8 },
    /// The judge returned `Err`.
    Judge(E),
}

impl<E> From<TargetError<E>> for String
where
    E: std::fmt::Display,
{
    fn from(e: TargetError<E>) -> Self {
        match e {
            TargetError::Encode(s) => format!("encode: {s}"),
            TargetError::ReconMissing { qp } => format!("recon missing at qp {qp}"),
            TargetError::Judge(e) => format!("judge: {e}"),
        }
    }
}

/// bd8 4:2:0 target encode — the still-image surface.
///
/// `configure` runs on every trial's pipeline between construction and
/// the frame encode — e.g.
/// `|p: &mut EncodePipeline| p.hdr.tune = svt_tune::SvtTune::Iq.to_raw()`
/// or `p.enhancements = p.enhancements.with(ZenEnhancement::StillImageTune)`.
/// It MUST be deterministic and identical across trials or the qp→score
/// curve is not comparable.
///
/// The judge sees each trial's decoded recon once; return every metric
/// it can compute — `spec.metric` selects which drives the search.
#[allow(clippy::too_many_arguments)]
pub fn encode_to_target_8bit<J, C, E>(
    y: &[u8],
    u: &[u8],
    v: &[u8],
    width: usize,
    height: usize,
    preset: u8,
    spec: &TargetSpec,
    mut judge: J,
    configure: C,
) -> Result<TargetEncodeResult, TargetError<E>>
where
    J: MetricJudge<E>,
    C: Fn(&mut EncodePipeline),
{
    let sign = spec.metric.sign();
    let mut trials: Vec<(u8, TrialOutput, Vec<MetricScore>)> = Vec::new();
    let search = search_target_qp(spec.value * sign, &spec.options, |qp| {
        let (out, bundle) =
            run_trial_bd8(y, u, v, width, height, preset, qp, &configure, &mut judge)?;
        let target_score = score_of(&bundle, spec.metric).unwrap_or(f64::NAN);
        trials.push((qp, out, bundle));
        Ok::<_, TargetError<E>>(target_score * sign)
    })?;
    Ok(finish(search, trials, sign))
}

/// bd10 4:2:0 target encode — the HDR twin of [`encode_to_target_8bit`].
/// Same spec/judge/configure contract; the recon is
/// `last_recon10_final` (10-bit decoder-exact).
#[allow(clippy::too_many_arguments)]
pub fn encode_to_target_10bit<J, C, E>(
    y: &[u16],
    u: &[u16],
    v: &[u16],
    width: usize,
    height: usize,
    preset: u8,
    spec: &TargetSpec,
    mut judge: J,
    configure: C,
) -> Result<TargetEncodeResult, TargetError<E>>
where
    J: MetricJudge<E>,
    C: Fn(&mut EncodePipeline),
{
    let sign = spec.metric.sign();
    let mut trials: Vec<(u8, TrialOutput, Vec<MetricScore>)> = Vec::new();
    let search = search_target_qp(spec.value * sign, &spec.options, |qp| {
        let (out, bundle) =
            run_trial_bd10(y, u, v, width, height, preset, qp, &configure, &mut judge)?;
        let target_score = score_of(&bundle, spec.metric).unwrap_or(f64::NAN);
        trials.push((qp, out, bundle));
        Ok::<_, TargetError<E>>(target_score * sign)
    })?;
    Ok(finish(search, trials, sign))
}

/// One bd8 encode→judge cycle at `qp`.
#[allow(clippy::too_many_arguments)]
fn run_trial_bd8<J, C, E>(
    y: &[u8],
    u: &[u8],
    v: &[u8],
    width: usize,
    height: usize,
    preset: u8,
    qp: u8,
    configure: &C,
    judge: &mut J,
) -> Result<(TrialOutput, Vec<MetricScore>), TargetError<E>>
where
    J: MetricJudge<E>,
    C: Fn(&mut EncodePipeline),
{
    let mut pipe = trial_pipeline(width, height, preset, qp, 8);
    configure(&mut pipe);
    let bytes = pipe
        .try_encode_frame_420(y, u, v, width)
        .map_err(|e| TargetError::Encode(format!("{e:?}")))?;
    let (ry, ru, rv) = pipe
        .last_recon
        .take()
        .ok_or(TargetError::ReconMissing { qp })?;
    let out = TrialOutput {
        recon: TrialRecon::Bd8 {
            y: ry,
            u: ru,
            v: rv,
        },
        aligned_w: width.next_multiple_of(64),
        bytes,
        qp,
    };
    let bundle = judge.score(&out).map_err(TargetError::Judge)?;
    Ok((out, bundle))
}

/// One bd10 encode→judge cycle at `qp`.
#[allow(clippy::too_many_arguments)]
fn run_trial_bd10<J, C, E>(
    y: &[u16],
    u: &[u16],
    v: &[u16],
    width: usize,
    height: usize,
    preset: u8,
    qp: u8,
    configure: &C,
    judge: &mut J,
) -> Result<(TrialOutput, Vec<MetricScore>), TargetError<E>>
where
    J: MetricJudge<E>,
    C: Fn(&mut EncodePipeline),
{
    let mut pipe = trial_pipeline(width, height, preset, qp, 10);
    configure(&mut pipe);
    let bytes = pipe
        .try_encode_frame_420_hbd(y, u, v, width)
        .map_err(|e| TargetError::Encode(format!("{e:?}")))?;
    let (ry, ru, rv) = pipe
        .last_recon10_final
        .take()
        .ok_or(TargetError::ReconMissing { qp })?;
    let out = TrialOutput {
        recon: TrialRecon::Bd10 {
            y: ry,
            u: ru,
            v: rv,
        },
        aligned_w: width.next_multiple_of(64),
        bytes,
        qp,
    };
    let bundle = judge.score(&out).map_err(TargetError::Judge)?;
    Ok((out, bundle))
}

/// The trial pipeline baseline: CQP at `qp`, single tile, 4:2:0, recon
/// on. `configure` mutates it before encode (tune, enhancements, QM…).
fn trial_pipeline(
    width: usize,
    height: usize,
    preset: u8,
    qp: u8,
    bit_depth: u8,
) -> EncodePipeline {
    let rc = RcConfig {
        mode: RcMode::Cqp,
        qp,
        ..RcConfig::default()
    };
    EncodePipeline::new(width as u32, height as u32, preset, rc, 0, 1)
        .with_bit_depth(bit_depth)
        .with_chroma_420(true)
        .with_tile_rows_log2(0)
        .with_tile_cols_log2(0)
        .with_sb_size(None)
        .with_recon_output(true)
}

/// Pick the trial the search selected and repackage it with its full
/// score bundle; the reported score returns on the metric's own scale.
fn finish(
    search: TargetSearchResult,
    trials: Vec<(u8, TrialOutput, Vec<MetricScore>)>,
    sign: f64,
) -> TargetEncodeResult {
    let (_, out, scores) = trials
        .into_iter()
        .rfind(|(qp, _, _)| *qp == search.qp)
        .expect("search only returns a qp it trialled");
    TargetEncodeResult {
        recon: out.recon,
        aligned_w: out.aligned_w,
        bytes: out.bytes,
        search: TargetSearchResult {
            score: search.score * sign,
            ..search
        },
        scores,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 128x128 bd8 4:2:0 synthetic: smooth gradient + texture — real
    /// RD-tradeoff work at tiny cost.
    fn synth8() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let (w, h) = (128usize, 128usize);
        let mut y = vec![0u8; w * h];
        for r in 0..h {
            for c in 0..w {
                let g = 60.0
                    + 140.0 * (c as f32 / w as f32)
                    + 30.0 * ((c as f32 * 0.35).sin() * (r as f32 * 0.27).cos());
                y[r * w + c] = g.clamp(16.0, 235.0) as u8;
            }
        }
        let (cw, ch) = (w / 2, h / 2);
        (y, vec![128u8; cw * ch], vec![120u8; cw * ch])
    }

    /// Judge: 100 - mean|Δ| on luma recon vs a captured source — cheap,
    /// monotone in quality, exercises the recon plumbing for real. Also
    /// reports a fake second metric to prove the bundle rides along.
    fn mad_judge(
        src_y: &[u8],
        w: usize,
    ) -> impl FnMut(&TrialOutput) -> Result<Vec<MetricScore>, String> {
        let src = src_y.to_vec();
        move |out| {
            let TrialRecon::Bd8 { y, .. } = &out.recon else {
                return Err("expected bd8 recon".into());
            };
            let mad = y
                .iter()
                .zip(src.iter())
                .take(w * w)
                .map(|(a, b)| f64::from(*a) - f64::from(*b))
                .map(f64::abs)
                .sum::<f64>()
                / (w * w) as f64;
            Ok(vec![
                (MetricKind::Ssimulacra2, 100.0 - 2.0 * mad),
                (MetricKind::Zensim, 100.0 - mad),
            ])
        }
    }

    #[test]
    fn bd8_target_search_returns_a_real_trial() {
        let (y, u, v) = synth8();
        let spec = TargetSpec::ssim2(70.0);
        let out =
            encode_to_target_8bit(&y, &u, &v, 128, 128, 10, &spec, mad_judge(&y, 128), |_| {})
                .expect("bd8 target encode");
        assert!(matches!(out.recon, TrialRecon::Bd8 { .. }));
        assert!(out.search.encodes_used <= 2, "{:?}", out.search);
        assert!(out.search.qp >= 1 && out.search.qp <= 63);
        // The full bundle rode along — both metrics scored from one decode.
        assert_eq!(out.scores.len(), 2);
        assert!(out.scores.iter().any(|(m, _)| *m == MetricKind::Zensim));
    }

    #[test]
    fn configure_hook_reaches_every_trial() {
        let (y, u, v) = synth8();
        let spec = TargetSpec {
            metric: MetricKind::Ssimulacra2,
            value: 50.0,
            options: TargetOptions {
                max_encodes: 1,
                qp_start: Some(32),
                tolerance: 0.0,
                ..Default::default()
            },
        };
        // Default tune vs tune IQ at the same qp: if the configure hook
        // does not reach the trial, the bytes are identical.
        let def =
            encode_to_target_8bit(&y, &u, &v, 128, 128, 10, &spec, mad_judge(&y, 128), |_| {})
                .expect("default");
        let iq = encode_to_target_8bit(&y, &u, &v, 128, 128, 10, &spec, mad_judge(&y, 128), |p| {
            p.hdr.tune = zenav1_svt::encoder::tune::TUNE_IQ
        })
        .expect("iq");
        assert_ne!(
            def.bytes, iq.bytes,
            "tune IQ must change the trial stream at fixed qp"
        );
    }

    #[test]
    fn missing_named_metric_never_wins() {
        let (y, u, v) = synth8();
        // Judge reports only zensim; spec constrains ssim2 — every trial
        // scores NaN on the named metric, search stays unsatisfied but
        // bounded and still returns a real trial.
        let spec = TargetSpec {
            metric: MetricKind::Ssimulacra2,
            value: 80.0,
            options: TargetOptions {
                max_encodes: 2,
                qp_start: Some(30),
                tolerance: 0.1,
                ..Default::default()
            },
        };
        let judge = |_out: &TrialOutput| -> Result<Vec<MetricScore>, String> {
            Ok(vec![(MetricKind::Zensim, 90.0)])
        };
        let out =
            encode_to_target_8bit(&y, &u, &v, 128, 128, 10, &spec, judge, |_| {}).expect("encode");
        assert!(!out.search.converged);
        assert!(out.search.score.is_nan());
    }
}
