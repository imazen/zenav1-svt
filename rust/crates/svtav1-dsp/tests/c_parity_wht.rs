//! Differential parity: the AV1 lossless (qindex 0) Walsh-Hadamard kernels vs
//! the C reference.
//!
//! * forward `svt_av1_fwht4x4_c` (transforms.c:3879),
//! * inverse `svt_av1_highbd_iwht4x4_16_add_c` (inv_transforms.c:2782),
//! * inverse `svt_av1_highbd_iwht4x4_1_add_c` (inv_transforms.c:2843).
//!
//! All three are exported symbols in `libSvtAv1Enc.a`, so every assertion here
//! drives the REAL C implementation — the strongest evidence tier this project
//! has. (The `static highbd_iwht4x4_add` selector at inv_transforms.c:2874 has
//! no exported symbol; it is a one-line `eob > 1` branch and is covered
//! indirectly by `selector_dispatches_by_eob_like_c` below.)
//!
//! These kernels are plain scalar code with no `incant!` dispatch, so no
//! `archmage::testing` token lock is needed (contrast c_parity_hadamard.rs).
//!
//! The inverse tests also transitively exercise `svtav1_dsp::hbd::
//! highbd_clip_pixel_add` / `check_range` / `clip_pixel_highbd` against C for
//! the first time — the coefficient range fuzzed here drives residuals well
//! past the bd8 `check_range` clamp of +/-34595, so that clamp is live.

use svtav1_cref as cref;
use svtav1_dsp::fwd_txfm::{UNIT_QUANT_SHIFT, fwht4x4};
use svtav1_dsp::inv_txfm::{highbd_iwht4x4_1_add, highbd_iwht4x4_16_add, highbd_iwht4x4_add};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    /// 8-bit residual range (`src - pred`, -255..255) with occasional
    /// extremes — what the lossless path actually feeds `fwht4x4`.
    fn residual8(&mut self) -> i16 {
        let v = (self.next() >> 40) as i16 % 256;
        if self.next() & 15 == 0 {
            if v & 1 == 0 { 255 } else { -255 }
        } else {
            v
        }
    }
    /// Full `int16_t` domain — the widest input the C signature admits.
    fn residual_full(&mut self) -> i16 {
        match self.next() % 16 {
            0 => i16::MIN,
            1 => i16::MAX,
            _ => self.next() as i16,
        }
    }
    /// Dequantized lossless coefficient. Real values are |c| <= ~3.3M (the
    /// forward's 4x-scaled output at qindex 0); 2^23 is a generous superset
    /// that still keeps every `int32_t` intermediate of the inverse below
    /// 12.5 * 2^22 (~52M), i.e. no overflow, so C's signed-overflow UB is
    /// never reached.
    fn coeff(&mut self) -> i32 {
        match self.next() % 32 {
            0 => 1 << 23,
            1 => -(1 << 23),
            2 => 0,
            _ => (self.next() as i32) % (1 << 23),
        }
    }
    /// The WHOLE `int32_t` domain, biased toward the saturation corners. Unlike
    /// [`Rng::coeff`] this DOES drive the inverse's intermediates past
    /// `int32_t`, which is where C has signed-overflow UB and the port has
    /// `wrapping_*` — see `inv_wht_matches_c_in_the_int32_wrap_regime`.
    fn coeff_wrapping(&mut self) -> i32 {
        match self.next() % 8 {
            0 => i32::MAX,
            1 => i32::MIN,
            2 => i32::MAX - (self.next() as i32 & 0xffff),
            3 => i32::MIN + (self.next() as i32 & 0xffff),
            _ => self.next() as i32,
        }
    }
    fn pixel(&mut self, bd: u8) -> u16 {
        let max = (1u32 << bd) - 1;
        (self.next() as u32 % (max + 1)) as u16
    }
}

// =============================================================================
// Forward: svt_av1_fwht4x4_c
// =============================================================================

fn check_fwd(input: &[i16], stride: usize) {
    let expect = cref::fwht4x4(input, stride);
    let mut got = [0i32; 16];
    fwht4x4(input, &mut got, stride);
    assert_eq!(
        got.to_vec(),
        expect,
        "fwht4x4 mismatch, stride {stride}, input {:?}",
        &input[..(3 * stride + 4).min(input.len())]
    );
}

#[test]
fn fwht4x4_matches_c_over_8bit_residuals() {
    let mut rng = Rng(0x5741_4c53_4800_0001);
    for _ in 0..4000 {
        // Random stride >= 4 exercises the strided column reads of pass 0.
        let stride = 4 + (rng.next() as usize % 4) * 3;
        let mut src = vec![0i16; 3 * stride + 4];
        for v in src.iter_mut() {
            *v = rng.residual8();
        }
        check_fwd(&src, stride);
    }
}

#[test]
fn fwht4x4_matches_c_over_full_i16_domain() {
    let mut rng = Rng(0x5741_4c53_4800_0002);
    for _ in 0..4000 {
        let stride = 4 + (rng.next() as usize % 4) * 3;
        let mut src = vec![0i16; 3 * stride + 4];
        for v in src.iter_mut() {
            *v = rng.residual_full();
        }
        check_fwd(&src, stride);
    }
}

/// Every one of the 2^16 saturated sign patterns (each of the 16 inputs is
/// `i16::MIN` or `i16::MAX`). These are the exact magnitude corners where an
/// `int32_t`-only implementation would overflow first, so this is the sweep
/// that matters for the `_c`-vs-`_sse4_1` question below.
#[test]
fn fwht4x4_matches_c_at_every_extremal_corner() {
    for mask in 0u32..(1 << 16) {
        let mut src = [0i16; 16];
        for (i, v) in src.iter_mut().enumerate() {
            *v = if mask & (1 << i) != 0 {
                i16::MAX
            } else {
                i16::MIN
            };
        }
        let expect = cref::fwht4x4(&src, 4);
        let mut got = [0i32; 16];
        fwht4x4(&src, &mut got, 4);
        assert_eq!(got.to_vec(), expect, "extremal corner mask {mask:#06x}");
    }
}

// -----------------------------------------------------------------------------
// The _c vs _sse4_1 question.
//
// `svt_av1_fwht4x4` is bound `SET_SSE41(_c, _sse4_1)` on x86 (aom_dsp_rtcd.c:319)
// and `SET_ONLY_C` on aarch64 / generic (:919, :1301) — so on an x86 host the C
// ENCODER runs the SSE4.1 kernel, not the one ported above. This project has a
// precedent where a `_c` function and its SIMD twin genuinely disagree
// (c_parity_hadamard.rs:236 — `svt_aom_hadamard_32x32_c` vs `_avx2` at bd10
// magnitudes), so "they're the same function" cannot be assumed.
//
// The two implementations differ in EXACTLY one respect: `_c` carries every
// intermediate in `int64_t` and casts to `int32_t` only at the stores, while
// `_sse4_1` works in `__m128i` 32-bit lanes throughout and scales with
// `_mm_slli_epi32(_, UNIT_QUANT_SHIFT)` instead of `* UNIT_QUANT_FACTOR`. They
// can therefore only disagree if some intermediate leaves the `int32_t` range.
//
// This host is aarch64, so `svt_av1_fwht4x4_sse4_1` is not in the library (`nm`
// on libSvtAv1Enc.a shows only `_c`) and cannot be called; Rosetta is absent so
// an x86 build cannot be run either. `fwht4x4_sse4_1_model` below is a
// line-by-line transcription of the SSE4.1 source, and the two tests that use
// it (a) pin the model against the REAL `_c` symbol over the same sweeps as
// above and (b) MEASURE the largest intermediate any `int16_t` input can
// produce. Evidence tier: (b) is a hard bound, (a) is model-vs-real-C.
// -----------------------------------------------------------------------------

/// Scalar transcription of `svt_av1_fwht4x4_sse4_1`
/// (ASM_SSE4_1/highbd_fwd_txfm_sse4.c:14835-14884). The four `__m128i`
/// registers hold rows; lane `k` of every register is column `k`, so the
/// vector body is this loop with `i32` (NOT `i64`) arithmetic. `wrapping_*`
/// models the modular wrap of `_mm_add_epi32` / `_mm_sub_epi32`.
fn fwht4x4_sse4_1_model(input: &[i16], stride: usize) -> [i32; 16] {
    // _mm_loadl_epi64 + _mm_cvtepi16_epi32: op[r] = row r, widened to i32.
    let mut op = [[0i32; 4]; 4];
    for (r, row) in op.iter_mut().enumerate() {
        for (c, v) in row.iter_mut().enumerate() {
            *v = i32::from(input[r * stride + c]);
        }
    }

    for pass in 0..2 {
        let mut next = [[0i32; 4]; 4];
        for lane in 0..4 {
            let mut a1 = op[0][lane];
            let mut b1 = op[1][lane];
            let mut c1 = op[2][lane];
            let mut d1 = op[3][lane];

            a1 = a1.wrapping_add(b1);
            d1 = d1.wrapping_sub(c1);
            let e1 = a1.wrapping_sub(d1) >> 1; // _mm_srai_epi32(_, 1)
            b1 = e1.wrapping_sub(b1);
            c1 = e1.wrapping_sub(c1);
            a1 = a1.wrapping_sub(c1);
            d1 = d1.wrapping_add(b1);

            next[0][lane] = a1;
            next[1][lane] = c1;
            next[2][lane] = d1;
            next[3][lane] = b1;
        }
        op = next;
        if pass == 0 {
            // transpose_32bit_4x4 (highbd_fwd_txfm_sse4.c:14807)
            let mut t = [[0i32; 4]; 4];
            for (r, row) in t.iter_mut().enumerate() {
                for (c, v) in row.iter_mut().enumerate() {
                    *v = op[c][r];
                }
            }
            op = t;
        }
    }

    let mut out = [0i32; 16];
    for (r, row) in op.iter().enumerate() {
        for (c, v) in row.iter().enumerate() {
            out[r * 4 + c] = v << UNIT_QUANT_SHIFT; // _mm_slli_epi32
        }
    }
    out
}

#[test]
fn fwht4x4_c_and_sse4_1_model_agree_over_full_i16_domain() {
    let mut rng = Rng(0x5741_4c53_4800_0003);
    let iters = 20_000;
    for _ in 0..iters {
        let stride = 4 + (rng.next() as usize % 4) * 3;
        let mut src = vec![0i16; 3 * stride + 4];
        for v in src.iter_mut() {
            *v = rng.residual_full();
        }
        let c = cref::fwht4x4(&src, stride);
        let sse = fwht4x4_sse4_1_model(&src, stride);
        assert_eq!(
            sse.to_vec(),
            c,
            "svt_av1_fwht4x4_c and the _sse4_1 model disagree at stride {stride}"
        );
    }
    // Plus every saturated corner, where an int32-only path would break first.
    for mask in 0u32..(1 << 16) {
        let mut src = [0i16; 16];
        for (i, v) in src.iter_mut().enumerate() {
            *v = if mask & (1 << i) != 0 {
                i16::MAX
            } else {
                i16::MIN
            };
        }
        assert_eq!(
            fwht4x4_sse4_1_model(&src, 4).to_vec(),
            cref::fwht4x4(&src, 4),
            "corner {mask:#06x}"
        );
    }
}

/// The reason the two agree: with `int16_t` inputs no intermediate can leave
/// the `int32_t` range, so the `_c` kernel's `int64_t` width is never load
/// bearing. This measures the actual bound instead of asserting it.
#[test]
fn fwht4x4_intermediates_never_leave_i32() {
    /// Mirror of the kernel that returns max |intermediate| INCLUDING the
    /// final `* UNIT_QUANT_FACTOR`.
    fn max_intermediate(input: &[i16], stride: usize) -> i64 {
        let mut peak = 0i64;
        let mut track = |v: i64| peak = peak.max(v.abs());
        let mut mid = [0i64; 16];
        for i in 0..4 {
            let (mut a1, mut b1, mut c1, mut d1) = (
                i64::from(input[i]),
                i64::from(input[stride + i]),
                i64::from(input[2 * stride + i]),
                i64::from(input[3 * stride + i]),
            );
            a1 += b1;
            d1 -= c1;
            let e1 = (a1 - d1) >> 1;
            track(a1 - d1);
            b1 = e1 - b1;
            c1 = e1 - c1;
            a1 -= c1;
            d1 += b1;
            for v in [a1, b1, c1, d1] {
                track(v);
            }
            mid[i * 4] = a1;
            mid[i * 4 + 1] = c1;
            mid[i * 4 + 2] = d1;
            mid[i * 4 + 3] = b1;
        }
        for i in 0..4 {
            let (mut a1, mut b1, mut c1, mut d1) = (mid[i], mid[4 + i], mid[8 + i], mid[12 + i]);
            a1 += b1;
            d1 -= c1;
            let e1 = (a1 - d1) >> 1;
            track(a1 - d1);
            b1 = e1 - b1;
            c1 = e1 - c1;
            a1 -= c1;
            d1 += b1;
            for v in [a1, b1, c1, d1] {
                track(v * 4);
            }
        }
        peak
    }

    let mut peak = 0i64;
    for mask in 0u32..(1 << 16) {
        let mut src = [0i16; 16];
        for (i, v) in src.iter_mut().enumerate() {
            *v = if mask & (1 << i) != 0 {
                i16::MAX
            } else {
                i16::MIN
            };
        }
        peak = peak.max(max_intermediate(&src, 4));
    }
    let mut rng = Rng(0x5741_4c53_4800_0004);
    for _ in 0..20_000 {
        let mut src = [0i16; 16];
        for v in src.iter_mut() {
            *v = rng.residual_full();
        }
        peak = peak.max(max_intermediate(&src, 4));
    }
    eprintln!("fwht4x4 peak |intermediate| over the swept inputs: {peak}");
    assert!(
        peak < i64::from(i32::MAX),
        "fwht4x4 intermediate {peak} exceeds int32 — the _c/_sse4_1 \
         equivalence argument would no longer hold"
    );
    // MEASURED 2026-08-03 on this sweep (65536 saturated corners + 20k random
    // full-i16 blocks): peak = 524288 = 2^19, i.e. 4096x below i32::MAX and
    // 4x below this tripwire. The loose analytic bound is 5 * 163839 * 4 <
    // 2^22. Fails loudly if a future change moves the peak toward the edge.
    assert!(peak < (1 << 21), "peak intermediate grew to {peak}");
}

// =============================================================================
// Inverse: svt_av1_highbd_iwht4x4_{16,1}_add_c
// =============================================================================

fn check_inv(coeffs: &[i32], base: &[u16], stride_r: usize, stride_w: usize, bd: u8, eob16: bool) {
    let expect = if eob16 {
        cref::highbd_iwht4x4_16_add(coeffs, base, stride_r, stride_w, bd)
    } else {
        cref::highbd_iwht4x4_1_add(coeffs, base, stride_r, stride_w, bd)
    };
    let mut got = vec![0u16; 3 * stride_w + 4];
    if eob16 {
        highbd_iwht4x4_16_add(coeffs, base, stride_r, &mut got, stride_w, bd);
    } else {
        highbd_iwht4x4_1_add(coeffs, base, stride_r, &mut got, stride_w, bd);
    }
    assert_eq!(
        got, expect,
        "iwht4x4 (16-coeff: {eob16}) mismatch at bd {bd}, strides {stride_r}/{stride_w}, \
         coeffs {coeffs:?}"
    );
}

fn fuzz_inv(bd: u8, seed: u64, eob16: bool) {
    let mut rng = Rng(seed);
    for _ in 0..3000 {
        let stride_r = 4 + (rng.next() as usize % 4) * 3;
        let stride_w = 4 + (rng.next() as usize % 4) * 3;
        let mut coeffs = [0i32; 16];
        for v in coeffs.iter_mut() {
            *v = rng.coeff();
        }
        let mut base = vec![0u16; 3 * stride_r + 4];
        for v in base.iter_mut() {
            *v = rng.pixel(bd);
        }
        check_inv(&coeffs, &base, stride_r, stride_w, bd, eob16);
    }
}

#[test]
fn highbd_iwht4x4_16_add_matches_c_bd8() {
    fuzz_inv(8, 0x5741_4c53_4800_0010, true);
}

#[test]
fn highbd_iwht4x4_16_add_matches_c_bd10() {
    fuzz_inv(10, 0x5741_4c53_4800_0011, true);
}

#[test]
fn highbd_iwht4x4_16_add_matches_c_bd12() {
    fuzz_inv(12, 0x5741_4c53_4800_0012, true);
}

#[test]
fn highbd_iwht4x4_1_add_matches_c_all_bit_depths() {
    fuzz_inv(8, 0x5741_4c53_4800_0020, false);
    fuzz_inv(10, 0x5741_4c53_4800_0021, false);
    fuzz_inv(12, 0x5741_4c53_4800_0022, false);
}

/// The C comment at inv_transforms.c:3214-3216 says the `eob <= 1` variant is
/// "significant (not just an optimization) for the lossless case". This pins
/// BOTH halves of that claim against the real C:
///
/// 1. when the tail is genuinely zero the two kernels agree exactly, so the
///    eob variant is not a different transform; but
/// 2. the caller only guarantees the first `eob` coefficients are meaningful,
///    and with a non-zero tail the two kernels give DIFFERENT reconstructions
///    — so a port that shipped only the 16-coefficient variant would produce
///    wrong pixels, not merely slower ones.
#[test]
fn eob1_variant_is_load_bearing_not_an_optimization() {
    let mut rng = Rng(0x5741_4c53_4800_0030);
    let base: Vec<u16> = (0..16).map(|_| rng.pixel(8)).collect();

    // (1) zero tail -> identical.
    for _ in 0..500 {
        let mut coeffs = [0i32; 16];
        coeffs[0] = rng.coeff();
        let sixteen = cref::highbd_iwht4x4_16_add(&coeffs, &base, 4, 4, 8);
        let one = cref::highbd_iwht4x4_1_add(&coeffs, &base, 4, 4, 8);
        assert_eq!(
            sixteen, one,
            "C kernels disagree on a DC-only block, coeffs {coeffs:?}"
        );
    }

    // (2) stale tail -> they diverge (measured, not assumed).
    let mut diverged = 0usize;
    let trials = 500usize;
    for _ in 0..trials {
        let mut coeffs = [0i32; 16];
        for v in coeffs.iter_mut() {
            *v = rng.coeff();
        }
        let sixteen = cref::highbd_iwht4x4_16_add(&coeffs, &base, 4, 4, 8);
        let one = cref::highbd_iwht4x4_1_add(&coeffs, &base, 4, 4, 8);
        if sixteen != one {
            diverged += 1;
        }
        // And the port reproduces whichever C kernel it was asked for.
        check_inv(&coeffs, &base, 4, 4, 8, true);
        check_inv(&coeffs, &base, 4, 4, 8, false);
    }
    assert!(
        diverged * 100 >= trials * 95,
        "expected the eob<=1 kernel to ignore a stale tail (diverged \
         {diverged}/{trials}) — if this drops, the premise of the C comment at \
         inv_transforms.c:3214 has changed"
    );
}

/// The port carries every inverse intermediate in `i32` with `wrapping_*`,
/// on the stated premise that this "reproduces the compiled behaviour" of C,
/// whose `TranLow` arithmetic is signed-overflow UB. The fuzz above
/// deliberately stays under `|c| <= 2^23` so no intermediate ever overflows —
/// which means that premise was ASSERTED but never MEASURED, and a real C
/// build is free to exploit the UB (it is compiled without `-fwrapv`).
///
/// This closes that gap: the whole `int32_t` coefficient domain, biased toward
/// `i32::MIN`/`i32::MAX`, where the inverse's butterflies genuinely wrap.
/// MEASURED 2026-08-03 on the prebuilt `libSvtAv1Enc.a`: 0 divergences in
/// 5000 blocks per kernel. If a future C build starts exploiting the UB this
/// fails loudly, and the port must switch to whatever the compiler actually
/// emits rather than to `wrapping_*` by assumption.
#[test]
fn inv_wht_matches_c_in_the_int32_wrap_regime() {
    let mut rng = Rng(0x5741_4c53_4800_0060);
    for _ in 0..5000 {
        let mut coeffs = [0i32; 16];
        for v in coeffs.iter_mut() {
            *v = rng.coeff_wrapping();
        }
        let base: Vec<u16> = (0..16).map(|_| rng.pixel(8)).collect();
        check_inv(&coeffs, &base, 4, 4, 8, true);
        for bd in [8u8, 10, 12] {
            check_inv(&coeffs, &base, 4, 4, bd, false);
        }
    }
}

/// `highbd_iwht4x4_add` (inv_transforms.c:2874) is `static` in C, so this
/// pins the port's selector against the two shimmed kernels directly.
#[test]
fn selector_dispatches_by_eob_like_c() {
    let mut rng = Rng(0x5741_4c53_4800_0040);
    let base: Vec<u16> = (0..16).map(|_| rng.pixel(8)).collect();
    let mut coeffs = [0i32; 16];
    for v in coeffs.iter_mut() {
        *v = rng.coeff();
    }
    for eob in [0i32, 1, 2, 8, 16] {
        let expect = if eob > 1 {
            cref::highbd_iwht4x4_16_add(&coeffs, &base, 4, 4, 8)
        } else {
            cref::highbd_iwht4x4_1_add(&coeffs, &base, 4, 4, 8)
        };
        let mut got = vec![0u16; 16];
        highbd_iwht4x4_add(&coeffs, &base, 4, &mut got, 4, eob, 8);
        assert_eq!(got, expect, "selector mismatch at eob {eob}");
    }
}

// =============================================================================
// End-to-end: the property that makes qindex 0 lossless
// =============================================================================

/// `iwht(transpose(fwht(residual)))` reconstructs the residual EXACTLY.
///
/// The transpose is C's, not ours: `svt_aom_estimate_transform`
/// (transforms.c:3955-3959) writes `coeff_buffer[(j << 2) + i] = dst[(i << 2) + j]`
/// after calling `svt_av1_fwht4x4`, because the forward kernel's two passes
/// contain a net transpose while the inverse's contain none. Getting that wrong
/// is silent: the roundtrip stops being lossless but nothing traps.
#[test]
fn wht_roundtrip_is_lossless() {
    let mut rng = Rng(0x5741_4c53_4800_0050);
    for _ in 0..2000 {
        let mut pred = [0u16; 16];
        let mut src = [0u16; 16];
        for i in 0..16 {
            pred[i] = rng.pixel(8);
            src[i] = rng.pixel(8);
        }
        let residual: Vec<i16> = (0..16).map(|i| src[i] as i16 - pred[i] as i16).collect();

        let mut dst = [0i32; 16];
        fwht4x4(&residual, &mut dst, 4);
        // The C dispatch-layer transpose.
        let mut coeffs = [0i32; 16];
        for i in 0..4 {
            for j in 0..4 {
                coeffs[(j << 2) + i] = dst[(i << 2) + j];
            }
        }

        let mut recon = [0u16; 16];
        highbd_iwht4x4_16_add(&coeffs, &pred, 4, &mut recon, 4, 8);
        assert_eq!(recon, src, "lossless roundtrip failed");

        // And C agrees, coefficient for coefficient and pixel for pixel.
        assert_eq!(cref::fwht4x4(&residual, 4), dst.to_vec());
        assert_eq!(
            cref::highbd_iwht4x4_16_add(&coeffs, &pred, 4, 4, 8),
            recon.to_vec()
        );
    }
}

/// Anti-vacuity for the test above: `wht_roundtrip_is_lossless` proves the
/// roundtrip works WITH the dispatch-layer transpose, but not that the
/// transpose is REQUIRED — a symmetric coefficient matrix would make it a
/// no-op and the test would pass either way, quietly blessing a wiring chunk
/// that dropped it. This runs the same roundtrip with the transpose OMITTED
/// and requires it to break. MEASURED: 500/500 blocks reconstruct WRONG, so
/// `fwht4x4`'s "output is the TRANSPOSE of the natural coefficient matrix"
/// doc claim is load-bearing, and the wiring chunk must carry
/// `coeff_buffer[(j << 2) + i] = dst[(i << 2) + j]` (transforms.c:3955-3959).
#[test]
fn dispatch_transpose_is_load_bearing() {
    let mut rng = Rng(0x5741_4c53_4800_0051);
    let mut broke = 0usize;
    let trials = 500usize;
    for _ in 0..trials {
        let pred: Vec<u16> = (0..16).map(|_| rng.pixel(8)).collect();
        let src: Vec<u16> = (0..16).map(|_| rng.pixel(8)).collect();
        let residual: Vec<i16> = (0..16).map(|i| src[i] as i16 - pred[i] as i16).collect();

        let mut dst = [0i32; 16];
        fwht4x4(&residual, &mut dst, 4);
        // Deliberately NO transpose — feed the kernel output straight in.
        let mut recon = vec![0u16; 16];
        highbd_iwht4x4_16_add(&dst, &pred, 4, &mut recon, 4, 8);
        if recon != src {
            broke += 1;
        }
    }
    assert!(
        broke > trials * 4 / 5,
        "omitting the dispatch transpose only broke {broke}/{trials} blocks — \
         if this drops, fwht4x4's net-transpose doc claim needs re-deriving"
    );
}
