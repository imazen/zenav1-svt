//! Masked-compound and wedge-search primitives.
//!
//! Ported from SVT-AV1 v4.2.0:
//! * `Source/Lib/Codec/inter_prediction.c` —
//!   `svt_aom_is_masked_compound_type` (:34),
//!   `svt_aom_highbd_subtract_block_c` (:38), `svt_aom_subtract_block_c` (:55),
//!   `diffwtd_mask` (:68), `diffwtd_mask_highbd` (:79),
//!   `svt_av1_build_compound_diffwtd_mask_highbd_c` (:139),
//!   `svt_av1_build_compound_diffwtd_mask_c` (:154),
//!   `svt_av1_wedge_sse_from_residuals_c` (:2457),
//!   `svt_aom_highbd_blend_a64_hmask_16bit_c` (:2500),
//!   `svt_aom_sum_squares_i16_c` (:2522).
//! * `Source/Lib/Codec/enc_inter_prediction.c` —
//!   `svt_av1_wedge_compute_delta_squares_c` (:375),
//!   `svt_av1_wedge_sign_from_residuals_c` (:414),
//!   `svt_aom_highbd_sse_c` (:597), `svt_aom_sse_c` (:612).
//!
//! None of these had a counterpart in the port. `svtav1-dsp/src/residual.rs`
//! is `i32`-slice based (`residual_i32`) and the coefficient-domain
//! `svt_full_distortion_kernel32_bits_c`; `svt_aom_sse_c` is pixel-domain over
//! two 8-bit planes, a different function that happens to share a name stem.

/// `WEDGE_WEIGHT_BITS` (definitions.h:1281).
pub const WEDGE_WEIGHT_BITS: u32 = 6;
/// `MAX_MASK_VALUE` (inter_prediction.c:2427) — `1 << WEDGE_WEIGHT_BITS`.
pub const MAX_MASK_VALUE: i32 = 1 << WEDGE_WEIGHT_BITS;
/// `AOM_BLEND_A64_ROUND_BITS` (definitions.h:1268).
pub const AOM_BLEND_A64_ROUND_BITS: i32 = 6;
/// `AOM_BLEND_A64_MAX_ALPHA` (definitions.h:1269) — 64.
pub const AOM_BLEND_A64_MAX_ALPHA: i32 = 1 << AOM_BLEND_A64_ROUND_BITS;
/// `DIFF_FACTOR_LOG2` (definitions.h:1273).
pub const DIFF_FACTOR_LOG2: i32 = 4;
/// `DIFF_FACTOR` (definitions.h:1274) — 16.
pub const DIFF_FACTOR: i32 = 1 << DIFF_FACTOR_LOG2;

/// `CompoundType` (definitions.h:1259-1265).
///
/// TRAP, measured the hard way: the order is AVERAGE, DISTWTD, **WEDGE**,
/// **DIFFWTD** — wedge comes FIRST. The obvious reading (diffwtd before wedge,
/// matching the order the two are usually discussed in) transposes the two
/// masked types, and `av1_get_compound_type_mask` then serves the wedge table
/// where the segmentation mask belongs and vice versa. This port had them
/// swapped until `build_masked_compound_no_round_matches_c` failed; the
/// `svt_aom_is_masked_compound_type` cell could NOT catch it, because that
/// predicate is true for both values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CompoundType {
    /// `COMPOUND_AVERAGE`
    Average = 0,
    /// `COMPOUND_DISTWTD`
    DistWtd = 1,
    /// `COMPOUND_WEDGE`
    Wedge = 2,
    /// `COMPOUND_DIFFWTD`
    DiffWtd = 3,
}

/// `svt_aom_is_masked_compound_type` (inter_prediction.c:34).
pub fn is_masked_compound_type(t: CompoundType) -> bool {
    matches!(t, CompoundType::Wedge | CompoundType::DiffWtd)
}

/// `DIFFWTD_MASK_TYPE` (definitions.h:1296).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DiffwtdMaskType {
    /// `DIFFWTD_38`
    D38 = 0,
    /// `DIFFWTD_38_INV`
    D38Inv = 1,
}

#[inline]
fn round_power_of_two_u64(value: u64, n: u32) -> u64 {
    if n == 0 {
        value
    } else {
        (value + (1u64 << (n - 1))) >> n
    }
}

#[inline]
fn round_power_of_two(value: i32, n: i32) -> i32 {
    if n == 0 {
        value
    } else {
        (value + (1 << (n - 1))) >> n
    }
}

/// `AOM_BLEND_A64(a, v0, v1)` (definitions.h:1271).
#[inline]
pub fn aom_blend_a64(a: i32, v0: i32, v1: i32) -> i32 {
    round_power_of_two(
        a * v0 + (AOM_BLEND_A64_MAX_ALPHA - a) * v1,
        AOM_BLEND_A64_ROUND_BITS,
    )
}

/// `svt_aom_subtract_block_c` (inter_prediction.c:55) — 8-bit
/// source-minus-prediction into an `int16` diff plane.
pub fn subtract_block(
    rows: usize,
    cols: usize,
    diff: &mut [i16],
    diff_stride: usize,
    src: &[u8],
    src_stride: usize,
    pred: &[u8],
    pred_stride: usize,
) {
    for r in 0..rows {
        for c in 0..cols {
            diff[r * diff_stride + c] =
                src[r * src_stride + c] as i16 - pred[r * pred_stride + c] as i16;
        }
    }
}

/// `svt_aom_highbd_subtract_block_c` (inter_prediction.c:38).
///
/// C takes `uint8_t*` and casts to `uint16_t*`; `bd` is accepted and
/// **unused**, so the subtraction never saturates to the bit depth. That is
/// reproduced: the difference of two 10-bit samples fits `int16` anyway, but
/// nothing here clamps.
pub fn highbd_subtract_block(
    rows: usize,
    cols: usize,
    diff: &mut [i16],
    diff_stride: usize,
    src: &[u16],
    src_stride: usize,
    pred: &[u16],
    pred_stride: usize,
) {
    for r in 0..rows {
        for c in 0..cols {
            diff[r * diff_stride + c] =
                (src[r * src_stride + c] as i32 - pred[r * pred_stride + c] as i32) as i16;
        }
    }
}

/// `svt_aom_sum_squares_i16_c` (inter_prediction.c:2522).
///
/// C's loop is a `do { } while (--n)`, so `n == 0` wraps and runs 2^32 times.
/// This port panics on 0 rather than reproducing an infinite loop — a refusal
/// is not a wrong answer (`WORKING-ON-THIS.md` §6), and no caller passes 0.
pub fn sum_squares_i16(src: &[i16], n: usize) -> u64 {
    assert!(
        n > 0,
        "svt_aom_sum_squares_i16_c's do-while cannot take n == 0"
    );
    let mut ss = 0u64;
    for &v in &src[..n] {
        ss += (v as i32 * v as i32) as u64;
    }
    ss
}

/// `svt_aom_sse_c` (enc_inter_prediction.c:612) — pixel-domain SSE over two
/// 8-bit planes. Distinct from the coefficient-domain
/// `svt_full_distortion_kernel32_bits_c` already in `residual.rs`.
pub fn sse(
    a: &[u8],
    a_stride: usize,
    b: &[u8],
    b_stride: usize,
    width: usize,
    height: usize,
) -> i64 {
    let mut acc = 0i64;
    for y in 0..height {
        for x in 0..width {
            let d = a[y * a_stride + x] as i32 - b[y * b_stride + x] as i32;
            acc += (d * d) as i64;
        }
    }
    acc
}

/// `svt_aom_highbd_sse_c` (enc_inter_prediction.c:597).
pub fn highbd_sse(
    a: &[u16],
    a_stride: usize,
    b: &[u16],
    b_stride: usize,
    width: usize,
    height: usize,
) -> i64 {
    let mut acc = 0i64;
    for y in 0..height {
        for x in 0..width {
            let d = a[y * a_stride + x] as i32 - b[y * b_stride + x] as i32;
            acc += (d * d) as i64;
        }
    }
    acc
}

/// `svt_av1_wedge_compute_delta_squares_c` (enc_inter_prediction.c:375) —
/// `a^2 - b^2`, saturated to `int16`.
pub fn wedge_compute_delta_squares(d: &mut [i16], a: &[i16], b: &[i16], n: usize) {
    for i in 0..n {
        let v = a[i] as i32 * a[i] as i32 - b[i] as i32 * b[i] as i32;
        d[i] = v.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
    }
}

/// `svt_av1_wedge_sign_from_residuals_c` (enc_inter_prediction.c:414).
///
/// Returns true when the negated mask has the lower SSE. `limit` is
/// `MAX_MASK_VALUE/2 * (sum(r0^2) - sum(r1^2))`, precomputed by the caller.
pub fn wedge_sign_from_residuals(ds: &[i16], m: &[u8], n: usize, limit: i64) -> bool {
    assert!(n > 0, "the C loop is a do-while and cannot take N == 0");
    let mut acc = 0i64;
    for i in 0..n {
        acc += (ds[i] as i32 * m[i] as i32) as i64;
    }
    acc > limit
}

/// `svt_av1_wedge_sse_from_residuals_c` (inter_prediction.c:2457).
///
/// The `clamp(t, INT16_MIN, INT16_MAX)` inside the loop is DELIBERATE — the C
/// comment says it exists so a SIMD implementation can be equivalent, and it
/// only bites when residuals exceed `16 - WEDGE_WEIGHT_BITS = 10` signed bits.
/// A port that drops it agrees on ordinary input and diverges on the extremes,
/// so it is reproduced verbatim and the parity test drives residuals well past
/// that range on purpose.
pub fn wedge_sse_from_residuals(r1: &[i16], d: &[i16], m: &[u8], n: usize) -> u64 {
    let mut csse = 0u64;
    for i in 0..n {
        let t = MAX_MASK_VALUE * r1[i] as i32 + m[i] as i32 * d[i] as i32;
        let t = t.clamp(i16::MIN as i32, i16::MAX as i32);
        csse += (t * t) as u64;
    }
    round_power_of_two_u64(csse, 2 * WEDGE_WEIGHT_BITS)
}

/// `diffwtd_mask` (inter_prediction.c:68).
///
/// MEASURED 2026-08-31: the UPPER clamp (`AOM_BLEND_A64_MAX_ALPHA`) is
/// unreachable for every live caller. Both exported entry points pass
/// `mask_base = 38`, and the largest `diff / DIFF_FACTOR` any bit depth can
/// produce is `255 / 16 = 15` (at bd > 8 the difference is first shifted down
/// by `bd - 8`, so 10- and 12-bit cap at the same 255), giving a maximum of
/// 53 — well under 64. Raising the clamp to 65 in this port does NOT fail
/// `build_compound_diffwtd_mask*_matches_c`. It is kept anyway
/// (`WORKING-ON-THIS.md` §7: dead-looking C stays translated, with its
/// reachability written down) — a future `mask_base` other than 38 would
/// reach it immediately.
pub fn diffwtd_mask(
    mask: &mut [u8],
    which_inverse: bool,
    mask_base: i32,
    src0: &[u8],
    src0_stride: usize,
    src1: &[u8],
    src1_stride: usize,
    h: usize,
    w: usize,
) {
    for i in 0..h {
        for j in 0..w {
            let diff = (src0[i * src0_stride + j] as i32 - src1[i * src1_stride + j] as i32).abs();
            let m = (mask_base + diff / DIFF_FACTOR).clamp(0, AOM_BLEND_A64_MAX_ALPHA);
            mask[i * w + j] = if which_inverse {
                (AOM_BLEND_A64_MAX_ALPHA - m) as u8
            } else {
                m as u8
            };
        }
    }
}

/// `diffwtd_mask_highbd` (inter_prediction.c:79).
///
/// C splits this four ways (bd == 8 vs bd > 8, crossed with `which_inverse`)
/// purely for speed: the `bd == 8` arms are the `bd > 8` arms with
/// `bd_shift = 0`, and `negative_to_zero` + `AOMMIN` is the same clamp the
/// 8-bit `diffwtd_mask` writes as `clamp(.., 0, 64)`. Verified by driving all
/// four arms against C. The single loop here is the collapsed form; the
/// specialisation is a perf split, not a behavioural one.
pub fn diffwtd_mask_highbd(
    mask: &mut [u8],
    which_inverse: bool,
    mask_base: i32,
    src0: &[u16],
    src0_stride: usize,
    src1: &[u16],
    src1_stride: usize,
    h: usize,
    w: usize,
    bd: u32,
) {
    assert!(bd >= 8);
    let bd_shift = bd - 8;
    for i in 0..h {
        for j in 0..w {
            let diff = ((src0[i * src0_stride + j] as i32 - src1[i * src1_stride + j] as i32)
                .abs()
                >> bd_shift)
                / DIFF_FACTOR;
            // C writes `negative_to_zero(...)` then `AOMMIN(..)`, i.e. this
            // clamp; the two-step form is kept as one call.
            let m = (mask_base + diff).clamp(0, AOM_BLEND_A64_MAX_ALPHA);
            mask[i * w + j] = if which_inverse {
                (AOM_BLEND_A64_MAX_ALPHA - m) as u8
            } else {
                m as u8
            };
        }
    }
}

/// `svt_av1_build_compound_diffwtd_mask_c` (inter_prediction.c:154).
///
/// Both live mask types use `mask_base = 38`; only the inversion differs.
pub fn build_compound_diffwtd_mask(
    mask: &mut [u8],
    mask_type: DiffwtdMaskType,
    src0: &[u8],
    src0_stride: usize,
    src1: &[u8],
    src1_stride: usize,
    h: usize,
    w: usize,
) {
    diffwtd_mask(
        mask,
        mask_type == DiffwtdMaskType::D38Inv,
        38,
        src0,
        src0_stride,
        src1,
        src1_stride,
        h,
        w,
    );
}

/// `svt_av1_build_compound_diffwtd_mask_highbd_c` (inter_prediction.c:139).
pub fn build_compound_diffwtd_mask_highbd(
    mask: &mut [u8],
    mask_type: DiffwtdMaskType,
    src0: &[u16],
    src0_stride: usize,
    src1: &[u16],
    src1_stride: usize,
    h: usize,
    w: usize,
    bd: u32,
) {
    diffwtd_mask_highbd(
        mask,
        mask_type == DiffwtdMaskType::D38Inv,
        38,
        src0,
        src0_stride,
        src1,
        src1_stride,
        h,
        w,
        bd,
    );
}

/// `svt_aom_highbd_blend_a64_hmask_16bit_c` (inter_prediction.c:2500) — the
/// 10-bit OBMC LEFT blend (its only caller is `build_obmc_inter_pred_left`).
///
/// The mask is indexed by COLUMN only (`mask[j]`), which is what makes it the
/// horizontal-mask variant; `bd` is accepted and unused.
pub fn highbd_blend_a64_hmask_16bit(
    dst: &mut [u16],
    dst_stride: usize,
    src0: &[u16],
    src0_stride: usize,
    src1: &[u16],
    src1_stride: usize,
    mask: &[u8],
    w: usize,
    h: usize,
) {
    for i in 0..h {
        for j in 0..w {
            dst[i * dst_stride + j] = aom_blend_a64(
                mask[j] as i32,
                src0[i * src0_stride + j] as i32,
                src1[i * src1_stride + j] as i32,
            ) as u16;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only WEDGE and DIFFWTD are masked; AVERAGE and DISTWTD are not.
    #[test]
    fn masked_compound_predicate() {
        assert!(is_masked_compound_type(CompoundType::Wedge));
        assert!(is_masked_compound_type(CompoundType::DiffWtd));
        assert!(!is_masked_compound_type(CompoundType::Average));
        assert!(!is_masked_compound_type(CompoundType::DistWtd));
    }

    /// The in-loop int16 clamp in `wedge_sse_from_residuals` is load-bearing on
    /// large residuals: without it the two results differ.
    #[test]
    fn wedge_sse_clamp_is_reachable() {
        let r1 = [30000i16; 16];
        let d = [30000i16; 16];
        let m = [64u8; 16];
        let clamped = wedge_sse_from_residuals(&r1, &d, &m, 16);
        // The unclamped form, computed here only to show the clamp matters.
        let mut raw = 0u64;
        for i in 0..16 {
            let t = MAX_MASK_VALUE as i64 * r1[i] as i64 + m[i] as i64 * d[i] as i64;
            raw += (t * t) as u64;
        }
        let raw = round_power_of_two_u64(raw, 2 * WEDGE_WEIGHT_BITS);
        assert_ne!(
            clamped, raw,
            "the clamp must change the result on this input"
        );
    }
}
