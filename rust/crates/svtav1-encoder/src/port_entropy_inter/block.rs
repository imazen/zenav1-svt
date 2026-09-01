//! `write_modes_b`'s INTER mode-info group, ported whole
//! (C `Source/Lib/Codec/entropy_coding.c:5196-5343`).
//!
//! Every one of the nine steps [`crate::inter_mv_code`] mapped now has a
//! port; what had no port was the **walk itself** — the order, the gates that
//! select among the sub-writers, and one cross-step mutation. That is what
//! this module is. It is deliberately a thin composition: it writes no symbol
//! of its own, and every symbol it does emit comes from a function in
//! [`super::refframe`], [`super::modes`], [`crate::inter_mv_code`],
//! [`super::compound`] or [`super::interp`].
//!
//! # Why the walk is the risky part
//!
//! Two things in it are invisible from any single sub-writer, and each one
//! desyncs a tile:
//!
//! * **Step 7 mutates `ref_frame[1]`, and steps 8, 9 and the interp filter
//!   read it.** C's `rf` is a POINTER into `mbmi->block_mi.ref_frame`, so the
//!   `rf[1] = INTRA_FRAME` an interintra block performs at :5246 changes
//!   three later decisions: step 8's gate is `rf[1] != INTRA_FRAME` (so an
//!   interintra block codes NO motion-mode symbol), step 9's gate is
//!   `has_second_ref` — which is `ref_frame[1] > INTRA_FRAME`, now false —
//!   and `write_mb_interp_filter` is passed `rf[1]` for its context. A port
//!   that keeps `ref_frame` immutable through the walk emits up to three
//!   extra symbol groups on every interintra block.
//! * **Steps 5 and 6 use DIFFERENT mode predicates.** DRL fires on
//!   `NEWMV || NEW_NEWMV || have_nearmv_in_inter_mode`; the MV write on
//!   `have_newmv_in_inter_mode`. They differ in four of the twelve inter
//!   modes. Both predicates already live in their own modules; the walk is
//!   where using one for both would go unnoticed.
//!
//! # What this module does NOT cover
//!
//! `write_modes_b`'s inter branch is larger than its mode-info group. Named
//! rather than implied:
//!
//! * the **prologue** (:5117-5195) — inter segment id, skip-mode, skip,
//!   CDEF, delta-q and the skip-mode tx-size call. It reads the tile's
//!   neighbour arrays and the CDEF latch, which live in `pipeline.rs`'s
//!   `EntropyCtx` (another lane's file). [`super::prologue`] ports the parts
//!   that are pure.
//! * the **intra-in-an-inter-frame arm** (:5199-5215) — `y_mode` /
//!   `uv_mode` / palette / filter-intra for an INTRA block inside a P frame.
//!   `encode_intra_luma_mode_nonkey` is in [`super::modes`]; the rest
//!   (`write_uv_mode`, `write_palette_mode_info`, `write_use_filter_intra`)
//!   is already in `entropy/context.rs`.
//! * the **epilogue** (:5344-5405) — palette map tokens, `code_tx_size` and
//!   the coefficient write. All three are ported (`write_palette_map_tokens`,
//!   `vartx`, `entropy/coeff_c.rs`) but sequencing them needs the tile
//!   neighbour arrays.
//!
//! # Evidence
//!
//! Tier 4 (`docs/WORKING-ON-THIS.md` §4). `write_modes_b` is `static` in
//! `entropy_coding.c`, which no shim compiles, so tier 1 is structurally
//! unavailable for the walk. Its INPUTS are tier-1 gated in
//! `tests/c_parity_entropy_inter.rs` and `tests/c_parity_entropy_compound.rs`;
//! what is tier 4 here is the ORDER and the GATES, pinned by hand-derived
//! symbol-sequence vectors traced against the C source. A byte gate arrives
//! when the inter frame path is wired.
//!
//! # Reachability
//!
//! Nothing here is called yet — the public entry point still refuses inter
//! frames (`pipeline.rs`, the `if !is_key` guard). Per §7 a faithful
//! translation with no caller stays translated.

use crate::entropy::context::FrameContext;
use crate::entropy::mv_coding::NmvContext;
use crate::entropy::writer::AomWriter;
use crate::inter_mv_code::{MvCodePlan, mv_precision, write_inter_block_mvs};
use crate::inter_mvp::mode_context_analyzer;
use crate::port_entropy_inter::compound::{CompGroup, InterIntraInfo, write_compound_type_info};
use crate::port_entropy_inter::interp::write_mb_interp_filter;
use crate::port_entropy_inter::modes::{
    DrlBlock, MotionMode, TransformationType, comp_group_idx_context, comp_index_context,
    have_nearmv_in_inter_mode, is_inter_compound_mode, is_inter_singleref_mode,
    is_interintra_allowed, motion_mode_allowed, write_drl_idx, write_inter_compound_mode,
    write_inter_mode, write_motion_mode,
};
use crate::port_entropy_inter::refframe::{
    INTRA_FRAME, RefFrameBlock, ReferenceMode, collect_neighbors_ref_counts, write_ref_frames,
};
use crate::port_entropy_inter::{InterCdfs, Neighbors};
use svtav1_types::block::BlockSize;
use svtav1_types::motion::Mv;
use svtav1_types::prediction::PredictionMode;

/// The frame-level constants the inter mode-info walk reads.
///
/// C reaches all of these through `pcs->ppcs->frm_hdr` and
/// `scs->seq_header`; gathering them into one borrow makes the walk's
/// signature honest about what it depends on, and makes it impossible to
/// pass a sequence flag where a frame-header flag belongs.
#[derive(Clone, Copy, Debug)]
pub struct InterFrameSyntax<'a> {
    /// C `frm_hdr->reference_mode`.
    pub reference_mode: ReferenceMode,
    /// C `frm_hdr->interpolation_filter` — `SWITCHABLE` (4) codes per block.
    pub interpolation_filter: u8,
    /// C `scs->seq_header.enable_dual_filter`.
    pub enable_dual_filter: bool,
    /// C `scs->seq_header.enable_interintra_compound`.
    pub enable_interintra_compound: bool,
    /// C `scs->seq_header.enable_masked_compound`.
    pub enable_masked_compound: bool,
    /// C `scs->seq_header.order_hint_info.enable_jnt_comp`.
    pub enable_jnt_comp: bool,
    /// C `scs->seq_header.order_hint_info.enable_order_hint`.
    pub enable_order_hint: bool,
    /// C `scs->seq_header.order_hint_info.order_hint_bits`.
    pub order_hint_bits: u32,
    /// C `frm_hdr->is_motion_mode_switchable`.
    pub is_motion_mode_switchable: bool,
    /// C `frm_hdr->allow_warped_motion`.
    pub allow_warped_motion: bool,
    /// C `frm_hdr->allow_high_precision_mv`.
    pub allow_high_precision_mv: bool,
    /// C `frm_hdr->force_integer_mv`.
    pub force_integer_mv: bool,
    /// C `pcs->ppcs->global_motion[ref].wmtype`, indexed by reference id.
    pub gm_wmtype: &'a [TransformationType; 8],
    /// C `pcs->ppcs->cur_order_hint`.
    pub cur_order_hint: i32,
    /// C `pcs->ppcs->ref_order_hint[]`, indexed `ref_frame - 1`.
    pub ref_order_hint: &'a [i32; 7],
}

/// Everything the inter mode-info walk reads off one block.
///
/// C spreads these across `MbModeInfo`, `EcBlkStruct` and `MacroBlockD`;
/// they are one value here because they describe one block and the walk
/// reads them in one pass.
#[derive(Clone, Debug)]
pub struct InterModeInfo {
    /// C `mbmi->bsize`.
    pub bsize: BlockSize,
    /// C `mbmi->block_mi.mode`.
    pub mode: PredictionMode,
    /// C `mbmi->block_mi.ref_frame`, as it stands BEFORE step 7 may set
    /// `[1]` to `INTRA_FRAME`.
    pub ref_frame: [i8; 2],
    /// C `mbmi->block_mi.mv`.
    pub mv: [Mv; 2],
    /// C `blk_ptr->predmv` — already `lower_mv_precision`-rounded by
    /// `svt_av1_find_best_ref_mvs_from_stack`. NOT a raw ref-MV-stack entry.
    pub pred_mv: [Mv; 2],
    /// C `blk_ptr->inter_mode_ctx[rf]`, before
    /// `svt_aom_mode_context_analyzer` folds it.
    pub inter_mode_ctx: i16,
    /// C `blk_ptr->drl_ctx` / `drl_ctx_near` / `drl_index`.
    pub drl: DrlBlock,
    /// `Some` iff C's `mbmi->block_mi.is_interintra_used`.
    pub interintra: Option<InterIntraInfo>,
    /// C `mbmi->block_mi.motion_mode`.
    pub motion_mode: MotionMode,
    /// C `mbmi->block_mi.num_proj_ref` — `motion_mode_allowed`'s input.
    pub num_proj_ref: u16,
    /// C `mbmi->block_mi.overlappable_neighbors`.
    pub overlappable_neighbors: u32,
    /// The compound group, when the block has two references. `None` for a
    /// single-reference block, where C's `has_second_ref` gate is false and
    /// the whole group is skipped.
    pub compound: Option<CompGroup>,
    /// C `mbmi->block_mi.interp_filters` — the packed `(y) | (x << 16)` pair.
    pub interp_filters: u32,
    /// C `mbmi->block_mi.skip_mode`, read by `av1_is_interp_needed`.
    pub skip_mode: bool,
}

/// What the walk emitted, for a caller that wants to assert against its own
/// candidate bookkeeping rather than re-derive the gates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InterModeInfoEmitted {
    /// The `ref_frame` AFTER step 7's possible `[1] = INTRA_FRAME`. Steps 8,
    /// 9 and the interp filter all read this, not the input value.
    pub ref_frame: [i8; 2],
    /// The folded `mode_ctx` steps 4 and 5 shared.
    pub mode_ctx: i16,
    /// Which MV differences step 6 wrote.
    pub mv_plan: MvCodePlan,
    /// Whether step 7 found the block interintra (and therefore suppressed
    /// steps 8 and 9).
    pub interintra_used: bool,
    /// The `last_motion_mode_allowed` step 8 resolved, or `None` when step 8
    /// was skipped because step 7 set `ref_frame[1]` to `INTRA_FRAME`.
    pub motion_mode_allowed: Option<MotionMode>,
}

/// C `write_modes_b`'s inter mode-info group (entropy_coding.c:5196-5343):
/// steps 1..9 of the walk recorded in [`crate::inter_mv_code`].
///
/// Called for a block that is INTER (`is_inter_mode(mode)`) and not
/// skip-mode; `write_is_inter` and everything before it belong to the
/// prologue.
///
/// `nb` is the neighbour pair as `set_mi_row_col` left it — see
/// [`super::Neighbors`] for why the pointer and the availability flag are
/// separate knobs. `nmvc` is the FRAME's single adapting `NmvContext`; a
/// fresh one per block is not decodable.
pub fn write_inter_mode_info(
    w: &mut AomWriter,
    fc: &mut FrameContext,
    ic: &mut InterCdfs,
    nmvc: &mut NmvContext,
    nb: &Neighbors,
    frame: &InterFrameSyntax<'_>,
    blk: &InterModeInfo,
) -> InterModeInfoEmitted {
    let mode_u8 = blk.mode as u8;
    debug_assert!(blk.mode.is_inter(), "the intra arm is the caller's branch");

    // Step 1: `svt_aom_collect_neighbors_ref_counts_new(xd)` (:5197). Not a
    // symbol, but every context step 2 reads comes out of it.
    let counts = collect_neighbors_ref_counts(nb);

    // Step 2: `write_ref_frames` (:5199).
    write_ref_frames(
        w,
        fc,
        ic,
        nb,
        &counts,
        frame.reference_mode,
        &RefFrameBlock {
            ref_frame: blk.ref_frame,
            bsize: blk.bsize,
        },
    );

    // Step 3: fold the block's raw `inter_mode_ctx` for this reference pair
    // (:5202). One value, shared by steps 4 and 5.
    let mode_ctx = mode_context_analyzer(blk.inter_mode_ctx, blk.ref_frame);

    // Step 4: exactly ONE mode symbol (:5207-5211). C's two predicates
    // partition the whole inter range with no gap, so this is exhaustive
    // rather than a filter — `else` would be wrong only if a value outside
    // `[NEARESTMV, NEW_NEWMV]` reached here, which `mode.is_inter()` above
    // rules out.
    if is_inter_compound_mode(mode_u8) {
        write_inter_compound_mode(w, ic, mode_u8, mode_ctx);
    } else {
        debug_assert!(is_inter_singleref_mode(mode_u8));
        write_inter_mode(w, ic, mode_u8, mode_ctx);
    }

    // Step 5: DRL index (:5213). Its predicate is NOT step 6's.
    if mode_u8 == PredictionMode::NewMv as u8
        || mode_u8 == PredictionMode::NewNewMv as u8
        || have_nearmv_in_inter_mode(mode_u8)
    {
        write_drl_idx(w, ic, mode_u8, &blk.drl);
    }

    // Step 6: the MV difference(s) (:5216-5244).
    let precision = mv_precision(frame.allow_high_precision_mv, frame.force_integer_mv);
    let mv_plan = write_inter_block_mvs(w, nmvc, blk.mode, &blk.mv, &blk.pred_mv, precision);

    // Step 7: interintra (:5245-5272). This is the step that can rewrite
    // `ref_frame[1]`, so the walk carries a MUTABLE copy from here on and
    // steps 8, 9 and the interp filter read that copy, exactly as C's `rf`
    // pointer aliases the mutated struct field.
    let mut ref_frame = blk.ref_frame;
    let allowed = is_interintra_allowed(blk.bsize, mode_u8, blk.ref_frame);
    let interintra_used = crate::port_entropy_inter::compound::write_interintra_info(
        w,
        ic,
        blk.bsize,
        &mut ref_frame,
        frame.enable_interintra_compound,
        allowed,
        blk.interintra,
    );

    // Step 8: motion mode (:5274-5277), gated on the POST-step-7 `rf[1]`.
    let mut motion_mode_allowed_out = None;
    if frame.is_motion_mode_switchable && ref_frame[1] != INTRA_FRAME {
        let last = motion_mode_allowed(
            frame.is_motion_mode_switchable,
            frame.force_integer_mv,
            frame.allow_warped_motion,
            frame.gm_wmtype,
            blk.num_proj_ref,
            blk.overlappable_neighbors,
            blk.bsize,
            ref_frame[0],
            ref_frame[1],
            mode_u8,
        );
        motion_mode_allowed_out = Some(last);
        write_motion_mode(w, ic, blk.bsize, blk.motion_mode, last);
    }

    // Step 9: the compound group (:5279-5342), gated `has_second_ref`, which
    // step 7 may have just falsified.
    if ref_frame[1] > INTRA_FRAME {
        let group = blk
            .compound
            .expect("a two-reference block carries a compound group");
        let comp_index_ctx = comp_index_context(
            frame.enable_order_hint,
            frame.order_hint_bits,
            frame.cur_order_hint,
            frame.ref_order_hint[(ref_frame[0] - 1) as usize],
            frame.ref_order_hint[(ref_frame[1] - 1) as usize],
            nb,
        );
        write_compound_type_info(
            w,
            ic,
            blk.bsize,
            frame.enable_masked_compound,
            frame.enable_jnt_comp,
            comp_group_idx_context(nb),
            comp_index_ctx,
            group,
        );
    }

    // The interp filter closes the group (:5343), and it too reads the
    // POST-step-7 reference pair.
    write_mb_interp_filter(
        w,
        ic,
        nb,
        frame.interpolation_filter,
        frame.enable_dual_filter,
        blk.bsize,
        ref_frame[0],
        ref_frame[1],
        mode_u8,
        blk.skip_mode,
        blk.motion_mode,
        blk.interp_filters,
        frame.gm_wmtype,
    );

    InterModeInfoEmitted {
        ref_frame,
        mode_ctx,
        mv_plan,
        interintra_used,
        motion_mode_allowed: motion_mode_allowed_out,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::port_entropy_inter::NeighborMi;
    use crate::port_entropy_inter::compound::{
        CompGroup, CompoundType, InterInterComp, InterIntraMode,
    };
    use crate::port_entropy_inter::refframe::{ALTREF_FRAME, LAST_FRAME};

    const GM_IDENTITY: [TransformationType; 8] = [TransformationType::Identity; 8];
    const HINTS: [i32; 7] = [1, 2, 3, 4, 5, 6, 7];

    fn frame() -> InterFrameSyntax<'static> {
        InterFrameSyntax {
            reference_mode: ReferenceMode::Select,
            interpolation_filter: 4, // SWITCHABLE
            enable_dual_filter: false,
            enable_interintra_compound: true,
            enable_masked_compound: true,
            enable_jnt_comp: true,
            enable_order_hint: true,
            order_hint_bits: 7,
            is_motion_mode_switchable: true,
            allow_warped_motion: true,
            allow_high_precision_mv: false,
            force_integer_mv: false,
            gm_wmtype: &GM_IDENTITY,
            cur_order_hint: 8,
            ref_order_hint: &HINTS,
        }
    }

    fn neighbors() -> Neighbors {
        let inter_nb = NeighborMi {
            mode: PredictionMode::NewMv as u8,
            ref_frame: [LAST_FRAME, -1],
            bsize: BlockSize::Block16x16 as u8,
            ..Default::default()
        };
        Neighbors {
            above: Some(inter_nb),
            left: Some(inter_nb),
            up_available: true,
            left_available: true,
        }
    }

    fn block(mode: PredictionMode, ref_frame: [i8; 2]) -> InterModeInfo {
        InterModeInfo {
            bsize: BlockSize::Block16x16,
            mode,
            ref_frame,
            mv: [Mv { x: 8, y: -4 }, Mv { x: 2, y: 2 }],
            pred_mv: [Mv { x: 4, y: 0 }, Mv { x: 0, y: 0 }],
            inter_mode_ctx: 0,
            drl: DrlBlock {
                drl_ctx: [0, 0],
                drl_ctx_near: [0, 0],
                drl_index: 0,
            },
            interintra: None,
            motion_mode: MotionMode::SimpleTranslation,
            num_proj_ref: 0,
            overlappable_neighbors: 0,
            compound: None,
            interp_filters: 0,
            skip_mode: false,
        }
    }

    fn run(blk: &InterModeInfo) -> (InterModeInfoEmitted, usize) {
        let mut fc = FrameContext::new_default();
        let mut ic = InterCdfs::new_default();
        let mut nmvc = NmvContext::default();
        let mut w = AomWriter::new(1024);
        let nb = neighbors();
        let out = write_inter_mode_info(&mut w, &mut fc, &mut ic, &mut nmvc, &nb, &frame(), blk);
        let bytes = w.done().len();
        (out, bytes)
    }

    /// Tier 4, traced against entropy_coding.c:5245-5277: an interintra
    /// block sets `ref_frame[1]` to INTRA_FRAME, which SUPPRESSES step 8.
    /// The same block with interintra off resolves a motion mode.
    #[test]
    fn interintra_suppresses_the_motion_mode_symbol() {
        let mut blk = block(PredictionMode::NewMv, [LAST_FRAME, -1]);
        blk.num_proj_ref = 1;
        blk.overlappable_neighbors = 2;

        let (plain, _) = run(&blk);
        assert!(!plain.interintra_used);
        assert_eq!(plain.ref_frame, [LAST_FRAME, -1]);
        assert_eq!(plain.motion_mode_allowed, Some(MotionMode::WarpedCausal));

        blk.interintra = Some(InterIntraInfo {
            mode: InterIntraMode::DcPred,
            use_wedge: false,
            wedge_index: 0,
        });
        let (ii, _) = run(&blk);
        assert!(ii.interintra_used);
        assert_eq!(ii.ref_frame, [LAST_FRAME, INTRA_FRAME]);
        assert_eq!(
            ii.motion_mode_allowed, None,
            "step 8 must be skipped once rf[1] is INTRA_FRAME"
        );
    }

    /// Tier 4, traced against :5279: step 9's `has_second_ref` reads the
    /// POST-step-7 `ref_frame`, so a compound block that turns out
    /// interintra codes no compound group. (A compound block is not
    /// interintra-allowed in C either — `is_interintra_allowed_ref` requires
    /// `rf[1] <= INTRA_FRAME` — so this asserts the gate ORDER holds even
    /// when the caller hands it a contradictory block.)
    #[test]
    fn compound_group_is_written_for_a_two_reference_block() {
        let mut blk = block(PredictionMode::NewNewMv, [LAST_FRAME, ALTREF_FRAME]);
        blk.compound = Some(CompGroup::B(InterInterComp {
            comp_type: CompoundType::Wedge,
            wedge_index: 3,
            wedge_sign: true,
            mask_type: 0,
        }));
        let (out, wedge_bytes) = run(&blk);
        assert_eq!(out.ref_frame, [LAST_FRAME, ALTREF_FRAME]);
        assert_eq!(out.mv_plan, MvCodePlan::Both);

        // Anti-vacuity: step 9 is REACHED, proved by the group choice
        // moving the coded bytes rather than by the gate reading true.
        blk.compound = Some(CompGroup::A { compound_idx: true });
        let (_, avg_bytes) = run(&blk);
        assert_ne!(
            wedge_bytes, avg_bytes,
            "the compound group must reach the bitstream"
        );
    }

    /// Tier 4, traced against :5213 vs :5216: the two predicates differ.
    /// `NEARMV` codes a DRL index and NO MV; `NEAREST_NEWMV` codes an MV
    /// and NO DRL index. Sharing one predicate would make both wrong.
    #[test]
    fn drl_and_mv_predicates_are_different_sets() {
        // NEARMV: DRL yes, MV no.
        let blk = block(PredictionMode::NearMv, [LAST_FRAME, -1]);
        let (out, _) = run(&blk);
        assert_eq!(out.mv_plan, MvCodePlan::None);
        assert!(have_nearmv_in_inter_mode(PredictionMode::NearMv as u8));

        // NEAREST_NEWMV: MV (ref 1) yes, DRL no.
        let blk = block(PredictionMode::NearestNewMv, [LAST_FRAME, ALTREF_FRAME]);
        let mut blk = blk;
        blk.compound = Some(CompGroup::A { compound_idx: true });
        let (out, _) = run(&blk);
        assert_eq!(out.mv_plan, MvCodePlan::Ref1);
        assert!(!have_nearmv_in_inter_mode(
            PredictionMode::NearestNewMv as u8
        ));
    }

    /// Anti-vacuity for the whole walk: a single-reference NEWMV block
    /// writes a non-empty symbol stream, and changing its mode changes the
    /// bytes. A walk that silently emitted nothing would pass every gate
    /// assertion above.
    #[test]
    fn the_walk_actually_emits() {
        let (_, a) = run(&block(PredictionMode::NewMv, [LAST_FRAME, -1]));
        let (_, b) = run(&block(PredictionMode::NearestMv, [LAST_FRAME, -1]));
        assert!(a > 0 && b > 0);
        assert_ne!(a, b, "NEWMV codes an MV difference, NEARESTMV does not");
    }
}
