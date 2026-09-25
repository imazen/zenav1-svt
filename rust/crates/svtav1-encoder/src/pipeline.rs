//! Encoding pipeline orchestrator — wires all stages together.
//!
//! Spec 00 (architecture.md): Full encoding pipeline orchestrator.
//!
//! This is the top-level encoding function that coordinates:
//! 1. Picture analysis (noise estimation, scene detection)
//! 2. Reference frame management (DPB, GOP structure)
//! 3. Motion estimation
//! 4. Mode decision + partition search
//! 5. Encoding loop (transform, quantize, entropy)
//! 6. Loop filtering (deblock, CDEF, restoration)
//! 7. Reconstruction and reference frame update
//! 8. Bitstream packetization (OBU output)

use crate::picture::{DecodedPictureBuffer, GopStructure, PictureControlSet, ReferenceFrame};
use crate::rate_control::{RcConfig, RcState, assign_picture_qp, update_rc_state};
use crate::speed_config::SpeedConfig;
use crate::{EncodeError, EncodeResult};
use alloc::vec;
mod bd10_reencode;
use bd10_reencode::{bd10_reencode_chroma, bd10_reencode_luma};

use alloc::boxed::Box;
use alloc::vec::Vec;
// `StopToken::check` is a method of the `enough::Stop` trait; bring the trait
// into scope so the frame-entry cancellation check resolves.
use enough::Stop;

impl EncodePipeline {
    // ------------------------------------------------------------------
    // Random access (`SVT_PRED_STRUCT=2`): the mini-GOP window driver
    // ------------------------------------------------------------------
    //
    // C's picture decision never runs on a single frame under random access.
    // Input frames accumulate in a PRE-ASSIGNMENT BUFFER in display order
    // until a whole mini-GOP (`1 << hierarchical_levels` pictures) is present
    // or the end of sequence cuts it short; `set_mini_gop_structure` then
    // maps the window, the kernel walks it once in display order to assign
    // each picture's pred-structure position, once more to assign
    // `decode_order` (`decode_base_number + entry.decode_order` for a
    // complete mini-GOP, else the `picture_number_alt` counter), and a third
    // time in DECODE order for the RPS/DPB state (`pd_process.c:5470-5693`).
    // `process_pics` emits the pictures in that same decode order, with a
    // `show_existing_frame` OBU_FRAME_HEADER interleaved wherever a hidden
    // picture's display position is reached.
    //
    // `ra_input` is the port's pre-assignment buffer.

    /// Shared frame encode body. `chroma = Some((u, v))` selects the 4:2:0
    /// path; `None` is the unchanged monochrome path.
    fn encode_frame_impl(
        &mut self,
        y_plane: &[u8],
        y_stride: usize,
        chroma: Option<(&[u8], &[u8])>,
        decided: Option<FrameDecision>,
    ) -> crate::EncodeResult<Vec<u8>> {
        // An AUTO `hierarchical_levels` that survived to encode time (no
        // `with_pred_structure` call — the low-delay default) resolves here,
        // the last point before `self.gop` is first read. A no-op after
        // `with_pred_structure` resolved it, or when the level was explicit.
        self.resolve_hierarchical_levels_auto();
        // `chroma_420` is the configured-format check (refuses 4:4:4 where
        // `chroma.is_some()` would pass); `chroma.is_some()` is the per-frame
        // check (refuses the mono entry on a chroma-configured pipeline).
        self.enhancements
            .validate(
                self.speed_config.preset,
                self.gop.intra_period == 1,
                self.chroma_420 && chroma.is_some(),
                self.bit_depth,
            )
            .map_err(|why| whereat::at!(EncodeError::UnsupportedConfig(why)))?;
        let zen_intra_edge_filter = self
            .enhancements
            .contains(crate::enhancements::ZenEnhancement::AomIntraEdgeFilter);
        self.reference
            .validate_hdr_config(&self.hdr)
            .map_err(|why| whereat::at!(EncodeError::UnsupportedConfig(why)))?;
        if self.reference == crate::reference::SvtReference::Mainline420 && chroma.is_none() {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "pristine mainline SVT supports 4:2:0 only; monochrome is a Rust extension",
            )));
        }
        #[cfg(feature = "__expert")]
        if self.chroma_q_override.is_some() && chroma.is_none() {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "chroma_q_override is set but this frame is monochrome: there is no U/V \
                 quantizer to override; clear the override for mono frames [C: no mono]",
            )));
        }
        // TUNE overrides (C `svt_av1_enc_set_parameter`, enc_handle.c:4889).
        // `--tune 3` (IQ, "still image only") and `--tune 4` (MS-SSIM) are not
        // single RD knobs: C rewrites qm on/min/max (luma AND chroma),
        // sharpness, variance boost on/strength/curve, and — for IQ —
        // `max_tx_size` and `screen_content_mode`. Applying them here, once,
        // against the CLI-domain qp is what makes `hdr.tune = TUNE_IQ` in this
        // port mean the same thing as `--tune 3` in C. A no-op for every other
        // tune, so the default path is byte-unchanged.
        //
        // These four (tune, QM, variance boost, sharpness) are MAINLINE v4.2.0
        // features, not fork additions — they used to be gated behind
        // `is_fork()` here, which silently ignored them in mainline mode.
        //
        // The application point MUST precede the superblock derivation below:
        // C applies these in `copy_api_from_app` (enc_handle.c:4890-4918) and
        // derives `super_block_size` later in `set_param_based_on_input`
        // (:4995 -> :4077), so the tune-IQ `enable_variance_boost = 1` is
        // already visible when the sb-size rule runs — and that rule forces
        // SB64 whenever variance boost is on. `new()` cannot reproduce that
        // ordering because `self.hdr` is caller-mutated after construction.
        self.hdr.apply_tune_overrides(self.rc_config.qp);
        // C derives `super_block_size` in `svt_av1_enc_init_handle` — AFTER
        // the full static_config exists — and `enable_variance_boost` forces
        // 64 (enc_handle.c:4075-4078). This port mutates `self.hdr` AFTER
        // `new()` ran the derivation with `variance_boost: false`, so an
        // encode-time VB request (tune IQ, `with_variance_boost`) could
        // leave a sb128 derivation in place that C never emits — and whose
        // delta-q emission then desynced the tile. Re-run the derivation at
        // the encode choke point with the true VB input.
        // An explicit `with_sb_size(128)` override still wins (that config
        // is a port extension C cannot express; the emission path below now
        // codes it decodably, just not byte-identically to anything C makes).
        {
            let sb_inputs = crate::sb128_geom::SbSizeInputs {
                qp: self.rc_config.qp,
                allintra: self.gop.intra_period == 1,
                variance_boost: self.hdr.enable_variance_boost,
                ..Default::default()
            };
            let derived = crate::sb128_geom::derive_super_block_size(
                self.width as usize,
                self.height as usize,
                self.speed_config.preset as i8,
                &sb_inputs,
            );
            let (sb_size, fell_back) =
                Self::resolve_sb_size(derived, self.sb_size_override, self.speed_config.preset);
            self.sb_size = sb_size;
            self.sb128_fallback = fell_back;
        }
        if let Some(why) = self.sb_size_config_error() {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(why)));
        }
        if let Some(reason) =
            crate::entropy::obu::TileLimits::for_frame(self.width, self.height, self.sb_size as u32)
                .untileable_reason(self.tile_cols_log2)
        {
            return Err(whereat::at!(EncodeError::InvalidDimensions {
                width: self.true_width,
                height: self.true_height,
                reason,
            }));
        }
        self.validate_film_grain()?;
        // `decided` carries a random-access frame: its display-order POC and
        // the `PicParams` the mini-GOP window decision already produced
        // (`encode_ra_window`). `None` is the sequential contract — display
        // order is the coded-frame count and picture decision runs inline.
        let display_order = decided
            .as_ref()
            .map_or(self.frame_count, |d| d.display_order);
        // Superres chunk B.3: refuse any combination whose SIGNALLED geometry
        // would not match what this encoder actually produced (see
        // `superres_config_error`). Checked at the single choke point every
        // entry point funnels through, so no path can slip past it.
        if let Some(why) = self.superres_config_error() {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(why)));
        }
        // Same choke point, same rule, for the bit-depth axis: a 10-bit request
        // that no bd10 stage can serve would emit 8-bit-quantized levels under a
        // 10-bit sequence header. See `bit_depth_config_error`. This covers the
        // u8 entry points too — the `hbd_source_consumed` screen on the `*_hbd`
        // entries only fires when the caller passed a native u16 source, so
        // `with_bit_depth(10)` + `encode_frame_420` used to slip past every guard.
        if let Some(why) = self.bit_depth_config_error(chroma.is_some()) {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(why)));
        }
        // Issue #9 items 3-5: the three config knobs C validates at init
        // (`svt_av1_verify_settings`) and this port therefore refuses at the
        // same choke point rather than encoding something C would never emit.
        if let Some(why) = self.knob_config_error() {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(why)));
        }
        // Issue #22: VBR/CBR were ACCEPTED here and silently encoded at qp 30.
        if let Some(why) = self.gop_config_error(self.gop.is_key_frame(display_order)) {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(why)));
        }
        if let Some(why) = self.rate_control_config_error() {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(why)));
        }
        // GLOBAL MOTION is NOT refused here: whether C searches depends on this
        // frame's ME residual, which does not exist yet at this choke point.
        // See `gm_search_config_error`, called immediately after `frame_me` below.
        // MULTI-FRAME IS NOT ENCODABLE — refuse it rather than emit a corrupt
        // stream. The 4:2:0 path already asserted this below; the MONOCHROME
        // path did not, so `EncodePipeline::new(w, h, preset, rc, hier,
        // /*intra_period=*/64).encode_frame(..)` produced inter frames with:
        //   - the sequence header's `order_hint_bits_minus_1` written BEFORE
        //     `seq_choose_screen_content_tools` / `seq_choose_integer_mv`,
        //     where spec 5.5.1 and C (entropy_coding.c:2812-2838) put it after;
        //   - `initial_display_delay_present_flag` hardcoded 0, omitting the
        //     five bits C always writes (enc_handle.c:4981-4993,
        //     entropy_coding.c:3731,:3749-3755);
        //   - an illegal 8-bit `refresh_frame_flags` on a SHOWN key frame
        //     (C writes it only `if (!show_frame)`, entropy_coding.c:3404-3407);
        //   - no `disable_frame_end_update_cdf`, which shifts `tile_info()` and
        //     every following field by one bit (entropy_coding.c:3553-3559);
        //   - MV coding against FRESH per-block CDFs, and an inter frame header
        //     with an admittedly incomplete `tile_info()` (obu.rs).
        //
        // MEASURED 2026-08-03 on a 5-frame 64x64 gradient encode through the
        // public API: aomdec reports "Corrupt frame detected: Failed to decode
        // tile data" at frame 1, and dav1d reports "Overrun in OBU bit buffer"
        // then "No data decoded". So this is not a byte-parity gap — it is the
        // zero-tolerance corruption class, on the public entry point.
        //
        // The fix is deliberately a REFUSAL, not a header patch: the inter path
        // beyond the header is unported too (no chroma in the DPB, homegrown ME,
        // fresh-CDF MV coding), so correcting the four fields would buy a
        // better-formed header on top of a stream that still cannot decode.
        //
        // The predicate is the FRAME TYPE, not `intra_period` — see the check
        // beside `is_key` below. A caller may legitimately construct a pipeline
        // with a GOP structure and encode only its key frame; that stream is a
        // valid still and is what several tests do. Only an actual INTER frame
        // is unencodable.
        // Task #6 chunk 1: TAKE the native 10-bit source (set only by the
        // `*_hbd` entry points) so it can never leak into a following u8
        // frame. `None` on every u8 path -> every bd10 stage keeps widening
        // `u8 << 2` exactly as before.
        let hbd_source = self.hbd_source.take();
        // Superres chunk B.3: the pre-scaling picture-statistics source (see
        // `superres_stats_luma`). `None` on every non-superres path.
        let stats_src = self.superres_stats_luma.take();
        let stale_vars = derive_stale_vars(stats_src);
        // Did a bd10 consumer actually READ it? The entry points pre-screen
        // the config (`hbd_source_consumed`), but the post-pass additionally
        // requires every SB's tree to be bd10-supported at RUNTIME — so this
        // flag is what turns "the low bits were silently dropped" into an
        // explicit error instead of a quietly-8-bit stream.
        let mut hbd_used = false;
        // The funnel side reports through an atomic because the tile loop
        // runs the funnel inside `std::thread::scope` (byte-inert: the flag
        // is write-only and never read by the search).
        let hbd_used_flag = core::sync::atomic::AtomicBool::new(false);
        // Feature 1: snapshot the cooperative-cancellation token once (a cheap
        // Arc clone; `Send + Sync`) so the per-SB loops here, the entropy-walk
        // closure, and `encode_tile_rows` all check the same token. The default
        // `Unstoppable` token's `may_stop()` is `false`, so every guarded check
        // below is a byte-inert false-branch.
        let stop = self.stop.clone();

        // Step 1: Determine frame type from GOP structure
        let is_key = self.gop.is_key_frame(display_order);
        // `__expert` chroma override: the SH (written on key frames only)
        // fixes separate_uv_delta_q until the next key frame, so an override
        // change that flips U/V separation mid-sequence is refused rather
        // than signalled under a mismatched SH.
        #[cfg(feature = "__expert")]
        {
            let separate = self.separate_uv_delta_q();
            if is_key {
                self.sh_separate_uv_delta_q = Some(separate);
            } else if self.sh_separate_uv_delta_q.is_some_and(|s| s != separate) {
                return Err(whereat::at!(EncodeError::UnsupportedConfig(
                    "chroma_q_override changed U/V separation after the key frame; the \
                     sequence header fixes separate_uv_delta_q until the next key frame",
                )));
            }
        }
        // C picks a DIFFERENT derivation function per arm of `scs->allintra`
        // (enc_handle.c:4406 — `intra_period_length == 0 || avif ||
        // pred_structure == ALL_INTRA`); the port's proxy for it is the same
        // conjunction. On the video arm `is_islice` marks the video-mode
        // I-slice — `is_key` covers every case this reaches today.
        //
        // Bound at frame level because `eff_enc_mode(sc_arm, ..)` is C's
        // `pcs->enc_mode` — the POST-clamp `static_config.enc_mode`
        // (enc_handle.c:4415-4436: allintra above M9 -> M9, non-RTC video
        // above M11 -> M11, runtime_enc_mode = static_config at
        // resource_coordination_process.c:139/:1267). Every C `enc_mode`
        // ladder must see the clamped value: at CLI p13 a video frame's
        // pic_lpd1_lvl, ME search area, prehme level and
        // stats_based_sb_lambda_modulation all derive differently from
        // raw 13 than from C's 11.
        let sc_arm = if self.gop.intra_period == 1 {
            crate::sc_detect::ScArm::Allintra
        } else {
            crate::sc_detect::ScArm::Video { is_islice: is_key }
        };
        // INTER FRAMES SHIP ON THE 4:2:0 PATH AND ARE REFUSED ON THE
        // MONOCHROME ONE, and that asymmetry is the whole of what this guard
        // now says.
        //
        // The blanket "inter frames are not implemented for the public API"
        // refusal, and the `SVTAV1_INTER_EXPERIMENTAL` variable that lifted
        // it, are both gone as of 2026-09-11. What replaced them is a
        // measurement the refusal itself asked for: `video_selfcheck_gate.sh`
        // decodes the port's own 8-frame stream with `aomdec` and requires the
        // port's final reconstruction to be byte-identical to the decoder's on
        // EVERY frame — 18 of 18 cells over six public-domain derf clips at
        // qp {20,40,55}. The stream the refusal called "silently wrong" is
        // measurably not.
        //
        // WHAT THIS DOES NOT CLAIM. Byte-identity to C on the 4:2:0 inter path
        // is NOT universal, and the README's video rows say so. MEASURED
        // 2026-09-11 on the campaign's 96-cell frontier grid
        // ({uniform,gradient,diag,screen} x {16,64,72,128} x {q20,q40,q55} x
        // {p6,p8}) at frames=4 low-delay P: 95 cells identical on frame 0, 95
        // on frame 1, 60 on frame 2 and 58 on frame 3. The chain frames'
        // divergence is concentrated in two known frontiers — 72x72 is a
        // PARTIAL superblock (17 of its 24 cells differ at frame 2) and
        // `gradient` content (19 of 24) — and `uniform` is 24 of 24 identical
        // on every frame. Re-measure those numbers in the SAME change whenever
        // `tools/inter_byte_matrix.sh` moves.
        //
        // THE MONOCHROME ARM SHIPS INTER — measured 2026-09-21. The corrupt
        // stream this refusal once cited was produced before the inter
        // correctness landings (write-time `overlappable_neighbors` /
        // `num_proj_ref` derivation, recon-only eob preservation, OBMC on
        // `SimpleTranslation`), all of which are format-agnostic: with them
        // in place a mono inter frame is byte-identical between this
        // encoder's reconstruction, `aomdec` AND `dav1d` across presets
        // {0,6,8,13}, sizes 64x64..256x128, sb64+sb128, qp0-lossless and
        // 10-bit, with real nonzero MVs — `tools/mono_inter_gate.sh` is the
        // standing witness. `chroma.is_none()` simply means the frame has
        // no chroma planes to predict, code or filter: the seq header
        // already signals mono_chrome, and the decoder derives a single
        // plane throughout.
        //
        // Keyed on the FRAME TYPE rather than `intra_period` so that
        // constructing a pipeline with a GOP structure and encoding only its
        // key frame keeps working: that stream is a valid still.
        // The 10-bit inter floor lifted 2026-09-18: the `hbd_md = 2` MDS3
        // bump mirror (product_coding_loop.c:9649 — `LeafBd10::mds3_hbd`)
        // closed the recon drift the refusal here used to cite, and
        // `bd10_video_selfcheck_gate.sh` pins the replacement measurement:
        // 396/396 cells x 8 frames reconstruct identically to `aomdec`
        // (presets -1..13 at 256x256, -1..5 at 128x128, qp {20,40,55}, six
        // derf clips).
        // Step 1b: C's PICTURE DECISION — the reference structure.
        //
        // `picture_decision_per_picture` fills `rps.ref_dpb_index[]` (the
        // header's `ref_frame_idx[]`), `rps.refresh_frame_mask` (its
        // `refresh_frame_flags`), the skip-mode allowance and the shadow DPB.
        // It has to run on the KEY frame too — `set_key_frame_rps` seeds the
        // layer-0 toggle ring that every later frame's refresh mask advances
        // from, so skipping it would put frame 1 in the wrong DPB slot.
        //
        // Byte-inert for a still encode: it is skipped entirely when no GOP is
        // configured, and even when it runs it only writes `self.pd_ctx` and
        // the local `PicParams` — nothing downstream of a KEY frame reads
        // either.
        let decided_is_some = decided.is_some();
        let (pic_decision, mut tpl_in) = if self.gop.intra_period != 1 {
            match decided {
                // Random access: the window decision already ran RPS + DPB
                // state for this picture — re-running it per frame would
                // advance the toggles twice. `tpl` is the TPL stage's
                // per-picture bundle (`None` whenever `scs->tpl` was off).
                Some(d) => (Some(d.pic), d.tpl),
                None => (
                    Some(self.run_picture_decision(display_order, is_key)?),
                    None,
                ),
            }
        } else {
            (None, decided.and_then(|d| d.tpl))
        };
        // `pcs->temporal_layer_index = pred_position_ptr->temporal_layer_index`
        // (`pd_process.c:5559`) — the pred-struct ENTRY's layer, read back from
        // the picture decision, not a position-derived paraphrase (the flat
        // GOP's only entry is layer 0, which is also the `None` answer).
        let temporal_layer = pic_decision.as_ref().map_or(0, |p| p.temporal_layer_index);
        // C reads `ppcs->hierarchical_levels` — the MINI-GOP's level for this
        // picture, which a cut-short RA window subdivides below the
        // configured `gop.hierarchical_levels` (`get_pred_struct_for_frame`).
        // Equals the configured level on every low-delay frame, so this is
        // byte-inert outside random access.
        //
        // Two stamps land on the same field: `pd_process.c:968` gives an IDR
        // the CONFIGURED level at decision time, then a DELAYED intra's TF
        // setup re-stamps it with the following mini-GOP's level
        // (`pd_process.c:3936-3943`, "Update the key frame pred structure" —
        // `filter_delayed_intra` is the port's copy). The picture's own field
        // therefore carries the right answer in both cases; reading
        // `gop.hierarchical_levels` for a key instead skips the delayed-intra
        // overwrite — measured 2026-09-25: a hier-5 RA cell coded the key's
        // `percents[0][0]` (75) arm as qindex 70 where C takes the re-stamped
        // level's `percents[1][0]` (76) arm and writes 67.
        let frame_hier = pic_decision
            .as_ref()
            .map_or(self.gop.hierarchical_levels, |p| p.hierarchical_levels);

        // C `av1_lambda_assign_md`'s TWO update-type selectors
        // (`pd0::inter_full_lambda_8bit`): the rdmult BASE reads
        // `ppcs->update_type`, which the picture decision already resolved,
        // and the frame-type FACTOR reads `update_lambda`'s own
        // `gf_update_type`. On a flat low-delay P GOP they DISAGREE (Lf vs
        // Arf), so both are carried and neither is derived from the other.
        // Hoisted here because the funnel's `c_quant` and PD0's own
        // `full_sb_lambda_md` must be the same number.
        let md_lambda_base_update_type = pic_decision.as_ref().map(|pic| match pic.update_type {
            crate::port_picstruct::FrameUpdateType::Kf => {
                crate::port_rc_process::FrameUpdateType::KfUpdate
            }
            crate::port_picstruct::FrameUpdateType::Lf => {
                crate::port_rc_process::FrameUpdateType::LfUpdate
            }
            crate::port_picstruct::FrameUpdateType::Gf => {
                crate::port_rc_process::FrameUpdateType::GfUpdate
            }
            crate::port_picstruct::FrameUpdateType::Arf => {
                crate::port_rc_process::FrameUpdateType::ArfUpdate
            }
            crate::port_picstruct::FrameUpdateType::Overlay => {
                crate::port_rc_process::FrameUpdateType::OverlayUpdate
            }
            crate::port_picstruct::FrameUpdateType::IntnlOverlay => {
                crate::port_rc_process::FrameUpdateType::IntnlOverlayUpdate
            }
            crate::port_picstruct::FrameUpdateType::IntnlArf => {
                crate::port_rc_process::FrameUpdateType::IntnlArfUpdate
            }
        });
        let md_lambda_factor_update_type =
            crate::port_rc_process::lambda_gf_update_type(is_key, frame_hier, temporal_layer);
        let md_alt_lambda_factors = self.hdr.is_fork() && self.hdr.alt_lambda_factors;

        // C `pcs->ref_intra_percentage` (`get_ref_intra_percentage`,
        // rc_process.c:66, set in `rc_init_frame_stats` at :613): the mean
        // `intra_coded_area` of the two nearest references, I-slice refs
        // skipped. Hoisted to frame level — it feeds `av1_lambda_assign_md`'s
        // LAMBDA_MOD_INTRA arm (md_process.c:738) and PD0's
        // `use_ref_info`/skip_intra controls alike. 0 on a key frame (no
        // decision), matching C's `ref_cnt == 0` answer.
        let md_ref_intra_percentage = pic_decision.as_ref().map_or(0, |p| {
            crate::port_rc_process::get_ref_intra_percentage(
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
        });
        // C `av1_lambda_assign_md`'s LAMBDA_MOD_INTRA arm
        // (md_process.c:730-745): `!rtc && stats_based_sb_lambda_modulation`
        // — `static_config.enc_mode <= M11` (enc_handle.c:4375) on the
        // POST-clamp mode, so live at EVERY video preset (p12/p13 encode as
        // M11) — `&& temporal_layer_index > 0 &&
        // ref_intra_percentage < (alt_lambda_factors ? 65 : 50)` scales the
        // SB's full and fast lambdas by 138>>7. MEASURED: `diag 64x64 q40 p8
        // hier=3` poc=4 (tl=1) prices its trellis at rdmult 206 584, i.e.
        // `full_lambda_md` 51 646, not the 47 905 the unmodulated chain
        // gives — C dropped a raster-3 coefficient the port kept (`eob` 6 vs
        // 7). 128 is the arm-not-taken identity; on a flat LD GOP every frame
        // is tl 0, so this is byte-inert there.
        let lambda_mod_intra: i64 = if temporal_layer > 0
            && crate::port_rc_process::stats_based_sb_lambda_modulation(
                crate::rate_arm::eff_enc_mode(sc_arm, self.speed_config.preset),
                false,
            )
            && md_ref_intra_percentage < if md_alt_lambda_factors { 65 } else { 50 }
        {
            138
        } else {
            128
        };

        // Step 2: Create PCS
        let mut pcs = if is_key {
            PictureControlSet::new_key_frame(self.width, self.height, display_order)
        } else {
            PictureControlSet::new_inter_frame(
                self.width,
                self.height,
                display_order,
                // `pcs->decode_order` — the RA window's permutation; display
                // order under low delay (the same value `pic_decision` carries).
                pic_decision
                    .as_ref()
                    .map_or(display_order, |p| p.decode_order),
                temporal_layer,
            )
        };
        // `pcs->frm_hdr.show_frame` — a hidden RA picture is coded but not
        // shown; `inter_signal` reads it through `pic_decision`, and
        // anything else wanting it must see the same value.
        if let Some(pic) = pic_decision.as_ref() {
            pcs.show_frame = pic.show_frame;
        }

        // C `frm_hdr->refresh_frame_flags = ppcs->rps.refresh_frame_mask`
        // — the SAME value the frame header signals (`inter_hdr_arm.rs`:
        // `refresh_frame_flags: pic.rps.refresh_frame_mask`).
        //
        // `PictureControlSet::new_inter_frame` hard-codes 0, and that constant
        // reached `self.dpb.refresh(..)` — so the HEADER announced C's real
        // mask while THIS ENCODER'S DPB never received an inter frame at all.
        // Invisible at two frames (nothing reads the DPB after frame 1) and
        // fatal at three: MEASURED 2026-09-03 on
        // `gradient 64x64 q32 p8 frames=3`, at poc 2 the port's
        // `rps.ref_dpb_index[0]` is slot 1 and every slot still held the KEY
        // frame, so LAST resolved to poc 0 where C's is poc 1. Every
        // frame-2 reading taken before this fix is void.
        //
        // A key frame keeps 0xFF from `new_key_frame`: it has no picture
        // decision on the allintra path, and C's own key-frame mask is 0xFF.
        if let Some(pic) = pic_decision.as_ref() {
            pcs.refresh_frame_flags = pic.rps.refresh_frame_mask;
        }

        // Step 3: Rate control — assign QP
        //
        // `temporal_layer` is passed as 0 on purpose: in C's CQP/CRF path
        // `picture_qp` stays the CLI qp for EVERY temporal layer — the
        // per-layer scaling lives inside `cqp_qindex_calc` on the qindex
        // (rc_crf_cqp.c:393), not on the CLI-domain value. Letting
        // `assign_picture_qp`'s homegrown `TEMPORAL_LAYER_QP_DELTA` apply
        // here too would double-count the boost on a hierarchical GOP. Byte-
        // inert on the flat GOP: its only temporal layer is 0 and
        // `delta[0] == 0`. (VBR/CBR are refused upstream.)
        pcs.qp = assign_picture_qp(&self.rc_config, &self.rc_state, 0);

        // Step 3b: temporal filtering is NOT done here. C's
        // `produce_temporally_filtered_pic` is ported as
        // `crate::port_tf_driver` and runs upstream on the random-access
        // buffer (`ra_tf_bufs` / `TfPicBufs`), so this function receives the
        // already-filtered source; low-delay has no TF in C
        // (`derive_tf_params`, enc_handle.c:3339-3343). The homegrown
        // recon-blending `temporal_filter::temporal_filter` that used to sit
        // here behind a literal `false` gate was removed on 2026-09-25.
        let w = self.width as usize;
        let h = self.height as usize;
        let n = w * h;
        // The frame's chroma format drives EVERY chroma-geometry derivation
        // below — `ChromaFormat::{chroma_width,chroma_height}` are the C
        // `>> subsampling` rules generalized, never a local `/2`.
        let fmt = self
            .chroma_format
            .unwrap_or(svtav1_types::chroma::ChromaFormat::Yuv420);
        let (ss_x, ss_y) = (fmt.subsampling_x() as usize, fmt.subsampling_y() as usize);
        // Aligned-extent chroma dims — the internal planes' stride/height.
        let (acw, ach) = (fmt.chroma_width(w), fmt.chroma_height(h));
        // Non-4:2:0 formats ship only where the generalized path is proven.
        // 4:4:4 is decoder-verified on the still arm at sb64 AND on the
        // non-funnel inter arm (motion-compensated chroma prediction +
        // residual, no `uv_mode` — the funnel stays 4:2:0-only, so inter
        // decisions come from `RefFrameCtx`'s luma-ME + explicit chroma
        // MC). sb128's interleaved multi-cell chroma walk, 10-bit 444
        // planes, IntraBC-on-444 and chroma superres remain unported — a
        // 444 stream outside the envelope takes the honest refusal.
        if fmt == svtav1_types::chroma::ChromaFormat::Yuv444
            && (self.sb_size != 64 || self.bit_depth != 8 || self.superres_denom.is_some())
        {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "ChromaFormat::Yuv444 is decoder-verified only for 8-bit \
                 frames at sb_size 64 without superres: the sb128 multi-cell \
                 chroma walk, 10-bit 444 planes, IntraBC chroma prediction \
                 and chroma superres are not yet ported (C itself refuses \
                 444 at verify_settings, enc_settings.c:470 — no byte oracle)",
            )));
        }
        // 4:4:4 staged bring-up: the chroma loop-filter KERNELS are still
        // 4:2:0-shaped, so at 444 every chroma filter arm is skipped and the
        // header signals chroma off (lf levels 0 / CDEF uv strengths 0 /
        // LR planes RESTORE_NONE) — decoder-consistent by construction.
        // `filter_chroma` is the apply-side "chroma arm runs" flag; at
        // 4:2:0 it is exactly `chroma.is_some()`.
        let filter_chroma = chroma.is_some() && fmt == svtav1_types::chroma::ChromaFormat::Yuv420;
        let encode_input = gather_rows(y_plane, y_stride, w, h)?;

        // Task #95 chunk 2 — partial-SB variance source. `compute_b64_variance`
        // walks a full 64x64 grid per b64, so on a partial SB (aligned dims not
        // a multiple of 64) it reads PAST the aligned extent into C's replicated
        // border (`pad_input_picture` + `svt_aom_generate_padding` net content =
        // the TRUE edge pixel, docs/arbitrary-dims-port-map.md). Build a source
        // buffer padded out to the SB extent and read the PD0 partition /
        // variance source from it. For a 64-aligned frame the extent equals the
        // aligned extent, so no padding is needed and `encode_input` is used
        // directly at stride `w` — fully byte-neutral for every full-SB cell.
        let grain_denoised = self.film_grain.denoise_apply
            && self.prepared_grain.as_ref().is_some_and(|p| p.apply_grain);
        let dims95 = if grain_denoised {
            crate::frame_geom::FrameDims::new(w, h)
        } else {
            crate::frame_geom::FrameDims::new(self.true_width as usize, self.true_height as usize)
        };
        let sb95 = 64usize;
        let ext_w = w.div_ceil(sb95) * sb95;
        let ext_h = h.div_ceil(sb95) * sb95;
        let sb_input_owned: Option<alloc::vec::Vec<u8>> = if ext_w == w && ext_h == h {
            None
        } else {
            let mut buf = svtav1_types::try_vec![0u8; ext_w * ext_h]?;
            for r in 0..h {
                buf[r * ext_w..r * ext_w + w].copy_from_slice(&encode_input[r * w..r * w + w]);
            }
            crate::frame_geom::pad_input_plane(&mut buf, &dims95, sb95);
            Some(buf)
        };
        let sb_input: &[u8] = sb_input_owned.as_deref().unwrap_or(&encode_input);
        let in_stride = if sb_input_owned.is_some() { ext_w } else { w };
        let sb_chroma_owned = build_sb_chroma(chroma, fmt, acw, ach, ext_w, ext_h)?;
        let hbd_sb_owned = build_hbd_sb(
            &hbd_source,
            w,
            h,
            fmt,
            acw,
            ach,
            ext_w,
            ext_h,
            &sb_input_owned,
        )?;

        // Screen-content derivation (allintra): scm 3 auto-detect at
        // preset <= 7 (enc_handle.c:4514-4527), off at M8+; palette level
        // + FH allow_screen_content_tools from sc_class5
        // (enc_mode_config.c:2374-2393). Runs on the SOURCE luma (C
        // pcs->enhanced_pic) before everything downstream: the flag gates
        // the per-block no-palette flag coding in the tile pack, the MD
        // rates (via the tile driver's own identical derivation), and the
        // FH bits.
        // Superres: screen-content detection runs on the CODED (downscaled)
        // picture, NOT the full-resolution one — unlike `pic_avg_variance`.
        // C's picture-decision process resizes at pd_process.c:4344 and only
        // then detects (`svt_aom_is_screen_content_antialiasing_aware`,
        // pd_process.c:4787), so the detector sees the scaled picture.
        // MEASURED: running it on the full-res source instead diverges from C
        // at preset 7 — the only allintra preset where scm-3 auto-detection is
        // live (enc_handle.c:4514-4527; M8+ has it off) — on the superres cell
        // gradient 64x64 q32 d10.
        // C derives the allintra screen-content mode from the preset
        // (scm 3 at <= M7, off at M8+, enc_handle.c:4641-4651) UNLESS the
        // config forces it — tune IQ sets `screen_content_mode = 3`
        // regardless of preset (enc_handle.c:4914). Model the force by
        // running the detector at a preset it is live for.
        let sc_preset = match self.hdr.screen_content_mode {
            Some(3) => self.speed_config.preset.min(7),
            // ZenEnhancement::AomScreenTools: run detection at every preset
            // (same min-7 clamp as an SCM-3 force) — C's own gate stops
            // running the detector entirely above M7 on the allintra arm.
            _ if self
                .enhancements
                .contains(crate::enhancements::ZenEnhancement::AomScreenTools) =>
            {
                self.speed_config.preset.min(7)
            }
            _ => self.speed_config.preset,
        };
        let mut sc_derivation = self.derive_screen_content(sc_arm, w, h, &encode_input, sc_preset);
        // ZenEnhancement::AomScreenTools: libaom keeps palette and IntraBC
        // ENABLED whenever the detector says screen — the allintra ladders
        // switch them off by preset (palette at M8+, IntraBC at M5+). When
        // the detector fires on the allintra arm AND the ladder left BOTH
        // tools dead, the arm substitutes the VIDEO ladder's levels for the
        // same preset index (clamped at its last nonzero row: palette holds
        // M10's, IntraBC M9's). The fill is deliberately all-or-nothing:
        // the measured wins are the M8+ zone where C signals no screen
        // tools at all, while the M5-M7 partial zone (palette live, only
        // IntraBC dead) regressed on the imazen-26 frontier cells
        // (+1.95% mean BD at p6 — IBC-5 costs on content palette-7 already
        // handles). A level the allintra arm already set is never lowered.
        if sc_arm == crate::sc_detect::ScArm::Allintra
            && self
                .enhancements
                .contains(crate::enhancements::ZenEnhancement::AomScreenTools)
            && sc_derivation.classes.sc_class5
            && sc_derivation.palette_level == 0
            && sc_derivation.intrabc_level == 0
        {
            use crate::port_enc_mode_config::multi_processes::{
                intrabc_level_default, palette_level_default,
            };
            let preset = self.speed_config.preset;
            sc_derivation.palette_level = palette_level_default(preset.min(10), true, true);
            sc_derivation.intrabc_level = intrabc_level_default(preset.min(9), true, true, true);
            sc_derivation.allow_intrabc =
                crate::intrabc::IbcCtrls::for_level(sc_derivation.intrabc_level).enabled;
            sc_derivation.allow_screen_content_tools =
                sc_derivation.palette_level != 0 || sc_derivation.allow_intrabc;
        }
        // 4:4:4 staged bring-up: IntraBC chroma prediction is not ported
        // (the IBC search is funnel-only and the funnel is 4:2:0-gated), so
        // the frame must NOT advertise the tool — `allow_intrabc=1` would
        // also suppress the LF/CDEF/LR FH param blocks (spec 5.9.11/19/20)
        // that this stream still needs to signal.
        if fmt == svtav1_types::chroma::ChromaFormat::Yuv444 {
            sc_derivation.allow_intrabc = false;
        }
        // Hoisted out of the walk: `&self` is borrowed across the pack loop, and
        // this is a frame-constant. One value for the header writer and the
        // walk (see `EntropyCtx::tx_mode_select`).
        let frame_tx_mode_select = self.frame_tx_mode_select(temporal_layer);

        // Step 3c: the CLI-domain qp `svt_av1_rc_calc_qindex_crf_cqp`
        // reads (`scs_qp`, rc_crf_cqp.c:471-473 — no `startup_qp_offset`
        // in this port's envelope). C's `aq_mode` does not shift this
        // value: `aq-mode` is TPL's enable, and TPL's output lands in the
        // QINDEX domain below (`crf_qindex_calc` / `sb_qp_derivation_tpl_
        // la`), never back on `static_config.qp`.
        //
        // There USED to be a homegrown VAQ + "TPL" qp shift here behind
        // `aq_mode != 0` — a port of nothing, kept only while `aq_mode !=
        // 0` was refused outright. The real TPL path replaces it.
        #[allow(unused_mut)]
        let mut tpl_adjusted_qp = pcs.qp;

        // --- The INTER branch of MODE DECISION (docs/INTER-ENCODE-PLAN.md
        // §1s items 1b/2/3/6). `None` on a key frame, which is what keeps the
        // whole still envelope byte-identical by construction.
        //
        // The open-loop search runs against the PREVIOUS FRAME'S SOURCE, not
        // the DPB recon — SVT's ME is open loop (`me_process.c:185-203` reads
        // the PA reference, `reference_object.c:242-250`). The recon side is
        // `ref_padded_luma`, which the motion COMPENSATION indexes.
        //
        // The PA picture is built only in VIDEO mode: on a still/AVIF encode
        // nothing can ever reference this frame, and the pyramid is a padded
        // copy plus two decimations of the whole luma plane — real work to
        // spend on a buffer with no reader.
        let mut pa_cur = (self.gop.intra_period != 1).then(|| match self.pa_scratch.take() {
            // Recycle the frame-before-last's pyramid. `refill_from_source`
            // rewrites every byte and every descriptor field, so this is
            // byte-identical to the fresh allocation it replaces.
            Some(mut recycled) => {
                recycled.refill_from_source(&encode_input, w, w, h, display_order);
                recycled
            }
            None => alloc::boxed::Box::new(crate::inter_me_arm::PaPicture::from_source(
                &encode_input,
                w,
                w,
                h,
                display_order,
            )),
        });
        // C `pa_ref_obj->avg_luma = input_pcs->avg_luma`
        // (`pic_analysis_process.c:2003`) — stamped onto the reference object
        // a later picture's `get_similar_ref_brightness` reads through
        // `pa_slots`. `INVALID_LUMA` whenever `calc_hist` was off, because
        // the picture decision's `avg_luma` was already gated there.
        if let Some(pa) = pa_cur.as_mut() {
            pa.avg_luma = pic_decision
                .as_ref()
                .map_or(crate::port_picstruct::INVALID_LUMA, |p| p.avg_luma);
        }
        // The TPL stage already ran this picture's open-loop ME against the
        // same reference pyramids with the same `FrameMeParams` — C's
        // `pa_me_data->me_results`, shared between `tpl_mc_flow` and the
        // picture's own encode. Reuse it verbatim; a member the stage
        // skipped (I slice, or a reference pyramid it could not resolve)
        // falls through to the sequential computation, whose `None`/`Some`
        // answer is then identical.
        let frame_me = self.resolve_frame_me(
            display_order,
            is_key,
            sc_arm,
            &pic_decision,
            &mut tpl_in,
            frame_hier,
            w,
            h,
            sc_derivation,
            &pa_cur,
        );

        // THE single CLI-qp -> qindex conversion (C: quantizer_to_qindex
        // lookup on picture_qp, rc_crf_cqp.c). Everything above this line
        // (assign_picture_qp, VAQ, TPL) works in the CLI 0..63 domain where
        // those deltas were calibrated — one CLI step maps to ~4 qindex
        // steps through the table. Everything below (quantizer step
        // tables, CDF q bucket, EC base_q_idx, chroma quantization,
        // deblock level picker, FH base_q_idx) consumes ONLY this qindex.
        // Lambda is the documented exception: it stays CLI-qp-calibrated
        // (see qp_to_lambda) until C's lambda_rate_tables.h port lands.
        // Issue #9 item 4 (fractional CRF): the quarter-step remainder rides
        // in as a qindex offset exactly where C adds it (rc_crf_cqp.c:471).
        //
        // C then keeps TWO qp values and they are NOT interchangeable once the
        // offset is non-zero:
        //   * `scs->static_config.qp` — the CLI value, UNCHANGED by the
        //     offset. Every qp-keyed LEVEL derivation reads it:
        //     `svt_aom_get_nsq_search_level_allintra` (enc_mode_config.c:10014),
        //     the qp-based-threshold scaling (:338), the coeff-level complexity
        //     (md_config_process.c:620/651), the max-can-bsize picks
        //     (enc_dec_process.c:1645/1723/2393), the IntraBC mesh scaling
        //     (pd_process.c:3740) — all `static_config.qp`.
        //   * `ppcs->picture_qp = clamp_qp((base_q_idx + 2) >> 2)`
        //     (rc_process.c:861) — re-derived FROM the offset qindex, and read
        //     only by the frame `lambda_weight` ladder
        //     (enc_mode_config.c:10093-10108, both the tune-IQ curve and the
        //     PSNR 0/150/175 tiers).
        // `tpl_adjusted_qp` is this port's `static_config.qp` analogue, so it
        // stays in the CLI domain and `picture_qp` is derived alongside it.
        // MEASURED: collapsing both onto the qindex-derived value (the first
        // cut of this change) diverged from C at preset 2 / qp 20 /
        // offsets 2-3 — exactly the offsets where `(80+off+2)>>2` rolls from
        // 20 to 21 — as `tools/issue9_knobs_gate.sh` cells
        // `crf20.2-gradient-128-p2` / `crf20.3-...` (port 2664 B vs C 2628 B,
        // first divergence FH `loop_filter_level[0]` C=4 Rust=5). With
        // offset 0 the two values are equal, so every pre-existing cell is
        // byte-identical either way.
        // BITRATE RC dispatch — C's `rc_cfg.mode != AOM_Q` fork
        // (rc_process.c:850-859): a VBR/CBR frame's qindex comes from
        // `svt_av1_rc_process_rate_allocation` +
        // `svt_av1_rc_calc_qindex_rate_control`, NEVER from the
        // `svt_av1_rc_calc_qindex_crf_cqp` chain below — the `!allintra`
        // block is skipped alongside it. `cbr_frame_rc` carries the PPCS
        // RC fields forward to the packetization-feedback update at the
        // end of this function; `cbr_sb_plan` is cyclic refresh's per-SB
        // map (`None` = C's flat arm). Only `RcMode::Cbr` reaches this:
        // VBR is refused at `rate_control_config_error` (first-pass
        // statistics are not ported), and LD+CBR is C's own envelope.
        let mut cbr_frame_rc: Option<crate::port_rc_vbr_cbr_state::FrameRc> = None;
        let mut cbr_sb_plan: Option<crate::sb_qindex::SbQindexPlan> = None;
        let mut base_qindex = self.derive_base_qindex(
            display_order,
            is_key,
            &pic_decision,
            frame_hier,
            sc_derivation,
            tpl_adjusted_qp,
            &frame_me,
            &mut cbr_frame_rc,
            &mut cbr_sb_plan,
        )?;
        // VIDEO-MODE QP SCALING (inter campaign C1a). C's `cqp_qindex_calc`
        // (rc_crf_cqp.c:393, the mainline `#else` arm) returns the qindex
        // untouched when `scs->allintra` — the early return the entire still
        // envelope takes — and scales it otherwise. `allintra` here is the
        // still predicate the rest of this function already uses.
        //
        // MEASURED, gradient 64x64 in video mode (SVT_AVIF=0): C writes
        // base_q_idx 67 at cli qp40 where the still path writes 160, because a
        // video key frame is coded far finer than a still — later frames
        // reference it. The derivation is tier-1 verified against C's exported
        // `svt_av1_convert_qindex_to_q` and `svt_av1_compute_qdelta`, and
        // against the base_q_idx C actually writes on four cells.
        //
        // `is_ref`/`idr_flag` are true for the key frame this reaches today;
        // the non-base temporal-layer arm needs a DPB the port does not have,
        // and `cqp_qindex_calc` documents that it must not be used there yet.
        let allintra = self.gop.intra_period == 1;
        // C `svt_av1_rc_calc_qindex_crf_cqp`'s dispatch
        // (rc_crf_cqp.c:481-489): `enable_qp_scaling_flag` is `!allintra`
        // (enc_handle.c:4390) with `use_fixed_qindex_offsets == 1` as the
        // one subtract — that config is refused upstream — and inside it
        // `tpl_ctrls.enable` selects `crf_qindex_calc` over
        // `cqp_qindex_calc`. `tpl_r0` carries `ppcs->r0` through the
        // in-place adjustments `crf_qindex_calc` makes, because
        // `sb_qp_derivation_tpl_la` gates on the POST-adjust value
        // (rc_aq.c:899).
        let tpl_fti = tpl_in.as_ref().filter(|f| f.tpl_ctrls.enable != 0);
        // `R0Flags` is Copy — hoist the four capability flags so `tpl_in`'s
        // `frame_me` can be taken by the ME site below while the flags stay
        // readable for the lambda context and the ref-object stamp.
        let tpl_flags = tpl_fti.map(|f| f.flags);
        let r0_delta_qp_md = tpl_flags.is_some_and(|f| f.r0_delta_qp_md);
        let mut tpl_r0 = tpl_fti.map_or(0.0, |f| f.r0);
        // CBR never enters the CQP/CRF dispatch — its qindex is the one
        // `rc_calc_qindex_rate_control` already produced above (C's own
        // `mode != AOM_Q` fork, rc_process.c:853-859).
        self.apply_tpl_qp(
            is_key,
            &pic_decision,
            temporal_layer,
            frame_hier,
            md_ref_intra_percentage,
            sc_derivation,
            &mut base_qindex,
            allintra,
            tpl_fti,
            &mut tpl_r0,
        );
        let mut picture_qp = crate::rate_control::picture_qp_from_qindex(base_qindex);
        if crate::dbgenv::qtrace() {
            eprintln!(
                "QTRACE display={display_order} base_qindex={base_qindex} temporal_layer={temporal_layer} hier={frame_hier} is_ref={:?} update_type={:?}",
                pic_decision.as_ref().map(|p| p.is_ref),
                pic_decision.as_ref().map(|p| p.update_type),
            );
        }
        // C's EXTENDED-CRF lambda bump (enc_mode_config.c:10109-10114): for
        // CRF 63.25..70 only — `static_config.qp == MAX_QP_VALUE (63)` with a
        // non-zero `extended_crf_qindex_offset` — the frame `lambda_weight`
        // gains `offset * 28`. This is the ONLY effect the offset has at qp 63:
        // the qindex itself saturates (`quantizer_to_qindex[63] == 255`, and
        // `clamp_qindex` caps at the max-qp qindex), so `(MAXQ - new_qindex) *
        // offset / 56` in rc_crf_cqp.c:511 evaluates to 0 there. 0 on every
        // other config, hence byte-inert everywhere else.
        let lw_bump: u32 = if self.rc_config.qp == 63 {
            u32::from(self.rc_config.extended_crf_qindex_offset) * 28
        } else {
            0
        };
        // [SVT_HDR_MODE] fork Variance Boost: derive the per-SB qindex plan
        // (sb_qindex.rs = C variance_adjust_qp(readjust=true) chain). The
        // recentered base REPLACES base_qindex BEFORE every downstream
        // consumer (lambda, CDF bucket, deblock, FH) — C order: rc_aq runs
        // in rc_init_sb_qindex ahead of MD. picture_qp follows C's
        // (base+2)>>2 update.
        let mut sb_plan = self.plan_sb_qindex(
            w,
            h,
            &encode_input,
            sb_input,
            in_stride,
            &mut tpl_adjusted_qp,
            cbr_sb_plan,
            &mut base_qindex,
            &mut picture_qp,
        )?;

        // C `svt_av1_rc_init_sb_qindex`'s TPL arm (rc_aq.c:897-901): under
        // `aq_mode == 2 && tpl_ctrls.enable && ppcs->r0 != 0` — the r0 the
        // qindex dispatch just adjusted, not the stage's raw one — each
        // superblock takes `get_deltaq_offset(beta)` on top of whatever
        // qindex map is live (the frame base, or the variance plan's), and
        // `sb_setup_lambda` folds the SB's rdmult ratio into
        // `tpl_sb_rdmult_scaling_factors`, the grid `blk_lambda_tuning`
        // reads per block. `delta_q_present` flips ONLY on
        // `r0_delta_qp_quant` (rc_aq.c:790-791); the SB loop itself needs
        // `r0_delta_qp_md && tpl_is_valid` (:799). `r0_delta_qp_md` without
        // `r0_delta_qp_quant` still quantizes at the per-SB qindex while
        // signalling nothing — C's own encoder/decoder disagreement,
        // reproduced rather than "fixed".
        let mut delta_q_present = sb_plan.is_some();
        let mut tpl_rdmult: Option<alloc::sync::Arc<crate::port_md_lambda::TplRdmult>> = None;
        if let Some(fti) = tpl_fti
            && self.rc_config.aq_mode == 2
            && tpl_r0 != 0.0
        {
            if fti.flags.r0_delta_qp_quant {
                delta_q_present = true;
            }
            if fti.flags.r0_delta_qp_md && fti.tpl_is_valid {
                let sb_cols = w.div_ceil(self.sb_size);
                let sb_rows = h.div_ceil(self.sb_size);
                let sb_cnt = sb_cols * sb_rows;
                debug_assert_eq!(fti.tpl_beta.len(), sb_cnt);
                // The SB map starts from whatever `rc_init_sb_qindex`
                // left: the variance plan's post-normalization values, else
                // the frame base on every SB.
                let mut sb_qindex: Vec<u8> = sb_plan
                    .as_ref()
                    .map_or_else(|| alloc::vec![base_qindex; sb_cnt], |p| p.sb_qindex.clone());
                crate::sb_qindex::sb_qp_derivation_tpl_la(
                    self.bit_depth,
                    is_key,
                    &fti.tpl_beta,
                    &mut sb_qindex,
                );
                // `generate_b64_me_qindex_map` (rc_process.c:747) feeds
                // `svt_aom_get_me_qindex`, `sb_setup_lambda`'s `me_qindex`
                // input. Under `r0_delta_qp_md` `update_lambda`'s arm reads
                // `q_index` so the value is inert INSIDE this pass — but
                // the same map is live for the frame's per-SB MD lambdas
                // below, which is why it is built here and not skipped.
                let b64_me_qindex = fti.frame_me.as_ref().map(|me| {
                    let mev: Vec<u32> = me.per_b64.iter().map(|o| o.me_8x8_cost_variance).collect();
                    crate::port_rc_process::generate_b64_me_qindex_map(
                        &mev,
                        i32::from(base_qindex),
                        is_key,
                    )
                });
                let lctx = crate::port_rc_process::LambdaContext {
                    frame_type: i32::from(!is_key),
                    temporal_layer_index: temporal_layer,
                    hierarchical_levels: frame_hier,
                    update_type: md_lambda_base_update_type
                        .unwrap_or(crate::port_rc_process::FrameUpdateType::KfUpdate),
                    alt_lambda_factors: md_alt_lambda_factors,
                    rtc: false,
                    stats_based_sb_lambda_modulation:
                        crate::port_rc_process::stats_based_sb_lambda_modulation(
                            crate::rate_arm::eff_enc_mode(sc_arm, self.speed_config.preset),
                            false,
                        ),
                    base_q_idx: i32::from(base_qindex),
                    delta_q_present,
                    r0_delta_qp_md: true,
                    // `scs->static_config.lambda_scale_factors` — the port
                    // does not expose the knob; 128 is C's identity.
                    lambda_scale_factors: [128; 7],
                };
                // `pcs->hbd_md` (enc_mode_config.c:2151-2164): bd10 &&
                // preset <= M5 selects the 10-bit MD quantizer for the
                // `compute_rd_mult` inside `sb_setup_lambda`; 8 elsewhere.
                let hbd_md_depth = match self.bit_depth {
                    10 if self.speed_config.preset <= 5 => 10,
                    _ => 8,
                };
                let mi_rows = (h >> 2) as i32;
                let mut sb_factors = fti.tpl_rdmult_scaling_factors.clone();
                for sb_row in 0..sb_rows {
                    for sb_col in 0..sb_cols {
                        crate::stop_check(&stop)?;
                        let sb_idx = sb_row * sb_cols + sb_col;
                        let me_q = b64_me_qindex.as_ref().map_or(base_qindex, |m| {
                            crate::port_md_rate_estimation::get_me_qindex(
                                m,
                                u16::try_from(w).unwrap_or(u16::MAX),
                                u16::try_from(h).unwrap_or(u16::MAX),
                                u32::try_from(sb_idx).unwrap_or(u32::MAX),
                                u32::try_from(sb_col * self.sb_size).unwrap_or(u32::MAX),
                                u32::try_from(sb_row * self.sb_size).unwrap_or(u32::MAX),
                                self.sb_size == 128,
                            )
                        });
                        crate::sb_qindex::sb_setup_lambda(
                            u32::try_from(sb_col * self.sb_size).unwrap_or(u32::MAX),
                            u32::try_from(sb_row * self.sb_size).unwrap_or(u32::MAX),
                            self.superres_denom.unwrap_or(8),
                            u32::try_from(w).unwrap_or(u32::MAX),
                            self.sb_size == 128,
                            fti.synth_blk_size,
                            mi_rows,
                            base_qindex,
                            sb_qindex[sb_idx],
                            me_q,
                            &lctx,
                            hbd_md_depth,
                            &fti.tpl_rdmult_scaling_factors,
                            &mut sb_factors,
                        );
                    }
                }
                #[cfg(feature = "std")]
                if std::env::var_os("SVTAV1_TPLQP").is_some() {
                    let poc = pic_decision
                        .as_ref()
                        .map_or(u32::MAX, |p| p.picture_number as u32);
                    let res_dbg = sb_plan.as_ref().map_or_else(
                        || crate::sb_qindex::delta_q_res_for(self.rc_config.qp, false),
                        |p| p.delta_q_res,
                    );
                    std::eprintln!(
                        "TPLQP pic={poc} dq={} res={} qmd={} qquant={} valid={} n_sb={}",
                        u8::from(delta_q_present),
                        res_dbg,
                        u8::from(fti.flags.r0_delta_qp_md),
                        u8::from(fti.flags.r0_delta_qp_quant),
                        u8::from(fti.tpl_is_valid),
                        sb_cnt
                    );
                    for (i, (&q, &b)) in sb_qindex.iter().zip(fti.tpl_beta.iter()).enumerate() {
                        std::eprintln!("SBQP pic={poc} sb={i} qindex={q} beta={b:.10}");
                    }
                    for (i, (&bse, &sbf)) in fti
                        .tpl_rdmult_scaling_factors
                        .iter()
                        .zip(sb_factors.iter())
                        .enumerate()
                    {
                        std::eprintln!("SBF pic={poc} i={i} base={bse:.10} sb={sbf:.10}");
                    }
                }
                // `reset_enc_dec`'s picture lambdas
                // (enc_dec_process.c:176-187): `svt_aom_lambda_assign` at
                // the FH base qindex, `multiply_lambda=true`, at both MD
                // depths. These are the bases `svt_aom_set_tuned_blk_lambda`
                // scales per block — NO `lambda_weight`, no per-SB stats
                // modulation (its `q_index == base_q_idx` makes the qdiff
                // factor the identity under every arm).
                let (pic_fast8, pic_full8) =
                    crate::port_rc_process::lambda_assign(&lctx, 8, base_qindex, true);
                let (_pic_fast10, pic_full10) =
                    crate::port_rc_process::lambda_assign(&lctx, 10, base_qindex, true);
                tpl_rdmult = Some(alloc::sync::Arc::new(crate::port_md_lambda::TplRdmult {
                    factors: sb_factors,
                    mi_rows,
                    unscaled_width: w as i32,
                    superres_denom: i32::from(self.superres_denom.unwrap_or(8)),
                    synth_blk_32: fti.synth_blk_size == 32,
                    sb_size_is_128: self.sb_size == 128,
                    pic_full8,
                    pic_full10,
                    pic_fast8,
                }));
                // C's ONE normalize call in `generate_sb_qindex`
                // (rc_process.c:741-744) sees the map AFTER TPL modified
                // it. The variance arm normalized inside its own port, so
                // applying TPL on top undoes the residue snap — a live
                // `delta_q_res != 1` re-normalizes here. On the
                // variance+TPL combination this sits within one res step
                // of C's var(raw) -> tpl -> normalize order; the measured
                // parity envelope is variance-off cells.
                let res = sb_plan.as_ref().map_or_else(
                    || crate::sb_qindex::delta_q_res_for(self.rc_config.qp, false),
                    |p| p.delta_q_res,
                );
                if delta_q_present && res != 1 {
                    let mut map_i32: Vec<i32> = sb_qindex.iter().map(|&q| i32::from(q)).collect();
                    crate::sb_qindex::normalize_sb_delta_q(base_qindex, res, &mut map_i32);
                    sb_qindex = map_i32.iter().map(|&q| q.clamp(0, 255) as u8).collect();
                }
                match sb_plan.as_mut() {
                    Some(p) => p.sb_qindex = sb_qindex,
                    None => {
                        sb_plan = Some(crate::sb_qindex::SbQindexPlan {
                            base_qindex,
                            sb_qindex,
                            // `get_delta_q_res(qp, enable_variance_boost =
                            // false)` is `DEFAULT_DELTA_Q_RES` — 1. This
                            // field only reaches the FH when
                            // `delta_q_present` (i.e. `r0_delta_qp_quant`).
                            delta_q_res: 1,
                        });
                    }
                }
            }
        }

        // Issue #5: `base_qindex == 0` signals CODED-LOSSLESS in the frame
        // header (spec 5.9.2 — with the zero chroma deltas and no
        // segmentation of this port's mainline path, base_q_idx 0 IS
        // CodedLossless), and the whole encode follows C's lossless rules:
        // the header writes no deblock/CDEF/LR/tx_mode bits (chunk 1,
        // 2026-08-27), every block is 4x4 or 8x8 coded at TX_4X4 with the
        // Walsh-Hadamard transform, no tx_size / tx_type symbols, RDOQ and
        // the tx-type search off, only DCT-chroma candidates injected, and no
        // in-loop filter runs (chunk 2, this arm's consumers below +
        // leaf_funnel). The envelope that is byte-verified against the C
        // oracle is `lossless_config_error`'s complement; everything outside
        // it is REFUSED rather than encoded wrong (the pre-chunk-2 measurement
        // of what "encoded wrong" looked like: ssim2 -200..-1100 vs source).
        let coded_lossless = base_qindex == 0;
        if coded_lossless && let Some(why) = self.lossless_config_error(is_key) {
            return Err(whereat::at!(crate::EncodeError::UnsupportedConfig(why)));
        }
        // The fork's tune-SSIM parallel full cost (`FunnelFrame::tune_ssim`,
        // armed by `alt_ssim_tuning`) has no INTER skip arm in the port
        // (leaf_funnel/mds3.rs asserts it away). Refuse the inter frame instead
        // of reaching that assert. MEASURED 2026-09-25 on i265: identity_run
        // `gradient 128 128 40 5` with SVT_HDR_MODE=1 SVT_FORK_ALT_SSIM_TUNING=1
        // SVTAV1_FRAMES=4 panicked in release on an inter frame.
        if !is_key && self.hdr.is_fork() && self.hdr.alt_ssim_tuning {
            return Err(whereat::at!(crate::EncodeError::UnsupportedConfig(
                "alt_ssim_tuning on inter frames is not ported (the tune-SSIM full cost has no \
                 inter skip arm) [C: accepts]"
            )));
        }
        // The header omits delta-q at base_q_idx 0. C's full_loop.c selects
        // the frame qindex whenever delta_q_present is false, even if the
        // variance planner produced positive per-SB indices. Preserve that
        // planner, but only feed a signaled plan to MD, quantization and pack.
        //
        // TWO plans, C's split: `delta_q_plan` is the SIGNAL side — what the
        // frame header's `delta_q_present` and the per-SB delta-q symbols
        // carry — while `md_sb_qindex` is `pcs->sb_ptr_array[..].qindex` at
        // the end of `generate_sb_qindex`, the map MD and quantization
        // consume (`ctx->qp_index = delta_q_present || r0_delta_qp_md ?
        // sb_qp : base_q_idx`, md_process.c:800-803). They differ exactly
        // when `r0_delta_qp_md` is on without `r0_delta_qp_quant`: C
        // quantizes per-SB and signals nothing.
        let delta_q_plan = sb_plan
            .as_ref()
            .filter(|_| delta_q_present && !coded_lossless);
        let md_sb_qindex = sb_plan.as_ref().filter(|_| !coded_lossless);

        // C-exact coding quantizer for the still/PD1 path (quant.rs): the
        // frame-level rdoq_level from `derive_intra_coeff_level`
        // (pic_avg_variance = mean of the per-B64 64x64 variances,
        // pic_analysis_process.c:608, truncated to u16) via the allintra
        // policy, the KF full lambda, and the default-CDF coefficient cost
        // tables. Only key/still frames at presets >= 4 (the PD0
        // fixed-tree paths: eff-M9 above 8, PD0_LVL_1 at 4..8 — the C
        // rdoq policy line `<=M5 -> 1, else f(coeff_lvl)` covers both,
        // enc_mode_config.c:14931) on 64-aligned dims — everywhere else
        // the legacy dead-zone quantizer stays.
        // MOVED UP with the inter MD derivations below (§1s item 8): MODE
        // DECISION prices against `md_frame_context`, which C copies from
        // this same reference (md_config_process.c:299-310) — so the binding
        // has to exist before MD runs, not only before the entropy walk.
        // CDF CONTINUATION, RESTORE side — C `reset_entropy_coding_picture`
        // (`ec_process.c:101-112`):
        //
        //     if (primary_ref_frame != PRIMARY_REF_NONE)
        //         svt_memcpy(ec->fc, &ref->frame_context, sizeof(FRAME_CONTEXT));
        //     else
        //         svt_aom_reset_entropy_coder(...);
        //
        // The DPB slot is the one the FRAME HEADER names:
        // `ref_frame_idx[primary_ref_frame]`, i.e. `rps.ref_dpb_index[]`, which
        // is what a DECODER resolves. (C indexes its own
        // `ref_pic_ptr_array[list][idx]` via `get_list_idx`/`get_ref_frame_idx`
        // instead; the two agree, and the spec mapping is the one conformance
        // depends on, so that is the one used here.)
        //
        // `binding` is recomputed at the header-assembly site below from the
        // same pure inputs; `primary_ref_frame_for_cdf` is carried down so the
        // two are ASSERTED equal rather than assumed — a tile coded against
        // slot A while the header announces slot B is a decoder desync, and it
        // is exactly the kind of divergence no byte count would explain.
        let (primary_ref_frame_for_cdf, primary_ref_cdfs) =
            match self.resolve_primary_ref_cdfs(is_key, &pic_decision, base_qindex) {
                Ok(value) => value,
                Err(value) => return value,
            };

        // GLOBAL MOTION, decided per FRAME (see `gm_level_for_frame` /
        // `gm_search_config_error`). It lives here because C decides it here too:
        // `me_process.c:264-272` calls `svt_aom_global_motion_estimation` the
        // moment the last b64's ME lands, and every input it reads
        // (`rc_me_distortion`, `rc_me_allow_gm`) is an ME output.
        //
        // `input_width`/`input_height` are C's `pcs->enhanced_pic`
        // (`me_process.c:136`) — the SOURCE picture — so the TRUE dims, not
        // the SB-aligned pair `frame_me` was run over. On a 64-aligned cell
        // the two agree; on a partial-SB cell they do not, and the integer
        // divide is what the whole decision turns on.
        let gm_estimation = self.estimate_gm(is_key, &pic_decision, temporal_layer, &frame_me);
        // C's SEARCH, when the derivation above says C would run one
        // (`global_me.c:190-300`). `None` when it would not — every reference
        // then keeps the IDENTITY model C initialised, and no correspondence
        // set, RANSAC fit or warp is computed, exactly as in C.
        //
        // The SOURCE plane is C's `input_pic` (`pcs->enhanced_pic`,
        // `me_process.c:136`) and the REFERENCE is the PA reference's
        // `input_padded_pic` — the same pyramid ME searched. Both are read
        // through clamped addressing (`port_warp`'s `sample_x.clamp(0, width -
        // 1)`) and from the picture ORIGIN, so a tightly-packed plane gives the
        // same answer as C's bordered one; the strides differ and the pixels do
        // not.
        let gm_models = self.resolve_gm_models(
            display_order,
            is_key,
            decided_is_some,
            &pic_decision,
            temporal_layer,
            w,
            h,
            &encode_input,
            &frame_me,
            gm_estimation,
        );
        // Printed BEFORE the refusal below, deliberately: the frame whose
        // derivation a join gate most needs to see is exactly the one the
        // refusal stops (`tools/gm_join_gate.sh`).
        dump_gm(display_order, &frame_me, gm_estimation, gm_models);

        if let Some(why) = Self::gm_search_config_error(gm_estimation.as_ref(), gm_models.as_ref())
        {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(why)));
        }
        let mut c_quant = self.build_coding_quant(
            &stale_vars,
            is_key,
            sc_arm,
            md_lambda_base_update_type,
            md_lambda_factor_update_type,
            md_alt_lambda_factors,
            lambda_mod_intra,
            w,
            h,
            filter_chroma,
            sb_input,
            in_stride,
            tpl_adjusted_qp,
            &frame_me,
            base_qindex,
            picture_qp,
            lw_bump,
            coded_lossless,
        );

        // Step 4: Encode the frame superblock-by-superblock in raster order.
        // This ensures each SB can read above/left neighbors from previously
        // reconstructed SBs, matching the AV1 decode order.
        // (Spec 00: "The main encoding loop processes SBs in raster order")
        let mut recon = svtav1_types::try_vec![128u8; n]?;
        // AV1 spec: use_128x128_superblock=0 in SH → sb_size=64.
        // The decoder always uses 64x64 SBs when this flag is 0.
        // The encoder's max_partition_depth controls how deep the
        // partition search goes WITHIN each 64x64 SB, not the SB size.
        // SUPERBLOCK SIZE (task #91). Derived once in `EncodePipeline::new`
        // by replaying C's rule (Globals/enc_handle.c:4071-4111) — see the
        // `sb_size` field. 64 for every cell the gates currently cover; 128
        // only once the SB128 encode path is capability-enabled, at which
        // point the seq header's `use_128x128_superblock` and the tile
        // limits follow it (both parameterized below).
        let sb_size = self.sb_size;
        // Lambda stays CLI-qp-calibrated (see qp_to_lambda's domain note);
        // tpl_adjusted_qp is the CLI-domain value base_qindex is derived
        // from, so this is qp_to_lambda(qindex_to_qp(base_qindex)).
        let lambda = (crate::rate_control::qp_to_lambda(tpl_adjusted_qp)
            * self.speed_config.lambda_scale()) as u64;

        let sb_cols = w.div_ceil(sb_size);
        let sb_rows = h.div_ceil(sb_size);

        // C `svt_aom_get_ref_pic_buffer(pcs, LAST_FRAME)` resolves LAST
        // through `pcs->ppcs->ref_pic_ptr_array[REF_LIST_0][0]`, i.e. this
        // picture's own RPS — `pic.rps.ref_dpb_index[LAST]`.
        //
        // The three sites below read `self.dpb.get(0)` until 2026-09-03, a
        // hard-coded DPB SLOT. That is right for every frame this port's
        // gates cover and WRONG the moment a second inter frame exists:
        // frame 1 refreshes slot 1 (`refresh_frame_mask` 0x02), so at poc 2
        // C's LAST is slot 1 = poc 1 while `get(0)` is still the KEY frame.
        // MEASURED on `gradient 64x64 q32 p8 frames=3` before this: the
        // port's frame-2 MD searched `mv=(2,-36)` against poc 0, where the
        // true poc-1 displacement is `(0,-24)`, and coded 100 % intra at
        // 466 B against C's 21. That is the SEVENTH "a caller passes a
        // constant where the derivation is already ported" finding of this
        // campaign, and it is invisible at two frames because slot 0 IS
        // LAST there.
        let last_ref_slot: Option<usize> = if is_key {
            None
        } else {
            pic_decision
                .as_ref()
                .map(|pic| pic.rps.ref_dpb_index[crate::port_picstruct::LAST] as usize)
                .or(Some(0))
        };
        // Get reference frame for inter prediction (if available). This is
        // the DPB slot's own shared handle, NOT a copy: until 2026-09-04 the
        // two bindings below deep-cloned `y_plane` and `padded` on every
        // inter frame (4.19 + 7.15 MB at 2048x2048), which was the whole of
        // `4e29d8fa7`'s +8.83 MB peak-heap step on both ISAs
        // (benchmarks/mem_refclone_2026-09-04.meta). The slot is read-only
        // once stored and is not refreshed until the end of this frame, so
        // the shared allocation reads the bytes the clone read.
        let last_ref: Option<alloc::sync::Arc<crate::picture::ReferenceFrame>> =
            last_ref_slot.and_then(|slot| self.dpb.get_shared(slot));
        let ref_frame_data: Option<&[u8]> = last_ref.as_ref().map(|rf| rf.y_plane.as_slice());

        // The padded twin of the reference plane above (C
        // `pad_ref_and_set_flags`, enc_dec_process.c:1072) — what INTER
        // PREDICTION indexes, because a legal MV reads outside the frame.
        let ref_padded_luma: Option<&crate::picture::PaddedRef> =
            last_ref.as_ref().and_then(|rf| rf.padded.as_deref());

        // MV map for spatial MV prediction (8x8 block grid)
        let mv_map_stride = w.div_ceil(8);
        let mv_map_size = mv_map_stride * h.div_ceil(8);
        // Read by the legacy (non-funnel) partition search's MV predictor and
        // always ZERO: an old post-encode per-SB full-pel search filled it
        // after the only reader had run, so it was removed (2026-09-25).
        let mv_map = svtav1_types::try_vec![svtav1_types::motion::Mv::ZERO; mv_map_size]?;

        // Task #86: real tile ROWS for the allintra KEY path. Per AV1 spec
        // a tile is prediction-independent — above/left neighbor context
        // (and the entropy coder + FrameContext) resets at every tile
        // boundary — so per-tile-row MD search with its own local recon
        // (the `encode_tile_rows` closure below) and per-tile-row entropy
        // walks (see `run_entropy_walk` further down: it loops tile rows
        // internally, resetting writer/frame_ctx/coeff_fc/ectx per tile)
        // are exactly what a conforming decoder expects — NOT a
        // continuity break. `tile_rows_log2` is resolved (clamped) the
        // same way C's `svt_aom_set_tile_info` clamps a nonsense request
        // (entropy_coding.c:2450-2579): out-of-range requests degrade to
        // the largest the frame supports rather than panicking or
        // producing a bitstream inconsistent with what was encoded.
        //
        // Task #96: the grid is resolved through `TileGrid::resolve`, the
        // shared port of C's get_tile_limits + calculate_tile_cols +
        // calculate_tile_rows. The load-bearing part is that
        // `grid.tile_rows` is the ACTUAL tile count, which C's algorithm
        // makes SMALLER than `1 << TileRowsLog2` whenever the SB-row
        // count is not a multiple of it (6 SB rows at log2=2 -> height 2
        // -> 3 tiles, not 4). Deriving the count as `1 << log2` instead
        // both encoded a trailing EMPTY tile and wrote an out-of-range
        // `context_update_tile_id`, which conforming decoders REJECT
        // ("Invalid context_update_tile"). See TileGrid's doc comment.
        let tile_grid = crate::entropy::obu::TileGrid::resolve(
            self.width,
            self.height,
            // Task #91: the tile limits are SB-derived (spec 5.9.15) —
            // max_tile_width_sb HALVES and max_tile_area_sb QUARTERS at
            // SB128 (C svt_av1_get_tile_limits shifts by the PIXEL
            // sb_size_log2). Identical to the old 64 constant whenever
            // sb_size == 64, i.e. for every currently gated cell.
            self.sb_size as u32,
            self.tile_rows_log2,
            self.tile_cols_log2,
        );
        let tile_rows_log2 = tile_grid.tile_rows_log2;
        let tile_cols_log2 = tile_grid.tile_cols_log2;

        // [SVT_HDR_MODE] fork chroma-q: derive the FH per-plane deltas and
        // the plane qindexes the quantizer must use. Mainline: all zero.
        let chroma_deltas = if self.hdr.is_fork() {
            crate::chroma_q::fork_chroma_q_deltas_tuned(
                base_qindex,
                &self.color_description,
                self.hdr.tune,
            )
        } else {
            // MAINLINE chroma-q (rc_crf_cqp.c's `#else` arm): all-zero at
            // every tune but IQ, where C boosts chroma by
            // `CLIP3(0, 16, new_qindex/2 - 14)`. This used to be hardcoded
            // to zero, which made tune IQ 0/6 byte-identical to the C oracle
            // on `tools/issue9_knobs_gate.sh` — the ONLY divergence, with the
            // tile payload already matching byte-for-byte in size.
            crate::chroma_q::mainline_chroma_q_deltas(base_qindex, self.hdr.tune)
        };
        // `__expert` override REPLACES the derivation (mono frames were
        // refused above, so this only ever reaches frames with chroma).
        #[cfg(feature = "__expert")]
        let chroma_deltas = self
            .chroma_q_override
            .map_or(chroma_deltas, crate::chroma_q::ChromaQOverride::deltas);
        let qindex_u = (i32::from(base_qindex) + i32::from(chroma_deltas.u_ac)).clamp(0, 255) as u8;
        let qindex_v = (i32::from(base_qindex) + i32::from(chroma_deltas.v_ac)).clamp(0, 255) as u8;
        // Stills are I-slices at temporal layer 0: effective = ac_bias * 0.3.
        let ac_bias_eff = svtav1_dsp::ac_bias::effective_ac_bias(self.hdr.ac_bias, true, 0);
        // [SVT_HDR_MODE] per-SB delta-q signaling (variance boost). This
        // chunk arms the FULL SYNTAX chain with a UNIFORM plan (every SB at
        // base qindex -> all delta symbols are 0): decoder-valid, exercises
        // FH delta_q_params + the per-SB delta_q_cdf symbols end to end.
        // The variance plan (sb_qindex::variance_adjust_qp) swaps in when
        // per-SB quantization threading lands (docs/HDR-ON-4.2.md).
        // Diagnostic: SVTAV1_VB_DUMP=<path> writes the per-SB qindex plan to a
        // FILE (never stderr — the identity harness parses this process's
        // stderr as its symbol trace). Answers "did the boost fire, and by how
        // much" without perturbing a byte-comparison run.
        #[cfg(feature = "std")]
        if let Ok(path) = std::env::var("SVTAV1_VB_DUMP") {
            let txt = match delta_q_plan {
                Some(p) => std::format!(
                    "base={base_qindex} res={} plan={:?}\n",
                    p.delta_q_res,
                    p.sb_qindex
                ),
                None => std::format!("base={base_qindex} plan=NONE (variance boost off)\n"),
            };
            let _ = std::fs::write(path, txt);
        }
        let delta_q_res_signal = delta_q_plan.map(|p| p.delta_q_res);
        // sharp-tx RDOQ activates only with per-SB delta-q present (C gate
        // `(use_sharpness || sharp_tx) && delta_q_present && plane==0`).
        // [SVT_HDR_MODE] tune SSIM/IQ/MS_SSIM: per-16x16 SSIM rdmult
        // scaling factors (aom_av1_set_mb_ssim_rdmult_scaling; the
        // alt_ssim_tuning multi-scale perceptual variant when that knob is
        // on) + the PICTURE lambdas C's per-block `aom_av1_set_ssim_rdmult`
        // scales them into `full_lambda_md`/`fast_lambda_md`
        // (mode_decision.c:4060-4110). Those bases are `reset_enc_dec`'s
        // `svt_aom_lambda_assign` outputs (enc_dec_process.c:176-187) —
        // NO `lambda_weight` and NO per-SB stats/qindex modulation; the
        // scale replaces the `av1_lambda_assign_md` result outright.
        let ssim_rdmult = self.derive_ssim_rdmult(
            is_key,
            sc_arm,
            temporal_layer,
            frame_hier,
            md_lambda_base_update_type,
            md_alt_lambda_factors,
            w,
            h,
            &encode_input,
            base_qindex,
            r0_delta_qp_md,
            delta_q_present,
        );
        // Per-tune LF sharpness (deblocking_filter.c:1120): this is live
        // in mainline and the HDR fork. KEY VQ/FILM_GRAIN adds 2 (max 7);
        // IQ/MS_SSIM caps by qindex on every frame type.
        // Applied to the SEARCH input, the SIGNALED bits, and the walk's
        // application consistently (one effective value).
        let lf_sharp_eff: u8 = {
            let base = self.hdr.sharpness.clamp(0, 7) as u8;
            if !is_key
                && matches!(
                    self.hdr.tune,
                    crate::tune::TUNE_VQ | crate::tune::TUNE_FILM_GRAIN
                )
            {
                base
            } else {
                crate::tune::lf_sharpness_for_tune(base, self.hdr.tune, base_qindex)
            }
        };
        let sharp_tx_active =
            self.hdr.is_fork() && self.hdr.sharp_tx == 1 && delta_q_plan.is_some();
        let qm_levels = self.derive_qm_levels(base_qindex, coded_lossless, chroma_deltas);
        // [SVT_HDR_MODE] photon-noise film grain (--noise*): synthesize
        // the table per frame; seed 7391 + 3381*frame (C resource_
        // coordination assign_film_grain_random_seed; zero is bumped).
        let film_grain = if let Some(mut fg) = self.film_grain.table.clone() {
            fg.apply_grain = true;
            fg.random_seed = self.film_grain_seed();
            Some(fg)
        } else if self.hdr.is_fork() && self.hdr.noise_strength > 0 {
            let mut fg = crate::noise_gen::generate_noise_table(
                self.width,
                self.height,
                u32::from(self.hdr.noise_strength),
                self.hdr.noise_strength_chroma,
                self.hdr.noise_chroma_from_luma as i8,
                self.hdr.noise_size,
                self.color_description.full_range,
            );
            fg.random_seed = self.film_grain_seed();
            Some(fg)
        } else {
            self.prepared_grain.take()
        };
        // Stamp the fork RDOQ knobs onto the encode-pass quant config (C
        // reads them off static_config inside svt_av1_optimize_txb; the
        // sharp-tx gate `(use_sharpness||sharp_tx) && delta_q_present &&
        // plane==0` is unconditional for sharp_tx=1, full_loop.c:1070-1078).
        if self.hdr.is_fork()
            && let Some(cq) = c_quant.as_mut()
        {
            let cfg = alloc::sync::Arc::get_mut(cq)
                .expect("c_quant is unshared before tile encoding starts");
            cfg.hdr_fork = true;
            cfg.sharpness = self.hdr.sharpness;
            cfg.noise_norm_strength = self.hdr.noise_norm_strength;
            cfg.sharp_tx_active = sharp_tx_active;
            cfg.qm_levels = qm_levels;
        }
        // MOVED UP (inter campaign, docs/INTER-ENCODE-PLAN.md §1s item 1b):
        // these three derivations used to sit beside the header assembly.
        // MODE DECISION now needs them — the inter branch of MD prices with
        // the same tables the pack codes with, and reads the same MVP
        // environment — and MD runs before the header is written. Nothing
        // between their old and new positions produced any of their inputs
        // (checked field by field: every one is a `self` field or a local
        // from above this point), so the move is byte-neutral by
        // construction and pinned by the still gates.
        // Sequence-level tool bits (C svt_aom_sig_deriv_pre_analysis_scs):
        // per-preset for the still/allintra path, off for multi-frame.
        // Threaded to the SH + FH writers AND the entropy walk below —
        // the per-block use_filter_intra symbol exists exactly when the
        // SH signals the tool, so all three consumers MUST see one value.
        let is_single_frame = self.gop.intra_period == 1;
        let seq_tools = self.derive_seq_tools(zen_intra_edge_filter, is_single_frame);

        // The picture-level MD inputs, BOUND rather than passed inline because
        // TWO derivations read them: `md_config_inputs` ->
        // `sig_deriv_mode_decision_config_default` (the tool ladders) and
        // `enc_dec_cand_reduction` (C's `svt_aom_sig_deriv_enc_dec_default`
        // half, which the inter candidate injector reads). Binding them means
        // the two cannot drift apart on an input.
        //
        // Only computed for an inter frame — a key frame's header carries none
        // of these fields, so both derivations are byte-inert for every still
        // cell.
        let pipeline_md_inputs = self.build_md_inputs(
            is_key,
            sc_arm,
            &pic_decision,
            temporal_layer,
            frame_hier,
            sc_derivation,
            base_qindex,
            picture_qp,
            &c_quant,
            seq_tools,
        );
        // C `svt_aom_sig_deriv_mode_decision_config_default` — the picture-level
        // tool ladders. It is EXPORTED and gated at tier 1; the frame header
        // reads its `allow_high_precision_mv`, `allow_warped_motion`,
        // `is_motion_mode_switchable`, `mfmv_level` and `interpolation_filter`
        // so the header can never disagree with the tools the encode ran.
        let md_config_signals = pipeline_md_inputs
            .clone()
            .and_then(crate::inter_hdr_arm::md_config_inputs)
            .and_then(
                crate::port_enc_mode_config::md_config::sig_deriv_mode_decision_config_default,
            );
        // C `set_global_motion_field` (md_config_process.c:37): the frame's
        // `global_motion[LAST..ALTREF]`, built from the search's own verdict.
        // It lives HERE, after the mode-decision signal derivation, because the
        // TRANSLATION arm reads `frm_hdr.allow_high_precision_mv` — which C
        // assigns in `svt_aom_sig_deriv_mode_decision_config`, i.e. AFTER the
        // ME-time search that used a hardcoded 0 (see the search call above).
        //
        // IDENTITY throughout on a key frame and wherever the search declined,
        // which is exactly what the header wrote unconditionally before.
        let gm_field: [svtav1_types::motion::WarpedMotionParams; 8] =
            match (gm_models.as_ref(), md_config_signals.as_ref()) {
                (Some(m), Some(sigs)) => crate::port_global_me::set_global_motion_field(
                    m,
                    sigs.allow_high_precision_mv != 0,
                ),
                _ => [svtav1_types::motion::WarpedMotionParams::default(); 8],
            };
        // C `pcs->ppcs->gm_ctrls` — `svt_aom_set_gm_controls` at this frame's
        // `gm_level`. `skip_identity` (level 4 only) makes the injector skip
        // IDENTITY references; `enabled` is what C assigns to
        // `ctx->global_mv_injection`, gating `inject_global_candidates`
        // outright at every preset where the level is 0.
        let gm_ctrls = crate::port_enc_mode_config::ctrls::set_gm_controls(
            self.gm_level_for_frame(is_key),
            crate::port_enc_mode_config::ResolutionRange::from_luma_area(self.width * self.height),
        );
        let gm_skip_identity = gm_ctrls.is_some_and(|c| c.skip_identity != 0);
        let gm_enabled = gm_ctrls.is_some_and(|c| c.enabled != 0);
        let ref_gm_field = self.derive_ref_gm_field(&pic_decision, primary_ref_frame_for_cdf);

        // C `svt_aom_sig_deriv_enc_dec_default` (`enc_mode_config.c:7826`) —
        // the per-superblock EncDec derivation, of which the injector reads
        // `cand_reduction_ctrls`. It is derived here beside the picture-level
        // ladders because every input it takes is picture-level: C calls it
        // per superblock but passes `pcs->cand_reduction_level` and, on this
        // arm, constants for everything else (see `enc_dec_cand_reduction`).
        //
        // A `None` here is REFUSED rather than folded into the inter path's
        // `None`: `inter_md_frame` being absent on an inter frame does not
        // refuse anything, it codes the frame all-intra — a plausible-but-wrong
        // stream, which is the one outcome `docs/WORKING-ON-THIS.md` §6 rules
        // out. `set_cand_reduction_ctrls` answers `None` only for a level
        // outside C's switch, which this arm cannot produce (it assigns 0, 1,
        // 2 or 6), so this is a guard on a claim about the ladder rather than
        // a reachable path.
        let inter_cand_reduction = match (pipeline_md_inputs.as_ref(), md_config_signals.as_ref()) {
            (Some(mi), Some(sigs)) => Some(
                crate::inter_hdr_arm::enc_dec_cand_reduction(mi, sigs.cand_reduction_level)
                    .ok_or_else(|| {
                        whereat::at!(EncodeError::UnsupportedConfig(
                            "cand_reduction_level is outside C's set_cand_reduction_ctrls \
                             switch (crate::inter_hdr_arm::enc_dec_cand_reduction)",
                        ))
                    })?,
            ),
            _ => None,
        };

        // The frame-level INTER syntax the pack's inter mode-info writer
        // reads (`docs/INTER-ENCODE-PLAN.md` §1s item 7). It is derived HERE,
        // beside `primary_ref_frame_for_cdf`, rather than at the header
        // assembly below, because the TILE is coded before the header is
        // written and the writer needs these values while it codes. The
        // header re-derives the same fields from the same inputs and the two
        // are asserted equal there, exactly like `primary_ref_frame`.
        //
        // `md_config_signals` moved up with it for the same reason; nothing
        // between its old and new position reads it.
        let inter_syntax_state = self.build_inter_syntax_state(
            display_order,
            &pic_decision,
            seq_tools,
            md_config_signals,
            gm_field,
        );

        // The frame-constant MVP environment the pack derives `predmv` /
        // `inter_mode_ctx` / `drl_ctx` from (§1s items 2 and 3). Same
        // provenance rule as `inter_syntax_state` above: every field is the
        // one the HEADER announces, so the contexts the tile codes are the
        // ones a decoder rebuilds.
        //
        // C `av1_setup_motion_field` (md_config_process.c:523, called from
        // `:933`) builds BOTH the temporal field and `pcs->ref_frame_side`
        // from the DPB, once per picture. Its two products go to different
        // consumers — `tpl_mvs` to the MVP scan below, `ref_frame_side` to the
        // walk's `av1_copy_frame_mvs` — so it is computed once here and both
        // are carried, rather than derived twice.
        let mut inter_ref_frame_side = [0i8; 8];
        let mut inter_mvp_env = self.build_inter_mvp_env(
            display_order,
            &pic_decision,
            w,
            h,
            sb_size,
            gm_field,
            &inter_syntax_state,
            &mut inter_ref_frame_side,
        );

        // C `ctx->ref_frame_type_arr` (`set_all_ref_frame_type`,
        // pd_process.c:1044) — single-reference entries AND the compound
        // pairs (bi-dir L0xL1 plus the unidir LAST_LAST2 set a B slice
        // earns), each naming the DPB pictures it predicts from. The
        // constituent-aware availability filter runs below, once
        // `inter_padded_by_ref` exists; a compound entry survives exactly
        // when BOTH its single references do.
        let inter_ref_types: alloc::vec::Vec<i8> = match pic_decision.as_ref() {
            Some(pic) if !is_key => {
                let (arr, tot) = crate::port_picstruct::set_all_ref_frame_type(pic);
                arr[..usize::from(tot)].to_vec()
            }
            _ => alloc::vec::Vec::new(),
        };
        // The padded DPB picture per `MvReferenceFrame`. C's
        // `svt_aom_get_ref_pic_buffer(pcs, rf)` resolves EACH reference through
        // its own `ref_dpb_index` entry — LAST through `[0]`, LAST2 through
        // `[1]`, BWDREF through `[4]`. On a flat low-delay-P GOP those slots
        // hold DIFFERENT pictures from frame 2 on (LAST is frame N-1, LAST2
        // frame N-2), and binding every reference to LAST's recon — which is
        // what this did — both starved LAST2 out of `inter_ref_types` and
        // would have predicted it from the wrong pixels had it survived.
        //
        // The `Arc` clones exist for borrow shape only: the `&PaddedRef`
        // table cannot borrow `self.dpb` directly because the encode below
        // mutates other `self` fields while the references are live. Eight
        // refcount bumps per inter frame is the whole cost; the buffers are
        // shared.
        let mut inter_ref_frames: [Option<alloc::sync::Arc<crate::picture::ReferenceFrame>>; 8] =
            Default::default();
        if let Some(pic) = pic_decision.as_ref()
            && !is_key
        {
            for rt in 1i8..=7 {
                let slot = pic.rps.ref_dpb_index[usize::from(rt as u8 - 1)] as usize;
                inter_ref_frames[rt as usize] = self.dpb.get_shared(slot);
            }
        }
        let mut inter_padded_by_ref: [Option<&crate::picture::PaddedRef>; 8] = [None; 8];
        for (rt, rf) in inter_ref_frames.iter().enumerate() {
            inter_padded_by_ref[rt] = rf.as_deref().and_then(|r| r.padded.as_deref());
        }
        // A single entry survives when ITS reference has a padded
        // reconstruction; a compound pair survives when BOTH constituents
        // do. `padded_by_ref` is indexed by `MvReferenceFrame` (1..=7) — a
        // pair type (8..) has no slot of its own and must never be indexed
        // into it.
        let inter_ref_types: alloc::vec::Vec<i8> = inter_ref_types
            .into_iter()
            .filter(|&rt| {
                let rf = crate::inter_mvp::av1_set_ref_frame(rt);
                inter_padded_by_ref[rf[0].max(0) as usize].is_some()
                    && (rf[1] == crate::inter_mvp::NONE_FRAME
                        || inter_padded_by_ref[rf[1].max(0) as usize].is_some())
            })
            .collect();
        // C's `symteric_refs` preconditions (adaptive_mv_pred.c:1339-1341):
        // a RA B picture above temporal layer 0. The LIST half of C's gate
        // ({LAST, BWDREF, LAST_BWD}) is evaluated per call inside
        // `generate_av1_mvp_table` on the block's own ref array —
        // `determine_best_references` may rebuild a block's
        // `ref_frame_type_arr` in an order the gate rejects even when the
        // picture-level list passes, so stamping the whole gate here is
        // wrong. `inter_mvp_fields` reads the same eligibility so the
        // coded stack matches MD's.
        if let (Some(env), Some(pic)) = (inter_mvp_env.as_mut(), pic_decision.as_ref()) {
            env.symmetric_refs_eligible = pic.temporal_layer_index > 0
                && self.pred_structure == crate::port_picstruct::PredStructure::RandomAccess;
        }
        #[cfg(feature = "std")]
        if crate::dbgenv::rpsdbg()
            && let Some(pic) = pic_decision.as_ref()
            && !is_key
        {
            let mut s = alloc::string::String::new();
            for i in 0..7usize {
                let slot = pic.rps.ref_dpb_index[i] as usize;
                let poc = self.dpb.get(slot).map_or(-1, |r| r.display_order as i64);
                s.push_str(&alloc::format!("{i}:{slot}->poc{poc} "));
            }
            std::eprintln!(
                "RPSDBG poc={} tl={} is_ref={} totrf={} rt={:?} refresh={:#x} dpb=[{}]",
                pic.picture_number,
                pic.temporal_layer_index,
                pic.is_ref,
                inter_ref_types.len(),
                inter_ref_types,
                pic.rps.refresh_frame_mask,
                s
            );
        }

        let inter_md_frame = Self::build_inter_md_frame(
            self.speed_config.preset,
            self.bit_depth,
            self.rc_config.qp,
            self.mrp_ctrls.use_best_references,
            self.hdr.tx_bias,
            self.hdr.tune,
            self.hdr.alt_ssim_tuning,
            self.hdr.ac_bias,
            &pic_decision,
            md_lambda_base_update_type,
            md_lambda_factor_update_type,
            md_alt_lambda_factors,
            lambda_mod_intra,
            w,
            h,
            sb_input,
            in_stride,
            sc_derivation,
            &frame_me,
            base_qindex,
            picture_qp,
            delta_q_plan,
            &primary_ref_cdfs,
            sb_size,
            ref_padded_luma,
            md_config_signals,
            gm_field,
            gm_skip_identity,
            gm_enabled,
            inter_cand_reduction,
            &inter_syntax_state,
            &inter_mvp_env,
            inter_padded_by_ref,
            &inter_ref_types,
        )?;

        // C `av1_lambda_assign_md` (md_process.c:725) run PER SUPERBLOCK, as
        // `svt_aom_mode_decision_configure_sb` (md_process.c:796) calls it —
        // `ctx->me_q_index = svt_aom_get_me_qindex(pcs, sb_ptr, ..)`
        // (enc_dec_process.c:2926) is a per-SB input, so `full_lambda_md[0]`
        // and `fast_lambda_md[0]` are per-SB even with no delta-q signalled.
        //
        // The chain, all already ported and tier-tested:
        //   `me_8x8_cost_variance[b64]`
        //     -> `port_rc_process::generate_b64_me_qindex_map` (rc_aq.c:656)
        //     -> `port_md_rate_estimation::get_me_qindex` (md_rate_estimation.c:1084)
        //     -> `update_lambda`'s `stats_based_sb_lambda_modulation` factor
        //        (rc_process.c:437-446), which is `me_q_index - base_q_idx`.
        //
        // MEASURED against C's own `SVT_PD0CFG_OUT` on `diag 72x72 q40 p6`
        // frame 1: `fastlam` 5182 / 5182 / 5182 / 7773 across the four
        // superblocks of ONE frame, where the port reported a flat 6633.
        //
        // BYTE-INERT ON A KEY FRAME BY CONSTRUCTION: the map's I-slice arm
        // writes `base_q_idx` into every entry, so `qdiff` is 0 and the
        // factor is the identity 128 — and this whole binding is `None`
        // unless `inter_md_frame` is `Some`, which is exactly "non-key with a
        // DPB reference".
        let sb_inter_lambda = self.derive_sb_inter_lambda(
            &stop,
            sc_arm,
            &pic_decision,
            temporal_layer,
            frame_hier,
            md_lambda_factor_update_type,
            md_alt_lambda_factors,
            lambda_mod_intra,
            w,
            h,
            &frame_me,
            base_qindex,
            r0_delta_qp_md,
            picture_qp,
            lw_bump,
            delta_q_present,
            md_sb_qindex,
            sb_size,
            sb_cols,
            sb_rows,
            &inter_md_frame,
        )?;

        // C `set_blocks_to_be_tested`'s per-SB `min_sq_size`
        // (enc_dec_process.c:1485), which `depth_removal_ctrls` decides.
        // Frame-level here because every input is: the level is a picture
        // signal, the ME distortions are `frame_me`'s per-b64 outputs, and the
        // reference's `sb_min_sq_size` is a DPB field. `None` on a key frame,
        // where `set_depth_removal_level_controls` returns `enabled = 0`
        // outright — which is why the still path has never carried this and is
        // byte-neutral by construction.
        //
        // MEASURED against C's own resolved controls (`SVT_PD0CFG_OUT`'s `dr`
        // field, added for this chunk) on `diag 64x64 q40 p8 frames=2`:
        // frame 0 `dr=0/0/0/0` and frame 1 `dr=1/0/1/1` — enabled, and
        // `disallow_below_32x32` set, so C's PD0 may not test below 32x32
        // there while `Pd0Ctx::min_sq` was a flat 8.
        // The per-SB `DepthRemovalResult` alongside it — `sig_deriv_enc_dec_pd0`'s
        // `subres_level` ladder (enc_mode_config.c:7344-7352) reads the
        // `depth_removal_ctrls` flags and the post-call `disallow_4x4`, which
        // the min_sq fold alone would discard.
        let mut pd0_dr_res: Option<Vec<crate::port_enc_mode_config::common::DepthRemovalResult>> =
            None;
        let pd0_min_sq = self.derive_pd0_min_sq(
            display_order,
            &stop,
            &pic_decision,
            w,
            h,
            &frame_me,
            base_qindex,
            r0_delta_qp_md,
            picture_qp,
            delta_q_present,
            md_sb_qindex,
            sb_size,
            sb_cols,
            sb_rows,
            last_ref_slot,
            md_config_signals,
            &inter_md_frame,
            &sb_inter_lambda,
            &mut pd0_dr_res,
        )?;

        // C `pd0_detector`'s reference-side inputs (enc_dec_process.c:
        // 2142-2168): `ref_pic_ptr_array[list][0]`'s `sb_intra`, admitted
        // only while all three of C's guards hold — the MRP-capped
        // `ref_list{0,1}_count_try`, `svt_aom_is_ref_same_size`
        // (enc_mode_config.c:2857; `is_not_scaled` short-circuits true, else
        // B-slice + non-null + matching dims), and the reference's
        // `tmp_layer_idx <= temporal_layer_index`. `None` per list is
        // exactly C's `l{0,1}_refs == 0` arm. List 0's slot-0 reference is
        // LAST (`ref_dpb_index[0]`); list 1's is BWD (`ref_dpb_index[4]`) —
        // the same binding `pipeline_md_inputs.ref_l{0,1}` makes.
        let pd0_ref_sb = |list1: bool| -> crate::part_arm::RefSbStats<'_> {
            let Some(pic) = pic_decision.as_ref() else {
                return Default::default();
            };
            let (count_try, rps_idx) = if list1 {
                (pic.ref_list1_count_try, crate::port_picstruct::BWD as usize)
            } else {
                (
                    pic.ref_list0_count_try,
                    crate::port_picstruct::LAST as usize,
                )
            };
            if count_try == 0 {
                return Default::default();
            }
            let Some(rf) = self.dpb.get(pic.rps.ref_dpb_index[rps_idx] as usize) else {
                return Default::default();
            };
            let same_size = self.superres_denom.is_none()
                || (rf.width == self.width as u32 && rf.height == self.height as u32);
            if !(same_size && rf.temporal_layer <= temporal_layer) {
                return Default::default();
            }
            crate::part_arm::RefSbStats {
                avail: true,
                sb_intra: (!rf.sb_intra.is_empty()).then_some(rf.sb_intra.as_slice()),
                sb_skip: (!rf.sb_skip.is_empty()).then_some(rf.sb_skip.as_slice()),
                me_64x64_dist: (!rf.sb_me_64x64_dist.is_empty())
                    .then_some(rf.sb_me_64x64_dist.as_slice()),
                me_8x8_cost_var: (!rf.sb_me_8x8_cost_var.is_empty())
                    .then_some(rf.sb_me_8x8_cost_var.as_slice()),
                mvp_64x64: (!rf.sb_64x64_mvp.is_empty()).then_some(rf.sb_64x64_mvp.as_slice()),
                is_islice: rf.is_islice,
            }
        };
        let pd0_det_frame = crate::part_arm::Pd0DetFrame {
            transition_present: pic_decision
                .as_ref()
                .is_some_and(|p| p.transition_present != 0),
            // `!frame_is_leaf(ppcs)` (enc_mode_config.h:113) — a KEY frame's
            // `KF_UPDATE` is not `LF_UPDATE`, so this is true on frame 0 and
            // false on a flat-GOP inter frame.
            is_not_last_layer: pic_decision
                .as_ref()
                .is_none_or(|p| p.update_type != crate::port_picstruct::FrameUpdateType::Lf),
            // The same value `pipeline_md_inputs` recomputes for
            // `PipelineMdInputs`-keyed consumers — hoisted to frame level for
            // the LAMBDA_MOD_INTRA arm above.
            ref_intra_percentage: md_ref_intra_percentage,
            l0: pd0_ref_sb(false),
            l1: pd0_ref_sb(true),
        };
        // The FRAME-level half of `resolve_sb_lpd1` — the `md_encode_block`
        // Light-PD1 dispatch's inputs that are constant across the picture's
        // superblocks. `None` on a key frame (`md_config_signals` absent) and
        // on every allintra cell, which is what keeps the still envelope
        // byte-neutral by construction.
        #[cfg(feature = "std")]
        if crate::dbgenv::lpd1dbg() {
            eprintln!(
                "RLPD1-FRAME tl={temporal_layer} key={is_key} md_cfg={:?} pmd={}",
                md_config_signals.as_ref().map(|m| m.pic_lpd1_lvl),
                pipeline_md_inputs.is_some(),
            );
        }
        let lpd1_frame = self.build_lpd1_frame(
            sc_arm,
            &pic_decision,
            &pipeline_md_inputs,
            md_config_signals,
        );

        let ref_min_max_sq = self.derive_ref_min_max_sq(&pic_decision, &last_ref);

        let (
            all_trees,
            sb_min_sq_sizes,
            sb_max_sq_sizes,
            cw,
            ext_cbuf,
            lr_true_w,
            lr_true_h,
            frame_coded_area,
            walk_end_cdfs,
        ) = Self::decide_and_encode_tiles(
            &self.hdr,
            &self.speed_config,
            self.bit_depth,
            self.true_width,
            self.true_height,
            self.thread_count,
            self.reference,
            self.enhancements,
            self.chroma_format,
            &mut self.last_recon10_final,
            &mut self.last_recon10_y,
            &mut self.last_recon10_uv,
            chroma,
            zen_intra_edge_filter,
            &hbd_source,
            stale_vars,
            &mut hbd_used,
            hbd_used_flag,
            &stop,
            is_key,
            sc_arm,
            temporal_layer,
            frame_hier,
            md_lambda_base_update_type,
            md_lambda_factor_update_type,
            md_alt_lambda_factors,
            lambda_mod_intra,
            w,
            h,
            fmt,
            ss_x,
            ss_y,
            acw,
            ach,
            filter_chroma,
            &encode_input,
            sb_input,
            in_stride,
            &sb_chroma_owned,
            hbd_sb_owned,
            sc_derivation,
            frame_tx_mode_select,
            tpl_adjusted_qp,
            base_qindex,
            picture_qp,
            lw_bump,
            tpl_rdmult,
            coded_lossless,
            delta_q_plan,
            md_sb_qindex,
            &primary_ref_cdfs,
            &c_quant,
            &mut recon,
            sb_size,
            lambda,
            sb_cols,
            sb_rows,
            ref_frame_data,
            ref_padded_luma,
            mv_map_stride,
            mv_map,
            tile_grid,
            chroma_deltas,
            qindex_u,
            qindex_v,
            ac_bias_eff,
            ssim_rdmult,
            sharp_tx_active,
            qm_levels,
            seq_tools,
            &inter_syntax_state,
            &inter_mvp_env,
            inter_md_frame,
            sb_inter_lambda,
            pd0_dr_res,
            pd0_min_sq,
            pd0_det_frame,
            lpd1_frame,
            ref_min_max_sq,
        )?;
        self.filter_and_pack_frame(
            chroma,
            display_order,
            hbd_source,
            hbd_used,
            stop,
            is_key,
            sc_arm,
            pic_decision,
            temporal_layer,
            frame_hier,
            pcs,
            w,
            h,
            n,
            fmt,
            ss_x,
            ss_y,
            acw,
            ach,
            filter_chroma,
            encode_input,
            sb_chroma_owned,
            sc_derivation,
            frame_tx_mode_select,
            pa_cur,
            frame_me,
            cbr_frame_rc,
            base_qindex,
            tpl_r0,
            coded_lossless,
            delta_q_plan,
            md_sb_qindex,
            primary_ref_frame_for_cdf,
            primary_ref_cdfs,
            gm_models,
            c_quant,
            recon,
            sb_size,
            sb_cols,
            sb_rows,
            ref_padded_luma,
            tile_grid,
            tile_rows_log2,
            tile_cols_log2,
            chroma_deltas,
            qindex_u,
            qindex_v,
            delta_q_res_signal,
            lf_sharp_eff,
            qm_levels,
            film_grain,
            is_single_frame,
            seq_tools,
            pipeline_md_inputs,
            md_config_signals,
            gm_field,
            ref_gm_field,
            inter_syntax_state,
            inter_ref_frame_side,
            inter_mvp_env,
            all_trees,
            sb_min_sq_sizes,
            sb_max_sq_sizes,
            cw,
            ext_cbuf,
            lr_true_w,
            lr_true_h,
            frame_coded_area,
            walk_end_cdfs,
        )
    }

}

#[cfg(test)]
mod tests;

/// The INTER TILE byte gate — inter campaign, the first byte-level evidence
/// about frame 1's tile.
///
/// # What is under test, and what is NOT
///
/// Under test: the CDF CONTINUATION restore ([`crate::port_frame_cdf`]) and
/// the inter mode-info writer
/// ([`crate::port_entropy_inter::block::write_inter_mode_info`]) together, end
/// to end, against C's actual tile bytes.
///
/// **NOT** under test: MODE DECISION. The block decision fed in below is C's
/// own, MEASURED off the reference encoder — not guessed from the bytes and
/// not produced by this port. That separation is the point: until the inter
/// branch of MD exists there is no other way to learn whether the entropy path
/// is right, and "three bytes came out equal after trying some motion vectors"
/// would be curve-fitting, not evidence.
///
/// # Where the decision came from
///
/// `tools/capture_c_trace/wrap_recon.c`'s `SVT_CINTER_OUT` dump, added with
/// this gate, prints the committed `BlockModeInfo` + `BlkStruct` fields that
/// `write_inter_mode_info` reads, from inside `svt_aom_update_mi_map`. On
/// `gradient 64x64 q40 p6 frames=2` (`tools/identity_diff_inter.sh`,
/// `SVTAV1_FRAME_SHIFT=3`) the committed 64x64 block is:
///
/// ```text
/// CINTER poc=1 mi=(0,0) bsize=12 part=0 mode=16 rf=1,-1 mv0=0,-24 pmv0=0,0
///        interp=0x0 mm=0 npr=0 ovl=0 imc=8 drl=0 drlctx=-1,-1 drlnear=0,0
///        iiu=0 skip=1 skipmode=0 cgi=0 cidx=0
/// ```
///
/// One 64x64 `PARTITION_NONE` block, `NEWMV` off `LAST_FRAME`, MV `(0, -24)`
/// eighth-pel — exactly the 3-pixel horizontal translation the harness
/// applies — predicted from `(0, 0)`, `EIGHTTAP_REGULAR`, `skip = 1`. The
/// whole frame.
///
/// That is independently corroborated by which CDFs C's tile ADAPTS: comparing
/// C's saved frame-0 and frame-1 contexts (the `SVT_FCTX_OUT` oracle) shows
/// exactly `partition`, `skip`, `intra_inter`, `comp_inter`, `single_ref`,
/// `newmv`, `switchable_interp`, `nmvc.joints` and `nmvc.comp1.*` moving — no
/// `refmv`, no `drl`, no `motion_mode`, no coefficient CDF, and only the
/// COLUMN component of the MV context. A dump and a completely different
/// measurement agreeing on the symbol set is what makes this a decision rather
/// than a fit.
#[cfg(test)]
mod inter_tile_byte_gate;

/// **How far does the PORT'S OWN decision get on the inter cell?**
///
/// `inter_tile_byte_gate` above proves the ENTROPY path by feeding C's
/// MEASURED block decision through the port's writers. That leaves one
/// question open, and it is the whole remaining campaign: does the port,
/// running its OWN ported machinery, arrive at that decision?
///
/// This module answers it field by field, for the two pieces that are
/// ported and gated but unwired — the MVP stack (`crate::inter_mvp`,
/// tier-1 against `svt_av1_find_best_ref_mvs_from_stack` /
/// `setup_ref_mv_list`) and the DRL/pred-MV chooser
/// (`crate::port_md::drl`, tier-1 against
/// `svt_aom_choose_best_av1_mv_pred`). It does NOT prove the pipeline
/// reaches them; `docs/INTER-ENCODE-PLAN.md` §1s item 1 is that wiring.
/// What it proves is that when they ARE reached with this cell's inputs,
/// they produce C's numbers — so a divergence found later is a WIRING
/// defect, not a translation one.
///
/// The C values it is checked against are the `SVT_CINTER_OUT` dump quoted
/// on `inter_tile_byte_gate`:
///
/// ```text
/// mode=16 rf=1,-1 mv0=0,-24 pmv0=0,0 imc=8 drl=0 drlctx=-1,-1 drlnear=0,0
/// ```
#[cfg(test)]
mod inter_decision_probe;

#[cfg(test)]
#[path = "film_grain_pipeline_tests.rs"]
mod film_grain_tests;

mod entropy_ctx;
pub(crate) use entropy_ctx::*;

mod lpd1;
pub(crate) use lpd1::*;

mod block_syntax;
use block_syntax::*;

mod partition_walk;
pub(crate) use partition_walk::*;

mod tile_walk;
use tile_walk::*;

mod ra;

mod tpl_stage;

mod setup;

mod entry;

mod grain;

mod config_check;

mod superres;

mod cbr;

mod frame_setup;

mod inter_setup;

mod restoration_stage;

mod frame_output;

mod md_setup;

mod loop_filters;

mod recon_output;

mod diagnostics;
use diagnostics::*;

mod walk_driver;

mod bd10_post;

mod inter_md_stage;

mod frame_prep;
use frame_prep::*;

mod state;
pub use state::*;

mod palette_cache;
pub(crate) use palette_cache::*;

mod pack_phase;

mod tile_phase;
