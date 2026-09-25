//! C-exact light-PD0 partition decision for allintra high presets.
//!
//! SVT-AV1 clamps allintra presets above M9 down to M9
//! (`enc_handle.c:4634-4644`), and at effective M9 the final partition
//! tree of every superblock is decided ENTIRELY by the PD0 pass:
//! `pred_depth_only=1`, `md_disallow_nsq_search=1`, `fixed_partition=1`,
//! so PD1 (light-PD1) codes exactly the PD0-picked {NONE, SPLIT} square
//! quadtree (no HORZ/VERT/AB/4:1 shapes are ever evaluated).
//!
//! This module ports that decision verbatim from the C sources
//! (v4.2.0-rc, all `CLN_RENAME_PD0`/`OPT_VLPD0_*` feature macros = 1):
//!
//! - `compute_b64_variance` (pic_analysis_process.c:312) — the 85-entry
//!   per-64x64 variance map at `BLOCK_MEAN_PREC_SUB` (even-row
//!   subsampled means), used by every decision below.
//! - `svt_aom_get_qp_based_th_scaling_factors` (md_config_process.c) —
//!   qp-based threshold scaling (both `lpd0_` and `cap_max_size_`
//!   variants are enabled at every preset, enc_handle.c:3990-4007).
//! - `get_max_block_size_allintra` (enc_mode_config.c:8969) — at
//!   effective >= M8 the 64x64 depth is REMOVED whenever the SB's 64x64
//!   source variance exceeds `round(7500 * qw / qwd)`; PD0 then has no
//!   parent cost at 64x64 and SPLIT is forced.
//! - `pd0_detector_allintra` (enc_dec_process.c:2373) — demotes
//!   `PD0_LVL_6 -> PD0_LVL_5` when the per-depth normalized variances
//!   are flat (no dominant depth).
//! - `compute_lpd0_cost_allintra` (product_coding_loop.c:8418) — the
//!   LVL_6 closed-form variance cost.
//! - `md_encode_block_pd0`/`full_loop_core_pd0`/`perform_tx_pd0`
//!   (product_coding_loop.c) — the LVL_5 light block encode: single
//!   DC_PRED candidate (inject_intra_candidates_pd0), prediction from
//!   SOURCE neighbors (`pd0_use_src_samples=1` for allintra,
//!   enc_mode_config.c:9437) with the spec unavailable-edge fills,
//!   max-square TX at depth 0 with optional row subsampling (subres
//!   step 1; gated per SB by `check_is_subres_safe` on the 64x64 DC
//!   prediction), `svt_aom_quantize_b` at `qindex + 8`
//!   (rate_est_ctrls.lpd0_qp_offset), frequency-domain SSE distortion
//!   (coeff vs dequantized coeff over the packed <=32x32 region plus
//!   `three_quad_energy`), coefficient rate `5000 + 100*eob`
//!   (`coeff_rate_est_lvl == 0`, product_coding_loop.c:4568), and
//!   `full_cost = RDCOST(lambda, bits + skip_bits + part_none_bits,
//!   dist)` (svt_aom_full_cost_pd0, rd_cost.c:1335).
//! - `test_split_partition_pd0` (product_coding_loop.c:10897) — the
//!   parent-vs-children compare: `split_cost = RDCOST(lambda,
//!   2 * partition_split_bits, 0) + sum(children)` (the x2 because
//!   `use_accurate_part_ctx = enc_mode <= M8` is false at M9; the split
//!   rate term is 0 entirely at LVL_6 allintra), parent wins iff
//!   `1000 * parent <= 1000 * split` (parent_cost_bias = 1000 for
//!   allintra), with the LVL_5-only early exits (split_cost_th=50,
//!   early_exit_th=0 -> treated as 1000).
//! - `svt_aom_compute_rd_mult` KF chain (rc_process.c:452) — the PD0
//!   lambda: `(3.3 + 0.0015*dc_q) * dc_q^2` truncated, `*150 >> 7`
//!   (rd_frame_type_factor[8bit][KF]); the stats-based factor is 128
//!   (qdiff 0) and lambda_scale_factors are 128, both no-ops.
//!
//! Every constant and every per-block cost in the unit tests below was
//! captured from the instrumented C library running the identity-harness
//! gradient-64 configs (docs/IDENTITY-STATUS.md, 2026-07-13 diagnosis).

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use archmage::prelude::*;

// ---------------------------------------------------------------------------
// Variance map (pic_analysis_process.c compute_b64_variance, PREC_SUB)
// ---------------------------------------------------------------------------

use svtav1_types::math::shift_u32::divide_and_round_u64 as divide_and_round;

use svtav1_types::math::rd::rdcost_u64 as rdcost;
#[cfg(test)]
mod pd0_quant_parity_tests;
#[cfg(test)]
mod research_lambda_tests;
/// PD0-picked square partition tree: leaves carry the block size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pd0Tree {
    Leaf(usize),
    Split(Box<[Pd0Tree; 4]>),
    /// A quadrant whose top-left lies at/after the ALIGNED frame extent —
    /// it codes NOTHING (C `svt_aom_write_modes_sb`'s `mi_row >= mi_rows ||
    /// mi_col >= mi_cols` early return / the SPLIT-loop `continue`). Only
    /// produced on partial superblocks (task #95 chunk 2); a 64-aligned
    /// frame never generates it.
    Off,
}

/// Fixed 8x8 leaf tree for lossless coding when 4x4 blocks are disallowed
/// (allintra color presets >= 4), also used by the monochrome lossless path.
/// C caps square candidates at 8x8 when `mimic_only_tx_4x4` is set
/// (enc_dec_process.c:1492). At color presets 0..3, the pipeline instead
/// runs PD0 and unrestricted PD1 to choose between 8x8 and 4x4 blocks.
/// Quadrants outside the aligned frame extent are `Off`.
pub fn lossless_tree(
    x0: usize,
    y0: usize,
    size: usize,
    aligned_w: usize,
    aligned_h: usize,
) -> Pd0Tree {
    if x0 >= aligned_w || y0 >= aligned_h {
        return Pd0Tree::Off;
    }
    if size <= 8 {
        return Pd0Tree::Leaf(size);
    }
    let half = size / 2;
    Pd0Tree::Split(Box::new([
        lossless_tree(x0, y0, half, aligned_w, aligned_h),
        lossless_tree(x0 + half, y0, half, aligned_w, aligned_h),
        lossless_tree(x0, y0 + half, half, aligned_w, aligned_h),
        lossless_tree(x0 + half, y0 + half, half, aligned_w, aligned_h),
    ]))
}

impl Pd0Tree {
    /// Leaf sizes in raster/coding order (debug aid). Off quadrants
    /// contribute nothing.
    pub fn leaf_sizes(&self) -> Vec<usize> {
        match self {
            Pd0Tree::Leaf(s) => vec![*s],
            Pd0Tree::Split(ch) => ch.iter().flat_map(|c| c.leaf_sizes()).collect(),
            Pd0Tree::Off => vec![],
        }
    }
}

/// PD0 evaluation record for one square node — the C `pc_tree` fields the
/// PD1 depth refinement reads (`tested_blk[PART_N][0]`,
/// `block_data[PART_N][0]->cost`, `partition`): every node the PD0 walk
/// visited carries whether its PART_N block was costed and that cost.
/// Children exist whenever the split test recursed into them (quadrants
/// skipped by the split-cost early exit stay `tested = false` with no
/// children, exactly like C's untouched `pc_tree->split[i]`).
#[derive(Debug, Clone)]
pub struct Pd0Eval {
    pub sq: usize,
    /// Some d1 shape was costed at this node — C `pc_tree->rdc.valid` after
    /// `svt_aom_pick_partition_pd0`. At a one-false BOUNDARY node the costed
    /// shape is PART_H/PART_V, NOT the square: see [`Pd0Eval::sq_tested`].
    pub tested: bool,
    /// C `tested_blk[PART_N][0]` — the SQUARE PART_N was costed at this node,
    /// so `block_data[PART_N][0]->cost` is readable.
    ///
    /// This is STRICTLY narrower than [`Pd0Eval::tested`] on a partial SB:
    /// `svt_aom_pick_partition_pd0` (product_coding_loop.c:10548-10560) writes
    /// `block_data[shape][0]` for the ONE shape `set_blocks_to_test` injected,
    /// which at a single-edge node is PART_H / PART_V. Every PD1
    /// depth-refinement gate that reads a PD0 cost is guarded on
    /// `tested_blk[PART_N][0]` for exactly that reason — C spells it out at
    /// `update_pred_th_offset` (enc_dec_process.c:1547-1549): *"For incomplete
    /// blocks, H/V partitions may be allowed, while square is not. In those
    /// cases, the selected depth may not have a valid SQ cost, so we need to
    /// check that the SQ block is available before using the cost."*
    /// Consequence: a boundary PD0 leaf gets `s_depth = e_depth = 0` — it is
    /// never refined, only coded at its own depth.
    ///
    /// Identical to `tested` on a 64-aligned frame (no node is one-false).
    pub sq_tested: bool,
    /// C `pc_tree->rdc.rd_cost` — the costed shape's cost (valid iff
    /// `tested`). The SQUARE cost only when `sq_tested`.
    pub cost: u64,
    /// PD0 picked SPLIT at this node (`pc_tree->partition`).
    pub split: bool,
    /// This node's top-left is at/after the ALIGNED frame extent — it codes
    /// nothing (partial-SB off-frame quadrant, task #95 chunk 2). Mutually
    /// exclusive with `tested`/`split`. Never set on a 64-aligned frame.
    pub off: bool,
    pub children: Option<Box<[Pd0Eval; 4]>>,
    /// The ROOT node's block-MD data (`pc_tree->block_data[PART_N][0]`) that
    /// `lpd1_detector_post_pd0` reads — meaningful only on the root eval. `None`
    /// on every child and on key frames (PD0's inter arm never ran).
    pub root_det: Option<Pd0RootDet>,
}

/// The ROOT node's `lpd1_detector_post_pd0` inputs — `pc_tree->rdc.rd_cost`
/// plus the `block_data[PART_N][0]` block payload that exists only when the
/// 64x64 PART_N block was costed at PD0.
#[derive(Clone, Copy, Debug)]
pub struct Pd0RootDet {
    /// `pc_tree->rdc.rd_cost` — the root node's CHOSEN cost (the min of its
    /// PART_N leaf and SPLIT children), i.e. the `total` [`Pd0Ctx::pick`]
    /// returns for org `(0,0)`. Filled by the top-level driver after `pick`.
    pub rd_cost: u64,
    /// `pc_tree->block_data[PART_N][0]` — `Some` only when the root 64x64
    /// PART_N block was costed (`tested_blk[PART_N][0]`). `None` reads as
    /// C's `nz_coeffs = ~0` / MV-test-skipped arm.
    pub blk: Option<Pd0RootBlk>,
}

/// C `pc_tree->block_data[PART_N][0]` fields `lpd1_detector_post_pd0` reads —
/// the root 64x64 block's winning mode/MV/coefficient count.
#[derive(Clone, Copy, Debug)]
pub struct Pd0RootBlk {
    /// `is_inter_mode(block_mi.mode)` — false when PD0's intra arm won.
    pub is_inter: bool,
    /// `block_mi.mv[0/1]` in quarter-pel units (`mv1 == ZERO` for unipred).
    pub mv0: svtav1_types::motion::Mv,
    pub mv1: svtav1_types::motion::Mv,
    /// `has_second_ref(&block_mi)` — the winner was a compound (bipred) block.
    pub bipred: bool,
    /// `cnt_nz_coeff` — the root block's `eob` after `tx_quant_core`.
    pub nz: u32,
}

impl Pd0Eval {
    fn untested(sq: usize) -> Self {
        Pd0Eval {
            sq,
            tested: false,
            sq_tested: false,
            cost: 0,
            split: false,
            off: false,
            children: None,
            root_det: None,
        }
    }

    /// An off-frame quadrant (top-left >= aligned extent): codes nothing.
    fn off(sq: usize) -> Self {
        Pd0Eval {
            sq,
            tested: false,
            sq_tested: false,
            cost: 0,
            split: false,
            off: true,
            children: None,
            root_det: None,
        }
    }

    /// The picked partition tree this eval corresponds to.
    pub fn tree(&self) -> Pd0Tree {
        if self.off {
            Pd0Tree::Off
        } else if self.split {
            let ch = self.children.as_ref().expect("split node has children");
            Pd0Tree::Split(Box::new([
                ch[0].tree(),
                ch[1].tree(),
                ch[2].tree(),
                ch[3].tree(),
            ]))
        } else {
            Pd0Tree::Leaf(self.sq)
        }
    }

    /// C `get_max_min_pd0_depths` (enc_dec_process.c:1959): max/min PICKED
    /// leaf sizes over the tree. Off-frame quadrants contribute nothing (C
    /// only walks in-bounds sub-trees).
    pub fn max_min_picked(&self, max: &mut usize, min: &mut usize) {
        if self.off {
            return;
        }
        if self.split {
            for c in self.children.as_ref().expect("split children").iter() {
                c.max_min_picked(max, min);
            }
        } else {
            *max = (*max).max(self.sq);
            *min = (*min).min(self.sq);
        }
    }
}

/// Which PD0 block-encode path prices a block (C `Pd0Level`, collapsed to
/// the three configurations reachable from the allintra preset ladder).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pd0Mode {
    /// PD0_LVL_6 closed-form variance cost (eff-M9, undemoted).
    Lvl6,
    /// PD0_LVL_5 light encode: qindex+8, subres, 5000+100*eob rate
    /// (eff-M9 demoted by the detector).
    Lvl5,
    /// PD0_LVL_0 full-RD partition search — C `set_pd0_ctrls`
    /// (enc_mode_config.c:5415) FORCES this level whenever `hbd_md` is set
    /// (i.e. bit_depth 10 DUAL), regardless of preset. PD0 itself runs
    /// entirely at 8-BIT (`enc_dec_process.c:2965` saves hbd_md and forces
    /// it to 0 for the whole PD0 pass), so the partition tree is a pure
    /// function of the 8-bit MSB-truncated plane — NO bd10 pixel/quant/lambda
    /// kernel is on the partition path.
    ///
    /// The block cost is IDENTICAL to [`Pd0Mode::Lvl5`] (same DC-from-source
    /// prediction, `lpd0_qp_offset = 8` -> qindex+8, `coeff_rate_est_lvl = 0`
    /// closed form `5000 + ires*1600 + 100*eob`, doubled split rate because
    /// `use_accurate_part_ctx = 0` above M8) EXCEPT that **subres is OFF**:
    /// LVL_0 is `pd0_level <= PD0_LVL_2` so `subres_level = 0`
    /// (enc_mode_config.c:7327), whereas LVL_5 enables step-1 subres via the
    /// odd/even-deviation check. There is also NO PD0-level detector: every
    /// SB runs the full block encode (LVL_5 only runs it when the detector
    /// demotes LVL_6). Verified end-to-end against real C's SVT_PD0COST_OUT +
    /// SVT_CTREE_OUT dumps at bd10.
    Lvl0,
    /// PD0_LVL_1 (allintra M2..M8): qindex+0, no subres, real
    /// `svt_av1_cost_coeffs_txb` rate at zero contexts, undoubled split
    /// rate (`use_accurate_part_ctx = 1`).
    Lvl1,
    /// PD0_LVL_3 — the VIDEO arm's level at M3..M8 (`set_pic_pd0_lvl_default`,
    /// enc_mode_config.c:8592; `set_pd0_ctrls` case 3, `:5435`, arms no
    /// detector at all).
    ///
    /// Block cost is [`Pd0Mode::Lvl1`]'s — `rate_est_level` is 2 for every
    /// `pd0_level <= PD0_LVL_3` (`svt_aom_sig_deriv_enc_dec_pd0`,
    /// enc_mode_config.c:7357), i.e. `lpd0_qp_offset = 0` +
    /// `coeff_rate_est_lvl = 1`, exactly LVL_1's — with TWO differences, both
    /// keyed on the level rather than the preset and neither of them visible
    /// in `sig_deriv_mode_decision_config`'s slot table:
    ///
    /// * **subres step 1.** `pd0_level <= PD0_LVL_2` forces `subres_level = 0`
    ///   (`:7337`); at LVL_3 an I-slice takes `subres_level = 1` outright
    ///   (`:7345`), still gated on `disallow_4x4` and a complete b64. The
    ///   per-SB `is_subres_safe` odd/even-deviation check then runs on the
    ///   64x64 exactly as it does at LVL_5.
    /// * **`depth_early_exit_lvl` 2** (`:7233`) — `early_exit_th` 900 instead
    ///   of LVL_1's 0-which-reads-as-1000, for the i > 0 quadrants.
    Lvl3,
    /// PD0_LVL_4 — the VIDEO arm's level at M9..M13 on <= 360p content
    /// (`set_pic_pd0_lvl_default`; `set_pd0_ctrls` case 4 arms a detector that
    /// is INERT on an I-slice — every branch of `pd0_detector` below the
    /// LVL_6 demote is `slice_type != I_SLICE`-gated, enc_dec_process.c:2473).
    ///
    /// Identical to [`Pd0Mode::Lvl3`] except that `rate_est_level` is **4**
    /// (`:7359`), i.e. `coeff_rate_est_lvl = 2` — the `eob < th ? 6000 +
    /// eob*500 : real` approximation `lvl1_block_cost_rect` already carries
    /// for the allintra M7/M8 rows.
    Lvl4,
}

impl Pd0Mode {
    /// `subres_ctrls.step` for this level when no per-SB
    /// `svt_aom_sig_deriv_enc_dec_pd0` resolution is threaded in — the value
    /// every pre-resolution caller priced with (1 for LVL_3/4/5, 0
    /// otherwise). `sig_deriv_enc_dec_pd0`'s own ladder (`enc_mode_config.c:
    /// 7327-7352`) can lower this to 0 (`cost_64x64 >= use_subres_th` at
    /// LVL_3/4) or raise it to 2 (leaf-layer LVL_5); the video-arm entry
    /// points take the resolved value instead.
    fn default_subres_step(self) -> u32 {
        u32::from(matches!(self, Self::Lvl3 | Self::Lvl4 | Self::Lvl5))
    }
}

/// The rate tables PD0_LVL_1 prices with. For single-SB frames these are
/// the default tables at the frame qindex bucket (C: `md_frame_context`
/// feeds SB 0); multi-SB refresh from the evolving frame context
/// (enc_dec_process.c:2991, `cdf_ctrl.enabled` at M6) is NOT yet ported —
/// SBs after the first reuse the defaults, which C only does for SB 0.
pub struct M6Pd0Tables {
    pub coeff: alloc::boxed::Box<crate::quant::CoeffCostTables>,
    tx_rates: TxTypeRatesDc,
    tx_rates_inter: TxTypeRatesInter,
    /// PARTITION_SPLIT rate per square size (index by log2(sq) - 3:
    /// 8/16/32/64), from THIS SB's chained partition CDFs (ctx row 0).
    split_bits: [u64; 4],
    /// BINARY SPLIT rate for a one-false BOUNDARY node, per square size —
    /// C `svt_aom_partition_rate_cost` boundary branch (rd_cost.c:1846-1863):
    /// the bottom-edge (`!has_rows`) uses `partition_vert_alike_fac_bits`, the
    /// right-edge (`!has_cols`) `partition_horz_alike_fac_bits`, indexed
    /// `[ctx][SPLIT]` — NOT the full-alphabet `split_bits`. Gather is
    /// CROSS-named vs the option. Slot 0 (8x8) is never used (8x8 is never an
    /// edge node).
    vert_alike_split_bits: [u64; 4],
    horz_alike_split_bits: [u64; 4],
    /// `partition_fac_bits[0][PARTITION_NONE]` (context index 0 — the
    /// 8x8-class row, rd_cost.c:1344-1349 approximation).
    none_bits_ctx0: u64,
    /// `skip_fac_bits[0][0]`.
    skip0_bits: u64,
}

/// Build the PD0_LVL_1 tables for a frame (default CDFs at `qindex`).
pub fn build_m6_pd0_tables(qindex: u8) -> M6Pd0Tables {
    let fc = crate::entropy::context::FrameContext::new_default();
    let cfc = crate::entropy::coeff_c::CoeffFc::default_for_qindex(qindex);
    build_m6_pd0_tables_from_ctx(&fc, &cfc)
}

/// [`build_m6_pd0_tables`] over an ARBITRARY (chained) context pair — the
/// per-SB `ec_ctx_array[sb]` rate refresh C runs at update_cdf_level 2
/// (enc_dec_process.c:3024-3043; the drifting 64x64 SPLIT rates
/// 1195 -> 1221 -> 1244 -> 1268 across g128 q55's SBs come from here).
pub fn build_m6_pd0_tables_from_ctx(
    fc: &crate::entropy::context::FrameContext,
    cfc: &crate::entropy::coeff_c::CoeffFc,
) -> M6Pd0Tables {
    // partition ctx row for sub-context 0 of each size class: bsl*4
    // (pipeline EntropyCtx::partition_ctx semantics; nsyms 10 for the
    // square 8..64 classes at ctx rows 0..=15 except row 0 = 4 syms).
    let mut split_bits = [0u64; 4];
    let mut vert_alike_split_bits = [0u64; 4];
    let mut horz_alike_split_bits = [0u64; 4];
    let mut none_bits_ctx0 = 0u64;
    for (slot, sq) in [(0usize, 8usize), (1, 16), (2, 32), (3, 64)] {
        let bsl = match sq {
            8 => 0usize,
            16 => 1,
            32 => 2,
            _ => 3,
        };
        let ctx = bsl * 4;
        let nsyms = if ctx <= 3 { 4 } else { 10 };
        let mut costs = [0i32; 10];
        crate::quant::syntax_rate_from_cdf(&mut costs[..nsyms], &fc.partition_cdf[ctx]);
        split_bits[slot] = costs[crate::partition::PartitionType::Split as usize] as u64;
        // Binary boundary SPLIT rate at the same ctx row (left = above = 0).
        // is_128 = false: PD0 squares here are <= 64. Slot 0 (8x8) computes a
        // value that is never consumed (8x8 is never an edge node).
        vert_alike_split_bits[slot] = crate::entropy::context::partition_alike_split_cost(
            &fc.partition_cdf[ctx],
            true, // !has_rows -> vert_alike (bottom edge)
            false,
        ) as u64;
        horz_alike_split_bits[slot] = crate::entropy::context::partition_alike_split_cost(
            &fc.partition_cdf[ctx],
            false, // !has_cols -> horz_alike (right edge)
            false,
        ) as u64;
        if sq == 8 {
            none_bits_ctx0 = costs[crate::partition::PartitionType::None as usize] as u64;
        }
    }
    let mut skip_costs = [0i32; 2];
    crate::quant::syntax_rate_from_cdf(&mut skip_costs, &fc.skip_cdf[0]);
    M6Pd0Tables {
        coeff: crate::quant::build_coeff_cost_tables_from_fc(cfc),
        tx_rates: build_tx_type_rates_dc_from_fc(cfc),
        tx_rates_inter: build_tx_type_rates_inter_from_fc(cfc),
        split_bits,
        vert_alike_split_bits,
        horz_alike_split_bits,
        none_bits_ctx0,
        skip0_bits: skip_costs[0] as u64,
    }
}

impl M6Pd0Tables {
    #[inline]
    fn size_slot(sq_size: usize) -> usize {
        // BENIGN `_ => 3`: slot 3 is BLOCK_64X64. This table only ever sees
        // PD0 squares in {8,16,32,64} — even at SB128 the b64-coding-unit
        // decomposition (`sb_coding_units`) keeps every coding square <= 64. So
        // `_` folds 64 (never 128) into slot 3. NOT the `EntropyCtx::bsl` class
        // of `_ => 3` bug (which wrongly folded 128 into the 64 level); there is
        // no 128 slot here because no 128 square reaches this function.
        match sq_size {
            8 => 0,
            16 => 1,
            32 => 2,
            _ => 3,
        }
    }
    #[inline]
    pub(crate) fn split_bits(&self, sq_size: usize) -> u64 {
        self.split_bits[Self::size_slot(sq_size)]
    }
    /// Binary boundary SPLIT rate for a one-false node. `bottom_edge`
    /// (`!has_rows`) -> vert_alike; else (right edge, `!has_cols`) -> horz_alike.
    #[inline]
    fn boundary_split_bits(&self, sq_size: usize, bottom_edge: bool) -> u64 {
        let slot = Self::size_slot(sq_size);
        if bottom_edge {
            self.vert_alike_split_bits[slot]
        } else {
            self.horz_alike_split_bits[slot]
        }
    }
}

/// The INTER arm of PD0's per-block prediction on a NON-KEY frame.
///
/// C's PD0 dispatches through
/// `product_prediction_fun_table_pd0[is_inter_mode(cand->block_mi.mode)]`
/// (product_coding_loop.c:53), and on a non-I slice the candidate set that
/// reaches it is ONE inter `NEWMV`:
///
/// * `svt_aom_sig_deriv_enc_dec_pd0` resolves `intra_level = 0` for
///   `slice_type != I_SLICE` at `pd0_level > PD0_LVL_2` and
///   `enc_mode <= M10` (enc_mode_config.c:7240-7253), so `set_intra_ctrls`
///   leaves `enable_intra = 0` and `generate_md_stage_0_cand_pd0` injects NO
///   intra candidate at all (mode_decision.c:3500).
/// * `inject_new_candidates_pd0` (mode_decision.c:2293) injects one `NEWMV`
///   per open-loop ME candidate at `me_mv * 8`, capped at three. The port's
///   low-delay-P reference set has ONE reference, so there is one.
/// * With a single candidate `md_encode_block_pd0` takes its
///   `fast_candidate_total_count == 1 && pd0_level < PD0_LVL_6` shortcut
///   (product_coding_loop.c:8390): prediction only, no MDS0 distortion, then
///   `md_stage_3_pd0` -> `full_loop_core_pd0` on it. So the ONLY thing that
///   changes versus the key-frame path is where `pred` comes from.
///
/// MEASURED with `SVT_PD0CFG_OUT` on `diag 64x64 q40 p8 frames=2`:
///
/// ```text
/// PD0CFG sb=0 org=(0,0) islice=1 lvl=4 ... intra=1/12/1 ...
/// PD0CFG sb=0 org=(0,0) islice=0 lvl=3 ... intra=0/0/0  ...
/// ```
///
/// `enable_intra/intra_mode_end/angular_pred_level` is `0/0/0` on the inter
/// frame — which is the assertion this struct's presence stands for.
#[derive(Clone, Copy)]
pub struct Pd0InterRef<'a> {
    /// The DPB reference's PADDED luma plane. C's PD0 MC does NOT clamp the
    /// MV on the unscaled path (see
    /// [`crate::inter_pred_arm::predict_inter_luma_pd0`]), so a legal MV
    /// reads the replicated margin.
    pub padded_y: &'a crate::picture::PaddedPlane,
    /// The padded DPB picture per `MvReferenceFrame` (index 1..=7) — the same
    /// table [`crate::inter_md_arm::InterMdFrame::padded_by_ref`] carries.
    ///
    /// Needed because PD0 injects EVERY surviving ME candidate
    /// (`inject_new_candidates_pd0`, mode_decision.c:2306), and a candidate's
    /// reference is its own `(direction, ref_idx)` pair — not always
    /// `LAST_FRAME`. On this port's low-delay-P envelope every populated slot
    /// aliases the same picture, so this agrees with [`Self::padded_y`]
    /// wherever it is filled.
    pub padded_by_ref: [Option<&'a crate::picture::PaddedRef>; 8],
    /// C `inject_inter_candidates_pd0`'s `allow_bipred` half that is not
    /// block-shaped: `frm_hdr->reference_mode != SINGLE_REFERENCE`
    /// (mode_decision.c:2828). The block half — `bwidth > 4 && bheight > 4` —
    /// is applied at the call.
    pub ref_mode_not_single: bool,
    /// This frame's open-loop motion search — C `pcs->ppcs->pa_me_data`.
    pub me: &'a crate::inter_me_arm::FrameMe,
    /// C `scs->super_block_size`, which sizes the driver's CONV_BUF.
    pub sb_size: usize,
    /// ALIGNED frame dims, for the (never-taken) scaled branch's edges.
    pub frame_w: usize,
    pub frame_h: usize,
    /// C `ppcs->update_type` — the rdmult BASE selector. See
    /// [`inter_full_lambda_8bit`]: PD0's `full_sb_lambda_md[EB_8_BIT_MD]` is
    /// the frame's MD lambda, so on an inter frame it is NOT the KF chain
    /// [`kf_full_lambda_8bit_lw`] that every key-frame caller passes.
    pub base_update_type: crate::port_rc_process::FrameUpdateType,
    /// C `update_lambda`'s own `gf_update_type` — the frame-type FACTOR row.
    pub factor_update_type: crate::port_rc_process::FrameUpdateType,
    /// [SVT_HDR_MODE] `static_config.alt_lambda_factors`.
    pub alt_lambda_factors: bool,
    /// C `set_blocks_to_be_tested`'s `min_sq_size` (enc_dec_process.c:1485),
    /// which `depth_removal_ctrls` decides:
    ///
    /// ```text
    /// disallow_below_64x64 ? 64
    ///   : disallow_below_32x32 ? 32
    ///   : (disallow_8x8 || disallow_below_16x16) ? 16
    ///   : disallow_4x4 ? 8 : 4          then MIN(.., max_tx_size)
    /// ```
    ///
    /// The controls are computed per SUPERBLOCK from the open-loop ME
    /// distortions, so this is a per-SB value and not a frame one. On an
    /// I-slice `set_depth_removal_level_controls` returns `enabled = 0`
    /// outright, which is why the key path never carries it.
    pub min_sq: usize,
    /// C `update_lambda`'s `me_q_index - base_q_idx` for THIS superblock
    /// (`rc_process.c:437-446`), which is what makes `full_lambda_md` and
    /// `fast_lambda_md` per-superblock. 0 reproduces the frame-level lambda.
    pub me_qdiff: i32,
    /// C `av1_lambda_assign_md`'s LAMBDA_MOD_INTRA arm (md_process.c:730-745)
    /// — 138 when `tl > 0 && ref_intra_percentage` is below its threshold
    /// under `stats_based_sb_lambda_modulation`, else the 128 identity.
    /// Frame-level; see [`inter_full_lambda_8bit`].
    pub lambda_mod_intra: i64,
}

/// C `av1_lambda_assign_md`'s per-SUPERBLOCK output on an INTER frame
/// (`svt_aom_mode_decision_configure_sb`, md_process.c:796).
///
/// One value per superblock in raster order. It exists because C's MD lambdas
/// are per-SB through `update_lambda`'s `stats_based_sb_lambda_modulation`
/// block (rc_process.c:423-446), whose `qdiff` is
/// `svt_aom_get_me_qindex(sb) - base_q_idx` — a quantity derived from
/// `me_8x8_cost_variance` alone and therefore live even with no per-SB
/// delta-q signalled.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SbInterLambda {
    /// C `ctx->full_lambda_md[EB_8_BIT_MD]` (== `full_sb_lambda_md[0]`).
    pub full_8bit: u32,
    /// C `ctx->fast_lambda_md[EB_8_BIT_MD]`.
    pub fast_8bit: u32,
    /// `me_q_index - base_q_idx` for this superblock — the ONE input the
    /// three lambda derivations (PD0, the MD funnel, the MD searches) share.
    pub me_qdiff: i32,
    /// C `ctx->full_lambda_md[EB_10_BIT_MD]` — what the encode pass hands the
    /// quantizer at `encoder_bit_depth == 10` (coding_loop.c:436/517/564).
    /// Per-SB for the same `update_lambda` reason as `full_8bit`; the bd10
    /// post-pass re-quantizes against THIS value, not the picture lambda.
    pub full_10bit: u32,
}

/// C `BlockSize` for a shape PD0 can cost — the square depths plus the two
/// fitting boundary rectangles [`Pd0Ctx::pick_q`] injects at a one-false node.
///
/// Only [`crate::port_md::predicates::get_me_block_offset`] reads it, and that
/// function keys on `MAX(bwidth, bheight)` plus the origin bits, so the
/// rectangles land on the same ME PU as their square parent — which is what C
/// does, because `svt_aom_get_me_block_offset` takes the NSQ `bsize` directly.
fn pd0_bsize(bw: usize, bh: usize) -> u8 {
    use svtav1_types::block::BlockSize as B;
    let b = match (bw, bh) {
        (4, 4) => B::Block4x4,
        (8, 8) => B::Block8x8,
        (16, 16) => B::Block16x16,
        (32, 32) => B::Block32x32,
        (64, 64) => B::Block64x64,
        (128, 128) => B::Block128x128,
        (8, 4) => B::Block8x4,
        (4, 8) => B::Block4x8,
        (16, 8) => B::Block16x8,
        (8, 16) => B::Block8x16,
        (32, 16) => B::Block32x16,
        (16, 32) => B::Block16x32,
        (64, 32) => B::Block64x32,
        (32, 64) => B::Block32x64,
        (128, 64) => B::Block128x64,
        (64, 128) => B::Block64x128,
        _ => unreachable!("PD0 block shape {bw}x{bh}"),
    };
    b as u8
}

/// C `pcs->input_frame16bit` — the packed 16-bit input picture
/// (`y_buffer` + `y_buffer_bit_inc` folded to plain `u16` samples) that
/// `svt_aom_pick_partition_pd0` selects when `SVT_EFFECTIVE_HBD_MD` is set
/// (product_coding_loop.c:10492). Ghost Robot `2c66d9ea` ("restoring the
/// full 10bit PD0 path") removes the `md_ctx->hbd_md = 0` pin around the
/// multi-pass PD loop (enc_dec_process.c), so PD0's whole per-block encode
/// — neighbour edges, DC prediction, residual, `quants_bd` quantization
/// and the `full_sb_lambda_md[EB_10_BIT_MD]` lambda — runs at 16 bits.
///
/// `None` on [`Pd0Ctx::src16`] keeps the 8-bit arm: mainline's pinned
/// surface (its `hbd_md = 0` window still stands) and every path where
/// the caller has no u16 plane.
#[derive(Clone, Copy)]
pub(crate) struct Pd0Src16<'a> {
    /// The frame luma plane at the aligned stride, u16 samples.
    pub(crate) src: &'a [u16],
    pub(crate) stride: usize,
}

/// The `hbd_md` arm's inputs for a PD0 entry point, bundled so the
/// entry signatures carry one Option instead of three.
///
/// `None` = `hbd_md` is 0 inside the PD0 window — the 8-bit arm every
/// reference but Ghost Robot keeps (and what the fork's video frames use
/// too: `pd0_use_src_samples` is `allintra`-only after `2c66d9ea`, so a
/// non-allintra picture predicts PD0 blocks from the 16-bit RECON canvas —
/// that arm is not threaded here yet).
#[derive(Clone, Copy)]
pub struct Pd0Hbd<'a> {
    /// `pcs->input_frame16bit` at the aligned stride.
    pub(crate) src16: &'a [u16],
    /// Its stride (== `in_stride` for the tile walk).
    pub(crate) stride16: usize,
    /// C `full_sb_lambda_md[EB_10_BIT_MD]` for this superblock —
    /// `av1_lambda_assign_md` at the SB's qindex (`kf_full_lambda_bd10_tuned`
    /// on a key frame; the `SbInterLambda.full_10bit` arm on an inter frame).
    pub(crate) lambda10: u64,
    /// `scs->static_config.sharpness`, folded into the `quants_bd` table
    /// `svt_av1_build_quantizer` built at sequence init.
    pub(crate) sharpness: i8,
    /// `frm_hdr.quantization_params.qm[PLANE_Y]` — the frame luma QM level;
    /// 15 = no matrix.
    pub(crate) qm_level: u8,
}

struct Pd0Ctx<'a> {
    src: &'a [u8],
    stride: usize,
    /// Ghost Robot `2c66d9ea` — set to put the WHOLE block-cost path on
    /// [`Pd0Src16`] (see [`Pd0Ctx::lvl5_like_block_cost_rect`] /
    /// [`Pd0Ctx::lvl1_block_cost_rect`]). While set, `lambda` holds
    /// `full_sb_lambda_md[EB_10_BIT_MD]` and `qm_level`/`sharpness` are the
    /// `quants_bd` inputs.
    src16: Option<Pd0Src16<'a>>,
    /// `scs->static_config.sharpness` for the `quants_bd` build — read only
    /// while `src16` is set (the 8-bit PD0 quantizer keeps `sharpness 0`,
    /// matching `build_quant_entry`).
    sharpness: i8,
    sb_x: usize,
    sb_y: usize,
    /// ALIGNED frame dims (mi-grid extent) — the spec-5.11.4 /
    /// set_blocks_to_test edge predicate is computed against these. For a
    /// 64-aligned frame every SB is complete, so `sb_x + 64 <= aligned_w`
    /// and the edge/off branches in [`Pd0Ctx::pick`] never fire.
    aligned_w: usize,
    aligned_h: usize,
    vars: SbVariance,
    qp: u32,
    qindex: u8,
    /// [SVT_HDR_MODE] Frame luma QM level (`frm_hdr.quantization_params.
    /// qm[PLANE_Y]`, from base_qindex) for the PD0 leaf quantize; 15 =
    /// identity/no matrices (mainline, and every non-bd10 fork path). Only
    /// the bd10 LVL_0 entry (`pd0_pick_sb_partition_lvl0`) sets it non-15,
    /// mirroring C's `set_pd0_ctrls` PD0_LVL_0 force at bd10 whose light
    /// encode applies QM (`svt_aom_quantize_inv_quantize_light`). Consumed by
    /// [`tx_quant_core`]; when 15 the non-QM `quantize_b` runs (byte-inert).
    qm_level: u8,
    lambda: u64,
    mode: Pd0Mode,
    lvl1: Option<&'a M6Pd0Tables>,
    max_sq: usize,
    min_sq: usize,
    /// C `ctx->is_subres_safe`: 255 = not yet determined (only a tested
    /// 64x64 block determines it); the effective per-block step is 0
    /// unless this is exactly 1.
    is_subres_safe: u8,
    /// C `ctx->subres_ctrls.step`, as `svt_aom_sig_deriv_enc_dec_pd0`
    /// resolves it PER SUPERBLOCK (`enc_mode_config.c:7327-7352`): 0 at
    /// `pd0_level <= PD0_LVL_2` or on an incomplete b64, 1 for `<=
    /// PD0_LVL_4` when the 64x64 ME cost clears `use_subres_th`, and 2 on
    /// a leaf-layer frame at PD0_LVL_5 (`:7346-7351`). The video-arm
    /// callers thread the per-SB value in; every other entry point
    /// applies [`Pd0Mode::default_subres_step`].
    subres_step: u32,
    /// C `input_resolution_factor[pcs->ppcs->input_resolution]`
    /// (perform_tx_pd0): the per-picture `factor * 1600` addend on the
    /// PD0_LVL_5 closed-form coeff rate. 0 for <= 240p pictures.
    ires_factor: u64,
    /// C `rate_est_ctrls.coeff_rate_est_lvl` at PD0 (perform_tx_pd0): 1
    /// (M2..M6) prices the real coeff rate; 2 (M7/M8, rate_est_level 4)
    /// uses the fast approximation `eob < th ? 6000 + eob*500 : real`
    /// (`th = (bw*bh)>>5`, bw/bh capped at 32). Only consulted by the
    /// LVL_1 block cost; LVL_5/6 use their own closed forms.
    coeff_rate_est_lvl: u8,
    /// Fork `get_effective_ac_bias(static_config.ac_bias, is_islice,
    /// temporal_layer_index)` — drives C's `svt_psy_adjust_rate_light`
    /// subtraction on `txb_coeff_bits`/`y_coeff_bits` inside
    /// `perform_tx_pd0` (product_coding_loop.c:4512/:4602), whichever
    /// coeff-rate arm produced the estimate. 0.0 under mainline.
    ac_bias_eff: f64,
    /// C `ctx->nsq_geom_ctrls.enabled` (svt_aom_get_nsq_geom_level_allintra,
    /// enc_mode_config.c:8240): 1 for allintra enc_mode <= M6 (presets 0..=6,
    /// nsq_geom_level 1/2/3), 0 for enc_mode > M6 (presets >= 7, level 0).
    /// Gates `set_blocks_to_test`'s one-false force-split: when NSQ is DISABLED
    /// a one-false boundary node yields `tot_shapes = 0` (force-split, no edge
    /// shape injected) — LVL_5/6 (presets >= 9) AND the LVL_1 presets 7/8. When
    /// ENABLED (presets <= 6) a fitting one-false node keeps its single edge
    /// shape (the `sq_size <= MAX(min_nsq=4, min_nsq_block_size<=8)` term never
    /// fires for edge nodes, which are always >= 16 wide on an 8-aligned frame).
    nsq_enabled: bool,
    /// C `pcs->ppcs->use_accurate_part_ctx` (`enc_mode_config.c:8955` /
    /// `:9937`): `enc_mode <= M8` on both arms. When FALSE, C doubles the
    /// SPLIT rate to bias against splitting (`test_split_partition_pd0`,
    /// product_coding_loop.c:10446). LVL_5 / LVL_0 hardcode the doubling
    /// because they only exist above M8; the LVL_1 family spans both sides of
    /// the boundary, so it reads this.
    accurate_part_ctx: bool,
    /// C `depth_early_exit_ctrls.early_exit_th` as `test_split_partition_pd0`
    /// reads it for the i > 0 quadrants (product_coding_loop.c:10469 — a
    /// stored 0 reads as 1000).
    ///
    /// `set_depth_early_exit_ctrls` (enc_mode_config.c:7182) is driven by
    /// `depth_early_exit_lvl`, which is 1 (`early_exit_th` 0 -> 1000) when
    /// `pd0_level <= PD0_LVL_1 || ctx->pic_pred_depth_only`, and 2
    /// (`early_exit_th` **900**) otherwise (`:7232`).
    /// `pic_pred_depth_only` is `depth_refinement_ctrls.mode ==
    /// PD0_DEPTH_PRED_PART_ONLY` (`:7095`) — i.e. the same predicate that
    /// makes the port take the FIXED-TREE path instead of the refinement
    /// walk — so it is a caller fact, not a level fact, and lives here
    /// rather than being derived from [`Pd0Mode`].
    depth_early_exit_th: u128,
    /// C `ctx->parent_cost_bias` (enc_mode_config.c:7280-7306): 1000
    /// everywhere EXCEPT `PD0_LVL_6` on a non-I slice, where
    /// `svt_aom_sig_deriv_enc_dec_pd0` derives it from `base_q_idx` and the
    /// SB's `me_8x8_cost_variance`, clipped to 900..=1200. It multiplies the
    /// parent cost in `test_split_partition_pd0`'s early exits AND its final
    /// split-vs-parent compare (product_coding_loop.c:10469, md_process.c's
    /// `!(islice && lvl6) ? parent_cost_bias : 1000` selection). On the
    /// allintra arm C substitutes 1000 at the compare, so the default here
    /// is 1000 — only the video arm's LVL_6 sets it off.
    parent_cost_bias: u32,
    /// Tile-row / tile-column pixel origin of this SB's tile (0 = single tile,
    /// i.e. byte-identical to the pre-fix frame-edge predicate). AV1 intra
    /// prediction never crosses a tile boundary, so a block at a tile's own
    /// top row / left column has NO above / left neighbour even when it is not
    /// the frame's own edge. The LVL_1 (M6) leaf-cost DC prediction — which
    /// drives the M6 PD0 partition decision — must honour this: otherwise it
    /// predicts across the tile boundary (from the frame-wide source), keeps a
    /// 64x64 NONE where C splits into 16x16/8x8 (C's `up_available` /
    /// `left_available` respect tiles at every preset), and codes a different
    /// tree. Only read by `lvl1_block_cost_rect`; LVL_5/LVL_6/LVL_0 leave these
    /// 0 so eff-M9 / bd10 are provably untouched.
    tile_top: usize,
    tile_left: usize,
    /// C `ctx->recon_neigh_y` while `pd0_use_src_samples` is FALSE — the
    /// VIDEO arm at every PD0 level (`allintra || hbd_md`,
    /// enc_mode_config.c:7309). `None` = the ALLINTRA behaviour this port has
    /// always had: predict every PD0 block from the SOURCE row/column, which
    /// is exactly what C's `md_encode_block_pd0` copies into the arrays when
    /// the flag is set (product_coding_loop.c:8370).
    ///
    /// Wired on the LVL_1 FAMILY only. LVL_5 / LVL_6 (CLI preset >= 9) are
    /// still source-predicted on both arms — a REAL remaining gap, recorded in
    /// `docs/INTER-ENCODE-PLAN.md`, not a claim that C differs there.
    recon_canvas: Option<Pd0ReconCanvas>,
    /// C `product_prediction_fun_table_pd0[1]` — the INTER arm of the PD0
    /// block prediction. `None` on every key frame and on the allintra arm,
    /// which is what keeps the still envelope byte-neutral by construction.
    inter: Option<&'a Pd0InterRef<'a>>,
    /// The recon of the block [`Pd0Ctx::lvl1_block_cost_rect`] just costed,
    /// held until [`Pd0Ctx::pick`] knows whether this node's partition was
    /// DECIDED (C only writes the arrays at the decision points, never for a
    /// block whose node ends up SPLIT).
    pending_recon: Option<alloc::vec::Vec<u8>>,
    /// Per-block-cost scratch, grown once per context and reused instead of
    /// the fresh `vec!`s the C port allocates per tested block. Every buffer
    /// is write-before-read within a call except `full`, whose tail beyond
    /// the packed transform region must read as zero and is `fill(0)`ed by
    /// its consumer.
    scratch: Pd0Scratch,
    /// The ROOT block's (64x64 PART_N, org `(0,0)`) MD outputs that C's
    /// `lpd1_detector_post_pd0` reads off `pc_tree->block_data[PART_N][0]`:
    /// `block_mi`'s inter flag + the two quarter-pel MVs, and `cnt_nz_coeff`
    /// (the winner's `eob`). `blk` is filled by [`Pd0Ctx::note_root_mv`] /
    /// [`Pd0Ctx::note_root_eob`]; `rd_cost` is stamped by the top-level
    /// driver and the whole thing surfaced on [`Pd0Eval::root_det`].
    root_det: Pd0RootDet,
}

impl Pd0Ctx<'_> {
    /// `lpd1_detector_post_pd0`'s PART_N block payload, recorded when the
    /// 64x64 root block is costed (org `(0,0)` in SB coords).
    fn note_root_mv(
        &mut self,
        bw: usize,
        bh: usize,
        org_x: usize,
        org_y: usize,
        is_inter: bool,
        mv0: svtav1_types::motion::Mv,
        mv1: svtav1_types::motion::Mv,
        bipred: bool,
    ) {
        if bw == 64 && bh == 64 && org_x == 0 && org_y == 0 {
            self.root_det.blk = Some(Pd0RootBlk {
                is_inter,
                mv0,
                mv1,
                bipred,
                nz: u32::MAX,
            });
        }
    }

    /// The root block's `cnt_nz_coeff` — `tx_quant_core`'s `eob`, folded into
    /// the payload [`Pd0Ctx::note_root_mv`] opened (or a fresh intra one).
    fn note_root_eob(&mut self, bw: usize, bh: usize, abs_x: usize, abs_y: usize, eob: u16) {
        if bw == 64 && bh == 64 && abs_x == self.sb_x && abs_y == self.sb_y {
            self.root_det
                .blk
                .get_or_insert(Pd0RootBlk {
                    is_inter: false,
                    mv0: svtav1_types::motion::Mv::ZERO,
                    mv1: svtav1_types::motion::Mv::ZERO,
                    bipred: false,
                    nz: u32::MAX,
                })
                .nz = u32::from(eob);
        }
    }
}

/// Reused block-cost buffers for [`Pd0Ctx::scratch`]. All start empty and
/// grow to the largest `(bw, bh)` actually costed — max 64x64.
#[derive(Default)]
struct Pd0Scratch {
    /// Intra/inter prediction buffer, `bw * bh` bytes.
    pred: alloc::vec::Vec<u8>,
    /// Second candidate buffer for `inter_best_pred`'s argmin-swap.
    cand: alloc::vec::Vec<u8>,
    /// `bw * tx_h` residuals.
    residual: alloc::vec::Vec<i16>,
    /// `bw * bh` u16 prediction buffer — the Ghost Robot 16-bit PD0 arm
    /// (`src16`) only.
    pred16: alloc::vec::Vec<u16>,
    /// `bw * tx_h` forward-transform output (pre-64-fold).
    coeffs: alloc::vec::Vec<i32>,
    /// Packed `min(bw,32) * min(tx_h,32)` quantized coefficients.
    qcoeff: alloc::vec::Vec<i32>,
    /// Packed dequantized coefficients.
    dqcoeff: alloc::vec::Vec<i32>,
    /// `bw * tx_h` unpacked dequant input to the recon inverse transform.
    full: alloc::vec::Vec<i32>,
    /// `bw * tx_h` inverse-transform output.
    inv: alloc::vec::Vec<i32>,
}

#[cfg(feature = "std")]
std::thread_local! {
    static PD0_SCRATCH: core::cell::RefCell<Pd0Scratch> =
        const { core::cell::RefCell::new(Pd0Scratch {
            pred: alloc::vec::Vec::new(), cand: alloc::vec::Vec::new(),
            residual: alloc::vec::Vec::new(), pred16: alloc::vec::Vec::new(),
            coeffs: alloc::vec::Vec::new(),
            qcoeff: alloc::vec::Vec::new(), dqcoeff: alloc::vec::Vec::new(),
            full: alloc::vec::Vec::new(), inv: alloc::vec::Vec::new(),
        }) };
}

/// This thread's [`Pd0Scratch`], taken for one superblock pick and handed
/// back by [`return_scratch`], so the buffers grow once per thread instead
/// of once per superblock. A fresh scratch under no_std.
fn take_scratch() -> Pd0Scratch {
    #[cfg(feature = "std")]
    {
        PD0_SCRATCH.with(|c| core::mem::take(&mut *c.borrow_mut()))
    }
    #[cfg(not(feature = "std"))]
    {
        Pd0Scratch::default()
    }
}

/// Returns a scratch taken by [`take_scratch`].
fn return_scratch(s: Pd0Scratch) {
    #[cfg(feature = "std")]
    PD0_SCRATCH.with(|c| *c.borrow_mut() = s);
    #[cfg(not(feature = "std"))]
    drop(s);
}

/// `v[..n]` as a mutable slice, growing the buffer once when it is too
/// small. Contents are NOT guaranteed zero — callers must write every
/// element they read (or `fill(0)` where a tail must read as zero).
fn scratch_i32(v: &mut alloc::vec::Vec<i32>, n: usize) -> &mut [i32] {
    if v.len() < n {
        v.resize(n, 0);
    }
    &mut v[..n]
}

/// [`scratch_i32`] for `u8` buffers.
fn scratch_u8(v: &mut alloc::vec::Vec<u8>, n: usize) -> &mut [u8] {
    if v.len() < n {
        v.resize(n, 0);
    }
    &mut v[..n]
}

/// [`scratch_i32`] for `i16` buffers.
fn scratch_i16(v: &mut alloc::vec::Vec<i16>, n: usize) -> &mut [i16] {
    if v.len() < n {
        v.resize(n, 0);
    }
    &mut v[..n]
}

/// [`scratch_i32`] for `u16` buffers (the 16-bit PD0 arm's prediction).
fn scratch_u16(v: &mut alloc::vec::Vec<u16>, n: usize) -> &mut [u16] {
    if v.len() < n {
        v.resize(n, 0);
    }
    &mut v[..n]
}

/// C `svt_aom_partition_rate_cost` at PD0: neighbor partition contexts are
/// 0 (never updated in PD0), `has_rows`/`has_cols` are true for the fully
/// in-picture blocks every current caller produces. Units: 1/512 bit.
fn partition_split_bits(sq_size: usize) -> u64 {
    crate::entropy::context::partition_symbol_cost(
        sq_size,
        0,
        crate::partition::PartitionType::Split as usize,
    ) as u64
}

/// Binary SPLIT-vs-{H,V} "alike" rate at a one-false boundary node, on the
/// DEFAULT partition CDF (LPD0 / PD0_LVL_5). `bottom_edge` = `!has_rows`.
fn partition_alike_split_bits(sq_size: usize, bottom_edge: bool) -> u64 {
    crate::entropy::context::partition_alike_split_symbol_cost(sq_size, bottom_edge, sq_size == 128)
        as u64
}

/// C `partition_fac_bits[0][PARTITION_NONE]`: svt_aom_full_cost_pd0 uses
/// **context index 0** — the bsl-0 (8x8 size class), sub-context-0 row —
/// as an approximation for every block size (rd_cost.c:1344-1349). 400
/// units of 1/512 bit from the default tables.
fn partition_none_bits_ctx0() -> u64 {
    crate::entropy::context::partition_symbol_cost(
        8,
        0,
        crate::partition::PartitionType::None as usize,
    ) as u64
}

/// C `skip_fac_bits[0][0]` — cost of skip=0 at context 0 from the default
/// skip CDF (icdf 1097 -> p(0) = 31671): 26 units of 1/512 bit.
fn skip0_bits() -> u64 {
    crate::entropy::context::av1_cost_symbol(32768 - 1097) as u64
}

/// `SVTAV1_ETXF=<px>,<py>` drill pin, parsed once.
#[cfg(feature = "std")]
fn etxf_pin() -> Option<(usize, usize)> {
    static PIN: std::sync::OnceLock<Option<(usize, usize)>> = std::sync::OnceLock::new();
    *PIN.get_or_init(|| {
        crate::dbgenv::raw_var("SVTAV1_ETXF").ok().and_then(|xy| {
            let mut it = xy.split(',');
            let px = it.next()?.parse().ok()?;
            let py = it.next()?.parse().ok()?;
            Some((px, py))
        })
    })
}

impl<'a> Pd0Ctx<'a> {
    /// `skip_fac_bits[0][0] + partition_fac_bits[0][PARTITION_NONE]` for the
    /// closed-form PD0 block cost (`svt_aom_full_cost_pd0`, rd_cost.c:1339).
    /// C reads the LIVE `md_rate_est_ctx`, which the rate-estimation update
    /// rewrites between frames — so where the caller chained the frame's
    /// `M6Pd0Tables` in (`lvl1 == Some`, the video arm) this uses them. The
    /// allintra LVL_5/LVL_0/LVL_6 arms carry no tables and keep the
    /// default-CDF constants, which is also what `md_rate_est_ctx` holds on
    /// frame 0 and at update_cdf_level 0.
    fn skip0_none0_bits(&self) -> (u64, u64) {
        match self.lvl1 {
            Some(t) => (t.skip0_bits, t.none_bits_ctx0),
            None => (skip0_bits(), partition_none_bits_ctx0()),
        }
    }

    /// `partition_fac_bits[ctx0(sq)][PARTITION_SPLIT]` — same chaining rule
    /// as [`Self::skip0_none0_bits`]: `svt_aom_partition_rate_cost` reads
    /// `md_rate_est_ctx`, not the static table.
    fn split_sym_bits(&self, sq_size: usize) -> u64 {
        match self.lvl1 {
            Some(t) => t.split_bits(sq_size),
            None => partition_split_bits(sq_size),
        }
    }

    /// Binary SPLIT-vs-{H,V} alike rate at a one-false boundary node — the
    /// `partition_{vert,horz}_alike_fac_bits` gather of
    /// `svt_aom_partition_rate_cost` (rd_cost.c:1846-1863), chained like
    /// [`Self::split_sym_bits`].
    fn alike_split_sym_bits(&self, sq_size: usize, bottom_edge: bool) -> u64 {
        match self.lvl1 {
            Some(t) => t.boundary_split_bits(sq_size, bottom_edge),
            None => partition_alike_split_bits(sq_size, bottom_edge),
        }
    }

    /// LVL_5 block cost (md_encode_block_pd0 full path). Also runs the
    /// per-SB subres-safety check when this is a 64x64 block and the
    /// safety is still undetermined (full_loop_core_pd0). The step is
    /// the RESOLVED `subres_ctrls.step` — 1 on the allintra/key-frame
    /// inputs those paths see, 2 on a leaf-layer inter SB.
    fn lvl5_block_cost(&mut self, sq_size: usize, org_x: usize, org_y: usize) -> u64 {
        self.lvl5_like_block_cost(sq_size, org_x, org_y, self.subres_step)
    }

    /// LVL_0 block cost (bd10-forced full-RD PD0). Same closed-form encode as
    /// [`Pd0Ctx::lvl5_block_cost`] but with subres FORCED OFF (`subres_level =
    /// 0` at `pd0_level <= PD0_LVL_2`, enc_mode_config.c:7327): step is always
    /// 0, so no 8x2/16x4 sub-sampled transform and no per-SB odd/even-deviation
    /// check. Every block runs it (no PD0-level detector).
    fn lvl0_block_cost(&mut self, sq_size: usize, org_x: usize, org_y: usize) -> u64 {
        self.lvl5_like_block_cost(sq_size, org_x, org_y, 0)
    }

    /// The 16-bit twin of the closed-form PD0 block cost — Ghost Robot
    /// `2c66d9ea` ("restoring the full 10bit PD0 path"). Under
    /// `SVT_EFFECTIVE_HBD_MD(ctx->hbd_md)`:
    ///
    /// - `svt_av1_intra_prediction`/`svt_av1_predict_intra_block` run at
    ///   `EB_TEN_BIT` on `input_frame16bit` — `copy_neighbour_arrays_pd0`
    ///   fills the 16-bit neighbour arrays from the SOURCE rows/columns
    ///   (`pd0_use_src_samples` on the allintra arm, which is the only arm
    ///   [`Pd0Hbd`] is wired to), then `av1_highbd_dc_predictor` predicts —
    ///   = `extract_neighbors_hbd` + `predict_dc_hbd`.
    /// - `svt_aom_residual_kernel` runs `svt_residual_kernel16bit` —
    ///   `(u16 src) - (u16 pred)` as wrapping i16.
    /// - `perform_tx_pd0` passes `EB_TEN_BIT` to
    ///   `svt_aom_quantize_inv_quantize_light` — `quants_bd`/`deq_bd` and
    ///   the highbd kernels ([`tx_quant_core`]'s `bit_depth == 10` arm).
    /// - `full_loop_core_pd0` prices with `full_sb_lambda_md[EB_10_BIT_MD]`
    ///   — bound on `self.lambda` by the caller.
    ///
    /// `mds_subres_step` is 0 (PD0_LVL_0 — `pd0_level <= PD0_LVL_2` forces
    /// `subres_level` 0, enc_mode_config.c:7327), so `tx_h == bh` and no
    /// subres-safety check runs; `self.lambda` is already the 10-bit value.
    fn lvl0_block_cost_hbd(
        &mut self,
        s16: Pd0Src16<'_>,
        bw: usize,
        bh: usize,
        abs_x: usize,
        abs_y: usize,
    ) -> u64 {
        // `copy_neighbour_arrays_pd0` + `svt_av1_predict_intra_block` on
        // the 16-bit picture — the source-neighbour arm (stills), so the
        // u8 `recon_canvas`/`inter` arms cannot be live.
        debug_assert!(self.inter.is_none() && self.recon_canvas.is_none());
        let (above, left, _tl, has_above, has_left) = crate::partition::extract_neighbors_hbd(
            s16.src,
            s16.stride,
            abs_x,
            abs_y,
            bw,
            bh,
            10,
            self.tile_top,
            self.tile_left,
            self.aligned_w,
            self.aligned_h,
        );
        let mut pred16 = core::mem::take(&mut self.scratch.pred16);
        svtav1_dsp::hbd::predict_dc_hbd(
            scratch_u16(&mut pred16, bw * bh),
            bw,
            &above,
            &left,
            bw,
            bh,
            has_above,
            has_left,
            10,
        );
        // `svt_aom_residual_kernel` — `is_16bit` arm (enc_dec_process.c's
        // residual dispatch under `hbd_md`).
        svtav1_dsp::pic_operators::residual_kernel_16bit(
            &s16.src[abs_y * s16.stride + abs_x..],
            s16.stride,
            &pred16[..bw * bh],
            bw,
            scratch_i16(&mut self.scratch.residual, bw * bh),
            bw,
            bw,
            bh,
        );
        self.scratch.pred16 = pred16;
        let qindex_off = (self.qindex as u32 + 8).min(255) as u8; // lpd0_qp_offset = 8
        let (eob, dist, _c_tx) = tx_quant_core(
            &mut self.scratch,
            bw,
            bh,
            qindex_off,
            self.qm_level,
            0,
            10,
            self.sharpness,
        );
        self.note_root_eob(bw, bh, abs_x, abs_y, eob);
        let mut bits = 5000 + self.ires_factor * 1600 + 100 * eob as u64;
        if self.ac_bias_eff != 0.0 {
            let pw = bw.min(32);
            let ph = bh.min(32);
            bits = svtav1_dsp::ac_bias::psy_adjust_rate_light(
                &self.scratch.dqcoeff[..pw * ph],
                bits,
                pw,
                ph,
                self.ac_bias_eff,
            );
        }
        let (skip0, none0) = self.skip0_none0_bits();
        let rate = bits + skip0 + none0;
        let cost = rdcost(self.lambda, rate, dist);
        #[cfg(feature = "std")]
        if crate::dbgenv::pd0dbg() {
            eprintln!(
                "PD0BLK org=({abs_x},{abs_y}) {bw}x{bh} dist={dist} ybits={bits} cost={cost} lambda={} subres=0 hbd=1",
                self.lambda,
            );
        }
        cost
    }

    /// Shared closed-form PD0 block cost (`full_loop_core_pd0` at
    /// `coeff_rate_est_lvl == 0`, `lpd0_qp_offset = 8`). `subres_step_cfg` is
    /// C's `subres_ctrls.step`: 1 for LVL_5 (the 64x64 odd/even-deviation
    /// check may then enable step-1 sub-sampling), 0 for LVL_0 (subres off ->
    /// no check, step stays 0 for every block).
    fn lvl5_like_block_cost(
        &mut self,
        sq_size: usize,
        org_x: usize,
        org_y: usize,
        subres_step_cfg: u32,
    ) -> u64 {
        self.lvl5_like_block_cost_rect(sq_size, sq_size, org_x, org_y, subres_step_cfg)
    }

    /// Non-square generalisation of [`Pd0Ctx::lvl5_like_block_cost`], the twin
    /// of [`Pd0Ctx::lvl1_block_cost_rect`] for the LIGHT PD0 closed form.
    ///
    /// `bw == bh` is the square PART_N path (unchanged); `bw != bh` costs the
    /// single in-frame PARTITION_HORZ / PARTITION_VERT block of a partial-SB
    /// boundary node. Every step below is dimension-general already — the DC
    /// predictor, the residual gather, `tx_quant_core` and the closed-form
    /// coeff rate — so this is a widening, not a second implementation.
    ///
    /// MEASURED on the C side before it was written (`SVT_PD0COST_OUT`,
    /// `gradient 72x88 q40 p9` video, the x = 64 superblock of a 72-wide
    /// frame): C prices `32x64`, `16x32` and `8x16` there, never the square.
    /// At `org=(64,0)` its `8x16` costs 2,905,600 against the two `8x8`s'
    /// 1,787,062 + 1,683,524 + split rate, so C keeps the rectangle — which is
    /// exactly the `BLOCK_8X16` the port was coding as `BLOCK_8X8` + a split.
    fn lvl5_like_block_cost_rect(
        &mut self,
        bw: usize,
        bh: usize,
        org_x: usize,
        org_y: usize,
        subres_step_cfg: u32,
    ) -> u64 {
        let abs_x = self.sb_x + org_x;
        let abs_y = self.sb_y + org_y;
        // Ghost Robot `2c66d9ea` — `hbd_md` effective: the whole PD0 block
        // encode runs on `input_frame16bit`, not the 8-bit MSB plane.
        if let Some(s16) = self.src16 {
            return self.lvl0_block_cost_hbd(s16, bw, bh, abs_x, abs_y);
        }
        // C `product_prediction_fun_table_pd0[is_inter_mode(mode)]`
        // (product_coding_loop.c:970): on a non-I slice PD0's ONLY candidate
        // is an inter NEWMV — `intra_ctrls.enable_intra` is 0 there — so C
        // never runs the intra neighbour extraction. The candidate loop is
        // the same `md_stage_0_pd0` argmin-variance pick the LVL_1 family
        // runs; only the cost model past `pred` differs. `pred_buf` is taken
        // out of the scratch so this function can hold it across the
        // `&mut self.scratch` calls below; it is put back before returning.
        let mut pred_buf = core::mem::take(&mut self.scratch.pred);
        if let Some(ir) = self.inter {
            let mut cand = core::mem::take(&mut self.scratch.cand);
            let (mv0, mv1, bip) =
                self.inter_best_pred_into(ir, bw, bh, abs_x, abs_y, &mut pred_buf, &mut cand);
            self.scratch.cand = cand;
            self.note_root_mv(bw, bh, org_x, org_y, true, mv0, mv1, bip);
        } else {
            // `pd0_use_src_samples` (enc_mode_config.c:7309) is `allintra ||
            // hbd_md`. TRUE — no canvas — means C copies the SOURCE row/column into
            // the recon-neighbour arrays (product_coding_loop.c:8370), so
            // predicting straight off the source plane IS that arm, and this keeps
            // the untiled extractor it has always used (byte-neutral). FALSE — a
            // canvas — means the arrays hold PD0's own recon, and the canvas is
            // that state; the availability, `n_top_px`/`n_left_px` clamp and edge
            // replication are the same function either way.
            let nb = match self.recon_canvas.as_ref() {
                None => crate::partition::extract_neighbors(
                    self.src,
                    self.stride,
                    abs_x,
                    abs_y,
                    bw,
                    bh,
                    self.aligned_w,
                    self.aligned_h,
                ),
                Some(cv) => {
                    // Row axis shifted into the canvas window, exactly as
                    // `lvl1_block_cost_rect` does it.
                    crate::partition::extract_neighbors_tiled(
                        &cv.buf,
                        cv.stride,
                        abs_x,
                        abs_y - cv.y0,
                        bw,
                        bh,
                        self.tile_top.saturating_sub(cv.y0),
                        self.tile_left,
                        self.aligned_w,
                        self.aligned_h - cv.y0,
                    )
                }
            };
            let (above, left, _tl, has_above, has_left) = nb.parts();
            let p = scratch_u8(&mut pred_buf, bw * bh);
            svtav1_dsp::intra_pred::predict_dc(p, bw, above, left, bw, bh, has_above, has_left);
        }
        let pred: &[u8] = &pred_buf;

        // Subres safety: determined once per SB by the first (and only)
        // tested 64x64 block; blocks tested while it is undetermined use
        // step 0 (C forces mds_subres_step = 0 when is_subres_safe != 1).
        // When subres is off entirely (LVL_0, subres_step_cfg == 0), the
        // check never runs and every block keeps step 0.
        if subres_step_cfg > 0 && bw == 64 && bh == 64 && self.is_subres_safe == 255 {
            self.is_subres_safe = u8::from(check_is_subres_safe(
                self.src,
                self.stride,
                abs_x,
                abs_y,
                &pred,
            ));
        }
        // subres_ctrls.step for this config; the 8-tall cap is on the SHORT
        // side (C `mds_subres_step` halves rows), so it keys on `bh`.
        let mut step = if bh >= 16 {
            subres_step_cfg
        } else {
            subres_step_cfg.min(1)
        };
        if self.is_subres_safe != 1 {
            step = 0;
        }

        let tx_h = bh >> step;
        // C `svt_residual_kernel8bit`, via the dsp kernel that already carries
        // NEON and AVX2 arms. The sub-resolution `step` is expressed as a
        // DOUBLED stride on both sides — `(r << step) * stride` is
        // `r * (stride << step)` — which is exactly what this loop did with its
        // shifted row index.
        let stride = self.stride;
        let src = self.src;
        svtav1_dsp::residual::residual_i16(
            &src[abs_y * stride + abs_x..],
            stride << step,
            pred,
            bw << step,
            bw,
            tx_h,
            scratch_i16(&mut self.scratch.residual, bw * tx_h),
        );
        let qindex_off = (self.qindex as u32 + 8).min(255) as u8; // lpd0_qp_offset = 8
        let (eob, dist, _c_tx) = tx_quant_core(
            &mut self.scratch,
            bw,
            tx_h,
            qindex_off,
            self.qm_level,
            step,
            8,
            0,
        );
        self.note_root_eob(bw, bh, abs_x, abs_y, eob);
        // coeff_rate_est_lvl == 0 closed form (perform_tx_pd0,
        // product_coding_loop.c:4579): 5000 + input_resolution_factor*1600 +
        // 100*eob. The resolution factor is a per-picture constant (0 for
        // <= 240p, e.g. all 64/128 synthetic cells; 1 at 360p incl. 512x512).
        let mut bits = 5000 + self.ires_factor * 1600 + 100 * eob as u64;
        // Ghost Robot: `svt_psy_adjust_rate_light` subtracts AC energy ×
        // `effective_ac_bias` from `txb_coeff_bits` after EVERY rate arm
        // (product_coding_loop.c:4511-4514). `recon_coeff` is the packed
        // dequantized buffer.
        if self.ac_bias_eff != 0.0 {
            let pw = bw.min(32);
            let ph = tx_h.min(32);
            bits = svtav1_dsp::ac_bias::psy_adjust_rate_light(
                &self.scratch.dqcoeff[..pw * ph],
                bits,
                pw,
                ph,
                self.ac_bias_eff,
            );
        }
        // svt_aom_full_cost_pd0: rate = coeff bits + skip(0) bits +
        // PARTITION_NONE bits at context 0 — read from the LIVE
        // `md_rate_est_ctx` (chained tables when the video arm passed them).
        let (skip0, none0) = self.skip0_none0_bits();
        let rate = bits + skip0 + none0;
        let cost = rdcost(self.lambda, rate, dist);
        // C `md_encode_block_pd0` (product_coding_loop.c:8429): with
        // `pd0_use_src_samples` FALSE, PD0 generates the block's RECON so the
        // next block can predict from it. Same inverse-transform +
        // even/odd-row expansion as the LVL_1 family's twin — see
        // `lvl1_block_cost_rect`, whose block this mirrors.
        // C `md_encode_block_pd0` (product_coding_loop.c:8430) generates the
        // recon only `if (!ctx->skip_intra && !ctx->pd0_use_src_samples)`. On
        // an INTER frame `intra_level == 0` makes `skip_intra` 1
        // (enc_mode_config.c:6718), so C writes NOTHING into the PD0 recon
        // neighbour arrays — which is consistent, because with no intra
        // candidate nothing reads them.
        if self.recon_canvas.is_some() && self.inter.is_none() {
            let mut recon = alloc::vec![0u8; bw * bh];
            if eob > 0 {
                let n = bw * tx_h;
                let packed_w = bw.min(32);
                let packed_h = tx_h.min(32);
                // The inverse transform reads the whole `bw * tx_h` input;
                // the scatter only fills the packed region, so the tail of
                // `full` must be zeroed on every call.
                let full = scratch_i32(&mut self.scratch.full, n);
                full.fill(0);
                let dqcoeff = &self.scratch.dqcoeff[..packed_w * packed_h];
                for r in 0..packed_h {
                    for c in 0..packed_w {
                        full[r * bw + c] = dqcoeff[r * packed_w + c];
                    }
                }
                let (tx_size, _) = pd0_tx_size(bw, tx_h);
                let inv = scratch_i32(&mut self.scratch.inv, n);
                svtav1_dsp::txfm_dispatch::inv_txfm2d_dispatch(
                    full,
                    inv,
                    bw,
                    tx_size,
                    svtav1_types::transform::TxType::DctDct,
                );
                for r in 0..tx_h {
                    let dst = (r << step) * bw;
                    for c in 0..bw {
                        recon[dst + c] =
                            (i32::from(pred[dst + c]) + inv[r * bw + c]).clamp(0, 255) as u8;
                    }
                    if step > 0 && (r << step) + 1 < bh {
                        let (a, b) = recon.split_at_mut(dst + bw);
                        b[..bw].copy_from_slice(&a[dst..dst + bw]);
                    }
                }
            } else {
                recon.copy_from_slice(&pred[..bw * bh]);
            }
            self.pending_recon = Some(recon);
        }
        // `SVTAV1_PD0DBG` on the LIGHT PD0 path too. The LVL_1 family has had
        // this dump since task #95; LVL_5 had none, so the video arm's PD0 at
        // preset >= 9 — the one the fixed-tree path runs — could not be joined
        // against C's `SVT_PD0COST_OUT` (`svt_aom_full_cost_pd0`) block for
        // block. Same first four fields in the same order as that dump, so the
        // two files line up without a translation step.
        #[cfg(feature = "std")]
        if crate::dbgenv::pd0dbg() {
            eprintln!(
                "PD0BLK org=({abs_x},{abs_y}) {bw}x{bh} dist={dist} ybits={bits} cost={cost} lambda={} subres={step}",
                self.lambda,
            );
        }
        self.scratch.pred = pred_buf;
        cost
    }

    /// PD0_LVL_1 block cost (md_encode_block_pd0 at allintra M2..M8):
    /// same DC-from-source prediction, but `lpd0_qp_offset = 0`, subres
    /// permanently off (`pd0_level <= PD0_LVL_2` -> subres_level 0), and
    /// the REAL coefficient rate (`coeff_rate_est_lvl = 1` ->
    /// svt_aom_txb_estimate_coeff_bits_pd0 with zero contexts).
    fn lvl1_block_cost(&mut self, sq_size: usize, org_x: usize, org_y: usize) -> u64 {
        self.lvl1_block_cost_rect(sq_size, sq_size, org_x, org_y)
    }

    /// Non-square generalisation of the PD0_LVL_1 block cost. `bw == bh` is
    /// the square PART_N path (unchanged); `bw != bh` costs the single
    /// in-frame PARTITION_HORZ / PARTITION_VERT block of a partial-SB boundary
    /// node (task #95 chunk 2) — C's LPD0 "single block per shape ... PART_H/
    /// PART_V for boundary blocks" (product_coding_loop.c:127). The DC
    /// predictor, residual, `tx_quant_core` (Tx32x16 / Tx16x8 / …) and PD0
    /// coeff-rate estimator are all dimension-general.
    fn lvl1_block_cost_rect(&mut self, bw: usize, bh: usize, org_x: usize, org_y: usize) -> u64 {
        let abs_x = self.sb_x + org_x;
        let abs_y = self.sb_y + org_y;
        // C `md_encode_block_pd0` (product_coding_loop.c:8370): with
        // `pd0_use_src_samples` the SOURCE row/column is copied into the recon
        // neighbour arrays, so predicting straight off the source plane IS the
        // allintra arm. Without it the arrays hold PD0's own recon, and the
        // canvas is that state. The availability, `n_top_px`/`n_left_px` clamp
        // and edge replication are the SAME function either way.
        // C `product_prediction_fun_table_pd0[is_inter_mode(mode)]`
        // (product_coding_loop.c:970). On a non-I slice PD0's ONLY candidate is
        // an inter NEWMV (see [`Pd0InterRef`]), so the INTRA neighbour
        // extraction below is not merely unused there — C never runs it,
        // because `pd0_use_src_samples` is false and `skip_intra` is 1.
        if let Some(ir) = self.inter {
            return self.lvl1_block_cost_inter(ir, bw, bh, abs_x, abs_y);
        }
        // Ghost Robot `2c66d9ea` — `hbd_md` effective: 16-bit source
        // neighbours + u16 DC prediction + `svt_residual_kernel16bit` + the
        // `quants_bd` quantize, priced at `full_sb_lambda_md[EB_10_BIT_MD]`
        // (`self.lambda`). On the allintra arm this serves the LVL_0-forced
        // refinement eval too — `svt_aom_sig_deriv_enc_dec_pd0` resolves
        // `rate_est_level` 2/4 there (`lpd0_qp_offset` 0 + real coeff rate),
        // which is exactly this block cost's shape.
        if let Some(s16) = self.src16 {
            let (above, left, _tl, has_above, has_left) = crate::partition::extract_neighbors_hbd(
                s16.src,
                s16.stride,
                abs_x,
                abs_y,
                bw,
                bh,
                10,
                self.tile_top,
                self.tile_left,
                self.aligned_w,
                self.aligned_h,
            );
            let mut pred16 = core::mem::take(&mut self.scratch.pred16);
            svtav1_dsp::hbd::predict_dc_hbd(
                scratch_u16(&mut pred16, bw * bh),
                bw,
                &above,
                &left,
                bw,
                bh,
                has_above,
                has_left,
                10,
            );
            let cost = self.lvl1_cost_from_pred_hbd(s16, bw, bh, abs_x, abs_y, &pred16);
            self.scratch.pred16 = pred16;
            return cost;
        }
        let nb = match self.recon_canvas.as_ref() {
            None => crate::partition::extract_neighbors_tiled(
                self.src,
                self.stride,
                abs_x,
                abs_y,
                bw,
                bh,
                self.tile_top,
                self.tile_left,
                self.aligned_w,
                self.aligned_h,
            ),
            Some(cv) => {
                // Shift the row axis into the canvas's window. `tile_top` and
                // `aligned_h` shift with it, so `abs_y > tile_top` and
                // `aligned_h - abs_y` are unchanged; the column axis is not
                // windowed at all.
                crate::partition::extract_neighbors_tiled(
                    &cv.buf,
                    cv.stride,
                    abs_x,
                    abs_y - cv.y0,
                    bw,
                    bh,
                    self.tile_top.saturating_sub(cv.y0),
                    self.tile_left,
                    self.aligned_w,
                    self.aligned_h - cv.y0,
                )
            }
        };
        let (above, left, _tl, has_above, has_left) = nb.parts();
        // `pred` is taken out of the scratch so it can be borrowed across the
        // `&mut self` call; it is put back afterwards.
        let mut pred = core::mem::take(&mut self.scratch.pred);
        svtav1_dsp::intra_pred::predict_dc(
            scratch_u8(&mut pred, bw * bh),
            bw,
            above,
            left,
            bw,
            bh,
            has_above,
            has_left,
        );
        let cost = self.lvl1_cost_from_pred(
            bw,
            bh,
            abs_x,
            abs_y,
            &pred,
            Some((&above, &left, has_above, has_left)),
        );
        self.scratch.pred = pred;
        cost
    }

    /// [`Pd0Ctx::lvl1_block_cost_rect`] from the point the PREDICTION exists —
    /// C `full_loop_core_pd0` (product_coding_loop.c:5963) plus the recon
    /// generation `md_encode_block_pd0` runs after it.
    ///
    /// Split out because the two arms of
    /// `product_prediction_fun_table_pd0` differ ONLY in how `pred` is
    /// produced; everything from the residual down is one C function that both
    /// reach. `nb` is the intra arm's neighbour state, carried only so the
    /// `SVTAV1_PD0DBG` line keeps printing it; `None` is the inter arm.
    #[allow(clippy::too_many_arguments)]
    fn lvl1_cost_from_pred(
        &mut self,
        bw: usize,
        bh: usize,
        abs_x: usize,
        abs_y: usize,
        pred: &[u8],
        nb: Option<(&[u8], &[u8], bool, bool)>,
    ) -> u64 {
        // `subres_ctrls.step`, and the per-SB safety check that gates it —
        // identical machinery to [`Pd0Ctx::lvl5_like_block_cost`], because it
        // is the same `full_loop_core_pd0` code. LVL_1 / LVL_0 configure step
        // 0 (`pd0_level <= PD0_LVL_2`, enc_mode_config.c:7337) so nothing here
        // runs for them and the pre-existing allintra paths are unchanged by
        // construction; LVL_3 / LVL_4 configure step 1.
        let subres_step_cfg = self.subres_step_cfg();
        if subres_step_cfg > 0 && bw == 64 && bh == 64 && self.is_subres_safe == 255 {
            self.is_subres_safe = u8::from(check_is_subres_safe(
                self.src,
                self.stride,
                abs_x,
                abs_y,
                pred,
            ));
        }
        let mut step = if bh >= 16 {
            subres_step_cfg
        } else {
            subres_step_cfg.min(1)
        };
        if self.is_subres_safe != 1 {
            step = 0;
        }

        let tx_h = bh >> step;
        // C `svt_residual_kernel8bit`, via the dsp kernel that already carries
        // NEON and AVX2 arms. The sub-resolution `step` is expressed as a
        // DOUBLED stride on both sides — `(r << step) * stride` is
        // `r * (stride << step)` — which is exactly what this loop did with its
        // shifted row index.
        let stride = self.stride;
        let src = self.src;
        svtav1_dsp::residual::residual_i16(
            &src[abs_y * stride + abs_x..],
            stride << step,
            pred,
            bw << step,
            bw,
            tx_h,
            scratch_i16(&mut self.scratch.residual, bw * tx_h),
        );
        let (eob, dist, c_tx) = tx_quant_core(
            &mut self.scratch,
            bw,
            tx_h,
            self.qindex,
            self.qm_level,
            step,
            8,
            0,
        );
        // TEMPORARY drill: residual + coeff dump pinned to one block.
        // `SVTAV1_ETXF=<px>,<py>` parsed once — a per-block env lookup would
        // sit inside the PD0 eval's hottest loop.
        #[cfg(feature = "std")]
        if let Some((px, py)) = etxf_pin()
            && abs_x == px
            && abs_y == py
            && bw == 8
            && tx_h == 8
        {
            eprint!("RSRES org=({abs_x},{abs_y}) res=[");
            for i in 0..(bw * tx_h).min(32) {
                eprint!("{},", self.scratch.residual[i]);
            }
            eprintln!("]");
            eprint!("RSCO org=({abs_x},{abs_y}) eob={eob} dist={dist} co=[");
            for i in 0..(bw * tx_h).min(24) {
                eprint!("{},", self.scratch.coeffs[i]);
            }
            eprintln!("]");
            eprint!("RSDQ dq=[");
            for i in 0..(bw * tx_h).min(24) {
                eprint!("{},", self.scratch.dqcoeff[i]);
            }
            eprintln!("]");
        }
        self.note_root_eob(bw, bh, abs_x, abs_y, eob);
        let tables = self.lvl1.expect("LVL_1 requires tables");
        // C `perform_tx_pd0` luma coeff rate (single-txb, product_coding_
        // loop.c:4501-4508): `th = (bwidth*bheight)>>5` where `bwidth =
        // txbwidth < 64 ? txbwidth : 32` and likewise for the height —
        // `.min(32)` is the same map on every power-of-two size <= 64.
        // coeff_rate_est_lvl 2 prices `eob < th ? 6000 + eob*500 : real`; the
        // eob==0 -> 6000 case folds into `eob < th`. Level 1 keeps the real
        // cost / skip cost.
        //
        // The HEIGHT here is the TRANSFORM's, not the block's: at
        // `mds_subres_step == 1` C rewrites `tx_size` TX_NxN -> TX_NxN/2
        // (`:4332-4344`) before `txbheight` is read, so an 8x8 block under
        // subres has `th = (8*4)>>5 = 1` and NOT 2. MEASURED on
        // `screenrep 72x88 q40 p8` video against C's `SVT_PD0COST_OUT`: with
        // `bh` the port priced every 8x8 at the 6500 shortcut where C priced
        // the real rate (~31528), 83 of 130 PD0 block costs differing. With
        // `tx_h` all 130 agree.
        //
        // It could not matter before PD0_LVL_4 was wired: `th` is read only
        // when `coeff_rate_est_lvl >= 2`, and the only levels that set that
        // are the allintra M7/M8 rows — which are PD0_LVL_1, subres step 0,
        // where `tx_h == bh`.
        let cw = bw.min(32);
        let ch = tx_h.min(32);
        let th = (cw * ch) >> 5;
        let mut bits = if self.coeff_rate_est_lvl >= 2 && (eob as usize) < th {
            6000 + eob as u64 * 500
        } else if eob == 0 {
            cost_skip_txb_pd0(c_tx, &tables.coeff) as u64
        } else {
            // `is_inter` selects the rate table — C's
            // `av1_transform_type_rate_estimation` reads
            // `inter_tx_type_fac_bits` on the inter arm, not the intra@DC
            // rows the intra arm uses.
            let tx_rates = if self.inter.is_some() {
                Pd0TxRates::Inter(&tables.tx_rates_inter)
            } else {
                Pd0TxRates::Intra(&tables.tx_rates)
            };
            cost_coeffs_txb_pd0(
                &self.scratch.qcoeff[..cw * ch],
                eob,
                c_tx,
                &tables.coeff,
                tx_rates,
                step,
            ) as u64
        };
        // Ghost Robot: `svt_psy_adjust_rate_light` on `txb_coeff_bits`
        // after whichever rate arm produced it (perform_tx_pd0,
        // product_coding_loop.c:4511-4514); `recon_coeff` = the packed
        // dequantized tx block.
        if self.ac_bias_eff != 0.0 {
            bits = svtav1_dsp::ac_bias::psy_adjust_rate_light(
                &self.scratch.dqcoeff[..cw * ch],
                bits,
                cw,
                ch,
                self.ac_bias_eff,
            );
        }
        let rate = bits + tables.skip0_bits + tables.none_bits_ctx0;
        let cost = rdcost(self.lambda, rate, dist);
        // C `md_encode_block_pd0` (product_coding_loop.c:8429): on the VIDEO
        // arm PD0 generates the block's RECON so the next block can predict
        // from it. `av1_perform_inverse_transform_recon` (:752) inverts the
        // SUB-SAMPLED transform into the recon's EVEN rows at a doubled stride
        // and then copies each even row down onto the odd row below it (:859);
        // with no coefficients it is a straight `svt_av1_picture_copy_y` of
        // the prediction (:873).
        if self.recon_canvas.is_some() {
            let mut recon = alloc::vec![0u8; bw * bh];
            if eob > 0 {
                let n = bw * tx_h;
                let packed_w = bw.min(32);
                let packed_h = tx_h.min(32);
                // The inverse transform reads the whole `bw * tx_h` input;
                // the scatter only fills the packed region, so the tail of
                // `full` must be zeroed on every call.
                let full = scratch_i32(&mut self.scratch.full, n);
                full.fill(0);
                let dqcoeff = &self.scratch.dqcoeff[..packed_w * packed_h];
                for r in 0..packed_h {
                    for c in 0..packed_w {
                        full[r * bw + c] = dqcoeff[r * packed_w + c];
                    }
                }
                let (tx_size, _) = pd0_tx_size(bw, tx_h);
                let inv = scratch_i32(&mut self.scratch.inv, n);
                svtav1_dsp::txfm_dispatch::inv_txfm2d_dispatch(
                    full,
                    inv,
                    bw,
                    tx_size,
                    svtav1_types::transform::TxType::DctDct,
                );
                for r in 0..tx_h {
                    let dst = (r << step) * bw;
                    for c in 0..bw {
                        recon[dst + c] =
                            (i32::from(pred[dst + c]) + inv[r * bw + c]).clamp(0, 255) as u8;
                    }
                    if step > 0 && (r << step) + 1 < bh {
                        let (a, b) = recon.split_at_mut(dst + bw);
                        b[..bw].copy_from_slice(&a[dst..dst + bw]);
                    }
                }
            } else {
                recon.copy_from_slice(&pred[..bw * bh]);
            }
            self.pending_recon = Some(recon);
        }
        // `SVTAV1_PD0DBG`: the port-side twin of the C `SVT_PD0COST_OUT`
        // interposer on `svt_aom_full_cost_pd0`. Same fields, same order, so
        // the two dumps join block-for-block without a translation step.
        #[cfg(feature = "std")]
        if crate::dbgenv::pd0dbg() {
            eprintln!(
                "PD0BLK org=({},{}) {}x{} dist={} ybits={} cost={} lambda={} eob={} qidx={} subres={} dc={} ha={} hl={} a0={:?} l0={:?}",
                abs_x,
                abs_y,
                bw,
                bh,
                dist,
                bits,
                cost,
                self.lambda,
                eob,
                self.qindex,
                step,
                pred[0],
                u8::from(nb.is_some_and(|n| n.2)),
                u8::from(nb.is_some_and(|n| n.3)),
                nb.map_or(&[][..], |n| &n.0[..n.0.len().min(4)]),
                nb.map_or(&[][..], |n| &n.1[..n.1.len().min(4)])
            );
        }
        cost
    }

    /// [`Pd0Ctx::lvl1_cost_from_pred`]'s 16-bit twin — Ghost Robot
    /// `2c66d9ea` (`hbd_md` effective): `svt_residual_kernel16bit` diffs the
    /// u16 prediction against `input_frame16bit`, `perform_tx_pd0` at
    /// `EB_TEN_BIT` quantizes with `quants_bd` + the highbd kernels, and
    /// `svt_aom_full_cost_pd0` prices at `full_sb_lambda_md[EB_10_BIT_MD]`
    /// (`self.lambda`). The coeff-rate arms and the psy adjustment are
    /// depth-independent — identical to the 8-bit twin.
    ///
    /// `mds_subres_step` is 0 on every arm `src16` serves (the `hbd_md`
    /// force resolves PD0_LVL_0; `pd0_level <= PD0_LVL_2` forces
    /// `subres_level` 0 — enc_mode_config.c:7327), so `tx_h == bh` and no
    /// subres-safety check runs. The u8 `recon_canvas` arm is skipped for
    /// the same reason [`Pd0Hbd`] documents: `pd0_use_src_samples` stays
    /// true on the allintra arm.
    fn lvl1_cost_from_pred_hbd(
        &mut self,
        s16: Pd0Src16<'_>,
        bw: usize,
        bh: usize,
        abs_x: usize,
        abs_y: usize,
        pred16: &[u16],
    ) -> u64 {
        debug_assert_eq!(
            self.subres_step_cfg(),
            0,
            "hbd PD0 is PD0_LVL_0 — subres off"
        );
        svtav1_dsp::pic_operators::residual_kernel_16bit(
            &s16.src[abs_y * s16.stride + abs_x..],
            s16.stride,
            pred16,
            bw,
            scratch_i16(&mut self.scratch.residual, bw * bh),
            bw,
            bw,
            bh,
        );
        let (eob, dist, c_tx) = tx_quant_core(
            &mut self.scratch,
            bw,
            bh,
            self.qindex,
            self.qm_level,
            0,
            10,
            self.sharpness,
        );
        self.note_root_eob(bw, bh, abs_x, abs_y, eob);
        let tables = self.lvl1.expect("LVL_1 requires tables");
        // The same `perform_tx_pd0` rate arms the u8 twin runs —
        // `coeff_rate_est_lvl >= 2` shortcut, `eob == 0` skip cost, else the
        // real coefficient rate.
        let cw = bw.min(32);
        let ch = bh.min(32);
        let th = (cw * ch) >> 5;
        let mut bits = if self.coeff_rate_est_lvl >= 2 && (eob as usize) < th {
            6000 + eob as u64 * 500
        } else if eob == 0 {
            cost_skip_txb_pd0(c_tx, &tables.coeff) as u64
        } else {
            cost_coeffs_txb_pd0(
                &self.scratch.qcoeff[..cw * ch],
                eob,
                c_tx,
                &tables.coeff,
                Pd0TxRates::Intra(&tables.tx_rates),
                0,
            ) as u64
        };
        // `svt_psy_adjust_rate_light` on `txb_coeff_bits`
        // (product_coding_loop.c:4511-4514) — `recon_coeff` is the packed
        // dequantized tx block.
        if self.ac_bias_eff != 0.0 {
            bits = svtav1_dsp::ac_bias::psy_adjust_rate_light(
                &self.scratch.dqcoeff[..cw * ch],
                bits,
                cw,
                ch,
                self.ac_bias_eff,
            );
        }
        let rate = bits + tables.skip0_bits + tables.none_bits_ctx0;
        let cost = rdcost(self.lambda, rate, dist);
        #[cfg(feature = "std")]
        if crate::dbgenv::pd0dbg() {
            eprintln!(
                "PD0BLK org=({abs_x},{abs_y}) {bw}x{bh} dist={dist} ybits={bits} cost={cost} lambda={} eob={eob} qidx={} subres=0 hbd=1",
                self.lambda, self.qindex,
            );
        }
        cost
    }

    /// `md_stage_0_pd0`'s candidate selection — `inject_new_candidates_pd0`
    /// (mode_decision.c:2293) injects EVERY surviving ME candidate for the
    /// block's ME PU, `md_stage_0_pd0` (product_coding_loop.c:1507) picks the
    /// argmin-VARIANCE one (`fast_cost` = `svt_aom_mefn_ptr[bsize].vf`, the
    /// two-buffer difference variance), and `md_stage_3_pd0` runs the real
    /// residual/TX/coeff cost on that winner alone. Evaluating candidate 0
    /// only — this arm's earlier form — mispriced any PU whose second or
    /// third candidate won (MEASURED on `diag 72x72 q20 p6` frame 3: C's
    /// `(64,16)` 8x16 winner is `ref_idx_l0 = 1` at dist 20 where candidate
    /// 0 reads dist 2301).
    ///
    /// The `cand_total_cnt > 2` break in C caps the INJECTED count at three —
    /// bipred candidates skipped by `allow_bipred` do not count.
    ///
    /// Shared by every inter PD0 level that runs `md_encode_block_pd0` (the
    /// LVL_1 family AND LVL_5): the candidate set and the pick are level-
    /// independent; the level decides only the cost model downstream.
    /// [`Pd0Ctx::inter_best_pred`] with caller-owned buffers: `best` receives
    /// the winning `bw * bh` prediction (or the zero-MV fallback) and `cand`
    /// is the per-candidate working buffer. A win SWAPS the two — the loser's
    /// contents are overwritten before they are ever read.
    /// Returns the winning candidate's quarter-pel `(mv0, mv1, is_bipred)` —
    /// `mv1` is `Mv::ZERO` and `is_bipred` false for a unipred winner.
    /// `lpd1_detector_post_pd0`'s `block_mi.mv[0/1]`/`has_second_ref` tests
    /// read these; [`Pd0Ctx::lvl1_block_cost_inter`] stores them on the ROOT
    /// block only.
    fn inter_best_pred_into(
        &self,
        ir: &Pd0InterRef<'_>,
        bw: usize,
        bh: usize,
        abs_x: usize,
        abs_y: usize,
        best: &mut Vec<u8>,
        cand: &mut Vec<u8>,
    ) -> (svtav1_types::motion::Mv, svtav1_types::motion::Mv, bool) {
        let bsize = pd0_bsize(bw, bh);
        let cands = ir.me.cands_for(abs_x, abs_y, bsize);
        // C `inject_inter_candidates_pd0` (mode_decision.c:2828): compound is
        // out when the frame is single-reference or either dim is 4.
        let allow_bipred = ir.ref_mode_not_single && bw > 4 && bh > 4;
        let src = &self.src[abs_y * self.stride + abs_x..];
        let n = bw * bh;
        let mut best_var = u64::MAX;
        let mut have_best = false;
        let mut best_mv0 = svtav1_types::motion::Mv::ZERO;
        let mut best_mv1 = svtav1_types::motion::Mv::ZERO;
        let mut best_bipred = false;
        let mut injected = 0u32;
        for (i, c) in cands.iter().enumerate() {
            let dir = c.direction();
            let mv_dbg;
            // `cmv0` is assigned on every path that reaches the cost compare
            // (both non-`continue` arms), so it needs no initializer.
            let cmv0;
            let mut cmv1 = svtav1_types::motion::Mv::ZERO;
            let cbip = dir >= crate::inter_me::context::BI_PRED;
            if dir < crate::inter_me::context::BI_PRED {
                let Some((_d, mv_fp)) = ir.me.cand_mv_for(abs_x, abs_y, bsize, i) else {
                    continue;
                };
                let ref_idx = if dir == 1 {
                    c.ref_idx_l1()
                } else {
                    c.ref_idx_l0()
                };
                let rf = crate::port_picstruct::get_ref_frame_type(dir, ref_idx);
                mv_dbg = (mv_fp.y * 8, mv_fp.x * 8, rf);
                cmv0 = svtav1_types::motion::Mv {
                    x: mv_fp.x.saturating_mul(8),
                    y: mv_fp.y.saturating_mul(8),
                };
                self.inter_pred_into(ir, bw, bh, abs_x, abs_y, mv_fp, rf, scratch_u8(cand, n));
            } else if allow_bipred {
                let Some(((mv0, rf0), (mv1, rf1))) = ir.me.cand_bipred_mvs(abs_x, abs_y, bsize, i)
                else {
                    continue;
                };
                mv_dbg = (mv0.y * 8, mv0.x * 8, rf0);
                cmv0 = svtav1_types::motion::Mv {
                    x: mv0.x.saturating_mul(8),
                    y: mv0.y.saturating_mul(8),
                };
                cmv1 = svtav1_types::motion::Mv {
                    x: mv1.x.saturating_mul(8),
                    y: mv1.y.saturating_mul(8),
                };
                self.inter_pred_into_bipred(
                    ir,
                    bw,
                    bh,
                    abs_x,
                    abs_y,
                    mv0,
                    rf0,
                    mv1,
                    rf1,
                    scratch_u8(cand, n),
                );
            } else {
                continue;
            }
            let var = u64::from(svtav1_dsp::variance::variance_diff(
                cand,
                bw,
                src,
                self.stride,
                bw,
                bh,
            ));
            #[cfg(feature = "std")]
            if crate::dbgenv::pd0dbg() {
                eprintln!(
                    "PD0CAND org=({abs_x},{abs_y}) {bw}x{bh} cand={i} dir={dir} var={var} mv={},{} ref={}",
                    mv_dbg.0, mv_dbg.1, mv_dbg.2
                );
            }
            if var < best_var {
                best_var = var;
                core::mem::swap(best, cand);
                best_mv0 = cmv0;
                best_mv1 = cmv1;
                best_bipred = cbip;
                have_best = true;
            }
            injected += 1;
            if injected > 2 {
                break;
            }
        }
        #[cfg(feature = "std")]
        if crate::dbgenv::pd0dbg() && crate::dbgenv::pd0pred() {
            if have_best {
                eprint!("PD0PRED org=({abs_x},{abs_y}) {bw}x{bh}");
                for r in 0..bh.min(4) {
                    eprint!(" r{r}=");
                    for c in 0..bw.min(16) {
                        eprint!("{},", best[r * bw + c]);
                    }
                }
                eprintln!();
            }
        }
        // `inject_zz_backup_candidate` (mode_decision.c:3314): zero-MV NEWMV
        // on LAST when the PU's candidate list is empty.
        if !have_best {
            self.inter_pred_into(
                ir,
                bw,
                bh,
                abs_x,
                abs_y,
                svtav1_types::motion::Mv::ZERO,
                1, // LAST_FRAME
                scratch_u8(best, n),
            );
        }
        (best_mv0, best_mv1, best_bipred)
    }

    /// C `compute_lpd0_cost_inter` (product_coding_loop.c:8267) — the
    /// PD0_LVL_6 block cost on a NON-KEY frame. For each of the block's
    /// surviving PA-ME candidates (BI_PRED skipped, the first THREE
    /// evaluated — `if (++cand_count > 2) break`), clip the full-pel ME MV
    /// against the candidate's own reference and take the prediction
    /// VARIANCE (`svt_aom_mefn_ptr[bsize].vf`); the cheapest goes through
    /// `compute_lpd0_cost_from_variance` (:8247):
    ///
    /// ```text
    /// dist = MIN(variance / area, lambda >> 10) * area
    /// cost = RDCOST(full_sb_lambda_md[8bit],
    ///               partition_fac_bits[0][PARTITION_NONE], dist)
    /// ```
    ///
    /// Unlike the LVL_1/LVL_5 inter arm there is no recon to carry —
    /// `md_encode_block_pd0` returns after writing only `blk_ptr->cost`, so
    /// `pending_recon` stays empty and the neighbour-array write is a
    /// dead canvas copy either way.
    fn lvl6_block_cost_inter(&self, bw: usize, bh: usize, org_x: usize, org_y: usize) -> u64 {
        let ir = self
            .inter
            .expect("lvl6_block_cost_inter is the `inter.is_some()` arm");
        let abs_x = self.sb_x + org_x;
        let abs_y = self.sb_y + org_y;
        let bsize = pd0_bsize(bw, bh);
        let src = &self.src[abs_y * self.stride + abs_x..];
        let mut best: Option<u32> = None;
        let mut evaluated = 0u32;
        for (i, c) in ir.me.cands_for(abs_x, abs_y, bsize).iter().enumerate() {
            let dir = c.direction();
            // `if (direction == BI_PRED) continue;` — compound candidates are
            // never costed here regardless of `ref_mode_not_single`.
            if dir >= crate::inter_me::context::BI_PRED {
                continue;
            }
            let Some((_d, mv_fp)) = ir.me.cand_mv_for(abs_x, abs_y, bsize, i) else {
                continue;
            };
            let ref_idx = if dir == 1 {
                c.ref_idx_l1()
            } else {
                c.ref_idx_l0()
            };
            let plane =
                Self::inter_ref_plane(ir, crate::port_picstruct::get_ref_frame_type(dir, ref_idx));
            let mut mv = svtav1_types::motion::Mv {
                x: mv_fp.x.saturating_mul(8),
                y: mv_fp.y.saturating_mul(8),
            };
            crate::port_md::coding_loop::clip_mv_on_pic_boundary(
                abs_x as i32,
                abs_y as i32,
                bw as i32,
                bh as i32,
                plane.width as i32,
                plane.height as i32,
                plane.border as i32,
                &mut mv.x,
                &mut mv.y,
            );
            let var = Self::lvl6_ref_variance(plane, abs_x, abs_y, bw, bh, mv, src, self.stride);
            best = Some(best.map_or(var, |b: u32| b.min(var)));
            evaluated += 1;
            if evaluated > 2 {
                break;
            }
        }
        // `best_cost == (uint64_t)~0` — no unipred candidate survived: the
        // `inject_zz_backup_candidate` twin, a zero-MV read on LAST.
        let var = best.unwrap_or_else(|| {
            Self::lvl6_ref_variance(
                ir.padded_y,
                abs_x,
                abs_y,
                bw,
                bh,
                svtav1_types::motion::Mv::ZERO,
                src,
                self.stride,
            )
        });
        // `compute_lpd0_cost_from_variance`. `partition_fac_bits[0]
        // [PARTITION_NONE]` is the LIVE `md_rate_est_ctx` value — the chained
        // tables the video arm always carries (`lvl1` is `Some` here).
        let (_, none0) = self.skip0_none0_bits();
        let area = (bw * bh) as u32;
        let noise = u32::try_from(self.lambda >> 10).unwrap_or(u32::MAX);
        let var_pp = var / area;
        let dist = u64::from(var_pp.min(noise)) * u64::from(area);
        rdcost(self.lambda, none0, dist)
    }

    /// The `fn_ptr->vf` half of `compute_lpd0_cost_inter` — the `bwidth x
    /// bheight` window of the reference at the clipped eighth-pel MV (always
    /// pixel-aligned), vs the source block. C reads
    /// `ref_pic->y_buffer + ref_origin_index`, whose negative or past-extent
    /// offsets land on the replicated margin — the [`crate::picture::PaddedPlane`]
    /// border is what `clip_mv_on_pic_boundary` clips the MV into, so the
    /// slice stays in bounds.
    #[allow(clippy::too_many_arguments)]
    fn lvl6_ref_variance(
        plane: &crate::picture::PaddedPlane,
        abs_x: usize,
        abs_y: usize,
        bw: usize,
        bh: usize,
        mv: svtav1_types::motion::Mv,
        src: &[u8],
        src_stride: usize,
    ) -> u32 {
        let rx = abs_x as isize + isize::from(mv.x >> 3);
        let ry = abs_y as isize + isize::from(mv.y >> 3);
        let off = (plane.origin as isize + ry * plane.stride as isize + rx) as usize;
        svtav1_dsp::variance::variance_diff(
            &plane.buf[off..],
            plane.stride,
            src,
            src_stride,
            bw,
            bh,
        )
    }

    /// The inter arm of `md_encode_block_pd0` for the LVL_1 family — the
    /// [`Pd0Ctx::inter_best_pred`] winner fed through the LVL_1 cost model.
    fn lvl1_block_cost_inter(
        &mut self,
        ir: &Pd0InterRef<'_>,
        bw: usize,
        bh: usize,
        abs_x: usize,
        abs_y: usize,
    ) -> u64 {
        let mut pred = core::mem::take(&mut self.scratch.pred);
        let mut cand = core::mem::take(&mut self.scratch.cand);
        let (mv0, mv1, bip) =
            self.inter_best_pred_into(ir, bw, bh, abs_x, abs_y, &mut pred, &mut cand);
        self.scratch.cand = cand;
        self.note_root_mv(
            bw,
            bh,
            abs_x - self.sb_x,
            abs_y - self.sb_y,
            true,
            mv0,
            mv1,
            bip,
        );
        let cost = self.lvl1_cost_from_pred(bw, bh, abs_x, abs_y, &pred, None);
        self.scratch.pred = pred;
        cost
    }

    /// The `padded_by_ref` lookup C does as
    /// `svt_aom_get_ref_pic_buffer(pcs, ref_frame)` — falling back to
    /// [`Pd0InterRef::padded_y`] (LAST) when the slot is unfilled, which on
    /// this port's single-picture low-delay envelope is the same picture.
    fn inter_ref_plane<'r>(ir: &Pd0InterRef<'r>, ref_frame: i8) -> &'r crate::picture::PaddedPlane {
        ir.padded_by_ref
            .get(ref_frame.max(0) as usize)
            .copied()
            .flatten()
            .map_or(ir.padded_y, |r| &r.y)
    }

    /// `svt_aom_inter_pu_prediction_av1_pd0` for ONE unipred candidate: the
    /// full-pel ME MV times 8 against the candidate's own reference picture.
    #[allow(clippy::too_many_arguments)]
    fn inter_pred_into(
        &self,
        ir: &Pd0InterRef<'_>,
        bw: usize,
        bh: usize,
        abs_x: usize,
        abs_y: usize,
        mv_fp: svtav1_types::motion::Mv,
        ref_frame: i8,
        out: &mut [u8],
    ) {
        assert!(
            self.is_lvl1_family() || matches!(self.mode, Pd0Mode::Lvl5),
            "PD0 inter compensation is wired for `md_encode_block_pd0`'s \
             levels (the LVL_1 family AND LVL_5), and the caller resolved \
             {:?}. C's PD0_LVL_6 inter arm is `compute_lpd0_cost_inter`'s \
             variance closed form, not this block encode; refusing rather \
             than costing an inter block with the wrong model.",
            self.mode
        );
        let mv = svtav1_types::motion::Mv {
            x: mv_fp.x.saturating_mul(8),
            y: mv_fp.y.saturating_mul(8),
        };
        crate::inter_pred_arm::predict_inter_luma_pd0(
            Self::inter_ref_plane(ir, ref_frame),
            abs_x,
            abs_y,
            bw,
            bh,
            mv,
            ir.sb_size,
            ir.frame_w,
            ir.frame_h,
            out,
            bw,
        );
    }

    /// The compound (NEW_NEWMV, MD_COMP_AVG) twin of [`Self::inter_pred_into`]
    /// — `av1_inter_prediction_pd0` with two MVs and two ref planes, whose
    /// convolve already averages when `mvs.len() > 1`.
    #[allow(clippy::too_many_arguments)]
    fn inter_pred_into_bipred(
        &self,
        ir: &Pd0InterRef<'_>,
        bw: usize,
        bh: usize,
        abs_x: usize,
        abs_y: usize,
        mv0_fp: svtav1_types::motion::Mv,
        rf0: i8,
        mv1_fp: svtav1_types::motion::Mv,
        rf1: i8,
        out: &mut [u8],
    ) {
        let mv0 = svtav1_types::motion::Mv {
            x: mv0_fp.x.saturating_mul(8),
            y: mv0_fp.y.saturating_mul(8),
        };
        let mv1 = svtav1_types::motion::Mv {
            x: mv1_fp.x.saturating_mul(8),
            y: mv1_fp.y.saturating_mul(8),
        };
        let p0 = Self::inter_ref_plane(ir, rf0);
        let p1 = Self::inter_ref_plane(ir, rf1);
        let rp0 = svtav1_dsp::port_pd_pred::RefPlane {
            buf: &p0.buf,
            origin: p0.origin,
            stride: p0.stride,
            width: p0.width as i32,
            height: p0.height as i32,
        };
        let rp1 = svtav1_dsp::port_pd_pred::RefPlane {
            buf: &p1.buf,
            origin: p1.origin,
            stride: p1.stride,
            width: p1.width as i32,
            height: p1.height as i32,
        };
        let sf = svtav1_dsp::port_scale_factors::ScaleFactors::setup_for_frame(
            ir.frame_w as i32,
            ir.frame_h as i32,
            ir.frame_w as i32,
            ir.frame_h as i32,
        );
        let edges = crate::inter_pred_arm::mb_edges(abs_x, abs_y, bw, bh, ir.frame_w, ir.frame_h);
        svtav1_dsp::port_pd_pred::av1_inter_prediction_pd0(
            &svtav1_dsp::port_pd_pred::BlkGeom {
                org_x: abs_x as i32,
                org_y: abs_y as i32,
                bwidth: bw,
                bheight: bh,
                bwidth_uv: bw / 2,
                bheight_uv: bh / 2,
                super_block_size: ir.sb_size as i32,
            },
            &[
                svtav1_dsp::port_subpel_params::Mv { x: mv0.x, y: mv0.y },
                svtav1_dsp::port_subpel_params::Mv { x: mv1.x, y: mv1.y },
            ],
            &[rp0, rp1],
            &[sf, sf],
            &edges,
            out,
            bw,
        );
    }

    /// The LVL_1 FAMILY — every level whose block cost is
    /// `md_encode_block_pd0` at `lpd0_qp_offset = 0` with a real (or
    /// approximated) coefficient rate, as opposed to LVL_5/LVL_0's
    /// `5000 + 100*eob` closed form or LVL_6's pure variance.
    #[inline]
    fn is_lvl1_family(&self) -> bool {
        matches!(self.mode, Pd0Mode::Lvl1 | Pd0Mode::Lvl3 | Pd0Mode::Lvl4)
    }

    /// Which PD0 modes price a one-false BOUNDARY node as its FITTING edge
    /// shape rather than as the square that does not fit.
    ///
    /// C decides this with no reference to `pd0_ctrls.pd0_level`:
    /// `set_blocks_to_test` (enc_dec_process.c:1394) injects exactly the
    /// fitting PART_H / PART_V on an incomplete node whenever NSQ geometry is
    /// enabled and the square is above `MAX(min_nsq, min_nsq_block_size)`
    /// (`:1420-1423`), and `svt_aom_pick_partition_pd0`
    /// (product_coding_loop.c:10534-10560) then costs
    /// `get_blk_geom_mds(mds_idx + ns_blk_offset_md[shape])` — the RECTANGLE.
    ///
    /// So the level list here is about what this PORT has MEASURED, not about
    /// what C does:
    /// * LVL_1 family — the allintra fixed-tree presets, wired 2026-08 (task
    ///   #95, the 96x80 milestone).
    /// * LVL_5 — added 2026-09-01 for the VIDEO arm. It could not matter on
    ///   the allintra arm, where `nsq_geom_level` is 0 above M6 so an LVL_5/6
    ///   boundary node force-splits before it can be costed at all; the video
    ///   arm never turns NSQ geometry off, so it reaches this and was pricing
    ///   the square. MEASURED against C's own `svt_aom_full_cost_pd0` dump —
    ///   see `lvl5_like_block_cost_rect`.
    /// * LVL_6 on the ALLINTRA arm is EXCLUDED — `nsq_geom_level` is 0 above
    ///   M6 there, so a boundary node force-splits before any shape is costed.
    ///   On the VIDEO arm (NSQ on, `md_disallow_nsq_search` notwithstanding)
    ///   C DOES cost the injected PART_H/PART_V at LVL_6 —
    ///   `md_encode_block_pd0` calls `compute_lpd0_cost_inter` on the rect's
    ///   own bsize — so `inter.is_some()` is the discriminator.
    /// * LVL_0 is EXCLUDED and that is a KNOWN GAP, not a claim about C: it is
    ///   the bd10-forced path (`set_pd0_ctrls`, enc_mode_config.c:5416), whose
    ///   partial-SB cells are byte-identical today, and nothing here has
    ///   dumped C's bd10 boundary cost. Widening it blind would trade a green
    ///   gate for a guess.
    fn prices_edge_shape(&self) -> bool {
        self.is_lvl1_family()
            || matches!(self.mode, Pd0Mode::Lvl5)
            || (matches!(self.mode, Pd0Mode::Lvl6) && self.inter.is_some())
    }

    /// C `ctx->subres_ctrls.step` for the LVL_1 family
    /// (`svt_aom_sig_deriv_enc_dec_pd0`, enc_mode_config.c:7337-7345): the
    /// RESOLVED per-SB value on the video arm, the level default elsewhere.
    /// LVL_5's own path passes [`Pd0Ctx::subres_step`] explicitly; LVL_1/0/6
    /// are step 0.
    #[inline]
    fn subres_step_cfg(&self) -> u32 {
        match self.mode {
            Pd0Mode::Lvl3 | Pd0Mode::Lvl4 => self.subres_step,
            _ => 0,
        }
    }

    fn block_cost(&mut self, sq_size: usize, org_x: usize, org_y: usize) -> u64 {
        match self.mode {
            Pd0Mode::Lvl1 | Pd0Mode::Lvl3 | Pd0Mode::Lvl4 => {
                self.lvl1_block_cost(sq_size, org_x, org_y)
            }
            Pd0Mode::Lvl5 => self.lvl5_block_cost(sq_size, org_x, org_y),
            Pd0Mode::Lvl0 => self.lvl0_block_cost(sq_size, org_x, org_y),
            // `md_encode_block_pd0` (product_coding_loop.c:8349-8358): the
            // LVL_6 cost is `compute_lpd0_cost_allintra` only when
            // `scs->allintra` — on a non-key VIDEO frame it is
            // `compute_lpd0_cost_inter`, the ME-candidate variance form.
            Pd0Mode::Lvl6 if self.inter.is_some() => {
                self.lvl6_block_cost_inter(sq_size, sq_size, org_x, org_y)
            }
            Pd0Mode::Lvl6 => lvl6_cost_allintra(&self.vars, sq_size, org_x, org_y, self.qp),
        }
    }

    /// C `svt_aom_pick_partition_pd0` + `test_split_partition_pd0`:
    /// parent-first DFS returning (cost, eval record) for this square
    /// node; the picked tree is `eval.tree()`.
    fn pick(&mut self, sq_size: usize, org_x: usize, org_y: usize) -> (u64, Pd0Eval) {
        // The SB root is quadrant 0 of nothing: C's `mds->index` for the root
        // is 0, which only matters for the `index < 3` leaf-update rule below.
        //
        // C's `svt_aom_mode_decision_kernel` (enc_dec_process.c:2989) DISCARDS
        // `svt_aom_pick_partition_pd0`'s return value at the SB root, and an
        // invalid root leaves `pc_tree->partition` at whatever
        // `svt_aom_init_sb_data` left. The port has no such reachable case —
        // an SB root is invalid only if EVERY in-bounds descendant is, and the
        // (0,0) quadrant chain always reaches a node with `has_rows &&
        // has_cols` before `min_sq` unless `min_sq` exceeds the SB's own
        // remainder, which `set_depth_removal_level_controls`' three
        // "entire SB can be covered" guards forbid — so this returns an
        // untested leaf rather than inventing a tree.
        match self.pick_q(sq_size, org_x, org_y, 0) {
            Some((cost, eval, _)) => (cost, eval),
            None => (0, Pd0Eval::untested(sq_size)),
        }
    }

    /// [`Pd0Ctx::pick`] with C's `mds->index` (this node's quadrant inside its
    /// parent) and the recon hand-back the neighbour-array protocol needs.
    ///
    /// The third return is this node's OWN block recon when the caller still
    /// has to write it — C's `test_split_partition_pd0` tail
    /// (product_coding_loop.c:10500) updates the arrays for the LAST quadrant,
    /// which `svt_aom_pick_partition_pd0`'s `mds->index < 3` guard
    /// deliberately skips "to avoid redundant copies". `None` everywhere the
    /// node either wrote itself or must not be written (it ended SPLIT).
    /// `None` is C's `svt_aom_pick_partition_pd0` returning **false** —
    /// `pc_tree->rdc.valid == 0`, i.e. this node costed no shape AND could not
    /// split. C's caller treats that as fatal to the PARENT's split ("all
    /// split quadrants must be valid for split to be selected",
    /// product_coding_loop.c:10481), which is NOT the same as an out-of-bounds
    /// quadrant — those are `continue`d and contribute nothing
    /// ([`Pd0Eval::off`]).
    fn pick_q(
        &mut self,
        sq_size: usize,
        org_x: usize,
        org_y: usize,
        quad_idx: usize,
    ) -> Option<(u64, Pd0Eval, Option<(alloc::vec::Vec<u8>, usize, usize)>)> {
        let abs_x = self.sb_x + org_x;
        let abs_y = self.sb_y + org_y;
        // C `svt_aom_write_modes_sb` early return: a node whose top-left is
        // outside the ALIGNED frame codes nothing. Its cost never enters a
        // parent decision (parents of off-frame nodes are forced-split edge
        // nodes, which ignore cost), so 0 is inert.
        if abs_x >= self.aligned_w || abs_y >= self.aligned_h {
            return Some((0, Pd0Eval::off(sq_size), None));
        }
        // spec 5.11.4 / `set_blocks_to_test` (enc_dec_process.c:1394) edge
        // predicate vs the ALIGNED grid. `half` = half the square's pixel
        // extent (C `hbs = (mi_size_wide[bsize] << 2) >> 1`).
        let half = sq_size / 2;
        let has_rows = abs_y + half < self.aligned_h;
        let has_cols = abs_x + half < self.aligned_w;
        let one_false = !has_rows || !has_cols;
        let both_false = !has_rows && !has_cols;
        // FORCED SPLIT — `set_blocks_to_test` (enc_dec_process.c:1405) yields
        // `tot_shapes = 0`, so PART_N is NEVER costed and the node splits with
        // no NONE/edge candidate. This fires for:
        //  - a BOTH-false node (extends past both edges), at every PD0 level;
        //  - a one-false node when NSQ geom is DISABLED (`!self.nsq_enabled`).
        //    `svt_aom_get_nsq_geom_level_allintra` returns level 0 → `enabled =
        //    0` for allintra CLI preset >= M7 (enc_mode_config.c:8240), which
        //    covers BOTH the LPD0 presets >= 9 (PD0_LVL_5/6) AND the LVL_1
        //    presets 7/8. C never injects the edge shape, so EVERY one-false
        //    boundary node force-splits, descending to the fitting sub-blocks
        //    (e.g. a thin 8-wide right edge -> all 8x8). Presets <= M6 keep NSQ
        //    enabled → the one-false edge-shape path below (`one_false &&
        //    self.nsq_enabled`), matching the M6 boundary milestone. (The C
        //    `sq_size <= MAX(min_nsq=4, min_nsq_block_size<=8)` term is inert:
        //    edge nodes are always >= 16 wide on an 8-aligned frame.)
        // 8x8 nodes are never edge nodes on an 8-aligned frame, so a
        // force-split node always has `sq_size > min_sq` and can split. A
        // has_rows && has_cols node can still STRADDLE the aligned extent (its
        // sq x sq block reaching past aligned); C codes such straddle blocks
        // reading its SB-extent pad and cropping the distortion, so the port
        // sizes the recon + chroma-source buffers to the SB extent — a
        // straddling block writes into the padded rows, never out of bounds.
        let forced_split = both_false || (one_false && !self.nsq_enabled);
        // C `init_md_scan` (enc_dec_process.c:1457): `mds->split_flag =
        // (sq_size > min_sq_size)` is set from the SIZE ALONE and is what
        // gates the recursion — `set_blocks_to_test`'s `tot_shapes = 0` says
        // only that no d1 SHAPE is costable here, never that the node splits.
        // When both hold the node is simply INVALID: no cost, no children,
        // `svt_aom_pick_partition_pd0` returns false.
        //
        // MEASURED 2026-09-02: this port force-split such a node anyway. On an
        // INTER frame `depth_removal_ctrls` raises `min_sq` above 8 — on a
        // superblock whose cropped extent is 40 px, `dimensions_require_8x8`
        // is false so `disallow_below_16x16` may arm and `min_sq` becomes 16 —
        // and the 16x16 both-false quadrant at the picture's bottom-right then
        // descended to 8x8, where `sq_size >= min_sq` is false, no cost
        // exists, and the leaf `expect` fired: "leaf must be tested (min_sq <=
        // size <= max_sq)". 14 of the 64 cells of
        // `tools/inter_completion_scan.sh` crashed there — every size
        // congruent to 40 mod 64 at preset 8/10/13. The old code carried the
        // reasoning that made it look safe: "8x8 nodes are never edge nodes on
        // an 8-aligned frame, so a force-split node always has sq_size >
        // min_sq". True at `min_sq == 8`, which is the only value a KEY frame
        // ever has, and false the moment depth removal raises it.
        if forced_split && sq_size <= self.min_sq {
            return None;
        }
        if forced_split {
            let mut children: Vec<Pd0Eval> = Vec::with_capacity(4);
            let mut total = 0u64;
            let mut last_recon: Option<(alloc::vec::Vec<u8>, usize, usize)> = None;
            let mut last_quad_valid = true;
            for i in 0..4 {
                let cx = org_x + (i & 1) * half;
                let cy = org_y + (i >> 1) * half;
                if self.sb_x + cx >= self.aligned_w || self.sb_y + cy >= self.aligned_h {
                    last_quad_valid = false;
                }
                // C `test_split_partition_pd0` returns false at the FIRST
                // invalid quadrant (:10481), before the array-update tail. A
                // force-split node has no shape of its own to fall back on, so
                // it is invalid too.
                let (c_cost, c_eval, c_recon) = self.pick_q(half, cx, cy, i)?;
                total += c_cost;
                if i == 3 {
                    last_recon = c_recon;
                }
                children.push(c_eval);
            }
            // C `test_split_partition_pd0`'s tail with an INVALID parent
            // (`tot_shapes == 0` -> `pc_tree->rdc.valid == 0`): split always
            // wins, so the last quadrant is the array-update part.
            if last_quad_valid && let Some((r, rw, rh)) = last_recon {
                self.write_recon(
                    self.sb_x + org_x + half,
                    self.sb_y + org_y + half,
                    rw,
                    rh,
                    Some(&r),
                );
            }
            // SPLIT rate feeding a STRADDLING parent's decision (the failing
            // thin-edge cells are self-contained from the SB root, where this
            // is inert; a straddling root like 48x48 consumes it). A both-false
            // node codes NO partition symbol -> rate 0; a one-false node codes
            // the BINARY SPLIT-vs-{H,V} symbol -> its alike rate (doubled at
            // LVL_5 since `use_accurate_part_ctx = 0`; 0 at LVL_6 allintra,
            // test_split_partition_pd0:10435; UNdoubled at LVL_1 presets 7/8
            // since `use_accurate_part_ctx = 1` at M7/M8, from this SB's
            // chained tables — the same boundary rate the preset<=6 edge-shape
            // node's split cost uses below).
            if !both_false {
                total += match self.mode {
                    // The SPLIT rate is doubled when `use_accurate_part_ctx =
                    // 0` — allintra above M8, where LVL_5/LVL_6 live, so their
                    // constructors set the flag false. On the VIDEO arm the
                    // flag is `enc_mode <= M8` (enc_mode_config.c:8955), so a
                    // preset-8 inter SB does NOT double — C charged the raw
                    // alike rate there, and doubling it priced the split out
                    // of `test_split_partition_pd0`'s early-exit window.
                    Pd0Mode::Lvl5 => rdcost(
                        self.lambda,
                        (if self.accurate_part_ctx { 1 } else { 2 })
                            * self.alike_split_sym_bits(sq_size, !has_rows),
                        0,
                    ),
                    Pd0Mode::Lvl0 => rdcost(
                        self.lambda,
                        2 * self.alike_split_sym_bits(sq_size, !has_rows),
                        0,
                    ),
                    // `test_split_partition_pd0` (:10434): the rate is 0 only
                    // for `allintra && pd0_level == PD0_LVL_6`. Inter LVL_6
                    // prices the binary boundary SPLIT exactly like LVL_5.
                    Pd0Mode::Lvl6 if self.inter.is_some() => rdcost(
                        self.lambda,
                        (if self.accurate_part_ctx { 1 } else { 2 })
                            * self.alike_split_sym_bits(sq_size, !has_rows),
                        0,
                    ),
                    Pd0Mode::Lvl6 => 0,
                    Pd0Mode::Lvl1 | Pd0Mode::Lvl3 | Pd0Mode::Lvl4 => {
                        let tables = self.lvl1.expect("LVL_1 family requires tables");
                        let mult = if self.accurate_part_ctx { 1 } else { 2 };
                        rdcost(
                            self.lambda,
                            mult * tables.boundary_split_bits(sq_size, !has_rows),
                            0,
                        )
                    }
                };
            }
            let ch: [Pd0Eval; 4] = children.try_into().expect("4 children");
            let eval = Pd0Eval {
                sq: sq_size,
                tested: false,
                sq_tested: false,
                cost: 0,
                split: true,
                off: false,
                children: Some(Box::new(ch)),
                root_det: None,
            };
            return Some((total, eval, None));
        }
        // A FITTING one-false node prices its EDGE SHAPE block, not the square
        // PART_N — C's LPD0 costs "PART_H/PART_V for boundary blocks"
        // (product_coding_loop.c:127). The square block would over-cost (twice
        // the pixels/coeffs) and wrongly lose to SPLIT. This "don't split" cost
        // competes with SPLIT exactly like the square path; a win makes the
        // node a PD0 leaf, coded as its (fitting) edge shape at
        // `encode_fixed_tree`. Only wired on the LVL_1 path (allintra
        // fixed-tree presets, incl. the 96x80 milestone); LVL_5/6 boundary
        // nodes keep the square cost.

        let tested = sq_size <= self.max_sq && sq_size >= self.min_sq;
        let parent_cost = if tested {
            if one_false && self.prices_edge_shape() {
                let (bw, bh) = if !has_rows {
                    (sq_size, half)
                } else {
                    (half, sq_size)
                };
                if self.is_lvl1_family() {
                    Some(self.lvl1_block_cost_rect(bw, bh, org_x, org_y))
                } else if matches!(self.mode, Pd0Mode::Lvl6) {
                    // `compute_lpd0_cost_inter` on the RECT's own bsize —
                    // `blk_geom->bsize` is the injected PART_H/PART_V.
                    Some(self.lvl6_block_cost_inter(bw, bh, org_x, org_y))
                } else {
                    // LVL_5's own closed form, with its resolved subres step.
                    Some(self.lvl5_like_block_cost_rect(bw, bh, org_x, org_y, self.subres_step))
                }
            } else {
                Some(self.block_cost(sq_size, org_x, org_y))
            }
        } else {
            None
        };
        // The node's own block recon, taken before the children can overwrite
        // `pending_recon`. Its DIMENSIONS are the shape that was costed.
        let node_recon = self.pending_recon.take();
        let (node_w, node_h) = if one_false && self.prices_edge_shape() {
            if !has_rows {
                (sq_size, half)
            } else {
                (half, sq_size)
            }
        } else {
            (sq_size, sq_size)
        };
        let mut eval = Pd0Eval {
            sq: sq_size,
            tested,
            // C `tested_blk[PART_N][0]`: a one-false node costs its injected
            // PART_H/PART_V, so the SQUARE slot stays untested
            // (svt_aom_pick_partition_pd0, product_coding_loop.c:10548-10560).
            // `one_false` is never true on a 64-aligned frame.
            sq_tested: tested && !one_false,
            cost: parent_cost.unwrap_or(0),
            split: false,
            off: false,
            children: None,
            root_det: None,
        };

        let split_flag = sq_size > self.min_sq;
        if !split_flag {
            // `parent_cost` is `Some` here because the recursion never
            // descends below `min_sq` (this branch is what stops it) and
            // `min_sq <= max_sq` — C asserts exactly that in
            // `set_blocks_to_be_tested` (enc_dec_process.c:1500), having
            // clamped `min_sq_size` to `static_config.max_tx_size` on the
            // preceding line for this reason. `pipeline.rs` applies the same
            // clamp when it fills `Pd0InterRef::min_sq`.
            let cost = parent_cost.expect(
                "a node at min_sq must be costable: C clamps min_sq_size to max_tx_size and \
                 asserts min_sq_size <= max_sq_size (enc_dec_process.c:1499-1500)",
            );
            // C `svt_aom_pick_partition_pd0` (product_coding_loop.c:10568):
            // a leaf updates the neighbour arrays itself for quadrants 0..2;
            // quadrant 3 is left to the parent's tail.
            if quad_idx < 3 {
                self.write_recon(abs_x, abs_y, node_w, node_h, node_recon.as_deref());
                return Some((cost, eval, None));
            }
            return Some((cost, eval, node_recon.map(|r| (r, node_w, node_h))));
        }

        // test_split_partition_pd0: split rate term (0 at LVL_6 allintra;
        // doubled at LVL_5 because use_accurate_part_ctx = 0 at eff-M9;
        // RAW at LVL_1 because use_accurate_part_ctx = 1 at M2..M8 —
        // observed 1195/1465/2020 in the instrumented PD0SPLITRATE dumps).
        let mut split_cost = match self.mode {
            // `test_split_partition_pd0` (:10434): 0 for `allintra &&
            // PD0_LVL_6` only; the inter LVL_6 arm prices the SPLIT symbol
            // through `svt_aom_partition_rate_cost` — the full-alphabet rate
            // interior, the binary alike rate at a one-false boundary node —
            // doubled when `use_accurate_part_ctx` is 0, same as LVL_5.
            Pd0Mode::Lvl6 if self.inter.is_none() => 0,
            Pd0Mode::Lvl6 if one_false => rdcost(
                self.lambda,
                (if self.accurate_part_ctx { 1 } else { 2 })
                    * self.alike_split_sym_bits(sq_size, !has_rows),
                0,
            ),
            Pd0Mode::Lvl6 => rdcost(
                self.lambda,
                (if self.accurate_part_ctx { 1 } else { 2 }) * self.split_sym_bits(sq_size),
                0,
            ),
            // LVL_0/LVL_5: `use_accurate_part_ctx = 0` -> SPLIT rate doubled,
            // priced from the DEFAULT partition CDF (ctx row 0).
            //
            // At a one-false BOUNDARY node the alphabet is BINARY
            // (split-vs-{H,V}, `svt_aom_partition_rate_cost` rd_cost.c:1846-
            // 1863), so the rate is the alike cost, not the full-alphabet
            // SPLIT cost — the same distinction the LVL_1 branch below makes.
            // It only matters where the node's non-split candidate exists,
            // i.e. exactly where `prices_edge_shape()` is true, so LVL_0 keeps
            // the full-alphabet rate along with the square cost.
            Pd0Mode::Lvl5 if one_false && self.prices_edge_shape() => rdcost(
                self.lambda,
                (if self.accurate_part_ctx { 1 } else { 2 })
                    * self.alike_split_sym_bits(sq_size, !has_rows),
                0,
            ),
            Pd0Mode::Lvl5 => rdcost(
                self.lambda,
                (if self.accurate_part_ctx { 1 } else { 2 }) * self.split_sym_bits(sq_size),
                0,
            ),
            Pd0Mode::Lvl0 => rdcost(self.lambda, 2 * self.split_sym_bits(sq_size), 0),
            Pd0Mode::Lvl1 | Pd0Mode::Lvl3 | Pd0Mode::Lvl4 => {
                let tables = self.lvl1.expect("LVL_1 family requires tables");
                // C `svt_aom_partition_rate_cost` (rd_cost.c:1846-1863): at a
                // one-false BOUNDARY node the SPLIT rate is the BINARY
                // split-vs-{H,V} cost (`partition_{vert,horz}_alike_fac_bits`),
                // not the full-alphabet `partition_fac_bits[ctx][SPLIT]`. Only
                // the LVL_1 family prices the edge shape (parent_cost), so only
                // it needs the matching boundary split rate; interior nodes and
                // LVL_5/6 keep the full-alphabet `split_bits`.
                let sbits = if one_false {
                    tables.boundary_split_bits(sq_size, !has_rows)
                } else {
                    tables.split_bits(sq_size)
                };
                // `use_accurate_part_ctx = 0` (enc_mode > M8) doubles it, the
                // same bias LVL_5 / LVL_0 hardcode.
                let mult = if self.accurate_part_ctx { 1 } else { 2 };
                rdcost(self.lambda, mult * sbits, 0)
            }
        };

        let half = sq_size / 2;
        let mut children: Vec<Pd0Eval> = Vec::with_capacity(4);
        let mut split_valid = true;
        let mut last_recon: Option<(alloc::vec::Vec<u8>, usize, usize)> = None;
        let mut last_quad_valid = true;
        let mut last_child_split = false;
        for i in 0..4 {
            let cx = org_x + (i & 1) * half;
            let cy = org_y + (i >> 1) * half;
            // C `test_split_partition_pd0` (product_coding_loop.c:10456):
            // a quadrant whose ORIGIN is outside the mi grid is `continue`d
            // BEFORE the depth-early-exit test, not after it. The port used to
            // run the test on those quadrants too, and because an out-of-bounds
            // child contributes 0 to `split_cost` the extra test at i == 3 can
            // fire on a running total that C has already finished accumulating
            // — turning C's "split wins" into the port's "parent wins".
            //
            // MEASURED on `gradient 72x88 q40 p5` video, SB1's 16x16 node at
            // (64,16): parent 4972162 vs split 4700296, so C splits; the port's
            // i == 3 test (`4972162 * 900 <= 4700296 * 1000`) fired and kept the
            // parent. Visible only once PD0 predicts from its own recon, because
            // the wrong winner is also what gets written into the neighbour
            // arrays — the block below then predicted off an 8x16's bottom row
            // where C uses an 8x8's.
            if self.sb_x + cx >= self.aligned_w || self.sb_y + cy >= self.aligned_h {
                last_quad_valid = false;
                children.push(Pd0Eval::off(half));
                continue;
            }
            // Early exits — disabled entirely for the ALLINTRA LVL_6 arm
            // (`!(pcs->slice_type == I_SLICE && ctx->pd0_ctrls.pd0_level ==
            // PD0_LVL_6)`, md_process.c:22865 — where `slice_type` is the
            // discriminator because `allintra` implies I-slice); the INTER
            // LVL_6 arm runs them with its derived `parent_cost_bias`.
            // th = split_cost_th(50) for i == 0, else early_exit_th
            // (0 -> 1000). Identical ths at LVL_5 and LVL_1
            // (depth_early_exit level 1 for both, enc_mode_config.c:9282).
            if (self.mode != Pd0Mode::Lvl6 || self.inter.is_some())
                && let Some(pc) = parent_cost
            {
                let th: u128 = if i == 0 { 50 } else { self.depth_early_exit_th };
                let bias = u128::from(self.parent_cost_bias);
                #[cfg(feature = "std")]
                if crate::dbgenv::pd0dbg() {
                    eprintln!(
                        "PD0SPLIT org=({},{}) sq={} i={} pc={} th={} split_cost={} lhs={} rhs={}",
                        abs_x,
                        abs_y,
                        sq_size,
                        i,
                        pc,
                        th,
                        split_cost,
                        (pc as u128) * th * bias,
                        (split_cost as u128) * 1_000_000,
                    );
                }
                if (pc as u128) * th * bias <= (split_cost as u128) * 1_000_000 {
                    split_valid = false;
                    break;
                }
            }
            // C `test_split_partition_pd0` (:10479-10483): "If split is
            // invalid, then exit (all split quadrants must be valid for split
            // to be selected)." Same effect as the depth-early-exit above —
            // the split is abandoned and the node keeps its own shape — so it
            // shares that arm rather than duplicating it.
            let Some((child_cost, child_eval, child_recon)) = self.pick_q(half, cx, cy, i) else {
                split_valid = false;
                break;
            };
            split_cost += child_cost;
            if i == 3 {
                last_recon = child_recon;
                last_child_split = child_eval.split;
            }
            children.push(child_eval);
        }

        // Record the visited children (C: their pc_tree nodes were
        // populated by the recursion even when the parent ends NONE);
        // quadrants skipped by the early exit stay untested.
        if !children.is_empty() {
            while children.len() < 4 {
                children.push(Pd0Eval::untested(half));
            }
            let ch: [Pd0Eval; 4] = children.try_into().expect("4 children");
            eval.children = Some(Box::new(ch));
        }

        if !split_valid {
            // The depth-early-exit arm cannot get here without a parent cost
            // (it reads one to fire), but the invalid-child arm can: a node
            // whose `sq_size` is outside `[min_sq, max_sq]` costs no shape
            // (C `init_md_scan`'s `test_depth`), and if its split is also
            // invalid then `pc_tree->rdc.valid` stays 0 and
            // `svt_aom_pick_partition_pd0` returns false to ITS parent.
            let Some(cost) = parent_cost else {
                return None;
            };
            // C `svt_aom_pick_partition_pd0` (:10564): `if (!valid_part &&
            // pc_tree->rdc.valid) mode_decision_update_neighbor_arrays_pd0`.
            // The abandoned split's children may already have written; the
            // node's own recon now supersedes them, exactly as in C.
            self.write_recon(abs_x, abs_y, node_w, node_h, node_recon.as_deref());
            return Some((cost, eval, None));
        }

        // `parent_cost_bias * pc <= split_cost * 1000`
        // (test_split_partition_pd0:10490) — the bias is 1000 everywhere but
        // the inter LVL_6 arm, where the per-SB derived value applies.
        #[cfg(feature = "std")]
        if crate::dbgenv::pd0dbg() {
            eprintln!(
                "PD0FIN org=({abs_x},{abs_y}) sq={sq_size} pc={parent_cost:?} split_cost={split_cost} bias={}",
                self.parent_cost_bias
            );
        }
        if let Some(pc) = parent_cost
            && pc * u64::from(self.parent_cost_bias) <= split_cost * 1000
        {
            // C `test_split_partition_pd0` (:10490): the parent keeps its
            // partition, so IT is the array-update part.
            self.write_recon(abs_x, abs_y, node_w, node_h, node_recon.as_deref());
            return Some((pc, eval, None));
        }
        eval.split = true;
        // Split wins: the array-update part is the LAST quadrant, and only
        // when it is in bounds and not itself split (:10496-10508).
        if last_quad_valid
            && !last_child_split
            && let Some((r, rw, rh)) = last_recon
        {
            self.write_recon(
                self.sb_x + org_x + half,
                self.sb_y + org_y + half,
                rw,
                rh,
                Some(&r),
            );
        }
        Some((split_cost, eval, None))
    }

    /// C `mode_decision_update_neighbor_arrays_pd0` (product_coding_loop.c:121)
    /// — a no-op on the ALLINTRA arm, where `pd0_use_src_samples` short-circuits
    /// it and the port carries no canvas.
    fn write_recon(
        &mut self,
        abs_x: usize,
        abs_y: usize,
        bw: usize,
        bh: usize,
        recon: Option<&[u8]>,
    ) {
        if let Some(cv) = self.recon_canvas.as_mut()
            && let Some(r) = recon
            && r.len() >= bw * bh
        {
            #[cfg(feature = "std")]
            if crate::dbgenv::pd0dbg() {
                eprintln!(
                    "PD0WR org=({abs_x},{abs_y}) {bw}x{bh} lastrow={:?}",
                    &r[(bh - 1) * bw..(bh - 1) * bw + bw.min(8)]
                );
            }
            cv.write(abs_x, abs_y, bw, bh, r);
        }
    }
}

#[cfg(test)]
mod alt_lambda_tests;
#[cfg(test)]
mod inter_lambda_tests;
/// Differential parity for the post-MD RD lambdas against the REAL exported
/// `svt_aom_compute_rd_mult_based_on_qindex` (rc_process.c:365) — the base
/// that `svt_aom_lambda_assign` builds every one of them from.
///
/// Both bd10 lambdas added for the bd10 CDEF/LR searches are pinned here
/// across the whole qindex range, not at hand-picked anchors: the bd10 chain
/// (`dc_qlookup_10` -> `(3.3+0.0015q)q²` -> `ROUND_POWER_OF_TWO(_,4)` ->
/// clamp -> `*128>>7`) has four places a transcription can be off by one and
/// only the C symbol settles them.
#[cfg(test)]
mod lambda_c_parity;
#[cfg(test)]
mod tests;
mod variance;
pub use variance::*;
mod lambda;
pub use lambda::*;
mod quantize;
use quantize::*;
mod coeff_cost;
pub use coeff_cost::*;

mod entry;
pub use entry::*;
