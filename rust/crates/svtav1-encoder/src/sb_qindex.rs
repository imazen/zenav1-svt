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
pub fn variance_adjust_qp(
    base_qindex: u8,
    variances: &[SbVariance],
    strength: u8,
    octile: u8,
    curve: u8,
    cli_qp: u8,
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

    // delta_q_res normalization (svt_av1_normalize_sb_delta_q), res != 1.
    let res = delta_q_res_for(cli_qp, true);
    if res != 1 {
        let resi = i32::from(res);
        let mask = !(resi - 1);
        let remainder = normalized_base & !mask;
        let adjustment = (resi - remainder) - (resi / 2);
        for q in sbq.iter_mut() {
            let adjusted = (*q + adjustment).clamp(1, MAXQ);
            let normalized = (adjusted & mask) + remainder;
            *q = if normalized == 0 { resi } else { normalized };
        }
    }

    SbQindexPlan {
        base_qindex: normalized_base as u8,
        sb_qindex: sbq.iter().map(|&q| q as u8).collect(),
        delta_q_res: res,
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
        let v = SbVariance { var_8x8: [0.5; 64], var_64x64: 0.5, mean: 30000 };
        let plan = variance_adjust_qp(200, &vec![v.clone(); 4], 2, 5, 0, 10);
        assert!(plan.sb_qindex.iter().all(|&q| q == plan.base_qindex));
        assert!(plan.base_qindex < 200, "flat content must boost (lower q)");
        assert_eq!(plan.delta_q_res, 1);
    }

    #[test]
    fn mixed_frame_offsets_clamped_and_normalized() {
        let flat = SbVariance { var_8x8: [0.5; 64], var_64x64: 0.5, mean: 30000 };
        let tex = SbVariance { var_8x8: [4096.0; 64], var_64x64: 4096.0, mean: 30000 };
        let plan = variance_adjust_qp(200, &[flat, tex], 2, 5, 0, 40);
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
