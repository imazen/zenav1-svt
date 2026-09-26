//! Animated AVIF encoding.
//!
//! The colour track is inter-coded by default: [`AnimationOptions::keyframes`]
//! sets how often a key frame (a sync sample) appears, and every other picture
//! is coded against the one before it. Inter coding is what makes an animation
//! smaller than the same frames as separate stills -- MEASURED on an 8-frame
//! 256x256 clip, see that field's docs.
//!
//! An ALPHA track, when present, stays all-intra; an all-sync alpha track is
//! strictly more seekable than the colour track it accompanies, so nothing a
//! player can do is lost. Monochrome tracks inter-code on the same terms as
//! colour since 2026-09-21 (`mono_inter_gate.sh`).
use super::{AvifEncoder, EncodeError};
use zenavif_serialize::{
    Av1CBox,
    animated::{AnimFrame, AnimatedImage},
};

pub use zenavif_serialize::{
    AmveBox, CclvBox, ClliBox, MdcvBox, PaspBox,
    animated::{CropRect, RepetitionCount},
};

/// How often an animation's colour track carries a key frame.
///
/// A key frame is a sync sample: a player can start or seek there without
/// having decoded anything earlier. Every other picture is coded against its
/// predecessor, which is where the compression comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keyframes {
    /// Every picture is a key frame. The largest files, and the only
    /// behaviour this path had before 2026-09-11.
    EveryFrame,
    /// One key frame every `n` pictures. `n` is clamped to at least 1, and
    /// `Every(1)` is [`Self::EveryFrame`].
    Every(u32),
}

/// Whether `why` is the pipeline's refusal of a monochrome inter frame it
/// cannot reconstruct as the decoder does: coded-lossless inter
/// (`lossless_config_error` — the mono lossless arm has no inter WHT
/// residual path). A monochrome animation that hits it is coded all-intra
/// instead. The monochrome animation test fails if the pipeline's wording
/// drifts from this.
fn is_mono_inter_refusal(why: &str) -> bool {
    why.starts_with("QP 0 (coded-lossless) inter frames are not implemented")
}

impl Keyframes {
    /// C's `GopStructure::intra_period`.
    fn intra_period(self) -> u32 {
        match self {
            Self::EveryFrame => 1,
            Self::Every(n) => n.max(1),
        }
    }
    /// Whether the picture at `index` is coded as a key frame, matching
    /// `GopStructure::is_key_frame`.
    fn is_key(self, index: usize) -> bool {
        let n = self.intra_period() as usize;
        index % n == 0
    }
}

impl Default for Keyframes {
    /// `Every(120)` — at a typical 24-30 fps that is a key frame every four to
    /// five seconds, the usual trade for seekable playback, and it makes any
    /// animation of 120 pictures or fewer a single closed GOP.
    fn default() -> Self {
        Self::Every(120)
    }
}

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
    /// How often the COLOUR track carries a key frame. Defaults to
    /// [`Keyframes::Every`]`(120)`; [`Keyframes::EveryFrame`] restores the
    /// all-intra behaviour this path had before 2026-09-11.
    ///
    /// MEASURED 2026-09-11, eight 256x256 frames of the public-domain derf
    /// clip `fourpeople` at quality 70: 58,823 B all-intra against 21,104 B
    /// as one closed GOP — 2.8x smaller for the same pixels in.
    ///
    /// Inter coding here is verified against a DECODER, not against C's
    /// bytes: `tools/video_selfcheck_gate.sh` requires the encoder's own
    /// reconstruction to equal `aomdec`'s on every frame. The alpha track is
    /// unaffected and stays all-intra.
    pub keyframes: Keyframes,
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
            keyframes: Keyframes::default(),
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnimationTiming {
    /// Ticks per second, strictly positive.
    pub timescale: u32,
}

impl AvifEncoder {
    /// The key-frame interval this encoder will actually use for a colour
    /// track, which is not always the one the caller asked for.
    ///
    /// A MONOCHROME track is always all-intra: `encode_frame_impl` refuses an
    /// inter frame on that arm, because every inter gate in this repo is
    /// 4:2:0.
    ///
    /// AT ANY DEPTH ABOVE 8, a colour track is all-intra too:
    /// `encode_frame_impl` refuses an inter frame there, because the port's
    /// reconstruction disagrees with a decoder's on some 10-bit content.
    /// Coding key frames instead is the honest fallback -- a larger file,
    /// never a wrong one, and the same shape `bd10_tree_supported` uses for
    /// 10-bit OBMC. The refusal's own text carries the measurement. (The
    /// preset-6 floor this check used to mirror came off on 2026-09-15, when
    /// the OBMC neighbour-prediction stale-cache fix swept the whole preset
    /// ladder clean against aomdec.)
    ///
    /// LOSSLESS is all-intra as a product choice: a lossless animation is a
    /// sequence of lossless stills, which is what a caller asking for
    /// lossless wants anyway. (QP 0 inter frames are implemented at 8-bit
    /// since 2026-09-21 — `qp0_inter_gate.sh` — so this is pacing policy,
    /// not a refusal.)
    ///
    /// MONOCHROME is inter-eligible at 8 bits since 2026-09-21 —
    /// `mono_inter_gate.sh` pins encoder-recon == aomdec == dav1d with real
    /// nonzero MVs. The chroma-free inter path is the same funnel; mono
    /// simply has no chroma planes to predict, code or filter.
    fn animation_keyframes(&self, options: &AnimationOptions) -> Keyframes {
        if !self.lossless && self.bit_depth <= 8 {
            options.keyframes
        } else {
            Keyframes::EveryFrame
        }
    }

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
        match self.encode_animation_samples(&planes, width, height, timing, options, false) {
            // The pipeline refuses one kind of monochrome inter frame
            // (`is_mono_inter_refusal`): coded-lossless ones, which a
            // near-lossless setting produces because the video-mode QP scaling
            // lowers inter frames below the key frame by a content-dependent
            // amount (measured 2026-09-25: cli qp <= 2, quality >= 97).
            // Monochrome is a Rust extension with no C reference; such an
            // animation is coded all-intra, which is what every monochrome
            // animation was before inter frames were enabled for it
            // (561267534).
            Err(EncodeError::UnsupportedConfig(why))
                if is_mono_inter_refusal(why) && options.keyframes != Keyframes::EveryFrame =>
            {
                let mut all_key = options.clone();
                all_key.keyframes = Keyframes::EveryFrame;
                self.encode_animation_samples(&planes, width, height, timing, &all_key, false)
            }
            other => other,
        }
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
            return Err(EncodeError::InvalidDimensions {
                width,
                height,
                reason: "animation needs frames, a non-zero timescale and dimensions in 1..=65535",
            });
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
                u32::try_from(frame.y_stride).map_err(|_| EncodeError::InvalidDimensions {
                    width,
                    height,
                    reason: "animation luma stride exceeds u32",
                })?;
            self.validate_dimensions(frame.y.len(), width, height, stride)?;
            if frame.duration == 0
                || frame.u.len() < cn
                || frame.v.len() < cn
                || frame.alpha.is_some() != has_alpha
                || frame.alpha.is_some_and(|a| a.len() < n)
            {
                return Err(EncodeError::InvalidDimensions {
                    width,
                    height,
                    reason: "animation frame needs non-zero duration, complete chroma and alpha planes, and consistent alpha presence",
                });
            }
        }
        // Monochrome animations inter-code on the same eligibility terms as
        // colour since 2026-09-21 (`mono_inter_gate.sh`): the funnel has no
        // chroma planes to predict or code, and both aomdec and dav1d
        // reconstruct mono inter frames byte-identically to our recon.
        let keyframes = self.animation_keyframes(options);
        // ABOVE 8 BITS the colour track is all-intra too — now a DELIBERATE
        // policy, not a correctness fallback: `encode_frame_impl` accepts a
        // 10-bit inter frame since 2026-09-18 (the `hbd_md = 2` MDS3 bump
        // mirror; `bd10_video_selfcheck_gate.sh` pins 396/396 cells of
        // encoder-recon == aomdec). Animations stay all-intra at bd10 because
        // the AVIF animation pacing/keyframe cadence for inter content is a
        // separate product decision that has not been measured; revisit
        // `animation_keyframes`' `bit_depth <= 8` term when it is.
        let mut color = self
            .build_pipeline_gop(width, height, keyframes.intra_period())
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
                // A sync sample is one a player can START at. That is
                // exactly the key frames now, not every picture: an inter
                // frame's reconstruction depends on the one before it, and
                // marking it `stss` would invite a seek that decodes garbage.
                let sample = AnimFrame::new(
                    colors[i]
                        .strip_prefix(delimiter.as_slice())
                        .unwrap_or(&colors[i]),
                    f.duration,
                )
                .with_sync(keyframes.is_key(i));
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
// Explicit path: this file is itself loaded by `#[path]` from avif.rs, and
// rustfmt resolves a bare `mod tests;` here to src/tests.rs.
#[path = "animation/tests.rs"]
mod tests;
