//! Convert the video track of an MP4 to an animated AVIF.
//!
//! ```text
//! cargo run --release --features avif-container --example mp4_to_avif -- in.mp4 out.avif [quality]
//! ```
//!
//! The whole chain is pure Rust:
//!
//! | step | crate | note |
//! |---|---|---|
//! | demux | `re_mp4` | boxes, sample table, `avcC` |
//! | decode | `rusty_h264` | `forbid(unsafe_code)` core, bit-exact vs Cisco's `h264dec` |
//! | encode + mux | this crate | |
//!
//! H.264 decodes to tightly packed I420 planes and this encoder codes tightly
//! packed I420 planes, so frames go straight across — no RGB round trip, and
//! no colour conversion to get wrong.
//!
//! MEASURED on an x264 `-preset medium -crf 23` MP4 of the public-domain Derf
//! clip `fourpeople` at 256x256, 8 frames (High profile, CABAC, 2 B-frames),
//! at quality 70: 8,674 B in, 17,985 B out, and the AVIF scores 50.7 dB luma
//! PSNR against the MP4's own decode. The same file was 58,823 B at 42.5 dB
//! before the animation path coded inter frames — smaller AND closer to the
//! source, because a near-static inter frame carries a small residual instead
//! of re-quantizing the whole picture.
//!
//! Only H.264 (`avc1`) video tracks are handled; anything else is reported by
//! codec name rather than guessed at.

use svtav1::avif::AvifEncoder;
use svtav1::avif::animation::{AnimationFrame, AnimationTiming};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let (Some(input), Some(output)) = (args.next(), args.next()) else {
        eprintln!("usage: mp4_to_avif <in.mp4> <out.avif> [quality 1..100, default 70]");
        std::process::exit(2);
    };
    let quality: f32 = args.next().map_or(Ok(70.0), |q| q.parse())?;

    // ---- demux ----
    let bytes = std::fs::read(&input)?;
    let mp4 = re_mp4::Mp4::read_bytes(&bytes)?;
    let track = mp4
        .tracks()
        .values()
        .find(|t| t.kind == Some(re_mp4::TrackKind::Video))
        .ok_or("no video track in this MP4")?;

    let codec = track.codec_string(&mp4).unwrap_or_default();
    if !codec.starts_with("avc1") {
        return Err(format!(
            "this example decodes H.264 (avc1) only; that track is `{codec}`. \
             Demux and encode are codec-agnostic — what a different codec needs \
             is a pure-Rust decoder for it."
        )
        .into());
    }
    let avcc = track
        .raw_codec_config(&mp4)
        .ok_or("the H.264 track carries no avcC configuration")?;
    let (nal_len_size, params) = parse_avcc(&avcc)?;

    // ---- AVCC -> Annex-B ----
    // MP4 stores each NAL behind a length prefix and keeps SPS/PPS out of band
    // in `avcC`; a decoder wants start codes with the parameter sets inline.
    let mut annexb = params;
    for s in &track.samples {
        let start = usize::try_from(s.offset)?;
        let end = start + usize::try_from(s.size)?;
        let sample = bytes.get(start..end).ok_or("sample range outside the file")?;
        let mut p = 0usize;
        while p + nal_len_size <= sample.len() {
            let mut len = 0usize;
            for _ in 0..nal_len_size {
                len = (len << 8) | usize::from(sample[p]);
                p += 1;
            }
            let nal = sample.get(p..p + len).ok_or("NAL length runs past the sample")?;
            annexb.extend_from_slice(&[0, 0, 0, 1]);
            annexb.extend_from_slice(nal);
            p += len;
        }
    }

    // Frame durations come from *composition* (display) timestamps, sorted:
    // `decode_stream` hands frames back in display order, and with B-frames
    // that is not the sample-table order. The last frame has no successor to
    // subtract from, so it reuses the previous gap — what a player does when
    // the track simply ends.
    let mut pts: Vec<i64> = track.samples.iter().map(|s| s.composition_timestamp).collect();
    pts.sort_unstable();
    let durations: Vec<i64> = (0..pts.len())
        .map(|i| {
            if i + 1 < pts.len() {
                pts[i + 1] - pts[i]
            } else {
                pts.len().checked_sub(2).map_or(1, |j| pts[i] - pts[j])
            }
        })
        .collect();

    // ---- decode ----
    let frames = rusty_h264::Decoder::new()
        .decode_stream(&annexb)
        .map_err(|e| format!("H.264 decode failed: {e:?}"))?;
    let Some(first) = frames.first() else {
        return Err("the video track decoded to zero frames".into());
    };
    if frames.len() != durations.len() {
        return Err(format!(
            "decoded {} frames but the sample table lists {}",
            frames.len(),
            durations.len()
        )
        .into());
    }
    let (w, h) = (first.width, first.height);
    if w % 2 != 0 || h % 2 != 0 {
        // 4:2:0 planes are width/2 by height/2 here, and the AVIF side wants
        // ceil(w/2) by ceil(h/2). Those agree only on even dimensions, and an
        // H.264 4:2:0 crop is always even, so this is a corrupt-file guard
        // rather than a limitation — refuse instead of coding short planes.
        return Err(format!("odd frame dimensions {w}x{h} in a 4:2:0 track").into());
    }
    if frames.iter().any(|f| f.width != w || f.height != h) {
        return Err("the track changes resolution mid-stream".into());
    }

    // ---- encode ----
    // Carry the track's own timescale rather than converting to milliseconds:
    // the durations are already exact in it.
    let timescale = u32::try_from(track.timescale)?;
    let anim: Vec<AnimationFrame<'_>> = frames
        .iter()
        .zip(&durations)
        .map(|(f, &d)| AnimationFrame {
            y: &f.y,
            u: &f.u,
            v: &f.v,
            y_stride: f.width,
            alpha: None,
            duration: u32::try_from(d).unwrap_or(1).max(1),
        })
        .collect();

    let avif = AvifEncoder::new()
        .with_quality(quality)
        .encode_animation_yuv420(&anim, w as u32, h as u32, AnimationTiming { timescale })
        .map_err(|e| format!("AVIF encode failed: {e:?}"))?;
    std::fs::write(&output, &avif)?;

    println!(
        "{input} -> {output}: {w}x{h}, {} frames, {} B -> {} B ({:.1}%)",
        frames.len(),
        bytes.len(),
        avif.len(),
        100.0 * avif.len() as f64 / bytes.len() as f64,
    );
    Ok(())
}

/// Split an `avcC` record into `(nal_length_size, annex-b SPS/PPS)`.
///
/// ISO/IEC 14496-15 §5.3.3.1: a one-byte version, three bytes of profile /
/// compatibility / level, `lengthSizeMinusOne` in the low two bits of the next
/// byte, then the SPS count in the low five bits of the byte after that, each
/// SPS as a 16-bit length and its bytes, then a full-byte PPS count and the
/// same shape again.
fn parse_avcc(avcc: &[u8]) -> Result<(usize, Vec<u8>), Box<dyn std::error::Error>> {
    if avcc.len() < 7 {
        return Err("avcC record is too short".into());
    }
    let nal_len_size = usize::from(avcc[4] & 0x03) + 1;
    let mut out = Vec::new();
    let mut p = 5usize;
    for round in 0..2 {
        let byte = *avcc.get(p).ok_or("avcC ended before a parameter-set count")?;
        let count = usize::from(if round == 0 { byte & 0x1f } else { byte });
        p += 1;
        for _ in 0..count {
            let hi = *avcc.get(p).ok_or("avcC ended mid parameter-set length")?;
            let lo = *avcc.get(p + 1).ok_or("avcC ended mid parameter-set length")?;
            let len = usize::from(u16::from_be_bytes([hi, lo]));
            p += 2;
            let nal = avcc
                .get(p..p + len)
                .ok_or("avcC parameter set runs past the record")?;
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(nal);
            p += len;
        }
    }
    Ok((nal_len_size, out))
}
