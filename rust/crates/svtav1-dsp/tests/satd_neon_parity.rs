//! NEON `satd_8x8` must equal the scalar version exactly.
//!
//! The NEON path deliberately reorders the transform — it runs BOTH Hadamard
//! passes vertically with a transpose between, where the scalar runs rows then
//! columns. That is valid because a 2D separable Hadamard commutes and the
//! result is an absolute sum over all 64 coefficients, but "valid in theory"
//! is exactly the kind of claim that needs pinning: a wrong transpose produces
//! a plausible-looking SATD that silently changes mode decisions.

fn scalar_satd_8x8(src: &[u8], ss: usize, rf: &[u8], rs: usize) -> u32 {
    let mut diff = [0i32; 64];
    for row in 0..8 {
        for col in 0..8 {
            diff[row * 8 + col] = src[row * ss + col] as i32 - rf[row * rs + col] as i32;
        }
    }
    let mut tmp = [0i32; 64];
    for row in 0..8 {
        let i = row * 8;
        let d = &diff[i..i + 8];
        let (a0, a1, a2, a3) = (d[0] + d[4], d[1] + d[5], d[2] + d[6], d[3] + d[7]);
        let (a4, a5, a6, a7) = (d[0] - d[4], d[1] - d[5], d[2] - d[6], d[3] - d[7]);
        let (b0, b1, b2, b3) = (a0 + a2, a1 + a3, a0 - a2, a1 - a3);
        let (b4, b5, b6, b7) = (a4 + a6, a5 + a7, a4 - a6, a5 - a7);
        tmp[i] = b0 + b1;
        tmp[i + 1] = b0 - b1;
        tmp[i + 2] = b2 + b3;
        tmp[i + 3] = b2 - b3;
        tmp[i + 4] = b4 + b5;
        tmp[i + 5] = b4 - b5;
        tmp[i + 6] = b6 + b7;
        tmp[i + 7] = b6 - b7;
    }
    let mut satd = 0u32;
    for col in 0..8 {
        let (a0, a1) = (tmp[col] + tmp[32 + col], tmp[8 + col] + tmp[40 + col]);
        let (a2, a3) = (tmp[16 + col] + tmp[48 + col], tmp[24 + col] + tmp[56 + col]);
        let (a4, a5) = (tmp[col] - tmp[32 + col], tmp[8 + col] - tmp[40 + col]);
        let (a6, a7) = (tmp[16 + col] - tmp[48 + col], tmp[24 + col] - tmp[56 + col]);
        let (b0, b1, b2, b3) = (a0 + a2, a1 + a3, a0 - a2, a1 - a3);
        let (b4, b5, b6, b7) = (a4 + a6, a5 + a7, a4 - a6, a5 - a7);
        for v in [
            b0 + b1,
            b0 - b1,
            b2 + b3,
            b2 - b3,
            b4 + b5,
            b4 - b5,
            b6 + b7,
            b6 - b7,
        ] {
            satd += v.unsigned_abs();
        }
    }
    (satd + 2) >> 2
}

#[test]
fn satd_8x8_matches_scalar_on_random_and_extreme_blocks() {
    let mut s = 0x9e37_79b9u32;
    let mut next = move || {
        s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (s >> 24) as u8
    };
    const STRIDE: usize = 24;

    // Random blocks.
    for _ in 0..5000 {
        let src: Vec<u8> = (0..STRIDE * 8).map(|_| next()).collect();
        let rf: Vec<u8> = (0..STRIDE * 8).map(|_| next()).collect();
        assert_eq!(
            svtav1_dsp::hadamard::satd_8x8(&src, STRIDE, &rf, STRIDE),
            scalar_satd_8x8(&src, STRIDE, &rf, STRIDE),
        );
    }

    // Maximum amplitude: every residual +/-255, which drives coefficients to
    // +/-16320 and would overflow i16 if the lane width were wrong.
    for &(a, b) in &[(255u8, 0u8), (0, 255)] {
        let src = vec![a; STRIDE * 8];
        let rf = vec![b; STRIDE * 8];
        assert_eq!(
            svtav1_dsp::hadamard::satd_8x8(&src, STRIDE, &rf, STRIDE),
            scalar_satd_8x8(&src, STRIDE, &rf, STRIDE),
        );
    }

    // Alternating pattern: maximizes the high-frequency coefficients, which is
    // where a wrong transpose shows up (a transposed error is invisible on
    // symmetric input).
    let mut src = vec![0u8; STRIDE * 8];
    let mut rf = vec![0u8; STRIDE * 8];
    for row in 0..8 {
        for col in 0..8 {
            src[row * STRIDE + col] = if (row + col) % 2 == 0 { 255 } else { 0 };
            rf[row * STRIDE + col] = if row % 2 == 0 { 200 } else { 30 };
        }
    }
    assert_eq!(
        svtav1_dsp::hadamard::satd_8x8(&src, STRIDE, &rf, STRIDE),
        scalar_satd_8x8(&src, STRIDE, &rf, STRIDE),
    );
}
