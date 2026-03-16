//! Transforms, prediction, filtering — SIMD hot path.
//!
//! Uses archmage for all SIMD dispatch.
#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

#[cfg(feature = "std")]
pub mod bench;
pub mod copy;
pub mod fwd_txfm;
pub mod hadamard;
pub mod inter_pred;
pub mod intra_pred;
pub mod inv_txfm;
pub mod loop_filter;
pub mod quant;
pub mod sad;
pub mod variance;
