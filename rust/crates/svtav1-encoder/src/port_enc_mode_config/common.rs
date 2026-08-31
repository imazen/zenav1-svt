//! `svt_aom_sig_deriv_enc_dec_common` (`Codec/enc_mode_config.c:7086`) — the
//! per-SB spine that ALL THREE arms call, allintra included
//! (`product_coding_loop.c:10867` is downstream of it), plus
//! `set_depth_removal_level_controls` (`:2965`), the largest table in the file.
//!
//! Its absence was a STILL-path gap that the current gates cannot see, because
//! the port hardcodes the values it would produce.
//!
//! **Tier 1** — the entry point is EXPORTED and `c_parity_sig_deriv_common.rs`
//! drives the real symbol.

use super::enc_mode::*;
use super::leaf::{
    dimensions_require_8x8, get_disallow_8x8_allintra, get_disallow_8x8_default,
    get_disallow_8x8_rtc,
};
use super::tail::rdcost;

/// C `LOW_8x8_DIST_VAR_TH` (`enc_mode_config.c:9`).
pub const LOW_8X8_DIST_VAR_TH: u32 = 25_000;
/// C `HIGH_8x8_DIST_VAR_TH` (`enc_mode_config.c:10`).
pub const HIGH_8X8_DIST_VAR_TH: u32 = 50_000;

/// C `PD0_DEPTH_NO_RESTRICTION` (`md_process.h:227`).
pub const PD0_DEPTH_NO_RESTRICTION: u8 = 0;
/// C `PD0_DEPTH_ADAPTIVE` (`md_process.h:228`).
pub const PD0_DEPTH_ADAPTIVE: u8 = 1;
/// C `PD0_DEPTH_PRED_PART_ONLY` (`md_process.h:229`).
pub const PD0_DEPTH_PRED_PART_ONLY: u8 = 2;

/// The `mode` field of C `set_block_based_depth_refinement_controls`
/// (`enc_mode_config.c:6816`, EXPORTED).
///
/// Only `mode` is translated here — it is the single field
/// `svt_aom_sig_deriv_enc_dec_common` reads (to set `pred_depth_only`). The
/// other thirteen fields of `DepthRefinementCtrls` belong to the depth-
/// refinement port, not this one.
#[must_use]
pub fn depth_refinement_mode(block_based_depth_refinement_level: u8) -> Option<u8> {
    match block_based_depth_refinement_level {
        0 => Some(PD0_DEPTH_NO_RESTRICTION),
        1..=9 => Some(PD0_DEPTH_ADAPTIVE),
        10 => Some(PD0_DEPTH_PRED_PART_ONLY),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// set_depth_removal_level_controls
// ---------------------------------------------------------------------------

/// C `DepthRemovalCtrls` (`md_process.h:213`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DepthRemovalCtrls {
    /// `enabled`
    pub enabled: u8,
    /// `disallow_below_64x64`
    pub disallow_below_64x64: u8,
    /// `disallow_below_32x32`
    pub disallow_below_32x32: u8,
    /// `disallow_below_16x16`
    pub disallow_below_16x16: u8,
    /// `disallow_4x4` — a field of the struct, but note the function writes
    /// `ctx->disallow_4x4`, NOT this one.
    pub disallow_4x4: u8,
}

/// The per-level constants of `set_depth_removal_level_controls`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DepthRemovalRow {
    disallow_4x4_mult: u64,
    below_16x16_mult: u64,
    below_32x32_mult: u64,
    below_64x64_mult: u64,
    dev_16x16_to_8x8_th: i64,
    dev_32x32_to_16x16_th: i64,
    qp_scale_factor: i8,
}

/// `enc_mode_config.c:3003-3157`. Level 0 is "disabled" and has no row.
///
/// NOTE C's `switch` has NO `default:` arm here — a level above 15 falls
/// through with `enabled` left at whatever the zeroed struct held (0), which is
/// the same observable outcome as level 0. The port returns `None` so the
/// caller sees the out-of-domain level instead of a silently-plausible one.
fn depth_removal_row(level: u8) -> Option<DepthRemovalRow> {
    let r = |b16: u64, b32: u64, b64: u64, d16: i64, d32: i64, q: i8| DepthRemovalRow {
        disallow_4x4_mult: 64,
        below_16x16_mult: b16,
        below_32x32_mult: b32,
        below_64x64_mult: b64,
        dev_16x16_to_8x8_th: d16,
        dev_32x32_to_16x16_th: d32,
        qp_scale_factor: q,
    };
    Some(match level {
        1 => r(0, 0, 0, 0, 0, 1),
        2 => r(0, 0, 0, 10, 0, 1),
        3 => r(0, 0, 0, 20, 0, 1),
        4 => r(0, 0, 0, 30, 0, 1),
        5 => r(6, 6, 0, 40, 0, 1),
        6 => r(6, 6, 0, 50, 25, 1),
        7 => r(6, 6, 0, 50, 25, 2),
        8 => r(16, 8, 8, 100, 50, 3),
        9 => r(32, 8, 8, 100, 50, 3),
        10 => r(128, 8, 8, 200, 75, 3),
        11 => r(128, 8, 8, 250, 125, 3),
        12 => r(128, 16, 8, 250, 150, 4),
        13 => r(256, 16, 8, 250, 150, 4),
        14 => r(256, 16, 16, 250, 150, 4),
        15 => r(384, 24, 24, 300, 200, 4),
        _ => return None,
    })
}

/// The inputs C `set_depth_removal_level_controls` reads.
#[derive(Debug, Clone, Copy)]
pub struct DepthRemovalInputs {
    /// `depth_removal_level` (`pcs->pic_depth_removal_level`)
    pub level: u8,
    /// `pcs->slice_type == I_SLICE`
    pub is_islice: bool,
    /// `ctx->fast_lambda_md[EB_8_BIT_MD]` — the ME distortion is always 8-bit
    /// here, so the hbd lambda is NOT used.
    pub fast_lambda_8bit: u32,
    /// `ppcs->frm_hdr.delta_q_params.delta_q_present`
    pub delta_q_present: bool,
    /// `ppcs->r0_delta_qp_md`
    pub r0_delta_qp_md: bool,
    /// `ctx->sb_ptr->qindex`
    pub sb_qindex: i32,
    /// `quantizer_to_qindex[ppcs->picture_qp]`
    pub picture_qindex: i32,
    /// `ppcs->picture_qp`
    pub picture_qp: i32,
    /// `ppcs->me_64x64_distortion[sb_index]`
    pub dist_64: u32,
    /// `ppcs->me_32x32_distortion[sb_index]`
    pub dist_32: u32,
    /// `ppcs->me_16x16_distortion[sb_index]`
    pub dist_16: u32,
    /// `ppcs->me_8x8_distortion[sb_index]`
    pub dist_8: u32,
    /// `ppcs->me_8x8_cost_variance[sb_index]`
    pub me_8x8_cost_variance: u32,
    /// `ppcs->sb_geom[sb_index].width`
    pub sb_width: u16,
    /// `ppcs->sb_geom[sb_index].height`
    pub sb_height: u16,
    /// `ctx->disallow_4x4` on entry.
    pub disallow_4x4_in: bool,
    /// The reference-frame `sb_min_sq_size` adjustment: `Some(size)` when a
    /// same-size reference within one POC is available, `None` otherwise
    /// (C's `(uint8_t)~0` sentinel).
    pub ref_sb_min_sq_size: Option<u8>,
}

/// What `set_depth_removal_level_controls` writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DepthRemovalResult {
    /// `ctx->depth_removal_ctrls`
    pub ctrls: DepthRemovalCtrls,
    /// `ctx->disallow_4x4` after the function (it can only be SET, never
    /// cleared).
    pub disallow_4x4: bool,
}

/// C `set_depth_removal_level_controls` (`enc_mode_config.c:2965`). static —
/// reached at tier 1 through `svt_aom_sig_deriv_enc_dec_common`.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn set_depth_removal_level_controls(i: DepthRemovalInputs) -> Option<DepthRemovalResult> {
    let mut ctrls = DepthRemovalCtrls::default();
    if i.is_islice {
        ctrls.enabled = 0;
        return Some(DepthRemovalResult {
            ctrls,
            disallow_4x4: i.disallow_4x4_in,
        });
    }

    // ME distortion is 8-bit, so the 8-bit lambda is used unconditionally.
    let fast_lambda = i64::from(i.fast_lambda_8bit);
    let sb_size: i64 = 64 * 64;
    let cost_th_rate: i64 = 1 << 13;

    // Modulate the level using the SB delta-qp.
    let mut level = i.level;
    if i.delta_q_present || i.r0_delta_qp_md {
        let diff = i.sb_qindex - i.picture_qindex;
        let sub = if diff <= -12 {
            4
        } else if diff <= -6 {
            3
        } else if diff <= -3 {
            2
        } else if diff < 0 {
            1
        } else {
            0
        };
        level = u8::try_from(i32::from(level).saturating_sub(sub).max(0)).unwrap_or(0);
    }

    if level == 0 {
        ctrls.enabled = 0;
        return Some(DepthRemovalResult {
            ctrls,
            disallow_4x4: i.disallow_4x4_in,
        });
    }
    let row = depth_removal_row(level)?;
    ctrls.enabled = 1;

    let mut dev_16x16_to_8x8_th = row.dev_16x16_to_8x8_th;
    let mut dev_32x32_to_16x16_th = row.dev_32x32_to_16x16_th;

    // Reference-frame information (skipped entirely under rtc).
    if let Some(sb_min_sq_size) = i.ref_sb_min_sq_size {
        if sb_min_sq_size >= 64 {
            dev_32x32_to_16x16_th += 5;
            dev_16x16_to_8x8_th += 20;
        } else if sb_min_sq_size >= 32 {
            dev_16x16_to_8x8_th += 15;
        }
    }

    // dev thresholds = f(me_8x8_cost_variance). NOTE the divisor is
    // `MAX(MAX(63 - (qp + 10), 1), 1)`, i.e. the inner MAX already floors at 1
    // and the outer one is redundant — reproduced as written.
    let qp_term = (63 - (i.picture_qp + 10)).max(1);
    let me_8x8_cost_variance = i.me_8x8_cost_variance / (qp_term.max(1) as u32);
    if me_8x8_cost_variance < LOW_8X8_DIST_VAR_TH {
        dev_16x16_to_8x8_th <<= 2;
    } else if me_8x8_cost_variance < HIGH_8X8_DIST_VAR_TH {
        dev_16x16_to_8x8_th <<= 1;
        dev_32x32_to_16x16_th >>= 1;
    } else {
        dev_16x16_to_8x8_th = 0;
        dev_32x32_to_16x16_th = 0;
    }

    // dev thresholds = f(QP). The shift is applied to the SAME qp_term.
    let qp_mult = i64::from((qp_term >> 4).max(1)) * i64::from(row.qp_scale_factor);
    dev_16x16_to_8x8_th *= qp_mult;
    dev_32x32_to_16x16_th *= qp_mult;
    // dev_32x32_to_8x8_th = f(dev_32x32_to_16x16_th); a bit higher.
    let dev_32x32_to_8x8_th = (dev_32x32_to_16x16_th * ((1 << 2) + 1)) >> 2;

    let th = |mult: u64| -> u64 {
        if mult == 0 {
            0
        } else {
            rdcost(fast_lambda, cost_th_rate, (sb_size >> 3) * mult as i64) as u64
        }
    };
    let disallow_below_16x16_cost_th = th(row.below_16x16_mult);
    let disallow_below_32x32_cost_th = th(row.below_32x32_mult);
    let disallow_below_64x64_cost_th = th(row.below_64x64_mult);

    let cost = |d: u32| -> u64 { rdcost(fast_lambda, 0, i64::from(d)) as u64 };
    let cost_64x64 = cost(i.dist_64);
    let cost_32x32 = cost(i.dist_32);
    let cost_16x16 = cost(i.dist_16);
    let cost_8x8 = cost(i.dist_8);

    let dev = |a: u64, b: u64| -> i64 {
        let a = (a.max(1)) as i64;
        let b = (b.max(1)) as i64;
        ((a - b) * 1000) / b
    };
    let dev_32x32_to_16x16 = dev(cost_32x32, cost_16x16);
    let dev_32x32_to_8x8 = dev(cost_32x32, cost_8x8);
    let dev_16x16_to_8x8 = dev(cost_16x16, cost_8x8);

    // Enable depth removal at a depth only if the whole SB can be covered by
    // blocks of that size.
    let w = u32::from(i.sb_width);
    let h = u32::from(i.sb_height);
    // NOTE the incoming ctrls fields are ZERO here (the caller cleared them),
    // so the `X || ...` disjunctions reduce to the right-hand side; the port
    // keeps the shape so a future non-zero entry state behaves the same.
    ctrls.disallow_below_64x64 = u8::from(
        ((w % 64) == 0 || (w % 64) > 32)
            && ((h % 64) == 0 || (h % 64) > 32)
            && (ctrls.disallow_below_64x64 != 0 || cost_64x64 < disallow_below_64x64_cost_th),
    );
    ctrls.disallow_below_32x32 = u8::from(
        ((w % 32) == 0 || (w % 32) > 16)
            && ((h % 32) == 0 || (h % 32) > 16)
            && (ctrls.disallow_below_32x32 != 0
                || cost_32x32 < disallow_below_32x32_cost_th
                || (dev_32x32_to_16x16 < dev_32x32_to_16x16_th
                    && dev_32x32_to_8x8 < dev_32x32_to_8x8_th)),
    );
    ctrls.disallow_below_16x16 = u8::from(
        !dimensions_require_8x8(i.sb_width, i.sb_height)
            && (ctrls.disallow_below_16x16 != 0
                || cost_16x16 < disallow_below_16x16_cost_th
                || dev_16x16_to_8x8 < dev_16x16_to_8x8_th),
    );

    let mut disallow_4x4 = i.disallow_4x4_in;
    if !disallow_4x4 && row.disallow_4x4_mult != 0 {
        let disallow_4x4_cost_th = rdcost(
            fast_lambda,
            cost_th_rate,
            (sb_size >> 1) * row.disallow_4x4_mult as i64,
        ) as u64;
        if cost_8x8 < disallow_4x4_cost_th && me_8x8_cost_variance < LOW_8X8_DIST_VAR_TH {
            disallow_4x4 = true;
        }
    }
    Some(DepthRemovalResult {
        ctrls,
        disallow_4x4,
    })
}

// ---------------------------------------------------------------------------
// get_max_block_size_{default, rtc, allintra}
// ---------------------------------------------------------------------------

/// C `get_max_block_size_default` (`enc_mode_config.c:6991`). static.
///
/// The video arm applies NO cap.
#[must_use]
pub fn get_max_block_size_default(super_block_size: u32) -> u32 {
    super_block_size
}

/// C `get_max_block_size_rtc` (`enc_mode_config.c:6995`). static.
///
/// Caps to half the SB when the SB's ME 8x8 cost variance exceeds a
/// preset-and-qp-scaled threshold. Incomplete edge SBs and I-slices bail to the
/// uncapped size.
#[must_use]
pub fn get_max_block_size_rtc(
    enc_mode: i8,
    super_block_size: u32,
    sb_width: u32,
    sb_height: u32,
    is_islice: bool,
    cap_qp_scaling: bool,
    static_qp: u32,
    me_8x8_cost_variance: u32,
) -> u32 {
    if sb_width < super_block_size || sb_height < super_block_size {
        return super_block_size;
    }
    if is_islice {
        return super_block_size;
    }
    let base_me_var_th: u32 = if enc_mode <= M8 {
        u32::MAX
    } else {
        HIGH_8X8_DIST_VAR_TH
    };
    let (qw, qwd) = super::me::get_qp_based_th_scaling_factors(cap_qp_scaling, static_qp);
    let me_var_th = if base_me_var_th == u32::MAX {
        base_me_var_th
    } else {
        super::me::divide_and_round(base_me_var_th * qw, qwd)
    };
    if me_8x8_cost_variance <= me_var_th {
        super_block_size
    } else {
        super_block_size >> 1
    }
}

/// C `get_max_block_size_allintra` (`enc_mode_config.c:7042`). static.
///
/// Same shape as the rtc arm but keyed on the SB's PIXEL variance
/// (`ppcs->variance[sb_index][ME_TIER_ZERO_PU_64x64]`), with a `uint16_t`
/// threshold.
#[must_use]
pub fn get_max_block_size_allintra(
    enc_mode: i8,
    super_block_size: u32,
    sb_width: u32,
    sb_height: u32,
    cap_qp_scaling: bool,
    static_qp: u32,
    sb_variance: u16,
) -> u32 {
    if sb_width < super_block_size || sb_height < super_block_size {
        return super_block_size;
    }
    let base_var_th_cap: u16 = if enc_mode <= M7 { u16::MAX } else { 7500 };
    let (qw, qwd) = super::me::get_qp_based_th_scaling_factors(cap_qp_scaling, static_qp);
    let var_th_cap = if base_var_th_cap == u16::MAX {
        base_var_th_cap
    } else {
        super::me::divide_and_round(u32::from(base_var_th_cap) * qw, qwd) as u16
    };
    if sb_variance <= var_th_cap {
        super_block_size
    } else {
        super_block_size >> 1
    }
}

// ---------------------------------------------------------------------------
// set_lpd1_ctrls — the level -> pd1_level mapping
// ---------------------------------------------------------------------------

/// C `REGULAR_PD1` (`definitions.h:774`) — **-1**.
pub const REGULAR_PD1: i8 = -1;

/// The `pd1_level` that C `set_lpd1_ctrls` (`enc_mode_config.c:5533`) stores
/// for a given `lpd1_lvl`.
///
/// This is the one field of `Lpd1Ctrls` that
/// `svt_aom_sig_deriv_enc_dec_common` makes observable, and it is NOT the
/// identity: level 0 maps to `REGULAR_PD1` (-1), levels 1..3 all map to
/// `LPD1_LVL_0` (0), and levels 4..8 map to LPD1 levels 1, 3, 4, 5, 6 — level
/// 2 of the LPD1 enum is never selected. The rest of `set_lpd1_ctrls` (the
/// nine per-level detector arrays) is NOT ported here.
#[must_use]
pub fn lpd1_pd1_level(lpd1_lvl: u8) -> Option<i8> {
    match lpd1_lvl {
        0 => Some(REGULAR_PD1),
        1..=3 => Some(0), // LPD1_LVL_0
        4 => Some(1),         // LPD1_LVL_1
        5 => Some(3),         // LPD1_LVL_3 — LPD1_LVL_2 is skipped
        6 => Some(4),         // LPD1_LVL_4
        7 => Some(5),         // LPD1_LVL_5
        8 => Some(6),         // LPD1_LVL_6
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// svt_aom_sig_deriv_enc_dec_common
// ---------------------------------------------------------------------------

/// Inputs of C `svt_aom_sig_deriv_enc_dec_common`.
#[derive(Debug, Clone, Copy)]
pub struct CommonInputs {
    /// `pcs->enc_mode`
    pub enc_mode: i8,
    /// `scs->static_config.rtc`
    pub rtc_tune: bool,
    /// `scs->allintra`
    pub allintra: bool,
    /// `!frame_is_leaf(ppcs)`
    pub is_not_last_layer: bool,
    /// `frame_is_boosted(ppcs)`
    pub is_base: bool,
    /// `pcs->pic_block_based_depth_refinement_level`
    pub pic_block_based_depth_refinement_level: u8,
    /// `ppcs->b64_geom[sb_index].width`
    pub b64_width: u16,
    /// `ppcs->b64_geom[sb_index].height`
    pub b64_height: u16,
    /// `pcs->pic_disallow_4x4`
    pub pic_disallow_4x4: bool,
    /// `scs->super_block_size`
    pub super_block_size: u32,
    /// `pcs->pic_lpd1_lvl`
    pub pic_lpd1_lvl: i32,
    /// `ctx->sb_ptr->qindex`
    pub sb_qindex: i32,
    /// `ppcs->frm_hdr.quantization_params.base_q_idx`
    pub base_q_idx: i32,
    /// `pcs->slice_type == I_SLICE`
    pub is_islice: bool,
    /// `ppcs->me_8x8_cost_variance[sb_index]`
    pub me_8x8_cost_variance: i32,
    /// `ctx->qp_index`
    pub qp_index: i32,
    /// `scs->static_config.max_tx_size`
    pub max_tx_size: u32,
    /// `ppcs->sb_geom[sb_index].width` — read by the rtc/allintra
    /// max-block-size arms (the b64 geometry is a different field).
    pub sb_geom_width: u32,
    /// `ppcs->sb_geom[sb_index].height`
    pub sb_geom_height: u32,
    /// `scs->qp_based_th_scaling_ctrls.cap_max_size_qp_based_th_scaling`
    pub cap_max_size_qp_based_th_scaling: bool,
    /// `scs->static_config.qp`
    pub static_qp: u32,
    /// `ppcs->variance[sb_index][ME_TIER_ZERO_PU_64x64]`
    pub sb_variance: u16,
    /// The inputs of the nested `set_depth_removal_level_controls` call.
    pub depth_removal: DepthRemovalInputs,
}

/// What `svt_aom_sig_deriv_enc_dec_common` writes, restricted to the fields
/// this lane models.
///
/// NOT modelled, each because it comes from a table this lane has not ported:
/// `ctx->depth_refinement_ctrls` beyond its `mode` (from
/// `set_block_based_depth_refinement_controls`), `ctx->pd0_ctrls` (from the
/// `static` `set_pd0_ctrls`), `ctx->lpd1_ctrls` (from `set_lpd1_ctrls`) and
/// `ctx->nsq_geom_ctrls` (from `svt_aom_set_nsq_geom_ctrls`). The LPD1 LEVEL
/// this function derives IS modelled, since deriving it is the part that lives
/// here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommonSignals {
    /// `ctx->depth_refinement_ctrls.mode`
    pub depth_refinement_mode: u8,
    /// `ctx->pred_depth_only` (== `ctx->pic_pred_depth_only`)
    pub pred_depth_only: bool,
    /// `ctx->depth_removal_ctrls`
    pub depth_removal: DepthRemovalCtrls,
    /// `ctx->disallow_8x8`
    pub disallow_8x8: bool,
    /// `ctx->disallow_4x4`
    pub disallow_4x4: bool,
    /// `ctx->max_block_size`
    pub max_block_size: u32,
    /// The LPD1 level passed to `set_lpd1_ctrls`.
    pub lpd1_lvl: i32,
    /// `ctx->lpd1_ctrls.pd1_level` — what `set_lpd1_ctrls` stores for
    /// [`Self::lpd1_lvl`]; see [`lpd1_pd1_level`], which is NOT the identity.
    pub lpd1_pd1_level: i8,
    /// `ctx->pd1_lvl_refinement`
    pub pd1_lvl_refinement: u8,
}

/// C `svt_aom_sig_deriv_enc_dec_common` (`enc_mode_config.c:7086`). EXPORTED.
#[must_use]
pub fn sig_deriv_enc_dec_common(i: CommonInputs) -> Option<CommonSignals> {
    let mode = depth_refinement_mode(i.pic_block_based_depth_refinement_level)?;
    let pred_depth_only = mode == PD0_DEPTH_PRED_PART_ONLY;

    // C zeroes the three disallow_below_* flags and then re-clears them under
    // b64-geometry conditions. Those re-clears are NO-OPS on a fresh call (the
    // flags are already 0); they are kept in the C for the case where the
    // struct arrives non-zero. Nothing observable depends on them here.

    // Must check disallow_8x8 on an SB level: a preset may want 8x8 off yet
    // still need it at a picture edge where the SB is <= 8 wide/tall.
    let disallow_8x8 = if i.allintra {
        get_disallow_8x8_allintra()
    } else if i.rtc_tune {
        get_disallow_8x8_rtc(i.enc_mode, i.b64_width, i.b64_height)
    } else {
        get_disallow_8x8_default()
    };

    let max_block_size = if i.allintra {
        get_max_block_size_allintra(
            i.enc_mode,
            i.super_block_size,
            i.sb_geom_width,
            i.sb_geom_height,
            i.cap_max_size_qp_based_th_scaling,
            i.static_qp,
            i.sb_variance,
        )
    } else if i.rtc_tune {
        get_max_block_size_rtc(
            i.enc_mode,
            i.super_block_size,
            i.sb_geom_width,
            i.sb_geom_height,
            i.is_islice,
            i.cap_max_size_qp_based_th_scaling,
            i.static_qp,
            u32::try_from(i.me_8x8_cost_variance).unwrap_or(0),
        )
    } else {
        get_max_block_size_default(i.super_block_size)
    };

    // set_depth_removal_level_controls, which can also SET ctx->disallow_4x4.
    let dr_in = DepthRemovalInputs {
        disallow_4x4_in: i.pic_disallow_4x4,
        ..i.depth_removal
    };
    let dr = set_depth_removal_level_controls(dr_in)?;

    // LPD1 level.
    let lpd1_lvl = if i.rtc_tune {
        let mut l = i.pic_lpd1_lvl;
        // For cyclic-refresh SBs signalled by a negative delta-QP, be
        // conservative.
        if l != 0 && i.sb_qindex < i.base_q_idx {
            l = (l - 2).max(0);
            l = l.min(if i.is_base { 2 } else { 4 });
        }
        l
    } else if i.enc_mode <= M10 {
        i.pic_lpd1_lvl
    } else {
        let mut l = i.pic_lpd1_lvl;
        if !i.is_islice {
            let me_8x8 = i.me_8x8_cost_variance;
            // NOTE the threshold is `3 * ctx->qp_index` at <= M8 and a FLAT
            // 3000 above it — not a scaled version of the same expression.
            let th = if i.enc_mode <= M8 {
                3 * i.qp_index
            } else {
                3000
            };
            if l == 0 {
                if me_8x8 < th {
                    l += 3;
                }
            } else if me_8x8 < th {
                l += 2;
            }
        }
        l.clamp(0, 7)
    };

    let pd1_lvl_refinement = if i.rtc_tune {
        if i.enc_mode <= M8 {
            0
        } else if i.enc_mode <= M10 {
            if i.is_not_last_layer { 0 } else { 2 }
        } else {
            2
        }
    } else if i.enc_mode <= M10 {
        0
    } else {
        2
    };

    let mut depth_removal = dr.ctrls;
    // Ensure at least 32x32 transforms remain available.
    if i.max_tx_size == 32 {
        depth_removal.disallow_below_64x64 = 0;
    }

    Some(CommonSignals {
        depth_refinement_mode: mode,
        pred_depth_only,
        depth_removal,
        disallow_8x8,
        disallow_4x4: dr.disallow_4x4,
        max_block_size,
        lpd1_lvl,
        lpd1_pd1_level: lpd1_pd1_level(u8::try_from(lpd1_lvl).ok()?)?,
        pd1_lvl_refinement,
    })
}
