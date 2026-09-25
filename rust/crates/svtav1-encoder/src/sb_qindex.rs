//! Per-SB qindex derivation for the fork's Variance Boost (delta-q L2).
//!
//! Mirrors the C hybrid's fork-side chain for still/KEY frames:
//! 1. Per-SB f64 variances + SB mean from SOURCE luma — the C producer
//!    (`pic_analysis_process.c compute_block_mean_compute_variance`) at the
//!    library default `block_mean_calc_prec = BLOCK_MEAN_PREC_SUB`
//!    (enc_handle.c:4400): each 8x8 mean sums 4 ALTERNATE rows (32 px,
//!    `svt_compute_sub_mean_8x8_c` loop `skip = vi+vi`) and scales
//!    `<< 3` into fp8; each 8x8 mean-of-squares sums the same 32 px of
//!    p^2 and scales `<< 11` into fp16. The 16x16/32x32/64x64 levels are
//!    `>> 2` averages of their children. Variance = fork
//!    `SVT_VAR_STORE(meansq - mean*mean, 16)` = `f64(x) / 65536`.
//!    `ppcs->mean[sb]` = the fp8 64x64 mean.
//! 2. `svt_av1_variance_adjust_qp(pcs, readjust_base_q_idx=true)` — the
//!    KEY-frame call site: per-SB boost via
//!    [`crate::var_boost::deltaq_sb_variance_boost`], min/max tracking,
//!    base recentering to `min + range/2`, offset clamp to ±max_range/2.
//! 3. `get_delta_q_res` (resource_coordination_process.c:319) +
//!    `svt_av1_normalize_sb_delta_q` (rc_aq.c:830) when res != 1.

use crate::var_boost;
use alloc::vec::Vec;

pub const MAXQ: i32 = 255;
const VAR_BOOST_MAX_PQ_DELTAQ_RANGE: i32 = 120;
const VAR_BOOST_MAX_DELTAQ_RANGE: i32 = 80;

/// One SB's variance-boost inputs (fork f64 semantics).
#[derive(Debug, Clone)]
pub struct SbVariance {
    /// The 64 8x8 variances, raster order within the SB.
    pub var_8x8: [f64; 64],
    /// Whole-SB (64x64) variance.
    pub var_64x64: f64,
    /// `ppcs->mean[sb]` — fp8 64x64 mean (0..=255<<8 range).
    pub mean: u64,
}

/// C `svt_compute_sub_mean_8x8_c`: 4 alternate rows x 8 px, `<< 3` (fp8).
#[inline]
fn sub_mean_8x8(px: &dyn Fn(usize, usize) -> u64, x0: usize, y0: usize) -> u64 {
    let mut s: u64 = 0;
    for vi in 0..4 {
        let y = y0 + 2 * vi;
        for hi in 0..8 {
            s += px(x0 + hi, y);
        }
    }
    s << 3
}

/// C `svt_aom_compute_sub_mean_squared_values_c`: same 32 px of p^2, `<< 11` (fp16).
#[inline]
fn sub_mean_sq_8x8(px: &dyn Fn(usize, usize) -> u64, x0: usize, y0: usize) -> u64 {
    let mut s: u64 = 0;
    for vi in 0..4 {
        let y = y0 + 2 * vi;
        for hi in 0..8 {
            let p = px(x0 + hi, y);
            s += p * p;
        }
    }
    s << 11
}

/// Fork variance producer for one 64x64 SB of SOURCE luma. Edge SBs read
/// the padded picture; C pads by edge replication, reproduced by clamping.
pub fn compute_sb_variances(
    luma: &[u8],
    stride: usize,
    frame_w: usize,
    frame_h: usize,
    sb_x: usize,
    sb_y: usize,
) -> SbVariance {
    let px = |x: usize, y: usize| -> u64 {
        u64::from(luma[y.min(frame_h - 1) * stride + x.min(frame_w - 1)])
    };

    // Level 0: the 64 8x8 sub-sampled means / mean-squares.
    let mut m8 = [0u64; 64];
    let mut sq8 = [0u64; 64];
    for row in 0..8 {
        for col in 0..8 {
            let idx = row * 8 + col;
            m8[idx] = sub_mean_8x8(&px, sb_x + col * 8, sb_y + row * 8);
            sq8[idx] = sub_mean_sq_8x8(&px, sb_x + col * 8, sb_y + row * 8);
        }
    }

    // fork SVT_VAR_STORE(x, 16) = f64(x) / 65536.
    let store = |meansq: u64, mean: u64| (meansq as i64 - (mean * mean) as i64) as f64 / 65536.0;

    let mut var8 = [0f64; 64];
    for i in 0..64 {
        var8[i] = store(sq8[i], m8[i]);
    }

    // Pyramid: 16x16 = avg of 4 children >> 2 (C exact integer shifts).
    let mut m16 = [0u64; 16];
    let mut sq16 = [0u64; 16];
    for r in 0..4 {
        for c in 0..4 {
            let f = (r * 2) * 8 + c * 2; // first 8x8 child index
            m16[r * 4 + c] = (m8[f] + m8[f + 1] + m8[f + 8] + m8[f + 9]) >> 2;
            sq16[r * 4 + c] = (sq8[f] + sq8[f + 1] + sq8[f + 8] + sq8[f + 9]) >> 2;
        }
    }
    let mut m32 = [0u64; 4];
    let mut sq32 = [0u64; 4];
    for r in 0..2 {
        for c in 0..2 {
            let f = (r * 2) * 4 + c * 2; // first 16x16 child index
            m32[r * 2 + c] = (m16[f] + m16[f + 1] + m16[f + 4] + m16[f + 5]) >> 2;
            sq32[r * 2 + c] = (sq16[f] + sq16[f + 1] + sq16[f + 4] + sq16[f + 5]) >> 2;
        }
    }
    let m64 = (m32[0] + m32[1] + m32[2] + m32[3]) >> 2;
    let sq64 = (sq32[0] + sq32[1] + sq32[2] + sq32[3]) >> 2;

    SbVariance {
        var_8x8: var8,
        var_64x64: store(sq64, m64),
        mean: m64,
    }
}

/// Result of the frame-level variance-boost pass.
#[derive(Debug, Clone)]
pub struct SbQindexPlan {
    /// The recentered frame base qindex to signal in the FH.
    pub base_qindex: u8,
    /// Per-SB qindexes in SB raster order (post-normalization).
    pub sb_qindex: Vec<u8>,
    /// FH `delta_q_res` (1/2/4/8).
    pub delta_q_res: u8,
}

/// C `svt_av1_normalize_sb_delta_q` (rc_aq.c:827-868) — MAINLINE, and shared:
/// this is the ONE definition in the C tree (it sits outside every
/// `#if SVT_HDR_MODE` block, unlike `svt_av1_variance_adjust_qp` /
/// `av1_get_deltaq_sb_variance_boost`, which are defined twice), so BOTH the
/// fork and the mainline arm below call it — like the C call site in
/// `generate_sb_qindex` (rc_process.c:741-744), which runs unconditionally
/// after `svt_av1_rc_init_sb_qindex` whenever
/// `delta_q_present && delta_q_res != 1`.
///
/// C has a SECOND call site, `recode_loop_decision_maker`
/// (enc_dec_process.c:2065-2068), which reruns the boost + this normalizer
/// inside the rate-control recode loop. It is unreachable here (the port has
/// no recode loop: `do_recode` is a VBR/CBR bitrate-targeting decision and the
/// still/CQP path never recodes). Worth knowing when reading the base-keying
/// note below: that site passes `readjust_base_q_idx = false`
/// (enc_dec_process.c:2056-2057) even in the FORK build, so "the fork
/// resignals the base" is a property of the `rc_init_sb_qindex` call site,
/// not of the fork build as such — which is exactly why the base is a
/// PARAMETER here rather than something this function derives.
///
/// It snaps every SB qindex onto the residue class of the FRAME base modulo
/// `delta_q_res`, which is what makes the pack's TRUNCATING integer divide
/// `(cur - prev) / delta_q_res` (entropy_coding.c:5002) exact. Without it the
/// encoder stores `prev = cur` while a conforming decoder stores
/// `prev = prev + reduced * delta_q_res` — the residues never cancel, so the
/// error COMPOUNDS across the SB raster and the two sides dequantize with
/// different qindexes. That is a corruption class, not a rate inefficiency.
///
/// `base_q_idx` is whatever the FRAME HEADER will signal — which differs by
/// mode, and is why this takes it as a parameter rather than reading a
/// "normalized base": the fork arm resignals the recentered base
/// (rc_aq.c:299-306, `if (readjust_base_q_idx)`), while MAINLINE never touches
/// `ppcs->frm_hdr.quantization_params.base_q_idx` at all (rc_aq.c:455 is
/// `(void)readjust_base_q_idx`), so the mainline call must key on the ORIGINAL
/// frame base. Keying mainline on the recentered value would put every SB in
/// the wrong residue class and reintroduce the same drift.
///
/// C exactness notes: `mask = ~(delta_q_res - 1)` is a `uint8_t` there, so
/// `adjusted & mask` clears the low `log2(res)` bits of a value already
/// clamped to `1..=255` — identical to the `i32` `!(res - 1)` used here for
/// every reachable input. `normalized == 0` (reachable only when
/// `adjusted < res` and the base's remainder is 0) is remapped to `delta_q_res`
/// because qindex 0 means lossless.
pub fn normalize_sb_delta_q(base_q_idx: u8, delta_q_res: u8, sb_qindex: &mut [i32]) {
    debug_assert!(
        matches!(delta_q_res, 2 | 4 | 8),
        "C asserts res in {{2,4,8}}"
    );
    let res = i32::from(delta_q_res);
    let mask = !(res - 1);
    let remainder = i32::from(base_q_idx) & !mask;
    // Push each SB toward the nearest multiple of `res` RELATIVE to the base
    // before truncating (C's `(res - remainder) - (res / 2)`).
    let adjustment = (res - remainder) - (res / 2);
    for q in sb_qindex.iter_mut() {
        let adjusted = (*q + adjustment).clamp(1, MAXQ);
        let normalized = (adjusted & mask) + remainder;
        *q = if normalized == 0 { res } else { normalized };
    }
}

/// C `get_delta_q_res` (resource_coordination_process.c:319).
pub fn delta_q_res_for(cli_qp: u8, enable_variance_boost: bool) -> u8 {
    if !enable_variance_boost {
        return 1;
    }
    let qindex = i32::from(crate::rate_control::qp_to_qindex(cli_qp));
    if qindex >= 160 {
        8
    } else if qindex >= 120 {
        4
    } else if qindex >= 80 {
        2
    } else {
        1
    }
}

/// The fork `svt_av1_variance_adjust_qp(pcs, true)` +
/// `svt_av1_normalize_sb_delta_q` chain for a still/KEY frame.
/// Mainline twin of [`variance_adjust_qp`] — C `svt_av1_variance_adjust_qp`
/// (rc_aq.c:454) with the mainline boost kernel (rc_aq.c:350).
///
/// Takes the INTEGER per-b64 variance maps (`pd0::compute_b64_variance`, the
/// same array C's picture analysis fills and `ppcs->variance[sb_addr]` hands
/// the boost) rather than the fork's f64 maps. Everything after the boost —
/// the base recentering and the +-range/2 offset clamp — is identical to the
/// fork path, because C shares that code between the two builds; only mainline
/// never writes `normalized_base_q_idx` back to the frame header
/// (`readjust_base_q_idx` is `(void)`-ignored at rc_aq.c:455), which this
/// function also does not do.
pub fn variance_adjust_qp_mainline(
    base_qindex: u8,
    variances: &[crate::pd0::SbVariance],
    strength: u8,
    octile: u8,
    curve: u8,
    cli_qp: u8,
    bit_depth: u8,
) -> SbQindexPlan {
    let max_range = VAR_BOOST_MAX_DELTAQ_RANGE;
    let mut sbq: alloc::vec::Vec<i32> = alloc::vec::Vec::with_capacity(variances.len());
    let mut min_q = MAXQ;
    let mut max_q = 0i32;
    for v in variances {
        let boost = var_boost::deltaq_sb_variance_boost_mainline(
            base_qindex,
            &v.0,
            strength,
            bit_depth,
            octile,
            curve,
        );
        let q = (i32::from(base_qindex) - boost).clamp(1, MAXQ);
        min_q = min_q.min(q);
        max_q = max_q.max(q);
        sbq.push(q);
    }
    let range = (max_q - min_q).min(max_range);
    let normalized_base = min_q + (range >> 1);
    for q in sbq.iter_mut() {
        let offset = (*q - normalized_base).clamp(-(max_range >> 1), max_range >> 1);
        *q = (normalized_base + offset).clamp(1, MAXQ);
    }

    // C `generate_sb_qindex` (rc_process.c:741-744) — MAINLINE, outside every
    // `#if SVT_HDR_MODE` block: `svt_av1_rc_init_sb_qindex` (which is where the
    // boost above lives) is ALWAYS followed by
    // `if (delta_q_present && delta_q_res != 1) svt_av1_normalize_sb_delta_q(pcs)`.
    // Skipping it desynchronizes the encoder from a conforming decoder (see
    // [`normalize_sb_delta_q`]). The base handed in is the ORIGINAL frame base,
    // because mainline never resignals it (rc_aq.c:455 `(void)readjust_base_q_idx`)
    // — `normalized_base` above only re-expresses the per-SB offsets and is NOT
    // what the frame header carries on this path.
    let res = delta_q_res_for(cli_qp, true);
    if res != 1 {
        normalize_sb_delta_q(base_qindex, res, &mut sbq);
    }

    SbQindexPlan {
        // MAINLINE keeps the frame base as-is: C's `readjust_base_q_idx` is
        // `(void)`-ignored (rc_aq.c:455), so `normalized_base_q_idx` only
        // re-expresses the per-SB values. (The fork path DOES resignal it.)
        base_qindex,
        delta_q_res: res,
        sb_qindex: sbq.iter().map(|&q| q as u8).collect(),
    }
}

pub fn variance_adjust_qp(
    base_qindex: u8,
    variances: &[SbVariance],
    strength: u8,
    octile: u8,
    curve: u8,
    cli_qp: u8,
    bit_depth: u8,
) -> SbQindexPlan {
    let max_range = if curve == 3 {
        VAR_BOOST_MAX_PQ_DELTAQ_RANGE
    } else {
        VAR_BOOST_MAX_DELTAQ_RANGE
    };

    // Pass 1: per-SB boost + min/max tracking (sb qindex starts at base).
    let mut sbq: Vec<i32> = Vec::with_capacity(variances.len());
    let mut min_q = MAXQ;
    let mut max_q = 0i32;
    for v in variances {
        let boost = var_boost::deltaq_sb_variance_boost(
            base_qindex,
            v.mean,
            &v.var_8x8,
            v.var_64x64,
            strength,
            octile,
            curve,
            bit_depth,
        );
        let q = (i32::from(base_qindex) - boost).clamp(1, MAXQ);
        min_q = min_q.min(q);
        max_q = max_q.max(q);
        sbq.push(q);
    }

    // Recenter the frame base (readjust_base_q_idx = true, KEY path).
    let range = (max_q - min_q).min(max_range);
    let normalized_base = min_q + (range >> 1);

    // Pass 2: clamp offsets to ±max_range/2 around the new base.
    // C: offset = MIN(offset, max_range>>1); offset = MAX(offset,
    // -max_range >> 1)  [note: -max_range >> 1, arithmetic shift of the
    // NEGATED value — same as -(max_range/2) for even max_range].
    for q in sbq.iter_mut() {
        let mut offset = *q - normalized_base;
        offset = offset.min(max_range >> 1);
        offset = offset.max(-max_range >> 1);
        *q = (normalized_base + offset).clamp(1, MAXQ);
    }

    // delta_q_res normalization (svt_av1_normalize_sb_delta_q, rc_aq.c:830 —
    // the same single C function the mainline arm calls; see
    // [`normalize_sb_delta_q`]). The FORK resignals the recentered base
    // (rc_aq.c:299-306), so THIS arm keys the residue class on
    // `normalized_base` — that is the value its frame header carries.
    let res = delta_q_res_for(cli_qp, true);
    if res != 1 {
        normalize_sb_delta_q(normalized_base as u8, res, &mut sbq);
    }

    SbQindexPlan {
        base_qindex: normalized_base as u8,
        sb_qindex: sbq.iter().map(|&q| q as u8).collect(),
        delta_q_res: res,
    }
}

// =============================================================================
// TPL-LA per-SB qindex — C `svt_aom_sb_qp_derivation_tpl_la` + `sb_setup_lambda`
// (rc_aq.c:695-780, 789-825)
// =============================================================================

/// C `svt_av1_get_deltaq_offset` (rc_aq.c:695-723).
///
/// Translates a TPL `beta` into a qindex delta by walking the DC-quantizer
/// ladder until the step crosses `q / sqrt(beta)` — or `q / sqrt(sqrt(beta))`
/// for a non-intra frame whose beta says the block deserves MORE bits
/// (`beta > 1` lowers q). The walk is C's own linear one.
#[must_use]
pub fn get_deltaq_offset(bit_depth: u8, qindex: i32, beta: f64, is_intra: bool) -> i32 {
    let dc_quant = |q: i32| -> i32 {
        let i = q.clamp(0, MAXQ) as usize;
        match bit_depth {
            8 => i32::from(svtav1_dsp::quant_tables::DC_QLOOKUP_8[i]),
            10 => i32::from(crate::bd10::DC_QLOOKUP_10[i]),
            _ => unreachable!("get_deltaq_offset: bit depth {bit_depth}"),
        }
    };
    let q = dc_quant(qindex);
    // `rint` — round-half-even — matches C's rint() on .5 cases.
    let newq = if !is_intra && beta > 1.0 {
        (q as f64 / beta.sqrt().sqrt()).round_ties_even() as i32
    } else {
        (q as f64 / beta.sqrt()).round_ties_even() as i32
    };
    let orig_qindex = qindex;
    if newq == q {
        return 0;
    }
    let mut qindex = qindex;
    if newq < q {
        while qindex > 0 {
            qindex -= 1;
            if newq >= dc_quant(qindex) {
                break;
            }
        }
    } else {
        while qindex < MAXQ {
            qindex += 1;
            if newq <= dc_quant(qindex) {
                break;
            }
        }
    }
    qindex - orig_qindex
}

/// C `svt_aom_sb_qp_derivation_tpl_la` (rc_aq.c:789-822) — the per-SB deltaq
/// half. `sb_qindex` is the per-SB plan initialized to `base_q_idx`; each
/// entry is adjusted by `get_deltaq_offset(beta)` clamped to ±35 then
/// `[1, MAXQ]`.
///
/// The `r0_delta_qp_quant -> delta_q_present = 1` write and the per-SB
/// `sb_setup_lambda` call are NOT here — the first is a frame-header flag the
/// caller sets, the second is [`sb_setup_lambda`] below (C calls it per SB
/// inside this same loop; the port splits it so the qindex plan and the
/// lambda-factor grid can be borrowed independently).
pub fn sb_qp_derivation_tpl_la(
    bit_depth: u8,
    is_intra: bool,
    tpl_beta: &[f64],
    sb_qindex: &mut [u8],
) {
    debug_assert_eq!(tpl_beta.len(), sb_qindex.len());
    for (sb, &beta) in sb_qindex.iter_mut().zip(tpl_beta.iter()) {
        let offset = get_deltaq_offset(bit_depth, i32::from(*sb), beta, is_intra).clamp(-35, 35);
        // `CLIP3(1, MAXQ, qindex + offset)` — qindex 0 is lossless, unused.
        *sb = (i32::from(*sb) + offset).clamp(1, MAXQ) as u8;
    }
}

/// C `sb_setup_lambda` (rc_aq.c:727-780) — propagate this SB's rdmult ratio
/// into `tpl_sb_rdmult_scaling_factors` over the synth-block grid cells the
/// SB covers, normalized so the geometric mean over the SB is preserved.
///
/// `tpl_rdmult_scaling_factors` is `pa_me_data->tpl_rdmult_scaling_factors`
/// (input); `tpl_sb_rdmult_scaling_factors` is the output grid of the SAME
/// length. `me_qindex` is `svt_aom_get_me_qindex(sb)` — the caller's
/// `b64_me_qindex` map lookup. `lctx` carries the frame's
/// [`crate::port_rc_process::LambdaContext`] for `compute_rd_mult`;
/// `bit_depth` is `hbd_md`'s depth (10 when the MD canvas is 10-bit, else 8).
/// Returns `true` — C sets `ppcs->blk_lambda_tuning = true` unconditionally.
#[allow(clippy::too_many_arguments)]
pub fn sb_setup_lambda(
    sb_org_x: u32,
    sb_org_y: u32,
    superres_denom: u8,
    enhanced_unscaled_width: u32,
    sb_size_is_128: bool,
    synth_blk_size: u8,
    mi_rows: i32,
    base_qindex: u8,
    sb_qindex: u8,
    me_qindex: u8,
    lctx: &crate::port_rc_process::LambdaContext,
    bit_depth: u8,
    tpl_rdmult_scaling_factors: &[f64],
    tpl_sb_rdmult_scaling_factors: &mut [f64],
) -> bool {
    let mi_col = (sb_org_x / 4) as i32;
    let mi_row = (sb_org_y / 4) as i32;

    let mi_col_sr = crate::port_md_lambda::coded_to_superres_mi(mi_col, i32::from(superres_denom));
    let mi_cols_sr = ((enhanced_unscaled_width as i32 + 15) / 16) << 2;
    let sb_mi_width_sr = crate::port_md_lambda::coded_to_superres_mi(
        if sb_size_is_128 { 32 } else { 16 },
        i32::from(superres_denom),
    );
    let (num_mi_w, num_mi_h) = if synth_blk_size == 32 {
        (8i32, 8i32)
    } else {
        (4, 4)
    };
    let num_cols = (mi_cols_sr + num_mi_w - 1) / num_mi_w;
    let num_rows = (mi_rows + num_mi_h - 1) / num_mi_h;
    let num_bcols = (sb_mi_width_sr + num_mi_w - 1) / num_mi_w;
    let num_brows = ((if sb_size_is_128 { 32i32 } else { 16 }) + num_mi_h - 1) / num_mi_h;

    let mut base_block_count = 0i32;
    let mut log_sum = 0.0f64;
    // C's row bound divides by `num_mi_w` and the column bound by `num_mi_h`
    // — a transposition that is harmless because both are equal (4 or 8).
    let row0 = mi_row / num_mi_w;
    let col0 = mi_col_sr / num_mi_h;
    for row in row0..num_rows.min(row0 + num_brows) {
        for col in col0..num_cols.min(col0 + num_bcols) {
            let index = (row * num_cols + col) as usize;
            log_sum += tpl_rdmult_scaling_factors[index].ln();
            base_block_count += 1;
        }
    }
    debug_assert!(base_block_count > 0);

    let orig_rdmult = f64::from(crate::port_rc_process::compute_rd_mult(
        lctx,
        base_qindex,
        base_qindex,
        bit_depth,
    ));
    let new_rdmult = f64::from(crate::port_rc_process::compute_rd_mult(
        lctx, sb_qindex, me_qindex, bit_depth,
    ));
    let scaling_factor = new_rdmult / orig_rdmult;
    let scale_adj = scaling_factor / (log_sum / f64::from(base_block_count)).exp();

    for row in row0..num_rows.min(row0 + num_brows) {
        for col in col0..num_cols.min(col0 + num_bcols) {
            let index = (row * num_cols + col) as usize;
            tpl_sb_rdmult_scaling_factors[index] = scale_adj * tpl_rdmult_scaling_factors[index];
        }
    }
    // C `ppcs->blk_lambda_tuning = true`.
    true
}

// ---------------------------------------------------------------------------
// Cyclic refresh (rc_aq.c:560-655) — the CBR per-SB qindex path
// ---------------------------------------------------------------------------

/// C `is_in_cr_band` (rc_aq.c:560) — the refresh band is `[sb_start, sb_end)`
/// in SB raster order; when `sb_start > sb_end` it wraps around the frame.
#[inline]
fn is_in_cr_band(b64_idx: u32, sb_start: u32, sb_end: u32) -> bool {
    if sb_start <= sb_end {
        b64_idx >= sb_start && b64_idx < sb_end
    } else {
        b64_idx >= sb_start || b64_idx < sb_end
    }
}

/// C `is_cr_motion_static` (rc_aq.c:566): the SB is eligible for refresh boost
/// when its 8x8 ME distortion is below `dist_reject_thresh` AND the 64x64
/// list-0 ME MV is within ±1 full pel (the `me_mv_array[0]` entry).
#[inline]
fn is_cr_motion_static(
    me_8x8_distortion: u32,
    mv: svtav1_types::motion::Mv,
    dist_reject_thresh: u64,
) -> bool {
    u64::from(me_8x8_distortion) < dist_reject_thresh
        && i32::from(mv.x).abs() <= 1
        && i32::from(mv.y).abs() <= 1
}

/// C `BOOST_MAX` (rc_aq.c:557).
const CR_BOOST_MAX: u64 = 10;

/// C `svt_aom_cyclic_refresh_setup` (rc_aq.c:571).
///
/// Refreshes `cr`'s per-segment statistics from this frame's ME data and may
/// switch `apply_cyclic_refresh` OFF when the motion gate rejects every SB in
/// the band — the `delta_q_present` overhead would then be pure syntax cost.
/// Runs inside `svt_av1_rc_calc_qindex_rate_control`'s CBR arm, after
/// `cyclic_refresh_init` has already positioned the band; the caller supplies
/// the callback there because this function lives in `rc_aq.c`, not
/// `rc_vbr_cbr.c`.
///
/// `per_b64` is the frame's ME result set: C reads
/// `me_results[b64]->me_mv_array[0]` (the 64x64 PU's list-0 MV) and the
/// `me_{8x8,64x64}_distortion[b64]` scalars off the same struct.
pub fn cyclic_refresh_setup(
    cr: &mut crate::port_rc_vbr_cbr_state::CyclicRefresh,
    b64_total_count: u32,
    norm_me_dist: u64,
    per_b64: &[crate::inter_me::context::MeB64Output],
) {
    cr.me_distortion = [0; 3];
    cr.actual_num_seg1_sbs = 0;
    cr.actual_num_seg2_sbs = 0;
    let mut seg2_dist: u64 = 0;
    let avg_me_dist = norm_me_dist;
    let dist_reject_thresh = avg_me_dist * 2 + 1;
    let dbg = crate::dbgenv::crdbg();
    for b64_idx in 0..b64_total_count as usize {
        let b64 = &per_b64[b64_idx];
        let in_cr_range = is_in_cr_band(b64_idx as u32, cr.sb_start, cr.sb_end);
        if dbg {
            eprintln!(
                "CRDBG b64={} in={} d8={} d64={} mv=({},{}) band=[{},{}] thresh={}",
                b64_idx,
                in_cr_range as u8,
                b64.me_8x8_distortion,
                b64.me_64x64_distortion,
                b64.me_mv_array[0].x,
                b64.me_mv_array[0].y,
                cr.sb_start,
                cr.sb_end,
                dist_reject_thresh
            );
        }
        if in_cr_range
            && is_cr_motion_static(
                b64.me_8x8_distortion,
                b64.me_mv_array[0],
                dist_reject_thresh,
            )
        {
            if u64::from(b64.me_8x8_distortion) < avg_me_dist {
                seg2_dist += u64::from(b64.me_8x8_distortion);
                cr.me_distortion[2] += u64::from(b64.me_64x64_distortion);
                cr.actual_num_seg2_sbs += 1;
            } else {
                cr.me_distortion[1] += u64::from(b64.me_64x64_distortion);
                cr.actual_num_seg1_sbs += 1;
            }
        } else {
            cr.me_distortion[0] += u64::from(b64.me_64x64_distortion);
        }
    }

    let actual_num_seg0_sbs =
        b64_total_count - cr.actual_num_seg1_sbs as u32 - cr.actual_num_seg2_sbs as u32;
    cr.me_distortion[0] = if actual_num_seg0_sbs != 0 {
        cr.me_distortion[0] / u64::from(actual_num_seg0_sbs)
    } else {
        0
    };
    cr.me_distortion[1] = if cr.actual_num_seg1_sbs != 0 {
        cr.me_distortion[1] / cr.actual_num_seg1_sbs as u64
    } else {
        0
    };
    cr.me_distortion[2] = if cr.actual_num_seg2_sbs != 0 {
        cr.me_distortion[2] / cr.actual_num_seg2_sbs as u64
    } else {
        0
    };

    // If the motion gate rejected ALL SBs in the refresh range, disable CR for
    // this frame to avoid delta_q_present signaling overhead with no actual
    // delta-Q benefit.
    if cr.actual_num_seg1_sbs + cr.actual_num_seg2_sbs == 0 {
        cr.apply_cyclic_refresh = false;
        return;
    }
    let mut rate_boost_fac = cr.rate_boost_fac;
    if cr.actual_num_seg2_sbs != 0 {
        seg2_dist /= cr.actual_num_seg2_sbs as u64;
        // Every term summed into seg2_dist was `< avg_me_dist`, so the mean
        // is too and this subtraction cannot underflow; `avg_me_dist` is
        // nonzero for the same reason (a zero mean admits no seg2 members).
        let dev = (avg_me_dist - seg2_dist) * 100 / avg_me_dist;
        // Quadratic scaling; boost = BOOST_MAX * (dev/100)^2.
        rate_boost_fac += (CR_BOOST_MAX * dev * dev / (100 * 100)) as i32;
    }
    cr.rate_ratio_qdelta_seg2 = 0.1 * rate_boost_fac as f64 * cr.rate_ratio_qdelta;
}

/// C `cyclic_sb_qp_assignment` (rc_aq.c:626) — the per-SB qindex write C's
/// `svt_av1_rc_init_sb_qindex` performs when `apply_cyclic_refresh` holds.
/// `delta_q_present = 1` is this function's header side effect; the caller
/// folds it into the [`SbQindexPlan`] it builds from the filled map.
///
/// Only valid at `sb_size == 64` — C's own comment ("only works for sb size =
/// 64") and `cyclic_refresh_init`'s `super_block_size != 64` gate agree.
pub fn cyclic_sb_qp_assignment(
    cr: &crate::port_rc_vbr_cbr_state::CyclicRefresh,
    base_q_idx: i32,
    norm_me_dist: u64,
    per_b64: &[crate::inter_me::context::MeB64Output],
    sb_qindex: &mut [u8],
) {
    let dist_reject_thresh = norm_me_dist * 2 + 1;
    for (b64_idx, q) in sb_qindex.iter_mut().enumerate() {
        let b64 = &per_b64[b64_idx];
        let mut offset = 0;
        if is_in_cr_band(b64_idx as u32, cr.sb_start, cr.sb_end) {
            if !is_cr_motion_static(
                b64.me_8x8_distortion,
                b64.me_mv_array[0],
                dist_reject_thresh,
            ) {
                // Non-static SB (any non-zero MV or high distortion): no boost.
                offset = 0;
            } else if u64::from(b64.me_8x8_distortion) < norm_me_dist {
                offset = cr.qindex_delta[2];
            } else {
                offset = cr.qindex_delta[1];
            }
        }
        *q = (base_q_idx + offset).clamp(1, MAXQ) as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn producer_flat_frame_zero_variance() {
        let luma = vec![128u8; 64 * 64];
        let v = compute_sb_variances(&luma, 64, 64, 64, 0, 0);
        assert!(v.var_8x8.iter().all(|&x| x == 0.0));
        assert_eq!(v.var_64x64, 0.0);
        // fp8 mean of a flat 128 frame = 128 << 8.
        assert_eq!(v.mean, 128 << 8);
    }

    #[test]
    fn producer_checkerboard_variance() {
        // 2-row alternating 0/255 stripes: the SUB producer samples rows
        // 0,2,4,6 of each 8x8 (all stripe-tops). Horizontal checker of
        // period 1 px: each sampled row alternates 0/255 -> per-8x8
        // mean fp8 = (4*4*255) << 3 = 32640; meansq fp16 = (16*65025)<<11.
        let mut luma = vec![0u8; 64 * 64];
        for y in 0..64 {
            for x in 0..64 {
                if x % 2 == 1 {
                    luma[y * 64 + x] = 255;
                }
            }
        }
        let v = compute_sb_variances(&luma, 64, 64, 64, 0, 0);
        let mean = (16u64 * 255) << 3;
        let meansq = (16u64 * 255 * 255) << 11;
        let expect = (meansq as i64 - (mean * mean) as i64) as f64 / 65536.0;
        for &x in &v.var_8x8 {
            assert_eq!(x, expect);
        }
        assert_eq!(v.mean, mean); // pyramid of identical children
    }

    #[test]
    fn delta_q_res_bands() {
        assert_eq!(delta_q_res_for(63, true), 8);
        assert_eq!(delta_q_res_for(40, true), 8);
        assert_eq!(delta_q_res_for(35, true), 4);
        assert_eq!(delta_q_res_for(25, true), 2);
        assert_eq!(delta_q_res_for(10, true), 1);
        assert_eq!(delta_q_res_for(63, false), 1);
    }

    #[test]
    fn flat_frame_uniform_boost_recenters() {
        let v = SbVariance {
            var_8x8: [0.5; 64],
            var_64x64: 0.5,
            mean: 30000,
        };
        let plan = variance_adjust_qp(200, &vec![v.clone(); 4], 2, 5, 0, 10, 8);
        assert!(plan.sb_qindex.iter().all(|&q| q == plan.base_qindex));
        assert!(plan.base_qindex < 200, "flat content must boost (lower q)");
        assert_eq!(plan.delta_q_res, 1);
    }

    /// The mainline arm must run `svt_av1_normalize_sb_delta_q`
    /// (rc_process.c:741-744), keyed on the ORIGINAL frame base — which is what
    /// mainline signals, since rc_aq.c:455 `(void)`s `readjust_base_q_idx`.
    ///
    /// Chosen so the two candidate bases land in DIFFERENT residue classes:
    /// cli_qp 55 -> base qindex 220, res 8, 220 % 8 == 4, while the recentered
    /// base the boost computes is a different value mod 8. Keying on the
    /// recentered base (i.e. copying the fork arm verbatim) fails this.
    #[test]
    fn mainline_plan_is_congruent_to_the_signalled_base() {
        let flat = crate::pd0::SbVariance([2u16; 85]);
        let tex = crate::pd0::SbVariance([3000u16; 85]);
        let mid = crate::pd0::SbVariance([48u16; 85]);
        let vars = [flat, mid, tex, mid];
        let base = crate::rate_control::qp_to_qindex(55);
        assert_eq!(base, 220);
        let plan = variance_adjust_qp_mainline(base, &vars, 3, 6, 2, 55, 8);
        assert_eq!(plan.delta_q_res, 8);
        assert_eq!(plan.base_qindex, base, "mainline never resignals the base");
        // Non-vacuity: the boost must actually spread the SBs apart.
        assert!(plan.sb_qindex.iter().any(|&q| q != plan.sb_qindex[0]));
        for (i, &q) in plan.sb_qindex.iter().enumerate() {
            assert_eq!(
                (i32::from(q) - i32::from(base)).rem_euclid(8),
                0,
                "sb {i} qindex {q} is not congruent to base {base} mod 8 — the \
                 pack's truncating (cur-prev)/res would desync the decoder"
            );
        }
    }

    /// The helper is the port of the ONE C definition (rc_aq.c:830); pin its
    /// two hand-traceable edge behaviors: the `normalized == 0` -> `delta_q_res`
    /// remap (qindex 0 is lossless), and the nonzero-remainder residue class.
    #[test]
    fn normalize_sb_delta_q_edges() {
        // base 8 (remainder 0), res 8: adjustment = (8-0)-4 = 4.
        // q=1 -> adjusted 5 -> 5 & !7 = 0 -> +0 = 0 -> remapped to res (8).
        let mut q = [1i32];
        normalize_sb_delta_q(8, 8, &mut q);
        assert_eq!(q, [8]);
        // base 220 (220 % 8 == 4), res 8: adjustment = (8-4)-4 = 0.
        // q=200 -> 200 & !7 = 200 -> +4 = 204 == 220 - 16 (same class).
        let mut q = [200i32, 221, 255];
        normalize_sb_delta_q(220, 8, &mut q);
        assert_eq!(q, [204, 220, 252]);
        for &v in &q {
            assert_eq!((v - 220).rem_euclid(8), 0);
        }
    }

    #[test]
    fn mixed_frame_offsets_clamped_and_normalized() {
        let flat = SbVariance {
            var_8x8: [0.5; 64],
            var_64x64: 0.5,
            mean: 30000,
        };
        let tex = SbVariance {
            var_8x8: [4096.0; 64],
            var_64x64: 4096.0,
            mean: 30000,
        };
        let plan = variance_adjust_qp(200, &[flat, tex], 2, 5, 0, 40, 8);
        let res = i32::from(plan.delta_q_res);
        assert_eq!(res, 8);
        for &q in &plan.sb_qindex {
            let d = i32::from(q) - i32::from(plan.base_qindex);
            assert_eq!(d.rem_euclid(res), 0, "q {q} base {}", plan.base_qindex);
            assert!(d.abs() <= 40);
        }
        assert!(plan.sb_qindex[0] < plan.sb_qindex[1]);
    }
}
