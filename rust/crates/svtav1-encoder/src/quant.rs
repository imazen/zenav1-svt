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

use crate::entropy::coeff_c;

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
    /// [SVT_HDR_MODE] QM level for the plane this table quantizes (15 =
    /// identity/no matrices — the non-QM kernels run). Set by the pipeline
    /// when fork `enable_qm` is on; `build_quant_table` defaults it to 15.
    pub qm_level: u8,
}

/// C `svt_aom_invert_quant` (inv_transforms.c:3507).
pub(crate) fn invert_quant(d: i32) -> (i32, i32) {
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
/// Fixed reference `base_q_idx` the quantizer table is built against.
/// `resource_coordination_process.c:365` hardcodes `frm_hdr.quantization_
/// params.base_q_idx = 31`, and `svt_av1_build_quantizer` runs ONCE at
/// picture-0 init (`initial_rc_process.c:804`, "1 time per sequence assuming
/// the qindex offset(s) are 0") — so the whole table is built against 31 and
/// later frames/blocks just index it by their real qindex.
const QUANT_SHARPNESS_BASE_Q_IDX: i32 = 31;

/// C `svt_av1_build_quantizer`'s sharpness adjustment of the dead-zone
/// factors (`md_config_process.c:106-120`). For a block quantizing at
/// `qindex` BELOW the fixed init reference (31), a positive `sharpness`
/// lowers the zbin factor (keep more coefficients) and raises the rounding
/// factor; a negative sharpness does the mirror above the reference. Only
/// `zbin`/`round` are touched (the `_fp`/`quant`/`dequant` rows are
/// unchanged), so ONLY the dead-zone `quantize_b` path shifts — `quantize_fp`
/// (the RDOQ initial quantize) is unaffected, matching C.
///
/// `sharpness == 0` (mainline default) is a no-op → byte-inert outside fork
/// mode. Bit-depth-INVARIANT: C applies the identical adjustment when it
/// builds `quants_8bit` AND `quants_bd` (the unconditional loop has no
/// bit-depth term), so both port builders route through here.
fn apply_quant_sharpness_factors(qzbin: i32, qround: i32, qindex: u8, sharpness: i8) -> (i32, i32) {
    let sv = sharpness as i32;
    let diff = qindex as i32 - QUANT_SHARPNESS_BASE_Q_IDX;
    if (sv > 0 && diff < 0) || (sv < 0 && diff > 0) {
        let offset = if sv > 0 {
            (sv << 1).max(diff.abs())
        } else {
            ((-sv) << 1).min(diff)
        };
        let qzbin = (if sv > 0 {
            qzbin - offset
        } else {
            qzbin + offset
        })
        .clamp(1, 256);
        let qround = (if sv > 0 {
            qround + offset
        } else {
            qround - offset
        })
        .clamp(1, 256);
        (qzbin, qround)
    } else {
        (qzbin, qround)
    }
}

pub fn build_quant_table(qindex: u8) -> QuantTable {
    build_quant_table_sharp(qindex, 0)
}

/// [SVT_HDR_MODE] bd8 quant table with the fork sharpness adjustment
/// (`svt_av1_build_quantizer`, md_config_process.c). `sharpness == 0` is
/// byte-identical to [`build_quant_table`].
pub fn build_quant_table_sharp(qindex: u8, sharpness: i8) -> QuantTable {
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
    let (qzbin_factor, qrounding_factor) =
        apply_quant_sharpness_factors(qzbin_factor, qrounding_factor, qindex, sharpness);
    let mut t = QuantTable {
        zbin: [0; 2],
        round: [0; 2],
        quant: [0; 2],
        quant_shift: [0; 2],
        round_fp: [0; 2],
        quant_fp: [0; 2],
        dequant: [0; 2],
        qm_level: 15,
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

/// Bit-depth-aware quantizer row (`svt_av1_build_quantizer` at `bd`).
///
/// Additive bd10 companion to [`build_quant_table`] (task #94, the u16 MD
/// path). For `bd == 8` it is byte-identical to [`build_quant_table`] (the
/// dc/ac qlookups + the 148 qzbin threshold coincide) — asserted in tests;
/// for `bd == 10` it uses the FFI-verified `crate::bd10::{dc,ac}_qlookup_10`
/// tables and the 592 qzbin threshold (`svt_aom_get_qzbin_factor`,
/// inv_transforms.c:3492-3505). Same `_fp`/`invert_quant`/round formulas as
/// the 8-bit builder — those are bit-depth-independent (they consume the
/// per-bd `quant_qtx` only). The bd8 path never calls this, so bd8 output is
/// provably unchanged.
pub fn build_quant_table_bd(qindex: u8, bd: u8) -> QuantTable {
    build_quant_table_bd_sharp(qindex, bd, 0)
}

/// [SVT_HDR_MODE] bd-aware quant table with the fork sharpness adjustment
/// (`svt_av1_build_quantizer`, md_config_process.c:106-120). `sharpness == 0`
/// is byte-identical to [`build_quant_table_bd`].
pub fn build_quant_table_bd_sharp(qindex: u8, bd: u8, sharpness: i8) -> QuantTable {
    let (dc, ac): (i32, i32) = match bd {
        8 => (
            svtav1_dsp::quant_tables::DC_QLOOKUP_8[qindex as usize] as i32,
            svtav1_dsp::quant_tables::AC_QLOOKUP_8[qindex as usize] as i32,
        ),
        10 => (
            crate::bd10::dc_qlookup_10(qindex) as i32,
            crate::bd10::ac_qlookup_10(qindex) as i32,
        ),
        _ => unreachable!("build_quant_table_bd: only bd 8/10 supported (bd12 out of scope)"),
    };
    // svt_aom_get_qzbin_factor: q==0 -> 64; else dc < th ? 84 : 80 with
    // th = 148 (bd8) / 592 (bd10). crate::bd10::qzbin_factor encodes both.
    let qzbin_factor = crate::bd10::qzbin_factor(qindex, dc, bd);
    let qrounding_factor = if qindex == 0 { 64 } else { 48 };
    let (qzbin_factor, qrounding_factor) =
        apply_quant_sharpness_factors(qzbin_factor, qrounding_factor, qindex, sharpness);
    let mut t = QuantTable {
        zbin: [0; 2],
        round: [0; 2],
        quant: [0; 2],
        quant_shift: [0; 2],
        round_fp: [0; 2],
        quant_fp: [0; 2],
        dequant: [0; 2],
        qm_level: 15,
    };
    for (i, quant_qtx) in [dc, ac].into_iter().enumerate() {
        let (quant, shift) = invert_quant(quant_qtx);
        t.quant[i] = quant;
        t.quant_shift[i] = shift;
        t.zbin[i] = (qzbin_factor * quant_qtx + 64) >> 7;
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
    let zbins = [
        (t.zbin[0] + ((1 << log_scale) >> 1)) >> log_scale,
        (t.zbin[1] + ((1 << log_scale) >> 1)) >> log_scale,
    ];
    let round = [
        (t.round[0] + ((1 << log_scale) >> 1)) >> log_scale,
        (t.round[1] + ((1 << log_scale) >> 1)) >> log_scale,
    ];
    // Dead-zone quantize in RASTER order (archmage SIMD, bd8-safe). C's
    // `non_zero_count` prescan only skips a trailing all-dead-zone SCAN suffix,
    // whose positions quantize to 0 under the same per-position zbin test the
    // raster kernel applies everywhere — so the qcoeff/dqcoeff arrays are
    // identical and `eob` (below) is the max scan index with a nonzero qcoeff.
    svtav1_dsp::quant_coding::quantize_b_raster(
        coeffs,
        qcoeff,
        dqcoeff,
        &zbins,
        &round,
        &t.quant,
        &t.quant_shift,
        &t.dequant,
        log_scale,
    );
    eob_from_qcoeff(scan, qcoeff)
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
    // Per-coefficient quantize in RASTER order (archmage SIMD, bd8-safe); the
    // scan-order walk only mattered for `eob`, computed below. Byte-exact with
    // the former scan loop — the arithmetic at each position is
    // scan-order-independent (svtav1_dsp::quant_coding module docs).
    svtav1_dsp::quant_coding::quantize_fp_raster(
        coeffs,
        qcoeff,
        dqcoeff,
        &rounding,
        &t.quant_fp,
        &t.dequant,
        log_scale,
    );
    eob_from_qcoeff(scan, qcoeff)
}

/// `eob = 1 + max{ i : qcoeff[scan[i]] != 0 }`, else 0 — the scan-order
/// end-of-block C's quantizers return, recovered from the finished (raster)
/// `qcoeff` with a reverse scan walk (load + compare, no arithmetic).
#[inline]
pub(crate) fn eob_from_qcoeff(scan: &[u16], qcoeff: &[i32]) -> u16 {
    // Every scan the encoder hands out has a generated inverse, which turns
    // the scattered reverse walk into a contiguous SIMD max.
    match crate::entropy::scan_tables::iscan_for(scan) {
        Some(iscan) => svtav1_dsp::quant_coding::eob_from_iscan(&qcoeff[..iscan.len()], iscan),
        None => eob_by_walk(scan, qcoeff),
    }
}

/// [`eob_from_qcoeff`] by a reverse scan walk, for a scan without a
/// generated inverse.
pub(crate) fn eob_by_walk(scan: &[u16], qcoeff: &[i32]) -> u16 {
    for i in (0..scan.len()).rev() {
        if qcoeff[scan[i] as usize] != 0 {
            return (i + 1) as u16;
        }
    }
    0
}

/// C `highbd_quantize_fp_helper_c` non-QM branch (full_loop.c:367-395) — the
/// bd>8 FP quantize. IDENTICAL to [`quantize_fp`] EXCEPT it does NOT clamp
/// `abs_coeff + rounding` to INT16: high-bit-depth coefficients routinely
/// exceed 2^15, and C's highbd path uses the full value (the 8-bit
/// `quantize_fp_helper_c` clamps to INT16, C full_loop.c:245 — that clamp is
/// bd8-ONLY). Task #94: the missing highbd path is exactly what made the first
/// bd10 cell's dark-corner DC level (block 0,0) diverge. Additive — the bd8
/// [`quantize_fp`] is untouched.
pub fn quantize_fp_hbd(
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
            // NO INT16 clamp (highbd path) — C highbd_quantize_fp_helper_c:382.
            let a = abs_coeff + rounding[iz] as i64;
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

/// C `svt_aom_highbd_quantize_b_c` non-QM branch (full_loop.c:85-136) — the
/// bd>8 dead-zone (b) quantize used by the RDOQ level-0 path
/// (`av1_quantize_b_facade` routes bd>8 to `svt_aom_highbd_quantize_b`).
/// IDENTICAL to [`quantize_b`] EXCEPT it does NOT clamp `abs_coeff + round` to
/// INT16 — the SAME bd8-only clamp (C `svt_aom_quantize_b_c` full_loop.c:67)
/// that [`quantize_fp_hbd`] drops for the FP path. C's highbd variant collects
/// the non-dead-zone scan indices into `idx_arr` rather than a contiguous
/// `non_zero_count` prefix, but the two are OUTCOME-identical: every index in
/// `idx_arr` lies in `[0, non_zero_count)` (all trailing coeffs are dead-zone,
/// which is exactly the break condition), and within that prefix the inner
/// `abs_coeff >= zbin` test selects the same set — so the contiguous prescan
/// (matching the port's [`quantize_b`]) yields the same qcoeff/dqcoeff/eob. The
/// no-QM `wt = 1<<AOM_QM_BITS` folds out identically to the bd8 path. Additive:
/// the bd8 [`quantize_b`] is untouched; only the bd10 re-encode `tx_unit_hbd`
/// rdoq-level-0 branch calls this (task #94 follow-up).
pub fn quantize_b_hbd(
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
            // NO INT16 clamp (highbd path) — C svt_aom_highbd_quantize_b_c:122.
            let tmp = (abs_coeff + round) as i64;
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
        costs[i] = crate::entropy::context::av1_cost_symbol(p15) as i32;
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
    // Every field is written by the fill below, so the tables are built by
    // value: `vec![zeros; 10]` used to pay a ~110KB calloc zero-fill per SB
    // that nothing ever read.
    fn costs_from_cdf<const N: usize>(cdf: &[u16]) -> [i32; N] {
        let mut costs = [0i32; N];
        syntax_rate_from_cdf(&mut costs, cdf);
        costs
    }
    let mut eob = [[EobCosts {
        eob_cost: [[0; 11]; 2],
    }; 2]; 7];
    for eob_multi_size in 0..7 {
        for plane in 0..2 {
            let e = &mut eob[eob_multi_size][plane];
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
    let mut tables = alloc::boxed::Box::new(CoeffCostTables {
        txb: alloc::vec::Vec::with_capacity(10),
        eob,
    });

    for txs_ctx in 0..5 {
        for plane in 0..2 {
            let idx = txs_ctx * 2 + plane;
            let mut base_cost: [[i32; 8]; SIG_COEF_CONTEXTS] =
                core::array::from_fn(|ctx| costs_from_cdf(&fc.coeff_base_cdf[idx * 42 + ctx]));
            for ctx in 0..SIG_COEF_CONTEXTS {
                base_cost[ctx][4] = 0;
                base_cost[ctx][5] = base_cost[ctx][1] + cost_literal(1) - base_cost[ctx][0];
                base_cost[ctx][6] = base_cost[ctx][2] - base_cost[ctx][1];
                base_cost[ctx][7] = base_cost[ctx][3] - base_cost[ctx][2];
            }
            let br_txs = txs_ctx.min(3);
            tables.txb.push(TxbCosts {
                txb_skip_cost: core::array::from_fn(|ctx| {
                    // txb_skip_cdf is [txs_ctx][ctx] (no plane dim in C).
                    costs_from_cdf(&fc.txb_skip_cdf[txs_ctx * 13 + ctx])
                }),
                base_eob_cost: core::array::from_fn(|ctx| {
                    costs_from_cdf(&fc.coeff_base_eob_cdf[idx * 4 + ctx])
                }),
                base_cost,
                eob_extra_cost: core::array::from_fn(|ctx| {
                    costs_from_cdf(&fc.eob_extra_cdf[idx * 9 + ctx])
                }),
                dc_sign_cost: core::array::from_fn(|ctx| {
                    costs_from_cdf(&fc.dc_sign_cdf[plane * 3 + ctx])
                }),
                lps_cost: core::array::from_fn(|ctx| {
                    // coeff_br_cdf is indexed with AOMMIN(txs_ctx, TX_32X32=3).
                    let mut lps = [0i32; 2 * (COEFF_BASE_RANGE + 1)];
                    let mut br_rate = [0i32; BR_CDF_SIZE];
                    syntax_rate_from_cdf(
                        &mut br_rate,
                        &fc.coeff_br_cdf[(br_txs * 2 + plane) * 21 + ctx],
                    );
                    let mut prev_cost = 0i32;
                    let mut i = 0usize;
                    while i < COEFF_BASE_RANGE {
                        for j in 0..BR_CDF_SIZE - 1 {
                            lps[i + j] = prev_cost + br_rate[j];
                        }
                        prev_cost += br_rate[BR_CDF_SIZE - 1];
                        i += BR_CDF_SIZE - 1;
                    }
                    lps[i] = prev_cost;
                    lps[COEFF_BASE_RANGE + 1] = lps[0];
                    for i2 in 1..=COEFF_BASE_RANGE {
                        lps[i2 + COEFF_BASE_RANGE + 1] = lps[i2] - lps[i2 - 1];
                    }
                    lps
                }),
            });
        }
    }
    tables
}

// ---------------------------------------------------------------------------
// RDOQ trellis (svt_av1_optimize_b)
// ---------------------------------------------------------------------------

use svtav1_types::math::rd::rdcost_i64 as rdcost;

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
///
/// `TC` is the transform class as a CONST — see [`optimize_b`] for why the
/// whole trellis is monomorphised on it.
#[allow(clippy::too_many_arguments)]
fn coeff_cost_general<const TC: usize>(
    is_last: bool,
    ci: usize,
    abs_qc: i32,
    sign: usize,
    coeff_ctx: usize,
    dc_sign_ctx: usize,
    txb_costs: &TxbCosts,
    bwl: usize,
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
                coeff_c::br_ctx_eob_tc::<TC>(ci, bwl)
            } else {
                coeff_c::br_ctx_tc::<TC>(levels_buf, ci, bwl)
            };
            cost += br_cost(abs_qc, &txb_costs.lps_cost[br_ctx]);
        }
    }
    cost
}

/// C `get_coeff_cost_eob` (full_loop.c:722).
#[allow(clippy::too_many_arguments)]
fn coeff_cost_eob<const TC: usize>(
    ci: usize,
    abs_qc: i32,
    sign: usize,
    coeff_ctx: usize,
    dc_sign_ctx: usize,
    txb_costs: &TxbCosts,
    bwl: usize,
) -> i32 {
    let mut cost = txb_costs.base_eob_cost[coeff_ctx][(abs_qc.min(3) - 1) as usize];
    if abs_qc != 0 {
        if ci == 0 {
            cost += txb_costs.dc_sign_cost[dc_sign_ctx][sign];
        } else {
            cost += cost_literal(1);
        }
        if abs_qc > NUM_BASE_LEVELS {
            let br_ctx = coeff_c::br_ctx_eob_tc::<TC>(ci, bwl);
            cost += br_cost(abs_qc, &txb_costs.lps_cost[br_ctx]);
        }
    }
    cost
}

/// C `get_two_coeff_cost_simple` (full_loop.c:696).
#[allow(clippy::too_many_arguments)]
fn two_coeff_cost_simple<const TC: usize>(
    ci: usize,
    abs_qc: i32,
    coeff_ctx: usize,
    txb_costs: &TxbCosts,
    bwl: usize,
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
            let br_ctx = coeff_c::br_ctx_tc::<TC>(levels_buf, ci, bwl);
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
    eob_cost_inner(eob, eob_costs, txb_costs, tx_class != coeff_c::TX_CLASS_2D)
}

/// [`eob_cost`] with the transform class as a CONST.
#[inline(always)]
fn eob_cost_tc<const TC: usize>(eob: i32, eob_costs: &EobCosts, txb_costs: &TxbCosts) -> i32 {
    eob_cost_inner(eob, eob_costs, txb_costs, TC != coeff_c::TX_CLASS_2D)
}

#[inline(always)]
fn eob_cost_inner(eob: i32, eob_costs: &EobCosts, txb_costs: &TxbCosts, is_1d_class: bool) -> i32 {
    let (eob_pt, eob_extra) = coeff_c::eob_pos_token(eob);
    let eob_multi_ctx = usize::from(is_1d_class);
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

/// Padded-map position of raster index `ci` inside the body-anchored
/// sub-slice [`coeff_c::txb_init_levels`] returns (C `get_padded_idx` on
/// `levels`, the `set_levels` pointer).
#[inline]
fn levels_idx(ci: usize, bwl: usize) -> usize {
    coeff_c::padded_idx(ci, bwl)
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
    /// [SVT_HDR_MODE] C local `sharpness = 1` (sharp-tx active on luma with
    /// delta_q_present): disables the trellis eob-shortening branches
    /// (full_loop.c:822/955 `sharpness == 0 &&` gates).
    pub sharpness_flag: bool,
    /// [SVT_HDR_MODE] QM inverse-weight slice for this txb (None = no QM).
    /// The trellis reads every dequant through C `get_dqv`
    /// (full_loop.c:741), which applies this weight.
    pub iwt: Option<&'a [u8]>,
    /// `rdoq_ctrls.cut_off_num / denum` (0 num = full RDOQ).
    pub cut_off_num: u32,
    pub cut_off_denum: u32,
}

/// C `plane_rd_mult[allintra || rtc][is_inter][plane_type]` — full_loop.c:994,
/// the MAINLINE table (the `#if TUNE_CHROMA_SSIM` twin at :985 is the fork's
/// and is only compiled with `SVT_HDR_MODE`; mainline v4.2 defines
/// `TUNE_CHROMA_SSIM 0`, EbDebugMacros.h:70).
///
/// The first index is the one this port ran as a constant `1` until
/// 2026-09-01: `scs->allintra || scs->static_config.rtc`. A VIDEO-mode frame
/// has BOTH false, so its CHROMA RDOQ weights rate at **20** where the
/// allintra arm weights it at 13 (intra) / 10 (inter). Luma is 17/16 on both
/// arms, which is why the divergence this fixed was chroma-only: on the
/// reference cell `gradient 64x64 q40 p6` video, the four coded 32x32 blocks'
/// LUMA levels were already byte-identical to C's while every chroma txb
/// differed (C kept at most one DC coefficient, the port kept DC + AC).
pub const PLANE_RD_MULT: [[[i64; 2]; 2]; 2] = [
    // [0] = video: neither allintra nor rtc
    [[17, 20], [16, 20]],
    // [1] = allintra or rtc
    [[17, 13], [16, 10]],
];

/// Look up [`PLANE_RD_MULT`]. `allintra_rd_mult` is C's `allintra || rtc`.
///
/// `is_inter` is a real axis of C's table — `pred_mode >= NEARESTMV`
/// (full_loop.c `svt_av1_optimize_b`), so IntraBC (mode DC_PRED) counts as
/// INTRA here even though it is inter-classified for the ext-tx set and the
/// tx-type rate rows. Real inter blocks reached the funnel's `tx_unit`
/// RDOQ in the inter campaign; the still/pack paths keep `false`.
pub fn plane_rd_mult(allintra_rd_mult: bool, is_inter: bool, plane_type: usize) -> i64 {
    PLANE_RD_MULT[usize::from(allintra_rd_mult)][usize::from(is_inter)][plane_type & 1]
}

/// C rdmult derivation inside `svt_av1_optimize_b` (full_loop.c:1074) for
/// the allintra/still path: sharpness 0 -> rweight 100, rshift 2;
/// `plane_rd_mult[allintra=1][is_inter=0][plane_type]` = 17 luma, 13 chroma.
///
/// The `allintra = 1` is BAKED IN — this is the still-path convenience form.
/// A video frame's CHROMA weight is 20, not 13 (see [`PLANE_RD_MULT`]); call
/// [`rdoq_rdmult_full`] with the frame's own arm for anything on that path.
pub fn rdoq_rdmult(lambda: u32, plane_type: usize) -> i64 {
    rdoq_rdmult_sharp(lambda, plane_type, 0, false)
}

/// Full C rweight/rshift derivation (full_loop.c `svt_av1_optimize_b`):
/// `rshift = MAX(2, CLIP3(0,7,sharpness))`; `rweight` 100 normally, **10
/// under the fork's light-RDOQ** (low-DC chroma, `#if SVT_HDR_MODE`).
/// The `(use_sharpness || sharp_tx) && delta_q_present && plane==0` block
/// (rweight=0 + local sharpness=1 into update_coeff_general) is DORMANT in
/// this port until per-SB delta-q lands — tracked in docs/HDR-ON-4.2.md.
/// Same allintra baking as [`rdoq_rdmult`] — read its note before using this
/// on a video frame.
pub fn rdoq_rdmult_sharp(lambda: u32, plane_type: usize, sharpness: i8, light_rdoq: bool) -> i64 {
    rdoq_rdmult_full(
        lambda, plane_type, sharpness, light_rdoq, false, true, false,
    )
}

/// Full form incl. the sharp-tx `rweight = 0` path (C full_loop.c:1075).
#[allow(clippy::too_many_arguments)]
pub fn rdoq_rdmult_full(
    lambda: u32,
    plane_type: usize,
    sharpness: i8,
    light_rdoq: bool,
    sharp_tx_active: bool,
    // C `scs->allintra || scs->static_config.rtc` — selects the first index
    // of `PLANE_RD_MULT`. FALSE on a video-mode frame.
    allintra_rd_mult: bool,
    // C `pred_mode >= NEARESTMV` — selects the second index of
    // `PLANE_RD_MULT`. Real inter blocks only: IntraBC (mode DC_PRED) is
    // intra on THIS axis (see `plane_rd_mult`'s doc).
    is_inter: bool,
) -> i64 {
    let sharpness_val = i64::from(sharpness).clamp(0, 7);
    let rshift = sharpness_val.max(2) as u32;
    let rweight: i64 = if light_rdoq {
        10
    } else if sharp_tx_active {
        0
    } else {
        100
    };
    let prm = plane_rd_mult(allintra_rd_mult, is_inter, plane_type);
    ((lambda as i64 * prm * rweight) / 100 + 2) >> rshift
}

/// Fork-only light-RDOQ trigger (C hybrid full_loop.c, `#if SVT_HDR_MODE`):
/// on the ENCODE pass, chroma blocks with a near-zero DC and a sparse tail
/// get a weakened RDOQ (rweight 10) to prevent color blotching from
/// aggressive coefficient decimation. `n_coeffs` = `av1_get_max_eob`
/// (= adjusted `txb_wide * txb_high`, 32-capped).
pub fn light_rdoq_low_dc_chroma(
    fork_mode: bool,
    is_encode_pass: bool,
    is_idtx: bool,
    plane_type: usize,
    dc: i32,
    eob: u16,
    n_coeffs: u32,
) -> bool {
    if !fork_mode || !is_encode_pass || is_idtx || plane_type == 0 || eob == 0 {
        return false;
    }
    ((-1..=1).contains(&dc) && u32::from(eob) <= n_coeffs / 16)
        || ((-4..=4).contains(&dc) && eob <= 1)
}

/// C `update_coeff_general` (full_loop.c:851).
#[allow(clippy::too_many_arguments)]
fn update_coeff_general<const TC: usize>(
    accu_rate: &mut i32,
    accu_dist: &mut i64,
    si: usize,
    eob: u16,
    o: &OptimizeCtx,
    bwl: usize,
    height: usize,
    dequant: &[i32; 2],
    iwt: Option<&[u8]>,
    shift: i32,
    scan: &[u16],
    tcoeff: &[i32],
    qcoeff: &mut [i32],
    dqcoeff: &mut [i32],
    levels_buf: &mut [u8],
) {
    let ci = scan[si] as usize;
    let dqv = crate::qm::dqv_qm(dequant, ci, iwt);
    let qc = qcoeff[ci];
    let is_last = si == (eob as usize - 1);
    let coeff_ctx = coeff_c::lower_levels_ctx_general_tc::<TC>(
        levels_buf, ci, bwl, height, si, is_last, o.tx_size,
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
        let rate = coeff_cost_general::<TC>(
            is_last,
            ci,
            abs_qc,
            sign,
            coeff_ctx,
            o.dc_sign_ctx,
            o.txb_costs,
            bwl,
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
            rate_low = coeff_cost_general::<TC>(
                is_last,
                ci,
                abs_qc_low,
                sign,
                coeff_ctx,
                o.dc_sign_ctx,
                o.txb_costs,
                bwl,
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
fn update_coeff_simple<const TC: usize>(
    accu_rate: &mut i32,
    si: usize,
    o: &OptimizeCtx,
    bwl: usize,
    dequant: &[i32; 2],
    iwt: Option<&[u8]>,
    shift: i32,
    scan: &[u16],
    tcoeff: &[i32],
    qcoeff: &mut [i32],
    dqcoeff: &mut [i32],
    levels_buf: &mut [u8],
) {
    debug_assert!(si > 0);
    let ci = scan[si] as usize;
    let dqv = crate::qm::dqv_qm(dequant, ci, iwt);
    let qc = qcoeff[ci];
    let coeff_ctx =
        coeff_c::lower_levels_ctx_general_tc::<TC>(levels_buf, ci, bwl, 0, si, false, o.tx_size);
    if qc == 0 {
        *accu_rate += o.txb_costs.base_cost[coeff_ctx][0];
    } else {
        let abs_qc = qc.abs();
        let abs_tqc = tcoeff[ci].abs();
        let abs_dqc = dqcoeff[ci].abs();
        let mut rate_low = 0i32;
        let rate = two_coeff_cost_simple::<TC>(
            ci,
            abs_qc,
            coeff_ctx,
            o.txb_costs,
            bwl,
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

        #[cfg(feature = "std")]
        if crate::dbgenv::trellis() {
            std::eprintln!(
                "TSMP si={si} ci={ci} qc={qc} tqc={abs_tqc} dqc={abs_dqc} ctx={coeff_ctx} rate={rate} ratel={rate_low} dist={dist} distl={dist_low} rd={rd} rdlow={rd_low} dqv={dqv} sh={shift} rm={} bc={:?}",
                o.rdmult,
                o.txb_costs.base_cost[coeff_ctx]
            );
        }
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
fn update_coeff_eob<const TC: usize>(
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
    iwt: Option<&[u8]>,
    shift: i32,
    scan: &[u16],
    tcoeff: &[i32],
    qcoeff: &mut [i32],
    dqcoeff: &mut [i32],
    levels_buf: &mut [u8],
) {
    debug_assert!(si != *eob as usize - 1);
    let ci = scan[si] as usize;
    let dqv = crate::qm::dqv_qm(dequant, ci, iwt);
    let qc = qcoeff[ci];
    let coeff_ctx =
        coeff_c::lower_levels_ctx_general_tc::<TC>(levels_buf, ci, bwl, 0, si, false, o.tx_size);
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
        let mut rate = coeff_cost_general::<TC>(
            false,
            ci,
            abs_qc,
            sign,
            coeff_ctx,
            o.dc_sign_ctx,
            o.txb_costs,
            bwl,
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
            rate_low = coeff_cost_general::<TC>(
                false,
                ci,
                abs_qc_low,
                sign,
                coeff_ctx,
                o.dc_sign_ctx,
                o.txb_costs,
                bwl,
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
        let coeff_ctx_new_eob = coeff_c::lower_levels_ctx_general_tc::<TC>(
            levels_buf, ci, bwl, height, si, true, o.tx_size,
        );
        let new_eob_cost = eob_cost_tc::<TC>(new_eob as i32, o.eob_costs, o.txb_costs);
        let mut rate_coeff_eob = new_eob_cost
            + coeff_cost_eob::<TC>(
                ci,
                abs_qc,
                sign,
                coeff_ctx_new_eob,
                o.dc_sign_ctx,
                o.txb_costs,
                bwl,
            );
        let mut dist_new_eob = dist;
        let mut rd_new_eob = rdcost(o.rdmult, rate_coeff_eob as i64, dist_new_eob);

        if abs_qc_low > 0 {
            let rate_coeff_eob_low = new_eob_cost
                + coeff_cost_eob::<TC>(
                    ci,
                    abs_qc_low,
                    sign,
                    coeff_ctx_new_eob,
                    o.dc_sign_ctx,
                    o.txb_costs,
                    bwl,
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

        #[cfg(feature = "std")]
        if crate::dbgenv::trellis() {
            std::eprintln!(
                "TREL si={si} ci={ci} qc={qc} ctx={coeff_ctx} ctxE={coeff_ctx_new_eob} ar={} ad={} rate={rate} dist={dist} rd={rd} rdlow={rd_low} rdn={rd_new_eob} nec={new_eob_cost} rm={}",
                *accu_rate,
                *accu_dist,
                o.rdmult
            );
        }
        // C gate: `sharpness == 0 && rd_new_eob < rd` (sharp-tx sets 1).
        if !o.sharpness_flag && rd_new_eob < if rd_low < rd { rd_low } else { rd } {
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
    sharpness_flag: bool,
) {
    let rd = rdcost(rdmult, (*accu_rate + non_skip_cost) as i64, accu_dist);
    let rd_new_eob = rdcost(rdmult, skip_cost as i64, 0);
    if !sharpness_flag && rd_new_eob < rd {
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
///
/// **The trellis is MONOMORPHISED on `tx_class`, as C's is.** C writes the
/// three scan loops as `switch (tx_class) { case TX_CLASS_2D:
/// UPDATE_COEFF_EOB_CASE(TX_CLASS_2D); ... }` (full_loop.c), a macro that
/// expands the whole body once per class with `tx_class` a compile-time
/// literal — which is why `get_nz_map_ctx` appears NOWHERE in C's profile:
/// every context derivation constant-folds into
/// `svt_aom_quantize_inv_quantize`. This function dispatches ONCE on
/// `o.tx_class` and every callee below takes the class as a const generic, so
/// the per-coefficient three-way branches fold the same way. The dispatch is
/// exhaustive because `tx_type_to_class` is ternary
/// (`coeff_c::tx_class_tests::tx_type_to_class_is_ternary`).
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
    coeff_c::with_txb_scratch(|sc| {
        let lv = &mut sc.levels;
        match o.tx_class {
            coeff_c::TX_CLASS_HORIZ => optimize_b_tc::<{ coeff_c::TX_CLASS_HORIZ }>(
                tcoeffs, qcoeff, dqcoeff, eob, scan, t, o, lv,
            ),
            coeff_c::TX_CLASS_VERT => optimize_b_tc::<{ coeff_c::TX_CLASS_VERT }>(
                tcoeffs, qcoeff, dqcoeff, eob, scan, t, o, lv,
            ),
            _ => {
                debug_assert_eq!(o.tx_class, coeff_c::TX_CLASS_2D);
                optimize_b_tc::<{ coeff_c::TX_CLASS_2D }>(
                    tcoeffs, qcoeff, dqcoeff, eob, scan, t, o, lv,
                )
            }
        }
    })
}

/// [`optimize_b`]'s body, with the transform class as a CONST.
#[allow(clippy::too_many_arguments)]
fn optimize_b_tc<const TC: usize>(
    tcoeffs: &[i32],
    qcoeff: &mut [i32],
    dqcoeff: &mut [i32],
    eob: &mut u16,
    scan: &[u16],
    t: &QuantTable,
    o: &OptimizeCtx,
    levels_buf: &mut [u8; coeff_c::LEVELS_SCRATCH_LEN],
) {
    debug_assert_eq!(TC, o.tx_class);
    let shift = TX_SCALE_TAB[o.tx_size];
    let bwl = coeff_c::txb_bwl(o.tx_size);
    let width = coeff_c::txb_wide(o.tx_size);
    let height = coeff_c::txb_high(o.tx_size);
    let non_skip_cost = o.txb_costs.txb_skip_cost[o.txb_skip_ctx][0];
    let skip_cost = o.txb_costs.txb_skip_cost[o.txb_skip_ctx][1];
    let eob_cost_init = eob_cost_tc::<TC>(*eob as i32, o.eob_costs, o.txb_costs);

    // The body-anchored sub-slice every trellis reader/writer indexes by
    // `padded_idx`. `eob > 1` fills it; at `eob <= 1` the trellis never reads
    // the map (the `is_eob`/`br_ctx_eob` arms are position-only), but
    // `levels_skip_init` still clears the small tap reach so the sub-slice is
    // uniformly "the map for this shape".
    let levels: &mut [u8] = if *eob > 1 {
        coeff_c::txb_init_levels(qcoeff, width, height, levels_buf)
    } else {
        coeff_c::levels_skip_init(levels_buf, width, height)
    };

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
        update_coeff_general::<TC>(
            &mut accu_rate,
            &mut accu_dist,
            si as usize,
            *eob,
            o,
            bwl,
            height,
            &t.dequant,
            o.iwt,
            shift,
            scan,
            tcoeffs,
            qcoeff,
            dqcoeff,
            levels,
        );
        si -= 1;
    } else {
        debug_assert_eq!(abs_qc, 1);
        let coeff_ctx = coeff_c::lower_levels_ctx_general_tc::<TC>(
            levels,
            ci,
            bwl,
            height,
            si as usize,
            true,
            o.tx_size,
        );
        accu_rate +=
            coeff_cost_eob::<TC>(ci, abs_qc, sign, coeff_ctx, o.dc_sign_ctx, o.txb_costs, bwl);
        let tqc = tcoeffs[ci];
        let dqc = dqcoeff[ci];
        let dist = get_coeff_dist(tqc, dqc, shift);
        let dist0 = get_coeff_dist(tqc, 0, shift);
        accu_dist += dist - dist0;
        si -= 1;
    }

    while si >= 0 && nz_num <= MAX_NZ_NUM {
        update_coeff_eob::<TC>(
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
            o.iwt,
            shift,
            scan,
            tcoeffs,
            qcoeff,
            dqcoeff,
            levels,
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
            o.sharpness_flag,
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
        update_coeff_simple::<TC>(
            &mut accu_rate,
            si as usize,
            o,
            bwl,
            &t.dequant,
            o.iwt,
            shift,
            scan,
            tcoeffs,
            qcoeff,
            dqcoeff,
            levels,
        );
        si -= 1;
    }

    // DC position.
    if si == 0 {
        let mut dummy_dist = 0i64;
        update_coeff_general::<TC>(
            &mut accu_rate,
            &mut dummy_dist,
            0,
            *eob,
            o,
            bwl,
            height,
            &t.dequant,
            o.iwt,
            shift,
            scan,
            tcoeffs,
            qcoeff,
            dqcoeff,
            levels,
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

/// C `derive_inter_coeff_level` (md_config_process.c:650): the INTER twin of
/// [`derive_intra_coeff_level`].
///
/// Two things differ and both matter: the complexity input is
/// `ppcs->norm_me_dist` (the mean of the open-loop ME's per-b64 8x8
/// distortion, `initial_rc_process.c:718-726`) rather than
/// `pic_avg_variance`, and the thresholds are the INTER set
/// `{5833/96, 5833/48, 16666/48}` = `{60, 121, 347}` (definitions.h:279-281,
/// integer division in C — the port spells the RESULTS so the truncation
/// cannot drift). The resolution scaling is the same ladder.
///
/// C runs it only when `!scs->allintra && !rtc && slice_type != I_SLICE`
/// (md_config_process.c:898-903): a VIDEO-mode KEY frame takes NEITHER arm
/// and keeps `INVALID_LVL`.
#[must_use]
pub fn derive_inter_coeff_level(norm_me_dist: u64, cli_qp: u32, w: usize, h: usize) -> CoeffLvl {
    let pixels = w * h;
    // C: `(5833 / 96)`, `(5833 / 48)`, `(16666 / 48)` — INTEGER division in
    // the macro, before the floating-point resolution scale.
    let (mut vlow, mut low, mut high) = (60.0f64, 121.0f64, 347.0f64);
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
    let cmplx = norm_me_dist / u64::from(1.max(cli_qp));
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

#[cfg(test)]
mod inter_coeff_level_tests;

/// C allintra RDOQ policy (enc_mode_config.c:14931), `OPT_APPROX_COEFF_RATE`
/// branch: presets <= M5 always level 1; above, by coeff_lvl.
pub fn rdoq_level_allintra(eff_enc_mode: i8, coeff_lvl: CoeffLvl) -> u8 {
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
    /// Picture coefficient class shared with research partition search.
    pub(crate) input_coeff_level: CoeffLvl,
    /// 0 = quantize_b (no RDOQ); >= 1 = quantize_fp + optimize_b.
    pub rdoq_level: u8,
    /// SVT_HDR_MODE: fork behaviors (light-RDOQ) may fire when true.
    pub hdr_fork: bool,
    /// Config sharpness (fork default 1; feeds the rshift formula, which
    /// only departs from mainline at sharpness >= 3).
    pub sharpness: i8,
    /// True on the encode/pack pass (C `is_encode_pass`) — the fork's
    /// light-RDOQ trigger is encode-pass-only.
    pub is_encode_pass: bool,
    /// Fork `--noise-norm-strength` (0 = off; fork default 1). Encode-pass
    /// luma-only coefficient revival after quantization/RDOQ.
    pub noise_norm_strength: u8,
    /// [SVT_HDR_MODE] sharp-tx RDOQ active (fork sharp_tx=1 + delta_q
    /// present): luma trellis gets rweight=0 + eob-shortening disabled.
    pub sharp_tx_active: bool,
    /// [SVT_HDR_MODE] per-plane QM levels [Y, U, V] (15 = identity/off).
    pub qm_levels: [u8; 3],
    /// C `scs->allintra || scs->static_config.rtc` — the first index of
    /// [`PLANE_RD_MULT`]. TRUE on the still/allintra path this struct was
    /// written for; FALSE on a video-mode frame, where chroma RDOQ weights
    /// rate at 20 instead of 13.
    pub allintra_rd_mult: bool,
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
            input_coeff_level: CoeffLvl::Normal,
            rdoq_level,
            hdr_fork: false,
            sharpness: 0,
            is_encode_pass: true,
            noise_norm_strength: 0,
            sharp_tx_active: false,
            qm_levels: [15; 3],
            allintra_rd_mult: true,
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
    is_idtx: bool,
    qm_level: u8,
) -> u16 {
    let t = build_quant_table(qindex);
    let log_scale = TX_SCALE_TAB[c_tx_size];
    // [SVT_HDR_MODE] QM applies to 2D transforms only; the caller passes
    // qm_level 15 for non-2D tx types (IS_2D_TRANSFORM gate).
    let qm = if !is_idtx && qm_level < 15 {
        crate::qm::qm_slices(usize::from(qm_level), plane_type == 1, c_tx_size)
    } else {
        None
    };
    // [SVT_HDR_MODE] fork noise normalization: encode-pass luma, non-IDTX,
    // after ALL quantization/RDOQ (C svt_aom_quantize_inv_quantize tail).
    let run_noise_norm = |qc: &mut [i32], dqc: &mut [i32], e: &mut u16| {
        if cfg.hdr_fork
            && cfg.noise_norm_strength > 0
            && cfg.is_encode_pass
            && *e != 0
            && !is_idtx
            && plane_type == 0
        {
            crate::noise_norm::perform_noise_normalization(
                &t.dequant,
                qm.map(|(_, iwt)| iwt),
                tcoeffs,
                qc,
                dqc,
                e,
                scan,
                c_tx_size,
                cfg.noise_norm_strength,
            );
        }
    };
    if cfg.rdoq_level == 0 {
        let mut eob = match qm {
            Some((wt, iwt)) => {
                crate::qm::quantize_b_qm(tcoeffs, scan, &t, log_scale, wt, iwt, qcoeff, dqcoeff)
            }
            None => quantize_b(tcoeffs, scan, &t, log_scale, qcoeff, dqcoeff),
        };
        run_noise_norm(qcoeff, dqcoeff, &mut eob);
        return eob;
    }
    let mut eob = match qm {
        Some((wt, iwt)) => {
            crate::qm::quantize_fp_qm(tcoeffs, scan, &t, log_scale, wt, iwt, qcoeff, dqcoeff)
        }
        None => quantize_fp(tcoeffs, scan, &t, log_scale, qcoeff, dqcoeff),
    };
    if eob != 0 {
        // [SVT_HDR_MODE] fork light-RDOQ: weakened trellis on low-DC chroma
        // (encode pass only). n_coeffs = adjusted txb pels (av1_get_max_eob).
        let n_coeffs = (coeff_c::txb_wide(c_tx_size) * coeff_c::txb_high(c_tx_size)) as u32;
        let light_rdoq = light_rdoq_low_dc_chroma(
            cfg.hdr_fork,
            cfg.is_encode_pass,
            is_idtx,
            plane_type,
            qcoeff[scan[0] as usize],
            eob,
            n_coeffs,
        );
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
            rdmult: rdoq_rdmult_full(
                cfg.lambda,
                plane_type,
                cfg.sharpness,
                light_rdoq,
                cfg.sharp_tx_active && plane_type == 0,
                cfg.allintra_rd_mult,
                // Still/pack-path quantizer — inter blocks are coded by
                // the funnel's `tx_unit`, which passes the real axis.
                false,
            ),
            sharpness_flag: cfg.sharp_tx_active && plane_type == 0,
            iwt: qm.map(|(_, iwt)| iwt),
            tx_size: c_tx_size,
            tx_class,
            txb_skip_ctx: 0,
            dc_sign_ctx: 0,
            cut_off_num,
            cut_off_denum,
        };
        optimize_b(tcoeffs, qcoeff, dqcoeff, &mut eob, scan, &t, &o);
    }
    run_noise_norm(qcoeff, dqcoeff, &mut eob);
    eob
}

#[cfg(test)]
mod tests;
