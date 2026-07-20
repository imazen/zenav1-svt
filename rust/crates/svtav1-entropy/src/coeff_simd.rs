//! SIMD fill for the coefficient level map (`svt_av1_txb_init_levels_c`).
//!
//! [`fill_levels`] is the per-txb inner map used by [`crate::coeff_c::txb_init_levels`]:
//! `levels[origin + r*(width+4) + c] = min(|coeff[r*width + c]|, 127)`. It is a
//! pure, independent per-element map (integer `abs` → clamp to `INT8_MAX` →
//! narrow to `u8`) with no cross-element reduction, so the columns of a row map
//! directly onto SIMD lanes and the result is **bit-identical** to the scalar
//! reference by construction. Called once per transform block on the coeff-
//! coding hot path (RDOQ trellis in `quant.rs`, plus `pd0`/`leaf_funnel`).
//!
//! Dispatched via archmage (`incant!([v3, neon, scalar])`) — the AVX2 `v3` arm
//! is the tuned path; the scalar core ([`fill_levels_core`], a verbatim copy of
//! the original inner loop) is the reference and the fallback for narrow (`width
//! == 4`) blocks and non-x86 targets. Proven equal to the exported real-C
//! `svt_av1_txb_init_levels_c` under **every** dispatch tier in
//! `tests/c_parity.rs::txb_init_levels_simd_matches_c`.

use archmage::prelude::*;

use crate::coeff_c::{TX_PAD_HOR, levels_origin};

/// Fill the coefficient level map. `levels_buf` is assumed pre-zeroed by the
/// caller ([`crate::coeff_c::txb_init_levels`]); this writes only the `width`
/// value columns of each of the `height` rows at the padded origin, leaving the
/// horizontal/vertical pad bytes at 0.
pub(crate) fn fill_levels(coeff: &[i32], width: usize, height: usize, levels_buf: &mut [u8]) {
    incant!(
        fill_levels_impl(coeff, width, height, levels_buf),
        [v3, neon, scalar]
    )
}

/// Scalar reference — byte-for-byte the original `txb_init_levels` inner loop
/// (`svt_av1_txb_init_levels_c`, rd_cost.c:93). The AVX2 path is proven
/// identical to this against real C in `tests/c_parity.rs`.
#[inline]
fn fill_levels_core(coeff: &[i32], width: usize, height: usize, levels_buf: &mut [u8]) {
    let stride = width + TX_PAD_HOR;
    let origin = levels_origin(width);
    for r in 0..height {
        let cb = r * width;
        let db = origin + r * stride;
        for c in 0..width {
            levels_buf[db + c] = coeff[cb + c].unsigned_abs().min(127) as u8;
        }
    }
}

fn fill_levels_impl_scalar(
    _token: ScalarToken,
    coeff: &[i32],
    width: usize,
    height: usize,
    levels_buf: &mut [u8],
) {
    fill_levels_core(coeff, width, height, levels_buf);
}

#[cfg(target_arch = "aarch64")]
#[arcane]
fn fill_levels_impl_neon(
    _token: NeonToken,
    coeff: &[i32],
    width: usize,
    height: usize,
    levels_buf: &mut [u8],
) {
    fill_levels_core(coeff, width, height, levels_buf);
}

/// AVX2 fill. Each row of `width` coefficients (`width` ∈ {8, 16, 32} on this
/// arm — `width == 4` blocks fall back to the scalar core) is packed 8 columns
/// at a time. Byte-identical to [`fill_levels_core`]: `_mm256_abs_epi32` then an
/// **unsigned** `_mm256_min_epu32` against `INT8_MAX` reproduces Rust's
/// `i32::unsigned_abs().min(127)` for the *entire* `i32` domain — including
/// `i32::MIN`, whose `abs_epi32` overflows to `0x8000_0000` but, read as
/// unsigned (`2^31`), still min-clamps to 127 exactly as `unsigned_abs()` does.
/// The two `packus` stages saturate to `[0, 255]`, a no-op on the ≤127 values.
#[cfg(target_arch = "x86_64")]
#[arcane]
fn fill_levels_impl_v3(
    token: Desktop64,
    coeff: &[i32],
    width: usize,
    height: usize,
    levels_buf: &mut [u8],
) {
    if width < 8 {
        // Only BLOCK width 4 (a row is a single 4-lane group with a 4-byte pad
        // gap to the next row's destination); not worth a masked path.
        fill_levels_core(coeff, width, height, levels_buf);
        return;
    }
    let stride = width + TX_PAD_HOR;
    let origin = levels_origin(width);
    for r in 0..height {
        let cb = r * width;
        let db = origin + r * stride;
        let mut c = 0usize;
        // width is 8/16/32 here, so this consumes the row exactly (no remainder).
        while c + 8 <= width {
            let src: &[i32; 8] = coeff[cb + c..cb + c + 8].try_into().unwrap();
            let dst: &mut [u8; 8] = (&mut levels_buf[db + c..db + c + 8]).try_into().unwrap();
            pack8_v3(token, src, dst);
            c += 8;
        }
    }
}

/// Map 8 contiguous `i32` coefficients to 8 `u8` levels `min(|x|, 127)` and
/// store them. See [`fill_levels_impl_v3`] for the exactness argument.
#[cfg(target_arch = "x86_64")]
#[rite]
fn pack8_v3(_token: Desktop64, src: &[i32; 8], dst: &mut [u8; 8]) {
    let v = _mm256_loadu_si256(src);
    let a = _mm256_abs_epi32(v);
    let clamped = _mm256_min_epu32(a, _mm256_set1_epi32(127));
    // Narrow i32x8 -> u8x8: packus i32->u16 (lane-crossing pair, order v0..v7),
    // then u16->u8; the low 8 bytes hold v0..v7. No saturation on 0..127.
    let lo = _mm256_castsi256_si128(clamped);
    let hi = _mm256_extracti128_si256::<1>(clamped);
    let packed16 = _mm_packus_epi32(lo, hi);
    let packed8 = _mm_packus_epi16(packed16, packed16);
    _mm_storeu_si64(dst, packed8);
}
