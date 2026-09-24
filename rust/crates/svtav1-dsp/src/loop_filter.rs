//! Loop filters: deblocking (C-exact).
//!
//! Spec 08 (loop-filters.md): Deblocking. CDEF lives in [`crate::cdef`],
//! Wiener loop restoration in [`crate::restoration`] (C-exact ports).
//!
//! Deblocking is ported from SVT-AV1's `deblocking_common.c` and smooths
//! transform/prediction edges to reduce blocking artifacts.

// =============================================================================
// AV1 deblocking loop filter — C-exact port
//
// Kernels: SVT-AV1 `deblocking_common.c` svt_aom_lpf_{horizontal,vertical}_
// {4,6,8,14}_c (8-bit), which are byte-identical to libaom
// `aom_dsp/loopfilter.c` aom_lpf_*_c (the decoder's kernels). Each call
// filters one 4-sample edge segment: `horizontal_*` filter a horizontal
// edge over 4 columns, `vertical_*` filter a vertical edge over 4 rows.
//
// Thresholds: `lf_thresholds` ports svt_aom_update_sharpness
// (deblocking_common.c:568) + the hev_thr init from
// svt_av1_loop_filter_init (deblocking_filter.c:96, `lvl >> 4`), identical
// to libaom av1/common/av1_loopfilter.c update_sharpness /
// av1_loop_filter_frame_init.
//
// All of these are differentially fuzzed against the linked C reference in
// tests/c_parity_lpf.rs (bit-exact over the full (level, sharpness)
// parameter space).
// =============================================================================

#[allow(unused_imports)]
use archmage::prelude::*;

/// Loop-filter thresholds for one filter level, as passed to the kernels.
///
/// C: `LoopFilterThresh { mblim, lim, hev_thr }` (definitions.h:1710); the
/// kernels receive them as the (blimit, limit, thresh) pointer arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LfThresh {
    /// Outer edge threshold (`mblim` = 2 * (level + 2) + lim).
    pub mblim: u8,
    /// Inner ("block inside") limit derived from level + sharpness.
    pub lim: u8,
    /// High-edge-variance threshold (`level >> 4`).
    pub hev_thr: u8,
}

/// Thresholds for a filter `level` (0..=63) and `sharpness` (0..=7).
///
/// C provenance: svt_aom_update_sharpness (deblocking_common.c:568-589)
/// computes lim/mblim; hev_thr = level >> 4 comes from
/// svt_av1_loop_filter_init (deblocking_filter.c:96-98). libaom decoder:
/// update_sharpness + av1_loop_filter_frame_init(av1_loopfilter.c:47/121)
/// are identical.
pub fn lf_thresholds(level: u8, sharpness: u8) -> LfThresh {
    let lvl = level as i32;
    let sharpness_lvl = sharpness as i32;
    let mut block_inside_limit = lvl >> ((sharpness_lvl > 0) as i32 + (sharpness_lvl > 4) as i32);
    if sharpness_lvl > 0 && block_inside_limit > 9 - sharpness_lvl {
        block_inside_limit = 9 - sharpness_lvl;
    }
    if block_inside_limit < 1 {
        block_inside_limit = 1;
    }
    LfThresh {
        mblim: (2 * (lvl + 2) + block_inside_limit) as u8,
        lim: block_inside_limit as u8,
        hev_thr: level >> 4,
    }
}

/// C `signed_char_clamp` (deblocking_common.c:30).
#[inline]
fn signed_char_clamp(t: i32) -> i8 {
    t.clamp(-128, 127) as i8
}

/// C `filter_mask2` (deblocking_common.c:152): should any filter run at
/// all for a 4-tap edge. Returns 0 (no) or -1/0xFF (yes).
#[inline]
fn filter_mask2(limit: u8, blimit: u8, p1: u8, p0: u8, q0: u8, q1: u8) -> i8 {
    let (p1, p0, q0, q1) = (p1 as i32, p0 as i32, q0 as i32, q1 as i32);
    let mut mask: i8 = 0;
    mask |= -(((p1 - p0).abs() > limit as i32) as i8);
    mask |= -(((q1 - q0).abs() > limit as i32) as i8);
    mask |= -(((p0 - q0).abs() * 2 + (p1 - q1).abs() / 2 > blimit as i32) as i8);
    !mask
}

/// C `filter_mask` (deblocking_common.c:160) for 8-tap edges.
#[inline]
#[allow(clippy::too_many_arguments)]
fn filter_mask(
    limit: u8,
    blimit: u8,
    p3: u8,
    p2: u8,
    p1: u8,
    p0: u8,
    q0: u8,
    q1: u8,
    q2: u8,
    q3: u8,
) -> i8 {
    let (p3, p2, p1, p0) = (p3 as i32, p2 as i32, p1 as i32, p0 as i32);
    let (q0, q1, q2, q3) = (q0 as i32, q1 as i32, q2 as i32, q3 as i32);
    let l = limit as i32;
    let mut mask: i8 = 0;
    mask |= -(((p3 - p2).abs() > l) as i8);
    mask |= -(((p2 - p1).abs() > l) as i8);
    mask |= -(((p1 - p0).abs() > l) as i8);
    mask |= -(((q1 - q0).abs() > l) as i8);
    mask |= -(((q2 - q1).abs() > l) as i8);
    mask |= -(((q3 - q2).abs() > l) as i8);
    mask |= -(((p0 - q0).abs() * 2 + (p1 - q1).abs() / 2 > blimit as i32) as i8);
    !mask
}

/// C `filter_mask3_chroma` (deblocking_common.c:173) for 6-tap edges.
#[inline]
fn filter_mask3_chroma(
    limit: u8,
    blimit: u8,
    p2: u8,
    p1: u8,
    p0: u8,
    q0: u8,
    q1: u8,
    q2: u8,
) -> i8 {
    let (p2, p1, p0) = (p2 as i32, p1 as i32, p0 as i32);
    let (q0, q1, q2) = (q0 as i32, q1 as i32, q2 as i32);
    let l = limit as i32;
    let mut mask: i8 = 0;
    mask |= -(((p2 - p1).abs() > l) as i8);
    mask |= -(((p1 - p0).abs() > l) as i8);
    mask |= -(((q1 - q0).abs() > l) as i8);
    mask |= -(((q2 - q1).abs() > l) as i8);
    mask |= -(((p0 - q0).abs() * 2 + (p1 - q1).abs() / 2 > blimit as i32) as i8);
    !mask
}

/// C `flat_mask3_chroma` (deblocking_common.c:184).
#[inline]
fn flat_mask3_chroma(thresh: u8, p2: u8, p1: u8, p0: u8, q0: u8, q1: u8, q2: u8) -> i8 {
    let (p2, p1, p0) = (p2 as i32, p1 as i32, p0 as i32);
    let (q0, q1, q2) = (q0 as i32, q1 as i32, q2 as i32);
    let t = thresh as i32;
    let mut mask: i8 = 0;
    mask |= -(((p1 - p0).abs() > t) as i8);
    mask |= -(((q1 - q0).abs() > t) as i8);
    mask |= -(((p2 - p0).abs() > t) as i8);
    mask |= -(((q2 - q0).abs() > t) as i8);
    !mask
}

/// C `flat_mask4` (deblocking_common.c:205).
#[inline]
#[allow(clippy::too_many_arguments)]
fn flat_mask4(thresh: u8, p3: u8, p2: u8, p1: u8, p0: u8, q0: u8, q1: u8, q2: u8, q3: u8) -> i8 {
    let (p3, p2, p1, p0) = (p3 as i32, p2 as i32, p1 as i32, p0 as i32);
    let (q0, q1, q2, q3) = (q0 as i32, q1 as i32, q2 as i32, q3 as i32);
    let t = thresh as i32;
    let mut mask: i8 = 0;
    mask |= -(((p1 - p0).abs() > t) as i8);
    mask |= -(((q1 - q0).abs() > t) as i8);
    mask |= -(((p2 - p0).abs() > t) as i8);
    mask |= -(((q2 - q0).abs() > t) as i8);
    mask |= -(((p3 - p0).abs() > t) as i8);
    mask |= -(((q3 - q0).abs() > t) as i8);
    !mask
}

/// C `hev_mask` (deblocking_common.c:218): high edge variance.
#[inline]
fn hev_mask(thresh: u8, p1: u8, p0: u8, q0: u8, q1: u8) -> i8 {
    let t = thresh as i32;
    let mut hev: i8 = 0;
    hev |= -((((p1 as i32) - (p0 as i32)).abs() > t) as i8);
    hev |= -((((q1 as i32) - (q0 as i32)).abs() > t) as i8);
    hev
}

/// C `filter4` (deblocking_common.c:225) on a `[p1, p0, q0, q1]` window.
#[inline]
fn filter4_line(mask: i8, thresh: u8, w: &mut [u8; 4]) {
    let ps1 = (w[0] ^ 0x80) as i8;
    let ps0 = (w[1] ^ 0x80) as i8;
    let qs0 = (w[2] ^ 0x80) as i8;
    let qs1 = (w[3] ^ 0x80) as i8;
    let hev = hev_mask(thresh, w[0], w[1], w[2], w[3]);

    // add outer taps if we have high edge variance
    let mut filter = signed_char_clamp(ps1 as i32 - qs1 as i32) & hev;
    // inner taps
    filter = signed_char_clamp(filter as i32 + 3 * (qs0 as i32 - ps0 as i32)) & mask;

    // save bottom 3 bits so that we round one side +4 and the other +3
    let filter1 = signed_char_clamp(filter as i32 + 4) >> 3;
    let filter2 = signed_char_clamp(filter as i32 + 3) >> 3;
    w[2] = (signed_char_clamp(qs0 as i32 - filter1 as i32) as u8) ^ 0x80;
    w[1] = (signed_char_clamp(ps0 as i32 + filter2 as i32) as u8) ^ 0x80;

    // outer tap adjustments: ROUND_POWER_OF_TWO(filter1, 1) & ~hev
    let filter = (((filter1 as i32) + 1) >> 1) as i8 & !hev;
    w[3] = (signed_char_clamp(qs1 as i32 - filter as i32) as u8) ^ 0x80;
    w[0] = (signed_char_clamp(ps1 as i32 + filter as i32) as u8) ^ 0x80;
}

/// C `ROUND_POWER_OF_TWO(x, 3)` for the flat-filter taps (non-negative).
#[inline]
fn rpot3(x: i32) -> u8 {
    ((x + 4) >> 3) as u8
}

/// C `ROUND_POWER_OF_TWO(x, 4)`.
#[inline]
fn rpot4(x: i32) -> u8 {
    ((x + 8) >> 4) as u8
}

/// C `filter6` (deblocking_common.c:285) on a `[p2, p1, p0, q0, q1, q2]`
/// window.
#[inline]
fn filter6_line(mask: i8, thresh: u8, flat: i8, w: &mut [u8; 6]) {
    if flat != 0 && mask != 0 {
        let (p2, p1, p0) = (w[0] as i32, w[1] as i32, w[2] as i32);
        let (q0, q1, q2) = (w[3] as i32, w[4] as i32, w[5] as i32);
        // 5-tap filter [1, 2, 2, 2, 1]
        w[1] = rpot3(p2 * 3 + p1 * 2 + p0 * 2 + q0);
        w[2] = rpot3(p2 + p1 * 2 + p0 * 2 + q0 * 2 + q1);
        w[3] = rpot3(p1 + p0 * 2 + q0 * 2 + q1 * 2 + q2);
        w[4] = rpot3(p0 + q0 * 2 + q1 * 2 + q2 * 3);
    } else {
        let mut inner = [w[1], w[2], w[3], w[4]];
        filter4_line(mask, thresh, &mut inner);
        [w[1], w[2], w[3], w[4]] = inner;
    }
}

/// C `filter8` (deblocking_common.c:301) on a `[p3..p0, q0..q3]` window.
#[inline]
fn filter8_line(mask: i8, thresh: u8, flat: i8, w: &mut [u8; 8]) {
    if flat != 0 && mask != 0 {
        let (p3, p2, p1, p0) = (w[0] as i32, w[1] as i32, w[2] as i32, w[3] as i32);
        let (q0, q1, q2, q3) = (w[4] as i32, w[5] as i32, w[6] as i32, w[7] as i32);
        // 7-tap filter [1, 1, 1, 2, 1, 1, 1]
        w[1] = rpot3(p3 + p3 + p3 + 2 * p2 + p1 + p0 + q0);
        w[2] = rpot3(p3 + p3 + p2 + 2 * p1 + p0 + q0 + q1);
        w[3] = rpot3(p3 + p2 + p1 + 2 * p0 + q0 + q1 + q2);
        w[4] = rpot3(p2 + p1 + p0 + 2 * q0 + q1 + q2 + q3);
        w[5] = rpot3(p1 + p0 + q0 + 2 * q1 + q2 + q3 + q3);
        w[6] = rpot3(p0 + q0 + q1 + 2 * q2 + q3 + q3 + q3);
    } else {
        let mut inner = [w[2], w[3], w[4], w[5]];
        filter4_line(mask, thresh, &mut inner);
        [w[2], w[3], w[4], w[5]] = inner;
    }
}

/// C `filter14` (deblocking_common.c:786) on a `[p6..p0, q0..q6]` window.
#[inline]
fn filter14_line(mask: i8, thresh: u8, flat: i8, flat2: i8, w: &mut [u8; 14]) {
    if flat2 != 0 && flat != 0 && mask != 0 {
        let (p6, p5, p4, p3) = (w[0] as i32, w[1] as i32, w[2] as i32, w[3] as i32);
        let (p2, p1, p0) = (w[4] as i32, w[5] as i32, w[6] as i32);
        let (q0, q1, q2, q3) = (w[7] as i32, w[8] as i32, w[9] as i32, w[10] as i32);
        let (q4, q5, q6) = (w[11] as i32, w[12] as i32, w[13] as i32);
        // 13-tap filter [1, 1, 1, 1, 1, 2, 2, 2, 1, 1, 1, 1, 1]
        w[1] = rpot4(p6 * 7 + p5 * 2 + p4 * 2 + p3 + p2 + p1 + p0 + q0);
        w[2] = rpot4(p6 * 5 + p5 * 2 + p4 * 2 + p3 * 2 + p2 + p1 + p0 + q0 + q1);
        w[3] = rpot4(p6 * 4 + p5 + p4 * 2 + p3 * 2 + p2 * 2 + p1 + p0 + q0 + q1 + q2);
        w[4] = rpot4(p6 * 3 + p5 + p4 + p3 * 2 + p2 * 2 + p1 * 2 + p0 + q0 + q1 + q2 + q3);
        w[5] = rpot4(p6 * 2 + p5 + p4 + p3 + p2 * 2 + p1 * 2 + p0 * 2 + q0 + q1 + q2 + q3 + q4);
        w[6] = rpot4(p6 + p5 + p4 + p3 + p2 + p1 * 2 + p0 * 2 + q0 * 2 + q1 + q2 + q3 + q4 + q5);
        w[7] = rpot4(p5 + p4 + p3 + p2 + p1 + p0 * 2 + q0 * 2 + q1 * 2 + q2 + q3 + q4 + q5 + q6);
        w[8] = rpot4(p4 + p3 + p2 + p1 + p0 + q0 * 2 + q1 * 2 + q2 * 2 + q3 + q4 + q5 + q6 * 2);
        w[9] = rpot4(p3 + p2 + p1 + p0 + q0 + q1 * 2 + q2 * 2 + q3 * 2 + q4 + q5 + q6 * 3);
        w[10] = rpot4(p2 + p1 + p0 + q0 + q1 + q2 * 2 + q3 * 2 + q4 * 2 + q5 + q6 * 4);
        w[11] = rpot4(p1 + p0 + q0 + q1 + q2 + q3 * 2 + q4 * 2 + q5 * 2 + q6 * 5);
        w[12] = rpot4(p0 + q0 + q1 + q2 + q3 + q4 * 2 + q5 * 2 + q6 * 7);
    } else {
        let mut inner = [w[3], w[4], w[5], w[6], w[7], w[8], w[9], w[10]];
        filter8_line(mask, thresh, flat, &mut inner);
        [w[3], w[4], w[5], w[6], w[7], w[8], w[9], w[10]] = inner;
    }
}

/// Gather `N` samples centered on the edge at `base` with step `step`
/// (sample k lives at `base + (k - N/2) * step`), i.e. `w[N/2]` is q0.
#[inline]
fn gather<const N: usize>(buf: &[u8], base: usize, step: usize) -> [u8; N] {
    let mut w = [0u8; N];
    let start = base - (N / 2) * step;
    for (k, s) in w.iter_mut().enumerate() {
        *s = buf[start + k * step];
    }
    w
}

/// Scatter the window back (inverse of [`gather`]).
#[inline]
fn scatter<const N: usize>(buf: &mut [u8], base: usize, step: usize, w: &[u8; N]) {
    let start = base - (N / 2) * step;
    for (k, s) in w.iter().enumerate() {
        buf[start + k * step] = *s;
    }
}

/// C `svt_aom_lpf_horizontal_4_c`: filter a horizontal edge over 4 columns.
/// `off` indexes q0 in the first column; taps step by `pitch`.
pub fn lpf_horizontal_4(buf: &mut [u8], off: usize, pitch: usize, t: LfThresh) {
    for i in 0..4 {
        let base = off + i;
        let mut w: [u8; 4] = gather(buf, base, pitch);
        let mask = filter_mask2(t.lim, t.mblim, w[0], w[1], w[2], w[3]);
        filter4_line(mask, t.hev_thr, &mut w);
        scatter(buf, base, pitch, &w);
    }
}

/// C `svt_aom_lpf_vertical_4_c`: filter a vertical edge over 4 rows.
/// `off` indexes q0 in the first row; taps are contiguous.
pub fn lpf_vertical_4(buf: &mut [u8], off: usize, pitch: usize, t: LfThresh) {
    for i in 0..4 {
        let base = off + i * pitch;
        let mut w: [u8; 4] = gather(buf, base, 1);
        let mask = filter_mask2(t.lim, t.mblim, w[0], w[1], w[2], w[3]);
        filter4_line(mask, t.hev_thr, &mut w);
        scatter(buf, base, 1, &w);
    }
}

/// C `svt_aom_lpf_horizontal_6` — RTCD dispatches to the `_sse2` kernel on
/// x86 (ported below) and `_c` elsewhere.
pub fn lpf_horizontal_6(buf: &mut [u8], off: usize, pitch: usize, t: LfThresh) {
    incant!(
        lpf_horizontal_6_impl(buf, off, pitch, t),
        [v3, neon, scalar]
    )
}

#[allow(clippy::too_many_arguments)]
fn lpf_horizontal_6_impl_scalar(
    _t: ScalarToken,
    buf: &mut [u8],
    off: usize,
    pitch: usize,
    t: LfThresh,
) {
    for i in 0..4 {
        let base = off + i;
        let mut w: [u8; 6] = gather(buf, base, pitch);
        let mask = filter_mask3_chroma(t.lim, t.mblim, w[0], w[1], w[2], w[3], w[4], w[5]);
        let flat = flat_mask3_chroma(1, w[0], w[1], w[2], w[3], w[4], w[5]);
        filter6_line(mask, t.hev_thr, flat, &mut w);
        scatter(buf, base, pitch, &w);
    }
}

/// C `svt_aom_lpf_vertical_6` — same dispatch as [`lpf_horizontal_6`].
pub fn lpf_vertical_6(buf: &mut [u8], off: usize, pitch: usize, t: LfThresh) {
    incant!(lpf_vertical_6_impl(buf, off, pitch, t), [v3, neon, scalar])
}

#[allow(clippy::too_many_arguments)]
fn lpf_vertical_6_impl_scalar(
    _t: ScalarToken,
    buf: &mut [u8],
    off: usize,
    pitch: usize,
    t: LfThresh,
) {
    for i in 0..4 {
        let base = off + i * pitch;
        let mut w: [u8; 6] = gather(buf, base, 1);
        let mask = filter_mask3_chroma(t.lim, t.mblim, w[0], w[1], w[2], w[3], w[4], w[5]);
        let flat = flat_mask3_chroma(1, w[0], w[1], w[2], w[3], w[4], w[5]);
        filter6_line(mask, t.hev_thr, flat, &mut w);
        scatter(buf, base, 1, &w);
    }
}

/// C `svt_aom_lpf_horizontal_8` — RTCD dispatches to the `_sse2` kernel on
/// x86 (ported below) and `_c` elsewhere.
pub fn lpf_horizontal_8(buf: &mut [u8], off: usize, pitch: usize, t: LfThresh) {
    incant!(
        lpf_horizontal_8_impl(buf, off, pitch, t),
        [v3, neon, scalar]
    )
}

#[allow(clippy::too_many_arguments)]
fn lpf_horizontal_8_impl_scalar(
    _t: ScalarToken,
    buf: &mut [u8],
    off: usize,
    pitch: usize,
    t: LfThresh,
) {
    for i in 0..4 {
        let base = off + i;
        let mut w: [u8; 8] = gather(buf, base, pitch);
        let mask = filter_mask(
            t.lim, t.mblim, w[0], w[1], w[2], w[3], w[4], w[5], w[6], w[7],
        );
        let flat = flat_mask4(1, w[0], w[1], w[2], w[3], w[4], w[5], w[6], w[7]);
        filter8_line(mask, t.hev_thr, flat, &mut w);
        scatter(buf, base, pitch, &w);
    }
}

/// C `svt_aom_lpf_vertical_8` — same dispatch as [`lpf_horizontal_8`].
pub fn lpf_vertical_8(buf: &mut [u8], off: usize, pitch: usize, t: LfThresh) {
    incant!(lpf_vertical_8_impl(buf, off, pitch, t), [v3, neon, scalar])
}

#[allow(clippy::too_many_arguments)]
fn lpf_vertical_8_impl_scalar(
    _t: ScalarToken,
    buf: &mut [u8],
    off: usize,
    pitch: usize,
    t: LfThresh,
) {
    for i in 0..4 {
        let base = off + i * pitch;
        let mut w: [u8; 8] = gather(buf, base, 1);
        let mask = filter_mask(
            t.lim, t.mblim, w[0], w[1], w[2], w[3], w[4], w[5], w[6], w[7],
        );
        let flat = flat_mask4(1, w[0], w[1], w[2], w[3], w[4], w[5], w[6], w[7]);
        filter8_line(mask, t.hev_thr, flat, &mut w);
        scatter(buf, base, 1, &w);
    }
}

/// Shared 14-tap body on a gathered `[p6..q6]` window (C
/// `mb_lpf_horizontal_edge_w` / `mb_lpf_vertical_edge_w` inner loop).
#[inline]
fn lpf14_window(w: &mut [u8; 14], t: LfThresh) {
    // mask/flat use the inner [p3..q3]; flat2 uses p6,p5,p4,p0,q0,q4,q5,q6.
    let mask = filter_mask(
        t.lim, t.mblim, w[3], w[4], w[5], w[6], w[7], w[8], w[9], w[10],
    );
    let flat = flat_mask4(1, w[3], w[4], w[5], w[6], w[7], w[8], w[9], w[10]);
    let flat2 = flat_mask4(1, w[0], w[1], w[2], w[6], w[7], w[11], w[12], w[13]);
    filter14_line(mask, t.hev_thr, flat, flat2, w);
}

/// C `svt_aom_lpf_horizontal_14` — RTCD dispatches to the `_sse2` kernel on
/// x86 (ported below) and `_c` elsewhere.
pub fn lpf_horizontal_14(buf: &mut [u8], off: usize, pitch: usize, t: LfThresh) {
    incant!(
        lpf_horizontal_14_impl(buf, off, pitch, t),
        [v3, neon, scalar]
    )
}

#[allow(clippy::too_many_arguments)]
fn lpf_horizontal_14_impl_scalar(
    _t: ScalarToken,
    buf: &mut [u8],
    off: usize,
    pitch: usize,
    t: LfThresh,
) {
    for i in 0..4 {
        let base = off + i;
        let mut w: [u8; 14] = gather(buf, base, pitch);
        lpf14_window(&mut w, t);
        scatter(buf, base, pitch, &w);
    }
}

/// C `svt_aom_lpf_vertical_14` — same dispatch as [`lpf_horizontal_14`].
pub fn lpf_vertical_14(buf: &mut [u8], off: usize, pitch: usize, t: LfThresh) {
    incant!(lpf_vertical_14_impl(buf, off, pitch, t), [v3, neon, scalar])
}

#[allow(clippy::too_many_arguments)]
fn lpf_vertical_14_impl_scalar(
    _t: ScalarToken,
    buf: &mut [u8],
    off: usize,
    pitch: usize,
    t: LfThresh,
) {
    for i in 0..4 {
        let base = off + i * pitch;
        let mut w: [u8; 14] = gather(buf, base, 1);
        lpf14_window(&mut w, t);
        scatter(buf, base, 1, &w);
    }
}

// =============================================================================
// AVX2 (v3) 14-tap kernels — ports of `ASM_SSE2/dlf_intrin_sse2.c`
// `svt_aom_lpf_{horizontal,vertical}_14_sse2` + their shared
// `lpf_internal_14_sse2` / `filter4_14_sse2` / `transpose_pq_14*` helpers.
//
// The SSE2 kernels are what the C encoder actually runs on x86 (RTCD maps
// lpf_14 to the _sse2 forms); the scalar bodies above remain the reference
// and the non-x86 fallback. Every op is exact integer math, so the vector
// arms are byte-identical to scalar by construction — both are fuzzed
// against the linked C reference in tests/c_parity_lpf.rs.
//
// Bounds: lpf14 only runs where `lpf_params` measured BOTH adjacent blocks
// at >=16 px (min_dim > 8 -> len 14), so the s-7..s+6 row window (horizontal)
// and the s-8..s+7 column window (vertical) are always inside the plane.
// =============================================================================

/// C `abs_diff` (dlf_intrin_sse2.c:143).
#[cfg(target_arch = "x86_64")]
#[rite]
fn lpf_abs_diff_v3(_t: Desktop64, a: __m128i, b: __m128i) -> __m128i {
    _mm_or_si128(_mm_subs_epu8(a, b), _mm_subs_epu8(b, a))
}

/// C `filter4_14_sse2` (dlf_intrin_sse2.c:227): the narrow-filter half of
/// the 14-tap kernel. Returns the (qs1qs0, ps1ps0) merged output pairs.
#[cfg(target_arch = "x86_64")]
#[rite]
fn filter4_14_v3(
    _t: Desktop64,
    p1p0: __m128i,
    q1q0: __m128i,
    hev: __m128i,
    mask: __m128i,
) -> (__m128i, __m128i) {
    let t3t4 = _mm_set_epi8(0, 0, 0, 0, 0, 0, 0, 0, 3, 3, 3, 3, 4, 4, 4, 4);
    let t80 = _mm_set1_epi8(0x80u8 as i8);
    let ff = _mm_cmpeq_epi8(t80, t80);

    let ps1ps0_work = _mm_xor_si128(p1p0, t80);
    let mut qs1qs0_work = _mm_xor_si128(q1q0, t80);

    // filter = signed_char_clamp(ps1 - qs1) & hev
    let work = _mm_subs_epi8(ps1ps0_work, qs1qs0_work);
    let mut filter = _mm_and_si128(_mm_srli_si128::<4>(work), hev);
    // filter = signed_char_clamp(filter + 3 * (qs0 - ps0)) & mask
    filter = _mm_subs_epi8(filter, work);
    filter = _mm_subs_epi8(filter, work);
    filter = _mm_subs_epi8(filter, work);
    filter = _mm_and_si128(filter, mask);
    filter = _mm_unpacklo_epi32(filter, filter);

    // filter1 = signed_char_clamp(filter + 4) >> 3;
    // filter2 = signed_char_clamp(filter + 3) >> 3
    let mut f21 = _mm_adds_epi8(filter, t3t4);
    f21 = _mm_unpacklo_epi8(f21, f21);
    f21 = _mm_srai_epi16::<11>(f21);
    f21 = _mm_packs_epi16(f21, f21);

    // filter = ROUND_POWER_OF_TWO(filter1, 1) & ~hev
    filter = _mm_subs_epi8(f21, ff);
    filter = _mm_unpacklo_epi8(filter, filter);
    filter = _mm_srai_epi16::<9>(filter);
    filter = _mm_packs_epi16(filter, filter);
    filter = _mm_andnot_si128(hev, filter);
    filter = _mm_unpacklo_epi32(filter, filter);

    let f21m = _mm_unpacklo_epi32(f21, filter);
    let hev1 = _mm_srli_si128::<8>(f21m);
    // signed_char_clamp(qs1 - filter), signed_char_clamp(qs0 - filter1)
    qs1qs0_work = _mm_subs_epi8(qs1qs0_work, f21m);
    // signed_char_clamp(ps1 + filter), signed_char_clamp(ps0 + filter2)
    let ps1ps0_out = _mm_adds_epi8(ps1ps0_work, hev1);

    (
        _mm_xor_si128(qs1qs0_work, t80),
        _mm_xor_si128(ps1ps0_out, t80),
    )
}

/// C `lpf_internal_14_sse2` (dlf_intrin_sse2.c:357). `qp[i]` is the merged
/// `q{i}p{i}` pair vector; `qp[0..=5]` are written back, `qp[6]` is
/// read-only (the outermost tap pair never changes).
#[cfg(target_arch = "x86_64")]
#[rite]
#[allow(clippy::too_many_arguments)]
fn lpf_internal_14_v3(
    token: Desktop64,
    qp: &mut [__m128i; 7],
    blimit16: __m128i,
    limit: __m128i,
    thresh: __m128i,
) {
    let zero = _mm_setzero_si128();
    let one = _mm_set1_epi8(1);
    let fe = _mm_set1_epi8(0xfeu8 as i8);
    let ff = _mm_cmpeq_epi8(fe, fe);

    let p1p0 = _mm_unpacklo_epi32(qp[0], qp[1]);
    let q1q0 = _mm_srli_si128::<8>(p1p0);

    // filter_mask + hev_mask (dlf_intrin_sse2.c:371-404)
    let abs_p1p0 = lpf_abs_diff_v3(token, qp[1], qp[0]);
    let abs_q1q0 = _mm_srli_si128::<4>(abs_p1p0);
    let abs_p0q0 = lpf_abs_diff_v3(token, p1p0, q1q0);
    let mut abs_p1q1 = _mm_srli_si128::<4>(abs_p0q0);

    let flat_a = _mm_max_epu8(abs_p1p0, abs_q1q0);
    let mut hev = _mm_subs_epu8(flat_a, thresh);
    hev = _mm_xor_si128(_mm_cmpeq_epi8(hev, zero), ff);
    // replicate for the "merged variables" usage
    hev = _mm_unpacklo_epi32(hev, hev);

    abs_p1q1 = _mm_srli_epi16::<1>(_mm_and_si128(abs_p1q1, fe));
    // The sum>blimit "don't filter" flag, computed in u16 and kept separate
    // from the diff max. C's _sse2 kernel (a) saturates the u8 sum at 255,
    // dropping the flag for true sums 256..510 at blimit=255, and (b) folds
    // the FF flag through subs(x, limit) where it collapses at limit=255.
    // _c's `~mask` keeps both. mblim <= 193 and lim <= 63 in every
    // derivable threshold (deblocking_common.c:574-587), so this is
    // _sse2-exact on all reachable inputs and _c-exact everywhere.
    let a16 = _mm_unpacklo_epi8(abs_p0q0, zero);
    let s16 = _mm_add_epi16(_mm_add_epi16(a16, a16), _mm_unpacklo_epi8(abs_p1q1, zero));
    let flag = _mm_packs_epi16(
        _mm_cmpgt_epi16(s16, blimit16),
        _mm_cmpgt_epi16(s16, blimit16),
    );

    let mut work = _mm_max_epu8(
        lpf_abs_diff_v3(token, qp[2], qp[1]),
        lpf_abs_diff_v3(token, qp[3], qp[2]),
    );
    work = _mm_max_epu8(abs_p1p0, work);
    work = _mm_max_epu8(work, _mm_srli_si128::<4>(work));
    work = _mm_subs_epu8(work, limit);
    let mask = _mm_andnot_si128(flag, _mm_cmpeq_epi8(work, zero));

    // lp filter (shared with the 6/8-tap kernels)
    let (qs1qs0, ps1ps0) = filter4_14_v3(token, p1p0, q1q0, hev, mask);
    let mut qs0ps0 = _mm_unpacklo_epi32(ps1ps0, qs1qs0);
    let mut qs1ps1 = _mm_srli_si128::<8>(qs0ps0);

    // flat mask
    let mut flat = _mm_max_epu8(
        lpf_abs_diff_v3(token, qp[2], qp[0]),
        lpf_abs_diff_v3(token, qp[3], qp[0]),
    );
    flat = _mm_max_epu8(abs_p1p0, flat);
    flat = _mm_max_epu8(flat, _mm_srli_si128::<4>(flat));
    flat = _mm_subs_epu8(flat, one);
    flat = _mm_cmpeq_epi8(flat, zero);
    flat = _mm_and_si128(flat, mask);
    flat = _mm_unpacklo_epi32(flat, flat);
    flat = _mm_unpacklo_epi64(flat, flat);

    // if flat == 0 then flat2 is zero as well and none of this is needed
    if _mm_movemask_epi8(_mm_cmpeq_epi8(flat, zero)) != 0xffff {
        // flat and wide-flat calculations
        let eight = _mm_set1_epi16(8);
        let four = _mm_set1_epi16(4);
        let mut pq_16 = [zero; 7];
        for (i, v) in pq_16.iter_mut().enumerate() {
            *v = _mm_unpacklo_epi8(qp[i], zero);
        }
        let q0_16 = _mm_srli_si128::<8>(pq_16[0]);
        let q1_16 = _mm_srli_si128::<8>(pq_16[1]);
        let q2_16 = _mm_srli_si128::<8>(pq_16[2]);
        let q3_16 = _mm_srli_si128::<8>(pq_16[3]);
        let q4_16 = _mm_srli_si128::<8>(pq_16[4]);
        let q5_16 = _mm_srli_si128::<8>(pq_16[5]);

        let mut sum_p = _mm_add_epi16(pq_16[5], _mm_add_epi16(pq_16[4], pq_16[3]));
        let mut sum_lp = _mm_add_epi16(pq_16[0], _mm_add_epi16(pq_16[2], pq_16[1]));
        sum_p = _mm_add_epi16(sum_p, sum_lp);

        let mut sum_lq = _mm_srli_si128::<8>(sum_lp);
        let mut sum_q = _mm_srli_si128::<8>(sum_p);

        let sum_p_0 = _mm_add_epi16(eight, _mm_add_epi16(sum_p, sum_q));
        sum_lp = _mm_add_epi16(four, _mm_add_epi16(sum_lp, sum_lq));

        let flat_p0 = _mm_add_epi16(sum_lp, _mm_add_epi16(pq_16[3], pq_16[0]));
        let flat_q0 = _mm_add_epi16(sum_lp, _mm_add_epi16(q3_16, q0_16));

        let mut sum_p6 = _mm_add_epi16(pq_16[6], pq_16[6]);
        let mut sum_p3 = _mm_add_epi16(pq_16[3], pq_16[3]);

        sum_q = _mm_sub_epi16(sum_p_0, pq_16[5]);
        sum_p = _mm_sub_epi16(sum_p_0, q5_16);

        let work0_0 = _mm_add_epi16(_mm_add_epi16(pq_16[6], pq_16[0]), pq_16[1]);
        let work0_1 = _mm_add_epi16(
            sum_p6,
            _mm_add_epi16(pq_16[1], _mm_add_epi16(pq_16[2], pq_16[0])),
        );

        sum_lq = _mm_sub_epi16(sum_lp, pq_16[2]);
        sum_lp = _mm_sub_epi16(sum_lp, q2_16);

        work = _mm_add_epi16(sum_p3, pq_16[1]);
        let flat_p1 = _mm_add_epi16(sum_lp, work);
        let flat_q1 = _mm_add_epi16(sum_lq, _mm_srli_si128::<8>(work));

        let mut flat_pq0 = _mm_srli_epi16::<3>(_mm_unpacklo_epi64(flat_p0, flat_q0));
        let mut flat_pq1 = _mm_srli_epi16::<3>(_mm_unpacklo_epi64(flat_p1, flat_q1));
        flat_pq0 = _mm_packus_epi16(flat_pq0, flat_pq0);
        flat_pq1 = _mm_packus_epi16(flat_pq1, flat_pq1);

        sum_lp = _mm_sub_epi16(sum_lp, q1_16);
        sum_lq = _mm_sub_epi16(sum_lq, pq_16[1]);

        sum_p3 = _mm_add_epi16(sum_p3, pq_16[3]);
        work = _mm_add_epi16(sum_p3, pq_16[2]);

        let flat_p2 = _mm_add_epi16(sum_lp, work);
        let flat_q2 = _mm_add_epi16(sum_lq, _mm_srli_si128::<8>(work));
        let mut flat_pq2 = _mm_srli_epi16::<3>(_mm_unpacklo_epi64(flat_p2, flat_q2));
        flat_pq2 = _mm_packus_epi16(flat_pq2, flat_pq2);

        // flat2 mask
        let mut flat2 = _mm_max_epu8(
            lpf_abs_diff_v3(token, qp[4], qp[0]),
            lpf_abs_diff_v3(token, qp[5], qp[0]),
        );
        work = lpf_abs_diff_v3(token, qp[6], qp[0]);
        flat2 = _mm_max_epu8(work, flat2);
        flat2 = _mm_max_epu8(flat2, _mm_srli_si128::<4>(flat2));
        flat2 = _mm_subs_epu8(flat2, one);
        flat2 = _mm_cmpeq_epi8(flat2, zero);
        flat2 = _mm_and_si128(flat2, flat); // flat2 & flat & mask
        flat2 = _mm_unpacklo_epi32(flat2, flat2);

        // apply flat
        qs0ps0 = _mm_andnot_si128(flat, qs0ps0);
        flat_pq0 = _mm_and_si128(flat, flat_pq0);
        qp[0] = _mm_or_si128(qs0ps0, flat_pq0);

        qs1ps1 = _mm_andnot_si128(flat, qs1ps1);
        flat_pq1 = _mm_and_si128(flat, flat_pq1);
        qp[1] = _mm_or_si128(qs1ps1, flat_pq1);

        qp[2] = _mm_andnot_si128(flat, qp[2]);
        flat_pq2 = _mm_and_si128(flat, flat_pq2);
        qp[2] = _mm_or_si128(qp[2], flat_pq2);

        if _mm_movemask_epi8(_mm_cmpeq_epi8(flat2, zero)) != 0xffff {
            let mut flat2_pq = [zero; 6];

            let flat2_p0 = _mm_add_epi16(sum_p_0, _mm_add_epi16(work0_0, q0_16));
            let flat2_q0 = _mm_add_epi16(
                sum_p_0,
                _mm_add_epi16(_mm_srli_si128::<8>(work0_0), pq_16[0]),
            );

            let flat2_p1 = _mm_add_epi16(sum_p, work0_1);
            let flat2_q1 = _mm_add_epi16(sum_q, _mm_srli_si128::<8>(work0_1));

            flat2_pq[0] = _mm_srli_epi16::<4>(_mm_unpacklo_epi64(flat2_p0, flat2_q0));
            flat2_pq[1] = _mm_srli_epi16::<4>(_mm_unpacklo_epi64(flat2_p1, flat2_q1));
            flat2_pq[0] = _mm_packus_epi16(flat2_pq[0], flat2_pq[0]);
            flat2_pq[1] = _mm_packus_epi16(flat2_pq[1], flat2_pq[1]);

            sum_p = _mm_sub_epi16(sum_p, q4_16);
            sum_q = _mm_sub_epi16(sum_q, pq_16[4]);

            sum_p6 = _mm_add_epi16(sum_p6, pq_16[6]);
            work = _mm_add_epi16(
                sum_p6,
                _mm_add_epi16(pq_16[2], _mm_add_epi16(pq_16[3], pq_16[1])),
            );
            let flat2_p2 = _mm_add_epi16(sum_p, work);
            let flat2_q2 = _mm_add_epi16(sum_q, _mm_srli_si128::<8>(work));
            flat2_pq[2] = _mm_srli_epi16::<4>(_mm_unpacklo_epi64(flat2_p2, flat2_q2));
            flat2_pq[2] = _mm_packus_epi16(flat2_pq[2], flat2_pq[2]);

            sum_p6 = _mm_add_epi16(sum_p6, pq_16[6]);
            sum_p = _mm_sub_epi16(sum_p, q3_16);
            sum_q = _mm_sub_epi16(sum_q, pq_16[3]);

            work = _mm_add_epi16(
                sum_p6,
                _mm_add_epi16(pq_16[3], _mm_add_epi16(pq_16[4], pq_16[2])),
            );
            let flat2_p3 = _mm_add_epi16(sum_p, work);
            let flat2_q3 = _mm_add_epi16(sum_q, _mm_srli_si128::<8>(work));
            flat2_pq[3] = _mm_srli_epi16::<4>(_mm_unpacklo_epi64(flat2_p3, flat2_q3));
            flat2_pq[3] = _mm_packus_epi16(flat2_pq[3], flat2_pq[3]);

            sum_p6 = _mm_add_epi16(sum_p6, pq_16[6]);
            sum_p = _mm_sub_epi16(sum_p, q2_16);
            sum_q = _mm_sub_epi16(sum_q, pq_16[2]);

            work = _mm_add_epi16(
                sum_p6,
                _mm_add_epi16(pq_16[4], _mm_add_epi16(pq_16[5], pq_16[3])),
            );
            let flat2_p4 = _mm_add_epi16(sum_p, work);
            let flat2_q4 = _mm_add_epi16(sum_q, _mm_srli_si128::<8>(work));
            flat2_pq[4] = _mm_srli_epi16::<4>(_mm_unpacklo_epi64(flat2_p4, flat2_q4));
            flat2_pq[4] = _mm_packus_epi16(flat2_pq[4], flat2_pq[4]);

            sum_p6 = _mm_add_epi16(sum_p6, pq_16[6]);
            sum_p = _mm_sub_epi16(sum_p, q1_16);
            sum_q = _mm_sub_epi16(sum_q, pq_16[1]);

            work = _mm_add_epi16(
                sum_p6,
                _mm_add_epi16(pq_16[5], _mm_add_epi16(pq_16[6], pq_16[4])),
            );
            let flat2_p5 = _mm_add_epi16(sum_p, work);
            let flat2_q5 = _mm_add_epi16(sum_q, _mm_srli_si128::<8>(work));
            flat2_pq[5] = _mm_srli_epi16::<4>(_mm_unpacklo_epi64(flat2_p5, flat2_q5));
            flat2_pq[5] = _mm_packus_epi16(flat2_pq[5], flat2_pq[5]);

            // wide flat apply
            for i in 0..6 {
                qp[i] = _mm_or_si128(
                    _mm_andnot_si128(flat2, qp[i]),
                    _mm_and_si128(flat2, flat2_pq[i]),
                );
            }
        }
    } else {
        qp[0] = qs0ps0;
        qp[1] = qs1ps1;
    }
}

/// C `svt_aom_lpf_horizontal_14_sse2` — 4 columns. Loads the 4-byte rows
/// p6..q6 pairwise into the merged `q{i}p{i}` vectors, filters, and stores
/// the 12 modified rows (p5..q5) back through `store_buffer_horz_8`.
#[cfg(target_arch = "x86_64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn lpf_horizontal_14_impl_v3(
    token: Desktop64,
    buf: &mut [u8],
    off: usize,
    pitch: usize,
    t: LfThresh,
) {
    let blimit16 = _mm_set1_epi16(t.mblim as i16);
    let limit = _mm_set1_epi8(t.lim as i8);
    let thresh = _mm_set1_epi8(t.hev_thr as i8);

    let row = |k: isize| -> __m128i {
        let i = (off as isize + k * pitch as isize) as usize;
        let r: &[u8; 4] = buf[i..i + 4].try_into().unwrap();
        _mm_loadu_si32(r)
    };
    let mut qp = [
        _mm_unpacklo_epi32(row(-1), row(0)),
        _mm_unpacklo_epi32(row(-2), row(1)),
        _mm_unpacklo_epi32(row(-3), row(2)),
        _mm_unpacklo_epi32(row(-4), row(3)),
        _mm_unpacklo_epi32(row(-5), row(4)),
        _mm_unpacklo_epi32(row(-6), row(5)),
        _mm_unpacklo_epi32(row(-7), row(6)),
    ];
    lpf_internal_14_v3(token, &mut qp, blimit16, limit, thresh);
    // C `store_buffer_horz_8(q{num}p{num}, p, num, s)`, num = 0..=5:
    // stores 4 bytes at s-(num+1)*p and s+num*p.
    for num in 0..6isize {
        let v = qp[num as usize];
        let lo = (off as isize - (num + 1) * pitch as isize) as usize;
        let d_lo: &mut [u8; 4] = (&mut buf[lo..lo + 4]).try_into().unwrap();
        _mm_storeu_si32(d_lo, v);
        let hi = (off as isize + num * pitch as isize) as usize;
        let d_hi: &mut [u8; 4] = (&mut buf[hi..hi + 4]).try_into().unwrap();
        _mm_storeu_si32(d_hi, _mm_srli_si128::<4>(v));
    }
}

/// C `transpose_pq_14_sse2` (dlf_intrin_sse2.c:1029): four 16-byte rows in
/// (x[0..4] = file rows 0..3), eight merged `q{i}p{i}` pairs out.
#[cfg(target_arch = "x86_64")]
#[rite]
fn transpose_pq_14_v3(_t: Desktop64, x: &[__m128i; 4]) -> [__m128i; 8] {
    let w0 = _mm_unpacklo_epi8(x[0], x[1]);
    let w1 = _mm_unpacklo_epi8(x[2], x[3]);
    let w2 = _mm_unpackhi_epi8(x[0], x[1]);
    let w3 = _mm_unpackhi_epi8(x[2], x[3]);

    let ww0 = _mm_unpacklo_epi16(w0, w1);
    let ww1 = _mm_unpackhi_epi16(w0, w1);
    let ww2 = _mm_unpacklo_epi16(w2, w3);
    let ww3 = _mm_unpackhi_epi16(w2, w3);

    [
        _mm_unpacklo_epi32(_mm_srli_si128::<12>(ww1), ww2), // q0p0
        _mm_unpackhi_epi32(ww1, _mm_slli_si128::<4>(ww2)),  // q1p1
        _mm_unpackhi_epi32(_mm_slli_si128::<4>(ww1), ww2),  // q2p2
        _mm_unpacklo_epi32(ww1, _mm_srli_si128::<12>(ww2)), // q3p3
        _mm_unpacklo_epi32(_mm_srli_si128::<12>(ww0), ww3), // q4p4
        _mm_unpackhi_epi32(ww0, _mm_slli_si128::<4>(ww3)),  // q5p5
        _mm_unpackhi_epi32(_mm_slli_si128::<4>(ww0), ww3),  // q6p6
        _mm_unpacklo_epi32(ww0, _mm_srli_si128::<12>(ww3)), // q7p7
    ]
}

/// C `transpose_pq_14_inv_sse2` (dlf_intrin_sse2.c:1062): eight merged
/// pairs in (x[0..8] = q7p7..q0p0, C's argument order), four 16-byte
/// rows out.
#[cfg(target_arch = "x86_64")]
#[rite]
fn transpose_pq_14_inv_v3(_t: Desktop64, x: &[__m128i; 8]) -> [__m128i; 4] {
    let w0 = _mm_unpacklo_epi8(x[0], x[1]);
    let w1 = _mm_unpacklo_epi8(x[2], x[3]);
    let w2 = _mm_unpacklo_epi8(x[4], x[5]);
    let w3 = _mm_unpacklo_epi8(x[6], x[7]);

    let w4 = _mm_unpacklo_epi16(w0, w1);
    let w5 = _mm_unpacklo_epi16(w2, w3);

    let d0 = _mm_unpacklo_epi32(w4, w5);
    let d2 = _mm_unpackhi_epi32(w4, w5);

    let w10 = _mm_unpacklo_epi8(x[7], x[6]);
    let w11 = _mm_unpacklo_epi8(x[5], x[4]);
    let w12 = _mm_unpacklo_epi8(x[3], x[2]);
    let w13 = _mm_unpacklo_epi8(x[1], x[0]);

    let w4b = _mm_unpackhi_epi16(w10, w11);
    let w5b = _mm_unpackhi_epi16(w12, w13);

    let d1 = _mm_unpacklo_epi32(w4b, w5b);
    let d3 = _mm_unpackhi_epi32(w4b, w5b);

    [
        _mm_unpacklo_epi64(d0, d1),
        _mm_unpackhi_epi64(d0, d1),
        _mm_unpacklo_epi64(d2, d3),
        _mm_unpackhi_epi64(d2, d3),
    ]
}

/// C `svt_aom_lpf_vertical_14_sse2` — 4 rows. Loads a 16-byte p7..q7
/// window per row (the outermost pair round-trips unchanged), transposes
/// to merged pairs, filters, transposes back, stores all 16 bytes.
#[cfg(target_arch = "x86_64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn lpf_vertical_14_impl_v3(
    token: Desktop64,
    buf: &mut [u8],
    off: usize,
    pitch: usize,
    t: LfThresh,
) {
    let blimit16 = _mm_set1_epi16(t.mblim as i16);
    let limit = _mm_set1_epi8(t.lim as i8);
    let thresh = _mm_set1_epi8(t.hev_thr as i8);

    let mut x = [_mm_setzero_si128(); 4];
    for (i, v) in x.iter_mut().enumerate() {
        let b = off - 8 + i * pitch;
        let r: &[u8; 16] = buf[b..b + 16].try_into().unwrap();
        *v = _mm_loadu_si128(r);
    }
    let qp8 = transpose_pq_14_v3(token, &x);
    let mut qp: [__m128i; 7] = qp8[..7].try_into().unwrap();
    lpf_internal_14_v3(token, &mut qp, blimit16, limit, thresh);
    // C passes q7p7..q0p0 to the inverse transpose.
    let inv_in = [qp8[7], qp[6], qp[5], qp[4], qp[3], qp[2], qp[1], qp[0]];
    let pq = transpose_pq_14_inv_v3(token, &inv_in);
    for (i, v) in pq.iter().enumerate() {
        let b = off - 8 + i * pitch;
        let d: &mut [u8; 16] = (&mut buf[b..b + 16]).try_into().unwrap();
        _mm_storeu_si128(d, *v);
    }
}

// =============================================================================
// AVX2 (v3) 6/8-tap kernels — ports of `ASM_SSE2/dlf_intrin_sse2.c`
// `svt_aom_lpf_{horizontal,vertical}_{6,8}_sse2` + their shared
// `lpf_internal_{6,8}_sse2` / `filter4_sse2` / `transpose{6x6,8x8}` helpers.
// Same dispatch story as the 14-tap kernels above.
//
// Bounds: lpf6 only runs where `lpf_params` measured BOTH adjacent blocks
// at >=4 px and lpf8 where both are >=8 px, so the s-3..s+2 / s-4..s+3
// windows are always inside the plane. The vertical kernels' 8-byte row
// loads read up to 2 bytes past the 6-tap window (s+4) — the same bytes
// C's `loadl_epi64` reads on the same padded plane.
// =============================================================================

/// C `filter4_sse2` (dlf_intrin_sse2.c:176): the narrow filter shared by
/// the 6- and 8-tap kernels. Takes the 8-byte merged pairs (`[p0|p1]`,
/// `[q0|q1]`) and replicated hev/mask; returns (qs1qs0, ps1ps0).
#[cfg(target_arch = "x86_64")]
#[rite]
fn filter4_68_v3(
    _t: Desktop64,
    p1p0: __m128i,
    q1q0: __m128i,
    hev: __m128i,
    mask: __m128i,
) -> (__m128i, __m128i) {
    let t3t4 = _mm_set_epi8(3, 3, 3, 3, 3, 3, 3, 3, 4, 4, 4, 4, 4, 4, 4, 4);
    let t80 = _mm_set1_epi8(0x80u8 as i8);
    let ff = _mm_cmpeq_epi8(t80, t80);

    let ps1ps0_work = _mm_xor_si128(p1p0, t80);
    let mut qs1qs0_work = _mm_xor_si128(q1q0, t80);

    // filter = signed_char_clamp(ps1 - qs1) & hev
    let work = _mm_subs_epi8(ps1ps0_work, qs1qs0_work);
    let mut filter = _mm_and_si128(_mm_srli_si128::<8>(work), hev);
    // filter = signed_char_clamp(filter + 3 * (qs0 - ps0)) & mask
    filter = _mm_subs_epi8(filter, work);
    filter = _mm_subs_epi8(filter, work);
    filter = _mm_subs_epi8(filter, work);
    filter = _mm_and_si128(filter, mask);
    filter = _mm_unpacklo_epi64(filter, filter);

    // filter1 = signed_char_clamp(filter + 4) >> 3 (low 8 bytes);
    // filter2 = signed_char_clamp(filter + 3) >> 3 (high 8 bytes)
    let f21 = _mm_adds_epi8(filter, t3t4);
    let f2_16 = _mm_srai_epi16::<11>(_mm_unpackhi_epi8(f21, f21));
    let f1_16 = _mm_srai_epi16::<11>(_mm_unpacklo_epi8(f21, f21));
    let f21 = _mm_packs_epi16(f1_16, f2_16); // [f1 | f2]

    // filter = ROUND_POWER_OF_TWO(filter1, 1) & ~hev
    let mut filter1p1 = _mm_subs_epi8(f21, ff); // [f1+1 | f2+1]
    filter1p1 = _mm_unpacklo_epi8(filter1p1, filter1p1);
    filter1p1 = _mm_srai_epi16::<9>(filter1p1);
    filter1p1 = _mm_packs_epi16(filter1p1, filter1p1); // [r1 | r1]
    filter1p1 = _mm_andnot_si128(hev, filter1p1); // [r1' | r1']

    // C reuses `hev` as the addend carrier [f2 | r1'].
    let ps_add = _mm_unpackhi_epi64(f21, filter1p1);
    let qs_sub = _mm_unpacklo_epi64(f21, filter1p1); // [f1 | r1']

    qs1qs0_work = _mm_subs_epi8(qs1qs0_work, qs_sub);
    let ps1ps0_out = _mm_adds_epi8(ps1ps0_work, ps_add);

    (
        _mm_xor_si128(qs1qs0_work, t80),
        _mm_xor_si128(ps1ps0_out, t80),
    )
}

/// Shared mask/hev prologue for the 6/8-tap internals: the merged pairs
/// are 8-byte `[p|q]` groups, so folds use `srli 8` and replication uses
/// `unpacklo_epi64`. `q3p3` is the outer pair for the 8-tap; the 6-tap
/// caller passes `q2p2` again so its `abs_diff` contributes zero.
/// Returns (mask, hev, abs_p1p0).
#[cfg(target_arch = "x86_64")]
#[rite]
fn mask_hev_68_v3(
    token: Desktop64,
    q2p2: __m128i,
    q3p3: __m128i,
    q1p1: __m128i,
    q0p0: __m128i,
    p1q1: __m128i,
    p0q0: __m128i,
    blimit16: __m128i,
    limit: __m128i,
    thresh: __m128i,
) -> (__m128i, __m128i, __m128i) {
    let zero = _mm_setzero_si128();
    let fe = _mm_set1_epi8(0xfeu8 as i8);
    let ff = _mm_cmpeq_epi8(fe, fe);

    let abs_p1p0 = lpf_abs_diff_v3(token, q1p1, q0p0);
    let abs_q1q0 = _mm_srli_si128::<8>(abs_p1p0);
    let abs_p0q0 = lpf_abs_diff_v3(token, q0p0, p0q0);
    let abs_p1q1 = lpf_abs_diff_v3(token, q1p1, p1q1);

    let flat_a = _mm_max_epu8(abs_p1p0, abs_q1q0);
    let mut hev = _mm_subs_epu8(flat_a, thresh);
    hev = _mm_xor_si128(_mm_cmpeq_epi8(hev, zero), ff);
    hev = _mm_unpacklo_epi64(hev, hev);

    // The sum>blimit "don't filter" flag in u16 — same reasoning as the
    // 14-tap kernel: _sse2-exact on all reachable inputs (mblim <= 193),
    // _c-exact everywhere.
    let a16 = _mm_unpacklo_epi8(abs_p0q0, zero);
    let b8 = _mm_srli_epi16::<1>(_mm_and_si128(abs_p1q1, fe));
    let s16 = _mm_add_epi16(_mm_add_epi16(a16, a16), _mm_unpacklo_epi8(b8, zero));
    let flag = _mm_packs_epi16(
        _mm_cmpgt_epi16(s16, blimit16),
        _mm_cmpgt_epi16(s16, blimit16),
    );

    let work = _mm_max_epu8(
        lpf_abs_diff_v3(token, q2p2, q1p1),
        lpf_abs_diff_v3(token, q3p3, q2p2),
    );
    let mut mask = _mm_max_epu8(abs_p1p0, work);
    mask = _mm_max_epu8(mask, _mm_srli_si128::<8>(mask));
    mask = _mm_subs_epu8(mask, limit);
    let mask = _mm_andnot_si128(flag, _mm_cmpeq_epi8(mask, zero));
    let mask = _mm_unpacklo_epi64(mask, mask);
    (mask, hev, abs_p1p0)
}

/// C `lpf_internal_8_sse2` (dlf_intrin_sse2.c:784). `p[i]`/`q[i]` are the
/// p{i}/q{i} row vectors (4-byte horizontal rows or 8-byte transposed
/// columns; the upper bytes may hold the neighbour vector, only the low
/// 8-byte group is consumed). Returns (q1q0, p1p0, p2, q2).
#[cfg(target_arch = "x86_64")]
#[rite]
fn lpf_internal_8_v3(
    token: Desktop64,
    p: &[__m128i; 4],
    q: &[__m128i; 4],
    blimit16: __m128i,
    limit: __m128i,
    thresh: __m128i,
) -> (__m128i, __m128i, __m128i, __m128i) {
    let zero = _mm_setzero_si128();
    let one = _mm_set1_epi8(1);

    let q3p3 = _mm_unpacklo_epi64(p[3], q[3]);
    let q2p2 = _mm_unpacklo_epi64(p[2], q[2]);
    let q1p1 = _mm_unpacklo_epi64(p[1], q[1]);
    let q0p0 = _mm_unpacklo_epi64(p[0], q[0]);

    let p1q1 = _mm_shuffle_epi32::<0x4e>(q1p1);
    let p0q0 = _mm_shuffle_epi32::<0x4e>(q0p0);

    let (mask, hev, abs_p1p0) = mask_hev_68_v3(
        token, q2p2, q3p3, q1p1, q0p0, p1q1, p0q0, blimit16, limit, thresh,
    );

    // flat_mask4
    let mut flat = _mm_max_epu8(
        lpf_abs_diff_v3(token, q2p2, q0p0),
        lpf_abs_diff_v3(token, q3p3, q0p0),
    );
    flat = _mm_max_epu8(abs_p1p0, flat);
    flat = _mm_max_epu8(flat, _mm_srli_si128::<8>(flat));
    flat = _mm_subs_epu8(flat, one);
    flat = _mm_cmpeq_epi8(flat, zero);
    flat = _mm_and_si128(flat, mask);
    flat = _mm_unpacklo_epi64(flat, flat);

    // filter8 — the 7-tap wide filter (dlf_intrin_sse2.c:852-896)
    let four = _mm_set1_epi16(4);
    let p2_16 = _mm_unpacklo_epi8(p[2], zero);
    let p1_16 = _mm_unpacklo_epi8(p[1], zero);
    let p0_16 = _mm_unpacklo_epi8(p[0], zero);
    let q0_16 = _mm_unpacklo_epi8(q[0], zero);
    let q1_16 = _mm_unpacklo_epi8(q[1], zero);
    let q2_16 = _mm_unpacklo_epi8(q[2], zero);
    let p3_16 = _mm_unpacklo_epi8(p[3], zero);
    let q3_16 = _mm_unpacklo_epi8(q[3], zero);

    // op2
    let mut workp_a = _mm_add_epi16(_mm_add_epi16(p3_16, p3_16), _mm_add_epi16(p2_16, p1_16));
    workp_a = _mm_add_epi16(_mm_add_epi16(workp_a, four), p0_16);
    let mut workp_b = _mm_add_epi16(_mm_add_epi16(q0_16, p2_16), p3_16);
    let s = _mm_srli_epi16::<3>(_mm_add_epi16(workp_a, workp_b));
    let op2 = _mm_packus_epi16(s, s);

    // op1
    workp_b = _mm_add_epi16(_mm_add_epi16(q0_16, q1_16), p1_16);
    let op1 = _mm_srli_epi16::<3>(_mm_add_epi16(workp_a, workp_b));

    // op0
    workp_a = _mm_add_epi16(_mm_sub_epi16(workp_a, p3_16), q2_16);
    workp_b = _mm_add_epi16(_mm_sub_epi16(workp_b, p1_16), p0_16);
    let op0 = _mm_srli_epi16::<3>(_mm_add_epi16(workp_a, workp_b));
    let flat_p1p0 = _mm_packus_epi16(op0, op1); // [op0 | op1]

    // oq0
    workp_a = _mm_add_epi16(_mm_sub_epi16(workp_a, p3_16), q3_16);
    workp_b = _mm_add_epi16(_mm_sub_epi16(workp_b, p0_16), q0_16);
    let oq0 = _mm_srli_epi16::<3>(_mm_add_epi16(workp_a, workp_b));

    // oq1
    workp_a = _mm_add_epi16(_mm_sub_epi16(workp_a, p2_16), q3_16);
    workp_b = _mm_add_epi16(_mm_sub_epi16(workp_b, q0_16), q1_16);
    let oq1 = _mm_srli_epi16::<3>(_mm_add_epi16(workp_a, workp_b));
    let flat_q0q1 = _mm_packus_epi16(oq0, oq1); // [oq0 | oq1]

    // oq2
    workp_a = _mm_add_epi16(_mm_sub_epi16(workp_a, p1_16), q3_16);
    workp_b = _mm_add_epi16(_mm_sub_epi16(workp_b, q1_16), q2_16);
    let s = _mm_srli_epi16::<3>(_mm_add_epi16(workp_a, workp_b));
    let oq2 = _mm_packus_epi16(s, s);

    // lp filter
    let p1p0 = _mm_unpacklo_epi64(q0p0, q1p1);
    let q1q0 = _mm_unpackhi_epi64(q0p0, q1p1);
    let (qs1qs0, ps1ps0) = filter4_68_v3(token, p1p0, q1q0, hev, mask);

    let q1q0_out = _mm_or_si128(
        _mm_andnot_si128(flat, qs1qs0),
        _mm_and_si128(flat, flat_q0q1),
    );
    let p1p0_out = _mm_or_si128(
        _mm_andnot_si128(flat, ps1ps0),
        _mm_and_si128(flat, flat_p1p0),
    );
    let q2_out = _mm_or_si128(_mm_andnot_si128(flat, q[2]), _mm_and_si128(flat, oq2));
    let p2_out = _mm_or_si128(_mm_andnot_si128(flat, p[2]), _mm_and_si128(flat, op2));
    (q1q0_out, p1p0_out, p2_out, q2_out)
}

/// C `lpf_internal_6_sse2` (dlf_intrin_sse2.c:632). Returns (q1q0, p1p0).
#[cfg(target_arch = "x86_64")]
#[rite]
fn lpf_internal_6_v3(
    token: Desktop64,
    p: &[__m128i; 3],
    q: &[__m128i; 3],
    blimit16: __m128i,
    limit: __m128i,
    thresh: __m128i,
) -> (__m128i, __m128i) {
    let zero = _mm_setzero_si128();
    let one = _mm_set1_epi8(1);

    let q2p2 = _mm_unpacklo_epi64(p[2], q[2]);
    let q1p1 = _mm_unpacklo_epi64(p[1], q[1]);
    let q0p0 = _mm_unpacklo_epi64(p[0], q[0]);

    let p1q1 = _mm_shuffle_epi32::<0x4e>(q1p1);
    let p0q0 = _mm_shuffle_epi32::<0x4e>(q0p0);

    let (mask, hev, abs_p1p0) = mask_hev_68_v3(
        token, q2p2, q2p2, q1p1, q0p0, p1q1, p0q0, blimit16, limit, thresh,
    );

    // flat_mask (flat_mask3)
    let mut flat = _mm_max_epu8(lpf_abs_diff_v3(token, q2p2, q0p0), abs_p1p0);
    flat = _mm_max_epu8(flat, _mm_srli_si128::<8>(flat));
    flat = _mm_subs_epu8(flat, one);
    flat = _mm_cmpeq_epi8(flat, zero);
    flat = _mm_and_si128(flat, mask);
    flat = _mm_unpacklo_epi64(flat, flat);

    // 5-tap filter (dlf_intrin_sse2.c:706-742)
    let four = _mm_set1_epi16(4);
    let p2_16 = _mm_unpacklo_epi8(p[2], zero);
    let p1_16 = _mm_unpacklo_epi8(p[1], zero);
    let p0_16 = _mm_unpacklo_epi8(p[0], zero);
    let q0_16 = _mm_unpacklo_epi8(q[0], zero);
    let q1_16 = _mm_unpacklo_epi8(q[1], zero);
    let q2_16 = _mm_unpacklo_epi8(q[2], zero);

    // op1
    let mut workp_a = _mm_add_epi16(_mm_add_epi16(p0_16, p0_16), _mm_add_epi16(p1_16, p1_16));
    workp_a = _mm_add_epi16(_mm_add_epi16(workp_a, four), p2_16);
    let mut workp_b = _mm_add_epi16(_mm_add_epi16(p2_16, p2_16), q0_16);
    let op1 = _mm_srli_epi16::<3>(_mm_add_epi16(workp_a, workp_b));

    // op0
    workp_b = _mm_add_epi16(_mm_add_epi16(q0_16, q0_16), q1_16);
    workp_a = _mm_add_epi16(workp_a, workp_b);
    let op0 = _mm_srli_epi16::<3>(workp_a);
    let flat_p1p0 = _mm_packus_epi16(op0, op1); // [op0 | op1]

    // oq0
    workp_a = _mm_sub_epi16(_mm_sub_epi16(workp_a, p2_16), p1_16);
    workp_b = _mm_add_epi16(q1_16, q2_16);
    workp_a = _mm_add_epi16(workp_a, workp_b);
    let oq0 = _mm_srli_epi16::<3>(workp_a);

    // oq1
    workp_a = _mm_sub_epi16(_mm_sub_epi16(workp_a, p1_16), p0_16);
    workp_b = _mm_add_epi16(q2_16, q2_16);
    let oq1 = _mm_srli_epi16::<3>(_mm_add_epi16(workp_a, workp_b));
    let flat_q0q1 = _mm_packus_epi16(oq0, oq1); // [oq0 | oq1]

    // lp filter
    let p1p0 = _mm_unpacklo_epi64(q0p0, q1p1);
    let q1q0 = _mm_unpackhi_epi64(q0p0, q1p1);
    let (qs1qs0, ps1ps0) = filter4_68_v3(token, p1p0, q1q0, hev, mask);

    let q1q0_out = _mm_or_si128(
        _mm_andnot_si128(flat, qs1qs0),
        _mm_and_si128(flat, flat_q0q1),
    );
    let p1p0_out = _mm_or_si128(
        _mm_andnot_si128(flat, ps1ps0),
        _mm_and_si128(flat, flat_p1p0),
    );
    (q1q0_out, p1p0_out)
}

/// C `svt_aom_lpf_horizontal_6_sse2` — 4 columns, 4-byte row loads.
#[cfg(target_arch = "x86_64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn lpf_horizontal_6_impl_v3(
    token: Desktop64,
    buf: &mut [u8],
    off: usize,
    pitch: usize,
    t: LfThresh,
) {
    let blimit16 = _mm_set1_epi16(t.mblim as i16);
    let limit = _mm_set1_epi8(t.lim as i8);
    let thresh = _mm_set1_epi8(t.hev_thr as i8);

    let row = |k: isize| -> __m128i {
        let i = (off as isize + k * pitch as isize) as usize;
        let r: &[u8; 4] = buf[i..i + 4].try_into().unwrap();
        _mm_loadu_si32(r)
    };
    let p = [row(-1), row(-2), row(-3)];
    let q = [row(0), row(1), row(2)];
    let (q1q0, p1p0) = lpf_internal_6_v3(token, &p, &q, blimit16, limit, thresh);

    let st = |buf: &mut [u8], k: isize, v: __m128i| {
        let i = (off as isize + k * pitch as isize) as usize;
        let d: &mut [u8; 4] = (&mut buf[i..i + 4]).try_into().unwrap();
        _mm_storeu_si32(d, v);
    };
    st(buf, -1, p1p0);
    st(buf, -2, _mm_srli_si128::<8>(p1p0));
    st(buf, 0, q1q0);
    st(buf, 1, _mm_srli_si128::<8>(q1q0));
}

/// C `svt_aom_lpf_horizontal_8_sse2` — 4 columns, 4-byte row loads.
#[cfg(target_arch = "x86_64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn lpf_horizontal_8_impl_v3(
    token: Desktop64,
    buf: &mut [u8],
    off: usize,
    pitch: usize,
    t: LfThresh,
) {
    let blimit16 = _mm_set1_epi16(t.mblim as i16);
    let limit = _mm_set1_epi8(t.lim as i8);
    let thresh = _mm_set1_epi8(t.hev_thr as i8);

    let row = |k: isize| -> __m128i {
        let i = (off as isize + k * pitch as isize) as usize;
        let r: &[u8; 4] = buf[i..i + 4].try_into().unwrap();
        _mm_loadu_si32(r)
    };
    let p = [row(-1), row(-2), row(-3), row(-4)];
    let q = [row(0), row(1), row(2), row(3)];
    let (q1q0, p1p0, p2, q2) = lpf_internal_8_v3(token, &p, &q, blimit16, limit, thresh);

    let st = |buf: &mut [u8], k: isize, v: __m128i| {
        let i = (off as isize + k * pitch as isize) as usize;
        let d: &mut [u8; 4] = (&mut buf[i..i + 4]).try_into().unwrap();
        _mm_storeu_si32(d, v);
    };
    st(buf, -1, p1p0);
    st(buf, -2, _mm_srli_si128::<8>(p1p0));
    st(buf, 0, q1q0);
    st(buf, 1, _mm_srli_si128::<8>(q1q0));
    st(buf, -3, p2);
    st(buf, 2, q2);
}

/// C `transpose6x6_sse2` (dlf_intrin_sse2.c:962): six rows in (rows 4-5
/// are zero on the forward call), three column-pair vectors out. The
/// same routine serves as the inverse: feeding the tap vectors back
/// yields the row pairs.
#[cfg(target_arch = "x86_64")]
#[rite]
fn transpose6x6_v3(_t: Desktop64, x: &[__m128i; 6]) -> [__m128i; 3] {
    let w0 = _mm_unpacklo_epi8(x[0], x[1]);
    let w1 = _mm_unpacklo_epi8(x[2], x[3]);
    let w2 = _mm_unpacklo_epi8(x[4], x[5]);

    let w4 = _mm_unpacklo_epi16(w0, w1);
    let w5 = _mm_unpacklo_epi16(w2, w0);
    let d0d1 = _mm_unpacklo_epi32(w4, w5);
    let d2d3 = _mm_unpackhi_epi32(w4, w5);

    let w4 = _mm_unpackhi_epi16(w0, w1);
    let w5 = _mm_unpackhi_epi16(w2, x[3]);
    let d4d5 = _mm_unpacklo_epi32(w4, w5);
    [d0d1, d2d3, d4d5]
}

/// C `transpose8x8_sse2` (dlf_intrin_sse2.c:993): eight rows in (rows
/// 4-7 are zero on the forward call), four column-pair vectors out.
/// Doubles as the inverse.
#[cfg(target_arch = "x86_64")]
#[rite]
fn transpose8x8_v3(_t: Desktop64, x: &[__m128i; 8]) -> [__m128i; 4] {
    let w0 = _mm_unpacklo_epi8(x[0], x[1]);
    let w1 = _mm_unpacklo_epi8(x[2], x[3]);
    let w2 = _mm_unpacklo_epi8(x[4], x[5]);
    let w3 = _mm_unpacklo_epi8(x[6], x[7]);

    let w4 = _mm_unpacklo_epi16(w0, w1);
    let w5 = _mm_unpacklo_epi16(w2, w3);
    let d0d1 = _mm_unpacklo_epi32(w4, w5);
    let d2d3 = _mm_unpackhi_epi32(w4, w5);

    let w6 = _mm_unpackhi_epi16(w0, w1);
    let w7 = _mm_unpackhi_epi16(w2, w3);
    let d4d5 = _mm_unpacklo_epi32(w6, w7);
    let d6d7 = _mm_unpackhi_epi32(w6, w7);
    [d0d1, d2d3, d4d5, d6d7]
}

/// Eight bytes at `buf[i..]`, zero-filled past the end of `buf`.
///
/// C's `svt_aom_lpf_vertical_6_sse2` loads 8 bytes per row at `s-3` and its
/// transpose consumes only the low 6 — the filter touches `s-3..=s+2`. C's
/// frames are padded, ours are not: on the last row of a plane whose buffer
/// ends right after `s+2`, the top 2 bytes of that load lie past the end and
/// the direct slice panicked (zenavif `svt_rs_partial_sb_roundtrip_at_low_presets`,
/// 100x37 at speed 4, 2026-09-24). Zero-filling them is output-identical:
/// those lanes never reach a filtered or stored pixel.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
#[inline]
fn load8_zero_padded(buf: &[u8], i: usize) -> [u8; 8] {
    match buf.get(i..i + 8) {
        Some(r) => r.try_into().unwrap(),
        None => {
            let tail = &buf[i..];
            let mut r = [0u8; 8];
            r[..tail.len()].copy_from_slice(tail);
            r
        }
    }
}

/// C `svt_aom_lpf_vertical_6_sse2` — 4 rows. Loads 8-byte rows at s-3
/// (the transpose consumes the low 6 bytes), transposes, filters,
/// transposes back, and stores 6 bytes per row.
#[cfg(target_arch = "x86_64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn lpf_vertical_6_impl_v3(token: Desktop64, buf: &mut [u8], off: usize, pitch: usize, t: LfThresh) {
    let blimit16 = _mm_set1_epi16(t.mblim as i16);
    let limit = _mm_set1_epi8(t.lim as i8);
    let thresh = _mm_set1_epi8(t.hev_thr as i8);

    let zero = _mm_setzero_si128();
    let row = |k: usize| -> __m128i {
        let r = load8_zero_padded(buf, off - 3 + k * pitch);
        _mm_loadu_si64(&r)
    };
    let x = [row(0), row(1), row(2), row(3), zero, zero];
    let [d0d1, d2d3, d4d5] = transpose6x6_v3(token, &x);
    let d1 = _mm_srli_si128::<8>(d0d1);
    let d3 = _mm_srli_si128::<8>(d2d3);
    let d5 = _mm_srli_si128::<8>(d4d5);

    // C: lpf_internal_6(&d0d1, &d5, &d1, &d4d5, &d2d3, &d3, ...)
    let p = [d2d3, d1, d0d1];
    let q = [d3, d4d5, d5];
    let (q1q0, p1p0) = lpf_internal_6_v3(token, &p, &q, blimit16, limit, thresh);

    let p0 = _mm_srli_si128::<8>(p1p0);
    let q0 = _mm_srli_si128::<8>(q1q0);
    // C: transpose6x6(&d0d1, &p0, &p1p0, &q1q0, &q0, &d5, ...)
    let xi = [d0d1, p0, p1p0, q1q0, q0, d5];
    let [o0, o2, _o4] = transpose6x6_v3(token, &xi);

    // C stores 6 bytes per row via a 16-byte temp + memcpy.
    let mut tmp = [0u8; 8];
    for (r, v) in [o0, _mm_srli_si128::<8>(o0), o2, _mm_srli_si128::<8>(o2)]
        .iter()
        .enumerate()
    {
        {
            let d: &mut [u8; 8] = &mut tmp;
            _mm_storeu_si64(d, *v);
        }
        let b = off - 3 + r * pitch;
        buf[b..b + 6].copy_from_slice(&tmp[0..6]);
    }
}

/// C `svt_aom_lpf_vertical_8_sse2` — 4 rows. Loads 8-byte rows at s-4,
/// transposes, filters, transposes back, stores 8 bytes per row.
#[cfg(target_arch = "x86_64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn lpf_vertical_8_impl_v3(token: Desktop64, buf: &mut [u8], off: usize, pitch: usize, t: LfThresh) {
    let blimit16 = _mm_set1_epi16(t.mblim as i16);
    let limit = _mm_set1_epi8(t.lim as i8);
    let thresh = _mm_set1_epi8(t.hev_thr as i8);

    let zero = _mm_setzero_si128();
    let row = |k: usize| -> __m128i {
        let i = off - 4 + k * pitch;
        let r: &[u8; 8] = buf[i..i + 8].try_into().unwrap();
        _mm_loadu_si64(r)
    };
    let x = [row(0), row(1), row(2), row(3), zero, zero, zero, zero];
    let [d0d1, d2d3, d4d5, d6d7] = transpose8x8_v3(token, &x);
    let d1 = _mm_srli_si128::<8>(d0d1);
    let d3 = _mm_srli_si128::<8>(d2d3);
    let d5 = _mm_srli_si128::<8>(d4d5);
    let d7 = _mm_srli_si128::<8>(d6d7);

    // C: lpf_internal_8(&d0d1, &d7, &d1, &d6d7, &d2d3, &d5, &d3, &d4d5, ...)
    let p = [d3, d2d3, d1, d0d1];
    let q = [d4d5, d5, d6d7, d7];
    let (q1q0, p1p0, p2, q2) = lpf_internal_8_v3(token, &p, &q, blimit16, limit, thresh);

    let p0 = _mm_srli_si128::<8>(p1p0);
    let q0 = _mm_srli_si128::<8>(q1q0);
    // C: transpose8x8(&d0d1, &p2, &p0, &p1p0, &q1q0, &q0, &q2, &d7, ...)
    let xi = [d0d1, p2, p0, p1p0, q1q0, q0, q2, d7];
    let [o0, o2, _o4, _o6] = transpose8x8_v3(token, &xi);

    let st = |buf: &mut [u8], k: usize, v: __m128i| {
        let i = off - 4 + k * pitch;
        let d: &mut [u8; 8] = (&mut buf[i..i + 8]).try_into().unwrap();
        _mm_storeu_si64(d, v);
    };
    st(buf, 0, o0);
    st(buf, 1, _mm_srli_si128::<8>(o0));
    st(buf, 2, o2);
    st(buf, 3, _mm_srli_si128::<8>(o2));
}

// =============================================================================
// AArch64 (neon) 6/8/14-tap kernels — transliterations of the AVX2 (v3)
// arms above, which are themselves ports of `ASM_SSE2/dlf_intrin_sse2.c`.
// Same merged-pair vector shapes, same register-level algorithm; every op
// maps to the exact NEON equivalent (`subs_epu8` -> `vqsubq_u8`,
// `unpacklo_*` -> `vzip1q_*`, `srli_si128` -> `vextq` against zero,
// `packs` -> `vqmovn_s16`, `packus` -> `vqmovun_s16`, `andnot` -> `vbicq`,
// `movemask != 0xffff` -> `vminvq_u8 == 0` on the {0,0xFF} mask).
//
// Loads mirror the SSE2 width semantics exactly: 4-byte rows are
// zero-extended to 16 bytes (`_mm_loadu_si32`), 8-byte rows to 16
// (`_mm_loadu_si64`) — the upper lanes feed max/sum folds, so they must be
// zero just as they are on x86.
// =============================================================================

/// `_mm_loadu_si32` equivalent: 4 bytes in lane 0, zeros above.
#[cfg(target_arch = "aarch64")]
#[rite]
fn lpf_ld4_neon(_t: NeonToken, r: &[u8; 4]) -> uint8x16_t {
    vreinterpretq_u8_u32(vsetq_lane_u32::<0>(u32::from_le_bytes(*r), vdupq_n_u32(0)))
}

/// `_mm_storeu_si32` equivalent: low 4 bytes.
#[cfg(target_arch = "aarch64")]
#[rite]
fn lpf_st4_neon(_t: NeonToken, d: &mut [u8; 4], v: uint8x16_t) {
    d.copy_from_slice(&vgetq_lane_u32::<0>(vreinterpretq_u32_u8(v)).to_le_bytes());
}

/// `_mm_loadu_si64` equivalent: 8 bytes, zeros above.
#[cfg(target_arch = "aarch64")]
#[rite]
fn lpf_ld8_neon(_t: NeonToken, r: &[u8; 8]) -> uint8x16_t {
    vcombine_u8(vld1_u8(r), vdup_n_u8(0))
}

/// `_mm_srli_si128::<N>` equivalent: whole-register byte shift right.
#[cfg(target_arch = "aarch64")]
#[rite]
fn lpf_bsrl_neon<const N: i32>(_t: NeonToken, v: uint8x16_t) -> uint8x16_t {
    vextq_u8::<N>(v, vdupq_n_u8(0))
}

/// `_mm_slli_si128::<4>` equivalent.
#[cfg(target_arch = "aarch64")]
#[rite]
fn lpf_bsll4_neon(_t: NeonToken, v: uint8x16_t) -> uint8x16_t {
    vextq_u8::<12>(vdupq_n_u8(0), v)
}

/// `_mm_unpacklo_epi8(a, b)` on byte vectors.
#[cfg(target_arch = "aarch64")]
#[rite]
fn lpf_ziplo8_neon(_t: NeonToken, a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    vzip1q_u8(a, b)
}

/// `_mm_unpackhi_epi8(a, b)`.
#[cfg(target_arch = "aarch64")]
#[rite]
fn lpf_ziphi8_neon(_t: NeonToken, a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    vzip2q_u8(a, b)
}

/// `_mm_unpacklo_epi64` — interleave the low u64 lane of each input.
#[cfg(target_arch = "aarch64")]
#[rite]
fn lpf_ziplo64_neon(_t: NeonToken, a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    vreinterpretq_u8_u64(vzip1q_u64(vreinterpretq_u64_u8(a), vreinterpretq_u64_u8(b)))
}

/// `_mm_unpackhi_epi64`.
#[cfg(target_arch = "aarch64")]
#[rite]
fn lpf_ziphi64_neon(_t: NeonToken, a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    vreinterpretq_u8_u64(vzip2q_u64(vreinterpretq_u64_u8(a), vreinterpretq_u64_u8(b)))
}

/// `_mm_unpacklo_epi32`.
#[cfg(target_arch = "aarch64")]
#[rite]
fn lpf_ziplo32_neon(_t: NeonToken, a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    vreinterpretq_u8_u32(vzip1q_u32(vreinterpretq_u32_u8(a), vreinterpretq_u32_u8(b)))
}

/// `_mm_unpackhi_epi32`.
#[cfg(target_arch = "aarch64")]
#[rite]
fn lpf_ziphi32_neon(_t: NeonToken, a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    vreinterpretq_u8_u32(vzip2q_u32(vreinterpretq_u32_u8(a), vreinterpretq_u32_u8(b)))
}

/// `_mm_unpacklo_epi8(v, 0)` — widen the low 8 bytes to u16.
#[cfg(target_arch = "aarch64")]
#[rite]
fn lpf_widen_neon(_t: NeonToken, v: uint8x16_t) -> uint16x8_t {
    vmovl_u8(vget_low_u8(v))
}

/// `_mm_shuffle_epi32::<0x4e>` — swap the two u64 halves.
#[cfg(target_arch = "aarch64")]
#[rite]
fn lpf_swap64_neon(_t: NeonToken, v: uint8x16_t) -> uint8x16_t {
    vextq_u8::<8>(v, v)
}

/// C `abs_diff`: |a - b| per u8 lane. One `vabdq_u8` — exact.
#[cfg(target_arch = "aarch64")]
#[rite]
fn lpf_abs_diff_neon(_t: NeonToken, a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    vabdq_u8(a, b)
}

/// `_mm_packs_epi16(a, a)` — saturating i16 -> i8 narrow, duplicated.
#[cfg(target_arch = "aarch64")]
#[rite]
fn lpf_packs_dup_neon(_t: NeonToken, v: int16x8_t) -> int8x16_t {
    vcombine_s8(vqmovn_s16(v), vqmovn_s16(v))
}

/// `_mm_packus_epi16(a, a)` — saturating i16 -> u8 narrow, duplicated.
#[cfg(target_arch = "aarch64")]
#[rite]
fn lpf_packus_dup_neon(_t: NeonToken, v: int16x8_t) -> uint8x16_t {
    vcombine_u8(vqmovun_s16(v), vqmovun_s16(v))
}

/// C `filter4_sse2` (dlf_intrin_sse2.c:176): the narrow filter shared by
/// the 6- and 8-tap kernels. All arithmetic is in the signed domain after
/// the 0x80 flip — the vector values are carried as int8x16_t.
#[cfg(target_arch = "aarch64")]
#[rite]
fn filter4_68_neon(
    token: NeonToken,
    p1p0: uint8x16_t,
    q1q0: uint8x16_t,
    hev: uint8x16_t,
    mask: uint8x16_t,
) -> (uint8x16_t, uint8x16_t) {
    // _mm_set_epi8(3 x8, 4 x8) — low lanes 4, high lanes 3.
    let t3t4: &[i8; 16] = &[4, 4, 4, 4, 4, 4, 4, 4, 3, 3, 3, 3, 3, 3, 3, 3];
    let t3t4 = vld1q_s8(t3t4);
    let t80 = vdupq_n_u8(0x80);
    let ff = vdupq_n_s8(-1);

    let ps1ps0_work = vreinterpretq_s8_u8(veorq_u8(p1p0, t80));
    let mut qs1qs0_work = vreinterpretq_s8_u8(veorq_u8(q1q0, t80));

    // filter = signed_char_clamp(ps1 - qs1) & hev
    let work = vqsubq_s8(ps1ps0_work, qs1qs0_work);
    let hev_s = vreinterpretq_s8_u8(hev);
    let mut filter = vandq_s8(
        vreinterpretq_s8_u8(lpf_bsrl_neon::<8>(token, vreinterpretq_u8_s8(work))),
        hev_s,
    );
    // filter = signed_char_clamp(filter + 3 * (qs0 - ps0)) & mask
    filter = vqsubq_s8(filter, work);
    filter = vqsubq_s8(filter, work);
    filter = vqsubq_s8(filter, work);
    filter = vandq_s8(filter, vreinterpretq_s8_u8(mask));
    filter = vreinterpretq_s8_u8(lpf_ziplo64_neon(
        token,
        vreinterpretq_u8_s8(filter),
        vreinterpretq_u8_s8(filter),
    ));

    // filter1 = signed_char_clamp(filter + 4) >> 3 (low 8 bytes);
    // filter2 = signed_char_clamp(filter + 3) >> 3 (high 8 bytes)
    let f21 = vqaddq_s8(filter, t3t4);
    let f2_16 = vshrq_n_s16::<11>(vreinterpretq_s16_s8(vzip2q_s8(f21, f21)));
    let f1_16 = vshrq_n_s16::<11>(vreinterpretq_s16_s8(vzip1q_s8(f21, f21)));
    let f21 = vcombine_s8(vqmovn_s16(f1_16), vqmovn_s16(f2_16)); // [f1 | f2]

    // filter = ROUND_POWER_OF_TWO(filter1, 1) & ~hev
    let mut filter1p1 = vqsubq_s8(f21, ff); // [f1+1 | f2+1]
    filter1p1 = vreinterpretq_s8_s16(vshrq_n_s16::<9>(vreinterpretq_s16_s8(vzip1q_s8(
        filter1p1, filter1p1,
    ))));
    filter1p1 = vcombine_s8(
        vqmovn_s16(vreinterpretq_s16_s8(filter1p1)),
        vqmovn_s16(vreinterpretq_s16_s8(filter1p1)),
    ); // [r1 | r1]
    filter1p1 = vreinterpretq_s8_u8(vbicq_u8(vreinterpretq_u8_s8(filter1p1), hev)); // [r1' | r1']

    // C reuses `hev` as the addend carrier [f2 | r1'].
    let ps_add = vreinterpretq_s8_u8(lpf_ziphi64_neon(
        token,
        vreinterpretq_u8_s8(f21),
        vreinterpretq_u8_s8(filter1p1),
    ));
    let qs_sub = vreinterpretq_s8_u8(lpf_ziplo64_neon(
        token,
        vreinterpretq_u8_s8(f21),
        vreinterpretq_u8_s8(filter1p1),
    )); // [f1 | r1']

    qs1qs0_work = vqsubq_s8(qs1qs0_work, qs_sub);
    let ps1ps0_out = vqaddq_s8(ps1ps0_work, ps_add);

    (
        veorq_u8(vreinterpretq_u8_s8(qs1qs0_work), t80),
        veorq_u8(vreinterpretq_u8_s8(ps1ps0_out), t80),
    )
}

/// Shared mask/hev prologue for the 6/8-tap internals — see
/// [`mask_hev_68_v3`] for the shape; identical ops on NEON.
/// Returns (mask, hev, abs_p1p0).
#[cfg(target_arch = "aarch64")]
#[rite]
#[allow(clippy::too_many_arguments)]
fn mask_hev_68_neon(
    token: NeonToken,
    q2p2: uint8x16_t,
    q3p3: uint8x16_t,
    q1p1: uint8x16_t,
    q0p0: uint8x16_t,
    p1q1: uint8x16_t,
    p0q0: uint8x16_t,
    blimit16: uint8x16_t,
    limit: uint8x16_t,
    thresh: uint8x16_t,
) -> (uint8x16_t, uint8x16_t, uint8x16_t) {
    let zero = vdupq_n_u8(0);
    let fe = vdupq_n_u8(0xfe);
    let ff = vdupq_n_u8(0xff);

    let abs_p1p0 = lpf_abs_diff_neon(token, q1p1, q0p0);
    let abs_q1q0 = lpf_bsrl_neon::<8>(token, abs_p1p0);
    let abs_p0q0 = lpf_abs_diff_neon(token, q0p0, p0q0);
    let abs_p1q1 = lpf_abs_diff_neon(token, q1p1, p1q1);

    let flat_a = vmaxq_u8(abs_p1p0, abs_q1q0);
    let mut hev = vqsubq_u8(flat_a, thresh);
    hev = veorq_u8(vceqq_u8(hev, zero), ff);
    hev = lpf_ziplo64_neon(token, hev, hev);

    // The sum>blimit "don't filter" flag in u16 — same reasoning as the
    // v3 arm: _sse2-exact on all reachable inputs (mblim <= 193),
    // _c-exact everywhere.
    let a16 = lpf_widen_neon(token, abs_p0q0);
    let b8 = vshrq_n_u16::<1>(vreinterpretq_u16_u8(vandq_u8(abs_p1q1, fe)));
    let s16 = vaddq_u16(
        vaddq_u16(a16, a16),
        lpf_widen_neon(token, vreinterpretq_u8_u16(b8)),
    );
    let cmp = vcgtq_s16(vreinterpretq_s16_u16(s16), vreinterpretq_s16_u8(blimit16));
    // `_mm_packs_epi16` — SIGNED narrow: 0xFFFF (-1) -> 0xFF, not the
    // unsigned `vqmovun` clamp to 0.
    let flag = vreinterpretq_u8_s8(lpf_packs_dup_neon(token, vreinterpretq_s16_u16(cmp)));

    let work = vmaxq_u8(
        lpf_abs_diff_neon(token, q2p2, q1p1),
        lpf_abs_diff_neon(token, q3p3, q2p2),
    );
    let mut mask = vmaxq_u8(abs_p1p0, work);
    mask = vmaxq_u8(mask, lpf_bsrl_neon::<8>(token, mask));
    mask = vqsubq_u8(mask, limit);
    let mask = vbicq_u8(vceqq_u8(mask, zero), flag);
    let mask = lpf_ziplo64_neon(token, mask, mask);
    (mask, hev, abs_p1p0)
}

/// C `lpf_internal_8_sse2` (dlf_intrin_sse2.c:784). Identical op sequence
/// to [`lpf_internal_8_v3`]; returns (q1q0, p1p0, p2, q2).
#[cfg(target_arch = "aarch64")]
#[rite]
fn lpf_internal_8_neon(
    token: NeonToken,
    p: &[uint8x16_t; 4],
    q: &[uint8x16_t; 4],
    blimit16: uint8x16_t,
    limit: uint8x16_t,
    thresh: uint8x16_t,
) -> (uint8x16_t, uint8x16_t, uint8x16_t, uint8x16_t) {
    let zero = vdupq_n_u8(0);
    let one = vdupq_n_u8(1);

    let q3p3 = lpf_ziplo64_neon(token, p[3], q[3]);
    let q2p2 = lpf_ziplo64_neon(token, p[2], q[2]);
    let q1p1 = lpf_ziplo64_neon(token, p[1], q[1]);
    let q0p0 = lpf_ziplo64_neon(token, p[0], q[0]);

    let p1q1 = lpf_swap64_neon(token, q1p1);
    let p0q0 = lpf_swap64_neon(token, q0p0);

    let (mask, hev, abs_p1p0) = mask_hev_68_neon(
        token, q2p2, q3p3, q1p1, q0p0, p1q1, p0q0, blimit16, limit, thresh,
    );

    // flat_mask4
    let mut flat = vmaxq_u8(
        lpf_abs_diff_neon(token, q2p2, q0p0),
        lpf_abs_diff_neon(token, q3p3, q0p0),
    );
    flat = vmaxq_u8(abs_p1p0, flat);
    flat = vmaxq_u8(flat, lpf_bsrl_neon::<8>(token, flat));
    flat = vqsubq_u8(flat, one);
    flat = vceqq_u8(flat, zero);
    flat = vandq_u8(flat, mask);
    flat = lpf_ziplo64_neon(token, flat, flat);

    // filter8 — the 7-tap wide filter (dlf_intrin_sse2.c:852-896)
    let four = vdupq_n_u16(4);
    let p2_16 = lpf_widen_neon(token, p[2]);
    let p1_16 = lpf_widen_neon(token, p[1]);
    let p0_16 = lpf_widen_neon(token, p[0]);
    let q0_16 = lpf_widen_neon(token, q[0]);
    let q1_16 = lpf_widen_neon(token, q[1]);
    let q2_16 = lpf_widen_neon(token, q[2]);
    let p3_16 = lpf_widen_neon(token, p[3]);
    let q3_16 = lpf_widen_neon(token, q[3]);

    // op2
    let mut workp_a = vaddq_u16(vaddq_u16(p3_16, p3_16), vaddq_u16(p2_16, p1_16));
    workp_a = vaddq_u16(vaddq_u16(workp_a, four), p0_16);
    let mut workp_b = vaddq_u16(vaddq_u16(q0_16, p2_16), p3_16);
    let s = vshrq_n_u16::<3>(vaddq_u16(workp_a, workp_b));
    let op2 = lpf_packus_dup_neon(token, vreinterpretq_s16_u16(s));

    // op1
    workp_b = vaddq_u16(vaddq_u16(q0_16, q1_16), p1_16);
    let op1 = vshrq_n_u16::<3>(vaddq_u16(workp_a, workp_b));

    // op0
    workp_a = vaddq_u16(vsubq_u16(workp_a, p3_16), q2_16);
    workp_b = vaddq_u16(vsubq_u16(workp_b, p1_16), p0_16);
    let op0 = vshrq_n_u16::<3>(vaddq_u16(workp_a, workp_b));
    let flat_p1p0 = vcombine_u8(
        vqmovun_s16(vreinterpretq_s16_u16(op0)),
        vqmovun_s16(vreinterpretq_s16_u16(op1)),
    ); // [op0 | op1]

    // oq0
    workp_a = vaddq_u16(vsubq_u16(workp_a, p3_16), q3_16);
    workp_b = vaddq_u16(vsubq_u16(workp_b, p0_16), q0_16);
    let oq0 = vshrq_n_u16::<3>(vaddq_u16(workp_a, workp_b));

    // oq1
    workp_a = vaddq_u16(vsubq_u16(workp_a, p2_16), q3_16);
    workp_b = vaddq_u16(vsubq_u16(workp_b, q0_16), q1_16);
    let oq1 = vshrq_n_u16::<3>(vaddq_u16(workp_a, workp_b));
    let flat_q0q1 = vcombine_u8(
        vqmovun_s16(vreinterpretq_s16_u16(oq0)),
        vqmovun_s16(vreinterpretq_s16_u16(oq1)),
    ); // [oq0 | oq1]

    // oq2
    workp_a = vaddq_u16(vsubq_u16(workp_a, p1_16), q3_16);
    workp_b = vaddq_u16(vsubq_u16(workp_b, q1_16), q2_16);
    let s = vshrq_n_u16::<3>(vaddq_u16(workp_a, workp_b));
    let oq2 = lpf_packus_dup_neon(token, vreinterpretq_s16_u16(s));

    // lp filter
    let p1p0 = lpf_ziplo64_neon(token, q0p0, q1p1);
    let q1q0 = lpf_ziphi64_neon(token, q0p0, q1p1);
    let (qs1qs0, ps1ps0) = filter4_68_neon(token, p1p0, q1q0, hev, mask);

    let q1q0_out = vorrq_u8(vbicq_u8(qs1qs0, flat), vandq_u8(flat, flat_q0q1));
    let p1p0_out = vorrq_u8(vbicq_u8(ps1ps0, flat), vandq_u8(flat, flat_p1p0));
    let q2_out = vorrq_u8(vbicq_u8(q[2], flat), vandq_u8(flat, oq2));
    let p2_out = vorrq_u8(vbicq_u8(p[2], flat), vandq_u8(flat, op2));
    (q1q0_out, p1p0_out, p2_out, q2_out)
}

/// C `lpf_internal_6_sse2` (dlf_intrin_sse2.c:632). Returns (q1q0, p1p0).
#[cfg(target_arch = "aarch64")]
#[rite]
fn lpf_internal_6_neon(
    token: NeonToken,
    p: &[uint8x16_t; 3],
    q: &[uint8x16_t; 3],
    blimit16: uint8x16_t,
    limit: uint8x16_t,
    thresh: uint8x16_t,
) -> (uint8x16_t, uint8x16_t) {
    let zero = vdupq_n_u8(0);
    let one = vdupq_n_u8(1);

    let q2p2 = lpf_ziplo64_neon(token, p[2], q[2]);
    let q1p1 = lpf_ziplo64_neon(token, p[1], q[1]);
    let q0p0 = lpf_ziplo64_neon(token, p[0], q[0]);

    let p1q1 = lpf_swap64_neon(token, q1p1);
    let p0q0 = lpf_swap64_neon(token, q0p0);

    let (mask, hev, abs_p1p0) = mask_hev_68_neon(
        token, q2p2, q2p2, q1p1, q0p0, p1q1, p0q0, blimit16, limit, thresh,
    );

    // flat_mask (flat_mask3)
    let mut flat = vmaxq_u8(lpf_abs_diff_neon(token, q2p2, q0p0), abs_p1p0);
    flat = vmaxq_u8(flat, lpf_bsrl_neon::<8>(token, flat));
    flat = vqsubq_u8(flat, one);
    flat = vceqq_u8(flat, zero);
    flat = vandq_u8(flat, mask);
    flat = lpf_ziplo64_neon(token, flat, flat);

    // 5-tap filter (dlf_intrin_sse2.c:706-742)
    let four = vdupq_n_u16(4);
    let p2_16 = lpf_widen_neon(token, p[2]);
    let p1_16 = lpf_widen_neon(token, p[1]);
    let p0_16 = lpf_widen_neon(token, p[0]);
    let q0_16 = lpf_widen_neon(token, q[0]);
    let q1_16 = lpf_widen_neon(token, q[1]);
    let q2_16 = lpf_widen_neon(token, q[2]);

    // op1
    let mut workp_a = vaddq_u16(vaddq_u16(p0_16, p0_16), vaddq_u16(p1_16, p1_16));
    workp_a = vaddq_u16(vaddq_u16(workp_a, four), p2_16);
    let mut workp_b = vaddq_u16(vaddq_u16(p2_16, p2_16), q0_16);
    let op1 = vshrq_n_u16::<3>(vaddq_u16(workp_a, workp_b));

    // op0
    workp_b = vaddq_u16(vaddq_u16(q0_16, q0_16), q1_16);
    workp_a = vaddq_u16(workp_a, workp_b);
    let op0 = vshrq_n_u16::<3>(workp_a);
    let flat_p1p0 = vcombine_u8(
        vqmovun_s16(vreinterpretq_s16_u16(op0)),
        vqmovun_s16(vreinterpretq_s16_u16(op1)),
    ); // [op0 | op1]

    // oq0
    workp_a = vsubq_u16(vsubq_u16(workp_a, p2_16), p1_16);
    workp_b = vaddq_u16(q1_16, q2_16);
    workp_a = vaddq_u16(workp_a, workp_b);
    let oq0 = vshrq_n_u16::<3>(workp_a);

    // oq1
    workp_a = vsubq_u16(vsubq_u16(workp_a, p1_16), p0_16);
    workp_b = vaddq_u16(q2_16, q2_16);
    let oq1 = vshrq_n_u16::<3>(vaddq_u16(workp_a, workp_b));
    let flat_q0q1 = vcombine_u8(
        vqmovun_s16(vreinterpretq_s16_u16(oq0)),
        vqmovun_s16(vreinterpretq_s16_u16(oq1)),
    ); // [oq0 | oq1]

    // lp filter
    let p1p0 = lpf_ziplo64_neon(token, q0p0, q1p1);
    let q1q0 = lpf_ziphi64_neon(token, q0p0, q1p1);
    let (qs1qs0, ps1ps0) = filter4_68_neon(token, p1p0, q1q0, hev, mask);

    let q1q0_out = vorrq_u8(vbicq_u8(qs1qs0, flat), vandq_u8(flat, flat_q0q1));
    let p1p0_out = vorrq_u8(vbicq_u8(ps1ps0, flat), vandq_u8(flat, flat_p1p0));
    (q1q0_out, p1p0_out)
}

/// C `svt_aom_lpf_horizontal_6_sse2` — 4 columns, 4-byte row loads.
#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn lpf_horizontal_6_impl_neon(
    token: NeonToken,
    buf: &mut [u8],
    off: usize,
    pitch: usize,
    t: LfThresh,
) {
    let blimit16 = vreinterpretq_u8_u16(vdupq_n_u16(t.mblim as u16));
    let limit = vdupq_n_u8(t.lim);
    let thresh = vdupq_n_u8(t.hev_thr);

    let row = |k: isize| -> uint8x16_t {
        let i = (off as isize + k * pitch as isize) as usize;
        let r: &[u8; 4] = buf[i..i + 4].try_into().unwrap();
        lpf_ld4_neon(token, r)
    };
    let p = [row(-1), row(-2), row(-3)];
    let q = [row(0), row(1), row(2)];
    let (q1q0, p1p0) = lpf_internal_6_neon(token, &p, &q, blimit16, limit, thresh);

    let st = |buf: &mut [u8], k: isize, v: uint8x16_t| {
        let i = (off as isize + k * pitch as isize) as usize;
        let d: &mut [u8; 4] = (&mut buf[i..i + 4]).try_into().unwrap();
        lpf_st4_neon(token, d, v);
    };
    st(buf, -1, p1p0);
    st(buf, -2, lpf_bsrl_neon::<8>(token, p1p0));
    st(buf, 0, q1q0);
    st(buf, 1, lpf_bsrl_neon::<8>(token, q1q0));
}

/// C `svt_aom_lpf_horizontal_8_sse2` — 4 columns, 4-byte row loads.
#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn lpf_horizontal_8_impl_neon(
    token: NeonToken,
    buf: &mut [u8],
    off: usize,
    pitch: usize,
    t: LfThresh,
) {
    let blimit16 = vreinterpretq_u8_u16(vdupq_n_u16(t.mblim as u16));
    let limit = vdupq_n_u8(t.lim);
    let thresh = vdupq_n_u8(t.hev_thr);

    let row = |k: isize| -> uint8x16_t {
        let i = (off as isize + k * pitch as isize) as usize;
        let r: &[u8; 4] = buf[i..i + 4].try_into().unwrap();
        lpf_ld4_neon(token, r)
    };
    let p = [row(-1), row(-2), row(-3), row(-4)];
    let q = [row(0), row(1), row(2), row(3)];
    let (q1q0, p1p0, p2, q2) = lpf_internal_8_neon(token, &p, &q, blimit16, limit, thresh);

    let st = |buf: &mut [u8], k: isize, v: uint8x16_t| {
        let i = (off as isize + k * pitch as isize) as usize;
        let d: &mut [u8; 4] = (&mut buf[i..i + 4]).try_into().unwrap();
        lpf_st4_neon(token, d, v);
    };
    st(buf, -1, p1p0);
    st(buf, -2, lpf_bsrl_neon::<8>(token, p1p0));
    st(buf, 0, q1q0);
    st(buf, 1, lpf_bsrl_neon::<8>(token, q1q0));
    st(buf, -3, p2);
    st(buf, 2, q2);
}

/// C `transpose6x6_sse2` (dlf_intrin_sse2.c:962): six rows in (rows 4-5
/// are zero on the forward call), three column-pair vectors out. Doubles
/// as the inverse, same as the v3 arm.
#[cfg(target_arch = "aarch64")]
#[rite]
fn transpose6x6_neon(token: NeonToken, x: &[uint8x16_t; 6]) -> [uint8x16_t; 3] {
    let w0 = lpf_ziplo8_neon(token, x[0], x[1]);
    let w1 = lpf_ziplo8_neon(token, x[2], x[3]);
    let w2 = lpf_ziplo8_neon(token, x[4], x[5]);

    let z16 = |a: uint8x16_t, b: uint8x16_t, hi: bool| -> uint8x16_t {
        let (a16, b16) = (vreinterpretq_u16_u8(a), vreinterpretq_u16_u8(b));
        vreinterpretq_u8_u16(if hi {
            vzip2q_u16(a16, b16)
        } else {
            vzip1q_u16(a16, b16)
        })
    };
    let w4 = z16(w0, w1, false);
    let w5 = z16(w2, w0, false);
    let d0d1 = lpf_ziplo32_neon(token, w4, w5);
    let d2d3 = lpf_ziphi32_neon(token, w4, w5);

    let w4 = z16(w0, w1, true);
    let w5 = z16(w2, x[3], true);
    let d4d5 = lpf_ziplo32_neon(token, w4, w5);
    [d0d1, d2d3, d4d5]
}

/// C `transpose8x8_sse2` (dlf_intrin_sse2.c:993): eight rows in (rows
/// 4-7 are zero on the forward call), four column-pair vectors out.
/// Doubles as the inverse.
#[cfg(target_arch = "aarch64")]
#[rite]
fn transpose8x8_neon(token: NeonToken, x: &[uint8x16_t; 8]) -> [uint8x16_t; 4] {
    let w0 = lpf_ziplo8_neon(token, x[0], x[1]);
    let w1 = lpf_ziplo8_neon(token, x[2], x[3]);
    let w2 = lpf_ziplo8_neon(token, x[4], x[5]);
    let w3 = lpf_ziplo8_neon(token, x[6], x[7]);

    let z16 = |a: uint8x16_t, b: uint8x16_t, hi: bool| -> uint8x16_t {
        let (a16, b16) = (vreinterpretq_u16_u8(a), vreinterpretq_u16_u8(b));
        vreinterpretq_u8_u16(if hi {
            vzip2q_u16(a16, b16)
        } else {
            vzip1q_u16(a16, b16)
        })
    };
    let w4 = z16(w0, w1, false);
    let w5 = z16(w2, w3, false);
    let d0d1 = lpf_ziplo32_neon(token, w4, w5);
    let d2d3 = lpf_ziphi32_neon(token, w4, w5);

    let w6 = z16(w0, w1, true);
    let w7 = z16(w2, w3, true);
    let d4d5 = lpf_ziplo32_neon(token, w6, w7);
    let d6d7 = lpf_ziphi32_neon(token, w6, w7);
    [d0d1, d2d3, d4d5, d6d7]
}

/// C `svt_aom_lpf_vertical_6_sse2` — 4 rows. Loads 8-byte rows at s-3
/// (the transpose consumes the low 6 bytes), transposes, filters,
/// transposes back, and stores 6 bytes per row.
#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn lpf_vertical_6_impl_neon(
    token: NeonToken,
    buf: &mut [u8],
    off: usize,
    pitch: usize,
    t: LfThresh,
) {
    let blimit16 = vreinterpretq_u8_u16(vdupq_n_u16(t.mblim as u16));
    let limit = vdupq_n_u8(t.lim);
    let thresh = vdupq_n_u8(t.hev_thr);

    let zero = vdupq_n_u8(0);
    let row = |k: usize| -> uint8x16_t {
        // Same 8-byte-load-of-6-used as the v3 arm: see `load8_zero_padded`.
        let r = load8_zero_padded(buf, off - 3 + k * pitch);
        lpf_ld8_neon(token, &r)
    };
    let x = [row(0), row(1), row(2), row(3), zero, zero];
    let [d0d1, d2d3, d4d5] = transpose6x6_neon(token, &x);
    let d1 = lpf_bsrl_neon::<8>(token, d0d1);
    let d3 = lpf_bsrl_neon::<8>(token, d2d3);
    let d5 = lpf_bsrl_neon::<8>(token, d4d5);

    // C: lpf_internal_6(&d0d1, &d5, &d1, &d4d5, &d2d3, &d3, ...)
    let p = [d2d3, d1, d0d1];
    let q = [d3, d4d5, d5];
    let (q1q0, p1p0) = lpf_internal_6_neon(token, &p, &q, blimit16, limit, thresh);

    let p0 = lpf_bsrl_neon::<8>(token, p1p0);
    let q0 = lpf_bsrl_neon::<8>(token, q1q0);
    // C: transpose6x6(&d0d1, &p0, &p1p0, &q1q0, &q0, &d5, ...)
    let xi = [d0d1, p0, p1p0, q1q0, q0, d5];
    let [o0, o2, _o4] = transpose6x6_neon(token, &xi);

    // C stores 6 bytes per row via a 16-byte temp + memcpy.
    let mut tmp = [0u8; 8];
    for (r, v) in [
        o0,
        lpf_bsrl_neon::<8>(token, o0),
        o2,
        lpf_bsrl_neon::<8>(token, o2),
    ]
    .iter()
    .enumerate()
    {
        vst1_u8(&mut tmp, vget_low_u8(*v));
        let b = off - 3 + r * pitch;
        buf[b..b + 6].copy_from_slice(&tmp[0..6]);
    }
}

/// C `svt_aom_lpf_vertical_8_sse2` — 4 rows. Loads 8-byte rows at s-4,
/// transposes, filters, transposes back, stores 8 bytes per row.
#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn lpf_vertical_8_impl_neon(
    token: NeonToken,
    buf: &mut [u8],
    off: usize,
    pitch: usize,
    t: LfThresh,
) {
    let blimit16 = vreinterpretq_u8_u16(vdupq_n_u16(t.mblim as u16));
    let limit = vdupq_n_u8(t.lim);
    let thresh = vdupq_n_u8(t.hev_thr);

    let zero = vdupq_n_u8(0);
    let row = |k: usize| -> uint8x16_t {
        let i = off - 4 + k * pitch;
        let r: &[u8; 8] = buf[i..i + 8].try_into().unwrap();
        lpf_ld8_neon(token, r)
    };
    let x = [row(0), row(1), row(2), row(3), zero, zero, zero, zero];
    let [d0d1, d2d3, d4d5, d6d7] = transpose8x8_neon(token, &x);
    let d1 = lpf_bsrl_neon::<8>(token, d0d1);
    let d3 = lpf_bsrl_neon::<8>(token, d2d3);
    let d5 = lpf_bsrl_neon::<8>(token, d4d5);
    let d7 = lpf_bsrl_neon::<8>(token, d6d7);

    // C: lpf_internal_8(&d0d1, &d7, &d1, &d6d7, &d2d3, &d5, &d3, &d4d5, ...)
    let p = [d3, d2d3, d1, d0d1];
    let q = [d4d5, d5, d6d7, d7];
    let (q1q0, p1p0, p2, q2) = lpf_internal_8_neon(token, &p, &q, blimit16, limit, thresh);

    let p0 = lpf_bsrl_neon::<8>(token, p1p0);
    let q0 = lpf_bsrl_neon::<8>(token, q1q0);
    // C: transpose8x8(&d0d1, &p2, &p0, &p1p0, &q1q0, &q0, &q2, &d7, ...)
    let xi = [d0d1, p2, p0, p1p0, q1q0, q0, q2, d7];
    let [o0, o2, _o4, _o6] = transpose8x8_neon(token, &xi);

    let st = |buf: &mut [u8], k: usize, v: uint8x16_t| {
        let i = off - 4 + k * pitch;
        let d: &mut [u8; 8] = (&mut buf[i..i + 8]).try_into().unwrap();
        vst1_u8(d, vget_low_u8(v));
    };
    st(buf, 0, o0);
    st(buf, 1, lpf_bsrl_neon::<8>(token, o0));
    st(buf, 2, o2);
    st(buf, 3, lpf_bsrl_neon::<8>(token, o2));
}

// -----------------------------------------------------------------------------
// AArch64 14-tap kernels — same transliteration of the v3 arms.
// -----------------------------------------------------------------------------

/// C `filter4_14_sse2` (dlf_intrin_sse2.c:227): the narrow-filter half of
/// the 14-tap kernel. Returns the (qs1qs0, ps1ps0) merged output pairs.
#[cfg(target_arch = "aarch64")]
#[rite]
fn filter4_14_neon(
    token: NeonToken,
    p1p0: uint8x16_t,
    q1q0: uint8x16_t,
    hev: uint8x16_t,
    mask: uint8x16_t,
) -> (uint8x16_t, uint8x16_t) {
    // _mm_set_epi8(0 x8, 3 x4, 4 x4) — low lanes 4 x4, then 3 x4, then 0 x8.
    let t3t4: &[i8; 16] = &[4, 4, 4, 4, 3, 3, 3, 3, 0, 0, 0, 0, 0, 0, 0, 0];
    let t3t4 = vld1q_s8(t3t4);
    let t80 = vdupq_n_u8(0x80);
    let ff = vdupq_n_s8(-1);

    let ps1ps0_work = vreinterpretq_s8_u8(veorq_u8(p1p0, t80));
    let mut qs1qs0_work = vreinterpretq_s8_u8(veorq_u8(q1q0, t80));

    // filter = signed_char_clamp(ps1 - qs1) & hev
    let work = vqsubq_s8(ps1ps0_work, qs1qs0_work);
    let mut filter = vandq_s8(
        vreinterpretq_s8_u8(lpf_bsrl_neon::<4>(token, vreinterpretq_u8_s8(work))),
        vreinterpretq_s8_u8(hev),
    );
    // filter = signed_char_clamp(filter + 3 * (qs0 - ps0)) & mask
    filter = vqsubq_s8(filter, work);
    filter = vqsubq_s8(filter, work);
    filter = vqsubq_s8(filter, work);
    filter = vandq_s8(filter, vreinterpretq_s8_u8(mask));
    filter = vreinterpretq_s8_u8(lpf_ziplo32_neon(
        token,
        vreinterpretq_u8_s8(filter),
        vreinterpretq_u8_s8(filter),
    ));

    // filter1 = signed_char_clamp(filter + 4) >> 3;
    // filter2 = signed_char_clamp(filter + 3) >> 3
    let mut f21 = vqaddq_s8(filter, t3t4);
    f21 = vreinterpretq_s8_u8(vzip1q_u8(
        vreinterpretq_u8_s8(f21),
        vreinterpretq_u8_s8(f21),
    ));
    f21 = vreinterpretq_s8_s16(vshrq_n_s16::<11>(vreinterpretq_s16_s8(f21)));
    f21 = lpf_packs_dup_neon(token, vreinterpretq_s16_s8(f21));

    // filter = ROUND_POWER_OF_TWO(filter1, 1) & ~hev
    filter = vqsubq_s8(f21, ff);
    filter = vreinterpretq_s8_u8(vzip1q_u8(
        vreinterpretq_u8_s8(filter),
        vreinterpretq_u8_s8(filter),
    ));
    filter = vreinterpretq_s8_s16(vshrq_n_s16::<9>(vreinterpretq_s16_s8(filter)));
    filter = lpf_packs_dup_neon(token, vreinterpretq_s16_s8(filter));
    filter = vreinterpretq_s8_u8(vbicq_u8(vreinterpretq_u8_s8(filter), hev));
    filter = vreinterpretq_s8_u8(lpf_ziplo32_neon(
        token,
        vreinterpretq_u8_s8(filter),
        vreinterpretq_u8_s8(filter),
    ));

    let f21m = lpf_ziplo32_neon(token, vreinterpretq_u8_s8(f21), vreinterpretq_u8_s8(filter));
    let hev1 = lpf_bsrl_neon::<8>(token, f21m);
    // signed_char_clamp(qs1 - filter), signed_char_clamp(qs0 - filter1)
    qs1qs0_work = vqsubq_s8(qs1qs0_work, vreinterpretq_s8_u8(f21m));
    // signed_char_clamp(ps1 + filter), signed_char_clamp(ps0 + filter2)
    let ps1ps0_out = vqaddq_s8(ps1ps0_work, vreinterpretq_s8_u8(hev1));

    (
        veorq_u8(vreinterpretq_u8_s8(qs1qs0_work), t80),
        veorq_u8(vreinterpretq_u8_s8(ps1ps0_out), t80),
    )
}

/// C `lpf_internal_14_sse2` (dlf_intrin_sse2.c:357). `qp[i]` is the merged
/// `q{i}p{i}` pair vector; `qp[0..=5]` are written back, `qp[6]` is
/// read-only. Identical op sequence to [`lpf_internal_14_v3`].
#[cfg(target_arch = "aarch64")]
#[rite]
#[allow(clippy::too_many_arguments)]
fn lpf_internal_14_neon(
    token: NeonToken,
    qp: &mut [uint8x16_t; 7],
    blimit16: uint8x16_t,
    limit: uint8x16_t,
    thresh: uint8x16_t,
) {
    let zero = vdupq_n_u8(0);
    let one = vdupq_n_u8(1);
    let fe = vdupq_n_u8(0xfe);
    let ff = vdupq_n_u8(0xff);

    let p1p0 = lpf_ziplo32_neon(token, qp[0], qp[1]);
    let q1q0 = lpf_bsrl_neon::<8>(token, p1p0);

    // filter_mask + hev_mask (dlf_intrin_sse2.c:371-404)
    let abs_p1p0 = lpf_abs_diff_neon(token, qp[1], qp[0]);
    let abs_q1q0 = lpf_bsrl_neon::<4>(token, abs_p1p0);
    let abs_p0q0 = lpf_abs_diff_neon(token, p1p0, q1q0);
    let mut abs_p1q1 = lpf_bsrl_neon::<4>(token, abs_p0q0);

    let flat_a = vmaxq_u8(abs_p1p0, abs_q1q0);
    let mut hev = vqsubq_u8(flat_a, thresh);
    hev = veorq_u8(vceqq_u8(hev, zero), ff);
    // replicate for the "merged variables" usage
    hev = lpf_ziplo32_neon(token, hev, hev);

    abs_p1q1 = vreinterpretq_u8_u16(vshrq_n_u16::<1>(vreinterpretq_u16_u8(vandq_u8(
        abs_p1q1, fe,
    ))));
    // The sum>blimit "don't filter" flag, computed in u16 — same
    // _sse2-exact/_c-exact reasoning as the v3 arm (mblim <= 193).
    let a16 = lpf_widen_neon(token, abs_p0q0);
    let s16 = vaddq_u16(vaddq_u16(a16, a16), lpf_widen_neon(token, abs_p1q1));
    let cmp = vcgtq_s16(vreinterpretq_s16_u16(s16), vreinterpretq_s16_u8(blimit16));
    let flag = vreinterpretq_u8_s8(lpf_packs_dup_neon(token, vreinterpretq_s16_u16(cmp)));

    let mut work = vmaxq_u8(
        lpf_abs_diff_neon(token, qp[2], qp[1]),
        lpf_abs_diff_neon(token, qp[3], qp[2]),
    );
    work = vmaxq_u8(abs_p1p0, work);
    work = vmaxq_u8(work, lpf_bsrl_neon::<4>(token, work));
    work = vqsubq_u8(work, limit);
    let mask = vbicq_u8(vceqq_u8(work, zero), flag);

    // lp filter (shared with the 6/8-tap kernels)
    let (qs1qs0, ps1ps0) = filter4_14_neon(token, p1p0, q1q0, hev, mask);
    let mut qs0ps0 = lpf_ziplo32_neon(token, ps1ps0, qs1qs0);
    let mut qs1ps1 = lpf_bsrl_neon::<8>(token, qs0ps0);

    // flat mask
    let mut flat = vmaxq_u8(
        lpf_abs_diff_neon(token, qp[2], qp[0]),
        lpf_abs_diff_neon(token, qp[3], qp[0]),
    );
    flat = vmaxq_u8(abs_p1p0, flat);
    flat = vmaxq_u8(flat, lpf_bsrl_neon::<4>(token, flat));
    flat = vqsubq_u8(flat, one);
    flat = vceqq_u8(flat, zero);
    flat = vandq_u8(flat, mask);
    flat = lpf_ziplo32_neon(token, flat, flat);
    flat = lpf_ziplo64_neon(token, flat, flat);

    // if flat == 0 then flat2 is zero as well and none of this is needed
    if vminvq_u8(vceqq_u8(flat, zero)) != 0xff {
        let eight = vdupq_n_u16(8);
        let four = vdupq_n_u16(4);
        let mut pq_16 = [vdupq_n_u16(0); 7];
        for (i, v) in pq_16.iter_mut().enumerate() {
            *v = lpf_widen_neon(token, qp[i]);
        }
        let q0_16 = vreinterpretq_u16_u8(lpf_bsrl_neon::<8>(token, vreinterpretq_u8_u16(pq_16[0])));
        let q1_16 = vreinterpretq_u16_u8(lpf_bsrl_neon::<8>(token, vreinterpretq_u8_u16(pq_16[1])));
        let q2_16 = vreinterpretq_u16_u8(lpf_bsrl_neon::<8>(token, vreinterpretq_u8_u16(pq_16[2])));
        let q3_16 = vreinterpretq_u16_u8(lpf_bsrl_neon::<8>(token, vreinterpretq_u8_u16(pq_16[3])));
        let q4_16 = vreinterpretq_u16_u8(lpf_bsrl_neon::<8>(token, vreinterpretq_u8_u16(pq_16[4])));
        let q5_16 = vreinterpretq_u16_u8(lpf_bsrl_neon::<8>(token, vreinterpretq_u8_u16(pq_16[5])));

        let mut sum_p = vaddq_u16(pq_16[5], vaddq_u16(pq_16[4], pq_16[3]));
        let mut sum_lp = vaddq_u16(pq_16[0], vaddq_u16(pq_16[2], pq_16[1]));
        sum_p = vaddq_u16(sum_p, sum_lp);

        let mut sum_lq =
            vreinterpretq_u16_u8(lpf_bsrl_neon::<8>(token, vreinterpretq_u8_u16(sum_lp)));
        let mut sum_q =
            vreinterpretq_u16_u8(lpf_bsrl_neon::<8>(token, vreinterpretq_u8_u16(sum_p)));

        let sum_p_0 = vaddq_u16(eight, vaddq_u16(sum_p, sum_q));
        sum_lp = vaddq_u16(four, vaddq_u16(sum_lp, sum_lq));

        let flat_p0 = vaddq_u16(sum_lp, vaddq_u16(pq_16[3], pq_16[0]));
        let flat_q0 = vaddq_u16(sum_lp, vaddq_u16(q3_16, q0_16));

        let mut sum_p6 = vaddq_u16(pq_16[6], pq_16[6]);
        let mut sum_p3 = vaddq_u16(pq_16[3], pq_16[3]);

        sum_q = vsubq_u16(sum_p_0, pq_16[5]);
        sum_p = vsubq_u16(sum_p_0, q5_16);

        let work0_0 = vaddq_u16(vaddq_u16(pq_16[6], pq_16[0]), pq_16[1]);
        let work0_1 = vaddq_u16(sum_p6, vaddq_u16(pq_16[1], vaddq_u16(pq_16[2], pq_16[0])));

        sum_lq = vsubq_u16(sum_lp, pq_16[2]);
        sum_lp = vsubq_u16(sum_lp, q2_16);

        work = vreinterpretq_u8_u16(vaddq_u16(sum_p3, pq_16[1]));
        let flat_p1 = vaddq_u16(sum_lp, vreinterpretq_u16_u8(work));
        let flat_q1 = vaddq_u16(
            sum_lq,
            vreinterpretq_u16_u8(lpf_bsrl_neon::<8>(token, work)),
        );

        let mut flat_pq0 = vshrq_n_u16::<3>(vreinterpretq_u16_u8(lpf_ziplo64_neon(
            token,
            vreinterpretq_u8_u16(flat_p0),
            vreinterpretq_u8_u16(flat_q0),
        )));
        let mut flat_pq1 = vshrq_n_u16::<3>(vreinterpretq_u16_u8(lpf_ziplo64_neon(
            token,
            vreinterpretq_u8_u16(flat_p1),
            vreinterpretq_u8_u16(flat_q1),
        )));
        flat_pq0 =
            vreinterpretq_u16_u8(lpf_packus_dup_neon(token, vreinterpretq_s16_u16(flat_pq0)));
        flat_pq1 =
            vreinterpretq_u16_u8(lpf_packus_dup_neon(token, vreinterpretq_s16_u16(flat_pq1)));

        sum_lp = vsubq_u16(sum_lp, q1_16);
        sum_lq = vsubq_u16(sum_lq, pq_16[1]);

        sum_p3 = vaddq_u16(sum_p3, pq_16[3]);
        work = vreinterpretq_u8_u16(vaddq_u16(sum_p3, pq_16[2]));

        let flat_p2 = vaddq_u16(sum_lp, vreinterpretq_u16_u8(work));
        let flat_q2 = vaddq_u16(
            sum_lq,
            vreinterpretq_u16_u8(lpf_bsrl_neon::<8>(token, work)),
        );
        let mut flat_pq2 = vshrq_n_u16::<3>(vreinterpretq_u16_u8(lpf_ziplo64_neon(
            token,
            vreinterpretq_u8_u16(flat_p2),
            vreinterpretq_u8_u16(flat_q2),
        )));
        flat_pq2 =
            vreinterpretq_u16_u8(lpf_packus_dup_neon(token, vreinterpretq_s16_u16(flat_pq2)));

        // flat2 mask
        let mut flat2 = vmaxq_u8(
            lpf_abs_diff_neon(token, qp[4], qp[0]),
            lpf_abs_diff_neon(token, qp[5], qp[0]),
        );
        work = lpf_abs_diff_neon(token, qp[6], qp[0]);
        flat2 = vmaxq_u8(work, flat2);
        flat2 = vmaxq_u8(flat2, lpf_bsrl_neon::<4>(token, flat2));
        flat2 = vqsubq_u8(flat2, one);
        flat2 = vceqq_u8(flat2, zero);
        flat2 = vandq_u8(flat2, flat); // flat2 & flat & mask
        flat2 = lpf_ziplo32_neon(token, flat2, flat2);

        // apply flat
        qs0ps0 = vbicq_u8(qs0ps0, flat);
        let flat_pq0u = vandq_u8(flat, vreinterpretq_u8_u16(flat_pq0));
        qp[0] = vorrq_u8(qs0ps0, flat_pq0u);

        qs1ps1 = vbicq_u8(qs1ps1, flat);
        let flat_pq1u = vandq_u8(flat, vreinterpretq_u8_u16(flat_pq1));
        qp[1] = vorrq_u8(qs1ps1, flat_pq1u);

        qp[2] = vbicq_u8(qp[2], flat);
        let flat_pq2u = vandq_u8(flat, vreinterpretq_u8_u16(flat_pq2));
        qp[2] = vorrq_u8(qp[2], flat_pq2u);

        if vminvq_u8(vceqq_u8(flat2, zero)) != 0xff {
            let mut flat2_pq = [vdupq_n_u8(0); 6];

            let flat2_p0 = vaddq_u16(sum_p_0, vaddq_u16(work0_0, q0_16));
            let flat2_q0 = vaddq_u16(
                sum_p_0,
                vaddq_u16(
                    vreinterpretq_u16_u8(lpf_bsrl_neon::<8>(token, vreinterpretq_u8_u16(work0_0))),
                    pq_16[0],
                ),
            );

            let flat2_p1 = vaddq_u16(sum_p, work0_1);
            let flat2_q1 = vaddq_u16(
                sum_q,
                vreinterpretq_u16_u8(lpf_bsrl_neon::<8>(token, vreinterpretq_u8_u16(work0_1))),
            );

            flat2_pq[0] =
                vreinterpretq_u8_u16(vshrq_n_u16::<4>(vreinterpretq_u16_u8(lpf_ziplo64_neon(
                    token,
                    vreinterpretq_u8_u16(flat2_p0),
                    vreinterpretq_u8_u16(flat2_q0),
                ))));
            flat2_pq[1] =
                vreinterpretq_u8_u16(vshrq_n_u16::<4>(vreinterpretq_u16_u8(lpf_ziplo64_neon(
                    token,
                    vreinterpretq_u8_u16(flat2_p1),
                    vreinterpretq_u8_u16(flat2_q1),
                ))));
            flat2_pq[0] = lpf_packus_dup_neon(token, vreinterpretq_s16_u8(flat2_pq[0]));
            flat2_pq[1] = lpf_packus_dup_neon(token, vreinterpretq_s16_u8(flat2_pq[1]));

            sum_p = vsubq_u16(sum_p, q4_16);
            sum_q = vsubq_u16(sum_q, pq_16[4]);

            sum_p6 = vaddq_u16(sum_p6, pq_16[6]);
            work = vreinterpretq_u8_u16(vaddq_u16(
                sum_p6,
                vaddq_u16(pq_16[2], vaddq_u16(pq_16[3], pq_16[1])),
            ));
            let flat2_p2 = vaddq_u16(sum_p, vreinterpretq_u16_u8(work));
            let flat2_q2 = vaddq_u16(sum_q, vreinterpretq_u16_u8(lpf_bsrl_neon::<8>(token, work)));
            flat2_pq[2] =
                vreinterpretq_u8_u16(vshrq_n_u16::<4>(vreinterpretq_u16_u8(lpf_ziplo64_neon(
                    token,
                    vreinterpretq_u8_u16(flat2_p2),
                    vreinterpretq_u8_u16(flat2_q2),
                ))));
            flat2_pq[2] = lpf_packus_dup_neon(token, vreinterpretq_s16_u8(flat2_pq[2]));

            sum_p6 = vaddq_u16(sum_p6, pq_16[6]);
            sum_p = vsubq_u16(sum_p, q3_16);
            sum_q = vsubq_u16(sum_q, pq_16[3]);

            work = vreinterpretq_u8_u16(vaddq_u16(
                sum_p6,
                vaddq_u16(pq_16[3], vaddq_u16(pq_16[4], pq_16[2])),
            ));
            let flat2_p3 = vaddq_u16(sum_p, vreinterpretq_u16_u8(work));
            let flat2_q3 = vaddq_u16(sum_q, vreinterpretq_u16_u8(lpf_bsrl_neon::<8>(token, work)));
            flat2_pq[3] =
                vreinterpretq_u8_u16(vshrq_n_u16::<4>(vreinterpretq_u16_u8(lpf_ziplo64_neon(
                    token,
                    vreinterpretq_u8_u16(flat2_p3),
                    vreinterpretq_u8_u16(flat2_q3),
                ))));
            flat2_pq[3] = lpf_packus_dup_neon(token, vreinterpretq_s16_u8(flat2_pq[3]));

            sum_p6 = vaddq_u16(sum_p6, pq_16[6]);
            sum_p = vsubq_u16(sum_p, q2_16);
            sum_q = vsubq_u16(sum_q, pq_16[2]);

            work = vreinterpretq_u8_u16(vaddq_u16(
                sum_p6,
                vaddq_u16(pq_16[4], vaddq_u16(pq_16[5], pq_16[3])),
            ));
            let flat2_p4 = vaddq_u16(sum_p, vreinterpretq_u16_u8(work));
            let flat2_q4 = vaddq_u16(sum_q, vreinterpretq_u16_u8(lpf_bsrl_neon::<8>(token, work)));
            flat2_pq[4] =
                vreinterpretq_u8_u16(vshrq_n_u16::<4>(vreinterpretq_u16_u8(lpf_ziplo64_neon(
                    token,
                    vreinterpretq_u8_u16(flat2_p4),
                    vreinterpretq_u8_u16(flat2_q4),
                ))));
            flat2_pq[4] = lpf_packus_dup_neon(token, vreinterpretq_s16_u8(flat2_pq[4]));

            sum_p6 = vaddq_u16(sum_p6, pq_16[6]);
            sum_p = vsubq_u16(sum_p, q1_16);
            sum_q = vsubq_u16(sum_q, pq_16[1]);

            work = vreinterpretq_u8_u16(vaddq_u16(
                sum_p6,
                vaddq_u16(pq_16[5], vaddq_u16(pq_16[6], pq_16[4])),
            ));
            let flat2_p5 = vaddq_u16(sum_p, vreinterpretq_u16_u8(work));
            let flat2_q5 = vaddq_u16(sum_q, vreinterpretq_u16_u8(lpf_bsrl_neon::<8>(token, work)));
            flat2_pq[5] =
                vreinterpretq_u8_u16(vshrq_n_u16::<4>(vreinterpretq_u16_u8(lpf_ziplo64_neon(
                    token,
                    vreinterpretq_u8_u16(flat2_p5),
                    vreinterpretq_u8_u16(flat2_q5),
                ))));
            flat2_pq[5] = lpf_packus_dup_neon(token, vreinterpretq_s16_u8(flat2_pq[5]));

            // wide flat apply
            for i in 0..6 {
                qp[i] = vorrq_u8(vbicq_u8(qp[i], flat2), vandq_u8(flat2, flat2_pq[i]));
            }
        }
    } else {
        qp[0] = qs0ps0;
        qp[1] = qs1ps1;
    }
}

/// C `svt_aom_lpf_horizontal_14_sse2` — 4 columns. Loads the 4-byte rows
/// p6..q6 pairwise into the merged `q{i}p{i}` vectors, filters, and stores
/// the 12 modified rows (p5..q5) back.
#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn lpf_horizontal_14_impl_neon(
    token: NeonToken,
    buf: &mut [u8],
    off: usize,
    pitch: usize,
    t: LfThresh,
) {
    let blimit16 = vreinterpretq_u8_u16(vdupq_n_u16(t.mblim as u16));
    let limit = vdupq_n_u8(t.lim);
    let thresh = vdupq_n_u8(t.hev_thr);

    let row = |k: isize| -> uint8x16_t {
        let i = (off as isize + k * pitch as isize) as usize;
        let r: &[u8; 4] = buf[i..i + 4].try_into().unwrap();
        lpf_ld4_neon(token, r)
    };
    let mut qp = [
        lpf_ziplo32_neon(token, row(-1), row(0)),
        lpf_ziplo32_neon(token, row(-2), row(1)),
        lpf_ziplo32_neon(token, row(-3), row(2)),
        lpf_ziplo32_neon(token, row(-4), row(3)),
        lpf_ziplo32_neon(token, row(-5), row(4)),
        lpf_ziplo32_neon(token, row(-6), row(5)),
        lpf_ziplo32_neon(token, row(-7), row(6)),
    ];
    lpf_internal_14_neon(token, &mut qp, blimit16, limit, thresh);
    // C `store_buffer_horz_8(q{num}p{num}, p, num, s)`, num = 0..=5:
    // stores 4 bytes at s-(num+1)*p and s+num*p.
    for num in 0..6isize {
        let v = qp[num as usize];
        let lo = (off as isize - (num + 1) * pitch as isize) as usize;
        let d_lo: &mut [u8; 4] = (&mut buf[lo..lo + 4]).try_into().unwrap();
        lpf_st4_neon(token, d_lo, v);
        let hi = (off as isize + num * pitch as isize) as usize;
        let d_hi: &mut [u8; 4] = (&mut buf[hi..hi + 4]).try_into().unwrap();
        lpf_st4_neon(token, d_hi, lpf_bsrl_neon::<4>(token, v));
    }
}

/// C `transpose_pq_14_sse2` (dlf_intrin_sse2.c:1029): four 16-byte rows in
/// (x[0..4] = file rows 0..3), eight merged `q{i}p{i}` pairs out.
#[cfg(target_arch = "aarch64")]
#[rite]
fn transpose_pq_14_neon(token: NeonToken, x: &[uint8x16_t; 4]) -> [uint8x16_t; 8] {
    let w0 = lpf_ziplo8_neon(token, x[0], x[1]);
    let w1 = lpf_ziplo8_neon(token, x[2], x[3]);
    let w2 = lpf_ziphi8_neon(token, x[0], x[1]);
    let w3 = lpf_ziphi8_neon(token, x[2], x[3]);

    let z16 = |a: uint8x16_t, b: uint8x16_t, hi: bool| -> uint8x16_t {
        let (a16, b16) = (vreinterpretq_u16_u8(a), vreinterpretq_u16_u8(b));
        vreinterpretq_u8_u16(if hi {
            vzip2q_u16(a16, b16)
        } else {
            vzip1q_u16(a16, b16)
        })
    };
    let ww0 = z16(w0, w1, false);
    let ww1 = z16(w0, w1, true);
    let ww2 = z16(w2, w3, false);
    let ww3 = z16(w2, w3, true);

    [
        lpf_ziplo32_neon(token, lpf_bsrl_neon::<12>(token, ww1), ww2), // q0p0
        lpf_ziphi32_neon(token, ww1, lpf_bsll4_neon(token, ww2)),      // q1p1
        lpf_ziphi32_neon(token, lpf_bsll4_neon(token, ww1), ww2),      // q2p2
        lpf_ziplo32_neon(token, ww1, lpf_bsrl_neon::<12>(token, ww2)), // q3p3
        lpf_ziplo32_neon(token, lpf_bsrl_neon::<12>(token, ww0), ww3), // q4p4
        lpf_ziphi32_neon(token, ww0, lpf_bsll4_neon(token, ww3)),      // q5p5
        lpf_ziphi32_neon(token, lpf_bsll4_neon(token, ww0), ww3),      // q6p6
        lpf_ziplo32_neon(token, ww0, lpf_bsrl_neon::<12>(token, ww3)), // q7p7
    ]
}

/// C `transpose_pq_14_inv_sse2` (dlf_intrin_sse2.c:1062): eight merged
/// pairs in (x[0..8] = q7p7..q0p0, C's argument order), four 16-byte
/// rows out.
#[cfg(target_arch = "aarch64")]
#[rite]
fn transpose_pq_14_inv_neon(token: NeonToken, x: &[uint8x16_t; 8]) -> [uint8x16_t; 4] {
    let z16 = |a: uint8x16_t, b: uint8x16_t, hi: bool| -> uint8x16_t {
        let (a16, b16) = (vreinterpretq_u16_u8(a), vreinterpretq_u16_u8(b));
        vreinterpretq_u8_u16(if hi {
            vzip2q_u16(a16, b16)
        } else {
            vzip1q_u16(a16, b16)
        })
    };
    let w0 = lpf_ziplo8_neon(token, x[0], x[1]);
    let w1 = lpf_ziplo8_neon(token, x[2], x[3]);
    let w2 = lpf_ziplo8_neon(token, x[4], x[5]);
    let w3 = lpf_ziplo8_neon(token, x[6], x[7]);

    let w4 = z16(w0, w1, false);
    let w5 = z16(w2, w3, false);

    let d0 = lpf_ziplo32_neon(token, w4, w5);
    let d2 = lpf_ziphi32_neon(token, w4, w5);

    let w10 = lpf_ziplo8_neon(token, x[7], x[6]);
    let w11 = lpf_ziplo8_neon(token, x[5], x[4]);
    let w12 = lpf_ziplo8_neon(token, x[3], x[2]);
    let w13 = lpf_ziplo8_neon(token, x[1], x[0]);

    let w4b = z16(w10, w11, true);
    let w5b = z16(w12, w13, true);

    let d1 = lpf_ziplo32_neon(token, w4b, w5b);
    let d3 = lpf_ziphi32_neon(token, w4b, w5b);

    [
        lpf_ziplo64_neon(token, d0, d1),
        lpf_ziphi64_neon(token, d0, d1),
        lpf_ziplo64_neon(token, d2, d3),
        lpf_ziphi64_neon(token, d2, d3),
    ]
}

/// C `svt_aom_lpf_vertical_14_sse2` — 4 rows. Loads a 16-byte p7..q7
/// window per row (the outermost pair round-trips unchanged), transposes
/// to merged pairs, filters, transposes back, stores all 16 bytes.
#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn lpf_vertical_14_impl_neon(
    token: NeonToken,
    buf: &mut [u8],
    off: usize,
    pitch: usize,
    t: LfThresh,
) {
    let blimit16 = vreinterpretq_u8_u16(vdupq_n_u16(t.mblim as u16));
    let limit = vdupq_n_u8(t.lim);
    let thresh = vdupq_n_u8(t.hev_thr);

    let mut x = [vdupq_n_u8(0); 4];
    for (i, v) in x.iter_mut().enumerate() {
        let b = off - 8 + i * pitch;
        let r: &[u8; 16] = buf[b..b + 16].try_into().unwrap();
        *v = vld1q_u8(r);
    }
    let qp8 = transpose_pq_14_neon(token, &x);
    let mut qp: [uint8x16_t; 7] = qp8[..7].try_into().unwrap();
    lpf_internal_14_neon(token, &mut qp, blimit16, limit, thresh);
    // C passes q7p7..q0p0 to the inverse transpose.
    let inv_in = [qp8[7], qp[6], qp[5], qp[4], qp[3], qp[2], qp[1], qp[0]];
    let pq = transpose_pq_14_inv_neon(token, &inv_in);
    for (i, v) in pq.iter().enumerate() {
        let b = off - 8 + i * pitch;
        let d: &mut [u8; 16] = (&mut buf[b..b + 16]).try_into().unwrap();
        vst1q_u8(d, *v);
    }
}
