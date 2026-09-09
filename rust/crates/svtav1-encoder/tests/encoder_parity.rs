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

mod c_parity_bd10_quant;
mod c_parity_cdef_pick;
mod c_parity_cdef_search_ctrls;
mod c_parity_dist_facade;
mod c_parity_dlf_ctrls;
mod c_parity_enc_dec_metrics;
mod c_parity_entropy_block;
mod c_parity_entropy_compound;
mod c_parity_entropy_inter;
mod c_parity_film_grain;
mod c_parity_frame_cdf;
mod c_parity_frame_update;
mod c_parity_full_loop_md;
mod c_parity_global_motion;
mod c_parity_inter_me;
mod c_parity_inter_mvp;
mod c_parity_intra_open_loop;
mod c_parity_intrabc;
mod c_parity_intrabc_hash;
mod c_parity_intrabc_mvp;
mod c_parity_intrabc_search;
mod c_parity_lf_levels;
mod c_parity_lr_syntax;
mod c_parity_md_drl;
mod c_parity_md_lambda;
mod c_parity_md_nics;
mod c_parity_md_pme;
mod c_parity_md_predicates;
mod c_parity_md_rate_estimation;
mod c_parity_md_ref_rate;
mod c_parity_md_ssim_hbd;
mod c_parity_md_subpel;
mod c_parity_md_winner;
mod c_parity_motion_est;
mod c_parity_mv;
mod c_parity_mv_code;
mod c_parity_noise_gen;
mod c_parity_noise_model;
mod c_parity_noise_norm;
mod c_parity_obmc_search;
mod c_parity_palette;
mod c_parity_pass2_gop;
mod c_parity_pcl_lpd1;
mod c_parity_pcl_nic;
mod c_parity_pcs_geom;
mod c_parity_picstruct;
mod c_parity_picstruct_dg;
mod c_parity_picstruct_ra_rps;
mod c_parity_picstruct_ref_mgmt;
mod c_parity_picstruct_statics;
mod c_parity_picstruct_tpl;
mod c_parity_preanalysis;
mod c_parity_qm;
mod c_parity_quant;
mod c_parity_ransac;
mod c_parity_rc_process;
mod c_parity_rc_qindex;
mod c_parity_rc_vbr_cbr_qpick;
mod c_parity_rc_vbr_cbr_state;
mod c_parity_rc_vbr_cbr_update;
mod c_parity_rd_cost;
mod c_parity_sb_qindex;
mod c_parity_segmentation;
mod c_parity_sgr_search;
mod c_parity_sig_deriv_common;
mod c_parity_sig_deriv_ctrls;
mod c_parity_sig_deriv_encdec;
mod c_parity_sig_deriv_leaf;
mod c_parity_sig_deriv_light_pd1;
mod c_parity_sig_deriv_md_config;
mod c_parity_sig_deriv_me;
mod c_parity_sig_deriv_multi_processes;
mod c_parity_sig_deriv_pd0;
mod c_parity_src_ops;
mod c_parity_ssim_md;
mod c_parity_subres_carry;
mod c_parity_superres_decision;
mod c_parity_temporal;
mod c_parity_temporal_filtering;
mod c_parity_tune_vmaf;
mod c_parity_tx_bias;
mod c_parity_var_boost;
mod cdef_screen_arm_reachability;
mod entropy_inter_writers_traced;
mod hbd_input_chunk1;
mod inter_me_traced;
mod inter_mvp_motion_field;
mod lossless_fh_c_capture;
mod lr_search_c_capture;
mod native_preset;
mod pass2_strategy_scalars;
mod port_pd_gop_traced;
mod port_picstruct_ra_traced;
mod port_picstruct_traced;
mod port_sframe_traced;
mod rc_rtc_cbr_scalars;
mod rc_vbr_cbr_tables_and_scalars;
mod sig_deriv_dlf_traced;
mod sig_deriv_tail_traced;
mod superres_header;
mod tf_traced;
mod traced_rc_ref_stats;
