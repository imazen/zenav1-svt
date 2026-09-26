use super::*;

impl EncodePipeline {
    #[inline(always)]
    pub(super) fn build_inter_md_frame<'a>(
        preset: i8,
        bit_depth: u8,
        // `self.reference` on the pipeline — selects the Ghost Robot
        // `7c4ada2e5` `hbd_md` bands.
        reference: crate::reference::SvtReference,
        rc_qp: u8,
        use_best_references: u8,
        tx_bias: u8,
        tune: u8,
        alt_ssim_tuning: bool,
        ac_bias: f64,
        pic_decision: &'a Option<crate::port_picstruct::PicParams>,
        md_lambda_base_update_type: Option<crate::port_rc_process::FrameUpdateType>,
        md_lambda_factor_update_type: crate::port_rc_process::FrameUpdateType,
        md_alt_lambda_factors: bool,
        lambda_mod_intra: i64,
        w: usize,
        h: usize,
        sb_input: &'a [u8],
        in_stride: usize,
        sc_derivation: crate::sc_detect::ScDerivation,
        frame_me: &'a Option<crate::inter_me_arm::FrameMe>,
        base_qindex: u8,
        picture_qp: u8,
        delta_q_plan: Option<&'a crate::sb_qindex::SbQindexPlan>,
        primary_ref_cdfs: &'a Option<alloc::sync::Arc<crate::port_frame_cdf::FrameCdfs>>,
        sb_size: usize,
        ref_padded_luma: Option<&'a crate::picture::PaddedRef>,
        md_config_signals: Option<crate::port_enc_mode_config::md_config::MdConfigSignals>,
        gm_field: [svtav1_types::motion::WarpedMotionParams; 8],
        gm_skip_identity: bool,
        gm_enabled: bool,
        inter_cand_reduction: Option<crate::port_enc_mode_config::encdec::CandReductionCtrls>,
        inter_syntax_state: &'a Option<InterSyntaxState>,
        inter_mvp_env: &'a Option<crate::partition::InterMdEnv>,
        inter_padded_by_ref: [Option<&'a crate::picture::PaddedRef>; 8],
        inter_ref_types: &'a [i8],
    ) -> Result<Option<crate::inter_md_arm::InterMdFrame<'a>>, whereat::prelude::At<EncodeError>>
    {
        let inter_md_frame = match (
            frame_me.as_ref(),
            ref_padded_luma,
            inter_syntax_state.as_ref(),
            inter_mvp_env.as_ref(),
            md_config_signals.as_ref(),
            inter_cand_reduction.as_ref(),
        ) {
            (Some(me), Some(padded), Some(st), Some(env), Some(sigs), Some(cand_red)) => {
                // §1s item 8, the inter half: the same `md_frame_context`
                // the intra rate tables are built from.
                let default_fc = crate::entropy::context::FrameContext::new_default();
                let default_ic = crate::port_entropy_inter::InterCdfs::new_default();
                let (fc, ic) = match primary_ref_cdfs.as_deref() {
                    Some(prev) => (&prev.fc, &prev.fc.inter),
                    None => (&default_fc, &default_ic),
                };
                let (fac, ref_fac) = crate::inter_md_arm::build_inter_rates(fc, ic);
                // C `svt_aom_estimate_mv_rate` (md_rate_estimation.c:458-465):
                // under `pcs->approx_inter_rate` the nmv cost tables are
                // memset to zero and `nmvcoststack` repointed at them, so
                // every MV prices at 0 in the motion searches.
                let nmv = if sigs.approx_inter_rate != 0 {
                    crate::intrabc::MvCostTables::zeroed()
                } else {
                    crate::inter_md_arm::nmv_cost_table(
                        &fc.nmvc,
                        crate::inter_mv_code::mv_precision(
                            st.allow_high_precision_mv,
                            st.force_integer_mv,
                        ),
                    )
                };
                let search = crate::inter_search_arm::frame_cfg(
                    &crate::inter_search_arm::SearchFrameInputs {
                        md_pme_level: sigs.md_pme_level,
                        me_subpel_level: sigs.me_subpel_level,
                        pme_subpel_level: sigs.pme_subpel_level,
                        md_nsq_mv_search_level: sigs.md_nsq_mv_search_level,
                        interpolation_search_level: sigs.interpolation_search_level,
                        dist_based_ref_pruning: sigs.dist_based_ref_pruning,
                        cli_qp: u32::from(rc_qp),
                        // `ppcs->picture_qp` — `perform_md_reference_pruning`'s
                        // check-closest threshold; inert while
                        // `check_closest_multiplier` is 0 (level <= 3).
                        picture_qp,
                        // `set_qp_based_th_scaling_ctrls_default`
                        // (enc_handle.c:3812) — 1 at every preset above
                        // `ENC_MR`, which is every preset this port reaches
                        // on the video arm.
                        pme_qp_based_th_scaling: preset > 0,
                        base_q_idx: base_qindex,
                        allow_high_precision_mv: st.allow_high_precision_mv,
                        approx_inter_rate: sigs.approx_inter_rate,
                        pic_width: w as u32,
                        pic_height: h as u32,
                    },
                )
                .ok_or_else(|| {
                    whereat::at!(EncodeError::UnsupportedConfig(
                        "a picture-level MD search level is outside the range its C control \
                         table accepts (crate::inter_search_arm::frame_cfg)",
                    ))
                })?;
                // `sharpness_ctrls.ifs` (enc_handle.c:3279-3285) arms the
                // IFS smooth bias together with `pcs->ppcs->is_noise_level`
                // (enc_inter_prediction.c:2166). `is_noise_level` IS derived
                // (port_picstruct::tf_window_noise + the `:4240` stamps): C's
                // `last_i_noise_levels_log1p_fp16` updates only inside
                // `derive_tf_window_params`, which runs on no LD picture —
                // the flag is 0 for the whole low-delay envelope on both
                // sides — and is real under RA+TF. Every subjective-tune arm
                // (`ifs`, `unipred_bias`, `cdef`, `restoration`) ANDs with
                // it, so a 0 makes all of them inert — except
                // `sharpness_ctrls.rdoq`, which is NOT noise-gated:
                // `(use_sharpness || sharp_tx) && delta_q_present`
                // (full_loop.c:1070). The port's `optimize_b` has no
                // `use_sharpness` term, so a sharpness tune meeting a live
                // delta-q plan diverges; and under is_noise_level = 1 the
                // unipred/cdef/restoration arms are unwired. Refuse the
                // union rather than guess either — the admitted surface is
                // exactly where every arm is provably inert.
                let sharp_tune = crate::tune::sharpness_ifs(tune, alt_ssim_tuning);
                let is_noise = pic_decision.as_ref().is_some_and(|p| p.is_noise_level);
                if sharp_tune && (is_noise || delta_q_plan.is_some()) {
                    return Err(whereat::at!(EncodeError::UnsupportedConfig(
                        "subjective-tune sharpness arms (tune vq / film-grain, or alt-ssim \
                         tuning) on an inter frame are supported only where they are inert: \
                         is_noise_level == 0 (always true on low delay) and no delta-q plan \
                         — this frame sets one, where C's unipred_bias/cdef/restoration or \
                         `use_sharpness` rdoq arms fire and this port does not model them",
                    )));
                }
                Some(crate::inter_md_arm::InterMdFrame {
                    skip_mode_flag: st.skip_mode_flag,
                    skip_mode_ref_frame_idx_0: st.skip_mode_ref_frame_idx_0,
                    skip_mode_ref_frame_idx_1: st.skip_mode_ref_frame_idx_1,
                    cand_reduction: *cand_red,
                    wm_level: sigs.wm_level,
                    bit_depth,
                    padded,
                    padded_by_ref: inter_padded_by_ref,
                    // The SB-EXTENT-padded source, NOT `encode_input` at
                    // stride `w`. C's MD searches read
                    // `input_pic->y_buffer + blk_org_y * y_stride + blk_org_x`
                    // over the BLOCK's full extent, and on a frame whose dims
                    // are not a multiple of 64 a straddling block runs past
                    // the aligned edge into C's replicated border
                    // (`pad_input_picture` + `svt_aom_generate_padding`).
                    // `sb_input` is that buffer and the port already reads PD0's
                    // b64 variance and every straddling leaf's residual out of
                    // it; wiring the inter search to the unpadded plane instead
                    // was an out-of-bounds READ, not a different number.
                    //
                    // MEASURED 2026-09-02: the port PANICKED at
                    // `port_md/md_search.rs`'s source gather ("the len is 5184
                    // but the index is 5184", 5184 = 72*72) on 18 of the 96
                    // grid cells — every 72x72 cell of uniform, diag and screen
                    // content. For a 64-aligned frame `sb_input == encode_input`
                    // and `in_stride == w`, so this is byte-neutral on the other
                    // 72 cells by construction.
                    src: sb_input,
                    src_stride: in_stride,
                    ref_frame_type_arr: &inter_ref_types,
                    search,
                    // The `md_subpel`-shape view of the same nmv storage —
                    // `nmv`'s `approx_inter_rate` zero arm applies here too.
                    search_tables: if sigs.approx_inter_rate != 0 {
                        crate::intrabc::MvCostTables::zeroed()
                    } else {
                        crate::intrabc::build_nmv_cost_table(
                            &fc.nmvc,
                            crate::inter_mv_code::mv_precision(
                                st.allow_high_precision_mv,
                                st.force_integer_mv,
                            ),
                        )
                    },
                    me,
                    fac,
                    ref_fac,
                    nmv,
                    interpolation_filter: st.interpolation_filter,
                    is_motion_mode_switchable: st.is_motion_mode_switchable,
                    allow_warped_motion: st.allow_warped_motion,
                    force_integer_mv: st.force_integer_mv,
                    allow_high_precision_mv: st.allow_high_precision_mv,
                    enable_dual_filter: st.enable_dual_filter,
                    enable_masked_compound: st.enable_masked_compound,
                    enable_jnt_comp: st.enable_jnt_comp,
                    enable_interintra_compound: st.enable_interintra_compound,
                    reference_mode_is_select: matches!(
                        st.reference_mode,
                        crate::port_entropy_inter::refframe::ReferenceMode::Select
                    ),
                    allow_screen_content_tools: sc_derivation.allow_screen_content_tools,
                    order_hint: crate::inter_md_arm::OrderHints {
                        enable_order_hint: st.enable_order_hint,
                        order_hint_bits: st.order_hint_bits,
                        cur_order_hint: st.cur_order_hint,
                        ref_order_hint: st.ref_order_hint,
                    },
                    mvp_env: env.mvp_env(),
                    mi_rows: env.mi_rows,
                    mi_cols: env.mi_cols,
                    tile: env.tile,
                    sb_mi_size: env.sb_mi_size,
                    frame_w: w,
                    frame_h: h,
                    sb_size,
                    gm_wmtype: st.gm_wmtype,
                    global_motion: gm_field,
                    gm_skip_identity,
                    gm_enabled,
                    // C `pcs->inter_compound_mode` — same signal derivation
                    // as `pic_obmc_level` below.
                    inter_compound_mode: md_config_signals
                        .as_ref()
                        .map_or(0, |sigs| sigs.inter_compound_mode),
                    // C `pcs->bipred3x3_injection` — the same
                    // `sig_deriv_mode_decision_config_default` signal
                    // (md_config's `bipred3x3_injection`).
                    bipred3x3_injection: md_config_signals
                        .as_ref()
                        .map_or(0, |sigs| sigs.bipred3x3_injection),
                    // C `pcs->inter_intra_level` — `svt_aom_get_inter_
                    // intra_level`'s ladder, the input to
                    // `set_inter_intra_ctrls` at injection.
                    inter_intra_level: md_config_signals
                        .as_ref()
                        .map_or(0, |sigs| sigs.inter_intra_level),
                    // C `pcs->hbd_md` (`sig_deriv_multi_processes_default`,
                    // enc_mode_config.c:2151-2164): mainline/hybrid are
                    // bd10 && preset<=MR → 1, bd10 && preset<=M5 → 2
                    // (`is_base` — a flat GOP makes every frame base), else
                    // 0 on an inter frame. Ghost Robot's `7c4ada2e5`
                    // re-banded it: full 10-bit MD through preset 5, then a
                    // temporal-layer ladder (<=M8 needs TL<=2, <=M9 needs
                    // TL<=1). The `hbd_mds` CLI override has no port input —
                    // it is DEFAULT here. This is the depth the inter-intra
                    // and masked-compound searches run at; the u16 arms are
                    // live wherever the ladder returns non-zero.
                    hbd_md: match bit_depth {
                        10 if reference == crate::reference::SvtReference::GhostRobot => {
                            let tl = pic_decision.as_ref().map_or(0, |p| p.temporal_layer_index);
                            if preset <= 5 {
                                1
                            } else if preset <= 8 && tl <= 2 {
                                1
                            } else if preset <= 9 && tl <= 1 {
                                1
                            } else {
                                0
                            }
                        }
                        10 if preset <= -1 => 1,
                        10 if preset <= 5 => 2,
                        _ => 0,
                    },
                    // The frame-owned mask tables C keeps file-scope
                    // (`init_ii_masks` / `svt_av1_init_wedge_masks`): the
                    // smooth inter-intra blends and the master wedge
                    // table both searches and the masked compound arm
                    // read.
                    ii_masks: svtav1_dsp::port_interintra::IiMasks::new(),
                    wedge_masks: svtav1_dsp::port_wedge_masks::WedgeMasks::new(),
                    // C `ppcs->pic_obmc_level`, straight off the mode-decision
                    // signal derivation that already computes it.
                    pic_obmc_level: md_config_signals
                        .as_ref()
                        .map_or(0, |sigs| sigs.pic_obmc_level),
                    ifs: crate::inter_md_arm::IfsFrameKnobs {
                        // `sharpness_ctrls.ifs && is_noise_level` — the
                        // refusal above admits only `is_noise_level == 0`
                        // frames under a sharpness tune, so this is 0 for
                        // exactly the same reason C's gate is.
                        smooth_bias: sharp_tune && is_noise,
                        tx_bias: tx_bias > 0,
                        // C `ppcs->picture_qp` — the index into
                        // `ifs_smooth_bias` (enc_inter_prediction.c:2171);
                        // the RC-derived value, not the CLI qp.
                        picture_qp,
                        // Inter picture, temporal layer 0 (hier_levels 0).
                        ac_bias_eff: svtav1_dsp::ac_bias::effective_ac_bias(ac_bias, false, 0),
                    },
                    base_update_type: md_lambda_base_update_type
                        .expect("an inter frame always has a picture decision"),
                    factor_update_type: md_lambda_factor_update_type,
                    alt_lambda_factors: md_alt_lambda_factors,
                    lambda_mod_intra,
                    // C `scs->mrp_ctrls.use_best_references` + the
                    // `determine_best_references` inputs — the per-block
                    // `ctx->ref_frame_type_arr` rebuild gate.
                    use_best_references,
                    temporal_layer_index: pic_decision
                        .as_ref()
                        .map_or(0, |p| p.temporal_layer_index),
                    ref_list0_count_try: pic_decision
                        .as_ref()
                        .is_some_and(|p| p.ref_list0_count_try > 0),
                    ref_list1_count_try: pic_decision
                        .as_ref()
                        .is_some_and(|p| p.ref_list1_count_try > 0),
                    sframe_ref_pruned: pic_decision.as_ref().is_some_and(|p| p.sframe_ref_pruned),
                    // C `pcs->ppcs->max_can_count` —
                    // `svt_aom_get_max_can_count(enc_mode, rtc)`; the rtc
                    // half is false on every config this port accepts
                    // (the same `false` `sig_deriv_multi_processes`
                    // passes at multi_processes.rs).
                    max_can_count: crate::port_enc_mode_config::leaf::get_max_can_count(
                        i8::try_from(preset).unwrap_or(i8::MAX),
                        false,
                        reference,
                    ),
                })
            }
            _ => None,
        };
        Ok(inter_md_frame)
    }
}
