//! Mode decision, rate control, encoding loop, and pipeline.
#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod encode_loop;
pub mod mode_decision;
pub mod motion_est;
pub mod rate_control;
