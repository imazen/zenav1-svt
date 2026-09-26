//! Mode decision, rate control, encoding loop, and pipeline.
#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]
// Without `std`, the diagnostics are dead: `dbgenv` flags read `false`, and
// the dumps gated on `feature = "std"` leave their inputs and helpers unused.
// The std build — the one CI lints with `-D warnings` — keeps these lints.
#![cfg_attr(
    not(feature = "std"),
    allow(dead_code, unused_variables, unused_assignments)
)]

extern crate alloc;

/// Without `std` there is no stderr. The debug dumps stay compiled — every
/// `dbgenv` flag reads `false` there, so they are dead code — and this inert
/// `eprintln!` keeps their format arguments type-checked and "used".
#[cfg(not(feature = "std"))]
macro_rules! eprintln {
    () => {};
    ($($t:tt)*) => {{
        let _ = ::core::format_args!($($t)*);
    }};
}

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
pub(crate) mod depth_refine;
// Modules marked `cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))`
// hold C translations the pipeline does not call; the parity differentials
// (compiled into this crate's tests) are their only callers, or nothing is.
// docs/DEAD-CODE.tsv lists every such item and CI keeps it current
// (tools/dead_code_ledger.py, plan T2): wire an item or delete it, never
// widen the allow.
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod dist_facade;
pub(crate) mod dlf_arm;
pub(crate) mod encdec_arm;
pub mod encode_loop;
pub mod enhancements;
pub mod entropy;
pub mod film_grain_config;
pub(crate) mod film_grain_denoise;
pub(crate) mod film_grain_fft;
pub(crate) mod film_grain_model;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod film_grain_synthesis;
pub mod fork_config;
pub(crate) mod frame_geom;
pub(crate) mod funnel_arm;
pub mod hdr_mode;
pub(crate) mod inter_hdr_arm;
pub(crate) mod inter_md_arm;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod inter_me;
pub(crate) mod inter_me_arm;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod inter_mv_code;
pub mod inter_mvp;
pub(crate) mod inter_pred_arm;
pub(crate) mod inter_search_arm;
pub(crate) mod intra_arm;
pub(crate) mod intra_edge;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod intra_open_loop;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code, unused_imports))]
pub(crate) mod intrabc;
pub(crate) mod intrabc_hash;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod intrabc_mvp;
pub(crate) mod intrabc_pred;
pub mod leaf_funnel;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod lf_levels;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod md_subpel;
pub(crate) mod mds0_arm;
pub mod mode_decision;
pub mod motion_est;
pub(crate) mod nic_arm;
pub(crate) mod noise_gen;
pub(crate) mod noise_norm;
pub(crate) mod obmc_pred_arm;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod palette;
pub(crate) mod part_arm;
pub mod partition;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod pd0;
pub(crate) mod picture;
pub mod pipeline;
pub(crate) mod port_coding_loop;
pub(crate) mod port_enc_dec_cdf;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod port_enc_dec_metrics;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code, unused_imports))]
pub(crate) mod port_enc_mode_config;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod port_entropy_inter;
pub(crate) mod port_frame_cdf;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod port_frame_update;
pub(crate) mod port_full_loop;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod port_full_loop_md;
pub(crate) mod port_global_me;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod port_global_motion;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod port_gm_correspondence;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod port_lr_level;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code, unused_imports))]
pub(crate) mod port_md;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod port_md_lambda;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod port_md_rate_estimation;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod port_md_winner;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod port_noise_model;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod port_pass2_gop;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod port_pass2_strategy;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod port_pcs_geom;
pub(crate) mod port_pd0_detector;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod port_pd_gop;
pub mod port_picstruct;
pub(crate) mod port_picstruct_ra;
pub mod port_preanalysis;
pub(crate) mod port_ransac;
pub(crate) mod port_rc_driver;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod port_rc_process;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod port_rc_rtc_cbr;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod port_rc_vbr_cbr;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod port_rc_vbr_cbr_qpick;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod port_rc_vbr_cbr_state;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod port_rc_vbr_cbr_update;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod port_rd_cost;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod port_ref_mgmt;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod port_sframe;
pub(crate) mod port_sgr_search;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod port_src_ops;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod port_superres_decision;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code, unused_imports))]
pub(crate) mod port_temporal_filtering;
pub(crate) mod port_tf_driver;
pub mod port_tpl;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod port_tune_vmaf;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod qm;
pub(crate) mod qm_tables;
pub mod quant;
pub(crate) mod rate_arm;
pub mod rate_control;
pub mod reference;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod restoration;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod sb128_geom;
pub(crate) mod sb_qindex;
pub mod sc_detect;
pub mod segmentation;
pub mod speed_config;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod ssim_md;
pub(crate) mod temporal_filter;
pub mod tune;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod tx_bias;
pub(crate) mod txs_arm;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod var_boost;
pub(crate) mod vartx;
pub(crate) mod vecpool;

mod lossless_mono;

/// Check a potentially stoppable token without touching encoder state.
#[inline]
pub(crate) fn stop_check(stop: &dyn enough::Stop) -> EncodeResult<()> {
    if stop.may_stop() {
        stop.check()
            .map_err(EncodeError::from)
            .map_err(whereat::at)?;
    }
    Ok(())
}

#[cfg(test)]
mod cancellation_tests;

// The C-parity differentials (`tests/encoder_parity.rs` and the modules it
// aggregates) compile INTO the crate's unit-test binary (plan T2), so they
// reach `pub(crate)` items and do not hold modules public. The alias keeps
// their `svtav1_encoder::...` paths resolving unchanged.
#[cfg(test)]
extern crate self as svtav1_encoder;
#[cfg(test)]
#[path = "../tests/encoder_parity.rs"]
mod encoder_parity;
