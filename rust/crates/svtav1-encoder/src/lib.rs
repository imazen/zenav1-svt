//! Mode decision, rate control, encoding loop, and pipeline.
#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

// Feature 2: per-crate whereat crate-info so `at!(..)` in this crate can tag
// errors with `crate::at_crate_info()` (source location + repo links).
whereat::define_at_crate_info!();

// Feature 2: re-export the shared error surface so callers use
// `svtav1_encoder::{EncodeError, EncodeResult}` alongside the pipeline.
pub use svtav1_types::{EncodeError, EncodeResult};

pub mod bd10;
pub mod cdef;
pub mod chroma_q;
/// Cached presence checks for the debug env vars (internal; see the module doc
/// for why the uncached `getenv` was ~1 % of encode wall time).
mod dbgenv;
pub mod deblock;
pub mod depth_refine;
pub mod dist_facade;
pub mod encode_loop;
pub mod entropy;
pub mod film_grain;
pub mod frame_geom;
pub mod hdr_mode;
pub mod inter_me;
pub mod inter_mv_code;
pub mod inter_mvp;
pub mod intra_edge;
pub mod intra_open_loop;
pub mod intrabc;
pub mod intrabc_hash;
pub mod intrabc_mvp;
pub mod intrabc_pred;
pub mod leaf_funnel;
pub mod lf_levels;
pub mod md_subpel;
pub mod mode_decision;
pub mod motion_est;
pub mod multipass;
pub mod noise_gen;
pub mod noise_norm;
pub mod palette;
pub mod partition;
pub mod pd0;
pub mod perceptual;
pub mod picture;
pub mod pipeline;
pub mod port_coding_loop;
pub mod port_enc_mode_config;
pub mod port_enc_dec_cdf;
pub mod port_enc_dec_metrics;
pub mod port_entropy_inter;
pub mod port_frame_update;
pub mod port_full_loop;
pub mod port_full_loop_md;
pub mod port_global_motion;
pub mod port_gm_correspondence;
pub mod port_lr_level;
pub mod port_md;
pub mod port_md_lambda;
pub mod port_md_rate_estimation;
pub mod port_md_winner;
pub mod port_rd_cost;
pub mod port_pass2_gop;
pub mod port_pass2_strategy;
pub mod port_picstruct;
pub mod port_picstruct_ra;
pub mod port_preanalysis;
pub mod port_ransac;
pub mod port_rc_process;
pub mod port_rc_rtc_cbr;
pub mod port_rc_vbr_cbr;
pub mod port_rc_vbr_cbr_qpick;
pub mod port_rc_vbr_cbr_state;
pub mod port_rc_vbr_cbr_update;
pub mod port_sgr_search;
pub mod port_src_ops;
pub mod port_temporal_filtering;
pub mod port_tune_vmaf;
pub mod qm;
pub mod qm_tables;
pub mod quant;
pub mod rate_control;
pub mod restoration;
pub mod sb128_geom;
pub mod sb_qindex;
pub mod sc_detect;
pub mod segmentation;
pub mod speed_config;
pub mod ssim_md;
pub mod temporal_filter;
pub mod tune;
pub mod tx_bias;
pub mod var_boost;
pub mod vartx;
