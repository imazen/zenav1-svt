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
