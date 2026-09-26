//! C's per-block MD motion-search driver — the loop over
//! `ctx->ref_frame_type_arr` that `product_coding_loop.c:9425-9447` runs
//! before `generate_md_stage_0_cand`.
//!
//! ```text
//! build_single_ref_mvp_array(pcs, ctx);   // :9429, gated
//! read_refine_me_mvs(pcs, ctx, pc_tree);  // :9431
//! ... reset pme_res ...                   // :9433-9438
//! perform_md_reference_pruning(pcs, ctx); // :9441, gated
//! pme_search(pcs, ctx, input_pic);        // :9445, gated on updated_enable_pme
//! ```
//!
//! # Why this is a module and not three calls in `inter_md_arm`
//!
//! Every leaf here was already ported with no caller
//! ([`crate::port_md::md_search`]'s `build_single_ref_mvp_list`,
//! `best_mvp_by_distortion`, `refine_me_mv_for_ref`, `pme_search_for_ref`,
//! `md_subpel_search`, `md_nsq_motion_search`, plus
//! [`crate::md_subpel`]'s two tree searches). What was missing is the
//! LOOP: each iteration needs a different reference picture, a different
//! MVP stack, a different MVP list and its own `mv_cost_params`. That
//! loop is this file; `inter_md_arm` keeps the candidate half.
//!
//! # Why it is atomic with the reference set
//!
//! MEASURED 2026-09-02 (`docs/INTER-ENCODE-PLAN.md` §1z¹⁴). On
//! `gradient 64x64 q40 p8` frame 1, C's `me_candidate_array` for the coded
//! 64x64 is `[dir=1, dir=2]` — a LIST-1 unipred and a BI_PRED, and no
//! list-0 entry at all. `inject_new_candidates` can therefore only produce
//! a BWDREF NEWMV, `inject_mvp_candidates_ii` never produces a NEWMV, so
//! **the LAST_FRAME NEWMV that C codes on that cell exists only because
//! `inject_pme_candidates` ran.** Widening `ref_frame_type_arr` without
//! this driver hands MD a BWDREF candidate and no LAST one; that was
//! implemented and measured at `inter_byte_gate` 23 of 36 FAILING.
//!
//! # Evidence tier
//!
//! Tier 4 for the driver (`read_refine_me_mvs`, `pme_search` and
//! `build_single_ref_mvp_array` are all `static` with no exported symbol),
//! on top of tier-1 leaves: `svt_av1_find_best_sub_pixel_tree_pruned`,
//! `svt_aom_choose_best_av1_mv_pred`, `svt_pme_sad_loop_kernel`,
//! `svt_aom_fp_mv_err_cost` and `clip_mv_on_pic_boundary` are exported and
//! driven by `c_parity_*` tests rather than re-transcribed here.
//!
//! The per-block JOIN against C is `SVT_SUBPEL_OUT`
//! (`tools/capture_c_trace/wrap_recon.c`), which fires INSIDE
//! `svt_av1_find_best_sub_pixel_tree_pruned` — once per
//! `(block, list_idx, ref_idx, search_stage)` — and prints C's
//! `mvp_array` / `fp_me_mv` / `fp_me_dist` / start and result MVs at that
//! instant. **Do not use `SVT_INJCFG_OUT`'s `PMEST` line for those
//! fields**: it reads them at neighbour-array-update time, which is after
//! the whole depth has been searched, so they belong to whatever block MD
//! processed last.

use crate::inter_mvp::{InterMvpStack, av1_set_ref_frame, get_list_idx, get_ref_frame_idx};
use crate::picture::PaddedRef;
use crate::port_enc_mode_config::encdec::{MdSubPelSearchCtrls, subpel_search_method};
use crate::port_md::md_search::{
    DistortionType, FullPelCtx, MdPmeCtrls, MdSubpelCtrls, PlaneDistortion, RefPicGeom, RefineMeIn,
    SubpelBlockGeom, best_mvp_by_distortion, build_single_ref_mvp_list, md_subpel_search,
    md_subpel_search_fixed_stage, pme_search_for_ref, refine_me_mv_for_ref,
};
use crate::port_md::pme::{MvCostParams, MvCostTable};
use crate::port_md::predicates::{
    InterCandGroup, MAX_NUM_OF_REF_PIC_LIST, TOT_INTER_GROUP, is_valid_unipred_ref,
};
use alloc::vec::Vec;
use svtav1_types::motion::Mv;

/// C `REF_LIST_MAX_DEPTH`.
pub const REF_LIST_MAX_DEPTH: usize = 4;

/// The frame-constant halves of C's `ModeDecisionContext` that the two
/// searches read.
///
/// Every field here is a picture-level signal: the four control structs come
/// from `svt_aom_sig_deriv_enc_dec_default`, which takes no per-SB input.
///
/// **The MD LAMBDAS ARE NOT AMONG THEM AND USED TO BE.** C sets
/// `full_lambda_md[EB_8_BIT_MD]` / `fast_lambda_md[EB_8_BIT_MD]` in
/// `svt_aom_mode_decision_configure_sb` (md_process.c:796) from that
/// superblock's `svt_aom_get_me_qindex`, so they vary by superblock even
/// with no per-SB delta-q signalled — `update_lambda`'s
/// `stats_based_sb_lambda_modulation` block keys on `me_q_index -
/// base_q_idx` (rc_process.c:423-446). They live on
/// [`BlockSearchIn`] for that reason; this struct carrying them was the
/// defect `docs/INTER-ENCODE-PLAN.md` 1z24 fixed.
pub struct SearchFrameCfg {
    /// C `ctx->md_pme_ctrls`.
    pub md_pme: MdPmeCtrls,
    /// C `ctx->md_pme_ctrls.dist_type` — carried separately because the
    /// ported [`MdPmeCtrls`] holds only the fields the PREDICATES read.
    pub pme_dist_type: DistortionType,
    /// C's qp-modulated `full_pel_search_{width,height}`
    /// (product_coding_loop.c:3203-3211) — applied once at the frame level
    /// because `svt_aom_get_qp_based_th_scaling_factors` reads
    /// `static_config.qp`, which is frame-constant.
    pub pme_full_pel_w: u8,
    pub pme_full_pel_h: u8,
    /// C `ctx->md_subpel_me_ctrls`.
    pub md_subpel_me: crate::port_enc_mode_config::encdec::MdSubPelSearchCtrls,
    /// C `ctx->md_subpel_pme_ctrls`.
    pub md_subpel_pme: crate::port_enc_mode_config::encdec::MdSubPelSearchCtrls,
    /// C `ctx->ifs_ctrls.level == IFS_MDS0` — whether the interpolation
    /// filter is DECIDED at MDS0 and therefore priced there.
    ///
    /// It is FALSE at every preset this port reaches:
    /// `pcs->interpolation_search_level` is 2 (`IFS_MDS1`) at MR and 4
    /// (`IFS_MDS3`) above it, never 1. MEASURED 2026-09-02 against C's own
    /// `svt_aom_inter_fast_cost` (`SVT_IFCOST_OUT`): the port hard-coded it
    /// TRUE and paid `get_switchable_rate` at MDS0 where C pays nothing,
    /// 20 to 109 rate units on every inter candidate.
    pub ifs_at_mds0: bool,
    /// `ctx->ifs_ctrls.level` (`set_interpolation_search_level_ctrls`,
    /// enc_mode_config.c:4069-4092) — the MD stage the interpolation-filter
    /// search runs at. On the video ladder (`:9083-9098`) a non-negative
    /// preset yields 4 (`IFS_MDS3`) or, above M8 on a non-base picture with
    /// a high `ref_skip_percentage`, 0 (`IFS_OFF`); 2 needs `ENC_MR`, which
    /// is -1. Consumed by `leaf_funnel::ifs::ifs_at_mds3`.
    pub ifs_level: crate::port_enc_mode_config::ctrls::IfsLevel,
    /// C `ctx->md_nsq_me_ctrls.enabled`.
    pub md_nsq_me_enabled: bool,
    /// C `ctx->md_nsq_me_ctrls`' search parameters — `md_nsq_motion_search`
    /// reads all of them, not just `enabled`.
    pub md_nsq_me_dist: DistortionType,
    pub md_nsq_full_pel_w: u8,
    pub md_nsq_full_pel_h: u8,
    pub md_nsq_enable_psad: bool,
    /// C `ctx->ref_pruning_ctrls` — the whole row, not just `enabled`:
    /// `perform_md_reference_pruning` reads `max_dev_to_best`,
    /// `check_closest_multiplier` and `closest_refs` too.
    pub ref_pruning: crate::port_enc_mode_config::ctrls::RefPruningControls,
    /// C `ctx->md_pme_ctrls.enabled` — the FIRST half of
    /// `ctx->updated_enable_pme` (product_coding_loop.c:9418). The second
    /// assignment (:9419-9422) zeroes it per block when
    /// `is_intra_bordered && use_neighbouring_mode_ctrls.enabled`, which is
    /// per-BLOCK state this frame-level cfg cannot hold — the caller
    /// resolves it into [`BlockSearchIn::updated_enable_pme`].
    pub md_pme_enabled: bool,
    /// C `frm_hdr->quantization_params.base_q_idx`.
    pub base_q_idx: u8,
    /// C `svt_aom_get_sad_per_bit(base_q_idx, 0)`.
    pub sad_per_bit: i32,
    pub allow_high_precision_mv: bool,
    /// C `ctx->shut_fast_rate`.
    pub shut_fast_rate: bool,
    /// C `ctx->approx_inter_rate`.
    pub approx_inter_rate: u8,
    /// C `pcs->ppcs->enhanced_pic->width` / `height` — the PME early
    /// MVP-vs-ME check's resolution term.
    pub pic_width: u32,
    pub pic_height: u32,
    /// C `scs->static_config.qp` — the CLI qp, carried through so the
    /// block-level `merge_inter_cands` threshold (mode_decision.c:3640)
    /// can read it where `frame_cfg` already consumed it.
    pub cli_qp: u32,
    /// C `ppcs->picture_qp` — read by `perform_md_reference_pruning`'s
    /// `check_closest` threshold (product_coding_loop.c:3053); inert at
    /// pruning levels where `check_closest_multiplier` is 0.
    pub picture_qp: u8,
}

/// The per-block inputs, all of which the caller already has.
pub struct BlockSearchIn<'a> {
    /// C `ctx->full_lambda_md[EB_8_BIT_MD]` for THIS superblock, as
    /// `svt_aom_mode_decision_configure_sb` set it (md_process.c:796 ->
    /// `av1_lambda_assign_md`). Per-SUPERBLOCK, not per-frame: see
    /// [`SearchFrameCfg`]'s header.
    pub full_lambda_8bit: u32,
    /// C `ctx->fast_lambda_md[EB_8_BIT_MD]` for this superblock.
    pub fast_lambda_8bit: u32,
    pub org_x: usize,
    pub org_y: usize,
    pub bw: usize,
    pub bh: usize,
    pub bsize: u8,
    /// C `ctx->blk_geom->sq_size`.
    pub sq_size: u16,
    pub mi_rows: i32,
    pub mi_cols: i32,
    /// C `pcs->ppcs->enhanced_pic` luma and its stride.
    pub src: &'a [u8],
    pub src_stride: usize,
    /// C `ctx->ref_frame_type_arr[0..tot_ref_frame_types]`.
    pub ref_frame_type_arr: &'a [i8],
    /// The padded DPB picture per `MvReferenceFrame` (index 1..=7).
    pub padded_by_ref: &'a [Option<&'a PaddedRef>; 8],
    /// C `ctx->ref_mv_stack[frame_type]`, one per `MvReferenceFrame`.
    pub stacks: &'a [InterMvpStack],
    /// C `blk_ptr->av1xd->ref_mv_count[frame_type]`
    /// (`MODE_CTX_REF_FRAMES` entries — compound types index past 7).
    pub ref_mv_count: &'a [u8],
    /// C `md_rate_est_ctx->nmv_vec_cost` + `nmvcoststack`.
    pub nmv: &'a MvCostTable,
    /// C `md_rate_est_ctx->drl_mode_fac_bits`.
    pub drl_mode_fac_bits: &'a [[i32; 2]; crate::port_md::drl::DRL_MODE_CONTEXTS],
    /// The same tables in the shape `md_subpel` wants.
    pub search_tables: &'a crate::intrabc::MvCostTables,
    /// This frame's open-loop ME.
    pub me: &'a crate::inter_me_arm::FrameMe,
    /// C `ctx->sq_sb_me_mv` + `pc_tree->tested_blk[PART_N][0]`, carried
    /// ACROSS blocks by the caller — see [`SqMeState`]. `None` means the
    /// caller has no square-parent state to offer, which makes every block
    /// take C's `me_mv_array` seed (what the port did before this existed).
    pub sq_me: Option<SqMeState>,
    /// C `ctx->updated_enable_pme` (product_coding_loop.c:9418-9422) —
    /// `md_pme_ctrls.enabled` zeroed per block when
    /// `is_intra_bordered && use_neighbouring_mode_ctrls.enabled`. Since
    /// `ctx->is_intra_bordered` is itself the `enabled`-gated product
    /// (:9417), the resolved value is `md_pme_enabled && !is_intra_bordered`.
    /// Per-BLOCK, which is why it lives here and not on
    /// [`SearchFrameCfg`].
    pub updated_enable_pme: bool,
}

/// C `ctx->sq_sb_me_mv` and `pc_tree->tested_blk[PART_N][0]`, which are the
/// two things an NSQ block's ME needs and neither of which is per-block.
///
/// C keeps ONE `sq_sb_me_mv[list][ref]` on the mode-decision context and
/// overwrites it at the end of every `read_refine_me_mvs` whose `ctx->shape`
/// is `PART_N` (product_coding_loop.c:2932-2934). Because MD walks a node's
/// shapes with the square FIRST, the value an NSQ shape reads is its own
/// square parent's — so a single slot IS the faithful structure, not an
/// approximation of a per-node one. `tested` is `pc_tree->tested_blk[PART_N][0]`:
/// false until any square has been searched, which is exactly when C falls
/// through to the `me_mv_array` seed.
#[derive(Clone, Copy, Debug, Default)]
pub struct SqMeState {
    /// C `ctx->sq_sb_me_mv[list][ref]`, EIGHTH-pel.
    pub sq_sb_me_mv: [[Mv; REF_LIST_MAX_DEPTH]; 2],
    /// `(org_x, org_y, size)` of the SQUARE whose MV [`Self::sq_sb_me_mv`]
    /// holds — `None` before any square has been searched.
    ///
    /// This is how `pc_tree->tested_blk[PART_N][0]` is answered without a
    /// node chain. C asks "was the square at MY node tested"; an NSQ shape's
    /// node is the square of side `max(bwidth, bheight)` containing it, and
    /// nodes are size-aligned, so the question is `sq_block == (org_x &
    /// !(s-1), org_y & !(s-1), s)`. A bare boolean would have answered "was
    /// ANY square tested", which is a different and weaker claim — and one
    /// that silently reads another node's MV if a node's square shape is ever
    /// skipped.
    pub sq_block: Option<(usize, usize, usize)>,
}

impl SqMeState {
    /// C `pc_tree->tested_blk[PART_N][0]` for the block at
    /// `(org_x, org_y)` of `bw x bh` — see [`Self::sq_block`].
    #[must_use]
    pub fn tested_for(&self, org_x: usize, org_y: usize, bw: usize, bh: usize) -> bool {
        let s = bw.max(bh);
        debug_assert!(
            s.is_power_of_two(),
            "a partition node's side is a power of two"
        );
        self.sq_block == Some((org_x & !(s - 1), org_y & !(s - 1), s))
    }

    /// C `if (ctx->shape == PART_N) ctx->sq_sb_me_mv = ctx->sb_me_mv`
    /// (product_coding_loop.c:2932-2934).
    pub fn record_square(
        &mut self,
        org_x: usize,
        org_y: usize,
        size: usize,
        mv: [[Mv; REF_LIST_MAX_DEPTH]; 2],
    ) {
        self.sq_sb_me_mv = mv;
        self.sq_block = Some((org_x, org_y, size));
    }
}

/// C `BLOCK_64X128` / `BLOCK_128X64` (`BlockSize`), the two shapes C excludes
/// from the square-MV inheritance because their second half and the 128x128
/// do not share an `me_results` entry (product_coding_loop.c:2858-2860).
const BLOCK_64X128: u8 = 13;
const BLOCK_128X64: u8 = 14;

/// What the driver leaves for `inject_inter_candidates`.
#[derive(Debug, Clone)]
pub struct BlockSearchOut {
    /// C `ctx->sb_me_mv[list][ref]`.
    pub sb_me_mv: [[Mv; REF_LIST_MAX_DEPTH]; 2],
    /// C `ctx->post_subpel_me_mv_cost[list][ref]`.
    pub post_subpel_me_mv_cost: [[u32; REF_LIST_MAX_DEPTH]; 2],
    /// C `ctx->valid_pme_mv[list][ref]`.
    pub valid_pme_mv: [[bool; REF_LIST_MAX_DEPTH]; 2],
    /// C `ctx->best_pme_mv[list][ref]`.
    pub best_pme_mv: [[Mv; REF_LIST_MAX_DEPTH]; 2],
    /// C `ctx->pme_res[list][ref].dist`. `u32::MAX` where `pme_search`
    /// never wrote the pair — the `~0` reset C leaves in place for every
    /// `continue`d entry (product_coding_loop.c:9434-9438).
    pub pme_dist: [[u32; REF_LIST_MAX_DEPTH]; 2],
    /// Which of C's four exits produced each entry, `None` where
    /// `pme_search` never looked at that pair.
    ///
    /// Not a C field — C distinguishes the exits only by control flow. It is
    /// here for the same reason [`crate::port_md::md_search::PmeExit`]
    /// exists: `valid_pme_mv = 1` says nothing about whether a SEARCH
    /// happened, so without it a test cannot tell "PME ran" from "PME handed
    /// back the ME MV", and the positive control below would pass with the
    /// search deleted.
    pub pme_exit: [[Option<crate::port_md::md_search::PmeExit>; REF_LIST_MAX_DEPTH]; 2],
    /// True when this block was a SQUARE shape, i.e. C would have executed
    /// `if (ctx->shape == PART_N) ctx->sq_sb_me_mv = ctx->sb_me_mv`. The
    /// caller stores [`Self::sb_me_mv`] into its [`SqMeState`] when set —
    /// the write is the caller's because the STATE is the caller's.
    pub is_square_shape: bool,
    /// C `ctx->ref_pruning_ctrls` + `ctx->ref_filtering_res` folded into the
    /// consumer's shape: what `perform_md_reference_pruning`
    /// (product_coding_loop.c:3004-3084) wrote for this block. `enabled:
    /// false` when the controls are off, which makes every
    /// `is_valid_{uni,bi}pred_ref` consult permissive — C's own behaviour
    /// (mode_decision.c:762-774, :793-813).
    pub ref_pruning: crate::port_md::predicates::RefPruningState,
}

impl Default for BlockSearchOut {
    fn default() -> Self {
        Self {
            sb_me_mv: [[Mv::ZERO; REF_LIST_MAX_DEPTH]; 2],
            // C's `(int32_t)~0` initialisation.
            post_subpel_me_mv_cost: [[u32::MAX; REF_LIST_MAX_DEPTH]; 2],
            valid_pme_mv: [[false; REF_LIST_MAX_DEPTH]; 2],
            best_pme_mv: [[Mv::ZERO; REF_LIST_MAX_DEPTH]; 2],
            pme_dist: [[u32::MAX; REF_LIST_MAX_DEPTH]; 2],
            pme_exit: [[None; REF_LIST_MAX_DEPTH]; 2],
            is_square_shape: false,
            ref_pruning: crate::port_md::predicates::RefPruningState::default(),
        }
    }
}

impl BlockSearchOut {
    /// C `ctx->md_me_dist` — the min over every reference's
    /// `post_subpel_me_mv_cost` (product_coding_loop.c:2796-2798 /
    /// :2898-2900); `u32::MAX` when no subpel ME ran.
    pub fn md_me_dist(&self) -> u32 {
        self.post_subpel_me_mv_cost
            .iter()
            .flatten()
            .copied()
            .min()
            .unwrap_or(u32::MAX)
    }
    /// C `ctx->md_pme_dist` — the min over `pme_res[..].dist`
    /// (product_coding_loop.c:3365-3371); `u32::MAX` when PME ran no
    /// reference.
    pub fn md_pme_dist(&self) -> u32 {
        self.pme_dist
            .iter()
            .flatten()
            .copied()
            .min()
            .unwrap_or(u32::MAX)
    }
}

/// The per-reference state C keeps in `ModeDecisionContext` between the
/// three passes.
#[derive(Clone, Default)]
struct RefState {
    mvps: Vec<Mv>,
    best_fp_mvp_idx: usize,
    best_fp_mvp_dist: u32,
    fp_me_mv: Mv,
    sub_me_mv: Mv,
    fp_me_dist: u32,
    post_subpel_me_mv_cost: u32,
    me_data_present: bool,
}

fn ref_geom(p: &PaddedRef) -> RefPicGeom {
    RefPicGeom {
        border: p.y.border as i32,
        max_width: p.y.width as i32,
        max_height: p.y.height as i32,
        y_stride: p.y.stride,
    }
}

/// The `md_subpel_search_fixed_stage` control view of an
/// [`MdSubPelSearchCtrls`] row — same fields, the fixed-stage function's
/// own struct. C passes the ME controls unconditionally:
/// `md_subpel_search_fixed_stage` reads `ctx->md_subpel_me_ctrls` even at
/// the `pme_search` call site (product_coding_loop.c:3351-3357), so the
/// PME closure below converts the ME row too.
fn fixed_stage_ctrls(c: &MdSubPelSearchCtrls) -> MdSubpelCtrls {
    MdSubpelCtrls {
        enabled: c.enabled != 0,
        max_precision: c.max_precision,
        abs_th_mult: u32::from(c.abs_th_mult),
        pred_variance_th: c.pred_variance_th.max(0) as u32,
        bias_fp: c.bias_fp.max(0) as u16,
        min_blk_sz: u16::from(c.min_blk_sz),
        fixed_stage: c.subpel_search_method == subpel_search_method::SUBPEL_FIXED_STAGE_SEARCH,
        subpel_iters_per_step: c.subpel_iters_per_step.max(0) as u8,
        skip_diag_refinement: c.skip_diag_refinement,
    }
}

/// C `perform_md_reference_pruning` (product_coding_loop.c:3004-3084,
/// `static`).
///
/// Ranks each single reference by `min(fp_me_dist, best_fp_mvp_dist)` —
/// the cheapest full-pel evidence the block has for that ref — and writes
/// `ref_filtering_res[group][list][ref].do_ref` per candidate group. A ref
/// whose distortion deviates from the best by more than
/// `max_dev_to_best[group]` percent loses its `do_ref`, which is how C
/// keeps LAST2 out of `inject_pme_candidates`/`inject_new_candidates` on a
/// block where its early evidence is far worse than LAST's.
///
/// The TPL arm — `use_tpl_info_offset` feeding `offset_tab` off
/// `get_sb_tpl_inter_stats` — reads `ppcs->tpl_ctrls.enable`, which is 0 on
/// every frame this driver reaches (flat low-delay runs no TPL lookahead),
/// so `offset_tab` here is all-zero. The offset is nonzero only at pruning
/// levels 4..=8, and a port that gains those levels must port
/// `get_sb_tpl_inter_stats` in the same change — `use_tpl_info` set from a
/// stubbed table would be a silent wrong answer.
fn perform_md_reference_pruning(
    cfg: &SearchFrameCfg,
    b: &BlockSearchIn<'_>,
    st: &[[RefState; REF_LIST_MAX_DEPTH]; 2],
    out: &mut BlockSearchOut,
) {
    const N: usize = MAX_NUM_OF_REF_PIC_LIST * REF_LIST_MAX_DEPTH;
    // C `svt_memset(early_inter_distortion_array, 0xFE, ...)` — a large
    // not-`~0` sentinel every unsearched slot keeps, which then flows
    // through `dev_to_the_best` like any other entry.
    let mut early = [0xFEFEFEFEu32; N];
    let mut min_dist = u32::MAX;
    for &pair in b.ref_frame_type_arr {
        let rf = av1_set_ref_frame(pair);
        if rf[1] != crate::inter_mvp::NONE_FRAME {
            continue;
        }
        let (li, ri) = (get_list_idx(rf[0]), get_ref_frame_idx(rf[0]));
        if ri >= REF_LIST_MAX_DEPTH {
            continue;
        }
        // `pa_me_distortion` is `fp_me_dist` when this block had ME data for
        // the ref and `(uint32_t)~0` — "any non zero value" — otherwise.
        let pa_me = if st[li][ri].me_data_present {
            st[li][ri].fp_me_dist
        } else {
            u32::MAX
        };
        early[li * REF_LIST_MAX_DEPTH + ri] = pa_me.min(st[li][ri].best_fp_mvp_dist);
        min_dist = min_dist.min(early[li * REF_LIST_MAX_DEPTH + ri]);
    }
    let th = (u32::from(cfg.ref_pruning.check_closest_multiplier)
        .saturating_mul((b.bw * b.bh) as u32)
        .saturating_mul(u32::from(cfg.picture_qp)))
        / 24;
    if cfg.ref_pruning.check_closest_multiplier != 0
        && early[0] < th
        && early[REF_LIST_MAX_DEPTH] < th
    {
        for g in 0..TOT_INTER_GROUP {
            for l in 0..MAX_NUM_OF_REF_PIC_LIST {
                for r in 0..REF_LIST_MAX_DEPTH {
                    if r == 0 || cfg.ref_pruning.max_dev_to_best[g] == u32::MAX {
                        out.ref_pruning.do_ref[g][l][r] = true;
                    }
                }
            }
        }
    } else {
        // C sorts nothing — `dev_to_the_best` is just the per-slot
        // percent-deviation from the best, and the `n - 1` bound leaves the
        // last slot (list1 ref3) at its 0 initialiser. Kept verbatim.
        let mut dev_to_the_best = [0u32; N];
        let denom = i64::from(min_dist.max(1));
        for (i, e) in early.iter().enumerate().take(N - 1) {
            dev_to_the_best[i] =
                (((i64::from((*e).max(1)) - i64::from(min_dist.max(1))) * 100) / denom) as u32;
        }
        for g in 0..TOT_INTER_GROUP {
            for l in 0..MAX_NUM_OF_REF_PIC_LIST {
                for r in 0..REF_LIST_MAX_DEPTH {
                    // `offset` is `offset_tab[li][ri]` — all-zero without
                    // the TPL arm, so the `offset == ~0` arm is unreachable
                    // and the threshold is `max_dev_to_best` itself.
                    let pruning_th = if cfg.ref_pruning.max_dev_to_best[g] == 0 {
                        0
                    } else if cfg.ref_pruning.max_dev_to_best[g] == u32::MAX {
                        u32::MAX
                    } else {
                        cfg.ref_pruning.max_dev_to_best[g]
                    };
                    if dev_to_the_best[l * REF_LIST_MAX_DEPTH + r] < pruning_th {
                        out.ref_pruning.do_ref[g][l][r] = true;
                    }
                }
            }
        }
    }
    out.ref_pruning.enabled = true;
    out.ref_pruning.closest_refs = cfg.ref_pruning.closest_refs.map(|c| c != 0);
}

/// C's `product_coding_loop.c:9425-9447` block, single-reference arm.
///
/// The compound entries of `ref_frame_type_arr` are skipped exactly as C
/// skips them: all three functions guard on `rf[1] == NONE_FRAME`.
#[must_use]
pub fn run_block_searches(cfg: &SearchFrameCfg, b: &BlockSearchIn<'_>) -> BlockSearchOut {
    let mut out = BlockSearchOut::default();
    // C `blk_geom->bwidth != blk_geom->bheight` (product_coding_loop.c:2830)
    // and `ctx->shape == PART_N` (:2932). A square-DIMENSION block is the
    // PART_N shape in this funnel, so the two spellings coincide.
    let b_w_ne_h = b.bw != b.bh;
    out.is_square_shape = !b_w_ne_h;
    // C `ctx->sq_sb_me_mv` + `pc_tree->tested_blk[PART_N][0]`. A caller that
    // offers none behaves exactly as this function did before `SqMeState`
    // existed: `tested = false` makes `me_mv_center` take the
    // `me_mv_array` arm for every shape.
    let sq_state = b.sq_me.unwrap_or_default();
    let sq_tested = sq_state.tested_for(b.org_x, b.org_y, b.bw, b.bh);
    let mut st: [[RefState; REF_LIST_MAX_DEPTH]; 2] = Default::default();

    let input_origin_index = b.org_y * b.src_stride + b.org_x;
    let mi_row = (b.org_y / 4) as i32;
    let mi_col = (b.org_x / 4) as i32;
    let bsize = svtav1_types::block::BlockSize::from_u8(b.bsize)
        .expect("an inter block always has a real BlockSize");

    // C `build_single_ref_mvp_array`'s gate (product_coding_loop.c:9425).
    let build_mvps = (cfg.md_subpel_me.enabled != 0
        && cfg.md_subpel_me.subpel_search_method
            == crate::port_enc_mode_config::encdec::subpel_search_method::SUBPEL_TREE_PRUNED
        && cfg.md_subpel_me.mvp_th != 0)
        || b.updated_enable_pme
        || cfg.ref_pruning.enabled != 0;

    for &pair in b.ref_frame_type_arr {
        let rf = av1_set_ref_frame(pair);
        if rf[1] != crate::inter_mvp::NONE_FRAME {
            continue;
        }
        let (li, ri) = (get_list_idx(rf[0]), get_ref_frame_idx(rf[0]));
        if ri >= REF_LIST_MAX_DEPTH {
            continue;
        }
        let Some(p) = b.padded_by_ref[rf[0].max(0) as usize] else {
            continue;
        };
        let r = ref_geom(p);
        let mut dist = PlaneDistortion {
            src: b.src,
            src_stride: b.src_stride,
            ref_plane: &p.y.buf,
            ref_org: p.y.origin,
            ref_stride: p.y.stride,
            bwidth: b.bw,
            bheight: b.bh,
        };
        let s = &mut st[li][ri];
        s.me_data_present = b.me.me_data_present(b.org_x, b.org_y, b.bsize, li, ri);
        if build_mvps {
            s.mvps = build_single_ref_mvp_list(
                cfg.shut_fast_rate,
                &stack_this_mvs(&b.stacks[rf[0].max(0) as usize]),
                b.ref_mv_count[rf[0].max(0) as usize],
                b.org_x as i32,
                b.org_y as i32,
                b.bw as i32,
                b.bh as i32,
                &r,
            );
            let (idx, d) = best_mvp_by_distortion(
                &s.mvps,
                &mut dist,
                b.org_x as i32,
                b.org_y as i32,
                r.y_stride,
                input_origin_index,
            );
            s.best_fp_mvp_idx = idx;
            s.best_fp_mvp_dist = d;
        }
    }

    // ---- read_refine_me_mvs (product_coding_loop.c:2815-2936) ----
    for &pair in b.ref_frame_type_arr {
        let rf = av1_set_ref_frame(pair);
        if rf[1] != crate::inter_mvp::NONE_FRAME {
            continue;
        }
        let (li, ri) = (get_list_idx(rf[0]), get_ref_frame_idx(rf[0]));
        if ri >= REF_LIST_MAX_DEPTH || !st[li][ri].me_data_present {
            continue;
        }
        let Some(p) = b.padded_by_ref[rf[0].max(0) as usize] else {
            continue;
        };
        let r = ref_geom(p);
        let Some(raw) = b.me.mv_for(b.org_x, b.org_y, b.bsize, li, ri, b.me.max_l0) else {
            continue;
        };

        // C `read_refine_me_mvs` picks the ME seed from THREE arms
        // (product_coding_loop.c:2856-2871) and this port has ONE
        // transcription of that choice — `md_search::me_mv_center`, which
        // `refine_me_mv_for_ref` calls below. C sets `ctx->ref_mv` from
        // `choose_best_av1_mv_pred(NEWMV, me_mv)` on the CLIPPED seed, before
        // any search, so the caller needs the same value here — and it gets
        // it by calling the SAME function, not by re-deriving it.
        // `docs/WORKING-ON-THIS.md` §4: a second transcription of a function
        // that already exists is how this campaign lost a lambda.
        let centre = seed_me_centre(b, &r, sq_state, sq_tested, li, ri, raw);
        let ref_mv = choose_pred_mv(b, cfg.shut_fast_rate, cfg.approx_inter_rate, rf[0], centre);
        // C's `md_full_pel_search` builds `mv_cost_params` PER CALL with
        // `rdmult = dist_type != SAD ? full_lambda : fast_lambda`
        // (product_coding_loop.c:1936-1941), and the only full-pel search
        // `read_refine_me_mvs` reaches is `md_nsq_motion_search`'s — which
        // runs at `md_nsq_me_ctrls.dist_type` (VAR at every enabled level).
        // Building the params as SAD here prices the NSQ search's MV error
        // cost off fast_lambda instead of full_lambda — measured at `diag
        // 72x72 q55 p8` poc3: the (64,0) 16x32 ladder picked (80,-544)
        // where C picks (64,-544).
        let mvcp = full_pel_mv_cost_params(cfg, b, ref_mv, cfg.md_nsq_me_dist);

        let fp_ctx = FullPelCtx {
            blk_org_x: b.org_x as i32,
            blk_org_y: b.org_y as i32,
            bwidth: b.bw as i32,
            bheight: b.bh as i32,
            // C `ctx->enable_psad = ctx->md_nsq_me_ctrls.enable_psad`
            // (product_coding_loop.c:2101). Inert while every enabled level
            // is VAR — the large-LBD dispatch needs SAD.
            enable_psad: cfg.md_nsq_enable_psad,
            hbd_md: false,
            sprs_lev0_start_x: 0,
            sprs_lev0_end_x: 0,
            sprs_lev0_start_y: 0,
            sprs_lev0_end_y: 0,
        };
        let mut dist = PlaneDistortion {
            src: b.src,
            src_stride: b.src_stride,
            ref_plane: &p.y.buf,
            ref_org: p.y.origin,
            ref_stride: p.y.stride,
            bwidth: b.bw,
            bheight: b.bh,
        };

        let geom = subpel_geom(b, mi_row, mi_col);
        let mut sub_ctx = crate::md_subpel::SubpelMdContext {
            pd_pass: 1,
            mvp_th: i32::from(cfg.md_subpel_me.mvp_th),
            hp_mv_th: cfg.md_subpel_me.hp_mv_th,
            best_fp_mvp_dist: st[li][ri].best_fp_mvp_dist,
            best_fp_mvp: st[li][ri]
                .mvps
                .get(st[li][ri].best_fp_mvp_idx)
                .copied()
                .unwrap_or(Mv::ZERO),
            fp_me_dist: 0,
            final_distortion: 0,
        };
        let mut subpel = |mv: &mut Mv| -> u32 {
            let fpme_mv = *mv;
            let start_mv = Mv {
                x: (mv.x >> 3).wrapping_mul(8),
                y: (mv.y >> 3).wrapping_mul(8),
            };
            // C's call-site dispatch (product_coding_loop.c:2891-2898):
            // `subpel_search_method == SUBPEL_FIXED_STAGE_SEARCH` runs
            // `md_subpel_search_fixed_stage`, a variance-only ladder that
            // takes NO mv_cost_params — everything else goes through
            // `md_subpel_search`.
            let err = if cfg.md_subpel_me.subpel_search_method
                == subpel_search_method::SUBPEL_FIXED_STAGE_SEARCH
            {
                let mut fsd = PlaneDistortion {
                    src: b.src,
                    src_stride: b.src_stride,
                    ref_plane: &p.y.buf,
                    ref_org: p.y.origin,
                    ref_stride: p.y.stride,
                    bwidth: b.bw,
                    bheight: b.bh,
                };
                md_subpel_search_fixed_stage(
                    &fixed_stage_ctrls(&cfg.md_subpel_me),
                    &mut fsd,
                    b.org_x as i32,
                    b.org_y as i32,
                    b.bw as u32,
                    b.bh as u32,
                    u32::from(crate::md_subpel::NUM_PELS_LOG2_LOOKUP[bsize as usize]),
                    p.y.stride,
                    input_origin_index,
                    mv,
                )
            } else {
                md_subpel_search(
                    crate::md_subpel::SPEL_ME,
                    &cfg.md_subpel_me,
                    geom,
                    bsize,
                    li,
                    ri,
                    cfg.allow_high_precision_mv,
                    ref_mv,
                    usize::from(cfg.base_q_idx),
                    b.full_lambda_8bit,
                    cfg.md_subpel_me.skip_diag_refinement,
                    Some(b.search_tables),
                    b.src,
                    input_origin_index,
                    b.src_stride,
                    &p.y.buf,
                    (p.y.origin + b.org_y * p.y.stride + b.org_x) as i64,
                    p.y.stride,
                    Some(&mut sub_ctx),
                    mv,
                )
            };
            // `SVTAV1_SUBPEL`: the port-side twin of the C interposer's
            // `SVT_SUBPEL_OUT` (wrap_recon.c's `__wrap_svt_av1_find_best_
            // sub_pixel_tree_pruned`), same field order so the two dumps join
            // line for line. `fpme` is the just-computed fp result (== `start`
            // pre-truncation); `subme`/`pscost` are the PREVIOUS block's stale
            // `ctx` values, matching C's read-before-write timing.
            #[cfg(feature = "std")]
            if crate::dbgenv::subpeldbg() {
                let s = &st[li][ri];
                std::eprint!(
                    "SUBPEL stage=0 org=({},{}) bsize={} bw={} bh={} sq={} li={} ri={}\
                     start=({},{}) best=({},{}) err={} dist={} refmv=({},{})\
                     epb={} spb={} mct={} flam={} fastlam={}\
                     fpme=({},{}) subme=({},{}) fpdist={} pscost={}\
                     mvpn={} bestidx={} bestdist={} mvp=",
                    b.org_x,
                    b.org_y,
                    b.bsize,
                    b.bw,
                    b.bh,
                    geom.sq_size,
                    li,
                    ri,
                    start_mv.y,
                    start_mv.x,
                    mv.y,
                    mv.x,
                    err,
                    sub_ctx.final_distortion,
                    ref_mv.y,
                    ref_mv.x,
                    ((b.full_lambda_8bit >> crate::intrabc::RD_EPB_SHIFT).max(1)),
                    cfg.sad_per_bit,
                    if cfg.md_subpel_me.skip_diag_refinement >= 3 {
                        4
                    } else {
                        0
                    },
                    b.full_lambda_8bit,
                    b.fast_lambda_8bit,
                    fpme_mv.y,
                    fpme_mv.x,
                    s.sub_me_mv.y,
                    s.sub_me_mv.x,
                    sub_ctx.fp_me_dist,
                    s.post_subpel_me_mv_cost,
                    s.mvps.len(),
                    s.best_fp_mvp_idx,
                    s.best_fp_mvp_dist,
                );
                for (k, m) in s.mvps.iter().enumerate() {
                    std::eprint!("{}({},{})", if k > 0 { "," } else { "" }, m.y, m.x);
                }
                std::eprintln!();
            }
            err
        };
        // C's no-subpel arm: the full-pel ME cost, computed only when
        // somebody downstream needs it.
        let mut fp_dist = |mv: Mv| -> u32 {
            let idx = (b.org_x as i32 + (i32::from(mv.x) >> 3))
                + (b.org_y as i32 + (i32::from(mv.y) >> 3)) * r.y_stride as i32;
            let mut d2 = PlaneDistortion {
                src: b.src,
                src_stride: b.src_stride,
                ref_plane: &p.y.buf,
                ref_org: p.y.origin,
                ref_stride: p.y.stride,
                bwidth: b.bw,
                bheight: b.bh,
            };
            use crate::port_md::md_search::DistortionSource as _;
            let v = d2.variance(idx, input_origin_index);
            let full = full_pel_mv_cost_params(cfg, b, ref_mv, DistortionType::Var);
            v.wrapping_add(crate::port_md::pme::fp_mv_err_cost(mv, &full) as u32)
        };

        let do_subpel = cfg.md_subpel_me.enabled != 0;
        // C `md_nsq_motion_search`'s own arguments. Built here and not inside
        // `refine_me_mv_for_ref` because the MVC list is ME-TABLE machinery —
        // `pu_search_index_map` over this block's b64 — which that module
        // deliberately takes from its caller.
        let nsq_mvcs = if b_w_ne_h && cfg.md_nsq_me_enabled {
            nsq_sub_block_mvs(b, li, ri)
        } else {
            Vec::new()
        };
        let res = refine_me_mv_for_ref(
            RefineMeIn {
                blk_avail_sqi: sq_tested,
                bsize_is_64x128_or_128x64: b.bsize == BLOCK_64X128 || b.bsize == BLOCK_128X64,
                bsize_is_4x4: b.bw == 4 && b.bh == 4,
                // See `seed_me_centre`: this port keeps one `SqMeState` slot,
                // not a node chain, so C's 4x4 arm has no counterpart. It is
                // unreachable at the presets measured (`shapes_for_size`
                // returns N_ONLY at size 4) and unported, not proven inert.
                parent_tested: false,
                sq_sb_me_mv: sq_state.sq_sb_me_mv[li][ri],
                raw_me_mv_full_pel: raw,
                md_nsq_me_enabled: cfg.md_nsq_me_enabled,
                do_subpel,
                subpel_fixed_stage: cfg.md_subpel_me.subpel_search_method
                    == subpel_search_method::SUBPEL_FIXED_STAGE_SEARCH,
                needs_fp_me_dist: b.updated_enable_pme || cfg.ref_pruning.enabled != 0,
                shape_is_part_n: !b_w_ne_h,
            },
            &fp_ctx,
            &r,
            if b_w_ne_h && cfg.md_nsq_me_enabled {
                Some((
                    cfg.md_nsq_me_dist,
                    cfg.md_nsq_full_pel_w,
                    cfg.md_nsq_full_pel_h,
                    nsq_mvcs.as_slice(),
                ))
            } else {
                None
            },
            if do_subpel { Some(&mut subpel) } else { None },
            if do_subpel { None } else { Some(&mut fp_dist) },
            &mut dist,
            &mvcp,
            input_origin_index,
        );

        let s = &mut st[li][ri];
        s.fp_me_mv = res.fp_me_mv;
        s.sub_me_mv = res.sub_me_mv;
        s.post_subpel_me_mv_cost = res.post_subpel_me_mv_cost;
        // C `svt_av1_find_best_sub_pixel_tree_pruned` writes `fp_me_dist`
        // from INSIDE the subpel search when `search_stage == SPEL_ME`
        // (mcomp.c:616-618); the no-subpel arm writes it here instead.
        s.fp_me_dist = if do_subpel {
            sub_ctx.fp_me_dist
        } else {
            res.fp_me_dist.unwrap_or(u32::MAX)
        };
        out.sb_me_mv[li][ri] = res.sb_me_mv;
        out.post_subpel_me_mv_cost[li][ri] = res.post_subpel_me_mv_cost;
    }

    // ---- perform_md_reference_pruning (product_coding_loop.c:3004-3084) ----
    // Runs AFTER `read_refine_me_mvs` and BEFORE `pme_search`
    // (product_coding_loop.c:9441/9445): the PME loop below consults its
    // `do_ref` per (list, ref), and every uni/bipred injector consults it
    // through `InjectCtx::ref_pruning`.
    if cfg.ref_pruning.enabled != 0 {
        perform_md_reference_pruning(cfg, b, &st, &mut out);
    }

    if !b.updated_enable_pme {
        return out;
    }

    // ---- pme_search (product_coding_loop.c:3197-3372) ----
    for &pair in b.ref_frame_type_arr {
        let rf = av1_set_ref_frame(pair);
        if rf[1] != crate::inter_mvp::NONE_FRAME {
            continue;
        }
        let (li, ri) = (get_list_idx(rf[0]), get_ref_frame_idx(rf[0]));
        if ri >= REF_LIST_MAX_DEPTH {
            continue;
        }
        let Some(p) = b.padded_by_ref[rf[0].max(0) as usize] else {
            continue;
        };
        // C `!svt_aom_is_valid_unipred_ref(ctx, PRED_ME_GROUP, list_idx,
        // ref_idx) -> continue` (product_coding_loop.c:3238-3241): a ref the
        // distance-based pruner dropped keeps `valid_pme_mv` at 0, which is
        // also what keeps `inject_pme_candidates`' compound arm from pairing
        // it into a NEW_NEWMV.
        if !is_valid_unipred_ref(&out.ref_pruning, InterCandGroup::PredMe, li, ri) {
            continue;
        }
        let r = ref_geom(p);
        let s = st[li][ri].clone();
        if s.mvps.is_empty() {
            continue;
        }
        let best_mvp = s.mvps[s.best_fp_mvp_idx];
        let ref_mv = choose_pred_mv(
            b,
            cfg.shut_fast_rate,
            cfg.approx_inter_rate,
            rf[0],
            best_mvp,
        );
        let mvcp = full_pel_mv_cost_params(cfg, b, ref_mv, cfg.pme_dist_type);

        let fp_ctx = FullPelCtx {
            blk_org_x: b.org_x as i32,
            blk_org_y: b.org_y as i32,
            bwidth: b.bw as i32,
            bheight: b.bh as i32,
            enable_psad: false,
            hbd_md: false,
            sprs_lev0_start_x: 0,
            sprs_lev0_end_x: 0,
            sprs_lev0_start_y: 0,
            sprs_lev0_end_y: 0,
        };
        let mut dist = PlaneDistortion {
            src: b.src,
            src_stride: b.src_stride,
            ref_plane: &p.y.buf,
            ref_org: p.y.origin,
            ref_stride: p.y.stride,
            bwidth: b.bw,
            bheight: b.bh,
        };
        let geom = subpel_geom(b, mi_row, mi_col);
        let mut sub_ctx = crate::md_subpel::SubpelMdContext {
            pd_pass: 1,
            mvp_th: i32::from(cfg.md_subpel_pme.mvp_th),
            hp_mv_th: cfg.md_subpel_pme.hp_mv_th,
            best_fp_mvp_dist: s.best_fp_mvp_dist,
            best_fp_mvp: best_mvp,
            fp_me_dist: 0,
            final_distortion: 0,
        };
        let mut subpel = |mv: &mut Mv| -> u32 {
            let fpme_mv = *mv;
            let start_mv = Mv {
                x: (mv.x >> 3).wrapping_mul(8),
                y: (mv.y >> 3).wrapping_mul(8),
            };
            // C's PME call site dispatches on the ME controls' method, not
            // the PME controls' — and `md_subpel_search_fixed_stage` reads
            // `ctx->md_subpel_me_ctrls` internally either way
            // (product_coding_loop.c:3351-3357).
            let err = if cfg.md_subpel_me.subpel_search_method
                == subpel_search_method::SUBPEL_FIXED_STAGE_SEARCH
            {
                let mut fsd = PlaneDistortion {
                    src: b.src,
                    src_stride: b.src_stride,
                    ref_plane: &p.y.buf,
                    ref_org: p.y.origin,
                    ref_stride: p.y.stride,
                    bwidth: b.bw,
                    bheight: b.bh,
                };
                md_subpel_search_fixed_stage(
                    &fixed_stage_ctrls(&cfg.md_subpel_me),
                    &mut fsd,
                    b.org_x as i32,
                    b.org_y as i32,
                    b.bw as u32,
                    b.bh as u32,
                    u32::from(crate::md_subpel::NUM_PELS_LOG2_LOOKUP[bsize as usize]),
                    p.y.stride,
                    input_origin_index,
                    mv,
                )
            } else {
                md_subpel_search(
                    crate::md_subpel::SPEL_PME,
                    &cfg.md_subpel_pme,
                    geom,
                    bsize,
                    li,
                    ri,
                    cfg.allow_high_precision_mv,
                    ref_mv,
                    usize::from(cfg.base_q_idx),
                    b.full_lambda_8bit,
                    // C reads the ME controls' `skip_diag_refinement` even on a
                    // PME call (`svt_init_mv_cost_params`, :1906).
                    cfg.md_subpel_me.skip_diag_refinement,
                    Some(b.search_tables),
                    b.src,
                    input_origin_index,
                    b.src_stride,
                    &p.y.buf,
                    (p.y.origin + b.org_y * p.y.stride + b.org_x) as i64,
                    p.y.stride,
                    Some(&mut sub_ctx),
                    mv,
                )
            };
            #[cfg(feature = "std")]
            if crate::dbgenv::subpeldbg() {
                std::eprint!(
                    "SUBPEL stage=1 org=({},{}) bsize={} bw={} bh={} sq={} li={} ri={}\
                     start=({},{}) best=({},{}) err={} dist={} refmv=({},{})\
                     epb={} spb={} mct={} flam={} fastlam={}\
                     fpme=({},{}) subme=({},{}) fpdist={} pscost={}\
                     mvpn={} bestidx={} bestdist={} mvp=",
                    b.org_x,
                    b.org_y,
                    b.bsize,
                    b.bw,
                    b.bh,
                    geom.sq_size,
                    li,
                    ri,
                    start_mv.y,
                    start_mv.x,
                    mv.y,
                    mv.x,
                    err,
                    sub_ctx.final_distortion,
                    ref_mv.y,
                    ref_mv.x,
                    ((b.full_lambda_8bit >> crate::intrabc::RD_EPB_SHIFT).max(1)),
                    cfg.sad_per_bit,
                    if cfg.md_subpel_me.skip_diag_refinement >= 3 {
                        4
                    } else {
                        0
                    },
                    b.full_lambda_8bit,
                    b.fast_lambda_8bit,
                    fpme_mv.y,
                    fpme_mv.x,
                    s.sub_me_mv.y,
                    s.sub_me_mv.x,
                    sub_ctx.fp_me_dist,
                    s.post_subpel_me_mv_cost,
                    s.mvps.len(),
                    s.best_fp_mvp_idx,
                    s.best_fp_mvp_dist,
                );
                for (k, m) in s.mvps.iter().enumerate() {
                    std::eprint!("{}({},{})", if k > 0 { "," } else { "" }, m.y, m.x);
                }
                std::eprintln!();
            }
            err
        };

        let res = pme_search_for_ref(
            &cfg.md_pme,
            &fp_ctx,
            &r,
            &mut dist,
            &mvcp,
            input_origin_index,
            cfg.pme_dist_type,
            /* skipped */ false,
            s.me_data_present,
            s.fp_me_mv,
            s.sub_me_mv,
            s.fp_me_dist,
            s.post_subpel_me_mv_cost,
            &s.mvps,
            cfg.pme_full_pel_w,
            cfg.pme_full_pel_h,
            cfg.pic_width,
            cfg.pic_height,
            if cfg.md_subpel_pme.enabled != 0 {
                Some(&mut subpel)
            } else {
                None
            },
        );
        out.valid_pme_mv[li][ri] = res.valid;
        out.best_pme_mv[li][ri] = res.best_pme_mv;
        out.pme_dist[li][ri] = res.dist;
        out.pme_exit[li][ri] = Some(res.exit);

        #[cfg(feature = "std")]
        if crate::dbgenv::canddbg() && crate::depth_refine::nsqdbg_here(b.org_x, b.org_y) {
            std::eprintln!(
                "PMEDBG org=({},{}) {}x{} li={li} ri={ri} mvpn={} bestidx={} bestdist={} \
                 fpme=({},{}) subme=({},{}) fpdist={} pscost={} exit={:?} pme=({},{}) valid={} \
                 flam={} fastlam={} epb={}",
                b.org_x,
                b.org_y,
                b.bw,
                b.bh,
                s.mvps.len(),
                s.best_fp_mvp_idx,
                s.best_fp_mvp_dist,
                s.fp_me_mv.y,
                s.fp_me_mv.x,
                s.sub_me_mv.y,
                s.sub_me_mv.x,
                s.fp_me_dist,
                s.post_subpel_me_mv_cost,
                res.exit,
                res.best_pme_mv.y,
                res.best_pme_mv.x,
                res.valid,
                b.full_lambda_8bit,
                b.fast_lambda_8bit,
                (b.full_lambda_8bit >> 6).max(1),
            );
        }
    }
    out
}

fn stack_this_mvs(stack: &InterMvpStack) -> Vec<Mv> {
    stack.stack.iter().map(|c| c.this_mv).collect()
}

/// The per-SB values `read_refine_me_mvs_light_pd1` reads where the regular
/// `read_refine_me_mvs` reads picture-level ones —
/// `svt_aom_sig_deriv_enc_dec_light_pd1_default`'s output
/// (enc_mode_config.c:7378-7580), narrowed to what the search consumes.
pub struct LightSearchSig<'a> {
    /// `ctx->md_subpel_me_ctrls` — `sig.md_subpel_me`. The light ladder
    /// resolves `me_subpel_level` to 7..10 whenever it is nonzero
    /// (enc_mode_config.c:7467-7490), i.e. `SUBPEL_FIXED_STAGE_SEARCH` on
    /// every SB this lane reaches in the supported envelope; the tree arm
    /// below exists for the levels the RTC ladder adds (:7656-7705).
    pub md_subpel_me: &'a MdSubPelSearchCtrls,
    /// `ctx->is_intra_bordered` — the caller applies C's
    /// `use_neighbouring_mode_ctrls.enabled ? is_intra_bordered(ctx) : 0`
    /// gate (product_coding_loop.c:9115); this is the POST-gate value.
    pub is_intra_bordered: bool,
    /// `ctx->cand_reduction_ctrls.use_neighbouring_mode_ctrls.enabled` —
    /// `sig.cand_reduction`, the light lane's own cand-reduction level.
    pub use_neighbouring_mode_enabled: bool,
    /// `ctx->shut_fast_rate` — `sig.shut_fast_rate`, hardcoded false in C's
    /// light derivation (enc_mode_config.c:7565); carried so the
    /// `no_mv_stack` arm is the transcription rather than a constant.
    pub shut_fast_rate: bool,
    /// `ctx->approx_inter_rate` — `sig.approx_inter_rate`, C's
    /// `MAX(1, pcs->approx_inter_rate)` (:7544).
    pub approx_inter_rate: u8,
}

/// C `read_refine_me_mvs_light_pd1` (product_coding_loop.c:2737-2812) — the
/// light lane's ENTIRE block search. Compared to the regular
/// [`run_block_searches`]:
///
/// * The seed is `me_mv_array` times 8, UNCLIPPED — the light path clips
///   `sb_me_mv` only, AFTER the search (:2808-2811), and never consults
///   `sq_sb_me_mv`/`pc_tree`.
/// * `skip_subpel` (:2771-2776) suppresses the search on an intra-bordered
///   block under neighbouring-mode controls, and on `sq_size <= min_blk_sz`.
/// * `ctx->ref_mv` is `0` under `shut_fast_rate`, else
///   `choose_best_av1_mv_pred(NEWMV, me_mv)` — and is written ONLY when the
///   subpel search runs.
/// * There is no `build_single_ref_mvp_array` (`mvp_count` is memset to 0 at
///   :9119), no `md_nsq_motion_search`/`md_sq_motion_search`, no `fp_me_mv`,
///   no `pme_search`, and no `perform_md_reference_pruning` — the
///   corresponding [`BlockSearchOut`] fields stay at their `~0`/empty
///   defaults, which is exactly what C's never-written ctx state reads as on
///   this lane (`md_pme_dist` keeps `~0`, so the light
///   `generate_md_stage_0_cand_light_pd1`'s `md_me_dist`-only DC check and
///   the light injectors' absence of PME reads are both served).
/// * `post_subpel_me_mv_cost[list][ref]` is written ONLY where the search
///   ran (:2793-2798) — the `u32::MAX` default stands in for the rest.
pub fn run_block_searches_light(
    cfg: &SearchFrameCfg,
    b: &BlockSearchIn<'_>,
    sig: &LightSearchSig<'_>,
) -> BlockSearchOut {
    let mut out = BlockSearchOut::default();
    // `is_square_shape` stays false: the `sq_sb_me_mv` store lives in the
    // regular `read_refine_me_mvs` (:2932-2934), not this function.

    let input_origin_index = b.org_y * b.src_stride + b.org_x;
    let mi_row = (b.org_y / 4) as i32;
    let mi_col = (b.org_x / 4) as i32;
    let bsize = svtav1_types::block::BlockSize::from_u8(b.bsize)
        .expect("an inter block always has a real BlockSize");
    let subpel_me = sig.md_subpel_me;

    for &pair in b.ref_frame_type_arr {
        let rf = av1_set_ref_frame(pair);
        if rf[1] != crate::inter_mvp::NONE_FRAME {
            continue;
        }
        let (li, ri) = (get_list_idx(rf[0]), get_ref_frame_idx(rf[0]));
        if ri >= REF_LIST_MAX_DEPTH {
            continue;
        }
        if !b.me.me_data_present(b.org_x, b.org_y, b.bsize, li, ri) {
            continue;
        }
        let Some(p) = b.padded_by_ref[rf[0].max(0) as usize] else {
            continue;
        };
        let r = ref_geom(p);
        let Some(raw) = b.me.mv_for(b.org_x, b.org_y, b.bsize, li, ri, b.me.max_l0) else {
            continue;
        };
        // C `me_mv = {{mv_cand.x * 8, mv_cand.y * 8}}` (:2759-2760) — the
        // light path's ONLY seed; no square-parent override, no pre-clip.
        let mut me_mv = Mv {
            x: raw.x.wrapping_mul(8),
            y: raw.y.wrapping_mul(8),
        };

        // :2771-2776 — "can only skip if using dc only b/c otherwise need
        // cost at candidate generation": intra-bordered under neighbouring-
        // mode controls, or a block at/below `min_blk_sz`, skips the search.
        let skip_subpel = (sig.is_intra_bordered && sig.use_neighbouring_mode_enabled)
            || b.sq_size <= u16::from(subpel_me.min_blk_sz);

        if subpel_me.enabled != 0 && !skip_subpel {
            // :2779-2787 — `ctx->ref_mv`: 0 under `no_mv_stack`
            // (`shut_fast_rate`), else the NEWMV best-pred MV.
            let ref_mv = if sig.shut_fast_rate {
                Mv::ZERO
            } else {
                choose_pred_mv(b, sig.shut_fast_rate, sig.approx_inter_rate, rf[0], me_mv)
            };
            // `mvp_count` is memset to 0 on this lane (:9119), so
            // `best_fp_mvp*` holds what C's never-written ctx state does for
            // a light-only frame — zero. The `mvp_th` arm of the tree
            // methods is unreachable in the supported envelope (see
            // [`LightSearchSig::md_subpel_me`]); were it reached, C's read
            // of `mvp_array`/`best_fp_mvp_dist` would be of STALE
            // cross-block state this port deliberately does not model.
            let mut sub_ctx = crate::md_subpel::SubpelMdContext {
                pd_pass: 1,
                mvp_th: i32::from(subpel_me.mvp_th),
                hp_mv_th: subpel_me.hp_mv_th,
                best_fp_mvp_dist: 0,
                best_fp_mvp: Mv::ZERO,
                fp_me_dist: 0,
                final_distortion: 0,
            };
            let err = if subpel_me.subpel_search_method
                == subpel_search_method::SUBPEL_FIXED_STAGE_SEARCH
            {
                let mut fsd = PlaneDistortion {
                    src: b.src,
                    src_stride: b.src_stride,
                    ref_plane: &p.y.buf,
                    ref_org: p.y.origin,
                    ref_stride: p.y.stride,
                    bwidth: b.bw,
                    bheight: b.bh,
                };
                md_subpel_search_fixed_stage(
                    &fixed_stage_ctrls(subpel_me),
                    &mut fsd,
                    b.org_x as i32,
                    b.org_y as i32,
                    b.bw as u32,
                    b.bh as u32,
                    u32::from(crate::md_subpel::NUM_PELS_LOG2_LOOKUP[bsize as usize]),
                    p.y.stride,
                    input_origin_index,
                    &mut me_mv,
                )
            } else {
                md_subpel_search(
                    crate::md_subpel::SPEL_ME,
                    subpel_me,
                    subpel_geom(b, mi_row, mi_col),
                    bsize,
                    li,
                    ri,
                    cfg.allow_high_precision_mv,
                    ref_mv,
                    usize::from(cfg.base_q_idx),
                    b.full_lambda_8bit,
                    subpel_me.skip_diag_refinement,
                    Some(b.search_tables),
                    b.src,
                    input_origin_index,
                    b.src_stride,
                    &p.y.buf,
                    (p.y.origin + b.org_y * p.y.stride + b.org_x) as i64,
                    p.y.stride,
                    Some(&mut sub_ctx),
                    &mut me_mv,
                )
            };
            out.post_subpel_me_mv_cost[li][ri] = err;
        }
        // :2806-2811 — `sb_me_mv` takes the (possibly refined) MV, then the
        // clip lands on the STORED value, not the search state.
        out.sb_me_mv[li][ri] = me_mv;
        crate::port_md::coding_loop::clip_mv_on_pic_boundary(
            b.org_x as i32,
            b.org_y as i32,
            b.bw as i32,
            b.bh as i32,
            r.max_width,
            r.max_height,
            r.border,
            &mut out.sb_me_mv[li][ri].x,
            &mut out.sb_me_mv[li][ri].y,
        );
    }
    out
}

fn subpel_geom(b: &BlockSearchIn<'_>, mi_row: i32, mi_col: i32) -> SubpelBlockGeom {
    SubpelBlockGeom {
        mi_row,
        mi_col,
        mi_width: (b.bw / 4) as i32,
        mi_height: (b.bh / 4) as i32,
        mi_rows: b.mi_rows,
        mi_cols: b.mi_cols,
        bwidth: b.bw,
        bheight: b.bh,
        sq_size: b.sq_size,
    }
}

/// C's ME centre after `clip_mv_on_pic_boundary`
/// (product_coding_loop.c:2870-2871), which runs BEFORE
/// `choose_best_av1_mv_pred`.
#[allow(clippy::too_many_arguments)]
fn seed_me_centre(
    b: &BlockSearchIn<'_>,
    r: &RefPicGeom,
    sq: SqMeState,
    sq_tested: bool,
    li: usize,
    ri: usize,
    raw_full_pel: Mv,
) -> Mv {
    let mut mv = crate::port_md::md_search::me_mv_center(
        sq_tested,
        b.bw as u16,
        b.bh as u16,
        b.bsize == BLOCK_64X128 || b.bsize == BLOCK_128X64,
        b.bw == 4 && b.bh == 4,
        // C `pc_tree->parent->tested_blk[PART_N][0]`, the 4x4 arm. This port
        // keeps ONE `SqMeState` slot, not a node chain, so it cannot answer
        // "was the PARENT node's square tested" separately from "was ANY
        // square tested". At the presets this campaign measures the arm is
        // unreachable — `shapes_for_size` returns `N_ONLY` at size 4, so the
        // funnel never evaluates a 4x4 leaf — but that is a reachability
        // argument, not an implementation, and it is said out loud here
        // rather than hidden behind a `false`.
        false,
        sq.sq_sb_me_mv[li][ri],
        raw_full_pel,
    );
    crate::port_md::coding_loop::clip_mv_on_pic_boundary(
        b.org_x as i32,
        b.org_y as i32,
        b.bw as i32,
        b.bh as i32,
        r.max_width,
        r.max_height,
        r.border,
        &mut mv.x,
        &mut mv.y,
    );
    mv
}

/// C `md_nsq_motion_search`'s MVC pass (product_coding_loop.c:2096-2134) —
/// the SUB-BLOCK MV list, geometry- and ME-presence-filtered.
///
/// C walks every square PU slot of this block's own b64 and keeps the ones
/// that (a) have a side equal to `MIN(bwidth, bheight)`, (b) sit inside this
/// block's rectangle, and (c) have ME data for `(list, ref)`. The whole pass
/// is gated on `(bwidth != 4 && bheight != 4) && sq_size >= 16`.
///
/// The MVs come back in FULL PEL times 8, C's own units, in C's `block_index`
/// order — the order is load-bearing, because `nsq_mvc_list` truncates at
/// `MAX_MD_NSQ_SEARCH_MVC_CNT` and a different order keeps a different set.
fn nsq_sub_block_mvs(b: &BlockSearchIn<'_>, li: usize, ri: usize) -> Vec<Mv> {
    let mut out = Vec::new();
    if (b.bw == 4 || b.bh == 4) || b.sq_size < 16 {
        return out;
    }
    let (b64_x, b64_y) = (b.org_x / 64, b.org_y / 64);
    // C `blk_geom_offset` — the block's origin relative to its own b64
    // (`blk_org_x - sb_origin_x - geom_offset_x`; the two subtractions
    // together are exactly the offset inside the b64, which is what a 128-wide
    // superblock's `geom_offset_x` exists to remove).
    let (off_x, off_y) = ((b.org_x % 64) as u32, (b.org_y % 64) as u32);
    let min_size = b.bw.min(b.bh) as u32;
    let n = crate::inter_me_arm::number_of_pus(b.me.enable_me_8x8, b.me.enable_me_16x16);
    for idx in 0..n {
        let (px, py, pw, ph) = crate::inter_me_arm::pu_geometry(idx);
        if (min_size != pw && min_size != ph)
            || px < off_x
            || px >= off_x + b.bw as u32
            || py < off_y
            || py >= off_y + b.bh as u32
            || !b.me.me_data_present_at_pu(b64_x, b64_y, idx, li, ri)
        {
            continue;
        }
        let Some(mv) = b.me.mv_at_pu(b64_x, b64_y, idx, li, ri) else {
            continue;
        };
        out.push(Mv {
            x: mv.x.wrapping_mul(8),
            y: mv.y.wrapping_mul(8),
        });
    }
    out
}

/// C `svt_aom_choose_best_av1_mv_pred(ctx, ref_pair, NEWMV, mv, 0, ...)`
/// -> `ctx->ref_mv` (the `best_pred_mv[0]` it writes).
///
/// `shut_fast_rate`/`approx_inter_rate` are `ctx->shut_fast_rate` /
/// `ctx->approx_inter_rate` — picture-level on the regular lane, the
/// per-SB light-PD1 signal's on the light lane (`sig_deriv_enc_dec_
/// light_pd1_default` writes `approx_inter_rate = MAX(1, pcs->..)`).
fn choose_pred_mv(
    b: &BlockSearchIn<'_>,
    shut_fast_rate: bool,
    approx_inter_rate: u8,
    frame_type: i8,
    mv: Mv,
) -> Mv {
    let mut drl_index = 0u8;
    let mut pred = [Mv::ZERO; 2];
    crate::port_md::drl::choose_best_av1_mv_pred(
        &crate::port_md::drl::ChooseDrlCtx {
            shut_fast_rate,
            approx_inter_rate,
            ref_mv_stack: &b.stacks[frame_type.max(0) as usize].stack,
            ref_mv_count: b.ref_mv_count[frame_type.max(0) as usize],
            nmv_cost: b.nmv,
            drl_mode_fac_bits: b.drl_mode_fac_bits,
        },
        svtav1_types::prediction::PredictionMode::NewMv,
        mv,
        Mv::ZERO,
        &mut drl_index,
        &mut pred,
    );
    pred[0]
}

/// C `svt_init_mv_cost_params` restricted to the members
/// `md_full_pel_search` reads, with C's own lambda choice
/// (`dist_type != SAD ? full_lambda : fast_lambda`, :1920-1922).
fn full_pel_mv_cost_params<'a>(
    cfg: &SearchFrameCfg,
    b: &'a BlockSearchIn<'a>,
    ref_mv: Mv,
    dist_type: DistortionType,
) -> MvCostParams<'a> {
    let rdmult = if dist_type == DistortionType::Sad {
        b.fast_lambda_8bit
    } else {
        b.full_lambda_8bit
    };
    crate::port_md::pme::init_mv_cost_params(
        ref_mv,
        b.sq_size,
        cfg.md_subpel_me.skip_diag_refinement,
        rdmult,
        Some(b.nmv),
    )
}

/// The PICTURE-level signals the frame configuration is derived from —
/// the four `enc_mode_config.c` levels plus the lambdas and the qp.
pub struct SearchFrameInputs {
    /// `pcs->md_pme_level`
    pub md_pme_level: u8,
    /// `pcs->me_subpel_level`
    pub me_subpel_level: u8,
    /// `pcs->pme_subpel_level`
    pub pme_subpel_level: u8,
    /// `pcs->md_nsq_mv_search_level`
    pub md_nsq_mv_search_level: u8,
    /// `pcs->interpolation_search_level`
    pub interpolation_search_level: u8,
    /// `pcs->dist_based_ref_pruning`
    pub dist_based_ref_pruning: u8,
    /// `scs->static_config.qp` — the CLI qp the PME search-area scaling
    /// reads (NOT `base_q_idx`).
    pub cli_qp: u32,
    /// `ppcs->picture_qp` — read by `perform_md_reference_pruning`'s
    /// `check_closest` threshold (product_coding_loop.c:3053). Only live at
    /// pruning levels 4..=8 (`check_closest_multiplier == 1`); at level 2
    /// the multiplier is 0 and the value is never read.
    pub picture_qp: u8,
    /// `scs->qp_based_th_scaling_ctrls.pme_qp_based_th_scaling`
    /// (`enc_handle.c:3812`: 1 on the `_default` arm above `ENC_MR`).
    pub pme_qp_based_th_scaling: bool,
    pub base_q_idx: u8,
    pub allow_high_precision_mv: bool,
    /// `ctx->approx_inter_rate`
    pub approx_inter_rate: u8,
    pub pic_width: u32,
    pub pic_height: u32,
}

/// Resolve C's four control rows and the qp-modulated PME extents once per
/// frame.
///
/// `None` when any level is outside the range its C table accepts (where C
/// would `assert(0)`), which is the same contract
/// `port_enc_mode_config::encdec::sig_deriv_enc_dec_default` has.
#[must_use]
pub fn frame_cfg(i: &SearchFrameInputs) -> Option<SearchFrameCfg> {
    use crate::port_enc_mode_config::ctrls;
    use crate::port_enc_mode_config::encdec;

    let pme = ctrls::md_pme_search_controls(i.md_pme_level)?;
    let subpel_me = encdec::md_subpel_me_controls(i.me_subpel_level)?;
    let subpel_pme = encdec::md_subpel_pme_controls(i.pme_subpel_level)?;
    let nsq = encdec::md_nsq_motion_search_controls(i.md_nsq_mv_search_level)?;
    let pruning = ctrls::set_dist_based_ref_pruning_controls(i.dist_based_ref_pruning)?;

    let (qw, qwd) = crate::port_enc_mode_config::me::get_qp_based_th_scaling_factors(
        i.pme_qp_based_th_scaling,
        i.cli_qp,
    );
    let (w, h) = crate::port_md::md_search::pme_search_extents(
        pme.full_pel_search_width,
        pme.full_pel_search_height,
        pme.sa_q_weight != 0,
        qw,
        qwd,
    );

    Some(SearchFrameCfg {
        md_pme: MdPmeCtrls {
            enabled: pme.enabled != 0,
            full_pel_search_width: pme.full_pel_search_width,
            full_pel_search_height: pme.full_pel_search_height,
            sa_q_weight: pme.sa_q_weight != 0,
            enable_psad: pme.enable_psad != 0,
            early_check_mv_th_multiplier: pme.early_check_mv_th_multiplier,
            pre_fp_pme_to_me_mv_th: pme.pre_fp_pme_to_me_mv_th,
            pre_fp_pme_to_me_cost_th: i64::from(pme.pre_fp_pme_to_me_cost_th),
            post_fp_pme_to_me_mv_th: pme.post_fp_pme_to_me_mv_th,
            post_fp_pme_to_me_cost_th: i64::from(pme.post_fp_pme_to_me_cost_th),
        },
        pme_dist_type: match pme.dist_type {
            ctrls::DistortionType::Sad => DistortionType::Sad,
            ctrls::DistortionType::Var => DistortionType::Var,
            ctrls::DistortionType::Ssd => DistortionType::Ssd,
        },
        pme_full_pel_w: w,
        pme_full_pel_h: h,
        md_subpel_me: subpel_me,
        md_subpel_pme: subpel_pme,
        ifs_at_mds0: ctrls::set_interpolation_search_level_ctrls(i.interpolation_search_level)?
            == ctrls::IfsLevel::Mds0,
        ifs_level: ctrls::set_interpolation_search_level_ctrls(i.interpolation_search_level)?,
        md_nsq_me_enabled: nsq.enabled != 0,
        md_nsq_me_dist: match nsq.dist_type {
            ctrls::DistortionType::Sad => DistortionType::Sad,
            ctrls::DistortionType::Var => DistortionType::Var,
            ctrls::DistortionType::Ssd => DistortionType::Ssd,
        },
        md_nsq_full_pel_w: nsq.full_pel_search_width,
        md_nsq_full_pel_h: nsq.full_pel_search_height,
        md_nsq_enable_psad: nsq.enable_psad != 0,
        ref_pruning: pruning,
        // C `ctx->md_pme_ctrls.enabled` — the FIRST assignment of
        // `ctx->updated_enable_pme` (product_coding_loop.c:9418). The
        // `is_intra_bordered && use_neighbouring_mode_ctrls.enabled`
        // zeroing (:9419-9422) is per-block and lands in
        // `BlockSearchIn::updated_enable_pme` at the `block_prelude` call.
        md_pme_enabled: pme.enabled != 0,
        cli_qp: i.cli_qp,
        picture_qp: i.picture_qp,
        base_q_idx: i.base_q_idx,
        sad_per_bit: crate::port_md::pme::get_sad_per_bit(usize::from(i.base_q_idx), false),
        allow_high_precision_mv: i.allow_high_precision_mv,
        shut_fast_rate: false,
        approx_inter_rate: i.approx_inter_rate,
        pic_width: i.pic_width,
        pic_height: i.pic_height,
    })
}

#[cfg(test)]
mod tests;
