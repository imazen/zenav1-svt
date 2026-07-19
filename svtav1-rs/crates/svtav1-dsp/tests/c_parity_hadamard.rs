//! Differential parity: MD fast-loop Hadamard/SATD kernels vs the C
//! reference (`svt_aom_hadamard_{8x8,16x16,32x32}_c`, `svt_aom_satd_c`).
//!
//! These feed C's `hadamard_path` (product_coding_loop.c:1187) — the MDS0
//! fast-cost distortion of the M6 leaf funnel. One coefficient of
//! divergence shifts a fast cost and can flip a NIC pruning decision, so
//! the kernels are pinned bit-exact over random residuals spanning the
//! full i16 residual range (8-bit source: -255..255) plus torture values.

use svtav1_cref as cref;
use svtav1_dsp::hadamard;

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
        // 8-bit residual range with occasional extremes.
        let v = (self.next() >> 40) as i16 % 256;
        if self.next() & 15 == 0 {
            if v & 1 == 0 { 255 } else { -255 }
        } else {
            v
        }
    }
    /// bd10 residual range (task #94): the MD fast loop at `hbd_md` feeds
    /// `hadamard_path` a TRUE 10-bit residual (`src10 - pred10`), i.e.
    /// -1023..1023 — 4x the 8-bit range the fuzzer above covers. C's
    /// `hadamard_col8` carries every intermediate in `int16_t`, so the
    /// second pass can WRAP at 10-bit magnitudes where it never does at
    /// 8-bit; the port must reproduce that wrap bit-exactly.
    fn residual_bd10(&mut self) -> i16 {
        let v = (self.next() >> 40) as i16 % 1024;
        if self.next() & 15 == 0 {
            if v & 1 == 0 { 1023 } else { -1023 }
        } else {
            v
        }
    }
}

fn fuzz_dim(dim: usize, iters: usize, seed: u64) {
    fuzz_dim_gen(dim, iters, seed, false)
}

fn fuzz_dim_gen(dim: usize, iters: usize, seed: u64, bd10: bool) {
    let mut rng = Rng(seed);
    for it in 0..iters {
        // Random stride >= dim exercises the strided column reads.
        let stride = dim + (rng.next() as usize % 3) * 8;
        let mut src = vec![0i16; stride * dim + 8];
        for v in src.iter_mut() {
            *v = if bd10 { rng.residual_bd10() } else { rng.residual() };
        }
        let mut c_out = vec![0i32; dim * dim];
        let mut r_out = vec![0i32; dim * dim];
        cref::hadamard(dim, &src, stride, &mut c_out);
        match dim {
            8 => hadamard::aom_hadamard_8x8(&src, stride, &mut r_out),
            16 => hadamard::aom_hadamard_16x16(&src, stride, &mut r_out),
            32 => hadamard::aom_hadamard_32x32(&src, stride, &mut r_out),
            _ => unreachable!(),
        }
        assert_eq!(c_out, r_out, "hadamard {dim}x{dim} iter {it} stride {stride}");
        assert_eq!(
            cref::satd(&c_out),
            hadamard::aom_satd(&r_out),
            "satd {dim}x{dim} iter {it}"
        );
    }
}

#[test]
fn hadamard_8x8_matches_c() {
    fuzz_dim(8, 400, 0x8ada_11ad_0808_0808);
}

#[test]
fn hadamard_16x16_matches_c() {
    fuzz_dim(16, 200, 0x8ada_11ad_1616_1616);
}

#[test]
fn hadamard_32x32_matches_c() {
    fuzz_dim(32, 100, 0x8ada_11ad_3232_3232);
}

// --- bd10-range residuals vs the kernel the ENCODER runs (task #94) --------
//
// The bd10 MD fast loop SATDs a TRUE 10-bit residual (-1023..1023). Above the
// 8-bit range `svt_aom_hadamard_{16x16,32x32}_c` and `_avx2` DISAGREE: the
// AVX2 kernels carry the 16x16 cross-combine in wrapping 16-bit lanes and
// buffer the 32x32's four 16x16 sub-transforms in an `int16_t temp_coeff`,
// while `_c` keeps both in `int32_t` (see the note in src/hadamard.rs). The
// encoder binds the RTCD pointers to `_avx2` on any AVX2 host, so `_avx2` is
// the bit-exactness target; the port is ported from it.
//
// 8x8 has no AVX2 variant on this path (`SET_SSE2`), and its stage is int16 in
// every implementation, so it is pinned against `_c` at both ranges.

fn fuzz_dim_avx2(dim: usize, iters: usize, seed: u64, bd10: bool) {
    let mut rng = Rng(seed);
    for it in 0..iters {
        let stride = dim + (rng.next() as usize % 3) * 8;
        let mut src = vec![0i16; stride * dim + 8];
        for v in src.iter_mut() {
            *v = if bd10 { rng.residual_bd10() } else { rng.residual() };
        }
        let mut c_out = vec![0i32; dim * dim];
        let mut r_out = vec![0i32; dim * dim];
        cref::hadamard_avx2(dim, &src, stride, &mut c_out);
        match dim {
            16 => hadamard::aom_hadamard_16x16(&src, stride, &mut r_out),
            32 => hadamard::aom_hadamard_32x32(&src, stride, &mut r_out),
            _ => unreachable!(),
        }
        // The AVX2 kernels emit the coefficients in a DIFFERENT ORDER from the
        // `_c` ones ("The order of the output coeff of the hadamard is not
        // important. For optimization purposes the final transpose may be
        // skipped." — picture_operators_c.c). The port keeps the `_c` order.
        // Only `svt_aom_satd` (an order-independent sum of absolutes) consumes
        // these, so parity is pinned on the SATD plus the full coefficient
        // MULTISET — which is strictly stronger than the SATD alone and is
        // exactly the invariant the encoder depends on.
        let mut c_sorted = c_out.clone();
        let mut r_sorted = r_out.clone();
        c_sorted.sort_unstable();
        r_sorted.sort_unstable();
        assert_eq!(
            c_sorted, r_sorted,
            "hadamard_avx2 {dim}x{dim} iter {it} stride {stride} bd10={bd10} (coeff multiset)"
        );
        assert_eq!(
            cref::satd(&c_out),
            hadamard::aom_satd(&r_out),
            "satd(avx2) {dim}x{dim} iter {it} bd10={bd10}"
        );
    }
}

#[test]
fn hadamard_8x8_matches_c_bd10_range() {
    fuzz_dim_gen(8, 400, 0xbd10_11ad_0808_0808, true);
}

#[test]
fn hadamard_16x16_matches_avx2_8bit_range() {
    fuzz_dim_avx2(16, 200, 0x8ada_a7f2_1616_1616, false);
}

#[test]
fn hadamard_32x32_matches_avx2_8bit_range() {
    fuzz_dim_avx2(32, 100, 0x8ada_a7f2_3232_3232, false);
}

#[test]
fn hadamard_16x16_matches_avx2_bd10_range() {
    fuzz_dim_avx2(16, 200, 0xbd10_a7f2_1616_1616, true);
}

#[test]
fn hadamard_32x32_matches_avx2_bd10_range() {
    fuzz_dim_avx2(32, 100, 0xbd10_a7f2_3232_3232, true);
}

/// The divergence this port targets is REAL, not a theoretical one: at bd10
/// magnitudes `_c` and `_avx2` genuinely differ, so a port of `_c` cannot
/// reproduce the encoder. Fails loudly if upstream ever unifies them (at
/// which point the AVX2-shaped port above should be revisited).
#[test]
fn c_and_avx2_hadamard_diverge_at_bd10_range() {
    let mut rng = Rng(0xd1f_0f_c_a7f2_3232);
    let mut diverged = false;
    for _ in 0..100 {
        let stride = 32usize;
        let mut src = vec![0i16; stride * 32 + 8];
        for v in src.iter_mut() {
            *v = rng.residual_bd10();
        }
        let mut c_out = vec![0i32; 1024];
        let mut a_out = vec![0i32; 1024];
        cref::hadamard(32, &src, stride, &mut c_out);
        cref::hadamard_avx2(32, &src, stride, &mut a_out);
        if c_out != a_out {
            diverged = true;
            break;
        }
    }
    assert!(
        diverged,
        "svt_aom_hadamard_32x32_c and _avx2 no longer diverge at 10-bit residuals"
    );
}
