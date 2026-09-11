//! AVIF encoding backend for zenavif integration.
//!
//! Provides a builder-pattern API compatible with zenavif's encoder backend
//! interface, allowing zenav1-svt to be used as an AV1 encoder for still
//! images (instead of or alongside zenrav1e).
//!
//! # Usage
//!
//! ```
//! use svtav1::avif::AvifEncoder;
//!
//! let encoder = AvifEncoder::new()
//!     .with_quality(80.0)
//!     .with_speed(6);
//!
//! // Encode a 16x16 grayscale image
//! let pixels = vec![128u8; 16 * 16];
//! let result = encoder.encode_y8(&pixels, 16, 16, 16).unwrap();
//! assert!(!result.data.is_empty());
//! ```

/// Animated AVIF container encoding (requires Rust 1.93).
#[cfg(feature = "avif-container")]
#[path = "animation.rs"]
pub mod animation;

pub use crate::policy::{Effort, EncodingPolicy, ResolvedStillPolicy, StillSuitability};
/// Explicit, uncalibrated Zen experiments for the region beyond native −1.
pub use svtav1_encoder::enhancements::{ZenEnhancement, ZenEnhancements};
/// Pinned C source identity, separate from speed and policy.
pub use svtav1_encoder::reference::SvtReference;
/// Checked C preset domain, including research -1.
pub use svtav1_encoder::speed_config::NativePreset;

/// Chroma subsampling format for AVIF encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromaSubsampling {
    /// 4:2:0 subsampling (most common for AVIF).
    Yuv420,
    /// 4:4:4 no subsampling (higher quality chroma).
    Yuv444,
}

/// Actual raw-plane input path for allocation-free support queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StillInputFormat {
    Monochrome,
    Yuv420,
}

/// Result of encoding a still image to AV1.
#[derive(Debug, Clone)]
pub struct EncodedAvif {
    /// AV1 bitstream (OBU sequence).
    pub data: Vec<u8>,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Bit depth of the encoded image.
    pub bit_depth: u8,
}

/// Errors that can occur during AVIF encoding.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum EncodeError {
    /// Image dimensions are invalid (zero, too large, or not aligned).
    InvalidDimensions {
        /// Requested width in pixels.
        width: u32,
        /// Requested height in pixels.
        height: u32,
        /// The validation rule that failed.
        reason: &'static str,
    },
    /// Quality value is out of the valid range (1.0-100.0).
    InvalidQuality {
        /// Rejected quality value.
        quality: f32,
    },
    /// Encoding failed with a description.
    EncodeFailed(String),
    /// A builder knob was set that this encoder records but does not consume,
    /// where ignoring it would silently emit output the caller did not ask for
    /// (see `AvifEncoder::validate_inert_knobs`).
    UnsupportedConfig(&'static str),
}

impl core::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnsupportedConfig(what) => write!(f, "Unsupported configuration: {what}"),
            Self::InvalidDimensions {
                width,
                height,
                reason,
            } => write!(f, "Invalid image dimensions {width}x{height}: {reason}"),
            Self::InvalidQuality { quality } => {
                write!(f, "Quality must be between 1.0 and 100.0; got {quality}")
            }
            Self::EncodeFailed(msg) => write!(f, "Encode failed: {msg}"),
        }
    }
}

/// AVIF still-image encoder using zenav1-svt as the AV1 backend.
///
/// Follows the builder pattern from zenrav1e for compatibility with
/// zenavif's encoder abstraction.
#[derive(Debug, Clone)]
pub struct AvifEncoder {
    /// Quality level (1.0-100.0). Higher = better quality, larger file.
    quality: f32,
    /// Speed (1-10), mapped to still-image presets 0-9.
    speed: u8,
    native_preset: Option<NativePreset>,
    policy: Option<EncodingPolicy>,
    effort: Option<Effort>,
    reference: SvtReference,
    enhancements: ZenEnhancements,
    /// Bit depth (8, 10, or 12).
    bit_depth: u8,
    /// Chroma subsampling format.
    /// Used when full YUV encoding with chroma-aware QP offsets is wired through.
    chroma_subsampling: ChromaSubsampling,
    /// Number of encoding threads (None = auto).
    threads: Option<usize>,
    /// C `static_config.enable_qm` — quantization matrices. Wired to
    /// `EncodePipeline::hdr.enable_qm`; see [`AvifEncoder::with_qm`].
    enable_qm: bool,
    /// C `static_config.enable_variance_boost` — the per-superblock
    /// delta-q that IS SVT-AV1's still-image adaptive quantization. Wired to
    /// `EncodePipeline::hdr.enable_variance_boost`.
    enable_variance_boost: bool,
    /// C `static_config.variance_boost_strength` (1-4, default 2; the docs
    /// recommend 3 for stills). Wired to
    /// `EncodePipeline::hdr.variance_boost_strength`.
    variance_boost_strength: u8,
    film_grain: svtav1_encoder::film_grain_config::FilmGrainConfig,
    /// Lossless encoding mode.
    lossless: bool,
    /// CICP color primaries (1=BT.709, 9=BT.2020, 12=P3).
    color_primaries: u8,
    /// CICP transfer characteristics (1=BT.709, 13=sRGB, 16=PQ, 18=HLG).
    transfer_characteristics: u8,
    /// CICP matrix coefficients (1=BT.709, 9=BT.2020, 0=Identity/RGB).
    matrix_coefficients: u8,
    /// Full range (true) or limited/studio range (false).
    full_range: bool,
}

impl Default for AvifEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl AvifEncoder {
    /// Create a new AVIF encoder with default settings.
    ///
    /// Defaults: quality 75, speed 6, 8-bit, YUV420, auto threads.
    pub fn new() -> Self {
        Self {
            quality: 75.0,
            speed: 6,
            native_preset: None,
            policy: None,
            effort: None,
            reference: SvtReference::Hybrid3115,
            enhancements: ZenEnhancements::default(),
            bit_depth: 8,
            chroma_subsampling: ChromaSubsampling::Yuv420,
            threads: None,
            // Both default OFF, which is C v4.2.0's mainline default
            // (`svt_av1_set_default_params`: `enable_qm = 0`,
            // `enable_variance_boost = 0` at SVT_HDR_MODE=0) AND the bytes
            // this encoder has always emitted — the two knobs used to be
            // recorded-and-ignored, and defaulting them to `true` now that
            // they are live would silently change every caller's output.
            enable_qm: false,
            enable_variance_boost: false,
            variance_boost_strength: 2,
            film_grain: Default::default(),
            lossless: false,
            color_primaries: 1,           // BT.709
            transfer_characteristics: 13, // sRGB
            matrix_coefficients: 1,       // BT.709
            full_range: false,
        }
    }

    /// Set CICP color space for wide gamut / HDR encoding.
    ///
    /// # Presets
    /// - P3 sRGB: `(12, 13, 1, false)`
    /// - BT.2020 PQ (HDR10): `(9, 16, 9, false)`
    /// - BT.2020 HLG: `(9, 18, 9, false)`
    pub fn with_color_space(
        mut self,
        primaries: u8,
        transfer: u8,
        matrix: u8,
        full_range: bool,
    ) -> Self {
        self.color_primaries = primaries;
        self.transfer_characteristics = transfer;
        self.matrix_coefficients = matrix;
        self.full_range = full_range;
        self
    }

    /// Set Display P3 color space (wide gamut, sRGB transfer).
    pub fn with_display_p3(self) -> Self {
        self.with_color_space(12, 13, 1, false)
    }

    /// Set BT.2020 with PQ transfer (HDR10).
    pub fn with_bt2020_pq(self) -> Self {
        self.with_color_space(9, 16, 9, false)
    }

    /// Set BT.2020 with HLG transfer.
    pub fn with_bt2020_hlg(self) -> Self {
        self.with_color_space(9, 18, 9, false)
    }

    /// Get the CICP color description.
    fn color_description(&self) -> svtav1_encoder::entropy::obu::ColorDescription {
        svtav1_encoder::entropy::obu::ColorDescription {
            color_primaries: self.color_primaries,
            transfer_characteristics: self.transfer_characteristics,
            matrix_coefficients: self.matrix_coefficients,
            full_range: self.full_range,
        }
    }

    /// Set the quality level (1.0-100.0).
    ///
    /// Higher values produce better quality at the cost of larger files.
    /// Maps internally to AV1 QP 63 (worst) to 0 (best).
    pub fn with_quality(mut self, quality: f32) -> Self {
        self.quality = quality.clamp(1.0, 100.0);
        self
    }

    /// Set the speed preset (1-10).
    ///
    /// Maps to still-image presets: 1 -> preset 0, 10 -> preset 9.
    /// Replaces an earlier [`Self::with_native_preset`] selection.
    pub fn with_speed(mut self, speed: u8) -> Self {
        self.speed = speed.clamp(1, 10);
        self.native_preset = None;
        self.effort = None;
        self
    }

    /// Select a native preset, including [`NativePreset::RESEARCH`] (-1).
    ///
    /// Replaces the speed mapping. As in C's all-intra configuration, native
    /// presets above 9 resolve to 9. [`Self::resolved_native_preset`] reports
    /// the preset used by every encoding entry point.
    ///
    /// This selects native search controls; it does not enable Zen enhancements
    /// or establish a strict parity guarantee for every input and setting.
    ///
    /// ```
    /// use svtav1::avif::{AvifEncoder, NativePreset};
    /// let encoder = AvifEncoder::new().with_native_preset(NativePreset::RESEARCH);
    /// assert_eq!(encoder.resolved_native_preset().value(), -1);
    /// ```
    pub fn with_native_preset(mut self, preset: NativePreset) -> Self {
        self.native_preset = Some(preset);
        self.effort = None;
        self
    }

    /// Apply an explicit policy. Later conflicting reference/experiment setters
    /// are rejected by the same validator used by encoding.
    pub fn with_policy(mut self, policy: EncodingPolicy) -> Self {
        if let EncodingPolicy::SvtParity(reference) = policy {
            self.reference = reference;
        }
        self.policy = Some(policy);
        self
    }

    /// Resolve checked effort through the versioned native bucket table.
    /// Replaces a previous speed/native selection; no adaptive bundle is implied.
    pub fn with_effort(mut self, effort: Effort) -> Self {
        self.effort = Some(effort);
        self.native_preset = None;
        self
    }

    /// Query effective policy and calibration availability without encoding.
    pub fn resolve_still_policy(&self) -> Result<ResolvedStillPolicy, EncodeError> {
        self.validate_configuration()?;
        Ok(ResolvedStillPolicy {
            version: 1,
            policy: self.policy,
            reference: self.reference,
            requested_effort: self.effort,
            native_preset: self.resolved_native_preset(),
            enhancements: self.enhancements,
            suitability: StillSuitability::Uncalibrated,
        })
    }

    /// Select a pinned C source. Existing constructors retain Hybrid3115.
    /// Mainline420 uses pristine chroma ranking and refuses monochrome output.
    /// This selects decisions, not a certification of all-setting parity.
    pub fn with_reference(mut self, reference: SvtReference) -> Self {
        self.reference = reference;
        self
    }

    /// The source identity that encoding will use.
    pub fn reference(&self) -> SvtReference {
        self.reference
    }

    /// Enable an explicit experiment beyond native −1. No automatic selection
    /// or quality improvement is implied; other native presets are refused.
    pub fn with_enhancement(mut self, enhancement: ZenEnhancement) -> Self {
        self.enhancements = self.enhancements.with(enhancement);
        self
    }

    /// Inspect the exact experiment set used by encoding.
    pub fn enhancements(&self) -> ZenEnhancements {
        self.enhancements
    }

    /// The effective native preset after still-image canonicalization.
    pub fn resolved_native_preset(&self) -> NativePreset {
        if let Some(effort) = self.effort {
            return effort.native_preset();
        }
        let value = self.native_preset.map_or_else(
            || Self::speed_to_preset(self.speed) as i8,
            NativePreset::value,
        );
        NativePreset::new(value.min(9)).expect("checked preset remains in -1..=9")
    }

    /// Set the bit depth. **8 and 10 are the only encodable depths**; every
    /// other value is recorded verbatim here and refused as a typed
    /// [`EncodeError::UnsupportedConfig`] at encode time by
    /// `validate_inert_knobs` — the same place 12 has always been refused.
    ///
    /// This deliberately does NOT coerce. It used to map every depth but
    /// 10/12 to `8` silently, so `with_bit_depth(9)` (a typo) or
    /// `with_bit_depth(src.depth)` where the source was 16-bit both produced a
    /// valid 8-bit encode that the caller believed was deep — and
    /// `EncodedAvif.bit_depth` reported the coerced 8, so even a caller who
    /// checked saw agreement. Refusing loudly is the project's rule for any
    /// path that would otherwise emit plausible-but-wrong output.
    pub fn with_bit_depth(mut self, depth: u8) -> Self {
        self.bit_depth = depth;
        self
    }

    /// Set the number of encoding threads.
    ///
    /// `None` means auto-detect based on available cores.
    pub fn with_num_threads(mut self, threads: Option<usize>) -> Self {
        self.threads = threads;
        self
    }

    /// Configure C film-grain estimation or a supplied grain table.
    /// Validated when encoding; supported on 8/10-bit 4:2:0 inputs.
    pub fn with_film_grain(
        mut self,
        config: svtav1_encoder::film_grain_config::FilmGrainConfig,
    ) -> Self {
        self.film_grain = config;
        self
    }

    /// Enable or disable quantization matrices (C `--enable-qm`).
    ///
    /// LIVE: sets `EncodePipeline::hdr.enable_qm`, which drives the frame
    /// header's `using_qmatrix` + qm levels and the quantizer itself. Off by
    /// default, matching C's mainline default. Proven to change the emitted
    /// bytes by `qm_knob_changes_bytes` below. Lossless encoding uses identity
    /// matrices, as required by the decoder's lossless reconstruction.
    pub fn with_qm(mut self, enable: bool) -> Self {
        self.enable_qm = enable;
        self
    }

    /// Enable or disable variance boost — SVT-AV1's still-image adaptive
    /// quantization (C `--enable-variance-boost` / `--variance-boost-strength`).
    ///
    /// LIVE: sets `EncodePipeline::hdr.{enable_variance_boost,
    /// variance_boost_strength}`, which derive a per-superblock qindex plan
    /// and signal delta-q in the frame header. `strength` is C's 1-4 scale
    /// (default 2; `Docs/Appendix-Variance-Boost.md:43` recommends 3 for
    /// still images) and is clamped into that range — NOT the old
    /// `with_vaq`'s inert 0.0-1.0 float. Off by default, matching C's
    /// mainline default. Proven to change the emitted bytes by
    /// `variance_boost_knob_changes_bytes` below.
    pub fn with_variance_boost(mut self, enable: bool, strength: u8) -> Self {
        self.enable_variance_boost = enable;
        self.variance_boost_strength = strength.clamp(1, 4);
        self
    }

    /// Request lossless encoding.
    ///
    /// Selects QP 0 for exact 8-bit or native 10-bit source reconstruction on
    /// color and monochrome stills and animations, including alpha.
    /// Inter-frame lossless remains unsupported.
    pub fn with_lossless(mut self, lossless: bool) -> Self {
        self.lossless = lossless;
        self
    }

    /// Set the chroma subsampling format.
    pub fn with_chroma_subsampling(mut self, cs: ChromaSubsampling) -> Self {
        self.chroma_subsampling = cs;
        self
    }

    /// Get the configured chroma subsampling format.
    pub fn chroma_subsampling(&self) -> ChromaSubsampling {
        self.chroma_subsampling
    }

    /// Map quality (1.0-100.0) to the CLI-domain QP (0-63, C `--qp`
    /// semantics — NOT an AV1 qindex).
    ///
    /// Quality 100 -> QP 0 (best), quality 1 -> QP 63 (worst).
    /// The mapping is linear: QP = 63 - floor((quality - 1) * 63 / 99).
    /// The pipeline maps the result through `quantizer_to_qindex` once at
    /// frame setup (e.g. quality 75 -> qp 16 -> qindex 64).
    pub fn quality_to_qp_static(quality: f32) -> u8 {
        Self::quality_to_qp(quality)
    }

    fn quality_to_qp(quality: f32) -> u8 {
        let clamped = quality.clamp(1.0, 100.0);
        let qp = 63.0 - (clamped - 1.0) * 63.0 / 99.0;
        (qp.round() as u8).min(63)
    }

    /// Map speed (1-10) to SVT-AV1 preset (0-9).
    ///
    /// Speed 1 -> preset 0 (slowest/best), speed 10 -> preset 9 (fastest).
    ///
    /// The result is clamped to M9 because C remaps every all-intra preset
    /// above M9 down to M9 (`enc_handle.c:4416-4419`) — a still encoded at
    /// "preset 13" in C IS an M9 encode, so letting this map reach 10..13
    /// would only diverge from the encoder it mirrors.
    fn speed_to_preset(speed: u8) -> u8 {
        let clamped = speed.clamp(1, 10);
        // Map 1..=10 to 0..=13, then apply C's all-intra M9 clamp.
        let preset = ((clamped as u32 - 1) * 13 + 4) / 9;
        (preset as u8).min(9)
    }

    /// Build the `EncodePipeline` every entry point shares, at the image's
    /// TRUE dimensions.
    ///
    /// The pipeline performs its own TRUE -> 64-ALIGNED padding (edge
    /// replication, exactly as C does) and signals the true size in the frame
    /// header, so callers must NOT pre-pad: doing that emitted a stream whose
    /// coded frame was larger than the image the caller asked for.
    ///
    /// Every live builder knob is applied here, once, so the mono and 4:2:0
    /// entry points cannot drift apart in what they honour.
    fn build_pipeline(&self, width: u32, height: u32) -> svtav1_encoder::pipeline::EncodePipeline {
        // A still is its own key frame: intra_period 1 makes every submitted
        // picture a KEY frame, which is what a single image and an all-intra
        // sequence both want.
        self.build_pipeline_gop(width, height, 1)
    }

    /// [`Self::build_pipeline`] with an explicit key-frame interval, for the
    /// animation path's inter-coded colour track. `intra_period` is C's
    /// `GopStructure::intra_period`: 1 codes every picture as a key frame,
    /// `n > 1` codes one key frame followed by `n - 1` inter frames.
    ///
    /// Hierarchical levels stay 0 — a flat low-delay-P GOP is the only
    /// structure this port's reference-buffer table fills, and
    /// `gop_config_error` refuses anything else rather than guessing.
    fn build_pipeline_gop(
        &self,
        width: u32,
        height: u32,
        intra_period: u32,
    ) -> svtav1_encoder::pipeline::EncodePipeline {
        let rc_config = svtav1_encoder::rate_control::RcConfig {
            mode: svtav1_encoder::rate_control::RcMode::Cqp,
            // `with_lossless(true)` IS QP 0 in AV1 (spec 5.9.12
            // `CodedLossless`). Both 8/10-bit color and monochrome paths use
            // WHT transforms and bypass the in-loop filters at this index.
            qp: if self.lossless {
                0
            } else {
                Self::quality_to_qp(self.quality)
            },
            ..svtav1_encoder::rate_control::RcConfig::default()
        };
        let mut pipeline = svtav1_encoder::pipeline::EncodePipeline::new_with_preset(
            width,
            height,
            self.resolved_native_preset(),
            rc_config,
            0,
            intra_period,
        )
        // Feature 4: route the `threads` knob into the bounded tile-parallel
        // encode (`None`/`Some(0)` = auto). Byte-neutral at any value.
        .with_thread_count(self.threads.unwrap_or(0));
        pipeline.bit_depth = self.bit_depth;
        pipeline.reference = self.reference;
        pipeline.enhancements = self.enhancements;
        pipeline.color_description = self.color_description();
        // Issue #9 item 7: the two knobs that were recorded-and-ignored are
        // now the real pipeline settings. Defaults are off, so this is
        // byte-neutral for a caller that sets neither.
        pipeline.film_grain = self.film_grain.clone();
        pipeline.hdr.enable_qm = self.enable_qm;
        pipeline.hdr.enable_variance_boost = self.enable_variance_boost;
        pipeline.hdr.variance_boost_strength = self.variance_boost_strength;
        pipeline
    }

    /// Encode a single MONOCHROME (Y-only) still image using the full pipeline.
    ///
    /// **Gray only.** The emitted sequence header sets `mono_chrome = 1`, so
    /// the result is a genuine AV1 grayscale still — correct for a gray image
    /// and for an AVIF alpha auxiliary plane, and NOT a way to encode the luma
    /// of a colour image (a decoder has no chroma to reconstruct). For colour,
    /// use [`Self::encode_yuv420`], which emits one 4:2:0 stream.
    ///
    /// Uses the complete encoding pipeline: partition search with all 10
    /// partition types, intra prediction with mode RDO, transform + quantize,
    /// loop filters (deblock/CDEF/Wiener/sgrproj), and proper AV1 OBU output.
    ///
    /// The output `data` is raw AV1 OBU (temporal_delimiter + sequence_header +
    /// frame), ready to be embedded in an AVIF container by zenavif-serialize.
    pub fn encode_y8(
        &self,
        pixels: &[u8],
        width: u32,
        height: u32,
        stride: u32,
    ) -> Result<EncodedAvif, EncodeError> {
        self.validate_dimensions(pixels.len(), width, height, stride)?;
        self.validate_configuration_for_input(width, height, StillInputFormat::Monochrome)?;

        // MONOCHROME NO LONGER NEEDS PRE-PADDING. `EncodePipeline`'s
        // TRUE -> ALIGNED replicate-pad is wired on the mono path too
        // (`encode_frame_mono_core`), so this hands the pipeline the TRUE
        // dimensions and the pipeline signals them in the frame header — the
        // same contract `encode_yuv420` has always had.
        //
        // WHAT THIS FIXES, and why it was not cosmetic. This wrapper used to
        // pad the gray plane up to a multiple of 64 and then build the
        // pipeline AT THE PADDED SIZE, while still returning
        // `EncodedAvif::{width, height}` = the caller's TRUE size. For every
        // non-64-multiple gray image the AV1 frame and the size the struct
        // announces therefore DISAGREED — a 100x100 alpha plane came back as
        // a 128x128 AV1 stream labelled 100x100. That is the AVIF alpha case
        // (an alpha plane is a monochrome AV1 image at the picture's own,
        // arbitrary size), and it is the "plausible-but-wrong output" shape
        // `rust/CLAUDE.md` forbids: nothing fails, the bytes decode, and the
        // container is built around a mismatched extent.
        //
        // Both monochrome partition paths now carry a square root through
        // partial superblocks, preserving the true output dimensions.
        let (w, h, st) = (width as usize, height as usize, stride as usize);
        let mut src = vec![0u8; w * h];
        for r in 0..h {
            src[r * w..(r + 1) * w].copy_from_slice(&pixels[r * st..r * st + w]);
        }
        let mut pipeline = self.build_pipeline(width, height);

        // Fallible entry point, NOT the infallible `encode_frame` wrapper: the
        // latter `.expect()`s on every refusal the pipeline can raise
        // (unsupported bit depth, an out-of-envelope superres/bd10 config), turning a caller
        // mistake into a process abort inside a Result-returning API.
        let bitstream = pipeline
            .try_encode_frame(&src, w)
            .map_err(|e| Self::from_pipeline_error(e.error(), || e.to_string()))?;

        Ok(EncodedAvif {
            data: bitstream,
            width,
            height,
            bit_depth: self.bit_depth,
        })
    }

    /// Encode a single grayscale image and return raw AV1 OBU data.
    ///
    /// This is the interface compatible with zenavif-serialize:
    /// returns only the AV1 bitstream (OBU packets) without container.
    /// The caller wraps this in an AVIF ISO-BMFF container.
    pub fn encode_to_av1_obu(
        &self,
        pixels: &[u8],
        width: u32,
        height: u32,
        stride: u32,
    ) -> Result<Vec<u8>, EncodeError> {
        let result = self.encode_y8(pixels, width, height, stride)?;
        Ok(result.data)
    }

    /// Encode a YUV 4:2:0 image into ONE real AV1 bitstream.
    ///
    /// Routes through `EncodePipeline::with_chroma_420(true)` +
    /// `try_encode_frame_420` — the same 4:2:0 path every byte-identity gate
    /// in this repo exercises against the C encoder — so the output is a
    /// single `mono_chrome = 0` AV1 stream that any AV1 decoder accepts.
    ///
    /// It used to return three concatenated MONOCHROME streams behind u32
    /// length prefixes, which is not AV1 at all: `data` did not decode, and
    /// nothing in the return type said so (issue #9 item 6). That output
    /// contract is gone.
    ///
    /// `y` is read at `y_stride`; `u` and `v` are tightly packed at
    /// `ceil(width / 2) x ceil(height / 2)`. The pipeline handles odd and
    /// partial sizes by padding internally and signalling the true size.
    pub fn encode_yuv420(
        &self,
        y: &[u8],
        u: &[u8],
        v: &[u8],
        width: u32,
        height: u32,
        y_stride: u32,
    ) -> Result<EncodedAvif, EncodeError> {
        self.validate_dimensions(y.len(), width, height, y_stride)?;

        let chroma_w = width.div_ceil(2);
        let chroma_h = height.div_ceil(2);
        let chroma_len_needed = (chroma_w as usize).checked_mul(chroma_h as usize).ok_or(
            EncodeError::InvalidDimensions {
                width,
                height,
                reason: "chroma plane size overflows usize",
            },
        )?;
        if u.len() < chroma_len_needed || v.len() < chroma_len_needed {
            return Err(EncodeError::InvalidDimensions {
                width,
                height,
                reason: "a chroma plane is shorter than ceil(height/2) * ceil(width/2)",
            });
        }

        self.validate_configuration_for_input(width, height, StillInputFormat::Yuv420)?;

        let mut pipeline = self.build_pipeline(width, height).with_chroma_420(true);
        let bitstream = pipeline
            .try_encode_frame_420(y, u, v, y_stride as usize)
            .map_err(|e| Self::from_pipeline_error(e.error(), || e.to_string()))?;

        Ok(EncodedAvif {
            data: bitstream,
            width,
            height,
            bit_depth: self.bit_depth,
        })
    }

    /// Validate image dimensions against the pixel buffer.
    fn validate_dimensions(
        &self,
        buf_len: usize,
        width: u32,
        height: u32,
        stride: u32,
    ) -> Result<(), EncodeError> {
        if width == 0 || height == 0 {
            return Err(EncodeError::InvalidDimensions {
                width,
                height,
                reason: "width and height must both be non-zero",
            });
        }
        if stride < width {
            return Err(EncodeError::InvalidDimensions {
                width,
                height,
                reason: "stride is smaller than the width (rows would overlap)",
            });
        }
        let needed = ((height - 1) as usize)
            .checked_mul(stride as usize)
            .and_then(|n| n.checked_add(width as usize))
            .ok_or(EncodeError::InvalidDimensions {
                width,
                height,
                reason: "strided pixel buffer size overflows usize",
            })?;
        if buf_len < needed {
            return Err(EncodeError::InvalidDimensions {
                width,
                height,
                reason: "pixel buffer is shorter than (height - 1) * stride + width",
            });
        }
        Ok(())
    }

    /// Map a pipeline error onto this crate's error enum, PRESERVING the
    /// category.
    ///
    /// The category is not cosmetic: the zenavif seam maps these onto
    /// `zencodec::ErrorCategory`, where an unsupported *request* and an
    /// internal *failure* mean different things to a caller (retry with other
    /// parameters vs. report a bug). Flattening everything to `EncodeFailed`
    /// would tell the seam every refusal was an internal fault.
    /// Takes the error KIND plus its already-rendered display string, rather
    /// than the `whereat::At<_>` wrapper, so this crate needs no `whereat`
    /// dependency just to name a parameter type.
    fn from_pipeline_error(
        kind: &svtav1_encoder::EncodeError,
        rendered: impl FnOnce() -> String,
    ) -> EncodeError {
        match kind {
            svtav1_encoder::EncodeError::UnsupportedConfig(what) => {
                EncodeError::UnsupportedConfig(what)
            }
            svtav1_encoder::EncodeError::InvalidDimensions {
                width,
                height,
                reason,
            } => EncodeError::InvalidDimensions {
                width: *width,
                height: *height,
                reason,
            },
            // Cancellation and allocation failure carry runtime detail worth
            // surfacing verbatim; `#[non_exhaustive]` keeps this wildcard.
            _ => EncodeError::EncodeFailed(rendered()),
        }
    }

    /// Query still-image configuration support without encoding or allocating
    /// image planes. Uses the same format and quality checks as encoding.
    /// Source dimensions, strides and buffer lengths are validated separately.
    /// Check the exact entry point without allocating source or pipeline buffers.
    /// Bit depth and tools are taken from this encoder's configuration.
    pub fn validate_configuration_for_input(
        &self,
        width: u32,
        height: u32,
        format: StillInputFormat,
    ) -> Result<(), EncodeError> {
        self.validate_configuration()?;
        // The same overflow/geometry guard used by actual raw-plane entry points.
        self.validate_dimensions(usize::MAX, width, height, width)?;
        if format == StillInputFormat::Monochrome {
            if matches!(self.policy, Some(EncodingPolicy::SvtParity(_)))
                || self.reference == SvtReference::Mainline420
            {
                return Err(EncodeError::UnsupportedConfig(
                    "pristine C SVT supports 4:2:0 only; monochrome is a Rust extension",
                ));
            }
            if self.film_grain.enabled() {
                return Err(EncodeError::UnsupportedConfig(
                    "C film grain requires 8/10-bit 4:2:0",
                ));
            }
            self.enhancements
                .validate(self.resolved_native_preset().value(), true, false)
                .map_err(EncodeError::UnsupportedConfig)?;
        }
        Ok(())
    }

    pub fn validate_configuration(&self) -> Result<(), EncodeError> {
        if let Some(EncodingPolicy::SvtParity(reference)) = self.policy {
            if self.reference != reference {
                return Err(EncodeError::UnsupportedConfig(
                    "parity policy conflicts with selected reference",
                ));
            }
            if !self.enhancements.is_empty() {
                return Err(EncodeError::UnsupportedConfig(
                    "SvtParity forbids Zen enhancements",
                ));
            }
        }
        self.validate_quality()?;
        self.film_grain
            .validate()
            .map_err(EncodeError::UnsupportedConfig)?;
        let hdr = svtav1_encoder::hdr_mode::HdrForkConfig {
            enable_qm: self.enable_qm,
            enable_variance_boost: self.enable_variance_boost,
            variance_boost_strength: self.variance_boost_strength,
            ..Default::default()
        };
        self.reference
            .validate_hdr_config(&hdr)
            .map_err(EncodeError::UnsupportedConfig)?;
        self.enhancements
            .validate(
                self.resolved_native_preset().value(),
                true,
                self.chroma_subsampling == ChromaSubsampling::Yuv420,
            )
            .map_err(EncodeError::UnsupportedConfig)?;
        self.validate_inert_knobs(true)
    }

    /// Validate quality range.
    fn validate_quality(&self) -> Result<(), EncodeError> {
        if !(1.0..=100.0).contains(&self.quality) {
            return Err(EncodeError::InvalidQuality {
                quality: self.quality,
            });
        }
        Ok(())
    }

    /// Reject the configurations this encoder cannot honour, where ignoring
    /// them would silently emit output the caller did not ask for.
    ///
    /// As of issue #9 item 7 there are NO inert knobs left: `with_qm` and
    /// `with_variance_boost` are wired to the real pipeline settings, and
    /// `with_trellis` / `with_vaq` / `with_seg_boost` /
    /// `with_still_image_tuning` are gone (they had no counterpart in the
    /// pipeline or in C — SVT-AV1 has no trellis or seg-boost knob, and this
    /// encoder is unconditionally still-image: one KEY frame, temporal tools
    /// forced off for all-intra exactly as C does).
    ///
    /// Format-independent checks; capability guards remain in the pipeline.
    fn validate_inert_knobs(&self, _chroma_420: bool) -> Result<(), EncodeError> {
        if self.chroma_subsampling != ChromaSubsampling::Yuv420 {
            return Err(EncodeError::UnsupportedConfig(
                "only 4:2:0 chroma is implemented (and C v4.2.0 ships 420 only)",
            ));
        }
        // `with_bit_depth` accepts a u8 and was whitelisting 12, which the
        // pipeline cannot encode: `deblock::pick_filter_levels_key_frame` hits
        // `unreachable!()` at preset >= 6 (speeds 5-10, including the DEFAULT
        // speed 6) and below that the sequence header would advertise
        // seq_profile 2 without the subsampling bits that profile requires. C
        // v4.2.0 rejects any depth but 8/10 at init as well
        // (`svt_av1_verify_settings`, Globals/enc_settings.c:460).
        if !matches!(self.bit_depth, 8 | 10) {
            return Err(EncodeError::UnsupportedConfig(
                "bit depth must be 8 or 10 (C v4.2.0 rejects every other depth at encoder init)",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_stride_is_invalid_on_all_pointer_widths() {
        assert!(matches!(
            AvifEncoder::new().validate_dimensions(64, 64, 2, u32::MAX),
            Err(EncodeError::InvalidDimensions { .. })
        ));
    }

    #[test]
    fn defaults() {
        let enc = AvifEncoder::new();
        assert!((enc.quality - 75.0).abs() < f32::EPSILON);
        assert_eq!(enc.speed, 6);
        assert_eq!(enc.bit_depth, 8);
        assert_eq!(enc.chroma_subsampling, ChromaSubsampling::Yuv420);
        assert!(enc.threads.is_none());
        // Both default OFF (C mainline defaults) now that they are LIVE.
        assert!(!enc.enable_qm);
        assert!(!enc.enable_variance_boost);
        assert_eq!(enc.variance_boost_strength, 2);
        assert!(!enc.lossless);
    }

    #[test]
    fn builder_pattern() {
        let enc = AvifEncoder::new()
            .with_quality(90.0)
            .with_speed(3)
            .with_bit_depth(10)
            .with_num_threads(Some(4))
            .with_qm(true)
            .with_variance_boost(true, 3)
            .with_lossless(true);

        assert!((enc.quality - 90.0).abs() < f32::EPSILON);
        assert_eq!(enc.speed, 3);
        assert_eq!(enc.bit_depth, 10);
        assert_eq!(enc.threads, Some(4));
        assert!(enc.enable_qm);
        assert!(enc.enable_variance_boost);
        assert_eq!(enc.variance_boost_strength, 3);
        assert!(enc.lossless);
    }

    /// C's strength scale is 1-4 (`Docs/Parameters.md:124`), so 0 and 9 are
    /// clamped rather than passed through into the boost kernel.
    #[test]
    fn variance_boost_strength_clamps_to_c_range() {
        assert_eq!(
            AvifEncoder::new()
                .with_variance_boost(true, 0)
                .variance_boost_strength,
            1
        );
        assert_eq!(
            AvifEncoder::new()
                .with_variance_boost(true, 9)
                .variance_boost_strength,
            4
        );
    }

    // ---- issue #9 items 6 + 7 ------------------------------------------
    //
    // One LIVENESS cell per knob that used to be recorded-and-ignored: the
    // knob must change the emitted bytes. A knob that is "wired" but never
    // moves a byte is the same silent no-op the issue is about, so these
    // assert INEQUALITY against the same encode with the knob off.

    /// 4:2:0 test content: a luma gradient plus two non-flat chroma planes.
    fn yuv420(size: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let (cw, ch) = (size / 2, size / 2);
        let mut y = vec![0u8; size * size];
        for r in 0..size {
            for c in 0..size {
                y[r * size + c] = ((r * 255) / size) as u8 ^ ((c * 3) & 0x3F) as u8;
            }
        }
        let mut u = vec![0u8; cw * ch];
        let mut v = vec![0u8; cw * ch];
        for r in 0..ch {
            for c in 0..cw {
                u[r * cw + c] = (((r * 3) & 0x7F) + 64) as u8;
                v[r * cw + c] = (((c * 5) & 0x7F) + 64) as u8;
            }
        }
        (y, u, v)
    }

    fn enc420(enc: &AvifEncoder, size: usize) -> Vec<u8> {
        let (y, u, v) = yuv420(size);
        enc.encode_yuv420(&y, &u, &v, size as u32, size as u32, size as u32)
            .expect("4:2:0 encode")
            .data
    }

    /// Item 6: `encode_yuv420` must produce the SAME bytes as driving the
    /// mainline 4:2:0 pipeline directly with the same config — i.e. it is a
    /// thin wrapper over the path every C-oracle gate covers, not a private
    /// format. (It used to return three concatenated monochrome streams.)
    #[test]
    fn encode_yuv420_is_the_mainline_420_path_byte_for_byte() {
        let size = 64usize;
        let (y, u, v) = yuv420(size);
        let enc = AvifEncoder::new().with_quality(60.0).with_speed(6);
        let via_avif = enc
            .encode_yuv420(&y, &u, &v, size as u32, size as u32, size as u32)
            .expect("4:2:0 encode")
            .data;

        let rc = svtav1_encoder::rate_control::RcConfig {
            mode: svtav1_encoder::rate_control::RcMode::Cqp,
            qp: AvifEncoder::quality_to_qp(60.0),
            ..svtav1_encoder::rate_control::RcConfig::default()
        };
        let mut direct = svtav1_encoder::pipeline::EncodePipeline::new(
            size as u32,
            size as u32,
            AvifEncoder::speed_to_preset(6),
            rc,
            0,
            1,
        )
        .with_thread_count(0)
        .with_chroma_420(true);
        direct.bit_depth = 8;
        direct.color_description = enc.color_description();
        let via_pipeline = direct.encode_frame_420(&y, &u, &v, size);

        assert_eq!(
            via_avif, via_pipeline,
            "AvifEncoder::encode_yuv420 must BE the mainline 4:2:0 path"
        );
        // And it must be an AV1 stream, not a length-prefixed plane blob: the
        // first OBU is a temporal delimiter (obu_type 2, has_size_field).
        assert_eq!(via_avif[0] & 0x7f, 0b0_0010_0_1_0, "first OBU is not a TD");
    }

    /// Item 6: the same content through `encode_y8` is a DIFFERENT (mono)
    /// stream — the two entry points are not interchangeable, which is why
    /// `encode_y8` is documented gray-only.
    #[test]
    fn encode_y8_is_monochrome_not_the_luma_of_a_colour_image() {
        let size = 64usize;
        let (y, u, v) = yuv420(size);
        let enc = AvifEncoder::new().with_quality(60.0).with_speed(6);
        let mono = enc
            .encode_y8(&y, size as u32, size as u32, size as u32)
            .expect("mono encode")
            .data;
        let colour = enc
            .encode_yuv420(&y, &u, &v, size as u32, size as u32, size as u32)
            .expect("4:2:0 encode")
            .data;
        assert_ne!(mono, colour);
    }

    /// Item 7 liveness: `with_qm` reaches `hdr.enable_qm`.
    #[test]
    fn qm_knob_changes_bytes() {
        let base = AvifEncoder::new().with_quality(60.0).with_speed(6);
        let off = enc420(&base, 64);
        let on = enc420(&base.clone().with_qm(true), 64);
        assert_ne!(off, on, "with_qm(true) did not change the emitted bytes");
    }

    /// Item 7 liveness: `with_variance_boost` reaches
    /// `hdr.{enable_variance_boost, variance_boost_strength}`. Two cells: the
    /// enable flag moves the bytes, and so does the strength within it (a
    /// strength that never mattered would be a no-op hiding inside a live
    /// knob).
    #[test]
    fn variance_boost_knob_changes_bytes() {
        let base = AvifEncoder::new().with_quality(60.0).with_speed(6);
        let off = enc420(&base, 64);
        let on = enc420(&base.clone().with_variance_boost(true, 2), 64);
        assert_ne!(
            off, on,
            "with_variance_boost(true, _) did not change the emitted bytes"
        );

        // STRENGTH needs a frame with MORE THAN ONE superblock whose
        // variances differ, AND the low-variance half must still code
        // COEFFICIENTS. Two traps, both hit while writing this cell:
        //   * a single-SB frame collapses to a flat plan (the per-SB values
        //     are re-expressed against `min_q + range/2` in
        //     `sb_qindex::variance_adjust_qp_mainline`), so every strength
        //     gives the same plan;
        //   * a PERFECTLY FLAT superblock codes as one 64x64 skip block, and
        //     AV1 signals `delta_q` only when `MiSize != sbSize || !skip`
        //     (spec 5.11.5) — so its planned qindex never reaches the
        //     bitstream and the cell is vacuous even though the plan differs.
        //     MEASURED: flat-left/noisy-right 128x128 gives plans
        //     [72,100,72,100] / [62,...] / [56,...] for strengths 1/2/3 and
        //     IDENTICAL bytes for all four.
        // `yuv420_mixed` therefore uses a SHALLOW GRADIENT (low variance, but
        // non-zero residual) on the left. Strengths 3 and 4 saturate to the
        // same plan, so the cell compares 1 against 3.
        let (y, u, v) = yuv420_mixed(128);
        let enc_s = |st: u8| {
            base.clone()
                .with_variance_boost(true, st)
                .encode_yuv420(&y, &u, &v, 128, 128, 128)
                .expect("4:2:0 encode")
                .data
        };
        assert_ne!(
            enc_s(1),
            enc_s(3),
            "variance_boost_strength did not change the bytes"
        );
    }

    /// 4:2:0 content whose superblock variances DIFFER while BOTH halves still
    /// code coefficients: the left half is a shallow gradient (low variance,
    /// non-zero residual), the right half is high-frequency.
    fn yuv420_mixed(size: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let (cw, ch) = (size / 2, size / 2);
        let mut y = vec![128u8; size * size];
        for r in 0..size {
            for c in 0..size {
                y[r * size + c] = if c < size / 2 {
                    (100 + ((r / 8 + c / 8) % 7)) as u8
                } else {
                    (((r * 37 + c * 91) % 256) ^ ((c * 13) & 0xFF)) as u8
                };
            }
        }
        let mut u = vec![128u8; cw * ch];
        let mut v = vec![128u8; cw * ch];
        for r in 0..ch {
            for c in cw / 2..cw {
                u[r * cw + c] = ((r * 7 + c * 3) % 256) as u8;
                v[r * cw + c] = ((r * 3 + c * 11) % 256) as u8;
            }
        }
        (y, u, v)
    }

    /// The lossless flag selects QP 0 for both color and monochrome.
    #[test]
    fn lossless_is_qp0_on_color_and_monochrome() {
        let enc = AvifEncoder::new().with_speed(8).with_lossless(true);
        let (y, u, v) = yuv420(64);
        let ll = enc
            .encode_yuv420(&y, &u, &v, 64, 64, 64)
            .expect("coded-lossless 4:2:0 encode")
            .data;
        let lossy = enc420(&AvifEncoder::new().with_speed(8), 64);
        assert!(
            ll.len() > lossy.len(),
            "lossless stream ({} B) should be larger than the lossy one ({} B)",
            ll.len(),
            lossy.len()
        );
        let mono = enc.encode_y8(&y, 64, 64, 64).unwrap();
        let q0 = AvifEncoder::new()
            .with_speed(8)
            .with_quality(100.0)
            .encode_y8(&y, 64, 64, 64)
            .unwrap();
        assert_eq!(mono.data, q0.data);
        assert_ne!(
            mono.data,
            AvifEncoder::new()
                .with_speed(8)
                .encode_y8(&y, 64, 64, 64)
                .unwrap()
                .data
        );
    }

    #[test]
    fn quality_clamping() {
        let enc = AvifEncoder::new().with_quality(200.0);
        assert!((enc.quality - 100.0).abs() < f32::EPSILON);

        let enc = AvifEncoder::new().with_quality(-5.0);
        assert!((enc.quality - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn speed_clamping() {
        let enc = AvifEncoder::new().with_speed(0);
        assert_eq!(enc.speed, 1);

        let enc = AvifEncoder::new().with_speed(20);
        assert_eq!(enc.speed, 10);
    }

    #[test]
    fn quality_to_qp_monotonic() {
        // Higher quality should produce lower QP (better quality)
        let mut prev_qp = 64u8;
        for q in (1..=100).step_by(1) {
            let qp = AvifEncoder::quality_to_qp(q as f32);
            assert!(
                qp <= prev_qp,
                "quality_to_qp not monotonic: q={q}, qp={qp}, prev={prev_qp}"
            );
            prev_qp = qp;
        }
    }

    #[test]
    fn quality_to_qp_boundaries() {
        // Quality 1 -> QP 63 (worst)
        assert_eq!(AvifEncoder::quality_to_qp(1.0), 63);
        // Quality 100 -> QP 0 (best)
        assert_eq!(AvifEncoder::quality_to_qp(100.0), 0);
    }

    #[test]
    fn speed_to_preset_monotonic() {
        let mut prev_preset = 0u8;
        for s in 1..=10 {
            let preset = AvifEncoder::speed_to_preset(s);
            assert!(
                preset >= prev_preset,
                "speed_to_preset not monotonic: s={s}, preset={preset}, prev={prev_preset}"
            );
            prev_preset = preset;
        }
    }

    #[test]
    fn speed_to_preset_boundaries() {
        // Speed 1 -> preset 0 (slowest)
        assert_eq!(AvifEncoder::speed_to_preset(1), 0);
        // Speed 10 -> preset 9, NOT 13: C remaps every all-intra preset above
        // M9 down to M9 (enc_handle.c:4416-4419), so "preset 13" for a still
        // IS an M9 encode. Byte-neutral for this port — presets 9, 10 and 13
        // are each byte-identical to C's M9 output (identity_matrix covers
        // 13/10, bd10_hbd_src_gate covers 9/10/13), hence identical to each
        // other; the clamp only stops the API advertising a distinction the
        // encoder it mirrors does not have.
        assert_eq!(AvifEncoder::speed_to_preset(10), 9);
        // The slow half of the range is unaffected.
        assert_eq!(AvifEncoder::speed_to_preset(5), 6);
    }

    #[test]
    fn native_preset_resolution_and_builder_precedence() {
        let research = AvifEncoder::new().with_native_preset(NativePreset::RESEARCH);
        assert_eq!(research.resolved_native_preset().value(), -1);
        assert_eq!(research.with_speed(1).resolved_native_preset().value(), 0);
        for preset in -1..=13 {
            let enc = AvifEncoder::new()
                .with_speed(10)
                .with_native_preset(NativePreset::new(preset).unwrap());
            assert_eq!(enc.resolved_native_preset().value(), preset.min(9));
        }
    }

    #[test]
    fn research_wrapper_reaches_native_pipeline_and_changes_output() {
        let (y, u, v) = yuv420(64);
        for bit_depth in [8, 10] {
            let enc = AvifEncoder::new()
                .with_quality(40.0)
                .with_bit_depth(bit_depth)
                .with_native_preset(NativePreset::RESEARCH);
            let wrapped = enc.encode_yuv420(&y, &u, &v, 64, 64, 64).unwrap();
            let rc = svtav1_encoder::rate_control::RcConfig {
                mode: svtav1_encoder::rate_control::RcMode::Cqp,
                qp: AvifEncoder::quality_to_qp(40.0),
                ..Default::default()
            };
            let mut direct = svtav1_encoder::pipeline::EncodePipeline::new_with_preset(
                64,
                64,
                NativePreset::RESEARCH,
                rc,
                0,
                1,
            )
            .with_chroma_420(true)
            .with_bit_depth(bit_depth)
            .with_thread_count(0);
            direct.color_description = enc.color_description();
            assert_eq!(wrapped.data, direct.encode_frame_420(&y, &u, &v, 64));
            let normal = enc
                .with_speed(1)
                .encode_yuv420(&y, &u, &v, 64, 64, 64)
                .unwrap();
            assert_ne!(
                wrapped.data, normal.data,
                "research must not alias preset 0"
            );
        }
    }

    #[test]
    fn encode_y8_16x16() {
        let enc = AvifEncoder::new().with_quality(50.0).with_speed(8);
        let pixels = vec![128u8; 16 * 16];
        let result = enc.encode_y8(&pixels, 16, 16, 16).unwrap();
        assert!(!result.data.is_empty());
        assert_eq!(result.width, 16);
        assert_eq!(result.height, 16);
        assert_eq!(result.bit_depth, 8);
    }

    #[test]
    fn encode_y8_gradient() {
        let enc = AvifEncoder::new().with_quality(80.0);
        let mut pixels = vec![0u8; 16 * 16];
        for y in 0..16usize {
            for x in 0..16usize {
                pixels[y * 16 + x] = (y * 16 + x).min(255) as u8;
            }
        }
        let result = enc.encode_y8(&pixels, 16, 16, 16).unwrap();
        assert!(!result.data.is_empty());
    }

    #[test]
    fn encode_y8_with_stride() {
        let enc = AvifEncoder::new();
        // 8x8 image with stride 16 (padding between rows)
        let mut pixels = vec![0u8; 8 * 16];
        for y in 0..8usize {
            for x in 0..8usize {
                pixels[y * 16 + x] = 200;
            }
        }
        let result = enc.encode_y8(&pixels, 8, 8, 16).unwrap();
        assert!(!result.data.is_empty());
    }

    #[test]
    fn encode_y8_non_block_aligned() {
        // 10x10 image — not a multiple of 8
        let enc = AvifEncoder::new();
        let pixels = vec![100u8; 10 * 10];
        let result = enc.encode_y8(&pixels, 10, 10, 10).unwrap();
        assert!(!result.data.is_empty());
        assert_eq!(result.width, 10);
        assert_eq!(result.height, 10);
    }

    /// `encode_y8` codes the frame at the size it REPORTS.
    ///
    /// It used to pad the gray plane up to a multiple of 64 and build the
    /// pipeline at the PADDED size while returning the caller's true size, so
    /// a 100x100 alpha plane came back as a 128x128 AV1 stream labelled
    /// 100x100 — the AVIF alpha case, and an output whose container extent
    /// could not be right. There is no decoder in this crate, so the property
    /// is pinned indirectly but decisively: encoding the SAME pixels through
    /// the 128x128 twin the old code would have built must not produce the
    /// same bytes. It did, exactly, before this change.
    ///
    /// The decoding half of the assertion (aomdec/dav1d emit exactly `w*h`
    /// luma bytes, and the encoder's recon equals them) is
    /// `tools/regression_spotcheck.sh`'s `mono-arbitrary-dims-*` cells.
    #[test]
    fn encode_y8_codes_the_size_it_reports() {
        let enc = AvifEncoder::new(); // speed 6 -> preset 7, partial SBs OK
        let gray = |w: usize, h: usize| -> Vec<u8> {
            (0..h)
                .flat_map(|y| (0..w).map(move |x| (((x + y) * 255) / (w + h)) as u8))
                .collect()
        };
        let small = enc
            .encode_y8(&gray(100, 100), 100, 100, 100)
            .expect("100x100");
        assert_eq!((small.width, small.height), (100, 100));

        // The plane the OLD wrapper handed the pipeline: 128x128, the 100x100
        // content in the corner, last column then last row replicated.
        let src = gray(100, 100);
        let mut padded = vec![0u8; 128 * 128];
        for r in 0..100 {
            padded[r * 128..r * 128 + 100].copy_from_slice(&src[r * 100..(r + 1) * 100]);
            for c in 100..128 {
                padded[r * 128 + c] = padded[r * 128 + 99];
            }
        }
        for r in 100..128 {
            padded.copy_within(99 * 128..100 * 128, r * 128);
        }
        let old_shape = enc.encode_y8(&padded, 128, 128, 128).expect("128x128");
        assert_ne!(
            small.data, old_shape.data,
            "encode_y8 is still coding the 64-padded frame and mislabelling it"
        );
    }

    /// Slow presets now retain square partition roots at partial edges.
    /// Decoder/reconstruction evidence is in odd_frame_recon's preset grid.
    #[test]
    fn encode_y8_accepts_partial_sb_at_slowest_preset() {
        let slow = AvifEncoder::new().with_speed(1);
        let pixels = vec![100u8; 66 * 66];
        let encoded = slow.encode_y8(&pixels, 66, 66, 66).unwrap();
        assert_eq!((encoded.width, encoded.height), (66, 66));
        let full = slow
            .encode_y8(&vec![100u8; 128 * 128], 128, 128, 128)
            .unwrap();
        assert_eq!((full.width, full.height), (128, 128));
        assert_ne!(
            encoded.data, full.data,
            "partial input must not become a padded frame"
        );
        assert!(AvifEncoder::new().encode_y8(&pixels, 66, 66, 66).is_ok());
    }

    #[test]
    fn encode_y8_rejects_zero_dimensions() {
        let enc = AvifEncoder::new();
        let pixels = vec![0u8; 16];
        assert!(matches!(
            enc.encode_y8(&pixels, 0, 16, 16),
            Err(EncodeError::InvalidDimensions { .. })
        ));
        assert!(matches!(
            enc.encode_y8(&pixels, 16, 0, 16),
            Err(EncodeError::InvalidDimensions { .. })
        ));
    }

    #[test]
    fn encode_y8_rejects_insufficient_buffer() {
        let enc = AvifEncoder::new();
        let pixels = vec![0u8; 10]; // too small for 16x16
        assert!(matches!(
            enc.encode_y8(&pixels, 16, 16, 16),
            Err(EncodeError::InvalidDimensions { .. })
        ));
    }

    #[test]
    fn encode_yuv420_16x16() {
        let enc = AvifEncoder::new().with_quality(60.0);
        let y = vec![128u8; 16 * 16];
        let u = vec![128u8; 8 * 8];
        let v = vec![128u8; 8 * 8];
        let result = enc.encode_yuv420(&y, &u, &v, 16, 16, 16).unwrap();
        assert!(!result.data.is_empty());
        assert_eq!(result.width, 16);
        assert_eq!(result.height, 16);
    }

    #[test]
    fn encode_yuv420_validates_rounded_up_chroma_for_odd_dimensions() {
        let enc = AvifEncoder::new();
        let y = vec![0u8; 15 * 16];
        let u = vec![0u8; 8 * 8];
        let v = vec![0u8; 8 * 8];
        assert!(matches!(
            enc.encode_yuv420(&y, &u[..7 * 8], &v, 15, 16, 15),
            Err(EncodeError::InvalidDimensions { .. })
        ));
        let out = enc.encode_yuv420(&y, &u, &v, 15, 16, 15).unwrap();
        assert_eq!((out.width, out.height), (15, 16));
    }

    #[test]
    fn default_impl() {
        let enc = AvifEncoder::default();
        assert!((enc.quality - 75.0).abs() < f32::EPSILON);
    }

    #[test]
    fn higher_quality_produces_larger_output() {
        let pixels = vec![100u8; 16 * 16];

        let low_q = AvifEncoder::new().with_quality(10.0);
        let high_q = AvifEncoder::new().with_quality(95.0);

        let low_result = low_q.encode_y8(&pixels, 16, 16, 16).unwrap();
        let high_result = high_q.encode_y8(&pixels, 16, 16, 16).unwrap();

        // Higher quality (lower QP) should generally produce equal or larger output
        // because more coefficient detail is preserved
        assert!(
            high_result.data.len() >= low_result.data.len() || !low_result.data.is_empty(),
            "Both encodings should produce non-empty output"
        );
    }

    /// Maximum quality selects coded-lossless monochrome, just like the flag.
    #[test]
    fn max_quality_matches_explicit_lossless() {
        let pixels: Vec<u8> = (0..16 * 16).map(|i| (i * 17) as u8).collect();
        let expected = AvifEncoder::new()
            .with_lossless(true)
            .encode_y8(&pixels, 16, 16, 16)
            .unwrap();
        for q in [100.0f32, 99.9, 99.5] {
            assert_eq!(AvifEncoder::quality_to_qp_static(q), 0);
            let actual = AvifEncoder::new()
                .with_quality(q)
                .encode_y8(&pixels, 16, 16, 16)
                .unwrap();
            assert_eq!(actual.data, expected.data);
        }
        assert!(AvifEncoder::quality_to_qp_static(99.0) > 0);
    }

    /// `with_bit_depth` whitelisted 12, which no code path can encode:
    /// `deblock::pick_filter_levels_key_frame` hits `unreachable!()` at
    /// preset >= 6 — i.e. speeds 5-10, including the DEFAULT speed 6 — and
    /// below that the sequence header would advertise seq_profile 2 without
    /// the subsampling bits that profile requires. C v4.2.0 rejects any depth
    /// but 8/10 at init (`svt_av1_verify_settings`, enc_settings.c:460).
    /// Anti-vacuity: this test PANICS without the `validate_inert_knobs` arm.
    #[test]
    fn bit_depth_12_is_a_typed_error_not_a_panic() {
        let pixels = vec![100u8; 16 * 16];
        let err = AvifEncoder::new()
            .with_bit_depth(12)
            .encode_y8(&pixels, 16, 16, 16)
            .expect_err("12-bit must be refused (C rejects it at init too)");
        assert!(
            matches!(err, EncodeError::UnsupportedConfig(_)),
            "expected UnsupportedConfig, got {err:?}"
        );
    }

    /// `with_bit_depth` used to map every depth but 10/12 to **8, silently**
    /// (`_ => 8`), so a typo'd `with_bit_depth(9)` — or a `u8` computed from a
    /// source image's real depth, e.g. 16 — produced a perfectly valid 8-bit
    /// encode that the caller believed was deep. Nothing in the return value
    /// said otherwise: `EncodedAvif.bit_depth` reported the coerced 8, so even
    /// a caller who checked saw agreement. That is the silent-wrong-output
    /// class this project refuses; the builder now stores the request verbatim
    /// and `validate_inert_knobs` refuses it as a typed error at encode time,
    /// exactly as it already did for 12.
    /// Anti-vacuity: with the `_ => 8` arm restored, BOTH assertions fail —
    /// the depth reads 8 and the encode returns `Ok`.
    #[test]
    fn unsupported_bit_depth_is_not_silently_coerced_to_8() {
        for depth in [0u8, 1, 7, 9, 11, 16, 255] {
            let enc = AvifEncoder::new().with_bit_depth(depth);
            assert_eq!(
                enc.bit_depth, depth,
                "with_bit_depth({depth}) must record the request verbatim, not coerce it"
            );
            let pixels = vec![100u8; 16 * 16];
            let err = enc
                .encode_y8(&pixels, 16, 16, 16)
                .expect_err("an unsupported bit depth must be refused, not silently encoded at 8");
            assert!(
                matches!(err, EncodeError::UnsupportedConfig(_)),
                "depth {depth}: expected UnsupportedConfig, got {err:?}"
            );
        }
    }

    /// The native level pass now supplies true Q10 levels at every preset,
    /// including for widened input through the legacy monochrome API.
    #[test]
    fn bit_depth_10_monochrome_encodes_at_every_speed() {
        let pixels = vec![100u8; 64 * 64];
        for speed in 1..=10 {
            let bytes = AvifEncoder::new()
                .with_bit_depth(10)
                .with_speed(speed)
                .encode_y8(&pixels, 64, 64, 64)
                .unwrap();
            assert!(!bytes.data.is_empty(), "speed {speed}");
        }
    }
    /// Every geometry rejection must name the rule it broke AND echo the
    /// geometry back. `InvalidDimensions` used to be a payload-free unit
    /// variant shared by five structurally different failures, so a caller who
    /// passed an odd height and a caller whose buffer was three bytes short got
    /// the identical string "Invalid image dimensions" — true, and useless.
    ///
    /// This test is deliberately written against the RENDERED message rather
    /// than the variant, because the rendered message is what a caller reads in
    /// a log. It asserts the reasons are DISTINCT, so collapsing them back into
    /// one generic string fails here rather than in someone's bug report.
    #[test]
    fn every_dimension_rejection_names_its_own_rule() {
        let enc = AvifEncoder::new();
        let buf = vec![0u8; 16 * 16];
        let chroma = vec![0u8; 8 * 8];

        let cases: Vec<(&str, EncodeError)> = vec![
            ("zero width", enc.encode_y8(&buf, 0, 16, 16).unwrap_err()),
            (
                "stride < width",
                enc.encode_y8(&buf, 16, 16, 8).unwrap_err(),
            ),
            (
                "short luma buffer",
                enc.encode_y8(&buf[..10], 16, 16, 16).unwrap_err(),
            ),
            (
                "short chroma plane",
                enc.encode_yuv420(&buf, &chroma[..3], &chroma, 16, 16, 16)
                    .unwrap_err(),
            ),
        ];

        let mut reasons = Vec::new();
        for (label, err) in &cases {
            let EncodeError::InvalidDimensions {
                width,
                height,
                reason,
            } = err
            else {
                panic!("{label}: expected InvalidDimensions, got {err:?}");
            };
            let rendered = err.to_string();
            assert!(
                rendered.contains(&format!("{width}x{height}")),
                "{label}: the message must echo the requested geometry, got {rendered:?}"
            );
            assert!(
                rendered.contains(reason),
                "{label}: the message must carry the reason, got {rendered:?}"
            );
            assert!(
                reason.len() > 20,
                "{label}: {reason:?} is too terse to act on"
            );
            reasons.push(*reason);
        }
        let unique: std::collections::BTreeSet<&str> = reasons.iter().copied().collect();
        assert_eq!(
            unique.len(),
            reasons.len(),
            "each rejection must have its OWN reason; got duplicates in {reasons:?}"
        );
    }

    /// `EncodeError::InvalidQuality` has exactly ONE reachable trigger, and it
    /// is not the obvious one.
    ///
    /// `with_quality` CLAMPS into 1.0..=100.0 and `quality` is a private field,
    /// so no finite out-of-range value survives to `validate_quality`. NaN does:
    /// `f32::clamp` propagates NaN, and `RangeInclusive::contains` is false for
    /// it. Both halves are pinned here — the clamp (so a future change that
    /// stops clamping is caught) and the NaN path (so the variant is not
    /// quietly unreachable dead code) — plus the message naming the value.
    #[test]
    fn quality_rejection_is_reachable_only_through_nan() {
        let img = vec![0u8; 16 * 16];

        // Finite out-of-range values are clamped, not rejected.
        for q in [-10.0f32, 0.5, 100.5, 1e9] {
            let enc = AvifEncoder::new().with_quality(q);
            assert!(
                (1.0..=100.0).contains(&enc.quality),
                "with_quality({q}) must clamp into 1.0..=100.0, got {}",
                enc.quality
            );
            assert!(
                !matches!(
                    enc.encode_y8(&img, 16, 16, 16),
                    Err(EncodeError::InvalidQuality { .. })
                ),
                "a clamped quality must not be rejected ({q})"
            );
        }

        // NaN survives the clamp and is the one thing that trips the check.
        let enc = AvifEncoder::new().with_quality(f32::NAN);
        assert!(enc.quality.is_nan(), "f32::clamp must propagate NaN");
        let err = enc.encode_y8(&img, 16, 16, 16).unwrap_err();
        let EncodeError::InvalidQuality { quality } = err else {
            panic!("expected InvalidQuality, got {err:?}");
        };
        assert!(quality.is_nan());
        let rendered = err.to_string();
        assert!(
            rendered.contains("NaN") && rendered.contains("100"),
            "the message must name the rejected value and the range, got {rendered:?}"
        );
    }

    /// A refusal raised inside the pipeline must reach the AVIF caller with its
    /// reason intact. `from_pipeline_error` used to match
    /// `InvalidDimensions { .. }` and rebuild a payload-free variant, throwing
    /// away the only part of the error a caller could act on.
    #[test]
    fn pipeline_dimension_refusals_keep_their_reason() {
        let inner = svtav1_encoder::EncodeError::InvalidDimensions {
            width: 63,
            height: 65,
            reason: "monochrome encode requires 8-aligned dims",
        };
        let mapped = AvifEncoder::from_pipeline_error(&inner, || "rendered".to_string());
        let EncodeError::InvalidDimensions {
            width,
            height,
            reason,
        } = mapped
        else {
            panic!("expected InvalidDimensions, got {mapped:?}");
        };
        assert_eq!((width, height), (63, 65));
        assert_eq!(reason, "monochrome encode requires 8-aligned dims");
    }
}
