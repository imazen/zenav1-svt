//! Coding-path (C-exact) raster-order quantizers — archmage SIMD.
//!
//! SIMD companions to the scalar bd8 non-QM quantizers in
//! `svtav1-encoder`'s `quant.rs` (C `quantize_fp_helper_c` /
//! `svt_aom_quantize_b_c`, full_loop.c). These do ONLY the per-coefficient
//! arithmetic — the caller derives `eob` from the resulting `qcoeff` (see
//! below) and owns the table build.
//!
//! ## Why raster order is byte-exact for a scan-order quantizer
//!
//! The C quantizer walks the coefficient array in SCAN order `i`, reading and
//! writing `coeff[scan[i]]`. But the arithmetic at each position depends only
//! on `coeff[rc]` and `iz = (rc != 0)` (DC vs AC) — NOT on the scan index. The
//! scan is a permutation, so every raster position `rc` is visited exactly
//! once; visiting them in raster order 0..n instead produces the identical
//! `qcoeff[rc]` / `dqcoeff[rc]` for every position. Raster order makes the
//! load and store CONTIGUOUS (no gather/scatter), which is what lets it
//! vectorize.
//!
//! `iz` collapses to `rc != 0` because AV1 default/H/V scans all place the DC
//! coefficient (`rc == 0`) at scan index 0 and nowhere else, so the DC is the
//! single special lane. The vector body runs the whole block with the AC
//! (`iz = 1`) constants and the DC (position 0) is re-computed scalar after.
//!
//! Only `eob` is scan-order dependent: `eob = 1 + max{ i : qcoeff[scan[i]] !=
//! 0 }`. The caller finds it with a reverse scan walk over the finished
//! `qcoeff` (a load + compare, no multiply) — see
//! `svtav1_encoder::quant::quantize_fp`.
//!
//! ## bd8 only (i32-safe)
//!
//! The 8-bit path clamps `abs_coeff + round` to INT16 (C
//! `quantize_fp_helper_c:245` / `svt_aom_quantize_b_c:67`), so every
//! intermediate product fits i32:
//!   * `a <= 32767`, `quant_fp = 65536/dequant <= 16384` (min dequant 4)
//!     → `a * quant_fp <= 5.37e8 < i32::MAX`.
//!   * b path: `tmp*quant <= 32767*32767 < 1.08e9`; `(...>>16 + tmp) *
//!     quant_shift <= 49151*16384 < 8.06e8`; both fit i32.
//!   * `tmp32 * dequant`: `tmp32 <= ~49146`, `dequant <= 1828` → `< 9e7`.
//! So `_mm256_mullo_epi32` (low 32 bits) equals the scalar i64 product exactly.
//! The HIGHBD variants drop the INT16 clamp (coeffs reach ~2^19), so `a *
//! quant_fp` can exceed i32 — they are NOT covered here and stay scalar in the
//! encoder.

use archmage::prelude::*;

// ---------------------------------------------------------------------------
// quantize_fp (RDOQ initial quantize) — C quantize_fp_helper_c non-QM branch
// ---------------------------------------------------------------------------

/// FP-quantize one transform block in raster order (bd8). Writes `qcoeff` and
/// `dqcoeff` for every position `0..coeffs.len()`; DC (index 0) uses table
/// index 0, all other positions use index 1. `rounding` is the pre-shifted
/// `_fp` round row `(t.round_fp[iz] + ((1<<log_scale)>>1)) >> log_scale`.
///
/// Byte-exact with the scalar scan-order `quantize_fp` body (per-position
/// arithmetic is scan-order-independent; see module docs).
pub fn quantize_fp_raster(
    coeffs: &[i32],
    qcoeff: &mut [i32],
    dqcoeff: &mut [i32],
    rounding: &[i32; 2],
    quant_fp: &[i32; 2],
    dequant: &[i32; 2],
    log_scale: i32,
) {
    incant!(
        quantize_fp_raster_impl(coeffs, qcoeff, dqcoeff, rounding, quant_fp, dequant, log_scale),
        [v3, neon, scalar]
    );
}

/// One FP position — the single source of per-coefficient truth, shared by
/// the scalar core and the vector body's tail / DC fix-up. Mirrors
/// `svtav1_encoder::quant::quantize_fp`'s inner iteration exactly.
#[inline]
#[allow(clippy::too_many_arguments)]
fn fp_one(
    rc: usize,
    iz: usize,
    coeffs: &[i32],
    qcoeff: &mut [i32],
    dqcoeff: &mut [i32],
    rounding: &[i32; 2],
    quant_fp: &[i32; 2],
    dequant: &[i32; 2],
    log_scale: i32,
) {
    let thresh = dequant[iz] as i64;
    let coeff = coeffs[rc];
    let coeff_sign: i32 = if coeff < 0 { -1 } else { 0 };
    let abs_coeff = ((coeff ^ coeff_sign) - coeff_sign) as i64;
    let mut tmp32 = 0i32;
    if (abs_coeff << (1 + log_scale)) >= thresh {
        let a = (abs_coeff + rounding[iz] as i64).clamp(i16::MIN as i64, i16::MAX as i64);
        tmp32 = ((a * quant_fp[iz] as i64) >> (16 - log_scale)) as i32;
    }
    if tmp32 != 0 {
        qcoeff[rc] = (tmp32 ^ coeff_sign) - coeff_sign;
        let abs_dq = ((tmp32 as i64 * dequant[iz] as i64) >> log_scale) as i32;
        dqcoeff[rc] = (abs_dq ^ coeff_sign) - coeff_sign;
    } else {
        qcoeff[rc] = 0;
        dqcoeff[rc] = 0;
    }
}

#[inline]
fn quantize_fp_raster_core(
    coeffs: &[i32],
    qcoeff: &mut [i32],
    dqcoeff: &mut [i32],
    rounding: &[i32; 2],
    quant_fp: &[i32; 2],
    dequant: &[i32; 2],
    log_scale: i32,
) {
    let n = coeffs.len().min(qcoeff.len()).min(dqcoeff.len());
    for rc in 0..n {
        fp_one(
            rc,
            usize::from(rc != 0),
            coeffs,
            qcoeff,
            dqcoeff,
            rounding,
            quant_fp,
            dequant,
            log_scale,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn quantize_fp_raster_impl_scalar(
    _t: ScalarToken,
    coeffs: &[i32],
    qcoeff: &mut [i32],
    dqcoeff: &mut [i32],
    rounding: &[i32; 2],
    quant_fp: &[i32; 2],
    dequant: &[i32; 2],
    log_scale: i32,
) {
    quantize_fp_raster_core(coeffs, qcoeff, dqcoeff, rounding, quant_fp, dequant, log_scale);
}

#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn quantize_fp_raster_impl_neon(
    _t: NeonToken,
    coeffs: &[i32],
    qcoeff: &mut [i32],
    dqcoeff: &mut [i32],
    rounding: &[i32; 2],
    quant_fp: &[i32; 2],
    dequant: &[i32; 2],
    log_scale: i32,
) {
    // Auto-vectorized inside the NEON target-feature region (same scalar core,
    // no gather/scatter). Byte-identical to the scalar tier by construction.
    quantize_fp_raster_core(coeffs, qcoeff, dqcoeff, rounding, quant_fp, dequant, log_scale);
}

#[cfg(target_arch = "x86_64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn quantize_fp_raster_impl_v3(
    _t: Desktop64,
    coeffs: &[i32],
    qcoeff: &mut [i32],
    dqcoeff: &mut [i32],
    rounding: &[i32; 2],
    quant_fp: &[i32; 2],
    dequant: &[i32; 2],
    log_scale: i32,
) {
    let n = coeffs.len().min(qcoeff.len()).min(dqcoeff.len());

    // AC (iz = 1) constants, broadcast.
    let round_v = _mm256_set1_epi32(rounding[1]);
    let quant_v = _mm256_set1_epi32(quant_fp[1]);
    let deq_v = _mm256_set1_epi32(dequant[1]);
    let thresh_v = _mm256_set1_epi32(dequant[1]);
    let lo = _mm256_set1_epi32(i16::MIN as i32);
    let hi = _mm256_set1_epi32(i16::MAX as i32);
    let sh_pre = _mm_cvtsi32_si128(1 + log_scale); // abs << (1+log_scale)
    let sh_q = _mm_cvtsi32_si128(16 - log_scale); // product >> (16-log_scale)
    let sh_dq = _mm_cvtsi32_si128(log_scale); // dq product >> log_scale

    let mut rc = 0usize;
    while rc + 8 <= n {
        let src: &[i32; 8] = coeffs[rc..rc + 8].try_into().unwrap();
        let coeff = _mm256_loadu_si256(src);
        let coeff_sign = _mm256_srai_epi32::<31>(coeff); // 0 or -1 per lane
        let abs = _mm256_abs_epi32(coeff);

        // threshold: pass iff (abs << (1+log_scale)) >= dequant; `fail` = below.
        let shifted = _mm256_sll_epi32(abs, sh_pre);
        let fail = _mm256_cmpgt_epi32(thresh_v, shifted);

        // a = clamp_i16(abs + round); tmp32 = (a * quant_fp) >> (16-log_scale)
        let sum = _mm256_add_epi32(abs, round_v);
        let a = _mm256_max_epi32(_mm256_min_epi32(sum, hi), lo);
        let prod = _mm256_mullo_epi32(a, quant_v);
        let tmp = _mm256_sra_epi32(prod, sh_q);
        // zero where the threshold failed (pass ? tmp : 0)
        let tmp_masked = _mm256_andnot_si256(fail, tmp);

        // qcoeff = (tmp_masked ^ sign) - sign
        let q = _mm256_sub_epi32(_mm256_xor_si256(tmp_masked, coeff_sign), coeff_sign);
        // abs_dq = (tmp_masked * dequant) >> log_scale ; dqcoeff = signed
        let dqprod = _mm256_mullo_epi32(tmp_masked, deq_v);
        let absdq = _mm256_sra_epi32(dqprod, sh_dq);
        let dq = _mm256_sub_epi32(_mm256_xor_si256(absdq, coeff_sign), coeff_sign);

        let qo: &mut [i32; 8] = (&mut qcoeff[rc..rc + 8]).try_into().unwrap();
        _mm256_storeu_si256(qo, q);
        let dqo: &mut [i32; 8] = (&mut dqcoeff[rc..rc + 8]).try_into().unwrap();
        _mm256_storeu_si256(dqo, dq);
        rc += 8;
    }
    // Scalar tail (blocks whose length isn't a multiple of 8; per-position iz).
    while rc < n {
        fp_one(
            rc,
            usize::from(rc != 0),
            coeffs,
            qcoeff,
            dqcoeff,
            rounding,
            quant_fp,
            dequant,
            log_scale,
        );
        rc += 1;
    }
    // DC fix-up: the vector body ran position 0 with AC constants — redo it
    // with the DC (iz = 0) row. (No-op-correct if the tail already did it.)
    if n > 0 {
        fp_one(0, 0, coeffs, qcoeff, dqcoeff, rounding, quant_fp, dequant, log_scale);
    }
}

// ---------------------------------------------------------------------------
// quantize_b (dead-zone quantize) — C svt_aom_quantize_b_c non-QM branch
// ---------------------------------------------------------------------------

/// Dead-zone (`b`) quantize one transform block in raster order (bd8). Writes
/// `qcoeff` / `dqcoeff` for every position; DC uses index 0, AC index 1.
/// `zbins` and `round` are the pre-shifted rows
/// `(t.zbin[iz] + ((1<<log_scale)>>1)) >> log_scale` and
/// `(t.round[iz] + ((1<<log_scale)>>1)) >> log_scale`.
///
/// Byte-exact with the scalar scan-order `quantize_b` body: the C prescan's
/// `non_zero_count` only skips a trailing all-dead-zone SCAN suffix, whose
/// positions quantize to 0 under the same per-position zbin test applied here
/// (see module docs). The caller derives `eob` from `qcoeff`.
#[allow(clippy::too_many_arguments)]
pub fn quantize_b_raster(
    coeffs: &[i32],
    qcoeff: &mut [i32],
    dqcoeff: &mut [i32],
    zbins: &[i32; 2],
    round: &[i32; 2],
    quant: &[i32; 2],
    quant_shift: &[i32; 2],
    dequant: &[i32; 2],
    log_scale: i32,
) {
    incant!(
        quantize_b_raster_impl(
            coeffs, qcoeff, dqcoeff, zbins, round, quant, quant_shift, dequant, log_scale
        ),
        [v3, neon, scalar]
    );
}

#[inline]
#[allow(clippy::too_many_arguments)]
fn b_one(
    rc: usize,
    iz: usize,
    coeffs: &[i32],
    qcoeff: &mut [i32],
    dqcoeff: &mut [i32],
    zbins: &[i32; 2],
    round: &[i32; 2],
    quant: &[i32; 2],
    quant_shift: &[i32; 2],
    dequant: &[i32; 2],
    log_scale: i32,
) {
    let coeff = coeffs[rc];
    let coeff_sign: i32 = if coeff < 0 { -1 } else { 0 };
    let abs_coeff = (coeff ^ coeff_sign) - coeff_sign;
    let mut tmp32 = 0i32;
    if abs_coeff >= zbins[iz] {
        let tmp = (abs_coeff + round[iz]).clamp(i16::MIN as i32, i16::MAX as i32) as i64;
        tmp32 = (((((tmp * quant[iz] as i64) >> 16) + tmp) * quant_shift[iz] as i64)
            >> (16 - log_scale)) as i32;
    }
    if tmp32 != 0 {
        qcoeff[rc] = (tmp32 ^ coeff_sign) - coeff_sign;
        let abs_dq = ((tmp32 as i64 * dequant[iz] as i64) >> log_scale) as i32;
        dqcoeff[rc] = (abs_dq ^ coeff_sign) - coeff_sign;
    } else {
        qcoeff[rc] = 0;
        dqcoeff[rc] = 0;
    }
}

#[inline]
#[allow(clippy::too_many_arguments)]
fn quantize_b_raster_core(
    coeffs: &[i32],
    qcoeff: &mut [i32],
    dqcoeff: &mut [i32],
    zbins: &[i32; 2],
    round: &[i32; 2],
    quant: &[i32; 2],
    quant_shift: &[i32; 2],
    dequant: &[i32; 2],
    log_scale: i32,
) {
    let n = coeffs.len().min(qcoeff.len()).min(dqcoeff.len());
    for rc in 0..n {
        b_one(
            rc,
            usize::from(rc != 0),
            coeffs,
            qcoeff,
            dqcoeff,
            zbins,
            round,
            quant,
            quant_shift,
            dequant,
            log_scale,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn quantize_b_raster_impl_scalar(
    _t: ScalarToken,
    coeffs: &[i32],
    qcoeff: &mut [i32],
    dqcoeff: &mut [i32],
    zbins: &[i32; 2],
    round: &[i32; 2],
    quant: &[i32; 2],
    quant_shift: &[i32; 2],
    dequant: &[i32; 2],
    log_scale: i32,
) {
    quantize_b_raster_core(
        coeffs, qcoeff, dqcoeff, zbins, round, quant, quant_shift, dequant, log_scale,
    );
}

#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn quantize_b_raster_impl_neon(
    _t: NeonToken,
    coeffs: &[i32],
    qcoeff: &mut [i32],
    dqcoeff: &mut [i32],
    zbins: &[i32; 2],
    round: &[i32; 2],
    quant: &[i32; 2],
    quant_shift: &[i32; 2],
    dequant: &[i32; 2],
    log_scale: i32,
) {
    quantize_b_raster_core(
        coeffs, qcoeff, dqcoeff, zbins, round, quant, quant_shift, dequant, log_scale,
    );
}

#[cfg(target_arch = "x86_64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn quantize_b_raster_impl_v3(
    _t: Desktop64,
    coeffs: &[i32],
    qcoeff: &mut [i32],
    dqcoeff: &mut [i32],
    zbins: &[i32; 2],
    round: &[i32; 2],
    quant: &[i32; 2],
    quant_shift: &[i32; 2],
    dequant: &[i32; 2],
    log_scale: i32,
) {
    let n = coeffs.len().min(qcoeff.len()).min(dqcoeff.len());

    // AC (iz = 1) constants, broadcast.
    let zbin_v = _mm256_set1_epi32(zbins[1]);
    let round_v = _mm256_set1_epi32(round[1]);
    let quant_v = _mm256_set1_epi32(quant[1]);
    let qshift_v = _mm256_set1_epi32(quant_shift[1]);
    let deq_v = _mm256_set1_epi32(dequant[1]);
    let lo = _mm256_set1_epi32(i16::MIN as i32);
    let hi = _mm256_set1_epi32(i16::MAX as i32);
    let sh_q = _mm_cvtsi32_si128(16 - log_scale); // final >> (16-log_scale)
    let sh_dq = _mm_cvtsi32_si128(log_scale); // dq product >> log_scale

    let mut rc = 0usize;
    while rc + 8 <= n {
        let src: &[i32; 8] = coeffs[rc..rc + 8].try_into().unwrap();
        let coeff = _mm256_loadu_si256(src);
        let coeff_sign = _mm256_srai_epi32::<31>(coeff);
        let abs = _mm256_abs_epi32(coeff);

        // dead-zone: pass iff abs >= zbin; `fail` = abs < zbin.
        let fail = _mm256_cmpgt_epi32(zbin_v, abs);

        // tmp = clamp_i16(abs + round)
        let sum = _mm256_add_epi32(abs, round_v);
        let tmp = _mm256_max_epi32(_mm256_min_epi32(sum, hi), lo);
        // tmp32 = (((tmp*quant) >> 16) + tmp) * quant_shift >> (16-log_scale)
        let p1 = _mm256_srai_epi32::<16>(_mm256_mullo_epi32(tmp, quant_v));
        let inner = _mm256_add_epi32(p1, tmp);
        let p2 = _mm256_mullo_epi32(inner, qshift_v);
        let tmp32 = _mm256_sra_epi32(p2, sh_q);
        let tmp_masked = _mm256_andnot_si256(fail, tmp32); // pass ? tmp32 : 0

        let q = _mm256_sub_epi32(_mm256_xor_si256(tmp_masked, coeff_sign), coeff_sign);
        let dqprod = _mm256_mullo_epi32(tmp_masked, deq_v);
        let absdq = _mm256_sra_epi32(dqprod, sh_dq);
        let dq = _mm256_sub_epi32(_mm256_xor_si256(absdq, coeff_sign), coeff_sign);

        let qo: &mut [i32; 8] = (&mut qcoeff[rc..rc + 8]).try_into().unwrap();
        _mm256_storeu_si256(qo, q);
        let dqo: &mut [i32; 8] = (&mut dqcoeff[rc..rc + 8]).try_into().unwrap();
        _mm256_storeu_si256(dqo, dq);
        rc += 8;
    }
    while rc < n {
        b_one(
            rc,
            usize::from(rc != 0),
            coeffs,
            qcoeff,
            dqcoeff,
            zbins,
            round,
            quant,
            quant_shift,
            dequant,
            log_scale,
        );
        rc += 1;
    }
    if n > 0 {
        b_one(
            0, 0, coeffs, qcoeff, dqcoeff, zbins, round, quant, quant_shift, dequant, log_scale,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;
    use archmage::testing::{for_each_token_permutation, CompileTimePolicy};

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
        /// Coefficient-shaped: mostly small, tail of large, occasional i16
        /// extremes (the bd8 quantizer input contract).
        fn coeff(&mut self) -> i32 {
            let r = self.next();
            let mag = match r & 7 {
                0..=3 => (r >> 32) as i32 % 16,
                4..=5 => (r >> 32) as i32 % 256,
                6 => (r >> 32) as i32 % (i16::MAX as i32),
                _ => i16::MAX as i32,
            };
            if r & 8 == 0 { mag } else { -mag }
        }
    }

    /// bd8 quant-table-shaped rows (DC, AC) so the fuzz exercises realistic
    /// magnitudes: quant may be negative (invert_quant), quant_shift a power
    /// of two, dequant the AC/DC step. Fixed representative rows across the
    /// qindex range plus the extremes.
    const ROWS: &[([i32; 2], [i32; 2], [i32; 2], [i32; 2], [i32; 2], [i32; 2])] = &[
        // (zbin, round, quant, quant_shift, quant_fp, dequant) — qindex 0
        ([2, 2], [64, 64], [1, 1], [16384, 16384], [16384, 16384], [4, 4]),
        // qindex 220 (from quant.rs capture)
        ([326, 583], [195, 349], [-1255, -29571], [128, 128], [125, 70], [522, 933]),
        // a mid row (qindex ~128-ish shape)
        ([120, 180], [90, 130], [-8000, -12000], [256, 256], [420, 300], [156, 220]),
        // high qindex 255-ish (large dequant)
        ([700, 1200], [400, 700], [-20000, -30000], [128, 128], [40, 30], [1336, 1828]),
    ];

    fn n_for(class: usize) -> usize {
        // exercise a range of adjusted coeff counts (all multiples of 8/16)
        [16usize, 32, 64, 256, 1024][class % 5]
    }

    #[test]
    fn quantize_fp_raster_all_tiers_match() {
        let mut rng = Rng(0xDEAD_BEEF_1234_5678);
        // Reference (scalar core) vs every dispatch tier, over many cells.
        for (ri, &(_zbin, _round, _quant, _qshift, quant_fp, dequant)) in ROWS.iter().enumerate() {
            for class in 0..5usize {
                let n = n_for(class);
                for &log_scale in &[0i32, 1, 2] {
                    let coeffs: Vec<i32> = (0..n).map(|_| rng.coeff()).collect();
                    // Pre-shifted `_fp` round row, real shape ((64*q)>>7)>>log_scale.
                    // Exact values don't matter here — the test asserts tier
                    // EQUALITY, not table correctness (that's c_parity_quant's job).
                    let round_fp = [
                        ((64 * dequant[0]) >> 7 >> log_scale).max(0),
                        ((64 * dequant[1]) >> 7 >> log_scale).max(0),
                    ];

                    let mut ref_q = vec![0i32; n];
                    let mut ref_dq = vec![0i32; n];
                    quantize_fp_raster_core(
                        &coeffs, &mut ref_q, &mut ref_dq, &round_fp, &quant_fp, &dequant, log_scale,
                    );

                    let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
                        let mut q = vec![0i32; n];
                        let mut dq = vec![0i32; n];
                        quantize_fp_raster(
                            &coeffs, &mut q, &mut dq, &round_fp, &quant_fp, &dequant, log_scale,
                        );
                        assert_eq!(q, ref_q, "fp qcoeff tier mismatch ri={ri} n={n} ls={log_scale}");
                        assert_eq!(
                            dq, ref_dq,
                            "fp dqcoeff tier mismatch ri={ri} n={n} ls={log_scale}"
                        );
                    });
                }
            }
        }
    }

    #[test]
    fn quantize_b_raster_all_tiers_match() {
        let mut rng = Rng(0x0BAD_F00D_CAFE_9999);
        for (ri, &(zbin, round, quant, qshift, _quant_fp, dequant)) in ROWS.iter().enumerate() {
            for class in 0..5usize {
                let n = n_for(class);
                for &log_scale in &[0i32, 1, 2] {
                    let coeffs: Vec<i32> = (0..n).map(|_| rng.coeff()).collect();
                    let zbins = [
                        (zbin[0] + ((1 << log_scale) >> 1)) >> log_scale,
                        (zbin[1] + ((1 << log_scale) >> 1)) >> log_scale,
                    ];
                    let rnd = [
                        (round[0] + ((1 << log_scale) >> 1)) >> log_scale,
                        (round[1] + ((1 << log_scale) >> 1)) >> log_scale,
                    ];

                    let mut ref_q = vec![0i32; n];
                    let mut ref_dq = vec![0i32; n];
                    quantize_b_raster_core(
                        &coeffs, &mut ref_q, &mut ref_dq, &zbins, &rnd, &quant, &qshift, &dequant,
                        log_scale,
                    );

                    let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
                        let mut q = vec![0i32; n];
                        let mut dq = vec![0i32; n];
                        quantize_b_raster(
                            &coeffs, &mut q, &mut dq, &zbins, &rnd, &quant, &qshift, &dequant,
                            log_scale,
                        );
                        assert_eq!(q, ref_q, "b qcoeff tier mismatch ri={ri} n={n} ls={log_scale}");
                        assert_eq!(dq, ref_dq, "b dqcoeff tier mismatch ri={ri} n={n} ls={log_scale}");
                    });
                }
            }
        }
    }
}
