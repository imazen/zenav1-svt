//! Transforms, prediction, filtering — SIMD hot path.
//!
//! Uses archmage for all SIMD dispatch.
#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod ac_bias;
#[cfg(feature = "std")]
pub mod bench;
pub mod cdef;
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
pub mod obmc;
pub mod port_convolve;
pub mod port_convolve_hbd;
pub mod port_inter_predictor;
pub mod port_interintra;
pub mod port_masked_compound;
pub mod port_wedge_masks;
pub mod port_scale_factors;
pub mod port_sgr;
pub mod port_warp;
pub mod quant;
pub mod quant_coding;
pub mod quant_tables;
pub mod residual;
pub mod resize;
pub mod restoration;
pub mod sad;
pub mod scale;
pub mod subpel_variance;
pub mod superres;
pub mod txfm_dispatch;
pub mod txfm_simd;
pub mod variance;
pub mod warp;
