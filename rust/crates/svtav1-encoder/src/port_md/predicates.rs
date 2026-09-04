//! The pure gates, tables and dedup rules of `Source/Lib/Codec/mode_decision.c`.
//!
//! These are the predicates every inter-candidate injector consults. They
//! decide *which candidates exist at all*, so a wrong answer here changes
//! the MDS0 pool membership — and for [`is_interintra_allowed`] and
//! [`obmc_motion_mode_allowed`] it also changes coded syntax, because the
//! same predicates gate the writer.
//!
//! | this module | C |
//! |---|---|
//! | [`get_ref_frame_type`] | `mode_decision.c:262-267` (EXPORTED) |
//! | [`get_max_drl_index`] | `mode_decision.c:269-291` (EXPORTED) |
//! | [`is_interintra_allowed`] + the three helpers | `mode_decision.c:96-100`, `mode_decision.h:142-152` |
//! | [`get_me_block_offset`] | `mode_decision.c:117-170` (EXPORTED) |
//! | [`is_me_data_present`] | `mode_decision.c:179-199` (EXPORTED) |
//! | [`is_valid_unipred_ref`] | `mode_decision.c:762-774` (EXPORTED) |
//! | [`is_valid_bipred_ref`] | `mode_decision.c:793-813` (`static`) |
//! | [`check_mv_validity`] | `mode_decision.c:80-94` (`static`) |
//! | [`is_valid_mv_diff`] | `mode_decision.c:776-791` (`static`) |
//! | [`mv_is_already_injected`] | `mode_decision.c:712-760` (`static`) |
//! | [`get_tot_comp_types_bsize`] | `mode_decision.c:111-113` (`static`) |
//! | [`obmc_motion_mode_allowed`] | `mode_decision.c:214-256` (EXPORTED) |
//! | [`warped_motion_mode_allowed`] | `mode_decision.c:208-212` (`static`) |
//! | [`wedge_params_bits`] | `inter_prediction.c:1990-2013` + `:2053` (EXPORTED) |
//!
//! # Build-configuration facts this port depends on (checked, not inferred)
//!
//! * `CONFIG_ENABLE_OBMC` is **1** in the mainline build:
//!   `EbConfigMacros.h:82` supplies the default and `:33`'s 0 is inside
//!   `#if RTC_BUILD`, which is 0 (`:25-27`). So the
//!   `#if CONFIG_ENABLE_OBMC` definition of `warped_motion_mode_allowed`
//!   (`mode_decision.c:207-212`) IS compiled.
//! * `svt_aom_obmc_motion_mode_allowed` sits OUTSIDE that `#if`; it is
//!   compiled unconditionally.
//! * Linkage was re-checked with `nm -g Bin/Release/libSvtAv1Enc.a`, not
//!   read off the prefix: `svt_aom_inject_inter_candidates` carries the
//!   `svt_aom_` prefix and is `static`, `set_md_stage_counts` carries no
//!   prefix and IS exported.
//!
//! # Evidence
//!
//! Tier 1 for the seven exported symbols — `tests/c_parity_md_predicates.rs`
//! drives the real `libSvtAv1Enc.a` through `shims/mode_decision_shims.c`.
//! The five `static` functions ([`is_valid_bipred_ref`],
//! [`check_mv_validity`], [`is_valid_mv_diff`], [`mv_is_already_injected`],
//! [`warped_motion_mode_allowed`]) have no exported symbol and are
//! **tier 4** — hand-derived vectors traced against the C source, labelled
//! as such in their tests.

use svtav1_types::motion::{Mv, TransformationType};
use svtav1_types::prediction::PredictionMode;
use svtav1_types::tables::block::{BLOCK_SIZE_HIGH, BLOCK_SIZE_WIDE};

// ---------------------------------------------------------------------------
// Constants (C definitions.h / cabac_context_model.h / md_process.h)
// ---------------------------------------------------------------------------

/// C `MV_IN_USE_BITS` (cabac_context_model.h:197).
pub const MV_IN_USE_BITS: u32 = 14;
/// C `MV_UPP` / `MV_LOW` (cabac_context_model.h:198-199).
const MV_UPP: i32 = 1 << MV_IN_USE_BITS;
const MV_LOW: i32 = -(1 << MV_IN_USE_BITS);

/// C `INTRA_FRAME` (definitions.h): reference-frame id 0.
pub const INTRA_FRAME: i8 = 0;
/// C `LAST_FRAME` .. `ALTREF_FRAME` (definitions.h:1390-1398).
pub const LAST_FRAME: i8 = 1;
pub const ALTREF_FRAME: i8 = 7;
/// C `INVALID_REF` (mode_decision.h:203) — the hole in `to_ref_frame[1][3]`.
/// It is **0xF, not 0xFF**; the tier-1 differential caught a first draft
/// that assumed the byte-wide sentinel.
pub const INVALID_REF: i32 = 0xF;
/// C `TOTAL_REFS_PER_FRAME` (definitions.h:1398) = `ALTREF - INTRA + 1`.
pub const TOTAL_REFS_PER_FRAME: usize = 8;

/// C `InterCandGroup` (md_process.h:64-78). The index into
/// `ctx->ref_filtering_res` and `ref_pruning_ctrls.closest_refs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum InterCandGroup {
    PaMe = 0,
    Uni3x3 = 1,
    Bi3x3 = 2,
    NrstNewNear = 3,
    NrstNear = 4,
    PredMe = 5,
    Global = 6,
    Warp = 7,
    Obmc = 8,
    InterIntra = 9,
    InterComp = 10,
}

/// C `TOT_INTER_GROUP` (md_process.h:77).
pub const TOT_INTER_GROUP: usize = 11;
/// C `MAX_NUM_OF_REF_PIC_LIST` (definitions.h:2048).
pub const MAX_NUM_OF_REF_PIC_LIST: usize = 2;
/// C `REF_LIST_MAX_DEPTH` (EbSvtAv1Enc.h:35).
pub const REF_LIST_MAX_DEPTH: usize = 4;

/// C `MotionMode` (definitions.h:1250-1255).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MotionMode {
    SimpleTranslation = 0,
    ObmcCausal = 1,
    WarpedCausal = 2,
}

/// C `MD_COMP_TYPE` (definitions.h:1285-1291).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum MdCompType {
    Avg = 0,
    Dist = 1,
    Diff0 = 2,
    Wedge = 3,
}

// ---------------------------------------------------------------------------
// Reference-frame table (mode_decision.c:262-267)
// ---------------------------------------------------------------------------

/// C `to_ref_frame[2][4]` (mode_decision.c:262-263).
///
/// The `[1][3]` slot is `INVALID_REF` (0xFF) — not a reference type. C's
/// return type is `MvReferenceFrame` (a `uint8_t`), so the port returns
/// `i32` and callers must not blindly narrow the hole to `i8`.
const TO_REF_FRAME: [[i32; 4]; 2] = [[1, 2, 3, 4], [5, 6, 7, INVALID_REF]];

/// C `svt_get_ref_frame_type` (mode_decision.c:265-267, EXPORTED).
///
/// Every injector and the reference-pruning code indexes this table, so
/// its `(list, ref_idx)` order is load-bearing: `list` is the OUTER index.
#[inline]
pub fn get_ref_frame_type(list: u8, ref_idx: u8) -> i32 {
    TO_REF_FRAME[list as usize][ref_idx as usize]
}

// ---------------------------------------------------------------------------
// DRL bound (mode_decision.c:269-291)
// ---------------------------------------------------------------------------

/// C `svt_aom_get_max_drl_index` (mode_decision.c:269-291, EXPORTED).
///
/// Bounds the DRL loop in BOTH injection and syntax writing. Note the two
/// arms use DIFFERENT thresholds on `refmv_cnt` (2 vs 3), and a mode in
/// neither arm returns 0 — C initialises `max_drl` to 0 and falls
/// through.
pub fn get_max_drl_index(refmv_cnt: u8, mode: PredictionMode) -> u8 {
    let mut max_drl = 0u8;

    if mode == PredictionMode::NewMv || mode == PredictionMode::NewNewMv {
        max_drl = if refmv_cnt < 2 {
            1
        } else if refmv_cnt == 2 {
            2
        } else {
            3
        };
    }

    if mode == PredictionMode::NearMv
        || mode == PredictionMode::NearNearMv
        || mode == PredictionMode::NearNewMv
        || mode == PredictionMode::NewNearMv
    {
        max_drl = if refmv_cnt < 3 {
            1
        } else if refmv_cnt == 3 {
            2
        } else {
            3
        };
    }

    max_drl
}

// ---------------------------------------------------------------------------
// Inter-intra gate (mode_decision.c:96-100 + mode_decision.h:142-152)
// ---------------------------------------------------------------------------

/// C `svt_aom_is_interintra_allowed_bsize` (mode_decision.h:142-144).
///
/// An ENUM-ORDER range, not a dimension test: `BLOCK_8X8 <= bsize <=
/// BLOCK_32X32`. In C's `BlockSize` order that is indices 3..=9, which
/// EXCLUDES the extended shapes 8X32 / 32X8 (indices 18/19) even though
/// their dimensions are within 8..32.
#[inline]
pub fn is_interintra_allowed_bsize(bsize: u8) -> bool {
    const BLOCK_8X8: u8 = 3;
    const BLOCK_32X32: u8 = 9;
    (BLOCK_8X8..=BLOCK_32X32).contains(&bsize)
}

/// C `svt_aom_is_interintra_allowed_mode` (mode_decision.h:146-148).
#[inline]
pub fn is_interintra_allowed_mode(mode: u8) -> bool {
    (PredictionMode::SINGLE_INTER_MODE_START..PredictionMode::SINGLE_INTER_MODE_END).contains(&mode)
}

/// C `svt_aom_is_interintra_allowed_ref` (mode_decision.h:150-152).
#[inline]
pub fn is_interintra_allowed_ref(rf: [i8; 2]) -> bool {
    rf[0] > INTRA_FRAME && rf[1] <= INTRA_FRAME
}

/// C `svt_is_interintra_allowed` (mode_decision.c:96-100, EXPORTED).
///
/// This gates inter-intra INJECTION and the `is_interintra_used` SYNTAX
/// (entropy_coding.c:4928-4930 calls the same three helpers), so a wrong
/// answer desyncs the tile rather than merely mis-ordering RD.
#[inline]
pub fn is_interintra_allowed(enable: bool, bsize: u8, mode: u8, rf: [i8; 2]) -> bool {
    enable
        && is_interintra_allowed_bsize(bsize)
        && is_interintra_allowed_mode(mode)
        && is_interintra_allowed_ref(rf)
}

// ---------------------------------------------------------------------------
// Compound-type cap (mode_decision.c:111-113)
// ---------------------------------------------------------------------------

/// C `wedge_params_lookup[bsize].bits` (inter_prediction.c:1990-2013),
/// reached through the EXPORTED `svt_aom_get_wedge_params_bits` (`:2053`).
///
/// 4 for the ten wedge-capable shapes, 0 elsewhere. Note 8X32 (18) and
/// 32X8 (19) DO carry wedge params while 4X16 (16) / 16X4 (17) /
/// 16X64 (20) / 64X16 (21) do not.
pub const WEDGE_PARAMS_BITS: [i32; 22] = [
    0, 0, 0, 4, 4, 4, 4, 4, 4, 4, 0, 0, 0, 0, 0, 0, 0, 0, 4, 4, 0, 0,
];

/// C `svt_aom_get_wedge_params_bits` (inter_prediction.c:2053, EXPORTED).
#[inline]
pub fn wedge_params_bits(bsize: u8) -> i32 {
    WEDGE_PARAMS_BITS[bsize as usize]
}

/// C `get_tot_comp_types_bsize` (mode_decision.c:111-113, `static`).
///
/// Caps the compound-type set to `MD_COMP_WEDGE` when the block size has
/// no wedge parameters. C's `MIN` is over the raw enum values, so the cap
/// keeps WEDGE itself available — it only bites when a caller passes a
/// value above `MD_COMP_WEDGE`.
#[inline]
pub fn get_tot_comp_types_bsize(tot_comp_types: u8, bsize: u8) -> u8 {
    if wedge_params_bits(bsize) == 0 {
        tot_comp_types.min(MdCompType::Wedge as u8)
    } else {
        tot_comp_types
    }
}

// ---------------------------------------------------------------------------
// ME-data presence (mode_decision.c:179-199)
// ---------------------------------------------------------------------------

/// C `MeCandidate` (me_sb_results.h:29-35), the five bitfields as bytes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MeCandidateRef {
    /// C `direction : 2` — 0 = list-0 uni, 1 = list-1 uni, 2 = bi.
    pub direction: u8,
    pub ref_idx_l0: u8,
    pub ref_idx_l1: u8,
    pub ref0_list: u8,
    pub ref1_list: u8,
}

/// C `svt_aom_is_me_data_present` (mode_decision.c:179-199, EXPORTED).
///
/// Decides whether a `(list_idx, ref_idx)` pair has ME data to inject
/// from. `totals` is C's `me_results->total_me_candidate_index`; `cands`
/// is the slice of `me_candidate_array` starting at `me_cand_offset`
/// (C indexes `me_candidate_array + me_cand_offset` and then walks
/// `totals[me_block_offset]` entries from there).
pub fn is_me_data_present(
    me_block_offset: usize,
    totals: &[u8],
    cands_from_offset: &[MeCandidateRef],
    list_idx: u8,
    ref_idx: u8,
) -> bool {
    let total_me_cnt = totals[me_block_offset] as usize;
    for cand in cands_from_offset.iter().take(total_me_cnt) {
        debug_assert!(cand.direction <= 2);
        if (cand.direction == 0 || cand.direction == 2)
            && list_idx == cand.ref0_list
            && ref_idx == cand.ref_idx_l0
        {
            return true;
        }
        if (cand.direction == 1 || cand.direction == 2)
            && list_idx == cand.ref1_list
            && ref_idx == cand.ref_idx_l1
        {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// ME block offset (mode_decision.c:117-170)
// ---------------------------------------------------------------------------

/// The two conversion tables live in the ME port already
/// (`coefficients.c:1572` / `:1585`); they are USED from there rather than
/// re-transcribed, so the two ports cannot drift.
use crate::inter_me::tables::{ME_IDX_16X16_TO_PARENT_32X32, ME_IDX_85_8X8_TO_16X16};

/// C `me_idx_85` (mode_decision.h:164-167). 85 entries: the z-order ME
/// index for each of the 85 PUs an SB64 can hold.
pub const ME_IDX_85: [u32; 85] = [
    0, 1, 5, 21, 22, 29, 30, 6, 23, 24, 31, 32, 9, 37, 38, 45, 46, 10, 39, 40, 47, 48, 2, 7, 25,
    26, 33, 34, 8, 27, 28, 35, 36, 11, 41, 42, 49, 50, 12, 43, 44, 51, 52, 3, 13, 53, 54, 61, 62,
    14, 55, 56, 63, 64, 17, 69, 70, 77, 78, 18, 71, 72, 79, 80, 4, 15, 57, 58, 65, 66, 16, 59, 60,
    67, 68, 19, 73, 74, 81, 82, 20, 75, 76, 83, 84,
];

/// C `MAX_SB64_PU_COUNT_NO_8X8` (definitions.h).
const MAX_SB64_PU_COUNT_NO_8X8: u32 = 21;
/// C `MAX_SB64_PU_COUNT_WO_16X16` (definitions.h).
const MAX_SB64_PU_COUNT_WO_16X16: u32 = 5;

/// C `svt_aom_get_me_block_offset` (mode_decision.c:117-170, EXPORTED).
///
/// Maps a block's origin + size to its slot in the SB's ME results. The C
/// `switch` uses `AOM_FALLTHROUGH_INTENDED` between the 4/8, 16 and 32
/// cases, so a 4x4 block accumulates all three sets of offsets; the port
/// spells that fallthrough out explicitly.
pub fn get_me_block_offset(
    org_x: u32,
    org_y: u32,
    bsize: u8,
    enable_me_8x8: bool,
    enable_me_16x16: bool,
) -> u32 {
    let bwidth = u32::from(BLOCK_SIZE_WIDE[bsize as usize]);
    let bheight = u32::from(BLOCK_SIZE_HIGH[bsize as usize]);
    let max_length = bwidth.max(bheight);

    let mut me_idx = 0u32;
    // The C switch falls through 4/8 -> 16 -> 32.
    let start = match max_length {
        4 | 8 => 0,
        16 => 1,
        32 => 2,
        _ => 3,
    };
    if start <= 0 {
        me_idx += 1;
        if org_x & 8 != 0 {
            me_idx += 1;
        }
        if org_y & 8 != 0 {
            me_idx += 2;
        }
    }
    if start <= 1 {
        me_idx += 1;
        if org_x & 16 != 0 {
            me_idx += 5;
        }
        if org_y & 16 != 0 {
            me_idx += 10;
        }
    }
    if start <= 2 {
        me_idx += 1;
        if org_x & 32 != 0 {
            me_idx += 21;
        }
        if org_y & 32 != 0 {
            me_idx += 42;
        }
    }

    let mut me_block_offset = ME_IDX_85[me_idx as usize];

    if !enable_me_8x8 {
        if me_block_offset >= MAX_SB64_PU_COUNT_NO_8X8 {
            me_block_offset = u32::from(
                ME_IDX_85_8X8_TO_16X16[(me_block_offset - MAX_SB64_PU_COUNT_NO_8X8) as usize],
            );
        }
        debug_assert!(me_block_offset < 21);
        if !enable_me_16x16 && me_block_offset >= MAX_SB64_PU_COUNT_WO_16X16 {
            debug_assert!(me_block_offset < 21);
            me_block_offset = u32::from(
                ME_IDX_16X16_TO_PARENT_32X32
                    [(me_block_offset - MAX_SB64_PU_COUNT_WO_16X16) as usize],
            );
        }
    }

    me_block_offset
}

// ---------------------------------------------------------------------------
// Reference-pruning gates (mode_decision.c:762-774, 793-813)
// ---------------------------------------------------------------------------

/// C `ctx->ref_pruning_ctrls` + `ctx->ref_filtering_res` as the two
/// pruning gates read them.
#[derive(Debug, Clone)]
pub struct RefPruningState {
    /// C `ref_pruning_ctrls.enabled`.
    pub enabled: bool,
    /// C `ref_filtering_res[group][list][ref].do_ref`.
    pub do_ref: [[[bool; REF_LIST_MAX_DEPTH]; MAX_NUM_OF_REF_PIC_LIST]; TOT_INTER_GROUP],
    /// C `ref_pruning_ctrls.closest_refs[group]`.
    pub closest_refs: [bool; TOT_INTER_GROUP],
}

impl Default for RefPruningState {
    fn default() -> Self {
        Self {
            enabled: false,
            do_ref: [[[false; REF_LIST_MAX_DEPTH]; MAX_NUM_OF_REF_PIC_LIST]; TOT_INTER_GROUP],
            closest_refs: [false; TOT_INTER_GROUP],
        }
    }
}

/// C `svt_aom_is_valid_unipred_ref` (mode_decision.c:762-774, EXPORTED).
///
/// Consulted before every uni-pred injection. The relaxation is
/// `!ref_idx` — i.e. only the CLOSEST reference of a list survives a
/// `do_ref == 0` when `closest_refs[group]` is set.
pub fn is_valid_unipred_ref(
    state: &RefPruningState,
    group: InterCandGroup,
    list_idx: usize,
    ref_idx: usize,
) -> bool {
    if !state.enabled {
        return true;
    }
    let g = group as usize;
    !(!state.do_ref[g][list_idx][ref_idx] && (ref_idx != 0 || !state.closest_refs[g]))
}

/// C `is_valid_bipred_ref` (mode_decision.c:793-813, `static`).
///
/// The compound counterpart. BOTH refs must pass, with a single
/// relaxation: when `closest_refs[group]` is set and BOTH `ref_idx` are 0
/// (LAST and BWD), the pair survives even though one or both `do_ref` are
/// clear.
pub fn is_valid_bipred_ref(
    state: &RefPruningState,
    group: InterCandGroup,
    list_idx_0: usize,
    ref_idx_0: usize,
    list_idx_1: usize,
    ref_idx_1: usize,
) -> bool {
    if !state.enabled {
        return true;
    }
    let g = group as usize;
    if !state.do_ref[g][list_idx_0][ref_idx_0] || !state.do_ref[g][list_idx_1][ref_idx_1] {
        if !state.closest_refs[g] {
            return false;
        }
        if ref_idx_0 != 0 || ref_idx_1 != 0 {
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------------------
// MV validity and injected-MV dedup (mode_decision.c:80-94, 712-791)
// ---------------------------------------------------------------------------

/// C `is_mv_valid` (inter_prediction.h:275-277).
#[inline]
pub fn is_mv_valid(mv: Mv) -> bool {
    let y = i32::from(mv.y);
    let x = i32::from(mv.x);
    y > MV_LOW && y < MV_UPP && x > MV_LOW && x < MV_UPP
}

/// C `check_mv_validity` (mode_decision.c:80-94, `static`).
///
/// `need_shift` promotes a 1/4-pel input to 1/8-pel BEFORE the range
/// test. C shifts an `int16_t` into an `int16_t` field, so the shift
/// WRAPS on overflow; the port reproduces that with `wrapping_shl`
/// rather than widening, because a widened value would pass a range test
/// C fails (and vice versa).
#[inline]
pub fn check_mv_validity(x_mv: i16, y_mv: i16, need_shift: u32) -> bool {
    let mv = Mv {
        x: x_mv.wrapping_shl(need_shift),
        y: y_mv.wrapping_shl(need_shift),
    };
    is_mv_valid(mv)
}

/// C `is_valid_mv_diff` (mode_decision.c:776-791, `static`).
///
/// The `MV_IN_USE_BITS` clamp on mv-minus-predmv. C computes
/// `mv.x - best_pred_mv.x` in `int` after integer promotion of two
/// `int16_t`s, so no wrap occurs; the port widens to `i32` to match.
pub fn is_valid_mv_diff(best_pred_mv: [Mv; 2], mv0: Mv, mv1: Mv, is_compound: bool) -> bool {
    let limit = 1i32 << MV_IN_USE_BITS;
    let d = |a: i16, b: i16| (i32::from(a) - i32::from(b)).abs();

    if d(mv0.x, best_pred_mv[0].x) > limit || d(mv0.y, best_pred_mv[0].y) > limit {
        return false;
    }
    if is_compound && (d(mv1.x, best_pred_mv[1].x) > limit || d(mv1.y, best_pred_mv[1].y) > limit) {
        return false;
    }
    true
}

/// C `RedundantCandCtrls` (md_process.h:675-678).
#[derive(Debug, Clone, Copy, Default)]
pub struct RedundantCandCtrls {
    pub score_th: i32,
    pub mag_th: i32,
}

/// The already-injected MV log C keeps on the MD context
/// (`ctx->injected_mvs` / `injected_ref_types` / `injected_mv_count`,
/// md_process.h:1023-1026).
#[derive(Debug, Clone, Default)]
pub struct InjectedMvLog {
    /// C `injected_mvs[i][0..2]`.
    pub mvs: Vec<[Mv; 2]>,
    /// C `injected_ref_types[i]`.
    pub ref_types: Vec<u8>,
}

impl InjectedMvLog {
    pub fn push(&mut self, mvs: [Mv; 2], ref_type: u8) {
        self.mvs.push(mvs);
        self.ref_types.push(ref_type);
    }

    /// C `ctx->injected_mv_count`.
    pub fn count(&self) -> usize {
        self.mvs.len()
    }
}

/// C `mv_is_already_injected` (mode_decision.c:712-760, `static`).
///
/// Returns `true` when the candidate must NOT be injected — either
/// because it duplicates one already logged, or because
/// `corrupted_mv_check` is on and the MV is out of AV1 range (C folds the
/// invalid case into "already injected" so the caller drops it).
///
/// `rf` is the decoded `(ref_frame[0], ref_frame[1])` pair for
/// `ref_type`; the uni-pred arm is taken when `rf[1] <= INTRA_FRAME`.
///
/// The bi-pred arm has TWO shapes. With `redund_ctrls.score_th == 0` it
/// is an exact `as_int` match on both MVs. With a non-zero `score_th` it
/// is an L1 distance over all four components, accepted as redundant
/// when the score is 0 OR (`score < score_th` AND all four components of
/// BOTH MVs exceed `mag_th` in magnitude). The `is_high_mag` conjunction
/// is over ALL FOUR components — a single small component disables the
/// approximate prune for the whole candidate.
pub fn mv_is_already_injected(
    log: &InjectedMvLog,
    redund_ctrls: RedundantCandCtrls,
    corrupted_mv_check: bool,
    mv0: Mv,
    mv1: Mv,
    ref_type: u8,
    rf: [i8; 2],
) -> bool {
    if rf[1] <= INTRA_FRAME {
        // Uni-pred candidate.
        if corrupted_mv_check && !check_mv_validity(mv0.x, mv0.y, 0) {
            return true;
        }
        for i in 0..log.count() {
            if log.ref_types[i] == ref_type && log.mvs[i][0].as_int() == mv0.as_int() {
                return true;
            }
        }
    } else {
        // Bi-pred candidate.
        if corrupted_mv_check
            && (!check_mv_validity(mv0.x, mv0.y, 0) || !check_mv_validity(mv1.x, mv1.y, 0))
        {
            return true;
        }
        if redund_ctrls.score_th != 0 {
            let mag = redund_ctrls.mag_th;
            let is_high_mag = i32::from(mv0.x).abs() > mag
                && i32::from(mv0.y).abs() > mag
                && i32::from(mv1.x).abs() > mag
                && i32::from(mv1.y).abs() > mag;
            for i in 0..log.count() {
                if log.ref_types[i] != ref_type {
                    continue;
                }
                let score = (i32::from(log.mvs[i][0].x) - i32::from(mv0.x)).abs()
                    + (i32::from(log.mvs[i][0].y) - i32::from(mv0.y)).abs()
                    + (i32::from(log.mvs[i][1].x) - i32::from(mv1.x)).abs()
                    + (i32::from(log.mvs[i][1].y) - i32::from(mv1.y)).abs();
                if score == 0 || (score < redund_ctrls.score_th && is_high_mag) {
                    return true;
                }
            }
        } else {
            for i in 0..log.count() {
                if log.ref_types[i] == ref_type
                    && log.mvs[i][0].as_int() == mv0.as_int()
                    && log.mvs[i][1].as_int() == mv1.as_int()
                {
                    return true;
                }
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Motion-mode gates (mode_decision.c:207-256)
// ---------------------------------------------------------------------------

/// C `is_motion_variation_allowed_bsize` (inter_prediction.h:407-409) — a
/// forward to the one body,
/// [`crate::port_entropy_inter::modes::is_motion_variation_allowed_bsize_idx`]
/// (this module carried its own copy until 2026-09-04).
#[inline]
pub fn is_motion_variation_allowed_bsize(bsize: u8) -> bool {
    crate::port_entropy_inter::modes::is_motion_variation_allowed_bsize_idx(usize::from(bsize))
}

/// C `is_inter_singleref_mode` (definitions.h:1626-1628).
#[inline]
pub fn is_inter_singleref_mode(mode: u8) -> bool {
    (PredictionMode::SINGLE_INTER_MODE_START..PredictionMode::SINGLE_INTER_MODE_END).contains(&mode)
}

/// C `is_global_mv_block` (inter_prediction.h:411-414) — a forward to the
/// one body, [`crate::port_entropy_inter::modes::is_global_mv_block_idx`].
#[inline]
pub fn is_global_mv_block(mode: u8, bsize: u8, wm_type: TransformationType) -> bool {
    crate::port_entropy_inter::modes::is_global_mv_block_idx(mode, usize::from(bsize), wm_type)
}

/// The C context fields [`obmc_motion_mode_allowed`] reads.
#[derive(Debug, Clone, Copy)]
pub struct MotionModeCtx {
    /// C `ctx->obmc_ctrls.trans_face_off`.
    pub trans_face_off: bool,
    /// C `ctx->obmc_ctrls.enabled`.
    pub obmc_enabled: bool,
    /// C `ctx->obmc_ctrls.max_blk_size`.
    pub obmc_max_blk_size: u8,
    /// C `pcs->ppcs->frm_hdr.is_motion_mode_switchable`.
    pub is_motion_mode_switchable: bool,
    /// C `pcs->ppcs->frm_hdr.force_integer_mv`.
    pub force_integer_mv: u8,
    /// C `blk_ptr->overlappable_neighbors != 0`.
    pub has_overlappable_candidates: bool,
    /// C `pcs->ppcs->frm_hdr.allow_warped_motion`.
    pub allow_warped_motion: bool,
    /// C `ctx->wm_ctrls.enabled`.
    pub wm_enabled: bool,
    /// C `ctx->blk_geom->bwidth` / `bheight`.
    pub blk_width: u16,
    pub blk_height: u16,
}

/// C `svt_aom_obmc_motion_mode_allowed` (mode_decision.c:214-256, EXPORTED).
///
/// `situation` is C's parameter: 0 = candidate preparation, 1 = data
/// preparation, 2 = simple-translation face-off. Only `situation == 0`
/// is short-circuited by `trans_face_off` — the face-off itself runs
/// later (`obmc_trans_face_off`, product_coding_loop.c:1068), which is
/// why the early return is not simply "OBMC off".
///
/// `gm_wmtype` is `pcs->ppcs->global_motion[rf0].wmtype`. The
/// `force_integer_mv == 0` guard is C's: with integer MVs forced, the
/// global-motion escape is skipped entirely.
///
/// This ALSO gates the `motion_mode` syntax (entropy side), so a wrong
/// answer desyncs the tile rather than merely mis-ordering RD.
pub fn obmc_motion_mode_allowed(
    ctx: &MotionModeCtx,
    bsize: u8,
    situation: u8,
    gm_wmtype: TransformationType,
    rf0: i8,
    rf1: i8,
    mode: u8,
) -> MotionMode {
    let _ = rf0;
    if ctx.trans_face_off && situation == 0 {
        return MotionMode::SimpleTranslation;
    }
    if BLOCK_SIZE_WIDE[bsize as usize] > ctx.obmc_max_blk_size
        || BLOCK_SIZE_HIGH[bsize as usize] > ctx.obmc_max_blk_size
    {
        return MotionMode::SimpleTranslation;
    }
    if !ctx.obmc_enabled {
        return MotionMode::SimpleTranslation;
    }
    if !ctx.is_motion_mode_switchable {
        return MotionMode::SimpleTranslation;
    }
    if ctx.force_integer_mv == 0 && is_global_mv_block(mode, bsize, gm_wmtype) {
        return MotionMode::SimpleTranslation;
    }
    // C: `rf1 != INTRA_FRAME && !(rf1 > INTRA_FRAME)` — i.e. rf1 < 0
    // (NONE_FRAME). Both halves are written out because the C is written
    // that way and the conjunction is NOT `rf1 <= INTRA_FRAME`.
    if is_motion_variation_allowed_bsize(bsize)
        && is_inter_singleref_mode(mode)
        && rf1 != INTRA_FRAME
        && !(rf1 > INTRA_FRAME)
    {
        if !ctx.has_overlappable_candidates {
            return MotionMode::SimpleTranslation;
        }
        return MotionMode::ObmcCausal;
    }
    MotionMode::SimpleTranslation
}

/// C `warped_motion_mode_allowed` (mode_decision.c:207-212, `static`,
/// inside `#if CONFIG_ENABLE_OBMC` — which is 1 in this build).
///
/// Without this no `WARPED_CAUSAL` candidate is ever injected.
#[inline]
pub fn warped_motion_mode_allowed(ctx: &MotionModeCtx) -> bool {
    ctx.allow_warped_motion
        && ctx.has_overlappable_candidates
        && ctx.blk_width >= 8
        && ctx.blk_height >= 8
        && ctx.wm_enabled
}

// ---------------------------------------------------------------------------
// Tests for the `static` C functions — TIER 4 (hand-derived vectors traced
// against the C source). The exported ones are gated at tier 1 in
// `tests/c_parity_md_predicates.rs`.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn mv(x: i16, y: i16) -> Mv {
        Mv { x, y }
    }

    /// TIER 4 — `check_mv_validity` (mode_decision.c:80-94) is `static`.
    /// Traced against `is_mv_valid` (inter_prediction.h:275): the bound is
    /// STRICT on both sides, so exactly +-16384 is INVALID.
    #[test]
    fn tier4_check_mv_validity_bounds_are_strict() {
        assert!(check_mv_validity(0, 0, 0));
        assert!(check_mv_validity(16383, 16383, 0));
        assert!(check_mv_validity(-16383, -16383, 0));
        // 16384 == MV_UPP: the comparison is `< MV_UPP`, so this fails.
        assert!(!check_mv_validity(16384, 0, 0));
        assert!(!check_mv_validity(0, 16384, 0));
        assert!(!check_mv_validity(-16384, 0, 0));
        assert!(!check_mv_validity(0, -16384, 0));
    }

    /// TIER 4 — the `need_shift` promotion. C computes `y_mv << shift` in
    /// `int` (integer promotion) and then ASSIGNS it into `Mv`'s
    /// `int16_t` field, so the high bits are discarded before the range
    /// test runs. `32767 << 1` is `0xFFFE` = `-2`, which PASSES; a port
    /// that widened to `i32` would see `65534` and reject it.
    #[test]
    fn tier4_check_mv_validity_shift_truncates_to_int16() {
        assert_eq!(32767i16.wrapping_shl(1), -2);
        assert!(check_mv_validity(4096, 4096, 1));
        assert!(check_mv_validity(32767, 0, 1));
        assert!(check_mv_validity(0, 32767, 1));
        // 16384 << 1 = 0x8000 = -32768, which is <= MV_LOW -> rejected.
        assert_eq!(16384i16.wrapping_shl(1), -32768);
        assert!(!check_mv_validity(16384, 0, 1));
    }

    /// TIER 4 — `is_valid_mv_diff` (mode_decision.c:776-791). The bound is
    /// `> (1 << 14)`, i.e. a difference of exactly 16384 is ACCEPTED.
    #[test]
    fn tier4_is_valid_mv_diff_bound_is_inclusive() {
        let pred = [mv(0, 0), mv(0, 0)];
        assert!(is_valid_mv_diff(pred, mv(16384, 0), mv(0, 0), false));
        assert!(!is_valid_mv_diff(pred, mv(16385, 0), mv(0, 0), false));
        assert!(!is_valid_mv_diff(pred, mv(0, -16385), mv(0, 0), false));
        // The second MV is only checked for compound candidates.
        assert!(is_valid_mv_diff(pred, mv(0, 0), mv(32000, 0), false));
        assert!(!is_valid_mv_diff(pred, mv(0, 0), mv(32000, 0), true));
    }

    /// TIER 4 — `is_valid_bipred_ref` (mode_decision.c:793-813).
    fn bipred_state(enabled: bool, do_ref: bool, closest: bool) -> RefPruningState {
        let mut s = RefPruningState {
            enabled,
            ..Default::default()
        };
        for g in 0..TOT_INTER_GROUP {
            s.closest_refs[g] = closest;
            for l in 0..MAX_NUM_OF_REF_PIC_LIST {
                for r in 0..REF_LIST_MAX_DEPTH {
                    s.do_ref[g][l][r] = do_ref;
                }
            }
        }
        s
    }

    #[test]
    fn tier4_is_valid_bipred_ref_disabled_accepts_everything() {
        let s = bipred_state(false, false, false);
        assert!(is_valid_bipred_ref(&s, InterCandGroup::PaMe, 0, 3, 1, 3));
    }

    #[test]
    fn tier4_is_valid_bipred_ref_needs_both_refs() {
        let mut s = bipred_state(true, true, false);
        assert!(is_valid_bipred_ref(&s, InterCandGroup::PaMe, 0, 1, 1, 1));
        s.do_ref[InterCandGroup::PaMe as usize][1][1] = false;
        assert!(!is_valid_bipred_ref(&s, InterCandGroup::PaMe, 0, 1, 1, 1));
    }

    #[test]
    fn tier4_is_valid_bipred_ref_closest_relaxation_needs_both_idx_zero() {
        let s = bipred_state(true, false, true);
        // LAST + BWD (both ref_idx 0) survive the relaxation.
        assert!(is_valid_bipred_ref(&s, InterCandGroup::Bi3x3, 0, 0, 1, 0));
        // Either index non-zero and the pair is rejected.
        assert!(!is_valid_bipred_ref(&s, InterCandGroup::Bi3x3, 0, 1, 1, 0));
        assert!(!is_valid_bipred_ref(&s, InterCandGroup::Bi3x3, 0, 0, 1, 2));
    }

    /// TIER 4 — `mv_is_already_injected` (mode_decision.c:712-760).
    #[test]
    fn tier4_mv_is_already_injected_unipred_matches_on_mv_and_ref_type() {
        let mut log = InjectedMvLog::default();
        log.push([mv(4, -8), mv(0, 0)], 1);
        let ctrls = RedundantCandCtrls::default();
        let rf = [1i8, -1];
        assert!(mv_is_already_injected(
            &log,
            ctrls,
            false,
            mv(4, -8),
            mv(0, 0),
            1,
            rf
        ));
        // Same MV, different ref type -> not a duplicate.
        assert!(!mv_is_already_injected(
            &log,
            ctrls,
            false,
            mv(4, -8),
            mv(0, 0),
            2,
            rf
        ));
        // Same ref type, different MV -> not a duplicate.
        assert!(!mv_is_already_injected(
            &log,
            ctrls,
            false,
            mv(4, -7),
            mv(0, 0),
            1,
            rf
        ));
    }

    #[test]
    fn tier4_mv_is_already_injected_corrupted_check_drops_out_of_range() {
        let log = InjectedMvLog::default();
        let ctrls = RedundantCandCtrls::default();
        let rf = [1i8, -1];
        // With the check OFF an out-of-range MV is NOT reported injected.
        assert!(!mv_is_already_injected(
            &log,
            ctrls,
            false,
            mv(16384, 0),
            mv(0, 0),
            1,
            rf
        ));
        // With it ON, C folds "invalid" into "already injected".
        assert!(mv_is_already_injected(
            &log,
            ctrls,
            true,
            mv(16384, 0),
            mv(0, 0),
            1,
            rf
        ));
    }

    #[test]
    fn tier4_mv_is_already_injected_bipred_exact_arm() {
        let mut log = InjectedMvLog::default();
        log.push([mv(4, 4), mv(-4, -4)], 9);
        let ctrls = RedundantCandCtrls::default();
        let rf = [1i8, 5];
        assert!(mv_is_already_injected(
            &log,
            ctrls,
            false,
            mv(4, 4),
            mv(-4, -4),
            9,
            rf
        ));
        // Only the second MV differs -> the exact arm keeps it.
        assert!(!mv_is_already_injected(
            &log,
            ctrls,
            false,
            mv(4, 4),
            mv(-4, -3),
            9,
            rf
        ));
    }

    #[test]
    fn tier4_mv_is_already_injected_bipred_score_arm_needs_all_four_high_mag() {
        let mut log = InjectedMvLog::default();
        log.push([mv(100, 100), mv(-100, -100)], 9);
        let ctrls = RedundantCandCtrls {
            score_th: 16,
            mag_th: 32,
        };
        let rf = [1i8, 5];
        // score = 4, every component magnitude > 32 -> pruned.
        assert!(mv_is_already_injected(
            &log,
            ctrls,
            false,
            mv(101, 101),
            mv(-101, -101),
            9,
            rf
        ));
        // One component below mag_th kills is_high_mag for the WHOLE
        // candidate, so the approximate prune no longer fires and only an
        // exact (score == 0) match would.
        assert!(!mv_is_already_injected(
            &log,
            ctrls,
            false,
            mv(101, 101),
            mv(-101, -1),
            9,
            rf
        ));
        // score == 0 still prunes regardless of magnitude.
        let mut small = InjectedMvLog::default();
        small.push([mv(1, 1), mv(1, 1)], 9);
        assert!(mv_is_already_injected(
            &small,
            ctrls,
            false,
            mv(1, 1),
            mv(1, 1),
            9,
            rf
        ));
    }

    /// TIER 4 — `warped_motion_mode_allowed` (mode_decision.c:207-212).
    #[test]
    fn tier4_warped_motion_mode_allowed_needs_all_five_conditions() {
        let base = MotionModeCtx {
            trans_face_off: false,
            obmc_enabled: true,
            obmc_max_blk_size: 128,
            is_motion_mode_switchable: true,
            force_integer_mv: 0,
            has_overlappable_candidates: true,
            allow_warped_motion: true,
            wm_enabled: true,
            blk_width: 16,
            blk_height: 16,
        };
        assert!(warped_motion_mode_allowed(&base));
        assert!(!warped_motion_mode_allowed(&MotionModeCtx {
            allow_warped_motion: false,
            ..base
        }));
        assert!(!warped_motion_mode_allowed(&MotionModeCtx {
            has_overlappable_candidates: false,
            ..base
        }));
        assert!(!warped_motion_mode_allowed(&MotionModeCtx {
            blk_width: 4,
            ..base
        }));
        assert!(!warped_motion_mode_allowed(&MotionModeCtx {
            blk_height: 4,
            ..base
        }));
        assert!(!warped_motion_mode_allowed(&MotionModeCtx {
            wm_enabled: false,
            ..base
        }));
    }
}
