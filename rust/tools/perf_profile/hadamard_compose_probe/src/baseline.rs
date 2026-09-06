use archmage::prelude::*;
/// C `hadamard_col8`: one 8-point Hadamard butterfly over strided input.
/// Output order is the C permutation, not natural order.
fn hadamard_col8(src_diff: &[i16], src_stride: usize, coeff: &mut [i16; 8]) {
    let s = |i: usize| src_diff[i * src_stride] as i32;
    let b0 = s(0) + s(1);
    let b1 = s(0) - s(1);
    let b2 = s(2) + s(3);
    let b3 = s(2) - s(3);
    let b4 = s(4) + s(5);
    let b5 = s(4) - s(5);
    let b6 = s(6) + s(7);
    let b7 = s(6) - s(7);

    let c0 = b0 + b2;
    let c1 = b1 + b3;
    let c2 = b0 - b2;
    let c3 = b1 - b3;
    let c4 = b4 + b6;
    let c5 = b5 + b7;
    let c6 = b4 - b6;
    let c7 = b5 - b7;

    coeff[0] = (c0 + c4) as i16;
    coeff[7] = (c1 + c5) as i16;
    coeff[3] = (c2 + c6) as i16;
    coeff[4] = (c3 + c7) as i16;
    coeff[2] = (c0 - c4) as i16;
    coeff[6] = (c1 - c5) as i16;
    coeff[1] = (c2 - c6) as i16;
    coeff[5] = (c3 - c7) as i16;
}

/// C `hadamard_col4` (picture_operators_c.c:72): 4-point butterfly with a
/// `>> 1` on the first stage.
fn hadamard_col4(src_diff: &[i16], src_stride: usize, coeff: &mut [i16; 4]) {
    let b0 = (src_diff[0] + src_diff[src_stride]) >> 1;
    let b1 = (src_diff[0] - src_diff[src_stride]) >> 1;
    let b2 = (src_diff[2 * src_stride] + src_diff[3 * src_stride]) >> 1;
    let b3 = (src_diff[2 * src_stride] - src_diff[3 * src_stride]) >> 1;
    coeff[0] = b0 + b2;
    coeff[1] = b1 + b3;
    coeff[2] = b0 - b2;
    coeff[3] = b1 - b3;
}

/// C `svt_aom_hadamard_4x4_c` (picture_operators_c.c:85): 2D 4x4 Hadamard
/// (column pass, row pass over the transposed intermediate, then the
/// extra transpose matching the SSE2 kernel's output order).
pub fn aom_hadamard_4x4(src_diff: &[i16], src_stride: usize, coeff: &mut [i32]) {
    let mut buffer = [0i16; 16];
    let mut buffer2 = [0i16; 16];
    for idx in 0..4 {
        let mut out = [0i16; 4];
        hadamard_col4(&src_diff[idx..], src_stride, &mut out);
        buffer[idx * 4..idx * 4 + 4].copy_from_slice(&out);
    }
    for idx in 0..4 {
        let mut out = [0i16; 4];
        hadamard_col4(&buffer[idx..], 4, &mut out);
        buffer2[idx * 4..idx * 4 + 4].copy_from_slice(&out);
    }
    for i in 0..4 {
        for j in 0..4 {
            coeff[i * 4 + j] = buffer2[j * 4 + i] as i32;
        }
    }
}

/// C `svt_aom_hadamard_8x8_c`: 2D 8x8 Hadamard of an int16 residual block
/// (stride `src_stride`) into 64 int32 coefficients. No scaling.
///
/// C dispatches this through RTCD to `svt_aom_hadamard_8x8_neon`
/// (`common_dsp_rtcd.c:1603`); it is also the inner kernel of the 16x16 and
/// 32x32 forms below, so it carries the whole MDS0 Hadamard cost — 7.5 % of the
/// port's frame at 512x512 preset 2 and 4.0 % at preset 6
/// (`benchmarks/perf_class_attrib_2026-08-13.tsv`).
pub fn aom_hadamard_8x8(src_diff: &[i16], src_stride: usize, coeff: &mut [i32]) {
    incant!(
        aom_hadamard_8x8_impl(src_diff, src_stride, coeff),
        [v3, neon, scalar]
    )
}

fn aom_hadamard_8x8_impl_scalar(
    _token: ScalarToken,
    src_diff: &[i16],
    src_stride: usize,
    coeff: &mut [i32],
) {
    aom_hadamard_8x8_core(src_diff, src_stride, coeff)
}

// C picture_operators_c.c:121-176: preserve the positional coefficient
// permutation. Wrapping i16 addition/subtraction commutes with C's truncation
// after each pass, including full-range i16 inputs. The transpose uses
// existing safe unpack intrinsics; no new Archmage primitive is needed.
#[cfg(target_arch = "x86_64")]
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

/// Both passes run VERTICALLY (one lane per column, no cross-lane work) with a
/// single 8x8 transpose between them, exactly like [`satd_8x8_impl_neon`] —
/// except that the butterfly here must reproduce [`hadamard_col8`]'s PERMUTED
/// output order, because these coefficients are positional (the SATD kernel
/// only sums them, so any order does).
///
/// Exactness: `hadamard_col8` computes in `i32` and truncates to `i16` on
/// store. Every operation is an add or a subtract, and truncation to 16 bits is
/// a ring homomorphism for `+`/`-`, so doing the whole butterfly in wrapping
/// `i16` lanes (`vaddq_s16`/`vsubq_s16`) yields bit-identical results — this is
/// NOT an "in range so it does not matter" argument, it holds for any input,
/// including the 10-bit residuals of the bd10 fast loop.
#[cfg(target_arch = "aarch64")]
#[arcane]
fn aom_hadamard_8x8_impl_neon(
    token: NeonToken,
    src_diff: &[i16],
    src_stride: usize,
    coeff: &mut [i32],
) {
    let mut d = [vdupq_n_s16(0); 8];
    for (row, slot) in d.iter_mut().enumerate() {
        let r: &[i16; 8] = src_diff[row * src_stride..row * src_stride + 8]
            .try_into()
            .unwrap();
        *slot = vld1q_s16(r);
    }
    // Pass 1 gives o[k] lane j = coefficient k of column j (C's
    // `buffer[j * 8 + k]`). Pass 2 reads `buffer[idx + i * 8]`, i.e. it needs
    // lane `idx` to hold `o[idx][i]` — the transpose of that.
    let o = hadamard_col8_vertical(token, d);
    let u = transpose8x8_s16(token, o);
    // C stores `buffer2` straight through (`coeff[idx] = buffer2[idx]`), and
    // `q[k]` lane idx is `buffer2[idx * 8 + k]` — so output row idx is the
    // vector across k, i.e. one more transpose.
    let q = transpose8x8_s16(token, hadamard_col8_vertical(token, u));
    for (i, v) in q.iter().enumerate() {
        let dst: &mut [i32; 8] = (&mut coeff[i * 8..i * 8 + 8]).try_into().unwrap();
        vst1q_s32(
            (&mut dst[0..4]).try_into().unwrap(),
            vmovl_s16(vget_low_s16(*v)),
        );
        vst1q_s32((&mut dst[4..8]).try_into().unwrap(), vmovl_high_s16(*v));
    }
}

/// [`hadamard_col8`]'s butterfly applied VERTICALLY across eight vectors, with
/// its permuted output order. Distinct from [`hadamard8_vertical`], which is
/// the same transform in natural order (fine for SATD, wrong for coefficients).
#[cfg(target_arch = "aarch64")]
#[rite]
fn hadamard_col8_vertical(_token: NeonToken, s: [int16x8_t; 8]) -> [int16x8_t; 8] {
    let b0 = vaddq_s16(s[0], s[1]);
    let b1 = vsubq_s16(s[0], s[1]);
    let b2 = vaddq_s16(s[2], s[3]);
    let b3 = vsubq_s16(s[2], s[3]);
    let b4 = vaddq_s16(s[4], s[5]);
    let b5 = vsubq_s16(s[4], s[5]);
    let b6 = vaddq_s16(s[6], s[7]);
    let b7 = vsubq_s16(s[6], s[7]);

    let c0 = vaddq_s16(b0, b2);
    let c1 = vaddq_s16(b1, b3);
    let c2 = vsubq_s16(b0, b2);
    let c3 = vsubq_s16(b1, b3);
    let c4 = vaddq_s16(b4, b6);
    let c5 = vaddq_s16(b5, b7);
    let c6 = vsubq_s16(b4, b6);
    let c7 = vsubq_s16(b5, b7);

    // coeff[0]=c0+c4, [7]=c1+c5, [3]=c2+c6, [4]=c3+c7,
    // coeff[2]=c0-c4, [6]=c1-c5, [1]=c2-c6, [5]=c3-c7
    [
        vaddq_s16(c0, c4),
        vsubq_s16(c2, c6),
        vsubq_s16(c0, c4),
        vaddq_s16(c2, c6),
        vaddq_s16(c3, c7),
        vsubq_s16(c3, c7),
        vsubq_s16(c1, c5),
        vaddq_s16(c1, c5),
    ]
}

fn aom_hadamard_8x8_core(src_diff: &[i16], src_stride: usize, coeff: &mut [i32]) {
    let mut buffer = [0i16; 64];
    let mut buffer2 = [0i16; 64];
    // Column pass: one butterfly per column, walking columns left→right.
    for idx in 0..8 {
        let col = &src_diff[idx..];
        let mut out = [0i16; 8];
        hadamard_col8(col, src_stride, &mut out);
        buffer[idx * 8..idx * 8 + 8].copy_from_slice(&out);
    }
    // Row pass over the transposed intermediate.
    for idx in 0..8 {
        let mut out = [0i16; 8];
        hadamard_col8(&buffer[idx..], 8, &mut out);
        buffer2[idx * 8..idx * 8 + 8].copy_from_slice(&out);
    }
    for idx in 0..64 {
        coeff[idx] = buffer2[idx] as i32;
    }
}

// ---------------------------------------------------------------------------
// 16x16 / 32x32: ported from the AVX2 kernels, NOT the `_c` references.
//
// `svt_aom_hadamard_{16x16,32x32}` are RTCD function POINTERS that the encoder
// binds to the AVX2 implementations on any AVX2 host
// (`SET_AVX2(svt_aom_hadamard_32x32, _c, _avx2)`, common_dsp_rtcd.c:1047-1048),
// and the AVX2 kernels are NOT equivalent to the `_c` ones once the residual
// leaves the 8-bit range they were written for (their own comment: "src_diff:
// 9 bit, dynamic range [-255, 255]"):
//
//   * `_c` carries the 8x8 sub-results into the 16x16 cross-combine as
//     `int32_t` and the 16x16 sub-results into the 32x32 combine as `int32_t`;
//     nothing after the 8x8 stage can wrap.
//   * `_avx2` keeps BOTH of those stages in 16-bit lanes: the 16x16 combine is
//     `_mm256_{add,sub}_epi16` + `_mm256_srai_epi16` (wrapping), and
//     `svt_aom_hadamard_32x32_avx2` buffers its four 16x16 sub-transforms in an
//     `int16_t temp_coeff[32*32]` (`is_final = 0`,
//     pic_operators_intrin_avx2.c:1721-1732) before sign-extending to 32-bit,
//     doing the `>> 2` in 32-bit, SATURATING back to 16-bit
//     (`_mm256_packs_epi32`) and finishing with wrapping 16-bit add/sub.
//
// At 8-bit residuals the 16x16 stage spans [-32640, 32640] and the post-shift
// 32x32 operands span [-16320, 16320], so no wrap or saturation is reachable
// and the two kernels agree bit-for-bit — which is why the 8-bit identity
// gates are unaffected by porting the AVX2 semantics. At 10-bit residuals
// (the bd10 MD fast loop, task #94) the 16x16 stage reaches ~+/-130560 and the
// AVX2 kernel wraps where `_c` does not, so ONLY the AVX2 form reproduces the
// encoder's SATD. Pinned against both references in tests/c_parity_hadamard.rs
// (`_c` over the 8-bit range, `_avx2` over the 8-bit AND 10-bit ranges).
// ---------------------------------------------------------------------------

/// `svt_aom_hadamard_16x16_avx2`: four 8x8 sub-transforms + a cross-combine
/// carried in WRAPPING 16-bit lanes (`_mm256_{add,sub}_epi16`,
/// `_mm256_srai_epi16`), widened to `int32` on store (`store_tran_low`).
pub fn aom_hadamard_16x16(src_diff: &[i16], src_stride: usize, coeff: &mut [i32]) {
    for idx in 0..4usize {
        let off = (idx >> 1) * 8 * src_stride + (idx & 1) * 8;
        aom_hadamard_8x8(&src_diff[off..], src_stride, &mut coeff[idx * 64..]);
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

/// `svt_aom_hadamard_32x32_avx2`: four 16x16 sub-transforms buffered as
/// `int16` (`is_final = 0`), then sign-extended to 32-bit for the pairwise
/// sum/difference and `>> 2`, SATURATED back to 16-bit (`_mm256_packs_epi32`)
/// and combined with wrapping 16-bit add/sub before the 32-bit store.
pub fn aom_hadamard_32x32(src_diff: &[i16], src_stride: usize, coeff: &mut [i32]) {
    for idx in 0..4usize {
        let off = (idx >> 1) * 16 * src_stride + (idx & 1) * 16;
        aom_hadamard_16x16(&src_diff[off..], src_stride, &mut coeff[idx * 256..]);
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
