//! Differential parity: temporal-filter noise estimator vs C
//! (`svt_estimate_noise_fp16_c`, temporal_filtering.c:3555).
//!
//! AUDIT 2026-07-14. `estimate_noise_fp16` is a bit-exact port; this suite
//! pins it against the linked C library over flat / noisy / gradient / edge
//! content and sizes, including the `-65536` "too few smooth pixels" sentinel.
//! The rest of `temporal_filter` (the planewise blend) is a homegrown,
//! non-normative, inter-only heuristic and is documented as such — not a port.

use svtav1_cref as cref;
use svtav1_encoder::temporal_filter::estimate_noise_fp16;

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

fn check(src: &[u8], w: usize, h: usize, stride: usize, label: &str) {
    let rust = estimate_noise_fp16(src, w, h, stride);
    let c = cref::estimate_noise_fp16(src, w, h, stride);
    assert_eq!(rust, c, "{label} ({w}x{h} stride {stride})");
}

#[test]
fn noise_estimate_flat_matches_c() {
    for &(w, h) in &[(8usize, 8usize), (16, 16), (32, 24), (64, 64)] {
        let src = vec![128u8; h * w];
        check(&src, w, h, w, "flat");
    }
}

#[test]
fn noise_estimate_noisy_matches_c() {
    let mut rng = Rng(0x7F_00_01);
    for &(w, h) in &[(16usize, 16usize), (48, 32), (64, 64), (33, 17)] {
        let src: Vec<u8> = (0..w * h).map(|_| rng.byte()).collect();
        check(&src, w, h, w, "noisy");
    }
}

#[test]
fn noise_estimate_low_noise_matches_c() {
    // Flat base with small dithered noise → many smooth pixels, small Laplacian.
    let mut rng = Rng(0x7F_00_02);
    let (w, h) = (64usize, 48usize);
    let src: Vec<u8> = (0..w * h)
        .map(|_| 128i32.wrapping_add((rng.byte() % 5) as i32 - 2) as u8)
        .collect();
    check(&src, w, h, w, "low-noise");
}

#[test]
fn noise_estimate_gradient_matches_c() {
    let (w, h) = (64usize, 40usize);
    let mut src = vec![0u8; w * h];
    for r in 0..h {
        for c in 0..w {
            src[r * w + c] = ((r * 3 + c * 2) & 0xff) as u8;
        }
    }
    check(&src, w, h, w, "gradient");
}

#[test]
fn noise_estimate_strided_matches_c() {
    // Non-tight stride: the estimator must honor y_stride.
    let mut rng = Rng(0x7F_00_03);
    let (w, h, stride) = (40usize, 32usize, 48usize);
    let src: Vec<u8> = (0..h * stride).map(|_| rng.byte()).collect();
    check(&src, w, h, stride, "strided-noisy");
}

#[test]
fn noise_estimate_checkerboard_matches_c() {
    // A checkerboard has *zero* Sobel gradient (each pixel's horizontal and
    // vertical neighbors are symmetric), so every interior pixel counts as
    // "smooth" with a large (2040) Laplacian — NOT a sentinel. Pin the exact
    // value both sides agree on.
    let (w, h) = (32usize, 32usize);
    let mut src = vec![0u8; w * h];
    for r in 0..h {
        for c in 0..w {
            src[r * w + c] = if (r + c) & 1 == 0 { 0 } else { 255 };
        }
    }
    check(&src, w, h, w, "checkerboard");
}

#[test]
fn noise_estimate_sentinel_matches_c() {
    // Tiny plane: (5-2)*(5-2)=9 smooth pixels < SMOOTH_THRESHOLD(16) → both
    // sides must return the -65536 (-1 fp16) "unreliable" sentinel.
    let small = vec![128u8; 5 * 5];
    assert_eq!(
        cref::estimate_noise_fp16(&small, 5, 5, 5),
        -65536,
        "tiny plane should be the C -1 sentinel"
    );
    check(&small, 5, 5, 5, "tiny-sentinel");
}
