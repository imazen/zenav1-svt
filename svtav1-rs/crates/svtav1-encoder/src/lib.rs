//! Mode decision, rate control, encoding loop, and pipeline.
#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod cdef;
pub mod deblock;
pub mod depth_refine;
pub mod encode_loop;
pub mod film_grain;
pub mod hdr_mode;
pub mod var_boost;
pub mod chroma_q;
pub mod noise_gen;
pub mod noise_norm;
pub mod qm;
pub mod qm_tables;
pub mod tx_bias;
pub mod intra_edge;
pub mod leaf_funnel;
pub mod mode_decision;
pub mod motion_est;
pub mod multipass;
pub mod partition;
pub mod pd0;
pub mod perceptual;
pub mod picture;
pub mod pipeline;
pub mod sc_detect;
pub mod quant;
pub mod sb_qindex;
pub mod rate_control;
pub mod restoration;
pub mod speed_config;
pub mod temporal_filter;
