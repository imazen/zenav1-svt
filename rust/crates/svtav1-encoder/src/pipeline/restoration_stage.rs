use super::*;

impl EncodePipeline {
    #[inline(always)]
    pub(super) fn search_restoration(
        &self,
        chroma: Option<(&[u8], &[u8])>,
        hbd_source: &Option<HbdSource>,
        hbd_used: &mut bool,
        stop: &almost_enough::StopToken,
        sc_arm: crate::sc_detect::ScArm,
        w: usize,
        fmt: svtav1_types::chroma::ChromaFormat,
        acw: usize,
        filter_chroma: bool,
        encode_input: Vec<u8>,
        sc_derivation: crate::sc_detect::ScDerivation,
        base_qindex: u8,
        coded_lossless: bool,
        recon: &mut Vec<u8>,
        seq_tools: crate::entropy::obu::SeqTools,
        cw: usize,
        lr_true_w: usize,
        lr_true_h: usize,
        u_recon: &mut Vec<u8>,
        v_recon: &mut Vec<u8>,
        recon10: &mut Option<(Vec<u16>, Vec<u16>, Vec<u16>)>,
        dlf_is_not_last_layer: u8,
        recon10_pre_cdef: Option<(Vec<u16>, Vec<u16>, Vec<u16>)>,
        lr_signal: &mut crate::entropy::obu::LrSignal,
        walk_rest_info: &mut Option<crate::restoration::FrameRestInfo>,
        decoder_chroma_recon: bool,
        output_restoration: &mut Option<crate::restoration::FrameRestInfo>,
        last_lr_unit_size: &mut Option<usize>,
        last_lr_stats: &mut ([u8; 3], usize),
    ) -> Result<(), whereat::prelude::At<EncodeError>> {
        Ok(
            if seq_tools.enable_restoration && !sc_derivation.allow_intrabc && !coded_lossless {
                // LOOP-RESTORATION LEVEL LADDERS — the `scs->allintra` fork
                // (`pd_process.c:4935-4938`), the same selector `sc_detect`, the
                // deblock ladder and the rate ladders already take.
                //
                // The all-intra arm is `wn_filter_level_allintra` (3 / 4 / off) with
                // `sg_filter_level_allintra` == 0 at normal presets 0 through 13,
                // which is why the port has only ever run Wiener. The VIDEO arm is
                // `_default`: Wiener 4 at <= M3 and 5 at <= M8 on a non-last layer
                // (level 5 is LUMA-ONLY), and SGR level 3 at <= M3 — so a video-mode
                // key frame at presets 0..3 can emit RESTORE_SGRPROJ and, on a plane
                // with more than one restoration unit, RESTORE_SWITCHABLE.
                //
                // The two arms must move TOGETHER: the video Wiener ladder is
                // nonzero at p7/p8 where the all-intra one is off, so wiring `sg`
                // alone would leave the frame RD comparing an SGR candidate against
                // a Wiener candidate C never searched, and wiring `wn` alone cannot
                // close the p3 cell whose gap is `sg`.
                let lr_enc_mode = crate::rate_arm::eff_enc_mode(sc_arm, self.speed_config.preset);
                // `ppcs->input_resolution`, derived exactly as the deblock ladder
                // above derives it.
                let lr_resolution = crate::port_enc_mode_config::ResolutionRange::from_luma_area(
                    self.true_width.next_multiple_of(8) * self.true_height.next_multiple_of(8),
                )
                .as_u8();
                // `is_not_last_layer = !ppcs->is_highest_layer` — the same value the
                // deblock ladder derived above, reused rather than re-derived so the
                // two cannot drift.
                let lr_is_not_last_layer = dlf_is_not_last_layer != 0;
                let (ctrls, sg_ctrls) = match sc_arm {
                    crate::sc_detect::ScArm::Allintra => (
                        crate::restoration::wn_filter_ctrls_allintra(self.speed_config.preset),
                        crate::port_lr_level::set_sg_filter_ctrls(
                            crate::port_enc_mode_config::leaf::get_sg_filter_level_allintra(
                                lr_enc_mode,
                            ),
                        ),
                    ),
                    crate::sc_detect::ScArm::Video { .. } => {
                        let wn = crate::port_lr_level::wn_filter_level_default(
                            lr_enc_mode,
                            lr_resolution,
                            lr_is_not_last_layer,
                        );
                        // `scs->static_config.fast_decode` is 0 for every
                        // configuration this port and the inter harness produce.
                        let sg = crate::port_lr_level::sg_filter_level_default(
                            lr_enc_mode,
                            lr_resolution,
                            false,
                        );
                        (
                            crate::restoration::WnFilterCtrls::from(
                                crate::port_lr_level::set_wn_filter_ctrls(wn),
                            ),
                            crate::port_lr_level::set_sg_filter_ctrls(sg),
                        )
                    }
                };
                if ctrls.enabled || sg_ctrls.enabled {
                    // C `x->rdmult` = `pic_full_lambda[bit_depth == EB_TEN_BIT ?
                    // EB_10_BIT_MD : EB_8_BIT_MD]` (enc_dec_process.c:3246-3247),
                    // i.e. `svt_aom_lambda_assign(.., multiply_lambda = true)` —
                    // whose `*= 16` arm is 10-bit-ONLY, so bd8 is the unweighted
                    // value and bd10 is 16x the bd10 one. (Contrast the CDEF
                    // search, enc_cdef.c:958, which passes false.)
                    let rdmult = match (self.bit_depth, recon10.as_ref()) {
                        (10, Some(_)) => crate::pd0::kf_full_lambda_bd10_pic(base_qindex) as i64,
                        _ => crate::pd0::kf_full_lambda_8bit_unweighted(base_qindex) as i64,
                    };
                    let (su, sv) = chroma.unwrap_or((&[][..], &[][..]));
                    // PORT-NOTE(VERIFIED whole-frame — do NOT make per-tile):
                    // this call (and the per-SB `write_lr_for_sb` walk below)
                    // computes the restoration-unit grid across the WHOLE FRAME
                    // (`svtav1_dsp::restoration::count_units_in_tile(unit_size,
                    // pw)` — restoration.rs:425-426 — with the full plane
                    // width/height), which is EXACTLY what C does regardless of
                    // tile count: `svt_aom_foreach_rest_unit_in_frame` /
                    // `_frame_seg` (restoration.c:1274-1297 / 1379-1394) build
                    // the grid from `whole_frame_rect`, call `on_tile(0,0)`
                    // exactly once, and the stripe-derivation tile loop is
                    // hardcoded `for i < 1 /*cm->tile_rows*/` (restoration.c:1699).
                    // So the LR RU grid / tap-delta chain is tile-INDEPENDENT.
                    // (The earlier task-#86 "genuinely PER-TILE" hypothesis was
                    // WRONG — read the C source, not the "in_tile" name.) The
                    // task-#86 2-tile-row `lr-taps` divergence was a downstream
                    // SYMPTOM: a recon difference reprices the whole-frame Wiener
                    // taps, and that recon difference was the M6 PD0 partition
                    // search predicting DC across the tile boundary (pd0.rs
                    // `lvl1_block_cost_rect`, now fixed via `extract_neighbors_
                    // tiled`). With that fixed the LR taps match C byte-for-byte
                    // on the full multi-tile sweep (162/162), confirming this
                    // whole-frame grid is correct as-is.
                    // Task #95 goal 1 (odd true dims): the search runs on the TRUE
                    // luma / CEILING chroma extent, reading the recon at its aligned
                    // buffer stride while `extend_frame` replicates the true edge —
                    // so it never sees the aligned padding (matching C, whose
                    // extend replicates the frame edge into the LR border). Extract
                    // tight true/ceil buffers from the aligned-strided recon +
                    // source (luma stride `w`, chroma stride `cw`); on an 8-aligned
                    // frame true == aligned, so these are byte-neutral copies.
                    let (lr_tcw, lr_tch) =
                        (fmt.chroma_width(lr_true_w), fmt.chroma_height(lr_true_h));
                    let extract_tight = |src: &[u8], src_stride: usize, pw: usize, ph: usize| {
                        let mut out = alloc::vec![0u8; pw * ph];
                        for r in 0..ph {
                            out[r * pw..(r + 1) * pw]
                                .copy_from_slice(&src[r * src_stride..r * src_stride + pw]);
                        }
                        out
                    };
                    let lr_src_y = extract_tight(&encode_input, w, lr_true_w, lr_true_h);
                    let lr_rec_y = extract_tight(&*recon, w, lr_true_w, lr_true_h);
                    let (lr_src_u, lr_src_v, lr_rec_u, lr_rec_v) = if chroma.is_some() {
                        (
                            extract_tight(su, cw, lr_tcw, lr_tch),
                            extract_tight(sv, cw, lr_tcw, lr_tch),
                            extract_tight(&*u_recon, cw, lr_tcw, lr_tch),
                            extract_tight(&*v_recon, cw, lr_tcw, lr_tch),
                        )
                    } else {
                        (
                            alloc::vec::Vec::new(),
                            alloc::vec::Vec::new(),
                            alloc::vec::Vec::new(),
                            alloc::vec::Vec::new(),
                        )
                    };
                    // bd10: run the search on the TRUE 10-bit post-CDEF recon
                    // against the true 10-bit source. Same tight true/ceil
                    // extraction as the u8 arm — the 10-bit canvas is already
                    // tight (`w` / `w/2` stride), and the 10-bit source is
                    // `u8 << (bd - 8)` by construction (the harness writes exactly
                    // that .yuv for both encoders).
                    // LR search-input dump (SVTAV1_LRREC_BIN) — the tight
                    // post-CDEF planes the Wiener/SGR search reads, one set per
                    // frame. Pairs with the C `SVT_LFRECON_BIN` interposer
                    // dump (post-deblock == post-CDEF whenever every coded CDEF
                    // strength is 0).
                    #[cfg(feature = "std")]
                    if let Ok(prefix) = std::env::var("SVTAV1_LRREC_BIN") {
                        static CALL: core::sync::atomic::AtomicUsize =
                            core::sync::atomic::AtomicUsize::new(0);
                        let call = CALL.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                        for (plane, buf) in
                            [&lr_rec_y, &lr_rec_u, &lr_rec_v].into_iter().enumerate()
                        {
                            if !buf.is_empty() {
                                std::fs::write(format!("{prefix}.f{call}.p{plane}"), buf)
                                    .expect("write LR search-input recon");
                            }
                        }
                    }
                    let rest_info = match recon10.as_ref() {
                        Some((y10, u10, v10)) => {
                            let sh = (self.bit_depth - 8) as u32;
                            let widen_tight =
                                |src: &[u8], src_stride: usize, pw: usize, ph: usize| -> Vec<u16> {
                                    if src.is_empty() {
                                        return Vec::new();
                                    }
                                    let mut out = alloc::vec![0u16; pw * ph];
                                    for r in 0..ph {
                                        for c in 0..pw {
                                            out[r * pw + c] =
                                                (src[r * src_stride + c] as u16) << sh;
                                        }
                                    }
                                    out
                                };
                            let tight10 = |src: &[u16],
                                           src_stride: usize,
                                           pw: usize,
                                           ph: usize|
                             -> Vec<u16> {
                                if src.is_empty() {
                                    return Vec::new();
                                }
                                let mut out = alloc::vec![0u16; pw * ph];
                                for r in 0..ph {
                                    out[r * pw..(r + 1) * pw]
                                        .copy_from_slice(&src[r * src_stride..r * src_stride + pw]);
                                }
                                out
                            };
                            // Task #6 chunk 2: with a native HBD source the Wiener
                            // tap search sees the caller's real u16 samples (same
                            // tight true/ceil extraction, just from the u16 plane);
                            // otherwise the identical `u8 << sh` widening as before.
                            let (lr_sy10, lr_su10, lr_sv10) = match hbd_source.as_ref() {
                                Some(hbd) => {
                                    *hbd_used = true;
                                    (
                                        tight10(&hbd.y, w, lr_true_w, lr_true_h),
                                        tight10(&hbd.u, cw, lr_tcw, lr_tch),
                                        tight10(&hbd.v, cw, lr_tcw, lr_tch),
                                    )
                                }
                                None => (
                                    widen_tight(&encode_input, w, lr_true_w, lr_true_h),
                                    widen_tight(su, cw, lr_tcw, lr_tch),
                                    widen_tight(sv, cw, lr_tcw, lr_tch),
                                ),
                            };
                            crate::restoration::search_restoration_still_configured_with_stop(
                                &ctrls,
                                &sg_ctrls,
                                &lr_sy10,
                                &lr_su10,
                                &lr_sv10,
                                &tight10(y10, w, lr_true_w, lr_true_h),
                                &tight10(u10, acw, lr_tcw, lr_tch),
                                &tight10(v10, acw, lr_tcw, lr_tch),
                                lr_true_w,
                                lr_true_h,
                                filter_chroma,
                                rdmult,
                                self.bit_depth,
                                self.enhancements.contains(
                                    crate::enhancements::ZenEnhancement::AomRestorationUnitSearch,
                                ),
                                self.sb_size,
                                stop,
                            )?
                        }
                        None => {
                            crate::restoration::search_restoration_still_configured_with_stop::<u8>(
                                &ctrls,
                                &sg_ctrls,
                                &lr_src_y,
                                &lr_src_u,
                                &lr_src_v,
                                &lr_rec_y,
                                &lr_rec_u,
                                &lr_rec_v,
                                lr_true_w,
                                lr_true_h,
                                filter_chroma,
                                rdmult,
                                8,
                                self.enhancements.contains(
                                    crate::enhancements::ZenEnhancement::AomRestorationUnitSearch,
                                ),
                                self.sb_size,
                                stop,
                            )?
                        }
                    };
                    #[cfg(feature = "std")]
                    if crate::dbgenv::dump_lr() {
                        for (p, pr) in rest_info.planes.iter().enumerate() {
                            eprintln!(
                                "LR plane={p} frame_rtype={} units={:?}",
                                pr.frame_rtype,
                                pr.units
                                    .iter()
                                    .map(|u| (u.rtype, u.wiener.vfilter, u.wiener.hfilter))
                                    .collect::<alloc::vec::Vec<_>>()
                            );
                        }
                    }
                    if rest_info.any_non_none() {
                        // Decoder-exact application to the output copy: stripe
                        // boundaries from the post-deblock (pre-CDEF) and
                        // post-CDEF planes (dlf_process.c:134 after_cdef=0,
                        // cdef_process.c:707 after_cdef=1).
                        let (pre_y, pre_u, pre_v) = self
                            .last_recon_pre_cdef
                            .as_ref()
                            .expect("pre-CDEF recon captured above");
                        // Task #95 goal 1 / issue #11: the boundary save and the
                        // unit walk take the SAME TRUE extent the search sized the
                        // RU grid from (C drives all three off one
                        // `whole_frame_rect`), read at the ALIGNED canvas strides
                        // the planes are stored at. Passing the aligned extent here
                        // while the grid was counted on the true one made the walk
                        // visit more units than the grid holds — an out-of-bounds
                        // index whenever alignment crossed a `count_units_in_tile`
                        // boundary (e.g. true 383 -> 1 unit, aligned 384 -> 2).
                        // Byte-neutral for 8-aligned dims (true == aligned).
                        let bounds = crate::restoration::save_lr_boundaries(
                            pre_y,
                            pre_u,
                            pre_v,
                            &*recon,
                            &*u_recon,
                            &*v_recon,
                            lr_true_w,
                            lr_true_h,
                            w,
                            cw,
                            filter_chroma,
                        );
                        crate::restoration::apply_restoration_frame_bd_with_stop(
                            recon,
                            u_recon,
                            v_recon,
                            lr_true_w,
                            lr_true_h,
                            w,
                            cw,
                            filter_chroma,
                            &rest_info,
                            &bounds,
                            8,
                            stop,
                        )?;
                        // Issue #13: the 10-bit canvas gets the SAME apply. The
                        // search above picked these taps on the 10-bit recon and
                        // the frame header signals them, so a decoder applies
                        // them to its 10-bit output — until now no 10-bit plane
                        // in the port ever received them. Same true extent, same
                        // ALIGNED strides the 10-bit canvas is stored at (`w`
                        // luma, `w / 2` chroma — see `tight10` above), boundary
                        // lines from the 10-bit post-deblock (pre-CDEF) and
                        // post-CDEF planes (C: rest_process.c on the 16-bit
                        // recon picture, highbd = 1).
                        if let (Some((y10, u10, v10)), Some((py10, pu10, pv10))) =
                            (recon10.as_mut(), recon10_pre_cdef.as_ref())
                        {
                            let bounds10 = crate::restoration::save_lr_boundaries_bd::<u16>(
                                py10,
                                pu10,
                                pv10,
                                y10,
                                u10,
                                v10,
                                lr_true_w,
                                lr_true_h,
                                w,
                                acw,
                                filter_chroma,
                            );
                            crate::restoration::apply_restoration_frame_bd_with_stop::<u16>(
                                y10,
                                u10,
                                v10,
                                lr_true_w,
                                lr_true_h,
                                w,
                                acw,
                                filter_chroma,
                                &rest_info,
                                &bounds10,
                                self.bit_depth,
                                stop,
                            )?;
                        }
                    }
                    *last_lr_unit_size = Some(rest_info.planes[0].unit_size as usize);
                    *last_lr_stats = (
                        [
                            rest_info.planes[0].frame_rtype,
                            rest_info.planes[1].frame_rtype,
                            rest_info.planes[2].frame_rtype,
                        ],
                        rest_info
                            .planes
                            .iter()
                            .flat_map(|p| p.units.iter())
                            .filter(|u| u.rtype == svtav1_dsp::restoration::RESTORE_WIENER)
                            .count(),
                    );
                    *lr_signal = crate::entropy::obu::LrSignal {
                        enabled: true,
                        frame_types: [
                            rest_info.planes[0].frame_rtype,
                            rest_info.planes[1].frame_rtype,
                            rest_info.planes[2].frame_rtype,
                        ],
                        unit_size: rest_info.planes[0].unit_size as u16,
                        // C: rst_info[1].size != rst_info[0].size — always
                        // equal (set_restoration_unit_size s = 0).
                        uv_size_differs: false,
                    };
                    if decoder_chroma_recon {
                        *output_restoration = Some(rest_info.clone());
                    }
                    // Arm the bit-producing walk below with the LR syntax —
                    // `Some` here is exactly the old `if rest_info.any_non_none()`
                    // re-walk gate.
                    *walk_rest_info = Some(rest_info);
                }
            },
        )
    }
}
