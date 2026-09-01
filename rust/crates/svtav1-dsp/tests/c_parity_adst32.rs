//! Differential parity for the 32-point ADST kernels — **evidence tier 1**.
//!
//! `av1_fadst32_new` (transforms.c:2488) and `av1_iadst32_new`
//! (inv_transforms.c:1131) are `static` in C, so neither has a symbol of its
//! own to bind. They are still driven here at tier 1, because the exported
//! 2-D entries reach them with one argument: `svt_aom_transform_config` maps
//! `av1_txfm_type_ls[3][TX_TYPE_1D_ADST] = TXFM_TYPE_ADST32`
//! (inv_transforms.h:195), so any ADST/FLIPADST 1-D type landing on a
//! 32-sample dimension of `svt_av1_transform_two_d_32x32_c` /
//! `svt_av1_fwd_txfm2d_{16x32,32x16,8x32,32x8,32x64,64x32}_c` — and their
//! inverse twins — runs the real C kernel. Every cell below is the port
//! against those exported symbols, not against a second transcription.
//!
//! Why the cells look unreachable and are tested anyway: AV1's ext-tx sets
//! never offer an ADST type at a 32-dimension block (`tx_size_square_up` of
//! every size with a 32 side is `TX_32X32`, whose set is DCTONLY or
//! DCT_IDTX), so a conformant encode cannot select one. C's dispatch table
//! offers it regardless, and the port previously answered `None` from
//! `get_{fwd,inv}_txfm_func` and REFUSED the transform where C computes a
//! result. Per the port's "dead-looking C stays translated" rule the kernels
//! are ported and pinned here.
//!
//! 64 is deliberately absent from the ADST axis: `av1_txfm_type_ls[4]` is
//! `TXFM_TYPE_INVALID` for both ADST columns, so C's `type_to_func` asserts
//! and returns NULL there. The 32x64 / 64x32 cells below therefore put the
//! ADST on the 32 side only and leave the 64 side DCT or IDENTITY.

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
    /// Producer-bounded: an 8-bit residual is `src - pred` with both in
    /// `0..=255`, so `-255..=255` is exactly what the encoder can hand the
    /// forward transform. Nothing wider is generated.
    fn residual(&mut self) -> i16 {
        (self.next() % 511) as i16 - 255
    }
    /// Producer bound for the INVERSE direction. The forward transform of a
    /// realistic residual never leaves the low hundreds, so it cannot witness
    /// `av1_iadst32_new`'s per-stage `clamp_buf` (measured: deleting one of
    /// the eleven clamps left every residual-derived cell byte-identical).
    /// The inverse's real producer is a conformant BITSTREAM, and spec 7.12.3
    /// bounds a dequantized coefficient to
    /// `-(1 << (7 + BitDepth)) ..= (1 << (7 + BitDepth)) - 1` — `-32768..=32767`
    /// at 8-bit. That is the bound used here, and it is what makes the clamp
    /// observable. It stays inside C's own arithmetic domain: the inverse
    /// `half_btf` forms `cospi * in` with `|cospi| <= 4096` (`INV_COS_BIT`
    /// 12) against a caller-clamped 16-bit input, i.e. at most `2^28` in an
    /// `int32_t` — no overflow, so unlike the forward kernels at large inputs
    /// there is no "what does C do" ambiguity here.
    fn dequant_coeff(&mut self) -> i32 {
        (self.next() % 65536) as i32 - 32768
    }
}

/// `(col_1d, row_1d)` for a TxType, mirroring `txfm_dispatch::tx_type_to_1d`.
/// 0 = DCT, 1 = ADST, 2 = FLIPADST, 3 = IDENTITY.
fn tx_1d(t: TxType) -> (u8, u8) {
    match t {
        TxType::DctDct => (0, 0),
        TxType::AdstDct => (1, 0),
        TxType::DctAdst => (0, 1),
        TxType::AdstAdst => (1, 1),
        TxType::FlipAdstDct => (2, 0),
        TxType::DctFlipAdst => (0, 2),
        TxType::FlipAdstFlipAdst => (2, 2),
        TxType::AdstFlipAdst => (1, 2),
        TxType::FlipAdstAdst => (2, 1),
        TxType::Idtx => (3, 3),
        TxType::VDct => (0, 3),
        TxType::HDct => (3, 0),
        TxType::VAdst => (1, 3),
        TxType::HAdst => (3, 1),
        TxType::VFlipAdst => (2, 3),
        TxType::HFlipAdst => (3, 2),
    }
}

const ALL_TYPES: [(TxType, usize); 16] = [
    (TxType::DctDct, 0),
    (TxType::AdstDct, 1),
    (TxType::DctAdst, 2),
    (TxType::AdstAdst, 3),
    (TxType::FlipAdstDct, 4),
    (TxType::DctFlipAdst, 5),
    (TxType::FlipAdstFlipAdst, 6),
    (TxType::AdstFlipAdst, 7),
    (TxType::FlipAdstAdst, 8),
    (TxType::Idtx, 9),
    (TxType::VDct, 10),
    (TxType::HDct, 11),
    (TxType::VAdst, 12),
    (TxType::HAdst, 13),
    (TxType::VFlipAdst, 14),
    (TxType::HFlipAdst, 15),
];

/// Sizes whose column (= height) or row (= width) kernel is 32 samples.
/// The 64-dim pair is included so the "ADST on the 32 side, DCT/IDENTITY on
/// the 64 side" combinations are covered too.
const SIZES: [(usize, usize, TxSize); 7] = [
    (32, 32, TxSize::Tx32x32),
    (16, 32, TxSize::Tx16x32),
    (32, 16, TxSize::Tx32x16),
    (8, 32, TxSize::Tx8x32),
    (32, 8, TxSize::Tx32x8),
    (32, 64, TxSize::Tx32x64),
    (64, 32, TxSize::Tx64x32),
];

/// Does this (size, type) pair route an ADST/FLIPADST kernel at length 32,
/// with no ADST asked of a 64-sample dimension (which C cannot answer)?
fn reaches_adst32(w: usize, h: usize, t: TxType) -> bool {
    let (col, row) = tx_1d(t);
    let is_adst = |k: u8| k == 1 || k == 2;
    if (h == 64 && is_adst(col)) || (w == 64 && is_adst(row)) {
        return false; // TXFM_TYPE_INVALID in C — not a legal call.
    }
    (h == 32 && is_adst(col)) || (w == 32 && is_adst(row))
}

fn cref_fwd(w: usize, h: usize, res: &[i16], txt: usize) -> Vec<i32> {
    if w == h {
        cref::fwd_txfm2d(w, res, txt)
    } else {
        cref::fwd_txfm2d_rect(w, h, res, txt)
    }
}

fn cref_inv_add(w: usize, h: usize, coeffs: &[i32], base: &[u16], txt: usize) -> Vec<u16> {
    if w == h {
        cref::inv_txfm2d_add(w, coeffs, base, txt)
    } else {
        cref::inv_txfm2d_add_rect(w, h, coeffs, base, txt)
    }
}

/// Positive control for the cell selector: if `reaches_adst32` ever stops
/// selecting anything, the two tests below would pass vacuously.
#[test]
fn adst32_cell_selector_is_not_empty() {
    let mut n = 0usize;
    let mut per_size = [0usize; SIZES.len()];
    for (i, &(w, h, _)) in SIZES.iter().enumerate() {
        for &(t, _) in &ALL_TYPES {
            if reaches_adst32(w, h, t) {
                n += 1;
                per_size[i] += 1;
            }
        }
    }
    assert_eq!(
        per_size,
        [12, 8, 8, 8, 8, 4, 4],
        "cell counts per size changed"
    );
    assert_eq!(n, 52, "expected 52 ADST-32 cells, got {n}");
}

#[test]
fn fwd_adst32_matches_c() {
    let mut rng = Rng(0xAD57_3200_2026_0831);
    let mut cells = 0usize;
    for &(w, h, ts) in &SIZES {
        for &(t, txt) in &ALL_TYPES {
            if !reaches_adst32(w, h, t) {
                continue;
            }
            cells += 1;
            for trial in 0..12 {
                let res16: Vec<i16> = (0..w * h).map(|_| rng.residual()).collect();
                let c_out = cref_fwd(w, h, &res16, txt);

                let res32: Vec<i32> = res16.iter().map(|&v| v as i32).collect();
                let mut ours = vec![0i32; w * h];
                assert!(
                    svtav1_dsp::txfm_dispatch::fwd_txfm2d_dispatch(&res32, &mut ours, w, ts, t),
                    "fwd dispatch must now support {w}x{h} {t:?} (ADST-32)"
                );
                if ours != c_out {
                    let first = ours
                        .iter()
                        .zip(c_out.iter())
                        .position(|(a, b)| a != b)
                        .unwrap();
                    panic!(
                        "fwd {w}x{h} {t:?} trial {trial}: first diff at {} (r{} c{}): ours={} c={}",
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
    assert_eq!(cells, 52, "forward ADST-32 cell count");
}

#[test]
fn inv_adst32_recon_matches_c() {
    // The 64-dim inverse wrappers take their coefficients PACKED at stride
    // min(w, 32) over min(h, 32) rows, which `inv_txfm2d_dispatch` does not
    // model (its named-wrapper twins do, and `c_parity_txfm.rs` pins those on
    // the DCT path). ADST is illegal on a 64 dimension anyway, so the 32-side
    // kernel that is new here is fully exercised by the five non-64 sizes.
    let mut rng = Rng(0x1AD5_7320_2026_0831);
    let mut cells = 0usize;
    for &(w, h, ts) in &SIZES {
        if w == 64 || h == 64 {
            continue;
        }
        for &(t, txt) in &ALL_TYPES {
            if !reaches_adst32(w, h, t) {
                continue;
            }
            cells += 1;
            for trial in 0..24 {
                // Trials 0..12 take coefficients from the C forward transform
                // of a real residual (valid coefficient statistics by
                // construction). Trials 12..24 take the full spec-legal
                // dequantized range, which is what exercises the per-stage
                // clamp — see `Rng::dequant_coeff`.
                let coeffs: Vec<i32> = if trial < 12 {
                    let res16: Vec<i16> = (0..w * h).map(|_| rng.residual()).collect();
                    cref_fwd(w, h, &res16, txt)
                } else {
                    (0..w * h).map(|_| rng.dequant_coeff()).collect()
                };

                let base = vec![128u16; w * h];
                let c_recon = cref_inv_add(w, h, &coeffs, &base, txt);

                let mut our_res = vec![0i32; w * h];
                assert!(
                    svtav1_dsp::txfm_dispatch::inv_txfm2d_dispatch(&coeffs, &mut our_res, w, ts, t),
                    "inv dispatch must now support {w}x{h} {t:?} (ADST-32)"
                );
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
                        "inv {w}x{h} {t:?} trial {trial}: first diff at {} (r{} c{}): ours={} c={}",
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
    assert_eq!(cells, 44, "inverse ADST-32 cell count");
}
