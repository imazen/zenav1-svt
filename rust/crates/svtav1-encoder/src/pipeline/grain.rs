use super::*;

impl EncodePipeline {
    pub(super) fn film_grain_seed(&self) -> u16 {
        // The zero transition restarts at 7391, so use the actual recurrence's
        // period rather than applying a one-frame correction after wrapping.
        const PERIOD: u64 = 63421;
        7391u16.wrapping_add(3381u16.wrapping_mul((self.frame_count % PERIOD) as u16))
    }

    pub(super) fn film_grain_reference(
        &self,
        fg: &crate::entropy::obu::FilmGrainParams,
        map: [u8; 7],
        is_b: bool,
    ) -> Option<u8> {
        if self.film_grain.ignore_ref {
            return None;
        }
        // C compares LIST_1[0] (BWDREF), but signals ALTREF's map index.
        // Preserve entropy_coding.c:3126-3131, including its explicit TODO.
        for (compare_kind, signal_kind) in [(0usize, 0usize), (4, 6)] {
            if compare_kind == 4 && !is_b {
                break;
            }
            let reference = self.grain_references[map[compare_kind] as usize].as_ref();
            if reference.is_some_and(|r| crate::film_grain_config::parameters_equal(fg, r)) {
                return Some(map[signal_kind]);
            }
        }
        None
    }

    pub(super) fn validate_film_grain(&self) -> EncodeResult<()> {
        self.film_grain
            .validate()
            .map_err(|why| whereat::at!(EncodeError::UnsupportedConfig(why)))?;
        let enabled =
            self.film_grain.enabled() || (self.hdr.is_fork() && self.hdr.noise_strength > 0);
        if enabled && (!self.chroma_420 || !matches!(self.bit_depth, 8 | 10)) {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "C film grain requires 8/10-bit 4:2:0"
            )));
        }
        // bd10 + superres: the denoise pass runs on the u8 canvas BEFORE the
        // downscale (`prepare_film_grain` -> `superres_downscale_420`), but the
        // native-10-bit arm produces that canvas only AFTER filtering u16 —
        // the order C's packed-buffer pipeline guarantees. Feeding the
        // denoiser a canvas that does not exist yet has no honest answer, so
        // the combination is refused rather than denoising a different
        // picture than the one that gets coded. A supplied grain table or the
        // HDR-fork noise path never denoises (same early-out
        // `prepare_film_grain` takes), so they stay allowed.
        let denoise_would_run = self.film_grain.table.is_none()
            && !(self.hdr.is_fork() && self.hdr.noise_strength > 0)
            && self.film_grain.denoise_strength > 0;
        if denoise_would_run && self.bit_depth == 10 && self.superres_denom.is_some() {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "film-grain denoise with 10-bit superres is not implemented: the native-input \
                 u8 canvas exists only after the u16 downscale, which runs after denoise — \
                 C's packed-buffer order has no u16 equivalent here [C: accepts]",
            )));
        }
        if self
            .grain_sequence_present
            .is_some_and(|present| present != enabled)
            && !self.gop.is_key_frame(self.frame_count)
        {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "film grain sequence presence can change only on a key frame"
            )));
        }
        Ok(())
    }

    pub(super) fn prepare_film_grain(
        &mut self,
        y: &[u8],
        u: &[u8],
        v: &[u8],
        y_stride: usize,
    ) -> EncodeResult<Option<([Vec<u8>; 3], usize)>> {
        if self.film_grain.table.is_some()
            || (self.hdr.is_fork() && self.hdr.noise_strength > 0)
            || self.film_grain.denoise_strength == 0
        {
            return Ok(None);
        }
        self.stop
            .check()
            .map_err(EncodeError::from)
            .map_err(whereat::at)?;
        let tw = self.upscaled_width as usize;
        let th = self.true_height as usize;
        // C pads to the mi grid before picture_pre_processing_operations.
        let w = tw.div_ceil(8) * 8;
        let h = th.div_ceil(8) * 8;
        let shift = self.bit_depth - 8;
        let strides = [w, w / 2, w / 2];
        let source = if let Some(src) = self.hbd_source.as_ref() {
            [src.y.clone(), src.u.clone(), src.v.clone()]
        } else {
            let padded = [
                pad_plane_replicate(y, y_stride, tw, th, w, h)?,
                pad_plane_replicate(
                    u,
                    tw.div_ceil(2),
                    tw.div_ceil(2),
                    th.div_ceil(2),
                    w / 2,
                    h / 2,
                )?,
                pad_plane_replicate(
                    v,
                    tw.div_ceil(2),
                    tw.div_ceil(2),
                    th.div_ceil(2),
                    w / 2,
                    h / 2,
                )?,
            ];
            padded.map(|p| p.into_iter().map(|v| u16::from(v) << shift).collect())
        };
        let (denoised, params, _status) = crate::film_grain_denoise::denoise_and_model(
            source.each_ref().map(Vec::as_slice),
            w,
            h,
            strides,
            self.bit_depth,
            self.film_grain.denoise_strength,
            self.film_grain.adaptive,
            self.film_grain_seed(),
        );
        self.stop
            .check()
            .map_err(EncodeError::from)
            .map_err(whereat::at)?;
        let apply = self.film_grain.denoise_apply && params.apply_grain;
        self.prepared_grain = Some(params);
        if !apply {
            return Ok(None);
        }
        // Keep C's denoised padding for the encode; repadding a cropped
        // denoised picture would replace actual filter output at the edges.
        let (ow, oh) = if self.superres_denom.is_some() {
            (tw, th)
        } else {
            (w, h)
        };
        let tight: [Vec<u8>; 3] = core::array::from_fn(|c| {
            let sub = usize::from(c > 0);
            let pw = ow.div_ceil(1 << sub);
            let ph = oh.div_ceil(1 << sub);
            let mut out = Vec::with_capacity(pw * ph);
            for row in 0..ph {
                out.extend(
                    denoised[c][row * strides[c]..row * strides[c] + pw]
                        .iter()
                        .map(|&v| (v >> shift) as u8),
                );
            }
            out
        });
        if self.bit_depth == 10 {
            let [y, u, v] = denoised;
            self.hbd_source = Some(HbdSource { y, u, v });
        }
        Ok(Some((tight, ow)))
    }
}
