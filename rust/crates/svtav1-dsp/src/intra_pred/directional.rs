use super::*;

/// Derivative table for directional prediction angles.
/// `eb_dr_intra_derivative[angle]` = 256/tan(angle) for zone 1 (0-90°).
pub(super) static DR_INTRA_DERIVATIVE: [u16; 90] = [
    0, 0, 0, 1023, 0, 0, 547, 0, 0, 372, 0, 0, 0, 0, 273, 0, 0, 215, 0, 0, 178, 0, 0, 151, 0, 0,
    132, 0, 0, 116, 0, 0, 102, 0, 0, 0, 90, 0, 0, 80, 0, 0, 71, 0, 0, 64, 0, 0, 57, 0, 0, 51, 0, 0,
    45, 0, 0, 0, 40, 0, 0, 35, 0, 0, 31, 0, 0, 27, 0, 0, 23, 0, 0, 19, 0, 0, 15, 0, 0, 0, 0, 11, 0,
    0, 7, 0, 0, 3, 0, 0,
];

/// Base angles for directional intra modes.
pub const MODE_TO_ANGLE: [i32; 8] = [
    0, // D45_PRED  → 45°
    0, // D135_PRED → 135°
    0, // D113_PRED → 113°
    0, // D157_PRED → 157°
    0, // D203_PRED → 203°
    0, // D67_PRED  → 67°
    0, 0,
];

pub(super) fn get_dx(angle: i32) -> i32 {
    if angle > 0 && angle < 90 {
        DR_INTRA_DERIVATIVE[angle as usize] as i32
    } else if angle > 90 && angle < 180 {
        DR_INTRA_DERIVATIVE[(180 - angle) as usize] as i32
    } else {
        1
    }
}

pub(super) fn get_dy(angle: i32) -> i32 {
    if angle > 90 && angle < 180 {
        DR_INTRA_DERIVATIVE[(angle - 90) as usize] as i32
    } else if angle > 180 && angle < 270 {
        DR_INTRA_DERIVATIVE[(270 - angle) as usize] as i32
    } else {
        1
    }
}

/// Directional prediction, zone 1: 0 < angle < 90.
/// Interpolates along the `above` neighbor row.
pub(super) fn dr_prediction_z1(
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u8],
    dx: i32,
) {
    let max_base_x = (bw + bh) as i32 - 1;
    for r in 0..bh {
        let x = dx * (r as i32 + 1);
        let base = x >> 6;
        let shift = ((x) & 0x3F) >> 1;

        for c in 0..bw {
            let b = base + c as i32;
            if b < max_base_x {
                let val = above[b as usize] as i32 * (32 - shift)
                    + above[(b + 1) as usize] as i32 * shift;
                dst[r * dst_stride + c] = ((val + 16) >> 5).clamp(0, 255) as u8;
            } else {
                dst[r * dst_stride + c] = above[max_base_x as usize];
            }
        }
    }
}

/// Directional prediction, zone 3: 180 < angle < 270.
/// Interpolates along the `left` neighbor column.
pub(super) fn dr_prediction_z3(
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    left: &[u8],
    dy: i32,
) {
    let max_base_y = (bw + bh) as i32 - 1;
    for c in 0..bw {
        let y = dy * (c as i32 + 1);
        let base = y >> 6;
        let shift = ((y) & 0x3F) >> 1;

        for r in 0..bh {
            let b = base + r as i32;
            if b < max_base_y {
                let val =
                    left[b as usize] as i32 * (32 - shift) + left[(b + 1) as usize] as i32 * shift;
                dst[r * dst_stride + c] = ((val + 16) >> 5).clamp(0, 255) as u8;
            } else {
                dst[r * dst_stride + c] = left[max_base_y as usize];
            }
        }
    }
}

/// Directional prediction, zone 2: 90 < angle < 180.
/// Interpolates using both `above` and `left` neighbors plus the
/// top-left corner sample.
///
/// C-exact port of libaom `av1_dr_prediction_z2_c`
/// (av1/common/reconintra.c), specialized to
/// `upsample_above == upsample_left == 0` (our sequence headers signal
/// `enable_intra_edge_filter = 0`, so upsampling never applies):
/// `min_base_x = -1`, `frac_bits_x = frac_bits_y = 6`.
///
/// The C code indexes `above[-1]` / `left[-1]` — the top-left neighbor
/// sample (`above_row[-1] == left_col[-1]` in
/// `build_directional_and_filter_intra_predictors`). Rust slices cannot
/// be indexed at -1, so that sample is passed separately as `top_left`.
pub(super) fn dr_prediction_z2(
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u8],
    left: &[u8],
    top_left: u8,
    dx: i32,
    dy: i32,
) {
    debug_assert!(dx > 0);
    debug_assert!(dy > 0);
    for r in 0..bh {
        for c in 0..bw {
            // C: int y = r + 1; int x = (c << 6) - y * dx;
            //    const int base_x = x >> frac_bits_x;
            let y = r as i32 + 1;
            let x = ((c as i32) << 6) - y * dx;
            let base_x = x >> 6;
            let val = if base_x >= -1 {
                // C: shift = ((x * (1 << upsample_above)) & 0x3F) >> 1;
                //    val = above[base_x] * (32 - shift) + above[base_x + 1] * shift;
                let shift = (x & 0x3F) >> 1;
                let p0 = if base_x < 0 {
                    top_left
                } else {
                    above[base_x as usize]
                } as i32;
                let p1 = above[(base_x + 1) as usize] as i32;
                p0 * (32 - shift) + p1 * shift
            } else {
                // C: x = c + 1; y = (r << 6) - x * dy;
                //    const int base_y = y >> frac_bits_y;
                //    assert(base_y >= min_base_y);
                let x2 = c as i32 + 1;
                let y2 = ((r as i32) << 6) - x2 * dy;
                let base_y = y2 >> 6;
                debug_assert!(base_y >= -1);
                let shift = (y2 & 0x3F) >> 1;
                let p0 = if base_y < 0 {
                    top_left
                } else {
                    left[base_y as usize]
                } as i32;
                let p1 = left[(base_y + 1) as usize] as i32;
                p0 * (32 - shift) + p1 * shift
            };
            // C: ROUND_POWER_OF_TWO(val, 5) — val is a 32-weight blend of
            // u8 samples, so (val + 16) >> 5 <= 255; no clamp in C either.
            dst[r * dst_stride + c] = ((val + 16) >> 5) as u8;
        }
    }
}

/// Predict a block using directional prediction at the given angle.
///
/// `angle` is in degrees (0-270). The 8 directional modes map to:
/// D45=45, D67=67, D113=113, D135=135, D157=157, D203=203
///
/// Mirrors libaom `dr_predictor` (av1/common/reconintra.c) with
/// `upsample_above == upsample_left == 0`.
///
/// Neighbor array requirements (all indices relative to the block origin,
/// exactly as libaom's `above_row` / `left_col` after
/// `build_directional_and_filter_intra_predictors`):
/// - zone 1 (angle < 90): `above` needs `width + height` samples
///   (block top row + top-right extension).
/// - zone 2 (90 < angle < 180): `above` needs `width`, `left` needs
///   `height`, plus `top_left` (the C `above_row[-1] == left_col[-1]`).
/// - zone 3 (angle > 180): `left` needs `width + height` samples
///   (left column + bottom-left extension).
pub fn predict_directional(
    dst: &mut [u8],
    dst_stride: usize,
    above: &[u8],
    left: &[u8],
    top_left: u8,
    width: usize,
    height: usize,
    angle: i32,
) {
    let dx = get_dx(angle);
    let dy = get_dy(angle);

    if angle > 0 && angle < 90 {
        dr_prediction_z1(dst, dst_stride, width, height, above, dx);
    } else if angle > 90 && angle < 180 {
        dr_prediction_z2(
            dst, dst_stride, width, height, above, left, top_left, dx, dy,
        );
    } else if angle > 180 && angle < 270 {
        dr_prediction_z3(dst, dst_stride, width, height, left, dy);
    } else if angle == 90 {
        // V_PRED
        predict_v(dst, dst_stride, above, width, height);
    } else if angle == 180 {
        // H_PRED
        predict_h(dst, dst_stride, left, width, height);
    }
}

// =============================================================================
// Intra edge filtering / upsampling + upsample-capable directional prediction
// (C SVT-AV1 intra_prediction.c: svt_aom_intra_edge_filter_strength,
// svt_aom_use_intra_edge_upsample, svt_av1_filter_intra_edge_c,
// filter_intra_edge_corner, svt_av1_upsample_intra_edge_c, and the
// upsample-aware svt_av1_dr_prediction_z1/z2/z3_c.)
//
// The edged buffers mirror C's `above_data[.. ]`/`left_data[..]` with the
// block origin at index `EDGE_ORIGIN` (C `above_row = above_data + 16`):
// upsampling writes p[-2], zone-2 reads above[-1]/left[-1] (the top-left
// sample) and, when upsampled, above[-2]/left[-2].
// =============================================================================

/// Origin index inside the edged above/left buffers (C `+ 16`).
pub const EDGE_ORIGIN: usize = 16;
/// Edged buffer length (C `MAX_TX_SIZE * 2 + 32` = 160).
pub const EDGE_BUF_LEN: usize = 64 * 2 + 32;

/// C `svt_aom_intra_edge_filter_strength` (intra_prediction.c:180).
#[allow(clippy::if_same_then_else)] // C intra_prediction.c:190-197 really does have two
// identical `d >= 40` arms for blk_wh <= 12 and <= 16; the port stays shape-faithful
pub fn intra_edge_filter_strength(bs0: i32, bs1: i32, delta: i32, filt_type: i32) -> i32 {
    let d = delta.abs();
    let blk_wh = bs0 + bs1;
    let mut strength = 0;
    if filt_type == 0 {
        if blk_wh <= 8 {
            if d >= 56 {
                strength = 1;
            }
        } else if blk_wh <= 12 {
            if d >= 40 {
                strength = 1;
            }
        } else if blk_wh <= 16 {
            if d >= 40 {
                strength = 1;
            }
        } else if blk_wh <= 24 {
            if d >= 8 {
                strength = 1;
            }
            if d >= 16 {
                strength = 2;
            }
            if d >= 32 {
                strength = 3;
            }
        } else if blk_wh <= 32 {
            if d >= 1 {
                strength = 1;
            }
            if d >= 4 {
                strength = 2;
            }
            if d >= 32 {
                strength = 3;
            }
        } else if d >= 1 {
            strength = 3;
        }
    } else if blk_wh <= 8 {
        if d >= 40 {
            strength = 1;
        }
        if d >= 64 {
            strength = 2;
        }
    } else if blk_wh <= 16 {
        if d >= 20 {
            strength = 1;
        }
        if d >= 48 {
            strength = 2;
        }
    } else if blk_wh <= 24 {
        if d >= 4 {
            strength = 3;
        }
    } else if d >= 1 {
        strength = 3;
    }
    strength
}

/// C `svt_aom_use_intra_edge_upsample` (intra_prediction.c:144).
pub fn use_intra_edge_upsample(bs0: i32, bs1: i32, delta: i32, filt_type: i32) -> bool {
    let d = delta.abs();
    let blk_wh = bs0 + bs1;
    if d <= 0 || d >= 40 {
        return false;
    }
    if filt_type != 0 {
        blk_wh <= 8
    } else {
        blk_wh <= 16
    }
}

/// C `svt_av1_filter_intra_edge_c` (intra_prediction.c:156) applied to
/// `p[start .. start + sz]` (C receives the pointer already offset — the
/// caller passes `origin - ab_le`).
pub fn filter_intra_edge(p: &mut [u8], start: usize, sz: usize, strength: i32) {
    if strength == 0 {
        return;
    }
    const KERNEL: [[i32; 5]; 3] = [[0, 4, 8, 4, 0], [0, 5, 6, 5, 0], [2, 4, 4, 4, 2]];
    let filt = (strength - 1) as usize;
    debug_assert!(sz <= 129);
    // 16 bytes of zero pad past the live edge so the v3 arm can make
    // unconditional 16-byte loads (only the low 8 lanes are consumed).
    let mut edge = [0u8; 129 + 16];
    edge[..sz].copy_from_slice(&p[start..start + sz]);
    #[cfg(target_arch = "x86_64")]
    if let Some(token) = X64V3Token::summon() {
        filter_intra_edge_v3(token, p, start, sz, &KERNEL[filt], &edge);
        return;
    }
    for i in 1..sz {
        let mut s = 0i32;
        for (j, &k_w) in KERNEL[filt].iter().enumerate() {
            let k = (i as i32 - 2 + j as i32).clamp(0, sz as i32 - 1) as usize;
            s += edge[k] as i32 * k_w;
        }
        p[start + i] = ((s + 8) >> 4) as u8;
    }
}

/// x86-64 v3 arm of [`filter_intra_edge`]. Rebuilding the taps' clamped
/// indexing as REPLICATE padding — `edge2[i] = edge[(i-2).clamp(0, sz-1)]`
/// — removes every clamp: `out[i] = Σ_j k_j · edge2[i + j]` for all
/// `i in 1..sz`, scalar tail included. `s <= 255 * 16` so the i16
/// accumulation and `as u8` store are exact.
#[cfg(target_arch = "x86_64")]
#[arcane]
pub(super) fn filter_intra_edge_v3(
    token: X64V3Token,
    p: &mut [u8],
    start: usize,
    sz: usize,
    kernel: &[i32; 5],
    edge: &[u8; 145],
) {
    use magetypes::simd::generic::{i16x8, u8x16};
    // Largest tap window: `edge2[i + 4 .. i + 20)` for `i <= sz - 1`,
    // i.e. index `sz + 22`.
    let mut edge2 = [0u8; 160];
    edge2[..2].fill(edge[0]);
    edge2[2..2 + sz].copy_from_slice(&edge[..sz]);
    edge2[2 + sz..2 + sz + 22].fill(edge[sz - 1]);
    let cv: [i16x8<_>; 5] = core::array::from_fn(|j| i16x8::splat(token, kernel[j] as i16));
    let rnd = i16x8::splat(token, 8);
    let mut i = 1usize;
    while i + 8 <= sz {
        let mut acc = i16x8::splat(token, 0);
        for (j, cf) in cv.iter().enumerate() {
            let v = u8x16::load(token, edge2[i + j..i + j + 16].try_into().unwrap())
                .widen_low()
                .bitcast_i16x8();
            acc += v * *cf;
        }
        let r = (acc + rnd).shr_arithmetic_uniform(4).to_array();
        for (j, rv) in r.iter().enumerate() {
            p[start + i + j] = *rv as u8;
        }
        i += 8;
    }
    while i < sz {
        let mut s = 0i32;
        for (j, &k_w) in kernel.iter().enumerate() {
            s += edge2[i + j] as i32 * k_w;
        }
        p[start + i] = ((s + 8) >> 4) as u8;
        i += 1;
    }
}

/// C `filter_intra_edge_corner` (intra_prediction.c:2356): smooths the
/// shared corner sample `above[origin-1] == left[origin-1]`.
pub fn filter_intra_edge_corner(above: &mut [u8], left: &mut [u8], origin: usize) {
    let s = (left[origin] as i32 * 5 + above[origin - 1] as i32 * 6 + above[origin] as i32 * 5 + 8)
        >> 4;
    above[origin - 1] = s as u8;
    left[origin - 1] = s as u8;
}

/// C `svt_av1_upsample_intra_edge_c` (C_DEFAULT/intra_prediction_c.c:39):
/// 2x upsample of `p[origin .. origin + sz]` in place, writing
/// `p[origin - 2 .. origin + 2 * sz - 1]` (reads `p[origin - 1]`).
pub fn upsample_intra_edge(p: &mut [u8], origin: usize, sz: usize) {
    debug_assert!(sz <= 16, "C MAX_UPSAMPLE_SZ");
    debug_assert!(origin >= 2);
    let mut input = [0u8; 16 + 3];
    input[0] = p[origin - 1];
    input[1] = p[origin - 1];
    input[2..2 + sz].copy_from_slice(&p[origin..origin + sz]);
    input[sz + 2] = p[origin + sz - 1];

    p[origin - 2] = input[0];
    for i in 0..sz {
        let s = -(input[i] as i32) + 9 * input[i + 1] as i32 + 9 * input[i + 2] as i32
            - input[i + 3] as i32;
        let s = ((s + 8) >> 4).clamp(0, 255);
        p[origin + 2 * i - 1] = s as u8;
        p[origin + 2 * i] = input[i + 2];
    }
}

/// C `svt_av1_dr_prediction_z1_c` (intra_prediction.c:351) with upsampling.
/// `above[origin + i]` is C `above[i]`.
///
/// Dispatches to a NEON arm on the NON-upsampled path, which is the one the
/// large blocks take (`svt_aom_use_intra_edge_upsample` can only return 1 for
/// `bw + bh <= 16`, C intra_prediction.c). The upsampled path and any caller
/// whose edge buffer is too short for a 16-lane load stay on the scalar core.
#[allow(clippy::too_many_arguments)]
pub(super) fn dr_z1_edged(
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u8],
    origin: usize,
    upsample_above: bool,
    dx: i32,
) {
    // The vector arm reads `above[origin + base + c + 1 .. + 16]` with
    // `base + c < max_base_x = bw + bh - 1`, so the highest index it can touch
    // is `origin + max_base_x + 1`. Anything shorter takes the scalar core.
    // `bw >= 16` is the gate, not just a guard: below it the vector loop
    // cannot run a single 16-lane chunk (`valid <= bw`), so the `incant!`
    // would summon a token and cross a target-feature boundary to execute
    // the scalar tail. MEASURED: without this the whole change is 0.992x at
    // 256x256 p6 (a REGRESSION, span entirely above 1.0) while still being
    // 1.016x at 512x512 p2 -- the small-block call rate at the fast presets
    // pays the dispatch and gets nothing back.
    //
    // Below 16 the C `4xH`/`8xH` row-vector kernel takes over. Its loads
    // top out at `edge[origin + base + 15]` (upsampled `vld1q`) or
    // `edge[origin + base + 8]` with `base < max_base`, so the guard is
    // `origin + max_base + 16` / `+ 9`. `svt_aom_use_intra_edge_upsample`
    // can only return 1 for `bw + bh <= 16`, so `max_base <= 30` whenever
    // the upsample branch runs in production and the 160-byte edged
    // buffer always satisfies the guard there; oversized or short
    // buffers still fall back to the scalar core.
    if bw == 4 || bw == 8 {
        let up = upsample_above as usize;
        let max_base = ((bw + bh - 1) << up) as usize;
        if above.len() >= origin + max_base + if upsample_above { 16 } else { 9 } {
            incant!(
                dr_z1_edged_small(dst, dst_stride, bw, bh, above, origin, upsample_above, dx),
                [neon, scalar]
            );
            return;
        }
    }
    if bw >= 16 && !upsample_above && above.len() > origin + bw + bh {
        incant!(
            dr_z1_edged_flat(dst, dst_stride, bw, bh, above, origin, dx),
            [neon, scalar]
        );
        return;
    }
    dr_z1_edged_core(dst, dst_stride, bw, bh, above, origin, upsample_above, dx);
}

pub(super) fn dr_z1_edged_flat_scalar(
    _token: ScalarToken,
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u8],
    origin: usize,
    dx: i32,
) {
    dr_z1_edged_core(dst, dst_stride, bw, bh, above, origin, false, dx);
}

/// # Why this is a hand-written per-ISA arm and not `#[magetypes]`
///
/// The kernel is `u8 -> u16 -> u8`: it widens two `u8x16` loads to `u16x8`
/// pairs, accumulates `(a0 << 5) + (a1 - a0) * shift` there, and narrows back
/// with a ROUNDING shift. magetypes 0.9.28 cannot express any of those three
/// steps, verified by source read of the local checkout
/// (`~/work/archmage/magetypes`), not from memory:
/// * `src/simd/backends/convert_int.rs` carries **only same-width bitcasts**
///   (`bitcast_u8_to_i8`, `bitcast_u16_to_i16`, ...); there is no
///   `u8x16 -> u16x8` widening in either direction, and
///   `src/simd/generic/cross_width.rs` is **f32-only** (`f32x4 <-> f32x8 <->
///   f32x16`).
/// * `U16x8Backend` has no rounding narrowing shift (`vrshrn`-shaped) and no
///   widening multiply-accumulate (`vmlal`-shaped).
/// * `u8x16`'s only reduction is `reduce_add(..) -> u8`, which wraps.
///
/// So this takes the same route `crate::me_sad` documents for the SAD family:
/// `incant!` + per-ISA `#[arcane]`. If magetypes gains integer widening
/// (`u8xN <-> u16xN`, `i16xN <-> i32xN`) plus a rounding narrowing shift, this
/// kernel and `dr_z3_edged_flat_neon` collapse into one generic body.
///
/// C `svt_av1_dr_prediction_z1_neon`'s inner loop
/// (`ASM_NEON/intra_prediction_neon.c:89`, the `_large` variant), at
/// `upsample_above == 0`.
///
/// EXACT, not an approximation. C's NEON arm rewrites the scalar
/// `(a0 * (32 - shift) + a1 * shift + 16) >> 5` as
/// `((a0 << 5) + (a1 - a0) * shift + 16) >> 5`. The two are the same integer:
/// they differ by algebra only, the true value lies in `[0, 255 * 32]`, and
/// `vsubl_u8(a1, a0)` wrapping when `a1 < a0` cancels in the `vmlaq_u16`
/// because everything is mod 2^16 and the true result fits in 16 bits. No
/// clamp is needed for the same reason (the scalar core's `.clamp(0, 255)` can
/// never fire), and `vrshrn_n_u16::<5>` IS `(v + 16) >> 5`.
#[cfg(target_arch = "aarch64")]
#[arcane]
pub(super) fn dr_z1_edged_flat_neon(
    _token: NeonToken,
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u8],
    origin: usize,
    dx: i32,
) {
    let max_base_x = (bw + bh) as i32 - 1;
    let fill = above[origin + max_base_x as usize];
    let mut x = dx;
    for r in 0..bh {
        let base = x >> 6;
        if base >= max_base_x {
            for row in dst.chunks_mut(dst_stride).skip(r).take(bh - r) {
                row[..bw].fill(fill);
            }
            return;
        }
        let bi = origin + base as usize;
        let shift = ((x & 0x3f) >> 1) as u16;
        let shift_v = vdupq_n_u16(shift);
        // Columns `c` with `base + c < max_base_x` take the interpolation;
        // the rest take `above[max_base_x]`, exactly as the scalar core does.
        let valid = core::cmp::min(bw, (max_base_x - base) as usize);
        let drow = &mut dst[r * dst_stride..r * dst_stride + bw];
        let mut c = 0;
        while c + 16 <= valid {
            let a0: &[u8; 16] = above[bi + c..bi + c + 16].try_into().unwrap();
            let a1: &[u8; 16] = above[bi + c + 1..bi + c + 17].try_into().unwrap();
            let a0v = vld1q_u8(a0);
            let a1v = vld1q_u8(a1);
            let lo = vmlaq_u16(
                vshll_n_u8::<5>(vget_low_u8(a0v)),
                vsubl_u8(vget_low_u8(a1v), vget_low_u8(a0v)),
                shift_v,
            );
            let hi = vmlaq_u16(
                vshll_n_u8::<5>(vget_high_u8(a0v)),
                vsubl_u8(vget_high_u8(a1v), vget_high_u8(a0v)),
                shift_v,
            );
            let out = vcombine_u8(vrshrn_n_u16::<5>(lo), vrshrn_n_u16::<5>(hi));
            let o: &mut [u8; 16] = (&mut drow[c..c + 16]).try_into().unwrap();
            vst1q_u8(o, out);
            c += 16;
        }
        let sh = shift as i32;
        while c < valid {
            let v = (above[bi + c] as i32 * (32 - sh) + above[bi + c + 1] as i32 * sh + 16) >> 5;
            drow[c] = v as u8;
            c += 1;
        }
        drow[c..].fill(fill);
        x += dx;
    }
}

/// One `u8x8` output vector of the shared z1/z3 small-block kernel: z1
/// row `r`, or z3 column `c` (the same computation on `left`/`dy` — C's
/// `dr_prediction_z1_WxH_internal_neon_small` is reused for both). Lane
/// `i` interpolates `edge[base + i * (1 << up)]` vs its successor, lanes
/// past the valid count take `edge[max_base]` — the fill the scalar
/// core's `else` arm writes.
///
/// SCALAR-EXACT, and deliberately NOT a transliteration of C's mask
/// count: C computes `base_max_diff = (max_base - base) >> upsample`
/// (floor), while the C scalar core interpolates column `c` whenever
/// `base + c * base_inc < max_base` (ceil). On odd `max_base - base`
/// with `upsample == 1` those differ by one lane — the x86 AVX2 kernel
/// shares NEON's floor form, so C's own ISAs disagree with C's scalar
/// on that lane. The port keeps every tier on the scalar formula:
/// `n_valid = ceil(diff / base_inc)`, so this arm and the scalar core
/// cannot diverge no matter which ISA runs.
///
/// Interpolation is `(a0 << 5) + (a1 - a0) * shift` in `u16x8` with a
/// `vrshrn_n_u16::<5>` narrow — the same mod-2^16-exact rewrite
/// [`dr_z1_edged_flat_neon`] documents.
#[cfg(target_arch = "aarch64")]
#[rite]
pub(super) fn dr_small_row_neon(
    _token: NeonToken,
    w: usize,
    edge: &[u8],
    origin: usize,
    upsample: bool,
    max_base: i32,
    x: i32,
) -> uint8x8_t {
    let up = upsample as i32;
    let fill_v = vdup_n_u8(edge[origin + max_base as usize]);
    let base = x >> (6 - up);
    if base >= max_base {
        return fill_v;
    }
    let n_valid = (((max_base - base) + (1i32 << up) - 1) >> up).min(w as i32) as u8;
    let shift = (((x << up) & 0x3f) >> 1) as u16;
    let bi = origin + base as usize;
    let (a0, a1) = if upsample {
        // One 16-byte load + uzp deinterleave == C's `vld2_u8` pair
        // (even lanes `edge[base + 2i]`, odd `edge[base + 2i + 1]`).
        let v: &[u8; 16] = edge[bi..bi + 16].try_into().unwrap();
        let q = vld1q_u8(v);
        (vget_low_u8(vuzp1q_u8(q, q)), vget_low_u8(vuzp2q_u8(q, q)))
    } else {
        (
            vld1_u8(edge[bi..bi + 8].try_into().unwrap()),
            vld1_u8(edge[bi + 1..bi + 9].try_into().unwrap()),
        )
    };
    let res = vmlaq_u16(vshll_n_u8::<5>(a0), vsubl_u8(a1, a0), vdupq_n_u16(shift));
    vbsl_u8(
        vclt_u8(vld1_u8(&Z2_IDX8), vdup_n_u8(n_valid)),
        vrshrn_n_u16::<5>(res),
        fill_v,
    )
}

/// Scalar-tier mirror of [`dr_z1_edged_small_neon`] for `incant!`.
pub(super) fn dr_z1_edged_small_scalar(
    _token: ScalarToken,
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u8],
    origin: usize,
    upsample_above: bool,
    dx: i32,
) {
    dr_z1_edged_core(dst, dst_stride, bw, bh, above, origin, upsample_above, dx);
}

/// C `dr_prediction_z1_4xH_neon` / `dr_prediction_z1_8xH_neon`: one
/// `u8x8` per row via [`dr_small_row_neon`], stored as 4 or 8 bytes.
/// Handles `upsample_above` (the deinterleaved load) — the flat arm
/// cannot — and any `bh <= 64`.
#[cfg(target_arch = "aarch64")]
#[arcane]
pub(super) fn dr_z1_edged_small_neon(
    token: NeonToken,
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u8],
    origin: usize,
    upsample_above: bool,
    dx: i32,
) {
    let max_base = ((bw + bh) as i32 - 1) << upsample_above as i32;
    let mut x = dx;
    for r in 0..bh {
        let v = dr_small_row_neon(token, bw, above, origin, upsample_above, max_base, x);
        let d = &mut dst[r * dst_stride..r * dst_stride + bw];
        if bw == 8 {
            vst1_u8(d.try_into().unwrap(), v);
        } else {
            let word = vget_lane_u32::<0>(vreinterpret_u32_u8(v));
            d.copy_from_slice(&word.to_le_bytes());
        }
        x += dx;
    }
}

/// Scalar C `svt_av1_dr_prediction_z1_c` (intra_prediction.c:351) with
/// upsampling. `above[origin + i]` is C `above[i]`. Reached through
/// [`dr_z1_edged`], which routes the flat (non-upsampled) case to a NEON arm.
#[allow(clippy::too_many_arguments)]
pub(super) fn dr_z1_edged_core(
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u8],
    origin: usize,
    upsample_above: bool,
    dx: i32,
) {
    if upsample_above {
        dr_z1_edged_core_impl::<true>(dst, dst_stride, bw, bh, above, origin, dx);
    } else {
        dr_z1_edged_core_impl::<false>(dst, dst_stride, bw, bh, above, origin, dx);
    }
}

/// [`dr_z1_edged_core`] with the upsample flag as a const generic: `base_inc`
/// folds to 1 or 2, so the non-upsampled tap walk is a plain contiguous
/// zip LLVM can vectorize.
#[allow(clippy::too_many_arguments)]
pub(super) fn dr_z1_edged_core_impl<const UA: bool>(
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u8],
    origin: usize,
    dx: i32,
) {
    let up = UA as i32;
    let max_base_x = ((bw + bh) as i32 - 1) << up;
    let frac_bits = 6 - up;
    let base_inc = 1i32 << up;
    let mut x = dx;
    for r in 0..bh {
        let base0 = x >> frac_bits;
        let shift = ((x << up) & 0x3F) >> 1;
        if base0 >= max_base_x {
            let fill = above[origin + max_base_x as usize];
            for row in dst.chunks_mut(dst_stride).skip(r).take(bh - r) {
                row[..bw].fill(fill);
            }
            return;
        }
        // `base` climbs by `base_inc` per column, so the `base < max_base_x`
        // test is decided once per row: interpolate the first `interp`
        // columns, fill the rest with the edge sample.
        let row = &mut dst[r * dst_stride..r * dst_stride + bw];
        let interp = ((max_base_x - base0 + base_inc - 1) / base_inc).min(bw as i32) as usize;
        if interp > 0 {
            let begin = origin + base0 as usize;
            let end = begin + (interp - 1) * base_inc as usize + 2;
            let keep = 32 - shift;
            let a = &above[begin..end - 1];
            let b = &above[begin + 1..end];
            if UA {
                for (out, (&p, &q)) in row[..interp]
                    .iter_mut()
                    .zip(a.iter().step_by(2).zip(b.iter().step_by(2)))
                {
                    *out = ((i32::from(p) * keep + i32::from(q) * shift + 16) >> 5).clamp(0, 255)
                        as u8;
                }
            } else {
                // `base_inc == 1`: contiguous taps, a plain zip LLVM can
                // vectorize.
                for (out, (&p, &q)) in row[..interp].iter_mut().zip(a.iter().zip(b.iter())) {
                    *out = ((i32::from(p) * keep + i32::from(q) * shift + 16) >> 5).clamp(0, 255)
                        as u8;
                }
            }
        }
        if interp < bw {
            row[interp..].fill(above[origin + max_base_x as usize]);
        }
        x += dx;
    }
}

/// C `svt_av1_dr_prediction_z2_c` (intra_prediction.c:386) with upsampling.
/// Reads `above[origin - 1 - up_above ..]` and `left[origin - 1 - up_left ..]`.
///
/// Dispatches to a NEON arm on the NON-upsampled path, which is the one every
/// block of 16 or more takes (`svt_aom_use_intra_edge_upsample` can only
/// return 1 for `bw + bh <= 16`). The scalar core stays the oracle for the
/// upsampled path and for any caller whose edge buffers are too short for a
/// 16-lane load.
#[allow(clippy::too_many_arguments)]
pub(super) fn dr_z2_edged(
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u8],
    left: &[u8],
    origin: usize,
    upsample_above: bool,
    upsample_left: bool,
    dx: i32,
    dy: i32,
) {
    debug_assert!(dx > 0 && dy > 0);
    #[cfg(target_arch = "x86_64")]
    if (4..=64).contains(&bw)
        && (4..=64).contains(&bh)
        && let Some(token) = X64V3Token::summon()
    {
        dr_z2_edged_split_v3(
            token,
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
        );
        return;
    }

    // `bw >= 16` for the reason [`dr_z1_edged`] records: pass 1 chunks
    // 16 lanes over COLUMNS, so a narrower block cannot run one vector
    // iteration. `bh` is unrestricted — pass 2's row-chunked loop just
    // never runs below 16 rows and its per-column scalar tail takes the
    // (short) left region, while pass 1 still vectorizes the bulk. C's
    // `dr_prediction_z2_WxH_neon` likewise routes every `bw >= 16` here.
    //
    // The load bounds. Pass 1 reads `above[origin + base_x + c ..= + 16]` with
    // `c + 16 <= bw` and `base_x <= -1`, so its highest index is
    // `origin + bw - 1`. Pass 2 reads `left[origin + by0 + r ..= + 16]` with
    // `r + 16 <= bh` and `by0 <= -1`, highest `origin + bh - 1`. The `+ 16`
    // in the guards is slack, not a requirement.
    if bw >= 16
        && bh <= 64
        && !upsample_above
        && !upsample_left
        && above.len() >= origin + bw + 16
        && left.len() >= origin + bh + 16
    {
        incant!(
            dr_z2_edged_flat(dst, dst_stride, bw, bh, above, left, origin, dx, dy),
            [neon, scalar]
        );
        return;
    }
    // Small blocks: C's `dr_prediction_z2_4xH_neon` / `_8xH_neon` (any
    // upsample combination). The loads reach `above[base_x .. base_x + 16)`
    // for `base_x` as low as `min_base_x - 1 - 8 * (1 << upsample_above)`
    // (only fully-masked lanes ever underflow, and the kernel clamps the
    // start index to 0 in that case) and `left[origin - 2 .. origin + 46)`,
    // so `origin >= 16` plus the length guards cover every read.
    #[cfg(target_arch = "aarch64")]
    if (bw == 4 || bw == 8)
        && origin >= 16
        && above.len() >= origin + 16
        && left.len() >= origin + 46
    {
        incant!(
            dr_z2_edged_small(
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
            ),
            [neon, scalar]
        );
        return;
    }
    dr_z2_edged_core(
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
    );
}

/// The scalar C formula switches edges at one column per row. Hoist that
/// boundary and the above-edge interpolation weight out of the pixel loop.
/// The contiguous above suffix can then auto-vectorize under AVX2 without
/// per-pixel edge selection. Specializing the two upsample flags keeps its
/// source stride constant (one or two bytes). The prefix retains C's left
/// indexing and rounding exactly; no gather primitive is needed.
#[cfg(target_arch = "x86_64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
pub(super) fn dr_z2_edged_split_v3(
    _token: X64V3Token,
    dst: &mut [u8],
    stride: usize,
    w: usize,
    h: usize,
    above: &[u8],
    left: &[u8],
    origin: usize,
    ua: bool,
    ul: bool,
    dx: i32,
    dy: i32,
) {
    match (ua, ul) {
        (false, false) => {
            dr_z2_edged_split_core::<false, false>(dst, stride, above, left, origin, w, h, dx, dy)
        }
        (false, true) => {
            dr_z2_edged_split_core::<false, true>(dst, stride, above, left, origin, w, h, dx, dy)
        }
        (true, false) => {
            dr_z2_edged_split_core::<true, false>(dst, stride, above, left, origin, w, h, dx, dy)
        }
        (true, true) => {
            dr_z2_edged_split_core::<true, true>(dst, stride, above, left, origin, w, h, dx, dy)
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
pub(super) fn dr_z2_edged_split_core<const UA: bool, const UL: bool>(
    dst: &mut [u8],
    stride: usize,
    above: &[u8],
    left: &[u8],
    origin: usize,
    w: usize,
    h: usize,
    dx: i32,
    dy: i32,
) {
    let step = 1usize << (UA as usize);
    for r in 0..h {
        let x = -((r as i32 + 1) * dx);
        let base = x >> (6 - UA as u32);
        let first =
            ((-(step as i32) - base + step as i32 - 1) >> (UA as u32)).clamp(0, w as i32) as usize;
        let row = &mut dst[r * stride..r * stride + w];
        // `y` steps by `-dy` per column; track it incrementally instead of a
        // `(c + 1) * dy` multiply per pixel.
        let mut y = ((r as i32) << 6) - dy;
        for out in row[..first].iter_mut() {
            let i = (origin as i32 + (y >> (6 - UL as u32))) as usize;
            let shift = ((y << (UL as u32)) & 63) >> 1;
            *out = ((i32::from(left[i]) * (32 - shift) + i32::from(left[i + 1]) * shift + 16) >> 5)
                as u8;
            y -= dy;
        }
        if first < w {
            let begin = (origin as i32 + base + (first * step) as i32) as usize;
            let len = w - first;
            let end = begin + (len - 1) * step + 2;
            // Constant shift down the whole row. `a`/`b` are the two taps —
            // `above[begin + j*step]` and `above[begin + j*step + 1]` — as
            // paired slices rather than `windows(2).step_by`, so the
            // `step == 1` case can vectorize.
            let a = &above[begin..end - 1];
            let b = &above[begin + 1..end];
            let shift = ((x << (UA as u32)) & 63) >> 1;
            let keep = 32 - shift;
            if UA {
                for (out, (&p, &q)) in row[first..]
                    .iter_mut()
                    .zip(a.iter().step_by(2).zip(b.iter().step_by(2)))
                {
                    *out = ((i32::from(p) * keep + i32::from(q) * shift + 16) >> 5) as u8;
                }
            } else {
                // `step == 1`: contiguous taps, a plain zip LLVM can vectorize.
                for (out, (&p, &q)) in row[first..].iter_mut().zip(a.iter().zip(b.iter())) {
                    *out = ((i32::from(p) * keep + i32::from(q) * shift + 16) >> 5) as u8;
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn dr_z2_edged_flat_scalar(
    _token: ScalarToken,
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u8],
    left: &[u8],
    origin: usize,
    dx: i32,
    dy: i32,
) {
    dr_z2_edged_core(
        dst, dst_stride, bw, bh, above, left, origin, false, false, dx, dy,
    );
}

/// NEON arm of [`dr_z2_edged`], `upsample_above == upsample_left == 0`.
///
/// # The structure, and why it is not C's
///
/// Zone 2 reads BOTH edges: cell `(r, c)` interpolates along `above` when
/// `base_x + c >= -1` and along `left` otherwise, where
/// `base_x(r) = (-(r + 1) * dx) >> 6`. C's NEON arm
/// (`ASM_NEON/intra_prediction_neon.c:505`, `dr_prediction_z2_WxH_neon`)
/// computes BOTH halves for every 16-column group and selects with
/// `vbslq_u8` over a `base_mask` table — and it reaches the left half with
/// `vqtbl4q_u8`, a 64-byte table lookup, because within a ROW the left
/// half's `base_y` and `shift` BOTH vary with the column.
///
/// This arm splits the block into the two regions instead and walks each one
/// along the axis on which it is contiguous, so there is no gather at all:
///
/// * `base_x` DECREASES with `r`, so `c0(r) = max(0, -1 - base_x(r))` — the
///   first `above` column of row `r` — is NON-DECREASING. The `above` region
///   is therefore the staircase `{ (r, c) : c >= c0(r) }` and its complement
///   is the `left` region.
/// * **Pass 1** walks the `above` region ROW-major. Along a row `base_x + c`
///   advances by one per column and `shift = ((-(r+1)*dx) & 0x3f) >> 1` is
///   CONSTANT, which is exactly [`dr_z1_edged_flat_neon`]'s kernel.
/// * **Pass 2** walks the `left` region COLUMN-major. `y = (r << 6) - (c+1)*dy`
///   and `r << 6` is an exact multiple of 64, so `base_y = r + by0` with
///   `by0 = (-(c+1)*dy) >> 6` and `shift = ((-(c+1)*dy) & 0x3f) >> 1` is
///   CONSTANT down a column — exactly [`dr_z3_edged_flat_neon`]'s kernel,
///   scatter and all. Because `c0` is non-decreasing, the first `left` row of
///   column `c` is non-decreasing too, so one two-pointer walk finds every
///   `r0(c)`.
///
/// The two regions are disjoint and cover the block, so every output byte is
/// written exactly once and neither pass needs a select.
///
/// The interpolation itself is [`dr_z1_edged_flat_neon`]'s, verbatim, with the
/// same exactness argument: `(a0 << 5) + (a1 - a0) * shift` is the same
/// integer as `a0 * (32 - shift) + a1 * shift` (mod 2^16, and the true value
/// lies in `[0, 255 * 32]`), and `vrshrn_n_u16::<5>` IS `(v + 16) >> 5`. The
/// scalar core's `.clamp(0, 255)` can never fire for the same reason.
///
/// # Why this is a hand-written per-ISA arm and not `#[magetypes]`
///
/// Same three missing primitives [`dr_z1_edged_flat_neon`] records, re-checked
/// against **the pinned version** rather than against `archmage`'s `main`:
/// `Cargo.lock` holds `magetypes 0.9.28`, and 0.9.28 is the release
/// IMMEDIATELY BEFORE archmage's `fd66480` (PR #71, uniform variable shifts +
/// saturating add/sub) and `fd3c609` (PR #74, `widen_low` / `widen_high` /
/// `narrow_saturating`). Verified by grep of the published crate source
/// (`cargo read magetypes` -> 0.9.28): `shl_uniform`, `saturating_add`,
/// `widen_low` and `narrow_saturating` are all ABSENT, and
/// `src/simd/backends/convert_int.rs` still carries same-width bitcasts only.
/// So the primitive that decides it is **`u8x16 -> u16x8` widening**, and the
/// pinned crate does not have it at any version this repo can resolve.
///
/// **CORRECTED 2026-09-04 — UPSTREAM HAS IT NOW, AND THIS KERNEL HAS BEEN
/// DEMONSTRATED AS ONE GENERIC BODY.** `imazen/archmage` `origin/main` is
/// `3cd0a04` and carries the widening/narrowing work; against a dev-only git
/// patch this kernel compiles as a single
/// `#[magetypes(define(u8x16, u16x8, i16x8), v4, v3, neon, wasm128, scalar)]`
/// body and is byte-exact against real C on every tier. What is NOT measured
/// is its SPEED. See `docs/perf-status.md`'s magetypes block for the exact
/// expression, the patch stanza and the A/B that has to run before it lands.
///
/// Two further gaps would survive even a bump to archmage `main`:
/// * `narrow_saturating` is a SATURATING narrow, not a ROUNDING one; this
///   kernel needs `vrshrn_n_u16::<5>`, i.e. `(v + 16) >> 5`, which would have
///   to be spelled as an explicit add-then-`shr_logical_const` before it;
/// * the shift here is uniform per row / per column, so PR #71's
///   `shl_uniform` is not what this kernel was missing in the first place.
#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
pub(super) fn dr_z2_edged_flat_neon(
    _token: NeonToken,
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u8],
    left: &[u8],
    origin: usize,
    dx: i32,
    dy: i32,
) {
    // `c0[r]`: the first column of row `r` that interpolates along `above`.
    // Clamped to `bw`, which means "this row is entirely `left`".
    let mut c0 = [0usize; 64];
    debug_assert!(bh <= 64);
    for (r, slot) in c0[..bh].iter_mut().enumerate() {
        let x = -((r as i32 + 1) * dx);
        *slot = ((-1 - (x >> 6)).max(0) as usize).min(bw);
    }

    // ---- pass 1: the `above` region, row-major. ----
    for (r, &c_start) in c0[..bh].iter().enumerate() {
        if c_start >= bw {
            continue;
        }
        let x = -((r as i32 + 1) * dx);
        let bi = origin as i32 + (x >> 6); // may be < 0; `bi + c >= origin - 1`
        let shift = ((x & 0x3f) >> 1) as u16;
        let shift_v = vdupq_n_u16(shift);
        let drow = &mut dst[r * dst_stride..r * dst_stride + bw];
        let mut c = c_start;
        if bw - c_start >= 16 {
            while c + 16 <= bw {
                let i0 = (bi + c as i32) as usize;
                let a0: &[u8; 16] = above[i0..i0 + 16].try_into().unwrap();
                let a1: &[u8; 16] = above[i0 + 1..i0 + 17].try_into().unwrap();
                let a0v = vld1q_u8(a0);
                let a1v = vld1q_u8(a1);
                let lo = vmlaq_u16(
                    vshll_n_u8::<5>(vget_low_u8(a0v)),
                    vsubl_u8(vget_low_u8(a1v), vget_low_u8(a0v)),
                    shift_v,
                );
                let hi = vmlaq_u16(
                    vshll_n_u8::<5>(vget_high_u8(a0v)),
                    vsubl_u8(vget_high_u8(a1v), vget_high_u8(a0v)),
                    shift_v,
                );
                let out = vcombine_u8(vrshrn_n_u16::<5>(lo), vrshrn_n_u16::<5>(hi));
                let o: &mut [u8; 16] = (&mut drow[c..c + 16]).try_into().unwrap();
                vst1q_u8(o, out);
                c += 16;
            }
            if c < bw {
                // Overlapping final chunk: `bw - 16 >= c_start` holds because
                // the segment is at least 16 wide, so it rewrites only
                // `above`-region columns.
                let c2 = bw - 16;
                let i0 = (bi + c2 as i32) as usize;
                let a0: &[u8; 16] = above[i0..i0 + 16].try_into().unwrap();
                let a1: &[u8; 16] = above[i0 + 1..i0 + 17].try_into().unwrap();
                let a0v = vld1q_u8(a0);
                let a1v = vld1q_u8(a1);
                let lo = vmlaq_u16(
                    vshll_n_u8::<5>(vget_low_u8(a0v)),
                    vsubl_u8(vget_low_u8(a1v), vget_low_u8(a0v)),
                    shift_v,
                );
                let hi = vmlaq_u16(
                    vshll_n_u8::<5>(vget_high_u8(a0v)),
                    vsubl_u8(vget_high_u8(a1v), vget_high_u8(a0v)),
                    shift_v,
                );
                let out = vcombine_u8(vrshrn_n_u16::<5>(lo), vrshrn_n_u16::<5>(hi));
                let o: &mut [u8; 16] = (&mut drow[c2..c2 + 16]).try_into().unwrap();
                vst1q_u8(o, out);
                c = bw;
            }
        }
        let sh = shift as i32;
        while c < bw {
            let i0 = (bi + c as i32) as usize;
            let v = (above[i0] as i32 * (32 - sh) + above[i0 + 1] as i32 * sh + 16) >> 5;
            drow[c] = v as u8;
            c += 1;
        }
    }

    // ---- pass 2: the `left` region, column-major. ----
    // `r0(c) = min { r : c0[r] > c }`, non-decreasing in `c`: one walk.
    let mut r0 = 0usize;
    let mut tmp = [0u8; 16];
    for c in 0..bw {
        while r0 < bh && c0[r0] <= c {
            r0 += 1;
        }
        if r0 >= bh {
            break; // and no later column has any `left` row either
        }
        let yv = -((c as i32 + 1) * dy);
        let li = origin as i32 + (yv >> 6); // may be < 0; `li + r >= origin - 1`
        let shift = ((yv & 0x3f) >> 1) as u16;
        let shift_v = vdupq_n_u16(shift);
        let mut r = r0;
        if bh - r0 >= 16 {
            loop {
                let rr = if r + 16 <= bh {
                    r
                } else if r < bh {
                    bh - 16 // overlapping final chunk, all inside the region
                } else {
                    break;
                };
                let i0 = (li + rr as i32) as usize;
                let a0: &[u8; 16] = left[i0..i0 + 16].try_into().unwrap();
                let a1: &[u8; 16] = left[i0 + 1..i0 + 17].try_into().unwrap();
                let a0v = vld1q_u8(a0);
                let a1v = vld1q_u8(a1);
                let lo = vmlaq_u16(
                    vshll_n_u8::<5>(vget_low_u8(a0v)),
                    vsubl_u8(vget_low_u8(a1v), vget_low_u8(a0v)),
                    shift_v,
                );
                let hi = vmlaq_u16(
                    vshll_n_u8::<5>(vget_high_u8(a0v)),
                    vsubl_u8(vget_high_u8(a1v), vget_high_u8(a0v)),
                    shift_v,
                );
                vst1q_u8(
                    &mut tmp,
                    vcombine_u8(vrshrn_n_u16::<5>(lo), vrshrn_n_u16::<5>(hi)),
                );
                for (k, &v) in tmp.iter().enumerate() {
                    dst[(rr + k) * dst_stride + c] = v;
                }
                r = rr + 16;
            }
        }
        let sh = shift as i32;
        while r < bh {
            let i0 = (li + r as i32) as usize;
            let v = (left[i0] as i32 * (32 - sh) + left[i0 + 1] as i32 * sh + 16) >> 5;
            dst[r * dst_stride + c] = v as u8;
            r += 1;
        }
    }
}

#[cfg(target_arch = "aarch64")]
#[allow(clippy::too_many_arguments)]
pub(super) fn dr_z2_edged_small_scalar(
    _token: ScalarToken,
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u8],
    left: &[u8],
    origin: usize,
    upsample_above: bool,
    upsample_left: bool,
    dx: i32,
    dy: i32,
) {
    dr_z2_edged_core(
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
    );
}

/// NEON arms for `bw` 4 and 8 — faithful transliterations of C
/// `dr_prediction_z2_4xH_neon` / `dr_prediction_z2_8xH_neon`
/// (`ASM_NEON/intra_prediction_neon.c:302` / `:400`), which compute BOTH
/// the above-edge and the left-edge interpolation for a whole row and
/// select per column with `vbsl` over `base_mask`. Unlike the `>= 16`
/// [`dr_z2_edged_flat_neon`] staircase split this handles upsampling too,
/// because the table gather (`vqtbl`) covers it for free.
///
/// Exactness: `vmlaq_u16` is mod-2^16 and the true interpolation value is
/// a convex combination in `[0, 255 * 32]`, so the wrapped difference
/// term cancels — same argument [`dr_z2_edged_flat_neon`] records.
/// `vrshrn_n_u16::<5>` IS `(v + 16) >> 5`. `vshl_s16` by `-frac_bits_y`
/// is the scalar `y >> frac_bits_y` on the s16 lanes; lanes whose wrapped
/// `y` exceeds s16 are always outside the left region and masked away
/// (the same lanes C's own s16 kernel computes).
#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
pub(super) fn dr_z2_edged_small_neon(
    token: NeonToken,
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u8],
    left: &[u8],
    origin: usize,
    upsample_above: bool,
    upsample_left: bool,
    dx: i32,
    dy: i32,
) {
    // The left-edge gather table covers `left[-2 .. 45]` (48 bytes), so the
    // largest usable `base_y` is 44 (`idx = base_y + 3 <= 47`). The left
    // region is a row SUFFIX (`base_x` decreases in `r`), and a used lane's
    // `base_y = (r << 6 - (c+1) * dy) >> frac_bits_y` peaks at lane 0 of
    // the last row — one scalar check bounds every lane the `vbsl` can
    // keep. Beyond it the scalar core runs (same as today). C's own 4xH
    // table WRAPS past index 17 (`vextq_u8(left_0, left_0, 14)`), which is
    // only sound while `base_y <= 14`; the port cannot copy that because
    // its oracle is the x86 kernel's scalar formula, not aarch64-C.
    let up_l = upsample_left as i32;
    let frac_bits_y = 6 - up_l;
    let min_base_x = -(1i32 << upsample_above as i32);
    let frac_bits_x = 6 - upsample_above as i32;
    let last_row_has_left = (-(bh as i32) * dx >> frac_bits_x) < min_base_x;
    let y_hi = (((bh - 1) as i32) << 6) - dy;
    if last_row_has_left && (y_hi >> frac_bits_y) > 44 {
        dr_z2_edged_core(
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
        );
        return;
    }
    match bw {
        4 => dr_z2_4xh_neon(
            token,
            dst,
            dst_stride,
            bh,
            above,
            left,
            origin,
            upsample_above,
            upsample_left,
            dx,
            dy,
        ),
        8 => dr_z2_8xh_neon(
            token,
            dst,
            dst_stride,
            bh,
            above,
            left,
            origin,
            upsample_above,
            upsample_left,
            dx,
            dy,
        ),
        _ => dr_z2_edged_core(
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
        ),
    }
}

/// Load `above` at `origin + base_x`, clamped to index 0 on underflow.
/// With `origin >= 16` an underflow implies `base_x <= -17`, at which
/// every lane of the row is left-region and the vector is discarded by
/// the `base_mask` blend, so any in-bounds bytes are a correct load.
#[cfg(target_arch = "aarch64")]
#[rite]
pub(super) fn z2_above_neon(_token: NeonToken, origin: usize, base_x: i32) -> usize {
    (origin as i32 + base_x).max(0) as usize
}

/// Column index vector `[0 .. 8)` for `vclt` mask construction.
#[cfg(target_arch = "aarch64")]
pub(super) const Z2_IDX8: [u8; 8] = [0, 1, 2, 3, 4, 5, 6, 7];

/// The shared `vqtbl3` gather table: three contiguous 16-byte loads
/// covering `left[-2 .. 45]`. The edged buffer is 160 bytes with
/// `origin >= 16`, so all three reads are in bounds.
#[cfg(target_arch = "aarch64")]
#[rite]
pub(super) fn z2_left_table_neon(_token: NeonToken, left: &[u8], origin: usize) -> uint8x16x3_t {
    let m2: &[u8; 16] = left[origin - 2..origin + 14].try_into().unwrap();
    let l14: &[u8; 16] = left[origin + 14..origin + 30].try_into().unwrap();
    let l30: &[u8; 16] = left[origin + 30..origin + 46].try_into().unwrap();
    uint8x16x3_t(vld1q_u8(m2), vld1q_u8(l14), vld1q_u8(l30))
}

/// C `dr_prediction_z2_4xH_neon`: one `uint16x8` per row, low four lanes
/// the above-edge interpolation, high four the left-edge gather, then one
/// `vmlaq` + `vrshrn` + `vbsl` + a 4-byte lane store.
#[cfg(target_arch = "aarch64")]
#[rite]
#[allow(clippy::too_many_arguments)]
pub(super) fn dr_z2_4xh_neon(
    token: NeonToken,
    dst: &mut [u8],
    dst_stride: usize,
    bh: usize,
    above: &[u8],
    left: &[u8],
    origin: usize,
    upsample_above: bool,
    upsample_left: bool,
    dx: i32,
    dy: i32,
) {
    let up_a = upsample_above as i32;
    let up_l = upsample_left as i32;
    let min_base_x = -(1i32 << up_a);
    let frac_bits_x = 6 - up_a;
    let frac_bits_y = 6 - up_l;

    let idx8 = vld1_u8(&Z2_IDX8);
    let c1f = vdup_n_u16(0x1f);
    let v_1234 = vcreate_s16(0x0004_0003_0002_0001);
    let dy64 = vdup_n_s16(dy as i16);
    let neg_fby = vdup_n_s16(-(frac_bits_y as i16));

    // 48-byte CONTIGUOUS gather table covering `left[-2 .. 45]` — unlike
    // C's wrapped 32-byte table this keeps the scalar formula's values for
    // `base_y` up to 44 (the caller guards taller reaches).
    let left_vals = z2_left_table_neon(token, left, origin);

    for r in 0..bh {
        let y = r as i32 + 1;
        let x = -y * dx;
        let base_x = x >> frac_bits_x;
        let mut base_shift = 0;
        if base_x < min_base_x - 1 {
            base_shift = (min_base_x - base_x - 1) >> up_a;
        }
        let base_min_diff = ((min_base_x - base_x + up_a) >> up_a).clamp(0, 4);

        let mut a0_x = vdupq_n_u16(0);
        let mut a1_x = vdupq_n_u16(0);
        let mut sh_lo = vdup_n_u16(0);
        let mut sh_hi = vdup_n_u16(0);

        if base_shift <= 4 {
            let lo = z2_above_neon(token, origin, base_x);
            if upsample_above {
                // `vld2` deinterleave via load + unzip.
                let a: &[u8; 16] = above[lo..lo + 16].try_into().unwrap();
                let v = vld1q_u8(a);
                a0_x = vmovl_u8(vget_low_u8(vuzp1q_u8(v, v)));
                a1_x = vmovl_u8(vget_low_u8(vuzp2q_u8(v, v)));
                sh_lo = vdup_n_u16((x & 0x1f) as u16);
            } else {
                let a: &[u8; 8] = above[lo..lo + 8].try_into().unwrap();
                let b: &[u8; 8] = above[lo + 1..lo + 9].try_into().unwrap();
                a0_x = vmovl_u8(vld1_u8(a));
                a1_x = vmovl_u8(vld1_u8(b));
                sh_lo = vdup_n_u16(((x & 0x3f) >> 1) as u16);
            }
        }

        if base_x < min_base_x {
            let y_c64 = vmls_s16(vdup_n_s16((r << 6) as i16), v_1234, dy64);
            let base_y = vshl_s16(y_c64, neg_fby);
            let idx0 = vreinterpret_u8_s16(vadd_s16(base_y, vdup_n_s16(2)));
            let idx1 = vreinterpret_u8_s16(vadd_s16(base_y, vdup_n_s16(3)));
            // Gathered byte + a zero byte make each u16 lane.
            let a0_y = vtrn1_u8(vqtbl3_u8(left_vals, idx0), vdup_n_u8(0));
            let a1_y = vtrn1_u8(vqtbl3_u8(left_vals, idx1), vdup_n_u8(0));
            a0_x = vcombine_u16(vget_low_u16(a0_x), vreinterpret_u16_u8(a0_y));
            a1_x = vcombine_u16(vget_low_u16(a1_x), vreinterpret_u16_u8(a1_y));
            sh_hi = if upsample_left {
                vand_u16(vreinterpret_u16_s16(y_c64), c1f)
            } else {
                vand_u16(vshr_n_u16::<1>(vreinterpret_u16_s16(y_c64)), c1f)
            };
        }

        let shift = vcombine_u16(sh_lo, sh_hi);
        let res = vmlaq_u16(vshlq_n_u16::<5>(a0_x), vsubq_u16(a1_x, a0_x), shift);
        let resx = vrshrn_n_u16::<5>(res);
        let resy = vext_u8::<4>(resx, vdup_n_u8(0));
        let mask = vclt_u8(idx8, vdup_n_u8(base_min_diff as u8));
        let resxy = vbsl_u8(mask, resy, resx);
        let word = vget_lane_u32::<0>(vreinterpret_u32_u8(resxy));
        dst[r * dst_stride..r * dst_stride + 4].copy_from_slice(&word.to_le_bytes());
    }
}

/// C `dr_prediction_z2_8xH_neon`: `uint16x8x2` per row — `val[0]` the
/// above-edge interpolation, `val[1]` the 48-entry `vqtbl3q` left-edge
/// gather — then `vmlaq` + `vrshrn` + `vbsl` + an 8-byte store.
#[cfg(target_arch = "aarch64")]
#[rite]
#[allow(clippy::too_many_arguments)]
pub(super) fn dr_z2_8xh_neon(
    token: NeonToken,
    dst: &mut [u8],
    dst_stride: usize,
    bh: usize,
    above: &[u8],
    left: &[u8],
    origin: usize,
    upsample_above: bool,
    upsample_left: bool,
    dx: i32,
    dy: i32,
) {
    let up_a = upsample_above as i32;
    let up_l = upsample_left as i32;
    let min_base_x = -(1i32 << up_a);
    let frac_bits_x = 6 - up_a;
    let frac_bits_y = 6 - up_l;

    let idx8 = vld1_u8(&Z2_IDX8);
    let c1f = vdupq_n_u16(0x1f);
    let c1234 = vcombine_s16(
        vcreate_s16(0x0004_0003_0002_0001),
        vcreate_s16(0x0008_0007_0006_0005),
    );
    let dy128 = vdupq_n_s16(dy as i16);
    let neg_fby = vdupq_n_s16(-(frac_bits_y as i16));

    // Same contiguous 48-byte gather table as the 4xH kernel.
    let left_vals = z2_left_table_neon(token, left, origin);

    for r in 0..bh {
        let y = r as i32 + 1;
        let x = -y * dx;
        let base_x = x >> frac_bits_x;
        let mut base_shift = 0;
        if base_x < min_base_x - 1 {
            base_shift = (min_base_x - base_x - 1) >> up_a;
        }
        let base_min_diff = ((min_base_x - base_x + up_a) >> up_a).clamp(0, 8);

        let resx = if base_shift <= 8 {
            let lo = z2_above_neon(token, origin, base_x);
            let (a0, a1, sx) = if upsample_above {
                let a: &[u8; 16] = above[lo..lo + 16].try_into().unwrap();
                let v = vld1q_u8(a);
                (
                    vget_low_u8(vuzp1q_u8(v, v)),
                    vget_low_u8(vuzp2q_u8(v, v)),
                    (x & 0x1f) as u16,
                )
            } else {
                let a: &[u8; 8] = above[lo..lo + 8].try_into().unwrap();
                let b: &[u8; 8] = above[lo + 1..lo + 9].try_into().unwrap();
                (vld1_u8(a), vld1_u8(b), ((x & 0x3f) >> 1) as u16)
            };
            vrshrn_n_u16::<5>(vmlaq_u16(
                vshll_n_u8::<5>(a0),
                vsubl_u8(a1, a0),
                vdupq_n_u16(sx),
            ))
        } else {
            vdup_n_u8(0)
        };

        let mut resxy = resx;
        if base_x < min_base_x {
            let y_c128 = vmlsq_s16(vdupq_n_s16((r << 6) as i16), c1234, dy128);
            let base_y = vshlq_s16(y_c128, neg_fby);
            let idx0 = vreinterpretq_u8_s16(vaddq_s16(base_y, vdupq_n_s16(2)));
            let idx1 = vreinterpretq_u8_s16(vaddq_s16(base_y, vdupq_n_s16(3)));
            // Even bytes of each s16 lane are the gather indices; uzp1 packs
            // the eight a0 indices then the eight a1 indices.
            let idx01 = vuzp1q_u8(idx0, idx1);
            let a01 = vqtbl3q_u8(left_vals, idx01);
            let a0_y = vget_low_u8(a01);
            let a1_y = vget_high_u8(a01);
            let sh_y = if upsample_left {
                vandq_u16(vreinterpretq_u16_s16(y_c128), c1f)
            } else {
                vandq_u16(vshrq_n_u16::<1>(vreinterpretq_u16_s16(y_c128)), c1f)
            };
            let resy =
                vrshrn_n_u16::<5>(vmlaq_u16(vshll_n_u8::<5>(a0_y), vsubl_u8(a1_y, a0_y), sh_y));
            let mask = vclt_u8(idx8, vdup_n_u8(base_min_diff as u8));
            resxy = vbsl_u8(mask, resy, resx);
        }

        let d: &mut [u8; 8] = (&mut dst[r * dst_stride..r * dst_stride + 8])
            .try_into()
            .unwrap();
        vst1_u8(d, resxy);
    }
}

/// Scalar C `svt_av1_dr_prediction_z2_c` (intra_prediction.c:386) with
/// upsampling. Reached through [`dr_z2_edged`], which routes the flat
/// (non-upsampled) case to a NEON arm.
#[allow(clippy::too_many_arguments)]
pub(super) fn dr_z2_edged_core(
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    above: &[u8],
    left: &[u8],
    origin: usize,
    upsample_above: bool,
    upsample_left: bool,
    dx: i32,
    dy: i32,
) {
    debug_assert!(dx > 0 && dy > 0);
    let up_a = upsample_above as i32;
    let up_l = upsample_left as i32;
    let min_base_x = -(1i32 << up_a);
    let frac_bits_x = 6 - up_a;
    let frac_bits_y = 6 - up_l;
    let base_inc_x = 1i32 << up_a;
    let mut x = -dx;
    for r in 0..bh {
        let mut base1 = x >> frac_bits_x;
        let mut y = ((r as i32) << 6) - dy;
        for c in 0..bw {
            let val = if base1 >= min_base_x {
                let shift1 = ((x * (1 << up_a)) & 0x3F) >> 1;
                let i0 = (origin as i32 + base1) as usize;
                above[i0] as i32 * (32 - shift1) + above[i0 + 1] as i32 * shift1
            } else {
                let base2 = y >> frac_bits_y;
                debug_assert!(base2 >= -(1 << up_l));
                let shift2 = ((y * (1 << up_l)) & 0x3F) >> 1;
                let i0 = (origin as i32 + base2) as usize;
                left[i0] as i32 * (32 - shift2) + left[i0 + 1] as i32 * shift2
            };
            dst[r * dst_stride + c] = (((val + 16) >> 5).clamp(0, 255)) as u8;
            base1 += base_inc_x;
            y -= dy;
        }
        x -= dx;
    }
}

/// C `svt_av1_dr_prediction_z3_c` (intra_prediction.c:321) with upsampling.
///
/// Dispatches to a NEON arm on the NON-upsampled path, which is the one large
/// blocks take. The output is COLUMN-major here (`dst[r * stride + c]` for a
/// fixed `c`), so the arm vectorises the arithmetic 16 rows at a time into a
/// stack staging buffer and scatters it out; C's NEON arm reaches the same
/// place by building a tile and transposing it. The scatter is the same number
/// of byte stores the scalar core does — what is saved is the interpolation.
#[allow(clippy::too_many_arguments)]
pub(super) fn dr_z3_edged(
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    left: &[u8],
    origin: usize,
    upsample_left: bool,
    dy: i32,
) {
    // The vector arm reads `left[origin + base + r + 1 .. + 16]` with
    // `base + r < max_base_y = bw + bh - 1`, so the highest index it can touch
    // is `origin + max_base_y + 1`.
    // `bh >= 16` for the same reason [`dr_z1_edged`] gates on `bw >= 16`:
    // this arm's 16-lane chunk walks ROWS, so below 16 rows it cannot run
    // one and the dispatch is pure overhead.
    //
    // `bh` of 4 or 8 takes the shared small kernel + a transposed store —
    // C's z3 small shapes. Same buffer guard as [`dr_z1_edged`]'s: the
    // loads top out at `edge[origin + max_base + {15,8}]`, and the
    // upsample branch only runs for `bw + bh <= 16` in production.
    if (bh == 4 || bh == 8) && (bw == 4 || bw % 8 == 0) && bw <= 64 {
        let up = upsample_left as usize;
        let max_base = ((bw + bh - 1) << up) as usize;
        if left.len() >= origin + max_base + if upsample_left { 16 } else { 9 } {
            incant!(
                dr_z3_edged_small(dst, dst_stride, bw, bh, left, origin, upsample_left, dy),
                [neon, scalar]
            );
            return;
        }
    }
    if bh >= 16 && !upsample_left && left.len() > origin + bw + bh {
        incant!(
            dr_z3_edged_flat(dst, dst_stride, bw, bh, left, origin, dy),
            [neon, scalar]
        );
        return;
    }
    dr_z3_edged_core(dst, dst_stride, bw, bh, left, origin, upsample_left, dy);
}

pub(super) fn dr_z3_edged_flat_scalar(
    _token: ScalarToken,
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    left: &[u8],
    origin: usize,
    dy: i32,
) {
    dr_z3_edged_core(dst, dst_stride, bw, bh, left, origin, false, dy);
}

/// NEON arm of [`dr_z3_edged`], `upsample_left == 0`.
///
/// Exactness is [`dr_z1_edged_flat_neon`]'s, verbatim: the same
/// `(a0 << 5) + (a1 - a0) * shift` rewrite of the same two-tap interpolation,
/// with the same mod-2^16 argument and the same `vrshrn_n_u16::<5>` rounding
/// narrow. Only the traversal differs — `base` walks with the ROW here, so a
/// 16-lane load covers 16 consecutive rows of one column.
#[cfg(target_arch = "aarch64")]
#[arcane]
pub(super) fn dr_z3_edged_flat_neon(
    _token: NeonToken,
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    left: &[u8],
    origin: usize,
    dy: i32,
) {
    let max_base_y = (bw + bh) as i32 - 1;
    let fill = left[origin + max_base_y as usize];
    let mut y = dy;
    for c in 0..bw {
        let base = y >> 6;
        let shift = ((y & 0x3f) >> 1) as u16;
        let shift_v = vdupq_n_u16(shift);
        let bi = origin + base as usize;
        // Rows `r` with `base + r < max_base_y` interpolate; the rest take
        // `left[max_base_y]`, exactly as the scalar core does (its inner
        // `while` fills the remainder of the column and breaks).
        let valid = if base >= max_base_y {
            0
        } else {
            core::cmp::min(bh, (max_base_y - base) as usize)
        };
        let mut r = 0usize;
        let mut tmp = [0u8; 16];
        while r + 16 <= valid {
            let a0: &[u8; 16] = left[bi + r..bi + r + 16].try_into().unwrap();
            let a1: &[u8; 16] = left[bi + r + 1..bi + r + 17].try_into().unwrap();
            let a0v = vld1q_u8(a0);
            let a1v = vld1q_u8(a1);
            let lo = vmlaq_u16(
                vshll_n_u8::<5>(vget_low_u8(a0v)),
                vsubl_u8(vget_low_u8(a1v), vget_low_u8(a0v)),
                shift_v,
            );
            let hi = vmlaq_u16(
                vshll_n_u8::<5>(vget_high_u8(a0v)),
                vsubl_u8(vget_high_u8(a1v), vget_high_u8(a0v)),
                shift_v,
            );
            vst1q_u8(
                &mut tmp,
                vcombine_u8(vrshrn_n_u16::<5>(lo), vrshrn_n_u16::<5>(hi)),
            );
            for (k, &v) in tmp.iter().enumerate() {
                dst[(r + k) * dst_stride + c] = v;
            }
            r += 16;
        }
        let sh = shift as i32;
        while r < valid {
            let v = (left[bi + r] as i32 * (32 - sh) + left[bi + r + 1] as i32 * sh + 16) >> 5;
            dst[r * dst_stride + c] = v as u8;
            r += 1;
        }
        while r < bh {
            dst[r * dst_stride + c] = fill;
            r += 1;
        }
        y += dy;
    }
}

/// C `transpose4x8_8x4_neon`: four `u8x8` column vectors (each = one z3
/// column, rows in lanes) become four `u32x2`; element `r / 2` lane
/// `r % 2` is output row `r` (4 bytes). `bh == 4` callers use only the
/// first two elements (C's `_low` variant stops after `d[0]`).
#[cfg(target_arch = "aarch64")]
#[rite]
pub(super) fn transpose4x8_8x4_neon(_token: NeonToken, x: &[uint8x8_t; 4]) -> [uint32x2_t; 4] {
    let w0 = vzip_u8(x[0], x[1]);
    let w1 = vzip_u8(x[2], x[3]);
    let d0 = vzip_u16(vreinterpret_u16_u8(w0.0), vreinterpret_u16_u8(w1.0));
    let d1 = vzip_u16(vreinterpret_u16_u8(w0.1), vreinterpret_u16_u8(w1.1));
    [
        vreinterpret_u32_u16(d0.0),
        vreinterpret_u32_u16(d0.1),
        vreinterpret_u32_u16(d1.0),
        vreinterpret_u32_u16(d1.1),
    ]
}

/// C `transpose8x8_neon`: eight `u8x8` column vectors become eight
/// `u32x2` output rows (element `r` = row `r`, 8 bytes). `bh == 4`
/// callers keep only the first four (C's `transpose8x8_low_neon`).
#[cfg(target_arch = "aarch64")]
#[rite]
pub(super) fn transpose8x8_neon(_token: NeonToken, x: &[uint8x8_t; 8]) -> [uint32x2_t; 8] {
    let w0 = vzip_u8(x[0], x[1]);
    let w1 = vzip_u8(x[2], x[3]);
    let w2 = vzip_u8(x[4], x[5]);
    let w3 = vzip_u8(x[6], x[7]);
    let w4 = vzip_u16(vreinterpret_u16_u8(w0.0), vreinterpret_u16_u8(w1.0));
    let w5 = vzip_u16(vreinterpret_u16_u8(w2.0), vreinterpret_u16_u8(w3.0));
    let w6 = vzip_u16(vreinterpret_u16_u8(w0.1), vreinterpret_u16_u8(w1.1));
    let w7 = vzip_u16(vreinterpret_u16_u8(w2.1), vreinterpret_u16_u8(w3.1));
    let d0 = vzip_u32(vreinterpret_u32_u16(w4.0), vreinterpret_u32_u16(w5.0));
    let d1 = vzip_u32(vreinterpret_u32_u16(w4.1), vreinterpret_u32_u16(w5.1));
    let d2 = vzip_u32(vreinterpret_u32_u16(w6.0), vreinterpret_u32_u16(w7.0));
    let d3 = vzip_u32(vreinterpret_u32_u16(w6.1), vreinterpret_u32_u16(w7.1));
    [d0.0, d0.1, d1.0, d1.1, d2.0, d2.1, d3.0, d3.1]
}

/// Scalar-tier mirror of [`dr_z3_edged_small_neon`] for `incant!`.
pub(super) fn dr_z3_edged_small_scalar(
    _token: ScalarToken,
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    left: &[u8],
    origin: usize,
    upsample_left: bool,
    dy: i32,
) {
    dr_z3_edged_core(dst, dst_stride, bw, bh, left, origin, upsample_left, dy);
}

/// C's z3 small shapes (`dr_prediction_z3_{4x4,8x8,4x8,8x4,16x8,16x4,
/// 32x8,32x4}_neon`): the shared [`dr_small_row_neon`] computes one
/// `u8x8` per COLUMN (`left`/`dy` playing the z1 roles), then the
/// vzip-transpose helpers store rows. Covers every `bw` that is 4 or a
/// multiple of 8 with `bh` of 4 or 8 — the union of C's small switch
/// arms, same semantics.
#[cfg(target_arch = "aarch64")]
#[arcane]
pub(super) fn dr_z3_edged_small_neon(
    token: NeonToken,
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    left: &[u8],
    origin: usize,
    upsample_left: bool,
    dy: i32,
) {
    let max_base = ((bw + bh) as i32 - 1) << upsample_left as i32;
    let mut vecs = [vdup_n_u8(0); 64];
    let mut y = dy;
    for v in vecs.iter_mut().take(bw) {
        *v = dr_small_row_neon(token, bh, left, origin, upsample_left, max_base, y);
        y += dy;
    }
    // vecs[c][r] = dst[r][c]; store transposed.
    if bw == 4 {
        let rows = transpose4x8_8x4_neon(token, vecs[..4].try_into().unwrap());
        for r in 0..bh {
            let word = if r % 2 == 0 {
                vget_lane_u32::<0>(rows[r / 2])
            } else {
                vget_lane_u32::<1>(rows[r / 2])
            };
            dst[r * dst_stride..r * dst_stride + 4].copy_from_slice(&word.to_le_bytes());
        }
    } else {
        for g in 0..bw / 8 {
            let rows = transpose8x8_neon(token, vecs[g * 8..g * 8 + 8].try_into().unwrap());
            for (r, &rv) in rows.iter().enumerate().take(bh) {
                let d: &mut [u8; 8] = (&mut dst
                    [r * dst_stride + g * 8..r * dst_stride + g * 8 + 8])
                    .try_into()
                    .unwrap();
                vst1_u8(d, vreinterpret_u8_u32(rv));
            }
        }
    }
}

/// Scalar C `svt_av1_dr_prediction_z3_c` (intra_prediction.c:321) with
/// upsampling. Reached through [`dr_z3_edged`], which routes the flat
/// (non-upsampled) case to a NEON arm.
#[allow(clippy::too_many_arguments)]
pub(super) fn dr_z3_edged_core(
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    left: &[u8],
    origin: usize,
    upsample_left: bool,
    dy: i32,
) {
    if upsample_left {
        dr_z3_edged_core_impl::<true>(dst, dst_stride, bw, bh, left, origin, dy);
    } else {
        dr_z3_edged_core_impl::<false>(dst, dst_stride, bw, bh, left, origin, dy);
    }
}

/// [`dr_z3_edged_core`] with the upsample flag const-generic, matching
/// [`dr_z1_edged_core_impl`].
#[allow(clippy::too_many_arguments)]
pub(super) fn dr_z3_edged_core_impl<const UL: bool>(
    dst: &mut [u8],
    dst_stride: usize,
    bw: usize,
    bh: usize,
    left: &[u8],
    origin: usize,
    dy: i32,
) {
    let up = UL as i32;
    let max_base_y = ((bw + bh - 1) as i32) << up;
    let frac_bits = 6 - up;
    let base_inc = 1i32 << up;
    let mut y = dy;
    for c in 0..bw {
        let base0 = y >> frac_bits;
        let shift = ((y << up) & 0x3F) >> 1;
        // `base` climbs by `base_inc` per row, so the `base < max_base_y`
        // test is decided once per column: interpolate the first `interp`
        // rows, fill the rest with the edge sample.
        let interp = ((max_base_y - base0 + base_inc - 1) / base_inc).clamp(0, bh as i32) as usize;
        let keep = 32 - shift;
        let mut base = base0;
        for r in 0..interp {
            let val = left[origin + base as usize] as i32 * keep
                + left[origin + base as usize + 1] as i32 * shift;
            dst[r * dst_stride + c] = ((val + 16) >> 5).clamp(0, 255) as u8;
            base += base_inc;
        }
        if interp < bh {
            let fill = left[origin + max_base_y as usize];
            for r in interp..bh {
                dst[r * dst_stride + c] = fill;
            }
        }
        y += dy;
    }
}

/// C `svt_aom_dr_predictor` over edged buffers (`above[origin + i]` = C
/// `above_row[i]`, `left[origin + i]` = C `left_col[i]`;
/// `above[origin - 1] == left[origin - 1]` is the top-left sample). The
/// upsample flags select the C kernels' upsampled indexing exactly.
#[allow(clippy::too_many_arguments)]
pub fn dr_predictor_edged(
    dst: &mut [u8],
    dst_stride: usize,
    above: &[u8],
    left: &[u8],
    origin: usize,
    upsample_above: bool,
    upsample_left: bool,
    width: usize,
    height: usize,
    angle: i32,
) {
    let dx = get_dx(angle);
    let dy = get_dy(angle);
    if angle > 0 && angle < 90 {
        dr_z1_edged(
            dst,
            dst_stride,
            width,
            height,
            above,
            origin,
            upsample_above,
            dx,
        );
    } else if angle > 90 && angle < 180 {
        dr_z2_edged(
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
        );
    } else if angle > 180 && angle < 270 {
        dr_z3_edged(
            dst,
            dst_stride,
            width,
            height,
            left,
            origin,
            upsample_left,
            dy,
        );
    } else if angle == 90 {
        predict_v(dst, dst_stride, &above[origin..], width, height);
    } else if angle == 180 {
        predict_h(dst, dst_stride, &left[origin..], width, height);
    }
}

/// Get smooth weight table for a given block dimension.
///
/// Weights decrease from 255 (top/left edge) to approximately 0 (bottom/right edge).
/// These are the Q8 weights from the AV1 spec.
pub(super) fn smooth_weights(n: usize) -> &'static [u8] {
    match n {
        4 => &SM_WEIGHTS_4,
        8 => &SM_WEIGHTS_8,
        16 => &SM_WEIGHTS_16,
        32 => &SM_WEIGHTS_32,
        64 => &SM_WEIGHTS_64,
        _ => &SM_WEIGHTS_4, // fallback
    }
}

// Smooth weight tables from the AV1 spec (Q8, 256 = 1.0)
pub(super) static SM_WEIGHTS_4: [u8; 4] = [255, 149, 85, 64];
pub(super) static SM_WEIGHTS_8: [u8; 8] = [255, 197, 146, 105, 73, 50, 37, 32];
pub(super) static SM_WEIGHTS_16: [u8; 16] = [
    255, 225, 196, 170, 145, 123, 102, 84, 68, 54, 43, 33, 26, 20, 17, 16,
];
pub(super) static SM_WEIGHTS_32: [u8; 32] = [
    255, 240, 225, 210, 196, 182, 169, 157, 145, 133, 122, 111, 101, 92, 83, 74, 66, 59, 52, 45,
    39, 34, 29, 25, 21, 17, 14, 12, 10, 9, 8, 8,
];
pub(super) static SM_WEIGHTS_64: [u8; 64] = [
    255, 248, 240, 233, 225, 218, 210, 203, 196, 189, 182, 176, 169, 163, 156, 150, 144, 138, 133,
    127, 121, 116, 111, 106, 101, 96, 91, 86, 82, 77, 73, 69, 65, 61, 57, 54, 50, 47, 44, 41, 38,
    35, 32, 29, 27, 25, 22, 20, 18, 16, 15, 13, 12, 10, 9, 8, 7, 6, 6, 5, 5, 4, 4, 4,
];

// =============================================================================
// Filter-intra prediction
// Ported from svt_av1_filter_intra_predictor_c in filterintra_c.c
// =============================================================================
