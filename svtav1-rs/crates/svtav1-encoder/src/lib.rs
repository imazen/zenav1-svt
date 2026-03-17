//! Mode decision, rate control, encoding loop, and pipeline.
#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod encode_loop;
pub mod film_grain;
pub mod mode_decision;
pub mod motion_est;
pub mod partition;
pub mod perceptual;
pub mod picture;
pub mod pipeline;
pub mod rate_control;
pub mod speed_config;
pub mod temporal_filter;
