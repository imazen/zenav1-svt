//! Animated AVIF encoding. Each submitted image currently becomes a sync
//! sample; inter-picture compression is tracked separately from this path.
use super::{AvifEncoder, EncodeError};
use zenavif_serialize::{
    Av1CBox,
    animated::{AnimFrame, AnimatedImage},
};

pub use zenavif_serialize::{
    AmveBox, CclvBox, ClliBox, MdcvBox, PaspBox,
    animated::{CropRect, RepetitionCount},
};

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
    pub amve: Option<AmveBox>,
    pub cclv: Option<CclvBox>,
    /// Explicit square-pixel spacing. AVIF requires positive, equal values.
    pub pixel_aspect_ratio: Option<PaspBox>,
    /// Crop in unrotated image coordinates, before rotation and mirroring.
    pub crop: Option<CropRect>,
    /// Counter-clockwise quarter-turn code (0..=3), before mirroring.
    pub rotation: Option<u8>,
    /// Mirror after rotation: 0 swaps top/bottom, 1 swaps left/right.
    pub mirror: Option<u8>,
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
            amve: None,
            cclv: None,
            pixel_aspect_ratio: None,
            crop: None,
            rotation: None,
            mirror: None,
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

/// One grayscale image with optional full-resolution alpha. Use `u8` for
/// 8-bit samples or `u16` for native 10-bit samples. Alpha is tightly packed;
/// luma may have a larger stride. No chroma planes are required or encoded.
pub struct MonochromeAnimationFrame<'a, T = u8> {
    pub y: &'a [T],
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
        self.encode_animation_samples(frames, width, height, timing, options, true)
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
    /// Partial frame edges are padded
    /// internally; the container and decoded frames retain the requested dimensions.
    pub fn encode_animation_yuv420_hbd_with_options(
        &self,
        frames: &[AnimationFrame<'_, u16>],
        width: u32,
        height: u32,
        timing: AnimationTiming,
        options: &AnimationOptions,
    ) -> Result<Vec<u8>, EncodeError> {
        self.encode_animation_samples(frames, width, height, timing, options, true)
    }

    /// Encode 8-bit grayscale frames with optional alpha as a monochrome
    /// animated AVIF. Each frame is an independently decodable sync sample.
    pub fn encode_animation_mono(
        &self,
        frames: &[MonochromeAnimationFrame<'_>],
        width: u32,
        height: u32,
        timing: AnimationTiming,
    ) -> Result<Vec<u8>, EncodeError> {
        self.encode_animation_mono_with_options(
            frames,
            width,
            height,
            timing,
            &AnimationOptions::default(),
        )
    }

    /// Encode grayscale animation with metadata, spatial properties and repetition.
    pub fn encode_animation_mono_with_options(
        &self,
        frames: &[MonochromeAnimationFrame<'_>],
        width: u32,
        height: u32,
        timing: AnimationTiming,
        options: &AnimationOptions,
    ) -> Result<Vec<u8>, EncodeError> {
        self.encode_monochrome_animation_samples(frames, width, height, timing, options)
    }

    /// Encode native 10-bit grayscale and optional alpha. Configure bit depth
    /// 10 on the encoder. Coded levels retain all ten bits at every speed;
    /// mode decisions currently use the upper eight bits.
    pub fn encode_animation_mono_hbd(
        &self,
        frames: &[MonochromeAnimationFrame<'_, u16>],
        width: u32,
        height: u32,
        timing: AnimationTiming,
    ) -> Result<Vec<u8>, EncodeError> {
        self.encode_animation_mono_hbd_with_options(
            frames,
            width,
            height,
            timing,
            &AnimationOptions::default(),
        )
    }

    /// Native 10-bit grayscale animation with metadata and repetition.
    pub fn encode_animation_mono_hbd_with_options(
        &self,
        frames: &[MonochromeAnimationFrame<'_, u16>],
        width: u32,
        height: u32,
        timing: AnimationTiming,
        options: &AnimationOptions,
    ) -> Result<Vec<u8>, EncodeError> {
        self.encode_monochrome_animation_samples(frames, width, height, timing, options)
    }

    fn encode_monochrome_animation_samples<T: AnimationSample>(
        &self,
        frames: &[MonochromeAnimationFrame<'_, T>],
        width: u32,
        height: u32,
        timing: AnimationTiming,
        options: &AnimationOptions,
    ) -> Result<Vec<u8>, EncodeError> {
        let planes: Vec<_> = frames
            .iter()
            .map(|frame| AnimationFrame {
                y: frame.y,
                u: &[],
                v: &[],
                y_stride: frame.y_stride,
                alpha: frame.alpha,
                duration: frame.duration,
            })
            .collect();
        self.encode_animation_samples(&planes, width, height, timing, options, false)
    }

    fn encode_animation_samples<T: AnimationSample>(
        &self,
        frames: &[AnimationFrame<'_, T>],
        width: u32,
        height: u32,
        timing: AnimationTiming,
        options: &AnimationOptions,
        chroma_420: bool,
    ) -> Result<Vec<u8>, EncodeError> {
        self.validate_quality()?;
        self.validate_inert_knobs(chroma_420)?;
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
        if options
            .crop
            .is_some_and(|crop| crop.to_clean_aperture(width, height).is_none())
        {
            return Err(EncodeError::UnsupportedConfig(
                "crop rectangle must be nonempty and within the image",
            ));
        }
        if options.rotation.is_some_and(|r| r > 3) || options.mirror.is_some_and(|m| m > 1) {
            return Err(EncodeError::UnsupportedConfig(
                "rotation must be 0..=3 and mirror axis must be 0..=1",
            ));
        }
        if options
            .pixel_aspect_ratio
            .is_some_and(|p| p.h_spacing == 0 || p.h_spacing != p.v_spacing)
        {
            return Err(EncodeError::UnsupportedConfig(
                "AVIF pixel aspect ratio must be positive and 1:1",
            ));
        }
        if options.amve.is_some_and(|v| !v.is_valid()) {
            return Err(EncodeError::UnsupportedConfig(
                "amve needs nonzero illuminance and chromaticities in 0..=50000",
            ));
        }
        if options.cclv.is_some_and(|v| !v.is_valid()) {
            return Err(EncodeError::UnsupportedConfig(
                "cclv needs a field, primaries in -5000000..=5000000, and min <= avg <= max",
            ));
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
        let cn = if chroma_420 {
            (width as usize).div_ceil(2) * (height as usize).div_ceil(2)
        } else {
            0
        };
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
            .with_chroma_420(chroma_420)
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
            colors.push(if chroma_420 {
                T::encode_color(&mut color, frame)?
            } else {
                T::encode_mono(&mut color, frame.y, frame.y_stride)?
            });
            if let Some(a) = frame.alpha {
                alphas.push(T::encode_mono(&mut alpha, a, width as usize)?);
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
        cfg.monochrome = !chroma_420;
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
        if let Some(crop) = options.crop {
            mux.set_crop(crop);
        }
        if let Some(rotation) = options.rotation {
            mux.set_rotation(rotation);
        }
        if let Some(mirror) = options.mirror {
            mux.set_mirror(mirror);
        }
        if let Some(pasp) = options.pixel_aspect_ratio {
            mux.set_pixel_aspect_ratio(pasp.h_spacing, pasp.v_spacing);
        }
        if let Some(clli) = options.clli {
            mux.set_clli(clli);
        }
        if let Some(mdcv) = options.mdcv {
            mux.set_mdcv(mdcv);
        }
        if let Some(amve) = options.amve {
            mux.set_amve(amve);
        }
        if let Some(cclv) = options.cclv {
            mux.set_cclv(cclv);
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
    fn encode_mono(
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
            fn encode_mono(
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
        for (w, h, has_alpha, speed, lossless, screen) in [
            (64usize, 64usize, true, 7, false, false),
            (65, 67, true, 7, false, false),
            (64, 64, false, 7, false, false),
            (65, 67, false, 2, false, false),
            (64, 64, true, 7, true, false),
            (65, 67, true, 7, true, false),
            (65, 67, false, 7, true, false),
            // Lower-preset lossless screen content, including the fixed-tree
            // p4 IntraBC source path and an odd-sized animation with alpha.
            (64, 64, true, 2, true, true),
            (128, 128, true, 3, true, true),
            (65, 67, true, 2, true, true),
        ] {
            let out =
                std::env::temp_dir().join(format!("svt-animation-{}-{w}x{h}", std::process::id()));
            fs::create_dir_all(&out).unwrap();
            let enc = AvifEncoder::new().with_speed(speed).with_lossless(lossless);
            let colors: Vec<Vec<u8>> = (0..3)
                .map(|f| {
                    (0..w * h)
                        .map(|i| {
                            if screen {
                                let (r, c) = (i / w, i % w);
                                let panel = ((r / 24) & 1) * 2 + ((c / 32 + f) & 1);
                                let text =
                                    (6..12).contains(&(r % 24)) && (c / 3 + r / 24 + f) % 5 != 0;
                                if text { 16 } else { [35, 110, 180, 235][panel] }
                            } else {
                                (32 + (i % w + f * 30) % 180) as u8
                            }
                        })
                        .collect()
                })
                .collect();
            let alphas: Vec<Vec<u8>> = (0..3)
                .map(|f| (0..w * h).map(|i| ((i + f * 13) % 256) as u8).collect())
                .collect();
            let uv: Vec<u8> = (0..w.div_ceil(2) * h.div_ceil(2))
                .map(|i| (100 + (i * 7) % 60) as u8)
                .collect();
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
                        if lossless {
                            assert_eq!(pixels[(y * w + x) * 4 + 3], alpha[y * w + x]);
                        }
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
                let raw = pipe.try_encode_frame_420(source, &uv, &uv, w).unwrap();
                fs::write(out.join("color.obu"), raw).unwrap();
                let mut pre = Vec::new();
                let (py, pu, pv) = pipe.last_recon_unfiltered.as_ref().unwrap();
                for (plane, stride, width, height) in [
                    (py, pipe.width as usize, w, h),
                    (pu, pipe.width as usize / 2, w.div_ceil(2), h.div_ceil(2)),
                    (pv, pipe.width as usize / 2, w.div_ceil(2), h.div_ceil(2)),
                ] {
                    for row in 0..height {
                        pre.extend_from_slice(&plane[row * stride..row * stride + width]);
                    }
                }
                fs::write(out.join("color-prefilter.yuv"), pre).unwrap();
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
                if lossless {
                    assert_eq!(
                        expected,
                        [source.as_slice(), uv.as_slice(), uv.as_slice()].concat()
                    );
                }
                let decoded = fs::read(out.join(format!("frame-{i:010}.y4m"))).unwrap();
                let start = decoded.windows(6).position(|s| s == b"FRAME\n").unwrap() + 6;
                fs::write(out.join("color-final.yuv"), &expected).unwrap();
                assert_eq!(decoded.len() - start, expected.len());
                assert!(
                    decoded[start..] == expected,
                    "8-bit color frame {i}, {w}x{h}, speed {speed}, first differing byte {:?}",
                    decoded[start..]
                        .iter()
                        .zip(&expected)
                        .position(|(a, b)| a != b)
                );
            }
            fs::remove_dir_all(out).unwrap();
        }
    }

    #[test]
    fn native_ten_bit_animation_matches_color_and_alpha_reconstruction() {
        for w in [64usize, 65, 72] {
            let stride = w + 5;
            let directory =
                std::env::temp_dir().join(format!("svt-animation-hbd-{}", std::process::id()));
            fs::create_dir_all(&directory).unwrap();
            for quality in [40.0, 98.0, 100.0] {
                for (speed, has_alpha) in [(9, true), (6, false)] {
                    let enc = AvifEncoder::new()
                        .with_bit_depth(10)
                        .with_speed(speed)
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
                    let u: Vec<u16> = (0..w.div_ceil(2).pow(2))
                        .map(|i| (480 + i % 64) as u16)
                        .collect();
                    let v: Vec<u16> = (0..w.div_ceil(2).pow(2))
                        .map(|i| (500 + i % 32) as u16)
                        .collect();
                    let frames: Vec<_> = (0..2)
                        .map(|i| AnimationFrame {
                            y: &colors[i],
                            u: &u,
                            v: &v,
                            y_stride: stride,
                            alpha: has_alpha.then_some(alphas[i].as_slice()),
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
                        let raw = pipe
                            .try_encode_frame_420_hbd(&colors[i], &u, &v, stride)
                            .unwrap();
                        let without_recon = enc
                            .build_pipeline(w as u32, w as u32)
                            .with_chroma_420(true)
                            .try_encode_frame_420_hbd(&colors[i], &u, &v, stride)
                            .unwrap();
                        assert_eq!(
                            raw, without_recon,
                            "reconstruction must not change coded bytes"
                        );
                        fs::write(directory.join("color.obu"), raw).unwrap();
                        let mut pre = Vec::new();
                        let (pu, pv) = pipe.last_recon10_uv.as_ref().unwrap();
                        for (plane, stride, side) in [
                            (
                                pipe.last_recon10_y.as_ref().unwrap(),
                                pipe.width as usize,
                                w,
                            ),
                            (pu, pipe.width as usize / 2, w.div_ceil(2)),
                            (pv, pipe.width as usize / 2, w.div_ceil(2)),
                        ] {
                            for row in 0..side {
                                pre.extend(
                                    plane[row * stride..row * stride + side]
                                        .iter()
                                        .flat_map(|n| n.to_le_bytes()),
                                );
                            }
                        }
                        fs::write(directory.join("color-prefilter.yuv"), pre).unwrap();
                        let (y, u, v) = pipe.last_recon10_final.unwrap();
                        let mut expected = Vec::new();
                        for (plane, stride, side) in [
                            (&y, pipe.width as usize, w),
                            (&u, pipe.width as usize / 2, w.div_ceil(2)),
                            (&v, pipe.width as usize / 2, w.div_ceil(2)),
                        ] {
                            for row in 0..side {
                                expected.extend(
                                    plane[row * stride..row * stride + side]
                                        .iter()
                                        .flat_map(|n| n.to_le_bytes()),
                                );
                            }
                        }
                        if quality == 100.0 {
                            let mut source: Vec<u8> = colors[i]
                                .chunks_exact(stride)
                                .flat_map(|row| row[..w].iter().flat_map(|v| v.to_le_bytes()))
                                .collect();
                            source.extend(frames[i].u.iter().flat_map(|v| v.to_le_bytes()));
                            source.extend(frames[i].v.iter().flat_map(|v| v.to_le_bytes()));
                            assert_eq!(expected, source, "native lossless color must equal source");
                        }
                        assert!(
                            y.iter().any(|n| n & 3 != 0),
                            "fixture must preserve native low bits"
                        );
                        let decoded =
                            fs::read(directory.join(format!("frame-{i:010}.y4m"))).unwrap();
                        let offset = decoded.windows(6).position(|p| p == b"FRAME\n").unwrap() + 6;
                        fs::write(directory.join("color-final.yuv"), &expected).unwrap();
                        assert_eq!(decoded.len() - offset, expected.len());
                        assert!(
                            decoded[offset..] == expected,
                            "10-bit color frame {i}, side {w}, quality {quality}, first differing byte {:?}",
                            decoded[offset..]
                                .iter()
                                .zip(&expected)
                                .position(|(a, b)| a != b)
                        );

                        if !has_alpha {
                            continue;
                        }
                        let mut settings = enc.clone();
                        settings.full_range = true;
                        let mut pipe = settings
                            .build_pipeline(w as u32, w as u32)
                            .with_recon_output(true);
                        pipe.try_encode_frame_hbd(&alphas[i], w).unwrap();
                        let (alpha, _, _) = pipe.last_recon10_final.unwrap();
                        assert_eq!(alpha.len(), pipe.width as usize * pipe.height as usize);
                        let file =
                            fs::File::open(directory.join(format!("frame-{i:010}.png"))).unwrap();
                        let mut reader = png::Decoder::new(std::io::BufReader::new(file))
                            .read_info()
                            .unwrap();
                        let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
                        let info = reader.next_frame(&mut pixels).unwrap();
                        assert_eq!(info.bit_depth, png::BitDepth::Sixteen);
                        assert_eq!(info.color_type, png::ColorType::Rgba);
                        assert_eq!((info.width as usize, info.height as usize), (w, w));
                        for (pixel_index, pixel) in pixels.as_chunks::<8>().0.iter().enumerate() {
                            let sample =
                                alpha[(pixel_index / w) * pipe.width as usize + pixel_index % w];
                            if quality == 100.0 {
                                assert_eq!(
                                    sample, alphas[i][pixel_index],
                                    "native lossless alpha must equal source"
                                );
                            }
                            let actual = u16::from_be_bytes([pixel[6], pixel[7]]);
                            // PNG expands alpha to 16 bits. Recover the native 10-bit
                            // integer to avoid testing the reader's expansion rounding.
                            let native = ((u32::from(actual) * 1023 + 32767) / 65535) as u16;
                            assert_eq!(native, sample, "10-bit alpha frame {i}");
                        }
                    }
                }
            }
            fs::remove_dir_all(directory).unwrap();
        }
    }

    #[test]
    fn monochrome_animation_decodes_with_exact_luma_and_alpha() {
        fn luma_sample(i: usize, frame: usize, depth: u8) -> u32 {
            let scale = 1u32 << (depth - 8);
            (32 + (i * 7 + frame * 30) % 180) as u32 * scale + i as u32 % scale
        }
        macro_rules! exercise {
            ($sample:ty, $depth:expr, $speed:expr, $entry:ident, $raw:ident, $sizes:expr) => {
                for (w, h) in $sizes {
                    for quality in [40.0, 98.0, 100.0] {
                        for has_alpha in [false, true] {
                            let dir = std::env::temp_dir().join(format!("svt-mono-animation-{}-{}-{w}x{h}-{}-{quality}-{has_alpha}", std::process::id(), $depth, $speed));
                            fs::create_dir_all(&dir).unwrap();
                            let stride = w + 5;
                            let max = (1u32 << $depth) - 1;
                            let colors: Vec<Vec<$sample>> = (0..2).map(|f| {
                                (0..stride*h).map(|i| luma_sample(i, f, $depth) as $sample).collect()
                            }).collect();
                            let alphas: Vec<Vec<$sample>> = (0..2).map(|f| {
                                (0..w*h).map(|i| ((i*3+f*47) as u32 & max) as $sample).collect()
                            }).collect();
                            let frames: Vec<_> = (0..2).map(|i| MonochromeAnimationFrame {
                                y: &colors[i], y_stride: stride,
                                alpha: has_alpha.then_some(alphas[i].as_slice()), duration: [17, 29][i],
                            }).collect();
                            let enc = AvifEncoder::new().with_bit_depth($depth).with_speed($speed).with_quality(quality);
                            let bytes = enc.$entry(&frames, w as u32, h as u32, AnimationTiming { timescale: 1000 }).unwrap();
                            fs::write(dir.join("input.avif"), bytes).unwrap();
                            for extension in ["y4m", "png"] {
                                let result = Command::new("avifdec").args(["-j", "1", "--index", "all"])
                                    .arg(dir.join("input.avif")).arg(dir.join(format!("frame.{extension}"))).output().unwrap();
                                assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
                            }
                            let mut color_pipe = enc.build_pipeline(w as u32, h as u32).with_image_sequence().with_recon_output(true);
                            let mut alpha_settings = enc.clone();
                            alpha_settings.full_range = true;
                            let mut alpha_pipe = alpha_settings.build_pipeline(w as u32, h as u32).with_image_sequence().with_recon_output(true);
                            color_pipe.rc_config.framerate = 1000.0 / 17.0;
                            alpha_pipe.rc_config.framerate = 1000.0 / 17.0;
                            for i in 0..2 {
                                color_pipe.$raw(&colors[i], stride).unwrap();
                                let reference: Vec<u16> = if $depth == 8 {
                                    color_pipe.last_recon.take().unwrap().0.into_iter().map(u16::from).collect()
                                } else { color_pipe.last_recon10_final.take().unwrap().0 };
                                let y4m = fs::read(dir.join(format!("frame-{i:010}.y4m"))).unwrap();
                                let header_end = y4m.iter().position(|&b| b == b'\n').unwrap();
                                let header = std::str::from_utf8(&y4m[..header_end]).unwrap();
                                assert!(header.contains(if $depth == 8 { "Cmono " } else { "Cmono10 " }), "{header}");
                                assert_eq!(&y4m[header_end+1..header_end+7], b"FRAME\n");
                                let pixels = &y4m[header_end+7..];
                                assert_eq!(pixels.len(), w*h*if $depth == 8 { 1 } else { 2 });
                                for y in 0..h { for x in 0..w {
                                    let offset = y*w+x;
                                    let actual = if $depth == 8 { u16::from(pixels[offset]) }
                                        else { u16::from_le_bytes([pixels[2*offset], pixels[2*offset+1]]) };
                                    if quality == 100.0 { assert_eq!(actual, colors[i][y*stride+x] as u16, "lossless luma must equal source"); }
                                    assert_eq!(actual, reference[y*color_pipe.width as usize+x], "luma depth={} {w}x{h} q={quality} frame={i} ({x},{y})", $depth);
                                }}
                                if has_alpha {
                                    alpha_pipe.$raw(&alphas[i], w).unwrap();
                                    let reference: Vec<u16> = if $depth == 8 {
                                        alpha_pipe.last_recon.take().unwrap().0.into_iter().map(u16::from).collect()
                                    } else { alpha_pipe.last_recon10_final.take().unwrap().0 };
                                    let mut reader = png::Decoder::new(std::io::BufReader::new(fs::File::open(dir.join(format!("frame-{i:010}.png"))).unwrap())).read_info().unwrap();
                                    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
                                    let info = reader.next_frame(&mut pixels).unwrap();
                                    assert_eq!(info.color_type, png::ColorType::GrayscaleAlpha);
                                    assert_eq!(info.bit_depth, if $depth == 8 { png::BitDepth::Eight } else { png::BitDepth::Sixteen });
                                    assert_eq!((info.width, info.height), (w as u32, h as u32));
                                    for y in 0..h { for x in 0..w {
                                        let pixel = y*w+x;
                                        let actual = if $depth == 8 { u16::from(pixels[2*pixel+1]) } else {
                                            let wide = u16::from_be_bytes([pixels[4*pixel+2], pixels[4*pixel+3]]);
                                            ((u32::from(wide)*1023+32767)/65535) as u16
                                        };
                                        if quality == 100.0 { assert_eq!(actual, alphas[i][y*w+x] as u16, "lossless alpha must equal source"); }
                                        assert_eq!(actual, reference[y*alpha_pipe.width as usize+x], "alpha depth={} {w}x{h} q={quality} frame={i} ({x},{y})", $depth);
                                    }}
                                }
                            }
                            fs::remove_dir_all(dir).unwrap();
                        }
                    }
                }
            };
        }
        exercise!(
            u8,
            8,
            7,
            encode_animation_mono,
            try_encode_frame,
            [(64usize, 80usize), (65, 67)]
        );
        exercise!(
            u8,
            8,
            2,
            encode_animation_mono,
            try_encode_frame,
            [(64usize, 64usize), (64, 80), (65, 67)]
        );
        exercise!(
            u16,
            10,
            9,
            encode_animation_mono_hbd,
            try_encode_frame_hbd,
            [(64usize, 80usize), (65, 67)]
        );
        exercise!(
            u16,
            10,
            2,
            encode_animation_mono_hbd,
            try_encode_frame_hbd,
            [(64usize, 80usize), (65, 67)]
        );
    }

    #[test]
    fn validates_whole_monochrome_animation_before_encoding() {
        let plane = vec![128; 65 * 67];
        let frames = [
            MonochromeAnimationFrame {
                y: &plane,
                y_stride: 65,
                alpha: None,
                duration: 1,
            },
            MonochromeAnimationFrame {
                y: &plane,
                y_stride: 65,
                alpha: None,
                duration: 0,
            },
        ];
        // Validate the whole submission, including an invalid second frame.
        assert!(matches!(
            AvifEncoder::new().with_speed(2).encode_animation_mono(
                &frames,
                65,
                67,
                AnimationTiming { timescale: 1000 }
            ),
            Err(EncodeError::InvalidDimensions)
        ));
    }

    #[test]
    fn validates_crop_before_encoding() {
        let plane = vec![128; 64 * 80];
        let frames = [AnimationFrame {
            y: &plane,
            u: &plane,
            v: &plane,
            y_stride: 64,
            alpha: None,
            duration: 1,
        }];
        for crop in [
            CropRect::new(0, 0, 0, 80),
            CropRect::new(0, 0, 64, 0),
            CropRect::new(64, 0, 1, 1),
            CropRect::new(0, 80, 1, 1),
            CropRect::new(1, 0, 64, 80),
            CropRect::new(0, 1, 64, 80),
            CropRect::new(u32::MAX, 0, 2, 2),
            CropRect::new(0, 0, u32::MAX, 1),
        ] {
            let options = AnimationOptions {
                crop: Some(crop),
                ..AnimationOptions::default()
            };
            assert!(matches!(
                AvifEncoder::new().encode_animation_yuv420_with_options(
                    &frames,
                    64,
                    80,
                    AnimationTiming { timescale: 1000 },
                    &options
                ),
                Err(EncodeError::UnsupportedConfig(
                    "crop rectangle must be nonempty and within the image"
                ))
            ));
        }
    }

    #[test]
    fn validates_orientation_before_encoding() {
        let plane = vec![128; 64 * 64];
        let frames = [AnimationFrame {
            y: &plane,
            u: &plane,
            v: &plane,
            y_stride: 64,
            alpha: None,
            duration: 1,
        }];
        for (rotation, mirror) in [(4, 0), (255, 0), (0, 2), (0, 255)] {
            let options = AnimationOptions {
                rotation: Some(rotation),
                mirror: Some(mirror),
                ..AnimationOptions::default()
            };
            assert!(matches!(
                AvifEncoder::new().encode_animation_yuv420_with_options(
                    &frames,
                    64,
                    64,
                    AnimationTiming { timescale: 1000 },
                    &options
                ),
                Err(EncodeError::UnsupportedConfig(
                    "rotation must be 0..=3 and mirror axis must be 0..=1"
                ))
            ));
        }
    }

    #[test]
    fn validates_pixel_aspect_ratio_before_encoding() {
        let plane = vec![128; 64 * 64];
        let frames = [AnimationFrame {
            y: &plane,
            u: &plane,
            v: &plane,
            y_stride: 64,
            alpha: None,
            duration: 1,
        }];
        for (h, v) in [(0, 0), (0, 1), (1, 0), (2, 1), (1, 2)] {
            let options = AnimationOptions {
                pixel_aspect_ratio: Some(PaspBox::new(h, v)),
                ..AnimationOptions::default()
            };
            assert!(matches!(
                AvifEncoder::new().encode_animation_yuv420_with_options(
                    &frames,
                    64,
                    64,
                    AnimationTiming { timescale: 1000 },
                    &options
                ),
                Err(EncodeError::UnsupportedConfig(
                    "AVIF pixel aspect ratio must be positive and 1:1"
                ))
            ));
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
