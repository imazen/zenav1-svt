//! Quantization matrices, scan orders, filter taps, and lookup tables.
//!
//! Pure const data — no_std, no alloc. Folded in from the former
//! `zenav1-svt-tables` crate (issue #3): every table is reachable as
//! `svtav1_types::tables::<module>::<ITEM>` and byte-for-byte unchanged.
pub mod block;
pub mod interp;
pub mod partition;
pub mod scan;
pub mod transform;
