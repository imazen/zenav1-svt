//! Rate tables — `md_rate_estimation` over a given frame context.
//!
//! Split out of `leaf_funnel.rs` on 2026-08-16 (11,247 lines).
//! PURE CODE MOVEMENT: every item keeps its name, order and effective
//! visibility (file-private became `pub(super)`, the same scope).

use super::*;

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
    /// CfL alpha rate: [joint_sign][plane][alpha_idx] (cfl_alpha_fac_bits,
    /// md_rate_estimation.c:192-213). Plane U already carries the joint-sign
    /// rate added in; plane V is the magnitude cost alone.
    pub cfl_alpha_fac_bits: [[[i32; 16]; 2]; 8],
    /// No-palette y flag cost `palette_ymode_fac_bits[bctx][mode_ctx][0]`
    /// (rd_cost.c:582-584). Indexed by the palette bsize ctx AND the
    /// neighbor palette-mode ctx (C `svt_aom_get_palette_mode_ctx`, 0..=2 —
    /// count of above/left neighbours whose luma palette_size>0). Priced into
    /// DC candidates' luma rate when allow_palette. Row `[_][0]` is the
    /// pre-#71 no-neighbour value (bit-identical for non-screen content).
    pub palette_y_no: [[i32; 3]; 7],
    /// No-palette uv flag cost `palette_uv_mode_fac_bits[use_palette_y][0]`
    /// (rd_cost.c:514-520, inside svt_aom_get_intra_uv_fast_rate) — part of
    /// EVERY UV_DC chroma fast rate when allow_palette. Indexed by
    /// `use_palette_y` (C `cand->palette_size[0] > 0`): `[0]` for a regular
    /// candidate (y-palette off), `[1]` for a palette candidate (y-palette
    /// on). The rows DIFFER — `[1][0]` is dearer (icdf 11280 vs 307) — so a
    /// palette candidate that priced the `[0]` row under-costs its own chroma
    /// flag, biasing the palette-vs-regular RD tie toward palette (a #71
    /// over-picking lever). `use_palette_uv` is hard-0 (chroma palette dead).
    pub palette_uv_no: [i32; 2],
    /// palette_y_mode YES flag cost `palette_ymode_fac_bits[bctx][mode_ctx][1]`
    /// (rd_cost.c:582-584) — the n>0 arm palette candidates price. Same
    /// `[bctx][mode_ctx]` indexing as [`Self::palette_y_no`].
    pub palette_y_yes: [[i32; 3]; 7],
    /// palette_y_size fac bits [bsize ctx][n-2] (md_rate_estimation.c:167).
    pub palette_ysize: [[i32; 7]; 7],
    /// palette_y_color_index fac bits [n-2][color ctx][idx<n]
    /// (md_rate_estimation.c:~180; row width = n symbols).
    pub palette_ycolor: [[[i32; 8]; 5]; 7],
    /// `use_intrabc` flag cost `intrabc_fac_bits[use_intrabc]`
    /// (md_rate_estimation.c:253-255, from `fc->intrabc_cdf`; default CDF
    /// AOM_CDF2(30531) gives `[51, 1982]`). C fills it only when
    /// `allow_intrabc` (leaving stale memory otherwise); the port fills
    /// unconditionally — the sole consumers are gated on the same
    /// frame-level flag (rd_cost.c:629-631 / :531-545), so the value is
    /// unread on non-IBC frames. Per-SB cadence: rebuilt with the rest of
    /// this struct from the avg'd snapshot (`update_se` is 1 at the
    /// funnel's CDF levels — enc_dec_process.c:2901-2909).
    pub intrabc_fac_bits: [i32; 2],
    /// INTER tx-type signalling costs from `inter_ext_tx_cdf` (IBC chunk 7:
    /// `inter_tx_type_fac_bits[eset][square_tx_size][tx_type]`,
    /// md_rate_estimation.c:~215 — the `is_inter` arm of
    /// `av1_txt_rate_est`). Only IntraBC candidates read these here.
    pub inter_ext_tx: [[i32; 17]; 4 * 4],
    /// `txfm_partition` split costs (`txfm_partition_fac_bits[ctx][split]`,
    /// md_rate_estimation.c:222, from `fc->txfm_partition_cdf`) — the
    /// inter var-tx tx_size rate rows (`cost_tx_size_vartx`, rd_cost.c
    /// :1591-1650). IntraBC-only consumers.
    pub txfm_partition_fac_bits: [[i32; 2]; svtav1_entropy::context::TXFM_PARTITION_CONTEXTS],
    /// Coefficient cost tables (svt_aom_estimate_coefficients_rate).
    pub coeff: alloc::boxed::Box<CoeffCostTables>,
}

/// C `av1_ext_tx_used[set][tx_type]` accessor for the pack's IntraBC
/// chroma follows-luma tx-type rule (tx_type_search,
/// product_coding_loop.c:5091-5096).
pub(crate) fn ext_tx_used(set_type: usize, tx_type: usize) -> bool {
    AV1_EXT_TX_USED[set_type][tx_type] != 0
}

/// C `sort_fast_cost_based_candidates` / `sort_full_cost_based_candidates`
/// (product_coding_loop.c:1415 / :1438): the swap-on-`<` exchange sort
/// `for i { for j>i { if cost[j] < cost[i] swap(i,j) } }`.
///
/// NOT stable: a swap displaces the element at `i` down to `j`, so when a
/// strictly-smaller element appears AFTER a group of equal-cost elements,
/// the group's first member is moved to the smaller element's position —
/// behind the rest of its tie group. On all-distinct keys the result equals
/// a stable ascending sort, so substituting this for `sort_by_key` is
/// byte-inert except on exact cost ties — where THIS order is the one C's
/// stage counts truncate (which candidates survive into MDS3).
pub(super) fn c_exchange_sort_by(idx: &mut [usize], cost: impl Fn(usize) -> u64) {
    let n = idx.len();
    for i in 0..n.saturating_sub(1) {
        for j in (i + 1)..n {
            if cost(idx[j]) < cost(idx[i]) {
                idx.swap(i, j);
            }
        }
    }
}

/// C `av1_ext_tx_used[EXT_TX_SET_TYPES][TX_TYPES]` (definitions.h) —
/// which tx types each ext set admits. Shared by `txt_search`'s set gate
/// and the IntraBC chroma follows-luma tx-type rule.
pub(super) const AV1_EXT_TX_USED: [[u8; 16]; 6] = [
    [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], // DCTONLY
    [1, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0], // DCT_IDTX
    [1, 1, 1, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0], // DTT4_IDTX
    [1, 1, 1, 1, 0, 0, 0, 0, 0, 1, 1, 1, 0, 0, 0, 0], // DTT4_IDTX_1DDCT
    [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0], // DTT9_IDTX_1DDCT
    [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1], // ALL16
];

/// Sentinel `intra_dir` marking an INTER-classified (IntraBC) txb through
/// the shared `tx_unit`/`cost_coeffs_txb`/`txt_search` plumbing: real
/// intra dirs are 0..=12, so 13 is unambiguous. `MdRates::txt_rate` maps
/// it to the inter tx-type rate rows; `txt_search` maps it to the inter
/// ext-tx set.
pub(crate) const INTER_TXT_DIR: usize = 13;

pub(super) fn costs_from_cdf<const N: usize>(cdf: &[u16]) -> [i32; N] {
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
        cfl_alpha_fac_bits: [[[0; 16]; 2]; 8],
        palette_y_no: [[0; 3]; 7],
        palette_uv_no: [0; 2],
        palette_y_yes: [[0; 3]; 7],
        palette_ysize: [[0; 7]; 7],
        palette_ycolor: [[[0; 8]; 5]; 7],
        intrabc_fac_bits: [0; 2],
        inter_ext_tx: [[0; 17]; 16],
        txfm_partition_fac_bits: [[0; 2]; svtav1_entropy::context::TXFM_PARTITION_CONTEXTS],
        coeff: crate::quant::build_coeff_cost_tables_from_fc(cfc),
    });
    r.intrabc_fac_bits = costs_from_cdf::<2>(&fc.intrabc_cdf);
    for row in 0..16 {
        r.inter_ext_tx[row] = costs_from_cdf::<17>(&cfc.inter_ext_tx_cdf[row]);
    }
    for (row, cdf) in fc.txfm_partition_cdf.iter().enumerate() {
        r.txfm_partition_fac_bits[row] = costs_from_cdf::<2>(cdf);
    }
    for b in 0..7 {
        // palette_ymode_fac_bits[bsize_ctx][mode_ctx][yes/no] — all 3
        // neighbor mode-ctx rows (C default_palette_y_mode_cdf, 7x3x2).
        for m in 0..3 {
            let c2 = costs_from_cdf::<2>(&fc.palette_y_mode_cdf[b][m]);
            r.palette_y_no[b][m] = c2[0];
            r.palette_y_yes[b][m] = c2[1];
        }
        r.palette_ysize[b] = costs_from_cdf::<7>(&fc.palette_y_size_cdf[b]);
    }
    r.palette_uv_no = [
        costs_from_cdf::<2>(&fc.palette_uv_mode_cdf[0])[0],
        costs_from_cdf::<2>(&fc.palette_uv_mode_cdf[1])[0],
    ];
    for n in 0..7 {
        for c in 0..5 {
            // Row width = n+2 symbols; syntax_rate_from_cdf reads to the
            // terminator, so slice per-row like the uv 13/14 handling.
            let nsym = n + 2;
            let mut full = [0i32; 8];
            let mut tmp = alloc::vec![0i32; nsym];
            // #71: the palette color-index MAP cost uses the FRAME-INIT
            // (default) CDF, NOT the per-SB-chained `fc`. C's MD-side
            // `update_palette_cdf` (md_rate_estimation.c:733-759) advances
            // ONLY palette_y_mode / palette_y_size — it NEVER touches
            // palette_y_color_index_cdf — so `palette_ycolor_fac_bitss` stays
            // at its frame-init value for every SB (measured: constant across
            // the whole frame). Building it from the chained `fc` (which the
            // port's full-walk chain sim adapts via write_palette_map_tokens)
            // drifted the map rate on 2nd+ palette blocks (graph p6 q5
            // mi(14,46): port 18875 vs C 17858) and flipped the palette-vs-
            // regular near-tie. The DEFAULT const == the frame-init fc, so
            // this is a no-op on the (default-fc) non-chain call sites and on
            // non-screen frames (palette_ycolor unused).
            crate::quant::syntax_rate_from_cdf(
                &mut tmp,
                &svtav1_entropy::default_cdfs::PALETTE_Y_COLOR_INDEX_CDF[n][c],
            );
            full[..nsym].copy_from_slice(&tmp);
            r.palette_ycolor[n][c] = full;
        }
    }
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
    // CfL alpha rate table (md_rate_estimation.c:192-213). sign_fac_bits
    // over cfl_sign_cdf; per joint_sign, each plane's magnitude costs from
    // cfl_alpha_cdf[CFL_CONTEXT_{U,V}] (zero-sign plane -> all-0); then the
    // joint-sign rate is folded into plane U only (matching the syntax:
    // sign coded once, U/V magnitudes follow).
    {
        use svtav1_entropy::context as ctx;
        let mut sign_fac_bits = [0i32; ctx::CFL_JOINT_SIGNS];
        crate::quant::syntax_rate_from_cdf(&mut sign_fac_bits, &fc.cfl_sign_cdf);
        for js in 0..ctx::CFL_JOINT_SIGNS {
            if ctx::cfl_sign_u(js) != 0 {
                crate::quant::syntax_rate_from_cdf(
                    &mut r.cfl_alpha_fac_bits[js][0],
                    &fc.cfl_alpha_cdf[ctx::cfl_context_u(js)],
                );
            }
            if ctx::cfl_sign_v(js) != 0 {
                crate::quant::syntax_rate_from_cdf(
                    &mut r.cfl_alpha_fac_bits[js][1],
                    &fc.cfl_alpha_cdf[ctx::cfl_context_v(js)],
                );
            }
            for u in 0..16 {
                r.cfl_alpha_fac_bits[js][0][u] += sign_fac_bits[js];
            }
        }
    }
    r
}

impl MdRates {
    /// C `av1_transform_type_rate_estimation` (rd_cost.c:107) /
    /// `av1_txt_rate_est` (product_coding_loop.c:4318): nonzero only when
    /// the tx size's ext set has > 1 type. `intra_dir` follows
    /// `fimode_to_intradir` for filter-intra blocks; the [`INTER_TXT_DIR`]
    /// sentinel selects the `is_inter` arm (IntraBC blocks — the inter
    /// ext-tx set + `inter_tx_type_fac_bits`, no intra-dir dimension).
    pub(super) fn txt_rate(&self, c_tx_size: usize, intra_dir: usize, tx_type: usize) -> i32 {
        let is_inter = intra_dir == INTER_TXT_DIR;
        if cc::ext_tx_types(c_tx_size, is_inter, false) <= 1 {
            return 0;
        }
        let set_type = cc::ext_tx_set_type(c_tx_size, is_inter, false);
        let eset = cc::EXT_TX_SET_INDEX[usize::from(is_inter)][set_type];
        if eset == 0 {
            return 0;
        }
        let sq_tx = cc::TXSIZE_SQR_MAP[c_tx_size];
        // SHIPPED-C QUIRK, second half (md_rate_estimation.c:225-243): C's
        // `{intra,inter}_tx_type_fac_bits` are indexed by the RAW `TxType`,
        // and `svt_aom_get_syntax_rate_from_cdf(..., av1_ext_tx_inv[set])`
        // SCATTERS the per-symbol costs into only the tx types that belong to
        // `set`. Every other entry keeps its zero-init value, so a query for a
        // tx type OUTSIDE the row's set reads a literal 0 — it is not a symbol
        // lookup at all.
        //
        // This port keeps the tables SYMBOL-indexed, so it has to reproduce
        // that "unpopulated entry" explicitly. Without the guard,
        // `AV1_EXT_TX_IND[set][out_of_set_type]` is 0 (that table's own
        // filler) and the out-of-set query silently returns SYMBOL 0's cost,
        // which is a real, large rate.
        //
        // The only caller that can query out-of-set is the IntraBC coeff cost
        // via the `cost_dir` remap in `cost_coeffs_txb` (the tx type comes
        // from the INTER search set while the row read is the INTRA set).
        // MEASURED on gb82-sc graph.png 512x512 q63 preset 2, block mi(8,80)
        // (a 32x32 IntraBC leaf), luma txb (16,0) 16x16 with V_DCT: C prices
        // the tx type at 0 (V_DCT is in the INTER 16x16 set DTT9_IDTX_1DDCT
        // but not the INTRA one, DTT4_IDTX) for a txb cost of 2808; the port
        // charged symbol 0 (= IDTX, 2489) for 5297, which flipped the per-txb
        // TXT winner to DCT_DCT/eob=0 where C codes V_DCT/eob=1.
        if AV1_EXT_TX_USED[set_type][tx_type] == 0 {
            return 0;
        }
        let sym = cc::AV1_EXT_TX_IND[set_type][tx_type];
        if is_inter {
            self.inter_ext_tx[eset as usize * 4 + sq_tx][sym]
        } else {
            let row = (eset as usize * 4 + sq_tx) * 13 + intra_dir;
            self.intra_ext_tx[row][sym]
        }
    }
}

// ---------------------------------------------------------------------------
// Frame-level funnel configuration
// ---------------------------------------------------------------------------

/// Frame-constant funnel parameters.
pub struct FunnelFrame {
    /// Superblock size in MI (4px) units — C `seq_header.sb_mi_size`, 16 at
    /// SB64 and 32 at SB128 (task #91). Feeds the intra availability tables
    /// (`intra_edge::has_top_right` / `has_bottom_left`), whose
    /// `blk_row_in_sb` / `blk_col_in_sb` are `mi & (sb_mi_size - 1)` — so a
    /// block at mi_col 16 is the SB's LEFT column at SB64 but its RIGHT
    /// half at SB128, with completely different top-right / bottom-left
    /// availability. 16 for every SB64 encode, i.e. byte-neutral there.
    pub sb_mi_size: usize,
    /// `full_lambda_md[EB_8_BIT_MD]` — the kf chain at the frame qindex.
    pub lambda: u64,
    /// CLI qp 0..63 (qp-based threshold scaling input).
    pub cli_qp: u32,
    /// Frame rdoq level (0 = quantize_b at MDS3 too).
    pub rdoq_level: u8,
    pub base_qindex: u8,
    /// Encode bit depth (8 or 10). At bd10 C forces `pd0_ctrls.pd0_level =
    /// PD0_LVL_0` (`set_pd0_ctrls`, enc_mode_config.c:5416) regardless of
    /// preset, so the eff-M9 per-SB TXS coupling
    /// (`svt_aom_sig_deriv_enc_dec_allintra`, enc_mode_config.c:8114-8118:
    /// `pcs->txs_level == 0 && pd0_level == PD0_LVL_6`) NEVER fires — TXS
    /// stays off (tx_depth 0 everywhere), where bd8 bumps it to level 5 for
    /// undemoted PD0_LVL_6 SBs. The funnel's `sb_is_lvl6` gate (partition.rs)
    /// forces false at bd10 to mirror this. bd8 unaffected.
    pub bit_depth: u8,
    /// Per-plane chroma quantization qindexes: clamp(base + FH delta_q_ac
    /// [plane]). == base_qindex in mainline mode (all FH chroma deltas 0);
    /// the fork's chroma-q path sets U/V independently (chroma_q.rs).
    pub qindex_u: u8,
    pub qindex_v: u8,
    /// Effective AC bias for MD spatial distortion (mainline v4.2 feature,
    /// fork default 1.0): `get_effective_ac_bias(ac_bias, is_islice,
    /// layer)` — stills are I-slices, so ac_bias * 0.3. 0.0 = off = the
    /// prior spatial SSE bit-exactly. The C sites add
    /// `get_svt_psy_full_dist` to the spatial dist BEFORE the <<4
    /// (full_loop.c svt_aom_full_loop_uv + the luma MDS3 path).
    pub ac_bias_eff: f64,
    /// Config sharpness for the RDOQ rshift formula (0 mainline; fork
    /// default 1 — departs from mainline only at >= 3).
    pub sharpness: i8,
    /// [SVT_HDR_MODE] sharp-tx RDOQ active (fork sharp_tx=1 + delta-q).
    pub sharp_tx_active: bool,
    /// [SVT_HDR_MODE] fork `--noise-norm-strength` (0 = off). Applied to
    /// the quantized luma coefficients in `tx_unit` — C runs it in the
    /// encode pass on the winner (full_loop.c:2017, `is_encode_pass &&
    /// eob!=0 && tx_type!=IDTX && LUMA`); this single-pass port applies it
    /// at MD quantization so dist/recon/coded levels stay consistent (fork
    /// mode carries no byte-vs-C gate; the kernel itself is parity-tested).
    pub noise_norm_strength: u8,
    /// [SVT_HDR_MODE] per-plane frame QM levels [Y, U, V] (15 = off);
    /// stamped onto the per-plane `QuantTable`s so every quantize site
    /// resolves the right matrices without extra threading.
    pub qm_levels: [u8; 3],
    /// [SVT_HDR_MODE] fork `--complex-hvs` (0 = off, the fork default):
    /// mds0_level 3 (fork enc_mode_config set_mds0_controls case 3) —
    /// the MDS0 fast-loop luma distortion switches from Hadamard SATD
    /// (`<< 4`) to whole-block spatial SSD (UNshifted; fast_loop_core
    /// `mds0_dist_type == SSD` arm takes precedence over hadamard,
    /// product_coding_loop.c:1351). pruning_method_th stays 0, same as
    /// the allintra I-slice level-0 the funnel already models.
    pub mds0_ssd: bool,
    /// [SVT_HDR_MODE] fork `--alt-ssim-tuning`: SSIM_LVL_1 at PD_PASS_1,
    /// I-slices INCLUDED (product_coding_loop.c:10316) — every MDS3
    /// candidate gets a parallel `full_cost_ssim` (same lambda/rate, the
    /// block-SSIM distortion of ssim_md.rs) and the winner is re-picked
    /// two-pass (lowest SSD cost, then lowest SSIM cost among candidates
    /// within `tune_ssim_threshold` x best SSD cost;
    /// mode_decision.c:3880-3915).
    pub tune_ssim: bool,
    /// `derive_ssim_threshold_factor_for_full_md`: 1.03 sub-1080p, 1.02 at
    /// >= 1080p (by luma sample count). Only read when `tune_ssim`.
    pub tune_ssim_threshold: f64,
    /// [SVT_HDR_MODE] fork `--tx-bias` (0 = off, the fork default). When
    /// set, the mds0/full-loop spatial SSE runs through the fork's
    /// distortion facade bias layer (tx_bias.rs; C
    /// svt_spatial_full_distortion_kernel_facade, pic_operators.c:252).
    pub tx_bias: u8,
    /// IBC chunk 7: the DV RD-cost tables (`md_rate_est_ctx->dv_cost` /
    /// `dv_joint_cost`, `svt_aom_estimate_mv_rate`'s dv arm) — FRAME-
    /// CONSTANT on the allintra path (`update_mv` forced 0 on I-slices;
    /// `build_dv_cost_tables`'s doc), built from the default `ndvc` at
    /// `MV_SUBPEL_NONE`. `None` unless `cfg.allow_intrabc`.
    pub dv_tables: Option<crate::intrabc::MvCostTables>,
    /// Frame height in pixels (`mi_rows * 4`, the ALIGNED height) — the
    /// C `mb_to_bottom_edge` bottom clip the inter var-tx walk applies
    /// (entropy_coding.c:4444-4452). Read by IBC candidates AND, with
    /// `frame_w_px`, by the cropped-TX RD distortion bound below.
    pub frame_h_px: usize,
    /// Frame width in pixels — the ALIGNED width, C `pcs->ppcs->aligned_width`
    /// (pcs.h:1031). Paired with `frame_h_px` it is the `FrameDims` the
    /// cropped-TX distortion bound needs (`frame_geom::cropped_tx_dims` /
    /// `_uv`, C product_coding_loop.c:4664 + full_loop.c:2228): on a PARTIAL
    /// superblock a coded TX block may straddle the aligned extent, and C
    /// prices only the part that is inside the frame. Equal to the aligned
    /// dims on every 64-aligned frame, where the crop is the identity.
    pub frame_w_px: usize,
    /// Per-preset intra-leaf config (M6 vs intra_level-7 M7/M8).
    pub cfg: FunnelCfg,
}

/// Per-preset leaf-funnel configuration (allintra still, presets 6/7/8),
/// verified against the instrumented C `svt_aom_sig_deriv_enc_dec_allintra`
/// config dump (enc_mode_config.c:11294). All fields are pure functions of
/// `enc_mode`; the M6 values reproduce the original hardcoded funnel exactly.
#[derive(Clone, Copy, Debug)]
pub struct FunnelCfg {
    /// C `pcs->pic_bypass_encdec` (svt_aom_get_bypass_encdec_allintra:
    /// `enc_mode <= ENC_M3` -> 0, else 1). Decides whether the MDS3 winner
    /// rebuild (av1_perform_inverse_transform_recon) lands in the shared
    /// `cand_bf->recon` (bypass=0) or is redirected away (bypass=1) — which
    /// switches WHAT the quad-dist gates measure (see `evaluate_leaf`).
    pub bypass_encdec: bool,
    /// filter-intra candidate + `use_filter_intra` syntax (M6: on level 2;
    /// M7/M8: `get_filter_intra_level_allintra` == 0 -> off).
    pub filter_intra: bool,
    /// `filter_intra_ctrls.max_filter_intra_mode` (set_filter_intra_ctrls,
    /// enc_mode_config.c:8045): the highest filter-intra mode injected as a
    /// candidate (all inject a DC_PRED block with filter_intra_mode = 0..N).
    /// filter_intra level 1 (M0) -> FILTER_PAETH_PRED (4 = all 5 modes);
    /// level 2 (M1..M6) -> FILTER_DC_PRED (0, the single FILTER_DC
    /// candidate). Only consulted when `filter_intra` is set.
    pub fi_max: u8,
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
    /// `nic_ctrls.pruning_ctrls.mds3_class_th` base (nic case: lvl1 25 /
    /// lvl3 25 / lvl5 15 / lvl6-7 5). u64::MAX == the `(uint64_t)~0` sentinel
    /// (inter-class MDS3 prune disabled). UNLIKE mds1/mds2_class_th (forced
    /// ~0 on the I-slice, product_coding_loop.c:7826/:7897) this one stays
    /// ACTIVE on I-slices: `MAX(25, scaled*i_mds3_class_th_mult)` (:7978-7979).
    /// Only reachable on the multi-class (palette) path — inert single-class.
    pub mds3_class_th: u64,
    /// `nic_ctrls.pruning_ctrls.mds3_band_cnt` (lvl1 4 / lvl3 8 / lvl5-7 16).
    pub mds3_band_cnt: u8,
    /// `nic_ctrls.pruning_ctrls.i_mds3_class_th_mult` (50 for every
    /// palette-reachable allintra level 1/3/5/6/7).
    pub i_mds3_class_th_mult: u64,
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
    /// `txs_ctrls.inter_class_max_depth_sq`: txs_level 2 at M0..M3 -> 1;
    /// txs_level 3 at M4..M7 -> 1 (set_txs_controls, enc_mode_config.c:
    /// 6185-6205). The port's IBC tx-depth cap (see [`end_tx_depth_inter`]
    /// — deliberately NOT C's mode-keyed intra clamp, pinned).
    pub txs_inter_max_sq: u8,
    /// `txs_ctrls.inter_class_max_depth_nsq`: M0..M3 -> 1; M4..M7 -> 0.
    pub txs_inter_max_nsq: u8,
    /// `txs_ctrls.depth1_txt_group_offset` / `depth2_txt_group_offset`
    /// (txs_level 3: 3/3; txs_level 2: 0/0) — subtracted from the TXT
    /// group count at that tx depth (min 1, get_tx_type_group).
    pub txt_d1_off: i32,
    pub txt_d2_off: i32,
    /// `txs_ctrls.prev_depth_coeff_exit_th` (txs_level <=4: 1; txs_level 5 /
    /// eff-M9 VLPD0 bump: 100): a deeper TX depth is skipped when the best
    /// depth so far kept fewer than this many non-zero coeffs
    /// (perform_tx_partitioning, product_coding_loop.c:5356). On flat
    /// content depth-0 eob < 100 -> depth 1 never tried (why synthetic
    /// identity is unaffected); rich AC (eob >= 100) evaluates the split.
    pub txs_prev_depth_exit: u32,
    /// `txs_ctrls.quadrant_th_sf` (txs_level 5: 100; else 0): per-txb
    /// early-abort of a deeper TX depth when the accumulated cost already
    /// exceeds its proportional share of the best depth cost
    /// (product_coding_loop.c:5437). 0 disables the check.
    pub txs_quadrant_sf: u64,
    /// eff-M9 only: TXS is enabled per-SB, gated on the SB staying at
    /// PD0_LVL_6 (undemoted by `pd0_detector_allintra`). C's
    /// `svt_aom_sig_deriv_enc_dec_allintra` bumps `pcs->txs_level` from 0 to
    /// MAX_TXS_LEVEL-1 (=5) only when `ctx->pd0_ctrls.pd0_level == PD0_LVL_6`
    /// (enc_mode_config.c:11366, FTR_COUPLE_VLPD0_TXS_PER_SB). false at
    /// M0..M8 (txs is uniform across SBs, no per-SB gate).
    pub txs_lvl6_gate: bool,
    /// `rate_est_ctrls.coeff_rate_est_lvl` (set_rate_est_ctrls,
    /// enc_mode_config.c:8342): the luma coeff-RATE estimator used in the RD
    /// compare. 1 (M6) / >=2 (M7/M8) -> the real `cost_coeffs_txb` (the
    /// funnel's `tx_unit` bits); 0 (eff-M9, rate_est_level 0) -> the fast
    /// per-txb approximation in `tx_type_search` (product_coding_loop.c:4976):
    /// `th = (txw*txh)>>6; eob < th ? 6000+eob*1000 : 3000+eob*100`. The
    /// lvl-0 approximation is applied in the eff-M9 depth loop (so the TXS
    /// depth compare matches C). The lvl-2 approximation (M7/M8:
    /// `eob < th ? 6000+eob*1000 : real`) is applied per-txb in `tx_unit`
    /// (LUMA only), so it prices both the MDS1 NIC pruning and the MDS3
    /// mode/tx-type decision like C's shared `full_loop_core`. Level 1 (M6)
    /// keeps the real estimate.
    pub coeff_rate_est_lvl: u8,
    /// chroma_level 4 (M5): CHROMA_MODE_0 with `ind_uv_last_mds = 2` —
    /// `search_best_mds3_uv_mode` over the MDS3 survivors' uv modes
    /// (+ UV_DC), then `update_intra_chroma_mode` rewrites each MDS3
    /// candidate's uv mode from `best_uv_mode[luma_mode]`
    /// (product_coding_loop.c:7561/:7436; skip_ind_uv_if_only_dc = 1).
    /// false = chroma_level 5 (CHROMA_MODE_1, uv follows luma — M6+).
    pub ind_uv_mds3: bool,
    /// chroma_level 1/2 (M0/M1): `search_best_independent_uv_mode`
    /// (product_coding_loop.c:7778, `ind_uv_last_mds` 0/1). A FULL
    /// independent uv search — inject ALL uv modes, fast-loop prune by
    /// residual variance to the `uv_nic`-scaled nfl (UV_DC always forced),
    /// then pick the best uv per luma mode by RD. Differs from the mds3
    /// variant (which only tests the survivors' uv-follows-luma modes):
    /// on flat chroma UV_PAETH is injected last and pruned, so a
    /// luma-PAETH block resolves to UV_DC (C M1 codes UV_DC where M2, the
    /// mds3 variant, codes UV_PAETH). `Some(uv_nic_scaling_num)` = 16 at
    /// chroma_level 1 (M0), 8 at chroma_level 2 (M1); mutually exclusive
    /// with `ind_uv_mds3`. `None` = not the independent variant.
    pub ind_uv_independent: Option<u16>,
    /// C `ind_uv_last_mds == 1` (chroma_level 2, M1): the independent uv
    /// search runs BEFORE MDS3, not before MDS0 (product_coding_loop.c:9477
    /// vs :9260) — so `ind_uv_avail` is 0 at injection time and every
    /// candidate is injected with uv-FOLLOWS-LUMA chroma
    /// (`intra_luma_to_chroma[fimode_to_intramode[..]]`, mode_decision.c
    /// :3288); the table only reaches candidates via the MDS3
    /// `update_intra_chroma_mode` rewrite (:7063, gated on
    /// `ind_uv_last_mds != 0` — so the last_mds==0 config M0 injects FROM
    /// the table and never rewrites). The table CONTENT is identical
    /// either way (the search reads only source + fixed neighbor recon and
    /// sets its own rdoq/spatial-sse/coeff-est flags), so the port builds
    /// it early for both and keys the two consumption points off this
    /// flag. false = last_mds 0 semantics (M0).
    pub ind_uv_last_mds1: bool,
    /// SH `enable_intra_edge_filter` (M5 still/420 only): directional
    /// predictions run the corner/edge filters + upsampling
    /// (enc_intra_prediction.c:181-215).
    pub edge_filter: bool,
    /// `cfl_ctrls.enabled` (set_cfl_ctrls, enc_mode_config.c:8304). In the
    /// still/allintra path (OPT_NSC_STILL_IMAGE) cfl_level is 1 for M0, 4 for
    /// M1..M6, 0 for M7+. C `cfl_prediction` runs for EVERY MDS3 intra
    /// candidate (product_coding_loop.c:7183-7193) — both the uv-follows-luma
    /// path (M6, freq-domain decision) and the independent-uv path (M0..M5,
    /// spatial-domain `check_best_indepedant_cfl`); M7+ disable it (cfl_level 0).
    pub cfl_enabled: bool,
    /// `cfl_ctrls.itr_th`: the alpha-search early-exit threshold in
    /// md_cfl_rd_pick_alpha (cfl_level 1 -> 2 [M0]; cfl_level 4 -> 1 [M1..M6]).
    pub cfl_itr_th: u8,
    /// `cfl_ctrls.cplx_th`: chroma-complexity detector threshold. 0 (cfl_level
    /// 1/2, M0) BYPASSES the detector — CfL is always evaluated (C :7183
    /// `!cplx_th`); 10 (cfl_level 4, M1..M6) gates CfL on the detector firing.
    pub cfl_cplx_th: u32,
    /// C `pcs->palette_level` for THIS frame (sc_class5-gated preset
    /// table, enc_mode_config.c:2374-2390; 0 = palette off). Stamped by
    /// the pipeline from the sc derivation next to `allow_sct`.
    pub palette_level: u8,
    /// FH `allow_screen_content_tools` for THIS frame (not a preset knob —
    /// the pipeline stamps it from the sc detector after `for_preset`).
    /// Gates the no-palette flag rates: C prices palette_ymode_fac_bits
    /// \[bctx\]\[ctx\]\[0\] into every DC candidate's luma rate
    /// (rd_cost.c:579) and palette_uv_mode_fac_bits\[0\]\[0\] into every
    /// UV_DC chroma fast rate (inside svt_aom_get_intra_uv_fast_rate,
    /// rd_cost.c:514) when `svt_aom_allow_palette` holds.
    pub allow_sct: bool,
    /// FH `allow_intrabc` for THIS frame (`svt_aom_allow_intrabc` — always
    /// I-slice + sct here; stamped by the pipeline from the sc derivation
    /// next to `allow_sct`, IBC chunk 3). On an IBC frame EVERY non-IBC
    /// candidate's luma rate is charged `intrabc_fac_bits[0]` — the coded
    /// `use_intrabc = 0` flag (rd_cost.c:629-631; the writer codes the flag
    /// for every block, entropy_coding.c:5021-5023).
    pub allow_intrabc: bool,
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
            bypass_encdec: true, // overridden from `preset` below
            filter_intra: true,
            prune_best_mode: false,
            nic_num: (6, 6, 6),
            mds1_cand_base_th: 1200,
            mds1_rank_factor: 3,
            mds2_cand_base_th: 15,
            mds2_rank_factor: 1,
            mds2_rel_dev_th: 5,
            mds3_cand_base_th: 15,
            // nic_level 6 (M5/M6) inter-class MDS3 pruning (case 6,
            // enc_mode_config.c:4711-4713). Presets 5/6/7 inherit these
            // (lvl7 == lvl6 for the class ths); 0-4 override below.
            mds3_class_th: 5,
            mds3_band_cnt: 16,
            i_mds3_class_th_mult: 50,
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
            txs_inter_max_sq: 1,
            txs_inter_max_nsq: 0,
            txt_d1_off: 3,
            txt_d2_off: 3,
            txs_prev_depth_exit: 1,
            txs_quadrant_sf: 0,
            txs_lvl6_gate: false,
            coeff_rate_est_lvl: 1,
            ind_uv_mds3: false,
            ind_uv_independent: None,
            ind_uv_last_mds1: false,
            fi_max: 0,
            edge_filter: false,
            // M6 cfl_level 4: enabled, itr_th 1, cplx_th 10 (detector-gated
            // — see chroma path). Presets that spread m6_tail but do
            // independent chroma (M0..M5) are excluded by the uv-follows-luma
            // gate; M7/M8/eff-M9 override to false (cfl_level 0).
            cfl_enabled: true,
            cfl_itr_th: 1,
            cfl_cplx_th: 10,
            palette_level: 0,
            allow_sct: false,
            allow_intrabc: false,
        };
        let mut cfg = match preset {
            // M1 (still/420): the svt_aom_get_*_allintra rows for enc_mode=1
            // give the SAME funnel-relevant config as M2 — nic_level 3
            // (svt_aom_get_nic_level_allintra :5994 `<= ENC_M2` -> 3),
            // txt_level 2, txs_level 2, filter_intra level 2 (fi_max 0 =
            // FILTER_DC only, get_filter_intra_level_allintra :12683
            // `<= ENC_M6` -> 2), intra_level 1 (mode_end PAETH, ang 1) —
            // EXCEPT chroma_level 2 (svt_aom_get_chroma_level_allintra
            // :12233 `<= ENC_M1` -> 2: ind_uv_last_mds=1, uv_nic 8,
            // skip_ind_uv_if_only_dc=0; set_chroma_controls case 2, :5757)
            // vs M2's chroma_level 4 (ind_uv_last_mds=2). This IS binding
            // even on flat chroma: chroma_level 2 runs
            // `search_best_independent_uv_mode` (a full independent uv
            // search whose distortion-sorted prune drops UV_PAETH), so a
            // luma-PAETH block resolves to UV_DC — whereas chroma_level 4's
            // `search_best_mds3_uv_mode` tests the survivors' uv-follows-
            // luma modes and picks UV_PAETH (cheap in the luma-conditioned
            // uv CDF). Differ-verified on g128 q55: C M1 codes UV_DC where
            // C M2 codes UV_PAETH. The other M1-vs-M2 deltas live outside
            // FunnelCfg — nsq_search level 10 vs 14 (NsqCfg::for_preset_qp)
            // and PD0_LVL_0 vs LVL_1 (the PD0 pick).
            // M0 (still/420): the svt_aom_get_*_allintra rows for enc_mode=0.
            // Deltas vs M1 (each C-verified):
            // - nic_level 1 (svt_aom_get_nic_level_allintra :5988 `<= ENC_M0`
            //   with OPT_NSC_STILL_IMAGE -> 1; set_nic_controls case 1 :6060):
            //   nic_scaling_level 0 -> MD_STAGE_NICS_SCAL_NUM[0] = {20,20,20};
            //   mds1_cand_base_th_intra MAX (no mds1 cand pruning), mds1 rank
            //   0; mds2/mds3 cand base 50, rank 0, rel_dev 0. (mds2/mds3 class
            //   ths 25/25 are single-intra-class-dead like the M2 case.)
            // - chroma_level 1 (svt_aom_get_chroma_level_allintra :12231
            //   `<= ENC_M0` -> 1; set_chroma_controls case 1 :5747):
            //   ind_uv_last_mds=0, uv_nic 16, skip_ind_uv_if_only_dc=0 — the
            //   independent uv search with a WIDER prune (nfl = 32*16/16 = 32).
            // - filter_intra level 1 (get_filter_intra_level_allintra :12681
            //   `<= ENC_M0` -> 1; set_filter_intra_ctrls case 1 :8053):
            //   max_filter_intra_mode FILTER_PAETH_PRED -> all five fi modes
            //   are candidates (fi_max 4), vs M1's fi_max 0 (FILTER_DC only).
            // - nsq_search level 3 vs M1's 10 (NsqCfg::for_preset_qp).
            // pd0_lvl 0, txt_level 2, txs_level 2, intra_level 1, dr_level 6
            // are all shared with M1.
            0 => FunnelCfg {
                mode_end: 12,
                angular_level: 1,
                nic_num: (20, 20, 20),
                mds1_cand_base_th: u64::MAX,
                mds1_rank_factor: 0,
                mds2_cand_base_th: 50,
                mds2_rank_factor: 0,
                mds2_rel_dev_th: 0,
                mds3_cand_base_th: 50,
                // nic_level 1 (case 1, enc_mode_config.c:4561-4562).
                mds3_class_th: 25,
                mds3_band_cnt: 4,
                txt_group_lt16: 6,
                txt_group_ge16: 6,
                txt_satd_th: 20,
                txt_rate_th: 250,
                txs_max_sq: 2,
                txs_max_nsq: 2,
                txs_inter_max_sq: 1,
                txs_inter_max_nsq: 1,
                // txs_level 2 inter caps (set_txs_controls case 2).
                txt_d1_off: 0,
                txt_d2_off: 0,
                fi_max: 4,
                ind_uv_mds3: false,
                ind_uv_independent: Some(16),
                // M0 cfl_level 1: itr_th 2, cplx_th 0 (detector bypassed —
                // CfL always evaluated). M1..M6 keep m6_tail's level-4 (1/10).
                cfl_itr_th: 2,
                cfl_cplx_th: 0,
                ..m6_tail
            },
            1 => FunnelCfg {
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
                txs_inter_max_sq: 1,
                txs_inter_max_nsq: 1,
                // txs_level 2 inter caps (set_txs_controls case 2).
                txt_d1_off: 0,
                txt_d2_off: 0,
                ind_uv_mds3: false,
                ind_uv_independent: Some(8),
                ind_uv_last_mds1: true,
                // nic_level 3 (case 3, enc_mode_config.c:4621-4622).
                mds3_class_th: 25,
                mds3_band_cnt: 8,
                ..m6_tail
            },
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
                // nic_level 3 (case 3, enc_mode_config.c:4621-4622).
                mds3_class_th: 25,
                mds3_band_cnt: 8,
                txt_group_lt16: 6,
                txt_group_ge16: 6,
                txt_satd_th: 20,
                txt_rate_th: 250,
                txs_max_sq: 2,
                txs_max_nsq: 2,
                txs_inter_max_sq: 1,
                txs_inter_max_nsq: 1,
                // txs_level 2 inter caps (set_txs_controls case 2).
                txt_d1_off: 0,
                txt_d2_off: 0,
                ind_uv_mds3: true,
                ..m6_tail
            },
            3 => FunnelCfg {
                mode_end: 12,
                angular_level: 1,
                // nic_level 5 (case 5, enc_mode_config.c:4681): class_th 15,
                // band 16 (== m6_tail).
                mds3_class_th: 15,
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
                txs_inter_max_sq: 1,
                txs_inter_max_nsq: 1,
                // txs_level 2 inter caps (set_txs_controls case 2).
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
                // nic_level 5 (case 5, enc_mode_config.c:4681): class_th 15,
                // band 16 (== m6_tail).
                mds3_class_th: 15,
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
            // M7 (still/420): intra_level 7 (set_intra_ctrls case 7:
            // mode_end SMOOTH, angular 4, prune_using_best_mode 1,
            // prune_using_edge_info 0; enc_mode_config.c:8577), nic_level 7
            // (scaling 8 -> nums 4/4/4; set_nic_controls case 7 mds1_base
            // 1200/rank3, mds2 15/1/5, mds3 15 == M6), txs_level 3 (== M6),
            // filter_intra 0 (get_filter_intra_level_allintra > ENC_M6).
            // Deltas from m6_tail that were previously MISSED (latent on
            // synthetic, binding on real content):
            // - rate_est_level 4 (enc_mode_config.c:15040 `<= ENC_M8`) ->
            //   set_rate_est_ctrls case 4: coeff_rate_est_lvl 2 (the LUMA
            //   fast approximation, applied in tx_unit), update_skip_*_ctx
            //   0/0 (real_coeff_ctx false).
            // - txt_level 10 (enc_mode_config.c:15000 `<= ENC_M8`) ->
            //   set_txt_controls case 10: txt_group_intra lt16 3 / ge16 2,
            //   txt_rate_cost_th 50 (satd_early_exit 10 == M6's case 8).
            7 => FunnelCfg {
                filter_intra: false,
                prune_best_mode: true,
                nic_num: (4, 4, 4),
                real_coeff_ctx: false,
                coeff_rate_est_lvl: 2,
                txt_group_lt16: 3,
                txt_group_ge16: 2,
                txt_rate_th: 50,
                cfl_enabled: false,
                ..m6_tail
            },
            // preset 8: nic_level 11 (scaling 15 -> nums 0/0/0 -> 1/1/1),
            // all cand thresholds 1, enable_skipping_mds1 (n1==1 makes it a
            // no-op for the pick), txs_level 0. Shares M7's rate_est_level 4
            // (coeff_rate_est_lvl 2) and txt_level 10 (groups 3/2, rate_th
            // 50) — the same previously-missed real-content deltas.
            8 => FunnelCfg {
                filter_intra: false,
                prune_best_mode: true,
                nic_num: (0, 0, 0),
                mds1_cand_base_th: 1,
                mds2_cand_base_th: 1,
                mds3_cand_base_th: 1,
                real_coeff_ctx: false,
                txs_on: false,
                coeff_rate_est_lvl: 2,
                txt_group_lt16: 3,
                txt_group_ge16: 2,
                txt_rate_th: 50,
                cfl_enabled: false,
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
                // eff-M9: pcs->txs_level is 0 at the picture level, but the
                // FTR_COUPLE_VLPD0_TXS_PER_SB coupling bumps it per-SB to
                // MAX_TXS_LEVEL-1 (=5) for SBs the pd0 detector leaves at
                // PD0_LVL_6 (undemoted) — set_txs_controls case 5: intra
                // sq/nsq max depth 1, prev_depth_coeff_exit 100,
                // quadrant_th_sf 100 (enc_mode_config.c:8024, :11366). The
                // per-SB gate is applied in evaluate_leaf via txs_lvl6_gate.
                txs_on: true,
                txs_max_sq: 1,
                txs_max_nsq: 1,
                txs_prev_depth_exit: 100,
                txs_quadrant_sf: 100,
                txs_lvl6_gate: true,
                coeff_rate_est_lvl: 0,
                dc_only_gate: true,
                txt_on: false,
                cfl_enabled: false,
                ..m6_tail
            },
        };
        cfg.bypass_encdec = preset >= 4;
        cfg
    }
}

/// C `RDCOST` (rd_cost.h:36).
#[inline]
pub(super) fn rdcost(lambda: u64, rate: u64, dist: u64) -> u64 {
    ((rate * lambda + 256) >> 9) + (dist << 7)
}

/// C `DIVIDE_AND_ROUND`.
#[inline]
pub(super) fn div_round(x: u64, y: u64) -> u64 {
    (x + (y >> 1)) / y
}

/// C `svt_aom_get_qp_based_th_scaling_factors(true, ..)` — the pd0 port.
pub(super) fn qp_scale_factors(cli_qp: u32) -> (u64, u64) {
    let (w, d) = crate::pd0::qp_th_scaling_factors(cli_qp);
    (w as u64, d as u64)
}

/// NIC counts for I-slice class 0 at the config's scaling nums:
/// `svt_aom_set_nics` (product_coding_loop.c:1347), base {64, 32, 16}
/// (MD_STAGE_NICS[I][C0] = 64, >>1, >>2), scaled by num/16 then qp-scaled.
/// `min_nics = 2` when the stage's scaling num != 0 (I-slice pic_type < 2),
/// else 1 — so nums 0/0/0 (nic level 15/M8) yield 1/1/1.
pub(super) fn nic_counts(cli_qp: u32, num: (u64, u64, u64)) -> (u32, u32, u32) {
    let (qw, qwd) = qp_scale_factors(cli_qp);
    let scale = |base: u64, num: u64| -> u32 {
        let min = if num != 0 { 2u64 } else { 1u64 };
        let n = min.max(div_round(base * num, 16));
        min.max(div_round(n * qw, qwd)) as u32
    };
    (scale(64, num.0), scale(32, num.1), scale(16, num.2))
}

/// C `svt_nxm_sad_kernel` (svt_nxm_sad_kernel_helper_c, compute_sad_c.c:21) —
/// the plain 8-bit SAD (sum of absolute differences) used as the bd8 fast-loop
/// chroma distortion in `search_best_independent_uv_mode`
/// (product_coding_loop.c:7643). `ctx->mds0_ctrls.mds0_dist_type` is NEVER
/// assigned anywhere in the C tree (definitions.h:892 `enum { SAD=0, VAR=1,
/// SSD=2 }`, and grep of `Source/Lib` finds no `mds0_dist_type =` site), so it
/// stays zero-initialized = SAD for EVERY preset/bit-depth — the fast loop
/// scores SAD, not the `vf` variance. `residual_sad`/`residual_sad_hbd` are the
/// u8/u16 halves of the same metric (C picks `svt_nxm_sad_kernel` vs
/// `sad_16b_kernel` on `hbd_md`). Using variance here (DC-invariant) mis-orders
/// the candidate SET on non-flat recon: a flat prediction scores 0 and displaces
/// the above-following modes (V/PAETH/D45) that SAD ranks best, dropping UV_PAETH
/// from the nfl=32 survivors where C keeps it — the gradient q32 p0 32x32 VERT_4
/// pin (a 4x16 chroma block whose luma-PAETH sub-block resolved to UV_DC under
/// variance but UV_PAETH under SAD, flipping the whole node NONE<->VERT_4).
pub(super) fn residual_sad(
    src: &[u8],
    src_stride: usize,
    sx: usize,
    sy: usize,
    pred: &[u8],
    w: usize,
    h: usize,
) -> u64 {
    let mut sad: u64 = 0;
    for r in 0..h {
        let base = (sy + r) * src_stride + sx;
        for c in 0..w {
            sad += (src[base + c] as i64 - pred[r * w + c] as i64).unsigned_abs();
        }
    }
    sad
}

/// C `sad_16b_kernel` (svt_aom_sad_16b_kernel_c) — the plain 16-bit SAD (sum of
/// absolute differences) used as the bd10 fast-loop chroma distortion in
/// `search_best_independent_uv_mode` when `mds0_dist_type != VAR`
/// (product_coding_loop.c:7658). `mds0_dist_type` is NEVER assigned in the C
/// tree (definitions.h:892 `enum { SAD=0, VAR=1, SSD=2 }`, default 0 = SAD), so
/// the ind_uv fast loop scores SAD, NOT the `vf_hbd_10` variance the mainline
/// LUMA mds0 uses unconditionally (product_coding_loop.c:1004). Using variance
/// here mis-orders the candidate SET on non-flat recon: variance is DC-invariant
/// so a flat prediction (e.g. off-frame-left H) scores 0 and displaces the
/// above-following modes (V/PAETH/D45) that SAD ranks best, dropping UV_PAETH
/// from the nfl=32 survivors where C keeps it.
pub(super) fn residual_sad_hbd(
    src: &[u16],
    src_stride: usize,
    sx: usize,
    sy: usize,
    pred: &[u16],
    w: usize,
    h: usize,
) -> u64 {
    let mut sad: u64 = 0;
    for r in 0..h {
        let base = (sy + r) * src_stride + sx;
        for c in 0..w {
            sad += (src[base + c] as i64 - pred[r * w + c] as i64).unsigned_abs();
        }
    }
    sad
}
