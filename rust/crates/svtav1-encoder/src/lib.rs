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

/// Feature 1: the one cooperative-cancellation poll, as a function.
///
/// Every in-encode stop check is this shape — `may_stop()` first so the
/// default `Unstoppable` token short-circuits to a predictable false branch
/// before any virtual `check()` call, then the typed error with a `whereat`
/// location. Written once so the poll cost, the error type and the location
/// tag cannot drift between the twenty-odd sites that use it.
///
/// BYTE-INERTNESS: this never touches encoder state. On the default token it
/// returns `Ok(())` unconditionally, so every caller's `?` is a not-taken
/// branch and the emitted bitstream is unchanged.
///
/// The poll DENSITY is a measured property, not a hope — see
/// `svtav1/examples/cancel_latency.rs` and
/// `benchmarks/cancel_latency_*.{tsv,meta}`.
#[inline]
pub fn stop_check(stop: &dyn enough::Stop) -> EncodeResult<()> {
    if stop.may_stop() {
        stop.check().map_err(EncodeError::from).map_err(whereat::at)?;
    }
    Ok(())
}

pub mod cdef;
pub mod deblock;
pub mod depth_refine;
pub mod encode_loop;
pub mod film_grain;
pub mod hdr_mode;
pub mod var_boost;
pub mod chroma_q;
pub mod noise_gen;
pub mod noise_norm;
pub mod palette;
pub mod qm;
pub mod ssim_md;
pub mod tune;
pub mod qm_tables;
pub mod tx_bias;
pub mod intra_edge;
pub mod leaf_funnel;
pub mod mode_decision;
pub mod motion_est;
pub mod multipass;
pub mod partition;
pub mod pd0;
pub mod perceptual;
pub mod picture;
pub mod pipeline;
pub mod sc_detect;
pub mod segmentation;
pub mod frame_geom;
pub mod sb128_geom;
pub mod bd10;
pub mod intrabc;
pub mod intrabc_hash;
pub mod intrabc_mvp;
pub mod intrabc_pred;
pub mod vartx;
pub mod quant;
pub mod sb_qindex;
pub mod rate_control;
pub mod restoration;
pub mod speed_config;
pub mod temporal_filter;
