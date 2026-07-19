//! Differential parity: the high-bit-depth distortion / variance / SAD
//! kernels vs the real C reference.
//!
//! These three feed every 10-bit RD decision: `full_distortion_kernel16_bits`
//! is the SSE the mode search minimizes, `highbd_variance` drives AQ / partition
//! variance gating, `highbd_sad_kernel` the coarse cost. All three are generic
//! (W, H) forms in C — no sized-wrapper family — so a stride/offset/overflow
//! bug is invisible until a real 10-bit encode. Fuzzed over a range of block
//! shapes and content at bd 10 and 12, on strided buffers (offset origin +
//! padded stride) so the offset/stride marshalling is exercised, not just the
//! tight-packed case.

use svtav1_cref as cref;
use svtav1_dsp::hbd;

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
}

/// AV1 luma/chroma block shapes the kernels are called on.
const SHAPES: &[(usize, usize)] = &[
    (4, 4), (8, 8), (16, 16), (32, 32), (64, 64), (4, 8), (8, 4),
    (16, 8), (8, 16), (32, 16), (16, 32), (64, 32), (4, 16), (16, 64),
];

/// A `w x h` region living at a non-zero offset inside a `stride`-padded plane
/// — so the offset/stride arguments are genuinely non-trivial.
fn plane(rng: &mut Rng, w: usize, h: usize, stride: usize, off: usize, bd: u8) -> Vec<u16> {
    let maxv = (1u32 << bd) - 1;
    let mut v = vec![0u16; off + h * stride + w];
    for px in v.iter_mut() {
        *px = (rng.next() as u32 & maxv) as u16;
    }
    v
}

#[test]
fn hbd_full_distortion_matches_c() {
    let mut rng = Rng(0xD157_2026_0718_0001);
    for bd in [10u8, 12] {
        for &(w, h) in SHAPES {
            for _ in 0..8 {
                let (in_stride, pr_stride) = (w + 5, w + 3);
                let (in_off, pr_off) = (2 * in_stride + 1, pr_stride + 2);
                let input = plane(&mut rng, w, h, in_stride, in_off, bd);
                let pred = plane(&mut rng, w, h, pr_stride, pr_off, bd);

                let ours = hbd::full_distortion_kernel16_bits(
                    &input, in_off, in_stride, &pred, pr_off, pr_stride, w, h,
                );
                let c = cref::full_distortion_kernel16(
                    &input, in_off, in_stride, &pred, pr_off, pr_stride, w, h,
                );
                assert_eq!(ours, c, "distortion bd{bd} {w}x{h}");
            }
        }
    }
}

#[test]
fn hbd_variance_matches_c() {
    let mut rng = Rng(0x7A21_2026_0718_0002);
    for bd in [10u8, 12] {
        for &(w, h) in SHAPES {
            for _ in 0..8 {
                let (a_stride, b_stride) = (w + 4, w + 6);
                let a = plane(&mut rng, w, h, a_stride, 0, bd);
                let b = plane(&mut rng, w, h, b_stride, 0, bd);
                let (ours_sse, ours_var) = hbd::highbd_variance(&a, a_stride, &b, b_stride, w, h);
                let (c_sse, c_var) = cref::variance_highbd(&a, a_stride, &b, b_stride, w, h);
                assert_eq!((ours_sse, ours_var), (c_sse, c_var), "variance bd{bd} {w}x{h}");
            }
        }
    }
}

#[test]
fn hbd_sad_matches_c() {
    let mut rng = Rng(0x5AD0_2026_0718_0003);
    for bd in [10u8, 12] {
        for &(w, h) in SHAPES {
            for _ in 0..8 {
                let (s_stride, r_stride) = (w + 7, w + 2);
                let src = plane(&mut rng, w, h, s_stride, 0, bd);
                let r = plane(&mut rng, w, h, r_stride, 0, bd);
                // Port takes (width, height); the cref wrapper takes the same
                // and swaps to C's (height, width) internally.
                let ours = hbd::highbd_sad_kernel(&src, s_stride, &r, r_stride, w, h);
                let c = cref::sad_16b_kernel(&src, s_stride, &r, r_stride, w, h);
                assert_eq!(ours, c, "sad bd{bd} {w}x{h}");
            }
        }
    }
}
