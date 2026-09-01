//! Deblock signal-derivation oracle — `shims/dlf_shims.c`.
//!
//! **Evidence tier 1.** `get_dlf_level_default`, `get_dlf_level_allintra`,
//! `dlf_level_modulation` and `svt_aom_set_dlf_controls` are all file-`static`
//! in `enc_mode_config.c`, but the EXPORTED
//! `svt_aom_sig_deriv_mode_decision_config_{default,allintra}` reach all four
//! and leave the result in `ppcs->dlf_ctrls`, which the shim reads back. So
//! this differential drives the real C ladder and the real C controls table.
//!
//! The `dlf_level` scalar is not stored anywhere C keeps, but the eight
//! control fields are distinct for each of the eight levels, so the composite
//! `level -> controls` mapping is fully observable through this surface.

unsafe extern "C" {
    fn ref_dlf_ctrls_default(input: *const i32, out: *mut i64);
    fn ref_dlf_ctrls_allintra(input: *const i32, out: *mut i64);
    fn ref_dlf_ctrls_in_slots() -> i32;
    fn ref_dlf_ctrls_out_slots() -> i32;
}

/// Input slot indices, mirroring the shim's `DLF_I_*`.
///
/// The layout is the md-config shim's `MD_I_*` (the whole
/// `svt_aom_sig_deriv_mode_decision_config_*` body runs, so every field it
/// dereferences must be populated) plus the two members of C's
/// `if (enable_dlf_flag && frm_hdr->allow_intrabc == 0)` guard.
pub mod dlf_in {
    /// `pcs->enc_mode`
    pub const ENC_MODE: usize = 0;
    /// `ppcs->is_ref`
    pub const IS_REF: usize = 1;
    /// `ppcs->temporal_layer_index`
    pub const TEMPORAL_LAYER: usize = 2;
    /// `ppcs->input_resolution`
    pub const INPUT_RES: usize = 3;
    /// `pcs->slice_type == I_SLICE`
    pub const IS_ISLICE: usize = 4;
    /// `ppcs->sc_class5`
    pub const SC_CLASS5: usize = 5;
    /// `scs->static_config.fast_decode`
    pub const FAST_DECODE: usize = 6;
    /// `ppcs->hierarchical_levels`
    pub const HIER_LEVELS: usize = 7;
    /// `ppcs->transition_present`
    pub const TRANSITION: usize = 8;
    /// `ppcs->is_highest_layer`
    pub const IS_HIGHEST_LAYER: usize = 9;
    /// `scs->static_config.qp`
    pub const SQ_QP: usize = 10;
    /// `scs->mfmv_enabled`
    pub const MFMV_ENABLED: usize = 11;
    /// `ppcs->frm_hdr.error_resilient_mode`
    pub const ERROR_RESILIENT: usize = 12;
    /// `ppcs->frm_hdr.quantization_params.base_q_idx`
    pub const BASE_Q: usize = 13;
    /// `pcs->ref_hp_percentage`
    pub const REF_HP_PERC: usize = 14;
    /// `scs->input_resolution`
    pub const SCS_INPUT_RES: usize = 15;
    /// `frm_hdr.frame_type == KEY_FRAME`
    pub const FRAME_IS_INTRA: usize = 16;
    /// `ppcs->frame_superres_enabled`
    pub const SUPERRES: usize = 17;
    /// `ppcs->frame_resize_enabled`
    pub const RESIZE_ENABLED: usize = 18;
    /// `scs->seq_qp_mod`
    pub const SEQ_QP_MOD: usize = 19;
    /// `scs->static_config.resize_mode`
    pub const RESIZE_MODE: usize = 20;
    /// `pcs->ref_intra_percentage`
    pub const REF_INTRA_PERC: usize = 21;
    /// `scs->rc_stat_gen_pass_mode`
    pub const RC_STAT_GEN: usize = 22;
    /// `pcs->ref_skip_percentage` — the `dlf_level_modulation` input.
    pub const REF_SKIP_PERC: usize = 23;
    /// `pcs->coeff_lvl`
    pub const COEFF_LVL: usize = 24;
    /// `ppcs->ref_list0_count_try`
    pub const REF_L0_TRY: usize = 25;
    /// `ppcs->ref_list1_count_try`
    pub const REF_L1_TRY: usize = 26;
    /// `scs->seq_header.enable_interintra_compound`
    pub const ENABLE_II: usize = 27;
    /// `scs->static_config.encoder_bit_depth`
    pub const BIT_DEPTH: usize = 28;
    /// `frm_hdr.segmentation_params.segmentation_enabled`
    pub const SEGMENTATION: usize = 29;
    /// `scs->super_block_size`
    pub const SB_SIZE: usize = 30;
    /// `ppcs->hbd_md`
    pub const HBD_MD: usize = 31;
    /// `ppcs->r0_gen`
    pub const R0_GEN: usize = 32;
    /// `ppcs->r0 * 1000`
    pub const R0_MILLI: usize = 33;
    /// `pcs->temporal_layer_index` — C's `is_base` argument is
    /// `(pcs->temporal_layer_index == 0)`.
    pub const PCS_TEMPORAL_LAYER: usize = 34;
    /// `scs->static_config.tune`
    pub const TUNE: usize = 35;
    /// `ppcs->picture_qp`
    pub const PICTURE_QP: usize = 36;
    /// `scs->static_config.extended_crf_qindex_offset`
    pub const EXT_CRF_OFFSET: usize = 37;
    /// `scs->static_config.enable_dlf_flag` (0 = off, 1 = on, 2 = "three
    /// presets lower").
    pub const ENABLE_DLF_FLAG: usize = 38;
    /// `ppcs->frm_hdr.allow_intrabc`
    pub const ALLOW_INTRABC: usize = 39;
    /// Slot count; cross-checked against the shim.
    pub const COUNT: usize = 40;
}

/// Output slot indices, mirroring the shim's `DLF_O_*` — the `DlfCtrls`
/// fields in declaration order (`Codec/pcs.h:603`).
pub mod dlf_out {
    /// `enabled`
    pub const ENABLED: usize = 0;
    /// `sb_based_dlf`
    pub const SB_BASED: usize = 1;
    /// `dlf_avg`
    pub const AVG: usize = 2;
    /// `use_ref_avg_y`
    pub const USE_REF_AVG_Y: usize = 3;
    /// `use_ref_avg_uv`
    pub const USE_REF_AVG_UV: usize = 4;
    /// `early_exit_convergence`
    pub const EARLY_EXIT: usize = 5;
    /// `zero_filter_strength_lvl`
    pub const ZERO_FILT_STRENGTH: usize = 6;
    /// `prev_dlf_dist_th`
    pub const PREV_DIST_TH: usize = 7;
    /// Slot count; cross-checked against the shim.
    pub const COUNT: usize = 8;
}

fn check_slots() {
    assert_eq!(
        unsafe { ref_dlf_ctrls_in_slots() } as usize,
        dlf_in::COUNT,
        "C shim dlf input slot count drifted"
    );
    assert_eq!(
        unsafe { ref_dlf_ctrls_out_slots() } as usize,
        dlf_out::COUNT,
        "C shim dlf output slot count drifted"
    );
}

/// Run the VIDEO arm — `svt_aom_sig_deriv_mode_decision_config_default`
/// (`enc_mode_config.c:8900`) — and return `ppcs->dlf_ctrls`.
///
/// # Panics
/// If the C shim's slot counts disagree with [`dlf_in::COUNT`] /
/// [`dlf_out::COUNT`].
#[must_use]
pub fn dlf_ctrls_default(input: &[i32; dlf_in::COUNT]) -> [i64; dlf_out::COUNT] {
    check_slots();
    let mut out = [0i64; dlf_out::COUNT];
    unsafe { ref_dlf_ctrls_default(input.as_ptr(), out.as_mut_ptr()) };
    out
}

/// Run the STILL arm — `svt_aom_sig_deriv_mode_decision_config_allintra`
/// (`enc_mode_config.c:9895`) — and return `ppcs->dlf_ctrls`.
///
/// # Panics
/// If the C shim's slot counts disagree with [`dlf_in::COUNT`] /
/// [`dlf_out::COUNT`].
#[must_use]
pub fn dlf_ctrls_allintra(input: &[i32; dlf_in::COUNT]) -> [i64; dlf_out::COUNT] {
    check_slots();
    let mut out = [0i64; dlf_out::COUNT];
    unsafe { ref_dlf_ctrls_allintra(input.as_ptr(), out.as_mut_ptr()) };
    out
}
