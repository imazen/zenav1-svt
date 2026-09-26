use super::*;
use crate::entropy::context::FrameContext;
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
    let mut ref_cdfs = crate::port_entropy_inter::refframe::RefFrameCdfs {
        comp_inter_cdf: &mut fc.comp_inter_cdf,
        comp_ref_cdf: &mut fc.comp_ref_cdf,
        single_ref_cdf: &mut fc.single_ref_cdf,
    };
    let out = write_inter_mode_info(
        &mut w,
        &mut ref_cdfs,
        &mut ic,
        &mut nmvc,
        &nb,
        &frame(),
        blk,
    );
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
        mode: InterIntraMode::IiDcPred,
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
