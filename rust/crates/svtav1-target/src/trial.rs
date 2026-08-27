//! Encode-to-target: the codec-owned trial cell around
//! [`crate::search::search_target_qp`].
//!
//! The metric judge is INJECTED (`judge` closure over the decoder-exact
//! reconstruction) — see the wave md's chunk-2 final shape: the crate owns
//! encoding + recon + search; the harness owns judge wiring, so a judge
//! swap (route a → b) never touches this crate.

use zenav1_svt::encoder::pipeline::EncodePipeline;
use zenav1_svt::encoder::rate_control::{RcConfig, RcMode};

use crate::search::{TargetOptions, TargetSearchResult, search_target_qp};

/// The decoder-exact 10-bit reconstruction of one trial, plus its stream.
pub struct TrialOutput {
    /// Post-filter 10-bit recon `(Y, U, V)` at ALIGNED strides (luma
    /// `aligned_w`, chroma `aligned_w/2`) — `EncodePipeline::
    /// last_recon10_final`, "what a conforming decoder outputs".
    pub recon10: (Vec<u16>, Vec<u16>, Vec<u16>),
    /// Aligned luma stride (chroma stride is half).
    pub aligned_w: usize,
    pub bytes: Vec<u8>,
    pub qp: u8,
}

/// One encode-to-target run over a bd10 4:2:0 source.
///
/// `judge` scores a [`TrialOutput`] against whatever reference/domain the
/// harness chose; its scale defines the target's scale. Every trial's
/// recon is produced with `with_recon_output(true)`; a missing bd10 recon
/// is a HARD error (the frozen instrument's renditions are all 64-aligned
/// — see the wave md — so `None` means a real defect, never a fallback).
#[allow(clippy::too_many_arguments)]
pub fn encode_to_target<J, E>(
    y: &[u16],
    u: &[u16],
    v: &[u16],
    width: usize,
    height: usize,
    preset: u8,
    target: f64,
    options: &TargetOptions,
    mut judge: J,
) -> Result<(TargetSearchResult, TrialOutput), TargetError<E>>
where
    J: FnMut(&TrialOutput) -> Result<f64, E>,
{
    let mut best_out: Option<TrialOutput> = None;
    let result = search_target_qp(target, options, |qp| -> Result<f64, TargetError<E>> {
        let rc = RcConfig {
            mode: RcMode::Cqp,
            qp,
            ..RcConfig::default()
        };
        let mut pipe = EncodePipeline::new(width as u32, height as u32, preset, rc, 0, 1)
            .with_bit_depth(10)
            .with_chroma_420(true)
            .with_tile_rows_log2(0)
            .with_tile_cols_log2(0)
            .with_sb_size(None)
            .with_recon_output(true);
        let bytes = pipe
            .try_encode_frame_420_hbd(y, u, v, width)
            .map_err(|e| TargetError::Encode(format!("{e:?}")))?;
        let recon10 = pipe
            .last_recon10_final
            .take()
            .ok_or(TargetError::ReconMissing { qp })?;
        let out = TrialOutput {
            recon10,
            aligned_w: width.next_multiple_of(64),
            bytes,
            qp,
        };
        let score = judge(&out).map_err(TargetError::Judge)?;
        let better = best_out
            .as_ref()
            .is_none_or(|b| (score - target).abs() < f64::INFINITY && b.qp != qp)
            || best_out.is_none();
        if better {
            best_out = Some(out);
        }
        Ok(score)
    })?;
    // Re-encode at the winning qp if the retained output is not the winner
    // (the search may have tried a later, worse qp after the best one).
    let out = match best_out {
        Some(o) if o.qp == result.qp => o,
        _ => {
            let rc = RcConfig {
                mode: RcMode::Cqp,
                qp: result.qp,
                ..RcConfig::default()
            };
            let mut pipe = EncodePipeline::new(width as u32, height as u32, preset, rc, 0, 1)
                .with_bit_depth(10)
                .with_chroma_420(true)
                .with_tile_rows_log2(0)
                .with_tile_cols_log2(0)
                .with_sb_size(None)
                .with_recon_output(true);
            let bytes = pipe
                .try_encode_frame_420_hbd(y, u, v, width)
                .map_err(|e| TargetError::Encode(format!("{e:?}")))?;
            let recon10 = pipe
                .last_recon10_final
                .take()
                .ok_or(TargetError::ReconMissing { qp: result.qp })?;
            TrialOutput {
                recon10,
                aligned_w: width.next_multiple_of(64),
                bytes,
                qp: result.qp,
            }
        }
    };
    Ok((result, out))
}

/// Errors from [`encode_to_target`].
#[derive(Debug)]
pub enum TargetError<E> {
    Encode(String),
    /// `last_recon10_final` was `None` — outside the bd10 recon envelope.
    /// LOUD by design (the instrument's renditions are all 64-aligned).
    ReconMissing { qp: u8 },
    Judge(E),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::TargetOptions;

    /// 128x128 bd10 4:2:0 synthetic: smooth gradient + texture, real
    /// RD-tradeoff work at tiny cost.
    fn synth_source() -> (Vec<u16>, Vec<u16>, Vec<u16>, usize, usize) {
        let (w, h) = (128usize, 128usize);
        let mut y = vec![0u16; w * h];
        for r in 0..h {
            for c in 0..w {
                let g = 200.0 + 500.0 * (c as f32 / w as f32)
                    + 90.0 * ((c as f32 * 0.35).sin() * (r as f32 * 0.27).cos());
                y[r * w + c] = g.clamp(64.0, 940.0) as u16;
            }
        }
        let (cw, ch) = (w / 2, h / 2);
        let u = vec![512u16; cw * ch];
        let v = vec![480u16; cw * ch];
        (y, u, v, w, h)
    }

    /// Judge: 100 − mean|Δ| in code-value units on luma — monotone in
    /// quality, cheap, and exercises the recon plumbing for real.
    fn code_mad_judge(src_y: &[u16], w: usize, h: usize) -> impl FnMut(&TrialOutput) -> Result<f64, String> + '_ {
        move |out: &TrialOutput| {
            let ry = &out.recon10.0;
            let aw = out.aligned_w;
            let mut mad = 0f64;
            for r in 0..h {
                for c in 0..w {
                    mad += (f64::from(src_y[r * w + c]) - f64::from(ry[r * aw + c])).abs();
                }
            }
            mad /= (w * h) as f64;
            Ok(100.0 - mad)
        }
    }

    #[test]
    fn encode_to_target_end_to_end_on_synth() {
        let (y, u, v, w, h) = synth_source();
        let judge = code_mad_judge(&y, w, h);
        let (res, out) = encode_to_target(
            &y, &u, &v, w, h, 8, /* fast preset */
            97.0,
            &TargetOptions {
                tolerance: 0.0,
                max_encodes: 3,
                ..Default::default()
            },
            judge,
        )
        .expect("end-to-end");
        assert_eq!(res.encodes_used, 3);
        assert!(!out.bytes.is_empty());
        assert_eq!(out.qp, res.qp, "emitted output matches the winning qp");
        assert!(res.score > 50.0, "sane judge range: {res:?}");
    }
}
