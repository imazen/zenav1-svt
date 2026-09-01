//! Differential parity: `enc_dec_process.c`'s frame SSIM
//! (`svtav1-encoder/src/port_enc_dec_metrics.rs`) vs the REAL compiled C.
//!
//! **Evidence tier 1** (`docs/WORKING-ON-THIS.md` §4). `aom_ssim2` is
//! `static` in C but keeps its source ABI in the Release object, so
//! `build.rs` promotes it with `llvm-objcopy --globalize-symbol` and this
//! test drives the real code.
//!
//! Each call pins three more functions that have no symbol at all:
//! `ssim_8x8`, `svt_aom_ssim_parms_8x8_c`, and `svt_aom_similarity` (which IS
//! exported, but is reached here through its real caller rather than in
//! isolation).
//!
//! **Bit-exact, not approximate.** SSIM is a `double` and both sides do the
//! same operations in the same order, so the comparison is `to_bits()`
//! equality — an epsilon would hide a reassociated sum, which is exactly the
//! kind of divergence this test exists to catch.
//!
//! **NOT covered, and the 10-bit case is the interesting one.**
//! `aom_highbd_ssim2` HAS a symbol and its width/height do arrive in the
//! registers its signature implies — but LLVM constant-folded its `bd` and
//! `shift` arguments away, so binding it made every 10-bit cell behave as
//! though `shift == 0`. That was caught here, by a bit-exact comparison
//! against a hand-computation from the C source: the divergence was in the
//! SEVENTH significant digit, exactly the size that an epsilon tolerance
//! would have hidden. The 10-bit chain is therefore **tier 4**, with
//! hand-derived vectors in the port module, and the C-side rationale is in
//! `link_globalized_enc_dec_statics` (`svtav1-cref/build.rs`).
//! `get_sse_10bit`, `recode_loop_decision_maker` and the CDF-averaging group
//! are tier 4 or unported for the same family of reasons.

use svtav1_cref::enc_dec_metrics as cref;
use svtav1_encoder::port_enc_dec_metrics as m;

/// A cheap deterministic generator; no rand dependency.
struct Rng(u64);
impl Rng {
    fn next_u32(&mut self) -> u32 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 32) as u32
    }
    fn byte(&mut self) -> u8 {
        (self.next_u32() & 0xff) as u8
    }
}

/// Content shapes chosen so the SSIM denominator spans its interesting range:
/// flat planes (variance 0, so the score is dominated by the stabilisers),
/// gradients, noise, and a source/reference pair that differ a lot.
fn planes(seed: u64, kind: u8, w: usize, stride: usize, h: usize) -> (Vec<u8>, Vec<u8>) {
    let mut rng = Rng(seed | 1);
    let mut a = vec![0u8; stride * h];
    let mut b = vec![0u8; stride * h];
    for y in 0..h {
        for x in 0..w {
            let (av, bv) = match kind {
                0 => (128, 128),                                 // flat, identical
                1 => (128, 130),                                 // flat, offset
                2 => ((x * 3 + y) as u8, (x * 3 + y) as u8),     // gradient, identical
                3 => ((x * 3 + y) as u8, (x * 3 + y + 7) as u8), // gradient, offset
                4 => (rng.byte(), rng.byte()),                   // independent noise
                _ => {
                    let v = rng.byte();
                    (v, v.wrapping_add(3))
                }
            };
            a[y * stride + x] = av;
            b[y * stride + x] = bv;
        }
    }
    (a, b)
}

#[test]
fn ssim2_matches_c() {
    if !cref::enc_dec_statics_oracle_is_available() {
        return;
    }
    let mut cells = 0usize;
    let mut distinct = std::collections::BTreeSet::new();
    let mut nans = 0usize;
    for &(w, h) in &[
        (4usize, 4usize), // below the 8-pixel floor -> NaN on both sides
        (8, 8),           // exactly 8: still NaN (the test is `<= 8`)
        (9, 9),
        (12, 12),
        (16, 16),
        (17, 13),
        (32, 24),
        (64, 64),
        (65, 33),
    ] {
        for &stride_pad in &[0usize, 7] {
            for kind in 0..6u8 {
                let stride = w + stride_pad;
                let (a, b) = planes(
                    0x9E37_79B9_u64 ^ (w as u64) << 8 ^ kind as u64,
                    kind,
                    w,
                    stride,
                    h,
                );
                let want = cref::ssim2(&a, stride, &b, stride, w, h).expect("oracle");
                let got = m::ssim2(&a, stride, &b, stride, w, h);
                if want.is_nan() {
                    assert!(
                        got.is_nan(),
                        "C returned NaN and the port did not, at {w}x{h} kind={kind}"
                    );
                    nans += 1;
                } else {
                    assert_eq!(
                        got.to_bits(),
                        want.to_bits(),
                        "ssim2 mismatch at {w}x{h} stride={stride} kind={kind}: \
                         got {got} want {want}"
                    );
                    distinct.insert(want.to_bits());
                }
                cells += 1;
            }
        }
    }
    assert!(cells >= 100, "sweep collapsed to {cells} cells");
    assert!(
        distinct.len() > 20,
        "the C oracle produced only {} distinct scores over {cells} cells",
        distinct.len()
    );
    // Positive control for the NaN arm: if the `<= 8` early return ever stops
    // firing, this drops to 0 and the test says so rather than passing.
    assert!(
        nans > 0,
        "no cell exercised the too-small-region NaN return"
    );
}
