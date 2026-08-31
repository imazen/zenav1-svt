//! Block-level inter mode syntax: the intra/inter flag, the non-key intra
//! luma mode, skip-mode, the single-ref and compound inter mode symbols, the
//! DRL index, the motion-mode gate and symbol, the interintra gate, and the
//! compound index / group-index contexts.
//!
//! C reference: `Source/Lib/Codec/entropy_coding.c`
//! (`svt_aom_get_comp_index_context_enc` :52,
//! `svt_aom_get_comp_group_idx_context_enc` :80,
//! `encode_intra_luma_mode_nonkey_av1` :1046, `av1_get_skip_mode_context`
//! :1097, `encode_skip_mode_av1` :1109, `write_is_inter` :1147,
//! `svt_aom_motion_mode_allowed` :1159, `write_motion_mode` :1198,
//! `write_inter_mode` :1383, `write_drl_idx` :1404,
//! `write_inter_compound_mode` :1627, `svt_aom_is_interintra_allowed` :4927).

use crate::entropy::context::FrameContext;
use crate::entropy::writer::AomWriter;
use crate::port_entropy_inter::refframe::INTRA_FRAME;
use crate::port_entropy_inter::{InterCdfs, Neighbors, intra_inter_context};
use svtav1_types::block::BlockSize;
use svtav1_types::tables::block::{BLOCK_SIZE_HIGH, BLOCK_SIZE_WIDE};

/// C `INTRA_MODES` — the y_mode alphabet.
pub const INTRA_MODES: usize = 13;
/// C `MAX_ANGLE_DELTA` (definitions.h:1327).
pub const MAX_ANGLE_DELTA: i8 = 3;
/// C `V_PRED`.
pub const V_PRED: u8 = 1;
/// C `D67_PRED`.
pub const D67_PRED: u8 = 8;
/// C `NEARESTMV` == `SINGLE_INTER_MODE_START`.
pub const NEARESTMV: u8 = 13;
/// C `NEARMV`.
pub const NEARMV: u8 = 14;
/// C `GLOBALMV`.
pub const GLOBALMV: u8 = 15;
/// C `NEWMV`.
pub const NEWMV: u8 = 16;
/// C `NEAREST_NEARESTMV` == `SINGLE_INTER_MODE_END` == `COMP_INTER_MODE_START`.
pub const NEAREST_NEARESTMV: u8 = 17;
/// C `NEAR_NEARMV`.
pub const NEAR_NEARMV: u8 = 18;
/// C `NEAR_NEWMV`.
pub const NEAR_NEWMV: u8 = 21;
/// C `NEW_NEARMV`.
pub const NEW_NEARMV: u8 = 22;
/// C `GLOBAL_GLOBALMV`.
pub const GLOBAL_GLOBALMV: u8 = 23;
/// C `NEW_NEWMV`.
pub const NEW_NEWMV: u8 = 24;
/// C `INTER_COMPOUND_MODES` = `1 + NEW_NEWMV - NEAREST_NEARESTMV`.
pub const INTER_COMPOUND_MODES: usize = 8;
/// C `MOTION_MODES` (definitions.h:1254).
pub const MOTION_MODES: usize = 3;
/// C `NEWMV_MODE_CONTEXTS` (definitions.h:1340) — note it is SMALLER than
/// what `NEWMV_CTX_MASK` admits.
pub const NEWMV_MODE_CONTEXTS: usize = 6;
/// C `GLOBALMV_MODE_CONTEXTS` (definitions.h:1341).
pub const GLOBALMV_MODE_CONTEXTS: usize = 2;
/// C `REFMV_MODE_CONTEXTS` (definitions.h:1342) — likewise smaller than
/// `REFMV_CTX_MASK` admits.
pub const REFMV_MODE_CONTEXTS: usize = 6;
/// C `DRL_MODE_CONTEXTS` (definitions.h:1343).
pub const DRL_MODE_CONTEXTS: usize = 3;
/// C `NEWMV_CTX_MASK` (definitions.h:1348).
pub const NEWMV_CTX_MASK: i16 = (1 << 3) - 1;
/// C `GLOBALMV_OFFSET` (definitions.h:1345).
pub const GLOBALMV_OFFSET: u32 = 3;
/// C `GLOBALMV_CTX_MASK` (definitions.h:1349).
pub const GLOBALMV_CTX_MASK: i16 = (1 << (4 - 3)) - 1;
/// C `REFMV_OFFSET` (definitions.h:1346).
pub const REFMV_OFFSET: u32 = 4;
/// C `REFMV_CTX_MASK` (definitions.h:1350).
pub const REFMV_CTX_MASK: i16 = (1 << (8 - 4)) - 1;

/// C `eb_size_group_lookup` (common_utils.c:36) — the `y_mode_cdf` row for a
/// block size. NOT the same shape as `entropy/context.rs`'s
/// `block_size_group(w, h)` helper (which derives it from dimensions); the
/// two agree on all 22 sizes, and this is the table C actually indexes.
#[rustfmt::skip]
pub const SIZE_GROUP_LOOKUP: [u8; 22] = [
    0, 0, 0, 1, 1, 1, 2, 2, 2, 3, 3,
    3, 3, 3, 3, 3, 0, 0, 1, 1, 2, 2,
];

/// C `av1_is_directional_mode` (intra_prediction.h:206).
#[inline]
pub const fn is_directional_mode(mode: u8) -> bool {
    mode >= V_PRED && mode <= D67_PRED
}

/// C `is_inter_singleref_mode` (definitions.h) — `[NEARESTMV, NEAREST_NEARESTMV)`.
#[inline]
pub const fn is_inter_singleref_mode(mode: u8) -> bool {
    mode >= NEARESTMV && mode < NEAREST_NEARESTMV
}

/// C `is_inter_compound_mode` — `[NEAREST_NEARESTMV, NEW_NEWMV]`.
#[inline]
pub const fn is_inter_compound_mode(mode: u8) -> bool {
    mode >= NEAREST_NEARESTMV && mode <= NEW_NEWMV
}

/// C `have_nearmv_in_inter_mode` (inter_prediction.h:417).
#[inline]
pub const fn have_nearmv_in_inter_mode(mode: u8) -> bool {
    matches!(mode, NEARMV | NEAR_NEARMV | NEAR_NEWMV | NEW_NEARMV)
}

/// C `is_motion_variation_allowed_bsize` (inter_prediction.h:407).
#[inline]
pub fn is_motion_variation_allowed_bsize(bsize: BlockSize) -> bool {
    let i = bsize.as_index();
    BLOCK_SIZE_WIDE[i] >= 8 && BLOCK_SIZE_HIGH[i] >= 8
}

/// C `TransformationType` (definitions.h:1755).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum TransformationType {
    /// 0-parameter.
    Identity = 0,
    /// 2-parameter.
    Translation = 1,
    /// 4-parameter.
    RotZoom = 2,
    /// 6-parameter.
    Affine = 3,
}

/// C `MotionMode` (definitions.h:1251).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum MotionMode {
    /// No extra symbol.
    SimpleTranslation = 0,
    /// 2-sided OBMC.
    ObmcCausal = 1,
    /// 2-sided warped.
    WarpedCausal = 2,
}

/// C `is_global_mv_block` (inter_prediction.h:411).
#[inline]
pub fn is_global_mv_block(mode: u8, bsize: BlockSize, ty: TransformationType) -> bool {
    (mode == GLOBALMV || mode == GLOBAL_GLOBALMV)
        && ty > TransformationType::Translation
        && is_motion_variation_allowed_bsize(bsize)
}

// ---- 1. write_is_inter (entropy_coding.c:1147) ----

/// C `write_is_inter` — the `intra_inter` symbol every block in a non-intra
/// frame emits first.
pub fn write_is_inter(w: &mut AomWriter, fc: &mut FrameContext, nb: &Neighbors, is_inter: bool) {
    let ctx = intra_inter_context(nb);
    w.write_symbol(usize::from(is_inter), &mut fc.intra_inter_cdf[ctx], 2);
}

// ---- 2. encode_intra_luma_mode_nonkey_av1 (entropy_coding.c:1046) ----

/// C `encode_intra_luma_mode_nonkey_av1` — an INTRA block inside an INTER
/// frame codes `y_mode_cdf[size_group]`, NOT the key-frame `kf_y_cdf` that
/// `entropy/context.rs::write_intra_mode_kf` uses.
///
/// Two exactnesses worth keeping:
/// * The `bsize >= BLOCK_8X8` gate is an ENUM-ORDINAL comparison, so it also
///   admits `BLOCK_4X16` (16) .. `BLOCK_64X16` (21) — including the two 4-wide
///   shapes. That is upstream AV1 behaviour, not an oversight.
/// * The directional test reads `mbmi->block_mi.mode` while the
///   `angle_delta_cdf` row is `luma_mode - V_PRED`. C passes both separately,
///   so both are parameters here.
pub fn encode_intra_luma_mode_nonkey(
    w: &mut AomWriter,
    fc: &mut FrameContext,
    bsize: BlockSize,
    mbmi_mode: u8,
    luma_mode: u8,
    angle_delta_y: i8,
) {
    let group = SIZE_GROUP_LOOKUP[bsize.as_index()] as usize;
    w.write_symbol(luma_mode as usize, &mut fc.y_mode_cdf[group], INTRA_MODES);

    if bsize as u8 >= BlockSize::Block8x8 as u8 && is_directional_mode(mbmi_mode) {
        let sym = (angle_delta_y + MAX_ANGLE_DELTA) as usize;
        let row = (luma_mode - V_PRED) as usize;
        w.write_symbol(
            sym,
            &mut fc.angle_delta_cdf[row],
            (2 * MAX_ANGLE_DELTA + 1) as usize,
        );
    }
}

// ---- 3. skip mode (entropy_coding.c:1097 / :1109) ----

/// C `av1_get_skip_mode_context` (entropy_coding.c:1097).
///
/// It tests the `above_mbmi` / `left_mbmi` POINTER, NOT `up_available` /
/// `left_available` — a genuinely different gate from the ref-count contexts.
pub fn skip_mode_context(nb: &Neighbors) -> usize {
    let a = nb.above.map(|m| usize::from(m.skip_mode)).unwrap_or(0);
    let l = nb.left.map(|m| usize::from(m.skip_mode)).unwrap_or(0);
    a + l
}

/// C `encode_skip_mode_av1` (entropy_coding.c:1109). Missing it shifts every
/// subsequent symbol in the block.
pub fn encode_skip_mode(
    w: &mut AomWriter,
    ic: &mut InterCdfs,
    nb: &Neighbors,
    skip_mode_flag: bool,
) {
    let ctx = skip_mode_context(nb);
    w.write_symbol(usize::from(skip_mode_flag), &mut ic.skip_mode_cdf[ctx], 2);
}

// ---- 4. write_inter_mode / write_inter_compound_mode (:1383 / :1627) ----

/// C `write_inter_mode` (entropy_coding.c:1383) — step 4, single-ref arm.
///
/// The three contexts are packed into one `mode_ctx` word by
/// `svt_aom_mode_context_analyzer`; the masks and shifts here are C's.
pub fn write_inter_mode(w: &mut AomWriter, ic: &mut InterCdfs, mode: u8, mode_ctx: i16) {
    let newmv_ctx = (mode_ctx & NEWMV_CTX_MASK) as usize;
    // C's own assert (entropy_coding.c:1387): the MASK is wider than the
    // table (7 vs NEWMV_MODE_CONTEXTS = 6), so an out-of-range `mode_ctx`
    // indexes past `newmv_cdf` in C too.
    debug_assert!(newmv_ctx < NEWMV_MODE_CONTEXTS);
    w.write_symbol(usize::from(mode != NEWMV), &mut ic.newmv_cdf[newmv_ctx], 2);

    if mode != NEWMV {
        let zeromv_ctx = ((mode_ctx >> GLOBALMV_OFFSET) & GLOBALMV_CTX_MASK) as usize;
        w.write_symbol(
            usize::from(mode != GLOBALMV),
            &mut ic.zeromv_cdf[zeromv_ctx],
            2,
        );

        if mode != GLOBALMV {
            let refmv_ctx = ((mode_ctx >> REFMV_OFFSET) & REFMV_CTX_MASK) as usize;
            // C's assert at entropy_coding.c:1397, same shape: the mask is 15
            // but REFMV_MODE_CONTEXTS is 6.
            debug_assert!(refmv_ctx < REFMV_MODE_CONTEXTS);
            w.write_symbol(
                usize::from(mode != NEARESTMV),
                &mut ic.refmv_cdf[refmv_ctx],
                2,
            );
        }
    }
}

/// C `write_inter_compound_mode` (entropy_coding.c:1627) — step 4, compound
/// arm. Together with [`write_inter_mode`] these exhaustively partition the
/// inter mode range, so every inter block emits exactly one of them.
pub fn write_inter_compound_mode(w: &mut AomWriter, ic: &mut InterCdfs, mode: u8, mode_ctx: i16) {
    debug_assert!(is_inter_compound_mode(mode));
    let sym = (mode - NEAREST_NEARESTMV) as usize; // C INTER_COMPOUND_OFFSET
    w.write_symbol(
        sym,
        &mut ic.inter_compound_mode_cdf[mode_ctx as usize],
        INTER_COMPOUND_MODES,
    );
}

// ---- 5. write_drl_idx (entropy_coding.c:1404) ----

/// The `EcBlkStruct` fields `write_drl_idx` reads.
#[derive(Clone, Copy, Debug, Default)]
pub struct DrlBlock {
    /// C `blk_ptr->drl_ctx[2]`; `-1` means "this position codes no bit".
    pub drl_ctx: [i8; 2],
    /// C `blk_ptr->drl_ctx_near[2]`.
    pub drl_ctx_near: [i8; 2],
    /// C `blk_ptr->drl_index`.
    pub drl_index: u8,
}

/// C `write_drl_idx` (entropy_coding.c:1404) — step 5.
///
/// Its predicate set is DIFFERENT from the MV-write predicate set: DRL fires
/// on `NEWMV || NEW_NEWMV || have_nearmv_in_inter_mode`, the MV write on
/// `have_newmv_in_inter_mode`. `NEARMV` and `NEAR_NEARMV` are DRL-only;
/// `NEAREST_NEWMV` and `NEW_NEARESTMV` are MV-only. Sharing one predicate is
/// wrong in four of the twelve inter modes (see `inter_mv_code.rs`).
///
/// Note the near arm's `idx` runs 1..3 while the compared index is `idx - 1`
/// — C's "temporary solution to compensate the NEARESTMV offset".
pub fn write_drl_idx(w: &mut AomWriter, ic: &mut InterCdfs, mode: u8, blk: &DrlBlock) {
    let new_mv = mode == NEWMV || mode == NEW_NEWMV;
    if new_mv {
        for idx in 0..2usize {
            if blk.drl_ctx[idx] != -1 {
                let ctx = blk.drl_ctx[idx] as usize;
                w.write_symbol(
                    usize::from(blk.drl_index as usize != idx),
                    &mut ic.drl_cdf[ctx],
                    2,
                );
                if blk.drl_index as usize == idx {
                    return;
                }
            }
        }
        return;
    }

    if have_nearmv_in_inter_mode(mode) {
        for idx in 1..3usize {
            if blk.drl_ctx_near[idx - 1] != -1 {
                let ctx = blk.drl_ctx_near[idx - 1] as usize;
                w.write_symbol(
                    usize::from(blk.drl_index as usize != idx - 1),
                    &mut ic.drl_cdf[ctx],
                    2,
                );
                if blk.drl_index as usize == idx - 1 {
                    return;
                }
            }
        }
    }
}

// ---- 6. motion mode (entropy_coding.c:1159 / :1198) ----

/// C `svt_aom_motion_mode_allowed` (entropy_coding.c:1159) — decides whether
/// a block codes ZERO, ONE (`obmc_cdf`) or a `MOTION_MODES` symbol. A wrong
/// answer changes the block's SYMBOL COUNT, which is unrecoverable.
///
/// Two literal details preserved from C:
/// * the global-motion early-out is skipped entirely when
///   `force_integer_mv != 0` (the `wmtype` is not even read then);
/// * the single-ref test is `rf1 != INTRA_FRAME && !(rf1 > INTRA_FRAME)`,
///   i.e. `rf1 < 0` (`NONE`). `rf1 == INTRA_FRAME` (0) does NOT qualify.
#[allow(clippy::too_many_arguments)]
pub fn motion_mode_allowed(
    is_motion_mode_switchable: bool,
    force_integer_mv: bool,
    allow_warped_motion: bool,
    gm_wmtype: &[TransformationType; 8],
    num_proj_ref: u16,
    overlappable_neighbors: u32,
    bsize: BlockSize,
    rf0: i8,
    rf1: i8,
    mode: u8,
) -> MotionMode {
    if !is_motion_mode_switchable {
        return MotionMode::SimpleTranslation;
    }
    if !force_integer_mv {
        let gm_type = gm_wmtype[rf0.clamp(0, 7) as usize];
        if is_global_mv_block(mode, bsize, gm_type) {
            return MotionMode::SimpleTranslation;
        }
    }
    if is_motion_variation_allowed_bsize(bsize)
        && is_inter_singleref_mode(mode)
        && rf1 != INTRA_FRAME
        && !(rf1 > INTRA_FRAME)
    {
        if overlappable_neighbors == 0 {
            return MotionMode::SimpleTranslation;
        }
        if allow_warped_motion && num_proj_ref >= 1 {
            if force_integer_mv {
                return MotionMode::ObmcCausal;
            }
            return MotionMode::WarpedCausal;
        }
        MotionMode::ObmcCausal
    } else {
        MotionMode::SimpleTranslation
    }
}

/// C `write_motion_mode` (entropy_coding.c:1198) — step 8.
#[allow(clippy::too_many_arguments)]
pub fn write_motion_mode(
    w: &mut AomWriter,
    ic: &mut InterCdfs,
    bsize: BlockSize,
    motion_mode: MotionMode,
    last_motion_mode_allowed: MotionMode,
) {
    match last_motion_mode_allowed {
        MotionMode::SimpleTranslation => {}
        MotionMode::ObmcCausal => {
            let bit = usize::from(motion_mode == MotionMode::ObmcCausal);
            w.write_symbol(bit, &mut ic.obmc_cdf[bsize.as_index()], 2);
        }
        MotionMode::WarpedCausal => {
            w.write_symbol(
                motion_mode as usize,
                &mut ic.motion_mode_cdf[bsize.as_index()],
                MOTION_MODES,
            );
        }
    }
}

// ---- 7. interintra gate (entropy_coding.c:4927) ----

/// C `svt_aom_is_interintra_allowed_bsize` (mode_decision.h:142) — an
/// ENUM-ORDINAL range, `BLOCK_8X8 (3) ..= BLOCK_32X32 (9)`.
#[inline]
pub fn is_interintra_allowed_bsize(bsize: BlockSize) -> bool {
    let b = bsize as u8;
    b >= BlockSize::Block8x8 as u8 && b <= BlockSize::Block32x32 as u8
}

/// C `svt_aom_is_interintra_allowed_mode` (mode_decision.h:146).
#[inline]
pub const fn is_interintra_allowed_mode(mode: u8) -> bool {
    is_inter_singleref_mode(mode)
}

/// C `svt_aom_is_interintra_allowed_ref` (mode_decision.h:150).
#[inline]
pub const fn is_interintra_allowed_ref(rf: [i8; 2]) -> bool {
    rf[0] > INTRA_FRAME && rf[1] <= INTRA_FRAME
}

/// C `svt_aom_is_interintra_allowed` (entropy_coding.c:4927) — step 7's gate.
#[inline]
pub fn is_interintra_allowed(bsize: BlockSize, mode: u8, ref_frame: [i8; 2]) -> bool {
    is_interintra_allowed_bsize(bsize)
        && is_interintra_allowed_mode(mode)
        && is_interintra_allowed_ref(ref_frame)
}

// ---- 8. compound index / group index contexts (:52 / :80) ----

/// C `svt_aom_get_relative_dist_enc` (inter_prediction.c:273) — the wrapped
/// order-hint difference, 0 when order hints are disabled.
#[inline]
pub fn get_relative_dist(enable_order_hint: bool, order_hint_bits: u32, a: i32, b: i32) -> i32 {
    if !enable_order_hint {
        return 0;
    }
    let diff = a - b;
    let m = 1i32 << (order_hint_bits - 1);
    (diff & (m - 1)) - (diff & m)
}

/// C `svt_aom_get_comp_index_context_enc` (entropy_coding.c:52) — step 9's
/// context for the `compound_index` symbol.
///
/// Tests the neighbour POINTERS, not `up_available` / `left_available`.
#[allow(clippy::too_many_arguments)]
pub fn comp_index_context(
    enable_order_hint: bool,
    order_hint_bits: u32,
    cur_frame_index: i32,
    bck_frame_index: i32,
    fwd_frame_index: i32,
    nb: &Neighbors,
) -> usize {
    let fwd = get_relative_dist(
        enable_order_hint,
        order_hint_bits,
        fwd_frame_index,
        cur_frame_index,
    )
    .abs();
    let bck = get_relative_dist(
        enable_order_hint,
        order_hint_bits,
        cur_frame_index,
        bck_frame_index,
    )
    .abs();
    let offset = usize::from(fwd == bck);

    let ctx_of = |mi: &Option<crate::port_entropy_inter::NeighborMi>| -> usize {
        match mi {
            Some(m) if m.has_second_ref() => m.compound_idx as usize,
            Some(m) if m.ref_frame[0] == crate::port_entropy_inter::refframe::ALTREF_FRAME => 1,
            _ => 0,
        }
    };
    ctx_of(&nb.above) + ctx_of(&nb.left) + 3 * offset
}

/// C `svt_aom_get_comp_group_idx_context_enc` (entropy_coding.c:80).
///
/// The ALTREF single-ref arm contributes **3**, not 1 as in
/// [`comp_index_context`]; the sum is then clamped with `AOMMIN(5, …)`.
pub fn comp_group_idx_context(nb: &Neighbors) -> usize {
    let ctx_of = |mi: &Option<crate::port_entropy_inter::NeighborMi>| -> usize {
        match mi {
            Some(m) if m.has_second_ref() => m.comp_group_idx as usize,
            Some(m) if m.ref_frame[0] == crate::port_entropy_inter::refframe::ALTREF_FRAME => 3,
            _ => 0,
        }
    };
    (ctx_of(&nb.above) + ctx_of(&nb.left)).min(5)
}
