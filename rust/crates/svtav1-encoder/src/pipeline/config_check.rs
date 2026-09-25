use super::*;

impl EncodePipeline {
    /// Whether the configured [`Self::chroma_format`] is consumable by
    /// this build. The staged-444 contract: `Some(Yuv420)` is the only
    /// C-parity surface; every other format refuses HERE, at the entry
    /// choke point, until its path is proven decoder-exact — C refuses
    /// all of them anyway (`verify_settings`, enc_settings.c:470), so a
    /// refusal is both the honest answer and the C-matching one.
    pub(super) fn chroma_format_support_error(&self) -> Option<&'static str> {
        match self.chroma_format {
            None | Some(svtav1_types::chroma::ChromaFormat::Yuv420) => None,
            Some(svtav1_types::chroma::ChromaFormat::Yuv400) => Some(
                "ChromaFormat::Yuv400 is the monochrome extension — use the mono \
                 entry points (try_encode_frame / encode_y8), which carry no \
                 chroma planes",
            ),
            // 4:4:4 is ungated at the entry — `encode_frame_impl` carries the
            // real envelope gate (8-bit key/still, SB64, no superres) with the
            // refusal text naming each unported piece.
            Some(svtav1_types::chroma::ChromaFormat::Yuv444) => None,
            Some(f) => Some(match f {
                svtav1_types::chroma::ChromaFormat::Yuv422 => {
                    "ChromaFormat::Yuv422 is not yet ported: chroma geometry is \
                     still derived for 4:2:0 (C itself refuses it at \
                     verify_settings, enc_settings.c:470 — no byte oracle)"
                }
                _ => unreachable!(),
            }),
        }
    }

    /// Does this configuration actually CONSUME a native 10-bit source?
    ///
    /// The full-RD color funnel and the level re-encode post-pass consume
    /// real u16 samples, including partial superblocks. Monochrome has only
    /// the latter consumer at every preset. Reject configurations without a
    /// consumer so the caller's low bits cannot be silently discarded.
    pub(super) fn hbd_source_consumed(&self, chroma_420: bool) -> bool {
        self.bd10_levels_native(chroma_420)
    }

    /// FH `frm_hdr->tx_mode == TX_MODE_SELECT` for this frame.
    ///
    /// ONE source for the header writer and the pack walk: the signalled mode
    /// and the coded symbols must agree or the stream does not decode (see
    /// `EntropyCtx::tx_mode_select`). `crate::txs_arm::tx_mode_select` is the
    /// ladder; the allintra arm is unconditional TX_MODE_SELECT, the video arm
    /// signals it only while `pcs->txs_level != 0`.
    pub(super) fn frame_tx_mode_select(&self, temporal_layer: u8) -> bool {
        let arm = if self.gop.intra_period == 1 {
            crate::sc_detect::ScArm::Allintra
        } else {
            crate::sc_detect::ScArm::Video { is_islice: true }
        };
        crate::txs_arm::tx_mode_select(
            arm,
            crate::rate_arm::eff_enc_mode(arm, self.speed_config.preset),
            // enc_mode_config.c:9177-9185: the video ladder's `is_base` is
            // `temporal_layer_index == 0`, NOT `frame_is_boosted` — a
            // non-base layer drops `txs_level` to 0 and the header signals
            // TX_MODE_LARGEST.
            temporal_layer == 0,
            u32::from(self.rc_config.qp),
        )
    }

    /// Whether the full-RD color funnel or native level re-encode post-pass
    /// produces 10-bit coded levels. Shared by native-input and bit-depth guards.
    pub(super) fn bd10_levels_native(&self, chroma_420: bool) -> bool {
        if self.bit_depth != 10 {
            return false;
        }
        let (w, h) = (self.width as usize, self.height as usize);
        // Monochrome builds no funnel (`use_funnel` requires 4:2:0). The
        // native level pass carries real coefficient contexts and the coded
        // parent partition's directional availability at every preset. The
        // sequence header disables mono edge filtering. Mode decision still
        // uses the upper eight bits; coded levels use all ten.
        let preset = self.speed_config.preset;
        // NO GEOMETRY TERM (2026-08-04). Both bd10 level producers are now
        // partial-SB aware: the full-RD funnel (preset <= 8) rides the shared,
        // already-correct partition search and leaf funnel, and the level-only
        // re-encode post-pass (preset >= 9) got SB-extent recon buffers,
        // straddle-clipped writes, SB-extent-padded sources, and the pack's
        // skip-off-frame-quadrant child walk. Both are gated per-CELL below
        // rather than by dimension.
        if !chroma_420 {
            return true;
        }
        // A capability QUERY, not a per-frame decision: it answers "can this
        // configuration serve 10-bit at all", so it asks the I-slice arm --
        // which is the one an all-intra caller takes on every frame.
        preset >= 9 || bd10_full_rd_supported(false, self.bit_depth, preset, chroma_420, true, w, h)
    }

    /// C `scs->tpl` for THIS pipeline's configuration
    /// ([`crate::inter_hdr_arm::scs_tpl`], a port of `get_tpl`,
    /// `Globals/enc_handle.c:3657`).
    ///
    /// Computed rather than written as a literal so the TPL-dependent
    /// refusals (`mfmv_level >= 2`) engage exactly when C's machinery does:
    /// `aq_mode == 2` under random access, where `run_tpl_stage` now runs
    /// `tpl_mc_flow` and `generate_r0beta` per base picture.
    pub(super) fn scs_tpl(&self) -> bool {
        crate::inter_hdr_arm::scs_tpl(
            self.gop.intra_period == 1,
            self.rc_config.aq_mode,
            // C's third disabling condition is `pred_structure == LOW_DELAY`.
            // The RA window path is the only non-low-delay encode this
            // pipeline runs, so TPL can engage exactly there.
            self.pred_structure != crate::port_picstruct::PredStructure::RandomAccess,
            self.superres_denom.is_some(),
        )
    }

    /// GLOBAL MOTION — the frame's own decision, not the preset's.
    ///
    /// C `svt_aom_derive_gm_level` (`enc_mode_config.c:194`) gives a NON-I
    /// slice `svt_aom_get_gm_core_level(enc_mode, super_res_off)`: 2 at
    /// `enc_mode <= ENC_MR`, 4 at `<= ENC_M4`, 0 above. A non-zero level only
    /// means C BUILDS `gm_ctrls`, though — whether it SEARCHES is
    /// `svt_aom_global_motion_estimation`'s own derivation
    /// (`crate::port_global_me`), and that is gated on
    /// `average_me_sad = sum(rc_me_distortion) / (w*h) >= 1` plus
    /// `bypass_based_on_me`.
    ///
    /// This site used to refuse on the LEVEL alone, i.e. on the preset, which
    /// is wrong in both directions of usefulness: it refused every inter frame
    /// at preset <= 4 including the ones where C provably codes seven
    /// `is_global = 0` bits — which is exactly what this port writes. MEASURED
    /// 2026-09-05 with `SVT_GM_OUT`: `avg_me_sad = 0` and `is_gm_on = 0` on
    /// {gradient, diag, screen} x {64, 128, 256, 512} and on `crop:` CID22
    /// photo at 256/512 with shifts 3/13/37 — the whole existing grid. With
    /// the refusal replaced by this derivation those cells encode, and 26 of
    /// them are byte-identical to C on both frames.
    ///
    /// When the derivation says C WOULD search, the port now RUNS the search
    /// and codes its result: `port_global_me::set_global_motion_field` builds
    /// the frame's `global_motion[]`, `port_entropy_inter::gm` writes
    /// `global_motion_params()`, and the same array feeds the MVP walk's
    /// `gm_mv`, the injector's GLOBALMV candidates and the prediction's
    /// `is_wm`. Only a frame whose search cannot RUN is still refused. See
    /// `tools/global_motion_gate.sh`.
    ///
    /// `super_res_off` is `true` at every call here: superres refuses inter
    /// frames at `superres_config_error`, and C's own `gm_level` only DROPS
    /// to 0 when superres is on, so `true` is the conservative reading.
    pub(super) fn gm_level_for_frame(&self, is_key: bool) -> u8 {
        crate::port_enc_mode_config::leaf::derive_gm_level(
            i8::try_from(self.speed_config.preset).unwrap_or(i8::MAX),
            is_key,
            /*super_res_off=*/ true,
        )
    }

    /// The refusal itself, once the frame's ME results exist.
    ///
    /// `None` = every reference keeps IDENTITY, which is what the header and
    /// the MVP environment already write.
    pub(super) fn gm_search_config_error(
        gm: Option<&crate::port_global_me::GmEstimation>,
        models: Option<&crate::port_global_me::GmModels>,
    ) -> Option<&'static str> {
        // No derivation at all (a key frame, or a preset where `gm_ctrls` are
        // disabled) — C memsets `is_global_motion` false and every model is
        // IDENTITY, which is what the header writes.
        let Some(g) = gm else { return None };
        // The derivation alone proved no search runs.
        if g.all_identity() {
            return None;
        }
        // The search RAN and left every reference IDENTITY. C's own
        // `pcs->is_gm_on` is 0 there, `global_motion[]` is identity, and the
        // header's seven zero bits are correct — so this frame encodes.
        if models.is_some_and(|m| !m.is_gm_on) {
            return None;
        }
        if crate::dbgenv::gm_experimental() {
            return None;
        }
        // THE SEARCH FINDING A MODEL IS NO LONGER A REFUSAL. `global_motion[]`
        // is built by `port_global_me::set_global_motion_field`, the header
        // codes it through `port_entropy_inter::gm::write_global_motion`, and
        // mode decision prices GLOBALMV against it — see the wiring below.
        //
        // What remains refused is the SEARCH not running at all, which is a
        // harness gap (a missing picture-analysis reference, or a downsample
        // level whose `svt_aom_upscale_wm_params` is unported) rather than a
        // picture-decision outcome. Treating it as IDENTITY would claim C found
        // nothing when the port simply did not look.
        if models.is_none() {
            return Some(
                "global motion is not implemented for this frame: C's \
                 svt_aom_global_motion_estimation would search (global_me.c:190), and this \
                 port could not run the search — the picture-analysis reference for a \
                 (list, ref) slot the search needs is missing, or the derived downsample \
                 level is not GM_FULL (crate::port_global_me::GmSearchError) [C: accepts]",
            );
        }
        None
    }

    /// Config knobs C rejects in `svt_av1_verify_settings`, refused here so
    /// the port never encodes a config the oracle cannot (issue #9):
    ///
    /// * `hdr.max_tx_size` must be 32 or 64 (enc_settings.c:922);
    /// * `rc_config.extended_crf_qindex_offset`: C's `str_to_crf` only ever
    ///   produces 0..=3 below qp 63, and `verify_settings` (:270) caps the
    ///   qp-63 extended range at 7*4 = 28 (CRF 70);
    /// * `chroma_sample_position` must be 0 (unknown), 1 (vertical) or 2
    ///   (colocated) — 3 is reserved and rejected (:762-770).
    pub(super) fn knob_config_error(&self) -> Option<&'static str> {
        if !matches!(self.hdr.max_tx_size, 32 | 64) {
            return Some("max_tx_size must be 32 or 64 (C verify_settings, enc_settings.c:922)");
        }
        let off = self.rc_config.extended_crf_qindex_offset;
        if (self.rc_config.qp < 63 && off > 3) || off > 28 {
            return Some(
                "extended_crf_qindex_offset must be 0..=3 (a quarter-step fractional CRF) or, at \
                 qp 63, at most 28 (CRF 70) — C verify_settings, enc_settings.c:270",
            );
        }
        if self.chroma_sample_position > 2 {
            return Some(
                "chroma_sample_position must be 0 (unknown), 1 (vertical) or 2 (colocated); 3 is \
                 reserved (C verify_settings, enc_settings.c:762)",
            );
        }
        // The fork/mainline tuning-knob bounds, `svt_av1_verify_settings`
        // (enc_settings.c:894-980). Without them a value past C's range
        // wraps or clamps silently inside a formula (`10 + (4 - s)` goes
        // negative for s > 4) — C's check exists precisely so the computed
        // shift factor stays in 0..=14.
        if self.hdr.tf_strength > 4 {
            return Some("tf_strength must be 0..=4 (C verify_settings, enc_settings.c:894)");
        }
        if self.hdr.noise_norm_strength > 4 {
            return Some(
                "noise_norm_strength must be 0..=4 (C verify_settings, enc_settings.c:945)",
            );
        }
        if self.hdr.kf_tf_strength > 4 {
            return Some("kf_tf_strength must be 0..=4 (C verify_settings, enc_settings.c:950)");
        }
        if self.hdr.sharp_tx > 1 {
            return Some("sharp_tx must be 0 or 1 (C verify_settings, enc_settings.c:955)");
        }
        if self.hdr.tx_bias > 3 {
            return Some("tx_bias must be 0..=3 (C verify_settings, enc_settings.c:960)");
        }
        if self.hdr.complex_hvs > 1 {
            return Some("complex_hvs must be 0 or 1 (C verify_settings, enc_settings.c:965)");
        }
        if self.hdr.noise_adaptive_filtering > 4 {
            return Some(
                "noise_adaptive_filtering must be 0..=4 (C verify_settings, enc_settings.c:970)",
            );
        }
        if !(1..=30).contains(&self.hdr.cdef_scaling) {
            return Some("cdef_scaling must be 1..=30 (C verify_settings, enc_settings.c:975)");
        }
        // Issue #9 item 8 — `aq_mode` semantics. C's `--aq-mode` default is
        // 2, which selects the TPL-gated per-SB deltaq
        // (`svt_aom_sb_qp_derivation_tpl_la`, rc_aq.c:899): live wherever
        // `tpl_ctrls.enable && r0 != 0` — i.e. random-access structures —
        // and INERT on stills and low-delay, exactly as C's `get_tpl`
        // disables the machinery there (enc_handle.c:3657). The port now
        // runs the same dispatch: `run_tpl_stage` produces r0/beta/scaling
        // for the RA window and the `rc_init_sb_qindex` twin consumes it.
        //
        // `aq_mode` 1 (variance AQ) and 3 (complexity AQ) are C's OTHER
        // per-SB deltaq derivations and are not ported — refuse rather than
        // silently take the TPL path for a knob that means something else.
        if !matches!(self.rc_config.aq_mode, 0 | 2) {
            return Some(
                "aq_mode must be 0 or 2: 2 is C's default TPL-gated per-SB deltaq \
                 (rc_aq.c:899), ported and live under random access; 1 (variance AQ) \
                 and 3 (complexity AQ) are different C derivations that are not \
                 ported [C: accepts]",
            );
        }
        None
    }

    /// Bit-depth configurations this encoder cannot encode faithfully, refused
    /// at the [`Self::encode_frame_impl`] choke point rather than emitted.
    ///
    /// Two distinct failure shapes:
    ///
    /// 1. **Any depth other than 8 or 10.** C v4.2.0 rejects those itself
    ///    (`svt_av1_verify_settings`, `Globals/enc_settings.c:460`), and this
    ///    port has no 12-bit path at all: `deblock::pick_filter_levels_key_frame`
    ///    hits `unreachable!()` at preset >= 6, and below that the sequence
    ///    header would advertise `seq_profile = 2` without the spec-5.5.2
    ///    subsampling bits that profile requires — an unparseable SH.
    ///
    /// 2. **`bit_depth == 10` with no bd10 producer** (see
    ///    [`Self::bd10_levels_native`]). Outside that envelope the entire
    ///    encode runs in the 8-bit domain — `quant::build_quant_table` takes no
    ///    bit-depth parameter, so the levels are Q8 — while the sequence header
    ///    signals `high_bitdepth = 1`. The decoder then dequantizes with the Q10
    ///    tables and reconstructs a picture the encoder never saw; because Q10
    ///    is only *approximately* 4x Q8, the error compounds through intra
    ///    prediction across the frame. On top of that the deblock levels
    ///    (preset >= 6) and CDEF strengths (preset >= 7) are signalled from the
    ///    bd10 closed forms while being applied by the encoder with the u8
    ///    kernels — three independent scale errors in one stream.
    ///
    ///    That output is decodable and looks like a successful encode at the
    ///    integration seam, which is exactly the class `rust/CLAUDE.md`
    ///    ("Refuse out-of-envelope configs; never emit a plausible-but-wrong
    ///    stream") forbids. The native producers now cover both formats at
    ///    every preset; retain the defensive check for future configuration gaps.
    pub(super) fn bit_depth_config_error(&self, chroma_420: bool) -> Option<&'static str> {
        match self.bit_depth {
            8 => return None,
            10 => {}
            _ => {
                // WORDING IS LOAD-BEARING HERE. `tools/refusal_inventory.sh`
                // classifies a refusal by its words, and the previous text
                // ended "and this port has no 12-bit kernels" — which matched
                // the CAPABILITY keyword list and filed a PERMANENT UPSTREAM
                // constraint in the table whose header says "this is DEBT".
                // It is not debt: C rejects the config at
                // `svt_av1_verify_settings`, so no oracle exists for any other
                // depth and implementing 12-bit kernels would put this encoder
                // OUTSIDE the envelope it is measured against. The identical
                // rule in `svtav1/src/avif.rs` was already filed as CONTRACT,
                // so the same constraint sat in both halves of the ledger.
                return Some(
                    "bit depth must be 8 or 10 — C v4.2.0 rejects every other depth at encoder \
                     init (svt_av1_verify_settings, Globals/enc_settings.c:460), so no oracle \
                     exists at any other depth: this is C's envelope, not this port's backlog \
                     [C: rejects]",
                );
            }
        }
        if self.bd10_levels_native(chroma_420) {
            return None;
        }
        // DEFENSIVE, AND PROVEN UNREACHABLE TODAY. Native mono levels cover
        // all presets; color has `preset >= 9 || preset <= 8`, so no
        // caller can land here. It is kept because the alternative is an
        // implicit `None` that would let a FUTURE bd10 gap encode 8-bit
        // levels under a 10-bit sequence header, which is the exact failure
        // this predicate exists to stop. `bd10_config_error_third_arm_is_
        // unreachable_over_the_whole_product` pins the claim, so if a new gap
        // ever makes it reachable that test fails and demands a message
        // naming the real gap instead of this catch-all.
        Some(
            "this 10-bit configuration has no bd10 stage to produce the coded levels; the encode \
             would be 8-bit-quantized under a 10-bit sequence header (defensive catch-all — \
             unreachable in the shipped envelope, see the unreachability test) [C: accepts]",
        )
    }

    /// Superres chunk B.3 — the combinations whose SIGNALLED stream would not
    /// match what the encoder actually produced. Rejecting beats emitting a
    /// stream that says "upscale me" over content the encoder handled with the
    /// wrong geometry.
    /// Issue #5: the coded-lossless (QP 0 / `base_q_idx` 0) envelope this port
    /// has byte-verified against the C oracle, as a refusal predicate for
    /// everything outside it. Each arm names the missing piece.
    pub(super) fn lossless_config_error(&self, is_key: bool) -> Option<&'static str> {
        if self.hdr.is_fork() {
            // The fork's chroma-q deltas (Cb +12) put every segment's chroma
            // qindex above 0 while base_q_idx stays 0, so the frame is NOT
            // CodedLossless yet quantizes luma at qindex 0 — and C's variance
            // boost is internally inconsistent there (SUSPECTED-C-BUGS.md #1).
            return Some(
                "QP 0 (coded-lossless) in HDR-fork mode is not implemented: the fork's chroma-q \
                 deltas leave the frame outside CodedLossless (spec 5.9.2) with base_q_idx 0 — \
                 use mainline mode or QP >= 1",
            );
        }
        if !is_key || self.gop.intra_period != 1 {
            // The encode can contain INTER frames. Coded-lossless inter is
            // decoder-verified only on the 8-bit 4:2:0 funnel arm: real
            // inter-coded blocks emit WHT residuals aomdec reconstructs
            // byte-exactly (qp0-inter sweep 2026-09-21: sb64+sb128). At
            // 10 bits the inter-frame lossless path predicts intra
            // per-BLOCK while the decoder predicts per-TXB (measured:
            // encoder recon == source but aomdec diverges), and the
            // monochrome / 4:4:4 arms have no inter WHT residual path at
            // all — their qp0 streams are legal but silently all-intra
            // (measured 2026-09-21: zero inter block decisions). Refuse
            // rather than emit them.
            if self.bit_depth != 8 || !self.chroma_420 {
                return Some(
                    "QP 0 (coded-lossless) inter frames are not implemented outside \
                     8-bit 4:2:0: the 10-bit path mis-predicts intra per-block vs \
                     the decoder's per-TXB rule and the monochrome / 4:4:4 arms \
                     have no inter WHT residual path [C: accepts] — use QP >= 1",
                );
            }
        }
        if self.superres_denom.is_some() {
            return Some(
                "QP 0 (coded-lossless) with superres is not implemented (the frame is not \
                 AllLossless at the upscaled size) — use QP >= 1 [C: accepts]",
            );
        }
        None
    }

    pub(super) fn sb_size_config_error(&self) -> Option<&'static str> {
        match self.sb_size_override {
            None | Some(64 | 128) => None,
            Some(_) => Some("superblock size override must be 64 or 128"),
        }
    }

    /// BITRATE-TARGETED RATE CONTROL — C's own envelope plus this port's
    /// coverage inside it.
    ///
    /// C admits `SVT_AV1_RC_MODE_CBR` only under LOW_DELAY and
    /// `SVT_AV1_RC_MODE_VBR` only outside it (enc_settings.c:157/:177). The
    /// port wires the one-pass CBR driver (`cbr_frame_qindex` →
    /// `port_rc_vbr_cbr_*` → `cbr_postencode`) on exactly that envelope.
    /// VBR stays refused: its ported arm consumes first-pass statistics and
    /// `firstpass.c` is not ported, so it would emit CRF-shaped output under
    /// a bitrate label — the plausible-but-wrong class the issue #22 refusal
    /// was written against.
    pub(super) fn rate_control_config_error(&self) -> Option<&'static str> {
        match self.rc_config.mode {
            crate::rate_control::RcMode::Cqp | crate::rate_control::RcMode::Crf => None,
            crate::rate_control::RcMode::Vbr => Some(
                "VBR rate control is not implemented: the ported two-pass arm \
                 (`svt_aom_process_rc_stat`/`av1_set_target_rate`, \
                 pass2_strategy.c) needs first-pass statistics, and \
                 firstpass.c is not ported — wiring VBR without them would \
                 emit CRF-shaped output under a bitrate label. Use \
                 RcMode::Cbr (LOW_DELAY) or RcMode::Cqp/Crf [C: accepts VBR \
                 only outside LOW_DELAY, enc_settings.c:177]",
            ),
            crate::rate_control::RcMode::Cbr => {
                // C's own envelope (enc_settings.c:157): CBR exists only
                // under LOW_DELAY. Inside it the ported one-pass driver
                // runs; outside it refuse rather than emit a stream C
                // would never produce.
                if self.pred_structure == crate::port_picstruct::PredStructure::LowDelay {
                    None
                } else {
                    Some(
                        "CBR rate control requires pred_structure == LOW_DELAY \
                         — C's own constraint (enc_settings.c:157, \"CBR Rate \
                         control is currently not supported for \
                         RANDOM_ACCESS/ALL_INTRA, use VBR mode\") [C: refuses]",
                    )
                }
            }
        }
    }

    /// Hierarchical levels above C's own ceiling are refused.
    /// `enc_settings.c:275` rejects `hierarchical_levels > 5`
    /// ("Hierarchical Levels supported: [0-5]"), and the pred-struct tables
    /// this port drives end at level 5 (`PRED_STRUCT_TEMPORAL_LAYER`) — the
    /// table index itself would panic, so this guard is unconditional, key
    /// frames included.
    pub(super) fn gop_config_error(&self, _is_key: bool) -> Option<&'static str> {
        if self.gop.hierarchical_levels > 5 {
            return Some(
                "hierarchical_levels > 5 is outside C's own supported range                  (enc_settings.c:275, \"Hierarchical Levels supported: [0-5]\") and                  the pred-struct tables end at level 5. Use hierarchical_levels                  <= 5 [C: refuses]",
            );
        }
        None
    }
}
