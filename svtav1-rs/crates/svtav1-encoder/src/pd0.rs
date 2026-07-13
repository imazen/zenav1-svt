//! C-exact light-PD0 partition decision for allintra high presets.
//!
//! SVT-AV1 clamps allintra presets above M9 down to M9
//! (`enc_handle.c:4634-4644`), and at effective M9 the final partition
//! tree of every superblock is decided ENTIRELY by the PD0 pass:
//! `pred_depth_only=1`, `md_disallow_nsq_search=1`, `fixed_partition=1`,
//! so PD1 (light-PD1) codes exactly the PD0-picked {NONE, SPLIT} square
//! quadtree (no HORZ/VERT/AB/4:1 shapes are ever evaluated).
//!
//! This module ports that decision verbatim from the C sources
//! (v4.2.0-rc, all `CLN_RENAME_PD0`/`OPT_VLPD0_*` feature macros = 1):
//!
//! - `compute_b64_variance` (pic_analysis_process.c:312) — the 85-entry
//!   per-64x64 variance map at `BLOCK_MEAN_PREC_SUB` (even-row
//!   subsampled means), used by every decision below.
//! - `svt_aom_get_qp_based_th_scaling_factors` (md_config_process.c) —
//!   qp-based threshold scaling (both `lpd0_` and `cap_max_size_`
//!   variants are enabled at every preset, enc_handle.c:3990-4007).
//! - `get_max_block_size_allintra` (enc_mode_config.c:8969) — at
//!   effective >= M8 the 64x64 depth is REMOVED whenever the SB's 64x64
//!   source variance exceeds `round(7500 * qw / qwd)`; PD0 then has no
//!   parent cost at 64x64 and SPLIT is forced.
//! - `pd0_detector_allintra` (enc_dec_process.c:2373) — demotes
//!   `PD0_LVL_6 -> PD0_LVL_5` when the per-depth normalized variances
//!   are flat (no dominant depth).
//! - `compute_lpd0_cost_allintra` (product_coding_loop.c:8418) — the
//!   LVL_6 closed-form variance cost.
//! - `md_encode_block_pd0`/`full_loop_core_pd0`/`perform_tx_pd0`
//!   (product_coding_loop.c) — the LVL_5 light block encode: single
//!   DC_PRED candidate (inject_intra_candidates_pd0), prediction from
//!   SOURCE neighbors (`pd0_use_src_samples=1` for allintra,
//!   enc_mode_config.c:9437) with the spec unavailable-edge fills,
//!   max-square TX at depth 0 with optional row subsampling (subres
//!   step 1; gated per SB by `check_is_subres_safe` on the 64x64 DC
//!   prediction), `svt_aom_quantize_b` at `qindex + 8`
//!   (rate_est_ctrls.lpd0_qp_offset), frequency-domain SSE distortion
//!   (coeff vs dequantized coeff over the packed <=32x32 region plus
//!   `three_quad_energy`), coefficient rate `5000 + 100*eob`
//!   (`coeff_rate_est_lvl == 0`, product_coding_loop.c:4568), and
//!   `full_cost = RDCOST(lambda, bits + skip_bits + part_none_bits,
//!   dist)` (svt_aom_full_cost_pd0, rd_cost.c:1335).
//! - `test_split_partition_pd0` (product_coding_loop.c:10897) — the
//!   parent-vs-children compare: `split_cost = RDCOST(lambda,
//!   2 * partition_split_bits, 0) + sum(children)` (the x2 because
//!   `use_accurate_part_ctx = enc_mode <= M8` is false at M9; the split
//!   rate term is 0 entirely at LVL_6 allintra), parent wins iff
//!   `1000 * parent <= 1000 * split` (parent_cost_bias = 1000 for
//!   allintra), with the LVL_5-only early exits (split_cost_th=50,
//!   early_exit_th=0 -> treated as 1000).
//! - `svt_aom_compute_rd_mult` KF chain (rc_process.c:452) — the PD0
//!   lambda: `(3.3 + 0.0015*dc_q) * dc_q^2` truncated, `*150 >> 7`
//!   (rd_frame_type_factor[8bit][KF]); the stats-based factor is 128
//!   (qdiff 0) and lambda_scale_factors are 128, both no-ops.
//!
//! Every constant and every per-block cost in the unit tests below was
//! captured from the instrumented C library running the identity-harness
//! gradient-64 configs (docs/IDENTITY-STATUS.md, 2026-07-13 diagnosis).

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Variance map (pic_analysis_process.c compute_b64_variance, PREC_SUB)
// ---------------------------------------------------------------------------

/// The 85-entry per-64x64-block variance map: index 0 = 64x64,
/// 1..=4 = 32x32 (2x2 raster), 5..=20 = 16x16 (4x4), 21..=84 = 8x8 (8x8).
pub(crate) struct SbVariance(pub [u16; 85]);

/// C `compute_b64_variance` at `BLOCK_MEAN_PREC_SUB` (the default,
/// enc_handle.c:4618): 8x8 means/mean-squares from the EVEN rows only
/// (rows 0,2,4,6 of each 8x8), `mean << 3` / `mean_sq << 11` fixed point,
/// averaged up the quad tree with `>> 2`, variance = `(msq - m*m) >> 16`
/// cast to u16 per level.
///
/// The source region must be fully inside the picture (the C input is the
/// edge-padded `input_padded_pic`; every current caller pads frames to
/// 64-aligned dimensions so the region is always complete).
pub(crate) fn compute_b64_variance(src: &[u8], stride: usize, org_x: usize, org_y: usize) -> SbVariance {
    let mut mean8 = [0u64; 64];
    let mut msq8 = [0u64; 64];
    for by in 0..8 {
        for bx in 0..8 {
            let mut sum = 0u64;
            let mut sq = 0u64;
            for r in [0usize, 2, 4, 6] {
                let row = (org_y + by * 8 + r) * stride + org_x + bx * 8;
                for c in 0..8 {
                    let v = src[row + c] as u64;
                    sum += v;
                    sq += v * v;
                }
            }
            mean8[by * 8 + bx] = sum << 3;
            msq8[by * 8 + bx] = sq << 11;
        }
    }
    let mut mean16 = [0u64; 16];
    let mut msq16 = [0u64; 16];
    for by in 0..4 {
        for bx in 0..4 {
            let f = by * 16 + bx * 2;
            mean16[by * 4 + bx] = (mean8[f] + mean8[f + 1] + mean8[f + 8] + mean8[f + 9]) >> 2;
            msq16[by * 4 + bx] = (msq8[f] + msq8[f + 1] + msq8[f + 8] + msq8[f + 9]) >> 2;
        }
    }
    let mut mean32 = [0u64; 4];
    let mut msq32 = [0u64; 4];
    for by in 0..2 {
        for bx in 0..2 {
            let f = by * 8 + bx * 2;
            mean32[by * 2 + bx] = (mean16[f] + mean16[f + 1] + mean16[f + 4] + mean16[f + 5]) >> 2;
            msq32[by * 2 + bx] = (msq16[f] + msq16[f + 1] + msq16[f + 4] + msq16[f + 5]) >> 2;
        }
    }
    let mean64 = (mean32[0] + mean32[1] + mean32[2] + mean32[3]) >> 2;
    let msq64 = (msq32[0] + msq32[1] + msq32[2] + msq32[3]) >> 2;

    const VARIANCE_PRECISION: u32 = 16;
    let mut v = [0u16; 85];
    v[0] = (msq64.wrapping_sub(mean64 * mean64) >> VARIANCE_PRECISION) as u16;
    for i in 0..4 {
        v[1 + i] = (msq32[i].wrapping_sub(mean32[i] * mean32[i]) >> VARIANCE_PRECISION) as u16;
    }
    for i in 0..16 {
        v[5 + i] = (msq16[i].wrapping_sub(mean16[i] * mean16[i]) >> VARIANCE_PRECISION) as u16;
    }
    for i in 0..64 {
        v[21 + i] = (msq8[i].wrapping_sub(mean8[i] * mean8[i]) >> VARIANCE_PRECISION) as u16;
    }
    SbVariance(v)
}

/// C `svt_aom_get_blk_var_map` (product_coding_loop.c:8368): variance-map
/// index of the square block at (org_x, org_y) relative to the SB, plus
/// its four sub-block indices (valid for block_size >= 16).
fn blk_var_map(block_size: usize, org_x: usize, org_y: usize) -> (usize, [usize; 4]) {
    const LOG2_LUT: [usize; 4] = [6, 5, 4, 3];
    const GRID_LUT: [usize; 4] = [1, 2, 4, 8];
    const BASE_LUT: [usize; 4] = [0, 1, 5, 21];
    let lvl = 6 - block_size.ilog2() as usize;
    debug_assert!(lvl <= 3);
    let blk_idx = BASE_LUT[lvl] + (org_y >> LOG2_LUT[lvl]) * GRID_LUT[lvl] + (org_x >> LOG2_LUT[lvl]);
    let sub_lvl = lvl + 1;
    let sub = if sub_lvl < 4 {
        let (ss, sb, sg) = (LOG2_LUT[sub_lvl], BASE_LUT[sub_lvl], GRID_LUT[sub_lvl]);
        let (sx, sy) = (org_x >> ss, org_y >> ss);
        [
            sb + sy * sg + sx,
            sb + sy * sg + sx + 1,
            sb + (sy + 1) * sg + sx,
            sb + (sy + 1) * sg + sx + 1,
        ]
    } else {
        [0; 4]
    };
    (blk_idx, sub)
}

// ---------------------------------------------------------------------------
// QP-based threshold scaling + RDCOST + lambda
// ---------------------------------------------------------------------------

/// C `DIVIDE_AND_ROUND` (utility.h:96).
fn divide_and_round(x: u64, y: u64) -> u64 {
    (x + (y >> 1)) / y
}

/// C `svt_aom_get_qp_based_th_scaling_factors` (md_config_process.c) with
/// scaling enabled (both users here — `lpd0_` and `cap_max_size_` — are
/// enabled at every preset, enc_handle.c:3990-4007).
///
/// qp < 46: `(max(10, qp), 63)`. qp >= 46:
/// `((1.05 - exp(-(max(40,qp)-35)/10)) * 10000) as u32, 10000)` — the
/// f64 `exp` matches C's libm result for all 18 reachable qp values
/// (pinned in tests; the truncation to 1e-4 makes ulp differences moot).
pub(crate) fn qp_th_scaling_factors(qp: u32) -> (u32, u32) {
    if qp >= 46 {
        let ex = -((qp.max(40) as f64) - 35.0) / 10.0;
        let w = (1.05 - ex.exp()) * 10000.0;
        (w as u32, 10000)
    } else {
        (qp.max(10), 63)
    }
}

/// C `RDCOST(RM, R, D)` (rd_cost.h:36): `ROUND_POWER_OF_TWO(R*RM, 9) +
/// (D << 7)` (AV1_PROB_COST_SHIFT = 9, RDDIV_BITS = 7).
fn rdcost(lambda: u64, rate: u64, dist: u64) -> u64 {
    ((rate * lambda + 256) >> 9) + (dist << 7)
}

/// PD0 full lambda for an allintra key frame at 8-bit: C
/// `av1_lambda_assign_md` (md_process.c:744-770) =
/// `svt_aom_compute_rd_mult` — `(int64)((3.3 + 0.0015*dc_q) * dc_q *
/// dc_q)` with dc_q = dc_quant_qtx(qindex) (rc_process.c:452,
/// def_kf_rd_multiplier), then `* rd_frame_type_factor[0][KF]=150 >> 7`
/// (update_lambda; the stats-based factor is 128 at me_qindex ==
/// base_q_idx — I-slices always are, rc_aq.c:448) — times the
/// **frame `lambda_weight`** (`enc_mode_config.c:13502`, tune PSNR,
/// enc_mode > MR): 0 below CLI qp 16, 150 for qp 16..55, 175 for
/// qp >= 56 on I-slices (the 300 tier is `!is_islice` only), `>> 7`.
/// `lambda_scale_factors` stay 128 (no-op). Verified against the
/// instrumented library: 25650/248207/1527856 at qindex 80/160/220
/// (CLI qp 20/40/55), intermediates 21888/211804/1303771.
pub(crate) fn kf_full_lambda_8bit(qindex: u8, cli_qp: u32) -> u32 {
    let dc_q = svtav1_dsp::quant_tables::DC_QLOOKUP_8[qindex as usize] as i64;
    let rdmult = ((3.3 + 0.0015 * dc_q as f64) * (dc_q as f64) * (dc_q as f64)) as i64;
    let mut lambda = ((rdmult * 150) >> 7) as u32;
    let lambda_weight: u32 = if cli_qp >= 56 {
        175
    } else if cli_qp >= 16 {
        150
    } else {
        0
    };
    if lambda_weight != 0 {
        lambda = ((lambda as u64 * lambda_weight as u64) >> 7) as u32;
    }
    lambda
}

// ---------------------------------------------------------------------------
// Depth-set cap + PD0-level detector
// ---------------------------------------------------------------------------

/// C `get_max_block_size_allintra` (enc_mode_config.c:8969), effective
/// enc_mode >= M8 branch (`base_var_th_cap = 7500`; presets <= M7 use no
/// cap): 32 when the SB 64x64 variance exceeds the qp-scaled cap.
pub(crate) fn max_block_size_allintra(var64: u16, qp: u32) -> usize {
    let (qw, qwd) = qp_th_scaling_factors(qp);
    let var_th_cap = divide_and_round(7500 * qw as u64, qwd as u64) as u16;
    if var64 <= var_th_cap {
        64
    } else {
        32
    }
}

/// C `pd0_detector_allintra` (enc_dec_process.c:2373): demote PD0_LVL_6 to
/// PD0_LVL_5 when no depth dominates the variance profile.
pub(crate) fn pd0_detector_allintra_demotes(vars: &SbVariance, qp: u32) -> bool {
    let v = &vars.0;
    let var64 = v[0] as i32;
    let var32 = ((v[1] as i32 + v[2] as i32 + v[3] as i32 + v[4] as i32) >> 2) * 4;
    let var16 = ((v[5..21].iter().map(|&x| x as i32).sum::<i32>()) >> 4) * 16;
    let (qw, qwd) = qp_th_scaling_factors(qp);
    let th = divide_and_round(7500 * qw as u64, qwd as u64) as i32;
    (var32 - var64).abs() < th && (var16 - var32).abs() < th
}

// ---------------------------------------------------------------------------
// PD0_LVL_6 block cost (compute_lpd0_cost_allintra)
// ---------------------------------------------------------------------------

/// C `compute_lpd0_cost_allintra` (product_coding_loop.c:8418).
pub(crate) fn lvl6_cost_allintra(vars: &SbVariance, sq_size: usize, org_x: usize, org_y: usize, qp: u32) -> u64 {
    let (qw, qwd) = qp_th_scaling_factors(qp);
    let (qw, qwd) = (qw as u64, qwd as u64);
    let (blk_idx, sub_idx) = blk_var_map(sq_size, org_x, org_y);
    let blk_var = vars.0[blk_idx] as u64;
    let area = (sq_size * sq_size) as u64;
    let mut bias = 1000u64;
    if sq_size == 64 {
        let abs_th = divide_and_round(100 * qw, qwd);
        bias += 50 * (blk_var / abs_th).min(10);
    } else if sq_size >= 16 {
        let mut min_var = u64::MAX;
        let mut max_var = 0u64;
        for &si in &sub_idx {
            let v = vars.0[si] as u64;
            min_var = min_var.min(v);
            max_var = max_var.max(v);
        }
        let spread = max_var - min_var;
        let abs_th = divide_and_round(400 * qw, qwd);
        bias += 25 * (blk_var / abs_th).min(10);
        let peak_th = divide_and_round(25 * qw, qwd);
        bias += 10 * (spread / peak_th).min(10);
    } else {
        let abs_th = divide_and_round(25 * qw, qwd);
        bias += 40 * (blk_var / abs_th).min(10);
    }
    (area * bias) / 1000
}

// ---------------------------------------------------------------------------
// PD0_LVL_5 block cost (md_encode_block_pd0 full path)
// ---------------------------------------------------------------------------

/// One row of the C `Quants`/`Dequants` tables for a single qindex,
/// luma, 8-bit: C `svt_av1_build_quantizer` (md_config_process.c:97) with
/// all delta-q 0 and sharpness 0 (`qzbin_factor` per
/// `svt_aom_get_qzbin_factor`, `qrounding_factor = 48` for q > 0).
/// `[0]` = DC, `[1]` = AC.
struct QuantEntry {
    zbin: [i32; 2],
    round: [i32; 2],
    quant: [i32; 2],
    quant_shift: [i32; 2],
    dequant: [i32; 2],
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

fn build_quant_entry(qindex: u8) -> QuantEntry {
    let q = qindex as usize;
    let dc = svtav1_dsp::quant_tables::DC_QLOOKUP_8[q] as i32;
    let ac = svtav1_dsp::quant_tables::AC_QLOOKUP_8[q] as i32;
    // svt_aom_get_qzbin_factor (inv_transforms.c:3492), 8-bit.
    let qzbin_factor = if q == 0 { 64 } else if dc < 148 { 84 } else { 80 };
    let qrounding_factor = if q == 0 { 64 } else { 48 };
    let mut e = QuantEntry {
        zbin: [0; 2],
        round: [0; 2],
        quant: [0; 2],
        quant_shift: [0; 2],
        dequant: [0; 2],
    };
    for (i, quant_qtx) in [dc, ac].into_iter().enumerate() {
        let (quant, shift) = invert_quant(quant_qtx);
        e.quant[i] = quant;
        e.quant_shift[i] = shift;
        e.zbin[i] = (qzbin_factor * quant_qtx + 64) >> 7; // ROUND_POWER_OF_TWO(x, 7)
        e.round[i] = (qrounding_factor * quant_qtx) >> 7;
        e.dequant[i] = quant_qtx;
    }
    e
}

/// C `av1_get_tx_scale_tab[TX_SIZES_ALL]` (full_loop.c:22), indexed by the
/// C TxSize value.
const TX_SCALE_TAB: [i32; 19] = [0, 0, 0, 1, 2, 0, 0, 0, 0, 1, 1, 2, 2, 0, 0, 0, 0, 1, 1];

/// C `svt_aom_quantize_b_c` (full_loop.c:31) without quant matrices
/// (`q_matrix == NULL`): returns (eob, packed qcoeff, packed dqcoeff).
/// `coeffs` is the packed coefficient buffer (row stride = packed width),
/// `scan` the DCT_DCT scan for the tx size, `log_scale` = tx scale.
fn quantize_b(
    coeffs: &[i32],
    scan: &[u16],
    e: &QuantEntry,
    log_scale: i32,
) -> (u16, Vec<i32>, Vec<i32>) {
    let n_coeffs = scan.len();
    let zbins = [
        (e.zbin[0] + ((1 << log_scale) >> 1)) >> log_scale,
        (e.zbin[1] + ((1 << log_scale) >> 1)) >> log_scale,
    ];
    let mut qcoeff = vec![0i32; coeffs.len()];
    let mut dqcoeff = vec![0i32; coeffs.len()];

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
            let round = (e.round[iz] + ((1 << log_scale) >> 1)) >> log_scale;
            let tmp = (abs_coeff + round).clamp(i16::MIN as i32, i16::MAX as i32) as i64;
            let tmp32 = (((((tmp * e.quant[iz] as i64) >> 16) + tmp) * e.quant_shift[iz] as i64)
                >> (16 - log_scale)) as i32;
            qcoeff[rc] = (tmp32 ^ coeff_sign) - coeff_sign;
            let abs_dq = ((tmp32 as i64 * e.dequant[iz] as i64) >> log_scale) as i32;
            dqcoeff[rc] = (abs_dq ^ coeff_sign) - coeff_sign;
            if tmp32 != 0 {
                eob = i as i64;
            }
        }
    }
    ((eob + 1) as u16, qcoeff, dqcoeff)
}

/// C `energy_computation` (transforms.c:3095): sum of squared
/// coefficients over an area.
fn energy(coeff: &[i32], stride: usize, w: usize, h: usize) -> u64 {
    let mut e = 0u64;
    for r in 0..h {
        for c in 0..w {
            let v = coeff[r * stride + c] as i64;
            e += (v * v) as u64;
        }
    }
    e
}

/// The per-block LVL_5 transform pipeline (perform_tx_pd0): forward
/// DCT_DCT at the (possibly subres-halved) max-square tx size, 64-dim
/// energy fold + pack (svt_handle_transform64x64/64x32), quantize at
/// `qindex + 8`, frequency-domain SSE + three_quad_energy, dist shift,
/// and the closed-form coefficient rate.
///
/// Returns (eob, dist, bits).
fn lvl5_tx_cost(residual: &[i32], sq_size: usize, tx_h: usize, qindex_off: u8, subres_step: u32) -> (u16, u64, u64) {
    use svtav1_types::transform::{TxSize, TxType};
    // tx size after the subres remap (perform_tx_pd0): the residual is
    // sq_size x tx_h with tx_h = sq_size >> subres_step.
    let (tx_size, c_tx_size) = match (sq_size, tx_h) {
        (64, 64) => (TxSize::Tx64x64, 4usize),
        (64, 32) => (TxSize::Tx64x32, 12),
        (32, 32) => (TxSize::Tx32x32, 3),
        (32, 16) => (TxSize::Tx32x16, 10),
        (16, 16) => (TxSize::Tx16x16, 2),
        (16, 8) => (TxSize::Tx16x8, 8),
        (8, 8) => (TxSize::Tx8x8, 1),
        (8, 4) => (TxSize::Tx8x4, 6),
        _ => unreachable!("PD0 tx {}x{}", sq_size, tx_h),
    };

    let mut coeffs = vec![0i32; sq_size * tx_h];
    svtav1_dsp::txfm_dispatch::fwd_txfm2d_dispatch(residual, &mut coeffs, sq_size, tx_size, TxType::DctDct);

    // 64-dim fold + pack (svt_handle_transform64x64 / 64x32).
    let mut three_quad_energy = 0u64;
    if sq_size == 64 {
        if tx_h == 64 {
            three_quad_energy = energy(&coeffs[32..], 64, 32, 32) + energy(&coeffs[32 * 64..], 64, 64, 32);
        } else {
            three_quad_energy = energy(&coeffs[32..], 64, 32, 32);
        }
        let pack_h = tx_h.min(32);
        for row in 1..pack_h {
            for c in 0..32 {
                coeffs[row * 32 + c] = coeffs[row * 64 + c];
            }
        }
        coeffs.truncate(32 * pack_h);
    }

    let packed_w = sq_size.min(32);
    let packed_h = tx_h.min(32);
    let log_scale = TX_SCALE_TAB[c_tx_size];
    let entry = build_quant_entry(qindex_off);
    let scan = svtav1_entropy::scan_tables::scan(c_tx_size, 0);
    debug_assert_eq!(scan.len(), packed_w * packed_h);
    let (eob, _qcoeff, dqcoeff) = quantize_b(&coeffs, scan, &entry, log_scale);

    // svt_aom_picture_full_distortion32_bits_single: freq-domain SSE
    // (or plain coeff energy when eob == 0) over the packed region.
    let mut dist = 0u64;
    if eob > 0 {
        for i in 0..coeffs.len() {
            let d = (coeffs[i] - dqcoeff[i]) as i64;
            dist += (d * d) as u64;
        }
    } else {
        dist = energy(&coeffs, packed_w, packed_w, packed_h);
    }
    dist += three_quad_energy;
    // RIGHT_SIGNED_SHIFT(dist, (MAX_TX_SCALE=1 - tx_scale) * 2) << subres
    let shift = (1 - log_scale) * 2;
    dist = if shift < 0 { dist << (-shift) } else { dist >> shift };
    dist <<= subres_step;

    // coeff_rate_est_lvl == 0 closed form (perform_tx_pd0): input
    // resolution factor is 0 for every <= 240p-range picture (all
    // identity/gate frames).
    let bits = 5000 + 100 * eob as u64;
    (eob, dist, bits)
}

/// C `check_is_subres_safe` (product_coding_loop.c): SAD of even vs odd
/// rows of (src - pred) over the 64x64; safe iff the deviation is within
/// `odd_to_even_deviation_th = 5` percent.
fn check_is_subres_safe(src: &[u8], stride: usize, org_x: usize, org_y: usize, pred: &[u8]) -> bool {
    let mut sad_even = 0i64;
    let mut sad_odd = 0i64;
    for r in 0..64 {
        let srow = (org_y + r) * stride + org_x;
        for c in 0..64 {
            let d = (src[srow + c] as i32 - pred[r * 64 + c] as i32).abs() as i64;
            if r % 2 == 0 {
                sad_even += d;
            } else {
                sad_odd += d;
            }
        }
    }
    let deviation = ((sad_even.max(1) - sad_odd.max(1)) * 100) / sad_odd.max(1);
    deviation.abs() <= 5
}

// ---------------------------------------------------------------------------
// The PD0 walk
// ---------------------------------------------------------------------------

/// PD0-picked square partition tree: leaves carry the block size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pd0Tree {
    Leaf(usize),
    Split(Box<[Pd0Tree; 4]>),
}

impl Pd0Tree {
    /// Leaf sizes in raster/coding order (debug aid).
    pub fn leaf_sizes(&self) -> Vec<usize> {
        match self {
            Pd0Tree::Leaf(s) => vec![*s],
            Pd0Tree::Split(ch) => ch.iter().flat_map(|c| c.leaf_sizes()).collect(),
        }
    }
}

struct Pd0Ctx<'a> {
    src: &'a [u8],
    stride: usize,
    sb_x: usize,
    sb_y: usize,
    vars: SbVariance,
    qp: u32,
    qindex: u8,
    lambda: u64,
    level5: bool,
    max_sq: usize,
    min_sq: usize,
    /// C `ctx->is_subres_safe`: 255 = not yet determined (only a tested
    /// 64x64 block determines it); the effective per-block step is 0
    /// unless this is exactly 1.
    is_subres_safe: u8,
}

/// C `svt_aom_partition_rate_cost` at PD0: neighbor partition contexts are
/// 0 (never updated in PD0), `has_rows`/`has_cols` are true for the fully
/// in-picture blocks every current caller produces. Units: 1/512 bit.
fn partition_split_bits(sq_size: usize) -> u64 {
    svtav1_entropy::context::partition_symbol_cost(sq_size, 0, crate::partition::PartitionType::Split as usize) as u64
}

/// C `partition_fac_bits[0][PARTITION_NONE]`: svt_aom_full_cost_pd0 uses
/// **context index 0** — the bsl-0 (8x8 size class), sub-context-0 row —
/// as an approximation for every block size (rd_cost.c:1344-1349). 400
/// units of 1/512 bit from the default tables.
fn partition_none_bits_ctx0() -> u64 {
    svtav1_entropy::context::partition_symbol_cost(8, 0, crate::partition::PartitionType::None as usize) as u64
}

/// C `skip_fac_bits[0][0]` — cost of skip=0 at context 0 from the default
/// skip CDF (icdf 1097 -> p(0) = 31671): 26 units of 1/512 bit.
fn skip0_bits() -> u64 {
    svtav1_entropy::context::av1_cost_symbol(32768 - 1097) as u64
}

impl<'a> Pd0Ctx<'a> {
    /// LVL_5 block cost (md_encode_block_pd0 full path). Also runs the
    /// per-SB subres-safety check when this is a 64x64 block and the
    /// safety is still undetermined (full_loop_core_pd0).
    fn lvl5_block_cost(&mut self, sq_size: usize, org_x: usize, org_y: usize) -> u64 {
        let abs_x = self.sb_x + org_x;
        let abs_y = self.sb_y + org_y;
        // DC prediction from SOURCE neighbors (pd0_use_src_samples=1):
        // the same spec unavailable-edge fills as the recon path.
        let (above, left, _tl, has_above, has_left) =
            crate::partition::extract_neighbors(self.src, self.stride, abs_x, abs_y, sq_size, sq_size);
        let mut pred = vec![0u8; sq_size * sq_size];
        svtav1_dsp::intra_pred::predict_dc(&mut pred, sq_size, &above, &left, sq_size, sq_size, has_above, has_left);

        // Subres safety: determined once per SB by the first (and only)
        // tested 64x64 block; blocks tested while it is undetermined use
        // step 0 (C forces mds_subres_step = 0 when is_subres_safe != 1).
        if sq_size == 64 && self.is_subres_safe == 255 {
            self.is_subres_safe = u8::from(check_is_subres_safe(self.src, self.stride, abs_x, abs_y, &pred));
        }
        // subres_ctrls.step = 1 for this config; 8x8 caps at min(1, step).
        let step_cfg = 1u32;
        let mut step = if sq_size >= 16 { step_cfg } else { step_cfg.min(1) };
        if self.is_subres_safe != 1 {
            step = 0;
        }

        let tx_h = sq_size >> step;
        let mut residual = vec![0i32; sq_size * tx_h];
        for r in 0..tx_h {
            let srow = (abs_y + (r << step)) * self.stride + abs_x;
            let prow = (r << step) * sq_size;
            for c in 0..sq_size {
                residual[r * sq_size + c] = self.src[srow + c] as i32 - pred[prow + c] as i32;
            }
        }
        let qindex_off = (self.qindex as u32 + 8).min(255) as u8; // lpd0_qp_offset = 8
        let (eob, dist, bits) = lvl5_tx_cost(&residual, sq_size, tx_h, qindex_off, step);
        let _ = eob;
        // svt_aom_full_cost_pd0: rate = coeff bits + skip(0) bits +
        // PARTITION_NONE bits at context 0.
        let rate = bits + skip0_bits() + partition_none_bits_ctx0();
        rdcost(self.lambda, rate, dist)
    }

    fn block_cost(&mut self, sq_size: usize, org_x: usize, org_y: usize) -> u64 {
        if self.level5 {
            self.lvl5_block_cost(sq_size, org_x, org_y)
        } else {
            lvl6_cost_allintra(&self.vars, sq_size, org_x, org_y, self.qp)
        }
    }

    /// C `svt_aom_pick_partition_pd0` + `test_split_partition_pd0`:
    /// parent-first DFS returning (cost, tree) for this square node.
    fn pick(&mut self, sq_size: usize, org_x: usize, org_y: usize) -> (u64, Pd0Tree) {
        let tested = sq_size <= self.max_sq && sq_size >= self.min_sq;
        let parent_cost = if tested {
            Some(self.block_cost(sq_size, org_x, org_y))
        } else {
            None
        };

        let split_flag = sq_size > self.min_sq;
        if !split_flag {
            let cost = parent_cost.expect("leaf must be tested (min_sq <= size <= max_sq)");
            return (cost, Pd0Tree::Leaf(sq_size));
        }

        // test_split_partition_pd0: split rate term (0 at LVL_6 allintra;
        // doubled at LVL_5 because use_accurate_part_ctx = 0 at eff-M9).
        let mut split_cost = if self.level5 {
            rdcost(self.lambda, 2 * partition_split_bits(sq_size), 0)
        } else {
            0
        };

        let half = sq_size / 2;
        let mut children: Vec<Pd0Tree> = Vec::with_capacity(4);
        let mut split_valid = true;
        for i in 0..4 {
            let cx = org_x + (i & 1) * half;
            let cy = org_y + (i >> 1) * half;
            // LVL_5-only early exits (disabled entirely for allintra
            // LVL_6): th = split_cost_th(50) for i == 0, else
            // early_exit_th(0 -> 1000); parent_cost_bias = 1000.
            if self.level5 {
                if let Some(pc) = parent_cost {
                    let th: u128 = if i == 0 { 50 } else { 1000 };
                    if (pc as u128) * th * 1000 <= (split_cost as u128) * 1_000_000 {
                        split_valid = false;
                        break;
                    }
                }
            }
            let (child_cost, child_tree) = self.pick(half, cx, cy);
            split_cost += child_cost;
            children.push(child_tree);
        }

        if !split_valid {
            let cost = parent_cost.expect("early exit requires a valid parent");
            return (cost, Pd0Tree::Leaf(sq_size));
        }

        // parent_cost_bias = 1000 (allintra): parent wins on <=.
        if let Some(pc) = parent_cost {
            if pc * 1000 <= split_cost * 1000 {
                return (pc, Pd0Tree::Leaf(sq_size));
            }
        }
        let ch: [Pd0Tree; 4] = children.try_into().expect("4 children");
        (split_cost, Pd0Tree::Split(Box::new(ch)))
    }
}

/// Decide the partition tree of one 64x64 superblock exactly like the C
/// PD0 pass at allintra effective-M9 (CLI preset >= 9).
///
/// `src` is the full luma plane (64-aligned frame, the caller's padding
/// convention), `qp` the CLI 0..63 qp, `qindex` the frame base_q_idx.
pub fn pd0_pick_sb_partition(
    src: &[u8],
    stride: usize,
    sb_x: usize,
    sb_y: usize,
    qp: u32,
    qindex: u8,
) -> Pd0Tree {
    let vars = compute_b64_variance(src, stride, sb_x, sb_y);
    let max_sq = max_block_size_allintra(vars.0[0], qp);
    let level5 = pd0_detector_allintra_demotes(&vars, qp);
    let lambda = kf_full_lambda_8bit(qindex, qp) as u64;
    let mut ctx = Pd0Ctx {
        src,
        stride,
        sb_x,
        sb_y,
        vars,
        qp,
        qindex,
        lambda,
        level5,
        max_sq,
        // disallow_4x4 = 1 (pic_disallow_4x4 for these presets),
        // disallow_8x8_allintra() = false, no depth removal flags.
        min_sq: 8,
        is_subres_safe: 255,
    };
    let (_cost, tree) = ctx.pick(64, 0, 0);
    tree
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The identity-harness gradient content (identity_run.rs).
    fn gradient64() -> Vec<u8> {
        let (w, h) = (64usize, 64usize);
        let mut y = vec![0u8; w * h];
        for r in 0..h {
            for c in 0..w {
                y[r * w + c] = (((r * 255) / h) ^ ((c * 3) & 0x3f)) as u8;
            }
        }
        y
    }

    /// C variance map for gradient-64, captured from the instrumented
    /// library (MDBG sb_var, docs/IDENTITY-STATUS.md 2026-07-13).
    const C_GRADIENT64_VARS: [u16; 85] = [
        5425, 1343, 1353, 1733, 1893, 336, 341, 340, 338, 645, 773, 837, 901, 645, 773, 837, 901,
        645, 773, 837, 901, 79, 163, 395, 83, 79, 487, 155, 83, 197, 503, 181, 325, 357, 171, 469,
        229, 197, 1099, 1717, 325, 1573, 1047, 661, 1957, 197, 503, 181, 325, 357, 171, 469, 229,
        197, 1099, 1717, 325, 1573, 1047, 661, 1957, 197, 503, 181, 325, 357, 171, 469, 229, 197,
        1099, 1717, 325, 1573, 1047, 661, 1957, 197, 503, 181, 325, 357, 171, 469, 229,
    ];

    #[test]
    fn variance_map_matches_c() {
        let y = gradient64();
        let v = compute_b64_variance(&y, 64, 0, 0);
        assert_eq!(v.0, C_GRADIENT64_VARS);
    }

    #[test]
    fn qp_scaling_factors_match_c() {
        // Linear branch (MDBG qw prints) + the exp branch at qp 55
        // (9146/10000, from the observed var cap and detector behavior).
        assert_eq!(qp_th_scaling_factors(20), (20, 63));
        assert_eq!(qp_th_scaling_factors(40), (40, 63));
        assert_eq!(qp_th_scaling_factors(55), (9146, 10000));
    }

    #[test]
    fn lambda_matches_c() {
        // MDBG split_enter lambda prints: qindex 80/160/220 (CLI qp
        // 20/40/55 through quantizer_to_qindex).
        assert_eq!(kf_full_lambda_8bit(80, 20), 25650);
        assert_eq!(kf_full_lambda_8bit(160, 40), 248207);
        assert_eq!(kf_full_lambda_8bit(220, 55), 1527856);
    }

    #[test]
    fn rate_constants_match_c() {
        // MDBG pd0_cand: skip_fac_bits[0][0]=26, partition_fac_bits[0][NONE]=400;
        // split_enter above_split_rate (post-double): 2390@64, 2930@32, 4040@16.
        assert_eq!(skip0_bits(), 26);
        assert_eq!(partition_none_bits_ctx0(), 400);
        assert_eq!(2 * partition_split_bits(64), 2390);
        assert_eq!(2 * partition_split_bits(32), 2930);
        assert_eq!(2 * partition_split_bits(16), 4040);
    }

    #[test]
    fn max_block_size_and_detector_match_c() {
        let y = gradient64();
        let v = compute_b64_variance(&y, 64, 0, 0);
        // MDBG: 64x64 depth excluded at q20/q40 (max 32), included at q55.
        assert_eq!(max_block_size_allintra(v.0[0], 20), 32);
        assert_eq!(max_block_size_allintra(v.0[0], 40), 32);
        assert_eq!(max_block_size_allintra(v.0[0], 55), 64);
        // MDBG: pd0_level 6 at q20, demoted to 5 at q40/q55.
        assert!(!pd0_detector_allintra_demotes(&v, 20));
        assert!(pd0_detector_allintra_demotes(&v, 40));
        assert!(pd0_detector_allintra_demotes(&v, 55));
        // Uniform content: all-zero variance map always demotes.
        let u = vec![128u8; 64 * 64];
        let vu = compute_b64_variance(&u, 64, 0, 0);
        assert_eq!(vu.0, [0u16; 85]);
        assert!(pd0_detector_allintra_demotes(&vu, 40));
        assert_eq!(max_block_size_allintra(0, 20), 64);
    }

    #[test]
    fn lvl6_costs_match_c() {
        // MDBG vlpd0cost lines, gradient-64 q20 (PD0_LVL_6).
        let y = gradient64();
        let v = compute_b64_variance(&y, 64, 0, 0);
        for (sq, ox, oy, cost) in [
            (32usize, 0usize, 0usize, 1382u64),
            (16, 0, 0, 294),
            (8, 0, 0, 87),
            (8, 8, 0, 89),
            (8, 0, 8, 89),
            (8, 8, 8, 89),
            (16, 16, 0, 294),
            (16, 0, 16, 313),
            (16, 16, 16, 320),
            (32, 32, 0, 1382),
        ] {
            assert_eq!(lvl6_cost_allintra(&v, sq, ox, oy, 20), cost, "sq={sq} ({ox},{oy})");
        }
    }

    #[test]
    fn lvl5_block_costs_match_c_q40() {
        // MDBG pd0_full_cost / tx_pd0_out, gradient-64 q40 (qindex 160,
        // PD0_LVL_5, subres forced off: no 64x64 block in the depth set).
        let y = gradient64();
        let mut ctx = Pd0Ctx {
            src: &y,
            stride: 64,
            sb_x: 0,
            sb_y: 0,
            vars: compute_b64_variance(&y, 64, 0, 0),
            qp: 40,
            qindex: 160,
            lambda: kf_full_lambda_8bit(160, 40) as u64,
            level5: true,
            max_sq: 32,
            min_sq: 8,
            is_subres_safe: 255,
        };
        for (sq, ox, oy, cost) in [
            (32usize, 0usize, 0usize, 187677438u64),
            (16, 0, 0, 48981821),
            (8, 0, 0, 9695714),
            (8, 8, 0, 11371661),
            (8, 0, 8, 16542374),
            (8, 8, 8, 20538852),
            (16, 16, 0, 41852989),
            (32, 32, 0, 190877950),
            (32, 0, 32, 181407102),
            (32, 32, 32, 183892222),
            (16, 48, 16, 53455823),
        ] {
            assert_eq!(ctx.lvl5_block_cost(sq, ox, oy), cost, "sq={sq} ({ox},{oy})");
        }
    }

    #[test]
    fn lvl5_block_costs_match_c_q55_with_subres() {
        // MDBG, gradient-64 q55 (qindex 220): the 64x64 block runs the
        // odd/even check (safe=1) and everything uses subres step 1.
        let y = gradient64();
        let mut ctx = Pd0Ctx {
            src: &y,
            stride: 64,
            sb_x: 0,
            sb_y: 0,
            vars: compute_b64_variance(&y, 64, 0, 0),
            qp: 55,
            qindex: 220,
            lambda: kf_full_lambda_8bit(220, 55) as u64,
            level5: true,
            max_sq: 64,
            min_sq: 8,
            is_subres_safe: 255,
        };
        assert_eq!(ctx.lvl5_block_cost(64, 0, 0), 1708208432);
        assert_eq!(ctx.is_subres_safe, 1, "64x64 DC pred must pass the odd/even check");
        for (sq, ox, oy, cost) in [
            (32usize, 0usize, 0usize, 522128378u64),
            (16, 0, 0, 137213980),
            (16, 16, 0, 135635996),
            (16, 0, 16, 232128024),
            (16, 16, 16, 194500372),
            (32, 32, 0, 594523898),
            (32, 0, 32, 475114621),
            (32, 32, 32, 469165693),
        ] {
            assert_eq!(ctx.lvl5_block_cost(sq, ox, oy), cost, "sq={sq} ({ox},{oy})");
        }
    }

    #[test]
    fn gradient64_trees_match_c() {
        let y = gradient64();
        // q20 (qindex 80): LVL_6, max 32 -> forced SPLIT at 64, every 32
        // SPLITs again, 16x16 leaves everywhere (C stream: op0 SPLIT,
        // op1 SPLIT, op2 NONE...).
        let t20 = pd0_pick_sb_partition(&y, 64, 0, 0, 20, 80);
        assert_eq!(t20.leaf_sizes(), vec![16; 16]);
        // q40 (qindex 160): LVL_5, max 32 -> forced SPLIT at 64, all four
        // 32x32 keep PARENT (C: op0 SPLIT, op1 NONE).
        let t40 = pd0_pick_sb_partition(&y, 64, 0, 0, 40, 160);
        assert_eq!(t40.leaf_sizes(), vec![32; 4]);
        // q55 (qindex 220): LVL_5, 64 in set and PARENT wins outright.
        let t55 = pd0_pick_sb_partition(&y, 64, 0, 0, 55, 220);
        assert_eq!(t55, Pd0Tree::Leaf(64));
        // Uniform: LVL_5 with zero residual everywhere -> 64x64 NONE.
        let u = vec![128u8; 64 * 64];
        let tu = pd0_pick_sb_partition(&u, 64, 0, 0, 40, 160);
        assert_eq!(tu, Pd0Tree::Leaf(64));
    }
}
