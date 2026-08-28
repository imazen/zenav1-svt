//! Arithmetic coder, CDF tables, and context models.
//!
//! Ported from SVT-AV1's `bitstream_unit.c/h` and `cabac_context_model.h`.
//! Folded in from the former `zenav1-svt-entropy` crate (issue #3): the
//! range coder has exactly one consumer — this encoder — so it lives here as
//! `svtav1_encoder::entropy::<module>`, byte-for-byte unchanged.
pub mod cdf;
pub mod coeff;
pub mod coeff_c;
mod coeff_simd;
pub mod context;
pub mod default_cdfs;
pub mod default_coef_cdfs;
pub mod lr;
pub mod mv_coding;
pub mod obu;
pub mod range_coder;
pub mod scan_tables;
pub mod tile;
pub mod writer;
