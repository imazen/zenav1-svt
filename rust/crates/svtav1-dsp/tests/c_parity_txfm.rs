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

const SIMD_SQUARE: [(usize, TxSize); 4] = [
    (8, TxSize::Tx8x8),
    (16, TxSize::Tx16x16),
    (32, TxSize::Tx32x32),
    (64, TxSize::Tx64x64),
];

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

// ---- non-square (rectangular) DCT-DCT SIMD differential ----
//
// Every rect size with both dims a multiple of 8 (the sizes the rect SIMD path
// handles). Same two-way proof as the square tests, PLUS the rectangular
// `NewSqrt2`/`NewInvSqrt2` scaling is exercised at every 2:1 size (the `rect_
// scale` i64 helper) — the classic rect_type byte-diff source. Edge (max-mag)
// patterns first to stress the forward rect scale's large-coefficient path.

const SIMD_RECT: [(usize, usize, TxSize); 10] = [
    (8, 16, TxSize::Tx8x16),
    (16, 8, TxSize::Tx16x8),
    (16, 32, TxSize::Tx16x32),
    (32, 16, TxSize::Tx32x16),
    (32, 64, TxSize::Tx32x64),
    (64, 32, TxSize::Tx64x32),
    (8, 32, TxSize::Tx8x32),
    (32, 8, TxSize::Tx32x8),
    (16, 64, TxSize::Tx16x64),
    (64, 16, TxSize::Tx64x16),
];

#[test]
fn fwd_dct_simd_rect_all_tiers_match_c() {
    let mut rng = Rng(0x2EC7_2026_0720_1234);
    for &(w, h, ts) in &SIMD_RECT {
        for pat in 0..40 {
            let res16 = simd_residual(pat, w * h, &mut rng);
            let c_out = cref::fwd_txfm2d_rect(w, h, &res16, 0); // DCT_DCT
            let res32: Vec<i32> = res16.iter().map(|&v| v as i32).collect();
            for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
                let mut ours = vec![0i32; w * h];
                assert!(svtav1_dsp::txfm_dispatch::fwd_txfm2d_dispatch(
                    &res32,
                    &mut ours,
                    w,
                    ts,
                    TxType::DctDct
                ));
                if ours != c_out {
                    let first = ours
                        .iter()
                        .zip(c_out.iter())
                        .position(|(a, b)| a != b)
                        .unwrap();
                    panic!(
                        "fwd rect {w}x{h} pat {pat}: SIMD tier != C at {first} (r{} c{}): ours={} c={}",
                        first / w,
                        first % w,
                        ours[first],
                        c_out[first]
                    );
                }
            });
        }
    }
}

#[test]
fn inv_dct_simd_rect_all_tiers_identical_and_recon_match_c() {
    let mut rng = Rng(0x1A5E_2026_0720_ABCD);
    for &(w, h, _ts) in &SIMD_RECT {
        // Both the port named wrapper and the C rect inverse consume the SAME
        // coefficient bytes (for 64-dim: packed at stride min(dim,32) via
        // `mod_input_64` — identical to C), so recon-equality is meaningful and
        // the all-tiers-identical residual check pins SIMD == scalar.
        for pat in 0..50 {
            let coeffs: Vec<i32> = if pat < 40 {
                let res16 = simd_residual(pat, w * h, &mut rng);
                cref::fwd_txfm2d_rect(w, h, &res16, 0)
            } else {
                // wide-magnitude synthetic (rect_scale i64 stress)
                (0..w * h)
                    .map(|_| (rng.next() % 60001) as i32 - 30000)
                    .collect()
            };
            let base = vec![128u16; w * h];
            let c_recon = cref::inv_txfm2d_add_rect(w, h, &coeffs, &base, 0);

            let mut first_res: Option<Vec<i32>> = None;
            for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
                let mut our_res = vec![0i32; w * h];
                named_inv_rect(w, h, &coeffs, &mut our_res, w);
                match &first_res {
                    None => first_res = Some(our_res.clone()),
                    Some(f) => {
                        assert_eq!(&our_res, f, "inv rect {w}x{h} pat {pat}: tier residual != scalar")
                    }
                }
                let recon: Vec<u16> = our_res
                    .iter()
                    .map(|&r| (128 + r).clamp(0, 255) as u16)
                    .collect();
                assert_eq!(recon, c_recon, "inv rect {w}x{h} pat {pat}: SIMD tier recon != C");
            });
        }
    }
}

#[test]
fn inv_dct_simd_all_tiers_identical_and_recon_match_c() {
    let mut rng = Rng(0xC0FF_EE20_2607_2012);
    for &(n, ts) in &SIMD_SQUARE {
        // 64-dim inverse carries only the top-left 32x32 (`keep`) coefficients
        // (the AV1 bitstream never sends the rest). The port dispatch reads that
        // block at frame stride `n`; the exported C 64x64 inverse reads it
        // 32-*packed*. Lay a single `keep x keep` block into BOTH conventions so
        // the two decoders consume identical coefficients (for n<=32, keep==n and
        // the two arrays coincide).
        let keep = n.min(32);
        for pat in 0..60 {
            // Coefficients: realistic (C forward of a keep x keep residual) for
            // pat < 40; wide-magnitude synthetic (near the row-range clamp,
            // overflow stress) for pat >= 40.
            let block: Vec<i32> = if pat < 40 {
                let res = simd_residual(pat, keep * keep, &mut rng);
                cref::fwd_txfm2d(keep, &res, 0)
            } else {
                (0..keep * keep)
                    .map(|_| (rng.next() % 60001) as i32 - 30000)
                    .collect()
            };
            // Port view: `block` at frame stride n (top-left, rest zero).
            let mut port_coeffs = vec![0i32; n * n];
            // C view: `block` packed at stride `keep` (first keep*keep, rest zero).
            let mut cref_coeffs = vec![0i32; n * n];
            for r in 0..keep {
                for c in 0..keep {
                    port_coeffs[r * n + c] = block[r * keep + c];
                    cref_coeffs[r * keep + c] = block[r * keep + c];
                }
            }
            let base = vec![128u16; n * n];
            let c_recon = cref::inv_txfm2d_add(n, &cref_coeffs, &base, 0);

            let mut first: Option<Vec<i32>> = None;
            for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
                let mut res = vec![0i32; n * n];
                assert!(svtav1_dsp::txfm_dispatch::inv_txfm2d_dispatch(
                    &port_coeffs,
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

// ============================================================================
// ADST SIMD DIFFERENTIAL (ADST_DCT / DCT_ADST / ADST_ADST, no flips).
//
// The four sizes AV1 allows ADST for with both dims a lane-group multiple
// (8x8, 16x16, 8x16, 16x8) x the three non-flip ADST combos. Same two-way proof
// as the DCT tiers (every tier == real C, and every tier's residual byte-
// identical = SIMD == scalar), over edge (max-mag) + random + wide-synthetic
// inputs. The 8x16/16x8 cells also exercise the rect NewSqrt2/NewInvSqrt2 scale.
// ============================================================================

const SIMD_ADST: [(usize, usize, TxSize, TxType); 12] = [
    (8, 8, TxSize::Tx8x8, TxType::AdstDct),
    (8, 8, TxSize::Tx8x8, TxType::DctAdst),
    (8, 8, TxSize::Tx8x8, TxType::AdstAdst),
    (16, 16, TxSize::Tx16x16, TxType::AdstDct),
    (16, 16, TxSize::Tx16x16, TxType::DctAdst),
    (16, 16, TxSize::Tx16x16, TxType::AdstAdst),
    (8, 16, TxSize::Tx8x16, TxType::AdstDct),
    (8, 16, TxSize::Tx8x16, TxType::DctAdst),
    (8, 16, TxSize::Tx8x16, TxType::AdstAdst),
    (16, 8, TxSize::Tx16x8, TxType::AdstDct),
    (16, 8, TxSize::Tx16x8, TxType::DctAdst),
    (16, 8, TxSize::Tx16x8, TxType::AdstAdst),
];

fn cref_fwd_any(w: usize, h: usize, res: &[i16], txt: usize) -> Vec<i32> {
    if w == h {
        cref::fwd_txfm2d(w, res, txt)
    } else {
        cref::fwd_txfm2d_rect(w, h, res, txt)
    }
}

fn cref_inv_add_any(w: usize, h: usize, coeffs: &[i32], base: &[u16], txt: usize) -> Vec<u16> {
    if w == h {
        cref::inv_txfm2d_add(w, coeffs, base, txt)
    } else {
        cref::inv_txfm2d_add_rect(w, h, coeffs, base, txt)
    }
}

#[test]
fn fwd_adst_simd_all_tiers_match_c() {
    let mut rng = Rng(0xAD57_2026_0720_0001);
    for &(w, h, ts, txt) in &SIMD_ADST {
        let txi = txt as usize;
        for pat in 0..40 {
            let res16 = simd_residual(pat, w * h, &mut rng);
            let c_out = cref_fwd_any(w, h, &res16, txi);
            let res32: Vec<i32> = res16.iter().map(|&v| v as i32).collect();
            for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
                let mut ours = vec![0i32; w * h];
                assert!(svtav1_dsp::txfm_dispatch::fwd_txfm2d_dispatch(
                    &res32, &mut ours, w, ts, txt
                ));
                if ours != c_out {
                    let first = ours
                        .iter()
                        .zip(c_out.iter())
                        .position(|(a, b)| a != b)
                        .unwrap();
                    panic!(
                        "fwd adst {w}x{h} {txt:?} pat {pat}: SIMD tier != C at {first} (r{} c{}): ours={} c={}",
                        first / w,
                        first % w,
                        ours[first],
                        c_out[first]
                    );
                }
            });
        }
    }
}

#[test]
fn inv_adst_simd_all_tiers_identical_and_recon_match_c() {
    let mut rng = Rng(0xAD57_2026_0720_0002);
    for &(w, h, ts, txt) in &SIMD_ADST {
        let txi = txt as usize;
        for pat in 0..50 {
            let coeffs: Vec<i32> = if pat < 40 {
                let res16 = simd_residual(pat, w * h, &mut rng);
                cref_fwd_any(w, h, &res16, txi)
            } else {
                (0..w * h)
                    .map(|_| (rng.next() % 60001) as i32 - 30000)
                    .collect()
            };
            let base = vec![128u16; w * h];
            let c_recon = cref_inv_add_any(w, h, &coeffs, &base, txi);

            let mut first_res: Option<Vec<i32>> = None;
            for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
                let mut our_res = vec![0i32; w * h];
                assert!(svtav1_dsp::txfm_dispatch::inv_txfm2d_dispatch(
                    &coeffs, &mut our_res, w, ts, txt
                ));
                match &first_res {
                    None => first_res = Some(our_res.clone()),
                    Some(f) => assert_eq!(
                        &our_res, f,
                        "inv adst {w}x{h} {txt:?} pat {pat}: tier residual != scalar"
                    ),
                }
                let recon: Vec<u16> = our_res
                    .iter()
                    .map(|&r| (128 + r).clamp(0, 255) as u16)
                    .collect();
                assert_eq!(
                    recon, c_recon,
                    "inv adst {w}x{h} {txt:?} pat {pat}: SIMD tier recon != C"
                );
            });
        }
    }
}

// ============================================================================
// bd10 SIMD == scalar consistency (rect DCT + ADST inverse).
//
// The bd8 differentials above prove SIMD == real C. The inverse SIMD drivers
// also carry the bit-depth-dependent row/col stage ranges + highbd_wraplow
// (inv_txfm_ranges(bd)); the bd10 gates exercise them. This proves every tier's
// bd10 residual is byte-identical to the (C-verified) scalar core, so the bd10
// end-to-end gates cannot see a SIMD-vs-scalar divergence. No C oracle needed —
// the all-tiers-identical assertion pins SIMD == scalar at bd10.
// ============================================================================
#[test]
fn inv_simd_bd10_all_tiers_identical_rect_and_adst() {
    let mut rng = Rng(0xBD10_2026_0720_FEED);
    // rect DCT-DCT sizes + the ADST sizes/types.
    for &(w, h, ts) in &SIMD_RECT {
        run_bd10_tier_check(w, h, ts, TxType::DctDct, &mut rng);
    }
    for &(w, h, ts, txt) in &SIMD_ADST {
        run_bd10_tier_check(w, h, ts, txt, &mut rng);
    }
}

fn run_bd10_tier_check(w: usize, h: usize, ts: TxSize, txt: TxType, rng: &mut Rng) {
    for pat in 0..24 {
        // bd10-scale coefficients (wider dynamic range than bd8).
        let coeffs: Vec<i32> = if pat < 16 {
            (0..w * h)
                .map(|_| (rng.next() % 200001) as i32 - 100000)
                .collect()
        } else {
            let res16 = simd_residual(pat, w * h, rng);
            if w == h {
                cref::fwd_txfm2d(w, &res16, txt as usize)
            } else {
                cref::fwd_txfm2d_rect(w, h, &res16, txt as usize)
            }
        };
        let mut first_res: Option<Vec<i32>> = None;
        for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            let mut our_res = vec![0i32; w * h];
            assert!(svtav1_dsp::txfm_dispatch::inv_txfm2d_dispatch_bd(
                &coeffs, &mut our_res, w, ts, txt, 10
            ));
            match &first_res {
                None => first_res = Some(our_res.clone()),
                Some(f) => assert_eq!(
                    &our_res, f,
                    "inv bd10 {w}x{h} {txt:?} pat {pat}: tier residual != scalar"
                ),
            }
        });
    }
}

// ============================================================================
// EXT SIMD DIFFERENTIAL — FLIPADST (all 5 combos, with the block edge flip),
// IDENTITY (IDTX), and the mixed V_/H_ 1D types.
//
// Same two-way proof as the DCT/ADST tiers (every tier == real C, and every
// tier's residual byte-identical = SIMD == scalar), over edge + random +
// wide-synthetic inputs. The FLIPADST cells exercise ud_flip/lr_flip (the
// reversed row read + `reverse8` lane mirror); the IDENTITY cells exercise the
// `fidentity*`/`iidentity*` scales (shift for 8/32, `rect_scale` for 16); the
// 8x16/16x8 rects also exercise the NewSqrt2/NewInvSqrt2 scale.
//
// Coverage:
//   * 8x8, 16x16, 8x16, 16x8 x all 12 ext tx types (max dim <= 16, legal in the
//     full AV1 ext_tx set), and
//   * IDTX at 32x32, 16x32, 32x16, 8x32, 32x8 (IDTX's larger-size envelope).
// ============================================================================

/// (w, h, TxSize, TxType) for every ext combo the SIMD path handles.
fn ext_cells() -> Vec<(usize, usize, TxSize, TxType)> {
    let full_sizes = [
        (8usize, 8usize, TxSize::Tx8x8),
        (16, 16, TxSize::Tx16x16),
        (8, 16, TxSize::Tx8x16),
        (16, 8, TxSize::Tx16x8),
    ];
    // Every ext type at the full sizes: the 5 FLIPADST combos, IDTX, and the 6
    // mixed V_/H_ types.
    let ext_types = [
        TxType::FlipAdstDct,
        TxType::DctFlipAdst,
        TxType::FlipAdstFlipAdst,
        TxType::AdstFlipAdst,
        TxType::FlipAdstAdst,
        TxType::Idtx,
        TxType::VDct,
        TxType::HDct,
        TxType::VAdst,
        TxType::HAdst,
        TxType::VFlipAdst,
        TxType::HFlipAdst,
    ];
    // IDTX-only larger sizes (DCT_IDTX ext set; the only ext type legal there).
    let idtx_sizes = [
        (32usize, 32usize, TxSize::Tx32x32),
        (16, 32, TxSize::Tx16x32),
        (32, 16, TxSize::Tx32x16),
        (8, 32, TxSize::Tx8x32),
        (32, 8, TxSize::Tx32x8),
    ];
    let mut v = Vec::new();
    for &(w, h, ts) in &full_sizes {
        for &tt in &ext_types {
            v.push((w, h, ts, tt));
        }
    }
    for &(w, h, ts) in &idtx_sizes {
        v.push((w, h, ts, TxType::Idtx));
    }
    v
}

#[test]
fn fwd_ext_simd_all_tiers_match_c() {
    let mut rng = Rng(0xE47_2026_0720_0001);
    for (w, h, ts, txt) in ext_cells() {
        let txi = txt as usize;
        for pat in 0..40 {
            let res16 = simd_residual(pat, w * h, &mut rng);
            let c_out = cref_fwd_any(w, h, &res16, txi);
            let res32: Vec<i32> = res16.iter().map(|&v| v as i32).collect();
            for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
                let mut ours = vec![0i32; w * h];
                assert!(
                    svtav1_dsp::txfm_dispatch::fwd_txfm2d_dispatch(&res32, &mut ours, w, ts, txt),
                    "dispatch must support {w}x{h} {txt:?}"
                );
                if ours != c_out {
                    let first = ours
                        .iter()
                        .zip(c_out.iter())
                        .position(|(a, b)| a != b)
                        .unwrap();
                    panic!(
                        "fwd ext {w}x{h} {txt:?} pat {pat}: SIMD tier != C at {first} (r{} c{}): ours={} c={}",
                        first / w,
                        first % w,
                        ours[first],
                        c_out[first]
                    );
                }
            });
        }
    }
}

#[test]
fn inv_ext_simd_all_tiers_identical_and_recon_match_c() {
    let mut rng = Rng(0xE47_2026_0720_0002);
    for (w, h, ts, txt) in ext_cells() {
        let txi = txt as usize;
        for pat in 0..50 {
            let coeffs: Vec<i32> = if pat < 40 {
                let res16 = simd_residual(pat, w * h, &mut rng);
                cref_fwd_any(w, h, &res16, txi)
            } else {
                (0..w * h)
                    .map(|_| (rng.next() % 60001) as i32 - 30000)
                    .collect()
            };
            let base = vec![128u16; w * h];
            let c_recon = cref_inv_add_any(w, h, &coeffs, &base, txi);

            let mut first_res: Option<Vec<i32>> = None;
            for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
                let mut our_res = vec![0i32; w * h];
                assert!(svtav1_dsp::txfm_dispatch::inv_txfm2d_dispatch(
                    &coeffs, &mut our_res, w, ts, txt
                ));
                match &first_res {
                    None => first_res = Some(our_res.clone()),
                    Some(f) => assert_eq!(
                        &our_res, f,
                        "inv ext {w}x{h} {txt:?} pat {pat}: tier residual != scalar"
                    ),
                }
                let recon: Vec<u16> = our_res
                    .iter()
                    .map(|&r| (128 + r).clamp(0, 255) as u16)
                    .collect();
                if recon != c_recon {
                    let first = recon
                        .iter()
                        .zip(c_recon.iter())
                        .position(|(a, b)| a != b)
                        .unwrap();
                    panic!(
                        "inv ext {w}x{h} {txt:?} pat {pat}: SIMD tier recon != C at {first} (r{} c{}): ours={} c={}",
                        first / w,
                        first % w,
                        recon[first],
                        c_recon[first]
                    );
                }
            });
        }
    }
}

#[test]
fn inv_ext_simd_bd10_all_tiers_identical() {
    let mut rng = Rng(0xE47_2026_0720_0003);
    for (w, h, ts, txt) in ext_cells() {
        run_bd10_tier_check(w, h, ts, txt, &mut rng);
    }
}
