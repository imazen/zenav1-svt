//! Transforms, prediction, filtering — SIMD hot path.
//!
//! Uses archmage for all SIMD dispatch.
#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod copy;
pub mod fwd_txfm;
pub mod hadamard;
pub mod intra_pred;
pub mod sad;
pub mod variance;
