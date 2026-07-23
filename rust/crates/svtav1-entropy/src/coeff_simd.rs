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

use crate::coeff_c::{
    TX_CLASS_2D, TX_CLASS_HORIZ, TX_PAD_HOR, TX_SIZES_ALL, levels_origin, nz_map_ctx_offset_1d,
    nz_map_ctx_offset_2d, txb_bwl, txb_high, txb_wide,
};

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

// ---------------------------------------------------------------------------
// nz-map coefficient contexts (`svt_av1_get_nz_map_contexts`)
// ---------------------------------------------------------------------------
//
// For every coefficient position the context is `min((Σ min(neighbour,3)+1)>>1,
// 4) + position_base`, summing a fixed 5-neighbour stencil of the padded level
// map. The scalar `_c` reference walks the block in *scan* order and writes only
// `scan[0..eob]`; the production C SIMD kernel (`svt_av1_get_nz_map_contexts_sse2`
// / `_avx2`, the RTCD default) instead fills the **whole** padded block in raster
// order (contiguous 16-byte neighbour loads, no scattered gathers) and then
// stamps the single scan-last (eob) position with its scan-index context. Both
// agree at every scan position — the only ones any caller reads (verified for
// both port call sites; see [`crate::coeff_c::get_nz_map_contexts`]).
//
// The `v3` arm below IS the `_sse2` kernel: it writes the raster contexts
// directly into `coeff_contexts` (16 positions per iteration — one row chunk at
// width ≥ 16, 4 rows at width 4, 2 rows at width 8, exactly C's
// `load_levels_{16x1,4x4,8x2}x5` shapes), applies the 2D DC zero, and stamps
// the scan-last position. The scalar (and NEON, until a NEON kernel is ported)
// arm runs the scan-order `_c` loop — the same per-arch split as C's RTCD.

/// Maximum coefficient count for any coded transform block (32×32 after the
/// 64→32 dimension cap for coefficient coding).
const MAX_TXB_COEFFS: usize = 32 * 32;

/// Per-`(tx_class, tx_size)` position-base context offset — C
/// `eb_av1_nz_map_ctx_offset` (2D) plus the `nz_map_ctx_offset_1d` bases (H/V) —
/// in **raster** (`row*width + col`) order. Built at compile time from the same
/// `nz_map_ctx_offset_2d` / `_1d` helpers the scalar path uses (and that
/// `tests/c_parity.rs` pins to the exported C `ref_nz_map_ctx_offset`), so the
/// SIMD fill needs no re-transcribed SSE2 vector constants — the classic
/// nz-map byte-diff source. Values fit `u8` (0..=36); only `idx < width*height`
/// is populated and read. ~57 KiB of `.rodata`.
static NZ_OFFSET: [[[u8; MAX_TXB_COEFFS]; TX_SIZES_ALL]; 3] = build_nz_offset();

const fn build_nz_offset() -> [[[u8; MAX_TXB_COEFFS]; TX_SIZES_ALL]; 3] {
    let mut t = [[[0u8; MAX_TXB_COEFFS]; TX_SIZES_ALL]; 3];
    let mut cls = 0usize;
    while cls < 3 {
        let mut ts = 0usize;
        while ts < TX_SIZES_ALL {
            let w = txb_wide(ts);
            let h = txb_high(ts);
            let bwl = txb_bwl(ts);
            let n = w * h;
            let mut idx = 0usize;
            while idx < n {
                let off = match cls {
                    // TX_CLASS_2D
                    0 => nz_map_ctx_offset_2d(ts, idx),
                    // TX_CLASS_HORIZ — base by column
                    1 => nz_map_ctx_offset_1d(idx & ((1 << bwl) - 1)),
                    // TX_CLASS_VERT — base by row
                    _ => nz_map_ctx_offset_1d(idx >> bwl),
                };
                t[cls][ts][idx] = off as u8;
                idx += 1;
            }
            ts += 1;
        }
        cls += 1;
    }
    t
}

/// Dispatch for [`crate::coeff_c::get_nz_map_contexts`] with `eob >= 2` (the
/// `eob <= 1` cases short-circuit in the public wrapper). The v3 arm is the
/// production `_sse2` raster kernel; scalar/NEON run the scan-order `_c` loop.
/// All arms write identical bytes at every `scan[0..eob]` position (proven
/// against both exported real-C kernels in `tests/c_parity.rs`); non-scan
/// positions are raster values on v3 and untouched on scalar, exactly the
/// per-arch difference C's RTCD ships.
pub(crate) fn nz_map_contexts(
    levels: &[u8],
    scan: &[u16],
    eob: usize,
    tx_size: usize,
    tx_class: usize,
    coeff_contexts: &mut [i8],
) {
    incant!(
        nz_map_ctxs_impl(levels, scan, eob, tx_size, tx_class, coeff_contexts),
        [v3, neon, scalar]
    )
}

fn nz_map_ctxs_impl_scalar(
    _token: ScalarToken,
    levels: &[u8],
    scan: &[u16],
    eob: usize,
    tx_size: usize,
    tx_class: usize,
    coeff_contexts: &mut [i8],
) {
    crate::coeff_c::nz_map_contexts_scan_order(levels, scan, eob, tx_size, tx_class, coeff_contexts);
}

#[cfg(target_arch = "aarch64")]
#[arcane]
fn nz_map_ctxs_impl_neon(
    _token: NeonToken,
    levels: &[u8],
    scan: &[u16],
    eob: usize,
    tx_size: usize,
    tx_class: usize,
    coeff_contexts: &mut [i8],
) {
    // Scan-order until `svt_av1_get_nz_map_contexts_neon` is ported.
    crate::coeff_c::nz_map_contexts_scan_order(levels, scan, eob, tx_size, tx_class, coeff_contexts);
}

/// `svt_av1_get_nz_map_contexts_sse2` (encodetxb_sse2.c:450), verbatim: fill
/// `coeff_contexts[0..w*h]` with every position's raster nz-map context, zero
/// the 2D DC, then stamp the scan-last position with its scan-index context.
///
/// 16 raster positions per iteration in C's three width shapes: one 16-column
/// row chunk (w ≥ 16, `load_levels_16x1x5` — contiguous unaligned loads), 4
/// whole rows (w == 4, `load_levels_4x4x5`), or 2 whole rows (w == 8,
/// `load_levels_8x2x5`) — the narrow shapes gather each tap's row segments
/// through the horizontal pad, whose zeros are exactly what the scalar
/// `nz_mag` computes for out-of-block neighbours. Every `w*h` is a multiple of
/// 16 (min txb 4×4) and `h % 4 == 0` at w == 4 / `h % 2 == 0` at w == 8 (C's
/// asserts), so the chunks tile the block exactly. Position-base offsets come
/// from [`NZ_OFFSET`] instead of C's per-row shifting vector constants.
///
/// Worst-case tap read (TX_CLASS_VERT, last row, last chunk) ends at byte
/// `(TX_PAD_TOP + h + 3) * stride + w` — exactly the `used` extent
/// [`crate::coeff_c::txb_init_levels`] zeroes (the same bound the scan-order
/// reader reaches from the last coefficient), so the raster fill never reads a
/// stale byte; `tests/c_parity.rs::
/// coeff_c_txb_init_levels_partial_zero_no_stale_reads` polices this with 0xFF
/// poison past `used`.
#[cfg(target_arch = "x86_64")]
#[arcane]
fn nz_map_ctxs_impl_v3(
    token: Desktop64,
    levels: &[u8],
    scan: &[u16],
    eob: usize,
    tx_size: usize,
    tx_class: usize,
    coeff_contexts: &mut [i8],
) {
    let w = txb_wide(tx_size);
    let h = txb_high(tx_size);
    let stride = w + TX_PAD_HOR;
    let origin = levels_origin(w);
    // Third/fourth/fifth stencil taps past the right (+1) and below (+stride)
    // ones — C's `offsets[3]` per tx_class (encodetxb_sse2.c:470-496).
    let (off0, off1, off2) = match tx_class {
        TX_CLASS_2D => (2, stride + 1, 2 * stride),
        TX_CLASS_HORIZ => (2, 3, 4),
        _ => (2 * stride, 3 * stride, 4 * stride),
    };
    let table = &NZ_OFFSET[tx_class][tx_size];

    match w {
        4 => {
            // 4 rows × 4 columns per iteration (h % 4 == 0).
            let mut row = 0usize;
            while row < h {
                let base = origin + row * stride;
                let idx = row * 4;
                let l0 = gather4(levels, base + 1, stride);
                let l1 = gather4(levels, base + stride, stride);
                let l2 = gather4(levels, base + off0, stride);
                let l3 = gather4(levels, base + off1, stride);
                let l4 = gather4(levels, base + off2, stride);
                nz_kernel16_v3(
                    token,
                    &l0,
                    &l1,
                    &l2,
                    &l3,
                    &l4,
                    table[idx..idx + 16].try_into().unwrap(),
                    (&mut coeff_contexts[idx..idx + 16]).try_into().unwrap(),
                );
                row += 4;
            }
        }
        8 => {
            // 2 rows × 8 columns per iteration (h % 2 == 0).
            let mut row = 0usize;
            while row < h {
                let base = origin + row * stride;
                let idx = row * 8;
                let l0 = gather8(levels, base + 1, stride);
                let l1 = gather8(levels, base + stride, stride);
                let l2 = gather8(levels, base + off0, stride);
                let l3 = gather8(levels, base + off1, stride);
                let l4 = gather8(levels, base + off2, stride);
                nz_kernel16_v3(
                    token,
                    &l0,
                    &l1,
                    &l2,
                    &l3,
                    &l4,
                    table[idx..idx + 16].try_into().unwrap(),
                    (&mut coeff_contexts[idx..idx + 16]).try_into().unwrap(),
                );
                row += 2;
            }
        }
        _ => {
            // w ∈ {16, 32}: 16 columns of one row per iteration; all five taps
            // are direct contiguous loads.
            let mut row = 0usize;
            while row < h {
                let mut cg = 0usize;
                while cg < w {
                    let base = origin + row * stride + cg;
                    let idx = row * w + cg;
                    nz_kernel16_v3(
                        token,
                        levels[base + 1..base + 17].try_into().unwrap(),
                        levels[base + stride..base + stride + 16].try_into().unwrap(),
                        levels[base + off0..base + off0 + 16].try_into().unwrap(),
                        levels[base + off1..base + off1 + 16].try_into().unwrap(),
                        levels[base + off2..base + off2 + 16].try_into().unwrap(),
                        table[idx..idx + 16].try_into().unwrap(),
                        (&mut coeff_contexts[idx..idx + 16]).try_into().unwrap(),
                    );
                    cg += 16;
                }
                row += 1;
            }
        }
    }

    // DC of a 2D block is context 0 (`(tx_class | coeff_idx) == 0` in
    // `get_nz_map_ctx_from_stats`); the raster fill computed `count + 0`.
    // C's 2D fill helpers end with the same `coeff_contexts[0] = 0`.
    if tx_class == TX_CLASS_2D {
        coeff_contexts[0] = 0;
    }
    // The scan-last coefficient uses its scan-index context, not neighbours
    // (`eob >= 2` here, so `last >= 1` and the `scan_idx == 0 → 0` arm of the
    // scalar `_c` is unreachable — C `_sse2` handled it in its `eob == 1`
    // early-out, our public wrapper likewise).
    let bwl = txb_bwl(tx_size);
    let last = eob - 1;
    let pos = scan[last] as usize;
    coeff_contexts[pos] = if last <= (h << bwl) / 8 {
        1
    } else if last <= (h << bwl) / 4 {
        2
    } else {
        3
    };
}

/// Gather 4 rows × 4 bytes at `s`, `s + stride`, … into one 16-lane block —
/// C `load_8bit_4x4_to_1_reg_sse2` (4 dword loads).
#[cfg(target_arch = "x86_64")]
#[inline(always)]
fn gather4(levels: &[u8], s: usize, stride: usize) -> [u8; 16] {
    let mut a = [0u8; 16];
    a[0..4].copy_from_slice(&levels[s..s + 4]);
    a[4..8].copy_from_slice(&levels[s + stride..s + stride + 4]);
    a[8..12].copy_from_slice(&levels[s + 2 * stride..s + 2 * stride + 4]);
    a[12..16].copy_from_slice(&levels[s + 3 * stride..s + 3 * stride + 4]);
    a
}

/// Gather 2 rows × 8 bytes at `s`, `s + stride` into one 16-lane block —
/// C `load_8bit_8x2_to_1_reg_sse2` (2 qword loads).
#[cfg(target_arch = "x86_64")]
#[inline(always)]
fn gather8(levels: &[u8], s: usize, stride: usize) -> [u8; 16] {
    let mut a = [0u8; 16];
    a[0..8].copy_from_slice(&levels[s..s + 8]);
    a[8..16].copy_from_slice(&levels[s + stride..s + stride + 8]);
    a
}

/// Sum the clamped 5-neighbour stencil for 16 positions, halve-round, clamp to
/// 4, add the position base, and store. Mirrors `get_coeff_contexts_kernel_sse2`
/// (encodetxb_sse2.c:65): `min(level, 3)` per tap (levels are 0..=127, so
/// `_mm_min_epu8` == C's `clip_max3` LUT), the five-tap sum ≤ 15 never
/// overflows a u8 lane, `_mm_avg_epu8(count, 0)` == `(count + 1) >> 1`, clamp
/// to 4, then `+ offset` ≤ 40 fits i8.
#[cfg(target_arch = "x86_64")]
#[rite]
fn nz_kernel16_v3(
    _token: Desktop64,
    l0: &[u8; 16],
    l1: &[u8; 16],
    l2: &[u8; 16],
    l3: &[u8; 16],
    l4: &[u8; 16],
    offtab: &[u8; 16],
    out: &mut [i8; 16],
) {
    let c3 = _mm_set1_epi8(3);
    let mut c = _mm_min_epu8(_mm_loadu_si128(l0), c3);
    c = _mm_add_epi8(c, _mm_min_epu8(_mm_loadu_si128(l1), c3));
    c = _mm_add_epi8(c, _mm_min_epu8(_mm_loadu_si128(l2), c3));
    c = _mm_add_epi8(c, _mm_min_epu8(_mm_loadu_si128(l3), c3));
    c = _mm_add_epi8(c, _mm_min_epu8(_mm_loadu_si128(l4), c3));
    c = _mm_avg_epu8(c, _mm_setzero_si128());
    c = _mm_min_epu8(c, _mm_set1_epi8(4));
    c = _mm_add_epi8(c, _mm_loadu_si128(offtab));
    _mm_storeu_si128(out, c);
}
