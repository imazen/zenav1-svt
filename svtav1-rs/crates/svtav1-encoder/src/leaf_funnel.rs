//! C-exact leaf intra-mode decision funnel (allintra presets 4..=10,
//! still/PD1 fixed-tree path).
//!
//! Per-preset configuration lives in [`FunnelCfg::for_preset`]; the M5
//! extension (mode_end PAETH, angular deltas {-3,0,+3}, SH-gated edge-
//! filtered directional prediction, independent-uv at MDS3, txt 6/6
//! satd 15 rate 250) and the M4 extension (intra_level 1: ALL 7 angle
//! deltas, unfiltered prediction — SH bit 0; nic case 5: rank factors
//! 0, mds2 base 20, rel-dev off) are documented against their C cites
//! there and in docs/IDENTITY-STATUS.md 2026-07-14. The staging
//! skeleton below is the M6 baseline the other presets specialize:
//!
//! Ports the REGULAR-PD1 `md_encode_block` staging for the allintra M6
//! configuration, verified against instrumented-library captures
//! (docs/captures/gradient_*_p6.m6fnl.txt; every constant below carries
//! its C cite):
//!
//! - Candidates (`generate_md_stage_0_cand`, mode_decision.c:3621):
//!   intra_level 6 (enc_mode_config.c:6907 M6 row; set_intra_ctrls case 6
//!   :8574) => mode_end SMOOTH, angular_pred_level 4 (D45.. masked, no
//!   angle deltas), no prune flags — injection order DC, V, H, SMOOTH —
//!   plus FILTER_DC_PRED for blocks <= 32x32 (filter_intra level 2,
//!   :8045; svt_aom_filter_intra_allowed_bsize mode_decision.c:102).
//!   `is_dc_only_safe` is dead at M6 (prune_using_edge_info == 0).
//! - MDS0 (`fast_loop_core`, product_coding_loop.c:1258): whole-block
//!   luma prediction, Hadamard SATD (`hadamard_path` :1187 — 32x32-capped
//!   tiles, `mds0_use_hadamard_sb = true` for allintra PD1,
//!   enc_mode_config.c:11408), fast cost = RDCOST(lambda, flr + fcr,
//!   satd << 4) with `svt_aom_intra_fast_cost` rates (rd_cost.c:526).
//! - NIC (nic_level 6, svt_aom_get_nic_level_allintra:5999):
//!   scaling level 6 => stage nums 6/6/6 over I-slice class-0 base 64
//!   (MD_STAGE_NICS), qp-scaled (svt_aom_set_nics,
//!   product_coding_loop.c:1347); pruning ths mds1 1200/rank 3,
//!   mds2 15/rank 1/dev 5, mds3 15 (set_nic_controls case 6:6209),
//!   qp-scaled via svt_aom_get_qp_based_th_scaling_factors.
//! - MDS1 (`md_stage_1` :7269, staging mode 1): luma-only full loop at
//!   tx_depth 0, DCT_DCT, `quantize_b` (mds_do_rdoq = false —
//!   svt_aom_quantize_inv_quantize full_loop.c:1754), FREQ-domain SSE
//!   (spatial level 3 = SSSE_MDS3 only), real txb/dc-sign contexts
//!   (rate_est_level 1 => update_skip_ctx_dc_sign_ctx = 1), full cost =
//!   svt_aom_full_cost with zero chroma terms.
//! - MDS3 (`md_stage_3` :7397): TXS depths 0..1 (txs_level 3 intra sq
//!   max depth 1, prev_depth_coeff_exit 1), per-txb TXT search
//!   (`tx_type_search` :4660 — groups 4 (>=16x16) / 5 (<16x16) intra,
//!   SATD early-exit th 10 qp-scaled, rate th 100, depth-1 group offset
//!   3), RDOQ per the frame policy with REAL contexts, spatial SSE << 4,
//!   CHROMA full loop (CHROMA_MODE_1: uv follows luma;
//!   `svt_aom_full_loop_uv` full_loop.c:2161) with the
//!   chroma-complexity detector (:6095) gating CFL (cfl level 4,
//!   cplx_th 10 — CFL is only *evaluated* when the detector fires;
//!   flat-chroma content never fires it; if it fires we currently keep
//!   the non-CFL uv mode, documented as a residual gap), full cost =
//!   `svt_aom_full_cost` (rd_cost.c:1357).
//! - Winner: lowest full cost, first-in-order ties
//!   (`svt_aom_product_full_mode_decision`, mode_decision.c:3869).

use alloc::vec;
use alloc::vec::Vec;

use svtav1_entropy::coeff_c as cc;
use svtav1_entropy::context::FrameContext;

use crate::quant::{CoeffCostTables, QuantTable};

/// FILTER_INTRA_MODES = "no filter intra" sentinel (C definitions.h:1339).
pub const FI_NONE: u8 = 5;

// ---------------------------------------------------------------------------
// Rate tables (md_rate_estimation over a given frame context)
// ---------------------------------------------------------------------------

/// Mode-syntax + coefficient rate tables for one SB's frame context —
/// C `MdRateEstimationContext` slices the funnel consumes, built by
/// `svt_aom_estimate_syntax_rate` + `svt_aom_estimate_coefficients_rate`
/// from `pcs->ec_ctx_array[sb]` (enc_dec_process.c:3024-3043). Single-SB
/// frames always use the default contexts (`md_frame_context`).
pub struct MdRates {
    /// kf y mode: [above_ctx][left_ctx][mode] (y_mode_fac_bits).
    pub kf_y: [[[i32; 13]; 5]; 5],
    /// uv mode: [cfl_allowed][y_mode][uv_mode] (intra_uv_mode_fac_bits).
    pub uv: [[[i32; 14]; 13]; 2],
    /// angle_delta: [dir_mode - V][3 + delta] (angle_delta_fac_bits).
    pub angle: [[i32; 7]; 8],
    /// filter_intra flag: [block_size_index][used] (filter_intra_fac_bits).
    pub fi_flag: [[i32; 2]; 22],
    /// filter_intra_mode: [fi_mode] (filter_intra_mode_fac_bits).
    pub fi_mode: [i32; 5],
    /// skip flag: [skip_ctx][skip] (skip_fac_bits).
    pub skip: [[i32; 2]; 3],
    /// tx size: [tx_size_cat][tx_size_ctx][depth] (tx_size_fac_bits).
    pub tx_size: [[[i32; 3]; 3]; 4],
    /// intra tx-type signalling: costs derived on demand from this
    /// context's `intra_ext_tx_cdf` (av1_transform_type_rate_estimation).
    pub intra_ext_tx: [[i32; 17]; 13 * 4 * 3],
    /// Coefficient cost tables (svt_aom_estimate_coefficients_rate).
    pub coeff: alloc::boxed::Box<CoeffCostTables>,
}

fn costs_from_cdf<const N: usize>(cdf: &[u16]) -> [i32; N] {
    let mut out = [0i32; N];
    crate::quant::syntax_rate_from_cdf(&mut out, cdf);
    out
}

/// Build the funnel's rate tables from a (possibly chained) frame context
/// pair. `fc` carries the mode CDFs, `cfc` the coefficient CDFs.
pub fn build_md_rates(fc: &FrameContext, cfc: &cc::CoeffFc) -> alloc::boxed::Box<MdRates> {
    let mut r = alloc::boxed::Box::new(MdRates {
        kf_y: [[[0; 13]; 5]; 5],
        uv: [[[0; 14]; 13]; 2],
        angle: [[0; 7]; 8],
        fi_flag: [[0; 2]; 22],
        fi_mode: [0; 5],
        skip: [[0; 2]; 3],
        tx_size: [[[0; 3]; 3]; 4],
        intra_ext_tx: [[0; 17]; 13 * 4 * 3],
        coeff: crate::quant::build_coeff_cost_tables_from_fc(cfc),
    });
    for a in 0..5 {
        for l in 0..5 {
            r.kf_y[a][l] = costs_from_cdf(&fc.kf_y_mode_cdf[a][l]);
        }
    }
    for cfl in 0..2 {
        for y in 0..13 {
            let mut c = [0i32; 14];
            // CFL-disallowed rows have 13 symbols; cost fn reads the CDF
            // up to the terminator, so slice per-row width.
            if cfl == 0 {
                let mut c13 = [0i32; 13];
                crate::quant::syntax_rate_from_cdf(&mut c13, &fc.uv_mode_cdf[cfl][y]);
                c[..13].copy_from_slice(&c13);
            } else {
                crate::quant::syntax_rate_from_cdf(&mut c, &fc.uv_mode_cdf[cfl][y]);
            }
            r.uv[cfl][y] = c;
        }
    }
    for m in 0..8 {
        r.angle[m] = costs_from_cdf(&fc.angle_delta_cdf[m]);
    }
    for b in 0..22 {
        r.fi_flag[b] = costs_from_cdf(&fc.filter_intra_cdfs[b]);
    }
    r.fi_mode = costs_from_cdf(&fc.filter_intra_mode_cdf);
    for ctx in 0..3 {
        r.skip[ctx] = costs_from_cdf(&fc.skip_cdf[ctx]);
    }
    for cat in 0..4 {
        for ctx in 0..3 {
            r.tx_size[cat][ctx] = costs_from_cdf(&fc.tx_size_cdf[cat][ctx]);
        }
    }
    for row in 0..(13 * 4 * 3) {
        r.intra_ext_tx[row] = costs_from_cdf(&cfc.intra_ext_tx_cdf[row]);
    }
    r
}

impl MdRates {
    /// C `av1_transform_type_rate_estimation` (rd_cost.c:107) for INTRA:
    /// nonzero only when the tx size's intra ext set has > 1 type.
    /// `intra_dir` follows `fimode_to_intradir` for filter-intra blocks.
    fn txt_rate(&self, c_tx_size: usize, intra_dir: usize, tx_type: usize) -> i32 {
        if cc::ext_tx_types(c_tx_size, false, false) <= 1 {
            return 0;
        }
        let set_type = cc::ext_tx_set_type(c_tx_size, false, false);
        let eset = cc::EXT_TX_SET_INDEX[0][set_type];
        if eset == 0 {
            return 0;
        }
        let sq_tx = cc::TXSIZE_SQR_MAP[c_tx_size];
        let row = (eset as usize * 4 + sq_tx) * 13 + intra_dir;
        let sym = cc::AV1_EXT_TX_IND[set_type][tx_type];
        self.intra_ext_tx[row][sym]
    }
}

// ---------------------------------------------------------------------------
// Frame-level funnel configuration
// ---------------------------------------------------------------------------

/// Frame-constant funnel parameters.
pub struct FunnelFrame {
    /// `full_lambda_md[EB_8_BIT_MD]` — the kf chain at the frame qindex.
    pub lambda: u64,
    /// CLI qp 0..63 (qp-based threshold scaling input).
    pub cli_qp: u32,
    /// Frame rdoq level (0 = quantize_b at MDS3 too).
    pub rdoq_level: u8,
    pub base_qindex: u8,
    /// Per-preset intra-leaf config (M6 vs intra_level-7 M7/M8).
    pub cfg: FunnelCfg,
}

/// Per-preset leaf-funnel configuration (allintra still, presets 6/7/8),
/// verified against the instrumented C `svt_aom_sig_deriv_enc_dec_allintra`
/// config dump (enc_mode_config.c:11294). All fields are pure functions of
/// `enc_mode`; the M6 values reproduce the original hardcoded funnel exactly.
#[derive(Clone, Copy, Debug)]
pub struct FunnelCfg {
    /// filter-intra candidate + `use_filter_intra` syntax (M6: on level 2;
    /// M7/M8: `get_filter_intra_level_allintra` == 0 -> off).
    pub filter_intra: bool,
    /// `intra_ctrls.prune_using_best_mode` (M6: 0; M7/M8 intra_level 7: 1) —
    /// the MDS0 order-dependent H/SMOOTH skip (product_coding_loop.c:1688).
    pub prune_best_mode: bool,
    /// `MD_STAGE_NICS_SCAL_NUM[nic_scaling_level]` stage-1/2/3 numerators
    /// (M6 lvl6: 6/6/6; M7 lvl8: 4/4/4; M8 lvl15: 0/0/0). Base counts are
    /// the I-slice class-0 {64,32,16} scaled by these / 16 then qp-scaled.
    pub nic_num: (u64, u64, u64),
    /// `mds1_cand_base_th_intra` (M6/M7: 1200; M8: 1).
    pub mds1_cand_base_th: u64,
    /// `mds1_cand_th_rank_factor` (M5..M8: 3; M4 nic case 5: 0). When 0
    /// the mds1 divisor is 1 — no per-rank tightening (C ternary,
    /// product_coding_loop.c:8095).
    pub mds1_rank_factor: u64,
    /// `mds2_cand_base_th` (M5..M7: 15; M4: 20; M8: 1).
    pub mds2_cand_base_th: u64,
    /// `mds2_cand_th_rank_factor` (M5..M8: 1; M4 nic case 5: 0). When 0
    /// the mds2 divisor is 1 and the +2 winner-coincide staging is dead
    /// (C guards the staging on the factor being nonzero,
    /// product_coding_loop.c:8158-8171).
    pub mds2_rank_factor: u64,
    /// `mds2_relative_dev_th` (M5..M8: 5; M4 nic case 5: 0 = the
    /// relative-dev exit is DISABLED — C `!mds2_relative_dev_th ||`,
    /// product_coding_loop.c:8170).
    pub mds2_rel_dev_th: u64,
    /// `mds3_cand_base_th` (M6/M7: 15; M8: 1).
    pub mds3_cand_base_th: u64,
    /// `rate_est_ctrls.update_skip_ctx_dc_sign_ctx`/`update_skip_coeff_ctx`
    /// (M6 rate_est 1: real neighbour contexts; M7/M8 rate_est 4: 0/0).
    pub real_coeff_ctx: bool,
    /// TX-size search on (M6/M7 txs_level 3) vs off (M8 txs_level 0 ->
    /// depth 0 only).
    pub txs_on: bool,
    /// `intra_ctrls.prune_using_edge_info` (intra_level 8 / eff-M9 only):
    /// arms the `is_dc_only_safe` variance gate (mode_decision.c:845). When
    /// it fires for a block the candidate set is forced to {DC_PRED}. Off
    /// for M6/M7/M8 (intra_level 6/7 -> the gate is dead).
    pub dc_only_gate: bool,
    /// TXT search on (M6 txt_level 8 / M7/M8 txt_level 10) vs off (eff-M9
    /// txt_level 0 -> DCT_DCT only for every tx size, incl. < 32 blocks
    /// where an ext-tx set would otherwise be searched).
    pub txt_on: bool,
    /// `intra_ctrls.intra_mode_end` (C PredictionMode index): SMOOTH (9)
    /// at intra_level 6/7/8 (M6+), PAETH (12) at intra_level 2 (M5).
    pub mode_end: u8,
    /// `intra_ctrls.angular_pred_level`: 4 = D45..D203 masked + no angle
    /// deltas (M6+); 2 = all directional modes with deltas {-3, 0, +3}
    /// (M5, `inject_intra_candidates` skips |delta| 1/2, mode_decision.c
    /// :3268-3271); 3 = directional at delta 0 only; 1 = all 7 deltas.
    pub angular_level: u8,
    /// `txt_ctrls.txt_group_of_tx_types_for_types_of_size_lt_16 / ge_16`
    /// (set_txt_controls): M6 5/4, M5 (txt_level 3) 6/6 — the M5DBG dump
    /// fields `txt_lt16=6 txt_ge16=6`.
    pub txt_group_lt16: i32,
    pub txt_group_ge16: i32,
    /// `txt_ctrls.satd_early_exit_th_intra` (M6: 10; M5: 15), qp-scaled.
    pub txt_satd_th: u64,
    /// `txt_ctrls.txt_rate_cost_th` (M6: 100; M5: 250).
    pub txt_rate_th: u64,
    /// `txs_ctrls.intra_class_max_depth_sq` (txs_level 3 at M4..M6: 1;
    /// txs_level 2 at M0..M3: 2). Only consulted when `txs_on`.
    pub txs_max_sq: u8,
    /// `txs_ctrls.intra_class_max_depth_nsq` (M4..M6: 0; M0..M3: 2).
    pub txs_max_nsq: u8,
    /// `txs_ctrls.depth1_txt_group_offset` / `depth2_txt_group_offset`
    /// (txs_level 3: 3/3; txs_level 2: 0/0) — subtracted from the TXT
    /// group count at that tx depth (min 1, get_tx_type_group).
    pub txt_d1_off: i32,
    pub txt_d2_off: i32,
    /// chroma_level 4 (M5): CHROMA_MODE_0 with `ind_uv_last_mds = 2` —
    /// `search_best_mds3_uv_mode` over the MDS3 survivors' uv modes
    /// (+ UV_DC), then `update_intra_chroma_mode` rewrites each MDS3
    /// candidate's uv mode from `best_uv_mode[luma_mode]`
    /// (product_coding_loop.c:7561/:7436; skip_ind_uv_if_only_dc = 1).
    /// false = chroma_level 5 (CHROMA_MODE_1, uv follows luma — M6+).
    pub ind_uv_mds3: bool,
    /// SH `enable_intra_edge_filter` (M5 still/420 only): directional
    /// predictions run the corner/edge filters + upsampling
    /// (enc_intra_prediction.c:181-215).
    pub edge_filter: bool,
}

impl FunnelCfg {
    /// C-exact per-preset derivation for the still/420 allintra path.
    /// Presets 6/7/8/9+ (the funnel scope); other presets never construct
    /// one. Presets >= 9 clamp to eff-M9 (enc_handle.c:4634).
    pub fn for_preset(preset: u8) -> Self {
        // M6+ common tail (intra_level 6/7/8: mode_end SMOOTH, angular
        // level 4, txt groups 5/4 satd 10 rate 100, uv follows luma, no
        // SH edge filter bit).
        let m6_tail = FunnelCfg {
            filter_intra: true,
            prune_best_mode: false,
            nic_num: (6, 6, 6),
            mds1_cand_base_th: 1200,
            mds1_rank_factor: 3,
            mds2_cand_base_th: 15,
            mds2_rank_factor: 1,
            mds2_rel_dev_th: 5,
            mds3_cand_base_th: 15,
            real_coeff_ctx: true,
            txs_on: true,
            dc_only_gate: false,
            txt_on: true,
            mode_end: 9,
            angular_level: 4,
            txt_group_lt16: 5,
            txt_group_ge16: 4,
            txt_satd_th: 10,
            txt_rate_th: 100,
            txs_max_sq: 1,
            txs_max_nsq: 0,
            txt_d1_off: 3,
            txt_d2_off: 3,
            ind_uv_mds3: false,
            edge_filter: false,
        };
        match preset {
            // M2/M3 (still/420): the M5DBG CFG enc_mode=2/3 rows
            // (docs/captures/m0m5_config_dlf.txt lines 12-13) — config ==
            // M4 except:
            // - txt_level 2 (svt_aom_set_txt_controls case 2):
            //   satd_early_exit_th_intra 20 (vs 15), groups 6/6 + rate_th
            //   250 unchanged.
            // - txs_level 2 (set_txs_controls, enc_mode_config.c:7992):
            //   intra_class_max_depth_sq/nsq = 2/2 (vs 1/0),
            //   depth1/2_txt_group_offset = 0/0 (vs 3/3).
            // - M2 additionally drops nic_level 5 -> 3 (set_nic_controls
            //   case 3, enc_mode_config.c:6124): scaling level 3 -> nums
            //   12/12/12, mds1_base 1200 rank 0, mds2_base 30 rank 0
            //   rel_dev 0, mds3_base 25 (single intra class, staging
            //   MODE_1 — same walk semantics as case 5's zeros).
            // update_cdf_level 1 (vs 2) differs only in update_mv, which
            // is forced 0 on I-slices (set_cdf_controls,
            // enc_mode_config.c:12047-12085) — no funnel impact.
            2 => FunnelCfg {
                mode_end: 12,
                angular_level: 1,
                nic_num: (12, 12, 12),
                mds1_rank_factor: 0,
                mds2_cand_base_th: 30,
                mds2_rank_factor: 0,
                mds2_rel_dev_th: 0,
                mds3_cand_base_th: 25,
                txt_group_lt16: 6,
                txt_group_ge16: 6,
                txt_satd_th: 20,
                txt_rate_th: 250,
                txs_max_sq: 2,
                txs_max_nsq: 2,
                txt_d1_off: 0,
                txt_d2_off: 0,
                ind_uv_mds3: true,
                ..m6_tail
            },
            3 => FunnelCfg {
                mode_end: 12,
                angular_level: 1,
                mds1_rank_factor: 0,
                mds2_cand_base_th: 20,
                mds2_rank_factor: 0,
                mds2_rel_dev_th: 0,
                txt_group_lt16: 6,
                txt_group_ge16: 6,
                txt_satd_th: 20,
                txt_rate_th: 250,
                txs_max_sq: 2,
                txs_max_nsq: 2,
                txt_d1_off: 0,
                txt_d2_off: 0,
                ind_uv_mds3: true,
                ..m6_tail
            },
            // M4 (still/420): the M5DBG CFG enc_mode=4 dump
            // (docs/captures/m0m5_config_dlf.txt line 14) — config == M5
            // except:
            // - intra_level 1 (svt_aom_get_intra_mode_levels_allintra
            //   enc_mode_config.c:6907 `<= ENC_M4`; set_intra_ctrls case 1
            //   :8469): mode_end PAETH, angular_pred_level[1] = 1 (:18) —
            //   the |delta| 1/2 skip (mode_decision.c:3268-3271) only arms
            //   at level >= 2, so ALL SEVEN deltas -3..+3 are injected per
            //   directional mode (61 regular candidates + FILTER_DC).
            // - SH enable_intra_edge_filter = 0 (enc_mode_config.c:
            //   4035-4048: angular_pred_level[1] = 1 not in {2,3}) ->
            //   directional prediction is UNFILTERED (disable_edge_filter,
            //   enc_intra_prediction.c:526), like M6.
            // - nic_level 5 (svt_aom_get_nic_level_allintra :5986
            //   `<= ENC_M4`; set_nic_controls case 5): same scaling 6 /
            //   mds1_base 1200 / mds3_base 15 / staging MODE_1 as case 6,
            //   but mds1_cand_th_rank_factor 0, mds2_cand_base_th 20,
            //   mds2_cand_th_rank_factor 0, mds2_relative_dev_th 0 (class
            //   ths 300/25/15 + band counts are dead: single intra class).
            // Depth refinement 6 (vs M5's 9) stays unported like M5's:
            // the ADAPTIVE extra depths lose the inter-depth compare on
            // every tracked cell (capture partition streams == PD0 trees).
            4 => FunnelCfg {
                mode_end: 12,
                angular_level: 1,
                txt_group_lt16: 6,
                txt_group_ge16: 6,
                txt_satd_th: 15,
                txt_rate_th: 250,
                ind_uv_mds3: true,
                mds1_rank_factor: 0,
                mds2_cand_base_th: 20,
                mds2_rank_factor: 0,
                mds2_rel_dev_th: 0,
                ..m6_tail
            },
            // M5 (still/420): the M5DBG CFG enc_mode=5 dump
            // (docs/captures/m0m5_config_dlf.txt) — intra_level 2
            // (mode_end PAETH, ang 2), fi_max 0 (FILTER_DC only, same
            // candidate as M6), nic_level 6 with the SAME pruning ths as
            // M6 (1200/3, 15/5, 15), txt_level 3 (groups 6/6, satd 15,
            // rate 250, d1 offset 3), txs_sq depth 1, rdoq 1,
            // rate_est_level 1, chroma_level 4 (ind-uv at MDS3,
            // skip-if-only-DC, uv_nic 1), SH enable_intra_edge_filter=1.
            5 => FunnelCfg {
                mode_end: 12,
                angular_level: 2,
                txt_group_lt16: 6,
                txt_group_ge16: 6,
                txt_satd_th: 15,
                txt_rate_th: 250,
                ind_uv_mds3: true,
                edge_filter: true,
                ..m6_tail
            },
            6 => m6_tail,
            7 => FunnelCfg {
                filter_intra: false,
                prune_best_mode: true,
                nic_num: (4, 4, 4),
                real_coeff_ctx: false,
                ..m6_tail
            },
            // preset 8: nic_level 11 (scaling 15 -> nums 0/0/0 -> 1/1/1),
            // all cand thresholds 1, enable_skipping_mds1 (n1==1 makes it a
            // no-op for the pick), txs_level 0.
            8 => FunnelCfg {
                filter_intra: false,
                prune_best_mode: true,
                nic_num: (0, 0, 0),
                mds1_cand_base_th: 1,
                mds2_cand_base_th: 1,
                mds3_cand_base_th: 1,
                real_coeff_ctx: false,
                txs_on: false,
                ..m6_tail
            },
            // eff-M9 (presets 9+): intra_level 8 arms the is_dc_only gate
            // (dc_only_gate); the non-DC funnel body is identical to M8
            // (nic 1/1/1, prune_best, 0/0 ctx, txs off). coeff_rate_est_lvl
            // differs (0 vs 2) but never affects a single-candidate MDS3
            // (mode = MDS0 SATD winner; coeffs are RDOQ), so the M8 chroma
            // approximation is reused.
            _ => FunnelCfg {
                filter_intra: false,
                prune_best_mode: true,
                nic_num: (0, 0, 0),
                mds1_cand_base_th: 1,
                mds2_cand_base_th: 1,
                mds3_cand_base_th: 1,
                real_coeff_ctx: false,
                txs_on: false,
                dc_only_gate: true,
                txt_on: false,
                ..m6_tail
            },
        }
    }
}

/// C `RDCOST` (rd_cost.h:36).
#[inline]
fn rdcost(lambda: u64, rate: u64, dist: u64) -> u64 {
    ((rate * lambda + 256) >> 9) + (dist << 7)
}

/// C `DIVIDE_AND_ROUND`.
#[inline]
fn div_round(x: u64, y: u64) -> u64 {
    (x + (y >> 1)) / y
}

/// C `svt_aom_get_qp_based_th_scaling_factors(true, ..)` — the pd0 port.
fn qp_scale_factors(cli_qp: u32) -> (u64, u64) {
    let (w, d) = crate::pd0::qp_th_scaling_factors(cli_qp);
    (w as u64, d as u64)
}

/// NIC counts for I-slice class 0 at the config's scaling nums:
/// `svt_aom_set_nics` (product_coding_loop.c:1347), base {64, 32, 16}
/// (MD_STAGE_NICS[I][C0] = 64, >>1, >>2), scaled by num/16 then qp-scaled.
/// `min_nics = 2` when the stage's scaling num != 0 (I-slice pic_type < 2),
/// else 1 — so nums 0/0/0 (nic level 15/M8) yield 1/1/1.
fn nic_counts(cli_qp: u32, num: (u64, u64, u64)) -> (u32, u32, u32) {
    let (qw, qwd) = qp_scale_factors(cli_qp);
    let scale = |base: u64, num: u64| -> u32 {
        let min = if num != 0 { 2u64 } else { 1u64 };
        let n = min.max(div_round(base * num, 16));
        min.max(div_round(n * qw, qwd)) as u32
    };
    (scale(64, num.0), scale(32, num.1), scale(16, num.2))
}

// ---------------------------------------------------------------------------
// Prediction helpers
// ---------------------------------------------------------------------------

/// Per-unit geometry the directional predictor needs beyond the plane
/// coords: the CODED BLOCK's luma mi position/dims (availability tables),
/// the plane subsampling, and the LUMA frame dims.
#[derive(Clone, Copy)]
pub(crate) struct UnitGeom {
    pub mi_row: usize,
    pub mi_col: usize,
    pub bw_px: usize,
    pub bh_px: usize,
    pub ss: usize,
    pub frame_w: usize,
    pub frame_h: usize,
}

/// Predict one intra mode (any of the 13 C modes + angle delta, or
/// FILTER_DC) for a whole prediction unit at absolute plane coords,
/// reading the live recon plane with the C edge-fill rules
/// (`svt_av1_intra_prediction` -> `build_intra_predictors`).
///
/// Non-directional modes and V/H at delta 0 (p_angle exactly 90/180 —
/// the decoder's edge filter skips them) use the extract_neighbors fills;
/// all other directional predictions run `intra_edge::dr_predict`, which
/// applies the SH-gated corner/edge filters + upsampling
/// (`edge_filter`, `filt_type` = C `get_filt_type`).
#[allow(clippy::too_many_arguments)]
fn predict_unit(
    recon: &[u8],
    stride: usize,
    abs_x: usize,
    abs_y: usize,
    w: usize,
    h: usize,
    mode: u8,
    delta: i8,
    fi_mode: u8,
    geom: &UnitGeom,
    edge_filter: bool,
    filt_type: i32,
    dst: &mut [u8],
) {
    use svtav1_dsp::intra_pred as ip;
    if matches!(mode, 3..=8) || (matches!(mode, 1 | 2) && delta != 0) {
        let p_angle = crate::intra_edge::MODE_TO_ANGLE_MAP[mode as usize] + delta as i32 * 3;
        debug_assert!(fi_mode == FI_NONE);
        let g = crate::intra_edge::DrGeom {
            px: abs_x,
            py: abs_y,
            txw: w,
            txh: h,
            mi_row: geom.mi_row,
            mi_col: geom.mi_col,
            bw_px: geom.bw_px,
            bh_px: geom.bh_px,
            row_off: 0,
            col_off: 0,
            ss: geom.ss,
            frame_w: geom.frame_w,
            frame_h: geom.frame_h,
        };
        crate::intra_edge::dr_predict(
            |x, y| recon[y * stride + x],
            &g,
            p_angle,
            edge_filter,
            filt_type,
            svtav1_types::partition::PartitionType::None,
            dst,
        );
        return;
    }
    let (above, left, top_left, has_above, has_left) =
        crate::partition::extract_neighbors(recon, stride, abs_x, abs_y, w, h);
    if fi_mode != FI_NONE {
        let mut above_c = vec![0u8; w + 1];
        above_c[0] = if has_above && has_left {
            top_left
        } else if has_above {
            above[0]
        } else if has_left {
            left[0]
        } else {
            128
        };
        above_c[1..].copy_from_slice(&above);
        ip::predict_filter_intra(dst, w, &above_c, &left, w, h, fi_mode);
        return;
    }
    match mode {
        0 => ip::predict_dc(dst, w, &above, &left, w, h, has_above, has_left),
        1 => ip::predict_v(dst, w, &above, w, h),
        2 => ip::predict_h(dst, w, &left, w, h),
        9 => ip::predict_smooth(dst, w, &above, &left, w, h),
        10 => ip::predict_smooth_v(dst, w, &above, &left, h, h, w),
        11 => ip::predict_smooth_h(dst, w, &above, &left, w, h),
        12 => ip::predict_paeth(dst, w, &above, &left, top_left, w, h),
        m => unreachable!("funnel mode {m}"),
    }
}

/// C `hadamard_path` (product_coding_loop.c:1187): residual over square
/// tiles of `MIN(TX_32X32, eb_max_txsize_lookup[bsize])` — the largest
/// square TX fitting the block (its MIN dimension), capped at 32 — aom
/// Hadamard per tile, SATD accumulated (raster tile order).
fn hadamard_satd(
    src: &[u8],
    src_stride: usize,
    src_off: usize,
    pred: &[u8],
    w: usize,
    h: usize,
) -> u64 {
    let tx = w.min(h).min(32);
    let mut satd: u64 = 0;
    let mut res = vec![0i16; tx * tx];
    let mut coeff = vec![0i32; tx * tx];
    for ty in (0..h).step_by(tx) {
        for tx_x in (0..w).step_by(tx) {
            for r in 0..tx {
                let srow = src_off + (ty + r) * src_stride + tx_x;
                let prow = (ty + r) * w + tx_x;
                for c in 0..tx {
                    res[r * tx + c] = src[srow + c] as i16 - pred[prow + c] as i16;
                }
            }
            match tx {
                4 => svtav1_dsp::hadamard::aom_hadamard_4x4(&res, tx, &mut coeff),
                8 => svtav1_dsp::hadamard::aom_hadamard_8x8(&res, tx, &mut coeff),
                16 => svtav1_dsp::hadamard::aom_hadamard_16x16(&res, tx, &mut coeff),
                32 => svtav1_dsp::hadamard::aom_hadamard_32x32(&res, tx, &mut coeff),
                _ => unreachable!("hadamard tile {tx}"),
            }
            satd += svtav1_dsp::hadamard::aom_satd(&coeff) as u64;
        }
    }
    satd
}

// ---------------------------------------------------------------------------
// Coefficient rate (svt_av1_cost_coeffs_txb, full scan, real contexts)
// ---------------------------------------------------------------------------

/// C `svt_av1_cost_coeffs_txb` (rd_cost.c:355) at
/// `mds_fast_coeff_est_level = 1` (FULL middle loop), arbitrary plane /
/// tx type / contexts. `eob > 0`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn cost_coeffs_txb(
    qcoeff: &[i32],
    eob: u16,
    c_tx_size: usize,
    tx_type: usize,
    plane_type: usize,
    txb_skip_ctx: usize,
    dc_sign_ctx: usize,
    intra_dir: usize,
    rates: &MdRates,
) -> i32 {
    debug_assert!(eob > 0);
    let tx_class = cc::TX_TYPE_TO_CLASS[tx_type];
    let txs_ctx = cc::txsize_entropy_ctx(c_tx_size);
    let bwl = cc::txb_bwl(c_tx_size);
    let width = cc::txb_wide(c_tx_size);
    let height = cc::txb_high(c_tx_size);
    let scan = svtav1_entropy::scan_tables::scan(
        c_tx_size,
        svtav1_entropy::scan_tables::TX_TYPE_TO_SCAN_INDEX[tx_type] as usize,
    );
    let costs = rates.coeff.txb(txs_ctx, plane_type);
    let eob_bits = &rates.coeff.eob[cc::TXSIZE_LOG2_MINUS4[c_tx_size]][plane_type];

    let mut cost = costs.txb_skip_cost[txb_skip_ctx][0];
    let mut levels_buf = vec![0u8; cc::TX_PAD_2D];
    if eob > 1 {
        cc::txb_init_levels(qcoeff, width, height, &mut levels_buf);
    }
    if plane_type == 0 {
        cost += rates.txt_rate(c_tx_size, intra_dir, tx_type);
    }
    cost += crate::quant::eob_cost(eob as i32, eob_bits, costs, tx_class);

    let mut coeff_contexts = vec![0i8; width * height];
    cc::get_nz_map_contexts(
        &levels_buf,
        scan,
        eob as usize,
        c_tx_size,
        tx_class,
        &mut coeff_contexts,
    );

    let lit = 512i32; // av1_cost_literal(1)
    let eob_us = eob as usize;

    let level_cost =
        |cost: &mut i32, pos: usize, v: i32, is_eob_pos: bool, is_dc: bool, levels_buf: &[u8]| {
            let level = v.unsigned_abs() as i32;
            let coeff_ctx = coeff_contexts[pos] as usize;
            if is_eob_pos {
                *cost += costs.base_eob_cost[coeff_ctx][(level.min(3) - 1) as usize];
            } else {
                *cost += costs.base_cost[coeff_ctx][level.min(3) as usize];
            }
            if v != 0 {
                if is_dc {
                    let sign = usize::from(v < 0);
                    *cost += costs.dc_sign_cost[dc_sign_ctx][sign];
                } else {
                    *cost += lit;
                }
                if level > cc::NUM_BASE_LEVELS {
                    let ctx = cc::br_ctx(levels_buf, pos, bwl, tx_class);
                    let base_range = level - 1 - cc::NUM_BASE_LEVELS;
                    if base_range < cc::COEFF_BASE_RANGE {
                        *cost += costs.lps_cost[ctx][base_range as usize];
                    } else {
                        *cost += costs.lps_cost[ctx][cc::COEFF_BASE_RANGE as usize];
                    }
                    if level >= 1 + cc::NUM_BASE_LEVELS + cc::COEFF_BASE_RANGE {
                        *cost += crate::quant::golomb_cost(level);
                    }
                }
            }
        };

    if eob_us == 1 {
        level_cost(&mut cost, 0, qcoeff[0], true, true, &levels_buf);
        return cost;
    }
    // eob - 1 (base_eob context), then DC, then the full middle loop —
    // av1_cost_coeffs_txb_loop_cost_eob with fast level 1 => every
    // position is priced.
    {
        let pos = scan[eob_us - 1] as usize;
        level_cost(&mut cost, pos, qcoeff[pos], true, false, &levels_buf);
    }
    level_cost(&mut cost, 0, qcoeff[0], false, true, &levels_buf);
    for c in (1..=eob_us - 2).rev() {
        let pos = scan[c] as usize;
        let v = qcoeff[pos];
        let level = v.unsigned_abs() as i32;
        if v != 0 {
            cost += lit;
        }
        if level > cc::NUM_BASE_LEVELS {
            let ctx = cc::br_ctx(&levels_buf, pos, bwl, tx_class);
            let base_range = level - 1 - cc::NUM_BASE_LEVELS;
            cost += costs.base_cost[coeff_contexts[pos] as usize][3];
            if base_range < cc::COEFF_BASE_RANGE {
                cost += costs.lps_cost[ctx][base_range as usize];
            } else {
                cost += crate::quant::golomb_cost(level)
                    + costs.lps_cost[ctx][cc::COEFF_BASE_RANGE as usize];
            }
        } else {
            cost += costs.base_cost[coeff_contexts[pos] as usize][level as usize];
        }
    }
    cost
}

/// C `av1_cost_skip_txb` (rd_cost.c:213): the eob == 0 txb rate.
pub(crate) fn cost_skip_txb(
    c_tx_size: usize,
    plane_type: usize,
    txb_skip_ctx: usize,
    rates: &MdRates,
) -> i32 {
    let txs_ctx = cc::txsize_entropy_ctx(c_tx_size);
    rates.coeff.txb(txs_ctx, plane_type).txb_skip_cost[txb_skip_ctx][1]
}

// ---------------------------------------------------------------------------
// TX pipeline for one transform unit
// ---------------------------------------------------------------------------

struct TxUnitOut {
    eob: u16,
    /// Packed (32-capped) quantized levels.
    qcoeff: Vec<i32>,
    /// Reconstructed pixels (w x h raster).
    recon: Vec<u8>,
    /// Frequency-domain RESIDUAL distortion (MDS1 path) or spatial SSE
    /// << 4 (MDS3 path), already shifted like C.
    dist: u64,
    /// Coefficient bits (or skip-txb bits when eob == 0).
    bits: i32,
    /// `(dc_sign << 6) | min(cul_level, 63)` neighbor byte.
    cul: u8,
}

impl TxUnitOut {
    /// The no-chroma placeholder for `has_uv == 0` blocks: C never runs
    /// the chroma full loop there, so every chroma term is EXACTLY zero
    /// (no skip-txb rate either — the syntax doesn't exist).
    fn absent() -> Self {
        TxUnitOut {
            eob: 0,
            qcoeff: Vec::new(),
            recon: Vec::new(),
            dist: 0,
            bits: 0,
            cul: 0,
        }
    }
}

/// C `svt_av1_compute_cul_level` (full_loop.c:1356).
fn compute_cul_level(scan: &[u16], qcoeff: &[i32], eob: u16) -> u8 {
    let mut cul: u32 = 0;
    for c in 0..eob as usize {
        cul += qcoeff[scan[c] as usize].unsigned_abs();
        if cul >= 63 {
            break;
        }
    }
    cul = cul.min(63);
    let dc = if eob > 0 { qcoeff[0] } else { 0 };
    if dc < 0 {
        cul |= 1 << 6;
    } else if dc > 0 {
        cul += 2 << 6;
    }
    cul as u8
}

/// Forward transform + (optional RDOQ) quantize + inverse recon + dist +
/// coeff bits for one TX unit. Mirrors the DCT/TXT iteration body of
/// `tx_type_search` / `perform_dct_dct_tx` / `svt_aom_full_loop_uv`.
///
/// `spatial_dist`: MDS3 (recon vs source SSE << 4); else the MDS1
/// freq-domain path. `do_rdoq` follows C `mds_do_rdoq && rdoq enabled`.
#[allow(clippy::too_many_arguments)]
fn tx_unit(
    src: &[u8],
    src_stride: usize,
    src_off: usize,
    pred: &[u8],
    pred_stride: usize,
    pred_off: usize,
    w: usize,
    h: usize,
    tx_type: usize,
    plane_type: usize,
    txb_skip_ctx: usize,
    dc_sign_ctx: usize,
    intra_dir: usize,
    qt: &QuantTable,
    frame: &FunnelFrame,
    rates: &MdRates,
    do_rdoq: bool,
    spatial_dist: bool,
) -> TxUnitOut {
    let n = w * h;
    let c_tx = cc::tx_size_from_dims(w, h);
    let rs_tx_type = TX_TYPE_FROM_C[tx_type];

    let mut residual = vec![0i32; n];
    for r in 0..h {
        let srow = src_off + r * src_stride;
        let prow = pred_off + r * pred_stride;
        for c in 0..w {
            residual[r * w + c] = src[srow + c] as i32 - pred[prow + c] as i32;
        }
    }
    let mut coeffs = vec![0i32; n];
    let ok = svtav1_dsp::txfm_dispatch::fwd_txfm2d_dispatch(
        &residual,
        &mut coeffs,
        w,
        rs_tx_size(w, h),
        rs_tx_type,
    );
    debug_assert!(ok, "fwd txfm {w}x{h} type {tx_type}");

    // 64-dim fold (svt_handle_transform64x64) + energy of discarded coeffs.
    let mut three_quad_energy: u64 = 0;
    let (pw, ph) = (w.min(32), h.min(32));
    let mut packed = if w > 32 || h > 32 {
        if w == 64 && h == 64 {
            three_quad_energy = energy_region(&coeffs[32..], 64, 32, 32)
                + energy_region(&coeffs[32 * 64..], 64, 64, 32);
        } else if w == 64 {
            // 64x32 / 64x16: top-right (w-32)-wide, h-tall region
            // (svt_handle_transform64x32_c / 64x16_c, transforms.c:3223).
            three_quad_energy = energy_region(&coeffs[32..], 64, 32, h.min(32));
        } else {
            // 32x64 / 16x64: bottom w-wide, (h-32)-tall region.
            three_quad_energy = energy_region(&coeffs[32 * w..], w, w, h - 32);
        }
        let mut v = vec![0i32; pw * ph];
        for r in 0..ph {
            v[r * pw..(r + 1) * pw].copy_from_slice(&coeffs[r * w..r * w + pw]);
        }
        v
    } else {
        coeffs.clone()
    };

    let scan = svtav1_entropy::scan_tables::scan(
        c_tx,
        svtav1_entropy::scan_tables::TX_TYPE_TO_SCAN_INDEX[tx_type] as usize,
    );
    let log_scale = TX_SCALE_TAB[c_tx];
    let mut qcoeff = vec![0i32; pw * ph];
    let mut dqcoeff = vec![0i32; pw * ph];
    let mut eob = if do_rdoq {
        let mut e =
            crate::quant::quantize_fp(&packed, scan, qt, log_scale, &mut qcoeff, &mut dqcoeff);
        if e != 0 {
            let (cut_off_num, cut_off_denum) = crate::quant::rdoq_cutoffs(frame.rdoq_level);
            let tx_class = cc::TX_TYPE_TO_CLASS[tx_type];
            let o = crate::quant::OptimizeCtx {
                txb_costs: rates.coeff.txb(cc::txsize_entropy_ctx(c_tx), plane_type),
                eob_costs: &rates.coeff.eob[cc::TXSIZE_LOG2_MINUS4[c_tx]][plane_type],
                rdmult: crate::quant::rdoq_rdmult(frame.lambda as u32, plane_type),
                tx_size: c_tx,
                tx_class,
                txb_skip_ctx,
                dc_sign_ctx,
                cut_off_num,
                cut_off_denum,
            };
            crate::quant::optimize_b(&packed, &mut qcoeff, &mut dqcoeff, &mut e, scan, qt, &o);
        }
        e
    } else {
        crate::quant::quantize_b(&packed, scan, qt, log_scale, &mut qcoeff, &mut dqcoeff)
    };
    let _ = &mut packed;
    let _ = &mut eob;

    // Reconstruction (needed for spatial dist AND for depth-1 neighbor
    // prediction — C inverts whenever spatial SSE or intra tx_depth > 0).
    let mut recon = vec![0u8; n];
    if eob > 0 {
        let mut dq_full = vec![0i32; n];
        for r in 0..ph {
            dq_full[r * w..r * w + pw].copy_from_slice(&dqcoeff[r * pw..(r + 1) * pw]);
        }
        let mut inv = vec![0i32; n];
        let ok = svtav1_dsp::txfm_dispatch::inv_txfm2d_dispatch(
            &dq_full,
            &mut inv,
            w,
            rs_tx_size(w, h),
            rs_tx_type,
        );
        debug_assert!(ok, "inv txfm {w}x{h} type {tx_type}");
        for r in 0..h {
            let prow = pred_off + r * pred_stride;
            for c in 0..w {
                recon[r * w + c] = (pred[prow + c] as i32 + inv[r * w + c]).clamp(0, 255) as u8;
            }
        }
    } else {
        for r in 0..h {
            let prow = pred_off + r * pred_stride;
            recon[r * w..(r + 1) * w].copy_from_slice(&pred[prow..prow + w]);
        }
    }

    let dist = if spatial_dist {
        let mut sse: u64 = 0;
        for r in 0..h {
            let srow = src_off + r * src_stride;
            for c in 0..w {
                let d = src[srow + c] as i64 - recon[r * w + c] as i64;
                sse += (d * d) as u64;
            }
        }
        sse << 4
    } else {
        // Freq-domain: svt_aom_picture_full_distortion32_bits_single
        // (RESIDUAL) + three_quad + RIGHT_SIGNED_SHIFT((1 - scale) * 2).
        let mut d: u64 = 0;
        if eob > 0 {
            for i in 0..pw * ph {
                let e = (packed[i] - dqcoeff[i]) as i64;
                d += (e * e) as u64;
            }
        } else {
            for i in 0..pw * ph {
                d += (packed[i] as i64 * packed[i] as i64) as u64;
            }
        }
        d += three_quad_energy;
        let shift = (1 - log_scale as i32) * 2;
        if shift < 0 { d << (-shift) } else { d >> shift }
    };

    let bits = if eob > 0 {
        cost_coeffs_txb(
            &qcoeff,
            eob,
            c_tx,
            tx_type,
            plane_type,
            txb_skip_ctx,
            dc_sign_ctx,
            intra_dir,
            rates,
        )
    } else {
        cost_skip_txb(c_tx, plane_type, txb_skip_ctx, rates)
    };
    let cul = compute_cul_level(scan, &qcoeff, eob);

    TxUnitOut {
        eob,
        qcoeff,
        recon,
        dist,
        bits,
        cul,
    }
}

fn energy_region(coeffs: &[i32], stride: usize, w: usize, h: usize) -> u64 {
    let mut e: u64 = 0;
    for r in 0..h {
        for c in 0..w {
            let v = coeffs[r * stride + c] as i64;
            e += (v * v) as u64;
        }
    }
    e
}

use crate::quant::TX_SCALE_TAB;

/// C TxType index -> Rust TxType (identical numbering).
const TX_TYPE_FROM_C: [svtav1_types::transform::TxType; 16] = {
    use svtav1_types::transform::TxType::*;
    [
        DctDct,
        AdstDct,
        DctAdst,
        AdstAdst,
        FlipAdstDct,
        DctFlipAdst,
        FlipAdstFlipAdst,
        AdstFlipAdst,
        FlipAdstAdst,
        Idtx,
        VDct,
        HDct,
        VAdst,
        HAdst,
        VFlipAdst,
        HFlipAdst,
    ]
};

fn rs_tx_size(w: usize, h: usize) -> svtav1_types::transform::TxSize {
    use svtav1_types::transform::TxSize;
    match (w, h) {
        (4, 4) => TxSize::Tx4x4,
        (8, 8) => TxSize::Tx8x8,
        (16, 16) => TxSize::Tx16x16,
        (32, 32) => TxSize::Tx32x32,
        (64, 64) => TxSize::Tx64x64,
        (4, 8) => TxSize::Tx4x8,
        (8, 4) => TxSize::Tx8x4,
        (8, 16) => TxSize::Tx8x16,
        (16, 8) => TxSize::Tx16x8,
        (16, 32) => TxSize::Tx16x32,
        (32, 16) => TxSize::Tx32x16,
        (32, 64) => TxSize::Tx32x64,
        (64, 32) => TxSize::Tx64x32,
        (4, 16) => TxSize::Tx4x16,
        (16, 4) => TxSize::Tx16x4,
        (8, 32) => TxSize::Tx8x32,
        (32, 8) => TxSize::Tx32x8,
        (16, 64) => TxSize::Tx16x64,
        (64, 16) => TxSize::Tx64x16,
        _ => unreachable!("funnel tx {w}x{h}"),
    }
}

// ---------------------------------------------------------------------------
// The funnel
// ---------------------------------------------------------------------------

/// C `intra_luma_to_chroma` (mode_decision.c:42) — identity mapping.
#[inline]
fn uv_from_y(mode: u8) -> u8 {
    mode
}

/// C `fimode_to_intradir` (common_utils.c:33).
const FIMODE_TO_INTRADIR: [u8; 5] = [0, 1, 2, 6, 0];

/// One funnel candidate's evolving state.
struct Cand {
    mode: u8,
    /// Luma angle delta (directional modes only; C ANGLE_STEP units).
    delta: i8,
    fi: u8,
    uv: u8,
    /// Chroma angle delta (= luma delta at injection; rewritten by the
    /// ind-uv MDS3 update at chroma_level 4).
    uv_delta: i8,
    /// Whole-block depth-0 luma prediction (w x h).
    pred: Vec<u8>,
    flr: u64,
    fcr: u64,
    fast_cost: u64,
    // MDS1:
    full_cost: u64,
    mds1_has_coeff: bool,
    // MDS3 winner data:
    tx_depth: u8,
    txb_q: Vec<Vec<i32>>,
    txb_eob: Vec<u16>,
    txb_cul: Vec<u8>,
    txb_type: Vec<u8>,
    y_recon: Vec<u8>,
    y_bits: u64,
    y_dist: u64,
    u_q: Vec<i32>,
    v_q: Vec<i32>,
    u_eob: u16,
    v_eob: u16,
    u_cul: u8,
    v_cul: u8,
    u_recon: Vec<u8>,
    v_recon: Vec<u8>,
    mds3_cost: u64,
    block_has_coeff: bool,
    /// C `blk_ptr->total_rate` / `full_dist` (svt_aom_full_cost writeback)
    /// — read by the NSQ component-multiple / recon-dist gates.
    total_rate: u64,
    full_dist: u64,
}

/// The chosen leaf coding, consumed by the fixed-tree walk + the entropy
/// pass.
pub struct LeafChoice {
    pub mode: u8,
    /// Luma angle delta (0 for non-directional modes).
    pub angle_delta: i8,
    pub fi_mode: u8,
    pub uv_mode: u8,
    /// Chroma angle delta (0 unless the ind-uv search picked one).
    pub uv_angle_delta: i8,
    pub tx_depth: u8,
    /// Per-txb packed quantized levels (1 txb at depth 0, 4 at depth 1),
    /// in raster txb order.
    pub txb_qcoeffs: Vec<Vec<i32>>,
    pub txb_eobs: Vec<u16>,
    /// Per-txb C TxType indices (winner of the per-txb TXT search).
    pub txb_tx_types: Vec<u8>,
    pub u_qcoeffs: Vec<i32>,
    pub v_qcoeffs: Vec<i32>,
    pub u_eob: u16,
    pub v_eob: u16,
    /// The winner's reconstructed chroma blocks (cw x ch rasters) — the
    /// entropy walk copies these into its chroma planes so the walk's
    /// recon evolution is byte-identical to the decision phase's.
    pub u_recon: Vec<u8>,
    pub v_recon: Vec<u8>,
}

/// Per-frame/SB mutable funnel context threaded through the fixed tree.
pub(crate) struct FunnelCtx<'a> {
    pub u_src: &'a [u8],
    pub v_src: &'a [u8],
    pub u_recon: &'a mut [u8],
    pub v_recon: &'a mut [u8],
    pub c_stride: usize,
    pub ectx: &'a mut crate::pipeline::EntropyCtx,
    pub rates: &'a MdRates,
    pub frame: &'a FunnelFrame,
}

/// One evaluated (not yet committed) PART_N funnel decision — the C
/// `md_encode_block` output before `md_update_all_neighbour_arrays`
/// commits it. The PD1 depth walk evaluates parent and child depths and
/// only commits the depth that wins the inter-depth compare.
pub(crate) struct LeafEval {
    pub abs_x: usize,
    pub abs_y: usize,
    pub w: usize,
    pub h: usize,
    /// C `ctx->has_uv` (is_chroma_reference) + the chroma PAIR geometry
    /// (bsize_uv dims at the ROUND_UV origin) — sub-8 NSQ children only
    /// deviate from (x/2, y/2, w/2, h/2).
    has_uv: bool,
    ccx: usize,
    ccy: usize,
    cw: usize,
    chh: usize,
    win: Cand,
    /// C `cand_bf->residual` content at `non_normative_txs` time: ALL
    /// MDS3 candidates share ONE residual workspace (verified by buffer-
    /// pointer instrumentation — docs/captures/nsq_m2m3), so the buffer
    /// holds the LAST MDS3-processed candidate's whole-block DEPTH-0
    /// residual (the depth-1/2 trials write the per-depth scratch
    /// buffers, init_tx_cand_bf copies OUT of this one).
    psq_resid: Vec<i32>,
}

impl LeafEval {
    /// The winner's MDS3 full cost (C `blk_ptr->cost` before the
    /// partition-rate term the depth walk adds).
    pub(crate) fn block_cost(&self) -> u64 {
        self.win.mds3_cost
    }

    /// C `cnt_nz_coeff` (sum of the winner's luma txb eobs,
    /// product_coding_loop.c:7166-7168).
    pub(crate) fn cnt_nz_coeff(&self) -> u32 {
        self.win.txb_eob.iter().map(|&e| e as u32).sum()
    }

    /// C `blk_ptr->total_rate` (the winner's full rate) and `full_dist`
    /// — inputs to the NSQ component-multiple gate.
    pub(crate) fn total_rate(&self) -> u64 {
        self.win.total_rate
    }

    pub(crate) fn full_dist(&self) -> u64 {
        self.win.full_dist
    }

    /// Winner luma mode (C `block_mi.mode`) — the NSQ recon-dist gate's
    /// modulation input.
    pub(crate) fn mode(&self) -> u8 {
        self.win.mode
    }

    pub(crate) fn block_has_coeff(&self) -> bool {
        self.win.block_has_coeff
    }

    /// The shared MDS3 residual-workspace state (C `cand_bf->residual`,
    /// consumed by the psq gate): the LAST MDS3 candidate's depth-0
    /// residual.
    pub(crate) fn psq_resid(&self) -> &[i32] {
        &self.psq_resid
    }

    /// Winner luma recon (w x h raster).
    pub(crate) fn y_recon(&self) -> &[u8] {
        &self.win.y_recon
    }

    /// Winner chroma recons ((size/2)^2 rasters).
    pub(crate) fn uv_recon(&self) -> (&[u8], &[u8]) {
        (&self.win.u_recon, &self.win.v_recon)
    }

    /// The walk/entropy-pass view of the winner.
    pub(crate) fn to_choice(&self) -> LeafChoice {
        let cand = &self.win;
        LeafChoice {
            mode: cand.mode,
            angle_delta: cand.delta,
            fi_mode: cand.fi,
            uv_mode: cand.uv,
            uv_angle_delta: cand.uv_delta,
            tx_depth: cand.tx_depth,
            txb_qcoeffs: cand.txb_q.clone(),
            txb_eobs: cand.txb_eob.clone(),
            txb_tx_types: cand.txb_type.clone(),
            u_qcoeffs: cand.u_q.clone(),
            v_qcoeffs: cand.v_q.clone(),
            u_eob: cand.u_eob,
            v_eob: cand.v_eob,
            u_recon: cand.u_recon.clone(),
            v_recon: cand.v_recon.clone(),
        }
    }
}

/// Decide one PART_N leaf of the fixed tree — the full MDS0/MDS1/MDS3
/// funnel — and commit the winner (luma recon into `y_recon`, chroma into
/// the funnel's decision planes, all neighbor context updates).
#[allow(clippy::too_many_arguments)]
pub(crate) fn decide_leaf(
    fx: &mut FunnelCtx<'_>,
    y_src: &[u8],
    y_src_stride: usize,
    y_src_off: usize,
    y_recon: &mut [u8],
    y_stride: usize,
    abs_x: usize,
    abs_y: usize,
    size: usize,
    dc_only: bool,
) -> LeafChoice {
    let ev = evaluate_leaf(
        fx,
        y_src,
        y_src_stride,
        y_src_off,
        y_recon,
        y_stride,
        abs_x,
        abs_y,
        size,
        size,
        dc_only,
    );
    let choice = ev.to_choice();
    commit_leaf(fx, y_recon, y_stride, &ev);
    choice
}

/// Evaluate one PART_N block through the funnel WITHOUT committing —
/// C `md_encode_block` (the neighbour arrays / MD recon planes are
/// untouched; the caller commits the winning depth via [`commit_leaf`]).
#[allow(clippy::too_many_arguments)]
pub(crate) fn evaluate_leaf(
    fx: &FunnelCtx<'_>,
    y_src: &[u8],
    y_src_stride: usize,
    y_src_off: usize,
    y_recon: &[u8],
    y_stride: usize,
    abs_x: usize,
    abs_y: usize,
    w: usize,
    h: usize,
    // eff-M9: `is_dc_only_safe` fired for this block -> C's dc_cand_only
    // injection restricts the candidate list to {DC_PRED}
    // (mode_decision.c:3633). Always false at M6/M7/M8 (gate dead).
    dc_only: bool,
) -> LeafEval {
    let frame = fx.frame;
    let rates = fx.rates;
    let lambda = frame.lambda;
    let qt = crate::quant::build_quant_table(frame.base_qindex);

    // -- Block-level contexts (svt_aom_coding_loop_context_generation) --
    // Intra-mode and tx-size contexts are always neighbour-derived; the
    // skip_coeff context is only real when `update_skip_coeff_ctx` is set
    // (rate_est_level 1 at M6). M7/M8 (rate_est_level 4) price it at ctx 0.
    let above_ctx = fx.ectx.above_mode_ctx(abs_x);
    let left_ctx = fx.ectx.left_mode_ctx(abs_y);
    let skip_ctx = if fx.frame.cfg.real_coeff_ctx {
        fx.ectx.skip_ctx(abs_x, abs_y)
    } else {
        0
    };
    let fi_allowed_bsize = w <= 32 && h <= 32;
    let bsize_idx = svtav1_entropy::context::block_size_index(w, h);
    let cfl_allowed = usize::from(w <= 32 && h <= 32);
    let use_angle = !matches!((w, h), (4, 4) | (4, 8) | (8, 4));
    // C `is_chroma_reference(mi_row, mi_col, bsize, 1, 1)`
    // (common_utils.h:315): sub-8 blocks carry chroma only at odd mi in
    // the sub-8 dimension; the chroma block then covers the PAIR
    // (bsize_uv dims = max(dim,8)/2 at the ROUND_UV origin).
    let has_uv = ((abs_y / 4) % 2 == 1 || (h / 4) % 2 == 0)
        && ((abs_x / 4) % 2 == 1 || (w / 4) % 2 == 0);

    // Block geometry for the directional predictor (availability tables +
    // frame-edge clamps) and the per-block C `get_filt_type` inputs (the
    // above/left CODED-BLOCK modes' smoothness, per plane).
    let frame_h_px = y_recon.len() / y_stride;
    let y_geom = UnitGeom {
        mi_row: abs_y >> 2,
        mi_col: abs_x >> 2,
        bw_px: w,
        bh_px: h,
        ss: 0,
        frame_w: y_stride,
        frame_h: frame_h_px,
    };
    let uv_geom = UnitGeom { ss: 1, ..y_geom };
    let filt_type_y = fx.ectx.filt_type_y(abs_x, abs_y);
    let filt_type_uv = fx.ectx.filt_type_uv(abs_x, abs_y);

    // -- Candidate injection + MDS0 --
    // C order (`generate_md_stage_0_cand`): regular intra modes DC ..
    // intra_mode_end with the angular-delta inner loop in counter order
    // (-3..3, level >= 2 keeping {-3, 0, +3}; inject_intra_candidates,
    // mode_decision.c:3254-3271), then filter-intra
    // (inject_filter_intra_candidates — FILTER_DC only at fi level 2).
    let cfg = frame.cfg;
    let fi_elig = cfg.filter_intra && fi_allowed_bsize;
    let mut cand_modes: Vec<(u8, i8, u8)> = Vec::new();
    if dc_only {
        // eff-M9 dc_cand_only injection: exactly {DC_PRED}, no filter-intra.
        cand_modes.push((0, 0, FI_NONE));
    } else {
        for mode in 0..=cfg.mode_end {
            let directional = matches!(mode, 1..=8);
            // directional_mode_skip_mask at angular_pred_level >= 4 masks
            // D45_PRED (3) .. D67_PRED (8) — V/H stay
            // (inject_intra_candidates, mode_decision.c:3246-3250).
            if matches!(mode, 3..=8) && cfg.angular_level >= 4 {
                continue;
            }
            if directional && cfg.angular_level <= 2 && use_angle {
                for d in -3i8..=3 {
                    if cfg.angular_level >= 2 && matches!(d, -2 | -1 | 1 | 2) {
                        continue;
                    }
                    cand_modes.push((mode, d, FI_NONE));
                }
            } else {
                cand_modes.push((mode, 0, FI_NONE));
            }
        }
    }
    if fi_elig && !dc_only {
        cand_modes.push((0, 0, 0)); // FILTER_DC_PRED
    }

    let mut cands: Vec<Cand> = Vec::with_capacity(cand_modes.len());
    // MDS0 with `prune_using_best_mode` (product_coding_loop.c:1680-1737):
    // candidates are evaluated in injection order; the running best REGULAR
    // (class-0, non-filter-intra) mode by fast cost is tracked and used to
    // SKIP later candidates — H when V is currently best, SMOOTH when DC is
    // still best. Skipped candidates never get a fast cost (never enter the
    // pool). At M6 (prune off) every candidate is evaluated, identical to
    // the original funnel.
    let mut best_reg_cost = u64::MAX;
    let mut best_reg_mode: i32 = -1;
    for &(mode, delta, fi) in &cand_modes {
        if cfg.prune_best_mode && fi == FI_NONE {
            // intra_mode_end SMOOTH >= H_PRED, so the gate is armed.
            if mode == 2 && best_reg_mode == 1 {
                continue; // V better than DC -> skip H
            }
            if mode == 9 && best_reg_mode == 0 {
                continue; // DC still best -> skip SMOOTH
            }
        }
        // uv = intra_luma_to_chroma[mode] with the SAME angle delta
        // (ind_uv_avail is 0 at injection — mode_decision.c:3280; the
        // ind-uv rewrite happens at MDS3 via update_intra_chroma_mode).
        let uv = uv_from_y(if fi != FI_NONE { 0 } else { mode });
        let uv_delta = if fi != FI_NONE { 0 } else { delta };
        let mut pred = vec![0u8; w * h];
        predict_unit(
            y_recon,
            y_stride,
            abs_x,
            abs_y,
            w,
            h,
            mode,
            delta,
            fi,
            &y_geom,
            cfg.edge_filter,
            filt_type_y,
            &mut pred,
        );
        let satd = hadamard_satd(y_src, y_src_stride, y_src_off, &pred, w, h);

        let mut flr = rates.kf_y[above_ctx][left_ctx][mode as usize] as u64;
        if use_angle && matches!(mode, 1..=8) {
            flr += rates.angle[mode as usize - 1][(3 + delta) as usize] as u64;
        }
        if fi_elig && mode == 0 {
            flr += rates.fi_flag[bsize_idx][usize::from(fi != FI_NONE)] as u64;
            if fi != FI_NONE {
                flr += rates.fi_mode[fi as usize] as u64;
            }
        }
        let mut fcr = if has_uv {
            rates.uv[cfl_allowed][mode as usize][uv as usize] as u64
        } else {
            // C fast cost: chroma_rate only when ctx->has_uv
            // (av1_intra_fast_cost, rd_cost.c:619).
            0
        };
        if has_uv && use_angle && matches!(uv, 1..=8) {
            fcr += rates.angle[uv as usize - 1][(3 + uv_delta) as usize] as u64;
        }
        let fast_cost = rdcost(lambda, flr + fcr, satd << 4);
        // C updates best_reg_intra_mode after fast_loop_core for regular
        // class-0 candidates when prune is armed (line 1727).
        if cfg.prune_best_mode && fi == FI_NONE && fast_cost < best_reg_cost {
            best_reg_cost = fast_cost;
            best_reg_mode = mode as i32;
        }
        cands.push(Cand {
            mode,
            delta,
            fi,
            uv,
            uv_delta,
            pred,
            flr,
            fcr,
            fast_cost,
            full_cost: u64::MAX,
            mds1_has_coeff: false,
            tx_depth: 0,
            txb_q: Vec::new(),
            txb_eob: Vec::new(),
            txb_cul: Vec::new(),
            txb_type: Vec::new(),
            y_recon: Vec::new(),
            y_bits: 0,
            y_dist: 0,
            u_q: Vec::new(),
            v_q: Vec::new(),
            u_eob: 0,
            v_eob: 0,
            u_cul: 0,
            v_cul: 0,
            u_recon: Vec::new(),
            v_recon: Vec::new(),
            mds3_cost: u64::MAX,
            block_has_coeff: false,
            total_rate: 0,
            full_dist: 0,
        });
    }
    let ncand = cands.len();

    // -- Sort by fast cost (stable == C's strict-less bubble) --
    let mut order: Vec<usize> = (0..ncand).collect();
    order.sort_by_key(|&i| cands[i].fast_cost);
    let mds0_best_idx = order[0];

    // -- post_mds0_nic_pruning (product_coding_loop.c:8045) --
    let (nic1, nic2, nic3) = nic_counts(frame.cli_qp, cfg.nic_num);
    let (qw, qwd) = qp_scale_factors(frame.cli_qp);
    let mds1_cand_th = div_round(cfg.mds1_cand_base_th * qw, qwd);
    let mut n1 = (ncand as u32).min(nic1) as usize;
    {
        let best = cands[order[0]].fast_cost;
        let mut count = 1usize;
        if best > 0 {
            while count < n1 {
                let dev = (cands[order[count]].fast_cost - best) * 100 / best;
                // C: `mds1_cand_th / (rank ? rank * cand_count : 1)`
                // (product_coding_loop.c:8095) — rank 0 (M4 nic case 5)
                // means the raw threshold, NOT a zero divisor.
                let div = if cfg.mds1_rank_factor != 0 {
                    cfg.mds1_rank_factor * count as u64
                } else {
                    1
                };
                if dev >= mds1_cand_th / div {
                    break;
                }
                count += 1;
            }
            n1 = count;
        }
    }

    // -- MDS1: luma-only full loop (freq dist, quantize_b, DCT, depth 0) --
    for &ci in order.iter().take(n1) {
        let cand = &mut cands[ci];
        let (txb_skip_ctx, dc_sign_ctx) = if cfg.real_coeff_ctx {
            let (above, left) = fx.ectx.coeff_neighbors(abs_x, abs_y, w, h);
            cc::get_txb_ctx(0, above, left, true, false)
        } else {
            (0, 0)
        };
        let out = tx_unit(
            y_src,
            y_src_stride,
            y_src_off,
            &cand.pred,
            w,
            0,
            w,
            h,
            cc::DCT_DCT,
            0,
            txb_skip_ctx,
            dc_sign_ctx,
            cand.mode as usize,
            &qt,
            frame,
            rates,
            false, // no RDOQ at MDS1
            false, // freq-domain dist
        );
        let has = out.eob > 0;
        let tsz_cat = tx_size_cat(w, h);
        let tsz_ctx = fx.ectx.tx_size_ctx(abs_x, abs_y, w, h);
        let tx_size_bits = rates.tx_size[tsz_cat][tsz_ctx][0] as u64;
        let coeff_rate = if has {
            out.bits as u64 + tx_size_bits + rates.skip[skip_ctx][0] as u64
        } else {
            rates.skip[skip_ctx][1] as u64 + tx_size_bits
        };
        cand.mds1_has_coeff = has;
        cand.full_cost = rdcost(lambda, cand.flr + cand.fcr + coeff_rate, out.dist);
    }

    // -- Sort survivors by full cost --
    let mut order1: Vec<usize> = order[..n1].to_vec();
    order1.sort_by_key(|&i| cands[i].full_cost);
    let mds1_best_idx = order1[0];

    // -- post_mds1_nic_pruning (:8111) --
    let mds2_cand_th = div_round(cfg.mds2_cand_base_th * qw, qwd);
    let mut n2 = (n1 as u32).min(nic2) as usize;
    {
        let best = cands[order1[0]].full_cost;
        let mut count = 1usize;
        if best > 0 && count < n2 {
            // C rank staging (product_coding_loop.c:8158-8166): only when
            // the config factor is nonzero — same class (the inter-class
            // +3 arm is dead: single intra class == the mds1 best class),
            // +2 when the MDS0 and MDS1 winners coincide.
            let mut rank_factor = cfg.mds2_rank_factor;
            if rank_factor != 0 && mds0_best_idx == mds1_best_idx {
                rank_factor += 2;
            }
            let mut prev_dev = (cands[order1[count]].full_cost - best) * 100 / best;
            let mut dev = prev_dev;
            // C while (:8169-8171): `(!mds2_relative_dev_th || dev <=
            // prev_dev + mds2_relative_dev_th) && dev < mds2_cand_th /
            // (rank ? rank * cand_count : 1)` — rel-dev th 0 (M4) DISABLES
            // the relative-dev exit; rank 0 means divisor 1.
            while (cfg.mds2_rel_dev_th == 0 || dev <= prev_dev + cfg.mds2_rel_dev_th)
                && dev
                    < mds2_cand_th
                        / (if rank_factor != 0 {
                            rank_factor * count as u64
                        } else {
                            1
                        })
            {
                count += 1;
                if count >= n2 {
                    break;
                }
                prev_dev = dev;
                dev = (cands[order1[count]].full_cost - best) * 100 / best;
            }
            n2 = count;
        }
    }

    // -- post_mds2_nic_pruning (:8189) on the SAME MDS1 costs (MDS2
    //    bypassed at staging mode 1) --
    let mds3_cand_th = div_round(cfg.mds3_cand_base_th * qw, qwd);
    let mut n3 = (n2 as u32).min(nic3) as usize;
    {
        let best = cands[order1[0]].full_cost;
        let mut count = 1usize;
        if best > 0 {
            while count < n3 {
                let dev = (cands[order1[count]].full_cost - best) * 100 / best;
                if dev >= mds3_cand_th {
                    break;
                }
                count += 1;
            }
            n3 = count;
        }
    }

    // -- MDS3: full loop with TXS + TXT + RDOQ + spatial SSE + chroma --
    let do_rdoq = frame.rdoq_level > 0;
    // txs_level 0 (M8) -> depth 0 only; else get_end_tx_depth clamped by
    // the config's intra sq/nsq max depths.
    let end_depth = if cfg.txs_on {
        end_tx_depth(w, h, &cfg)
    } else {
        0
    };
    // Chroma pair geometry (C blk_geom bsize_uv + ROUND_UV origins).
    let cw = w.max(8) / 2;
    let chh = h.max(8) / 2;
    let ccx = ((abs_x >> 3) << 3) / 2 + if w >= 8 { (abs_x % 8) / 2 } else { 0 };
    let ccy = ((abs_y >> 3) << 3) / 2 + if h >= 8 { (abs_y % 8) / 2 } else { 0 };
    let tsz_cat = tx_size_cat(w, h);
    let tsz_ctx = fx.ectx.tx_size_ctx(abs_x, abs_y, w, h);

    // Chroma txb contexts (real at rate_est_level 1; candidate-independent
    // — the neighbour bytes don't change during this block's search).
    let (cb_tsc, cb_dsc) = if cfg.real_coeff_ctx {
        let (a, l) = fx.ectx.coeff_neighbors_uv(0, ccx, ccy, cw, chh);
        cc::get_txb_ctx(1, a, l, true, false)
    } else {
        (0, 0)
    };
    let (cr_tsc, cr_dsc) = if cfg.real_coeff_ctx {
        let (a, l) = fx.ectx.coeff_neighbors_uv(1, ccx, ccy, cw, chh);
        cc::get_txb_ctx(1, a, l, true, false)
    } else {
        (0, 0)
    };

    // One full-loop chroma evaluation of a (uv_mode, uv_delta) pair —
    // the shared body of `search_best_mds3_uv_mode`'s full loop and
    // MDS3's `svt_aom_full_loop_uv` (identical settings: rdoq per frame
    // policy, spatial SSE, real contexts).
    let chroma_eval = |fx: &FunnelCtx<'_>, uv: u8, uv_delta: i8| -> (TxUnitOut, TxUnitOut) {
        let mut u_pred = vec![0u8; cw * chh];
        let mut v_pred = vec![0u8; cw * chh];
        predict_unit(
            fx.u_recon,
            fx.c_stride,
            ccx,
            ccy,
            cw,
            chh,
            uv,
            uv_delta,
            FI_NONE,
            &uv_geom,
            cfg.edge_filter,
            filt_type_uv,
            &mut u_pred,
        );
        predict_unit(
            fx.v_recon,
            fx.c_stride,
            ccx,
            ccy,
            cw,
            chh,
            uv,
            uv_delta,
            FI_NONE,
            &uv_geom,
            cfg.edge_filter,
            filt_type_uv,
            &mut v_pred,
        );
        let tt = uv_tx_type(uv, cw, chh);
        let u_out = tx_unit(
            fx.u_src,
            fx.c_stride,
            ccy * fx.c_stride + ccx,
            &u_pred,
            cw,
            0,
            cw,
            chh,
            tt,
            1,
            cb_tsc,
            cb_dsc,
            0,
            &qt,
            frame,
            rates,
            do_rdoq,
            true,
        );
        let v_out = tx_unit(
            fx.v_src,
            fx.c_stride,
            ccy * fx.c_stride + ccx,
            &v_pred,
            cw,
            0,
            cw,
            chh,
            tt,
            1,
            cr_tsc,
            cr_dsc,
            0,
            &qt,
            frame,
            rates,
            do_rdoq,
            true,
        );
        (u_out, v_out)
    };

    // -- Independent chroma search before MDS3 (chroma_level 4:
    //    `search_best_mds3_uv_mode`, product_coding_loop.c:7561, invoked
    //    per :10098-10105 when `perform_ind_uv_search_last_mds` — at
    //    least one MDS3 intra candidate whose (injected, uv-follows-luma)
    //    uv mode is not UV_DC (skip_ind_uv_if_only_dc = 1; the
    //    inter_vs_intra_cost_th=100 arm never fires on I-slices:
    //    MAX_MODE_COST * 100 does not overflow and dwarfs any intra
    //    cost). Produces best_uv[(luma mode)] -> (uv mode, uv delta);
    //    `update_intra_chroma_mode` (:7326) then rewrites each MDS3
    //    candidate before its full loop. --
    let mut ind_uv: Option<[(u8, i8); 13]> = None;
    if cfg.ind_uv_mds3 && has_uv && order1.iter().take(n3).any(|&ci| cands[ci].uv != 0) {
        // Distinct (uv, uv_delta) pairs of the MDS3 survivors, in
        // survivor order, excluding UV_DC; then UV_DC (delta 0) last.
        let mut tested = [[false; 7]; 13];
        let mut uv_list: Vec<(u8, i8)> = Vec::new();
        for &ci in order1.iter().take(n3) {
            let (uvm, uvd) = (cands[ci].uv, cands[ci].uv_delta);
            if uvm == 0 || tested[uvm as usize][(3 + uvd) as usize] {
                continue;
            }
            tested[uvm as usize][(3 + uvd) as usize] = true;
            uv_list.push((uvm, uvd));
        }
        uv_list.push((0, 0));

        // Full loop per uv candidate: coeff_rate + SSD distortion
        // (DIST_CALC_RESIDUAL — both planes summed).
        let mut uv_rd: Vec<(u64, u64)> = Vec::with_capacity(uv_list.len());
        for &(uvm, uvd) in &uv_list {
            let (u_out, v_out) = chroma_eval(fx, uvm, uvd);
            uv_rd.push((
                u_out.bits as u64 + v_out.bits as u64,
                u_out.dist + v_out.dist,
            ));
        }

        // Per distinct surviving luma mode (survivor order), pick the
        // lowest-cost uv pair (strict less, list order on ties).
        let mut table = [(0u8, 0i8); 13];
        let mut mode_seen = [false; 13];
        for &ci in order1.iter().take(n3) {
            let luma = cands[ci].mode as usize;
            if mode_seen[luma] {
                continue;
            }
            mode_seen[luma] = true;
            let mut best_cost = u64::MAX;
            for (k, &(uvm, uvd)) in uv_list.iter().enumerate() {
                let mut fcr2 = rates.uv[cfl_allowed][luma][uvm as usize] as u64;
                if use_angle && matches!(uvm, 1..=8) {
                    fcr2 += rates.angle[uvm as usize - 1][(3 + uvd) as usize] as u64;
                }
                let (bits, dist) = uv_rd[k];
                let cost = rdcost(lambda, bits + fcr2, dist);
                if cost < best_cost {
                    best_cost = cost;
                    table[luma] = (uvm, uvd);
                }
            }
        }
        ind_uv = Some(table);
    }

    for &ci in order1.iter().take(n3) {
        // `update_intra_chroma_mode`: rewrite the candidate's chroma from
        // the ind-uv table (fast chroma rate recomputed for the luma
        // mode + new uv pair — same formula as injection, so an
        // unconditional recompute is C-identical).
        if let Some(tbl) = &ind_uv {
            let (uvm, uvd) = tbl[cands[ci].mode as usize];
            let c = &mut cands[ci];
            c.uv = uvm;
            c.uv_delta = uvd;
            let mut fcr = rates.uv[cfl_allowed][c.mode as usize][uvm as usize] as u64;
            if use_angle && matches!(uvm, 1..=8) {
                fcr += rates.angle[uvm as usize - 1][(3 + uvd) as usize] as u64;
            }
            c.fcr = fcr;
        }
        // ---- Luma: TX depth loop ----
        let mut best_depth = 0u8;
        let mut best_cost = u64::MAX;
        let mut best_bits: u64 = 0;
        let mut best_dist: u64 = 0;
        let mut best_txb_q: Vec<Vec<i32>> = Vec::new();
        let mut best_txb_eob: Vec<u16> = Vec::new();
        let mut best_txb_cul: Vec<u8> = Vec::new();
        let mut best_txb_type: Vec<u8> = Vec::new();
        let mut best_recon: Vec<u8> = Vec::new();
        let mut best_coeff_count = u32::MAX;

        for depth in 0..=end_depth {
            // prev_depth_coeff_exit_th = 1: skip deeper depths when the
            // previous depth kept no coefficients.
            if best_coeff_count < 1 {
                continue;
            }
            // C tx geometry at this depth (tx_depth_to_tx_size /
            // tx_blocks_per_depth / the intra tx_org raster).
            let (txw, txh) = txb_dims_at_depth(w, h, depth);
            let cols = w / txw;
            let txbs = cols * (h / txh);
            // TX-local dc_sign/cul overlay (tx_reset_neighbor_arrays).
            let mut loc_above = fx.ectx.above_coeff_span(abs_x, w).to_vec();
            let mut loc_left = fx.ectx.left_coeff_span(abs_y, h).to_vec();
            let mut dep_bits: u64 = 0;
            let mut dep_dist: u64 = 0;
            let mut dep_q: Vec<Vec<i32>> = Vec::with_capacity(txbs);
            let mut dep_eob: Vec<u16> = Vec::with_capacity(txbs);
            let mut dep_cul: Vec<u8> = Vec::with_capacity(txbs);
            let mut dep_type: Vec<u8> = Vec::with_capacity(txbs);
            let mut dep_recon = vec![0u8; w * h];
            let mut dep_has_coeff = false;
            let mut aborted = false;

            for txb in 0..txbs {
                let tx_x = (txb % cols) * txw;
                let tx_y = (txb / cols) * txh;
                let cand = &cands[ci];
                // Per-txb prediction: depth 0 reuses the MDS0 pred;
                // depth > 0 predicts from the live canvas (frame recon
                // outside the block, this depth's recon inside).
                let mut txb_pred = vec![0u8; txw * txh];
                if depth == 0 {
                    txb_pred.copy_from_slice(&cand.pred);
                } else {
                    // Overlay canvas: temporarily splice this depth's
                    // reconstructed txbs into the frame recon.
                    predict_unit_overlay(
                        y_recon,
                        y_stride,
                        abs_x,
                        abs_y,
                        &dep_recon,
                        w,
                        h,
                        tx_x,
                        tx_y,
                        txw,
                        txh,
                        cand.mode,
                        cand.delta,
                        cand.fi,
                        &y_geom,
                        cfg.edge_filter,
                        filt_type_y,
                        &mut txb_pred,
                    );
                }
                // Per-txb contexts from the TX-local overlay (real at M6;
                // 0/0 at M7/M8 where update_skip_ctx_dc_sign_ctx == 0, so
                // cul_level never accumulates — full_loop.c:1880).
                let (tsc, dsc) = if cfg.real_coeff_ctx {
                    txb_ctx_from_spans(&loc_above, &loc_left, tx_x, tx_y, txw, txh, depth == 0)
                } else {
                    (0, 0)
                };
                // TXT search over this txb.
                let intra_dir = if cand.fi != FI_NONE {
                    FIMODE_TO_INTRADIR[cand.fi as usize] as usize
                } else {
                    cand.mode as usize
                };
                let (out, txt) = txt_search(
                    y_src,
                    y_src_stride,
                    y_src_off + tx_y * y_src_stride + tx_x,
                    &txb_pred,
                    txw,
                    txh,
                    depth,
                    tsc,
                    dsc,
                    intra_dir,
                    &qt,
                    frame,
                    rates,
                    do_rdoq,
                    lambda,
                );
                dep_bits += out.bits as u64;
                dep_dist += out.dist;
                dep_has_coeff |= out.eob > 0;
                // tx_update_neighbor_arrays: cul byte over the txb span.
                let a0 = tx_x / 4;
                let a1 = (a0 + txw / 4).min(loc_above.len());
                for v in loc_above[a0..a1].iter_mut() {
                    *v = out.cul;
                }
                let l0 = tx_y / 4;
                let l1 = (l0 + txh / 4).min(loc_left.len());
                for v in loc_left[l0..l1].iter_mut() {
                    *v = out.cul;
                }
                for r in 0..txh {
                    let dst = (tx_y + r) * w + tx_x;
                    dep_recon[dst..dst + txw].copy_from_slice(&out.recon[r * txw..(r + 1) * txw]);
                }
                dep_q.push(out.qcoeff);
                dep_eob.push(out.eob);
                dep_cul.push(out.cul);
                dep_type.push(txt as u8);

                // C txb loop early exit: current accumulated cost already
                // above the best depth cost.
                if rdcost(lambda, dep_bits, dep_dist) > best_cost {
                    aborted = true;
                    break;
                }
            }
            if aborted && depth > 0 {
                continue;
            }
            let tx_size_bits = rates.tx_size[tsz_cat][tsz_ctx][depth as usize] as u64;
            let cost = rdcost(lambda, dep_bits + tx_size_bits, dep_dist);
            if cost < best_cost {
                best_cost = cost;
                best_depth = depth;
                best_bits = dep_bits;
                best_dist = dep_dist;
                best_txb_q = dep_q;
                best_txb_eob = dep_eob.clone();
                best_txb_cul = dep_cul;
                best_txb_type = dep_type;
                best_recon = dep_recon;
                best_coeff_count = dep_eob.iter().map(|&e| e as u32).sum();
                let _ = dep_has_coeff;
            }
        }

        // ---- Chroma full loop (uv per candidate: follows-luma at
        //      CHROMA_MODE_1, or the ind-uv table pick at chroma_level 4)
        //      + the complexity detector (CFL gate; see below) ----
        //      Skipped entirely for non-chroma-ref blocks (C gates every
        //      chroma stage on ctx->has_uv).
        let cand = &cands[ci];
        let (u_out, v_out) = if has_uv {
            chroma_eval(fx, cand.uv, cand.uv_delta)
        } else {
            (TxUnitOut::absent(), TxUnitOut::absent())
        };
        if has_uv {
            // Chroma complexity detector (chroma_complexity_check_pred,
            // product_coding_loop.c:6095) — its only funnel-visible effect
            // is the CFL gate (tx shortcuts are level 0). When it fires C
            // would evaluate cfl_prediction for <= 32x32 blocks; CFL
            // search is unported, so the non-CFL uv mode is kept and the
            // event recorded (never fires on flat-chroma content — all
            // tracked identity cells).
            let mut u_pred = vec![0u8; cw * chh];
            let mut v_pred = vec![0u8; cw * chh];
            predict_unit(
                fx.u_recon,
                fx.c_stride,
                ccx,
                ccy,
                cw,
                chh,
                cand.uv,
                cand.uv_delta,
                FI_NONE,
                &uv_geom,
                cfg.edge_filter,
                filt_type_uv,
                &mut u_pred,
            );
            predict_unit(
                fx.v_recon,
                fx.c_stride,
                ccx,
                ccy,
                cw,
                chh,
                cand.uv,
                cand.uv_delta,
                FI_NONE,
                &uv_geom,
                cfg.edge_filter,
                filt_type_uv,
                &mut v_pred,
            );
            let cfl_would_run = chroma_detector_fires(
                y_src,
                y_src_stride,
                y_src_off,
                &cand.pred,
                w,
                fx.u_src,
                fx.v_src,
                &u_pred,
                &v_pred,
                fx.c_stride,
                ccy * fx.c_stride + ccx,
                cw,
                chh,
            );
            // When the gate arms on <= 32x32 blocks C would run
            // cfl_prediction and possibly pick UV_CFL — unported: we keep
            // the non-CFL uv mode. Tracked cells never arm it (flat
            // chroma); arming on other content yields a valid
            // (non-C-identical) stream.
            let _ = cfl_would_run;
        }

        // ---- svt_aom_full_cost (rd_cost.c:1357) ----
        let block_has_coeff = best_coeff_count > 0 || u_out.eob > 0 || v_out.eob > 0;
        let tx_size_bits_final = rates.tx_size[tsz_cat][tsz_ctx][best_depth as usize] as u64;
        // Chroma coeff rate: M6 (coeff_rate_est_lvl 1) prices the real
        // cost_coeffs_txb / cost_skip_txb (already in u_out.bits/v_out.bits).
        // M7/M8 (coeff_rate_est_lvl 2, OPT_APPROX_COEFF_RATE) use the
        // eob-based chroma approximation (`skip_chroma_rate_est`,
        // full_loop.c:1922): th = (tx_w_uv * tx_h_uv) >> 6; eob == 0 -> 0,
        // eob < th -> 3000 + eob*500, eob >= th -> full estimation.
        let (u_bits, v_bits) = if cfg.real_coeff_ctx {
            (u_out.bits as u64, v_out.bits as u64)
        } else {
            let uv_th = ((cw * chh) >> 6) as u16;
            let approx = |eob: u16, full: u64| -> u64 {
                if eob == 0 {
                    0
                } else if eob < uv_th {
                    3000 + eob as u64 * 500
                } else {
                    full
                }
            };
            (
                approx(u_out.eob, u_out.bits as u64),
                approx(v_out.eob, v_out.bits as u64),
            )
        };
        let coeff_rate = if block_has_coeff {
            best_bits + u_bits + v_bits + tx_size_bits_final + rates.skip[skip_ctx][0] as u64
        } else {
            rates.skip[skip_ctx][1] as u64 + tx_size_bits_final
        };
        let dist = best_dist + u_out.dist + v_out.dist;
        let full = rdcost(lambda, cand.flr + cand.fcr + coeff_rate, dist);

        let cand = &mut cands[ci];
        cand.mds3_cost = full;
        cand.total_rate = cand.flr + cand.fcr + coeff_rate;
        cand.full_dist = dist;
        cand.tx_depth = best_depth;
        cand.txb_q = best_txb_q;
        cand.txb_eob = best_txb_eob;
        cand.txb_cul = best_txb_cul;
        cand.txb_type = best_txb_type;
        cand.y_recon = best_recon;
        cand.y_bits = best_bits;
        cand.y_dist = best_dist;
        cand.u_q = u_out.qcoeff;
        cand.v_q = v_out.qcoeff;
        cand.u_eob = u_out.eob;
        cand.v_eob = v_out.eob;
        cand.u_cul = u_out.cul;
        cand.v_cul = v_out.cul;
        cand.u_recon = u_out.recon;
        cand.v_recon = v_out.recon;
        cand.block_has_coeff = block_has_coeff;
    }

    // -- svt_aom_product_full_mode_decision: lowest cost, first wins --
    let mut win = order1[0];
    let mut win_cost = cands[order1[0]].mds3_cost;
    for &ci in order1.iter().take(n3).skip(1) {
        if cands[ci].mds3_cost < win_cost {
            win_cost = cands[ci].mds3_cost;
            win = ci;
        }
    }

    // The shared MDS3 residual workspace after the loop: the LAST
    // processed candidate's (order1[n3-1]) whole-block depth-0 residual.
    let mut psq_resid = vec![0i32; w * h];
    {
        let last = &cands[order1[n3 - 1]];
        for r in 0..h {
            let srow = y_src_off + r * y_src_stride;
            for c in 0..w {
                psq_resid[r * w + c] = y_src[srow + c] as i32 - last.pred[r * w + c] as i32;
            }
        }
    }

    LeafEval {
        abs_x,
        abs_y,
        w,
        h,
        has_uv,
        ccx,
        ccy,
        cw,
        chh,
        win: cands.swap_remove(win),
        psq_resid,
    }
}

/// Commit an evaluated winner — C `md_update_all_neighbour_arrays` (+ the
/// MD recon plane writes `copy_recon_md` feeds): luma recon into
/// `y_recon`, chroma into the funnel's decision planes, mode/skip/uv
/// rows, chosen-tx txfm dims, per-txb + chroma coefficient contexts.
/// Every array write spans exactly the block, so re-committing a parent
/// block after its children were committed overwrites them completely
/// (the C winner-overwrite in `test_split_partition`).
pub(crate) fn commit_leaf(
    fx: &mut FunnelCtx<'_>,
    y_recon: &mut [u8],
    y_stride: usize,
    ev: &LeafEval,
) {
    let (abs_x, abs_y) = (ev.abs_x, ev.abs_y);
    let (w, h) = (ev.w, ev.h);
    let (ccx, ccy, cw, chh) = (ev.ccx, ev.ccy, ev.cw, ev.chh);
    let cand = &ev.win;
    for r in 0..h {
        let dst = (abs_y + r) * y_stride + abs_x;
        y_recon[dst..dst + w].copy_from_slice(&cand.y_recon[r * w..(r + 1) * w]);
    }
    if ev.has_uv {
        for r in 0..chh {
            let dst = (ccy + r) * fx.c_stride + ccx;
            fx.u_recon[dst..dst + cw].copy_from_slice(&cand.u_recon[r * cw..(r + 1) * cw]);
            fx.v_recon[dst..dst + cw].copy_from_slice(&cand.v_recon[r * cw..(r + 1) * cw]);
        }
    }
    let skip = !cand.block_has_coeff;
    fx.ectx
        .record_block(abs_x, abs_y, w, h, cand.mode, cand.uv, skip);
    // MD partition-context bytes (mode_decision_update_neighbor_arrays,
    // product_coding_loop.c:179-192: partition_context_lookup[bsize]
    // written over the block span — per-DIMENSION levels for rect NSQ
    // children). Consumed by the depth walk's partition rates
    // (update_part_neighs); inert for the fixed-tree paths (nothing
    // reads the decision ectx's partition bytes there).
    fx.ectx.update_partition_ctx_leaf(abs_x, abs_y, w, h);
    // set_txfm_ctxs with the CHOSEN tx dims (mode_decision_update:246-256).
    let (txw, txh) = txb_dims_at_depth(w, h, cand.tx_depth);
    fx.ectx.record_txfm_dims(abs_x, abs_y, w, h, txw, txh);
    // Per-txb luma cul bytes; chroma culs over the chroma span.
    let cols = w / txw;
    for (txb, &cul) in cand.txb_cul.iter().enumerate() {
        let tx_x = (txb % cols) * txw;
        let tx_y = (txb / cols) * txh;
        fx.ectx
            .record_coeff(abs_x + tx_x, abs_y + tx_y, txw, txh, cul);
    }
    if ev.has_uv {
        fx.ectx.record_coeff_uv(0, ccx, ccy, cw, chh, cand.u_cul);
        fx.ectx.record_coeff_uv(1, ccx, ccy, cw, chh, cand.v_cul);
    }
}

/// C `get_end_tx_depth` (product_coding_loop.c:4171) clamped by
/// `intra_class_max_depth_sq` / `_nsq` (get_start_end_tx_depth :6973;
/// shape == PART_N <=> w == h — HVA/HVB shapes with square children are
/// geometry-disabled at every funnel preset).
fn end_tx_depth(w: usize, h: usize, cfg: &FunnelCfg) -> u8 {
    let base: u8 = match (w, h) {
        // 2-depth blocks (the bsize list at :4173-4176).
        (64, 64) | (32, 32) | (16, 16) => 2,
        (64, 32) | (32, 64) | (32, 16) | (16, 32) | (16, 8) | (8, 16) => 2,
        (64, 16) | (16, 64) | (32, 8) | (8, 32) | (16, 4) | (4, 16) => 2,
        (8, 8) => 1,
        _ => 0, // 8x4, 4x8, 4x4
    };
    let cap = if w == h { cfg.txs_max_sq } else { cfg.txs_max_nsq };
    base.min(cap)
}

/// C `bsize_to_tx_size_cat`: category of the block's max tx size chain —
/// `TXSIZE_SQR_UP` of the max rect TX (== the larger block dim as a
/// square), minus TX_8X8, capped at MAX_TX_CATS-1. 4x8/8x4 -> TX_8X8 ->
/// cat 0; 4x16/16x4 -> TX_16X16 -> cat 1.
fn tx_size_cat(w: usize, h: usize) -> usize {
    match w.max(h) {
        4 | 8 => 0,
        16 => 1,
        32 => 2,
        _ => 3, // 64 (TX_64X64 -> cat 3)
    }
}

/// C `tx_depth_to_tx_size[depth][bsize]` (common_utils.c:95) — the TX
/// dims at a given depth — plus the txb count/raster geometry
/// (`tx_blocks_per_depth` / the intra `tx_org` rows, transforms.c:48;
/// pinned against the instrumented dump in the tests below). Positions
/// are plain raster: x fastest, `w/txw` columns.
pub(crate) fn txb_dims_at_depth(w: usize, h: usize, depth: u8) -> (usize, usize) {
    let (mut tw, mut th) = (w.min(64), h.min(64));
    for _ in 0..depth {
        (tw, th) = sub_tx_dims(tw, th);
    }
    (tw, th)
}

/// C `sub_tx_size_map` chain expressed on dims: square TXs halve both
/// dims (min 4); 2:1 rects halve the long dim; 4:1 rects halve the long
/// dim (64x16 -> 32x16 -> 16x16 per the table).
fn sub_tx_dims(tw: usize, th: usize) -> (usize, usize) {
    if tw == th {
        ((tw / 2).max(4), (th / 2).max(4))
    } else if tw > th {
        (tw / 2, th)
    } else {
        (tw, th / 2)
    }
}

/// C `non_normative_txs` (product_coding_loop.c:9641): re-transform the
/// shared MDS3 residual workspace (`cand_bf->residual` = the LAST MDS3
/// candidate's whole-block depth-0 residual — every MDS3 candidate
/// full-loops through ONE pixel workspace at staging mode 1; pointer-
/// instrumented) with the two half-height TXs (H split) and the two
/// half-width TXs (V split), DCT_DCT + `svt_aom_quantize_inv_quantize_
/// light` (plain quantize_b, y tables, full_loop.c:1253), and return
/// the min eob per split direction. `None` when the winner kept no
/// coefficients (C leaves the ~0 sentinels, so the psq gate can't
/// fire).
pub(crate) fn min_nz_hv(ev: &LeafEval, qindex: u8) -> Option<(u16, u16)> {
    if !ev.block_has_coeff() {
        return None;
    }
    let (w, h) = (ev.w, ev.h);
    debug_assert!(w == h && w >= 8, "psq gate runs on SQ blocks only");
    let qt = crate::quant::build_quant_table(qindex);
    let resid = ev.psq_resid();
    debug_assert_eq!(resid.len(), w * h);

    let half_eob = |ox: usize, oy: usize, tw: usize, th: usize| -> u16 {
        let n = tw * th;
        let c_tx = cc::tx_size_from_dims(tw, th);
        let mut residual = vec![0i32; n];
        for r in 0..th {
            let rrow = (oy + r) * w + ox;
            residual[r * tw..(r + 1) * tw].copy_from_slice(&resid[rrow..rrow + tw]);
        }
        let mut coeffs = vec![0i32; n];
        let ok = svtav1_dsp::txfm_dispatch::fwd_txfm2d_dispatch(
            &residual,
            &mut coeffs,
            tw,
            rs_tx_size(tw, th),
            TX_TYPE_FROM_C[cc::DCT_DCT],
        );
        debug_assert!(ok, "psq fwd txfm {tw}x{th}");
        // 64-dim fold (the 64x32/32x64 halves of a 64x64 block).
        let (pw, ph) = (tw.min(32), th.min(32));
        let packed = if tw > 32 || th > 32 {
            let mut v = vec![0i32; pw * ph];
            for r in 0..ph {
                v[r * pw..(r + 1) * pw].copy_from_slice(&coeffs[r * tw..r * tw + pw]);
            }
            v
        } else {
            coeffs
        };
        let scan = svtav1_entropy::scan_tables::scan(
            c_tx,
            svtav1_entropy::scan_tables::TX_TYPE_TO_SCAN_INDEX[cc::DCT_DCT] as usize,
        );
        let mut qcoeff = vec![0i32; pw * ph];
        let mut dqcoeff = vec![0i32; pw * ph];
        crate::quant::quantize_b(
            &packed,
            scan,
            &qt,
            TX_SCALE_TAB[c_tx],
            &mut qcoeff,
            &mut dqcoeff,
        )
    };

    let mut nz_h = u16::MAX;
    for part in 0..2usize {
        nz_h = nz_h.min(half_eob(0, part * (h / 2), w, h / 2));
    }
    let mut nz_v = u16::MAX;
    for part in 0..2usize {
        nz_v = nz_v.min(half_eob(part * (w / 2), 0, w / 2, h));
    }
    Some((nz_h, nz_v))
}

/// Chroma tx type: C `svt_aom_get_intra_uv_tx_type`
/// (mode_decision.c:2991) = `g_intra_mode_to_tx_type[uv_mode]` clamped to
/// DCT_DCT when the chroma tx size's intra ext set doesn't carry the
/// type (32x32 chroma is DCT-only; the WIN dumps' ttuv fields pin the
/// mapping). The uv tx type affects the SCAN + coeff coding only when
/// eob > 0.
pub(crate) fn uv_tx_type(uv: u8, cw: usize, chh: usize) -> usize {
    /// C `g_intra_mode_to_tx_type[INTRA_MODES]` (DCT=0, ADST_DCT=1,
    /// DCT_ADST=2, ADST_ADST=3).
    const MODE_TO_TX: [usize; 13] = [0, 1, 2, 0, 3, 1, 2, 2, 1, 3, 1, 2, 3];
    let t = MODE_TO_TX[uv as usize];
    // DCT-only tx sizes (>= 32 in either dim).
    if cw >= 32 || chh >= 32 {
        cc::DCT_DCT
    } else {
        t
    }
}

/// Per-txb luma prediction at depth > 0: reads the frame recon for
/// out-of-block neighbors and this depth's partial recon inside the block.
/// Mirrors C `av1_intra_luma_prediction` (product_coding_loop.c:4072):
/// `svt_av1_predict_intra_block` at (row_off, col_off) over the
/// tx-search neighbor arrays (block interior = this depth's recon so
/// far, exterior = frame recon).
#[allow(clippy::too_many_arguments)]
fn predict_unit_overlay(
    y_recon: &[u8],
    y_stride: usize,
    blk_x: usize,
    blk_y: usize,
    dep_recon: &[u8],
    blk_w: usize,
    blk_h: usize,
    tx_x: usize,
    tx_y: usize,
    txw: usize,
    txh: usize,
    mode: u8,
    delta: i8,
    fi: u8,
    geom: &UnitGeom,
    edge_filter: bool,
    filt_type: i32,
    dst: &mut [u8],
) {
    if matches!(mode, 3..=8) || (matches!(mode, 1 | 2) && delta != 0) {
        let p_angle = crate::intra_edge::MODE_TO_ANGLE_MAP[mode as usize] + delta as i32 * 3;
        debug_assert!(fi == FI_NONE);
        let g = crate::intra_edge::DrGeom {
            px: blk_x + tx_x,
            py: blk_y + tx_y,
            txw,
            txh,
            mi_row: geom.mi_row,
            mi_col: geom.mi_col,
            bw_px: geom.bw_px,
            bh_px: geom.bh_px,
            row_off: tx_y / 4,
            col_off: tx_x / 4,
            ss: 0,
            frame_w: geom.frame_w,
            frame_h: geom.frame_h,
        };
        crate::intra_edge::dr_predict(
            |x, y| {
                if x >= blk_x && x < blk_x + blk_w && y >= blk_y && y < blk_y + blk_h {
                    dep_recon[(y - blk_y) * blk_w + (x - blk_x)]
                } else {
                    y_recon[y * y_stride + x]
                }
            },
            &g,
            p_angle,
            edge_filter,
            filt_type,
            svtav1_types::partition::PartitionType::None,
            dst,
        );
        return;
    }
    // Build a small canvas: (txh + 1) left col + (txw + 1) top row around
    // the txb, sourcing in-block pixels from dep_recon and out-of-block
    // pixels from the frame recon, then run the standard edge extraction
    // on it. Canvas layout: (txh+1) rows x (txw+1) cols, txb at (1, 1).
    let cw_dim = txw + 1;
    let ch_dim = txh + 1;
    let abs_tx_x = blk_x + tx_x;
    let abs_tx_y = blk_y + tx_y;
    let mut canvas = vec![0u8; cw_dim * ch_dim];
    let sample = |x: isize, y: isize| -> u8 {
        // (x, y) absolute plane coords.
        if x < 0 || y < 0 {
            return 128; // never read: extract handles borders
        }
        let (x, y) = (x as usize, y as usize);
        let in_blk_x = x >= blk_x && x < blk_x + blk_w;
        let in_blk_y = y >= blk_y && y < blk_y + blk_h;
        if in_blk_x && in_blk_y {
            dep_recon[(y - blk_y) * blk_w + (x - blk_x)]
        } else {
            let row_len = y_stride;
            let idx = y * y_stride + x.min(row_len - 1);
            if idx < y_recon.len() {
                y_recon[idx]
            } else {
                y_recon[y_recon.len() - row_len + x.min(row_len - 1)]
            }
        }
    };
    // top row (incl. corner) and left col of the canvas
    for cx in 0..cw_dim {
        canvas[cx] = sample(abs_tx_x as isize + cx as isize - 1, abs_tx_y as isize - 1);
    }
    for cy in 1..ch_dim {
        canvas[cy * cw_dim] = sample(abs_tx_x as isize - 1, abs_tx_y as isize + cy as isize - 1);
    }
    // Predict at canvas coords (1, 1): availability mirrors the absolute
    // position (frame edges).
    let has_above = abs_tx_y > 0;
    let has_left = abs_tx_x > 0;
    let above: Vec<u8> = if has_above {
        canvas[1..cw_dim].to_vec()
    } else {
        vec![if has_left { canvas[cw_dim] } else { 127 }; txw]
    };
    let left: Vec<u8> = if has_left {
        (1..ch_dim).map(|cy| canvas[cy * cw_dim]).collect()
    } else {
        vec![if has_above { canvas[1] } else { 129 }; txh]
    };
    let top_left = if has_above && has_left {
        canvas[0]
    } else if has_above {
        canvas[1]
    } else if has_left {
        canvas[cw_dim]
    } else {
        128
    };
    if fi != FI_NONE {
        let mut above_c = vec![0u8; txw + 1];
        above_c[0] = top_left;
        above_c[1..].copy_from_slice(&above);
        svtav1_dsp::intra_pred::predict_filter_intra(dst, txw, &above_c, &left, txw, txh, fi);
        return;
    }
    match mode {
        0 => svtav1_dsp::intra_pred::predict_dc(
            dst, txw, &above, &left, txw, txh, has_above, has_left,
        ),
        1 => svtav1_dsp::intra_pred::predict_v(dst, txw, &above, txw, txh),
        2 => svtav1_dsp::intra_pred::predict_h(dst, txw, &left, txw, txh),
        9 => svtav1_dsp::intra_pred::predict_smooth(dst, txw, &above, &left, txw, txh),
        10 => svtav1_dsp::intra_pred::predict_smooth_v(dst, txw, &above, &left, txh, txh, txw),
        11 => svtav1_dsp::intra_pred::predict_smooth_h(dst, txw, &above, &left, txw, txh),
        12 => svtav1_dsp::intra_pred::predict_paeth(dst, txw, &above, &left, top_left, txw, txh),
        m => unreachable!("funnel mode {m}"),
    }
}

/// txb skip / dc sign contexts from TX-local (block-span) overlay arrays.
/// `spans` are the block's above/left coeff-byte slices (4x4 units);
/// txb at (tx_x, tx_y) within the block, `tx` square dims.
fn txb_ctx_from_spans(
    above_span: &[u8],
    left_span: &[u8],
    tx_x: usize,
    tx_y: usize,
    txw: usize,
    txh: usize,
    block_eq_tx: bool,
) -> (usize, usize) {
    let a0 = tx_x / 4;
    let l0 = tx_y / 4;
    let a = &above_span[a0..(a0 + txw / 4).min(above_span.len())];
    let l = &left_span[l0..(l0 + txh / 4).min(left_span.len())];
    cc::get_txb_ctx(0, a, l, block_eq_tx, false)
}

/// TXT search for one luma txb (`tx_type_search`, product_coding_loop.c:
/// 4660): DCT-only above 16x16 intra (ext-tx set), otherwise the intra
/// tx-type groups with SATD early exit + rate-cost gate. Returns the best
/// type's unit output.
#[allow(clippy::too_many_arguments)]
fn txt_search(
    src: &[u8],
    src_stride: usize,
    src_off: usize,
    pred: &[u8],
    w: usize,
    h: usize,
    depth: u8,
    txb_skip_ctx: usize,
    dc_sign_ctx: usize,
    intra_dir: usize,
    qt: &QuantTable,
    frame: &FunnelFrame,
    rates: &MdRates,
    do_rdoq: bool,
    lambda: u64,
) -> (TxUnitOut, usize) {
    let c_tx = cc::tx_size_from_dims(w, h);
    // search_dct_dct_only (product_coding_loop.c:4601): txt disabled
    // (eff-M9 txt_level 0 -> !mds_do_txt), dims > 32, a single-type ext
    // set, or ext set index 0.
    let only_dct = !frame.cfg.txt_on
        || w > 32
        || h > 32
        || cc::ext_tx_types(c_tx, false, false) == 1
        || cc::ext_tx_set(c_tx, false, false) == 0;
    // get_tx_type_group (product_coding_loop.c:4358): per-preset intra
    // group counts (M6 txt_level 8: ge16 4 / lt16 5; M5 txt_level 3:
    // 6 / 6 — the dump's txt_ge16/txt_lt16); depth-1 offset 3 (min 1).
    let mut groups: i32 = if only_dct {
        1
    } else if w >= 16 && h >= 16 {
        frame.cfg.txt_group_ge16
    } else {
        frame.cfg.txt_group_lt16
    };
    if depth == 1 && !only_dct {
        groups = (groups - frame.cfg.txt_d1_off).max(1);
    } else if depth == 2 && !only_dct {
        groups = (groups - frame.cfg.txt_d2_off).max(1);
    }

    /// C `av1_ext_tx_used[EXT_TX_SET_TYPES][TX_TYPES]` (definitions.h).
    const AV1_EXT_TX_USED: [[u8; 16]; 6] = [
        [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], // DCTONLY
        [1, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0], // DCT_IDTX
        [1, 1, 1, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0], // DTT4_IDTX
        [1, 1, 1, 1, 0, 0, 0, 0, 0, 1, 1, 1, 0, 0, 0, 0], // DTT4_IDTX_1DDCT
        [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0], // DTT9_IDTX_1DDCT
        [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1], // ALL16
    ];
    const TX_TYPE_GROUPS: [&[usize]; 6] = [
        &[cc::DCT_DCT],
        &[10, 11], // V_DCT, H_DCT
        &[3],      // ADST_ADST
        &[1, 2],   // ADST_DCT, DCT_ADST
        &[6, 9],   // FLIPADST_FLIPADST, IDTX
        &[4, 5, 7, 8, 12, 13, 14, 15],
    ];

    let set_type = cc::ext_tx_set_type(c_tx, false, false);
    // qp-scaled SATD early-exit th (satd_th_q_weight = 1; intra th 10 at
    // M6, 15 at M5 — txt_satd_intra in the dumps).
    let (qw, qwd) = qp_scale_factors(frame.cli_qp);
    let satd_th = if only_dct {
        0
    } else {
        div_round(frame.cfg.txt_satd_th * qw, qwd)
    } as i64;

    let mut best: Option<TxUnitOut> = None;
    let mut best_type = cc::DCT_DCT;
    let mut best_cost = u64::MAX;
    let mut dct_cost = u64::MAX;
    let mut best_satd = i64::MAX;

    'groups: for g in 0..groups as usize {
        for &tx_type in TX_TYPE_GROUPS[g] {
            if only_dct && tx_type != cc::DCT_DCT {
                continue;
            }
            if tx_type != cc::DCT_DCT {
                if AV1_EXT_TX_USED[set_type][tx_type] == 0 {
                    continue;
                }
                // txt_rate_cost_th (100 at M6, 250 at M5): skip types
                // whose signalling rate alone exceeds the DCT cost
                // fraction (product_coding_loop.c:4787-4794).
                let tx_type_rate = rates.txt_rate(c_tx, intra_dir, tx_type) as u64;
                if dct_cost != u64::MAX
                    && rdcost(lambda, tx_type_rate, 0) * 1000 > dct_cost * frame.cfg.txt_rate_th
                {
                    continue;
                }
            }
            let out = tx_unit(
                src,
                src_stride,
                src_off,
                pred,
                w,
                0,
                w,
                h,
                tx_type,
                0,
                txb_skip_ctx,
                dc_sign_ctx,
                intra_dir,
                qt,
                frame,
                rates,
                do_rdoq,
                true, // MDS3 spatial dist
            );
            // SATD early exit between transform and quantize in C; we
            // apply it post-hoc on the transform coefficients via a
            // dedicated pass only when the th is armed.
            if satd_th > 0 {
                let satd = txb_coeff_satd(src, src_stride, src_off, pred, w, h, tx_type);
                if satd < best_satd {
                    best_satd = satd;
                } else if (satd - best_satd) * 100 > best_satd * satd_th {
                    continue;
                }
            }
            // A non-DCT type with no coefficients is not signalable.
            if out.eob == 0 && tx_type != cc::DCT_DCT {
                continue;
            }
            let cost = rdcost(lambda, out.bits as u64, out.dist);
            if cost < best_cost {
                best_cost = cost;
                best_type = tx_type;
                if tx_type == cc::DCT_DCT {
                    dct_cost = cost;
                }
                best = Some(out);
            } else if tx_type == cc::DCT_DCT {
                dct_cost = cost;
            }
            if only_dct {
                break 'groups;
            }
        }
    }
    (best.expect("DCT_DCT always evaluated"), best_type)
}

/// SATD of the forward-transform coefficients (C computes it inline on
/// `ctx->tx_coeffs` right after svt_aom_estimate_transform).
fn txb_coeff_satd(
    src: &[u8],
    src_stride: usize,
    src_off: usize,
    pred: &[u8],
    w: usize,
    h: usize,
    tx_type: usize,
) -> i64 {
    let n = w * h;
    let mut residual = vec![0i32; n];
    for r in 0..h {
        let srow = src_off + r * src_stride;
        let prow = r * w;
        for c in 0..w {
            residual[r * w + c] = src[srow + c] as i32 - pred[prow + c] as i32;
        }
    }
    let mut coeffs = vec![0i32; n];
    svtav1_dsp::txfm_dispatch::fwd_txfm2d_dispatch(
        &residual,
        &mut coeffs,
        w,
        rs_tx_size(w, h),
        TX_TYPE_FROM_C[tx_type],
    );
    let mut satd: i64 = 0;
    for &c in &coeffs {
        satd += c.unsigned_abs() as i64;
    }
    satd
}

/// C `chroma_complexity_check_pred` (product_coding_loop.c:6095), exact:
/// subsampled SADs of the candidate's luma/chroma predictions vs their
/// sources; the CFL gate (`cfl_complexity == COMPONENT_CHROMA`) arms when
/// either chroma SAD exceeds 2x the luma SAD over the chroma-sized
/// region. (The use_var arm only raises chroma_complexity, which has no
/// funnel-visible effect at M6 — tx shortcuts are level 0.)
#[allow(clippy::too_many_arguments)]
fn chroma_detector_fires(
    y_src: &[u8],
    y_stride: usize,
    y_off: usize,
    y_pred: &[u8],
    y_pred_stride: usize,
    u_src: &[u8],
    v_src: &[u8],
    u_pred: &[u8],
    v_pred: &[u8],
    c_stride: usize,
    c_off: usize,
    cw: usize,
    chh: usize,
) -> bool {
    let shift = if chh > 8 {
        2usize
    } else if chh > 4 {
        1
    } else {
        0
    };
    let rows = chh >> shift;
    let sad =
        |a: &[u8], a_off: usize, a_stride: usize, b: &[u8], b_off: usize, b_stride: usize| -> u32 {
            let mut s = 0u32;
            for r in 0..rows {
                let ar = a_off + r * (a_stride << shift);
                let br = b_off + r * (b_stride << shift);
                for c in 0..cw {
                    s += (a[ar + c] as i32 - b[br + c] as i32).unsigned_abs();
                }
            }
            s
        };
    let y_dist = sad(y_src, y_off, y_stride, y_pred, 0, y_pred_stride) << 1;
    let cb_dist = sad(u_src, c_off, c_stride, u_pred, 0, cw);
    let cr_dist = sad(v_src, c_off, c_stride, v_pred, 0, cw);
    cb_dist > y_dist || cr_dist > y_dist
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Instrumented-capture pins: `M6FNL NICS c0` lines — mds1/2/3
    /// counts at CLI qp 20/40/55 (M6 nic level 6, nums 6/6/6, base
    /// 24/12/6 q-scaled).
    #[test]
    fn nic_counts_match_c() {
        // M6 (nic level 6): nums 6/6/6.
        assert_eq!(nic_counts(20, (6, 6, 6)), (8, 4, 2));
        assert_eq!(nic_counts(40, (6, 6, 6)), (15, 8, 4));
        assert_eq!(nic_counts(55, (6, 6, 6)), (22, 11, 5));
        // M8 (nic level 11 -> scaling level 15 -> nums 0/0/0): the min-1
        // floor (scaling num == 0) pins every stage to 1 at all tracked qps.
        assert_eq!(nic_counts(20, (0, 0, 0)), (1, 1, 1));
        assert_eq!(nic_counts(40, (0, 0, 0)), (1, 1, 1));
        assert_eq!(nic_counts(55, (0, 0, 0)), (1, 1, 1));
    }

    /// RDCOST identity from the captured g64 q55 MDS3 rows: the DC
    /// candidate's full cost decomposition
    /// (rate 547+273+176560+112+112+1280+26, dist 10963760).
    #[test]
    fn rdcost_matches_capture() {
        assert_eq!(rdcost(1527856, 178910, 10963760), 1937245493);
        // H row: rate 181608, dist 10996528 -> 1949490882.
        assert_eq!(rdcost(1527856, 181608, 10996528), 1949490882);
        // MDS0 fast cost, DC @ q55: rate 820, satd 204088 << 4.
        assert_eq!(rdcost(1527856, 820, 204088 << 4), 420419181);
    }

    /// Mode/uv/fi/tx_size rate pins from the M6FNL MDS0/FLC dumps
    /// (default contexts, coeff tables at the respective qindexes).
    #[test]
    fn md_rates_match_c_captures() {
        let fc = svtav1_entropy::context::FrameContext::new_default();
        let cfc = svtav1_entropy::coeff_c::CoeffFc::default_for_qindex(220);
        let r = build_md_rates(&fc, &cfc);
        // kf y mode at ctx (0,0): DC 547, SMOOTH 1556 (q55 64x64 flr).
        assert_eq!(r.kf_y[0][0][0], 547);
        assert_eq!(r.kf_y[0][0][9], 1556);
        // V/H flr include the angle0 symbol: 2874 / 2555.
        assert_eq!(r.kf_y[0][0][1] + r.angle[0][3], 2874);
        assert_eq!(r.kf_y[0][0][2] + r.angle[1][3], 2555);
        // uv fcr rows: 64x64 (CFL-disallowed) DC 273, V 1033, H 1009;
        // 32x32 (CFL-allowed) DC 845, SMOOTH 1362.
        assert_eq!(r.uv[0][0][0], 273);
        assert_eq!(r.uv[0][1][1] + r.angle[0][3], 1033);
        assert_eq!(r.uv[0][2][2] + r.angle[1][3], 1009);
        assert_eq!(r.uv[1][0][0], 845);
        assert_eq!(r.uv[1][9][9], 1362);
        // filter-intra at 32x32 (bsize_idx 9): flag-off 281 (DC flr
        // 828 - 547), flag-on + FILTER_DC mode = 1803 (FI flr 2350).
        assert_eq!(r.fi_flag[9][0], 281);
        assert_eq!(r.fi_flag[9][1] + r.fi_mode[0], 1803);
        // skip=0 at ctx 0: 26.
        assert_eq!(r.skip[0][0], 26);
        // tx_size bits: 64x64 ctx0 depth0/1 = 1280/1292; 32x32 ctx0
        // depth0 = 683 (q40 FLC nsk_txsz).
        assert_eq!(r.tx_size[3][0][0], 1280);
        assert_eq!(r.tx_size[3][0][1], 1292);
        assert_eq!(r.tx_size[2][0][0], 683);
    }

    /// FunnelCfg::for_preset(5) pins vs the instrumented M5DBG CFG
    /// enc_mode=5 dump (docs/captures/m0m5_config_dlf.txt): intra_level 2
    /// -> mode_end PAETH / ang 2; fi_max 0 (FILTER_DC only); nic 6 with
    /// M6's pruning ths; txt 6/6 satd 15 rate 250; chroma_level 4
    /// (ind-uv MDS3); SH edge filter.
    #[test]
    fn m5_cfg_matches_capture() {
        let c = FunnelCfg::for_preset(5);
        assert_eq!(c.mode_end, 12);
        assert_eq!(c.angular_level, 2);
        assert!(c.filter_intra && !c.prune_best_mode);
        assert_eq!(c.nic_num, (6, 6, 6));
        assert_eq!(
            (c.mds1_cand_base_th, c.mds1_rank_factor, c.mds2_cand_base_th),
            (1200, 3, 15)
        );
        assert_eq!((c.mds2_rel_dev_th, c.mds3_cand_base_th), (5, 15));
        assert_eq!((c.txt_group_lt16, c.txt_group_ge16), (6, 6));
        assert_eq!((c.txt_satd_th, c.txt_rate_th), (15, 250));
        assert!(c.real_coeff_ctx && c.txs_on && c.txt_on);
        assert!(c.ind_uv_mds3 && c.edge_filter && !c.dc_only_gate);
        assert_eq!(c.mds2_rank_factor, 1);
        // M6 keeps the original shape (regression pin for the shared tail).
        let m6 = FunnelCfg::for_preset(6);
        assert_eq!(m6.mode_end, 9);
        assert_eq!(m6.angular_level, 4);
        assert_eq!((m6.txt_group_lt16, m6.txt_group_ge16), (5, 4));
        assert_eq!((m6.txt_satd_th, m6.txt_rate_th), (10, 100));
        assert!(!m6.ind_uv_mds3 && !m6.edge_filter);
        assert_eq!(m6.mds2_rank_factor, 1);
    }

    /// FunnelCfg::for_preset(4) pins vs the instrumented M5DBG CFG
    /// enc_mode=4 dump (docs/captures/m0m5_config_dlf.txt line 14):
    /// intra_level 1 -> mode_end PAETH / angular_pred_level 1 (ALL 7
    /// deltas); SH edge filter OFF (ang 1 not in {2,3}); nic case 5 —
    /// scal 6, mds1 1200/rank 0, mds2 20/rank 0/rel-dev 0, mds3 15;
    /// txt/txs/rdoq/chroma identical to M5.
    #[test]
    fn m4_cfg_matches_capture() {
        let c = FunnelCfg::for_preset(4);
        assert_eq!(c.mode_end, 12);
        assert_eq!(c.angular_level, 1);
        assert!(c.filter_intra && !c.prune_best_mode);
        assert_eq!(c.nic_num, (6, 6, 6));
        assert_eq!(
            (c.mds1_cand_base_th, c.mds1_rank_factor, c.mds2_cand_base_th),
            (1200, 0, 20)
        );
        assert_eq!((c.mds2_rank_factor, c.mds2_rel_dev_th), (0, 0));
        assert_eq!(c.mds3_cand_base_th, 15);
        assert_eq!((c.txt_group_lt16, c.txt_group_ge16), (6, 6));
        assert_eq!((c.txt_satd_th, c.txt_rate_th), (15, 250));
        assert!(c.real_coeff_ctx && c.txs_on && c.txt_on);
        assert!(c.ind_uv_mds3 && !c.edge_filter && !c.dc_only_gate);
    }

    /// M4 candidate enumeration (angular_pred_level 1): every directional
    /// mode carries all 7 deltas in counter order -3..+3
    /// (mode_decision.c:3259-3271 — the |1|/|2| skip only arms at
    /// level >= 2), non-directionals one entry each, FILTER_DC last:
    /// 13 modes + 8 x 6 extra deltas = 61 regular + 1 filter-intra.
    #[test]
    fn m4_candidate_set_shape() {
        let cfg = FunnelCfg::for_preset(4);
        let mut n = 0usize;
        let mut first_dir_deltas: Vec<i8> = Vec::new();
        for mode in 0..=cfg.mode_end {
            let directional = matches!(mode, 1..=8);
            if matches!(mode, 3..=8) && cfg.angular_level >= 4 {
                continue;
            }
            if directional && cfg.angular_level <= 2 {
                for d in -3i8..=3 {
                    if cfg.angular_level >= 2 && matches!(d, -2 | -1 | 1 | 2) {
                        continue;
                    }
                    if mode == 1 {
                        first_dir_deltas.push(d);
                    }
                    n += 1;
                }
            } else {
                n += 1;
            }
        }
        assert_eq!(n, 61);
        assert_eq!(first_dir_deltas, alloc::vec![-3, -2, -1, 0, 1, 2, 3]);
    }

    /// The chroma tx type derivation confirmed by the WIN dumps
    /// (ttuv 0/1/2/3 for DC/V/H/SMOOTH; DCT-only at >= 32) + the full
    /// g_intra_mode_to_tx_type rows the M5 ind-uv modes reach.
    #[test]
    fn uv_tx_type_matches_c() {
        // SMOOTH_V -> ADST_DCT, SMOOTH_H -> DCT_ADST, PAETH -> ADST_ADST,
        // D45 -> DCT_DCT, D135 -> ADST_ADST (mode_decision.c:2991 table).
        assert_eq!(uv_tx_type(10, 16, 16), 1);
        assert_eq!(uv_tx_type(11, 16, 16), 2);
        assert_eq!(uv_tx_type(12, 16, 16), 3);
        assert_eq!(uv_tx_type(3, 16, 16), 0);
        assert_eq!(uv_tx_type(4, 16, 16), 3);
    }

    #[test]
    fn uv_tx_type_m6_subset_matches_c() {
        assert_eq!(uv_tx_type(0, 16, 16), 0);
        assert_eq!(uv_tx_type(1, 16, 16), 1);
        assert_eq!(uv_tx_type(2, 16, 16), 2);
        assert_eq!(uv_tx_type(9, 16, 16), 3);
        assert_eq!(uv_tx_type(2, 32, 32), 0); // 64x64 luma -> DCT only
    }
}
