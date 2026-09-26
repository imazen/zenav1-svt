use super::*;

impl EncodePipeline {
    #[inline(always)]
    pub(super) fn derive_inter_signal(
        &self,
        display_order: u64,
        is_key: bool,
        pic_decision: &Option<crate::port_picstruct::PicParams>,
        base_qindex: u8,
        primary_ref_frame_for_cdf: u8,
        gm_models: Option<crate::port_global_me::GmModels>,
        seq_tools: crate::entropy::obu::SeqTools,
        md_config_signals: Option<crate::port_enc_mode_config::md_config::MdConfigSignals>,
    ) -> Result<Option<crate::entropy::obu::InterSignal>, whereat::prelude::At<EncodeError>> {
        let inter_signal: Option<crate::entropy::obu::InterSignal> = if is_key {
            None
        } else {
            let pic = pic_decision.as_ref().ok_or_else(|| {
                whereat::at!(EncodeError::UnsupportedConfig(
                    "an inter frame reached header signalling without a picture decision — \
                     unreachable since run_picture_decision covers every intra_period != 1 \
                     config (defensive; remove the caller's is_key guard instead of emitting \
                     a header that disagrees with the encode) [C: accepts]",
                ))
            })?;
            let ref_queue =
                crate::inter_hdr_arm::ref_queue_from_dpb(&pic.ref_queue_dpb, base_qindex);
            let binding = crate::port_picstruct::bind_refs_and_primary_ref_frame(
                pic, &ref_queue,
                // C `frame_end_cdf_update_mode` — the picture manager assigns
                // REFRESH_FRAME_CONTEXT_BACKWARD to coded pictures, which is
                // the same fact the header's `disable_frame_end_update_cdf = 0`
                // records (obu.rs).
                true, /*is_s_frame=*/ false,
            );
            // THE FRAME-2 WALL, MOVED 2026-09-03 and now naming the gap
            // that is actually left.
            //
            // It used to be `md_config_inputs` returning `None` for any
            // `get_ref_hp_percentage` answer other than its -1 sentinel,
            // because the port carried NONE of the three coded-area
            // statistics C reads off a reference. It carries all three now —
            // `ReferenceFrame::{intra,skip,hp}_coded_area`, accumulated in
            // the walk by `CodedAreaAcc` exactly where C's `update_b` does,
            // and VERIFIED against C's own reference objects
            // (`SVT_REFSTATS_OUT` on `gradient 64x64 q32 p8 frames=3`): C
            // reads its frame-2 list-0 reference as `0/0/100/0`
            // (slice/intra/skip/hp) and this port writes exactly
            // `slice=0 intra=0 skip=100 hp=0` onto frame 1's DPB entry.
            //
            // WHAT IS STILL MISSING — RE-KEYED 2026-09-03, because the
            // mechanism this refusal used to name was NOT the first
            // divergence and the number it quoted was a DIFFERENT defect's.
            //
            // It said frame 2 needs `pd0_detector`'s per-superblock
            // `ref_obj_l0->sb_intra[sb_index]` and cited "466 B against C's
            // 21". That 466 B was the port PREDICTING FRAME 2 FROM THE WRONG
            // PICTURE: `ref_frame_data` / `ref_padded_luma` / the PD0
            // `sb_min_sq_size` read read a hard-coded DPB SLOT 0, where C
            // resolves LAST through `pic.rps.ref_dpb_index[LAST]` — slot 1 at
            // poc 2. With that fixed (see `last_ref_slot`) the same cell codes
            // **22 B against C's 21**, and the frame-2 frontier is a byte, not
            // a rewrite.
            //
            // The measured first divergence NOW is the TEMPORAL MOTION FIELD.
            // `fh_fields.py` on frame 2 of `diag 64x64 q40 p8 frames=3` shows
            // `use_ref_frame_mvs = 1` on BOTH sides, so C's MFMV block is live
            // and so is the port's — but the port's `tpl_mvs` are all
            // `INVALID_MV` (see `inter_mvp_env` above). On the 2-frame
            // envelope that is FAITHFUL — the reference is a key frame and C's
            // own projection returns 0 for it
            // (`start_frame_buf->frame_type == KEY_FRAME`,
            // md_config_process.c:441) — and at poc 2 it is a gap: C's
            // `SVT_CINTER_OUT` codes `mode=13 NEARESTMV mv=(0,-24)` off a
            // stack whose only source is that field (0 spatial matches,
            // `imc=8` on both sides), while the port's `SVTAV1_CANDDBG`
            // reports `refmvcnt=0` and a NEARESTMV of `(0,0)`.
            //
            // THAT FIELD IS WIRED NOW (§1z²⁸) and the refusal survived it.
            // `ReferenceFrame::mvs` carries C's per-8x8 `MV_REF` grid, the
            // walk folds it through `port_coding_loop::copy_frame_mvs` under
            // C's own gate, and `inter_mvp_env.tpl_mvs` is
            // `inter_mvp::setup_motion_field`'s output rather than a constant.
            // MEASURED: the port's frame-2 `NEARESTMV` is C's `(0,-24)` off a
            // stack of 1 where it was `(0,0)` off an empty one, and SIX of
            // eight `frames=3` cells match C's frame-2 byte COUNT. None is
            // byte-identical, so what is left is the RECON — the first
            // diverging frame-header field is a CDEF search output, and on two
            // cells no header field differs at all.
            //
            // `part_arm::VideoPic`'s missing `InterOnInterRef` arm — §1z²⁵'s
            // mechanism — is still a real gap and still unported. It is a
            // candidate for that residual: it picks `pic_pd0_lvl`, the level
            // the whole partition search runs at, and the DPB already carries
            // the `sb_intra` / `sb_skip` it needs.
            //
            // See `docs/INTER-ENCODE-PLAN.md` 1z27 and 1z28.
            #[cfg(feature = "std")]
            if crate::dbgenv::refstats() {
                std::eprintln!(
                    "PORTREFBIND poc={display_order} l0cnt={} l1cnt={} dpb0={} l0slot={} \
                     l0_is_islice={:?} prf={} refresh=0x{:02x}",
                    pic.ref_list0_count_try,
                    pic.ref_list1_count_try,
                    self.dpb.occupied_slots(),
                    pic.rps.ref_dpb_index[0],
                    self.dpb
                        .get(pic.rps.ref_dpb_index[0] as usize)
                        .map(|rf| rf.is_islice),
                    binding.primary_ref_frame,
                    pic.rps.refresh_frame_mask,
                );
            }
            // THE CHAIN REFUSAL IS GONE (2026-09-11). It used to reject any
            // inter frame whose LIST-0 reference is itself an inter frame —
            // i.e. everything past frame 1 — and it was lifted only by
            // `SVTAV1_INTER_CHAIN_EXPERIMENTAL`, now deleted along with it.
            //
            // Its text said "NONE is byte-identical" and named the temporal
            // motion field as the suspect. Both halves have since been
            // settled by measurement. The field's defect was
            // `sb64_sq_no4xn_geom` being derived from `sb_size == 64` alone,
            // so C's SIMPLIFIED MFMV block walk ran on rectangular blocks and
            // used `n4_w` for both extents; with that corrected,
            // `tools/video_selfcheck_gate.sh` reports 18 of 18 real-clip cells
            // reconstructing byte-identically to `aomdec` across all 8 frames.
            // And "NONE is byte-identical" is simply no longer true: on the
            // 96-cell frontier grid at frames=4, 60 cells match C exactly on
            // frame 2 and 58 on frame 3 (MEASURED 2026-09-11).
            //
            // What is left is a parity frontier, not a correctness one, and it
            // is recorded in the README's video rows rather than as a refusal
            // — a refusal that describes a closed gap tells the next reader
            // not to look.
            let sigs = md_config_signals.ok_or_else(|| {
                whereat::at!(EncodeError::UnsupportedConfig(
                    "an inter frame's mode-decision configuration is outside this port's \
                     envelope: sig_deriv_mode_decision_config_default declined a level \
                     (crate::inter_hdr_arm::md_config_inputs) [C: accepts]",
                ))
            })?;
            // The tile above was coded from whatever `primary_ref_frame_for_cdf`
            // resolved to; the header must announce the SAME reference or the
            // decoder restores different CDFs than the encoder used.
            assert_eq!(
                binding.primary_ref_frame, primary_ref_frame_for_cdf,
                "the header's primary_ref_frame must equal the one the tile's CDFs came from",
            );
            Some(
                crate::inter_hdr_arm::inter_signal(
                    pic,
                    &sigs,
                    binding.primary_ref_frame,
                    crate::entropy::obu::ORDER_HINT_BITS,
                    crate::inter_hdr_arm::SeqInterTools {
                        enable_order_hint: seq_tools.enable_order_hint,
                        enable_ref_frame_mvs: seq_tools.enable_ref_frame_mvs,
                        enable_warped_motion: seq_tools.enable_warped_motion,
                    },
                    self.scs_tpl(),
                    // Retained as an ASSERTION input, not a refusal: the
                    // header now codes the real models. `gm_models` is `None`
                    // only when a search was needed and could not run, which
                    // `gm_search_config_error` still refuses above.
                    gm_models.as_ref().is_none_or(|m| !m.is_gm_on),
                )
                .map_err(|e| {
                    // One message per variant. These used to share one, so a
                    // GLOBAL-MOTION refusal was reported as an mfmv/TPL one —
                    // a refusal that names the wrong feature sends the next
                    // reader to the wrong file.
                    whereat::at!(EncodeError::UnsupportedConfig(match e {
                        crate::inter_hdr_arm::InterHdrError::MfmvLevelNotDerivable(_) =>
                            "an inter frame header field is not implemented for this \
                             configuration: use_ref_frame_mvs at mfmv_level >= 2 needs the TPL \
                             r0 and the references' own is_mfmv_used (crate::inter_hdr_arm::\
                             InterHdrError). This port's TPL is structurally off (aq_mode 0), \
                             so reaching this means the aq_mode refusal was lifted without \
                             porting r0 [C: accepts]",
                    }))
                })?,
            )
        };
        Ok(inter_signal)
    }

    #[inline(always)]
    pub(super) fn assemble_bitstream(
        &self,
        chroma: Option<(&[u8], &[u8])>,
        is_key: bool,
        frame_tx_mode_select: bool,
        base_qindex: u8,
        tile_rows_log2: u8,
        tile_cols_log2: u8,
        chroma_deltas: crate::chroma_q::ChromaQDeltas,
        delta_q_res_signal: Option<u8>,
        lf_sharp_eff: u8,
        qm_levels: [u8; 3],
        film_grain: &Option<crate::noise_gen::FilmGrainParams>,
        is_single_frame: bool,
        seq_tools: crate::entropy::obu::SeqTools,
        tile_data: Vec<u8>,
        tile_size_bytes_minus_1: u8,
        lf_levels: crate::deblock::LfLevels,
        cdef_params: &crate::cdef::CdefPick,
        lr_signal: crate::entropy::obu::LrSignal,
        sc_signal: crate::entropy::obu::ScSignal,
        inter_signal: Option<crate::entropy::obu::InterSignal>,
    ) -> Vec<u8> {
        let bitstream = {
            let mut bs = alloc::vec::Vec::new();
            bs.extend_from_slice(&crate::entropy::obu::write_temporal_delimiter());
            // The sequence header is written once, on the key frame — C emits
            // it on the first packet only (verified: `c.obu.pts1` of the
            // 2-frame cell is a temporal delimiter plus one OBU_FRAME).
            if is_key {
                bs.extend_from_slice(&crate::entropy::obu::write_sequence_header_ex(
                    // TRUE (unaligned) dims flow to the sequence header:
                    // max_frame_width/height_minus_1 carry the coded size, and
                    // the level derivation keys off the real picture size (C
                    // captures max_frame_width BEFORE 8-alignment,
                    // enc_handle.c:4792). Everything else in the encode uses
                    // the aligned self.width/height.
                    // Superres: the sequence header advertises the UPSCALED
                    // width (what a decoder outputs); the encode itself ran at
                    // the reduced `true_width`. Equal when superres is off.
                    self.upscaled_width,
                    self.true_height,
                    is_single_frame && !self.image_sequence,
                    self.bit_depth,
                    &self.color_description,
                    chroma.is_none(), // mono_chrome unless the 4:2:0 path is active
                    // seq_level_idx auto-derivation input (C: scs->frame_rate).
                    self.rc_config.framerate,
                    {
                        let mut t = seq_tools;
                        // The coded chroma format selects seq_profile +
                        // subsampling bits (spec 5.5.2); at Yuv420 this is
                        // profile 0, identical to every existing cell.
                        t.chroma_format = self
                            .chroma_format
                            .unwrap_or(svtav1_types::chroma::ChromaFormat::Yuv420);
                        t
                    },
                ));
            }
            // Frame header (raw bytes) + tile group with proper header.
            // base_qindex is the SAME value used for quantization, CDF
            // bucket selection and the deblock picker above — the decoder's
            // dequant/CDF init must match the encoder's exactly.
            let fh_bytes = crate::entropy::obu::write_frame_header_full_lr_sb(
                self.width,
                self.height,
                base_qindex,
                is_single_frame && !self.image_sequence,
                chroma.is_none(),
                // The levels applied to the output recon above — signaling
                // and application MUST agree or the recon desyncs from
                // every conforming decoder.
                lf_levels.levels,
                // Signaled loop_filter_sharpness — must match the value the
                // deblock search + application used (fork default 1).
                lf_sharp_eff,
                // The CDEF strengths applied to the output recon above —
                // like the deblock levels, signaling and application MUST
                // agree or the recon desyncs from every conforming decoder.
                &cdef_params.signal(),
                // lr_params: `enabled` MUST equal the SH's
                // enable_restoration bit (spec 5.9.20 gates on it — same
                // SeqTools the SH got); the per-plane types/taps are the
                // ones the tile signals and the output recon had applied.
                &lr_signal,
                sc_signal,
                // Chroma-q deltas: the quantizer above used qindex_u /
                // qindex_v built from EXACTLY these deltas, so signaling and
                // application agree (chroma_q.rs). BOTH modes derive them now
                // — the fork block unconditionally, MAINLINE only under tune
                // IQ (rc_crf_cqp.c's `#else` arm). `None` selects the
                // zero-delta bit pattern, which is what every non-tune-IQ
                // mainline encode still gets.
                //
                // Separate is checked FIRST: under a SH that signalled
                // separate_uv_delta_q = 1 the decoder reads diff_uv_delta (and,
                // with QM, qm_v) even when every delta is zero, and `None`
                // writes neither — a desync. The fork's derived deltas are
                // never all zero (U = V + 12), so this ordering is byte-inert
                // for it; an `__expert` override of (0, 0) on the fork is the
                // case that reached it (tools/chroma_q_override_gate.sh,
                // 2026-09-24: aomdec and dav1d both rejected the stream).
                if self.separate_uv_delta_q() {
                    // The fork's SH signals separate_uv_delta_q = 1, so the FH
                    // carries diff_uv_delta + four independent deltas (its U
                    // delta has a further +12, so U and V really do differ).
                    // An `__expert` override with U != V takes this form too.
                    Some(crate::entropy::obu::ChromaQSignal::Separate([
                        chroma_deltas.u_dc,
                        chroma_deltas.u_ac,
                        chroma_deltas.v_dc,
                        chroma_deltas.v_ac,
                    ]))
                } else if chroma_deltas.is_zero() {
                    None
                } else {
                    // MAINLINE: the SH signals separate_uv_delta_q = 0, so the
                    // FH must NOT write a diff_uv_delta bit — one (dc, ac)
                    // pair, reused for V. C assigns the same value to
                    // delta_q_{dc,ac}[1] and [2] (rc_crf_cqp.c:600-601), which
                    // this asserts rather than assumes.
                    debug_assert_eq!(
                        (chroma_deltas.u_dc, chroma_deltas.u_ac),
                        (chroma_deltas.v_dc, chroma_deltas.v_ac),
                        "mainline chroma-q must be plane-symmetric (SH separate_uv_delta_q = 0)"
                    );
                    Some(crate::entropy::obu::ChromaQSignal::Shared {
                        dc: chroma_deltas.u_dc,
                        ac: chroma_deltas.u_ac,
                    })
                },
                // [SVT_HDR_MODE] per-SB delta-q res (variance boost). The
                // same value gates the walk's per-SB delta symbols.
                delta_q_res_signal,
                // C hardwires `delta_lf_present = 0`
                // (resource_coordination_process.c:434-441).
                false,
                // [SVT_HDR_MODE] frame QM levels (fork enable_qm); None in
                // mainline mode. The quantizers used the SAME levels.
                if qm_levels == [15; 3] {
                    None
                } else {
                    Some(qm_levels)
                },
                film_grain.as_ref(),
                // task #86: real tile rows. tile_rows_log2 was resolved
                // (clamped) before encode_tile_rows/run_entropy_walk ran;
                // tile_size_bytes_minus_1 comes from the SAME walk that
                // produced tile_data (updated alongside every re-walk
                // reassignment above), so the FH's declared TileSizeBytes
                // always matches the tile group's actual size prefixes.
                tile_rows_log2,
                tile_cols_log2,
                tile_size_bytes_minus_1,
                // Task #91: must match the SH's use_128x128_superblock
                // (the FH's tile_info() limits are SB-derived).
                self.sb_size as u32,
                // `frm_hdr->tx_mode`, for THIS arm (`crate::txs_arm`). The
                // allintra arm signals TX_MODE_SELECT unconditionally; the
                // video arm signals it only while `pcs->txs_level != 0`,
                // which is false from preset 10 up — where this used to emit
                // a literal 1 and then code per-block tx_depth symbols that
                // TX_MODE_LARGEST forbids.
                frame_tx_mode_select,
                // `None` on a key frame -> exactly the previous bit layout.
                inter_signal.as_ref(),
            );
            // Diagnostic (SVTAV1_FHDUMP=<path>): dump the raw frame-header
            // bytes (the OBU_FRAME payload prefix before tile data — the FH
            // is byte-aligned at its end, so a prefix compare against the C
            // stream's frame OBU is exact FH byte identity). Consumed by
            // tools/screen_ibc_fh_gate.sh (IBC chunk 1).
            #[cfg(feature = "std")]
            if let Some(path) = crate::dbgenv::raw_var_os("SVTAV1_FHDUMP") {
                let _ = std::fs::write(path, &fh_bytes);
            }
            // tile_data is already a complete tile_group (with TG header)
            let mut frame_payload = alloc::vec::Vec::new();
            frame_payload.extend_from_slice(&fh_bytes);
            frame_payload.extend_from_slice(&tile_data);
            bs.extend_from_slice(&crate::entropy::obu::write_obu(
                crate::entropy::obu::ObuType::Frame,
                &frame_payload,
            ));
            bs
        };
        bitstream
    }

    #[inline(always)]
    pub(super) fn superres_upscale_stage(
        &self,
        chroma: Option<(&[u8], &[u8])>,
        fmt: svtav1_types::chroma::ChromaFormat,
        recon: &[u8],
        u_recon: &[u8],
        v_recon: &[u8],
        recon10: &Option<(Vec<u16>, Vec<u16>, Vec<u16>)>,
        decoder_output8: &Option<(Vec<u8>, Vec<u8>, Vec<u8>)>,
        out8: &mut Option<(Vec<u8>, Vec<u8>, Vec<u8>)>,
        out10: &mut Option<(Vec<u16>, Vec<u16>, Vec<u16>)>,
    ) -> Result<(), whereat::prelude::At<EncodeError>> {
        if self.superres_denom.is_some() {
            let (cw, uw, hh) = (
                self.width as usize,
                self.upscaled_width as usize,
                self.height as usize,
            );
            // Phase uses the coded plane width, while the tile rectangle
            // extends to mi_col_end. Its reconstructed padding participates
            // in the final taps (C av1_upscale_normative_rows).
            let upscale = |src: &[u8],
                           stride: usize,
                           coded: usize,
                           dst: &mut [u8],
                           out: usize,
                           rows: usize| {
                let step = svtav1_dsp::superres::upscale_convolve_step(coded as i32, out as i32);
                let x0 = svtav1_dsp::superres::upscale_convolve_x0(coded as i32, out as i32, step);
                for r in 0..rows {
                    svtav1_dsp::superres::upscale_normative_row(
                        &src[r * stride..],
                        0,
                        stride,
                        &mut dst[r * out..],
                        out,
                        step,
                        x0,
                        svtav1_dsp::superres::TileColPad::FRAME,
                    );
                }
            };
            let upscale_frame =
                |y: &[u8], u: &[u8], v: &[u8]| -> EncodeResult<(Vec<u8>, Vec<u8>, Vec<u8>)> {
                    let mut y_up = svtav1_types::try_vec![0u8; uw * hh]?;
                    upscale(y, cw, self.true_width as usize, &mut y_up, uw, hh);
                    let (mut u_up, mut v_up) = (alloc::vec::Vec::new(), alloc::vec::Vec::new());
                    if chroma.is_some() {
                        let (ccw, cuw, chh) = (
                            fmt.chroma_width(cw),
                            fmt.chroma_width(uw),
                            fmt.chroma_height(hh),
                        );
                        let coded = fmt.chroma_width(self.true_width as usize);
                        u_up = svtav1_types::try_vec![0u8; cuw * chh]?;
                        v_up = svtav1_types::try_vec![0u8; cuw * chh]?;
                        upscale(u, ccw, coded, &mut u_up, cuw, chh);
                        upscale(v, ccw, coded, &mut v_up, cuw, chh);
                    }
                    Ok((y_up, u_up, v_up))
                };
            // The decoder-equivalent output takes the replayed-filters
            // canvas when it exists, else the search recon — same preference
            // `last_recon` expresses below.
            *out8 = Some(match decoder_output8.as_ref() {
                Some((y, u, v)) => upscale_frame(y, u, v)?,
                None => upscale_frame(recon, u_recon, v_recon)?,
            });
            // The 10-bit output: C runs `av1_highbd_convolve_horiz_rs_c` on
            // the unpacked recon (the `high_bd` arm of
            // `svt_av1_upscale_normative_rows`, super_res.c:289). Same
            // normative taps and border policy on u16; `recon10` stays at
            // coded geometry for `padded_ref_hbd`/`recon_msb8` below.
            if let Some((y10, u10, v10)) = recon10.as_ref() {
                let bd = i32::from(self.bit_depth);
                let upscale_hbd = |src: &[u16],
                                   stride: usize,
                                   coded: usize,
                                   out: usize,
                                   rows: usize|
                 -> EncodeResult<Vec<u16>> {
                    let step =
                        svtav1_dsp::superres::upscale_convolve_step(coded as i32, out as i32);
                    let x0 =
                        svtav1_dsp::superres::upscale_convolve_x0(coded as i32, out as i32, step);
                    let mut dst = svtav1_types::try_vec![0u16; out * rows]?;
                    for r in 0..rows {
                        svtav1_dsp::superres::highbd_upscale_normative_row(
                            &src[r * stride..],
                            0,
                            stride,
                            &mut dst[r * out..],
                            out,
                            step,
                            x0,
                            svtav1_dsp::superres::TileColPad::FRAME,
                            bd,
                        );
                    }
                    Ok(dst)
                };
                let (ccw, cuw, chh) = (
                    fmt.chroma_width(cw),
                    fmt.chroma_width(uw),
                    fmt.chroma_height(hh),
                );
                let coded = fmt.chroma_width(self.true_width as usize);
                *out10 = Some((
                    upscale_hbd(y10, cw, self.true_width as usize, uw, hh)?,
                    upscale_hbd(u10, ccw, coded, cuw, chh)?,
                    upscale_hbd(v10, ccw, coded, cuw, chh)?,
                ));
            }
        }
        Ok(())
    }

    #[inline(always)]
    pub(super) fn build_reference_frame(
        &self,
        chroma: Option<(&[u8], &[u8])>,
        display_order: u64,
        is_key: bool,
        pic_decision: Option<crate::port_picstruct::PicParams>,
        temporal_layer: u8,
        frame_me: &Option<crate::inter_me_arm::FrameMe>,
        base_qindex: u8,
        tpl_r0: f64,
        recon: Vec<u8>,
        gm_field: [svtav1_types::motion::WarpedMotionParams; 8],
        sb_min_sq_sizes: Vec<u8>,
        sb_max_sq_sizes: Vec<u8>,
        frame_coded_area: &core::cell::RefCell<Option<CodedAreaAcc>>,
        walk_end_cdfs: core::cell::RefCell<Option<crate::port_frame_cdf::FrameCdfs>>,
        u_recon: Vec<u8>,
        v_recon: Vec<u8>,
        lf_levels: crate::deblock::LfLevels,
        dlf_dist_dev: i32,
        cdef_params: crate::cdef::CdefPick,
        cdef_dist_dev: i32,
        recon_msb8: Option<(Vec<u8>, Vec<u8>, Vec<u8>)>,
        padded_ref: alloc::boxed::Box<crate::picture::PaddedRef>,
        coded_area_pct: (u8, u8, u8),
    ) -> ReferenceFrame {
        let ref_frame = ReferenceFrame {
            padded: Some(padded_ref),
            // C `EbReferenceObject::global_motion` — what a later frame that
            // names this picture in `primary_ref_frame` delta-codes against.
            // C substitutes IDENTITY for an I_SLICE reference at READ time
            // (pic_manager_process.c:833); this stores IDENTITY for a key frame
            // at WRITE time, which is the same value with one fewer place to
            // get the slice type wrong.
            global_motion: if is_key {
                [svtav1_types::motion::WarpedMotionParams::default(); 8]
            } else {
                gm_field
            },
            // The stored planes are the same canvas `padded` pads — the
            // 10-bit recon's MSBs at bd10 (`recon_msb8`), the u8-domain
            // recon otherwise.
            y_plane: recon_msb8
                .as_ref()
                .map_or_else(|| recon.clone(), |(my, _, _)| my.clone()),
            // 4:2:0 chroma recon, empty on the monochrome path. Inter
            // prediction needs all three planes; see `ReferenceFrame::u_plane`.
            u_plane: if chroma.is_some() {
                recon_msb8
                    .as_ref()
                    .map_or_else(|| u_recon.clone(), |(_, mu, _)| mu.clone())
            } else {
                alloc::vec::Vec::new()
            },
            v_plane: if chroma.is_some() {
                recon_msb8
                    .as_ref()
                    .map_or_else(|| v_recon.clone(), |(_, _, mv)| mv.clone())
            } else {
                alloc::vec::Vec::new()
            },
            // C `rest_process.c:207-210`: the strengths the FRAME HEADER
            // signalled, not the ones the search proposed. A later frame's
            // CDEF candidate set is rewritten from these
            // (`update_cdef_filters_on_ref_info`).
            sb_min_sq_size: sb_min_sq_sizes,
            sb_max_sq_size: sb_max_sq_sizes,
            // C `copy_statistics_to_ref_obj_ect` (rest_process.c:190-220):
            // the NORMALISED percentages (`:347-349`) and the per-superblock
            // flags this picture's walk accumulated. All zero / empty on
            // every allintra cell, where the walk never armed the
            // accumulator — and C's own readers return 0 / 0 / -1 for an
            // I_SLICE reference regardless of what it stored.
            intra_coded_area: coded_area_pct.0,
            skip_coded_area: coded_area_pct.1,
            hp_coded_area: coded_area_pct.2,
            sb_intra: frame_coded_area
                .borrow()
                .as_ref()
                .map(|a| a.sb_intra.clone())
                .unwrap_or_default(),
            sb_skip: frame_coded_area
                .borrow()
                .as_ref()
                .map(|a| a.sb_skip.clone())
                .unwrap_or_default(),
            // C `copy_statistics_to_ref_obj_ect` (rest_process.c:190-220)
            // also stores this picture's open-loop ME per-SB distortions, so a
            // later frame's `lpd1_detector_skip_pd0` can compare its own
            // `me_64x64_distortion`/`me_8x8_cost_variance` against them.
            // Empty on a key frame, where `frame_me` never ran.
            sb_me_64x64_dist: frame_me
                .as_ref()
                .map(|m| m.per_b64.iter().map(|b| b.me_64x64_distortion).collect())
                .unwrap_or_default(),
            sb_me_8x8_cost_var: frame_me
                .as_ref()
                .map(|m| m.per_b64.iter().map(|b| b.me_8x8_cost_variance).collect())
                .unwrap_or_default(),
            // C `rest_process.c:216` copies `pcs->sb_64x64_mvp` to the ref —
            // the same `update_b`-accumulated array as `sb_intra`/`sb_skip`.
            sb_64x64_mvp: frame_coded_area
                .borrow()
                .as_ref()
                .map(|a| a.sb_64x64_mvp.clone())
                .unwrap_or_default(),
            // C `EbReferenceObject::slice_type`.
            is_islice: is_key,
            // C `enc_dec_process.c:1248-1252` — the ref object takes the
            // signalled `base_q_idx` and `ppcs->r0` (post `crf_qindex_calc`
            // adjustment; 0 when TPL did not run this frame) so a LATER
            // frame's `ref_base_q_idx`/`ref_pic_r0` reads them back.
            base_q_idx: base_qindex,
            r0: tpl_r0,
            cdef_y_strengths: cdef_params.strengths.iter().map(|s| s.0).collect(),
            cdef_uv_strengths: cdef_params.strengths.iter().map(|s| s.1).collect(),
            // C `packetization_process.c:741-744`: reset the CDF symbol
            // counters, THEN copy into the reference object. The reset is not
            // cosmetic — `update_cdf` reads `cdf[nsymbs]` to choose the
            // adaptation RATE, so a saved state that kept a frame's final
            // counts would make the next frame adapt at the slow late-frame
            // rate from its first symbol.
            frame_cdfs: walk_end_cdfs.borrow_mut().take().map(|mut c| {
                c.reset_symbol_counters();
                #[cfg(feature = "std")]
                if let Some(path) = crate::dbgenv::raw_var_os("SVTAV1_FCTX_OUT") {
                    // Same format and field order as the C oracle's
                    // `__wrap_svt_av1_reset_cdf_symbol_counters`
                    // (tools/capture_c_trace/wrap_recon.c), so
                    // tools/fctx_diff.py can compare them directly.
                    c.dump_to(&path, display_order as u32);
                }
                alloc::sync::Arc::new(c)
            }),
            // C `rest_process.c:200-204`: the loop-filter levels this frame's
            // HEADER signalled, plus the SSE-improvement measure. The next
            // frame's deblock level is derived from BOTH — see
            // `crate::dlf_arm`.
            lf_levels: lf_levels.levels,
            dlf_dist_dev,
            // C `rest_process.c:205`: `obj->cdef_dist_dev =
            // pcs->cdef_dist_dev` — the search-path measurement, -1 on the
            // fast paths, 0 whenever the signalled strengths are all zero.
            cdef_dist_dev,
            width: self.width,
            height: self.height,
            display_order,
            order_hint: display_order as u32,
            // C `av1_copy_frame_mvs`'s output, which `update_b` wrote into
            // this picture's own reference object during the walk. EMPTY on a
            // key frame and on every allintra cell, exactly where C's gate is
            // false — see `CodedAreaAcc::mfmv_active`.
            mvs: frame_coded_area
                .borrow()
                .as_ref()
                .map(|a| a.mvs.clone())
                .unwrap_or_default(),
            // C `EbReferenceObject::ref_order_hint[0..7]`
            // (`rest_process.c` / `pad_ref_and_set_flags`): the order hints of
            // THIS picture's own references, which a LATER picture's
            // `motion_field_projection` reads to scale a saved MV. All zero on
            // a key frame, which has none.
            ref_order_hint: {
                let mut a = [0i32; 7];
                if let Some(pic) = pic_decision.as_ref() {
                    for (i, oh) in a.iter_mut().enumerate() {
                        let slot = pic.rps.ref_dpb_index[i] as usize;
                        *oh = self.dpb.get(slot).map_or(0, |r| r.order_hint as i32);
                    }
                }
                a
            },
            // C `EbReferenceObject::tmp_layer_idx` — this picture's own
            // `temporal_layer_index` (enc_dec_process.c:2149's reader
            // compares a LATER frame's layer against it). 0 on every flat
            // low-delay frame.
            temporal_layer,
        };
        ref_frame
    }
}
