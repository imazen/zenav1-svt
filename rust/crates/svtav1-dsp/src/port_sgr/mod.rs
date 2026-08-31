//! Self-guided (SGR) loop restoration — a faithful port of the SGR half of
//! `Codec/restoration.c`.
//!
//! # Why this exists — the "SGR is never searched" claim is ALL-INTRA-ONLY
//!
//! `rust/CLAUDE.md` envelope guard 5, `docs/C-TEST-PORTING-AUDIT.md:85-87`,
//! `docs/SUSPECTED-C-BUGS.md:134-136` and `svtav1-encoder/src/restoration.rs`'s
//! own module doc all say sgrproj is N/A because C enables it only at
//! `ENC_MR`, which the port cannot express (`SpeedConfig::preset` is a `u8`,
//! `ENC_MR` is -1).
//!
//! That is TRUE of `svt_aom_get_sg_filter_level_allintra`
//! (`enc_mode_config.c:1431` — 1 at `<= ENC_MR`, else 0). It is NOT true of
//! the path the inter campaign runs. The selector is `pd_process.c:4937`:
//!
//! ```text
//! allintra ? _allintra : rtc_tune ? _rtc : _default
//! ```
//!
//! and `scs->allintra = (intra_period_length == 0 || avif)`
//! (`enc_handle.c:4406/4704`). With `SVT_AVIF=0` the capture clears `avif` and
//! leaves `intra_period_length` at its default, which resolves to a real GOP
//! length, so `allintra == false` and the selector lands on
//! `svt_aom_get_sg_filter_level_default` (`enc_mode_config.c:1402`), which
//! returns 3 for `enc_mode <= ENC_M3`. Level 3 is
//! `SgFilterCtrls { enabled: 1, use_chroma: 1, ep 0..16 step 8 on lane 0,
//! ep 4..5 on lane 1, refine[0]: 1 }`. With Wiener also on,
//! `rest_finish_search` sets `force_restore_type_d = RESTORE_TYPES`
//! (`restoration_pick.c:1566`) and the switchable decision runs.
//!
//! So **in video mode at presets 0..3 both `RESTORE_SGRPROJ` and
//! `RESTORE_SWITCHABLE` are emittable**, and a port without this chain cannot
//! match C at the loop-restoration layer. None of those blocks is under
//! `#if TUNE_*` or `SVT_HDR_MODE`. At presets >= 4 `sg` is 0 and the existing
//! Wiener-only path stays faithful.
//!
//! # Evidence
//!
//! Tier 1 — `tests/c_parity_sgr.rs` drives the real exported
//! `svt_av1_selfguided_restoration_c`, `svt_apply_selfguided_restoration_c`
//! and `svt_decode_xq`. The `static` helpers (`boxsum`, `boxsum1`, `boxsum2`,
//! `selfguided_restoration_internal`,
//! `selfguided_restoration_fast_internal`) have no exported symbol and are
//! covered TRANSITIVELY through those two wrappers, which is exactly what
//! drives them in C.

pub mod tables;

use alloc::vec;
use alloc::vec::Vec;

pub use tables::{ONE_BY_X, SGR_PARAMS, X_BY_XPLUS1};

// --------------------------------------------------------------------------
// Constants (restoration.h)
// --------------------------------------------------------------------------

/// `SGRPROJ_BORDER_VERT` (restoration.h:41).
pub const SGRPROJ_BORDER_VERT: i32 = 3;
/// `SGRPROJ_BORDER_HORZ` (restoration.h:42).
pub const SGRPROJ_BORDER_HORZ: i32 = 3;
/// `RESTORATION_PROC_UNIT_SIZE` (restoration.h:37).
pub const RESTORATION_PROC_UNIT_SIZE: i32 = 64;
/// `RESTORATION_BORDER_VERT` / `_HORZ` — both resolve to `SGRPROJ_BORDER_*`
/// because 3 >= WIENER_BORDER_VERT (2) and 3 >= WIENER_BORDER_HORZ (3).
pub const RESTORATION_BORDER_VERT: i32 = 3;
pub const RESTORATION_BORDER_HORZ: i32 = 3;
/// `RESTORATION_PADDING` (restoration.h:73).
pub const RESTORATION_PADDING: i32 = 20;
/// `RESTORATION_PROC_UNIT_PELS` (restoration.h:74).
pub const RESTORATION_PROC_UNIT_PELS: usize =
    ((RESTORATION_PROC_UNIT_SIZE + RESTORATION_BORDER_HORZ * 2 + RESTORATION_PADDING)
        * (RESTORATION_PROC_UNIT_SIZE + RESTORATION_BORDER_VERT * 2 + RESTORATION_PADDING))
        as usize;

/// `SGRPROJ_PARAMS_BITS` (restoration.h:91).
pub const SGRPROJ_PARAMS_BITS: i32 = 4;
/// `SGRPROJ_PARAMS` — the number of signalled `ep` presets.
pub const SGRPROJ_PARAMS: usize = 1 << SGRPROJ_PARAMS_BITS;
/// `SGRPROJ_PRJ_BITS` (restoration.h:94).
pub const SGRPROJ_PRJ_BITS: i32 = 7;
/// `SGRPROJ_RST_BITS` (restoration.h:96).
pub const SGRPROJ_RST_BITS: i32 = 4;
/// `SGRPROJ_SGR_BITS` (restoration.h:98).
pub const SGRPROJ_SGR_BITS: i32 = 8;
/// `SGRPROJ_SGR` (restoration.h:99).
pub const SGRPROJ_SGR: i32 = 1 << SGRPROJ_SGR_BITS;
/// `SGRPROJ_MTABLE_BITS` (restoration.h:112).
pub const SGRPROJ_MTABLE_BITS: i32 = 20;
/// `SGRPROJ_RECIP_BITS` (restoration.h:113).
pub const SGRPROJ_RECIP_BITS: i32 = 12;

/// `SGRPROJ_PRJ_MIN0` / `MAX0` / `MIN1` / `MAX1` (restoration.h:101-104) —
/// the signalled range of each `xqd` component.
pub const SGRPROJ_PRJ_MIN0: i32 = -(1 << SGRPROJ_PRJ_BITS) * 3 / 4;
pub const SGRPROJ_PRJ_MAX0: i32 = SGRPROJ_PRJ_MIN0 + (1 << SGRPROJ_PRJ_BITS) - 1;
pub const SGRPROJ_PRJ_MIN1: i32 = -(1 << SGRPROJ_PRJ_BITS) / 4;
pub const SGRPROJ_PRJ_MAX1: i32 = SGRPROJ_PRJ_MIN1 + (1 << SGRPROJ_PRJ_BITS) - 1;
/// `SGRPROJ_PRJ_SUBEXP_K` (restoration.h:106).
pub const SGRPROJ_PRJ_SUBEXP_K: i32 = 4;
/// `SGRPROJ_BITS` (restoration.h:108).
pub const SGRPROJ_BITS: i32 = SGRPROJ_PRJ_BITS * 2 + SGRPROJ_PARAMS_BITS;

/// `RESTORATION_UNITSIZE_MAX` (restoration.h:85).
pub const RESTORATION_UNITSIZE_MAX: i32 = 256;
/// `RESTORATION_UNIT_OFFSET` (restoration.h:39).
pub const RESTORATION_UNIT_OFFSET: i32 = 8;
/// `RESTORATION_UNITPELS_HORZ_MAX` (restoration.h:86).
pub const RESTORATION_UNITPELS_HORZ_MAX: i32 =
    RESTORATION_UNITSIZE_MAX * 3 / 2 + 2 * RESTORATION_BORDER_HORZ + 16;
/// `RESTORATION_UNITPELS_VERT_MAX` (restoration.h:87).
pub const RESTORATION_UNITPELS_VERT_MAX: i32 =
    RESTORATION_UNITSIZE_MAX * 3 / 2 + 2 * RESTORATION_BORDER_VERT + RESTORATION_UNIT_OFFSET;
/// `RESTORATION_UNITPELS_MAX` (restoration.h:88).
pub const RESTORATION_UNITPELS_MAX: usize =
    (RESTORATION_UNITPELS_HORZ_MAX * RESTORATION_UNITPELS_VERT_MAX) as usize;

/// `MAX_RADIUS` (restoration.h:110).
pub const MAX_RADIUS: i32 = 2;

/// `SgrParamsType` (restoration.h) — the per-`ep` radii and noise parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SgrParamsType {
    /// Radii for the two filters; `0` means "skip this filter".
    pub r: [i32; 2],
    /// Noise parameter for each filter; `-1` where the filter is skipped.
    pub s: [i32; 2],
}

/// `ROUND_POWER_OF_TWO(value, n)` on `i32` (definitions.h:478).
#[inline]
const fn round_power_of_two(value: i32, n: i32) -> i32 {
    (value + ((1 << n) >> 1)) >> n
}

/// The unsigned form. C applies `ROUND_POWER_OF_TWO` to `uint32_t` operands
/// inside the A/B loop (`a`, `b`, `p * s`, and the `B[k]` product), where the
/// shift is a LOGICAL one and the addition wraps modulo 2^32. Doing that
/// arithmetic in `i32` would differ on the values C's own comments say can
/// reach 2^32, so the port keeps the unsigned type.
#[inline]
const fn round_power_of_two_u32(value: u32, n: i32) -> u32 {
    value.wrapping_add((1u32 << n) >> 1) >> n
}

/// `clip_pixel_highbd(v, bd)`.
#[inline]
const fn clip_pixel_highbd(v: i32, bd: i32) -> u16 {
    let max = (1i32 << bd) - 1;
    if v < 0 {
        0
    } else if v > max {
        max as u16
    } else {
        v as u16
    }
}

// --------------------------------------------------------------------------
// Box sums (restoration.c:430 / :497 / :586)
// --------------------------------------------------------------------------

/// Port of `boxsum1` (restoration.c:430) — windowed sums (`sqr == false`) or
/// sums of squares (`sqr == true`) over a 3x3 window.
///
/// The C loops are written with the induction variable ESCAPING the loop
/// (`for (i = 1; i < height - 2; ++i) {...}` then `dst[i * dst_stride + j]`
/// using the post-loop `i`), which is what fills the last two rows/columns.
/// The port keeps that shape literally rather than "cleaning it up", because
/// the post-loop value is `max(1, height - 2)` and a rewritten loop would
/// disagree for small inputs.
pub fn boxsum1(
    src: &[i32],
    src_origin: usize,
    width: i32,
    height: i32,
    src_stride: usize,
    sqr: bool,
    dst: &mut [i32],
    dst_origin: usize,
    dst_stride: usize,
) {
    debug_assert!(width > 2 * SGRPROJ_BORDER_HORZ);
    debug_assert!(height > 2 * SGRPROJ_BORDER_VERT);
    let s = |i: i32, j: i32| -> i32 { src[src_origin + (i as usize) * src_stride + j as usize] };

    // Vertical sum over 3-pixel regions, src -> dst.
    for j in 0..width {
        let sq = |v: i32| if sqr { v * v } else { v };
        let mut a = sq(s(0, j));
        let mut b = sq(s(1, j));
        let mut c = sq(s(2, j));

        dst[dst_origin + j as usize] = a + b;
        let mut i = 1i32;
        while i < height - 2 {
            dst[dst_origin + (i as usize) * dst_stride + j as usize] = a + b + c;
            a = b;
            b = c;
            c = sq(s(i + 2, j));
            i += 1;
        }
        dst[dst_origin + (i as usize) * dst_stride + j as usize] = a + b + c;
        dst[dst_origin + ((i + 1) as usize) * dst_stride + j as usize] = b + c;
    }

    // Horizontal sum over 3-pixel regions of dst.
    for i in 0..height {
        let row = dst_origin + (i as usize) * dst_stride;
        let mut a = dst[row];
        let mut b = dst[row + 1];
        let mut c = dst[row + 2];

        dst[row] = a + b;
        let mut j = 1i32;
        while j < width - 2 {
            dst[row + j as usize] = a + b + c;
            a = b;
            b = c;
            c = dst[row + (j + 2) as usize];
            j += 1;
        }
        dst[row + j as usize] = a + b + c;
        dst[row + (j + 1) as usize] = b + c;
    }
}

/// Port of `boxsum2` (restoration.c:497) — the 5x5 window.
pub fn boxsum2(
    src: &[i32],
    src_origin: usize,
    width: i32,
    height: i32,
    src_stride: usize,
    sqr: bool,
    dst: &mut [i32],
    dst_origin: usize,
    dst_stride: usize,
) {
    debug_assert!(width > 2 * SGRPROJ_BORDER_HORZ);
    debug_assert!(height > 2 * SGRPROJ_BORDER_VERT);
    let s = |i: i32, j: i32| -> i32 { src[src_origin + (i as usize) * src_stride + j as usize] };

    for j in 0..width {
        let sq = |v: i32| if sqr { v * v } else { v };
        let mut a = sq(s(0, j));
        let mut b = sq(s(1, j));
        let mut c = sq(s(2, j));
        let mut d = sq(s(3, j));
        let mut e = sq(s(4, j));

        dst[dst_origin + j as usize] = a + b + c;
        dst[dst_origin + dst_stride + j as usize] = a + b + c + d;
        let mut i = 2i32;
        while i < height - 3 {
            dst[dst_origin + (i as usize) * dst_stride + j as usize] = a + b + c + d + e;
            a = b;
            b = c;
            c = d;
            d = e;
            e = sq(s(i + 3, j));
            i += 1;
        }
        dst[dst_origin + (i as usize) * dst_stride + j as usize] = a + b + c + d + e;
        dst[dst_origin + ((i + 1) as usize) * dst_stride + j as usize] = b + c + d + e;
        dst[dst_origin + ((i + 2) as usize) * dst_stride + j as usize] = c + d + e;
    }

    for i in 0..height {
        let row = dst_origin + (i as usize) * dst_stride;
        let mut a = dst[row];
        let mut b = dst[row + 1];
        let mut c = dst[row + 2];
        let mut d = dst[row + 3];
        let mut e = dst[row + 4];

        dst[row] = a + b + c;
        dst[row + 1] = a + b + c + d;
        let mut j = 2i32;
        while j < width - 3 {
            dst[row + j as usize] = a + b + c + d + e;
            a = b;
            b = c;
            c = d;
            d = e;
            e = dst[row + (j + 3) as usize];
            j += 1;
        }
        dst[row + j as usize] = a + b + c + d + e;
        dst[row + (j + 1) as usize] = b + c + d + e;
        dst[row + (j + 2) as usize] = c + d + e;
    }
}

/// Port of `boxsum` (restoration.c:586) — the radius dispatcher. C asserts on
/// any radius other than 1 or 2 (`MAX_RADIUS` is 2), so the port panics there
/// rather than silently returning wrong sums.
#[allow(clippy::too_many_arguments)]
pub fn boxsum(
    src: &[i32],
    src_origin: usize,
    width: i32,
    height: i32,
    src_stride: usize,
    r: i32,
    sqr: bool,
    dst: &mut [i32],
    dst_origin: usize,
    dst_stride: usize,
) {
    match r {
        1 => boxsum1(
            src, src_origin, width, height, src_stride, sqr, dst, dst_origin, dst_stride,
        ),
        2 => boxsum2(
            src, src_origin, width, height, src_stride, sqr, dst, dst_origin, dst_stride,
        ),
        _ => panic!("Invalid value of r in self-guided filter: {r}"),
    }
}

// --------------------------------------------------------------------------
// svt_decode_xq (restoration.c:597)
// --------------------------------------------------------------------------

/// Port of `svt_decode_xq` (restoration.c:597) — the normative derivation of
/// the two projection weights from the signalled `xqd` pair and the `ep`
/// preset's radii.
///
/// Note the asymmetry, which is C's and is normative: when `r[0] == 0` the
/// SECOND weight absorbs the whole unit gain (`(1 << PRJ_BITS) - xqd[1]`),
/// when `r[1] == 0` the second weight is simply 0 (the first is passed
/// through unchanged and does NOT absorb the unit gain).
#[inline]
pub fn decode_xq(xqd: &[i32; 2], params: &SgrParamsType) -> [i32; 2] {
    if params.r[0] == 0 {
        [0, (1 << SGRPROJ_PRJ_BITS) - xqd[1]]
    } else if params.r[1] == 0 {
        [xqd[0], 0]
    } else {
        let x0 = xqd[0];
        [x0, (1 << SGRPROJ_PRJ_BITS) - x0 - xqd[1]]
    }
}

// --------------------------------------------------------------------------
// The A/B computation shared by both internals
// --------------------------------------------------------------------------

/// The body of the A/B loop, identical in `selfguided_restoration_internal`
/// and `selfguided_restoration_fast_internal` — the two differ only in the
/// row STEP (1 vs 2) and in the cross-filter that follows.
#[inline]
#[allow(clippy::too_many_arguments)]
fn compute_ab_at(a: &mut [i32], b: &mut [i32], idx: usize, n: i32, s: i32, bit_depth: i32) {
    // a < 2^16 * n < 2^22 regardless of bit depth.
    let av = round_power_of_two_u32(a[idx] as u32, 2 * (bit_depth - 8));
    // b < 2^8 * n < 2^14 regardless of bit depth.
    let bv = round_power_of_two_u32(b[idx] as u32, bit_depth - 8);

    // Sometimes, at high bit depth, rounding can make a*n < b*b; C saturates
    // p to 0 there rather than wrapping.
    let an = av.wrapping_mul(n as u32);
    let bb = bv.wrapping_mul(bv);
    // C: `p = (a * n < b * b) ? 0 : a * n - b * b` — saturate, never wrap.
    let p = an.saturating_sub(bb);

    let z = round_power_of_two_u32(p.wrapping_mul(s as u32), SGRPROJ_MTABLE_BITS);

    // A[k] lands in [1, 256]. The `z == 0 -> 1` saturation is deliberate and
    // load-bearing (see X_BY_XPLUS1's doc comment): saturating on the other
    // side (A[k] <= 255) would be wrong, because that is the very-variable
    // case where the individual pixel value must be preserved.
    a[idx] = X_BY_XPLUS1[z.min(255) as usize];

    b[idx] = round_power_of_two_u32(
        (SGRPROJ_SGR - a[idx]) as u32 * (b[idx] as u32) * (ONE_BY_X[(n - 1) as usize] as u32),
        SGRPROJ_RECIP_BITS,
    ) as i32;
}

/// `buf_stride` for the A/B arrays (restoration.c:643). The `& ~3` plus 16 is
/// C's cache/SIMD alignment choice and it CHANGES THE INDEXING, so it is part
/// of the port, not an implementation detail to simplify away.
#[inline]
fn ab_buf_stride(width_ext: i32) -> usize {
    (((width_ext + 3) & !3) + 16) as usize
}

/// Port of `selfguided_restoration_fast_internal` (restoration.c:632) — the
/// `r == 2` fast path, which computes A/B on EVERY OTHER ROW and uses two
/// different 3x3 cross-filters (5-tap-ish on even rows, 4-tap on odd) to
/// interpolate the skipped ones.
#[allow(clippy::too_many_arguments)]
fn selfguided_restoration_fast_internal(
    dgd: &[i32],
    dgd_origin: usize,
    width: i32,
    height: i32,
    dgd_stride: usize,
    dst: &mut [i32],
    dst_stride: usize,
    bit_depth: i32,
    sgr_params_idx: usize,
    radius_idx: usize,
) {
    let params = &SGR_PARAMS[sgr_params_idx];
    let r = params.r[radius_idx];
    let width_ext = width + 2 * SGRPROJ_BORDER_HORZ;
    let height_ext = height + 2 * SGRPROJ_BORDER_VERT;
    let buf_stride = ab_buf_stride(width_ext);

    let mut a_buf = vec![0i32; RESTORATION_PROC_UNIT_PELS];
    let mut b_buf = vec![0i32; RESTORATION_PROC_UNIT_PELS];

    debug_assert!(r <= MAX_RADIUS);
    // C asserts `r <= SGRPROJ_BORDER_VERT - 1 && r <= SGRPROJ_BORDER_HORZ - 1`;
    // both borders are 3, so one check covers both here.
    debug_assert!(r < SGRPROJ_BORDER_VERT);

    // C passes `dgd - dgd_stride * BORDER_VERT - BORDER_HORZ`, i.e. the raw
    // base of the bordered buffer.
    let box_src =
        dgd_origin - dgd_stride * SGRPROJ_BORDER_VERT as usize - SGRPROJ_BORDER_HORZ as usize;
    boxsum(
        dgd, box_src, width_ext, height_ext, dgd_stride, r, false, &mut b_buf, 0, buf_stride,
    );
    boxsum(
        dgd, box_src, width_ext, height_ext, dgd_stride, r, true, &mut a_buf, 0, buf_stride,
    );
    // C advances the A/B pointers past the border; the port carries the same
    // shift as an index origin.
    let ab_off = SGRPROJ_BORDER_VERT as usize * buf_stride + SGRPROJ_BORDER_HORZ as usize;

    let n = (2 * r + 1) * (2 * r + 1);
    let s = params.s[radius_idx];
    // A one-pixel border of A/B is computed: for a 64x64 unit that is 66x66.
    let mut i = -1i32;
    while i < height + 1 {
        for j in -1..width + 1 {
            let k = i * buf_stride as i32 + j;
            let idx = (ab_off as i32 + k) as usize;
            compute_ab_at(&mut a_buf, &mut b_buf, idx, n, s, bit_depth);
        }
        i += 2;
    }

    debug_assert_eq!(r, 2);
    let bs = buf_stride as i32;
    for i in 0..height {
        for j in 0..width {
            let k = ab_off as i32 + i * bs + j;
            let l = dgd_origin as i32 + i * dgd_stride as i32 + j;
            let m = (i as usize) * dst_stride + j as usize;
            let (a, b, nb) = if i & 1 == 0 {
                // even row: A/B were computed on rows i-1 and i+1
                let ku = (k - bs) as usize;
                let kd = (k + bs) as usize;
                (
                    (a_buf[ku] + a_buf[kd]) * 6
                        + (a_buf[ku - 1] + a_buf[kd - 1] + a_buf[ku + 1] + a_buf[kd + 1]) * 5,
                    (b_buf[ku] + b_buf[kd]) * 6
                        + (b_buf[ku - 1] + b_buf[kd - 1] + b_buf[ku + 1] + b_buf[kd + 1]) * 5,
                    5,
                )
            } else {
                // odd row: A/B were computed on row i itself
                let kk = k as usize;
                (
                    a_buf[kk] * 6 + (a_buf[kk - 1] + a_buf[kk + 1]) * 5,
                    b_buf[kk] * 6 + (b_buf[kk - 1] + b_buf[kk + 1]) * 5,
                    4,
                )
            };
            let v = a * dgd[l as usize] + b;
            dst[m] = round_power_of_two(v, SGRPROJ_SGR_BITS + nb - SGRPROJ_RST_BITS);
        }
    }
}

/// Port of `selfguided_restoration_internal` (restoration.c:766) — the full
/// path: A/B on every row, then a 3x3 cross-filter with weights 4 (plus) and
/// 3 (diagonal).
#[allow(clippy::too_many_arguments)]
fn selfguided_restoration_internal(
    dgd: &[i32],
    dgd_origin: usize,
    width: i32,
    height: i32,
    dgd_stride: usize,
    dst: &mut [i32],
    dst_stride: usize,
    bit_depth: i32,
    sgr_params_idx: usize,
    radius_idx: usize,
) {
    let params = &SGR_PARAMS[sgr_params_idx];
    let r = params.r[radius_idx];
    let width_ext = width + 2 * SGRPROJ_BORDER_HORZ;
    let height_ext = height + 2 * SGRPROJ_BORDER_VERT;
    let buf_stride = ab_buf_stride(width_ext);

    let mut a_buf = vec![0i32; RESTORATION_PROC_UNIT_PELS];
    let mut b_buf = vec![0i32; RESTORATION_PROC_UNIT_PELS];

    debug_assert!(r <= MAX_RADIUS);
    // C asserts `r <= SGRPROJ_BORDER_VERT - 1 && r <= SGRPROJ_BORDER_HORZ - 1`;
    // both borders are 3, so one check covers both here.
    debug_assert!(r < SGRPROJ_BORDER_VERT);

    let box_src =
        dgd_origin - dgd_stride * SGRPROJ_BORDER_VERT as usize - SGRPROJ_BORDER_HORZ as usize;
    boxsum(
        dgd, box_src, width_ext, height_ext, dgd_stride, r, false, &mut b_buf, 0, buf_stride,
    );
    boxsum(
        dgd, box_src, width_ext, height_ext, dgd_stride, r, true, &mut a_buf, 0, buf_stride,
    );
    let ab_off = SGRPROJ_BORDER_VERT as usize * buf_stride + SGRPROJ_BORDER_HORZ as usize;

    let n = (2 * r + 1) * (2 * r + 1);
    let s = params.s[radius_idx];
    for i in -1..height + 1 {
        for j in -1..width + 1 {
            let k = i * buf_stride as i32 + j;
            let idx = (ab_off as i32 + k) as usize;
            compute_ab_at(&mut a_buf, &mut b_buf, idx, n, s, bit_depth);
        }
    }

    let bs = buf_stride as i32;
    for i in 0..height {
        for j in 0..width {
            let k = (ab_off as i32 + i * bs + j) as usize;
            let l = dgd_origin as i32 + i * dgd_stride as i32 + j;
            let m = (i as usize) * dst_stride + j as usize;
            let nb = 5;
            let ku = k - bs as usize;
            let kd = k + bs as usize;
            let a = (a_buf[k] + a_buf[k - 1] + a_buf[k + 1] + a_buf[ku] + a_buf[kd]) * 4
                + (a_buf[ku - 1] + a_buf[kd - 1] + a_buf[ku + 1] + a_buf[kd + 1]) * 3;
            let b = (b_buf[k] + b_buf[k - 1] + b_buf[k + 1] + b_buf[ku] + b_buf[kd]) * 4
                + (b_buf[ku - 1] + b_buf[kd - 1] + b_buf[ku + 1] + b_buf[kd + 1]) * 3;
            let v = a * dgd[l as usize] + b;
            dst[m] = round_power_of_two(v, SGRPROJ_SGR_BITS + nb - SGRPROJ_RST_BITS);
        }
    }
}

// --------------------------------------------------------------------------
// The exported wrappers (restoration.c:886 / :924)
// --------------------------------------------------------------------------

/// A source plane at either bit depth. `origin` is the buffer index of pixel
/// `(0, 0)`; the SGR kernels read `SGRPROJ_BORDER_*` pixels outside that in
/// every direction, so the caller must have extended the plane first
/// (`svt_extend_frame` does this in the real pipeline).
#[derive(Debug, Clone, Copy)]
pub enum SgrSrc<'a> {
    Lowbd(&'a [u8]),
    Highbd(&'a [u16]),
}

impl SgrSrc<'_> {
    #[inline]
    fn get(&self, idx: usize) -> i32 {
        match self {
            SgrSrc::Lowbd(p) => i32::from(p[idx]),
            SgrSrc::Highbd(p) => i32::from(p[idx]),
        }
    }
    #[inline]
    fn is_highbd(&self) -> bool {
        matches!(self, SgrSrc::Highbd(_))
    }
}

/// Port of `svt_av1_selfguided_restoration_c` (restoration.c:886).
///
/// Widens the bordered source into an `i32` scratch plane and runs whichever
/// of the two filters the `ep` preset enables, writing `flt0` (the r=2 fast
/// filter) and/or `flt1` (the r=1 full filter). A filter whose radius is 0 is
/// SKIPPED, and its `flt` buffer is left untouched — callers must treat it as
/// `u` (the unfiltered value), which is what `apply_selfguided_restoration`
/// does by gating on `params.r[i] > 0`.
#[allow(clippy::too_many_arguments)]
pub fn selfguided_restoration(
    dgd: SgrSrc<'_>,
    dgd_origin: usize,
    width: i32,
    height: i32,
    dgd_stride: usize,
    flt0: &mut [i32],
    flt1: &mut [i32],
    flt_stride: usize,
    sgr_params_idx: usize,
    bit_depth: i32,
) {
    let dgd32_stride = (width + 2 * SGRPROJ_BORDER_HORZ) as usize;
    let mut dgd32_ = vec![0i32; RESTORATION_PROC_UNIT_PELS];
    let dgd32_origin = dgd32_stride * SGRPROJ_BORDER_VERT as usize + SGRPROJ_BORDER_HORZ as usize;

    for i in -SGRPROJ_BORDER_VERT..height + SGRPROJ_BORDER_VERT {
        for j in -SGRPROJ_BORDER_HORZ..width + SGRPROJ_BORDER_HORZ {
            let d = dgd32_origin as i32 + i * dgd32_stride as i32 + j;
            let s = dgd_origin as i32 + i * dgd_stride as i32 + j;
            dgd32_[d as usize] = dgd.get(s as usize);
        }
    }

    let params = &SGR_PARAMS[sgr_params_idx];
    // Both radii zero would be equivalent to skipping SGR entirely; C asserts.
    debug_assert!(!(params.r[0] == 0 && params.r[1] == 0));

    if params.r[0] > 0 {
        selfguided_restoration_fast_internal(
            &dgd32_,
            dgd32_origin,
            width,
            height,
            dgd32_stride,
            flt0,
            flt_stride,
            bit_depth,
            sgr_params_idx,
            0,
        );
    }
    if params.r[1] > 0 {
        selfguided_restoration_internal(
            &dgd32_,
            dgd32_origin,
            width,
            height,
            dgd32_stride,
            flt1,
            flt_stride,
            bit_depth,
            sgr_params_idx,
            1,
        );
    }
}

/// A destination plane at either bit depth.
#[derive(Debug)]
pub enum SgrDst<'a> {
    Lowbd(&'a mut [u8]),
    Highbd(&'a mut [u16]),
}

impl SgrDst<'_> {
    #[inline]
    fn set(&mut self, idx: usize, v: u16) {
        match self {
            SgrDst::Lowbd(p) => p[idx] = v as u8,
            SgrDst::Highbd(p) => p[idx] = v,
        }
    }
}

/// Port of `svt_apply_selfguided_restoration_c` (restoration.c:924) — the
/// normative apply step the DECODER runs: derive `flt0`/`flt1`, derive `xq`
/// from `(ep, xqd)`, then project.
///
/// C takes a caller-owned `tmpbuf` of `SGRPROJ_TMPBUF_SIZE`; the port
/// allocates the two `flt` planes itself. That is a memory-ownership
/// difference only — every arithmetic step is C's.
#[allow(clippy::too_many_arguments)]
pub fn apply_selfguided_restoration(
    dat: SgrSrc<'_>,
    dat_origin: usize,
    width: i32,
    height: i32,
    stride: usize,
    eps: usize,
    xqd: &[i32; 2],
    dst: &mut SgrDst<'_>,
    dst_origin: usize,
    dst_stride: usize,
    bit_depth: i32,
) {
    debug_assert!((width * height) as usize <= RESTORATION_UNITPELS_MAX);
    let mut flt0: Vec<i32> = vec![0; RESTORATION_UNITPELS_MAX];
    let mut flt1: Vec<i32> = vec![0; RESTORATION_UNITPELS_MAX];

    selfguided_restoration(
        dat,
        dat_origin,
        width,
        height,
        stride,
        &mut flt0,
        &mut flt1,
        width as usize,
        eps,
        bit_depth,
    );
    let params = &SGR_PARAMS[eps];
    let xq = decode_xq(xqd, params);
    debug_assert_eq!(dat.is_highbd(), matches!(dst, SgrDst::Highbd(_)));

    for i in 0..height as usize {
        for j in 0..width as usize {
            let k = i * width as usize + j;
            let pre_u = dat.get(dat_origin + i * stride + j);
            let u = pre_u << SGRPROJ_RST_BITS;
            let mut v = u << SGRPROJ_PRJ_BITS;
            // A skipped filter's flt[k] is conceptually u, so its term is 0
            // and C simply does not add it.
            if params.r[0] > 0 {
                v += xq[0] * (flt0[k] - u);
            }
            if params.r[1] > 0 {
                v += xq[1] * (flt1[k] - u);
            }
            // C narrows through int16_t here BEFORE clipping — that truncation
            // is part of the normative result, not a C wart to widen away.
            let w = round_power_of_two(v, SGRPROJ_PRJ_BITS + SGRPROJ_RST_BITS) as i16;
            let out = clip_pixel_highbd(i32::from(w), bit_depth);
            dst.set(dst_origin + i * dst_stride + j, out);
        }
    }
}

// --------------------------------------------------------------------------
// Stripe drivers (restoration.c:964 / :1010)
// --------------------------------------------------------------------------

/// Port of `sgrproj_filter_stripe` (restoration.c:964) — the 8-bit stripe
/// driver `loop_restoration_filter_unit` dispatches to for `RESTORE_SGRPROJ`.
/// The port's existing `filter_unit_impl` has a Wiener arm only; this is the
/// missing one.
#[allow(clippy::too_many_arguments)]
pub fn sgrproj_filter_stripe(
    ep: usize,
    xqd: &[i32; 2],
    stripe_width: i32,
    stripe_height: i32,
    procunit_width: i32,
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u8],
    dst_origin: usize,
    dst_stride: usize,
) {
    let mut j = 0i32;
    while j < stripe_width {
        let w = procunit_width.min(stripe_width - j);
        let mut d = SgrDst::Lowbd(dst);
        apply_selfguided_restoration(
            SgrSrc::Lowbd(src),
            src_origin + j as usize,
            w,
            stripe_height,
            src_stride,
            ep,
            xqd,
            &mut d,
            dst_origin + j as usize,
            dst_stride,
            8,
        );
        j += procunit_width;
    }
}

/// Port of `sgrproj_filter_stripe_highbd` (restoration.c:1010) — the 10-bit
/// twin. Note the C code does NOT round `w` up to a multiple of 16 the way
/// `wiener_filter_stripe_highbd` does; the two differ and this one is a plain
/// `AOMMIN`.
#[allow(clippy::too_many_arguments)]
pub fn sgrproj_filter_stripe_highbd(
    ep: usize,
    xqd: &[i32; 2],
    stripe_width: i32,
    stripe_height: i32,
    procunit_width: i32,
    src: &[u16],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u16],
    dst_origin: usize,
    dst_stride: usize,
    bit_depth: i32,
) {
    let mut j = 0i32;
    while j < stripe_width {
        let w = procunit_width.min(stripe_width - j);
        let mut d = SgrDst::Highbd(dst);
        apply_selfguided_restoration(
            SgrSrc::Highbd(src),
            src_origin + j as usize,
            w,
            stripe_height,
            src_stride,
            ep,
            xqd,
            &mut d,
            dst_origin + j as usize,
            dst_stride,
            bit_depth,
        );
        j += procunit_width;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sgr_params_shape() {
        assert_eq!(SGR_PARAMS.len(), SGRPROJ_PARAMS);
        // Exactly one radius may be zero, never both.
        for (i, p) in SGR_PARAMS.iter().enumerate() {
            assert!(
                !(p.r[0] == 0 && p.r[1] == 0),
                "ep {i} disables both filters"
            );
            assert!(p.r[0] == 0 || p.r[0] == 2, "ep {i} r0 {}", p.r[0]);
            assert!(p.r[1] == 0 || p.r[1] == 1, "ep {i} r1 {}", p.r[1]);
        }
    }

    #[test]
    fn x_by_xplus1_zero_maps_to_one_not_zero() {
        // The special case C calls out explicitly. If a transcription dropped
        // it, flat content would take a different A[k] and shift every pixel.
        assert_eq!(X_BY_XPLUS1[0], 1);
        assert_eq!(X_BY_XPLUS1[255], 256);
    }

    #[test]
    fn decode_xq_matches_the_three_arms() {
        // r[0] == 0 (ep 10): xq[0] = 0, xq[1] absorbs the unit gain.
        let p = SGR_PARAMS[10];
        assert_eq!(p.r[0], 0);
        assert_eq!(decode_xq(&[13, 27], &p), [0, 128 - 27]);
        // r[1] == 0 (ep 14): xq[1] = 0, xq[0] passes through.
        let p = SGR_PARAMS[14];
        assert_eq!(p.r[1], 0);
        assert_eq!(decode_xq(&[13, 27], &p), [13, 0]);
        // both live (ep 0).
        let p = SGR_PARAMS[0];
        assert_eq!(decode_xq(&[13, 27], &p), [13, 128 - 13 - 27]);
    }
}
