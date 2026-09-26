use super::*;
use alloc::vec;
use alloc::vec::Vec;
use archmage::testing::{CompileTimePolicy, for_each_token_permutation};

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
#[allow(clippy::type_complexity)] // inline tuple documents the shape; a `type` alias would hide it
const ROWS: &[([i32; 2], [i32; 2], [i32; 2], [i32; 2], [i32; 2], [i32; 2])] = &[
    // (zbin, round, quant, quant_shift, quant_fp, dequant) — qindex 0
    (
        [2, 2],
        [64, 64],
        [1, 1],
        [16384, 16384],
        [16384, 16384],
        [4, 4],
    ),
    // qindex 220 (from quant.rs capture)
    (
        [326, 583],
        [195, 349],
        [-1255, -29571],
        [128, 128],
        [125, 70],
        [522, 933],
    ),
    // a mid row (qindex ~128-ish shape)
    (
        [120, 180],
        [90, 130],
        [-8000, -12000],
        [256, 256],
        [420, 300],
        [156, 220],
    ),
    // high qindex 255-ish (large dequant)
    (
        [700, 1200],
        [400, 700],
        [-20000, -30000],
        [128, 128],
        [40, 30],
        [1336, 1828],
    ),
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
                    &coeffs,
                    &mut ref_q,
                    &mut ref_dq,
                    &round_fp,
                    &quant_fp,
                    &dequant,
                    log_scale,
                );

                let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
                    let mut q = vec![0i32; n];
                    let mut dq = vec![0i32; n];
                    quantize_fp_raster(
                        &coeffs, &mut q, &mut dq, &round_fp, &quant_fp, &dequant, log_scale,
                    );
                    assert_eq!(
                        q, ref_q,
                        "fp qcoeff tier mismatch ri={ri} n={n} ls={log_scale}"
                    );
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
                    &coeffs,
                    &mut ref_q,
                    &mut ref_dq,
                    &zbins,
                    &rnd,
                    &quant,
                    &qshift,
                    &dequant,
                    log_scale,
                );

                let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
                    let mut q = vec![0i32; n];
                    let mut dq = vec![0i32; n];
                    quantize_b_raster(
                        &coeffs, &mut q, &mut dq, &zbins, &rnd, &quant, &qshift, &dequant,
                        log_scale,
                    );
                    assert_eq!(
                        q, ref_q,
                        "b qcoeff tier mismatch ri={ri} n={n} ls={log_scale}"
                    );
                    assert_eq!(
                        dq, ref_dq,
                        "b dqcoeff tier mismatch ri={ri} n={n} ls={log_scale}"
                    );
                });
            }
        }
    }
}
