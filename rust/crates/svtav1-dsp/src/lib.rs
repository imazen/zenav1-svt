//! Transforms, prediction, filtering — SIMD hot path.
//!
//! Uses archmage for all SIMD dispatch.
#![cfg_attr(not(any(feature = "std", test)), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod ac_bias;
#[cfg(feature = "std")]
pub mod bench;
pub mod cdef;
pub(crate) mod cfl_kernel;
pub mod copy;
pub mod fwd_txfm;
pub mod fwd_txfm_pf;
pub mod hadamard;
pub mod hbd;
pub mod inter_pred;
pub mod intra_pred;
// NOTE: no `intrabc` module here. A naive non-C-faithful placeholder
// (sum-of-pixels hash, hand-rolled DV validity missing the tile bounds /
// sub-8x8 chroma margin / INTRABC_DELAY wavefront) briefly lived at
// `src/intrabc.rs`; it was removed (IBC chunk 0, docs/ibc-port-map.md §B.4)
// in favor of the single canonical translation in
// `svtav1-encoder/src/intrabc.rs`. Do not resurrect it — the encoder module
// is the one verified against C (`svt_aom_is_dv_valid` et al.).
pub mod inv_txfm;
pub mod loop_filter;
pub mod me_sad;
pub(crate) mod obmc;
pub mod pic_operators;
pub mod port_compound_prep;
pub mod port_convolve;
// Modules marked `cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))`
// hold C translations the pipeline does not call; the parity differentials
// (compiled into this crate's tests) are their only callers, or nothing is.
// docs/DEAD-CODE.tsv lists every such item and CI keeps it current
// (tools/dead_code_ledger.py, plan T2): wire an item or delete it, never
// widen the allow.
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod port_convolve_hbd;
pub(crate) mod port_convolve_scale;
pub(crate) mod port_diffwtd_d16;
pub mod port_enc_make_pred;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod port_full_pd1_pred;
pub mod port_ifs;
pub mod port_inter_predictor;
pub mod port_interintra;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod port_make_pred;
pub mod port_masked_blend;
pub mod port_masked_compound;
pub mod port_model_rd;
pub mod port_obmc_build;
pub mod port_obmc_data;
pub mod port_obmc_nb_pred;
pub mod port_obmc_pred;
pub(crate) mod port_obmc_single_pred;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod port_pack;
pub mod port_pd_pred;
pub mod port_resize_hbd;
pub mod port_scale_factors;
pub mod port_sgr;
pub mod port_subpel_params;
pub mod port_tf_pred;
pub mod port_warp;
pub mod port_wedge_masks;
pub mod port_wedge_search;
pub mod quant;
pub mod quant_coding;
pub mod quant_tables;
pub mod residual;
pub mod resize;
pub mod restoration;
pub mod sad;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod scale;
pub mod subpel_variance;
pub mod superres;
pub mod txfm_dispatch;
pub(crate) mod txfm_simd;
pub mod variance;
#[cfg_attr(not(feature = "__dead_code_audit"), allow(dead_code))]
pub(crate) mod warp;

// The C-parity differentials (`tests/dsp_parity.rs` and the modules it
// aggregates) compile INTO the crate's unit-test binary (plan T2), so they
// reach `pub(crate)` items and do not hold modules public. The alias keeps
// their `svtav1_dsp::...` paths resolving unchanged.
#[cfg(test)]
extern crate self as svtav1_dsp;
#[cfg(test)]
#[path = "../tests/dsp_parity.rs"]
mod dsp_parity;
