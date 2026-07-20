// Vectorized 1D DCT kernels — each processes 8 independent columns (or rows) in
// the 8 lanes of an `i32x8`. Transcribed op-for-op from the scalar kernels in
// `inv_txfm.rs` / `fwd_txfm.rs`: `a + b` → `_mm256_add_epi32`, `a - b` →
// `_mm256_sub_epi32`, `half_btf(w0,x,w1,y,bit)` → [`hbtf`], `clamp_value` →
// [`clampv`]. Same operations, same order → bit-identical (see module docs).
//
// Included into `mod v3`; `crate::fwd_txfm::COSPI` (Q12) is the inverse table;
// the forward selects `cospi_arr(cos_bit)` per pass.

use crate::fwd_txfm::COSPI;

/// `splat(cospi[k])`.
macro_rules! c {
    ($t:expr, $cospi:expr, $k:expr) => {
        splat($t, $cospi[$k])
    };
}
/// `splat(-cospi[k])`.
macro_rules! cn {
    ($t:expr, $cospi:expr, $k:expr) => {
        splat($t, -$cospi[$k])
    };
}
macro_rules! add {
    ($a:expr, $b:expr) => {
        _mm256_add_epi32($a, $b)
    };
}
macro_rules! sub {
    ($a:expr, $b:expr) => {
        _mm256_sub_epi32($a, $b)
    };
}
/// `-x` across 8 lanes (`0 - x`). Used by the ADST input/output permutations.
macro_rules! neg {
    ($a:expr) => {
        _mm256_sub_epi32(_mm256_setzero_si256(), $a)
    };
}

// ---------------------------------------------------------------------------
// 8-point inverse DCT (svt_av1_idct8_new / inv_txfm.rs::idct8)
// ---------------------------------------------------------------------------
#[rite]
pub(super) fn idct8_x8(
    t: Desktop64,
    inp: &[__m256i; 8],
    out: &mut [__m256i; 8],
    rnd: __m256i,
    sh: __m128i,
    lo: __m256i,
    hi: __m256i,
) {
    let cospi = &COSPI;
    let cl = |v| clampv(t, v, lo, hi);

    // stage 1: permutation
    let s1 = [
        inp[0], inp[4], inp[2], inp[6], inp[1], inp[5], inp[3], inp[7],
    ];
    // stage 2
    let mut s2 = s1;
    s2[4] = hbtf(t, c!(t, cospi, 56), s1[4], cn!(t, cospi, 8), s1[7], rnd, sh);
    s2[5] = hbtf(t, c!(t, cospi, 24), s1[5], cn!(t, cospi, 40), s1[6], rnd, sh);
    s2[6] = hbtf(t, c!(t, cospi, 40), s1[5], c!(t, cospi, 24), s1[6], rnd, sh);
    s2[7] = hbtf(t, c!(t, cospi, 8), s1[4], c!(t, cospi, 56), s1[7], rnd, sh);
    // stage 3
    let mut s3 = s2;
    s3[0] = hbtf(t, c!(t, cospi, 32), s2[0], c!(t, cospi, 32), s2[1], rnd, sh);
    s3[1] = hbtf(t, c!(t, cospi, 32), s2[0], cn!(t, cospi, 32), s2[1], rnd, sh);
    s3[2] = hbtf(t, c!(t, cospi, 48), s2[2], cn!(t, cospi, 16), s2[3], rnd, sh);
    s3[3] = hbtf(t, c!(t, cospi, 16), s2[2], c!(t, cospi, 48), s2[3], rnd, sh);
    s3[4] = cl(add!(s2[4], s2[5]));
    s3[5] = cl(sub!(s2[4], s2[5]));
    s3[6] = cl(sub!(s2[7], s2[6]));
    s3[7] = cl(add!(s2[6], s2[7]));
    // stage 4
    let mut s4 = s3;
    s4[0] = cl(add!(s3[0], s3[3]));
    s4[1] = cl(add!(s3[1], s3[2]));
    s4[2] = cl(sub!(s3[1], s3[2]));
    s4[3] = cl(sub!(s3[0], s3[3]));
    s4[5] = hbtf(t, cn!(t, cospi, 32), s3[5], c!(t, cospi, 32), s3[6], rnd, sh);
    s4[6] = hbtf(t, c!(t, cospi, 32), s3[5], c!(t, cospi, 32), s3[6], rnd, sh);
    // stage 5: final combine
    out[0] = cl(add!(s4[0], s4[7]));
    out[1] = cl(add!(s4[1], s4[6]));
    out[2] = cl(add!(s4[2], s4[5]));
    out[3] = cl(add!(s4[3], s4[4]));
    out[4] = cl(sub!(s4[3], s4[4]));
    out[5] = cl(sub!(s4[2], s4[5]));
    out[6] = cl(sub!(s4[1], s4[6]));
    out[7] = cl(sub!(s4[0], s4[7]));
}

// ---------------------------------------------------------------------------
// 8-point forward DCT (svt_av1_fdct8_new / fwd_txfm.rs::fdct8). No clamps.
// ---------------------------------------------------------------------------
#[rite]
pub(super) fn fdct8_x8(t: Desktop64, inp: &[__m256i; 8], out: &mut [__m256i; 8], cos_bit: i8) {
    let cospi = cospi_arr(cos_bit);
    let rnd = splat(t, 1 << (cos_bit as u32 - 1));
    let sh = _mm_cvtsi32_si128(cos_bit as i32);

    // stage 1
    let s1 = [
        add!(inp[0], inp[7]),
        add!(inp[1], inp[6]),
        add!(inp[2], inp[5]),
        add!(inp[3], inp[4]),
        sub!(inp[3], inp[4]),
        sub!(inp[2], inp[5]),
        sub!(inp[1], inp[6]),
        sub!(inp[0], inp[7]),
    ];
    // stage 2
    let mut s2 = s1;
    s2[0] = add!(s1[0], s1[3]);
    s2[1] = add!(s1[1], s1[2]);
    s2[2] = sub!(s1[1], s1[2]);
    s2[3] = sub!(s1[0], s1[3]);
    s2[5] = hbtf(t, cn!(t, cospi, 32), s1[5], c!(t, cospi, 32), s1[6], rnd, sh);
    s2[6] = hbtf(t, c!(t, cospi, 32), s1[6], c!(t, cospi, 32), s1[5], rnd, sh);
    // stage 3
    let mut s3 = s2;
    s3[0] = hbtf(t, c!(t, cospi, 32), s2[0], c!(t, cospi, 32), s2[1], rnd, sh);
    s3[1] = hbtf(t, cn!(t, cospi, 32), s2[1], c!(t, cospi, 32), s2[0], rnd, sh);
    s3[2] = hbtf(t, c!(t, cospi, 48), s2[2], c!(t, cospi, 16), s2[3], rnd, sh);
    s3[3] = hbtf(t, c!(t, cospi, 48), s2[3], cn!(t, cospi, 16), s2[2], rnd, sh);
    s3[4] = add!(s2[4], s2[5]);
    s3[5] = sub!(s2[4], s2[5]);
    s3[6] = sub!(s2[7], s2[6]);
    s3[7] = add!(s2[7], s2[6]);
    // stage 4
    let mut s4 = s3;
    s4[4] = hbtf(t, c!(t, cospi, 56), s3[4], c!(t, cospi, 8), s3[7], rnd, sh);
    s4[5] = hbtf(t, c!(t, cospi, 24), s3[5], c!(t, cospi, 40), s3[6], rnd, sh);
    s4[6] = hbtf(t, c!(t, cospi, 24), s3[6], cn!(t, cospi, 40), s3[5], rnd, sh);
    s4[7] = hbtf(t, c!(t, cospi, 56), s3[7], cn!(t, cospi, 8), s3[4], rnd, sh);
    // stage 5: output permutation
    out[0] = s4[0];
    out[1] = s4[4];
    out[2] = s4[2];
    out[3] = s4[6];
    out[4] = s4[1];
    out[5] = s4[5];
    out[6] = s4[3];
    out[7] = s4[7];
}

// ---------------------------------------------------------------------------
// 16-point inverse DCT (svt_av1_idct16_new / inv_txfm.rs::idct16)
// ---------------------------------------------------------------------------
#[rite]
pub(super) fn idct16_x8(
    t: Desktop64,
    inp: &[__m256i; 16],
    out: &mut [__m256i; 16],
    rnd: __m256i,
    sh: __m128i,
    lo: __m256i,
    hi: __m256i,
) {
    let cospi = &COSPI;
    let cl = |v| clampv(t, v, lo, hi);

    // stage 1: input permutation
    let s1 = [
        inp[0], inp[8], inp[4], inp[12], inp[2], inp[10], inp[6], inp[14], inp[1], inp[9], inp[5],
        inp[13], inp[3], inp[11], inp[7], inp[15],
    ];

    // stage 2
    let mut s2 = s1;
    s2[8] = hbtf(t, c!(t, cospi, 60), s1[8], cn!(t, cospi, 4), s1[15], rnd, sh);
    s2[9] = hbtf(t, c!(t, cospi, 28), s1[9], cn!(t, cospi, 36), s1[14], rnd, sh);
    s2[10] = hbtf(t, c!(t, cospi, 44), s1[10], cn!(t, cospi, 20), s1[13], rnd, sh);
    s2[11] = hbtf(t, c!(t, cospi, 12), s1[11], cn!(t, cospi, 52), s1[12], rnd, sh);
    s2[12] = hbtf(t, c!(t, cospi, 52), s1[11], c!(t, cospi, 12), s1[12], rnd, sh);
    s2[13] = hbtf(t, c!(t, cospi, 20), s1[10], c!(t, cospi, 44), s1[13], rnd, sh);
    s2[14] = hbtf(t, c!(t, cospi, 36), s1[9], c!(t, cospi, 28), s1[14], rnd, sh);
    s2[15] = hbtf(t, c!(t, cospi, 4), s1[8], c!(t, cospi, 60), s1[15], rnd, sh);

    // stage 3
    let mut s3 = s2;
    s3[4] = hbtf(t, c!(t, cospi, 56), s2[4], cn!(t, cospi, 8), s2[7], rnd, sh);
    s3[5] = hbtf(t, c!(t, cospi, 24), s2[5], cn!(t, cospi, 40), s2[6], rnd, sh);
    s3[6] = hbtf(t, c!(t, cospi, 40), s2[5], c!(t, cospi, 24), s2[6], rnd, sh);
    s3[7] = hbtf(t, c!(t, cospi, 8), s2[4], c!(t, cospi, 56), s2[7], rnd, sh);
    s3[8] = cl(add!(s2[8], s2[9]));
    s3[9] = cl(sub!(s2[8], s2[9]));
    s3[10] = cl(sub!(s2[11], s2[10]));
    s3[11] = cl(add!(s2[10], s2[11]));
    s3[12] = cl(add!(s2[12], s2[13]));
    s3[13] = cl(sub!(s2[12], s2[13]));
    s3[14] = cl(sub!(s2[15], s2[14]));
    s3[15] = cl(add!(s2[14], s2[15]));

    // stage 4
    let mut s4 = s3;
    s4[0] = hbtf(t, c!(t, cospi, 32), s3[0], c!(t, cospi, 32), s3[1], rnd, sh);
    s4[1] = hbtf(t, c!(t, cospi, 32), s3[0], cn!(t, cospi, 32), s3[1], rnd, sh);
    s4[2] = hbtf(t, c!(t, cospi, 48), s3[2], cn!(t, cospi, 16), s3[3], rnd, sh);
    s4[3] = hbtf(t, c!(t, cospi, 16), s3[2], c!(t, cospi, 48), s3[3], rnd, sh);
    s4[4] = cl(add!(s3[4], s3[5]));
    s4[5] = cl(sub!(s3[4], s3[5]));
    s4[6] = cl(sub!(s3[7], s3[6]));
    s4[7] = cl(add!(s3[6], s3[7]));
    s4[9] = hbtf(t, cn!(t, cospi, 16), s3[9], c!(t, cospi, 48), s3[14], rnd, sh);
    s4[10] = hbtf(t, cn!(t, cospi, 48), s3[10], cn!(t, cospi, 16), s3[13], rnd, sh);
    s4[13] = hbtf(t, cn!(t, cospi, 16), s3[10], c!(t, cospi, 48), s3[13], rnd, sh);
    s4[14] = hbtf(t, c!(t, cospi, 48), s3[9], c!(t, cospi, 16), s3[14], rnd, sh);

    // stage 5
    let mut s5 = s4;
    s5[0] = cl(add!(s4[0], s4[3]));
    s5[1] = cl(add!(s4[1], s4[2]));
    s5[2] = cl(sub!(s4[1], s4[2]));
    s5[3] = cl(sub!(s4[0], s4[3]));
    s5[5] = hbtf(t, cn!(t, cospi, 32), s4[5], c!(t, cospi, 32), s4[6], rnd, sh);
    s5[6] = hbtf(t, c!(t, cospi, 32), s4[5], c!(t, cospi, 32), s4[6], rnd, sh);
    s5[8] = cl(add!(s4[8], s4[11]));
    s5[9] = cl(add!(s4[9], s4[10]));
    s5[10] = cl(sub!(s4[9], s4[10]));
    s5[11] = cl(sub!(s4[8], s4[11]));
    s5[12] = cl(sub!(s4[15], s4[12]));
    s5[13] = cl(sub!(s4[14], s4[13]));
    s5[14] = cl(add!(s4[13], s4[14]));
    s5[15] = cl(add!(s4[12], s4[15]));

    // stage 6
    let mut s6 = s5;
    s6[0] = cl(add!(s5[0], s5[7]));
    s6[1] = cl(add!(s5[1], s5[6]));
    s6[2] = cl(add!(s5[2], s5[5]));
    s6[3] = cl(add!(s5[3], s5[4]));
    s6[4] = cl(sub!(s5[3], s5[4]));
    s6[5] = cl(sub!(s5[2], s5[5]));
    s6[6] = cl(sub!(s5[1], s5[6]));
    s6[7] = cl(sub!(s5[0], s5[7]));
    s6[10] = hbtf(t, cn!(t, cospi, 32), s5[10], c!(t, cospi, 32), s5[13], rnd, sh);
    s6[11] = hbtf(t, cn!(t, cospi, 32), s5[11], c!(t, cospi, 32), s5[12], rnd, sh);
    s6[12] = hbtf(t, c!(t, cospi, 32), s5[11], c!(t, cospi, 32), s5[12], rnd, sh);
    s6[13] = hbtf(t, c!(t, cospi, 32), s5[10], c!(t, cospi, 32), s5[13], rnd, sh);

    // stage 7: final combine
    out[0] = cl(add!(s6[0], s6[15]));
    out[1] = cl(add!(s6[1], s6[14]));
    out[2] = cl(add!(s6[2], s6[13]));
    out[3] = cl(add!(s6[3], s6[12]));
    out[4] = cl(add!(s6[4], s6[11]));
    out[5] = cl(add!(s6[5], s6[10]));
    out[6] = cl(add!(s6[6], s6[9]));
    out[7] = cl(add!(s6[7], s6[8]));
    out[8] = cl(sub!(s6[7], s6[8]));
    out[9] = cl(sub!(s6[6], s6[9]));
    out[10] = cl(sub!(s6[5], s6[10]));
    out[11] = cl(sub!(s6[4], s6[11]));
    out[12] = cl(sub!(s6[3], s6[12]));
    out[13] = cl(sub!(s6[2], s6[13]));
    out[14] = cl(sub!(s6[1], s6[14]));
    out[15] = cl(sub!(s6[0], s6[15]));
}

// ---------------------------------------------------------------------------
// 16-point forward DCT (svt_av1_fdct16_new / fwd_txfm.rs::fdct16). No clamps.
// `cos_bit` selects the cospi table row (13 for the col pass, 12 for the row
// pass at 16x16) and the rounding.
// ---------------------------------------------------------------------------
#[rite]
pub(super) fn fdct16_x8(
    t: Desktop64,
    inp: &[__m256i; 16],
    out: &mut [__m256i; 16],
    cos_bit: i8,
) {
    let cospi = cospi_arr(cos_bit);
    let rnd = splat(t, 1 << (cos_bit as u32 - 1));
    let sh = _mm_cvtsi32_si128(cos_bit as i32);

    // stage 1
    let mut s1 = [_mm256_setzero_si256(); 16];
    for i in 0..8 {
        s1[i] = add!(inp[i], inp[15 - i]);
        s1[15 - i] = sub!(inp[i], inp[15 - i]);
    }

    // stage 2
    let mut s2 = s1;
    s2[0] = add!(s1[0], s1[7]);
    s2[1] = add!(s1[1], s1[6]);
    s2[2] = add!(s1[2], s1[5]);
    s2[3] = add!(s1[3], s1[4]);
    s2[4] = sub!(s1[3], s1[4]);
    s2[5] = sub!(s1[2], s1[5]);
    s2[6] = sub!(s1[1], s1[6]);
    s2[7] = sub!(s1[0], s1[7]);
    s2[10] = hbtf(t, cn!(t, cospi, 32), s1[10], c!(t, cospi, 32), s1[13], rnd, sh);
    s2[11] = hbtf(t, cn!(t, cospi, 32), s1[11], c!(t, cospi, 32), s1[12], rnd, sh);
    s2[12] = hbtf(t, c!(t, cospi, 32), s1[12], c!(t, cospi, 32), s1[11], rnd, sh);
    s2[13] = hbtf(t, c!(t, cospi, 32), s1[13], c!(t, cospi, 32), s1[10], rnd, sh);

    // stage 3
    let mut s3 = s2;
    s3[0] = add!(s2[0], s2[3]);
    s3[1] = add!(s2[1], s2[2]);
    s3[2] = sub!(s2[1], s2[2]);
    s3[3] = sub!(s2[0], s2[3]);
    s3[5] = hbtf(t, cn!(t, cospi, 32), s2[5], c!(t, cospi, 32), s2[6], rnd, sh);
    s3[6] = hbtf(t, c!(t, cospi, 32), s2[6], c!(t, cospi, 32), s2[5], rnd, sh);
    s3[8] = add!(s2[8], s2[11]);
    s3[9] = add!(s2[9], s2[10]);
    s3[10] = sub!(s2[9], s2[10]);
    s3[11] = sub!(s2[8], s2[11]);
    s3[12] = sub!(s2[15], s2[12]);
    s3[13] = sub!(s2[14], s2[13]);
    s3[14] = add!(s2[14], s2[13]);
    s3[15] = add!(s2[15], s2[12]);

    // stage 4
    let mut s4 = s3;
    s4[0] = hbtf(t, c!(t, cospi, 32), s3[0], c!(t, cospi, 32), s3[1], rnd, sh);
    s4[1] = hbtf(t, cn!(t, cospi, 32), s3[1], c!(t, cospi, 32), s3[0], rnd, sh);
    s4[2] = hbtf(t, c!(t, cospi, 48), s3[2], c!(t, cospi, 16), s3[3], rnd, sh);
    s4[3] = hbtf(t, c!(t, cospi, 48), s3[3], cn!(t, cospi, 16), s3[2], rnd, sh);
    s4[4] = add!(s3[4], s3[5]);
    s4[5] = sub!(s3[4], s3[5]);
    s4[6] = sub!(s3[7], s3[6]);
    s4[7] = add!(s3[7], s3[6]);
    s4[9] = hbtf(t, cn!(t, cospi, 16), s3[9], c!(t, cospi, 48), s3[14], rnd, sh);
    s4[10] = hbtf(t, cn!(t, cospi, 48), s3[10], cn!(t, cospi, 16), s3[13], rnd, sh);
    s4[13] = hbtf(t, c!(t, cospi, 48), s3[13], cn!(t, cospi, 16), s3[10], rnd, sh);
    s4[14] = hbtf(t, c!(t, cospi, 16), s3[14], c!(t, cospi, 48), s3[9], rnd, sh);

    // stage 5
    let mut s5 = s4;
    s5[4] = hbtf(t, c!(t, cospi, 56), s4[4], c!(t, cospi, 8), s4[7], rnd, sh);
    s5[5] = hbtf(t, c!(t, cospi, 24), s4[5], c!(t, cospi, 40), s4[6], rnd, sh);
    s5[6] = hbtf(t, c!(t, cospi, 24), s4[6], cn!(t, cospi, 40), s4[5], rnd, sh);
    s5[7] = hbtf(t, c!(t, cospi, 56), s4[7], cn!(t, cospi, 8), s4[4], rnd, sh);
    s5[8] = add!(s4[8], s4[9]);
    s5[9] = sub!(s4[8], s4[9]);
    s5[10] = sub!(s4[11], s4[10]);
    s5[11] = add!(s4[11], s4[10]);
    s5[12] = add!(s4[12], s4[13]);
    s5[13] = sub!(s4[12], s4[13]);
    s5[14] = sub!(s4[15], s4[14]);
    s5[15] = add!(s4[15], s4[14]);

    // stage 6
    let mut s6 = s5;
    s6[8] = hbtf(t, c!(t, cospi, 60), s5[8], c!(t, cospi, 4), s5[15], rnd, sh);
    s6[9] = hbtf(t, c!(t, cospi, 28), s5[9], c!(t, cospi, 36), s5[14], rnd, sh);
    s6[10] = hbtf(t, c!(t, cospi, 44), s5[10], c!(t, cospi, 20), s5[13], rnd, sh);
    s6[11] = hbtf(t, c!(t, cospi, 12), s5[11], c!(t, cospi, 52), s5[12], rnd, sh);
    s6[12] = hbtf(t, c!(t, cospi, 12), s5[12], cn!(t, cospi, 52), s5[11], rnd, sh);
    s6[13] = hbtf(t, c!(t, cospi, 44), s5[13], cn!(t, cospi, 20), s5[10], rnd, sh);
    s6[14] = hbtf(t, c!(t, cospi, 28), s5[14], cn!(t, cospi, 36), s5[9], rnd, sh);
    s6[15] = hbtf(t, c!(t, cospi, 60), s5[15], cn!(t, cospi, 4), s5[8], rnd, sh);

    // stage 7: output permutation
    out[0] = s6[0];
    out[1] = s6[8];
    out[2] = s6[4];
    out[3] = s6[12];
    out[4] = s6[2];
    out[5] = s6[10];
    out[6] = s6[6];
    out[7] = s6[14];
    out[8] = s6[1];
    out[9] = s6[9];
    out[10] = s6[5];
    out[11] = s6[13];
    out[12] = s6[3];
    out[13] = s6[11];
    out[14] = s6[7];
    out[15] = s6[15];
}

// ===========================================================================
// 32- and 64-point kernels — mechanically generated from the scalar butterfly
// source (tools/gen_txfm_simd.py) and differential-verified byte-exact vs real
// C in c_parity_txfm.rs. Same op sequence as the scalar => bit-identical.
// ===========================================================================

#[rite]
pub(super) fn idct32_x8(
    t: Desktop64,
    inp: &[__m256i; 32],
    out: &mut [__m256i; 32],
    rnd: __m256i,
    sh: __m128i,
    lo: __m256i,
    hi: __m256i,
) {
    let cospi = &COSPI;
    let cl = |v| clampv(t, v, lo, hi);
    // stage 1
    let s1: [__m256i; 32] = [
        inp[0], inp[16], inp[8], inp[24],
        inp[4], inp[20], inp[12], inp[28],
        inp[2], inp[18], inp[10], inp[26],
        inp[6], inp[22], inp[14], inp[30],
        inp[1], inp[17], inp[9], inp[25],
        inp[5], inp[21], inp[13], inp[29],
        inp[3], inp[19], inp[11], inp[27],
        inp[7], inp[23], inp[15], inp[31],
    ];
    // stage 2
    let s2: [__m256i; 32] = [
        s1[0], s1[1], s1[2], s1[3],
        s1[4], s1[5], s1[6], s1[7],
        s1[8], s1[9], s1[10], s1[11],
        s1[12], s1[13], s1[14], s1[15],
        hbtf(t, c!(t, cospi, 62), s1[16], cn!(t, cospi, 2), s1[31], rnd, sh), hbtf(t, c!(t, cospi, 30), s1[17], cn!(t, cospi, 34), s1[30], rnd, sh), hbtf(t, c!(t, cospi, 46), s1[18], cn!(t, cospi, 18), s1[29], rnd, sh), hbtf(t, c!(t, cospi, 14), s1[19], cn!(t, cospi, 50), s1[28], rnd, sh),
        hbtf(t, c!(t, cospi, 54), s1[20], cn!(t, cospi, 10), s1[27], rnd, sh), hbtf(t, c!(t, cospi, 22), s1[21], cn!(t, cospi, 42), s1[26], rnd, sh), hbtf(t, c!(t, cospi, 38), s1[22], cn!(t, cospi, 26), s1[25], rnd, sh), hbtf(t, c!(t, cospi, 6), s1[23], cn!(t, cospi, 58), s1[24], rnd, sh),
        hbtf(t, c!(t, cospi, 58), s1[23], c!(t, cospi, 6), s1[24], rnd, sh), hbtf(t, c!(t, cospi, 26), s1[22], c!(t, cospi, 38), s1[25], rnd, sh), hbtf(t, c!(t, cospi, 42), s1[21], c!(t, cospi, 22), s1[26], rnd, sh), hbtf(t, c!(t, cospi, 10), s1[20], c!(t, cospi, 54), s1[27], rnd, sh),
        hbtf(t, c!(t, cospi, 50), s1[19], c!(t, cospi, 14), s1[28], rnd, sh), hbtf(t, c!(t, cospi, 18), s1[18], c!(t, cospi, 46), s1[29], rnd, sh), hbtf(t, c!(t, cospi, 34), s1[17], c!(t, cospi, 30), s1[30], rnd, sh), hbtf(t, c!(t, cospi, 2), s1[16], c!(t, cospi, 62), s1[31], rnd, sh),
    ];
    // stage 3
    let s3: [__m256i; 32] = [
        s2[0], s2[1], s2[2], s2[3],
        s2[4], s2[5], s2[6], s2[7],
        hbtf(t, c!(t, cospi, 60), s2[8], cn!(t, cospi, 4), s2[15], rnd, sh), hbtf(t, c!(t, cospi, 28), s2[9], cn!(t, cospi, 36), s2[14], rnd, sh), hbtf(t, c!(t, cospi, 44), s2[10], cn!(t, cospi, 20), s2[13], rnd, sh), hbtf(t, c!(t, cospi, 12), s2[11], cn!(t, cospi, 52), s2[12], rnd, sh),
        hbtf(t, c!(t, cospi, 52), s2[11], c!(t, cospi, 12), s2[12], rnd, sh), hbtf(t, c!(t, cospi, 20), s2[10], c!(t, cospi, 44), s2[13], rnd, sh), hbtf(t, c!(t, cospi, 36), s2[9], c!(t, cospi, 28), s2[14], rnd, sh), hbtf(t, c!(t, cospi, 4), s2[8], c!(t, cospi, 60), s2[15], rnd, sh),
        cl(add!(s2[16], s2[17])), cl(sub!(s2[16], s2[17])), cl(sub!(s2[19], s2[18])), cl(add!(s2[18], s2[19])),
        cl(add!(s2[20], s2[21])), cl(sub!(s2[20], s2[21])), cl(sub!(s2[23], s2[22])), cl(add!(s2[22], s2[23])),
        cl(add!(s2[24], s2[25])), cl(sub!(s2[24], s2[25])), cl(sub!(s2[27], s2[26])), cl(add!(s2[26], s2[27])),
        cl(add!(s2[28], s2[29])), cl(sub!(s2[28], s2[29])), cl(sub!(s2[31], s2[30])), cl(add!(s2[30], s2[31])),
    ];
    // stage 4
    let s4: [__m256i; 32] = [
        s3[0], s3[1], s3[2], s3[3],
        hbtf(t, c!(t, cospi, 56), s3[4], cn!(t, cospi, 8), s3[7], rnd, sh), hbtf(t, c!(t, cospi, 24), s3[5], cn!(t, cospi, 40), s3[6], rnd, sh), hbtf(t, c!(t, cospi, 40), s3[5], c!(t, cospi, 24), s3[6], rnd, sh), hbtf(t, c!(t, cospi, 8), s3[4], c!(t, cospi, 56), s3[7], rnd, sh),
        cl(add!(s3[8], s3[9])), cl(sub!(s3[8], s3[9])), cl(sub!(s3[11], s3[10])), cl(add!(s3[10], s3[11])),
        cl(add!(s3[12], s3[13])), cl(sub!(s3[12], s3[13])), cl(sub!(s3[15], s3[14])), cl(add!(s3[14], s3[15])),
        s3[16], hbtf(t, cn!(t, cospi, 8), s3[17], c!(t, cospi, 56), s3[30], rnd, sh), hbtf(t, cn!(t, cospi, 56), s3[18], cn!(t, cospi, 8), s3[29], rnd, sh), s3[19],
        s3[20], hbtf(t, cn!(t, cospi, 40), s3[21], c!(t, cospi, 24), s3[26], rnd, sh), hbtf(t, cn!(t, cospi, 24), s3[22], cn!(t, cospi, 40), s3[25], rnd, sh), s3[23],
        s3[24], hbtf(t, cn!(t, cospi, 40), s3[22], c!(t, cospi, 24), s3[25], rnd, sh), hbtf(t, c!(t, cospi, 24), s3[21], c!(t, cospi, 40), s3[26], rnd, sh), s3[27],
        s3[28], hbtf(t, cn!(t, cospi, 8), s3[18], c!(t, cospi, 56), s3[29], rnd, sh), hbtf(t, c!(t, cospi, 56), s3[17], c!(t, cospi, 8), s3[30], rnd, sh), s3[31],
    ];
    // stage 5
    let s5: [__m256i; 32] = [
        hbtf(t, c!(t, cospi, 32), s4[0], c!(t, cospi, 32), s4[1], rnd, sh), hbtf(t, c!(t, cospi, 32), s4[0], cn!(t, cospi, 32), s4[1], rnd, sh), hbtf(t, c!(t, cospi, 48), s4[2], cn!(t, cospi, 16), s4[3], rnd, sh), hbtf(t, c!(t, cospi, 16), s4[2], c!(t, cospi, 48), s4[3], rnd, sh),
        cl(add!(s4[4], s4[5])), cl(sub!(s4[4], s4[5])), cl(sub!(s4[7], s4[6])), cl(add!(s4[6], s4[7])),
        s4[8], hbtf(t, cn!(t, cospi, 16), s4[9], c!(t, cospi, 48), s4[14], rnd, sh), hbtf(t, cn!(t, cospi, 48), s4[10], cn!(t, cospi, 16), s4[13], rnd, sh), s4[11],
        s4[12], hbtf(t, cn!(t, cospi, 16), s4[10], c!(t, cospi, 48), s4[13], rnd, sh), hbtf(t, c!(t, cospi, 48), s4[9], c!(t, cospi, 16), s4[14], rnd, sh), s4[15],
        cl(add!(s4[16], s4[19])), cl(add!(s4[17], s4[18])), cl(sub!(s4[17], s4[18])), cl(sub!(s4[16], s4[19])),
        cl(sub!(s4[23], s4[20])), cl(sub!(s4[22], s4[21])), cl(add!(s4[21], s4[22])), cl(add!(s4[20], s4[23])),
        cl(add!(s4[24], s4[27])), cl(add!(s4[25], s4[26])), cl(sub!(s4[25], s4[26])), cl(sub!(s4[24], s4[27])),
        cl(sub!(s4[31], s4[28])), cl(sub!(s4[30], s4[29])), cl(add!(s4[29], s4[30])), cl(add!(s4[28], s4[31])),
    ];
    // stage 6
    let s6: [__m256i; 32] = [
        cl(add!(s5[0], s5[3])), cl(add!(s5[1], s5[2])), cl(sub!(s5[1], s5[2])), cl(sub!(s5[0], s5[3])),
        s5[4], hbtf(t, cn!(t, cospi, 32), s5[5], c!(t, cospi, 32), s5[6], rnd, sh), hbtf(t, c!(t, cospi, 32), s5[5], c!(t, cospi, 32), s5[6], rnd, sh), s5[7],
        cl(add!(s5[8], s5[11])), cl(add!(s5[9], s5[10])), cl(sub!(s5[9], s5[10])), cl(sub!(s5[8], s5[11])),
        cl(sub!(s5[15], s5[12])), cl(sub!(s5[14], s5[13])), cl(add!(s5[13], s5[14])), cl(add!(s5[12], s5[15])),
        s5[16], s5[17], hbtf(t, cn!(t, cospi, 16), s5[18], c!(t, cospi, 48), s5[29], rnd, sh), hbtf(t, cn!(t, cospi, 16), s5[19], c!(t, cospi, 48), s5[28], rnd, sh),
        hbtf(t, cn!(t, cospi, 48), s5[20], cn!(t, cospi, 16), s5[27], rnd, sh), hbtf(t, cn!(t, cospi, 48), s5[21], cn!(t, cospi, 16), s5[26], rnd, sh), s5[22], s5[23],
        s5[24], s5[25], hbtf(t, cn!(t, cospi, 16), s5[21], c!(t, cospi, 48), s5[26], rnd, sh), hbtf(t, cn!(t, cospi, 16), s5[20], c!(t, cospi, 48), s5[27], rnd, sh),
        hbtf(t, c!(t, cospi, 48), s5[19], c!(t, cospi, 16), s5[28], rnd, sh), hbtf(t, c!(t, cospi, 48), s5[18], c!(t, cospi, 16), s5[29], rnd, sh), s5[30], s5[31],
    ];
    // stage 7
    let s7: [__m256i; 32] = [
        cl(add!(s6[0], s6[7])), cl(add!(s6[1], s6[6])), cl(add!(s6[2], s6[5])), cl(add!(s6[3], s6[4])),
        cl(sub!(s6[3], s6[4])), cl(sub!(s6[2], s6[5])), cl(sub!(s6[1], s6[6])), cl(sub!(s6[0], s6[7])),
        s6[8], s6[9], hbtf(t, cn!(t, cospi, 32), s6[10], c!(t, cospi, 32), s6[13], rnd, sh), hbtf(t, cn!(t, cospi, 32), s6[11], c!(t, cospi, 32), s6[12], rnd, sh),
        hbtf(t, c!(t, cospi, 32), s6[11], c!(t, cospi, 32), s6[12], rnd, sh), hbtf(t, c!(t, cospi, 32), s6[10], c!(t, cospi, 32), s6[13], rnd, sh), s6[14], s6[15],
        cl(add!(s6[16], s6[23])), cl(add!(s6[17], s6[22])), cl(add!(s6[18], s6[21])), cl(add!(s6[19], s6[20])),
        cl(sub!(s6[19], s6[20])), cl(sub!(s6[18], s6[21])), cl(sub!(s6[17], s6[22])), cl(sub!(s6[16], s6[23])),
        cl(sub!(s6[31], s6[24])), cl(sub!(s6[30], s6[25])), cl(sub!(s6[29], s6[26])), cl(sub!(s6[28], s6[27])),
        cl(add!(s6[27], s6[28])), cl(add!(s6[26], s6[29])), cl(add!(s6[25], s6[30])), cl(add!(s6[24], s6[31])),
    ];
    // stage 8
    let s8: [__m256i; 32] = [
        cl(add!(s7[0], s7[15])), cl(add!(s7[1], s7[14])), cl(add!(s7[2], s7[13])), cl(add!(s7[3], s7[12])),
        cl(add!(s7[4], s7[11])), cl(add!(s7[5], s7[10])), cl(add!(s7[6], s7[9])), cl(add!(s7[7], s7[8])),
        cl(sub!(s7[7], s7[8])), cl(sub!(s7[6], s7[9])), cl(sub!(s7[5], s7[10])), cl(sub!(s7[4], s7[11])),
        cl(sub!(s7[3], s7[12])), cl(sub!(s7[2], s7[13])), cl(sub!(s7[1], s7[14])), cl(sub!(s7[0], s7[15])),
        s7[16], s7[17], s7[18], s7[19],
        hbtf(t, cn!(t, cospi, 32), s7[20], c!(t, cospi, 32), s7[27], rnd, sh), hbtf(t, cn!(t, cospi, 32), s7[21], c!(t, cospi, 32), s7[26], rnd, sh), hbtf(t, cn!(t, cospi, 32), s7[22], c!(t, cospi, 32), s7[25], rnd, sh), hbtf(t, cn!(t, cospi, 32), s7[23], c!(t, cospi, 32), s7[24], rnd, sh),
        hbtf(t, c!(t, cospi, 32), s7[23], c!(t, cospi, 32), s7[24], rnd, sh), hbtf(t, c!(t, cospi, 32), s7[22], c!(t, cospi, 32), s7[25], rnd, sh), hbtf(t, c!(t, cospi, 32), s7[21], c!(t, cospi, 32), s7[26], rnd, sh), hbtf(t, c!(t, cospi, 32), s7[20], c!(t, cospi, 32), s7[27], rnd, sh),
        s7[28], s7[29], s7[30], s7[31],
    ];
    // stage 9
    *out = [
        cl(add!(s8[0], s8[31])), cl(add!(s8[1], s8[30])), cl(add!(s8[2], s8[29])), cl(add!(s8[3], s8[28])),
        cl(add!(s8[4], s8[27])), cl(add!(s8[5], s8[26])), cl(add!(s8[6], s8[25])), cl(add!(s8[7], s8[24])),
        cl(add!(s8[8], s8[23])), cl(add!(s8[9], s8[22])), cl(add!(s8[10], s8[21])), cl(add!(s8[11], s8[20])),
        cl(add!(s8[12], s8[19])), cl(add!(s8[13], s8[18])), cl(add!(s8[14], s8[17])), cl(add!(s8[15], s8[16])),
        cl(sub!(s8[15], s8[16])), cl(sub!(s8[14], s8[17])), cl(sub!(s8[13], s8[18])), cl(sub!(s8[12], s8[19])),
        cl(sub!(s8[11], s8[20])), cl(sub!(s8[10], s8[21])), cl(sub!(s8[9], s8[22])), cl(sub!(s8[8], s8[23])),
        cl(sub!(s8[7], s8[24])), cl(sub!(s8[6], s8[25])), cl(sub!(s8[5], s8[26])), cl(sub!(s8[4], s8[27])),
        cl(sub!(s8[3], s8[28])), cl(sub!(s8[2], s8[29])), cl(sub!(s8[1], s8[30])), cl(sub!(s8[0], s8[31])),
    ];
}

#[rite]
pub(super) fn fdct32_x8(t: Desktop64, inp: &[__m256i; 32], out: &mut [__m256i; 32], cos_bit: i8) {
    let cospi = cospi_arr(cos_bit);
    let rnd = splat(t, 1 << (cos_bit as u32 - 1));
    let sh = _mm_cvtsi32_si128(cos_bit as i32);
    // stage 1
    let s1: [__m256i; 32] = [
        add!(inp[0], inp[31]), add!(inp[1], inp[30]), add!(inp[2], inp[29]), add!(inp[3], inp[28]),
        add!(inp[4], inp[27]), add!(inp[5], inp[26]), add!(inp[6], inp[25]), add!(inp[7], inp[24]),
        add!(inp[8], inp[23]), add!(inp[9], inp[22]), add!(inp[10], inp[21]), add!(inp[11], inp[20]),
        add!(inp[12], inp[19]), add!(inp[13], inp[18]), add!(inp[14], inp[17]), add!(inp[15], inp[16]),
        sub!(inp[15], inp[16]), sub!(inp[14], inp[17]), sub!(inp[13], inp[18]), sub!(inp[12], inp[19]),
        sub!(inp[11], inp[20]), sub!(inp[10], inp[21]), sub!(inp[9], inp[22]), sub!(inp[8], inp[23]),
        sub!(inp[7], inp[24]), sub!(inp[6], inp[25]), sub!(inp[5], inp[26]), sub!(inp[4], inp[27]),
        sub!(inp[3], inp[28]), sub!(inp[2], inp[29]), sub!(inp[1], inp[30]), sub!(inp[0], inp[31]),
    ];
    // stage 2
    let s2: [__m256i; 32] = [
        add!(s1[0], s1[15]), add!(s1[1], s1[14]), add!(s1[2], s1[13]), add!(s1[3], s1[12]),
        add!(s1[4], s1[11]), add!(s1[5], s1[10]), add!(s1[6], s1[9]), add!(s1[7], s1[8]),
        sub!(s1[7], s1[8]), sub!(s1[6], s1[9]), sub!(s1[5], s1[10]), sub!(s1[4], s1[11]),
        sub!(s1[3], s1[12]), sub!(s1[2], s1[13]), sub!(s1[1], s1[14]), sub!(s1[0], s1[15]),
        s1[16], s1[17], s1[18], s1[19],
        hbtf(t, cn!(t, cospi, 32), s1[20], c!(t, cospi, 32), s1[27], rnd, sh), hbtf(t, cn!(t, cospi, 32), s1[21], c!(t, cospi, 32), s1[26], rnd, sh), hbtf(t, cn!(t, cospi, 32), s1[22], c!(t, cospi, 32), s1[25], rnd, sh), hbtf(t, cn!(t, cospi, 32), s1[23], c!(t, cospi, 32), s1[24], rnd, sh),
        hbtf(t, c!(t, cospi, 32), s1[24], c!(t, cospi, 32), s1[23], rnd, sh), hbtf(t, c!(t, cospi, 32), s1[25], c!(t, cospi, 32), s1[22], rnd, sh), hbtf(t, c!(t, cospi, 32), s1[26], c!(t, cospi, 32), s1[21], rnd, sh), hbtf(t, c!(t, cospi, 32), s1[27], c!(t, cospi, 32), s1[20], rnd, sh),
        s1[28], s1[29], s1[30], s1[31],
    ];
    // stage 3
    let s3: [__m256i; 32] = [
        add!(s2[0], s2[7]), add!(s2[1], s2[6]), add!(s2[2], s2[5]), add!(s2[3], s2[4]),
        sub!(s2[3], s2[4]), sub!(s2[2], s2[5]), sub!(s2[1], s2[6]), sub!(s2[0], s2[7]),
        s2[8], s2[9], hbtf(t, cn!(t, cospi, 32), s2[10], c!(t, cospi, 32), s2[13], rnd, sh), hbtf(t, cn!(t, cospi, 32), s2[11], c!(t, cospi, 32), s2[12], rnd, sh),
        hbtf(t, c!(t, cospi, 32), s2[12], c!(t, cospi, 32), s2[11], rnd, sh), hbtf(t, c!(t, cospi, 32), s2[13], c!(t, cospi, 32), s2[10], rnd, sh), s2[14], s2[15],
        add!(s2[16], s2[23]), add!(s2[17], s2[22]), add!(s2[18], s2[21]), add!(s2[19], s2[20]),
        sub!(s2[19], s2[20]), sub!(s2[18], s2[21]), sub!(s2[17], s2[22]), sub!(s2[16], s2[23]),
        sub!(s2[31], s2[24]), sub!(s2[30], s2[25]), sub!(s2[29], s2[26]), sub!(s2[28], s2[27]),
        add!(s2[28], s2[27]), add!(s2[29], s2[26]), add!(s2[30], s2[25]), add!(s2[31], s2[24]),
    ];
    // stage 4
    let s4: [__m256i; 32] = [
        add!(s3[0], s3[3]), add!(s3[1], s3[2]), sub!(s3[1], s3[2]), sub!(s3[0], s3[3]),
        s3[4], hbtf(t, cn!(t, cospi, 32), s3[5], c!(t, cospi, 32), s3[6], rnd, sh), hbtf(t, c!(t, cospi, 32), s3[6], c!(t, cospi, 32), s3[5], rnd, sh), s3[7],
        add!(s3[8], s3[11]), add!(s3[9], s3[10]), sub!(s3[9], s3[10]), sub!(s3[8], s3[11]),
        sub!(s3[15], s3[12]), sub!(s3[14], s3[13]), add!(s3[14], s3[13]), add!(s3[15], s3[12]),
        s3[16], s3[17], hbtf(t, cn!(t, cospi, 16), s3[18], c!(t, cospi, 48), s3[29], rnd, sh), hbtf(t, cn!(t, cospi, 16), s3[19], c!(t, cospi, 48), s3[28], rnd, sh),
        hbtf(t, cn!(t, cospi, 48), s3[20], cn!(t, cospi, 16), s3[27], rnd, sh), hbtf(t, cn!(t, cospi, 48), s3[21], cn!(t, cospi, 16), s3[26], rnd, sh), s3[22], s3[23],
        s3[24], s3[25], hbtf(t, c!(t, cospi, 48), s3[26], cn!(t, cospi, 16), s3[21], rnd, sh), hbtf(t, c!(t, cospi, 48), s3[27], cn!(t, cospi, 16), s3[20], rnd, sh),
        hbtf(t, c!(t, cospi, 16), s3[28], c!(t, cospi, 48), s3[19], rnd, sh), hbtf(t, c!(t, cospi, 16), s3[29], c!(t, cospi, 48), s3[18], rnd, sh), s3[30], s3[31],
    ];
    // stage 5
    let s5: [__m256i; 32] = [
        hbtf(t, c!(t, cospi, 32), s4[0], c!(t, cospi, 32), s4[1], rnd, sh), hbtf(t, cn!(t, cospi, 32), s4[1], c!(t, cospi, 32), s4[0], rnd, sh), hbtf(t, c!(t, cospi, 48), s4[2], c!(t, cospi, 16), s4[3], rnd, sh), hbtf(t, c!(t, cospi, 48), s4[3], cn!(t, cospi, 16), s4[2], rnd, sh),
        add!(s4[4], s4[5]), sub!(s4[4], s4[5]), sub!(s4[7], s4[6]), add!(s4[7], s4[6]),
        s4[8], hbtf(t, cn!(t, cospi, 16), s4[9], c!(t, cospi, 48), s4[14], rnd, sh), hbtf(t, cn!(t, cospi, 48), s4[10], cn!(t, cospi, 16), s4[13], rnd, sh), s4[11],
        s4[12], hbtf(t, c!(t, cospi, 48), s4[13], cn!(t, cospi, 16), s4[10], rnd, sh), hbtf(t, c!(t, cospi, 16), s4[14], c!(t, cospi, 48), s4[9], rnd, sh), s4[15],
        add!(s4[16], s4[19]), add!(s4[17], s4[18]), sub!(s4[17], s4[18]), sub!(s4[16], s4[19]),
        sub!(s4[23], s4[20]), sub!(s4[22], s4[21]), add!(s4[22], s4[21]), add!(s4[23], s4[20]),
        add!(s4[24], s4[27]), add!(s4[25], s4[26]), sub!(s4[25], s4[26]), sub!(s4[24], s4[27]),
        sub!(s4[31], s4[28]), sub!(s4[30], s4[29]), add!(s4[30], s4[29]), add!(s4[31], s4[28]),
    ];
    // stage 6
    let s6: [__m256i; 32] = [
        s5[0], s5[1], s5[2], s5[3],
        hbtf(t, c!(t, cospi, 56), s5[4], c!(t, cospi, 8), s5[7], rnd, sh), hbtf(t, c!(t, cospi, 24), s5[5], c!(t, cospi, 40), s5[6], rnd, sh), hbtf(t, c!(t, cospi, 24), s5[6], cn!(t, cospi, 40), s5[5], rnd, sh), hbtf(t, c!(t, cospi, 56), s5[7], cn!(t, cospi, 8), s5[4], rnd, sh),
        add!(s5[8], s5[9]), sub!(s5[8], s5[9]), sub!(s5[11], s5[10]), add!(s5[11], s5[10]),
        add!(s5[12], s5[13]), sub!(s5[12], s5[13]), sub!(s5[15], s5[14]), add!(s5[15], s5[14]),
        s5[16], hbtf(t, cn!(t, cospi, 8), s5[17], c!(t, cospi, 56), s5[30], rnd, sh), hbtf(t, cn!(t, cospi, 56), s5[18], cn!(t, cospi, 8), s5[29], rnd, sh), s5[19],
        s5[20], hbtf(t, cn!(t, cospi, 40), s5[21], c!(t, cospi, 24), s5[26], rnd, sh), hbtf(t, cn!(t, cospi, 24), s5[22], cn!(t, cospi, 40), s5[25], rnd, sh), s5[23],
        s5[24], hbtf(t, c!(t, cospi, 24), s5[25], cn!(t, cospi, 40), s5[22], rnd, sh), hbtf(t, c!(t, cospi, 40), s5[26], c!(t, cospi, 24), s5[21], rnd, sh), s5[27],
        s5[28], hbtf(t, c!(t, cospi, 56), s5[29], cn!(t, cospi, 8), s5[18], rnd, sh), hbtf(t, c!(t, cospi, 8), s5[30], c!(t, cospi, 56), s5[17], rnd, sh), s5[31],
    ];
    // stage 7
    let s7: [__m256i; 32] = [
        s6[0], s6[1], s6[2], s6[3],
        s6[4], s6[5], s6[6], s6[7],
        hbtf(t, c!(t, cospi, 60), s6[8], c!(t, cospi, 4), s6[15], rnd, sh), hbtf(t, c!(t, cospi, 28), s6[9], c!(t, cospi, 36), s6[14], rnd, sh), hbtf(t, c!(t, cospi, 44), s6[10], c!(t, cospi, 20), s6[13], rnd, sh), hbtf(t, c!(t, cospi, 12), s6[11], c!(t, cospi, 52), s6[12], rnd, sh),
        hbtf(t, c!(t, cospi, 12), s6[12], cn!(t, cospi, 52), s6[11], rnd, sh), hbtf(t, c!(t, cospi, 44), s6[13], cn!(t, cospi, 20), s6[10], rnd, sh), hbtf(t, c!(t, cospi, 28), s6[14], cn!(t, cospi, 36), s6[9], rnd, sh), hbtf(t, c!(t, cospi, 60), s6[15], cn!(t, cospi, 4), s6[8], rnd, sh),
        add!(s6[16], s6[17]), sub!(s6[16], s6[17]), sub!(s6[19], s6[18]), add!(s6[19], s6[18]),
        add!(s6[20], s6[21]), sub!(s6[20], s6[21]), sub!(s6[23], s6[22]), add!(s6[23], s6[22]),
        add!(s6[24], s6[25]), sub!(s6[24], s6[25]), sub!(s6[27], s6[26]), add!(s6[27], s6[26]),
        add!(s6[28], s6[29]), sub!(s6[28], s6[29]), sub!(s6[31], s6[30]), add!(s6[31], s6[30]),
    ];
    // stage 8
    let s8: [__m256i; 32] = [
        s7[0], s7[1], s7[2], s7[3],
        s7[4], s7[5], s7[6], s7[7],
        s7[8], s7[9], s7[10], s7[11],
        s7[12], s7[13], s7[14], s7[15],
        hbtf(t, c!(t, cospi, 62), s7[16], c!(t, cospi, 2), s7[31], rnd, sh), hbtf(t, c!(t, cospi, 30), s7[17], c!(t, cospi, 34), s7[30], rnd, sh), hbtf(t, c!(t, cospi, 46), s7[18], c!(t, cospi, 18), s7[29], rnd, sh), hbtf(t, c!(t, cospi, 14), s7[19], c!(t, cospi, 50), s7[28], rnd, sh),
        hbtf(t, c!(t, cospi, 54), s7[20], c!(t, cospi, 10), s7[27], rnd, sh), hbtf(t, c!(t, cospi, 22), s7[21], c!(t, cospi, 42), s7[26], rnd, sh), hbtf(t, c!(t, cospi, 38), s7[22], c!(t, cospi, 26), s7[25], rnd, sh), hbtf(t, c!(t, cospi, 6), s7[23], c!(t, cospi, 58), s7[24], rnd, sh),
        hbtf(t, c!(t, cospi, 6), s7[24], cn!(t, cospi, 58), s7[23], rnd, sh), hbtf(t, c!(t, cospi, 38), s7[25], cn!(t, cospi, 26), s7[22], rnd, sh), hbtf(t, c!(t, cospi, 22), s7[26], cn!(t, cospi, 42), s7[21], rnd, sh), hbtf(t, c!(t, cospi, 54), s7[27], cn!(t, cospi, 10), s7[20], rnd, sh),
        hbtf(t, c!(t, cospi, 14), s7[28], cn!(t, cospi, 50), s7[19], rnd, sh), hbtf(t, c!(t, cospi, 46), s7[29], cn!(t, cospi, 18), s7[18], rnd, sh), hbtf(t, c!(t, cospi, 30), s7[30], cn!(t, cospi, 34), s7[17], rnd, sh), hbtf(t, c!(t, cospi, 62), s7[31], cn!(t, cospi, 2), s7[16], rnd, sh),
    ];
    // stage 9
    *out = [
        s8[0], s8[16], s8[8], s8[24],
        s8[4], s8[20], s8[12], s8[28],
        s8[2], s8[18], s8[10], s8[26],
        s8[6], s8[22], s8[14], s8[30],
        s8[1], s8[17], s8[9], s8[25],
        s8[5], s8[21], s8[13], s8[29],
        s8[3], s8[19], s8[11], s8[27],
        s8[7], s8[23], s8[15], s8[31],
    ];
}

#[rite]
pub(super) fn idct64_x8(
    t: Desktop64,
    inp: &[__m256i; 64],
    out: &mut [__m256i; 64],
    rnd: __m256i,
    sh: __m128i,
    lo: __m256i,
    hi: __m256i,
) {
    let cospi = &COSPI;
    let cl = |v| clampv(t, v, lo, hi);
    // stage 1
    let s1: [__m256i; 64] = [
        inp[0], inp[32], inp[16], inp[48],
        inp[8], inp[40], inp[24], inp[56],
        inp[4], inp[36], inp[20], inp[52],
        inp[12], inp[44], inp[28], inp[60],
        inp[2], inp[34], inp[18], inp[50],
        inp[10], inp[42], inp[26], inp[58],
        inp[6], inp[38], inp[22], inp[54],
        inp[14], inp[46], inp[30], inp[62],
        inp[1], inp[33], inp[17], inp[49],
        inp[9], inp[41], inp[25], inp[57],
        inp[5], inp[37], inp[21], inp[53],
        inp[13], inp[45], inp[29], inp[61],
        inp[3], inp[35], inp[19], inp[51],
        inp[11], inp[43], inp[27], inp[59],
        inp[7], inp[39], inp[23], inp[55],
        inp[15], inp[47], inp[31], inp[63],
    ];
    // stage 2
    let s2: [__m256i; 64] = [
        s1[0], s1[1], s1[2], s1[3],
        s1[4], s1[5], s1[6], s1[7],
        s1[8], s1[9], s1[10], s1[11],
        s1[12], s1[13], s1[14], s1[15],
        s1[16], s1[17], s1[18], s1[19],
        s1[20], s1[21], s1[22], s1[23],
        s1[24], s1[25], s1[26], s1[27],
        s1[28], s1[29], s1[30], s1[31],
        hbtf(t, c!(t, cospi, 63), s1[32], cn!(t, cospi, 1), s1[63], rnd, sh), hbtf(t, c!(t, cospi, 31), s1[33], cn!(t, cospi, 33), s1[62], rnd, sh), hbtf(t, c!(t, cospi, 47), s1[34], cn!(t, cospi, 17), s1[61], rnd, sh), hbtf(t, c!(t, cospi, 15), s1[35], cn!(t, cospi, 49), s1[60], rnd, sh),
        hbtf(t, c!(t, cospi, 55), s1[36], cn!(t, cospi, 9), s1[59], rnd, sh), hbtf(t, c!(t, cospi, 23), s1[37], cn!(t, cospi, 41), s1[58], rnd, sh), hbtf(t, c!(t, cospi, 39), s1[38], cn!(t, cospi, 25), s1[57], rnd, sh), hbtf(t, c!(t, cospi, 7), s1[39], cn!(t, cospi, 57), s1[56], rnd, sh),
        hbtf(t, c!(t, cospi, 59), s1[40], cn!(t, cospi, 5), s1[55], rnd, sh), hbtf(t, c!(t, cospi, 27), s1[41], cn!(t, cospi, 37), s1[54], rnd, sh), hbtf(t, c!(t, cospi, 43), s1[42], cn!(t, cospi, 21), s1[53], rnd, sh), hbtf(t, c!(t, cospi, 11), s1[43], cn!(t, cospi, 53), s1[52], rnd, sh),
        hbtf(t, c!(t, cospi, 51), s1[44], cn!(t, cospi, 13), s1[51], rnd, sh), hbtf(t, c!(t, cospi, 19), s1[45], cn!(t, cospi, 45), s1[50], rnd, sh), hbtf(t, c!(t, cospi, 35), s1[46], cn!(t, cospi, 29), s1[49], rnd, sh), hbtf(t, c!(t, cospi, 3), s1[47], cn!(t, cospi, 61), s1[48], rnd, sh),
        hbtf(t, c!(t, cospi, 61), s1[47], c!(t, cospi, 3), s1[48], rnd, sh), hbtf(t, c!(t, cospi, 29), s1[46], c!(t, cospi, 35), s1[49], rnd, sh), hbtf(t, c!(t, cospi, 45), s1[45], c!(t, cospi, 19), s1[50], rnd, sh), hbtf(t, c!(t, cospi, 13), s1[44], c!(t, cospi, 51), s1[51], rnd, sh),
        hbtf(t, c!(t, cospi, 53), s1[43], c!(t, cospi, 11), s1[52], rnd, sh), hbtf(t, c!(t, cospi, 21), s1[42], c!(t, cospi, 43), s1[53], rnd, sh), hbtf(t, c!(t, cospi, 37), s1[41], c!(t, cospi, 27), s1[54], rnd, sh), hbtf(t, c!(t, cospi, 5), s1[40], c!(t, cospi, 59), s1[55], rnd, sh),
        hbtf(t, c!(t, cospi, 57), s1[39], c!(t, cospi, 7), s1[56], rnd, sh), hbtf(t, c!(t, cospi, 25), s1[38], c!(t, cospi, 39), s1[57], rnd, sh), hbtf(t, c!(t, cospi, 41), s1[37], c!(t, cospi, 23), s1[58], rnd, sh), hbtf(t, c!(t, cospi, 9), s1[36], c!(t, cospi, 55), s1[59], rnd, sh),
        hbtf(t, c!(t, cospi, 49), s1[35], c!(t, cospi, 15), s1[60], rnd, sh), hbtf(t, c!(t, cospi, 17), s1[34], c!(t, cospi, 47), s1[61], rnd, sh), hbtf(t, c!(t, cospi, 33), s1[33], c!(t, cospi, 31), s1[62], rnd, sh), hbtf(t, c!(t, cospi, 1), s1[32], c!(t, cospi, 63), s1[63], rnd, sh),
    ];
    // stage 3
    let s3: [__m256i; 64] = [
        s2[0], s2[1], s2[2], s2[3],
        s2[4], s2[5], s2[6], s2[7],
        s2[8], s2[9], s2[10], s2[11],
        s2[12], s2[13], s2[14], s2[15],
        hbtf(t, c!(t, cospi, 62), s2[16], cn!(t, cospi, 2), s2[31], rnd, sh), hbtf(t, c!(t, cospi, 30), s2[17], cn!(t, cospi, 34), s2[30], rnd, sh), hbtf(t, c!(t, cospi, 46), s2[18], cn!(t, cospi, 18), s2[29], rnd, sh), hbtf(t, c!(t, cospi, 14), s2[19], cn!(t, cospi, 50), s2[28], rnd, sh),
        hbtf(t, c!(t, cospi, 54), s2[20], cn!(t, cospi, 10), s2[27], rnd, sh), hbtf(t, c!(t, cospi, 22), s2[21], cn!(t, cospi, 42), s2[26], rnd, sh), hbtf(t, c!(t, cospi, 38), s2[22], cn!(t, cospi, 26), s2[25], rnd, sh), hbtf(t, c!(t, cospi, 6), s2[23], cn!(t, cospi, 58), s2[24], rnd, sh),
        hbtf(t, c!(t, cospi, 58), s2[23], c!(t, cospi, 6), s2[24], rnd, sh), hbtf(t, c!(t, cospi, 26), s2[22], c!(t, cospi, 38), s2[25], rnd, sh), hbtf(t, c!(t, cospi, 42), s2[21], c!(t, cospi, 22), s2[26], rnd, sh), hbtf(t, c!(t, cospi, 10), s2[20], c!(t, cospi, 54), s2[27], rnd, sh),
        hbtf(t, c!(t, cospi, 50), s2[19], c!(t, cospi, 14), s2[28], rnd, sh), hbtf(t, c!(t, cospi, 18), s2[18], c!(t, cospi, 46), s2[29], rnd, sh), hbtf(t, c!(t, cospi, 34), s2[17], c!(t, cospi, 30), s2[30], rnd, sh), hbtf(t, c!(t, cospi, 2), s2[16], c!(t, cospi, 62), s2[31], rnd, sh),
        cl(add!(s2[32], s2[33])), cl(sub!(s2[32], s2[33])), cl(sub!(s2[35], s2[34])), cl(add!(s2[34], s2[35])),
        cl(add!(s2[36], s2[37])), cl(sub!(s2[36], s2[37])), cl(sub!(s2[39], s2[38])), cl(add!(s2[38], s2[39])),
        cl(add!(s2[40], s2[41])), cl(sub!(s2[40], s2[41])), cl(sub!(s2[43], s2[42])), cl(add!(s2[42], s2[43])),
        cl(add!(s2[44], s2[45])), cl(sub!(s2[44], s2[45])), cl(sub!(s2[47], s2[46])), cl(add!(s2[46], s2[47])),
        cl(add!(s2[48], s2[49])), cl(sub!(s2[48], s2[49])), cl(sub!(s2[51], s2[50])), cl(add!(s2[50], s2[51])),
        cl(add!(s2[52], s2[53])), cl(sub!(s2[52], s2[53])), cl(sub!(s2[55], s2[54])), cl(add!(s2[54], s2[55])),
        cl(add!(s2[56], s2[57])), cl(sub!(s2[56], s2[57])), cl(sub!(s2[59], s2[58])), cl(add!(s2[58], s2[59])),
        cl(add!(s2[60], s2[61])), cl(sub!(s2[60], s2[61])), cl(sub!(s2[63], s2[62])), cl(add!(s2[62], s2[63])),
    ];
    // stage 4
    let s4: [__m256i; 64] = [
        s3[0], s3[1], s3[2], s3[3],
        s3[4], s3[5], s3[6], s3[7],
        hbtf(t, c!(t, cospi, 60), s3[8], cn!(t, cospi, 4), s3[15], rnd, sh), hbtf(t, c!(t, cospi, 28), s3[9], cn!(t, cospi, 36), s3[14], rnd, sh), hbtf(t, c!(t, cospi, 44), s3[10], cn!(t, cospi, 20), s3[13], rnd, sh), hbtf(t, c!(t, cospi, 12), s3[11], cn!(t, cospi, 52), s3[12], rnd, sh),
        hbtf(t, c!(t, cospi, 52), s3[11], c!(t, cospi, 12), s3[12], rnd, sh), hbtf(t, c!(t, cospi, 20), s3[10], c!(t, cospi, 44), s3[13], rnd, sh), hbtf(t, c!(t, cospi, 36), s3[9], c!(t, cospi, 28), s3[14], rnd, sh), hbtf(t, c!(t, cospi, 4), s3[8], c!(t, cospi, 60), s3[15], rnd, sh),
        cl(add!(s3[16], s3[17])), cl(sub!(s3[16], s3[17])), cl(sub!(s3[19], s3[18])), cl(add!(s3[18], s3[19])),
        cl(add!(s3[20], s3[21])), cl(sub!(s3[20], s3[21])), cl(sub!(s3[23], s3[22])), cl(add!(s3[22], s3[23])),
        cl(add!(s3[24], s3[25])), cl(sub!(s3[24], s3[25])), cl(sub!(s3[27], s3[26])), cl(add!(s3[26], s3[27])),
        cl(add!(s3[28], s3[29])), cl(sub!(s3[28], s3[29])), cl(sub!(s3[31], s3[30])), cl(add!(s3[30], s3[31])),
        s3[32], hbtf(t, cn!(t, cospi, 4), s3[33], c!(t, cospi, 60), s3[62], rnd, sh), hbtf(t, cn!(t, cospi, 60), s3[34], cn!(t, cospi, 4), s3[61], rnd, sh), s3[35],
        s3[36], hbtf(t, cn!(t, cospi, 36), s3[37], c!(t, cospi, 28), s3[58], rnd, sh), hbtf(t, cn!(t, cospi, 28), s3[38], cn!(t, cospi, 36), s3[57], rnd, sh), s3[39],
        s3[40], hbtf(t, cn!(t, cospi, 20), s3[41], c!(t, cospi, 44), s3[54], rnd, sh), hbtf(t, cn!(t, cospi, 44), s3[42], cn!(t, cospi, 20), s3[53], rnd, sh), s3[43],
        s3[44], hbtf(t, cn!(t, cospi, 52), s3[45], c!(t, cospi, 12), s3[50], rnd, sh), hbtf(t, cn!(t, cospi, 12), s3[46], cn!(t, cospi, 52), s3[49], rnd, sh), s3[47],
        s3[48], hbtf(t, cn!(t, cospi, 52), s3[46], c!(t, cospi, 12), s3[49], rnd, sh), hbtf(t, c!(t, cospi, 12), s3[45], c!(t, cospi, 52), s3[50], rnd, sh), s3[51],
        s3[52], hbtf(t, cn!(t, cospi, 20), s3[42], c!(t, cospi, 44), s3[53], rnd, sh), hbtf(t, c!(t, cospi, 44), s3[41], c!(t, cospi, 20), s3[54], rnd, sh), s3[55],
        s3[56], hbtf(t, cn!(t, cospi, 36), s3[38], c!(t, cospi, 28), s3[57], rnd, sh), hbtf(t, c!(t, cospi, 28), s3[37], c!(t, cospi, 36), s3[58], rnd, sh), s3[59],
        s3[60], hbtf(t, cn!(t, cospi, 4), s3[34], c!(t, cospi, 60), s3[61], rnd, sh), hbtf(t, c!(t, cospi, 60), s3[33], c!(t, cospi, 4), s3[62], rnd, sh), s3[63],
    ];
    // stage 5
    let s5: [__m256i; 64] = [
        s4[0], s4[1], s4[2], s4[3],
        hbtf(t, c!(t, cospi, 56), s4[4], cn!(t, cospi, 8), s4[7], rnd, sh), hbtf(t, c!(t, cospi, 24), s4[5], cn!(t, cospi, 40), s4[6], rnd, sh), hbtf(t, c!(t, cospi, 40), s4[5], c!(t, cospi, 24), s4[6], rnd, sh), hbtf(t, c!(t, cospi, 8), s4[4], c!(t, cospi, 56), s4[7], rnd, sh),
        cl(add!(s4[8], s4[9])), cl(sub!(s4[8], s4[9])), cl(sub!(s4[11], s4[10])), cl(add!(s4[10], s4[11])),
        cl(add!(s4[12], s4[13])), cl(sub!(s4[12], s4[13])), cl(sub!(s4[15], s4[14])), cl(add!(s4[14], s4[15])),
        s4[16], hbtf(t, cn!(t, cospi, 8), s4[17], c!(t, cospi, 56), s4[30], rnd, sh), hbtf(t, cn!(t, cospi, 56), s4[18], cn!(t, cospi, 8), s4[29], rnd, sh), s4[19],
        s4[20], hbtf(t, cn!(t, cospi, 40), s4[21], c!(t, cospi, 24), s4[26], rnd, sh), hbtf(t, cn!(t, cospi, 24), s4[22], cn!(t, cospi, 40), s4[25], rnd, sh), s4[23],
        s4[24], hbtf(t, cn!(t, cospi, 40), s4[22], c!(t, cospi, 24), s4[25], rnd, sh), hbtf(t, c!(t, cospi, 24), s4[21], c!(t, cospi, 40), s4[26], rnd, sh), s4[27],
        s4[28], hbtf(t, cn!(t, cospi, 8), s4[18], c!(t, cospi, 56), s4[29], rnd, sh), hbtf(t, c!(t, cospi, 56), s4[17], c!(t, cospi, 8), s4[30], rnd, sh), s4[31],
        cl(add!(s4[32], s4[35])), cl(add!(s4[33], s4[34])), cl(sub!(s4[33], s4[34])), cl(sub!(s4[32], s4[35])),
        cl(sub!(s4[39], s4[36])), cl(sub!(s4[38], s4[37])), cl(add!(s4[37], s4[38])), cl(add!(s4[36], s4[39])),
        cl(add!(s4[40], s4[43])), cl(add!(s4[41], s4[42])), cl(sub!(s4[41], s4[42])), cl(sub!(s4[40], s4[43])),
        cl(sub!(s4[47], s4[44])), cl(sub!(s4[46], s4[45])), cl(add!(s4[45], s4[46])), cl(add!(s4[44], s4[47])),
        cl(add!(s4[48], s4[51])), cl(add!(s4[49], s4[50])), cl(sub!(s4[49], s4[50])), cl(sub!(s4[48], s4[51])),
        cl(sub!(s4[55], s4[52])), cl(sub!(s4[54], s4[53])), cl(add!(s4[53], s4[54])), cl(add!(s4[52], s4[55])),
        cl(add!(s4[56], s4[59])), cl(add!(s4[57], s4[58])), cl(sub!(s4[57], s4[58])), cl(sub!(s4[56], s4[59])),
        cl(sub!(s4[63], s4[60])), cl(sub!(s4[62], s4[61])), cl(add!(s4[61], s4[62])), cl(add!(s4[60], s4[63])),
    ];
    // stage 6
    let s6: [__m256i; 64] = [
        hbtf(t, c!(t, cospi, 32), s5[0], c!(t, cospi, 32), s5[1], rnd, sh), hbtf(t, c!(t, cospi, 32), s5[0], cn!(t, cospi, 32), s5[1], rnd, sh), hbtf(t, c!(t, cospi, 48), s5[2], cn!(t, cospi, 16), s5[3], rnd, sh), hbtf(t, c!(t, cospi, 16), s5[2], c!(t, cospi, 48), s5[3], rnd, sh),
        cl(add!(s5[4], s5[5])), cl(sub!(s5[4], s5[5])), cl(sub!(s5[7], s5[6])), cl(add!(s5[6], s5[7])),
        s5[8], hbtf(t, cn!(t, cospi, 16), s5[9], c!(t, cospi, 48), s5[14], rnd, sh), hbtf(t, cn!(t, cospi, 48), s5[10], cn!(t, cospi, 16), s5[13], rnd, sh), s5[11],
        s5[12], hbtf(t, cn!(t, cospi, 16), s5[10], c!(t, cospi, 48), s5[13], rnd, sh), hbtf(t, c!(t, cospi, 48), s5[9], c!(t, cospi, 16), s5[14], rnd, sh), s5[15],
        cl(add!(s5[16], s5[19])), cl(add!(s5[17], s5[18])), cl(sub!(s5[17], s5[18])), cl(sub!(s5[16], s5[19])),
        cl(sub!(s5[23], s5[20])), cl(sub!(s5[22], s5[21])), cl(add!(s5[21], s5[22])), cl(add!(s5[20], s5[23])),
        cl(add!(s5[24], s5[27])), cl(add!(s5[25], s5[26])), cl(sub!(s5[25], s5[26])), cl(sub!(s5[24], s5[27])),
        cl(sub!(s5[31], s5[28])), cl(sub!(s5[30], s5[29])), cl(add!(s5[29], s5[30])), cl(add!(s5[28], s5[31])),
        s5[32], s5[33], hbtf(t, cn!(t, cospi, 8), s5[34], c!(t, cospi, 56), s5[61], rnd, sh), hbtf(t, cn!(t, cospi, 8), s5[35], c!(t, cospi, 56), s5[60], rnd, sh),
        hbtf(t, cn!(t, cospi, 56), s5[36], cn!(t, cospi, 8), s5[59], rnd, sh), hbtf(t, cn!(t, cospi, 56), s5[37], cn!(t, cospi, 8), s5[58], rnd, sh), s5[38], s5[39],
        s5[40], s5[41], hbtf(t, cn!(t, cospi, 40), s5[42], c!(t, cospi, 24), s5[53], rnd, sh), hbtf(t, cn!(t, cospi, 40), s5[43], c!(t, cospi, 24), s5[52], rnd, sh),
        hbtf(t, cn!(t, cospi, 24), s5[44], cn!(t, cospi, 40), s5[51], rnd, sh), hbtf(t, cn!(t, cospi, 24), s5[45], cn!(t, cospi, 40), s5[50], rnd, sh), s5[46], s5[47],
        s5[48], s5[49], hbtf(t, cn!(t, cospi, 40), s5[45], c!(t, cospi, 24), s5[50], rnd, sh), hbtf(t, cn!(t, cospi, 40), s5[44], c!(t, cospi, 24), s5[51], rnd, sh),
        hbtf(t, c!(t, cospi, 24), s5[43], c!(t, cospi, 40), s5[52], rnd, sh), hbtf(t, c!(t, cospi, 24), s5[42], c!(t, cospi, 40), s5[53], rnd, sh), s5[54], s5[55],
        s5[56], s5[57], hbtf(t, cn!(t, cospi, 8), s5[37], c!(t, cospi, 56), s5[58], rnd, sh), hbtf(t, cn!(t, cospi, 8), s5[36], c!(t, cospi, 56), s5[59], rnd, sh),
        hbtf(t, c!(t, cospi, 56), s5[35], c!(t, cospi, 8), s5[60], rnd, sh), hbtf(t, c!(t, cospi, 56), s5[34], c!(t, cospi, 8), s5[61], rnd, sh), s5[62], s5[63],
    ];
    // stage 7
    let s7: [__m256i; 64] = [
        cl(add!(s6[0], s6[3])), cl(add!(s6[1], s6[2])), cl(sub!(s6[1], s6[2])), cl(sub!(s6[0], s6[3])),
        s6[4], hbtf(t, cn!(t, cospi, 32), s6[5], c!(t, cospi, 32), s6[6], rnd, sh), hbtf(t, c!(t, cospi, 32), s6[5], c!(t, cospi, 32), s6[6], rnd, sh), s6[7],
        cl(add!(s6[8], s6[11])), cl(add!(s6[9], s6[10])), cl(sub!(s6[9], s6[10])), cl(sub!(s6[8], s6[11])),
        cl(sub!(s6[15], s6[12])), cl(sub!(s6[14], s6[13])), cl(add!(s6[13], s6[14])), cl(add!(s6[12], s6[15])),
        s6[16], s6[17], hbtf(t, cn!(t, cospi, 16), s6[18], c!(t, cospi, 48), s6[29], rnd, sh), hbtf(t, cn!(t, cospi, 16), s6[19], c!(t, cospi, 48), s6[28], rnd, sh),
        hbtf(t, cn!(t, cospi, 48), s6[20], cn!(t, cospi, 16), s6[27], rnd, sh), hbtf(t, cn!(t, cospi, 48), s6[21], cn!(t, cospi, 16), s6[26], rnd, sh), s6[22], s6[23],
        s6[24], s6[25], hbtf(t, cn!(t, cospi, 16), s6[21], c!(t, cospi, 48), s6[26], rnd, sh), hbtf(t, cn!(t, cospi, 16), s6[20], c!(t, cospi, 48), s6[27], rnd, sh),
        hbtf(t, c!(t, cospi, 48), s6[19], c!(t, cospi, 16), s6[28], rnd, sh), hbtf(t, c!(t, cospi, 48), s6[18], c!(t, cospi, 16), s6[29], rnd, sh), s6[30], s6[31],
        cl(add!(s6[32], s6[39])), cl(add!(s6[33], s6[38])), cl(add!(s6[34], s6[37])), cl(add!(s6[35], s6[36])),
        cl(sub!(s6[35], s6[36])), cl(sub!(s6[34], s6[37])), cl(sub!(s6[33], s6[38])), cl(sub!(s6[32], s6[39])),
        cl(sub!(s6[47], s6[40])), cl(sub!(s6[46], s6[41])), cl(sub!(s6[45], s6[42])), cl(sub!(s6[44], s6[43])),
        cl(add!(s6[43], s6[44])), cl(add!(s6[42], s6[45])), cl(add!(s6[41], s6[46])), cl(add!(s6[40], s6[47])),
        cl(add!(s6[48], s6[55])), cl(add!(s6[49], s6[54])), cl(add!(s6[50], s6[53])), cl(add!(s6[51], s6[52])),
        cl(sub!(s6[51], s6[52])), cl(sub!(s6[50], s6[53])), cl(sub!(s6[49], s6[54])), cl(sub!(s6[48], s6[55])),
        cl(sub!(s6[63], s6[56])), cl(sub!(s6[62], s6[57])), cl(sub!(s6[61], s6[58])), cl(sub!(s6[60], s6[59])),
        cl(add!(s6[59], s6[60])), cl(add!(s6[58], s6[61])), cl(add!(s6[57], s6[62])), cl(add!(s6[56], s6[63])),
    ];
    // stage 8
    let s8: [__m256i; 64] = [
        cl(add!(s7[0], s7[7])), cl(add!(s7[1], s7[6])), cl(add!(s7[2], s7[5])), cl(add!(s7[3], s7[4])),
        cl(sub!(s7[3], s7[4])), cl(sub!(s7[2], s7[5])), cl(sub!(s7[1], s7[6])), cl(sub!(s7[0], s7[7])),
        s7[8], s7[9], hbtf(t, cn!(t, cospi, 32), s7[10], c!(t, cospi, 32), s7[13], rnd, sh), hbtf(t, cn!(t, cospi, 32), s7[11], c!(t, cospi, 32), s7[12], rnd, sh),
        hbtf(t, c!(t, cospi, 32), s7[11], c!(t, cospi, 32), s7[12], rnd, sh), hbtf(t, c!(t, cospi, 32), s7[10], c!(t, cospi, 32), s7[13], rnd, sh), s7[14], s7[15],
        cl(add!(s7[16], s7[23])), cl(add!(s7[17], s7[22])), cl(add!(s7[18], s7[21])), cl(add!(s7[19], s7[20])),
        cl(sub!(s7[19], s7[20])), cl(sub!(s7[18], s7[21])), cl(sub!(s7[17], s7[22])), cl(sub!(s7[16], s7[23])),
        cl(sub!(s7[31], s7[24])), cl(sub!(s7[30], s7[25])), cl(sub!(s7[29], s7[26])), cl(sub!(s7[28], s7[27])),
        cl(add!(s7[27], s7[28])), cl(add!(s7[26], s7[29])), cl(add!(s7[25], s7[30])), cl(add!(s7[24], s7[31])),
        s7[32], s7[33], s7[34], s7[35],
        hbtf(t, cn!(t, cospi, 16), s7[36], c!(t, cospi, 48), s7[59], rnd, sh), hbtf(t, cn!(t, cospi, 16), s7[37], c!(t, cospi, 48), s7[58], rnd, sh), hbtf(t, cn!(t, cospi, 16), s7[38], c!(t, cospi, 48), s7[57], rnd, sh), hbtf(t, cn!(t, cospi, 16), s7[39], c!(t, cospi, 48), s7[56], rnd, sh),
        hbtf(t, cn!(t, cospi, 48), s7[40], cn!(t, cospi, 16), s7[55], rnd, sh), hbtf(t, cn!(t, cospi, 48), s7[41], cn!(t, cospi, 16), s7[54], rnd, sh), hbtf(t, cn!(t, cospi, 48), s7[42], cn!(t, cospi, 16), s7[53], rnd, sh), hbtf(t, cn!(t, cospi, 48), s7[43], cn!(t, cospi, 16), s7[52], rnd, sh),
        s7[44], s7[45], s7[46], s7[47],
        s7[48], s7[49], s7[50], s7[51],
        hbtf(t, cn!(t, cospi, 16), s7[43], c!(t, cospi, 48), s7[52], rnd, sh), hbtf(t, cn!(t, cospi, 16), s7[42], c!(t, cospi, 48), s7[53], rnd, sh), hbtf(t, cn!(t, cospi, 16), s7[41], c!(t, cospi, 48), s7[54], rnd, sh), hbtf(t, cn!(t, cospi, 16), s7[40], c!(t, cospi, 48), s7[55], rnd, sh),
        hbtf(t, c!(t, cospi, 48), s7[39], c!(t, cospi, 16), s7[56], rnd, sh), hbtf(t, c!(t, cospi, 48), s7[38], c!(t, cospi, 16), s7[57], rnd, sh), hbtf(t, c!(t, cospi, 48), s7[37], c!(t, cospi, 16), s7[58], rnd, sh), hbtf(t, c!(t, cospi, 48), s7[36], c!(t, cospi, 16), s7[59], rnd, sh),
        s7[60], s7[61], s7[62], s7[63],
    ];
    // stage 9
    let s9: [__m256i; 64] = [
        cl(add!(s8[0], s8[15])), cl(add!(s8[1], s8[14])), cl(add!(s8[2], s8[13])), cl(add!(s8[3], s8[12])),
        cl(add!(s8[4], s8[11])), cl(add!(s8[5], s8[10])), cl(add!(s8[6], s8[9])), cl(add!(s8[7], s8[8])),
        cl(sub!(s8[7], s8[8])), cl(sub!(s8[6], s8[9])), cl(sub!(s8[5], s8[10])), cl(sub!(s8[4], s8[11])),
        cl(sub!(s8[3], s8[12])), cl(sub!(s8[2], s8[13])), cl(sub!(s8[1], s8[14])), cl(sub!(s8[0], s8[15])),
        s8[16], s8[17], s8[18], s8[19],
        hbtf(t, cn!(t, cospi, 32), s8[20], c!(t, cospi, 32), s8[27], rnd, sh), hbtf(t, cn!(t, cospi, 32), s8[21], c!(t, cospi, 32), s8[26], rnd, sh), hbtf(t, cn!(t, cospi, 32), s8[22], c!(t, cospi, 32), s8[25], rnd, sh), hbtf(t, cn!(t, cospi, 32), s8[23], c!(t, cospi, 32), s8[24], rnd, sh),
        hbtf(t, c!(t, cospi, 32), s8[23], c!(t, cospi, 32), s8[24], rnd, sh), hbtf(t, c!(t, cospi, 32), s8[22], c!(t, cospi, 32), s8[25], rnd, sh), hbtf(t, c!(t, cospi, 32), s8[21], c!(t, cospi, 32), s8[26], rnd, sh), hbtf(t, c!(t, cospi, 32), s8[20], c!(t, cospi, 32), s8[27], rnd, sh),
        s8[28], s8[29], s8[30], s8[31],
        cl(add!(s8[32], s8[47])), cl(add!(s8[33], s8[46])), cl(add!(s8[34], s8[45])), cl(add!(s8[35], s8[44])),
        cl(add!(s8[36], s8[43])), cl(add!(s8[37], s8[42])), cl(add!(s8[38], s8[41])), cl(add!(s8[39], s8[40])),
        cl(sub!(s8[39], s8[40])), cl(sub!(s8[38], s8[41])), cl(sub!(s8[37], s8[42])), cl(sub!(s8[36], s8[43])),
        cl(sub!(s8[35], s8[44])), cl(sub!(s8[34], s8[45])), cl(sub!(s8[33], s8[46])), cl(sub!(s8[32], s8[47])),
        cl(sub!(s8[63], s8[48])), cl(sub!(s8[62], s8[49])), cl(sub!(s8[61], s8[50])), cl(sub!(s8[60], s8[51])),
        cl(sub!(s8[59], s8[52])), cl(sub!(s8[58], s8[53])), cl(sub!(s8[57], s8[54])), cl(sub!(s8[56], s8[55])),
        cl(add!(s8[55], s8[56])), cl(add!(s8[54], s8[57])), cl(add!(s8[53], s8[58])), cl(add!(s8[52], s8[59])),
        cl(add!(s8[51], s8[60])), cl(add!(s8[50], s8[61])), cl(add!(s8[49], s8[62])), cl(add!(s8[48], s8[63])),
    ];
    // stage 10
    let s10: [__m256i; 64] = [
        cl(add!(s9[0], s9[31])), cl(add!(s9[1], s9[30])), cl(add!(s9[2], s9[29])), cl(add!(s9[3], s9[28])),
        cl(add!(s9[4], s9[27])), cl(add!(s9[5], s9[26])), cl(add!(s9[6], s9[25])), cl(add!(s9[7], s9[24])),
        cl(add!(s9[8], s9[23])), cl(add!(s9[9], s9[22])), cl(add!(s9[10], s9[21])), cl(add!(s9[11], s9[20])),
        cl(add!(s9[12], s9[19])), cl(add!(s9[13], s9[18])), cl(add!(s9[14], s9[17])), cl(add!(s9[15], s9[16])),
        cl(sub!(s9[15], s9[16])), cl(sub!(s9[14], s9[17])), cl(sub!(s9[13], s9[18])), cl(sub!(s9[12], s9[19])),
        cl(sub!(s9[11], s9[20])), cl(sub!(s9[10], s9[21])), cl(sub!(s9[9], s9[22])), cl(sub!(s9[8], s9[23])),
        cl(sub!(s9[7], s9[24])), cl(sub!(s9[6], s9[25])), cl(sub!(s9[5], s9[26])), cl(sub!(s9[4], s9[27])),
        cl(sub!(s9[3], s9[28])), cl(sub!(s9[2], s9[29])), cl(sub!(s9[1], s9[30])), cl(sub!(s9[0], s9[31])),
        s9[32], s9[33], s9[34], s9[35],
        s9[36], s9[37], s9[38], s9[39],
        hbtf(t, cn!(t, cospi, 32), s9[40], c!(t, cospi, 32), s9[55], rnd, sh), hbtf(t, cn!(t, cospi, 32), s9[41], c!(t, cospi, 32), s9[54], rnd, sh), hbtf(t, cn!(t, cospi, 32), s9[42], c!(t, cospi, 32), s9[53], rnd, sh), hbtf(t, cn!(t, cospi, 32), s9[43], c!(t, cospi, 32), s9[52], rnd, sh),
        hbtf(t, cn!(t, cospi, 32), s9[44], c!(t, cospi, 32), s9[51], rnd, sh), hbtf(t, cn!(t, cospi, 32), s9[45], c!(t, cospi, 32), s9[50], rnd, sh), hbtf(t, cn!(t, cospi, 32), s9[46], c!(t, cospi, 32), s9[49], rnd, sh), hbtf(t, cn!(t, cospi, 32), s9[47], c!(t, cospi, 32), s9[48], rnd, sh),
        hbtf(t, c!(t, cospi, 32), s9[47], c!(t, cospi, 32), s9[48], rnd, sh), hbtf(t, c!(t, cospi, 32), s9[46], c!(t, cospi, 32), s9[49], rnd, sh), hbtf(t, c!(t, cospi, 32), s9[45], c!(t, cospi, 32), s9[50], rnd, sh), hbtf(t, c!(t, cospi, 32), s9[44], c!(t, cospi, 32), s9[51], rnd, sh),
        hbtf(t, c!(t, cospi, 32), s9[43], c!(t, cospi, 32), s9[52], rnd, sh), hbtf(t, c!(t, cospi, 32), s9[42], c!(t, cospi, 32), s9[53], rnd, sh), hbtf(t, c!(t, cospi, 32), s9[41], c!(t, cospi, 32), s9[54], rnd, sh), hbtf(t, c!(t, cospi, 32), s9[40], c!(t, cospi, 32), s9[55], rnd, sh),
        s9[56], s9[57], s9[58], s9[59],
        s9[60], s9[61], s9[62], s9[63],
    ];
    // stage 11
    *out = [
        cl(add!(s10[0], s10[63])), cl(add!(s10[1], s10[62])), cl(add!(s10[2], s10[61])), cl(add!(s10[3], s10[60])),
        cl(add!(s10[4], s10[59])), cl(add!(s10[5], s10[58])), cl(add!(s10[6], s10[57])), cl(add!(s10[7], s10[56])),
        cl(add!(s10[8], s10[55])), cl(add!(s10[9], s10[54])), cl(add!(s10[10], s10[53])), cl(add!(s10[11], s10[52])),
        cl(add!(s10[12], s10[51])), cl(add!(s10[13], s10[50])), cl(add!(s10[14], s10[49])), cl(add!(s10[15], s10[48])),
        cl(add!(s10[16], s10[47])), cl(add!(s10[17], s10[46])), cl(add!(s10[18], s10[45])), cl(add!(s10[19], s10[44])),
        cl(add!(s10[20], s10[43])), cl(add!(s10[21], s10[42])), cl(add!(s10[22], s10[41])), cl(add!(s10[23], s10[40])),
        cl(add!(s10[24], s10[39])), cl(add!(s10[25], s10[38])), cl(add!(s10[26], s10[37])), cl(add!(s10[27], s10[36])),
        cl(add!(s10[28], s10[35])), cl(add!(s10[29], s10[34])), cl(add!(s10[30], s10[33])), cl(add!(s10[31], s10[32])),
        cl(sub!(s10[31], s10[32])), cl(sub!(s10[30], s10[33])), cl(sub!(s10[29], s10[34])), cl(sub!(s10[28], s10[35])),
        cl(sub!(s10[27], s10[36])), cl(sub!(s10[26], s10[37])), cl(sub!(s10[25], s10[38])), cl(sub!(s10[24], s10[39])),
        cl(sub!(s10[23], s10[40])), cl(sub!(s10[22], s10[41])), cl(sub!(s10[21], s10[42])), cl(sub!(s10[20], s10[43])),
        cl(sub!(s10[19], s10[44])), cl(sub!(s10[18], s10[45])), cl(sub!(s10[17], s10[46])), cl(sub!(s10[16], s10[47])),
        cl(sub!(s10[15], s10[48])), cl(sub!(s10[14], s10[49])), cl(sub!(s10[13], s10[50])), cl(sub!(s10[12], s10[51])),
        cl(sub!(s10[11], s10[52])), cl(sub!(s10[10], s10[53])), cl(sub!(s10[9], s10[54])), cl(sub!(s10[8], s10[55])),
        cl(sub!(s10[7], s10[56])), cl(sub!(s10[6], s10[57])), cl(sub!(s10[5], s10[58])), cl(sub!(s10[4], s10[59])),
        cl(sub!(s10[3], s10[60])), cl(sub!(s10[2], s10[61])), cl(sub!(s10[1], s10[62])), cl(sub!(s10[0], s10[63])),
    ];
}

#[rite]
pub(super) fn fdct64_x8(t: Desktop64, inp: &[__m256i; 64], out: &mut [__m256i; 64], cos_bit: i8) {
    let cospi = cospi_arr(cos_bit);
    let rnd = splat(t, 1 << (cos_bit as u32 - 1));
    let sh = _mm_cvtsi32_si128(cos_bit as i32);
    // stage 1
    let s1: [__m256i; 64] = [
        add!(inp[0], inp[63]), add!(inp[1], inp[62]), add!(inp[2], inp[61]), add!(inp[3], inp[60]),
        add!(inp[4], inp[59]), add!(inp[5], inp[58]), add!(inp[6], inp[57]), add!(inp[7], inp[56]),
        add!(inp[8], inp[55]), add!(inp[9], inp[54]), add!(inp[10], inp[53]), add!(inp[11], inp[52]),
        add!(inp[12], inp[51]), add!(inp[13], inp[50]), add!(inp[14], inp[49]), add!(inp[15], inp[48]),
        add!(inp[16], inp[47]), add!(inp[17], inp[46]), add!(inp[18], inp[45]), add!(inp[19], inp[44]),
        add!(inp[20], inp[43]), add!(inp[21], inp[42]), add!(inp[22], inp[41]), add!(inp[23], inp[40]),
        add!(inp[24], inp[39]), add!(inp[25], inp[38]), add!(inp[26], inp[37]), add!(inp[27], inp[36]),
        add!(inp[28], inp[35]), add!(inp[29], inp[34]), add!(inp[30], inp[33]), add!(inp[31], inp[32]),
        sub!(inp[31], inp[32]), sub!(inp[30], inp[33]), sub!(inp[29], inp[34]), sub!(inp[28], inp[35]),
        sub!(inp[27], inp[36]), sub!(inp[26], inp[37]), sub!(inp[25], inp[38]), sub!(inp[24], inp[39]),
        sub!(inp[23], inp[40]), sub!(inp[22], inp[41]), sub!(inp[21], inp[42]), sub!(inp[20], inp[43]),
        sub!(inp[19], inp[44]), sub!(inp[18], inp[45]), sub!(inp[17], inp[46]), sub!(inp[16], inp[47]),
        sub!(inp[15], inp[48]), sub!(inp[14], inp[49]), sub!(inp[13], inp[50]), sub!(inp[12], inp[51]),
        sub!(inp[11], inp[52]), sub!(inp[10], inp[53]), sub!(inp[9], inp[54]), sub!(inp[8], inp[55]),
        sub!(inp[7], inp[56]), sub!(inp[6], inp[57]), sub!(inp[5], inp[58]), sub!(inp[4], inp[59]),
        sub!(inp[3], inp[60]), sub!(inp[2], inp[61]), sub!(inp[1], inp[62]), sub!(inp[0], inp[63]),
    ];
    // stage 2
    let s2: [__m256i; 64] = [
        add!(s1[0], s1[31]), add!(s1[1], s1[30]), add!(s1[2], s1[29]), add!(s1[3], s1[28]),
        add!(s1[4], s1[27]), add!(s1[5], s1[26]), add!(s1[6], s1[25]), add!(s1[7], s1[24]),
        add!(s1[8], s1[23]), add!(s1[9], s1[22]), add!(s1[10], s1[21]), add!(s1[11], s1[20]),
        add!(s1[12], s1[19]), add!(s1[13], s1[18]), add!(s1[14], s1[17]), add!(s1[15], s1[16]),
        sub!(s1[15], s1[16]), sub!(s1[14], s1[17]), sub!(s1[13], s1[18]), sub!(s1[12], s1[19]),
        sub!(s1[11], s1[20]), sub!(s1[10], s1[21]), sub!(s1[9], s1[22]), sub!(s1[8], s1[23]),
        sub!(s1[7], s1[24]), sub!(s1[6], s1[25]), sub!(s1[5], s1[26]), sub!(s1[4], s1[27]),
        sub!(s1[3], s1[28]), sub!(s1[2], s1[29]), sub!(s1[1], s1[30]), sub!(s1[0], s1[31]),
        s1[32], s1[33], s1[34], s1[35],
        s1[36], s1[37], s1[38], s1[39],
        hbtf(t, cn!(t, cospi, 32), s1[40], c!(t, cospi, 32), s1[55], rnd, sh), hbtf(t, cn!(t, cospi, 32), s1[41], c!(t, cospi, 32), s1[54], rnd, sh), hbtf(t, cn!(t, cospi, 32), s1[42], c!(t, cospi, 32), s1[53], rnd, sh), hbtf(t, cn!(t, cospi, 32), s1[43], c!(t, cospi, 32), s1[52], rnd, sh),
        hbtf(t, cn!(t, cospi, 32), s1[44], c!(t, cospi, 32), s1[51], rnd, sh), hbtf(t, cn!(t, cospi, 32), s1[45], c!(t, cospi, 32), s1[50], rnd, sh), hbtf(t, cn!(t, cospi, 32), s1[46], c!(t, cospi, 32), s1[49], rnd, sh), hbtf(t, cn!(t, cospi, 32), s1[47], c!(t, cospi, 32), s1[48], rnd, sh),
        hbtf(t, c!(t, cospi, 32), s1[48], c!(t, cospi, 32), s1[47], rnd, sh), hbtf(t, c!(t, cospi, 32), s1[49], c!(t, cospi, 32), s1[46], rnd, sh), hbtf(t, c!(t, cospi, 32), s1[50], c!(t, cospi, 32), s1[45], rnd, sh), hbtf(t, c!(t, cospi, 32), s1[51], c!(t, cospi, 32), s1[44], rnd, sh),
        hbtf(t, c!(t, cospi, 32), s1[52], c!(t, cospi, 32), s1[43], rnd, sh), hbtf(t, c!(t, cospi, 32), s1[53], c!(t, cospi, 32), s1[42], rnd, sh), hbtf(t, c!(t, cospi, 32), s1[54], c!(t, cospi, 32), s1[41], rnd, sh), hbtf(t, c!(t, cospi, 32), s1[55], c!(t, cospi, 32), s1[40], rnd, sh),
        s1[56], s1[57], s1[58], s1[59],
        s1[60], s1[61], s1[62], s1[63],
    ];
    // stage 3
    let s3: [__m256i; 64] = [
        add!(s2[0], s2[15]), add!(s2[1], s2[14]), add!(s2[2], s2[13]), add!(s2[3], s2[12]),
        add!(s2[4], s2[11]), add!(s2[5], s2[10]), add!(s2[6], s2[9]), add!(s2[7], s2[8]),
        sub!(s2[7], s2[8]), sub!(s2[6], s2[9]), sub!(s2[5], s2[10]), sub!(s2[4], s2[11]),
        sub!(s2[3], s2[12]), sub!(s2[2], s2[13]), sub!(s2[1], s2[14]), sub!(s2[0], s2[15]),
        s2[16], s2[17], s2[18], s2[19],
        hbtf(t, cn!(t, cospi, 32), s2[20], c!(t, cospi, 32), s2[27], rnd, sh), hbtf(t, cn!(t, cospi, 32), s2[21], c!(t, cospi, 32), s2[26], rnd, sh), hbtf(t, cn!(t, cospi, 32), s2[22], c!(t, cospi, 32), s2[25], rnd, sh), hbtf(t, cn!(t, cospi, 32), s2[23], c!(t, cospi, 32), s2[24], rnd, sh),
        hbtf(t, c!(t, cospi, 32), s2[24], c!(t, cospi, 32), s2[23], rnd, sh), hbtf(t, c!(t, cospi, 32), s2[25], c!(t, cospi, 32), s2[22], rnd, sh), hbtf(t, c!(t, cospi, 32), s2[26], c!(t, cospi, 32), s2[21], rnd, sh), hbtf(t, c!(t, cospi, 32), s2[27], c!(t, cospi, 32), s2[20], rnd, sh),
        s2[28], s2[29], s2[30], s2[31],
        add!(s2[32], s2[47]), add!(s2[33], s2[46]), add!(s2[34], s2[45]), add!(s2[35], s2[44]),
        add!(s2[36], s2[43]), add!(s2[37], s2[42]), add!(s2[38], s2[41]), add!(s2[39], s2[40]),
        sub!(s2[39], s2[40]), sub!(s2[38], s2[41]), sub!(s2[37], s2[42]), sub!(s2[36], s2[43]),
        sub!(s2[35], s2[44]), sub!(s2[34], s2[45]), sub!(s2[33], s2[46]), sub!(s2[32], s2[47]),
        sub!(s2[63], s2[48]), sub!(s2[62], s2[49]), sub!(s2[61], s2[50]), sub!(s2[60], s2[51]),
        sub!(s2[59], s2[52]), sub!(s2[58], s2[53]), sub!(s2[57], s2[54]), sub!(s2[56], s2[55]),
        add!(s2[56], s2[55]), add!(s2[57], s2[54]), add!(s2[58], s2[53]), add!(s2[59], s2[52]),
        add!(s2[60], s2[51]), add!(s2[61], s2[50]), add!(s2[62], s2[49]), add!(s2[63], s2[48]),
    ];
    // stage 4
    let s4: [__m256i; 64] = [
        add!(s3[0], s3[7]), add!(s3[1], s3[6]), add!(s3[2], s3[5]), add!(s3[3], s3[4]),
        sub!(s3[3], s3[4]), sub!(s3[2], s3[5]), sub!(s3[1], s3[6]), sub!(s3[0], s3[7]),
        s3[8], s3[9], hbtf(t, cn!(t, cospi, 32), s3[10], c!(t, cospi, 32), s3[13], rnd, sh), hbtf(t, cn!(t, cospi, 32), s3[11], c!(t, cospi, 32), s3[12], rnd, sh),
        hbtf(t, c!(t, cospi, 32), s3[12], c!(t, cospi, 32), s3[11], rnd, sh), hbtf(t, c!(t, cospi, 32), s3[13], c!(t, cospi, 32), s3[10], rnd, sh), s3[14], s3[15],
        add!(s3[16], s3[23]), add!(s3[17], s3[22]), add!(s3[18], s3[21]), add!(s3[19], s3[20]),
        sub!(s3[19], s3[20]), sub!(s3[18], s3[21]), sub!(s3[17], s3[22]), sub!(s3[16], s3[23]),
        sub!(s3[31], s3[24]), sub!(s3[30], s3[25]), sub!(s3[29], s3[26]), sub!(s3[28], s3[27]),
        add!(s3[28], s3[27]), add!(s3[29], s3[26]), add!(s3[30], s3[25]), add!(s3[31], s3[24]),
        s3[32], s3[33], s3[34], s3[35],
        hbtf(t, cn!(t, cospi, 16), s3[36], c!(t, cospi, 48), s3[59], rnd, sh), hbtf(t, cn!(t, cospi, 16), s3[37], c!(t, cospi, 48), s3[58], rnd, sh), hbtf(t, cn!(t, cospi, 16), s3[38], c!(t, cospi, 48), s3[57], rnd, sh), hbtf(t, cn!(t, cospi, 16), s3[39], c!(t, cospi, 48), s3[56], rnd, sh),
        hbtf(t, cn!(t, cospi, 48), s3[40], cn!(t, cospi, 16), s3[55], rnd, sh), hbtf(t, cn!(t, cospi, 48), s3[41], cn!(t, cospi, 16), s3[54], rnd, sh), hbtf(t, cn!(t, cospi, 48), s3[42], cn!(t, cospi, 16), s3[53], rnd, sh), hbtf(t, cn!(t, cospi, 48), s3[43], cn!(t, cospi, 16), s3[52], rnd, sh),
        s3[44], s3[45], s3[46], s3[47],
        s3[48], s3[49], s3[50], s3[51],
        hbtf(t, c!(t, cospi, 48), s3[52], cn!(t, cospi, 16), s3[43], rnd, sh), hbtf(t, c!(t, cospi, 48), s3[53], cn!(t, cospi, 16), s3[42], rnd, sh), hbtf(t, c!(t, cospi, 48), s3[54], cn!(t, cospi, 16), s3[41], rnd, sh), hbtf(t, c!(t, cospi, 48), s3[55], cn!(t, cospi, 16), s3[40], rnd, sh),
        hbtf(t, c!(t, cospi, 16), s3[56], c!(t, cospi, 48), s3[39], rnd, sh), hbtf(t, c!(t, cospi, 16), s3[57], c!(t, cospi, 48), s3[38], rnd, sh), hbtf(t, c!(t, cospi, 16), s3[58], c!(t, cospi, 48), s3[37], rnd, sh), hbtf(t, c!(t, cospi, 16), s3[59], c!(t, cospi, 48), s3[36], rnd, sh),
        s3[60], s3[61], s3[62], s3[63],
    ];
    // stage 5
    let s5: [__m256i; 64] = [
        add!(s4[0], s4[3]), add!(s4[1], s4[2]), sub!(s4[1], s4[2]), sub!(s4[0], s4[3]),
        s4[4], hbtf(t, cn!(t, cospi, 32), s4[5], c!(t, cospi, 32), s4[6], rnd, sh), hbtf(t, c!(t, cospi, 32), s4[6], c!(t, cospi, 32), s4[5], rnd, sh), s4[7],
        add!(s4[8], s4[11]), add!(s4[9], s4[10]), sub!(s4[9], s4[10]), sub!(s4[8], s4[11]),
        sub!(s4[15], s4[12]), sub!(s4[14], s4[13]), add!(s4[14], s4[13]), add!(s4[15], s4[12]),
        s4[16], s4[17], hbtf(t, cn!(t, cospi, 16), s4[18], c!(t, cospi, 48), s4[29], rnd, sh), hbtf(t, cn!(t, cospi, 16), s4[19], c!(t, cospi, 48), s4[28], rnd, sh),
        hbtf(t, cn!(t, cospi, 48), s4[20], cn!(t, cospi, 16), s4[27], rnd, sh), hbtf(t, cn!(t, cospi, 48), s4[21], cn!(t, cospi, 16), s4[26], rnd, sh), s4[22], s4[23],
        s4[24], s4[25], hbtf(t, c!(t, cospi, 48), s4[26], cn!(t, cospi, 16), s4[21], rnd, sh), hbtf(t, c!(t, cospi, 48), s4[27], cn!(t, cospi, 16), s4[20], rnd, sh),
        hbtf(t, c!(t, cospi, 16), s4[28], c!(t, cospi, 48), s4[19], rnd, sh), hbtf(t, c!(t, cospi, 16), s4[29], c!(t, cospi, 48), s4[18], rnd, sh), s4[30], s4[31],
        add!(s4[32], s4[39]), add!(s4[33], s4[38]), add!(s4[34], s4[37]), add!(s4[35], s4[36]),
        sub!(s4[35], s4[36]), sub!(s4[34], s4[37]), sub!(s4[33], s4[38]), sub!(s4[32], s4[39]),
        sub!(s4[47], s4[40]), sub!(s4[46], s4[41]), sub!(s4[45], s4[42]), sub!(s4[44], s4[43]),
        add!(s4[44], s4[43]), add!(s4[45], s4[42]), add!(s4[46], s4[41]), add!(s4[47], s4[40]),
        add!(s4[48], s4[55]), add!(s4[49], s4[54]), add!(s4[50], s4[53]), add!(s4[51], s4[52]),
        sub!(s4[51], s4[52]), sub!(s4[50], s4[53]), sub!(s4[49], s4[54]), sub!(s4[48], s4[55]),
        sub!(s4[63], s4[56]), sub!(s4[62], s4[57]), sub!(s4[61], s4[58]), sub!(s4[60], s4[59]),
        add!(s4[60], s4[59]), add!(s4[61], s4[58]), add!(s4[62], s4[57]), add!(s4[63], s4[56]),
    ];
    // stage 6
    let s6: [__m256i; 64] = [
        hbtf(t, c!(t, cospi, 32), s5[0], c!(t, cospi, 32), s5[1], rnd, sh), hbtf(t, cn!(t, cospi, 32), s5[1], c!(t, cospi, 32), s5[0], rnd, sh), hbtf(t, c!(t, cospi, 48), s5[2], c!(t, cospi, 16), s5[3], rnd, sh), hbtf(t, c!(t, cospi, 48), s5[3], cn!(t, cospi, 16), s5[2], rnd, sh),
        add!(s5[4], s5[5]), sub!(s5[4], s5[5]), sub!(s5[7], s5[6]), add!(s5[7], s5[6]),
        s5[8], hbtf(t, cn!(t, cospi, 16), s5[9], c!(t, cospi, 48), s5[14], rnd, sh), hbtf(t, cn!(t, cospi, 48), s5[10], cn!(t, cospi, 16), s5[13], rnd, sh), s5[11],
        s5[12], hbtf(t, c!(t, cospi, 48), s5[13], cn!(t, cospi, 16), s5[10], rnd, sh), hbtf(t, c!(t, cospi, 16), s5[14], c!(t, cospi, 48), s5[9], rnd, sh), s5[15],
        add!(s5[16], s5[19]), add!(s5[17], s5[18]), sub!(s5[17], s5[18]), sub!(s5[16], s5[19]),
        sub!(s5[23], s5[20]), sub!(s5[22], s5[21]), add!(s5[22], s5[21]), add!(s5[23], s5[20]),
        add!(s5[24], s5[27]), add!(s5[25], s5[26]), sub!(s5[25], s5[26]), sub!(s5[24], s5[27]),
        sub!(s5[31], s5[28]), sub!(s5[30], s5[29]), add!(s5[30], s5[29]), add!(s5[31], s5[28]),
        s5[32], s5[33], hbtf(t, cn!(t, cospi, 8), s5[34], c!(t, cospi, 56), s5[61], rnd, sh), hbtf(t, cn!(t, cospi, 8), s5[35], c!(t, cospi, 56), s5[60], rnd, sh),
        hbtf(t, cn!(t, cospi, 56), s5[36], cn!(t, cospi, 8), s5[59], rnd, sh), hbtf(t, cn!(t, cospi, 56), s5[37], cn!(t, cospi, 8), s5[58], rnd, sh), s5[38], s5[39],
        s5[40], s5[41], hbtf(t, cn!(t, cospi, 40), s5[42], c!(t, cospi, 24), s5[53], rnd, sh), hbtf(t, cn!(t, cospi, 40), s5[43], c!(t, cospi, 24), s5[52], rnd, sh),
        hbtf(t, cn!(t, cospi, 24), s5[44], cn!(t, cospi, 40), s5[51], rnd, sh), hbtf(t, cn!(t, cospi, 24), s5[45], cn!(t, cospi, 40), s5[50], rnd, sh), s5[46], s5[47],
        s5[48], s5[49], hbtf(t, c!(t, cospi, 24), s5[50], cn!(t, cospi, 40), s5[45], rnd, sh), hbtf(t, c!(t, cospi, 24), s5[51], cn!(t, cospi, 40), s5[44], rnd, sh),
        hbtf(t, c!(t, cospi, 40), s5[52], c!(t, cospi, 24), s5[43], rnd, sh), hbtf(t, c!(t, cospi, 40), s5[53], c!(t, cospi, 24), s5[42], rnd, sh), s5[54], s5[55],
        s5[56], s5[57], hbtf(t, c!(t, cospi, 56), s5[58], cn!(t, cospi, 8), s5[37], rnd, sh), hbtf(t, c!(t, cospi, 56), s5[59], cn!(t, cospi, 8), s5[36], rnd, sh),
        hbtf(t, c!(t, cospi, 8), s5[60], c!(t, cospi, 56), s5[35], rnd, sh), hbtf(t, c!(t, cospi, 8), s5[61], c!(t, cospi, 56), s5[34], rnd, sh), s5[62], s5[63],
    ];
    // stage 7
    let s7: [__m256i; 64] = [
        s6[0], s6[1], s6[2], s6[3],
        hbtf(t, c!(t, cospi, 56), s6[4], c!(t, cospi, 8), s6[7], rnd, sh), hbtf(t, c!(t, cospi, 24), s6[5], c!(t, cospi, 40), s6[6], rnd, sh), hbtf(t, c!(t, cospi, 24), s6[6], cn!(t, cospi, 40), s6[5], rnd, sh), hbtf(t, c!(t, cospi, 56), s6[7], cn!(t, cospi, 8), s6[4], rnd, sh),
        add!(s6[8], s6[9]), sub!(s6[8], s6[9]), sub!(s6[11], s6[10]), add!(s6[11], s6[10]),
        add!(s6[12], s6[13]), sub!(s6[12], s6[13]), sub!(s6[15], s6[14]), add!(s6[15], s6[14]),
        s6[16], hbtf(t, cn!(t, cospi, 8), s6[17], c!(t, cospi, 56), s6[30], rnd, sh), hbtf(t, cn!(t, cospi, 56), s6[18], cn!(t, cospi, 8), s6[29], rnd, sh), s6[19],
        s6[20], hbtf(t, cn!(t, cospi, 40), s6[21], c!(t, cospi, 24), s6[26], rnd, sh), hbtf(t, cn!(t, cospi, 24), s6[22], cn!(t, cospi, 40), s6[25], rnd, sh), s6[23],
        s6[24], hbtf(t, c!(t, cospi, 24), s6[25], cn!(t, cospi, 40), s6[22], rnd, sh), hbtf(t, c!(t, cospi, 40), s6[26], c!(t, cospi, 24), s6[21], rnd, sh), s6[27],
        s6[28], hbtf(t, c!(t, cospi, 56), s6[29], cn!(t, cospi, 8), s6[18], rnd, sh), hbtf(t, c!(t, cospi, 8), s6[30], c!(t, cospi, 56), s6[17], rnd, sh), s6[31],
        add!(s6[32], s6[35]), add!(s6[33], s6[34]), sub!(s6[33], s6[34]), sub!(s6[32], s6[35]),
        sub!(s6[39], s6[36]), sub!(s6[38], s6[37]), add!(s6[38], s6[37]), add!(s6[39], s6[36]),
        add!(s6[40], s6[43]), add!(s6[41], s6[42]), sub!(s6[41], s6[42]), sub!(s6[40], s6[43]),
        sub!(s6[47], s6[44]), sub!(s6[46], s6[45]), add!(s6[46], s6[45]), add!(s6[47], s6[44]),
        add!(s6[48], s6[51]), add!(s6[49], s6[50]), sub!(s6[49], s6[50]), sub!(s6[48], s6[51]),
        sub!(s6[55], s6[52]), sub!(s6[54], s6[53]), add!(s6[54], s6[53]), add!(s6[55], s6[52]),
        add!(s6[56], s6[59]), add!(s6[57], s6[58]), sub!(s6[57], s6[58]), sub!(s6[56], s6[59]),
        sub!(s6[63], s6[60]), sub!(s6[62], s6[61]), add!(s6[62], s6[61]), add!(s6[63], s6[60]),
    ];
    // stage 8
    let s8: [__m256i; 64] = [
        s7[0], s7[1], s7[2], s7[3],
        s7[4], s7[5], s7[6], s7[7],
        hbtf(t, c!(t, cospi, 60), s7[8], c!(t, cospi, 4), s7[15], rnd, sh), hbtf(t, c!(t, cospi, 28), s7[9], c!(t, cospi, 36), s7[14], rnd, sh), hbtf(t, c!(t, cospi, 44), s7[10], c!(t, cospi, 20), s7[13], rnd, sh), hbtf(t, c!(t, cospi, 12), s7[11], c!(t, cospi, 52), s7[12], rnd, sh),
        hbtf(t, c!(t, cospi, 12), s7[12], cn!(t, cospi, 52), s7[11], rnd, sh), hbtf(t, c!(t, cospi, 44), s7[13], cn!(t, cospi, 20), s7[10], rnd, sh), hbtf(t, c!(t, cospi, 28), s7[14], cn!(t, cospi, 36), s7[9], rnd, sh), hbtf(t, c!(t, cospi, 60), s7[15], cn!(t, cospi, 4), s7[8], rnd, sh),
        add!(s7[16], s7[17]), sub!(s7[16], s7[17]), sub!(s7[19], s7[18]), add!(s7[19], s7[18]),
        add!(s7[20], s7[21]), sub!(s7[20], s7[21]), sub!(s7[23], s7[22]), add!(s7[23], s7[22]),
        add!(s7[24], s7[25]), sub!(s7[24], s7[25]), sub!(s7[27], s7[26]), add!(s7[27], s7[26]),
        add!(s7[28], s7[29]), sub!(s7[28], s7[29]), sub!(s7[31], s7[30]), add!(s7[31], s7[30]),
        s7[32], hbtf(t, cn!(t, cospi, 4), s7[33], c!(t, cospi, 60), s7[62], rnd, sh), hbtf(t, cn!(t, cospi, 60), s7[34], cn!(t, cospi, 4), s7[61], rnd, sh), s7[35],
        s7[36], hbtf(t, cn!(t, cospi, 36), s7[37], c!(t, cospi, 28), s7[58], rnd, sh), hbtf(t, cn!(t, cospi, 28), s7[38], cn!(t, cospi, 36), s7[57], rnd, sh), s7[39],
        s7[40], hbtf(t, cn!(t, cospi, 20), s7[41], c!(t, cospi, 44), s7[54], rnd, sh), hbtf(t, cn!(t, cospi, 44), s7[42], cn!(t, cospi, 20), s7[53], rnd, sh), s7[43],
        s7[44], hbtf(t, cn!(t, cospi, 52), s7[45], c!(t, cospi, 12), s7[50], rnd, sh), hbtf(t, cn!(t, cospi, 12), s7[46], cn!(t, cospi, 52), s7[49], rnd, sh), s7[47],
        s7[48], hbtf(t, c!(t, cospi, 12), s7[49], cn!(t, cospi, 52), s7[46], rnd, sh), hbtf(t, c!(t, cospi, 52), s7[50], c!(t, cospi, 12), s7[45], rnd, sh), s7[51],
        s7[52], hbtf(t, c!(t, cospi, 44), s7[53], cn!(t, cospi, 20), s7[42], rnd, sh), hbtf(t, c!(t, cospi, 20), s7[54], c!(t, cospi, 44), s7[41], rnd, sh), s7[55],
        s7[56], hbtf(t, c!(t, cospi, 28), s7[57], cn!(t, cospi, 36), s7[38], rnd, sh), hbtf(t, c!(t, cospi, 36), s7[58], c!(t, cospi, 28), s7[37], rnd, sh), s7[59],
        s7[60], hbtf(t, c!(t, cospi, 60), s7[61], cn!(t, cospi, 4), s7[34], rnd, sh), hbtf(t, c!(t, cospi, 4), s7[62], c!(t, cospi, 60), s7[33], rnd, sh), s7[63],
    ];
    // stage 9
    let s9: [__m256i; 64] = [
        s8[0], s8[1], s8[2], s8[3],
        s8[4], s8[5], s8[6], s8[7],
        s8[8], s8[9], s8[10], s8[11],
        s8[12], s8[13], s8[14], s8[15],
        hbtf(t, c!(t, cospi, 62), s8[16], c!(t, cospi, 2), s8[31], rnd, sh), hbtf(t, c!(t, cospi, 30), s8[17], c!(t, cospi, 34), s8[30], rnd, sh), hbtf(t, c!(t, cospi, 46), s8[18], c!(t, cospi, 18), s8[29], rnd, sh), hbtf(t, c!(t, cospi, 14), s8[19], c!(t, cospi, 50), s8[28], rnd, sh),
        hbtf(t, c!(t, cospi, 54), s8[20], c!(t, cospi, 10), s8[27], rnd, sh), hbtf(t, c!(t, cospi, 22), s8[21], c!(t, cospi, 42), s8[26], rnd, sh), hbtf(t, c!(t, cospi, 38), s8[22], c!(t, cospi, 26), s8[25], rnd, sh), hbtf(t, c!(t, cospi, 6), s8[23], c!(t, cospi, 58), s8[24], rnd, sh),
        hbtf(t, c!(t, cospi, 6), s8[24], cn!(t, cospi, 58), s8[23], rnd, sh), hbtf(t, c!(t, cospi, 38), s8[25], cn!(t, cospi, 26), s8[22], rnd, sh), hbtf(t, c!(t, cospi, 22), s8[26], cn!(t, cospi, 42), s8[21], rnd, sh), hbtf(t, c!(t, cospi, 54), s8[27], cn!(t, cospi, 10), s8[20], rnd, sh),
        hbtf(t, c!(t, cospi, 14), s8[28], cn!(t, cospi, 50), s8[19], rnd, sh), hbtf(t, c!(t, cospi, 46), s8[29], cn!(t, cospi, 18), s8[18], rnd, sh), hbtf(t, c!(t, cospi, 30), s8[30], cn!(t, cospi, 34), s8[17], rnd, sh), hbtf(t, c!(t, cospi, 62), s8[31], cn!(t, cospi, 2), s8[16], rnd, sh),
        add!(s8[32], s8[33]), sub!(s8[32], s8[33]), sub!(s8[35], s8[34]), add!(s8[35], s8[34]),
        add!(s8[36], s8[37]), sub!(s8[36], s8[37]), sub!(s8[39], s8[38]), add!(s8[39], s8[38]),
        add!(s8[40], s8[41]), sub!(s8[40], s8[41]), sub!(s8[43], s8[42]), add!(s8[43], s8[42]),
        add!(s8[44], s8[45]), sub!(s8[44], s8[45]), sub!(s8[47], s8[46]), add!(s8[47], s8[46]),
        add!(s8[48], s8[49]), sub!(s8[48], s8[49]), sub!(s8[51], s8[50]), add!(s8[51], s8[50]),
        add!(s8[52], s8[53]), sub!(s8[52], s8[53]), sub!(s8[55], s8[54]), add!(s8[55], s8[54]),
        add!(s8[56], s8[57]), sub!(s8[56], s8[57]), sub!(s8[59], s8[58]), add!(s8[59], s8[58]),
        add!(s8[60], s8[61]), sub!(s8[60], s8[61]), sub!(s8[63], s8[62]), add!(s8[63], s8[62]),
    ];
    // stage 10
    let s10: [__m256i; 64] = [
        s9[0], s9[1], s9[2], s9[3],
        s9[4], s9[5], s9[6], s9[7],
        s9[8], s9[9], s9[10], s9[11],
        s9[12], s9[13], s9[14], s9[15],
        s9[16], s9[17], s9[18], s9[19],
        s9[20], s9[21], s9[22], s9[23],
        s9[24], s9[25], s9[26], s9[27],
        s9[28], s9[29], s9[30], s9[31],
        hbtf(t, c!(t, cospi, 63), s9[32], c!(t, cospi, 1), s9[63], rnd, sh), hbtf(t, c!(t, cospi, 31), s9[33], c!(t, cospi, 33), s9[62], rnd, sh), hbtf(t, c!(t, cospi, 47), s9[34], c!(t, cospi, 17), s9[61], rnd, sh), hbtf(t, c!(t, cospi, 15), s9[35], c!(t, cospi, 49), s9[60], rnd, sh),
        hbtf(t, c!(t, cospi, 55), s9[36], c!(t, cospi, 9), s9[59], rnd, sh), hbtf(t, c!(t, cospi, 23), s9[37], c!(t, cospi, 41), s9[58], rnd, sh), hbtf(t, c!(t, cospi, 39), s9[38], c!(t, cospi, 25), s9[57], rnd, sh), hbtf(t, c!(t, cospi, 7), s9[39], c!(t, cospi, 57), s9[56], rnd, sh),
        hbtf(t, c!(t, cospi, 59), s9[40], c!(t, cospi, 5), s9[55], rnd, sh), hbtf(t, c!(t, cospi, 27), s9[41], c!(t, cospi, 37), s9[54], rnd, sh), hbtf(t, c!(t, cospi, 43), s9[42], c!(t, cospi, 21), s9[53], rnd, sh), hbtf(t, c!(t, cospi, 11), s9[43], c!(t, cospi, 53), s9[52], rnd, sh),
        hbtf(t, c!(t, cospi, 51), s9[44], c!(t, cospi, 13), s9[51], rnd, sh), hbtf(t, c!(t, cospi, 19), s9[45], c!(t, cospi, 45), s9[50], rnd, sh), hbtf(t, c!(t, cospi, 35), s9[46], c!(t, cospi, 29), s9[49], rnd, sh), hbtf(t, c!(t, cospi, 3), s9[47], c!(t, cospi, 61), s9[48], rnd, sh),
        hbtf(t, c!(t, cospi, 3), s9[48], cn!(t, cospi, 61), s9[47], rnd, sh), hbtf(t, c!(t, cospi, 35), s9[49], cn!(t, cospi, 29), s9[46], rnd, sh), hbtf(t, c!(t, cospi, 19), s9[50], cn!(t, cospi, 45), s9[45], rnd, sh), hbtf(t, c!(t, cospi, 51), s9[51], cn!(t, cospi, 13), s9[44], rnd, sh),
        hbtf(t, c!(t, cospi, 11), s9[52], cn!(t, cospi, 53), s9[43], rnd, sh), hbtf(t, c!(t, cospi, 43), s9[53], cn!(t, cospi, 21), s9[42], rnd, sh), hbtf(t, c!(t, cospi, 27), s9[54], cn!(t, cospi, 37), s9[41], rnd, sh), hbtf(t, c!(t, cospi, 59), s9[55], cn!(t, cospi, 5), s9[40], rnd, sh),
        hbtf(t, c!(t, cospi, 7), s9[56], cn!(t, cospi, 57), s9[39], rnd, sh), hbtf(t, c!(t, cospi, 39), s9[57], cn!(t, cospi, 25), s9[38], rnd, sh), hbtf(t, c!(t, cospi, 23), s9[58], cn!(t, cospi, 41), s9[37], rnd, sh), hbtf(t, c!(t, cospi, 55), s9[59], cn!(t, cospi, 9), s9[36], rnd, sh),
        hbtf(t, c!(t, cospi, 15), s9[60], cn!(t, cospi, 49), s9[35], rnd, sh), hbtf(t, c!(t, cospi, 47), s9[61], cn!(t, cospi, 17), s9[34], rnd, sh), hbtf(t, c!(t, cospi, 31), s9[62], cn!(t, cospi, 33), s9[33], rnd, sh), hbtf(t, c!(t, cospi, 63), s9[63], cn!(t, cospi, 1), s9[32], rnd, sh),
    ];
    // stage 11
    *out = [
        s10[0], s10[32], s10[16], s10[48],
        s10[8], s10[40], s10[24], s10[56],
        s10[4], s10[36], s10[20], s10[52],
        s10[12], s10[44], s10[28], s10[60],
        s10[2], s10[34], s10[18], s10[50],
        s10[10], s10[42], s10[26], s10[58],
        s10[6], s10[38], s10[22], s10[54],
        s10[14], s10[46], s10[30], s10[62],
        s10[1], s10[33], s10[17], s10[49],
        s10[9], s10[41], s10[25], s10[57],
        s10[5], s10[37], s10[21], s10[53],
        s10[13], s10[45], s10[29], s10[61],
        s10[3], s10[35], s10[19], s10[51],
        s10[11], s10[43], s10[27], s10[59],
        s10[7], s10[39], s10[23], s10[55],
        s10[15], s10[47], s10[31], s10[63],
    ];
}

// ===========================================================================
// ADST kernels — hand-transcribed op-for-op from the scalar `fadst*` /
// `iadst*` (fwd_txfm.rs / inv_txfm.rs), in the same explicit-per-stage form as
// the DCT kernels above. The ADST butterfly is asymmetric (sinpi-free at 8/16;
// the 4-point sinpi ADST stays scalar) but every stage is still `neg` / `add` /
// `sub` / `half_btf` / `clamp_value`, so the vector op sequence is bit-identical
// (see module docs). `c_parity_txfm.rs` proves it byte-exact vs real C.
// ---------------------------------------------------------------------------
// 8-point forward ADST (svt_av1_fadst8_new / fwd_txfm.rs::fadst8). No clamps.
// ---------------------------------------------------------------------------
#[rite]
pub(super) fn fadst8_x8(t: Desktop64, inp: &[__m256i; 8], out: &mut [__m256i; 8], cos_bit: i8) {
    let cospi = cospi_arr(cos_bit);
    let rnd = splat(t, 1 << (cos_bit as u32 - 1));
    let sh = _mm_cvtsi32_si128(cos_bit as i32);
    // stage 1: input permutation with sign flips
    let s1: [__m256i; 8] = [
        inp[0], neg!(inp[7]), neg!(inp[3]), inp[4],
        neg!(inp[1]), inp[6], inp[2], neg!(inp[5]),
    ];
    // stage 2
    let s2: [__m256i; 8] = [
        s1[0], s1[1],
        hbtf(t, c!(t, cospi, 32), s1[2], c!(t, cospi, 32), s1[3], rnd, sh),
        hbtf(t, c!(t, cospi, 32), s1[2], cn!(t, cospi, 32), s1[3], rnd, sh),
        s1[4], s1[5],
        hbtf(t, c!(t, cospi, 32), s1[6], c!(t, cospi, 32), s1[7], rnd, sh),
        hbtf(t, c!(t, cospi, 32), s1[6], cn!(t, cospi, 32), s1[7], rnd, sh),
    ];
    // stage 3
    let s3: [__m256i; 8] = [
        add!(s2[0], s2[2]), add!(s2[1], s2[3]), sub!(s2[0], s2[2]), sub!(s2[1], s2[3]),
        add!(s2[4], s2[6]), add!(s2[5], s2[7]), sub!(s2[4], s2[6]), sub!(s2[5], s2[7]),
    ];
    // stage 4
    let s4: [__m256i; 8] = [
        s3[0], s3[1], s3[2], s3[3],
        hbtf(t, c!(t, cospi, 16), s3[4], c!(t, cospi, 48), s3[5], rnd, sh),
        hbtf(t, c!(t, cospi, 48), s3[4], cn!(t, cospi, 16), s3[5], rnd, sh),
        hbtf(t, cn!(t, cospi, 48), s3[6], c!(t, cospi, 16), s3[7], rnd, sh),
        hbtf(t, c!(t, cospi, 16), s3[6], c!(t, cospi, 48), s3[7], rnd, sh),
    ];
    // stage 5
    let s5: [__m256i; 8] = [
        add!(s4[0], s4[4]), add!(s4[1], s4[5]), add!(s4[2], s4[6]), add!(s4[3], s4[7]),
        sub!(s4[0], s4[4]), sub!(s4[1], s4[5]), sub!(s4[2], s4[6]), sub!(s4[3], s4[7]),
    ];
    // stage 6
    let s6: [__m256i; 8] = [
        hbtf(t, c!(t, cospi, 4), s5[0], c!(t, cospi, 60), s5[1], rnd, sh),
        hbtf(t, c!(t, cospi, 60), s5[0], cn!(t, cospi, 4), s5[1], rnd, sh),
        hbtf(t, c!(t, cospi, 20), s5[2], c!(t, cospi, 44), s5[3], rnd, sh),
        hbtf(t, c!(t, cospi, 44), s5[2], cn!(t, cospi, 20), s5[3], rnd, sh),
        hbtf(t, c!(t, cospi, 36), s5[4], c!(t, cospi, 28), s5[5], rnd, sh),
        hbtf(t, c!(t, cospi, 28), s5[4], cn!(t, cospi, 36), s5[5], rnd, sh),
        hbtf(t, c!(t, cospi, 52), s5[6], c!(t, cospi, 12), s5[7], rnd, sh),
        hbtf(t, c!(t, cospi, 12), s5[6], cn!(t, cospi, 52), s5[7], rnd, sh),
    ];
    // stage 7: output permutation
    *out = [s6[1], s6[6], s6[3], s6[4], s6[5], s6[2], s6[7], s6[0]];
}

// ---------------------------------------------------------------------------
// 8-point inverse ADST (svt_av1_iadst8_new / inv_txfm.rs::iadst8).
// ---------------------------------------------------------------------------
#[rite]
pub(super) fn iadst8_x8(
    t: Desktop64,
    inp: &[__m256i; 8],
    out: &mut [__m256i; 8],
    rnd: __m256i,
    sh: __m128i,
    lo: __m256i,
    hi: __m256i,
) {
    let cospi = &COSPI;
    let cl = |v| clampv(t, v, lo, hi);
    // stage 1: input permutation
    let s1: [__m256i; 8] = [
        inp[7], inp[0], inp[5], inp[2], inp[3], inp[4], inp[1], inp[6],
    ];
    // stage 2
    let s2: [__m256i; 8] = [
        hbtf(t, c!(t, cospi, 4), s1[0], c!(t, cospi, 60), s1[1], rnd, sh),
        hbtf(t, c!(t, cospi, 60), s1[0], cn!(t, cospi, 4), s1[1], rnd, sh),
        hbtf(t, c!(t, cospi, 20), s1[2], c!(t, cospi, 44), s1[3], rnd, sh),
        hbtf(t, c!(t, cospi, 44), s1[2], cn!(t, cospi, 20), s1[3], rnd, sh),
        hbtf(t, c!(t, cospi, 36), s1[4], c!(t, cospi, 28), s1[5], rnd, sh),
        hbtf(t, c!(t, cospi, 28), s1[4], cn!(t, cospi, 36), s1[5], rnd, sh),
        hbtf(t, c!(t, cospi, 52), s1[6], c!(t, cospi, 12), s1[7], rnd, sh),
        hbtf(t, c!(t, cospi, 12), s1[6], cn!(t, cospi, 52), s1[7], rnd, sh),
    ];
    // stage 3
    let s3: [__m256i; 8] = [
        cl(add!(s2[0], s2[4])), cl(add!(s2[1], s2[5])), cl(add!(s2[2], s2[6])), cl(add!(s2[3], s2[7])),
        cl(sub!(s2[0], s2[4])), cl(sub!(s2[1], s2[5])), cl(sub!(s2[2], s2[6])), cl(sub!(s2[3], s2[7])),
    ];
    // stage 4
    let s4: [__m256i; 8] = [
        s3[0], s3[1], s3[2], s3[3],
        hbtf(t, c!(t, cospi, 16), s3[4], c!(t, cospi, 48), s3[5], rnd, sh),
        hbtf(t, c!(t, cospi, 48), s3[4], cn!(t, cospi, 16), s3[5], rnd, sh),
        hbtf(t, cn!(t, cospi, 48), s3[6], c!(t, cospi, 16), s3[7], rnd, sh),
        hbtf(t, c!(t, cospi, 16), s3[6], c!(t, cospi, 48), s3[7], rnd, sh),
    ];
    // stage 5
    let s5: [__m256i; 8] = [
        cl(add!(s4[0], s4[2])), cl(add!(s4[1], s4[3])), cl(sub!(s4[0], s4[2])), cl(sub!(s4[1], s4[3])),
        cl(add!(s4[4], s4[6])), cl(add!(s4[5], s4[7])), cl(sub!(s4[4], s4[6])), cl(sub!(s4[5], s4[7])),
    ];
    // stage 6
    let s6: [__m256i; 8] = [
        s5[0], s5[1],
        hbtf(t, c!(t, cospi, 32), s5[2], c!(t, cospi, 32), s5[3], rnd, sh),
        hbtf(t, c!(t, cospi, 32), s5[2], cn!(t, cospi, 32), s5[3], rnd, sh),
        s5[4], s5[5],
        hbtf(t, c!(t, cospi, 32), s5[6], c!(t, cospi, 32), s5[7], rnd, sh),
        hbtf(t, c!(t, cospi, 32), s5[6], cn!(t, cospi, 32), s5[7], rnd, sh),
    ];
    // stage 7: output with negations
    *out = [s6[0], neg!(s6[4]), s6[6], neg!(s6[2]), s6[3], neg!(s6[7]), s6[5], neg!(s6[1])];
}

// ---------------------------------------------------------------------------
// 16-point forward ADST (svt_av1_fadst16_new / fwd_txfm.rs::fadst16). No clamps.
// ---------------------------------------------------------------------------
#[rite]
pub(super) fn fadst16_x8(t: Desktop64, inp: &[__m256i; 16], out: &mut [__m256i; 16], cos_bit: i8) {
    let cospi = cospi_arr(cos_bit);
    let rnd = splat(t, 1 << (cos_bit as u32 - 1));
    let sh = _mm_cvtsi32_si128(cos_bit as i32);
    // stage 1: input permutation with sign flips
    let s1: [__m256i; 16] = [
        inp[0], neg!(inp[15]), neg!(inp[7]), inp[8],
        neg!(inp[3]), inp[12], inp[4], neg!(inp[11]),
        neg!(inp[1]), inp[14], inp[6], neg!(inp[9]),
        inp[2], neg!(inp[13]), neg!(inp[5]), inp[10],
    ];
    // stage 2
    let s2: [__m256i; 16] = [
        s1[0], s1[1],
        hbtf(t, c!(t, cospi, 32), s1[2], c!(t, cospi, 32), s1[3], rnd, sh),
        hbtf(t, c!(t, cospi, 32), s1[2], cn!(t, cospi, 32), s1[3], rnd, sh),
        s1[4], s1[5],
        hbtf(t, c!(t, cospi, 32), s1[6], c!(t, cospi, 32), s1[7], rnd, sh),
        hbtf(t, c!(t, cospi, 32), s1[6], cn!(t, cospi, 32), s1[7], rnd, sh),
        s1[8], s1[9],
        hbtf(t, c!(t, cospi, 32), s1[10], c!(t, cospi, 32), s1[11], rnd, sh),
        hbtf(t, c!(t, cospi, 32), s1[10], cn!(t, cospi, 32), s1[11], rnd, sh),
        s1[12], s1[13],
        hbtf(t, c!(t, cospi, 32), s1[14], c!(t, cospi, 32), s1[15], rnd, sh),
        hbtf(t, c!(t, cospi, 32), s1[14], cn!(t, cospi, 32), s1[15], rnd, sh),
    ];
    // stage 3
    let s3: [__m256i; 16] = [
        add!(s2[0], s2[2]), add!(s2[1], s2[3]), sub!(s2[0], s2[2]), sub!(s2[1], s2[3]),
        add!(s2[4], s2[6]), add!(s2[5], s2[7]), sub!(s2[4], s2[6]), sub!(s2[5], s2[7]),
        add!(s2[8], s2[10]), add!(s2[9], s2[11]), sub!(s2[8], s2[10]), sub!(s2[9], s2[11]),
        add!(s2[12], s2[14]), add!(s2[13], s2[15]), sub!(s2[12], s2[14]), sub!(s2[13], s2[15]),
    ];
    // stage 4
    let s4: [__m256i; 16] = [
        s3[0], s3[1], s3[2], s3[3],
        hbtf(t, c!(t, cospi, 16), s3[4], c!(t, cospi, 48), s3[5], rnd, sh),
        hbtf(t, c!(t, cospi, 48), s3[4], cn!(t, cospi, 16), s3[5], rnd, sh),
        hbtf(t, cn!(t, cospi, 48), s3[6], c!(t, cospi, 16), s3[7], rnd, sh),
        hbtf(t, c!(t, cospi, 16), s3[6], c!(t, cospi, 48), s3[7], rnd, sh),
        s3[8], s3[9], s3[10], s3[11],
        hbtf(t, c!(t, cospi, 16), s3[12], c!(t, cospi, 48), s3[13], rnd, sh),
        hbtf(t, c!(t, cospi, 48), s3[12], cn!(t, cospi, 16), s3[13], rnd, sh),
        hbtf(t, cn!(t, cospi, 48), s3[14], c!(t, cospi, 16), s3[15], rnd, sh),
        hbtf(t, c!(t, cospi, 16), s3[14], c!(t, cospi, 48), s3[15], rnd, sh),
    ];
    // stage 5
    let s5: [__m256i; 16] = [
        add!(s4[0], s4[4]), add!(s4[1], s4[5]), add!(s4[2], s4[6]), add!(s4[3], s4[7]),
        sub!(s4[0], s4[4]), sub!(s4[1], s4[5]), sub!(s4[2], s4[6]), sub!(s4[3], s4[7]),
        add!(s4[8], s4[12]), add!(s4[9], s4[13]), add!(s4[10], s4[14]), add!(s4[11], s4[15]),
        sub!(s4[8], s4[12]), sub!(s4[9], s4[13]), sub!(s4[10], s4[14]), sub!(s4[11], s4[15]),
    ];
    // stage 6
    let s6: [__m256i; 16] = [
        s5[0], s5[1], s5[2], s5[3], s5[4], s5[5], s5[6], s5[7],
        hbtf(t, c!(t, cospi, 8), s5[8], c!(t, cospi, 56), s5[9], rnd, sh),
        hbtf(t, c!(t, cospi, 56), s5[8], cn!(t, cospi, 8), s5[9], rnd, sh),
        hbtf(t, c!(t, cospi, 40), s5[10], c!(t, cospi, 24), s5[11], rnd, sh),
        hbtf(t, c!(t, cospi, 24), s5[10], cn!(t, cospi, 40), s5[11], rnd, sh),
        hbtf(t, cn!(t, cospi, 56), s5[12], c!(t, cospi, 8), s5[13], rnd, sh),
        hbtf(t, c!(t, cospi, 8), s5[12], c!(t, cospi, 56), s5[13], rnd, sh),
        hbtf(t, cn!(t, cospi, 24), s5[14], c!(t, cospi, 40), s5[15], rnd, sh),
        hbtf(t, c!(t, cospi, 40), s5[14], c!(t, cospi, 24), s5[15], rnd, sh),
    ];
    // stage 7
    let s7: [__m256i; 16] = [
        add!(s6[0], s6[8]), add!(s6[1], s6[9]), add!(s6[2], s6[10]), add!(s6[3], s6[11]),
        add!(s6[4], s6[12]), add!(s6[5], s6[13]), add!(s6[6], s6[14]), add!(s6[7], s6[15]),
        sub!(s6[0], s6[8]), sub!(s6[1], s6[9]), sub!(s6[2], s6[10]), sub!(s6[3], s6[11]),
        sub!(s6[4], s6[12]), sub!(s6[5], s6[13]), sub!(s6[6], s6[14]), sub!(s6[7], s6[15]),
    ];
    // stage 8
    let s8: [__m256i; 16] = [
        hbtf(t, c!(t, cospi, 2), s7[0], c!(t, cospi, 62), s7[1], rnd, sh),
        hbtf(t, c!(t, cospi, 62), s7[0], cn!(t, cospi, 2), s7[1], rnd, sh),
        hbtf(t, c!(t, cospi, 10), s7[2], c!(t, cospi, 54), s7[3], rnd, sh),
        hbtf(t, c!(t, cospi, 54), s7[2], cn!(t, cospi, 10), s7[3], rnd, sh),
        hbtf(t, c!(t, cospi, 18), s7[4], c!(t, cospi, 46), s7[5], rnd, sh),
        hbtf(t, c!(t, cospi, 46), s7[4], cn!(t, cospi, 18), s7[5], rnd, sh),
        hbtf(t, c!(t, cospi, 26), s7[6], c!(t, cospi, 38), s7[7], rnd, sh),
        hbtf(t, c!(t, cospi, 38), s7[6], cn!(t, cospi, 26), s7[7], rnd, sh),
        hbtf(t, c!(t, cospi, 34), s7[8], c!(t, cospi, 30), s7[9], rnd, sh),
        hbtf(t, c!(t, cospi, 30), s7[8], cn!(t, cospi, 34), s7[9], rnd, sh),
        hbtf(t, c!(t, cospi, 42), s7[10], c!(t, cospi, 22), s7[11], rnd, sh),
        hbtf(t, c!(t, cospi, 22), s7[10], cn!(t, cospi, 42), s7[11], rnd, sh),
        hbtf(t, c!(t, cospi, 50), s7[12], c!(t, cospi, 14), s7[13], rnd, sh),
        hbtf(t, c!(t, cospi, 14), s7[12], cn!(t, cospi, 50), s7[13], rnd, sh),
        hbtf(t, c!(t, cospi, 58), s7[14], c!(t, cospi, 6), s7[15], rnd, sh),
        hbtf(t, c!(t, cospi, 6), s7[14], cn!(t, cospi, 58), s7[15], rnd, sh),
    ];
    // stage 9: output permutation
    *out = [
        s8[1], s8[14], s8[3], s8[12], s8[5], s8[10], s8[7], s8[8],
        s8[9], s8[6], s8[11], s8[4], s8[13], s8[2], s8[15], s8[0],
    ];
}

// ---------------------------------------------------------------------------
// 16-point inverse ADST (svt_av1_iadst16_new / inv_txfm.rs::iadst16).
// ---------------------------------------------------------------------------
#[rite]
pub(super) fn iadst16_x8(
    t: Desktop64,
    inp: &[__m256i; 16],
    out: &mut [__m256i; 16],
    rnd: __m256i,
    sh: __m128i,
    lo: __m256i,
    hi: __m256i,
) {
    let cospi = &COSPI;
    let cl = |v| clampv(t, v, lo, hi);
    // stage 1: input permutation
    let s1: [__m256i; 16] = [
        inp[15], inp[0], inp[13], inp[2], inp[11], inp[4], inp[9], inp[6],
        inp[7], inp[8], inp[5], inp[10], inp[3], inp[12], inp[1], inp[14],
    ];
    // stage 2
    let s2: [__m256i; 16] = [
        hbtf(t, c!(t, cospi, 2), s1[0], c!(t, cospi, 62), s1[1], rnd, sh),
        hbtf(t, c!(t, cospi, 62), s1[0], cn!(t, cospi, 2), s1[1], rnd, sh),
        hbtf(t, c!(t, cospi, 10), s1[2], c!(t, cospi, 54), s1[3], rnd, sh),
        hbtf(t, c!(t, cospi, 54), s1[2], cn!(t, cospi, 10), s1[3], rnd, sh),
        hbtf(t, c!(t, cospi, 18), s1[4], c!(t, cospi, 46), s1[5], rnd, sh),
        hbtf(t, c!(t, cospi, 46), s1[4], cn!(t, cospi, 18), s1[5], rnd, sh),
        hbtf(t, c!(t, cospi, 26), s1[6], c!(t, cospi, 38), s1[7], rnd, sh),
        hbtf(t, c!(t, cospi, 38), s1[6], cn!(t, cospi, 26), s1[7], rnd, sh),
        hbtf(t, c!(t, cospi, 34), s1[8], c!(t, cospi, 30), s1[9], rnd, sh),
        hbtf(t, c!(t, cospi, 30), s1[8], cn!(t, cospi, 34), s1[9], rnd, sh),
        hbtf(t, c!(t, cospi, 42), s1[10], c!(t, cospi, 22), s1[11], rnd, sh),
        hbtf(t, c!(t, cospi, 22), s1[10], cn!(t, cospi, 42), s1[11], rnd, sh),
        hbtf(t, c!(t, cospi, 50), s1[12], c!(t, cospi, 14), s1[13], rnd, sh),
        hbtf(t, c!(t, cospi, 14), s1[12], cn!(t, cospi, 50), s1[13], rnd, sh),
        hbtf(t, c!(t, cospi, 58), s1[14], c!(t, cospi, 6), s1[15], rnd, sh),
        hbtf(t, c!(t, cospi, 6), s1[14], cn!(t, cospi, 58), s1[15], rnd, sh),
    ];
    // stage 3
    let s3: [__m256i; 16] = [
        cl(add!(s2[0], s2[8])), cl(add!(s2[1], s2[9])), cl(add!(s2[2], s2[10])), cl(add!(s2[3], s2[11])),
        cl(add!(s2[4], s2[12])), cl(add!(s2[5], s2[13])), cl(add!(s2[6], s2[14])), cl(add!(s2[7], s2[15])),
        cl(sub!(s2[0], s2[8])), cl(sub!(s2[1], s2[9])), cl(sub!(s2[2], s2[10])), cl(sub!(s2[3], s2[11])),
        cl(sub!(s2[4], s2[12])), cl(sub!(s2[5], s2[13])), cl(sub!(s2[6], s2[14])), cl(sub!(s2[7], s2[15])),
    ];
    // stage 4
    let s4: [__m256i; 16] = [
        s3[0], s3[1], s3[2], s3[3], s3[4], s3[5], s3[6], s3[7],
        hbtf(t, c!(t, cospi, 8), s3[8], c!(t, cospi, 56), s3[9], rnd, sh),
        hbtf(t, c!(t, cospi, 56), s3[8], cn!(t, cospi, 8), s3[9], rnd, sh),
        hbtf(t, c!(t, cospi, 40), s3[10], c!(t, cospi, 24), s3[11], rnd, sh),
        hbtf(t, c!(t, cospi, 24), s3[10], cn!(t, cospi, 40), s3[11], rnd, sh),
        hbtf(t, cn!(t, cospi, 56), s3[12], c!(t, cospi, 8), s3[13], rnd, sh),
        hbtf(t, c!(t, cospi, 8), s3[12], c!(t, cospi, 56), s3[13], rnd, sh),
        hbtf(t, cn!(t, cospi, 24), s3[14], c!(t, cospi, 40), s3[15], rnd, sh),
        hbtf(t, c!(t, cospi, 40), s3[14], c!(t, cospi, 24), s3[15], rnd, sh),
    ];
    // stage 5
    let s5: [__m256i; 16] = [
        cl(add!(s4[0], s4[4])), cl(add!(s4[1], s4[5])), cl(add!(s4[2], s4[6])), cl(add!(s4[3], s4[7])),
        cl(sub!(s4[0], s4[4])), cl(sub!(s4[1], s4[5])), cl(sub!(s4[2], s4[6])), cl(sub!(s4[3], s4[7])),
        cl(add!(s4[8], s4[12])), cl(add!(s4[9], s4[13])), cl(add!(s4[10], s4[14])), cl(add!(s4[11], s4[15])),
        cl(sub!(s4[8], s4[12])), cl(sub!(s4[9], s4[13])), cl(sub!(s4[10], s4[14])), cl(sub!(s4[11], s4[15])),
    ];
    // stage 6
    let s6: [__m256i; 16] = [
        s5[0], s5[1], s5[2], s5[3],
        hbtf(t, c!(t, cospi, 16), s5[4], c!(t, cospi, 48), s5[5], rnd, sh),
        hbtf(t, c!(t, cospi, 48), s5[4], cn!(t, cospi, 16), s5[5], rnd, sh),
        hbtf(t, cn!(t, cospi, 48), s5[6], c!(t, cospi, 16), s5[7], rnd, sh),
        hbtf(t, c!(t, cospi, 16), s5[6], c!(t, cospi, 48), s5[7], rnd, sh),
        s5[8], s5[9], s5[10], s5[11],
        hbtf(t, c!(t, cospi, 16), s5[12], c!(t, cospi, 48), s5[13], rnd, sh),
        hbtf(t, c!(t, cospi, 48), s5[12], cn!(t, cospi, 16), s5[13], rnd, sh),
        hbtf(t, cn!(t, cospi, 48), s5[14], c!(t, cospi, 16), s5[15], rnd, sh),
        hbtf(t, c!(t, cospi, 16), s5[14], c!(t, cospi, 48), s5[15], rnd, sh),
    ];
    // stage 7
    let s7: [__m256i; 16] = [
        cl(add!(s6[0], s6[2])), cl(add!(s6[1], s6[3])), cl(sub!(s6[0], s6[2])), cl(sub!(s6[1], s6[3])),
        cl(add!(s6[4], s6[6])), cl(add!(s6[5], s6[7])), cl(sub!(s6[4], s6[6])), cl(sub!(s6[5], s6[7])),
        cl(add!(s6[8], s6[10])), cl(add!(s6[9], s6[11])), cl(sub!(s6[8], s6[10])), cl(sub!(s6[9], s6[11])),
        cl(add!(s6[12], s6[14])), cl(add!(s6[13], s6[15])), cl(sub!(s6[12], s6[14])), cl(sub!(s6[13], s6[15])),
    ];
    // stage 8
    let s8: [__m256i; 16] = [
        s7[0], s7[1],
        hbtf(t, c!(t, cospi, 32), s7[2], c!(t, cospi, 32), s7[3], rnd, sh),
        hbtf(t, c!(t, cospi, 32), s7[2], cn!(t, cospi, 32), s7[3], rnd, sh),
        s7[4], s7[5],
        hbtf(t, c!(t, cospi, 32), s7[6], c!(t, cospi, 32), s7[7], rnd, sh),
        hbtf(t, c!(t, cospi, 32), s7[6], cn!(t, cospi, 32), s7[7], rnd, sh),
        s7[8], s7[9],
        hbtf(t, c!(t, cospi, 32), s7[10], c!(t, cospi, 32), s7[11], rnd, sh),
        hbtf(t, c!(t, cospi, 32), s7[10], cn!(t, cospi, 32), s7[11], rnd, sh),
        s7[12], s7[13],
        hbtf(t, c!(t, cospi, 32), s7[14], c!(t, cospi, 32), s7[15], rnd, sh),
        hbtf(t, c!(t, cospi, 32), s7[14], cn!(t, cospi, 32), s7[15], rnd, sh),
    ];
    // stage 9: output with negations
    *out = [
        s8[0], neg!(s8[8]), s8[12], neg!(s8[4]), s8[6], neg!(s8[14]), s8[10], neg!(s8[2]),
        s8[3], neg!(s8[11]), s8[15], neg!(s8[7]), s8[5], neg!(s8[13]), s8[9], neg!(s8[1]),
    ];
}
