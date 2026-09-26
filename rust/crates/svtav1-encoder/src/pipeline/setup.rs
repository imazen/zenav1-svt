use super::*;

impl EncodePipeline {
    /// Create a new encoding pipeline.
    pub fn new(
        width: u32,
        height: u32,
        preset: u8,
        rc_config: RcConfig,
        hierarchical_levels: u8,
        intra_period: u32,
    ) -> Self {
        Self::new_with_preset(
            width,
            height,
            crate::speed_config::NativePreset::new(preset.min(13) as i8).unwrap(),
            rc_config,
            hierarchical_levels,
            intra_period,
        )
    }

    /// Construct with a checked native C preset, including research mode -1.
    ///
    /// Research-mode translation is in progress; accepting -1 is not a parity
    /// guarantee. See `docs/research-preset-port-map.md` for missing consumers.
    ///
    /// `hierarchical_levels` mirrors C's `cfg.hierarchical_levels`: an
    /// explicit level (the envelope C supports is 0-5, refused above by
    /// [`Self::gop_config_error`]) or
    /// [`crate::port_picstruct::HIERARCHICAL_LEVELS_AUTO`], the API default
    /// C ships — resolved in-library by
    /// [`Self::resolve_hierarchical_levels_auto`].
    pub fn new_with_preset(
        width: u32,
        height: u32,
        preset: crate::speed_config::NativePreset,
        rc_config: RcConfig,
        hierarchical_levels: u8,
        intra_period: u32,
    ) -> Self {
        let preset = preset.value();
        // C `hierarchical_levels == HIERARCHICAL_LEVELS_AUTO` survives as a
        // field until `enc_handle.c:4556` resolves it; here `gop`/`mg_map`
        // need a literal immediately, so the sentinel parks a provisional
        // flat structure until [`Self::resolve_hierarchical_levels_auto`]
        // swaps in the resolved one — before any consumer can read it.
        let hier_auto = hierarchical_levels == crate::port_picstruct::HIERARCHICAL_LEVELS_AUTO;
        let hierarchical_levels = if hier_auto { 0 } else { hierarchical_levels };
        // TWO boundary systems (frame_geom::FrameDims): the caller passes
        // TRUE dims; the encode runs on ALIGNED (8-rounded) dims. The
        // full-SB (aligned % 64 == 0) scope constraint is enforced on the
        // 4:2:0 pad path ([`Self::encode_frame_420`]) where it matters —
        // NOT here, so the monochrome state-tracking path keeps working at
        // its historical sub-64 dims (aligned == true, no padding).
        let dims = crate::frame_geom::FrameDims::new(width as usize, height as usize);
        // Task #91: replay C's `super_block_size` derivation
        // (Globals/enc_handle.c:4071-4111) on the ALIGNED dims — C
        // classifies resolution on `max_input_luma_width/height` AFTER the
        // 8-pad fold (enc_handle.c:3920, verified empirically). `allintra`
        // mirrors the identity oracle, which passes `avif = true`
        // (capture_c_trace.c) and therefore always lands in C's allintra
        // branch; the port's still/key pipeline is the same shape.
        let sb_inputs = crate::sb128_geom::SbSizeInputs {
            qp: rc_config.qp,
            allintra: intra_period == 1,
            ..Default::default()
        };
        let derived_sb = crate::sb128_geom::derive_super_block_size(
            dims.aligned_w,
            dims.aligned_h,
            preset as i8,
            &sb_inputs,
        );
        let (sb_size, sb128_fallback) = Self::resolve_sb_size(derived_sb, None, preset);
        Self {
            hdr: crate::hdr_mode::HdrForkConfig::default(),
            reference: crate::reference::SvtReference::Mainline420,
            enhancements: crate::enhancements::ZenEnhancements::default(),
            film_grain: Default::default(),
            #[cfg(feature = "__expert")]
            chroma_q_override: None,
            #[cfg(feature = "__expert")]
            sh_separate_uv_delta_q: None,
            prepared_grain: None,
            grain_references: core::array::from_fn(|_| None),
            grain_sequence_present: None,
            pd_ctx: crate::port_picstruct::PicDecisionCtx::new(),
            enc_pic: crate::port_picstruct::EncCtxPicParams::default(),
            mg_map: crate::port_picstruct::MiniGopMap::for_sequence(hierarchical_levels),
            mrp_ctrls: crate::port_picstruct::MrpCtrls::default(),
            speed_config: SpeedConfig::from_native_preset(
                crate::speed_config::NativePreset::new(preset).unwrap(),
            ),
            rc_config,
            rc_state: RcState::default(),
            rc_vbr_cbr: None,
            enable_tf: true,
            enable_tf_key: true,
            dpb: DecodedPictureBuffer::new(),
            gop: GopStructure::new(hierarchical_levels, intra_period),
            hier_auto,
            pred_structure: crate::port_picstruct::PredStructure::LowDelay,
            ra_input: alloc::vec::Vec::new(),
            ra_stats: alloc::vec::Vec::new(),
            delayed_intra: None,
            delayed_intra_out: None,
            delayed_intra_tpl: None,
            ra_display_next: 0,
            frame_count: 0,
            width: dims.aligned_w as u32,
            height: dims.aligned_h as u32,
            true_width: width,
            true_height: height,
            bit_depth: 8,
            upscaled_width: width,
            superres_denom: None,
            image_sequence: false,
            superres_stats_luma: None,
            hbd_source: None,
            hbd_superres_src: None,
            pa_ref: None,
            pa_slots: [const { None }; 8],
            pa_scratch: None,
            me_scratch: None,
            // C-matched default: CICP "unspecified" (cp/tc/mc = 2/2/2,
            // studio range) — the library defaults of enc_settings.c:1043.
            // The SH then carries color_description_present_flag=0 and
            // color_range=0, byte-matching C at matched configs. Callers
            // that know their color space (AVIF path) override via
            // with_color_description.
            color_description: crate::entropy::obu::ColorDescription::default(),
            chroma_sample_position: 0,
            chroma_420: false,
            chroma_format: None,
            recon_output: false,
            last_recon: None,
            last_recon_display_order: None,
            recon_frames: alloc::collections::VecDeque::new(),
            last_recon_unfiltered: None,
            last_recon_pre_cdef: None,
            last_recon10_y: None,
            last_recon10_uv: None,
            last_recon10_final: None,
            last_cdef_stats: crate::cdef::CdefStats::default(),
            last_cdef_signaled: None,
            last_lr_stats: ([0; 3], 0),
            last_lr_unit_size: None,
            tile_rows_log2: 0,
            tile_cols_log2: 0,
            sb_size,
            sb_size_override: None,
            derived_sb_size: derived_sb,
            sb128_fallback,
            // Feature 4: auto by default (byte-inert regardless of value).
            thread_count: 0,
            // Feature 1: no-op token (never stops) — zero-cost `None` variant.
            stop: almost_enough::StopToken::new(enough::Unstoppable),
        }
    }

    /// Whether the SB128 encode path can code a frame at this preset.
    ///
    /// SB128 is a whole second geometry (128 partition root with the
    /// 8-symbol alphabet, the b64<->sb stat bridges, the CDEF 4-quadrant
    /// contract, per-128-region CDF seeding — see docs/sb128-port-map.md).
    /// Until every one of those lands, a cell C would code at 128 is coded
    /// at 64 instead: a valid, decodable stream that does NOT byte-match.
    /// That is deliberate — the alternative is a panic or an undecodable
    /// stream, both worse. `sb128_fallback` reports when it happened.
    ///
    /// Flipped per-capability as the chunks land.
    ///
    /// LANDED (task #91 chunk 3): the 128 partition ROOT. The SB is walked
    /// as its b64 coding units in Z-order and coded as a `PARTITION_SPLIT`
    /// at the 128 square (8-symbol alphabet, ctx 16..19) — see
    /// `merge_sb_units` and `sb128_geom::sb_coding_units`. Everything below
    /// the root is the byte-proven per-64 path.
    ///
    /// STILL UNPORTED (the supported path below uses forced SPLIT):
    /// a genuine 128-level NONE/HORZ/VERT RD search (this path is
    /// forced-SPLIT), the b64<->sb stat bridges (`get_sb128_variance` /
    /// `get_sb128_me_data`), and the CDEF 4-quadrant three-phase contract.
    pub(super) fn sb128_encode_supported(preset: i8) -> bool {
        // All presets are admitted; no content gate is applied here.
        // Presets 0/1 are the only ones C ever codes at 128 in
        // allintra (`derive_super_block_size`), so anything else reaching
        // here is an `SVTAV1_SB=128` override — honour it, the walk is
        // preset-agnostic.
        let _ = preset;
        true
    }

    pub(super) fn resolve_sb_size(
        derived: usize,
        override_: Option<usize>,
        preset: i8,
    ) -> (usize, bool) {
        let want = override_
            .filter(|n| matches!(n, 64 | 128))
            .unwrap_or(derived);
        debug_assert!(
            want == 64 || want == 128,
            "sb_size must be 64 or 128, got {want}"
        );
        if want == 128 && !Self::sb128_encode_supported(preset) {
            (64, true)
        } else {
            (want, false)
        }
    }

    /// SH `separate_uv_delta_q`. The fork always signals it; an `__expert`
    /// chroma override with distinct U/V deltas needs it too. The SH bit and
    /// the FH [`crate::entropy::obu::ChromaQSignal`] form both read this, so
    /// they cannot disagree.
    pub(super) fn separate_uv_delta_q(&self) -> bool {
        #[cfg(feature = "__expert")]
        if self
            .chroma_q_override
            .is_some_and(crate::chroma_q::ChromaQOverride::needs_separate_uv)
        {
            return true;
        }
        self.hdr.is_fork()
    }

    /// Produce the decoder-exact reconstruction in `last_recon` /
    /// `last_recon_unfiltered` / `last_recon_pre_cdef` (default: OFF).
    ///
    /// WHY IT IS OFF BY DEFAULT (measured 2026-08-11, Apple M4 Pro): the
    /// reconstruction is not an input to the bitstream on a still frame whose
    /// loop restoration is disabled — which is every preset >= 7, since
    /// `seq_tools_for_preset` turns Wiener off there — so the in-loop deblock
    /// and CDEF *application* passes run purely to materialise it. Skipping
    /// them is byte-inert (90/90 cells of {64,128,256} x p{7,8,9,10,13} x
    /// qp{20,40,55} x {gradient,uniform} unchanged) and buys **1.36-1.39x at
    /// p10/p13 and 1.11-1.15x at p7** on the whole encode (n=9 interleaved
    /// paired rounds/cell, identity control band 0.99-1.02). At preset <= 6
    /// the passes stay on regardless: the CDEF search and the Wiener search
    /// both read the filtered recon, so they feed the bitstream there — and
    /// the same experiment shows 13/36 of those cells change bytes when the
    /// passes are removed.
    ///
    /// The C reference behaves the same way: `svt_av1_enc_get_packet` yields
    /// no reconstruction, and `SvtAv1EncApp` only produces one under `-o`.
    /// Its profile at preset 10 contains zero CDEF and zero loop-filter
    /// samples while emitting byte-identical output.
    ///
    /// Turn it ON for recon parity, PSNR/evidence tooling, or anything that
    /// reads the `last_recon*` fields — they are `None` otherwise.
    pub fn with_recon_output(mut self, enabled: bool) -> Self {
        self.recon_output = enabled;
        self
    }

    /// Pin the superblock size instead of deriving it (`SVTAV1_SB`).
    /// `Some(128)` on a cell whose encode path is unsupported still falls
    /// back to 64 and sets [`Self::sb128_fallback`] — the override chooses
    /// what to ASK for, not what to bypass.
    pub fn with_sb_size(mut self, sb: Option<usize>) -> Self {
        self.sb_size_override = sb;
        let (sb_size, fell_back) =
            Self::resolve_sb_size(self.derived_sb_size, sb, self.speed_config.preset);
        self.sb_size = sb_size;
        self.sb128_fallback = fell_back;
        self
    }

    /// Enable super-resolution at `denom` (9..=16) — superres chunk B.3.
    ///
    /// The frame is then ENCODED at the reduced width `upscaled_w * 8 / denom`
    /// (C `calculate_scaled_size_helper`) and a conforming decoder upscales it
    /// back to the width the caller passed to [`Self::new`], with the
    /// normative 8-tap filter ([`svtav1_dsp::superres`]). The caller keeps
    /// handing in FULL-width planes; the pipeline downscales them
    /// ([`svtav1_dsp::resize`], C `svt_av1_resize_plane_horizontal`).
    ///
    /// Height is unchanged — superres is horizontal only.
    ///
    /// Off by default (denominator 8), matching C. Re-derives the aligned
    /// encode dims and the superblock size from the CODED width, exactly as
    /// [`Self::new`] would have for a frame of that width.
    pub fn with_superres(mut self, denom: u8) -> Self {
        // Preserve an invalid request for the fallible encode entry point;
        // do not divide by it or let it change the working geometry.
        if !(9..=16).contains(&denom) {
            self.superres_denom = Some(denom);
            return self;
        }
        let coded = u32::from(svtav1_dsp::superres::scaled_size(
            self.upscaled_width as u16,
            denom,
        ));
        self.superres_denom = Some(denom);
        self.true_width = coded;
        let dims = crate::frame_geom::FrameDims::new(coded as usize, self.true_height as usize);
        self.width = dims.aligned_w as u32;
        self.height = dims.aligned_h as u32;
        // The SB derivation keys off the ALIGNED dims, which just changed.
        let sb_inputs = crate::sb128_geom::SbSizeInputs {
            qp: self.rc_config.qp,
            allintra: self.gop.intra_period == 1,
            ..Default::default()
        };
        self.derived_sb_size = crate::sb128_geom::derive_super_block_size(
            dims.aligned_w,
            dims.aligned_h,
            self.speed_config.preset as i8,
            &sb_inputs,
        );
        let (sb_size, fell_back) = Self::resolve_sb_size(
            self.derived_sb_size,
            self.sb_size_override,
            self.speed_config.preset,
        );
        self.sb_size = sb_size;
        self.sb128_fallback = fell_back;
        self
    }

    /// Use full AV1 headers for an animated image sequence. All-intra coding
    /// remains selected by the GOP configuration. An all-intra GOP produces
    /// a sync sample for each picture.
    pub fn with_image_sequence(mut self) -> Self {
        self.image_sequence = true;
        self
    }

    /// Set bit depth (8, 10, or 12).
    pub fn with_bit_depth(mut self, depth: u8) -> Self {
        self.bit_depth = depth;
        self
    }

    /// Request `TileRowsLog2` tile rows (`1 << log2` tile rows; 0 = single
    /// tile row, the default). Out-of-range requests are clamped exactly
    /// like C (see [`Self::tile_rows_log2`]) rather than rejected.
    pub fn with_tile_rows_log2(mut self, log2: u8) -> Self {
        self.tile_rows_log2 = log2;
        self
    }

    /// Request `TileColsLog2` tile columns (`1 << log2`; 0 = single tile
    /// column, the default). Clamped exactly like C — see
    /// [`Self::tile_cols_log2`].
    pub fn with_tile_cols_log2(mut self, log2: u8) -> Self {
        self.tile_cols_log2 = log2;
        self
    }

    /// Set CICP color description for wide gamut / HDR signaling.
    pub fn with_color_description(mut self, cd: crate::entropy::obu::ColorDescription) -> Self {
        self.color_description = cd;
        self
    }

    /// Set the SH `chroma_sample_position` (0 unknown, 1 vertical, 2
    /// colocated) — see [`Self::chroma_sample_position`]. Values > 2 are
    /// refused at encode time.
    pub fn with_chroma_sample_position(mut self, csp: u8) -> Self {
        self.chroma_sample_position = csp;
        self
    }

    /// C `scs->static_config.pred_structure` (`SVT_PRED_STRUCT`): `LowDelay`
    /// (the default) or `RandomAccess`.
    ///
    /// Random access changes the public contract: `try_encode_frame*` accepts
    /// an input frame and returns `Ok(Vec::new())` until a whole mini-GOP is
    /// buffered, then returns that window's packets concatenated in DECODE
    /// order (which is also the correct bitstream order — AV1 packets are
    /// emitted in decode order, with `order_hint` carrying display order).
    /// Call [`Self::try_flush`] after the last input to drain a partial
    /// trailing mini-GOP, which C codes low-delay (`is_pic_cutting_short_ra_mg`).
    pub fn with_pred_structure(mut self, p: crate::port_picstruct::PredStructure) -> Self {
        self.pred_structure = p;
        // The AUTO arm reads `pred_structure`, so it resolves here where the
        // structure becomes final — mirroring enc_handle, where resolution
        // runs after the config fields are all copied into static_config.
        self.resolve_hierarchical_levels_auto();
        self
    }

    /// C's `hierarchical_levels == HIERARCHICAL_LEVELS_AUTO` resolution
    /// (`Globals/enc_handle.c:4556-4567`), run once the AUTO inputs are
    /// final:
    ///
    /// ```text
    /// pred_structure == LOW_DELAY && (rc == CBR || !(enc_mode <= ENC_M9)) -> 2
    /// pred_structure == LOW_DELAY -> 3
    /// rc == VBR || rc == CBR
    ///   || (input_resolution >= 1080p_RANGE && enc_mode >= ENC_M8)
    ///   || !(enc_mode <= ENC_M8) || input_resolution >= 8K_RANGE -> 4
    /// otherwise -> 5
    /// ```
    ///
    /// then the all-intra clamp (`enc_handle.c:4576-4579`, applied to AUTO
    /// and explicit levels alike in C): `allintra` — `intra_period == 0 ||
    /// avif || pred_structure == ALL_INTRA` in C, `intra_period == 1 ||
    /// PredStructure::AllIntra` here — forces 2. C's single-pass low-delay
    /// CBR clamp (`:4568-4573`) cannot move an AUTO result (the low-delay
    /// arm is already `<= 2` whenever `rc == CBR`), so it is documented,
    /// not run. `input_resolution` is the same class
    /// [`crate::pd0::input_resolution_class`] derives on the ALIGNED luma
    /// count (`scs->max_input_luma_width * height`, enc_handle.c:3992).
    pub(super) fn resolve_hierarchical_levels_auto(&mut self) {
        if !self.hier_auto {
            return;
        }
        let ld = self.pred_structure == crate::port_picstruct::PredStructure::LowDelay;
        let mode = self.rc_config.mode;
        let em = self.speed_config.preset;
        let res = crate::pd0::input_resolution_class(self.width as usize * self.height as usize);
        let mut hier: u8 = if ld && (mode == crate::rate_control::RcMode::Cbr || em > 9) {
            2
        } else if ld {
            3
        } else if matches!(
            mode,
            crate::rate_control::RcMode::Vbr | crate::rate_control::RcMode::Cbr
        ) || (res >= crate::port_md_winner::INPUT_SIZE_1080P_RANGE && em >= 8)
            || em > 8
            || res >= crate::port_lr_level::INPUT_SIZE_8K_RANGE
        {
            4
        } else {
            5
        };
        if self.gop.intra_period == 1
            || self.pred_structure == crate::port_picstruct::PredStructure::AllIntra
        {
            hier = 2;
        }
        self.gop = GopStructure::new(hier, self.gop.intra_period);
        self.mg_map = crate::port_picstruct::MiniGopMap::for_sequence(hier);
        self.hier_auto = false;
    }

    /// Enable/disable the opt-in 4:2:0 chroma mode (see `chroma_420` field).
    /// `true` is exactly `with_chroma_format(Yuv420)`; `false` withdraws
    /// chroma entirely (mono entry points only).
    pub fn with_chroma_420(mut self, enabled: bool) -> Self {
        self.chroma_420 = enabled;
        self.chroma_format = enabled.then_some(svtav1_types::chroma::ChromaFormat::Yuv420);
        self
    }

    /// Configure the chroma subsampling format directly.
    ///
    /// # Parity contract (measured, not aspirational)
    ///
    /// * [`ChromaFormat::Yuv420`] — the byte-parity surface; identical to
    ///   `with_chroma_420(true)`.
    /// * [`ChromaFormat::Yuv444`] — Zen extension, SHIPPING on the
    ///   8-bit key/still + `sb_size 64` + no-superres envelope:
    ///   decoder-verified (`aomdec`/`dav1d` reconstruction equality,
    ///   `tools/rd_ext_sweep.sh` for the SSIM2/RD oracle), never a
    ///   claim of C parity — C v4.2.0 REFUSES it
    ///   (`Globals/enc_settings.c:470`).
    /// * [`ChromaFormat::Yuv422`] — Zen extension, still staged:
    ///   `try_encode_frame_422` keeps refusing until the arm is proven
    ///   decoder-exact.
    /// * [`ChromaFormat::Yuv400`] — the existing monochrome extension
    ///   (`encode_frame_mono_core`), same no-oracle tier.
    ///
    /// `None` withdraws chroma (same as `with_chroma_420(false)`).
    pub fn with_chroma_format(
        mut self,
        format: Option<svtav1_types::chroma::ChromaFormat>,
    ) -> Self {
        self.chroma_format = format;
        self.chroma_420 = format == Some(svtav1_types::chroma::ChromaFormat::Yuv420);
        if format == Some(svtav1_types::chroma::ChromaFormat::Yuv444) {
            // Full-resolution chroma uses 64x64 superblocks — C's force is
            // unconditional and AFTER the resolution/preset ladder
            // (enc_handle.c:4158-4160, f67a0f74: "at most four 32x32 chroma
            // transforms per coding block"), so a frame that would derive
            // 128 at 4:2:0 derives 64 here. An explicit `with_sb_size(128)`
            // still refuses at the encode envelope rather than silently
            // degrading.
            if self.sb_size_override.is_none() {
                self.sb_size = 64;
                self.derived_sb_size = 64;
                self.sb128_fallback = false;
            }
        }
        self
    }

    /// C `subsampling_x`/`subsampling_y` pair for the configured format
    /// (`enc_handle.c:4636-4637`); `(1, 1)` when chroma is absent — the
    /// value C's own geometry helpers produce on the mono path.
    pub fn subsampling(&self) -> (u8, u8) {
        self.chroma_format
            .map(|f| (f.subsampling_x(), f.subsampling_y()))
            .unwrap_or((1, 1))
    }

    /// Feature 4: bound the tile-parallel encode to at most `n` concurrent OS
    /// threads (`0` = auto via `available_parallelism`). See
    /// [`Self::thread_count`]. Byte-inert — tiles are always reassembled in
    /// tile order — so this only trades throughput against core pressure.
    pub fn with_thread_count(mut self, n: usize) -> Self {
        self.thread_count = n;
        self
    }

    /// Feature 1: install a cooperative cancellation token. Any
    /// [`enough::Stop`] implementation works (e.g. `almost_enough::Stopper`);
    /// it is checked once at the entry of [`Self::try_encode_frame`] /
    /// [`Self::try_encode_frame_420`]. The infallible `encode_frame*` methods
    /// ignore it. See [`Self::stop`].
    pub fn with_stop(mut self, stop: impl enough::Stop + 'static) -> Self {
        self.stop = almost_enough::StopToken::new(stop);
        self
    }

    /// Feature 1 convenience: bound every frame this pipeline encodes with a
    /// wall-clock deadline measured from this call. Composes with any token
    /// already installed via [`Self::with_stop`] (`OrStop` — whichever fires
    /// first wins), so an explicit cancel source is never displaced.
    ///
    /// Expiry surfaces from the fallible `try_encode_frame*` entries as
    /// `Err(Cancelled(StopReason::TimedOut))` — distinguishable from a
    /// caller's explicit `Cancelled`, which is what makes an encode wedged
    /// on adversarial input (or an encoder-side infinite loop) *detectable*
    /// rather than merely killed by an outer `timeout(1)`.
    pub fn with_timeout(self, budget: core::time::Duration) -> Self {
        let existing = self.stop.clone();
        self.with_stop(almost_enough::OrStop::new(
            existing,
            almost_enough::WithTimeout::new(enough::Unstoppable, budget),
        ))
    }
}
