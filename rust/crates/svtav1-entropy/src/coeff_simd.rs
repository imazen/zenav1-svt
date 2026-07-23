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
    TX_CLASS_2D, TX_CLASS_HORIZ, TX_PAD_HOR, TX_SIZES_ALL, levels_origin, nz_map_ctx,
    nz_map_ctx_offset_1d, nz_map_ctx_offset_2d, txb_bwl, txb_high, txb_wide,
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
// agree at every scan position — the only ones any caller reads.
//
// [`nz_map_contexts_raster`] reproduces the raster kernel into a dense scratch;
// [`crate::coeff_c::get_nz_map_contexts`] then scatters `scan[0..eob]` out, so its
// public output stays byte-identical to the scan-order `_c` at every written
// position. The heavy per-position neighbour sum is what vectorises.

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

/// Fill `scratch[0..width*height]` with the raster nz-map context of every
/// coefficient position, then apply the 2D DC-zero and the eob (scan-last)
/// scan-index context — reproducing `svt_av1_get_nz_map_contexts_sse2` exactly
/// (for `eob >= 2`; for `eob == 1` only position 0 is meaningful and is set to
/// 0). `scratch` must have room for `txb_wide*txb_high` bytes; `eob >= 1`.
pub(crate) fn nz_map_contexts_raster(
    levels: &[u8],
    scan: &[u16],
    eob: usize,
    tx_size: usize,
    tx_class: usize,
    scratch: &mut [u8],
) {
    fill_raster(levels, tx_size, tx_class, scratch);
    // DC (position 0) of a 2D block is context 0 (`(tx_class|coeff_idx)==0`);
    // the SIMD kernel computes `count + offset(0)=count`, so normalise here.
    if tx_class == TX_CLASS_2D {
        scratch[0] = 0;
    }
    // The scan-last (eob) coefficient uses a scan-index context, not neighbours.
    let last = eob - 1;
    if last == 0 {
        scratch[0] = 0;
    } else {
        let bwl = txb_bwl(tx_size);
        let total = txb_high(tx_size) << bwl;
        let pos = scan[last] as usize;
        scratch[pos] = if last <= total / 8 {
            1
        } else if last <= total / 4 {
            2
        } else {
            3
        };
    }
}

/// Scalar raster fill — the reference the SIMD path is proven equal to. Each
/// position independently reuses the tested [`nz_map_ctx`] (`is_eob=false`), so
/// this is `_c`'s per-position math evaluated over the *whole* block.
fn fill_raster_core(levels: &[u8], tx_size: usize, tx_class: usize, scratch: &mut [u8]) {
    let bwl = txb_bwl(tx_size);
    let height = txb_high(tx_size);
    let origin = levels_origin(txb_wide(tx_size));
    let n = txb_wide(tx_size) * height;
    for (idx, s) in scratch[..n].iter_mut().enumerate() {
        *s = nz_map_ctx(levels, origin, idx, bwl, height, 0, false, tx_size, tx_class) as u8;
    }
}

fn fill_raster(levels: &[u8], tx_size: usize, tx_class: usize, scratch: &mut [u8]) {
    incant!(
        fill_raster_impl(levels, tx_size, tx_class, scratch),
        [v3, neon, scalar]
    )
}

fn fill_raster_impl_scalar(
    _token: ScalarToken,
    levels: &[u8],
    tx_size: usize,
    tx_class: usize,
    scratch: &mut [u8],
) {
    fill_raster_core(levels, tx_size, tx_class, scratch);
}

#[cfg(target_arch = "aarch64")]
#[arcane]
fn fill_raster_impl_neon(
    _token: NeonToken,
    levels: &[u8],
    tx_size: usize,
    tx_class: usize,
    scratch: &mut [u8],
) {
    fill_raster_core(levels, tx_size, tx_class, scratch);
}

/// AVX2/SSE2 raster fill for the `width >= 16` transform sizes (the dominant
/// cost); narrower blocks fall back to the scalar core. Processes 16 raster
/// positions of a row per iteration with contiguous unaligned neighbour loads —
/// identical stencil to [`fill_raster_core`], so byte-identical by construction
/// (the padded map's inter-row gaps are 0, so out-of-block neighbours read 0
/// exactly as the scalar `nz_mag` computes for edge positions).
#[cfg(target_arch = "x86_64")]
#[arcane]
fn fill_raster_impl_v3(
    token: Desktop64,
    levels: &[u8],
    tx_size: usize,
    tx_class: usize,
    scratch: &mut [u8],
) {
    let w = txb_wide(tx_size);
    if w < 16 {
        fill_raster_core(levels, tx_size, tx_class, scratch);
        return;
    }
    let h = txb_high(tx_size);
    let stride = w + TX_PAD_HOR;
    let origin = levels_origin(w);
    // Neighbour byte offsets past the right (+1) and below (+stride) taps.
    let (off0, off1, off2) = match tx_class {
        TX_CLASS_2D => (2, stride + 1, 2 * stride),
        TX_CLASS_HORIZ => (2, 3, 4),
        _ => (2 * stride, 3 * stride, 4 * stride),
    };
    let table = &NZ_OFFSET[tx_class][tx_size];
    let mut row = 0usize;
    while row < h {
        let mut cg = 0usize;
        while cg < w {
            let base = origin + row * stride + cg;
            let idx = row * w + cg;
            let offtab: &[u8; 16] = table[idx..idx + 16].try_into().unwrap();
            let out: &mut [u8; 16] = (&mut scratch[idx..idx + 16]).try_into().unwrap();
            nz_kernel16_v3(token, levels, base, stride, off0, off1, off2, offtab, out);
            cg += 16;
        }
        row += 1;
    }
}

/// Sum the clamped 5-neighbour stencil for 16 positions, halve-round, clamp to
/// 4, add the position base, and store. Mirrors `get_coeff_contexts_kernel_sse2`
/// (`_mm_avg_epu8(count, 0)` = `(count+1)>>1`; the sum of five `min(_,3)` bytes
/// is ≤ 15 so u8 lanes never overflow).
#[cfg(target_arch = "x86_64")]
#[rite]
#[allow(clippy::too_many_arguments)]
fn nz_kernel16_v3(
    _token: Desktop64,
    levels: &[u8],
    base: usize,
    stride: usize,
    off0: usize,
    off1: usize,
    off2: usize,
    offtab: &[u8; 16],
    out: &mut [u8; 16],
) {
    let l0: &[u8; 16] = levels[base + 1..base + 17].try_into().unwrap();
    let l1: &[u8; 16] = levels[base + stride..base + stride + 16].try_into().unwrap();
    let l2: &[u8; 16] = levels[base + off0..base + off0 + 16].try_into().unwrap();
    let l3: &[u8; 16] = levels[base + off1..base + off1 + 16].try_into().unwrap();
    let l4: &[u8; 16] = levels[base + off2..base + off2 + 16].try_into().unwrap();
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
