//! Tier-suffixed block SAD primitives for the motion-estimation search loops.
//!
//! # Why this module exists separately from [`crate::sad`]
//!
//! [`crate::sad::sad`] is a *dispatching* entry point: it summons a token on
//! every call. The ME search loops call a block SAD once per **search
//! position** — tens of thousands of times per superblock — so a per-call
//! `incant!` would put a target-feature boundary inside the hot loop, which
//! archmage measures at ~4x (`README.md`, "The target-feature boundary").
//!
//! This module therefore exports the block SAD as **tier-suffixed `#[arcane]`
//! helpers** that a caller invokes from inside its OWN `#[arcane]` body, after
//! summoning the token once outside the search loop. [`block_sad`] is provided
//! for the handful of call sites that are genuinely one-shot.
//!
//! # Exactness
//!
//! Every variant computes `sum |src[y][x] - ref[y][x]|` over `w * h` 8-bit
//! samples. Integer absolute difference and integer addition are exact and
//! associative, and the maximum possible total (`255 * 128 * 128 = 4_177_920`)
//! is far inside `u32`, so **every variant returns bit-identical results
//! regardless of lane order or accumulator width**. `me_sad_all_tiers_agree`
//! pins that across every archmage token permutation.
//!
//! # Why not `#[magetypes]`
//!
//! magetypes 0.9.28 exposes no integer-widening conversion (`u8x16 -> u16x8`),
//! no `abs_diff`, and no pairwise-widening accumulate; `U8x16Backend`'s only
//! reduction is `reduce_add(..) -> u8`, which wraps. A u8 SAD reducing into
//! `u32` therefore cannot be expressed against the generic types at all — this
//! is the documented "one tier benefits from something the generic API can't
//! express" case, so the arms are hand-written per ISA and dispatched by
//! `incant!`, which is also what [`crate::sad`] already does.
//!
//! The `arm_v2` arm is the one that matches C: `Arm64V2Token` bundles
//! `dotprod`, so `vabdq_u8` + `vdotq_u32` reproduces the shape of C's
//! `svt_sad_loop_kernel*_neon_dotprod` without a widening step.

use archmage::prelude::*;

/// Scalar reference implementation. Also the `incant!` fallback arm.
///
/// C `svt_nxm_sad_kernel_helper_c` (`C_DEFAULT/compute_sad_c.c:21`).
pub fn block_sad_scalar(
    _token: ScalarToken,
    src: &[u8],
    src_stride: usize,
    rf: &[u8],
    ref_stride: usize,
    w: usize,
    h: usize,
) -> u32 {
    let mut sad = 0u32;
    for y in 0..h {
        let s = &src[y * src_stride..y * src_stride + w];
        let r = &rf[y * ref_stride..y * ref_stride + w];
        for x in 0..w {
            sad += u32::from(s[x].abs_diff(r[x]));
        }
    }
    sad
}

// --- AArch64 NEON (no dotprod) ---

/// NEON block SAD: `vabdq_u8` + pairwise-widening accumulate.
///
/// The `u16` row accumulator cannot overflow: each 16-lane chunk contributes
/// at most `2 * 255 = 510` to a lane, and the widest row here is 128 px
/// (8 chunks, `4080`), plus at most `255` from the 8-wide remainder.
#[cfg(target_arch = "aarch64")]
#[arcane]
pub fn block_sad_neon(
    _token: NeonToken,
    src: &[u8],
    src_stride: usize,
    rf: &[u8],
    ref_stride: usize,
    w: usize,
    h: usize,
) -> u32 {
    let mut acc = vdupq_n_u32(0);
    let mut tail = 0u32;
    for y in 0..h {
        let so = y * src_stride;
        let ro = y * ref_stride;
        let mut c = 0usize;
        let mut racc = vdupq_n_u16(0);
        while c + 16 <= w {
            let a: &[u8; 16] = src[so + c..so + c + 16].try_into().unwrap();
            let b: &[u8; 16] = rf[ro + c..ro + c + 16].try_into().unwrap();
            racc = vpadalq_u8(racc, vabdq_u8(vld1q_u8(a), vld1q_u8(b)));
            c += 16;
        }
        if c + 8 <= w {
            let a: &[u8; 8] = src[so + c..so + c + 8].try_into().unwrap();
            let b: &[u8; 8] = rf[ro + c..ro + c + 8].try_into().unwrap();
            racc = vaddq_u16(racc, vmovl_u8(vabd_u8(vld1_u8(a), vld1_u8(b))));
            c += 8;
        }
        acc = vpadalq_u16(acc, racc);
        while c < w {
            tail += u32::from(src[so + c].abs_diff(rf[ro + c]));
            c += 1;
        }
    }
    vaddvq_u32(acc) + tail
}

// --- AArch64 with dotprod (Arm64V2Token bundles `dotprod`) ---

/// NEON-dotprod block SAD — the shape of C's `*_neon_dotprod` kernels.
///
/// `vdotq_u32` accumulates four byte-lanes straight into a `u32` lane, so
/// there is no widening step and no per-row drain: a lane grows by at most
/// `4 * 255 = 1020` per chunk and the whole 128x128 worst case is 4.2 M.
#[cfg(target_arch = "aarch64")]
#[arcane]
pub fn block_sad_arm_v2(
    _token: Arm64V2Token,
    src: &[u8],
    src_stride: usize,
    rf: &[u8],
    ref_stride: usize,
    w: usize,
    h: usize,
) -> u32 {
    let ones_q = vdupq_n_u8(1);
    let ones_d = vdup_n_u8(1);
    let mut acc = vdupq_n_u32(0);
    let mut acc8 = vdup_n_u32(0);
    let mut tail = 0u32;
    for y in 0..h {
        let so = y * src_stride;
        let ro = y * ref_stride;
        let mut c = 0usize;
        while c + 16 <= w {
            let a: &[u8; 16] = src[so + c..so + c + 16].try_into().unwrap();
            let b: &[u8; 16] = rf[ro + c..ro + c + 16].try_into().unwrap();
            acc = vdotq_u32(acc, vabdq_u8(vld1q_u8(a), vld1q_u8(b)), ones_q);
            c += 16;
        }
        if c + 8 <= w {
            let a: &[u8; 8] = src[so + c..so + c + 8].try_into().unwrap();
            let b: &[u8; 8] = rf[ro + c..ro + c + 8].try_into().unwrap();
            acc8 = vdot_u32(acc8, vabd_u8(vld1_u8(a), vld1_u8(b)), ones_d);
            c += 8;
        }
        while c < w {
            tail += u32::from(src[so + c].abs_diff(rf[ro + c]));
            c += 1;
        }
    }
    vaddvq_u32(acc) + vaddv_u32(acc8) + tail
}

// --- x86-64 AVX2 ---

/// AVX2 block SAD: `_mm256_sad_epu8` / `_mm_sad_epu8`, reduced once at the end.
///
/// The 64-bit lanes cannot overflow: each `_mm256_sad_epu8` lane adds at most
/// `8 * 255 = 2040`, and the worst case here is 512 chunks.
#[cfg(target_arch = "x86_64")]
#[arcane]
pub fn block_sad_v3(
    _token: Desktop64,
    src: &[u8],
    src_stride: usize,
    rf: &[u8],
    ref_stride: usize,
    w: usize,
    h: usize,
) -> u32 {
    let mut acc256 = _mm256_setzero_si256();
    let mut acc128 = _mm_setzero_si128();
    let mut tail = 0u32;
    for y in 0..h {
        let so = y * src_stride;
        let ro = y * ref_stride;
        let mut c = 0usize;
        while c + 32 <= w {
            let a: &[u8; 32] = src[so + c..so + c + 32].try_into().unwrap();
            let b: &[u8; 32] = rf[ro + c..ro + c + 32].try_into().unwrap();
            let d = _mm256_sad_epu8(_mm256_loadu_si256(a), _mm256_loadu_si256(b));
            acc256 = _mm256_add_epi64(acc256, d);
            c += 32;
        }
        while c + 16 <= w {
            let a: &[u8; 16] = src[so + c..so + c + 16].try_into().unwrap();
            let b: &[u8; 16] = rf[ro + c..ro + c + 16].try_into().unwrap();
            acc128 = _mm_add_epi64(acc128, _mm_sad_epu8(_mm_loadu_si128(a), _mm_loadu_si128(b)));
            c += 16;
        }
        if c + 8 <= w {
            let a: &[u8; 8] = src[so + c..so + c + 8].try_into().unwrap();
            let b: &[u8; 8] = rf[ro + c..ro + c + 8].try_into().unwrap();
            acc128 = _mm_add_epi64(acc128, _mm_sad_epu8(_mm_loadu_si64(a), _mm_loadu_si64(b)));
            c += 8;
        }
        while c < w {
            tail += u32::from(src[so + c].abs_diff(rf[ro + c]));
            c += 1;
        }
    }
    let lo = _mm256_castsi256_si128(acc256);
    let hi = _mm256_extracti128_si256::<1>(acc256);
    let s = _mm_add_epi64(_mm_add_epi64(lo, hi), acc128);
    let s = _mm_add_epi64(s, _mm_srli_si128::<8>(s));
    (_mm_cvtsi128_si64(s) as u64 as u32) + tail
}

// ---------------------------------------------------------------------------
// Sum / sum-of-squares, the other shape the ME distortions need.
//
// `variance_c` (`C_DEFAULT/variance.c:141`) and every caller of it wants
//   sum = SUM(a - b)   and   sse = SUM((a - b)^2)
// over `w * h` 8-bit samples, and then `sse - (sum * sum) / n`.
//
// Both are computed exactly without a signed SIMD subtract:
//   * `sum` is `SUM(a) - SUM(b)`, two unsigned reductions,
//   * `sse` is `SUM(|a - b|^2)`, and `|d|^2 == d^2`.
// Ranges at the 128x128 worst case: `SUM(a) <= 4_177_920` (u32), and
// `sse <= 255^2 * 16384 = 1_065_369_600` (u32). Nothing can overflow the
// accumulators below, and integer addition is associative, so every tier
// returns the identical pair.
// ---------------------------------------------------------------------------

/// Scalar reference for [`block_sum_sse`]. Returns `(SUM(a - b), SUM((a-b)^2))`.
pub fn block_sum_sse_scalar(
    _token: ScalarToken,
    a: &[u8],
    a_stride: usize,
    b: &[u8],
    b_stride: usize,
    w: usize,
    h: usize,
) -> (i32, u32) {
    let mut sum: i32 = 0;
    let mut sse: u32 = 0;
    for y in 0..h {
        let ao = y * a_stride;
        let bo = y * b_stride;
        for x in 0..w {
            let d = i32::from(a[ao + x]) - i32::from(b[bo + x]);
            sum += d;
            sse += (d * d) as u32;
        }
    }
    (sum, sse)
}

#[cfg(target_arch = "aarch64")]
#[arcane]
pub fn block_sum_sse_neon(
    _token: NeonToken,
    a: &[u8],
    a_stride: usize,
    b: &[u8],
    b_stride: usize,
    w: usize,
    h: usize,
) -> (i32, u32) {
    let mut acc_a = vdupq_n_u32(0);
    let mut acc_b = vdupq_n_u32(0);
    let mut acc_sse = vdupq_n_u32(0);
    let mut tail_sum: i32 = 0;
    let mut tail_sse: u32 = 0;
    for y in 0..h {
        let ao = y * a_stride;
        let bo = y * b_stride;
        let mut c = 0usize;
        let mut ra = vdupq_n_u16(0);
        let mut rb = vdupq_n_u16(0);
        while c + 16 <= w {
            let av: &[u8; 16] = a[ao + c..ao + c + 16].try_into().unwrap();
            let bv: &[u8; 16] = b[bo + c..bo + c + 16].try_into().unwrap();
            let va = vld1q_u8(av);
            let vb = vld1q_u8(bv);
            ra = vpadalq_u8(ra, va);
            rb = vpadalq_u8(rb, vb);
            let d = vabdq_u8(va, vb);
            // |d| <= 255 so d*d <= 65025, inside u16; the widening pairwise
            // accumulate then drains into u32.
            acc_sse = vpadalq_u16(acc_sse, vmull_u8(vget_low_u8(d), vget_low_u8(d)));
            acc_sse = vpadalq_u16(acc_sse, vmull_high_u8(d, d));
            c += 16;
        }
        if c + 8 <= w {
            let av: &[u8; 8] = a[ao + c..ao + c + 8].try_into().unwrap();
            let bv: &[u8; 8] = b[bo + c..bo + c + 8].try_into().unwrap();
            let va = vld1_u8(av);
            let vb = vld1_u8(bv);
            ra = vaddq_u16(ra, vmovl_u8(va));
            rb = vaddq_u16(rb, vmovl_u8(vb));
            let d = vabd_u8(va, vb);
            acc_sse = vpadalq_u16(acc_sse, vmull_u8(d, d));
            c += 8;
        }
        acc_a = vpadalq_u16(acc_a, ra);
        acc_b = vpadalq_u16(acc_b, rb);
        while c < w {
            let d = i32::from(a[ao + c]) - i32::from(b[bo + c]);
            tail_sum += d;
            tail_sse += (d * d) as u32;
            c += 1;
        }
    }
    let sum = (vaddvq_u32(acc_a) as i32) - (vaddvq_u32(acc_b) as i32) + tail_sum;
    (sum, vaddvq_u32(acc_sse) + tail_sse)
}

#[cfg(target_arch = "x86_64")]
#[arcane]
pub fn block_sum_sse_v3(
    _token: Desktop64,
    a: &[u8],
    a_stride: usize,
    b: &[u8],
    b_stride: usize,
    w: usize,
    h: usize,
) -> (i32, u32) {
    let mut acc_a = _mm256_setzero_si256();
    let mut acc_b = _mm256_setzero_si256();
    let mut acc_sse = _mm256_setzero_si256();
    let mut tail_sum: i32 = 0;
    let mut tail_sse: u32 = 0;
    for y in 0..h {
        let ao = y * a_stride;
        let bo = y * b_stride;
        let mut c = 0usize;
        while c + 16 <= w {
            let av: &[u8; 16] = a[ao + c..ao + c + 16].try_into().unwrap();
            let bv: &[u8; 16] = b[bo + c..bo + c + 16].try_into().unwrap();
            // Widen the 16 bytes into the 256-bit lane so one code path
            // covers both halves.
            let va = _mm256_cvtepu8_epi16(_mm_loadu_si128(av));
            let vb = _mm256_cvtepu8_epi16(_mm_loadu_si128(bv));
            acc_a = _mm256_add_epi32(acc_a, _mm256_madd_epi16(va, _mm256_set1_epi16(1)));
            acc_b = _mm256_add_epi32(acc_b, _mm256_madd_epi16(vb, _mm256_set1_epi16(1)));
            let d = _mm256_sub_epi16(va, vb);
            acc_sse = _mm256_add_epi32(acc_sse, _mm256_madd_epi16(d, d));
            c += 16;
        }
        if c + 8 <= w {
            let av: &[u8; 8] = a[ao + c..ao + c + 8].try_into().unwrap();
            let bv: &[u8; 8] = b[bo + c..bo + c + 8].try_into().unwrap();
            let va = _mm256_cvtepu8_epi16(_mm_loadu_si64(av));
            let vb = _mm256_cvtepu8_epi16(_mm_loadu_si64(bv));
            acc_a = _mm256_add_epi32(acc_a, _mm256_madd_epi16(va, _mm256_set1_epi16(1)));
            acc_b = _mm256_add_epi32(acc_b, _mm256_madd_epi16(vb, _mm256_set1_epi16(1)));
            let d = _mm256_sub_epi16(va, vb);
            acc_sse = _mm256_add_epi32(acc_sse, _mm256_madd_epi16(d, d));
            c += 8;
        }
        while c < w {
            let d = i32::from(a[ao + c]) - i32::from(b[bo + c]);
            tail_sum += d;
            tail_sse += (d * d) as u32;
            c += 1;
        }
    }
    let red = |v: __m256i| -> i32 {
        let lo = _mm256_castsi256_si128(v);
        let hi = _mm256_extracti128_si256::<1>(v);
        let s = _mm_add_epi32(lo, hi);
        let s = _mm_add_epi32(s, _mm_shuffle_epi32::<0b01_00_11_10>(s));
        let s = _mm_add_epi32(s, _mm_shuffle_epi32::<0b00_01_00_01>(s));
        _mm_cvtsi128_si32(s)
    };
    let sum = red(acc_a) - red(acc_b) + tail_sum;
    (sum, (red(acc_sse) as u32).wrapping_add(tail_sse))
}

/// Dispatching `(SUM(a - b), SUM((a - b)^2))` for the one-shot call sites.
pub fn block_sum_sse(
    a: &[u8],
    a_stride: usize,
    b: &[u8],
    b_stride: usize,
    w: usize,
    h: usize,
) -> (i32, u32) {
    incant!(
        block_sum_sse(a, a_stride, b, b_stride, w, h),
        [v3, neon, scalar]
    )
}

// --- one-shot dispatching entry point ---

/// Dispatching block SAD, for the call sites that are NOT inside a search
/// loop. Inside a loop, summon once and call the `_arm_v2` / `_neon` / `_v3` /
/// `_scalar` helper directly.
pub fn block_sad(
    src: &[u8],
    src_stride: usize,
    rf: &[u8],
    ref_stride: usize,
    w: usize,
    h: usize,
) -> u32 {
    incant!(
        block_sad(src, src_stride, rf, ref_stride, w, h),
        [arm_v2, v3, neon, scalar]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;
    use archmage::testing::{CompileTimePolicy, for_each_token_permutation};

    fn plane(seed: u32, n: usize) -> Vec<u8> {
        let mut s = seed | 1;
        (0..n)
            .map(|_| {
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (s >> 24) as u8
            })
            .collect()
    }

    /// Every (w, h) the ME search actually uses, plus the odd widths that
    /// exercise the 8-wide remainder and the scalar tail.
    const SIZES: &[(usize, usize)] = &[
        (4, 4),
        (8, 4),
        (8, 8),
        (16, 8),
        (16, 16),
        (24, 16),
        (32, 32),
        (48, 32),
        (64, 64),
        (128, 64),
        (128, 128),
        (12, 6),
        (20, 3),
        (5, 7),
    ];

    #[test]
    fn me_sad_all_tiers_agree() {
        let stride = 160usize;
        let src = plane(7, stride * 160);
        let rf = plane(1_234_567, stride * 160);
        for &(w, h) in SIZES {
            let want = block_sad_scalar(
                ScalarToken::summon().unwrap(),
                &src,
                stride,
                &rf,
                stride,
                w,
                h,
            );
            // Independent recomputation, so the reference is not the arm.
            let mut check = 0u32;
            for y in 0..h {
                for x in 0..w {
                    check += u32::from(src[y * stride + x].abs_diff(rf[y * stride + x]));
                }
            }
            assert_eq!(want, check, "scalar {w}x{h}");

            let report = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_| {
                assert_eq!(
                    block_sad(&src, stride, &rf, stride, w, h),
                    want,
                    "tier mismatch at {w}x{h}"
                );
            });
            assert!(report.warnings.is_empty(), "excluded tokens: {report:?}");
            assert!(
                report.permutations_run >= 2,
                "no dispatch coverage: {report:?}"
            );
        }
    }

    #[test]
    fn me_sad_handles_distinct_strides_and_offsets() {
        let (ss, rs) = (137usize, 96usize);
        let src = plane(99, ss * 140);
        let rf = plane(4_242, rs * 140);
        for &(w, h) in SIZES {
            let mut want = 0u32;
            for y in 0..h {
                for x in 0..w {
                    want += u32::from(src[y * ss + x].abs_diff(rf[y * rs + x]));
                }
            }
            assert_eq!(block_sad(&src, ss, &rf, rs, w, h), want, "{w}x{h}");
        }
    }

    #[test]
    fn me_sum_sse_all_tiers_agree() {
        let (as_, bs) = (137usize, 96usize);
        let a = plane(31, as_ * 140);
        let b = plane(9_871, bs * 140);
        for &(w, h) in SIZES {
            let mut sum = 0i32;
            let mut sse = 0u32;
            for y in 0..h {
                for x in 0..w {
                    let d = i32::from(a[y * as_ + x]) - i32::from(b[y * bs + x]);
                    sum += d;
                    sse += (d * d) as u32;
                }
            }
            let report = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_| {
                assert_eq!(block_sum_sse(&a, as_, &b, bs, w, h), (sum, sse), "{w}x{h}");
            });
            assert!(report.warnings.is_empty(), "excluded tokens: {report:?}");
            assert!(
                report.permutations_run >= 2,
                "no dispatch coverage: {report:?}"
            );
        }
    }

    /// The u32 `sse` accumulator's worst case is `255^2 * 128 * 128`, which is
    /// 1_065_369_600 — inside u32 but only by 4x, so it is asserted rather
    /// than argued.
    #[test]
    fn me_sum_sse_extremes() {
        let a = [0u8; 128 * 128];
        let b = [255u8; 128 * 128];
        assert_eq!(
            block_sum_sse(&a, 128, &b, 128, 128, 128),
            (-(255 * 128 * 128), 255 * 255 * 128 * 128)
        );
        assert_eq!(
            block_sum_sse(&b, 128, &a, 128, 128, 128),
            (255 * 128 * 128, 255 * 255 * 128 * 128)
        );
    }

    #[test]
    fn me_sad_zero_and_max() {
        let a = [0u8; 128 * 8];
        let b = [255u8; 128 * 8];
        assert_eq!(block_sad(&a, 128, &a, 128, 128, 8), 0);
        assert_eq!(block_sad(&a, 128, &b, 128, 128, 8), 255 * 128 * 8);
    }
}
