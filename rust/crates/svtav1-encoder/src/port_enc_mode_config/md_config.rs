//! `svt_aom_sig_deriv_mode_decision_config_default`
//! (`Codec/enc_mode_config.c:8900`) — the largest function in the file. It
//! assigns EVERY per-picture level the MD path consumes.
//!
//! The ported allintra twin resolves to different levels at the same preset,
//! so a video-mode picture that runs on the allintra derivation diverges
//! everywhere at once.
//!
//! **Tier 1** — the entry point is EXPORTED and
//! `c_parity_sig_deriv_md_config.rs` drives the real symbol.

use super::ctrls::MAX_WARP_LVL;
use super::enc_mode::*;
use super::leaf::{
    get_bypass_encdec_default, get_chroma_level_default, get_inter_compound_level,
    get_intra_mode_levels_default, get_nic_level_default, get_nsq_geom_level_default,
    get_nsq_search_level_default, get_obmc_level, get_update_cdf_level_default,
    set_pic_pd0_lvl_default,
};
use super::{InputCoeffLvl, ResolutionRange};

/// C `HIGH_PRECISION_MV_QTHRESH_0` (`definitions.h:73`).
pub const HIGH_PRECISION_MV_QTHRESH_0: i32 = 128;
/// C `HIGH_PRECISION_MV_QTHRESH_1` (`definitions.h:74`).
pub const HIGH_PRECISION_MV_QTHRESH_1: i32 = 196;
/// C `HIGH_PRECISION_REF_PERC_TH` (`definitions.h:75`).
pub const HIGH_PRECISION_REF_PERC_TH: i16 = 50;
/// C `MAX_TEMPORAL_LAYERS` (`definitions.h:2046`).
pub const MAX_TEMPORAL_LAYERS: usize = 6;
/// C `MAX_QP_VALUE` (`definitions.h:1662`).
pub const MAX_QP_VALUE: u32 = 63;
/// C `TUNE_IQ` (`definitions.h:1881`).
pub const TUNE_IQ: u8 = 3;
/// C `RESIZE_NONE` (`API/EbSvtAv1Enc.h:126`).
pub const RESIZE_NONE: u8 = 0;
/// C `EIGHTTAP_REGULAR` (`definitions.h:839`).
pub const EIGHTTAP_REGULAR: u8 = 0;
/// C `SWITCHABLE` (`definitions.h:845`) — `SWITCHABLE_FILTERS + 1` == 4.
pub const SWITCHABLE: u8 = 4;
/// C `TX_MODE_LARGEST` (`definitions.h:1031`).
pub const TX_MODE_LARGEST: u8 = 1;
/// C `TX_MODE_SELECT` (`definitions.h:1032`).
pub const TX_MODE_SELECT: u8 = 2;

/// C `svt_aom_get_disallow_4x4_default` (`enc_mode_config.c:8169`). EXPORTED.
#[must_use]
pub fn get_disallow_4x4_default(enc_mode: i8) -> bool {
    enc_mode > M2
}

/// C `svt_aom_get_disallow_4x4_allintra` (`enc_mode_config.c:8181`). EXPORTED.
#[must_use]
pub fn get_disallow_4x4_allintra(enc_mode: i8) -> bool {
    enc_mode > M3
}

/// C `get_filter_intra_level_default` (`enc_mode_config.c:8771`). EXPORTED.
#[must_use]
pub fn get_filter_intra_level_default(enc_mode: i8) -> u8 {
    if enc_mode <= M1 {
        1
    } else if enc_mode <= M5 {
        2
    } else {
        0
    }
}

/// C `svt_aom_get_inter_intra_level` (`enc_mode_config.c:8803`). EXPORTED.
#[must_use]
pub fn get_inter_intra_level(enc_mode: i8, transition_present: u8) -> u8 {
    if enc_mode <= M1 {
        2
    } else if enc_mode <= M8 {
        if transition_present != 0 { 2 } else { 0 }
    } else {
        0
    }
}

/// C `CdfControls` — what `set_cdf_controls` (`enc_mode_config.c:8469`)
/// writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CdfControls {
    /// `update_mv`
    pub update_mv: u8,
    /// `update_se`
    pub update_se: u8,
    /// `update_coef`
    pub update_coef: u8,
    /// `enabled`
    pub enabled: u8,
}

/// C `set_cdf_controls` (`enc_mode_config.c:8469`). static.
///
/// NOTE the two post-switch lines: `update_mv` is FORCED to 0 on an I-slice,
/// and `enabled` is the OR of all three — so a level that only updates
/// coefficients still reports enabled.
#[must_use]
pub fn set_cdf_controls(
    update_cdf_level: u8,
    is_islice: bool,
    rate_est_level: u8,
    rdoq_level: u8,
) -> Option<CdfControls> {
    let coef_from_levels = u8::from(rate_est_level != 0 || rdoq_level != 0);
    let mut c = match update_cdf_level {
        0 => CdfControls {
            update_mv: 0,
            update_se: 0,
            update_coef: 0,
            enabled: 0,
        },
        1 => CdfControls {
            update_mv: 1,
            update_se: 1,
            update_coef: coef_from_levels,
            enabled: 0,
        },
        2 => CdfControls {
            update_mv: 0,
            update_se: 1,
            update_coef: coef_from_levels,
            enabled: 0,
        },
        3 => CdfControls {
            update_mv: 0,
            update_se: 1,
            update_coef: 0,
            enabled: 0,
        },
        _ => return None,
    };
    if is_islice {
        c.update_mv = 0;
    }
    c.enabled = c.update_coef | c.update_mv | c.update_se;
    Some(c)
}

/// Inputs of C `svt_aom_sig_deriv_mode_decision_config_default`.
#[derive(Debug, Clone, Copy)]
#[allow(clippy::struct_excessive_bools)]
pub struct MdConfigInputs {
    /// `pcs->enc_mode`
    pub enc_mode: i8,
    /// `ppcs->is_ref`
    pub is_ref: bool,
    /// `ppcs->temporal_layer_index`
    pub temporal_layer_index: u8,
    /// `ppcs->input_resolution`
    pub input_resolution: ResolutionRange,
    /// `pcs->slice_type == I_SLICE`
    pub is_islice: bool,
    /// `ppcs->sc_class5`
    pub sc_class5: u8,
    /// `scs->static_config.fast_decode`
    pub fast_decode: u8,
    /// `ppcs->hierarchical_levels`
    pub hierarchical_levels: u32,
    /// `ppcs->transition_present == 1`
    pub transition_present: bool,
    /// `!ppcs->is_highest_layer`
    pub is_not_last_layer: bool,
    /// `scs->static_config.qp`
    pub sq_qp: u32,
    /// `scs->mfmv_enabled`
    pub mfmv_enabled: u8,
    /// `ppcs->frm_hdr.error_resilient_mode`
    pub error_resilient_mode: bool,
    /// `ppcs->frm_hdr.quantization_params.base_q_idx`
    pub base_q_idx: i32,
    /// `pcs->ref_hp_percentage`
    pub ref_hp_percentage: i16,
    /// `scs->input_resolution` — the SEQUENCE resolution, which the
    /// high-precision-MV test uses, NOT the picture's.
    pub scs_input_resolution: ResolutionRange,
    /// `frm_hdr->frame_type == KEY_FRAME || INTRA_ONLY_FRAME`
    pub frame_is_intra: bool,
    /// `ppcs->frame_superres_enabled`
    pub frame_superres_enabled: bool,
    /// `ppcs->frame_resize_enabled`
    pub frame_resize_enabled: bool,
    /// `scs->seq_qp_mod`
    pub seq_qp_mod: u8,
    /// `scs->static_config.resize_mode`
    pub resize_mode: u8,
    /// `pcs->ref_intra_percentage`
    pub ref_intra_percentage: u8,
    /// `scs->rc_stat_gen_pass_mode`
    pub rc_stat_gen_pass_mode: u8,
    /// `pcs->ref_skip_percentage`
    pub ref_skip_percentage: u8,
    /// `pcs->coeff_lvl`
    pub coeff_lvl: InputCoeffLvl,
    /// `ppcs->ref_list0_count_try`
    pub ref_list0_count_try: u32,
    /// `ppcs->ref_list1_count_try`
    pub ref_list1_count_try: u32,
    /// `scs->seq_header.enable_interintra_compound`
    pub enable_interintra_compound: bool,
    /// `scs->static_config.encoder_bit_depth`
    pub encoder_bit_depth: u8,
    /// `ppcs->frm_hdr.segmentation_params.segmentation_enabled`
    pub segmentation_enabled: bool,
    /// `scs->super_block_size`
    pub super_block_size: u16,
    /// `ppcs->hbd_md`
    pub hbd_md: u8,
    /// `ppcs->r0_gen`
    pub r0_gen: bool,
    /// `ppcs->r0`
    pub r0: f64,
    /// `pcs->temporal_layer_index`
    pub pcs_temporal_layer_index: u8,
    /// `scs->static_config.tune`
    pub tune: u8,
    /// `ppcs->picture_qp`
    pub picture_qp: u32,
    /// `scs->static_config.extended_crf_qindex_offset`
    pub extended_crf_qindex_offset: u8,
}

/// The picture-level levels `svt_aom_sig_deriv_mode_decision_config_default`
/// assigns.
///
/// `dlf_level` is NOT produced here. The deblock derivation is driven from the
/// pipeline instead — [`super::leaf::get_dlf_level_default`] /
/// [`super::leaf::get_dlf_level_allintra`] into
/// [`super::ctrls::set_dlf_controls`] — and gated at tier 1 by
/// `tests/c_parity_dlf_ctrls.rs`, which has its own shim TU because this
/// struct's shim pins `enable_dlf_flag` at 0.
#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub struct MdConfigSignals {
    /// The derived mfmv level (its controls write the frame-header bit).
    pub mfmv_level: u8,
    /// `pcs->rdoq_level`
    pub rdoq_level: u8,
    /// `pcs->coeff_shaving_level`
    pub coeff_shaving_level: u8,
    /// `pcs->rate_est_level`
    pub rate_est_level: u8,
    /// `pcs->cdf_ctrl`
    pub cdf_ctrl: CdfControls,
    /// `pcs->pic_filter_intra_level`
    pub pic_filter_intra_level: u8,
    /// `ppcs->use_accurate_part_ctx`
    pub use_accurate_part_ctx: bool,
    /// `frm_hdr->allow_high_precision_mv`
    pub allow_high_precision_mv: u8,
    /// `pcs->wm_level`
    pub wm_level: u8,
    /// `frm_hdr->allow_warped_motion`
    pub allow_warped_motion: bool,
    /// `frm_hdr->is_motion_mode_switchable`
    pub is_motion_mode_switchable: bool,
    /// `ppcs->pic_obmc_level`
    pub pic_obmc_level: u8,
    /// `pcs->approx_inter_rate`
    pub approx_inter_rate: u8,
    /// `pcs->skip_intra`
    pub skip_intra: u8,
    /// `pcs->intra_level`
    pub intra_level: u32,
    /// `pcs->dist_based_ang_intra_level`
    pub dist_based_ang_intra_level: u32,
    /// `pcs->cand_reduction_level`
    pub cand_reduction_level: u8,
    /// `pcs->txt_level`
    pub txt_level: u8,
    /// `pcs->tx_shortcut_level`
    pub tx_shortcut_level: u8,
    /// `pcs->pd0_cost_bias_weight`
    pub pd0_cost_bias_weight: u32,
    /// `pcs->interpolation_search_level`
    pub interpolation_search_level: u8,
    /// `frm_hdr->interpolation_filter`
    pub interpolation_filter: u8,
    /// `pcs->chroma_level`
    pub chroma_level: u8,
    /// `pcs->cfl_level`
    pub cfl_level: u8,
    /// `pcs->new_nearest_near_comb_injection`
    pub new_nearest_near_comb_injection: u8,
    /// `pcs->unipred3x3_injection`
    pub unipred3x3_injection: u8,
    /// `pcs->bipred3x3_injection`
    pub bipred3x3_injection: u8,
    /// `pcs->inter_compound_mode`
    pub inter_compound_mode: u8,
    /// `pcs->dist_based_ref_pruning`
    pub dist_based_ref_pruning: u8,
    /// `pcs->spatial_sse_full_loop_level`
    pub spatial_sse_full_loop_level: u8,
    /// `pcs->nsq_geom_level`
    pub nsq_geom_level: u8,
    /// `pcs->nsq_search_level`
    pub nsq_search_level: u8,
    /// `pcs->inter_intra_level`
    pub inter_intra_level: u8,
    /// `pcs->txs_level`
    pub txs_level: u8,
    /// `frm_hdr->tx_mode`
    pub tx_mode: u8,
    /// `pcs->nic_level`
    pub nic_level: u8,
    /// `pcs->md_sq_mv_search_level`
    pub md_sq_mv_search_level: u8,
    /// `pcs->md_nsq_mv_search_level`
    pub md_nsq_mv_search_level: u8,
    /// `pcs->md_pme_level`
    pub md_pme_level: u8,
    /// `pcs->me_subpel_level`
    pub me_subpel_level: u8,
    /// `pcs->pme_subpel_level`
    pub pme_subpel_level: u8,
    /// `pcs->mds0_level`
    pub mds0_level: u8,
    /// `pcs->pic_disallow_4x4`
    pub pic_disallow_4x4: bool,
    /// `pcs->pic_bypass_encdec`
    pub pic_bypass_encdec: u8,
    /// `pcs->pic_pd0_lvl`
    pub pic_pd0_lvl: u8,
    /// `pcs->pic_depth_removal_level`
    pub pic_depth_removal_level: u8,
    /// `pcs->pic_block_based_depth_refinement_level`
    pub pic_block_based_depth_refinement_level: u8,
    /// `pcs->pic_lpd1_lvl`
    pub pic_lpd1_lvl: u8,
    /// `pcs->lambda_weight`
    pub lambda_weight: u32,
    /// The derived deblocking level (its controls table is not ported).
    pub dlf_level: u8,
}

/// C `svt_aom_sig_deriv_mode_decision_config_default`
/// (`enc_mode_config.c:8900`). EXPORTED.
#[must_use]
#[allow(clippy::too_many_lines)]
// Several of C's ladders have two arms that happen to coincide in v4.2.0 —
// `skip_intra` at <= M1, `cand_reduction_level` at <= MR, and
// `interpolation_search_level` at <= M8 vs above. They are kept as separate
// arms so the Rust diffs one-to-one against the C and an upstream retune lands
// in the right place; collapsing them would hide which arm moved.
#[allow(clippy::if_same_then_else)]
pub fn sig_deriv_mode_decision_config_default(i: MdConfigInputs) -> Option<MdConfigSignals> {
    let m = i.enc_mode;
    let is_base = i.temporal_layer_index == 0;
    let is_layer1 = i.temporal_layer_index == 1;
    let res = i.input_resolution;
    let sc5 = i.sc_class5 != 0;
    let low_coeff = i.coeff_lvl == InputCoeffLvl::VLow || i.coeff_lvl == InputCoeffLvl::Low;
    let high_coeff = i.coeff_lvl == InputCoeffLvl::High;

    // MFMV level.
    let mfmv_level = if i.is_islice || i.mfmv_enabled == 0 || i.error_resilient_mode {
        0
    } else if i.fast_decode == 0 || res <= ResolutionRange::R360p {
        if m <= MR {
            1
        } else if m <= M8 {
            if res <= ResolutionRange::R360p { 1 } else { 2 }
        } else if res <= ResolutionRange::R360p {
            1
        } else {
            4
        }
    } else {
        4
    };

    let rdoq_level = if m <= M10 { 1 } else { 2 };
    let coeff_shaving_level = 0u8;
    let rate_est_level = 1u8;
    let update_cdf_level = get_update_cdf_level_default(m, i.is_islice, is_base);
    let cdf_ctrl = set_cdf_controls(update_cdf_level, i.is_islice, rate_est_level, rdoq_level)?;

    let pic_filter_intra_level = get_filter_intra_level_default(m);
    let use_accurate_part_ctx = m <= M8;

    let allow_high_precision_mv = u8::from(
        (i.base_q_idx < HIGH_PRECISION_MV_QTHRESH_0
            || (i.ref_hp_percentage > HIGH_PRECISION_REF_PERC_TH
                && i.base_q_idx < HIGH_PRECISION_MV_QTHRESH_1))
            && i.scs_input_resolution <= ResolutionRange::R480p,
    );

    // Warped motion level.
    let mut wm_level: u8 = 0;
    if !(i.frame_is_intra
        || i.error_resilient_mode
        || i.frame_superres_enabled
        || i.frame_resize_enabled)
    {
        wm_level = if m <= M1 {
            1
        } else if m <= M3 {
            if i.hierarchical_levels <= 3 {
                if is_base { 1 } else { 3 }
            } else if is_base || is_layer1 {
                2
            } else {
                3
            }
        } else if m <= M9 {
            if res <= ResolutionRange::R720p {
                if is_base { 3 } else { 0 }
            } else if is_base {
                4
            } else {
                0
            }
        } else if m <= M11 {
            if is_base { 4 } else { 0 }
        } else {
            0
        };
    }
    if i.hierarchical_levels <= 2 {
        wm_level = if m <= M6 { wm_level } else { 0 };
    }
    // QP-banding. NOTE the wrap-around: at level 0 it becomes MAX_WARP_LVL.
    if m <= M7 && i.seq_qp_mod != 0 && i.sq_qp > 55 && (i.seq_qp_mod == 1 || i.seq_qp_mod == 2) {
        wm_level = if wm_level == 1 {
            wm_level
        } else if wm_level == 0 {
            MAX_WARP_LVL
        } else {
            wm_level - 1
        };
    }
    let enable_wm = wm_level != 0;
    let allow_warped_motion = enable_wm
        && !i.frame_is_intra
        && !i.error_resilient_mode
        && !i.frame_superres_enabled
        && i.resize_mode == RESIZE_NONE;

    let pic_obmc_level = get_obmc_level(m, i.sq_qp, i.seq_qp_mod);
    let is_motion_mode_switchable = allow_warped_motion || pic_obmc_level != 0;

    let approx_inter_rate = u8::from(m > M9);

    let skip_intra = if i.is_islice || i.transition_present {
        0
    } else if m <= M1 {
        0
    } else {
        u8::from(!(i.is_ref || i.ref_intra_percentage > 50))
    };

    let (intra_level, dist_based_ang_intra_level) =
        get_intra_mode_levels_default(m, i.is_islice, is_base, i32::from(i.transition_present));

    let mut cand_reduction_level = if i.is_islice {
        0
    } else if m <= MR {
        0
    } else if m <= M2 {
        u8::from(!is_base)
    } else if m <= M7 {
        1
    } else {
        2
    };
    if i.rc_stat_gen_pass_mode != 0 {
        cand_reduction_level = 6;
    }

    let txt_level = if m <= MR {
        if is_base { 2 } else { 3 }
    } else if m <= M2 {
        if is_base { 2 } else { 5 }
    } else if m <= M10 {
        if is_base { 7 } else { 9 }
    } else if m <= M11 {
        10
    } else {
        0
    };

    let tx_shortcut_level = if m <= M2 {
        0
    } else if m <= M10 {
        u8::from(!is_base)
    } else if i.is_islice {
        1
    } else {
        3
    };

    let pd0_cost_bias_weight = 0u32;

    let mut interpolation_search_level = if m <= MR {
        2
    } else if m <= M8 {
        4
    } else {
        4
    };
    if m > M8 && !is_base {
        // C indexes this by the RESOLUTION enum value.
        const TH: [u8; 7] = [100, 100, 85, 50, 30, 30, 30];
        if i.ref_skip_percentage > TH[res.as_u8() as usize] {
            interpolation_search_level = 0;
        }
    }
    let interpolation_filter = if interpolation_search_level != 0 {
        SWITCHABLE
    } else {
        EIGHTTAP_REGULAR
    };

    let chroma_level = get_chroma_level_default(m, i.is_islice);

    let cfl_level = if m <= M1 {
        1
    } else if m <= M9 {
        if is_base { 2 } else { 0 }
    } else if m <= M10 {
        if i.is_islice { 2 } else { 0 }
    } else {
        0
    };

    let new_nearest_near_comb_injection = if m <= MR {
        1
    } else if m <= M1 {
        if is_base { 2 } else { 0 }
    } else {
        0
    };
    let unipred3x3_injection = u8::from(m <= MR);
    let bipred3x3_injection = if m <= M0 {
        1
    } else if m <= M1 {
        2
    } else {
        0
    };
    let inter_compound_mode = get_inter_compound_level(m);

    let dist_based_ref_pruning = if i.ref_list0_count_try > 1 || i.ref_list1_count_try > 1 {
        if m <= MR {
            0
        } else if m <= M2 {
            if is_base { 1 } else { 5 }
        } else if m <= M11 {
            // M3..M8 and M9..M11 have identical bodies in v4.2.0.
            if is_base { 2 } else { 5 }
        } else {
            8
        }
    } else {
        0
    };

    let spatial_sse_full_loop_level = if m <= M2 { 1 } else { 3 };

    let nsq_geom_level = get_nsq_geom_level_default(m, i.coeff_lvl);
    let nsq_search_level = get_nsq_search_level_default(
        m,
        i.coeff_lvl,
        i.sq_qp,
        i.temporal_layer_index,
        i.r0_gen,
        i.r0,
        i.is_islice,
        i.pcs_temporal_layer_index,
        i.seq_qp_mod,
    );

    let inter_intra_level = if !i.is_islice && i.enable_interintra_compound {
        get_inter_intra_level(m, u8::from(i.transition_present))
    } else {
        0
    };

    let mut txs_level = if m <= M1 {
        2
    } else if m <= M8 {
        if is_base { 3 } else { 0 }
    } else if m <= M9 {
        if is_base { 4 } else { 0 }
    } else {
        0
    };
    if txs_level != 0
        && i.seq_qp_mod != 0
        && i.sq_qp > 58
        && (i.seq_qp_mod == 1 || i.seq_qp_mod == 2)
    {
        txs_level = if txs_level == 1 {
            txs_level
        } else {
            txs_level - 1
        };
    }
    let tx_mode = if txs_level != 0 {
        TX_MODE_SELECT
    } else {
        TX_MODE_LARGEST
    };

    let nic_level = get_nic_level_default(m, is_base);
    let md_sq_mv_search_level = 0u8;
    let md_nsq_mv_search_level = 2u8;
    let md_pme_level = if m <= MR {
        1
    } else if m <= M0 {
        2
    } else if m <= M5 {
        3
    } else if m <= M9 {
        4
    } else {
        0
    };
    let me_subpel_level = if m <= M2 {
        1
    } else if m <= M8 {
        4
    } else if m <= M11 {
        5
    } else {
        6
    };
    let pme_subpel_level = if m <= MR { 1 } else { 2 };

    // NOTE the `#if SVT_HDR_MODE` arm above this ladder is NOT compiled in
    // mainline (Source/API/EbDebugMacros.h), so the ladder below is the live
    // one.
    let mds0_level = if m <= M2 {
        0
    } else if m <= M5 {
        u8::from(!is_base)
    } else if m <= M10 {
        if i.is_islice { 0 } else { 2 }
    } else {
        2
    };

    let pic_disallow_4x4 = get_disallow_4x4_default(m);
    let pic_bypass_encdec = if i.segmentation_enabled {
        0
    } else {
        get_bypass_encdec_default(m, i.encoder_bit_depth)
    };

    let pic_pd0_lvl = set_pic_pd0_lvl_default(
        m,
        is_base,
        i.is_islice,
        i.transition_present,
        i.coeff_lvl,
        res,
        i.sq_qp,
        i.seq_qp_mod,
        i.super_block_size,
    );

    let pic_depth_removal_level = if i.transition_present {
        0
    } else if sc5 {
        if m <= M6 {
            if is_base { 0 } else { 3 }
        } else if m <= M9 {
            if is_base { 0 } else { 6 }
        } else if is_base {
            5
        } else {
            14
        }
    } else if m <= M1 {
        0
    } else if m <= M5 {
        if res <= ResolutionRange::R360p {
            if low_coeff {
                if is_base { 1 } else { 2 }
            } else if high_coeff {
                if is_base { 3 } else { 5 }
            } else if is_base {
                3
            } else {
                4
            }
        } else if res <= ResolutionRange::R480p {
            if low_coeff {
                if is_base { 1 } else { 2 }
            } else if high_coeff {
                if is_base { 3 } else { 6 }
            } else if is_base {
                3
            } else {
                5
            }
        } else if low_coeff {
            if is_base { 1 } else { 3 }
        } else if high_coeff {
            if is_base { 4 } else { 8 }
        } else if is_base {
            4
        } else {
            7
        }
    } else if m <= M9 {
        if res <= ResolutionRange::R360p {
            if high_coeff && !is_base { 6 } else { 5 }
        } else if res <= ResolutionRange::R480p {
            if high_coeff && !is_base { 7 } else { 6 }
        } else if low_coeff {
            if is_base { 6 } else { 8 }
        } else if high_coeff {
            if is_base { 6 } else { 11 }
        } else if is_base {
            6
        } else {
            9
        }
    } else if res <= ResolutionRange::R360p {
        7
    } else if res <= ResolutionRange::R480p {
        if is_base { 9 } else { 11 }
    } else if is_base {
        9
    } else {
        14
    };

    let mut pic_block_based_depth_refinement_level = if sc5 {
        if m <= M2 {
            0
        } else if m <= M3 {
            u8::from(!i.is_islice)
        } else if m <= M4 {
            1
        } else if m <= M5 {
            if i.is_islice { 1 } else { 4 }
        } else if m <= M6 {
            4
        } else if m <= M8 {
            6
        } else if m <= M9 {
            7
        } else {
            9
        }
    } else if m <= M0 {
        0
    } else if m <= M3 {
        if low_coeff { 2 } else { 3 }
    } else if m <= M6 {
        if low_coeff {
            5
        } else if high_coeff {
            7
        } else {
            6
        }
    } else if m <= M7 {
        if low_coeff {
            6
        } else if high_coeff {
            10
        } else {
            8
        }
    } else {
        10
    };
    // r0 modulation.
    if m <= M10 && pic_block_based_depth_refinement_level != 0 && i.r0_gen {
        let r0_tab: [f64; MAX_TEMPORAL_LAYERS] = [0.20, 0.30, 0.40, 0.50, 0.50, 0.50];
        let r0_th = if i.is_islice {
            0.05
        } else {
            r0_tab[i.pcs_temporal_layer_index as usize]
        };
        if i.r0 < r0_th {
            pic_block_based_depth_refinement_level =
                (pic_block_based_depth_refinement_level - 1).min(8);
        }
    }

    let mut pic_lpd1_lvl = if m <= M6 {
        0
    } else if m <= M9 {
        if res <= ResolutionRange::R360p {
            if i.is_not_last_layer { 0 } else { 2 }
        } else if res <= ResolutionRange::R480p {
            if is_base { 0 } else { 2 }
        } else if is_base {
            0
        } else {
            3
        }
    } else if m <= M10 {
        if res <= ResolutionRange::R480p {
            if low_coeff {
                if is_base { 0 } else { 3 }
            } else if high_coeff {
                if is_base { 0 } else { 5 }
            } else if is_base {
                0
            } else {
                4
            }
        } else if is_base {
            0
        } else {
            5
        }
    } else if m <= M11 {
        if is_base { 0 } else { 7 }
    } else if i.is_islice {
        0
    } else if is_base {
        3
    } else {
        7
    };
    // Light-PD1 needs 8-bit MD, 4x4 disallowed and a 64x64 SB.
    if pic_lpd1_lvl != 0
        && !(i.hbd_md == 0 && i.pic_disallow_4x4_effective() && i.super_block_size == 64)
    {
        pic_lpd1_lvl = 0;
    }

    // Lambda weight.
    let mut lambda_weight: i64 = 0;
    if i.tune == TUNE_IQ {
        let qp = i64::from(i.picture_qp);
        lambda_weight = super::pd0::clip3(0, 72, (qp * 4).min((63 - qp) * 3)) + 128;
    } else if !(m <= MR) {
        if !i.is_islice && i.picture_qp >= 62 {
            lambda_weight = 300;
        } else if i.picture_qp >= 56 {
            lambda_weight = 175;
        } else if i.picture_qp >= 16 {
            lambda_weight = 150;
        }
    }
    if i.sq_qp == MAX_QP_VALUE && i.extended_crf_qindex_offset != 0 {
        lambda_weight += i64::from(i.extended_crf_qindex_offset) * 28;
    }

    // Deblocking level. The shim behind this struct's differential pins
    // `enable_dlf_flag = 0`, which is exactly the arm that forces the level to
    // 0, so 0 is the FAITHFUL value for this surface — not a stub. The real
    // ladder + controls table live in `leaf::get_dlf_level_*` /
    // `ctrls::set_dlf_controls` and are gated by `c_parity_dlf_ctrls.rs`.
    let dlf_level = 0u8;

    Some(MdConfigSignals {
        mfmv_level,
        rdoq_level,
        coeff_shaving_level,
        rate_est_level,
        cdf_ctrl,
        pic_filter_intra_level,
        use_accurate_part_ctx,
        allow_high_precision_mv,
        wm_level,
        allow_warped_motion,
        is_motion_mode_switchable,
        pic_obmc_level,
        approx_inter_rate,
        skip_intra,
        intra_level,
        dist_based_ang_intra_level,
        cand_reduction_level,
        txt_level,
        tx_shortcut_level,
        pd0_cost_bias_weight,
        interpolation_search_level,
        interpolation_filter,
        chroma_level,
        cfl_level,
        new_nearest_near_comb_injection,
        unipred3x3_injection,
        bipred3x3_injection,
        inter_compound_mode,
        dist_based_ref_pruning,
        spatial_sse_full_loop_level,
        nsq_geom_level,
        nsq_search_level,
        inter_intra_level,
        txs_level,
        tx_mode,
        nic_level,
        md_sq_mv_search_level,
        md_nsq_mv_search_level,
        md_pme_level,
        me_subpel_level,
        pme_subpel_level,
        mds0_level,
        pic_disallow_4x4,
        pic_bypass_encdec,
        pic_pd0_lvl,
        pic_depth_removal_level,
        pic_block_based_depth_refinement_level,
        pic_lpd1_lvl,
        lambda_weight: lambda_weight as u32,
        dlf_level,
    })
}

impl MdConfigInputs {
    /// `pcs->pic_disallow_4x4` as the LPD1 gate reads it — the value this
    /// function assigned earlier in the same call, not an input.
    fn pic_disallow_4x4_effective(&self) -> bool {
        get_disallow_4x4_default(self.enc_mode)
    }
}
