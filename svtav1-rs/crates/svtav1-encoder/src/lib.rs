//! Mode decision, rate control, encoding loop, and pipeline.
#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod cdef;
pub mod deblock;
pub mod encode_loop;
pub mod film_grain;
pub mod intra_edge;
pub mod mode_decision;
pub mod motion_est;
pub mod multipass;
pub mod partition;
pub mod pd0;
pub mod perceptual;
pub mod quant;
pub mod picture;
pub mod pipeline;
pub mod rate_control;
pub mod restoration;
pub mod speed_config;
pub mod temporal_filter;
