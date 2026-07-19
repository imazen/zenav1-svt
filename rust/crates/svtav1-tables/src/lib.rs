//! Quantization matrices, scan orders, filter taps, and lookup tables.
//!
//! Pure const data — no_std, no alloc.
#![no_std]
#![forbid(unsafe_code)]

pub mod block;
pub mod interp;
pub mod partition;
pub mod scan;
pub mod transform;
