//! Differential parity: SAD (sum of absolute differences) vs the C reference
//! `svt_aom_sad{W}x{H}_c` (Source/Lib/C_DEFAULT/compute_sad_c.c).
//!
//! SAD is the motion-estimation distortion primitive. The C `_c` kernel is a
//! plain `sum += |src[x]-ref[x]|` over the block, so a correct port must be
//! bit-exact. v4.2_functions.md shows compute_sad_c.c did not change 4.1->4.2.
//!
//! Fuzzed over every AV1 block size the C library exposes, with randomized
//! strides (padded buffers) and random 8-bit content, plus the extreme
//! (all-0 vs all-255) case per size.

use svtav1_cref as cref;
use svtav1_dsp::sad::sad;

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

/// Every (w,h) that `svt_aom_sad{W}x{H}_c` is defined for.
const SIZES: &[(usize, usize)] = &[
    (4, 4),
    (4, 8),
    (4, 16),
    (8, 4),
    (8, 8),
    (8, 16),
    (8, 32),
    (16, 4),
    (16, 8),
    (16, 16),
    (16, 32),
    (16, 64),
    (32, 8),
    (32, 16),
    (32, 32),
    (32, 64),
    (64, 16),
    (64, 32),
    (64, 64),
    (64, 128),
    (128, 64),
    (128, 128),
];

#[test]
fn sad_matches_c_all_sizes_random() {
    let mut rng = Rng(0x5AD_C0FFEE_u64);
    for &(w, h) in SIZES {
        for _ in 0..40 {
            // Random strides >= width; independent for src and ref.
            let src_stride = w + rng.range(20);
            let ref_stride = w + rng.range(20);
            let src: Vec<u8> = (0..src_stride * h).map(|_| rng.byte()).collect();
            let refb: Vec<u8> = (0..ref_stride * h).map(|_| rng.byte()).collect();

            let got = sad(&src, src_stride, &refb, ref_stride, w, h);
            let want = cref::sad(w, h, &src, 0, src_stride, &refb, 0, ref_stride);
            assert_eq!(got, want, "SAD {w}x{h} src_s={src_stride} ref_s={ref_stride}");
        }
    }
}

#[test]
fn sad_matches_c_extremes() {
    for &(w, h) in SIZES {
        // Max per-pixel difference: 0 vs 255 -> SAD = 255 * w * h.
        let a = vec![0u8; w * h];
        let b = vec![255u8; w * h];
        let got = sad(&a, w, &b, w, w, h);
        let want = cref::sad(w, h, &a, 0, w, &b, 0, w);
        assert_eq!(got, want, "SAD extreme {w}x{h}");
        assert_eq!(got, 255 * (w * h) as u32);

        // Identical blocks -> 0.
        assert_eq!(sad(&a, w, &a, w, w, h), cref::sad(w, h, &a, 0, w, &a, 0, w));
    }
}

/// Offset (strided sub-region) parity: pull src/ref from the interior of a
/// larger buffer so the C origin offset is exercised on both sides.
#[test]
fn sad_matches_c_offset_regions() {
    let mut rng = Rng(0xABCD_1234_u64);
    for &(w, h) in SIZES {
        let stride = w + 32;
        let rows = h + 8;
        let src: Vec<u8> = (0..stride * rows).map(|_| rng.byte()).collect();
        let refb: Vec<u8> = (0..stride * rows).map(|_| rng.byte()).collect();
        let so = 4 * stride + 7;
        let ro = 3 * stride + 5;
        let got = sad(&src[so..], stride, &refb[ro..], stride, w, h);
        let want = cref::sad(w, h, &src, so, stride, &refb, ro, stride);
        assert_eq!(got, want, "SAD offset {w}x{h}");
    }
}
