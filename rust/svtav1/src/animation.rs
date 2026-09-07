//! Animated AVIF encoding. Each submitted image currently becomes a sync
//! sample; inter-picture compression is tracked separately from this path.
use super::{AvifEncoder, EncodeError};
use zenavif_serialize::{
    Av1CBox,
    animated::{AnimFrame, AnimatedImage},
};

/// One planar 8-bit 4:2:0 image with an optional full-resolution alpha plane.
/// Chroma and alpha are tightly packed; luma may have a larger stride.
pub struct AnimationFrame<'a> {
    pub y: &'a [u8],
    pub u: &'a [u8],
    pub v: &'a [u8],
    pub y_stride: usize,
    pub alpha: Option<&'a [u8]>,
    /// Duration in the animation's timescale ticks, strictly positive.
    pub duration: u32,
}

/// Timing of an animated AVIF stream.
pub struct AnimationTiming {
    /// Ticks per second, strictly positive.
    pub timescale: u32,
}

impl AvifEncoder {
    /// Encode an animated AVIF container from planar images. Dimensions and
    /// alpha presence must agree across frames. This initial path accepts
    /// 8-bit input and writes all-intra sequences with full AV1 headers.
    pub fn encode_animation_yuv420(
        &self,
        frames: &[AnimationFrame<'_>],
        width: u32,
        height: u32,
        timing: AnimationTiming,
    ) -> Result<Vec<u8>, EncodeError> {
        self.validate_quality()?;
        self.validate_inert_knobs(true)?;
        if self.bit_depth != 8 {
            return Err(EncodeError::UnsupportedConfig(
                "animation u8 input requires bit depth 8",
            ));
        }
        if frames.is_empty()
            || timing.timescale == 0
            || width == 0
            || height == 0
            || width > 65535
            || height > 65535
        {
            return Err(EncodeError::InvalidDimensions);
        }
        let has_alpha = frames[0].alpha.is_some();
        let n = width as usize * height as usize;
        let cn = (width as usize).div_ceil(2) * (height as usize).div_ceil(2);
        // Validate the entire submission before encoding any picture.
        for frame in frames {
            let stride =
                u32::try_from(frame.y_stride).map_err(|_| EncodeError::InvalidDimensions)?;
            self.validate_dimensions(frame.y.len(), width, height, stride)?;
            if frame.duration == 0
                || frame.u.len() < cn
                || frame.v.len() < cn
                || frame.alpha.is_some() != has_alpha
                || frame.alpha.is_some_and(|a| a.len() < n)
            {
                return Err(EncodeError::InvalidDimensions);
            }
        }
        let mut color = self
            .build_pipeline(width, height)
            .with_chroma_420(true)
            .with_image_sequence();
        // Alpha values are full-range coverage, without color grain.
        let mut alpha_settings = self.clone();
        alpha_settings.film_grain = Default::default();
        alpha_settings.full_range = true;
        let mut alpha = alpha_settings
            .build_pipeline(width, height)
            .with_image_sequence();
        // The level declaration must account for the fastest displayed
        // interval, including variable-duration animations.
        let fps = f64::from(timing.timescale)
            / f64::from(frames.iter().map(|f| f.duration).min().unwrap());
        color.rc_config.framerate = fps;
        alpha.rc_config.framerate = fps;
        let mut colors = Vec::with_capacity(frames.len());
        let mut alphas = Vec::with_capacity(frames.len());
        for frame in frames {
            colors.push(
                color
                    .try_encode_frame_420(frame.y, frame.u, frame.v, frame.y_stride)
                    .map_err(|e| Self::from_pipeline_error(e.error(), || e.to_string()))?,
            );
            if let Some(a) = frame.alpha {
                alphas.push(
                    alpha
                        .try_encode_frame(a, width as usize)
                        .map_err(|e| Self::from_pipeline_error(e.error(), || e.to_string()))?,
                );
            }
        }
        let color_seq = sequence_header(&colors[0])?;
        let alpha_seq = if has_alpha {
            Some(sequence_header(&alphas[0])?)
        } else {
            None
        };
        let mut cfg = Av1CBox::default();
        cfg.seq_level_idx_0 = svtav1_encoder::entropy::obu::compute_seq_level_idx(
            width,
            height,
            color.rc_config.framerate,
        );
        let mut mux = AnimatedImage::new();
        mux.set_timescale(timing.timescale).set_color_config(cfg);
        if has_alpha {
            let mut cfg = Av1CBox::default();
            cfg.monochrome = true;
            cfg.seq_level_idx_0 = svtav1_encoder::entropy::obu::compute_seq_level_idx(
                width,
                height,
                alpha.rc_config.framerate,
            );
            mux.set_alpha_config(cfg);
        }
        // ISOBMFF samples already delimit temporal units; omit the raw-stream
        // delimiter as recommended by AV1-ISOBMFF section 2.4.
        let delimiter = svtav1_encoder::entropy::obu::write_temporal_delimiter();
        let samples: Vec<_> = frames
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let sample = AnimFrame::new(
                    colors[i]
                        .strip_prefix(delimiter.as_slice())
                        .unwrap_or(&colors[i]),
                    f.duration,
                )
                .with_sync(true);
                if has_alpha {
                    sample.with_alpha(
                        alphas[i]
                            .strip_prefix(delimiter.as_slice())
                            .unwrap_or(&alphas[i]),
                    )
                } else {
                    sample
                }
            })
            .collect();
        Ok(mux.serialize(width, height, &samples, &color_seq, alpha_seq.as_deref()))
    }
}

// These are trusted encoder-produced OBUs. Still check their bounds so a
// future writer change cannot turn a missing header into malformed av1C data.
fn sequence_header(data: &[u8]) -> Result<Vec<u8>, EncodeError> {
    let mut pos = 0;
    while pos < data.len() {
        let start = pos;
        let header = data[pos];
        pos += 1;
        if header & 4 != 0 {
            pos += 1;
        }
        if header & 2 == 0 {
            break;
        }
        let mut len = 0usize;
        let mut complete = false;
        for shift in (0..56).step_by(7) {
            let Some(&b) = data.get(pos) else {
                break;
            };
            pos += 1;
            len |= usize::from(b & 127)
                .checked_shl(shift)
                .unwrap_or(usize::MAX);
            if b & 128 == 0 {
                complete = true;
                break;
            }
        }
        let Some(end) = pos
            .checked_add(len)
            .filter(|&v| complete && v <= data.len())
        else {
            break;
        };
        if (header >> 3) & 15 == 1 {
            return Ok(data[start..end].to_vec());
        }
        pos = end;
    }
    Err(EncodeError::EncodeFailed(
        "encoder did not emit a valid sequence header".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, process::Command};

    #[test]
    fn three_frame_alpha_sequence_decodes_with_exact_timing_and_alpha() {
        for (w, h, has_alpha) in [(64usize, 64usize, true), (65, 67, true), (64, 64, false)] {
            let out =
                std::env::temp_dir().join(format!("svt-animation-{}-{w}x{h}", std::process::id()));
            fs::create_dir_all(&out).unwrap();
            let enc = AvifEncoder::new().with_speed(7);
            let colors: Vec<Vec<u8>> = (0..3)
                .map(|f| {
                    (0..w * h)
                        .map(|i| (32 + (i % w + f * 30) % 180) as u8)
                        .collect()
                })
                .collect();
            let alphas: Vec<Vec<u8>> = (0..3)
                .map(|f| (0..w * h).map(|i| ((i + f * 13) % 256) as u8).collect())
                .collect();
            let uv = vec![128; w.div_ceil(2) * h.div_ceil(2)];
            let frames: Vec<_> = (0..3)
                .map(|f| AnimationFrame {
                    y: &colors[f],
                    u: &uv,
                    v: &uv,
                    y_stride: w,
                    alpha: has_alpha.then_some(alphas[f].as_slice()),
                    duration: [100, 200, 300][f],
                })
                .collect();
            let avif = enc
                .encode_animation_yuv420(
                    &frames,
                    w as u32,
                    h as u32,
                    AnimationTiming { timescale: 1000 },
                )
                .unwrap();
            fs::write(out.join("sequence.avif"), avif).unwrap();
            let result = Command::new("avifdec")
                .args(["-j", "1", "--index", "all"])
                .arg(out.join("sequence.avif"))
                .arg(out.join("frame.png"))
                .output()
                .expect("avifdec required for animated AVIF conformance");
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            let info = String::from_utf8_lossy(&result.stdout);
            assert!(info.contains("3 frames"), "{info}");
            assert!(info.contains("600 timescales"), "{info}");
            for duration in [100, 200, 300] {
                assert!(info.contains(&format!("({duration} timescales)")), "{info}");
            }
            let mut alpha_settings = enc.clone();
            alpha_settings.full_range = true;
            for (i, alpha) in alphas.iter().enumerate() {
                if !has_alpha {
                    continue;
                }
                let mut pipe = alpha_settings
                    .build_pipeline(w as u32, h as u32)
                    .with_recon_output(true);
                pipe.try_encode_frame(alpha, w).unwrap();
                let (reference, _, _) = pipe.last_recon.unwrap();
                let file = fs::File::open(out.join(format!("frame-{i:010}.png"))).unwrap();
                let mut reader = png::Decoder::new(std::io::BufReader::new(file))
                    .read_info()
                    .unwrap();
                let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
                let image = reader.next_frame(&mut pixels).unwrap();
                assert_eq!(image.color_type, png::ColorType::Rgba);
                assert_eq!((image.width, image.height), (w as u32, h as u32));
                for y in 0..h {
                    for x in 0..w {
                        assert_eq!(
                            pixels[(y * w + x) * 4 + 3],
                            reference[y * pipe.width as usize + x],
                            "frame {i} alpha ({x},{y})"
                        );
                    }
                }
            }
            let result = Command::new("avifdec")
                .args(["-j", "1", "--index", "all"])
                .arg(out.join("sequence.avif"))
                .arg(out.join("frame.y4m"))
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            for (i, source) in colors.iter().enumerate() {
                let mut pipe = enc
                    .build_pipeline(w as u32, h as u32)
                    .with_chroma_420(true)
                    .with_recon_output(true);
                pipe.try_encode_frame_420(source, &uv, &uv, w).unwrap();
                let (y, u, v) = pipe.last_recon.unwrap();
                let mut expected = Vec::new();
                for (plane, stride, width, height) in [
                    (&y, pipe.width as usize, w, h),
                    (&u, pipe.width as usize / 2, w.div_ceil(2), h.div_ceil(2)),
                    (&v, pipe.width as usize / 2, w.div_ceil(2), h.div_ceil(2)),
                ] {
                    for row in 0..height {
                        expected.extend_from_slice(&plane[row * stride..row * stride + width]);
                    }
                }
                let decoded = fs::read(out.join(format!("frame-{i:010}.y4m"))).unwrap();
                let start = decoded.windows(6).position(|s| s == b"FRAME\n").unwrap() + 6;
                assert_eq!(
                    &decoded[start..],
                    expected,
                    "frame {i} decoded color planes"
                );
            }
            fs::remove_dir_all(out).unwrap();
        }
    }

    #[test]
    fn validates_whole_animation_before_encoding() {
        let enc = AvifEncoder::new();
        assert!(
            enc.encode_animation_yuv420(&[], 64, 64, AnimationTiming { timescale: 1000 })
                .is_err()
        );
        let p = vec![128; 64 * 64];
        let frame = AnimationFrame {
            y: &p,
            u: &p,
            v: &p,
            y_stride: 64,
            alpha: None,
            duration: 0,
        };
        assert!(
            enc.encode_animation_yuv420(&[frame], 64, 64, AnimationTiming { timescale: 1000 })
                .is_err()
        );
    }
}
