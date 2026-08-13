//! Mode decision, rate control, encoding loop, and pipeline.
#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

// Feature 2: per-crate whereat crate-info so `at!(..)` in this crate can tag
// errors with `crate::at_crate_info()` (source location + repo links).
whereat::define_at_crate_info!();

// Feature 2: re-export the shared error surface so callers use
// `svtav1_encoder::{EncodeError, EncodeResult}` alongside the pipeline.
pub use svtav1_types::{EncodeError, EncodeResult};

/// TEMPORARY (2026-08-11): expose the DSP crate so the identity harness can
/// print `residual::ovf_probe`'s census. Probe-build only.
#[cfg(feature = "__ovf_probe")]
pub use svtav1_dsp as dsp;

pub mod bd10;
pub mod cdef;
pub mod chroma_q;
/// Cached presence checks for the debug env vars (internal; see the module doc
/// for why the uncached `getenv` was ~1 % of encode wall time).
mod dbgenv;
pub mod deblock;
pub mod depth_refine;
pub mod encode_loop;
pub mod film_grain;
pub mod frame_geom;
pub mod hdr_mode;
pub mod intra_edge;
pub mod intrabc;
pub mod intrabc_hash;
pub mod intrabc_mvp;
pub mod intrabc_pred;
pub mod leaf_funnel;
pub mod mode_decision;
pub mod motion_est;
pub mod multipass;
pub mod noise_gen;
pub mod noise_norm;
pub mod palette;
pub mod partition;
pub mod pd0;
pub mod perceptual;
pub mod picture;
pub mod pipeline;
pub mod qm;
pub mod qm_tables;
pub mod quant;
pub mod rate_control;
pub mod restoration;
pub mod sb128_geom;
pub mod sb_qindex;
pub mod sc_detect;
pub mod segmentation;
pub mod speed_config;
pub mod ssim_md;
pub mod temporal_filter;
pub mod tune;
pub mod tx_bias;
pub mod var_boost;
pub mod vartx;
