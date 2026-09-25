//! Safe Rust AV1 encoder — algorithm-for-algorithm port of SVT-AV1.
//!
//! Still pictures are byte-identical to the named C oracle across a broad
//! tested envelope; video encodes in a measured envelope verified against a
//! decoder rather than against C's bytes. The README's support table has the
//! gate behind every claim, and configurations outside the envelope are
//! refused with the measurement in the refusal text, never approximated.
//!
//! # Two ways in
//!
//! - [`avif::AvifEncoder`]: stills and (with `avif-container`) animated AVIF,
//!   configured by quality, speed, reference and fork knobs.
//! - [`pipeline::EncodePipeline`]: raw AV1 OBUs, one frame per call, for
//!   callers that own the container and the GOP.
//!
//! ```no_run
//! use svtav1::pipeline::{EncodePipeline, RcConfig, RcMode};
//!
//! let (w, h) = (64u32, 64u32);
//! let rc = RcConfig { mode: RcMode::Cqp, qp: 32, ..RcConfig::default() };
//! let mut p = EncodePipeline::new(w, h, 8, rc, 0, 1).with_chroma_420(true);
//! let (y, u, v) = (vec![128u8; 64 * 64], vec![128u8; 32 * 32], vec![128u8; 32 * 32]);
//! let obu = p.try_encode_frame_420(&y, &u, &v, w as usize).expect("encode");
//! assert!(!obu.is_empty());
//! ```
//!
//! The internal crates (`encoder`, `dsp`, `types`, `entropy`, `tables`) are
//! re-exported only with the `__expert` feature: their surface is the port's
//! working structure, not an interface.
//!
//! # Safety
//!
//! This crate uses `#![forbid(unsafe_code)]` — it is fully safe Rust.
//! SIMD acceleration is provided through the `archmage` crate's
//! safe intrinsics API.
#![forbid(unsafe_code)]

pub mod avif;
pub mod policy;
/// RGBA convenience: decoded pixels in, animated AVIF out.
#[cfg(feature = "avif-container")]
pub mod rgba;

/// The raw-OBU encode API: the pipeline and the types its signatures take.
pub mod pipeline {
    pub use svtav1_encoder::entropy::obu::{
        ColorDescription, FilmGrainParams, compute_seq_level_idx,
    };
    pub use svtav1_encoder::film_grain_config::FilmGrainConfig;
    pub use svtav1_encoder::fork_config::ForkConfig;
    pub use svtav1_encoder::hdr_mode::{HdrForkConfig, SvtHdrMode};
    pub use svtav1_encoder::pipeline::EncodePipeline;
    pub use svtav1_encoder::rate_control::{RcConfig, RcMode};
    pub use svtav1_encoder::reference::SvtReference;
    pub use svtav1_types::chroma::ChromaFormat;
    pub use svtav1_types::{EncodeError, EncodeResult};
}

#[cfg(feature = "__expert")]
pub use svtav1_dsp as dsp;
#[cfg(feature = "__expert")]
pub use svtav1_encoder as encoder;
#[cfg(feature = "__expert")]
pub use svtav1_encoder::entropy;
#[cfg(feature = "__expert")]
pub use svtav1_types as types;
#[cfg(feature = "__expert")]
pub use svtav1_types::tables;
