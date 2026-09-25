use super::*;

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

/// Research mode omits the QP ladder for tune 0–2 in both C derivation arms
/// (`enc_mode_config.c:9450,10101`). IQ and the extended-CRF bump remain live.
pub(crate) fn frame_lambda_weight_for_preset(
    preset: i8,
    picture_qp: u32,
    tune_iq: bool,
    extended_crf_bump: u32,
) -> u32 {
    if preset <= -1 && !tune_iq {
        extended_crf_bump
    } else {
        frame_lambda_weight(picture_qp, tune_iq, extended_crf_bump)
    }
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
pub(super) const RD_FRAME_TYPE_FACTOR_8BIT: [i64; 7] = [150, 180, 150, 150, 180, 180, 150];
/// C `rd_frame_type_factor_alt` (rc_process.c:397).
pub(super) const RD_FRAME_TYPE_FACTOR_ALT: [i64; 7] = [140, 180, 128, 140, 164, 164, 140];

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
    // C `av1_lambda_assign_md`'s LAMBDA_MOD_INTRA arm (md_process.c:730-745):
    // `!rtc && stats_based_sb_lambda_modulation && temporal_layer_index > 0 &&
    //  ref_intra_percentage < (alt_lambda_factors ? 65 : LAMBDA_MOD_INTRA_TH)`
    // scales BOTH `full_lambda_md` and `fast_lambda_md` by
    // `LAMBDA_MOD_INTRA_SCALING_FACTOR` (138) BEFORE `lambda_weight`. 128 is
    // the arm-not-taken identity.
    lambda_mod_intra: i64,
    lambda_weight: u32,
) -> u32 {
    use crate::port_rc_process::FrameUpdateType as U;
    let q = svtav1_dsp::quant_tables::DC_QLOOKUP_8[qindex as usize] as f64;
    let base = match base_update_type {
        U::Kf => 3.3,
        U::Gf | U::Arf => 3.25,
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
    // md_process.c:739-742 — inside `av1_lambda_assign_md`, between
    // `svt_aom_compute_rd_mult` and the `lambda_weight` multiply.
    rdmult = (rdmult * lambda_mod_intra) >> 7;
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
pub(crate) fn kf_full_lambda_bd10(qindex: u8, picture_qp: u32, preset: i8) -> u32 {
    let q = crate::bd10::dc_qlookup_10(qindex) as i64;
    let mut rdmult = ((3.3 + 0.0015 * q as f64) * q as f64 * q as f64) as i64;
    rdmult = (rdmult + 8) >> 4; // ROUND_POWER_OF_TWO(_, 4) — bd10
    rdmult = (rdmult * 128) >> 7; // rd_frame_type_factor[1][KF_UPDATE] = 128
    let mut lambda = rdmult as u32;
    let lambda_weight: u32 = frame_lambda_weight_for_preset(preset, picture_qp, false, 0);
    if lambda_weight != 0 {
        lambda = ((lambda as u64 * lambda_weight as u64) >> 7) as u32;
    }
    lambda * 16 // md_process.c:753 — full_lambda_md[1] *= 16 (2^(2*(10-8)))
}

/// INTER full MD lambda at bd10 (C `full_lambda_md[EB_10_BIT_MD]` from
/// `av1_lambda_assign_md`, md_process.c:725-759) — the chain
/// `coding_loop.c:436` feeds `svt_aom_quantize_inv_quantize` on the encode
/// pass, which ALWAYS runs at the real bit depth (`pic_bypass_encdec` is
/// forced off at bd > 8, md_config_process.c:1046).
///
/// Same chain as [`inter_full_lambda_8bit`] with three 10-bit differences:
/// `q` is `dc_qlookup_10`, the rdmult base gets `ROUND_POWER_OF_TWO(_, 4)`
/// (rc_process.c:382), the frame-type factor reads `rd_frame_type_factor[1]`
/// (the {128,144,128,128,144,144,128} row — a no-op for every update type a
/// flat low-delay GOP produces), and the result gets `*= 16`
/// (md_process.c:753). `lambda_mod_intra` and `lambda_weight` are the
/// caller-resolved MD values — they DO apply here, unlike
/// [`crate::port_rc_process::lambda_assign`], which is the `pic_full_lambda`
/// chain (no weight).
pub(crate) fn inter_full_lambda_bd10(
    qindex: u8,
    base_update_type: crate::port_rc_process::FrameUpdateType,
    factor_update_type: crate::port_rc_process::FrameUpdateType,
    alt_lambda_factors: bool,
    qdiff_vs_base: i32,
    lambda_mod_intra: i64,
    lambda_weight: u32,
) -> u32 {
    use crate::port_rc_process::FrameUpdateType as U;
    let q = crate::bd10::dc_qlookup_10(qindex) as f64;
    let base = match base_update_type {
        U::Kf => 3.3,
        U::Gf | U::Arf => 3.25,
        _ => 3.2,
    };
    let mut rdmult = ((base + 0.0015 * q) * q * q) as i64;
    rdmult = (rdmult + 8) >> 4; // ROUND_POWER_OF_TWO(_, 4) — bd10
    let ut = factor_update_type as usize;
    rdmult = (rdmult
        * if alt_lambda_factors {
            RD_FRAME_TYPE_FACTOR_ALT[ut]
        } else {
            // `rd_frame_type_factor[bit_depth != EB_EIGHT_BIT][..]` —
            // rc_process.c:395-396 row 1.
            crate::port_rc_process::RD_FRAME_TYPE_FACTOR[1][ut] as i64
        })
        >> 7;
    let stats_factor: i64 = if qdiff_vs_base < 0 {
        if qdiff_vs_base <= -4 { 100 } else { 115 }
    } else if qdiff_vs_base > 0 {
        if qdiff_vs_base <= 4 { 135 } else { 150 }
    } else {
        128
    };
    rdmult = (rdmult * stats_factor) >> 7;
    // md_process.c:739-742 — LAMBDA_MOD_INTRA before lambda_weight.
    rdmult = (rdmult * lambda_mod_intra) >> 7;
    let mut lambda = rdmult as u32;
    if lambda_weight != 0 {
        lambda = ((u64::from(lambda) * u64::from(lambda_weight)) >> 7) as u32;
    }
    lambda * 16 // md_process.c:753 — full_lambda_md[1] *= 16
}

/// [`kf_full_lambda_bd10`]'s tuned twin (C `update_lambda`, rc_process.c:
/// 401-449 at `EB_TEN_BIT`) — `full_sb_lambda_md[EB_10_BIT_MD]`, the lambda
/// Ghost Robot's 16-bit PD0 prices every block with (`2c66d9ea`'s
/// `full_loop_core_pd0` reads `full_sb_lambda_md[EB_10_BIT_MD]` under
/// `SVT_EFFECTIVE_HBD_MD`, product_coding_loop.c:5967).
///
/// Differences from [`kf_full_lambda_bd10`], all shared with the 8-bit twin
/// [`kf_full_lambda_8bit_tuned`]:
/// * `alt_lambda_factors` (fork flag, OFF at Ghost Robot's own defaults)
///   swaps the KF frame-type factor for `rd_frame_type_factor_alt[KF_UPDATE]`
///   = 140 — at bd10 the non-alt row's factor is already a 128 no-op, so the
///   alt flag is the only way this multiply does anything.
/// * `qdiff_vs_base` (`q_index - base_q_idx`) drives
///   `stats_based_sb_lambda_modulation`'s delta_q_present arm:
///   {<=-8: 90, <0: 115, <=8: 135, >8: 150} — live only where the frame
///   signals per-SB delta-q (variance boost); 128 otherwise.
/// * `lambda_weight_override` is the caller-resolved `pcs->lambda_weight`
///   (tune-IQ curve OR PSNR ladder + extended-CRF bump); `None` re-derives
///   the PSNR ladder from `picture_qp`.
pub(crate) fn kf_full_lambda_bd10_tuned(
    qindex: u8,
    picture_qp: u32,
    alt_lambda_factors: bool,
    qdiff_vs_base: i32,
    lambda_weight_override: Option<u32>,
) -> u32 {
    let q = crate::bd10::dc_qlookup_10(qindex) as i64;
    let mut rdmult = ((3.3 + 0.0015 * q as f64) * q as f64 * q as f64) as i64;
    rdmult = (rdmult + 8) >> 4; // ROUND_POWER_OF_TWO(_, 4) — EB_TEN_BIT
    // `rd_frame_type_factor[1][KF_UPDATE]` = 128 (a no-op); the alt row's
    // KF_UPDATE entry is 140 on both depth rows.
    let ftf: i64 = if alt_lambda_factors { 140 } else { 128 };
    rdmult = (rdmult * ftf) >> 7;
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
        lambda = ((u64::from(lambda) * u64::from(lambda_weight)) >> 7) as u32;
    }
    lambda * 16 // md_process.c:753 — full_lambda_md[1] *= 16
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
