use super::*;

impl EncodePipeline {
    /// Encode a single frame through the full pipeline (monochrome).
    ///
    /// Returns the encoded bitstream data and updates internal state.
    ///
    /// `y_plane` is (true_w x true_h) at `y_stride`, where the TRUE dims are
    /// what the caller passed to [`Self::new`]. When those differ from the
    /// ALIGNED encode dims the plane is edge-replicated up to the aligned grid
    /// here, exactly as [`Self::encode_frame_420`] does it (C
    /// `pad_input_picture`, pic_operators.c:561); for 8-aligned inputs this is
    /// a zero-copy pass-through and the emitted bytes are unchanged.
    pub fn encode_frame(&mut self, y_plane: &[u8], y_stride: usize) -> Vec<u8> {
        // The non-PD0 search now carries a square root through partial SBs
        // and codes the same forced-edge syntax as the entropy walk.
        // Additive fallible core (Feature 1+3). This wrapper KEEPS its exact
        // signature and panicking contract: with the default `Unstoppable`
        // token and the infallible-alloc feature default, the core cannot
        // return `Err` on the trusted path, so `.expect()` never fires and the
        // emitted bytes are unchanged. Callers wanting graceful OOM /
        // cancellation use `try_encode_frame`.
        //
        // "Trusted path" EXCLUDES an unsupported configuration. The config
        // choke point refuses `RcMode::Vbr` (first-pass statistics are not
        // ported) and `RcMode::Cbr` outside LOW_DELAY (C's own envelope),
        // so a caller that builds the pipeline with one and then uses this
        // infallible wrapper panics HERE instead of silently receiving a
        // stream the port never meant to emit. That is the intended trade —
        // a panic is loud, a mislabeled bitstream is not — but the message
        // must say so rather than claim infallibility.
        self.encode_frame_mono_core(y_plane, y_stride).expect(
            "encode_frame is infallible on the default/trusted path; an \
                 UnsupportedConfig here means the pipeline was built with a \
                 configuration this port refuses (e.g. RcMode::Vbr or \
                 non-LOW_DELAY Cbr, issue #22) — use try_encode_frame to \
                 handle it as an error",
        )
    }

    /// Fallible core of the MONOCHROME path: the same TRUE -> ALIGNED edge
    /// replication [`Self::encode_frame_420_core`] performs, then the shared
    /// `encode_frame_impl`. Shared by [`Self::encode_frame`] and
    /// [`Self::try_encode_frame`], which both validate before calling in.
    ///
    /// # Why this exists (and what it replaced)
    ///
    /// Until now the mono entry points REFUSED `width != true_width` outright
    /// — "arbitrary-dims padding is wired on the 4:2:0 path only". The padding
    /// is not 4:2:0-specific: `encode_frame_impl` already takes the padded
    /// plane at the ALIGNED stride and signals `true_width`/`true_height` in
    /// the frame header, and the mono arm differs from the 4:2:0 arm only in
    /// plane count. The refusal cost the AVIF alpha case: an alpha plane is a
    /// MONOCHROME AV1 image at the picture's own, arbitrary size, and
    /// `AvifEncoder::encode_y8` worked around the refusal by pre-padding to a
    /// multiple of 64 while still reporting the caller's true size — so the
    /// coded frame and the size the container would announce DISAGREED for
    /// every non-64-multiple gray image.
    ///
    /// # Evidence tier
    ///
    /// TIER 3, and it can never be better: C v4.2.0 has no monochrome mode at
    /// all (`verify_settings` rejects any `encoder_color_format` other than
    /// `EB_YUV420`, `Globals/enc_settings.c:473`), so there is no byte oracle
    /// for ANY mono cell. The oracle used here is the one the repo already
    /// uses for mono geometry bugs (`tools/regression_spotcheck.sh`'s
    /// `monoReconEq`): the encoder's FINAL reconstruction must equal the
    /// reference decoder's output, plus decodability under aomdec and dav1d.
    pub(super) fn encode_frame_mono_core(
        &mut self,
        y: &[u8],
        y_stride: usize,
    ) -> crate::EncodeResult<Vec<u8>> {
        let (tw, th) = (self.true_width as usize, self.true_height as usize);
        let (aw, ah) = (self.width as usize, self.height as usize);
        if aw == tw && ah == th {
            // Natively 8-aligned: pass through unchanged (byte-identical to
            // every mono stream this encoder has ever emitted).
            return self.encode_frame_impl(y, y_stride, None, None);
        }
        let y_pad = pad_plane_replicate(y, y_stride, tw, th, aw, ah)?;
        self.encode_frame_impl(&y_pad, aw, None, None)
    }

    /// Encode a single 4:2:0 still/key frame (NumPlanes=3).
    ///
    /// `u`/`v` are (true_w/2 x true_h/2) planes at stride `true_w/2`, and
    /// `y` is (true_w x true_h) at `y_stride`, where the TRUE dims are what
    /// the caller passed to [`Self::new`]. When those differ from the
    /// ALIGNED encode dims (task #95), the planes are edge-replicated up to
    /// the aligned grid here (C `pad_input_picture`, pic_operators.c:561);
    /// for 8-aligned inputs this is a zero-copy pass-through.
    /// Requires `chroma_420` to be enabled via [`Self::with_chroma_420`].
    pub fn encode_frame_420(&mut self, y: &[u8], u: &[u8], v: &[u8], y_stride: usize) -> Vec<u8> {
        assert!(
            self.chroma_420,
            "encode_frame_420 requires the pipeline to be built with with_chroma_420(true)"
        );
        let (tw, th) = (self.true_width as usize, self.true_height as usize);
        // TRUE chroma dims (4:2:0 ceiling, matching the input .yuv layout).
        let (tcw, tch) = (tw.div_ceil(2), th.div_ceil(2));
        let cn_true = tcw * tch;
        assert!(
            u.len() >= cn_true && v.len() >= cn_true,
            "u/v planes must be (true_w/2 x true_h/2)"
        );
        // Additive fallible core (Feature 1+3), shared with `try_encode_frame_420`.
        // KEEPS the exact panicking contract: on the default/trusted path the
        // core cannot return `Err`, so `.expect()` never fires and the bytes are
        // unchanged. "Trusted path" EXCLUDES an unsupported configuration — see
        // the note on `encode_frame`; a `RcMode::Vbr` or non-LOW_DELAY `Cbr`
        // pipeline panics here rather than emitting a stream C would refuse
        // or the port cannot compute honestly.
        self.encode_frame_420_core(y, u, v, y_stride).expect(
            "encode_frame_420 is infallible on the default/trusted path; an \
                 UnsupportedConfig here means the pipeline was built with a \
                 configuration this port refuses (e.g. RcMode::Vbr or \
                 non-LOW_DELAY Cbr, issue #22) — use try_encode_frame_420 to \
                 handle it as an error",
        )
    }

    /// Fallible core of the 4:2:0 path (TRUE->ALIGNED padding + the shared
    /// `encode_frame_impl`). Shared by the panicking [`Self::encode_frame_420`]
    /// wrapper and the fallible [`Self::try_encode_frame_420`]; both validate
    /// the chroma flag + plane sizes before calling in.
    pub(super) fn encode_frame_420_core(
        &mut self,
        y: &[u8],
        u: &[u8],
        v: &[u8],
        y_stride: usize,
    ) -> EncodeResult<Vec<u8>> {
        let result = self.encode_frame_420_prepared(y, u, v, y_stride);
        // Also clear on errors before encode_frame_impl takes these fields.
        self.prepared_grain = None;
        self.hbd_source = None;
        self.hbd_superres_src = None;
        result
    }

    pub(super) fn encode_frame_420_prepared(
        &mut self,
        y: &[u8],
        u: &[u8],
        v: &[u8],
        y_stride: usize,
    ) -> crate::EncodeResult<Vec<u8>> {
        // Superres chunk B.3: the caller hands in FULL-width planes; the
        // encode runs at the reduced CODED width. Downscale horizontally with
        // C's `svt_av1_resize_plane_horizontal` (svtav1-dsp::resize, pinned
        // byte-exact vs C) after grain preprocessing, then the existing
        // TRUE->ALIGNED padding and the whole pipeline operate on the coded
        // planes. No-op when superres is off.
        if let Some(why) = self.chroma_format_support_error() {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(why)));
        }
        self.validate_film_grain()?;
        self.prepared_grain = None;
        let prepared = self.prepare_film_grain(y, u, v, y_stride)?;
        if self.superres_denom.is_none() {
            if let Some((planes, stride)) = prepared.as_ref() {
                return self.encode_frame_impl(
                    &planes[0],
                    *stride,
                    Some((&planes[1], &planes[2])),
                    None,
                );
            }
        }
        let (y, u, v, y_stride) = match prepared.as_ref() {
            Some((planes, stride)) => (
                planes[0].as_slice(),
                planes[1].as_slice(),
                planes[2].as_slice(),
                *stride,
            ),
            None => (y, u, v, y_stride),
        };
        let downscaled = self.superres_downscale_420(y, u, v, y_stride)?;
        let (y, u, v, y_stride) = match downscaled.as_ref() {
            Some((yd, ud, vd)) => (
                yd.as_slice(),
                ud.as_slice(),
                vd.as_slice(),
                self.true_width as usize,
            ),
            None => (y, u, v, y_stride),
        };
        let (tw, th) = (self.true_width as usize, self.true_height as usize);
        let (aw, ah) = (self.width as usize, self.height as usize);
        // Task #95 chunk 2: partial SBs are now supported. Every 4:2:0 KEY
        // frame routes through the PD0 fixed-tree path (use_funnel is always
        // live for 4:2:0 key), which starts from a 64x64 root carrying the
        // spec-5.11.4 forced edge splits and codes the partition symbols with
        // the edge-aware alphabets (encode_partition_av1). The only invariant
        // is that the ALIGNED dims are a multiple of MIN_BLOCK_SIZE (8), which
        // FrameDims guarantees by construction.
        debug_assert!(
            aw % crate::frame_geom::MIN_BLOCK_SIZE == 0
                && ah % crate::frame_geom::MIN_BLOCK_SIZE == 0,
            "aligned dims must be 8-aligned; got {aw}x{ah} for true {tw}x{th}"
        );
        // TRUE chroma dims — format-derived (ChromaFormat's ceiling);
        // at Yuv420 this is exactly `div_ceil(2)`, byte-identical.
        let fmt = self
            .chroma_format
            .unwrap_or(svtav1_types::chroma::ChromaFormat::Yuv420);
        let (tcw, tch) = (fmt.chroma_width(tw), fmt.chroma_height(th));
        if aw == tw && ah == th {
            // Natively 8-aligned: pass through unchanged (byte-identical to
            // the pre-#95 path).
            return self.encode_frame_impl(y, y_stride, Some((u, v)), None);
        }
        // Pad TRUE -> ALIGNED. C replicates the last valid column, then the
        // last valid row (incl. the new right pad); the per-pixel min-clamp
        // in `pad_plane_replicate` is equivalent for a rectangular region.
        let (acw, ach) = (fmt.chroma_width(aw), fmt.chroma_height(ah));
        let y_pad = pad_plane_replicate(y, y_stride, tw, th, aw, ah)?;
        let u_pad = pad_plane_replicate(u, tcw, tcw, tch, acw, ach)?;
        let v_pad = pad_plane_replicate(v, tcw, tcw, tch, acw, ach)?;
        self.encode_frame_impl(&y_pad, aw, Some((&u_pad, &v_pad)), None)
    }

    /// Fallible twin of [`Self::encode_frame`] (Feature 1 + 2).
    ///
    /// Byte-identical to [`Self::encode_frame`] on success. The difference is
    /// purely at the boundary: the legacy `assert!`s become typed
    /// [`EncodeError`]s, and the cooperative cancellation token
    /// ([`Self::stop`]) is checked once at entry. The legacy method is left
    /// untouched. Internally this calls the same fallible `encode_frame_impl`;
    /// its configuration, allocation and cancellation errors propagate.
    pub fn try_encode_frame(&mut self, y_plane: &[u8], y_stride: usize) -> EncodeResult<Vec<u8>> {
        if let Some(e) = self.ra_entry_error("try_encode_frame") {
            return Err(e);
        }
        // (a) Validate the true input extent. Padding is performed in
        // encode_frame_mono_core; both partition paths handle partial SBs.
        let (tw, th) = (self.true_width as usize, self.true_height as usize);
        if y_plane.len() < (th - 1) * y_stride + tw {
            return Err(whereat::at!(EncodeError::InvalidDimensions {
                width: self.true_width,
                height: self.true_height,
                reason: "monochrome luma plane must cover the true dims at y_stride",
            }));
        }
        // (b) Feature 1 entry stop-check (frame-granular).
        self.stop
            .check()
            .map_err(EncodeError::from)
            .map_err(whereat::at)?;
        // (c) The fallible core (asserts above pre-satisfied). Its own in-loop
        // stop-checks + fallible allocations propagate here as `Err` instead of
        // panicking/aborting; on success the bytes match `encode_frame`.
        self.encode_frame_mono_core(y_plane, y_stride)
    }

    /// Fallible twin of [`Self::encode_frame_420`] (Feature 1 + 2).
    ///
    /// Byte-identical to [`Self::encode_frame_420`] on success. The legacy
    /// `assert!`s (chroma flag, u/v plane sizes, still/key-only) become typed
    /// [`EncodeError`]s and the cancellation token is checked at entry;
    /// otherwise it delegates to the fallible core, which pads the true
    /// dimensions to the aligned canvas and calls `encode_frame_impl`.
    pub fn try_encode_frame_420(
        &mut self,
        y: &[u8],
        u: &[u8],
        v: &[u8],
        y_stride: usize,
    ) -> EncodeResult<Vec<u8>> {
        // (a) Validate — mirror the `encode_frame_420` + impl asserts.
        if !self.chroma_420 {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "encode_frame_420 requires the pipeline to be built with with_chroma_420(true)",
            )));
        }
        // Random access changes the contract: an input may produce no packet
        // until its mini-GOP completes; `try_flush` drains the tail.
        if self.pred_structure == crate::port_picstruct::PredStructure::RandomAccess {
            return self.try_encode_frame_420_ra(y, u, v, y_stride);
        }
        // INTER FRAMES ARE SHIPPED HERE as of 2026-09-11. This used to be a
        // blanket "still/key frames only" refusal, lifted only by
        // `SVTAV1_INTER_EXPERIMENTAL`; both that refusal and that variable are
        // gone. What replaced them is `tools/video_selfcheck_gate.sh`: the
        // port's own final reconstruction is byte-identical to `aomdec`'s for
        // every frame of an 8-frame encode, on all six public-domain derf
        // clips at qp {20,40,55} — 18 of 18 cells. The configuration envelope
        // is still enforced, by `gop_config_error` (flat low-delay P only) and
        // the other `*_config_error` guards, so an unwired GOP shape is
        // refused exactly as before.
        //
        // See `encode_frame_impl` for the parity claim this does NOT make.
        let (tw, th) = (self.true_width as usize, self.true_height as usize);
        let (tcw, tch) = (tw.div_ceil(2), th.div_ceil(2));
        let cn_true = tcw * tch;
        if u.len() < cn_true || v.len() < cn_true {
            return Err(whereat::at!(EncodeError::InvalidDimensions {
                width: self.true_width,
                height: self.true_height,
                reason: "u/v planes must each be at least (true_w/2 x true_h/2)",
            }));
        }
        // (b) Feature 1 entry stop-check (frame-granular).
        self.stop
            .check()
            .map_err(EncodeError::from)
            .map_err(whereat::at)?;
        // (c) The fallible core (padding + encode_frame_impl), NOT the panicking
        // `encode_frame_420` wrapper — so a fallible alloc / cancellation
        // surfaces as `Err` here instead of unwinding through `.expect()`.
        self.encode_frame_420_core(y, u, v, y_stride)
    }

    /// Native 10-bit (u16) 4:2:0 entry point — task #6 chunk 1
    /// (`rust/docs/hbd-input-port-map.md`).
    ///
    /// `y`/`u`/`v` carry REAL 10-bit samples (0..=1023) in the same TRUE-dim
    /// layout [`Self::try_encode_frame_420`] takes for `u8` (`y` at
    /// `y_stride`, chroma at `(true_w+1)/2`). Requires the pipeline to be
    /// built with `with_bit_depth(10)` and `with_chroma_420(true)`.
    ///
    /// # What chunk 1 threads — and what it does not
    ///
    /// The low 2 bits reach the **mode decision and the coded levels**: the
    /// bd10 MD funnel (MDS0 SATD, and the MDS1/MDS3 full-RD inputs for luma
    /// AND chroma) plus the bd10 level re-encode post-pass all read the real
    /// u16 samples. The **post-filter searches** (deblock level, CDEF
    /// strength, Wiener taps) and the recon SSE still run on the
    /// MSB-truncated u8 planes — that is chunk 2. The emitted bitstream is a
    /// valid 10-bit stream either way; the band-limit is on filter DECISIONS,
    /// not on the coded residual.
    ///
    /// # Errors
    ///
    /// [`EncodeError::UnsupportedConfig`] if the pipeline is not bd10/4:2:0,
    /// if the frame is not a key frame, if a sample exceeds 10 bits, or if
    /// this preset/dimension combination has no consumer for the native
    /// source (see [`Self::hbd_source_consumed`]) — rejecting beats silently
    /// encoding the MSB-truncated content. Also propagates cancellation and
    /// fallible-allocation failures like the u8 entry points.
    pub fn try_encode_frame_420_hbd(
        &mut self,
        y: &[u16],
        u: &[u16],
        v: &[u16],
        y_stride: usize,
    ) -> EncodeResult<Vec<u8>> {
        if !self.chroma_420 {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "try_encode_frame_420_hbd requires the pipeline to be built with \
                 with_chroma_420(true)",
            )));
        }
        if let Some(e) = self.ra_entry_error("try_encode_frame_420_hbd") {
            return Err(e);
        }
        if self.bit_depth != 10 {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "try_encode_frame_420_hbd requires with_bit_depth(10) (8-bit sources use \
                 encode_frame_420; 12-bit is outside C's shipping envelope)",
            )));
        }
        if !self.hbd_source_consumed(true) {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "native 10-bit input needs a bd10 consumer: either preset \
                 >= 9 or a full-RD-capable preset <= 8 (non-screen content) — see \
                 docs/hbd-input-port-map.md chunk 2",
            )));
        }
        // Inter frames ship on the 10-bit entry point too, on the same
        // evidence as the 8-bit one above plus `tools/bd10_video_gate.sh`
        // (24/24 encode AND decode). The blanket refusal and
        // `SVTAV1_INTER_EXPERIMENTAL` are both gone; the configuration
        // envelope is still enforced by the `*_config_error` guards.
        let (tw, th) = (self.true_width as usize, self.true_height as usize);
        // The caller hands FULL-width (upscaled) planes under superres,
        // coded-width planes otherwise — the same contract as
        // [`Self::try_encode_frame_420`]. Chroma is 4:2:0 ceiling either way.
        let (iw, icw) = if self.superres_denom.is_some() {
            let uw = self.upscaled_width as usize;
            (uw, uw.div_ceil(2))
        } else {
            (tw, tw.div_ceil(2))
        };
        let ich = th.div_ceil(2);
        if y.len() < (th - 1) * y_stride + iw || u.len() < icw * ich || v.len() < icw * ich {
            return Err(whereat::at!(EncodeError::InvalidDimensions {
                width: iw as u32,
                height: self.true_height,
                reason: "hbd planes must cover the input dims (y at y_stride, u/v at w/2; \
                         FULL width under superres)",
            }));
        }
        let max = 1u16 << self.bit_depth;
        if y.iter().chain(u.iter()).chain(v.iter()).any(|&s| s >= max) {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "hbd source carries a sample above the configured bit depth",
            )));
        }
        self.stop
            .check()
            .map_err(EncodeError::from)
            .map_err(whereat::at)?;
        if self.superres_denom.is_some() {
            // Stage the native FULL-width source for the downscale inside
            // `encode_frame_420_prepared`. That arm produces BOTH outputs in
            // C's order — filtered u16 -> `hbd_source` (coded dims, aligned),
            // `filtered_u16 >> 2` -> the u8 canvas — so building either here
            // would duplicate the step or force the wrong
            // truncate-then-filter order. The u8 arguments are unused on
            // this arm (film-grain denoise + bd10 + superres is refused in
            // `validate_film_grain` before the planes are read).
            let mut sy = svtav1_types::try_vec![0u16; iw * th]?;
            for r in 0..th {
                sy[r * iw..(r + 1) * iw].copy_from_slice(&y[r * y_stride..r * y_stride + iw]);
            }
            let mut su = svtav1_types::try_vec![0u16; icw * ich]?;
            su.copy_from_slice(&u[..icw * ich]);
            let mut sv = svtav1_types::try_vec![0u16; icw * ich]?;
            sv.copy_from_slice(&v[..icw * ich]);
            self.hbd_superres_src = Some(HbdSuperresSrc {
                y: sy,
                u: su,
                v: sv,
            });
            // `encode_frame_420_core` clears `hbd_superres_src` and the
            // produced `hbd_source` unconditionally — error paths included —
            // so nothing staged can leak into the next frame.
            return self.encode_frame_420_core(&[], &[], &[], 0);
        }
        // Stash the ALIGNED-padded u16 planes for `encode_frame_impl` to take,
        // and drive the existing core with the MSB-truncated u8 planes — the
        // sites chunk 1 does not thread (post-filter searches, recon SSE) keep
        // reading those. Truncation and edge replication are both per-sample
        // gathers, so truncate-then-pad == pad-then-truncate: the u8 planes the
        // core builds are exactly the u8 planes an 8-bit caller would pass.
        let shift = u32::from(self.bit_depth - 8);
        let (aw, ah) = (self.width as usize, self.height as usize);
        let hbd = HbdSource {
            y: pad_plane_replicate_u16(y, y_stride, tw, th, aw, ah)?,
            u: pad_plane_replicate_u16(u, icw, icw, ich, aw / 2, ah / 2)?,
            v: pad_plane_replicate_u16(v, icw, icw, ich, aw / 2, ah / 2)?,
        };
        let mut y8 = svtav1_types::try_vec![0u8; tw * th]?;
        for r in 0..th {
            for c in 0..tw {
                y8[r * tw + c] = (y[r * y_stride + c] >> shift) as u8;
            }
        }
        let mut u8p = svtav1_types::try_vec![0u8; icw * ich]?;
        let mut v8p = svtav1_types::try_vec![0u8; icw * ich]?;
        for i in 0..icw * ich {
            u8p[i] = (u[i] >> shift) as u8;
            v8p[i] = (v[i] >> shift) as u8;
        }
        self.hbd_source = Some(hbd);
        let out = self.encode_frame_420_core(&y8, &u8p, &v8p, tw);
        // Never leave a stale source behind for the next frame (the happy
        // path already took it; this covers the early-error paths).
        self.hbd_source = None;
        out
    }

    /// 4:4:4 chroma entry point — Zen extension (C refuses non-420 at
    /// `verify_settings`, `enc_settings.c:470`; no byte oracle).
    ///
    /// `y`/`u`/`v` are all FULL-resolution (each `true_w × true_h`, `y`
    /// at `y_stride`). Requires `with_chroma_format(Some(Yuv444))`.
    ///
    /// Support envelope (decoder-verified — C refuses non-4:2:0 at
    /// `verify_settings`, enc_settings.c:470, so no byte oracle exists):
    /// 8-bit, `sb_size 64`, no superres — key AND inter frames (the
    /// non-funnel inter arm predicts chroma by motion compensation and
    /// residual-codes against it; no `uv_mode` is signalled). Everything
    /// else (10-bit, sb128, superres, IntraBC) takes the honest refusal
    /// at `encode_frame_impl`'s envelope gate. Chroma loop filters are
    /// signalled off (lf levels 0 / CDEF uv 0 / LR RESTORE_NONE) until the
    /// filter kernels are ported — decoder-consistent by construction.
    /// Quality oracle: `tools/rd_ext_sweep.sh` (SSIMULACRA2 + per-plane
    /// PSNR against aomenc `--i444 --profile=1`).
    ///
    /// # Errors
    /// [`EncodeError::UnsupportedConfig`] outside the envelope;
    /// [`EncodeError::InvalidDimensions`] on short planes.
    pub fn try_encode_frame_444(
        &mut self,
        y: &[u8],
        u: &[u8],
        v: &[u8],
        y_stride: usize,
    ) -> EncodeResult<Vec<u8>> {
        if self.chroma_format != Some(svtav1_types::chroma::ChromaFormat::Yuv444) {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "try_encode_frame_444 requires the pipeline to be built with \
                 with_chroma_format(Some(ChromaFormat::Yuv444))",
            )));
        }
        if let Some(e) = self.ra_entry_error("try_encode_frame_444") {
            return Err(e);
        }
        let (tw, th) = (self.true_width as usize, self.true_height as usize);
        let n = tw * th;
        if y.len() < (th - 1) * y_stride + tw || u.len() < n || v.len() < n {
            return Err(whereat::at!(EncodeError::InvalidDimensions {
                width: self.true_width,
                height: self.true_height,
                reason: "4:4:4 planes must each cover the true dims (u/v full-resolution)",
            }));
        }
        self.stop
            .check()
            .map_err(EncodeError::from)
            .map_err(whereat::at)?;
        self.encode_frame_420_core(y, u, v, y_stride)
    }

    /// 4:2:2 chroma entry point — Zen extension, same staging contract
    /// as [`Self::try_encode_frame_444`]. `u`/`v` are `true_w/2 × true_h`
    /// (horizontally subsampled only).
    ///
    /// # Errors
    /// As [`Self::try_encode_frame_444`].
    pub fn try_encode_frame_422(
        &mut self,
        y: &[u8],
        u: &[u8],
        v: &[u8],
        y_stride: usize,
    ) -> EncodeResult<Vec<u8>> {
        if self.chroma_format != Some(svtav1_types::chroma::ChromaFormat::Yuv422) {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "try_encode_frame_422 requires the pipeline to be built with \
                 with_chroma_format(Some(ChromaFormat::Yuv422))",
            )));
        }
        if let Some(e) = self.ra_entry_error("try_encode_frame_422") {
            return Err(e);
        }
        let (tw, th) = (self.true_width as usize, self.true_height as usize);
        let (tcw, n) = (tw.div_ceil(2), tw.div_ceil(2) * th);
        if y.len() < (th - 1) * y_stride + tw || u.len() < n || v.len() < n {
            return Err(whereat::at!(EncodeError::InvalidDimensions {
                width: self.true_width,
                height: self.true_height,
                reason: "4:2:2 planes: y at y_stride over true dims, u/v at (true_w+1)/2 x true_h",
            }));
        }
        let _ = tcw;
        self.stop
            .check()
            .map_err(EncodeError::from)
            .map_err(whereat::at)?;
        self.encode_frame_420_core(y, u, v, y_stride)
    }

    /// Native 10-bit (u16) monochrome entry point — the mono twin of
    /// [`Self::try_encode_frame_420_hbd`] (task #6 chunk 1).
    ///
    /// Monochrome builds no MD funnel, so the only consumer of the real u16
    /// samples is the bd10 level re-encode post-pass (every preset): the coded
    /// LEVELS are computed at true 10 bits, while the mode decision itself
    /// still runs on the MSB-truncated plane. Rejects any config where even
    /// that consumer is absent.
    ///
    /// # Errors
    ///
    /// As [`Self::try_encode_frame_420_hbd`], plus the monochrome dimension
    /// rules of [`Self::try_encode_frame`].
    pub fn try_encode_frame_hbd(&mut self, y: &[u16], y_stride: usize) -> EncodeResult<Vec<u8>> {
        if self.chroma_420 {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "try_encode_frame_hbd is the monochrome entry point; use \
                 try_encode_frame_420_hbd on a 4:2:0 pipeline",
            )));
        }
        if let Some(e) = self.ra_entry_error("try_encode_frame_hbd") {
            return Err(e);
        }
        if self.bit_depth != 10 {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "try_encode_frame_hbd requires with_bit_depth(10)",
            )));
        }
        if !self.hbd_source_consumed(false) {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "native 10-bit monochrome input requires a native level producer (defensive check; all current presets are supported)",
            )));
        }
        let (tw, th) = (self.true_width as usize, self.true_height as usize);
        if y.len() < (th - 1) * y_stride + tw {
            return Err(whereat::at!(EncodeError::InvalidDimensions {
                width: self.true_width,
                height: self.true_height,
                reason: "hbd luma plane must cover the true dims at y_stride",
            }));
        }
        let max = 1u16 << self.bit_depth;
        if y.iter().any(|&s| s >= max) {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "hbd source carries a sample above the configured bit depth",
            )));
        }
        self.stop
            .check()
            .map_err(EncodeError::from)
            .map_err(whereat::at)?;
        let shift = u32::from(self.bit_depth - 8);
        let mut y8 = svtav1_types::try_vec![0u8; tw * th]?;
        for r in 0..th {
            for c in 0..tw {
                y8[r * tw + c] = (y[r * y_stride + c] >> shift) as u8;
            }
        }
        // Match the u8 mono core's TRUE -> ALIGNED padding on both source
        // representations; the native level producer consumes the u16 plane.
        self.hbd_source = Some(HbdSource {
            y: pad_plane_replicate_u16(
                y,
                y_stride,
                tw,
                th,
                self.width as usize,
                self.height as usize,
            )?,
            u: alloc::vec::Vec::new(),
            v: alloc::vec::Vec::new(),
        });
        let out = self.encode_frame_mono_core(&y8, tw);
        self.hbd_source = None;
        out
    }
}
