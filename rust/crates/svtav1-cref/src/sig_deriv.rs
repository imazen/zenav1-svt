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
