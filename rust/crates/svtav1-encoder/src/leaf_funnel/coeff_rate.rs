//! Coefficient rate — `svt_av1_cost_coeffs_txb`, full scan, real contexts.
//!
//! Split out of `leaf_funnel.rs` on 2026-08-16 (11,247 lines).
//! PURE CODE MOVEMENT: every item keeps its name, order and effective
//! visibility (file-private became `pub(super)`, the same scope).

use super::*;

// ---------------------------------------------------------------------------
// Coefficient rate (svt_av1_cost_coeffs_txb, full scan, real contexts)
// ---------------------------------------------------------------------------

/// SVTAV1_CCOSTDBG: mirror the C --wrap interposer
/// (tools/capture_c_trace/wrap_recon.c __wrap_svt_av1_cost_coeffs_txb) so the
/// port's coeff-rate estimate can be diffed against C's for identical qcoeff
/// (the first coding block feeds both the same residual). Answers whether an
/// M2/M3 partition near-tie flips on RATE (this estimator) vs DISTORTION.
#[cfg(feature = "std")]
pub(super) fn ccost_log(
    plane: usize,
    c_tx_size: usize,
    tx_type: usize,
    eob: u16,
    skip: usize,
    dc: usize,
    qcoeff: &[i32],
    width: usize,
    height: usize,
    cost: i32,
) {
    use core::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::OnceLock;
    // Cache the env lookup — this fn is on the per-txb hot path, so a getenv
    // per call would be a real regression. OnceLock => one atomic load when off.
    static ON: OnceLock<bool> = OnceLock::new();
    if !*ON.get_or_init(|| std::env::var_os("SVTAV1_CCOSTDBG").is_some()) {
        return;
    }
    static N: AtomicUsize = AtomicUsize::new(0);
    let i = N.fetch_add(1, Ordering::Relaxed);
    if i >= 200 {
        return;
    }
    let n = (width * height).min(qcoeff.len());
    let sumabs: i64 = qcoeff[..n].iter().map(|&v| (v as i64).abs()).sum();
    let q = |k: usize| if n > k { qcoeff[k] } else { 0 };
    eprintln!(
        "CCOST i={i} plane={plane} txs={c_tx_size} txt={tx_type} eob={eob} skip={skip} dc={dc} \
         sumabs={sumabs} q0={} q1={} q2={} cost={cost}",
        q(0),
        q(1),
        q(2),
    );
}

/// C `svt_av1_cost_coeffs_txb` (rd_cost.c:355) at
/// `mds_fast_coeff_est_level = 1` (FULL middle loop), arbitrary plane /
/// tx type / contexts. `eob > 0`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn cost_coeffs_txb(
    qcoeff: &[i32],
    eob: u16,
    c_tx_size: usize,
    tx_type: usize,
    plane_type: usize,
    txb_skip_ctx: usize,
    dc_sign_ctx: usize,
    intra_dir: usize,
    rates: &MdRates,
) -> i32 {
    debug_assert!(eob > 0);
    let tx_class = cc::TX_TYPE_TO_CLASS[tx_type];
    let txs_ctx = cc::txsize_entropy_ctx(c_tx_size);
    let bwl = cc::txb_bwl(c_tx_size);
    let width = cc::txb_wide(c_tx_size);
    let height = cc::txb_high(c_tx_size);
    let scan = svtav1_entropy::scan_tables::scan(
        c_tx_size,
        svtav1_entropy::scan_tables::TX_TYPE_TO_SCAN_INDEX[tx_type] as usize,
    );
    let costs = rates.coeff.txb(txs_ctx, plane_type);
    let eob_bits = &rates.coeff.eob[cc::TXSIZE_LOG2_MINUS4[c_tx_size]][plane_type];

    let mut cost = costs.txb_skip_cost[txb_skip_ctx][0];
    let mut levels_buf = [0u8; cc::LEVELS_SCRATCH_LEN];
    if eob > 1 {
        cc::txb_init_levels(qcoeff, width, height, &mut levels_buf);
    }
    if plane_type == 0 {
        // SHIPPED-C QUIRK (svt_av1_cost_coeffs_txb, rd_cost.c:394): the
        // coeff-cost's tx-type rate keys `is_inter` on
        // `is_inter_mode(mode)` WITHOUT `|| use_intrabc` — an IntraBC
        // candidate (mode DC_PRED, fi off) therefore prices its tx type
        // on the INTRA rows at intra_dir = DC (the WRITE path and the
        // txt-search set/gates stay inter-classified). Witnessed:
        // gui_p4_q48 mi(62,32) IBC 8x8/d1 — C's V_DCT txb costs read the
        // intra-DC row (~1.9k) where the inter row is 2504 (default),
        // flipping both the per-txb TXT winners ({0,10,0,10} vs the
        // port's {0,0,0,10}) and the candidate's total coeff rate (C ycb
        // 29518 vs port 32124) -> the IBC-vs-intra arbitration.
        let cost_dir = if intra_dir == INTER_TXT_DIR {
            0
        } else {
            intra_dir
        };
        cost += rates.txt_rate(c_tx_size, cost_dir, tx_type);
    }
    cost += crate::quant::eob_cost(eob as i32, eob_bits, costs, tx_class);

    let mut coeff_contexts = vec![0i8; width * height];
    cc::get_nz_map_contexts(
        &levels_buf,
        scan,
        eob as usize,
        c_tx_size,
        tx_class,
        &mut coeff_contexts,
    );

    let lit = 512i32; // av1_cost_literal(1)
    let eob_us = eob as usize;

    let level_cost =
        |cost: &mut i32, pos: usize, v: i32, is_eob_pos: bool, is_dc: bool, levels_buf: &[u8]| {
            let level = v.unsigned_abs() as i32;
            let coeff_ctx = coeff_contexts[pos] as usize;
            if is_eob_pos {
                *cost += costs.base_eob_cost[coeff_ctx][(level.min(3) - 1) as usize];
            } else {
                *cost += costs.base_cost[coeff_ctx][level.min(3) as usize];
            }
            if v != 0 {
                if is_dc {
                    let sign = usize::from(v < 0);
                    *cost += costs.dc_sign_cost[dc_sign_ctx][sign];
                } else {
                    *cost += lit;
                }
                if level > cc::NUM_BASE_LEVELS {
                    let ctx = cc::br_ctx(levels_buf, pos, bwl, tx_class);
                    let base_range = level - 1 - cc::NUM_BASE_LEVELS;
                    if base_range < cc::COEFF_BASE_RANGE {
                        *cost += costs.lps_cost[ctx][base_range as usize];
                    } else {
                        *cost += costs.lps_cost[ctx][cc::COEFF_BASE_RANGE as usize];
                    }
                    if level >= 1 + cc::NUM_BASE_LEVELS + cc::COEFF_BASE_RANGE {
                        *cost += crate::quant::golomb_cost(level);
                    }
                }
            }
        };

    if eob_us == 1 {
        level_cost(&mut cost, 0, qcoeff[0], true, true, &levels_buf);
        #[cfg(feature = "std")]
        ccost_log(
            plane_type,
            c_tx_size,
            tx_type,
            eob,
            txb_skip_ctx,
            dc_sign_ctx,
            qcoeff,
            width,
            height,
            cost,
        );
        return cost;
    }
    // eob - 1 (base_eob context), then DC, then the full middle loop —
    // av1_cost_coeffs_txb_loop_cost_eob with fast level 1 => every
    // position is priced.
    {
        let pos = scan[eob_us - 1] as usize;
        level_cost(&mut cost, pos, qcoeff[pos], true, false, &levels_buf);
    }
    level_cost(&mut cost, 0, qcoeff[0], false, true, &levels_buf);
    for c in (1..=eob_us - 2).rev() {
        let pos = scan[c] as usize;
        let v = qcoeff[pos];
        let level = v.unsigned_abs() as i32;
        if v != 0 {
            cost += lit;
        }
        if level > cc::NUM_BASE_LEVELS {
            let ctx = cc::br_ctx(&levels_buf, pos, bwl, tx_class);
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
    }
    #[cfg(feature = "std")]
    ccost_log(
        plane_type,
        c_tx_size,
        tx_type,
        eob,
        txb_skip_ctx,
        dc_sign_ctx,
        qcoeff,
        width,
        height,
        cost,
    );
    cost
}

/// C `av1_cost_skip_txb` (rd_cost.c:213): the eob == 0 txb rate.
pub(crate) fn cost_skip_txb(
    c_tx_size: usize,
    plane_type: usize,
    txb_skip_ctx: usize,
    rates: &MdRates,
) -> i32 {
    let txs_ctx = cc::txsize_entropy_ctx(c_tx_size);
    rates.coeff.txb(txs_ctx, plane_type).txb_skip_cost[txb_skip_ctx][1]
}
