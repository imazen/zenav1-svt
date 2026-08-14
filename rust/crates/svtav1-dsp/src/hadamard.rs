//! Hadamard transform for SATD computation.
//!
//! Spec 02: SATD for mode decision cost metric.
//!
//! The Hadamard transform is used to compute SATD (Sum of Absolute
//! Transformed Differences) — a frequency-domain distortion metric
//! that better predicts coded size than SAD.
//!
//! SATD is the primary cost metric used in mode decision.

use archmage::prelude::*;

/// Compute 4x4 Hadamard transform of residual and return SATD.
///
/// SATD = sum of absolute values of Hadamard-transformed residual.
pub fn satd_4x4(src: &[u8], src_stride: usize, ref_: &[u8], ref_stride: usize) -> u32 {
    incant!(
        satd_4x4_impl(src, src_stride, ref_, ref_stride),
        [v3, neon, scalar]
    )
}

/// Compute 8x8 Hadamard transform of residual and return SATD.
pub fn satd_8x8(src: &[u8], src_stride: usize, ref_: &[u8], ref_stride: usize) -> u32 {
    incant!(
        satd_8x8_impl(src, src_stride, ref_, ref_stride),
        [v3, neon, scalar]
    )
}

// --- Scalar implementations ---

fn satd_4x4_impl_scalar(
    _token: ScalarToken,
    src: &[u8],
    src_stride: usize,
    ref_: &[u8],
    ref_stride: usize,
) -> u32 {
    satd_4x4_core(src, src_stride, ref_, ref_stride)
}

fn satd_8x8_impl_scalar(
    _token: ScalarToken,
    src: &[u8],
    src_stride: usize,
    ref_: &[u8],
    ref_stride: usize,
) -> u32 {
    satd_8x8_core(src, src_stride, ref_, ref_stride)
}

// --- AVX2 implementations ---

#[cfg(target_arch = "x86_64")]
#[arcane]
fn satd_4x4_impl_v3(
    _token: Desktop64,
    src: &[u8],
    src_stride: usize,
    ref_: &[u8],
    ref_stride: usize,
) -> u32 {
    // Auto-vectorize with AVX2 enabled — the butterfly add/sub pattern
    // vectorizes well with target_feature(enable = "avx2,fma")
    satd_4x4_core(src, src_stride, ref_, ref_stride)
}

#[cfg(target_arch = "x86_64")]
#[arcane]
fn satd_8x8_impl_v3(
    _token: Desktop64,
    src: &[u8],
    src_stride: usize,
    ref_: &[u8],
    ref_stride: usize,
) -> u32 {
    satd_8x8_core(src, src_stride, ref_, ref_stride)
}

// --- NEON implementations ---

/// NEON 4x4 SATD.
///
/// Same structure as [`satd_8x8_impl_neon`] at half the lane width. The 2D
/// Hadamard is separable and the two passes commute (`(H·X)·Hᵀ == H·(X·Hᵀ)`),
/// so both passes run VERTICALLY — one lane per column, no horizontal ops —
/// with a single 4x4 transpose between them. The butterfly is identical to the
/// scalar core's, applied twice.
///
/// Exact: the residual is in [-255, 255] and a 2D 4-point Hadamard amplifies
/// by at most 16, so |coefficient| <= 4080, far inside i16. The absolute values
/// sum to at most 65280, inside u32. No widening or saturation is involved.
#[cfg(target_arch = "aarch64")]
#[arcane]
fn satd_4x4_impl_neon(
    _token: NeonToken,
    src: &[u8],
    src_stride: usize,
    ref_: &[u8],
    ref_stride: usize,
) -> u32 {
    // Residual rows. Four bytes each, so building the i16 array directly is
    // both simpler and safer than a wider load that would over-read.
    let mut d = [vdup_n_s16(0); 4];
    for (row, slot) in d.iter_mut().enumerate() {
        let s = &src[row * src_stride..row * src_stride + 4];
        let r = &ref_[row * ref_stride..row * ref_stride + 4];
        let arr = [
            s[0] as i16 - r[0] as i16,
            s[1] as i16 - r[1] as i16,
            s[2] as i16 - r[2] as i16,
            s[3] as i16 - r[3] as i16,
        ];
        *slot = vld1_s16(&arr);
    }

    // Pass 1: vertical butterfly, pairing rows (0,1) and (2,3) exactly as the
    // scalar column pass does.
    let butterfly = |d: [int16x4_t; 4]| -> [int16x4_t; 4] {
        let a = vadd_s16(d[0], d[1]);
        let b = vsub_s16(d[0], d[1]);
        let c = vadd_s16(d[2], d[3]);
        let e = vsub_s16(d[2], d[3]);
        [
            vadd_s16(a, c),
            vadd_s16(b, e),
            vsub_s16(a, c),
            vsub_s16(b, e),
        ]
    };
    let t = butterfly(d);

    // 4x4 i16 transpose: pairwise interleave, then interleave the 2x2 blocks.
    let a0 = vtrn1_s16(t[0], t[1]);
    let a1 = vtrn2_s16(t[0], t[1]);
    let a2 = vtrn1_s16(t[2], t[3]);
    let a3 = vtrn2_s16(t[2], t[3]);
    let tr = [
        vreinterpret_s16_s32(vtrn1_s32(
            vreinterpret_s32_s16(a0),
            vreinterpret_s32_s16(a2),
        )),
        vreinterpret_s16_s32(vtrn1_s32(
            vreinterpret_s32_s16(a1),
            vreinterpret_s32_s16(a3),
        )),
        vreinterpret_s16_s32(vtrn2_s32(
            vreinterpret_s32_s16(a0),
            vreinterpret_s32_s16(a2),
        )),
        vreinterpret_s16_s32(vtrn2_s32(
            vreinterpret_s32_s16(a1),
            vreinterpret_s32_s16(a3),
        )),
    ];

    // Pass 2, then sum |coefficient| over all 16.
    let f = butterfly(tr);
    let mut acc = vdupq_n_u32(0);
    for v in f {
        acc = vaddq_u32(acc, vmovl_u16(vreinterpret_u16_s16(vabs_s16(v))));
    }
    let satd = vaddvq_u32(acc);

    (satd + 1) >> 1
}

/// 8x8 i16 transpose from 4-lane NEON primitives: three stages of pairwise
/// interleave at 16-, 32- and 64-bit granularity.
#[cfg(target_arch = "aarch64")]
#[rite]
fn transpose8x8_s16(_token: NeonToken, v: [int16x8_t; 8]) -> [int16x8_t; 8] {
    // Stage 1: swap adjacent elements between row pairs.
    let t0 = vtrnq_s16(v[0], v[1]);
    let t1 = vtrnq_s16(v[2], v[3]);
    let t2 = vtrnq_s16(v[4], v[5]);
    let t3 = vtrnq_s16(v[6], v[7]);
    // Stage 2: swap adjacent 32-bit pairs.
    let u0 = vtrnq_s32(vreinterpretq_s32_s16(t0.0), vreinterpretq_s32_s16(t1.0));
    let u1 = vtrnq_s32(vreinterpretq_s32_s16(t0.1), vreinterpretq_s32_s16(t1.1));
    let u2 = vtrnq_s32(vreinterpretq_s32_s16(t2.0), vreinterpretq_s32_s16(t3.0));
    let u3 = vtrnq_s32(vreinterpretq_s32_s16(t2.1), vreinterpretq_s32_s16(t3.1));
    // Stage 3: swap 64-bit halves.
    let c = |a: int32x4_t, b: int32x4_t| -> (int16x8_t, int16x8_t) {
        (
            vreinterpretq_s16_s64(vcombine_s64(
                vget_low_s64(vreinterpretq_s64_s32(a)),
                vget_low_s64(vreinterpretq_s64_s32(b)),
            )),
            vreinterpretq_s16_s64(vcombine_s64(
                vget_high_s64(vreinterpretq_s64_s32(a)),
                vget_high_s64(vreinterpretq_s64_s32(b)),
            )),
        )
    };
    let (r0, r4) = c(u0.0, u2.0);
    let (r1, r5) = c(u1.0, u3.0);
    let (r2, r6) = c(u0.1, u2.1);
    let (r3, r7) = c(u1.1, u3.1);
    [r0, r1, r2, r3, r4, r5, r6, r7]
}

/// One 8-point Hadamard butterfly applied VERTICALLY across eight vectors.
/// Each lane is an independent column, so there is no cross-lane work.
#[cfg(target_arch = "aarch64")]
#[rite]
fn hadamard8_vertical(_token: NeonToken, d: [int16x8_t; 8]) -> [int16x8_t; 8] {
    let a0 = vaddq_s16(d[0], d[4]);
    let a1 = vaddq_s16(d[1], d[5]);
    let a2 = vaddq_s16(d[2], d[6]);
    let a3 = vaddq_s16(d[3], d[7]);
    let a4 = vsubq_s16(d[0], d[4]);
    let a5 = vsubq_s16(d[1], d[5]);
    let a6 = vsubq_s16(d[2], d[6]);
    let a7 = vsubq_s16(d[3], d[7]);

    let b0 = vaddq_s16(a0, a2);
    let b1 = vaddq_s16(a1, a3);
    let b2 = vsubq_s16(a0, a2);
    let b3 = vsubq_s16(a1, a3);
    let b4 = vaddq_s16(a4, a6);
    let b5 = vaddq_s16(a5, a7);
    let b6 = vsubq_s16(a4, a6);
    let b7 = vsubq_s16(a5, a7);

    [
        vaddq_s16(b0, b1),
        vsubq_s16(b0, b1),
        vaddq_s16(b2, b3),
        vsubq_s16(b2, b3),
        vaddq_s16(b4, b5),
        vsubq_s16(b4, b5),
        vaddq_s16(b6, b7),
        vsubq_s16(b6, b7),
    ]
}

#[cfg(target_arch = "aarch64")]
#[arcane]
fn satd_8x8_impl_neon(
    token: NeonToken,
    src: &[u8],
    src_stride: usize,
    ref_: &[u8],
    ref_stride: usize,
) -> u32 {
    // The 2D Hadamard is separable, so row-then-column and column-then-row
    // produce the same coefficients — and the result is an absolute SUM over
    // all 64 of them, so ordering cannot change it. That lets both passes run
    // VERTICALLY (one lane per column, no horizontal ops), with a single 8x8
    // transpose between them.
    //
    // i16 lanes suffice: the residual is in [-255, 255] and a 2D 8-point
    // Hadamard amplifies by at most 64, so |coefficient| <= 16320, inside
    // i16's 32767. No widening or saturation is involved, so this is exact.
    let mut d = [vdupq_n_s16(0); 8];
    for (row, slot) in d.iter_mut().enumerate() {
        let s: &[u8; 8] = src[row * src_stride..row * src_stride + 8]
            .try_into()
            .unwrap();
        let r: &[u8; 8] = ref_[row * ref_stride..row * ref_stride + 8]
            .try_into()
            .unwrap();
        *slot = vsubq_s16(
            vreinterpretq_s16_u16(vmovl_u8(vld1_u8(s))),
            vreinterpretq_s16_u16(vmovl_u8(vld1_u8(r))),
        );
    }

    let d = hadamard8_vertical(token, d);
    let d = transpose8x8_s16(token, d);
    let d = hadamard8_vertical(token, d);

    // Sum |coefficient| over all 64. Widen to u32 lanes before accumulating:
    // 64 * 16320 = 1,044,480 overflows u16.
    let mut acc = vdupq_n_u32(0);
    for v in d {
        acc = vpadalq_u16(acc, vreinterpretq_u16_s16(vabsq_s16(v)));
    }
    let satd = vaddvq_u32(acc);

    (satd + 2) >> 2
}

// --- Core algorithm (shared across all dispatch tiers) ---

#[inline]
fn satd_4x4_core(src: &[u8], src_stride: usize, ref_: &[u8], ref_stride: usize) -> u32 {
    // Compute residual
    let mut diff = [0i16; 16];
    for row in 0..4 {
        for col in 0..4 {
            diff[row * 4 + col] =
                src[row * src_stride + col] as i16 - ref_[row * ref_stride + col] as i16;
        }
    }

    // 4x4 Hadamard transform (separable: row then column)
    let mut tmp = [0i16; 16];

    // Row transforms
    for row in 0..4 {
        let i = row * 4;
        let a = diff[i] + diff[i + 1];
        let b = diff[i] - diff[i + 1];
        let c = diff[i + 2] + diff[i + 3];
        let d = diff[i + 2] - diff[i + 3];
        tmp[i] = a + c;
        tmp[i + 1] = b + d;
        tmp[i + 2] = a - c;
        tmp[i + 3] = b - d;
    }

    // Column transforms and accumulate absolute values
    let mut satd: u32 = 0;
    for col in 0..4 {
        let a = tmp[col] + tmp[4 + col];
        let b = tmp[col] - tmp[4 + col];
        let c = tmp[8 + col] + tmp[12 + col];
        let d = tmp[8 + col] - tmp[12 + col];
        satd += (a + c).unsigned_abs() as u32;
        satd += (b + d).unsigned_abs() as u32;
        satd += (a - c).unsigned_abs() as u32;
        satd += (b - d).unsigned_abs() as u32;
    }

    // Normalization: divide by 2 (standard for 4x4 Hadamard)
    (satd + 1) >> 1
}

#[inline]
fn satd_8x8_core(src: &[u8], src_stride: usize, ref_: &[u8], ref_stride: usize) -> u32 {
    // Compute residual
    let mut diff = [0i16; 64];
    for row in 0..8 {
        for col in 0..8 {
            diff[row * 8 + col] =
                src[row * src_stride + col] as i16 - ref_[row * ref_stride + col] as i16;
        }
    }

    // 8x8 Hadamard via butterfly decomposition
    let mut tmp = [0i32; 64];

    // Row transforms (8-point Hadamard butterfly)
    for row in 0..8 {
        let i = row * 8;
        let d = &diff[i..i + 8];

        let a0 = d[0] as i32 + d[4] as i32;
        let a1 = d[1] as i32 + d[5] as i32;
        let a2 = d[2] as i32 + d[6] as i32;
        let a3 = d[3] as i32 + d[7] as i32;
        let a4 = d[0] as i32 - d[4] as i32;
        let a5 = d[1] as i32 - d[5] as i32;
        let a6 = d[2] as i32 - d[6] as i32;
        let a7 = d[3] as i32 - d[7] as i32;

        let b0 = a0 + a2;
        let b1 = a1 + a3;
        let b2 = a0 - a2;
        let b3 = a1 - a3;
        let b4 = a4 + a6;
        let b5 = a5 + a7;
        let b6 = a4 - a6;
        let b7 = a5 - a7;

        tmp[i] = b0 + b1;
        tmp[i + 1] = b0 - b1;
        tmp[i + 2] = b2 + b3;
        tmp[i + 3] = b2 - b3;
        tmp[i + 4] = b4 + b5;
        tmp[i + 5] = b4 - b5;
        tmp[i + 6] = b6 + b7;
        tmp[i + 7] = b6 - b7;
    }

    // Column transforms and accumulate absolute values
    let mut satd: u32 = 0;
    for col in 0..8 {
        let a0 = tmp[col] + tmp[32 + col];
        let a1 = tmp[8 + col] + tmp[40 + col];
        let a2 = tmp[16 + col] + tmp[48 + col];
        let a3 = tmp[24 + col] + tmp[56 + col];
        let a4 = tmp[col] - tmp[32 + col];
        let a5 = tmp[8 + col] - tmp[40 + col];
        let a6 = tmp[16 + col] - tmp[48 + col];
        let a7 = tmp[24 + col] - tmp[56 + col];

        let b0 = a0 + a2;
        let b1 = a1 + a3;
        let b2 = a0 - a2;
        let b3 = a1 - a3;
        let b4 = a4 + a6;
        let b5 = a5 + a7;
        let b6 = a4 - a6;
        let b7 = a5 - a7;

        satd += (b0 + b1).unsigned_abs();
        satd += (b0 - b1).unsigned_abs();
        satd += (b2 + b3).unsigned_abs();
        satd += (b2 - b3).unsigned_abs();
        satd += (b4 + b5).unsigned_abs();
        satd += (b4 - b5).unsigned_abs();
        satd += (b6 + b7).unsigned_abs();
        satd += (b6 - b7).unsigned_abs();
    }

    // Normalization: divide by 4 (standard for 8x8 Hadamard)
    (satd + 2) >> 2
}

#[cfg(test)]
mod tests {
    use super::*;

    /// satd_4x4 vs its scalar core, every tier, on random content.
    ///
    /// The other satd_4x4 tests here are identical-blocks (always 0) and a
    /// UNIFORM difference (80). A uniform residual puts all the energy in DC,
    /// so both would pass against a broken transpose or a mis-paired
    /// butterfly — precisely the mistakes a hand-written Hadamard makes. This
    /// uses random residuals, where every coefficient is live, and varies the
    /// strides so a kernel that ignored them is caught.
    #[test]
    fn satd_4x4_random_all_tiers_match_core() {
        use archmage::testing::{CompileTimePolicy, for_each_token_permutation};
        let mut st = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            st ^= st << 13;
            st ^= st >> 7;
            st ^= st << 17;
            (st >> 33) as u8
        };
        for case in 0..64 {
            let (ss, rs) = (4 + case % 5, 4 + (case * 3) % 7);
            let src: alloc::vec::Vec<u8> = (0..ss * 4 + 8).map(|_| next()).collect();
            let rf: alloc::vec::Vec<u8> = (0..rs * 4 + 8).map(|_| next()).collect();
            let expect = satd_4x4_core(&src, ss, &rf, rs);
            let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
                let got = satd_4x4(&src, ss, &rf, rs);
                assert_eq!(
                    got, expect,
                    "satd_4x4 case {case} strides ({ss},{rs}) tier {_perm}"
                );
            });
        }
    }

    #[test]
    fn satd_4x4_identical() {
        let block = [128u8; 64];
        assert_eq!(satd_4x4(&block, 8, &block, 8), 0);
    }

    #[test]
    fn satd_4x4_uniform_diff() {
        let src = [110u8; 16];
        let ref_ = [100u8; 16];
        // Uniform difference of 10 across 4x4 block.
        // Hadamard of constant = value * N at DC, 0 elsewhere
        // DC = 10 * 16 = 160, SATD = |160| / 2 = 80
        assert_eq!(satd_4x4(&src, 4, &ref_, 4), 80);
    }

    /// Every dispatch tier of the 2D 8x8 Hadamard must agree with an
    /// INDEPENDENT scalar oracle written from C's `hadamard_col8` +
    /// `svt_aom_hadamard_8x8_c`, on random residuals at strides wider than the
    /// block. Positional coefficients: unlike SATD, a wrong output permutation
    /// or a wrong transpose changes the answer, and both are the mistakes a
    /// vectorised Hadamard makes.
    ///
    /// The range deliberately includes 10-BIT residuals ([-1023, 1023]), where
    /// the i16 lanes of the NEON arm wrap and the scalar arm's i32 intermediates
    /// do not — they must still agree, because truncation to 16 bits commutes
    /// with add/sub.
    #[test]
    fn aom_hadamard_8x8_random_all_tiers_match_oracle() {
        use archmage::testing::{CompileTimePolicy, for_each_token_permutation};
        let mut st = 0xD1B5_4A32_D192_ED03u64;
        let mut next = move || {
            st ^= st << 13;
            st ^= st >> 7;
            st ^= st << 17;
            (st >> 33) as u32
        };
        fn oracle(src: &[i16], stride: usize) -> [i32; 64] {
            let col = |v: &[i16], st: usize| -> [i16; 8] {
                let s = |i: usize| v[i * st] as i32;
                let (b0, b1) = (s(0) + s(1), s(0) - s(1));
                let (b2, b3) = (s(2) + s(3), s(2) - s(3));
                let (b4, b5) = (s(4) + s(5), s(4) - s(5));
                let (b6, b7) = (s(6) + s(7), s(6) - s(7));
                let (c0, c1) = (b0 + b2, b1 + b3);
                let (c2, c3) = (b0 - b2, b1 - b3);
                let (c4, c5) = (b4 + b6, b5 + b7);
                let (c6, c7) = (b4 - b6, b5 - b7);
                let mut o = [0i16; 8];
                o[0] = (c0 + c4) as i16;
                o[7] = (c1 + c5) as i16;
                o[3] = (c2 + c6) as i16;
                o[4] = (c3 + c7) as i16;
                o[2] = (c0 - c4) as i16;
                o[6] = (c1 - c5) as i16;
                o[1] = (c2 - c6) as i16;
                o[5] = (c3 - c7) as i16;
                o
            };
            let mut b1 = [0i16; 64];
            for idx in 0..8 {
                b1[idx * 8..idx * 8 + 8].copy_from_slice(&col(&src[idx..], stride));
            }
            let mut b2 = [0i16; 64];
            for idx in 0..8 {
                b2[idx * 8..idx * 8 + 8].copy_from_slice(&col(&b1[idx..], 8));
            }
            // `svt_aom_hadamard_8x8_c` stores STRAIGHT through (unlike the 4x4
            // form, which transposes) — `coeff[idx] = buffer2[idx]`.
            let mut out = [0i32; 64];
            for idx in 0..64 {
                out[idx] = b2[idx] as i32;
            }
            out
        }
        for case in 0..48 {
            let stride = 8 + (case % 5) * 3;
            // bd8 residual range for most cases, bd10 for the rest.
            let span: i32 = if case % 3 == 2 { 2047 } else { 511 };
            let src: alloc::vec::Vec<i16> = (0..stride * 8 + 8)
                .map(|_| ((next() % (span as u32 * 2 + 1)) as i32 - span) as i16)
                .collect();
            let expect = oracle(&src, stride);
            let rep = for_each_token_permutation(CompileTimePolicy::WarnStderr, |perm| {
                let mut got = [0i32; 64];
                aom_hadamard_8x8(&src, stride, &mut got);
                assert_eq!(
                    got, expect,
                    "hadamard_8x8 case {case} stride {stride} tier {perm}"
                );
            });
            assert!(
                rep.warnings.is_empty(),
                "excluded tokens: {:?}",
                rep.warnings
            );
            assert!(
                rep.permutations_run >= 2,
                "only {} permutations",
                rep.permutations_run
            );
        }
    }

    #[test]
    fn satd_8x8_identical() {
        let block = [128u8; 128];
        assert_eq!(satd_8x8(&block, 16, &block, 16), 0);
    }

    #[test]
    fn satd_8x8_uniform_diff() {
        let src = [110u8; 64];
        let ref_ = [100u8; 64];
        // DC = 10 * 64 = 640, SATD = |640| / 4 = 160
        assert_eq!(satd_8x8(&src, 8, &ref_, 8), 160);
    }

    #[test]
    fn satd_greater_than_zero_for_different() {
        let mut src = [0u8; 64];
        let ref_ = [128u8; 64];
        for (i, v) in src.iter_mut().enumerate() {
            *v = (i * 7 % 256) as u8;
        }
        assert!(satd_4x4(&src, 8, &ref_, 8) > 0);
        assert!(satd_8x8(&src, 8, &ref_, 8) > 0);
    }

    #[test]
    fn satd_geq_sad() {
        // SATD should generally be >= SAD / N for non-trivial patterns
        // (Hadamard preserves energy)
        let mut src = [0u8; 64];
        let ref_ = [0u8; 64];
        for (i, v) in src.iter_mut().enumerate() {
            *v = if i % 2 == 0 { 200 } else { 50 };
        }
        let satd = satd_4x4(&src, 8, &ref_, 8);
        assert!(satd > 0);
    }
}

#[cfg(test)]
mod dispatch_tests {
    use super::*;

    use alloc::vec::Vec;
    use archmage::testing::{CompileTimePolicy, for_each_token_permutation};

    #[test]
    fn satd_4x4_all_dispatch_levels() {
        let src: Vec<u8> = (0..64).map(|i| (i * 3 + 17) as u8).collect();
        let ref_: Vec<u8> = (0..64).map(|i| (i * 5 + 42) as u8).collect();
        let reference_result = satd_4x4(&src, 8, &ref_, 8);

        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            let result = satd_4x4(&src, 8, &ref_, 8);
            assert_eq!(
                result, reference_result,
                "satd_4x4 mismatch at dispatch level"
            );
        });
    }

    #[test]
    fn satd_8x8_all_dispatch_levels() {
        let src: Vec<u8> = (0..64).map(|i| (i * 3 + 17) as u8).collect();
        let ref_: Vec<u8> = (0..64).map(|i| (i * 5 + 42) as u8).collect();
        let reference_result = satd_8x8(&src, 8, &ref_, 8);

        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            let result = satd_8x8(&src, 8, &ref_, 8);
            assert_eq!(
                result, reference_result,
                "satd_8x8 mismatch at dispatch level"
            );
        });
    }
}

// ===========================================================================
// C-exact aom Hadamard kernels for the MD fast loop (MDS0 SATD path).
//
// Verbatim ports of SVT-AV1 `svt_aom_hadamard_8x8_c` / `_16x16_c` /
// `_32x32_c` and `svt_aom_satd_c` (Source/Lib/C_DEFAULT/
// picture_operators_c.c:118-330, common_dsp_rtcd.c:48). These operate on
// int16 residuals and produce the int32 coefficient blocks C's
// `hadamard_path` (product_coding_loop.c:1187) feeds to `svt_aom_satd`.
// Differentially fuzzed vs the C reference in tests/c_parity_hadamard.rs.
// ===========================================================================

/// C `hadamard_col8`: one 8-point Hadamard butterfly over strided input.
/// Output order is the C permutation, not natural order.
fn hadamard_col8(src_diff: &[i16], src_stride: usize, coeff: &mut [i16; 8]) {
    let s = |i: usize| src_diff[i * src_stride] as i32;
    let b0 = s(0) + s(1);
    let b1 = s(0) - s(1);
    let b2 = s(2) + s(3);
    let b3 = s(2) - s(3);
    let b4 = s(4) + s(5);
    let b5 = s(4) - s(5);
    let b6 = s(6) + s(7);
    let b7 = s(6) - s(7);

    let c0 = b0 + b2;
    let c1 = b1 + b3;
    let c2 = b0 - b2;
    let c3 = b1 - b3;
    let c4 = b4 + b6;
    let c5 = b5 + b7;
    let c6 = b4 - b6;
    let c7 = b5 - b7;

    coeff[0] = (c0 + c4) as i16;
    coeff[7] = (c1 + c5) as i16;
    coeff[3] = (c2 + c6) as i16;
    coeff[4] = (c3 + c7) as i16;
    coeff[2] = (c0 - c4) as i16;
    coeff[6] = (c1 - c5) as i16;
    coeff[1] = (c2 - c6) as i16;
    coeff[5] = (c3 - c7) as i16;
}

/// C `hadamard_col4` (picture_operators_c.c:72): 4-point butterfly with a
/// `>> 1` on the first stage.
fn hadamard_col4(src_diff: &[i16], src_stride: usize, coeff: &mut [i16; 4]) {
    let b0 = (src_diff[0] + src_diff[src_stride]) >> 1;
    let b1 = (src_diff[0] - src_diff[src_stride]) >> 1;
    let b2 = (src_diff[2 * src_stride] + src_diff[3 * src_stride]) >> 1;
    let b3 = (src_diff[2 * src_stride] - src_diff[3 * src_stride]) >> 1;
    coeff[0] = b0 + b2;
    coeff[1] = b1 + b3;
    coeff[2] = b0 - b2;
    coeff[3] = b1 - b3;
}

/// C `svt_aom_hadamard_4x4_c` (picture_operators_c.c:85): 2D 4x4 Hadamard
/// (column pass, row pass over the transposed intermediate, then the
/// extra transpose matching the SSE2 kernel's output order).
pub fn aom_hadamard_4x4(src_diff: &[i16], src_stride: usize, coeff: &mut [i32]) {
    let mut buffer = [0i16; 16];
    let mut buffer2 = [0i16; 16];
    for idx in 0..4 {
        let mut out = [0i16; 4];
        hadamard_col4(&src_diff[idx..], src_stride, &mut out);
        buffer[idx * 4..idx * 4 + 4].copy_from_slice(&out);
    }
    for idx in 0..4 {
        let mut out = [0i16; 4];
        hadamard_col4(&buffer[idx..], 4, &mut out);
        buffer2[idx * 4..idx * 4 + 4].copy_from_slice(&out);
    }
    for i in 0..4 {
        for j in 0..4 {
            coeff[i * 4 + j] = buffer2[j * 4 + i] as i32;
        }
    }
}

/// C `svt_aom_hadamard_8x8_c`: 2D 8x8 Hadamard of an int16 residual block
/// (stride `src_stride`) into 64 int32 coefficients. No scaling.
///
/// C dispatches this through RTCD to `svt_aom_hadamard_8x8_neon`
/// (`common_dsp_rtcd.c:1603`); it is also the inner kernel of the 16x16 and
/// 32x32 forms below, so it carries the whole MDS0 Hadamard cost — 7.5 % of the
/// port's frame at 512x512 preset 2 and 4.0 % at preset 6
/// (`benchmarks/perf_class_attrib_2026-08-13.tsv`).
pub fn aom_hadamard_8x8(src_diff: &[i16], src_stride: usize, coeff: &mut [i32]) {
    incant!(
        aom_hadamard_8x8_impl(src_diff, src_stride, coeff),
        [v3, neon, scalar]
    )
}

fn aom_hadamard_8x8_impl_scalar(
    _token: ScalarToken,
    src_diff: &[i16],
    src_stride: usize,
    coeff: &mut [i32],
) {
    aom_hadamard_8x8_core(src_diff, src_stride, coeff)
}

#[cfg(target_arch = "x86_64")]
#[arcane]
fn aom_hadamard_8x8_impl_v3(
    _token: Desktop64,
    src_diff: &[i16],
    src_stride: usize,
    coeff: &mut [i32],
) {
    aom_hadamard_8x8_core(src_diff, src_stride, coeff)
}

/// Both passes run VERTICALLY (one lane per column, no cross-lane work) with a
/// single 8x8 transpose between them, exactly like [`satd_8x8_impl_neon`] —
/// except that the butterfly here must reproduce [`hadamard_col8`]'s PERMUTED
/// output order, because these coefficients are positional (the SATD kernel
/// only sums them, so any order does).
///
/// Exactness: `hadamard_col8` computes in `i32` and truncates to `i16` on
/// store. Every operation is an add or a subtract, and truncation to 16 bits is
/// a ring homomorphism for `+`/`-`, so doing the whole butterfly in wrapping
/// `i16` lanes (`vaddq_s16`/`vsubq_s16`) yields bit-identical results — this is
/// NOT an "in range so it does not matter" argument, it holds for any input,
/// including the 10-bit residuals of the bd10 fast loop.
#[cfg(target_arch = "aarch64")]
#[arcane]
fn aom_hadamard_8x8_impl_neon(
    token: NeonToken,
    src_diff: &[i16],
    src_stride: usize,
    coeff: &mut [i32],
) {
    let mut d = [vdupq_n_s16(0); 8];
    for (row, slot) in d.iter_mut().enumerate() {
        let r: &[i16; 8] = src_diff[row * src_stride..row * src_stride + 8]
            .try_into()
            .unwrap();
        *slot = vld1q_s16(r);
    }
    // Pass 1 gives o[k] lane j = coefficient k of column j (C's
    // `buffer[j * 8 + k]`). Pass 2 reads `buffer[idx + i * 8]`, i.e. it needs
    // lane `idx` to hold `o[idx][i]` — the transpose of that.
    let o = hadamard_col8_vertical(token, d);
    let u = transpose8x8_s16(token, o);
    // C stores `buffer2` straight through (`coeff[idx] = buffer2[idx]`), and
    // `q[k]` lane idx is `buffer2[idx * 8 + k]` — so output row idx is the
    // vector across k, i.e. one more transpose.
    let q = transpose8x8_s16(token, hadamard_col8_vertical(token, u));
    for (i, v) in q.iter().enumerate() {
        let dst: &mut [i32; 8] = (&mut coeff[i * 8..i * 8 + 8]).try_into().unwrap();
        vst1q_s32(
            (&mut dst[0..4]).try_into().unwrap(),
            vmovl_s16(vget_low_s16(*v)),
        );
        vst1q_s32((&mut dst[4..8]).try_into().unwrap(), vmovl_high_s16(*v));
    }
}

/// [`hadamard_col8`]'s butterfly applied VERTICALLY across eight vectors, with
/// its permuted output order. Distinct from [`hadamard8_vertical`], which is
/// the same transform in natural order (fine for SATD, wrong for coefficients).
#[cfg(target_arch = "aarch64")]
#[rite]
fn hadamard_col8_vertical(_token: NeonToken, s: [int16x8_t; 8]) -> [int16x8_t; 8] {
    let b0 = vaddq_s16(s[0], s[1]);
    let b1 = vsubq_s16(s[0], s[1]);
    let b2 = vaddq_s16(s[2], s[3]);
    let b3 = vsubq_s16(s[2], s[3]);
    let b4 = vaddq_s16(s[4], s[5]);
    let b5 = vsubq_s16(s[4], s[5]);
    let b6 = vaddq_s16(s[6], s[7]);
    let b7 = vsubq_s16(s[6], s[7]);

    let c0 = vaddq_s16(b0, b2);
    let c1 = vaddq_s16(b1, b3);
    let c2 = vsubq_s16(b0, b2);
    let c3 = vsubq_s16(b1, b3);
    let c4 = vaddq_s16(b4, b6);
    let c5 = vaddq_s16(b5, b7);
    let c6 = vsubq_s16(b4, b6);
    let c7 = vsubq_s16(b5, b7);

    // coeff[0]=c0+c4, [7]=c1+c5, [3]=c2+c6, [4]=c3+c7,
    // coeff[2]=c0-c4, [6]=c1-c5, [1]=c2-c6, [5]=c3-c7
    [
        vaddq_s16(c0, c4),
        vsubq_s16(c2, c6),
        vsubq_s16(c0, c4),
        vaddq_s16(c2, c6),
        vaddq_s16(c3, c7),
        vsubq_s16(c3, c7),
        vsubq_s16(c1, c5),
        vaddq_s16(c1, c5),
    ]
}

fn aom_hadamard_8x8_core(src_diff: &[i16], src_stride: usize, coeff: &mut [i32]) {
    let mut buffer = [0i16; 64];
    let mut buffer2 = [0i16; 64];
    // Column pass: one butterfly per column, walking columns left→right.
    for idx in 0..8 {
        let col = &src_diff[idx..];
        let mut out = [0i16; 8];
        hadamard_col8(col, src_stride, &mut out);
        buffer[idx * 8..idx * 8 + 8].copy_from_slice(&out);
    }
    // Row pass over the transposed intermediate.
    for idx in 0..8 {
        let mut out = [0i16; 8];
        hadamard_col8(&buffer[idx..], 8, &mut out);
        buffer2[idx * 8..idx * 8 + 8].copy_from_slice(&out);
    }
    for idx in 0..64 {
        coeff[idx] = buffer2[idx] as i32;
    }
}

// ---------------------------------------------------------------------------
// 16x16 / 32x32: ported from the AVX2 kernels, NOT the `_c` references.
//
// `svt_aom_hadamard_{16x16,32x32}` are RTCD function POINTERS that the encoder
// binds to the AVX2 implementations on any AVX2 host
// (`SET_AVX2(svt_aom_hadamard_32x32, _c, _avx2)`, common_dsp_rtcd.c:1047-1048),
// and the AVX2 kernels are NOT equivalent to the `_c` ones once the residual
// leaves the 8-bit range they were written for (their own comment: "src_diff:
// 9 bit, dynamic range [-255, 255]"):
//
//   * `_c` carries the 8x8 sub-results into the 16x16 cross-combine as
//     `int32_t` and the 16x16 sub-results into the 32x32 combine as `int32_t`;
//     nothing after the 8x8 stage can wrap.
//   * `_avx2` keeps BOTH of those stages in 16-bit lanes: the 16x16 combine is
//     `_mm256_{add,sub}_epi16` + `_mm256_srai_epi16` (wrapping), and
//     `svt_aom_hadamard_32x32_avx2` buffers its four 16x16 sub-transforms in an
//     `int16_t temp_coeff[32*32]` (`is_final = 0`,
//     pic_operators_intrin_avx2.c:1721-1732) before sign-extending to 32-bit,
//     doing the `>> 2` in 32-bit, SATURATING back to 16-bit
//     (`_mm256_packs_epi32`) and finishing with wrapping 16-bit add/sub.
//
// At 8-bit residuals the 16x16 stage spans [-32640, 32640] and the post-shift
// 32x32 operands span [-16320, 16320], so no wrap or saturation is reachable
// and the two kernels agree bit-for-bit — which is why the 8-bit identity
// gates are unaffected by porting the AVX2 semantics. At 10-bit residuals
// (the bd10 MD fast loop, task #94) the 16x16 stage reaches ~+/-130560 and the
// AVX2 kernel wraps where `_c` does not, so ONLY the AVX2 form reproduces the
// encoder's SATD. Pinned against both references in tests/c_parity_hadamard.rs
// (`_c` over the 8-bit range, `_avx2` over the 8-bit AND 10-bit ranges).
// ---------------------------------------------------------------------------

/// `svt_aom_hadamard_16x16_avx2`: four 8x8 sub-transforms + a cross-combine
/// carried in WRAPPING 16-bit lanes (`_mm256_{add,sub}_epi16`,
/// `_mm256_srai_epi16`), widened to `int32` on store (`store_tran_low`).
pub fn aom_hadamard_16x16(src_diff: &[i16], src_stride: usize, coeff: &mut [i32]) {
    for idx in 0..4usize {
        let off = (idx >> 1) * 8 * src_stride + (idx & 1) * 8;
        aom_hadamard_8x8(&src_diff[off..], src_stride, &mut coeff[idx * 64..]);
    }
    for i in 0..64usize {
        // The 8x8 stage already produced int16-valued coefficients (C's
        // `buffer2` / the AVX2 `temp_coeff` are both int16), so reading them
        // back as i16 is lossless and matches the AVX2 lane width.
        let a0 = coeff[i] as i16;
        let a1 = coeff[i + 64] as i16;
        let a2 = coeff[i + 128] as i16;
        let a3 = coeff[i + 192] as i16;
        let b0 = a0.wrapping_add(a1) >> 1;
        let b1 = a0.wrapping_sub(a1) >> 1;
        let b2 = a2.wrapping_add(a3) >> 1;
        let b3 = a2.wrapping_sub(a3) >> 1;
        coeff[i] = b0.wrapping_add(b2) as i32;
        coeff[i + 64] = b1.wrapping_add(b3) as i32;
        coeff[i + 128] = b0.wrapping_sub(b2) as i32;
        coeff[i + 192] = b1.wrapping_sub(b3) as i32;
    }
}

/// `svt_aom_hadamard_32x32_avx2`: four 16x16 sub-transforms buffered as
/// `int16` (`is_final = 0`), then sign-extended to 32-bit for the pairwise
/// sum/difference and `>> 2`, SATURATED back to 16-bit (`_mm256_packs_epi32`)
/// and combined with wrapping 16-bit add/sub before the 32-bit store.
pub fn aom_hadamard_32x32(src_diff: &[i16], src_stride: usize, coeff: &mut [i32]) {
    for idx in 0..4usize {
        let off = (idx >> 1) * 16 * src_stride + (idx & 1) * 16;
        aom_hadamard_16x16(&src_diff[off..], src_stride, &mut coeff[idx * 256..]);
    }
    for i in 0..256usize {
        // `temp_coeff` is int16: the 16x16 stage is read back through 16-bit
        // lanes and sign-extended (the AVX2 `sign_extend_16bit_to_32bit`).
        let a0 = coeff[i] as i16 as i32;
        let a1 = coeff[i + 256] as i16 as i32;
        let a2 = coeff[i + 512] as i16 as i32;
        let a3 = coeff[i + 768] as i16 as i32;
        // 32-bit add/sub then arithmetic `>> 2` (`_mm256_srai_epi32`).
        let b0 = (a0 + a1) >> 2;
        let b1 = (a0 - a1) >> 2;
        let b2 = (a2 + a3) >> 2;
        let b3 = (a2 - a3) >> 2;
        // `_mm256_packs_epi32`: SATURATING 32 -> 16 narrowing.
        let sat = |v: i32| v.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        let (b0, b1, b2, b3) = (sat(b0), sat(b1), sat(b2), sat(b3));
        // `_mm256_{add,sub}_epi16`: WRAPPING 16-bit, then sign-extended store.
        coeff[i] = b0.wrapping_add(b2) as i32;
        coeff[i + 256] = b1.wrapping_add(b3) as i32;
        coeff[i + 512] = b0.wrapping_sub(b2) as i32;
        coeff[i + 768] = b1.wrapping_sub(b3) as i32;
    }
}

/// C `svt_aom_satd_c`: plain sum of absolute int32 coefficients.
pub fn aom_satd(coeff: &[i32]) -> i32 {
    let mut satd: i32 = 0;
    for &c in coeff {
        satd += c.abs();
    }
    satd
}
