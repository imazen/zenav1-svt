use super::*;

impl EncodePipeline {
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
