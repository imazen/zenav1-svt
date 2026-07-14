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
}

fn fuzz_dim(dim: usize, iters: usize, seed: u64) {
    let mut rng = Rng(seed);
    for it in 0..iters {
        // Random stride >= dim exercises the strided column reads.
        let stride = dim + (rng.next() as usize % 3) * 8;
        let mut src = vec![0i16; stride * dim + 8];
        for v in src.iter_mut() {
            *v = rng.residual();
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
