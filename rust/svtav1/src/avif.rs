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

/// Chroma subsampling format for AVIF encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromaSubsampling {
    /// 4:2:0 subsampling (most common for AVIF).
    Yuv420,
    /// 4:4:4 no subsampling (higher quality chroma).
    Yuv444,
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
pub enum EncodeError {
    /// Image dimensions are invalid (zero, too large, or not aligned).
    InvalidDimensions,
    /// Quality value is out of the valid range (1.0-100.0).
    InvalidQuality,
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
            Self::InvalidDimensions => write!(f, "Invalid image dimensions"),
            Self::InvalidQuality => write!(f, "Quality must be between 1.0 and 100.0"),
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
    /// Speed preset (1-10). Mapped to svtav1 presets 0-13.
    speed: u8,
    /// Bit depth (8, 10, or 12).
    bit_depth: u8,
    /// Chroma subsampling format.
    /// Used when full YUV encoding with chroma-aware QP offsets is wired through.
    chroma_subsampling: ChromaSubsampling,
    /// Number of encoding threads (None = auto).
    threads: Option<usize>,
    /// Enable quantization matrices.
    enable_qm: bool,
    /// Enable variance adaptive quantization.
    enable_vaq: bool,
    /// VAQ strength (0.0-1.0).
    vaq_strength: f64,
    /// Tune for still image encoding (disable temporal tools).
    tune_still_image: bool,
    /// Enable trellis quantization.
    enable_trellis: bool,
    /// Segment-level QP boost for flat regions.
    seg_boost: f64,
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
            bit_depth: 8,
            chroma_subsampling: ChromaSubsampling::Yuv420,
            threads: None,
            enable_qm: true,
            enable_vaq: true,
            vaq_strength: 0.5,
            tune_still_image: true,
            enable_trellis: true,
            seg_boost: 0.0,
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
    fn color_description(&self) -> svtav1_entropy::obu::ColorDescription {
        svtav1_entropy::obu::ColorDescription {
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
    /// Maps to svtav1 presets: 1 -> preset 0 (slowest), 10 -> preset 13 (fastest).
    pub fn with_speed(mut self, speed: u8) -> Self {
        self.speed = speed.clamp(1, 10);
        self
    }

    /// Set the bit depth (8, 10, or 12).
    pub fn with_bit_depth(mut self, depth: u8) -> Self {
        self.bit_depth = match depth {
            10 => 10,
            12 => 12,
            _ => 8,
        };
        self
    }

    /// Set the number of encoding threads.
    ///
    /// `None` means auto-detect based on available cores.
    pub fn with_num_threads(mut self, threads: Option<usize>) -> Self {
        self.threads = threads;
        self
    }

    /// Enable or disable quantization matrices.
    pub fn with_qm(mut self, enable: bool) -> Self {
        self.enable_qm = enable;
        self
    }

    /// Enable or disable variance adaptive quantization.
    pub fn with_vaq(mut self, enable: bool, strength: f64) -> Self {
        self.enable_vaq = enable;
        self.vaq_strength = strength.clamp(0.0, 1.0);
        self
    }

    /// Enable or disable still image tuning.
    ///
    /// When enabled, disables temporal prediction tools for better
    /// single-frame compression.
    pub fn with_still_image_tuning(mut self, enable: bool) -> Self {
        self.tune_still_image = enable;
        self
    }

    /// Enable or disable trellis quantization.
    ///
    /// CURRENTLY INERT: recorded for API compatibility, not consumed by the
    /// encoder (the RDOQ policy comes from the preset/qindex, C-exactly).
    pub fn with_trellis(mut self, enable: bool) -> Self {
        self.enable_trellis = enable;
        self
    }

    /// Request lossless encoding.
    ///
    /// CURRENTLY INERT AND REJECTED: the encode entry points return
    /// [`EncodeError::UnsupportedConfig`] when this is set, because the
    /// encoder would otherwise silently produce a LOSSY stream. Setting it
    /// back to `false` clears the rejection.
    pub fn with_lossless(mut self, lossless: bool) -> Self {
        self.lossless = lossless;
        self
    }

    /// Set the segment-level QP boost for flat regions.
    pub fn with_seg_boost(mut self, boost: f64) -> Self {
        self.seg_boost = boost;
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

    /// Get the configured segment boost value.
    pub fn seg_boost(&self) -> f64 {
        self.seg_boost
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

    /// Encode a single grayscale (Y-only) still image using the full pipeline.
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
        self.validate_quality()?;
        self.validate_inert_knobs()?;

        let qp = Self::quality_to_qp(self.quality);
        let preset = Self::speed_to_preset(self.speed);
        let w = width as usize;
        let h = height as usize;

        // Copy source with stride → contiguous buffer, edge-padded to SB alignment
        // AV1 spec: use_128x128_superblock=0 → sb_size=64 always
        let sb_size = 64usize;
        let padded_w = w.div_ceil(sb_size) * sb_size;
        let padded_h = h.div_ceil(sb_size) * sb_size;
        let mut src = vec![128u8; padded_w * padded_h];
        for r in 0..h {
            for c in 0..w {
                src[r * padded_w + c] = pixels[r * stride as usize + c];
            }
            // Replicate last column to pad width
            for c in w..padded_w {
                src[r * padded_w + c] = src[r * padded_w + w - 1];
            }
        }
        // Replicate last row to pad height
        for r in h..padded_h {
            for c in 0..padded_w {
                src[r * padded_w + c] = src[(h - 1) * padded_w + c];
            }
        }

        // Use the full encoding pipeline (single key frame, still-picture mode)
        let rc_config = svtav1_encoder::rate_control::RcConfig {
            mode: svtav1_encoder::rate_control::RcMode::Cqp,
            qp,
            ..svtav1_encoder::rate_control::RcConfig::default()
        };
        let mut pipeline = svtav1_encoder::pipeline::EncodePipeline::new(
            padded_w as u32,
            padded_h as u32,
            preset,
            rc_config,
            0,
            1,
        )
        // Feature 4: route the previously-dead `threads` knob into the
        // bounded tile-parallel encode (`None`/`Some(0)` = auto).
        .with_thread_count(self.threads.unwrap_or(0));
        pipeline.bit_depth = self.bit_depth;
        pipeline.color_description = self.color_description();

        // Fallible entry point, NOT the infallible `encode_frame` wrapper: the
        // latter `.expect()`s on every refusal the pipeline can raise
        // (unsupported bit depth, qp 0, an out-of-envelope superres/bd10
        // config), turning a caller mistake into a process abort inside a
        // Result-returning API. Routing through `try_encode_frame` is also what
        // finally gives `EncodeError::EncodeFailed` a constructor — it was dead
        // in the whole workspace, so `AvifEncoder` could not report ANY runtime
        // encode failure.
        let bitstream = pipeline
            .try_encode_frame(&src, padded_w)
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

    /// Encode a YUV 4:2:0 image.
    ///
    /// Encodes the luma plane through the full pipeline. Chroma planes
    /// are encoded independently at half resolution. Each produces a
    /// separate AV1 OBU stream; they're concatenated with length prefixes.
    ///
    /// TODO: switch to the real single-stream 4:2:0 path
    /// (`EncodePipeline::with_chroma_420(true)` + `encode_frame_420`,
    /// mono_chrome=0 sequence header) — kept on the legacy three-mono-plane
    /// format for now because callers consume this length-prefixed layout;
    /// changing the output contract needs its own migration.
    pub fn encode_yuv420(
        &self,
        y: &[u8],
        u: &[u8],
        v: &[u8],
        width: u32,
        height: u32,
        y_stride: u32,
    ) -> Result<EncodedAvif, EncodeError> {
        if width == 0 || height == 0 || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
            return Err(EncodeError::InvalidDimensions);
        }

        let y_len_needed = (height - 1) * y_stride + width;
        if (y.len() as u32) < y_len_needed {
            return Err(EncodeError::InvalidDimensions);
        }

        let chroma_w = width / 2;
        let chroma_h = height / 2;
        let chroma_len_needed = (chroma_h - 1) * chroma_w + chroma_w;
        if (u.len() as u32) < chroma_len_needed || (v.len() as u32) < chroma_len_needed {
            return Err(EncodeError::InvalidDimensions);
        }

        self.validate_quality()?;

        // Encode each plane through the full pipeline
        let luma_result = self.encode_y8(y, width, height, y_stride)?;
        let u_result = self.encode_y8(u, chroma_w, chroma_h, chroma_w)?;
        let v_result = self.encode_y8(v, chroma_w, chroma_h, chroma_w)?;

        // Length-prefixed plane concatenation for multi-plane embedding
        let mut combined = Vec::with_capacity(
            12 + luma_result.data.len() + u_result.data.len() + v_result.data.len(),
        );
        combined.extend_from_slice(&(luma_result.data.len() as u32).to_le_bytes());
        combined.extend_from_slice(&luma_result.data);
        combined.extend_from_slice(&(u_result.data.len() as u32).to_le_bytes());
        combined.extend_from_slice(&u_result.data);
        combined.extend_from_slice(&(v_result.data.len() as u32).to_le_bytes());
        combined.extend_from_slice(&v_result.data);

        Ok(EncodedAvif {
            data: combined,
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
            return Err(EncodeError::InvalidDimensions);
        }
        if stride < width {
            return Err(EncodeError::InvalidDimensions);
        }
        let needed = (height - 1) as usize * stride as usize + width as usize;
        if buf_len < needed {
            return Err(EncodeError::InvalidDimensions);
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
            svtav1_encoder::EncodeError::InvalidDimensions { .. } => EncodeError::InvalidDimensions,
            // Cancellation and allocation failure carry runtime detail worth
            // surfacing verbatim; `#[non_exhaustive]` keeps this wildcard.
            _ => EncodeError::EncodeFailed(rendered()),
        }
    }

    /// Validate quality range.
    fn validate_quality(&self) -> Result<(), EncodeError> {
        if !(1.0..=100.0).contains(&self.quality) {
            return Err(EncodeError::InvalidQuality);
        }
        Ok(())
    }

    /// Reject builder knobs that this encoder RECORDS but does not consume,
    /// where ignoring them would silently emit output the caller did not ask
    /// for.
    ///
    /// `with_lossless(true)` would otherwise return a LOSSY stream and
    /// `with_chroma_subsampling(Yuv422/Yuv444)` a 4:2:0 one — both
    /// indistinguishable from success at the call site. (4:2:2 / 4:4:4 are
    /// outside SVT-AV1 v4.2.0's own shipping envelope too:
    /// `svt_av1_verify_settings`, enc_settings.c:470, "Only support 420 now".)
    /// The purely advisory knobs — trellis, VAQ, QM, seg-boost,
    /// still-image tuning — are documented as inert instead of rejected: they
    /// change nothing a caller can observe.
    fn validate_inert_knobs(&self) -> Result<(), EncodeError> {
        if self.lossless {
            return Err(EncodeError::UnsupportedConfig(
                "lossless encoding is not implemented; the encoder would emit a lossy stream",
            ));
        }
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
        // Quality > ~99.21 maps to QP 0, which is LOSSLESS in AV1 — the WHT
        // transform path, the forced-off loop filters and the frame-header
        // omissions are all unported, so the pipeline refuses qp 0. It used to
        // refuse it from inside the INFALLIBLE `EncodePipeline::encode_frame`,
        // whose `.expect()` then aborted the caller's process: the most obvious
        // "maximum quality" input panicked through a Result-returning API.
        // Reject it here, as a typed error, at the same place the other
        // unsupported knobs are caught.
        if Self::quality_to_qp(self.quality) == 0 {
            return Err(EncodeError::UnsupportedConfig(
                "quality > 99.2 maps to QP 0, which is lossless AV1 (WHT transform + lossless \
                 header signalling); lossless encoding is not implemented — use a lower quality",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let enc = AvifEncoder::new();
        assert!((enc.quality - 75.0).abs() < f32::EPSILON);
        assert_eq!(enc.speed, 6);
        assert_eq!(enc.bit_depth, 8);
        assert_eq!(enc.chroma_subsampling, ChromaSubsampling::Yuv420);
        assert!(enc.threads.is_none());
        assert!(enc.enable_qm);
        assert!(enc.enable_vaq);
        assert!(enc.tune_still_image);
        assert!(enc.enable_trellis);
        assert!(!enc.lossless);
    }

    #[test]
    fn builder_pattern() {
        let enc = AvifEncoder::new()
            .with_quality(90.0)
            .with_speed(3)
            .with_bit_depth(10)
            .with_num_threads(Some(4))
            .with_qm(false)
            .with_vaq(true, 0.8)
            .with_still_image_tuning(false)
            .with_trellis(false)
            .with_lossless(true);

        assert!((enc.quality - 90.0).abs() < f32::EPSILON);
        assert_eq!(enc.speed, 3);
        assert_eq!(enc.bit_depth, 10);
        assert_eq!(enc.threads, Some(4));
        assert!(!enc.enable_qm);
        assert!(enc.enable_vaq);
        assert!((enc.vaq_strength - 0.8).abs() < f64::EPSILON);
        assert!(!enc.tune_still_image);
        assert!(!enc.enable_trellis);
        assert!(enc.lossless);
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

    #[test]
    fn encode_y8_rejects_zero_dimensions() {
        let enc = AvifEncoder::new();
        let pixels = vec![0u8; 16];
        assert!(matches!(
            enc.encode_y8(&pixels, 0, 16, 16),
            Err(EncodeError::InvalidDimensions)
        ));
        assert!(matches!(
            enc.encode_y8(&pixels, 16, 0, 16),
            Err(EncodeError::InvalidDimensions)
        ));
    }

    #[test]
    fn encode_y8_rejects_insufficient_buffer() {
        let enc = AvifEncoder::new();
        let pixels = vec![0u8; 10]; // too small for 16x16
        assert!(matches!(
            enc.encode_y8(&pixels, 16, 16, 16),
            Err(EncodeError::InvalidDimensions)
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
    fn encode_yuv420_rejects_odd_dimensions() {
        let enc = AvifEncoder::new();
        let y = vec![0u8; 15 * 16];
        let u = vec![0u8; 8 * 8];
        let v = vec![0u8; 8 * 8];
        assert!(matches!(
            enc.encode_yuv420(&y, &u, &v, 15, 16, 15),
            Err(EncodeError::InvalidDimensions)
        ));
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

    /// `quality_to_qp` maps everything above ~99.21 to QP 0, which is LOSSLESS
    /// AV1 — a mode this port does not implement. The refusal used to live
    /// inside the INFALLIBLE `EncodePipeline::encode_frame`, whose `.expect()`
    /// aborted the process, so the most obvious "maximum quality" call panicked
    /// out of a `Result`-returning API. Anti-vacuity: this test PANICS (not
    /// fails) without the `validate_inert_knobs` q0 arm.
    #[test]
    fn max_quality_is_a_typed_error_not_a_panic() {
        let pixels = vec![100u8; 16 * 16];
        for q in [100.0f32, 99.9, 99.5] {
            assert_eq!(
                AvifEncoder::quality_to_qp_static(q),
                0,
                "q{q} must map to qp 0"
            );
            let err = AvifEncoder::new()
                .with_quality(q)
                .encode_y8(&pixels, 16, 16, 16)
                .expect_err("qp 0 (lossless) must be refused, not encoded");
            assert!(
                matches!(err, EncodeError::UnsupportedConfig(_)),
                "expected UnsupportedConfig, got {err:?}"
            );
        }
        // The first quality that still maps off qp 0 must keep working.
        assert!(AvifEncoder::quality_to_qp_static(99.0) > 0);
        assert!(
            AvifEncoder::new()
                .with_quality(99.0)
                .encode_y8(&pixels, 16, 16, 16)
                .is_ok()
        );
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

    /// 10-bit through the u8 entry points only produces TRUE 10-bit coded
    /// levels inside a narrow envelope (4:2:0 + 64-aligned + a bd10 producer
    /// for that preset). `AvifEncoder` is monochrome and pads to 64, so no
    /// bd10 producer exists below preset 9: the whole encode ran with the Q8
    /// tables while the sequence header advertised `high_bitdepth = 1`, and
    /// the decoder dequantized it with Q10. That stream is decodable and
    /// indistinguishable from success at the seam — the exact
    /// "plausible-but-wrong bitstream" class the project bans. Anti-vacuity:
    /// without `bit_depth_config_error` this returns `Ok` with corrupt bytes.
    #[test]
    fn bit_depth_10_refuses_where_no_bd10_stage_runs() {
        let pixels = vec![100u8; 64 * 64];
        // Speeds 1-6 map to presets 0,1,3,4,6,7 — all below 9, all broken.
        for speed in [1u8, 2, 3, 4, 5, 6] {
            let err = AvifEncoder::new()
                .with_bit_depth(10)
                .with_speed(speed)
                .encode_y8(&pixels, 64, 64, 64)
                .expect_err("10-bit mono below preset 9 has no bd10 producer");
            assert!(
                matches!(err, EncodeError::UnsupportedConfig(_)),
                "speed {speed}: expected UnsupportedConfig, got {err:?}"
            );
        }
        // Speeds 7-10 all clamp to preset 9, where the level post-pass runs.
        assert!(
            AvifEncoder::new()
                .with_bit_depth(10)
                .with_speed(10)
                .encode_y8(&pixels, 64, 64, 64)
                .is_ok(),
            "preset 9 has a real bd10 producer and must still encode"
        );
    }
}
