use super::*;

/// Body of [`write_key_frame_header_full_lr`] (see the compat wrapper above).
///
/// `inter` is `None` for a key frame and `Some(..)` for an INTER frame; see
/// [`InterSignal`].
#[allow(clippy::too_many_arguments)]
pub(super) fn frame_header_bits_lr(
    width: u32,
    height: u32,
    base_qindex: u8,
    reduced_sh: bool,
    monochrome: bool,
    lf_levels: [u8; 4],
    lf_sharpness: u8,
    cdef: &CdefSignal,
    lr: &LrSignal,
    sc: ScSignal,
    chroma_q: Option<ChromaQSignal>,
    delta_q_res: Option<u8>,
    // `frm_hdr->delta_lf_params.delta_lf_present` — only meaningful with
    // `delta_q_res: Some` (the syntax nests inside delta_q_params).
    delta_lf: bool,
    qm: Option<[u8; 3]>,
    fgs: Option<&FilmGrainParams>,
    tile_rows_log2: u8,
    tile_cols_log2: u8,
    tile_size_bytes_minus_1: u8,
    // Superblock size in PIXELS (64 or 128) — the tile limits are
    // SB-derived (spec 5.9.15), so this must match the SH's
    // `use_128x128_superblock`.
    sb_size: u32,
    // `frm_hdr->tx_mode == TX_MODE_SELECT` (`crate::txs_arm::tx_mode_select`).
    // ALWAYS true on the allintra arm, which is why this used to be a literal.
    tx_mode_select: bool,
    // `None` -> KEY_FRAME, the historical layout. `Some(..)` -> INTER_FRAME.
    inter: Option<&InterSignal>,
) -> BitWriter {
    let mut wb = BitWriter::new();

    if !reduced_sh {
        // ---- Full frame header preamble ----
        wb.write_bit(false); // show_existing_frame = 0
        // frame_type: KEY_FRAME (0) or INTER_FRAME (1).
        wb.write_bits(u32::from(inter.is_some()), 2);
        wb.write_bit(inter.map_or(true, |it| it.show_frame)); // show_frame
        if let Some(it) = inter {
            // showable_frame: read iff show_frame = 0 (C
            // entropy_coding.c:3344-3347). Every hidden picture this port
            // codes is shown later through show_existing_frame, so 1.
            if !it.show_frame {
                wb.write_bit(true);
            }
            // error_resilient_mode: implicit 1 for KEY_FRAME with
            // show_frame = 1 (and for SWITCH_FRAME); READ for every other
            // frame type. C's low-delay CQP inter frames carry 0, which is
            // what makes `primary_ref_frame` present a few fields down.
            wb.write_bit(it.error_resilient_mode);
        }
    }
    // For reduced SH: show_existing_frame/frame_type/show_frame/error_resilient
    // are all implicit.

    // disable_cdf_update is ALWAYS signaled (spec 5.9.2 reads it outside the
    // reduced_still_picture_header branch; C writes it unconditionally at
    // entropy_coding.c:3373).
    wb.write_bit(false); // disable_cdf_update = 0

    // allow_screen_content_tools: seq_force = SELECT → read 1 bit
    // (C entropy_coding.c:3345-3348; value from sig_deriv_multi_processes
    // _allintra :2393 = palette_level || allow_intrabc).
    wb.write_bit(sc.allow_screen_content_tools);
    if sc.allow_screen_content_tools {
        // seq_force_integer_mv = SELECT (2, sequence_control_set.c:101) →
        // the frame force_integer_mv bit follows; C keeps
        // frm_hdr->force_integer_mv = 0 unconditionally
        // (resource_coordination_process.c:362, never reassigned).
        wb.write_bit(false); // force_integer_mv = 0
    }

    if !reduced_sh {
        wb.write_bit(false); // frame_size_override_flag = 0
        wb.write_bits(
            u32::from(inter.map_or(0, |it| it.order_hint)),
            ORDER_HINT_BITS,
        ); // order_hint (0 for a key frame)
        if let Some(it) = inter {
            // primary_ref_frame (spec 5.9.2): read when
            // `!FrameIsIntra && !error_resilient_mode`. It names the reference
            // whose END-OF-FRAME CDFs this frame starts from, so getting it
            // wrong desynchronises the entropy decoder at the first symbol —
            // it is a hard bitstream field, not a hint.
            if !it.error_resilient_mode {
                wb.write_bits(u32::from(it.primary_ref_frame), 3);
            }
            // refresh_frame_flags: implicit `allFrames` ONLY for SWITCH_FRAME
            // and a SHOWN key frame; written for every inter frame.
            wb.write_bits(u32::from(it.refresh_frame_flags), 8);
            // The `ref_order_hint[NUM_REF_FRAMES]` map that follows is read
            // only when `error_resilient_mode && enable_order_hint`. This port
            // does not produce error-resilient inter frames, and emitting the
            // map wrong would shift every following field, so the branch is an
            // assertion rather than a guess.
            debug_assert!(
                !it.error_resilient_mode,
                "error-resilient inter frames need the ref_order_hint[8] map \
                 (spec 5.9.2); this writer does not emit it"
            );
        }
        // primary_ref_frame: NOT signaled for KEY_FRAME with error_resilient=1
        //   (implicit PRIMARY_REF_NONE)
        //
        // refresh_frame_flags: NOT signaled either. Spec 5.9.2 and C
        // (entropy_coding.c:3404-3407, `if (!show_frame)`) leave it IMPLICIT
        // at allFrames for a SWITCH frame or a SHOWN key frame — and this
        // writer only emits shown key frames. Writing the 8 bits put an
        // illegal field in the stream and shifted every following one, which
        // is the third of the four defects the inter refusal in pipeline.rs
        // names. MEASURED: it was the first divergence in the video-mode
        // frame OBU, at bit 14, immediately after order_hint.
    }

    if let Some(it) = inter {
        // ---- the INTER arm of spec 5.9.2 ----
        // Order is load-bearing and differs from the intra arm: the reference
        // indices come BEFORE frame_size()/render_size(), not after.
        //
        // frame_refs_short_signaling: read when the SH has enable_order_hint
        // (it does — the key frame this port already emits byte-identically
        // signals it). C never sets it (`entropy_coding.c:3492-3497` writes
        // `pcs->frame_refs_short_signaling`, left 0), so all seven indices are
        // written explicitly.
        wb.write_bit(false); // frame_refs_short_signaling = 0
        for idx in it.ref_frame_idx {
            debug_assert!(idx < 8, "ref_frame_idx is a DPB slot, 0..=7");
            wb.write_bits(u32::from(idx), 3);
        }
        // frame_id_numbers_present_flag = 0 in the SH -> no delta_frame_id.
        // frame_size_override_flag = 0 -> frame_size(); render_size().
        sc.superres.write(&mut wb);
        wb.write_bit(false); // render_and_frame_size_different = 0
        // force_integer_mv is 0 here: it is only written when
        // allow_screen_content_tools is set, and the writer above emits a
        // literal 0 for it in that case.
        wb.write_bit(it.allow_high_precision_mv);
        // read_interpolation_filter()
        match it.interpolation_filter {
            None => wb.write_bit(true), // is_filter_switchable = 1
            Some(f) => {
                debug_assert!(f < 4, "interpolation_filter is 2 bits");
                wb.write_bit(false);
                wb.write_bits(u32::from(f), 2);
            }
        }
        wb.write_bit(it.is_motion_mode_switchable);
        // use_ref_frame_mvs: no bit when error_resilient_mode ||
        // !enable_ref_frame_mvs.
        if let Some(v) = it.use_ref_frame_mvs {
            wb.write_bit(v);
        }
        // allow_intrabc is an intra-frame field only.
    } else {
        // ---- frame_size() ----
        // frame_size_override_flag = 0 → use SH dimensions, no bits.
        // superres_params() (spec 5.9.8): one `use_superres` bit when the SH
        // has the tool on, plus 3 bits of `coded_denom` when it is set.
        // Nothing is written when `enable_superres` is off (every current gate
        // cell), so this is bit-for-bit the previous behaviour there.
        sc.superres.write(&mut wb);

        // ---- render_size() ----
        wb.write_bit(false); // render_and_frame_size_different = 0

        // allow_intrabc: signaled iff allow_screen_content_tools AND the frame
        // is NOT superres-scaled (spec 5.9.11 `UpscaledWidth == FrameWidth`; C
        // entropy_coding.c:3464-3466, after write_frame_size). IntraBC cannot
        // be combined with superres because the block copy would read the
        // pre-upscale grid.
        if sc.allow_screen_content_tools && sc.superres.denom.is_none() {
            wb.write_bit(sc.allow_intrabc);
        }
    }

    // ---- disable_frame_end_update_cdf (spec 5.9.2) ----
    // C's `might_bwd_adapt` (entropy_coding.c:3553-3559):
    //     !reduced_still_picture_header && !disable_cdf_update
    // and only then is the bit written, as
    // `refresh_frame_context == REFRESH_FRAME_CONTEXT_DISABLED`. The picture
    // manager assigns REFRESH_FRAME_CONTEXT_BACKWARD to coded pictures
    // (pic_manager_process.c:868,875), so the bit is 0 on this path.
    //
    // In the reduced (still) header the field is IMPLICIT 1 and no bit is
    // written, which is why every still cell was already byte-identical
    // without it. Omitting it from the video header shifted tile_info() and
    // every following field by one bit — the fourth of the four defects the
    // inter refusal in pipeline.rs names.
    //
    // `disable_cdf_update` is written as 0 above, so the guard reduces to
    // `!reduced_sh` today; it is spelled out anyway so that making
    // disable_cdf_update dynamic cannot silently desync the header.
    let disable_cdf_update = false;
    if !reduced_sh && !disable_cdf_update {
        wb.write_bit(false); // refresh_frame_context != DISABLED
    }

    // ---- tile_info() ----
    write_tile_info(
        &mut wb,
        width,
        height,
        tile_rows_log2,
        tile_cols_log2,
        tile_size_bytes_minus_1,
        sb_size,
    );

    // ---- quantization_params() ----
    // Spec 5.9.12; decoder authority: libaom setup_quantization
    // (av1/decoder/decodeframe.c:1818-1841).
    wb.write_bits(base_qindex as u32, 8); // base_q_idx
    wb.write_bit(false); // DeltaQYDc: delta_coded = 0
    if !monochrome {
        // NumPlanes=3. The SH signaled separate_uv_delta_q=0, so the
        // decoder does NOT read diff_uv_delta (it stays 0):
        //     if (separate_uv_delta_q) diff_uv_delta = aom_rb_read_bit(rb);
        // It then reads DeltaQUDc and DeltaQUAc, and because
        // diff_uv_delta==0 the V plane reuses the U deltas — no V bits.
        // (libaom decodeframe.c:1823-1834; C write path writes the same
        // two zero delta_coded bits for u_dc/u_ac.)
        // With SH separate_uv_delta_q=1 the decoder first reads
        // diff_uv_delta, then U and (if diff) V deltas. delta_q syntax
        // (spec 5.9.13 read_delta_q): 1-bit delta_coded, then su(1+6)
        // = 7-bit two's-complement inv_signed_literal.
        let write_delta = |wb: &mut BitWriter, d: i8| {
            if d == 0 {
                wb.write_bit(false); // delta_coded = 0
            } else {
                wb.write_bit(true); // delta_coded = 1
                wb.write_bits((d as i32 & 0x7f) as u32, 7); // su(1+6)
            }
        };
        match chroma_q {
            None => {
                wb.write_bit(false); // DeltaQUDc: delta_coded = 0
                wb.write_bit(false); // DeltaQUAc: delta_coded = 0
            }
            // SH separate_uv_delta_q = 0: NO diff_uv_delta bit (the decoder
            // does not read one), U deltas only, V reuses them.
            Some(ChromaQSignal::Shared { dc, ac }) => {
                write_delta(&mut wb, dc);
                write_delta(&mut wb, ac);
            }
            // SH separate_uv_delta_q = 1 (fork): diff_uv_delta then all four.
            Some(ChromaQSignal::Separate([u_dc, u_ac, v_dc, v_ac])) => {
                wb.write_bit(true); // diff_uv_delta
                for d in [u_dc, u_ac, v_dc, v_ac] {
                    write_delta(&mut wb, d);
                }
            }
        }
    }
    // NumPlanes=1 (mono_chrome): no DeltaQUDc, DeltaQUAc
    // [SVT_HDR_MODE] quantizer matrices (spec 5.9.12; C entropy_coding.c:
    // 2451): qm_y + qm_u always, qm_v only when the SH signaled
    // separate_uv_delta_q=1 (the fork chroma-q path — chroma_q Some).
    match qm {
        None => wb.write_bit(false), // using_qmatrix = 0
        Some([qm_y, qm_u, qm_v]) => {
            wb.write_bit(true); // using_qmatrix = 1
            wb.write_bits(u32::from(qm_y), 4);
            wb.write_bits(u32::from(qm_u), 4);
            // qm_v is written ONLY when the SH signalled
            // separate_uv_delta_q = 1 — the same bit that gates
            // diff_uv_delta, so it follows the SEPARATE variant, not merely
            // "chroma deltas exist". Keying it on `is_some()` would emit a
            // stray 4-bit field on the mainline Shared path.
            if matches!(chroma_q, Some(ChromaQSignal::Separate(_))) {
                wb.write_bits(u32::from(qm_v), 4);
            } else {
                debug_assert_eq!(qm_u, qm_v, "qm_v needs separate_uv_delta_q");
            }
        }
    }

    // ---- segmentation_params() ----
    wb.write_bit(false); // segmentation_enabled = 0

    // ---- delta_q_params() ----
    // Spec: delta_q_present is only signaled when base_q_idx > 0.
    // [SVT_HDR_MODE] fork variance boost signals per-SB delta-q:
    // delta_q_present=1 + 2-bit log2(delta_q_res) (spec 5.9.17).
    if base_qindex > 0 {
        match delta_q_res {
            None => wb.write_bit(false), // delta_q_present = 0
            Some(res) => {
                debug_assert!(matches!(res, 1 | 2 | 4 | 8));
                wb.write_bit(true); // delta_q_present = 1
                wb.write_bits(res.trailing_zeros(), 2); // delta_q_res log2
                // delta_lf_params(): read only when delta_q_present &&
                // !allow_intrabc (spec 5.9.18). aom `--delta-lf-mode`
                // signals present + delta_lf_res log2 (2 -> 1) +
                // delta_lf_multi (0); C writes the same layout with
                // present hardwired 0 (entropy_coding.c:3578-3588).
                if !sc.allow_intrabc {
                    wb.write_bit(delta_lf); // delta_lf_present
                    if delta_lf {
                        wb.write_bits(1, 2); // delta_lf_res = 2 -> log2 = 1
                        wb.write_bit(false); // delta_lf_multi = 0
                    }
                }
            }
        }
    }
    // delta_lf_params(): not signaled when delta_q_present=0

    // ---- CodedLossless / AllLossless (spec 5.9.2 tail of
    // read_quantization_params / segmentation: `LosslessArray[seg] =
    // qindex == 0 && DeltaQYDc == 0 && DeltaQUAc == 0 && DeltaQUDc == 0 &&
    // DeltaQVAc == 0 && DeltaQVDc == 0`, CodedLossless = all segments;
    // AllLossless = CodedLossless && FrameWidth == UpscaledWidth). Segmentation
    // is off (above), so every segment carries base_q_idx; DeltaQYDc is
    // always 0 here and the chroma deltas are `chroma_q`. C:
    // md_config_process.c:992-1017 (`coded_lossless = lossless[0] =
    // !base_q_idx` when segmentation is off) and entropy_coding.c:3594-3612.
    // Issue #5 chunk 1: the header half of the coded-lossless envelope —
    // pipeline.rs still refuses base_qindex 0 until the tile half (TX_4X4
    // WHT, no tx_size / tx_type symbols) is ported and byte-verified.
    let coded_lossless = base_qindex == 0 && chroma_q.is_none_or(|d| d.is_zero());
    let all_lossless = coded_lossless && sc.superres.denom.is_none();

    // ---- loop_filter_params() ----
    // When CodedLossless or allow_intrabc: NO loop filter bits (spec 5.9.11
    // sets the levels to 0; C entropy_coding.c:3597 skips encode_loopfilter).
    // Field set matches C encode_loopfilter (entropy_coding.c:2338) and
    // spec 5.9.11; libaom setup_loopfilter (decodeframe.c:1766) reads it.
    if !sc.allow_intrabc && !coded_lossless {
        wb.write_bits(lf_levels[0] as u32, 6); // loop_filter_level[0]
        wb.write_bits(lf_levels[1] as u32, 6); // loop_filter_level[1]
        // NumPlanes=1: no loop_filter_level[2]/[3].
        // NumPlanes=3: levels [2] (U) and [3] (V) are only coded when
        // (loop_filter_level[0] || loop_filter_level[1]).
        if !monochrome && (lf_levels[0] != 0 || lf_levels[1] != 0) {
            wb.write_bits(lf_levels[2] as u32, 6); // loop_filter_level[2] (U)
            wb.write_bits(lf_levels[3] as u32, 6); // loop_filter_level[3] (V)
        }
        wb.write_bits(u32::from(lf_sharpness), 3); // loop_filter_sharpness (fork default 1)
        // loop_filter_delta_enabled = 0: the C encoder runs with
        // mode_ref_delta_enabled = 0 (resource_coordination_process.c:393) and
        // encode_loopfilter writes the flag verbatim, so no ref/mode deltas are
        // signaled or applied — the filter level is uniform per plane/direction.
        wb.write_bit(false); // loop_filter_delta_enabled = 0
    }

    // ---- cdef_params() ----
    // Spec 5.9.19; C write path encode_cdef (entropy_coding.c:2398), read
    // path libaom setup_cdef (decodeframe.c:1799). Present because the SH
    // signals enable_cdef=1 and this header is neither CodedLossless nor
    // allow_intrabc — either one means NO cdef bits (spec 5.9.19 early-out;
    // C entropy_coding.c:3598-3600 under `!coded_lossless`).
    if !sc.allow_intrabc && !coded_lossless {
        // `damping == 0` is C's "the pick never ran" state, NOT an
        // out-of-range value — see `cdef::CdefFrameParams::never_picked`.
        debug_assert!(
            (3..=6).contains(&cdef.damping) || cdef.damping == 0,
            "cdef_damping out of range"
        );
        debug_assert_eq!(cdef.strengths.len(), 1usize << cdef.bits);
        // C `svt_aom_wb_write_literal(wb, cdef_damping - 3, 2)`
        // (entropy_coding.c:2349) on a `uint8_t` promoted to `int`: at
        // damping 0 that is `-3`, whose low two bits are 1. The wrapping
        // subtraction reproduces it instead of underflowing in debug and
        // silently differing in release.
        wb.write_bits(u32::from(cdef.damping.wrapping_sub(3)) & 3, 2); // cdef_damping_minus_3
        wb.write_bits(cdef.bits as u32, 2); // cdef_bits -> (1 << bits) strength sets
        for &(y, uv) in &cdef.strengths {
            wb.write_bits(y as u32, 6); // cdef_y_pri(4) + cdef_y_sec(2) packed
            if !monochrome {
                // NumPlanes=3 only: libaom reads uv strengths iff num_planes > 1
                // (C SVT always writes both — it cannot emit monochrome).
                wb.write_bits(uv as u32, 6); // cdef_uv_pri(4) + cdef_uv_sec(2)
            }
        }
    }

    // ---- lr_params() ----
    // Spec 5.9.20; C writer: encode_restoration_mode
    // (entropy_coding.c:2243-2307), whose call is gated on
    // seq_header.enable_restoration at entropy_coding.c:3652-3653 (the
    // spec folds the gate into lr_params' AllLossless/allow_intrabc/
    // enable_restoration early-out; base_q_idx > 0 and
    // allow_intrabc = 0 are the same standing assumptions as
    // cdef_params above). Per-plane bit pairs (entropy_coding.c:2263-2282;
    // read as lr_type f(2) with Remap_Lr_Type): NONE (0,0), WIENER (1,0),
    // SGRPROJ (1,1), SWITCHABLE (0,1). When any plane restores, the luma
    // unit-size bits follow (sb 64: bit(size>64), then bit(size>128));
    // when a CHROMA plane restores, the lr_uv_shift bit follows.
    // NumPlanes = 1 for mono, 3 for 4:2:0 (C is always 3-plane; the
    // decoder reads NumPlanes lr_types — libaom decode_restoration_mode,
    // decodeframe.c).
    // When allow_intrabc: NO lr bits (spec 5.9.20 folds allow_intrabc into
    // the same early-out as !enable_restoration).
    // AllLossless also folds in (spec 5.9.20: `if AllLossless || allow_intrabc
    // || !enable_restoration` -> no lr bits; C entropy_coding.c:3594 codes
    // restoration only in the `!all_lossless` arm).
    if lr.enabled && !sc.allow_intrabc && !all_lossless {
        let num_planes = if monochrome { 1 } else { 3 };
        let mut all_none = true;
        let mut chroma_none = true;
        for p in 0..num_planes {
            let t = lr.frame_types[p];
            if t != 0 {
                all_none = false;
                if p > 0 {
                    chroma_none = false;
                }
            }
            let (b0, b1) = match t {
                0 => (false, false), // RESTORE_NONE
                1 => (true, false),  // RESTORE_WIENER
                2 => (true, true),   // RESTORE_SGRPROJ
                _ => (false, true),  // RESTORE_SWITCHABLE
            };
            wb.write_bit(b0);
            wb.write_bit(b1);
        }
        if !all_none {
            // C encode_restoration_mode (entropy_coding.c:2225-2236):
            //
            //     if (sb_size == 64) wb_write_bit(unit_size > 64);
            //     if (unit_size > 64) wb_write_bit(unit_size > 128);
            //
            // The FIRST bit is written ONLY at SB64 (task #91). At SB128
            // the spec's RESTORATION_UNITSIZE_MAX-relative encoding starts
            // one step up, because a restoration unit may never be smaller
            // than the superblock (C asserts `unit_size >= sb_size`), so
            // `unit_size > 64` is not a free choice — it is implied. Writing
            // it anyway shifts every following header bit by one and
            // silently corrupts tx_mode_select / reduced_tx_set (MEASURED:
            // exactly the `diag 512x384 q55 p0` and `gradient 512x384 q55
            // p0` divergences, whose tile payloads were already
            // byte-identical).
            debug_assert!(u32::from(lr.unit_size) >= sb_size);
            if sb_size == 64 {
                wb.write_bit(lr.unit_size > 64);
            }
            if lr.unit_size > 64 {
                wb.write_bit(lr.unit_size > 128);
            }
        }
        if !chroma_none {
            wb.write_bit(lr.uv_size_differs);
        }
    }

    // ---- read_tx_mode() ----
    // CodedLossless -> TxMode = ONLY_4X4 and NO bit (spec 5.9.21; C
    // entropy_coding.c:3608-3612). Otherwise TX_MODE_SELECT, like C: SVT
    // always sets frm_hdr->tx_mode = TX_MODE_SELECT at these presets ("Use
    // TX_MODE_SELECT even when txs_level == 0", enc_mode_config.c:15140-15143;
    // written at entropy_coding.c:3659). The tile walk then codes a per-block
    // tx_depth symbol for every bsize > 4x4 (always depth 0 = largest —
    // matching what the LARGEST mode implied, but now in C's syntax).
    if !coded_lossless {
        // The ALLINTRA arm sets TX_MODE_SELECT unconditionally ("Use
        // TX_MODE_SELECT even when txs_level == 0, as the decision may change
        // from OFF to Fastest at the SB level", enc_mode_config.c:10025). The
        // VIDEO arm sets it only when `pcs->txs_level != 0` (`:9194`), i.e.
        // TX_MODE_LARGEST at preset 10 and up — where this writer used to emit
        // a literal 1 and then code per-block tx_depth symbols that
        // TX_MODE_LARGEST forbids.
        wb.write_bit(tx_mode_select);
    }

    // ---- frame_reference_mode() + skip_mode_params() + allow_warped_motion ----
    // Intra frames write none of these (spec 5.9.2 gates each on
    // `!FrameIsIntra`); C: entropy_coding.c:3615-3629.
    if let Some(it) = inter {
        wb.write_bit(it.reference_select);
        // skip_mode_present: no bit when `skipModeAllowed` is 0.
        if let Some(v) = it.skip_mode_present {
            wb.write_bit(v);
        }
        // allow_warped_motion: no bit when error_resilient_mode ||
        // !enable_warped_motion.
        if let Some(v) = it.allow_warped_motion {
            wb.write_bit(v);
        }
    }

    wb.write_bit(false); // reduced_tx_set = 0

    // ---- global_motion_params() (spec 5.9.24) ----
    // Intra frames write nothing. For an inter frame, one `is_global` bit per
    // reference, and a set bit is followed by the type and up to six
    // parameters, each delta-coded against the primary reference picture's own
    // model. This used to write seven zero bits under a `debug_assert` that
    // the frame had no model, with the pipeline refusing any frame where C's
    // search found one; `port_entropy_inter::gm::write_global_motion` is that
    // coding, and `tools/global_motion_gate.sh` pins the result against dav1d.
    if let Some(it) = inter {
        crate::port_entropy_inter::gm::write_global_motion(
            &mut wb,
            &it.global_motion,
            &it.ref_global_motion,
            // C `write_global_motion` reads `pcs->ppcs->frm_hdr.primary_ref_frame`
            // to choose between the reference models and IDENTITY.
            it.primary_ref_frame,
            it.allow_high_precision_mv,
        );
    }

    // ---- film_grain_params() (spec 5.9.30) ---- read only when the SH
    // signaled film_grain_params_present && show_frame (C
    // entropy_coding.c:3698). The SH bit and this Option MUST agree.
    if let Some(fg) = fgs {
        write_film_grain_params(&mut wb, fg, inter);
    }

    // NOTE: byte_alignment() is applied by the caller
    // (write_key_frame_header_full) so tests can observe the raw bit count.
    wb
}
