use super::*;

impl EncodePipeline {
    #[inline(always)]
    pub(super) fn collect_dlf_refs(
        &self,
        is_key: bool,
        pic_decision: &Option<crate::port_picstruct::PicParams>,
    ) -> Vec<crate::dlf_arm::RefDlfState> {
        let dlf_refs: alloc::vec::Vec<crate::dlf_arm::RefDlfState> = match pic_decision.as_ref() {
            Some(pic) if !is_key => {
                let mut v = alloc::vec::Vec::new();
                let mut push = |idx: usize| {
                    if let Some(rf) = self.dpb.get(pic.rps.ref_dpb_index[idx] as usize) {
                        v.push(crate::dlf_arm::RefDlfState {
                            filter_level: [
                                i32::from(rf.lf_levels[0]),
                                i32::from(rf.lf_levels[1]),
                                i32::from(rf.lf_levels[2]),
                                i32::from(rf.lf_levels[3]),
                            ],
                            dlf_dist_dev: rf.dlf_dist_dev,
                        });
                    }
                };
                for i in 0..usize::from(pic.ref_list0_count_try) {
                    push(crate::port_picstruct::LAST + i);
                }
                for i in 0..usize::from(pic.ref_list1_count_try) {
                    push(crate::port_picstruct::BWD + i);
                }
                v
            }
            _ => alloc::vec::Vec::new(),
        };
        dlf_refs
    }

    #[inline(always)]
    pub(super) fn pick_lf_levels(
        &self,
        chroma: Option<(&[u8], &[u8])>,
        hbd_source: &Option<HbdSource>,
        hbd_used: &mut bool,
        stop: &almost_enough::StopToken,
        w: usize,
        h: usize,
        filter_chroma: bool,
        encode_input: &Vec<u8>,
        sc_derivation: crate::sc_detect::ScDerivation,
        coded_lossless: bool,
        recon: &Vec<u8>,
        lf_sharp_eff: u8,
        deblock_geom: &crate::deblock::DeblockGeom,
        u_recon: &Vec<u8>,
        v_recon: &Vec<u8>,
        recon10: &Option<(Vec<u16>, Vec<u16>, Vec<u16>)>,
        dlf_ctrls: crate::port_enc_mode_config::ctrls::DlfCtrls,
        dlf_pick_inputs: crate::dlf_arm::DlfPickInputs<'_>,
        dlf_zero_filt_sse: &mut i64,
        dlf_best_filt_sse: &mut i64,
        dlf_full_image_ran: &mut bool,
    ) -> Result<crate::deblock::LfLevels, whereat::prelude::At<EncodeError>> {
        let lf_levels = if sc_derivation.allow_intrabc || coded_lossless {
            crate::deblock::LfLevels::default()
        } else {
            if dlf_ctrls.enabled == 0 {
                // `enable_dlf_flag == 0` or a level-0 ladder entry: C neither
                // picks nor applies, and the header codes zeros.
                crate::deblock::LfLevels::default()
            } else if dlf_ctrls.sb_based_dlf == 0 {
                let (su, sv) = chroma.unwrap_or((&[][..], &[][..]));
                let early_exit_convergence = i32::from(dlf_ctrls.early_exit_convergence);
                let pick = match recon10.as_ref() {
                    // bd10: search on the true 10-bit unfiltered recon
                    // against the true 10-bit source, with the highbd lpf
                    // kernels and `svt_full_distortion_kernel16_bits`
                    // (C `picture_sse_calculations` at is_16bit,
                    // deblocking_filter.c:768).
                    Some((y10, u10, v10)) => {
                        let sh = (self.bit_depth - 8) as u32;
                        let widen = |p: &[u8]| -> Vec<u16> {
                            p.iter().map(|&s| (s as u16) << sh).collect()
                        };
                        // Task #6 chunk 2: the deblock level search compares the
                        // 10-bit recon against the 10-bit SOURCE. With a native
                        // HBD source that is the caller's real u16 (so the low 2
                        // bits participate in the SSE that picks the level);
                        // otherwise the same `u8 << sh` widening as before.
                        let (sy10, su10, sv10) = match hbd_source.as_ref() {
                            Some(hbd) => {
                                *hbd_used = true;
                                (hbd.y.clone(), hbd.u.clone(), hbd.v.clone())
                            }
                            None => (widen(encode_input), widen(su), widen(sv)),
                        };
                        let input = crate::deblock::DlfSearchInput::<u16> {
                            sharpness: lf_sharp_eff,
                            y_src: &sy10,
                            u_src: &su10,
                            v_src: &sv10,
                            y_recon: y10,
                            u_recon: u10,
                            v_recon: v10,
                            width: w,
                            height: h,
                            chroma_420: filter_chroma,
                            geom: deblock_geom,
                            early_exit_convergence,
                            bit_depth: self.bit_depth,
                        };
                        crate::deblock::pick_filter_levels_full_image_with_stop(
                            &input,
                            &dlf_pick_inputs,
                            stop,
                        )?
                    }
                    None => {
                        let input = crate::deblock::DlfSearchInput::<u8> {
                            sharpness: lf_sharp_eff,
                            y_src: encode_input,
                            u_src: su,
                            v_src: sv,
                            y_recon: recon,
                            u_recon: u_recon,
                            v_recon: v_recon,
                            width: w,
                            height: h,
                            chroma_420: filter_chroma,
                            geom: deblock_geom,
                            early_exit_convergence,
                            bit_depth: self.bit_depth,
                        };
                        crate::deblock::pick_filter_levels_full_image_with_stop(
                            &input,
                            &dlf_pick_inputs,
                            stop,
                        )?
                    }
                };
                *dlf_full_image_ran = true;
                *dlf_zero_filt_sse = pick.zero_filt_sse;
                *dlf_best_filt_sse = pick.best_filt_sse;
                pick.levels
            } else {
                // `sb_based_dlf = 1` -> LPF_PICK_FROM_Q
                // (`enc_dec_process.c:3132`). This path computes no SSE, so
                // `dlf_dist_dev` stays -1 and the NEXT frame skips this one
                // rather than reading a zero.
                crate::dlf_arm::pick_filter_level_by_q(&dlf_pick_inputs)
            }
        };
        Ok(lf_levels)
    }

    #[inline(always)]
    pub(super) fn pick_cdef(
        &self,
        chroma: Option<(&[u8], &[u8])>,
        hbd_source: &Option<HbdSource>,
        hbd_used: &mut bool,
        stop: &almost_enough::StopToken,
        w: usize,
        h: usize,
        filter_chroma: bool,
        encode_input: &Vec<u8>,
        sc_derivation: crate::sc_detect::ScDerivation,
        base_qindex: u8,
        coded_lossless: bool,
        recon: &Vec<u8>,
        deblock_geom: &crate::deblock::DeblockGeom,
        u_recon: &Vec<u8>,
        v_recon: &Vec<u8>,
        recon10: &Option<(Vec<u16>, Vec<u16>, Vec<u16>)>,
        cdef_zero_fs_cost_bias: u16,
        cdef_ctrls: crate::port_enc_mode_config::cdef_search::CdefSearchControls,
        cdef_force_off: bool,
    ) -> Result<(crate::cdef::CdefPick, i32), whereat::prelude::At<EncodeError>> {
        let (cdef_params, cdef_dist_dev) = if sc_derivation.allow_intrabc || coded_lossless {
            (
                crate::cdef::CdefPick::single(crate::cdef::CdefFrameParams::default()),
                0,
            )
        } else if cdef_force_off {
            // C `pcs->ppcs->cdef_level = 0` inside
            // `update_cdef_filters_on_ref_info`: no search, no application,
            // and the header codes zero strengths — plus the DAMPING quirk
            // that comes with never calling `finish_cdef_search`
            // (`CdefFrameParams::never_picked`).
            (
                crate::cdef::CdefPick::single(crate::cdef::CdefFrameParams::never_picked()),
                0,
            )
        } else if cdef_ctrls.use_reference_cdef_fs != 0 {
            // The reference-derived prediction REPLACES the search
            // (`md_config_process.c:713-722` / `:750-758`). Damping is still
            // this frame's own `CDEF_DAMPING_FROM_QP` (`enc_cdef.c:1446`) —
            // only the strengths come from the reference. C's
            // `finish_cdef_search` early-returns on `use_reference_cdef_fs`
            // (enc_cdef.c:937-941), so `cdef_dist_dev` keeps -1.
            (
                crate::cdef::CdefPick::single(crate::cdef::CdefFrameParams {
                    damping: 3 + (base_qindex >> 6),
                    y_strength: cdef_ctrls.pred_y_f as u8,
                    uv_strength: cdef_ctrls.pred_uv_f as u8,
                }),
                -1,
            )
        } else {
            // C runs the CDEF pick on EVERY coded frame; this used to be
            // `else if is_key`, and an inter frame fell through to
            // `CdefFrameParams::default()` — damping 3 and zero strengths,
            // which was the ONLY divergence left in the inter frame header
            // (`docs/INTER-ENCODE-PLAN.md` §1q).
            //
            // C splits the strength policy per preset (allintra
            // enc_mode_config.c:3543-3600): presets <= M6 run the CDEF
            // RDO search, >= M7 the use_qp_strength fast path we ported.
            // Of the search, exactly ONE outcome is ported so far: the
            // sb_count == 0 case — every filter block all-skip, e.g.
            // flat content — where finish_cdef_search deterministically
            // signals cdef_bits=0 with zero strengths (see
            // pick_cdef_params_all_skip_search provenance). Search
            // presets with any non-skip filter block keep the qp fast
            // path for now: still self-consistent (signal == apply),
            // but their signaled strengths diverge from C's searched
            // ones (gap 2a, narrowed to the non-all-skip case).
            if cdef_ctrls.enabled != 0 && !cdef_ctrls.use_qp_strength {
                if deblock_geom.cdef_frame_all_skip() {
                    (
                        crate::cdef::CdefPick::single(
                            crate::cdef::pick_cdef_params_all_skip_search(base_qindex),
                        ),
                        // All-zero strengths -> the cdef_process.c:699-702
                        // rule; written 0 directly for clarity.
                        0,
                    )
                } else {
                    // The live-block RDO search (svt_av1_cdef_search +
                    // finish_cdef_search, per-preset candidate sets:
                    // level 2 at M0, 3 at M1-M3, 5 at M4-M5, 7 at M6):
                    // filter the POST-DEBLOCK recon per candidate strength
                    // and RD-pick against the source. The multi-strength
                    // outcome (cdef_bits>0 needs per-SB cdef_idx syntax
                    // the tile writer lacks) falls back to the qp fast
                    // path — self-consistent, documented divergence.
                    let (su, sv) = chroma.unwrap_or((&[][..], &[][..]));
                    let cfg = crate::cdef::cdef_search_cfg_from_ctrls(
                        &cdef_ctrls,
                        cdef_zero_fs_cost_bias,
                    );
                    // bd10: search the TRUE 10-bit post-deblock recon against
                    // the true 10-bit source (C `cdef_seg_search` at
                    // is_16bit). The 10-bit source is `u8 << (bd - 8)` by
                    // construction — the harness writes exactly that .yuv for
                    // both encoders, so widening here is not an approximation.
                    let searched = match recon10.as_ref() {
                        Some((y10, u10, v10)) => {
                            let sh = (self.bit_depth - 8) as u32;
                            let widen = |p: &[u8]| -> Vec<u16> {
                                p.iter().map(|&s| (s as u16) << sh).collect()
                            };
                            // Task #6 chunk 2: real u16 source for the CDEF
                            // strength search's distortion (see the deblock
                            // site); identical widening on every other path.
                            let (sy10, su10, sv10) = match hbd_source.as_ref() {
                                Some(hbd) => {
                                    *hbd_used = true;
                                    (hbd.y.clone(), hbd.u.clone(), hbd.v.clone())
                                }
                                None => (widen(encode_input), widen(su), widen(sv)),
                            };
                            crate::cdef::cdef_search_still_hbd_with_stop(
                                &cfg,
                                y10,
                                u10,
                                v10,
                                &sy10,
                                &su10,
                                &sv10,
                                w,
                                h,
                                filter_chroma,
                                deblock_geom,
                                base_qindex,
                                self.bit_depth,
                                stop,
                            )?
                        }
                        None => crate::cdef::cdef_search_still_with_stop(
                            &cfg,
                            recon,
                            u_recon,
                            v_recon,
                            encode_input,
                            su,
                            sv,
                            w,
                            h,
                            filter_chroma,
                            deblock_geom,
                            base_qindex,
                            stop,
                        )?,
                    };
                    match searched {
                        crate::cdef::CdefSearchPick::Picked {
                            pick: mut p,
                            dist_dev,
                        } => {
                            // [SVT_HDR_MODE] fork cdef-scaling: search-path
                            // only (finish_cdef_search, enc_cdef.c:1444).
                            if self.hdr.is_fork() {
                                crate::cdef::scale_strengths(&mut p, self.hdr.cdef_scaling);
                            }
                            (p, dist_dev)
                        }
                        crate::cdef::CdefSearchPick::AllSkip => (
                            crate::cdef::CdefPick::single(
                                crate::cdef::pick_cdef_params_all_skip_search(base_qindex),
                            ),
                            0,
                        ),
                    }
                }
            } else if cdef_ctrls.use_qp_strength {
                // C's `use_qp_strength` fast path takes the screen-content
                // arm of `svt_pick_cdef_from_qp` when
                // `allintra ? ppcs->sc_class5 : ppcs->sc_class1` is set
                // (enc_cdef.c:913-918) — allintra here, so sc_class5. This
                // is the FRAME-level derivation the frame header is written
                // from (the same `sc_derivation` that gates palette/IBC
                // above), not a tile-local one. Reachable at preset M7
                // exactly under a default config: use_qp_strength needs
                // cdef_search_level == 10 (allintra M7+,
                // enc_mode_config.c:3543-3600) and screen detection is
                // force-disabled at M8+ (enc_handle.c:4641-4651, mirrored by
                // `derive_allintra_sc`'s `preset <= 7` gate); it extends to
                // M8-M13 when a tune forces screen_content_mode = 3.
                // C `finish_cdef_search` early-returns on `use_qp_strength`
                // (enc_cdef.c:912-926), before `cdef_dist_dev` is computed —
                // it keeps the -1 seed.
                (
                    crate::cdef::CdefPick::single(crate::cdef::pick_cdef_params_key_frame(
                        base_qindex,
                        self.bit_depth,
                        sc_derivation.classes.sc_class5,
                    )),
                    -1,
                )
            } else {
                // `cdef_search_level == 0`: CDEF is off for this frame, so
                // neither arm runs and the header codes zero strengths. Only
                // reachable through a level-0 ladder entry, since the
                // IntraBC/lossless suppression is the outer branch above.
                // Same C state as the `cdef_force_off` arm — `cdef_level == 0`
                // means `finish_cdef_search` is never called, so `cdef_damping`
                // keeps its 0 initialisation and the header signals 1.
                (
                    crate::cdef::CdefPick::single(crate::cdef::CdefFrameParams::never_picked()),
                    0,
                )
            }
        };
        Ok((cdef_params, cdef_dist_dev))
    }
}

impl EncodePipeline {
    #[inline(always)]
    pub(super) fn build_dlf_pick_inputs<'a>(
        &self,
        is_key: bool,
        pic_decision: &Option<crate::port_picstruct::PicParams>,
        frame_hier: u8,
        base_qindex: u8,
        dlf_resolution: crate::port_enc_mode_config::ResolutionRange,
        dlf_temporal_layer_index: u8,
        dlf_ctrls: crate::port_enc_mode_config::ctrls::DlfCtrls,
        dlf_refs: &'a [crate::dlf_arm::RefDlfState],
        dlf_avg_me_sad: u32,
    ) -> crate::dlf_arm::DlfPickInputs<'a> {
        let dlf_pick_inputs = crate::dlf_arm::DlfPickInputs {
            ctrls: dlf_ctrls,
            frame_type_is_key: is_key,
            // `pcs->slice_type == I_SLICE`. This port has no intra-only
            // non-key frame, so it equals `is_key`; C reads two fields and so
            // does `DlfPickInputs`.
            is_intra_slice: is_key,
            // `frame_is_boosted` / `frame_is_leaf` come from the picture
            // decision's `update_type`, the same source `cdef_frame_is_boosted`
            // below already uses. A KEY frame is intra-only and KF_UPDATE, so
            // boosted is true and leaf is false either way.
            frame_is_boosted: pic_decision
                .as_ref()
                .map_or(is_key, crate::port_picstruct::frame_is_boosted),
            frame_is_leaf: pic_decision
                .as_ref()
                .is_some_and(|pic| pic.update_type == crate::port_picstruct::FrameUpdateType::Lf),
            hierarchical_levels: frame_hier,
            temporal_layer_index: dlf_temporal_layer_index,
            input_resolution: dlf_resolution,
            refs: &dlf_refs,
            avg_me_sad: dlf_avg_me_sad,
            base_qindex,
            bit_depth: self.bit_depth,
        };
        dlf_pick_inputs
    }

    #[inline(always)]
    pub(super) fn derive_cdef_level(
        &self,
        sc_derivation: crate::sc_detect::ScDerivation,
        is_single_frame: bool,
        dlf_resolution: crate::port_enc_mode_config::ResolutionRange,
        dlf_is_base: bool,
    ) -> u8 {
        let cdef_level = if is_single_frame {
            crate::port_enc_mode_config::cdef_search::cdef_search_level_allintra(
                self.speed_config.preset as i8,
                CDEF_FAST_DECODE,
                dlf_resolution,
                SEQ_CDEF_LEVEL,
                sc_derivation.allow_intrabc,
                crate::port_enc_mode_config::cdef_search::CONFIG_DEFAULT,
            )
        } else {
            // The ladder's own `is_base` is `temporal_layer_index == 0`, which
            // every KEY frame is — NOT the `frame_is_boosted` one the controls
            // table below uses.
            crate::port_enc_mode_config::cdef_search::cdef_search_level_default(
                self.speed_config.preset as i8,
                dlf_is_base,
                SEQ_CDEF_LEVEL,
                sc_derivation.allow_intrabc,
                crate::port_enc_mode_config::cdef_search::CONFIG_DEFAULT,
            )
        };
        cdef_level
    }
}
