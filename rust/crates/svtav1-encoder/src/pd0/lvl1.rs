use super::*;

impl<'a> Pd0Ctx<'a> {
    /// PD0_LVL_1 block cost (md_encode_block_pd0 at allintra M2..M8):
    /// same DC-from-source prediction, but `lpd0_qp_offset = 0`, subres
    /// permanently off (`pd0_level <= PD0_LVL_2` -> subres_level 0), and
    /// the REAL coefficient rate (`coeff_rate_est_lvl = 1` ->
    /// svt_aom_txb_estimate_coeff_bits_pd0 with zero contexts).
    pub(super) fn lvl1_block_cost(&mut self, sq_size: usize, org_x: usize, org_y: usize) -> u64 {
        self.lvl1_block_cost_rect(sq_size, sq_size, org_x, org_y)
    }

    /// Non-square generalisation of the PD0_LVL_1 block cost. `bw == bh` is
    /// the square PART_N path (unchanged); `bw != bh` costs the single
    /// in-frame PARTITION_HORZ / PARTITION_VERT block of a partial-SB boundary
    /// node (task #95 chunk 2) — C's LPD0 "single block per shape ... PART_H/
    /// PART_V for boundary blocks" (product_coding_loop.c:127). The DC
    /// predictor, residual, `tx_quant_core` (Tx32x16 / Tx16x8 / …) and PD0
    /// coeff-rate estimator are all dimension-general.
    pub(super) fn lvl1_block_cost_rect(
        &mut self,
        bw: usize,
        bh: usize,
        org_x: usize,
        org_y: usize,
    ) -> u64 {
        let abs_x = self.sb_x + org_x;
        let abs_y = self.sb_y + org_y;
        // C `md_encode_block_pd0` (product_coding_loop.c:8370): with
        // `pd0_use_src_samples` the SOURCE row/column is copied into the recon
        // neighbour arrays, so predicting straight off the source plane IS the
        // allintra arm. Without it the arrays hold PD0's own recon, and the
        // canvas is that state. The availability, `n_top_px`/`n_left_px` clamp
        // and edge replication are the SAME function either way.
        // C `product_prediction_fun_table_pd0[is_inter_mode(mode)]`
        // (product_coding_loop.c:970). On a non-I slice PD0's ONLY candidate is
        // an inter NEWMV (see [`Pd0InterRef`]), so the INTRA neighbour
        // extraction below is not merely unused there — C never runs it,
        // because `pd0_use_src_samples` is false and `skip_intra` is 1.
        if let Some(ir) = self.inter {
            return self.lvl1_block_cost_inter(ir, bw, bh, abs_x, abs_y);
        }
        // Ghost Robot `2c66d9ea` — `hbd_md` effective: 16-bit source
        // neighbours + u16 DC prediction + `svt_residual_kernel16bit` + the
        // `quants_bd` quantize, priced at `full_sb_lambda_md[EB_10_BIT_MD]`
        // (`self.lambda`). On the allintra arm this serves the LVL_0-forced
        // refinement eval too — `svt_aom_sig_deriv_enc_dec_pd0` resolves
        // `rate_est_level` 2/4 there (`lpd0_qp_offset` 0 + real coeff rate),
        // which is exactly this block cost's shape.
        if let Some(s16) = self.src16 {
            let (above, left, _tl, has_above, has_left) = crate::partition::extract_neighbors_hbd(
                s16.src,
                s16.stride,
                abs_x,
                abs_y,
                bw,
                bh,
                10,
                self.tile_top,
                self.tile_left,
                self.aligned_w,
                self.aligned_h,
            );
            let mut pred16 = core::mem::take(&mut self.scratch.pred16);
            svtav1_dsp::hbd::predict_dc_hbd(
                scratch_u16(&mut pred16, bw * bh),
                bw,
                &above,
                &left,
                bw,
                bh,
                has_above,
                has_left,
                10,
            );
            let cost = self.lvl1_cost_from_pred_hbd(s16, bw, bh, abs_x, abs_y, &pred16);
            self.scratch.pred16 = pred16;
            return cost;
        }
        let nb = match self.recon_canvas.as_ref() {
            None => crate::partition::extract_neighbors_tiled(
                self.src,
                self.stride,
                abs_x,
                abs_y,
                bw,
                bh,
                self.tile_top,
                self.tile_left,
                self.aligned_w,
                self.aligned_h,
            ),
            Some(cv) => {
                // Shift the row axis into the canvas's window. `tile_top` and
                // `aligned_h` shift with it, so `abs_y > tile_top` and
                // `aligned_h - abs_y` are unchanged; the column axis is not
                // windowed at all.
                crate::partition::extract_neighbors_tiled(
                    &cv.buf,
                    cv.stride,
                    abs_x,
                    abs_y - cv.y0,
                    bw,
                    bh,
                    self.tile_top.saturating_sub(cv.y0),
                    self.tile_left,
                    self.aligned_w,
                    self.aligned_h - cv.y0,
                )
            }
        };
        let (above, left, _tl, has_above, has_left) = nb.parts();
        // `pred` is taken out of the scratch so it can be borrowed across the
        // `&mut self` call; it is put back afterwards.
        let mut pred = core::mem::take(&mut self.scratch.pred);
        svtav1_dsp::intra_pred::predict_dc(
            scratch_u8(&mut pred, bw * bh),
            bw,
            above,
            left,
            bw,
            bh,
            has_above,
            has_left,
        );
        let cost = self.lvl1_cost_from_pred(
            bw,
            bh,
            abs_x,
            abs_y,
            &pred,
            Some((&above, &left, has_above, has_left)),
        );
        self.scratch.pred = pred;
        cost
    }

    /// [`Pd0Ctx::lvl1_block_cost_rect`] from the point the PREDICTION exists —
    /// C `full_loop_core_pd0` (product_coding_loop.c:5963) plus the recon
    /// generation `md_encode_block_pd0` runs after it.
    ///
    /// Split out because the two arms of
    /// `product_prediction_fun_table_pd0` differ ONLY in how `pred` is
    /// produced; everything from the residual down is one C function that both
    /// reach. `nb` is the intra arm's neighbour state, carried only so the
    /// `SVTAV1_PD0DBG` line keeps printing it; `None` is the inter arm.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn lvl1_cost_from_pred(
        &mut self,
        bw: usize,
        bh: usize,
        abs_x: usize,
        abs_y: usize,
        pred: &[u8],
        nb: Option<(&[u8], &[u8], bool, bool)>,
    ) -> u64 {
        // `subres_ctrls.step`, and the per-SB safety check that gates it —
        // identical machinery to [`Pd0Ctx::lvl5_like_block_cost`], because it
        // is the same `full_loop_core_pd0` code. LVL_1 / LVL_0 configure step
        // 0 (`pd0_level <= PD0_LVL_2`, enc_mode_config.c:7337) so nothing here
        // runs for them and the pre-existing allintra paths are unchanged by
        // construction; LVL_3 / LVL_4 configure step 1.
        let subres_step_cfg = self.subres_step_cfg();
        if subres_step_cfg > 0 && bw == 64 && bh == 64 && self.is_subres_safe == 255 {
            self.is_subres_safe = u8::from(check_is_subres_safe(
                self.src,
                self.stride,
                abs_x,
                abs_y,
                pred,
            ));
        }
        let mut step = if bh >= 16 {
            subres_step_cfg
        } else {
            subres_step_cfg.min(1)
        };
        if self.is_subres_safe != 1 {
            step = 0;
        }

        let tx_h = bh >> step;
        // C `svt_residual_kernel8bit`, via the dsp kernel that already carries
        // NEON and AVX2 arms. The sub-resolution `step` is expressed as a
        // DOUBLED stride on both sides — `(r << step) * stride` is
        // `r * (stride << step)` — which is exactly what this loop did with its
        // shifted row index.
        let stride = self.stride;
        let src = self.src;
        svtav1_dsp::residual::residual_i16(
            &src[abs_y * stride + abs_x..],
            stride << step,
            pred,
            bw << step,
            bw,
            tx_h,
            scratch_i16(&mut self.scratch.residual, bw * tx_h),
        );
        let (eob, dist, c_tx) = tx_quant_core(
            &mut self.scratch,
            bw,
            tx_h,
            self.qindex,
            self.qm_level,
            step,
            8,
            0,
        );
        // TEMPORARY drill: residual + coeff dump pinned to one block.
        // `SVTAV1_ETXF=<px>,<py>` parsed once — a per-block env lookup would
        // sit inside the PD0 eval's hottest loop.
        #[cfg(feature = "std")]
        if let Some((px, py)) = etxf_pin()
            && abs_x == px
            && abs_y == py
            && bw == 8
            && tx_h == 8
        {
            eprint!("RSRES org=({abs_x},{abs_y}) res=[");
            for i in 0..(bw * tx_h).min(32) {
                eprint!("{},", self.scratch.residual[i]);
            }
            eprintln!("]");
            eprint!("RSCO org=({abs_x},{abs_y}) eob={eob} dist={dist} co=[");
            for i in 0..(bw * tx_h).min(24) {
                eprint!("{},", self.scratch.coeffs[i]);
            }
            eprintln!("]");
            eprint!("RSDQ dq=[");
            for i in 0..(bw * tx_h).min(24) {
                eprint!("{},", self.scratch.dqcoeff[i]);
            }
            eprintln!("]");
        }
        self.note_root_eob(bw, bh, abs_x, abs_y, eob);
        let tables = self.lvl1.expect("LVL_1 requires tables");
        // C `perform_tx_pd0` luma coeff rate (single-txb, product_coding_
        // loop.c:4501-4508): `th = (bwidth*bheight)>>5` where `bwidth =
        // txbwidth < 64 ? txbwidth : 32` and likewise for the height —
        // `.min(32)` is the same map on every power-of-two size <= 64.
        // coeff_rate_est_lvl 2 prices `eob < th ? 6000 + eob*500 : real`; the
        // eob==0 -> 6000 case folds into `eob < th`. Level 1 keeps the real
        // cost / skip cost.
        //
        // The HEIGHT here is the TRANSFORM's, not the block's: at
        // `mds_subres_step == 1` C rewrites `tx_size` TX_NxN -> TX_NxN/2
        // (`:4332-4344`) before `txbheight` is read, so an 8x8 block under
        // subres has `th = (8*4)>>5 = 1` and NOT 2. MEASURED on
        // `screenrep 72x88 q40 p8` video against C's `SVT_PD0COST_OUT`: with
        // `bh` the port priced every 8x8 at the 6500 shortcut where C priced
        // the real rate (~31528), 83 of 130 PD0 block costs differing. With
        // `tx_h` all 130 agree.
        //
        // It could not matter before PD0_LVL_4 was wired: `th` is read only
        // when `coeff_rate_est_lvl >= 2`, and the only levels that set that
        // are the allintra M7/M8 rows — which are PD0_LVL_1, subres step 0,
        // where `tx_h == bh`.
        let cw = bw.min(32);
        let ch = tx_h.min(32);
        let th = (cw * ch) >> 5;
        let mut bits = if self.coeff_rate_est_lvl >= 2 && (eob as usize) < th {
            6000 + eob as u64 * 500
        } else if eob == 0 {
            cost_skip_txb_pd0(c_tx, &tables.coeff) as u64
        } else {
            // `is_inter` selects the rate table — C's
            // `av1_transform_type_rate_estimation` reads
            // `inter_tx_type_fac_bits` on the inter arm, not the intra@DC
            // rows the intra arm uses.
            let tx_rates = if self.inter.is_some() {
                Pd0TxRates::Inter(&tables.tx_rates_inter)
            } else {
                Pd0TxRates::Intra(&tables.tx_rates)
            };
            cost_coeffs_txb_pd0(
                &self.scratch.qcoeff[..cw * ch],
                eob,
                c_tx,
                &tables.coeff,
                tx_rates,
                step,
            ) as u64
        };
        // Ghost Robot: `svt_psy_adjust_rate_light` on `txb_coeff_bits`
        // after whichever rate arm produced it (perform_tx_pd0,
        // product_coding_loop.c:4511-4514); `recon_coeff` = the packed
        // dequantized tx block.
        if self.ac_bias_eff != 0.0 {
            bits = svtav1_dsp::ac_bias::psy_adjust_rate_light(
                &self.scratch.dqcoeff[..cw * ch],
                bits,
                cw,
                ch,
                self.ac_bias_eff,
            );
        }
        let rate = bits + tables.skip0_bits + tables.none_bits_ctx0;
        let cost = rdcost(self.lambda, rate, dist);
        // C `md_encode_block_pd0` (product_coding_loop.c:8429): on the VIDEO
        // arm PD0 generates the block's RECON so the next block can predict
        // from it. `av1_perform_inverse_transform_recon` (:752) inverts the
        // SUB-SAMPLED transform into the recon's EVEN rows at a doubled stride
        // and then copies each even row down onto the odd row below it (:859);
        // with no coefficients it is a straight `svt_av1_picture_copy_y` of
        // the prediction (:873).
        if self.recon_canvas.is_some() {
            let mut recon = alloc::vec![0u8; bw * bh];
            if eob > 0 {
                let n = bw * tx_h;
                let packed_w = bw.min(32);
                let packed_h = tx_h.min(32);
                // The inverse transform reads the whole `bw * tx_h` input;
                // the scatter only fills the packed region, so the tail of
                // `full` must be zeroed on every call.
                let full = scratch_i32(&mut self.scratch.full, n);
                full.fill(0);
                let dqcoeff = &self.scratch.dqcoeff[..packed_w * packed_h];
                for r in 0..packed_h {
                    for c in 0..packed_w {
                        full[r * bw + c] = dqcoeff[r * packed_w + c];
                    }
                }
                let (tx_size, _) = pd0_tx_size(bw, tx_h);
                let inv = scratch_i32(&mut self.scratch.inv, n);
                svtav1_dsp::txfm_dispatch::inv_txfm2d_dispatch(
                    full,
                    inv,
                    bw,
                    tx_size,
                    svtav1_types::transform::TxType::DctDct,
                );
                for r in 0..tx_h {
                    let dst = (r << step) * bw;
                    for c in 0..bw {
                        recon[dst + c] =
                            (i32::from(pred[dst + c]) + inv[r * bw + c]).clamp(0, 255) as u8;
                    }
                    if step > 0 && (r << step) + 1 < bh {
                        let (a, b) = recon.split_at_mut(dst + bw);
                        b[..bw].copy_from_slice(&a[dst..dst + bw]);
                    }
                }
            } else {
                recon.copy_from_slice(&pred[..bw * bh]);
            }
            self.pending_recon = Some(recon);
        }
        // `SVTAV1_PD0DBG`: the port-side twin of the C `SVT_PD0COST_OUT`
        // interposer on `svt_aom_full_cost_pd0`. Same fields, same order, so
        // the two dumps join block-for-block without a translation step.
        #[cfg(feature = "std")]
        if crate::dbgenv::pd0dbg() {
            eprintln!(
                "PD0BLK org=({},{}) {}x{} dist={} ybits={} cost={} lambda={} eob={} qidx={} subres={} dc={} ha={} hl={} a0={:?} l0={:?}",
                abs_x,
                abs_y,
                bw,
                bh,
                dist,
                bits,
                cost,
                self.lambda,
                eob,
                self.qindex,
                step,
                pred[0],
                u8::from(nb.is_some_and(|n| n.2)),
                u8::from(nb.is_some_and(|n| n.3)),
                nb.map_or(&[][..], |n| &n.0[..n.0.len().min(4)]),
                nb.map_or(&[][..], |n| &n.1[..n.1.len().min(4)])
            );
        }
        cost
    }

    /// [`Pd0Ctx::lvl1_cost_from_pred`]'s 16-bit twin — Ghost Robot
    /// `2c66d9ea` (`hbd_md` effective): `svt_residual_kernel16bit` diffs the
    /// u16 prediction against `input_frame16bit`, `perform_tx_pd0` at
    /// `EB_TEN_BIT` quantizes with `quants_bd` + the highbd kernels, and
    /// `svt_aom_full_cost_pd0` prices at `full_sb_lambda_md[EB_10_BIT_MD]`
    /// (`self.lambda`). The coeff-rate arms and the psy adjustment are
    /// depth-independent — identical to the 8-bit twin.
    ///
    /// `mds_subres_step` is 0 on every arm `src16` serves (the `hbd_md`
    /// force resolves PD0_LVL_0; `pd0_level <= PD0_LVL_2` forces
    /// `subres_level` 0 — enc_mode_config.c:7327), so `tx_h == bh` and no
    /// subres-safety check runs. The u8 `recon_canvas` arm is skipped for
    /// the same reason [`Pd0Hbd`] documents: `pd0_use_src_samples` stays
    /// true on the allintra arm.
    pub(super) fn lvl1_cost_from_pred_hbd(
        &mut self,
        s16: Pd0Src16<'_>,
        bw: usize,
        bh: usize,
        abs_x: usize,
        abs_y: usize,
        pred16: &[u16],
    ) -> u64 {
        debug_assert_eq!(
            self.subres_step_cfg(),
            0,
            "hbd PD0 is PD0_LVL_0 — subres off"
        );
        svtav1_dsp::pic_operators::residual_kernel_16bit(
            &s16.src[abs_y * s16.stride + abs_x..],
            s16.stride,
            pred16,
            bw,
            scratch_i16(&mut self.scratch.residual, bw * bh),
            bw,
            bw,
            bh,
        );
        let (eob, dist, c_tx) = tx_quant_core(
            &mut self.scratch,
            bw,
            bh,
            self.qindex,
            self.qm_level,
            0,
            10,
            self.sharpness,
        );
        self.note_root_eob(bw, bh, abs_x, abs_y, eob);
        let tables = self.lvl1.expect("LVL_1 requires tables");
        // The same `perform_tx_pd0` rate arms the u8 twin runs —
        // `coeff_rate_est_lvl >= 2` shortcut, `eob == 0` skip cost, else the
        // real coefficient rate.
        let cw = bw.min(32);
        let ch = bh.min(32);
        let th = (cw * ch) >> 5;
        let mut bits = if self.coeff_rate_est_lvl >= 2 && (eob as usize) < th {
            6000 + eob as u64 * 500
        } else if eob == 0 {
            cost_skip_txb_pd0(c_tx, &tables.coeff) as u64
        } else {
            cost_coeffs_txb_pd0(
                &self.scratch.qcoeff[..cw * ch],
                eob,
                c_tx,
                &tables.coeff,
                Pd0TxRates::Intra(&tables.tx_rates),
                0,
            ) as u64
        };
        // `svt_psy_adjust_rate_light` on `txb_coeff_bits`
        // (product_coding_loop.c:4511-4514) — `recon_coeff` is the packed
        // dequantized tx block.
        if self.ac_bias_eff != 0.0 {
            bits = svtav1_dsp::ac_bias::psy_adjust_rate_light(
                &self.scratch.dqcoeff[..cw * ch],
                bits,
                cw,
                ch,
                self.ac_bias_eff,
            );
        }
        let rate = bits + tables.skip0_bits + tables.none_bits_ctx0;
        let cost = rdcost(self.lambda, rate, dist);
        #[cfg(feature = "std")]
        if crate::dbgenv::pd0dbg() {
            eprintln!(
                "PD0BLK org=({abs_x},{abs_y}) {bw}x{bh} dist={dist} ybits={bits} cost={cost} lambda={} eob={eob} qidx={} subres=0 hbd=1",
                self.lambda, self.qindex,
            );
        }
        cost
    }
}
