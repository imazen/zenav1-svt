#![forbid(unsafe_code)]
use archmage::prelude::*;
pub mod baseline;
type HadamardV3 = magetypes::simd::generic::i16x8<X64V3Token>;

#[cfg(target_arch = "x86_64")]
#[rite]
fn hadamard_col8_vertical_v3(_token: X64V3Token, s: [HadamardV3; 8]) -> [HadamardV3; 8] {
    let b0 = s[0] + s[1];
    let b1 = s[0] - s[1];
    let b2 = s[2] + s[3];
    let b3 = s[2] - s[3];
    let b4 = s[4] + s[5];
    let b5 = s[4] - s[5];
    let b6 = s[6] + s[7];
    let b7 = s[6] - s[7];
    let c0 = b0 + b2;
    let c1 = b1 + b3;
    let c2 = b0 - b2;
    let c3 = b1 - b3;
    let c4 = b4 + b6;
    let c5 = b5 + b7;
    let c6 = b4 - b6;
    let c7 = b5 - b7;
    [
        c0 + c4,
        c2 - c6,
        c0 - c4,
        c2 + c6,
        c3 + c7,
        c3 - c7,
        c1 - c5,
        c1 + c5,
    ]
}

#[cfg(target_arch = "x86_64")]
#[rite]
fn hadamard_transpose8_v3(token: X64V3Token, v: [HadamardV3; 8]) -> [HadamardV3; 8] {
    let x = v.map(HadamardV3::raw);
    let a0 = _mm_unpacklo_epi16(x[0], x[1]);
    let a1 = _mm_unpackhi_epi16(x[0], x[1]);
    let a2 = _mm_unpacklo_epi16(x[2], x[3]);
    let a3 = _mm_unpackhi_epi16(x[2], x[3]);
    let a4 = _mm_unpacklo_epi16(x[4], x[5]);
    let a5 = _mm_unpackhi_epi16(x[4], x[5]);
    let a6 = _mm_unpacklo_epi16(x[6], x[7]);
    let a7 = _mm_unpackhi_epi16(x[6], x[7]);
    let b0 = _mm_unpacklo_epi32(a0, a2);
    let b1 = _mm_unpackhi_epi32(a0, a2);
    let b2 = _mm_unpacklo_epi32(a1, a3);
    let b3 = _mm_unpackhi_epi32(a1, a3);
    let b4 = _mm_unpacklo_epi32(a4, a6);
    let b5 = _mm_unpackhi_epi32(a4, a6);
    let b6 = _mm_unpacklo_epi32(a5, a7);
    let b7 = _mm_unpackhi_epi32(a5, a7);
    [
        _mm_unpacklo_epi64(b0, b4),
        _mm_unpackhi_epi64(b0, b4),
        _mm_unpacklo_epi64(b1, b5),
        _mm_unpackhi_epi64(b1, b5),
        _mm_unpacklo_epi64(b2, b6),
        _mm_unpackhi_epi64(b2, b6),
        _mm_unpacklo_epi64(b3, b7),
        _mm_unpackhi_epi64(b3, b7),
    ]
    .map(|r| HadamardV3::from_m128i(token, r))
}

#[cfg(target_arch = "x86_64")]
#[arcane]
fn aom_hadamard_8x8_impl_v3(token: X64V3Token, input: &[i16], stride: usize, output: &mut [i32]) {
    let rows = core::array::from_fn(|r| {
        HadamardV3::load(token, input[r * stride..r * stride + 8].try_into().unwrap())
    });
    let first = hadamard_transpose8_v3(token, hadamard_col8_vertical_v3(token, rows));
    let result = hadamard_transpose8_v3(token, hadamard_col8_vertical_v3(token, first));
    let output: &mut [i32; 64] = (&mut output[..64]).try_into().unwrap();
    for (r, row) in result.into_iter().enumerate() {
        output[r * 8..r * 8 + 8].copy_from_slice(&row.to_array().map(i32::from));
    }
}

#[rite]
fn inner16(token: X64V3Token, src_diff: &[i16], src_stride: usize, coeff: &mut [i32]) {
    for idx in 0..4usize {
        let off = (idx >> 1) * 8 * src_stride + (idx & 1) * 8;
        aom_hadamard_8x8_impl_v3(token, &src_diff[off..], src_stride, &mut coeff[idx * 64..]);
    }
    for i in 0..64usize {
        // The 8x8 stage already produced int16-valued coefficients (C's
        // `buffer2` / the AVX2 `temp_coeff` are both int16), so reading them
        // back as i16 is lossless and matches the AVX2 lane width.
        let a0 = coeff[i] as i16;
        let a1 = coeff[i + 64] as i16;
        let a2 = coeff[i + 128] as i16;
        let a3 = coeff[i + 192] as i16;
        let b0 = a0.wrapping_add(a1) >> 1;
        let b1 = a0.wrapping_sub(a1) >> 1;
        let b2 = a2.wrapping_add(a3) >> 1;
        let b3 = a2.wrapping_sub(a3) >> 1;
        coeff[i] = b0.wrapping_add(b2) as i32;
        coeff[i + 64] = b1.wrapping_add(b3) as i32;
        coeff[i + 128] = b0.wrapping_sub(b2) as i32;
        coeff[i + 192] = b1.wrapping_sub(b3) as i32;
    }
}

#[rite]
fn inner32(token: X64V3Token, src_diff: &[i16], src_stride: usize, coeff: &mut [i32]) {
    for idx in 0..4usize {
        let off = (idx >> 1) * 16 * src_stride + (idx & 1) * 16;
        inner16(token, &src_diff[off..], src_stride, &mut coeff[idx * 256..]);
    }
    for i in 0..256usize {
        // `temp_coeff` is int16: the 16x16 stage is read back through 16-bit
        // lanes and sign-extended (the AVX2 `sign_extend_16bit_to_32bit`).
        let a0 = coeff[i] as i16 as i32;
        let a1 = coeff[i + 256] as i16 as i32;
        let a2 = coeff[i + 512] as i16 as i32;
        let a3 = coeff[i + 768] as i16 as i32;
        // 32-bit add/sub then arithmetic `>> 2` (`_mm256_srai_epi32`).
        let b0 = (a0 + a1) >> 2;
        let b1 = (a0 - a1) >> 2;
        let b2 = (a2 + a3) >> 2;
        let b3 = (a2 - a3) >> 2;
        // `_mm256_packs_epi32`: SATURATING 32 -> 16 narrowing.
        let sat = |v: i32| v.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        let (b0, b1, b2, b3) = (sat(b0), sat(b1), sat(b2), sat(b3));
        // `_mm256_{add,sub}_epi16`: WRAPPING 16-bit, then sign-extended store.
        coeff[i] = b0.wrapping_add(b2) as i32;
        coeff[i + 256] = b1.wrapping_add(b3) as i32;
        coeff[i + 512] = b0.wrapping_sub(b2) as i32;
        coeff[i + 768] = b1.wrapping_sub(b3) as i32;
    }
}

pub fn candidate16(src: &[i16], stride: usize, coeff: &mut [i32]) {
    incant!(dispatch16(src, stride, coeff), [v3, scalar]);
}
#[arcane]
fn dispatch16_v3(token: X64V3Token, src: &[i16], stride: usize, coeff: &mut [i32]) {
    inner16(token, src, stride, coeff);
}
fn dispatch16_scalar(_token: ScalarToken, src: &[i16], stride: usize, coeff: &mut [i32]) {
    baseline::aom_hadamard_16x16(src, stride, coeff);
}
pub fn candidate32(src: &[i16], stride: usize, coeff: &mut [i32]) {
    incant!(dispatch32(src, stride, coeff), [v3, scalar]);
}
#[arcane]
fn dispatch32_v3(token: X64V3Token, src: &[i16], stride: usize, coeff: &mut [i32]) {
    inner32(token, src, stride, coeff);
}
fn dispatch32_scalar(_token: ScalarToken, src: &[i16], stride: usize, coeff: &mut [i32]) {
    baseline::aom_hadamard_32x32(src, stride, coeff);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn candidate_matches_port_and_c() {
        let _lock = archmage::testing::lock_token_testing();
        X64V3Token::summon().expect("AVX2 oracle");
        let mut seed = 987654321u32;
        let mut count = 0;
        for n in [16, 32] {
            for stride in [n, n + 7, n * 2] {
                for pattern in 0..100 {
                    let mut input = vec![0i16; stride * n + 3];
                    for (i, v) in input.iter_mut().enumerate() {
                        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                        *v = match pattern {
                            0 => 0,
                            1 => 255,
                            2 => -255,
                            3 => 1023,
                            4 => -1023,
                            5 => i16::MIN,
                            6 => i16::MAX,
                            7 => {
                                if i % 2 == 0 {
                                    i16::MIN
                                } else {
                                    i16::MAX
                                }
                            }
                            _ => seed as i16,
                        };
                    }
                    let mut c = vec![0; n * n];
                    let mut b = vec![777; n * n + 9];
                    let mut a = b.clone();
                    svtav1_cref::hadamard_avx2(n, &input[3..], stride, &mut c);
                    if n == 16 {
                        baseline::aom_hadamard_16x16(&input[3..], stride, &mut b[3..3 + n * n]);
                        candidate16(&input[3..], stride, &mut a[3..3 + n * n]);
                    } else {
                        baseline::aom_hadamard_32x32(&input[3..], stride, &mut b[3..3 + n * n]);
                        candidate32(&input[3..], stride, &mut a[3..3 + n * n]);
                    }
                    assert_eq!(a, b, "positional n={n} stride={stride} pattern={pattern}");
                    let mut sorted = a[3..3 + n * n].to_vec();
                    sorted.sort_unstable();
                    c.sort_unstable();
                    assert_eq!(
                        sorted, c,
                        "C multiset n={n} stride={stride} pattern={pattern}"
                    );
                    count += 1;
                }
            }
        }
        assert_eq!(count, 600);
    }
}
