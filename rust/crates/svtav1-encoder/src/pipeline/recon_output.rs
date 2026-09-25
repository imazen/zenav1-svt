use super::*;

impl EncodePipeline {
    #[inline(always)]
    pub(super) fn emit_recon_output(
        &mut self,
        display_order: u64,
        w: usize,
        h: usize,
        ss_x: usize,
        recon: &Vec<u8>,
        film_grain: &Option<crate::noise_gen::FilmGrainParams>,
        u_recon: &Vec<u8>,
        v_recon: &Vec<u8>,
        recon10: Option<(Vec<u16>, Vec<u16>, Vec<u16>)>,
        decoder_output8: Option<(Vec<u8>, Vec<u8>, Vec<u8>)>,
        out8: Option<(Vec<u8>, Vec<u8>, Vec<u8>)>,
        out10: Option<(Vec<u16>, Vec<u16>, Vec<u16>)>,
    ) {
        if self.recon_output {
            // Output-only replay must not alter the DPB or later frame decisions.
            self.last_recon_display_order = Some(display_order);
            let rec_planes = match out8 {
                // Under superres `out8` already holds the upscaled planes of
                // whichever canvas a decoder displays (replayed filters when
                // they ran, search recon otherwise).
                Some(planes) => planes,
                None => decoder_output8
                    .unwrap_or_else(|| (recon.clone(), u_recon.clone(), v_recon.clone())),
            };
            self.recon_frames
                .push_back((display_order, rec_planes.clone()));
            self.last_recon = Some(rec_planes);
            // Issue #13: the 10-bit final recon (deblock -> CDEF -> LR all
            // applied to the 10-bit canvas), normatively upscaled to the
            // output geometry under superres (`out10`); the coded canvas is
            // the output geometry itself when superres is off.
            self.last_recon10_final = out10.or(recon10);
            if let Some(fg) = film_grain.as_ref().filter(|fg| fg.apply_grain) {
                let stride = if self.superres_denom.is_some() {
                    self.upscaled_width as usize
                } else {
                    w
                };
                if self.bit_depth == 10 {
                    if let Some((y, u, v)) = self.last_recon10_final.as_mut() {
                        crate::film_grain_synthesis::add_grain_for_output(
                            fg,
                            [y, u, v],
                            [stride, stride >> ss_x, stride >> ss_x],
                            stride,
                            h,
                            10,
                        );
                    }
                } else if let Some((y, u, v)) = self.last_recon.as_mut() {
                    let mut planes = [
                        y.iter().map(|&v| u16::from(v)).collect::<Vec<_>>(),
                        u.iter().map(|&v| u16::from(v)).collect(),
                        v.iter().map(|&v| u16::from(v)).collect(),
                    ];
                    let [gy, gu, gv] = &mut planes;
                    crate::film_grain_synthesis::add_grain_for_output(
                        fg,
                        [gy, gu, gv],
                        [stride, stride >> ss_x, stride >> ss_x],
                        stride,
                        h,
                        8,
                    );
                    for (dst, src) in [y, u, v].into_iter().zip(planes) {
                        for (d, s) in dst.iter_mut().zip(src) {
                            *d = s as u8;
                        }
                    }
                }
            }
        }
    }

    #[inline(always)]
    pub(super) fn build_padded_ref(
        &self,
        chroma: Option<(&[u8], &[u8])>,
        w: usize,
        h: usize,
        ss_x: usize,
        ss_y: usize,
        recon: &Vec<u8>,
        u_recon: &Vec<u8>,
        v_recon: &Vec<u8>,
        padded_ref_hbd: Option<crate::picture::PaddedRefHbd>,
        recon_msb8: &Option<(Vec<u8>, Vec<u8>, Vec<u8>)>,
    ) -> alloc::boxed::Box<crate::picture::PaddedRef> {
        let padded_ref = {
            let (rw, rh) = (w, h);
            // C's reference-picture border is `super_block_size + 32`
            // (enc_handle.c:1212-1217), NOT `scs->border` — see
            // [`crate::picture::ref_pic_border`].
            let rb = crate::picture::ref_pic_border(self.sb_size, self.superres_denom.is_some());
            // The u8 planes the reference exposes are the 10-bit recon's
            // MSBs when a 10-bit canvas exists (`recon_msb8` above); the
            // u8-domain `recon` is only ever the reference on a bd8 encode.
            let (y8, u8r, v8r) = match recon_msb8.as_ref() {
                Some((my, mu, mv)) => (my.as_slice(), mu.as_slice(), mv.as_slice()),
                None => (recon.as_slice(), u_recon.as_slice(), v_recon.as_slice()),
            };
            let y = crate::picture::PaddedPlane::from_plane(y8, rw, rh, rb);
            let uv = if chroma.is_some() {
                // C `(border + (1 << ss_x) - 1) >> ss_x` (:1102-1112).
                let cb = rb.div_ceil(1 << ss_x);
                let mut cv =
                    crate::picture::PaddedPlane::from_plane(v8r, rw >> ss_x, rh >> ss_y, cb);
                let mut cu =
                    crate::picture::PaddedPlane::from_plane(u8r, rw >> ss_x, rh >> ss_y, cb);
                // C's recon `buffer_alloc` is `[y][u][v]` contiguous: a
                // maximally UMV-clamped chroma read past `u`'s region
                // answers with `v`'s margin bytes. `v` is the last
                // region, so its overread runs off the calloc — the
                // zero tail keeps the port deterministic there.
                cu.extend_tail(&cv.buf);
                let tail = cv.buf.len();
                cv.extend_tail_zeros(tail);
                Some((cu, cv))
            } else {
                None
            };
            alloc::boxed::Box::new(crate::picture::PaddedRef {
                y,
                uv,
                hbd: padded_ref_hbd,
            })
        };
        padded_ref
    }
}

impl EncodePipeline {
    #[inline(always)]
    pub(super) fn take_recon10(
        &self,
        chroma: Option<(&[u8], &[u8])>,
    ) -> Option<(Vec<u16>, Vec<u16>, Vec<u16>)> {
        // ---- bd10 post-filter canvas ------------------------------------
        // At 10 bits C runs the WHOLE post-MD filter chain on the 16-bit
        // recon against the 16-bit source, and the THREE SEARCHES in that
        // chain — the deblock LEVEL search, the CDEF strength search and the
        // Wiener LR taps — each write frame-header syntax. Running them at 8
        // bits is therefore a bitstream divergence, not just a recon
        // approximation. This carries the true 10-bit planes through the
        // chain in parallel with the u8 ones; the u8 chain still produces the
        // output/DPB recon, unchanged, and bd8 never enters any of it.
        //
        // Built BEFORE the LF-level decision because the deblock-level search
        // reads the UNFILTERED recon (each trial filters a scratch copy — C
        // re-instates the frame from `temp_lf_recon_buffer` after every
        // try_filter_frame, deblocking_filter.c:828).
        //
        // `Some` iff this frame produced a complete 10-bit recon (the bd10
        // re-encode gate above). When it declined, the searches fall back to
        // the u8 chain exactly as before.
        let recon10: Option<(Vec<u16>, Vec<u16>, Vec<u16>)> = match (
            self.bit_depth,
            self.last_recon10_y.as_ref(),
            self.last_recon10_uv.as_ref(),
        ) {
            (10, Some(y10), Some((u10, v10))) if chroma.is_some() => {
                Some((y10.clone(), u10.clone(), v10.clone()))
            }
            (10, Some(y10), _) if chroma.is_none() => Some((y10.clone(), Vec::new(), Vec::new())),
            _ => None,
        };
        recon10
    }

    #[inline(always)]
    pub(super) fn decoder_chroma_recon_stage(
        &self,
        w: usize,
        h: usize,
        acw: usize,
        filter_chroma: bool,
        lf_sharp_eff: u8,
        mut deblock_geom: crate::deblock::DeblockGeom,
        recon10: &mut Option<(Vec<u16>, Vec<u16>, Vec<u16>)>,
        lf_levels: crate::deblock::LfLevels,
        cdef_params: &crate::cdef::CdefPick,
        decoder_chroma_recon: bool,
        output_restoration: Option<crate::restoration::FrameRestInfo>,
        decoder_output8: &mut Option<(Vec<u8>, Vec<u8>, Vec<u8>)>,
    ) {
        if decoder_chroma_recon {
            deblock_geom.use_decoder_chroma_bounds();
            macro_rules! decoder_recon {
                ($input:expr, $deblock:path, $cdef:path $(, $depth:expr)?) => {{
                    let (mut y, mut u, mut v) = $input;
                    $deblock(&mut y, &mut u, &mut v, w, h, filter_chroma, &deblock_geom,
                        &lf_levels, lf_sharp_eff $(, $depth)?);
                    let before_cdef = output_restoration.as_ref().map(|_| (y.clone(), u.clone(), v.clone()));
                    $cdef(&mut y, &mut u, &mut v, w, h, filter_chroma, &deblock_geom,
                        &cdef_params $(, $depth)?);
                    if let (Some(info), Some((py, pu, pv))) = (output_restoration.as_ref(), before_cdef) {
                        let bounds = crate::restoration::save_lr_boundaries_bd(
                            &py, &pu, &pv, &y, &u, &v, self.true_width as usize,
                            self.true_height as usize, w, acw, filter_chroma);
                        crate::restoration::apply_restoration_frame_bd(
                            &mut y, &mut u, &mut v, self.true_width as usize,
                            self.true_height as usize, w, acw, filter_chroma, info, &bounds, self.bit_depth);
                    }
                    (y, u, v)
                }};
            }
            if self.bit_depth == 10 {
                if let (Some(y), Some((u, v))) =
                    (self.last_recon10_y.as_ref(), self.last_recon10_uv.as_ref())
                {
                    *recon10 = Some(decoder_recon!(
                        (y.clone(), u.clone(), v.clone()),
                        crate::deblock::apply_deblock_frame_hbd,
                        crate::cdef::apply_cdef_frame_hbd,
                        self.bit_depth
                    ));
                }
            } else if let Some(unfiltered) = self.last_recon_unfiltered.as_ref() {
                *decoder_output8 = Some(decoder_recon!(
                    unfiltered.clone(),
                    crate::deblock::apply_deblock_frame,
                    crate::cdef::apply_cdef_frame
                ));
            }
        }
    }

    #[inline(always)]
    pub(super) fn build_padded_ref_hbd(
        &self,
        chroma: Option<(&[u8], &[u8])>,
        w: usize,
        h: usize,
        ss_x: usize,
        acw: usize,
        ach: usize,
        recon10: &Option<(Vec<u16>, Vec<u16>, Vec<u16>)>,
    ) -> Option<crate::picture::PaddedRefHbd> {
        // C `pad_ref_and_set_flags` again, on the 16-bit picture: at
        // `bit_depth > 8` C's reference IS the 10-bit buffer and
        // `svt_aom_generate_padding16_bit` pads it from the same call site
        // (enc_dec_process.c:1088-1112). Built HERE, before the `recon_output`
        // block, for two reasons: that block MOVES `recon10` into
        // `last_recon10_final`, and it also applies FILM GRAIN, which is an
        // output-only transform that must never reach the DPB.
        let padded_ref_hbd = recon10.as_ref().map(|(y10, u10, v10)| {
            let rb = crate::picture::ref_pic_border(self.sb_size, self.superres_denom.is_some());
            let y = crate::picture::PaddedPlaneHbd::from_plane(y10, w, h, rb);
            let uv = if chroma.is_some() {
                let cb = rb.div_ceil(1 << ss_x);
                let mut cv = crate::picture::PaddedPlaneHbd::from_plane(v10, acw, ach, cb);
                let mut cu = crate::picture::PaddedPlaneHbd::from_plane(u10, acw, ach, cb);
                // Same contiguous `[u][v]` layout as the 8-bit reference —
                // see the `padded_ref` block below.
                cu.extend_tail(&cv.buf);
                let tail = cv.buf.len();
                cv.extend_tail_zeros(tail);
                Some((cu, cv))
            } else {
                None
            };
            crate::picture::PaddedRefHbd { y, uv }
        });
        padded_ref_hbd
    }

    #[inline(always)]
    pub(super) fn stash_pa_picture(
        &mut self,
        pcs: &PictureControlSet,
        pa_cur: Option<Box<crate::inter_me_arm::PaPicture>>,
    ) {
        // The PA (picture-analysis) reference the NEXT frame's open-loop
        // motion search reads — this frame's padded SOURCE pyramid, not its
        // recon. `None` in still mode, where no later frame exists.
        if let Some(cur) = pa_cur {
            let cur = alloc::sync::Arc::from(cur);
            // The pyramid `pa_ref` displaces is two frames back: the search
            // only ever reads `pa_ref` (the PREVIOUS frame) against `pa_cur`,
            // so nothing can still be looking at it UNLESS a GM slot below
            // still names it. `Arc::into_inner` answers exactly that question,
            // and gives the allocation back when the answer is no.
            if let Some(old) = self.pa_ref.take() {
                self.pa_scratch = alloc::sync::Arc::into_inner(old).map(alloc::boxed::Box::new);
            }
            // Mirror the DPB refresh into the PA slots, so a later frame's
            // global-motion search can reach the plane for ANY reference its
            // `ref_dpb_index` names — not only the nearest one.
            //
            // This used to be gated on `gm_level_for_frame(false) != 0`,
            // because GM was the slots' only reader and the pyramids are not
            // free to retain. From 2026-09-13 the open-loop ME itself is a
            // reader: C searches `ref_list0_count_try` references
            // (`me_process.c:212`), so a frame 2 with `l0cnt = 2` needs BOTH
            // frame 1's and frame 0's pyramids — not only `pa_ref`'s.
            for slot in 0..8 {
                if pcs.refresh_frame_flags & (1 << slot) != 0 {
                    let evicted = self.pa_slots[slot].replace(alloc::sync::Arc::clone(&cur));
                    if self.pa_scratch.is_none()
                        && let Some(e) = evicted
                    {
                        self.pa_scratch =
                            alloc::sync::Arc::into_inner(e).map(alloc::boxed::Box::new);
                    }
                }
            }
            self.pa_ref = Some(cur);
        }
    }
}
