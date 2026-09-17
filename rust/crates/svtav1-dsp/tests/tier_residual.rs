//! Dispatch-tier lock for `residual_i32` / `residual_i16`.
//!
//! The AVX2 arms widen each u8 pair to i16, subtract (the difference is in
//! `[-255, 255]`, exact in i16), and store either as-is (i16) or widened to
//! i32 — the identical value the scalar core produces. No C shim exists for
//! the residual kernel itself; the end-to-end byte-identity gate is the C
//! parity check. This test pins every dispatch tier against a local scalar
//! reference across the 32-column / 16-column / scalar-tail paths, plus
//! non-multiple widths and padded strides.

use archmage::testing::{CompileTimePolicy, for_each_token_permutation};
use svtav1_dsp::residual::{residual_i16, residual_i32};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

#[test]
fn residual_all_tiers_match_scalar() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    // 32-wide hits the u8x32 arm, 16 the u8x16 arm, 8/4 the scalar tail,
    // and the odd widths exercise tail columns after a vector chunk.
    for &(w, h) in &[
        (64usize, 4usize),
        (32, 8),
        (48, 2),
        (16, 16),
        (20, 3),
        (8, 8),
        (4, 4),
        (12, 2),
        (36, 1),
        (1, 4),
    ] {
        let src_stride = w + 7;
        let pred_stride = w + 13;
        let src: Vec<u8> = (0..src_stride * h).map(|_| rng.next() as u8).collect();
        let pred: Vec<u8> = (0..pred_stride * h).map(|_| rng.next() as u8).collect();

        let want32: Vec<i32> = (0..w * h)
            .map(|i| {
                let r = i / w;
                let c = i % w;
                src[r * src_stride + c] as i32 - pred[r * pred_stride + c] as i32
            })
            .collect();
        let want16: Vec<i16> = want32.iter().map(|&v| v as i16).collect();

        let report = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            let mut out32 = vec![-1i32; w * h];
            residual_i32(&src, src_stride, &pred, pred_stride, w, h, &mut out32);
            assert_eq!(out32, want32, "residual_i32 w={w} h={h}");

            let mut out16 = vec![0x5555i16; w * h];
            residual_i16(&src, src_stride, &pred, pred_stride, w, h, &mut out16);
            assert_eq!(out16, want16, "residual_i16 w={w} h={h}");
        });
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert!(report.permutations_run >= 2);
    }
}
