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
