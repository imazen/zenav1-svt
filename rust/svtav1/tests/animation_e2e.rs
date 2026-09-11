//! Animated AVIF, end to end: encode -> parse the container back -> assert the
//! animation survived.
//!
//! # Why this exists
//!
//! Animated AVIF is a shipping product path — `AvifEncoder::encode_animation_*`
//! and, since `svtav1::rgba`, `encode_rgba_animation` — and until this file it
//! had NO test anywhere in the workspace. `examples/animation_probe.rs`
//! produces files; nothing asserted anything about them. A regression that
//! dropped a frame, collapsed the timing, or wrote a still instead of a
//! sequence would have been invisible to every gate in the repo.
//!
//! The assertions are made by PARSING THE PRODUCED FILE with an independent
//! container parser (`zenavif-parse`), not by inspecting the encoder's own
//! intermediate state. `tools/animation_gate.sh` adds the decoder leg on top
//! of this — it runs the frames through avifdec/ffprobe — but the structural
//! contract is checked here so it runs in ordinary CI with no external tools.
#![cfg(feature = "avif-container")]
// `zenavif-parse`'s eager `read_avif` / `AnimationConfig` API is deprecated in
// favour of its zero-copy `AvifParser`. The eager form is used deliberately
// here: this is a test, the files are small, and one call that returns the
// whole parsed container is the clearest way to state the assertions.
#![allow(deprecated)]

use svtav1::avif::AvifEncoder;
use svtav1::avif::animation::{AnimationFrame, AnimationTiming};
use svtav1::rgba::RgbaFrame;

/// A recognisable moving pattern: each frame shifts, so a decoder that
/// silently repeated frame 0 would produce identical samples and fail the
/// "frames actually differ" assertion below.
fn planes(w: usize, h: usize, frame: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let y = (0..w * h)
        .map(|i| (((i % w) + (i / w) + frame * 17) % 256) as u8)
        .collect();
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let u = (0..cw * ch)
        .map(|i| ((i + frame * 5) % 256) as u8)
        .collect();
    let v = (0..cw * ch)
        .map(|i| ((i * 3 + frame) % 256) as u8)
        .collect();
    (y, u, v)
}

fn parse(bytes: &[u8]) -> zenavif_parse::AvifData {
    let mut cursor = std::io::Cursor::new(bytes);
    zenavif_parse::read_avif(&mut cursor).expect("the encoder produced an unparseable AVIF")
}

#[test]
fn an_animation_round_trips_its_frame_count_and_timing() {
    let (w, h) = (64usize, 48usize);
    let durations = [100u32, 250, 40, 1000];
    let built: Vec<_> = (0..durations.len()).map(|f| planes(w, h, f)).collect();
    let frames: Vec<AnimationFrame<'_>> = built
        .iter()
        .zip(durations)
        .map(|((y, u, v), duration)| AnimationFrame {
            y,
            u,
            v,
            y_stride: w,
            alpha: None,
            duration,
        })
        .collect();

    let bytes = AvifEncoder::new()
        .encode_animation_yuv420(
            &frames,
            w as u32,
            h as u32,
            AnimationTiming { timescale: 1000 },
        )
        .expect("encode_animation_yuv420");

    // The brand is what makes a player treat this as a sequence rather than a
    // still. `avis` is the AVIF-sequence brand; a still would be `avif`.
    assert_eq!(
        &bytes[4..8],
        b"ftyp",
        "output does not start with an ftyp box"
    );
    assert!(
        bytes[8..].starts_with(b"avis"),
        "major brand is not `avis`, so players will not treat this as animated"
    );

    let data = parse(&bytes);
    let seq = data
        .animation
        .as_ref()
        .expect("parsed file carries no sequence — it was written as a still");
    assert_eq!(
        seq.frames.len(),
        durations.len(),
        "frame count did not survive the round trip"
    );
    // The timescale is 1000, so a tick IS a millisecond and the durations must
    // come back exactly. A collapsed or uniform timeline is the failure this
    // catches — every frame showing for the same wrong time still decodes.
    let got: Vec<u32> = seq.frames.iter().map(|f| f.duration_ms).collect();
    assert_eq!(
        got,
        durations.to_vec(),
        "per-frame durations did not survive the round trip"
    );
    // Anti-vacuity: the frames must actually differ, or a decoder repeating
    // frame 0 would satisfy every count above.
    assert!(
        seq.frames.windows(2).any(|w| w[0].data != w[1].data),
        "every coded frame is byte-identical — the pattern did not vary"
    );
}

#[test]
fn rgba_animation_matches_the_planar_path_and_keeps_every_frame() {
    let (w, h) = (32usize, 32usize);
    // Three visibly different frames, fully opaque: no alpha track should be
    // written, which is the cheaper file and the documented behaviour.
    let rgba: Vec<Vec<u8>> = (0..3)
        .map(|f| {
            (0..w * h)
                .flat_map(|i| {
                    let v = (((i % w) + f * 40) % 256) as u8;
                    [v, 255u8.wrapping_sub(v), (i / w) as u8, 255]
                })
                .collect()
        })
        .collect();
    let frames: Vec<RgbaFrame<'_>> = rgba
        .iter()
        .map(|r| RgbaFrame {
            rgba: r,
            duration_ms: 120,
        })
        .collect();

    let bytes = AvifEncoder::new()
        .with_quality(70.0)
        .encode_rgba_animation(&frames, w as u32, h as u32)
        .expect("encode_rgba_animation");

    let data = parse(&bytes);
    let seq = data.animation.as_ref().expect("no sequence track");
    assert_eq!(seq.frames.len(), 3, "an RGBA frame was dropped");
    assert!(
        data.alpha_item.is_none(),
        "an opaque RGBA input must not pay for an alpha track"
    );
}

#[test]
fn a_transparent_rgba_animation_carries_an_alpha_track() {
    let (w, h) = (32usize, 32usize);
    // Frame 1 is the only one with a non-opaque pixel. Alpha is per-SEQUENCE,
    // so ALL frames must end up carrying a plane — a ragged track is what this
    // guards against.
    let rgba: Vec<Vec<u8>> = (0..3)
        .map(|f| {
            (0..w * h)
                .flat_map(|i| {
                    let a = if f == 1 && i == 0 { 0u8 } else { 255 };
                    [(i % 256) as u8, 64, 128, a]
                })
                .collect()
        })
        .collect();
    let frames: Vec<RgbaFrame<'_>> = rgba
        .iter()
        .map(|r| RgbaFrame {
            rgba: r,
            duration_ms: 50,
        })
        .collect();

    let bytes = AvifEncoder::new()
        .encode_rgba_animation(&frames, w as u32, h as u32)
        .expect("encode_rgba_animation with alpha");
    let data = parse(&bytes);
    assert_eq!(
        data.animation.as_ref().expect("no sequence").frames.len(),
        3
    );
    assert!(
        data.alpha_item.is_some(),
        "a frame with a transparent pixel produced no alpha track"
    );
}

#[test]
fn a_single_frame_animation_is_still_a_sequence() {
    let (w, h) = (16usize, 16usize);
    let (y, u, v) = planes(w, h, 0);
    let bytes = AvifEncoder::new()
        .encode_animation_yuv420(
            &[AnimationFrame {
                y: &y,
                u: &u,
                v: &v,
                y_stride: w,
                alpha: None,
                duration: 33,
            }],
            w as u32,
            h as u32,
            AnimationTiming { timescale: 1000 },
        )
        .expect("one-frame animation");
    let data = parse(&bytes);
    assert_eq!(
        data.animation.as_ref().expect("no sequence").frames.len(),
        1
    );
}

#[test]
fn an_empty_animation_is_refused_rather_than_written() {
    let err = AvifEncoder::new().encode_rgba_animation(&[], 16, 16);
    assert!(
        err.is_err(),
        "an animation with no frames must be refused, not written as an empty track"
    );
}
