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

/// C `svt_aom_lpf_horizontal_6_c`.
pub fn lpf_horizontal_6(buf: &mut [u8], off: usize, pitch: usize, t: LfThresh) {
    for i in 0..4 {
        let base = off + i;
        let mut w: [u8; 6] = gather(buf, base, pitch);
        let mask = filter_mask3_chroma(t.lim, t.mblim, w[0], w[1], w[2], w[3], w[4], w[5]);
        let flat = flat_mask3_chroma(1, w[0], w[1], w[2], w[3], w[4], w[5]);
        filter6_line(mask, t.hev_thr, flat, &mut w);
        scatter(buf, base, pitch, &w);
    }
}

/// C `svt_aom_lpf_vertical_6_c`.
pub fn lpf_vertical_6(buf: &mut [u8], off: usize, pitch: usize, t: LfThresh) {
    for i in 0..4 {
        let base = off + i * pitch;
        let mut w: [u8; 6] = gather(buf, base, 1);
        let mask = filter_mask3_chroma(t.lim, t.mblim, w[0], w[1], w[2], w[3], w[4], w[5]);
        let flat = flat_mask3_chroma(1, w[0], w[1], w[2], w[3], w[4], w[5]);
        filter6_line(mask, t.hev_thr, flat, &mut w);
        scatter(buf, base, 1, &w);
    }
}

/// C `svt_aom_lpf_horizontal_8_c`.
pub fn lpf_horizontal_8(buf: &mut [u8], off: usize, pitch: usize, t: LfThresh) {
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

/// C `svt_aom_lpf_vertical_8_c`.
pub fn lpf_vertical_8(buf: &mut [u8], off: usize, pitch: usize, t: LfThresh) {
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

/// C `svt_aom_lpf_horizontal_14_c` (4 columns).
pub fn lpf_horizontal_14(buf: &mut [u8], off: usize, pitch: usize, t: LfThresh) {
    for i in 0..4 {
        let base = off + i;
        let mut w: [u8; 14] = gather(buf, base, pitch);
        lpf14_window(&mut w, t);
        scatter(buf, base, pitch, &w);
    }
}

/// C `svt_aom_lpf_vertical_14_c` (4 rows).
pub fn lpf_vertical_14(buf: &mut [u8], off: usize, pitch: usize, t: LfThresh) {
    for i in 0..4 {
        let base = off + i * pitch;
        let mut w: [u8; 14] = gather(buf, base, 1);
        lpf14_window(&mut w, t);
        scatter(buf, base, 1, &w);
    }
}
