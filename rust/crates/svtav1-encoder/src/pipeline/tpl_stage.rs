use super::*;

impl EncodePipeline {
    /// The TPL stage for one released window — C's
    /// `store_extended_group` + `set_tpl_group`/`set_tpl_params` per picture
    /// (`initial_rc_process.c`), `tpl_prep_info`/`tpl_mc_flow`
    /// (`src_ops_process.c`), and `svt_aom_generate_r0beta`
    /// (`rc_init_frame_stats`), all at `scs->tpl_lad_mg == 0`: C's
    /// lookahead-backed extended group (`ctx->lad_queue`, which is what
    /// makes a group span `tpl_lad_mg + 1` mini-GOPs) degenerates to exactly
    /// the in-flight mini-GOP this pipeline already buffers, so the whole
    /// stage runs inside `encode_ra_window` with no pipeline restructure.
    ///
    /// Runs AFTER `run_ra_picture_decision` (every member's `PicParams`
    /// exists) and BEFORE the emit loop. Two groups are processed, matching
    /// C's per-BASE-picture processing in `initial_rc_process`:
    ///
    /// * The held key frame's group — `[key] + this window in decode order`
    ///   (C's delayed intra is released into the next mini-GOP's batch, so
    ///   its `ext_group` is the queue's whole contents;
    ///   `limited_tpl_group_size` for an I-slice base is
    ///   `1 + (tpl_lad_mg + 1) * mg_size`).
    /// * The window base's group — the window in decode order
    ///   (`(tpl_lad_mg + 1) * mg_size` members).
    ///
    /// Each member's OWN `r0`/`tpl_beta`/`tpl_rdmult_scaling_factors` come
    /// from the group run whose base it is — C recomputes them per base
    /// picture and later runs overwrite `pa_me_data->tpl_stats`.
    ///
    /// The per-member open-loop ME results the dispenser consumes are the
    /// SAME `FrameMe` the picture's own encode computes — identical current
    /// and reference pyramids — so each member's [`FrameTplIn`] carries its
    /// `frame_me` for `encode_frame_impl` to reuse, matching C's
    /// `pa_me_data->me_results` ownership (PA ME runs once per picture,
    /// ahead of both src-ops and enc-dec).
    pub(super) fn run_tpl_stage(
        &mut self,
        frames: &[RaBufferedFrame],
        pics: &[Option<crate::port_picstruct::PicParams>],
        emit: &[usize],
        frames_tf: &[crate::port_tf_driver::TfPicBufs],
    ) -> EncodeResult<Option<TplStageOut>> {
        use crate::port_picstruct as pp;
        use crate::port_tpl as pt;

        // `scs->tpl` (`get_tpl`, enc_handle.c:3657) — the SEQ gate; every
        // picture's own `tpl_ctrls.enable` is derived below per picture.
        if !self.scs_tpl() {
            return Ok(None);
        }
        // `scs->tpl_lad_mg` — C's lookahead in mini-GOP units
        // (`initial_rc_process.c`'s lad-queue window). This pipeline's
        // one-mini-GOP RA buffer is the `tpl_lad_mg == 0` shape; the
        // two-mini-GOP `tpl_lad_mg == 1` shape needs a deeper lookahead
        // queue and is refused upstream for now.
        let tpl_lad_mg = 0u8;
        // C's `scs->tpl_lad_mg` CONFIG value (enc_handle.c:4041-4063) — NOT
        // the port's group shape. `allintra || LOW_DELAY -> 0`; else
        // `look_ahead < mg_size -> 0`, `scs->tpl -> 1` (an `scs->tpl` short
        // of this stage already returned above). The RA window this
        // pipeline buffers IS one full mini-GOP, so C's
        // `look_ahead_distance < mg_size` test is false wherever TPL runs.
        // MEASURED (johnny 256x256 9f qp40 p6 hier3 aq2): C's
        // `r0_adjust_factor` is 0 for every picture because
        // `!scs->tpl_lad_mg` is false — the port's structural 0 wrongly fed
        // `set_tpl_group` here and divided every inter `r0` by
        // `0.8 * tpl_hl_base_frame_div_factor[3]` = 1.6, dropping
        // `qstep_ratio`/`base_q_idx` (poc8: port 104 vs C 128) and halving
        // the inter SB lambda (poc8 sb1: port 37775 vs C 76453).
        let scs_tpl_lad_mg: u8 = if self.gop.intra_period == 1
            || self.pred_structure == crate::port_picstruct::PredStructure::LowDelay
        {
            0
        } else {
            1
        };
        let (tw, th) = (self.true_width as usize, self.true_height as usize);
        let (aw, ah) = (self.width as usize, self.height as usize);
        let sb_size = self.sb_size;
        let enc_mode_allintra = self.gop.intra_period == 1;
        let input_resolution =
            crate::port_enc_mode_config::ResolutionRange::from_luma_area((aw * ah) as u32) as u8;

        // `svt_aom_get_tpl_group_level`/`get_tpl_params_level` are
        // enc_mode-keyed; `eff_enc_mode` applies C's M11 clamp.
        let tpl_group_level = |is_i: bool| {
            let sc_arm = if enc_mode_allintra {
                crate::sc_detect::ScArm::Allintra
            } else {
                crate::sc_detect::ScArm::Video { is_islice: is_i }
            };
            let em = crate::rate_arm::eff_enc_mode(sc_arm, self.speed_config.preset);
            pp::get_tpl_group_level(1, em)
        };
        let tpl_params_level = |is_i: bool| {
            let sc_arm = if enc_mode_allintra {
                crate::sc_detect::ScArm::Allintra
            } else {
                crate::sc_detect::ScArm::Video { is_islice: is_i }
            };
            let em = crate::rate_arm::eff_enc_mode(sc_arm, self.speed_config.preset);
            pp::get_tpl_params_level(em)
        };

        // Per-picture `tpl_ctrls` — `set_tpl_group` + `set_tpl_params` on the
        // picture's own `TplPicParams`, exactly C's `initial_rc_process`
        // sequence. `(ctrls, synth_blk_size)` per ra_input member plus the
        // held key.
        let pic_tpl = |pic: &pp::PicParams| -> (pp::TplControls, u8) {
            let (mut t, synth) = pp::set_tpl_group(
                Some(&pp::TplPicParams {
                    slice_type: pic.slice_type,
                    hierarchical_levels: pic.hierarchical_levels,
                    input_resolution,
                    // C `pcs->scs->tpl_lad_mg` — the CONFIG knob C's
                    // `r0_adjust_factor` gate reads, not the port's
                    // group-shape `tpl_lad_mg` (which stays 0).
                    tpl_lad_mg: scs_tpl_lad_mg,
                    rate_control_mode: self.rc_config.mode.into(),
                }),
                tpl_group_level(pic.slice_type == pp::SliceType::I),
                self.width,
                self.height,
            );
            pp::set_tpl_params(
                &mut t,
                tpl_params_level(pic.slice_type == pp::SliceType::I),
                input_resolution,
            );
            (t, synth)
        };

        let n = frames.len();
        let member_tpl: alloc::vec::Vec<(pp::TplControls, u8)> = (0..n)
            .map(|i| pic_tpl(pics[i].as_ref().expect("PD ran for every member")))
            .collect();
        let key_ctx: Option<(
            pp::PicParams,
            pp::TplControls,
            u8,
            &crate::inter_me_arm::PaPicture,
        )> = self.delayed_intra_out.as_ref().map(|(bufs, pic)| {
            let (t, synth) = pic_tpl(pic);
            (pic.clone(), t, synth, &bufs.pa)
        });

        // ---------------------------------------------------------------
        // Member PA pyramids — the `EbPaReferenceObject->input_padded_pic`
        // equivalent. A TF-filtered member reuses `frames_tf[i].pa` (the
        // filtered pyramid the encode also sees); an unfiltered member gets
        // its own pyramid built on the true->aligned replicated pad, the
        // same bytes `encode_frame_impl` pads for it.
        // ---------------------------------------------------------------
        let mut owned_pa: alloc::vec::Vec<Option<crate::inter_me_arm::PaPicture>> =
            (0..n).map(|_| None).collect();
        for (i, f) in frames.iter().enumerate() {
            if frames_tf.get(i).is_none() {
                let y_pad = if aw == tw && ah == th {
                    f.y.clone()
                } else {
                    pad_plane_replicate(&f.y, tw, tw, th, aw, ah)?
                };
                owned_pa[i] = Some(crate::inter_me_arm::PaPicture::from_source(
                    &y_pad,
                    aw,
                    aw,
                    ah,
                    f.display_order,
                ));
            }
        }
        let ra_pa: alloc::vec::Vec<&crate::inter_me_arm::PaPicture> = (0..n)
            .map(|i| {
                frames_tf
                    .get(i)
                    .map(|t| &t.pa)
                    .or(owned_pa[i].as_ref())
                    .expect("member PA built above")
            })
            .collect();

        // Resolve a reference POC to its PA source: an in-window member's
        // own `ra_pa` entry, the held key's pyramid (staged but not yet in
        // `pa_slots` — `run_tpl_stage` runs before `encode_delayed_intra`),
        // else the coded picture's `pa_slots` DPB pyramid — the three
        // origins C's `ref_pa_pic_ptr_array` merges.
        let pa_for_poc = |poc: u64, slot: usize| -> Option<&crate::inter_me_arm::PaPicture> {
            ra_pa
                .iter()
                .find(|p| p.picture_number == poc)
                .copied()
                .or_else(|| {
                    key_ctx
                        .as_ref()
                        .map(|k| k.3)
                        .filter(|p| p.picture_number == poc)
                })
                .or_else(|| self.pa_slots.get(slot).and_then(|s| s.as_deref()))
        };

        // ---------------------------------------------------------------
        // Member open-loop ME — `pa_me`'s output, shared between the TPL
        // dispenser (as `me_results`) and the picture's own encode.
        // `run_frame_me_into` on the same reference pyramids the emit path
        // resolves, so reusing it is byte-identical.
        // ---------------------------------------------------------------
        let mut frame_mes: alloc::vec::Vec<Option<crate::inter_me_arm::FrameMe>> =
            (0..n).map(|_| None).collect();
        for i in 0..n {
            let pic = pics[i].as_ref().unwrap();
            if pic.slice_type == pp::SliceType::I {
                continue;
            }
            let mut refs = crate::inter_me::context::MeRefs::default();
            for rt in 1i8..=7 {
                let (li, ri) = (
                    crate::inter_mvp::get_list_idx(rt),
                    crate::inter_mvp::get_ref_frame_idx(rt),
                );
                let slot = pic.rps.ref_dpb_index[usize::from(rt as u8 - 1)] as usize;
                let poc = pic.rps.ref_poc_array[usize::from(rt as u8 - 1)];
                if let Some(pa) = pa_for_poc(poc, slot) {
                    refs.arr[li][ri] = Some(pa.ds_ref());
                }
            }
            let num_to_search = [pic.ref_list0_count_try, pic.ref_list1_count_try];
            let complete = (0..2)
                .all(|li| (0..usize::from(num_to_search[li])).all(|ri| refs.arr[li][ri].is_some()));
            #[cfg(feature = "std")]
            if crate::dbgenv::medbg() {
                std::eprintln!(
                    "MERPS poc={} rps_poc={:?} rps_idx={:?} ns={:?} refs=[{:?},{:?},{:?},{:?}] complete={complete}",
                    pic.picture_number,
                    pic.rps.ref_poc_array,
                    pic.rps.ref_dpb_index,
                    num_to_search,
                    refs.arr[0][0].map(|d| d.picture_number),
                    refs.arr[0][1].map(|d| d.picture_number),
                    refs.arr[1][0].map(|d| d.picture_number),
                    refs.arr[1][1].map(|d| d.picture_number),
                );
            }
            if !complete {
                continue;
            }
            let sc_arm = crate::sc_detect::ScArm::Video { is_islice: false };
            // The member's own `sc_class5` — `derive_sc` on the same bytes
            // the encode sees.
            let y_src = frames_tf
                .get(i)
                .map(|t| t.extract_luma())
                .unwrap_or_else(|| {
                    owned_pa[i]
                        .as_ref()
                        .map(|p| {
                            let v = &p.full;
                            let mut out = alloc::vec![0u8; v.width * v.height];
                            for r in 0..v.height {
                                out[r * v.width..r * v.width + v.width].copy_from_slice(
                                    &v.buf[v.org + r * v.stride..v.org + r * v.stride + v.width],
                                );
                            }
                            out
                        })
                        .unwrap_or_default()
                });
            let sc_derivation =
                crate::sc_detect::derive_sc(sc_arm, self.speed_config.preset, &y_src, aw, aw, ah);
            let mut out = self
                .me_scratch
                .take()
                .unwrap_or_else(crate::inter_me_arm::FrameMe::empty);
            crate::inter_me_arm::run_frame_me_into(
                &mut out,
                ra_pa[i],
                &refs,
                num_to_search,
                crate::inter_me_arm::FrameMeParams {
                    enc_mode: crate::rate_arm::eff_enc_mode(sc_arm, self.speed_config.preset),
                    qp: self.rc_config.qp,
                    width: aw,
                    height: ah,
                    picture_number: frames[i].display_order,
                    frame_is_boosted: pp::frame_is_boosted(pic),
                    hierarchical_levels: pic.hierarchical_levels,
                    temporal_layer_index: pic.temporal_layer_index,
                    is_ref: pic.is_ref,
                    sc_class5: u8::from(sc_derivation.classes.sc_class5),
                    only_l_bwd: self.mrp_ctrls.only_l_bwd != 0,
                    safe_limit_nref: self.mrp_ctrls.safe_limit_nref,
                    safe_limit_zz_th: self.mrp_ctrls.safe_limit_zz_th,
                    // C `pcs->similar_brightness_refs` / `frame_is_leaf(pcs)`
                    // — picture decision's outputs, gating the safe-limit ME
                    // arm (`motion_estimation.c:2231`).
                    similar_brightness_refs: pic.similar_brightness_refs,
                    frame_is_leaf: pp::frame_is_leaf(pic.update_type),
                },
            );
            frame_mes[i] = Some(out);
        }
        // The held key's ME — an I slice never searches; C's
        // `pa_me_data->me_results` for an I picture is untouched and the
        // dispenser never reads it, so the row carries `None`.

        // ---------------------------------------------------------------
        // Group flow — one `tpl_prep_info` + `tpl_mc_flow` +
        // `generate_r0beta` per group base.
        // ---------------------------------------------------------------
        let stats_len = |synth: u8| -> usize {
            let mb_w = aw.div_ceil(16);
            let mb_h = ah.div_ceil(16);
            match synth {
                8 => (mb_w << 1) * (mb_h << 1),
                32 => aw.div_ceil(32) * ah.div_ceil(32),
                _ => mb_w * mb_h,
            }
        };
        let factor_grid_len = |synth_blk_32: bool, mi_rows: i32| -> usize {
            let n = if synth_blk_32 { 8 } else { 4 };
            let mi_cols_sr = ((aw as i32 + 15) / 16) << 2;
            let cols = (mi_cols_sr + n - 1) / n;
            let rows = (mi_rows + n - 1) / n;
            (rows * cols) as usize
        };
        let sb_cnt = aw.div_ceil(sb_size) * ah.div_ceil(sb_size);
        // The dispenser's 64x64-block geometry — C iterates `scs->b64_geom`
        // (always 64 px, `is_full_64x64` gates the search level).
        let b64_geom: alloc::vec::Vec<(u32, u32, bool)> = (0..(aw.div_ceil(64) * ah.div_ceil(64))
            as u32)
            .map(|i| {
                let g = crate::port_pcs_geom::b64_geom(aw as u16, ah as u16, 64, i);
                (u32::from(g.org_x), u32::from(g.org_y), g.is_complete_b64)
            })
            .collect();
        let sb_orgs: alloc::vec::Vec<(u32, u32)> = (0..sb_cnt as u32)
            .map(|i| {
                let g = crate::port_pcs_geom::sb_geom(aw as u16, ah as u16, sb_size as u16, i);
                (u32::from(g.org_x), u32::from(g.org_y))
            })
            .collect();

        // ---------------------------------------------------------------
        // The two group runs, in C's decode-order processing sequence:
        // the held key's `[key] + window` group first (its initial_rc runs
        // first), then the window base's. Each fills its members'
        // `tpl_stats` grids; the window run is the LAST writer, which is
        // what `generate_r0beta` for each member then reads — matching C,
        // where every picture's own initial_rc later re-dispenses the
        // group and overwrites `pa_me_data->tpl_stats`.
        // ---------------------------------------------------------------

        /// One member of a TPL group run: its PA source pyramid, its
        /// picture-decision row, its own `tpl_ctrls`, and its open-loop ME
        /// results (`None` only for an I slice).
        struct GroupRow<'a> {
            pa: &'a crate::inter_me_arm::PaPicture,
            pic: &'a pp::PicParams,
            ctrls: &'a pp::TplControls,
            me: Option<&'a crate::inter_me_arm::FrameMe>,
        }

        /// `ext_group` + `store_extended_group` over a queue slice — the
        /// group C's `initial_rc_process` builds when the head of `rows`
        /// is the base. Every row shares `ext_mg_id` (C's `mg_progress_id`
        /// is one release batch).
        fn group_for(rows: &[GroupRow<'_>], ext_mg_id: i64, tpl_lad_mg: u8) -> pp::TplGroup {
            let ext_group: alloc::vec::Vec<pp::ExtGroupPic> = rows
                .iter()
                .map(|r| pp::ExtGroupPic {
                    picture_number: r.pic.picture_number,
                    slice_type: r.pic.slice_type,
                    temporal_layer_index: r.pic.temporal_layer_index,
                    ext_mg_id,
                    is_delayed_intra: r.pic.is_delayed_intra,
                    is_skipped: false,
                })
                .collect();
            pp::store_extended_group(
                &ext_group,
                rows[0].pic.slice_type,
                rows[0].pic.hierarchical_levels,
                u32::from(tpl_lad_mg),
                rows[0].ctrls.reduced_tpl_group,
            )
        }

        /// `tpl_prep_info` + `tpl_mc_flow` + `generate_r0beta` for ONE
        /// group base — the `src_ops_process.c`/`rc_init_frame_stats` half.
        /// `rows` is the picture's `ext_group` in decode order; `rows[0]` is
        /// the base whose `initial_rc` is being modeled.
        ///
        /// C's `tpl_mc_flow` runs the dispenser+synthesizer ONLY when the
        /// group base's `tpl_data.tpl_temporal_layer_index == 0`; a non-tl0
        /// base's `initial_rc` builds the group but dispenses nothing, so
        /// this returns `(group, [])` for those. When the flow does run it
        /// rewrites EVERY group member's stats (invalid members keep the
        /// memset zeros), and `generate_r0beta` is evaluated per member —
        /// exactly the `pa_me_data` state a member's own
        /// `rc_init_frame_stats` would later read, since the LAST tl0 run
        /// containing a member is what its stats hold by then.
        /// Returns `(group, [(rows index, r0beta, factors, beta)])`.
        #[allow(clippy::too_many_arguments)]
        fn run_group<'pa>(
            base_synth: u8,
            rows: &[GroupRow<'_>],
            tpl_lad_mg: u8,
            qp: u8,
            ext_off: i32,
            sb_size: usize,
            aw: usize,
            ah: usize,
            tw: usize,
            th: usize,
            b64_geom: &[(u32, u32, bool)],
            sb_orgs: &[(u32, u32)],
            stats_len: usize,
            factor_grid_len: usize,
            sb_cnt: usize,
            pa_slots: &'pa [Option<alloc::sync::Arc<crate::inter_me_arm::PaPicture>>; 8],
            ra_pa: &[&'pa crate::inter_me_arm::PaPicture],
            key_pa: Option<&'pa crate::inter_me_arm::PaPicture>,
        ) -> (
            pp::TplGroup,
            alloc::vec::Vec<(
                usize,
                pt::R0Beta,
                alloc::vec::Vec<f64>,
                alloc::vec::Vec<f64>,
            )>,
        ) {
            let g = group_for(rows, 0, tpl_lad_mg);
            let mut out: alloc::vec::Vec<(
                usize,
                pt::R0Beta,
                alloc::vec::Vec<f64>,
                alloc::vec::Vec<f64>,
            )> = alloc::vec::Vec::new();
            if g.members.is_empty() {
                return (g, out);
            }
            /// A `PaPicture`'s enhanced luma as a [`pt::TplPic`] view — the
            /// `EbPaReferenceObject->input_padded_pic` equivalent.
            fn pa_as_tpl<'pa>(pa: &'pa crate::inter_me_arm::PaPicture) -> pt::TplPic<'pa> {
                let f = &pa.full;
                pt::TplPic {
                    y: &f.buf,
                    y_stride: f.stride,
                    width: f.width as u32,
                    height: f.height as u32,
                    max_width: (f.width + 2 * f.border) as u32,
                    max_height: (f.height + 2 * f.border) as u32,
                    origin: f.org,
                }
            }
            /// Resolve a ref POC to its PA pyramid — an in-window member's
            /// `ra_pa` entry, the held key's, or the coded picture's
            /// `pa_slots` DPB pyramid.
            fn resolve_pa<'pa>(
                poc: u64,
                slot: usize,
                ra_pa: &[&'pa crate::inter_me_arm::PaPicture],
                key_pa: Option<&'pa crate::inter_me_arm::PaPicture>,
                pa_slots: &'pa [Option<alloc::sync::Arc<crate::inter_me_arm::PaPicture>>; 8],
            ) -> Option<&'pa crate::inter_me_arm::PaPicture> {
                ra_pa
                    .iter()
                    .find(|p| p.picture_number == poc)
                    .copied()
                    .or_else(|| key_pa.filter(|p| p.picture_number == poc))
                    .or_else(|| pa_slots.get(slot).and_then(|s| s.as_deref()))
            }

            // `tpl_ref_ds` PA-source table — every (list, ref) resolves to
            // its PA pyramid. C stores the `EbPaReferenceObject` directly;
            // the port carries indices into this dense table.
            let mut ref_pics: alloc::vec::Vec<pt::TplPic<'_>> = alloc::vec::Vec::new();

            let prep: alloc::vec::Vec<pt::TplPrepPic> = g
                .members
                .iter()
                .map(|&ext_i| {
                    let row = &rows[ext_i];
                    let pic = row.pic;
                    let mut ref_pic_poc = [[0u64; 8]; 2];
                    let mut ref_pic_index = [[0usize; 8]; 2];
                    let mut ref_poc_ds = [[0u64; 8]; 2];
                    for (list, count) in [pic.ref_list0_count_try, pic.ref_list1_count_try]
                        .iter()
                        .enumerate()
                    {
                        for ri in 0..usize::from(*count) {
                            let rt = list * 4 + ri;
                            let poc = pic.rps.ref_poc_array[rt];
                            let slot = pic.rps.ref_dpb_index[rt] as usize;
                            ref_pic_poc[list][ri] = poc;
                            ref_poc_ds[list][ri] = poc;
                            ref_pic_index[list][ri] =
                                match resolve_pa(poc, slot, ra_pa, key_pa, pa_slots) {
                                    Some(pa) => {
                                        ref_pics.push(pa_as_tpl(pa));
                                        ref_pics.len() - 1
                                    }
                                    None => {
                                        // A ref the RA structure offered but no
                                        // source resolves: cannot happen on a
                                        // consistent RPS — keep a degenerate
                                        // entry rather than emit wrong stats
                                        // silently.
                                        debug_assert!(
                                            false,
                                            "tpl: ref poc {poc} slot {slot} unresolved"
                                        );
                                        ref_pics.push(pt::TplPic {
                                            y: &[],
                                            y_stride: 0,
                                            width: 0,
                                            height: 0,
                                            max_width: 0,
                                            max_height: 0,
                                            origin: 0,
                                        });
                                        ref_pics.len() - 1
                                    }
                                };
                        }
                    }
                    pt::TplPrepPic {
                        slice_type: pic.slice_type,
                        temporal_layer_index: pic.temporal_layer_index,
                        is_ref: pic.is_ref,
                        decode_order: pic.decode_order as i32,
                        picture_number: pic.picture_number,
                        ref_list_count_try: [pic.ref_list0_count_try, pic.ref_list1_count_try],
                        ref_pic_poc,
                        ref_pic_index,
                        ref_poc_ds,
                    }
                })
                .collect();
            let group_pocs: alloc::vec::Vec<u64> = prep.iter().map(|p| p.picture_number).collect();
            let tpl_datas = pt::tpl_prep_info(&group_pocs, &prep);

            // C gates the WHOLE dispenser+synthesizer on the BASE's tpl
            // temporal layer (`src_ops_process.c`: `tpl_group[0]->tpl_data
            // .tpl_temporal_layer_index == 0`). A non-tl0 base's `initial_rc`
            // built the group (tpl_group_size is still consumed later) but
            // rewrote no stats — members keep what the last tl0 run left.
            if tpl_datas[0].tpl_temporal_layer_index != 0 {
                return (g, out);
            }

            // Per-member TplWindowFrame — owned stats grids sized for the
            // BASE's synth block size (the whole group shares the base's
            // `tpl_ctrls.synth_blk_size` inside `tpl_mc_flow`).
            let mut stats_store: alloc::vec::Vec<alloc::vec::Vec<pt::TplStats>> = g
                .members
                .iter()
                .map(|_| alloc::vec![pt::TplStats::default(); stats_len])
                .collect();
            let mut src_stats_store: alloc::vec::Vec<alloc::vec::Vec<pt::TplSrcStats>> =
                g.members.iter().map(|_| alloc::vec::Vec::new()).collect();
            // Recon planes — the `mc_flow_rec_picture_buffer` pool. One
            // buffer per member: stride aw+64, 32-px top/left border
            // (TPL_PAD is 32).
            let recon_stride = aw + 64;
            let mut recon_store: alloc::vec::Vec<alloc::vec::Vec<u8>> = g
                .members
                .iter()
                .map(|_| alloc::vec![0u8; recon_stride * (ah + 64)])
                .collect();

            // The base's `tpl_valid_pic`, MEMBER-ordered — `ref_tpl_group_
            // idx` is a position in `group_pocs`, not an ext index.
            let member_valid: alloc::vec::Vec<u8> = g.members.iter().map(|&i| g.valid[i]).collect();

            let qp_qindex = crate::rate_control::QUANTIZER_TO_QINDEX[qp.min(63) as usize];
            let quant_for = |row: &GroupRow<'_>| -> crate::quant::QuantTable {
                let qi = pt::tpl_qindex(
                    qp,
                    ext_off,
                    row.ctrls.enable_tpl_qps != 0,
                    row.pic.slice_type,
                    row.pic.hierarchical_levels,
                    row.pic.temporal_layer_index,
                );
                crate::quant::build_quant_table(qi.clamp(0, i32::from(u8::MAX)) as u8)
            };

            // Member metadata the dispense closure binds per frame_idx.
            struct MemberCtx<'a> {
                row: &'a GroupRow<'a>,
                quant: crate::quant::QuantTable,
            }
            let member_ctxs: alloc::vec::Vec<MemberCtx<'_>> = g
                .members
                .iter()
                .map(|&ext_i| MemberCtx {
                    row: &rows[ext_i],
                    quant: quant_for(&rows[ext_i]),
                })
                .collect();

            let mut window: alloc::vec::Vec<pt::TplWindowFrame<'_>> = g
                .members
                .iter()
                .enumerate()
                .zip(
                    stats_store
                        .iter_mut()
                        .zip(src_stats_store.iter_mut())
                        .zip(tpl_datas.into_iter()),
                )
                .map(
                    |((k, &ext_i), ((stats, src_stats), tpl_data))| pt::TplWindowFrame {
                        picture_number: rows[ext_i].pic.picture_number,
                        valid: member_valid[k] != 0,
                        tpl_data,
                        stats,
                        src_stats,
                        aligned_width: aw as u32,
                        mi_rows: rows[ext_i].pic.mi_rows as i32,
                        mi_cols: rows[ext_i].pic.mi_cols as i32,
                        tpl_src_data_ready: false,
                    },
                )
                .collect();
            let mut recons: alloc::vec::Vec<pt::TplPicMut<'_>> = recon_store
                .iter_mut()
                .map(|buf| pt::TplPicMut {
                    y: buf,
                    y_stride: recon_stride,
                    width: aw as u32,
                    height: ah as u32,
                    border: 32,
                    origin: 32 * recon_stride + 32,
                })
                .collect();

            let mut base_rdmults = alloc::vec![0i32; g.members.len()];
            let sf_identity = svtav1_dsp::port_scale_factors::ScaleFactors::setup_for_frame(
                aw as i32, ah as i32, aw as i32, ah as i32,
            );
            let qp_cfg = qp;
            let ext_off_cfg = ext_off;
            pt::tpl_mc_flow(
                &mut window,
                &mut recons,
                base_synth,
                rows[0].ctrls.compute_rate != 0,
                tpl_lad_mg,
                |args| {
                    let mc = &member_ctxs[args.frame_idx];
                    let row = mc.row;
                    let pic = row.pic;
                    let me = row.me;
                    base_rdmults[args.frame_idx] = pt::tpl_mc_flow_dispenser(
                        qp_cfg,
                        ext_off_cfg,
                        b64_geom,
                        |_sb_index| pt::TplSbCtx {
                            input_pic: {
                                let f = &row.pa.full;
                                pt::TplPic {
                                    y: &f.buf,
                                    y_stride: f.stride,
                                    width: f.width as u32,
                                    height: f.height as u32,
                                    max_width: (f.width + 2 * f.border) as u32,
                                    max_height: (f.height + 2 * f.border) as u32,
                                    origin: f.org,
                                }
                            },
                            tpl_ctrls: *row.ctrls,
                            tpl_data: &args.frame.tpl_data,
                            temporal_layer_index: pic.temporal_layer_index,
                            hierarchical_levels: pic.hierarchical_levels,
                            slice_type: pic.slice_type,
                            update_type: pic.update_type,
                            qp_qindex,
                            extended_crf_qindex_offset: ext_off_cfg,
                            tpl_src_data_ready: args.frame.tpl_src_data_ready,
                            enable_me_16x16: me.map_or(true, |m| m.enable_me_16x16),
                            tpl_lad_mg,
                            sb_origin: (0, 0),
                            aligned_width: aw as u32,
                            aligned16_width: aw.div_ceil(16),
                            mi_rows: pic.mi_rows as i32,
                            mi_cols: pic.mi_cols as i32,
                            max_input_luma_width: tw as u32,
                            max_input_luma_height: th as u32,
                            super_block_size: sb_size as i32,
                            sf_identity,
                            quant: mc.quant,
                            poc_map_idx: args.poc_map_idx,
                            base_tpl_valid_pic: &member_valid,
                        },
                        args.recon,
                        args.ref_pics,
                        &ref_pics,
                        |sb| {
                            let Some(me) = me else {
                                return pt::TplMeResults {
                                    total_me_candidate_index: &[],
                                    me_candidate_array: &[],
                                    me_mv_array: &[],
                                    max_cand: 0,
                                    max_refs: 0,
                                    max_l0: 0,
                                };
                            };
                            let b64 = &me.per_b64[sb];
                            pt::TplMeResults {
                                total_me_candidate_index: &b64.total_me_candidate_index,
                                me_candidate_array: &b64.me_candidate_array,
                                me_mv_array: &b64.me_mv_array,
                                max_cand: me.max_cand,
                                max_refs: me.max_refs,
                                max_l0: me.max_l0,
                            }
                        },
                        args.frame.src_stats,
                        args.frame.stats,
                    );
                    if crate::dbgenv::dispdbg() {
                        let (mut s, mut r, mut d) = (0i64, 0i64, 0i64);
                        for st in args.frame.stats.iter() {
                            s += st.srcrf_dist;
                            r += st.recrf_dist;
                            d += st.mc_dep_dist;
                        }
                        eprintln!(
                            "DISPDBG idx={} poc={} src={} rec={} dep={} rdmult={} nref={},{}",
                            args.frame_idx,
                            pic.picture_number,
                            s,
                            r,
                            d,
                            base_rdmults[args.frame_idx],
                            args.frame.tpl_data.tpl_ref0_count,
                            args.frame.tpl_data.tpl_ref1_count,
                        );
                        for l in 0..2 {
                            for ri in 0..pt::REF_LIST_MAX_DEPTH {
                                eprintln!(
                                    "  REF l{} r{} poc={} sw={} gidx={}",
                                    l,
                                    ri,
                                    args.frame.tpl_data.tpl_ref_ds[l][ri].picture_number,
                                    args.frame.tpl_data.ref_in_slide_window[l][ri] as i32,
                                    args.frame.tpl_data.ref_tpl_group_idx[l][ri],
                                );
                            }
                        }
                    }
                    0
                },
            );

            // `generate_r0beta` per member — each member's own
            // `rc_init_frame_stats` reads the stats this run left in ITS
            // `pa_me_data` (the run only executes for tl0 bases, so these
            // are the values a member actually sees: the last tl0 run that
            // contained it).
            for (k, &ext_i) in g.members.iter().enumerate() {
                let row = &rows[ext_i];
                let flags = crate::rate_control::r0_flags(
                    row.ctrls.enable != 0,
                    row.ctrls.reduced_tpl_group,
                    row.pic.temporal_layer_index,
                    row.pic.hierarchical_levels,
                    row.pic.slice_type == pp::SliceType::I,
                );
                let mut member_factors = alloc::vec![0.0f64; factor_grid_len];
                let mut member_beta = alloc::vec![0.0f64; sb_cnt];
                let rb = if flags.r0_gen {
                    pt::generate_r0beta(
                        base_synth,
                        sb_size as u32,
                        aw as u32,
                        ah as u32,
                        aw as u32,
                        ah as u32,
                        8,
                        row.pic.mi_rows as i32,
                        i64::from(base_rdmults[k]),
                        &stats_store[k],
                        sb_orgs,
                        &mut member_factors,
                        &mut member_beta,
                    )
                } else {
                    member_factors.clear();
                    member_beta.clear();
                    pt::R0Beta {
                        r0: 0.0,
                        tpl_is_valid: false,
                    }
                };
                out.push((ext_i, rb, member_factors, member_beta));
            }
            (g, out)
        }

        // ---------------------------------------------------------------
        // C's ordering: EVERY picture's `initial_rc` builds its own
        // `tpl_group` from its queue tail, but `tpl_mc_flow` only dispenses
        // for tl0 bases. A member's `pa_me_data` stats therefore hold
        // whatever the LAST tl0-base run containing it wrote — the runs
        // below execute in decode order so later writes win, exactly like
        // the shared `pa_me_data` buffers in C.
        // ---------------------------------------------------------------
        let mut stage = TplStageOut {
            frames: (0..n).map(|_| None).collect(),
            key: None,
        };
        // Per-member "the stats a member's rc_init_frame_stats would read":
        // (rb, factors, beta) from the most recent tl0 run containing it.
        let mut last: alloc::vec::Vec<
            Option<(pt::R0Beta, alloc::vec::Vec<f64>, alloc::vec::Vec<f64>)>,
        > = (0..n).map(|_| None).collect();
        // Each member's OWN group shape — `pcs->tpl_group_size` /
        // `used_tpl_frame_num` come from the member's own
        // `store_extended_group`, which runs for every picture regardless
        // of the tl0 flow gate.
        let mut own: alloc::vec::Vec<(u32, u32)> = (0..n).map(|_| (0, 0)).collect();
        let make_fti = |row: &GroupRow<'_>,
                        base_synth: u8,
                        rb: &pt::R0Beta,
                        factors: alloc::vec::Vec<f64>,
                        beta: alloc::vec::Vec<f64>,
                        own: (u32, u32)|
         -> pt::FrameTplIn {
            let pic = row.pic;
            pt::FrameTplIn {
                tpl_ctrls: *row.ctrls,
                synth_blk_size: base_synth,
                flags: crate::rate_control::r0_flags(
                    row.ctrls.enable != 0,
                    row.ctrls.reduced_tpl_group,
                    pic.temporal_layer_index,
                    pic.hierarchical_levels,
                    pic.slice_type == pp::SliceType::I,
                ),
                r0: rb.r0,
                tpl_is_valid: rb.tpl_is_valid,
                tpl_group_size: own.0,
                used_tpl_frame_num: own.1,
                tpl_beta: beta,
                tpl_rdmult_scaling_factors: factors,
                // Bound in the post-run sweep — `rows` still borrows
                // `frame_mes`/`key_frame_me` here.
                frame_me: None,
            }
        };

        // ---------------------------------------------------------------
        // The held key's group: `[key] + window` — the key's lad-queue
        // contents when its `initial_rc` ran (a delayed intra leaves the
        // whole queued mini-GOP behind it).
        // ---------------------------------------------------------------
        if let Some((key_pic, key_ctrls, key_synth, key_pa)) = key_ctx.as_ref() {
            let mut rows: alloc::vec::Vec<GroupRow<'_>> = alloc::vec::Vec::with_capacity(n + 1);
            rows.push(GroupRow {
                pa: key_pa,
                pic: key_pic,
                ctrls: key_ctrls,
                me: None,
            });
            for &i in emit {
                rows.push(GroupRow {
                    pa: ra_pa[i],
                    pic: pics[i].as_ref().unwrap(),
                    ctrls: &member_tpl[i].0,
                    me: frame_mes[i].as_ref(),
                });
            }
            let base_synth = *key_synth;
            let (g, results) = run_group(
                base_synth,
                &rows,
                tpl_lad_mg,
                self.rc_config.qp,
                i32::from(self.rc_config.extended_crf_qindex_offset),
                sb_size,
                aw,
                ah,
                tw,
                th,
                &b64_geom,
                &sb_orgs,
                stats_len(base_synth),
                factor_grid_len(base_synth == 32, key_pic.mi_rows as i32),
                sb_cnt,
                &self.pa_slots,
                &ra_pa,
                Some(key_pa),
            );
            // The key's own `store_extended_group` output IS `g`; member
            // results land in `last` — a later tl0 run may still overwrite
            // them (its group contains the whole window).
            for (ext_i, rb, factors, beta) in results {
                if ext_i == 0 {
                    stage.key = Some(make_fti(
                        &rows[0],
                        base_synth,
                        &rb,
                        factors,
                        beta,
                        (g.members.len() as u32, g.used_tpl_frame_num),
                    ));
                } else {
                    last[emit[ext_i - 1]] = Some((rb, factors, beta));
                }
            }
        }

        // ---------------------------------------------------------------
        // Per-member runs — every picture's `initial_rc` builds its own
        // group from the queue tail `emit[k..]`. `run_group` only writes
        // results for tl0 bases (the `tpl_mc_flow` gate), so `last` ends
        // holding exactly what C's shared `pa_me_data` would: each member's
        // stats as the LAST tl0 run containing it left them.
        // ---------------------------------------------------------------
        for (k, &i) in emit.iter().enumerate() {
            let rows: alloc::vec::Vec<GroupRow<'_>> = emit[k..]
                .iter()
                .map(|&j| GroupRow {
                    pa: ra_pa[j],
                    pic: pics[j].as_ref().unwrap(),
                    ctrls: &member_tpl[j].0,
                    me: frame_mes[j].as_ref(),
                })
                .collect();
            let base_synth = member_tpl[i].1;
            let (g, results) = run_group(
                base_synth,
                &rows,
                tpl_lad_mg,
                self.rc_config.qp,
                i32::from(self.rc_config.extended_crf_qindex_offset),
                sb_size,
                aw,
                ah,
                tw,
                th,
                &b64_geom,
                &sb_orgs,
                stats_len(base_synth),
                factor_grid_len(base_synth == 32, rows[0].pic.mi_rows as i32),
                sb_cnt,
                &self.pa_slots,
                &ra_pa,
                // Out-of-window refs still resolve to the held key — the
                // base frame's L0 reference is the previous GOP's key
                // picture, which is not yet in `pa_slots` at stage time.
                key_ctx.as_ref().map(|k| k.3),
            );
            // `g` is member `i`'s OWN group — record its shape for the fti
            // even when the tl0 gate skipped the flow.
            own[i] = (g.members.len() as u32, g.used_tpl_frame_num);
            for (ext_i, rb, factors, beta) in results {
                last[emit[k + ext_i]] = Some((rb, factors, beta));
            }
        }

        // Materialize each member's `FrameTplIn` from `last` + `own`.
        for (i, f) in stage.frames.iter_mut().enumerate() {
            if let Some((rb, factors, beta)) = last[i].take() {
                let row = GroupRow {
                    pa: ra_pa[i],
                    pic: pics[i].as_ref().unwrap(),
                    ctrls: &member_tpl[i].0,
                    me: None,
                };
                *f = Some(make_fti(&row, member_tpl[i].1, &rb, factors, beta, own[i]));
            }
        }

        // Move each member's `FrameMe` into its `FrameTplIn` — the group
        // `rows` are all dropped now, so the borrows have ended.
        for (i, f) in stage.frames.iter_mut().enumerate() {
            if let Some(fti) = f {
                if let Some(me) = frame_mes[i].take() {
                    fti.frame_me = Some(me);
                }
            }
        }
        // The held key's `frame_me` stays `None` — an I slice ran no
        // open-loop ME.

        Ok(Some(stage))
    }
}
