//! CDEF (Constrained Directional Enhancement Filter) kernels, C-exact.
//!
//! Ported from SVT-AV1 `Source/Lib/Codec/cdef.c` (v4.2.0-rc):
//! - [`cdef_find_dir`]      = `svt_aom_cdef_find_dir_c`      (cdef.c:88)
//! - [`cdef_find_dir_8bit`] = `svt_aom_cdef_find_dir_8bit_c` (cdef.c:303)
//! - [`cdef_filter_block`]  = `svt_cdef_filter_block_c`, dst8 arm (cdef.c:193)
//! - [`cdef_filter_block_8bit`] = `svt_cdef_filter_block_8bit_c` (cdef.c:257)
//!
//! libaom's `cdef_find_dir_c` / `cdef_filter_block_internal`
//! (av1/common/cdef_block.c — what aomdec runs) are the same math with two
//! packaging differences, both proven output-neutral at 8 bit:
//!
//! 1. **Sentinel value**: SVT marks unavailable pixels with
//!    `CDEF_VERY_LARGE = 0x7f7f`; libaom uses `0x4000`. Both are
//!    constrain-neutral — for any threshold <= 63 and damping <= 6+shift the
//!    damped clamp `max(0, threshold - (|diff| >> shift))` is exactly 0 when
//!    `|diff| >= 0x4000 - 255` (16129 >> 6 = 252 > 63) — are excluded from
//!    `max` by an equality compare against the *same* constant, and being
//!    large positive can never win `min`. Identical availability geometry
//!    therefore yields bit-identical output for either constant.
//! 2. **Strength-index dispatch**: libaom routes (t, sec) through 4 kernel
//!    variants where the min/max clamp only exists when BOTH strengths are
//!    nonzero. The SVT kernel (ported here) always clamps. Equivalent: with
//!    one side disabled its constrain() terms are 0, and a single-side
//!    filtered value provably stays within [min, max] of its own live taps
//!    (total tap weight 12: `y - x <= (8 + 12*d) >> 4 <= d` for the largest
//!    positive live-tap diff `d`, symmetrically for min), so the extra clamp
//!    never fires; with both disabled the output is exactly `x` either way.
//!
//! Only the 8-bit (`dst8`, `coeff_shift = 0..`) arm of the filter is ported —
//! the pipeline is 8-bit-only (the C `dst16` arm serves HBD and the encoder's
//! packed-output RDO search, neither of which exists here). All kernels are
//! differentially fuzzed bit-exact against `libSvtAv1Enc.a` in
//! `tests/c_parity_cdef.rs`.

use archmage::prelude::*;

/// 6-bit packed strength: `pri * CDEF_SEC_STRENGTHS + sec` (spec 5.9.19).
pub const CDEF_STRENGTH_BITS: u32 = 6;
/// Number of primary strengths (4-bit field).
pub const CDEF_PRI_STRENGTHS: i32 = 16;
/// Number of *signaled* secondary strengths (2-bit field; 3 decodes as 4).
pub const CDEF_SEC_STRENGTHS: i32 = 4;

/// Rows buffered above/below a filter block (`CDEF_VBORDER`, cdef.h).
pub const CDEF_VBORDER: usize = 3;
/// Columns buffered left/right (`CDEF_HBORDER`, cdef.h — 8 for alignment;
/// taps only reach +-2).
pub const CDEF_HBORDER: usize = 8;
/// Padded row stride of the CDEF intermediate buffer:
/// `ALIGN_POWER_OF_TWO(128 + 2 * CDEF_HBORDER, 3)` = 144.
pub const CDEF_BSTRIDE: usize = 144;
/// Intermediate buffer size (covers a 128px superblock; we use 64px).
pub const CDEF_INBUF_SIZE: usize = CDEF_BSTRIDE * (128 + 2 * CDEF_VBORDER);

/// Unavailable-pixel sentinel, SVT convention (`(uint8_t)~0 >> 1 |
/// ((uint8_t)~0 >> 1) << 8` = 0x7f7f). libaom uses 0x4000; see the module
/// docs for the bit-exactness argument.
pub const CDEF_VERY_LARGE: u16 = 0x7f7f;

/// SVT/libaom `BLOCK_4X4` (definitions.h:924 — enum starts at 0).
pub const BLOCK_4X4: i32 = 0;
/// SVT/libaom `BLOCK_4X8`.
pub const BLOCK_4X8: i32 = 1;
/// SVT/libaom `BLOCK_8X4`.
pub const BLOCK_8X4: i32 = 2;
/// SVT/libaom `BLOCK_8X8`.
pub const BLOCK_8X8: i32 = 3;

/// `eb_cdef_directions_padded` (cdef.c:35): Cdef_Directions (spec 7.15.3)
/// with 2 padding entries at each end so `dir - 2 .. dir + 2` indexes without
/// masking. Offsets are into a `CDEF_BSTRIDE`-strided buffer.
const CDEF_DIRECTIONS_PADDED: [[i32; 2]; 12] = {
    const S: i32 = CDEF_BSTRIDE as i32;
    [
        /* padding: directions[6] */ [S, 2 * S],
        /* padding: directions[7] */ [S, 2 * S - 1],
        [-S + 1, -2 * S + 2],
        [1, -S + 2],
        [1, 2],
        [1, S + 2],
        [S + 1, 2 * S + 2],
        [S, 2 * S + 1],
        [S, 2 * S],
        [S, 2 * S - 1],
        /* padding: directions[0] */ [-S + 1, -2 * S + 2],
        /* padding: directions[1] */ [1, -S + 2],
    ]
};

/// `svt_aom_eb_cdef_directions[dir][k]` with the C `+2` base offset:
/// accepts `dir` in `-2..=9`.
#[inline]
fn cdef_direction(dir: i32, k: usize) -> i32 {
    CDEF_DIRECTIONS_PADDED[(dir + 2) as usize][k]
}

/// `svt_aom_eb_cdef_pri_taps` (cdef.c:189), row selected by
/// `(pri_strength >> coeff_shift) & 1`.
const CDEF_PRI_TAPS: [[i32; 2]; 2] = [[4, 2], [3, 3]];
/// `svt_aom_eb_cdef_sec_taps` (cdef.c:190) — both rows identical.
const CDEF_SEC_TAPS: [[i32; 2]; 2] = [[2, 1], [2, 1]];

/// C `get_msb` (definitions.h:603): `31 - clz(n)` = floor(log2(n)), n > 0.
#[inline]
fn get_msb(n: u32) -> i32 {
    debug_assert!(n != 0);
    31 - n.leading_zeros() as i32
}

/// C `constrain` (cdef.c:20): damped, sign-preserving tap clamp.
#[inline]
fn constrain(diff: i32, threshold: i32, damping: i32) -> i32 {
    if threshold == 0 {
        return 0;
    }
    let shift = (damping - get_msb(threshold as u32)).max(0);
    let sign = if diff < 0 { -1 } else { 1 };
    sign * diff.abs().min((threshold - (diff.abs() >> shift)).max(0))
}

/// C `adjust_strength` (cdef.c:66): scale the primary strength for a luma
/// 8x8 by its directional-variance class (`var` from [`cdef_find_dir`]).
#[inline]
pub fn adjust_strength(strength: i32, var: i32) -> i32 {
    let i = if var >> 6 != 0 {
        get_msb((var >> 6) as u32).min(12)
    } else {
        0
    };
    if var != 0 {
        (strength * (4 + i) + 8) >> 4
    } else {
        0
    }
}

/// `svt_aom_cdef_find_dir_c` (cdef.c:88): direction search over an 8x8 block
/// of 16-bit pixels. Returns `(best_dir, var)`. 0 = 45-degree up-right,
/// 2 = horizontal, 4 = down-right, 6 = vertical (spec 7.15.3 ordering).
///
/// Reads exactly the 8x8 interior — border/sentinel pixels are never seen.
///
/// Runtime-dispatched (`incant!([neon, scalar])`). Both arms build the SAME
/// `partial[8][15]` array and then run the SAME cost/argmax tail
/// ([`cdef_dir_from_partials`]), so only the accumulation differs and the
/// arithmetic that turns partials into `(dir, var)` cannot diverge between
/// tiers by construction. C ships `svt_aom_cdef_find_dir_neon`
/// (`cdef_block_neon.c:337`) here and the port was scalar: 15x on the measured
/// profile (`benchmarks/perf_videokey_attrib_2026-09-03.meta` —
/// `cdef::cdef_find_dir` 0.918 ms against C's `cdef_dir_from_lines_neon`
/// 0.069 ms at 512x512 preset 8).
pub fn cdef_find_dir(img: &[u16], stride: usize, coeff_shift: i32) -> (u8, i32) {
    incant!(cdef_find_dir_impl(img, stride, coeff_shift), [neon, scalar])
}

/// 840/n for n in 1..=8 (offset by 1; entry 0 unused).
const DIV_TABLE: [i32; 9] = [0, 840, 420, 280, 210, 168, 140, 120, 105];

/// The eight direction partial-sum arrays, verbatim `svt_aom_cdef_find_dir_c`'s
/// accumulation loop. `partial[k][m]` collects every pixel whose (row, col)
/// maps to index `m` under direction `k`'s formula.
fn cdef_dir_partials_scalar(img: &[u16], stride: usize, coeff_shift: i32) -> [[i32; 15]; 8] {
    let mut partial = [[0i32; 15]; 8];
    for i in 0..8usize {
        for j in 0..8usize {
            let x = ((img[i * stride + j] as i32) >> coeff_shift) - 128;
            partial[0][i + j] += x;
            partial[1][i + j / 2] += x;
            partial[2][i] += x;
            partial[3][3 + i - j / 2] += x;
            partial[4][7 + i - j] += x;
            partial[5][3 - i / 2 + j] += x;
            partial[6][j] += x;
            partial[7][i / 2 + j] += x;
        }
    }
    partial
}

/// The cost/argmax/variance tail of `svt_aom_cdef_find_dir_c`, verbatim, split
/// out so every dispatch tier shares it. Only the partial-sum ACCUMULATION is
/// tier-specific; this half is one copy.
fn cdef_dir_from_partials(partial: &[[i32; 15]; 8]) -> (u8, i32) {
    let mut cost = [0i32; 8];
    let mut best_cost = 0i32;
    let mut best_dir = 0usize;
    for i in 0..8 {
        cost[2] += partial[2][i] * partial[2][i];
        cost[6] += partial[6][i] * partial[6][i];
    }
    cost[2] *= DIV_TABLE[8];
    cost[6] *= DIV_TABLE[8];
    for i in 0..7 {
        cost[0] += (partial[0][i] * partial[0][i] + partial[0][14 - i] * partial[0][14 - i])
            * DIV_TABLE[i + 1];
        cost[4] += (partial[4][i] * partial[4][i] + partial[4][14 - i] * partial[4][14 - i])
            * DIV_TABLE[i + 1];
    }
    cost[0] += partial[0][7] * partial[0][7] * DIV_TABLE[8];
    cost[4] += partial[4][7] * partial[4][7] * DIV_TABLE[8];
    let mut i = 1;
    while i < 8 {
        for j in 0..5 {
            cost[i] += partial[i][3 + j] * partial[i][3 + j];
        }
        cost[i] *= DIV_TABLE[8];
        for j in 0..3 {
            cost[i] += (partial[i][j] * partial[i][j] + partial[i][10 - j] * partial[i][10 - j])
                * DIV_TABLE[2 * j + 2];
        }
        i += 2;
    }
    for (i, &c) in cost.iter().enumerate() {
        if c > best_cost {
            best_cost = c;
            best_dir = i;
        }
    }
    let mut var = best_cost - cost[(best_dir + 4) & 7];
    var >>= 10;
    (best_dir as u8, var)
}

fn cdef_find_dir_impl_scalar(
    _token: ScalarToken,
    img: &[u16],
    stride: usize,
    coeff_shift: i32,
) -> (u8, i32) {
    cdef_dir_from_partials(&cdef_dir_partials_scalar(img, stride, coeff_shift))
}

/// NEON partial-sum accumulation for [`cdef_find_dir`].
///
/// Every one of the eight directions is the SAME shape once the index formula
/// is read as "place a row-derived vector at an offset":
///
/// ```text
///   k=0  partial[i + j]        <- the row,                 at offset i
///   k=1  partial[i + j/2]      <- adjacent-pair sums,      at offset i
///   k=2  partial[i]            <- the row's horizontal sum
///   k=3  partial[3 + i - j/2]  <- REVERSED pair sums,      at offset i
///   k=4  partial[7 + i - j]    <- the REVERSED row,        at offset i
///   k=5  partial[3 - i/2 + j]  <- the row,                 at offset 3 - i/2
///   k=6  partial[j]            <- the column sums
///   k=7  partial[i/2 + j]      <- the row,                 at offset i/2
/// ```
///
/// so the scalar's 8 x 64 = 512 accumulations become, per row, one 8-lane
/// vector add per placed direction plus a pairwise add and two reverses.
///
/// THE ACCUMULATORS LIVE IN REGISTERS, which is why the row loop is unrolled
/// by hand. A first version kept them as `[[i16; 16]; 8]` in memory and did
/// `acc[off..off+8] += v` with a load/store pair; that MEASURED 1.56x over the
/// scalar on `benches/kernel_tiers.rs` and NULL on the whole encoder, because
/// each direction's accumulator is re-loaded immediately after being stored and
/// the store-to-load latency serialises all eight rows. Two 8-lane registers
/// per placed direction (indices 0..7 and 8..15) plus `vextq_s16` for the
/// offset removes that — the shape C's `compute_vert_directions_neon` /
/// `compute_horiz_directions_neon` use. `vextq_s16::<N>` needs a literal `N`,
/// which is why the eight rows are written out.
///
/// EXACTNESS. `x = (img >> coeff_shift) - 128` and every real caller feeds
/// reconstructed pixels at the frame's bit depth with `coeff_shift = bd - 8`,
/// so `img >> coeff_shift <= 255` and `x` is in `[-128, 127]`; a partial is a
/// sum of at most 8 of them, `|partial| <= 1024`, exact in `i16`. That bound is
/// CHECKED rather than assumed — if any shifted pixel exceeds 255 the caller
/// falls back to the scalar accumulation, so the tier can never disagree with
/// `svt_aom_cdef_find_dir_c` on an input outside the domain. The cost tail is
/// shared code ([`cdef_dir_from_partials`]), not a second transcription.
#[cfg(target_arch = "aarch64")]
#[rite]
fn cdef_dir_partials_neon(
    _token: NeonToken,
    img: &[u16],
    stride: usize,
    coeff_shift: i32,
) -> Option<[[i32; 15]; 8]> {
    let shift = vdupq_n_s16(-(coeff_shift as i16));
    let bias = vdupq_n_u16(128);
    let zero = vdupq_n_s16(0);
    let mut rows = [zero; 8];
    let mut over = vdupq_n_u16(0);
    for (i, r) in rows.iter_mut().enumerate() {
        let src: &[u16; 8] = img[i * stride..i * stride + 8].try_into().ok()?;
        let v = vshlq_u16(vld1q_u16(src), shift);
        over = vmaxq_u16(over, v);
        *r = vreinterpretq_s16_u16(vsubq_u16(v, bias));
    }
    // Outside `[0, 255]` the i16 partials could overflow; hand those to the
    // scalar reference rather than risk a silently different direction.
    if vmaxvq_u16(over) > 255 {
        return None;
    }

    // `a[k] = [indices 0..7, indices 8..15]` for the six PLACED directions
    // (k = 2 and k = 6 are the row and column sums and need no placement).
    let mut a = [[zero; 2]; 8];
    let mut colsum = zero;

    // `place!(k, N, v)` adds `v` at offset `8 - N`: `vextq_s16::<N>(zero, v)`
    // is `8 - N` zeros followed by `v[0 .. N]`, and `vextq_s16::<N>(v, zero)`
    // is the part that spills past lane 7. `place0!` is the offset-0 case
    // (`vextq_s16::<8>` does not exist and nothing spills).
    macro_rules! place {
        ($k:literal, $n:literal, $v:expr) => {{
            a[$k][0] = vaddq_s16(a[$k][0], vextq_s16::<$n>(zero, $v));
            a[$k][1] = vaddq_s16(a[$k][1], vextq_s16::<$n>($v, zero));
        }};
    }
    macro_rules! place0 {
        ($k:literal, $v:expr) => {{
            a[$k][0] = vaddq_s16(a[$k][0], $v);
        }};
    }

    // row 0
    {
        let v = rows[0];
        let pv = vpaddq_s16(v, zero);
        let rq = vrev64q_s16(v);
        let rv = vextq_s16::<4>(rq, rq);
        let rpv = vrev64q_s16(pv);
        colsum = vaddq_s16(colsum, v);
        place0!(0, v);
        place0!(1, pv);
        place0!(3, rpv);
        place0!(4, rv);
        place!(5, 5, v);
        place0!(7, v);
    }
    // row 1
    {
        let v = rows[1];
        let pv = vpaddq_s16(v, zero);
        let rq = vrev64q_s16(v);
        let rv = vextq_s16::<4>(rq, rq);
        let rpv = vrev64q_s16(pv);
        colsum = vaddq_s16(colsum, v);
        place!(0, 7, v);
        place!(1, 7, pv);
        place!(3, 7, rpv);
        place!(4, 7, rv);
        place!(5, 5, v);
        place0!(7, v);
    }
    // row 2
    {
        let v = rows[2];
        let pv = vpaddq_s16(v, zero);
        let rq = vrev64q_s16(v);
        let rv = vextq_s16::<4>(rq, rq);
        let rpv = vrev64q_s16(pv);
        colsum = vaddq_s16(colsum, v);
        place!(0, 6, v);
        place!(1, 6, pv);
        place!(3, 6, rpv);
        place!(4, 6, rv);
        place!(5, 6, v);
        place!(7, 7, v);
    }
    // row 3
    {
        let v = rows[3];
        let pv = vpaddq_s16(v, zero);
        let rq = vrev64q_s16(v);
        let rv = vextq_s16::<4>(rq, rq);
        let rpv = vrev64q_s16(pv);
        colsum = vaddq_s16(colsum, v);
        place!(0, 5, v);
        place!(1, 5, pv);
        place!(3, 5, rpv);
        place!(4, 5, rv);
        place!(5, 6, v);
        place!(7, 7, v);
    }
    // row 4
    {
        let v = rows[4];
        let pv = vpaddq_s16(v, zero);
        let rq = vrev64q_s16(v);
        let rv = vextq_s16::<4>(rq, rq);
        let rpv = vrev64q_s16(pv);
        colsum = vaddq_s16(colsum, v);
        place!(0, 4, v);
        place!(1, 4, pv);
        place!(3, 4, rpv);
        place!(4, 4, rv);
        place!(5, 7, v);
        place!(7, 6, v);
    }
    // row 5
    {
        let v = rows[5];
        let pv = vpaddq_s16(v, zero);
        let rq = vrev64q_s16(v);
        let rv = vextq_s16::<4>(rq, rq);
        let rpv = vrev64q_s16(pv);
        colsum = vaddq_s16(colsum, v);
        place!(0, 3, v);
        place!(1, 3, pv);
        place!(3, 3, rpv);
        place!(4, 3, rv);
        place!(5, 7, v);
        place!(7, 6, v);
    }
    // row 6
    {
        let v = rows[6];
        let pv = vpaddq_s16(v, zero);
        let rq = vrev64q_s16(v);
        let rv = vextq_s16::<4>(rq, rq);
        let rpv = vrev64q_s16(pv);
        colsum = vaddq_s16(colsum, v);
        place!(0, 2, v);
        place!(1, 2, pv);
        place!(3, 2, rpv);
        place!(4, 2, rv);
        place0!(5, v);
        place!(7, 5, v);
    }
    // row 7
    {
        let v = rows[7];
        let pv = vpaddq_s16(v, zero);
        let rq = vrev64q_s16(v);
        let rv = vextq_s16::<4>(rq, rq);
        let rpv = vrev64q_s16(pv);
        colsum = vaddq_s16(colsum, v);
        place!(0, 1, v);
        place!(1, 1, pv);
        place!(3, 1, rpv);
        place!(4, 1, rv);
        place0!(5, v);
        place!(7, 5, v);
    }

    let mut partial = [[0i32; 15]; 8];
    let mut lanes = [0i16; 8];
    for k in [0usize, 1, 3, 4, 5, 7] {
        vst1q_s16(&mut lanes, a[k][0]);
        for m in 0..8 {
            partial[k][m] = lanes[m] as i32;
        }
        vst1q_s16(&mut lanes, a[k][1]);
        for m in 0..7 {
            partial[k][8 + m] = lanes[m] as i32;
        }
    }
    // The eight ROW sums (`partial[2]`) as a pairwise-add tree over the eight
    // row vectors: six `vpaddq_s16` instead of eight `vaddvq_s16`, which are
    // cross-lane reductions with a serial dependency each.
    let p01 = vpaddq_s16(rows[0], rows[1]);
    let p23 = vpaddq_s16(rows[2], rows[3]);
    let p45 = vpaddq_s16(rows[4], rows[5]);
    let p67 = vpaddq_s16(rows[6], rows[7]);
    let rowsums = vpaddq_s16(vpaddq_s16(p01, p23), vpaddq_s16(p45, p67));
    let mut rs = [0i16; 8];
    vst1q_s16(&mut rs, rowsums);
    vst1q_s16(&mut lanes, colsum);
    for i in 0..8 {
        partial[2][i] = rs[i] as i32;
        partial[6][i] = lanes[i] as i32;
    }
    Some(partial)
}

#[cfg(target_arch = "aarch64")]
#[arcane]
fn cdef_find_dir_impl_neon(
    token: NeonToken,
    img: &[u16],
    stride: usize,
    coeff_shift: i32,
) -> (u8, i32) {
    match cdef_dir_partials_neon(token, img, stride, coeff_shift) {
        Some(p) => cdef_dir_from_partials(&p),
        None => cdef_dir_from_partials(&cdef_dir_partials_scalar(img, stride, coeff_shift)),
    }
}

/// `svt_aom_cdef_find_dir_8bit_c` (cdef.c:303): widen an 8x8 of 8-bit pixels
/// to 16 bit and delegate to [`cdef_find_dir`].
pub fn cdef_find_dir_8bit(img: &[u8], stride: usize, coeff_shift: i32) -> (u8, i32) {
    let mut img16 = [0u16; 64];
    for i in 0..8 {
        for j in 0..8 {
            img16[i * 8 + j] = img[i * stride + j] as u16;
        }
    }
    cdef_find_dir(&img16, 8, coeff_shift)
}

/// C `clamp` (definitions.h).
#[inline]
fn clamp_i32(value: i32, low: i32, high: i32) -> i32 {
    if value < low {
        low
    } else if value > high {
        high
    } else {
        value
    }
}

/// `svt_cdef_filter_block_c` (cdef.c:193), dst8 arm: primary + secondary
/// directional filtering of one block inside a `CDEF_BSTRIDE`-strided 16-bit
/// buffer where unavailable pixels hold [`CDEF_VERY_LARGE`].
///
/// `inb`/`ioff`: padded input buffer and the index of the block's (0,0)
/// pixel (tap offsets are signed; `ioff` must leave `CDEF_VBORDER` rows and
/// at least 2 columns of headroom, which any `CDEF_INBUF_SIZE` layout does).
/// `dst`/`doff`/`dstride`: 8-bit output. `dir`: 0..=7. `bsize`: one of
/// [`BLOCK_8X8`]/[`BLOCK_4X8`]/[`BLOCK_8X4`]/[`BLOCK_4X4`].
/// `subsampling_factor` (1 or 2) skips every other row (C search decimation;
/// the decoder path always passes 1).
#[allow(clippy::too_many_arguments)]
pub fn cdef_filter_block(
    dst: &mut [u8],
    doff: usize,
    dstride: usize,
    inb: &[u16],
    ioff: usize,
    pri_strength: i32,
    sec_strength: i32,
    dir: i32,
    pri_damping: i32,
    sec_damping: i32,
    bsize: i32,
    coeff_shift: i32,
    subsampling_factor: usize,
) {
    incant!(
        cdef_filter_block_impl(
            dst,
            doff,
            dstride,
            inb,
            ioff,
            pri_strength,
            sec_strength,
            dir,
            pri_damping,
            sec_damping,
            bsize,
            coeff_shift,
            subsampling_factor
        ),
        [v3, neon, scalar]
    )
}

#[allow(clippy::too_many_arguments)]
fn cdef_filter_block_impl_scalar(
    _token: ScalarToken,
    dst: &mut [u8],
    doff: usize,
    dstride: usize,
    inb: &[u16],
    ioff: usize,
    pri_strength: i32,
    sec_strength: i32,
    dir: i32,
    pri_damping: i32,
    sec_damping: i32,
    bsize: i32,
    coeff_shift: i32,
    subsampling_factor: usize,
) {
    cdef_filter_block_core(
        dst,
        doff,
        dstride,
        inb,
        ioff,
        pri_strength,
        sec_strength,
        dir,
        pri_damping,
        sec_damping,
        bsize,
        coeff_shift,
        subsampling_factor,
    );
}

/// NEON port of [`cdef_filter_cols8_v3`]. Same algorithm, same guarantees:
/// each output pixel is an independent 12-tap integer sum, so 8 columns map to
/// 8 lanes with NO cross-lane reduction. NEON's i32 vectors are 4 lanes, so
/// each "8-wide" quantity is carried as a `[int32x4_t; 2]` pair.
///
/// The `sum` is accumulated in i32 and sign-truncated to i16 once at the end
/// (`(x<<16)>>16`); two's-complement add is associative mod 2^16, so that
/// equals the scalar's per-tap `wrapping_add::<i16>` exactly. Products are
/// small enough that the i32 accumulator cannot overflow across 12 taps.
#[cfg(target_arch = "aarch64")]
#[rite]
fn cdef_load8_u16_neon(_token: NeonToken, inb: &[u16], idx: usize) -> [int32x4_t; 2] {
    let a: &[u16; 8] = inb[idx..idx + 8].try_into().unwrap();
    // SIGN-extend, not zero-extend. The AVX2 arm uses `_mm256_cvtepi16_epi32`,
    // i.e. it reads the buffer as int16_t, so values >= 0x8000 are NEGATIVE.
    // Zero-extending here produced a silent mismatch that only appears when
    // taps straddle 0x8000 — caught by `filter_block_sign_straddle_matches_c`,
    // which is built precisely to hit that boundary.
    let v = vreinterpretq_s16_u16(vld1q_u16(a));
    [vmovl_s16(vget_low_s16(v)), vmovl_s16(vget_high_s16(v))]
}

/// Load 8 taps as a BIASED `uint16x8_t` (`value ^ 0x8000`).
///
/// The buffer is `int16_t` to every arm of this kernel — the AVX2 arm reads it
/// with `_mm256_cvtepi16_epi32` and the scalar core with `at(..) as i16`, so a
/// tap at or above 0x8000 is NEGATIVE. Biasing by `^ 0x8000` maps that signed
/// value into `[0, 65535]` **preserving order and preserving differences**
/// (`a_biased - b_biased == a_signed - b_signed`), which is what lets the whole
/// constrain step run in u16 lanes with saturating arithmetic and stay exact
/// over the entire i16 domain. C's NEON arm works in the same u16 domain
/// (`ASM_NEON/cdef_filter_block_neon.c:18`, `constrain_neon(uint16x8_t a,
/// uint16x8_t b, ...)`); it does not need the bias only because its inputs are
/// pixel values below 0x8000.
#[cfg(target_arch = "aarch64")]
#[rite]
fn cdef_load8_bias_neon(_token: NeonToken, inb: &[u16], idx: usize) -> uint16x8_t {
    let a: &[u16; 8] = inb[idx..idx + 8].try_into().unwrap();
    veorq_u16(vld1q_u16(a), vdupq_n_u16(0x8000))
}

/// SIMD [`constrain`] over 8 biased taps against the biased centre pixel,
/// returning `sign(tap - x) * min(|tap - x|, max(thr - (|tap - x| >> shift), 0))`
/// as an `int16x8_t`.
///
/// EXACT over the whole i16 domain, with no bound on the strengths or the
/// damping, because every step is exact in u16:
/// * `vabdq_u16` on the biased pair IS `|tap - x|` computed on the SIGNED
///   values (the bias cancels in the difference) and cannot overflow — the
///   largest possible value is 65535.
/// * `vqsubq_u16(thr, shifted)` IS `max(thr - shifted, 0)`; the saturation is
///   the clamp, not an approximation of it.
/// * `m = min(adiff, capped) <= thr <= 32767`, so reinterpreting it as `i16`
///   and negating it cannot overflow.
///
/// The i32 arm this replaces widened to `[int32x4_t; 2]` per row and therefore
/// issued twice the vector operations for the same 8 columns; C has always
/// worked in `int16x8` here.
#[cfg(target_arch = "aarch64")]
#[rite]
fn cdef_constrain8_bias_neon(
    _token: NeonToken,
    tap: uint16x8_t,
    x: uint16x8_t,
    thr: uint16x8_t,
    neg_shift: int16x8_t,
    active: bool,
) -> int16x8_t {
    if !active {
        return vdupq_n_s16(0);
    }
    let adiff = vabdq_u16(tap, x);
    // Negative shift count = right shift in NEON's variable-shift.
    let shifted = vshlq_u16(adiff, neg_shift);
    let capped = vqsubq_u16(thr, shifted);
    let m = vreinterpretq_s16_u16(vminq_u16(adiff, capped));
    // sign(tap - x) * m; `m` is already 0 where the taps are equal.
    vbslq_s16(vcltq_u16(tap, x), vnegq_s16(m), m)
}

/// SIMD [`constrain`] over an i32x4 of diffs. `threshold`/`shift` are scalar
/// (broadcast); `active == false` disables the tap, matching the scalar
/// early-return for `threshold == 0`.
#[cfg(target_arch = "aarch64")]
#[rite]
fn cdef_constrain4_neon(
    _token: NeonToken,
    diff: int32x4_t,
    thr: int32x4_t,
    shift: int32x4_t,
    active: bool,
) -> int32x4_t {
    if !active {
        return vdupq_n_s32(0);
    }
    let adiff = vabsq_s32(diff);
    // Negative shift count = right shift in NEON's variable-shift.
    let shifted = vshlq_s32(adiff, vnegq_s32(shift));
    let capped = vmaxq_s32(vsubq_s32(thr, shifted), vdupq_n_s32(0));
    let m = vminq_s32(adiff, capped);
    // sign(diff) * m — negate where diff < 0. m is already 0 where diff == 0.
    let neg = vcltq_s32(diff, vdupq_n_s32(0));
    vbslq_s32(neg, vnegq_s32(m), m)
}

#[cfg(target_arch = "aarch64")]
#[rite]
#[allow(clippy::too_many_arguments)]
pub(crate) fn cdef_filter_cols8_neon(
    token: NeonToken,
    inb: &[u16],
    ioff: usize,
    pri_strength: i32,
    sec_strength: i32,
    dir: i32,
    pri_damping: i32,
    sec_damping: i32,
    coeff_shift: i32,
    rows: i32,
    sub: i32,
    out: &mut [i32; 64],
) {
    let s = CDEF_BSTRIDE as i32;
    let pri_taps = CDEF_PRI_TAPS[((pri_strength >> coeff_shift) & 1) as usize];
    let sec_taps = CDEF_SEC_TAPS[((pri_strength >> coeff_shift) & 1) as usize];

    let pri_active = pri_strength != 0;
    let sec_active = sec_strength != 0;
    let pri_shift = if pri_active {
        (pri_damping - get_msb(pri_strength as u32)).max(0)
    } else {
        0
    };
    let sec_shift = if sec_active {
        (sec_damping - get_msb(sec_strength as u32)).max(0)
    } else {
        0
    };
    // u16 lanes for the constrain chain (see `cdef_constrain8_bias_neon`);
    // NEON takes a NEGATIVE count for a variable right shift.
    let pri_nsh = vdupq_n_s16(-(pri_shift as i16));
    let sec_nsh = vdupq_n_s16(-(sec_shift as i16));
    let pri_thr = vdupq_n_u16(pri_strength as u16);
    let sec_thr = vdupq_n_u16(sec_strength as u16);
    // The sentinel compared in the BIASED domain, like every tap here.
    let sentinel = vdupq_n_u16((CDEF_VERY_LARGE as u16) ^ 0x8000);
    let eight = vdupq_n_s32(8);

    let p_off = [
        cdef_direction(dir, 0),
        -cdef_direction(dir, 0),
        cdef_direction(dir, 1),
        -cdef_direction(dir, 1),
    ];
    let p_cof = [pri_taps[0], pri_taps[0], pri_taps[1], pri_taps[1]];
    let s_off = [
        cdef_direction(dir + 2, 0),
        -cdef_direction(dir + 2, 0),
        cdef_direction(dir - 2, 0),
        -cdef_direction(dir - 2, 0),
        cdef_direction(dir + 2, 1),
        -cdef_direction(dir + 2, 1),
        cdef_direction(dir - 2, 1),
        -cdef_direction(dir - 2, 1),
    ];
    let s_cof = [
        sec_taps[0],
        sec_taps[0],
        sec_taps[0],
        sec_taps[0],
        sec_taps[1],
        sec_taps[1],
        sec_taps[1],
        sec_taps[1],
    ];

    let mut i = 0i32;
    while i < rows {
        let base = (ioff as i32 + i * s) as usize;
        let x = cdef_load8_bias_neon(token, inb, base);
        let mut sum = vdupq_n_s16(0);
        let mut mx = x;
        let mut mn = x;

        for t in 0..4usize {
            let idx = (base as i32 + p_off[t]) as usize;
            let tap = cdef_load8_bias_neon(token, inb, idx);
            let cof = vdupq_n_s16(p_cof[t] as i16);
            let c = cdef_constrain8_bias_neon(token, tap, x, pri_thr, pri_nsh, pri_active);
            // `vmulq_s16` keeps the LOW 16 bits of the product, which is
            // exactly the scalar core's `(pri_taps[k] * constrain(..)) as i16`,
            // and `vaddq_s16` wraps exactly like its `wrapping_add`.
            sum = vaddq_s16(sum, vmulq_s16(c, cof));
            let is_sent = vceqq_u16(tap, sentinel);
            mx = vbslq_u16(is_sent, mx, vmaxq_u16(mx, tap));
            mn = vminq_u16(mn, tap);
        }
        for t in 0..8usize {
            let idx = (base as i32 + s_off[t]) as usize;
            let tap = cdef_load8_bias_neon(token, inb, idx);
            let cof = vdupq_n_s16(s_cof[t] as i16);
            let c = cdef_constrain8_bias_neon(token, tap, x, sec_thr, sec_nsh, sec_active);
            sum = vaddq_s16(sum, vmulq_s16(c, cof));
            let is_sent = vceqq_u16(tap, sentinel);
            mx = vbslq_u16(is_sent, mx, vmaxq_u16(mx, tap));
            mn = vminq_u16(mn, tap);
        }

        // Unbias back to the signed domain for the tail.
        let bias = vdupq_n_u16(0x8000);
        let x_s = vreinterpretq_s16_u16(veorq_u16(x, bias));
        let mx_s = vreinterpretq_s16_u16(veorq_u16(mx, bias));
        let mn_s = vreinterpretq_s16_u16(veorq_u16(mn, bias));

        // THE TAIL STAYS IN i32, deliberately. The scalar core computes
        // `x + ((8 + sum - (sum < 0)) >> 4)` on the SIGN-EXTENDED `sum`, and
        // `8 + sum` leaves i16 range for a `sum` near 32767 — unreachable with
        // AV1 strengths but reachable by a synthetic caller, and "unreachable"
        // is not a proof. `vmovl_s16` of the i16 accumulator IS the previous
        // arm's `(sum << 16) >> 16` sign-truncation, so the mod-2^16 argument
        // that arm carried is unchanged: two's-complement addition is
        // associative mod 2^16, so an i16 accumulator equals the scalar's
        // per-tap `wrapping_add::<i16>` bit for bit.
        let sum32 = [vmovl_s16(vget_low_s16(sum)), vmovl_s16(vget_high_s16(sum))];
        let x32 = [vmovl_s16(vget_low_s16(x_s)), vmovl_s16(vget_high_s16(x_s))];
        let mn32 = [
            vmovl_s16(vget_low_s16(mn_s)),
            vmovl_s16(vget_high_s16(mn_s)),
        ];
        let mx32 = [
            vmovl_s16(vget_low_s16(mx_s)),
            vmovl_s16(vget_high_s16(mx_s)),
        ];
        for h in 0..2usize {
            let sw = sum32[h];
            let neg = vreinterpretq_s32_u32(vshrq_n_u32::<31>(vreinterpretq_u32_s32(sw)));
            let adj = vshrq_n_s32::<4>(vsubq_s32(vaddq_s32(eight, sw), neg));
            let val = vaddq_s32(x32[h], adj);
            let y = vminq_s32(vmaxq_s32(val, mn32[h]), mx32[h]);
            let dst: &mut [i32; 4] = (&mut out[i as usize * 8 + h * 4..i as usize * 8 + h * 4 + 4])
                .try_into()
                .unwrap();
            vst1q_s32(dst, y);
        }
        i += sub;
    }
}

/// 4-lane twin of [`cdef_load8_u16_neon`] for the 4-wide chroma shapes.
/// Same SIGN-extension (the buffer is read as `int16_t`, so taps at or above
/// 0x8000 are negative — the exact bug `filter_block_sign_straddle_matches_c`
/// exists to catch).
#[cfg(target_arch = "aarch64")]
#[rite]
fn cdef_load4_u16_neon(_token: NeonToken, inb: &[u16], idx: usize) -> int32x4_t {
    let a: &[u16; 4] = inb[idx..idx + 4].try_into().unwrap();
    vmovl_s16(vreinterpret_s16_u16(vld1_u16(a)))
}

/// 4-wide chroma CDEF — [`cdef_filter_cols8_neon`] with one `int32x4_t` per row
/// instead of a pair. C ships this shape as
/// `svt_av1_cdef_filter_block_4xn_8_native_neon`; the port previously fell back
/// to [`cdef_filter_block_core`] for BLOCK_4X4 / BLOCK_4X8, which profiling
/// measured at 6.08 % of the port's preset-10 self time at 512x512.
///
/// Byte-identity rests on the same property as the 8-wide arm: each output
/// pixel is an INDEPENDENT 12-tap integer sum, so columns map to lanes with no
/// cross-lane reduction, and the number of lanes cannot change a result. `sum`
/// accumulates in i32 and is sign-truncated to i16 once at the end
/// (`(x << 16) >> 16`); two's-complement addition is associative mod 2^16, so
/// that equals the scalar's per-tap `wrapping_add::<i16>`.
#[cfg(target_arch = "aarch64")]
#[rite]
#[allow(clippy::too_many_arguments)]
pub(crate) fn cdef_filter_cols4_neon(
    token: NeonToken,
    inb: &[u16],
    ioff: usize,
    pri_strength: i32,
    sec_strength: i32,
    dir: i32,
    pri_damping: i32,
    sec_damping: i32,
    coeff_shift: i32,
    rows: i32,
    sub: i32,
    out: &mut [i32; 32],
) {
    let s = CDEF_BSTRIDE as i32;
    let pri_taps = CDEF_PRI_TAPS[((pri_strength >> coeff_shift) & 1) as usize];
    let sec_taps = CDEF_SEC_TAPS[((pri_strength >> coeff_shift) & 1) as usize];

    let pri_active = pri_strength != 0;
    let sec_active = sec_strength != 0;
    let pri_shift = if pri_active {
        (pri_damping - get_msb(pri_strength as u32)).max(0)
    } else {
        0
    };
    let sec_shift = if sec_active {
        (sec_damping - get_msb(sec_strength as u32)).max(0)
    } else {
        0
    };
    let pri_shift_v = vdupq_n_s32(pri_shift);
    let sec_shift_v = vdupq_n_s32(sec_shift);
    let pri_thr = vdupq_n_s32(pri_strength);
    let sec_thr = vdupq_n_s32(sec_strength);
    let sentinel = vdupq_n_s32(CDEF_VERY_LARGE as i32);
    let eight = vdupq_n_s32(8);

    let p_off = [
        cdef_direction(dir, 0),
        -cdef_direction(dir, 0),
        cdef_direction(dir, 1),
        -cdef_direction(dir, 1),
    ];
    let p_cof = [pri_taps[0], pri_taps[0], pri_taps[1], pri_taps[1]];
    let s_off = [
        cdef_direction(dir + 2, 0),
        -cdef_direction(dir + 2, 0),
        cdef_direction(dir - 2, 0),
        -cdef_direction(dir - 2, 0),
        cdef_direction(dir + 2, 1),
        -cdef_direction(dir + 2, 1),
        cdef_direction(dir - 2, 1),
        -cdef_direction(dir - 2, 1),
    ];
    let s_cof = [
        sec_taps[0],
        sec_taps[0],
        sec_taps[0],
        sec_taps[0],
        sec_taps[1],
        sec_taps[1],
        sec_taps[1],
        sec_taps[1],
    ];

    let mut i = 0i32;
    while i < rows {
        let base = (ioff as i32 + i * s) as usize;
        let x = cdef_load4_u16_neon(token, inb, base);
        let mut sum = vdupq_n_s32(0);
        let mut mx = x;
        let mut mn = x;

        for t in 0..4usize {
            let idx = (base as i32 + p_off[t]) as usize;
            let tap = cdef_load4_u16_neon(token, inb, idx);
            let cof = vdupq_n_s32(p_cof[t]);
            let diff = vsubq_s32(tap, x);
            let c = cdef_constrain4_neon(token, diff, pri_thr, pri_shift_v, pri_active);
            sum = vaddq_s32(sum, vmulq_s32(c, cof));
            let is_sent = vceqq_s32(tap, sentinel);
            mx = vbslq_s32(is_sent, mx, vmaxq_s32(mx, tap));
            mn = vminq_s32(mn, tap);
        }
        for t in 0..8usize {
            let idx = (base as i32 + s_off[t]) as usize;
            let tap = cdef_load4_u16_neon(token, inb, idx);
            let cof = vdupq_n_s32(s_cof[t]);
            let diff = vsubq_s32(tap, x);
            let c = cdef_constrain4_neon(token, diff, sec_thr, sec_shift_v, sec_active);
            sum = vaddq_s32(sum, vmulq_s32(c, cof));
            let is_sent = vceqq_s32(tap, sentinel);
            mx = vbslq_s32(is_sent, mx, vmaxq_s32(mx, tap));
            mn = vminq_s32(mn, tap);
        }

        // sign-extend the low 16 bits, then x + ((8 + sum - (sum<0)) >> 4)
        let sw = vshrq_n_s32::<16>(vshlq_n_s32::<16>(sum));
        let neg = vreinterpretq_s32_u32(vshrq_n_u32::<31>(vreinterpretq_u32_s32(sw)));
        let adj = vshrq_n_s32::<4>(vsubq_s32(vaddq_s32(eight, sw), neg));
        let val = vaddq_s32(x, adj);
        let y = vminq_s32(vmaxq_s32(val, mn), mx);
        let dst: &mut [i32; 4] = (&mut out[i as usize * 4..i as usize * 4 + 4])
            .try_into()
            .unwrap();
        vst1q_s32(dst, y);
        i += sub;
    }
}

#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn cdef_filter_block_impl_neon(
    token: NeonToken,
    dst: &mut [u8],
    doff: usize,
    dstride: usize,
    inb: &[u16],
    ioff: usize,
    pri_strength: i32,
    sec_strength: i32,
    dir: i32,
    pri_damping: i32,
    sec_damping: i32,
    bsize: i32,
    coeff_shift: i32,
    subsampling_factor: usize,
) {
    let rows = if bsize == BLOCK_8X8 || bsize == BLOCK_4X8 {
        8
    } else {
        4
    };
    let cols = if bsize == BLOCK_8X8 || bsize == BLOCK_8X4 {
        8
    } else {
        4
    };
    // Both column shapes now take a vector path. The 8-wide arm is the
    // original; the 4-wide chroma arm ([`cdef_filter_cols4_neon`]) is the same
    // kernel at one int32x4 per row instead of two, which is what the C
    // reference ships as `svt_av1_cdef_filter_block_4xn_8_native_neon`.
    if cols == 8 {
        let mut scratch = [0i32; 64];
        cdef_filter_cols8_neon(
            token,
            inb,
            ioff,
            pri_strength,
            sec_strength,
            dir,
            pri_damping,
            sec_damping,
            coeff_shift,
            rows,
            subsampling_factor as i32,
            &mut scratch,
        );
        let mut i = 0i32;
        while i < rows {
            let drow = doff + i as usize * dstride;
            let srow = i as usize * 8;
            for j in 0..8usize {
                dst[drow + j] = scratch[srow + j] as u8;
            }
            i += subsampling_factor as i32;
        }
    } else {
        let mut scratch = [0i32; 32];
        cdef_filter_cols4_neon(
            token,
            inb,
            ioff,
            pri_strength,
            sec_strength,
            dir,
            pri_damping,
            sec_damping,
            coeff_shift,
            rows,
            subsampling_factor as i32,
            &mut scratch,
        );
        let mut i = 0i32;
        while i < rows {
            let drow = doff + i as usize * dstride;
            let srow = i as usize * 4;
            for j in 0..4usize {
                dst[drow + j] = scratch[srow + j] as u8;
            }
            i += subsampling_factor as i32;
        }
    }
}

/// AVX2 dst8 filter. Byte-identical to [`cdef_filter_block_core`]: each output
/// pixel is an independent 12-tap integer sum, so 8 columns of a row map to 8
/// SIMD lanes with NO cross-lane reduction. The `sum` is accumulated in i32 and
/// truncated to i16 once at the end (`(x<<16)>>16`); since two's-complement add
/// is associative mod 2^16, that equals the scalar's per-tap `wrapping_add::<i16>`
/// exactly (products are small, no i32 overflow across 12 taps). Only the
/// `cols == 8` shapes (BLOCK_8X8 / BLOCK_8X4) use the vector path; the rare 4-wide
/// chroma shapes fall back to the scalar core.
#[cfg(target_arch = "x86_64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn cdef_filter_block_impl_v3(
    token: Desktop64,
    dst: &mut [u8],
    doff: usize,
    dstride: usize,
    inb: &[u16],
    ioff: usize,
    pri_strength: i32,
    sec_strength: i32,
    dir: i32,
    pri_damping: i32,
    sec_damping: i32,
    bsize: i32,
    coeff_shift: i32,
    subsampling_factor: usize,
) {
    let cols = if bsize == BLOCK_8X8 || bsize == BLOCK_8X4 {
        8
    } else {
        4
    };
    if cols != 8 {
        cdef_filter_block_core(
            dst,
            doff,
            dstride,
            inb,
            ioff,
            pri_strength,
            sec_strength,
            dir,
            pri_damping,
            sec_damping,
            bsize,
            coeff_shift,
            subsampling_factor,
        );
        return;
    }
    let rows = if bsize == BLOCK_8X8 || bsize == BLOCK_4X8 {
        8
    } else {
        4
    };
    let mut scratch = [0i32; 64];
    cdef_filter_cols8_v3(
        token,
        inb,
        ioff,
        pri_strength,
        sec_strength,
        dir,
        pri_damping,
        sec_damping,
        coeff_shift,
        rows,
        subsampling_factor as i32,
        &mut scratch,
    );
    let mut i = 0i32;
    while i < rows {
        let drow = doff + i as usize * dstride;
        let srow = i as usize * 8;
        for j in 0..8usize {
            dst[drow + j] = scratch[srow + j] as u8;
        }
        i += subsampling_factor as i32;
    }
}

/// Load 8 contiguous `u16` at `idx` and SIGN-extend to an `i32x8` — the C
/// reference reads the `uint16_t*` input into `int16_t` locals (`cdef.c:205-224`),
/// so values ≥ 0x8000 wrap negative. Sign-extension reproduces that exactly (for
/// real pixels ≤ 0x7f7f, incl. the `CDEF_VERY_LARGE` sentinel, it equals a
/// zero-extension, so the sentinel equality compare is unaffected).
#[cfg(target_arch = "x86_64")]
#[rite]
fn cdef_load8_u16_v3(_token: Desktop64, inb: &[u16], idx: usize) -> __m256i {
    let a: &[u16; 8] = inb[idx..idx + 8].try_into().unwrap();
    _mm256_cvtepi16_epi32(_mm_loadu_si128(a))
}

/// SIMD [`constrain`] over an `i32x8` of diffs. `threshold`/`shift` are scalar
/// (broadcast); when `active` is false the tap is disabled (threshold 0 ⇒ 0),
/// matching the scalar early-return.
#[cfg(target_arch = "x86_64")]
#[rite]
fn cdef_constrain8_v3(
    _token: Desktop64,
    diff: __m256i,
    thr: __m256i,
    shift_c: __m128i,
    active: bool,
) -> __m256i {
    if !active {
        return _mm256_setzero_si256();
    }
    let adiff = _mm256_abs_epi32(diff);
    let shifted = _mm256_srl_epi32(adiff, shift_c);
    let capped = _mm256_max_epi32(_mm256_sub_epi32(thr, shifted), _mm256_setzero_si256());
    let m = _mm256_min_epi32(adiff, capped);
    // sign(diff) * m: _mm256_sign_epi32 negates m where diff<0, zeros where diff==0
    // (m is already 0 there), else keeps m — exactly `sign * min(|diff|, cap)`.
    _mm256_sign_epi32(m, diff)
}

/// Shared 8-wide CDEF filter core (dst8 and dst16 arms both call this). Writes
/// the filtered pixels of the `cols == 8` block into `out` row-major (`rows`
/// rows × 8), touching only the rows the `subsampling_factor` visits. Callers
/// clamp-cast each `i32` to the output pixel type.
#[cfg(target_arch = "x86_64")]
#[rite]
#[allow(clippy::too_many_arguments)]
pub(crate) fn cdef_filter_cols8_v3(
    token: Desktop64,
    inb: &[u16],
    ioff: usize,
    pri_strength: i32,
    sec_strength: i32,
    dir: i32,
    pri_damping: i32,
    sec_damping: i32,
    coeff_shift: i32,
    rows: i32,
    sub: i32,
    out: &mut [i32; 64],
) {
    let s = CDEF_BSTRIDE as i32;
    let pri_taps = CDEF_PRI_TAPS[((pri_strength >> coeff_shift) & 1) as usize];
    let sec_taps = CDEF_SEC_TAPS[((pri_strength >> coeff_shift) & 1) as usize];

    let pri_active = pri_strength != 0;
    let sec_active = sec_strength != 0;
    let pri_shift = if pri_active {
        (pri_damping - get_msb(pri_strength as u32)).max(0)
    } else {
        0
    };
    let sec_shift = if sec_active {
        (sec_damping - get_msb(sec_strength as u32)).max(0)
    } else {
        0
    };
    let pri_shift_c = _mm_cvtsi32_si128(pri_shift);
    let sec_shift_c = _mm_cvtsi32_si128(sec_shift);
    let pri_thr = _mm256_set1_epi32(pri_strength);
    let sec_thr = _mm256_set1_epi32(sec_strength);
    let sentinel = _mm256_set1_epi32(CDEF_VERY_LARGE as i32);
    let eight = _mm256_set1_epi32(8);

    // 4 primary taps (±dir·k) and 8 secondary taps (±(dir±2)·k); offsets and
    // coefficients are block-constant (independent of the row), so precompute.
    let p_off = [
        cdef_direction(dir, 0),
        -cdef_direction(dir, 0),
        cdef_direction(dir, 1),
        -cdef_direction(dir, 1),
    ];
    let p_cof = [pri_taps[0], pri_taps[0], pri_taps[1], pri_taps[1]];
    let s_off = [
        cdef_direction(dir + 2, 0),
        -cdef_direction(dir + 2, 0),
        cdef_direction(dir - 2, 0),
        -cdef_direction(dir - 2, 0),
        cdef_direction(dir + 2, 1),
        -cdef_direction(dir + 2, 1),
        cdef_direction(dir - 2, 1),
        -cdef_direction(dir - 2, 1),
    ];
    let s_cof = [
        sec_taps[0],
        sec_taps[0],
        sec_taps[0],
        sec_taps[0],
        sec_taps[1],
        sec_taps[1],
        sec_taps[1],
        sec_taps[1],
    ];

    let mut i = 0i32;
    while i < rows {
        let base = (ioff as i32 + i * s) as usize;
        let x = cdef_load8_u16_v3(token, inb, base);
        let mut sum = _mm256_setzero_si256();
        let mut mx = x;
        let mut mn = x;
        for t in 0..4usize {
            let idx = (base as i32 + p_off[t]) as usize;
            let tap = cdef_load8_u16_v3(token, inb, idx);
            let diff = _mm256_sub_epi32(tap, x);
            let c = cdef_constrain8_v3(token, diff, pri_thr, pri_shift_c, pri_active);
            sum = _mm256_add_epi32(sum, _mm256_mullo_epi32(c, _mm256_set1_epi32(p_cof[t])));
            let is_sent = _mm256_cmpeq_epi32(tap, sentinel);
            mx = _mm256_blendv_epi8(_mm256_max_epi32(mx, tap), mx, is_sent);
            mn = _mm256_min_epi32(mn, tap);
        }
        for t in 0..8usize {
            let idx = (base as i32 + s_off[t]) as usize;
            let tap = cdef_load8_u16_v3(token, inb, idx);
            let diff = _mm256_sub_epi32(tap, x);
            let c = cdef_constrain8_v3(token, diff, sec_thr, sec_shift_c, sec_active);
            sum = _mm256_add_epi32(sum, _mm256_mullo_epi32(c, _mm256_set1_epi32(s_cof[t])));
            let is_sent = _mm256_cmpeq_epi32(tap, sentinel);
            mx = _mm256_blendv_epi8(_mm256_max_epi32(mx, tap), mx, is_sent);
            mn = _mm256_min_epi32(mn, tap);
        }
        // `sum as i16 as i32` (sign-extend low 16 bits) reproduces the scalar's
        // wrapping i16 accumulation; then `x + ((8 + sum - (sum<0)) >> 4)`.
        let sw = _mm256_srai_epi32::<16>(_mm256_slli_epi32::<16>(sum));
        let neg = _mm256_srli_epi32::<31>(sw);
        let adj = _mm256_srai_epi32::<4>(_mm256_sub_epi32(_mm256_add_epi32(eight, sw), neg));
        let val = _mm256_add_epi32(x, adj);
        let y = _mm256_min_epi32(_mm256_max_epi32(val, mn), mx);
        let row_arr: &mut [i32; 8] = (&mut out[i as usize * 8..i as usize * 8 + 8])
            .try_into()
            .unwrap();
        _mm256_storeu_si256(row_arr, y);
        i += sub;
    }
}

/// Scalar reference body for [`cdef_filter_block`] — `svt_cdef_filter_block_c`
/// (dst8 arm). The AVX2 path ([`cdef_filter_block_impl_v3`]) is proven
/// byte-identical to this against real C in `tests/c_parity_cdef.rs`.
#[allow(clippy::too_many_arguments)]
fn cdef_filter_block_core(
    dst: &mut [u8],
    doff: usize,
    dstride: usize,
    inb: &[u16],
    ioff: usize,
    pri_strength: i32,
    sec_strength: i32,
    dir: i32,
    pri_damping: i32,
    sec_damping: i32,
    bsize: i32,
    coeff_shift: i32,
    subsampling_factor: usize,
) {
    let s = CDEF_BSTRIDE as i32;
    let pri_taps = CDEF_PRI_TAPS[((pri_strength >> coeff_shift) & 1) as usize];
    let sec_taps = CDEF_SEC_TAPS[((pri_strength >> coeff_shift) & 1) as usize];
    let rows = if bsize == BLOCK_8X8 || bsize == BLOCK_4X8 {
        8
    } else {
        4
    };
    let cols = if bsize == BLOCK_8X8 || bsize == BLOCK_8X4 {
        8
    } else {
        4
    };

    let at = |i: i32, j: i32, off: i32| -> u16 { inb[(ioff as i32 + i * s + j + off) as usize] };

    let mut i = 0i32;
    while i < rows {
        for j in 0..cols {
            let mut sum = 0i16;
            let x = at(i, j, 0) as i16;
            let mut max = x as i32;
            let mut min = x as i32;
            for k in 0..2usize {
                let p0 = at(i, j, cdef_direction(dir, k)) as i16;
                let p1 = at(i, j, -cdef_direction(dir, k)) as i16;
                sum = sum.wrapping_add(
                    (pri_taps[k] * constrain(p0 as i32 - x as i32, pri_strength, pri_damping))
                        as i16,
                );
                sum = sum.wrapping_add(
                    (pri_taps[k] * constrain(p1 as i32 - x as i32, pri_strength, pri_damping))
                        as i16,
                );
                if p0 as u16 != CDEF_VERY_LARGE {
                    max = (p0 as i32).max(max);
                }
                if p1 as u16 != CDEF_VERY_LARGE {
                    max = (p1 as i32).max(max);
                }
                min = (p0 as i32).min(min);
                min = (p1 as i32).min(min);
                let s0 = at(i, j, cdef_direction(dir + 2, k)) as i16;
                let s1 = at(i, j, -cdef_direction(dir + 2, k)) as i16;
                let s2 = at(i, j, cdef_direction(dir - 2, k)) as i16;
                let s3 = at(i, j, -cdef_direction(dir - 2, k)) as i16;
                if s0 as u16 != CDEF_VERY_LARGE {
                    max = (s0 as i32).max(max);
                }
                if s1 as u16 != CDEF_VERY_LARGE {
                    max = (s1 as i32).max(max);
                }
                if s2 as u16 != CDEF_VERY_LARGE {
                    max = (s2 as i32).max(max);
                }
                if s3 as u16 != CDEF_VERY_LARGE {
                    max = (s3 as i32).max(max);
                }
                min = (s0 as i32).min(min);
                min = (s1 as i32).min(min);
                min = (s2 as i32).min(min);
                min = (s3 as i32).min(min);
                sum = sum.wrapping_add(
                    (sec_taps[k] * constrain(s0 as i32 - x as i32, sec_strength, sec_damping))
                        as i16,
                );
                sum = sum.wrapping_add(
                    (sec_taps[k] * constrain(s1 as i32 - x as i32, sec_strength, sec_damping))
                        as i16,
                );
                sum = sum.wrapping_add(
                    (sec_taps[k] * constrain(s2 as i32 - x as i32, sec_strength, sec_damping))
                        as i16,
                );
                sum = sum.wrapping_add(
                    (sec_taps[k] * constrain(s3 as i32 - x as i32, sec_strength, sec_damping))
                        as i16,
                );
            }
            let y = clamp_i32(
                x as i32 + ((8 + sum as i32 - i32::from(sum < 0)) >> 4),
                min,
                max,
            );
            dst[doff + i as usize * dstride + j as usize] = y as u8;
        }
        i += subsampling_factor as i32;
    }
}

/// `svt_cdef_filter_block_8bit_c` (cdef.c:257): native 8-bit interior filter
/// — identical math to [`cdef_filter_block`] but reads an 8-bit padded
/// buffer with NO sentinel handling (every tap participates in min/max), so
/// it is only valid for blocks whose full tap halo is real pixels.
#[allow(clippy::too_many_arguments)]
pub fn cdef_filter_block_8bit(
    dst: &mut [u8],
    doff: usize,
    dstride: usize,
    inb: &[u8],
    ioff: usize,
    pri_strength: i32,
    sec_strength: i32,
    dir: i32,
    damping: i32,
    bsize: i32,
    coeff_shift: i32,
    subsampling_factor: usize,
) {
    let s = CDEF_BSTRIDE as i32;
    let pri_taps = CDEF_PRI_TAPS[((pri_strength >> coeff_shift) & 1) as usize];
    let sec_taps = CDEF_SEC_TAPS[((pri_strength >> coeff_shift) & 1) as usize];
    let rows = if bsize == BLOCK_8X8 || bsize == BLOCK_4X8 {
        8
    } else {
        4
    };
    let cols = if bsize == BLOCK_8X8 || bsize == BLOCK_8X4 {
        8
    } else {
        4
    };
    let sub = if bsize == BLOCK_4X4 {
        1
    } else {
        subsampling_factor
    };

    let at =
        |i: i32, j: i32, off: i32| -> i16 { inb[(ioff as i32 + i * s + j + off) as usize] as i16 };

    let mut i = 0i32;
    while i < rows {
        for j in 0..cols {
            let x = at(i, j, 0);
            let mut sum = 0i16;
            let mut max = x as i32;
            let mut min = x as i32;
            for k in 0..2usize {
                let p0 = at(i, j, cdef_direction(dir, k));
                let p1 = at(i, j, -cdef_direction(dir, k));
                sum = sum.wrapping_add(
                    (pri_taps[k] * constrain(p0 as i32 - x as i32, pri_strength, damping)) as i16,
                );
                sum = sum.wrapping_add(
                    (pri_taps[k] * constrain(p1 as i32 - x as i32, pri_strength, damping)) as i16,
                );
                max = (p0 as i32).max(max);
                max = (p1 as i32).max(max);
                min = (p0 as i32).min(min);
                min = (p1 as i32).min(min);
                let s0 = at(i, j, cdef_direction(dir + 2, k));
                let s1 = at(i, j, -cdef_direction(dir + 2, k));
                let s2 = at(i, j, cdef_direction(dir - 2, k));
                let s3 = at(i, j, -cdef_direction(dir - 2, k));
                sum = sum.wrapping_add(
                    (sec_taps[k] * constrain(s0 as i32 - x as i32, sec_strength, damping)) as i16,
                );
                sum = sum.wrapping_add(
                    (sec_taps[k] * constrain(s1 as i32 - x as i32, sec_strength, damping)) as i16,
                );
                sum = sum.wrapping_add(
                    (sec_taps[k] * constrain(s2 as i32 - x as i32, sec_strength, damping)) as i16,
                );
                sum = sum.wrapping_add(
                    (sec_taps[k] * constrain(s3 as i32 - x as i32, sec_strength, damping)) as i16,
                );
                max = (s0 as i32).max(max);
                max = (s1 as i32).max(max);
                max = (s2 as i32).max(max);
                max = (s3 as i32).max(max);
                min = (s0 as i32).min(min);
                min = (s1 as i32).min(min);
                min = (s2 as i32).min(min);
                min = (s3 as i32).min(min);
            }
            let y = clamp_i32(
                x as i32 + ((8 + sum as i32 - i32::from(sum < 0)) >> 4),
                min,
                max,
            );
            dst[doff + i as usize * dstride + j as usize] = y as u8;
        }
        i += sub as i32;
    }
}

// ---------------------------------------------------------------------------
// Search-side distortion (enc_cdef.c) — shared by the bd8 and bd10 searches.
// ---------------------------------------------------------------------------

/// The (by, bx) block dims and packed-block stride shift for a CDEF
/// `plane_bsize` — C's four-way switch in `svt_aom_compute_cdef_dist_*_c`.
/// Returns `(block_w, block_h)`; the packed offset for block `bi` is
/// `bi << (log2(block_w) + log2(block_h))`, and the plane offset is
/// `(by << log2(block_h)) * stride + (bx << log2(block_w))`.
#[inline]
fn cdef_dist_block_dims(bsize: i32) -> (usize, usize) {
    match bsize {
        BLOCK_8X8 => (8, 8),
        BLOCK_4X8 => (4, 8),
        BLOCK_8X4 => (8, 4),
        _ => (4, 4),
    }
}

/// C `svt_aom_compute_cdef_dist_16bit_c` (enc_cdef.c:77) — the bd10/bd12
/// search's per-filter-block distortion.
///
/// `plane` is the SOURCE picture (C's `dst` parameter — the naming is
/// inverted at the only call site, cdef_process.c:541-551), `plane_off` the
/// filter block's top-left, `pstride` the picture stride. `packed` is C's
/// `src`: `tmp_dst`, the filtered blocks packed back-to-back.
#[allow(clippy::too_many_arguments)]
pub fn compute_cdef_dist_16bit(
    plane: &[u16],
    plane_off: usize,
    pstride: usize,
    packed: &[u16],
    dlist: &[(u8, u8)],
    bsize: i32,
    coeff_shift: i32,
    subsampling_factor: usize,
) -> u64 {
    let (bw, bh) = cdef_dist_block_dims(bsize);
    let (lw, lh) = (bw.trailing_zeros() as usize, bh.trailing_zeros() as usize);
    let mut sum = 0u64;
    for (bi, &(by, bx)) in dlist.iter().enumerate() {
        let poff = plane_off + ((by as usize) << lh) * pstride + ((bx as usize) << lw);
        let packed_off = bi << (lw + lh);
        let mut i = 0usize;
        while i < bh {
            for j in 0..bw {
                let e =
                    plane[poff + i * pstride + j] as i32 - packed[packed_off + i * bw + j] as i32;
                sum += (e * e) as u64;
            }
            i += subsampling_factor;
        }
    }
    sum >> (2 * coeff_shift)
}

/// C `svt_aom_compute_cdef_dist_8bit_c` (enc_cdef.c:114). Same parameter
/// roles as [`compute_cdef_dist_16bit`].
#[allow(clippy::too_many_arguments)]
pub fn compute_cdef_dist_8bit(
    plane: &[u8],
    plane_off: usize,
    pstride: usize,
    packed: &[u8],
    dlist: &[(u8, u8)],
    bsize: i32,
    coeff_shift: i32,
    subsampling_factor: usize,
) -> u64 {
    let (bw, bh) = cdef_dist_block_dims(bsize);
    let (lw, lh) = (bw.trailing_zeros() as usize, bh.trailing_zeros() as usize);
    let mut sum = 0u64;
    for (bi, &(by, bx)) in dlist.iter().enumerate() {
        let poff = plane_off + ((by as usize) << lh) * pstride + ((bx as usize) << lw);
        let packed_off = bi << (lw + lh);
        let mut i = 0usize;
        while i < bh {
            for j in 0..bw {
                let e =
                    plane[poff + i * pstride + j] as i32 - packed[packed_off + i * bw + j] as i32;
                sum += (e * e) as u64;
            }
            i += subsampling_factor;
        }
    }
    sum >> (2 * coeff_shift)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spec 7.15.3 Cdef_Directions cross-check: the padded table's live rows
    /// (index 2..10) decoded back to (dy, dx) must equal the spec table.
    #[test]
    fn direction_table_matches_spec() {
        const SPEC: [[[i32; 2]; 2]; 8] = [
            [[-1, 1], [-2, 2]],
            [[0, 1], [-1, 2]],
            [[0, 1], [0, 2]],
            [[0, 1], [1, 2]],
            [[1, 1], [2, 2]],
            [[1, 0], [2, 1]],
            [[1, 0], [2, 0]],
            [[1, 0], [2, -1]],
        ];
        let s = CDEF_BSTRIDE as i32;
        for dir in 0..8 {
            for k in 0..2 {
                let off = cdef_direction(dir, k);
                // decode: dy = round-to-nearest row (offsets have |dx| <= 2)
                let dy = if off >= 0 {
                    (off + s / 2) / s
                } else {
                    -((-off + s / 2) / s)
                };
                let dx = off - dy * s;
                assert_eq!([dy, dx], SPEC[dir as usize][k], "dir {dir} k {k}");
            }
        }
        // padded ends replicate dir 6,7 and 0,1
        assert_eq!(cdef_direction(-2, 0), cdef_direction(6, 0));
        assert_eq!(cdef_direction(-1, 1), cdef_direction(7, 1));
        assert_eq!(cdef_direction(8, 0), cdef_direction(0, 0));
        assert_eq!(cdef_direction(9, 1), cdef_direction(1, 1));
    }

    /// A flat block has no direction energy: var must be 0 and filtering at
    /// any strength must be the identity.
    #[test]
    fn flat_block_identity() {
        let mut inb = alloc::vec![CDEF_VERY_LARGE; CDEF_INBUF_SIZE];
        let ioff = CDEF_VBORDER * CDEF_BSTRIDE + CDEF_HBORDER;
        for r in 0..8 {
            for c in 0..8 {
                inb[ioff + r * CDEF_BSTRIDE + c] = 77;
            }
        }
        let (_dir, var) = cdef_find_dir(&inb[ioff..], CDEF_BSTRIDE, 0);
        assert_eq!(var, 0);
        let mut dst = [0u8; 64];
        cdef_filter_block(&mut dst, 0, 8, &inb, ioff, 15, 4, 3, 6, 6, BLOCK_8X8, 0, 1);
        assert!(dst.iter().all(|&v| v == 77));
    }

    /// constrain() reproduces the C damping shape at hand-checked points.
    #[test]
    fn constrain_c_values() {
        // threshold 0 -> 0 regardless
        assert_eq!(constrain(1000, 0, 6), 0);
        // shift = max(0, 4 - msb(4)=2) = 2: c(5,4,4) = min(5, 4 - (5>>2)) = 3
        assert_eq!(constrain(5, 4, 4), 3);
        assert_eq!(constrain(-5, 4, 4), -3);
        // sentinel-sized diff is fully damped to 0
        assert_eq!(constrain(0x7f7f - 128, 15, 6), 0);
        assert_eq!(constrain(0x4000 - 128, 15, 6), 0);
        // large threshold, small diff: passes through
        assert_eq!(constrain(2, 15, 3), 2);
    }

    /// adjust_strength C anchor points.
    #[test]
    fn adjust_strength_c_values() {
        assert_eq!(adjust_strength(12, 0), 0);
        // var=63: var>>6 = 0 -> i=0 -> (12*4+8)>>4 = 3
        assert_eq!(adjust_strength(12, 63), 3);
        // var=1<<18: i = min(msb(1<<12)=12, 12) -> (12*16+8)>>4 = 12
        assert_eq!(adjust_strength(12, 1 << 18), 12);
    }
}
