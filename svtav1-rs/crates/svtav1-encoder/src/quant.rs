//! C-exact coding-path quantization for the still/PD1 path.
//!
//! At allintra effective-M9 the coefficients that reach the bitstream are
//! produced by MD stage 3 (`perform_dct_dct_tx` / `perform_tx_partitioning`
//! -> `svt_aom_quantize_inv_quantize`, `Source/Lib/Codec/full_loop.c:1647`);
//! the encode pass never requantizes (`pic_bypass_encdec = 1` above M3,
//! `svt_aom_get_bypass_encdec_allintra` enc_mode_config.c:12037 — verified
//! with an instrumented library: zero `is_encode_pass` quantize calls).
//! That path is:
//!
//! - `rdoq_level == 0`  -> `av1_quantize_b_facade_ii` ->
//!   `svt_aom_quantize_b_c` (full_loop.c:31): zbin dead zone + round/quant/
//!   quant_shift fixed-point division.
//! - `rdoq_level > 0`   -> `svt_av1_quantize_fp_facade` ->
//!   `quantize_fp_helper_c` (full_loop.c:222) with the `_fp` round/quant
//!   tables, then the RDOQ trellis `svt_av1_optimize_b` (full_loop.c:1038)
//!   which lowers/zeroes levels minimizing `RDCOST(rdmult, rate, dist)`
//!   against the default-CDF coefficient cost tables
//!   (`svt_aom_estimate_coefficients_rate`, md_rate_estimation.c:495 —
//!   static for the whole frame at eff-M9 because `update_cdf_level = 0`,
//!   enc_mode_config.c:12047).
//!
//! The picture-level `rdoq_level` comes from `pcs->coeff_lvl`
//! (`derive_intra_coeff_level`, md_config_process.c:620) via the allintra
//! policy (enc_mode_config.c:14931): above M5 — HIGH -> 0, NORMAL -> 3,
//! else -> 2. MD stage 3 with bypassed enc-dec clears `skip_uv` and
//! `dct_dct_only` (product_coding_loop.c `md_stage_3`), so RDOQ applies to
//! chroma too; `eob_th`/`eob_fast_th` are 255 at levels 1-3 (never fire).
//! Contexts passed to the trellis are 0/0 (`rate_est_level = 0` above M8
//! -> `update_skip_ctx_dc_sign_ctx = 0`, enc_mode_config.c:8342).
//!
//! Instrumented-C captures backing this module (gradient content, eff-M9):
//!
//! | cell | pic_avg_variance | cmplx | coeff_lvl | rdoq_level | lambda |
//! |------|------------------|-------|-----------|------------|--------|
//! | g64  q20 | 5425 | 271 | HIGH(3)   | 0 | 25650   |
//! | g64  q40 | 5425 | 135 | NORMAL(2) | 3 | 248207  |
//! | g64  q55 | 5425 |  98 | NORMAL(2) | 3 | 1527856 |
//! | g128 q20 | 1483 |  74 | LOW(1)    | 2 | 25650   |

use svtav1_entropy::coeff_c;

// ---------------------------------------------------------------------------
// Quantizer table row (svt_av1_build_quantizer)
// ---------------------------------------------------------------------------

/// One qindex row of the C `Quants`/`Dequants` tables, luma, 8-bit:
/// `svt_av1_build_quantizer` (md_config_process.c:97) with all delta-q 0
/// and sharpness 0. Chroma uses the identical row because the frame
/// header signals DeltaQUDc = DeltaQUAc = 0. `[0]` = DC, `[1]` = AC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuantTable {
    pub zbin: [i32; 2],
    pub round: [i32; 2],
    pub quant: [i32; 2],
    pub quant_shift: [i32; 2],
    /// `y_round_fp`: `(64 * q) >> 7` (qrounding_factor_fp = 64).
    pub round_fp: [i32; 2],
    /// `y_quant_fp`: `(1 << 16) / q`.
    pub quant_fp: [i32; 2],
    pub dequant: [i32; 2],
}

/// C `svt_aom_invert_quant` (inv_transforms.c:3507).
fn invert_quant(d: i32) -> (i32, i32) {
    let mut t = d as u32;
    let mut l = 0i32;
    while t > 1 {
        t >>= 1;
        l += 1;
    }
    let m = 1i64 + (1i64 << (16 + l)) / d as i64;
    ((m - (1 << 16)) as i32, 1 << (16 - l))
}

/// Build the (DC, AC) quantizer row for `qindex` — the superset of
/// pd0.rs's `build_quant_entry` including the `_fp` fields the RDOQ path
/// quantizes with. Pinned against the instrumented library at qindex 220
/// (see tests).
pub fn build_quant_table(qindex: u8) -> QuantTable {
    let q = qindex as usize;
    let dc = svtav1_dsp::quant_tables::DC_QLOOKUP_8[q] as i32;
    let ac = svtav1_dsp::quant_tables::AC_QLOOKUP_8[q] as i32;
    // svt_aom_get_qzbin_factor (inv_transforms.c:3492), 8-bit.
    let qzbin_factor = if q == 0 {
        64
    } else if dc < 148 {
        84
    } else {
        80
    };
    let qrounding_factor = if q == 0 { 64 } else { 48 };
    let mut t = QuantTable {
        zbin: [0; 2],
        round: [0; 2],
        quant: [0; 2],
        quant_shift: [0; 2],
        round_fp: [0; 2],
        quant_fp: [0; 2],
        dequant: [0; 2],
    };
    for (i, quant_qtx) in [dc, ac].into_iter().enumerate() {
        let (quant, shift) = invert_quant(quant_qtx);
        t.quant[i] = quant;
        t.quant_shift[i] = shift;
        t.zbin[i] = (qzbin_factor * quant_qtx + 64) >> 7; // ROUND_POWER_OF_TWO(x, 7)
        t.round[i] = (qrounding_factor * quant_qtx) >> 7;
        t.quant_fp[i] = (1 << 16) / quant_qtx;
        t.round_fp[i] = (64 * quant_qtx) >> 7;
        t.dequant[i] = quant_qtx;
    }
    t
}

/// C `av1_get_tx_scale_tab[TX_SIZES_ALL]` (full_loop.c:22), indexed by the
/// C TxSize value.
pub const TX_SCALE_TAB: [i32; 19] = [0, 0, 0, 1, 2, 0, 0, 0, 0, 1, 1, 2, 2, 0, 0, 0, 0, 1, 1];

// ---------------------------------------------------------------------------
// Quantizers (packed-coefficient domain; scan positions index the packed
// raster like the C facades that operate on the adjusted tx size)
// ---------------------------------------------------------------------------

/// C `svt_aom_quantize_b_c` (full_loop.c:31) without quant matrices:
/// writes `qcoeff`/`dqcoeff` (zeroed first) and returns eob (scan count).
pub fn quantize_b(
    coeffs: &[i32],
    scan: &[u16],
    t: &QuantTable,
    log_scale: i32,
    qcoeff: &mut [i32],
    dqcoeff: &mut [i32],
) -> u16 {
    let n_coeffs = scan.len();
    let zbins = [
        (t.zbin[0] + ((1 << log_scale) >> 1)) >> log_scale,
        (t.zbin[1] + ((1 << log_scale) >> 1)) >> log_scale,
    ];
    qcoeff[..coeffs.len()].fill(0);
    dqcoeff[..coeffs.len()].fill(0);

    // Pre-scan pass: find the last scan position outside the zbin dead zone.
    let mut non_zero_count = n_coeffs;
    for i in (0..n_coeffs).rev() {
        let rc = scan[i] as usize;
        let coeff = coeffs[rc];
        let iz = usize::from(rc != 0);
        if coeff < zbins[iz] && coeff > -zbins[iz] {
            non_zero_count -= 1;
        } else {
            break;
        }
    }

    let mut eob: i64 = -1;
    for i in 0..non_zero_count {
        let rc = scan[i] as usize;
        let coeff = coeffs[rc];
        let iz = usize::from(rc != 0);
        let coeff_sign: i32 = if coeff < 0 { -1 } else { 0 };
        let abs_coeff = (coeff ^ coeff_sign) - coeff_sign;
        if abs_coeff >= zbins[iz] {
            let round = (t.round[iz] + ((1 << log_scale) >> 1)) >> log_scale;
            let tmp = (abs_coeff + round).clamp(i16::MIN as i32, i16::MAX as i32) as i64;
            let tmp32 = (((((tmp * t.quant[iz] as i64) >> 16) + tmp) * t.quant_shift[iz] as i64)
                >> (16 - log_scale)) as i32;
            qcoeff[rc] = (tmp32 ^ coeff_sign) - coeff_sign;
            let abs_dq = ((tmp32 as i64 * t.dequant[iz] as i64) >> log_scale) as i32;
            dqcoeff[rc] = (abs_dq ^ coeff_sign) - coeff_sign;
            if tmp32 != 0 {
                eob = i as i64;
            }
        }
    }
    (eob + 1) as u16
}

/// C `quantize_fp_helper_c` (full_loop.c:222), no-quant-matrix branch —
/// the initial quantization of the RDOQ path (`svt_av1_quantize_fp_facade`
/// routes here for every log_scale; the round/quant inputs are the `_fp`
/// table rows).
pub fn quantize_fp(
    coeffs: &[i32],
    scan: &[u16],
    t: &QuantTable,
    log_scale: i32,
    qcoeff: &mut [i32],
    dqcoeff: &mut [i32],
) -> u16 {
    let rounding = [
        (t.round_fp[0] + ((1 << log_scale) >> 1)) >> log_scale,
        (t.round_fp[1] + ((1 << log_scale) >> 1)) >> log_scale,
    ];
    qcoeff[..coeffs.len()].fill(0);
    dqcoeff[..coeffs.len()].fill(0);

    let mut eob: i64 = -1;
    for (i, &sc) in scan.iter().enumerate() {
        let rc = sc as usize;
        let thresh = t.dequant[usize::from(rc != 0)] as i64;
        let coeff = coeffs[rc];
        let coeff_sign: i32 = if coeff < 0 { -1 } else { 0 };
        let abs_coeff = ((coeff ^ coeff_sign) - coeff_sign) as i64;
        let mut tmp32 = 0i32;
        if (abs_coeff << (1 + log_scale)) >= thresh {
            let iz = usize::from(rc != 0);
            let a = (abs_coeff + rounding[iz] as i64).clamp(i16::MIN as i64, i16::MAX as i64);
            tmp32 = ((a * t.quant_fp[iz] as i64) >> (16 - log_scale)) as i32;
            if tmp32 != 0 {
                qcoeff[rc] = (tmp32 ^ coeff_sign) - coeff_sign;
                let abs_dq = ((tmp32 as i64 * t.dequant[iz] as i64) >> log_scale) as i32;
                dqcoeff[rc] = (abs_dq ^ coeff_sign) - coeff_sign;
            }
        }
        if tmp32 != 0 {
            eob = i as i64;
        }
    }
    (eob + 1) as u16
}

// ---------------------------------------------------------------------------
// Default-CDF coefficient cost tables (svt_aom_estimate_coefficients_rate)
// ---------------------------------------------------------------------------

pub const TXB_SKIP_CONTEXTS: usize = 13;
pub const SIG_COEF_CONTEXTS_EOB: usize = 4;
pub const SIG_COEF_CONTEXTS: usize = 42;
pub const EOB_COEF_CONTEXTS: usize = 9;
pub const DC_SIGN_CONTEXTS: usize = 3;
pub const LEVEL_CONTEXTS: usize = 21;
const COEFF_BASE_RANGE: usize = 12;
const NUM_BASE_LEVELS: i32 = 2;
const BR_CDF_SIZE: usize = 4;

/// C `LvMapCoeffCost` (md_rate_estimation.h:37) for one (txs_ctx, plane).
#[derive(Clone)]
pub struct TxbCosts {
    pub txb_skip_cost: [[i32; 2]; TXB_SKIP_CONTEXTS],
    pub base_eob_cost: [[i32; 3]; SIG_COEF_CONTEXTS_EOB],
    pub base_cost: [[i32; 8]; SIG_COEF_CONTEXTS],
    pub eob_extra_cost: [[i32; 2]; EOB_COEF_CONTEXTS],
    pub dc_sign_cost: [[i32; 2]; DC_SIGN_CONTEXTS],
    pub lps_cost: [[i32; 2 * (COEFF_BASE_RANGE + 1)]; LEVEL_CONTEXTS],
}

/// C `LvMapEobCost` (md_rate_estimation.h:33) for one (eob_multi_size, plane).
#[derive(Clone, Copy)]
pub struct EobCosts {
    pub eob_cost: [[i32; 11]; 2],
}

/// The complete per-frame coefficient rate tables MD prices RDOQ with:
/// `coeff_fac_bits[TX_SIZES=5][plane]` + `eob_frac_bits[7][plane]`.
pub struct CoeffCostTables {
    pub txb: alloc::vec::Vec<TxbCosts>, // [txs_ctx * 2 + plane], 10 entries
    pub eob: [[EobCosts; 2]; 7],        // [eob_multi_size][plane]
}

impl CoeffCostTables {
    #[inline]
    pub fn txb(&self, txs_ctx: usize, plane: usize) -> &TxbCosts {
        &self.txb[txs_ctx * 2 + plane]
    }
}

/// C `av1_cost_literal(n)` = `n * (1 << AV1_PROB_COST_SHIFT)` (1/512-bit).
#[inline]
const fn cost_literal(n: i32) -> i32 {
    n * 512
}

/// C `svt_aom_get_syntax_rate_from_cdf` (md_rate_estimation.c:48): per-
/// symbol costs from a stored inverse CDF (icdf convention: value 0 marks
/// the last symbol), probability floored at EC_MIN_PROB = 4.
pub(crate) fn syntax_rate_from_cdf(costs: &mut [i32], cdf: &[u16]) {
    let mut prev_cum = 0u32; // AOM_ICDF(cdf[i]) accumulates the cumulative prob
    for (i, &v) in cdf.iter().enumerate() {
        let cum = 32768 - v as u32; // AOM_ICDF
        let p15 = (cum - prev_cum).max(4);
        prev_cum = cum;
        costs[i] = svtav1_entropy::context::av1_cost_symbol(p15) as i32;
        if v == 0 {
            break;
        }
    }
}

/// C `svt_aom_estimate_coefficients_rate` (md_rate_estimation.c:495) on
/// the default-CDF frame context for `base_qindex`'s bucket — exactly the
/// tables eff-M9 MD uses all frame (update_cdf_level = 0).
pub fn build_coeff_cost_tables(base_qindex: u8) -> alloc::boxed::Box<CoeffCostTables> {
    let fc = coeff_c::CoeffFc::default_for_qindex(base_qindex);
    build_coeff_cost_tables_from_fc(&fc)
}

/// [`build_coeff_cost_tables`] over an ARBITRARY coefficient frame context
/// — the per-SB chained contexts of C's `update_cdf_level = 2` presets
/// (M4..M6: `svt_aom_estimate_coefficients_rate` over
/// `pcs->ec_ctx_array[sb]`, enc_dec_process.c:3039-3042).
pub fn build_coeff_cost_tables_from_fc(
    fc: &coeff_c::CoeffFc,
) -> alloc::boxed::Box<CoeffCostTables> {
    let mut tables = alloc::boxed::Box::new(CoeffCostTables {
        txb: alloc::vec![
            TxbCosts {
                txb_skip_cost: [[0; 2]; TXB_SKIP_CONTEXTS],
                base_eob_cost: [[0; 3]; SIG_COEF_CONTEXTS_EOB],
                base_cost: [[0; 8]; SIG_COEF_CONTEXTS],
                eob_extra_cost: [[0; 2]; EOB_COEF_CONTEXTS],
                dc_sign_cost: [[0; 2]; DC_SIGN_CONTEXTS],
                lps_cost: [[0; 2 * (COEFF_BASE_RANGE + 1)]; LEVEL_CONTEXTS],
            };
            10
        ],
        eob: [[EobCosts {
            eob_cost: [[0; 11]; 2],
        }; 2]; 7],
    });

    for eob_multi_size in 0..7 {
        for plane in 0..2 {
            let e = &mut tables.eob[eob_multi_size][plane];
            for ctx in 0..2 {
                let idx = plane * 2 + ctx;
                match eob_multi_size {
                    0 => syntax_rate_from_cdf(&mut e.eob_cost[ctx], &fc.eob_flag_cdf16[idx]),
                    1 => syntax_rate_from_cdf(&mut e.eob_cost[ctx], &fc.eob_flag_cdf32[idx]),
                    2 => syntax_rate_from_cdf(&mut e.eob_cost[ctx], &fc.eob_flag_cdf64[idx]),
                    3 => syntax_rate_from_cdf(&mut e.eob_cost[ctx], &fc.eob_flag_cdf128[idx]),
                    4 => syntax_rate_from_cdf(&mut e.eob_cost[ctx], &fc.eob_flag_cdf256[idx]),
                    5 => syntax_rate_from_cdf(&mut e.eob_cost[ctx], &fc.eob_flag_cdf512[idx]),
                    _ => syntax_rate_from_cdf(&mut e.eob_cost[ctx], &fc.eob_flag_cdf1024[idx]),
                }
            }
        }
    }

    for txs_ctx in 0..5 {
        for plane in 0..2 {
            let p = &mut tables.txb[txs_ctx * 2 + plane];
            for ctx in 0..TXB_SKIP_CONTEXTS {
                // txb_skip_cdf is [txs_ctx][ctx] (no plane dim in C).
                syntax_rate_from_cdf(
                    &mut p.txb_skip_cost[ctx],
                    &fc.txb_skip_cdf[txs_ctx * 13 + ctx],
                );
            }
            for ctx in 0..SIG_COEF_CONTEXTS_EOB {
                syntax_rate_from_cdf(
                    &mut p.base_eob_cost[ctx],
                    &fc.coeff_base_eob_cdf[(txs_ctx * 2 + plane) * 4 + ctx],
                );
            }
            for ctx in 0..SIG_COEF_CONTEXTS {
                syntax_rate_from_cdf(
                    &mut p.base_cost[ctx],
                    &fc.coeff_base_cdf[(txs_ctx * 2 + plane) * 42 + ctx],
                );
            }
            for ctx in 0..SIG_COEF_CONTEXTS {
                p.base_cost[ctx][4] = 0;
                p.base_cost[ctx][5] = p.base_cost[ctx][1] + cost_literal(1) - p.base_cost[ctx][0];
                p.base_cost[ctx][6] = p.base_cost[ctx][2] - p.base_cost[ctx][1];
                p.base_cost[ctx][7] = p.base_cost[ctx][3] - p.base_cost[ctx][2];
            }
            for ctx in 0..EOB_COEF_CONTEXTS {
                syntax_rate_from_cdf(
                    &mut p.eob_extra_cost[ctx],
                    &fc.eob_extra_cdf[(txs_ctx * 2 + plane) * 9 + ctx],
                );
            }
            for ctx in 0..DC_SIGN_CONTEXTS {
                syntax_rate_from_cdf(&mut p.dc_sign_cost[ctx], &fc.dc_sign_cdf[plane * 3 + ctx]);
            }
            for ctx in 0..LEVEL_CONTEXTS {
                // coeff_br_cdf is indexed with AOMMIN(txs_ctx, TX_32X32=3).
                let br_txs = txs_ctx.min(3);
                let mut br_rate = [0i32; BR_CDF_SIZE];
                syntax_rate_from_cdf(
                    &mut br_rate,
                    &fc.coeff_br_cdf[(br_txs * 2 + plane) * 21 + ctx],
                );
                let mut prev_cost = 0i32;
                let mut i = 0usize;
                while i < COEFF_BASE_RANGE {
                    for j in 0..BR_CDF_SIZE - 1 {
                        p.lps_cost[ctx][i + j] = prev_cost + br_rate[j];
                    }
                    prev_cost += br_rate[BR_CDF_SIZE - 1];
                    i += BR_CDF_SIZE - 1;
                }
                p.lps_cost[ctx][i] = prev_cost;
            }
            for ctx in 0..LEVEL_CONTEXTS {
                p.lps_cost[ctx][COEFF_BASE_RANGE + 1] = p.lps_cost[ctx][0];
                for i in 1..=COEFF_BASE_RANGE {
                    p.lps_cost[ctx][i + COEFF_BASE_RANGE + 1] =
                        p.lps_cost[ctx][i] - p.lps_cost[ctx][i - 1];
                }
            }
        }
    }
    tables
}

// ---------------------------------------------------------------------------
// RDOQ trellis (svt_av1_optimize_b)
// ---------------------------------------------------------------------------

/// C `RDCOST(RM, R, D)` (rd_cost.h:36): `ROUND_POWER_OF_TWO(R*RM, 9) +
/// (D << RDDIV_BITS=7)`.
#[inline]
fn rdcost(rdmult: i64, rate: i64, dist: i64) -> i64 {
    ((rate * rdmult + 256) >> 9) + (dist << 7)
}

#[inline]
fn get_coeff_dist(tcoeff: i32, dqcoeff: i32, shift: i32) -> i64 {
    let d = (tcoeff as i64 - dqcoeff as i64) * (1i64 << shift);
    d * d
}

/// C `get_golomb_cost` (full_loop.c:613).
#[inline]
pub(crate) fn golomb_cost(abs_qc: i32) -> i32 {
    if abs_qc >= 1 + NUM_BASE_LEVELS + COEFF_BASE_RANGE as i32 {
        let r = abs_qc - COEFF_BASE_RANGE as i32 - NUM_BASE_LEVELS;
        let length = (31 - (r as u32).leading_zeros()) as i32 + 1; // get_msb + 1
        cost_literal(2 * length - 1)
    } else {
        0
    }
}

/// C `get_br_cost` (full_loop.c:622).
#[inline]
fn br_cost(level: i32, coeff_lps: &[i32]) -> i32 {
    let base_range = (level - 1 - NUM_BASE_LEVELS).min(COEFF_BASE_RANGE as i32);
    coeff_lps[base_range as usize] + golomb_cost(level)
}

const GOLOMB_BITS_COST: [i32; 32] = [
    0,
    512,
    512 * 3,
    512 * 3,
    512 * 5,
    512 * 5,
    512 * 5,
    512 * 5,
    512 * 7,
    512 * 7,
    512 * 7,
    512 * 7,
    512 * 7,
    512 * 7,
    512 * 7,
    512 * 7,
    512 * 9,
    512 * 9,
    512 * 9,
    512 * 9,
    512 * 9,
    512 * 9,
    512 * 9,
    512 * 9,
    512 * 9,
    512 * 9,
    512 * 9,
    512 * 9,
    512 * 9,
    512 * 9,
    512 * 9,
    512 * 9,
];
const GOLOMB_COST_DIFF: [i32; 32] = [
    0,
    512,
    512 * 2,
    0,
    512 * 2,
    0,
    0,
    0,
    512 * 2,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
    512 * 2,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
];

/// C `get_br_cost_with_diff` (full_loop.c:675).
#[inline]
fn br_cost_with_diff(level: i32, coeff_lps: &[i32], diff: &mut i32) -> i32 {
    let base_range = (level - 1 - NUM_BASE_LEVELS).min(COEFF_BASE_RANGE as i32);
    let mut golomb_bits = 0i32;
    if level <= COEFF_BASE_RANGE as i32 + 1 + NUM_BASE_LEVELS {
        *diff += coeff_lps[(base_range + COEFF_BASE_RANGE as i32 + 1) as usize];
    }
    if level >= COEFF_BASE_RANGE as i32 + 1 + NUM_BASE_LEVELS {
        let r = level - COEFF_BASE_RANGE as i32 - NUM_BASE_LEVELS;
        if r < 32 {
            golomb_bits = GOLOMB_BITS_COST[r as usize];
            *diff += GOLOMB_COST_DIFF[r as usize];
        } else {
            golomb_bits = golomb_cost(level);
            *diff += if (r & (r - 1)) == 0 { 1024 } else { 0 };
        }
    }
    coeff_lps[base_range as usize] + golomb_bits
}

/// C `get_coeff_cost_general` (full_loop.c:627). `levels_buf` is the full
/// padded buffer; `ci` a packed raster position.
#[allow(clippy::too_many_arguments)]
fn coeff_cost_general(
    is_last: bool,
    ci: usize,
    abs_qc: i32,
    sign: usize,
    coeff_ctx: usize,
    dc_sign_ctx: usize,
    txb_costs: &TxbCosts,
    bwl: usize,
    tx_class: usize,
    levels_buf: &[u8],
) -> i32 {
    let mut cost = if is_last {
        txb_costs.base_eob_cost[coeff_ctx][(abs_qc.min(3) - 1) as usize]
    } else {
        txb_costs.base_cost[coeff_ctx][abs_qc.min(3) as usize]
    };
    if abs_qc != 0 {
        if ci == 0 {
            cost += txb_costs.dc_sign_cost[dc_sign_ctx][sign];
        } else {
            cost += cost_literal(1);
        }
        if abs_qc > NUM_BASE_LEVELS {
            let br_ctx = if is_last {
                coeff_c::br_ctx_eob(ci, bwl, tx_class)
            } else {
                coeff_c::br_ctx(levels_buf, ci, bwl, tx_class)
            };
            cost += br_cost(abs_qc, &txb_costs.lps_cost[br_ctx]);
        }
    }
    cost
}

/// C `get_coeff_cost_eob` (full_loop.c:722).
#[allow(clippy::too_many_arguments)]
fn coeff_cost_eob(
    ci: usize,
    abs_qc: i32,
    sign: usize,
    coeff_ctx: usize,
    dc_sign_ctx: usize,
    txb_costs: &TxbCosts,
    bwl: usize,
    tx_class: usize,
) -> i32 {
    let mut cost = txb_costs.base_eob_cost[coeff_ctx][(abs_qc.min(3) - 1) as usize];
    if abs_qc != 0 {
        if ci == 0 {
            cost += txb_costs.dc_sign_cost[dc_sign_ctx][sign];
        } else {
            cost += cost_literal(1);
        }
        if abs_qc > NUM_BASE_LEVELS {
            let br_ctx = coeff_c::br_ctx_eob(ci, bwl, tx_class);
            cost += br_cost(abs_qc, &txb_costs.lps_cost[br_ctx]);
        }
    }
    cost
}

/// C `get_two_coeff_cost_simple` (full_loop.c:696).
#[allow(clippy::too_many_arguments)]
fn two_coeff_cost_simple(
    ci: usize,
    abs_qc: i32,
    coeff_ctx: usize,
    txb_costs: &TxbCosts,
    bwl: usize,
    tx_class: usize,
    levels_buf: &[u8],
    cost_low: &mut i32,
) -> i32 {
    let mut cost = txb_costs.base_cost[coeff_ctx][abs_qc.min(3) as usize];
    let mut diff = 0i32;
    if abs_qc <= 3 {
        diff = txb_costs.base_cost[coeff_ctx][(abs_qc + 4) as usize];
    }
    if abs_qc != 0 {
        cost += cost_literal(1);
        if abs_qc > NUM_BASE_LEVELS {
            let br_ctx = coeff_c::br_ctx(levels_buf, ci, bwl, tx_class);
            let mut brcost_diff = 0i32;
            cost += br_cost_with_diff(abs_qc, &txb_costs.lps_cost[br_ctx], &mut brcost_diff);
            diff += brcost_diff;
        }
    }
    *cost_low = cost - diff;
    cost
}

/// C `get_eob_cost` (rd_cost.c:198).
pub fn eob_cost(eob: i32, eob_costs: &EobCosts, txb_costs: &TxbCosts, tx_class: usize) -> i32 {
    let (eob_pt, eob_extra) = coeff_c::eob_pos_token(eob);
    let eob_multi_ctx = usize::from(tx_class != coeff_c::TX_CLASS_2D);
    let mut cost = eob_costs.eob_cost[eob_multi_ctx][eob_pt - 1];
    if eob_pt > 2 {
        let cnt = eob_pt - 3;
        let bit = ((eob_extra >> cnt) & 1) as usize;
        cost += txb_costs.eob_extra_cost[cnt][bit];
        cost += cost_literal(cnt as i32);
    }
    cost
}

/// C `get_qc_dqc_low` (full_loop.c:659). `sign` is 1 for negative.
#[inline]
fn qc_dqc_low(abs_qc: i32, sign: i32, dqv: i32, shift: i32) -> (i32, i32) {
    let abs_qc_low = abs_qc - 1;
    let qc_low = (-sign ^ abs_qc_low) + sign;
    let abs_dqc_low = ((abs_qc_low as i64 * dqv as i64) >> shift) as i32;
    let dqc_low = (-sign ^ abs_dqc_low) + sign;
    (qc_low, dqc_low)
}

#[inline]
fn levels_idx(ci: usize, bwl: usize) -> usize {
    coeff_c::levels_origin(1 << bwl) + coeff_c::padded_idx(ci, bwl)
}

/// Everything `svt_av1_optimize_b` needs beyond the coefficient buffers.
pub struct OptimizeCtx<'a> {
    pub txb_costs: &'a TxbCosts,
    pub eob_costs: &'a EobCosts,
    /// `RDCOST` multiplier: `((lambda * plane_rd_mult) + 2) >> 2` — see
    /// [`rdoq_rdmult`].
    pub rdmult: i64,
    /// C TxSize index (for bwl/height/entropy ctx of the ADJUSTED size).
    pub tx_size: usize,
    pub tx_class: usize,
    pub txb_skip_ctx: usize,
    pub dc_sign_ctx: usize,
    /// `rdoq_ctrls.cut_off_num / denum` (0 num = full RDOQ).
    pub cut_off_num: u32,
    pub cut_off_denum: u32,
}

/// C rdmult derivation inside `svt_av1_optimize_b` (full_loop.c:1074) for
/// the allintra/still path: sharpness 0 -> rweight 100, rshift 2;
/// `plane_rd_mult[allintra=1][is_inter=0][plane_type]` = 17 luma, 13
/// chroma (both `TUNE_CHROMA_SSIM` variants agree on that row).
pub fn rdoq_rdmult(lambda: u32, plane_type: usize) -> i64 {
    let prm: i64 = if plane_type == 0 { 17 } else { 13 };
    ((lambda as i64 * prm * 100) / 100 + 2) >> 2
}

/// C `update_coeff_general` (full_loop.c:851).
#[allow(clippy::too_many_arguments)]
fn update_coeff_general(
    accu_rate: &mut i32,
    accu_dist: &mut i64,
    si: usize,
    eob: u16,
    o: &OptimizeCtx,
    bwl: usize,
    height: usize,
    dequant: &[i32; 2],
    shift: i32,
    scan: &[u16],
    tcoeff: &[i32],
    qcoeff: &mut [i32],
    dqcoeff: &mut [i32],
    levels_buf: &mut [u8],
) {
    let ci = scan[si] as usize;
    let dqv = dequant[usize::from(ci != 0)];
    let qc = qcoeff[ci];
    let is_last = si == (eob as usize - 1);
    let coeff_ctx = coeff_c::lower_levels_ctx_general(
        levels_buf, ci, bwl, height, si, is_last, o.tx_size, o.tx_class,
    );
    if qc == 0 {
        *accu_rate += o.txb_costs.base_cost[coeff_ctx][0];
    } else {
        let sign = usize::from(qc < 0);
        let abs_qc = qc.abs();
        let tqc = tcoeff[ci];
        let dqc = dqcoeff[ci];
        let dist = get_coeff_dist(tqc, dqc, shift);
        let dist0 = get_coeff_dist(tqc, 0, shift);
        let rate = coeff_cost_general(
            is_last,
            ci,
            abs_qc,
            sign,
            coeff_ctx,
            o.dc_sign_ctx,
            o.txb_costs,
            bwl,
            o.tx_class,
            levels_buf,
        );
        let rd = rdcost(o.rdmult, rate as i64, dist);

        let (qc_low, dqc_low, abs_qc_low, dist_low, rate_low);
        if abs_qc == 1 {
            abs_qc_low = 0;
            qc_low = 0;
            dqc_low = 0;
            dist_low = dist0;
            rate_low = o.txb_costs.base_cost[coeff_ctx][0];
        } else {
            let (q, d) = qc_dqc_low(abs_qc, if sign == 1 { 1 } else { 0 }, dqv, shift);
            qc_low = q;
            dqc_low = d;
            abs_qc_low = abs_qc - 1;
            dist_low = get_coeff_dist(tqc, dqc_low, shift);
            rate_low = coeff_cost_general(
                is_last,
                ci,
                abs_qc_low,
                sign,
                coeff_ctx,
                o.dc_sign_ctx,
                o.txb_costs,
                bwl,
                o.tx_class,
                levels_buf,
            );
        }

        let rd_low = rdcost(o.rdmult, rate_low as i64, dist_low);
        if rd_low < rd {
            qcoeff[ci] = qc_low;
            dqcoeff[ci] = dqc_low;
            levels_buf[levels_idx(ci, bwl)] = abs_qc_low.min(i8::MAX as i32) as u8;
            *accu_rate += rate_low;
            *accu_dist += dist_low - dist0;
        } else {
            *accu_rate += rate;
            *accu_dist += dist - dist0;
        }
    }
}

/// C `update_coeff_simple` (full_loop.c:904).
#[allow(clippy::too_many_arguments)]
fn update_coeff_simple(
    accu_rate: &mut i32,
    si: usize,
    o: &OptimizeCtx,
    bwl: usize,
    dequant: &[i32; 2],
    shift: i32,
    scan: &[u16],
    tcoeff: &[i32],
    qcoeff: &mut [i32],
    dqcoeff: &mut [i32],
    levels_buf: &mut [u8],
) {
    debug_assert!(si > 0);
    let ci = scan[si] as usize;
    let dqv = dequant[usize::from(ci != 0)];
    let qc = qcoeff[ci];
    let coeff_ctx =
        coeff_c::lower_levels_ctx_general(levels_buf, ci, bwl, 0, si, false, o.tx_size, o.tx_class);
    if qc == 0 {
        *accu_rate += o.txb_costs.base_cost[coeff_ctx][0];
    } else {
        let abs_qc = qc.abs();
        let abs_tqc = tcoeff[ci].abs();
        let abs_dqc = dqcoeff[ci].abs();
        let mut rate_low = 0i32;
        let rate = two_coeff_cost_simple(
            ci,
            abs_qc,
            coeff_ctx,
            o.txb_costs,
            bwl,
            o.tx_class,
            levels_buf,
            &mut rate_low,
        );
        if abs_dqc < abs_tqc {
            *accu_rate += rate;
            return;
        }

        let dist = get_coeff_dist(abs_tqc, abs_dqc, shift);
        let rd = rdcost(o.rdmult, rate as i64, dist);

        let abs_qc_low = abs_qc - 1;
        let abs_dqc_low = ((abs_qc_low as i64 * dqv as i64) >> shift) as i32;
        let dist_low = get_coeff_dist(abs_tqc, abs_dqc_low, shift);
        let rd_low = rdcost(o.rdmult, rate_low as i64, dist_low);

        if rd_low < rd {
            let sign: i32 = if qc < 0 { 1 } else { 0 };
            qcoeff[ci] = (-sign ^ abs_qc_low) + sign;
            dqcoeff[ci] = (-sign ^ abs_dqc_low) + sign;
            levels_buf[levels_idx(ci, bwl)] = abs_qc_low.min(i8::MAX as i32) as u8;
            *accu_rate += rate_low;
        } else {
            *accu_rate += rate;
        }
    }
}

/// C `update_coeff_eob` (full_loop.c:749).
#[allow(clippy::too_many_arguments)]
fn update_coeff_eob(
    accu_rate: &mut i32,
    accu_dist: &mut i64,
    eob: &mut u16,
    nz_num: &mut usize,
    nz_ci: &mut [usize; 5],
    si: usize,
    o: &OptimizeCtx,
    bwl: usize,
    height: usize,
    dequant: &[i32; 2],
    shift: i32,
    scan: &[u16],
    tcoeff: &[i32],
    qcoeff: &mut [i32],
    dqcoeff: &mut [i32],
    levels_buf: &mut [u8],
) {
    debug_assert!(si != *eob as usize - 1);
    let ci = scan[si] as usize;
    let dqv = dequant[usize::from(ci != 0)];
    let qc = qcoeff[ci];
    let coeff_ctx =
        coeff_c::lower_levels_ctx_general(levels_buf, ci, bwl, 0, si, false, o.tx_size, o.tx_class);
    if qc == 0 {
        *accu_rate += o.txb_costs.base_cost[coeff_ctx][0];
    } else {
        let mut lower_level = false;
        let abs_qc = qc.abs();
        let tqc = tcoeff[ci];
        let dqc = dqcoeff[ci];
        let sign = usize::from(qc < 0);
        let dist0 = get_coeff_dist(tqc, 0, shift);
        let mut dist = get_coeff_dist(tqc, dqc, shift) - dist0;
        let mut rate = coeff_cost_general(
            false,
            ci,
            abs_qc,
            sign,
            coeff_ctx,
            o.dc_sign_ctx,
            o.txb_costs,
            bwl,
            o.tx_class,
            levels_buf,
        );
        let rd = rdcost(o.rdmult, (*accu_rate + rate) as i64, *accu_dist + dist);

        let (qc_low, dqc_low, abs_qc_low, dist_low, rate_low, rd_low);
        if abs_qc == 1 {
            abs_qc_low = 0;
            qc_low = 0;
            dqc_low = 0;
            dist_low = 0;
            rate_low = o.txb_costs.base_cost[coeff_ctx][0];
            rd_low = rdcost(o.rdmult, (*accu_rate + rate_low) as i64, *accu_dist);
        } else {
            let (q, d) = qc_dqc_low(abs_qc, if sign == 1 { 1 } else { 0 }, dqv, shift);
            qc_low = q;
            dqc_low = d;
            abs_qc_low = abs_qc - 1;
            dist_low = get_coeff_dist(tqc, dqc_low, shift) - dist0;
            rate_low = coeff_cost_general(
                false,
                ci,
                abs_qc_low,
                sign,
                coeff_ctx,
                o.dc_sign_ctx,
                o.txb_costs,
                bwl,
                o.tx_class,
                levels_buf,
            );
            rd_low = rdcost(
                o.rdmult,
                (*accu_rate + rate_low) as i64,
                *accu_dist + dist_low,
            );
        }

        let mut lower_level_new_eob = false;
        let new_eob = si + 1;
        let coeff_ctx_new_eob = coeff_c::lower_levels_ctx_general(
            levels_buf, ci, bwl, height, si, true, o.tx_size, o.tx_class,
        );
        let new_eob_cost = eob_cost(new_eob as i32, o.eob_costs, o.txb_costs, o.tx_class);
        let mut rate_coeff_eob = new_eob_cost
            + coeff_cost_eob(
                ci,
                abs_qc,
                sign,
                coeff_ctx_new_eob,
                o.dc_sign_ctx,
                o.txb_costs,
                bwl,
                o.tx_class,
            );
        let mut dist_new_eob = dist;
        let mut rd_new_eob = rdcost(o.rdmult, rate_coeff_eob as i64, dist_new_eob);

        if abs_qc_low > 0 {
            let rate_coeff_eob_low = new_eob_cost
                + coeff_cost_eob(
                    ci,
                    abs_qc_low,
                    sign,
                    coeff_ctx_new_eob,
                    o.dc_sign_ctx,
                    o.txb_costs,
                    bwl,
                    o.tx_class,
                );
            let dist_new_eob_low = dist_low;
            let rd_new_eob_low = rdcost(o.rdmult, rate_coeff_eob_low as i64, dist_new_eob_low);
            if rd_new_eob_low < rd_new_eob {
                lower_level_new_eob = true;
                rd_new_eob = rd_new_eob_low;
                rate_coeff_eob = rate_coeff_eob_low;
                dist_new_eob = dist_new_eob_low;
            }
        }

        if rd_low < rd {
            lower_level = true;
            rate = rate_low;
            dist = dist_low;
        }

        // sharpness == 0 on this path.
        if rd_new_eob < if rd_low < rd { rd_low } else { rd } {
            for &last_ci in nz_ci.iter().take(*nz_num) {
                levels_buf[levels_idx(last_ci, bwl)] = 0;
                qcoeff[last_ci] = 0;
                dqcoeff[last_ci] = 0;
            }
            *eob = new_eob as u16;
            *nz_num = 0;
            *accu_rate = rate_coeff_eob;
            *accu_dist = dist_new_eob;
            lower_level = lower_level_new_eob;
        } else {
            *accu_rate += rate;
            *accu_dist += dist;
        }

        if lower_level {
            qcoeff[ci] = qc_low;
            dqcoeff[ci] = dqc_low;
            levels_buf[levels_idx(ci, bwl)] = abs_qc_low.min(i8::MAX as i32) as u8;
        }
        if qcoeff[ci] != 0 {
            nz_ci[*nz_num] = ci;
            *nz_num += 1;
        }
    }
}

/// C `update_skip` (full_loop.c:951), sharpness == 0.
#[allow(clippy::too_many_arguments)]
fn update_skip(
    accu_rate: &mut i32,
    accu_dist: i64,
    eob: &mut u16,
    nz_num: usize,
    nz_ci: &[usize; 5],
    rdmult: i64,
    skip_cost: i32,
    non_skip_cost: i32,
    qcoeff: &mut [i32],
    dqcoeff: &mut [i32],
) {
    let rd = rdcost(rdmult, (*accu_rate + non_skip_cost) as i64, accu_dist);
    let rd_new_eob = rdcost(rdmult, skip_cost as i64, 0);
    if rd_new_eob < rd {
        for &ci in nz_ci.iter().take(nz_num) {
            qcoeff[ci] = 0;
            dqcoeff[ci] = 0;
        }
        *accu_rate = 0;
        *eob = 0;
    }
}

/// C `svt_av1_optimize_b` (full_loop.c:1038) for the still/allintra path:
/// `sharpness = 0` (config sharpness 0 / no delta-q), `is_inter = false`.
/// Operates on the PACKED (adjusted-size) coefficient buffers; `tcoeffs`
/// are the pre-quantization transform coefficients, `qcoeff`/`dqcoeff` the
/// `quantize_fp` outputs, updated in place. `eob` must be > 0.
#[allow(clippy::too_many_arguments)]
pub fn optimize_b(
    tcoeffs: &[i32],
    qcoeff: &mut [i32],
    dqcoeff: &mut [i32],
    eob: &mut u16,
    scan: &[u16],
    t: &QuantTable,
    o: &OptimizeCtx,
) {
    let shift = TX_SCALE_TAB[o.tx_size];
    let bwl = coeff_c::txb_bwl(o.tx_size);
    let width = coeff_c::txb_wide(o.tx_size);
    let height = coeff_c::txb_high(o.tx_size);
    let non_skip_cost = o.txb_costs.txb_skip_cost[o.txb_skip_ctx][0];
    let skip_cost = o.txb_costs.txb_skip_cost[o.txb_skip_ctx][1];
    let eob_cost_init = eob_cost(*eob as i32, o.eob_costs, o.txb_costs, o.tx_class);

    let mut levels_buf = [0u8; coeff_c::TX_PAD_2D];
    if *eob > 1 {
        coeff_c::txb_init_levels(qcoeff, width, height, &mut levels_buf);
    }

    let mut accu_rate = eob_cost_init;
    let mut accu_dist = 0i64;
    let mut si = *eob as i32 - 1;
    let ci = scan[si as usize] as usize;
    let qc = qcoeff[ci];
    let abs_qc = qc.abs();
    let sign = usize::from(qc < 0);
    const MAX_NZ_NUM: usize = 4;
    let mut nz_num = 1usize;
    let mut nz_ci = [ci, 0, 0, 0, 0];
    if abs_qc >= 2 {
        update_coeff_general(
            &mut accu_rate,
            &mut accu_dist,
            si as usize,
            *eob,
            o,
            bwl,
            height,
            &t.dequant,
            shift,
            scan,
            tcoeffs,
            qcoeff,
            dqcoeff,
            &mut levels_buf,
        );
        si -= 1;
    } else {
        debug_assert_eq!(abs_qc, 1);
        let coeff_ctx = coeff_c::lower_levels_ctx_general(
            &levels_buf,
            ci,
            bwl,
            height,
            si as usize,
            true,
            o.tx_size,
            o.tx_class,
        );
        accu_rate += coeff_cost_eob(
            ci,
            abs_qc,
            sign,
            coeff_ctx,
            o.dc_sign_ctx,
            o.txb_costs,
            bwl,
            o.tx_class,
        );
        let tqc = tcoeffs[ci];
        let dqc = dqcoeff[ci];
        let dist = get_coeff_dist(tqc, dqc, shift);
        let dist0 = get_coeff_dist(tqc, 0, shift);
        accu_dist += dist - dist0;
        si -= 1;
    }

    while si >= 0 && nz_num <= MAX_NZ_NUM {
        update_coeff_eob(
            &mut accu_rate,
            &mut accu_dist,
            eob,
            &mut nz_num,
            &mut nz_ci,
            si as usize,
            o,
            bwl,
            height,
            &t.dequant,
            shift,
            scan,
            tcoeffs,
            qcoeff,
            dqcoeff,
            &mut levels_buf,
        );
        si -= 1;
    }

    if si == -1 && nz_num <= MAX_NZ_NUM {
        update_skip(
            &mut accu_rate,
            accu_dist,
            eob,
            nz_num,
            &nz_ci,
            o.rdmult,
            skip_cost,
            non_skip_cost,
            qcoeff,
            dqcoeff,
        );
    }

    let mut si_end = 1i32; // default: full RDOQ
    if o.cut_off_num != 0 {
        let cut_off_coeff = (((width * height) >> 7) as u32)
            .max((*eob as u32 * o.cut_off_num) / o.cut_off_denum)
            as i32;
        si_end = (*eob as i32 - cut_off_coeff).max(1);
    }
    while si >= si_end {
        update_coeff_simple(
            &mut accu_rate,
            si as usize,
            o,
            bwl,
            &t.dequant,
            shift,
            scan,
            tcoeffs,
            qcoeff,
            dqcoeff,
            &mut levels_buf,
        );
        si -= 1;
    }

    // DC position.
    if si == 0 {
        let mut dummy_dist = 0i64;
        update_coeff_general(
            &mut accu_rate,
            &mut dummy_dist,
            0,
            *eob,
            o,
            bwl,
            height,
            &t.dequant,
            shift,
            scan,
            tcoeffs,
            qcoeff,
            dqcoeff,
            &mut levels_buf,
        );
    }
}

// ---------------------------------------------------------------------------
// Picture-level policy: coeff_lvl -> rdoq_level (allintra)
// ---------------------------------------------------------------------------

/// C `InputCoeffLvl` (VLOW=0, LOW=1, NORMAL=2, HIGH=3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoeffLvl {
    VLow,
    Low,
    Normal,
    High,
}

/// C `derive_intra_coeff_level` (md_config_process.c:620): complexity =
/// `pic_avg_variance / max(1, cli_qp)` against the intra thresholds
/// {25, 50, 150} scaled by input resolution (x1.7 under 240p — every
/// 64-aligned still we encode today; x1.3 to 480p, x1.2 to 720p per
/// `svt_aom_derive_input_resolution` sequence_control_set.c:120 with
/// pixel-count breaks 0x28500/0x4CE00/0xA1400/0x16DA00).
pub fn derive_intra_coeff_level(
    pic_avg_variance: u16,
    cli_qp: u32,
    w: usize,
    h: usize,
) -> CoeffLvl {
    let pixels = w * h;
    let (mut vlow, mut low, mut high) = (25.0f64, 50.0f64, 150.0f64);
    if pixels < 0x28500 {
        vlow *= 1.7;
        low *= 1.7;
        high *= 1.7;
    } else if pixels < 0xA1400 {
        vlow *= 1.3;
        low *= 1.3;
        high *= 1.3;
    } else if pixels < 0x16DA00 {
        vlow *= 1.2;
        low *= 1.2;
        high *= 1.2;
    }
    let (vlow, low, high) = (vlow as u64, low as u64, high as u64);
    let cmplx = pic_avg_variance as u64 / 1.max(cli_qp) as u64;
    if cmplx < vlow {
        CoeffLvl::VLow
    } else if cmplx < low {
        CoeffLvl::Low
    } else if cmplx > high {
        CoeffLvl::High
    } else {
        CoeffLvl::Normal
    }
}

/// C allintra RDOQ policy (enc_mode_config.c:14931), `OPT_APPROX_COEFF_RATE`
/// branch: presets <= M5 always level 1; above, by coeff_lvl.
pub fn rdoq_level_allintra(eff_enc_mode: u8, coeff_lvl: CoeffLvl) -> u8 {
    if eff_enc_mode <= 5 {
        1
    } else {
        match coeff_lvl {
            CoeffLvl::High => 0,
            CoeffLvl::Normal => 3,
            _ => 2,
        }
    }
}

/// `set_rdoq_controls` (enc_mode_config.c:5136) — the `cut_off_num /
/// cut_off_denum` pair per level (0 num = full RDOQ; levels 4/5 also set
/// eob thresholds, which never arise on the allintra path this module
/// serves). Levels: 1 -> full, 2 -> 80/100, 3..=5 -> 60/100.
pub fn rdoq_cutoffs(rdoq_level: u8) -> (u32, u32) {
    match rdoq_level {
        1 => (0, 0),
        2 => (80, 100),
        _ => (60, 100),
    }
}

/// Frame-level C-exact coding-quantizer configuration for the still path.
pub struct CodingQuantCfg {
    /// 0 = quantize_b (no RDOQ); >= 1 = quantize_fp + optimize_b.
    pub rdoq_level: u8,
    /// `full_lambda_md[EB_8_BIT_MD]` (the KF chain — pd0's
    /// `kf_full_lambda_8bit` at the frame qindex).
    pub lambda: u32,
    /// Default-CDF coefficient cost tables for the frame's qindex bucket.
    pub costs: alloc::boxed::Box<CoeffCostTables>,
}

impl core::fmt::Debug for CodingQuantCfg {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CodingQuantCfg")
            .field("rdoq_level", &self.rdoq_level)
            .field("lambda", &self.lambda)
            .finish_non_exhaustive()
    }
}

impl CodingQuantCfg {
    pub fn new(rdoq_level: u8, lambda: u32, base_qindex: u8) -> Self {
        Self {
            rdoq_level,
            lambda,
            costs: build_coeff_cost_tables(base_qindex),
        }
    }
}

/// Quantize one packed transform block exactly like C's
/// `svt_aom_quantize_inv_quantize` on the MDS3/still path (contexts 0/0):
/// `quantize_b` when `rdoq_level == 0`, else `quantize_fp` + `optimize_b`
/// (the level-1..3 eob thresholds are 255 and never fire; the eob-percent
/// check uses the REAL tx dims per full_loop.c:1810-1815).
///
/// `real_pels` = unadjusted `tx_size_wide * tx_size_high`.
#[allow(clippy::too_many_arguments)]
pub fn quantize_inv_quantize_still(
    cfg: &CodingQuantCfg,
    tcoeffs: &[i32],
    qcoeff: &mut [i32],
    dqcoeff: &mut [i32],
    scan: &[u16],
    qindex: u8,
    c_tx_size: usize,
    tx_class: usize,
    plane_type: usize,
    real_pels: u32,
) -> u16 {
    let t = build_quant_table(qindex);
    let log_scale = TX_SCALE_TAB[c_tx_size];
    if cfg.rdoq_level == 0 {
        return quantize_b(tcoeffs, scan, &t, log_scale, qcoeff, dqcoeff);
    }
    let mut eob = quantize_fp(tcoeffs, scan, &t, log_scale, qcoeff, dqcoeff);
    if eob != 0 {
        // eob_th / eob_fast_th are 255 at rdoq levels 1..3: eob_perc
        // (<= 100) can never reach them, so no quantize_b fallback and no
        // fast path. Keep the C check shape for documentation.
        let eob_perc = eob as u32 * 100 / real_pels;
        debug_assert!(eob_perc < 255);
        let _ = eob_perc;
        let (cut_off_num, cut_off_denum) = rdoq_cutoffs(cfg.rdoq_level);
        let txs_ctx = coeff_c::txsize_entropy_ctx(c_tx_size);
        let o = OptimizeCtx {
            txb_costs: cfg.costs.txb(txs_ctx, plane_type),
            eob_costs: &cfg.costs.eob[coeff_c::TXSIZE_LOG2_MINUS4[c_tx_size]][plane_type],
            rdmult: rdoq_rdmult(cfg.lambda, plane_type),
            tx_size: c_tx_size,
            tx_class,
            txb_skip_ctx: 0,
            dc_sign_ctx: 0,
            cut_off_num,
            cut_off_denum,
        };
        optimize_b(tcoeffs, qcoeff, dqcoeff, &mut eob, scan, &t, &o);
    }
    eob
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Instrumented-library capture (qindex 220, g64 q55 QIQ tables line):
    /// dc zbin=326 round=195 quant=-1255 qshift=128 qfp=125 rfp=261 deq=522
    /// ac zbin=583 round=349 quant=-29571 qshift=128 qfp=70 rfp=466 deq=933
    #[test]
    fn quant_table_matches_c_qindex220() {
        let t = build_quant_table(220);
        assert_eq!(t.zbin, [326, 583]);
        assert_eq!(t.round, [195, 349]);
        assert_eq!(t.quant, [-1255, -29571]);
        assert_eq!(t.quant_shift, [128, 128]);
        assert_eq!(t.quant_fp, [125, 70]);
        assert_eq!(t.round_fp, [261, 466]);
        assert_eq!(t.dequant, [522, 933]);
    }

    /// OPTB capture: rdmult=1054880 @ lambda=248207 (q40) and
    /// rdmult=6493388 @ lambda=1527856 (q55), luma.
    #[test]
    fn rdmult_matches_c() {
        assert_eq!(rdoq_rdmult(248207, 0), 1054880);
        assert_eq!(rdoq_rdmult(1527856, 0), 6493388);
        assert_eq!(rdoq_rdmult(25650, 0), 109013); // (25650*17+2)>>2 = 436052>>2
    }

    /// COEFFLVL captures: g64 pav=5425 -> HIGH/NORMAL/NORMAL at qp
    /// 20/40/55; g128 pav=1483 -> LOW at qp 20. Thresholds 42/85/255.
    #[test]
    fn coeff_lvl_matches_c() {
        assert_eq!(derive_intra_coeff_level(5425, 20, 64, 64), CoeffLvl::High);
        assert_eq!(derive_intra_coeff_level(5425, 40, 64, 64), CoeffLvl::Normal);
        assert_eq!(derive_intra_coeff_level(5425, 55, 64, 64), CoeffLvl::Normal);
        assert_eq!(derive_intra_coeff_level(1483, 20, 128, 128), CoeffLvl::Low);
        // Boundary: cmplx == 255 is NOT > 255 -> NORMAL, 256 -> HIGH.
        assert_eq!(derive_intra_coeff_level(255, 1, 64, 64), CoeffLvl::Normal);
        assert_eq!(derive_intra_coeff_level(256, 1, 64, 64), CoeffLvl::High);
    }

    #[test]
    fn rdoq_policy_matches_c() {
        assert_eq!(rdoq_level_allintra(9, CoeffLvl::High), 0);
        assert_eq!(rdoq_level_allintra(9, CoeffLvl::Normal), 3);
        assert_eq!(rdoq_level_allintra(9, CoeffLvl::Low), 2);
        assert_eq!(rdoq_level_allintra(9, CoeffLvl::VLow), 2);
        assert_eq!(rdoq_level_allintra(5, CoeffLvl::High), 1);
        assert_eq!(rdoq_cutoffs(2), (80, 100));
        assert_eq!(rdoq_cutoffs(3), (60, 100));
    }

    /// The q/dq mirror must be the decoder's ((q * dqv) >> log_scale) at
    /// every position after fp + optimize, so the reconstruction the
    /// encoder builds is exactly what the decoder will build.
    #[test]
    fn optimize_keeps_dequant_mirror() {
        // Synthetic 16x16 (c_tx_size 2, log_scale 0) residual spectrum.
        let n = 256usize;
        let mut tcoeffs = alloc::vec![0i32; n];
        let mut s = 0x1234_5678u32;
        for (i, c) in tcoeffs.iter_mut().enumerate() {
            s = s.wrapping_mul(1103515245).wrapping_add(12345);
            let mag = (s >> 20) as i32 % 900;
            *c = if s & 1 == 0 { mag } else { -mag } / (1 + i as i32 / 16);
        }
        let scan = svtav1_entropy::scan_tables::scan(2, 0);
        let cfg = CodingQuantCfg::new(3, 248207, 160);
        let mut q = alloc::vec![0i32; n];
        let mut dq = alloc::vec![0i32; n];
        let eob =
            quantize_inv_quantize_still(&cfg, &tcoeffs, &mut q, &mut dq, scan, 160, 2, 0, 0, 256);
        let t = build_quant_table(160);
        for i in 0..n {
            let expect =
                ((q[i].unsigned_abs() as i64 * t.dequant[usize::from(i != 0)] as i64) >> 0) as i32;
            assert_eq!(dq[i].abs(), expect, "dq mirror at {i}");
            assert_eq!(dq[i] < 0, q[i] < 0 && q[i] != 0, "sign at {i}");
        }
        if eob > 0 {
            assert_ne!(
                q[scan[eob as usize - 1] as usize],
                0,
                "eob-1 must be nonzero"
            );
        }
    }
}
