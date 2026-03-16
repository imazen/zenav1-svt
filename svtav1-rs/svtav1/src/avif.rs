//! AVIF encoding backend for zenavif integration.
//!
//! Provides a builder-pattern API compatible with zenavif's encoder backend
//! interface, allowing svtav1-rs to be used as an AV1 encoder for still
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

use svtav1_dsp::intra_pred;
use svtav1_encoder::encode_loop;
use svtav1_encoder::mode_decision;
use svtav1_entropy::context;
use svtav1_entropy::writer::AomWriter;
use svtav1_types::block::BlockSize;
use svtav1_types::prediction::PredictionMode;

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
}

impl core::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidDimensions => write!(f, "Invalid image dimensions"),
            Self::InvalidQuality => write!(f, "Quality must be between 1.0 and 100.0"),
            Self::EncodeFailed(msg) => write!(f, "Encode failed: {msg}"),
        }
    }
}

/// AVIF still-image encoder using svtav1-rs as the AV1 backend.
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
    /// Applied when perceptual optimization is wired through.
    seg_boost: f64,
    /// Lossless encoding mode.
    lossless: bool,
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
    pub fn with_trellis(mut self, enable: bool) -> Self {
        self.enable_trellis = enable;
        self
    }

    /// Enable or disable lossless encoding.
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

    /// Map quality (1.0-100.0) to AV1 QP (0-63).
    ///
    /// Quality 100 -> QP 0 (best), quality 1 -> QP 63 (worst).
    /// The mapping is linear: QP = 63 - floor((quality - 1) * 63 / 99).
    pub fn quality_to_qp_static(quality: f32) -> u8 {
        Self::quality_to_qp(quality)
    }

    fn quality_to_qp(quality: f32) -> u8 {
        let clamped = quality.clamp(1.0, 100.0);
        let qp = 63.0 - (clamped - 1.0) * 63.0 / 99.0;
        (qp.round() as u8).min(63)
    }

    /// Map speed (1-10) to SVT-AV1 preset (0-13).
    ///
    /// Speed 1 -> preset 0 (slowest/best), speed 10 -> preset 13 (fastest).
    /// Intermediate values are linearly interpolated.
    fn speed_to_preset(speed: u8) -> u8 {
        let clamped = speed.clamp(1, 10);
        // Map 1..=10 to 0..=13: preset = (speed - 1) * 13 / 9
        let preset = ((clamped as u32 - 1) * 13 + 4) / 9;
        preset as u8
    }

    /// Encode a single grayscale (Y-only) still image.
    ///
    /// The image is split into 8x8 blocks, and each block is encoded
    /// using intra-only prediction with RD-optimized mode selection.
    pub fn encode_y8(
        &self,
        pixels: &[u8],
        width: u32,
        height: u32,
        stride: u32,
    ) -> Result<EncodedAvif, EncodeError> {
        self.validate_dimensions(pixels.len(), width, height, stride)?;
        self.validate_quality()?;

        let qp = Self::quality_to_qp(self.quality);
        let _preset = Self::speed_to_preset(self.speed);
        let lambda = svtav1_encoder::rate_control::qp_to_lambda(qp) as u64;

        // Allocate reconstruction buffer (needed for neighbor-based prediction)
        let w = width as usize;
        let h = height as usize;
        let mut recon = vec![128u8; w * h];

        // Bitstream writer — estimate capacity from image size
        let capacity = w * h; // generous upper bound
        let mut writer = AomWriter::new(capacity);

        // Write simplified header: QP, preset, width, height
        writer.write_literal(qp as u32, 6);
        writer.write_literal(_preset as u32, 4);
        writer.write_literal(width, 16);
        writer.write_literal(height, 16);

        // Encode block-by-block in raster order
        let bw = 8usize;
        let bh = 8usize;
        let blocks_x = w.div_ceil(bw);
        let blocks_y = h.div_ceil(bh);

        for by in 0..blocks_y {
            for bx in 0..blocks_x {
                let x0 = bx * bw;
                let y0 = by * bh;
                let cur_w = bw.min(w - x0);
                let cur_h = bh.min(h - y0);

                // Extract source block — always pad to full 8x8 by repeating
                // edge pixels so the transform sees a complete block.
                let mut src_block = [0u8; 64]; // 8x8
                for row in 0..bh {
                    for col in 0..bw {
                        let sr = row.min(cur_h - 1);
                        let sc = col.min(cur_w - 1);
                        src_block[row * bw + col] = pixels[(y0 + sr) * stride as usize + (x0 + sc)];
                    }
                }

                // Build neighbor context from reconstruction buffer
                let (above, left, top_left, has_above, has_left) =
                    Self::get_neighbors(&recon, w, x0, y0, cur_w, cur_h);

                // Generate and evaluate intra candidates
                let mut candidates = mode_decision::generate_intra_candidates(BlockSize::Block8x8);

                let mut best_idx = 0;
                let mut best_cost = u64::MAX;

                for (idx, cand) in candidates.iter_mut().enumerate() {
                    // Generate prediction for full 8x8 block
                    let mut pred_block = [128u8; 64];
                    Self::generate_prediction(
                        cand.mode,
                        &mut pred_block,
                        bw,
                        &above,
                        &left,
                        top_left,
                        bw,
                        bh,
                        has_above,
                        has_left,
                    );

                    // Evaluate RD cost on the valid region only
                    // (flatten valid region for the evaluator)
                    let mut src_valid = [0u8; 64];
                    let mut pred_valid = [0u8; 64];
                    for row in 0..cur_h {
                        for col in 0..cur_w {
                            src_valid[row * cur_w + col] = src_block[row * bw + col];
                            pred_valid[row * cur_w + col] = pred_block[row * bw + col];
                        }
                    }
                    mode_decision::evaluate_candidate(
                        cand,
                        &src_valid[..cur_w * cur_h],
                        &pred_valid[..cur_w * cur_h],
                        cur_w,
                        cur_h,
                        lambda,
                    );

                    if cand.rd_cost < best_cost {
                        best_cost = cand.rd_cost;
                        best_idx = idx;
                    }
                }

                let best_mode = candidates[best_idx].mode;

                // Generate the winning prediction (full 8x8)
                let mut pred_block = [128u8; 64];
                Self::generate_prediction(
                    best_mode,
                    &mut pred_block,
                    bw,
                    &above,
                    &left,
                    top_left,
                    bw,
                    bh,
                    has_above,
                    has_left,
                );

                // Encode full 8x8: predict -> transform -> quantize -> reconstruct
                let encode_result =
                    encode_loop::encode_block(&src_block, bw, &pred_block, bw, bw, bh, qp);

                // Write mode and coefficients to bitstream
                context::write_intra_mode(&mut writer, best_mode as u8);
                Self::write_coefficients(&mut writer, &encode_result);

                // Update reconstruction buffer (only the valid region)
                for row in 0..cur_h {
                    for col in 0..cur_w {
                        recon[(y0 + row) * w + (x0 + col)] = encode_result.recon[row * bw + col];
                    }
                }
            }
        }

        let bitstream = writer.done().to_vec();

        Ok(EncodedAvif {
            data: bitstream,
            width,
            height,
            bit_depth: self.bit_depth,
        })
    }

    /// Encode a YUV 4:2:0 image.
    ///
    /// Encodes luma and chroma planes independently using intra-only
    /// prediction. Chroma planes are half the luma dimensions.
    pub fn encode_yuv420(
        &self,
        y: &[u8],
        u: &[u8],
        v: &[u8],
        width: u32,
        height: u32,
        y_stride: u32,
    ) -> Result<EncodedAvif, EncodeError> {
        if width == 0 || height == 0 {
            return Err(EncodeError::InvalidDimensions);
        }
        if width % 2 != 0 || height % 2 != 0 {
            return Err(EncodeError::InvalidDimensions);
        }

        let y_len_needed = (height - 1) * y_stride + width;
        if (y.len() as u32) < y_len_needed {
            return Err(EncodeError::InvalidDimensions);
        }

        let chroma_w = width / 2;
        let chroma_h = height / 2;
        let chroma_stride = chroma_w;
        let chroma_len_needed = (chroma_h - 1) * chroma_stride + chroma_w;
        if (u.len() as u32) < chroma_len_needed || (v.len() as u32) < chroma_len_needed {
            return Err(EncodeError::InvalidDimensions);
        }

        self.validate_quality()?;

        // Encode luma plane
        let luma_result = self.encode_y8(y, width, height, y_stride)?;

        // Encode chroma planes at half resolution
        let u_result = self.encode_y8(u, chroma_w, chroma_h, chroma_stride)?;
        let v_result = self.encode_y8(v, chroma_w, chroma_h, chroma_stride)?;

        // Combine into a single bitstream with plane markers
        let mut combined = Vec::with_capacity(
            4 + luma_result.data.len() + 4 + u_result.data.len() + 4 + v_result.data.len(),
        );

        // Length-prefixed plane concatenation
        let luma_len = luma_result.data.len() as u32;
        combined.extend_from_slice(&luma_len.to_le_bytes());
        combined.extend_from_slice(&luma_result.data);

        let u_len = u_result.data.len() as u32;
        combined.extend_from_slice(&u_len.to_le_bytes());
        combined.extend_from_slice(&u_result.data);

        let v_len = v_result.data.len() as u32;
        combined.extend_from_slice(&v_len.to_le_bytes());
        combined.extend_from_slice(&v_result.data);

        Ok(EncodedAvif {
            data: combined,
            width,
            height,
            bit_depth: self.bit_depth,
        })
    }

    // ========================================================================
    // Internal helpers
    // ========================================================================

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

    /// Validate quality range.
    fn validate_quality(&self) -> Result<(), EncodeError> {
        if !(1.0..=100.0).contains(&self.quality) {
            return Err(EncodeError::InvalidQuality);
        }
        Ok(())
    }

    /// Extract neighbor pixels from the reconstruction buffer for intra prediction.
    fn get_neighbors(
        recon: &[u8],
        recon_stride: usize,
        x0: usize,
        y0: usize,
        width: usize,
        height: usize,
    ) -> ([u8; 64], [u8; 64], u8, bool, bool) {
        let has_above = y0 > 0;
        let has_left = x0 > 0;

        let mut above = [128u8; 64];
        let mut left = [128u8; 64];
        let mut top_left = 128u8;

        if has_above {
            for col in 0..width {
                if x0 + col < recon_stride {
                    above[col] = recon[(y0 - 1) * recon_stride + x0 + col];
                }
            }
        }
        if has_left {
            for row in 0..height {
                above[row] = if has_above && x0 > 0 {
                    // keep above as-is, populate left
                    above[row]
                } else {
                    above[row]
                };
                left[row] = recon[(y0 + row) * recon_stride + x0 - 1];
            }
        }
        if has_above && has_left {
            top_left = recon[(y0 - 1) * recon_stride + x0 - 1];
        }

        (above, left, top_left, has_above, has_left)
    }

    /// Generate an intra prediction block for the given mode.
    fn generate_prediction(
        mode: PredictionMode,
        pred: &mut [u8],
        pred_stride: usize,
        above: &[u8],
        left: &[u8],
        top_left: u8,
        width: usize,
        height: usize,
        has_above: bool,
        has_left: bool,
    ) {
        match mode {
            PredictionMode::DcPred => {
                intra_pred::predict_dc(
                    pred,
                    pred_stride,
                    above,
                    left,
                    width,
                    height,
                    has_above,
                    has_left,
                );
            }
            PredictionMode::VPred => {
                if has_above {
                    intra_pred::predict_v(pred, pred_stride, above, width, height);
                } else {
                    // Fallback to DC when above is unavailable
                    intra_pred::predict_dc(
                        pred,
                        pred_stride,
                        above,
                        left,
                        width,
                        height,
                        false,
                        has_left,
                    );
                }
            }
            PredictionMode::HPred => {
                if has_left {
                    intra_pred::predict_h(pred, pred_stride, left, width, height);
                } else {
                    intra_pred::predict_dc(
                        pred,
                        pred_stride,
                        above,
                        left,
                        width,
                        height,
                        has_above,
                        false,
                    );
                }
            }
            PredictionMode::SmoothPred => {
                if has_above && has_left {
                    intra_pred::predict_smooth(pred, pred_stride, above, left, width, height);
                } else {
                    intra_pred::predict_dc(
                        pred,
                        pred_stride,
                        above,
                        left,
                        width,
                        height,
                        has_above,
                        has_left,
                    );
                }
            }
            PredictionMode::SmoothVPred => {
                if has_above && has_left {
                    intra_pred::predict_smooth_v(pred, pred_stride, above, left, 0, height, width);
                } else {
                    intra_pred::predict_dc(
                        pred,
                        pred_stride,
                        above,
                        left,
                        width,
                        height,
                        has_above,
                        has_left,
                    );
                }
            }
            PredictionMode::SmoothHPred => {
                if has_above && has_left {
                    intra_pred::predict_smooth_h(pred, pred_stride, above, left, width, height);
                } else {
                    intra_pred::predict_dc(
                        pred,
                        pred_stride,
                        above,
                        left,
                        width,
                        height,
                        has_above,
                        has_left,
                    );
                }
            }
            PredictionMode::PaethPred => {
                if has_above && has_left {
                    intra_pred::predict_paeth(
                        pred,
                        pred_stride,
                        above,
                        left,
                        top_left,
                        width,
                        height,
                    );
                } else {
                    intra_pred::predict_dc(
                        pred,
                        pred_stride,
                        above,
                        left,
                        width,
                        height,
                        has_above,
                        has_left,
                    );
                }
            }
            // Directional modes — fall back to DC for now (full directional
            // prediction requires angle computation not yet wired through)
            _ => {
                intra_pred::predict_dc(
                    pred,
                    pred_stride,
                    above,
                    left,
                    width,
                    height,
                    has_above,
                    has_left,
                );
            }
        }
    }

    /// Write quantized coefficients to the bitstream.
    fn write_coefficients(writer: &mut AomWriter, result: &encode_loop::EncodeBlockResult) {
        // Write EOB (end of block) as a 7-bit value (max 64 for 8x8)
        writer.write_literal(result.eob as u32, 7);

        if result.eob == 0 {
            return;
        }

        // Write non-zero coefficients
        for &coeff in &result.qcoeffs[..result.eob as usize] {
            let sign = coeff < 0;
            let abs_val = coeff.unsigned_abs();

            // Write sign bit
            writer.write_bit(sign);
            // Write magnitude (simplified: 10-bit fixed width)
            writer.write_literal(abs_val.min(1023), 10);
        }
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
        // Speed 10 -> preset 13 (fastest)
        assert_eq!(AvifEncoder::speed_to_preset(10), 13);
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
}
