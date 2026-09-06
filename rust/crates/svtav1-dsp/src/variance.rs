//! Variance and SSE (Sum of Squared Errors) computation.
//!
//! Spec 13 (segmentation.md): Variance for adaptive quantization.
//!
//! Variance is used for adaptive quantization, activity masking,
//! and screen content detection. SSE is the primary distortion metric
//! for rate-distortion optimization.

use archmage::prelude::*;

/// Compute variance of an 8-bit pixel block.
///
/// Returns (variance, mean) where variance = E[x²] - E[x]² scaled by N.
/// More precisely: variance = sum((x - mean)²) = sum(x²) - sum(x)²/N
pub fn variance(src: &[u8], src_stride: usize, width: usize, height: usize) -> (u64, u32) {
    incant!(
        variance_impl(src, src_stride, width, height),
        [v3, neon, scalar]
    )
}

/// C `svt_aom_variance{W}x{H}_c` — the TWO-BUFFER difference variance.
///
/// `variance_c` (`Lib/C_DEFAULT/variance.c:141`) accumulates the signed
/// difference `sum` and the squared difference `sse` over the block; the `VAR`
/// macro (`:184`) then returns
/// `*sse - (uint32_t)(((int64_t)sum * sum) / (W * H))`.
///
/// Distinct from [`variance`] above, which is the SINGLE-buffer activity
/// variance used by segmentation/AQ. This one is C's `AomVarianceFnPtr::vf`
/// (`Lib/Codec/av1me.c:30`) — the MDS0 fast distortion on the arm C takes when
/// `mds0_use_hadamard_blk` is false (`product_coding_loop.c:1296-1302`).
///
/// Exactness notes, all binding for byte identity:
/// * `sum` is C's `int`; the widest block here (128x128) reaches |sum| <= 4.18e6
///   and sse <= 1.07e9, so i32/u32 do not overflow — the u64/i64 accumulators
///   below are a superset and truncate to the same values.
/// * the division is C integer division of a non-negative `sum * sum` by
///   `w * h`, i.e. truncation, and it happens BEFORE the subtraction.
/// * the subtraction is in `uint32_t`. `sse >= sum^2/n` always (Cauchy-Schwarz),
///   so it cannot wrap; `wrapping_sub` is used to make that explicit rather than
///   to permit it.
pub fn variance_diff(
    a: &[u8],
    a_stride: usize,
    b: &[u8],
    b_stride: usize,
    width: usize,
    height: usize,
) -> u32 {
    let (sse, sum) = incant!(
        variance_diff_parts_impl(a, a_stride, b, b_stride, width, height),
        [v3, neon, scalar]
    );
    let n = (width * height) as i64;
    (sse as u32).wrapping_sub(((sum * sum) / n) as u32)
}

/// Compute SSE between two blocks of 8-bit pixels.
pub fn sse(
    src: &[u8],
    src_stride: usize,
    ref_: &[u8],
    ref_stride: usize,
    width: usize,
    height: usize,
) -> u64 {
    incant!(
        sse_impl(src, src_stride, ref_, ref_stride, width, height),
        [v3, neon, scalar]
    )
}

// --- Scalar implementations ---

fn variance_impl_scalar(
    _token: ScalarToken,
    src: &[u8],
    src_stride: usize,
    width: usize,
    height: usize,
) -> (u64, u32) {
    let mut sum: u64 = 0;
    let mut sum_sq: u64 = 0;
    for row in 0..height {
        let offset = row * src_stride;
        for col in 0..width {
            let v = src[offset + col] as u64;
            sum += v;
            sum_sq += v * v;
        }
    }
    let n = (width * height) as u64;
    let variance = sum_sq * n - sum * sum;
    let mean = (sum / n) as u32;
    (variance, mean)
}

/// `(sse, sum)` for [`variance_diff`] — scalar reference, mirrors C's
/// `variance_c` loop exactly.
fn variance_diff_parts_impl_scalar(
    _token: ScalarToken,
    a: &[u8],
    a_stride: usize,
    b: &[u8],
    b_stride: usize,
    width: usize,
    height: usize,
) -> (u64, i64) {
    let mut sse: u64 = 0;
    let mut sum: i64 = 0;
    for row in 0..height {
        let a_off = row * a_stride;
        let b_off = row * b_stride;
        for col in 0..width {
            let diff = a[a_off + col] as i32 - b[b_off + col] as i32;
            sum += diff as i64;
            sse += (diff * diff) as u64;
        }
    }
    (sse, sum)
}

// --- AVX2 implementations ---

#[cfg(target_arch = "x86_64")]
#[arcane]
fn variance_impl_v3(
    _token: Desktop64,
    src: &[u8],
    src_stride: usize,
    width: usize,
    height: usize,
) -> (u64, u32) {
    // Auto-vectorize with AVX2 enabled — compiler does well here
    let mut sum: u64 = 0;
    let mut sum_sq: u64 = 0;
    for row in 0..height {
        let offset = row * src_stride;
        for col in 0..width {
            let v = src[offset + col] as u64;
            sum += v;
            sum_sq += v * v;
        }
    }
    let n = (width * height) as u64;
    let variance = sum_sq * n - sum * sum;
    let mean = (sum / n) as u32;
    (variance, mean)
}

#[cfg(target_arch = "x86_64")]
#[arcane]
fn variance_diff_parts_impl_v3(
    _token: Desktop64,
    a: &[u8],
    a_stride: usize,
    b: &[u8],
    b_stride: usize,
    width: usize,
    height: usize,
) -> (u64, i64) {
    // Auto-vectorizes well with AVX2 enabled; the shape is identical to the
    // scalar reference so the two cannot drift.
    let mut sse: u64 = 0;
    let mut sum: i64 = 0;
    for row in 0..height {
        let a_off = row * a_stride;
        let b_off = row * b_stride;
        for col in 0..width {
            let diff = a[a_off + col] as i32 - b[b_off + col] as i32;
            sum += diff as i64;
            sse += (diff * diff) as u64;
        }
    }
    (sse, sum)
}

// --- NEON implementations ---

#[cfg(target_arch = "aarch64")]
#[arcane]
fn variance_impl_neon(
    _token: NeonToken,
    src: &[u8],
    src_stride: usize,
    width: usize,
    height: usize,
) -> (u64, u32) {
    // Sum via the vpaddlq/vpadalq widening chain; sum-of-squares via vmull_u8
    // (u8*u8 fits u16) drained into u32 lanes every iteration.
    //
    // Overflow: a u16 lane holds at most 255*255 = 65025, so squares must NOT
    // accumulate in u16. Draining to u32 per 16-byte chunk keeps the largest
    // block used here (128x128 = 16384 px, worst case 65025*16384 = 1.07e9)
    // inside u32's 4.29e9.
    let mut sum_acc = vdupq_n_u32(0);
    let mut sq_acc = vdupq_n_u32(0);
    let mut tail_sum: u64 = 0;
    let mut tail_sq: u64 = 0;

    for row in 0..height {
        let off = row * src_stride;
        let mut col = 0;
        while col + 16 <= width {
            let c: &[u8; 16] = src[off + col..off + col + 16].try_into().unwrap();
            let v = vld1q_u8(c);
            sum_acc = vpadalq_u16(sum_acc, vpaddlq_u8(v));
            let lo = vget_low_u8(v);
            let hi = vget_high_u8(v);
            sq_acc = vpadalq_u16(sq_acc, vmull_u8(lo, lo));
            sq_acc = vpadalq_u16(sq_acc, vmull_u8(hi, hi));
            col += 16;
        }
        while col < width {
            let v = src[off + col] as u64;
            tail_sum += v;
            tail_sq += v * v;
            col += 1;
        }
    }

    let sum = vaddvq_u32(sum_acc) as u64 + tail_sum;
    let sum_sq = vaddvq_u32(sq_acc) as u64 + tail_sq;
    let n = (width * height) as u64;
    let variance = sum_sq * n - sum * sum;
    let mean = (sum / n) as u32;
    (variance, mean)
}

#[cfg(target_arch = "aarch64")]
#[arcane]
fn variance_diff_parts_impl_neon(
    _token: NeonToken,
    a: &[u8],
    a_stride: usize,
    b: &[u8],
    b_stride: usize,
    width: usize,
    height: usize,
) -> (u64, i64) {
    // `sum` of signed differences is computed as sum(a) - sum(b) — exact, and it
    // avoids needing a signed 8-bit subtract. Each is a vpaddlq/vpadalq widening
    // chain drained to u64 per ROW, so an arbitrarily tall block cannot overflow
    // the u32 accumulator (worst row: 128 px * 255 = 32640).
    //
    // `sse` uses vabdq_u8 (|a-b| is exact for u8, and |d|^2 == d^2) squared with
    // vmull_u8 into u16 lanes; squares reach 65025 so they are drained to u32
    // every 16-byte chunk, and the u32 total is drained to u64 per row.
    let mut sse: u64 = 0;
    let mut sum_a: u64 = 0;
    let mut sum_b: u64 = 0;

    for row in 0..height {
        let a_off = row * a_stride;
        let b_off = row * b_stride;
        let mut col = 0;
        let mut sse_acc = vdupq_n_u32(0);
        let mut a_acc = vdupq_n_u32(0);
        let mut b_acc = vdupq_n_u32(0);
        while col + 16 <= width {
            let ac: &[u8; 16] = a[a_off + col..a_off + col + 16].try_into().unwrap();
            let bc: &[u8; 16] = b[b_off + col..b_off + col + 16].try_into().unwrap();
            let va = vld1q_u8(ac);
            let vb = vld1q_u8(bc);
            a_acc = vpadalq_u16(a_acc, vpaddlq_u8(va));
            b_acc = vpadalq_u16(b_acc, vpaddlq_u8(vb));
            let d = vabdq_u8(va, vb);
            let lo = vget_low_u8(d);
            let hi = vget_high_u8(d);
            sse_acc = vpadalq_u16(sse_acc, vmull_u8(lo, lo));
            sse_acc = vpadalq_u16(sse_acc, vmull_u8(hi, hi));
            col += 16;
        }
        sse += vaddvq_u32(sse_acc) as u64;
        sum_a += vaddvq_u32(a_acc) as u64;
        sum_b += vaddvq_u32(b_acc) as u64;
        while col < width {
            let av = a[a_off + col] as u64;
            let bv = b[b_off + col] as u64;
            let diff = av as i64 - bv as i64;
            sum_a += av;
            sum_b += bv;
            sse += (diff * diff) as u64;
            col += 1;
        }
    }

    (sse, sum_a as i64 - sum_b as i64)
}

/// Scalar SSE reference. The value every tier must equal, and the oracle the
/// tier sweep in `sse_simd_matches_scalar_exhaustively` compares against.
///
/// Integer squares and their sum are exact and associative, so lane order and
/// accumulator width cannot change the result — only overflow could, and the
/// vector body's drain schedule is sized against that below.
#[cfg(test)]
fn sse_core(
    src: &[u8],
    src_stride: usize,
    ref_: &[u8],
    ref_stride: usize,
    width: usize,
    height: usize,
) -> u64 {
    let mut sse: u64 = 0;
    for row in 0..height {
        let s_off = row * src_stride;
        let r_off = row * ref_stride;
        for col in 0..width {
            let diff = src[s_off + col] as i32 - ref_[r_off + col] as i32;
            sse += (diff * diff) as u64;
        }
    }
    sse
}

/// SSE with shared row traversal and per-tier vector arithmetic.
///
/// # C's shape, and the arithmetic width that is the whole difference
///
/// `svt_spatial_full_distortion_kernel_avx2`
/// (`ASM_AVX2/pic_operators_intrin_avx2.c:802`) reaches
/// `spatial_full_distortion_kernel16_avx2_intrin`
/// (`ASM_AVX2/pic_operators_inline_avx2.h:111`), which is four instructions
/// per sixteen samples:
///
/// ```text
///   in16 = _mm256_cvtepu8_epi16(input);      re16 = _mm256_cvtepu8_epi16(recon);
///   diff = _mm256_sub_epi16(in16, re16);
///   dist = _mm256_madd_epi16(diff, diff);    // d*d + d*d -> ONE i32 lane
///   sum  = _mm256_add_epi32(sum, dist);      // i32 lanes, widened only at the end
/// ```
///
/// The port's x86 arm was the scalar double loop with a `u64` accumulator
/// (`sse += (diff * diff) as u64`), left to auto-vectorise. It does vectorise,
/// but the `u64` accumulator forces every square to be widened to 64-bit
/// lanes, so the same work costs four times the vector registers. That single
/// line carried **21,363,780 Ir of `__arcane_sse_impl_v3`'s 27,915,788 self
/// Ir** on photo_cid 512² p6 (`benchmarks/stall_attrib_2026-09-05.meta` §5) —
/// the same class of finding as the CfL kernel's i32-vs-i16 rounding
/// (`benchmarks/cfl_simd_kernel_2026-09-05.meta`): the port had the right
/// algorithm at the wrong arithmetic width.
///
/// `i16xN::madd_adjacent` (archmage PR #96, which closes the issue this port
/// filed) is `_mm256_madd_epi16` / `vmlal` / `i32x4.dot_i16x8_s` behind one
/// generic name, so C's exact shape becomes expressible without a hand-written
/// arm per ISA. The scalar/WASM body uses `u8xN::abs_diff` before widening.
/// The x86 body below widens both byte inputs to sixteen i16 lanes before
/// subtraction, matching AVX2's single-madd arithmetic width.
///
/// **This body is `cfg`'d OFF on aarch64 and that is a measurement, not an
/// oversight** — see `sse_impl_neon`, which is 1.45x-2.20x faster than this
/// body there because magetypes' NEON `madd_adjacent` costs three instructions
/// per eight lanes on top of two widenings, against `vmull_u8` + `vpadalq_u16`
/// doing both in two.
///
/// # Small widths take C's row-PACKING, not a narrow kernel
///
/// At preset 2 most calls are transform units of 4 or 8 columns, and a 16-lane
/// kernel with a scalar tail degenerates to the scalar loop on all of them:
/// measured, `variance::sse` is **5.17 % of the photo_cid p2 frame's
/// instructions** against 3.42 % at p6. C does not run a narrow kernel there
/// either — `svt_spatial_full_distortion_kernel_avx2`'s `leftover == 8` arm
/// (`ASM_AVX2/pic_operators_intrin_avx2.c:831-847`) packs TWO rows into one
/// register with `_mm256_setr_m128i` of two `_mm_loadl_epi64`, and its
/// `leftover == 4` arm (`:815-830`) does the same with two `_mm_cvtsi32_si128`.
/// This body packs `16 / width` rows into one 16-byte vector for widths 8 and
/// 4, so a 4x4 block is ONE fold instead of sixteen scalar iterations. The
/// staging copies stand in for `setr_m128i`: magetypes has no
/// vector-concatenate.
///
/// # Overflow, computed rather than asserted
///
/// A `u8` difference squares to at most `255² = 65_025`. `drain_every` is
/// `32_000 / width` rows (at least one), so the i32 accumulator holds at most
/// `65_025 * width * (32_000 / width) <= 65_025 * 32_000 = 2_080_800_000`
/// across all accumulator lanes before it is reduced. The row-packed path adds up to
/// `g - 1 = 3` rows beyond the threshold before it notices, worth another
/// `3 * 65_025 * 4 = 780_300`, for `2_081_580_300` — inside `i32::MAX`
/// (2_147_483_647), so `reduce_add` neither wraps nor goes negative. The
/// per-row scalar tail accumulates in `u32`: at most `65_025 * width`, which
/// needs `width <= 66_046` to be safe and is checked by the same bound.
#[cfg(not(target_arch = "aarch64"))]
#[magetypes(define(u8x16, u16x8, i16x8, i32x4), wasm128, scalar)]
fn sse_impl(
    token: Token,
    src: &[u8],
    src_stride: usize,
    ref_: &[u8],
    ref_stride: usize,
    width: usize,
    height: usize,
) -> u64 {
    // One sixteen-byte pair folded into the i32 accumulator. Captures only the
    // token, so it is a plain value-in / value-out closure and the borrow
    // checker never sees `acc` inside it.
    let fold = |acc: i32x4, a: &[u8; 16], b: &[u8; 16]| -> i32x4 {
        let d = u8x16::load(token, a).abs_diff(u8x16::load(token, b));
        let lo = d.widen_low().bitcast_i16x8();
        let hi = d.widen_high().bitcast_i16x8();
        acc + lo.madd_adjacent(lo) + hi.madd_adjacent(hi)
    };
    sse_rows(
        src,
        src_stride,
        ref_,
        ref_stride,
        width,
        height,
        i32x4::splat(token, 0),
        fold,
        |acc| acc.reduce_add(),
    )
}

/// Widen bytes before subtracting so AVX2 squares sixteen differences with
/// one 256-bit madd. The staged high half is unused and optimized away.
#[cfg(target_arch = "x86_64")]
#[arcane]
fn sse_impl_v3(
    token: X64V3Token,
    src: &[u8],
    src_stride: usize,
    ref_: &[u8],
    ref_stride: usize,
    width: usize,
    height: usize,
) -> u64 {
    use magetypes::simd::generic::{i32x8, u8x32};
    let fold = |acc: i32x8<X64V3Token>, a: &[u8; 16], b: &[u8; 16]| {
        let mut aa = [0u8; 32];
        let mut bb = [0u8; 32];
        aa[..16].copy_from_slice(a);
        bb[..16].copy_from_slice(b);
        let a = u8x32::from_array(token, aa).widen_low().bitcast_i16x16();
        let b = u8x32::from_array(token, bb).widen_low().bitcast_i16x16();
        let d = a - b;
        acc + d.madd_adjacent(d)
    };
    // Keep narrow row widths constant through inlining: runtime-length
    // copy_from_slice otherwise survives as memcpy calls for every row.
    let run = |w| {
        sse_rows(
            src,
            src_stride,
            ref_,
            ref_stride,
            w,
            height,
            i32x8::splat(token, 0),
            fold,
            |acc| acc.reduce_add(),
        )
    };
    match width {
        4 => run(4),
        8 => run(8),
        _ => run(width),
    }
}

#[cfg(not(target_arch = "aarch64"))]
#[inline(always)]
fn sse_rows<A: Copy>(
    src: &[u8],
    src_stride: usize,
    ref_: &[u8],
    ref_stride: usize,
    width: usize,
    height: usize,
    zero: A,
    fold: impl Fn(A, &[u8; 16], &[u8; 16]) -> A,
    reduce: impl Fn(A) -> i32,
) -> u64 {
    let scalar_row = |s_off: usize, r_off: usize, from: usize| -> u32 {
        let mut t: u32 = 0;
        for col in from..width {
            let diff = src[s_off + col] as i32 - ref_[r_off + col] as i32;
            t += (diff * diff) as u32;
        }
        t
    };

    let mut total: u64 = 0;
    let mut acc = zero;
    let drain_every = (32_000 / width.max(1)).max(1);
    let mut since_drain = 0usize;

    if width >= 16 {
        for row in 0..height {
            let s_off = row * src_stride;
            let r_off = row * ref_stride;
            let mut col = 0usize;
            while col + 16 <= width {
                let a: &[u8; 16] = src[s_off + col..s_off + col + 16].try_into().unwrap();
                let b: &[u8; 16] = ref_[r_off + col..r_off + col + 16].try_into().unwrap();
                acc = fold(acc, a, b);
                col += 16;
            }
            total += u64::from(scalar_row(s_off, r_off, col));
            since_drain += 1;
            if since_drain >= drain_every {
                total += u64::from(reduce(acc) as u32);
                acc = zero;
                since_drain = 0;
            }
        }
    } else if width == 8 || width == 4 {
        // C's `leftover == 8` / `leftover == 4` arms
        // (`ASM_AVX2/pic_operators_intrin_avx2.c:831-847` and `:815-830`) do
        // exactly this: they pack TWO rows (`_mm256_setr_m128i(in0, in1)` of two
        // `_mm_loadl_epi64`) or two four-byte rows into one register rather than
        // running a narrow kernel per row. `16 / width` rows fill one 16-byte
        // vector here, so a `4x4` block is ONE fold instead of sixteen scalar
        // iterations. The two 8- or 4-byte copies into the staging array are what
        // stand in for x86's `setr_m128i`; magetypes has no vector-concatenate.
        let g = 16 / width;
        let mut row = 0usize;
        while row + g <= height {
            let mut sa = [0u8; 16];
            let mut rb = [0u8; 16];
            for k in 0..g {
                let s_off = (row + k) * src_stride;
                let r_off = (row + k) * ref_stride;
                sa[k * width..(k + 1) * width].copy_from_slice(&src[s_off..s_off + width]);
                rb[k * width..(k + 1) * width].copy_from_slice(&ref_[r_off..r_off + width]);
            }
            acc = fold(acc, &sa, &rb);
            row += g;
            since_drain += g;
            if since_drain >= drain_every {
                total += u64::from(reduce(acc) as u32);
                acc = zero;
                since_drain = 0;
            }
        }
        while row < height {
            total += u64::from(scalar_row(row * src_stride, row * ref_stride, 0));
            row += 1;
        }
    } else {
        for row in 0..height {
            total += u64::from(scalar_row(row * src_stride, row * ref_stride, 0));
        }
    }
    total + u64::from(reduce(acc) as u32)
}

/// aarch64 keeps ITS OWN arm, byte-identical to what `main` already had, and
/// BOTH attempts to change it were MEASURED AND REVERTED.
///
/// 1. **The generic `#[magetypes]` body above is 1.45x-2.20x SLOWER here.**
///    magetypes lowers `i16x8::madd_adjacent` on aarch64 to
///    `vpaddq_s32(vmull_s16(lo, lo), vmull_high_s16(a, a))` — three
///    instructions per eight lanes — on top of two `vmovl_u8` widenings, where
///    the `vmull_u8` + `vpadalq_u16` pair below squares AND widens in two with
///    no widening step at all. Eleven vector instructions per sixteen bytes
///    against five.
/// 2. **C's row PACKING for widths 8 and 4 is 1.32x-1.57x SLOWER here** when
///    the rows are staged through a `[u8; 16]`, and it drags the untouched
///    `width >= 16` path down with it (a code-shape effect; the wide loop's
///    source did not change). It is a clear Ir win on x86, where it stays.
///
/// Both are in `benchmarks/sse_madd_2026-09-05.{tsv,meta}` §5 with three
/// alternating runs of each build on an M4 Pro and disjoint intervals, and
/// finding 1 is reported on archmage#96. **The open aarch64 item is a row-pack
/// that does NOT stage through memory** — `vcombine_u8(vld1_u8(row0),
/// vld1_u8(row1))` is C's `_mm256_setr_m128i` exactly and was NOT tried; the
/// staged form is what lost.
#[cfg(target_arch = "aarch64")]
#[arcane]
fn sse_impl_neon(
    _token: NeonToken,
    src: &[u8],
    src_stride: usize,
    ref_: &[u8],
    ref_stride: usize,
    width: usize,
    height: usize,
) -> u64 {
    // Small transform blocks never enter the 16-byte body. Keep their
    // vector path separate so larger blocks retain the existing row loop.
    if width == 4 || width == 8 {
        let mut total = 0u64;
        for row in 0..height {
            let s_off = row * src_stride;
            let r_off = row * ref_stride;
            let (a, b) = if width == 8 {
                let a: &[u8; 8] = src[s_off..s_off + 8].try_into().unwrap();
                let b: &[u8; 8] = ref_[r_off..r_off + 8].try_into().unwrap();
                (vld1_u8(a), vld1_u8(b))
            } else {
                let a: &[u8; 4] = src[s_off..s_off + 4].try_into().unwrap();
                let b: &[u8; 4] = ref_[r_off..r_off + 4].try_into().unwrap();
                (
                    vcreate_u8(u64::from(u32::from_le_bytes(*a))),
                    vcreate_u8(u64::from(u32::from_le_bytes(*b))),
                )
            };
            let d = vabd_u8(a, b);
            total += u64::from(vaddlvq_u16(vmull_u8(d, d)));
        }
        return total;
    }

    // |a-b| via vabdq_u8 (exact for u8), squared with vmull_u8 into u16 and
    // drained to u32 each chunk — squares reach 65025 so they must not
    // accumulate in u16. The u32 accumulator is drained to u64 per ROW, so an
    // arbitrarily tall block cannot overflow it.
    let mut total: u64 = 0;
    let mut tail: u64 = 0;

    for row in 0..height {
        let s_off = row * src_stride;
        let r_off = row * ref_stride;
        let mut col = 0;
        let mut acc = vdupq_n_u32(0);

        while col + 16 <= width {
            let a: &[u8; 16] = src[s_off + col..s_off + col + 16].try_into().unwrap();
            let b: &[u8; 16] = ref_[r_off + col..r_off + col + 16].try_into().unwrap();
            let d = vabdq_u8(vld1q_u8(a), vld1q_u8(b));
            let lo = vget_low_u8(d);
            let hi = vget_high_u8(d);
            acc = vpadalq_u16(acc, vmull_u8(lo, lo));
            acc = vpadalq_u16(acc, vmull_u8(hi, hi));
            col += 16;
        }
        total += vaddvq_u32(acc) as u64;

        while col < width {
            let diff = src[s_off + col] as i32 - ref_[r_off + col] as i32;
            tail += (diff * diff) as u64;
            col += 1;
        }
    }
    total + tail
}

/// The plain scalar loop, kept as the aarch64 `incant!` fallback tier.
///
/// On x86/wasm the `#[magetypes]` body generates its own `_scalar` variant and
/// this one is not compiled; on aarch64 the generic body is not generated at
/// all, so the tier sweep needs a real scalar arm here.
#[cfg(target_arch = "aarch64")]
fn sse_impl_scalar(
    _token: ScalarToken,
    src: &[u8],
    src_stride: usize,
    ref_: &[u8],
    ref_stride: usize,
    width: usize,
    height: usize,
) -> u64 {
    let mut sse: u64 = 0;
    for row in 0..height {
        let s_off = row * src_stride;
        let r_off = row * ref_stride;
        for col in 0..width {
            let diff = src[s_off + col] as i32 - ref_[r_off + col] as i32;
            sse += (diff * diff) as u64;
        }
    }
    sse
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variance_uniform_block() {
        let block = [128u8; 64];
        let (var, mean) = variance(&block, 8, 8, 8);
        assert_eq!(var, 0, "uniform block should have zero variance");
        assert_eq!(mean, 128);
    }

    #[test]
    fn variance_known_values() {
        // 4x4 block: 0,1,2,...,15
        let mut block = [0u8; 16];
        for (i, b) in block.iter_mut().enumerate() {
            *b = i as u8;
        }
        let (var, _mean) = variance(&block, 4, 4, 4);
        // sum = 120, sum_sq = 1240, n = 16
        // var = 1240 * 16 - 120 * 120 = 19840 - 14400 = 5440
        assert_eq!(var, 5440);
    }

    #[test]
    fn sse_identical_blocks() {
        let block = [42u8; 64];
        assert_eq!(sse(&block, 8, &block, 8, 8, 8), 0);
    }

    /// C `variance_c` + `VAR(W,H)` by hand on a case where the mean difference
    /// is NON-ZERO, so the `- sum*sum/n` term is load-bearing (an implementation
    /// that returned plain SSE would pass an identical-mean test).
    #[test]
    fn variance_diff_known_values() {
        // 4x4: a = 0..15, b = all 4. diffs -4..11.
        let mut a = [0u8; 16];
        for (i, v) in a.iter_mut().enumerate() {
            *v = i as u8;
        }
        let b = [4u8; 16];
        // sum  = (0+..+15) - 16*4 = 120 - 64 = 56
        // sse  = sum over d in -4..=11 of d*d = (16+9+4+1) + 11*12*23/6 = 536
        // var  = 536 - (56*56)/16 = 536 - 196 = 340
        assert_eq!(variance_diff(&a, 4, &b, 4, 4, 4), 340);
        // identical blocks: sum = 0, sse = 0.
        assert_eq!(variance_diff(&a, 4, &a, 4, 4, 4), 0);
    }

    /// Every dispatch tier must agree with the scalar core on random content,
    /// at strides wider than the block (the MDS0 caller passes a full-frame
    /// source stride against a tightly-packed prediction) and at widths that
    /// are not a multiple of the 16-lane NEON chunk, which is where a tail bug
    /// hides. Consumes the `PermutationReport` — a bare call would silently
    /// degrade to native-tier-only coverage (see rust/CLAUDE.md, Archmage Rules).
    #[test]
    fn variance_diff_random_all_tiers_match_scalar() {
        use archmage::testing::{CompileTimePolicy, for_each_token_permutation};
        let mut st = 0x2545_F491_4F6C_DD1Du64;
        let mut next = move || {
            st ^= st << 13;
            st ^= st >> 7;
            st ^= st << 17;
            (st >> 33) as u8
        };
        // 4x4 .. 64x64 square + the non-square and non-16-multiple shapes.
        for &(w, h) in &[
            (4usize, 4usize),
            (4, 8),
            (8, 4),
            (8, 8),
            (12, 4),
            (16, 16),
            (16, 4),
            (4, 16),
            (20, 12),
            (32, 32),
            (32, 8),
            (64, 64),
            (64, 16),
        ] {
            for pad in [0usize, 7] {
                let (astr, bstr) = (w + pad, w + 2 * pad);
                let a: alloc::vec::Vec<u8> = (0..astr * h).map(|_| next()).collect();
                let b: alloc::vec::Vec<u8> = (0..bstr * h).map(|_| next()).collect();
                // Independent oracle, written straight from C's variance_c +
                // VAR(W,H) rather than by calling the scalar arm — so a bug
                // shared by every arm still fails the test.
                let (mut sse_ref, mut sum_ref) = (0i64, 0i64);
                for r in 0..h {
                    for c in 0..w {
                        let d = a[r * astr + c] as i64 - b[r * bstr + c] as i64;
                        sum_ref += d;
                        sse_ref += d * d;
                    }
                }
                let n = (w * h) as i64;
                let expect = (sse_ref as u32).wrapping_sub(((sum_ref * sum_ref) / n) as u32);
                let rep = for_each_token_permutation(CompileTimePolicy::WarnStderr, |perm| {
                    assert_eq!(
                        variance_diff(&a, astr, &b, bstr, w, h),
                        expect,
                        "variance_diff {w}x{h} strides ({astr},{bstr}) tier {perm}"
                    );
                });
                assert!(
                    rep.warnings.is_empty(),
                    "tokens excluded at compile time, coverage is not what it looks like: {:?}",
                    rep.warnings
                );
                assert!(
                    rep.permutations_run >= 2,
                    "only {} permutation(s) ran",
                    rep.permutations_run
                );
            }
        }
    }

    #[test]
    fn sse_known_value() {
        let src = [10u8; 16];
        let ref_ = [20u8; 16];
        // Each pixel diff = 10, diff² = 100, 16 pixels => SSE = 1600
        assert_eq!(sse(&src, 4, &ref_, 4, 4, 4), 1600);
    }

    #[test]
    fn sse_max_difference() {
        let src = [0u8; 16];
        let ref_ = [255u8; 16];
        assert_eq!(sse(&src, 4, &ref_, 4, 4, 4), 255 * 255 * 16);
    }
}

#[cfg(test)]
mod dispatch_tests {
    use super::*;

    use alloc::vec::Vec;
    use archmage::testing::{CompileTimePolicy, for_each_token_permutation};

    #[test]
    fn variance_all_dispatch_levels() {
        let block: Vec<u8> = (0..64).map(|i| (i * 3 + 17) as u8).collect();
        let reference_result = variance(&block, 8, 8, 8);

        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            let result = variance(&block, 8, 8, 8);
            assert_eq!(
                result, reference_result,
                "variance mismatch at dispatch level"
            );
        });
    }

    /// EVERY archmage tier of the vector SSE == the scalar reference, over the
    /// WHOLE per-lane input domain and at the exact accumulator bound.
    ///
    /// Three independent things can break in `sse_impl` and each is a case:
    ///
    /// * **the per-lane arithmetic** — `u8xN::abs_diff` then
    ///   `i16xN::madd_adjacent(self)`. Case 1 is EXHAUSTIVE: a 256x256 block
    ///   whose `src[i][j] = i` and `ref[i][j] = j` puts every one of the
    ///   65_536 ordered `(u8, u8)` pairs through the kernel, so both `MIN-MAX`
    ///   directions and the `|d| = 255` extreme are covered by construction,
    ///   not by sampling;
    /// * **the 16-lane split and the scalar tail** — case 2 walks widths
    ///   1..=33 plus 48/64/128 against heights 1..=5, 16 and 33, so every
    ///   `width % 16` class and every one-row/one-column shape is crossed;
    /// * **the i32 accumulator's drain schedule** — case 3 runs the two
    ///   shapes that fill it to the documented maximum with the maximum
    ///   difference: `16 x 8192`, `128 x 1024`, `8 x 16384` and `4 x 32768` of
    ///   0-against-255, whose per-drain totals are `65_025 * 32_000` (plus the
    ///   row-packed path's up-to-three-row overshoot), i.e. the largest value
    ///   `reduce_add` may ever see. The last two also drive the row-PACKED
    ///   path's own drain. Every one of the four has a true SSE above
    ///   `u32::MAX`, so a missing drain or a sign slip cannot pass.
    ///
    /// The `PermutationReport` is CONSUMED: `warnings.is_empty()` and
    /// `permutations_run >= 2` are asserted, so a sweep that silently
    /// collapses to the native tier fails instead of passing green
    /// (`rust/CLAUDE.md`'s silent-coverage hazard).
    #[test]
    fn sse_simd_matches_scalar_exhaustively() {
        use alloc::vec::Vec;
        use archmage::testing::{CompileTimePolicy, for_each_token_permutation};

        // Case 1 — every ordered (u8, u8) pair, once.
        let mut ex_src = Vec::with_capacity(256 * 256);
        let mut ex_ref = Vec::with_capacity(256 * 256);
        for i in 0..256usize {
            for j in 0..256usize {
                ex_src.push(i as u8);
                ex_ref.push(j as u8);
            }
        }
        let ex_expect = sse_core(&ex_src, 256, &ex_ref, 256, 256, 256);
        // sum over all ordered pairs of (i - j)^2 = 2 * sum_{d=1}^{255} (256-d) d^2
        let mut closed: u64 = 0;
        for d in 1..=255u64 {
            closed += 2 * (256 - d) * d * d;
        }
        assert_eq!(
            ex_expect, closed,
            "the scalar reference itself disagrees with the closed form"
        );

        // Case 2 — width/height shapes.
        // (src_stride, ref_stride, src, ref, width, height, expected)
        type Shape = (usize, usize, Vec<u8>, Vec<u8>, usize, usize, u64);
        // (src, ref, width, height, expected)
        type Big = (Vec<u8>, Vec<u8>, usize, usize, u64);
        let widths: Vec<usize> = (1..=33).chain([48, 64, 128]).collect();
        let heights = [1usize, 2, 3, 4, 5, 16, 33];
        let mut shapes: Vec<Shape> = Vec::new();
        for (wi, &w) in widths.iter().enumerate() {
            for (hi, &h) in heights.iter().enumerate() {
                let ss = w + 3 + wi % 5;
                let rs = w + 1 + hi % 7;
                let sb: Vec<u8> = (0..ss * h)
                    .map(|i| ((i * 37 + wi * 11 + hi * 5) % 256) as u8)
                    .collect();
                let rb: Vec<u8> = (0..rs * h)
                    .map(|i| ((i * 53 + wi * 7 + hi * 13) % 251) as u8)
                    .collect();
                let e = sse_core(&sb, ss, &rb, rs, w, h);
                shapes.push((ss, rs, sb, rb, w, h, e));
            }
        }

        // Case 3 — the accumulator at its documented maximum.
        let big: [(usize, usize); 4] = [(16, 8192), (128, 1024), (8, 16384), (4, 32768)];
        let mut bigs: Vec<Big> = Vec::new();
        for &(w, h) in &big {
            let sb = alloc::vec![0u8; w * h];
            let rb = alloc::vec![255u8; w * h];
            let e = 255u64 * 255 * (w * h) as u64;
            assert!(e > u64::from(u32::MAX), "case 3 must exceed u32::MAX");
            bigs.push((sb, rb, w, h, e));
        }

        let report = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            assert_eq!(
                sse(&ex_src, 256, &ex_ref, 256, 256, 256),
                ex_expect,
                "exhaustive (u8, u8) domain"
            );
            for (ss, rs, sb, rb, w, h, e) in &shapes {
                assert_eq!(
                    sse(sb, *ss, rb, *rs, *w, *h),
                    *e,
                    "shape w{w} h{h} src_stride{ss} ref_stride{rs}"
                );
            }
            for (sb, rb, w, h, e) in &bigs {
                assert_eq!(sse(sb, *w, rb, *w, *w, *h), *e, "drain w{w} h{h}");
            }
        });
        assert!(
            report.warnings.is_empty(),
            "archmage excluded {} token(s) from the sweep: {:?}",
            report.warnings.len(),
            report.warnings
        );
        assert!(
            report.permutations_run >= 2,
            "the tier sweep ran {} permutation(s) -- only the native tier, which \
             cannot catch a SIMD-vs-scalar divergence",
            report.permutations_run
        );
    }

    #[test]
    fn sse_all_dispatch_levels() {
        let src: Vec<u8> = (0..64).map(|i| (i * 3 + 17) as u8).collect();
        let ref_: Vec<u8> = (0..64).map(|i| (i * 5 + 42) as u8).collect();
        let reference_result = sse(&src, 8, &ref_, 8, 8, 8);

        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            let result = sse(&src, 8, &ref_, 8, 8, 8);
            assert_eq!(result, reference_result, "sse mismatch at dispatch level");
        });
    }
}
