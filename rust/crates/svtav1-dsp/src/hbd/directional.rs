use super::*;

/// Derivative table for directional prediction angles — bd-independent.
/// Duplicated from `intra_pred.rs`'s private `DR_INTRA_DERIVATIVE` (same
/// C provenance, `eb_dr_intra_derivative`).
pub(super) static DR_INTRA_DERIVATIVE_HBD: [u16; 90] = [
    0, 0, 0, 1023, 0, 0, 547, 0, 0, 372, 0, 0, 0, 0, 273, 0, 0, 215, 0, 0, 178, 0, 0, 151, 0, 0,
    132, 0, 0, 116, 0, 0, 102, 0, 0, 0, 90, 0, 0, 80, 0, 0, 71, 0, 0, 64, 0, 0, 57, 0, 0, 51, 0, 0,
    45, 0, 0, 0, 40, 0, 0, 35, 0, 0, 31, 0, 0, 27, 0, 0, 23, 0, 0, 19, 0, 0, 15, 0, 0, 0, 0, 11, 0,
    0, 7, 0, 0, 3, 0, 0,
];

pub(super) fn get_dx_hbd(angle: i32) -> i32 {
    if angle > 0 && angle < 90 {
        DR_INTRA_DERIVATIVE_HBD[angle as usize] as i32
    } else if angle > 90 && angle < 180 {
        DR_INTRA_DERIVATIVE_HBD[(180 - angle) as usize] as i32
    } else {
        1
    }
}

pub(super) fn get_dy_hbd(angle: i32) -> i32 {
    if angle > 90 && angle < 180 {
        DR_INTRA_DERIVATIVE_HBD[(angle - 90) as usize] as i32
    } else if angle > 180 && angle < 270 {
        DR_INTRA_DERIVATIVE_HBD[(270 - angle) as usize] as i32
    } else {
        1
    }
}

/// C `svt_av1_filter_intra_edge_high_c` (intra_prediction.c:2459-2480). No
/// bd/clip needed — every kernel row sums to 16 (convex combination), so a
/// weighted average of samples already within `[0, 2^bd - 1]` cannot exceed
/// that range after the `>>4` round (matches the lbd sibling
/// `intra_pred::filter_intra_edge`, which is also clip-free for the same
/// reason).
pub fn filter_intra_edge_high(p: &mut [u16], start: usize, sz: usize, strength: i32) {
    if strength == 0 {
        return;
    }
    const KERNEL: [[i32; 5]; 3] = [[0, 4, 8, 4, 0], [0, 5, 6, 5, 0], [2, 4, 4, 4, 2]];
    let filt = (strength - 1) as usize;
    debug_assert!(sz <= 129);
    let mut edge = [0u16; 129];
    edge[..sz].copy_from_slice(&p[start..start + sz]);
    for i in 1..sz {
        let mut s = 0i32;
        for (j, &k_w) in KERNEL[filt].iter().enumerate() {
            let k = (i as i32 - 2 + j as i32).clamp(0, sz as i32 - 1) as usize;
            s += edge[k] as i32 * k_w;
        }
        p[start + i] = ((s + 8) >> 4) as u16;
    }
}

/// C `filter_intra_edge_corner_high` (intra_prediction.c:2482-2489).
pub fn filter_intra_edge_corner_high(above: &mut [u16], left: &mut [u16], origin: usize) {
    let s = (left[origin] as i32 * 5 + above[origin - 1] as i32 * 6 + above[origin] as i32 * 5 + 8)
        >> 4;
    above[origin - 1] = s as u16;
    left[origin - 1] = s as u16;
}

/// C `svt_av1_upsample_intra_edge_high_c` (C_DEFAULT/intra_prediction_c.c:
/// 15-37). Unlike the clip-free edge filter above, the FIR-like
/// `[-1, 9, 9, -1]` kernel here has a negative tap and CAN overshoot
/// `[0, 2^bd - 1]`, so C clips via `clip_pixel_highbd` — the lbd sibling
/// `intra_pred::upsample_intra_edge` clips too (`.clamp(0, 255)`); this is
/// just the bd-generalized form of that same clamp.
pub fn upsample_intra_edge_high(p: &mut [u16], origin: usize, sz: usize, bd: u8) {
    debug_assert!(sz <= 16, "C MAX_UPSAMPLE_SZ");
    debug_assert!(origin >= 2);
    let mut input = [0u16; 16 + 3];
    input[0] = p[origin - 1];
    input[1] = p[origin - 1];
    input[2..2 + sz].copy_from_slice(&p[origin..origin + sz]);
    input[sz + 2] = p[origin + sz - 1];

    p[origin - 2] = input[0];
    for i in 0..sz {
        let s = -(input[i] as i32) + 9 * input[i + 1] as i32 + 9 * input[i + 2] as i32
            - input[i + 3] as i32;
        let s = clip_pixel_highbd((s + 8) >> 4, bd);
        p[origin + 2 * i - 1] = s;
        p[origin + 2 * i] = input[i + 2];
    }
}

/// C `svt_av1_highbd_dr_prediction_z1_c` (intra_prediction.c:2367-2401).
/// Shares `intra_pred::dr_z1_edged`'s incremental-accumulator structure
/// exactly (verified line-for-line against the C) — `shift` is constant
/// across a row, so C accumulates `base` incrementally per-column rather
/// than recomputing from scratch.
pub(super) fn dr_z1_edged_hbd(
    dst: &mut [u16],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u16],
    origin: usize,
    upsample_above: bool,
    dx: i32,
    bd: u8,
) {
    // The vector arm reads `above[origin + base + c .. c + 9]` with
    // `base + c < max_base_x = bw + bh - 1`, so the highest index it can
    // touch is `origin + max_base_x` — one past the scalar core's own
    // `above[origin + max_base_x]` fill read. `bw >= 8` is the u8
    // sibling's `bw >= 16` halved: 8-lane u16 chunks instead of 16-lane
    // u8. Upsampled rows take the scalar core (production upsample only
    // fires for `bw + bh <= 16` anyway).
    if bw >= 8 && !upsample_above && above.len() > origin + bw + bh {
        incant!(
            dr_z1_edged_hbd_flat(dst, dst_stride, bw, bh, above, origin, dx, bd),
            [neon, scalar]
        );
        return;
    }
    dr_z1_edged_hbd_core(
        dst,
        dst_stride,
        bw,
        bh,
        above,
        origin,
        upsample_above,
        dx,
        bd,
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) fn dr_z1_edged_hbd_flat_scalar(
    _token: ScalarToken,
    dst: &mut [u16],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u16],
    origin: usize,
    dx: i32,
    bd: u8,
) {
    dr_z1_edged_hbd_core(dst, dst_stride, bw, bh, above, origin, false, dx, bd);
}

/// aarch64 arm of [`dr_z1_edged_hbd`], non-upsampled rows: C's
/// `highbd_dr_prediction_z1_upsample0_neon` shape
/// (`highbd_intra_prediction_neon.c:1402`) — u32 widening accumulate
/// `a1*2s + a0*(64-2s)` then `vrshrn_n_u32::<6>`, which equals the scalar
/// `(a0*(32-s) + a1*s + 16) >> 5` exactly (`(2X+32)>>6 == (X+16)>>5`),
/// plus a `vmin` against `bd_max` replicating `clip_pixel_highbd`'s upper
/// clamp (the lower clamp can never fire: every term is non-negative).
#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
pub(super) fn dr_z1_edged_hbd_flat_neon(
    _token: NeonToken,
    dst: &mut [u16],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u16],
    origin: usize,
    dx: i32,
    bd: u8,
) {
    let max_base_x = (bw + bh) as i32 - 1;
    let fill = above[origin + max_base_x as usize];
    // Same max as `clip_pixel_highbd`: non-{10,12} bd groups to 255.
    let bd_max: u16 = match bd {
        10 => 1023,
        12 => 4095,
        _ => 255,
    };
    let bd_max_v = vdupq_n_u16(bd_max);
    let mut x = dx;
    for r in 0..bh {
        let base = x >> 6;
        if base >= max_base_x {
            for row in dst.chunks_mut(dst_stride).skip(r).take(bh - r) {
                row[..bw].fill(fill);
            }
            return;
        }
        let shift = ((x & 0x3f) >> 1) as u32;
        // u16 lane factors: s2 = 2*shift in [0,62], w0 = 64-2s in [2,64].
        let s2 = (shift * 2) as u16;
        let w0 = 64 - s2;
        let bi = origin + base as usize;
        let valid = core::cmp::min(bw, (max_base_x - base) as usize);
        let drow = &mut dst[r * dst_stride..r * dst_stride + bw];
        let mut c = 0;
        while c + 8 <= valid {
            let a0 = vld1q_u16(above[bi + c..bi + c + 8].try_into().unwrap());
            let a1 = vld1q_u16(above[bi + c + 1..bi + c + 9].try_into().unwrap());
            let lo = vmlal_n_u16(vmull_n_u16(vget_low_u16(a1), s2), vget_low_u16(a0), w0);
            let hi = vmlal_n_u16(vmull_n_u16(vget_high_u16(a1), s2), vget_high_u16(a0), w0);
            let out = vminq_u16(
                vcombine_u16(vrshrn_n_u32::<6>(lo), vrshrn_n_u32::<6>(hi)),
                bd_max_v,
            );
            vst1q_u16((&mut drow[c..c + 8]).try_into().unwrap(), out);
            c += 8;
        }
        if c + 4 <= valid {
            let a0 = vld1_u16(above[bi + c..bi + c + 4].try_into().unwrap());
            let a1 = vld1_u16(above[bi + c + 1..bi + c + 5].try_into().unwrap());
            let res = vmlal_n_u16(vmull_n_u16(a1, s2), a0, w0);
            let out = vmin_u16(vrshrn_n_u32::<6>(res), vget_low_u16(bd_max_v));
            vst1_u16((&mut drow[c..c + 4]).try_into().unwrap(), out);
            c += 4;
        }
        let sh = shift as i32;
        while c < valid {
            let v = (above[bi + c] as i32 * (32 - sh) + above[bi + c + 1] as i32 * sh + 16) >> 5;
            drow[c] = (v as u16).min(bd_max);
            c += 1;
        }
        drow[c..].fill(fill);
        x += dx;
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn dr_z1_edged_hbd_core(
    dst: &mut [u16],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u16],
    origin: usize,
    upsample_above: bool,
    dx: i32,
    bd: u8,
) {
    let up = upsample_above as i32;
    let max_base_x = ((bw + bh) as i32 - 1) << up;
    let frac_bits = 6 - up;
    let base_inc = 1i32 << up;
    let mut x = dx;
    for r in 0..bh {
        let mut base = x >> frac_bits;
        let shift = ((x << up) & 0x3F) >> 1;
        if base >= max_base_x {
            let fill = above[origin + max_base_x as usize];
            for row in dst.chunks_mut(dst_stride).skip(r).take(bh - r) {
                row[..bw].fill(fill);
            }
            return;
        }
        for c in 0..bw {
            let v = if base < max_base_x {
                let val = above[origin + base as usize] as i32 * (32 - shift)
                    + above[origin + base as usize + 1] as i32 * shift;
                clip_pixel_highbd((val + 16) >> 5, bd)
            } else {
                above[origin + max_base_x as usize]
            };
            dst[r * dst_stride + c] = v;
            base += base_inc;
        }
        x += dx;
    }
}

/// C `svt_av1_highbd_dr_prediction_z2_c` (intra_prediction.c:2404-2435).
///
/// **Intentionally NOT structured like `intra_pred::dr_z2_edged`** — see
/// the module doc "Findings": SVT's lbd z2 (`svt_av1_dr_prediction_z2_c`,
/// intra_prediction.c:386-415) uses an incremental accumulator (`x`/`base1`
/// carried across the row/column loops), but the hbd z2 independently
/// recomputes `x`, `y`, `base` from scratch at every `(r, c)` — a genuine
/// difference in SVT-AV1's own source, translated literally here rather
/// than reconciled with the lbd shape.
#[allow(clippy::too_many_arguments)]
pub(super) fn dr_z2_edged_hbd(
    dst: &mut [u16],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u16],
    left: &[u16],
    origin: usize,
    upsample_above: bool,
    upsample_left: bool,
    dx: i32,
    dy: i32,
    bd: u8,
) {
    let up_a = upsample_above as usize;
    let up_l = upsample_left as usize;
    // Worst-case vector reads (see `dr_z2_edged_hbd_simd_neon`): the above
    // pass touches `above[origin + base]` for `base <= base0 + step*(bw-1)`
    // with `base0 <= -1`, plus <=15 elements of chunk/tail padding; the left
    // pass touches `left[origin + base2]` for `base2 <= (bh-1)*step - 1`
    // plus the same padding. Both bounds always hold for the real
    // EDGE_BUF_LEN=160 buffers; the gates exist so synthetic callers with
    // tight slices fall back to the scalar core rather than over-read.
    if origin >= (1 << up_a).max(1 << up_l)
        && above.len() > origin + (bw - 1) * (1 << up_a) + 15
        && left.len() > origin + (bh - 1) * (1 << up_l) + 15
    {
        incant!(
            dr_z2_edged_hbd_simd(
                dst,
                dst_stride,
                bw,
                bh,
                above,
                left,
                origin,
                upsample_above,
                upsample_left,
                dx,
                dy,
                bd,
            ),
            [neon, scalar]
        );
        return;
    }
    dr_z2_edged_hbd_core(
        dst,
        dst_stride,
        bw,
        bh,
        above,
        left,
        origin,
        upsample_above,
        upsample_left,
        dx,
        dy,
        bd,
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) fn dr_z2_edged_hbd_core(
    dst: &mut [u16],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u16],
    left: &[u16],
    origin: usize,
    upsample_above: bool,
    upsample_left: bool,
    dx: i32,
    dy: i32,
    bd: u8,
) {
    debug_assert!(dx > 0 && dy > 0);
    let up_a = upsample_above as i32;
    let up_l = upsample_left as i32;
    let min_base_x = -(1i32 << up_a);
    let frac_bits_x = 6 - up_a;
    let frac_bits_y = 6 - up_l;
    for r in 0..bh {
        for c in 0..bw {
            let y = r as i32 + 1;
            let x = ((c as i32) << 6) - y * dx;
            let base = x >> frac_bits_x;
            let val = if base >= min_base_x {
                let shift = ((x * (1 << up_a)) & 0x3F) >> 1;
                let i0 = (origin as i32 + base) as usize;
                let v = above[i0] as i32 * (32 - shift) + above[i0 + 1] as i32 * shift;
                (v + 16) >> 5
            } else {
                let x2 = c as i32 + 1;
                let y2 = ((r as i32) << 6) - x2 * dy;
                let base2 = y2 >> frac_bits_y;
                debug_assert!(base2 >= -(1 << up_l));
                let shift = ((y2 * (1 << up_l)) & 0x3F) >> 1;
                let i0 = (origin as i32 + base2) as usize;
                let v = left[i0] as i32 * (32 - shift) + left[i0 + 1] as i32 * shift;
                (v + 16) >> 5
            };
            dst[r * dst_stride + c] = clip_pixel_highbd(val, bd);
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn dr_z2_edged_hbd_simd_scalar(
    _token: ScalarToken,
    dst: &mut [u16],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u16],
    left: &[u16],
    origin: usize,
    upsample_above: bool,
    upsample_left: bool,
    dx: i32,
    dy: i32,
    bd: u8,
) {
    dr_z2_edged_hbd_core(
        dst,
        dst_stride,
        bw,
        bh,
        above,
        left,
        origin,
        upsample_above,
        upsample_left,
        dx,
        dy,
        bd,
    );
}

/// Load 8 interpolation pairs starting at element `idx`: `a0[j] =
/// buf[idx + j*step]`, `a1[j] = buf[idx + j*step + 1]`. `step` is the
/// upsample stride — 1 contiguous (two overlapping loads), 2 = `vuzp`
/// deinterleave of a 16-element load.
#[cfg(target_arch = "aarch64")]
#[rite]
pub(super) fn dr_z2_hbd_load_pairs_neon(
    _token: NeonToken,
    buf: &[u16],
    idx: usize,
    step2: bool,
) -> (uint16x8_t, uint16x8_t) {
    if step2 {
        let v0: &[u16; 8] = buf[idx..idx + 8].try_into().unwrap();
        let v1: &[u16; 8] = buf[idx + 8..idx + 16].try_into().unwrap();
        let e = vld1q_u16(v0);
        let o = vld1q_u16(v1);
        (vuzp1q_u16(e, o), vuzp2q_u16(e, o))
    } else {
        let a0: &[u16; 8] = buf[idx..idx + 8].try_into().unwrap();
        let a1: &[u16; 8] = buf[idx + 1..idx + 9].try_into().unwrap();
        (vld1q_u16(a0), vld1q_u16(a1))
    }
}

/// The shared z1/z2 hbd interpolation: `(a0*(32-s) + a1*s + 16) >> 5`
/// computed as `vrshrn::<6>(a1*2s + a0*(64-2s))` (exact, see
/// `dr_z1_edged_hbd_flat_neon`), then `vmin` replicating the only reachable
/// half of `clip_pixel_highbd` (every term is non-negative).
#[cfg(target_arch = "aarch64")]
#[rite]
pub(super) fn dr_z2_hbd_interp_neon(
    _token: NeonToken,
    a0: uint16x8_t,
    a1: uint16x8_t,
    s2: u16,
    w0: u16,
    bd_max_v: uint16x8_t,
) -> uint16x8_t {
    let lo = vmlal_n_u16(vmull_n_u16(vget_low_u16(a1), s2), vget_low_u16(a0), w0);
    let hi = vmlal_n_u16(vmull_n_u16(vget_high_u16(a1), s2), vget_high_u16(a0), w0);
    vminq_u16(
        vcombine_u16(vrshrn_n_u32::<6>(lo), vrshrn_n_u32::<6>(hi)),
        bd_max_v,
    )
}

/// aarch64 arm of [`dr_z2_edged_hbd`]. C ships this as tbl-gather kernels
/// (`highbd_dr_prediction_z2_*_neon`), but the index arithmetic is AFFINE,
/// not a real gather:
///
/// - Row `r` (above side): `x = 64c - (r+1)*dx` increases by exactly 64 per
///   column, so `base(c) = base0 + c*step` (`step = 1 << up_a`) and the
///   per-lane shift `((x << up_a) & 0x3F) >> 1` is CONSTANT across the row
///   (`64*step ≡ 0 mod 64`). Above-region lanes are the row SUFFIX
///   `[c*, bw)` where `c*` is the first column with `base >= min_base_x`.
/// - Column `c` (left side): `y2 = 64r - (c+1)*dy` is affine in `r` the same
///   way, so `base2(r) = base2_0 + r*step_y` — contiguous or stride-2
///   loads down the column — and the shift is constant per column. The
///   left-region lanes are the column TAIL `[r*, bh)`.
///
/// The two regions are exact complements of the same predicate, so pass A
/// stores full row suffixes and pass B overwrites the left lanes — no
/// blend needed. Byte-exactness is per-lane: every element computes the
/// scalar core's interpolation on the scalar core's indices.
#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
pub(super) fn dr_z2_edged_hbd_simd_neon(
    _token: NeonToken,
    dst: &mut [u16],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u16],
    left: &[u16],
    origin: usize,
    upsample_above: bool,
    upsample_left: bool,
    dx: i32,
    dy: i32,
    bd: u8,
) {
    let up_a = upsample_above as i32;
    let up_l = upsample_left as i32;
    let min_base_x = -(1i32 << up_a);
    let frac_x = 6 - up_a;
    let frac_y = 6 - up_l;
    let step_x = 1usize << up_a;
    let step_y = 1usize << up_l;
    // Same max as `clip_pixel_highbd`: non-{10,12} bd groups to 255.
    let bd_max: u16 = match bd {
        10 => 1023,
        12 => 4095,
        _ => 255,
    };
    let bd_max_v = vdupq_n_u16(bd_max);

    // Pass A — above region: row suffix [c*, bw).
    for r in 0..bh {
        let x0 = -((r as i32 + 1) * dx); // x at c=0
        let base0 = x0 >> frac_x;
        let need = min_base_x - base0;
        let c_star = if need <= 0 {
            0
        } else {
            (need as usize).div_ceil(step_x).min(bw)
        };
        let n = bw - c_star;
        if n == 0 {
            continue;
        }
        let shift = ((x0 << up_a) & 0x3f) >> 1;
        let s2 = (shift * 2) as u16;
        let w0 = 64 - s2;
        // `base0` alone can be far below the window (only the suffix
        // `[c*, bw)` is an above lane) — keep the index arithmetic signed
        // until the first REAL lane's index.
        let bi = (origin as i32 + base0 + (c_star * step_x) as i32) as usize;
        let drow = r * dst_stride + c_star;
        if n >= 8 {
            let mut j = 0usize;
            while j + 8 <= n {
                let (a0, a1) = dr_z2_hbd_load_pairs_neon(_token, above, bi + j * step_x, up_a == 1);
                let out = dr_z2_hbd_interp_neon(_token, a0, a1, s2, w0, bd_max_v);
                vst1q_u16((&mut dst[drow + j..drow + j + 8]).try_into().unwrap(), out);
                j += 8;
            }
            if j < n {
                // Overlap the tail into the last full chunk — the
                // recomputed lanes hold identical values.
                let j = n - 8;
                let (a0, a1) = dr_z2_hbd_load_pairs_neon(_token, above, bi + j * step_x, up_a == 1);
                let out = dr_z2_hbd_interp_neon(_token, a0, a1, s2, w0, bd_max_v);
                vst1q_u16((&mut dst[drow + j..drow + j + 8]).try_into().unwrap(), out);
            }
        } else {
            // n < 8: one vector computes 8 lanes from bounded indices
            // (`base(c* + j)` stays within min_base..min_base+8*step);
            // only the first n lanes store.
            let (a0, a1) = dr_z2_hbd_load_pairs_neon(_token, above, bi, up_a == 1);
            let out = dr_z2_hbd_interp_neon(_token, a0, a1, s2, w0, bd_max_v);
            let mut tmp = [0u16; 8];
            vst1q_u16(&mut tmp, out);
            dst[drow..drow + n].copy_from_slice(&tmp[..n]);
        }
    }

    // Pass B — left region: column tail [r*, bh). For column c the
    // above-side base is `(64c - (r+1)*dx) >> frac_x`, decreasing in r, so
    // the left lanes are exactly the rows with
    // `(r+1)*dx > 64c - (min_base_x << frac_x)`, i.e. `r >= floor(t/dx)`.
    for c in 0..bw {
        let t = 64 * c as i32 - (min_base_x << frac_x);
        let r_star = if t < 0 { 0 } else { (t / dx) as usize }.min(bh);
        let n = bh - r_star;
        if n == 0 {
            continue;
        }
        let y20 = -((c as i32 + 1) * dy); // y2 at r=0
        let shift = ((y20 << up_l) & 0x3f) >> 1;
        let s2 = (shift * 2) as u16;
        let w0 = 64 - s2;
        // Same signed-until-real-lane rule as pass A: `y20 >> frac_y` is the
        // r=0 index, which can be far below the window while the first left
        // lane at `r*` is inside it.
        let bi = (origin as i32 + (y20 >> frac_y) + (r_star * step_y) as i32) as usize;
        let mut k = 0usize;
        while k + 8 <= n {
            let (a0, a1) = dr_z2_hbd_load_pairs_neon(_token, left, bi + k * step_y, up_l == 1);
            let out = dr_z2_hbd_interp_neon(_token, a0, a1, s2, w0, bd_max_v);
            let mut tmp = [0u16; 8];
            vst1q_u16(&mut tmp, out);
            for j in 0..8 {
                dst[(r_star + k + j) * dst_stride + c] = tmp[j];
            }
            k += 8;
        }
        if k < n {
            let (a0, a1) = dr_z2_hbd_load_pairs_neon(_token, left, bi + k * step_y, up_l == 1);
            let out = dr_z2_hbd_interp_neon(_token, a0, a1, s2, w0, bd_max_v);
            let mut tmp = [0u16; 8];
            vst1q_u16(&mut tmp, out);
            for j in 0..n - k {
                dst[(r_star + k + j) * dst_stride + c] = tmp[j];
            }
        }
    }
}

/// C `svt_av1_highbd_dr_prediction_z3_c` (C_DEFAULT/intra_prediction_c.c:
/// 64-93). Shares `intra_pred::dr_z3_edged`'s incremental-accumulator
/// structure exactly (verified line-for-line against the C).
pub(super) fn dr_z3_edged_hbd(
    dst: &mut [u16],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    left: &[u16],
    origin: usize,
    upsample_left: bool,
    dy: i32,
    bd: u8,
) {
    // Column-wise mirror of [`dr_z1_edged_hbd`]'s flat gate: the arm reads
    // `left[origin + base + r .. r + 9]` with `base + r < max_base_y`, and
    // `bh >= 8` covers one full 8-lane u16 chunk.
    if bh >= 8 && !upsample_left && left.len() > origin + bw + bh {
        incant!(
            dr_z3_edged_hbd_flat(dst, dst_stride, bw, bh, left, origin, dy, bd),
            [neon, scalar]
        );
        return;
    }
    dr_z3_edged_hbd_core(dst, dst_stride, bw, bh, left, origin, upsample_left, dy, bd);
}

#[allow(clippy::too_many_arguments)]
pub(super) fn dr_z3_edged_hbd_flat_scalar(
    _token: ScalarToken,
    dst: &mut [u16],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    left: &[u16],
    origin: usize,
    dy: i32,
    bd: u8,
) {
    dr_z3_edged_hbd_core(dst, dst_stride, bw, bh, left, origin, false, dy, bd);
}

/// aarch64 arm of [`dr_z3_edged_hbd`], non-upsampled columns: the same
/// u32 widening `a1*2s + a0*(64-2s)` + `vrshrn_n_u32::<6>` + `vmin(bd_max)`
/// interpolation as [`dr_z1_edged_hbd_flat_neon`], computed vertically then
/// scattered — strided column stores cannot vectorize, the same shape
/// [`crate::intra_pred::dr_z3_edged_flat_neon`] uses.
#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
pub(super) fn dr_z3_edged_hbd_flat_neon(
    _token: NeonToken,
    dst: &mut [u16],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    left: &[u16],
    origin: usize,
    dy: i32,
    bd: u8,
) {
    let max_base_y = (bw + bh) as i32 - 1;
    let fill = left[origin + max_base_y as usize];
    // Same max as `clip_pixel_highbd`: non-{10,12} bd groups to 255.
    let bd_max: u16 = match bd {
        10 => 1023,
        12 => 4095,
        _ => 255,
    };
    let bd_max_v = vdupq_n_u16(bd_max);
    let mut y = dy;
    for c in 0..bw {
        let base = y >> 6;
        let shift = ((y & 0x3f) >> 1) as u32;
        let s2 = (shift * 2) as u16;
        let w0 = 64 - s2;
        let bi = origin + base as usize;
        let valid = if base >= max_base_y {
            0
        } else {
            core::cmp::min(bh, (max_base_y - base) as usize)
        };
        let mut r = 0usize;
        let mut tmp = [0u16; 8];
        while r + 8 <= valid {
            let a0 = vld1q_u16(left[bi + r..bi + r + 8].try_into().unwrap());
            let a1 = vld1q_u16(left[bi + r + 1..bi + r + 9].try_into().unwrap());
            let lo = vmlal_n_u16(vmull_n_u16(vget_low_u16(a1), s2), vget_low_u16(a0), w0);
            let hi = vmlal_n_u16(vmull_n_u16(vget_high_u16(a1), s2), vget_high_u16(a0), w0);
            let out = vminq_u16(
                vcombine_u16(vrshrn_n_u32::<6>(lo), vrshrn_n_u32::<6>(hi)),
                bd_max_v,
            );
            vst1q_u16(&mut tmp, out);
            for (k, &v) in tmp.iter().enumerate() {
                dst[(r + k) * dst_stride + c] = v;
            }
            r += 8;
        }
        let sh = shift as i32;
        while r < valid {
            let v = (left[bi + r] as i32 * (32 - sh) + left[bi + r + 1] as i32 * sh + 16) >> 5;
            dst[r * dst_stride + c] = (v as u16).min(bd_max);
            r += 1;
        }
        while r < bh {
            dst[r * dst_stride + c] = fill;
            r += 1;
        }
        y += dy;
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn dr_z3_edged_hbd_core(
    dst: &mut [u16],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    left: &[u16],
    origin: usize,
    upsample_left: bool,
    dy: i32,
    bd: u8,
) {
    let up = upsample_left as i32;
    let max_base_y = ((bw + bh - 1) as i32) << up;
    let frac_bits = 6 - up;
    let base_inc = 1i32 << up;
    let mut y = dy;
    for c in 0..bw {
        let mut base = y >> frac_bits;
        let shift = ((y << up) & 0x3F) >> 1;
        let mut r = 0usize;
        while r < bh {
            if base < max_base_y {
                let val = left[origin + base as usize] as i32 * (32 - shift)
                    + left[origin + base as usize + 1] as i32 * shift;
                dst[r * dst_stride + c] = clip_pixel_highbd((val + 16) >> 5, bd);
            } else {
                let fill = left[origin + max_base_y as usize];
                while r < bh {
                    dst[r * dst_stride + c] = fill;
                    r += 1;
                }
                break;
            }
            r += 1;
            base += base_inc;
        }
        y += dy;
    }
}

/// C `svt_aom_highbd_dr_predictor` (intra_prediction.c:2437-2457), over
/// edged buffers exactly as `intra_pred::dr_predictor_edged` (origin
/// convention documented there — `above[origin - 1] == left[origin - 1]`
/// is the top-left sample).
#[allow(clippy::too_many_arguments)]
pub fn dr_predictor_edged_hbd(
    dst: &mut [u16],
    dst_stride: usize,
    above: &[u16],
    left: &[u16],
    origin: usize,
    upsample_above: bool,
    upsample_left: bool,
    width: usize,
    height: usize,
    angle: i32,
    bd: u8,
) {
    let dx = get_dx_hbd(angle);
    let dy = get_dy_hbd(angle);
    if angle > 0 && angle < 90 {
        dr_z1_edged_hbd(
            dst,
            dst_stride,
            width,
            height,
            above,
            origin,
            upsample_above,
            dx,
            bd,
        );
    } else if angle > 90 && angle < 180 {
        dr_z2_edged_hbd(
            dst,
            dst_stride,
            width,
            height,
            above,
            left,
            origin,
            upsample_above,
            upsample_left,
            dx,
            dy,
            bd,
        );
    } else if angle > 180 && angle < 270 {
        dr_z3_edged_hbd(
            dst,
            dst_stride,
            width,
            height,
            left,
            origin,
            upsample_left,
            dy,
            bd,
        );
    } else if angle == 90 {
        predict_v_hbd(dst, dst_stride, &above[origin..], width, height);
    } else if angle == 180 {
        predict_h_hbd(dst, dst_stride, &left[origin..], width, height);
    }
}

// =============================================================================
// 4. Filter-intra prediction, highbd.
// C: intra_prediction.c:2549-2595 (`svt_aom_highbd_filter_intra_predictor`).
// Mirrors `intra_pred::predict_filter_intra` exactly (same 33x33 staging
// buffer, same tap-table indexing), swapping `.clamp(0, 255)` for
// `clip_pixel_highbd(.., bd)`.
//
// PORT-NOTE(unverified): verify vs FFI parity once wired.
// =============================================================================
