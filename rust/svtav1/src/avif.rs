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
/// `__expert`: fixed per-plane chroma delta-q; see
/// [`AvifEncoder::with_chroma_q_override`].
#[cfg(feature = "__expert")]
pub use svtav1_encoder::chroma_q::ChromaQOverride;
/// Explicit, uncalibrated Zen experiments for the region beyond native −1.
pub use svtav1_encoder::enhancements::{ZenEnhancement, ZenEnhancements};
pub use svtav1_encoder::fork_config::ForkConfig;
/// Pinned C source identity, separate from speed and policy.
pub use svtav1_encoder::reference::SvtReference;
/// Checked C preset domain, including research -1.
pub use svtav1_encoder::speed_config::NativePreset;
/// C `--tune` bundles (`--tune 0..4`) for the still/allintra path.
pub use svtav1_encoder::tune::SvtTune;

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
    /// The cooperative stop token installed via [`AvifEncoder::with_stop`] /
    /// [`AvifEncoder::with_timeout`] fired mid-encode. The payload keeps
    /// `TimedOut` distinguishable from an explicit `Cancelled` — the
    /// difference between "the caller's budget expired" and "the caller
    /// aborted", which a watchdog needs to name a wedged encode.
    Cancelled(enough::StopReason),
}

impl core::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnsupportedConfig(what) => write!(f, "Unsupported configuration: {what}"),
            Self::Cancelled(reason) => write!(f, "Encode stopped: {reason}"),
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
    /// Fork and mainline knobs by their C names ([`AvifEncoder::with_fork`],
    /// [`AvifEncoder::with_qm`], [`AvifEncoder::with_variance_boost`]). Unset
    /// fields keep the selected reference's own defaults (for example QM and
    /// variance boost: off under mainline, on under Ghost Robot).
    fork: ForkConfig,
    /// C `static_config.tune` (`--tune`) — the per-tune configuration
    /// bundle C applies in `svt_av1_enc_set_parameter`. Wired to
    /// `EncodePipeline::hdr.tune`; see [`AvifEncoder::with_tune`].
    tune: SvtTune,
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
    /// Cooperative cancellation token forwarded to every `EncodePipeline`
    /// this encoder builds; see [`Self::with_stop`].
    stop: Option<almost_enough::StopToken>,
    /// `__expert`: see [`Self::with_chroma_q_override`].
    #[cfg(feature = "__expert")]
    chroma_q_override: Option<ChromaQOverride>,
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
            reference: SvtReference::Mainline420,
            enhancements: ZenEnhancements::default(),
            bit_depth: 8,
            chroma_subsampling: ChromaSubsampling::Yuv420,
            threads: None,
            // Empty: the selected reference's own defaults apply
            // (`HdrForkConfig::defaults_for`), so no hard default here can
            // silently override them.
            fork: ForkConfig::default(),
            tune: SvtTune::Psnr,
            film_grain: Default::default(),
            lossless: false,
            color_primaries: 1,           // BT.709
            transfer_characteristics: 13, // sRGB
            matrix_coefficients: 1,       // BT.709
            full_range: false,
            stop: None,
            #[cfg(feature = "__expert")]
            chroma_q_override: None,
        }
    }

    /// `__expert`: replace the derived chroma delta-q with fixed per-plane
    /// qindex deltas for U (Cb) and V (Cr). Each is clamped to `[-64, 63]`
    /// and applied to both the DC and AC quantizer of its plane; positive is
    /// coarser. `u != v` signals `separate_uv_delta_q = 1`.
    ///
    /// For decorrelated-plane research stimuli (chroma much coarser or finer
    /// than luma); not a quality knob, and no C-parity claim. Monochrome
    /// encodes with an override set are refused with an explicit error.
    #[cfg(feature = "__expert")]
    pub fn with_chroma_q_override(mut self, u: i8, v: i8) -> Self {
        self.chroma_q_override = Some(ChromaQOverride::new(u, v));
        self
    }

    /// Install a cooperative cancellation token, forwarded to every
    /// [`EncodePipeline`](svtav1_encoder::pipeline::EncodePipeline) this
    /// encoder builds — stills and every animation frame. It is checked at
    /// each frame's entry and at the post-filter checkpoints; when it fires
    /// the encode returns [`EncodeError::Cancelled`] carrying the
    /// [`enough::StopReason`].
    ///
    /// This is the caller's DoS bound: an encode over untrusted input that
    /// runs past its budget returns an error instead of spinning. See
    /// [`Self::with_timeout`] for the common deadline case.
    pub fn with_stop(mut self, stop: impl enough::Stop + 'static) -> Self {
        self.stop = Some(almost_enough::StopToken::new(stop));
        self
    }

    /// Bound every encode with a wall-clock deadline measured from this call —
    /// [`Self::with_stop`] pre-loaded with an `almost_enough::WithTimeout`.
    /// Expiry reports [`enough::StopReason::TimedOut`].
    pub fn with_timeout(self, budget: core::time::Duration) -> Self {
        self.with_stop(almost_enough::WithTimeout::new(enough::Unstoppable, budget))
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
    /// Maps internally to AV1 QP 63 (worst) to 0 (best). A value outside
    /// 1.0-100.0 (or NaN) is refused by validation, not clamped.
    pub fn with_quality(mut self, quality: f32) -> Self {
        self.quality = quality;
        self
    }

    /// Set the speed preset (1-10).
    ///
    /// Maps to still-image presets: 1 -> preset 0, 10 -> preset 9. A value
    /// outside 1-10 is refused by validation, not clamped.
    /// Replaces an earlier [`Self::with_native_preset`] selection.
    pub fn with_speed(mut self, speed: u8) -> Self {
        self.speed = speed;
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

    /// Select a pinned C source. The default is Mainline420 (pristine v4.2.0).
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
    /// `None` auto-detects based on available cores. Threading is
    /// tile-parallel only: an explicit `Some(n > 1)` derives a tile grid —
    /// `n` rounds up to a power of two and splits as evenly as possible
    /// across columns and rows (4 -> 2x2, 8 -> 4x2), clamped to what the
    /// frame can tile — and the bounded wave executor runs that grid on up
    /// to `n` threads. Tiles are coded independently, so **the output bytes
    /// depend on the grid**: `Some(4)` does not emit the same stream as
    /// `Some(1)`. The grid is a pure function of `n`, so a given count is
    /// byte-reproducible on any machine.
    ///
    /// `None`, `Some(0)` and `Some(1)` keep the default grid — a single
    /// tile unless the frame geometry forces tiling (width > 4096 px or
    /// area above `MAX_TILE_AREA`) — where the auto-detected count still
    /// bounds the parallel wave without changing bytes.
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
    /// header's `using_qmatrix` + qm levels and the quantizer itself. Unset,
    /// the selected reference's default applies (off under mainline,
    /// on under Ghost Robot). Proven to change the emitted
    /// bytes by `qm_knob_changes_bytes` below. Lossless encoding uses identity
    /// matrices, as required by the decoder's lossless reconstruction.
    pub fn with_qm(mut self, enable: bool) -> Self {
        self.fork.enable_qm = Some(enable);
        self
    }

    /// Enable or disable variance boost — SVT-AV1's still-image adaptive
    /// quantization (C `--enable-variance-boost` / `--variance-boost-strength`).
    ///
    /// LIVE: sets `EncodePipeline::hdr.{enable_variance_boost,
    /// variance_boost_strength}`, which derive a per-superblock qindex plan
    /// and signal delta-q in the frame header. `strength` is C's 1-4 scale
    /// (default 2; `Docs/Appendix-Variance-Boost.md:43` recommends 3 for
    /// still images); a value outside it is refused when the encoder is
    /// validated, as C refuses it, not clamped. Unset, the selected reference's
    /// default applies (off under mainline, on under Ghost Robot). Proven
    /// to change the emitted bytes by `variance_boost_knob_changes_bytes`
    /// below.
    pub fn with_variance_boost(mut self, enable: bool, strength: u8) -> Self {
        self.fork.enable_variance_boost = Some(enable);
        self.fork.variance_boost_strength = Some(strength);
        self
    }

    /// Override fork and mainline knobs by their C names. Each field `fork`
    /// sets replaces any earlier setting of it (including by
    /// [`Self::with_qm`] and [`Self::with_variance_boost`]); unset fields
    /// keep their current value. Values are passed through unclamped:
    /// [`Self::validate_configuration`] refuses what C's
    /// `svt_av1_verify_settings` refuses, and what the port has not
    /// implemented for the selected reference. `tune` is set with
    /// [`Self::with_tune`].
    pub fn with_fork(mut self, fork: ForkConfig) -> Self {
        self.fork = self.fork.merged(fork);
        self
    }

    /// The knob overrides set so far.
    pub fn fork(&self) -> ForkConfig {
        self.fork
    }

    /// Select C's `--tune` bundle (C `static_config.tune`,
    /// `svt_av1_enc_set_parameter` / `apply_tune_overrides`).
    ///
    /// LIVE: sets `EncodePipeline::hdr.tune`. [`SvtTune::Iq`] is the
    /// still-image tune — QM on, sharpness 7, variance boost strength 3 /
    /// curve 2, `max_tx_size` 32 at qp<=45, screen detection forced on —
    /// and is measured BD-positive vs the default on photographic content
    /// at no encode-time cost. The tune's bundle overrides the individual
    /// `with_qm`/`with_variance_boost` settings where they disagree,
    /// matching C's apply order. Default [`SvtTune::Psnr`] is byte-unchanged.
    ///
    /// Byte-parity vs C is per-variant (see [`SvtTune`]); the
    /// SSIM-rdmult-bearing tunes (`Ssim`, `Iq`, `MsSsim`) carry pinned
    /// decision-level divergences on part of the gate grid while remaining
    /// decoder-valid.
    pub fn with_tune(mut self, tune: SvtTune) -> Self {
        self.tune = tune;
        self
    }

    /// The configured `--tune` bundle.
    pub fn tune(&self) -> SvtTune {
        self.tune
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
    /// `n > 1` codes one key frame followed by `n - 1` inter frames, and 0
    /// codes a single key frame followed by inter frames for the rest of
    /// the sequence — the low-delay equivalent of C's `intra_period_length`
    /// beyond the clip end (byte-verified against C on flat-LDP cells).
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
        // encode (`None`/`Some(0)` = auto-detect). Byte-neutral where the
        // grid is left at its default — see the tile derivation below.
        .with_thread_count(self.threads.unwrap_or(0));
        // Issue #24: the wave executor only engages when `num_tiles() > 1`,
        // which without a request happens only at forced-tile geometry
        // (width > MAX_TILE_WIDTH or area > MAX_TILE_AREA) — so an explicit
        // thread count was inert for every ordinary AVIF size. `Some(n > 1)`
        // now derives a grid targeting `n` tiles: `n` rounds up to a power
        // of two, split as evenly as possible across columns and rows
        // (4 -> 2x2, 8 -> 4x2), and `TileGrid::resolve` clamps the request
        // to what the frame can tile. The grid is a pure function of `n` —
        // deterministic bytes per count, machine-independent — while `None`,
        // `Some(0)` and `Some(1)` leave the default grid so auto-detection
        // can never make output depend on the host's core count.
        if let Some(n) = self.threads {
            if n > 1 {
                let total_log2 = (usize::BITS - (n - 1).leading_zeros()) as u8;
                let cols_log2 = total_log2.div_ceil(2);
                pipeline = pipeline
                    .with_tile_cols_log2(cols_log2)
                    .with_tile_rows_log2(total_log2 - cols_log2);
            }
        }
        if let Some(stop) = &self.stop {
            pipeline = pipeline.with_stop(stop.clone());
        }
        pipeline.bit_depth = self.bit_depth;
        pipeline.reference = self.reference;
        pipeline.enhancements = self.enhancements;
        #[cfg(feature = "__expert")]
        {
            pipeline.chroma_q_override = self.chroma_q_override;
        }
        pipeline.color_description = self.color_description();
        // Issue #9 item 7: the two knobs that were recorded-and-ignored are
        // now the real pipeline settings. Unset, the reference's own
        // `svt_av1_set_default_params` defaults apply — off under mainline
        // (byte-neutral), on under Ghost Robot.
        pipeline.hdr = self.resolved_hdr();
        pipeline.film_grain = self.film_grain.clone();
        pipeline
    }

    /// The reference's defaults (Ghost Robot is HdrFork-only) with the
    /// caller's overrides and tune applied — what both the encode and
    /// [`Self::validate_configuration`] use.
    fn resolved_hdr(&self) -> svtav1_encoder::hdr_mode::HdrForkConfig {
        let mut hdr = svtav1_encoder::hdr_mode::HdrForkConfig::defaults_for(
            self.reference,
            self.implied_hdr_mode(),
        );
        self.fork.apply(&mut hdr);
        hdr.tune = self.tune.to_raw();
        hdr
    }

    /// The HDR mode this facade's pipeline runs in. Ghost Robot has no
    /// mainline mode (`SvtReference::validate_hdr_config`), so that
    /// reference implies `HdrFork`; every other reference keeps the
    /// facade's historical mainline base.
    fn implied_hdr_mode(&self) -> svtav1_encoder::hdr_mode::SvtHdrMode {
        match self.reference {
            SvtReference::GhostRobot => svtav1_encoder::hdr_mode::SvtHdrMode::HdrFork,
            _ => svtav1_encoder::hdr_mode::SvtHdrMode::Mainline,
        }
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
            svtav1_encoder::EncodeError::Cancelled(reason) => EncodeError::Cancelled(*reason),
            // Allocation failure carries runtime detail worth surfacing
            // verbatim; `#[non_exhaustive]` keeps this wildcard.
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
            // Monochrome is a Rust extension with no C claim; the reference
            // still selects which C decisions its shared parts follow. Only
            // the parity policy, which promises C's envelope, refuses it.
            if matches!(self.policy, Some(EncodingPolicy::SvtParity(_))) {
                return Err(EncodeError::UnsupportedConfig(
                    "SvtParity: C SVT supports 4:2:0 only; monochrome is a Rust extension",
                ));
            }
            if self.film_grain.enabled() {
                return Err(EncodeError::UnsupportedConfig(
                    "C film grain requires 8/10-bit 4:2:0",
                ));
            }
            self.enhancements
                .validate(
                    self.resolved_native_preset().value(),
                    true,
                    false,
                    self.bit_depth,
                )
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
        // The same resolved config the encode will use.
        self.reference
            .validate_hdr_config(&self.resolved_hdr())
            .map_err(EncodeError::UnsupportedConfig)?;
        self.enhancements
            .validate(
                self.resolved_native_preset().value(),
                true,
                self.chroma_subsampling == ChromaSubsampling::Yuv420,
                self.bit_depth,
            )
            .map_err(EncodeError::UnsupportedConfig)?;
        self.validate_inert_knobs(true)
    }

    /// Validate quality range.
    /// Quality and speed are range-checked here, not clamped by their
    /// setters, so an out-of-range value is an error the caller sees.
    fn validate_quality(&self) -> Result<(), EncodeError> {
        if !(1.0..=100.0).contains(&self.quality) {
            return Err(EncodeError::InvalidQuality {
                quality: self.quality,
            });
        }
        if !(1..=10).contains(&self.speed) {
            return Err(EncodeError::UnsupportedConfig(
                "speed must be between 1 and 10 (use with_native_preset for C's preset scale)",
            ));
        }
        Ok(())
    }

    /// Reject the configurations this encoder cannot honour, where ignoring
    /// them would silently emit output the caller did not ask for.
    ///
    /// As of issue #9 item 7 the recorded-and-ignored knobs are gone:
    /// `with_qm` and `with_variance_boost` are wired to the real pipeline
    /// settings, and `with_trellis` / `with_vaq` / `with_seg_boost` /
    /// `with_still_image_tuning` were removed (they had no counterpart in
    /// the pipeline or in C — SVT-AV1 has no trellis or seg-boost knob, and
    /// this encoder is unconditionally still-image: one KEY frame, temporal
    /// tools forced off for all-intra exactly as C does).
    ///
    /// `with_num_threads` is the partial exception (issue #24): it is live
    /// only because `Some(n > 1)` derives a tile grid — a byte-changing,
    /// opt-in behavior, documented on the knob. `None`/`Some(0)`/`Some(1)`
    /// affect execution solely where frame geometry already forces tiles.
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
mod tests;
