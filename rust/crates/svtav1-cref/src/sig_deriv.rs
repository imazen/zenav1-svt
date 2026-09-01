//! Bindings to the exported per-preset signal derivations of
//! `Source/Lib/Codec/enc_mode_config.c`.
//!
//! Everything here drives the REAL symbol out of `libSvtAv1Enc.a`, so the
//! differentials built on it are `WORKING-ON-THIS.md` §4 **tier 1**.
//!
//! Functions whose C signature takes only scalars are bound directly; the ones
//! that take a `PictureControlSet*` / `PictureParentControlSet*` /
//! `SequenceControlSet*` go through a shim in `shims/sigderiv_shims.c` that
//! builds a synthetic control set (heap-allocated per call — no `static`
//! state, see that file's header).

unsafe extern "C" {
    // --- direct bindings: pure functions of scalars ---
    fn svt_aom_get_enable_me_8x8(enc_mode: i8, input_resolution: i32, rtc_tune: bool) -> u8;
    fn svt_aom_get_enable_me_16x16(enc_mode: i8) -> u8;
    fn svt_aom_get_gm_core_level(enc_mode: i8, super_res_off: bool) -> u8;
    fn svt_aom_get_max_can_count(enc_mode: i8, rtc: bool) -> u16;
    fn svt_aom_get_disallow_8x8_default() -> bool;
    fn svt_aom_get_disallow_8x8_rtc(enc_mode: i8, aligned_width: u16, aligned_height: u16) -> bool;
    fn svt_aom_get_disallow_8x8_allintra() -> bool;
    fn svt_aom_get_nsq_geom_level_default(enc_mode: i8, coeff_lvl: i32) -> u8;
    fn svt_aom_get_nsq_geom_level_rtc() -> u8;
    fn svt_aom_get_nsq_geom_level_allintra(enc_mode: i8) -> u8;
    fn svt_aom_get_nic_level_default(enc_mode: i8, is_base: u8) -> u8;
    fn svt_aom_get_nic_level_rtc(enc_mode: i8) -> u8;
    fn svt_aom_get_nic_level_allintra(enc_mode: i8) -> u8;
    fn svt_aom_get_bypass_encdec_default(enc_mode: i8, encoder_bit_depth: u8) -> u8;
    fn svt_aom_get_bypass_encdec_rtc(enc_mode: i8, encoder_bit_depth: u8) -> u8;
    fn svt_aom_get_bypass_encdec_allintra(enc_mode: i8) -> u8;
    fn svt_aom_get_update_cdf_level_default(enc_mode: i8, is_islice: i32, is_base: u8) -> u8;
    fn svt_aom_get_update_cdf_level_rtc(enc_mode: i8, is_islice: i32) -> u8;
    fn svt_aom_get_update_cdf_level_allintra(enc_mode: i8) -> u8;
    fn svt_aom_get_chroma_level_default(enc_mode: i8, is_islice: u8) -> u8;
    fn svt_aom_get_chroma_level_rtc(enc_mode: i8) -> u8;
    fn svt_aom_get_chroma_level_allintra(enc_mode: i8) -> u8;
    fn svt_aom_get_enable_sg_default(enc_mode: i8, input_resolution: u8, fast_decode: u8) -> u8;
    fn svt_aom_get_enable_sg_rtc(input_resolution: u8, fast_decode: u8) -> u8;
    fn svt_aom_get_enable_sg_allintra(enc_mode: i8) -> u8;
    fn get_inter_compound_level(enc_mode: i8) -> u8;
    fn svt_aom_get_obmc_level(enc_mode: i8, qp: u32, seq_qp_mod: u8) -> u8;
    fn svt_aom_get_intra_mode_levels_default(
        enc_mode: i8,
        is_islice: bool,
        is_base: bool,
        transition_present: i32,
        intra_level: *mut u32,
        dist_based_ang_intra_level: *mut u32,
    );
    fn svt_aom_get_intra_mode_levels_rtc(
        enc_mode: i8,
        is_islice: bool,
        transition_present: i32,
        use_flat_ipp: bool,
        intra_level: *mut u32,
        dist_based_ang_intra_level: *mut u32,
    );
    fn svt_aom_get_intra_mode_levels_allintra(
        enc_mode: i8,
        intra_level: *mut u32,
        dist_based_ang_intra_level: *mut u32,
    );

    // --- shimmed: the C signature takes a control set ---
    fn ref_get_nsq_search_level_default(
        enc_mode: i8,
        coeff_lvl: i32,
        qp: u32,
        ppcs_temporal_layer_index: u8,
        r0_gen: u8,
        r0: f64,
        is_islice: u8,
        temporal_layer_index: u8,
        seq_qp_mod: u8,
    ) -> u8;
    fn ref_get_nsq_search_level_rtc(coeff_lvl: i32, qp: u32, seq_qp_mod: u8) -> u8;
    fn ref_get_nsq_search_level_allintra(
        enc_mode: i8,
        qp: u32,
        coeff_lvl: i32,
        seq_qp_mod: u8,
    ) -> u8;
    fn ref_derive_gm_level(enc_mode: i8, is_islice: u8, super_res_off: u8) -> u8;
    fn ref_sig_deriv_pre_analysis_pcs(enc_mode: i8, max_w: u16, max_h: u16, rtc: u8, out: *mut u8);
    fn ref_set_mfmv_config(enc_mode: i8, rtc: u8, config_enable_mfmv: i32) -> u8;
    fn ref_is_ref_same_size(
        is_not_scaled: u8,
        is_b_slice: u8,
        ref_present: u8,
        ref_w: u16,
        ref_h: u16,
        frame_w: u16,
        frame_h: u16,
    ) -> u8;
}

/// C `svt_aom_get_enable_me_8x8`.
#[must_use]
pub fn get_enable_me_8x8(enc_mode: i8, input_resolution: u8, rtc_tune: bool) -> u8 {
    unsafe { svt_aom_get_enable_me_8x8(enc_mode, i32::from(input_resolution), rtc_tune) }
}

/// C `svt_aom_get_enable_me_16x16`.
#[must_use]
pub fn get_enable_me_16x16(enc_mode: i8) -> u8 {
    unsafe { svt_aom_get_enable_me_16x16(enc_mode) }
}

/// C `svt_aom_get_gm_core_level`.
#[must_use]
pub fn get_gm_core_level(enc_mode: i8, super_res_off: bool) -> u8 {
    unsafe { svt_aom_get_gm_core_level(enc_mode, super_res_off) }
}

/// C `svt_aom_derive_gm_level` (through the synthetic-PPCS shim).
#[must_use]
pub fn derive_gm_level(enc_mode: i8, is_islice: bool, super_res_off: bool) -> u8 {
    unsafe { ref_derive_gm_level(enc_mode, u8::from(is_islice), u8::from(super_res_off)) }
}

/// C `svt_aom_get_max_can_count`.
#[must_use]
pub fn get_max_can_count(enc_mode: i8, rtc: bool) -> u16 {
    unsafe { svt_aom_get_max_can_count(enc_mode, rtc) }
}

/// C `svt_aom_get_disallow_8x8_default`.
#[must_use]
pub fn get_disallow_8x8_default() -> bool {
    unsafe { svt_aom_get_disallow_8x8_default() }
}

/// C `svt_aom_get_disallow_8x8_rtc`.
#[must_use]
pub fn get_disallow_8x8_rtc(enc_mode: i8, aligned_width: u16, aligned_height: u16) -> bool {
    unsafe { svt_aom_get_disallow_8x8_rtc(enc_mode, aligned_width, aligned_height) }
}

/// C `svt_aom_get_disallow_8x8_allintra`.
#[must_use]
pub fn get_disallow_8x8_allintra() -> bool {
    unsafe { svt_aom_get_disallow_8x8_allintra() }
}

/// C `svt_aom_get_nsq_geom_level_default`.
#[must_use]
pub fn get_nsq_geom_level_default(enc_mode: i8, coeff_lvl: u8) -> u8 {
    unsafe { svt_aom_get_nsq_geom_level_default(enc_mode, i32::from(coeff_lvl)) }
}

/// C `INVALID_LVL` (`Codec/definitions.h:288`) — `~0`, the value
/// `pcs->coeff_lvl` keeps on a video-mode I-slice (`md_config_process.c:898`
/// runs neither `derive_intra_coeff_level` nor `derive_inter_coeff_level`
/// there). The enum carries a negative enumerator, so its underlying type is
/// signed and the value is `-1` — which is why the `_raw` entry points below
/// exist: the `u8` wrappers cannot express it.
pub const INVALID_COEFF_LVL: i32 = -1;

/// C `svt_aom_get_nsq_geom_level_default` with the raw `InputCoeffLvl` integer,
/// so a caller can pass [`INVALID_COEFF_LVL`].
#[must_use]
pub fn get_nsq_geom_level_default_raw(enc_mode: i8, coeff_lvl: i32) -> u8 {
    unsafe { svt_aom_get_nsq_geom_level_default(enc_mode, coeff_lvl) }
}

/// C `svt_aom_get_nsq_search_level_default` with the raw `InputCoeffLvl`
/// integer, so a caller can pass [`INVALID_COEFF_LVL`].
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn get_nsq_search_level_default_raw(
    enc_mode: i8,
    coeff_lvl: i32,
    qp: u32,
    ppcs_temporal_layer_index: u8,
    r0_gen: bool,
    r0: f64,
    is_islice: bool,
    temporal_layer_index: u8,
    seq_qp_mod: u8,
) -> u8 {
    unsafe {
        ref_get_nsq_search_level_default(
            enc_mode,
            coeff_lvl,
            qp,
            ppcs_temporal_layer_index,
            u8::from(r0_gen),
            r0,
            u8::from(is_islice),
            temporal_layer_index,
            seq_qp_mod,
        )
    }
}

/// C `svt_aom_get_nsq_geom_level_rtc`.
#[must_use]
pub fn get_nsq_geom_level_rtc() -> u8 {
    unsafe { svt_aom_get_nsq_geom_level_rtc() }
}

/// C `svt_aom_get_nsq_geom_level_allintra`.
#[must_use]
pub fn get_nsq_geom_level_allintra(enc_mode: i8) -> u8 {
    unsafe { svt_aom_get_nsq_geom_level_allintra(enc_mode) }
}

/// C `svt_aom_get_nsq_search_level_default` (through the synthetic-PCS shim).
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn get_nsq_search_level_default(
    enc_mode: i8,
    coeff_lvl: u8,
    qp: u32,
    ppcs_temporal_layer_index: u8,
    r0_gen: bool,
    r0: f64,
    is_islice: bool,
    temporal_layer_index: u8,
    seq_qp_mod: u8,
) -> u8 {
    unsafe {
        ref_get_nsq_search_level_default(
            enc_mode,
            i32::from(coeff_lvl),
            qp,
            ppcs_temporal_layer_index,
            u8::from(r0_gen),
            r0,
            u8::from(is_islice),
            temporal_layer_index,
            seq_qp_mod,
        )
    }
}

/// C `svt_aom_get_nsq_search_level_rtc` (through the synthetic-PCS shim).
#[must_use]
pub fn get_nsq_search_level_rtc(coeff_lvl: u8, qp: u32, seq_qp_mod: u8) -> u8 {
    unsafe { ref_get_nsq_search_level_rtc(i32::from(coeff_lvl), qp, seq_qp_mod) }
}

/// C `svt_aom_get_nsq_search_level_allintra` (through the synthetic-PCS shim).
#[must_use]
pub fn get_nsq_search_level_allintra(enc_mode: i8, qp: u32, coeff_lvl: u8, seq_qp_mod: u8) -> u8 {
    unsafe { ref_get_nsq_search_level_allintra(enc_mode, qp, i32::from(coeff_lvl), seq_qp_mod) }
}

/// C `svt_aom_get_nic_level_default`.
#[must_use]
pub fn get_nic_level_default(enc_mode: i8, is_base: bool) -> u8 {
    unsafe { svt_aom_get_nic_level_default(enc_mode, u8::from(is_base)) }
}

/// C `svt_aom_get_nic_level_rtc`.
#[must_use]
pub fn get_nic_level_rtc(enc_mode: i8) -> u8 {
    unsafe { svt_aom_get_nic_level_rtc(enc_mode) }
}

/// C `svt_aom_get_nic_level_allintra`.
#[must_use]
pub fn get_nic_level_allintra(enc_mode: i8) -> u8 {
    unsafe { svt_aom_get_nic_level_allintra(enc_mode) }
}

/// C `svt_aom_get_bypass_encdec_default`.
#[must_use]
pub fn get_bypass_encdec_default(enc_mode: i8, encoder_bit_depth: u8) -> u8 {
    unsafe { svt_aom_get_bypass_encdec_default(enc_mode, encoder_bit_depth) }
}

/// C `svt_aom_get_bypass_encdec_rtc`.
#[must_use]
pub fn get_bypass_encdec_rtc(enc_mode: i8, encoder_bit_depth: u8) -> u8 {
    unsafe { svt_aom_get_bypass_encdec_rtc(enc_mode, encoder_bit_depth) }
}

/// C `svt_aom_get_bypass_encdec_allintra`.
#[must_use]
pub fn get_bypass_encdec_allintra(enc_mode: i8) -> u8 {
    unsafe { svt_aom_get_bypass_encdec_allintra(enc_mode) }
}

/// C `svt_aom_get_update_cdf_level_default`. The C parameter is typed
/// `SliceType`, and every call site passes the BOOLEAN `is_islice`, not the
/// enum — so this binding takes a bool and widens it the same way.
#[must_use]
pub fn get_update_cdf_level_default(enc_mode: i8, is_islice: bool, is_base: bool) -> u8 {
    unsafe {
        svt_aom_get_update_cdf_level_default(enc_mode, i32::from(is_islice), u8::from(is_base))
    }
}

/// C `svt_aom_get_update_cdf_level_rtc`.
#[must_use]
pub fn get_update_cdf_level_rtc(enc_mode: i8, is_islice: bool) -> u8 {
    unsafe { svt_aom_get_update_cdf_level_rtc(enc_mode, i32::from(is_islice)) }
}

/// C `svt_aom_get_update_cdf_level_allintra`.
#[must_use]
pub fn get_update_cdf_level_allintra(enc_mode: i8) -> u8 {
    unsafe { svt_aom_get_update_cdf_level_allintra(enc_mode) }
}

/// C `svt_aom_get_chroma_level_default`.
#[must_use]
pub fn get_chroma_level_default(enc_mode: i8, is_islice: bool) -> u8 {
    unsafe { svt_aom_get_chroma_level_default(enc_mode, u8::from(is_islice)) }
}

/// C `svt_aom_get_chroma_level_rtc`.
#[must_use]
pub fn get_chroma_level_rtc(enc_mode: i8) -> u8 {
    unsafe { svt_aom_get_chroma_level_rtc(enc_mode) }
}

/// C `svt_aom_get_chroma_level_allintra`.
#[must_use]
pub fn get_chroma_level_allintra(enc_mode: i8) -> u8 {
    unsafe { svt_aom_get_chroma_level_allintra(enc_mode) }
}

/// C `svt_aom_get_enable_sg_default` — the tier-1 reach onto the `static`
/// `svt_aom_get_sg_filter_level_default` behind it.
#[must_use]
pub fn get_enable_sg_default(enc_mode: i8, input_resolution: u8, fast_decode: u8) -> u8 {
    unsafe { svt_aom_get_enable_sg_default(enc_mode, input_resolution, fast_decode) }
}

/// C `svt_aom_get_enable_sg_rtc`.
#[must_use]
pub fn get_enable_sg_rtc(input_resolution: u8, fast_decode: u8) -> u8 {
    unsafe { svt_aom_get_enable_sg_rtc(input_resolution, fast_decode) }
}

/// C `svt_aom_get_enable_sg_allintra`.
#[must_use]
pub fn get_enable_sg_allintra(enc_mode: i8) -> u8 {
    unsafe { svt_aom_get_enable_sg_allintra(enc_mode) }
}

/// C `get_inter_compound_level`.
#[must_use]
pub fn inter_compound_level(enc_mode: i8) -> u8 {
    unsafe { get_inter_compound_level(enc_mode) }
}

/// C `svt_aom_get_obmc_level`.
#[must_use]
pub fn get_obmc_level(enc_mode: i8, qp: u32, seq_qp_mod: u8) -> u8 {
    unsafe { svt_aom_get_obmc_level(enc_mode, qp, seq_qp_mod) }
}

/// C `svt_aom_get_intra_mode_levels_default` — `(intra_level,
/// dist_based_ang_intra_level)`.
#[must_use]
pub fn get_intra_mode_levels_default(
    enc_mode: i8,
    is_islice: bool,
    is_base: bool,
    transition_present: i32,
) -> (u32, u32) {
    let mut a = 0u32;
    let mut b = 0u32;
    unsafe {
        svt_aom_get_intra_mode_levels_default(
            enc_mode,
            is_islice,
            is_base,
            transition_present,
            &raw mut a,
            &raw mut b,
        );
    }
    (a, b)
}

/// C `svt_aom_get_intra_mode_levels_rtc`.
#[must_use]
pub fn get_intra_mode_levels_rtc(
    enc_mode: i8,
    is_islice: bool,
    transition_present: i32,
    use_flat_ipp: bool,
) -> (u32, u32) {
    let mut a = 0u32;
    let mut b = 0u32;
    unsafe {
        svt_aom_get_intra_mode_levels_rtc(
            enc_mode,
            is_islice,
            transition_present,
            use_flat_ipp,
            &raw mut a,
            &raw mut b,
        );
    }
    (a, b)
}

/// C `svt_aom_get_intra_mode_levels_allintra`.
#[must_use]
pub fn get_intra_mode_levels_allintra(enc_mode: i8) -> (u32, u32) {
    let mut a = 0u32;
    let mut b = 0u32;
    unsafe { svt_aom_get_intra_mode_levels_allintra(enc_mode, &raw mut a, &raw mut b) };
    (a, b)
}

/// C `svt_aom_sig_deriv_pre_analysis_pcs` (through the synthetic-PPCS shim).
///
/// Returns the ten flags in the order
/// `[enable_me_16x16, enable_me_8x8, enable_hme, hme_l0, hme_l1, hme_l2,
///   tf_enable_hme, tf_hme_l0, tf_hme_l1, tf_hme_l2]`.
#[must_use]
pub fn sig_deriv_pre_analysis_pcs(enc_mode: i8, max_w: u16, max_h: u16, rtc: bool) -> [u8; 10] {
    let mut out = [0u8; 10];
    unsafe {
        ref_sig_deriv_pre_analysis_pcs(enc_mode, max_w, max_h, u8::from(rtc), out.as_mut_ptr());
    }
    out
}

/// C `svt_aom_set_mfmv_config` — returns `scs->mfmv_enabled`.
#[must_use]
pub fn set_mfmv_config(enc_mode: i8, rtc: bool, config_enable_mfmv: i32) -> u8 {
    unsafe { ref_set_mfmv_config(enc_mode, u8::from(rtc), config_enable_mfmv) }
}

/// C `svt_aom_is_ref_same_size` (through the synthetic-PCS shim).
#[must_use]
pub fn is_ref_same_size(
    is_not_scaled: bool,
    is_b_slice: bool,
    ref_present: bool,
    ref_w: u16,
    ref_h: u16,
    frame_w: u16,
    frame_h: u16,
) -> bool {
    unsafe {
        ref_is_ref_same_size(
            u8::from(is_not_scaled),
            u8::from(is_b_slice),
            u8::from(ref_present),
            ref_w,
            ref_h,
            frame_w,
            frame_h,
        ) != 0
    }
}

// ---------------------------------------------------------------------------
// level -> controls tables
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn ref_set_wm_controls(wm_level: u8, out: *mut u32);
    fn ref_set_bipred3x3_controls(level: u8, out: *mut u32);
    fn ref_set_dist_based_ref_pruning_controls(level: u8, out: *mut u32);
    fn ref_md_pme_search_controls(level: u8, out: *mut i32);
    fn ref_set_gm_controls(gm_level: u8, input_resolution: i32, out: *mut u32);
}

/// C `svt_aom_set_wm_controls` on a zeroed `ModeDecisionContext`.
///
/// `[enabled, use_wm_for_mvp, refinement_iterations, refine_diag,
///   refine_level, lower_band_th, upper_band_th, shut_approx_if_not_mds0]`
#[must_use]
pub fn set_wm_controls(wm_level: u8) -> [u32; 8] {
    let mut out = [0u32; 8];
    unsafe { ref_set_wm_controls(wm_level, out.as_mut_ptr()) };
    out
}

/// C `svt_aom_set_bipred3x3_controls` on a zeroed `ModeDecisionContext`.
///
/// `[enabled, search_diag, use_best_list, use_l0_l1_dev]`
#[must_use]
pub fn set_bipred3x3_controls(level: u8) -> [u32; 4] {
    let mut out = [0u32; 4];
    unsafe { ref_set_bipred3x3_controls(level, out.as_mut_ptr()) };
    out
}

/// C `svt_aom_set_dist_based_ref_pruning_controls` on a zeroed context.
///
/// `[enabled, use_tpl_info_offset, check_closest_multiplier,
///   max_dev_to_best[0..11], closest_refs[0..11]]`
#[must_use]
pub fn set_dist_based_ref_pruning_controls(level: u8) -> [u32; 25] {
    let mut out = [0u32; 25];
    unsafe { ref_set_dist_based_ref_pruning_controls(level, out.as_mut_ptr()) };
    out
}

/// C `svt_aom_md_pme_search_controls` on a zeroed context.
///
/// `[enabled, dist_type, full_pel_search_width, full_pel_search_height,
///   early_check_mv_th_multiplier, pre_fp_pme_to_me_cost_th,
///   pre_fp_pme_to_me_mv_th, post_fp_pme_to_me_cost_th,
///   post_fp_pme_to_me_mv_th, enable_psad, sa_q_weight]`
#[must_use]
pub fn md_pme_search_controls(level: u8) -> [i32; 11] {
    let mut out = [0i32; 11];
    unsafe { ref_md_pme_search_controls(level, out.as_mut_ptr()) };
    out
}

/// C `svt_aom_set_gm_controls` on a zeroed `PictureParentControlSet`.
///
/// `[enabled, identiy_exit, search_start_model, search_end_model,
///   skip_identity, bypass_based_on_me, params_refinement_steps,
///   downsample_level, corners, chess_rfn, match_sz, inj_psq_glb, pp_enabled,
///   ref_idx0_only, rfn_early_exit, correspondence_method]`
#[must_use]
pub fn set_gm_controls(gm_level: u8, input_resolution: u8) -> [u32; 16] {
    let mut out = [0u32; 16];
    unsafe { ref_set_gm_controls(gm_level, i32::from(input_resolution), out.as_mut_ptr()) };
    out
}

// ---------------------------------------------------------------------------
// ME signal derivation
// ---------------------------------------------------------------------------

/// Number of `u32` slots the ME dump uses. Mirrors the C shim's `ME_O_COUNT`,
/// which carries a compile-time assertion on this value.
pub const ME_OUT_SLOTS: usize = 62;

/// Slot indices of the ME dump, mirroring the C shim's `ME_O_*` enum.
pub mod me_slot {
    /// `me_sa.sa_min.width`
    pub const SA_MIN_W: usize = 0;
    /// `me_sa.sa_min.height`
    pub const SA_MIN_H: usize = 1;
    /// `me_sa.sa_max.width`
    pub const SA_MAX_W: usize = 2;
    /// `me_sa.sa_max.height`
    pub const SA_MAX_H: usize = 3;
    /// `num_hme_sa_w`
    pub const NUM_HME_W: usize = 4;
    /// `num_hme_sa_h`
    pub const NUM_HME_H: usize = 5;
    /// `hme_l0_sa.sa_min.width` (TF: `hme_l0_sa_default_tf`)
    pub const HME_L0_MIN_W: usize = 6;
    /// `hme_l0_sa.sa_min.height`
    pub const HME_L0_MIN_H: usize = 7;
    /// `hme_l0_sa.sa_max.width`
    pub const HME_L0_MAX_W: usize = 8;
    /// `hme_l0_sa.sa_max.height`
    pub const HME_L0_MAX_H: usize = 9;
    /// `hme_l1_sa.width`
    pub const HME_L1_W: usize = 10;
    /// `hme_l1_sa.height`
    pub const HME_L1_H: usize = 11;
    /// `hme_l2_sa.width`
    pub const HME_L2_W: usize = 12;
    /// `hme_l2_sa.height`
    pub const HME_L2_H: usize = 13;
    /// `enable_hme_flag`
    pub const EN_HME: usize = 14;
    /// `enable_hme_level0_flag`
    pub const EN_HME_L0: usize = 15;
    /// `enable_hme_level1_flag`
    pub const EN_HME_L1: usize = 16;
    /// `enable_hme_level2_flag`
    pub const EN_HME_L2: usize = 17;
    /// `hme_search_method`
    pub const HME_METHOD: usize = 18;
    /// `me_search_method`
    pub const ME_METHOD: usize = 19;
    /// `reduce_hme_l0_sr_th_min`
    pub const RED_HME_MIN: usize = 20;
    /// `reduce_hme_l0_sr_th_max`
    pub const RED_HME_MAX: usize = 21;
    /// `prehme_ctrl.enable`
    pub const PREHME_EN: usize = 22;
    /// `prehme_ctrl.prehme_sa_cfg[0].sa_min.width`
    pub const PREHME_V_MIN_W: usize = 23;
    /// `prehme_ctrl.prehme_sa_cfg[0].sa_min.height`
    pub const PREHME_V_MIN_H: usize = 24;
    /// `prehme_ctrl.prehme_sa_cfg[0].sa_max.width`
    pub const PREHME_V_MAX_W: usize = 25;
    /// `prehme_ctrl.prehme_sa_cfg[0].sa_max.height`
    pub const PREHME_V_MAX_H: usize = 26;
    /// `prehme_ctrl.prehme_sa_cfg[1].sa_min.width`
    pub const PREHME_H_MIN_W: usize = 27;
    /// `prehme_ctrl.prehme_sa_cfg[1].sa_min.height`
    pub const PREHME_H_MIN_H: usize = 28;
    /// `prehme_ctrl.prehme_sa_cfg[1].sa_max.width`
    pub const PREHME_H_MAX_W: usize = 29;
    /// `prehme_ctrl.prehme_sa_cfg[1].sa_max.height`
    pub const PREHME_H_MAX_H: usize = 30;
    /// `prehme_ctrl.skip_search_line`
    pub const PREHME_SKIP_LINE: usize = 31;
    /// `prehme_ctrl.l1_early_exit`
    pub const PREHME_L1_EXIT: usize = 32;
    /// `me_hme_prune_ctrls.enable_me_hme_ref_pruning`
    pub const PRUNE_EN: usize = 33;
    /// `me_hme_prune_ctrls.prune_ref_if_hme_sad_dev_bigger_than_th`
    pub const PRUNE_HME_DEV: usize = 34;
    /// `me_hme_prune_ctrls.prune_ref_if_me_sad_dev_bigger_than_th`
    pub const PRUNE_ME_DEV: usize = 35;
    /// `me_hme_prune_ctrls.zz_sad_th`
    pub const PRUNE_ZZ_TH: usize = 36;
    /// `me_hme_prune_ctrls.zz_sad_pct`
    pub const PRUNE_ZZ_PCT: usize = 37;
    /// `me_hme_prune_ctrls.phme_sad_th`
    pub const PRUNE_PHME_TH: usize = 38;
    /// `me_hme_prune_ctrls.phme_sad_pct`
    pub const PRUNE_PHME_PCT: usize = 39;
    /// `me_sr_adjustment_ctrls.enable_me_sr_adjustment`
    pub const SR_EN: usize = 40;
    /// `me_sr_adjustment_ctrls.reduce_me_sr_based_on_mv_length_th`
    pub const SR_MV_LEN_TH: usize = 41;
    /// `me_sr_adjustment_ctrls.stationary_hme_sad_abs_th`
    pub const SR_STAT_TH: usize = 42;
    /// `me_sr_adjustment_ctrls.stationary_me_sr_divisor`
    pub const SR_STAT_DIV: usize = 43;
    /// `me_sr_adjustment_ctrls.reduce_me_sr_based_on_hme_sad_abs_th`
    pub const SR_RED_TH: usize = 44;
    /// `me_sr_adjustment_ctrls.me_sr_divisor_for_low_hme_sad`
    pub const SR_LOW_DIV: usize = 45;
    /// `me_sr_adjustment_ctrls.distance_based_hme_resizing`
    pub const SR_DIST_RESIZE: usize = 46;
    /// `mv_based_sa_adj.enabled`
    pub const MVSA_EN: usize = 47;
    /// `mv_based_sa_adj.nearest_ref_only`
    pub const MVSA_NEAREST: usize = 48;
    /// `mv_based_sa_adj.mv_size_th`
    pub const MVSA_MV_TH: usize = 49;
    /// `mv_based_sa_adj.sa_multiplier`
    pub const MVSA_MULT: usize = 50;
    /// `me_8x8_var_ctrls.enabled`
    pub const VAR_EN: usize = 51;
    /// `me_8x8_var_ctrls.me_sr_div4_th`
    pub const VAR_DIV4: usize = 52;
    /// `me_8x8_var_ctrls.me_sr_div2_th`
    pub const VAR_DIV2: usize = 53;
    /// `me_8x8_var_ctrls.me_sr_mult2_th`
    pub const VAR_MULT2: usize = 54;
    /// `prune_me_candidates_th`
    pub const PRUNE_CAND_TH: usize = 55;
    /// `sc_class_me_boost`
    pub const SC_BOOST: usize = 56;
    /// `use_best_unipred_cand_only`
    pub const BEST_UNIPRED: usize = 57;
    /// `me_early_exit_th`
    pub const EARLY_EXIT: usize = 58;
    /// `me_static_b64_th`
    pub const STATIC_B64: usize = 59;
    /// `me_safe_limit_zz_th`
    pub const SAFE_ZZ: usize = 60;
    /// `prev_me_stage_based_exit_th`
    pub const PREV_STAGE: usize = 61;
}

unsafe extern "C" {
    #[allow(clippy::too_many_arguments)]
    fn ref_sig_deriv_me(
        enc_mode: i8,
        sc_class5: u8,
        input_resolution: i32,
        rtc: u8,
        is_base: u8,
        hierarchical_levels: u8,
        en_hme: u8,
        en_hme_l0: u8,
        en_hme_l1: u8,
        en_hme_l2: u8,
        use_best_unipred: u8,
        me_qp_scaling: u8,
        hme_qp_scaling: u8,
        qp: u32,
        safe_limit_nref: u8,
        safe_limit_zz_th: u32,
        out: *mut u32,
    );
    fn ref_sig_deriv_me_tf(
        hme_me_level: u8,
        input_resolution: i32,
        qp_opt: u8,
        tf_me_qp_scaling: u8,
        qp: u32,
        tf_en_hme: u8,
        tf_en_hme_l0: u8,
        tf_en_hme_l1: u8,
        tf_en_hme_l2: u8,
        out: *mut u32,
    );
}

/// Inputs to [`sig_deriv_me`], mirroring what C reads off the SCS / PPCS.
#[derive(Debug, Clone, Copy)]
pub struct MeArgs {
    /// `pcs->enc_mode`
    pub enc_mode: i8,
    /// `pcs->sc_class5`
    pub sc_class5: u8,
    /// `scs->input_resolution`
    pub input_resolution: u8,
    /// `scs->static_config.rtc`
    pub rtc: bool,
    /// `frame_is_boosted(pcs)`
    pub is_base: bool,
    /// `pcs->hierarchical_levels`
    pub hierarchical_levels: u8,
    /// `pcs->enable_hme_flag`
    pub en_hme: u8,
    /// `pcs->enable_hme_level0_flag`
    pub en_hme_l0: u8,
    /// `pcs->enable_hme_level1_flag`
    pub en_hme_l1: u8,
    /// `pcs->enable_hme_level2_flag`
    pub en_hme_l2: u8,
    /// `pcs->use_best_me_unipred_cand_only`
    pub use_best_unipred: u8,
    /// `scs->qp_based_th_scaling_ctrls.me_qp_based_th_scaling`
    pub me_qp_scaling: bool,
    /// `scs->qp_based_th_scaling_ctrls.hme_qp_based_th_scaling`
    pub hme_qp_scaling: bool,
    /// `scs->static_config.qp`
    pub qp: u32,
    /// `scs->mrp_ctrls.safe_limit_nref`
    pub safe_limit_nref: u8,
    /// `scs->mrp_ctrls.safe_limit_zz_th`
    pub safe_limit_zz_th: u32,
}

/// C `svt_aom_sig_deriv_me` driven on a synthetic SCS / PPCS, with the whole
/// resulting `MeContext` dumped by slot (see [`me_slot`]).
#[must_use]
pub fn sig_deriv_me(a: MeArgs) -> [u32; ME_OUT_SLOTS] {
    let mut out = [0u32; ME_OUT_SLOTS];
    unsafe {
        ref_sig_deriv_me(
            a.enc_mode,
            a.sc_class5,
            i32::from(a.input_resolution),
            u8::from(a.rtc),
            u8::from(a.is_base),
            a.hierarchical_levels,
            a.en_hme,
            a.en_hme_l0,
            a.en_hme_l1,
            a.en_hme_l2,
            a.use_best_unipred,
            u8::from(a.me_qp_scaling),
            u8::from(a.hme_qp_scaling),
            a.qp,
            a.safe_limit_nref,
            a.safe_limit_zz_th,
            out.as_mut_ptr(),
        );
    }
    out
}

/// C `svt_aom_sig_deriv_me_tf` driven on a synthetic PPCS.
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn sig_deriv_me_tf(
    hme_me_level: u8,
    input_resolution: u8,
    qp_opt: bool,
    tf_me_qp_scaling: bool,
    qp: u32,
    tf_en_hme: u8,
    tf_en_hme_l0: u8,
    tf_en_hme_l1: u8,
    tf_en_hme_l2: u8,
) -> [u32; ME_OUT_SLOTS] {
    let mut out = [0u32; ME_OUT_SLOTS];
    unsafe {
        ref_sig_deriv_me_tf(
            hme_me_level,
            i32::from(input_resolution),
            u8::from(qp_opt),
            u8::from(tf_me_qp_scaling),
            qp,
            tf_en_hme,
            tf_en_hme_l0,
            tf_en_hme_l1,
            tf_en_hme_l2,
            out.as_mut_ptr(),
        );
    }
    out
}

// ---------------------------------------------------------------------------
// svt_aom_sig_deriv_enc_dec_default
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn ref_sig_deriv_enc_dec_default(input: *const i32, out: *mut i64);
    fn ref_enc_dec_default_in_slots() -> i32;
    fn ref_enc_dec_default_out_slots() -> i32;
}

/// Input slot indices for [`sig_deriv_enc_dec_default`], mirroring the C shim's
/// `ED_I_*` enum. The count is cross-checked at runtime against the C.
pub mod ed_in {
    /// `pcs->enc_mode`
    pub const ENC_MODE: usize = 0;
    /// `pcs->slice_type == I_SLICE`
    pub const IS_ISLICE: usize = 1;
    /// `pcs->nsq_search_level`
    pub const NSQ_SEARCH: usize = 2;
    /// `pcs->nic_level`
    pub const NIC: usize = 3;
    /// `pcs->cand_reduction_level`
    pub const CAND_RED: usize = 4;
    /// `pcs->txt_level`
    pub const TXT: usize = 5;
    /// `pcs->tx_shortcut_level`
    pub const TX_SHORTCUT: usize = 6;
    /// `pcs->interpolation_search_level`
    pub const IFS: usize = 7;
    /// `pcs->chroma_level`
    pub const CHROMA: usize = 8;
    /// `pcs->cfl_level`
    pub const CFL: usize = 9;
    /// `pcs->wm_level`
    pub const WM: usize = 10;
    /// `pcs->bipred3x3_injection`
    pub const BIPRED3X3: usize = 11;
    /// `pcs->inter_compound_mode`
    pub const INTER_COMP: usize = 12;
    /// `pcs->dist_based_ref_pruning`
    pub const REF_PRUNE: usize = 13;
    /// `pcs->spatial_sse_full_loop_level`
    pub const SPATIAL_SSE: usize = 14;
    /// `pcs->rdoq_level`
    pub const RDOQ: usize = 15;
    /// `pcs->coeff_shaving_level`
    pub const COEFF_SHAVE: usize = 16;
    /// `ppcs->pic_obmc_level`
    pub const OBMC: usize = 17;
    /// `pcs->inter_intra_level`
    pub const INTER_INTRA: usize = 18;
    /// `pcs->txs_level`
    pub const TXS: usize = 19;
    /// `pcs->pic_filter_intra_level`
    pub const FILTER_INTRA: usize = 20;
    /// `pcs->md_sq_mv_search_level`
    pub const MD_SQ_MV: usize = 21;
    /// `pcs->md_nsq_mv_search_level`
    pub const MD_NSQ_MV: usize = 22;
    /// `pcs->md_pme_level`
    pub const MD_PME: usize = 23;
    /// `pcs->me_subpel_level`
    pub const ME_SUBPEL: usize = 24;
    /// `pcs->pme_subpel_level`
    pub const PME_SUBPEL: usize = 25;
    /// `pcs->rate_est_level`
    pub const RATE_EST: usize = 26;
    /// `pcs->intra_level`
    pub const INTRA: usize = 27;
    /// `pcs->dist_based_ang_intra_level`
    pub const DIST_ANG_INTRA: usize = 28;
    /// `pcs->mds0_level`
    pub const MDS0: usize = 29;
    /// `ppcs->update_type` (`SVT_AV1_LF_UPDATE` == 1 makes it a leaf).
    pub const UPDATE_TYPE: usize = 30;
    /// `ppcs->me_8x8_distortion[sb_index]`
    pub const ME_8X8_DIST: usize = 31;
    /// `ppcs->me_8x8_cost_variance[sb_index]`
    pub const ME_8X8_VAR: usize = 32;
    /// `pcs->unipred3x3_injection`
    pub const UNIPRED3X3: usize = 33;
    /// `pcs->new_nearest_near_comb_injection`
    pub const NN_COMB: usize = 34;
    /// `pcs->approx_inter_rate`
    pub const APPROX_INTER_RATE: usize = 35;
    /// `ppcs->frm_hdr.allow_intrabc`
    pub const ALLOW_INTRABC: usize = 36;
    /// `ppcs->palette_level`
    pub const PALETTE_LEVEL: usize = 37;
    /// `ppcs->gm_ctrls.enabled`
    pub const GM_ENABLED: usize = 38;
    /// `ppcs->picture_qp`
    pub const PICTURE_QP: usize = 39;
    /// `pcs->ref_skip_percentage`
    pub const REF_SKIP_PERC: usize = 40;
    /// `scs->static_config.rtc` — read only by `set_cand_reduction_ctrls`'s
    /// `use_flat_ipp` on this path.
    pub const RTC: usize = 41;
    /// `ppcs->hierarchical_levels`
    pub const HIER_LEVELS: usize = 42;
    /// `ctx->lpd1_ctrls.pd1_level` — `is_lpd1` is `> REGULAR_PD1`, and
    /// `REGULAR_PD1` is **-1**, so a zeroed context already counts as LPD1.
    pub const LPD1_PD1_LEVEL: usize = 43;
    /// `ppcs->ref_list0_count_try`
    pub const REF_L0_TRY: usize = 44;
    /// `ppcs->ref_list1_count_try`
    pub const REF_L1_TRY: usize = 45;
    /// `ppcs->use_best_me_unipred_cand_only`
    pub const PPCS_BEST_UNIPRED: usize = 46;
    /// Number of input slots.
    pub const COUNT: usize = 47;
}

/// Number of output slots the enc-dec-default dump uses.
pub const ED_OUT_SLOTS: usize = 119;

/// C `svt_aom_sig_deriv_enc_dec_default` driven on a synthetic PCS, with the
/// resulting `ModeDecisionContext` dumped by slot.
///
/// # Panics
/// If the C shim's slot counts disagree with [`ed_in::COUNT`] /
/// [`ED_OUT_SLOTS`] — a layout drift that would otherwise silently misalign
/// every comparison.
#[must_use]
pub fn sig_deriv_enc_dec_default(input: &[i32; ed_in::COUNT]) -> [i64; ED_OUT_SLOTS] {
    assert_eq!(
        unsafe { ref_enc_dec_default_in_slots() } as usize,
        ed_in::COUNT,
        "C shim input slot count drifted"
    );
    assert_eq!(
        unsafe { ref_enc_dec_default_out_slots() } as usize,
        ED_OUT_SLOTS,
        "C shim output slot count drifted"
    );
    let mut out = [0i64; ED_OUT_SLOTS];
    unsafe { ref_sig_deriv_enc_dec_default(input.as_ptr(), out.as_mut_ptr()) };
    out
}

// ---------------------------------------------------------------------------
// svt_aom_sig_deriv_enc_dec_pd0
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn ref_sig_deriv_enc_dec_pd0(input: *const i32, out: *mut i64);
    fn ref_set_intra_ctrls_via_enc_dec_default(
        intra_level: i32,
        dist_ang_level: i32,
        is_islice: i32,
        out: *mut i64,
    );
    fn ref_pd0_in_slots() -> i32;
    fn ref_pd0_out_slots() -> i32;
}

/// Input slot indices for [`sig_deriv_enc_dec_pd0`], mirroring `PD0_I_*`.
pub mod pd0_in {
    /// `ctx->pd0_ctrls.pd0_level`
    pub const LEVEL: usize = 0;
    /// `pcs->slice_type == I_SLICE`
    pub const IS_ISLICE: usize = 1;
    /// `scs->allintra`
    pub const ALLINTRA: usize = 2;
    /// `scs->static_config.rtc`
    pub const RTC: usize = 3;
    /// `ppcs->update_type` (1 == leaf)
    pub const UPDATE_TYPE: usize = 4;
    /// `pcs->enc_mode`
    pub const ENC_MODE: usize = 5;
    /// `ppcs->transition_present`
    pub const TRANSITION: usize = 6;
    /// `ctx->pic_pred_depth_only`
    pub const PRED_DEPTH_ONLY: usize = 7;
    /// `ctx->hbd_md`
    pub const CTX_HBD: usize = 8;
    /// `pcs->hbd_md`
    pub const PCS_HBD: usize = 9;
    /// `ctx->fast_lambda_md[EB_8_BIT_MD]`
    pub const LAMBDA8: usize = 10;
    /// `ctx->fast_lambda_md[EB_10_BIT_MD]`
    pub const LAMBDA10: usize = 11;
    /// `ppcs->me_64x64_distortion[sb_index]`
    pub const ME64_DIST: usize = 12;
    /// `ppcs->me_8x8_cost_variance[sb_index]`
    pub const ME8_VAR: usize = 13;
    /// `ppcs->me_8x8_distortion[sb_index]`
    pub const ME8_DIST: usize = 14;
    /// `ppcs->frm_hdr.quantization_params.base_q_idx`
    pub const BASE_Q: usize = 15;
    /// `pcs->pd0_cost_bias_weight`
    pub const BIAS_WEIGHT: usize = 16;
    /// `pcs->rate_est_level`
    pub const RATE_EST: usize = 17;
    /// `ctx->disallow_4x4`
    pub const DISALLOW_4X4: usize = 18;
    /// `ctx->disallow_8x8`
    pub const DISALLOW_8X8: usize = 19;
    /// `ctx->depth_removal_ctrls.enabled`
    pub const DR_ENABLED: usize = 20;
    /// `ctx->depth_removal_ctrls.disallow_below_16x16`
    pub const DR_B16: usize = 21;
    /// `ctx->depth_removal_ctrls.disallow_below_32x32`
    pub const DR_B32: usize = 22;
    /// `ctx->depth_removal_ctrls.disallow_below_64x64`
    pub const DR_B64: usize = 23;
    /// `ppcs->b64_geom[sb_index].is_complete_b64`
    pub const B64_COMPLETE: usize = 24;
    /// `scs->super_block_size`
    pub const SB_SIZE: usize = 25;
    /// Number of input slots.
    pub const COUNT: usize = 26;
}

/// Output slot indices for [`sig_deriv_enc_dec_pd0`], mirroring `PD0_O_*`.
pub mod pd0_out {
    /// `ctx->md_disallow_nsq_search`
    pub const NSQ_OFF: usize = 0;
    /// `ctx->shut_fast_rate`
    pub const SHUT_FAST_RATE: usize = 1;
    /// `ctx->depth_early_exit_ctrls.split_cost_th`
    pub const DEE_SPLIT: usize = 2;
    /// `ctx->depth_early_exit_ctrls.early_exit_th`
    pub const DEE_EXIT: usize = 3;
    /// `ctx->parent_cost_bias`
    pub const PARENT_BIAS: usize = 4;
    /// `ctx->pd0_use_src_samples`
    pub const USE_SRC: usize = 5;
    /// `ctx->pf_ctrls.pf_shape`
    pub const PF_SHAPE: usize = 6;
    /// `ctx->subres_ctrls.step`
    pub const SUBRES_STEP: usize = 7;
    /// `ctx->subres_ctrls.odd_to_even_deviation_th`
    pub const SUBRES_DEV: usize = 8;
    /// `ctx->approx_inter_rate`
    pub const APPROX_RATE: usize = 9;
    /// `ctx->uv_ctrls.uv_mode`
    pub const UV_MODE: usize = 10;
    /// First of the five `rate_est_ctrls` slots.
    pub const RATE_EST: usize = 11;
    /// First of the eight `intra_ctrls` slots.
    pub const INTRA_CTRLS: usize = 16;
    /// Number of output slots.
    pub const COUNT: usize = 24;
}

/// C `svt_aom_sig_deriv_enc_dec_pd0` driven on a synthetic SCS/PCS/ctx.
///
/// # Panics
/// If the C shim's slot counts disagree with [`pd0_in::COUNT`] /
/// [`pd0_out::COUNT`].
#[must_use]
pub fn sig_deriv_enc_dec_pd0(input: &[i32; pd0_in::COUNT]) -> [i64; pd0_out::COUNT] {
    assert_eq!(
        unsafe { ref_pd0_in_slots() } as usize,
        pd0_in::COUNT,
        "C shim pd0 input slot count drifted"
    );
    assert_eq!(
        unsafe { ref_pd0_out_slots() } as usize,
        pd0_out::COUNT,
        "C shim pd0 output slot count drifted"
    );
    let mut out = [0i64; pd0_out::COUNT];
    unsafe { ref_sig_deriv_enc_dec_pd0(input.as_ptr(), out.as_mut_ptr()) };
    out
}

/// C's `set_intra_ctrls` at a caller-chosen level, reached through the
/// exported `svt_aom_sig_deriv_enc_dec_default`.
///
/// This exists so a derived `intra_level` can be validated against C without
/// transcribing `set_intra_ctrls` (which this lane has not ported): feed the
/// port's level in here and compare against what the pd0 path produced.
#[must_use]
pub fn set_intra_ctrls_at_level(intra_level: u8, dist_ang_level: u8, is_islice: bool) -> [i64; 8] {
    let mut out = [0i64; 8];
    unsafe {
        ref_set_intra_ctrls_via_enc_dec_default(
            i32::from(intra_level),
            i32::from(dist_ang_level),
            i32::from(is_islice),
            out.as_mut_ptr(),
        );
    }
    out
}

// ---------------------------------------------------------------------------
// svt_aom_sig_deriv_enc_dec_common
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn ref_sig_deriv_enc_dec_common(input: *const i32, out: *mut i64);
    fn ref_common_in_slots() -> i32;
    fn ref_common_out_slots() -> i32;
}

/// Input slot indices for [`sig_deriv_enc_dec_common`], mirroring `CM_I_*`.
pub mod cm_in {
    /// `pcs->enc_mode`
    pub const ENC_MODE: usize = 0;
    /// `scs->static_config.rtc`
    pub const RTC: usize = 1;
    /// `scs->allintra`
    pub const ALLINTRA: usize = 2;
    /// `ppcs->update_type` (1 == leaf)
    pub const UPDATE_TYPE: usize = 3;
    /// `frame_is_boosted(ppcs)`
    pub const IS_BASE: usize = 4;
    /// `pcs->pic_block_based_depth_refinement_level`
    pub const DEPTH_REFINE_LVL: usize = 5;
    /// `ppcs->b64_geom[sb_index].width`
    pub const B64_W: usize = 6;
    /// `ppcs->b64_geom[sb_index].height`
    pub const B64_H: usize = 7;
    /// `pcs->pic_disallow_4x4`
    pub const PIC_DISALLOW_4X4: usize = 8;
    /// `scs->super_block_size`
    pub const SB_SIZE: usize = 9;
    /// `pcs->pic_lpd1_lvl`
    pub const PIC_LPD1_LVL: usize = 10;
    /// `ctx->sb_ptr->qindex`
    pub const SB_QINDEX: usize = 11;
    /// `ppcs->frm_hdr.quantization_params.base_q_idx`
    pub const BASE_Q: usize = 12;
    /// `pcs->slice_type == I_SLICE`
    pub const IS_ISLICE: usize = 13;
    /// `ppcs->me_8x8_cost_variance[sb_index]`
    pub const ME8_VAR: usize = 14;
    /// `ctx->qp_index`
    pub const QP_INDEX: usize = 15;
    /// `scs->static_config.max_tx_size`
    pub const MAX_TX_SIZE: usize = 16;
    /// `pcs->pic_depth_removal_level`
    pub const DR_LEVEL: usize = 17;
    /// `ctx->fast_lambda_md[EB_8_BIT_MD]`
    pub const LAMBDA8: usize = 18;
    /// `ppcs->frm_hdr.delta_q_params.delta_q_present`
    pub const DELTA_Q_PRESENT: usize = 19;
    /// `ppcs->r0_delta_qp_md`
    pub const R0_DELTA_QP: usize = 20;
    /// `quantizer_to_qindex[ppcs->picture_qp]` — supplied to the port; the C
    /// side derives it from `PICTURE_QP` through its own table.
    pub const PIC_QINDEX: usize = 21;
    /// `ppcs->picture_qp`
    pub const PICTURE_QP: usize = 22;
    /// `ppcs->me_64x64_distortion[sb_index]`
    pub const DIST64: usize = 23;
    /// `ppcs->me_32x32_distortion[sb_index]`
    pub const DIST32: usize = 24;
    /// `ppcs->me_16x16_distortion[sb_index]`
    pub const DIST16: usize = 25;
    /// `ppcs->me_8x8_distortion[sb_index]`
    pub const DIST8: usize = 26;
    /// `ppcs->sb_geom[sb_index].width`
    pub const SB_GEOM_W: usize = 27;
    /// `ppcs->sb_geom[sb_index].height`
    pub const SB_GEOM_H: usize = 28;
    /// Whether a same-POC same-size reference is available in both lists.
    pub const REF_AVAIL: usize = 29;
    /// That reference's `sb_min_sq_size[sb_index]`.
    pub const REF_MIN_SQ_SIZE: usize = 30;
    /// `ppcs->variance[sb_index][ME_TIER_ZERO_PU_64x64]`
    pub const SB_VARIANCE: usize = 31;
    /// `scs->qp_based_th_scaling_ctrls.cap_max_size_qp_based_th_scaling`
    pub const CAP_QP_SCALING: usize = 32;
    /// `scs->static_config.qp`
    pub const STATIC_QP: usize = 33;
    /// Number of input slots.
    pub const COUNT: usize = 34;
}

/// Output slot indices for [`sig_deriv_enc_dec_common`], mirroring `CM_O_*`.
pub mod cm_out {
    /// `ctx->depth_refinement_ctrls.mode`
    pub const DEPTH_REFINE_MODE: usize = 0;
    /// `ctx->pred_depth_only`
    pub const PRED_DEPTH_ONLY: usize = 1;
    /// `ctx->pic_pred_depth_only`
    pub const PIC_PRED_DEPTH_ONLY: usize = 2;
    /// `ctx->depth_removal_ctrls.enabled`
    pub const DR_ENABLED: usize = 3;
    /// `ctx->depth_removal_ctrls.disallow_below_64x64`
    pub const DR_B64: usize = 4;
    /// `ctx->depth_removal_ctrls.disallow_below_32x32`
    pub const DR_B32: usize = 5;
    /// `ctx->depth_removal_ctrls.disallow_below_16x16`
    pub const DR_B16: usize = 6;
    /// `ctx->depth_removal_ctrls.disallow_4x4`
    pub const DR_4X4: usize = 7;
    /// `ctx->disallow_8x8`
    pub const DISALLOW_8X8: usize = 8;
    /// `ctx->disallow_4x4`
    pub const DISALLOW_4X4: usize = 9;
    /// `ctx->max_block_size`
    pub const MAX_BLOCK_SIZE: usize = 10;
    /// `ctx->pd1_lvl_refinement`
    pub const PD1_LVL_REFINEMENT: usize = 11;
    /// `ctx->lpd1_ctrls.pd1_level` — the observable proxy for the LPD1 level
    /// this function derives.
    pub const LPD1_PD1_LEVEL: usize = 12;
    /// First slot of `lpd1_ctrls`'s seven per-level rows x nine fields,
    /// row-major: `LPD1_ROWS + row * 9 + field`, field order
    /// `[use_lpd1_detector, use_ref_info, cost_th_dist, cost_th_rate,
    ///   nz_coeff_th, max_mv_length, me_8x8_cost_variance_th,
    ///   skip_pd0_edge_dist_th, skip_pd0_me_shift]`.
    pub const LPD1_ROWS: usize = 13;
    /// Number of output slots.
    pub const COUNT: usize = 13 + 7 * 9;
}

/// C `svt_aom_sig_deriv_enc_dec_common` driven on a synthetic SCS/PCS/ctx.
///
/// # Panics
/// If the C shim's slot counts disagree with [`cm_in::COUNT`] /
/// [`cm_out::COUNT`].
#[must_use]
pub fn sig_deriv_enc_dec_common(input: &[i32; cm_in::COUNT]) -> [i64; cm_out::COUNT] {
    assert_eq!(
        unsafe { ref_common_in_slots() } as usize,
        cm_in::COUNT,
        "C shim common input slot count drifted"
    );
    assert_eq!(
        unsafe { ref_common_out_slots() } as usize,
        cm_out::COUNT,
        "C shim common output slot count drifted"
    );
    let mut out = [0i64; cm_out::COUNT];
    unsafe { ref_sig_deriv_enc_dec_common(input.as_ptr(), out.as_mut_ptr()) };
    out
}

// ---------------------------------------------------------------------------
// svt_aom_sig_deriv_enc_dec_light_pd1_default
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn ref_sig_deriv_light_pd1_default(input: *const i32, out: *mut i64);
    fn ref_light_pd1_in_slots() -> i32;
    fn ref_light_pd1_out_slots() -> i32;
}

/// Input slot indices for [`sig_deriv_light_pd1_default`], mirroring `LP_I_*`.
pub mod lp_in {
    /// `ctx->lpd1_ctrls.pd1_level`
    pub const LPD1_LEVEL: usize = 0;
    /// `pcs->enc_mode`
    pub const ENC_MODE: usize = 1;
    /// `ppcs->input_resolution`
    pub const INPUT_RES: usize = 2;
    /// `pcs->slice_type == B_SLICE`
    pub const IS_B_SLICE: usize = 3;
    /// `ppcs->picture_qp`
    pub const PICTURE_QP: usize = 4;
    /// Whether L0's reference picture exists (drives `is_ref_same_size`).
    pub const REF_L0_AVAIL: usize = 5;
    /// Whether L1's reference picture exists.
    pub const REF_L1_AVAIL: usize = 6;
    /// `ppcs->ref_list1_count_try`
    pub const REF_L1_TRY: usize = 7;
    /// `ppcs->me_8x8_cost_variance[sb_index]`
    pub const ME8_VAR: usize = 8;
    /// `ppcs->me_64x64_distortion[sb_index]`
    pub const ME64_DIST: usize = 9;
    /// L0's `sb_skip[sb_index]`
    pub const L0_SKIP: usize = 10;
    /// L1's `sb_skip[sb_index]`
    pub const L1_SKIP: usize = 11;
    /// L0's `sb_64x64_mvp[sb_index]`
    pub const L0_MVP: usize = 12;
    /// L1's `sb_64x64_mvp[sb_index]`
    pub const L1_MVP: usize = 13;
    /// `pcs->ref_skip_percentage`
    pub const REF_SKIP_PERC: usize = 14;
    /// `pcs->cand_reduction_level`
    pub const CAND_RED: usize = 15;
    /// `pcs->rdoq_level`
    pub const RDOQ: usize = 16;
    /// `pcs->coeff_shaving_level`
    pub const COEFF_SHAVE: usize = 17;
    /// `pcs->me_subpel_level`
    pub const ME_SUBPEL: usize = 18;
    /// `pcs->rate_est_level`
    pub const RATE_EST: usize = 19;
    /// `pcs->approx_inter_rate`
    pub const APPROX_RATE: usize = 20;
    /// `pcs->intra_level`
    pub const INTRA: usize = 21;
    /// `ppcs->ref_list0_count_try`
    pub const REF_L0_TRY: usize = 22;
    /// `ppcs->use_best_me_unipred_cand_only`
    pub const BEST_UNIPRED: usize = 23;
    /// `scs->static_config.rtc`
    pub const RTC: usize = 24;
    /// `ppcs->hierarchical_levels`
    pub const HIER_LEVELS: usize = 25;
    /// `ppcs->update_type` (1 == leaf)
    pub const UPDATE_TYPE: usize = 26;
    /// Number of input slots.
    pub const COUNT: usize = 27;
}

/// Output slot indices for [`sig_deriv_light_pd1_default`], mirroring `LP_O_*`.
pub mod lp_out {
    /// `ctx->lpd1_globalmv_bypass_th`
    pub const GLOBALMV_TH: usize = 0;
    /// First of the eleven `cand_reduction_ctrls` slots.
    pub const CAND_RED: usize = 1;
    /// First of the four `coeff_shaving_ctrls` slots.
    pub const COEFF_SHAVE: usize = 12;
    /// First of the thirteen `md_subpel_me_ctrls` slots.
    pub const SUBPEL_ME: usize = 16;
    /// First of the three `lpd1_tx_skip_decision_ctrls` slots.
    pub const TX_SKIP: usize = 29;
    /// First of the four `lpd1_tx_ctrls` slots.
    pub const LPD1_TX: usize = 32;
    /// `ctx->lpd1_blk_skip_luma_rd_pct`
    pub const BLK_SKIP_LUMA_PCT: usize = 36;
    /// `ctx->lpd1_chroma_skip_energy_th`
    pub const CHROMA_SKIP_ENERGY: usize = 37;
    /// First of the five `rate_est_ctrls` slots.
    pub const RATE_EST: usize = 38;
    /// `ctx->approx_inter_rate`
    pub const APPROX_RATE: usize = 43;
    /// `ctx->pf_ctrls.pf_shape`
    pub const PF_SHAPE: usize = 44;
    /// `ctx->shut_fast_rate`
    pub const SHUT_FAST_RATE: usize = 45;
    /// `ctx->uv_ctrls.enabled`
    pub const UV_EN: usize = 46;
    /// `ctx->uv_ctrls.uv_mode`
    pub const UV_MODE: usize = 47;
    /// `ctx->md_disallow_nsq_search`
    pub const NSQ_OFF: usize = 48;
    /// `ctx->new_nearest_injection`
    pub const NN_INJ: usize = 49;
    /// `ctx->blk_skip_decision`
    pub const BLK_SKIP_DEC: usize = 50;
    /// `ctx->subres_ctrls.odd_to_even_deviation_th`
    pub const SUBRES_DEV: usize = 51;
    /// First of the four `inter_intra_comp_ctrls` slots.
    pub const INTER_INTRA: usize = 52;
    /// First of the eight `intra_ctrls` slots.
    pub const INTRA_CTRLS: usize = 56;
    /// Number of output slots.
    pub const COUNT: usize = 64;
}

/// C `svt_aom_sig_deriv_enc_dec_light_pd1_default` driven on a synthetic
/// SCS/PCS/ctx.
///
/// # Panics
/// If the C shim's slot counts disagree with [`lp_in::COUNT`] /
/// [`lp_out::COUNT`].
#[must_use]
pub fn sig_deriv_light_pd1_default(input: &[i32; lp_in::COUNT]) -> [i64; lp_out::COUNT] {
    assert_eq!(
        unsafe { ref_light_pd1_in_slots() } as usize,
        lp_in::COUNT,
        "C shim light-pd1 input slot count drifted"
    );
    assert_eq!(
        unsafe { ref_light_pd1_out_slots() } as usize,
        lp_out::COUNT,
        "C shim light-pd1 output slot count drifted"
    );
    let mut out = [0i64; lp_out::COUNT];
    unsafe { ref_sig_deriv_light_pd1_default(input.as_ptr(), out.as_mut_ptr()) };
    out
}

// ---------------------------------------------------------------------------
// svt_aom_sig_deriv_multi_processes_default
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn ref_sig_deriv_multi_processes_default(input: *const i32, out: *mut i64);
    fn ref_multi_processes_in_slots() -> i32;
    fn ref_multi_processes_out_slots() -> i32;
}

/// Input slot indices for [`sig_deriv_multi_processes_default`], `MP_I_*`.
pub mod mp_in {
    /// `pcs->enc_mode`
    pub const ENC_MODE: usize = 0;
    /// `pcs->slice_type == I_SLICE`
    pub const IS_ISLICE: usize = 1;
    /// `pcs->temporal_layer_index`
    pub const TEMPORAL_LAYER: usize = 2;
    /// `pcs->input_resolution`
    pub const INPUT_RES: usize = 3;
    /// `scs->static_config.fast_decode`
    pub const FAST_DECODE: usize = 4;
    /// `pcs->sc_class5`
    pub const SC_CLASS5: usize = 5;
    /// `pcs->is_highest_layer`
    pub const IS_HIGHEST_LAYER: usize = 6;
    /// `pcs->tf_ctrls.hme_me_level`
    pub const TF_HME_LEVEL: usize = 7;
    /// `scs->static_config.enable_intrabc`
    pub const ENABLE_INTRABC: usize = 8;
    /// `scs->seq_header.cdef_level`
    pub const SEQ_CDEF_LEVEL: usize = 9;
    /// `scs->static_config.cdef_level`
    pub const CFG_CDEF_LEVEL: usize = 10;
    /// `scs->seq_header.enable_restoration`
    pub const SEQ_ENABLE_RESTORATION: usize = 11;
    /// `scs->max_initial_input_luma_width`
    pub const INIT_LUMA_W: usize = 12;
    /// `scs->max_initial_input_luma_height`
    pub const INIT_LUMA_H: usize = 13;
    /// `scs->encoder_bit_depth`
    pub const ENCODER_BIT_DEPTH: usize = 14;
    /// `scs->static_config.hbd_mds`
    pub const CFG_HBD_MDS: usize = 15;
    /// Number of input slots.
    pub const COUNT: usize = 16;
}

/// Output slot indices for [`sig_deriv_multi_processes_default`], `MP_O_*`.
pub mod mp_out {
    /// First of the sixteen `gm_ctrls` slots.
    pub const GM: usize = 0;
    /// `pcs->enable_hme_flag`
    pub const HME: usize = 16;
    /// `pcs->enable_hme_level0_flag`
    pub const HME_L0: usize = 17;
    /// `pcs->enable_hme_level1_flag`
    pub const HME_L1: usize = 18;
    /// `pcs->enable_hme_level2_flag`
    pub const HME_L2: usize = 19;
    /// `pcs->tf_enable_hme_flag`
    pub const TF_HME: usize = 20;
    /// `pcs->tf_enable_hme_level0_flag`
    pub const TF_HME_L0: usize = 21;
    /// `pcs->tf_enable_hme_level1_flag`
    pub const TF_HME_L1: usize = 22;
    /// `pcs->tf_enable_hme_level2_flag`
    pub const TF_HME_L2: usize = 23;
    /// `pcs->multi_pass_pd_level`
    pub const MULTI_PASS_PD: usize = 24;
    /// `frm_hdr->allow_intrabc`
    pub const ALLOW_INTRABC: usize = 25;
    /// `pcs->palette_level`
    pub const PALETTE_LEVEL: usize = 26;
    /// `frm_hdr->allow_screen_content_tools`
    pub const ALLOW_SC_TOOLS: usize = 27;
    /// `pcs->cdef_level`
    pub const CDEF_LEVEL: usize = 28;
    /// First of the three `cdef_recon_ctrls` slots.
    pub const CDEF_RECON: usize = 29;
    /// First of the ten `sg_filter_ctrls` slots.
    pub const SG: usize = 32;
    /// `pcs->enable_restoration`
    pub const ENABLE_RESTORATION: usize = 42;
    /// `pcs->frame_end_cdf_update_mode`
    pub const FRAME_END_CDF: usize = 43;
    /// `pcs->hbd_md`
    pub const HBD_MD: usize = 44;
    /// `pcs->max_can_count`
    pub const MAX_CAN_COUNT: usize = 45;
    /// `pcs->use_best_me_unipred_cand_only`
    pub const BEST_UNIPRED: usize = 46;
    /// Number of output slots.
    pub const COUNT: usize = 47;
}

/// C `svt_aom_sig_deriv_multi_processes_default` on a synthetic SCS/PPCS.
///
/// # Panics
/// If the C shim's slot counts disagree with [`mp_in::COUNT`] /
/// [`mp_out::COUNT`].
#[must_use]
pub fn sig_deriv_multi_processes_default(input: &[i32; mp_in::COUNT]) -> [i64; mp_out::COUNT] {
    assert_eq!(
        unsafe { ref_multi_processes_in_slots() } as usize,
        mp_in::COUNT,
        "C shim multi-processes input slot count drifted"
    );
    assert_eq!(
        unsafe { ref_multi_processes_out_slots() } as usize,
        mp_out::COUNT,
        "C shim multi-processes output slot count drifted"
    );
    let mut out = [0i64; mp_out::COUNT];
    unsafe { ref_sig_deriv_multi_processes_default(input.as_ptr(), out.as_mut_ptr()) };
    out
}

// ---------------------------------------------------------------------------
// svt_aom_sig_deriv_mode_decision_config_default
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn ref_sig_deriv_md_config_default(input: *const i32, out: *mut i64);
    fn ref_sig_deriv_md_config_allintra(input: *const i32, out: *mut i64);
    fn ref_md_config_in_slots() -> i32;
    fn ref_md_config_out_slots() -> i32;
    fn ref_md_config_allintra_out_slots() -> i32;
}

/// Input slot indices for [`sig_deriv_md_config_default`], mirroring `MD_I_*`.
pub mod md_in {
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
    /// `frm_hdr->error_resilient_mode`
    pub const ERROR_RESILIENT: usize = 12;
    /// `frm_hdr->quantization_params.base_q_idx`
    pub const BASE_Q: usize = 13;
    /// `pcs->ref_hp_percentage`
    pub const REF_HP_PERC: usize = 14;
    /// `scs->input_resolution`
    pub const SCS_INPUT_RES: usize = 15;
    /// `frm_hdr->frame_type` is a key/intra-only frame
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
    /// `pcs->ref_skip_percentage`
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
    /// `frm_hdr->segmentation_params.segmentation_enabled`
    pub const SEGMENTATION: usize = 29;
    /// `scs->super_block_size`
    pub const SB_SIZE: usize = 30;
    /// `ppcs->hbd_md`
    pub const HBD_MD: usize = 31;
    /// `ppcs->r0_gen`
    pub const R0_GEN: usize = 32;
    /// `ppcs->r0`, in THOUSANDTHS (the shim divides by 1000.0).
    pub const R0_MILLI: usize = 33;
    /// `pcs->temporal_layer_index`
    pub const PCS_TEMPORAL_LAYER: usize = 34;
    /// `scs->static_config.tune`
    pub const TUNE: usize = 35;
    /// `ppcs->picture_qp`
    pub const PICTURE_QP: usize = 36;
    /// `scs->static_config.extended_crf_qindex_offset`
    pub const EXT_CRF_OFFSET: usize = 37;
    /// Number of input slots.
    pub const COUNT: usize = 38;
}

/// Number of output slots the md-config dump uses.
pub const MD_OUT_SLOTS: usize = 52;

/// C `svt_aom_sig_deriv_mode_decision_config_default` on a synthetic PCS.
///
/// The output layout mirrors the C shim's `MD_O_*` enum; the test carries the
/// indices and the slot counts are cross-checked here.
///
/// # Panics
/// If the C shim's slot counts disagree with [`md_in::COUNT`] /
/// [`MD_OUT_SLOTS`].
#[must_use]
pub fn sig_deriv_md_config_default(input: &[i32; md_in::COUNT]) -> [i64; MD_OUT_SLOTS] {
    assert_eq!(
        unsafe { ref_md_config_in_slots() } as usize,
        md_in::COUNT,
        "C shim md-config input slot count drifted"
    );
    assert_eq!(
        unsafe { ref_md_config_out_slots() } as usize,
        MD_OUT_SLOTS,
        "C shim md-config output slot count drifted"
    );
    let mut out = [0i64; MD_OUT_SLOTS];
    unsafe { ref_sig_deriv_md_config_default(input.as_ptr(), out.as_mut_ptr()) };
    out
}

/// Output slot indices for [`sig_deriv_md_config_allintra`].
///
/// Since 2026-09-01 the allintra shim dumps the SAME `MD_O_*` layout as
/// [`sig_deriv_md_config_default`], so these are indices INTO that layout and
/// the two arms can be diffed slot-for-slot. Only the names the rate-arm
/// differential uses are given here; a caller that wants another field indexes
/// the shared layout directly (the indices are listed in
/// `tests/c_parity_sig_deriv_md_config.rs`).
pub mod md_allintra_out {
    /// `pcs->rdoq_level` (`MD_O_RDOQ`)
    pub const RDOQ: usize = 1;
    /// `pcs->rate_est_level` (`MD_O_RATE_EST`)
    pub const RATE_EST: usize = 3;
    /// `pcs->cdf_ctrl.update_mv` (`MD_O_CDF_MV`)
    pub const CDF_MV: usize = 4;
    /// `pcs->cdf_ctrl.update_se` (`MD_O_CDF_SE`)
    pub const CDF_SE: usize = 5;
    /// `pcs->cdf_ctrl.update_coef` (`MD_O_CDF_COEF`)
    pub const CDF_COEF: usize = 6;
    /// `pcs->cdf_ctrl.enabled` (`MD_O_CDF_EN`)
    pub const CDF_EN: usize = 7;
    /// Number of output slots — the shared `MD_O_*` count.
    pub const COUNT: usize = super::MD_OUT_SLOTS;
}

/// C `svt_aom_sig_deriv_mode_decision_config_allintra` on a synthetic PCS,
/// reading back the full `MD_O_*` slot set — the same layout
/// [`sig_deriv_md_config_default`] returns, so the arms diff slot-for-slot.
///
/// Takes the SAME input array as [`sig_deriv_md_config_default`] so the two
/// arms are driven from one population.
///
/// # Panics
/// If the C shim's slot counts disagree with [`md_in::COUNT`] /
/// [`md_allintra_out::COUNT`].
#[must_use]
pub fn sig_deriv_md_config_allintra(input: &[i32; md_in::COUNT]) -> [i64; MD_OUT_SLOTS] {
    assert_eq!(
        unsafe { ref_md_config_in_slots() } as usize,
        md_in::COUNT,
        "C shim md-config input slot count drifted"
    );
    assert_eq!(
        unsafe { ref_md_config_allintra_out_slots() } as usize,
        md_allintra_out::COUNT,
        "C shim md-config allintra output slot count drifted"
    );
    let mut out = [0i64; MD_OUT_SLOTS];
    unsafe { ref_sig_deriv_md_config_allintra(input.as_ptr(), out.as_mut_ptr()) };
    out
}
