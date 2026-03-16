//! Safe Rust AV1 encoder — algorithm-for-algorithm port of SVT-AV1.
//!
//! This is the public facade crate that re-exports the encoder and types.
#![forbid(unsafe_code)]

pub use svtav1_tables as tables;
pub use svtav1_types as types;
