use super::*;

impl EncodePipeline {
    /// The LD+CBR frame qindex and per-SB plan — C's `RC_INPUT` task body for
    /// `rc_cfg.mode == AOM_CBR` (rc_process.c:833-877):
    /// `rc_init_frame_stats` → `svt_av1_rc_process_rate_allocation` →
    /// `svt_av1_rc_calc_qindex_rate_control`, then `generate_sb_qindex`'s
    /// CBR arm (`svt_av1_rc_init_sb_qindex`, rc_aq.c:879-885).
    ///
    /// Returns `(base_qindex, frame_rc, sb_plan)`. `frame_rc` is the PPCS
    /// half the post-encode update consumes — carry it to
    /// [`Self::cbr_postencode`]. `sb_plan` is `Some` only when cyclic
    /// refresh armed; `None` is C's flat arm (every SB takes the frame
    /// `base_q_idx`, `delta_q_present` stays 0).
    pub(super) fn cbr_frame_qindex(
        &mut self,
        pic: Option<&crate::port_picstruct::PicParams>,
        is_key: bool,
        display_order: u64,
        frame_hier: u8,
        sc_class1: bool,
        frame_me: Option<&crate::inter_me_arm::FrameMe>,
    ) -> crate::EncodeResult<(
        u8,
        crate::port_rc_vbr_cbr_state::FrameRc,
        Option<crate::sb_qindex::SbQindexPlan>,
    )> {
        // `svt_aom_set_rc_param` + `set_param_based_on_input`, built once from
        // the encode config. The config gate admits CBR only on LOW_DELAY,
        // which is also the envelope `RcVbrCbr::new_cbr` documents.
        if self.rc_vbr_cbr.is_none() {
            self.rc_vbr_cbr = Some(crate::port_rc_driver::RcVbrCbr::new_cbr(
                &self.rc_config,
                self.width,
                self.height,
                self.bit_depth,
                // C `scs->static_config.intra_period_length` is the CLI
                // `--intra-period` MINUS one (the app prints length+1); -1
                // is C's "no periodic intra" sentinel.
                if self.gop.intra_period == 0 {
                    -1
                } else {
                    self.gop.intra_period as i32 - 1
                },
                frame_hier,
                self.sb_size as u16,
                (self.width as usize).div_ceil(self.sb_size) as u16,
                (self.height as usize).div_ceil(self.sb_size) as u16,
            ));
        }
        let rcs = self.rc_vbr_cbr.as_mut().unwrap();
        let b64_count = (self.width.div_ceil(64) * self.height.div_ceil(64)) as u16;
        let mut frame = crate::port_rc_driver::frame_rc(
            pic,
            is_key,
            display_order,
            self.width,
            self.height,
            self.upscaled_width,
            b64_count,
            frame_hier,
            sc_class1,
        );
        // `pcs->me_64x64_distortion[]` — the RC path's copy of the open-loop
        // per-b64 distortions. Empty on a key frame, where ME never runs;
        // `rc_init_frame_stats` then leaves both averages alone, exactly as
        // C does on an I_SLICE.
        let me_64x64_dist: alloc::vec::Vec<u32> = frame_me.map_or_else(alloc::vec::Vec::new, |m| {
            m.per_b64.iter().map(|b| b.me_64x64_distortion).collect()
        });
        let slice_type = if is_key {
            crate::port_rc_process::SliceType::I
        } else {
            crate::port_rc_process::SliceType::B
        };
        let ref_l0_stats = pic
            .filter(|p| p.ref_list0_count_try > 0)
            .and_then(|p| ref_obj_stats(&self.dpb, p.rps.ref_dpb_index[0] as usize));
        let ref_l1_stats = pic
            .filter(|p| p.ref_list1_count_try > 0)
            .and_then(|p| ref_obj_stats(&self.dpb, p.rps.ref_dpb_index[4] as usize));
        // C `rc_init_frame_stats` (rc_process.c:836 → :604). Of its outputs
        // only `avg_base_me_dist` is consumed on this path — the three
        // ref-object percentages are the port's separately-computed
        // `ref_obj_stats`/`md_ref_intra_percentage` inputs, and
        // `rate_average_periodin_frames` is a two-pass field the ported CBR
        // arm never reads.
        let stats =
            crate::port_rc_process::rc_init_frame_stats(&crate::port_rc_process::FrameStatsInput {
                slice_type,
                ref_list1_count_try: pic.map_or(0, |p| p.ref_list1_count_try),
                ref_l0: ref_l0_stats.as_ref(),
                ref_l1: ref_l1_stats.as_ref(),
                passes: 1,
                max_bit_rate: u64::from(self.rc_config.max_bitrate) * 1000,
                total_stats_count: 0,
                me_64x64_distortion: &me_64x64_dist,
            });
        if let Some(avg) = stats.avg_base_me_dist {
            rcs.rc.prev_avg_base_me_dist = rcs.rc.cur_avg_base_me_dist;
            rcs.rc.cur_avg_base_me_dist = avg;
        }
        // C `svt_av1_rc_process_rate_allocation` (rc_process.c:857 →
        // rc_vbr_cbr.c). The two `FnOnce` are VBR's `svt_aom_process_rc_stat`
        // / `av1_set_target_rate` pair — unreachable under AOM_CBR and wired
        // to nothing because first-pass stats are not ported.
        let bw = rcs.frame_bandwidth();
        let (best_q, worst_q) = rcs.best_worst_allowed_q();
        let mode = rcs.cfg.mode;
        let hier = i32::from(rcs.scs.hierarchical_levels);
        crate::port_rc_vbr_cbr_update::process_rate_allocation(
            &mut rcs.rc,
            &rcs.cfg,
            &rcs.scs,
            &mut frame,
            // `ppcs->tpl_ctrls.enable` is 0 under LOW_DELAY (`get_tpl`), the
            // only structure CBR is admitted on — `TplCtrlsRc::default()`
            // says exactly that.
            &crate::port_rc_vbr_cbr_qpick::TplCtrlsRc::default(),
            &mut rcs.resize_pending,
            crate::port_rc_vbr_cbr_update::RtResizeMode::None,
            false,
            |rc| crate::port_rc_driver::apply_rc_init(rc, mode, best_q, worst_q, hier, bw),
            |_rc, _f| {},
            |_rc, _f| {},
        );
        // `pcs->ref_pic_ptr_array` — C fills list 0 from
        // `ref_dpb_index[LAST..=GOLD]` (rps indices 0..4) and list 1 from
        // `[BWD..=ALT]` (4..7); under LD the second list is empty.
        let mut l0: alloc::vec::Vec<crate::port_rc_vbr_cbr_qpick::RefPicRc> =
            alloc::vec::Vec::new();
        let mut l1: alloc::vec::Vec<crate::port_rc_vbr_cbr_qpick::RefPicRc> =
            alloc::vec::Vec::new();
        if let Some(p) = pic {
            for i in 0..usize::from(p.ref_list0_count_try) {
                if let Some(rf) = self.dpb.get(p.rps.ref_dpb_index[i] as usize) {
                    l0.push(crate::port_rc_driver::ref_pic_rc(rf));
                }
            }
            for i in 0..usize::from(p.ref_list1_count_try) {
                if let Some(rf) = self.dpb.get(p.rps.ref_dpb_index[4 + i] as usize) {
                    l1.push(crate::port_rc_driver::ref_pic_rc(rf));
                }
            }
        }
        let refs = crate::port_rc_vbr_cbr_qpick::RefLists {
            l0: &l0,
            l1: &l1,
            l0_count_try: pic.map_or(0, |p| usize::from(p.ref_list0_count_try)),
            l1_count_try: pic.map_or(0, |p| usize::from(p.ref_list1_count_try)),
        };
        // `pcs->norm_me_dist` (initial_rc_process.c:718-726) — the per-b64
        // 8x8 distortion mean the cyclic-refresh motion gates threshold
        // against; 0 on an I slice, exactly as C leaves it.
        let norm_me_dist = if is_key {
            0u64
        } else {
            frame_me.map_or(0, |m| {
                let n = m.per_b64.len() as u64;
                m.per_b64
                    .iter()
                    .fold(0u64, |a, b| a + u64::from(b.me_8x8_distortion))
                    .checked_div(n)
                    .unwrap_or(0)
            })
        };
        let new_qindex = crate::port_rc_vbr_cbr_qpick::rc_calc_qindex_rate_control(
            &mut rcs.rc,
            &rcs.cfg,
            &rcs.scs,
            &rcs.twopass,
            &mut frame,
            &refs,
            slice_type,
            // `MeDistortion` is read only by the VBR reference-qindex floor —
            // unreachable under AOM_CBR.
            None,
            &mut rcs.cr_sb_end,
            &mut rcs.cr,
            |cr| {
                if let Some(me) = frame_me {
                    crate::sb_qindex::cyclic_refresh_setup(
                        cr,
                        u32::from(b64_count),
                        norm_me_dist,
                        &me.per_b64,
                    );
                } else {
                    // C runs the setup over zeroed ME arrays on a key frame;
                    // its only possible outcome is what
                    // `cyclic_refresh_init` already left — refresh stays off
                    // for an I slice.
                    cr.apply_cyclic_refresh = false;
                }
            },
        );
        let Some(new_qindex) = new_qindex else {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "CBR rate control could not resolve this frame's qindex bounds: \
                 `rc_pick_q_and_bounds_no_stats_cbr` reads the LAST reference \
                 unconditionally and its DPB slot is empty"
            )));
        };
        #[cfg(feature = "std")]
        if crate::dbgenv::rcdbg() {
            std::eprintln!(
                "RCDBG pic={} ft={} this_tgt={} base_tgt={} qidx={} cr={} \
                 buf={} bot={} avg_bw={} lastq=[{},{}] active_worst={} \
                 roll_t={} roll_a={} since_key={} to_key={} band={}",
                frame.picture_number,
                frame.update_type as i32,
                frame.this_frame_target,
                frame.base_frame_target,
                new_qindex,
                rcs.cr.apply_cyclic_refresh as i32,
                rcs.rc.buffer_level,
                rcs.rc.bits_off_target,
                rcs.rc.avg_frame_bandwidth,
                rcs.rc.last_q[0],
                rcs.rc.last_q[1],
                rcs.rc.active_worst_quality,
                rcs.rc.rolling_target_bits,
                rcs.rc.rolling_actual_bits,
                rcs.rc.frames_since_key,
                rcs.rc.frames_to_key,
                rcs.cr_sb_end,
            );
        }
        // `generate_sb_qindex`'s CBR arm — `svt_av1_rc_init_sb_qindex`
        // (rc_aq.c:879-885): cyclic refresh assigns per-SB qindexes and flips
        // `delta_q_present`; otherwise every SB takes the frame base. C skips
        // `svt_av1_normalize_sb_delta_q` at `delta_q_res == 1`
        // (rc_process.c:741-744), which `SbQindexPlan.delta_q_res = 1`
        // carries into the signal side.
        let sb_plan = if rcs.cr.apply_cyclic_refresh
            && let Some(me) = frame_me
        {
            let mut sb_qindex = alloc::vec![0u8; usize::from(b64_count)];
            crate::sb_qindex::cyclic_sb_qp_assignment(
                &rcs.cr,
                new_qindex,
                norm_me_dist,
                &me.per_b64,
                &mut sb_qindex,
            );
            Some(crate::sb_qindex::SbQindexPlan {
                base_qindex: new_qindex as u8,
                sb_qindex,
                delta_q_res: 1,
            })
        } else {
            None
        };
        Ok((new_qindex as u8, frame, sb_plan))
    }

    /// C `rc_process_packetization_feedback`'s one-pass CBR arm
    /// (rc_process.c:758-792): `svt_av1_rc_postencode_update` consumes the
    /// coded bit count and the normalized zero-MV area, then
    /// `svt_aom_update_rc_counts` advances the frame counters.
    pub(super) fn cbr_postencode(
        &mut self,
        frame: &mut crate::port_rc_vbr_cbr_state::FrameRc,
        total_num_bits: u64,
        avg_cnt_zeromv: u64,
    ) {
        let Some(rcs) = self.rc_vbr_cbr.as_mut() else {
            return;
        };
        crate::port_rc_vbr_cbr_update::postencode_update(
            &mut rcs.rc,
            &rcs.cfg,
            &rcs.scs,
            frame,
            &rcs.cr,
            total_num_bits,
            avg_cnt_zeromv,
        );
        let (frames_since_key, frames_to_key, frames_since_cdf_update) =
            crate::port_rc_process::update_rc_counts(
                frame.showable_frame,
                // `ppcs->frm_hdr.disable_cdf_update` — always signalled 0 by
                // this encoder (obu.rs:1600/:2515/:3450).
                false,
                rcs.rc.frames_since_key,
                rcs.rc.frames_to_key,
                rcs.rc.frames_since_cdf_update,
            );
        rcs.rc.frames_since_key = frames_since_key;
        rcs.rc.frames_to_key = frames_to_key;
        rcs.rc.frames_since_cdf_update = frames_since_cdf_update;
        #[cfg(feature = "std")]
        if crate::dbgenv::rcdbg() {
            std::eprintln!(
                "RCDBG-POST pic={} bits={} proj={} zeromv={} buf={} bot={} \
                 since_key={} to_key={} low_motion={}",
                frame.picture_number,
                total_num_bits,
                frame.projected_frame_size,
                avg_cnt_zeromv,
                rcs.rc.buffer_level,
                rcs.rc.bits_off_target,
                rcs.rc.frames_since_key,
                rcs.rc.frames_to_key,
                rcs.rc.avg_frame_low_motion,
            );
        }
    }
}
