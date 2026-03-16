//! Core data structures, constants, and enums for the SVT-AV1 Rust port.
//!
//! This crate contains all AV1 bitstream types, encoder-internal types,
//! and constants. No runtime code — just type definitions and const data.
//!
//! All types are ported from SVT-AV1's `definitions.h`, `mv.h`,
//! `block_structures.h`, and `av1_structs.h`.
#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod block;
pub mod constants;
pub mod frame;
pub mod interp;
pub mod motion;
pub mod partition;
pub mod prediction;
pub mod quantization;
pub mod reference;
pub mod restoration;
pub mod transform;
