use super::*;

impl EncodePipeline {
    #[inline(always)]
    pub(super) fn apply_tpl_qp(
        &self,
        is_key: bool,
        pic_decision: &Option<crate::port_picstruct::PicParams>,
        temporal_layer: u8,
        frame_hier: u8,
        md_ref_intra_percentage: u8,
        sc_derivation: crate::sc_detect::ScDerivation,
        base_qindex: &mut u8,
        allintra: bool,
        tpl_fti: Option<&crate::port_tpl::FrameTplIn>,
        tpl_r0: &mut f64,
    ) {
        if !allintra && self.rc_config.mode != crate::rate_control::RcMode::Cbr {
            let new_qindex: i32 = if let Some(fti) = tpl_fti {
                // `rc->active_worst_quality` — `scs_qindex` forever in the
                // 1-pass envelope: `svt_av1_rc_init` seeds it at
                // `scs_qindex` (rc_crf_cqp.c:486-488) and the ONLY writer
                // past that point is `svt_aom_crf_assign_max_rate`, the
                // `max_bit_rate` arm that is refused upstream.
                let pic = pic_decision.as_ref();
                let ref0 = pic.and_then(|p| self.dpb.get(p.rps.ref_dpb_index[0] as usize));
                let ref1 = pic.and_then(|p| {
                    (p.slice_type == crate::port_picstruct::SliceType::B
                        && p.ref_list1_count_try != 0)
                        .then(|| self.dpb.get(p.rps.ref_dpb_index[4] as usize))
                        .flatten()
                });
                let crf_out = crate::rate_control::crf_qindex_calc(
                    i32::from(*base_qindex),
                    &crate::rate_control::CrfQindexInputs {
                        is_intra_only: is_key,
                        temporal_layer_index: temporal_layer,
                        hierarchical_levels: frame_hier,
                        is_highest_layer: crate::port_picstruct::is_highest_layer(
                            temporal_layer,
                            frame_hier,
                        ),
                        r0_qps: fti.flags.r0_qps,
                        r0: fti.r0,
                        r0_adjust_factor: fti.tpl_ctrls.r0_adjust_factor,
                        used_tpl_frame_num: fti.used_tpl_frame_num,
                        tpl_group_size: fti.tpl_group_size,
                        // `scs->lad_mg != 0` — C's CONFIG value
                        // (enc_handle.c:4041-4065), `= scs->tpl_lad_mg`
                        // under CQP_OR_CRF: 0 for allintra/LOW_DELAY, else
                        // 1 whenever TPL is in play (the RA window this
                        // pipeline buffers is a full mini-GOP, so C's
                        // `look_ahead < mg_size` test is false). It gates
                        // the base-layer weight bump `weight = min(w+0.1,1)`
                        // (rc_crf_cqp.c:285-287) — with it off, a base
                        // frame's `r0_weight[1]=0.9` never bumps to 1.0
                        // and `qstep_ratio`/`base_q_idx` come out low
                        // (johnny 9f p6: poc8 port 121 vs C 128).
                        scs_lad_mg: !allintra
                            && self.pred_structure
                                != crate::port_picstruct::PredStructure::LowDelay,
                        input_resolution: i32::from(
                            crate::port_enc_mode_config::ResolutionRange::from_luma_area(
                                self.width * self.height,
                            ) as u8,
                        ),
                        bit_depth: self.bit_depth,
                        sc_class1: sc_derivation.classes.sc_class1,
                        // `SVT_QP_SCALE_WEIGHT`/`_ON` (definitions.h:245-253)
                        // — mainline's 4-entry table vs the fork's linear
                        // formula on `hdr.qp_scale_compress_strength`.
                        qp_scale_weight: if self.hdr.is_fork() {
                            1.0 + self.hdr.qp_scale_compress_strength * 0.125
                        } else {
                            crate::rate_control::QP_SCALE_COMPRESS_WEIGHT
                                [self.hdr.qp_scale_compress_strength.clamp(0.0, 3.0) as usize]
                        },
                        // Mainline's field is a `uint8_t` index — sub-1.0
                        // values truncate to 0, i.e. OFF, exactly like C.
                        qp_scale_on: if self.hdr.is_fork() {
                            self.hdr.qp_scale_compress_strength > 0.0
                        } else {
                            self.hdr.qp_scale_compress_strength >= 1.0
                        },
                        // `rc->best_quality`/`worst_quality` =
                        // `quantizer_to_qindex[min/max_qp_allowed]` — C's
                        // defaults are 0..63, i.e. the full u8 range.
                        best_quality: 0,
                        worst_quality: 255,
                        // `rc->arf_q` = `ref_base_q_idx[L0][0]`, max'd
                        // with L1[0] for a B slice with list1 refs
                        // (rc_crf_cqp.c:200-204).
                        arf_q: ref0
                            .map_or(i32::from(*base_qindex), |r| i32::from(r.base_q_idx))
                            .max(ref1.map_or(0, |r| i32::from(r.base_q_idx))),
                        ref0_tmp_layer: ref0.map_or(0, |r| r.temporal_layer),
                        ref1_tmp_layer: ref1.map(|r| r.temporal_layer),
                        ref_intra_percentage: i32::from(md_ref_intra_percentage),
                    },
                );
                *tpl_r0 = crf_out.r0;
                crf_out.qindex
            } else {
                // rc_crf_cqp.c:439-444 — the LOW_DELAY non-base boost reads
                // the L0 reference's per-SB intra counts (`get_ref_obj(pcs,
                // REF_LIST_0, 0)` == the picture `ref_dpb_index[LAST]` names).
                // `None` whenever there is no picture decision or this is a
                // base-layer/key frame — the arm is gated on
                // `temporal_layer_index != 0` inside `cqp_qindex_calc` too.
                // C gates the boost on `scs->static_config.pred_structure ==
                // LOW_DELAY` (rc_crf_cqp.c:439) — the SEQUENCE's configured
                // structure. A cut-short RA mini-GOP flips its pictures to
                // `pred_struct_type LowDelay` but the sequence stays
                // RANDOM_ACCESS, so the boost must not fire under RA even
                // though the picture's own pred-struct type says low-delay.
                let ld_boost = (self.pred_structure
                    == crate::port_picstruct::PredStructure::LowDelay)
                    .then(|| {
                        pic_decision.as_ref().and_then(|p| {
                            let rf = self.dpb.get(p.rps.ref_dpb_index[0] as usize)?;
                            Some(crate::rate_control::non_base_boost(
                                rf.is_islice,
                                &rf.sb_intra,
                            ))
                        })
                    })
                    .flatten();
                crate::rate_control::cqp_qindex_calc(
                    i32::from(*base_qindex),
                    allintra,
                    /*slice_is_intra=*/ is_key,
                    /*is_ref=*/ pic_decision.as_ref().is_none_or(|p| p.is_ref),
                    /*idr_flag=*/ is_key,
                    temporal_layer,
                    frame_hier,
                    self.bit_depth,
                    ld_boost,
                )
            };
            *base_qindex = new_qindex.clamp(0, 255) as u8;
            // C's extended-CRF arm (rc_crf_cqp.c:510-513) applies to the
            // POST-dispatch qindex whenever `qp == 63` and the offset is
            // nonzero — outside the `enable_qp_scaling_flag` gate, so it
            // covers the TPL arm's output too.
            if self.rc_config.qp == 63 && self.rc_config.extended_crf_qindex_offset != 0 {
                let off = i32::from(self.rc_config.extended_crf_qindex_offset);
                *base_qindex = (i32::from(*base_qindex)
                    + (255 - i32::from(*base_qindex)) * off / 56)
                    .clamp(0, 255) as u8;
            }
        }
    }

    #[inline(always)]
    pub(super) fn resolve_primary_ref_cdfs(
        &self,
        is_key: bool,
        pic_decision: &Option<crate::port_picstruct::PicParams>,
        base_qindex: u8,
    ) -> Result<
        (
            u8,
            Option<alloc::sync::Arc<crate::port_frame_cdf::FrameCdfs>>,
        ),
        Result<Vec<u8>, whereat::prelude::At<EncodeError>>,
    > {
        let (primary_ref_frame_for_cdf, primary_ref_cdfs) = if is_key {
            (crate::port_picstruct::PRIMARY_REF_NONE, None)
        } else if let Some(pic) = pic_decision.as_ref() {
            let ref_queue =
                crate::inter_hdr_arm::ref_queue_from_dpb(&pic.ref_queue_dpb, base_qindex);
            let b = crate::port_picstruct::bind_refs_and_primary_ref_frame(
                pic, &ref_queue, /*frame_end_cdf_update_mode=*/ true,
                /*is_s_frame=*/ false,
            );
            let prf = b.primary_ref_frame;
            let cdfs = if prf == crate::port_picstruct::PRIMARY_REF_NONE {
                None
            } else {
                let slot = pic.rps.ref_dpb_index[prf as usize] as usize;
                let stored = self.dpb.get(slot).and_then(|rf| rf.frame_cdfs.clone());
                if stored.is_none() {
                    // REFUSE rather than fall back to the defaults. The header
                    // this frame is about to write says "start from slot N's
                    // end-of-frame CDFs"; coding against the defaults instead
                    // produces a stream a conforming decoder turns into
                    // garbage, which is the one failure mode `docs/WORKING-ON-
                    // THIS.md` §6 exists to forbid. It is also the POSITIVE
                    // CONTROL for this wiring: if the store ever stops
                    // running, every inter cell fails loudly instead of
                    // quietly regressing to default-CDF bytes.
                    return Err(Err(whereat::at!(EncodeError::UnsupportedConfig(
                        "the frame header names a primary_ref_frame, but the DPB slot it \
                         resolves to carries no saved CDF state — the referenced frame's \
                         entropy walk never ran (crate::port_frame_cdf)",
                    ))));
                }
                stored
            };
            (prf, cdfs)
        } else {
            (crate::port_picstruct::PRIMARY_REF_NONE, None)
        };
        Ok((primary_ref_frame_for_cdf, primary_ref_cdfs))
    }

    #[inline(always)]
    pub(super) fn estimate_gm(
        &self,
        is_key: bool,
        pic_decision: &Option<crate::port_picstruct::PicParams>,
        temporal_layer: u8,
        frame_me: &Option<crate::inter_me_arm::FrameMe>,
    ) -> Option<crate::port_global_me::GmEstimation> {
        let gm_estimation = frame_me.as_ref().and_then(|me| {
            let gm_level = self.gm_level_for_frame(is_key);
            let ctrls = crate::port_enc_mode_config::ctrls::set_gm_controls(
                gm_level,
                crate::port_enc_mode_config::ResolutionRange::from_luma_area(
                    self.width * self.height,
                ),
            )?;
            let dist: alloc::vec::Vec<u32> =
                me.per_b64.iter().map(|b| b.rc_me_distortion).collect();
            let allow: alloc::vec::Vec<u8> = me.per_b64.iter().map(|b| b.rc_me_allow_gm).collect();
            crate::port_global_me::global_motion_estimation(
                &crate::port_global_me::GmEstimationInputs {
                    gm_ctrls: ctrls,
                    rc_me_distortion: &dist,
                    rc_me_allow_gm: &allow,
                    input_width: self.true_width,
                    input_height: self.true_height,
                    ref_list0_count_try: pic_decision
                        .as_ref()
                        .map_or(0, |p| u32::from(p.ref_list0_count_try)),
                    ref_list1_count_try: pic_decision
                        .as_ref()
                        .map_or(0, |p| u32::from(p.ref_list1_count_try)),
                    temporal_layer_index: temporal_layer,
                    // `pp_enabled` is false at every level this port can
                    // express, so this is never read; `false` is C's own
                    // initial value.
                    gm_pp_detected: false,
                },
            )
        });
        gm_estimation
    }

    #[inline(always)]
    pub(super) fn derive_ssim_rdmult(
        &self,
        is_key: bool,
        sc_arm: crate::sc_detect::ScArm,
        temporal_layer: u8,
        frame_hier: u8,
        md_lambda_base_update_type: Option<crate::port_rc_process::FrameUpdateType>,
        md_alt_lambda_factors: bool,
        w: usize,
        h: usize,
        encode_input: &Vec<u8>,
        base_qindex: u8,
        r0_delta_qp_md: bool,
        delta_q_present: bool,
    ) -> Option<crate::tune::SsimRdmult> {
        let ssim_rdmult: Option<crate::tune::SsimRdmult> =
            if crate::tune::tune_uses_ssim_rdmult(self.hdr.tune) {
                let (factors, num_cols, num_rows) = crate::tune::ssim_rdmult_factors(
                    encode_input,
                    w,
                    w,
                    h,
                    self.hdr.alt_ssim_tuning,
                );
                let pic_lctx = crate::port_rc_process::LambdaContext {
                    frame_type: i32::from(!is_key),
                    temporal_layer_index: temporal_layer,
                    hierarchical_levels: frame_hier,
                    update_type: md_lambda_base_update_type
                        .unwrap_or(crate::port_rc_process::FrameUpdateType::Kf),
                    alt_lambda_factors: md_alt_lambda_factors,
                    rtc: false,
                    stats_based_sb_lambda_modulation:
                        crate::port_rc_process::stats_based_sb_lambda_modulation(
                            crate::rate_arm::eff_enc_mode(sc_arm, self.speed_config.preset),
                            false,
                        ),
                    base_q_idx: i32::from(base_qindex),
                    // The picture lambda evaluates `update_lambda` at
                    // `q_index == base`, where the qdiff factor is the
                    // identity under every arm — these flags are still the
                    // frame's real ones, not literals.
                    delta_q_present,
                    r0_delta_qp_md,
                    // `scs->static_config.lambda_scale_factors` — the port
                    // does not expose the knob; 128 is the identity C ships.
                    lambda_scale_factors: [128; 7],
                };
                // `svt_aom_lambda_assign(pcs, &fast, &full, bd, base_q_idx,
                // multiply_lambda=true)` at both depths — `(fast, full)`.
                let (pic_fast8, pic_full8) =
                    crate::port_rc_process::lambda_assign(&pic_lctx, 8, base_qindex, true);
                let (_pic_fast10, pic_full10) =
                    crate::port_rc_process::lambda_assign(&pic_lctx, 10, base_qindex, true);
                Some(crate::tune::SsimRdmult {
                    factors,
                    num_cols,
                    num_rows,
                    pic_full8,
                    pic_full10,
                    pic_fast8,
                })
            } else {
                None
            };
        ssim_rdmult
    }

    #[inline(always)]
    pub(super) fn build_md_inputs(
        &self,
        is_key: bool,
        sc_arm: crate::sc_detect::ScArm,
        pic_decision: &Option<crate::port_picstruct::PicParams>,
        temporal_layer: u8,
        frame_hier: u8,
        sc_derivation: crate::sc_detect::ScDerivation,
        base_qindex: u8,
        picture_qp: u8,
        c_quant: &Option<alloc::sync::Arc<crate::quant::CodingQuantCfg>>,
        seq_tools: crate::entropy::obu::SeqTools,
    ) -> Option<crate::inter_hdr_arm::PipelineMdInputs> {
        let pipeline_md_inputs = if is_key {
            None
        } else {
            let pd = pic_decision.as_ref();
            Some(crate::inter_hdr_arm::PipelineMdInputs {
                // C `pcs->enc_mode` — post-clamp (enc_handle.c:4433): the
                // md-config ladders branch at M11 (pic_lpd1_lvl is the
                // observed one — raw p13 took the M12+ `is_base ? 3 : 7`
                // row where C's 11 takes `is_base ? 0 : 7`).
                enc_mode: crate::rate_arm::eff_enc_mode(sc_arm, self.speed_config.preset),
                sq_qp: u32::from(self.rc_config.qp),
                base_q_idx: base_qindex,
                // C `ppcs->picture_qp` — the RC-derived value, NOT the CLI
                // qp: `rc_process.c:901` recomputes it as
                // `(base_q_idx + 2) >> 2` every frame, which is what the
                // pd0/lpd1/me-variance thresholds inside index.
                picture_qp: u32::from(picture_qp),
                temporal_layer_index: temporal_layer,
                hierarchical_levels: frame_hier,
                // C `ppcs->update_type` — the picture decision's
                // `set_frame_update_type` output, which `enc_dec_cand_reduction`
                // reads as `frame_is_leaf` (`enc_mode_config.c:4100`).
                update_type: pd.map(|p| p.update_type),
                is_ref: pd.is_some_and(|p| p.is_ref),
                is_islice: false,
                sc_class5: u8::from(sc_derivation.classes.sc_class5),
                input_resolution: crate::port_enc_mode_config::ResolutionRange::from_luma_area(
                    self.width * self.height,
                ),
                encoder_bit_depth: self.bit_depth,
                super_block_size: self.sb_size as u16,
                enable_interintra_compound: seq_tools.enable_interintra_compound,
                frame_superres_enabled: self.superres_denom.is_some(),
                // C `ppcs->ref_list{0,1}_count_try` — the MRP-CAPPED counts
                // `update_count_try` (pd_process.c:4507) produces, NOT
                // `ref_list{0,1}_count`. This site read the uncapped pair
                // until 2026-09-03; the two agree on every cell this port has
                // encoded (both are `min(found, base_ref_listN_count)` on a
                // base-layer frame) but they diverge the moment `list0_only`
                // or a non-base layer applies, and `ref_list1_count_try` is
                // exactly the field C's own dump reports going 1 -> 0 at
                // frame 2 (`benchmarks/ref_coded_area_stats_2026-09-02.md`).
                ref_list0_count_try: pd.map_or(0, |p| u32::from(p.ref_list0_count_try)),
                ref_list1_count_try: pd.map_or(0, |p| u32::from(p.ref_list1_count_try)),
                // C `pcs->ref_pic_ptr_array[REF_LIST_{0,1}][0]->object_ptr`,
                // through the RPS's DPB indices — the same slots
                // `port_picstruct::bind_refs_and_primary_ref_frame` binds.
                // `None` when that list is empty, which is C's own guard in
                // every `get_ref_*_percentage` reader.
                ref_l0: pd.and_then(|p| {
                    (p.ref_list0_count_try > 0)
                        .then(|| ref_obj_stats(&self.dpb, p.rps.ref_dpb_index[0] as usize))
                        .flatten()
                }),
                ref_l1: pd.and_then(|p| {
                    (p.ref_list1_count_try > 0)
                        .then(|| ref_obj_stats(&self.dpb, p.rps.ref_dpb_index[4] as usize))
                        .flatten()
                }),
                // C `pcs->coeff_lvl` — `derive_inter_coeff_level`'s output
                // (md_config_process.c:650), which runs BEFORE
                // `sig_deriv_mode_decision_config_default` reads it. The
                // coding quantizer above carries it; `Normal` on a `None`
                // matches C's `INVALID_LVL` under every equality check the
                // ladders make (neither `low_coeff` nor `high_coeff`).
                coeff_lvl: c_quant
                    .as_ref()
                    .map_or(crate::port_enc_mode_config::InputCoeffLvl::Normal, |q| {
                        crate::part_arm::input_coeff_lvl(q.input_coeff_level)
                    }),
            })
        };
        pipeline_md_inputs
    }
}
