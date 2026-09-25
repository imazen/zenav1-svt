use super::*;

impl EncodePipeline {
    #[inline(always)]
    pub(super) fn resolve_frame_me(
        &mut self,
        display_order: u64,
        is_key: bool,
        sc_arm: crate::sc_detect::ScArm,
        pic_decision: &Option<crate::port_picstruct::PicParams>,
        tpl_in: &mut Option<crate::port_tpl::FrameTplIn>,
        frame_hier: u8,
        w: usize,
        h: usize,
        sc_derivation: crate::sc_detect::ScDerivation,
        pa_cur: &Option<alloc::boxed::Box<crate::inter_me_arm::PaPicture>>,
    ) -> Option<crate::inter_me_arm::FrameMe> {
        let frame_me = match tpl_in.as_mut().and_then(|f| f.frame_me.take()) {
            me @ Some(_) => me,
            None => match (is_key, pa_cur.as_deref(), pic_decision.as_ref()) {
                (false, Some(cur), Some(pic)) => {
                    // C `pcs->ref_pa_pic_ptr_array[list][ref]` — EVERY reference
                    // the picture decision offered, resolved to its DPB slot's PA
                    // pyramid (`assign_and_release_pa_refs`, pd_process.c:4990).
                    // This used to feed only `pa_ref` — the PREVIOUS frame — to
                    // both lists, which is the [1,1] shape frame 1 happens to
                    // produce but leaves a frame with `ref_list0_count_try > 1`
                    // (frame 2 onward on a flat GOP) searching LAST2's MV slot
                    // against LAST's picture.
                    let mut refs = crate::inter_me::context::MeRefs::default();
                    for rt in 1i8..=7 {
                        let (li, ri) = (
                            crate::inter_mvp::get_list_idx(rt),
                            crate::inter_mvp::get_ref_frame_idx(rt),
                        );
                        let slot = pic.rps.ref_dpb_index[usize::from(rt as u8 - 1)] as usize;
                        if let Some(pa) = self.pa_slots.get(slot).and_then(|s| s.as_deref()) {
                            refs.arr[li][ri] = Some(pa.ds_ref());
                        }
                    }
                    #[cfg(feature = "std")]
                    if crate::dbgenv::medbg() {
                        let mut s = alloc::string::String::new();
                        for (i, sl) in self.pa_slots.iter().enumerate() {
                            s.push_str(&alloc::format!(
                                "{i}:{} ",
                                sl.as_deref().map_or(-1, |p| p.picture_number as i64)
                            ));
                        }
                        std::eprintln!(
                            "PASLOTS poc={} dpb={:?} slots=[{s}]",
                            pic.picture_number,
                            pic.rps.ref_dpb_index
                        );
                    }
                    // `me_process.c:212-213` — the counts the picture decision
                    // offered. `MeRefs::get` panics on a hole a search reaches,
                    // so a missing pyramid means no ME rather than a wrong one —
                    // the same shape the `pa_ref == None` arm produced before.
                    let num_to_search = [pic.ref_list0_count_try, pic.ref_list1_count_try];
                    let complete = (0..2).all(|li| {
                        (0..usize::from(num_to_search[li])).all(|ri| refs.arr[li][ri].is_some())
                    });
                    if !complete {
                        None
                    } else {
                        // Recycle the previous frame's result set.
                        // `run_frame_me_into` resets every per-b64 entry to
                        // exactly what `MeB64Output::new` builds and reassigns
                        // every scalar, so this is byte-identical to a fresh
                        // `run_frame_me`.
                        let mut out = self
                            .me_scratch
                            .take()
                            .unwrap_or_else(crate::inter_me_arm::FrameMe::empty);
                        crate::inter_me_arm::run_frame_me_into(
                            &mut out,
                            cur,
                            &refs,
                            num_to_search,
                            crate::inter_me_arm::FrameMeParams {
                                // C `pcs->enc_mode` — post-clamp
                                // (enc_handle.c:4433): the `sig_deriv_me`
                                // ladders branch at M11 (search area, prehme).
                                enc_mode: crate::rate_arm::eff_enc_mode(
                                    sc_arm,
                                    self.speed_config.preset,
                                ),
                                qp: self.rc_config.qp,
                                width: w,
                                height: h,
                                picture_number: display_order,
                                // C `frame_is_boosted(pcs)` (enc_mode_config.h:108)
                                // = `frame_is_kf_gf_arf` = intra-only || ARF || GF
                                // update. A flat low-delay P GOP DOES still emit
                                // GF_UPDATE frames (picture_decision marks the
                                // base of each mini-GOP `SVT_AV1_GF_UPDATE`), so
                                // `sig_deriv_me`'s `is_base ? 1 : 6` arm is live
                                // here — the `96x96 q20 p6` cell's poc4 is one.
                                frame_is_boosted: crate::port_picstruct::frame_is_boosted(pic),
                                hierarchical_levels: frame_hier,
                                // C `me_process.c:214-215` — `pcs->temporal_layer_index`
                                // / `pcs->is_ref`, straight off the picture decision.
                                temporal_layer_index: pic.temporal_layer_index,
                                is_ref: pic.is_ref,
                                sc_class5: u8::from(sc_derivation.classes.sc_class5),
                                // C `scs->mrp_ctrls` — set this frame by
                                // `run_picture_decision` above.
                                only_l_bwd: self.mrp_ctrls.only_l_bwd != 0,
                                safe_limit_nref: self.mrp_ctrls.safe_limit_nref,
                                safe_limit_zz_th: self.mrp_ctrls.safe_limit_zz_th,
                                // C `pcs->similar_brightness_refs` /
                                // `frame_is_leaf(pcs)` — picture decision's
                                // outputs, gating the safe-limit ME arm
                                // (`motion_estimation.c:2231`).
                                similar_brightness_refs: pic.similar_brightness_refs,
                                frame_is_leaf: crate::port_picstruct::frame_is_leaf(
                                    pic.update_type,
                                ),
                            },
                        );
                        Some(out)
                    }
                }
                _ => None,
            },
        };
        frame_me
    }

    #[inline(always)]
    pub(super) fn plan_sb_qindex(
        &self,
        w: usize,
        h: usize,
        encode_input: &Vec<u8>,
        sb_input: &[u8],
        in_stride: usize,
        tpl_adjusted_qp: &mut u8,
        cbr_sb_plan: Option<crate::sb_qindex::SbQindexPlan>,
        base_qindex: &mut u8,
        picture_qp: &mut u8,
    ) -> Result<Option<crate::sb_qindex::SbQindexPlan>, whereat::prelude::At<EncodeError>> {
        let sb_plan = if self.rc_config.mode == crate::rate_control::RcMode::Cbr {
            // C `svt_av1_rc_init_sb_qindex` (rc_aq.c:879-885): under AOM_CBR
            // the cyclic-refresh decision made inside
            // `rc_calc_qindex_rate_control` is the ONLY per-SB plan —
            // variance boost and the TPL arm below are skipped entirely
            // ("mutually exclusive with other AQ modes"). `None` is C's
            // flat arm — every SB takes the frame `base_q_idx` and
            // `delta_q_present` stays 0.
            cbr_sb_plan
        } else if self.hdr.enable_variance_boost {
            let sb_cols_p = w.div_ceil(64);
            let sb_rows_p = h.div_ceil(64);
            // C iterates the per-SB plan `sb_addr < scs->sb_total_count`
            // (rc_aq.c:465 / :233) over `ppcs->variance[sb_addr]` — but that
            // array is the per-B64 map picture analysis fills
            // (pic_analysis_process.c:414). At sb_size 128 sb_total_count is
            // a QUARTER of the b64 count, so C's plan consumes only the
            // first sb_cnt b64 entries (raster order = the frame's top-left
            // quadrant) for every real SB. Mirror the quirk by truncating
            // the b64 map to the real SB count; at sb_size 64 the counts are
            // equal and this is byte-neutral.
            let sb_cnt = w.div_ceil(self.sb_size) * h.div_ceil(self.sb_size);
            let mut vars = svtav1_types::try_with_capacity![sb_cols_p * sb_rows_p]?;
            for r in 0..sb_rows_p {
                for c in 0..sb_cols_p {
                    vars.push(crate::sb_qindex::compute_sb_variances(
                        encode_input,
                        w,
                        w,
                        h,
                        c * 64,
                        r * 64,
                    ));
                }
            }
            vars.truncate(sb_cnt);
            // C has TWO boost paths and they take DIFFERENT variance domains:
            // mainline (rc_aq.c:350/454) reads the INTEGER per-b64 map that
            // picture analysis builds (`pd0::compute_b64_variance`) and leaves
            // the frame base alone; the fork build (rc_aq.c:87/226) reads f64
            // maps, takes a mean, and resignals the recentered base. Feeding
            // the fork kernel on a mainline encode computes the boost in the
            // wrong domain and returns 0 — which is what made mainline tune IQ
            // emit a flat delta-q plan where C emits a real one.
            let plan = if self.hdr.is_fork() {
                crate::sb_qindex::variance_adjust_qp(
                    *base_qindex,
                    &vars,
                    self.hdr.variance_boost_strength,
                    self.hdr.variance_octile,
                    self.hdr.variance_boost_curve,
                    *tpl_adjusted_qp,
                    self.bit_depth,
                )
            } else {
                let ivars: alloc::vec::Vec<crate::pd0::SbVariance> = (0..sb_rows_p)
                    .flat_map(|r| (0..sb_cols_p).map(move |c| (r, c)))
                    .map(|(r, c)| {
                        crate::pd0::compute_b64_variance(sb_input, in_stride, c * 64, r * 64)
                    })
                    .take(sb_cnt)
                    .collect();
                crate::sb_qindex::variance_adjust_qp_mainline(
                    *base_qindex,
                    &ivars,
                    self.hdr.variance_boost_strength,
                    self.hdr.variance_octile,
                    self.hdr.variance_boost_curve,
                    *tpl_adjusted_qp,
                    self.bit_depth,
                )
            };
            *base_qindex = plan.base_qindex;
            // The fork's recentered base moves BOTH: C's variance-boost path
            // resignals `frm_hdr.base_q_idx` before rate control's
            // `picture_qp` update, and the port has always carried the
            // recentre into its single CLI-domain qp. Keep that (identical to
            // the pre-split behaviour whenever the CRF offset is 0, which is
            // every fork cell the gates cover).
            *picture_qp = crate::rate_control::picture_qp_from_qindex(plan.base_qindex);
            *tpl_adjusted_qp = *picture_qp;
            Some(plan)
        } else {
            None
        };
        Ok(sb_plan)
    }

    #[inline(always)]
    pub(super) fn resolve_gm_models(
        &self,
        display_order: u64,
        is_key: bool,
        decided_is_some: bool,
        pic_decision: &Option<crate::port_picstruct::PicParams>,
        temporal_layer: u8,
        w: usize,
        h: usize,
        encode_input: &Vec<u8>,
        frame_me: &Option<crate::inter_me_arm::FrameMe>,
        gm_estimation: Option<crate::port_global_me::GmEstimation>,
    ) -> Option<crate::port_global_me::GmModels> {
        let gm_models = match gm_estimation.as_ref() {
            Some(g) if !g.all_identity() => {
                let me = frame_me.as_ref().expect("gm_estimation implies frame_me");
                let ctrls = crate::port_enc_mode_config::ctrls::set_gm_controls(
                    self.gm_level_for_frame(is_key),
                    crate::port_enc_mode_config::ResolutionRange::from_luma_area(
                        self.width * self.height,
                    ),
                );
                let pa_ref = self.pa_ref.as_deref();
                match (ctrls, pa_ref) {
                    (Some(ctrls), Some(pr)) => {
                        // C's `me_results[b64]` arrays are one allocation per
                        // b64; `MeResultsView` wants them flat across the
                        // picture, which is how C's `pa_me_data` indexes them.
                        let pu = crate::inter_me::context::SQUARE_PU_COUNT;
                        let mut totals: alloc::vec::Vec<u8> =
                            alloc::vec::Vec::with_capacity(me.per_b64.len() * pu);
                        let mut cands: alloc::vec::Vec<crate::port_md::predicates::MeCandidateRef> =
                            alloc::vec::Vec::with_capacity(me.per_b64.len() * pu * me.max_cand);
                        let mut mvs: alloc::vec::Vec<svtav1_types::motion::Mv> =
                            alloc::vec::Vec::with_capacity(me.per_b64.len() * pu * me.max_refs);
                        for b in &me.per_b64 {
                            totals.extend_from_slice(&b.total_me_candidate_index);
                            cands.extend(b.me_candidate_array.iter().map(|c| {
                                crate::port_md::predicates::MeCandidateRef {
                                    direction: c.direction(),
                                    ref_idx_l0: c.ref_idx_l0(),
                                    ref_idx_l1: c.ref_idx_l1(),
                                    ref0_list: c.ref0_list(),
                                    ref1_list: c.ref1_list(),
                                }
                            }));
                            mvs.extend_from_slice(&b.me_mv_array);
                        }
                        let view = crate::port_gm_correspondence::MeResultsView {
                            total_me_candidate_index: &totals,
                            me_candidate_array: &cands,
                            me_mv_array: &mvs,
                            pu_count: pu,
                            max_cand: me.max_cand,
                            max_refs: me.max_refs,
                            max_l0: me.max_l0,
                        };
                        let geom = crate::port_gm_correspondence::GmPictureGeometry {
                            aligned_width: w as u32,
                            aligned_height: h as u32,
                            b64_size: 64,
                            enable_me_8x8: me.enable_me_8x8,
                            enable_me_16x16: me.enable_me_16x16,
                            gm_downsample_level:
                                crate::port_gm_correspondence::GmDownsampleLevel::Full,
                        };
                        // `encode_input` is at stride `w` (the ALIGNED width);
                        // `in_stride` belongs to `sb_input`, the SB-extent
                        // padded twin, and they differ on any frame whose
                        // aligned dims are not a multiple of 64.
                        let src_plane = crate::port_global_me::GmPlane {
                            buf: encode_input,
                            stride: w,
                            width: self.true_width,
                            height: self.true_height,
                        };
                        let rp = &pr.full;
                        let ref_plane = crate::port_global_me::GmPlane {
                            buf: &rp.buf[rp.org..],
                            stride: rp.stride,
                            width: self.true_width,
                            height: self.true_height,
                        };
                        // C `pcs->pa_ref_pic_ptr_array[list][ref]`. The
                        // (list, ref) pair names a REFERENCE FRAME, and the
                        // DPB slot it resolves to is the one the header's
                        // `ref_frame_idx[]` carries: list 0 is
                        // LAST..GOLDEN (entries 0..3) and list 1 is
                        // BWDREF..ALTREF (entries 4..6), which is C's
                        // `get_list_idx` / `get_ref_frame_idx` read backwards.
                        //
                        // Resolved into planes HERE rather than inside the
                        // closure because the closure also borrows `self`
                        // through `true_width`/`true_height`.
                        let mut slot_pics = [0u64; 7];
                        let slot_planes: [Option<crate::port_global_me::GmPlane<'_>>; 7] =
                            core::array::from_fn(|i| {
                                let slot = pic_decision.as_ref()?.rps.ref_dpb_index[i] as usize;
                                let pa = self.pa_slots.get(slot)?.as_ref()?;
                                slot_pics[i] = pa.picture_number;
                                Some(crate::port_global_me::GmPlane {
                                    buf: &pa.full.buf[pa.full.org..],
                                    stride: pa.full.stride,
                                    width: self.true_width,
                                    height: self.true_height,
                                })
                            });
                        if crate::dbgenv::gmdbg() {
                            eprintln!(
                                "GMSLOTS poc={display_order} ref_dpb={:?} pics={:?} pa_ref={:?}",
                                pic_decision.as_ref().map(|p| p.rps.ref_dpb_index),
                                slot_pics,
                                self.pa_ref.as_ref().map(|p| p.picture_number)
                            );
                        }
                        let mut sink = |a: core::fmt::Arguments<'_>| {
                            if crate::dbgenv::gmdbg() {
                                eprintln!("GMSEARCH poc={display_order} {a}");
                            }
                        };
                        crate::port_global_me::global_motion_search(
                            g,
                            &ctrls,
                            &view,
                            &geom,
                            src_plane,
                            &|l, r| {
                                // Every (list, ref) resolves through the
                                // DPB table first: `ref_pa_pic_ptr_array`
                                // names the picture the RPS chose, which is
                                // NOT `pa_ref` once hierarchical reference
                                // selection puts an older frame in the
                                // nearest list-0 slot (measured 2026-09-25:
                                // LD+hl3 poc3's list0 ref0 is a DIFFERENT
                                // picture than the previous frame — warping
                                // pa_ref computed pic_sad 1072640 where C's
                                // own dump reads 1879552, rejecting a
                                // translation model C accepts).
                                //
                                // `pa_ref` remains the (0,0) fallback for
                                // the flat single-reference path, whose
                                // slots are populated only at the presets
                                // where `gm_level` is non-zero.
                                let idx = if l == 0 { r } else { 4 + r };
                                slot_planes.get(idx).copied().flatten().or(
                                    if l == 0 && r == 0 && !decided_is_some {
                                        Some(ref_plane)
                                    } else {
                                        None
                                    },
                                )
                            },
                            // C's `allow_high_precision_mv` argument is
                            // `pcs->frm_hdr.allow_high_precision_mv`
                            // (`global_me.c:270`), and at ME time that field
                            // is STILL ZERO: it is assigned in
                            // `svt_aom_sig_deriv_mode_decision_config`
                            // (md_config_process), which runs AFTER
                            // me_process. MEASURED with `SVT_GMSEARCH_OUT` on
                            // the 33/32-zoom photo 256 cell at q10 and q20 —
                            // two quantizers whose final
                            // `allow_high_precision_mv` differs — C's
                            // `GMCOST` line reads `hp=0` in both. So this is
                            // C's own ordering, not a value to derive.
                            /*allow_high_precision_mv=*/
                            false,
                            temporal_layer,
                            [
                                pic_decision
                                    .as_ref()
                                    .map_or(0, |p| u32::from(p.ref_list0_count_try)),
                                pic_decision
                                    .as_ref()
                                    .map_or(0, |p| u32::from(p.ref_list1_count_try)),
                            ],
                            [
                                pic_decision
                                    .as_ref()
                                    .map_or(0, |p| u32::from(p.ref_list0_count)),
                                pic_decision
                                    .as_ref()
                                    .map_or(0, |p| u32::from(p.ref_list1_count)),
                            ],
                            Some(&mut sink),
                        )
                        .ok()
                    }
                    _ => None,
                }
            }
            _ => Some(crate::port_global_me::GmModels::default()),
        };
        gm_models
    }

    #[inline(always)]
    pub(super) fn build_coding_quant(
        &self,
        stale_vars: &Option<Vec<crate::pd0::SbVariance>>,
        is_key: bool,
        sc_arm: crate::sc_detect::ScArm,
        md_lambda_base_update_type: Option<crate::port_rc_process::FrameUpdateType>,
        md_lambda_factor_update_type: crate::port_rc_process::FrameUpdateType,
        md_alt_lambda_factors: bool,
        lambda_mod_intra: i64,
        w: usize,
        h: usize,
        filter_chroma: bool,
        sb_input: &[u8],
        in_stride: usize,
        tpl_adjusted_qp: u8,
        frame_me: &Option<crate::inter_me_arm::FrameMe>,
        base_qindex: u8,
        picture_qp: u8,
        lw_bump: u32,
        coded_lossless: bool,
    ) -> Option<alloc::sync::Arc<crate::quant::CodingQuantCfg>> {
        let c_quant: Option<alloc::sync::Arc<crate::quant::CodingQuantCfg>> =
            // Task #95 chunk 2: was gated on 64-aligned dims; the padded
            // `sb_input` now lets the per-b64 walk read C's replicated border
            // on partial SBs, so the still/PD0 coding quantizer is built for any
            // 8-aligned key frame. pic_avg_variance averages over the ALIGNED
            // b64 grid (sb_cols x sb_rows), matching C. Full-SB is unchanged.
            if is_key {
                // Superres chunk B.3: C's picture analysis runs BEFORE the
                // superres downscale (pd_process.c:4344), so `pic_avg_variance`
                // is derived from the FULL-RESOLUTION picture. Walk that grid
                // when a superres source was stashed; otherwise this is the
                // unchanged coded-source walk.
                let pic_avg_variance = if let Some(vars) = stale_vars.as_ref() {
                    // Reuse the full-resolution, border-padded statistics.
                    // The tight pre-scaling source cannot serve a full b64
                    // read when either original dimension is partial.
                    (vars.iter().map(|v| u64::from(v.0[0])).sum::<u64>()
                        / vars.len() as u64) as u16
                } else {
                    let mut tot = 0u64;
                    let mut cnt = 0u64;
                    for sy in (0..h).step_by(64) {
                        for sx in (0..w).step_by(64) {
                            tot += u64::from(crate::pd0::compute_b64_variance(
                                sb_input, in_stride, sx, sy,
                            ).0[0]);
                            cnt += 1;
                        }
                    }
                    (tot / cnt) as u16
                };
                let coeff_lvl = crate::quant::derive_intra_coeff_level(
                    pic_avg_variance,
                    tpl_adjusted_qp as u32,
                    w,
                    h,
                );
                // C's per-arm preset clamp (enc_handle.c:4415-4436): allintra
                // above M9 -> M9, video (non-RTC) above M11 -> M11. The still
                // path's `preset.min(9)` is the allintra arm of the same rule
                // (`rate_arm::allintra_flattening_matches_the_ladder` pins it).
                let eff_mode = crate::rate_arm::eff_enc_mode(sc_arm, self.speed_config.preset);
                // Coded-lossless: `perform_rdoq = !svt_av1_is_lossless_segment
                // && ...` (full_loop.c:1756) — RDOQ never runs at qp 0.
                //
                // The VIDEO arm's ladder (`rdoq_level_default`, :8933) is a
                // flat 1 up to M10 and ignores `coeff_lvl` entirely — which is
                // why C can leave `pcs->coeff_lvl` at INVALID_LVL for a
                // video-mode I-slice. The allintra arm (:9904) is the
                // coeff-driven one, and is unchanged here.
                let rdoq_level = if coded_lossless {
                    0
                } else {
                    // ZenEnhancement::DeepSearch (allintra 4:2:0 only —
                    // `filter_chroma` keeps 4:4:4 and mono out of the arm):
                    // the -1 tier's `rdoq_level = 1` — full RDOQ at every
                    // preset, instead of the coeff-driven 0/2/3 ladder
                    // above M5.
                    crate::rate_arm::rdoq_level(
                        sc_arm,
                        if self
                            .enhancements
                            .contains(crate::enhancements::ZenEnhancement::DeepSearch)
                            && matches!(sc_arm, crate::sc_detect::ScArm::Allintra)
                            && filter_chroma
                        {
                            -1
                        } else {
                            eff_mode
                        },
                        coeff_lvl,
                    )
                };
                let lambda = crate::pd0::kf_full_lambda_8bit_tuned(
                    base_qindex,
                    picture_qp as u32,
                    self.hdr.is_fork() && self.hdr.alt_lambda_factors,
                    0,
                    // The frame `lambda_weight`, resolved exactly as C's
                    // allintra block does (enc_mode_config.c:10093-10115):
                    // the tune-IQ curve OR the PSNR ladder, then the
                    // extended-CRF bump. Both key on `picture_qp`.
                    Some(crate::pd0::frame_lambda_weight_for_preset(self.speed_config.preset,
                        picture_qp as u32,
                        self.hdr.tune == crate::tune::TUNE_IQ,
                        lw_bump,
                    )),
                );
                let mut cq = crate::quant::CodingQuantCfg::new(
                    rdoq_level,
                    lambda,
                    base_qindex,
                );
                cq.input_coeff_level = coeff_lvl;
                // C `svt_av1_optimize_b`'s `allintra || rtc` (full_loop.c:1046)
                // — the first index of `PLANE_RD_MULT`. `scs->allintra` is set
                // only for `intra_period_length == 0 || avif` (enc_handle.c:518),
                // which is exactly `ScArm::Allintra` here; `rtc` is never set by
                // this port. Video frames therefore weight CHROMA rate at 20,
                // not 13.
                //
                // Ghost Robot 0c1c4dec ("Remove Luma bias") drops the
                // `[allintra || rtc]` index entirely — the fork's
                // `plane_rd_mult` IS this port's row [1] ({17,13}/{16,10})
                // for every frame, so the arm is always "allintra" under it.
                cq.allintra_rd_mult = matches!(sc_arm, crate::sc_detect::ScArm::Allintra)
                    || self.reference == crate::reference::SvtReference::GhostRobot;
                Some(alloc::sync::Arc::new(cq))
            } else if let Some(me) = frame_me.as_ref() {
                // The INTER frame's coding quantizer (docs/INTER-ENCODE-PLAN.md
                // §1s item 1b). Without it `use_funnel` is false on every frame
                // with a reference and the C-exact MD path is unreachable no
                // matter what the two `ref_*.is_none()` gates say — which is
                // what item 1's measurement could not see.
                //
                // C `derive_inter_coeff_level` (md_config_process.c:650) keys
                // on `ppcs->norm_me_dist`, the MEAN of the open-loop ME's
                // per-b64 8x8 distortion (initial_rc_process.c:718-726) — so
                // the search has to have run, which is why this sits below it.
                let dist: u64 = me.per_b64.iter().map(|o| u64::from(o.me_8x8_distortion)).sum();
                let norm_me_dist = dist / me.per_b64.len().max(1) as u64;
                let coeff_lvl = crate::quant::derive_inter_coeff_level(
                    norm_me_dist,
                    tpl_adjusted_qp as u32,
                    w,
                    h,
                );
                if crate::dbgenv::coeffdbg() {
                    eprintln!(
                        "COEFFDBG nmd={} qp={} w={} h={} -> {:?}",
                        norm_me_dist, tpl_adjusted_qp, w, h, coeff_lvl
                    );
                }
                let eff_mode = crate::rate_arm::eff_enc_mode(sc_arm, self.speed_config.preset);
                // The VIDEO arm's RDOQ ladder (`rdoq_level_default`,
                // enc_mode_config.c:8933) is a flat 1 through M10 and ignores
                // `coeff_lvl` — which is why C can leave a video-mode I-slice
                // at INVALID_LVL. The level is still derived, because the
                // coeff-driven arms above M10 read it.
                let rdoq_level = crate::rate_arm::rdoq_level(sc_arm, eff_mode, coeff_lvl);
                // C `av1_lambda_assign_md` (md_process.c:725) for a non-key
                // frame. The rdmult BASE and the frame-type FACTOR read
                // DIFFERENT update types — `ppcs->update_type` and
                // `update_lambda`'s own `gf_update_type` — and on a flat
                // low-delay P GOP they disagree (LF vs ARF). MEASURED against
                // C's `svt_aom_full_cost_pd0` lambda: 241 378 on
                // `diag 64x64 q40 p8` frame 1, where one update type for both
                // gave 244 792.
                let lambda = crate::pd0::inter_full_lambda_8bit(
                    base_qindex,
                    md_lambda_base_update_type
                        .expect("an inter frame always has a picture decision"),
                    md_lambda_factor_update_type,
                    md_alt_lambda_factors,
                    0,
                    lambda_mod_intra,
                    crate::pd0::frame_lambda_weight_for_preset(self.speed_config.preset,
                        picture_qp as u32,
                        self.hdr.tune == crate::tune::TUNE_IQ,
                        lw_bump,
                    ),
                );
                let mut cq =
                    crate::quant::CodingQuantCfg::new(rdoq_level, lambda, base_qindex);
                // `pcs->coeff_lvl` — the depth-refinement ladder
                // (enc_mode_config.c:9370-9390) and the NSQ-search ladder read
                // it on inter frames.
                cq.input_coeff_level = coeff_lvl;
                // C `svt_av1_optimize_b`'s `allintra || rtc` (full_loop.c:1046):
                // an inter frame is never `allintra`, so chroma rate weighs 20.
                // Ghost Robot 0c1c4dec removes that index — the fork's flat
                // `plane_rd_mult` (this port's row [1]) applies on video too.
                cq.allintra_rd_mult =
                    self.reference == crate::reference::SvtReference::GhostRobot;
                Some(alloc::sync::Arc::new(cq))
            } else {
                None
            };
        c_quant
    }

    #[inline(always)]
    pub(super) fn derive_seq_tools(
        &self,
        is_single_frame: bool,
    ) -> crate::entropy::obu::SeqTools {
        let seq_tools = {
            let mut t = crate::speed_config::seq_tools_for_preset(
                self.speed_config.preset,
                is_single_frame,
                self.width as usize * self.height as usize,
            );
            // Task #91: C derives `use_128x128_superblock` at SH-write time
            // from `sb_size == BLOCK_128X128` (entropy_coding.c:2800). The
            // port's `sb_size` comes from the same rule
            // (sb128_geom::derive_super_block_size), so the bit follows it.
            t.use_128x128_superblock = self.sb_size == 128;
            // Superres chunk B.3: the SH tool bit must agree with what the
            // frame header signals (`SuperresParams::enabled_in_seq`) or the
            // decoder's bit walk desyncs. Off by default -> unchanged bit.
            t.enable_superres = self.superres_denom.is_some();
            // Issue #9 item 5: C writes `static_config.chroma_sample_position`
            // into the 4:2:0 color_config (entropy_coding.c:2743).
            t.chroma_sample_position = self.chroma_sample_position;
            // Inter campaign C1a: the non-reduced header's
            // `initial_display_delay` is `min(hierarchical_levels + 1, 10)`
            // (enc_handle.c:4975-4993). Unread on the still path, where those
            // bits are not written at all.
            t.hierarchical_levels = self.gop.hierarchical_levels;
            // ZenEnhancement::DeepSearch (allintra 4:2:0 — `validate`
            // refuses anything else): the funnel's filter-intra and
            // intra-edge-filter search config evaluates at enc_mode -1,
            // so the two symbol-gating sequence bits must take the -1
            // values or the decoder's bit walk desyncs
            // (`enable_filter_intra`) or its prediction semantics
            // disagree with what MD priced (`enable_intra_edge_filter`).
            // The other preset-derived bits — wn/sg/`enable_restoration`
            // — stay at the caller's preset: they are post-filters the
            // deep-search arm does not touch.
            if self
                .enhancements
                .contains(crate::enhancements::ZenEnhancement::DeepSearch)
                && self.gop.intra_period == 1
                && self.chroma_420
            {
                t.enable_filter_intra =
                    crate::intra_arm::filter_intra_level(crate::sc_detect::ScArm::Allintra, -1)
                        != 0;
                t.enable_intra_edge_filter =
                    crate::intra_arm::intra_edge_filter(crate::sc_detect::ScArm::Allintra, -1);
            }
            // [SVT_HDR_MODE] the fork ALWAYS signals separate_uv_delta_q
            // (its FH writes independent U/V deltas — entropy_coding.c
            // fork block hardcodes both flags true). An `__expert` chroma
            // override with distinct U/V deltas needs it too.
            if self.separate_uv_delta_q() {
                t.separate_uv_delta_q = true;
            }
            if self.hdr.is_fork() {
                // Photon noise signals grain tables per frame.
                t.film_grain_params_present = self.hdr.noise_strength > 0;
            }
            t.film_grain_params_present |= self.film_grain.enabled();
            // enable_intra_edge_filter's C-parity surface is still/420
            // (the C matched config). The mono extension keeps 0: C cannot
            // emit mono, and the mono leaf coder predicts without edge
            // filtering — signaling 0 keeps our recon decoder-exact on
            // that self-consistent surface.
            t.enable_intra_edge_filter &= self.chroma_420;
            // Small-frame implementation limit (enc_settings.c:214-232):
            // when the TRUE source width OR height is < 64, C force-clears
            // enable_restoration_filtering (and aq_mode, already off on the
            // allintra path) BEFORE the SH derivation, so the SH bit is 0.
            // Uses the TRUE (unaligned) dims — a 60x60 frame aligns to
            // 64x64 but still trips this.
            if self.true_width < 64 || self.true_height < 64 {
                t.enable_restoration = false;
            }
            t
        };
        seq_tools
    }
}

impl EncodePipeline {
    #[inline(always)]
    pub(super) fn derive_screen_content(
        &self,
        sc_arm: crate::sc_detect::ScArm,
        w: usize,
        h: usize,
        encode_input: &Vec<u8>,
        sc_preset: i8,
    ) -> crate::sc_detect::ScDerivation {
        // `sc_arm` is bound at frame level above; it matters HERE because C
        // picks a DIFFERENT derivation function per arm of `scs->allintra`.
        // On the video arm the intra-BC ladder is
        // `sig_deriv_multi_processes_default`'s (:2033-2052) instead of the
        // allintra one (:2346-2369) — which is what makes a video-mode
        // screen-content key frame set `frm_hdr->allow_intrabc` at M6, where
        // the still arm leaves it clear.
        let sc_derivation = match self.hdr.screen_content_mode {
            Some(mode @ 0..=1) => {
                // C forces every classification, not just the header flag.
                // Keep the real preset for palette/IntraBC tool selection.
                let forced = mode == 1;
                crate::sc_detect::derive_sc_classes(
                    sc_arm,
                    self.speed_config.preset,
                    crate::sc_detect::ScClasses {
                        sc_class0: forced,
                        sc_class1: forced,
                        sc_class2: forced,
                        sc_class3: forced,
                        sc_class4: forced,
                        sc_class5: forced,
                    },
                )
            }
            _ => crate::sc_detect::derive_sc(sc_arm, sc_preset, encode_input, w, w, h),
        };
        sc_derivation
    }

    #[inline(always)]
    pub(super) fn derive_base_qindex(
        &mut self,
        display_order: u64,
        is_key: bool,
        pic_decision: &Option<crate::port_picstruct::PicParams>,
        frame_hier: u8,
        sc_derivation: crate::sc_detect::ScDerivation,
        tpl_adjusted_qp: u8,
        frame_me: &Option<crate::inter_me_arm::FrameMe>,
        cbr_frame_rc: &mut Option<crate::port_rc_vbr_cbr_state::FrameRc>,
        cbr_sb_plan: &mut Option<crate::sb_qindex::SbQindexPlan>,
    ) -> Result<u8, whereat::prelude::At<EncodeError>> {
        #[allow(unused_mut)]
        let mut base_qindex = if self.rc_config.mode == crate::rate_control::RcMode::Cbr {
            let (q, frame, plan) = self.cbr_frame_qindex(
                pic_decision.as_ref(),
                is_key,
                display_order,
                frame_hier,
                sc_derivation.classes.sc_class1,
                frame_me.as_ref(),
            )?;
            *cbr_frame_rc = Some(frame);
            *cbr_sb_plan = plan;
            q
        } else {
            crate::rate_control::qp_to_qindex_with_offset(
                tpl_adjusted_qp,
                self.rc_config.extended_crf_qindex_offset,
            )
        };
        Ok(base_qindex)
    }

    #[inline(always)]
    pub(super) fn derive_qm_levels(
        &self,
        base_qindex: u8,
        coded_lossless: bool,
        chroma_deltas: crate::chroma_q::ChromaQDeltas,
    ) -> [u8; 3] {
        // [SVT_HDR_MODE] frame QM levels (svt_av1_qm_init,
        // md_config_process.c:249): the linear qindex map (default tune =
        // PSNR in the fork); chroma levels derive from base + the FH
        // chroma AC deltas. [15;3] = QM off (identity).
        // A lossless segment uses identity matrices in the decoder even when
        // using_qmatrix is signaled. C applies nonidentity weights here at QP0
        // and produces wrong decoded samples (SUSPECTED-C-BUGS.md #31). Keep
        // the raw matrix helpers C-exact, but use the decoder's identity rule
        // for lossless MD, quantization, reconstruction and header signaling.
        let qm_levels: [u8; 3] = if self.hdr.enable_qm && !coded_lossless {
            // TUNE_IQ / TUNE_MS_SSIM use the still-image polynomial
            // (svt_av1_qm_init switch, md_config_process.c:255).
            let still = matches!(
                self.hdr.tune,
                crate::tune::TUNE_IQ | crate::tune::TUNE_MS_SSIM
            );
            let lvl = move |q: i32, lo: u8, hi: u8| {
                if still {
                    crate::qm::still_get_qmlevel(q, i32::from(lo), i32::from(hi)) as u8
                } else {
                    crate::qm::aom_get_qmlevel(q, i32::from(lo), i32::from(hi)) as u8
                }
            };
            [
                lvl(
                    i32::from(base_qindex),
                    self.hdr.min_qm_level,
                    self.hdr.max_qm_level,
                ),
                lvl(
                    i32::from(base_qindex) + i32::from(chroma_deltas.u_ac),
                    self.hdr.min_chroma_qm_level,
                    self.hdr.max_chroma_qm_level,
                ),
                lvl(
                    i32::from(base_qindex) + i32::from(chroma_deltas.v_ac),
                    self.hdr.min_chroma_qm_level,
                    self.hdr.max_chroma_qm_level,
                ),
            ]
        } else {
            [15; 3]
        };
        qm_levels
    }
}
