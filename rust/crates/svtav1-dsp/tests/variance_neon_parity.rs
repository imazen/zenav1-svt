//! NEON `variance` and `sse` must equal the scalar versions exactly.
//!
//! Both feed RD cost, so a wrong sum changes mode decisions and breaks this
//! crate's byte-identical-OBU bar invisibly.
//!
//! The NEON paths accumulate squares through u16 lanes (`vmull_u8`) into u32.
//! A u16 lane saturates conceptually at 65025 = 255^2, so the tests below
//! include all-255 inputs at the largest block sizes — where accumulating in
//! u16, or failing to drain to u32/u64 often enough, overflows.

fn scalar_variance(src: &[u8], stride: usize, w: usize, h: usize) -> (u64, u32) {
    let (mut sum, mut sum_sq) = (0u64, 0u64);
    for row in 0..h {
        for col in 0..w {
            let v = src[row * stride + col] as u64;
            sum += v;
            sum_sq += v * v;
        }
    }
    let n = (w * h) as u64;
    (sum_sq * n - sum * sum, (sum / n) as u32)
}

fn scalar_sse(s: &[u8], ss: usize, r: &[u8], rs: usize, w: usize, h: usize) -> u64 {
    let mut acc = 0u64;
    for row in 0..h {
        for col in 0..w {
            let d = s[row * ss + col] as i32 - r[row * rs + col] as i32;
            acc += (d * d) as u64;
        }
    }
    acc
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
fn variance_and_sse_match_scalar_exactly() {
    const STRIDE: usize = 160;
    let src = plane(STRIDE * 160, 3);
    let rf = plane(STRIDE * 160, 7);

    // Includes widths that are not multiples of 16 so the scalar tail runs.
    for &w in &[4usize, 8, 12, 16, 20, 24, 32, 48, 64, 96, 128] {
        for &h in &[4usize, 8, 16, 32, 64, 128] {
            assert_eq!(
                svtav1_dsp::variance::variance(&src, STRIDE, w, h),
                scalar_variance(&src, STRIDE, w, h),
                "variance mismatch at {w}x{h}"
            );
            assert_eq!(
                svtav1_dsp::variance::sse(&src, STRIDE, &rf, STRIDE, w, h),
                scalar_sse(&src, STRIDE, &rf, STRIDE, w, h),
                "sse mismatch at {w}x{h}"
            );
        }
    }
}

#[test]
fn variance_and_sse_survive_maximum_accumulation() {
    // All-255 vs all-0 is the worst case for every accumulator: each squared
    // difference is 65025, and a 128x128 block sums to 1,065,369,600 — which
    // overflows u16 by ~16000x and would expose a missing drain.
    const STRIDE: usize = 128;
    let hi = vec![255u8; STRIDE * 128];
    let lo = vec![0u8; STRIDE * 128];
    for &(w, h) in &[(16usize, 16usize), (64, 64), (128, 128)] {
        assert_eq!(
            svtav1_dsp::variance::sse(&hi, STRIDE, &lo, STRIDE, w, h),
            65025u64 * (w * h) as u64,
            "sse worst case wrong at {w}x{h}"
        );
        assert_eq!(
            svtav1_dsp::variance::variance(&hi, STRIDE, w, h),
            scalar_variance(&hi, STRIDE, w, h),
            "variance worst case wrong at {w}x{h}"
        );
    }
}
