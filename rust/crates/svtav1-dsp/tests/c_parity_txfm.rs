//! Differential parity: 2D transform WRAPPERS vs the C reference.
//!
//! The 1D kernels are golden-tested elsewhere; these tests target the 2D
//! composition (stage shifts, transpose order, intermediate ranges), which
//! the decoder implements per spec. Any wrapper divergence is invisible to
//! the encoder (its own fwd+inv roundtrip stays consistent) but corrupts
//! every AC coefficient on the wire.

use svtav1_cref as cref;
use svtav1_types::transform::{TxSize, TxType};

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
    fn residual(&mut self) -> i16 {
        (self.next() % 511) as i16 - 255
    }
}

const SIZES: [(usize, TxSize); 5] = [
    (4, TxSize::Tx4x4),
    (8, TxSize::Tx8x8),
    (16, TxSize::Tx16x16),
    (32, TxSize::Tx32x32),
    (64, TxSize::Tx64x64),
];

#[test]
fn fwd_txfm2d_matches_c() {
    let mut rng = Rng(0x5EED_F00D_2026_0713);
    for &(n, ts) in &SIZES {
        for trial in 0..24 {
            let res16: Vec<i16> = (0..n * n).map(|_| rng.residual()).collect();
            let c_out = cref::fwd_txfm2d(n, &res16, 0); // DCT_DCT

            let res32: Vec<i32> = res16.iter().map(|&v| v as i32).collect();
            let mut ours = vec![0i32; n * n];
            assert!(
                svtav1_dsp::txfm_dispatch::fwd_txfm2d_dispatch(
                    &res32,
                    &mut ours,
                    n,
                    ts,
                    TxType::DctDct
                ),
                "dispatch must support {n}x{n} DCT_DCT"
            );

            if ours != c_out {
                let first = ours
                    .iter()
                    .zip(c_out.iter())
                    .position(|(a, b)| a != b)
                    .unwrap();
                panic!(
                    "fwd {n}x{n} trial {trial}: first diff at {} (r{} c{}): ours={} c={} | ours[0..4]={:?} c[0..4]={:?}",
                    first,
                    first / n,
                    first % n,
                    ours[first],
                    c_out[first],
                    &ours[..4],
                    &c_out[..4]
                );
            }
        }
    }
}

#[test]
fn inv_txfm2d_recon_matches_c() {
    let mut rng = Rng(0xBADC_0FFE_E123_4567);
    for &(n, _ts) in &SIZES {
        for trial in 0..24 {
            // Coefficients from the C forward transform of a random residual
            // (valid coefficient statistics by construction).
            let res16: Vec<i16> = (0..n * n).map(|_| rng.residual()).collect();
            let coeffs = cref::fwd_txfm2d(n, &res16, 0);

            let base = vec![128u16; n * n];
            let c_recon = cref::inv_txfm2d_add(n, &coeffs, &base, 0);

            let mut our_res = vec![0i32; n * n];
            match n {
                4 => svtav1_dsp::inv_txfm::inv_txfm2d_4x4_dct_dct(&coeffs, &mut our_res, n),
                8 => svtav1_dsp::inv_txfm::inv_txfm2d_8x8_dct_dct(&coeffs, &mut our_res, n),
                16 => svtav1_dsp::inv_txfm::inv_txfm2d_16x16_dct_dct(&coeffs, &mut our_res, n),
                32 => svtav1_dsp::inv_txfm::inv_txfm2d_32x32_dct_dct(&coeffs, &mut our_res, n),
                _ => svtav1_dsp::inv_txfm::inv_txfm2d_64x64_dct_dct(&coeffs, &mut our_res, n),
            }
            let our_recon: Vec<u16> = our_res
                .iter()
                .map(|&r| (128 + r).clamp(0, 255) as u16)
                .collect();

            if our_recon != c_recon {
                let first = our_recon
                    .iter()
                    .zip(c_recon.iter())
                    .position(|(a, b)| a != b)
                    .unwrap();
                panic!(
                    "inv {n}x{n} trial {trial}: first diff at {} (r{} c{}): ours={} c={}",
                    first,
                    first / n,
                    first % n,
                    our_recon[first],
                    c_recon[first]
                );
            }
        }
    }
}

// ============================================================================
// ADDED CASES (additive only — the tests above are untouched).
//
// The two tests above pin the DISPATCH path; encode_loop's `use_optimized`
// branch calls the NAMED per-size wrappers instead, and the rect sizes were
// never pinned at all. These tests pin every named wrapper bit-exactly.
// ============================================================================

const RECTS: [(usize, usize, TxSize); 14] = [
    (4, 8, TxSize::Tx4x8),
    (8, 4, TxSize::Tx8x4),
    (8, 16, TxSize::Tx8x16),
    (16, 8, TxSize::Tx16x8),
    (16, 32, TxSize::Tx16x32),
    (32, 16, TxSize::Tx32x16),
    (32, 64, TxSize::Tx32x64),
    (64, 32, TxSize::Tx64x32),
    (4, 16, TxSize::Tx4x16),
    (16, 4, TxSize::Tx16x4),
    (8, 32, TxSize::Tx8x32),
    (32, 8, TxSize::Tx32x8),
    (16, 64, TxSize::Tx16x64),
    (64, 16, TxSize::Tx64x16),
];

/// Residual for a trial: two flat DC-only patterns first (the encode_loop
/// halving repro shape), then random.
fn trial_residual(rng: &mut Rng, n: usize, trial: usize) -> Vec<i16> {
    match trial {
        0 => vec![192i16; n],
        1 => vec![-192i16; n],
        _ => (0..n).map(|_| rng.residual()).collect(),
    }
}

fn named_fwd_square(n: usize, input: &[i32], output: &mut [i32], stride: usize) {
    use svtav1_dsp::fwd_txfm::*;
    match n {
        4 => fwd_txfm2d_4x4_dct_dct(input, output, stride),
        8 => fwd_txfm2d_8x8_dct_dct(input, output, stride),
        16 => fwd_txfm2d_16x16_dct_dct(input, output, stride),
        32 => fwd_txfm2d_32x32_dct_dct(input, output, stride),
        _ => fwd_txfm2d_64x64_dct_dct(input, output, stride),
    }
}

#[test]
fn fwd_named_square_wrappers_match_c() {
    let mut rng = Rng(0xC0DE_C0DE_2026_0713);
    for &(n, _ts) in &SIZES {
        for trial in 0..26 {
            let res16 = trial_residual(&mut rng, n * n, trial);
            let c_out = cref::fwd_txfm2d(n, &res16, 0);
            let res32: Vec<i32> = res16.iter().map(|&v| v as i32).collect();
            let mut ours = vec![0i32; n * n];
            named_fwd_square(n, &res32, &mut ours, n);
            if ours != c_out {
                let first = ours
                    .iter()
                    .zip(c_out.iter())
                    .position(|(a, b)| a != b)
                    .unwrap();
                panic!(
                    "named fwd {n}x{n} trial {trial}: first diff at {} (r{} c{}): ours={} c={}",
                    first,
                    first / n,
                    first % n,
                    ours[first],
                    c_out[first]
                );
            }
        }
    }
}

fn named_fwd_rect(w: usize, h: usize, input: &[i32], output: &mut [i32], stride: usize) {
    use svtav1_dsp::fwd_txfm::*;
    match (w, h) {
        (4, 8) => fwd_txfm2d_4x8_dct_dct(input, output, stride),
        (8, 4) => fwd_txfm2d_8x4_dct_dct(input, output, stride),
        (8, 16) => fwd_txfm2d_8x16_dct_dct(input, output, stride),
        (16, 8) => fwd_txfm2d_16x8_dct_dct(input, output, stride),
        (16, 32) => fwd_txfm2d_16x32_dct_dct(input, output, stride),
        (32, 16) => fwd_txfm2d_32x16_dct_dct(input, output, stride),
        (32, 64) => fwd_txfm2d_32x64_dct_dct(input, output, stride),
        (64, 32) => fwd_txfm2d_64x32_dct_dct(input, output, stride),
        (4, 16) => fwd_txfm2d_4x16_dct_dct(input, output, stride),
        (16, 4) => fwd_txfm2d_16x4_dct_dct(input, output, stride),
        (8, 32) => fwd_txfm2d_8x32_dct_dct(input, output, stride),
        (32, 8) => fwd_txfm2d_32x8_dct_dct(input, output, stride),
        (16, 64) => fwd_txfm2d_16x64_dct_dct(input, output, stride),
        _ => fwd_txfm2d_64x16_dct_dct(input, output, stride),
    }
}

#[test]
fn fwd_named_rect_wrappers_match_c() {
    let mut rng = Rng(0xFACE_FEED_2026_0713);
    for &(w, h, _ts) in &RECTS {
        for trial in 0..26 {
            let res16 = trial_residual(&mut rng, w * h, trial);
            let c_out = cref::fwd_txfm2d_rect(w, h, &res16, 0);
            let res32: Vec<i32> = res16.iter().map(|&v| v as i32).collect();
            let mut ours = vec![0i32; w * h];
            named_fwd_rect(w, h, &res32, &mut ours, w);
            if ours != c_out {
                let first = ours
                    .iter()
                    .zip(c_out.iter())
                    .position(|(a, b)| a != b)
                    .unwrap();
                panic!(
                    "named fwd {w}x{h} trial {trial}: first diff at {} (r{} c{}): ours={} c={}",
                    first,
                    first / w,
                    first % w,
                    ours[first],
                    c_out[first]
                );
            }
        }
    }
}

#[test]
fn fwd_dispatch_rect_matches_c() {
    // encode_loop's non-square path goes through the dispatch, not the named
    // wrappers — pin it separately.
    let mut rng = Rng(0xD15B_A7C4_2026_0713);
    for &(w, h, ts) in &RECTS {
        for trial in 0..26 {
            let res16 = trial_residual(&mut rng, w * h, trial);
            let c_out = cref::fwd_txfm2d_rect(w, h, &res16, 0);
            let res32: Vec<i32> = res16.iter().map(|&v| v as i32).collect();
            let mut ours = vec![0i32; w * h];
            assert!(
                svtav1_dsp::txfm_dispatch::fwd_txfm2d_dispatch(
                    &res32,
                    &mut ours,
                    w,
                    ts,
                    TxType::DctDct
                ),
                "dispatch must support {w}x{h} DCT_DCT"
            );
            if ours != c_out {
                let first = ours
                    .iter()
                    .zip(c_out.iter())
                    .position(|(a, b)| a != b)
                    .unwrap();
                panic!(
                    "dispatch fwd {w}x{h} trial {trial}: first diff at {} (r{} c{}): ours={} c={}",
                    first,
                    first / w,
                    first % w,
                    ours[first],
                    c_out[first]
                );
            }
        }
    }
}

fn named_inv_rect(w: usize, h: usize, input: &[i32], output: &mut [i32], stride: usize) {
    use svtav1_dsp::inv_txfm::*;
    match (w, h) {
        (4, 8) => inv_txfm2d_4x8_dct_dct(input, output, stride),
        (8, 4) => inv_txfm2d_8x4_dct_dct(input, output, stride),
        (8, 16) => inv_txfm2d_8x16_dct_dct(input, output, stride),
        (16, 8) => inv_txfm2d_16x8_dct_dct(input, output, stride),
        (16, 32) => inv_txfm2d_16x32_dct_dct(input, output, stride),
        (32, 16) => inv_txfm2d_32x16_dct_dct(input, output, stride),
        (32, 64) => inv_txfm2d_32x64_dct_dct(input, output, stride),
        (64, 32) => inv_txfm2d_64x32_dct_dct(input, output, stride),
        (4, 16) => inv_txfm2d_4x16_dct_dct(input, output, stride),
        (16, 4) => inv_txfm2d_16x4_dct_dct(input, output, stride),
        (8, 32) => inv_txfm2d_8x32_dct_dct(input, output, stride),
        (32, 8) => inv_txfm2d_32x8_dct_dct(input, output, stride),
        (16, 64) => inv_txfm2d_16x64_dct_dct(input, output, stride),
        _ => inv_txfm2d_64x16_dct_dct(input, output, stride),
    }
}

#[test]
fn inv_named_rect_wrappers_recon_match_c() {
    // Coefficients from the C rect forward transform; both sides consume the
    // same buffer (for 64-dim sizes the C inverse — and our named wrappers —
    // read it packed at stride min(w,32) over min(h,32) rows).
    let mut rng = Rng(0xBEEF_CAFE_2026_0713);
    for &(w, h, _ts) in &RECTS {
        for trial in 0..26 {
            let res16 = trial_residual(&mut rng, w * h, trial);
            let coeffs = cref::fwd_txfm2d_rect(w, h, &res16, 0);

            let base = vec![128u16; w * h];
            let c_recon = cref::inv_txfm2d_add_rect(w, h, &coeffs, &base, 0);

            let mut our_res = vec![0i32; w * h];
            named_inv_rect(w, h, &coeffs, &mut our_res, w);
            let our_recon: Vec<u16> = our_res
                .iter()
                .map(|&r| (128 + r).clamp(0, 255) as u16)
                .collect();

            if our_recon != c_recon {
                let first = our_recon
                    .iter()
                    .zip(c_recon.iter())
                    .position(|(a, b)| a != b)
                    .unwrap();
                panic!(
                    "named inv {w}x{h} trial {trial}: first diff at {} (r{} c{}): ours={} c={}",
                    first,
                    first / w,
                    first % w,
                    our_recon[first],
                    c_recon[first]
                );
            }
        }
    }
}

#[test]
fn inv_named_square_wrappers_flat_dc_match_c() {
    // The existing inverse test already pins the named square wrappers on
    // random-residual coefficients; add the flat DC-only shapes from the
    // encode_loop halving repro.
    for &(n, _ts) in &SIZES {
        for &dc in &[192i16, -192, 96, -96] {
            let res16 = vec![dc; n * n];
            let coeffs = cref::fwd_txfm2d(n, &res16, 0);
            let base = vec![128u16; n * n];
            let c_recon = cref::inv_txfm2d_add(n, &coeffs, &base, 0);

            let mut our_res = vec![0i32; n * n];
            match n {
                4 => svtav1_dsp::inv_txfm::inv_txfm2d_4x4_dct_dct(&coeffs, &mut our_res, n),
                8 => svtav1_dsp::inv_txfm::inv_txfm2d_8x8_dct_dct(&coeffs, &mut our_res, n),
                16 => svtav1_dsp::inv_txfm::inv_txfm2d_16x16_dct_dct(&coeffs, &mut our_res, n),
                32 => svtav1_dsp::inv_txfm::inv_txfm2d_32x32_dct_dct(&coeffs, &mut our_res, n),
                _ => svtav1_dsp::inv_txfm::inv_txfm2d_64x64_dct_dct(&coeffs, &mut our_res, n),
            }
            let our_recon: Vec<u16> = our_res
                .iter()
                .map(|&r| (128 + r).clamp(0, 255) as u16)
                .collect();
            assert_eq!(
                our_recon, c_recon,
                "named inv {n}x{n} flat dc={dc} recon mismatch"
            );
        }
    }
}

// ============================================================================
// SIMD DIFFERENTIAL (archmage AVX2 square DCT-DCT fast path).
//
// Proves the SIMD transform is byte-exact TWO ways, per size, over randomized
// AND edge (max-magnitude) inputs, under EVERY archmage dispatch tier
// (`for_each_token_permutation` forces v3 + scalar, so the SIMD path is
// genuinely exercised — not silently skipped):
//   (a) every tier's output == the exported real C reference; and
//   (b) every tier's residual is byte-identical to every other tier's — i.e.
//       SIMD == the (already C-verified) scalar core at full i32 precision
//       (stronger than the recon compare, which a clamp could mask).
// Extend `SIMD_SQUARE` as more sizes are vectorized.
// ============================================================================

use archmage::testing::{CompileTimePolicy, for_each_token_permutation};

const SIMD_SQUARE: [(usize, TxSize); 2] = [(8, TxSize::Tx8x8), (16, TxSize::Tx16x16)];

/// Residual patterns: max-magnitude edges first (stress the `mullo_epi32`
/// no-overflow invariant), then random.
fn simd_residual(pat: usize, n: usize, rng: &mut Rng) -> Vec<i16> {
    match pat {
        0 => vec![255i16; n],
        1 => vec![-255i16; n],
        2 => (0..n).map(|i| if i % 2 == 0 { 255 } else { -255 }).collect(),
        3 => (0..n).map(|i| if (i / 4) % 2 == 0 { 255 } else { -255 }).collect(),
        4 => (0..n).map(|i| if i % 3 == 0 { 255 } else { 0 }).collect(),
        _ => (0..n).map(|_| rng.residual()).collect(),
    }
}

#[test]
fn fwd_dct_simd_all_tiers_match_c() {
    let mut rng = Rng(0x51D0_2026_0720_C0DE);
    for &(n, ts) in &SIMD_SQUARE {
        for pat in 0..40 {
            let res16 = simd_residual(pat, n * n, &mut rng);
            let c_out = cref::fwd_txfm2d(n, &res16, 0); // DCT_DCT
            let res32: Vec<i32> = res16.iter().map(|&v| v as i32).collect();
            for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
                let mut ours = vec![0i32; n * n];
                assert!(svtav1_dsp::txfm_dispatch::fwd_txfm2d_dispatch(
                    &res32,
                    &mut ours,
                    n,
                    ts,
                    TxType::DctDct
                ));
                assert_eq!(ours, c_out, "fwd {n}x{n} pat {pat}: SIMD tier != C");
            });
        }
    }
}

#[test]
fn inv_dct_simd_all_tiers_identical_and_recon_match_c() {
    let mut rng = Rng(0xC0FF_EE20_2607_2012);
    for &(n, ts) in &SIMD_SQUARE {
        for pat in 0..60 {
            // Coefficients: realistic (from the C forward of a residual) for
            // pat < 40; wide-magnitude synthetic (near the row-range clamp,
            // overflow stress) for pat >= 40.
            let coeffs: Vec<i32> = if pat < 40 {
                let res16 = simd_residual(pat, n * n, &mut rng);
                cref::fwd_txfm2d(n, &res16, 0)
            } else {
                (0..n * n)
                    .map(|_| (rng.next() % 60001) as i32 - 30000)
                    .collect()
            };
            let base = vec![128u16; n * n];
            let c_recon = cref::inv_txfm2d_add(n, &coeffs, &base, 0);

            let mut first: Option<Vec<i32>> = None;
            for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
                let mut res = vec![0i32; n * n];
                assert!(svtav1_dsp::txfm_dispatch::inv_txfm2d_dispatch(
                    &coeffs,
                    &mut res,
                    n,
                    ts,
                    TxType::DctDct
                ));
                // (b) all tiers produce byte-identical residuals (SIMD==scalar)
                match &first {
                    None => first = Some(res.clone()),
                    Some(f) => assert_eq!(&res, f, "inv {n}x{n} pat {pat}: tier residual != scalar"),
                }
                // (a) recon == real C
                let recon: Vec<u16> =
                    res.iter().map(|&r| (128 + r).clamp(0, 255) as u16).collect();
                assert_eq!(recon, c_recon, "inv {n}x{n} pat {pat}: SIMD tier recon != C");
            });
        }
    }
}
