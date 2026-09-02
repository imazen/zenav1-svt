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
use crate::port_enc_mode_config::enc_mode as enc_mode_c;
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
    /// C `pcs->pa_me_data->max_cand` — the `me_candidate_array` row stride.
    pub max_cand: usize,
    /// C `pcs->pa_me_data->max_l0` — the list-1 offset inside an
    /// `me_mv_array` row.
    pub max_l0: usize,
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

    /// The MV of ME candidate `cand` for the block at `(org_x, org_y)`, read
    /// out of the `me_mv_array` slot that candidate's OWN `direction` names —
    /// which is how C indexes it (`mode_decision.c:2320-2326`):
    ///
    /// ```text
    /// me_mv_array[me_block_offset * max_refs + (inter_direction ? max_l0 : 0) + ref_idx]
    /// ```
    ///
    /// Returns `(direction, mv)` in FULL PEL, or `None` for a block outside
    /// the picture, a candidate past `total_me_candidate_index`, or a BI_PRED
    /// candidate (which names two slots, not one).
    ///
    /// C `svt_aom_is_me_data_present` (mode_decision.c:179-198): does ANY
    /// surviving ME candidate for this block name `(list_idx, ref_idx)`?
    ///
    /// **A BI_PRED candidate counts for BOTH lists**, which is the whole
    /// reason this is not `cand_mv_for(..).is_some()`. MEASURED 2026-09-02
    /// on `gradient 64x64 q40 p8` frame 1: C's candidate array for the
    /// coded 64x64 is `[dir=1, dir=2]` — no list-0 UNIPRED entry — yet
    /// `is_me_data_present(list 0)` is TRUE because of the BI_PRED one, so
    /// `read_refine_me_mvs` refines a list-0 MV and `pme_search` takes its
    /// bail-to-ME arm. On `gradient 128x128 q40 p8` the array is `[dir=1]`
    /// alone, list 0 has no data, and PME runs its full search instead —
    /// the two cells diverge on exactly this predicate.
    #[must_use]
    pub fn me_data_present(
        &self,
        org_x: usize,
        org_y: usize,
        bsize: u8,
        list_idx: usize,
        ref_idx: usize,
    ) -> bool {
        let (b64_x, b64_y) = (org_x / 64, org_y / 64);
        if b64_x >= self.b64_cols || b64_y >= self.b64_rows {
            return false;
        }
        let out = &self.per_b64[b64_y * self.b64_cols + b64_x];
        let off = crate::port_md::predicates::get_me_block_offset(
            (org_x % 64) as u32,
            (org_y % 64) as u32,
            bsize,
            self.enable_me_8x8,
            self.enable_me_16x16,
        ) as usize;
        let n = out
            .total_me_candidate_index
            .get(off)
            .map_or(0, |&v| usize::from(v));
        for i in 0..n {
            let Some(c) = out.me_candidate_array.get(off * self.max_cand + i) else {
                break;
            };
            if (c.direction == 0 || c.direction == 2)
                && list_idx == usize::from(c.ref0_list)
                && ref_idx == usize::from(c.ref_idx_l0)
            {
                return true;
            }
            if (c.direction == 1 || c.direction == 2)
                && list_idx == usize::from(c.ref1_list)
                && ref_idx == usize::from(c.ref_idx_l1)
            {
                return true;
            }
        }
        false
    }

    /// This block's surviving ME candidates, in C's own order — what
    /// `inject_new_candidates` and `unipred_3x3_candidates_injection` walk.
    #[must_use]
    pub fn cands_for(
        &self,
        org_x: usize,
        org_y: usize,
        bsize: u8,
    ) -> &[crate::inter_me::context::MeCandidate] {
        let (b64_x, b64_y) = (org_x / 64, org_y / 64);
        if b64_x >= self.b64_cols || b64_y >= self.b64_rows {
            return &[];
        }
        let out = &self.per_b64[b64_y * self.b64_cols + b64_x];
        let off = crate::port_md::predicates::get_me_block_offset(
            (org_x % 64) as u32,
            (org_y % 64) as u32,
            bsize,
            self.enable_me_8x8,
            self.enable_me_16x16,
        ) as usize;
        let n = out
            .total_me_candidate_index
            .get(off)
            .map_or(0, |&v| usize::from(v));
        let start = off * self.max_cand;
        let end = (start + n).min(out.me_candidate_array.len());
        out.me_candidate_array.get(start..end).unwrap_or(&[])
    }

    /// **Why a consumer must not just read list 0.** On a flat low-delay-P
    /// GOP `construct_me_candidate_array_mrp_off` frequently emits its single
    /// unipred candidate for LIST 1, because `use_best_unipred_cand_only` is
    /// set and a tie in `p_sb_best_sad` resolves to list 1; and when list 0's
    /// distortion is far worse it is PRUNED outright by
    /// `prune_me_candidates_th`, in which case the list-0 slot is never
    /// written at all and still reads (0, 0). MEASURED 2026-09-02 on
    /// `gradient 128x128 q40 p8` frame 1: C's list-0 slot is `(0,0)`, its
    /// list-1 slot is `(-3,0)`, and `(-3,0)` is the MV mode decision uses.
    #[must_use]
    pub fn cand_mv_for(
        &self,
        org_x: usize,
        org_y: usize,
        bsize: u8,
        cand: usize,
    ) -> Option<(u8, svtav1_types::motion::Mv)> {
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
        if cand >= usize::from(*out.total_me_candidate_index.get(off)?) {
            return None;
        }
        let c = out.me_candidate_array.get(off * self.max_cand + cand)?;
        // C `BI_PRED` is 2; a bipred candidate names one slot per list and has
        // no single MV to return.
        if c.direction >= 2 {
            return None;
        }
        let ref_idx = usize::from(if c.direction == 1 {
            c.ref_idx_l1
        } else {
            c.ref_idx_l0
        });
        let slot = off * self.max_refs + if c.direction == 1 { self.max_l0 } else { 0 } + ref_idx;
        out.me_mv_array.get(slot).copied().map(|m| (c.direction, m))
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
    /// C `pcs->sc_class5` — the screen-content class that gates HME level 2.
    pub sc_class5: u8,
}

/// The `svt_aom_sig_deriv_me` INPUT set for one frame, split out of
/// [`run_frame_me`] so a test can assert the resolved search areas against
/// C's own `SVT_HME_OUT` `MESIG` line without re-transcribing the derivation
/// (`docs/WORKING-ON-THIS.md` §4: two transcriptions of one C function will
/// diverge).
#[must_use]
pub fn me_deriv_inputs(p: FrameMeParams, input_resolution: ResolutionRange) -> MeDerivInputs {
    let enc_mode = p.enc_mode as i8;
    // C `svt_aom_sig_deriv_multi_processes_default` (enc_mode_config.c:1988) —
    // levels 0 and 1 ARE unconditional, but level 2 is on only for SCREEN
    // CONTENT at <= M2. The port's own transcription of that ladder is
    // `port_enc_mode_config::multi_processes::sig_deriv_multi_processes_default`;
    // this driver carried a SECOND, wrong copy that hard-coded all four to 1,
    // with a comment citing the right lines and reading the wrong branch.
    // MEASURED 2026-09-02 through `SVT_HME_OUT` on `gradient 128x128 q40 p8`:
    // C reports `hme=1/1/1/0`. See `docs/INTER-ENCODE-PLAN.md` §1z13.
    let enable_hme_level2_flag = u8::from(p.sc_class5 != 0 && enc_mode <= enc_mode_c::M2);
    // C `set_qp_based_th_scaling_ctrls_default` (`enc_handle.c:3785`): every
    // scaling flag is 1 above ENC_MR, and both the ME and the HME search areas
    // are modulated by the SEQUENCE qp before the search ever runs. Passing
    // `false` here left the port searching C's UNSCALED areas —
    // `mesa=16x6/24x12` where C has `10x4/15x8`, and `l0sa=16x16/192x192`
    // where C has `10x10/122x122` (same measurement).
    let qp_th_scaling = enc_mode > enc_mode_c::MR;
    MeDerivInputs {
        enc_mode,
        sc_class5: p.sc_class5,
        input_resolution,
        rtc_tune: false,
        is_base: p.frame_is_boosted,
        hierarchical_levels: p.hierarchical_levels,
        enable_hme_flag: 1,
        enable_hme_level0_flag: 1,
        enable_hme_level1_flag: 1,
        enable_hme_level2_flag,
        // C `enc_mode_config.c:2168`.
        use_best_me_unipred_cand_only: u8::from(enc_mode > enc_mode_c::M1),
        me_qp_based_th_scaling: qp_th_scaling,
        hme_qp_based_th_scaling: qp_th_scaling,
        qp: u32::from(p.qp),
        safe_limit_nref: 0,
        safe_limit_zz_th: 0,
    }
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
    let signals = sig_deriv_me(me_deriv_inputs(p, input_resolution));

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

    let ds_ref = MeDsRef {
        picture: reference.full.view(),
        quarter: reference.quarter.view(),
        sixteenth: reference.sixteenth.view(),
        picture_number: reference.picture_number,
    };
    let refs = MeRefs {
        arr: [
            [Some(ds_ref), None, None, None],
            [Some(ds_ref), None, None, None],
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
            me.num_of_list_to_search = 2;
            me.num_of_ref_pic_to_search = [1, 1];
            // C `me_process.c:213` — every picture of a low-delay P GOP is a
            // reference. INERT at every preset this arm runs today, because
            // `me_early_exit_th != 0` takes the other arm of
            // `motion_estimation.c:1261`; set for faithfulness, not effect.
            me.is_ref = true;
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
        max_cand: MAX_CAND,
        max_l0: MAX_L0,
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
                sc_class5: 0,
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
                sc_class5: 0,
            },
        );
        assert_eq!(
            me_still.mv_for(0, 0, 12, 0, 0, 4).map(|m| (m.x, m.y)),
            Some((0, 0)),
            "an unmoved picture must give the zero MV — otherwise the -3 above \
             is noise, not a search result"
        );
    }

    /// **The two-SB-column cell, PINNED to values read out of the real C
    /// encoder** through `SVT_HME_OUT` (`tools/capture_c_trace/wrap_recon.c`'s
    /// `__wrap_svt_aom_motion_estimation_b64`), which is the only exported
    /// vantage point on `motion_estimation.c`'s otherwise-`static` pyramid.
    ///
    /// Measured 2026-09-02, `gradient 128x128 q40 p8 frames=2` frame 1, all
    /// four b64s, on the container oracle (`tools/ctrace-linux/run.sh`):
    ///
    /// ```text
    /// MESIG hme=1/1/1/0 mesa=10x4/15x8 l0sa=10x10/122x122 ubuc=1 nlist=2 nref=1/1
    /// MERES b64=0 d64=0 bestsad64=18816 bestmv64=(40,0)  mecand=1
    /// MERES b64=1 d64=0 bestsad64=13312 bestmv64=(-24,0) mecand=1
    /// MEL1  b64=* l1sad64=0 l1mv64=(-3,0) l1hme=(0,0):0 mv0=(0,0) mvl1=(-3,0)
    /// ```
    ///
    /// Every assertion below is one of those numbers, and each one has TEETH
    /// against a specific defect this chunk fixed
    /// (`docs/INTER-ENCODE-PLAN.md` §1z¹³):
    ///
    /// * `me_64x64_distortion == 0` and `cand_mv_for == (1, (-3,0))` fail if
    ///   list 1 is not searched (`num_of_list_to_search = 1`) — the port then
    ///   reports 3328/4704 and candidate direction 0.
    /// * `mv_for(list 0) == (0,0)` on the x=64 column fails if
    ///   `enable_hme_level2_flag` is wrongly 1: level 2 refines list 0 all the
    ///   way to (-3,0) and the list-0 slot stops being C's.
    ///
    /// It does NOT witness the qp-based search-area scaling: turning that flag
    /// back off leaves every assertion here green (measured). The scaling has
    /// its own cell, [`the_search_areas_join_cs_measured_mesig_line`].
    ///
    /// Evidence tier 2 (`docs/WORKING-ON-THIS.md` §4) — the constants come
    /// from the real `libSvtAv1Enc.a` running the real encoder, but through a
    /// link-time interposer on an outer entry point, not a per-function
    /// differential.
    #[test]
    fn the_me_arm_joins_cs_measured_two_sb_column_state() {
        const W: usize = 128;
        const H: usize = 128;
        let (f0, f1) = reference_cell(W, H, 3);
        let me = run_frame_me(
            &PaPicture::from_source(&f1, W, W, H, 1),
            &PaPicture::from_source(&f0, W, W, H, 0),
            FrameMeParams {
                enc_mode: 8,
                qp: 40,
                width: W,
                height: H,
                picture_number: 1,
                frame_is_boosted: false,
                hierarchical_levels: 0,
                sc_class5: 0,
            },
        );
        assert_eq!((me.b64_cols, me.b64_rows), (2, 2));
        for (b64, (org_x, org_y)) in [(0, 0), (64, 0), (0, 64), (64, 64)].into_iter().enumerate() {
            let out = &me.per_b64[b64];
            assert_eq!(
                out.me_64x64_distortion, 0,
                "b64 {b64}: C reports med=0/0/0/0 on every superblock"
            );
            assert_eq!(out.me_32x32_distortion, 0, "b64 {b64}");
            assert_eq!(out.me_8x8_cost_variance, 0, "b64 {b64}");
            assert_eq!(
                out.total_me_candidate_index[0], 1,
                "b64 {b64}: C emits ONE me candidate (list 0 is pruned by \
                 prune_me_candidates_th against list 1's zero distortion)"
            );
            assert_eq!(
                me.cand_mv_for(org_x, org_y, 12, 0)
                    .map(|(d, m)| (d, m.x, m.y)),
                Some((1, -3, 0)),
                "b64 {b64}: C's surviving candidate is LIST 1's, at the cell's \
                 true full-pel MV"
            );
            // C's list-0 slot: written on neither column, because list 0 is
            // pruned before `construct_me_candidate_array_mrp_off` writes it.
            assert_eq!(
                me.mv_for(org_x, org_y, 12, 0, 0, me.max_l0)
                    .map(|m| (m.x, m.y)),
                Some((0, 0)),
                "b64 {b64}: C leaves the list-0 me_mv_array slot untouched"
            );
        }
    }

    /// **The resolved ME/HME search areas, PINNED to C's `MESIG` line.**
    /// Measured 2026-09-02 through `SVT_HME_OUT` on a 128x128 preset-8
    /// low-delay-P encode:
    ///
    /// ```text
    /// q40: mesa=10x4/15x8   l0sa=10x10/122x122   hme=1/1/1/0   ubuc=1
    /// q55: mesa=15x5/22x11  l0sa=15x15/176x176   hme=1/1/1/0   ubuc=1
    /// ```
    ///
    /// TEETH: with `me_qp_based_th_scaling` / `hme_qp_based_th_scaling` forced
    /// back to `false` — the value the driver passed before §1z¹³ — this reads
    /// `mesa=16x6/24x12` and `l0sa=16x16/192x192` at BOTH qps, and every
    /// assertion below fails. Evidence tier 2.
    #[test]
    fn the_search_areas_join_cs_measured_mesig_line() {
        let res = ResolutionRange::from_luma_area(128 * 128);
        let params = |qp: u8| FrameMeParams {
            enc_mode: 8,
            qp,
            width: 128,
            height: 128,
            picture_number: 1,
            frame_is_boosted: false,
            hierarchical_levels: 0,
            sc_class5: 0,
        };
        for (qp, sa_min, sa_max, l0sa) in [
            (
                40u8,
                (10u16, 4u16),
                (15u16, 8u16),
                (10u16, 10u16, 122u16, 122u16),
            ),
            (55, (15, 5), (22, 11), (15, 15, 176, 176)),
        ] {
            let s = sig_deriv_me(me_deriv_inputs(params(qp), res));
            assert_eq!(
                (s.me_sa.sa_min.width, s.me_sa.sa_min.height),
                sa_min,
                "q{qp} me_sa.sa_min"
            );
            assert_eq!(
                (s.me_sa.sa_max.width, s.me_sa.sa_max.height),
                sa_max,
                "q{qp} me_sa.sa_max"
            );
            assert_eq!(
                (
                    s.hme.hme_l0_sa.sa_min.width,
                    s.hme.hme_l0_sa.sa_min.height,
                    s.hme.hme_l0_sa.sa_max.width,
                    s.hme.hme_l0_sa.sa_max.height
                ),
                l0sa,
                "q{qp} hme_l0_sa"
            );
            assert_eq!(
                (
                    s.enable_hme_flag,
                    s.enable_hme_level0_flag,
                    s.enable_hme_level1_flag,
                    s.enable_hme_level2_flag
                ),
                (1, 1, 1, 0),
                "q{qp} HME flags — C reports hme=1/1/1/0 at preset 8"
            );
            assert_eq!(s.use_best_unipred_cand_only, 1, "q{qp} ubuc");
        }
    }
}

// ---------------------------------------------------------------------------
// The ME-table geometry `md_nsq_motion_search` walks (motion_estimation.h:97-143)
// ---------------------------------------------------------------------------

/// C `SQUARE_PU_COUNT` (me_sb_results.h:25) — the number of square PU slots
/// in a b64's ME result: one 64x64, four 32x32, sixteen 16x16, sixty-four
/// 8x8.
pub const SQUARE_PU_COUNT: usize = 85;
/// C `MAX_SB64_PU_COUNT_NO_8X8` (me_sb_results.h:26) — 64x64 down to 16x16.
pub const MAX_SB64_PU_COUNT_NO_8X8: usize = 21;
/// C `MAX_SB64_PU_COUNT_WO_16X16` (me_sb_results.h:27) — 64x64 and 32x32.
pub const MAX_SB64_PU_COUNT_WO_16X16: usize = 5;

/// C `pu_search_index_map` (motion_estimation.h:115) — each PU slot's origin
/// `(x, y)` inside its b64.
///
/// Generated rather than transcribed as 85 literal pairs, because the layout
/// is exactly the four raster blocks `partition_width` already states and a
/// hand-copied 85-entry table is a transcription with 170 chances to be
/// wrong. [`pu_geometry_matches_c`] pins the generated table against the C
/// values that MATTER — the block sizes and the raster order — and
/// `md_nsq_motion_search`'s MVC list is keyed on both.
#[must_use]
pub fn pu_geometry(index: usize) -> (u32, u32, u32, u32) {
    // (org_x, org_y, width, height). C's four groups, in C's order.
    match index {
        0 => (0, 0, 64, 64),
        1..=4 => {
            let i = index - 1;
            (((i % 2) * 32) as u32, ((i / 2) * 32) as u32, 32, 32)
        }
        5..=20 => {
            let i = index - 5;
            (((i % 4) * 16) as u32, ((i / 4) * 16) as u32, 16, 16)
        }
        21..=84 => {
            let i = index - 21;
            (((i % 8) * 8) as u32, ((i / 8) * 8) as u32, 8, 8)
        }
        _ => panic!("pu_geometry: index {index} is outside SQUARE_PU_COUNT"),
    }
}

/// C's `number_of_pus` bound (product_coding_loop.c:2100-2103): the ME result
/// carries only the depths the picture's ME actually filled.
#[must_use]
pub fn number_of_pus(enable_me_8x8: bool, enable_me_16x16: bool) -> usize {
    if !enable_me_16x16 {
        MAX_SB64_PU_COUNT_WO_16X16
    } else if enable_me_8x8 {
        SQUARE_PU_COUNT
    } else {
        MAX_SB64_PU_COUNT_NO_8X8
    }
}

impl FrameMe {
    /// C `me_results->me_mv_array[block_index * max_refs + slot]` — the ME MV
    /// for a RAW PU slot, in FULL PEL.
    ///
    /// [`Self::mv_for`] resolves a block ORIGIN to a slot;
    /// `md_nsq_motion_search`'s MVC pass walks the slots directly, so it needs
    /// this shape instead.
    #[must_use]
    pub fn mv_at_pu(
        &self,
        b64_x: usize,
        b64_y: usize,
        block_index: usize,
        list: usize,
        ref_idx: usize,
    ) -> Option<svtav1_types::motion::Mv> {
        let out = self.per_b64.get(b64_y * self.b64_cols + b64_x)?;
        let slot = block_index * self.max_refs + if list == 1 { self.max_l0 } else { 0 } + ref_idx;
        out.me_mv_array.get(slot).copied()
    }

    /// C `svt_aom_is_me_data_present(block_index, block_index * max_cand, ...)`
    /// for a RAW PU slot — the same rule as [`Self::me_data_present`],
    /// BI_PRED counting for both lists included, keyed on the slot rather than
    /// on a block origin.
    #[must_use]
    pub fn me_data_present_at_pu(
        &self,
        b64_x: usize,
        b64_y: usize,
        block_index: usize,
        list_idx: usize,
        ref_idx: usize,
    ) -> bool {
        let Some(out) = self.per_b64.get(b64_y * self.b64_cols + b64_x) else {
            return false;
        };
        let n = out
            .total_me_candidate_index
            .get(block_index)
            .map_or(0, |&v| usize::from(v));
        (0..n).any(|i| {
            out.me_candidate_array
                .get(block_index * self.max_cand + i)
                .is_some_and(|c| {
                    ((c.direction == 0 || c.direction == 2)
                        && list_idx == usize::from(c.ref0_list)
                        && ref_idx == usize::from(c.ref_idx_l0))
                        || ((c.direction == 1 || c.direction == 2)
                            && list_idx == usize::from(c.ref1_list)
                            && ref_idx == usize::from(c.ref_idx_l1))
                })
        })
    }
}

#[cfg(test)]
mod pu_geometry_tests {
    use super::*;

    /// TIER 4 — the generated table against C's own literals
    /// (motion_estimation.h:97-143), spot-checked at every group boundary
    /// plus the two corners of each group. A generated table that agreed with
    /// itself would prove nothing; these are C's numbers, read out of the
    /// header.
    #[test]
    fn pu_geometry_matches_c() {
        // C `pu_search_index_map` + `partition_width` / `partition_height`.
        let expect: &[(usize, (u32, u32, u32, u32))] = &[
            (0, (0, 0, 64, 64)),
            (1, (0, 0, 32, 32)),
            (2, (32, 0, 32, 32)),
            (3, (0, 32, 32, 32)),
            (4, (32, 32, 32, 32)),
            (5, (0, 0, 16, 16)),
            (8, (48, 0, 16, 16)),
            (9, (0, 16, 16, 16)),
            (20, (48, 48, 16, 16)),
            (21, (0, 0, 8, 8)),
            (28, (56, 0, 8, 8)),
            (29, (0, 8, 8, 8)),
            (84, (56, 56, 8, 8)),
        ];
        for &(i, want) in expect {
            assert_eq!(pu_geometry(i), want, "pu_geometry({i})");
        }
    }

    /// TIER 4 — every slot lies inside the b64 and no two slots of the same
    /// size share an origin. A shifted raster would still pass the spot
    /// checks above at the group corners; this catches the middle.
    #[test]
    fn pu_geometry_tiles_the_b64_without_overlap() {
        let mut seen = alloc::collections::BTreeSet::new();
        for i in 0..SQUARE_PU_COUNT {
            let (x, y, w, h) = pu_geometry(i);
            assert!(x + w <= 64 && y + h <= 64, "slot {i} escapes the b64");
            assert!(
                seen.insert((x, y, w, h)),
                "slot {i} duplicates an earlier one"
            );
        }
        assert_eq!(seen.len(), SQUARE_PU_COUNT);
    }

    #[test]
    fn number_of_pus_matches_c_gate() {
        assert_eq!(number_of_pus(true, true), SQUARE_PU_COUNT);
        assert_eq!(number_of_pus(false, true), MAX_SB64_PU_COUNT_NO_8X8);
        assert_eq!(number_of_pus(false, false), MAX_SB64_PU_COUNT_WO_16X16);
        assert_eq!(number_of_pus(true, false), MAX_SB64_PU_COUNT_WO_16X16);
    }
}
