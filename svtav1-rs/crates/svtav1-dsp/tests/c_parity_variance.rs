//! Differential parity: variance / SSE vs the C reference.
//!
//! The C oracle is `svt_aom_variance{W}x{H}_c` (Source/Lib/C_DEFAULT/variance.c),
//! a TWO-block function: it fills `*sse = sum((a-b)^2)` and returns
//! `*sse - sum(a-b)^2 / (W*H)`. v4.2_functions.md shows variance.c did not
//! change 4.1->4.2.
//!
//! Two Rust APIs are audited:
//!
//!  * `sse(a, b, ...)` computes `sum((a-b)^2)` — this maps EXACTLY onto the
//!    `*sse` the C variance kernel produces, so it is checked bit-exact.
//!
//!  * `variance(block, ...)` is a SINGLE-block helper returning
//!    `(N*sum(x^2) - sum(x)^2, sum(x)/N)`. This is NOT the C two-block
//!    `svt_aom_variance*` quantity (different signature and, because C divides
//!    `sum^2/N` with integer truncation, a different value). It is verified
//!    here against the exact numerator rebuilt from two C oracles:
//!        sum(x)   = svt_aom_sad(block, zeros)        (pixels are >= 0)
//!        sum(x^2) = sse-output of svt_aom_variance(block, zeros)
//!    so the check is still fully C-grounded and bit-exact.

use svtav1_cref as cref;
use svtav1_dsp::variance::{sse, variance};

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
    fn byte(&mut self) -> u8 {
        (self.next() >> 33) as u8
    }
    fn range(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const SIZES: &[(usize, usize)] = &[
    (4, 4),
    (4, 8),
    (8, 8),
    (8, 16),
    (16, 16),
    (16, 32),
    (32, 32),
    (32, 64),
    (64, 64),
    (64, 128),
    (128, 128),
];

/// `sse(a,b)` must equal the `*sse` output of the C variance kernel.
#[test]
fn sse_matches_c_variance_sse_output() {
    let mut rng = Rng(0x55E_1234_u64);
    for &(w, h) in SIZES {
        for _ in 0..40 {
            let sa = w + rng.range(16);
            let sb = w + rng.range(16);
            let a: Vec<u8> = (0..sa * h).map(|_| rng.byte()).collect();
            let b: Vec<u8> = (0..sb * h).map(|_| rng.byte()).collect();

            let got = sse(&a, sa, &b, sb, w, h);
            let (_var, want_sse) = cref::variance(w, h, &a, 0, sa, &b, 0, sb);
            assert_eq!(got, want_sse as u64, "SSE {w}x{h} sa={sa} sb={sb}");
        }
    }
}

/// Single-block `variance()` matches the exact numerator/mean rebuilt from C
/// oracles (sum via SAD-vs-zeros, sum-of-squares via variance-vs-zeros).
#[test]
fn single_block_variance_matches_c_derived_numerator() {
    let mut rng = Rng(0xABC_9999_u64);
    for &(w, h) in SIZES {
        for _ in 0..40 {
            let stride = w + rng.range(16);
            let block: Vec<u8> = (0..stride * h).map(|_| rng.byte()).collect();
            let zeros = vec![0u8; stride * h];

            // C-derived ground truth.
            let sum_x = cref::sad(w, h, &block, 0, stride, &zeros, 0, stride) as u64;
            let (_v, sum_x2) = cref::variance(w, h, &block, 0, stride, &zeros, 0, stride);
            let n = (w * h) as u64;
            let want_var = sum_x2 as u64 * n - sum_x * sum_x;
            let want_mean = (sum_x / n) as u32;

            let (got_var, got_mean) = variance(&block, stride, w, h);
            assert_eq!(got_var, want_var, "variance() numerator {w}x{h}");
            assert_eq!(got_mean, want_mean, "variance() mean {w}x{h}");
        }
    }
}

/// Guard the documented semantic gap: the single-block `variance()` return is
/// deliberately NOT the C two-block `svt_aom_variance*` value. Confirm they
/// differ on a case where `sum^2` is not divisible by N (so C's integer
/// `sum^2/N` truncation makes `N*C_return != our numerator`).
#[test]
fn single_block_variance_is_not_c_two_block_variance() {
    // 4x4 ramp 0..15: sum=120, sum2=1240, N=16. sum^2=14400, 14400/16=900 exact
    // -> pick content where sum^2 % N != 0. 4x4 with one 1 and rest 0: sum=1,
    // sum2=1, sum^2=1, 1/16=0 (trunc). C_return = 1 - 0 = 1. Our numerator =
    // 16*1 - 1 = 15. 16*C_return = 16 != 15 -> divergence is real.
    let mut block = vec![0u8; 16];
    block[5] = 1;
    let (our_num, _) = variance(&block, 4, 4, 4);
    let (c_var, _sse) = cref::variance(4, 4, &block, 0, 4, &vec![0u8; 16], 0, 4);
    assert_eq!(our_num, 15, "our N^2-scaled numerator");
    assert_eq!(c_var, 1, "C two-block variance vs zeros (integer trunc)");
    assert_ne!(
        our_num,
        16 * c_var as u64,
        "documented: single-block variance() != N * svt_aom_variance"
    );
}
