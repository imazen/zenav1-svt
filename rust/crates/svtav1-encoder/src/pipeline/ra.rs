use super::*;

impl EncodePipeline {
    /// Apply the override + capability gate to a derived SB size.
    /// Returns `(sb_size, fell_back)`.
    /// Run C's picture decision for one picture and return its
    /// [`crate::port_picstruct::PicParams`].
    ///
    /// This is the port's only caller of `av1_generate_rps_info`; everything
    /// the INTER frame header says about references comes from here, so the
    /// header and the DPB can never describe different structures.
    ///
    /// # Errors
    ///
    /// [`EncodeError::UnsupportedConfig`] for a prediction-structure branch
    /// [`crate::port_picstruct::generate_rps_info`] does not translate. That
    /// is a refusal, not a fallback: an invented reference row would put
    /// `ref_frame_idx[]` in the header pointing at the wrong DPB slots.
    pub(super) fn run_picture_decision(
        &mut self,
        display_order: u64,
        is_key: bool,
    ) -> EncodeResult<crate::port_picstruct::PicParams> {
        use crate::port_picstruct as pp;

        let hier = self.gop.hierarchical_levels;
        // C `set_mrp_ctrl` (`enc_handle.c:3574`) — the preset-derived MRP
        // caps. NOT inert even on a two-frame cell: `set_ref_list_counts`
        // counts list 1 as 1 whenever its BWD-duplicate guard skips the
        // comparisons, and at M10/low-delay/8-bit the level-0 row's zero
        // list-1 caps are the only thing that makes `ref_list1_count_try`
        // 0 — which is what puts ME on C's single-reference candidate path
        // and keeps `me_distortion` L0-only. MEASURED 2026-09-14 on the
        // `gradient 104x104 q32 p10` cell: the neutral defaults let list 1
        // search, found a perfect (-3,0) match in two of four b64s, and the
        // resulting `norm_me_dist` halved C's, shifting the picture PD0
        // level 7->6 and demoting one SB to level 4 (29B vs C's 417B).
        let mrp_ctrls = pp::set_mrp_ctrl(
            self.speed_config.preset,
            false,
            hier,
            pp::PredStructure::LowDelay,
            self.bit_depth == 8,
            self.rc_config.mode.into(),
        );
        self.mrp_ctrls = mrp_ctrls;
        let (tf_level, tf_params_per_type) = pp::derive_tf_params(
            self.pred_structure,
            self.speed_config.preset,
            hier,
            self.enable_tf,
            /*lossless=*/ false,
        );
        let seq = pp::SeqPicParams {
            // The campaign's GOP: low-delay P, CQP/CRF. C's driver is given
            // `SVT_PRED_STRUCT=1` (LOW_DELAY) for the same cells. Under RA a
            // mid-stream key reaches this path via the drain release, where
            // `scs->static_config.pred_structure` stays RANDOM_ACCESS — so
            // this is `self.pred_structure`, not a literal. The RC mode is
            // the caller's: under CBR `av1_generate_rps_info` takes the
            // lay0/lay1-toggle L0-only arm (pd_process.c:2066) instead of the
            // CQP/CRF 7-slot rotation — visible in `ref_frame_idx[4..6]`.
            pred_structure: self.pred_structure,
            rate_control_mode: self.rc_config.mode.into(),
            rtc: false,
            allintra: false,
            mrp_ctrls,
            order_hint_info: crate::inter_mvp::OrderHintInfo {
                enable_order_hint: true,
                order_hint_bits: crate::entropy::obu::ORDER_HINT_BITS,
            },
            hierarchical_levels: hier,
            max_managed_refs: 0,
            enable_tf_key: self.enable_tf_key,
            tf_level,
            tf_params_per_type,
        };
        let mini_gop = 1u32 << hier;
        self.pd_ctx.mini_gop_length[0] = mini_gop;
        // C `ctx->list0_only` is a persistent `PictureDecisionContext` field
        // written ONLY inside `initialize_mini_gop_activity_array`
        // (`pd_process.c:846-848`), which runs only when the pre-assignment
        // buffer holds more than one picture or a non-intra RA buffer
        // (`:4756-4758`). Low delay releases one picture at a time, so the
        // init never runs and the field keeps its ctor value 0 —
        // `send_picture_out`'s tl0 list-1 clamp (`:4950-4952`) never fires
        // for low delay and `ref_list1_count_try` keeps the
        // `update_count_try` value. `mg_map.list0_only` is the port's copy of
        // the same persistent field; mirror it here.
        self.pd_ctx.list0_only = self.mg_map.list0_only;
        let mut pic = pp::PicParams {
            picture_number: display_order,
            decode_order: display_order,
            slice_type: if is_key {
                pp::SliceType::I
            } else {
                pp::SliceType::B
            },
            is_key_frame: is_key,
            is_intra_only: is_key,
            // C `svt_aom_is_delayed_intra` (`pd_process.c:3620`): an IDR/CRA
            // inside a RANDOM_ACCESS sequence with a live intra period. The
            // port has no per-input `end_of_sequence_flag` (C's is an input
            // buffer field), so that clause is always-false here.
            is_delayed_intra: is_key
                && self.pred_structure == pp::PredStructure::RandomAccess
                && self.gop.intra_period != 0,
            hierarchical_levels: hier,
            pred_struct_type: pp::PredStructure::LowDelay,
            pred_struct_entry_count: mini_gop,
            frame_offset: display_order,
            aligned_width: self.width,
            aligned_height: self.height,
            ..Default::default()
        };
        // C `update_pred_struct_and_pic_type` (`pd_process.c:4814-4871`) plus
        // the kernel's elapsed-counter update (`:5563-5590`). On a pure
        // low-delay stream the ported chain reduces to: IDR -> position 0;
        // the picture directly after an I slice -> `init_pic_index + 1` = 1;
        // otherwise position += 1, wrapping at `pred_struct_entry_count`.
        // `init_pic_index` is 0 for every C pred structure
        // (`pred_structure.c:679`), CRA/S-frame/cut-short arms are unreachable
        // (every key is an IDR, `pred_struct_type` is LOW_DELAY, the
        // hierarchy never changes), so the ported function is driven with
        // exactly those flags rather than re-deriving the chain.
        let slice = pp::update_pred_struct_and_pic_type(
            &mut pic,
            &mut self.enc_pic,
            &mut self.mg_map,
            &mut self.pd_ctx,
            /*mini_gop_index=*/ 0,
            /*pre_assignment_buffer_first_pass_flag=*/ false,
            /*idr_flag=*/ is_key,
            /*cra_flag=*/ false,
            /*init_pred_struct_position_flag=*/ false,
            /*init_pic_index=*/ 0,
        );
        debug_assert_eq!(slice, pic.slice_type);
        // `pd_process.c:5568-5588`: I_SLICE zeroes the elapsed-CRA counter
        // (and `get_pic_idx_in_mg`'s caller already moved `last_idr_picture`),
        // B_SLICE increments it clipped.
        if is_key {
            self.enc_pic.elapsed_non_cra_count = 0;
        } else {
            self.enc_pic.elapsed_non_cra_count =
                self.enc_pic.elapsed_non_cra_count.saturating_add(1);
        }
        // `pd_process.c:4869/5559`:
        // `pred_position_ptr = entry_array[pred_struct_position]` and
        // `temporal_layer_index = pred_position_ptr->temporal_layer_index`.
        pic.temporal_layer_index = pp::PRED_STRUCT_TEMPORAL_LAYER[usize::from(hier)]
            [self.enc_pic.pred_struct_position as usize];
        // `pd_process.c:5548`: the RPS branch selector. Its low-delay arm is
        // `(pos - 1) % entry_count` with a special case at 0 — NOT the
        // position itself — and it also writes `frame_offset`.
        let pic_idx = pp::get_pic_idx_in_mg(&mut pic, &seq, &self.enc_pic, &self.mg_map, 0, 0);
        // C `ref_pa_pic_ptr_array[list][0]`'s `avg_luma` — the PA reference
        // object's luma mean (`pic_analysis_process.c:2003`). Every
        // reference this path can name is an already-encoded frame, so its
        // DPB-mirrored PA slot holds the value `get_similar_ref_brightness`
        // reads; `INVALID_LUMA` when the slot's pyramid is stale or absent.
        let pa_slots = &self.pa_slots;
        let pa_luma = |poc: u64, slot: usize| -> u64 {
            pa_slots
                .get(slot)
                .and_then(|s| s.as_deref())
                .filter(|p| p.picture_number == poc)
                .map_or(pp::INVALID_LUMA, |p| p.avg_luma)
        };
        pp::picture_decision_per_picture(&mut pic, &seq, &mut self.pd_ctx, pic_idx, 0, &pa_luma)
            .map_err(|_| {
                whereat::at!(EncodeError::UnsupportedConfig(
                    "this GOP shape's reference structure is not implemented: every \
                     top-level branch of C's av1_generate_rps_info is translated, so \
                     this is a case C itself rejects — LD-CBR outside hierarchical \
                     levels 1-2, or a mini-GOP position outside the ported tables \
                     [C: logs the same config as an error]",
                ))
            })?;
        Ok(pic)
    }

    /// The configuration envelope random access adds to `gop_config_error`'s.
    /// `None` means a window may run.
    pub(super) fn ra_config_error(&self) -> Option<&'static str> {
        if self.gop.intra_period <= 1 {
            return Some(
                "pred_structure RandomAccess needs a GOP: intra_period == 1 makes every \
                 frame a key frame, so there is no mini-GOP to reorder, and \
                 intra_period == 0 (single key then low-delay inter) is untested on \
                 the RA path [C: accepts but degenerates to all-intra at == 1]",
            );
        }
        if self.superres_denom.is_some() {
            return Some(
                "pred_structure RandomAccess with superres is untested: references at \
                 a different coded width inside the window need decoder verification \
                 first [C: accepts]",
            );
        }
        if self.film_grain.enabled() || self.hdr.noise_strength > 0 {
            return Some(
                "pred_structure RandomAccess with film grain is untested: C's \
                 show_existing headers would have to re-signal grain state \
                 [C: accepts]",
            );
        }
        None
    }

    /// Entry-point guard for the non-420 arms: random access buffers and
    /// reorders input, which only `try_encode_frame_420` implements today —
    /// a mono/444/hbd frame fed through the sequential path would silently
    /// break the mini-GOP's display order, so refuse instead of emitting it.
    pub(super) fn ra_entry_error(&self, _entry: &str) -> Option<whereat::At<EncodeError>> {
        (self.pred_structure == crate::port_picstruct::PredStructure::RandomAccess).then(|| {
            whereat::at!(EncodeError::UnsupportedConfig(
                "pred_structure RandomAccess is wired on try_encode_frame_420 only; \
                 this entry takes the sequential path, which cannot buffer a \
                 mini-GOP [C: accepts]",
            ))
        })
    }

    /// The random-access arm of [`Self::try_encode_frame_420`].
    ///
    /// Buffers one input at `ra_display_next`'s display position, and only
    /// encodes when a whole mini-GOP is present — returning `Ok(Vec::new())`
    /// until then. A key frame (intra-period boundary) drains the partial
    /// window FIRST, then codes itself through the ordinary sequential path,
    /// exactly like C's `pre_assignment_buffer_idr_count` release.
    pub(super) fn try_encode_frame_420_ra(
        &mut self,
        y: &[u8],
        u: &[u8],
        v: &[u8],
        y_stride: usize,
    ) -> EncodeResult<Vec<u8>> {
        if let Some(why) = self.ra_config_error() {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(why)));
        }
        let display_order = self.ra_display_next;
        self.ra_display_next += 1;

        // Stage the frame at TRUE dims; the encode pads per picture. The
        // PA statistics gather is unconditional — `calc_hist` is 1 in RA.
        // A key frame is just another reorder-queue entry here: C's drain
        // stamps `pcs->idr_flag` on it, which both terminates the current
        // release (`pre_assignment_buffer_intra_count > 0`) and, under a
        // live intra period, defers its TF window + packet to the NEXT
        // release (`svt_aom_is_delayed_intra` → `ctx->prev_delayed_intra`,
        // `pd_process.c:3620-3635, 5103-5131`).
        let is_key = self.gop.is_key_frame(display_order);
        let frame = self.pack_ra_frame(y, u, v, y_stride, display_order, is_key);
        let stats = self.gather_tf_stats_frame(&frame);
        self.ra_input.push(frame);
        self.ra_stats.push(Some(alloc::boxed::Box::new(stats)));
        self.ra_try_release()
    }

    /// Fire every pending release whose window the newly pushed frame just
    /// completed (`check_window_availability`'s `pd_window[2..2+scd_delay)`
    /// requirement — `pd_process.c:4659-4676` + the release trigger at
    /// `:5478-5484`). Queue order: an intra-terminated `[tail + intra]`
    /// release fires the moment the intra is committed — but only once the
    /// INTRA's own `scd_delay` future entries are queued, since its
    /// detector turn runs in the same drain.
    pub(super) fn ra_try_release(&mut self) -> EncodeResult<Vec<u8>> {
        let mut out = alloc::vec::Vec::new();
        loop {
            let len = self.ra_input.len();
            let mg = self.gop.mini_gop_size as usize;
            let scd = self.ra_scd_delay() as usize;
            let fire = match self.ra_input.iter().position(|f| f.is_key) {
                // The release is `[tail + intra]` — `kp + 1` entries — once
                // the intra's `scd_delay` futures sit behind it:
                // `kp + scd < len`.
                Some(kp) if kp < mg => len > kp + scd,
                // A plain release commits `mg` entries; the deepest one's
                // window needs `mg - 1 + scd < len`.
                _ => len >= mg + scd,
            };
            if !fire {
                break;
            }
            out.extend_from_slice(&self.encode_ra_window()?);
        }
        Ok(out)
    }

    /// Pack one input into a [`RaBufferedFrame`] at TRUE dims.
    pub(super) fn pack_ra_frame(
        &self,
        y: &[u8],
        u: &[u8],
        v: &[u8],
        y_stride: usize,
        display_order: u64,
        is_key: bool,
    ) -> RaBufferedFrame {
        let (tw, th) = (self.true_width as usize, self.true_height as usize);
        let (cw, ch) = (tw.div_ceil(2), th.div_ceil(2));
        let mut yb = alloc::vec::Vec::with_capacity(tw * th);
        for r in 0..th {
            yb.extend_from_slice(&y[r * y_stride..r * y_stride + tw]);
        }
        RaBufferedFrame {
            y: yb,
            u: u[..cw * ch].to_vec(),
            v: v[..cw * ch].to_vec(),
            display_order,
            is_key,
            is_eos: false,
        }
    }

    /// The `enhanced_pic` + `pa_ref_pic_wrapper` pair one buffered input
    /// owns in C: the padded aligned-dims planes the TF window's members
    /// are read through, fused into one [`crate::port_tf_driver::TfPicBufs`].
    pub(super) fn ra_tf_bufs(&self, f: &RaBufferedFrame) -> crate::port_tf_driver::TfPicBufs {
        let (tw, th) = (self.true_width as usize, self.true_height as usize);
        let (aw, ah) = (self.width as usize, self.height as usize);
        let (cw, ch) = (tw.div_ceil(2), th.div_ceil(2));
        let (acw, ach) = (aw.div_ceil(2), ah.div_ceil(2));
        let y_pad = if aw == tw && ah == th {
            f.y.clone()
        } else {
            pad_plane_replicate(&f.y, tw, tw, th, aw, ah)
                .expect("aligned dims are never smaller than true dims")
        };
        let (u_pad, v_pad) = if f.u.is_empty() {
            (alloc::vec::Vec::new(), alloc::vec::Vec::new())
        } else {
            (
                pad_plane_replicate(&f.u, cw, cw, ch, acw, ach)
                    .expect("aligned dims are never smaller than true dims"),
                pad_plane_replicate(&f.v, cw, cw, ch, acw, ach)
                    .expect("aligned dims are never smaller than true dims"),
            )
        };
        crate::port_tf_driver::TfPicBufs::from_aligned_planes(
            &y_pad,
            &u_pad,
            &v_pad,
            aw,
            ah,
            f.display_order,
        )
    }

    /// `scs->scd_delay` (`enc_handle.c:4005-4038`) for this stream: the
    /// future-picture depth `check_window_availability` requires before a
    /// queue entry may pass picture decision — and the extra depth
    /// `ra_input` must buffer past each release.
    pub(super) fn ra_scd_delay(&self) -> u32 {
        let (_tf_level, tf_params_per_type) = crate::port_picstruct::derive_tf_params(
            self.pred_structure,
            self.speed_config.preset,
            self.gop.hierarchical_levels,
            self.enable_tf,
            /*lossless=*/ false,
        );
        crate::port_picstruct::derive_scd_delay(
            /*intra_period_is_zero=*/ self.gop.intra_period == 0,
            &tf_params_per_type,
            // `vq_ctrls.sharpness_ctrls.scene_transition` is 1 for every
            // single-pass RANDOM_ACCESS config (`enc_handle.c:3282-3291`,
            // cleared only for low delay / first pass at `:3324-3326`).
            /*scene_transition_armed=*/
            true,
            /*lap_rc=*/ false,
        )
    }

    /// One release's queue turns of `perform_scene_change_detection` +
    /// `copy_histograms` (`pd_process.c:5378-5386, 5449-5452`), in display
    /// order over the release's `committed` queue entries.
    ///
    /// `check_window_availability` requires `scd_delay` future queue entries
    /// past each picture, none of them `end_of_sequence_flag`
    /// (`pd_process.c:4664-4674`). Mid-stream the release triggers guarantee
    /// every committed picture clears it; at EOS the missing tail entries
    /// are exactly what C's `eos_reached` resolves to `window_avail ==
    /// false` — the entry still commits, its detection is skipped.
    pub(super) fn ra_pump_release(&mut self, committed: usize) {
        use crate::port_picstruct as pp;
        let scd = self.ra_scd_delay() as usize;
        let eff_len = self.ra_input.len();
        // `enhanced_pic->width/height` is the 8-aligned input size
        // (`max_input_luma_*` after `MIN_BLOCK_SIZE` pad,
        // resource_coordination_process.c:715-727); the region split keys
        // on the same field (`enc_handle.c:4392-4397`).
        let (tw, th) = (self.width, self.height);
        let (rw, rh) = (
            if tw >= 64 { 4usize } else { 1 },
            if th >= 64 { 4usize } else { 1 },
        );
        // Take the detector state out of `pd_ctx` so the stats borrows on
        // `self` can coexist with the detector's `&mut`.
        let mut sd = core::mem::take(&mut self.pd_ctx.scene_detect);
        let mut td = self.pd_ctx.transition_detected;
        for pos in 0..committed {
            // `:4664-4674` — every slot in `pd_window[2..2+scd)` must be
            // filled AND none may be `end_of_sequence_flag`; an EOS frame
            // inside the window also marks it unavailable.
            let window_avail =
                pos + scd < eff_len && !self.ra_input[pos + 1..=pos + scd].iter().any(|f| f.is_eos);
            let display_order = self.ra_input[pos].display_order;
            if window_avail && display_order > 0 {
                let cur = self.ra_stats[pos]
                    .as_deref()
                    .expect("committed positions always carry stats");
                let fut = self.ra_stats[pos + 1]
                    .as_deref()
                    .expect("window_avail implies at least one future");
                let out = pp::perform_scene_change_detection(
                    // `static_config.scene_change_detection` is force-zeroed
                    // in this SVT (`enc_settings.c:839-843`) — only the
                    // sharpness arm ever runs the detector.
                    /*scene_change_detection_enabled=*/
                    false,
                    /*sharpness_scene_transition=*/ true,
                    td,
                    /*cra_flag_in=*/ false,
                    || {
                        pp::scene_transition_detector(
                            &mut sd,
                            &cur.picture_histogram,
                            &cur.average_intensity_per_region,
                            &fut.average_intensity_per_region,
                            tw,
                            th,
                            rw,
                            rh,
                        )
                    },
                );
                td = out.transition_detected;
            }
            let cur = self.ra_stats[pos]
                .as_deref()
                .expect("committed positions always carry stats");
            pp::copy_histograms(
                &mut sd,
                &cur.picture_histogram,
                &cur.average_intensity_per_region,
            );
        }
        self.pd_ctx.scene_detect = sd;
        self.pd_ctx.transition_detected = td;
    }

    /// Drain any pending output. Under `PredStructure::RandomAccess` this
    /// encodes a partial trailing mini-GOP — C's `pre_assignment_buffer_eos_flag`
    /// release, which `is_pic_cutting_short_ra_mg` codes low-delay. Under the
    /// default low-delay structure there is never pending output and this is
    /// a no-op. Must be called after the last input, or the tail of an RA
    /// stream is silently un-emitted.
    pub fn try_flush(&mut self) -> EncodeResult<Vec<u8>> {
        if self.pred_structure != crate::port_picstruct::PredStructure::RandomAccess
            || (self.ra_input.is_empty() && self.delayed_intra.is_none())
        {
            return Ok(alloc::vec::Vec::new());
        }
        // The last input carries `end_of_sequence_flag` — C reads it in
        // `check_window_availability` (window never resolves past it) and
        // `svt_aom_is_delayed_intra` (a trailing intra is NOT delayed).
        if let Some(last) = self.ra_input.last_mut() {
            last.is_eos = true;
        }
        // C releases the whole reorder queue at EOS in queue order — an
        // intra-terminated release first when an intra is queued, then
        // `mini_gop_size` windows, then the cut-short remainder
        // (`pre_assignment_buffer_eos_flag`). Each `encode_ra_window` call
        // commits exactly one release and always makes progress.
        let mut out = alloc::vec::Vec::new();
        while !self.ra_input.is_empty() || self.delayed_intra.is_some() {
            out.extend_from_slice(&self.encode_ra_window()?);
        }
        Ok(out)
    }

    /// One random-access pre-assignment buffer's worth of packets.
    ///
    /// Runs [`Self::run_ra_picture_decision`] — the ported window kernel —
    /// then encodes the pictures in DECODE order, appending a
    /// `show_existing_frame` OBU_FRAME_HEADER right after any picture whose
    /// decision produced one (`pic.has_show_existing`), which is exactly the
    /// display-order point C emits it at.
    pub(super) fn encode_ra_window(&mut self) -> EncodeResult<Vec<u8>> {
        let mg = self.gop.mini_gop_size as usize;
        // A held `prev_delayed_intra` with an empty queue still gets its
        // deferred `mctf_frame` + `send_picture_out` — C's `process_pics`
        // runs it ahead of any member work; here there are no members.
        if self.ra_input.is_empty() && self.delayed_intra.is_some() {
            let scs_tf = self.ra_tf_scs();
            let (q_weight, q_weight_denom) =
                crate::port_enc_mode_config::me::get_qp_based_th_scaling_factors(
                    self.speed_config.preset > -1,
                    u32::from(self.rc_config.qp),
                );
            let rw = if self.width >= 64 { 4 } else { 1 };
            let rh = if self.height >= 64 { 4 } else { 1 };
            self.filter_delayed_intra(
                &scs_tf,
                q_weight,
                q_weight_denom,
                &mut alloc::vec::Vec::new(),
                /*first_future_hier=*/ None,
                rw,
                rh,
                /*mg_len=*/ 0,
            );
            return self.encode_delayed_intra();
        }
        // Queue order decides the release shape: an intra inside the next
        // `mg` entries terminates the release at `[tail + intra]`
        // (`pre_assignment_buffer_intra_count > 0`, `pd_process.c:5478`);
        // otherwise the release is the next `min(mg, len)` entries.
        let kp = self.ra_input.iter().position(|f| f.is_key);
        let (n, key_member) = match kp {
            Some(kp) if kp < mg => (kp, true),
            _ => (mg.min(self.ra_input.len()), false),
        };
        // One display-order pass of detection + histogram carry for every
        // committed queue entry (`pd_process.c:5378-5386, 5449-5452`).
        self.ra_pump_release(n + usize::from(key_member));
        let (mut pics, emit, frames_tf) = self.run_ra_picture_decision(n, key_member)?;
        #[cfg(feature = "std")]
        if crate::dbgenv::medbg() {
            std::eprintln!(
                "RAEMIT emit={emit:?} pics={:?}",
                pics.iter()
                    .map(|p| p.as_ref().map(|p| (
                        p.picture_number,
                        p.decode_order,
                        p.temporal_layer_index
                    )))
                    .collect::<alloc::vec::Vec<_>>()
            );
        }
        // Drain only this release's members: the `scd_delay` spillover (and
        // any post-intra frames past an intra-terminated release) stay
        // buffered for the next release. Holding the drained frames locally
        // keeps `&mut self` free for the per-picture encode.
        let m = n + usize::from(key_member);
        let mut frames: alloc::vec::Vec<RaBufferedFrame> = self.ra_input.drain(..m).collect();
        let stats_drained: alloc::vec::Vec<_> = self.ra_stats.drain(..m).collect();
        if key_member && pics[n].as_ref().is_some_and(|p| p.is_delayed_intra) {
            // `ctx->prev_delayed_intra = pcs` (`pd_process.c:5151-5154`): the
            // intra member's send is replaced by the hold — its TF window and
            // packet run inside the NEXT release's `process_pics`.
            debug_assert!(frames.last().is_some_and(|f| f.is_key));
            let kf = frames.pop().expect("the intra is the last member");
            let stats = stats_drained[n]
                .as_deref()
                .expect("the intra gathered stats at push")
                .clone();
            self.delayed_intra = Some(DelayedIntra {
                bufs: self.ra_tf_bufs(&kf),
                frame: kf,
                stats,
                pic: pics[n]
                    .clone()
                    .expect("the intra's decision ran in the release"),
            });
        }
        // C `initial_rc_process` (src_ops): TPL runs between picture
        // decision and the encode loop — `tpl_prep_info` + `tpl_mc_flow` +
        // `generate_r0beta` per group base, producing each picture's `r0`,
        // `tpl_beta` and `tpl_rdmult_scaling_factors` plus the open-loop ME
        // results the encode then reuses. `None` on every configuration
        // where C's `get_tpl` disables TPL.
        let mut tpl_stage = self.run_tpl_stage(&frames, &pics, &emit, &frames_tf)?;
        self.delayed_intra_tpl = tpl_stage.as_mut().and_then(|s| s.key.take());
        let mut out = alloc::vec::Vec::new();
        // `send_picture_out(ctx->prev_delayed_intra)` (`pd_process.c:5129`):
        // the delayed intra's packet lands inside THIS release, after its
        // `mctf_frame` in pass 4 staged `delayed_intra_out`, ahead of the
        // member packets.
        if self.delayed_intra_out.is_some() {
            out.extend_from_slice(&self.encode_delayed_intra()?);
        }
        let (tw, th) = (self.true_width as usize, self.true_height as usize);
        let (aw, ah) = (self.width as usize, self.height as usize);
        let tiles_log2 = self.tile_rows_log2 + self.tile_cols_log2;
        let td = crate::entropy::obu::write_temporal_delimiter();
        // C `count_frames_in_next_tu` (packetization_process.c:90-115): a
        // temporal unit is [hidden frames]* + ONE displayable frame — the TD
        // opens each TU and a `show_frame` picture closes it. The emit loop
        // is in decode order, so a shown picture simply ends the open TU.
        let mut tu_open = false;
        for idx in emit {
            let frame = &frames[idx];
            let pic = pics[idx]
                .take()
                .expect("emit order visits each picture once");
            let display_order = frame.display_order;
            let show_frame = pic.show_frame;
            let has_show_existing = pic.has_show_existing;
            let show_slot = pic.show_existing_frame;
            let decided = Some(FrameDecision {
                display_order,
                pic,
                tpl: tpl_stage.as_mut().and_then(|s| s.frames[idx].take()),
            });
            if !tu_open {
                out.extend_from_slice(&td);
                tu_open = true;
            }
            let pkt = if let Some(tf_pic) = frames_tf.get(idx).filter(|t| t.do_tf) {
                // The temporally filtered planes ARE the aligned-dims
                // source C's `enhanced_pic` carries out of
                // `svt_av1_init_temporal_filtering` — feed them to the
                // encode directly (the true->aligned pad is already inside
                // them, so `pad_plane_replicate` must not run again).
                let y_f = tf_pic.extract_luma();
                let u_f = tf_pic.extract_u();
                let v_f = tf_pic.extract_v();
                self.encode_frame_impl(&y_f, aw, Some((&u_f, &v_f)), decided)?
            } else if aw == tw && ah == th {
                self.encode_frame_impl(&frame.y, tw, Some((&frame.u, &frame.v)), decided)?
            } else {
                let (cw, ch) = (tw.div_ceil(2), th.div_ceil(2));
                let (acw, ach) = (aw.div_ceil(2), ah.div_ceil(2));
                let y_pad = pad_plane_replicate(&frame.y, tw, tw, th, aw, ah)?;
                let u_pad = pad_plane_replicate(&frame.u, cw, cw, ch, acw, ach)?;
                let v_pad = pad_plane_replicate(&frame.v, cw, cw, ch, acw, ach)?;
                self.encode_frame_impl(&y_pad, aw, Some((&u_pad, &v_pad)), decided)?
            };
            // `encode_frame_impl` prepends the sequential path's
            // one-TU-per-frame TD; an RA TU spans hidden+shown frames, so the
            // loop above owns TD placement and each packet's copy is dropped.
            debug_assert_eq!(
                pkt[..td.len()],
                td[..],
                "frame packet must begin with the TD it always emits"
            );
            out.extend_from_slice(&pkt[td.len()..]);
            if show_frame {
                tu_open = false;
            }
            if has_show_existing {
                // C emits each show_existing as its own temporal unit
                // (TD + OBU_FRAME_HEADER); without the TD a decoder treats
                // it as part of the preceding frame's TU and drops it.
                out.extend_from_slice(&td);
                out.extend_from_slice(&crate::entropy::obu::write_show_existing_obu(
                    show_slot, tiles_log2,
                ));
            }
        }
        Ok(out)
    }

    /// C `svt_aom_picture_decision_kernel`'s mini-GOP loop
    /// (`pd_process.c:5489-5693`), transcribed for one full pre-assignment
    /// buffer. Returns one [`PicParams`] per input picture (indexed by
    /// `ra_input` position) plus the emit order: buffer indices in DECODE
    /// order, which `store_mg_picture_arrays` computes per mini-GOP.
    ///
    /// The three passes match C exactly: pred-structure/slice-type per
    /// picture in DISPLAY order, `decode_order` assignment in DISPLAY order,
    /// then `picture_decision_per_picture` — RPS + DPB + settings — in
    /// DECODE order, because `update_dpb` applies each picture's refresh
    /// mask to the toggle ring in coded order.
    /// `n` non-intra members, plus — when `key_member` — the intra queue
    /// entry at member index `n`. C's pre-assignment buffer holds the intra
    /// like any other committed entry: `set_mini_gop_structure` splits the
    /// `n + 1` buffer with `pcs->idr_flag` exceptions, and the intra goes
    /// through passes 1-3 (`update_pred_struct_and_pic_type`,
    /// `av1_generate_rps_info`, `init_pic_settings` — where its TL0
    /// `transition_present` consumes `ctx->transition_detected`). Its
    /// `mctf_frame`/`send_picture_out` defer to the next release via
    /// `ctx->prev_delayed_intra` — the caller moves the member into
    /// `self.delayed_intra` when `pics[n].is_delayed_intra`.
    pub(super) fn run_ra_picture_decision(
        &mut self,
        n: usize,
        key_member: bool,
    ) -> EncodeResult<(
        alloc::vec::Vec<Option<crate::port_picstruct::PicParams>>,
        alloc::vec::Vec<usize>,
        alloc::vec::Vec<crate::port_tf_driver::TfPicBufs>,
    )> {
        use crate::port_picstruct as pp;
        let m = n + usize::from(key_member);
        let hier = self.gop.hierarchical_levels;
        // C `set_mrp_ctrl` (`enc_handle.c:3574`) — same call as the
        // low-delay path but with the RANDOM_ACCESS structure, whose caps
        // differ (backward references are real under RA).
        let mrp_ctrls = pp::set_mrp_ctrl(
            self.speed_config.preset,
            false,
            hier,
            pp::PredStructure::RandomAccess,
            self.bit_depth == 8,
            self.rc_config.mode.into(),
        );
        self.mrp_ctrls = mrp_ctrls;
        let (tf_level, tf_params_per_type) = pp::derive_tf_params(
            self.pred_structure,
            self.speed_config.preset,
            hier,
            self.enable_tf,
            /*lossless=*/ false,
        );
        let seq = pp::SeqPicParams {
            pred_structure: pp::PredStructure::RandomAccess,
            rate_control_mode: self.rc_config.mode.into(),
            rtc: false,
            allintra: false,
            mrp_ctrls,
            order_hint_info: crate::inter_mvp::OrderHintInfo {
                enable_order_hint: true,
                order_hint_bits: crate::entropy::obu::ORDER_HINT_BITS,
            },
            hierarchical_levels: hier,
            max_managed_refs: 0,
            enable_tf_key: self.enable_tf_key,
            tf_level,
            tf_params_per_type,
        };

        // The buffer C releases is `pre_assignment_buffer_count` committed
        // queue entries — `n` non-intra members plus, for an
        // intra-terminated release, the intra itself (member `n`): the
        // `pcs->idr_flag` exceptions in the split see it.
        self.enc_pic.pre_assignment_buffer_count = m as u32;
        self.enc_pic.pre_assignment_buffer_intra_count = u32::from(key_member);
        self.enc_pic.pre_assignment_buffer_idr_count = u32::from(key_member);
        // `set_mini_gop_structure` reads `pic.picture_number` — the LAST
        // committed entry's POC (`pcs` at `pd_process.c:5486` is the entry
        // just appended) — and `pic.hierarchical_levels` (RTC arm only —
        // `seq.rtc` is false here).
        let stub = pp::PicParams {
            picture_number: self.ra_input[m - 1].display_order,
            hierarchical_levels: hier,
            ..Default::default()
        };
        // `enable_dg = false`: C's dynamic-GOP split is unported and refused
        // above for the only level that can reach it (hier 5 / 32-pic MG).
        // `list0_only_base = enc_mode > ENC_M2` (`enc_handle.c:4292`) — C's
        // `scs->list0_only_base`, which `initialize_mini_gop_activity_array`
        // copies to `ctx->list0_only` (`pd_process.c:846-848`) for
        // `send_picture_out`'s tl0 list-1 clamp (`:4950`). That init only
        // runs when the pre-assignment buffer holds more than one picture
        // (`:4756-4758`), so the field persists otherwise — mirror the map's
        // value rather than re-deriving it.
        let list0_only = self.speed_config.preset > crate::port_enc_mode_config::enc_mode::M2;
        let needs_dg = pp::set_mini_gop_structure(
            &mut self.mg_map,
            &mut self.enc_pic,
            &seq,
            &stub,
            u32::from(hier),
            /*startup_mg_size=*/ 0,
            // The last committed entry's `pcs->idr_flag` — the intra's own
            // flag on an intra-terminated release.
            /*idr_flag=*/
            key_member,
            /*enable_dg=*/ false,
            list0_only,
        );
        self.pd_ctx.list0_only = self.mg_map.list0_only;
        debug_assert!(
            !needs_dg,
            "eval_sub_mini_gop is unported; hier < 5 cannot reach it"
        );

        let mut pics: alloc::vec::Vec<Option<pp::PicParams>> = (0..m).map(|_| None).collect();
        let mut emit: alloc::vec::Vec<usize> = alloc::vec::Vec::new();

        // `scs->calc_hist` (enc_handle.c:1353): the region histograms +
        // `avg_luma` are gathered for EVERY buffered input at push time —
        // the sharpness scene-transition arm makes `calc_hist` 1 in every
        // RA config, not just TF-enabled ones.
        debug_assert_eq!(
            self.ra_stats.len(),
            self.ra_input.len(),
            "ra_stats must be parallel to ra_input"
        );

        // The `enhanced_pic` + `pa_ref_pic_wrapper` pair every buffered
        // input owns in C: the padded aligned-dims planes the TF window's
        // members are read through, fused into one [`TfPicBufs`] per slot.
        // Built under the same `calc_hist` gate — without it no picture can
        // be TF-enabled, so `frames_tf` stays empty and the encode loop
        // takes the unfiltered arm for every slot.
        let mut frames_tf: alloc::vec::Vec<crate::port_tf_driver::TfPicBufs> =
            alloc::vec::Vec::new();
        if tf_params_per_type.iter().any(|c| c.enabled) {
            // EVERY queued entry owns buffers — C's reorder-queue entries
            // each carry `enhanced_pic`/`pa_ref_pic_wrapper`, and a member's
            // TF window reaches past the release into the `scd_delay`
            // spillover (`mg_pictures_array` + queue lookahead).
            for f in &self.ra_input {
                frames_tf.push(self.ra_tf_bufs(f));
            }
        }

        for mg_idx in 0..self.mg_map.total_number_of_mini_gops {
            let start = self.mg_map.start_index[mg_idx] as usize;
            let end = self.mg_map.end_index[mg_idx] as usize;
            let mg_len = self.mg_map.length[mg_idx];
            // `pd_process.c:5494-5504`: the first picture's
            // `hierarchical_layers_diff` vs the PREVIOUS mini-GOP decides
            // `init_pred_struct_position_flag` (and `is_mini_gop_changed`,
            // which nothing downstream reads here).
            let init_pos_flag = self.enc_pic.previous_mini_gop_hierarchical_levels
                != self.mg_map.hierarchical_levels[mg_idx];
            self.enc_pic.previous_mini_gop_hierarchical_levels =
                self.mg_map.hierarchical_levels[mg_idx];
            self.pd_ctx.cut_short_ra_mg = 0;

            // ---- Pass 1, DISPLAY order (`pd_process.c:5506-5608`) ----
            // pred structure + slice type + pred_struct_position per picture.
            let mut pred_struct_index = alloc::vec::Vec::with_capacity(end - start + 1);
            let mut pic_idx_in_mg = alloc::vec::Vec::with_capacity(end - start + 1);
            for pic_idx in start..=end {
                let first = pic_idx == start;
                // The intra-terminated release's last member IS the intra:
                // `pcs->idr_flag` runs the IDR arms of the pred-struct
                // helpers and `picture_decision_per_picture`, and pass 3
                // stamps `is_delayed_intra`.
                let intra_member = key_member && pic_idx == m - 1;
                let mut pic = pp::PicParams {
                    picture_number: self.ra_input[pic_idx].display_order,
                    // `frm_hdr.frame_type` (`pd_process.c:5674-5679`): the
                    // intra member is a KEY_FRAME — `generate_rps_info`'s
                    // `set_key_frame_rps` arm resets the DPB on it.
                    is_key_frame: intra_member,
                    is_intra_only: intra_member,
                    slice_type: pp::SliceType::B,
                    pred_struct_type: pp::PredStructure::RandomAccess,
                    aligned_width: self.width,
                    aligned_height: self.height,
                    // `pcs.c:422`: the configured input dims, not the aligned
                    // ones — the TF window's resolution-change exclusion
                    // compares these.
                    frame_width: self.true_width,
                    frame_height: self.true_height,
                    // `resource_coordination_process.c:414-416`: non-CBR
                    // `frame_offset = picture_number`. `get_pic_idx_in_mg`
                    // overwrites it only in low delay.
                    frame_offset: self.ra_input[pic_idx].display_order,
                    // C `pcs->avg_luma` — `svt_aom_gathering_picture_statistics`'s
                    // output (`pic_analysis_process.c:628-635`), `INVALID_LUMA`
                    // whenever `calc_hist` is off: `ra_stats` is `None` per slot
                    // under exactly that gate.
                    avg_luma: self
                        .ra_stats
                        .get(pic_idx)
                        .and_then(|s| s.as_deref())
                        .map_or(pp::INVALID_LUMA, |s| s.avg_luma),
                    ..Default::default()
                };
                pp::get_pred_struct_for_frame(
                    &mut pic,
                    &mut self.mg_map,
                    mg_idx,
                    seq.pred_structure,
                    hier,
                    /*startup_mg_size=*/ 0,
                    /*idr_flag=*/ intra_member,
                    /*cra_flag=*/ false,
                );
                // `pred_struct_ptr` follows the (possibly cut-short) type
                // and the MINI-GOP's level, not the sequence's.
                pic.pred_struct_entry_count = 1u32 << pic.hierarchical_levels;
                pic.slice_type = pp::update_pred_struct_and_pic_type(
                    &mut pic,
                    &mut self.enc_pic,
                    &mut self.mg_map,
                    &mut self.pd_ctx,
                    mg_idx,
                    /*pre_assignment_buffer_first_pass_flag=*/ first,
                    /*idr_flag=*/ intra_member,
                    /*cra_flag=*/ false,
                    /*init_pred_struct_position_flag=*/ first && init_pos_flag,
                    /*init_pic_index=*/ 0,
                );
                if intra_member {
                    // `svt_aom_is_delayed_intra` (`pd_process.c:3620-3635`),
                    // stamped where C computes it — before `init_pic_settings`'s
                    // `copy_tf_params` reads it. An IDR in RA with a live
                    // intra period is held for the next release unless it IS
                    // the sequence's last input (`end_of_sequence_flag`).
                    pic.is_delayed_intra = pp::is_delayed_intra(
                        /*idr_flag=*/ true,
                        /*cra_flag=*/ false,
                        seq.pred_structure,
                        self.gop.intra_period as i32,
                        self.ra_input[pic_idx].is_eos,
                        self.enc_pic.pre_assignment_buffer_count,
                        pic.pred_struct_entry_count,
                    );
                }
                // `pd_process.c:5563-5590`'s slice-type switch: an I slice
                // RESETS `elapsed_non_cra_count` (and, for an IDR, stamps
                // `ctx->key_poc`); a B slice bumps both elapsed counters,
                // clipped. (`elapsed_non_idr_count` has no reader in the
                // ported subset; the CRA one feeds the position walk.)
                if pic.slice_type == pp::SliceType::I {
                    self.enc_pic.elapsed_non_cra_count = 0;
                    if intra_member {
                        self.pd_ctx.key_poc = pic.picture_number;
                    }
                } else {
                    self.enc_pic.elapsed_non_cra_count =
                        self.enc_pic.elapsed_non_cra_count.saturating_add(1);
                }
                // `pd_process.c:5548/4869/5559`: position AFTER the walk
                // indexes the entry; the entry carries the temporal layer.
                let pos = self.enc_pic.pred_struct_position as usize;
                pred_struct_index.push(pos as u8);
                pic.temporal_layer_index =
                    pp::PRED_STRUCT_TEMPORAL_LAYER[usize::from(pic.hierarchical_levels)][pos];
                pic_idx_in_mg.push(pp::get_pic_idx_in_mg(
                    &mut pic,
                    &seq,
                    &self.enc_pic,
                    &self.mg_map,
                    pic_idx as u32,
                    mg_idx,
                ));
                pics[pic_idx] = Some(pic);
            }

            // ---- Pass 2, DISPLAY order (`pd_process.c:5612-5667`) ----
            // `picture_number_alt++` every picture; the RA permutation arm
            // needs a COMPLETE mini-GOP (`length == entry_count`) with no
            // intra inside; everything else falls to the alt counter.
            for pic_idx in start..=end {
                let pic = pics[pic_idx].as_mut().unwrap();
                let alt = self.enc_pic.picture_number_alt;
                self.enc_pic.picture_number_alt += 1;
                pic.decode_order = if self.mg_map.idr_count[mg_idx] == 0
                    && self.mg_map.length[mg_idx] == pic.pred_struct_entry_count
                    && seq.pred_structure == pp::PredStructure::RandomAccess
                {
                    self.enc_pic.decode_base_number
                        + u64::from(
                            pp::PRED_STRUCT_DECODE_ORDER[usize::from(pic.hierarchical_levels)]
                                [usize::from(pred_struct_index[pic_idx - start])],
                        )
                } else {
                    alt
                };
            }
            // `pd_process.c:5660-5662`: decode base advances by the mini-GOP
            // length (overlays are off in this envelope — `has_overlay = 0`).
            self.enc_pic.decode_base_number += u64::from(mg_len);

            // ---- `store_mg_picture_arrays` + pass 3, DECODE order ----
            let decode_orders: alloc::vec::Vec<u64> = (start..=end)
                .map(|i| pics[i].as_ref().unwrap().decode_order)
                .collect();
            let (decode_perm, _display_perm) = pp::store_mg_picture_arrays(&decode_orders);
            // C `ref_pa_pic_ptr_array[list][0]`'s `avg_luma` — the PA
            // reference object's luma mean (`pic_analysis_process.c:2003`),
            // resolved for `get_similar_ref_brightness` inside
            // `init_pic_settings`. A reference can name two kinds of
            // picture here: an in-window member (its `ra_stats` slot — the
            // same gather `avg_luma` was stamped from) or a prior window's
            // coded picture (the DPB-mirrored `pa_slots` pyramid).
            // `INVALID_LUMA` when the reference can't be resolved — which
            // is also C's answer whenever `calc_hist` is off, since every
            // arm is gated upstream. Scoped to pass 3 so the field borrows
            // end before pass 4's `&mut self` filter prep.
            let pa_luma = |poc: u64, slot: usize| -> u64 {
                if let Some(i) = self.ra_input.iter().position(|f| f.display_order == poc) {
                    return self
                        .ra_stats
                        .get(i)
                        .and_then(|s| s.as_deref())
                        .map_or(pp::INVALID_LUMA, |s| s.avg_luma);
                }
                // The delayed intra (C's `prev_delayed_intra`) is a viable
                // reference target for the NEXT release's members — the
                // new GOP's L0 anchor. Its PA mean sits on `DelayedIntra`
                // before pass 4's `mctf_frame`, then on the staged output's
                // `PicParams` (`pcs->avg_luma` — stamped at member
                // construction from the same gather) after the hold moves.
                if let Some(d) = self
                    .delayed_intra
                    .as_ref()
                    .filter(|d| d.frame.display_order == poc)
                {
                    return d.stats.avg_luma;
                }
                if let Some((_, p)) = self
                    .delayed_intra_out
                    .as_ref()
                    .filter(|(_, p)| p.picture_number == poc)
                {
                    return p.avg_luma;
                }
                self.pa_slots
                    .get(slot)
                    .and_then(|s| s.as_deref())
                    .filter(|p| p.picture_number == poc)
                    .map_or(pp::INVALID_LUMA, |p| p.avg_luma)
            };
            for &local in &decode_perm {
                let pic_idx = start + local;
                let pic = pics[pic_idx].as_mut().unwrap();
                pp::picture_decision_per_picture(
                    pic,
                    &seq,
                    &mut self.pd_ctx,
                    pic_idx_in_mg[local],
                    mg_idx,
                    &pa_luma,
                )
                .map_err(|_| {
                    whereat::at!(EncodeError::UnsupportedConfig(
                        "this GOP shape's reference structure is not implemented: every \
                         top-level branch of C's av1_generate_rps_info is translated, \
                         so this is a case C itself rejects — LD-CBR outside \
                         hierarchical levels 1-2, or a mini-GOP position outside \
                         the ported tables [C: logs the same config as an error]",
                    ))
                })?;
                // `pd_process.c:5151-5154`: the delayed intra's send is
                // replaced by the `ctx->prev_delayed_intra` hold — its
                // packet emits inside the NEXT release.
                if !pic.is_delayed_intra {
                    emit.push(pic_idx);
                }
            }

            // ---- Pass 4, DISPLAY order (`pd_process.c:5090-5132`) ----
            // `process_pics`' MCTF loop: per-picture `filt_to_unfilt_diff`
            // inheritance, `derive_tf_window_params` (noise selection,
            // reference-count modulation, window assembly), the FILTER
            // itself (`mctf_frame` -> `svt_av1_init_temporal_filtering`)
            // and the publish carry.
            self.run_ra_tf_prep(&mut pics, start, end, &mut frames_tf);
        }
        Ok((pics, emit, frames_tf))
    }

    /// `svt_aom_gathering_picture_statistics` for one buffered RA input
    /// (`pic_analysis_process.c:1960-2006`): pad the luma to aligned dims,
    /// decimate to the 1/16 plane, then gather the region histograms and
    /// `avg_luma` the TF window's `calc_ahd` reads — and, now that
    /// `calc_hist` is always on in RA, the scene-transition detector's
    /// per-region histograms and intensities.
    pub(super) fn gather_tf_stats_frame(
        &self,
        frame: &RaBufferedFrame,
    ) -> crate::port_preanalysis::PictureStatistics {
        let (tw, th) = (self.true_width as usize, self.true_height as usize);
        let (aw, ah) = (self.width as usize, self.height as usize);
        // `svt_aom_pad_input_pictures` + `downsample_filtering_input_picture`:
        // the LIVE route decimates the ALIGNED padded luma twice by 2
        // (`PaPicture::from_source` owns that exact chain).
        let y_pad = if aw == tw && ah == th {
            frame.y.clone()
        } else {
            pad_plane_replicate(&frame.y, tw, tw, th, aw, ah)
                .expect("aligned dims are never smaller than true dims")
        };
        let pa =
            crate::inter_me_arm::PaPicture::from_source(&y_pad, aw, aw, ah, frame.display_order);
        // `scs->picture_analysis_number_of_regions_per_*` keys on
        // `max_input_luma_*` AFTER the `MIN_BLOCK_SIZE` pad — the aligned
        // dims (`enc_handle.c:4392-4397`).
        let regions_per_width = if self.width >= 64 { 4 } else { 1 };
        let regions_per_height = if self.height >= 64 { 4 } else { 1 };
        let mut stats = crate::port_preanalysis::PictureStatistics::default();
        crate::port_preanalysis::gathering_picture_statistics(
            /*calc_hist=*/ true,
            /*calculate_variance=*/ false,
            regions_per_width,
            regions_per_height,
            /*scene_change_detection=*/ false,
            &pa.sixteenth.buf[pa.sixteenth.org..],
            pa.sixteenth.stride,
            pa.sixteenth.width,
            pa.sixteenth.height,
            None,
            &mut stats,
        );
        stats
    }

    /// C `process_pics`' display-order MCTF loop (`pd_process.c:5090-5132`)
    /// for one mini-GOP, minus the filter dispatch itself.
    ///
    /// Per picture, in display order:
    /// 1. inherit `ctx->filt_to_unfilt_diff` (`:5122`);
    /// 2. `mctf_frame` — here: `derive_tf_window_params`, i.e. the noise
    ///    selection/carry, `ref_pics_modulation`, window counts and the
    ///    `temp_filt_pcs_list` assembly (`:4194-4241`); when `tf_ctrls` is
    ///    off the picture's `do_tf` is cleared (`:4239`) — no window is
    ///    built, which is the port's equivalent;
    /// 3. `is_noise_level` is stamped on EVERY picture (`:4240-4241`);
    /// 4. an I slice publishes its filt/unfilt difference back to the
    ///    context (`:5124`).
    ///
    /// The delayed-intra slot (`:5103-5110`): an `is_delayed_intra` member
    /// skips this loop — its `mctf_frame` runs inside the NEXT release's
    /// `process_pics` via `filter_delayed_intra`, exactly C's
    /// `prev_delayed_intra` ordering. `gm_pp_enabled`'s base-layer toggle
    /// (`:5117-5120`) has no ported consumer yet and is noted rather than
    /// reproduced.
    /// The `RaTfScs` bundle `run_ra_tf_prep` builds — extracted so the held
    /// key frame's lone-release path (EOS, or a back-to-back intra) runs the
    /// same scaffolding.
    pub(super) fn ra_tf_scs(&self) -> crate::port_tf_driver::RaTfScs {
        // Same `max_input_luma_*` (post-`MIN_BLOCK_SIZE`-pad) key as the
        // gather — `enc_handle.c:4392-4397`.
        let regions_per_width = if self.width >= 64 { 4 } else { 1 };
        let regions_per_height = if self.height >= 64 { 4 } else { 1 };
        // `derive_vq_params`' `vq_ctrls.sharpness_ctrls.tf`
        // (`enc_handle.c:3277-3293`): on for the subjective tunes only.
        let vq_sharpness_tf =
            matches!(self.hdr.tune, 0 | 5) || (self.hdr.alt_ssim_tuning && self.hdr.tune == 2);
        // `scs->calculate_variance` (`enc_handle.c:4361-4365`) for the RA
        // envelope: `allintra`, `rtc` and `scene_change_detection` are all
        // off here, `aq_mode != 0` is refused upstream.
        let calculate_variance = vq_sharpness_tf || self.hdr.enable_variance_boost;
        let (seg_rows, seg_cols) =
            crate::port_tf_driver::tf_segment_counts(self.width, self.height);
        crate::port_tf_driver::RaTfScs {
            bit_depth: self.bit_depth,
            qp: u32::from(self.rc_config.qp),
            enable_tf: u8::from(self.enable_tf),
            tf_strength: self.hdr.tf_strength,
            // `SVT_HDR_MODE` (temporal_filtering.c:2785/3320): the fork's
            // `kf_tf_strength` replaces the mainline kf arm entirely. `None`
            // in mainline mode so a stray configured value can't leak in —
            // mirrors the compile-time `#if`.
            kf_tf_strength: self.hdr.is_fork().then_some(self.hdr.kf_tf_strength),
            tf_ref_qp_based_th_scaling: self.speed_config.preset > -1,
            vq_sharpness_tf,
            calculate_variance,
            compute_psnr: false,
            compute_ssim: false,
            enc_mode: self.speed_config.preset,
            sb_size: self.sb_size as i32,
            input_resolution: crate::port_enc_mode_config::ResolutionRange::from_luma_area(
                self.width * self.height,
            ),
            regions_per_width,
            regions_per_height,
            tf_segment_row_count: seg_rows,
            tf_segment_column_count: seg_cols,
            true_width: self.true_width,
            true_height: self.true_height,
            aligned_width: self.width,
            aligned_height: self.height,
        }
    }

    /// `ctx->prev_delayed_intra`'s `mctf_frame` (`pd_process.c:5103-5132`),
    /// run at the head of the FOLLOWING release's `process_pics` — ahead of
    /// the member loop so its `:5124` publish lands before any member
    /// inherits `:5122`. The future members C pulls from the released
    /// buffer's `mg_pictures_array` are this release's `mg_len` `frames`;
    /// `mg_len == 0` is the lone-release case (EOS), where C's buffer was
    /// just as empty. The filtered buffers land in `delayed_intra_out` for
    /// `encode_ra_window` to emit ahead of the member packets.
    pub(super) fn filter_delayed_intra(
        &mut self,
        scs_tf: &crate::port_tf_driver::RaTfScs,
        q_weight: u32,
        q_weight_denom: u32,
        frames: &mut alloc::vec::Vec<crate::port_tf_driver::TfPicBufs>,
        first_future_hier: Option<u8>,
        regions_per_width: usize,
        regions_per_height: usize,
        mg_len: usize,
    ) {
        use crate::port_picstruct as pp;
        let Some(key) = self.delayed_intra.take() else {
            return;
        };
        // The member's decision IS the pcs C carries — slice I, delayed
        // intra, decided in its own release.
        let mut pic = key.pic;
        // `derive_tf_window_params`' key-frame pred-structure fixup
        // (`pd_process.c:3936-3943`): when the first future picture's
        // hierarchy differs from the delayed intra's, the intra adopts the
        // member's level BEFORE the `tf_max_ref_per_struct` cap is taken.
        if let Some(hier) = first_future_hier
            && hier != pic.hierarchical_levels
        {
            pic.hierarchical_levels = hier;
        }
        // `copy_tf_params` (`pd_process.c:4468-4497`): an RA intra with a live
        // intra period is a DELAYED intra — `tf_params_per_type[0]`, not the
        // BASE row.
        let (_tf_level, table) = pp::derive_tf_params(
            self.pred_structure,
            self.speed_config.preset,
            self.gop.hierarchical_levels,
            self.enable_tf,
            /*lossless=*/ false,
        );
        pic.tf_ctrls = match pp::copy_tf_params(
            self.pred_structure,
            pic.slice_type,
            pic.is_key_frame,
            pic.temporal_layer_index,
            pic.hierarchical_levels,
            /*is_overlay=*/ false,
            self.enable_tf_key,
            pic.is_delayed_intra,
        ) {
            pp::TfParamsChoice::DelayedIntra => table[0],
            pp::TfParamsChoice::Base => table[1],
            pp::TfParamsChoice::L1 => table[2],
            pp::TfParamsChoice::Disabled => pp::TfCtrls::default(),
        };
        // `:5122` — inherit the carried filt/unfilt difference.
        pp::tf_inherit_filt_to_unfilt_diff(&self.pd_ctx, &mut pic);
        let ctrls = pic.tf_ctrls;
        if ctrls.enabled {
            let (tw, th) = (self.true_width as usize, self.true_height as usize);
            let cw = tw.div_ceil(2);
            // `derive_tf_window_params`' noise half (`:3755-3849`). An I
            // slice always estimates fresh (`do_noise_est` is true whenever
            // `is_i_slice`) and publishes to the carry slot.
            let fresh_y = Some(crate::port_temporal_filtering::noise_log1p_fp16(
                crate::temporal_filter::estimate_noise_fp16(&key.frame.y, tw, th, tw),
            ));
            let fresh_uv = if ctrls.chroma_lvl != 0 {
                [
                    crate::port_temporal_filtering::noise_log1p_fp16(
                        crate::temporal_filter::estimate_noise_fp16(
                            &key.frame.u,
                            tw >> 1,
                            th >> 1,
                            cw,
                        ),
                    ),
                    crate::port_temporal_filtering::noise_log1p_fp16(
                        crate::temporal_filter::estimate_noise_fp16(
                            &key.frame.v,
                            tw >> 1,
                            th >> 1,
                            cw,
                        ),
                    ),
                ]
            } else {
                [0, 0]
            };
            let noise = pp::tf_window_noise(
                ctrls.use_intra_for_noise_est,
                /*is_i_slice=*/ true,
                ctrls.chroma_lvl != 0,
                fresh_y,
                fresh_uv,
                &mut self.pd_ctx.last_i_noise_levels_log1p_fp16,
            );
            // `:3850` — reference-count modulation.
            let offset = if ctrls.modulate_pics != 0 {
                pp::ref_pics_modulation(
                    /*is_i_slice=*/ true,
                    /*temporal_layer_index=*/ 0,
                    &ctrls,
                    noise.levels_log1p_fp16[0],
                    pic.filt_to_unfilt_diff,
                    q_weight,
                    q_weight_denom,
                )
            } else {
                0
            };
            // `pcs->idr_flag` arm of `derive_tf_window_params`
            // (`:3964-3998`): centre at slot 0, future from the released
            // buffer's `mg_pictures_array` — this release's `mg_len`
            // members (`ra_input[..mg_len]` while they still sit in the
            // queue, `frames` for their bufs).
            let empty_hist: alloc::boxed::Box<pp::RegionHistograms> =
                alloc::boxed::Box::new([[[0u32; 256]; 4]; 4]);
            let mut key_cands: alloc::vec::Vec<pp::TfWindowCand<'_>> =
                alloc::vec::Vec::with_capacity(mg_len + 1);
            key_cands.push(pp::TfWindowCand {
                picture_number: key.frame.display_order,
                frame_width: self.true_width,
                frame_height: self.true_height,
                hierarchical_levels: pic.hierarchical_levels,
                avg_luma: key.stats.avg_luma,
                picture_histogram: &key.stats.picture_histogram,
            });
            key_cands.extend((0..mg_len).map(|i| {
                let st = self.ra_stats.get(i).and_then(|s| s.as_deref());
                pp::TfWindowCand {
                    picture_number: self.ra_input[i].display_order,
                    frame_width: self.true_width,
                    frame_height: self.true_height,
                    // Only the delayed-intra pred-structure fixup reads this
                    // field, and only on the poc+1 member — which
                    // `first_future_hier` already carries.
                    hierarchical_levels: first_future_hier.unwrap_or(self.gop.hierarchical_levels),
                    avg_luma: st.map_or(crate::port_preanalysis::INVALID_LUMA, |s| s.avg_luma),
                    picture_histogram: st.map_or(&*empty_hist, |s| &s.picture_histogram),
                }
            }));
            // `pd_process.c:3922-3965` — the delayed-intra arm: future
            // pictures poc-matched out of the released buffer (the
            // `mg_pictures_array` that follows the held intra), capped by
            // `tf_max_ref_per_struct` on the post-fixup hierarchy.
            let counts = pp::derive_tf_window_counts(
                pp::TfWindowArm::DelayedIntra,
                &ctrls,
                offset,
                u32::from(pic.hierarchical_levels),
                pic.temporal_layer_index,
            );
            let window = pp::assemble_tf_window(
                pp::TfWindowArm::DelayedIntra,
                &counts,
                /*centre_idx=*/ 0,
                &key_cands,
                /*mg_lo=*/ 0,
                /*mg_hi=*/ key_cands.len(),
                /*ld_past=*/ &[],
                /*avail_past=*/ 0,
                regions_per_width,
                regions_per_height,
            );
            pic.noise_levels_log1p_fp16 = noise.levels_log1p_fp16;
            pic.past_altref_nframes = window.past_altref_nframes as u8;
            pic.future_altref_nframes = window.future_altref_nframes as u8;
            pic.tf_avg_luma = window.tf_avg_luma;
            pic.tf_avg_ahd_error = window.tf_avg_ahd_error;
            pic.tf_window = Some(alloc::boxed::Box::new(window));
            // The window's member indexes address the combined buffer view:
            // the intra at 0, this release's members after it.
            frames.insert(0, key.bufs);
            let mut member_stats: alloc::vec::Vec<
                Option<alloc::boxed::Box<crate::port_preanalysis::PictureStatistics>>,
            > = alloc::vec::Vec::with_capacity(mg_len + 1);
            member_stats.push(Some(alloc::boxed::Box::new(key.stats)));
            member_stats.extend(self.ra_stats.iter().take(mg_len).cloned());
            let out = crate::port_tf_driver::ra_mctf_filter(
                scs_tf,
                /*centre_slot=*/ 0,
                &pic,
                &member_stats,
                frames,
            );
            self.pd_ctx.tf_motion_direction = out.motion_direction;
            if let Some(diff) = out.filt_to_unfilt_diff {
                pic.filt_to_unfilt_diff = diff;
            }
            let bufs = frames.remove(0);
            // `:4240-4241` — stamped on every picture, TF enabled or not.
            pic.is_noise_level =
                self.pd_ctx.last_i_noise_levels_log1p_fp16[0] >= pp::VQ_NOISE_LVL_TH;
            // `:5124` — an I slice publishes its measured difference.
            pp::tf_publish_filt_to_unfilt_diff(&mut self.pd_ctx, &pic);
            self.delayed_intra_out = Some((bufs, pic));
        } else {
            // Disabled ctrls still take the stamp + publish (`:4240`, `:5124`);
            // the bufs pass through unfiltered.
            pic.is_noise_level =
                self.pd_ctx.last_i_noise_levels_log1p_fp16[0] >= pp::VQ_NOISE_LVL_TH;
            pp::tf_publish_filt_to_unfilt_diff(&mut self.pd_ctx, &pic);
            self.delayed_intra_out = Some((key.bufs, pic));
        }
    }

    /// Emit the delayed intra's packet — the filtered (or pass-through)
    /// aligned planes, C's `send_picture_out(ctx->prev_delayed_intra)`
    /// (`pd_process.c:5129`). The decision ran in the intra's own release
    /// as a buffer member, so it arrives as `decided` like any window
    /// picture; re-running it here would advance the `pd_ctx` toggles a
    /// second time.
    pub(super) fn encode_delayed_intra(&mut self) -> EncodeResult<Vec<u8>> {
        let Some((bufs, pic)) = self.delayed_intra_out.take() else {
            return Ok(alloc::vec::Vec::new());
        };
        let aw = self.width as usize;
        let y_f = bufs.extract_luma();
        let u_f = bufs.extract_u();
        let v_f = bufs.extract_v();
        let tpl = self.delayed_intra_tpl.take();
        self.encode_frame_impl(
            &y_f,
            aw,
            Some((&u_f, &v_f)),
            Some(FrameDecision {
                display_order: pic.picture_number,
                pic,
                tpl,
            }),
        )
    }

    pub(super) fn run_ra_tf_prep(
        &mut self,
        pics: &mut [Option<crate::port_picstruct::PicParams>],
        mg_lo: usize,
        mg_hi: usize,
        frames: &mut alloc::vec::Vec<crate::port_tf_driver::TfPicBufs>,
    ) {
        use crate::port_picstruct as pp;
        let (tw, th) = (self.true_width as usize, self.true_height as usize);
        let cw = tw.div_ceil(2);
        // `scs->picture_analysis_number_of_regions_per_*` — aligned dims
        // (`enc_handle.c:4392-4397`).
        let regions_per_width = if self.width >= 64 { 4 } else { 1 };
        let regions_per_height = if self.height >= 64 { 4 } else { 1 };
        let scs_tf = self.ra_tf_scs();
        // `set_qp_based_th_scaling_ctrls_default` (enc_handle.c:3785-3816):
        // `tf_ref_qp_based_th_scaling` is off only at ENC_MR (preset -1).
        let (q_weight, q_weight_denom) =
            crate::port_enc_mode_config::me::get_qp_based_th_scaling_factors(
                self.speed_config.preset > -1,
                u32::from(self.rc_config.qp),
            );

        // `ctx->prev_delayed_intra`'s `mctf_frame` + `send_picture_out`
        // run at the head of THIS release's `process_pics`
        // (`pd_process.c:5103-5131`) — ahead of the member loop, so the
        // intra's `filt_to_unfilt_diff` publish and noise carry land before
        // the members inherit `:5122`, and `delayed_intra_out` stages for
        // `encode_ra_window`'s emit-ahead.
        self.filter_delayed_intra(
            &scs_tf,
            q_weight,
            q_weight_denom,
            frames,
            pics[0].as_ref().map(|p| p.hierarchical_levels),
            regions_per_width,
            regions_per_height,
            // The intra's future window is this release's `mg_pictures_array`
            // — the `pics` member count, not the deeper queue `frames`
            // covers.
            pics.len(),
        );

        // The candidate view `assemble_tf_window` searches: every buffered
        // input in display order. C's `mg_pictures_array` / reorder queue /
        // pre-assignment buffer are all entries of this one buffer under the
        // port's driver.
        let empty_hist: alloc::boxed::Box<pp::RegionHistograms> =
            alloc::boxed::Box::new([[[0u32; 256]; 4]; 4]);
        let cands: alloc::vec::Vec<pp::TfWindowCand> = (0..self.ra_input.len())
            .map(|i| {
                let st = self.ra_stats.get(i).and_then(|s| s.as_deref());
                pp::TfWindowCand {
                    picture_number: self.ra_input[i].display_order,
                    frame_width: self.true_width,
                    frame_height: self.true_height,
                    // Members carry their decided level; spillover entries
                    // deeper in the queue have no decision yet — C's pcs
                    // ctor value, `scs->static_config.hierarchical_levels`.
                    hierarchical_levels: pics
                        .get(i)
                        .and_then(|o| o.as_ref())
                        .map_or(self.gop.hierarchical_levels, |p| p.hierarchical_levels),
                    avg_luma: st.map_or(crate::port_preanalysis::INVALID_LUMA, |s| s.avg_luma),
                    picture_histogram: st.map_or(&*empty_hist, |s| &s.picture_histogram),
                }
            })
            .collect();
        let mg_pocs: alloc::vec::Vec<u64> = (mg_lo..=mg_hi)
            .map(|i| self.ra_input[i].display_order)
            .collect();

        for pic_idx in mg_lo..=mg_hi {
            let (is_delayed_intra, tf_enabled, is_i, tl, hier, poc, ctrls, is_key) = {
                let p = pics[pic_idx].as_ref().unwrap();
                (
                    p.is_delayed_intra,
                    p.tf_ctrls.enabled,
                    p.slice_type == pp::SliceType::I,
                    p.temporal_layer_index,
                    p.hierarchical_levels,
                    p.picture_number,
                    p.tf_ctrls,
                    p.is_key_frame,
                )
            };
            // `pd_process.c:5115`: delayed intra runs through the
            // `prev_delayed_intra` slot, not this loop.
            if is_delayed_intra {
                continue;
            }
            // `:5122` — inherit the carried filt/unfilt difference.
            pp::tf_inherit_filt_to_unfilt_diff(&self.pd_ctx, pics[pic_idx].as_mut().unwrap());
            if tf_enabled {
                // `derive_tf_window_params`' noise half (`:3755-3849`).
                // 8-bit arm only: the RA driver is the `try_encode_frame_420`
                // (u8) path; the highbd estimator is ported for when a 10-bit
                // RA entry exists.
                let do_est = !ctrls.use_intra_for_noise_est || is_i;
                let fresh_y = if do_est {
                    let f = &self.ra_input[pic_idx];
                    Some(crate::port_temporal_filtering::noise_log1p_fp16(
                        crate::temporal_filter::estimate_noise_fp16(&f.y, tw, th, tw),
                    ))
                } else {
                    None
                };
                let fresh_uv = if ctrls.chroma_lvl != 0 {
                    let f = &self.ra_input[pic_idx];
                    // `pd_process.c:3826-3841`: the estimate runs on
                    // `width >> ss_x` (FLOOR) — differs from the packed
                    // plane's `div_ceil` stride only at odd widths.
                    [
                        crate::port_temporal_filtering::noise_log1p_fp16(
                            crate::temporal_filter::estimate_noise_fp16(&f.u, tw >> 1, th >> 1, cw),
                        ),
                        crate::port_temporal_filtering::noise_log1p_fp16(
                            crate::temporal_filter::estimate_noise_fp16(&f.v, tw >> 1, th >> 1, cw),
                        ),
                    ]
                } else {
                    [0, 0]
                };
                let noise = pp::tf_window_noise(
                    ctrls.use_intra_for_noise_est,
                    is_i,
                    ctrls.chroma_lvl != 0,
                    fresh_y,
                    fresh_uv,
                    &mut self.pd_ctx.last_i_noise_levels_log1p_fp16,
                );
                // `:3850` — reference-count modulation (qp-scaled when the
                // control table's `qp_opt` is set).
                let filt_diff = pics[pic_idx].as_ref().unwrap().filt_to_unfilt_diff;
                let offset = if ctrls.modulate_pics != 0 {
                    pp::ref_pics_modulation(
                        is_i,
                        tl,
                        &ctrls,
                        noise.levels_log1p_fp16[0],
                        filt_diff,
                        q_weight,
                        q_weight_denom,
                    )
                } else {
                    0
                };
                // `pcs->idr_flag` selects the IDR arm; a buffered I slice is
                // an IDR in this envelope (CRAs are refused upstream).
                let arm = if is_key {
                    pp::TfWindowArm::RandomAccessIdr
                } else {
                    pp::TfWindowArm::RandomAccessInter
                };
                let counts = pp::derive_tf_window_counts(arm, &ctrls, offset, u32::from(hier), tl);
                let avail_past = pp::avail_past_pictures(&mg_pocs, poc);
                let window = pp::assemble_tf_window(
                    arm,
                    &counts,
                    pic_idx,
                    &cands,
                    mg_lo,
                    mg_hi + 1,
                    // `pd_ctx->tf_pic_array` — empty under random access.
                    &[],
                    avail_past,
                    regions_per_width,
                    regions_per_height,
                );
                let pic = pics[pic_idx].as_mut().unwrap();
                pic.noise_levels_log1p_fp16 = noise.levels_log1p_fp16;
                if let Some(lvl) = window.hier_fixup {
                    pic.hierarchical_levels = lvl;
                }
                pic.past_altref_nframes = window.past_altref_nframes as u8;
                pic.future_altref_nframes = window.future_altref_nframes as u8;
                pic.tf_avg_luma = window.tf_avg_luma;
                pic.tf_avg_ahd_error = window.tf_avg_ahd_error;
                pic.tf_window = Some(alloc::boxed::Box::new(window));
            }
            if tf_enabled {
                // `mctf_frame`'s second half — `svt_av1_init_temporal_filtering`
                // itself (`pd_process.c:5122`'s `mctf_frame` call). It MUST
                // sit inside this display-order loop, before the publish at
                // `:5124`: an I slice's measured `filt_to_unfilt_diff` has
                // to reach `pic.filt_to_unfilt_diff` here or the next
                // picture inherits the stale carried value at `:5122`. The
                // filtered pixels land in `frames[pic_idx]`, which the
                // encode loop substitutes for the source.
                let out = crate::port_tf_driver::ra_mctf_filter(
                    &scs_tf,
                    pic_idx,
                    pics[pic_idx].as_ref().unwrap(),
                    &self.ra_stats,
                    frames,
                );
                self.pd_ctx.tf_motion_direction = out.motion_direction;
                if let Some(diff) = out.filt_to_unfilt_diff {
                    pics[pic_idx].as_mut().unwrap().filt_to_unfilt_diff = diff;
                }
            }
            let pic = pics[pic_idx].as_mut().unwrap();
            // `:4240-4241` — stamped on every picture, TF enabled or not.
            pic.is_noise_level =
                self.pd_ctx.last_i_noise_levels_log1p_fp16[0] >= pp::VQ_NOISE_LVL_TH;
            // `:5124` — only an I slice publishes its measured difference.
            pp::tf_publish_filt_to_unfilt_diff(&mut self.pd_ctx, pic);
        }
    }
}
