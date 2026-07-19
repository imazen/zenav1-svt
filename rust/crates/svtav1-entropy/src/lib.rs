//! Arithmetic coder, CDF tables, and context models.
//!
//! Ported from SVT-AV1's `bitstream_unit.c/h` and `cabac_context_model.h`.
#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]

#[cfg(feature = "std")]
extern crate std;

extern crate alloc;

pub mod cdf;
pub mod coeff;
pub mod coeff_c;
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
