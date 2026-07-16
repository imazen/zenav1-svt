//! Differential parity: `predict_filter_intra` vs the C reference
//! (`svt_av1_filter_intra_predictor_c`, Source/Lib/C_DEFAULT/
//! filterintra_c.c:70).
//!
//! Filter-intra is a candidate at allintra presets <= M6 (filter_intra
//! level 2 -> FILTER_DC_PRED only), and when it wins the leaf RD compare
//! the WALK must reconstruct the block byte-exactly vs the decoder — so
//! every (mode, square tx size <= 32) pair is fuzzed over random edges,
//! including the flat / extreme edge patterns the spec fills produce.

use svtav1_cref as cref;
use svtav1_dsp::intra_pred;

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
        (self.next() >> 32) as u8
    }
}

/// (C TxSize index, width, height) for the square sizes filter-intra can
/// code (4x4..32x32; the flag is gated to blocks with w,h <= 32).
const SIZES: [(usize, usize, usize); 14] = [
    (0, 4, 4),
    (1, 8, 8),
    (2, 16, 16),
    (3, 32, 32),
    // Rectangular TXs (C TxSize enum ids) — the fi flag allows any w,h <= 32,
    // and the M0 g128-q20 drill proved the 16x4 kernel diverged while the
    // square-only sweep stayed green: rects were never differentially tested.
    (5, 4, 8),
    (6, 8, 4),
    (7, 8, 16),
    (8, 16, 8),
    (9, 16, 32),
    (10, 32, 16),
    (13, 4, 16),
    (14, 16, 4),
    (15, 8, 32),
    (16, 32, 8),
];

#[test]
fn filter_intra_predictor_matches_c() {
    let mut rng = Rng(0xf117e2_1a7a_0001);
    for &(c_tx, w, h) in &SIZES {
        for mode in 0u8..5 {
            for it in 0..200 {
                // above_with_corner[0] = corner, [1..=w] = above row.
                let mut above = vec![0u8; w + 1];
                let mut left = vec![0u8; h];
                match it % 4 {
                    0 => {
                        for v in above.iter_mut() {
                            *v = rng.byte();
                        }
                        for v in left.iter_mut() {
                            *v = rng.byte();
                        }
                    }
                    1 => {
                        // Spec edge-fill flavors: flat 127/129/128 fills.
                        above.fill(127);
                        left.fill(129);
                        above[0] = 128;
                    }
                    2 => {
                        // Gradient-ish edges.
                        for (i, v) in above.iter_mut().enumerate() {
                            *v = (i * 4) as u8;
                        }
                        for (i, v) in left.iter_mut().enumerate() {
                            *v = 255u8.wrapping_sub((i * 4) as u8);
                        }
                    }
                    _ => {
                        for v in above.iter_mut() {
                            *v = if rng.next() & 1 == 0 { 0 } else { 255 };
                        }
                        for v in left.iter_mut() {
                            *v = if rng.next() & 1 == 0 { 255 } else { 0 };
                        }
                    }
                }

                let stride = w + (rng.next() as usize % 2) * 16;
                let mut c_dst = vec![0u8; stride * h];
                let mut r_dst = vec![0u8; stride * h];

                cref::filter_intra_predictor(&mut c_dst, stride, c_tx, &above, &left, mode);
                intra_pred::predict_filter_intra(&mut r_dst, stride, &above, &left, w, h, mode);

                assert_eq!(
                    c_dst, r_dst,
                    "filter-intra mode {mode} {w}x{h} iter {it} stride {stride}"
                );
            }
        }
    }
}
