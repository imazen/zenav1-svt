//! The frame-level driver for the ported open-loop motion search.
//!
//! [`crate::inter_me`] is a wholesale port of `motion_estimation.c` and, until
//! this module, **nothing in the encoder called it** — the same shape
//! `crate::inter_pred_arm` closed for the reconstruction convolve. This is the
//! adapter that turns the pipeline's per-frame state (a source plane, the
//! previous frame's source, a preset and a qp) into the
//! `MePicParams` / `MeContext` / `MeSrcBufs` / `MeRefs` set
//! [`crate::inter_me::motion_estimation_b64`] takes, and runs it once per b64.
//!
//! # SVT's ME is OPEN LOOP — the reference is a SOURCE, not a recon
//!
//! `me_process.c:185-203` slices its reference out of the **PA reference
//! picture**, which `reference_object.c:242-250` documents as pointing
//! "directly to the Luma input samples of the app data". So the search
//! compares this frame's source against the PREVIOUS FRAME'S SOURCE, at three
//! resolutions, and never sees a reconstruction. [`PaPicture`] is that set;
//! the DPB's `padded` recon (`crate::picture::PaddedRef`) is a different
//! buffer with a different purpose (motion COMPENSATION), and mixing the two
//! up would silently change every MV.
//!
//! # Borders, and why they are three different numbers
//!
//! | plane | C border | source |
//! |---|--:|---|
//! | full resolution | `scs->border` = `BLOCK_SIZE_64 + 4` = 68 | `reference_object.c:252`, `enc_handle.c:4256` |
//! | quarter | `scs->b64_size >> 1` = 32 | `reference_object.c:263` |
//! | sixteenth | `scs->b64_size >> 2` = 16 | `reference_object.c:274` |
//!
//! Each is produced by `svt_aom_generate_padding` after its own decimation
//! (`pic_analysis_process.c:1500-1546`: full -> quarter with `downsample_2d`
//! step 2, then quarter -> sixteenth with step 2), which is the tier-1-gated
//! [`crate::port_preanalysis::generate_padding`] /
//! [`crate::port_preanalysis::downsample_2d`] pair.

use crate::inter_me::context::{
    MeB64Output, MeContext, MeDsRef, MePicParams, MeRefs, MeSrcBufs, MeType, Plane,
};
use crate::inter_me::motion_estimation_b64;
use crate::port_enc_mode_config::ResolutionRange;
use crate::port_enc_mode_config::me::{MeDerivInputs, apply_me_signals, sig_deriv_me};
use crate::port_preanalysis::{downsample_2d, generate_padding};
use alloc::vec;
use alloc::vec::Vec;

/// C `scs->border` (`enc_handle.c:4256`), the PA reference's full-resolution
/// margin.
pub const PA_BORDER: usize = 64 + 4;
/// C `scs->b64_size >> 1` (`reference_object.c:263`).
pub const PA_BORDER_QUARTER: usize = 32;
/// C `scs->b64_size >> 2` (`reference_object.c:274`).
pub const PA_BORDER_SIXTEENTH: usize = 16;

/// One padded luma plane of a PA picture.
#[derive(Clone, Debug)]
pub struct PaPlane {
    /// The whole padded allocation.
    pub buf: Vec<u8>,
    /// Index of pixel (0, 0).
    pub org: usize,
    pub stride: usize,
    pub width: usize,
    pub height: usize,
    pub border: usize,
}

impl PaPlane {
    /// Copy `src` (tightly strided at `stride`) into a fresh allocation with a
    /// `border`-wide replicated margin on all four sides.
    fn from_plane(
        src: &[u8],
        src_stride: usize,
        width: usize,
        height: usize,
        border: usize,
    ) -> Self {
        let stride = width + 2 * border;
        let org = border * stride + border;
        let mut buf = vec![0u8; stride * (height + 2 * border)];
        for r in 0..height {
            buf[org + r * stride..org + r * stride + width]
                .copy_from_slice(&src[r * src_stride..r * src_stride + width]);
        }
        generate_padding(&mut buf, org, stride, width, height, border, border);
        Self {
            buf,
            org,
            stride,
            width,
            height,
            border,
        }
    }

    /// Decimate `self` by `step` and pad the result to `border`.
    fn decimate(&self, step: usize, border: usize) -> Self {
        let (dw, dh) = (self.width / step, self.height / step);
        let stride = dw + 2 * border;
        let org = border * stride + border;
        let mut buf = vec![0u8; stride * (dh + 2 * border)];
        downsample_2d(
            &self.buf[self.org..],
            self.stride,
            self.width,
            self.height,
            &mut buf[org..],
            stride,
            step,
        );
        generate_padding(&mut buf, org, stride, dw, dh, border, border);
        Self {
            buf,
            org,
            stride,
            width: dw,
            height: dh,
            border,
        }
    }

    fn view(&self) -> Plane<'_> {
        Plane {
            data: &self.buf,
            org: self.org,
            stride: self.stride,
            width: self.width as u16,
            height: self.height as u16,
            border: self.border as u16,
        }
    }
}

/// C's PA reference picture: the padded SOURCE luma at full, 1/4 and 1/16
/// resolution, plus the picture number the search's temporal scaling reads.
#[derive(Clone, Debug)]
pub struct PaPicture {
    pub full: PaPlane,
    pub quarter: PaPlane,
    pub sixteenth: PaPlane,
    pub picture_number: u64,
}

impl PaPicture {
    /// Build the three-level pyramid from one ALIGNED source luma plane.
    #[must_use]
    pub fn from_source(
        y: &[u8],
        y_stride: usize,
        width: usize,
        height: usize,
        picture_number: u64,
    ) -> Self {
        let full = PaPlane::from_plane(y, y_stride, width, height, PA_BORDER);
        let quarter = full.decimate(2, PA_BORDER_QUARTER);
        let sixteenth = quarter.decimate(2, PA_BORDER_SIXTEENTH);
        Self {
            full,
            quarter,
            sixteenth,
            picture_number,
        }
    }
}

/// The per-b64 open-loop ME results for one frame, in raster b64 order.
pub struct FrameMe {
    /// One entry per b64, `sb_row * sb_cols + sb_col`.
    pub per_b64: Vec<MeB64Output>,
    pub b64_cols: usize,
    pub b64_rows: usize,
    /// C `pcs->pa_me_data->max_refs` — the `me_mv_array` row stride.
    pub max_refs: usize,
    /// C `pcs->enable_me_8x8` / `enable_me_16x16`, which
    /// [`crate::port_md::predicates::get_me_block_offset`] needs to resolve a
    /// block origin to a slot in these arrays.
    pub enable_me_8x8: bool,
    pub enable_me_16x16: bool,
}

impl FrameMe {
    /// The full-pel MV this frame's search chose for the block at
    /// `(org_x, org_y)` of size `bsize`, against `[list][ref_idx]`.
    ///
    /// The units are FULL PEL, which is what `me_mv_array` stores; C multiplies
    /// by 8 at injection (`mode_decision.c:2323-2325`).
    #[must_use]
    pub fn mv_for(
        &self,
        org_x: usize,
        org_y: usize,
        bsize: u8,
        list: usize,
        ref_idx: usize,
        max_l0: usize,
    ) -> Option<svtav1_types::motion::Mv> {
        let (b64_x, b64_y) = (org_x / 64, org_y / 64);
        if b64_x >= self.b64_cols || b64_y >= self.b64_rows {
            return None;
        }
        let out = &self.per_b64[b64_y * self.b64_cols + b64_x];
        let off = crate::port_md::predicates::get_me_block_offset(
            (org_x % 64) as u32,
            (org_y % 64) as u32,
            bsize,
            self.enable_me_8x8,
            self.enable_me_16x16,
        ) as usize;
        // C `me_results->me_mv_array[me_block_offset * max_refs +
        // (inter_direction ? max_l0 : 0) + ref_idx]` (mode_decision.c:2323).
        let slot = off * self.max_refs + if list == 1 { max_l0 } else { 0 } + ref_idx;
        out.me_mv_array.get(slot).copied()
    }
}

/// Everything the frame-level search needs that is not a picture.
#[derive(Clone, Copy, Debug)]
pub struct FrameMeParams {
    /// C `enc_mode`.
    pub enc_mode: u8,
    /// CLI qp 0..63 (C `pcs->picture_qp`, which `sig_deriv_me`'s qp-based
    /// threshold scaling reads).
    pub qp: u8,
    /// ALIGNED luma dims.
    pub width: usize,
    pub height: usize,
    /// C `pcs->picture_number`.
    pub picture_number: u64,
    /// C `frame_is_boosted(pcs)`.
    pub frame_is_boosted: bool,
    /// C `pcs->hierarchical_levels`.
    pub hierarchical_levels: u8,
}

/// Run C's open-loop motion estimation over the whole frame.
///
/// `cur` is THIS frame's PA picture and `reference` the previous frame's; both
/// are built by [`PaPicture::from_source`]. One reference in list 0 (the
/// low-delay-P shape the inter campaign encodes), which is why
/// `num_of_ref_pic_to_search` is `[1, 0]`.
#[must_use]
pub fn run_frame_me(cur: &PaPicture, reference: &PaPicture, p: FrameMeParams) -> FrameMe {
    // C `pcs->pa_me_data->max_*` (pcs.c) for a single-reference low-delay
    // list-0 configuration; the arrays are sized by these, so they only need
    // to be >= what the search writes.
    const MAX_CAND: usize = 23;
    const MAX_REFS: usize = 7;
    const MAX_L0: usize = 4;

    let input_resolution = ResolutionRange::from_luma_area((p.width * p.height) as u32);
    // C `svt_aom_sig_deriv_me` (enc_mode_config.c) — this is what installs the
    // HME/ME search areas. A default `MeContext` has a ZERO search area, so
    // skipping it would silently pin every MV to (0, 0).
    let signals = sig_deriv_me(MeDerivInputs {
        enc_mode: p.enc_mode as i8,
        sc_class5: 0,
        input_resolution,
        rtc_tune: false,
        is_base: p.frame_is_boosted,
        hierarchical_levels: p.hierarchical_levels,
        // enc_mode_config.c:1987-1999 sets all four unconditionally.
        enable_hme_flag: 1,
        enable_hme_level0_flag: 1,
        enable_hme_level1_flag: 1,
        enable_hme_level2_flag: 1,
        use_best_me_unipred_cand_only: 0,
        me_qp_based_th_scaling: false,
        hme_qp_based_th_scaling: false,
        qp: u32::from(p.qp),
        safe_limit_nref: 0,
        safe_limit_zz_th: 0,
    });

    let pic = MePicParams {
        picture_number: p.picture_number,
        aligned_width: p.width as i16,
        aligned_height: p.height as i16,
        enhanced_width: p.width as u32,
        enhanced_height: p.height as u32,
        ahd_error: u32::MAX,
        input_resolution: input_resolution.as_u8(),
        enable_me_8x8: true,
        enable_me_16x16: true,
        max_number_of_pus_per_sb: 85,
        hierarchical_levels: p.hierarchical_levels,
        similar_brightness_refs: false,
        frame_is_boosted: p.frame_is_boosted,
        frame_is_leaf: false,
        gm_enabled: false,
        only_l_bwd: false,
        max_cand: MAX_CAND,
        max_refs: MAX_REFS,
        max_l0: MAX_L0,
        b64_geom_width: p.width as u32,
        b64_geom_height: p.height as u32,
        input_width: p.width as u16,
        input_height: p.height as u16,
    };

    let refs = MeRefs {
        arr: [
            [
                Some(MeDsRef {
                    picture: reference.full.view(),
                    quarter: reference.quarter.view(),
                    sixteenth: reference.sixteenth.view(),
                    picture_number: reference.picture_number,
                }),
                None,
                None,
                None,
            ],
            [None, None, None, None],
        ],
    };

    let b64_cols = p.width.div_ceil(64);
    let b64_rows = p.height.div_ceil(64);
    let mut per_b64 = Vec::with_capacity(b64_cols * b64_rows);
    for b64_y in 0..b64_rows {
        for b64_x in 0..b64_cols {
            let (ox, oy) = (b64_x * 64, b64_y * 64);
            // C `me_process.c:172-203`: the three source buffers are SLICES of
            // the padded picture at the b64 origin, at the picture's stride.
            let src = MeSrcBufs {
                b64: &cur.full.buf[cur.full.org + oy * cur.full.stride + ox..],
                b64_stride: cur.full.stride,
                quarter: &cur.quarter.buf
                    [cur.quarter.org + (oy >> 1) * cur.quarter.stride + (ox >> 1)..],
                quarter_stride: cur.quarter.stride,
                sixteenth: &cur.sixteenth.buf
                    [cur.sixteenth.org + (oy >> 2) * cur.sixteenth.stride + (ox >> 2)..],
                sixteenth_stride: cur.sixteenth.stride,
            };
            let mut me = MeContext::default();
            apply_me_signals(&mut me, &signals);
            me.num_of_list_to_search = 1;
            me.num_of_ref_pic_to_search = [1, 0];
            me.me_type = MeType::OpenLoop;
            let mut out = MeB64Output::new(MAX_CAND, MAX_REFS);
            motion_estimation_b64(&pic, ox as u32, oy as u32, &mut me, &src, &refs, &mut out);
            per_b64.push(out);
        }
    }

    FrameMe {
        per_b64,
        b64_cols,
        b64_rows,
        max_refs: MAX_REFS,
        enable_me_8x8: pic.enable_me_8x8,
        enable_me_16x16: pic.enable_me_16x16,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reference cell's two frames, exactly as `tools/identity_run`'s
    /// `gradient` content and its `SVTAV1_FRAME_SHIFT` translate build them.
    fn reference_cell(w: usize, h: usize, shift: usize) -> (Vec<u8>, Vec<u8>) {
        let f0: Vec<u8> = (0..h)
            .flat_map(|r| (0..w).map(move |c| (((r * 255) / h) as u8) ^ (((c * 3) & 0x3f) as u8)))
            .collect();
        let mut f1 = vec![0u8; w * h];
        for r in 0..h {
            for c in 0..w {
                f1[r * w + c] = f0[r * w + c.saturating_sub(shift)];
            }
        }
        (f0, f1)
    }

    /// **The FRAME-LEVEL driver recovers C's MV** — the same result
    /// `pipeline::inter_decision_probe::the_ports_own_svt_motion_search_finds_
    /// cs_mv_on_the_reference_cell` gets from a hand-built call, now through
    /// the arm the encoder will use.
    ///
    /// C's `SVT_CINTER_OUT` on `gradient 64x64 q40 p6 frames=2` prints
    /// `mv0=0,-24` eighth-pel = the full-pel `(-3, 0)` this asserts.
    ///
    /// Evidence tier 4 (`docs/WORKING-ON-THIS.md` §4): most of
    /// `motion_estimation.c` is `static`, so this is a reachability and wiring
    /// result, not a bit-exactness one. The kernels under it are tier 1
    /// (`tests/c_parity_inter_me.rs`).
    #[test]
    fn the_frame_level_me_arm_finds_cs_mv_on_the_reference_cell() {
        const W: usize = 64;
        const H: usize = 64;
        const SHIFT: usize = 3;
        let (f0, f1) = reference_cell(W, H, SHIFT);

        let pa_ref = PaPicture::from_source(&f0, W, W, H, 0);
        let pa_cur = PaPicture::from_source(&f1, W, W, H, 1);

        // The left margin is what makes the -3 match EXACT: `identity_run`
        // builds frame 1 by replicating column 0, and C's own PA reference
        // replicates the same way (`svt_aom_generate_padding`).
        assert_eq!(
            pa_ref.full.buf[pa_ref.full.org - 3],
            f0[0],
            "the PA reference's left margin must replicate column 0"
        );

        let me = run_frame_me(
            &pa_cur,
            &pa_ref,
            FrameMeParams {
                enc_mode: 6,
                qp: 40,
                width: W,
                height: H,
                picture_number: 1,
                frame_is_boosted: false,
                hierarchical_levels: 0,
            },
        );
        assert_eq!((me.b64_cols, me.b64_rows), (1, 1));

        // C `BLOCK_64X64` / `BLOCK_32X32` / `BLOCK_16X16` / `BLOCK_8X8`.
        for (bsize, bw) in [(12u8, 64usize), (9, 32), (6, 16), (3, 8)] {
            for oy in (0..H).step_by(bw) {
                for ox in (0..W).step_by(bw) {
                    let mv = me
                        .mv_for(ox, oy, bsize, 0, 0, 4)
                        .expect("every in-frame block has an ME slot");
                    assert_eq!(
                        (mv.x, mv.y),
                        (-(SHIFT as i16), 0),
                        "block {bw}x{bw} at ({ox},{oy}) must recover the cell's full-pel MV"
                    );
                }
            }
        }

        // POSITIVE CONTROL that the search area was installed: with the
        // signals bridge writing nothing a default `MeContext` has a ZERO
        // search area, in which no MV but (0,0) is reachable — so the
        // assertion above could only pass by the content being static.
        let (still0, still1) = (f0.clone(), f0.clone());
        let me_still = run_frame_me(
            &PaPicture::from_source(&still1, W, W, H, 1),
            &PaPicture::from_source(&still0, W, W, H, 0),
            FrameMeParams {
                enc_mode: 6,
                qp: 40,
                width: W,
                height: H,
                picture_number: 1,
                frame_is_boosted: false,
                hierarchical_levels: 0,
            },
        );
        assert_eq!(
            me_still.mv_for(0, 0, 12, 0, 0, 4).map(|m| (m.x, m.y)),
            Some((0, 0)),
            "an unmoved picture must give the zero MV — otherwise the -3 above \
             is noise, not a search result"
        );
    }
}
