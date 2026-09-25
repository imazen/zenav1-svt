use super::*;

/// Build this block's INTER candidate set, exactly as C composes it.
///
/// One reference's precomputed local-warp neighbour samples — C's
/// `ctx->wm_sample_info[ref]`, filled once per block by
/// `svt_aom_init_wm_samples` and read by every candidate's warp derivation.
#[derive(Clone, Copy)]
pub struct WarpSamples {
    pub n: u8,
    pub pts: [[i32; 2]; crate::inter_mvp::LEAST_SQUARES_SAMPLES_MAX],
    pub pts_inref: [[i32; 2]; crate::inter_mvp::LEAST_SQUARES_SAMPLES_MAX],
}

impl Default for WarpSamples {
    fn default() -> Self {
        WarpSamples {
            n: 0,
            pts: [[0; 2]; crate::inter_mvp::LEAST_SQUARES_SAMPLES_MAX],
            pts_inref: [[0; 2]; crate::inter_mvp::LEAST_SQUARES_SAMPLES_MAX],
        }
    }
}

/// Everything the MDS1 warp MV refinement needs that is PER BLOCK.
///
/// C keeps all of it on `ModeDecisionContext` and `svt_aom_wm_motion_refinement`
/// reads it there. The port builds the candidate list in this module and runs
/// MDS1 in the leaf funnel, so the block-scoped half has to travel; the
/// frame-scoped half (`nmv`, `fac.drl_mode`, `allow_high_precision_mv`) stays
/// on [`InterMdFrame`] and is read through `fx.inter`.
pub struct WarpRefineBlock {
    /// The per-reference MV stacks `svt_aom_generate_av1_mvp_table` built for
    /// this block.
    ///
    /// The OBMC MV refinement re-picks the DRL index against them
    /// (`svt_aom_choose_best_av1_mv_pred`, mode_decision.c:2272), so it needs
    /// the SAME stack the injector priced the candidate against — rebuilding
    /// it in the funnel would be a second derivation that could disagree.
    pub mvp_stacks: alloc::vec::Vec<crate::inter_mvp::InterMvpStack>,

    /// False when this block can produce no warped candidate at all, which
    /// makes the refinement a no-op without the caller having to know why.
    pub enabled: bool,
    /// C `ctx->wm_ctrls`, the fields the refinement reads.
    pub refinement_iterations: u8,
    pub refine_diag: bool,
    pub shut_approx_if_not_mds0: bool,
    pub lower_band_th: u16,
    pub upper_band_th: u16,
    /// C `svt_aom_set_wm_controls`'s `refine_level`: 1 -> MDS1, 2 -> MDS3.
    pub refine_level: u8,
    /// C `ctx->wm_sample_info[ref]`.
    pub samples: [WarpSamples; 8],
    /// C `ctx->ref_mv_stack[ref]` and `xd->ref_mv_count[ref]`, for the DRL
    /// re-pick the refinement ends with.
    pub stacks: alloc::vec::Vec<crate::inter_mvp::InterMvpStack>,
    pub ref_mv_count: [u8; crate::inter_mvp::MODE_CTX_REF_FRAMES],
    pub mi_row: i32,
    pub mi_col: i32,
    pub bsize: svtav1_types::block::BlockSize,
    pub bwidth: usize,
    pub bheight: usize,
}

impl Default for WarpRefineBlock {
    fn default() -> Self {
        WarpRefineBlock {
            mvp_stacks: alloc::vec::Vec::new(),
            enabled: false,
            refinement_iterations: 0,
            refine_diag: false,
            shut_approx_if_not_mds0: false,
            lower_band_th: 0,
            upper_band_th: 0,
            refine_level: 0,
            samples: Default::default(),
            stacks: alloc::vec::Vec::new(),
            ref_mv_count: [0; crate::inter_mvp::MODE_CTX_REF_FRAMES],
            mi_row: 0,
            mi_col: 0,
            bsize: svtav1_types::block::BlockSize::Block8x8,
            bwidth: 0,
            bheight: 0,
        }
    }
}

impl WarpRefineBlock {
    /// C `svt_aom_warped_motion_parameters` (adaptive_mv_pred.c:1776) for an
    /// arbitrary test MV — the SAME derivation the injector's hook runs, which
    /// is why it lives here rather than being duplicated in the funnel.
    ///
    /// `shut_approx` skips the band gate; C passes
    /// `ctx->wm_ctrls.shut_approx_if_not_mds0` inside the search and a literal
    /// 1 for the final re-derivation, with `lower`/`upper` forced to 0 there
    /// ("this call is not part of a search, so disable the shortcuts").
    #[must_use]
    pub fn warp_params_for(
        &self,
        ref_frame: i8,
        mv: svtav1_types::motion::Mv,
        shut_approx: bool,
        wm: &mut svtav1_types::motion::WarpedMotionParams,
    ) -> Option<u8> {
        use svtav1_dsp::port_warp::{find_projection, select_samples};
        if self.bwidth < 8 || self.bheight < 8 {
            return None;
        }
        let s = &self.samples[ref_frame.max(0) as usize];
        if s.n == 0 {
            return None;
        }
        let mut pts = [0i32; 2 * crate::inter_mvp::LEAST_SQUARES_SAMPLES_MAX];
        let mut pts_inref = [0i32; 2 * crate::inter_mvp::LEAST_SQUARES_SAMPLES_MAX];
        for i in 0..usize::from(s.n) {
            pts[2 * i] = s.pts[i][0];
            pts[2 * i + 1] = s.pts[i][1];
            pts_inref[2 * i] = s.pts_inref[i][0];
            pts_inref[2 * i + 1] = s.pts_inref[i][1];
        }
        let mut nsamples = s.n;
        if nsamples > 1 {
            nsamples = select_samples(
                mv,
                &mut pts,
                &mut pts_inref,
                usize::from(nsamples),
                self.bsize,
            );
        }
        let mut apply = !find_projection(
            usize::from(nsamples),
            &pts,
            &pts_inref,
            self.bsize,
            mv,
            wm,
            self.mi_row,
            self.mi_col,
        );
        if apply && !shut_approx {
            let (a, b) = (i32::from(wm.alpha).abs(), i32::from(wm.beta).abs());
            let (g, d) = (i32::from(wm.gamma).abs(), i32::from(wm.delta).abs());
            let lo = i32::from(self.lower_band_th);
            let hi = i32::from(self.upper_band_th);
            if a + b < lo && g + d < lo {
                apply = false;
            }
            if 4 * a + 7 * b > hi && 4 * g + 4 * d > hi {
                apply = false;
            }
        }
        // C assigns `*num_samples` regardless of the projection's success, so
        // the count is returned even when the model is rejected.
        if apply { Some(nsamples) } else { None }
    }
}

/// `ctx->cmp_store`'s OUTPUT — the masked-compound preparation
/// `calc_pred_masked_compound` fills and `search_compound_diff_wedge`
/// reads (C's `ctx->pred0`/`pred1`/`residual1`/`diff10`).
///
/// C's store is a four-entry MV-keyed cache per list
/// (`port_compound_prep::cmp_store_lookup` is that policy); the
/// predictors are deterministic per (ref, MV), so this port keeps only
/// the live pair's buffers and pays a second uniprediction on a repeated
/// MV rather than holding eight slots of scratch per block. The search
/// output is identical either way.
#[derive(Default)]
pub(super) struct CmpStore {
    /// The 8-bit domain buffers — used whenever the remap
    /// (`hbd_md == EB_DUAL_BIT_MD ? 8 : hbd_md`, enc_inter_prediction.c:3538)
    /// lands on 8 bits, which is every reachable case.
    pub(super) pred0_8: Vec<u8>,
    pub(super) pred1_8: Vec<u8>,
    pub(super) residual1_8: Vec<i16>,
    pub(super) diff10_8: Vec<i16>,
    /// The `hbd_md == EB_10_BIT_MD` twins — wired for faithfulness, dead
    /// on the shipped ladder (`hbd_md` is 0 on inter frames).
    pub(super) pred0_16: Vec<u16>,
    pub(super) pred1_16: Vec<u16>,
    pub(super) residual1_16: Vec<i16>,
    pub(super) diff10_16: Vec<i16>,
    /// The post-remap search depth the preparation ran at; `None` until
    /// `calc_pred_masked_compound` has filled the store.
    pub(super) hbd: Option<bool>,
}

/// The injector's [`InjectHooks`] with the WARP derivation, the
/// inter-intra search, and the masked-compound preparation/pick wired.
///
/// `blk` is the block-scoped warp-refinement state; `f`/`b` carry the
/// frame and block inputs the three pixel searches read; `ii_ctrls` and
/// `comp_ctrls` are the SAME control objects `InjectCtx` holds (C's
/// `ctx->inter_intra_comp_ctrls` / `inter_comp_ctrls`); `cmp` is the
/// `calc_pred_masked_compound` → `search_compound_diff_wedge` buffer
/// bridge.
pub(super) struct WarpHooks<'a> {
    pub(super) blk: &'a WarpRefineBlock,
    pub(super) f: &'a InterMdFrame<'a>,
    pub(super) b: &'a InterBlockCtx<'a>,
    pub(super) ii_ctrls: crate::port_md::inject::InterIntraCompCtrls,
    pub(super) comp_ctrls: crate::port_md::inject::InterCompCtrls,
    pub(super) cmp: CmpStore,
}

/// The injector's `MotionMode` (port_md::predicates) as the prediction/
/// rate crate's (`port_entropy_inter::modes`) — same discriminants.
pub(super) fn map_mm(m: crate::port_md::predicates::MotionMode) -> MotionMode {
    match m {
        crate::port_md::predicates::MotionMode::SimpleTranslation => MotionMode::SimpleTranslation,
        crate::port_md::predicates::MotionMode::ObmcCausal => MotionMode::ObmcCausal,
        crate::port_md::predicates::MotionMode::WarpedCausal => MotionMode::WarpedCausal,
    }
}

/// The single-reference luma prediction C's `svt_aom_inter_prediction`
/// makes for a unipred candidate at the 8-bit domain — the warp leaf when
/// `is_wm`, the plain convolve otherwise, `interp_filters` 0.
///
/// This is what `inter_intra_search` blends against (mode_decision.c:357)
/// and what `calc_pred_masked_compound` fills `pred0`/`pred1` with
/// (:3579/:3630) — the `unipred_override` shape: no compound, no
/// inter-intra, no OBMC.
#[allow(clippy::too_many_arguments)]
pub(super) fn unipred_luma8(
    p: &crate::picture::PaddedPlane,
    mut wm: svtav1_types::motion::WarpedMotionParams,
    is_wm: bool,
    org_x: usize,
    org_y: usize,
    bw: usize,
    bh: usize,
    mv: Mv,
    sb_size: usize,
    frame_w: usize,
    frame_h: usize,
    dst: &mut [u8],
) {
    if is_wm {
        crate::inter_pred_arm::predict_inter_yuv_warped(
            (p, None),
            &mut wm,
            org_x,
            org_y,
            bw,
            bh,
            mv,
            0,
            sb_size,
            frame_w,
            frame_h,
            dst,
            bw,
            &mut [],
            &mut [],
            0,
        );
    } else {
        crate::inter_pred_arm::predict_inter_luma(
            p, org_x, org_y, bw, bh, mv, 0, sb_size, frame_w, frame_h, dst, bw,
        );
    }
}

/// [`unipred_luma8`] at true 10 bits — `predict_inter_leaf_hbd`'s luma-
/// only form, for the `hbd_md` search domains.
#[allow(clippy::too_many_arguments)]
pub(super) fn unipred_luma16(
    p: &crate::picture::PaddedPlaneHbd,
    motion_mode: MotionMode,
    is_wm: bool,
    wm: svtav1_types::motion::WarpedMotionParams,
    org_x: usize,
    org_y: usize,
    bw: usize,
    bh: usize,
    mv: Mv,
    sb_size: usize,
    frame_w: usize,
    frame_h: usize,
    bit_depth: u8,
    dst: &mut [u16],
) {
    crate::inter_pred_arm::predict_inter_leaf_hbd(
        p,
        None,
        motion_mode,
        is_wm,
        wm,
        org_x,
        org_y,
        bw,
        bh,
        mv,
        0,
        sb_size,
        frame_w,
        frame_h,
        bit_depth,
        dst,
        bw,
        &mut [],
        &mut [],
        0,
    );
}

impl crate::port_md::inject::InjectHooks for WarpHooks<'_> {
    /// C `inter_intra_search` (mode_decision.c:326-484): the regular
    /// inter prediction blended against each of the four
    /// `intrapred_buf` modes, priced by SSE or the RD model, then the
    /// `ii_wedge_mode` wedge search on the winner. The candidate keeps
    /// the strict-`<` winners; `use_wedge_interintra` follows C's
    /// mode-1/mode-2 rule exactly.
    fn inter_intra_search(&mut self, cand: &mut crate::port_md::inject::InterCandidate) {
        use svtav1_dsp::port_interintra::{
            InterIntraMode, combine_interintra, combine_interintra_highbd,
        };
        use svtav1_dsp::port_masked_compound::{
            highbd_sse, highbd_subtract_block, sse, subtract_block,
        };
        use svtav1_dsp::port_wedge_search::{SearchCtx, pick_wedge_fixed_sign};

        let f = self.f;
        let b = self.b;
        let Some(ii) = b.ii.as_ref() else {
            // `precompute_intra_pred_for_inter_intra` never ran for this
            // block — the funnel's gate and the injector's
            // `is_interintra_allowed` should agree, but refuse rather
            // than blend an empty table.
            debug_assert!(
                false,
                "inter_intra_search without precomputed intrapred_buf"
            );
            return;
        };
        // C reads `ctx->hbd_md` DIRECTLY here (mode_decision.c:333) —
        // no EB_DUAL_BIT_MD remap — so a DUAL block searches at 10 bits.
        let hbd = f.hbd_md != 0;
        let full_lambda = if hbd { b.full_lambda10 } else { b.full_lambda8 };
        let (bw, bh) = (b.bw, b.bh);
        let n = bw * bh;
        let bsize = svtav1_types::block::BlockSize::from_u8(b.bsize)
            .expect("an injected inter block must have a real BlockSize");
        let padded = f.padded_by_ref[cand.ref_frame[0].max(0) as usize].unwrap_or_else(|| {
            panic!(
                "an inter candidate names reference {} with no DPB picture",
                cand.ref_frame[0]
            )
        });
        let mm = map_mm(cand.motion_mode);
        let is_wm = crate::inter_pred_arm::inter_pred_uses_warp(
            mm,
            cand.mode as u8,
            bw,
            bh,
            &cand.wm_params_l0,
        );
        // C `svt_aom_inter_prediction` into `tmp_buf` — the plain
        // single-reference prediction (`interp_filters` 0,
        // `is_interintra_used` 0 on the copy the injector hands us).
        let mut inter8: Vec<u8> = Vec::new();
        let mut inter16: Vec<u16> = Vec::new();
        if hbd {
            inter16 = alloc::vec![0u16; n];
            let h = padded
                .hbd
                .as_ref()
                .expect("hbd_md inter-intra search needs the 10-bit DPB twin");
            unipred_luma16(
                &h.y,
                mm,
                is_wm,
                cand.wm_params_l0,
                b.org_x,
                b.org_y,
                bw,
                bh,
                cand.mv[0],
                f.sb_size,
                f.frame_w,
                f.frame_h,
                f.bit_depth,
                &mut inter16,
            );
        } else {
            inter8 = alloc::vec![0u8; n];
            unipred_luma8(
                &padded.y,
                cand.wm_params_l0,
                is_wm,
                b.org_x,
                b.org_y,
                bw,
                bh,
                cand.mv[0],
                f.sb_size,
                f.frame_w,
                f.frame_h,
                &mut inter8,
            );
        }
        let src8_off = b.org_y * f.src_stride + b.org_x;
        let bsize_group =
            crate::port_entropy_inter::modes::SIZE_GROUP_LOOKUP[bsize as usize] as usize;
        // The four-way mode loop (mode_decision.c:382-461): blend, score,
        // keep the strict-`<` best. `best_interintra_rd` is INT64_MAX
        // init, so the first mode always lands.
        let mut best_rd = i64::MAX;
        let mut best_mode = svtav1_dsp::port_interintra::INTERINTRA_MODES as u8;
        let mut blend: Vec<u8> = Vec::new();
        let mut blend16: Vec<u16> = Vec::new();
        if hbd {
            blend16 = alloc::vec![0u16; n];
        } else {
            blend = alloc::vec![0u8; n];
        }
        for (j, mode) in InterIntraMode::ALL.iter().enumerate() {
            let rmode = i64::from(f.fac.inter_intra_mode[bsize_group][j]);
            if hbd {
                combine_interintra_highbd(
                    &f.ii_masks,
                    &f.wedge_masks,
                    *mode,
                    false,
                    0,
                    0,
                    bsize as usize,
                    bsize as usize,
                    &mut blend16,
                    bw,
                    &inter16,
                    bw,
                    &ii.luma16[j * n..],
                    bw,
                );
            } else {
                combine_interintra(
                    &f.ii_masks,
                    &f.wedge_masks,
                    *mode,
                    false,
                    0,
                    0,
                    bsize as usize,
                    bsize as usize,
                    &mut blend,
                    bw,
                    &inter8,
                    bw,
                    &ii.luma[j * n..],
                    bw,
                );
            }
            let rd: i64 = if self.ii_ctrls.use_rd_model {
                // `src_stride` is consumed by the LIVE arm only — the
                // 10-bit source is `blk_y_src10`, a block-local `bw`-
                // strided canvas (C's `input_frame16bit` crop), while the
                // 8-bit source is the frame-strided `enhanced_pic`.
                let (rate_sum, dist_sum) = svtav1_dsp::port_model_rd::model_rd_for_sb_with_curvfit(
                    bsize,
                    bw,
                    bh,
                    &f.src[src8_off..],
                    if hbd { bw } else { f.src_stride },
                    b.y_src10,
                    &blend,
                    bw,
                    &blend16,
                    0,
                    0,
                    hbd,
                    b.quantizer,
                    full_lambda,
                    None,
                );
                crate::port_rd_cost::rdcost(
                    u64::from(full_lambda),
                    (i64::from(rate_sum) + rmode).max(0) as u64,
                    dist_sum.max(0) as u64,
                ) as i64
            } else if hbd {
                highbd_sse(b.y_src10, bw, &blend16, bw, bw, bh) as i64
            } else {
                sse(&f.src[src8_off..], f.src_stride, &blend, bw, bw, bh) as i64
            };
            if rd < best_rd {
                best_rd = rd;
                best_mode = j as u8;
                cand.interintra_mode = j as u8;
            }
        }
        // The wedge arm (mode_decision.c:463-484): residuals against the
        // SMOOTH-winner's intra pred and the same inter pred, then
        // `pick_wedge_fixed_sign` at sign 0 (`INTERINTRA_WEDGE_SIGN`).
        let ii_wedge_mode = if b.is_part_n {
            self.ii_ctrls.wedge_mode_sq
        } else {
            self.ii_ctrls.wedge_mode_nsq
        };
        let mut best_rd_wedge = i64::MAX;
        if ii_wedge_mode != 0 {
            let mut residual1 = alloc::vec![0i16; n];
            let mut diff10 = alloc::vec![0i16; n];
            if hbd {
                highbd_subtract_block(bh, bw, &mut residual1, bw, b.y_src10, bw, &inter16, bw);
                highbd_subtract_block(
                    bh,
                    bw,
                    &mut diff10,
                    bw,
                    &inter16,
                    bw,
                    &ii.luma16[best_mode as usize * n..],
                    bw,
                );
            } else {
                subtract_block(
                    bh,
                    bw,
                    &mut residual1,
                    bw,
                    &f.src[src8_off..],
                    f.src_stride,
                    &inter8,
                    bw,
                );
                subtract_block(
                    bh,
                    bw,
                    &mut diff10,
                    bw,
                    &inter8,
                    bw,
                    &ii.luma[best_mode as usize * n..],
                    bw,
                );
            }
            let ctx = SearchCtx {
                hbd,
                full_lambda,
                // `pick_wedge_fixed_sign` reads
                // `inter_intra_comp_ctrls.use_rd_model` on this arm — see
                // `SearchCtx::use_rate`'s doc.
                use_rate: self.ii_ctrls.use_rd_model,
                quantizer: b.quantizer,
            };
            let (rd_wedge, wedge_index) = pick_wedge_fixed_sign(
                &f.wedge_masks,
                &ctx,
                bsize,
                &residual1,
                &diff10,
                0,
                &f.fac.wedge_idx[bsize as usize],
            );
            cand.interintra_wedge_index = wedge_index as i8;
            best_rd_wedge = rd_wedge;
        }
        // C's exact rule (mode_decision.c:481-483): mode 1 always wedges;
        // mode 2 wedges only when it beats the smooth winner.
        cand.use_wedge_interintra = ii_wedge_mode == 1 || best_rd_wedge < best_rd;
    }

    /// C `svt_aom_wm_motion_refinement` (mode_decision.c:1873-2011) at
    /// INJECTION time — `inj_non_simple_modes` calls it only for `NEWMV`
    /// clones when `refinement_iterations != 0 && refine_level == 0`
    /// (wm_level 1). The same diamond search the MDS1 lane drives through
    /// [`crate::port_md::mv_refine::wm_motion_refinement`], differing in two
    /// inputs C passes literally at this call site (mode_decision.c:925):
    /// `shut_approx` is 0 (the band gate is live, though level 1's band is
    /// 0/~0) and the rate reference is the candidate's injection-time
    /// `pred_mv`.
    fn wm_motion_refinement(&mut self, cand: &mut crate::port_md::inject::InterCandidate) -> bool {
        let blk = self.blk;
        let f = self.f;
        let b = self.b;
        let rf = cand.ref_frame[0].max(0) as usize;
        let stack = &blk.stacks[rf];
        // C `error_per_bit = full_lambda_md[EB_8_BIT_MD] >> RD_EPB_SHIFT`
        // with the `+= (x == 0)` MAX(1) (mode_decision.c:1884-1886).
        let epb = (b.full_lambda8 >> crate::intrabc::RD_EPB_SHIFT) as i32;
        let refine_ctx = crate::port_md::mv_refine::WmRefineCtx {
            refinement_iterations: blk.refinement_iterations,
            refine_diag: blk.refine_diag,
            allow_high_precision_mv: f.allow_high_precision_mv,
            approx_inter_rate: f.search.approx_inter_rate,
            corrupted_mv_check: true,
            error_per_bit: epb + i32::from(epb == 0),
            drl: crate::port_md::drl::ChooseDrlCtx {
                shut_fast_rate: false,
                approx_inter_rate: f.search.approx_inter_rate,
                ref_mv_stack: &stack.stack,
                ref_mv_count: blk.ref_mv_count[rf],
                nmv_cost: &f.nmv,
                drl_mode_fac_bits: &f.fac.drl_mode,
            },
        };
        let (bw, bh) = (b.bw, b.bh);
        // C `ctx->scratch_prediction_ptr` — luma only; the candidate's own
        // prediction is untouched until a winner exists.
        let mut scratch = crate::vecpool::dirty_pool::<u8>(bw * bh);
        let padded = f.padded_by_ref[rf]
            .unwrap_or_else(|| panic!("warp-refined candidate names ref {rf} with no DPB picture"));
        let src_off = b.org_y * f.src_stride + b.org_x;
        let mut wm = svtav1_types::motion::WarpedMotionParams::default();
        let r = crate::port_md::mv_refine::wm_motion_refinement(
            &refine_ctx,
            cand.mv[0],
            cand.pred_mv[0],
            cand.mode,
            |test_mv| {
                wm = svtav1_types::motion::WarpedMotionParams {
                    wm_type: svtav1_types::motion::TransformationType::Affine,
                    ..Default::default()
                };
                // `shut_approx` is a literal 0 at C's injection call
                // (mode_decision.c:925) — the band gate is live.
                blk.warp_params_for(cand.ref_frame[0], test_mv, false, &mut wm)?;
                unipred_luma8(
                    &padded.y,
                    wm,
                    true,
                    b.org_x,
                    b.org_y,
                    bw,
                    bh,
                    test_mv,
                    f.sb_size,
                    f.frame_w,
                    f.frame_h,
                    &mut scratch,
                );
                // C `fn_ptr->vf(pred, src, &sse)` — variance, not SSE.
                Some(svtav1_dsp::variance::variance_diff(
                    &scratch,
                    bw,
                    &f.src[src_off..],
                    f.src_stride,
                    bw,
                    bh,
                ) as i32)
            },
        );
        cand.mv[0] = r.best_mv;
        cand.drl_index = r.drl_index;
        // C copies back `best_pred_mv[0]` only (mode_decision.c:2000) —
        // `pred_mv[1]` keeps whatever the cloned simple candidate carried.
        cand.pred_mv[0] = r.pred_mv[0];
        r.valid
    }

    /// C `svt_aom_warped_motion_parameters` (adaptive_mv_pred.c:1776).
    ///
    /// `shut_approx` is FALSE at the injection call site (`mode_decision.c:936`
    /// passes a literal 0), so the band gate below is live — and at wm_level 3
    /// `lower_band_th` is `1 << 10`, which is what rejects a warp too weak to
    /// be worth its own motion mode.
    fn warped_motion_parameters(
        &mut self,
        cand: &mut crate::port_md::inject::InterCandidate,
    ) -> bool {
        // C assigns `*num_samples = 0` on every early return, so a rejected
        // candidate leaves the count zeroed rather than stale.
        cand.num_proj_ref = 0;
        match self.blk.warp_params_for(
            cand.ref_frame[0],
            cand.mv[0],
            // `shut_approx` is a literal 0 at C's injection call site
            // (mode_decision.c:936), so the band gate is LIVE here -- at
            // wm_level 3 `lower_band_th` is 1 << 10, which is what rejects a
            // warp too weak to be worth its own motion mode.
            false,
            &mut cand.wm_params_l0,
        ) {
            Some(n) => {
                // The count is `block_mi.num_proj_ref`, which the WRITER reads
                // to pick the motion-mode ALPHABET. Getting it wrong is an
                // arithmetic-coder desync, not a quality choice.
                cand.num_proj_ref = n;
                true
            }
            None => false,
        }
    }

    /// OBMC is not wired. The injector never asks: `obmc_ctrls` is
    /// `Default::default()` (disabled) on this arm.
    ///
    /// THE JUSTIFICATION THAT USED TO BE HERE WAS AN OVER-READ. It said "the
    /// census that motivated the warp wiring found C selecting OBMC on ZERO
    /// blocks at these presets" — true of presets 6 and 8, which is all that
    /// census measured, and read ever since as a fact about OBMC.
    /// `benchmarks/obmc_census_2026-09-10.meta` asked the question at the
    /// presets global motion made reachable: C codes OBMC on **22.5 % of every
    /// coded inter block at preset 0** (987 of 4382 over twelve real-video
    /// cells), 22.2 % at MR and 27.0 % at preset 1, and EXACTLY ZERO at preset
    /// 2 and above. The cutoff is `svt_aom_get_obmc_level`'s level 3 -> 5 step.
    ///
    /// So this is a CORRECTNESS gap at presets -1/0/1, not RD in a wider
    /// envelope. `tools/motion_mode_census.sh` re-measures it.
    fn obmc_motion_refinement(
        &mut self,
        _cand: &mut crate::port_md::inject::InterCandidate,
    ) -> bool {
        true
    }

    /// C `svt_aom_calc_pred_masked_compound`
    /// (enc_inter_prediction.c:3535-3703): the two unipred predictors
    /// for the candidate's ref pair, the `pred0_to_pred1_mult` SAD early
    /// exit, then `residual1 = src - pred1` and `diff10 = pred1 - pred0`
    /// into the store `search_compound_diff_wedge` reads. A `true`
    /// return is C's `exit_compound_prep`, which makes `inj_comp_modes`
    /// inject nothing for the pair.
    fn calc_pred_masked_compound(&mut self, cand: &crate::port_md::inject::InterCandidate) -> bool {
        use svtav1_dsp::port_compound_prep::{
            compound_residuals, compound_residuals_hbd, exit_compound_prep,
        };

        let f = self.f;
        let b = self.b;
        // C :3538 — EB_DUAL_BIT_MD remaps to the 8-bit domain; only
        // EB_10_BIT_MD (1) prepares at true depth. On the shipped ladder
        // `hbd_md` is 0, so the 8-bit arm is the live one.
        let hbd = f.hbd_md == 1;
        self.cmp.hbd = Some(hbd);
        let (bw, bh) = (b.bw, b.bh);
        let n = bw * bh;
        // The unipred override (C :3573/:3624): `mode` is NEWMV, or
        // GLOBALMV when the candidate is GLOBAL_GLOBALMV — which is what
        // makes `inter_pred_uses_warp` fire on the model that candidate
        // carries — and `interp_filters`/`is_interintra_used` forced 0.
        let mode = if cand.mode == PredictionMode::GlobalGlobalMv {
            PredictionMode::GlobalMv
        } else {
            PredictionMode::NewMv
        };
        let mm = map_mm(cand.motion_mode);
        for i in 0..2 {
            let rf = cand.ref_frame[i];
            let padded = f.padded_by_ref[rf.max(0) as usize].unwrap_or_else(|| {
                panic!("a compound candidate names reference {rf} with no DPB picture")
            });
            let wm = if i == 0 {
                cand.wm_params_l0
            } else {
                cand.wm_params_l1
            };
            let is_wm = crate::inter_pred_arm::inter_pred_uses_warp(mm, mode as u8, bw, bh, &wm);
            if hbd {
                let h = padded
                    .hbd
                    .as_ref()
                    .expect("hbd_md masked-compound prep needs the 10-bit DPB twin");
                let dst = if i == 0 {
                    self.cmp.pred0_16 = alloc::vec![0u16; n];
                    &mut self.cmp.pred0_16
                } else {
                    self.cmp.pred1_16 = alloc::vec![0u16; n];
                    &mut self.cmp.pred1_16
                };
                unipred_luma16(
                    &h.y,
                    mm,
                    is_wm,
                    wm,
                    b.org_x,
                    b.org_y,
                    bw,
                    bh,
                    cand.mv[i],
                    f.sb_size,
                    f.frame_w,
                    f.frame_h,
                    f.bit_depth,
                    dst,
                );
            } else {
                let dst = if i == 0 {
                    self.cmp.pred0_8 = alloc::vec![0u8; n];
                    &mut self.cmp.pred0_8
                } else {
                    self.cmp.pred1_8 = alloc::vec![0u8; n];
                    &mut self.cmp.pred1_8
                };
                unipred_luma8(
                    &padded.y, wm, is_wm, b.org_x, b.org_y, bw, bh, cand.mv[i], f.sb_size,
                    f.frame_w, f.frame_h, dst,
                );
            }
        }
        // C :3662-3671 — the per-pixel SAD budget: too-similar predictors
        // have nothing for a mask to separate.
        let dist = if hbd {
            svtav1_dsp::hbd::highbd_sad_kernel(
                &self.cmp.pred0_16,
                bw,
                &self.cmp.pred1_16,
                bw,
                bw,
                bh,
            )
        } else {
            svtav1_dsp::sad::sad(&self.cmp.pred0_8, bw, &self.cmp.pred1_8, bw, bw, bh)
        };
        if exit_compound_prep(dist, bw, bh, u32::from(self.comp_ctrls.pred0_to_pred1_mult)) {
            return true;
        }
        // C :3675/:3696 — `residual1 = src - pred1`, `diff10 = pred1 -
        // pred0`, at the SAME depth the predictors ran at.
        if hbd {
            self.cmp.residual1_16 = alloc::vec![0i16; n];
            self.cmp.diff10_16 = alloc::vec![0i16; n];
            compound_residuals_hbd(
                &mut self.cmp.residual1_16,
                &mut self.cmp.diff10_16,
                b.y_src10,
                bw,
                &self.cmp.pred0_16,
                &self.cmp.pred1_16,
                bw,
                bh,
            );
        } else {
            self.cmp.residual1_8 = alloc::vec![0i16; n];
            self.cmp.diff10_8 = alloc::vec![0i16; n];
            let src8_off = b.org_y * f.src_stride + b.org_x;
            compound_residuals(
                &mut self.cmp.residual1_8,
                &mut self.cmp.diff10_8,
                &f.src[src8_off..],
                f.src_stride,
                &self.cmp.pred0_8,
                &self.cmp.pred1_8,
                bw,
                bh,
            );
        }
        false
    }

    /// C `svt_aom_search_compound_diff_wedge`
    /// (enc_inter_prediction.c:3705-3710) — `pick_interinter_mask` over
    /// the store `calc_pred_masked_compound` just filled, writing the
    /// CODED `wedge_index`/`wedge_sign` (WEDGE) or `mask_type`
    /// (DIFFWTD) on the candidate.
    fn search_compound_diff_wedge(&mut self, cand: &mut crate::port_md::inject::InterCandidate) {
        use svtav1_dsp::port_masked_compound::CompoundType;
        use svtav1_dsp::port_wedge_search::{PickedMask, SearchCtx, pick_interinter_mask};

        let f = self.f;
        let b = self.b;
        let bsize = svtav1_types::block::BlockSize::from_u8(b.bsize)
            .expect("an injected inter block must have a real BlockSize");
        let comp_type = match cand.interinter_comp_type {
            t if t == CompoundType::Wedge as u8 => CompoundType::Wedge,
            t if t == CompoundType::DiffWtd as u8 => CompoundType::DiffWtd,
            _ => return,
        };
        // The pair contract: `determine_compound_mode` reaches this only
        // inside `inj_comp_modes`, after `calc_pred_masked_compound` ran
        // for the same ref pair.
        let Some(hbd) = self.cmp.hbd else {
            debug_assert!(
                false,
                "search_compound_diff_wedge before calc_pred_masked_compound"
            );
            return;
        };
        let ctx = SearchCtx {
            hbd,
            full_lambda: if hbd { b.full_lambda10 } else { b.full_lambda8 },
            use_rate: self.comp_ctrls.use_rate,
            quantizer: b.quantizer,
        };
        let src8_off = b.org_y * f.src_stride + b.org_x;
        let empty8: &[u8] = &[];
        let empty16: &[u16] = &[];
        let (src8, src16, src_stride, p0_8, p1_8, p0_16, p1_16, r1, d10) = if hbd {
            (
                empty8,
                b.y_src10,
                b.bw,
                empty8,
                empty8,
                &self.cmp.pred0_16[..],
                &self.cmp.pred1_16[..],
                &self.cmp.residual1_16[..],
                &self.cmp.diff10_16[..],
            )
        } else {
            (
                &f.src[src8_off..],
                empty16,
                f.src_stride,
                &self.cmp.pred0_8[..],
                &self.cmp.pred1_8[..],
                empty16,
                empty16,
                &self.cmp.residual1_8[..],
                &self.cmp.diff10_8[..],
            )
        };
        match pick_interinter_mask(
            &f.wedge_masks,
            &ctx,
            comp_type,
            bsize,
            src8,
            src16,
            src_stride,
            p0_8,
            p1_8,
            p0_16,
            p1_16,
            r1,
            d10,
        ) {
            Some(PickedMask::Wedge { sign, index }) => {
                cand.interinter_wedge_sign = sign != 0;
                cand.interinter_wedge_index = index as i8;
            }
            Some(PickedMask::Seg(mask_type)) => {
                cand.interinter_mask_type = mask_type as u8;
            }
            // C `assert(0)`s on a non-masked type; `pick_interinter_mask`
            // returns `None` there, which cannot happen on this dispatch.
            None => {}
        }
    }
}
