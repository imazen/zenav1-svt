/// C `av1_cost_literal(n)` (1/512-bit units).
#[inline]
pub(super) const fn cost_literal(n: i32) -> i32 {
    n * 512
}

/// Intra tx-type signalling rate for DCT_DCT at a DC-predicted block —
/// `av1_transform_type_rate_estimation` (rd_cost.c:107): nonzero only for
/// tx sizes whose intra ext-tx set has > 1 type (4x4, 8x8 and 16x16 among
/// the square PD0 sizes; 32/64 are DCT-only). Costs derive from the DEFAULT
/// `intra_ext_tx_cdf` rows (qindex-independent) at intra_dir = DC.
///
/// TX_4X4 matters for M0-M3, whose PD0 admits 4x4 leaves (`disallow_4x4` is
/// false ≤ M3). Omitting it made every PD0 4x4 leaf cost `tx4` bits too
/// cheap, systematically biasing PD0 toward SPLIT (real-content M2/M3).
#[derive(Debug, Clone, Copy)]
pub struct TxTypeRatesDc {
    pub(super) tx4: i32,
    pub(super) tx8: i32,
    pub(super) tx16: i32,
}

/// Inter tx-type signalling rate for DCT_DCT — the `is_inter` arm of
/// `av1_transform_type_rate_estimation` (rd_cost.c:127-131):
/// `inter_tx_type_fac_bits[ext_tx_set][square_tx_size][DCT_DCT]`, filled from
/// `fc->inter_ext_tx_cdf` exactly where C's `use_inter_ext_tx_for_txsize`
/// (md_rate_estimation.h:139) has a 1 — eset 1 at sqr {4x4,8x8}, eset 2 at
/// sqr {16x16}, eset 3 at sqr {8x8,16x16,32x32}. Every other slot stays 0,
/// matching the unfilled rows of C's table (rect transforms index by
/// sqr-map, so e.g. TX_32X16 reads eset 3 / sqr 16x16).
#[derive(Debug, Clone, Copy)]
pub struct TxTypeRatesInter {
    /// `[eset * EXT_TX_SIZES(4) + square_tx_size]`.
    pub(super) fac: [i32; 16],
}

pub(crate) fn build_tx_type_rates_inter_from_fc(
    fc: &crate::entropy::coeff_c::CoeffFc,
) -> TxTypeRatesInter {
    use crate::entropy::coeff_c as cc;
    // eset -> set_type, the inverse of `EXT_TX_SET_INDEX[1]`.
    const SET_TYPE_FOR_ESET: [usize; 4] = [
        0,
        cc::EXT_TX_SET_ALL16,
        cc::EXT_TX_SET_DTT9_IDTX_1DDCT,
        cc::EXT_TX_SET_DCT_IDTX,
    ];
    const USED_SQRS: [[usize; 3]; 4] = [
        [0, 0, 0],
        [0, 1, usize::MAX],
        [2, usize::MAX, usize::MAX],
        [1, 2, 3],
    ];
    let mut rates = TxTypeRatesInter { fac: [0; 16] };
    for eset in 1usize..4 {
        let set_type = SET_TYPE_FOR_ESET[eset];
        let nsyms = cc::AV1_NUM_EXT_TX_SET[set_type];
        let sym = cc::AV1_EXT_TX_IND[set_type][cc::DCT_DCT];
        for sq_tx in USED_SQRS[eset] {
            if sq_tx == usize::MAX {
                break;
            }
            let row = &fc.inter_ext_tx_cdf[eset * 4 + sq_tx];
            let mut costs = [0i32; 17];
            crate::quant::syntax_rate_from_cdf(&mut costs[..nsyms], row);
            rates.fac[eset * 4 + sq_tx] = costs[sym];
        }
    }
    rates
}

pub(crate) fn build_tx_type_rates_dc_from_fc(
    fc: &crate::entropy::coeff_c::CoeffFc,
) -> TxTypeRatesDc {
    use crate::entropy::coeff_c as cc;
    let mut rates = TxTypeRatesDc {
        tx4: 0,
        tx8: 0,
        tx16: 0,
    };
    for tx_size in [0usize, 1, 2] {
        // TX_4X4 = 0, TX_8X8 = 1, TX_16X16 = 2 in the C TxSize enum.
        let set_type = cc::ext_tx_set_type(tx_size, false, false);
        let eset = cc::EXT_TX_SET_INDEX[0][set_type];
        debug_assert!(eset > 0);
        let sq_tx = cc::TXSIZE_SQR_MAP[tx_size];
        let row = &fc.intra_ext_tx_cdf[(eset as usize * 4 + sq_tx) * 13 /* + DC=0 */];
        let mut costs = [0i32; 17];
        crate::quant::syntax_rate_from_cdf(&mut costs, row);
        let sym = cc::AV1_EXT_TX_IND[set_type][cc::DCT_DCT];
        let r = costs[sym];
        match tx_size {
            0 => rates.tx4 = r,
            1 => rates.tx8 = r,
            _ => rates.tx16 = r,
        }
    }
    rates
}

/// The arm-selected tx-type rate row passed into [`cost_coeffs_txb_pd0`] —
/// intra@DC on the allintra/intra arm, the inter table on the inter arm.
/// C makes the same choice inside `av1_transform_type_rate_estimation` off
/// the `is_inter` argument.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Pd0TxRates<'a> {
    Intra(&'a TxTypeRatesDc),
    Inter(&'a TxTypeRatesInter),
}

impl Pd0TxRates<'_> {
    #[inline]
    pub(super) fn rate_for(&self, c_tx_size: usize) -> i32 {
        use crate::entropy::coeff_c as cc;
        match self {
            Self::Intra(t) => t.rate_for(c_tx_size),
            Self::Inter(t) => {
                // `get_ext_tx_types > 1 && get_ext_tx_set > 0` collapses to
                // "not DCTONLY" for inter: the other inter set types all have
                // > 1 type and a positive eset.
                let set_type = cc::ext_tx_set_type(c_tx_size, true, false);
                if set_type == cc::EXT_TX_SET_DCTONLY {
                    return 0;
                }
                let eset = cc::EXT_TX_SET_INDEX[1][set_type];
                debug_assert!(eset > 0);
                t.fac[eset as usize * 4 + cc::TXSIZE_SQR_MAP[c_tx_size]]
            }
        }
    }
}

impl TxTypeRatesDc {
    #[inline]
    pub(super) fn rate_for(&self, c_tx_size: usize) -> i32 {
        use crate::entropy::coeff_c as cc;
        // C `av1_transform_type_rate_estimation` (rd_cost.c:107): the intra
        // tx-type bit is coded only when the ext-tx set is NOT DCT-only
        // (`sqr_up >= TX_32X32` => DCTONLY intra => 0), and its DCT_DCT cost
        // uses the tx's SQUARE-MAPPED CDF row. So a RECTANGULAR transform
        // charges the SAME rate as its square map — `ext_tx_set_type` and the
        // CDF row are identical for TX_16X8 and TX_8X8 (both DTT4_IDTX_1DDCT,
        // row TX_8X8). Rectangular transforms only occur at PD0 boundary
        // edge-shape blocks (partial SBs); the prior `_ => 0` dropped the
        // tx-type bit on every one of them (748 bits for TX_16X8/8X16),
        // undercosting the edge shape and flipping the edge-vs-SPLIT PD0
        // partition near-tie. SQUARE sizes are unchanged: TX_4X4/8X8/16X16 ->
        // tx4/tx8/tx16, TX_32X32/64X64 -> 0 (DCTONLY) — so 64-aligned frames
        // are byte-neutral.
        if cc::ext_tx_set_type(c_tx_size, false, false) == cc::EXT_TX_SET_DCTONLY {
            return 0;
        }
        match cc::TXSIZE_SQR_MAP[c_tx_size] {
            0 => self.tx4,  // sqr map TX_4X4  (TX_4X4/4X8/8X4/4X16/16X4)
            1 => self.tx8,  // sqr map TX_8X8  (TX_8X8/8X16/16X8/8X32/32X8)
            2 => self.tx16, // sqr map TX_16X16 (TX_16X16/16X32/32X16 -> but those are DCTONLY above)
            _ => 0,
        }
    }
}

/// C `mds_fast_coeff_est_level` on the PD0 pass
/// (`ctx->rate_est_ctrls.pd0_fast_coeff_est_level`, product_coding_loop.c:7026).
/// `set_rate_est_ctrls` (enc_mode_config.c:6428) assigns 2 for every PD0
/// `rate_est_level` this port reaches — 0, 2 and 4 — and 1 only at level 1,
/// which `svt_aom_sig_deriv_enc_dec_pd0` never selects (it picks 0, 2 or 4 and
/// then raises with `MAX`). MEASURED from C on the reference cell:
/// `SVT_PD0CFG_OUT` reports `fastcoef=2` on both arms.
pub(super) const PD0_FAST_COEFF_EST_LEVEL: i32 = 2;

/// C `av1_cost_coeffs_txb_loop_cost_eob` (rd_cost.c:255) for plane Y,
/// DCT_DCT (TX_CLASS_2D), dc_sign_ctx 0, `mds_fast_coeff_est_level = 2`.
/// `eob >= 1`.
///
/// `subres_step` is C's `ctx->mds_subres_step`, and it is LOAD-BEARING in the
/// middle loop: `c_start = MIN(eob - 2, eob / MAX(1, fast_coeff_est_level -
/// mds_subres_step))` (rd_cost.c:329). At step 0 the divisor is 2 and half the
/// scan is priced; at step 1 it is 1 and the WHOLE scan is priced. Dropping
/// that term under-prices a sub-sampled PD0 block by up to 2x.
#[allow(clippy::too_many_arguments)]
pub(super) fn loop_cost_eob_pd0(
    qcoeff: &[i32],
    eob: u16,
    scan: &[u16],
    coeff_contexts: &[i8],
    costs: &crate::quant::TxbCosts,
    levels_buf: &[u8],
    bwl: usize,
    subres_step: u32,
) -> i32 {
    use crate::entropy::coeff_c as cc;
    const TX_CLASS: usize = cc::TX_CLASS_2D;
    let eob = eob as usize;
    let lit = cost_literal(1);
    let mut cost = 0i32;

    if eob == 1 {
        // av1_cost_coeffs_txb_loop_cost_one_eob
        let v = qcoeff[0];
        let level = v.unsigned_abs() as i32;
        let coeff_ctx = coeff_contexts[0] as usize;
        cost += costs.base_eob_cost[coeff_ctx][(level.min(3) - 1) as usize];
        if v != 0 {
            let sign = usize::from(v < 0);
            cost += costs.dc_sign_cost[0][sign];
            if level > cc::NUM_BASE_LEVELS {
                let base_range = level - 1 - cc::NUM_BASE_LEVELS;
                if base_range < cc::COEFF_BASE_RANGE {
                    cost += costs.lps_cost[0][base_range as usize];
                } else {
                    cost += costs.lps_cost[0][cc::COEFF_BASE_RANGE as usize];
                }
                if level >= 1 + cc::NUM_BASE_LEVELS + cc::COEFF_BASE_RANGE {
                    cost += crate::quant::golomb_cost(level);
                }
            }
        }
        return cost;
    }

    // first (eob - 1) index
    {
        let pos = scan[eob - 1] as usize;
        let v = qcoeff[pos];
        let level = v.unsigned_abs() as i32;
        let coeff_ctx = coeff_contexts[pos] as usize;
        cost += costs.base_eob_cost[coeff_ctx][(level.min(3) - 1) as usize];
        if v != 0 {
            cost += lit;
            if level > cc::NUM_BASE_LEVELS {
                let ctx = cc::br_ctx(levels_buf, pos, bwl, TX_CLASS);
                let base_range = level - 1 - cc::NUM_BASE_LEVELS;
                if base_range < cc::COEFF_BASE_RANGE {
                    cost += costs.lps_cost[ctx][base_range as usize];
                } else {
                    cost += costs.lps_cost[ctx][cc::COEFF_BASE_RANGE as usize];
                }
                if level >= 1 + cc::NUM_BASE_LEVELS + cc::COEFF_BASE_RANGE {
                    cost += crate::quant::golomb_cost(level);
                }
            }
        }
    }
    // last (0) index
    {
        let v = qcoeff[0];
        let level = v.unsigned_abs() as i32;
        let coeff_ctx = coeff_contexts[0] as usize;
        cost += costs.base_cost[coeff_ctx][level.min(3) as usize];
        if v != 0 {
            let sign = usize::from(v < 0);
            cost += costs.dc_sign_cost[0][sign];
            if level > cc::NUM_BASE_LEVELS {
                let ctx = cc::br_ctx(levels_buf, 0, bwl, TX_CLASS);
                let base_range = level - 1 - cc::NUM_BASE_LEVELS;
                if base_range < cc::COEFF_BASE_RANGE {
                    cost += costs.lps_cost[ctx][base_range as usize];
                } else {
                    cost += costs.lps_cost[ctx][cc::COEFF_BASE_RANGE as usize];
                }
                if level >= 1 + cc::NUM_BASE_LEVELS + cc::COEFF_BASE_RANGE {
                    cost += crate::quant::golomb_cost(level);
                }
            }
        }
    }
    // Optimized middle loop (rd_cost.c:329): only the first
    // `eob / MAX(1, fast_coeff_est_level - mds_subres_step)` scan positions
    // (excluding DC and eob-1) are priced; the rest contribute nothing.
    let denom = (PD0_FAST_COEFF_EST_LEVEL - subres_step as i32).max(1);
    let c_start = (eob as i32 - 2).min(eob as i32 / denom);
    let mut cost_literal_cnt = 0u32;
    let mut c = c_start;
    while c >= 1 {
        let pos = scan[c as usize] as usize;
        let v = qcoeff[pos];
        cost_literal_cnt += u32::from(v != 0);
        let level = v.unsigned_abs() as i32;
        if level > cc::NUM_BASE_LEVELS {
            let ctx = cc::br_ctx(levels_buf, pos, bwl, TX_CLASS);
            let base_range = level - 1 - cc::NUM_BASE_LEVELS;
            cost += costs.base_cost[coeff_contexts[pos] as usize][3];
            if base_range < cc::COEFF_BASE_RANGE {
                cost += costs.lps_cost[ctx][base_range as usize];
            } else {
                cost += crate::quant::golomb_cost(level)
                    + costs.lps_cost[ctx][cc::COEFF_BASE_RANGE as usize];
            }
        } else {
            cost += costs.base_cost[coeff_contexts[pos] as usize][level as usize];
        }
        c -= 1;
    }
    cost + cost_literal_cnt as i32 * lit
}

/// C `svt_av1_cost_coeffs_txb` (rd_cost.c:355) specialized to the
/// PD0_LVL_1 call (rd_cost.c:1207 `svt_aom_txb_estimate_coeff_bits_pd0`):
/// plane Y, DCT_DCT, `txb_skip_ctx = 0`, `dc_sign_ctx = 0`,
/// `reduced_tx_set = 0`, no CDF updates. `eob > 0`.
pub(super) fn cost_coeffs_txb_pd0(
    qcoeff: &[i32],
    eob: u16,
    c_tx_size: usize,
    tables: &crate::quant::CoeffCostTables,
    tx_rates: Pd0TxRates<'_>,
    subres_step: u32,
) -> i32 {
    crate::entropy::coeff_c::with_txb_scratch(|sc| {
        cost_coeffs_txb_pd0_inner(qcoeff, eob, c_tx_size, tables, tx_rates, subres_step, sc)
    })
}

/// [`cost_coeffs_txb_pd0`]'s body, with the level map and the nz-map context
/// array supplied by the caller (see `coeff_c::TxbScratch`).
#[allow(clippy::too_many_arguments)]
pub(super) fn cost_coeffs_txb_pd0_inner(
    qcoeff: &[i32],
    eob: u16,
    c_tx_size: usize,
    tables: &crate::quant::CoeffCostTables,
    tx_rates: Pd0TxRates<'_>,
    subres_step: u32,
    sc: &mut crate::entropy::coeff_c::TxbScratch,
) -> i32 {
    use crate::entropy::coeff_c as cc;
    debug_assert!(eob > 0);
    let txs_ctx = cc::txsize_entropy_ctx(c_tx_size);
    let bwl = cc::txb_bwl(c_tx_size);
    let width = cc::txb_wide(c_tx_size);
    let height = cc::txb_high(c_tx_size);
    let scan = crate::entropy::scan_tables::scan(c_tx_size, 0);
    let coeff_costs = tables.txb(txs_ctx, 0);
    let eob_multi_size = cc::TXSIZE_LOG2_MINUS4[c_tx_size];
    let eob_bits = &tables.eob[eob_multi_size][0];

    let mut cost = coeff_costs.txb_skip_cost[0][0];

    let cc::TxbScratch {
        levels: levels_buf,
        ctx: ctx_buf,
        ..
    } = sc;
    // Body-anchored sub-slice — `eob > 1` fills it; at `eob <= 1`
    // `loop_cost_eob_pd0` reads only `lps_cost[0]`/position-derived contexts
    // and `get_nz_map_contexts` writes `ctx[scan[0]]` without touching the
    // map, but `levels_skip_init` keeps the sub-slice uniformly shaped.
    let levels: &[u8] = if eob > 1 {
        cc::txb_init_levels(qcoeff, width, height, levels_buf)
    } else {
        cc::levels_skip_init(levels_buf, width, height)
    };
    cost += tx_rates.rate_for(c_tx_size);
    cost += crate::quant::eob_cost(eob as i32, eob_bits, coeff_costs, cc::TX_CLASS_2D);

    // Same per-thread scratch as `leaf_funnel::coeff_rate::cost_coeffs_txb`'s
    // — see `cc::TxbScratch`. `get_nz_map_contexts` writes every position a
    // caller can read (`scan[0..eob]` on the scan-order arm, the whole raster
    // on SIMD), so no per-call clear.
    let n_ctx = width * height;
    debug_assert!(n_ctx <= cc::MAX_TXB_COEFF_AREA);
    cc::get_nz_map_contexts(
        levels,
        scan,
        eob as usize,
        c_tx_size,
        cc::TX_CLASS_2D,
        &mut ctx_buf[..n_ctx],
    );
    let coeff_contexts: &[i8] = &ctx_buf[..n_ctx];
    let cost = cost
        + loop_cost_eob_pd0(
            qcoeff,
            eob,
            scan,
            coeff_contexts,
            coeff_costs,
            levels,
            bwl,
            subres_step,
        );
    // C `svt_aom_txb_estimate_coeff_bits_pd0` (rd_cost.c:1224):
    // `*y_txb_coeff_bits <<= ctx->mds_subres_step` — the sub-sampled residual
    // stands in for the full one, so its RATE is scaled the same way its
    // DISTORTION is. Only the eob != 0 branch shifts; the skip cost does not.
    cost << subres_step
}

/// C `av1_cost_skip_txb` (rd_cost.c:213) at context 0: the eob == 0 rate.
pub(super) fn cost_skip_txb_pd0(c_tx_size: usize, tables: &crate::quant::CoeffCostTables) -> i32 {
    let txs_ctx = crate::entropy::coeff_c::txsize_entropy_ctx(c_tx_size);
    tables.txb(txs_ctx, 0).txb_skip_cost[0][1]
}

/// C `check_is_subres_safe` (product_coding_loop.c): SAD of even vs odd
/// rows of (src - pred) over the 64x64; safe iff the deviation is within
/// `odd_to_even_deviation_th = 5` percent.
pub(super) fn check_is_subres_safe(
    src: &[u8],
    stride: usize,
    org_x: usize,
    org_y: usize,
    pred: &[u8],
) -> bool {
    // Even/odd-row SADs: two strided `block_sad` calls compute exactly the
    // scalar loop's sums — every |src - pred| term is non-negative and a
    // 64x32 sub-block can reach at most 2048 * 255, far under u32::MAX.
    let base = org_y * stride + org_x;
    let sad_even =
        svtav1_dsp::me_sad::block_sad(&src[base..], 2 * stride, pred, 2 * 64, 64, 32) as i64;
    let sad_odd = svtav1_dsp::me_sad::block_sad(
        &src[base + stride..],
        2 * stride,
        &pred[64..],
        2 * 64,
        64,
        32,
    ) as i64;
    let deviation = ((sad_even.max(1) - sad_odd.max(1)) * 100) / sad_odd.max(1);
    deviation.abs() <= 5
}

// ---------------------------------------------------------------------------
// The PD0 walk
// ---------------------------------------------------------------------------
