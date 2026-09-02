//! The MDS0 rate of an inter candidate — `svt_aom_inter_fast_cost`
//! (rd_cost.c:1005), its `approx_inter_rate` twin `av1_inter_fast_cost_light`
//! (:870), and the two rate helpers they share.
//!
//! # Shape of the port
//!
//! C reads its inputs out of four fat structs (`PictureControlSet`,
//! `ModeDecisionContext`, `ModeDecisionCandidateBuffer`, `MacroBlockD`) and
//! writes two of its results BACK into the candidate buffer
//! (`fast_luma_rate`, `fast_chroma_rate`) before returning the cost. This port
//! takes the fields as three explicit inputs ([`InterFrame`], [`InterBlock`],
//! [`InterCandidate`]) and RETURNS the pair as [`FastRate`] instead of
//! mutating a buffer — same arithmetic, no aliasing, and the caller can price
//! a candidate without owning C's allocation graph.
//!
//! The rate tables keep C's shapes exactly ([`InterFacBits`]); they are the
//! `MdRateEstimationContext` rows these two functions read, and nothing else.

use svtav1_types::block::BlockSize;
use svtav1_types::motion::{CandidateMv, Mv};
use svtav1_types::prediction::{CompoundType, PredictionMode};

use crate::entropy::context::{
    BLOCK_SIZE_GROUPS, BLOCK_SIZES_ALL, DRL_MODE_CONTEXTS, GLOBALMV_MODE_CONTEXTS,
    INTER_MODE_CONTEXTS, INTRA_INTER_CONTEXTS, NEWMV_MODE_CONTEXTS, REFMV_MODE_CONTEXTS,
    SKIP_MODE_CONTEXTS, SWITCHABLE_FILTERS,
};
use crate::inter_mv_code::{
    have_nearmv_in_inter_mode, have_newmv_in_inter_mode, is_inter_compound_mode,
    is_inter_singleref_mode, mv_code_plan,
};
use crate::port_entropy_inter::Neighbors;
use crate::port_entropy_inter::interp::{
    SWITCHABLE_FILTER_CONTEXTS, extract_interp_filter, is_nontrans_global_motion,
    pred_context_switchable_interp,
};
use crate::port_entropy_inter::modes::{
    MOTION_MODES, MotionMode, TransformationType, comp_group_idx_context, comp_index_context,
    motion_mode_allowed,
};
use crate::port_entropy_inter::refframe::is_comp_ref_allowed;
use crate::port_md::drl::{av1_drl_ctx, mv_bit_cost};
use crate::port_md::pme::MvCostTable;
use crate::port_md_rate_estimation::WEDGE_PARAMS_BITS;
use crate::port_rd_cost::{MV_COST_WEIGHT, cost_literal, rdcost};

/// C `INTERINTRA_MODES` (definitions.h:1257).
pub const INTERINTRA_MODES: usize = 4;
/// C `INTER_COMPOUND_MODES` (definitions.h:1332): `1 + NEW_NEWMV - NEAREST_NEARESTMV`.
pub const INTER_COMPOUND_MODES: usize = 8;
/// C `MASKED_COMPOUND_TYPES` (definitions.h:1265).
pub const MASKED_COMPOUND_TYPES: usize = 2;
/// C `COMP_INDEX_CONTEXTS` (definitions.h:1337).
pub const COMP_INDEX_CONTEXTS: usize = 6;
/// C `COMP_GROUP_IDX_CONTEXTS` (definitions.h:1338).
pub const COMP_GROUP_IDX_CONTEXTS: usize = 6;
/// C `GLOBALMV_OFFSET` (definitions.h:1345).
const GLOBALMV_OFFSET: u32 = 3;
/// C `REFMV_OFFSET` (definitions.h:1346).
const REFMV_OFFSET: u32 = 4;
/// C `NEWMV_CTX_MASK` (definitions.h:1348).
const NEWMV_CTX_MASK: i16 = (1 << GLOBALMV_OFFSET) - 1;
/// C `GLOBALMV_CTX_MASK` (definitions.h:1349).
const GLOBALMV_CTX_MASK: i16 = (1 << (REFMV_OFFSET - GLOBALMV_OFFSET)) - 1;
/// C `REFMV_CTX_MASK` (definitions.h:1350).
const REFMV_CTX_MASK: i16 = (1 << (8 - REFMV_OFFSET)) - 1;
/// C `SWITCHABLE` (definitions.h:846) — the frame-header
/// interpolation-filter value that means "coded per block". Anything else
/// costs nothing.
///
/// It is `SWITCHABLE_FILTERS + 1` = **4**, NOT `SWITCHABLE_FILTERS` — the
/// enum is `EIGHTTAP, EIGHTTAP_SMOOTH, EIGHTTAP_SHARP, BILINEAR`, and
/// `SWITCHABLE_FILTERS = BILINEAR = 3` is the COUNT of coded filters while
/// `SWITCHABLE = 4` is a value outside that set. A first draft of this port
/// used 3, which silently zeroed every interpolation-filter rate on a real
/// switchable frame; the tier-1 differential caught it on the first cell.
pub const SWITCHABLE: u8 = SWITCHABLE_FILTERS as u8 + 1;
/// C `INTRA_FRAME` (definitions.h:1390).
const INTRA_FRAME: i8 = 0;
/// C `eb_size_group_lookup` (common_utils.c:36).
pub const SIZE_GROUP_LOOKUP: [u8; BLOCK_SIZES_ALL] = [
    0, 0, 0, 1, 1, 1, 2, 2, 2, 3, 3, 3, 3, 3, 3, 3, 0, 0, 1, 1, 2, 2,
];

// ---------------------------------------------------------------------------
// Compound-type predicates (inter_prediction.h / .c)
// ---------------------------------------------------------------------------

/// C `svt_aom_is_masked_compound_type` (inter_prediction.c:34).
#[inline]
pub fn is_masked_compound_type(t: CompoundType) -> bool {
    matches!(t, CompoundType::Wedge | CompoundType::DiffWtd)
}

/// C `svt_aom_is_interintra_wedge_used` (inter_prediction.c:2015):
/// `wedge_params_lookup[bsize].bits > 0`.
#[inline]
pub fn is_interintra_wedge_used(bsize: BlockSize) -> bool {
    WEDGE_PARAMS_BITS[bsize.as_index()] > 0
}

/// C `is_interinter_compound_used` (inter_prediction.h:288).
///
/// C's `default:` arm asserts and returns 0; the port makes the four cases
/// total instead, which is the same result without an unreachable arm.
#[inline]
pub fn is_interinter_compound_used(t: CompoundType, bsize: BlockSize) -> bool {
    let comp_allowed = is_comp_ref_allowed(bsize);
    match t {
        CompoundType::Average | CompoundType::DistWtd | CompoundType::DiffWtd => comp_allowed,
        CompoundType::Wedge => comp_allowed && WEDGE_PARAMS_BITS[bsize.as_index()] > 0,
    }
}

/// C `is_any_masked_compound_used` (inter_prediction.h:303).
///
/// C loops `COMPOUND_TYPES` and tests `is_masked_compound_type &&
/// is_interinter_compound_used`; the only two masked types are WEDGE and
/// DIFFWTD, so the loop reduces to their disjunction.
#[inline]
pub fn is_any_masked_compound_used(bsize: BlockSize) -> bool {
    if !is_comp_ref_allowed(bsize) {
        return false;
    }
    is_interinter_compound_used(CompoundType::Wedge, bsize)
        || is_interinter_compound_used(CompoundType::DiffWtd, bsize)
}

/// C `av1_is_interp_needed_md` (rd_cost.h:71) — `av1_is_interp_needed`
/// WITHOUT the `skip_mode` early-out, because IFS and skip-mode are mutually
/// exclusive by construction (C's own comment at rd_cost.h:70).
#[inline]
pub fn is_interp_needed_md(
    motion_mode: MotionMode,
    mode: PredictionMode,
    bsize: BlockSize,
    ref_frame: [i8; 2],
    gm_wmtype: &[TransformationType; 8],
) -> bool {
    if motion_mode == MotionMode::WarpedCausal {
        return false;
    }
    !is_nontrans_global_motion(mode as u8, bsize, ref_frame, gm_wmtype)
}

// ---------------------------------------------------------------------------
// Rate tables
// ---------------------------------------------------------------------------

/// The `MdRateEstimationContext` rows the two inter fast costs read
/// (md_rate_estimation.h:58-95), in C's shapes.
///
/// Every table is `int32_t` in C and every use widens into a `uint64_t`
/// accumulator, so the port keeps them `i32` and widens at the add — the
/// widening is C's choice, not the port's.
#[derive(Debug, Clone)]
pub struct InterFacBits {
    /// C `skip_mode_fac_bits[SKIP_CONTEXTS][2]`.
    pub skip_mode: [[i32; 2]; SKIP_MODE_CONTEXTS],
    /// C `intra_inter_fac_bits[INTRA_INTER_CONTEXTS][2]`.
    pub intra_inter: [[i32; 2]; INTRA_INTER_CONTEXTS],
    /// C `new_mv_mode_fac_bits[NEWMV_MODE_CONTEXTS][2]`.
    pub new_mv_mode: [[i32; 2]; NEWMV_MODE_CONTEXTS],
    /// C `zero_mv_mode_fac_bits[GLOBALMV_MODE_CONTEXTS][2]`.
    pub zero_mv_mode: [[i32; 2]; GLOBALMV_MODE_CONTEXTS],
    /// C `ref_mv_mode_fac_bits[REFMV_MODE_CONTEXTS][2]`.
    pub ref_mv_mode: [[i32; 2]; REFMV_MODE_CONTEXTS],
    /// C `drl_mode_fac_bits[DRL_MODE_CONTEXTS][2]`.
    pub drl_mode: [[i32; 2]; DRL_MODE_CONTEXTS],
    /// C `inter_compound_mode_fac_bits[INTER_MODE_CONTEXTS][INTER_COMPOUND_MODES]`.
    pub inter_compound_mode: [[i32; INTER_COMPOUND_MODES]; INTER_MODE_CONTEXTS],
    /// C `switchable_interp_fac_bitss[SWITCHABLE_FILTER_CONTEXTS][SWITCHABLE_FILTERS]`
    /// — the double `s` is C's typo, kept out of the port's name.
    pub switchable_interp: [[i32; SWITCHABLE_FILTERS]; SWITCHABLE_FILTER_CONTEXTS],
    /// C `motion_mode_fac_bits[BLOCK_SIZES_ALL][MOTION_MODES]`.
    pub motion_mode: [[i32; MOTION_MODES]; BLOCK_SIZES_ALL],
    /// C `motion_mode_fac_bits1[BLOCK_SIZES_ALL][2]` — the OBMC-only binary.
    pub motion_mode1: [[i32; 2]; BLOCK_SIZES_ALL],
    /// C `inter_intra_fac_bits[BlockSize_GROUPS][2]`.
    pub inter_intra: [[i32; 2]; BLOCK_SIZE_GROUPS],
    /// C `inter_intra_mode_fac_bits[BlockSize_GROUPS][INTERINTRA_MODES]`.
    pub inter_intra_mode: [[i32; INTERINTRA_MODES]; BLOCK_SIZE_GROUPS],
    /// C `wedge_inter_intra_fac_bits[BLOCK_SIZES_ALL][2]`.
    pub wedge_inter_intra: [[i32; 2]; BLOCK_SIZES_ALL],
    /// C `wedge_idx_fac_bits[BLOCK_SIZES_ALL][16]`.
    pub wedge_idx: [[i32; 16]; BLOCK_SIZES_ALL],
    /// C `comp_group_idx_fac_bits[COMP_GROUP_IDX_CONTEXTS][2]`.
    pub comp_group_idx: [[i32; 2]; COMP_GROUP_IDX_CONTEXTS],
    /// C `comp_idx_fac_bits[COMP_INDEX_CONTEXTS][2]`.
    pub comp_idx: [[i32; 2]; COMP_INDEX_CONTEXTS],
    /// C `compound_type_fac_bits[BLOCK_SIZES_ALL][MASKED_COMPOUND_TYPES]`.
    pub compound_type: [[i32; MASKED_COMPOUND_TYPES]; BLOCK_SIZES_ALL],
}

impl Default for InterFacBits {
    fn default() -> Self {
        Self::zeroed()
    }
}

impl InterFacBits {
    /// All-zero tables — the shape a caller fills from a frame context.
    pub fn zeroed() -> Self {
        Self {
            skip_mode: [[0; 2]; SKIP_MODE_CONTEXTS],
            intra_inter: [[0; 2]; INTRA_INTER_CONTEXTS],
            new_mv_mode: [[0; 2]; NEWMV_MODE_CONTEXTS],
            zero_mv_mode: [[0; 2]; GLOBALMV_MODE_CONTEXTS],
            ref_mv_mode: [[0; 2]; REFMV_MODE_CONTEXTS],
            drl_mode: [[0; 2]; DRL_MODE_CONTEXTS],
            inter_compound_mode: [[0; INTER_COMPOUND_MODES]; INTER_MODE_CONTEXTS],
            switchable_interp: [[0; SWITCHABLE_FILTERS]; SWITCHABLE_FILTER_CONTEXTS],
            motion_mode: [[0; MOTION_MODES]; BLOCK_SIZES_ALL],
            motion_mode1: [[0; 2]; BLOCK_SIZES_ALL],
            inter_intra: [[0; 2]; BLOCK_SIZE_GROUPS],
            inter_intra_mode: [[0; INTERINTRA_MODES]; BLOCK_SIZE_GROUPS],
            wedge_inter_intra: [[0; 2]; BLOCK_SIZES_ALL],
            wedge_idx: [[0; 16]; BLOCK_SIZES_ALL],
            comp_group_idx: [[0; 2]; COMP_GROUP_IDX_CONTEXTS],
            comp_idx: [[0; 2]; COMP_INDEX_CONTEXTS],
            compound_type: [[0; MASKED_COMPOUND_TYPES]; BLOCK_SIZES_ALL],
        }
    }
}

impl InterFacBits {
    /// Fill every table from the LIVE frame contexts, exactly as C's
    /// `svt_aom_estimate_syntax_rate` does (`rd_cost.c` /
    /// `md_rate_estimation.c`): each entry is
    /// `av1_cost_symbol(cdf)` over that element's alphabet.
    ///
    /// The two sources are the two places this port keeps inter CDFs:
    /// [`crate::entropy::context::FrameContext`] holds `intra_inter_cdf`, and
    /// [`crate::port_entropy_inter::InterCdfs`] holds the rest — because
    /// `FrameContext`'s `newmv` / `zeromv` / `refmv` / `drl` / `skip_mode` /
    /// `interp_filter` fields are UNIFORM PLACEHOLDERS (documented at
    /// `port_entropy_inter/cdfs.rs:14`) and pricing against a placeholder
    /// would give every inter mode the same rate.
    ///
    /// Alphabet sizes are C's, not `len - 1`: a CDF array can be wider than
    /// its alphabet (`docs/INTER-ENCODE-PLAN.md` §1s records the same trap on
    /// the adaptation side), so each `costs_from_cdf::<N>` spells N out.
    #[must_use]
    pub fn from_cdfs(
        fc: &crate::entropy::context::FrameContext,
        ic: &crate::port_entropy_inter::InterCdfs,
    ) -> Self {
        fn fill<const N: usize>(cdf: &[u16]) -> [i32; N] {
            let mut out = [0i32; N];
            crate::quant::syntax_rate_from_cdf(&mut out, cdf);
            out
        }
        fn rows<const R: usize, const N: usize>(src: impl Fn(usize) -> [i32; N]) -> [[i32; N]; R] {
            core::array::from_fn(src)
        }
        Self {
            skip_mode: rows(|i| fill::<2>(&ic.skip_mode_cdf[i])),
            intra_inter: rows(|i| fill::<2>(&fc.intra_inter_cdf[i])),
            new_mv_mode: rows(|i| fill::<2>(&ic.newmv_cdf[i])),
            zero_mv_mode: rows(|i| fill::<2>(&ic.zeromv_cdf[i])),
            ref_mv_mode: rows(|i| fill::<2>(&ic.refmv_cdf[i])),
            drl_mode: rows(|i| fill::<2>(&ic.drl_cdf[i])),
            inter_compound_mode: rows(|i| {
                fill::<INTER_COMPOUND_MODES>(&ic.inter_compound_mode_cdf[i])
            }),
            switchable_interp: rows(|i| fill::<SWITCHABLE_FILTERS>(&ic.switchable_interp_cdf[i])),
            motion_mode: rows(|i| fill::<MOTION_MODES>(&ic.motion_mode_cdf[i])),
            motion_mode1: rows(|i| fill::<2>(&ic.obmc_cdf[i])),
            inter_intra: rows(|i| fill::<2>(&ic.interintra_cdf[i])),
            inter_intra_mode: rows(|i| fill::<INTERINTRA_MODES>(&ic.interintra_mode_cdf[i])),
            wedge_inter_intra: rows(|i| fill::<2>(&ic.wedge_interintra_cdf[i])),
            wedge_idx: rows(|i| fill::<16>(&ic.wedge_idx_cdf[i])),
            comp_group_idx: rows(|i| fill::<2>(&ic.comp_group_idx_cdf[i])),
            comp_idx: rows(|i| fill::<2>(&ic.compound_index_cdf[i])),
            compound_type: rows(|i| fill::<MASKED_COMPOUND_TYPES>(&ic.compound_type_cdf[i])),
        }
    }
}

// ---------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------

/// The frame-level fields both fast costs read.
#[derive(Debug, Clone, Copy)]
pub struct InterFrame<'a> {
    /// C `frm_hdr->allow_screen_content_tools` — picks the light path's
    /// MV-rate factor (20 vs 50).
    pub allow_screen_content_tools: bool,
    /// C `frm_hdr->skip_mode_params.skip_mode_flag`.
    pub skip_mode_flag: bool,
    /// C `frm_hdr->interpolation_filter`. Only [`SWITCHABLE`] costs bits.
    pub interpolation_filter: u8,
    /// C `frm_hdr->is_motion_mode_switchable`.
    pub is_motion_mode_switchable: bool,
    /// C `frm_hdr->force_integer_mv`.
    pub force_integer_mv: bool,
    /// C `frm_hdr->allow_warped_motion`.
    pub allow_warped_motion: bool,
    /// C `scs->seq_header.enable_dual_filter`.
    pub enable_dual_filter: bool,
    /// C `scs->seq_header.enable_masked_compound`.
    pub enable_masked_compound: bool,
    /// C `scs->seq_header.order_hint_info.enable_jnt_comp`.
    pub enable_jnt_comp: bool,
    /// C `scs->seq_header.enable_interintra_compound`.
    pub enable_interintra_compound: bool,
    /// C `scs->seq_header.order_hint_info.enable_order_hint`.
    pub enable_order_hint: bool,
    /// C `scs->seq_header.order_hint_info.order_hint_bits`.
    pub order_hint_bits: u32,
    /// C `ppcs->cur_order_hint`.
    pub cur_order_hint: i32,
    /// C `ppcs->ref_order_hint[]`, indexed by `rf - 1`.
    pub ref_order_hint: &'a [i32; 7],
    /// C `ppcs->global_motion[ref].wmtype`.
    pub gm_wmtype: &'a [TransformationType; 8],
}

/// The block-level context both fast costs read.
#[derive(Debug, Clone, Copy)]
pub struct InterBlock<'a> {
    /// C `ctx->blk_geom->bsize`.
    pub bsize: BlockSize,
    /// C `ctx->skip_mode_ctx`.
    pub skip_mode_ctx: usize,
    /// C `ctx->is_inter_ctx`.
    pub is_inter_ctx: usize,
    /// C `ctx->inter_mode_ctx[ref_frame_type]` — the RAW value; this port
    /// applies `svt_aom_mode_context_analyzer` itself, as C does.
    pub inter_mode_ctx: i16,
    /// C `xd->ref_mv_count[ref_frame_type]`.
    pub ref_mv_count: u8,
    /// C `ctx->ref_mv_stack[ref_frame_type]`.
    pub ref_mv_stack: &'a [CandidateMv],
    /// C `ctx->estimate_ref_frames_num_bits[ref_frame_type]` — precomputed by
    /// [`crate::port_md::ref_frame_rate::estimate_ref_frames_num_bits`].
    pub ref_frames_num_bits: u64,
    /// C `blk_ptr->av1xd` neighbour halves — the switchable-interp,
    /// comp-group and comp-index contexts.
    pub neighbors: &'a Neighbors,
    /// C `blk_ptr->overlappable_neighbors`.
    pub overlappable_neighbors: u32,
    /// C `ctx->approx_inter_rate` (0, 1 or 2). >= 1 selects the light cost;
    /// >= 2 additionally drops the reference-signalling bits.
    pub approx_inter_rate: u8,
    /// C `ctx->ifs_ctrls.level == IFS_MDS0` — whether the interpolation
    /// filter is already known at MDS0 and therefore priced here.
    pub ifs_at_mds0: bool,
}

/// The candidate fields both fast costs read (C `ModeDecisionCandidate` /
/// `BlockModeInfo`).
#[derive(Debug, Clone, Copy)]
pub struct InterCandidate {
    /// C `block_mi.mode`.
    pub mode: PredictionMode,
    /// C `block_mi.ref_frame`.
    pub ref_frame: [i8; 2],
    /// C `block_mi.mv`.
    pub mv: [Mv; 2],
    /// C `cand->pred_mv`.
    pub pred_mv: [Mv; 2],
    /// C `cand->drl_index`.
    pub drl_index: u8,
    /// C `block_mi.interp_filters` — packed `(y) | (x << 16)`.
    pub interp_filters: u32,
    /// C `block_mi.motion_mode`.
    pub motion_mode: MotionMode,
    /// C `block_mi.num_proj_ref`.
    pub num_proj_ref: u16,
    /// C `block_mi.is_interintra_used`.
    pub is_interintra_used: bool,
    /// C `block_mi.interintra_mode`.
    pub interintra_mode: u8,
    /// C `block_mi.use_wedge_interintra`.
    pub use_wedge_interintra: bool,
    /// C `block_mi.interintra_wedge_index`.
    pub interintra_wedge_index: u8,
    /// C `block_mi.comp_group_idx`.
    pub comp_group_idx: u8,
    /// C `block_mi.compound_idx`.
    pub compound_idx: u8,
    /// C `block_mi.interinter_comp.type`.
    pub interinter_comp_type: CompoundType,
    /// C `block_mi.interinter_comp.wedge_index`.
    pub interinter_wedge_index: u8,
    /// C `cand->skip_mode_allowed`.
    pub skip_mode_allowed: bool,
}

/// What C writes back into the candidate buffer before returning
/// (`cand_bf->fast_luma_rate` / `fast_chroma_rate`), returned instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FastRate {
    /// C `cand_bf->fast_luma_rate`. C stores a `uint32_t`, and the sum that
    /// produces it is cast down from `uint64_t` — the port reproduces the
    /// truncation rather than widening it away.
    pub luma: u32,
    /// C `cand_bf->fast_chroma_rate`. Always 0 on the inter path.
    pub chroma: u32,
}

/// A fast cost plus the two rates C leaves behind in the candidate buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FastCost {
    /// The `RDCOST` C returns.
    pub cost: u64,
    /// The rates C wrote into `cand_bf`.
    pub rate: FastRate,
}

// ---------------------------------------------------------------------------
// svt_aom_get_switchable_rate (rd_cost.c:849)
// ---------------------------------------------------------------------------

/// C `svt_aom_get_switchable_rate` (rd_cost.c:849, EXPORTED).
///
/// Zero unless the frame header says `SWITCHABLE`. `enable_dual_filter`
/// decides whether one or two directions are priced; each pays
/// `switchable_interp_fac_bits[pred_ctx][filter]` for the filter packed into
/// `interp_filters` at that direction.
pub fn get_switchable_rate(
    interpolation_filter: u8,
    ref_frame: [i8; 2],
    interp_filters: u32,
    neighbors: &Neighbors,
    enable_dual_filter: bool,
    t: &InterFacBits,
) -> i32 {
    if interpolation_filter != SWITCHABLE {
        return 0;
    }
    let max_dir = if enable_dual_filter { 2 } else { 1 };
    (0..max_dir)
        .map(|dir| {
            let pred_ctx =
                pred_context_switchable_interp(ref_frame[0], ref_frame[1], neighbors, dir);
            let filter = extract_interp_filter(interp_filters, dir) as usize;
            debug_assert!(pred_ctx < SWITCHABLE_FILTER_CONTEXTS);
            debug_assert!(filter < SWITCHABLE_FILTERS);
            t.switchable_interp[pred_ctx][filter]
        })
        .sum()
}

// ---------------------------------------------------------------------------
// get_compound_mode_rate (rd_cost.c:783, static)
// ---------------------------------------------------------------------------

/// C `get_compound_mode_rate` (rd_cost.c:783).
///
/// Zero for a single-reference candidate — C's whole body is inside
/// `if (has_second_ref(mbmi))`, and `has_second_ref` is `ref_frame[1] >
/// INTRA_FRAME`, so an `INTRA_FRAME` second reference (the inter-intra case)
/// pays nothing here.
///
/// C's `assert`s on the un-taken arms are not reproduced: they assert
/// properties of the CANDIDATE (`comp_group_idx == 0` when masked compound is
/// unavailable, `compound_idx == 1` when jnt-comp is off), which belong to the
/// injector, and asserting them here would turn a caller's bug into a panic
/// inside a cost function.
pub fn get_compound_mode_rate(
    bsize: BlockSize,
    cand: &InterCandidate,
    frame: &InterFrame<'_>,
    neighbors: &Neighbors,
    t: &InterFacBits,
) -> u32 {
    // C `has_second_ref(&mbmi->block_mi)` after mbmi is loaded from the
    // candidate's own ref pair.
    if cand.ref_frame[1] <= INTRA_FRAME {
        return 0;
    }
    let bi = bsize.as_index();
    let mut comp_rate: u32 = 0;

    let masked_compound_used = is_any_masked_compound_used(bsize) && frame.enable_masked_compound;
    if masked_compound_used {
        let ctx = comp_group_idx_context(neighbors);
        comp_rate += t.comp_group_idx[ctx][usize::from(cand.comp_group_idx != 0)] as u32;
    }

    if cand.comp_group_idx == 0 {
        if frame.enable_jnt_comp {
            // C indexes `ref_order_hint[rf - 1]` for each reference, so a
            // reference id of 0 (INTRA_FRAME) would read out of bounds; the
            // `has_second_ref` gate above makes both ids >= 1.
            let bck = frame.ref_order_hint[(cand.ref_frame[0] as usize).saturating_sub(1)];
            let fwd = frame.ref_order_hint[(cand.ref_frame[1] as usize).saturating_sub(1)];
            let ctx = comp_index_context(
                frame.enable_order_hint,
                frame.order_hint_bits,
                frame.cur_order_hint,
                bck,
                fwd,
                neighbors,
            );
            comp_rate += t.comp_idx[ctx][usize::from(cand.compound_idx != 0)] as u32;
        }
    } else {
        // comp_group_idx == 1: a masked compound (wedge or diffwtd).
        if is_interinter_compound_used(CompoundType::Wedge, bsize) {
            let row = cand.interinter_comp_type as usize - CompoundType::Wedge as usize;
            comp_rate += t.compound_type[bi][row] as u32;
        }
        if cand.interinter_comp_type == CompoundType::Wedge {
            comp_rate += t.wedge_idx[bi][cand.interinter_wedge_index as usize] as u32;
        }
        // Both arms pay one literal bit: the wedge sign, or the diffwtd mask
        // type. C writes the same `av1_cost_literal(1)` in each.
        comp_rate += cost_literal(1);
    }

    comp_rate
}

// ---------------------------------------------------------------------------
// The mode / DRL rate shared by both fast costs
// ---------------------------------------------------------------------------

/// The inter-mode symbol rate: the compound-mode symbol, or the
/// NEWMV/GLOBALMV/REFMV cascade (rd_cost.c:1024-1055 and :884-908, which are
/// textually identical).
fn inter_mode_rate(mode: PredictionMode, mode_context: i16, t: &InterFacBits) -> u64 {
    if is_inter_compound_mode(mode) {
        let off = mode as usize - PredictionMode::NearestNearestMv as usize;
        debug_assert!(off < INTER_COMPOUND_MODES);
        // `mode_context` here is `svt_aom_mode_context_analyzer`'s compound
        // output, which its table bounds to 0..7 = INTER_MODE_CONTEXTS - 1.
        // C indexes with no mask and the port does the same.
        let ctx = mode_context as usize;
        debug_assert!(ctx < INTER_MODE_CONTEXTS);
        return t.inter_compound_mode[ctx][off] as u64;
    }
    let newmv_ctx = (mode_context & NEWMV_CTX_MASK) as usize;
    let mut bits = t.new_mv_mode[newmv_ctx][usize::from(mode != PredictionMode::NewMv)] as u64;
    if mode != PredictionMode::NewMv {
        let zero_mv_ctx = ((mode_context >> GLOBALMV_OFFSET) & GLOBALMV_CTX_MASK) as usize;
        bits += t.zero_mv_mode[zero_mv_ctx][usize::from(mode != PredictionMode::GlobalMv)] as u64;
        if mode != PredictionMode::GlobalMv {
            let ref_mv_ctx = ((mode_context >> REFMV_OFFSET) & REFMV_CTX_MASK) as usize;
            bits +=
                t.ref_mv_mode[ref_mv_ctx][usize::from(mode != PredictionMode::NearestMv)] as u64;
        }
    }
    bits
}

/// The DRL-index rate (rd_cost.c:1056-1086 / :909-936 — again identical).
///
/// Two independent walks, not one: a NEW-MV candidate walks `idx` 0..2 and a
/// NEAR-MV candidate walks 1..3 comparing against `idx - 1`, and a mode that
/// is both (`NEW_NEARMV`, `NEAR_NEWMV`) pays BOTH walks. Each walk stops at
/// the first index the candidate actually selected.
fn drl_rate(
    mode: PredictionMode,
    drl_index: u8,
    ref_mv_count: u8,
    ref_mv_stack: &[CandidateMv],
    t: &InterFacBits,
) -> u64 {
    let new_mv = mode == PredictionMode::NewMv || mode == PredictionMode::NewNewMv;
    let near_mv = have_nearmv_in_inter_mode(mode);
    if !new_mv && !near_mv {
        return 0;
    }
    let mut bits = 0u64;
    if new_mv {
        for idx in 0..2usize {
            if usize::from(ref_mv_count) > idx + 1 {
                let ctx = av1_drl_ctx(ref_mv_stack, idx) as usize;
                bits += t.drl_mode[ctx][usize::from(usize::from(drl_index) != idx)] as u64;
                if usize::from(drl_index) == idx {
                    break;
                }
            }
        }
    }
    if near_mv {
        for idx in 1..3usize {
            if usize::from(ref_mv_count) > idx + 1 {
                let ctx = av1_drl_ctx(ref_mv_stack, idx) as usize;
                bits += t.drl_mode[ctx][usize::from(usize::from(drl_index) != idx - 1)] as u64;
                if usize::from(drl_index) == idx - 1 {
                    break;
                }
            }
        }
    }
    bits
}

/// The IFS rate both costs add when the filter is already decided at MDS0.
fn ifs_rate(
    frame: &InterFrame<'_>,
    block: &InterBlock<'_>,
    cand: &InterCandidate,
    t: &InterFacBits,
) -> u32 {
    if !block.ifs_at_mds0
        || frame.interpolation_filter != SWITCHABLE
        || !is_interp_needed_md(
            cand.motion_mode,
            cand.mode,
            block.bsize,
            cand.ref_frame,
            frame.gm_wmtype,
        )
    {
        return 0;
    }
    get_switchable_rate(
        frame.interpolation_filter,
        cand.ref_frame,
        cand.interp_filters,
        block.neighbors,
        frame.enable_dual_filter,
        t,
    ) as u32
}

/// C's shared tail: fold `luma_rate` into an RDCOST, and take the skip-mode
/// cost instead when it is CHEAPER in RATE (rd_cost.c:997-1003 / :1197-1204).
///
/// Note the comparison is on the RATE, not on the cost — C compares
/// `skip_mode_rate < luma_rate` and only then recomputes the RDCOST. Both use
/// the same distortion, so the two orderings agree, but the port keeps C's
/// form because the distortion argument is the same object either way.
fn finish(
    luma_rate: u32,
    skip_mode_allowed: bool,
    skip_mode_ctx: usize,
    lambda: u64,
    luma_distortion: u64,
    t: &InterFacBits,
) -> FastCost {
    let rate = FastRate {
        luma: luma_rate,
        chroma: 0,
    };
    if skip_mode_allowed {
        let skip_mode_rate = t.skip_mode[skip_mode_ctx][1] as u32;
        if skip_mode_rate < luma_rate {
            return FastCost {
                cost: rdcost(lambda, u64::from(skip_mode_rate), luma_distortion),
                rate,
            };
        }
    }
    FastCost {
        cost: rdcost(lambda, u64::from(luma_rate), luma_distortion),
        rate,
    }
}

// ---------------------------------------------------------------------------
// av1_inter_fast_cost_light (rd_cost.c:870, static)
// ---------------------------------------------------------------------------

/// C `av1_inter_fast_cost_light` (rd_cost.c:870).
///
/// The `approx_inter_rate` path. Two things differ from the full cost beyond
/// the obvious omissions (no inter-intra, no motion mode, no compound rate):
///
/// * the MV rate is the table-free approximation `1296 + factor * (|dx| +
///   |dy|)` with `factor` **20 on a screen-content frame and 50 otherwise** —
///   `svt_av1_mv_bit_cost_light` hardcodes 50, so this function does NOT call
///   it and the port does not either;
/// * the reference-signalling bits are dropped entirely at
///   `approx_inter_rate >= 2`.
pub fn inter_fast_cost_light(
    frame: &InterFrame<'_>,
    block: &InterBlock<'_>,
    cand: &InterCandidate,
    lambda: u64,
    luma_distortion: u64,
    t: &InterFacBits,
) -> FastCost {
    let mode_context =
        crate::inter_mvp::mode_context_analyzer(block.inter_mode_ctx, cand.ref_frame);

    let reference_picture_bits = if block.approx_inter_rate < 2 {
        block.ref_frames_num_bits
    } else {
        0
    };

    let inter_mode_bits = inter_mode_rate(cand.mode, mode_context, t)
        + drl_rate(
            cand.mode,
            cand.drl_index,
            block.ref_mv_count,
            block.ref_mv_stack,
            t,
        );

    let mv_rate = if have_newmv_in_inter_mode(cand.mode) {
        let factor: u32 = if frame.allow_screen_content_tools {
            20
        } else {
            50
        };
        light_mv_rate(cand, factor)
    } else {
        0
    };

    let ifs = ifs_rate(frame, block, cand, t);
    let is_inter_rate = t.intra_inter[block.is_inter_ctx][1] as u32;
    let skip_mode_rate = if frame.skip_mode_flag && is_comp_ref_allowed(block.bsize) {
        t.skip_mode[block.skip_mode_ctx][0] as u32
    } else {
        0
    };

    // C: `luma_rate = (uint32_t)(<uint64 sum>)` — the cast is C's.
    let luma_rate = (reference_picture_bits
        + u64::from(skip_mode_rate)
        + inter_mode_bits
        + mv_rate
        + u64::from(is_inter_rate)
        + u64::from(ifs)) as u32;

    finish(
        luma_rate,
        cand.skip_mode_allowed,
        block.skip_mode_ctx,
        lambda,
        luma_distortion,
        t,
    )
}

/// The light path's MV-rate term (rd_cost.c:937-975).
///
/// `absmvdiff*` are C `uint16_t`, so the difference of two `int16_t`
/// components is taken in `int`, absolute-valued, and then NARROWED to 16
/// bits. A legal MV pair cannot exceed the range where that matters, but the
/// narrowing is part of the contract and the port keeps it.
fn light_mv_rate(cand: &InterCandidate, factor: u32) -> u64 {
    let one = |i: usize| -> u64 {
        let dx = (i32::from(cand.mv[i].x) - i32::from(cand.pred_mv[i].x)).unsigned_abs() as u16;
        let dy = (i32::from(cand.mv[i].y) - i32::from(cand.pred_mv[i].y)).unsigned_abs() as u16;
        u64::from(1296 + factor * (u32::from(dx) + u32::from(dy)))
    };
    match cand.mode {
        PredictionMode::NewNewMv => one(0) + one(1),
        PredictionMode::NearestNewMv | PredictionMode::NearNewMv => one(1),
        PredictionMode::NewNearestMv | PredictionMode::NewNearMv => one(0),
        // Single-reference NEWMV: the unipred MV lives at index 0.
        _ => one(0),
    }
}

// ---------------------------------------------------------------------------
// svt_aom_inter_fast_cost (rd_cost.c:1005)
// ---------------------------------------------------------------------------

/// C `svt_aom_inter_fast_cost` (rd_cost.c:1005, EXPORTED).
///
/// Dispatches to [`inter_fast_cost_light`] when `approx_inter_rate` is set,
/// exactly as C does at its head.
pub fn inter_fast_cost(
    frame: &InterFrame<'_>,
    block: &InterBlock<'_>,
    cand: &InterCandidate,
    lambda: u64,
    luma_distortion: u64,
    nmv: Option<&MvCostTable>,
    t: &InterFacBits,
) -> FastCost {
    if block.approx_inter_rate != 0 {
        return inter_fast_cost_light(frame, block, cand, lambda, luma_distortion, t);
    }
    let bi = block.bsize.as_index();
    let mode_context =
        crate::inter_mvp::mode_context_analyzer(block.inter_mode_ctx, cand.ref_frame);

    let mut inter_mode_bits = inter_mode_rate(cand.mode, mode_context, t)
        + drl_rate(
            cand.mode,
            cand.drl_index,
            block.ref_mv_count,
            block.ref_mv_stack,
            t,
        );

    // The real MV rate (rd_cost.c:1088-1128). WHICH references are priced is
    // `mv_code_plan`, this port's existing translation of that dispatch —
    // shared with the writer so MD cannot price MVs the writer will not emit.
    // `nmv` is `None` on the `approx_inter_rate` arm, where C zeroes the
    // tables and every lookup returns 0.
    let mv_rate = match (have_newmv_in_inter_mode(cand.mode), nmv) {
        (true, Some(tables)) => mv_code_plan(cand.mode)
            .refs()
            .iter()
            .map(|&r| mv_bit_cost(cand.mv[r], cand.pred_mv[r], tables, MV_COST_WEIGHT) as u64)
            .sum(),
        _ => 0,
    };

    // Inter-intra (rd_cost.c:1130-1157). C signals inter-intra OFF even when
    // the tool is disabled for the block, so the flag's rate is paid whenever
    // the sequence enables the compound and the block/mode allows it.
    if frame.enable_interintra_compound
        && crate::port_md::predicates::is_interintra_allowed(
            true,
            block.bsize as u8,
            cand.mode as u8,
            cand.ref_frame,
        )
    {
        let group = SIZE_GROUP_LOOKUP[bi] as usize;
        inter_mode_bits += t.inter_intra[group][usize::from(cand.is_interintra_used)] as u64;
        if cand.is_interintra_used {
            inter_mode_bits += t.inter_intra_mode[group][cand.interintra_mode as usize] as u64;
            if is_interintra_wedge_used(block.bsize) {
                inter_mode_bits +=
                    t.wedge_inter_intra[bi][usize::from(cand.use_wedge_interintra)] as u64;
                if cand.use_wedge_interintra {
                    inter_mode_bits += t.wedge_idx[bi][cand.interintra_wedge_index as usize] as u64;
                }
            }
        }
    }

    // Motion mode (rd_cost.c:1159-1176). The ALLOWED mode decides the
    // alphabet: none, the OBMC binary, or the full MOTION_MODES symbol.
    if is_inter_singleref_mode(cand.mode)
        && frame.is_motion_mode_switchable
        && cand.ref_frame[1] != INTRA_FRAME
    {
        let last_allowed = motion_mode_allowed(
            frame.is_motion_mode_switchable,
            frame.force_integer_mv,
            frame.allow_warped_motion,
            frame.gm_wmtype,
            cand.num_proj_ref,
            block.overlappable_neighbors,
            block.bsize,
            cand.ref_frame[0],
            cand.ref_frame[1],
            cand.mode as u8,
        );
        match last_allowed {
            MotionMode::SimpleTranslation => {}
            MotionMode::ObmcCausal => {
                inter_mode_bits += t.motion_mode1[bi]
                    [usize::from(cand.motion_mode == MotionMode::ObmcCausal)]
                    as u64;
            }
            MotionMode::WarpedCausal => {
                inter_mode_bits += t.motion_mode[bi][cand.motion_mode as usize] as u64;
            }
        }
    }

    inter_mode_bits += u64::from(get_compound_mode_rate(
        block.bsize,
        cand,
        frame,
        block.neighbors,
        t,
    ));

    let ifs = ifs_rate(frame, block, cand, t);
    let is_inter_rate = t.intra_inter[block.is_inter_ctx][1] as u32;
    let skip_mode_rate = if frame.skip_mode_flag && is_comp_ref_allowed(block.bsize) {
        t.skip_mode[block.skip_mode_ctx][0] as u32
    } else {
        0
    };

    let luma_rate = (block.ref_frames_num_bits
        + u64::from(skip_mode_rate)
        + inter_mode_bits
        + mv_rate
        + u64::from(is_inter_rate)
        + u64::from(ifs)) as u32;

    finish(
        luma_rate,
        cand.skip_mode_allowed,
        block.skip_mode_ctx,
        lambda,
        luma_distortion,
        t,
    )
}
