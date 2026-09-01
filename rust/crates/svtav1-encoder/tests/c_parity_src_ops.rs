//! Tier-1 differentials for `svtav1_encoder::port_src_ops` — the leaves of
//! `Source/Lib/Codec/src_ops_process.c` that are exported.
//!
//! The three variance measures are driven through their real exported C
//! symbols (`docs/WORKING-ON-THIS.md` §4 tier 1). The three TPL propagation
//! leaves in the same module are `static` with no exported caller that
//! isolates them, so their tests here are TIER 4 — derived from the C source,
//! and labelled as such in each test's name and body.

use svtav1_cref::preanalysis as cpre;
use svtav1_encoder::port_src_ops as port;
use svtav1_types::block::BlockSize;

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
}

/// Every square and rectangular block size the three measures accept, with
/// its `(width, height)`. C indexes `svt_aom_mefn_ptr` by the same
/// discriminant, and only the sizes `init_fn_ptr` fills have a `vf`.
const SIZES: &[(BlockSize, usize, usize)] = &[
    (BlockSize::Block4x4, 4, 4),
    (BlockSize::Block4x8, 4, 8),
    (BlockSize::Block8x4, 8, 4),
    (BlockSize::Block8x8, 8, 8),
    (BlockSize::Block8x16, 8, 16),
    (BlockSize::Block16x8, 16, 8),
    (BlockSize::Block16x16, 16, 16),
    (BlockSize::Block16x32, 16, 32),
    (BlockSize::Block32x16, 32, 16),
    (BlockSize::Block32x32, 32, 32),
    (BlockSize::Block32x64, 32, 64),
    (BlockSize::Block64x32, 64, 32),
    (BlockSize::Block64x64, 64, 64),
];

/// Blocks chosen so the mean sweeps the whole 0..255 range — the perceptual
/// weight is a parabola in the mean, so a suite clustered near 128 tests only
/// its peak.
fn blocks(bw: usize, bh: usize, stride: usize) -> Vec<(String, Vec<u8>)> {
    let mut rng = Rng(0x2468_ACE0_1357_9BDF ^ (bw as u64) << 8 ^ bh as u64);
    let n = stride * (bh + 1);
    let mut out: Vec<(String, Vec<u8>)> = Vec::new();
    for &v in &[0u8, 1, 63, 127, 128, 129, 200, 254, 255] {
        out.push((format!("flat{v}"), vec![v; n]));
    }
    // Many random blocks, not one. The perceptual boost's last step is a
    // FLOAT division truncated to an integer, so whether computing it in `f32`
    // (C's types) or `f64` gives the same answer depends on where the quotient
    // falls relative to an integer boundary — a single random block will not
    // find such a case.
    for k in 0..40 {
        out.push((format!("noise{k}"), (0..n).map(|_| rng.byte()).collect()));
    }
    // Two-value blocks around each of several means, so `mean` lands on both
    // sides of 128 with a non-zero variance.
    for &(lo, hi) in &[(0u8, 255u8), (0, 4), (100, 160), (250, 255), (60, 62)] {
        let mut b = vec![lo; n];
        for r in 0..bh {
            for c in 0..bw {
                if (r + c) % 2 == 0 {
                    b[r * stride + c] = hi;
                }
            }
        }
        out.push((format!("split{lo}_{hi}"), b));
    }
    // A ramp, so the mean's rounding (`ROUND_POWER_OF_TWO`) is exercised at
    // both a .5 boundary and away from one.
    for &off in &[0u32, 1, 2, 3] {
        let mut b = vec![0u8; n];
        for r in 0..bh {
            for c in 0..bw {
                b[r * stride + c] = ((r * bw + c) as u32 + off) as u8;
            }
        }
        out.push((format!("ramp{off}"), b));
    }
    out
}

#[test]
fn get_perpixel_variance_matches_c() {
    let mut cells = 0usize;
    let mut nonzero = 0usize;
    for &(bsize, bw, bh) in SIZES {
        let stride = bw + 5;
        for (name, blk) in blocks(bw, bh, stride) {
            let c = cpre::sops_get_perpixel_variance(&blk, stride, bsize as i32, bh);
            let r = port::get_perpixel_variance(&blk, stride, bsize);
            assert_eq!(r, c, "perpixel_variance {name} {bw}x{bh}");
            if c != 0 {
                nonzero += 1;
            }
            cells += 1;
        }
    }
    assert_eq!(cells, SIZES.len() * blocks(4, 4, 9).len());
    assert!(nonzero > 0, "every perpixel_variance probe returned 0");
}

#[test]
fn get_mean_and_perpixel_variance_matches_c() {
    let mut cells = 0usize;
    let mut means: Vec<u32> = Vec::new();
    for &(bsize, bw, bh) in SIZES {
        let stride = bw + 5;
        for (name, blk) in blocks(bw, bh, stride) {
            let c = cpre::sops_get_mean_and_perpixel_variance(&blk, stride, bsize as i32, bh);
            let r = port::get_mean_and_perpixel_variance(&blk, stride, bsize);
            assert_eq!(r, c, "mean_and_perpixel_variance {name} {bw}x{bh}");
            if !means.contains(&c.1) {
                means.push(c.1);
            }
            cells += 1;
        }
    }
    assert_eq!(cells, SIZES.len() * blocks(4, 4, 9).len());
    // The perceptual weight is a parabola in the mean; a suite whose means all
    // sit near 128 would only test its peak.
    assert!(means.iter().any(|m| *m < 40), "no low-mean block");
    assert!(means.iter().any(|m| *m > 215), "no high-mean block");
    assert!(
        means.iter().any(|m| (100..=160).contains(m)),
        "no mid-grey block"
    );
}

#[test]
fn get_perceptual_perpixel_variance_matches_c() {
    let mut cells = 0usize;
    let mut boosted = 0usize;
    for &(bsize, bw, bh) in SIZES {
        let stride = bw + 5;
        for (name, blk) in blocks(bw, bh, stride) {
            let c = cpre::sops_get_perceptual_perpixel_variance(&blk, stride, bsize as i32, bh);
            let r = port::get_perceptual_perpixel_variance(&blk, stride, bsize);
            assert_eq!(r, c, "perceptual_perpixel_variance {name} {bw}x{bh}");
            let (plain, _) = port::get_mean_and_perpixel_variance(&blk, stride, bsize);
            if c > plain {
                boosted += 1;
            }
            cells += 1;
        }
    }
    assert_eq!(cells, SIZES.len() * blocks(4, 4, 9).len());
    // If the boost never fired, the float division that defines this function
    // was never exercised and the test is a re-run of the previous one.
    assert!(
        boosted > 0,
        "the perceptual boost never raised the variance"
    );
}

// ---------------------------------------------------------------------------
// TIER 4 — `static` in C, derived from the source
// ---------------------------------------------------------------------------

/// `round_floor` (src_ops_process.c:1441) floors toward NEGATIVE infinity,
/// which is NOT what C's (or Rust's) `/` does for negative numerators. The
/// cases below are the ones that separate the two.
#[test]
fn tier4_round_floor_floors_toward_negative_infinity() {
    for bsize in [1i32, 2, 4, 8, 16, 32, 64, 128] {
        for pos in -300i32..=300 {
            let got = port::round_floor(pos, bsize).expect("non-zero bsize");
            // The mathematical floor, computed a different way.
            let want = (pos as f64 / bsize as f64).floor() as i32;
            assert_eq!(got, want, "round_floor({pos}, {bsize})");
        }
    }
    // The specific divergence from truncating division, spelled out.
    assert_eq!(port::round_floor(-1, 4), Some(-1));
    assert_eq!(-1 / 4, 0);
    assert_eq!(port::round_floor(-4, 4), Some(-1));
    assert_eq!(port::round_floor(-5, 4), Some(-2));
    assert_eq!(port::round_floor(0, 4), Some(0));
    assert_eq!(port::round_floor(3, 4), Some(0));
    assert_eq!(port::round_floor(4, 4), Some(1));
    // C would divide by zero; this refuses.
    assert_eq!(port::round_floor(7, 0), None);
}

/// `get_overlap_area` (src_ops_process.c:1411). TIER 4.
#[test]
fn tier4_get_overlap_area_matches_the_four_corners() {
    use port::OverlapCorner::*;
    // Aligned blocks overlap fully in every corner form.
    for (bsize, side) in [
        (BlockSize::Block4x4, 4i32),
        (BlockSize::Block8x8, 8),
        (BlockSize::Block16x16, 16),
        (BlockSize::Block32x32, 32),
        (BlockSize::Block64x64, 64),
    ] {
        for corner in [UpLeft, UpRight, DownLeft, DownRight] {
            assert_eq!(
                port::get_overlap_area(0, 0, 0, 0, corner, bsize),
                side * side,
                "aligned {bsize:?} {corner:?}"
            );
        }
        // A half-block shift shrinks the overlap by half in one direction.
        // WHICH position has to move is decided by the corner, and getting it
        // wrong is easy — the first draft of this test moved `ref_pos_row` for
        // the DownLeft case and expected a shrink, but DownLeft's height is
        // `ref_pos_row + bh - grid_pos_row`, so moving `ref_pos_row` GROWS it.
        // Each expectation below is spelled out from the C formula it tests.
        let half = side / 2;
        // UpLeft:    width = grid_col + bw - ref_col, height = grid_row + bh - ref_row
        assert_eq!(
            port::get_overlap_area(0, 0, 0, half, UpLeft, bsize),
            (side - half) * side
        );
        // UpRight:   width = ref_col + bw - grid_col, height = grid_row + bh - ref_row
        assert_eq!(
            port::get_overlap_area(0, half, 0, 0, UpRight, bsize),
            (side - half) * side
        );
        // DownLeft:  width = grid_col + bw - ref_col, height = ref_row + bh - grid_row
        assert_eq!(
            port::get_overlap_area(half, 0, 0, 0, DownLeft, bsize),
            side * (side - half)
        );
        // ... and moving `ref_pos_row` instead GROWS DownLeft's height.
        assert_eq!(
            port::get_overlap_area(0, 0, half, 0, DownLeft, bsize),
            side * (side + half)
        );
        // DownRight: width = ref_col + bw - grid_col, height = ref_row + bh - grid_row
        assert_eq!(
            port::get_overlap_area(half, 0, 0, 0, DownRight, bsize),
            side * (side - half)
        );
        assert_eq!(
            port::get_overlap_area(0, half, 0, 0, DownRight, bsize),
            (side - half) * side
        );
    }
    // Rectangular sizes: the width and height come from different tables.
    assert_eq!(
        port::get_overlap_area(0, 0, 0, 0, UpLeft, BlockSize::Block16x32),
        16 * 32
    );
    assert_eq!(
        port::get_overlap_area(0, 0, 0, 0, UpLeft, BlockSize::Block64x16),
        64 * 16
    );
    // Non-overlapping blocks give a negative product, which C returns as-is.
    assert!(port::get_overlap_area(0, 0, 0, 100, UpLeft, BlockSize::Block8x8) < 0);
}

/// `delta_rate_cost` (src_ops_process.c:1452). TIER 4, and the only function
/// in the module that calls a transcendental — see the port's cross-ISA note.
#[test]
fn tier4_delta_rate_cost_arms() {
    // `srcrf_dist <= 128` returns the rate unchanged, before any float work.
    for d in [0i64, 1, 128] {
        assert_eq!(port::delta_rate_cost(12345, 1000, d, 256), Some(12345));
    }
    assert_ne!(port::delta_rate_cost(12345, 1000, 129, 256), Some(12345));

    // `recrf_dist == 0` divides by zero in C; this refuses instead.
    assert_eq!(port::delta_rate_cost(1, 0, 1000, 256), None);
    assert_eq!(port::delta_rate_cost(1, 1000, 1000, 0), None);

    // Both float arms are reached. The high-`log_den` arm is taken when beta
    // is large (srcrf much bigger than recrf) or `delta_rate` is;  the other
    // when it is not.
    let big = port::delta_rate_cost(1 << 30, 1, 1 << 40, 16).expect("defined");
    let small = port::delta_rate_cost(0, 1000, 1000, 256).expect("defined");
    assert_ne!(big, small);
    // The shift is applied on both arms, so every result is a multiple of
    // `1 << (TPL_DEP_COST_SCALE_LOG2 + AV1_PROB_COST_SHIFT)`.
    let shift = port::TPL_DEP_COST_SCALE_LOG2 + port::AV1_PROB_COST_SHIFT;
    for r in [big, small] {
        assert_eq!(r & ((1i64 << shift) - 1), 0, "result {r} is not shifted");
    }
}
