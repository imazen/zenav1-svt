//! Animated AVIF encoding. Each submitted image currently becomes a sync
//! sample; inter-picture compression is tracked separately from this path.
use super::{AvifEncoder, EncodeError};
use zenavif_serialize::{
    Av1CBox,
    animated::{AnimFrame, AnimatedImage},
};

pub use zenavif_serialize::{ClliBox, MdcvBox, animated::RepetitionCount};

/// Container metadata and playback policy. Metadata applies to the color
/// track and its poster item. ICC bytes take display-color precedence over
/// CICP; CICP still describes the encoded YUV samples.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct AnimationOptions {
    pub repetition: RepetitionCount,
    pub icc: Option<Vec<u8>>,
    /// Raw TIFF bytes or an already-framed HEIF Exif item.
    pub exif: Option<Vec<u8>>,
    pub xmp: Option<Vec<u8>>,
    pub clli: Option<ClliBox>,
    pub mdcv: Option<MdcvBox>,
    /// The input color planes have already been premultiplied by alpha.
    pub premultiplied_alpha: bool,
}

impl Default for AnimationOptions {
    fn default() -> Self {
        Self {
            repetition: RepetitionCount::Infinite,
            icc: None,
            exif: None,
            xmp: None,
            clli: None,
            mdcv: None,
            premultiplied_alpha: false,
        }
    }
}

/// One planar 4:2:0 image with an optional full-resolution alpha plane.
/// Use `u8` for 8-bit samples or `u16` for native 10-bit samples.
/// Chroma and alpha are tightly packed; luma may have a larger stride.
pub struct AnimationFrame<'a, T = u8> {
    pub y: &'a [T],
    pub u: &'a [T],
    pub v: &'a [T],
    pub y_stride: usize,
    pub alpha: Option<&'a [T]>,
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
        self.encode_animation_yuv420_with_options(
            frames,
            width,
            height,
            timing,
            &AnimationOptions::default(),
        )
    }

    /// Encode timed color and alpha samples with metadata and repetition.
    pub fn encode_animation_yuv420_with_options(
        &self,
        frames: &[AnimationFrame<'_>],
        width: u32,
        height: u32,
        timing: AnimationTiming,
        options: &AnimationOptions,
    ) -> Result<Vec<u8>, EncodeError> {
        self.encode_animation_samples(frames, width, height, timing, options)
    }

    /// Encode native 10-bit color and alpha samples. Configure bit depth 10
    /// on the encoder. Samples are right-aligned values in `0..=1023`.
    pub fn encode_animation_yuv420_hbd(
        &self,
        frames: &[AnimationFrame<'_, u16>],
        width: u32,
        height: u32,
        timing: AnimationTiming,
    ) -> Result<Vec<u8>, EncodeError> {
        self.encode_animation_yuv420_hbd_with_options(
            frames,
            width,
            height,
            timing,
            &AnimationOptions::default(),
        )
    }

    /// Encode native 10-bit animation with container metadata and repetition.
    /// The native pipeline currently requires 64-aligned dimensions; native
    /// alpha also requires preset 9 or higher.
    pub fn encode_animation_yuv420_hbd_with_options(
        &self,
        frames: &[AnimationFrame<'_, u16>],
        width: u32,
        height: u32,
        timing: AnimationTiming,
        options: &AnimationOptions,
    ) -> Result<Vec<u8>, EncodeError> {
        self.encode_animation_samples(frames, width, height, timing, options)
    }

    fn encode_animation_samples<T: AnimationSample>(
        &self,
        frames: &[AnimationFrame<'_, T>],
        width: u32,
        height: u32,
        timing: AnimationTiming,
        options: &AnimationOptions,
    ) -> Result<Vec<u8>, EncodeError> {
        self.validate_quality()?;
        self.validate_inert_knobs(true)?;
        if self.bit_depth != T::BIT_DEPTH {
            return Err(EncodeError::UnsupportedConfig(
                "animation input sample type must match the configured bit depth",
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
        let duration = frames
            .iter()
            .try_fold(0u64, |sum, frame| {
                sum.checked_add(u64::from(frame.duration))
            })
            .ok_or(EncodeError::UnsupportedConfig(
                "animation duration overflow",
            ))?;
        if let RepetitionCount::Finite(count) = options.repetition {
            duration
                .checked_mul(u64::from(count) + 1)
                .filter(|&value| value != u64::MAX)
                .ok_or(EncodeError::UnsupportedConfig(
                    "repeated animation duration overflow",
                ))?;
        }
        let has_alpha = frames[0].alpha.is_some();
        if options.premultiplied_alpha && !has_alpha {
            return Err(EncodeError::UnsupportedConfig(
                "premultiplied animation requires alpha",
            ));
        }
        for bytes in [&options.icc, &options.exif, &options.xmp]
            .into_iter()
            .flatten()
        {
            if bytes.is_empty() {
                return Err(EncodeError::EncodeFailed(
                    "metadata payload must not be empty".into(),
                ));
            }
        }

        let n = width as usize * height as usize;
        let cn = (width as usize).div_ceil(2) * (height as usize).div_ceil(2);
        // Validate the entire submission before encoding any picture.
        for frame in frames {
            if frame
                .y
                .iter()
                .chain(frame.u)
                .chain(frame.v)
                .chain(frame.alpha.unwrap_or(&[]))
                .any(|&sample| !T::in_range(sample))
            {
                return Err(EncodeError::UnsupportedConfig(
                    "animation sample exceeds the configured bit depth",
                ));
            }
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
            colors.push(T::encode_color(&mut color, frame)?);
            if let Some(a) = frame.alpha {
                alphas.push(T::encode_alpha(&mut alpha, a, width as usize)?);
            }
        }
        let color_seq = sequence_header(&colors[0])?;
        let alpha_seq = if has_alpha {
            Some(sequence_header(&alphas[0])?)
        } else {
            None
        };
        let mut cfg = Av1CBox::default();
        cfg.high_bitdepth = T::BIT_DEPTH > 8;
        cfg.seq_level_idx_0 = svtav1_encoder::entropy::obu::compute_seq_level_idx(
            width,
            height,
            color.rc_config.framerate,
        );
        let mut mux = AnimatedImage::new();
        mux.set_timescale(timing.timescale).set_color_config(cfg);
        mux.set_repetition_count(options.repetition)
            .set_color_description(
                u16::from(self.color_primaries),
                u16::from(self.transfer_characteristics),
                u16::from(self.matrix_coefficients),
                self.full_range,
            )
            .set_premultiplied_alpha(options.premultiplied_alpha);
        if let Some(icc) = options.icc.as_ref() {
            mux.set_icc_profile(icc.clone());
        }
        if let Some(exif) = options.exif.as_ref() {
            mux.set_exif(exif.clone());
        }
        if let Some(xmp) = options.xmp.as_ref() {
            mux.set_xmp(xmp.clone());
        }
        if let Some(clli) = options.clli {
            mux.set_clli(clli);
        }
        if let Some(mdcv) = options.mdcv {
            mux.set_mdcv(mdcv);
        }

        if has_alpha {
            let mut cfg = Av1CBox::default();
            cfg.high_bitdepth = T::BIT_DEPTH > 8;
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
        mux.try_serialize(width, height, &samples, &color_seq, alpha_seq.as_deref())
            .map_err(|e| EncodeError::EncodeFailed(e.to_string()))
    }
}

// These are trusted encoder-produced OBUs. Still check their bounds so a
// future writer change cannot turn a missing header into malformed av1C data.
trait AnimationSample: Copy {
    const BIT_DEPTH: u8;
    fn in_range(value: Self) -> bool;
    fn encode_color(
        pipe: &mut svtav1_encoder::pipeline::EncodePipeline,
        frame: &AnimationFrame<'_, Self>,
    ) -> Result<Vec<u8>, EncodeError>;
    fn encode_alpha(
        pipe: &mut svtav1_encoder::pipeline::EncodePipeline,
        alpha: &[Self],
        stride: usize,
    ) -> Result<Vec<u8>, EncodeError>;
}

macro_rules! animation_sample {
    ($sample:ty, $depth:expr, $color:ident, $alpha:ident) => {
        impl AnimationSample for $sample {
            const BIT_DEPTH: u8 = $depth;
            fn in_range(value: Self) -> bool {
                u32::from(value) < (1u32 << $depth)
            }
            fn encode_color(
                pipe: &mut svtav1_encoder::pipeline::EncodePipeline,
                frame: &AnimationFrame<'_, Self>,
            ) -> Result<Vec<u8>, EncodeError> {
                pipe.$color(frame.y, frame.u, frame.v, frame.y_stride)
                    .map_err(|e| AvifEncoder::from_pipeline_error(e.error(), || e.to_string()))
            }
            fn encode_alpha(
                pipe: &mut svtav1_encoder::pipeline::EncodePipeline,
                alpha: &[Self],
                stride: usize,
            ) -> Result<Vec<u8>, EncodeError> {
                pipe.$alpha(alpha, stride)
                    .map_err(|e| AvifEncoder::from_pipeline_error(e.error(), || e.to_string()))
            }
        }
    };
}
animation_sample!(u8, 8, try_encode_frame_420, try_encode_frame);
animation_sample!(u16, 10, try_encode_frame_420_hbd, try_encode_frame_hbd);

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
    fn native_ten_bit_animation_matches_color_and_alpha_reconstruction() {
        let w = 64usize;
        let stride = w + 5;
        let directory =
            std::env::temp_dir().join(format!("svt-animation-hbd-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        for quality in [40.0, 98.0] {
            let enc = AvifEncoder::new()
                .with_bit_depth(10)
                .with_speed(9)
                .with_quality(quality);
            let colors: Vec<Vec<u16>> = (0..2)
                .map(|f| {
                    (0..stride * w)
                        .map(|i| (100 + (i * 7 + f * 133) % 750) as u16)
                        .collect()
                })
                .collect();
            let alphas: Vec<Vec<u16>> = (0..2)
                .map(|f| {
                    (0..w * w)
                        .map(|i| ((i * 3 + f * 47) % 1024) as u16)
                        .collect()
                })
                .collect();
            let u: Vec<u16> = (0..w * w / 4).map(|i| (480 + i % 64) as u16).collect();
            let v: Vec<u16> = (0..w * w / 4).map(|i| (500 + i % 32) as u16).collect();
            let frames: Vec<_> = (0..2)
                .map(|i| AnimationFrame {
                    y: &colors[i],
                    u: &u,
                    v: &v,
                    y_stride: stride,
                    alpha: Some(alphas[i].as_slice()),
                    duration: [17, 29][i],
                })
                .collect();
            let bytes = enc
                .encode_animation_yuv420_hbd(
                    &frames,
                    w as u32,
                    w as u32,
                    AnimationTiming { timescale: 1000 },
                )
                .unwrap();
            let input = directory.join("input.avif");
            fs::write(&input, bytes).unwrap();
            for extension in ["png", "y4m"] {
                let result = Command::new("avifdec")
                    .args(["-j", "1", "--index", "all"])
                    .arg(&input)
                    .arg(directory.join(format!("frame.{extension}")))
                    .output()
                    .unwrap();
                assert!(
                    result.status.success(),
                    "{}",
                    String::from_utf8_lossy(&result.stderr)
                );
            }
            for i in 0..2 {
                let mut pipe = enc
                    .build_pipeline(w as u32, w as u32)
                    .with_chroma_420(true)
                    .with_recon_output(true);
                pipe.try_encode_frame_420_hbd(&colors[i], &u, &v, stride)
                    .unwrap();
                let (y, u, v) = pipe.last_recon10_final.unwrap();
                let expected: Vec<u8> = y
                    .iter()
                    .chain(&u)
                    .chain(&v)
                    .flat_map(|n| n.to_le_bytes())
                    .collect();
                assert!(
                    y.iter().any(|n| n & 3 != 0),
                    "fixture must preserve native low bits"
                );
                let decoded = fs::read(directory.join(format!("frame-{i:010}.y4m"))).unwrap();
                let offset = decoded.windows(6).position(|p| p == b"FRAME\n").unwrap() + 6;
                assert_eq!(&decoded[offset..], expected, "10-bit color frame {i}");

                let mut settings = enc.clone();
                settings.full_range = true;
                let mut pipe = settings
                    .build_pipeline(w as u32, w as u32)
                    .with_recon_output(true);
                pipe.try_encode_frame_hbd(&alphas[i], w).unwrap();
                let (alpha, _, _) = pipe.last_recon10_final.unwrap();
                assert_eq!(alpha.len(), w * w);
                let file = fs::File::open(directory.join(format!("frame-{i:010}.png"))).unwrap();
                let mut reader = png::Decoder::new(std::io::BufReader::new(file))
                    .read_info()
                    .unwrap();
                let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
                let info = reader.next_frame(&mut pixels).unwrap();
                assert_eq!(info.bit_depth, png::BitDepth::Sixteen);
                assert_eq!(info.color_type, png::ColorType::Rgba);
                for (pixel, &sample) in pixels.chunks_exact(8).zip(&alpha) {
                    let actual = u16::from_be_bytes([pixel[6], pixel[7]]);
                    // PNG expands alpha to 16 bits. Recover the native 10-bit
                    // integer to avoid testing the reader's expansion rounding.
                    let native = ((u32::from(actual) * 1023 + 32767) / 65535) as u16;
                    assert_eq!(native, sample, "10-bit alpha frame {i}");
                }
            }
        }
        fs::remove_dir_all(directory).unwrap();
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
