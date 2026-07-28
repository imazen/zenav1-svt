//! The NEON SAD must equal the scalar SAD exactly.
//!
//! SAD drives motion and mode search, so a wrong sum silently changes every
//! encoding decision — and this crate's bar is byte-identical OBUs against the
//! C encoder, which a one-off SAD would break invisibly.
//!
//! The NEON path accumulates through `vpadalq_u8` into u16 then `vpadalq_u16`
//! into u32. That is exact only if the u16 accumulator cannot overflow, so the
//! widths and heights below deliberately include the largest blocks the
//! encoder uses.

fn scalar_sad(
    src: &[u8], src_stride: usize, rf: &[u8], ref_stride: usize, w: usize, h: usize,
) -> u32 {
    let mut sum = 0u32;
    for row in 0..h {
        for col in 0..w {
            let s = src[row * src_stride + col] as i32;
            let r = rf[row * ref_stride + col] as i32;
            sum += (s - r).unsigned_abs();
        }
    }
    sum
}

fn plane(n: usize, seed: u32) -> Vec<u8> {
    let mut s = seed | 1;
    (0..n)
        .map(|_| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (s >> 24) as u8
        })
        .collect()
}

#[test]
fn sad_neon_matches_scalar_exactly() {
    const STRIDE: usize = 160;
    let src = plane(STRIDE * 160, 3);
    let rf = plane(STRIDE * 160, 7);

    // Every AV1 block dimension, plus widths that are NOT multiples of 16 so
    // the scalar tail is exercised, plus the maximum size where a u16
    // accumulator would overflow if the per-row drain were missing.
    for &w in &[4usize, 8, 12, 16, 20, 24, 32, 48, 64, 96, 128] {
        for &h in &[4usize, 8, 16, 32, 64, 128] {
            let want = scalar_sad(&src, STRIDE, &rf, STRIDE, w, h);
            let got = svtav1_dsp::sad::sad(&src, STRIDE, &rf, STRIDE, w, h);
            assert_eq!(got, want, "SAD mismatch at {w}x{h}");
        }
    }
}

#[test]
fn sad_neon_matches_scalar_on_worst_case_values() {
    // All-255 vs all-0 maximizes every accumulator: 255 * 128 * 128 = 4177920,
    // which overflows u16 many times over and would expose a missing drain.
    const STRIDE: usize = 128;
    let src = vec![255u8; STRIDE * 128];
    let rf = vec![0u8; STRIDE * 128];
    for &(w, h) in &[(16usize, 16usize), (64, 64), (128, 128)] {
        let want = scalar_sad(&src, STRIDE, &rf, STRIDE, w, h);
        let got = svtav1_dsp::sad::sad(&src, STRIDE, &rf, STRIDE, w, h);
        assert_eq!(got, want, "worst-case SAD mismatch at {w}x{h}");
        assert_eq!(got, 255 * (w * h) as u32);
    }
}
