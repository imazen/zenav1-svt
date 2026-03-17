//! Arithmetic coder, CDF tables, and context models.
//!
//! Ported from SVT-AV1's `bitstream_unit.c/h` and `cabac_context_model.h`.
#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod cdf;
pub mod context;
pub mod obu;
pub mod range_coder;
pub mod writer;
