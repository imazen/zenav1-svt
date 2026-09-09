//! Aggregated integration-test entry point.
//!
//! Every file in `tests/` used to be its own crate AND its own linked
//! executable -- 182 of them workspace-wide, 76-97% DWARF each, 5.62 GB per
//! clean build (measured 2026-09-09). Declaring them as modules of one target
//! links once instead of once per file. The files are NOT moved, so every
//! `include_bytes!("data/...")` path still resolves.
//!
//! DELIBERATELY NOT MERGED, and each still its own `[[test]]` target:
//!   * files calling `archmage::testing::for_each_token_permutation` or
//!     `lock_token_testing` -- token disabling is PROCESS-WIDE, and this repo
//!     also runs plain `cargo test` (threaded, one process) from CI and from
//!     tools/regression_spotcheck.sh, where merging them would let one test
//!     flip the dispatch tier underneath another.
//!   * files named directly by a gate: `--test tier_invariance` (CI),
//!     `--test odd_frame_recon` and `--test still_policy` (spotcheck).

mod c_parity_ac_bias;
mod c_parity_adst32;
mod c_parity_dc;
mod c_parity_estimate_transform;
mod c_parity_intra_pred_hbd;
mod c_parity_inv_recon;
mod c_parity_lpf;
mod c_parity_lpf_hbd;
mod c_parity_obmc;
mod c_parity_pic_operators;
mod c_parity_pic_operators_hbd;
mod c_parity_port_convolve;
mod c_parity_port_convolve_hbd;
mod c_parity_port_convolve_scale;
mod c_parity_port_diffwtd_d16;
mod c_parity_port_enc_make_pred;
mod c_parity_port_inter_predictor;
mod c_parity_port_interintra;
mod c_parity_port_light_pd1_hbd;
mod c_parity_port_masked_blend;
mod c_parity_port_masked_compound;
mod c_parity_port_model_rd;
mod c_parity_port_obmc_data;
mod c_parity_port_obmc_nb_pred;
mod c_parity_port_obmc_single_pred;
mod c_parity_port_pack;
mod c_parity_port_scale_factors;
mod c_parity_port_subpel_params;
mod c_parity_port_tf_pred;
mod c_parity_port_wedge_masks;
mod c_parity_port_wedge_search;
mod c_parity_resize;
mod c_parity_resize_avx2_divergence;
mod c_parity_resize_hbd;
mod c_parity_resize_plane_2d;
mod c_parity_scale;
mod c_parity_sgr;
mod c_parity_subpel_variance;
mod c_parity_superres;
mod c_parity_txfm_pf;
mod c_parity_txfm_pf_2d;
mod c_parity_txfm_pf_entry;
mod c_parity_warp;
mod c_parity_warp_model;
mod c_parity_wht;
mod paeth_neon_parity;
mod quantize_neon_parity;
mod sad_neon_parity;
mod satd_neon_parity;
mod variance_neon_parity;
