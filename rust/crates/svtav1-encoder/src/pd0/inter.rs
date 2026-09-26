use super::*;

impl<'a> Pd0Ctx<'a> {
    /// `md_stage_0_pd0`'s candidate selection — `inject_new_candidates_pd0`
    /// (mode_decision.c:2293) injects EVERY surviving ME candidate for the
    /// block's ME PU, `md_stage_0_pd0` (product_coding_loop.c:1507) picks the
    /// argmin-VARIANCE one (`fast_cost` = `svt_aom_mefn_ptr[bsize].vf`, the
    /// two-buffer difference variance), and `md_stage_3_pd0` runs the real
    /// residual/TX/coeff cost on that winner alone. Evaluating candidate 0
    /// only — this arm's earlier form — mispriced any PU whose second or
    /// third candidate won (MEASURED on `diag 72x72 q20 p6` frame 3: C's
    /// `(64,16)` 8x16 winner is `ref_idx_l0 = 1` at dist 20 where candidate
    /// 0 reads dist 2301).
    ///
    /// The `cand_total_cnt > 2` break in C caps the INJECTED count at three —
    /// bipred candidates skipped by `allow_bipred` do not count.
    ///
    /// Shared by every inter PD0 level that runs `md_encode_block_pd0` (the
    /// LVL_1 family AND LVL_5): the candidate set and the pick are level-
    /// independent; the level decides only the cost model downstream.
    /// [`Pd0Ctx::inter_best_pred`] with caller-owned buffers: `best` receives
    /// the winning `bw * bh` prediction (or the zero-MV fallback) and `cand`
    /// is the per-candidate working buffer. A win SWAPS the two — the loser's
    /// contents are overwritten before they are ever read.
    /// Returns the winning candidate's quarter-pel `(mv0, mv1, is_bipred)` —
    /// `mv1` is `Mv::ZERO` and `is_bipred` false for a unipred winner.
    /// `lpd1_detector_post_pd0`'s `block_mi.mv[0/1]`/`has_second_ref` tests
    /// read these; [`Pd0Ctx::lvl1_block_cost_inter`] stores them on the ROOT
    /// block only.
    pub(super) fn inter_best_pred_into(
        &self,
        ir: &Pd0InterRef<'_>,
        bw: usize,
        bh: usize,
        abs_x: usize,
        abs_y: usize,
        best: &mut Vec<u8>,
        cand: &mut Vec<u8>,
    ) -> (svtav1_types::motion::Mv, svtav1_types::motion::Mv, bool) {
        let bsize = pd0_bsize(bw, bh);
        let cands = ir.me.cands_for(abs_x, abs_y, bsize);
        // C `inject_inter_candidates_pd0` (mode_decision.c:2828): compound is
        // out when the frame is single-reference or either dim is 4.
        let allow_bipred = ir.ref_mode_not_single && bw > 4 && bh > 4;
        let src = &self.src[abs_y * self.stride + abs_x..];
        let n = bw * bh;
        let mut best_var = u64::MAX;
        let mut have_best = false;
        let mut best_mv0 = svtav1_types::motion::Mv::ZERO;
        let mut best_mv1 = svtav1_types::motion::Mv::ZERO;
        let mut best_bipred = false;
        let mut injected = 0u32;
        for (i, c) in cands.iter().enumerate() {
            let dir = c.direction();
            let mv_dbg;
            // `cmv0` is assigned on every path that reaches the cost compare
            // (both non-`continue` arms), so it needs no initializer.
            let cmv0;
            let mut cmv1 = svtav1_types::motion::Mv::ZERO;
            let cbip = dir >= crate::inter_me::context::BI_PRED;
            if dir < crate::inter_me::context::BI_PRED {
                let Some((_d, mv_fp)) = ir.me.cand_mv_for(abs_x, abs_y, bsize, i) else {
                    continue;
                };
                let ref_idx = if dir == 1 {
                    c.ref_idx_l1()
                } else {
                    c.ref_idx_l0()
                };
                let rf = crate::port_picstruct::get_ref_frame_type(dir, ref_idx);
                mv_dbg = (mv_fp.y * 8, mv_fp.x * 8, rf);
                cmv0 = svtav1_types::motion::Mv {
                    x: mv_fp.x.saturating_mul(8),
                    y: mv_fp.y.saturating_mul(8),
                };
                self.inter_pred_into(ir, bw, bh, abs_x, abs_y, mv_fp, rf, scratch_u8(cand, n));
            } else if allow_bipred {
                let Some(((mv0, rf0), (mv1, rf1))) = ir.me.cand_bipred_mvs(abs_x, abs_y, bsize, i)
                else {
                    continue;
                };
                mv_dbg = (mv0.y * 8, mv0.x * 8, rf0);
                cmv0 = svtav1_types::motion::Mv {
                    x: mv0.x.saturating_mul(8),
                    y: mv0.y.saturating_mul(8),
                };
                cmv1 = svtav1_types::motion::Mv {
                    x: mv1.x.saturating_mul(8),
                    y: mv1.y.saturating_mul(8),
                };
                self.inter_pred_into_bipred(
                    ir,
                    bw,
                    bh,
                    abs_x,
                    abs_y,
                    mv0,
                    rf0,
                    mv1,
                    rf1,
                    scratch_u8(cand, n),
                );
            } else {
                continue;
            }
            let var = u64::from(svtav1_dsp::variance::variance_diff(
                cand,
                bw,
                src,
                self.stride,
                bw,
                bh,
            ));
            #[cfg(feature = "std")]
            if crate::dbgenv::pd0dbg() {
                eprintln!(
                    "PD0CAND org=({abs_x},{abs_y}) {bw}x{bh} cand={i} dir={dir} var={var} mv={},{} ref={}",
                    mv_dbg.0, mv_dbg.1, mv_dbg.2
                );
            }
            if var < best_var {
                best_var = var;
                core::mem::swap(best, cand);
                best_mv0 = cmv0;
                best_mv1 = cmv1;
                best_bipred = cbip;
                have_best = true;
            }
            injected += 1;
            if injected > 2 {
                break;
            }
        }
        #[cfg(feature = "std")]
        if crate::dbgenv::pd0dbg() && crate::dbgenv::pd0pred() {
            if have_best {
                eprint!("PD0PRED org=({abs_x},{abs_y}) {bw}x{bh}");
                for r in 0..bh.min(4) {
                    eprint!(" r{r}=");
                    for c in 0..bw.min(16) {
                        eprint!("{},", best[r * bw + c]);
                    }
                }
                eprintln!();
            }
        }
        // `inject_zz_backup_candidate` (mode_decision.c:3314): zero-MV NEWMV
        // on LAST when the PU's candidate list is empty.
        if !have_best {
            self.inter_pred_into(
                ir,
                bw,
                bh,
                abs_x,
                abs_y,
                svtav1_types::motion::Mv::ZERO,
                1, // LAST_FRAME
                scratch_u8(best, n),
            );
        }
        (best_mv0, best_mv1, best_bipred)
    }

    /// C `compute_lpd0_cost_inter` (product_coding_loop.c:8267) — the
    /// PD0_LVL_6 block cost on a NON-KEY frame. For each of the block's
    /// surviving PA-ME candidates (BI_PRED skipped, the first THREE
    /// evaluated — `if (++cand_count > 2) break`), clip the full-pel ME MV
    /// against the candidate's own reference and take the prediction
    /// VARIANCE (`svt_aom_mefn_ptr[bsize].vf`); the cheapest goes through
    /// `compute_lpd0_cost_from_variance` (:8247):
    ///
    /// ```text
    /// dist = MIN(variance / area, lambda >> 10) * area
    /// cost = RDCOST(full_sb_lambda_md[8bit],
    ///               partition_fac_bits[0][PARTITION_NONE], dist)
    /// ```
    ///
    /// Unlike the LVL_1/LVL_5 inter arm there is no recon to carry —
    /// `md_encode_block_pd0` returns after writing only `blk_ptr->cost`, so
    /// `pending_recon` stays empty and the neighbour-array write is a
    /// dead canvas copy either way.
    pub(super) fn lvl6_block_cost_inter(
        &self,
        bw: usize,
        bh: usize,
        org_x: usize,
        org_y: usize,
    ) -> u64 {
        let ir = self
            .inter
            .expect("lvl6_block_cost_inter is the `inter.is_some()` arm");
        let abs_x = self.sb_x + org_x;
        let abs_y = self.sb_y + org_y;
        let bsize = pd0_bsize(bw, bh);
        let src = &self.src[abs_y * self.stride + abs_x..];
        let mut best: Option<u32> = None;
        let mut evaluated = 0u32;
        for (i, c) in ir.me.cands_for(abs_x, abs_y, bsize).iter().enumerate() {
            let dir = c.direction();
            // `if (direction == BI_PRED) continue;` — compound candidates are
            // never costed here regardless of `ref_mode_not_single`.
            if dir >= crate::inter_me::context::BI_PRED {
                continue;
            }
            let Some((_d, mv_fp)) = ir.me.cand_mv_for(abs_x, abs_y, bsize, i) else {
                continue;
            };
            let ref_idx = if dir == 1 {
                c.ref_idx_l1()
            } else {
                c.ref_idx_l0()
            };
            let plane =
                Self::inter_ref_plane(ir, crate::port_picstruct::get_ref_frame_type(dir, ref_idx));
            let mut mv = svtav1_types::motion::Mv {
                x: mv_fp.x.saturating_mul(8),
                y: mv_fp.y.saturating_mul(8),
            };
            crate::port_md::coding_loop::clip_mv_on_pic_boundary(
                abs_x as i32,
                abs_y as i32,
                bw as i32,
                bh as i32,
                plane.width as i32,
                plane.height as i32,
                plane.border as i32,
                &mut mv.x,
                &mut mv.y,
            );
            let var = Self::lvl6_ref_variance(plane, abs_x, abs_y, bw, bh, mv, src, self.stride);
            best = Some(best.map_or(var, |b: u32| b.min(var)));
            evaluated += 1;
            if evaluated > 2 {
                break;
            }
        }
        // `best_cost == (uint64_t)~0` — no unipred candidate survived: the
        // `inject_zz_backup_candidate` twin, a zero-MV read on LAST.
        let var = best.unwrap_or_else(|| {
            Self::lvl6_ref_variance(
                ir.padded_y,
                abs_x,
                abs_y,
                bw,
                bh,
                svtav1_types::motion::Mv::ZERO,
                src,
                self.stride,
            )
        });
        // `compute_lpd0_cost_from_variance`. `partition_fac_bits[0]
        // [PARTITION_NONE]` is the LIVE `md_rate_est_ctx` value — the chained
        // tables the video arm always carries (`lvl1` is `Some` here).
        let (_, none0) = self.skip0_none0_bits();
        let area = (bw * bh) as u32;
        let noise = u32::try_from(self.lambda >> 10).unwrap_or(u32::MAX);
        let var_pp = var / area;
        let dist = u64::from(var_pp.min(noise)) * u64::from(area);
        rdcost(self.lambda, none0, dist)
    }

    /// The `fn_ptr->vf` half of `compute_lpd0_cost_inter` — the `bwidth x
    /// bheight` window of the reference at the clipped eighth-pel MV (always
    /// pixel-aligned), vs the source block. C reads
    /// `ref_pic->y_buffer + ref_origin_index`, whose negative or past-extent
    /// offsets land on the replicated margin — the [`crate::picture::PaddedPlane`]
    /// border is what `clip_mv_on_pic_boundary` clips the MV into, so the
    /// slice stays in bounds.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn lvl6_ref_variance(
        plane: &crate::picture::PaddedPlane,
        abs_x: usize,
        abs_y: usize,
        bw: usize,
        bh: usize,
        mv: svtav1_types::motion::Mv,
        src: &[u8],
        src_stride: usize,
    ) -> u32 {
        let rx = abs_x as isize + isize::from(mv.x >> 3);
        let ry = abs_y as isize + isize::from(mv.y >> 3);
        let off = (plane.origin as isize + ry * plane.stride as isize + rx) as usize;
        svtav1_dsp::variance::variance_diff(
            &plane.buf[off..],
            plane.stride,
            src,
            src_stride,
            bw,
            bh,
        )
    }

    /// The inter arm of `md_encode_block_pd0` for the LVL_1 family — the
    /// [`Pd0Ctx::inter_best_pred`] winner fed through the LVL_1 cost model.
    pub(super) fn lvl1_block_cost_inter(
        &mut self,
        ir: &Pd0InterRef<'_>,
        bw: usize,
        bh: usize,
        abs_x: usize,
        abs_y: usize,
    ) -> u64 {
        let mut pred = core::mem::take(&mut self.scratch.pred);
        let mut cand = core::mem::take(&mut self.scratch.cand);
        let (mv0, mv1, bip) =
            self.inter_best_pred_into(ir, bw, bh, abs_x, abs_y, &mut pred, &mut cand);
        self.scratch.cand = cand;
        self.note_root_mv(
            bw,
            bh,
            abs_x - self.sb_x,
            abs_y - self.sb_y,
            true,
            mv0,
            mv1,
            bip,
        );
        let cost = self.lvl1_cost_from_pred(bw, bh, abs_x, abs_y, &pred, None);
        self.scratch.pred = pred;
        cost
    }

    /// The `padded_by_ref` lookup C does as
    /// `svt_aom_get_ref_pic_buffer(pcs, ref_frame)` — falling back to
    /// [`Pd0InterRef::padded_y`] (LAST) when the slot is unfilled, which on
    /// this port's single-picture low-delay envelope is the same picture.
    pub(super) fn inter_ref_plane<'r>(
        ir: &Pd0InterRef<'r>,
        ref_frame: i8,
    ) -> &'r crate::picture::PaddedPlane {
        ir.padded_by_ref
            .get(ref_frame.max(0) as usize)
            .copied()
            .flatten()
            .map_or(ir.padded_y, |r| &r.y)
    }

    /// `svt_aom_inter_pu_prediction_av1_pd0` for ONE unipred candidate: the
    /// full-pel ME MV times 8 against the candidate's own reference picture.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn inter_pred_into(
        &self,
        ir: &Pd0InterRef<'_>,
        bw: usize,
        bh: usize,
        abs_x: usize,
        abs_y: usize,
        mv_fp: svtav1_types::motion::Mv,
        ref_frame: i8,
        out: &mut [u8],
    ) {
        assert!(
            self.is_lvl1_family() || matches!(self.mode, Pd0Mode::Lvl5),
            "PD0 inter compensation is wired for `md_encode_block_pd0`'s \
             levels (the LVL_1 family AND LVL_5), and the caller resolved \
             {:?}. C's PD0_LVL_6 inter arm is `compute_lpd0_cost_inter`'s \
             variance closed form, not this block encode; refusing rather \
             than costing an inter block with the wrong model.",
            self.mode
        );
        let mv = svtav1_types::motion::Mv {
            x: mv_fp.x.saturating_mul(8),
            y: mv_fp.y.saturating_mul(8),
        };
        crate::inter_pred_arm::predict_inter_luma_pd0(
            Self::inter_ref_plane(ir, ref_frame),
            abs_x,
            abs_y,
            bw,
            bh,
            mv,
            ir.sb_size,
            ir.frame_w,
            ir.frame_h,
            out,
            bw,
        );
    }

    /// The compound (NEW_NEWMV, MD_COMP_AVG) twin of [`Self::inter_pred_into`]
    /// — `av1_inter_prediction_pd0` with two MVs and two ref planes, whose
    /// convolve already averages when `mvs.len() > 1`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn inter_pred_into_bipred(
        &self,
        ir: &Pd0InterRef<'_>,
        bw: usize,
        bh: usize,
        abs_x: usize,
        abs_y: usize,
        mv0_fp: svtav1_types::motion::Mv,
        rf0: i8,
        mv1_fp: svtav1_types::motion::Mv,
        rf1: i8,
        out: &mut [u8],
    ) {
        let mv0 = svtav1_types::motion::Mv {
            x: mv0_fp.x.saturating_mul(8),
            y: mv0_fp.y.saturating_mul(8),
        };
        let mv1 = svtav1_types::motion::Mv {
            x: mv1_fp.x.saturating_mul(8),
            y: mv1_fp.y.saturating_mul(8),
        };
        let p0 = Self::inter_ref_plane(ir, rf0);
        let p1 = Self::inter_ref_plane(ir, rf1);
        let rp0 = svtav1_dsp::port_pd_pred::RefPlane {
            buf: &p0.buf,
            origin: p0.origin,
            stride: p0.stride,
            width: p0.width as i32,
            height: p0.height as i32,
        };
        let rp1 = svtav1_dsp::port_pd_pred::RefPlane {
            buf: &p1.buf,
            origin: p1.origin,
            stride: p1.stride,
            width: p1.width as i32,
            height: p1.height as i32,
        };
        let sf = svtav1_dsp::port_scale_factors::ScaleFactors::setup_for_frame(
            ir.frame_w as i32,
            ir.frame_h as i32,
            ir.frame_w as i32,
            ir.frame_h as i32,
        );
        let edges = crate::inter_pred_arm::mb_edges(abs_x, abs_y, bw, bh, ir.frame_w, ir.frame_h);
        svtav1_dsp::port_pd_pred::av1_inter_prediction_pd0(
            &svtav1_dsp::port_pd_pred::BlkGeom {
                org_x: abs_x as i32,
                org_y: abs_y as i32,
                bwidth: bw,
                bheight: bh,
                bwidth_uv: bw / 2,
                bheight_uv: bh / 2,
                super_block_size: ir.sb_size as i32,
            },
            &[
                svtav1_dsp::port_subpel_params::Mv { x: mv0.x, y: mv0.y },
                svtav1_dsp::port_subpel_params::Mv { x: mv1.x, y: mv1.y },
            ],
            &[rp0, rp1],
            &[sf, sf],
            &edges,
            out,
            bw,
        );
    }
}
