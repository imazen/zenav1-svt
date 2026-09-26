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
//! # Which tiers use `#[magetypes]`
//!
//! **CORRECTED 2026-09-05 — the old reason expired, and a NEW one replaces it.**
//! The paragraph here used to say magetypes exposes no integer widening, no
//! `abs_diff` and no pairwise-widening accumulate. Since archmage PR #96 (the PR
//! that closes the issue this port filed) it exposes all three, plus a FUSED
//! `u8xN::sum_abs_diff`. The x86 and dotprod arms stay handwritten for a
//! measured reason: **`sum_abs_diff` fully REDUCES per call.** Its backends are
//! `_mm256_sad_epu8` followed by `_mm_add_epi64` + `_mm_srli_si128` +
//! `_mm_cvtsi128_si64` (`magetypes/src/simd/impls/x86_v3.rs:4815`) and
//! `vaddlvq_u8(vabdq_u8(..))` (`impls/arm_neon.rs:4527`) — a CROSS-LANE
//! reduction every 16 or 32 bytes. `block_sad_v3` and `block_sad_arm_v2`
//! keep the whole block in `psadbw` / `vdotq_u32` lanes and reduce ONCE, which
//! the fused API cannot express; the missing primitive is an ACCUMULATING form
//! (`sad_accumulate(rhs, acc) -> u64xN`). Reported on archmage#96 with the
//! backend sources; see `benchmarks/sse_madd_2026-09-05.meta` §6.
//! **This is an instruction-sequence argument from the generated backends, NOT a
//! wall-clock measurement** — the generic body was not built and benched, so if
//! you want to overturn it, bench it rather than re-reading this paragraph.
//! (What DID get benched is the sibling case: in `variance::sse` the generic
//! `abs_diff` + `madd_adjacent` body is 1.45x-2.20x SLOWER than the hand NEON
//! arm on an M4 Pro, so "the primitives exist now" is not by itself a reason to
//! collapse an arm.)
//!
//! The baseline NEON SAD and sum/SSE arms now use the unsigned
//! `pairwise_widen_add` API released in magetypes 0.9.29. This retains
//! lane accumulation and the final reduction; historical M4 Pro measurements
//! found identical ME instruction bodies and no meaningful speed change.
//! See `benchmarks/arm_pairwise_2026-09-07.meta`.
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
#[magetypes(define(u8x16, u16x8, u32x4), neon, -scalar)]
pub fn block_sad(
    token: Token,
    src: &[u8],
    src_stride: usize,
    rf: &[u8],
    ref_stride: usize,
    w: usize,
    h: usize,
) -> u32 {
    let mut acc = u32x4::splat(token, 0);
    let mut tail = 0u32;
    for y in 0..h {
        let so = y * src_stride;
        let ro = y * ref_stride;
        let mut c = 0usize;
        let mut racc = u16x8::splat(token, 0);
        while c + 16 <= w {
            let a: &[u8; 16] = src[so + c..so + c + 16].try_into().unwrap();
            let b: &[u8; 16] = rf[ro + c..ro + c + 16].try_into().unwrap();
            racc += u8x16::load(token, a)
                .abs_diff(u8x16::load(token, b))
                .pairwise_widen_add();
            c += 16;
        }
        if c + 8 <= w {
            let a: &[u8; 8] = src[so + c..so + c + 8].try_into().unwrap();
            let b: &[u8; 8] = rf[ro + c..ro + c + 8].try_into().unwrap();
            let d = vabd_u8(vld1_u8(a), vld1_u8(b));
            racc += u16x8::from_repr(token, vmovl_u8(d));
            c += 8;
        }
        acc += racc.pairwise_widen_add();
        while c < w {
            tail += u32::from(src[so + c].abs_diff(rf[ro + c]));
            c += 1;
        }
    }
    acc.reduce_add() + tail
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

// --- x86-64 AVX-512 ---
//
// `_mm512_sad_epu8` sums each contiguous 8-byte group into its own u64 lane.
// The block total is the sum of all lanes regardless of how rows are packed
// into the register, so the narrow widths below pack MULTIPLE ROWS per vector
// — the same reordering `block_sad_x4_v3` proves bit-identical for w == 4 and
// w == 8. The 64-byte rung covers the >= 64-wide calls (HME base level,
// `full_pel_search`, `md_search` whole blocks); nothing here changes the sum.

/// AVX-512 block SAD. Lane accumulators are u64 — each `_mm512_sad_epu8` lane
/// adds at most `8 * 255 = 2040` per op, so no h can overflow them.
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
#[arcane]
pub fn block_sad_v4(
    _token: X64V4Token,
    src: &[u8],
    src_stride: usize,
    rf: &[u8],
    ref_stride: usize,
    w: usize,
    h: usize,
) -> u32 {
    let mut acc512 = _mm512_setzero_si512();
    let mut acc256 = _mm256_setzero_si256();
    let mut acc128 = _mm_setzero_si128();
    let mut tail = 0u32;

    // w == 8: the most-called ME shape (every `ext_*` block SAD is 8x4/8x8).
    // Four rows per 256-bit register, `block_sad_x4_v3`'s proven packing.
    if w == 8 {
        let ld = |p: &[u8], o: usize| i64::from_le_bytes(p[o..o + 8].try_into().unwrap());
        let mut y = 0;
        while y + 4 <= h {
            let a = _mm256_setr_epi64x(
                ld(src, y * src_stride),
                ld(src, (y + 1) * src_stride),
                ld(src, (y + 2) * src_stride),
                ld(src, (y + 3) * src_stride),
            );
            let b = _mm256_setr_epi64x(
                ld(rf, y * ref_stride),
                ld(rf, (y + 1) * ref_stride),
                ld(rf, (y + 2) * ref_stride),
                ld(rf, (y + 3) * ref_stride),
            );
            acc256 = _mm256_add_epi64(acc256, _mm256_sad_epu8(a, b));
            y += 4;
        }
        while y < h {
            for x in 0..w {
                tail += u32::from(src[y * src_stride + x].abs_diff(rf[y * ref_stride + x]));
            }
            y += 1;
        }
    } else if w == 16 {
        // Four rows per 512-bit register: 4 xmm loads + 3 inserts replace
        // 4 x (2 loads + sad + add) of the straight xmm ladder.
        let mut y = 0;
        while y + 4 <= h {
            let la = |r: usize| -> __m128i {
                let s: &[u8; 16] = src[r * src_stride..r * src_stride + 16].try_into().unwrap();
                _mm_loadu_si128(s)
            };
            let lb = |r: usize| -> __m128i {
                let s: &[u8; 16] = rf[r * ref_stride..r * ref_stride + 16].try_into().unwrap();
                _mm_loadu_si128(s)
            };
            let a = _mm512_inserti32x4::<3>(
                _mm512_inserti32x4::<2>(
                    _mm512_inserti32x4::<1>(_mm512_castsi128_si512(la(y)), la(y + 1)),
                    la(y + 2),
                ),
                la(y + 3),
            );
            let b = _mm512_inserti32x4::<3>(
                _mm512_inserti32x4::<2>(
                    _mm512_inserti32x4::<1>(_mm512_castsi128_si512(lb(y)), lb(y + 1)),
                    lb(y + 2),
                ),
                lb(y + 3),
            );
            acc512 = _mm512_add_epi64(acc512, _mm512_sad_epu8(a, b));
            y += 4;
        }
        while y < h {
            let a: &[u8; 16] = src[y * src_stride..y * src_stride + 16].try_into().unwrap();
            let b: &[u8; 16] = rf[y * ref_stride..y * ref_stride + 16].try_into().unwrap();
            acc128 = _mm_add_epi64(acc128, _mm_sad_epu8(_mm_loadu_si128(a), _mm_loadu_si128(b)));
            y += 1;
        }
    } else if w == 32 {
        // Two rows per 512-bit register.
        let mut y = 0;
        while y + 2 <= h {
            let a0: &[u8; 32] = src[y * src_stride..y * src_stride + 32].try_into().unwrap();
            let a1: &[u8; 32] = src[(y + 1) * src_stride..(y + 1) * src_stride + 32]
                .try_into()
                .unwrap();
            let a = _mm512_inserti64x4::<1>(
                _mm512_castsi256_si512(_mm256_loadu_si256(a0)),
                _mm256_loadu_si256(a1),
            );
            let b0: &[u8; 32] = rf[y * ref_stride..y * ref_stride + 32].try_into().unwrap();
            let b1: &[u8; 32] = rf[(y + 1) * ref_stride..(y + 1) * ref_stride + 32]
                .try_into()
                .unwrap();
            let b = _mm512_inserti64x4::<1>(
                _mm512_castsi256_si512(_mm256_loadu_si256(b0)),
                _mm256_loadu_si256(b1),
            );
            acc512 = _mm512_add_epi64(acc512, _mm512_sad_epu8(a, b));
            y += 2;
        }
        if y < h {
            let a: &[u8; 32] = src[y * src_stride..y * src_stride + 32].try_into().unwrap();
            let b: &[u8; 32] = rf[y * ref_stride..y * ref_stride + 32].try_into().unwrap();
            acc256 = _mm256_add_epi64(
                acc256,
                _mm256_sad_epu8(_mm256_loadu_si256(a), _mm256_loadu_si256(b)),
            );
        }
    } else {
        // General width ladder: 64-byte chunks, then the same 32/16/8
        // sub-rungs and scalar tail as `block_sad_v3`.
        for y in 0..h {
            let so = y * src_stride;
            let ro = y * ref_stride;
            let mut c = 0usize;
            while c + 64 <= w {
                let a: &[u8; 64] = src[so + c..so + c + 64].try_into().unwrap();
                let b: &[u8; 64] = rf[ro + c..ro + c + 64].try_into().unwrap();
                acc512 = _mm512_add_epi64(
                    acc512,
                    _mm512_sad_epu8(_mm512_loadu_si512(a), _mm512_loadu_si512(b)),
                );
                c += 64;
            }
            while c + 32 <= w {
                let a: &[u8; 32] = src[so + c..so + c + 32].try_into().unwrap();
                let b: &[u8; 32] = rf[ro + c..ro + c + 32].try_into().unwrap();
                acc256 = _mm256_add_epi64(
                    acc256,
                    _mm256_sad_epu8(_mm256_loadu_si256(a), _mm256_loadu_si256(b)),
                );
                c += 32;
            }
            while c + 16 <= w {
                let a: &[u8; 16] = src[so + c..so + c + 16].try_into().unwrap();
                let b: &[u8; 16] = rf[ro + c..ro + c + 16].try_into().unwrap();
                acc128 =
                    _mm_add_epi64(acc128, _mm_sad_epu8(_mm_loadu_si128(a), _mm_loadu_si128(b)));
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
    }

    let s = _mm_add_epi64(
        acc128,
        _mm_add_epi64(
            _mm256_castsi256_si128(acc256),
            _mm256_extracti128_si256::<1>(acc256),
        ),
    );
    let s = _mm_add_epi64(s, _mm_srli_si128::<8>(s));
    (_mm512_reduce_add_epi64(acc512) as u32) + (_mm_cvtsi128_si64(s) as u64 as u32) + tail
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
#[magetypes(define(u8x16, u16x8, u32x4), neon, -scalar)]
pub fn block_sum_sse(
    token: Token,
    a: &[u8],
    a_stride: usize,
    b: &[u8],
    b_stride: usize,
    w: usize,
    h: usize,
) -> (i32, u32) {
    let mut acc_a = u32x4::splat(token, 0);
    let mut acc_b = u32x4::splat(token, 0);
    let mut acc_sse = u32x4::splat(token, 0);
    let mut tail_sum: i32 = 0;
    let mut tail_sse: u32 = 0;
    for y in 0..h {
        let ao = y * a_stride;
        let bo = y * b_stride;
        let mut c = 0usize;
        let mut ra = u16x8::splat(token, 0);
        let mut rb = u16x8::splat(token, 0);
        while c + 16 <= w {
            let av: &[u8; 16] = a[ao + c..ao + c + 16].try_into().unwrap();
            let bv: &[u8; 16] = b[bo + c..bo + c + 16].try_into().unwrap();
            let va = u8x16::load(token, av);
            let vb = u8x16::load(token, bv);
            ra += va.pairwise_widen_add();
            rb += vb.pairwise_widen_add();
            let d = va.abs_diff(vb);
            // |d| <= 255 so d*d <= 65025, inside u16; the widening pairwise
            // accumulate then drains into u32.
            let lo = d.widen_low();
            let hi = d.widen_high();
            acc_sse += (lo * lo).pairwise_widen_add();
            acc_sse += (hi * hi).pairwise_widen_add();
            c += 16;
        }
        if c + 8 <= w {
            let av: &[u8; 8] = a[ao + c..ao + c + 8].try_into().unwrap();
            let bv: &[u8; 8] = b[bo + c..bo + c + 8].try_into().unwrap();
            let va = vld1_u8(av);
            let vb = vld1_u8(bv);
            ra += u16x8::from_repr(token, vmovl_u8(va));
            rb += u16x8::from_repr(token, vmovl_u8(vb));
            let d = vabd_u8(va, vb);
            acc_sse += u16x8::from_repr(token, vmull_u8(d, d)).pairwise_widen_add();
            c += 8;
        }
        acc_a += ra.pairwise_widen_add();
        acc_b += rb.pairwise_widen_add();
        while c < w {
            let d = i32::from(a[ao + c]) - i32::from(b[bo + c]);
            tail_sum += d;
            tail_sse += (d * d) as u32;
            c += 1;
        }
    }
    let sum = (acc_a.reduce_add() as i32) - (acc_b.reduce_add() as i32) + tail_sum;
    (sum, acc_sse.reduce_add() + tail_sse)
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

/// AVX-512 `block_sum_sse`: the same widen-to-i16 + `_mm512_madd_epi16`
/// pipeline as `_v3`, on 32-byte chunks, with the 16/8 rungs kept at 256-bit.
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
#[arcane]
pub fn block_sum_sse_v4(
    _token: X64V4Token,
    a: &[u8],
    a_stride: usize,
    b: &[u8],
    b_stride: usize,
    w: usize,
    h: usize,
) -> (i32, u32) {
    let mut acc_a = _mm512_setzero_si512();
    let mut acc_b = _mm512_setzero_si512();
    let mut acc_sse = _mm512_setzero_si512();
    let mut acc_a256 = _mm256_setzero_si256();
    let mut acc_b256 = _mm256_setzero_si256();
    let mut acc_sse256 = _mm256_setzero_si256();
    let mut tail_sum: i32 = 0;
    let mut tail_sse: u32 = 0;
    let ones = _mm512_set1_epi16(1);
    let ones256 = _mm256_set1_epi16(1);
    for y in 0..h {
        let ao = y * a_stride;
        let bo = y * b_stride;
        let mut c = 0usize;
        while c + 32 <= w {
            let av: &[u8; 32] = a[ao + c..ao + c + 32].try_into().unwrap();
            let bv: &[u8; 32] = b[bo + c..bo + c + 32].try_into().unwrap();
            let va = _mm512_cvtepu8_epi16(_mm256_loadu_si256(av));
            let vb = _mm512_cvtepu8_epi16(_mm256_loadu_si256(bv));
            acc_a = _mm512_add_epi32(acc_a, _mm512_madd_epi16(va, ones));
            acc_b = _mm512_add_epi32(acc_b, _mm512_madd_epi16(vb, ones));
            let d = _mm512_sub_epi16(va, vb);
            acc_sse = _mm512_add_epi32(acc_sse, _mm512_madd_epi16(d, d));
            c += 32;
        }
        while c + 16 <= w {
            let av: &[u8; 16] = a[ao + c..ao + c + 16].try_into().unwrap();
            let bv: &[u8; 16] = b[bo + c..bo + c + 16].try_into().unwrap();
            let va = _mm256_cvtepu8_epi16(_mm_loadu_si128(av));
            let vb = _mm256_cvtepu8_epi16(_mm_loadu_si128(bv));
            acc_a256 = _mm256_add_epi32(acc_a256, _mm256_madd_epi16(va, ones256));
            acc_b256 = _mm256_add_epi32(acc_b256, _mm256_madd_epi16(vb, ones256));
            let d = _mm256_sub_epi16(va, vb);
            acc_sse256 = _mm256_add_epi32(acc_sse256, _mm256_madd_epi16(d, d));
            c += 16;
        }
        if c + 8 <= w {
            let av: &[u8; 8] = a[ao + c..ao + c + 8].try_into().unwrap();
            let bv: &[u8; 8] = b[bo + c..bo + c + 8].try_into().unwrap();
            let va = _mm256_cvtepu8_epi16(_mm_loadu_si64(av));
            let vb = _mm256_cvtepu8_epi16(_mm_loadu_si64(bv));
            acc_a256 = _mm256_add_epi32(acc_a256, _mm256_madd_epi16(va, ones256));
            acc_b256 = _mm256_add_epi32(acc_b256, _mm256_madd_epi16(vb, ones256));
            let d = _mm256_sub_epi16(va, vb);
            acc_sse256 = _mm256_add_epi32(acc_sse256, _mm256_madd_epi16(d, d));
            c += 8;
        }
        while c < w {
            let d = i32::from(a[ao + c]) - i32::from(b[bo + c]);
            tail_sum += d;
            tail_sse += (d * d) as u32;
            c += 1;
        }
    }
    let red256 = |v: __m256i| -> i32 {
        let s = _mm_add_epi32(_mm256_castsi256_si128(v), _mm256_extracti128_si256::<1>(v));
        let s = _mm_add_epi32(s, _mm_shuffle_epi32::<0b01_00_11_10>(s));
        let s = _mm_add_epi32(s, _mm_shuffle_epi32::<0b00_01_00_01>(s));
        _mm_cvtsi128_si32(s)
    };
    let sum = (_mm512_reduce_add_epi32(acc_a) + red256(acc_a256))
        - (_mm512_reduce_add_epi32(acc_b) + red256(acc_b256))
        + tail_sum;
    let sse = (_mm512_reduce_add_epi32(acc_sse) as u32)
        .wrapping_add(red256(acc_sse256) as u32)
        .wrapping_add(tail_sse);
    (sum, sse)
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
        [v4, v3, neon, scalar]
    )
}

// --- one-shot dispatching entry point ---

/// Dispatching block SAD, for the call sites that are NOT inside a search
/// loop. Inside a loop, summon once and call the `_arm_v2` / `_neon` / `_v3` /
/// `_v4` / `_scalar` helper directly.
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
        [v4, arm_v2, v3, neon, scalar]
    )
}

/// Four independent SADs with a shared source, in caller-supplied order.
/// Mirrors C's `sdx4df` contract used by IntraBC mesh search.
pub fn block_sad_x4(
    src: &[u8],
    src_stride: usize,
    refs: [&[u8]; 4],
    ref_stride: usize,
    w: usize,
    h: usize,
) -> [u32; 4] {
    incant!(
        block_sad_x4(src, src_stride, refs, ref_stride, w, h),
        [v4, v3, neon, scalar]
    )
}

pub fn block_sad_x4_scalar(
    _token: ScalarToken,
    src: &[u8],
    src_stride: usize,
    refs: [&[u8]; 4],
    ref_stride: usize,
    w: usize,
    h: usize,
) -> [u32; 4] {
    core::array::from_fn(|i| block_sad(src, src_stride, refs[i], ref_stride, w, h))
}

/// C `sadwxhx4d_neon` (ASM_NEON/sad_x4d_neon.c:27): each source row is loaded
/// once and differenced against all four references, keeping four independent
/// lane accumulators. SAD is an exact integer sum, so lane/fold order cannot
/// change the result.
///
/// Wide path (`w >= 16`): `vabdq_u8` + `vpadalq_u8` accumulate into u16 lanes
/// (<=510 per lane per 16-chunk), folded into u32 every `2048/w` rows —
/// C's bound: `floor(w/16) * 510 * fold_rows <= 510 * 128 = 65280 < 65536`
/// for every `w >= 16`. Sub-16 column tails accumulate scalarly into u32, so
/// they add nothing to the u16 lane bound.
///
/// Narrow path (`w < 16`): per-row `vabd_u8` + `vaddw_u8`/`vmovl_u8` into u16
/// lanes (<=255 per lane per row), folded every 128 rows — safe for any `h`,
/// unlike C's kernel which relies on `h <= 32` for w<16.
#[cfg(target_arch = "aarch64")]
#[arcane]
pub fn block_sad_x4_neon(
    _token: NeonToken,
    src: &[u8],
    src_stride: usize,
    refs: [&[u8]; 4],
    ref_stride: usize,
    w: usize,
    h: usize,
) -> [u32; 4] {
    if w >= 16 {
        let fold_rows = (2048 / w).max(1);
        let mut a = [vdupq_n_u32(0); 4];
        let mut b = [vdupq_n_u16(0); 4];
        let mut tail = [0u32; 4];
        let mut since = 0usize;
        for y in 0..h {
            let so = y * src_stride;
            let ro = y * ref_stride;
            let mut x = 0usize;
            while x + 16 <= w {
                let s = vld1q_u8(src[so + x..so + x + 16].try_into().unwrap());
                for i in 0..4 {
                    let r = vld1q_u8(refs[i][ro + x..ro + x + 16].try_into().unwrap());
                    b[i] = vpadalq_u8(b[i], vabdq_u8(s, r));
                }
                x += 16;
            }
            while x < w {
                for i in 0..4 {
                    tail[i] += u32::from(src[so + x].abs_diff(refs[i][ro + x]));
                }
                x += 1;
            }
            since += 1;
            if since == fold_rows {
                for i in 0..4 {
                    a[i] = vaddq_u32(a[i], vpaddlq_u16(b[i]));
                }
                b = [vdupq_n_u16(0); 4];
                since = 0;
            }
        }
        let mut out = tail;
        for i in 0..4 {
            a[i] = vaddq_u32(a[i], vpaddlq_u16(b[i]));
            out[i] += vaddvq_u32(a[i]);
        }
        out
    } else {
        // Narrow path: u16 lanes hold <=255 per row, fold every 128 rows.
        let mut a = [vdupq_n_u32(0); 4];
        let mut b = [vdupq_n_u16(0); 4];
        let mut tail = [0u32; 4];
        let mut since = 0usize;
        for y in 0..h {
            let so = y * src_stride;
            let ro = y * ref_stride;
            let mut x = 0usize;
            if x + 8 <= w {
                let s = vld1_u8(src[so..so + 8].try_into().unwrap());
                for i in 0..4 {
                    let r = vld1_u8(refs[i][ro..ro + 8].try_into().unwrap());
                    b[i] = vaddw_u8(b[i], vabd_u8(s, r));
                }
                x += 8;
            }
            if x + 4 <= w {
                let s = vcreate_u8(u64::from(u32::from_le_bytes(
                    src[so + x..so + x + 4].try_into().unwrap(),
                )));
                for i in 0..4 {
                    let r = vcreate_u8(u64::from(u32::from_le_bytes(
                        refs[i][ro + x..ro + x + 4].try_into().unwrap(),
                    )));
                    b[i] = vaddw_u8(b[i], vabd_u8(s, r));
                }
                x += 4;
            }
            while x < w {
                for i in 0..4 {
                    tail[i] += u32::from(src[so + x].abs_diff(refs[i][ro + x]));
                }
                x += 1;
            }
            since += 1;
            if since == 128 {
                for i in 0..4 {
                    a[i] = vaddq_u32(a[i], vpaddlq_u16(b[i]));
                }
                b = [vdupq_n_u16(0); 4];
                since = 0;
            }
        }
        let mut out = tail;
        for i in 0..4 {
            a[i] = vaddq_u32(a[i], vpaddlq_u16(b[i]));
            out[i] += vaddvq_u32(a[i]);
        }
        out
    }
}

/// C's four-candidate SAD: reuse each source load across four references and
/// accumulate in independent lanes. Narrow loads never cross the block edge.
#[cfg(target_arch = "x86_64")]
#[arcane]
pub fn block_sad_x4_v3(
    _token: Desktop64,
    src: &[u8],
    src_stride: usize,
    refs: [&[u8]; 4],
    ref_stride: usize,
    w: usize,
    h: usize,
) -> [u32; 4] {
    let mut wide = [_mm256_setzero_si256(); 4];
    let mut narrow = [_mm_setzero_si128(); 4];
    let mut tail = [0u32; 4];

    // WIDTH-4 ROW PACKING. The generic row loop below issues one 4-BYTE
    // `_mm_sad_epu8` per row per reference -- a 128-bit register doing four
    // bytes of work -- and 4x4 blocks are among the most-called sizes in this
    // encoder (a C profile of the same cell ranks sad4x8x4d and sad4x16x4d in
    // its top SAD kernels, alongside the 8-wide ones).
    //
    // Four rows of 4 bytes are exactly one 16-byte vector, assembled with
    // `_mm_setr_epi32` from four direct loads -- no staging buffer. Per four
    // rows that is 4 `setr` + 4 `sad` + 4 `add` against the generic path's
    // 16 `cvtsi32` + 16 `sad` + 16 `add`.
    //
    // `_mm_sad_epu8` sums each 8-byte half into its own lane and the reduction
    // adds both, so packing rows only reorders an integer sum: bit-identical.
    if w == 4 {
        let ld = |p: &[u8], o: usize| i32::from_le_bytes(p[o..o + 4].try_into().unwrap());
        let mut y = 0;
        while y + 4 <= h {
            let (s0, s1, s2, s3) = (
                y * src_stride,
                (y + 1) * src_stride,
                (y + 2) * src_stride,
                (y + 3) * src_stride,
            );
            let a = _mm_setr_epi32(ld(src, s0), ld(src, s1), ld(src, s2), ld(src, s3));
            let (r0, r1, r2, r3) = (
                y * ref_stride,
                (y + 1) * ref_stride,
                (y + 2) * ref_stride,
                (y + 3) * ref_stride,
            );
            for i in 0..4 {
                let rp = refs[i];
                let b = _mm_setr_epi32(ld(rp, r0), ld(rp, r1), ld(rp, r2), ld(rp, r3));
                narrow[i] = _mm_add_epi64(narrow[i], _mm_sad_epu8(a, b));
            }
            y += 4;
        }
        while y < h {
            let (so, ro) = (y * src_stride, y * ref_stride);
            for x in 0..w {
                for i in 0..4 {
                    tail[i] += u32::from(src[so + x].abs_diff(refs[i][ro + x]));
                }
            }
            y += 1;
        }
        for i in 0..4 {
            let sw = _mm_add_epi64(narrow[i], _mm_srli_si128::<8>(narrow[i]));
            tail[i] += _mm_cvtsi128_si64(sw) as u32;
        }
        return tail;
    }

    // WIDTH-8 ROW PACKING, the same argument one size up: four rows of 8 bytes
    // fill one 256-bit vector, so a 8xN block costs one `_mm256_sad_epu8` per
    // four rows instead of four `_mm_sad_epu8`. C's sad8x8x4d / sad8x16x4d /
    // sad8x4x4d are among its most-called kernels on this content.
    if w == 8 {
        let ld = |p: &[u8], o: usize| i64::from_le_bytes(p[o..o + 8].try_into().unwrap());
        let mut y = 0;
        while y + 4 <= h {
            let a = _mm256_setr_epi64x(
                ld(src, y * src_stride),
                ld(src, (y + 1) * src_stride),
                ld(src, (y + 2) * src_stride),
                ld(src, (y + 3) * src_stride),
            );
            for i in 0..4 {
                let rp = refs[i];
                let b = _mm256_setr_epi64x(
                    ld(rp, y * ref_stride),
                    ld(rp, (y + 1) * ref_stride),
                    ld(rp, (y + 2) * ref_stride),
                    ld(rp, (y + 3) * ref_stride),
                );
                wide[i] = _mm256_add_epi64(wide[i], _mm256_sad_epu8(a, b));
            }
            y += 4;
        }
        while y < h {
            let (so, ro) = (y * src_stride, y * ref_stride);
            for x in 0..w {
                for i in 0..4 {
                    tail[i] += u32::from(src[so + x].abs_diff(refs[i][ro + x]));
                }
            }
            y += 1;
        }
        for i in 0..4 {
            let sw = _mm_add_epi64(
                _mm256_castsi256_si128(wide[i]),
                _mm256_extracti128_si256::<1>(wide[i]),
            );
            let sw = _mm_add_epi64(sw, _mm_srli_si128::<8>(sw));
            tail[i] += _mm_cvtsi128_si64(sw) as u32;
        }
        return tail;
    }

    for y in 0..h {
        let so = y * src_stride;
        let ro = y * ref_stride;
        let mut x = 0;
        while x + 32 <= w {
            let a = _mm256_loadu_si256::<[u8; 32]>(src[so + x..so + x + 32].try_into().unwrap());
            for i in 0..4 {
                let b = _mm256_loadu_si256::<[u8; 32]>(
                    refs[i][ro + x..ro + x + 32].try_into().unwrap(),
                );
                wide[i] = _mm256_add_epi64(wide[i], _mm256_sad_epu8(a, b));
            }
            x += 32;
        }
        if x + 16 <= w {
            let a = _mm_loadu_si128::<[u8; 16]>(src[so + x..so + x + 16].try_into().unwrap());
            for i in 0..4 {
                let b =
                    _mm_loadu_si128::<[u8; 16]>(refs[i][ro + x..ro + x + 16].try_into().unwrap());
                narrow[i] = _mm_add_epi64(narrow[i], _mm_sad_epu8(a, b));
            }
            x += 16;
        }
        if x + 8 <= w {
            let a = _mm_loadu_si64::<[u8; 8]>(src[so + x..so + x + 8].try_into().unwrap());
            for i in 0..4 {
                let b = _mm_loadu_si64::<[u8; 8]>(refs[i][ro + x..ro + x + 8].try_into().unwrap());
                narrow[i] = _mm_add_epi64(narrow[i], _mm_sad_epu8(a, b));
            }
            x += 8;
        }
        if x + 4 <= w {
            let a = _mm_cvtsi32_si128(i32::from_le_bytes(
                src[so + x..so + x + 4].try_into().unwrap(),
            ));
            for i in 0..4 {
                let b = _mm_cvtsi32_si128(i32::from_le_bytes(
                    refs[i][ro + x..ro + x + 4].try_into().unwrap(),
                ));
                narrow[i] = _mm_add_epi64(narrow[i], _mm_sad_epu8(a, b));
            }
            x += 4;
        }
        while x < w {
            for i in 0..4 {
                tail[i] += u32::from(src[so + x].abs_diff(refs[i][ro + x]));
            }
            x += 1;
        }
    }
    for i in 0..4 {
        let s = _mm_add_epi64(
            narrow[i],
            _mm_add_epi64(
                _mm256_castsi256_si128(wide[i]),
                _mm256_extracti128_si256::<1>(wide[i]),
            ),
        );
        let s = _mm_add_epi64(s, _mm_srli_si128::<8>(s));
        tail[i] += _mm_cvtsi128_si64(s) as u32;
    }
    tail
}

/// AVX-512 four-candidate SAD: same load-once/accumulate-four shape as
/// `block_sad_x4_v3` with a 64-byte chunk rung and 512-bit row packs for
/// w == 16 and w == 32. The w == 4 / w == 8 packs stay at 128/256-bit — the
/// GPR loads that feed a wider pack dominate, so a zmm pack is not a win
/// there. Sum order is again only a reordering of an integer total.
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
#[arcane]
pub fn block_sad_x4_v4(
    _token: X64V4Token,
    src: &[u8],
    src_stride: usize,
    refs: [&[u8]; 4],
    ref_stride: usize,
    w: usize,
    h: usize,
) -> [u32; 4] {
    let mut zmm = [_mm512_setzero_si512(); 4];
    let mut wide = [_mm256_setzero_si256(); 4];
    let mut narrow = [_mm_setzero_si128(); 4];
    let mut tail = [0u32; 4];

    if w == 4 {
        let ld = |p: &[u8], o: usize| i32::from_le_bytes(p[o..o + 4].try_into().unwrap());
        let mut y = 0;
        while y + 4 <= h {
            let (s0, s1, s2, s3) = (
                y * src_stride,
                (y + 1) * src_stride,
                (y + 2) * src_stride,
                (y + 3) * src_stride,
            );
            let a = _mm_setr_epi32(ld(src, s0), ld(src, s1), ld(src, s2), ld(src, s3));
            let (r0, r1, r2, r3) = (
                y * ref_stride,
                (y + 1) * ref_stride,
                (y + 2) * ref_stride,
                (y + 3) * ref_stride,
            );
            for i in 0..4 {
                let rp = refs[i];
                let b = _mm_setr_epi32(ld(rp, r0), ld(rp, r1), ld(rp, r2), ld(rp, r3));
                narrow[i] = _mm_add_epi64(narrow[i], _mm_sad_epu8(a, b));
            }
            y += 4;
        }
        while y < h {
            let (so, ro) = (y * src_stride, y * ref_stride);
            for x in 0..w {
                for i in 0..4 {
                    tail[i] += u32::from(src[so + x].abs_diff(refs[i][ro + x]));
                }
            }
            y += 1;
        }
        for i in 0..4 {
            let sw = _mm_add_epi64(narrow[i], _mm_srli_si128::<8>(narrow[i]));
            tail[i] += _mm_cvtsi128_si64(sw) as u32;
        }
        return tail;
    }

    if w == 8 {
        let ld = |p: &[u8], o: usize| i64::from_le_bytes(p[o..o + 8].try_into().unwrap());
        let mut y = 0;
        while y + 4 <= h {
            let a = _mm256_setr_epi64x(
                ld(src, y * src_stride),
                ld(src, (y + 1) * src_stride),
                ld(src, (y + 2) * src_stride),
                ld(src, (y + 3) * src_stride),
            );
            for i in 0..4 {
                let rp = refs[i];
                let b = _mm256_setr_epi64x(
                    ld(rp, y * ref_stride),
                    ld(rp, (y + 1) * ref_stride),
                    ld(rp, (y + 2) * ref_stride),
                    ld(rp, (y + 3) * ref_stride),
                );
                wide[i] = _mm256_add_epi64(wide[i], _mm256_sad_epu8(a, b));
            }
            y += 4;
        }
        while y < h {
            let (so, ro) = (y * src_stride, y * ref_stride);
            for x in 0..w {
                for i in 0..4 {
                    tail[i] += u32::from(src[so + x].abs_diff(refs[i][ro + x]));
                }
            }
            y += 1;
        }
        for i in 0..4 {
            let sw = _mm_add_epi64(
                _mm256_castsi256_si128(wide[i]),
                _mm256_extracti128_si256::<1>(wide[i]),
            );
            let sw = _mm_add_epi64(sw, _mm_srli_si128::<8>(sw));
            tail[i] += _mm_cvtsi128_si64(sw) as u32;
        }
        return tail;
    }

    if w == 16 {
        // Four rows per 512-bit register: 4 xmm loads + 3 inserts for the
        // source, the same per reference, then one `_mm512_sad_epu8`.
        let mut y = 0;
        while y + 4 <= h {
            let la = |p: &[u8], stride: usize, r: usize| -> __m128i {
                let s: &[u8; 16] = p[r * stride..r * stride + 16].try_into().unwrap();
                _mm_loadu_si128(s)
            };
            let pack = |p: &[u8], stride: usize, y: usize| -> __m512i {
                _mm512_inserti32x4::<3>(
                    _mm512_inserti32x4::<2>(
                        _mm512_inserti32x4::<1>(
                            _mm512_castsi128_si512(la(p, stride, y)),
                            la(p, stride, y + 1),
                        ),
                        la(p, stride, y + 2),
                    ),
                    la(p, stride, y + 3),
                )
            };
            let a = pack(src, src_stride, y);
            for i in 0..4 {
                let b = pack(refs[i], ref_stride, y);
                zmm[i] = _mm512_add_epi64(zmm[i], _mm512_sad_epu8(a, b));
            }
            y += 4;
        }
        while y < h {
            let (so, ro) = (y * src_stride, y * ref_stride);
            let a = _mm_loadu_si128::<[u8; 16]>(src[so..so + 16].try_into().unwrap());
            for i in 0..4 {
                let b = _mm_loadu_si128::<[u8; 16]>(refs[i][ro..ro + 16].try_into().unwrap());
                narrow[i] = _mm_add_epi64(narrow[i], _mm_sad_epu8(a, b));
            }
            y += 1;
        }
        for i in 0..4 {
            tail[i] += _mm512_reduce_add_epi64(zmm[i]) as u32;
            let sw = _mm_add_epi64(narrow[i], _mm_srli_si128::<8>(narrow[i]));
            tail[i] += _mm_cvtsi128_si64(sw) as u32;
        }
        return tail;
    }

    if w == 32 {
        // Two rows per 512-bit register.
        let mut y = 0;
        while y + 2 <= h {
            let la = |p: &[u8], stride: usize, r: usize| -> __m256i {
                let s: &[u8; 32] = p[r * stride..r * stride + 32].try_into().unwrap();
                _mm256_loadu_si256(s)
            };
            let pack = |p: &[u8], stride: usize, y: usize| -> __m512i {
                _mm512_inserti64x4::<1>(
                    _mm512_castsi256_si512(la(p, stride, y)),
                    la(p, stride, y + 1),
                )
            };
            let a = pack(src, src_stride, y);
            for i in 0..4 {
                let b = pack(refs[i], ref_stride, y);
                zmm[i] = _mm512_add_epi64(zmm[i], _mm512_sad_epu8(a, b));
            }
            y += 2;
        }
        while y < h {
            let (so, ro) = (y * src_stride, y * ref_stride);
            let a = _mm256_loadu_si256::<[u8; 32]>(src[so..so + 32].try_into().unwrap());
            for i in 0..4 {
                let b = _mm256_loadu_si256::<[u8; 32]>(refs[i][ro..ro + 32].try_into().unwrap());
                wide[i] = _mm256_add_epi64(wide[i], _mm256_sad_epu8(a, b));
            }
            y += 1;
        }
        for i in 0..4 {
            tail[i] += _mm512_reduce_add_epi64(zmm[i]) as u32;
            let sw = _mm_add_epi64(
                _mm256_castsi256_si128(wide[i]),
                _mm256_extracti128_si256::<1>(wide[i]),
            );
            let sw = _mm_add_epi64(sw, _mm_srli_si128::<8>(sw));
            tail[i] += _mm_cvtsi128_si64(sw) as u32;
        }
        return tail;
    }

    for y in 0..h {
        let so = y * src_stride;
        let ro = y * ref_stride;
        let mut x = 0;
        while x + 64 <= w {
            let a = _mm512_loadu_si512::<[u8; 64]>(src[so + x..so + x + 64].try_into().unwrap());
            for i in 0..4 {
                let b = _mm512_loadu_si512::<[u8; 64]>(
                    refs[i][ro + x..ro + x + 64].try_into().unwrap(),
                );
                zmm[i] = _mm512_add_epi64(zmm[i], _mm512_sad_epu8(a, b));
            }
            x += 64;
        }
        while x + 32 <= w {
            let a = _mm256_loadu_si256::<[u8; 32]>(src[so + x..so + x + 32].try_into().unwrap());
            for i in 0..4 {
                let b = _mm256_loadu_si256::<[u8; 32]>(
                    refs[i][ro + x..ro + x + 32].try_into().unwrap(),
                );
                wide[i] = _mm256_add_epi64(wide[i], _mm256_sad_epu8(a, b));
            }
            x += 32;
        }
        if x + 16 <= w {
            let a = _mm_loadu_si128::<[u8; 16]>(src[so + x..so + x + 16].try_into().unwrap());
            for i in 0..4 {
                let b =
                    _mm_loadu_si128::<[u8; 16]>(refs[i][ro + x..ro + x + 16].try_into().unwrap());
                narrow[i] = _mm_add_epi64(narrow[i], _mm_sad_epu8(a, b));
            }
            x += 16;
        }
        if x + 8 <= w {
            let a = _mm_loadu_si64::<[u8; 8]>(src[so + x..so + x + 8].try_into().unwrap());
            for i in 0..4 {
                let b = _mm_loadu_si64::<[u8; 8]>(refs[i][ro + x..ro + x + 8].try_into().unwrap());
                narrow[i] = _mm_add_epi64(narrow[i], _mm_sad_epu8(a, b));
            }
            x += 8;
        }
        if x + 4 <= w {
            let a = _mm_cvtsi32_si128(i32::from_le_bytes(
                src[so + x..so + x + 4].try_into().unwrap(),
            ));
            for i in 0..4 {
                let b = _mm_cvtsi32_si128(i32::from_le_bytes(
                    refs[i][ro + x..ro + x + 4].try_into().unwrap(),
                ));
                narrow[i] = _mm_add_epi64(narrow[i], _mm_sad_epu8(a, b));
            }
            x += 4;
        }
        while x < w {
            for i in 0..4 {
                tail[i] += u32::from(src[so + x].abs_diff(refs[i][ro + x]));
            }
            x += 1;
        }
    }
    for i in 0..4 {
        let s = _mm_add_epi64(
            narrow[i],
            _mm_add_epi64(
                _mm256_castsi256_si128(wide[i]),
                _mm256_extracti128_si256::<1>(wide[i]),
            ),
        );
        let s = _mm_add_epi64(s, _mm_srli_si128::<8>(s));
        tail[i] += (_mm_cvtsi128_si64(s) as u32) + (_mm512_reduce_add_epi64(zmm[i]) as u32);
    }
    tail
}

#[cfg(test)]
mod tests;
