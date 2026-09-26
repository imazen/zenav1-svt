use super::*;

impl EncodePipeline {
    #[inline(always)]
    pub(super) fn filter_and_pack_frame(
        &mut self,
        fs: &FrameShape<'_>,
        display_order: u64,
        hbd_source: Option<HbdSource>,
        mut hbd_used: bool,
        stop: almost_enough::StopToken,
        pic_decision: Option<crate::port_picstruct::PicParams>,
        pcs: PictureControlSet,
        n: usize,
        encode_input: Vec<u8>,
        sb_chroma_owned: Option<(Vec<u8>, Vec<u8>)>,
        pa_cur: Option<Box<crate::inter_me_arm::PaPicture>>,
        frame_me: Option<crate::inter_me_arm::FrameMe>,
        mut cbr_frame_rc: Option<crate::port_rc_vbr_cbr_state::FrameRc>,
        tpl_r0: f64,
        primary_ref_frame_for_cdf: u8,
        primary_ref_cdfs: Option<alloc::sync::Arc<crate::port_frame_cdf::FrameCdfs>>,
        gm_models: Option<crate::port_global_me::GmModels>,
        c_quant: Option<alloc::sync::Arc<crate::quant::CodingQuantCfg>>,
        mut recon: Vec<u8>,
        tile_rows_log2: u8,
        tile_cols_log2: u8,
        delta_q_res_signal: Option<u8>,
        lf_sharp_eff: u8,
        film_grain: Option<crate::noise_gen::FilmGrainParams>,
        is_single_frame: bool,
        pipeline_md_inputs: Option<crate::inter_hdr_arm::PipelineMdInputs>,
        md_config_signals: Option<crate::port_enc_mode_config::md_config::MdConfigSignals>,
        gm_field: [svtav1_types::motion::WarpedMotionParams; 8],
        ref_gm_field: [svtav1_types::motion::WarpedMotionParams; 8],
        inter_syntax_state: Option<InterSyntaxState>,
        inter_ref_frame_side: [i8; 8],
        inter_mvp_env: Option<crate::partition::InterMdEnv>,
        all_trees: Vec<crate::partition::PartitionTree>,
        sb_min_sq_sizes: Vec<u8>,
        sb_max_sq_sizes: Vec<u8>,
        cw: usize,
        ext_cbuf: usize,
        lr_true_w: usize,
        lr_true_h: usize,
        frame_coded_area: core::cell::RefCell<Option<CodedAreaAcc>>,
        walk_end_cdfs: core::cell::RefCell<Option<crate::port_frame_cdf::FrameCdfs>>,
    ) -> Result<Vec<u8>, whereat::prelude::At<EncodeError>> {
        let FrameShape {
            chroma,
            is_key,
            sc_arm,
            temporal_layer,
            frame_hier,
            w,
            h,
            fmt,
            ss_x,
            ss_y,
            acw,
            ach,
            filter_chroma,
            sc_derivation,
            frame_tx_mode_select,
            base_qindex,
            coded_lossless,
            delta_q_plan,
            md_sb_qindex,
            sb_size,
            sb_cols,
            sb_rows,
            ref_padded_luma,
            tile_grid,
            chroma_deltas,
            qindex_u,
            qindex_v,
            qm_levels,
            seq_tools,
        } = *fs;
        // Frame-boundary marker in the PACKTREE file: the dump appends one
        // record per coded leaf with no frame tag, which made a multi-frame
        // ptree unusable — a frame-1 divergence could not be split from
        // frame 0's rows. `# FRAME` matches tree_diff.py's convention of
        // ignoring any line that does not start with the record tag.
        #[cfg(feature = "std")]
        if let Some(path) = crate::dbgenv::packtree() {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                let _ = writeln!(f, "# FRAME poc={display_order}");
            }
        }
        #[allow(clippy::type_complexity)]
        // inline tuple documents the shape; a `type` alias would hide it
        // `recon_only` — see the call-site comment at the recon walk below:
        // the traversal and its reconstruction side effects are identical,
        // but no symbol is written and no CDF/coded-area state is kept.
        let run_entropy_walk = |lr: Option<&crate::restoration::FrameRestInfo>,
                                cdef_walk: Option<&crate::cdef::CdefPick>,
                                recon_only: bool|
         -> crate::EncodeResult<(
            Vec<u8>,
            crate::deblock::DeblockGeom,
            Vec<u8>,
            Vec<u8>,
            u8,
        )> {
            Self::entropy_walk(
                self.speed_config.preset,
                self.superres_denom,
                self.chroma_format,
                self.bit_depth,
                chroma,
                &stop,
                is_key,
                sc_arm,
                &pic_decision,
                w,
                h,
                n,
                &sb_chroma_owned,
                sc_derivation,
                frame_tx_mode_select,
                base_qindex,
                delta_q_plan,
                md_sb_qindex,
                &primary_ref_cdfs,
                &c_quant,
                sb_size,
                sb_cols,
                sb_rows,
                ref_padded_luma,
                tile_grid,
                chroma_deltas,
                qindex_u,
                qindex_v,
                delta_q_res_signal,
                qm_levels,
                seq_tools,
                md_config_signals,
                &inter_syntax_state,
                inter_ref_frame_side,
                &inter_mvp_env,
                &all_trees,
                cw,
                ext_cbuf,
                lr_true_w,
                lr_true_h,
                &frame_coded_area,
                &walk_end_cdfs,
                lr,
                cdef_walk,
                recon_only,
            )
        };
        // Whether the walk's side effects (the recon planes + deblock
        // geometry) are consumed downstream: the CDEF search (when this
        // preset runs it), the loop-restoration search, both preset <= 6 —
        // or the LR stripe-boundary save; (2) the caller, via `last_recon*` /
        // a later frame predicting from this recon through the DPB. When NONE
        // of those exist the filtered pixels are dead: nothing reads them and
        // the bitstream is already written. C behaves identically (its
        // preset-10 profile contains zero CDEF/LPF samples for
        // byte-identical output).
        //
        // Byte-inertness is measured, not assumed: skipping the two apply
        // passes changed 0/90 cells at presets 7..13 and 13/36 at presets 2/6
        // (tools/byteid_fingerprint.sh, {64,128,256} x qp{20,40,55} x
        // {gradient,uniform}) — see benchmarks/perf_postfilter_2026-08-11.meta.
        //
        // Hoisted above the entropy walk: the same condition selects the
        // walk SPLIT below — when side effects are consumed, a cheap
        // recon-only walk produces them and the ONE bit-producing walk runs
        // after every filter parameter is known (C order: rest_process
        // before the EC kernel), so a CDEF/LR re-walk never discards a full
        // pass of symbol work.
        let postfilter_consumed = seq_tools.enable_restoration
            || crate::cdef::allintra_preset_uses_cdef_search(self.speed_config.preset)
            || self.recon_output
            // A later frame may predict from this recon via the DPB. Only an
            // all-key sequence (`intra_period == 1`) provably has no such
            // reader — every `self.dpb.get(..)` site is gated on `!is_key`.
            || !is_single_frame;
        let (mut tile_data, deblock_geom, mut u_recon, mut v_recon, mut tile_size_bytes_minus_1) =
            if postfilter_consumed {
                // Recon-only walk: identical traversal and reconstruction side
                // effects (the chroma recon planes + deblock geometry the
                // searches below read), but no symbols — its tile bytes would be
                // discarded whenever a CDEF/LR re-walk followed, and the
                // bit-producing walk now runs once, at the end, armed with every
                // syntax the searches picked.
                let (_t, geom, u, v, _s) = run_entropy_walk(None, None, true)?;
                (Vec::new(), geom, u, v, 0)
            } else {
                // Nothing downstream reads the walk's side effects: a single
                // full walk is the final bitstream (unchanged fast path).
                run_entropy_walk(None, None, false)?
            };

        crate::stop_check(&stop)?;

        // Step 6a: Deblocking — pick the levels the frame header will
        // signal (C svt_av1_pick_filter_level_by_q closed form) and apply
        // the filter decoder-exactly to the OUTPUT reconstruction. The
        // prediction sources are untouched: intra prediction read the live
        // unfiltered buffers (tile_frame_recon for luma, u/v_recon during
        // the walk) and the walk is complete by now — the filtered copy
        // becomes last_recon and the DPB frame, exactly the decoder's
        // split (it predicts intra from unfiltered pixels and stores the
        // filtered frame for output/reference).
        //
        // Inter frames keep levels 0 (write_inter_frame signals 0): the
        // q-based picker is only wired for key frames, and signaling
        // nothing while applying nothing stays self-consistent.
        //
        // Preset split (C get_dlf_level_allintra, enc_mode_config.c:2214,
        // fast_decode 0): presets <= M5 get dlf_level 1/2 -> sb_based_dlf=0
        // -> dlf_process runs svt_av1_pick_filter_level with
        // LPF_PICK_FROM_FULL_IMAGE (real SSE trials on the post-encode
        // recon); presets >= M6 get dlf_level 5 -> sb_based_dlf=1 -> the
        // LPF_PICK_FROM_Q closed form. early_exit_convergence is 0 at
        // dlf_level 1 (<= M3) and 1 at dlf_level 2 (M4/M5).
        // Pre-DLF recon dump (SVTAV1_RECONDBG) — before the preset split so
        // it fires at every preset (#90); matches C's dlf_process.c:101
        // dump point (recon final, not yet deblocked).
        #[cfg(feature = "std")]
        {
            let (su, sv) = chroma.unwrap_or((&[][..], &[][..]));
            crate::deblock::recondbg_dump(
                &encode_input,
                su,
                sv,
                &recon,
                &u_recon,
                &v_recon,
                w,
                h,
                // Debug dump assumes 4:2:0 dims internally (`width/2`) —
                // dump luma only at 4:4:4 rather than mis-shape chroma.
                filter_chroma,
            );
        }
        self.dump_recon10_bin();
        let mut recon10 = self.take_recon10(chroma);
        // ---- deblock signal derivation inputs (C enc_mode_config.c) -----
        //
        // `dlf_enc_mode`: C starts from `pcs->enc_mode` and, when
        // `enable_dlf_flag == 2`, re-derives as if three presets lower
        // (`AOMMAX(ENC_MR, enc_mode - 3)`). The port carries no
        // `enable_dlf_flag` config — it is always 1 — so the adjustment is
        // translated but cannot fire; `EncMode` is `int8_t`-ranged with
        // `ENC_MR = -1`, hence the `i8`.
        //
        // `pcs->enc_mode` is the ARM-CLAMPED preset: C rewrites
        // `scs->static_config.enc_mode` once in `svt_av1_enc_set_parameter`
        // (`enc_handle.c:4415-4436`) — allintra `> M9 -> M9`, video non-RTC
        // `> M11 -> M11` — so every downstream ladder reads the clamped value.
        // MEASURED: without the clamp, `get_dlf_level_default(12)` falls into
        // the `else` arm and returns 0 (deblock OFF) where C, seeing M11,
        // returns 6 on a base picture -> `sb_based_dlf` -> the by-q closed
        // form -> `loop_filter_level = 3`. That one field is what made every
        // video-mode key frame at preset 12/13 exactly ONE byte short of C's
        // on all five synthetic content classes at once.
        let dlf_enc_mode = crate::rate_arm::eff_enc_mode(sc_arm, self.speed_config.preset) as i8;
        // `ppcs->input_resolution` — `svt_aom_derive_input_resolution` over
        // `scs->max_input_luma_width * scs->max_input_luma_height`, which is
        // the source size padded up to MIN_BLOCK_SIZE (8) on each axis
        // (`enc_handle.c:3918-3930`, then `:3992`).
        let dlf_resolution = crate::port_enc_mode_config::ResolutionRange::from_luma_area(
            self.true_width.next_multiple_of(8) * self.true_height.next_multiple_of(8),
        );
        // `ppcs->temporal_layer_index` / `ppcs->is_highest_layer`. A KEY frame
        // is always temporal layer 0, and C's
        // `is_highest_layer = (temporal_layer_index == hierarchical_levels) &&
        // hierarchical_levels != 0` (`pd_process.c:5560`) is therefore false
        // for it at every hierarchy depth INCLUDING flat (the second clause
        // exists precisely so a flat GOP does not mark every picture highest).
        // Written out rather than folded to constants so the inter chunks
        // inherit the rule instead of re-deriving it.
        //
        // INTER (2026-09-02): this is now the REAL `temporal_layer`, not the
        // key frame's literal 0. On the campaign's flat low-delay-P GOP it is
        // still 0 for every picture — which is the point: `is_highest_layer`
        // ANDs in `hierarchical_levels != 0` (`pd_process.c:5560`, its own
        // comment says "for flat, set is_highest_layer to false to avoid using
        // aggressive settings for all pictures"), so `is_not_last_layer` is
        // TRUE on a flat GOP and `get_dlf_level_default` gives 3 at <= M6 and
        // 6 at <= M9. Both are ENABLED levels; the port used to signal 0.
        let dlf_temporal_layer_index: u8 = temporal_layer;
        let dlf_is_base = dlf_temporal_layer_index == 0;
        let dlf_is_highest_layer =
            crate::port_picstruct::is_highest_layer(dlf_temporal_layer_index, frame_hier);
        let dlf_is_not_last_layer = u8::from(!dlf_is_highest_layer);

        // IBC (chunk 1): C kills the deblock filter at SIGNAL-DERIVATION on
        // IntraBC frames — `dlf_level` stays 0 unless `enable_dlf_flag &&
        // frm_hdr->allow_intrabc == 0` (enc_mode_config.c:10117-10127), so
        // neither the level pick nor the frame apply runs and the FH codes
        // no loop-filter params (obu.rs suppresses them on the same flag).
        // Only sc_class5 presets <= 4 frames take this arm.
        // Coded-lossless: `dlf_ctrls.enabled = 0`, `cdef_level = 0` and (at
        // AllLossless, which every unscaled lossless frame is) `enable_restoration
        // = 0` (md_config_process.c:1022-1035): no search, no application, and
        // the frame header carries none of the three (chunk 1). Same shape as
        // the IntraBC frame-level suppression this predicate already handles.
        //
        // WHICH picker runs is not a preset rule but a two-step C derivation
        // that FORKS on `scs->allintra` (`md_config_process.c:924-930`):
        //
        //   allintra -> svt_aom_sig_deriv_mode_decision_config_allintra
        //               -> get_dlf_level_allintra  (enc_mode_config.c:1540)
        //   video    -> svt_aom_sig_deriv_mode_decision_config_default
        //               -> get_dlf_level_default   (enc_mode_config.c:1466)
        //
        // and then maps that LEVEL through `svt_aom_set_dlf_controls` (:1561).
        // `sb_based_dlf` is what selects the picker: set, `enc_dec_process.c
        // :3132` runs LPF_PICK_FROM_Q (the closed form); clear,
        // `dlf_process.c:97` runs LPF_PICK_FROM_FULL_IMAGE (the SSE search).
        //
        // Before the inter campaign the port encoded the ALLINTRA resolution of
        // that chain inline (`preset <= 5` -> search, else closed form, with
        // `early_exit_convergence` 0 below M4). That flattening is exactly
        // right for the still envelope and is reproduced bit-for-bit by the
        // table below — but it was gated on `is_single_frame`, so a VIDEO-mode
        // key frame fell through to the closed form, which is not the arm C
        // takes. At preset 6 / qindex 67 that signalled `loop_filter_level = 3`
        // where C signals 0.
        // C `pcs->ref_skip_percentage` (`rc_process.c:96`, `rc_init_frame_stats`
        // — the mean `skip_coded_area` of the nearest L0/L1 references,
        // I-slice refs counting 0). Feeds `dlf_level_modulation` inside
        // `get_dlf_level_default`; a high-skip reference set is what shuts
        // the loop filter off on well-predicted hierarchical frames. 0 on a
        // key frame (no refs — C's I-slice early-out).
        let dlf_ref_skip_percentage = pic_decision.as_ref().map_or(0, |p| {
            if is_key {
                0
            } else {
                crate::port_rc_process::get_ref_skip_percentage(
                    crate::port_rc_process::SliceType::B,
                    u8::try_from(p.ref_list1_count_try).unwrap_or(u8::MAX),
                    (p.ref_list0_count_try > 0)
                        .then(|| ref_obj_stats(&self.dpb, p.rps.ref_dpb_index[0] as usize))
                        .flatten()
                        .as_ref(),
                    (p.ref_list1_count_try > 0)
                        .then(|| ref_obj_stats(&self.dpb, p.rps.ref_dpb_index[4] as usize))
                        .flatten()
                        .as_ref(),
                )
            }
        });
        let dlf_level = derive_dlf_level(
            is_single_frame,
            dlf_enc_mode,
            dlf_resolution,
            dlf_is_base,
            dlf_is_not_last_layer,
            dlf_ref_skip_percentage,
            self.reference,
        );
        // C's `default:` arm is `assert(0)`; the port refuses rather than
        // inventing a control set.
        let dlf_ctrls = crate::port_enc_mode_config::ctrls::set_dlf_controls(dlf_level).ok_or(
            EncodeError::UnsupportedConfig("dlf level outside svt_aom_set_dlf_controls' 0..=7"),
        )?;

        // C `ppcs->ref_frame_type_arr[0 .. tot_ref_frame_types]`, restricted
        // to the SINGLE-reference entries (`rf[1] == NONE_FRAME`) — the only
        // ones either picker reads. `set_all_ref_frame_type`
        // (`pd_process.c:1044`, ported at `port_picstruct`) lays list 0's
        // singles down first, then list 1's, then the compounds, so the
        // singles are exactly the first `ref_list0_count_try +
        // ref_list1_count_try` entries and the DPB slot for each is
        // `rps.ref_dpb_index[LAST + i]` / `[BWD + i]`.
        //
        // EMPTY on a key frame, which is what makes `tot_ref_frame_types == 0`
        // and leaves every reference-dependent branch inert.
        let dlf_refs = self.collect_dlf_refs(is_key, &pic_decision);
        // C `average_me_sad` (`deblocking_filter.c:982-986`): the MEAN of
        // `ppcs->rc_me_distortion[b64]` over `b64_total_count`, which
        // `motion_estimation.c:2778` fills with the b64's 8x8 SAD sum at
        // <= 480p and its 16x16 sum above. Read ONLY by `me_based_dlf_skip`,
        // i.e. only at a `dlf_level` whose controls set
        // `zero_filter_strength_lvl` (5 / 6 / 7), and never on an I_SLICE.
        //
        // MEASURED 2026-09-02, and it is why this is not optional at p8:
        // `uniform` content translates exactly, so every b64's SAD is 0 and C
        // writes `loop_filter_level = 0` on every one of its inter frames
        // DESPITE a nonzero reference level — while `gradient 16x16 q40 p8`
        // clears the threshold and C writes 9. `screen 16x16 q40 p8` lands
        // BETWEEN the two thresholds and C writes luma 9 with chroma 0.
        let dlf_avg_me_sad: u32 = match frame_me.as_ref() {
            Some(me) if !me.per_b64.is_empty() => {
                let total: u64 = me
                    .per_b64
                    .iter()
                    .map(|b| u64::from(b.rc_me_distortion))
                    .sum();
                (total / me.per_b64.len() as u64) as u32
            }
            _ => 0,
        };
        let dlf_pick_inputs = self.build_dlf_pick_inputs(
            is_key,
            &pic_decision,
            frame_hier,
            base_qindex,
            dlf_resolution,
            dlf_temporal_layer_index,
            dlf_ctrls,
            &dlf_refs,
            dlf_avg_me_sad,
        );
        // C `dlf_process.c:89-91` seeds all three to -1 ("not computed"); only
        // the non-SB-based path overwrites them, so a frame that takes the
        // by-q arm genuinely has no measurement and its `dlf_dist_dev` must
        // stay -1 for the NEXT frame to skip rather than average.
        let mut dlf_zero_filt_sse: i64 = -1;
        let mut dlf_best_filt_sse: i64 = -1;
        let mut dlf_full_image_ran = false;
        let mut lf_levels = self.pick_lf_levels(
            chroma,
            &hbd_source,
            &mut hbd_used,
            &stop,
            w,
            h,
            filter_chroma,
            &encode_input,
            sc_derivation,
            coded_lossless,
            &recon,
            lf_sharp_eff,
            &deblock_geom,
            &u_recon,
            &v_recon,
            &recon10,
            dlf_ctrls,
            dlf_pick_inputs,
            &mut dlf_zero_filt_sse,
            &mut dlf_best_filt_sse,
            &mut dlf_full_image_ran,
        )?;
        // The in-loop post-filters (deblock -> CDEF) apply only when
        // `postfilter_consumed` says the filtered pixels have a reader —
        // see the derivation comment at the entropy walk above.
        if self.recon_output {
            self.last_recon_unfiltered = Some((recon.clone(), u_recon.clone(), v_recon.clone()));
        }
        // C `dlf_process.c:103-112`, and it is NOT dead code on the inter
        // path even though the port's key-frame comment used to say the guard
        // "can never fire": `search_filter_level` seeds its hill climb at
        // `last_frame_filter_level`, which is 0 on a key frame (so `ss_err[0]`
        // is always evaluated and `zero_filt_sse` is always set) but is the
        // REFERENCE'S level on an inter frame, so level 0 can go unvisited.
        // C then measures the unfiltered SSE explicitly, and shuts the filter
        // off if filtering did not actually beat not filtering.
        //
        // The ref-average arms reach here with BOTH sentinels intact (they
        // never call the search at all), which is exactly the state where C
        // recomputes `zero` and leaves `best` for after the filter.
        if dlf_full_image_ran && dlf_zero_filt_sse == -1 && lf_levels.any() {
            dlf_zero_filt_sse = crate::deblock::plane_sse(&encode_input, &recon, w, h);
            if dlf_best_filt_sse != -1 && dlf_zero_filt_sse <= dlf_best_filt_sse {
                lf_levels = crate::deblock::LfLevels::default();
            }
        }
        // 4:4:4 staged bring-up (`filter_chroma` above): signal chroma
        // loop-filter levels 0 so the decoder skips chroma filtering — the
        // ss=0 edge kernels are not yet ported. Luma is untouched; a level-0
        // chroma entry is identity in every `lpf_params` gate.
        if fmt == svtav1_types::chroma::ChromaFormat::Yuv444 {
            lf_levels.levels[2] = 0;
            lf_levels.levels[3] = 0;
        }
        if let Some((y10, u10, v10)) = recon10.as_mut()
            && lf_levels.any()
            && postfilter_consumed
        {
            crate::deblock::apply_deblock_frame_hbd_with_stop(
                y10,
                u10,
                v10,
                w,
                h,
                filter_chroma,
                &deblock_geom,
                &lf_levels,
                lf_sharp_eff,
                self.bit_depth,
                &stop,
            )?;
        }
        #[cfg(feature = "std")]
        if crate::dbgenv::lfdbg() {
            std::eprintln!(
                "LFDBG key={} lf_levels={:?} sharp={} postfilter_consumed={}",
                u8::from(is_key),
                lf_levels.levels,
                lf_sharp_eff,
                postfilter_consumed,
            );
        }
        if lf_levels.any() && postfilter_consumed {
            crate::deblock::apply_deblock_frame_with_stop(
                &mut recon,
                &mut u_recon,
                &mut v_recon,
                w,
                h,
                filter_chroma,
                &deblock_geom,
                &lf_levels,
                lf_sharp_eff, // = signaled loop_filter_sharpness
                &stop,
            )?;
            // C `dlf_process.c:114-117`: the FILTERED SSE, measured after
            // `svt_av1_loop_filter_frame`, when the search did not leave one.
            if dlf_full_image_ran && dlf_best_filt_sse == -1 {
                dlf_best_filt_sse = crate::deblock::plane_sse(&encode_input, &recon, w, h);
            }
        }
        // C `pcs->dlf_dist_dev` (`dlf_process.c:119`) — the per-mille SSE
        // improvement this frame's own deblock bought, which the NEXT frame
        // reads off the reference object to decide whether to filter at all.
        // -1 ("never computed") everywhere the SB-based arm ran, per
        // `dlf_process.c:92`.
        let dlf_dist_dev = if dlf_full_image_ran {
            crate::dlf_arm::dlf_dist_dev(lf_levels, dlf_zero_filt_sse, dlf_best_filt_sse)
        } else {
            -1
        };

        crate::stop_check(&stop)?;

        // Step 6a': CDEF — decoder order is deblock -> CDEF (-> restoration,
        // unported). Key frames signal the qp-picked strengths
        // (svt_pick_cdef_from_qp intra branch) and apply the decoder-exact
        // frame pass (libaom av1_cdef_frame) to the SAME output copy; the
        // per-64x64 cdef_idx costs ZERO arithmetic-coder bits because
        // cdef_bits = 0 (libaom read_cdef does aom_read_literal(r, 0) —
        // a no-iteration loop, bitreader.h:161 — so the entropy walk needs
        // no syntax change). Inter frames signal zero strengths and apply
        // nothing — consistent.
        // IBC (chunk 1): C kills CDEF at SIGNAL-DERIVATION on IntraBC frames
        // — `if (!scs->seq_header.cdef_level || frm_hdr->allow_intrabc)
        // cdef_search_level = 0` (allintra: enc_mode_config.c:2396-2398) and
        // cdef_process re-zeroes cdef_params (cdef_process.c:692-697). The
        // all-zero-strength default makes apply_cdef_frame a structural
        // no-op and cdef_bits stays 0 (no per-SB syntax, no FH params).
        //
        // WHICH CDEF policy runs is the same two-step C derivation the deblock
        // levels above take, forking on `scs->allintra`:
        //
        //   allintra -> svt_aom_sig_deriv_multi_processes_allintra
        //               -> its cdef_search_level ladder (enc_mode_config.c:2396)
        //   video    -> svt_aom_sig_deriv_multi_processes_default
        //               -> its cdef_search_level ladder (:2083)
        //
        // and then maps that LEVEL through `set_cdef_search_controls` (:891).
        // `use_qp_strength` is what selects the fast path: level 10 sets it,
        // levels 1..=9 clear it and carry a candidate set to RD-search.
        //
        // Before the inter campaign the port encoded the ALLINTRA resolution of
        // that chain inline (`preset <= 6` -> search, else the qp closed form,
        // with the candidate set flattened per preset). That flattening is
        // exactly right for the still envelope and is reproduced entry for
        // entry by the ladder below (`cdef.rs`'s
        // `allintra_flattening_matches_the_ladder`) — but it was gated on
        // `is_single_frame`, so a VIDEO-mode key frame fell through to the qp
        // fast path, which is not the arm C takes. C's video ladder gives
        // `is_base ? 5 : 6` at M6..M7 and 7 above, i.e. a video key frame
        // SEARCHES at every preset; at preset 6 / qindex 67 the port signalled
        // y=(pri 1, sec 0) / uv=(pri 1, sec 0) where C signals y=(0, 2) /
        // uv=(7, 0) — the level-5 candidate set {0, 28, 60} + {2, 30, 62}.
        //
        let cdef_level =
            self.derive_cdef_level(sc_derivation, is_single_frame, dlf_resolution, dlf_is_base);
        // `set_cdef_search_controls`' `is_base` is `frame_is_boosted` =
        // `frame_is_kf_gf_arf` = intra-only OR ARF OR GF update, and
        // `is_not_highest_layer` is `!frame_is_leaf` = `update_type !=
        // LF_UPDATE` (enc_mode_config.h:100-116). A KEY frame is intra-only
        // and KF_UPDATE, so both are true; written out rather than folded to
        // literals so the inter chunks inherit the rule.
        // C `frame_is_boosted` = `frame_is_kf_gf_arf`, and `is_not_highest_layer`
        // = `!frame_is_leaf` = `update_type != LF_UPDATE`
        // (`enc_mode_config.h:100-116`). Both were literal `is_key` while only
        // key frames were encodable. They now come from the picture decision's
        // `update_type` (`port_picstruct::set_frame_update_type`,
        // `pd_process.c:4591`), which is what C reads.
        //
        // A KEY frame is intra-only and KF_UPDATE, so both stay true there —
        // byte-inert for every existing cell, by construction rather than by
        // measurement alone.
        let (cdef_frame_is_boosted, cdef_is_not_highest_layer) = match pic_decision.as_ref() {
            Some(pic) => (
                crate::port_picstruct::frame_is_boosted(pic),
                pic.update_type != crate::port_picstruct::FrameUpdateType::Lf,
            ),
            None => (is_key, is_key),
        };
        // C's `default:` arm is `assert(0)`; the port refuses rather than
        // inventing a control set.
        // C `cdef_recon_level` -> `set_cdef_recon_controls` (enc_mode_config.c
        // :1200). ANOTHER arm ladder, and the port ran neither side of it: the
        // allintra arm is `enc_mode <= M7 ? 0 : 1` (`:2432`) and the video arm
        // `<= M8 ? 0 : <= M10 ? 1 : 2` (`:2102`), both at C's default
        // `fast_decode == 0` (the `fast_decode` branches are unreachable here
        // for the same reason the CDEF search ladder's are). Only
        // `zero_fs_cost_bias` is live on a KEY frame — see `CdefSearchCfg`.
        //
        // The allintra M10..M13 -> M9 clamp does not move this: every preset
        // from M8 up lands on level 1 either way.
        let cdef_recon_level: u8 = if is_single_frame {
            u8::from(self.speed_config.preset > 7)
        } else if self.speed_config.preset <= 8 {
            0
        } else if self.speed_config.preset <= 10 {
            1
        } else {
            2
        };
        let cdef_recon_ctrls = crate::port_enc_mode_config::tail::set_cdef_recon_controls(
            cdef_recon_level,
        )
        .ok_or(EncodeError::UnsupportedConfig(
            "cdef recon level outside set_cdef_recon_controls' 0..=4",
        ))?;
        let cdef_zero_fs_cost_bias = cdef_recon_ctrls.zero_fs_cost_bias;
        let mut cdef_ctrls = crate::port_enc_mode_config::cdef_search::set_cdef_search_controls(
            cdef_level,
            cdef_frame_is_boosted,
            cdef_is_not_highest_layer,
            self.reference,
        )
        .ok_or(EncodeError::UnsupportedConfig(
            "cdef search level outside set_cdef_search_controls' 0..=10",
        ))?;
        // C `md_config_process.c:983-985`: when the level asked for either
        // reference-derived mode, the candidate set is REWRITTEN from the
        // reference pictures' own chosen strengths. Unreachable on a key frame
        // — `search_best_ref_fs` is `is_not_highest_layer ? 0 : 1` and a key
        // frame's `is_not_highest_layer` is true — so this is byte-inert for
        // the whole still envelope by construction.
        //
        // C's ORDER, and it is an `else if` (`md_config_process.c:980-985`):
        // the three CDEF-OFF gates are tested FIRST, and the reference-derived
        // rewrite runs only when none of them fired.
        //
        //   me_based_cdef_skip(pcs)
        //   || (cdef_ctrls->skip_th && skip_perc >= cdef_skip_th)
        //   || (vq sharpness && is_noise_level)      -> cdef_level = 0
        //   else if (use_reference_cdef_fs || search_best_ref_fs)
        //                                            -> update_cdef_filters_on_ref_info
        //
        // The SECOND gate is wired here, and it is what C's frame 2 takes:
        // `skip_th` is `is_base ? 0 : 80` from level 7 up, the QP adjustment is
        // `CLIP3(25, 100, skip_th + (base_q_idx - 128) / 4)`, and
        // `ref_skip_percentage` is the value `md_config_inputs` already
        // derives. MEASURED on `diag 64x64 q40 p8 frames=3`: at poc 2 the
        // reference is a 22-byte all-skip frame, so `skip_perc` is 100 against
        // a threshold of 88 and C switches CDEF OFF for the whole frame —
        // which is why its header codes `cdef_damping - 3` as the low two bits
        // of **-3** (the `never_picked` quirk: `cdef_damping` keeps its
        // `resource_coordination_process.c:423` initialiser 0 because
        // `finish_cdef_search` never runs). The port coded 2 there.
        //
        // The FIRST gate IS modelled: `me_based_cdef_skip`
        // (`md_config_process.c:781`). It is inert below preset 9 —
        // `cdef_recon_ctrls.zero_filter_strength_lvl` is 0 there by C's own
        // table (`set_cdef_recon_controls(0)` = every video preset <= 8) —
        // and LIVE at M9+, where it needs this frame's `rc_me_distortion`
        // mean (the same `avg_me_sad` `me_based_dlf_skip` reads) and the
        // references' `cdef_dist_dev`. The proving cell: `gradient 64x64 q40
        // p13` poc 4, a base frame where `skip_th` is 0 and
        // `use_reference_cdef_fs`/`search_best_ref_fs` are both 0 — without
        // this gate the port coded `cdef_damping = 5` where C's
        // `cdef_level = 0` leaves the field at its 0 initialiser.
        //
        // The ref list is C's `ref_frame_type_arr` restricted to
        // `rf[1] == NONE_FRAME` — the same single-ref slots `dlf_refs`
        // walks, carrying `cdef_dist_dev` + `tmp_layer_idx` instead of the
        // deblock fields.
        let cdef_dist_refs: alloc::vec::Vec<crate::port_enc_mode_config::cdef_search::RefCdefDist> =
            match pic_decision.as_ref() {
                Some(pic) if !is_key => {
                    let mut v = alloc::vec::Vec::new();
                    let mut push = |idx: usize| {
                        if let Some(rf) = self.dpb.get(pic.rps.ref_dpb_index[idx] as usize) {
                            v.push(crate::port_enc_mode_config::cdef_search::RefCdefDist {
                                cdef_dist_dev: rf.cdef_dist_dev,
                                tmp_layer_idx: rf.temporal_layer,
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
        // The THIRD gate needs `vq_ctrls.sharpness_ctrls`, which this port
        // does not configure (C defaults it off).
        let mut cdef_force_off = !is_key
            && (crate::port_enc_mode_config::cdef_search::me_based_cdef_skip(
                &crate::port_enc_mode_config::cdef_search::CdefMeSkipInputs {
                    is_intra_slice: is_key,
                    hierarchical_levels: frame_hier,
                    temporal_layer_index: dlf_temporal_layer_index,
                    frame_is_boosted: cdef_frame_is_boosted,
                    frame_is_leaf: !cdef_is_not_highest_layer,
                    input_resolution: dlf_resolution,
                    zero_filter_strength_lvl: cdef_recon_ctrls.zero_filter_strength_lvl,
                    prev_cdef_dist_th: cdef_recon_ctrls.prev_cdef_dist_th,
                    refs: &cdef_dist_refs,
                    avg_me_sad: dlf_avg_me_sad,
                },
            ) || crate::port_enc_mode_config::cdef_search::cdef_skip_gate(
                cdef_ctrls.skip_th,
                base_qindex,
                pipeline_md_inputs
                    .as_ref()
                    .map_or(0, crate::inter_hdr_arm::ref_skip_percentage),
            ));
        if !cdef_force_off
            && !is_key
            && (cdef_ctrls.use_reference_cdef_fs != 0 || cdef_ctrls.search_best_ref_fs != 0)
            && let Some(pic) = pic_decision.as_ref()
        {
            use crate::port_enc_mode_config::cdef_search::RefCdefStrengths;
            // C reads `ref_pic_ptr_array[REF_LIST_0][0]` and
            // `[REF_LIST_1][0]` — the FIRST entry of each list, which is
            // LAST_FRAME's and BWDREF's DPB slot.
            let strengths_of = |slot: usize| -> Option<RefCdefStrengths> {
                let rf = self.dpb.get(slot)?;
                Some(RefCdefStrengths {
                    y0: *rf.cdef_y_strengths.first()?,
                    uv0: *rf.cdef_uv_strengths.first()?,
                    // C's `use_reference_cdef_fs` arm walks every slot
                    // (`ref_cdef_strengths_num`), not just slot 0, so the two
                    // extremes are computed here rather than assumed equal.
                    y_min: rf.cdef_y_strengths.iter().copied().min()?,
                    y_max: rf.cdef_y_strengths.iter().copied().max()?,
                })
            };
            const LAST: usize = 0;
            const BWD: usize = 4;
            if let Some(l0) = strengths_of(pic.rps.ref_dpb_index[LAST] as usize) {
                // C's list-1 guard: `slice_type == B_SLICE && ref_list1_count_try`.
                let l1 = (pic.ref_list1_count_try != 0)
                    .then(|| strengths_of(pic.rps.ref_dpb_index[BWD] as usize))
                    .flatten();
                let upd = crate::port_enc_mode_config::cdef_search::update_cdef_filters_on_ref_info(
                    &mut cdef_ctrls,
                    l0,
                    l1,
                );
                cdef_force_off = upd.force_cdef_off;
            }
        }
        // C `pcs->cdef_dist_dev` (`cdef_process.c:682-702`): seeded -1,
        // overwritten ONLY by the RD search (`enc_cdef.c:1057` — the
        // `use_qp_strength` and `use_reference_cdef_fs` arms of
        // `finish_cdef_search` return before it), and forced to 0 whenever
        // the signalled strengths end all-zero. The second element carries
        // the pre-override value; the override is applied once below.
        let (cdef_params, cdef_dist_dev) = self.pick_cdef(
            chroma,
            &hbd_source,
            &mut hbd_used,
            &stop,
            w,
            h,
            filter_chroma,
            &encode_input,
            sc_derivation,
            base_qindex,
            coded_lossless,
            &recon,
            &deblock_geom,
            &u_recon,
            &v_recon,
            &recon10,
            cdef_zero_fs_cost_bias,
            cdef_ctrls,
            cdef_force_off,
        )?;
        // 4:4:4 staged bring-up (`filter_chroma`): chroma CDEF is signaled
        // OFF — zero every uv strength so the header declares "chroma
        // unfiltered" and the apply call below skips the ss=0 kernels.
        let mut cdef_params = cdef_params;
        if fmt == svtav1_types::chroma::ChromaFormat::Yuv444 {
            for s in cdef_params.strengths.iter_mut() {
                s.1 = 0;
            }
        }
        // C `cdef_process.c:699-702`: whatever the pick left, a frame whose
        // signalled strengths are all zero (and `nb_cdef_strengths == 1`)
        // records `cdef_dist_dev = 0` — "no filtering happened" — which is
        // what the NEXT frame's `me_based_cdef_skip` reads as a shut-off
        // precedent.
        let cdef_dist_dev =
            if cdef_params.bits == 0 && cdef_params.strengths.first() == Some(&(0, 0)) {
                0
            } else {
                cdef_dist_dev
            };
        // Non-vacuity evidence (same role as `last_cdef_stats` /
        // `last_lr_stats`): the strength set 0 actually WRITTEN into the
        // frame header. Without this a gate cannot observe which arm of
        // `svt_pick_cdef_from_qp` the pipeline selected, so dropping the
        // `sc_class5` argument would be invisible to the whole suite.
        self.last_cdef_signaled = Some(crate::cdef::CdefFrameParams {
            damping: cdef_params.damping,
            y_strength: cdef_params.strengths[0].0,
            uv_strength: cdef_params.strengths[0].1,
        });
        // cdef_bits > 0 adds per-SB cdef_idx literals to the tile — the
        // walk is re-run with the emission armed, but NOT yet: when the LR
        // search below also signals, its walk carries the cdef syntax too
        // (`cdef_walk_opt`) and the intermediate walk's bytes would be
        // overwritten unread. The re-walk runs once, after both searches.
        // The pre-CDEF snapshot is load-bearing when LR is on (its stripe
        // boundaries are saved from it below), and an evidence aid otherwise.
        if seq_tools.enable_restoration || self.recon_output {
            self.last_recon_pre_cdef = Some((recon.clone(), u_recon.clone(), v_recon.clone()));
        }
        if postfilter_consumed {
            self.last_cdef_stats = crate::cdef::apply_cdef_frame_with_stop(
                &mut recon,
                &mut u_recon,
                &mut v_recon,
                w,
                h,
                filter_chroma,
                &deblock_geom,
                &cdef_params,
                &stop,
            )?;
        }
        // bd10: the post-deblock / pre-CDEF 10-bit planes are the `after_cdef
        // = 0` stripe-boundary context for the 10-bit LR apply (issue #13) —
        // the 10-bit twin of `last_recon_pre_cdef` above, taken at the same
        // point in the chain (dlf_process.c:134 saves them here in C).
        let recon10_pre_cdef: Option<(Vec<u16>, Vec<u16>, Vec<u16>)> =
            if seq_tools.enable_restoration {
                recon10.clone()
            } else {
                None
            };
        // bd10: apply CDEF to the 10-bit canvas too. Not for output — the u8
        // chain above still produces that — but because the Wiener LR search
        // reads the POST-CDEF recon, and at 10 bits that must be the 10-bit
        // one (C: rest_process runs after cdef_process on the same 16-bit
        // recon picture CDEF just filtered in place).
        if let (Some((y10, u10, v10)), true) = (recon10.as_mut(), postfilter_consumed) {
            crate::cdef::apply_cdef_frame_hbd_with_stop(
                y10,
                u10,
                v10,
                w,
                h,
                filter_chroma,
                &deblock_geom,
                &cdef_params,
                self.bit_depth,
                &stop,
            )?;
        }

        crate::stop_check(&stop)?;

        // Step 6a'': Wiener loop restoration — C order deblock -> CDEF ->
        // LR. The C-exact search (restoration_seg_search +
        // rest_finish_search at the allintra wn_filter controls) picks
        // per-RU taps against the POST-CDEF recon; when any plane signals
        // RESTORE_WIENER the tile is RE-walked with the per-SB lr syntax
        // (the flag+taps precede the first partition symbol, so the whole
        // arithmetic stream shifts — exactly like C, whose EC kernel runs
        // after rest_process), the FH carries the real lr_params, and the
        // output copy gets the decoder-exact stripe-boundary filter pass
        // (svt_av1_loop_restoration_filter_frame). Prediction sources are
        // untouched — the decoder's split.
        self.last_lr_stats = ([0; 3], 0);
        self.last_lr_unit_size = None;
        let mut lr_signal = crate::entropy::obu::LrSignal::none(seq_tools.enable_restoration);
        // The search result the bit-producing walk below is armed with —
        // `Some` exactly when the old code would have run an LR re-walk
        // (`rest_info.any_non_none()`); the walk itself now runs once,
        // after both searches.
        let mut walk_rest_info: Option<crate::restoration::FrameRestInfo> = None;
        let decoder_chroma_recon = self.recon_output
            && chroma.is_some()
            && (!(self.true_width as usize).is_multiple_of(1 << ss_x)
                || !(self.true_height as usize).is_multiple_of(1 << ss_y));
        let mut output_restoration = None;
        // IBC (chunk 1): unlike DLF/CDEF, C suppresses loop restoration at
        // PIPELINE EXECUTION, not signal-derivation — `if (ppcs->
        // enable_restoration && frm_hdr->allow_intrabc == 0)` gates BOTH the
        // search (rest_process.c:262) and the apply/finish (:325, else-arm
        // forces all planes RESTORE_NONE). enable_restoration itself (and
        // the SH bit) stays UNCHANGED — do NOT fold this into the
        // derivation (docs/ibc-port-map.md §A.7).
        //
        // The gate is NOT `is_key`: `ppcs->enable_restoration` is the
        // PICTURE-level `(wn > 0 || sg > 0)` (enc_mode_config.c:2142), and
        // the video ladder keeps Wiener live on an inter frame —
        // `wn_filter_level_default` gives level 5 (luma-only) at M4..M8
        // whenever `is_not_last_layer`, which a flat GOP's `hierarchical_
        // levels != 0` clause makes true for EVERY picture
        // (pd_process.c:5560). The `ctrls.enabled || sg_ctrls.enabled`
        // check inside IS that per-picture term, so `is_key` here only
        // ever wrongly disabled the stage on inter frames — the exact
        // `lr_type[0]` C=2 vs 0 divergence on `johnny_256x256` q40 p6
        // frame 1.
        // `&self`, not `&mut self`: `run_entropy_walk` holds a shared borrow of
        // `self` across this stage, so the LR records come back out here.
        let (mut last_lr_unit_size, mut last_lr_stats) =
            (self.last_lr_unit_size, self.last_lr_stats);
        self.search_restoration(
            chroma,
            &hbd_source,
            &mut hbd_used,
            &stop,
            sc_arm,
            w,
            fmt,
            acw,
            filter_chroma,
            encode_input,
            sc_derivation,
            base_qindex,
            coded_lossless,
            &mut recon,
            seq_tools,
            cw,
            lr_true_w,
            lr_true_h,
            &mut u_recon,
            &mut v_recon,
            &mut recon10,
            dlf_is_not_last_layer,
            recon10_pre_cdef,
            &mut lr_signal,
            &mut walk_rest_info,
            decoder_chroma_recon,
            &mut output_restoration,
            &mut last_lr_unit_size,
            &mut last_lr_stats,
        )?;
        self.last_lr_unit_size = last_lr_unit_size;
        self.last_lr_stats = last_lr_stats;

        // The ONE bit-producing walk — C order (rest_process before the EC
        // kernel): armed with whatever CDEF/LR syntax the searches picked,
        // or none. Runs only on the recon-pass split; the no-consumer path
        // above already produced its single full walk. Exactly one of these
        // cases holds: LR syntax (`walk_rest_info`), CDEF syntax
        // (`cdef_params.bits > 0`), both, or neither — the old code ran up
        // to three walks for the same matrix.
        if postfilter_consumed {
            let cdef_walk_opt = (cdef_params.bits > 0).then_some(&cdef_params);
            let (tile_f, _geom_f, u_f, v_f, tsb_f) =
                run_entropy_walk(walk_rest_info.as_ref(), cdef_walk_opt, false)?;
            // The final walk reproduces the PRE-filter recon; u_recon/v_recon
            // were deblocked (and possibly CDEF'd/restored) IN PLACE above,
            // so compare against the pre-deblock copy (the old `== u_recon`
            // form only held on content where chroma deblock was a no-op —
            // it fired spuriously on flat+textured content at mid qp,
            // mainline included, pre-dating the fork work).
            #[cfg(debug_assertions)]
            if let Some((_, u_unf, v_unf)) = self.last_recon_unfiltered.as_ref() {
                debug_assert_eq!(&u_f, u_unf, "final walk chroma recon must be identical");
                debug_assert_eq!(&v_f, v_unf, "final walk chroma recon must be identical");
            }
            let _ = (&u_f, &v_f);
            tile_data = tile_f;
            tile_size_bytes_minus_1 = tsb_f;
        }

        crate::stop_check(&stop)?;

        // Step 7: Build OBU bitstream
        // Use full (non-reduced) sequence header for multi-frame sequences,
        // still-picture header only for single-frame mode. is_single_frame
        // + seq_tools were derived before the entropy walk (the walk codes
        // use_filter_intra flags iff the SH will signal the tool).
        // FH screen-content bits from the pre-walk derivation (see the
        // EntropyCtx::new site): MD palette/IBC candidates are NOT ported
        // yet (#71) — frames the detector fires on still diverge in the
        // tile, but their FH + no-palette flag stream now match C for the
        // palette-only presets M5-M7; M2-M4 additionally need the IBC
        // vertical. Frames it does not fire on are unaffected.
        let sc_signal = crate::entropy::obu::ScSignal {
            allow_screen_content_tools: sc_derivation.allow_screen_content_tools,
            allow_intrabc: sc_derivation.allow_intrabc,
            // Superres chunk B.3: signal what the encode actually did. Off by
            // default -> `SuperresParams::default()` -> zero bits written,
            // i.e. the pre-superres header layout exactly.
            superres: crate::entropy::obu::SuperresParams {
                enabled_in_seq: self.superres_denom.is_some(),
                denom: self.superres_denom,
            },
        };

        // The INTER frame header's picture-level fields, from the SAME
        // derivations the encode used: the reference structure out of
        // `run_picture_decision` and the tool ladders out of
        // `svt_aom_sig_deriv_mode_decision_config_default`. See
        // `crate::inter_hdr_arm`.
        let mut inter_signal = self.derive_inter_signal(
            display_order,
            is_key,
            &pic_decision,
            base_qindex,
            primary_ref_frame_for_cdf,
            gm_models,
            seq_tools,
            md_config_signals,
        )?;

        // The header's `global_motion_params()`. `inter_signal` leaves both
        // arrays IDENTITY (it has no access to the search); they are filled
        // here from the same two values the tile's mode decision uses, so the
        // header and the pack cannot disagree about the model a GLOBALMV block
        // was priced against.
        if let Some(signal) = inter_signal.as_mut() {
            for i in 0..8 {
                signal.global_motion[i] = gm_field[i].into();
                signal.ref_global_motion[i] = ref_gm_field[i].into();
            }
            signal.sync_is_global();
        }

        if let (Some(signal), Some(fg)) = (inter_signal.as_mut(), film_grain.as_ref()) {
            let is_b = pic_decision
                .as_ref()
                .is_some_and(|p| p.slice_type == crate::port_picstruct::SliceType::B);
            signal.film_grain_ref_idx = self.film_grain_reference(fg, signal.ref_frame_idx, is_b);
        }

        // The tile above coded its MVP contexts from `inter_mvp_env`, which
        // derived `use_ref_frame_mvs` from the same two rules
        // `inter_hdr_arm::inter_signal` applies. Assert rather than assume:
        // that bit is the ONLY term that sets the GLOBALMV bit of
        // `mode_context` on a block with no coded neighbours (§1t), so a
        // disagreement silently moves a `newmv` CDF row and is invisible in
        // any byte count.
        if let (Some(sig), Some(env)) = (inter_signal.as_ref(), inter_mvp_env.as_ref()) {
            assert_eq!(
                sig.use_ref_frame_mvs.unwrap_or(false),
                env.use_ref_frame_mvs,
                "the header's use_ref_frame_mvs must equal the one the tile's MVP used",
            );
        }

        // ONE assembly path for both frame types. It used to fork into a
        // separate, monochrome-shaped `write_inter_frame` that shared none of
        // the key frame's derivations — so the inter header could not carry
        // the deblock levels, the CDEF strengths, the LR types, real
        // tile_info() or the chroma quantizer deltas the encode actually used.
        // Signaling and application must agree on every one of those or the
        // recon desyncs from a conforming decoder.
        let bitstream = self.assemble_bitstream(
            chroma,
            is_key,
            frame_tx_mode_select,
            base_qindex,
            tile_rows_log2,
            tile_cols_log2,
            chroma_deltas,
            delta_q_res_signal,
            lf_sharp_eff,
            qm_levels,
            &film_grain,
            is_single_frame,
            seq_tools,
            tile_data,
            tile_size_bytes_minus_1,
            lf_levels,
            &cdef_params,
            lr_signal,
            sc_signal,
            inter_signal,
        );

        // C's deblock search truncates odd chroma dimensions. A decoder
        // filters the ceiling-sized plane. Replay the signaled filters on
        // the output copy after all decisions and entropy coding are complete.
        let mut decoder_output8 = None;
        self.decoder_chroma_recon_stage(
            w,
            h,
            acw,
            filter_chroma,
            lf_sharp_eff,
            deblock_geom,
            &mut recon10,
            lf_levels,
            &cdef_params,
            decoder_chroma_recon,
            output_restoration,
            &mut decoder_output8,
        );

        crate::stop_check(&stop)?;

        // Step 7: Publish recon for the recon-parity gate, then update DPB.
        //
        // Superres chunk B.3: what a DECODER outputs is the coded-width recon
        // normatively upscaled back to `upscaled_width` (C
        // `svt_av1_superres_upscale_frame`, cdef_process.c:152 — after CDEF,
        // before loop restoration; LR is off for every config this port lets
        // superres run at, see `superres_config_error`, so "after CDEF" is
        // here). The BITSTREAM is unaffected: nothing downstream of this point
        // codes symbols. No-op when superres is off.
        // The OUTPUT-side upscaled planes (the `Some` arms exist only under
        // superres). `recon` / `decoder_output8` / `recon10` themselves stay
        // at the CODED geometry — the DPB reference built below must carry
        // the picture a decoder predicts from, not the display picture.
        // (This used to upscale `recon` in place; `padded_ref` then read the
        // upscaled buffer at the coded stride — a diagonal smear that made
        // every inter prediction under superres score the wrong reference.)
        let mut out8: Option<(Vec<u8>, Vec<u8>, Vec<u8>)> = None;
        let mut out10: Option<(Vec<u16>, Vec<u16>, Vec<u16>)> = None;
        self.superres_upscale_stage(
            chroma,
            fmt,
            &recon,
            &u_recon,
            &v_recon,
            &recon10,
            &decoder_output8,
            &mut out8,
            &mut out10,
        )?;

        // C `pad_ref_and_set_flags` (enc_dec_process.c:1088) opens with
        // `pad_input_picture` on the recon itself: "Non visible Reference
        // samples should be overwritten by the last visible line of
        // pixels". A later frame's motion-compensated read past the
        // signalled edge lands in exactly this region, and must find the
        // edge pixel every conforming decoder's edge extension produces —
        // not the coded pad columns' recon. Missing it is what made
        // monochrome inter frames diverge from aomdec/dav1d at sizes that
        // are not 8-aligned (measured 2026-09-25: 65x64, 65x67, 70x64,
        // 100x96 ...). The u8 planes below get the same treatment.
        let (tw, th) = (self.true_width as usize, self.true_height as usize);
        if let Some((y10, u10, v10)) = recon10.as_mut() {
            recon_output::pad_ref_visible_edge(y10, w, tw, th, h);
            let (tcw, tch) = (fmt.chroma_width(tw), fmt.chroma_height(th));
            recon_output::pad_ref_visible_edge(u10, acw, tcw, tch, ach);
            recon_output::pad_ref_visible_edge(v10, acw, tcw, tch, ach);
        }
        let padded_ref_hbd = self.build_padded_ref_hbd(chroma, w, h, ss_x, acw, ach, &recon10);

        // The 8-bit reference canvas at `bit_depth > 8`. C does not keep a
        // separately-reconstructed 8-bit recon: the packed 10-bit picture's
        // `y_buffer` holds `recon10 >> 2`, and every 8-bit reader — ME's
        // `enhanced_pic` references, `svt_inter_predictor_light_pd1`'s u8
        // prediction at `hbd_md = 0` — MCs against those MSBs
        // (resource_coordination_process.c:512-560 "10bit packed"). The port
        // instead stored the u8-domain funnel recon (u8 pred + u8-domain
        // dequant), which differs from `recon10 >> 2` by a few LSBs per
        // sample; the resulting inter predictions landed ~2% off C's
        // distortion and flipped near-tie candidate rankings (MEASURED
        // johnny 128x128 p6 bd10 frame 1, mi=(16,8): C ranked compound
        // NEW_NEWMV 36223238 ahead of NEARMV 36574531; the port's canvas
        // scored them 35619078 vs 35276099 and chose the unipred).
        // Downconverting the SAME post-filter 10-bit canvas C packs makes
        // the u8 reference identical by construction.
        let recon_msb8: Option<(
            alloc::vec::Vec<u8>,
            alloc::vec::Vec<u8>,
            alloc::vec::Vec<u8>,
        )> = recon10.as_ref().map(|(y10, u10, v10)| {
            // Monochrome carries empty chroma planes — keep them empty.
            let down = |p: &[u16], n: usize| -> alloc::vec::Vec<u8> {
                if p.is_empty() {
                    alloc::vec::Vec::new()
                } else {
                    p[..n].iter().map(|&s| (s >> 2) as u8).collect()
                }
            };
            let cn = acw * ach;
            (down(y10, w * h), down(u10, cn), down(v10, cn))
        });

        self.emit_recon_output(
            display_order,
            w,
            h,
            ss_x,
            &recon,
            &film_grain,
            &u_recon,
            &v_recon,
            recon10,
            decoder_output8,
            out8,
            out10,
        );
        // C `pad_ref_and_set_flags` (enc_dec_process.c:1072-1112): the recon
        // is padded with a replicated margin BEFORE it becomes a reference,
        // because inter prediction indexes negative offsets from pixel
        // (0,0). Built here, once, from the same buffers stored below.
        // The `pad_input_picture` half of that call — the non-visible
        // columns/rows overwritten with the visible edge — runs in place
        // on the stored recon so `padded_ref`, `y_plane` and every other
        // reference-plane reader share the decoder's edge extension.
        recon_output::pad_ref_visible_edge(&mut recon, w, tw, th, h);
        if chroma.is_some() {
            let (tcw, tch) = (fmt.chroma_width(tw), fmt.chroma_height(th));
            recon_output::pad_ref_visible_edge(&mut u_recon, acw, tcw, tch, ach);
            recon_output::pad_ref_visible_edge(&mut v_recon, acw, tcw, tch, ach);
        }
        let padded_ref = self.build_padded_ref(
            chroma,
            w,
            h,
            ss_x,
            ss_y,
            &recon,
            &u_recon,
            &v_recon,
            padded_ref_hbd,
            &recon_msb8,
        );
        // C `rest_process.c:347-349`, run on EVERY coded picture (the RC
        // reads them even for a non-reference frame) — `intra_coded_area` is
        // forced to 0 on an I_SLICE there, which is why a key frame stores
        // only its skip/hp areas.
        let coded_area_pct = frame_coded_area
            .borrow()
            .as_ref()
            .map_or((0, 0, 0), |a| a.percentages(w, h, is_key));
        // The join to C's `SVT_REFSTATS_OUT` interposer, which prints the
        // reference object's `slice_type/intra/skip/hp` for the frame that
        // READS it. This prints the same four for the frame that WRITES it,
        // so `REFSTATS poc=N ... l0=a/b/c/d` on the C side must equal
        // `PORTREFSTATS poc=N-1 slice=a intra=b skip=c hp=d` here.
        dump_refstats(display_order, is_key, &frame_coded_area, coded_area_pct);
        #[cfg(feature = "std")]
        if crate::dbgenv::pd0dbg() {
            let acc = frame_coded_area.borrow();
            eprintln!(
                "REFSTORE poc={display_order} intra={:?} skip={:?}",
                acc.as_ref().map(|a| a.sb_intra.as_slice()),
                acc.as_ref().map(|a| a.sb_skip.as_slice()),
            );
        }
        let ref_frame = self.build_reference_frame(
            chroma,
            display_order,
            is_key,
            pic_decision,
            temporal_layer,
            &frame_me,
            base_qindex,
            tpl_r0,
            recon,
            gm_field,
            sb_min_sq_sizes,
            sb_max_sq_sizes,
            &frame_coded_area,
            walk_end_cdfs,
            u_recon,
            v_recon,
            lf_levels,
            dlf_dist_dev,
            cdef_params,
            cdef_dist_dev,
            recon_msb8,
            padded_ref,
            coded_area_pct,
        );
        dump_mvs(&ref_frame);
        #[cfg(feature = "std")]
        if crate::dbgenv::mfmv_dbg() {
            let mut named = 0usize;
            let mut hash: u64 = 1469598103934665603;
            for m in &ref_frame.mvs {
                if m.ref_frame > 0 {
                    named += 1;
                }
                hash = (hash ^ (m.mv.as_int() as u32 as u64)).wrapping_mul(1099511628211);
                hash = (hash ^ u64::from(m.ref_frame as u8)).wrapping_mul(1099511628211);
            }
            std::eprintln!(
                "RS_MVSAVE poc={display_order} ftype={} oh={} roh={},{},{},{},{},{},{} \
                 cells={} named={named} hash={hash:x}",
                u8::from(!is_key),
                ref_frame.order_hint,
                ref_frame.ref_order_hint[0],
                ref_frame.ref_order_hint[1],
                ref_frame.ref_order_hint[2],
                ref_frame.ref_order_hint[3],
                ref_frame.ref_order_hint[4],
                ref_frame.ref_order_hint[5],
                ref_frame.ref_order_hint[6],
                ref_frame.mvs.len(),
            );
        }
        for slot in 0..8 {
            if pcs.refresh_frame_flags & (1 << slot) != 0 {
                self.grain_references[slot] = film_grain.clone();
            }
        }
        self.grain_sequence_present = Some(seq_tools.film_grain_params_present);
        self.dpb.refresh(pcs.refresh_frame_flags, ref_frame);
        self.stash_pa_picture(&pcs, pa_cur);
        // This frame's ME results have been consumed by mode decision and the
        // pack; keep the allocation for the next frame's search.
        self.me_scratch = frame_me;

        crate::stop_check(&stop)?;

        // C `rc_process_packetization_feedback`'s one-pass CBR arm
        // (rc_process.c:758-792): the packetized bit count
        // (`output_stream_ptr->n_filled_len << 3`,
        // packetization_process.c:818) feeds `svt_av1_rc_postencode_update`,
        // then `svt_aom_update_rc_counts` advances the counters.
        // `n_filled_len` is the bitstream buffer WITHOUT the temporal
        // delimiter — `svt_aom_encode_td_av1` writes it into the reorder
        // queue, not `pcs->bitstream_ptr` — so the 2-byte `12 00` TD header
        // this port prepends to every TU comes back out of the count.
        // `avg_cnt_zeromv` is the `rest_process.c:350` normalization of the
        // `update_b`-accumulated zero-MV area.
        if let Some(frame) = cbr_frame_rc.as_mut() {
            let avg_cnt_zeromv = frame_coded_area.borrow().as_ref().map_or(0, |a| {
                let n = (w * h) as u64;
                (100 * a.zeromv_area).checked_div(n).unwrap_or(0)
            });
            self.cbr_postencode(
                frame,
                bitstream.len().saturating_sub(2) as u64 * 8,
                avg_cnt_zeromv,
            );
        }
        // Step 8: Update rate control state
        update_rc_state(&mut self.rc_state, bitstream.len() as u64 * 8, pcs.qp);

        // Task #6 chunk 1 — no silent 8-bit fallback. If the caller supplied a
        // native 10-bit source and NO bd10 stage read it (an out-of-envelope
        // tree turned the level post-pass off at runtime, say), the bytes
        // above encode the MSB-truncated content. Emitting them would look
        // exactly like a real 10-bit encode, so fail loudly instead. The u8
        // path never takes this branch (`hbd_source` is `None` there), and the
        // frame counter is left un-advanced so the caller can retry a
        // supported config on the same pipeline.
        if hbd_source.is_some() && !hbd_used {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "native 10-bit source went unconsumed (the bd10 level re-encode was skipped for \
                 this frame's partition trees) — the encode would have silently truncated to 8 \
                 bits; see docs/hbd-input-port-map.md chunk 2",
            )));
        }
        self.frame_count += 1;
        Ok(bitstream)
    }
}
