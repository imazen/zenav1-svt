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
#[derive(Clone, Copy)]
pub struct SbVariance(pub [u16; 85]);

/// C `compute_b64_variance` at `BLOCK_MEAN_PREC_SUB` (the default,
/// enc_handle.c:4618): 8x8 means/mean-squares from the EVEN rows only
/// (rows 0,2,4,6 of each 8x8), `mean << 3` / `mean_sq << 11` fixed point,
/// averaged up the quad tree with `>> 2`, variance = `(msq - m*m) >> 16`
/// cast to u16 per level.
///
/// The source region must be fully inside the picture (the C input is the
/// edge-padded `input_padded_pic`; every current caller pads frames to
/// 64-aligned dimensions so the region is always complete).
pub(crate) fn compute_b64_variance(
    src: &[u8],
    stride: usize,
    org_x: usize,
    org_y: usize,
) -> SbVariance {
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
    let blk_idx =
        BASE_LUT[lvl] + (org_y >> LOG2_LUT[lvl]) * GRID_LUT[lvl] + (org_x >> LOG2_LUT[lvl]);
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
/// The kf full lambda WITHOUT the frame `lambda_weight` multiply — what C's
/// `svt_aom_lambda_assign` hands the CDEF search (enc_cdef.c:991) and the
/// restoration search rdmult. Instrumented: 21888 / 211804 / 1303771 at
/// qindex 80/160/220 (= kf_full_lambda_8bit * 128 / 150 exactly).
pub(crate) fn kf_full_lambda_8bit_unweighted(qindex: u8) -> u32 {
    let dc_q = svtav1_dsp::quant_tables::DC_QLOOKUP_8[qindex as usize] as i64;
    let rdmult = ((3.3 + 0.0015 * dc_q as f64) * (dc_q as f64) * (dc_q as f64)) as i64;
    ((rdmult * 150) >> 7) as u32
}

/// Only the tests reach this now: every production caller resolves the frame
/// `lambda_weight` with [`frame_lambda_weight`] and goes through
/// [`kf_full_lambda_8bit_lw`] or [`kf_full_lambda_8bit_tuned`].
#[cfg(test)]
pub(crate) fn kf_full_lambda_8bit(qindex: u8, picture_qp: u32) -> u32 {
    kf_full_lambda_8bit_ex(qindex, picture_qp, false, 0)
}

/// C's frame `lambda_weight` for an all-intra still —
/// `svt_aom_sig_deriv_mode_decision_config_allintra`
/// (enc_mode_config.c:10093-10115), the ONE frame-level factor every MD
/// lambda is scaled by (`av1_lambda_assign_md`, md_process.c:747-751):
///
/// * tune IQ -> the still-picture curve `CLIP3(0, 72, MIN(pq*4, (63-pq)*3))
///   + 128` (:10099). It is C's `if` arm, so it REPLACES the PSNR ladder.
/// * otherwise -> 0 below 16, 150 for 16..=55, 175 at >= 56 (:10101-10107;
///   C's `!(enc_mode <= ENC_MR)` guard is always true here because `ENC_MR`
///   is unreachable from a `u8` preset).
/// * then, for the EXTENDED CRF range ONLY (`static_config.qp == 63` with a
///   non-zero `extended_crf_qindex_offset`, i.e. CRF 63.25..70),
///   `+= extended_crf_qindex_offset * 28` (:10109-10114).
///
/// The qp this keys on is `ppcs->picture_qp = clamp_qp((base_q_idx + 2) >> 2)`
/// (rc_process.c:861) — re-derived from the (possibly fractional-CRF-offset)
/// qindex — NOT `static_config.qp`, which every qp-keyed LEVEL derivation
/// reads instead. The two are equal whenever the CRF offset is 0.
pub(crate) fn frame_lambda_weight(picture_qp: u32, tune_iq: bool, extended_crf_bump: u32) -> u32 {
    let ladder = if tune_iq {
        crate::tune::iq_lambda_weight(picture_qp)
    } else if picture_qp >= 56 {
        175
    } else if picture_qp >= 16 {
        150
    } else {
        0
    };
    ladder + extended_crf_bump
}

/// [`kf_full_lambda_8bit`] with the frame `lambda_weight` supplied directly
/// (already resolved by [`frame_lambda_weight`]) instead of re-derived from a
/// qp. Used wherever the caller knows the frame weight — which is the only way
/// the extended-CRF bump and the tune-IQ curve can reach a per-SB lambda.
pub(crate) fn kf_full_lambda_8bit_lw(qindex: u8, lambda_weight: u32) -> u32 {
    let dc_q = svtav1_dsp::quant_tables::DC_QLOOKUP_8[qindex as usize] as i64;
    let rdmult = ((3.3 + 0.0015 * dc_q as f64) * (dc_q as f64) * (dc_q as f64)) as i64;
    let mut lambda = ((rdmult * 150) >> 7) as u32;
    if lambda_weight != 0 {
        lambda = ((u64::from(lambda) * u64::from(lambda_weight)) >> 7) as u32;
    }
    lambda
}

/// [SVT_HDR_MODE] full form of the KF lambda chain (C `update_lambda`,
/// rc_process.c:401):
/// * `alt_lambda_factors` (fork default 1) swaps the KF frame-type factor
///   150 -> `rd_frame_type_factor_alt[KF_UPDATE]` = 140 (rc_process.c:398).
/// * With per-SB delta-q present, the stats-based SB factor is no longer
///   the 128 no-op: `qdiff = q_index - base_q_idx` picks {<=-8: 90,
///   <0: 115, <=8 above: 135, >8: 150} (rc_process.c:437-446). The frame
///   `lambda_weight` multiply follows, as in C's av1_lambda_assign_md.
pub(crate) fn kf_full_lambda_8bit_ex(
    qindex: u8,
    picture_qp: u32,
    alt_lambda_factors: bool,
    qdiff_vs_base: i32,
) -> u32 {
    kf_full_lambda_8bit_tuned(qindex, picture_qp, alt_lambda_factors, qdiff_vs_base, None)
}

/// [SVT_HDR_MODE] full form incl. the TUNE_IQ still-picture
/// `lambda_weight` curve (enc_mode_config.c:13513) — when Some, it
/// REPLACES the PSNR 0/150/175 ladder entirely (C sets pcs->lambda_weight
/// from the tune before the ladder ever runs).
pub(crate) fn kf_full_lambda_8bit_tuned(
    qindex: u8,
    picture_qp: u32,
    alt_lambda_factors: bool,
    qdiff_vs_base: i32,
    lambda_weight_override: Option<u32>,
) -> u32 {
    let dc_q = svtav1_dsp::quant_tables::DC_QLOOKUP_8[qindex as usize] as i64;
    let rdmult = ((3.3 + 0.0015 * dc_q as f64) * (dc_q as f64) * (dc_q as f64)) as i64;
    let ftf: i64 = if alt_lambda_factors { 140 } else { 150 };
    let mut rdmult = (rdmult * ftf) >> 7;
    let stats_factor: i64 = if qdiff_vs_base < 0 {
        if qdiff_vs_base <= -8 { 90 } else { 115 }
    } else if qdiff_vs_base > 0 {
        if qdiff_vs_base <= 8 { 135 } else { 150 }
    } else {
        128
    };
    rdmult = (rdmult * stats_factor) >> 7;
    let mut lambda = rdmult as u32;
    let lambda_weight: u32 =
        lambda_weight_override.unwrap_or_else(|| frame_lambda_weight(picture_qp, false, 0));
    if lambda_weight != 0 {
        lambda = ((lambda as u64 * lambda_weight as u64) >> 7) as u32;
    }
    lambda
}

/// C `rd_frame_type_factor[0]` (rc_process.c:395), the 8-bit row, indexed by
/// [`crate::port_rc_process::FrameUpdateType`].
const RD_FRAME_TYPE_FACTOR_8BIT: [i64; 7] = [150, 180, 150, 150, 180, 180, 150];
/// C `rd_frame_type_factor_alt` (rc_process.c:397).
const RD_FRAME_TYPE_FACTOR_ALT: [i64; 7] = [140, 180, 128, 140, 164, 164, 140];

/// The 8-bit full MD lambda for a NON-KEY frame — C
/// `svt_aom_compute_rd_mult` -> `update_lambda` (rc_process.c:365-449),
/// which `av1_lambda_assign_md` (md_process.c:725) calls.
///
/// It differs from [`kf_full_lambda_8bit_tuned`] in exactly two places, and
/// both are frame-type switches rather than new arithmetic:
///
/// * the rdmult BASE multiplier — `def_kf_rd_multiplier` is `3.3 + 0.0015 q`
///   (rc_process.c:361), `def_arf_rd_multiplier` `3.25 + …` (:354) and
///   `def_inter_rd_multiplier` `3.2 + …` (:347); `compute_rd_mult_based_on_
///   qindex` (:365) picks by `update_type`;
/// * the frame-type FACTOR row, `rd_frame_type_factor[bd != 8][update_type]`
///   (:417) or the `_alt` row when `alt_lambda_factors` is set (:415).
///
/// **The two switches read DIFFERENT update types, and conflating them is a
/// measured defect.** `svt_aom_compute_rd_mult_based_on_qindex` (:365) is
/// called with `ppcs->update_type` — the PICTURE's own type from
/// `set_frame_update_type` (pd_process.c:4591) — while `update_lambda` (:406)
/// derives its OWN `gf_update_type` from `frame_type` + `temporal_layer_index`
/// and indexes `rd_frame_type_factor` with THAT. For a flat low-delay P GOP
/// (`hierarchical_levels == 0`) they disagree: `set_frame_update_type` falls
/// through to the `frame_offset & 1 -> LF_UPDATE` arm (base `3.2`), while
/// `update_lambda` sees `temporal_layer_index == 0` and picks `ARF_UPDATE`
/// (factor 150).
///
/// MEASURED on `diag 64x64 q40 p8 frames=2`, frame 1 (`base_q_idx = 160`),
/// against C's own `svt_aom_full_cost_pd0` lambda argument
/// (`SVT_PD0COST_OUT`): C is **241 378**, which is base 3.2 x factor 150.
/// Passing one update type for both gave 244 792 (ARF base 3.25) — the value
/// `docs/INTER-ENCODE-PLAN.md` §1y recorded for the port — and the KF chain
/// would give 248 207.
///
/// `stats_based_sb_lambda_modulation`'s factor is the 128 no-op whenever
/// `q_index == base_q_idx` (:432-441), which is every frame this port emits
/// (no per-SB delta-q is signalled), so it is carried as `qdiff_vs_base`
/// exactly like the KF builder's.
pub(crate) fn inter_full_lambda_8bit(
    qindex: u8,
    // C `ppcs->update_type` (`set_frame_update_type`, pd_process.c:4591) —
    // the rdmult BASE multiplier's selector.
    base_update_type: crate::port_rc_process::FrameUpdateType,
    // C `gf_update_type` (`update_lambda`, rc_process.c:406) — the
    // `rd_frame_type_factor` row's selector.
    factor_update_type: crate::port_rc_process::FrameUpdateType,
    alt_lambda_factors: bool,
    qdiff_vs_base: i32,
    lambda_weight: u32,
) -> u32 {
    use crate::port_rc_process::FrameUpdateType as U;
    let q = svtav1_dsp::quant_tables::DC_QLOOKUP_8[qindex as usize] as f64;
    let base = match base_update_type {
        U::KfUpdate => 3.3,
        U::GfUpdate | U::ArfUpdate => 3.25,
        _ => 3.2,
    };
    let mut rdmult = ((base + 0.0015 * q) * q * q) as i64;
    let ut = factor_update_type as usize;
    rdmult = (rdmult
        * if alt_lambda_factors {
            RD_FRAME_TYPE_FACTOR_ALT[ut]
        } else {
            RD_FRAME_TYPE_FACTOR_8BIT[ut]
        })
        >> 7;
    // C `update_lambda`'s `stats_based_sb_lambda_modulation` block
    // (rc_process.c:423-446), FINAL `else` arm — the one an INTER frame in
    // this port takes, because `rtc` is false and neither `delta_q_present`
    // nor `r0_delta_qp_md` is set. `qdiff_vs_base` is C's
    // `me_q_index - base_q_idx`, NOT `q_index - base_q_idx`.
    //
    // CORRECTED 2026-09-02: this carried `kf_full_lambda_8bit_tuned`'s block
    // verbatim — the `delta_q_present` arm, thresholds +-8 with a low factor
    // of **90**. That arm is right for THAT function (its caller is the fork's
    // per-SB delta-q path, and C keys it on `q_index`); it is the wrong one
    // here, where the live arm is +-4 with a low factor of **100**. Inert
    // while every caller passes 0 — which they still do — and wrong the moment
    // a real per-superblock `me_q_index` arrives. Found while deriving where
    // C's per-SB `fastlam` comes from: the arithmetic, the C sites and the
    // verification cell are in
    // `benchmarks/pd0_depth_removal_join_2026-09-02.md`. The sweep in
    // `inter_lambda_tests` now drives this axis against
    // `port_rc_process::compute_rd_mult`, which is the tier-1-gated
    // transcription, instead of only the 128 no-op.
    let stats_factor: i64 = if qdiff_vs_base < 0 {
        if qdiff_vs_base <= -4 { 100 } else { 115 }
    } else if qdiff_vs_base > 0 {
        if qdiff_vs_base <= 4 { 135 } else { 150 }
    } else {
        128
    };
    rdmult = (rdmult * stats_factor) >> 7;
    let mut lambda = rdmult as u32;
    if lambda_weight != 0 {
        lambda = ((u64::from(lambda) * u64::from(lambda_weight)) >> 7) as u32;
    }
    lambda
}

/// KF full MD lambda at bd10 (C `full_lambda_md[1]`, md_process.c:725-759),
/// mainline still/allintra path. Task #94 (the u16 MD path): the bd10 lambda
/// is NOT `kf_full_lambda_8bit * 16` — the rdmult base is computed from the
/// bit-depth-specific DC quant and a different frame-type factor:
/// - `q = svt_aom_dc_quant_qtx(qindex, 0, 10)` = `dc_qlookup_10` (rc_process.c:366),
/// - `rdmult = (3.3 + 0.0015*q) * q * q`, then `ROUND_POWER_OF_TWO(rdmult, 4)`
///   for bd10 (rc_process.c:382),
/// - frame-type factor `rd_frame_type_factor[1][KF_UPDATE] = 128` at bd!=8
///   (rc_process.c:417 — a no-op ×128>>7, vs the 150 real scaling at bd8),
/// - then the same `lambda_weight` ladder and `full_lambda_md[1] *= 16`
///   (md_process.c:753). Intra-scaling (temporal_layer>0) and scale_factor
///   (128) are no-ops on the KF still path — same as the bd8 builder.
pub(crate) fn kf_full_lambda_bd10(qindex: u8, picture_qp: u32) -> u32 {
    let q = crate::bd10::dc_qlookup_10(qindex) as i64;
    let mut rdmult = ((3.3 + 0.0015 * q as f64) * q as f64 * q as f64) as i64;
    rdmult = (rdmult + 8) >> 4; // ROUND_POWER_OF_TWO(_, 4) — bd10
    rdmult = (rdmult * 128) >> 7; // rd_frame_type_factor[1][KF_UPDATE] = 128
    let mut lambda = rdmult as u32;
    let lambda_weight: u32 = frame_lambda_weight(picture_qp, false, 0);
    if lambda_weight != 0 {
        lambda = ((lambda as u64 * lambda_weight as u64) >> 7) as u32;
    }
    lambda * 16 // md_process.c:753 — full_lambda_md[1] *= 16 (2^(2*(10-8)))
}

/// bd10 twin of [`kf_full_lambda_8bit_unweighted`]: C
/// `svt_aom_compute_rd_mult(pcs, q, q, EB_TEN_BIT)` -> `update_lambda`
/// (rc_process.c:365-449) with NO `lambda_weight` ladder and NO `*= 16`.
///
/// This is `svt_aom_lambda_assign(.., EB_TEN_BIT, qidx, multiply_lambda =
/// false)`'s `full_lambda` — the CDEF search's lambda (enc_cdef.c:958-964,
/// which passes `enhanced_pic->bit_depth` and `false`). Chain:
/// * `q = svt_aom_dc_quant_qtx(qindex, 0, EB_TEN_BIT)` = `dc_qlookup_10`,
/// * `rdmult = (3.3 + 0.0015*q) * q * q` (`def_kf_rd_multiplier`, KF_UPDATE),
/// * `ROUND_POWER_OF_TWO(rdmult, 4)` for EB_TEN_BIT (rc_process.c:382),
/// * clamped to `>= 1` (rc_process.c:392),
/// * `* rd_frame_type_factor[bit_depth != 8][KF_UPDATE] = 128 >> 7`.
///
/// The `* 16` in [`kf_full_lambda_bd10`] comes from `multiply_lambda =
/// true`, which only the MD (enc_dec_process.c:177-188) and LR
/// (`pic_full_lambda[EB_10_BIT_MD]`, enc_dec_process.c:3246) paths pass —
/// NOT the CDEF search.
pub(crate) fn kf_full_lambda_bd10_unweighted(qindex: u8) -> u32 {
    let q = crate::bd10::dc_qlookup_10(qindex) as i64;
    let mut rdmult = ((3.3 + 0.0015 * q as f64) * q as f64 * q as f64) as i64;
    rdmult = (rdmult + 8) >> 4; // ROUND_POWER_OF_TWO(_, 4) — EB_TEN_BIT
    rdmult = rdmult.max(1); // rc_process.c:392 `rdmult > 0 ? .. : 1`
    ((rdmult * 128) >> 7) as u32 // rd_frame_type_factor[1][KF_UPDATE]
}

/// The LR search's `x->rdmult` at bd10: `pic_full_lambda[EB_10_BIT_MD]`
/// (enc_dec_process.c:3246-3247), i.e. `svt_aom_lambda_assign(..,
/// EB_TEN_BIT, qidx, multiply_lambda = true)` — the same base as
/// [`kf_full_lambda_bd10_unweighted`] with the `*= 16` applied
/// (rc_process.c:479). bd8's twin is `kf_full_lambda_8bit_unweighted`
/// (the `multiply_lambda` branch is 10-bit-only, so bd8 is unscaled).
pub(crate) fn kf_full_lambda_bd10_pic(qindex: u8) -> u32 {
    kf_full_lambda_bd10_unweighted(qindex) * 16
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
    if var64 <= var_th_cap { 64 } else { 32 }
}

/// C `svt_aom_derive_input_resolution` (sequence_control_set.c:120) mapped
/// through `input_resolution_factor[INPUT_SIZE_COUNT] = {0,1,2,3,4,4,4}`
/// (perform_tx_pd0, product_coding_loop.c:4579). At `coeff_rate_est_lvl == 0`
/// (the PD0_LVL_5 closed-form coeff rate) C adds `factor * 1600` bits to
/// EVERY block's coeff rate; the factor is a per-picture constant keyed on
/// the luma pixel count `width * height` (the padded encode dims — C uses
/// `picture_width * picture_height`, pcs.c:105). The thresholds are the
/// verbatim `INPUT_SIZE_*_TH` hex constants (definitions.h:1851-1857).
/// 64x64 (4096) and 128x128 (16384) are both < 240p_TH -> factor 0, so the
/// synthetic identity matrix is unaffected; 512x512 (262144) is 360p -> 1.
pub(crate) fn input_resolution_factor(pixels: usize) -> u64 {
    const FACTOR: [u64; 7] = [0, 1, 2, 3, 4, 4, 4];
    FACTOR[input_resolution_class(pixels) as usize]
}

/// C `svt_aom_derive_input_resolution` (sequence_control_set.c:120) — the
/// `ResolutionRange` class itself (definitions.h:1823-1832), keyed on the luma
/// pixel count against the verbatim `INPUT_SIZE_*_TH` constants.
///
/// 0 = 240p .. 6 = 8K. Several C signal derivations consult the class rather
/// than the factor above — `svt_aom_get_wn_filter_level_default` and
/// `svt_aom_get_sg_filter_level_default` zero themselves at
/// `>= INPUT_SIZE_8K_RANGE`, and the latter also at `> 360p` under
/// `fast_decode` — so the class is the shared primitive and
/// `input_resolution_factor` is one consumer of it.
pub(crate) fn input_resolution_class(pixels: usize) -> u8 {
    if pixels < 0x28500 {
        0 // 240p range
    } else if pixels < 0x4CE00 {
        1 // 360p range
    } else if pixels < 0xA1400 {
        2 // 480p range
    } else if pixels < 0x16DA00 {
        3 // 720p range
    } else if pixels < 0x535200 {
        4 // 1080p range
    } else if pixels < 0x140A000 {
        5 // 4K range
    } else {
        6 // 8K range
    }
}

/// C `is_dc_only_safe` (mode_decision.c:845) — the variance half, verbatim.
///
/// At allintra effective-M9 the PD1 intra controls are
/// `set_intra_ctrls(pcs, ctx, 8, 0)` (pcs->intra_level = 8 from
/// `svt_aom_get_intra_mode_levels_allintra` enc_mode_config.c:6907,
/// applied by `svt_aom_sig_deriv_enc_dec_allintra` enc_mode_config.c:11294;
/// note the light-PD1 path is NEVER taken for allintra —
/// `pcs->pic_lpd1_lvl = 0` unconditionally, enc_mode_config.c:15250 — so
/// PD1 is REGULAR with the allintra signals). Level 8 sets
/// `prune_using_edge_info = 1` (enc_mode_config.c:8576-8582), which arms
/// this gate inside `generate_md_stage_0_cand` (mode_decision.c:3633):
/// when it returns true the intra candidate set is EXACTLY {DC_PRED}
/// (`inject_intra_candidates` with dc_cand_only_flag; filter-intra,
/// palette and intrabc are all level-0 at eff-M9), so the leaf y_mode is
/// DC by construction — no cost compare ever runs. Verified live with the
/// instrumented library at gradient-64: q40 all four 32x32 leaves and q20
/// all sixteen 16x16 leaves print `dc_only=1 safe=1 ncand=1 modes: 0/0`;
/// the q55 64x64 prints `safe=0 ncand=4 modes: 0 1 2 9` (var 5425 >= 2000).
///
/// The C early exits (`prune_using_edge_info`, SB-128, `shape != PART_N`,
/// `sq_size == 4`) are the caller's context here: the fixed-tree PD1 walk
/// at still presets >= 9 is exactly PART_N squares 8..64 in a 64x64 SB.
/// (org_x, org_y) are SB-relative.
pub fn is_dc_only_safe(vars: &SbVariance, sq_size: usize, org_x: usize, org_y: usize) -> bool {
    if sq_size == 4 {
        return false;
    }
    let (blk_idx, sub_idx) = blk_var_map(sq_size, org_x, org_y);
    let blk_var = vars.0[blk_idx] as u32;

    // For 8x8, we do not have 4x4 sub-variance, skip spread check.
    if sq_size == 8 {
        return blk_var < 2000;
    }

    // For 16x16 and above, compute spread from sub-blocks.
    let mut min_var = u32::MAX;
    let mut max_var = 0u32;
    for &si in &sub_idx {
        let v = vars.0[si] as u32;
        min_var = min_var.min(v);
        max_var = max_var.max(v);
    }
    let spread_var = max_var - min_var;

    blk_var < 2000 && spread_var < 4000
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
pub(crate) fn lvl6_cost_allintra(
    vars: &SbVariance,
    sq_size: usize,
    org_x: usize,
    org_y: usize,
    qp: u32,
) -> u64 {
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
    let qzbin_factor = if q == 0 {
        64
    } else if dc < 148 {
        84
    } else {
        80
    };
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

/// C `svt_av1_quantize_b_qm` — the QM arm of `svt_aom_quantize_inv_quantize_
/// light` (full_loop.c:1346, 8-bit) — i.e. [`quantize_b`] with the frame luma
/// quantization matrix applied. `wt`/`iwt` are the raster-indexed matrix
/// slices from [`crate::qm::qm_slices`]. Mirrors the differentially C-tested
/// [`crate::qm::quantize_b_qm`] (tests/c_parity_qm.rs) on PD0's [`QuantEntry`]
/// (whose zbin/round/quant/quant_shift/dequant fields are identical to
/// `QuantTable`'s). Keeps the bd8-domain INT16 clamp (C's 8-bit kernel clamps
/// `INT16_MIN..INT16_MAX`, av1_quantize.c) — PD0 quantizes 8-bit residuals
/// even at bd10.
fn quantize_b_qm(
    coeffs: &[i32],
    scan: &[u16],
    e: &QuantEntry,
    log_scale: i32,
    wt: &[u8],
    iwt: &[u8],
) -> (u16, Vec<i32>, Vec<i32>) {
    const AOM_QM_BITS: i32 = 5;
    let n_coeffs = scan.len();
    let zbins = [
        (e.zbin[0] + ((1 << log_scale) >> 1)) >> log_scale,
        (e.zbin[1] + ((1 << log_scale) >> 1)) >> log_scale,
    ];
    let mut qcoeff = vec![0i32; coeffs.len()];
    let mut dqcoeff = vec![0i32; coeffs.len()];

    // Pre-scan pass (weighted zbin dead zone).
    let mut non_zero_count = n_coeffs;
    for i in (0..n_coeffs).rev() {
        let rc = scan[i] as usize;
        let w = i32::from(wt[rc]);
        let coeff = coeffs[rc] * w;
        let iz = usize::from(rc != 0);
        if coeff < zbins[iz] * (1 << AOM_QM_BITS) && coeff > -zbins[iz] * (1 << AOM_QM_BITS) {
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
        let w = i64::from(wt[rc]);
        if i64::from(abs_coeff) * w >= i64::from(zbins[iz]) << AOM_QM_BITS {
            let round = (e.round[iz] + ((1 << log_scale) >> 1)) >> log_scale;
            let mut tmp = i64::from((abs_coeff + round).clamp(i16::MIN as i32, i16::MAX as i32));
            tmp *= w;
            let tmp32 = (((((tmp * e.quant[iz] as i64) >> 16) + tmp) * e.quant_shift[iz] as i64)
                >> (16 - log_scale + AOM_QM_BITS)) as i32;
            qcoeff[rc] = (tmp32 ^ coeff_sign) - coeff_sign;
            let dequant =
                (e.dequant[iz] * i32::from(iwt[rc]) + (1 << (AOM_QM_BITS - 1))) >> AOM_QM_BITS;
            let abs_dq = ((tmp32 as i64 * dequant as i64) >> log_scale) as i32;
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

/// The shared perform_tx_pd0 transform+quant+distortion core: forward
/// DCT_DCT at the (possibly subres-halved) max-square tx size, 64-dim
/// energy fold + pack (svt_handle_transform64x64/64x32), quantize at
/// `qindex_off` (the caller applies `rate_est_ctrls.lpd0_qp_offset`),
/// frequency-domain SSE + three_quad_energy, and the dist shift.
///
/// C's PD0 RECON neighbour state for one superblock — the port's model of
/// `ctx->recon_neigh_y` while `pd0_use_src_samples` is FALSE.
///
/// **Why it exists.** `svt_aom_sig_deriv_enc_dec_pd0` sets
/// `ctx->pd0_use_src_samples = allintra || pcs->hbd_md`
/// (enc_mode_config.c:7309), so on a VIDEO frame PD0 does NOT copy the source
/// row/column into the recon-neighbour arrays. Instead every PD0 block
/// predicts from the RECON that PD0 itself generates
/// (`av1_perform_inverse_transform_recon`, product_coding_loop.c:8438) and
/// writes back through `mode_decision_update_neighbor_arrays_pd0` (:121) at
/// the points where a node's partition is DECIDED. MEASURED with the
/// `SVT_PD0COST_OUT` interposer on `gradient 64x64 q40 p6` video: with the
/// level and subres right but the prediction still from source, C and the port
/// agree to the unit on every block that has NO neighbour and diverge on every
/// block that has one.
///
/// **Why a pixel canvas and not the 1-D arrays.** C's neighbour array keeps,
/// per column, the bottom row of the last block written there, and per row the
/// right column. PD0's decided blocks TILE the superblock, so for every read
/// the C array holds exactly the canvas pixel at `(x, y-1)` / `(x-1, y)` — the
/// canvas is equivalent and it lets the existing
/// [`crate::partition::extract_neighbors_tiled`] supply C's `n_top_px` /
/// `n_left_px` clamp and edge replication unchanged.
///
/// The canvas covers rows `sb_y - 1 ..= sb_y + 64` at the frame's aligned
/// stride, seeded from the MD recon of the already-coded superblocks — which
/// is what C's arrays hold at SB entry, since `copy_neighbour_arrays_pd0`
/// snapshots the live MD arrays (enc_dec_process.c:2980) rather than clearing
/// them.
struct Pd0ReconCanvas {
    buf: alloc::vec::Vec<u8>,
    stride: usize,
    /// Frame row the canvas's row 0 corresponds to (`sb_y - 1`, or 0).
    y0: usize,
}

/// The number of canvas rows: 1 above + 64 SB + 1 so a 64-tall block's left
/// column read (`abs_y .. abs_y + 64` at canvas row `abs_y - y0`) stays in
/// bounds instead of tripping `extract_neighbors_tiled`'s length guard.
const PD0_CANVAS_ROWS: usize = 66;

impl Pd0ReconCanvas {
    /// Seed from the frame's MD recon plane at this SB's origin.
    fn new(recon: &[u8], stride: usize, sb_y: usize) -> Self {
        let y0 = sb_y.saturating_sub(1);
        let mut buf = alloc::vec![128u8; stride * PD0_CANVAS_ROWS];
        for r in 0..PD0_CANVAS_ROWS {
            let src = (y0 + r) * stride;
            if src + stride <= recon.len() {
                buf[r * stride..r * stride + stride].copy_from_slice(&recon[src..src + stride]);
            }
        }
        Self { buf, stride, y0 }
    }

    /// C `svt_aom_update_recon_neighbor_array` for one decided node: the
    /// block's recon becomes the neighbour reference for everything below and
    /// to the right of it. The straddle clip is `commit_leaf`'s — a block
    /// reaching past the aligned stride must not wrap into the next row.
    fn write(&mut self, abs_x: usize, abs_y: usize, bw: usize, bh: usize, recon: &[u8]) {
        let wr = bw.min(self.stride.saturating_sub(abs_x));
        for r in 0..bh {
            let row = (abs_y + r).saturating_sub(self.y0);
            if row >= PD0_CANVAS_ROWS || wr == 0 {
                continue;
            }
            let dst = row * self.stride + abs_x;
            self.buf[dst..dst + wr].copy_from_slice(&recon[r * bw..r * bw + wr]);
        }
    }
}

/// C `perform_tx_pd0`'s tx size after the subres remap
/// (product_coding_loop.c:4318-4344): the residual is `bw x tx_h` with
/// `tx_h = bh >> mds_subres_step`. Returns the port enum and C's TxSize index.
fn pd0_tx_size(bw: usize, tx_h: usize) -> (svtav1_types::transform::TxSize, usize) {
    use svtav1_types::transform::TxSize;
    match (bw, tx_h) {
        (64, 64) => (TxSize::Tx64x64, 4usize),
        (64, 32) => (TxSize::Tx64x32, 12),
        (32, 32) => (TxSize::Tx32x32, 3),
        (32, 16) => (TxSize::Tx32x16, 10),
        (16, 16) => (TxSize::Tx16x16, 2),
        (16, 8) => (TxSize::Tx16x8, 8),
        (8, 8) => (TxSize::Tx8x8, 1),
        (8, 4) => (TxSize::Tx8x4, 6),
        (4, 4) => (TxSize::Tx4x4, 0),
        // Task #95 chunk 2: "tall" rect TX for the PARTITION_VERT boundary
        // block (`sq/2 x sq`) of a right-edge partial-SB node. The AV1 enum
        // indices mirror the "wide" ones (TX_8X16=7, TX_16X32=9, TX_32X64=11).
        (32, 64) => (TxSize::Tx32x64, 11),
        (16, 32) => (TxSize::Tx16x32, 9),
        (8, 16) => (TxSize::Tx8x16, 7),
        _ => unreachable!("PD0 tx {bw}x{tx_h}"),
    }
}

/// Returns (eob, dist, packed qcoeff, packed C TxSize, packed dqcoeff).
///
/// The DEQUANTIZED coefficients come back too because C's video PD0 needs
/// them: with `pd0_use_src_samples = false` the block's RECON feeds the next
/// block's intra prediction, and recon is `pred + inverse_transform(dqcoeff)`.
/// The allintra paths ignore the extra value.
fn tx_quant_core(
    residual: &[i32],
    sq_size: usize,
    tx_h: usize,
    qindex_off: u8,
    qm_level: u8,
    subres_step: u32,
) -> (u16, u64, Vec<i32>, usize, Vec<i32>) {
    use svtav1_types::transform::TxType;
    let (tx_size, c_tx_size) = pd0_tx_size(sq_size, tx_h);

    let mut coeffs = vec![0i32; sq_size * tx_h];
    svtav1_dsp::txfm_dispatch::fwd_txfm2d_dispatch(
        residual,
        &mut coeffs,
        sq_size,
        tx_size,
        TxType::DctDct,
    );

    // 64-dim fold + pack (svt_handle_transform64x64 / 64x32 / 32x64).
    let mut three_quad_energy = 0u64;
    if sq_size == 64 {
        if tx_h == 64 {
            three_quad_energy =
                energy(&coeffs[32..], 64, 32, 32) + energy(&coeffs[32 * 64..], 64, 64, 32);
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
    } else if tx_h == 64 {
        // Tall 32x64 (svt_handle_transform32x64): the block is 32 wide (no
        // width fold), so the top 32 rows are already contiguous — keep them
        // and route the bottom 32 rows' energy to three_quad_energy.
        three_quad_energy = energy(&coeffs[sq_size * 32..], sq_size, sq_size, 32);
        coeffs.truncate(sq_size * 32);
    }

    let packed_w = sq_size.min(32);
    let packed_h = tx_h.min(32);
    let log_scale = TX_SCALE_TAB[c_tx_size];
    let entry = build_quant_entry(qindex_off);
    let scan = crate::entropy::scan_tables::scan(c_tx_size, 0);
    debug_assert_eq!(scan.len(), packed_w * packed_h);
    // [SVT_HDR_MODE] Quantization matrices in PD0. C's md_encode_block_pd0
    // quantize (`svt_aom_quantize_inv_quantize_light`, full_loop.c:1263)
    // applies the frame's luma QM whenever `frm_hdr.quantization_params.
    // using_qmatrix` is set (fork default ON) — the QM arm calls
    // `svt_av1_quantize_b_qm`. PD0 always transforms DCT_DCT, which
    // IS_2D_TRANSFORM, so the matrix applies whenever the frame luma
    // `qm_level < 15`. The matrix LEVEL is the frame value derived from
    // base_qindex (`frm_hdr.quantization_params.qm[PLANE_Y]`,
    // md_config_process.c:270), NOT the `qindex_off` quant step. C passes
    // `bit_depth = EB_EIGHT_BIT` to the PD0 quantize (product_coding_loop.c:
    // 4397/4471), so even at bd10 it is the 8-bit QM kernel over the 8-bit
    // `quants_8bit` tables `build_quant_entry` already models — this fix is
    // 8-bit-domain and carries no highbd term. Without it PD0 dequantized
    // WITHOUT matrices, so a QM-tipped partition near-tie (top-left 32x32 of
    // a smooth SB) coded SPLIT where C keeps NONE (fork x bd10 Class A).
    let (eob, qcoeff, dqcoeff) = match (qm_level < 15)
        .then(|| crate::qm::qm_slices(usize::from(qm_level), false, c_tx_size))
        .flatten()
    {
        Some((wt, iwt)) => quantize_b_qm(&coeffs, scan, &entry, log_scale, wt, iwt),
        None => quantize_b(&coeffs, scan, &entry, log_scale),
    };

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
    dist = if shift < 0 {
        dist << (-shift)
    } else {
        dist >> shift
    };
    dist <<= subres_step;

    (eob, dist, qcoeff, c_tx_size, dqcoeff)
}

// ---------------------------------------------------------------------------
// PD0_LVL_1 coefficient rate (svt_av1_cost_coeffs_txb, contexts 0)
// ---------------------------------------------------------------------------

/// C `av1_cost_literal(n)` (1/512-bit units).
#[inline]
const fn cost_literal(n: i32) -> i32 {
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
    tx4: i32,
    tx8: i32,
    tx16: i32,
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

impl TxTypeRatesDc {
    #[inline]
    fn rate_for(&self, c_tx_size: usize) -> i32 {
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
const PD0_FAST_COEFF_EST_LEVEL: i32 = 2;

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
fn loop_cost_eob_pd0(
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
fn cost_coeffs_txb_pd0(
    qcoeff: &[i32],
    eob: u16,
    c_tx_size: usize,
    tables: &crate::quant::CoeffCostTables,
    tx_rates: &TxTypeRatesDc,
    subres_step: u32,
) -> i32 {
    crate::entropy::coeff_c::with_txb_scratch(|sc| {
        cost_coeffs_txb_pd0_inner(qcoeff, eob, c_tx_size, tables, tx_rates, subres_step, sc)
    })
}

/// [`cost_coeffs_txb_pd0`]'s body, with the level map and the nz-map context
/// array supplied by the caller (see `coeff_c::TxbScratch`).
#[allow(clippy::too_many_arguments)]
fn cost_coeffs_txb_pd0_inner(
    qcoeff: &[i32],
    eob: u16,
    c_tx_size: usize,
    tables: &crate::quant::CoeffCostTables,
    tx_rates: &TxTypeRatesDc,
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
    } = sc;
    if eob > 1 {
        cc::txb_init_levels(qcoeff, width, height, levels_buf);
    } else {
        // See the twin in `leaf_funnel::coeff_rate::cost_coeffs_txb_inner`:
        // eob == 1 reads the map without filling it, so the `used` prefix must
        // be zeroed to reproduce the old per-call stack array.
        let used = cc::levels_used_len(width, height, levels_buf.len());
        levels_buf[..used].fill(0);
    }
    cost += tx_rates.rate_for(c_tx_size);
    cost += crate::quant::eob_cost(eob as i32, eob_bits, coeff_costs, cc::TX_CLASS_2D);

    // Same per-thread scratch as `leaf_funnel::coeff_rate::cost_coeffs_txb`'s
    // — see `cc::TxbScratch`. Only the `n_ctx` prefix is cleared.
    let n_ctx = width * height;
    debug_assert!(n_ctx <= cc::MAX_TXB_COEFF_AREA);
    ctx_buf[..n_ctx].fill(0);
    cc::get_nz_map_contexts(
        &levels_buf[..],
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
            &levels_buf[..],
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
fn cost_skip_txb_pd0(c_tx_size: usize, tables: &crate::quant::CoeffCostTables) -> i32 {
    let txs_ctx = crate::entropy::coeff_c::txsize_entropy_ctx(c_tx_size);
    tables.txb(txs_ctx, 0).txb_skip_cost[0][1]
}

/// C `check_is_subres_safe` (product_coding_loop.c): SAD of even vs odd
/// rows of (src - pred) over the 64x64; safe iff the deviation is within
/// `odd_to_even_deviation_th = 5` percent.
fn check_is_subres_safe(
    src: &[u8],
    stride: usize,
    org_x: usize,
    org_y: usize,
    pred: &[u8],
) -> bool {
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
    /// A quadrant whose top-left lies at/after the ALIGNED frame extent —
    /// it codes NOTHING (C `svt_aom_write_modes_sb`'s `mi_row >= mi_rows ||
    /// mi_col >= mi_cols` early return / the SPLIT-loop `continue`). Only
    /// produced on partial superblocks (task #95 chunk 2); a 64-aligned
    /// frame never generates it.
    Off,
}

/// The partition tree of a CODED-LOSSLESS coding unit (issue #5): C forces
/// `max_sq_size = MIN(max_sq_size, 8)` when `mimic_only_tx_4x4` is set
/// (enc_dec_process.c:1492-1493) and `min_sq_size` is 8 wherever 4x4 is
/// disallowed (`svt_aom_get_disallow_4x4_default`: every preset above M2, and
/// this port reaches the lossless envelope only there), so every square above
/// 8x8 is never tested — only SPLIT — and every leaf is an 8x8 PARTITION_NONE
/// (NSQ is off at these presets, `nsq_search_level = 0`). Quadrants whose
/// origin lies at or past the ALIGNED frame extent are `Off`, exactly as
/// `Pd0Eval::tree` produces them on a partial superblock.
pub fn lossless_tree(
    x0: usize,
    y0: usize,
    size: usize,
    aligned_w: usize,
    aligned_h: usize,
) -> Pd0Tree {
    if x0 >= aligned_w || y0 >= aligned_h {
        return Pd0Tree::Off;
    }
    if size <= 8 {
        return Pd0Tree::Leaf(size);
    }
    let half = size / 2;
    Pd0Tree::Split(Box::new([
        lossless_tree(x0, y0, half, aligned_w, aligned_h),
        lossless_tree(x0 + half, y0, half, aligned_w, aligned_h),
        lossless_tree(x0, y0 + half, half, aligned_w, aligned_h),
        lossless_tree(x0 + half, y0 + half, half, aligned_w, aligned_h),
    ]))
}

impl Pd0Tree {
    /// Leaf sizes in raster/coding order (debug aid). Off quadrants
    /// contribute nothing.
    pub fn leaf_sizes(&self) -> Vec<usize> {
        match self {
            Pd0Tree::Leaf(s) => vec![*s],
            Pd0Tree::Split(ch) => ch.iter().flat_map(|c| c.leaf_sizes()).collect(),
            Pd0Tree::Off => vec![],
        }
    }
}

/// PD0 evaluation record for one square node — the C `pc_tree` fields the
/// PD1 depth refinement reads (`tested_blk[PART_N][0]`,
/// `block_data[PART_N][0]->cost`, `partition`): every node the PD0 walk
/// visited carries whether its PART_N block was costed and that cost.
/// Children exist whenever the split test recursed into them (quadrants
/// skipped by the split-cost early exit stay `tested = false` with no
/// children, exactly like C's untouched `pc_tree->split[i]`).
#[derive(Debug, Clone)]
pub struct Pd0Eval {
    pub sq: usize,
    /// Some d1 shape was costed at this node — C `pc_tree->rdc.valid` after
    /// `svt_aom_pick_partition_pd0`. At a one-false BOUNDARY node the costed
    /// shape is PART_H/PART_V, NOT the square: see [`Pd0Eval::sq_tested`].
    pub tested: bool,
    /// C `tested_blk[PART_N][0]` — the SQUARE PART_N was costed at this node,
    /// so `block_data[PART_N][0]->cost` is readable.
    ///
    /// This is STRICTLY narrower than [`Pd0Eval::tested`] on a partial SB:
    /// `svt_aom_pick_partition_pd0` (product_coding_loop.c:10548-10560) writes
    /// `block_data[shape][0]` for the ONE shape `set_blocks_to_test` injected,
    /// which at a single-edge node is PART_H / PART_V. Every PD1
    /// depth-refinement gate that reads a PD0 cost is guarded on
    /// `tested_blk[PART_N][0]` for exactly that reason — C spells it out at
    /// `update_pred_th_offset` (enc_dec_process.c:1547-1549): *"For incomplete
    /// blocks, H/V partitions may be allowed, while square is not. In those
    /// cases, the selected depth may not have a valid SQ cost, so we need to
    /// check that the SQ block is available before using the cost."*
    /// Consequence: a boundary PD0 leaf gets `s_depth = e_depth = 0` — it is
    /// never refined, only coded at its own depth.
    ///
    /// Identical to `tested` on a 64-aligned frame (no node is one-false).
    pub sq_tested: bool,
    /// C `pc_tree->rdc.rd_cost` — the costed shape's cost (valid iff
    /// `tested`). The SQUARE cost only when `sq_tested`.
    pub cost: u64,
    /// PD0 picked SPLIT at this node (`pc_tree->partition`).
    pub split: bool,
    /// This node's top-left is at/after the ALIGNED frame extent — it codes
    /// nothing (partial-SB off-frame quadrant, task #95 chunk 2). Mutually
    /// exclusive with `tested`/`split`. Never set on a 64-aligned frame.
    pub off: bool,
    pub children: Option<Box<[Pd0Eval; 4]>>,
}

impl Pd0Eval {
    fn untested(sq: usize) -> Self {
        Pd0Eval {
            sq,
            tested: false,
            sq_tested: false,
            cost: 0,
            split: false,
            off: false,
            children: None,
        }
    }

    /// An off-frame quadrant (top-left >= aligned extent): codes nothing.
    fn off(sq: usize) -> Self {
        Pd0Eval {
            sq,
            tested: false,
            sq_tested: false,
            cost: 0,
            split: false,
            off: true,
            children: None,
        }
    }

    /// The picked partition tree this eval corresponds to.
    pub fn tree(&self) -> Pd0Tree {
        if self.off {
            Pd0Tree::Off
        } else if self.split {
            let ch = self.children.as_ref().expect("split node has children");
            Pd0Tree::Split(Box::new([
                ch[0].tree(),
                ch[1].tree(),
                ch[2].tree(),
                ch[3].tree(),
            ]))
        } else {
            Pd0Tree::Leaf(self.sq)
        }
    }

    /// C `get_max_min_pd0_depths` (enc_dec_process.c:1959): max/min PICKED
    /// leaf sizes over the tree. Off-frame quadrants contribute nothing (C
    /// only walks in-bounds sub-trees).
    pub fn max_min_picked(&self, max: &mut usize, min: &mut usize) {
        if self.off {
            return;
        }
        if self.split {
            for c in self.children.as_ref().expect("split children").iter() {
                c.max_min_picked(max, min);
            }
        } else {
            *max = (*max).max(self.sq);
            *min = (*min).min(self.sq);
        }
    }
}

/// Which PD0 block-encode path prices a block (C `Pd0Level`, collapsed to
/// the three configurations reachable from the allintra preset ladder).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pd0Mode {
    /// PD0_LVL_6 closed-form variance cost (eff-M9, undemoted).
    Lvl6,
    /// PD0_LVL_5 light encode: qindex+8, subres, 5000+100*eob rate
    /// (eff-M9 demoted by the detector).
    Lvl5,
    /// PD0_LVL_0 full-RD partition search — C `set_pd0_ctrls`
    /// (enc_mode_config.c:5415) FORCES this level whenever `hbd_md` is set
    /// (i.e. bit_depth 10 DUAL), regardless of preset. PD0 itself runs
    /// entirely at 8-BIT (`enc_dec_process.c:2965` saves hbd_md and forces
    /// it to 0 for the whole PD0 pass), so the partition tree is a pure
    /// function of the 8-bit MSB-truncated plane — NO bd10 pixel/quant/lambda
    /// kernel is on the partition path.
    ///
    /// The block cost is IDENTICAL to [`Pd0Mode::Lvl5`] (same DC-from-source
    /// prediction, `lpd0_qp_offset = 8` -> qindex+8, `coeff_rate_est_lvl = 0`
    /// closed form `5000 + ires*1600 + 100*eob`, doubled split rate because
    /// `use_accurate_part_ctx = 0` above M8) EXCEPT that **subres is OFF**:
    /// LVL_0 is `pd0_level <= PD0_LVL_2` so `subres_level = 0`
    /// (enc_mode_config.c:7327), whereas LVL_5 enables step-1 subres via the
    /// odd/even-deviation check. There is also NO PD0-level detector: every
    /// SB runs the full block encode (LVL_5 only runs it when the detector
    /// demotes LVL_6). Verified end-to-end against real C's SVT_PD0COST_OUT +
    /// SVT_CTREE_OUT dumps at bd10.
    Lvl0,
    /// PD0_LVL_1 (allintra M2..M8): qindex+0, no subres, real
    /// `svt_av1_cost_coeffs_txb` rate at zero contexts, undoubled split
    /// rate (`use_accurate_part_ctx = 1`).
    Lvl1,
    /// PD0_LVL_3 — the VIDEO arm's level at M3..M8 (`set_pic_pd0_lvl_default`,
    /// enc_mode_config.c:8592; `set_pd0_ctrls` case 3, `:5435`, arms no
    /// detector at all).
    ///
    /// Block cost is [`Pd0Mode::Lvl1`]'s — `rate_est_level` is 2 for every
    /// `pd0_level <= PD0_LVL_3` (`svt_aom_sig_deriv_enc_dec_pd0`,
    /// enc_mode_config.c:7357), i.e. `lpd0_qp_offset = 0` +
    /// `coeff_rate_est_lvl = 1`, exactly LVL_1's — with TWO differences, both
    /// keyed on the level rather than the preset and neither of them visible
    /// in `sig_deriv_mode_decision_config`'s slot table:
    ///
    /// * **subres step 1.** `pd0_level <= PD0_LVL_2` forces `subres_level = 0`
    ///   (`:7337`); at LVL_3 an I-slice takes `subres_level = 1` outright
    ///   (`:7345`), still gated on `disallow_4x4` and a complete b64. The
    ///   per-SB `is_subres_safe` odd/even-deviation check then runs on the
    ///   64x64 exactly as it does at LVL_5.
    /// * **`depth_early_exit_lvl` 2** (`:7233`) — `early_exit_th` 900 instead
    ///   of LVL_1's 0-which-reads-as-1000, for the i > 0 quadrants.
    Lvl3,
    /// PD0_LVL_4 — the VIDEO arm's level at M9..M13 on <= 360p content
    /// (`set_pic_pd0_lvl_default`; `set_pd0_ctrls` case 4 arms a detector that
    /// is INERT on an I-slice — every branch of `pd0_detector` below the
    /// LVL_6 demote is `slice_type != I_SLICE`-gated, enc_dec_process.c:2473).
    ///
    /// Identical to [`Pd0Mode::Lvl3`] except that `rate_est_level` is **4**
    /// (`:7359`), i.e. `coeff_rate_est_lvl = 2` — the `eob < th ? 6000 +
    /// eob*500 : real` approximation `lvl1_block_cost_rect` already carries
    /// for the allintra M7/M8 rows.
    Lvl4,
}

/// The rate tables PD0_LVL_1 prices with. For single-SB frames these are
/// the default tables at the frame qindex bucket (C: `md_frame_context`
/// feeds SB 0); multi-SB refresh from the evolving frame context
/// (enc_dec_process.c:2991, `cdf_ctrl.enabled` at M6) is NOT yet ported —
/// SBs after the first reuse the defaults, which C only does for SB 0.
pub struct M6Pd0Tables {
    pub coeff: alloc::boxed::Box<crate::quant::CoeffCostTables>,
    tx_rates: TxTypeRatesDc,
    /// PARTITION_SPLIT rate per square size (index by log2(sq) - 3:
    /// 8/16/32/64), from THIS SB's chained partition CDFs (ctx row 0).
    split_bits: [u64; 4],
    /// BINARY SPLIT rate for a one-false BOUNDARY node, per square size —
    /// C `svt_aom_partition_rate_cost` boundary branch (rd_cost.c:1846-1863):
    /// the bottom-edge (`!has_rows`) uses `partition_vert_alike_fac_bits`, the
    /// right-edge (`!has_cols`) `partition_horz_alike_fac_bits`, indexed
    /// `[ctx][SPLIT]` — NOT the full-alphabet `split_bits`. Gather is
    /// CROSS-named vs the option. Slot 0 (8x8) is never used (8x8 is never an
    /// edge node).
    vert_alike_split_bits: [u64; 4],
    horz_alike_split_bits: [u64; 4],
    /// `partition_fac_bits[0][PARTITION_NONE]` (context index 0 — the
    /// 8x8-class row, rd_cost.c:1344-1349 approximation).
    none_bits_ctx0: u64,
    /// `skip_fac_bits[0][0]`.
    skip0_bits: u64,
}

/// Build the PD0_LVL_1 tables for a frame (default CDFs at `qindex`).
pub fn build_m6_pd0_tables(qindex: u8) -> M6Pd0Tables {
    let fc = crate::entropy::context::FrameContext::new_default();
    let cfc = crate::entropy::coeff_c::CoeffFc::default_for_qindex(qindex);
    build_m6_pd0_tables_from_ctx(&fc, &cfc)
}

/// [`build_m6_pd0_tables`] over an ARBITRARY (chained) context pair — the
/// per-SB `ec_ctx_array[sb]` rate refresh C runs at update_cdf_level 2
/// (enc_dec_process.c:3024-3043; the drifting 64x64 SPLIT rates
/// 1195 -> 1221 -> 1244 -> 1268 across g128 q55's SBs come from here).
pub fn build_m6_pd0_tables_from_ctx(
    fc: &crate::entropy::context::FrameContext,
    cfc: &crate::entropy::coeff_c::CoeffFc,
) -> M6Pd0Tables {
    // partition ctx row for sub-context 0 of each size class: bsl*4
    // (pipeline EntropyCtx::partition_ctx semantics; nsyms 10 for the
    // square 8..64 classes at ctx rows 0..=15 except row 0 = 4 syms).
    let mut split_bits = [0u64; 4];
    let mut vert_alike_split_bits = [0u64; 4];
    let mut horz_alike_split_bits = [0u64; 4];
    let mut none_bits_ctx0 = 0u64;
    for (slot, sq) in [(0usize, 8usize), (1, 16), (2, 32), (3, 64)] {
        let bsl = match sq {
            8 => 0usize,
            16 => 1,
            32 => 2,
            _ => 3,
        };
        let ctx = bsl * 4;
        let nsyms = if ctx <= 3 { 4 } else { 10 };
        let mut costs = [0i32; 10];
        crate::quant::syntax_rate_from_cdf(&mut costs[..nsyms], &fc.partition_cdf[ctx]);
        split_bits[slot] = costs[crate::partition::PartitionType::Split as usize] as u64;
        // Binary boundary SPLIT rate at the same ctx row (left = above = 0).
        // is_128 = false: PD0 squares here are <= 64. Slot 0 (8x8) computes a
        // value that is never consumed (8x8 is never an edge node).
        vert_alike_split_bits[slot] = crate::entropy::context::partition_alike_split_cost(
            &fc.partition_cdf[ctx],
            true, // !has_rows -> vert_alike (bottom edge)
            false,
        ) as u64;
        horz_alike_split_bits[slot] = crate::entropy::context::partition_alike_split_cost(
            &fc.partition_cdf[ctx],
            false, // !has_cols -> horz_alike (right edge)
            false,
        ) as u64;
        if sq == 8 {
            none_bits_ctx0 = costs[crate::partition::PartitionType::None as usize] as u64;
        }
    }
    let mut skip_costs = [0i32; 2];
    crate::quant::syntax_rate_from_cdf(&mut skip_costs, &fc.skip_cdf[0]);
    M6Pd0Tables {
        coeff: crate::quant::build_coeff_cost_tables_from_fc(cfc),
        tx_rates: build_tx_type_rates_dc_from_fc(cfc),
        split_bits,
        vert_alike_split_bits,
        horz_alike_split_bits,
        none_bits_ctx0,
        skip0_bits: skip_costs[0] as u64,
    }
}

impl M6Pd0Tables {
    #[inline]
    fn size_slot(sq_size: usize) -> usize {
        // BENIGN `_ => 3`: slot 3 is BLOCK_64X64. This table only ever sees
        // PD0 squares in {8,16,32,64} — even at SB128 the b64-coding-unit
        // decomposition (`sb_coding_units`) keeps every coding square <= 64. So
        // `_` folds 64 (never 128) into slot 3. NOT the `EntropyCtx::bsl` class
        // of `_ => 3` bug (which wrongly folded 128 into the 64 level); there is
        // no 128 slot here because no 128 square reaches this function.
        match sq_size {
            8 => 0,
            16 => 1,
            32 => 2,
            _ => 3,
        }
    }
    #[inline]
    pub(crate) fn split_bits(&self, sq_size: usize) -> u64 {
        self.split_bits[Self::size_slot(sq_size)]
    }
    /// Binary boundary SPLIT rate for a one-false node. `bottom_edge`
    /// (`!has_rows`) -> vert_alike; else (right edge, `!has_cols`) -> horz_alike.
    #[inline]
    fn boundary_split_bits(&self, sq_size: usize, bottom_edge: bool) -> u64 {
        let slot = Self::size_slot(sq_size);
        if bottom_edge {
            self.vert_alike_split_bits[slot]
        } else {
            self.horz_alike_split_bits[slot]
        }
    }
}

/// The INTER arm of PD0's per-block prediction on a NON-KEY frame.
///
/// C's PD0 dispatches through
/// `product_prediction_fun_table_pd0[is_inter_mode(cand->block_mi.mode)]`
/// (product_coding_loop.c:53), and on a non-I slice the candidate set that
/// reaches it is ONE inter `NEWMV`:
///
/// * `svt_aom_sig_deriv_enc_dec_pd0` resolves `intra_level = 0` for
///   `slice_type != I_SLICE` at `pd0_level > PD0_LVL_2` and
///   `enc_mode <= M10` (enc_mode_config.c:7240-7253), so `set_intra_ctrls`
///   leaves `enable_intra = 0` and `generate_md_stage_0_cand_pd0` injects NO
///   intra candidate at all (mode_decision.c:3500).
/// * `inject_new_candidates_pd0` (mode_decision.c:2293) injects one `NEWMV`
///   per open-loop ME candidate at `me_mv * 8`, capped at three. The port's
///   low-delay-P reference set has ONE reference, so there is one.
/// * With a single candidate `md_encode_block_pd0` takes its
///   `fast_candidate_total_count == 1 && pd0_level < PD0_LVL_6` shortcut
///   (product_coding_loop.c:8390): prediction only, no MDS0 distortion, then
///   `md_stage_3_pd0` -> `full_loop_core_pd0` on it. So the ONLY thing that
///   changes versus the key-frame path is where `pred` comes from.
///
/// MEASURED with `SVT_PD0CFG_OUT` on `diag 64x64 q40 p8 frames=2`:
///
/// ```text
/// PD0CFG sb=0 org=(0,0) islice=1 lvl=4 ... intra=1/12/1 ...
/// PD0CFG sb=0 org=(0,0) islice=0 lvl=3 ... intra=0/0/0  ...
/// ```
///
/// `enable_intra/intra_mode_end/angular_pred_level` is `0/0/0` on the inter
/// frame — which is the assertion this struct's presence stands for.
#[derive(Clone, Copy)]
pub struct Pd0InterRef<'a> {
    /// The DPB reference's PADDED luma plane. C's PD0 MC does NOT clamp the
    /// MV on the unscaled path (see
    /// [`crate::inter_pred_arm::predict_inter_luma_pd0`]), so a legal MV
    /// reads the replicated margin.
    pub padded_y: &'a crate::picture::PaddedPlane,
    /// This frame's open-loop motion search — C `pcs->ppcs->pa_me_data`.
    pub me: &'a crate::inter_me_arm::FrameMe,
    /// C `scs->super_block_size`, which sizes the driver's CONV_BUF.
    pub sb_size: usize,
    /// ALIGNED frame dims, for the (never-taken) scaled branch's edges.
    pub frame_w: usize,
    pub frame_h: usize,
    /// C `ppcs->update_type` — the rdmult BASE selector. See
    /// [`inter_full_lambda_8bit`]: PD0's `full_sb_lambda_md[EB_8_BIT_MD]` is
    /// the frame's MD lambda, so on an inter frame it is NOT the KF chain
    /// [`kf_full_lambda_8bit_lw`] that every key-frame caller passes.
    pub base_update_type: crate::port_rc_process::FrameUpdateType,
    /// C `update_lambda`'s own `gf_update_type` — the frame-type FACTOR row.
    pub factor_update_type: crate::port_rc_process::FrameUpdateType,
    /// [SVT_HDR_MODE] `static_config.alt_lambda_factors`.
    pub alt_lambda_factors: bool,
    /// C `set_blocks_to_be_tested`'s `min_sq_size` (enc_dec_process.c:1485),
    /// which `depth_removal_ctrls` decides:
    ///
    /// ```text
    /// disallow_below_64x64 ? 64
    ///   : disallow_below_32x32 ? 32
    ///   : (disallow_8x8 || disallow_below_16x16) ? 16
    ///   : disallow_4x4 ? 8 : 4          then MIN(.., max_tx_size)
    /// ```
    ///
    /// The controls are computed per SUPERBLOCK from the open-loop ME
    /// distortions, so this is a per-SB value and not a frame one. On an
    /// I-slice `set_depth_removal_level_controls` returns `enabled = 0`
    /// outright, which is why the key path never carries it.
    pub min_sq: usize,
    /// C `update_lambda`'s `me_q_index - base_q_idx` for THIS superblock
    /// (`rc_process.c:437-446`), which is what makes `full_lambda_md` and
    /// `fast_lambda_md` per-superblock. 0 reproduces the frame-level lambda.
    pub me_qdiff: i32,
}

/// C `av1_lambda_assign_md`'s per-SUPERBLOCK output on an INTER frame
/// (`svt_aom_mode_decision_configure_sb`, md_process.c:796).
///
/// One value per superblock in raster order. It exists because C's MD lambdas
/// are per-SB through `update_lambda`'s `stats_based_sb_lambda_modulation`
/// block (rc_process.c:423-446), whose `qdiff` is
/// `svt_aom_get_me_qindex(sb) - base_q_idx` — a quantity derived from
/// `me_8x8_cost_variance` alone and therefore live even with no per-SB
/// delta-q signalled.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SbInterLambda {
    /// C `ctx->full_lambda_md[EB_8_BIT_MD]` (== `full_sb_lambda_md[0]`).
    pub full_8bit: u32,
    /// C `ctx->fast_lambda_md[EB_8_BIT_MD]`.
    pub fast_8bit: u32,
    /// `me_q_index - base_q_idx` for this superblock — the ONE input the
    /// three lambda derivations (PD0, the MD funnel, the MD searches) share.
    pub me_qdiff: i32,
}

/// C `BlockSize` for a shape PD0 can cost — the square depths plus the two
/// fitting boundary rectangles [`Pd0Ctx::pick_q`] injects at a one-false node.
///
/// Only [`crate::port_md::predicates::get_me_block_offset`] reads it, and that
/// function keys on `MAX(bwidth, bheight)` plus the origin bits, so the
/// rectangles land on the same ME PU as their square parent — which is what C
/// does, because `svt_aom_get_me_block_offset` takes the NSQ `bsize` directly.
fn pd0_bsize(bw: usize, bh: usize) -> u8 {
    use svtav1_types::block::BlockSize as B;
    let b = match (bw, bh) {
        (4, 4) => B::Block4x4,
        (8, 8) => B::Block8x8,
        (16, 16) => B::Block16x16,
        (32, 32) => B::Block32x32,
        (64, 64) => B::Block64x64,
        (128, 128) => B::Block128x128,
        (8, 4) => B::Block8x4,
        (4, 8) => B::Block4x8,
        (16, 8) => B::Block16x8,
        (8, 16) => B::Block8x16,
        (32, 16) => B::Block32x16,
        (16, 32) => B::Block16x32,
        (64, 32) => B::Block64x32,
        (32, 64) => B::Block32x64,
        (128, 64) => B::Block128x64,
        (64, 128) => B::Block64x128,
        _ => unreachable!("PD0 block shape {bw}x{bh}"),
    };
    b as u8
}

struct Pd0Ctx<'a> {
    src: &'a [u8],
    stride: usize,
    sb_x: usize,
    sb_y: usize,
    /// ALIGNED frame dims (mi-grid extent) — the spec-5.11.4 /
    /// set_blocks_to_test edge predicate is computed against these. For a
    /// 64-aligned frame every SB is complete, so `sb_x + 64 <= aligned_w`
    /// and the edge/off branches in [`Pd0Ctx::pick`] never fire.
    aligned_w: usize,
    aligned_h: usize,
    vars: SbVariance,
    qp: u32,
    qindex: u8,
    /// [SVT_HDR_MODE] Frame luma QM level (`frm_hdr.quantization_params.
    /// qm[PLANE_Y]`, from base_qindex) for the PD0 leaf quantize; 15 =
    /// identity/no matrices (mainline, and every non-bd10 fork path). Only
    /// the bd10 LVL_0 entry (`pd0_pick_sb_partition_lvl0`) sets it non-15,
    /// mirroring C's `set_pd0_ctrls` PD0_LVL_0 force at bd10 whose light
    /// encode applies QM (`svt_aom_quantize_inv_quantize_light`). Consumed by
    /// [`tx_quant_core`]; when 15 the non-QM `quantize_b` runs (byte-inert).
    qm_level: u8,
    lambda: u64,
    mode: Pd0Mode,
    lvl1: Option<&'a M6Pd0Tables>,
    max_sq: usize,
    min_sq: usize,
    /// C `ctx->is_subres_safe`: 255 = not yet determined (only a tested
    /// 64x64 block determines it); the effective per-block step is 0
    /// unless this is exactly 1.
    is_subres_safe: u8,
    /// C `input_resolution_factor[pcs->ppcs->input_resolution]`
    /// (perform_tx_pd0): the per-picture `factor * 1600` addend on the
    /// PD0_LVL_5 closed-form coeff rate. 0 for <= 240p pictures.
    ires_factor: u64,
    /// C `rate_est_ctrls.coeff_rate_est_lvl` at PD0 (perform_tx_pd0): 1
    /// (M2..M6) prices the real coeff rate; 2 (M7/M8, rate_est_level 4)
    /// uses the fast approximation `eob < th ? 6000 + eob*500 : real`
    /// (`th = (bw*bh)>>5`, bw/bh capped at 32). Only consulted by the
    /// LVL_1 block cost; LVL_5/6 use their own closed forms.
    coeff_rate_est_lvl: u8,
    /// C `ctx->nsq_geom_ctrls.enabled` (svt_aom_get_nsq_geom_level_allintra,
    /// enc_mode_config.c:8240): 1 for allintra enc_mode <= M6 (presets 0..=6,
    /// nsq_geom_level 1/2/3), 0 for enc_mode > M6 (presets >= 7, level 0).
    /// Gates `set_blocks_to_test`'s one-false force-split: when NSQ is DISABLED
    /// a one-false boundary node yields `tot_shapes = 0` (force-split, no edge
    /// shape injected) — LVL_5/6 (presets >= 9) AND the LVL_1 presets 7/8. When
    /// ENABLED (presets <= 6) a fitting one-false node keeps its single edge
    /// shape (the `sq_size <= MAX(min_nsq=4, min_nsq_block_size<=8)` term never
    /// fires for edge nodes, which are always >= 16 wide on an 8-aligned frame).
    nsq_enabled: bool,
    /// C `pcs->ppcs->use_accurate_part_ctx` (`enc_mode_config.c:8955` /
    /// `:9937`): `enc_mode <= M8` on both arms. When FALSE, C doubles the
    /// SPLIT rate to bias against splitting (`test_split_partition_pd0`,
    /// product_coding_loop.c:10446). LVL_5 / LVL_0 hardcode the doubling
    /// because they only exist above M8; the LVL_1 family spans both sides of
    /// the boundary, so it reads this.
    accurate_part_ctx: bool,
    /// C `depth_early_exit_ctrls.early_exit_th` as `test_split_partition_pd0`
    /// reads it for the i > 0 quadrants (product_coding_loop.c:10469 — a
    /// stored 0 reads as 1000).
    ///
    /// `set_depth_early_exit_ctrls` (enc_mode_config.c:7182) is driven by
    /// `depth_early_exit_lvl`, which is 1 (`early_exit_th` 0 -> 1000) when
    /// `pd0_level <= PD0_LVL_1 || ctx->pic_pred_depth_only`, and 2
    /// (`early_exit_th` **900**) otherwise (`:7232`).
    /// `pic_pred_depth_only` is `depth_refinement_ctrls.mode ==
    /// PD0_DEPTH_PRED_PART_ONLY` (`:7095`) — i.e. the same predicate that
    /// makes the port take the FIXED-TREE path instead of the refinement
    /// walk — so it is a caller fact, not a level fact, and lives here
    /// rather than being derived from [`Pd0Mode`].
    depth_early_exit_th: u128,
    /// Tile-row / tile-column pixel origin of this SB's tile (0 = single tile,
    /// i.e. byte-identical to the pre-fix frame-edge predicate). AV1 intra
    /// prediction never crosses a tile boundary, so a block at a tile's own
    /// top row / left column has NO above / left neighbour even when it is not
    /// the frame's own edge. The LVL_1 (M6) leaf-cost DC prediction — which
    /// drives the M6 PD0 partition decision — must honour this: otherwise it
    /// predicts across the tile boundary (from the frame-wide source), keeps a
    /// 64x64 NONE where C splits into 16x16/8x8 (C's `up_available` /
    /// `left_available` respect tiles at every preset), and codes a different
    /// tree. Only read by `lvl1_block_cost_rect`; LVL_5/LVL_6/LVL_0 leave these
    /// 0 so eff-M9 / bd10 are provably untouched.
    tile_top: usize,
    tile_left: usize,
    /// C `ctx->recon_neigh_y` while `pd0_use_src_samples` is FALSE — the
    /// VIDEO arm at every PD0 level (`allintra || hbd_md`,
    /// enc_mode_config.c:7309). `None` = the ALLINTRA behaviour this port has
    /// always had: predict every PD0 block from the SOURCE row/column, which
    /// is exactly what C's `md_encode_block_pd0` copies into the arrays when
    /// the flag is set (product_coding_loop.c:8370).
    ///
    /// Wired on the LVL_1 FAMILY only. LVL_5 / LVL_6 (CLI preset >= 9) are
    /// still source-predicted on both arms — a REAL remaining gap, recorded in
    /// `docs/INTER-ENCODE-PLAN.md`, not a claim that C differs there.
    recon_canvas: Option<Pd0ReconCanvas>,
    /// C `product_prediction_fun_table_pd0[1]` — the INTER arm of the PD0
    /// block prediction. `None` on every key frame and on the allintra arm,
    /// which is what keeps the still envelope byte-neutral by construction.
    inter: Option<&'a Pd0InterRef<'a>>,
    /// The recon of the block [`Pd0Ctx::lvl1_block_cost_rect`] just costed,
    /// held until [`Pd0Ctx::pick`] knows whether this node's partition was
    /// DECIDED (C only writes the arrays at the decision points, never for a
    /// block whose node ends up SPLIT).
    pending_recon: Option<alloc::vec::Vec<u8>>,
}

/// C `svt_aom_partition_rate_cost` at PD0: neighbor partition contexts are
/// 0 (never updated in PD0), `has_rows`/`has_cols` are true for the fully
/// in-picture blocks every current caller produces. Units: 1/512 bit.
fn partition_split_bits(sq_size: usize) -> u64 {
    crate::entropy::context::partition_symbol_cost(
        sq_size,
        0,
        crate::partition::PartitionType::Split as usize,
    ) as u64
}

/// Binary SPLIT-vs-{H,V} "alike" rate at a one-false boundary node, on the
/// DEFAULT partition CDF (LPD0 / PD0_LVL_5). `bottom_edge` = `!has_rows`.
fn partition_alike_split_bits(sq_size: usize, bottom_edge: bool) -> u64 {
    crate::entropy::context::partition_alike_split_symbol_cost(sq_size, bottom_edge, sq_size == 128)
        as u64
}

/// C `partition_fac_bits[0][PARTITION_NONE]`: svt_aom_full_cost_pd0 uses
/// **context index 0** — the bsl-0 (8x8 size class), sub-context-0 row —
/// as an approximation for every block size (rd_cost.c:1344-1349). 400
/// units of 1/512 bit from the default tables.
fn partition_none_bits_ctx0() -> u64 {
    crate::entropy::context::partition_symbol_cost(
        8,
        0,
        crate::partition::PartitionType::None as usize,
    ) as u64
}

/// C `skip_fac_bits[0][0]` — cost of skip=0 at context 0 from the default
/// skip CDF (icdf 1097 -> p(0) = 31671): 26 units of 1/512 bit.
fn skip0_bits() -> u64 {
    crate::entropy::context::av1_cost_symbol(32768 - 1097) as u64
}

impl<'a> Pd0Ctx<'a> {
    /// LVL_5 block cost (md_encode_block_pd0 full path). Also runs the
    /// per-SB subres-safety check when this is a 64x64 block and the
    /// safety is still undetermined (full_loop_core_pd0).
    fn lvl5_block_cost(&mut self, sq_size: usize, org_x: usize, org_y: usize) -> u64 {
        self.lvl5_like_block_cost(sq_size, org_x, org_y, 1)
    }

    /// LVL_0 block cost (bd10-forced full-RD PD0). Same closed-form encode as
    /// [`Pd0Ctx::lvl5_block_cost`] but with subres FORCED OFF (`subres_level =
    /// 0` at `pd0_level <= PD0_LVL_2`, enc_mode_config.c:7327): step is always
    /// 0, so no 8x2/16x4 sub-sampled transform and no per-SB odd/even-deviation
    /// check. Every block runs it (no PD0-level detector).
    fn lvl0_block_cost(&mut self, sq_size: usize, org_x: usize, org_y: usize) -> u64 {
        self.lvl5_like_block_cost(sq_size, org_x, org_y, 0)
    }

    /// Shared closed-form PD0 block cost (`full_loop_core_pd0` at
    /// `coeff_rate_est_lvl == 0`, `lpd0_qp_offset = 8`). `subres_step_cfg` is
    /// C's `subres_ctrls.step`: 1 for LVL_5 (the 64x64 odd/even-deviation
    /// check may then enable step-1 sub-sampling), 0 for LVL_0 (subres off ->
    /// no check, step stays 0 for every block).
    fn lvl5_like_block_cost(
        &mut self,
        sq_size: usize,
        org_x: usize,
        org_y: usize,
        subres_step_cfg: u32,
    ) -> u64 {
        self.lvl5_like_block_cost_rect(sq_size, sq_size, org_x, org_y, subres_step_cfg)
    }

    /// Non-square generalisation of [`Pd0Ctx::lvl5_like_block_cost`], the twin
    /// of [`Pd0Ctx::lvl1_block_cost_rect`] for the LIGHT PD0 closed form.
    ///
    /// `bw == bh` is the square PART_N path (unchanged); `bw != bh` costs the
    /// single in-frame PARTITION_HORZ / PARTITION_VERT block of a partial-SB
    /// boundary node. Every step below is dimension-general already — the DC
    /// predictor, the residual gather, `tx_quant_core` and the closed-form
    /// coeff rate — so this is a widening, not a second implementation.
    ///
    /// MEASURED on the C side before it was written (`SVT_PD0COST_OUT`,
    /// `gradient 72x88 q40 p9` video, the x = 64 superblock of a 72-wide
    /// frame): C prices `32x64`, `16x32` and `8x16` there, never the square.
    /// At `org=(64,0)` its `8x16` costs 2,905,600 against the two `8x8`s'
    /// 1,787,062 + 1,683,524 + split rate, so C keeps the rectangle — which is
    /// exactly the `BLOCK_8X16` the port was coding as `BLOCK_8X8` + a split.
    fn lvl5_like_block_cost_rect(
        &mut self,
        bw: usize,
        bh: usize,
        org_x: usize,
        org_y: usize,
        subres_step_cfg: u32,
    ) -> u64 {
        let abs_x = self.sb_x + org_x;
        let abs_y = self.sb_y + org_y;
        // `pd0_use_src_samples` (enc_mode_config.c:7309) is `allintra ||
        // hbd_md`. TRUE — no canvas — means C copies the SOURCE row/column into
        // the recon-neighbour arrays (product_coding_loop.c:8370), so
        // predicting straight off the source plane IS that arm, and this keeps
        // the untiled extractor it has always used (byte-neutral). FALSE — a
        // canvas — means the arrays hold PD0's own recon, and the canvas is
        // that state; the availability, `n_top_px`/`n_left_px` clamp and edge
        // replication are the same function either way.
        let (above, left, _tl, has_above, has_left) = match self.recon_canvas.as_ref() {
            None => crate::partition::extract_neighbors(
                self.src,
                self.stride,
                abs_x,
                abs_y,
                bw,
                bh,
                self.aligned_w,
                self.aligned_h,
            ),
            Some(cv) => {
                // Row axis shifted into the canvas window, exactly as
                // `lvl1_block_cost_rect` does it.
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
        let mut pred = vec![0u8; bw * bh];
        svtav1_dsp::intra_pred::predict_dc(
            &mut pred, bw, &above, &left, bw, bh, has_above, has_left,
        );

        // Subres safety: determined once per SB by the first (and only)
        // tested 64x64 block; blocks tested while it is undetermined use
        // step 0 (C forces mds_subres_step = 0 when is_subres_safe != 1).
        // When subres is off entirely (LVL_0, subres_step_cfg == 0), the
        // check never runs and every block keeps step 0.
        if subres_step_cfg > 0 && bw == 64 && bh == 64 && self.is_subres_safe == 255 {
            self.is_subres_safe = u8::from(check_is_subres_safe(
                self.src,
                self.stride,
                abs_x,
                abs_y,
                &pred,
            ));
        }
        // subres_ctrls.step for this config; the 8-tall cap is on the SHORT
        // side (C `mds_subres_step` halves rows), so it keys on `bh`.
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
        let mut residual = vec![0i32; bw * tx_h];
        svtav1_dsp::residual::residual_i32(
            &self.src[abs_y * self.stride + abs_x..],
            self.stride << step,
            &pred,
            bw << step,
            bw,
            tx_h,
            &mut residual,
        );
        let qindex_off = (self.qindex as u32 + 8).min(255) as u8; // lpd0_qp_offset = 8
        let (eob, dist, _qcoeff, _c_tx, dqcoeff) =
            tx_quant_core(&residual, bw, tx_h, qindex_off, self.qm_level, step);
        // coeff_rate_est_lvl == 0 closed form (perform_tx_pd0,
        // product_coding_loop.c:4579): 5000 + input_resolution_factor*1600 +
        // 100*eob. The resolution factor is a per-picture constant (0 for
        // <= 240p, e.g. all 64/128 synthetic cells; 1 at 360p incl. 512x512).
        let bits = 5000 + self.ires_factor * 1600 + 100 * eob as u64;
        // svt_aom_full_cost_pd0: rate = coeff bits + skip(0) bits +
        // PARTITION_NONE bits at context 0.
        let rate = bits + skip0_bits() + partition_none_bits_ctx0();
        let cost = rdcost(self.lambda, rate, dist);
        // C `md_encode_block_pd0` (product_coding_loop.c:8429): with
        // `pd0_use_src_samples` FALSE, PD0 generates the block's RECON so the
        // next block can predict from it. Same inverse-transform +
        // even/odd-row expansion as the LVL_1 family's twin — see
        // `lvl1_block_cost_rect`, whose block this mirrors.
        // C `md_encode_block_pd0` (product_coding_loop.c:8430) generates the
        // recon only `if (!ctx->skip_intra && !ctx->pd0_use_src_samples)`. On
        // an INTER frame `intra_level == 0` makes `skip_intra` 1
        // (enc_mode_config.c:6718), so C writes NOTHING into the PD0 recon
        // neighbour arrays — which is consistent, because with no intra
        // candidate nothing reads them.
        if self.recon_canvas.is_some() && self.inter.is_none() {
            let mut recon = alloc::vec![0u8; bw * bh];
            if eob > 0 {
                let packed_w = bw.min(32);
                let packed_h = tx_h.min(32);
                let mut full = alloc::vec![0i32; bw * tx_h];
                for r in 0..packed_h {
                    for c in 0..packed_w {
                        full[r * bw + c] = dqcoeff[r * packed_w + c];
                    }
                }
                let mut inv = alloc::vec![0i32; bw * tx_h];
                let (tx_size, _) = pd0_tx_size(bw, tx_h);
                svtav1_dsp::txfm_dispatch::inv_txfm2d_dispatch(
                    &full,
                    &mut inv,
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
        // `SVTAV1_PD0DBG` on the LIGHT PD0 path too. The LVL_1 family has had
        // this dump since task #95; LVL_5 had none, so the video arm's PD0 at
        // preset >= 9 — the one the fixed-tree path runs — could not be joined
        // against C's `SVT_PD0COST_OUT` (`svt_aom_full_cost_pd0`) block for
        // block. Same first four fields in the same order as that dump, so the
        // two files line up without a translation step.
        #[cfg(feature = "std")]
        if crate::dbgenv::pd0dbg() {
            eprintln!(
                "PD0BLK org=({abs_x},{abs_y}) {bw}x{bh} dist={dist} ybits={bits} cost={cost} lambda={} subres={step}",
                self.lambda,
            );
        }
        cost
    }

    /// PD0_LVL_1 block cost (md_encode_block_pd0 at allintra M2..M8):
    /// same DC-from-source prediction, but `lpd0_qp_offset = 0`, subres
    /// permanently off (`pd0_level <= PD0_LVL_2` -> subres_level 0), and
    /// the REAL coefficient rate (`coeff_rate_est_lvl = 1` ->
    /// svt_aom_txb_estimate_coeff_bits_pd0 with zero contexts).
    fn lvl1_block_cost(&mut self, sq_size: usize, org_x: usize, org_y: usize) -> u64 {
        self.lvl1_block_cost_rect(sq_size, sq_size, org_x, org_y)
    }

    /// Non-square generalisation of the PD0_LVL_1 block cost. `bw == bh` is
    /// the square PART_N path (unchanged); `bw != bh` costs the single
    /// in-frame PARTITION_HORZ / PARTITION_VERT block of a partial-SB boundary
    /// node (task #95 chunk 2) — C's LPD0 "single block per shape ... PART_H/
    /// PART_V for boundary blocks" (product_coding_loop.c:127). The DC
    /// predictor, residual, `tx_quant_core` (Tx32x16 / Tx16x8 / …) and PD0
    /// coeff-rate estimator are all dimension-general.
    fn lvl1_block_cost_rect(&mut self, bw: usize, bh: usize, org_x: usize, org_y: usize) -> u64 {
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
            let mut pred = vec![0u8; bw * bh];
            self.inter_pred_into(ir, bw, bh, abs_x, abs_y, &mut pred);
            return self.lvl1_cost_from_pred(bw, bh, abs_x, abs_y, pred, None);
        }
        let (above, left, _tl, has_above, has_left) = match self.recon_canvas.as_ref() {
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
        let mut pred = vec![0u8; bw * bh];
        svtav1_dsp::intra_pred::predict_dc(
            &mut pred, bw, &above, &left, bw, bh, has_above, has_left,
        );
        self.lvl1_cost_from_pred(
            bw,
            bh,
            abs_x,
            abs_y,
            pred,
            Some((&above, &left, has_above, has_left)),
        )
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
    fn lvl1_cost_from_pred(
        &mut self,
        bw: usize,
        bh: usize,
        abs_x: usize,
        abs_y: usize,
        pred: Vec<u8>,
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
                &pred,
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
        let mut residual = vec![0i32; bw * tx_h];
        svtav1_dsp::residual::residual_i32(
            &self.src[abs_y * self.stride + abs_x..],
            self.stride << step,
            &pred,
            bw << step,
            bw,
            tx_h,
            &mut residual,
        );
        let (eob, dist, qcoeff, c_tx, dqcoeff) =
            tx_quant_core(&residual, bw, tx_h, self.qindex, self.qm_level, step);
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
        let bits = if self.coeff_rate_est_lvl >= 2 && (eob as usize) < th {
            6000 + eob as u64 * 500
        } else if eob == 0 {
            cost_skip_txb_pd0(c_tx, &tables.coeff) as u64
        } else {
            cost_coeffs_txb_pd0(&qcoeff, eob, c_tx, &tables.coeff, &tables.tx_rates, step) as u64
        };
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
                let packed_w = bw.min(32);
                let packed_h = tx_h.min(32);
                let mut full = alloc::vec![0i32; bw * tx_h];
                for r in 0..packed_h {
                    for c in 0..packed_w {
                        full[r * bw + c] = dqcoeff[r * packed_w + c];
                    }
                }
                let mut inv = alloc::vec![0i32; bw * tx_h];
                let (tx_size, _) = pd0_tx_size(bw, tx_h);
                svtav1_dsp::txfm_dispatch::inv_txfm2d_dispatch(
                    &full,
                    &mut inv,
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
                "PD0BLK org=({},{}) {}x{} dist={} ybits={} cost={} lambda={} subres={} dc={} ha={} hl={} a0={:?} l0={:?}",
                abs_x,
                abs_y,
                bw,
                bh,
                dist,
                bits,
                cost,
                self.lambda,
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

    /// C `svt_aom_inter_pu_prediction_av1_pd0` for this block's ONE injected
    /// `NEWMV` candidate.
    ///
    /// The MV is `inject_new_candidates_pd0`'s (mode_decision.c:2320-2325):
    /// the open-loop search's FULL-PEL result for this block's ME PU, times 8.
    /// When the search left no candidate for the PU,
    /// `generate_md_stage_0_cand_pd0` falls back to
    /// `inject_zz_backup_candidate` (mode_decision.c:3511) — the ZERO MV on
    /// `LAST_FRAME` — which is the `unwrap_or` here and NOT a silent default.
    fn inter_pred_into(
        &self,
        ir: &Pd0InterRef<'_>,
        bw: usize,
        bh: usize,
        abs_x: usize,
        abs_y: usize,
        out: &mut [u8],
    ) {
        assert!(
            self.is_lvl1_family(),
            "PD0 inter compensation is wired for the LVL_1 family only, and the \
             caller resolved {:?}. C's PD0_LVL_5 / PD0_LVL_6 inter arms are \
             `compute_lpd0_cost_inter`'s variance closed form, not this block \
             encode; refusing rather than costing an inter block with the wrong \
             model. On this port's low-delay-P envelope `pd0_detector` walks \
             every picture level down to PD0_LVL_3 before a search runs \
             (docs/INTER-ENCODE-PLAN.md 1z^6), so this is unreachable today.",
            self.mode
        );
        // C `inject_new_candidates` (mode_decision.c:2320) reads the
        // `me_mv_array` slot named by the ME CANDIDATE's own `direction`, not
        // list 0's — and on this envelope the single candidate is usually
        // LIST 1's (see `inter_me_arm::FrameMe::cand_mv_for`). Reading list 0
        // unconditionally picked up a slot C never writes.
        let mv_fp = ir
            .me
            .cand_mv_for(abs_x, abs_y, pd0_bsize(bw, bh), 0)
            .map_or(svtav1_types::motion::Mv::ZERO, |(_dir, mv)| mv);
        let mv = svtav1_types::motion::Mv {
            x: mv_fp.x.saturating_mul(8),
            y: mv_fp.y.saturating_mul(8),
        };
        crate::inter_pred_arm::predict_inter_luma_pd0(
            ir.padded_y,
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

    /// The LVL_1 FAMILY — every level whose block cost is
    /// `md_encode_block_pd0` at `lpd0_qp_offset = 0` with a real (or
    /// approximated) coefficient rate, as opposed to LVL_5/LVL_0's
    /// `5000 + 100*eob` closed form or LVL_6's pure variance.
    #[inline]
    fn is_lvl1_family(&self) -> bool {
        matches!(self.mode, Pd0Mode::Lvl1 | Pd0Mode::Lvl3 | Pd0Mode::Lvl4)
    }

    /// Which PD0 modes price a one-false BOUNDARY node as its FITTING edge
    /// shape rather than as the square that does not fit.
    ///
    /// C decides this with no reference to `pd0_ctrls.pd0_level`:
    /// `set_blocks_to_test` (enc_dec_process.c:1394) injects exactly the
    /// fitting PART_H / PART_V on an incomplete node whenever NSQ geometry is
    /// enabled and the square is above `MAX(min_nsq, min_nsq_block_size)`
    /// (`:1420-1423`), and `svt_aom_pick_partition_pd0`
    /// (product_coding_loop.c:10534-10560) then costs
    /// `get_blk_geom_mds(mds_idx + ns_blk_offset_md[shape])` — the RECTANGLE.
    ///
    /// So the level list here is about what this PORT has MEASURED, not about
    /// what C does:
    /// * LVL_1 family — the allintra fixed-tree presets, wired 2026-08 (task
    ///   #95, the 96x80 milestone).
    /// * LVL_5 — added 2026-09-01 for the VIDEO arm. It could not matter on
    ///   the allintra arm, where `nsq_geom_level` is 0 above M6 so an LVL_5/6
    ///   boundary node force-splits before it can be costed at all; the video
    ///   arm never turns NSQ geometry off, so it reaches this and was pricing
    ///   the square. MEASURED against C's own `svt_aom_full_cost_pd0` dump —
    ///   see `lvl5_like_block_cost_rect`.
    /// * LVL_6 is EXCLUDED because it runs no transform at all
    ///   (`compute_lpd0_cost_allintra` / `compute_lpd0_cost_inter`), so there
    ///   is no block cost to make rectangular.
    /// * LVL_0 is EXCLUDED and that is a KNOWN GAP, not a claim about C: it is
    ///   the bd10-forced path (`set_pd0_ctrls`, enc_mode_config.c:5416), whose
    ///   partial-SB cells are byte-identical today, and nothing here has
    ///   dumped C's bd10 boundary cost. Widening it blind would trade a green
    ///   gate for a guess.
    fn prices_edge_shape(&self) -> bool {
        self.is_lvl1_family() || matches!(self.mode, Pd0Mode::Lvl5)
    }

    /// C `ctx->subres_ctrls.step` for this level on an I-slice
    /// (`svt_aom_sig_deriv_enc_dec_pd0`, enc_mode_config.c:7337-7345).
    /// LVL_5's own step is passed explicitly by its caller instead.
    #[inline]
    fn subres_step_cfg(&self) -> u32 {
        match self.mode {
            Pd0Mode::Lvl3 | Pd0Mode::Lvl4 => 1,
            _ => 0,
        }
    }

    fn block_cost(&mut self, sq_size: usize, org_x: usize, org_y: usize) -> u64 {
        match self.mode {
            Pd0Mode::Lvl1 | Pd0Mode::Lvl3 | Pd0Mode::Lvl4 => {
                self.lvl1_block_cost(sq_size, org_x, org_y)
            }
            Pd0Mode::Lvl5 => self.lvl5_block_cost(sq_size, org_x, org_y),
            Pd0Mode::Lvl0 => self.lvl0_block_cost(sq_size, org_x, org_y),
            Pd0Mode::Lvl6 => lvl6_cost_allintra(&self.vars, sq_size, org_x, org_y, self.qp),
        }
    }

    /// C `svt_aom_pick_partition_pd0` + `test_split_partition_pd0`:
    /// parent-first DFS returning (cost, eval record) for this square
    /// node; the picked tree is `eval.tree()`.
    fn pick(&mut self, sq_size: usize, org_x: usize, org_y: usize) -> (u64, Pd0Eval) {
        // The SB root is quadrant 0 of nothing: C's `mds->index` for the root
        // is 0, which only matters for the `index < 3` leaf-update rule below.
        //
        // C's `svt_aom_mode_decision_kernel` (enc_dec_process.c:2989) DISCARDS
        // `svt_aom_pick_partition_pd0`'s return value at the SB root, and an
        // invalid root leaves `pc_tree->partition` at whatever
        // `svt_aom_init_sb_data` left. The port has no such reachable case —
        // an SB root is invalid only if EVERY in-bounds descendant is, and the
        // (0,0) quadrant chain always reaches a node with `has_rows &&
        // has_cols` before `min_sq` unless `min_sq` exceeds the SB's own
        // remainder, which `set_depth_removal_level_controls`' three
        // "entire SB can be covered" guards forbid — so this returns an
        // untested leaf rather than inventing a tree.
        match self.pick_q(sq_size, org_x, org_y, 0) {
            Some((cost, eval, _)) => (cost, eval),
            None => (0, Pd0Eval::untested(sq_size)),
        }
    }

    /// [`Pd0Ctx::pick`] with C's `mds->index` (this node's quadrant inside its
    /// parent) and the recon hand-back the neighbour-array protocol needs.
    ///
    /// The third return is this node's OWN block recon when the caller still
    /// has to write it — C's `test_split_partition_pd0` tail
    /// (product_coding_loop.c:10500) updates the arrays for the LAST quadrant,
    /// which `svt_aom_pick_partition_pd0`'s `mds->index < 3` guard
    /// deliberately skips "to avoid redundant copies". `None` everywhere the
    /// node either wrote itself or must not be written (it ended SPLIT).
    /// `None` is C's `svt_aom_pick_partition_pd0` returning **false** —
    /// `pc_tree->rdc.valid == 0`, i.e. this node costed no shape AND could not
    /// split. C's caller treats that as fatal to the PARENT's split ("all
    /// split quadrants must be valid for split to be selected",
    /// product_coding_loop.c:10481), which is NOT the same as an out-of-bounds
    /// quadrant — those are `continue`d and contribute nothing
    /// ([`Pd0Eval::off`]).
    fn pick_q(
        &mut self,
        sq_size: usize,
        org_x: usize,
        org_y: usize,
        quad_idx: usize,
    ) -> Option<(u64, Pd0Eval, Option<(alloc::vec::Vec<u8>, usize, usize)>)> {
        let abs_x = self.sb_x + org_x;
        let abs_y = self.sb_y + org_y;
        // C `svt_aom_write_modes_sb` early return: a node whose top-left is
        // outside the ALIGNED frame codes nothing. Its cost never enters a
        // parent decision (parents of off-frame nodes are forced-split edge
        // nodes, which ignore cost), so 0 is inert.
        if abs_x >= self.aligned_w || abs_y >= self.aligned_h {
            return Some((0, Pd0Eval::off(sq_size), None));
        }
        // spec 5.11.4 / `set_blocks_to_test` (enc_dec_process.c:1394) edge
        // predicate vs the ALIGNED grid. `half` = half the square's pixel
        // extent (C `hbs = (mi_size_wide[bsize] << 2) >> 1`).
        let half = sq_size / 2;
        let has_rows = abs_y + half < self.aligned_h;
        let has_cols = abs_x + half < self.aligned_w;
        let one_false = !has_rows || !has_cols;
        let both_false = !has_rows && !has_cols;
        // FORCED SPLIT — `set_blocks_to_test` (enc_dec_process.c:1405) yields
        // `tot_shapes = 0`, so PART_N is NEVER costed and the node splits with
        // no NONE/edge candidate. This fires for:
        //  - a BOTH-false node (extends past both edges), at every PD0 level;
        //  - a one-false node when NSQ geom is DISABLED (`!self.nsq_enabled`).
        //    `svt_aom_get_nsq_geom_level_allintra` returns level 0 → `enabled =
        //    0` for allintra CLI preset >= M7 (enc_mode_config.c:8240), which
        //    covers BOTH the LPD0 presets >= 9 (PD0_LVL_5/6) AND the LVL_1
        //    presets 7/8. C never injects the edge shape, so EVERY one-false
        //    boundary node force-splits, descending to the fitting sub-blocks
        //    (e.g. a thin 8-wide right edge -> all 8x8). Presets <= M6 keep NSQ
        //    enabled → the one-false edge-shape path below (`one_false &&
        //    self.nsq_enabled`), matching the M6 boundary milestone. (The C
        //    `sq_size <= MAX(min_nsq=4, min_nsq_block_size<=8)` term is inert:
        //    edge nodes are always >= 16 wide on an 8-aligned frame.)
        // 8x8 nodes are never edge nodes on an 8-aligned frame, so a
        // force-split node always has `sq_size > min_sq` and can split. A
        // has_rows && has_cols node can still STRADDLE the aligned extent (its
        // sq x sq block reaching past aligned); C codes such straddle blocks
        // reading its SB-extent pad and cropping the distortion, so the port
        // sizes the recon + chroma-source buffers to the SB extent — a
        // straddling block writes into the padded rows, never out of bounds.
        let forced_split = both_false || (one_false && !self.nsq_enabled);
        // C `init_md_scan` (enc_dec_process.c:1457): `mds->split_flag =
        // (sq_size > min_sq_size)` is set from the SIZE ALONE and is what
        // gates the recursion — `set_blocks_to_test`'s `tot_shapes = 0` says
        // only that no d1 SHAPE is costable here, never that the node splits.
        // When both hold the node is simply INVALID: no cost, no children,
        // `svt_aom_pick_partition_pd0` returns false.
        //
        // MEASURED 2026-09-02: this port force-split such a node anyway. On an
        // INTER frame `depth_removal_ctrls` raises `min_sq` above 8 — on a
        // superblock whose cropped extent is 40 px, `dimensions_require_8x8`
        // is false so `disallow_below_16x16` may arm and `min_sq` becomes 16 —
        // and the 16x16 both-false quadrant at the picture's bottom-right then
        // descended to 8x8, where `sq_size >= min_sq` is false, no cost
        // exists, and the leaf `expect` fired: "leaf must be tested (min_sq <=
        // size <= max_sq)". 14 of the 64 cells of
        // `tools/inter_completion_scan.sh` crashed there — every size
        // congruent to 40 mod 64 at preset 8/10/13. The old code carried the
        // reasoning that made it look safe: "8x8 nodes are never edge nodes on
        // an 8-aligned frame, so a force-split node always has sq_size >
        // min_sq". True at `min_sq == 8`, which is the only value a KEY frame
        // ever has, and false the moment depth removal raises it.
        if forced_split && sq_size <= self.min_sq {
            return None;
        }
        if forced_split {
            let mut children: Vec<Pd0Eval> = Vec::with_capacity(4);
            let mut total = 0u64;
            let mut last_recon: Option<(alloc::vec::Vec<u8>, usize, usize)> = None;
            let mut last_quad_valid = true;
            for i in 0..4 {
                let cx = org_x + (i & 1) * half;
                let cy = org_y + (i >> 1) * half;
                if self.sb_x + cx >= self.aligned_w || self.sb_y + cy >= self.aligned_h {
                    last_quad_valid = false;
                }
                // C `test_split_partition_pd0` returns false at the FIRST
                // invalid quadrant (:10481), before the array-update tail. A
                // force-split node has no shape of its own to fall back on, so
                // it is invalid too.
                let (c_cost, c_eval, c_recon) = self.pick_q(half, cx, cy, i)?;
                total += c_cost;
                if i == 3 {
                    last_recon = c_recon;
                }
                children.push(c_eval);
            }
            // C `test_split_partition_pd0`'s tail with an INVALID parent
            // (`tot_shapes == 0` -> `pc_tree->rdc.valid == 0`): split always
            // wins, so the last quadrant is the array-update part.
            if last_quad_valid && let Some((r, rw, rh)) = last_recon {
                self.write_recon(
                    self.sb_x + org_x + half,
                    self.sb_y + org_y + half,
                    rw,
                    rh,
                    Some(&r),
                );
            }
            // SPLIT rate feeding a STRADDLING parent's decision (the failing
            // thin-edge cells are self-contained from the SB root, where this
            // is inert; a straddling root like 48x48 consumes it). A both-false
            // node codes NO partition symbol -> rate 0; a one-false node codes
            // the BINARY SPLIT-vs-{H,V} symbol -> its alike rate (doubled at
            // LVL_5 since `use_accurate_part_ctx = 0`; 0 at LVL_6 allintra,
            // test_split_partition_pd0:10435; UNdoubled at LVL_1 presets 7/8
            // since `use_accurate_part_ctx = 1` at M7/M8, from this SB's
            // chained tables — the same boundary rate the preset<=6 edge-shape
            // node's split cost uses below).
            if !both_false {
                total += match self.mode {
                    // LVL_0 and LVL_5 both have `use_accurate_part_ctx = 0`
                    // (allintra above M8) -> the boundary SPLIT rate is doubled.
                    Pd0Mode::Lvl5 | Pd0Mode::Lvl0 => rdcost(
                        self.lambda,
                        2 * partition_alike_split_bits(sq_size, !has_rows),
                        0,
                    ),
                    Pd0Mode::Lvl6 => 0,
                    Pd0Mode::Lvl1 | Pd0Mode::Lvl3 | Pd0Mode::Lvl4 => {
                        let tables = self.lvl1.expect("LVL_1 family requires tables");
                        let mult = if self.accurate_part_ctx { 1 } else { 2 };
                        rdcost(
                            self.lambda,
                            mult * tables.boundary_split_bits(sq_size, !has_rows),
                            0,
                        )
                    }
                };
            }
            let ch: [Pd0Eval; 4] = children.try_into().expect("4 children");
            let eval = Pd0Eval {
                sq: sq_size,
                tested: false,
                sq_tested: false,
                cost: 0,
                split: true,
                off: false,
                children: Some(Box::new(ch)),
            };
            return Some((total, eval, None));
        }
        // A FITTING one-false node prices its EDGE SHAPE block, not the square
        // PART_N — C's LPD0 costs "PART_H/PART_V for boundary blocks"
        // (product_coding_loop.c:127). The square block would over-cost (twice
        // the pixels/coeffs) and wrongly lose to SPLIT. This "don't split" cost
        // competes with SPLIT exactly like the square path; a win makes the
        // node a PD0 leaf, coded as its (fitting) edge shape at
        // `encode_fixed_tree`. Only wired on the LVL_1 path (allintra
        // fixed-tree presets, incl. the 96x80 milestone); LVL_5/6 boundary
        // nodes keep the square cost.

        let tested = sq_size <= self.max_sq && sq_size >= self.min_sq;
        let parent_cost = if tested {
            if one_false && self.prices_edge_shape() {
                let (bw, bh) = if !has_rows {
                    (sq_size, half)
                } else {
                    (half, sq_size)
                };
                if self.is_lvl1_family() {
                    Some(self.lvl1_block_cost_rect(bw, bh, org_x, org_y))
                } else {
                    // LVL_5's own closed form, with its subres step.
                    Some(self.lvl5_like_block_cost_rect(bw, bh, org_x, org_y, 1))
                }
            } else {
                Some(self.block_cost(sq_size, org_x, org_y))
            }
        } else {
            None
        };
        // The node's own block recon, taken before the children can overwrite
        // `pending_recon`. Its DIMENSIONS are the shape that was costed.
        let node_recon = self.pending_recon.take();
        let (node_w, node_h) = if one_false && self.prices_edge_shape() {
            if !has_rows {
                (sq_size, half)
            } else {
                (half, sq_size)
            }
        } else {
            (sq_size, sq_size)
        };
        let mut eval = Pd0Eval {
            sq: sq_size,
            tested,
            // C `tested_blk[PART_N][0]`: a one-false node costs its injected
            // PART_H/PART_V, so the SQUARE slot stays untested
            // (svt_aom_pick_partition_pd0, product_coding_loop.c:10548-10560).
            // `one_false` is never true on a 64-aligned frame.
            sq_tested: tested && !one_false,
            cost: parent_cost.unwrap_or(0),
            split: false,
            off: false,
            children: None,
        };

        let split_flag = sq_size > self.min_sq;
        if !split_flag {
            // `parent_cost` is `Some` here because the recursion never
            // descends below `min_sq` (this branch is what stops it) and
            // `min_sq <= max_sq` — C asserts exactly that in
            // `set_blocks_to_be_tested` (enc_dec_process.c:1500), having
            // clamped `min_sq_size` to `static_config.max_tx_size` on the
            // preceding line for this reason. `pipeline.rs` applies the same
            // clamp when it fills `Pd0InterRef::min_sq`.
            let cost = parent_cost.expect(
                "a node at min_sq must be costable: C clamps min_sq_size to max_tx_size and \
                 asserts min_sq_size <= max_sq_size (enc_dec_process.c:1499-1500)",
            );
            // C `svt_aom_pick_partition_pd0` (product_coding_loop.c:10568):
            // a leaf updates the neighbour arrays itself for quadrants 0..2;
            // quadrant 3 is left to the parent's tail.
            if quad_idx < 3 {
                self.write_recon(abs_x, abs_y, node_w, node_h, node_recon.as_deref());
                return Some((cost, eval, None));
            }
            return Some((cost, eval, node_recon.map(|r| (r, node_w, node_h))));
        }

        // test_split_partition_pd0: split rate term (0 at LVL_6 allintra;
        // doubled at LVL_5 because use_accurate_part_ctx = 0 at eff-M9;
        // RAW at LVL_1 because use_accurate_part_ctx = 1 at M2..M8 —
        // observed 1195/1465/2020 in the instrumented PD0SPLITRATE dumps).
        let mut split_cost = match self.mode {
            Pd0Mode::Lvl6 => 0,
            // LVL_0/LVL_5: `use_accurate_part_ctx = 0` -> SPLIT rate doubled,
            // priced from the DEFAULT partition CDF (ctx row 0).
            //
            // At a one-false BOUNDARY node the alphabet is BINARY
            // (split-vs-{H,V}, `svt_aom_partition_rate_cost` rd_cost.c:1846-
            // 1863), so the rate is the alike cost, not the full-alphabet
            // SPLIT cost — the same distinction the LVL_1 branch below makes.
            // It only matters where the node's non-split candidate exists,
            // i.e. exactly where `prices_edge_shape()` is true, so LVL_0 keeps
            // the full-alphabet rate along with the square cost.
            Pd0Mode::Lvl5 if one_false && self.prices_edge_shape() => rdcost(
                self.lambda,
                2 * partition_alike_split_bits(sq_size, !has_rows),
                0,
            ),
            Pd0Mode::Lvl5 | Pd0Mode::Lvl0 => {
                rdcost(self.lambda, 2 * partition_split_bits(sq_size), 0)
            }
            Pd0Mode::Lvl1 | Pd0Mode::Lvl3 | Pd0Mode::Lvl4 => {
                let tables = self.lvl1.expect("LVL_1 family requires tables");
                // C `svt_aom_partition_rate_cost` (rd_cost.c:1846-1863): at a
                // one-false BOUNDARY node the SPLIT rate is the BINARY
                // split-vs-{H,V} cost (`partition_{vert,horz}_alike_fac_bits`),
                // not the full-alphabet `partition_fac_bits[ctx][SPLIT]`. Only
                // the LVL_1 family prices the edge shape (parent_cost), so only
                // it needs the matching boundary split rate; interior nodes and
                // LVL_5/6 keep the full-alphabet `split_bits`.
                let sbits = if one_false {
                    tables.boundary_split_bits(sq_size, !has_rows)
                } else {
                    tables.split_bits(sq_size)
                };
                // `use_accurate_part_ctx = 0` (enc_mode > M8) doubles it, the
                // same bias LVL_5 / LVL_0 hardcode.
                let mult = if self.accurate_part_ctx { 1 } else { 2 };
                rdcost(self.lambda, mult * sbits, 0)
            }
        };

        let half = sq_size / 2;
        let mut children: Vec<Pd0Eval> = Vec::with_capacity(4);
        let mut split_valid = true;
        let mut last_recon: Option<(alloc::vec::Vec<u8>, usize, usize)> = None;
        let mut last_quad_valid = true;
        let mut last_child_split = false;
        for i in 0..4 {
            let cx = org_x + (i & 1) * half;
            let cy = org_y + (i >> 1) * half;
            // C `test_split_partition_pd0` (product_coding_loop.c:10456):
            // a quadrant whose ORIGIN is outside the mi grid is `continue`d
            // BEFORE the depth-early-exit test, not after it. The port used to
            // run the test on those quadrants too, and because an out-of-bounds
            // child contributes 0 to `split_cost` the extra test at i == 3 can
            // fire on a running total that C has already finished accumulating
            // — turning C's "split wins" into the port's "parent wins".
            //
            // MEASURED on `gradient 72x88 q40 p5` video, SB1's 16x16 node at
            // (64,16): parent 4972162 vs split 4700296, so C splits; the port's
            // i == 3 test (`4972162 * 900 <= 4700296 * 1000`) fired and kept the
            // parent. Visible only once PD0 predicts from its own recon, because
            // the wrong winner is also what gets written into the neighbour
            // arrays — the block below then predicted off an 8x16's bottom row
            // where C uses an 8x8's.
            if self.sb_x + cx >= self.aligned_w || self.sb_y + cy >= self.aligned_h {
                last_quad_valid = false;
                children.push(Pd0Eval::off(half));
                continue;
            }
            // Early exits (disabled entirely for allintra LVL_6): th =
            // split_cost_th(50) for i == 0, else early_exit_th(0 -> 1000);
            // parent_cost_bias = 1000. Identical ths at LVL_5 and LVL_1
            // (depth_early_exit level 1 for both, enc_mode_config.c:9282).
            if self.mode != Pd0Mode::Lvl6
                && let Some(pc) = parent_cost
            {
                let th: u128 = if i == 0 { 50 } else { self.depth_early_exit_th };
                if (pc as u128) * th * 1000 <= (split_cost as u128) * 1_000_000 {
                    split_valid = false;
                    break;
                }
            }
            // C `test_split_partition_pd0` (:10479-10483): "If split is
            // invalid, then exit (all split quadrants must be valid for split
            // to be selected)." Same effect as the depth-early-exit above —
            // the split is abandoned and the node keeps its own shape — so it
            // shares that arm rather than duplicating it.
            let Some((child_cost, child_eval, child_recon)) = self.pick_q(half, cx, cy, i) else {
                split_valid = false;
                break;
            };
            split_cost += child_cost;
            if i == 3 {
                last_recon = child_recon;
                last_child_split = child_eval.split;
            }
            children.push(child_eval);
        }

        // Record the visited children (C: their pc_tree nodes were
        // populated by the recursion even when the parent ends NONE);
        // quadrants skipped by the early exit stay untested.
        if !children.is_empty() {
            while children.len() < 4 {
                children.push(Pd0Eval::untested(half));
            }
            let ch: [Pd0Eval; 4] = children.try_into().expect("4 children");
            eval.children = Some(Box::new(ch));
        }

        if !split_valid {
            // The depth-early-exit arm cannot get here without a parent cost
            // (it reads one to fire), but the invalid-child arm can: a node
            // whose `sq_size` is outside `[min_sq, max_sq]` costs no shape
            // (C `init_md_scan`'s `test_depth`), and if its split is also
            // invalid then `pc_tree->rdc.valid` stays 0 and
            // `svt_aom_pick_partition_pd0` returns false to ITS parent.
            let Some(cost) = parent_cost else {
                return None;
            };
            // C `svt_aom_pick_partition_pd0` (:10564): `if (!valid_part &&
            // pc_tree->rdc.valid) mode_decision_update_neighbor_arrays_pd0`.
            // The abandoned split's children may already have written; the
            // node's own recon now supersedes them, exactly as in C.
            self.write_recon(abs_x, abs_y, node_w, node_h, node_recon.as_deref());
            return Some((cost, eval, None));
        }

        // parent_cost_bias = 1000 (allintra): parent wins on <=.
        if let Some(pc) = parent_cost
            && pc * 1000 <= split_cost * 1000
        {
            // C `test_split_partition_pd0` (:10490): the parent keeps its
            // partition, so IT is the array-update part.
            self.write_recon(abs_x, abs_y, node_w, node_h, node_recon.as_deref());
            return Some((pc, eval, None));
        }
        eval.split = true;
        // Split wins: the array-update part is the LAST quadrant, and only
        // when it is in bounds and not itself split (:10496-10508).
        if last_quad_valid
            && !last_child_split
            && let Some((r, rw, rh)) = last_recon
        {
            self.write_recon(
                self.sb_x + org_x + half,
                self.sb_y + org_y + half,
                rw,
                rh,
                Some(&r),
            );
        }
        Some((split_cost, eval, None))
    }

    /// C `mode_decision_update_neighbor_arrays_pd0` (product_coding_loop.c:121)
    /// — a no-op on the ALLINTRA arm, where `pd0_use_src_samples` short-circuits
    /// it and the port carries no canvas.
    fn write_recon(
        &mut self,
        abs_x: usize,
        abs_y: usize,
        bw: usize,
        bh: usize,
        recon: Option<&[u8]>,
    ) {
        if let Some(cv) = self.recon_canvas.as_mut()
            && let Some(r) = recon
            && r.len() >= bw * bh
        {
            #[cfg(feature = "std")]
            if crate::dbgenv::pd0dbg() {
                eprintln!(
                    "PD0WR org=({abs_x},{abs_y}) {bw}x{bh} lastrow={:?}",
                    &r[(bh - 1) * bw..(bh - 1) * bw + bw.min(8)]
                );
            }
            cv.write(abs_x, abs_y, bw, bh, r);
        }
    }
}

/// Decide the partition tree of one 64x64 superblock exactly like the C
/// PD0 pass at allintra effective-M9 (CLI preset >= 9).
///
/// `src` is the full luma plane (64-aligned frame, the caller's padding
/// convention), `qp` the CLI 0..63 qp, `qindex` the frame base_q_idx.
#[allow(clippy::too_many_arguments)]
pub fn pd0_pick_sb_partition(
    src: &[u8],
    stride: usize,
    sb_x: usize,
    sb_y: usize,
    qp: u32,
    qindex: u8,
    // C's frame `lambda_weight` (`pcs->lambda_weight`,
    // enc_mode_config.c:10093-10115), resolved ONCE per frame by
    // [`frame_lambda_weight`] and multiplied into every MD lambda
    // (md_process.c:747-751). Passed in rather than re-derived from `qp`
    // because it is keyed on `ppcs->picture_qp` (the qindex-derived value,
    // which a fractional CRF moves off `static_config.qp`) and because the
    // tune-IQ curve and the extended-CRF bump are frame-level facts this
    // function cannot see. `frame_lambda_weight(qp, false, 0)` reproduces
    // the pre-fractional-CRF value exactly.
    lambda_weight: u32,
    ires_factor: u64,
    aligned_w: usize,
    aligned_h: usize,
    // Superres chunk B.4: C's `pcs->variance` is computed by picture analysis
    // on the FULL-RESOLUTION picture and `scale_pcs_params` (resize.c:1434)
    // re-inits the b64/SB geometry for the coded size WITHOUT recomputing it —
    // so under superres the PD0 gates read full-res variances through
    // coded-grid indices. `Some(v)` hands this SB that stale entry; `None`
    // (every non-superres path) recomputes from the source exactly as before.
    stale_vars: Option<&SbVariance>,
    // C `static_config.max_tx_size` (32 or 64). At 32 the partition search may
    // not use 64x64 squares: `max_sq_size = MIN(max_sq_size, 32)`
    // (enc_dec_process.c:1494-1495), and the depth-refinement applies the same
    // cap (:1815). 64 = no cap = the pre-tune-IQ behaviour.
    max_tx_size: u8,
) -> Pd0Tree {
    let vars = match stale_vars {
        Some(v) => *v,
        None => compute_b64_variance(src, stride, sb_x, sb_y),
    };
    let max_sq = max_block_size_allintra(vars.0[0], qp).min(max_tx_size as usize);
    let mode = if pd0_detector_allintra_demotes(&vars, qp) {
        Pd0Mode::Lvl5
    } else {
        Pd0Mode::Lvl6
    };
    let lambda = kf_full_lambda_8bit_lw(qindex, lambda_weight) as u64;
    let mut ctx = Pd0Ctx {
        src,
        stride,
        sb_x,
        sb_y,
        aligned_w,
        aligned_h,
        vars,
        qp,
        qindex,
        // Non-bd10 PD0 paths never carry a live QM level (mainline QM-off; the
        // bd8 fork LVL_5/LVL_6 path is left byte-inert per the fork-bd10 scope).
        qm_level: 15,
        lambda,
        mode,
        lvl1: None,
        max_sq,
        // disallow_4x4 = 1 (pic_disallow_4x4 for these presets),
        // disallow_8x8_allintra() = false, no depth removal flags.
        min_sq: 8,
        // C enc_mode_config.c:7326: LVL_5 subres is forced OFF (level 0) on an
        // INCOMPLETE b64 (`!b64_geom->is_complete_b64`, i.e. an SB whose 64x64
        // extent reaches past the ALIGNED frame). Seed is_subres_safe to the
        // "determined, not safe" sentinel (0) on such SBs so the 64x64
        // odd/even-deviation check never runs and every LVL_5 block keeps
        // step 0 — matching C, which computes the full-res transform there.
        // Complete SBs keep 255 (the 64x64 block determines subres exactly as
        // before — byte-neutral for every full-SB cell).
        is_subres_safe: if sb_x + 64 <= aligned_w && sb_y + 64 <= aligned_h {
            255
        } else {
            0
        },
        ires_factor,
        // LVL_5/6 use their own closed-form coeff rates; unused here.
        coeff_rate_est_lvl: 0,
        // eff-M9 (preset >= 9) => enc_mode > M6 => nsq_geom_level 0 =>
        // NSQ disabled: every one-false boundary node force-splits.
        accurate_part_ctx: true,
        depth_early_exit_th: 1000,
        nsq_enabled: false,
        tile_top: 0,
        tile_left: 0,
        recon_canvas: None,
        inter: None,
        pending_recon: None,
    };
    let (_cost, eval) = ctx.pick(64, 0, 0);
    eval.tree()
}

/// Decide the partition tree of one 64x64 superblock exactly like C's
/// **PD0_LVL_0** full-RD pass — the level `set_pd0_ctrls`
/// (enc_mode_config.c:5415) FORCES at bit-depth 10 (`hbd_md` set),
/// regardless of preset. PD0 runs at 8-bit, so `src` is the 8-bit
/// MSB-truncated luma plane (the same plane the bd8 pickers read); the
/// resulting tree is fed to `pipeline::bd10_reencode_luma`, which recomputes
/// the bd10 coded levels + recon over this fixed partition.
///
/// Differences from the eff-M9 (LVL_6/LVL_5) entry
/// [`pd0_pick_sb_partition`]:
/// - NO PD0-level detector: every block runs the full closed-form encode
///   (`lvl0_block_cost` = LVL_5 cost with subres OFF), never the LVL_6
///   variance heuristic — this is the whole point (the heuristic over-splits
///   where the full-RD keeps the parent);
/// - the 64x64 variance cap on the depth set still applies
///   (`get_max_block_size_allintra`, bit-depth-independent), so a busy SB
///   (`var64 > qp-scaled 7500`) force-splits the 64x64 to 32x32 exactly as at
///   bd8;
/// - split rate DOUBLED (`use_accurate_part_ctx = 0` above M8), like LVL_5.
#[allow(clippy::too_many_arguments)]
pub fn pd0_pick_sb_partition_lvl0(
    src: &[u8],
    stride: usize,
    sb_x: usize,
    sb_y: usize,
    qp: u32,
    qindex: u8,
    // C's frame `lambda_weight` (`pcs->lambda_weight`,
    // enc_mode_config.c:10093-10115), resolved ONCE per frame by
    // [`frame_lambda_weight`] and multiplied into every MD lambda
    // (md_process.c:747-751). Passed in rather than re-derived from `qp`
    // because it is keyed on `ppcs->picture_qp` (the qindex-derived value,
    // which a fractional CRF moves off `static_config.qp`) and because the
    // tune-IQ curve and the extended-CRF bump are frame-level facts this
    // function cannot see. `frame_lambda_weight(qp, false, 0)` reproduces
    // the pre-fractional-CRF value exactly.
    lambda_weight: u32,
    // [SVT_HDR_MODE] Frame luma QM level (base_qindex-derived
    // `frm_hdr.quantization_params.qm[PLANE_Y]`); 15 = no matrices. C forces
    // PD0_LVL_0 at bd10 and its light encode applies QM when using_qmatrix
    // (fork default), so this is the ONLY PD0 entry that carries a live QM
    // level. Mainline / QM-off callers pass 15 (byte-inert non-QM path).
    qm_level: u8,
    ires_factor: u64,
    aligned_w: usize,
    aligned_h: usize,
    // Superres chunk B.4: C's `pcs->variance` is computed by picture analysis
    // on the FULL-RESOLUTION picture and `scale_pcs_params` (resize.c:1434)
    // re-inits the b64/SB geometry for the coded size WITHOUT recomputing it —
    // so under superres the PD0 gates read full-res variances through
    // coded-grid indices. `Some(v)` hands this SB that stale entry; `None`
    // (every non-superres path) recomputes from the source exactly as before.
    stale_vars: Option<&SbVariance>,
    // C `static_config.max_tx_size` (32 or 64). At 32 the partition search may
    // not use 64x64 squares: `max_sq_size = MIN(max_sq_size, 32)`
    // (enc_dec_process.c:1494-1495), and the depth-refinement applies the same
    // cap (:1815). 64 = no cap = the pre-tune-IQ behaviour.
    max_tx_size: u8,
) -> Pd0Tree {
    let vars = match stale_vars {
        Some(v) => *v,
        None => compute_b64_variance(src, stride, sb_x, sb_y),
    };
    let max_sq = max_block_size_allintra(vars.0[0], qp).min(max_tx_size as usize);
    let lambda = kf_full_lambda_8bit_lw(qindex, lambda_weight) as u64;
    let mut ctx = Pd0Ctx {
        src,
        stride,
        sb_x,
        sb_y,
        aligned_w,
        aligned_h,
        vars,
        qp,
        qindex,
        qm_level,
        lambda,
        mode: Pd0Mode::Lvl0,
        lvl1: None,
        max_sq,
        min_sq: 8,
        // subres OFF (LVL_0 is pd0_level <= PD0_LVL_2 -> subres_level 0). The
        // "determined, not safe" sentinel (0) makes lvl5_like_block_cost keep
        // step 0 for every block AND skip the 64x64 odd/even-deviation check.
        is_subres_safe: 0,
        ires_factor,
        // coeff_rate_est_lvl 0 (PD0 rate_est_level 0 above M8): closed-form
        // coeff rate. Unused by the LVL_0/LVL_5 closed forms directly (they
        // read `ires_factor`), kept 0 for consistency.
        coeff_rate_est_lvl: 0,
        // enc_mode > M6 => nsq_geom_level 0 => NSQ disabled: one-false
        // boundary nodes force-split (inert on 64-aligned frames).
        accurate_part_ctx: true,
        depth_early_exit_th: 1000,
        nsq_enabled: false,
        tile_top: 0,
        tile_left: 0,
        recon_canvas: None,
        inter: None,
        pending_recon: None,
    };
    let (_cost, eval) = ctx.pick(64, 0, 0);
    eval.tree()
}

/// Decide the partition tree of one 64x64 superblock exactly like the C
/// PD0 pass at allintra M2..M8 (`pic_pd0_lvl = 1` -> PD0_LVL_1,
/// depth-refinement level 10 -> PRED_PART_ONLY, so this tree IS the coded
/// tree). Differences from the eff-M9 entry above, all instrumented-C
/// verified (docs/IDENTITY-STATUS.md M6 chunk):
/// - no variance cap on the depth set (`base_var_th_cap = ~0` below M8):
///   the 64x64 depth is always evaluated;
/// - no PD0-level detector (`use_pd0_detector[PD0_LVL_1] = 0`): every SB
///   runs the LVL_1 block encode;
/// - LVL_1 block costs (real coeff rate, qindex+0, no subres);
/// - split rate NOT doubled (`use_accurate_part_ctx = 1`).
///
/// `tables` carries the frame-level default cost tables (C
/// `md_frame_context` for the first SB; the per-SB refresh from the
/// evolving frame context under `cdf_ctrl.enabled` is not yet ported).
#[allow(clippy::too_many_arguments)]
pub fn pd0_pick_sb_partition_m6(
    src: &[u8],
    stride: usize,
    sb_x: usize,
    sb_y: usize,
    qp: u32,
    qindex: u8,
    // C's frame `lambda_weight` (`pcs->lambda_weight`,
    // enc_mode_config.c:10093-10115), resolved ONCE per frame by
    // [`frame_lambda_weight`] and multiplied into every MD lambda
    // (md_process.c:747-751). Passed in rather than re-derived from `qp`
    // because it is keyed on `ppcs->picture_qp` (the qindex-derived value,
    // which a fractional CRF moves off `static_config.qp`) and because the
    // tune-IQ curve and the extended-CRF bump are frame-level facts this
    // function cannot see. `frame_lambda_weight(qp, false, 0)` reproduces
    // the pre-fractional-CRF value exactly.
    lambda_weight: u32,
    tables: &M6Pd0Tables,
    coeff_rate_est_lvl: u8,
    nsq_enabled: bool,
    aligned_w: usize,
    aligned_h: usize,
    // Superres chunk B.4: C's `pcs->variance` is computed by picture analysis
    // on the FULL-RESOLUTION picture and `scale_pcs_params` (resize.c:1434)
    // re-inits the b64/SB geometry for the coded size WITHOUT recomputing it —
    // so under superres the PD0 gates read full-res variances through
    // coded-grid indices. `Some(v)` hands this SB that stale entry; `None`
    // (every non-superres path) recomputes from the source exactly as before.
    stale_vars: Option<&SbVariance>,
    // C `static_config.max_tx_size` (32 or 64). At 32 the partition search may
    // not use 64x64 squares: `max_sq_size = MIN(max_sq_size, 32)`
    // (enc_dec_process.c:1494-1495), and the depth-refinement applies the same
    // cap (:1815). 64 = no cap = the pre-tune-IQ behaviour.
    max_tx_size: u8,
) -> Pd0Tree {
    let vars = match stale_vars {
        Some(v) => *v,
        None => compute_b64_variance(src, stride, sb_x, sb_y),
    };
    let lambda = kf_full_lambda_8bit_lw(qindex, lambda_weight) as u64;
    let mut ctx = Pd0Ctx {
        src,
        stride,
        sb_x,
        sb_y,
        aligned_w,
        aligned_h,
        vars,
        qp,
        qindex,
        // Non-bd10 PD0 paths never carry a live QM level (mainline QM-off; the
        // bd8 fork LVL_5/LVL_6 path is left byte-inert per the fork-bd10 scope).
        qm_level: 15,
        lambda,
        mode: Pd0Mode::Lvl1,
        lvl1: Some(tables),
        max_sq: 64usize.min(max_tx_size as usize),
        min_sq: 8,
        is_subres_safe: 255,
        ires_factor: 0,
        coeff_rate_est_lvl,
        accurate_part_ctx: true,
        depth_early_exit_th: 1000,
        nsq_enabled,
        tile_top: 0,
        tile_left: 0,
        recon_canvas: None,
        inter: None,
        pending_recon: None,
    };
    let (_cost, eval) = ctx.pick(64, 0, 0);
    eval.tree()
}

/// The two `Pd0Ctx` fields an INTER frame resolves differently from a key
/// frame, in ONE place so the video-arm entry points cannot drift apart.
///
/// * **the MD lambda.** `full_sb_lambda_md[EB_8_BIT_MD]` is the FRAME's MD
///   lambda (`av1_lambda_assign_md`, md_process.c:763), and the KF chain only
///   on a key frame. MEASURED through C's own `SVT_PD0COST_OUT` on
///   `gradient 64x64 q20 p6 frames=2`: 1407 on frame 0 and **24898** on
///   frame 1, where the KF chain at that frame's qindex gives 25650.
/// * **`min_sq_size`.** `set_blocks_to_be_tested` (enc_dec_process.c:1485)
///   reads `depth_removal_ctrls`, which `set_depth_removal_level_controls`
///   returns `enabled = 0` for on an I-slice — so a key frame is always the
///   `disallow_4x4 ? 8 : 4` arm and only a non-key frame can raise it. Same
///   cell: C's `SVT_PD0CFG_OUT` reports `dr=1/0/1/1` on frame 1
///   (`disallow_below_32x32`), and C's PD0 evaluates exactly FIVE nodes there
///   — one 64x64 and four 32x32 — against 80 when `min_sq` is left at 8.
///
/// The third inter-only fact, PD0's ONE candidate being an inter `NEWMV`
/// rather than the DC intra prediction, is the `Pd0Ctx::inter` field itself
/// (`product_prediction_fun_table_pd0[is_inter_mode(mode)]`,
/// product_coding_loop.c:970).
fn pd0_frame_lambda_and_min_sq(
    qindex: u8,
    lambda_weight: u32,
    key_min_sq: usize,
    inter: Option<&Pd0InterRef<'_>>,
) -> (u64, usize) {
    match inter {
        None => (
            kf_full_lambda_8bit_lw(qindex, lambda_weight) as u64,
            key_min_sq,
        ),
        Some(ir) => (
            inter_full_lambda_8bit(
                qindex,
                ir.base_update_type,
                ir.factor_update_type,
                ir.alt_lambda_factors,
                ir.me_qdiff,
                lambda_weight,
            ) as u64,
            ir.min_sq,
        ),
    }
}

/// [`pd0_pick_sb_partition_m6`] returning the full evaluation record —
/// the PD1 depth refinement's input (per-node tested/cost like C's
/// `pc_tree` after PD0).
///
/// `min_sq`: 8 when `disallow_4x4` (presets >= 4), else 4 — C
/// `set_blocks_to_be_tested` (enc_dec_process.c:1494: depth removal off
/// on the allintra still path, `ctx->disallow_4x4 ? 8 : 4`); the PD0B
/// capture rows confirm C's LPD0 evaluates 4x4 blocks at M2/M3.
#[allow(clippy::too_many_arguments)]
pub fn pd0_pick_sb_partition_m6_eval(
    src: &[u8],
    stride: usize,
    sb_x: usize,
    sb_y: usize,
    qp: u32,
    qindex: u8,
    // C's frame `lambda_weight` (`pcs->lambda_weight`,
    // enc_mode_config.c:10093-10115), resolved ONCE per frame by
    // [`frame_lambda_weight`] and multiplied into every MD lambda
    // (md_process.c:747-751). Passed in rather than re-derived from `qp`
    // because it is keyed on `ppcs->picture_qp` (the qindex-derived value,
    // which a fractional CRF moves off `static_config.qp`) and because the
    // tune-IQ curve and the extended-CRF bump are frame-level facts this
    // function cannot see. `frame_lambda_weight(qp, false, 0)` reproduces
    // the pre-fractional-CRF value exactly.
    lambda_weight: u32,
    tables: &M6Pd0Tables,
    min_sq: usize,
    coeff_rate_est_lvl: u8,
    // Which PD0 block-encode path prices a block. The ALLINTRA arm's level at
    // every preset this entry point serves is PD0_LVL_1
    // (`set_pic_pd0_lvl_allintra`); the VIDEO arm's is PD0_LVL_3 at M3..M7
    // (`set_pic_pd0_lvl_default`), which is the same block cost plus subres
    // step 1 — see [`Pd0Mode::Lvl3`].
    mode: Pd0Mode,
    // C `depth_early_exit_ctrls.early_exit_th` for the i > 0 quadrants, as
    // `test_split_partition_pd0` reads it: 1000 when `pd0_level <= PD0_LVL_1
    // || ctx->pic_pred_depth_only`, else 900 (enc_mode_config.c:7232).
    depth_early_exit_th: u128,
    cap_max_block: bool,
    nsq_enabled: bool,
    aligned_w: usize,
    aligned_h: usize,
    // Tile pixel origin (0 = single tile → frame-edge predicate, byte-inert).
    // The DC leaf-cost prediction that drives the M6 PD0 partition must not
    // read across a tile boundary, matching C's tile-scoped up/left_available.
    tile_top: usize,
    tile_left: usize,
    // Superres chunk B.4: C's `pcs->variance` is computed by picture analysis
    // on the FULL-RESOLUTION picture and `scale_pcs_params` (resize.c:1434)
    // re-inits the b64/SB geometry for the coded size WITHOUT recomputing it —
    // so under superres the PD0 gates read full-res variances through
    // coded-grid indices. `Some(v)` hands this SB that stale entry; `None`
    // (every non-superres path) recomputes from the source exactly as before.
    stale_vars: Option<&SbVariance>,
    // C `static_config.max_tx_size` (32 or 64). At 32 the partition search may
    // not use 64x64 squares: `max_sq_size = MIN(max_sq_size, 32)`
    // (enc_dec_process.c:1494-1495), and the depth-refinement applies the same
    // cap (:1815). 64 = no cap = the pre-tune-IQ behaviour.
    max_tx_size: u8,
    // C `ctx->pd0_use_src_samples == false` (enc_mode_config.c:7309): the
    // VIDEO arm's PD0 predicts each block from the RECON it generates rather
    // than from the source. `Some((md_recon_plane, stride))` is the frame's
    // MD recon at this SB's origin — what C's neighbour arrays hold on entry,
    // since `copy_neighbour_arrays_pd0` snapshots the live arrays rather than
    // clearing them. `None` = the ALLINTRA arm, byte-identical to before.
    video_recon: Option<(&[u8], usize)>,
    // C `product_prediction_fun_table_pd0[1]` — PD0's INTER arm, `Some` on a
    // NON-KEY frame only. It carries this superblock's `min_sq` and the
    // frame's update types as well as the reference, because all three are
    // things only a non-key frame has; see [`pd0_frame_lambda_and_min_sq`].
    //
    // `None` on every key frame and every allintra cell, which is what makes
    // this parameter byte-neutral for the still envelope BY CONSTRUCTION
    // rather than by measurement — the caller's value is `inter_md.map(..)`,
    // and `inter_md` IS "this frame is a non-I slice with a DPB reference".
    inter: Option<&Pd0InterRef<'_>>,
) -> Pd0Eval {
    let vars = match stale_vars {
        Some(v) => *v,
        None => compute_b64_variance(src, stride, sb_x, sb_y),
    };
    let (lambda, min_sq) = pd0_frame_lambda_and_min_sq(qindex, lambda_weight, min_sq, inter);
    // C `get_max_block_size_allintra` (enc_mode_config.c:7042): the
    // 64-variance cap fires ONLY at enc_mode >= M8 (base_var_th_cap is
    // (uint16_t)~0 = unlimited through M7, 7500 at M8+). A busy SB
    // (var64 > qp-scaled 7500) never tests the 64x64 PART_N — forced
    // split. Missing this made p8 keep 64x64 NONE where C split
    // (6763758 p8: port 64-NONE 120718451 beat C's 4x16x16 split total
    // 124435885 that C never compared against a 64-NONE at all).
    // Callers pass cap_max_block = (preset >= 8) && complete-SB (C keeps
    // the cap at sb_size for incomplete edge SBs).
    let max_sq = if cap_max_block {
        max_block_size_allintra(vars.0[0], qp)
    } else {
        64
    }
    .min(max_tx_size as usize);
    let mut ctx = Pd0Ctx {
        src,
        stride,
        sb_x,
        sb_y,
        aligned_w,
        aligned_h,
        vars,
        qp,
        qindex,
        // Non-bd10 PD0 paths never carry a live QM level (mainline QM-off; the
        // bd8 fork LVL_5/LVL_6 path is left byte-inert per the fork-bd10 scope).
        qm_level: 15,
        lambda,
        mode,
        lvl1: Some(tables),
        max_sq,
        min_sq,
        // C forces PD0 `subres_level = 0` on an INCOMPLETE b64
        // (`!b64_geom->is_complete_b64`, enc_mode_config.c:7337), so seed the
        // "determined, not safe" sentinel (0) there and let a complete SB run
        // the 64x64 odd/even-deviation check. Byte-inert on every level whose
        // `subres_step_cfg()` is 0, which is every ALLINTRA level this entry
        // point serves.
        is_subres_safe: if sb_x + 64 <= aligned_w && sb_y + 64 <= aligned_h {
            255
        } else {
            0
        },
        ires_factor: 0,
        coeff_rate_est_lvl,
        // Every preset this entry point serves is <= M8 on both arms, where
        // `use_accurate_part_ctx` is true (enc_mode_config.c:8955 / :9937).
        accurate_part_ctx: true,
        depth_early_exit_th,
        nsq_enabled,
        tile_top,
        tile_left,
        recon_canvas: video_recon.map(|(r, st)| Pd0ReconCanvas::new(r, st, sb_y)),
        inter,
        pending_recon: None,
    };
    let (_cost, eval) = ctx.pick(64, 0, 0);
    eval
}

/// The block-cost model a RESOLVED C `Pd0Level` selects, on the VIDEO arm.
///
/// **The argument is a `Pd0Level` (0..=6), NOT C's `pcs->pic_pd0_lvl`
/// (0..=8).** Those two numberings are not the same map — `set_pd0_ctrls`
/// (`enc_mode_config.c:5413`) sends `lpd0_lvl` 5 AND 6 to `PD0_LVL_5`, and 7
/// AND 8 to `PD0_LVL_6` — and the caller has already run BOTH `set_pd0_ctrls`
/// and `pd0_detector` (which C does at `enc_dec_process.c:2957`, before
/// `svt_aom_sig_deriv_enc_dec_pd0`). See
/// [`crate::part_arm::video_pd0_params`], which is the single place that
/// resolution happens.
///
/// CORRECTED 2026-09-02. This function used to take the raw `pic_pd0_lvl`,
/// map `5 | 6` to `Lvl5`, and carry a doc comment asserting that "the video
/// ladder never assigns a level whose `pd0_level` is `PD0_LVL_6` at the
/// presets this port encodes". The ladder DOES assign it — a KEY frame at
/// 480p and up takes `set_pic_pd0_lvl_default`'s `lpd0_lvl` 7 — and what
/// keeps C's `assert(IMPLIES(I_SLICE, pd0_level < PD0_LVL_6))` (`:2517`)
/// true is `pd0_detector`'s I_SLICE demote, not the ladder. With the demote
/// missing the port panicked on 4 of 64 video completion cells; the invariant
/// the comment asserted was exactly the code that was absent.
///
/// # Panics
/// On `PD0_LVL_0..PD0_LVL_2`, whose block cost this port carries only in the
/// bd10 [`pd0_pick_sb_partition_lvl0`] entry point, and on `PD0_LVL_6`
/// (VERY_LIGHT_PD0), which is unported. Neither is reachable from
/// [`crate::part_arm::video_pd0_params`] today: `PD0_LVL_6` survives the
/// detector only on a non-I slice whose references were not all intra, and
/// the port's low-delay-P envelope has exactly one reference and it is the
/// key frame.
#[must_use]
fn video_pd0_mode(pd0_level: u8) -> Pd0Mode {
    match pd0_level {
        3 => Pd0Mode::Lvl3,
        4 => Pd0Mode::Lvl4,
        5 => Pd0Mode::Lvl5,
        other => panic!(
            "PD0_LVL_{other} has no block cost in this port's video arm \
             (0..=2 live only in the bd10 entry point; 6 is VERY_LIGHT_PD0, \
             which is unported and which pd0_detector demotes on an I-slice)"
        ),
    }
}

/// Decide the partition tree of one 64x64 superblock on the VIDEO arm.
///
/// The allintra twin is [`pd0_pick_sb_partition`] (preset >= 9, whose level
/// comes from `pd0_detector_allintra` + `get_max_block_size_allintra`) and
/// [`pd0_pick_sb_partition_m6_eval`] (below it). Three things differ, all of
/// them arm facts rather than preset facts:
///
/// * **the LEVEL** comes from `set_pic_pd0_lvl_default` rather than the
///   allintra detector — at 240p and `seq_qp_mod = 2` that is a flat 3 for
///   M3..M7, `3 + ldp0_lvl_offset[qp_band]` at M8 and
///   `4 + ldp0_lvl_offset[qp_band]` from M9 up, so a video key frame runs
///   PD0_LVL_3 / _4 / _5 where the still path runs LVL_1 / LVL_5 / LVL_6.
///   (CORRECTED 2026-09-01: this said `4 + offset` "for M8 up", i.e. 5 at
///   M8/qp40. C's own `SVT_PD0CFG_OUT` dump on `gradient 72x88 q40 p8` reports
///   `lvl=4`, and `:8631` is `MIN(MAX_PD0_LVL, 3 + qp_offset)` for the whole
///   `enc_mode <= ENC_M8` arm. The IMPLEMENTATION was right — it is
///   tier-1 gated — only this comment was wrong.);
/// * **`ctx->max_block_size` is uncapped** — `get_max_block_size_default`
///   returns `scs->super_block_size` outright, with no 64x64-variance cap;
/// * **NSQ geometry is ON** at every preset (`nsq_geom_level` 2 or 3 against
///   the allintra arm's 0 above M6), so a one-false boundary node keeps its
///   single injected edge shape instead of force-splitting.
///
/// KNOWN REMAINING DELTA, stated rather than hidden: C's video PD0 predicts
/// each block from the RECON it generates per block, because
/// `ctx->pd0_use_src_samples` is `allintra || hbd_md` (enc_mode_config.c:7309)
/// and the recon-neighbour arrays are filled from the source ONLY on the
/// allintra arm (product_coding_loop.c:8370). This function still predicts
/// from source. See `docs/INTER-ENCODE-PLAN.md` §1f.
#[allow(clippy::too_many_arguments)]
pub fn pd0_pick_sb_partition_video(
    src: &[u8],
    stride: usize,
    sb_x: usize,
    sb_y: usize,
    qp: u32,
    qindex: u8,
    lambda_weight: u32,
    tables: &M6Pd0Tables,
    // The RESOLVED C `Pd0Level` (0..=6) — `set_pic_pd0_lvl_default` ->
    // `set_pd0_ctrls` -> `pd0_detector`, all three of which
    // `crate::part_arm::video_pd0_params` runs. NOT `pcs->pic_pd0_lvl`,
    // whose 0..=8 numbering is a different map; see [`video_pd0_mode`].
    pd0_level: u8,
    // C `MAX(2, pcs->rate_est_level)` / `MAX(4, ..)` at PD0
    // (`svt_aom_sig_deriv_enc_dec_pd0`, enc_mode_config.c:7355) mapped
    // through `set_rate_est_ctrls` to `coeff_rate_est_lvl`. Read only by the
    // LVL_1 family; LVL_5's closed form ignores it.
    coeff_rate_est_lvl: u8,
    // C `pcs->ppcs->use_accurate_part_ctx` (`enc_mode <= M8`).
    accurate_part_ctx: bool,
    // C `ctx->nsq_geom_ctrls.enabled`.
    nsq_enabled: bool,
    // C `pd0_level <= PD0_LVL_1 || ctx->pic_pred_depth_only`.
    depth_early_exit_lvl1: bool,
    // C `input_resolution_factor[..]` — the LVL_5 closed form's per-picture
    // coeff-rate addend.
    ires_factor: u64,
    aligned_w: usize,
    aligned_h: usize,
    tile_top: usize,
    tile_left: usize,
    stale_vars: Option<&SbVariance>,
    max_tx_size: u8,
    // C `ctx->pd0_use_src_samples == false` (enc_mode_config.c:7309) — the
    // same parameter `pd0_pick_sb_partition_m6_eval` takes, and the same
    // value from the same call site. `Some((md_recon_plane, stride))` is the
    // frame's MD recon; `None` keeps the source prediction.
    video_recon: Option<(&[u8], usize)>,
    // C `product_prediction_fun_table_pd0[1]` — `Some` on a NON-KEY frame.
    inter: Option<&Pd0InterRef<'_>>,
) -> Pd0Tree {
    pd0_pick_sb_partition_video_eval(
        src,
        stride,
        sb_x,
        sb_y,
        qp,
        qindex,
        lambda_weight,
        tables,
        pd0_level,
        coeff_rate_est_lvl,
        accurate_part_ctx,
        nsq_enabled,
        depth_early_exit_lvl1,
        ires_factor,
        aligned_w,
        aligned_h,
        tile_top,
        tile_left,
        stale_vars,
        max_tx_size,
        video_recon,
        false,
        inter,
    )
    .tree()
}

/// [`pd0_pick_sb_partition_video`]'s tree, as the EVAL it is derived from, so
/// `SVTAV1_PD0DBG` can walk the per-node PD0 costs on the pred-depth-only path
/// the way it already can on the `pd0_pick_sb_partition_m6_eval` one. Same
/// computation — `pd0_pick_sb_partition_video` is `…_eval(..).tree()`.
#[allow(clippy::too_many_arguments)]
pub fn pd0_pick_sb_partition_video_eval(
    src: &[u8],
    stride: usize,
    sb_x: usize,
    sb_y: usize,
    qp: u32,
    qindex: u8,
    lambda_weight: u32,
    tables: &M6Pd0Tables,
    // The RESOLVED C `Pd0Level` (0..=6) — `set_pic_pd0_lvl_default` ->
    // `set_pd0_ctrls` -> `pd0_detector`, all three of which
    // `crate::part_arm::video_pd0_params` runs. NOT `pcs->pic_pd0_lvl`,
    // whose 0..=8 numbering is a different map; see [`video_pd0_mode`].
    pd0_level: u8,
    // C `MAX(2, pcs->rate_est_level)` / `MAX(4, ..)` at PD0
    // (`svt_aom_sig_deriv_enc_dec_pd0`, enc_mode_config.c:7355) mapped
    // through `set_rate_est_ctrls` to `coeff_rate_est_lvl`. Read only by the
    // LVL_1 family; LVL_5's closed form ignores it.
    coeff_rate_est_lvl: u8,
    // C `pcs->ppcs->use_accurate_part_ctx` (`enc_mode <= M8`).
    accurate_part_ctx: bool,
    // C `ctx->nsq_geom_ctrls.enabled`.
    nsq_enabled: bool,
    // C `pd0_level <= PD0_LVL_1 || ctx->pic_pred_depth_only`.
    depth_early_exit_lvl1: bool,
    // C `input_resolution_factor[..]` — the LVL_5 closed form's per-picture
    // coeff-rate addend.
    ires_factor: u64,
    aligned_w: usize,
    aligned_h: usize,
    tile_top: usize,
    tile_left: usize,
    stale_vars: Option<&SbVariance>,
    max_tx_size: u8,
    // C `ctx->pd0_use_src_samples == false` (enc_mode_config.c:7309) — the
    // same parameter `pd0_pick_sb_partition_m6_eval` takes, and the same
    // value from the same call site. `Some((md_recon_plane, stride))` is the
    // frame's MD recon; `None` keeps the source prediction.
    video_recon: Option<(&[u8], usize)>,
    // `SVTAV1_PD0_NOSPLIT`, a CONTROL — see `crate::dbgenv::pd0_nosplit`.
    // Never true in a shipped configuration.
    ctl_nosplit: bool,
    // C `product_prediction_fun_table_pd0[1]` — `Some` on a NON-KEY frame
    // only. See [`Pd0InterRef`] for why an inter frame's PD0 has exactly one
    // candidate and it is never intra.
    inter: Option<&Pd0InterRef<'_>>,
) -> Pd0Eval {
    let vars = match stale_vars {
        Some(v) => *v,
        None => compute_b64_variance(src, stride, sb_x, sb_y),
    };
    let mode = video_pd0_mode(pd0_level);
    // C `full_sb_lambda_md[EB_8_BIT_MD]` = `av1_lambda_assign_md`'s
    // `full_lambda_md[0]` (md_process.c:763), which is the FRAME's MD lambda —
    // the KF chain only on a key frame. MEASURED on `diag 64x64 q40 p8`:
    // C's `svt_aom_full_cost_pd0` lambda is 18500 on frame 0 (`base_q_idx`
    // 67, the KF chain) and 241378 on frame 1 (`base_q_idx` 160, base 3.2 x
    // factor 150) — where the KF chain at qindex 160 would give 248207.
    // Shared with `pd0_pick_sb_partition_m6_eval`, which is the entry point
    // the REFINEMENT path takes — a second copy of this resolution is exactly
    // the duplicate-transcription trap docs/WORKING-ON-THIS.md §4 records.
    // `8` is C's `disallow_4x4 ? 8 : 4` arm, i.e. `min_sq` with depth removal
    // off, which is the only value a key frame can have.
    let (lambda, inter_min_sq) = pd0_frame_lambda_and_min_sq(qindex, lambda_weight, 8, inter);
    let mut ctx = Pd0Ctx {
        src,
        stride,
        sb_x,
        sb_y,
        aligned_w,
        aligned_h,
        vars,
        qp,
        qindex,
        qm_level: 15,
        lambda,
        mode,
        lvl1: Some(tables),
        // `get_max_block_size_default` = `scs->super_block_size`, uncapped.
        max_sq: 64.min(max_tx_size as usize),
        // `pic_disallow_4x4` is 1 on both arms at every preset this reaches,
        // which is C's `disallow_4x4 ? 8 : 4` arm. On a NON-KEY frame
        // `depth_removal_ctrls` can raise it to 16, 32 or 64 per superblock —
        // see `Pd0InterRef::min_sq`.
        min_sq: if ctl_nosplit { 64 } else { inter_min_sq },

        is_subres_safe: if sb_x + 64 <= aligned_w && sb_y + 64 <= aligned_h {
            255
        } else {
            0
        },
        ires_factor,
        coeff_rate_est_lvl,
        accurate_part_ctx,
        depth_early_exit_th: if depth_early_exit_lvl1 { 1000 } else { 900 },
        nsq_enabled,
        tile_top,
        tile_left,
        recon_canvas: video_recon.map(|(r, st)| Pd0ReconCanvas::new(r, st, sb_y)),
        inter,
        pending_recon: None,
    };
    ctx.pick(64, 0, 0).1
}

#[cfg(test)]
mod inter_lambda_tests {
    use super::*;
    use crate::port_rc_process::FrameUpdateType as U;

    /// C's OWN `full_sb_lambda_md[EB_8_BIT_MD]`, read off the `lambda`
    /// argument of `svt_aom_full_cost_pd0` through the `SVT_PD0COST_OUT`
    /// interposer on `diag 64x64 q40 p8 frames=2` (evidence tier 2 — the real
    /// encoder, not a transcription):
    ///
    /// ```text
    /// PD0COST org=(0,0) 64x64 dist=64712 ybits=1519 cost=9142574 lambda=241378
    /// ```
    ///
    /// with `base_q_idx = 160` on that frame (`tools/fh_fields.py --index 1`).
    /// The KEY frame of the same cell is `base_q_idx = 67`, `lambda=18500`,
    /// which the KF builder already reproduced.
    #[test]
    fn the_low_delay_p_inter_lambda_matches_cs_measured_value() {
        // The two selectors C actually uses on that frame.
        assert_eq!(
            inter_full_lambda_8bit(160, U::LfUpdate, U::ArfUpdate, false, 0, 150),
            241_378
        );
        // The KEY frame, for the same cell, from the same dump.
        assert_eq!(kf_full_lambda_8bit_lw(67, 150), 18_500);
    }

    /// **The port already HAD a correct transcription, and this function is a
    /// SECOND one that diverged from it.**
    /// `port_rc_process::compute_rd_mult` takes `LambdaContext::update_type`
    /// for the base and lets `update_lambda` derive its own `gf_update_type`
    /// for the factor — exactly C's split — and has done since it was ported.
    /// `inter_full_lambda_8bit` re-transcribed the same C chain and collapsed
    /// the two, which is the failure mode a duplicate transcription always
    /// has. This test PINS THEM TOGETHER over a sweep so they cannot diverge
    /// again: the only thing this function adds is the `lambda_weight`
    /// multiply `av1_lambda_assign_md` (md_process.c:747) applies afterwards.
    #[test]
    fn it_agrees_with_port_rc_process_compute_rd_mult_over_a_sweep() {
        use crate::port_rc_process::{LambdaContext, compute_rd_mult};
        for &qindex in &[0u8, 1, 20, 67, 100, 160, 200, 255] {
            for &(base, factor_tl) in &[
                (U::LfUpdate, 0u8),
                (U::ArfUpdate, 0),
                (U::GfUpdate, 0),
                (U::IntnlArfUpdate, 0),
                (U::LfUpdate, 3),
            ] {
                for &lw in &[0u32, 128, 150, 175] {
                    // Every qdiff that selects a DIFFERENT factor, plus both
                    // boundaries of each threshold. Sweeping this axis is what
                    // lets the test SEE the wrong-arm transcription corrected
                    // on 2026-09-02: the `delta_q_present` arm and the one an
                    // inter frame takes agree at qdiff 0 and at no other point
                    // in this list, so a revert fails here.
                    for &qd in &[0i32, -3, -4, -5, -9, 3, 4, 5, 9] {
                        let ctx = LambdaContext {
                            frame_type: 1, // not KEY_FRAME
                            temporal_layer_index: factor_tl,
                            hierarchical_levels: 3,
                            update_type: base,
                            alt_lambda_factors: false,
                            rtc: false,
                            stats_based_sb_lambda_modulation: true,
                            base_q_idx: i32::from(qindex),
                            delta_q_present: false,
                            r0_delta_qp_md: false,
                            lambda_scale_factors: [128; 7],
                        };
                        let me_q = (i32::from(qindex) + qd).clamp(0, 255) as u8;
                        let qd = i32::from(me_q) - i32::from(qindex);
                        let rc = compute_rd_mult(&ctx, qindex, me_q, 8);
                        let want = if lw == 0 {
                            rc
                        } else {
                            ((u64::from(rc) * u64::from(lw)) >> 7) as u32
                        };
                        let factor = crate::port_rc_process::lambda_gf_update_type(
                            false,
                            ctx.hierarchical_levels,
                            factor_tl,
                        );
                        assert_eq!(
                            inter_full_lambda_8bit(qindex, base, factor, false, qd, lw),
                            want,
                            "qindex {qindex} base {base:?} tl {factor_tl} lw {lw} qdiff {qd}"
                        );
                    }
                }
            }
        }
    }

    /// The NEGATIVE control: conflating the two update types — which is what
    /// the port did until 2026-09-02 — gives a DIFFERENT number, so the test
    /// above cannot pass with the split reverted.
    #[test]
    fn one_update_type_for_both_halves_does_not_reproduce_c() {
        // ARF for both: the value `docs/INTER-ENCODE-PLAN.md` §1y recorded.
        assert_eq!(
            inter_full_lambda_8bit(160, U::ArfUpdate, U::ArfUpdate, false, 0, 150),
            244_792
        );
        // LF for both: factor 180 instead of 150.
        assert_eq!(
            inter_full_lambda_8bit(160, U::LfUpdate, U::LfUpdate, false, 0, 150),
            289_654
        );
        // And the KF chain at the same qindex, which is what a caller that
        // forgot the frame type entirely would produce.
        assert_eq!(kf_full_lambda_8bit_lw(160, 150), 248_207);
    }
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
    fn lvl0_block_costs_match_c() {
        // C `svt_aom_full_cost_pd0` per-block RD, gradient-64 q20 (qindex 80),
        // captured from the REAL library's SVT_PD0COST_OUT wrap at bd10 (PD0
        // runs at 8-bit, hbd_md forced 0). Closed-form coeff rate (5000 +
        // 100*eob, coeff_rate_est_lvl 0), qindex+8 quant (lpd0_qp_offset 8),
        // subres OFF, 8-bit lambda 25650.
        let y = gradient64();
        let mut ctx = Pd0Ctx {
            src: &y,
            stride: 64,
            sb_x: 0,
            sb_y: 0,
            aligned_w: 64,
            aligned_h: 64,
            vars: compute_b64_variance(&y, 64, 0, 0),
            qp: 20,
            qindex: 80,
            qm_level: 15,
            lambda: kf_full_lambda_8bit(80, 20) as u64,
            mode: Pd0Mode::Lvl0,
            coeff_rate_est_lvl: 0,
            lvl1: None,
            max_sq: 32,
            min_sq: 8,
            is_subres_safe: 0, // subres off
            ires_factor: 0,
            accurate_part_ctx: true,
            depth_early_exit_th: 1000,
            nsq_enabled: false,
            tile_top: 0,
            tile_left: 0,
            recon_canvas: None,
            inter: None,
            pending_recon: None,
        };
        assert_eq!(ctx.lambda, 25650);
        // (sq, org_x, org_y, C full_cost)
        for (sq, ox, oy, cost) in [
            (32usize, 0usize, 0usize, 26185862u64),
            (16, 0, 0, 8396609),
            (8, 0, 0, 2143413),
            (8, 8, 0, 1990844),
            (8, 0, 8, 2225589),
            (8, 8, 8, 2168757),
            (16, 16, 0, 6559425),
            (16, 0, 16, 8443329),
            (16, 16, 16, 8792001),
            (32, 32, 0, 28871046),
            (32, 0, 32, 22111622),
            (32, 32, 32, 22521222),
        ] {
            assert_eq!(ctx.lvl0_block_cost(sq, ox, oy), cost, "sq={sq} ({ox},{oy})");
        }
    }

    #[test]
    fn lvl0_gradient64_tree_matches_c() {
        // C bd10 CTREE (svt_aom_update_mi_map wrap): gradient-64 q20 p10 codes
        // 4x BLOCK_32X32 PARTITION_NONE (the LVL_0 full-RD keeps the 32x32
        // parent where the LVL_6 heuristic over-splits to 16x 16x16). The
        // 64x64 force-splits (var64 5425 > qp-scaled cap -> max_sq 32).
        let y = gradient64();
        let tree = pd0_pick_sb_partition_lvl0(
            &y,
            64,
            0,
            0,
            20,
            80,
            frame_lambda_weight(20, false, 0),
            15,
            0,
            64,
            64,
            None,
            64,
        );
        assert_eq!(tree.leaf_sizes(), vec![32, 32, 32, 32]);
        // q40 / q55 keep the same 4x32 shape here (the parent still wins);
        // q55's 64x64 is IN the depth set (max_sq 64) and PARENT wins outright
        // -> a single 64x64 leaf.
        let t55 = pd0_pick_sb_partition_lvl0(
            &y,
            64,
            0,
            0,
            55,
            220,
            frame_lambda_weight(55, false, 0),
            15,
            0,
            64,
            64,
            None,
            64,
        );
        assert_eq!(t55.leaf_sizes(), vec![64]);
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
            assert_eq!(
                lvl6_cost_allintra(&v, sq, ox, oy, 20),
                cost,
                "sq={sq} ({ox},{oy})"
            );
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
            aligned_w: 64,
            aligned_h: 64,
            vars: compute_b64_variance(&y, 64, 0, 0),
            qp: 40,
            qindex: 160,
            qm_level: 15,
            lambda: kf_full_lambda_8bit(160, 40) as u64,
            mode: Pd0Mode::Lvl5,
            coeff_rate_est_lvl: 0,
            lvl1: None,
            max_sq: 32,
            min_sq: 8,
            is_subres_safe: 255,
            ires_factor: 0,
            accurate_part_ctx: true,
            depth_early_exit_th: 1000,
            nsq_enabled: false,
            tile_top: 0,
            tile_left: 0,
            recon_canvas: None,
            inter: None,
            pending_recon: None,
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
            aligned_w: 64,
            aligned_h: 64,
            vars: compute_b64_variance(&y, 64, 0, 0),
            qp: 55,
            qindex: 220,
            qm_level: 15,
            lambda: kf_full_lambda_8bit(220, 55) as u64,
            mode: Pd0Mode::Lvl5,
            coeff_rate_est_lvl: 0,
            lvl1: None,
            max_sq: 64,
            min_sq: 8,
            is_subres_safe: 255,
            ires_factor: 0,
            accurate_part_ctx: true,
            depth_early_exit_th: 1000,
            nsq_enabled: false,
            tile_top: 0,
            tile_left: 0,
            recon_canvas: None,
            inter: None,
            pending_recon: None,
        };
        assert_eq!(ctx.lvl5_block_cost(64, 0, 0), 1708208432);
        assert_eq!(
            ctx.is_subres_safe, 1,
            "64x64 DC pred must pass the odd/even check"
        );
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
    fn dc_only_safe_matches_c() {
        // Instrumented-C capture (SVT_MDBG2 cand_gen prints, gradient-64,
        // 2026-07-13): q40 32x32 leaves and q20 16x16 leaves all print
        // dc_only=1 safe=1 (candidate set = {DC}); the q55/q40 64x64
        // prints safe=0 (var 5425 >= 2000).
        let y = gradient64();
        let v = compute_b64_variance(&y, 64, 0, 0);
        assert!(!is_dc_only_safe(&v, 64, 0, 0), "64x64: var 5425 >= 2000");
        for (ox, oy) in [(0usize, 0usize), (32, 0), (0, 32), (32, 32)] {
            assert!(is_dc_only_safe(&v, 32, ox, oy), "32x32 ({ox},{oy})");
        }
        for by in 0..4 {
            for bx in 0..4 {
                assert!(
                    is_dc_only_safe(&v, 16, bx * 16, by * 16),
                    "16x16 ({bx},{by})"
                );
            }
        }
        // 8x8: blk_var < 2000 only (all gradient 8x8 vars are 79..1957).
        for by in 0..8 {
            for bx in 0..8 {
                assert!(is_dc_only_safe(&v, 8, bx * 8, by * 8), "8x8 ({bx},{by})");
            }
        }
        // 4x4 has no variance data: C early-exits with 0.
        assert!(!is_dc_only_safe(&v, 4, 0, 0));
        // Uniform content: zero variance everywhere -> always DC-only.
        let u = vec![128u8; 64 * 64];
        let vu = compute_b64_variance(&u, 64, 0, 0);
        for sq in [64usize, 32, 16, 8] {
            assert!(is_dc_only_safe(&vu, sq, 0, 0), "uniform sq={sq}");
        }
    }

    #[test]
    fn gradient64_trees_match_c() {
        let y = gradient64();
        // q20 (qindex 80): LVL_6, max 32 -> forced SPLIT at 64, every 32
        // SPLITs again, 16x16 leaves everywhere (C stream: op0 SPLIT,
        // op1 SPLIT, op2 NONE...).
        let t20 = pd0_pick_sb_partition(
            &y,
            64,
            0,
            0,
            20,
            80,
            frame_lambda_weight(20, false, 0),
            0,
            64,
            64,
            None,
            64,
        );
        assert_eq!(t20.leaf_sizes(), vec![16; 16]);
        // q40 (qindex 160): LVL_5, max 32 -> forced SPLIT at 64, all four
        // 32x32 keep PARENT (C: op0 SPLIT, op1 NONE).
        let t40 = pd0_pick_sb_partition(
            &y,
            64,
            0,
            0,
            40,
            160,
            frame_lambda_weight(40, false, 0),
            0,
            64,
            64,
            None,
            64,
        );
        assert_eq!(t40.leaf_sizes(), vec![32; 4]);
        // q55 (qindex 220): LVL_5, 64 in set and PARENT wins outright.
        let t55 = pd0_pick_sb_partition(
            &y,
            64,
            0,
            0,
            55,
            220,
            frame_lambda_weight(55, false, 0),
            0,
            64,
            64,
            None,
            64,
        );
        assert_eq!(t55, Pd0Tree::Leaf(64));
        // Uniform: LVL_5 with zero residual everywhere -> 64x64 NONE.
        let u = vec![128u8; 64 * 64];
        let tu = pd0_pick_sb_partition(
            &u,
            64,
            0,
            0,
            40,
            160,
            frame_lambda_weight(40, false, 0),
            0,
            64,
            64,
            None,
            64,
        );
        assert_eq!(tu, Pd0Tree::Leaf(64));
    }

    /// PD0_LVL_1 per-block costs pinned from the instrumented M6 run
    /// (SVT_M6DBG PD0BLK lines, gradient-64, docs/IDENTITY-STATUS.md M6
    /// chunk). Single-SB frame -> default tables, exactly like C's SB 0.
    #[test]
    fn lvl1_block_costs_match_c() {
        let y = gradient64();
        let tables = build_m6_pd0_tables(220);
        let mut ctx = Pd0Ctx {
            src: &y,
            stride: 64,
            sb_x: 0,
            sb_y: 0,
            aligned_w: 64,
            aligned_h: 64,
            vars: compute_b64_variance(&y, 64, 0, 0),
            qp: 55,
            qindex: 220,
            qm_level: 15,
            lambda: kf_full_lambda_8bit(220, 55) as u64,
            mode: Pd0Mode::Lvl1,
            coeff_rate_est_lvl: 1,
            lvl1: Some(&tables),
            max_sq: 64,
            min_sq: 8,
            is_subres_safe: 255,
            ires_factor: 0,
            accurate_part_ctx: true,
            depth_early_exit_th: 1000,
            nsq_enabled: true,
            tile_top: 0,
            tile_left: 0,
            recon_canvas: None,
            inter: None,
            pending_recon: None,
        };
        for (sq, ox, oy, cost) in [
            (64usize, 0usize, 0usize, 1791569177u64),
            (32, 0, 0, 526486441),
            (32, 32, 0, 572301943),
            (16, 0, 0, 146206469),
            (8, 0, 0, 44014180),
            (8, 8, 0, 35188942),
            (8, 0, 8, 37535984),
            (8, 8, 8, 60514499),
        ] {
            assert_eq!(
                ctx.lvl1_block_cost(sq, ox, oy),
                cost,
                "q55 sq={sq} ({ox},{oy})"
            );
        }

        let tables40 = build_m6_pd0_tables(160);
        let mut ctx40 = Pd0Ctx {
            src: &y,
            stride: 64,
            sb_x: 0,
            sb_y: 0,
            aligned_w: 64,
            aligned_h: 64,
            vars: compute_b64_variance(&y, 64, 0, 0),
            qp: 40,
            qindex: 160,
            qm_level: 15,
            lambda: kf_full_lambda_8bit(160, 40) as u64,
            mode: Pd0Mode::Lvl1,
            coeff_rate_est_lvl: 1,
            lvl1: Some(&tables40),
            max_sq: 64,
            min_sq: 8,
            is_subres_safe: 255,
            ires_factor: 0,
            accurate_part_ctx: true,
            depth_early_exit_th: 1000,
            nsq_enabled: true,
            tile_top: 0,
            tile_left: 0,
            recon_canvas: None,
            inter: None,
            pending_recon: None,
        };
        for (sq, ox, oy, cost) in [
            (64usize, 0usize, 0usize, 1176293547u64),
            (32, 0, 0, 230378290),
            (16, 0, 0, 62496975),
            (8, 0, 0, 16077204),
        ] {
            assert_eq!(
                ctx40.lvl1_block_cost(sq, ox, oy),
                cost,
                "q40 sq={sq} ({ox},{oy})"
            );
        }

        let tables20 = build_m6_pd0_tables(80);
        let mut ctx20 = Pd0Ctx {
            src: &y,
            stride: 64,
            sb_x: 0,
            sb_y: 0,
            aligned_w: 64,
            aligned_h: 64,
            vars: compute_b64_variance(&y, 64, 0, 0),
            qp: 20,
            qindex: 80,
            qm_level: 15,
            lambda: kf_full_lambda_8bit(80, 20) as u64,
            mode: Pd0Mode::Lvl1,
            coeff_rate_est_lvl: 1,
            lvl1: Some(&tables20),
            max_sq: 64,
            min_sq: 8,
            is_subres_safe: 255,
            ires_factor: 0,
            accurate_part_ctx: true,
            depth_early_exit_th: 1000,
            nsq_enabled: true,
            tile_top: 0,
            tile_left: 0,
            recon_canvas: None,
            inter: None,
            pending_recon: None,
        };
        for (sq, ox, oy, cost) in [
            (64usize, 0usize, 0usize, 903280295u64),
            (32, 0, 0, 51245980),
            (16, 0, 0, 14528276),
            (8, 0, 0, 3483565),
            (8, 8, 0, 3484388),
        ] {
            assert_eq!(
                ctx20.lvl1_block_cost(sq, ox, oy),
                cost,
                "q20 sq={sq} ({ox},{oy})"
            );
        }
    }

    /// M6 PD0 trees for the gradient-64 identity cells (instrumented
    /// PD0CMP verdicts): q20/q40 -> 64 SPLIT + four 32x32 PARENT (q20 is
    /// SHALLOWER than the eff-M9 16x16 tree), q55 -> single 64x64.
    #[test]
    fn m6_gradient64_trees_match_c() {
        let y = gradient64();
        let t20 = pd0_pick_sb_partition_m6(
            &y,
            64,
            0,
            0,
            20,
            80,
            frame_lambda_weight(20, false, 0),
            &build_m6_pd0_tables(80),
            1,
            true,
            64,
            64,
            None,
            64,
        );
        assert_eq!(t20.leaf_sizes(), vec![32; 4]);
        let t40 = pd0_pick_sb_partition_m6(
            &y,
            64,
            0,
            0,
            40,
            160,
            frame_lambda_weight(40, false, 0),
            &build_m6_pd0_tables(160),
            1,
            true,
            64,
            64,
            None,
            64,
        );
        assert_eq!(t40.leaf_sizes(), vec![32; 4]);
        let t55 = pd0_pick_sb_partition_m6(
            &y,
            64,
            0,
            0,
            55,
            220,
            frame_lambda_weight(55, false, 0),
            &build_m6_pd0_tables(220),
            1,
            true,
            64,
            64,
            None,
            64,
        );
        assert_eq!(t55, Pd0Tree::Leaf(64));
        // Uniform content: exact DC prediction, zero residual -> 64 NONE
        // (keeps every uniform p6 identity cell byte-identical).
        let u = vec![128u8; 64 * 64];
        let tu = pd0_pick_sb_partition_m6(
            &u,
            64,
            0,
            0,
            40,
            160,
            frame_lambda_weight(40, false, 0),
            &build_m6_pd0_tables(160),
            1,
            true,
            64,
            64,
            None,
            64,
        );
        assert_eq!(tu, Pd0Tree::Leaf(64));
    }
}

#[cfg(test)]
mod alt_lambda_tests {
    #[test]
    fn alt_factor_changes_lambda() {
        let base = super::kf_full_lambda_8bit_ex(160, 40, false, 0);
        let alt = super::kf_full_lambda_8bit_ex(160, 40, true, 0);
        assert!(alt < base, "{alt} vs {base}");
        assert_eq!(base, super::kf_full_lambda_8bit(160, 40));
        let qd = super::kf_full_lambda_8bit_ex(160, 40, false, 9);
        assert!(qd > base);
    }
}

/// Differential parity for the post-MD RD lambdas against the REAL exported
/// `svt_aom_compute_rd_mult_based_on_qindex` (rc_process.c:365) — the base
/// that `svt_aom_lambda_assign` builds every one of them from.
///
/// Both bd10 lambdas added for the bd10 CDEF/LR searches are pinned here
/// across the whole qindex range, not at hand-picked anchors: the bd10 chain
/// (`dc_qlookup_10` -> `(3.3+0.0015q)q²` -> `ROUND_POWER_OF_TWO(_,4)` ->
/// clamp -> `*128>>7`) has four places a transcription can be off by one and
/// only the C symbol settles them.
#[cfg(test)]
mod lambda_c_parity {
    use svtav1_cref as cref;

    /// `SVT_AV1_KF_UPDATE` (definitions.h) — the KEY-frame update type.
    const KF_UPDATE: i32 = 0;

    /// `update_lambda`'s frame-type scale for KF at each bit depth:
    /// `rd_frame_type_factor[bit_depth != EB_EIGHT_BIT][KF_UPDATE]`
    /// (rc_process.c:395-396) = 150 at bd8, 128 at bd10.
    fn c_full_lambda_unweighted(bit_depth: u8, qindex: u8) -> u32 {
        let base = cref::compute_rd_mult_based_on_qindex(bit_depth, KF_UPDATE, qindex) as i64;
        let ftf: i64 = if bit_depth == 8 { 150 } else { 128 };
        ((base * ftf) >> 7) as u32
    }

    /// The CDEF search's lambda: `svt_aom_lambda_assign(.., enhanced_pic->
    /// bit_depth, base_q_idx, multiply_lambda = false)` (enc_cdef.c:958).
    #[test]
    fn cdef_search_lambda_matches_c_at_every_qindex() {
        for q in 0..=255u16 {
            let q = q as u8;
            assert_eq!(
                super::kf_full_lambda_8bit_unweighted(q),
                c_full_lambda_unweighted(8, q),
                "bd8 CDEF lambda at qindex {q}"
            );
            assert_eq!(
                super::kf_full_lambda_bd10_unweighted(q),
                c_full_lambda_unweighted(10, q),
                "bd10 CDEF lambda at qindex {q}"
            );
        }
        // Non-vacuity: the two depths must genuinely differ (a bd10 arm that
        // silently returned the bd8 value would pass a same-value compare).
        let differ = (0..=255u16)
            .filter(|&q| {
                super::kf_full_lambda_bd10_unweighted(q as u8)
                    != super::kf_full_lambda_8bit_unweighted(q as u8)
            })
            .count();
        assert!(
            differ > 200,
            "bd8/bd10 lambdas differ at only {differ} qindexes"
        );
    }

    /// The LR search's `x->rdmult` = `pic_full_lambda[EB_{8,10}_BIT_MD]`
    /// (enc_dec_process.c:3246), i.e. `multiply_lambda = true` — which only
    /// scales the 10-bit arm (`*= 16`, rc_process.c:479).
    #[test]
    fn lr_search_rdmult_matches_c_at_every_qindex() {
        for q in 0..=255u16 {
            let q = q as u8;
            // bd8: multiply_lambda is a no-op, so it equals the unweighted.
            assert_eq!(
                super::kf_full_lambda_8bit_unweighted(q),
                c_full_lambda_unweighted(8, q),
                "bd8 LR rdmult at qindex {q}"
            );
            assert_eq!(
                super::kf_full_lambda_bd10_pic(q),
                c_full_lambda_unweighted(10, q) * 16,
                "bd10 LR rdmult at qindex {q}"
            );
        }
    }
}
