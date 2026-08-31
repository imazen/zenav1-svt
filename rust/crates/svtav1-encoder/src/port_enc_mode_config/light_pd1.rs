//! `svt_aom_sig_deriv_enc_dec_light_pd1_default`
//! (`Codec/enc_mode_config.c:7378`) — the whole light-PD1 signal set.
//!
//! `pic_lpd1_lvl` is nonzero for non-base inter pictures at M7..M13
//! (`enc_mode_config.c:9407-9420`), presets the port supports, so a high-preset
//! video GOP takes THIS path instead of `svt_aom_sig_deriv_enc_dec_default`.
//!
//! **Tier 1** — the entry point is EXPORTED and
//! `c_parity_sig_deriv_light_pd1.rs` drives the real symbol.

use super::ResolutionRange;
use super::common::REGULAR_PD1;
use super::ctrls::{InterIntraCompCtrls, set_inter_intra_ctrls};
use super::enc_mode::*;
use super::encdec::{
    CandReductionCtrls, CandReductionInputs, CoeffShavingCtrls, Lpd1TxCtrls,
    Lpd1TxSkipDecisionCtrls, MdSubPelSearchCtrls, PfCtrls, chroma_mode, md_subpel_me_controls,
    set_cand_reduction_ctrls, set_coeff_shaving_controls, set_lpd1_tx_ctrls,
    set_lpd1_tx_skip_decision_ctrls, set_pf_controls,
};
use super::leaf::MAX_INTRA_LEVEL;
use super::pd0::{MdRateEstCtrls, set_rate_est_ctrls};

/// C `LPD1_LVL_*` (`definitions.h:775`).
pub mod lpd1_level {
    /// `LPD1_LVL_0`
    pub const L0: i8 = 0;
    /// `LPD1_LVL_1`
    pub const L1: i8 = 1;
    /// `LPD1_LVL_2`
    pub const L2: i8 = 2;
    /// `LPD1_LVL_3`
    pub const L3: i8 = 3;
    /// `LPD1_LVL_4`
    pub const L4: i8 = 4;
    /// `LPD1_LVL_5`
    pub const L5: i8 = 5;
    /// `LPD1_LVL_6`
    pub const L6: i8 = 6;
}

/// The inputs `svt_aom_sig_deriv_enc_dec_light_pd1_default` reads.
#[derive(Debug, Clone, Copy)]
pub struct LightPd1Inputs {
    /// `ctx->lpd1_ctrls.pd1_level`
    pub lpd1_level: i8,
    /// `pcs->enc_mode`
    pub enc_mode: i8,
    /// `ppcs->input_resolution`
    pub input_resolution: ResolutionRange,
    /// `pcs->slice_type == B_SLICE`
    pub is_b_slice: bool,
    /// `ppcs->picture_qp`
    pub picture_qp: u32,
    /// `svt_aom_is_ref_same_size(pcs, REF_LIST_0, 0)`
    pub is_ref_l0_avail: bool,
    /// `svt_aom_is_ref_same_size(pcs, REF_LIST_1, 0)`
    pub is_ref_l1_avail: bool,
    /// `ppcs->ref_list1_count_try`
    pub ref_list1_count_try: u32,
    /// `ppcs->me_8x8_cost_variance[sb_index]`
    pub me_8x8_cost_variance: u32,
    /// `ppcs->me_64x64_distortion[sb_index]`
    pub me_64x64_distortion: u32,
    /// L0's `sb_skip[sb_index]`
    pub l0_sb_skip: u8,
    /// L1's `sb_skip[sb_index]`
    pub l1_sb_skip: u8,
    /// L0's `sb_64x64_mvp[sb_index]`
    pub l0_sb_64x64_mvp: u8,
    /// L1's `sb_64x64_mvp[sb_index]`
    pub l1_sb_64x64_mvp: u8,
    /// `pcs->ref_skip_percentage`
    pub ref_skip_percentage: u8,
    /// `pcs->cand_reduction_level`
    pub cand_reduction_level: u8,
    /// `pcs->rdoq_level`
    pub rdoq_level: u8,
    /// `pcs->coeff_shaving_level`
    pub coeff_shaving_level: u8,
    /// `pcs->me_subpel_level`
    pub me_subpel_level: u8,
    /// `pcs->rate_est_level`
    pub rate_est_level: u8,
    /// `pcs->approx_inter_rate`
    pub approx_inter_rate: u8,
    /// `pcs->intra_level`
    pub intra_level: u8,
    /// `ppcs->ref_list0_count_try` — read by `set_cand_reduction_ctrls`.
    pub ref_list0_count_try: u32,
    /// `ppcs->use_best_me_unipred_cand_only` — same.
    pub use_best_me_unipred_cand_only: u8,
    /// `scs->static_config.rtc && ppcs->hierarchical_levels == 0` — same.
    pub use_flat_ipp: bool,
    /// `!frame_is_leaf(ppcs)` — same.
    pub is_not_last_layer: bool,
}

/// What `svt_aom_sig_deriv_enc_dec_light_pd1_default` writes, restricted to the
/// fields this lane models.
///
/// `rdoq_ctrls`, `cfl_ctrls` and `intra_ctrls` come from tables this lane has
/// not ported, so the LEVELS this function derives for them are exposed
/// instead — deriving them is the part that lives here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LightPd1Signals {
    /// `ctx->lpd1_globalmv_bypass_th` — hardcoded 0.
    pub lpd1_globalmv_bypass_th: u32,
    /// The derived `cand_reduction_level`.
    pub cand_reduction_level: u8,
    /// `ctx->cand_reduction_ctrls`
    pub cand_reduction: CandReductionCtrls,
    /// The derived `rdoq_level` (the controls table is not ported).
    pub rdoq_level: u8,
    /// `ctx->coeff_shaving_ctrls`
    pub coeff_shaving: CoeffShavingCtrls,
    /// The derived `me_subpel_level`.
    pub me_subpel_level: u8,
    /// `ctx->md_subpel_me_ctrls`
    pub md_subpel_me: MdSubPelSearchCtrls,
    /// The derived `lpd1_tx_skip_decision_level`.
    pub lpd1_tx_skip_decision_level: u8,
    /// `ctx->lpd1_tx_skip_decision_ctrls`
    pub lpd1_tx_skip_decision: Lpd1TxSkipDecisionCtrls,
    /// The derived `lpd1_tx_level`.
    pub lpd1_tx_level: u8,
    /// `ctx->lpd1_tx_ctrls`
    pub lpd1_tx: Lpd1TxCtrls,
    /// `ctx->lpd1_blk_skip_luma_rd_pct`
    pub lpd1_blk_skip_luma_rd_pct: u8,
    /// `ctx->lpd1_chroma_skip_energy_th` — hardcoded 0.
    pub lpd1_chroma_skip_energy_th: u32,
    /// The derived `rate_est_level`.
    pub rate_est_level: u8,
    /// `ctx->rate_est_ctrls`, AFTER the two post-hoc overrides at the end of
    /// the function.
    pub rate_est: MdRateEstCtrls,
    /// `ctx->approx_inter_rate`
    pub approx_inter_rate: u8,
    /// `ctx->pf_ctrls`
    pub pf: PfCtrls,
    /// The derived `intra_level` (the controls table is not ported).
    pub intra_level: u8,
    /// `ctx->shut_fast_rate` — hardcoded FALSE here, unlike the PD0 path.
    pub shut_fast_rate: bool,
    /// `ctx->uv_ctrls.enabled`
    pub uv_enabled: u8,
    /// `ctx->uv_ctrls.uv_mode`
    pub uv_mode: u8,
    /// `ctx->md_disallow_nsq_search`
    pub md_disallow_nsq_search: u8,
    /// `ctx->new_nearest_injection`
    pub new_nearest_injection: u8,
    /// `ctx->blk_skip_decision`
    pub blk_skip_decision: bool,
    /// `ctx->subres_ctrls.odd_to_even_deviation_th` — forced to 0 at the end.
    pub subres_odd_to_even_deviation_th: u8,
    /// `ctx->inter_intra_comp_ctrls`
    pub inter_intra: InterIntraCompCtrls,
}

/// C `svt_aom_sig_deriv_enc_dec_light_pd1_default` (`enc_mode_config.c:7378`).
/// EXPORTED.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn sig_deriv_enc_dec_light_pd1_default(i: LightPd1Inputs) -> Option<LightPd1Signals> {
    let lpd1 = i.lpd1_level;
    let qp = i.picture_qp;

    // Reference info. NOTE the sentinels: when L0 is unavailable both
    // distortions stay at (uint32_t)~0 and all four reference flags stay 0.
    // And when L0 IS available, l1_was_skip / l1_was_64x64_mvp are set to 1
    // FIRST and only overwritten from L1 on a B-slice with a usable L1 — so a
    // P-slice behaves as though L1 agreed.
    let mut me_8x8_cost_variance = u32::MAX;
    let mut me_64x64_distortion = u32::MAX;
    let mut l0_was_skip = 0u8;
    let mut l1_was_skip = 0u8;
    let mut l0_was_64x64_mvp = 0u8;
    let mut l1_was_64x64_mvp = 0u8;
    if i.is_ref_l0_avail {
        me_8x8_cost_variance = i.me_8x8_cost_variance;
        me_64x64_distortion = i.me_64x64_distortion;
        l0_was_skip = i.l0_sb_skip;
        l1_was_skip = 1;
        l0_was_64x64_mvp = i.l0_sb_64x64_mvp;
        l1_was_64x64_mvp = 1;
        if i.is_b_slice && i.is_ref_l1_avail && i.ref_list1_count_try != 0 {
            l1_was_skip = i.l1_sb_skip;
            l1_was_64x64_mvp = i.l1_sb_64x64_mvp;
        }
    }
    let ref_skip_perc = i.ref_skip_percentage;

    // Candidate reduction level.
    let mut cand_reduction_level = 0u8;
    if i.cand_reduction_level != 0 {
        cand_reduction_level = if lpd1 <= lpd1_level::L0 {
            2
        } else if lpd1 <= lpd1_level::L2 {
            3
        } else if lpd1 <= lpd1_level::L3 {
            4
        } else {
            5
        };
        if cand_reduction_level != 0 {
            cand_reduction_level = cand_reduction_level.max(i.cand_reduction_level);
        }
    }
    let cand_reduction = set_cand_reduction_ctrls(CandReductionInputs {
        level: cand_reduction_level,
        is_lpd1: lpd1 > REGULAR_PD1,
        is_not_last_layer: i.is_not_last_layer,
        use_flat_ipp: i.use_flat_ipp,
        picture_qp: qp,
        me_8x8_cost_variance,
        me_64x64_distortion,
        l0_was_skip,
        l1_was_skip,
        ref_skip_perc,
        ref_list0_count_try: i.ref_list0_count_try,
        ref_list1_count_try: i.ref_list1_count_try,
        use_best_me_unipred_cand_only: i.use_best_me_unipred_cand_only,
    })?;

    // RDOQ level.
    let mut rdoq_level = 0u8;
    if i.rdoq_level != 0 {
        rdoq_level = if i.enc_mode <= M8 {
            if lpd1 <= lpd1_level::L4 { 1 } else { 0 }
        } else if lpd1 <= lpd1_level::L0 {
            4
        } else if lpd1 <= lpd1_level::L4 {
            5
        } else {
            0
        };
        if rdoq_level != 0 {
            rdoq_level = rdoq_level.max(i.rdoq_level);
        }
    }

    let coeff_shaving = set_coeff_shaving_controls(i.coeff_shaving_level)?;

    // Sub-pel level.
    let mut me_subpel_level = 0u8;
    if i.me_subpel_level != 0 {
        if lpd1 <= lpd1_level::L0 {
            me_subpel_level = if i.input_resolution <= ResolutionRange::R480p {
                7
            } else if i.input_resolution <= ResolutionRange::R1080p {
                8
            } else {
                10
            };
        } else {
            me_subpel_level = if i.input_resolution <= ResolutionRange::R480p {
                8
            } else if i.input_resolution <= ResolutionRange::R1080p {
                9
            } else {
                10
            };
            // A very static SB with agreeing references turns sub-pel OFF.
            if ((l0_was_skip != 0 && l1_was_skip != 0 && ref_skip_perc > 50)
                || (l0_was_64x64_mvp != 0 && l1_was_64x64_mvp != 0))
                && me_8x8_cost_variance < (200 * qp)
                && me_64x64_distortion < (200 * qp)
            {
                me_subpel_level = 0;
            }
        }
        if me_subpel_level != 0 {
            me_subpel_level = me_subpel_level.max(i.me_subpel_level);
        }
    }
    let md_subpel_me = md_subpel_me_controls(me_subpel_level)?;

    // The two LPD1 transform levels share ONE predicate, spelled out twice in
    // the C with different constants only in the level they pick.
    let static_sb = ((l0_was_skip != 0 && l1_was_skip != 0 && ref_skip_perc > 35)
        && me_8x8_cost_variance < (800 * qp)
        && me_64x64_distortion < (800 * qp))
        || (me_8x8_cost_variance < (100 * qp) && me_64x64_distortion < (100 * qp));

    let lpd1_tx_skip_decision_level = if lpd1 <= lpd1_level::L2 {
        2
    } else if static_sb {
        4
    } else {
        3
    };
    let lpd1_tx_skip_decision = set_lpd1_tx_skip_decision_ctrls(lpd1_tx_skip_decision_level)?;

    let lpd1_tx_level = if lpd1 <= lpd1_level::L2 {
        3
    } else if static_sb {
        6
    } else {
        4
    };
    let lpd1_tx = set_lpd1_tx_ctrls(lpd1_tx_level)?;

    let lpd1_blk_skip_luma_rd_pct = if lpd1 <= lpd1_level::L2 { 0 } else { 90 };

    // Rate estimation level.
    let mut rate_est_level = 0u8;
    if i.rate_est_level != 0 {
        rate_est_level = if lpd1 <= lpd1_level::L0 { 4 } else { 0 };
        if rate_est_level != 0 {
            rate_est_level = rate_est_level.max(i.rate_est_level);
        }
    }
    let mut rate_est = set_rate_est_ctrls(rate_est_level)?;
    // The two post-hoc overrides at the end of the function.
    rate_est.update_skip_ctx_dc_sign_ctx = false;
    rate_est.update_skip_coeff_ctx = false;

    // NOTE this is a MAX against 1, not a copy: the LPD1 default (1) wins
    // unless the picture asked for something MORE aggressive.
    let approx_inter_rate = i.approx_inter_rate.max(1);

    let pf = set_pf_controls(1)?;

    let mut intra_level = 0u8;
    if i.intra_level != 0 {
        intra_level = if lpd1 <= lpd1_level::L2 {
            6
        } else {
            (MAX_INTRA_LEVEL - 1) as u8
        };
        if intra_level != 0 {
            intra_level = intra_level.max(i.intra_level);
        }
    }

    Some(LightPd1Signals {
        lpd1_globalmv_bypass_th: 0,
        cand_reduction_level,
        cand_reduction,
        rdoq_level,
        coeff_shaving,
        me_subpel_level,
        md_subpel_me,
        lpd1_tx_skip_decision_level,
        lpd1_tx_skip_decision,
        lpd1_tx_level,
        lpd1_tx,
        lpd1_blk_skip_luma_rd_pct,
        lpd1_chroma_skip_energy_th: 0,
        rate_est_level,
        rate_est,
        approx_inter_rate,
        pf,
        intra_level,
        // Unlike the PD0 path, light-PD1 sets shut_fast_rate FALSE.
        shut_fast_rate: false,
        uv_enabled: 1,
        uv_mode: chroma_mode::FAST,
        md_disallow_nsq_search: 1,
        new_nearest_injection: 1,
        blk_skip_decision: true,
        subres_odd_to_even_deviation_th: 0,
        inter_intra: set_inter_intra_ctrls(0)?,
    })
}
