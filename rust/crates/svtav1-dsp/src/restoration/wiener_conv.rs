use super::*;

/// Which tier the `incant!` in [`wiener_convolve_add_src`] resolves to on the
/// CPU running the tests.
///
/// This exists because "the `_v4` arm is compiled" and "the `_v4` arm RUNS"
/// are different facts, and this repo has confused them fourteen times. The
/// tier list here is a CHARACTER-FOR-CHARACTER copy of the dispatch list in
/// `wiener_convolve_add_src`, so `incant!` expands to the same summon ladder
/// and the name this returns is the arm that ladder selects.
#[cfg(test)]
#[magetypes(v4(cfg(avx512)), v3, neon, wasm128, scalar)]
pub(super) fn wiener_simd_tier_name(_token: Token) -> &'static str {
    core::any::type_name::<Token>()
}

#[cfg(test)]
pub(super) fn wiener_simd_tier() -> &'static str {
    incant!(
        wiener_simd_tier_name(),
        [v4(cfg(avx512)), v3, neon, wasm128, scalar]
    )
}

/// The vector arm of [`wiener_convolve_add_src`] — one body, five tiers,
/// including x86's first genuine 512-bit arm in this port.
///
/// # Why `#[magetypes]` here, when the other kernels are per-ISA `#[arcane]`
///
/// `intra_pred`, `residual` and `me_sad` each carry a "why not `#[magetypes]`"
/// note: they need an integer WIDENING conversion, and the PUBLISHED
/// magetypes 0.9.28 (the one in `Cargo.lock`; verified with
/// `cargo read magetypes`, not from the local `~/work/archmage` checkout,
/// which carries unpublished `widen_low` / `narrow_saturating_*` work that
/// crates.io does not have) has none. That note is correct and unchanged.
///
/// This kernel sidesteps it: it never converts between lane widths. Every
/// value lives in `i32x16` from the moment the source byte is read, and the
/// u8 -> i32 widening is a plain `for t { s32[t] = i32::from(src[..]) }` loop
/// that LLVM lowers to `vpmovzxbd` inside each tier's `#[target_feature]`
/// region. The only magetypes surface used is `i32x16::{splat, from_slice,
/// store, min, max, shr_arithmetic_const}`, `Add` and `Mul<i32>` — all of them
/// in the published crate, and all of them backed by `type Repr = __m512i`
/// for `X64V4Token`, so the `_v4` arm really is 512 bits wide.
///
/// The cost of staying in i32 is the horizontal pass: C keeps it in i16 lanes
/// (`maddubs` on byte-shuffled data, 32 lanes per register) where this runs 16.
/// Closing that needs two primitives magetypes does not export —
/// `madd_adjacent` (`_mm512_madd_epi16`) and a u8 -> i16 widen — which is
/// archmage issue #89's list, already tracking `madd_adjacent` / `abs_diff`.
/// Until then the i32 form is what one body can honestly deliver on five
/// tiers, and it is the difference between a vector arm and none.
///
/// # The oracle
///
/// C's `svt_av1_wiener_convolve_add_src_avx512`
/// (`Source/Lib/ASM_AVX512/wiener_convolve_avx512.c:270`). One of its
/// rearrangements is reproduced and one is deliberately not:
///
/// * **Reproduced** — `:296` folds the `add_src` term into the coefficient
///   (`coeffs_y + offset_0`, a `1 << FILTER_BITS` planted at lane 3), so the
///   centre column is `r3 * (f[3] + 128)` rather than a separate `r3 << 7`.
///   This arm does the same on BOTH passes, saving one multiply each. It is an
///   exact regrouping of the scalar row's
///   `(s[x+3] << FILTER_BITS) + S s[x+k]*f[k]`, not an approximation.
/// * **Not reproduced** — `calc_zero_coef` (`:274`) specialises to 3-, 5- and
///   7-tap forms when the outer taps are zero, and `wiener_clip_avx512`
///   (`:27`) shifts before adding the centre sample so the whole horizontal
///   pass fits in i16. The first is a call-shape optimisation this port can
///   revisit; the second is only needed by an i16 accumulator.
#[allow(clippy::too_many_arguments)]
#[magetypes(define(i32x16), v4(cfg(avx512)), v3, neon, wasm128, scalar)]
pub(super) fn wiener_convolve_simd(
    token: Token,
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u8],
    dst_origin: usize,
    dst_stride: usize,
    hfilter: &[i16; 8],
    vfilter: &[i16; 8],
    w: usize,
    h: usize,
) {
    const LANES: usize = 16;
    let ih = h + 6;
    let n_src = w + 7;

    // Horizontal coefficients with the `add_src` term folded into the centre
    // tap: `(s[x+3] << FILTER_BITS)` IS `s[x+3] * (1 << FILTER_BITS)`.
    let mut hc = [0i32; 8];
    for k in 0..8 {
        hc[k] = i32::from(hfilter[k]);
    }
    hc[3] += 1 << FILTER_BITS;

    let zero = i32x16::splat(token, 0);
    let hi_h = i32x16::splat(
        token,
        (1i32 << (8 + 1 + FILTER_BITS - WIENER_ROUND0_BITS)) - 1,
    );
    let hi_v = i32x16::splat(token, 255);
    // `(1 << (bd + FILTER_BITS - 1))` seed plus the round-0 rounding term.
    let bias_h = i32x16::splat(
        token,
        (1 << (8 + FILTER_BITS - 1)) + (1 << (WIENER_ROUND0_BITS - 1)),
    );
    // `-(1 << (bd + round_1 - 1))` seed plus the round-1 rounding term — the
    // same constant C builds as `round_v` at wiener_convolve_avx512.c:283.
    let bias_v = i32x16::splat(
        token,
        (1 << (WIENER_ROUND1_BITS - 1)) - (1 << (8 + WIENER_ROUND1_BITS - 1)),
    );
    let vc0 = i32::from(vfilter[0]);
    let vc1 = i32::from(vfilter[1]);
    let vc2 = i32::from(vfilter[2]);
    let vc3 = i32::from(vfilter[3]) + (1 << FILTER_BITS);

    let mut ring = [[0i32; WIENER_SIMD_RING_W]; 8];
    let mut s32 = [0i32; WIENER_SIMD_SRC_W];
    let mut out32 = [0i32; WIENER_SIMD_RING_W];

    // C receives `src - 3 * stride` and subtracts three columns internally, so
    // intermediate row `j` is built from source row `j - 3` starting at
    // column -3 — the same indexing as the scalar streamed form below.
    let h_row_base = |j: usize| -> usize {
        ((src_origin + j * src_stride) as isize - 3 * src_stride as isize - 3) as usize
    };

    // One horizontal-pass row into `out`. `j >= ih` is the row C memsets: its
    // vertical weight is zero, but the sample is read.
    let fill = |j: usize,
                s32: &mut [i32; WIENER_SIMD_SRC_W],
                out: &mut [i32; WIENER_SIMD_RING_W]| {
        if j >= ih {
            out.fill(0);
            return;
        }
        let base = h_row_base(j);
        // Widen the row's `w + 7` source bytes once (LLVM: `vpmovzxbd`).
        // Zero-filling the tail is load-bearing — the last vector block reads
        // past `n_src`, and a zero there keeps stale data from the previous
        // row out of lanes that a narrower `w` would otherwise carry forward.
        for (o, &p) in s32[..n_src].iter_mut().zip(src[base..base + n_src].iter()) {
            *o = i32::from(p);
        }
        s32[n_src..].fill(0);

        let mut x = 0;
        while x < w {
            // Symmetric taps (certified by `wiener_simd_applicable`):
            // fold `s[k]*h[k] + s[7-k]*h[7-k]` into `h[k]*(s[k]+s[7-k])`
            // — C's `wiener_convolve_h_tap7_kernel_avx512` pair-sum, four
            // multiplies instead of eight.
            let acc = bias_h
                + (i32x16::from_slice(token, &s32[x..]) + i32x16::from_slice(token, &s32[x + 6..]))
                    * hc[0]
                + (i32x16::from_slice(token, &s32[x + 1..])
                    + i32x16::from_slice(token, &s32[x + 5..]))
                    * hc[1]
                + (i32x16::from_slice(token, &s32[x + 2..])
                    + i32x16::from_slice(token, &s32[x + 4..]))
                    * hc[2]
                + i32x16::from_slice(token, &s32[x + 3..]) * hc[3];
            let v = acc
                .shr_arithmetic_const::<WIENER_ROUND0_BITS>()
                .max(zero)
                .min(hi_h);
            let slot: &mut [i32; LANES] = (&mut out[x..x + LANES]).try_into().unwrap();
            v.store(slot);
            x += LANES;
        }
    };

    for j in 0..8 {
        fill(j, &mut s32, &mut ring[j % 8]);
    }

    for y in 0..h {
        let mut x = 0;
        while x < w {
            let r0 = i32x16::from_slice(token, &ring[y % 8][x..]);
            let r1 = i32x16::from_slice(token, &ring[(y + 1) % 8][x..]);
            let r2 = i32x16::from_slice(token, &ring[(y + 2) % 8][x..]);
            let r3 = i32x16::from_slice(token, &ring[(y + 3) % 8][x..]);
            let r4 = i32x16::from_slice(token, &ring[(y + 4) % 8][x..]);
            let r5 = i32x16::from_slice(token, &ring[(y + 5) % 8][x..]);
            let r6 = i32x16::from_slice(token, &ring[(y + 6) % 8][x..]);
            // Row 7's weight is zero (checked by `wiener_simd_applicable`), so
            // it is not loaded at all.
            let acc = bias_v + (r0 + r6) * vc0 + (r1 + r5) * vc1 + (r2 + r4) * vc2 + r3 * vc3;
            let v = acc
                .shr_arithmetic_const::<WIENER_ROUND1_BITS>()
                .max(zero)
                .min(hi_v);
            let slot: &mut [i32; LANES] = (&mut out32[x..x + LANES]).try_into().unwrap();
            v.store(slot);
            x += LANES;
        }
        let d = dst_origin + y * dst_stride;
        for (o, &p) in dst[d..d + w].iter_mut().zip(out32[..w].iter()) {
            *o = p as u8;
        }
        if y + 8 <= ih {
            fill(y + 8, &mut s32, &mut ring[(y + 8) % 8]);
        }
    }
}

/// C `svt_av1_wiener_convolve_add_src_c` (convolve.c:106), 8-bit.
///
/// `src`/`dst` are whole padded planes; `src_origin`/`dst_origin` index the
/// top-left pixel of the `w x h` block. Margins REQUIRED in-bounds around the
/// block in `src`: 3 above, 3 left, 3 below, 4 right (the 8th tap is zero but
/// the C code reads the sample; this port reads it too so the fuzz proves the
/// exact access pattern is safe on our padded planes).
///
/// `hfilter`/`vfilter` are full 8-tap rows (tap\[7\] = 0 by construction).
/// round0/round1 are `get_conv_params_wiener(8)`: 3 and 11.
///
/// # Streamed, and row-major on BOTH passes
///
/// C materialises the whole `(h + 7) x w` `uint16_t` intermediate, and so did
/// this port — one heap allocation per processing unit, on a function the
/// loop-restoration filter calls once per 64-wide column of every stripe of
/// every frame. Output row `y` reads intermediate rows `y .. y + 7`, so the
/// dependency is seven rows deep and the whole thing streams through a ring of
/// eight rows on the stack.
///
/// The vertical pass ALSO ran `for x { for y { .. } }`, i.e. column-major over
/// both the intermediate and the destination — every inner step jumped a
/// stride. C's `svt_aom_convolve_add_src_vert_hip` is row-major. Reordering two
/// independent loops changes nothing arithmetically and is byte-identical by
/// construction; it is called out here because it is the kind of change that
/// looks like a rewrite in a diff and is not one.
///
/// Intermediate row `h + 6` (the last one output row `h - 1` reads) is past
/// what the horizontal pass produces; C memsets it and its taps are weight 0.
/// The ring zero-fills it for the same reason.
#[allow(clippy::too_many_arguments)]
pub fn wiener_convolve_add_src(
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u8],
    dst_origin: usize,
    dst_stride: usize,
    hfilter: &[i16; 8],
    vfilter: &[i16; 8],
    w: usize,
    h: usize,
) {
    if w > WIENER_MAX_PROC_W {
        return wiener_convolve_add_src_materialised(
            src, src_origin, src_stride, dst, dst_origin, dst_stride, hfilter, vfilter, w, h,
        );
    }

    if wiener_simd_applicable(hfilter, vfilter, w) {
        return incant!(
            wiener_convolve_simd(
                src, src_origin, src_stride, dst, dst_origin, dst_stride, hfilter, vfilter, w, h,
            ),
            [v4(cfg(avx512)), v3, neon, wasm128, scalar]
        );
    }

    let bd = 8i32;
    let ih = h + 6;
    let clamp_limit = (1i32 << (bd + 1 + FILTER_BITS - WIENER_ROUND0_BITS)) - 1;

    // Ring of the eight intermediate rows output row `y` needs. Row `j` of the
    // intermediate lives at `ring[j % 8]`.
    let mut ring = [[0u16; WIENER_MAX_PROC_W]; 8];

    // C receives `src - 3 * stride` and subtracts three columns internally, so
    // intermediate row `j` is built from source row `j - 3` starting at
    // column -3.
    let h_row_base = |j: usize| -> usize {
        ((src_origin + j * src_stride) as isize - 3 * src_stride as isize - 3) as usize
    };

    let fill = |ring: &mut [[u16; WIENER_MAX_PROC_W]; 8], j: usize| {
        if j < ih {
            let base = h_row_base(j);
            wiener_h_row(&src[base..], w, hfilter, clamp_limit, &mut ring[j % 8]);
        } else {
            // Intermediate row `ih` is the one C memsets; weight 0, but read.
            ring[j % 8][..w].fill(0);
        }
    };

    for j in 0..8 {
        fill(&mut ring, j);
    }

    let mut out_row = [0u8; WIENER_MAX_PROC_W];
    for y in 0..h {
        {
            let rows: [&[u16]; 8] = [
                &ring[y % 8],
                &ring[(y + 1) % 8],
                &ring[(y + 2) % 8],
                &ring[(y + 3) % 8],
                &ring[(y + 4) % 8],
                &ring[(y + 5) % 8],
                &ring[(y + 6) % 8],
                &ring[(y + 7) % 8],
            ];
            wiener_v_row(&rows, w, vfilter, &mut out_row);
        }
        let d = dst_origin + y * dst_stride;
        dst[d..d + w].copy_from_slice(&out_row[..w]);
        if y + 8 <= ih {
            fill(&mut ring, y + 8);
        }
    }
}

/// The materialised form C writes: the whole `(h + 7) x w` intermediate, both
/// passes over it. Kept as the `w > WIENER_MAX_PROC_W` fallback AND as the
/// oracle [`wiener_convolve_add_src`]'s streamed form is pinned against
/// (`wiener_streaming_matches_materialised`).
#[allow(clippy::too_many_arguments)]
pub fn wiener_convolve_add_src_materialised(
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u8],
    dst_origin: usize,
    dst_stride: usize,
    hfilter: &[i16; 8],
    vfilter: &[i16; 8],
    w: usize,
    h: usize,
) {
    let bd = 8i32;
    // intermediate_height = (((h - 1) * 16 + 0) >> 4) + 8 - 1 = h + 6
    let ih = h + 6;
    // Temp rows ih + 1: the C memsets one row past the end (the 8th tap of
    // the bottom-most vertical windows reads it, times weight 0). Zero-init
    // covers it.
    let mut temp = alloc::vec![0u16; (ih + 1) * w.max(1)];
    let tstride = w;

    // --- Horizontal pass (svt_aom_convolve_add_src_horiz_hip) ---
    // C receives src - 3*stride, then subtracts 3 columns internally; rows
    // -3..h+2 relative to the block, window cols x-3..x+4.
    let clamp_limit = (1i32 << (bd + 1 + FILTER_BITS - WIENER_ROUND0_BITS)) - 1;
    for y in 0..ih {
        // Block-relative source row (y - 3), as index into the plane.
        let row_base = (src_origin + y * src_stride) as isize - 3 * src_stride as isize;
        for x in 0..w {
            let px = |k: usize| -> i32 {
                let idx = row_base + x as isize + k as isize - 3;
                src[idx as usize] as i32
            };
            let mut sum: i32 = (px(3) << FILTER_BITS) + (1 << (bd + FILTER_BITS - 1));
            for (k, &f) in hfilter.iter().enumerate() {
                sum += px(k) * f as i32;
            }
            temp[y * tstride + x] =
                round_power_of_two(sum, WIENER_ROUND0_BITS).clamp(0, clamp_limit) as u16;
        }
    }

    // --- Vertical pass (svt_aom_convolve_add_src_vert_hip) ---
    // C receives temp + 3*stride then subtracts 3 rows; window rows y..y+7
    // in temp coordinates (top-most window centered on block row 0).
    for x in 0..w {
        for y in 0..h {
            let base = y * tstride + x;
            let center = temp[base + 3 * tstride] as i32;
            let mut sum: i32 = (center << FILTER_BITS) - (1 << (bd + WIENER_ROUND1_BITS - 1));
            for (k, &f) in vfilter.iter().enumerate() {
                sum += temp[base + k * tstride] as i32 * f as i32;
            }
            dst[dst_origin + y * dst_stride + x] =
                round_power_of_two(sum, WIENER_ROUND1_BITS).clamp(0, 255) as u8;
        }
    }
}

/// C `find_average` (restoration_pick.h:21).
pub fn find_average(
    src: &[u8],
    origin: usize,
    stride: usize,
    h_start: i32,
    h_end: i32,
    v_start: i32,
    v_end: i32,
) -> u8 {
    let mut sum: u64 = 0;
    for i in v_start..v_end {
        for j in h_start..h_end {
            let idx = origin as isize + i as isize * stride as isize + j as isize;
            sum += src[idx as usize] as u64;
        }
    }
    (sum / ((v_end - v_start) as u64 * (h_end - h_start) as u64)) as u8
}

/// C `svt_av1_compute_stats_c` (restoration_pick.c:652).
///
/// `m` must hold `win*win` entries, `h` `win^2 * win^2`. `dgd` needs
/// `win/2` margins around the region (the search extends the recon by 3+
/// before calling). Coordinates are plane-relative; `origin` indexes (0,0).
///
/// Runtime-dispatched (`incant!([v3, neon, scalar])`): the AVX2 (`_v3`) and
/// NEON arms are C's six-step kernel (`compute_stats_win{5,7}_avx2` /
/// `_neon`) — full madd dots for `M` and the first block row/column of `H`,
/// every other `H` entry derived from a neighbour by an exact O(width) or
/// O(height) shift delta — written once in `cs_kernel!` over seven per-ISA
/// lane primitives; the `_scalar` arm is the verbatim C transcription. All
/// three are byte-identical (every intermediate is an exact integer; see the
/// macro's doc). Pinned by `tests/c_parity_wiener.rs` (`compute_stats_matches_c`
/// on the host tier + `compute_stats_all_tiers_match_c` forcing every tier).
#[allow(clippy::too_many_arguments)]
pub fn compute_stats(
    wiener_win: usize,
    dgd: &[u8],
    dgd_origin: usize,
    dgd_stride: usize,
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    h_start: i32,
    h_end: i32,
    v_start: i32,
    v_end: i32,
    m: &mut [i64],
    h: &mut [i64],
) {
    incant!(
        compute_stats_impl(
            wiener_win, dgd, dgd_origin, dgd_stride, src, src_origin, src_stride, h_start, h_end,
            v_start, v_end, m, h
        ),
        [v3, neon, scalar]
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn compute_stats_impl_scalar(
    _token: ScalarToken,
    wiener_win: usize,
    dgd: &[u8],
    dgd_origin: usize,
    dgd_stride: usize,
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    h_start: i32,
    h_end: i32,
    v_start: i32,
    v_end: i32,
    m: &mut [i64],
    h: &mut [i64],
) {
    compute_stats_scalar_core(
        wiener_win, dgd, dgd_origin, dgd_stride, src, src_origin, src_stride, h_start, h_end,
        v_start, v_end, m, h,
    );
}

// ---- AVX2 lane primitives for `cs_kernel!` ------------------------------
//
// One `__m256i` holds 16 `i16` (a window-row chunk) or 8 `i32` (an
// accumulator). `cs_madd_v3` is C's `madd_avx2` (`pickrst_avx2.h:235`):
// `_mm256_madd_epi16` forms the 16 products, sums ADJACENT pairs into 8
// `i32` lanes, and the add accumulates — one instruction pair per 16 MACs.

#[cfg(target_arch = "x86_64")]
#[rite]
pub(super) fn cs_load_v3(_t: Desktop64, a: &[i16; 16]) -> __m256i {
    _mm256_loadu_si256(a)
}

/// Lane mask: lanes `< n` all-ones, the rest zero (`n <= 16`).
#[cfg(target_arch = "x86_64")]
#[rite]
pub(super) fn cs_mask_v3(_t: Desktop64, n: usize) -> __m256i {
    let mut a = [0i16; 16];
    for v in a.iter_mut().take(n) {
        *v = -1;
    }
    _mm256_loadu_si256(&a)
}

#[cfg(target_arch = "x86_64")]
#[rite]
pub(super) fn cs_and_v3(_t: Desktop64, a: __m256i, b: __m256i) -> __m256i {
    _mm256_and_si256(a, b)
}

#[cfg(target_arch = "x86_64")]
#[rite]
pub(super) fn cs_zero_v3(_t: Desktop64) -> __m256i {
    _mm256_setzero_si256()
}

#[cfg(target_arch = "x86_64")]
#[rite]
pub(super) fn cs_madd_v3(_t: Desktop64, acc: __m256i, a: __m256i, b: __m256i) -> __m256i {
    _mm256_add_epi32(acc, _mm256_madd_epi16(a, b))
}

#[cfg(target_arch = "x86_64")]
#[rite]
pub(super) fn cs_msub_v3(_t: Desktop64, acc: __m256i, a: __m256i, b: __m256i) -> __m256i {
    _mm256_sub_epi32(acc, _mm256_madd_epi16(a, b))
}

/// Horizontal sum of the eight `i32` lanes, widened BEFORE adding (the
/// lane total may exceed `i32` even though each lane cannot).
#[cfg(target_arch = "x86_64")]
#[rite]
pub(super) fn cs_reduce_v3(_t: Desktop64, acc: __m256i) -> i64 {
    let mut a = [0i32; 8];
    _mm256_storeu_si256(&mut a, acc);
    a.iter().map(|&v| i64::from(v)).sum()
}

/// C's `find_average_avx2` (`pickrst_avx2.c:24`): the region's mean pixel,
/// summed 32 bytes at a time with `_mm256_sad_epu8` against zero into four
/// `u64` lanes. Exact — the same `u64` sum and truncating divide as
/// [`find_average`], so the same `u8`.
#[cfg(target_arch = "x86_64")]
#[rite]
#[allow(clippy::too_many_arguments)]
pub(super) fn cs_find_average_v3(
    _t: Desktop64,
    src: &[u8],
    origin: usize,
    stride: usize,
    h_start: i32,
    h_end: i32,
    v_start: i32,
    v_end: i32,
) -> u8 {
    let width = (h_end - h_start) as usize;
    let height = (v_end - v_start) as usize;
    let zero = _mm256_setzero_si256();
    let mut acc = _mm256_setzero_si256();
    let mut tail: u64 = 0;
    for r in 0..height {
        let base = (origin as isize
            + (v_start as isize + r as isize) * stride as isize
            + h_start as isize) as usize;
        let row = &src[base..base + width];
        let (c32, rem) = row.as_chunks::<32>();
        for ch in c32 {
            acc = _mm256_add_epi64(acc, _mm256_sad_epu8(_mm256_loadu_si256(ch), zero));
        }
        for &p in rem {
            tail += u64::from(p);
        }
    }
    let mut lanes = [0u64; 4];
    _mm256_storeu_si256(&mut lanes, acc);
    let sum = lanes.iter().sum::<u64>() + tail;
    (sum / (width as u64 * height as u64)) as u8
}

// ---- NEON lane primitives for `cs_kernel!` ------------------------------
//
// A 16-lane `i16` chunk is two `int16x8_t`; an accumulator is two
// `int32x4_t`. `cs_madd_neon` is C's `madd_neon` (`pickrst_neon.h:46`):
// `vmlal_s16` / `vmlal_high_s16` widen-multiply-accumulate 4 lanes each, so
// one accumulator lane receives TWO products per chunk, the same per-lane
// growth as the AVX2 pairwise `madd` — the drain interval below is
// ISA-independent.
