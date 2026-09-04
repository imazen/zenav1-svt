//! The funnel's data model: the per-candidate working state, the caller-facing
//! context, and the evaluated-leaf result.
//!
//! [`Cand`] is one candidate's evolving state as it walks MDS0 -> MDS1 -> MDS3;
//! [`FunnelCtx`] is everything a leaf evaluation reads and mutates outside
//! itself; [`LeafEval`] is the decided winner plus the reconstruction the
//! commit step stamps back.
//!
//! Split out of `leaf_funnel/mod.rs` on 2026-08-25. PURE CODE MOVEMENT: every
//! item keeps its name, order and effective visibility (file-private became
//! `pub(super)`, the same scope).

use super::*;

/// One funnel candidate's evolving state.
///
/// `Default` is TEST-ONLY on purpose: production always fills every field at
/// injection, and a derived default in the shipping build would let a new
/// field be forgotten there silently. Under `cfg(test)` it is what lets the
/// NIC staging pins state a candidate as the two costs and the lane that
/// decide its fate, instead of forty fields of noise.
#[cfg_attr(test, derive(Default))]
pub(super) struct Cand {
    pub(super) mode: u8,
    /// Luma angle delta (directional modes only; C ANGLE_STEP units).
    pub(super) delta: i8,
    pub(super) fi: u8,
    pub(super) uv: u8,
    /// Chroma angle delta (= luma delta at injection; rewritten by the
    /// ind-uv MDS3 update at chroma_level 4).
    pub(super) uv_delta: i8,
    /// Whole-block depth-0 luma prediction (w x h).
    pub(super) pred: Vec<u8>,
    /// The SAME prediction at TRUE 10 bits, from the bd10 recon canvas
    /// (task #94). MDS0 already computes this to score the fast cost and
    /// used to throw it away; MDS1/MDS3 need it as their depth-0 predictor.
    /// Empty unless the bd10 full-RD funnel is active.
    pub(super) pred10: Vec<u16>,
    pub(super) flr: u64,
    pub(super) fcr: u64,
    pub(super) fast_cost: u64,
    // MDS1:
    pub(super) full_cost: u64,
    /// [SVT_HDR_MODE] parallel SSIM full cost (only when frame.tune_ssim).
    pub(super) mds3_cost_ssim: u64,
    pub(super) mds1_has_coeff: bool,
    // MDS3 winner data:
    pub(super) tx_depth: u8,
    pub(super) txb_q: Vec<Vec<i32>>,
    pub(super) txb_eob: Vec<u16>,
    pub(super) txb_cul: Vec<u8>,
    pub(super) txb_type: Vec<u8>,
    pub(super) y_recon: Vec<u8>,
    /// The winner's TRUE 10-bit LUMA recon (w*h), from the winning tx depth
    /// of the bd10 MDS3 loop. Empty unless the bd10 full-RD funnel is active.
    pub(super) y_recon10: Vec<u16>,
    /// The winner's TRUE 10-bit chroma recon (cw*chh each), produced by the
    /// bd10 chroma full loop. `commit_leaf` writes them into the bd10 chroma
    /// canvases so the NEXT block predicts chroma from 10-bit neighbours —
    /// the same sequential coupling `y_recon10` closes for luma. Empty
    /// unless the bd10 full-RD funnel is active.
    pub(super) u_recon10: Vec<u16>,
    pub(super) v_recon10: Vec<u16>,
    /// The tx_depth-0 luma recon (C's shared `cand_bf->recon` state after the
    /// TX loop — deeper depths reconstruct in aux buffers and are never
    /// copied back, so the quad-dist gates measure THIS, not `y_recon`).
    pub(super) y_recon_d0: Vec<u8>,
    pub(super) y_bits: u64,
    pub(super) y_dist: u64,
    pub(super) u_q: Vec<i32>,
    pub(super) v_q: Vec<i32>,
    pub(super) u_eob: u16,
    pub(super) v_eob: u16,
    pub(super) u_cul: u8,
    pub(super) v_cul: u8,
    pub(super) u_recon: Vec<u8>,
    pub(super) v_recon: Vec<u8>,
    /// CfL alpha idx/signs when the MDS3 chroma decision picked
    /// UV_CFL_PRED (uv == 13); both 0 otherwise (C block_mi.cfl_alpha_*).
    pub(super) cfl_alpha_idx: u8,
    pub(super) cfl_alpha_signs: u8,
    /// Luma palette candidate payload (colors, full-size idx map) — Some
    /// only for candidates injected by `inject_palette_candidates`
    /// (mode == DC, fi == NONE). The prediction is map->colors
    /// SUBSTITUTION (position-only, no neighbor edges) at every stage.
    pub(super) palette: Option<(Vec<u16>, Vec<u8>)>,
    /// IntraBC candidate payload `(dv, pred_dv)` (IBC chunk 7/8) — Some
    /// only for candidates injected by the IBC lane
    /// (`inject_intra_bc_candidates`): the winning eighth-pel DV +
    /// `ref_mv_stack[INTRA_FRAME][0].this_mv` (the dv_ref the writer's
    /// `svt_av1_encode_dv` diffs against). The candidate's other fields
    /// follow `build_intra_bc_candidate`: mode DC (0), uv DC (0), fi
    /// NONE, deltas 0 — an IBC cand is `is_inter`-classified everywhere
    /// (tx set, tx_size vartx coding, no CfL / no ind-uv rewrite).
    pub(super) ibc: Option<(svtav1_types::motion::Mv, svtav1_types::motion::Mv)>,
    /// INTER candidate payload (`docs/INTER-ENCODE-PLAN.md` §1s item 1b) —
    /// `Some` only for candidates injected by
    /// [`super::inject::inject_inter_candidates`]. Like `ibc`, an inter cand
    /// is `is_inter`-classified everywhere ([`Cand::is_inter`]): the inter
    /// ext-tx set, the var-tx `tx_size` coding, no CfL and no ind-uv rewrite.
    /// Its `mode` / `uv` fields stay 0 because an inter block codes NEITHER
    /// an intra y_mode nor a uv_mode (`docs/INTER-ENCODE-PLAN.md` §1x defect
    /// 2), and the neighbour grid reads `InterCand::mode` instead (defect 6).
    pub(super) inter: Option<alloc::boxed::Box<InterCand>>,
    pub(super) mds3_cost: u64,
    pub(super) block_has_coeff: bool,
    /// C `blk_ptr->total_rate` / `full_dist` (svt_aom_full_cost writeback)
    /// — read by the NSQ component-multiple / recon-dist gates.
    pub(super) total_rate: u64,
    pub(super) full_dist: u64,
}

impl Cand {
    /// C `is_inter_block(mbmi)` = `use_intrabc || ref_frame[0] > INTRA_FRAME`
    /// (block_structures.h:119).
    ///
    /// `docs/INTER-ENCODE-PLAN.md` §1u and §1x record what happens when a
    /// site tests `use_intrabc` instead: while IntraBC was the only
    /// inter-CLASSIFIED candidate the funnel could build, the two predicates
    /// were the same, and four separate pack defects came from the moment
    /// they stopped being.
    pub(super) fn is_inter(&self) -> bool {
        self.ibc.is_some() || self.inter.is_some()
    }
}

/// One INTER candidate's mode-decision payload.
///
/// It carries exactly what MODE DECISION chooses; the three context fields C
/// caches on `BlkStruct` (`pred_mv`, `inter_mode_ctx`, `drl_ctx`) are derived
/// in the pack from the committed mode-info grid instead — see §1u for why
/// that split is structural rather than cosmetic. `pred_mv` is the one
/// exception: the MV RATE needs it at injection time, before any pack runs.
#[derive(Clone, Debug)]
pub struct InterCand {
    pub mode: svtav1_types::prediction::PredictionMode,
    /// C `ref_frame[2]`; `[LAST_FRAME, NONE_FRAME]` = `[1, -1]` for the
    /// single-reference low-delay shape this port injects.
    pub ref_frame: [i8; 2],
    /// Eighth-pel MVs, one per reference.
    pub mv: [svtav1_types::motion::Mv; 2],
    /// The MVP stack's chosen predictor — what the writer differences the
    /// coded MV against, and what the MV rate is measured from.
    pub pred_mv: [svtav1_types::motion::Mv; 2],
    pub drl_index: u8,
    /// C's packed `(y) | (x << 16)` interpolation filter pair.
    pub interp_filters: u32,
    pub motion_mode: crate::port_entropy_inter::modes::MotionMode,
    pub num_proj_ref: u8,
    pub overlappable_neighbors: u8,
    /// The MOTION-COMPENSATED chroma prediction (`cw * chh` each), produced
    /// with the luma one in a single `av1_inter_prediction_light_pd1` call at
    /// injection (§1s item 6). It is carried rather than recomputed because
    /// C's chroma arm reuses the LUMA block's `compute_subpel_params` result
    /// at a halved origin — predicting chroma separately would be different
    /// arithmetic, not a refactor.
    pub u_pred: alloc::vec::Vec<u8>,
    pub v_pred: alloc::vec::Vec<u8>,
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
    /// CfL alpha idx/signs for a UV_CFL_PRED (uv_mode == 13) leaf; the
    /// entropy writer emits `write_cfl_alphas` from these. 0/0 otherwise.
    pub cfl_alpha_idx: u8,
    pub cfl_alpha_signs: u8,
    /// Winning palette payload (colors, full-size idx map) — Some iff the
    /// palette candidate won this leaf; flows into BlockDecision.palette.
    pub palette: Option<(Vec<u16>, Vec<u8>)>,
    /// IBC chunk 8: `(dv, dv_ref)` — Some iff the IntraBC candidate won
    /// this leaf; flows into BlockDecision (chunk 9) for the pack's
    /// `write_intrabc_info` + var-tx tx_size writer.
    pub ibc: Option<(svtav1_types::motion::Mv, svtav1_types::motion::Mv)>,
    /// The winning INTER candidate's payload — `Some` iff an inter candidate
    /// won this leaf. Flows into `BlockDecision::inter`, which
    /// `encode_block_syntax`'s inter arm requires (it REFUSES rather than
    /// falling back; §1u).
    pub inter: Option<alloc::boxed::Box<InterCand>>,
}

/// Per-frame/SB mutable funnel context threaded through the fixed tree.
/// Native 10-bit (u16) SOURCE planes for the bd10 funnel — task #6 chunk 1.
///
/// `Some` on [`FunnelCtx::src10`] exactly when the caller entered through
/// [`crate::pipeline::EncodePipeline::try_encode_frame_420_hbd`]: the planes
/// carry the REAL 10-bit samples instead of the `u8 << 2` widening every bd10
/// stage used before. Frame-strided over the ALIGNED frame (the bd10 envelope
/// is 64-aligned-gated, so `y_stride` equals the funnel's `y_src`/`y_recon`
/// stride and a block at `(abs_x, abs_y)` indexes identically in both).
#[derive(Clone, Copy)]
pub(crate) struct FunnelSrc10<'a> {
    pub y: &'a [u16],
    pub y_stride: usize,
    /// Chroma planes at `c_stride` (== the u8 `u_src`/`v_src` layout). Empty
    /// on a monochrome frame (which never builds a funnel today).
    pub u: &'a [u16],
    pub v: &'a [u16],
    pub c_stride: usize,
}

pub(crate) struct FunnelCtx<'a> {
    pub u_src: &'a [u8],
    pub v_src: &'a [u8],
    /// Native 10-bit source planes (task #6 chunk 1). `None` on every u8
    /// entry point AND on a bd10 encode of an 8-bit source, where the bd10
    /// stages keep widening `u8 << 2` exactly as before — so every existing
    /// gate cell is byte-unchanged.
    pub src10: Option<FunnelSrc10<'a>>,
    pub u_recon: &'a mut [u8],
    pub v_recon: &'a mut [u8],
    pub c_stride: usize,
    pub ectx: &'a mut crate::pipeline::EntropyCtx,
    pub rates: &'a MdRates,
    pub frame: &'a FunnelFrame,
    /// bd10 LUMA mode-decision recon canvas (task #94, the u16 mode funnel):
    /// the TRUE 10-bit reconstruction of every committed block, frame-strided
    /// (== the u8 `y_recon` canvas dims/stride). `Some` ONLY for complete-SB
    /// eff-M9 (preset ≥ 9) bd10 frames; `None` (bd8, and every other bd10
    /// preset/partial-SB) leaves the funnel byte-IDENTICAL. When present,
    /// `evaluate_leaf`'s MDS0 fast loop predicts each candidate at 10-bit from
    /// this canvas and scores the 10-bit SATD (so the mode survivor is C's
    /// bd10 winner, not the u8 winner — the DC↔SMOOTH flips on diagonal-edge
    /// content), and `commit_leaf` writes the winner's 10-bit recon back for
    /// the next block's neighbours. The coded LEVELS come from the post-pass
    /// `bd10_reencode_luma`, which reads these bd10-decided modes.
    pub y_recon10: Option<&'a mut [u16]>,
    /// bd10 CHROMA mode-decision recon canvases — the chroma twins of
    /// `y_recon10`, chroma-strided (`c_stride`). `Some` exactly when
    /// `full_rd10` is set: the MDS3 chroma full loop predicts from them so
    /// the joint (luma + chroma) block RD is entirely 10-bit.
    pub u_recon10: Option<&'a mut [u16]>,
    pub v_recon10: Option<&'a mut [u16]>,
    /// Run the FULL-RD stages (MDS1 + MDS3, luma AND chroma) at bd10.
    ///
    /// `y_recon10` alone only fixes MDS0, which is sufficient at eff-M9
    /// (`nic_counts == (1,1,1)` -> the fast survivor IS the coded mode) but
    /// NOT below it: at M6 `nic_counts == (6,6,6)`, several candidates reach
    /// MDS1/MDS3 and the full-RD compare picks the winner. Widening only the
    /// MDS0 funnel to M6..M8 was measured to close ZERO cells
    /// (docs/bd10-port-map.md "MEASURED NEGATIVE"), which is what this flag
    /// exists to fix. Requires `y_recon10`/`u_recon10`/`v_recon10` to be set.
    pub full_rd10: bool,
    /// IBC chunk 8: frame-level IntraBC search state (hash table, site
    /// config, search cost tables, ctrls, tile/mi geometry). `None`
    /// unless `cfg.allow_intrabc` — every IBC path is unreachable then.
    pub ibc: Option<&'a IbcFrameState>,
    /// The MD mode-info grid the INTRA_FRAME MVP scans read (C
    /// `pcs->mi_grid_base` as MD stamps it): one entry per 4x4 mi cell,
    /// frame-wide, stamped by [`commit_leaf`] per mid-walk commit exactly
    /// like C's `svt_aom_update_mi_map` (product_coding_loop.c:670) — and
    /// NOT restored by the NSQ walk's node snapshots (C never restores the
    /// mi map between shapes; losing shapes' stamps linger until
    /// overwritten, so this lives OUTSIDE `EntropyCtx`). `None` unless
    /// `cfg.allow_intrabc`.
    pub ibc_mvp: Option<&'a mut alloc::vec::Vec<crate::intrabc_mvp::MvpMiEntry>>,
    /// Per-leaf IBC gate input, set by the partition/NSQ walk before each
    /// `evaluate_leaf` call (the C `ctx->shape` + `pc_tree` state the
    /// `do_intra_bc` gate reads, mode_decision.c:3597-3616).
    pub ibc_gate: IbcGateInput,
    /// INTER frame state (`docs/INTER-ENCODE-PLAN.md` §1s items 1b/2/3/6):
    /// the padded DPB reference, the frame's open-loop motion search, the
    /// inter rate tables and the MVP environment. `None` on a KEY frame,
    /// where every inter path is unreachable — which is what keeps the whole
    /// still envelope byte-identical by construction.
    ///
    /// It shares [`Self::ibc_mvp`] as its mode-info grid: the grid IS C's
    /// `mi_grid_base` as MD stamps it, and IntraBC (an intra-frame-only tool
    /// — the spec gates `allow_intrabc` on an intra frame) and inter
    /// prediction can never both be live on one frame, so there is exactly
    /// one grid and one stamping site.
    pub inter: Option<&'a crate::inter_md_arm::InterMdFrame<'a>>,
    /// C `ctx->sq_sb_me_mv` + `pc_tree->tested_blk[PART_N][0]`
    /// ([`crate::inter_search_arm::SqMeState`]) — ONE slot, written by every
    /// square block's MD motion search and read by the NSQ shapes that follow
    /// it. It is mutable and lives on the funnel context for the same reason
    /// `ibc_mvp` does: it is state that crosses blocks, and C keeps it on the
    /// mode-decision context for exactly that reason.
    pub inter_sq_me: Option<&'a mut crate::inter_search_arm::SqMeState>,
}

/// C `BlockSize` enum index from pixel dims (definitions.h block order) —
/// the MVP block-ctx derivation consumes the C index.
pub(crate) fn c_bsize_index(w: usize, h: usize) -> usize {
    match (w, h) {
        (4, 4) => 0,
        (4, 8) => 1,
        (8, 4) => 2,
        (8, 8) => 3,
        (8, 16) => 4,
        (16, 8) => 5,
        (16, 16) => 6,
        (16, 32) => 7,
        (32, 16) => 8,
        (32, 32) => 9,
        (32, 64) => 10,
        (64, 32) => 11,
        (64, 64) => 12,
        (64, 128) => 13,
        (128, 64) => 14,
        (128, 128) => 15,
        (4, 16) => 16,
        (16, 4) => 17,
        (8, 32) => 18,
        (32, 8) => 19,
        (16, 64) => 20,
        (64, 16) => 21,
        _ => panic!("no C BlockSize for {w}x{h}"),
    }
}

/// The per-leaf inputs of the IBC injection gate + the current block's
/// live partition (C `pc_tree->partition` on the current mbmi — read by
/// `has_top_right`'s VERT_A case via the CURRENT mi cell).
#[derive(Clone, Copy, Debug)]
pub(crate) struct IbcGateInput {
    /// C PartitionType of the shape under evaluation (NONE=0, HORZ=1,
    /// VERT=2, SPLIT=3, HORZ_A=4, HORZ_B=5, VERT_A=6, VERT_B=7,
    /// HORZ_4=8, VERT_4=9).
    pub partition: u8,
    /// `ctx->shape == PART_N`.
    pub is_part_n: bool,
    /// The node's PART_N (square) winner: `(tested, used_intrabc)` — C
    /// `pc_tree->tested_blk[PART_N][0]` +
    /// `block_data[PART_N][0]->block_mi.use_intrabc`.
    pub sibling_n0: (bool, bool),
}

impl Default for IbcGateInput {
    /// The fixed-tree default: PART_N (square leaves; the gate always
    /// allows — b4 gating is off at every allintra IBC level).
    fn default() -> Self {
        Self {
            partition: 0,
            is_part_n: true,
            sibling_n0: (false, false),
        }
    }
}

/// Frame-constant IntraBC search state (IBC chunk 8) — everything
/// `intra_bc_search` + the MVP build need beyond the funnel context.
/// Built once per frame in the pipeline when `allow_intrabc`.
pub struct IbcFrameState {
    /// Per-level controls with the one-shot QP mesh rescale applied
    /// (md_config_process.c:956-969).
    pub ctrls: crate::intrabc::IbcCtrls,
    /// The frame source hash table (`generate_ibc_data`).
    pub hash: crate::intrabc_hash::HashTable,
    /// Diamond site config (per-frame, source stride baked).
    pub sites: crate::intrabc::SearchSiteConfig,
    /// SEARCH-time mv cost tables: C `md_rate_est_ctx->nmv_vec_cost` /
    /// `nmvcoststack` — built from `fc->nmvc` at precision
    /// `allow_high_precision_mv` (= 0 = LOW on a KEY frame, i.e. WITH
    /// fractional-bit costs; svt_aom_estimate_mv_rate). Frame-constant
    /// (update_mv forced 0 on I-slices). Distinct from the RD-time
    /// `FunnelFrame::dv_tables` (ndvc at MV_SUBPEL_NONE).
    pub search_tables: crate::intrabc::MvCostTables,
    /// `svt_aom_get_sad_per_bit(base_q_idx, 0)` (mode_decision.c:3010).
    pub sad_per_bit: i32,
    /// `full_lambda >> RD_EPB_SHIFT`, min 1 (mode_decision.c:3011-3012).
    pub error_per_bit: i32,
    pub mi_rows: i32,
    pub mi_cols: i32,
    pub tile: crate::intrabc::TileMiBounds,
    pub sb_mi_size: i32,
    pub sb_size_log2_mi: u32,
    pub sb_size_px: i32,
    /// `pcs->pic_disallow_4x4` — gates the 4x4 hash size out of the table.
    pub disallow_4x4: bool,
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
    pub(super) has_uv: bool,
    pub(super) ccx: usize,
    pub(super) ccy: usize,
    pub(super) cw: usize,
    pub(super) chh: usize,
    pub(super) win: Cand,
    /// The shared `cand_bf->recon` state the quad-dist gates measure
    /// (skip-sub-depth cond1 + the NSQ recon-dist gates): bypass_encdec=0
    /// -> the winner rebuild (== winner final recon+chroma); bypass=1 ->
    /// the LAST MDS3 candidate's depth-0 luma recon + its chroma (the
    /// rebuild is redirected away and never reaches the shared buffer).
    pub(super) gate_y: Vec<u8>,
    pub(super) gate_u: Vec<u8>,
    pub(super) gate_v: Vec<u8>,
    /// C `cand_bf->residual` content at `non_normative_txs` time: ALL
    /// MDS3 candidates share ONE residual workspace (verified by buffer-
    /// pointer instrumentation — docs/captures/nsq_m2m3), so the buffer
    /// holds the LAST MDS3-processed candidate's whole-block DEPTH-0
    /// residual (the depth-1/2 trials write the per-depth scratch
    /// buffers, init_tx_cand_bf copies OUT of this one).
    pub(super) psq_resid: Vec<i32>,
    /// bd10 twin of `psq_resid` (task #94, root #2): the LAST MDS3 candidate's
    /// whole-block depth-0 residual at TRUE 10 bits (`src10 - last.pred10`).
    /// C's `non_normative_txs` (product_coding_loop.c:9180) transforms +
    /// quantizes this at `EB_TEN_BIT` (Q10 tables, `svt_aom_highbd_quantize_b`)
    /// to derive `min_nz_h`/`min_nz_v` — the counts the `skip_by_sq_txs` NSQ
    /// gate reads. Deciding that gate on the bd8 residual + Q8 quant flips
    /// which NSQ shapes are pruned (H-vs-V), so the port over/under-splits at
    /// bd10. Empty on the u8 path (bd8 keeps `psq_resid`, byte-unchanged).
    pub(super) psq_resid10: Vec<i32>,
    /// bd10 mode funnel (task #94): the winner's TRUE 10-bit recon (w×h
    /// raster), reconstructed by `evaluate_leaf` from the bd10 canvas when
    /// `FunnelCtx::y_recon10` is `Some`. `commit_leaf` writes it back into the
    /// canvas for the next block's neighbour prediction. Empty on the u8 path.
    pub(super) win_recon10: Vec<u16>,
    /// The winner's TRUE 10-bit CHROMA recon (cw*chh each) — the chroma twins
    /// of `win_recon10`, written into the bd10 chroma canvases by
    /// `commit_leaf`. Empty unless the bd10 full-RD funnel is active.
    pub(super) win_u_recon10: Vec<u16>,
    pub(super) win_v_recon10: Vec<u16>,
}

impl LeafEval {
    /// The winner's MDS3 full cost (C `blk_ptr->cost` before the
    /// partition-rate term the depth walk adds).
    pub(crate) fn block_cost(&self) -> u64 {
        self.win.mds3_cost
    }

    /// IBC chunk 8: whether the winner is an IntraBC candidate — the C
    /// `block_data[PART_N][0]->block_mi.use_intrabc` the NSQ parent gate
    /// reads (mode_decision.c:3608-3612).
    pub(crate) fn used_ibc(&self) -> bool {
        self.win.ibc.is_some()
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

    /// Winner tx_depth (diagnostic; only read by the std-gated NSQDBG dumps).
    #[cfg(feature = "std")]
    pub(crate) fn tx_depth(&self) -> u8 {
        self.win.tx_depth
    }

    /// Winner uv_mode (diagnostic — 13 == UV_CFL_PRED; std-gated NSQDBG only).
    #[cfg(feature = "std")]
    pub(crate) fn uv_mode(&self) -> u8 {
        self.win.uv
    }

    pub(crate) fn block_has_coeff(&self) -> bool {
        self.win.block_has_coeff
    }

    /// NSQDBG only: winner per-txb tx types / luma eobs as "a,b,c" strings,
    /// plus chroma eobs — joined against C's CLEAF dump to catch coeff-level
    /// (tx_type/RDOQ) divergence that mode/uv/txd comparison misses.
    /// std-only (returns `String`; only consumed by the std-gated NSQDBG dumps).
    #[cfg(feature = "std")]
    pub(crate) fn dbg_txb_types(&self) -> String {
        let v: Vec<String> = self.win.txb_type.iter().map(|t| t.to_string()).collect();
        v.join(",")
    }

    #[cfg(feature = "std")]
    pub(crate) fn dbg_txb_eobs(&self) -> String {
        let v: Vec<String> = self.win.txb_eob.iter().map(|e| e.to_string()).collect();
        v.join(",")
    }

    #[cfg(feature = "std")]
    pub(crate) fn dbg_uv_eobs(&self) -> (u16, u16) {
        (self.win.u_eob, self.win.v_eob)
    }

    /// NSQDBG only: the winner's filter-intra mode (0 == FI off/none for
    /// non-DC winners; distinguishes FILTER_* candidates from plain DC).
    #[cfg(feature = "std")]
    pub(crate) fn dbg_fi(&self) -> u8 {
        self.win.fi
    }

    /// NSQDBG only: the winner's luma + chroma angle deltas.
    #[cfg(feature = "std")]
    pub(crate) fn dbg_deltas(&self) -> (i8, i8) {
        (self.win.delta, self.win.uv_delta)
    }

    /// NSQDBG only: the winner's per-txb quantized DC levels.
    #[cfg(feature = "std")]
    pub(crate) fn dbg_qdcs(&self) -> String {
        let v: Vec<String> = self.win.txb_q.iter().map(|q| q[0].to_string()).collect();
        v.join(",")
    }

    /// NSQDBG only: the winner's whole-block depth-0 luma prediction.
    #[cfg(feature = "std")]
    pub(crate) fn dbg_pred(&self) -> &[u8] {
        &self.win.pred
    }

    /// The quad-dist gate recon planes (see the `gate_y` field doc).
    pub(crate) fn gate_y(&self) -> &[u8] {
        &self.gate_y
    }

    pub(crate) fn gate_uv(&self) -> (&[u8], &[u8]) {
        (&self.gate_u, &self.gate_v)
    }

    /// bd10 (task #94, root #2): the 10-bit twin of [`gate_y`](Self::gate_y) for
    /// the NSQ recon-dist gate. C's `calc_scr_to_recon_dist_per_quadrant`
    /// (product_coding_loop.c:8065) reads `cand_bf->recon` through
    /// `svt_full_distortion_kernel16_bits` at `hbd_md`, i.e. the 10-bit recon —
    /// while `gate_y` is the MSB-truncated u8 proxy. At bypass_encdec=0
    /// (preset <= 3) `cand_bf->recon` is the winner's final (winning-depth)
    /// recon, whose bd10 twin is exactly `win_recon10`. Empty on the u8 path.
    pub(crate) fn win_recon10(&self) -> &[u16] {
        &self.win_recon10
    }

    /// bd10 twin of [`gate_uv`](Self::gate_uv) — the winner's 10-bit chroma
    /// recon (chroma has no tx-depth split, so the winner recon is unambiguous).
    /// Empty unless the bd10 chroma full loop ran.
    pub(crate) fn win_uv_recon10(&self) -> (&[u16], &[u16]) {
        (&self.win_u_recon10, &self.win_v_recon10)
    }

    /// The shared MDS3 residual-workspace state (C `cand_bf->residual`,
    /// consumed by the psq gate): the LAST MDS3 candidate's depth-0
    /// residual.
    pub(crate) fn psq_resid(&self) -> &[i32] {
        &self.psq_resid
    }

    /// The bd10 twin of [`psq_resid`](Self::psq_resid): the last MDS3
    /// candidate's depth-0 residual at TRUE 10 bits. Empty on the u8 path.
    pub(crate) fn psq_resid10(&self) -> &[i32] {
        &self.psq_resid10
    }

    /// Winner luma recon (w x h raster).
    ///
    /// Kept alongside the used `psq_resid`/`psq_resid10` family so the
    /// winner's buffers have a uniform read surface; no live caller today.
    #[allow(dead_code)]
    pub(crate) fn y_recon(&self) -> &[u8] {
        &self.win.y_recon
    }

    /// The walk/entropy-pass view of the winner.
    ///
    /// Takes `self` and MOVES the winner's seven owned buffers into the
    /// `LeafChoice` rather than cloning them. They were cloned only because
    /// this used to run BEFORE `commit_leaf`, which borrows the `LeafEval`;
    /// `commit_leaf` reads none of `to_choice`'s output and `to_choice` reads
    /// none of `commit_leaf`'s side effects, so both callers simply commit
    /// first and convert second. Same bytes, seven fewer allocate+memcpy+free
    /// round trips per coded block.
    pub(crate) fn into_choice(self) -> LeafChoice {
        let cand = self.win;
        LeafChoice {
            mode: cand.mode,
            angle_delta: cand.delta,
            fi_mode: cand.fi,
            uv_mode: cand.uv,
            uv_angle_delta: cand.uv_delta,
            tx_depth: cand.tx_depth,
            txb_qcoeffs: cand.txb_q,
            txb_eobs: cand.txb_eob,
            txb_tx_types: cand.txb_type,
            u_qcoeffs: cand.u_q,
            v_qcoeffs: cand.v_q,
            u_eob: cand.u_eob,
            v_eob: cand.v_eob,
            u_recon: cand.u_recon,
            v_recon: cand.v_recon,
            cfl_alpha_idx: cand.cfl_alpha_idx,
            cfl_alpha_signs: cand.cfl_alpha_signs,
            palette: cand.palette,
            ibc: cand.ibc,
            inter: cand.inter,
        }
    }
}

/// Where this leaf is, what shape it is, and the neighbour-derived contexts
/// that go with it -- C `blk_geom` plus
/// `svt_aom_coding_loop_context_generation`.
///
/// Derived once at the top of a leaf evaluation and unchanged for its
/// lifetime, which is why it is a named value rather than two dozen locals
/// sharing one enormous scope.
///
/// Fields are added when a reader exists, never before -- `skip_ctx`,
/// `blk_crop` and `aligned_dims` each arrived with the stage that reads them.
#[derive(Clone, Copy)]
pub(super) struct LeafGeom {
    /// Luma block dims.
    pub(super) w: usize,
    pub(super) h: usize,
    /// Luma origin in the (aligned) frame.
    pub(super) abs_x: usize,
    pub(super) abs_y: usize,
    /// C `is_chroma_reference` (common_utils.h:315): sub-8 blocks carry chroma
    /// only at odd mi in the sub-8 dimension.
    pub(super) has_uv: bool,
    /// Prediction geometry for the luma unit: availability tables and the
    /// frame-edge clamps, taken against the ALIGNED extent (C
    /// `mb_to_right_edge`/`mb_to_bottom_edge`, NOT the recon buffer's shape --
    /// see the issue #15 defect 2 note in CLAUDE.md).
    pub(super) y_geom: UnitGeom,
    /// C `get_filt_type` for luma: the above/left coded modes' smoothness.
    pub(super) filt_type_y: i32,
    /// C `BLOCK_SIZE` index for this block's dims.
    pub(super) bsize_idx: usize,
    /// C `is_cfl_allowed` (both dims <= 32), as the 0/1 index the rate tables
    /// want.
    pub(super) cfl_allowed: usize,
    /// Angle deltas are only signalled at >= 8x8 (C `av1_use_angle_delta`).
    pub(super) use_angle: bool,
    /// C `svt_aom_filter_intra_allowed_bsize` (both dims <= 32).
    pub(super) fi_allowed_bsize: bool,
    /// Neighbour-derived intra-mode contexts.
    pub(super) above_ctx: usize,
    pub(super) left_ctx: usize,
    /// C `ctx->is_inter_ctx` — `svt_av1_get_intra_inter_context` over the
    /// neighbours' `is_inter_block`. Read only on a NON-I-slice, where an
    /// intra candidate pays the `is_inter = 0` flag; 0 (C's own initial
    /// value) on a key frame, where no such symbol exists.
    pub(super) is_inter_ctx: usize,
    /// Real skip-coeff context, or 0 when the config does not price it.
    pub(super) skip_ctx: usize,
    /// Spatial-distortion crop for the whole-block luma txb (C
    /// `cropped_tx_width`/`_height`). The identity off a straddling block.
    pub(super) blk_crop: (usize, usize),
    /// The ALIGNED frame extent, as the spatial-distortion crops are taken
    /// against it. NOT the recon buffer's shape -- that mistake was issue #15
    /// defect 2.
    pub(super) aligned_dims: crate::frame_geom::FrameDims,
}

/// The 10-bit state a leaf evaluation carries, or the inert shape of it.
///
/// `active` is C's bd10 mode funnel: when the bd10 recon canvas is present the
/// MDS0 mode decision is made at TRUE 10 bits rather than on the MSB-truncated
/// u8 recon. When it is false NONE of the bd10 branches run and every path is
/// byte-identical to the 8-bit encoder.
///
/// A borrowing VIEW: the buffers live in the leaf evaluation itself (later
/// stages read them directly), so this names the set without owning it.
#[derive(Clone, Copy)]
pub(super) struct LeafBd10<'a> {
    /// The bd10 LUMA mode funnel is on for this leaf.
    pub(super) active: bool,
    /// Block-local 10-bit luma source (real u16 samples when the caller
    /// supplied a native HBD source, else the same `u8 << shift` widening).
    /// Empty when `active` is false.
    pub(super) blk_y_src10: &'a [u16],
    /// C `fast_lambda_md[1]` -- the MDS0 fast-cost lambda.
    pub(super) lambda_fast: u64,
    /// The MDS1/MDS3 inputs at true depth. `None` on every u8 path AND on a
    /// bd10 leaf where only the MDS0 funnel is enabled.
    pub(super) rd: &'a Option<Bd10Rd>,
}

/// The palette-flag rates for this leaf.
///
/// Per-leaf constants (they depend on the block size and the neighbour palette
/// grid, not on any candidate), read by both candidate injection and MDS3.
#[derive(Clone, Copy)]
pub(super) struct PalFlagRates {
    /// C `svt_aom_allow_palette` on the LUMA bsize.
    pub(super) allow: bool,
    /// C `svt_aom_get_palette_mode_ctx` (rd_cost.c:583): the above+left count
    /// of palette-coded neighbours, 0..=2. Zero until a palette candidate wins
    /// a neighbour, so non-screen content is byte-identical.
    pub(super) mode_ctx: usize,
    /// Cost of the "no luma palette" flag.
    pub(super) y_no: u64,
    /// Cost of the "no chroma palette" flag, `use_palette_y = 0` row.
    pub(super) uv_no: u64,
    /// The same flag on the `use_palette_y = 1` row -- what a luma-palette
    /// candidate pays (rd_cost.c:518-520).
    pub(super) uv_no_y1: u64,
}
