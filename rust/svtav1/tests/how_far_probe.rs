//! Exercise how-far as a library author would at the public raw-encoder seam.
//! Counts are actual completed frames; the encoder itself is not instrumented.
//! `EncodePipeline::with_stop` owns a `'static` token, so a borrowed pulse in
//! this probe can cancel between frames but cannot enter the inner hot loop.

use how_far::{NoPulse, PhaseSpec, ProgressExt, Pulse, RunError, Steps, Total};
use how_far_along::{Observer, Outcome, Phase, PulseTree, Status};
use svtav1::pipeline::{EncodePipeline, RcConfig, RcMode};

#[derive(Debug)]
enum ProbeError {
    Stopped(how_far::StopReason),
    Encode(whereat::At<svtav1::pipeline::EncodeError>),
}

impl core::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Stopped(reason) => write!(f, "stopped: {reason}"),
            Self::Encode(error) => error.fmt(f),
        }
    }
}

#[derive(Clone, Copy)]
enum Stage {
    Frames,
    Flush,
}

impl Stage {
    const fn label(self) -> &'static str {
        match self {
            Self::Frames => "encode frames",
            Self::Flush => "flush",
        }
    }
}

fn encode_two_frames(
    pulse: &dyn Pulse,
    invalid_second_frame: bool,
) -> Result<Vec<u8>, RunError<ProbeError>> {
    let rc = RcConfig {
        mode: RcMode::Cqp,
        qp: 40,
        ..RcConfig::default()
    };
    let mut encoder = EncodePipeline::new(64, 64, 8, rc, 0, 64).with_chroma_420(true);
    let mut stages = Steps::new(
        pulse,
        &[
            PhaseSpec::new(Stage::Frames.label(), 9, Total::Exact(2)).units("frames"),
            PhaseSpec::new(Stage::Flush.label(), 1, Total::Exact(1)),
        ],
    )?;
    let y = vec![128_u8; 64 * 64];
    let uv = vec![128_u8; 32 * 32];
    let mut output = stages.run_classified(
        |error| matches!(error, ProbeError::Stopped(_)),
        |stage| {
            let mut bytes = Vec::new();
            for index in 0..2 {
                stage.check().map_err(ProbeError::Stopped)?;
                let u = if invalid_second_frame && index == 1 {
                    &[][..]
                } else {
                    uv.as_slice()
                };
                bytes.extend(
                    encoder
                        .try_encode_frame_420(&y, u, &uv, 64)
                        .map_err(ProbeError::Encode)?,
                );
                stage.step(1).map_err(ProbeError::Stopped)?;
            }
            Ok::<_, ProbeError>(bytes)
        },
    )?;
    output.extend(stages.run_classified(
        |error| matches!(error, ProbeError::Stopped(_)),
        |stage| {
            let tail = encoder.try_flush().map_err(ProbeError::Encode)?;
            stage.step(1).map_err(ProbeError::Stopped)?;
            Ok::<_, ProbeError>(tail)
        },
    )?);
    stages.finish()?;
    Ok(output)
}

#[test]
fn typed_stage_labels_and_real_frame_counts_need_no_tracker_in_library_code() {
    let pulse = PulseTree::new(
        Phase::new("video", Total::Unknown),
        &how_far_along::Unstoppable,
    );
    let observer = pulse.observer();
    let with_progress = encode_two_frames(&pulse, false).unwrap();
    let without_progress = encode_two_frames(&NoPulse, false).unwrap();
    assert_eq!(with_progress, without_progress);
    assert!(!with_progress.is_empty());
    let snapshot = observer.snapshot();
    assert_eq!(snapshot.status, Status::Finished(Outcome::Succeeded));
    assert_eq!(snapshot.children[0].name, "encode frames");
    assert_eq!(snapshot.children[0].completed, 2);
    assert_eq!(snapshot.children[0].units, "frames");
    assert_eq!(snapshot.children[1].name, "flush");
    assert_eq!(snapshot.children[1].completed, 1);
}

struct CancelAfterFirstFrame(Observer);

impl how_far::Stop for CancelAfterFirstFrame {
    fn check(&self) -> Result<(), how_far::StopReason> {
        if self.0.snapshot().children[0].completed >= 1 {
            Err(how_far::StopReason::Cancelled)
        } else {
            Ok(())
        }
    }
}

#[test]
fn a_stop_between_frames_preserves_count_and_skips_flush() {
    let phase = Phase::new("video", Total::Unknown);
    let observer = phase.observer();
    let stop = CancelAfterFirstFrame(observer.clone());
    let pulse = PulseTree::new(phase, &stop);
    assert!(matches!(
        encode_two_frames(&pulse, false),
        Err(RunError::Work(ProbeError::Stopped(
            how_far::StopReason::Cancelled
        )))
    ));
    let snapshot = observer.snapshot();
    assert_eq!(snapshot.status, Status::Finished(Outcome::Cancelled));
    assert_eq!(snapshot.children[0].completed, 1);
    assert_eq!(
        snapshot.children[1].status,
        Status::Finished(Outcome::Skipped)
    );
}

#[test]
fn a_bad_second_frame_is_failure_after_one_completed_frame() {
    let pulse = PulseTree::new(
        Phase::new("video", Total::Unknown),
        &how_far_along::Unstoppable,
    );
    let observer = pulse.observer();
    assert!(matches!(
        encode_two_frames(&pulse, true),
        Err(RunError::Work(ProbeError::Encode(_)))
    ));
    let snapshot = observer.snapshot();
    assert_eq!(snapshot.status, Status::Finished(Outcome::Failed));
    assert_eq!(snapshot.children[0].completed, 1);
    assert_eq!(
        snapshot.children[1].status,
        Status::Finished(Outcome::Skipped)
    );
}
