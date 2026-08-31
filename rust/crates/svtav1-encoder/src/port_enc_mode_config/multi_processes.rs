//! `svt_aom_sig_deriv_multi_processes_default`
//! (`Codec/enc_mode_config.c:1973`) — the picture-level tool derivation for
//! EVERY video-mode picture, the key frame included.
//!
//! Without it a `SVT_AVIF=0` key frame runs on the allintra tool set and
//! diverges before the first tile byte, so chunk C1a
//! (`docs/INTER-ENCODE-PLAN.md`) cannot pass.
//!
//! **Tier 1** — the entry point is EXPORTED and
//! `c_parity_sig_deriv_multi_processes.rs` drives the real symbol.

use super::ResolutionRange;
use super::ctrls::{GmControls, set_gm_controls};
use super::enc_mode::*;
use super::leaf::{derive_gm_level, get_max_can_count, get_sg_filter_level_default};
use super::tail::{CdefReconControls, SgFilterCtrls, set_cdef_recon_controls, set_sg_filter_ctrls};

/// C `MULTI_PASS_PD_ON` — the only value this arm assigns.
pub const MULTI_PASS_PD_ON: u8 = 1;
/// C `DEFAULT` — the "not overridden by static_config" sentinel.
pub const CONFIG_DEFAULT: i32 = -1;
/// C `EB_EIGHT_BIT`.
pub const EB_EIGHT_BIT: u32 = 8;

/// C `svt_aom_get_wn_filter_level_default` (`enc_mode_config.c:1356`). static.
///
/// Wiener is OFF on the highest temporal layer at every preset, and off
/// entirely above M8 and at 8K.
#[must_use]
pub fn get_wn_filter_level_default(
    enc_mode: i8,
    input_resolution: u8,
    is_not_last_layer: bool,
) -> u8 {
    let mut wn = if enc_mode <= M3 {
        if is_not_last_layer { 4 } else { 0 }
    } else if enc_mode <= M8 {
        if is_not_last_layer { 5 } else { 0 }
    } else {
        0
    };
    if input_resolution >= ResolutionRange::R8k.as_u8() {
        wn = 0;
    }
    wn
}

/// The `enabled` field of C `set_intrabc_level` (`enc_mode_config.c:1657`).
///
/// Every nonzero level sets it; only level 0 clears it. The rest of
/// `IntrabcCtrls` belongs to the IntraBC port, not this one.
#[must_use]
pub fn intrabc_enabled(ibc_level: u8) -> bool {
    ibc_level != 0
}

/// Inputs of C `svt_aom_sig_deriv_multi_processes_default`.
#[derive(Debug, Clone, Copy)]
pub struct MultiProcessesInputs {
    /// `pcs->enc_mode`
    pub enc_mode: i8,
    /// `pcs->slice_type == I_SLICE`
    pub is_islice: bool,
    /// `pcs->temporal_layer_index == 0`
    pub is_base: bool,
    /// `pcs->input_resolution`
    pub input_resolution: ResolutionRange,
    /// `scs->static_config.fast_decode`
    pub fast_decode: u8,
    /// `pcs->sc_class5`
    pub sc_class5: u8,
    /// `!pcs->is_highest_layer`
    pub is_not_last_layer: bool,
    /// `pcs->tf_ctrls.hme_me_level`
    pub tf_hme_me_level: u8,
    /// `scs->static_config.enable_intrabc`
    pub enable_intrabc: bool,
    /// `scs->seq_header.cdef_level`
    pub seq_cdef_level: u8,
    /// `scs->static_config.cdef_level` (`DEFAULT` == -1 means "derive")
    pub config_cdef_level: i32,
    /// `scs->seq_header.enable_restoration`
    pub seq_enable_restoration: bool,
    /// `scs->max_initial_input_luma_width`
    pub max_initial_luma_width: u16,
    /// `scs->max_initial_input_luma_height`
    pub max_initial_luma_height: u16,
    /// `scs->encoder_bit_depth`
    pub encoder_bit_depth: u32,
    /// `scs->static_config.hbd_mds` (`DEFAULT` == -1 means "derive")
    pub config_hbd_mds: i32,
    /// `pcs->slice_type == I_SLICE` for the GM derivation (same value as
    /// [`Self::is_islice`], kept separate because C reads it through a
    /// different helper).
    pub gm_super_res_off: bool,
}

/// What `svt_aom_sig_deriv_multi_processes_default` writes, restricted to the
/// fields this lane models.
///
/// NOT modelled, each because it comes from a table this lane has not ported:
/// `pcs->intrabc_ctrls` beyond `enabled`, `pcs->palette_ctrls` (from
/// `set_palette_level`), `pcs->cdef_search_ctrls` (from
/// `set_cdef_search_controls`) and `cm->wn_filter_ctrls` (from
/// `svt_aom_set_wn_filter_ctrls`). The LEVELS feeding all four ARE modelled,
/// since deriving them is what lives here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MultiProcessesSignals {
    /// `pcs->gm_ctrls`
    pub gm: GmControls,
    /// `pcs->enable_hme_flag`
    pub enable_hme_flag: u8,
    /// `pcs->enable_hme_level0_flag`
    pub enable_hme_level0_flag: u8,
    /// `pcs->enable_hme_level1_flag`
    pub enable_hme_level1_flag: u8,
    /// `pcs->enable_hme_level2_flag`
    pub enable_hme_level2_flag: u8,
    /// `pcs->tf_enable_hme_flag`
    pub tf_enable_hme_flag: u8,
    /// `pcs->tf_enable_hme_level0_flag`
    pub tf_enable_hme_level0_flag: u8,
    /// `pcs->tf_enable_hme_level1_flag`
    pub tf_enable_hme_level1_flag: u8,
    /// `pcs->tf_enable_hme_level2_flag`
    pub tf_enable_hme_level2_flag: u8,
    /// `pcs->multi_pass_pd_level`
    pub multi_pass_pd_level: u8,
    /// The derived intra-BC level (the controls table is not ported).
    pub intrabc_level: u8,
    /// `frm_hdr->allow_intrabc`
    pub allow_intrabc: u8,
    /// `pcs->palette_level`
    pub palette_level: u8,
    /// `frm_hdr->allow_screen_content_tools`
    pub allow_screen_content_tools: u8,
    /// `pcs->cdef_level` — the CDEF SEARCH level.
    pub cdef_level: u8,
    /// The derived CDEF recon level.
    pub cdef_recon_level: u8,
    /// `pcs->cdef_recon_ctrls`
    pub cdef_recon: CdefReconControls,
    /// The derived Wiener level (the controls table is not ported).
    pub wn_filter_level: u8,
    /// The derived SGR level.
    pub sg_filter_level: u8,
    /// `cm->sg_filter_ctrls`
    pub sg_filter: SgFilterCtrls,
    /// `pcs->enable_restoration`
    pub enable_restoration: u8,
    /// `pcs->frame_end_cdf_update_mode`
    pub frame_end_cdf_update_mode: u8,
    /// `pcs->hbd_md`
    pub hbd_md: u8,
    /// `pcs->max_can_count`
    pub max_can_count: u16,
    /// `pcs->use_best_me_unipred_cand_only`
    pub use_best_me_unipred_cand_only: u8,
}

/// C `svt_aom_sig_deriv_multi_processes_default` (`enc_mode_config.c:1973`).
/// EXPORTED.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn sig_deriv_multi_processes_default(i: MultiProcessesInputs) -> Option<MultiProcessesSignals> {
    let enc_mode = i.enc_mode;
    let sc5 = i.sc_class5 != 0;

    // GM controls are set assuming super-res is OFF, for the gm-pp need.
    let gm_level = derive_gm_level(enc_mode, i.is_islice, true);
    let gm = set_gm_controls(gm_level, i.input_resolution)?;

    // HME flags. Level 2 is on ONLY for screen content at <= M2.
    let enable_hme_level2_flag = u8::from(sc5 && enc_mode <= M2);

    // TF HME flags, keyed on tf_ctrls.hme_me_level.
    let (tf_l1, tf_l2) = match i.tf_hme_me_level {
        0 => (1u8, 1u8),
        1 | 2 => (1, 0),
        3 | 4 => (0, 0),
        _ => return None,
    };

    // Intra-BC level. Screen content on an I-slice only.
    let intrabc_level = if !i.enable_intrabc || !sc5 || !i.is_islice {
        0
    } else if enc_mode <= M3 {
        2
    } else if enc_mode <= M5 {
        3
    } else if enc_mode <= M8 {
        5
    } else if enc_mode <= M9 {
        6
    } else {
        0
    };
    let allow_intrabc = u8::from(intrabc_enabled(intrabc_level));

    // Palette level. Screen content on an I-slice only.
    let palette_level = if !sc5 || !i.is_islice {
        0
    } else if enc_mode <= M0 {
        1
    } else if enc_mode <= M1 {
        2
    } else if enc_mode <= M2 {
        4
    } else if enc_mode <= M5 {
        5
    } else if enc_mode <= M9 {
        6
    } else if enc_mode <= M10 {
        8
    } else {
        0
    };

    let allow_screen_content_tools = u8::from(sc5 && (palette_level != 0 || allow_intrabc != 0));

    // CDEF search level.
    let cdef_level = if i.seq_cdef_level == 0 || allow_intrabc != 0 {
        0
    } else if i.config_cdef_level != CONFIG_DEFAULT {
        // C casts through int8_t.
        i.config_cdef_level as i8 as u8
    } else if enc_mode <= MR {
        1
    } else if enc_mode <= M2 {
        2
    } else if enc_mode <= M5 {
        5
    } else if enc_mode <= M7 {
        if i.is_base { 5 } else { 6 }
    } else {
        7
    };

    let cdef_recon_level =
        super::tail::cdef_recon_level_default(enc_mode, i.fast_decode, i.input_resolution);
    let cdef_recon = set_cdef_recon_controls(cdef_recon_level)?;

    // Loop restoration. NOTE the resolution used is the SEQUENCE's INITIAL
    // one, not the picture's current one — allocation already happened on it.
    let mut wn_filter_level = 0u8;
    let mut sg_filter_level = 0u8;
    if i.seq_enable_restoration {
        let init_res = ResolutionRange::from_luma_area(
            u32::from(i.max_initial_luma_width) * u32::from(i.max_initial_luma_height),
        );
        wn_filter_level =
            get_wn_filter_level_default(enc_mode, init_res.as_u8(), i.is_not_last_layer);
        sg_filter_level = get_sg_filter_level_default(enc_mode, init_res.as_u8(), i.fast_decode);
    }
    let sg_filter = set_sg_filter_ctrls(sg_filter_level)?;
    let enable_restoration = u8::from(wn_filter_level > 0 || sg_filter_level > 0);

    // High-bit-depth mode decision.
    let hbd_md = if i.encoder_bit_depth == EB_EIGHT_BIT {
        0
    } else if i.config_hbd_mds != CONFIG_DEFAULT {
        i.config_hbd_mds as u8
    } else if enc_mode <= MR {
        1
    } else if enc_mode <= M5 {
        if i.is_base { 2 } else { 0 }
    } else if i.is_islice {
        2
    } else {
        0
    };

    Some(MultiProcessesSignals {
        gm,
        enable_hme_flag: 1,
        enable_hme_level0_flag: 1,
        enable_hme_level1_flag: 1,
        enable_hme_level2_flag,
        tf_enable_hme_flag: 1,
        tf_enable_hme_level0_flag: 1,
        tf_enable_hme_level1_flag: tf_l1,
        tf_enable_hme_level2_flag: tf_l2,
        multi_pass_pd_level: MULTI_PASS_PD_ON,
        intrabc_level,
        allow_intrabc,
        palette_level,
        allow_screen_content_tools,
        cdef_level,
        cdef_recon_level,
        cdef_recon,
        wn_filter_level,
        sg_filter_level,
        sg_filter,
        enable_restoration,
        frame_end_cdf_update_mode: 1,
        hbd_md,
        max_can_count: get_max_can_count(enc_mode, false),
        use_best_me_unipred_cand_only: u8::from(enc_mode > M1),
    })
}
