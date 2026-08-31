//! The remaining `enc_mode_config.c` tables: loop-restoration SGR search
//! controls, CDEF recon controls, the two PD0 cost thresholds, the mfmv
//! frame-header decision and the TPL intra-stat reader.
//!
//! All five are file-`static` in C with no exported symbol, so they carry
//! **tier 4** evidence (hand-derived vectors traced against the C source) — the
//! weakest tier, and the tests say so.

use super::enc_mode::*;

/// C `PLANE_TYPES` (`definitions.h:706`) — the length of the per-plane arrays
/// in [`SgFilterCtrls`]: index 0 is luma, index 1 is chroma.
pub const PLANE_TYPES: usize = 2;

// ---------------------------------------------------------------------------
// Self-guided (SGR) loop-restoration search controls
// ---------------------------------------------------------------------------

/// C `SgFilterCtrls` (`av1_common.h:47`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SgFilterCtrls {
    /// `enabled`
    pub enabled: bool,
    /// `start_ep[PLANE_TYPES]` — search start index.
    pub start_ep: [i8; PLANE_TYPES],
    /// `end_ep[PLANE_TYPES]` — search end index; the search stops at `end - 1`.
    pub end_ep: [i8; PLANE_TYPES],
    /// `ep_inc[PLANE_TYPES]` — search increment.
    pub ep_inc: [i8; PLANE_TYPES],
    /// `refine[PLANE_TYPES]` — 1 refines alpha/beta, 0 does not.
    pub refine: [i8; PLANE_TYPES],
    /// `use_chroma`
    pub use_chroma: bool,
}

/// C `svt_aom_set_sg_filter_ctrls` (`enc_mode_config.c:1295`). static — tier 4.
///
/// SCOPE: `svt_aom_get_sg_filter_level_default` returns 3 for
/// `enc_mode <= ENC_M3`, so this table IS live in video mode at presets 0..3.
/// `rust/CLAUDE.md` envelope guard 5 ("SGR is dead for M0..M13") is an
/// ALLINTRA-only statement. A p0..p3 video cell also needs the SGR *search*
/// vertical (`restoration.c`), which is not ported — so this table alone does
/// not make those presets reachable, and this port does not claim it does.
///
/// Level 0 writes ONLY `enabled`; the rest keep the context's prior (zeroed)
/// values.
#[must_use]
pub fn set_sg_filter_ctrls(sg_filter_lvl: u8) -> Option<SgFilterCtrls> {
    let mut c = SgFilterCtrls::default();
    match sg_filter_lvl {
        0 => c.enabled = false,
        1 => {
            c.enabled = true;
            c.use_chroma = true;
            c.start_ep = [0, 0];
            c.end_ep = [16, 16];
            c.ep_inc = [1, 1];
            c.refine = [1, 1];
        }
        2 => {
            c.enabled = true;
            c.use_chroma = true;
            c.start_ep = [0, 4];
            c.end_ep = [16, 5];
            c.ep_inc = [1, 1];
            c.refine = [1, 0];
        }
        3 => {
            c.enabled = true;
            c.use_chroma = true;
            c.start_ep = [0, 4];
            c.end_ep = [16, 5];
            // The luma increment jumps to 8 here — levels 2 and 3 differ ONLY
            // in `ep_inc[0]`.
            c.ep_inc = [8, 1];
            c.refine = [1, 0];
        }
        4 => {
            c.enabled = true;
            // ...and levels 3 and 4 differ ONLY in `use_chroma`.
            c.use_chroma = false;
            c.start_ep = [0, 4];
            c.end_ep = [16, 5];
            c.ep_inc = [8, 1];
            c.refine = [1, 0];
        }
        _ => return None,
    }
    Some(c)
}

// ---------------------------------------------------------------------------
// CDEF recon controls
// ---------------------------------------------------------------------------

/// C `CdefReconControls` (`pcs.h:593`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CdefReconControls {
    /// `zero_fs_cost_bias` — scale factor `x/64` on the zero-filter-strength
    /// cost; 0 is off, higher is safer.
    pub zero_fs_cost_bias: u16,
    /// `zero_filter_strength_lvl`
    pub zero_filter_strength_lvl: u8,
    /// `prev_cdef_dist_th` — a percent times 10.
    pub prev_cdef_dist_th: u16,
}

/// C `set_cdef_recon_controls` (`enc_mode_config.c:1200`). static — tier 4.
///
/// 0 at `<= M8` but nonzero at M9+ for video, and CDEF output feeds the
/// reference frames every inter block predicts from.
#[must_use]
pub fn set_cdef_recon_controls(cdef_recon_level: u8) -> Option<CdefReconControls> {
    match cdef_recon_level {
        0 => Some(CdefReconControls::default()),
        1 => Some(CdefReconControls {
            zero_fs_cost_bias: 61,
            zero_filter_strength_lvl: 2,
            prev_cdef_dist_th: 10,
        }),
        2 => Some(CdefReconControls {
            zero_fs_cost_bias: 61,
            zero_filter_strength_lvl: 3,
            prev_cdef_dist_th: 10,
        }),
        // C labels these "old level 4" and "old level 5"; only the bias moves.
        3 => Some(CdefReconControls {
            zero_fs_cost_bias: 60,
            zero_filter_strength_lvl: 3,
            prev_cdef_dist_th: 10,
        }),
        4 => Some(CdefReconControls {
            zero_fs_cost_bias: 58,
            zero_filter_strength_lvl: 3,
            prev_cdef_dist_th: 10,
        }),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// PD0 cost thresholds
// ---------------------------------------------------------------------------

/// C `AV1_PROB_COST_SHIFT` (`md_rate_estimation.h:29`).
pub const AV1_PROB_COST_SHIFT: u32 = 9;
/// C `RDDIV_BITS` (`rd_cost.h:34`).
pub const RDDIV_BITS: u32 = 7;

/// C `ROUND_POWER_OF_TWO(value, n)` (`definitions.h:478`).
#[must_use]
pub const fn round_power_of_two(value: i64, n: u32) -> i64 {
    (value + ((1i64 << n) >> 1)) >> n
}

/// C `RDCOST(RM, R, D)` (`rd_cost.h:36`).
///
/// `ROUND_POWER_OF_TWO(R * RM, AV1_PROB_COST_SHIFT) + (D << RDDIV_BITS)`, all
/// in `int64_t`.
#[must_use]
pub const fn rdcost(rate_mult: i64, rate: i64, dist: i64) -> i64 {
    round_power_of_two(rate * rate_mult, AV1_PROB_COST_SHIFT) + (dist << RDDIV_BITS)
}

/// C `compute_intra_pd0_th` (`enc_mode_config.c:6279`). static — tier 4.
///
/// Decides whether PD0 tests intra on an inter picture. The `fast_lambda`
/// argument is `ctx->fast_lambda_md[EB_10_BIT_MD]` when `ctx->hbd_md` is set
/// and `[EB_8_BIT_MD]` otherwise — the caller resolves that.
#[must_use]
pub fn compute_intra_pd0_th(fast_lambda: u32, super_block_size: u32) -> u64 {
    let sb_size = i64::from(super_block_size) * i64::from(super_block_size);
    let cost_th_rate: i64 = 1 << 13;
    rdcost(i64::from(fast_lambda), cost_th_rate, sb_size * 6) as u64
}

/// C `compute_subres_th` (`enc_mode_config.c:6290`). static — tier 4.
///
/// Identical body to [`compute_intra_pd0_th`] in v4.2.0; kept as its own
/// function because C does, so an upstream edit to one lands in the right
/// place.
#[must_use]
pub fn compute_subres_th(fast_lambda: u32, super_block_size: u32) -> u64 {
    let sb_size = i64::from(super_block_size) * i64::from(super_block_size);
    let cost_th_rate: i64 = 1 << 13;
    rdcost(i64::from(fast_lambda), cost_th_rate, sb_size * 6) as u64
}

// ---------------------------------------------------------------------------
// mfmv
// ---------------------------------------------------------------------------

/// The inputs C `mfmv_controls` reads.
#[derive(Debug, Clone, Copy)]
pub struct MfmvInputs {
    /// `mfmv_level`
    pub mfmv_level: u8,
    /// `ppcs->temporal_layer_index == 0`
    pub is_base: bool,
    /// `ppcs->scs->tpl`
    pub tpl: bool,
    /// `pcs->ppcs->r0_gen`
    pub r0_gen: bool,
    /// `pcs->ppcs->r0`
    pub r0: f64,
    /// `pcs->slice_type == B_SLICE`
    pub is_b_slice: bool,
    /// `ppcs->ref_list1_count_try`
    pub ref_list1_count_try: u32,
    /// L0 ref 0's `is_mfmv_used`
    pub ref_l0_is_mfmv_used: bool,
    /// L1 ref 0's `is_mfmv_used`
    pub ref_l1_is_mfmv_used: bool,
}

/// C `mfmv_controls` (`enc_mode_config.c:8853`). static — tier 4.
///
/// Returns the value C stores in `ppcs->frm_hdr.use_ref_frame_mvs` — a FRAME
/// HEADER bit, and the gate on temporal MV candidates entering the MVP stack.
///
/// NOTE the `r0_th` guard: levels 2/3/4 set a threshold that is **0 when TPL is
/// off**, and C then tests `if (r0_th)` — a floating-point truth test. With TPL
/// off those levels therefore leave `use_ref_frame_mvs` at 0 and never touch
/// the reference objects, which is why they are safe on a TPL-less encode.
#[must_use]
pub fn mfmv_controls(i: MfmvInputs) -> Option<u8> {
    let mut use_ref_frame_mvs: u8 = 0;
    let r0_th: f64 = match i.mfmv_level {
        0 => {
            return Some(0);
        }
        1 => {
            return Some(1);
        }
        2 => {
            if i.tpl {
                0.15
            } else {
                0.0
            }
        }
        3 => {
            if i.tpl {
                0.13
            } else {
                0.0
            }
        }
        4 => {
            if i.tpl {
                0.10
            } else {
                0.0
            }
        }
        _ => return None,
    };

    if r0_th != 0.0 {
        if i.r0_gen && i.is_base && i.r0 < r0_th {
            use_ref_frame_mvs = 1;
        }
        // C asserts the picture is not an I-slice here.
        // Keep mfmv if at least one of the closest reference frames used it.
        if i.ref_l0_is_mfmv_used {
            use_ref_frame_mvs = 1;
        }
        if i.is_b_slice && i.ref_list1_count_try != 0 && i.ref_l1_is_mfmv_used {
            use_ref_frame_mvs = 1;
        }
    }
    Some(use_ref_frame_mvs)
}

// ---------------------------------------------------------------------------
// TPL intra stats
// ---------------------------------------------------------------------------

/// What C `get_sb_tpl_intra_stats` returns through its three out-parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SbTplIntraStats {
    /// `*sb_ang_intra_count`
    pub ang_intra_count: i32,
    /// `*sb_max_intra` — the highest-valued intra `PredictionMode` seen.
    pub max_intra: u8,
    /// `*sb_intra_count`
    pub intra_count: i32,
}

/// The per-block TPL datum `get_sb_tpl_intra_stats` reads.
#[derive(Debug, Clone, Copy)]
pub struct TplSrcBlock {
    /// `tpl_src_stats_buffer->best_mode`
    pub best_mode: u8,
}

/// The inputs C `get_sb_tpl_intra_stats` reads off the PPCS and context.
#[derive(Debug, Clone, Copy)]
pub struct TplIntraStatsInputs {
    /// `ppcs->tpl_ctrls.enable`
    pub tpl_enable: bool,
    /// `ppcs->tpl_src_data_ready`
    pub tpl_src_data_ready: bool,
    /// `pcs->temporal_layer_index`
    pub temporal_layer_index: u8,
    /// `ppcs->hierarchical_levels`
    pub hierarchical_levels: u8,
    /// `ppcs->tpl_ctrls.disable_intra_pred_nref`
    pub disable_intra_pred_nref: bool,
    /// `ppcs->aligned_width`
    pub aligned_width: u32,
    /// `ctx->sb_origin_x`
    pub sb_origin_x: u32,
    /// `ctx->sb_origin_y`
    pub sb_origin_y: u32,
    /// `ppcs->tpl_ctrls.dispenser_search_level`
    pub dispenser_search_level: u8,
    /// `ppcs->sb_geom[ctx->sb_index].width`
    pub sb_width: u32,
    /// `ppcs->sb_geom[ctx->sb_index].height`
    pub sb_height: u32,
}

/// C `is_intra_mode` — `mode < INTRA_MODE_END` (13 in AV1's PredictionMode
/// enum, where `DC_PRED` is 0 and `PAETH_PRED` is 12).
#[must_use]
pub fn is_intra_mode(mode: u8) -> bool {
    mode <= PAETH_PRED
}

/// C `DC_PRED`.
pub const DC_PRED: u8 = 0;
/// C `PAETH_PRED` — the last intra mode.
pub const PAETH_PRED: u8 = 12;

/// C `av1_is_directional_mode` — `mode >= V_PRED && mode <= D67_PRED`.
#[must_use]
pub fn av1_is_directional_mode(mode: u8) -> bool {
    (V_PRED..=D67_PRED).contains(&mode)
}

/// C `V_PRED`.
pub const V_PRED: u8 = 1;
/// C `D67_PRED` — the last directional mode.
pub const D67_PRED: u8 = 8;

/// C `get_sb_tpl_intra_stats` (`enc_mode_config.c:6480`). static — tier 4.
///
/// Returns `None` when TPL data is unavailable (C returns 0 and leaves the out
/// parameters untouched). The still path never noticed this function because
/// TPL is off there; it IS live in video mode, and it depends on the unported
/// TPL vertical — so this is a faithful translation whose live arm is
/// documented-unreachable-for-now, NOT a verified-in-production port.
///
/// `blocks` is `ppcs->pa_me_data->tpl_src_stats_buffer` as a flat row-major
/// array of `aligned16_width` columns; the C walks it with a per-row base of
/// `((sb_origin_y >> 4) + i * step) * aligned16_width + (sb_origin_x >> 4)` and
/// a per-column stride of `step`.
#[must_use]
pub fn get_sb_tpl_intra_stats(
    i: TplIntraStatsInputs,
    blocks: &[TplSrcBlock],
) -> Option<SbTplIntraStats> {
    if !(i.tpl_enable
        && i.tpl_src_data_ready
        && (i.temporal_layer_index < i.hierarchical_levels || !i.disable_intra_pred_nref))
    {
        return None;
    }
    let aligned16_width = ((i.aligned_width + 15) >> 4) as usize;
    // The TPL stats buffer is always built for 16x16 blocks, so a coarser
    // dispenser level means a larger block size AND a larger step.
    let (tpl_blk_size, tpl_blk_step) = match i.dispenser_search_level {
        0 => (16u32, 1usize),
        1 => (32, 2),
        _ => (64, 4),
    };
    let sb_cols = (i.sb_width / tpl_blk_size).max(1) as usize;
    let sb_rows = (i.sb_height / tpl_blk_size).max(1) as usize;

    let mut ang_intra_count = 0i32;
    let mut max_intra = DC_PRED;
    let mut intra_count = 0i32;

    for r in 0..sb_rows {
        let row_base = (((i.sb_origin_y >> 4) as usize) + r * tpl_blk_step) * aligned16_width
            + ((i.sb_origin_x >> 4) as usize);
        for cidx in 0..sb_cols {
            let idx = row_base + cidx * tpl_blk_step;
            let Some(b) = blocks.get(idx) else {
                // C reads unconditionally; the port refuses rather than
                // fabricating a mode for an out-of-range index.
                return None;
            };
            if is_intra_mode(b.best_mode) {
                max_intra = max_intra.max(b.best_mode);
                intra_count += 1;
            }
            if av1_is_directional_mode(b.best_mode) {
                ang_intra_count += 1;
            }
        }
    }
    Some(SbTplIntraStats {
        ang_intra_count,
        max_intra,
        intra_count,
    })
}

// ---------------------------------------------------------------------------
// The per-preset levels feeding the two tables above
// ---------------------------------------------------------------------------

/// The CDEF-recon LEVEL ladder inside
/// `svt_aom_sig_deriv_multi_processes_default` (`enc_mode_config.c:2102`),
/// which is what feeds [`set_cdef_recon_controls`].
///
/// This is a piece of the picture-level derivation (queue item 1) that this
/// lane did NOT port in full; the ladder is translated here because the table
/// it feeds is meaningless without it. Do not read it as "item 1 is ported".
#[must_use]
pub fn cdef_recon_level_default(
    enc_mode: i8,
    fast_decode: u8,
    input_resolution: super::ResolutionRange,
) -> u8 {
    if fast_decode == 0 || input_resolution <= super::ResolutionRange::R360p {
        if enc_mode <= M8 {
            0
        } else if enc_mode <= M10 {
            1
        } else {
            2
        }
    } else if fast_decode == 1 {
        1
    } else if enc_mode <= M8 {
        // fast-decode 2
        2
    } else {
        1
    }
}
