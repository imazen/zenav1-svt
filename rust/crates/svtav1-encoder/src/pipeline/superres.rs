use super::*;

impl EncodePipeline {
    /// Superres chunk B.3: horizontally downscale the caller's FULL-width
    /// 4:2:0 planes to the coded width. `None` when superres is off (the
    /// planes are used as passed — zero copies, byte-identical path).
    ///
    /// Chroma is resized at its OWN widths (4:2:0 ceiling on both sides), so
    /// the coded chroma width is `(coded_w + 1) / 2` — the same rounding the
    /// rest of the pipeline uses.
    #[allow(clippy::type_complexity)] // ported C signature: a `type` alias here would hide the shape and churn the byte-identity gate for no benefit
    pub(super) fn superres_downscale_420(
        &mut self,
        y: &[u8],
        u: &[u8],
        v: &[u8],
        y_stride: usize,
    ) -> crate::EncodeResult<Option<(Vec<u8>, Vec<u8>, Vec<u8>)>> {
        let Some(_denom) = self.superres_denom else {
            return Ok(None);
        };
        // Native 10-bit arm (bd10 + superres): the staged u16 source is
        // downscaled at full precision and only THEN truncated to the u8
        // canvas — C's `svt_aom_resize_frame` pack -> `highbd_resize` ->
        // unpack order (resize.c). The u8 plane arguments are unused on this
        // arm; see `superres_downscale_420_hbd`.
        if let Some(src) = self.hbd_superres_src.take() {
            return self.superres_downscale_420_hbd(&src).map(Some);
        }
        let (uw, th) = (self.upscaled_width as usize, self.true_height as usize);
        let cw = self.true_width as usize;
        let (ucw, uch) = (uw.div_ceil(2), th.div_ceil(2));
        let ccw = cw.div_ceil(2);
        let mut yd = svtav1_types::try_vec![0u8; cw * th]?;
        let mut ud = svtav1_types::try_vec![0u8; ccw * uch]?;
        let mut vd = svtav1_types::try_vec![0u8; ccw * uch]?;
        svtav1_dsp::resize::resize_plane_horizontal(y, th, uw, y_stride, &mut yd, cw, cw);
        svtav1_dsp::resize::resize_plane_horizontal(u, uch, ucw, ucw, &mut ud, ccw, ccw);
        svtav1_dsp::resize::resize_plane_horizontal(v, uch, ucw, ucw, &mut vd, ccw, ccw);
        // Keep the ORIGINAL luma (tightened to `uw` stride) for the picture
        // statistics C derives before scaling — see `superres_stats_luma`.
        let mut orig = svtav1_types::try_vec![0u8; uw * th]?;
        for r in 0..th {
            orig[r * uw..(r + 1) * uw].copy_from_slice(&y[r * y_stride..r * y_stride + uw]);
        }
        self.superres_stats_luma = Some((orig, uw, th));
        Ok(Some((yd, ud, vd)))
    }

    /// Superres chunk B.3, native 10-bit arm: horizontally downscale the
    /// staged u16 source to the coded width with C's highbd resize ladder
    /// (`svt_av1_highbd_resize_plane_horizontal`, pinned byte-exact by
    /// `c_parity_resize_hbd`), then derive BOTH source representations the
    /// rest of the frame needs:
    ///
    /// * `hbd_source` — the coded-width u16 planes padded TRUE -> ALIGNED,
    ///   the same contract the non-superres bd10 arm establishes for the
    ///   bd10 consumers (MD funnel + level re-encode). This is C's unpacked
    ///   `enhanced_downscaled_pic` u16 buffer.
    /// * the returned u8 planes — `filtered_u16 >> 2`, C's `y_buffer` MSB
    ///   plane of the packed picture. Deriving them from the FILTERED u16
    ///   (not by u8-resizing an already-truncated source) is the
    ///   byte-parity-critical ordering — the two orders differ by the
    ///   low bits the filters accumulate.
    ///
    /// Picture statistics still read the FULL-resolution luma (truncated to
    /// u8, the same canvas C's `enhanced_pic->y_buffer` carries), matching
    /// the 8-bit arm's `superres_stats_luma` contract.
    pub(super) fn superres_downscale_420_hbd(
        &mut self,
        src: &HbdSuperresSrc,
    ) -> crate::EncodeResult<(Vec<u8>, Vec<u8>, Vec<u8>)> {
        let (uw, th) = (self.upscaled_width as usize, self.true_height as usize);
        let cw = self.true_width as usize;
        let (ucw, uch) = (uw.div_ceil(2), th.div_ceil(2));
        let ccw = cw.div_ceil(2);
        let bd = i32::from(self.bit_depth);
        // The filtered coded-width u16 planes.
        let mut yd = svtav1_types::try_vec![0u16; cw * th]?;
        let mut ud = svtav1_types::try_vec![0u16; ccw * uch]?;
        let mut vd = svtav1_types::try_vec![0u16; ccw * uch]?;
        svtav1_dsp::port_resize_hbd::highbd_resize_plane_horizontal(
            &src.y, th, uw, uw, &mut yd, cw, cw, bd,
        );
        svtav1_dsp::port_resize_hbd::highbd_resize_plane_horizontal(
            &src.u, uch, ucw, ucw, &mut ud, ccw, ccw, bd,
        );
        svtav1_dsp::port_resize_hbd::highbd_resize_plane_horizontal(
            &src.v, uch, ucw, ucw, &mut vd, ccw, ccw, bd,
        );
        // The coded u16 canvas is what every bd10 consumer reads — pad
        // TRUE(coded) -> ALIGNED exactly as the non-superres arm does.
        let (aw, ah) = (self.width as usize, self.height as usize);
        self.hbd_source = Some(HbdSource {
            y: pad_plane_replicate_u16(&yd, cw, cw, th, aw, ah)?,
            u: pad_plane_replicate_u16(&ud, ccw, ccw, uch, aw / 2, ah / 2)?,
            v: pad_plane_replicate_u16(&vd, ccw, ccw, uch, aw / 2, ah / 2)?,
        });
        // The u8 canvas, unpacked AFTER filtering (C's MSB plane of the
        // resized packed picture).
        let shift = u32::from(self.bit_depth - 8);
        let mut y8 = svtav1_types::try_vec![0u8; cw * th]?;
        for (d, &s) in y8.iter_mut().zip(yd.iter()) {
            *d = (s >> shift) as u8;
        }
        let mut u8p = svtav1_types::try_vec![0u8; ccw * uch]?;
        let mut v8p = svtav1_types::try_vec![0u8; ccw * uch]?;
        for ((du, dv), (&su, &sv)) in u8p
            .iter_mut()
            .zip(v8p.iter_mut())
            .zip(ud.iter().zip(vd.iter()))
        {
            *du = (su >> shift) as u8;
            *dv = (sv >> shift) as u8;
        }
        // Full-resolution luma for the pre-scaling picture statistics —
        // the same u8-truncated canvas the 8-bit arm stores.
        let mut orig = svtav1_types::try_vec![0u8; uw * th]?;
        for r in 0..th {
            for c in 0..uw {
                orig[r * uw + c] = (src.y[r * uw + c] >> shift) as u8;
            }
        }
        self.superres_stats_luma = Some((orig, uw, th));
        Ok((y8, u8p, v8p))
    }

    pub(super) fn superres_config_error(&self) -> Option<&'static str> {
        let denom = self.superres_denom?;
        if !(9..=16).contains(&denom) {
            return Some("SuperresDenom must be 9..=16");
        }
        // Loop restoration runs on the UPSCALED frame in C
        // (`svt_av1_superres_upscale_frame` sits between CDEF and LR,
        // cdef_process.c:152); this port still searches/applies LR at the
        // coded width, so the two would disagree. Restoration is off for
        // allintra presets >= 7 (`seq_tools_for_preset`: wn = 0 there), which
        // is where the superres gate lives until the upscaled-LR wiring lands.
        let tools = crate::speed_config::seq_tools_for_preset(
            self.speed_config.preset,
            self.gop.intra_period == 1,
            self.width as usize * self.height as usize,
        );
        if tools.enable_restoration
            && !crate::frame_geom::small_frame_disables_restoration(
                &crate::frame_geom::FrameDims::new(
                    self.upscaled_width as usize,
                    self.true_height as usize,
                ),
            )
        {
            return Some(
                "superres is not wired for frames that run loop restoration (allintra preset <= 6, \
                 except small frames where restoration is disabled) — C runs LR on the \
                 UPSCALED frame; use preset >= 7",
            );
        }
        if !matches!(self.bit_depth, 8 | 10) {
            return Some(
                "superres supports 8/10-bit only — C v4.2.0 rejects every other depth at \
                 encoder init (svt_av1_verify_settings, Globals/enc_settings.c:460), so no \
                 oracle exists outside that envelope [C: rejects]",
            );
        }
        // Mono pipelines never reach `superres_downscale_420` — the mono core
        // feeds the full-width luma straight to `encode_frame_impl`, which
        // would code a left-cropped canvas and signal the normative upscale
        // over it: plausible-but-wrong output, the class this encoder refuses
        // everywhere else. (Measured 2026-09-21: `SVTAV1_MONO=1
        // SVTAV1_SUPERRES=16` emitted a 269-byte stream whose decode showed
        // the left half stretched, not the downscaled source.) C has no mono
        // mode at all, so this is the port's own extension surface.
        let mono = !self.chroma_420
            && !matches!(
                self.chroma_format,
                Some(svtav1_types::chroma::ChromaFormat::Yuv444)
                    | Some(svtav1_types::chroma::ChromaFormat::Yuv422)
            );
        if mono {
            return Some(
                "superres is not wired for monochrome — the mono entry has no downscale arm \
                 and would code a left-cropped plane under an upscale header (C has no mono \
                 mode at any depth; this is a port-extension gap, not a C envelope)",
            );
        }
        // Inter frames under superres are refused: the measured surface is
        // stills/KEY frames (`tools/superres_gate.sh` and its bd10 arm drive
        // single-frame encodes, and the raw-sequence harness never applies a
        // denominator). C accepts superres on inter frames
        // (`--superres-denom` vs `--superres-kf-denom`), so this is capability
        // debt, not a C envelope: the per-reference geometry under a changing
        // coded width (`frame_size_with_refs`, render-vs-frame-size) is
        // decoder-ungated here. A key frame in a GOP pipeline is still fine —
        // the predicate is the frame type, not `intra_period`.
        if !self.gop.is_key_frame(self.frame_count) {
            return Some(
                "superres on an INTER frame is not implemented: stills are the measured \
                 surface and the reference-geometry signaling under a changing coded width \
                 is decoder-ungated (C accepts it — this is a port capability gap, not a \
                 C envelope) [C: accepts]",
            );
        }
        None
    }
}
