//! Safe Rust AV1 encoder — algorithm-for-algorithm port of SVT-AV1.
//!
//! # Overview
//!
//! `svtav1` is a safe Rust implementation of the SVT-AV1 video encoder,
//! targeting real-time transcoding (H.264/H.265 → AV1) with optional
//! CUDA GPU acceleration.
//!
//! # Architecture
//!
//! The encoder is split into focused crates:
//! - [`types`] — Core AV1 data structures, enums, and constants
//! - [`tables`] — Static lookup tables (quantization, filters, scan orders)
//! - [`dsp`] — SIMD-accelerated DSP primitives (transforms, prediction, filtering)
//! - [`entropy`] — Arithmetic coder and CDF-based entropy coding
//! - [`encoder`] — Encoding pipeline (ME, mode decision, rate control)
//!
//! # Usage
//!
//! ```no_run
//! use svtav1::{EncoderConfig, Encoder};
//!
//! let config = EncoderConfig::new(8); // preset 8
//! let mut encoder = Encoder::new(config).unwrap();
//! // encoder.send_frame(frame);
//! // let packet = encoder.receive_packet();
//! ```
//!
//! # Safety
//!
//! This crate uses `#![forbid(unsafe_code)]` — it is fully safe Rust.
//! SIMD acceleration is provided through the `archmage` crate's
//! safe intrinsics API.
#![forbid(unsafe_code)]

pub mod avif;

pub use svtav1_dsp as dsp;
pub use svtav1_encoder as encoder;
pub use svtav1_entropy as entropy;
pub use svtav1_tables as tables;
pub use svtav1_types as types;

// Re-export key types at the crate root for convenience
pub use svtav1_types::block::BlockSize;
pub use svtav1_types::frame::FrameType;
pub use svtav1_types::prediction::PredictionMode;
pub use svtav1_types::transform::{TxSize, TxType};

use svtav1_encoder::rate_control::{RcConfig, RcMode, RcState};

/// Encoder configuration.
#[derive(Debug, Clone)]
pub struct EncoderConfig {
    /// Encoder preset (0-13). Lower = slower/better quality, higher = faster.
    pub preset: u8,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Bit depth (8, 10, or 12).
    pub bit_depth: u8,
    /// Chroma subsampling (420, 422, or 444).
    pub chroma_format: ChromaFormat,
    /// Rate control configuration.
    pub rc: RcConfig,
    /// Number of encoding threads (0 = auto).
    pub threads: u32,
    /// Hierarchical levels for GOP structure (0-5).
    pub hierarchical_levels: u8,
    /// Whether to use 128x128 superblocks (vs 64x64).
    pub use_128x128_sb: bool,
}

/// Chroma subsampling format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromaFormat {
    /// 4:2:0 subsampling (most common for video).
    Yuv420,
    /// 4:2:2 subsampling.
    Yuv422,
    /// 4:4:4 no subsampling.
    Yuv444,
    /// Monochrome (no chroma).
    Yuv400,
}

impl EncoderConfig {
    /// Create a new encoder configuration with the given preset.
    ///
    /// Preset controls the speed/quality tradeoff:
    /// - 0-3: Research/high-quality (slowest)
    /// - 4-6: Good quality (moderate speed)
    /// - 7-9: Fast encoding
    /// - 10-13: Real-time encoding (fastest)
    pub fn new(preset: u8) -> Self {
        Self {
            preset: preset.min(13),
            width: 1920,
            height: 1080,
            bit_depth: 8,
            chroma_format: ChromaFormat::Yuv420,
            rc: RcConfig {
                mode: RcMode::Crf,
                qp: 30,
                ..RcConfig::default()
            },
            threads: 0,
            hierarchical_levels: 4,
            use_128x128_sb: false,
        }
    }

    /// Set resolution.
    pub fn with_resolution(mut self, width: u32, height: u32) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    /// Set bit depth.
    pub fn with_bit_depth(mut self, bd: u8) -> Self {
        self.bit_depth = bd;
        self
    }

    /// Set CRF quality target (0-63).
    pub fn with_crf(mut self, crf: u8) -> Self {
        self.rc.mode = RcMode::Crf;
        self.rc.qp = crf.min(63);
        self
    }

    /// Set VBR target bitrate in kbps.
    pub fn with_vbr(mut self, bitrate_kbps: u32) -> Self {
        self.rc.mode = RcMode::Vbr;
        self.rc.target_bitrate = bitrate_kbps;
        self
    }

    /// Set CBR target bitrate in kbps.
    pub fn with_cbr(mut self, bitrate_kbps: u32) -> Self {
        self.rc.mode = RcMode::Cbr;
        self.rc.target_bitrate = bitrate_kbps;
        self
    }
}

/// A YUV video frame to be encoded.
#[derive(Debug)]
pub struct Frame {
    /// Y (luma) plane data.
    pub y: Vec<u8>,
    /// U (chroma) plane data.
    pub u: Vec<u8>,
    /// V (chroma) plane data.
    pub v: Vec<u8>,
    /// Y plane stride.
    pub y_stride: usize,
    /// U plane stride.
    pub u_stride: usize,
    /// V plane stride.
    pub v_stride: usize,
    /// Presentation timestamp.
    pub pts: u64,
}

/// An encoded AV1 packet.
#[derive(Debug)]
pub struct Packet {
    /// Encoded AV1 data (OBU sequence).
    pub data: Vec<u8>,
    /// Frame type.
    pub frame_type: FrameType,
    /// Presentation timestamp.
    pub pts: u64,
    /// Size in bytes.
    pub size: usize,
}

/// The AV1 encoder.
pub struct Encoder {
    config: EncoderConfig,
    /// Rate control state — updated after each encoded picture.
    pub rc_state: RcState,
    frame_count: u64,
}

/// Encoder error types.
#[derive(Debug, Clone)]
pub enum EncoderError {
    /// Invalid configuration parameter.
    InvalidConfig(String),
    /// Encoder not ready to accept frames.
    NotReady,
    /// Encoding failed.
    EncodeFailed(String),
    /// End of stream.
    Eof,
}

impl core::fmt::Display for EncoderError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidConfig(msg) => write!(f, "Invalid config: {msg}"),
            Self::NotReady => write!(f, "Encoder not ready"),
            Self::EncodeFailed(msg) => write!(f, "Encode failed: {msg}"),
            Self::Eof => write!(f, "End of stream"),
        }
    }
}

impl Encoder {
    /// Create a new encoder with the given configuration.
    pub fn new(config: EncoderConfig) -> Result<Self, EncoderError> {
        if config.width == 0 || config.height == 0 {
            return Err(EncoderError::InvalidConfig(
                "Width and height must be > 0".into(),
            ));
        }
        if config.width % 2 != 0 || config.height % 2 != 0 {
            return Err(EncoderError::InvalidConfig(
                "Width and height must be even".into(),
            ));
        }
        if !matches!(config.bit_depth, 8 | 10 | 12) {
            return Err(EncoderError::InvalidConfig(
                "Bit depth must be 8, 10, or 12".into(),
            ));
        }

        Ok(Self {
            config,
            rc_state: RcState::default(),
            frame_count: 0,
        })
    }

    /// Send a frame to the encoder for encoding.
    ///
    /// Frames must be sent in display order. Send `None` to signal
    /// end of stream and flush remaining packets.
    pub fn send_frame(&mut self, frame: Option<Frame>) -> Result<(), EncoderError> {
        if let Some(_frame) = frame {
            self.frame_count += 1;
            Ok(())
        } else {
            // Flush signal
            Ok(())
        }
    }

    /// Receive an encoded packet from the encoder.
    ///
    /// Returns `Err(EncoderError::NotReady)` if no packet is available yet.
    /// Returns `Err(EncoderError::Eof)` when all packets have been flushed.
    pub fn receive_packet(&mut self) -> Result<Packet, EncoderError> {
        // Placeholder — real implementation would encode queued frames
        Err(EncoderError::NotReady)
    }

    /// Get the current encoder configuration.
    pub fn config(&self) -> &EncoderConfig {
        &self.config
    }

    /// Get the number of frames sent to the encoder.
    pub fn frame_count(&self) -> u64 {
        self.frame_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_encoder_default() {
        let config = EncoderConfig::new(8);
        let encoder = Encoder::new(config).unwrap();
        assert_eq!(encoder.frame_count(), 0);
    }

    #[test]
    fn create_encoder_with_resolution() {
        let config = EncoderConfig::new(4)
            .with_resolution(1280, 720)
            .with_crf(28);
        let encoder = Encoder::new(config).unwrap();
        assert_eq!(encoder.config().width, 1280);
        assert_eq!(encoder.config().height, 720);
    }

    #[test]
    fn reject_zero_dimensions() {
        let mut config = EncoderConfig::new(8);
        config.width = 0;
        assert!(Encoder::new(config).is_err());
    }

    #[test]
    fn reject_odd_dimensions() {
        let config = EncoderConfig::new(8).with_resolution(1921, 1080);
        assert!(Encoder::new(config).is_err());
    }

    #[test]
    fn reject_invalid_bit_depth() {
        let mut config = EncoderConfig::new(8);
        config.bit_depth = 9;
        assert!(Encoder::new(config).is_err());
    }

    #[test]
    fn preset_clamping() {
        let config = EncoderConfig::new(99);
        assert_eq!(config.preset, 13);
    }

    #[test]
    fn builder_pattern() {
        let config = EncoderConfig::new(6)
            .with_resolution(3840, 2160)
            .with_bit_depth(10)
            .with_vbr(10000);
        assert_eq!(config.width, 3840);
        assert_eq!(config.bit_depth, 10);
        assert_eq!(config.rc.mode, RcMode::Vbr);
        assert_eq!(config.rc.target_bitrate, 10000);
    }

    #[test]
    fn send_frame() {
        let config = EncoderConfig::new(8);
        let mut encoder = Encoder::new(config).unwrap();
        let frame = Frame {
            y: vec![128; 1920 * 1080],
            u: vec![128; 960 * 540],
            v: vec![128; 960 * 540],
            y_stride: 1920,
            u_stride: 960,
            v_stride: 960,
            pts: 0,
        };
        encoder.send_frame(Some(frame)).unwrap();
        assert_eq!(encoder.frame_count(), 1);
    }
}
