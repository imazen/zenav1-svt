//! INTER-frame MVP (motion-vector-predictor) stack — the general
//! (`ref_frame > INTRA_FRAME`) branch of SVT's reference-MV list machinery
//! (inter campaign chunk C2, `docs/INTER-ENCODE-PLAN.md` §2).
//!
//! [`crate::intrabc_mvp`] holds the same machinery restricted to
//! `INTRA_FRAME` on a KEY frame, where the temporal-MVP block, the
//! compound arms, the sign-bias flips and the global-motion substitution
//! are all structurally unreachable. This module is the branch those
//! restrictions cut away: single AND compound reference pairs, the
//! temporal (MFMV) candidates, global-motion candidates, and the
//! `ref_frame_sign_bias` flips in the light rescan.
//!
//! C sources (SVT-AV1 v4.2.0, transcribed line-for-line):
//!
//! | this module | C |
//! |---|---|
//! | [`add_ref_mv_candidate`] | `adaptive_mv_pred.c:57-128` (BOTH arms) |
//! | [`scan_row_mbmi`] / [`scan_col_mbmi`] / [`scan_blk_mbmi`] | `:130-264` |
//! | [`has_top_right`] | `:266-325` |
//! | [`get_relative_dist`] | `:335-350` |
//! | [`add_tpl_ref_mv`] | `:352-448` |
//! | [`scan_row_col_light`] | `:469-648` (BOTH arms) |
//! | [`setup_ref_mv_list`] | `:651-971` (INCLUDING the `:756-860` MFMV block) |
//! | [`gm_get_motion_vector_enc`] | `:983-1036` |
//! | [`compute_inter_mode_ctx_light`] | `:1138-1327` |
//! | [`generate_av1_mvp_table`] | `:1329-1405` (inter path) |
//! | [`get_av1_mv_pred_drl`] | `:1407-1457` |
//! | [`get_ref_mv_from_stack`] / [`find_best_ref_mvs_from_stack`] | `:2002-2040` |
//! | [`get_mv_projection`] / [`lower_mv_precision`] / [`integer_mv_precision`] / [`check_sb_border`] | `inter_prediction.h:203-266` |
//! | [`av1_set_ref_frame`] / [`av1_ref_frame_type`] / [`get_list_idx`] / [`get_ref_frame_idx`] / [`is_global_mv_block`] | `inter_prediction.h:411-545` |
//! | [`get_block_position`] / [`motion_field_projection`] / [`setup_motion_field`] | `md_config_process.c:396-580` |
//!
//! Reuse vs duplication (deliberate, per the chunk's file-ownership rule):
//! [`crate::intrabc_mvp`]'s `MvpMiEntry`, `MvpGrid`, `MvpBlockCtx`,
//! `derive_block_ctx`, `sort_mvp_table`, `REF_CAT_LEVEL`,
//! `MAX_MV_REF_CANDIDATES` and `INVALID_MV` are `pub` and are USED here —
//! the mode-info grid and the block context are the same C structs. The
//! neighbour helpers that module keeps private (`clamp_mv_ref`,
//! `is_inside`, `has_top_right`, `find_valid_*_offset`, `is_inter_block`,
//! `have_newmv_in_inter_mode`) are re-transcribed here rather than
//! refactored out of it, so that lane stays byte-stable.
//!
//! Evidence: tier 1 — `tests/c_parity_inter_mvp.rs` drives the EXPORTED
//! `setup_ref_mv_list`, `svt_aom_gm_get_motion_vector_enc`,
//! `svt_aom_compute_inter_mode_ctx_light`, `svt_aom_get_av1_mv_pred_drl`
//! and `svt_av1_find_best_ref_mvs_from_stack` through new shims. The
//! motion-field projection ([`motion_field_projection`],
//! [`setup_motion_field`], [`get_block_position`]) is `static` in
//! `md_config_process.c` with no exported symbol: it is covered by
//! hand-derived vectors traced against the C source and is labelled
//! **tier 4** in its tests.

use crate::intrabc::TileMiBounds;
use crate::intrabc_mvp::{
    INVALID_MV, MAX_MV_REF_CANDIDATES, MvpBlockCtx, MvpGrid, MvpMiEntry, REF_CAT_LEVEL,
    sort_mvp_table,
};
use svtav1_types::motion::{
    CandidateMv, MAX_REF_MV_STACK_SIZE, Mv, TransformationType, WarpedMotionParams,
};
use svtav1_types::tables::block::{
    BLOCK_SIZE_HIGH, BLOCK_SIZE_WIDE, NUM_4X4_BLOCKS_HIGH, NUM_4X4_BLOCKS_WIDE,
};

// ---------------------------------------------------------------------------
// Constants (C definitions.h / cabac_context_model.h / inter_prediction.h)
// ---------------------------------------------------------------------------

/// C `MVREF_ROWS` / `MVREF_COLS` (adaptive_mv_pred.c:30-31).
const MVREF_ROWS: i32 = 3;
/// C `MV_BORDER` (inter_prediction.h:31): 16 pels in 1/8-pel units.
const MV_BORDER: i32 = 16 << 3;
/// C `REFMV_OFFSET` / `GLOBALMV_OFFSET` (definitions.h:1345-1346).
const REFMV_OFFSET: i16 = 4;
const GLOBALMV_OFFSET: i16 = 3;
/// C `MAX_FRAME_DISTANCE` (definitions.h:405): `(1 << FRAME_OFFSET_BITS) - 1`.
const MAX_FRAME_DISTANCE: i32 = 31;
/// C `MV_UPP` / `MV_LOW` (cabac_context_model.h:198-199), `MV_IN_USE_BITS = 14`.
const MV_UPP: i32 = 1 << 14;
const MV_LOW: i32 = -(1 << 14);
/// C `GM_TRANS_ONLY_PREC_DIFF` (definitions.h:1741) = `WARPEDMODEL_PREC_BITS - 3`.
const GM_TRANS_ONLY_PREC_DIFF: u32 = 16 - 3;
/// C `WARPEDMODEL_PREC_BITS` (definitions.h).
const WARPEDMODEL_PREC_BITS: u32 = 16;
/// C `MAX_OFFSET_WIDTH` / `MAX_OFFSET_HEIGHT` (md_config_process.c:33-34).
const MAX_OFFSET_WIDTH: i32 = 64;
const MAX_OFFSET_HEIGHT: i32 = 0;
/// C `MFMV_STACK_SIZE` (md_config_process.c:35).
const MFMV_STACK_SIZE: i32 = 3;

// Reference-frame enum (definitions.h:1378-1404).
pub const NONE_FRAME: i8 = -1;
pub const INTRA_FRAME: i8 = 0;
pub const LAST_FRAME: i8 = 1;
pub const LAST2_FRAME: i8 = 2;
pub const LAST3_FRAME: i8 = 3;
pub const GOLDEN_FRAME: i8 = 4;
pub const BWDREF_FRAME: i8 = 5;
pub const ALTREF2_FRAME: i8 = 6;
pub const ALTREF_FRAME: i8 = 7;
/// C `REF_FRAMES` (== 8) and `TOTAL_REFS_PER_FRAME` (== 8).
pub const REF_FRAMES: usize = 8;
pub const TOTAL_REFS_PER_FRAME: usize = 8;
/// C `INTER_REFS_PER_FRAME` (== 7).
pub const INTER_REFS_PER_FRAME: usize = 7;
/// C `LAST_BWD_FRAME` (definitions.h:1412).
pub const LAST_BWD_FRAME: i8 = 8;
/// C `FWD_REFS` / `BWD_REFS`.
const FWD_REFS: i8 = 4;
/// C `TOTAL_UNIDIR_COMP_REFS` (definitions.h:1417-1431).
const TOTAL_UNIDIR_COMP_REFS: usize = 9;
/// C `MODE_CTX_REF_FRAMES` = `TOTAL_REFS_PER_FRAME + FWD*BWD + UNIDIR` = 8 + 12 + 9.
pub const MODE_CTX_REF_FRAMES: usize = 29;

// Prediction modes used by name here (definitions.h:1189-1215).
pub const NEARESTMV: u8 = 13;
pub const NEARMV: u8 = 14;
pub const GLOBALMV: u8 = 15;
pub const NEWMV: u8 = 16;
pub const NEAREST_NEARESTMV: u8 = 17;
pub const NEAR_NEWMV: u8 = 21;
pub const NEW_NEARMV: u8 = 22;
pub const GLOBAL_GLOBALMV: u8 = 23;
pub const NEW_NEWMV: u8 = 24;
/// C `MB_MODE_COUNT` — the "not a compound mode" sentinel in the
/// `compound_ref{0,1}_mode` LUTs.
pub const MB_MODE_COUNT: u8 = 25;

/// C `div_mult` (inter_prediction.h:199-201).
const DIV_MULT: [i32; 32] = [
    0, 16384, 8192, 5461, 4096, 3276, 2730, 2340, 2048, 1820, 1638, 1489, 1365, 1260, 1170, 1092,
    1024, 963, 910, 862, 819, 780, 744, 712, 682, 655, 630, 606, 585, 564, 546, 528,
];

/// C `ref_frame_map` (inter_prediction.h:490-511), the compound-pair LUT.
const REF_FRAME_MAP: [[i8; 2]; 21] = [
    [LAST_FRAME, BWDREF_FRAME],
    [LAST2_FRAME, BWDREF_FRAME],
    [LAST3_FRAME, BWDREF_FRAME],
    [GOLDEN_FRAME, BWDREF_FRAME],
    [LAST_FRAME, ALTREF2_FRAME],
    [LAST2_FRAME, ALTREF2_FRAME],
    [LAST3_FRAME, ALTREF2_FRAME],
    [GOLDEN_FRAME, ALTREF2_FRAME],
    [LAST_FRAME, ALTREF_FRAME],
    [LAST2_FRAME, ALTREF_FRAME],
    [LAST3_FRAME, ALTREF_FRAME],
    [GOLDEN_FRAME, ALTREF_FRAME],
    [LAST_FRAME, LAST2_FRAME],
    [LAST_FRAME, LAST3_FRAME],
    [LAST_FRAME, GOLDEN_FRAME],
    [BWDREF_FRAME, ALTREF_FRAME],
    [LAST2_FRAME, LAST3_FRAME],
    [LAST2_FRAME, GOLDEN_FRAME],
    [LAST3_FRAME, GOLDEN_FRAME],
    [BWDREF_FRAME, ALTREF2_FRAME],
    [ALTREF2_FRAME, ALTREF_FRAME],
];

/// C `comp_ref0` LUT (inter_prediction.h:422-436).
const COMP_REF0: [i8; TOTAL_UNIDIR_COMP_REFS] = [
    LAST_FRAME,
    LAST_FRAME,
    LAST_FRAME,
    BWDREF_FRAME,
    LAST2_FRAME,
    LAST2_FRAME,
    LAST3_FRAME,
    BWDREF_FRAME,
    ALTREF2_FRAME,
];
/// C `comp_ref1` LUT (inter_prediction.h:438-452).
const COMP_REF1: [i8; TOTAL_UNIDIR_COMP_REFS] = [
    LAST2_FRAME,
    LAST3_FRAME,
    GOLDEN_FRAME,
    ALTREF_FRAME,
    LAST3_FRAME,
    GOLDEN_FRAME,
    GOLDEN_FRAME,
    ALTREF2_FRAME,
    ALTREF_FRAME,
];

/// C `ref_type_to_list_idx` (inter_prediction.h:531).
const REF_TYPE_TO_LIST_IDX: [u8; 8] = [0, 0, 0, 0, 0, 1, 1, 1];
/// C `ref_type_to_ref_idx` (inter_prediction.h:537).
const REF_TYPE_TO_REF_IDX: [u8; 8] = [0, 0, 1, 2, 3, 0, 1, 2];

/// C `compound_ref0_mode` LUT (inter_prediction.h:341-372).
const COMPOUND_REF0_MODE: [u8; 25] = [
    MB_MODE_COUNT,
    MB_MODE_COUNT,
    MB_MODE_COUNT,
    MB_MODE_COUNT,
    MB_MODE_COUNT,
    MB_MODE_COUNT,
    MB_MODE_COUNT,
    MB_MODE_COUNT,
    MB_MODE_COUNT,
    MB_MODE_COUNT,
    MB_MODE_COUNT,
    MB_MODE_COUNT,
    MB_MODE_COUNT,
    MB_MODE_COUNT,
    MB_MODE_COUNT,
    MB_MODE_COUNT,
    MB_MODE_COUNT,
    NEARESTMV,
    NEARMV,
    NEARESTMV,
    NEWMV,
    NEARMV,
    NEWMV,
    GLOBALMV,
    NEWMV,
];
/// C `compound_ref1_mode` LUT (inter_prediction.h:374-405).
const COMPOUND_REF1_MODE: [u8; 25] = [
    MB_MODE_COUNT,
    MB_MODE_COUNT,
    MB_MODE_COUNT,
    MB_MODE_COUNT,
    MB_MODE_COUNT,
    MB_MODE_COUNT,
    MB_MODE_COUNT,
    MB_MODE_COUNT,
    MB_MODE_COUNT,
    MB_MODE_COUNT,
    MB_MODE_COUNT,
    MB_MODE_COUNT,
    MB_MODE_COUNT,
    MB_MODE_COUNT,
    MB_MODE_COUNT,
    MB_MODE_COUNT,
    MB_MODE_COUNT,
    NEARESTMV,
    NEARMV,
    NEWMV,
    NEARESTMV,
    NEWMV,
    NEARMV,
    GLOBALMV,
    NEWMV,
];

// ---------------------------------------------------------------------------
// Small helpers transcribed from C
// ---------------------------------------------------------------------------

/// C `ROUND_POWER_OF_TWO_SIGNED` (definitions.h:481-482).
#[inline]
fn round_power_of_two_signed(value: i32, n: u32) -> i32 {
    #[inline]
    fn rpot(v: i32, n: u32) -> i32 {
        (v.wrapping_add((1i32 << n) >> 1)) >> n
    }
    if value < 0 {
        -rpot(-value, n)
    } else {
        rpot(value, n)
    }
}

/// C `clamp` (an `int32_t` clamp; `low <= high` is a C precondition and the
/// callers here all satisfy it).
#[inline]
fn clamp_i32(v: i32, low: i32, high: i32) -> i32 {
    if v < low {
        low
    } else if v > high {
        high
    } else {
        v
    }
}

/// C `is_inter_block` (block_structures.h:119-121).
#[inline]
fn is_inter_block(e: &MvpMiEntry) -> bool {
    e.use_intrabc || e.ref_frame[0] > INTRA_FRAME
}

/// C `svt_aom_have_newmv_in_inter_mode` — the NEWMV family.
#[inline]
fn have_newmv_in_inter_mode(mode: u8) -> bool {
    matches!(mode, 16 | 19 | 20 | 21 | 22 | 24)
}

/// C `is_motion_variation_allowed_bsize` (inter_prediction.h:407-409).
#[inline]
fn is_motion_variation_allowed_bsize(bsize: usize) -> bool {
    BLOCK_SIZE_WIDE[bsize] >= 8 && BLOCK_SIZE_HIGH[bsize] >= 8
}

/// C `is_global_mv_block` (inter_prediction.h:411-414).
#[inline]
pub fn is_global_mv_block(mode: u8, bsize: usize, wm_type: TransformationType) -> bool {
    (mode == GLOBALMV || mode == GLOBAL_GLOBALMV)
        && (wm_type as u8) > (TransformationType::Translation as u8)
        && is_motion_variation_allowed_bsize(bsize)
}

/// C `av1_set_ref_frame` (inter_prediction.h:513-522): expand a
/// `MvReferenceFrame` type into the `{rf0, rf1}` pair.
#[inline]
pub fn av1_set_ref_frame(ref_frame_type: i8) -> [i8; 2] {
    if ref_frame_type >= TOTAL_REFS_PER_FRAME as i8 {
        REF_FRAME_MAP[(ref_frame_type as usize) - TOTAL_REFS_PER_FRAME]
    } else {
        [ref_frame_type, NONE_FRAME]
    }
}

/// C `get_uni_comp_ref_idx` (inter_prediction.h:463-471).
#[inline]
fn get_uni_comp_ref_idx(rf: [i8; 2]) -> i32 {
    for idx in 0..TOTAL_UNIDIR_COMP_REFS {
        if rf[0] == COMP_REF0[idx] && rf[1] == COMP_REF1[idx] {
            return idx as i32;
        }
    }
    -1
}

/// C `av1_ref_frame_type` (inter_prediction.h:473-485): collapse a
/// `{rf0, rf1}` pair back into the `MvReferenceFrame` type index.
#[inline]
pub fn av1_ref_frame_type(rf: [i8; 2]) -> i8 {
    if rf[1] > INTRA_FRAME {
        let uni = get_uni_comp_ref_idx(rf);
        if uni >= 0 {
            // TOTAL_REFS_PER_FRAME + FWD_REFS * BWD_REFS + uni
            (TOTAL_REFS_PER_FRAME as i32 + 12 + uni) as i8
        } else {
            // TOTAL_REFS_PER_FRAME + FWD_RF_OFFSET(rf0) + BWD_RF_OFFSET(rf1) * FWD_REFS
            TOTAL_REFS_PER_FRAME as i8 + (rf[0] - LAST_FRAME) + (rf[1] - BWDREF_FRAME) * FWD_REFS
        }
    } else {
        rf[0]
    }
}

/// C `get_list_idx` (inter_prediction.h:533-535).
#[inline]
pub fn get_list_idx(ref_type: i8) -> usize {
    REF_TYPE_TO_LIST_IDX[ref_type as usize] as usize
}

/// C `get_ref_frame_idx` (inter_prediction.h:539-541).
#[inline]
pub fn get_ref_frame_idx(ref_type: i8) -> usize {
    REF_TYPE_TO_REF_IDX[ref_type as usize] as usize
}

/// C `integer_mv_precision` (inter_prediction.h:203-227).
pub fn integer_mv_precision(mv: &mut Mv) {
    let adjust = |v: &mut i16| {
        let m = i32::from(*v) % 8;
        if m != 0 {
            let mut nv = i32::from(*v) - m;
            if m.abs() > 4 {
                if m > 0 {
                    nv += 8;
                } else {
                    nv -= 8;
                }
            }
            *v = nv as i16;
        }
    };
    adjust(&mut mv.y);
    adjust(&mut mv.x);
}

/// C `lower_mv_precision` (inter_prediction.h:229-243).
pub fn lower_mv_precision(mv: &mut Mv, allow_hp: bool, is_integer: bool) {
    if is_integer {
        integer_mv_precision(mv);
    } else if !allow_hp {
        if mv.y & 1 != 0 {
            mv.y += if mv.y > 0 { -1 } else { 1 };
        }
        if mv.x & 1 != 0 {
            mv.x += if mv.x > 0 { -1 } else { 1 };
        }
    }
}

/// C `get_mv_projection` (inter_prediction.h:244-253).
///
/// The C product `ref.y * num * div_mult[den]` is evaluated in `int`
/// (32-bit) and can overflow for large MVs — `wrapping_mul` reproduces
/// what the compiled C does on both supported ISAs rather than trapping.
/// `den` is a `uint8_t` in every caller, so the table index is in range
/// after the `AOMMIN(den, MAX_FRAME_DISTANCE)`.
pub fn get_mv_projection(reference: Mv, num: i32, den: i32) -> Mv {
    // C is `AOMMIN(den, MAX_FRAME_DISTANCE)`; every caller passes a
    // `uint8_t`, so the lower bound only makes the table index total.
    let den = den.clamp(0, MAX_FRAME_DISTANCE);
    let num = if num > 0 {
        num.min(MAX_FRAME_DISTANCE)
    } else {
        num.max(-MAX_FRAME_DISTANCE)
    };
    let mult = DIV_MULT[den as usize];
    let mv_row = round_power_of_two_signed(
        i32::from(reference.y).wrapping_mul(num).wrapping_mul(mult),
        14,
    );
    let mv_col = round_power_of_two_signed(
        i32::from(reference.x).wrapping_mul(num).wrapping_mul(mult),
        14,
    );
    let clamp_max = MV_UPP - 1;
    let clamp_min = MV_LOW + 1;
    Mv {
        y: clamp_i32(mv_row, clamp_min, clamp_max) as i16,
        x: clamp_i32(mv_col, clamp_min, clamp_max) as i16,
    }
}

/// C `check_sb_border` (inter_prediction.h:255-266). Note the 64x64 SB
/// grid is hard-coded in C (`mi_size_wide[BLOCK_64X64]`), independent of
/// the sequence `sb_size`.
#[inline]
pub fn check_sb_border(mi_row: i32, mi_col: i32, row_offset: i32, col_offset: i32) -> bool {
    let sb_mi_size = 16i32;
    let row = mi_row & (sb_mi_size - 1);
    let col = mi_col & (sb_mi_size - 1);
    !(row + row_offset < 0
        || row + row_offset >= sb_mi_size
        || col + col_offset < 0
        || col + col_offset >= sb_mi_size)
}

/// C `get_relative_dist` (adaptive_mv_pred.c:335-350 and the identical
/// copy at md_config_process.c:379-394).
#[inline]
pub fn get_relative_dist(oh: OrderHintInfo, a: i32, b: i32) -> i32 {
    if !oh.enable_order_hint {
        return 0;
    }
    let bits = oh.order_hint_bits;
    let mut diff = a - b;
    let m = 1i32 << (bits - 1);
    diff = (diff & (m - 1)) - (diff & m);
    diff
}

/// C `convert_to_trans_prec` (utility.h:234-240).
#[inline]
fn convert_to_trans_prec(allow_hp: bool, coor: i32) -> i32 {
    if allow_hp {
        round_power_of_two_signed(coor, WARPEDMODEL_PREC_BITS - 3)
    } else {
        round_power_of_two_signed(coor, WARPEDMODEL_PREC_BITS - 2) * 2
    }
}

/// C `block_center_x` (adaptive_mv_pred.c:973-976).
#[inline]
fn block_center_x(mi_col: i32, bsize: usize) -> i32 {
    mi_col * 4 + i32::from(BLOCK_SIZE_WIDE[bsize]) / 2 - 1
}

/// C `block_center_y` (adaptive_mv_pred.c:978-981).
#[inline]
fn block_center_y(mi_row: i32, bsize: usize) -> i32 {
    mi_row * 4 + i32::from(BLOCK_SIZE_HIGH[bsize]) / 2 - 1
}

/// C `svt_aom_gm_get_motion_vector_enc` (adaptive_mv_pred.c:983-1036,
/// EXPORTED). The `TRANSLATION` arm keeps the spec's x/y swap
/// (crbug.com/aomedia/3328) verbatim — it is the oracle.
pub fn gm_get_motion_vector_enc(
    gm: &WarpedMotionParams,
    allow_hp: bool,
    bsize: usize,
    mi_col: i32,
    mi_row: i32,
    is_integer: bool,
) -> Mv {
    let mut res = Mv::default();
    if gm.wm_type == TransformationType::Identity {
        return res;
    }
    if gm.wm_type == TransformationType::Translation {
        res.y = (gm.wmmat[0] >> GM_TRANS_ONLY_PREC_DIFF) as i16;
        res.x = (gm.wmmat[1] >> GM_TRANS_ONLY_PREC_DIFF) as i16;
    } else {
        let mat = &gm.wmmat;
        let x = block_center_x(mi_col, bsize);
        let y = block_center_y(mi_row, bsize);
        let one = 1i32 << WARPEDMODEL_PREC_BITS;
        let xc = mat[2]
            .wrapping_sub(one)
            .wrapping_mul(x)
            .wrapping_add(mat[3].wrapping_mul(y))
            .wrapping_add(mat[0]);
        let yc = mat[4]
            .wrapping_mul(x)
            .wrapping_add(mat[5].wrapping_sub(one).wrapping_mul(y))
            .wrapping_add(mat[1]);
        res.y = convert_to_trans_prec(allow_hp, yc) as i16;
        res.x = convert_to_trans_prec(allow_hp, xc) as i16;
    }
    if is_integer {
        integer_mv_precision(&mut res);
    }
    res
}

// ---------------------------------------------------------------------------
// Frame-level environment the inter branch reads
// ---------------------------------------------------------------------------

/// C `OrderHintInfo` (av1_structs.h) — the two fields the MVP path reads.
#[derive(Debug, Clone, Copy, Default)]
pub struct OrderHintInfo {
    pub enable_order_hint: bool,
    /// C `order_hint_bits`; `>= 1` when `enable_order_hint`.
    pub order_hint_bits: u32,
}

/// C `TPL_MV_REF` (coding_unit.h:39-42) — one temporal MV field cell.
#[derive(Debug, Clone, Copy)]
pub struct TplMvRef {
    pub mfmv0: Mv,
    pub ref_frame_offset: u8,
}

impl Default for TplMvRef {
    /// C `av1_setup_motion_field`'s reset (md_config_process.c:542-545):
    /// `INVALID_MV` and a zero offset.
    fn default() -> Self {
        Self {
            mfmv0: Mv::from_int(INVALID_MV),
            ref_frame_offset: 0,
        }
    }
}

/// C `MV_REF` (coding_unit.h) — one cell of a reference frame's saved
/// motion field, read by [`motion_field_projection`].
#[derive(Debug, Clone, Copy, Default)]
pub struct MvRef {
    pub mv: Mv,
    pub ref_frame: i8,
}

/// The frame-level inputs `setup_ref_mv_list`'s inter branch reads off
/// `PictureControlSet` / `PictureParentControlSet` / `Av1Common`.
#[derive(Debug, Clone, Copy)]
pub struct InterMvpEnv<'a> {
    /// C `pcs->ppcs->global_motion[TOTAL_REFS_PER_FRAME]`.
    pub global_motion: &'a [WarpedMotionParams; TOTAL_REFS_PER_FRAME],
    /// C `cm->ref_frame_sign_bias[TOTAL_REFS_PER_FRAME]`.
    pub ref_frame_sign_bias: [u32; TOTAL_REFS_PER_FRAME],
    /// C `frm_hdr.allow_high_precision_mv`.
    pub allow_high_precision_mv: bool,
    /// C `frm_hdr.force_integer_mv`.
    pub force_integer_mv: bool,
    /// C `frm_hdr.use_ref_frame_mvs` — gates the whole MFMV block.
    pub use_ref_frame_mvs: bool,
    /// C `pcs->ppcs->scs->seq_header.order_hint_info`.
    pub order_hint_info: OrderHintInfo,
    /// C `pcs->ppcs->cur_order_hint`.
    pub cur_order_hint: i32,
    /// Order hints of the current frame's references, indexed by
    /// `MvReferenceFrame` (`LAST_FRAME=1 ..= ALTREF_FRAME=7`); slot 0 is
    /// unused. C reaches these through
    /// `pcs->ref_pic_ptr_array[list][idx]->object_ptr->order_hint`.
    pub ref_order_hint: [i32; REF_FRAMES],
    /// C `pcs->tpl_mvs`, the temporal MV field.
    pub tpl_mvs: &'a [TplMvRef],
    /// C `cm->mi_stride >> 1` — the tpl field's row stride.
    pub tpl_stride: i32,
    /// C `ctx->sb64_sq_no4xn_geom` — selects the simplified MFMV block
    /// walk (64x64 SB, square, no 4xN).
    pub sb64_sq_no4xn_geom: bool,
    /// C `symteric_refs` (sic) — the LAST/BWD symmetric-projection
    /// shortcut, set by `generate_av1_mvp_table` (:1339-1347).
    pub symmetric_refs: bool,
}

// ---------------------------------------------------------------------------
// Neighbour scans (adaptive_mv_pred.c:49-264)
// ---------------------------------------------------------------------------

/// C `clamp_mv_ref` (adaptive_mv_pred.c:49-55): clamp into the block's
/// UMV border box (`bw_px`/`bh_px` in PIXELS = `n8 << MI_SIZE_LOG2`).
fn clamp_mv_ref(mv: &mut Mv, bw_px: i32, bh_px: i32, ctx: &MvpBlockCtx) {
    mv.x = clamp_i32(
        i32::from(mv.x),
        ctx.mb_to_left_edge - bw_px * 8 - MV_BORDER,
        ctx.mb_to_right_edge + bw_px * 8 + MV_BORDER,
    ) as i16;
    mv.y = clamp_i32(
        i32::from(mv.y),
        ctx.mb_to_top_edge - bh_px * 8 - MV_BORDER,
        ctx.mb_to_bottom_edge + bh_px * 8 + MV_BORDER,
    ) as i16;
}

/// C `is_inside` (adaptive_mv_pred.c:44-47).
#[inline]
fn is_inside(tile: TileMiBounds, mi_col: i32, mi_row: i32, pos_row: i32, pos_col: i32) -> bool {
    !(mi_row + pos_row < tile.mi_row_start
        || mi_col + pos_col < tile.mi_col_start
        || mi_row + pos_row >= tile.mi_row_end
        || mi_col + pos_col >= tile.mi_col_end)
}

/// Read a neighbour cell at `offset` from the block's top-left mi cell —
/// C's `xd->mi[offset]`.
#[inline]
fn cell_at<'g>(grid: &'g MvpGrid<'g>, offset: i32) -> &'g MvpMiEntry {
    &grid.entries[(grid.base + offset) as usize]
}

/// C `add_ref_mv_candidate` (adaptive_mv_pred.c:57-128) — BOTH arms.
#[allow(clippy::too_many_arguments)]
fn add_ref_mv_candidate(
    candidate: &MvpMiEntry,
    rf: [i8; 2],
    refmv_count: &mut u8,
    ref_match_count: &mut u8,
    newmv_count: &mut u8,
    ref_mv_stack: &mut [CandidateMv; MAX_REF_MV_STACK_SIZE],
    len: i32,
    gm_mv_candidates: [Mv; 2],
    gm_params: &[WarpedMotionParams; TOTAL_REFS_PER_FRAME],
    weight: i32,
) {
    if !is_inter_block(candidate) {
        return; // for intrabc
    }
    // C has `assert(weight % 2 == 0)` here (:63). It does NOT hold, and is
    // only invisible because the reference ships with NDEBUG: with
    // `row_adj = 1` (an 8x4 block at an odd `mi_row`) `max_row_offset`
    // becomes -5, so `scan_row_mbmi`'s `inc = AOMMIN(-max_row_offset +
    // row_offset + 1, mi_size_high[cand])` is 5 for a candidate 8 or 16 mi
    // tall, and `weight = AOMMAX(2, inc)` is 5. Measured: it fires on the
    // randomized grids in `tests/c_parity_inter_mvp.rs`. The assert is
    // therefore NOT transcribed — an odd weight is a legal input and the
    // arithmetic below is unaffected by it.

    if rf[1] == NONE_FRAME {
        // single reference frame
        for r in 0..2usize {
            if candidate.ref_frame[r] == rf[0] {
                let this_refmv = if is_global_mv_block(
                    candidate.mode,
                    usize::from(candidate.bsize),
                    gm_params[rf[0] as usize].wm_type,
                ) {
                    gm_mv_candidates[0]
                } else {
                    candidate.mv[r]
                };
                let mut index = usize::from(*refmv_count);
                for (i, entry) in ref_mv_stack
                    .iter_mut()
                    .enumerate()
                    .take(usize::from(*refmv_count))
                {
                    if entry.this_mv.as_int() == this_refmv.as_int() {
                        entry.weight += weight * len;
                        index = i;
                        break;
                    }
                }
                if index == usize::from(*refmv_count)
                    && usize::from(*refmv_count) < MAX_REF_MV_STACK_SIZE
                {
                    ref_mv_stack[index].this_mv = this_refmv;
                    ref_mv_stack[index].weight = weight * len;
                    *refmv_count += 1;
                }
                if have_newmv_in_inter_mode(candidate.mode) {
                    *newmv_count += 1;
                }
                *ref_match_count += 1;
            }
        }
    } else {
        // compound reference frame
        if candidate.ref_frame[0] == rf[0] && candidate.ref_frame[1] == rf[1] {
            let mut this_refmv = [Mv::default(); 2];
            for r in 0..2usize {
                this_refmv[r] = if is_global_mv_block(
                    candidate.mode,
                    usize::from(candidate.bsize),
                    gm_params[rf[r] as usize].wm_type,
                ) {
                    gm_mv_candidates[r]
                } else {
                    candidate.mv[r]
                };
            }
            let mut index = usize::from(*refmv_count);
            for (i, entry) in ref_mv_stack
                .iter_mut()
                .enumerate()
                .take(usize::from(*refmv_count))
            {
                if entry.this_mv.as_int() == this_refmv[0].as_int()
                    && entry.comp_mv.as_int() == this_refmv[1].as_int()
                {
                    entry.weight += weight * len;
                    index = i;
                    break;
                }
            }
            if index == usize::from(*refmv_count)
                && usize::from(*refmv_count) < MAX_REF_MV_STACK_SIZE
            {
                ref_mv_stack[index].this_mv = this_refmv[0];
                ref_mv_stack[index].comp_mv = this_refmv[1];
                ref_mv_stack[index].weight = weight * len;
                *refmv_count += 1;
            }
            if have_newmv_in_inter_mode(candidate.mode) {
                *newmv_count += 1;
            }
            *ref_match_count += 1;
        }
    }
}

/// C `scan_row_mbmi` (adaptive_mv_pred.c:130-184).
#[allow(clippy::too_many_arguments)]
fn scan_row_mbmi(
    grid: &MvpGrid,
    ctx: &MvpBlockCtx,
    rf: [i8; 2],
    row_offset: i32,
    ref_mv_stack: &mut [CandidateMv; MAX_REF_MV_STACK_SIZE],
    refmv_count: &mut u8,
    ref_match_count: &mut u8,
    newmv_count: &mut u8,
    gm_mv_candidates: [Mv; 2],
    gm_params: &[WarpedMotionParams; TOTAL_REFS_PER_FRAME],
    max_row_offset: i32,
    processed_rows: &mut i32,
) {
    let mut end_mi = ctx.n8_w.min(ctx.mi_cols - ctx.mi_col);
    end_mi = end_mi.min(16); // mi_size_wide[BLOCK_64X64]
    let n8_w_8 = 2i32;
    let n8_w_16 = 4i32;
    let mut col_offset = 0i32;
    if row_offset.abs() > 1 {
        col_offset = 1;
        if ctx.mi_col & 1 != 0 && ctx.n8_w < n8_w_8 {
            col_offset -= 1;
        }
    }
    let use_step_16 = ctx.n8_w >= 16;

    let mut i = 0i32;
    while i < end_mi {
        let candidate = cell_at(grid, row_offset * grid.stride + col_offset + i);
        let cand_bsize = usize::from(candidate.bsize);
        let n8_w = i32::from(NUM_4X4_BLOCKS_WIDE[cand_bsize]);
        let mut len = ctx.n8_w.min(n8_w);
        if use_step_16 {
            len = n8_w_16.max(len);
        } else if row_offset.abs() > 1 {
            len = len.max(n8_w_8);
        }

        let mut weight = 2i32;
        if ctx.n8_w >= n8_w_8 && ctx.n8_w <= n8_w {
            let inc =
                (-max_row_offset + row_offset + 1).min(i32::from(NUM_4X4_BLOCKS_HIGH[cand_bsize]));
            weight = weight.max(inc); // << shift(0)
            *processed_rows = inc - row_offset - 1;
        }

        add_ref_mv_candidate(
            candidate,
            rf,
            refmv_count,
            ref_match_count,
            newmv_count,
            ref_mv_stack,
            len,
            gm_mv_candidates,
            gm_params,
            weight,
        );
        i += len;
    }
}

/// C `scan_col_mbmi` (adaptive_mv_pred.c:186-239).
#[allow(clippy::too_many_arguments)]
fn scan_col_mbmi(
    grid: &MvpGrid,
    ctx: &MvpBlockCtx,
    rf: [i8; 2],
    col_offset: i32,
    ref_mv_stack: &mut [CandidateMv; MAX_REF_MV_STACK_SIZE],
    refmv_count: &mut u8,
    ref_match_count: &mut u8,
    newmv_count: &mut u8,
    gm_mv_candidates: [Mv; 2],
    gm_params: &[WarpedMotionParams; TOTAL_REFS_PER_FRAME],
    max_col_offset: i32,
    processed_cols: &mut i32,
) {
    let mut end_mi = ctx.n8_h.min(ctx.mi_rows - ctx.mi_row);
    end_mi = end_mi.min(16); // mi_size_high[BLOCK_64X64]
    let n8_h_8 = 2i32;
    let n8_h_16 = 4i32;
    let mut row_offset = 0i32;
    if col_offset.abs() > 1 {
        row_offset = 1;
        if ctx.mi_row & 1 != 0 && ctx.n8_h < n8_h_8 {
            row_offset -= 1;
        }
    }
    let use_step_16 = ctx.n8_h >= 16;

    let mut i = 0i32;
    while i < end_mi {
        let candidate = cell_at(grid, (row_offset + i) * grid.stride + col_offset);
        let cand_bsize = usize::from(candidate.bsize);
        let n8_h = i32::from(NUM_4X4_BLOCKS_HIGH[cand_bsize]);
        let mut len = ctx.n8_h.min(n8_h);
        if use_step_16 {
            len = n8_h_16.max(len);
        } else if col_offset.abs() > 1 {
            len = len.max(n8_h_8);
        }

        let mut weight = 2i32;
        if ctx.n8_h >= n8_h_8 && ctx.n8_h <= n8_h {
            let inc =
                (-max_col_offset + col_offset + 1).min(i32::from(NUM_4X4_BLOCKS_WIDE[cand_bsize]));
            weight = weight.max(inc);
            *processed_cols = inc - col_offset - 1;
        }

        add_ref_mv_candidate(
            candidate,
            rf,
            refmv_count,
            ref_match_count,
            newmv_count,
            ref_mv_stack,
            len,
            gm_mv_candidates,
            gm_params,
            weight,
        );
        i += len;
    }
}

/// C `scan_blk_mbmi` (adaptive_mv_pred.c:241-264).
#[allow(clippy::too_many_arguments)]
fn scan_blk_mbmi(
    grid: &MvpGrid,
    ctx: &MvpBlockCtx,
    rf: [i8; 2],
    row_offset: i32,
    col_offset: i32,
    ref_mv_stack: &mut [CandidateMv; MAX_REF_MV_STACK_SIZE],
    ref_match_count: &mut u8,
    newmv_count: &mut u8,
    gm_mv_candidates: [Mv; 2],
    gm_params: &[WarpedMotionParams; TOTAL_REFS_PER_FRAME],
    refmv_count: &mut u8,
) {
    if is_inside(ctx.tile, ctx.mi_col, ctx.mi_row, row_offset, col_offset) {
        let candidate = cell_at(grid, row_offset * grid.stride + col_offset);
        add_ref_mv_candidate(
            candidate,
            rf,
            refmv_count,
            ref_match_count,
            newmv_count,
            ref_mv_stack,
            2, // mi_size_wide[BLOCK_8X8]
            gm_mv_candidates,
            gm_params,
            2,
        );
    }
}

/// C `has_top_right` (adaptive_mv_pred.c:266-325).
///
/// **`bs` is MUTATED by the 4x4-group loop and the `PARTITION_VERT_A`
/// check below reads the MUTATED value** (C `:314-322` runs after the
/// `bs <<= 1` loop at `:303-313`). Carrying the original `bs` into that
/// check is a live divergence, not a cosmetic one: measured at
/// `mi = (36, 10)`, an 8x8 block in a 64x64-mi SB with
/// `partition == PARTITION_VERT_A`, where `bs` enters as 2, the loop
/// advances it to 4 (because `mask_col == 10` has bit 1 set and bit 2
/// clear), and `mask_row == 4` then makes C return 0 while the
/// original-`bs` reading returns 1 — a 4-unit weight difference on
/// `ref_mv_stack[0]`. Found by `tests/c_parity_inter_mvp.rs` against the
/// exported C symbol.
fn has_top_right(grid: &MvpGrid, ctx: &MvpBlockCtx, bs: i32) -> bool {
    if bs > 16 {
        return false;
    }
    if ctx.n8_w > ctx.n8_h && ctx.is_sec_rect {
        return false;
    }
    if ctx.n8_w < ctx.n8_h && !ctx.is_sec_rect {
        return true;
    }

    let sb_mi_size = ctx.sb_mi_size;
    let mask_row = ctx.mi_row & (sb_mi_size - 1);
    let mask_col = ctx.mi_col & (sb_mi_size - 1);

    let mut bs = bs;
    let mut has_tr = !((mask_row & bs != 0) && (mask_col & bs != 0));

    while bs < sb_mi_size {
        if mask_col & bs != 0 {
            if (mask_col & (2 * bs) != 0) && (mask_row & (2 * bs) != 0) {
                has_tr = false;
                break;
            }
        } else {
            break;
        }
        bs <<= 1;
    }

    if cell_at(grid, 0).partition == 6 {
        // PARTITION_VERT_A — reads the MUTATED bs (see the fn doc).
        if ctx.n8_w == ctx.n8_h && mask_row & bs != 0 {
            return false;
        }
    }

    has_tr
}

/// C `find_valid_row_offset` (adaptive_mv_pred.c:327-329).
#[inline]
fn find_valid_row_offset(tile: TileMiBounds, mi_row: i32, row_offset: i32) -> i32 {
    row_offset.clamp(tile.mi_row_start - mi_row, tile.mi_row_end - mi_row - 1)
}

/// C `find_valid_col_offset` (adaptive_mv_pred.c:331-333).
#[inline]
fn find_valid_col_offset(tile: TileMiBounds, mi_col: i32, col_offset: i32) -> i32 {
    col_offset.clamp(tile.mi_col_start - mi_col, tile.mi_col_end - mi_col - 1)
}

// ---------------------------------------------------------------------------
// Temporal MV candidates (adaptive_mv_pred.c:352-448)
// ---------------------------------------------------------------------------

/// C `add_tpl_ref_mv` (adaptive_mv_pred.c:352-448).
///
/// `mv_ref0` is C's rolling `Mv* mv_ref0` cursor: in the `two_symetric_refs`
/// mode the LAST_FRAME pass STORES the projected MV there and the
/// BWDREF/other passes read it back (negated for BWDREF). Returns C's
/// `int` (1 when the cell contributed, 0 when it bailed).
#[allow(clippy::too_many_arguments)]
fn add_tpl_ref_mv(
    env: &InterMvpEnv,
    ctx: &MvpBlockCtx,
    ref_frame: i8,
    blk_row: i32,
    blk_col: i32,
    gm_mv_candidates: [Mv; 2],
    refmv_count: &mut u8,
    mv_ref0: &mut Mv,
    cur_offset_0: i32,
    cur_offset_1: i32,
    ref_mv_stack: &mut [CandidateMv; MAX_REF_MV_STACK_SIZE],
    mode_context: &mut i16,
) -> i32 {
    let mi_row = ctx.mi_row;
    let mi_col = ctx.mi_col;
    let pos_row = if mi_row & 0x01 != 0 {
        blk_row
    } else {
        blk_row + 1
    };
    let pos_col = if mi_col & 0x01 != 0 {
        blk_col
    } else {
        blk_col + 1
    };

    if !is_inside(ctx.tile, mi_col, mi_row, pos_row, pos_col) {
        return 0;
    }

    let idx_tpl = ((mi_row + pos_row) >> 1) * env.tpl_stride + ((mi_col + pos_col) >> 1);
    // C indexes `pcs->tpl_mvs` unconditionally; the field is allocated
    // ((mi_rows + MAX_MIB_SIZE) >> 1) * (mi_stride >> 1) entries, which
    // covers every position `is_inside` admits. A short slice is a caller
    // bug, so it panics rather than reading a neighbouring row.
    let prev_frame_mvs = env.tpl_mvs[idx_tpl as usize];
    if prev_frame_mvs.mfmv0.as_int() == INVALID_MV {
        return 0;
    }

    let weight_unit = 1i32;
    let den = i32::from(prev_frame_mvs.ref_frame_offset);

    let this_refmv;
    if env.symmetric_refs {
        if ref_frame == LAST_FRAME {
            let mut m = get_mv_projection(prev_frame_mvs.mfmv0, cur_offset_0, den);
            lower_mv_precision(&mut m, env.allow_high_precision_mv, false);
            *mv_ref0 = m; // store for future use
            this_refmv = m;
        } else if ref_frame == BWDREF_FRAME {
            this_refmv = Mv {
                x: -mv_ref0.x,
                y: -mv_ref0.y,
            };
        } else {
            this_refmv = *mv_ref0;
        }
    } else {
        let mut m = get_mv_projection(prev_frame_mvs.mfmv0, cur_offset_0, den);
        lower_mv_precision(&mut m, env.allow_high_precision_mv, false);
        this_refmv = m;
    }

    // single ref case could be detected by ref_frame
    if ref_frame < LAST_BWD_FRAME {
        if blk_row == 0 && blk_col == 0 {
            let dy = i32::from(this_refmv.y) - i32::from(gm_mv_candidates[0].y);
            let dx = i32::from(this_refmv.x) - i32::from(gm_mv_candidates[0].x);
            if dy.abs() >= 16 || dx.abs() >= 16 {
                *mode_context |= 1 << GLOBALMV_OFFSET;
            }
        }
        let mut idx = usize::from(*refmv_count);
        for (i, e) in ref_mv_stack
            .iter_mut()
            .enumerate()
            .take(usize::from(*refmv_count))
        {
            if this_refmv.as_int() == e.this_mv.as_int() {
                e.weight += 2 * weight_unit;
                idx = i;
                break;
            }
        }
        if idx == usize::from(*refmv_count) && usize::from(*refmv_count) < MAX_REF_MV_STACK_SIZE {
            ref_mv_stack[idx].this_mv = this_refmv;
            ref_mv_stack[idx].weight = 2 * weight_unit;
            *refmv_count += 1;
        }
    } else {
        // Process compound inter mode
        let comp_refmv = if env.symmetric_refs {
            Mv {
                x: -mv_ref0.x,
                y: -mv_ref0.y,
            }
        } else {
            let mut m = get_mv_projection(prev_frame_mvs.mfmv0, cur_offset_1, den);
            lower_mv_precision(&mut m, env.allow_high_precision_mv, false);
            m
        };

        if blk_row == 0 && blk_col == 0 {
            let d0y = i32::from(this_refmv.y) - i32::from(gm_mv_candidates[0].y);
            let d0x = i32::from(this_refmv.x) - i32::from(gm_mv_candidates[0].x);
            let d1y = i32::from(comp_refmv.y) - i32::from(gm_mv_candidates[1].y);
            let d1x = i32::from(comp_refmv.x) - i32::from(gm_mv_candidates[1].x);
            if d0y.abs() >= 16 || d0x.abs() >= 16 || d1y.abs() >= 16 || d1x.abs() >= 16 {
                *mode_context |= 1 << GLOBALMV_OFFSET;
            }
        }
        let mut idx = usize::from(*refmv_count);
        for (i, e) in ref_mv_stack
            .iter_mut()
            .enumerate()
            .take(usize::from(*refmv_count))
        {
            if this_refmv.as_int() == e.this_mv.as_int()
                && comp_refmv.as_int() == e.comp_mv.as_int()
            {
                e.weight += 2 * weight_unit;
                idx = i;
                break;
            }
        }
        if idx == usize::from(*refmv_count) && usize::from(*refmv_count) < MAX_REF_MV_STACK_SIZE {
            ref_mv_stack[idx].this_mv = this_refmv;
            ref_mv_stack[idx].comp_mv = comp_refmv;
            ref_mv_stack[idx].weight = 2 * weight_unit;
            *refmv_count += 1;
        }
    }

    1
}

// ---------------------------------------------------------------------------
// Light rescan (adaptive_mv_pred.c:469-648) — BOTH arms
// ---------------------------------------------------------------------------

/// C `scan_row_col_light` (adaptive_mv_pred.c:469-648, EXPORTED).
#[allow(clippy::too_many_arguments)]
fn scan_row_col_light(
    grid: &MvpGrid,
    ctx: &MvpBlockCtx,
    env: &InterMvpEnv,
    rf: [i8; 2],
    ref_mv_stack: &mut [CandidateMv; MAX_REF_MV_STACK_SIZE],
    refmv_count: &mut u8,
    gm_mv_candidates: [Mv; 2],
    max_row_offset: i32,
    max_col_offset: i32,
) {
    let mut mi_width = 16i32.min(ctx.n8_w);
    mi_width = mi_width.min(ctx.mi_cols - ctx.mi_col);
    let mut mi_height = 16i32.min(ctx.n8_h);
    mi_height = mi_height.min(ctx.mi_rows - ctx.mi_row);
    let mi_size = mi_width.min(mi_height);

    let sign_bias = |r: i8| -> u32 { env.ref_frame_sign_bias[r as usize] };

    if rf[1] > NONE_FRAME {
        // ---- Multiple ref frames path (:479-577) ----
        let mut ref_id = [[Mv::default(); 2]; 2];
        let mut ref_diff = [[Mv::default(); 2]; 2];
        let mut ref_id_count = [0usize; 2];
        let mut ref_diff_count = [0usize; 2];

        // ROW=-1 rescan with relaxed constraints.
        let mut idx = 0i32;
        while max_row_offset.abs() >= 1 && idx < mi_size {
            let candidate = cell_at(grid, -grid.stride + idx);
            let cand_bsize = usize::from(candidate.bsize);
            for rf_idx in 0..2usize {
                let can_rf = candidate.ref_frame[rf_idx];
                for cmp_idx in 0..2usize {
                    if can_rf == rf[cmp_idx] && ref_id_count[cmp_idx] < 2 {
                        ref_id[cmp_idx][ref_id_count[cmp_idx]] = candidate.mv[rf_idx];
                        ref_id_count[cmp_idx] += 1;
                    } else if can_rf > INTRA_FRAME && ref_diff_count[cmp_idx] < 2 {
                        let mut this_mv = candidate.mv[rf_idx];
                        if sign_bias(can_rf) != sign_bias(rf[cmp_idx]) {
                            this_mv.y = -this_mv.y;
                            this_mv.x = -this_mv.x;
                        }
                        ref_diff[cmp_idx][ref_diff_count[cmp_idx]] = this_mv;
                        ref_diff_count[cmp_idx] += 1;
                    }
                }
            }
            idx += i32::from(NUM_4X4_BLOCKS_WIDE[cand_bsize]);
        }

        // COL=-1 rescan with relaxed constraints.
        let mut idx = 0i32;
        while max_col_offset.abs() >= 1 && idx < mi_size {
            let candidate = cell_at(grid, idx * grid.stride - 1);
            let cand_bsize = usize::from(candidate.bsize);
            for rf_idx in 0..2usize {
                let can_rf = candidate.ref_frame[rf_idx];
                for cmp_idx in 0..2usize {
                    if can_rf == rf[cmp_idx] && ref_id_count[cmp_idx] < 2 {
                        ref_id[cmp_idx][ref_id_count[cmp_idx]] = candidate.mv[rf_idx];
                        ref_id_count[cmp_idx] += 1;
                    } else if can_rf > INTRA_FRAME && ref_diff_count[cmp_idx] < 2 {
                        let mut this_mv = candidate.mv[rf_idx];
                        if sign_bias(can_rf) != sign_bias(rf[cmp_idx]) {
                            this_mv.y = -this_mv.y;
                            this_mv.x = -this_mv.x;
                        }
                        ref_diff[cmp_idx][ref_diff_count[cmp_idx]] = this_mv;
                        ref_diff_count[cmp_idx] += 1;
                    }
                }
            }
            idx += i32::from(NUM_4X4_BLOCKS_HIGH[cand_bsize]);
        }

        // Build up the compound mv predictor (:543-557).
        let mut comp_list = [[Mv::default(); 2]; MAX_MV_REF_CANDIDATES + 1];
        for idx in 0..2usize {
            let mut comp_idx = 0usize;
            let mut list_idx = 0usize;
            while list_idx < ref_id_count[idx] && comp_idx < MAX_MV_REF_CANDIDATES {
                comp_list[comp_idx][idx] = ref_id[idx][list_idx];
                list_idx += 1;
                comp_idx += 1;
            }
            let mut list_idx = 0usize;
            while list_idx < ref_diff_count[idx] && comp_idx < MAX_MV_REF_CANDIDATES {
                comp_list[comp_idx][idx] = ref_diff[idx][list_idx];
                list_idx += 1;
                comp_idx += 1;
            }
            while comp_idx < MAX_MV_REF_CANDIDATES {
                comp_list[comp_idx][idx] = gm_mv_candidates[idx];
                comp_idx += 1;
            }
        }

        // Fill the stack, increment the counter (:559-576).
        if *refmv_count != 0 {
            debug_assert_eq!(*refmv_count, 1);
            let slot = usize::from(*refmv_count);
            if comp_list[0][0].as_int() == ref_mv_stack[0].this_mv.as_int()
                && comp_list[0][1].as_int() == ref_mv_stack[0].comp_mv.as_int()
            {
                ref_mv_stack[slot].this_mv = comp_list[1][0];
                ref_mv_stack[slot].comp_mv = comp_list[1][1];
            } else {
                ref_mv_stack[slot].this_mv = comp_list[0][0];
                ref_mv_stack[slot].comp_mv = comp_list[0][1];
            }
            ref_mv_stack[slot].weight = 2;
            *refmv_count += 1;
        } else {
            for entry in comp_list.iter().take(MAX_MV_REF_CANDIDATES) {
                let slot = usize::from(*refmv_count);
                ref_mv_stack[slot].this_mv = entry[0];
                ref_mv_stack[slot].comp_mv = entry[1];
                ref_mv_stack[slot].weight = 2;
                *refmv_count += 1;
            }
        }
        debug_assert!(*refmv_count >= 2);
    } else {
        // ---- Single reference frame extension (:578-647) ----
        let mut idx = 0i32;
        while max_row_offset.abs() >= 1
            && idx < mi_size
            && usize::from(*refmv_count) < MAX_MV_REF_CANDIDATES
        {
            let candidate = cell_at(grid, -grid.stride + idx);
            let cand_bsize = usize::from(candidate.bsize);
            for r in 0..2usize {
                if candidate.ref_frame[r] > INTRA_FRAME {
                    let mut this_mv = candidate.mv[r];
                    if sign_bias(candidate.ref_frame[r]) != sign_bias(rf[0]) {
                        this_mv.y = -this_mv.y;
                        this_mv.x = -this_mv.x;
                    }
                    let mut stack_idx = usize::from(*refmv_count);
                    for (i, e) in ref_mv_stack
                        .iter()
                        .enumerate()
                        .take(usize::from(*refmv_count))
                    {
                        if this_mv.as_int() == e.this_mv.as_int() {
                            stack_idx = i;
                            break;
                        }
                    }
                    if stack_idx == usize::from(*refmv_count) {
                        ref_mv_stack[stack_idx].this_mv = this_mv;
                        ref_mv_stack[stack_idx].weight = 2;
                        *refmv_count += 1;
                    }
                }
            }
            idx += i32::from(NUM_4X4_BLOCKS_WIDE[cand_bsize]);
        }

        let mut idx = 0i32;
        while max_col_offset.abs() >= 1
            && idx < mi_size
            && usize::from(*refmv_count) < MAX_MV_REF_CANDIDATES
        {
            let candidate = cell_at(grid, idx * grid.stride - 1);
            let cand_bsize = usize::from(candidate.bsize);
            for r in 0..2usize {
                if candidate.ref_frame[r] > INTRA_FRAME {
                    let mut this_mv = candidate.mv[r];
                    if sign_bias(candidate.ref_frame[r]) != sign_bias(rf[0]) {
                        this_mv.y = -this_mv.y;
                        this_mv.x = -this_mv.x;
                    }
                    let mut stack_idx = usize::from(*refmv_count);
                    for (i, e) in ref_mv_stack
                        .iter()
                        .enumerate()
                        .take(usize::from(*refmv_count))
                    {
                        if this_mv.as_int() == e.this_mv.as_int() {
                            stack_idx = i;
                            break;
                        }
                    }
                    if stack_idx == usize::from(*refmv_count) {
                        ref_mv_stack[stack_idx].this_mv = this_mv;
                        ref_mv_stack[stack_idx].weight = 2;
                        *refmv_count += 1;
                    }
                }
            }
            idx += i32::from(NUM_4X4_BLOCKS_HIGH[cand_bsize]);
        }

        // gm-fill (:644-646): this_mv only — count and weight untouched.
        for idx in usize::from(*refmv_count)..MAX_MV_REF_CANDIDATES {
            ref_mv_stack[idx].this_mv = gm_mv_candidates[0];
        }
    }
}

// ---------------------------------------------------------------------------
// setup_ref_mv_list — the full inter path (adaptive_mv_pred.c:651-971)
// ---------------------------------------------------------------------------

/// The output of [`setup_ref_mv_list`] — C's `(ref_mv_stack, refmv_count,
/// mode_context)` triple plus the `mv_ref0` scratch the symmetric-refs
/// mode threads through `add_tpl_ref_mv`.
#[derive(Debug, Clone)]
pub struct InterMvpStack {
    pub stack: [CandidateMv; MAX_REF_MV_STACK_SIZE],
    pub count: u8,
    pub mode_context: i16,
    /// C's `Mv mv_ref0[64]` after the MFMV walk (only the entries the walk
    /// visited are written; the rest stay zero).
    pub mv_ref0: [Mv; 64],
}

impl Default for InterMvpStack {
    fn default() -> Self {
        Self {
            stack: [CandidateMv::default(); MAX_REF_MV_STACK_SIZE],
            count: 0,
            mode_context: 0,
            mv_ref0: [Mv::default(); 64],
        }
    }
}

/// C `setup_ref_mv_list` (adaptive_mv_pred.c:651-971, EXPORTED), the
/// general branch: any `ref_frame` (single or compound), with the
/// temporal-MVP block live when `env.use_ref_frame_mvs`.
///
/// The caller supplies `gm_mv_candidates` exactly as
/// [`generate_av1_mvp_table`] computes them.
pub fn setup_ref_mv_list(
    grid: &MvpGrid,
    ctx: &MvpBlockCtx,
    env: &InterMvpEnv,
    ref_frame: i8,
    gm_mv_candidates: [Mv; 2],
) -> InterMvpStack {
    setup_ref_mv_list_seeded(
        grid,
        ctx,
        env,
        ref_frame,
        gm_mv_candidates,
        [Mv::default(); 64],
    )
}

/// [`setup_ref_mv_list`] with C's `Mv mv_ref0[64]` scratch seeded by the
/// caller.
///
/// The scratch is a LOCAL of `svt_aom_generate_av1_mvp_table` that is
/// SHARED across its `ref_frames` loop (adaptive_mv_pred.c:1336), which is
/// exactly how the `symteric_refs` shortcut works: the `LAST_FRAME` pass
/// writes the projected MV into slot `i` and the `BWDREF_FRAME` /
/// `LAST_BWD_FRAME` passes read it back from the same slot. Driving
/// `setup_ref_mv_list` for a non-`LAST` ref with a zeroed scratch is
/// therefore NOT the same computation as driving it inside the loop —
/// [`generate_av1_mvp_table`] threads the scratch for you.
pub fn setup_ref_mv_list_seeded(
    grid: &MvpGrid,
    ctx: &MvpBlockCtx,
    env: &InterMvpEnv,
    ref_frame: i8,
    gm_mv_candidates: [Mv; 2],
    mv_ref0_init: [Mv; 64],
) -> InterMvpStack {
    let mut out = InterMvpStack {
        mv_ref0: mv_ref0_init,
        ..InterMvpStack::default()
    };
    let stack = &mut out.stack;
    let mut refmv_count = 0u8;
    let mut mode_context: i16 = 0;

    let bs = ctx.n8_w.max(ctx.n8_h);
    let has_tr = has_top_right(grid, ctx, bs);
    let row_adj = ctx.n8_h < 2 && (ctx.mi_row & 1) != 0;
    let col_adj = ctx.n8_w < 2 && (ctx.mi_col & 1) != 0;
    let mut processed_rows = 0i32;
    let mut processed_cols = 0i32;

    let rf = av1_set_ref_frame(ref_frame);

    let mut max_row_offset = 0i32;
    let mut max_col_offset = 0i32;
    if ctx.up_available {
        max_row_offset = -(MVREF_ROWS << 1) + i32::from(row_adj);
        if ctx.n8_h < 2 {
            max_row_offset = -(2 << 1) + i32::from(row_adj);
        }
        max_row_offset = find_valid_row_offset(ctx.tile, ctx.mi_row, max_row_offset);
    }
    if ctx.left_available {
        max_col_offset = -(MVREF_ROWS << 1) + i32::from(col_adj);
        if ctx.n8_w < 2 {
            max_col_offset = -(2 << 1) + i32::from(col_adj);
        }
        max_col_offset = find_valid_col_offset(ctx.tile, ctx.mi_col, max_col_offset);
    }

    let mut col_match_count = 0u8;
    let mut row_match_count = 0u8;
    let mut newmv_count = 0u8;

    // ROW-1 (:696-710).
    if max_row_offset.abs() >= 1 {
        scan_row_mbmi(
            grid,
            ctx,
            rf,
            -1,
            stack,
            &mut refmv_count,
            &mut row_match_count,
            &mut newmv_count,
            gm_mv_candidates,
            env.global_motion,
            max_row_offset,
            &mut processed_rows,
        );
    }
    // COL-1 (:712-727).
    if max_col_offset.abs() >= 1 {
        scan_col_mbmi(
            grid,
            ctx,
            rf,
            -1,
            stack,
            &mut refmv_count,
            &mut col_match_count,
            &mut newmv_count,
            gm_mv_candidates,
            env.global_motion,
            max_col_offset,
            &mut processed_cols,
        );
    }
    // TOP-RIGHT (:729-744).
    if has_tr {
        scan_blk_mbmi(
            grid,
            ctx,
            rf,
            -1,
            ctx.n8_w,
            stack,
            &mut row_match_count,
            &mut newmv_count,
            gm_mv_candidates,
            env.global_motion,
            &mut refmv_count,
        );
    }

    let nearest_match = u8::from(row_match_count > 0) + u8::from(col_match_count > 0);

    for entry in stack.iter_mut().take(usize::from(refmv_count)) {
        entry.weight += REF_CAT_LEVEL;
    }

    // ---- Temporal MVP / MFMV (:754-860) ----
    if env.use_ref_frame_mvs {
        let mut is_available = 0i32;

        // C uses `xd->n4_w`/`n4_h` here; init_xd sets them from
        // `blk_geom->bwidth >> MI_SIZE_LOG2`, which equals
        // `mi_size_wide[bsize]` = n8_w for every BlockSize, so n8 is the
        // same value (asserted by the C-parity test, which drives both).
        let n4_w = ctx.n8_w;
        let n4_h = ctx.n8_h;
        let (blk_row_end, blk_col_end, step_w, step_h, allow_extension);
        if env.sb64_sq_no4xn_geom {
            blk_row_end = n4_w;
            blk_col_end = n4_w;
            step_w = if n4_w >= 16 { 4 } else { 2 };
            step_h = step_w;
            allow_extension = (2..16).contains(&n4_w);
        } else {
            blk_row_end = n4_h.min(16);
            blk_col_end = n4_w.min(16);
            allow_extension = (2..16).contains(&n4_h) && (2..16).contains(&n4_w);
            step_h = if n4_h >= 16 { 4 } else { 2 };
            // NOTE (faithful to C :770): the ELSE arm of step_w uses
            // `mi_size_high[BLOCK_8X8]`, not `mi_size_wide` — same value
            // (2), so it is a cosmetic asymmetry, transcribed as-is.
            step_w = if n4_w >= 16 { 4 } else { 2 };
        }

        let list_idx0 = get_list_idx(rf[0]);
        let ref_idx_l0 = get_ref_frame_idx(rf[0]);
        debug_assert!(list_idx0 < 2 && ref_idx_l0 < 4);
        let cur_frame_index = env.cur_order_hint;
        let frame0_index = env.ref_order_hint[rf[0] as usize];
        let cur_offset_0 = get_relative_dist(env.order_hint_info, cur_frame_index, frame0_index);
        let mut cur_offset_1 = 0i32;
        if rf[1] != NONE_FRAME {
            let frame1_index = env.ref_order_hint[rf[1] as usize];
            cur_offset_1 = get_relative_dist(env.order_hint_info, cur_frame_index, frame1_index);
        }

        // C walks a rolling `Mv* mv_ref0` cursor over a 64-entry array.
        let mut mv_ref0_idx = 0usize;
        let mut blk_row = 0i32;
        while blk_row < blk_row_end {
            let mut blk_col = 0i32;
            while blk_col < blk_col_end {
                let mut slot = out.mv_ref0[mv_ref0_idx];
                let ret = add_tpl_ref_mv(
                    env,
                    ctx,
                    ref_frame,
                    blk_row,
                    blk_col,
                    gm_mv_candidates,
                    &mut refmv_count,
                    &mut slot,
                    cur_offset_0,
                    cur_offset_1,
                    stack,
                    &mut mode_context,
                );
                out.mv_ref0[mv_ref0_idx] = slot;
                if blk_row == 0 && blk_col == 0 {
                    is_available = ret;
                }
                mv_ref0_idx += 1;
                blk_col += step_w;
            }
            blk_row += step_h;
        }

        if is_available == 0 {
            mode_context |= 1 << GLOBALMV_OFFSET;
        }

        if allow_extension {
            let voffset = if env.sb64_sq_no4xn_geom {
                n4_h
            } else {
                2.max(n4_h)
            };
            let hoffset = if env.sb64_sq_no4xn_geom {
                n4_h
            } else {
                2.max(n4_w)
            };
            let tpl_sample_pos = [[voffset, -2], [voffset, hoffset], [voffset - 2, hoffset]];
            for pos in tpl_sample_pos {
                let (blk_row, blk_col) = (pos[0], pos[1]);
                if !check_sb_border(ctx.mi_row, ctx.mi_col, blk_row, blk_col) {
                    continue;
                }
                let mut slot = out.mv_ref0[mv_ref0_idx];
                add_tpl_ref_mv(
                    env,
                    ctx,
                    ref_frame,
                    blk_row,
                    blk_col,
                    gm_mv_candidates,
                    &mut refmv_count,
                    &mut slot,
                    cur_offset_0,
                    cur_offset_1,
                    stack,
                    &mut mode_context,
                );
                out.mv_ref0[mv_ref0_idx] = slot;
                mv_ref0_idx += 1;
            }
        }
    } // End temporal MVP

    // TOP-LEFT (:862-877), with the dummy newmv counter.
    let mut dummy_newmv_count = 0u8;
    scan_blk_mbmi(
        grid,
        ctx,
        rf,
        -1,
        -1,
        stack,
        &mut row_match_count,
        &mut dummy_newmv_count,
        gm_mv_candidates,
        env.global_motion,
        &mut refmv_count,
    );

    // ROW-3/COL-3, ROW-5/COL-5 (:880-915).
    for idx in 2..=MVREF_ROWS {
        let row_offset = -(idx << 1) + 1 + i32::from(row_adj);
        let col_offset = -(idx << 1) + 1 + i32::from(col_adj);

        if row_offset.abs() <= max_row_offset.abs() && row_offset.abs() > processed_rows {
            scan_row_mbmi(
                grid,
                ctx,
                rf,
                row_offset,
                stack,
                &mut refmv_count,
                &mut row_match_count,
                &mut dummy_newmv_count,
                gm_mv_candidates,
                env.global_motion,
                max_row_offset,
                &mut processed_rows,
            );
        }
        if col_offset.abs() <= max_col_offset.abs() && col_offset.abs() > processed_cols {
            scan_col_mbmi(
                grid,
                ctx,
                rf,
                col_offset,
                stack,
                &mut refmv_count,
                &mut col_match_count,
                &mut dummy_newmv_count,
                gm_mv_candidates,
                env.global_motion,
                max_col_offset,
                &mut processed_cols,
            );
        }
    }

    // Mode-context derivation (:917-949).
    let ref_match_count = u8::from(row_match_count > 0) + u8::from(col_match_count > 0);
    match nearest_match {
        0 => {
            if ref_match_count >= 1 {
                mode_context |= 1;
            }
            if ref_match_count == 1 {
                mode_context |= 1 << REFMV_OFFSET;
            } else if ref_match_count >= 2 {
                mode_context |= 2 << REFMV_OFFSET;
            }
        }
        1 => {
            mode_context |= if newmv_count > 0 { 2 } else { 3 };
            if ref_match_count == 1 {
                mode_context |= 3 << REFMV_OFFSET;
            } else if ref_match_count >= 2 {
                mode_context |= 4 << REFMV_OFFSET;
            }
        }
        _ => {
            mode_context |= if newmv_count >= 1 { 4 } else { 5 };
            mode_context |= 5 << REFMV_OFFSET;
        }
    }

    // Sort (:952-955).
    if refmv_count > 1 {
        sort_mvp_table(stack, refmv_count);
    }

    // Light rescan (:957-961).
    if usize::from(refmv_count) < MAX_MV_REF_CANDIDATES {
        scan_row_col_light(
            grid,
            ctx,
            env,
            rf,
            stack,
            &mut refmv_count,
            gm_mv_candidates,
            max_row_offset,
            max_col_offset,
        );
    }

    // Final clamp (:963-970): comp_mv too on the compound path.
    let bw_px = ctx.n8_w << 2;
    let bh_px = ctx.n8_h << 2;
    for entry in stack.iter_mut().take(usize::from(refmv_count)) {
        clamp_mv_ref(&mut entry.this_mv, bw_px, bh_px, ctx);
        if rf[1] > NONE_FRAME {
            clamp_mv_ref(&mut entry.comp_mv, bw_px, bh_px, ctx);
        }
    }

    out.count = refmv_count;
    out.mode_context = mode_context;
    out
}

/// C `svt_aom_generate_av1_mvp_table` (adaptive_mv_pred.c:1329-1405,
/// EXPORTED) — the INTER path: the `gm_mv` derivation per ref type, then
/// [`setup_ref_mv_list`]. Returns one stack per entry of `ref_frames`.
///
/// C's `symteric_refs` gate (:1338-1347) — random-access pred structure,
/// `temporal_layer_index > 0`, and the ref list being exactly
/// `{LAST, BWDREF, LAST_BWD}` — is the caller's decision and arrives via
/// [`InterMvpEnv::symmetric_refs`]; [`symmetric_refs_gate`] computes it.
pub fn generate_av1_mvp_table(
    grid: &MvpGrid,
    ctx: &MvpBlockCtx,
    env: &InterMvpEnv,
    bsize: usize,
    ref_frames: &[i8],
) -> alloc::vec::Vec<InterMvpStack> {
    // C's `Mv mv_ref0[64]` is ONE local, shared across the whole ref loop
    // (adaptive_mv_pred.c:1336) — the symmetric-refs shortcut depends on
    // that sharing, so thread it here rather than restarting per ref.
    let mut mv_ref0 = [Mv::default(); 64];
    let mut out = alloc::vec::Vec::with_capacity(ref_frames.len());
    for &ref_frame in ref_frames {
        let gm_mv = gm_mv_candidates_for(env, ref_frame, bsize, ctx.mi_col, ctx.mi_row);
        let stack = setup_ref_mv_list_seeded(grid, ctx, env, ref_frame, gm_mv, mv_ref0);
        mv_ref0 = stack.mv_ref0;
        out.push(stack);
    }
    out
}

/// C `svt_aom_generate_av1_mvp_table`'s `gm_mv` derivation
/// (adaptive_mv_pred.c:1372-1394), split out so callers that drive
/// [`setup_ref_mv_list`] directly get the same candidates.
pub fn gm_mv_candidates_for(
    env: &InterMvpEnv,
    ref_frame: i8,
    bsize: usize,
    mi_col: i32,
    mi_row: i32,
) -> [Mv; 2] {
    if ref_frame == INTRA_FRAME {
        return [Mv::default(); 2];
    }
    let rf = av1_set_ref_frame(ref_frame);
    if (ref_frame as usize) < REF_FRAMES {
        [
            gm_get_motion_vector_enc(
                &env.global_motion[ref_frame as usize],
                env.allow_high_precision_mv,
                bsize,
                mi_col,
                mi_row,
                env.force_integer_mv,
            ),
            Mv::default(),
        ]
    } else {
        [
            gm_get_motion_vector_enc(
                &env.global_motion[rf[0] as usize],
                env.allow_high_precision_mv,
                bsize,
                mi_col,
                mi_row,
                env.force_integer_mv,
            ),
            gm_get_motion_vector_enc(
                &env.global_motion[rf[1] as usize],
                env.allow_high_precision_mv,
                bsize,
                mi_col,
                mi_row,
                env.force_integer_mv,
            ),
        ]
    }
}

/// C `svt_aom_generate_av1_mvp_table`'s `symteric_refs` gate
/// (adaptive_mv_pred.c:1338-1347). `pred_structure_is_random_access` is
/// C's `scs->static_config.pred_structure == RANDOM_ACCESS`.
pub fn symmetric_refs_gate(
    temporal_layer_index: u8,
    pred_structure_is_random_access: bool,
    ref_frames: &[i8],
) -> bool {
    temporal_layer_index > 0
        && pred_structure_is_random_access
        && ref_frames.len() == 3
        && ref_frames[0] == LAST_FRAME
        && ref_frames[1] == BWDREF_FRAME
        && ref_frames[2] == LAST_BWD_FRAME
}

// ---------------------------------------------------------------------------
// From-stack reads (adaptive_mv_pred.c:1407-1457, 2002-2040)
// ---------------------------------------------------------------------------

/// C `svt_av1_get_ref_mv_from_stack` (adaptive_mv_pred.c:2002-2028,
/// EXPORTED). `ref_frame` is the `{rf0, rf1}` pair as C passes it.
pub fn get_ref_mv_from_stack(
    ref_idx: usize,
    ref_frame: [i8; 2],
    ref_mv_idx: usize,
    stack: &InterMvpStack,
) -> Mv {
    if ref_frame[1] > INTRA_FRAME {
        if ref_idx == 0 {
            stack.stack[ref_mv_idx].this_mv
        } else {
            debug_assert_eq!(ref_idx, 1);
            stack.stack[ref_mv_idx].comp_mv
        }
    } else {
        debug_assert_eq!(ref_idx, 0);
        if ref_mv_idx < usize::from(stack.count) {
            stack.stack[ref_mv_idx].this_mv
        } else {
            Mv::from_int(INVALID_MV)
        }
    }
}

/// C `svt_av1_find_best_ref_mvs_from_stack` (adaptive_mv_pred.c:2030-2040,
/// EXPORTED). Note C builds `ref_frames = {ref_frame, NONE_FRAME}` so the
/// single-ref arm of `get_ref_mv_from_stack` is always taken, even for a
/// compound `ref_frame` type.
pub fn find_best_ref_mvs_from_stack(
    stack: &InterMvpStack,
    ref_frame: i8,
    allow_hp: bool,
    is_integer: bool,
) -> (Mv, Mv) {
    let ref_frames = [ref_frame, NONE_FRAME];
    let mut nearest = get_ref_mv_from_stack(0, ref_frames, 0, stack);
    lower_mv_precision(&mut nearest, allow_hp, is_integer);
    let mut near = get_ref_mv_from_stack(0, ref_frames, 1, stack);
    lower_mv_precision(&mut near, allow_hp, is_integer);
    (nearest, near)
}

/// C `compound_ref0_mode` (inter_prediction.h:341-372).
#[inline]
pub fn compound_ref0_mode(mode: u8) -> u8 {
    COMPOUND_REF0_MODE[mode as usize]
}

/// C `compound_ref1_mode` (inter_prediction.h:374-405).
#[inline]
pub fn compound_ref1_mode(mode: u8) -> u8 {
    COMPOUND_REF1_MODE[mode as usize]
}

/// The `(nearestmv, nearmv, ref_mv)` triple [`get_av1_mv_pred_drl`]
/// produces (C writes them through out-params).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DrlMvPred {
    pub nearestmv: [Mv; 2],
    pub nearmv: [Mv; 2],
    pub ref_mv: [Mv; 2],
}

/// C `svt_aom_get_av1_mv_pred_drl` (adaptive_mv_pred.c:1407-1457,
/// EXPORTED).
///
/// C leaves `nearestmv`/`nearmv` UNINITIALIZED on the branches it does not
/// write (e.g. `!is_compound && mode == GLOBALMV`); the caller's arrays
/// carry whatever was there before. This port takes them as `initial` so
/// that behaviour is explicit and reproducible instead of implicit.
pub fn get_av1_mv_pred_drl(
    stack: &InterMvpStack,
    is_compound: bool,
    mode: u8,
    drl_index: usize,
    initial: DrlMvPred,
) -> DrlMvPred {
    let mut nearestmv = initial.nearestmv;
    let mut nearmv = initial.nearmv;

    if !is_compound && mode != GLOBALMV {
        nearestmv[0] = stack.stack[0].this_mv;
        nearmv[0] = stack.stack[1].this_mv;
    }

    if is_compound && mode != GLOBAL_GLOBALMV {
        let ref_mv_idx = drl_index + 1;
        nearestmv[0] = stack.stack[0].this_mv;
        nearestmv[1] = stack.stack[0].comp_mv;
        nearmv[0] = stack.stack[ref_mv_idx].this_mv;
        nearmv[1] = stack.stack[ref_mv_idx].comp_mv;
    } else if drl_index > 0 && mode == NEARMV {
        debug_assert!(1 + drl_index < MAX_REF_MV_STACK_SIZE);
        nearmv[0] = stack.stack[1 + drl_index].this_mv;
    }

    let mut ref_mv = [nearestmv[0], nearestmv[1]];

    if is_compound {
        let mut ref_mv_idx = drl_index;
        if mode == NEAR_NEWMV || mode == NEW_NEARMV {
            ref_mv_idx = 1 + drl_index;
        }
        if compound_ref0_mode(mode) == NEWMV {
            ref_mv[0] = stack.stack[ref_mv_idx].this_mv;
        }
        if compound_ref1_mode(mode) == NEWMV {
            ref_mv[1] = stack.stack[ref_mv_idx].comp_mv;
        }
    } else if mode == NEWMV && stack.count > 1 {
        ref_mv[0] = stack.stack[drl_index].this_mv;
    }

    DrlMvPred {
        nearestmv,
        nearmv,
        ref_mv,
    }
}

// ---------------------------------------------------------------------------
// Light mode-context derivation (adaptive_mv_pred.c:1138-1327)
// ---------------------------------------------------------------------------

/// C `count_ref_match` (adaptive_mv_pred.c:1128-1136).
#[inline]
fn count_ref_match(bmi: &MvpMiEntry, target_rf: i8, match_count: &mut u8, newmv_count: &mut u8) {
    if bmi.ref_frame[0] == target_rf || bmi.ref_frame[1] == target_rf {
        *match_count += 1;
        if have_newmv_in_inter_mode(bmi.mode) {
            *newmv_count += 1;
        }
    }
}

/// C `svt_aom_compute_inter_mode_ctx_light` (adaptive_mv_pred.c:1138-1327,
/// EXPORTED): the LPD1 fast path — the same neighbour walk as
/// [`setup_ref_mv_list`] but tracking only the three counters, and
/// assuming block size >= 8x8 (so `row_adj == col_adj == 0`). Returns
/// C's `ctx->inter_mode_ctx[ref_frame]`.
pub fn compute_inter_mode_ctx_light(grid: &MvpGrid, ctx: &MvpBlockCtx, ref_frame: i8) -> i16 {
    let tile = ctx.tile;
    let mi_row = ctx.mi_row;
    let mi_col = ctx.mi_col;
    let rf = av1_set_ref_frame(ref_frame);
    let target_rf = rf[0];

    let mut max_row_offset = 0i32;
    let mut max_col_offset = 0i32;
    let mut processed_rows = 0i32;
    let mut processed_cols = 0i32;

    if ctx.up_available {
        max_row_offset = find_valid_row_offset(tile, mi_row, -(MVREF_ROWS << 1));
    }
    if ctx.left_available {
        max_col_offset = find_valid_col_offset(tile, mi_col, -(MVREF_ROWS << 1));
    }

    let mut row_match = 0u8;
    let mut col_match = 0u8;
    let mut newmv_count = 0u8;
    let n8_w = ctx.n8_w;
    let n8_h = ctx.n8_h;

    // ROW -1 (:1167-1187).
    if max_row_offset.abs() >= 1 {
        let mut end_mi = n8_w.min(ctx.mi_cols - mi_col);
        end_mi = end_mi.min(16);
        let use_step_16 = n8_w >= 16;
        let mut i = 0i32;
        while i < end_mi {
            let cand = cell_at(grid, -grid.stride + i);
            let cb = usize::from(cand.bsize);
            let mut len = n8_w.min(i32::from(NUM_4X4_BLOCKS_WIDE[cb]));
            if use_step_16 {
                len = len.max(4);
            }
            if n8_w <= i32::from(NUM_4X4_BLOCKS_WIDE[cb]) {
                processed_rows = (-max_row_offset).min(i32::from(NUM_4X4_BLOCKS_HIGH[cb]));
            }
            if is_inter_block(cand) {
                count_ref_match(cand, target_rf, &mut row_match, &mut newmv_count);
            }
            i += len;
        }
    }

    // COL -1 (:1189-1209).
    if max_col_offset.abs() >= 1 {
        let mut end_mi = n8_h.min(ctx.mi_rows - mi_row);
        end_mi = end_mi.min(16);
        let use_step_16 = n8_h >= 16;
        let mut i = 0i32;
        while i < end_mi {
            let cand = cell_at(grid, i * grid.stride - 1);
            let cb = usize::from(cand.bsize);
            let mut len = n8_h.min(i32::from(NUM_4X4_BLOCKS_HIGH[cb]));
            if use_step_16 {
                len = len.max(4);
            }
            if n8_h <= i32::from(NUM_4X4_BLOCKS_HIGH[cb]) {
                processed_cols = (-max_col_offset).min(i32::from(NUM_4X4_BLOCKS_WIDE[cb]));
            }
            if is_inter_block(cand) {
                count_ref_match(cand, target_rf, &mut col_match, &mut newmv_count);
            }
            i += len;
        }
    }

    // TOP-RIGHT (:1211-1219).
    if has_top_right(grid, ctx, n8_w.max(n8_h))
        && mi_col + n8_w < tile.mi_col_end
        && mi_row > tile.mi_row_start
    {
        let cand = cell_at(grid, -grid.stride + n8_w);
        if is_inter_block(cand) {
            count_ref_match(cand, target_rf, &mut row_match, &mut newmv_count);
        }
    }

    let nearest_match = u8::from(row_match > 0) + u8::from(col_match > 0);

    // TOP-LEFT (:1224-1230).
    if mi_col > tile.mi_col_start && mi_row > tile.mi_row_start {
        let cand = cell_at(grid, -grid.stride - 1);
        if is_inter_block(cand)
            && (cand.ref_frame[0] == target_rf || cand.ref_frame[1] == target_rf)
        {
            row_match += 1;
        }
    }

    // Outer rows/cols 3, 5 (:1234-1290), with C's early exit.
    if !(row_match > 0 && col_match > 0) {
        for idx in 2..=MVREF_ROWS {
            let row_offset = -(idx << 1) + 1;
            let col_offset = -(idx << 1) + 1;

            if row_offset.abs() <= max_row_offset.abs() && row_offset.abs() > processed_rows {
                let mut end_mi = n8_w.min(ctx.mi_cols - mi_col);
                end_mi = end_mi.min(16);
                let use_step_16 = n8_w >= 16;
                let mut i = 0i32;
                while i < end_mi {
                    let cand = cell_at(grid, row_offset * grid.stride + 1 + i);
                    let cb = usize::from(cand.bsize);
                    let mut len = n8_w.min(i32::from(NUM_4X4_BLOCKS_WIDE[cb]));
                    if use_step_16 {
                        len = len.max(4);
                    } else {
                        len = len.max(2);
                    }
                    if n8_w <= i32::from(NUM_4X4_BLOCKS_WIDE[cb]) {
                        processed_rows = (-max_row_offset + row_offset + 1)
                            .min(i32::from(NUM_4X4_BLOCKS_HIGH[cb]))
                            - row_offset
                            - 1;
                    }
                    if is_inter_block(cand)
                        && (cand.ref_frame[0] == target_rf || cand.ref_frame[1] == target_rf)
                    {
                        row_match += 1;
                    }
                    i += len;
                }
            }
            if col_offset.abs() <= max_col_offset.abs() && col_offset.abs() > processed_cols {
                let mut end_mi = n8_h.min(ctx.mi_rows - mi_row);
                end_mi = end_mi.min(16);
                let use_step_16 = n8_h >= 16;
                let mut i = 0i32;
                while i < end_mi {
                    let cand = cell_at(grid, (1 + i) * grid.stride + col_offset);
                    let cb = usize::from(cand.bsize);
                    let mut len = n8_h.min(i32::from(NUM_4X4_BLOCKS_HIGH[cb]));
                    if use_step_16 {
                        len = len.max(4);
                    } else {
                        len = len.max(2);
                    }
                    if n8_h <= i32::from(NUM_4X4_BLOCKS_HIGH[cb]) {
                        processed_cols = (-max_col_offset + col_offset + 1)
                            .min(i32::from(NUM_4X4_BLOCKS_WIDE[cb]))
                            - col_offset
                            - 1;
                    }
                    if is_inter_block(cand)
                        && (cand.ref_frame[0] == target_rf || cand.ref_frame[1] == target_rf)
                    {
                        col_match += 1;
                    }
                    i += len;
                }
            }
            if row_match > 0 && col_match > 0 {
                break;
            }
        }
    }

    // Mode-context derivation (:1292-1325).
    let mut mode_ctx = 0i16;
    let ref_match_count = u8::from(row_match > 0) + u8::from(col_match > 0);
    match nearest_match {
        0 => {
            if ref_match_count >= 1 {
                mode_ctx |= 1;
            }
            if ref_match_count == 1 {
                mode_ctx |= 1 << REFMV_OFFSET;
            } else if ref_match_count >= 2 {
                mode_ctx |= 2 << REFMV_OFFSET;
            }
        }
        1 => {
            mode_ctx |= if newmv_count > 0 { 2 } else { 3 };
            if ref_match_count == 1 {
                mode_ctx |= 3 << REFMV_OFFSET;
            } else if ref_match_count >= 2 {
                mode_ctx |= 4 << REFMV_OFFSET;
            }
        }
        _ => {
            mode_ctx |= if newmv_count >= 1 { 4 } else { 5 };
            mode_ctx |= 5 << REFMV_OFFSET;
        }
    }
    mode_ctx
}

// ---------------------------------------------------------------------------
// Motion-field projection (md_config_process.c:396-580)
//
// EVIDENCE TIER 4: `get_block_position`, `motion_field_projection` and
// `av1_setup_motion_field` are all `static` in md_config_process.c and
// export no symbol (verified with `nm` over Bin/Release/libSvtAv1Enc.a),
// so these are covered by hand-derived vectors traced against the C
// source, not by a differential.
// ---------------------------------------------------------------------------

/// C `get_block_position` (md_config_process.c:396-419). Returns
/// `Some((mi_r, mi_c))` when the projected position is usable.
pub fn get_block_position(
    mi_rows: i32,
    mi_cols: i32,
    blk_row: i32,
    blk_col: i32,
    mv: Mv,
    sign_bias: bool,
) -> Option<(i32, i32)> {
    let base_blk_row = (blk_row >> 3) << 3;
    let base_blk_col = (blk_col >> 3) << 3;

    // (4 + MI_SIZE_LOG2) == 6.
    let row_offset = if mv.y >= 0 {
        i32::from(mv.y) >> 6
    } else {
        -((-i32::from(mv.y)) >> 6)
    };
    let col_offset = if mv.x >= 0 {
        i32::from(mv.x) >> 6
    } else {
        -((-i32::from(mv.x)) >> 6)
    };

    let row = if sign_bias {
        blk_row - row_offset
    } else {
        blk_row + row_offset
    };
    let col = if sign_bias {
        blk_col - col_offset
    } else {
        blk_col + col_offset
    };

    if row < 0 || row >= (mi_rows >> 1) || col < 0 || col >= (mi_cols >> 1) {
        return None;
    }
    if row < base_blk_row - (MAX_OFFSET_HEIGHT >> 3)
        || row >= base_blk_row + 8 + (MAX_OFFSET_HEIGHT >> 3)
        || col < base_blk_col - (MAX_OFFSET_WIDTH >> 3)
        || col >= base_blk_col + 8 + (MAX_OFFSET_WIDTH >> 3)
    {
        return None;
    }
    Some((row, col))
}

/// One reference frame's saved motion field plus the metadata
/// [`motion_field_projection`] reads off `EbReferenceObject`.
pub struct RefMotionField<'a> {
    /// C `start_frame_buf->mvs`, an `mvs_rows * mvs_cols` grid.
    pub mvs: &'a [MvRef],
    /// C `start_frame_buf->order_hint`.
    pub order_hint: i32,
    /// C `start_frame_buf->ref_order_hint[0..7]`, indexed by
    /// `ref - LAST_FRAME`.
    pub ref_order_hint: [i32; INTER_REFS_PER_FRAME],
    /// C `start_frame_buf->frame_type` being KEY / INTRA_ONLY aborts the
    /// projection.
    pub is_intra_only: bool,
    /// C `start_frame_buf->mi_rows` / `mi_cols` — a mismatch with the
    /// current frame aborts (AV1 spec 7.9.2).
    pub mi_rows: i32,
    pub mi_cols: i32,
}

/// C `motion_field_projection` (md_config_process.c:427-521). Writes into
/// `tpl_mvs` (stride `mi_stride >> 1`) and returns C's `int`.
///
/// `dir` is C's `dir` argument (0 or 2); the sign flip and the
/// `get_block_position` sign bias both derive from it exactly as C does.
pub fn motion_field_projection(
    tpl_mvs: &mut [TplMvRef],
    tpl_stride: i32,
    mi_rows: i32,
    mi_cols: i32,
    cur_order_hint: i32,
    order_hint_info: OrderHintInfo,
    start_frame: Option<&RefMotionField>,
    dir: i32,
) -> i32 {
    let Some(buf) = start_frame else {
        return 0;
    };
    if buf.is_intra_only {
        return 0;
    }
    if buf.mi_rows != mi_rows || buf.mi_cols != mi_cols {
        return 0;
    }

    let start_frame_order_hint = buf.order_hint;
    let mut start_to_current_frame_offset =
        get_relative_dist(order_hint_info, start_frame_order_hint, cur_order_hint);

    let mut ref_offset = [0i32; REF_FRAMES];
    for i in (LAST_FRAME as usize)..=(ALTREF_FRAME as usize) {
        ref_offset[i] = get_relative_dist(
            order_hint_info,
            start_frame_order_hint,
            buf.ref_order_hint[i - LAST_FRAME as usize],
        );
    }

    if dir == 2 {
        start_to_current_frame_offset = -start_to_current_frame_offset;
    }

    let mvs_rows = (mi_rows + 1) >> 1;
    let mvs_cols = (mi_cols + 1) >> 1;

    for blk_row in 0..mvs_rows {
        for blk_col in 0..mvs_cols {
            let mv_ref = buf.mvs[(blk_row * mvs_cols + blk_col) as usize];
            let fwd_mv = mv_ref.mv;

            if mv_ref.ref_frame > INTRA_FRAME {
                let ref_frame_offset = ref_offset[mv_ref.ref_frame as usize];

                let pos_valid = ref_frame_offset.abs() <= MAX_FRAME_DISTANCE
                    && ref_frame_offset > 0
                    && start_to_current_frame_offset.abs() <= MAX_FRAME_DISTANCE;

                if pos_valid {
                    let this_mv =
                        get_mv_projection(fwd_mv, start_to_current_frame_offset, ref_frame_offset);
                    if let Some((mi_r, mi_c)) = get_block_position(
                        mi_rows,
                        mi_cols,
                        blk_row,
                        blk_col,
                        this_mv,
                        (dir >> 1) != 0,
                    ) {
                        let mi_offset = mi_r * tpl_stride + mi_c;
                        tpl_mvs[mi_offset as usize].mfmv0 = fwd_mv;
                        tpl_mvs[mi_offset as usize].ref_frame_offset = ref_frame_offset as u8;
                    }
                }
            }
        }
    }

    1
}

/// The per-reference inputs [`setup_motion_field`] needs, indexed by
/// `ref - LAST_FRAME` (0..7).
pub struct MotionFieldRefs<'a> {
    pub refs: [Option<RefMotionField<'a>>; INTER_REFS_PER_FRAME],
}

/// What [`setup_motion_field`] produces besides the filled `tpl_mvs`:
/// C's `pcs->ref_frame_side[TOTAL_REFS_PER_FRAME]`.
pub type RefFrameSide = [i8; TOTAL_REFS_PER_FRAME];

/// C `av1_setup_motion_field` (md_config_process.c:523-580).
///
/// Fills `tpl_mvs` (which the caller sizes
/// `((mi_rows + MAX_MIB_SIZE) >> 1) * (mi_stride >> 1)`, as C does) and
/// returns `ref_frame_side`. Note C computes `ref_frame_side`
/// unconditionally but returns EARLY — before touching `tpl_mvs` — when
/// `use_ref_frame_mvs` is 0; that early return is reproduced.
pub fn setup_motion_field(
    tpl_mvs: &mut [TplMvRef],
    tpl_stride: i32,
    mi_rows: i32,
    mi_cols: i32,
    cur_order_hint: i32,
    order_hint_info: OrderHintInfo,
    use_ref_frame_mvs: bool,
    refs: &MotionFieldRefs,
) -> RefFrameSide {
    let mut ref_frame_side: RefFrameSide = [0; TOTAL_REFS_PER_FRAME];
    if !order_hint_info.enable_order_hint {
        return ref_frame_side;
    }

    let mut ref_order_hint = [0i32; INTER_REFS_PER_FRAME];
    for ref_frame in (LAST_FRAME as usize)..=(ALTREF_FRAME as usize) {
        let ref_idx = ref_frame - LAST_FRAME as usize;
        let order_hint = refs.refs[ref_idx].as_ref().map_or(0, |b| b.order_hint);
        ref_order_hint[ref_idx] = order_hint;
        if get_relative_dist(order_hint_info, order_hint, cur_order_hint) > 0 {
            ref_frame_side[ref_frame] = 1;
        } else if order_hint == cur_order_hint {
            ref_frame_side[ref_frame] = -1;
        }
    }

    if !use_ref_frame_mvs {
        return ref_frame_side;
    }

    for slot in tpl_mvs.iter_mut() {
        *slot = TplMvRef::default();
    }

    let project = |tpl: &mut [TplMvRef], start: i8, dir: i32| -> i32 {
        motion_field_projection(
            tpl,
            tpl_stride,
            mi_rows,
            mi_cols,
            cur_order_hint,
            order_hint_info,
            refs.refs[(start - LAST_FRAME) as usize].as_ref(),
            dir,
        )
    };

    let mut ref_stamp = MFMV_STACK_SIZE - 1;
    if refs.refs[0].is_some() {
        let alt_of_lst_order_hint = refs.refs[0].as_ref().map_or(0, |b| {
            b.ref_order_hint[(ALTREF_FRAME - LAST_FRAME) as usize]
        });
        let is_lst_overlay =
            alt_of_lst_order_hint == ref_order_hint[(GOLDEN_FRAME - LAST_FRAME) as usize];
        if !is_lst_overlay {
            project(tpl_mvs, LAST_FRAME, 2);
        }
        ref_stamp -= 1;
    }

    if get_relative_dist(
        order_hint_info,
        ref_order_hint[(BWDREF_FRAME - LAST_FRAME) as usize],
        cur_order_hint,
    ) > 0
        && project(tpl_mvs, BWDREF_FRAME, 0) != 0
    {
        ref_stamp -= 1;
    }

    if get_relative_dist(
        order_hint_info,
        ref_order_hint[(ALTREF2_FRAME - LAST_FRAME) as usize],
        cur_order_hint,
    ) > 0
        && project(tpl_mvs, ALTREF2_FRAME, 0) != 0
    {
        ref_stamp -= 1;
    }

    if get_relative_dist(
        order_hint_info,
        ref_order_hint[(ALTREF_FRAME - LAST_FRAME) as usize],
        cur_order_hint,
    ) > 0
        && ref_stamp >= 0
        && project(tpl_mvs, ALTREF_FRAME, 0) != 0
    {
        ref_stamp -= 1;
    }

    if ref_stamp >= 0 {
        project(tpl_mvs, LAST2_FRAME, 2);
    }

    ref_frame_side
}

// ---------------------------------------------------------------------------
// Compound mode-context collapse (inter_prediction.c:2565-2581)
// ---------------------------------------------------------------------------

/// C `COMP_NEWMV_CTXS` (definitions.h:1352).
const COMP_NEWMV_CTXS: usize = 5;
/// C `NEWMV_CTX_MASK` = `(1 << GLOBALMV_OFFSET) - 1` = 7 (definitions.h:1348).
const NEWMV_CTX_MASK: i16 = (1 << GLOBALMV_OFFSET) - 1;
/// C `REFMV_CTX_MASK` = `(1 << (8 - REFMV_OFFSET)) - 1` = 15 (definitions.h:1350).
const REFMV_CTX_MASK: i16 = (1 << (8 - REFMV_OFFSET)) - 1;

/// C `svt_aom_compound_mode_ctx_map` (inter_prediction.c:2566-2570).
const COMPOUND_MODE_CTX_MAP: [[i16; COMP_NEWMV_CTXS]; 3] =
    [[0, 1, 1, 1, 1], [1, 2, 3, 4, 4], [4, 4, 5, 6, 7]];

/// C `svt_aom_mode_context_analyzer` (inter_prediction.c:2565-2581,
/// EXPORTED): collapse [`setup_ref_mv_list`]'s packed mode context into
/// the single compound context. A single-ref pair passes through
/// untouched.
///
/// C asserts `(refmv_ctx >> 1) < 3` (`:2578`). That DOES hold for every
/// context `setup_ref_mv_list` produces — its REFMV field is 0..5, so
/// `refmv_ctx >> 1` is 0..2 — but the function is `int16_t`-typed and will
/// index out of range on a hand-built context with a REFMV field >= 6.
/// This port keeps the assert as a `debug_assert!` rather than clamping,
/// because clamping would silently disagree with C on exactly the inputs
/// C would fault on.
pub fn mode_context_analyzer(mode_context: i16, rf: [i8; 2]) -> i16 {
    if rf[1] <= INTRA_FRAME {
        return mode_context;
    }
    let newmv_ctx = mode_context & NEWMV_CTX_MASK;
    let refmv_ctx = (mode_context >> REFMV_OFFSET) & REFMV_CTX_MASK;
    debug_assert!((refmv_ctx >> 1) < 3);
    COMPOUND_MODE_CTX_MAP[(refmv_ctx >> 1) as usize]
        [(newmv_ctx.min(COMP_NEWMV_CTXS as i16 - 1)) as usize]
}

// ---------------------------------------------------------------------------
// Overlappable-neighbour counts (adaptive_mv_pred.c:1830-1906)
// ---------------------------------------------------------------------------

/// C `is_neighbor_overlappable` (inter_prediction.h:271-273).
#[inline]
fn is_neighbor_overlappable(e: &MvpMiEntry) -> bool {
    e.ref_frame[0] > INTRA_FRAME
}

/// C `count_overlappable_nb_above` (adaptive_mv_pred.c:1830-1861).
///
/// The `mi_step == 1` arm rewinds the LOOP VARIABLE (`above_mi_col &= ~1`)
/// and then reads the cell one to its right — a 4-wide block is treated as
/// half of a chroma pair — so the rewind is observable in the iteration
/// order, not just in which cell is read.
fn count_overlappable_nb_above(grid: &MvpGrid, ctx: &MvpBlockCtx, nb_max: u32) -> u32 {
    let mut nb_count = 0u32;
    if !ctx.up_available {
        return nb_count;
    }
    let end_col = (ctx.mi_col + ctx.n8_w).min(ctx.mi_cols);
    let mut above_mi_col = ctx.mi_col;
    while above_mi_col < end_col && nb_count < nb_max {
        // prev_row_mi + above_mi_col == xd->mi[-mi_stride + (col - mi_col)]
        let mut off = -grid.stride + (above_mi_col - ctx.mi_col);
        let mut mi_step =
            i32::from(NUM_4X4_BLOCKS_WIDE[usize::from(cell_at(grid, off).bsize)]).min(16);
        if mi_step == 1 {
            above_mi_col &= !1;
            off = -grid.stride + (above_mi_col - ctx.mi_col) + 1;
            mi_step = 2;
        }
        if is_neighbor_overlappable(cell_at(grid, off)) {
            nb_count += 1;
        }
        above_mi_col += mi_step;
    }
    nb_count
}

/// C `count_overlappable_nb_left` (adaptive_mv_pred.c:1864-1891).
fn count_overlappable_nb_left(grid: &MvpGrid, ctx: &MvpBlockCtx, nb_max: u32) -> u32 {
    let mut nb_count = 0u32;
    if !ctx.left_available {
        return nb_count;
    }
    let end_row = (ctx.mi_row + ctx.n8_h).min(ctx.mi_rows);
    let mut left_mi_row = ctx.mi_row;
    while left_mi_row < end_row && nb_count < nb_max {
        // prev_col_mi + left_mi_row * mi_stride
        //   == xd->mi[(row - mi_row) * mi_stride - 1]
        let mut off = (left_mi_row - ctx.mi_row) * grid.stride - 1;
        let mut mi_step =
            i32::from(NUM_4X4_BLOCKS_HIGH[usize::from(cell_at(grid, off).bsize)]).min(16);
        if mi_step == 1 {
            left_mi_row &= !1;
            off = (left_mi_row + 1 - ctx.mi_row) * grid.stride - 1;
            mi_step = 2;
        }
        if is_neighbor_overlappable(cell_at(grid, off)) {
            nb_count += 1;
        }
        left_mi_row += mi_step;
    }
    nb_count
}

/// C `svt_av1_count_overlappable_neighbors` (adaptive_mv_pred.c:1893-1906,
/// EXPORTED): `blk_ptr->overlappable_neighbors`, the OBMC gate. Zero for
/// any block narrower or shorter than 8 px.
pub fn count_overlappable_neighbors(grid: &MvpGrid, ctx: &MvpBlockCtx, bsize: usize) -> u32 {
    if !is_motion_variation_allowed_bsize(bsize) {
        return 0;
    }
    count_overlappable_nb_above(grid, ctx, u32::MAX)
        + count_overlappable_nb_left(grid, ctx, u32::MAX)
}

// ---------------------------------------------------------------------------
// av1_find_samples (adaptive_mv_pred.c:1594-1750) — the WARPED-MOTION sample
// scan, whose COUNT decides the motion-mode ALPHABET
// ---------------------------------------------------------------------------

/// C `LEAST_SQUARES_SAMPLES_MAX` (definitions.h:469) — `1 << 3`.
pub const LEAST_SQUARES_SAMPLES_MAX: usize = 8;

/// C `record_samples` (adaptive_mv_pred.c:1594) — one neighbour's centre
/// point and its projection through that neighbour's MV, both in EIGHTH pel
/// and relative to this block's top-left pixel.
///
/// `sign_r` / `sign_c` are C's own `+1` / `-1` selectors, not booleans: the
/// above scan passes `(0, -1, col_offset, 1)` and the left scan
/// `(row_offset, 1, 0, -1)`, so the two differ in WHICH axis gets the
/// half-block bias and in its direction.
#[must_use]
fn record_samples(
    e: &crate::intrabc_mvp::MvpMiEntry,
    row_offset: i32,
    sign_r: i32,
    col_offset: i32,
    sign_c: i32,
) -> ([i32; 2], [i32; 2]) {
    let bw = i32::from(svtav1_types::tables::block::BLOCK_SIZE_WIDE[e.bsize as usize]);
    let bh = i32::from(svtav1_types::tables::block::BLOCK_SIZE_HIGH[e.bsize as usize]);
    // C `MI_SIZE` is 4.
    let x = col_offset * 4 + sign_c * bw.max(4) / 2 - 1;
    let y = row_offset * 4 + sign_r * bh.max(4) / 2 - 1;
    (
        [x * 8, y * 8],
        [x * 8 + i32::from(e.mv[0].x), y * 8 + i32::from(e.mv[0].y)],
    )
}

/// C `av1_find_samples` (adaptive_mv_pred.c:1610-1750) — how many
/// SINGLE-reference neighbours predict from `rf0`, and where they are.
///
/// # Why the COUNT is bitstream-critical even with warped motion switched off
///
/// `motion_mode_allowed` promotes a block to `WARPED_CAUSAL` — and with it
/// the THREE-symbol `MOTION_MODES` alphabet instead of the two-symbol OBMC
/// one — when `allow_warped_motion` is set and this count is `>= 1`. The
/// DECODER runs the same scan. A port that leaves the count at 0 writes the
/// wrong ALPHABET on every inter block with an overlappable neighbour, and
/// the arithmetic coder desynchronises: `docs/INTER-ENCODE-PLAN.md` §1z¹⁸
/// measured `aomdec` rejecting 22 of the campaign's 96 cells for exactly
/// this, every one of them at the preset where `allow_warped_motion` is 1.
///
/// **So this is not a warped-motion feature.** Turning `wm_ctrls` off keeps
/// warped motion out of the candidate SET and does nothing about the symbol
/// every inter block writes.
///
/// Four scans in C's order — above, left, top-left, top-right — each with
/// C's own early return at [`LEAST_SQUARES_SAMPLES_MAX`]. The `do_tl` /
/// `do_tr` suppressions are set by the ABOVE and LEFT scans' "current block
/// is no wider/taller than the neighbour" arms and are read by the last two,
/// so the order is load-bearing.
///
/// The sample POINTS are returned as well as the count. Nothing in this port
/// consumes them yet — warped-motion parameter estimation is unported — but
/// they are what a future `svt_aom_warped_motion_parameters` needs, and
/// computing the count without them would be a second, partial transcription
/// of the same scan.
///
/// Evidence tier 4 — `av1_find_samples` is `static` with no exported symbol.
#[must_use]
pub fn find_warp_samples(
    grid: &crate::intrabc_mvp::MvpGrid<'_>,
    ctx: &crate::intrabc_mvp::MvpBlockCtx,
    rf0: i8,
) -> (
    u8,
    [[i32; 2]; LEAST_SQUARES_SAMPLES_MAX],
    [[i32; 2]; LEAST_SQUARES_SAMPLES_MAX],
) {
    let mut pts = [[0i32; 2]; LEAST_SQUARES_SAMPLES_MAX];
    let mut pts_inref = [[0i32; 2]; LEAST_SQUARES_SAMPLES_MAX];
    let mut np: usize = 0;
    let (mut do_tl, mut do_tr) = (true, true);
    let stride = grid.stride;

    // C's `mbmi->block_mi.ref_frame[0] == rf0 && ref_frame[1] == NONE_FRAME`.
    let matches = |e: &crate::intrabc_mvp::MvpMiEntry| -> bool {
        e.ref_frame[0] == rf0 && e.ref_frame[1] == NONE_FRAME
    };
    let n4w = |e: &crate::intrabc_mvp::MvpMiEntry| -> i32 {
        i32::from(svtav1_types::tables::block::BLOCK_SIZE_WIDE[e.bsize as usize]) >> 2
    };
    let n4h = |e: &crate::intrabc_mvp::MvpMiEntry| -> i32 {
        i32::from(svtav1_types::tables::block::BLOCK_SIZE_HIGH[e.bsize as usize]) >> 2
    };

    // ---- the nearest ABOVE row ----
    if ctx.up_available {
        let above = *grid.at(-stride);
        let a_n4_w = n4w(&above);
        if ctx.n8_w <= a_n4_w {
            // C `int col_offset = -mi_col % n4_w;` — a C remainder, which is
            // NEGATIVE for a positive mi_col, and both suppressions below read
            // that sign. Rust's `%` on i32 truncates toward zero exactly as
            // C's does, so this is the same expression.
            let col_offset = -ctx.mi_col % a_n4_w;
            if col_offset < 0 {
                do_tl = false;
            }
            if col_offset + a_n4_w > ctx.n8_w {
                do_tr = false;
            }
            if matches(&above) {
                let (p, q) = record_samples(&above, 0, -1, col_offset, 1);
                pts[np] = p;
                pts_inref[np] = q;
                np += 1;
                if np >= LEAST_SQUARES_SAMPLES_MAX {
                    return (LEAST_SQUARES_SAMPLES_MAX as u8, pts, pts_inref);
                }
            }
        } else {
            let mut i = 0i32;
            let end = ctx.n8_w.min(ctx.mi_cols - ctx.mi_col);
            while i < end {
                let e = *grid.at(i - stride);
                // C `mi_step = AOMMIN(xd->n4_w, n4_w)` with no lower bound —
                // it does not need one, because every `BlockSize` is at least
                // 4 px wide and `mi_size_wide` is therefore at least 1. The
                // `.max(1)` is a LOOP-TERMINATION guard on a value C proves
                // and Rust does not; it cannot change behaviour for any real
                // bsize, and without it a corrupt grid entry would hang.
                let step = ctx.n8_w.min(n4w(&e)).max(1);
                if matches(&e) {
                    let (p, q) = record_samples(&e, 0, -1, i, 1);
                    pts[np] = p;
                    pts_inref[np] = q;
                    np += 1;
                    if np >= LEAST_SQUARES_SAMPLES_MAX {
                        return (LEAST_SQUARES_SAMPLES_MAX as u8, pts, pts_inref);
                    }
                }
                i += step;
            }
        }
    }

    // ---- the nearest LEFT column ----
    if ctx.left_available {
        let left = *grid.at(-1);
        let l_n4_h = n4h(&left);
        if ctx.n8_h <= l_n4_h {
            let row_offset = -ctx.mi_row % l_n4_h;
            if row_offset < 0 {
                do_tl = false;
            }
            if matches(&left) {
                let (p, q) = record_samples(&left, row_offset, 1, 0, -1);
                pts[np] = p;
                pts_inref[np] = q;
                np += 1;
                if np >= LEAST_SQUARES_SAMPLES_MAX {
                    return (LEAST_SQUARES_SAMPLES_MAX as u8, pts, pts_inref);
                }
            }
        } else {
            let mut i = 0i32;
            let end = ctx.n8_h.min(ctx.mi_rows - ctx.mi_row);
            while i < end {
                let e = *grid.at(i * stride - 1);
                // Same guard as the above scan — see there.
                let step = ctx.n8_h.min(n4h(&e)).max(1);
                if matches(&e) {
                    let (p, q) = record_samples(&e, i, 1, 0, -1);
                    pts[np] = p;
                    pts_inref[np] = q;
                    np += 1;
                    if np >= LEAST_SQUARES_SAMPLES_MAX {
                        return (LEAST_SQUARES_SAMPLES_MAX as u8, pts, pts_inref);
                    }
                }
                i += step;
            }
        }
    }

    // ---- TOP-LEFT ----
    if do_tl && ctx.left_available && ctx.up_available {
        let e = *grid.at(-stride - 1);
        if matches(&e) {
            let (p, q) = record_samples(&e, 0, -1, 0, -1);
            pts[np] = p;
            pts_inref[np] = q;
            np += 1;
            if np >= LEAST_SQUARES_SAMPLES_MAX {
                return (LEAST_SQUARES_SAMPLES_MAX as u8, pts, pts_inref);
            }
        }
    }

    // ---- TOP-RIGHT ----
    if do_tr
        && crate::intrabc_mvp::has_top_right(grid, ctx, ctx.n8_w.max(ctx.n8_h))
        && crate::intrabc_mvp::is_inside(ctx.tile, ctx.mi_col, ctx.mi_row, -1, ctx.n8_w)
    {
        let e = *grid.at(ctx.n8_w - stride);
        if matches(&e) {
            let (p, q) = record_samples(&e, 0, -1, ctx.n8_w, 1);
            pts[np] = p;
            pts_inref[np] = q;
            np += 1;
        }
    }

    (np as u8, pts, pts_inref)
}

#[cfg(test)]
mod find_warp_samples_tests {
    use super::*;
    use crate::intrabc::TileMiBounds;
    use crate::intrabc_mvp::{MvpGrid, MvpMiEntry, derive_block_ctx};

    const MI: i32 = 16; // a 64x64 frame in 4x4 mi units
    const BSIZE_8X8: usize = 3;

    fn tile() -> TileMiBounds {
        TileMiBounds {
            mi_row_start: 0,
            mi_col_start: 0,
            mi_row_end: MI,
            mi_col_end: MI,
        }
    }

    /// An 8x8 neighbour predicting from `rf0` with MV `(mvy, mvx)` in
    /// eighth-pel, or an INTRA cell when `rf0` is `INTRA_FRAME`.
    fn cell(rf0: i8, rf1: i8, mvy: i16, mvx: i16) -> MvpMiEntry {
        MvpMiEntry {
            bsize: BSIZE_8X8 as u8,
            mode: 13, // NEARESTMV
            use_intrabc: false,
            ref_frame: [rf0, rf1],
            mv: [Mv { x: mvx, y: mvy }, Mv::ZERO],
            partition: 0,
            interp_filters: 0,
        }
    }

    /// Grid of intra cells with the above row, left column and top-left
    /// corner of the block at mi `(4,4)` set to `n`.
    fn grid_with_neighbours(n: MvpMiEntry) -> alloc::vec::Vec<MvpMiEntry> {
        let mut g = alloc::vec![MvpMiEntry::default(); (MI * MI) as usize];
        for c in 0..MI {
            g[(3 * MI + c) as usize] = n; // the whole row above
        }
        for r in 0..MI {
            g[(r * MI + 3) as usize] = n; // the whole column left
        }
        g
    }

    fn run(entries: &[MvpMiEntry], rf0: i8) -> u8 {
        let ctx = derive_block_ctx(4, 4, BSIZE_8X8, MI, MI, tile(), MI);
        let grid = MvpGrid {
            entries,
            stride: MI,
            base: 4 * MI + 4,
        };
        find_warp_samples(&grid, &ctx, rf0).0
    }

    /// TIER 4 — ALL FOUR scans fire for an 8x8 block at mi (4,4) with 8x8
    /// neighbours above, left, top-left and top-right.
    /// `av1_find_samples` is `static` with no exported symbol, so this is
    /// hand-derived against `adaptive_mv_pred.c:1610-1750`:
    ///
    /// * above: `xd->n4_w`(2) `<= n4_w`(2), so `col_offset = -4 % 2 = 0`;
    ///   `col_offset < 0` is false so `do_tl` survives, and
    ///   `col_offset + n4_w > xd->n4_w` is `2 > 2` = false so `do_tr`
    ///   survives too. Sample 1.
    /// * left: the mirror, `row_offset = -4 % 2 = 0`. Sample 2.
    /// * top-left: `do_tl` and both edges. Sample 3.
    /// * top-right: `do_tr`, `has_top_right` (`mask_row & 2` and
    ///   `mask_col & 2` are both 0 at mi 4, so it is not suppressed) and
    ///   `is_inside`. Sample 4.
    ///
    /// **The first draft of this test expected THREE and was wrong** — it
    /// forgot the top-right scan, and the implementation was right. Recorded
    /// because a test written to agree with the code it tests is worth
    /// nothing; this one was re-derived from C after it failed.
    ///
    /// It is a POSITIVE CONTROL as much as a value check: the whole point of
    /// this function is that the count is NON-ZERO where C's is, because a
    /// zero writes the wrong motion-mode ALPHABET
    /// (`docs/INTER-ENCODE-PLAN.md` §1z¹⁸).
    #[test]
    fn counts_all_four_scans_for_an_8x8_with_8x8_neighbours() {
        let g = grid_with_neighbours(cell(1, NONE_FRAME, -8, 16));
        assert_eq!(run(&g, 1), 4, "above + left + top-left + top-right");
    }

    /// TIER 4 — a neighbour that predicts from a DIFFERENT reference is not a
    /// sample. C's test is `ref_frame[0] == rf0`, and the scan is run once per
    /// reference precisely because the answer differs per reference.
    #[test]
    fn a_different_reference_contributes_nothing() {
        let g = grid_with_neighbours(cell(1, NONE_FRAME, -8, 16));
        assert_eq!(run(&g, 5), 0, "BWDREF has no samples here");
    }

    /// TIER 4 — a COMPOUND neighbour is not a sample either: C requires
    /// `ref_frame[1] == NONE_FRAME`. Without this half of the test a port
    /// that dropped the second condition would still pass the two above.
    #[test]
    fn a_compound_neighbour_contributes_nothing() {
        let g = grid_with_neighbours(cell(1, 5, -8, 16));
        assert_eq!(run(&g, 1), 0, "LAST_BWD neighbours are not samples");
    }

    /// TIER 4 — an all-INTRA neighbourhood gives zero, which is the case the
    /// port used to hard-code for every block.
    #[test]
    fn an_intra_neighbourhood_gives_zero() {
        let g = alloc::vec![MvpMiEntry::default(); (MI * MI) as usize];
        assert_eq!(run(&g, 1), 0);
    }

    /// TIER 4 — the recorded POINT, hand-derived from C's `record_samples`
    /// (adaptive_mv_pred.c:1594) for the ABOVE neighbour of an 8x8 block at
    /// mi (4,4) with 8x8 neighbours:
    ///
    /// `n4_w` is 2 and `xd->n4_w` is 2, so C takes the "current block width
    /// <= above block width" arm with `col_offset = -mi_col % n4_w = -4 % 2 =
    /// 0`. Then `x = 0*4 + 1*max(8,4)/2 - 1 = 3` and
    /// `y = 0*4 + (-1)*max(8,4)/2 - 1 = -5`, and the point is `(x*8, y*8)`
    /// with the projection offset by the neighbour's own MV.
    ///
    /// The MV components are deliberately DIFFERENT (`mv = (y=-8, x=16)`) so
    /// a transposed `pts_inref` cannot pass.
    #[test]
    fn the_recorded_point_matches_c_for_the_above_neighbour() {
        let g = grid_with_neighbours(cell(1, NONE_FRAME, -8, 16));
        let ctx = derive_block_ctx(4, 4, BSIZE_8X8, MI, MI, tile(), MI);
        let grid = MvpGrid {
            entries: &g,
            stride: MI,
            base: 4 * MI + 4,
        };
        let (n, pts, pts_inref) = find_warp_samples(&grid, &ctx, 1);
        assert_eq!(n, 4);
        assert_eq!(pts[0], [3 * 8, -5 * 8], "the above neighbour's centre");
        assert_eq!(
            pts_inref[0],
            [3 * 8 + 16, -5 * 8 + -8],
            "projected through the neighbour's own MV"
        );
    }
}
