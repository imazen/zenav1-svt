use super::*;

impl EncodePipeline {
    #[inline(always)]
    pub(super) fn build_inter_syntax_state(
        &self,
        display_order: u64,
        pic_decision: &Option<crate::port_picstruct::PicParams>,
        seq_tools: crate::entropy::obu::SeqTools,
        md_config_signals: Option<crate::port_enc_mode_config::md_config::MdConfigSignals>,
        gm_field: [svtav1_types::motion::WarpedMotionParams; 8],
    ) -> Option<InterSyntaxState> {
        let inter_syntax_state: Option<InterSyntaxState> = md_config_signals.map(|sigs| {
            let mut ref_order_hint = [0i32; 7];
            if let Some(pic) = pic_decision.as_ref() {
                for (i, oh) in ref_order_hint.iter_mut().enumerate() {
                    let slot = pic.rps.ref_dpb_index[i] as usize;
                    *oh = self.dpb.get(slot).map_or(0, |r| r.order_hint as i32);
                }
            }
            InterSyntaxState {
                // C `pd_process.c:4958`, and NOT the constant this port used
                // to imply: `skip_mode_flag` IS `skip_mode_allowed`.
                skip_mode_flag: pic_decision
                    .as_ref()
                    .is_some_and(|p| p.skip_mode.skip_mode_allowed != 0),
                // C `frm_hdr->skip_mode_params.ref_frame_idx_{0,1}` —
                // `setup_skip_mode_allowed` leaves them `INVALID_IDX` (-1)
                // when the frame's reference structure has no skip-mode
                // pair (single reference, or order-hint signalling off).
                skip_mode_ref_frame_idx_0: pic_decision
                    .as_ref()
                    .map_or(-1, |p| p.skip_mode.ref_frame_idx_0 as i8),
                skip_mode_ref_frame_idx_1: pic_decision
                    .as_ref()
                    .map_or(-1, |p| p.skip_mode.ref_frame_idx_1 as i8),
                // C `frm_hdr->reference_mode`. The port has no compound
                // candidate yet, but the SYMBOL layout depends on this bit
                // and the header writes it, so it must be the header's value
                // and not a convenient constant.
                // C `frm_hdr->reference_mode`, i.e. the header's
                // `reference_select` bit — `inter_hdr_arm::inter_signal`
                // derives it from `pic.reference_mode` and this reads the
                // same field, so the tile and the header cannot disagree.
                reference_mode: match pic_decision.as_ref().map(|p| p.reference_mode) {
                    Some(crate::port_picstruct::ReferenceMode::Select) => {
                        crate::port_entropy_inter::refframe::ReferenceMode::Select
                    }
                    _ => crate::port_entropy_inter::refframe::ReferenceMode::Single,
                },
                interpolation_filter: sigs.interpolation_filter,
                enable_dual_filter: seq_tools.enable_dual_filter,
                enable_interintra_compound: seq_tools.enable_interintra_compound,
                enable_masked_compound: seq_tools.enable_masked_compound,
                enable_jnt_comp: seq_tools.enable_jnt_comp,
                enable_order_hint: seq_tools.enable_order_hint,
                order_hint_bits: u32::from(crate::entropy::obu::ORDER_HINT_BITS),
                is_motion_mode_switchable: sigs.is_motion_mode_switchable,
                allow_warped_motion: sigs.allow_warped_motion,
                allow_high_precision_mv: sigs.allow_high_precision_mv != 0,
                // C keeps `frm_hdr->force_integer_mv = 0` unconditionally
                // (resource_coordination_process.c:362), which is also the
                // bit `write_uncompressed_header` emits (obu.rs:1421). Read
                // from the same place rather than from a signal that has no
                // such field.
                force_integer_mv: false,
                // C `pcs->ppcs->global_motion[ref].wmtype`, read by the
                // entropy walk to decide whether a block's mode is a GLOBALMV
                // that codes no MV. It is the SAME array the frame header
                // wrote (`gm_field`, above), converted through the one
                // `WarpParams` conversion, so the header and the per-block
                // walk cannot disagree about a reference's model.
                gm_wmtype: core::array::from_fn(|i| {
                    crate::port_entropy_inter::gm::WarpParams::from(gm_field[i]).wmtype
                }),
                cur_order_hint: display_order as i32,
                ref_order_hint,
                // C `mfmv_controls` (enc_mode_config.c:8853) for the VALUE
                // and `frame_might_allow_ref_frame_mvs`
                // (entropy_coding.h:71) for its PRESENCE — the same two
                // rules `inter_hdr_arm::inter_signal` applies to write the
                // header bit, asserted equal to it below.
                //
                // This used to spell the VALUE as `mfmv_level == 1`, a THIRD
                // transcription of a function ported once in
                // `port_enc_mode_config::tail` and re-derived in
                // `inter_hdr_arm`. It agreed with C only because every level
                // above 1 was refused before it could be reached; the moment
                // level 2 was allowed it would have been a silent
                // disagreement in a bit that moves a `newmv` CDF row. It now
                // calls the same shared port the header does.
                use_ref_frame_mvs: !crate::dbgenv::mfmv_off()
                    && seq_tools.enable_ref_frame_mvs
                    && seq_tools.enable_order_hint
                    && crate::port_enc_mode_config::tail::mfmv_controls(
                        crate::port_enc_mode_config::tail::MfmvInputs {
                            mfmv_level: sigs.mfmv_level,
                            is_base: pic_decision
                                .as_ref()
                                .is_some_and(|p| p.temporal_layer_index == 0),
                            tpl: self.scs_tpl(),
                            r0_gen: false,
                            r0: 0.0,
                            is_b_slice: pic_decision.as_ref().is_some_and(|p| {
                                p.slice_type == crate::port_picstruct::SliceType::B
                            }),
                            ref_list1_count_try: pic_decision
                                .as_ref()
                                .map_or(0, |p| u32::from(p.ref_list1_count_try)),
                            ref_l0_is_mfmv_used: false,
                            ref_l1_is_mfmv_used: false,
                        },
                    )
                    .is_some_and(|v| v != 0),
            }
        });
        inter_syntax_state
    }

    #[inline(always)]
    pub(super) fn build_inter_mvp_env(
        &self,
        display_order: u64,
        pic_decision: &Option<crate::port_picstruct::PicParams>,
        w: usize,
        h: usize,
        sb_size: usize,
        gm_field: [svtav1_types::motion::WarpedMotionParams; 8],
        inter_syntax_state: &Option<InterSyntaxState>,
        inter_ref_frame_side: &mut [i8; 8],
    ) -> Option<crate::partition::InterMdEnv> {
        let inter_mvp_env: Option<crate::partition::InterMdEnv> =
            inter_syntax_state.as_ref().map(|st| {
                let (mi_cols, mi_rows) = (w.div_ceil(4) as i32, h.div_ceil(4) as i32);
                let tpl_stride = (mi_cols + 1) >> 1;
                crate::partition::InterMdEnv {
                    mi_stride: mi_cols,
                    mi_rows,
                    mi_cols,
                    tile: crate::intrabc::TileMiBounds {
                        mi_col_start: 0,
                        mi_col_end: mi_cols,
                        mi_row_start: 0,
                        mi_row_end: mi_rows,
                    },
                    sb_mi_size: (sb_size / 4) as i32,
                    global_motion: gm_field,
                    allow_high_precision_mv: st.allow_high_precision_mv,
                    force_integer_mv: st.force_integer_mv,
                    use_ref_frame_mvs: st.use_ref_frame_mvs,
                    order_hint_info: crate::inter_mvp::OrderHintInfo {
                        enable_order_hint: st.enable_order_hint,
                        order_hint_bits: st.order_hint_bits,
                    },
                    cur_order_hint: st.cur_order_hint,
                    // `inter_mvp` indexes by `MvReferenceFrame`
                    // (LAST = 1 ..= ALTREF = 7, slot 0 unused); the entropy
                    // side's array is `ref_frame - 1`.
                    ref_order_hint: {
                        let mut a = [0i32; 8];
                        a[1..8].copy_from_slice(&st.ref_order_hint);
                        a
                    },
                    // C `pcs->av1_cm->ref_frame_sign_bias[8]`
                    // (`svt_av1_setup_frame_sign_bias`,
                    // pd_process.c:4894-4909) — already derived into
                    // `pic_decision` by `port_picstruct::set_ref_frame_sign_bias`
                    // on every frame; zeroed on a key or when order hints
                    // are off.
                    ref_frame_sign_bias: pic_decision
                        .as_ref()
                        .map_or([0; 8], |p| p.ref_frame_sign_bias),
                    // Stamped below with `pic_decision` — the picture-level
                    // `temporal_layer_index > 0 && RANDOM_ACCESS` half of
                    // C's `symteric_refs` gate; the list half is evaluated
                    // per call inside `generate_av1_mvp_table`.
                    symmetric_refs_eligible: false,
                    // C `av1_setup_motion_field`, run over this picture's own
                    // DPB references. On a two-frame cell every projection
                    // returns 0 — LAST is the KEY frame and C aborts on
                    // `start_frame_buf->frame_type == KEY_FRAME`
                    // (md_config_process.c:441) — so every cell stays
                    // `INVALID_MV` and `add_tpl_ref_mv` returns 0, which is
                    // what sets the GLOBALMV bit of `mode_context` (§1t).
                    // From the SECOND inter frame on, LAST has a saved motion
                    // field and this is where it enters the ref-MV stack.
                    tpl_mvs: {
                        let mut tpl = alloc::vec![
                            crate::inter_mvp::TplMvRef::default();
                            (((mi_rows + 32) >> 1) * tpl_stride) as usize
                        ];
                        let refs = crate::inter_mvp::MotionFieldRefs {
                            refs: core::array::from_fn(|idx| {
                                let pic = pic_decision.as_ref()?;
                                let slot = pic.rps.ref_dpb_index[idx] as usize;
                                let rf = self.dpb.get(slot)?;
                                // C's field always exists at the reference's
                                // own half-mi extent. A DPB entry that carries
                                // none is one this port wrote before the
                                // writeback existed, or an allintra picture
                                // that can never be a reference in a GOP; C's
                                // own `mi_rows != cm->mi_rows` abort covers a
                                // different-SIZE reference, but nothing in
                                // that function can see a SHORT slice, so the
                                // length is checked here rather than indexed
                                // on faith.
                                let rf_mi_rows = (rf.height as i32 + 3) >> 2;
                                let rf_mi_cols = (rf.width as i32 + 3) >> 2;
                                if rf.mvs.len()
                                    != (((rf_mi_rows + 1) >> 1) * ((rf_mi_cols + 1) >> 1)) as usize
                                {
                                    return None;
                                }
                                Some(crate::inter_mvp::RefMotionField {
                                    mvs: &rf.mvs,
                                    order_hint: rf.order_hint as i32,
                                    ref_order_hint: rf.ref_order_hint,
                                    is_intra_only: rf.is_islice,
                                    mi_rows: rf_mi_rows,
                                    mi_cols: rf_mi_cols,
                                })
                            }),
                        };
                        *inter_ref_frame_side = crate::inter_mvp::setup_motion_field(
                            &mut tpl,
                            tpl_stride,
                            mi_rows,
                            mi_cols,
                            st.cur_order_hint,
                            crate::inter_mvp::OrderHintInfo {
                                enable_order_hint: st.enable_order_hint,
                                order_hint_bits: st.order_hint_bits,
                            },
                            st.use_ref_frame_mvs,
                            &refs,
                        );
                        #[cfg(feature = "std")]
                        if let Some(path) = crate::dbgenv::raw_var_os("SVTAV1_TPL_OUT") {
                            // Diagnostic twin of the vendored-libaom `TPL`
                            // dump (AOM_TPL_OUT): the projected temporal-MV
                            // field this frame's ref-MV scan consumes,
                            // emitted in libaom's `as_int` packing
                            // (row | col<<16) so the two can be diffed.
                            use std::io::Write as _;
                            if let Ok(file) = std::fs::OpenOptions::new()
                                .create(true)
                                .append(true)
                                .open(&path)
                            {
                                let mut w = std::io::BufWriter::new(file);
                                let mut line = alloc::string::String::with_capacity(
                                    32 + tpl.len() * 12,
                                );
                                core::fmt::Write::write_fmt(
                                    &mut line,
                                    format_args!("TPL {} {}", st.cur_order_hint, tpl.len()),
                                )
                                .ok();
                                for t in &tpl {
                                    let c_int = (t.mfmv0.y as u16 as u32)
                                        | ((t.mfmv0.x as u16 as u32) << 16);
                                    core::fmt::Write::write_fmt(
                                        &mut line,
                                        format_args!(
                                            " {c_int},{}",
                                            t.ref_frame_offset as i8
                                        ),
                                    )
                                    .ok();
                                }
                                line.push('\n');
                                let _ = w.write_all(line.as_bytes());
                            }
                        }
                        #[cfg(feature = "std")]
                        if crate::dbgenv::mfmv_dbg() {
                            let mut valid = 0usize;
                            let mut hash: u64 = 1469598103934665603;
                            for t in &tpl {
                                if t.ref_frame_offset != 0 || t.mfmv0.as_int() != crate::intrabc_mvp::INVALID_MV {
                                    valid += 1;
                                    hash = (hash ^ (t.mfmv0.as_int() as u32 as u64))
                                        .wrapping_mul(1099511628211);
                                    hash = (hash ^ u64::from(t.ref_frame_offset))
                                        .wrapping_mul(1099511628211);
                                }
                            }
                            std::eprintln!(
                                "RS_MFMV poc={display_order} cur={} use={} mi={mi_rows}x{mi_cols} \
                                 stride={} roh={},{},{},{},{},{},{} side={},{},{},{},{},{},{}",
                                st.cur_order_hint,
                                u8::from(st.use_ref_frame_mvs),
                                tpl_stride * 2,
                                st.ref_order_hint[0], st.ref_order_hint[1], st.ref_order_hint[2],
                                st.ref_order_hint[3], st.ref_order_hint[4], st.ref_order_hint[5],
                                st.ref_order_hint[6],
                                inter_ref_frame_side[1], inter_ref_frame_side[2],
                                inter_ref_frame_side[3], inter_ref_frame_side[4],
                                inter_ref_frame_side[5], inter_ref_frame_side[6],
                                inter_ref_frame_side[7],
                            );
                            std::eprintln!(
                                "RS_MFMVSUM poc={display_order} size={} valid={valid} hash={hash:x}",
                                tpl.len(),
                            );
                        }
                        #[cfg(feature = "std")]
                        if crate::dbgenv::zz_tpl() {
                            let valid = tpl
                                .iter()
                                .filter(|t| t.ref_frame_offset != 0)
                                .count();
                            std::eprintln!(
                                "ZZTPL poc={display_order} cells={} valid={} use_mvs={} refs_present={}",
                                tpl.len(),
                                valid,
                                st.use_ref_frame_mvs,
                                refs.refs.iter().filter(|r| r.is_some()).count(),
                            );
                        }
                        tpl
                    },
                    tpl_stride,
                    // C `ctx->sb64_sq_no4xn_geom` selects the SIMPLIFIED MFMV
                    // block walk, and it means all three of its parts: a 64x64
                    // superblock, SQUARE-only shapes, and no 4xN. This was set
                    // from `sb_size == 64` ALONE, which is true at every preset
                    // this port ships, so the simplified walk ran even where
                    // rectangular blocks exist.
                    //
                    // The simplified walk uses `n4_w` for BOTH the row and the
                    // column extent (`inter_mvp.rs`, the `sb64_sq_no4xn_geom`
                    // arm). On a square block that is the same number; on a
                    // 16x32 it scans four rows instead of eight and never sees
                    // the lower half's temporal candidates. NEARESTMV/NEARMV
                    // derive their MV from that stack, so the encoder and a
                    // decoder pick DIFFERENT motion vectors for the same block.
                    //
                    // MEASURED: with this corrected, vidyo1/vidyo3/vidyo4 all go
                    // from drifting (or failing to decode) to 8 of 8 frames
                    // byte-identical to aomdec.
                    sb_size_64: sb_size == 64,
                }
            });
        inter_mvp_env
    }

    #[inline(always)]
    pub(super) fn derive_sb_inter_lambda(
        &self,
        stop: &almost_enough::StopToken,
        sc_arm: crate::sc_detect::ScArm,
        pic_decision: &Option<crate::port_picstruct::PicParams>,
        temporal_layer: u8,
        frame_hier: u8,
        md_lambda_factor_update_type: crate::port_rc_process::FrameUpdateType,
        md_alt_lambda_factors: bool,
        lambda_mod_intra: i64,
        w: usize,
        h: usize,
        frame_me: &Option<crate::inter_me_arm::FrameMe>,
        base_qindex: u8,
        r0_delta_qp_md: bool,
        picture_qp: u8,
        lw_bump: u32,
        delta_q_present: bool,
        md_sb_qindex: Option<&crate::sb_qindex::SbQindexPlan>,
        sb_size: usize,
        sb_cols: usize,
        sb_rows: usize,
        inter_md_frame: &Option<crate::inter_md_arm::InterMdFrame<'_>>,
    ) -> Result<Option<Vec<crate::pd0::SbInterLambda>>, whereat::prelude::At<EncodeError>> {
        let sb_inter_lambda: Option<Vec<crate::pd0::SbInterLambda>> = match (
            frame_me.as_ref(),
            inter_md_frame.as_ref(),
        ) {
            // C `scs->stats_based_sb_lambda_modulation` (enc_handle.c:4375)
            // reads the POST-clamp `static_config.enc_mode` — so at CLI
            // p12/p13 the non-RTC video arm still sees M11 and the
            // modulation stays ON. When it is off, `generate_sb_qindex`
            // never builds `b64_me_qindex` at all (rc_process.c:747).
            // Skipping the whole binding there leaves every consumer on
            // the frame lambda, which is what C prices with.
            (Some(me), Some(imf))
                if crate::port_rc_process::stats_based_sb_lambda_modulation(
                    crate::rate_arm::eff_enc_mode(sc_arm, self.speed_config.preset),
                    false,
                ) =>
            {
                let mev: Vec<u32> = me.per_b64.iter().map(|o| o.me_8x8_cost_variance).collect();
                let map = crate::port_rc_process::generate_b64_me_qindex_map(
                    &mev,
                    i32::from(base_qindex),
                    /*is_islice=*/ false,
                );
                let lw = crate::pd0::frame_lambda_weight_for_preset(
                    self.speed_config.preset,
                    picture_qp as u32,
                    self.hdr.tune == crate::tune::TUNE_IQ,
                    lw_bump,
                );
                let lctx = crate::port_rc_process::LambdaContext {
                    frame_type: 1, // not KEY_FRAME
                    temporal_layer_index: temporal_layer,
                    hierarchical_levels: frame_hier,
                    update_type: imf.base_update_type,
                    alt_lambda_factors: md_alt_lambda_factors,
                    rtc: false,
                    // C `scs->stats_based_sb_lambda_modulation`
                    // (enc_handle.c:4375). The match guard above already
                    // proved it true for this frame; passing a literal
                    // here would be a second spelling of the same rule.
                    stats_based_sb_lambda_modulation: true,
                    base_q_idx: i32::from(base_qindex),
                    delta_q_present,
                    r0_delta_qp_md,
                    lambda_scale_factors: [128; 7],
                };
                // C `svt_aom_mode_decision_configure_sb` (md_process.c:800-803):
                // `ctx->qp_index = delta_q_present || r0_delta_qp_md ?
                // sb_qp : base_q_idx`. `sb_qp` is the `generate_sb_qindex`
                // map — TPL-derived when `r0_delta_qp_md` ran.
                let qp_mod_arm = delta_q_present || r0_delta_qp_md;
                let mut out = Vec::with_capacity(sb_cols * sb_rows);
                for sb_row in 0..sb_rows {
                    for sb_col in 0..sb_cols {
                        crate::stop_check(stop)?;
                        let sb_idx = sb_row * sb_cols + sb_col;
                        let me_q = crate::port_md_rate_estimation::get_me_qindex(
                            &map,
                            u16::try_from(w).unwrap_or(u16::MAX),
                            u16::try_from(h).unwrap_or(u16::MAX),
                            u32::try_from(sb_idx).unwrap_or(u32::MAX),
                            u32::try_from(sb_col * sb_size).unwrap_or(u32::MAX),
                            u32::try_from(sb_row * sb_size).unwrap_or(u32::MAX),
                            sb_size == 128,
                        );
                        let qp_idx = if qp_mod_arm {
                            md_sb_qindex.map_or(base_qindex, |p| p.sb_qindex[sb_idx])
                        } else {
                            base_qindex
                        };
                        // `update_lambda` picks its stats-factor arm on the
                        // lctx flags: `delta_q_present || r0_delta_qp_md`
                        // uses `q_index - base` at +-8 (rc_process.c:430-441),
                        // else `me_q_index - base` at +-4 (:442-446) — the
                        // same factor PD0's lambda fold below carries as
                        // `me_qdiff`.
                        let me_qdiff = if qp_mod_arm {
                            i32::from(qp_idx) - i32::from(base_qindex)
                        } else {
                            i32::from(me_q) - i32::from(base_qindex)
                        };
                        let raw =
                            crate::port_rc_process::compute_fast_lambda(&lctx, qp_idx, me_q, 8);
                        // C scales `fast_lambda_md` by the same
                        // LAMBDA_MOD_INTRA arm before `lambda_weight`
                        // (md_process.c:740).
                        let raw = ((u64::from(raw) * lambda_mod_intra as u64) >> 7) as u32;
                        // `full_lambda_md[0]` — `svt_aom_compute_rd_mult`
                        // then LAMBDA_MOD_INTRA then `lambda_weight`
                        // (md_process.c:725-751).
                        let mut full_8bit = ((u64::from(crate::port_rc_process::compute_rd_mult(
                            &lctx, qp_idx, me_q, 8,
                        )) * lambda_mod_intra as u64)
                            >> 7) as u32;
                        if lw != 0 {
                            full_8bit = ((u64::from(full_8bit) * u64::from(lw)) >> 7) as u32;
                        }
                        // `full_lambda_md[EB_10_BIT_MD]` — the same chain at
                        // 10 bits, ending in `*16` (md_process.c:728/753).
                        let mut full_10bit = ((u64::from(crate::port_rc_process::compute_rd_mult(
                            &lctx, qp_idx, me_q, 10,
                        )) * lambda_mod_intra as u64)
                            >> 7) as u32;
                        if lw != 0 {
                            full_10bit = ((u64::from(full_10bit) * u64::from(lw)) >> 7) as u32;
                        }
                        // `*full_lambda *= 16` on C's `uint32_t` wraps.
                        let full_10bit = full_10bit.wrapping_mul(16);
                        #[cfg(feature = "std")]
                        if crate::dbgenv::lamdump() {
                            std::eprintln!(
                                "LAM poc={} tl={} sb={} baseq={} but={:?} fut={:?} alt={} meq={} meqd={} lmi={} lw={} -> full={} fast={}",
                                pic_decision
                                    .as_ref()
                                    .map_or(-1, |p| p.picture_number as i64),
                                temporal_layer,
                                sb_idx,
                                base_qindex,
                                imf.base_update_type,
                                md_lambda_factor_update_type,
                                md_alt_lambda_factors,
                                me_q,
                                me_qdiff,
                                lambda_mod_intra,
                                lw,
                                full_8bit,
                                raw,
                            );
                        }
                        out.push(crate::pd0::SbInterLambda {
                            full_8bit,
                            fast_8bit: if lw == 0 {
                                raw
                            } else {
                                ((u64::from(raw) * u64::from(lw)) >> 7) as u32
                            },
                            me_qdiff,
                            full_10bit,
                        });
                    }
                }
                Some(out)
            }
            _ => None,
        };
        Ok(sb_inter_lambda)
    }

    #[inline(always)]
    pub(super) fn derive_pd0_min_sq(
        &self,
        display_order: u64,
        stop: &almost_enough::StopToken,
        pic_decision: &Option<crate::port_picstruct::PicParams>,
        w: usize,
        h: usize,
        frame_me: &Option<crate::inter_me_arm::FrameMe>,
        base_qindex: u8,
        r0_delta_qp_md: bool,
        picture_qp: u8,
        delta_q_present: bool,
        md_sb_qindex: Option<&crate::sb_qindex::SbQindexPlan>,
        sb_size: usize,
        sb_cols: usize,
        sb_rows: usize,
        last_ref_slot: Option<usize>,
        md_config_signals: Option<crate::port_enc_mode_config::md_config::MdConfigSignals>,
        inter_md_frame: &Option<crate::inter_md_arm::InterMdFrame<'_>>,
        sb_inter_lambda: &Option<Vec<crate::pd0::SbInterLambda>>,
        pd0_dr_res: &mut Option<Vec<crate::port_enc_mode_config::common::DepthRemovalResult>>,
    ) -> Result<Option<Vec<u8>>, whereat::prelude::At<EncodeError>> {
        let pd0_min_sq: Option<Vec<u8>> = match (
            md_config_signals.as_ref(),
            frame_me.as_ref(),
            inter_md_frame.as_ref(),
        ) {
            (Some(sigs), Some(me), Some(_imf)) => {
                use crate::port_enc_mode_config::common as pcommon;
                // C `ctx->disallow_8x8` on the VIDEO arm
                // (`sig_deriv_enc_dec_common`, enc_mode_config.c:7122).
                let disallow_8x8 = crate::port_enc_mode_config::leaf::get_disallow_8x8_default();
                // C `pd0_depth_removal`'s reference read is
                // `ref_obj_l0->sb_min_sq_size[sb_index]`, i.e. LAST's — see
                // `last_ref_slot`, not DPB slot 0. C only reads it when the
                // reference is POC-ADJACENT (`abs(picture_number - ref_poc)
                // <= 1`, enc_mode_config.c:3175-3177); a farther ref leaves
                // `sb_min_sq_size` at `(uint8_t)~0` and the deviation
                // thresholds get NO bump — feeding the value unconditionally
                // was the `96x96 hier` over-disallow on poc>=2. The L1
                // `MIN()` arm (:3179-3186) fires on an RA B slice where
                // `ref_list1_count_try` and a same-size BWD reference
                // (`ref_dpb_index[BWD]`) exist, on ITS OWN POC adjacency —
                // unreachable on low-delay, which never populates list 1.
                // `svt_aom_is_ref_same_size` (enc_mode_config.c:2857) gates
                // each list: `is_not_scaled` short-circuits true on the
                // older references. Ghost Robot `507025f65` dropped the
                // shortcut — it checks the dims unconditionally AND requires
                // `ref_list{0,1}_count_try > 0`. `superres_denom` is `None`
                // on every inter frame the port accepts (superres-inter is
                // refused), so under the older references the shortcut is
                // the dims check anyway.
                let gr = self.reference == crate::reference::SvtReference::GhostRobot;
                let ref_same_size = |rf: &crate::picture::ReferenceFrame| {
                    (!gr && self.superres_denom.is_none())
                        || (rf.width == self.width as u32 && rf.height == self.height as u32)
                };
                let ref_l0_adj = last_ref_slot
                    .filter(|_| {
                        // Ghost Robot `507025f65`: the L0 read requires
                        // `ref_list0_count_try > 0` inside `is_ref_same_size`
                        // itself; the older references only need the ref.
                        !gr || pic_decision
                            .as_ref()
                            .is_some_and(|p| p.ref_list0_count_try > 0)
                    })
                    .and_then(|slot| self.dpb.get(slot))
                    .filter(|rf| ref_same_size(rf))
                    .filter(|rf| display_order.abs_diff(rf.display_order) <= 1);
                let ref_l1_adj = pic_decision
                    .as_ref()
                    .filter(|p| {
                        p.slice_type == crate::port_picstruct::SliceType::B
                            && p.ref_list1_count_try > 0
                    })
                    .and_then(|p| {
                        self.dpb
                            .get(p.rps.ref_dpb_index[crate::port_picstruct::BWD] as usize)
                    })
                    .filter(|rf| ref_same_size(rf))
                    .filter(|rf| display_order.abs_diff(rf.display_order) <= 1);
                let mut out = Vec::with_capacity(sb_cols * sb_rows);
                let mut dr_out = Vec::with_capacity(sb_cols * sb_rows);
                for sb_row in 0..sb_rows {
                    for sb_col in 0..sb_cols {
                        crate::stop_check(stop)?;
                        let sb_idx = sb_row * sb_cols + sb_col;
                        let (x0, y0) = (sb_col * sb_size, sb_row * sb_size);
                        let b = me.per_b64.get(sb_idx);
                        // C `ctx->fast_lambda_md[EB_8_BIT_MD]` for THIS
                        // superblock, from the one derivation above — the
                        // depth-removal thresholds are scaled by it
                        // (`set_depth_removal_level_controls`), and C's own
                        // `SVT_PD0CFG_OUT` `fastlam` field is this value.
                        let fast_lambda = sb_inter_lambda
                            .as_ref()
                            .and_then(|v| v.get(sb_idx))
                            .map_or(0, |l| l.fast_8bit);
                        let res = pcommon::set_depth_removal_level_controls(
                            pcommon::DepthRemovalInputs {
                                level: sigs.pic_depth_removal_level,
                                is_islice: false,
                                fast_lambda_8bit: fast_lambda,
                                delta_q_present,
                                r0_delta_qp_md,
                                // `sb_ptr->qindex` — the generate_sb_qindex
                                // map — vs the signalled frame base; the
                                // level modulation arm reads their diff.
                                sb_qindex: md_sb_qindex.map_or(i32::from(base_qindex), |p| {
                                    i32::from(p.sb_qindex[sb_idx])
                                }),
                                picture_qindex: i32::from(base_qindex),
                                picture_qp: i32::from(picture_qp),
                                dist_64: b.map_or(0, |o| o.me_64x64_distortion),
                                dist_32: b.map_or(0, |o| o.me_32x32_distortion),
                                dist_16: b.map_or(0, |o| o.me_16x16_distortion),
                                dist_8: b.map_or(0, |o| o.me_8x8_distortion),
                                me_8x8_cost_variance: b.map_or(0, |o| o.me_8x8_cost_variance),
                                sb_width: u16::try_from(sb_size.min(w - x0)).unwrap_or(u16::MAX),
                                sb_height: u16::try_from(sb_size.min(h - y0)).unwrap_or(u16::MAX),
                                disallow_4x4_in: true,
                                // C `(uint8_t)~0` when neither list's
                                // reference is POC-adjacent — `None` takes
                                // the no-adjustment arm. With both present
                                // C keeps the `MIN()`.
                                ref_sb_min_sq_size: [
                                    ref_l0_adj.and_then(|rf| rf.sb_min_sq_size.get(sb_idx)),
                                    ref_l1_adj.and_then(|rf| rf.sb_min_sq_size.get(sb_idx)),
                                ]
                                .into_iter()
                                .flatten()
                                .copied()
                                .reduce(u8::min),
                            },
                        );
                        let c = res.map(|r| r.ctrls).unwrap_or_default();
                        // C `set_blocks_to_be_tested` (enc_dec_process.c:1485).
                        let min_sq = if c.enabled != 0 && c.disallow_below_64x64 != 0 {
                            64usize
                        } else if c.enabled != 0 && c.disallow_below_32x32 != 0 {
                            32
                        } else if disallow_8x8 || (c.enabled != 0 && c.disallow_below_16x16 != 0) {
                            16
                        } else {
                            // `pic_disallow_4x4` is 1 at every preset this
                            // port reaches, so C's `: 4` arm is unreachable.
                            8
                        };
                        // `SVTAV1_PD0DBG`: the port-side twin of C's
                        // `SVT_PD0CFG_OUT` `dr=<enabled>/<64>/<32>/<16>`
                        // field, same order, so the two join per superblock.
                        #[cfg(feature = "std")]
                        if crate::dbgenv::pd0dbg() {
                            eprintln!(
                                "PD0DR poc={display_order} nb64={} sb={sb_idx} org=({x0},{y0}) dr={}/{}/{}/{} minsq={min_sq} \
                                 drlvl={} fastlam={fast_lambda} pqp={picture_qp} \
                                 med={}/{}/{}/{} mev={} refmin={}/{}",
                                me.per_b64.len(),
                                c.enabled,
                                c.disallow_below_64x64,
                                c.disallow_below_32x32,
                                c.disallow_below_16x16,
                                sigs.pic_depth_removal_level,
                                b.map_or(0, |o| o.me_64x64_distortion),
                                b.map_or(0, |o| o.me_32x32_distortion),
                                b.map_or(0, |o| o.me_16x16_distortion),
                                b.map_or(0, |o| o.me_8x8_distortion),
                                b.map_or(0, |o| o.me_8x8_cost_variance),
                                ref_l0_adj
                                    .and_then(|rf| rf.sb_min_sq_size.get(sb_idx))
                                    .map_or(255, |v| u32::from(*v)),
                                ref_l1_adj
                                    .and_then(|rf| rf.sb_min_sq_size.get(sb_idx))
                                    .map_or(255, |v| u32::from(*v)),
                            );
                        }
                        out.push(
                            u8::try_from(min_sq.min(usize::from(self.hdr.max_tx_size)))
                                .unwrap_or(8),
                        );
                        // The whole result — `sig_deriv_enc_dec_pd0`'s
                        // subres ladder reads `depth_removal_ctrls` and the
                        // post-call `disallow_4x4` per superblock, not just
                        // the folded `min_sq`.
                        dr_out.push(res.unwrap_or(pcommon::DepthRemovalResult {
                            ctrls: pcommon::DepthRemovalCtrls::default(),
                            disallow_4x4: true,
                        }));
                    }
                }
                *pd0_dr_res = Some(dr_out);
                Some(out)
            }
            _ => None,
        };
        Ok(pd0_min_sq)
    }
}

impl EncodePipeline {
    #[inline(always)]
    pub(super) fn derive_ref_gm_field(
        &self,
        pic_decision: &Option<crate::port_picstruct::PicParams>,
        primary_ref_frame_for_cdf: u8,
    ) -> [svtav1_types::motion::WarpedMotionParams; 8] {
        // C `pcs->child_pcs->ref_global_motion[]` (pic_manager_process.c:831):
        // the PRIMARY-REF picture's own saved models, which every parameter is
        // delta-coded against. An I_SLICE reference contributes IDENTITY, and
        // so does `PRIMARY_REF_NONE` — the same slot the CDF continuation
        // resolves, so the two cannot disagree about which picture this frame
        // is coded against.
        let ref_gm_field: [svtav1_types::motion::WarpedMotionParams; 8] =
            if primary_ref_frame_for_cdf == crate::port_picstruct::PRIMARY_REF_NONE {
                [svtav1_types::motion::WarpedMotionParams::default(); 8]
            } else {
                pic_decision
                    .as_ref()
                    .map(|p| p.rps.ref_dpb_index[primary_ref_frame_for_cdf as usize] as usize)
                    .and_then(|slot| self.dpb.get(slot))
                    .map_or(
                        [svtav1_types::motion::WarpedMotionParams::default(); 8],
                        |rf| rf.global_motion,
                    )
            };
        ref_gm_field
    }

    #[inline(always)]
    pub(super) fn build_lpd1_frame(
        &self,
        sc_arm: crate::sc_detect::ScArm,
        pic_decision: &Option<crate::port_picstruct::PicParams>,
        pipeline_md_inputs: &Option<crate::inter_hdr_arm::PipelineMdInputs>,
        md_config_signals: Option<crate::port_enc_mode_config::md_config::MdConfigSignals>,
        // `frm_hdr.coded_lossless` — C's `mimic_only_tx_4x4` input.
        coded_lossless: bool,
    ) -> Option<Lpd1FrameIn> {
        let lpd1_frame = md_config_signals
            .as_ref()
            .zip(pipeline_md_inputs.as_ref())
            .map(|(mc, p)| {
                let enc_mode = crate::rate_arm::eff_enc_mode(sc_arm, self.speed_config.preset);
                Lpd1FrameIn {
                    pic_lpd1_lvl: mc.pic_lpd1_lvl,
                    // C's `coded_lossless` post-step already zeroes
                    // `pic_lpd1_lvl` in `md_config_process`; the field here
                    // feeds Ghost Robot's `a74cfb9ec` per-SB `lpd1_lvl` /
                    // `pd1_lvl_refinement` gates in `resolve_sb_lpd1`.
                    coded_lossless,
                    enc_mode,
                    is_b_slice: pic_decision
                        .as_ref()
                        .is_some_and(|d| d.slice_type == crate::port_picstruct::SliceType::B),
                    input_resolution: p.input_resolution,
                    picture_qp: p.picture_qp,
                    ref_list0_count_try: p.ref_list0_count_try,
                    ref_list1_count_try: p.ref_list1_count_try,
                    reference: self.reference,
                    ref_skip_percentage: crate::inter_hdr_arm::ref_skip_percentage(p),
                    use_best_me_unipred_cand_only: u8::from(
                        enc_mode > crate::port_enc_mode_config::enc_mode::M1,
                    ),
                    merge_inter_cands_mult: if mc.nic_level >= 6 { 4 } else { u8::MAX },
                    cand_reduction_level: mc.cand_reduction_level,
                    rdoq_level: mc.rdoq_level,
                    coeff_shaving_level: mc.coeff_shaving_level,
                    me_subpel_level: mc.me_subpel_level,
                    rate_est_level: mc.rate_est_level,
                    approx_inter_rate: mc.approx_inter_rate,
                    intra_level: mc.intra_level,
                }
            });
        lpd1_frame
    }

    #[inline(always)]
    pub(super) fn derive_ref_min_max_sq(
        &self,
        pic_decision: &Option<crate::port_picstruct::PicParams>,
        last_ref: &Option<alloc::sync::Arc<ReferenceFrame>>,
    ) -> Option<(Vec<u8>, Vec<u8>)> {
        // `update_pred_th_offset`'s `use_ref_info` read
        // (enc_dec_process.c:1611-1621) takes `ref_obj_l0`'s
        // `sb_min_sq_size`/`sb_max_sq_size`, then — on a B slice with
        // `ref_list1_count_try` and a same-size L1 reference — merges
        // `MIN(min, l1_min)` / `MAX(max, l1_max)`. Unlike the depth-removal
        // read above there is NO POC-adjacency gate. Owned vectors because
        // the merge produces a new array; `None` on a key frame.
        let ref_min_max_sq: Option<(Vec<u8>, Vec<u8>)> = last_ref
            .as_ref()
            .filter(|rf| {
                self.superres_denom.is_none()
                    || (rf.width == self.width as u32 && rf.height == self.height as u32)
            })
            .map(|rf| {
                let mut mn = rf.sb_min_sq_size.clone();
                let mut mx = rf.sb_max_sq_size.clone();
                let l1 = pic_decision
                    .as_ref()
                    .filter(|p| {
                        p.slice_type == crate::port_picstruct::SliceType::B
                            && p.ref_list1_count_try > 0
                    })
                    .and_then(|p| {
                        self.dpb
                            .get(p.rps.ref_dpb_index[crate::port_picstruct::BWD] as usize)
                    })
                    .filter(|rf| {
                        self.superres_denom.is_none()
                            || (rf.width == self.width as u32 && rf.height == self.height as u32)
                    });
                if let Some(l1) = l1 {
                    for (m, v) in mn.iter_mut().zip(l1.sb_min_sq_size.iter()) {
                        *m = (*m).min(*v);
                    }
                    for (m, v) in mx.iter_mut().zip(l1.sb_max_sq_size.iter()) {
                        *m = (*m).max(*v);
                    }
                }
                (mn, mx)
            });
        ref_min_max_sq
    }
}
