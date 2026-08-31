//! Reduced-coefficient-shape ("PF" / partial-frequency) forward transforms.
//!
//! Port of the `_N2` / `_N4` family in `Codec/transforms.c`. These are the
//! transforms SVT-AV1 runs when a caller asks for only the top-left half
//! (`N2_SHAPE`) or quarter (`N4_SHAPE`) of the coefficient block: TPL's
//! `svt_av1_wht_fwd_txfm` at `tpl_params_level >= 4`, and MD's transform
//! shortcut (`apply_pf_on_coeffs`) on non-base inter frames.
//!
//! The 1-D kernels below are NOT the full kernels with a truncated output —
//! C prunes whole butterflies, so the surviving coefficients are computed by
//! a different (shorter) dependency chain. They are transcribed stage by
//! stage from the C bodies and gated at evidence tier 1 against the real
//! exported symbols (`tests/c_parity_txfm_pf.rs`).
//!
//! Two properties that a "just truncate the full transform" implementation
//! would get wrong, both faithful here:
//!   * a kernel writes only part of `output` and leaves the rest at whatever
//!     the caller had there (C aliases `temp_out` into the caller's buffer);
//!   * `svt_av1_fadst4_new_N2` / `_N4` short-circuit to FOUR zeros when all
//!     four inputs are zero, even though only one/two outputs are otherwise
//!     produced.
//!
//! ## Arithmetic domain, measured 2026-08-31
//!
//! `half_btf` here is `crate::fwd_txfm::half_btf`, which forms both products
//! in i64. C's `half_btf` (`inv_transforms.h:270`) writes
//! `(int64_t)(w0 * in0) + (int64_t)(w1 * in1)` — the products are `int32_t`,
//! so signed overflow there is UB and the compiler inlines/reassociates each
//! call site independently. That is observable: driving
//! `svt_av1_fdct64_new_N2` with 1-D inputs at +/- 2^15 makes the built
//! oracle disagree with an i64 `half_btf` by exactly 2^19 at `output[8]`
//! (a 2^32 accumulator wrap shifted down by `cos_bit = 13`) — and ALSO
//! disagree with a faithful wrapping-i32 `half_btf`, at a different index
//! (`output[6]`), because clang wraps at some inlined sites and not others.
//! Above 2^14 there is therefore no single "what C does"; below it the two
//! formulations coincide. The encoder never gets there: the column pass is
//! fed an int16 residual (|r| <= 1023 at 10-bit) left-shifted by
//! `shift[0] = 2`, i.e. |input| <= 2^12, and the row pass sees the
//! stage-range-bounded column output. `tests/c_parity_txfm_pf.rs` sweeps
//! +/- 2^14 for that reason, and the 2-D tests drive real residuals.

use crate::fwd_txfm::{NEW_SQRT2, NEW_SQRT2_BITS, cospi_arr, half_btf, round_shift_i64, sinpi_arr};

// =============================================================================
// Coefficient shape (C `TxCoeffShape`, definitions.h:2062)
// =============================================================================

/// C `TxCoeffShape` — how much of the coefficient block a transform produces.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum TxCoeffShape {
    /// `DEFAULT_SHAPE = 0` — the full block.
    #[default]
    Default,
    /// `N2_SHAPE = 1` — top-left half in each dimension.
    N2,
    /// `N4_SHAPE = 2` — top-left quarter in each dimension.
    N4,
    /// `ONLY_DC_SHAPE = 3` — DC only.
    OnlyDc,
}

// =============================================================================
// 1-D identity kernels (C `svt_av1_fidentityN_{N2,N4}_c`)
// =============================================================================

/// Port of C `svt_av1_fidentity4_N2_c` (transforms.c:5044).
pub fn fidentity4_n2(input: &[i32], output: &mut [i32], _cos_bit: i8) {
    output[0] = round_shift_i64(input[0] as i64 * NEW_SQRT2 as i64, NEW_SQRT2_BITS);
    output[1] = round_shift_i64(input[1] as i64 * NEW_SQRT2 as i64, NEW_SQRT2_BITS);
}

/// Port of C `svt_av1_fidentity4_N4_c` (transforms.c:6415).
pub fn fidentity4_n4(input: &[i32], output: &mut [i32], _cos_bit: i8) {
    output[0] = round_shift_i64(input[0] as i64 * NEW_SQRT2 as i64, NEW_SQRT2_BITS);
}

/// Port of C `svt_av1_fidentity8_N2_c` (transforms.c:4885).
pub fn fidentity8_n2(input: &[i32], output: &mut [i32], _cos_bit: i8) {
    for i in 0..4 {
        output[i] = input[i] * 2;
    }
}

/// Port of C `svt_av1_fidentity8_N4_c` (transforms.c:6552).
pub fn fidentity8_n4(input: &[i32], output: &mut [i32], _cos_bit: i8) {
    for i in 0..2 {
        output[i] = input[i] * 2;
    }
}

/// Port of C `svt_av1_fidentity16_N2_c` (transforms.c:4565).
pub fn fidentity16_n2(input: &[i32], output: &mut [i32], _cos_bit: i8) {
    for i in 0..8 {
        output[i] = round_shift_i64(input[i] as i64 * 2 * NEW_SQRT2 as i64, NEW_SQRT2_BITS);
    }
}

/// Port of C `svt_av1_fidentity16_N4_c` (transforms.c:6828).
pub fn fidentity16_n4(input: &[i32], output: &mut [i32], _cos_bit: i8) {
    for i in 0..4 {
        output[i] = round_shift_i64(input[i] as i64 * 2 * NEW_SQRT2 as i64, NEW_SQRT2_BITS);
    }
}

/// Port of C `svt_av1_fidentity32_N2_c` (transforms.c:5410).
pub fn fidentity32_n2(input: &[i32], output: &mut [i32], _cos_bit: i8) {
    for i in 0..16 {
        output[i] = input[i] * 4;
    }
}

/// Port of C `svt_av1_fidentity32_N4_c` (transforms.c:7086).
pub fn fidentity32_n4(input: &[i32], output: &mut [i32], _cos_bit: i8) {
    for i in 0..8 {
        output[i] = input[i] * 4;
    }
}

/// Port of C `av1_fidentity64_N2_c` (transforms.c:6090; `static`, reachable
/// through `svt_av1_fwd_txfm2d_*_N2_c` with an IDTX/V_/H_ transform type).
pub fn fidentity64_n2(input: &[i32], output: &mut [i32], _cos_bit: i8) {
    for i in 0..32 {
        output[i] = round_shift_i64(input[i] as i64 * 4 * NEW_SQRT2 as i64, NEW_SQRT2_BITS);
    }
}

/// Port of C `av1_fidentity64_N4_c` (transforms.c:7687; `static`).
pub fn fidentity64_n4(input: &[i32], output: &mut [i32], _cos_bit: i8) {
    for i in 0..16 {
        output[i] = round_shift_i64(input[i] as i64 * 4 * NEW_SQRT2 as i64, NEW_SQRT2_BITS);
    }
}

// =============================================================================
// 1-D ADST-4 (C keeps this one in scalar variables, not the bf0/bf1 form)
// =============================================================================

/// Port of C `svt_av1_fadst4_new_N2` (transforms.c:5052).
///
/// The all-zero short circuit writes FOUR zeros — not the two coefficients
/// this shape otherwise produces. Faithful on purpose.
pub fn fadst4_n2(input: &[i32], output: &mut [i32], cos_bit: i8) {
    let bit = cos_bit as u32;
    let sinpi = sinpi_arr(cos_bit);
    let (x0, x1, x2, x3) = (input[0], input[1], input[2], input[3]);
    if (x0 | x1 | x2 | x3) == 0 {
        output[0] = 0;
        output[1] = 0;
        output[2] = 0;
        output[3] = 0;
        return;
    }
    // stage 1 (i64 intermediates; C accumulates in int32 and promotes at
    // round_shift — identical over the encoder's coefficient range, see the
    // module header's note on out-of-range half_btf)
    let s0 = sinpi[1] as i64 * x0 as i64;
    let s2 = sinpi[2] as i64 * x1 as i64;
    let s4 = sinpi[3] as i64 * x2 as i64;
    let s5 = sinpi[4] as i64 * x3 as i64;
    let mut s7 = (x0 + x1) as i64;
    // stage 2
    s7 -= x3 as i64;
    // stage 3
    let mut x0 = s0 + s2;
    let x1 = sinpi[3] as i64 * s7;
    // stage 4
    x0 += s5;
    // stage 5
    let s0 = x0 + s4;
    output[0] = round_shift_i64(s0, bit);
    output[1] = round_shift_i64(x1, bit);
}

/// Port of C `svt_av1_fadst4_new_N4` (transforms.c:6378). Same four-zero
/// short circuit as [`fadst4_n2`].
pub fn fadst4_n4(input: &[i32], output: &mut [i32], cos_bit: i8) {
    let bit = cos_bit as u32;
    let sinpi = sinpi_arr(cos_bit);
    let (x0, x1, x2, x3) = (input[0], input[1], input[2], input[3]);
    if (x0 | x1 | x2 | x3) == 0 {
        output[0] = 0;
        output[1] = 0;
        output[2] = 0;
        output[3] = 0;
        return;
    }
    // stage 1 (i64 intermediates, as in [`fadst4_n2`])
    let s0 = sinpi[1] as i64 * x0 as i64;
    let s2 = sinpi[2] as i64 * x1 as i64;
    let s4 = sinpi[3] as i64 * x2 as i64;
    let s5 = sinpi[4] as i64 * x3 as i64;
    // stage 3
    let mut x0 = s0 + s2;
    // stage 4
    x0 += s5;
    // stage 5
    let s0 = x0 + s4;
    output[0] = round_shift_i64(s0, bit);
}

// =============================================================================
// 1-D DCT / ADST kernels, transcribed stage-for-stage from C
// =============================================================================

/// Port of C `svt_av1_fdct4_new_N2` (transforms.c).
pub fn fdct4_n2(input: &[i32], output: &mut [i32], cos_bit: i8) {
    let cospi = cospi_arr(cos_bit);
    let step = &mut [0i32; 4];
    // stage 1;
    step[0] = input[0] + input[3];
    step[1] = input[1] + input[2];
    step[2] = -input[2] + input[1];
    step[3] = -input[3] + input[0];
    // stage 2
    output[0] = half_btf(cospi[32], step[0], cospi[32], step[1], cos_bit as u32);
    output[1] = half_btf(cospi[48], step[2], cospi[16], step[3], cos_bit as u32);
}

/// Port of C `svt_av1_fdct4_new_N4` (transforms.c).
pub fn fdct4_n4(input: &[i32], output: &mut [i32], cos_bit: i8) {
    let cospi = cospi_arr(cos_bit);
    let step = &mut [0i32; 4];
    // stage 1;
    step[0] = input[0] + input[3];
    step[1] = input[1] + input[2];
    output[0] = half_btf(cospi[32], step[0], cospi[32], step[1], cos_bit as u32);
}

/// Port of C `svt_av1_fdct8_new_N2` (transforms.c).
pub fn fdct8_n2(input: &[i32], output: &mut [i32], cos_bit: i8) {
    let cospi = cospi_arr(cos_bit);
    let step = &mut [0i32; 8];
    // stage 0;
    // stage 1;
    output[0] = input[0] + input[7];
    output[1] = input[1] + input[6];
    output[2] = input[2] + input[5];
    output[3] = input[3] + input[4];
    output[4] = -input[4] + input[3];
    output[5] = -input[5] + input[2];
    output[6] = -input[6] + input[1];
    output[7] = -input[7] + input[0];
    // stage 2
    step[0] = output[0] + output[3];
    step[1] = output[1] + output[2];
    step[2] = -output[2] + output[1];
    step[3] = -output[3] + output[0];
    step[4] = output[4];
    step[5] = half_btf(-cospi[32], output[5], cospi[32], output[6], cos_bit as u32);
    step[6] = half_btf(cospi[32], output[6], cospi[32], output[5], cos_bit as u32);
    step[7] = output[7];
    // stage 3
    output[0] = half_btf(cospi[32], step[0], cospi[32], step[1], cos_bit as u32);
    output[2] = half_btf(cospi[48], step[2], cospi[16], step[3], cos_bit as u32);
    output[4] = step[4] + step[5];
    output[5] = -step[5] + step[4];
    output[6] = -step[6] + step[7];
    output[7] = step[7] + step[6];
    // stage 4
    step[0] = output[0];
    step[2] = output[2];
    step[4] = half_btf(cospi[56], output[4], cospi[8], output[7], cos_bit as u32);
    step[6] = half_btf(cospi[24], output[6], -cospi[40], output[5], cos_bit as u32);
    // stage 5
    output[0] = step[0];
    output[1] = step[4];
    output[2] = step[2];
    output[3] = step[6];
}

/// Port of C `svt_av1_fdct8_new_N4` (transforms.c).
pub fn fdct8_n4(input: &[i32], output: &mut [i32], cos_bit: i8) {
    let cospi = cospi_arr(cos_bit);
    let step = &mut [0i32; 8];
    // stage 0;
    // stage 1;
    output[0] = input[0] + input[7];
    output[1] = input[1] + input[6];
    output[2] = input[2] + input[5];
    output[3] = input[3] + input[4];
    output[4] = -input[4] + input[3];
    output[5] = -input[5] + input[2];
    output[6] = -input[6] + input[1];
    output[7] = -input[7] + input[0];
    // stage 2
    step[0] = output[0] + output[3];
    step[1] = output[1] + output[2];
    step[4] = output[4];
    step[5] = half_btf(-cospi[32], output[5], cospi[32], output[6], cos_bit as u32);
    step[6] = half_btf(cospi[32], output[6], cospi[32], output[5], cos_bit as u32);
    step[7] = output[7];
    // stage 3
    output[0] = half_btf(cospi[32], step[0], cospi[32], step[1], cos_bit as u32);
    output[4] = step[4] + step[5];
    output[7] = step[7] + step[6];
    // stage 4
    step[0] = output[0];
    step[4] = half_btf(cospi[56], output[4], cospi[8], output[7], cos_bit as u32);
    // stage 5
    output[0] = step[0];
    output[1] = step[4];
}

/// Port of C `svt_av1_fdct16_new_N2` (transforms.c).
pub fn fdct16_n2(input: &[i32], output: &mut [i32], cos_bit: i8) {
    let cospi = cospi_arr(cos_bit);
    let step = &mut [0i32; 16];
    // stage 0;
    // stage 1;
    output[0] = input[0] + input[15];
    output[1] = input[1] + input[14];
    output[2] = input[2] + input[13];
    output[3] = input[3] + input[12];
    output[4] = input[4] + input[11];
    output[5] = input[5] + input[10];
    output[6] = input[6] + input[9];
    output[7] = input[7] + input[8];
    output[8] = -input[8] + input[7];
    output[9] = -input[9] + input[6];
    output[10] = -input[10] + input[5];
    output[11] = -input[11] + input[4];
    output[12] = -input[12] + input[3];
    output[13] = -input[13] + input[2];
    output[14] = -input[14] + input[1];
    output[15] = -input[15] + input[0];
    // stage 2
    step[0] = output[0] + output[7];
    step[1] = output[1] + output[6];
    step[2] = output[2] + output[5];
    step[3] = output[3] + output[4];
    step[4] = -output[4] + output[3];
    step[5] = -output[5] + output[2];
    step[6] = -output[6] + output[1];
    step[7] = -output[7] + output[0];
    step[8] = output[8];
    step[9] = output[9];
    step[10] = half_btf(
        -cospi[32],
        output[10],
        cospi[32],
        output[13],
        cos_bit as u32,
    );
    step[11] = half_btf(
        -cospi[32],
        output[11],
        cospi[32],
        output[12],
        cos_bit as u32,
    );
    step[12] = half_btf(cospi[32], output[12], cospi[32], output[11], cos_bit as u32);
    step[13] = half_btf(cospi[32], output[13], cospi[32], output[10], cos_bit as u32);
    step[14] = output[14];
    step[15] = output[15];
    // stage 3
    output[0] = step[0] + step[3];
    output[1] = step[1] + step[2];
    output[2] = -step[2] + step[1];
    output[3] = -step[3] + step[0];
    output[4] = step[4];
    output[5] = half_btf(-cospi[32], step[5], cospi[32], step[6], cos_bit as u32);
    output[6] = half_btf(cospi[32], step[6], cospi[32], step[5], cos_bit as u32);
    output[7] = step[7];
    output[8] = step[8] + step[11];
    output[9] = step[9] + step[10];
    output[10] = -step[10] + step[9];
    output[11] = -step[11] + step[8];
    output[12] = -step[12] + step[15];
    output[13] = -step[13] + step[14];
    output[14] = step[14] + step[13];
    output[15] = step[15] + step[12];
    // stage 4
    step[0] = half_btf(cospi[32], output[0], cospi[32], output[1], cos_bit as u32);
    step[2] = half_btf(cospi[48], output[2], cospi[16], output[3], cos_bit as u32);
    step[4] = output[4] + output[5];
    step[5] = -output[5] + output[4];
    step[6] = -output[6] + output[7];
    step[7] = output[7] + output[6];
    step[8] = output[8];
    step[9] = half_btf(-cospi[16], output[9], cospi[48], output[14], cos_bit as u32);
    step[10] = half_btf(
        -cospi[48],
        output[10],
        -cospi[16],
        output[13],
        cos_bit as u32,
    );
    step[11] = output[11];
    step[12] = output[12];
    step[13] = half_btf(
        cospi[48],
        output[13],
        -cospi[16],
        output[10],
        cos_bit as u32,
    );
    step[14] = half_btf(cospi[16], output[14], cospi[48], output[9], cos_bit as u32);
    step[15] = output[15];
    // stage 5
    output[0] = step[0];
    output[2] = step[2];
    output[4] = half_btf(cospi[56], step[4], cospi[8], step[7], cos_bit as u32);
    output[6] = half_btf(cospi[24], step[6], -cospi[40], step[5], cos_bit as u32);
    output[8] = step[8] + step[9];
    output[9] = -step[9] + step[8];
    output[10] = -step[10] + step[11];
    output[11] = step[11] + step[10];
    output[12] = step[12] + step[13];
    output[13] = -step[13] + step[12];
    output[14] = -step[14] + step[15];
    output[15] = step[15] + step[14];
    // stage 6
    step[0] = output[0];
    step[2] = output[2];
    step[4] = output[4];
    step[6] = output[6];
    step[8] = half_btf(cospi[60], output[8], cospi[4], output[15], cos_bit as u32);
    step[10] = half_btf(cospi[44], output[10], cospi[20], output[13], cos_bit as u32);
    step[12] = half_btf(
        cospi[12],
        output[12],
        -cospi[52],
        output[11],
        cos_bit as u32,
    );
    step[14] = half_btf(cospi[28], output[14], -cospi[36], output[9], cos_bit as u32);
    // stage 7
    output[0] = step[0];
    output[1] = step[8];
    output[2] = step[4];
    output[3] = step[12];
    output[4] = step[2];
    output[5] = step[10];
    output[6] = step[6];
    output[7] = step[14];
}

/// Port of C `svt_av1_fdct16_new_N4` (transforms.c).
pub fn fdct16_n4(input: &[i32], output: &mut [i32], cos_bit: i8) {
    let cospi = cospi_arr(cos_bit);
    let step = &mut [0i32; 16];
    // stage 0;
    // stage 1;
    output[0] = input[0] + input[15];
    output[1] = input[1] + input[14];
    output[2] = input[2] + input[13];
    output[3] = input[3] + input[12];
    output[4] = input[4] + input[11];
    output[5] = input[5] + input[10];
    output[6] = input[6] + input[9];
    output[7] = input[7] + input[8];
    output[8] = -input[8] + input[7];
    output[9] = -input[9] + input[6];
    output[10] = -input[10] + input[5];
    output[11] = -input[11] + input[4];
    output[12] = -input[12] + input[3];
    output[13] = -input[13] + input[2];
    output[14] = -input[14] + input[1];
    output[15] = -input[15] + input[0];
    // stage 2
    step[0] = output[0] + output[7];
    step[1] = output[1] + output[6];
    step[2] = output[2] + output[5];
    step[3] = output[3] + output[4];
    step[4] = -output[4] + output[3];
    step[5] = -output[5] + output[2];
    step[6] = -output[6] + output[1];
    step[7] = -output[7] + output[0];
    step[8] = output[8];
    step[9] = output[9];
    step[10] = half_btf(
        -cospi[32],
        output[10],
        cospi[32],
        output[13],
        cos_bit as u32,
    );
    step[11] = half_btf(
        -cospi[32],
        output[11],
        cospi[32],
        output[12],
        cos_bit as u32,
    );
    step[12] = half_btf(cospi[32], output[12], cospi[32], output[11], cos_bit as u32);
    step[13] = half_btf(cospi[32], output[13], cospi[32], output[10], cos_bit as u32);
    step[14] = output[14];
    step[15] = output[15];
    // stage 3
    output[0] = step[0] + step[3];
    output[1] = step[1] + step[2];
    output[4] = step[4];
    output[5] = half_btf(-cospi[32], step[5], cospi[32], step[6], cos_bit as u32);
    output[6] = half_btf(cospi[32], step[6], cospi[32], step[5], cos_bit as u32);
    output[7] = step[7];
    output[8] = step[8] + step[11];
    output[9] = step[9] + step[10];
    output[10] = -step[10] + step[9];
    output[11] = -step[11] + step[8];
    output[12] = -step[12] + step[15];
    output[13] = -step[13] + step[14];
    output[14] = step[14] + step[13];
    output[15] = step[15] + step[12];
    // stage 4
    step[0] = half_btf(cospi[32], output[0], cospi[32], output[1], cos_bit as u32);
    step[4] = output[4] + output[5];
    step[7] = output[7] + output[6];
    step[8] = output[8];
    step[9] = half_btf(-cospi[16], output[9], cospi[48], output[14], cos_bit as u32);
    step[10] = half_btf(
        -cospi[48],
        output[10],
        -cospi[16],
        output[13],
        cos_bit as u32,
    );
    step[11] = output[11];
    step[12] = output[12];
    step[13] = half_btf(
        cospi[48],
        output[13],
        -cospi[16],
        output[10],
        cos_bit as u32,
    );
    step[14] = half_btf(cospi[16], output[14], cospi[48], output[9], cos_bit as u32);
    step[15] = output[15];
    // stage 5
    output[0] = step[0];
    output[4] = half_btf(cospi[56], step[4], cospi[8], step[7], cos_bit as u32);
    output[8] = step[8] + step[9];
    output[11] = step[11] + step[10];
    output[12] = step[12] + step[13];
    output[15] = step[15] + step[14];
    // stage 6
    step[0] = output[0];
    step[4] = output[4];
    step[8] = half_btf(cospi[60], output[8], cospi[4], output[15], cos_bit as u32);
    step[12] = half_btf(
        cospi[12],
        output[12],
        -cospi[52],
        output[11],
        cos_bit as u32,
    );
    // stage 7
    output[0] = step[0];
    output[1] = step[8];
    output[2] = step[4];
    output[3] = step[12];
}

/// Port of C `svt_av1_fdct32_new_N2` (transforms.c).
pub fn fdct32_n2(input: &[i32], output: &mut [i32], cos_bit: i8) {
    let cospi = cospi_arr(cos_bit);
    let step = &mut [0i32; 32];
    // stage 0;
    // stage 1;
    output[0] = input[0] + input[31];
    output[1] = input[1] + input[30];
    output[2] = input[2] + input[29];
    output[3] = input[3] + input[28];
    output[4] = input[4] + input[27];
    output[5] = input[5] + input[26];
    output[6] = input[6] + input[25];
    output[7] = input[7] + input[24];
    output[8] = input[8] + input[23];
    output[9] = input[9] + input[22];
    output[10] = input[10] + input[21];
    output[11] = input[11] + input[20];
    output[12] = input[12] + input[19];
    output[13] = input[13] + input[18];
    output[14] = input[14] + input[17];
    output[15] = input[15] + input[16];
    output[16] = -input[16] + input[15];
    output[17] = -input[17] + input[14];
    output[18] = -input[18] + input[13];
    output[19] = -input[19] + input[12];
    output[20] = -input[20] + input[11];
    output[21] = -input[21] + input[10];
    output[22] = -input[22] + input[9];
    output[23] = -input[23] + input[8];
    output[24] = -input[24] + input[7];
    output[25] = -input[25] + input[6];
    output[26] = -input[26] + input[5];
    output[27] = -input[27] + input[4];
    output[28] = -input[28] + input[3];
    output[29] = -input[29] + input[2];
    output[30] = -input[30] + input[1];
    output[31] = -input[31] + input[0];
    // stage 2
    step[0] = output[0] + output[15];
    step[1] = output[1] + output[14];
    step[2] = output[2] + output[13];
    step[3] = output[3] + output[12];
    step[4] = output[4] + output[11];
    step[5] = output[5] + output[10];
    step[6] = output[6] + output[9];
    step[7] = output[7] + output[8];
    step[8] = -output[8] + output[7];
    step[9] = -output[9] + output[6];
    step[10] = -output[10] + output[5];
    step[11] = -output[11] + output[4];
    step[12] = -output[12] + output[3];
    step[13] = -output[13] + output[2];
    step[14] = -output[14] + output[1];
    step[15] = -output[15] + output[0];
    step[16] = output[16];
    step[17] = output[17];
    step[18] = output[18];
    step[19] = output[19];
    step[20] = half_btf(
        -cospi[32],
        output[20],
        cospi[32],
        output[27],
        cos_bit as u32,
    );
    step[21] = half_btf(
        -cospi[32],
        output[21],
        cospi[32],
        output[26],
        cos_bit as u32,
    );
    step[22] = half_btf(
        -cospi[32],
        output[22],
        cospi[32],
        output[25],
        cos_bit as u32,
    );
    step[23] = half_btf(
        -cospi[32],
        output[23],
        cospi[32],
        output[24],
        cos_bit as u32,
    );
    step[24] = half_btf(cospi[32], output[24], cospi[32], output[23], cos_bit as u32);
    step[25] = half_btf(cospi[32], output[25], cospi[32], output[22], cos_bit as u32);
    step[26] = half_btf(cospi[32], output[26], cospi[32], output[21], cos_bit as u32);
    step[27] = half_btf(cospi[32], output[27], cospi[32], output[20], cos_bit as u32);
    step[28] = output[28];
    step[29] = output[29];
    step[30] = output[30];
    step[31] = output[31];
    // stage 3
    output[0] = step[0] + step[7];
    output[1] = step[1] + step[6];
    output[2] = step[2] + step[5];
    output[3] = step[3] + step[4];
    output[4] = -step[4] + step[3];
    output[5] = -step[5] + step[2];
    output[6] = -step[6] + step[1];
    output[7] = -step[7] + step[0];
    output[8] = step[8];
    output[9] = step[9];
    output[10] = half_btf(-cospi[32], step[10], cospi[32], step[13], cos_bit as u32);
    output[11] = half_btf(-cospi[32], step[11], cospi[32], step[12], cos_bit as u32);
    output[12] = half_btf(cospi[32], step[12], cospi[32], step[11], cos_bit as u32);
    output[13] = half_btf(cospi[32], step[13], cospi[32], step[10], cos_bit as u32);
    output[14] = step[14];
    output[15] = step[15];
    output[16] = step[16] + step[23];
    output[17] = step[17] + step[22];
    output[18] = step[18] + step[21];
    output[19] = step[19] + step[20];
    output[20] = -step[20] + step[19];
    output[21] = -step[21] + step[18];
    output[22] = -step[22] + step[17];
    output[23] = -step[23] + step[16];
    output[24] = -step[24] + step[31];
    output[25] = -step[25] + step[30];
    output[26] = -step[26] + step[29];
    output[27] = -step[27] + step[28];
    output[28] = step[28] + step[27];
    output[29] = step[29] + step[26];
    output[30] = step[30] + step[25];
    output[31] = step[31] + step[24];
    // stage 4
    step[0] = output[0] + output[3];
    step[1] = output[1] + output[2];
    step[2] = -output[2] + output[1];
    step[3] = -output[3] + output[0];
    step[4] = output[4];
    step[5] = half_btf(-cospi[32], output[5], cospi[32], output[6], cos_bit as u32);
    step[6] = half_btf(cospi[32], output[6], cospi[32], output[5], cos_bit as u32);
    step[7] = output[7];
    step[8] = output[8] + output[11];
    step[9] = output[9] + output[10];
    step[10] = -output[10] + output[9];
    step[11] = -output[11] + output[8];
    step[12] = -output[12] + output[15];
    step[13] = -output[13] + output[14];
    step[14] = output[14] + output[13];
    step[15] = output[15] + output[12];
    step[16] = output[16];
    step[17] = output[17];
    step[18] = half_btf(
        -cospi[16],
        output[18],
        cospi[48],
        output[29],
        cos_bit as u32,
    );
    step[19] = half_btf(
        -cospi[16],
        output[19],
        cospi[48],
        output[28],
        cos_bit as u32,
    );
    step[20] = half_btf(
        -cospi[48],
        output[20],
        -cospi[16],
        output[27],
        cos_bit as u32,
    );
    step[21] = half_btf(
        -cospi[48],
        output[21],
        -cospi[16],
        output[26],
        cos_bit as u32,
    );
    step[22] = output[22];
    step[23] = output[23];
    step[24] = output[24];
    step[25] = output[25];
    step[26] = half_btf(
        cospi[48],
        output[26],
        -cospi[16],
        output[21],
        cos_bit as u32,
    );
    step[27] = half_btf(
        cospi[48],
        output[27],
        -cospi[16],
        output[20],
        cos_bit as u32,
    );
    step[28] = half_btf(cospi[16], output[28], cospi[48], output[19], cos_bit as u32);
    step[29] = half_btf(cospi[16], output[29], cospi[48], output[18], cos_bit as u32);
    step[30] = output[30];
    step[31] = output[31];
    // stage 5
    output[0] = half_btf(cospi[32], step[0], cospi[32], step[1], cos_bit as u32);
    output[2] = half_btf(cospi[48], step[2], cospi[16], step[3], cos_bit as u32);
    output[4] = step[4] + step[5];
    output[5] = -step[5] + step[4];
    output[6] = -step[6] + step[7];
    output[7] = step[7] + step[6];
    output[8] = step[8];
    output[9] = half_btf(-cospi[16], step[9], cospi[48], step[14], cos_bit as u32);
    output[10] = half_btf(-cospi[48], step[10], -cospi[16], step[13], cos_bit as u32);
    output[11] = step[11];
    output[12] = step[12];
    output[13] = half_btf(cospi[48], step[13], -cospi[16], step[10], cos_bit as u32);
    output[14] = half_btf(cospi[16], step[14], cospi[48], step[9], cos_bit as u32);
    output[15] = step[15];
    output[16] = step[16] + step[19];
    output[17] = step[17] + step[18];
    output[18] = -step[18] + step[17];
    output[19] = -step[19] + step[16];
    output[20] = -step[20] + step[23];
    output[21] = -step[21] + step[22];
    output[22] = step[22] + step[21];
    output[23] = step[23] + step[20];
    output[24] = step[24] + step[27];
    output[25] = step[25] + step[26];
    output[26] = -step[26] + step[25];
    output[27] = -step[27] + step[24];
    output[28] = -step[28] + step[31];
    output[29] = -step[29] + step[30];
    output[30] = step[30] + step[29];
    output[31] = step[31] + step[28];
    // stage 6
    step[0] = output[0];
    step[2] = output[2];
    step[4] = half_btf(cospi[56], output[4], cospi[8], output[7], cos_bit as u32);
    step[6] = half_btf(cospi[24], output[6], -cospi[40], output[5], cos_bit as u32);
    step[8] = output[8] + output[9];
    step[9] = -output[9] + output[8];
    step[10] = -output[10] + output[11];
    step[11] = output[11] + output[10];
    step[12] = output[12] + output[13];
    step[13] = -output[13] + output[12];
    step[14] = -output[14] + output[15];
    step[15] = output[15] + output[14];
    step[16] = output[16];
    step[17] = half_btf(-cospi[8], output[17], cospi[56], output[30], cos_bit as u32);
    step[18] = half_btf(
        -cospi[56],
        output[18],
        -cospi[8],
        output[29],
        cos_bit as u32,
    );
    step[19] = output[19];
    step[20] = output[20];
    step[21] = half_btf(
        -cospi[40],
        output[21],
        cospi[24],
        output[26],
        cos_bit as u32,
    );
    step[22] = half_btf(
        -cospi[24],
        output[22],
        -cospi[40],
        output[25],
        cos_bit as u32,
    );
    step[23] = output[23];
    step[24] = output[24];
    step[25] = half_btf(
        cospi[24],
        output[25],
        -cospi[40],
        output[22],
        cos_bit as u32,
    );
    step[26] = half_btf(cospi[40], output[26], cospi[24], output[21], cos_bit as u32);
    step[27] = output[27];
    step[28] = output[28];
    step[29] = half_btf(cospi[56], output[29], -cospi[8], output[18], cos_bit as u32);
    step[30] = half_btf(cospi[8], output[30], cospi[56], output[17], cos_bit as u32);
    step[31] = output[31];
    // stage 7
    output[0] = step[0];
    output[2] = step[2];
    output[4] = step[4];
    output[6] = step[6];
    output[8] = half_btf(cospi[60], step[8], cospi[4], step[15], cos_bit as u32);
    output[10] = half_btf(cospi[44], step[10], cospi[20], step[13], cos_bit as u32);
    output[12] = half_btf(cospi[12], step[12], -cospi[52], step[11], cos_bit as u32);
    output[14] = half_btf(cospi[28], step[14], -cospi[36], step[9], cos_bit as u32);
    output[16] = step[16] + step[17];
    output[17] = -step[17] + step[16];
    output[18] = -step[18] + step[19];
    output[19] = step[19] + step[18];
    output[20] = step[20] + step[21];
    output[21] = -step[21] + step[20];
    output[22] = -step[22] + step[23];
    output[23] = step[23] + step[22];
    output[24] = step[24] + step[25];
    output[25] = -step[25] + step[24];
    output[26] = -step[26] + step[27];
    output[27] = step[27] + step[26];
    output[28] = step[28] + step[29];
    output[29] = -step[29] + step[28];
    output[30] = -step[30] + step[31];
    output[31] = step[31] + step[30];
    // stage 8
    step[0] = output[0];
    step[2] = output[2];
    step[4] = output[4];
    step[6] = output[6];
    step[8] = output[8];
    step[10] = output[10];
    step[12] = output[12];
    step[14] = output[14];
    step[16] = half_btf(cospi[62], output[16], cospi[2], output[31], cos_bit as u32);
    step[18] = half_btf(cospi[46], output[18], cospi[18], output[29], cos_bit as u32);
    step[20] = half_btf(cospi[54], output[20], cospi[10], output[27], cos_bit as u32);
    step[22] = half_btf(cospi[38], output[22], cospi[26], output[25], cos_bit as u32);
    step[24] = half_btf(cospi[6], output[24], -cospi[58], output[23], cos_bit as u32);
    step[26] = half_btf(
        cospi[22],
        output[26],
        -cospi[42],
        output[21],
        cos_bit as u32,
    );
    step[28] = half_btf(
        cospi[14],
        output[28],
        -cospi[50],
        output[19],
        cos_bit as u32,
    );
    step[30] = half_btf(
        cospi[30],
        output[30],
        -cospi[34],
        output[17],
        cos_bit as u32,
    );
    // stage 9
    output[0] = step[0];
    output[1] = step[16];
    output[2] = step[8];
    output[3] = step[24];
    output[4] = step[4];
    output[5] = step[20];
    output[6] = step[12];
    output[7] = step[28];
    output[8] = step[2];
    output[9] = step[18];
    output[10] = step[10];
    output[11] = step[26];
    output[12] = step[6];
    output[13] = step[22];
    output[14] = step[14];
    output[15] = step[30];
}

/// Port of C `svt_av1_fdct32_new_N4` (transforms.c).
pub fn fdct32_n4(input: &[i32], output: &mut [i32], cos_bit: i8) {
    let cospi = cospi_arr(cos_bit);
    let step = &mut [0i32; 32];
    // stage 0;
    // stage 1;
    output[0] = input[0] + input[31];
    output[1] = input[1] + input[30];
    output[2] = input[2] + input[29];
    output[3] = input[3] + input[28];
    output[4] = input[4] + input[27];
    output[5] = input[5] + input[26];
    output[6] = input[6] + input[25];
    output[7] = input[7] + input[24];
    output[8] = input[8] + input[23];
    output[9] = input[9] + input[22];
    output[10] = input[10] + input[21];
    output[11] = input[11] + input[20];
    output[12] = input[12] + input[19];
    output[13] = input[13] + input[18];
    output[14] = input[14] + input[17];
    output[15] = input[15] + input[16];
    output[16] = -input[16] + input[15];
    output[17] = -input[17] + input[14];
    output[18] = -input[18] + input[13];
    output[19] = -input[19] + input[12];
    output[20] = -input[20] + input[11];
    output[21] = -input[21] + input[10];
    output[22] = -input[22] + input[9];
    output[23] = -input[23] + input[8];
    output[24] = -input[24] + input[7];
    output[25] = -input[25] + input[6];
    output[26] = -input[26] + input[5];
    output[27] = -input[27] + input[4];
    output[28] = -input[28] + input[3];
    output[29] = -input[29] + input[2];
    output[30] = -input[30] + input[1];
    output[31] = -input[31] + input[0];
    // stage 2
    step[0] = output[0] + output[15];
    step[1] = output[1] + output[14];
    step[2] = output[2] + output[13];
    step[3] = output[3] + output[12];
    step[4] = output[4] + output[11];
    step[5] = output[5] + output[10];
    step[6] = output[6] + output[9];
    step[7] = output[7] + output[8];
    step[8] = -output[8] + output[7];
    step[9] = -output[9] + output[6];
    step[10] = -output[10] + output[5];
    step[11] = -output[11] + output[4];
    step[12] = -output[12] + output[3];
    step[13] = -output[13] + output[2];
    step[14] = -output[14] + output[1];
    step[15] = -output[15] + output[0];
    step[16] = output[16];
    step[17] = output[17];
    step[18] = output[18];
    step[19] = output[19];
    step[20] = half_btf(
        -cospi[32],
        output[20],
        cospi[32],
        output[27],
        cos_bit as u32,
    );
    step[21] = half_btf(
        -cospi[32],
        output[21],
        cospi[32],
        output[26],
        cos_bit as u32,
    );
    step[22] = half_btf(
        -cospi[32],
        output[22],
        cospi[32],
        output[25],
        cos_bit as u32,
    );
    step[23] = half_btf(
        -cospi[32],
        output[23],
        cospi[32],
        output[24],
        cos_bit as u32,
    );
    step[24] = half_btf(cospi[32], output[24], cospi[32], output[23], cos_bit as u32);
    step[25] = half_btf(cospi[32], output[25], cospi[32], output[22], cos_bit as u32);
    step[26] = half_btf(cospi[32], output[26], cospi[32], output[21], cos_bit as u32);
    step[27] = half_btf(cospi[32], output[27], cospi[32], output[20], cos_bit as u32);
    step[28] = output[28];
    step[29] = output[29];
    step[30] = output[30];
    step[31] = output[31];
    // stage 3
    output[0] = step[0] + step[7];
    output[1] = step[1] + step[6];
    output[2] = step[2] + step[5];
    output[3] = step[3] + step[4];
    output[4] = -step[4] + step[3];
    output[5] = -step[5] + step[2];
    output[6] = -step[6] + step[1];
    output[7] = -step[7] + step[0];
    output[8] = step[8];
    output[9] = step[9];
    output[10] = half_btf(-cospi[32], step[10], cospi[32], step[13], cos_bit as u32);
    output[11] = half_btf(-cospi[32], step[11], cospi[32], step[12], cos_bit as u32);
    output[12] = half_btf(cospi[32], step[12], cospi[32], step[11], cos_bit as u32);
    output[13] = half_btf(cospi[32], step[13], cospi[32], step[10], cos_bit as u32);
    output[14] = step[14];
    output[15] = step[15];
    output[16] = step[16] + step[23];
    output[17] = step[17] + step[22];
    output[18] = step[18] + step[21];
    output[19] = step[19] + step[20];
    output[20] = -step[20] + step[19];
    output[21] = -step[21] + step[18];
    output[22] = -step[22] + step[17];
    output[23] = -step[23] + step[16];
    output[24] = -step[24] + step[31];
    output[25] = -step[25] + step[30];
    output[26] = -step[26] + step[29];
    output[27] = -step[27] + step[28];
    output[28] = step[28] + step[27];
    output[29] = step[29] + step[26];
    output[30] = step[30] + step[25];
    output[31] = step[31] + step[24];
    // stage 4
    step[0] = output[0] + output[3];
    step[1] = output[1] + output[2];
    step[4] = output[4];
    step[5] = half_btf(-cospi[32], output[5], cospi[32], output[6], cos_bit as u32);
    step[6] = half_btf(cospi[32], output[6], cospi[32], output[5], cos_bit as u32);
    step[7] = output[7];
    step[8] = output[8] + output[11];
    step[9] = output[9] + output[10];
    step[10] = -output[10] + output[9];
    step[11] = -output[11] + output[8];
    step[12] = -output[12] + output[15];
    step[13] = -output[13] + output[14];
    step[14] = output[14] + output[13];
    step[15] = output[15] + output[12];
    step[16] = output[16];
    step[17] = output[17];
    step[18] = half_btf(
        -cospi[16],
        output[18],
        cospi[48],
        output[29],
        cos_bit as u32,
    );
    step[19] = half_btf(
        -cospi[16],
        output[19],
        cospi[48],
        output[28],
        cos_bit as u32,
    );
    step[20] = half_btf(
        -cospi[48],
        output[20],
        -cospi[16],
        output[27],
        cos_bit as u32,
    );
    step[21] = half_btf(
        -cospi[48],
        output[21],
        -cospi[16],
        output[26],
        cos_bit as u32,
    );
    step[22] = output[22];
    step[23] = output[23];
    step[24] = output[24];
    step[25] = output[25];
    step[26] = half_btf(
        cospi[48],
        output[26],
        -cospi[16],
        output[21],
        cos_bit as u32,
    );
    step[27] = half_btf(
        cospi[48],
        output[27],
        -cospi[16],
        output[20],
        cos_bit as u32,
    );
    step[28] = half_btf(cospi[16], output[28], cospi[48], output[19], cos_bit as u32);
    step[29] = half_btf(cospi[16], output[29], cospi[48], output[18], cos_bit as u32);
    step[30] = output[30];
    step[31] = output[31];
    // stage 5
    output[0] = half_btf(cospi[32], step[0], cospi[32], step[1], cos_bit as u32);
    output[4] = step[4] + step[5];
    output[7] = step[7] + step[6];
    output[8] = step[8];
    output[9] = half_btf(-cospi[16], step[9], cospi[48], step[14], cos_bit as u32);
    output[10] = half_btf(-cospi[48], step[10], -cospi[16], step[13], cos_bit as u32);
    output[11] = step[11];
    output[12] = step[12];
    output[13] = half_btf(cospi[48], step[13], -cospi[16], step[10], cos_bit as u32);
    output[14] = half_btf(cospi[16], step[14], cospi[48], step[9], cos_bit as u32);
    output[15] = step[15];
    output[16] = step[16] + step[19];
    output[17] = step[17] + step[18];
    output[18] = -step[18] + step[17];
    output[19] = -step[19] + step[16];
    output[20] = -step[20] + step[23];
    output[21] = -step[21] + step[22];
    output[22] = step[22] + step[21];
    output[23] = step[23] + step[20];
    output[24] = step[24] + step[27];
    output[25] = step[25] + step[26];
    output[26] = -step[26] + step[25];
    output[27] = -step[27] + step[24];
    output[28] = -step[28] + step[31];
    output[29] = -step[29] + step[30];
    output[30] = step[30] + step[29];
    output[31] = step[31] + step[28];
    // stage 6
    step[0] = output[0];
    step[4] = half_btf(cospi[56], output[4], cospi[8], output[7], cos_bit as u32);
    step[8] = output[8] + output[9];
    step[11] = output[11] + output[10];
    step[12] = output[12] + output[13];
    step[15] = output[15] + output[14];
    step[16] = output[16];
    step[17] = half_btf(-cospi[8], output[17], cospi[56], output[30], cos_bit as u32);
    step[18] = half_btf(
        -cospi[56],
        output[18],
        -cospi[8],
        output[29],
        cos_bit as u32,
    );
    step[19] = output[19];
    step[20] = output[20];
    step[21] = half_btf(
        -cospi[40],
        output[21],
        cospi[24],
        output[26],
        cos_bit as u32,
    );
    step[22] = half_btf(
        -cospi[24],
        output[22],
        -cospi[40],
        output[25],
        cos_bit as u32,
    );
    step[23] = output[23];
    step[24] = output[24];
    step[25] = half_btf(
        cospi[24],
        output[25],
        -cospi[40],
        output[22],
        cos_bit as u32,
    );
    step[26] = half_btf(cospi[40], output[26], cospi[24], output[21], cos_bit as u32);
    step[27] = output[27];
    step[28] = output[28];
    step[29] = half_btf(cospi[56], output[29], -cospi[8], output[18], cos_bit as u32);
    step[30] = half_btf(cospi[8], output[30], cospi[56], output[17], cos_bit as u32);
    step[31] = output[31];
    // stage 7
    output[0] = step[0];
    output[4] = step[4];
    output[8] = half_btf(cospi[60], step[8], cospi[4], step[15], cos_bit as u32);
    output[12] = half_btf(cospi[12], step[12], -cospi[52], step[11], cos_bit as u32);
    output[16] = step[16] + step[17];
    output[19] = step[19] + step[18];
    output[20] = step[20] + step[21];
    output[23] = step[23] + step[22];
    output[24] = step[24] + step[25];
    output[27] = step[27] + step[26];
    output[28] = step[28] + step[29];
    output[31] = step[31] + step[30];
    // stage 8
    step[0] = output[0];
    step[4] = output[4];
    step[8] = output[8];
    step[12] = output[12];
    step[16] = half_btf(cospi[62], output[16], cospi[2], output[31], cos_bit as u32);
    step[20] = half_btf(cospi[54], output[20], cospi[10], output[27], cos_bit as u32);
    step[24] = half_btf(cospi[6], output[24], -cospi[58], output[23], cos_bit as u32);
    step[28] = half_btf(
        cospi[14],
        output[28],
        -cospi[50],
        output[19],
        cos_bit as u32,
    );
    // stage 9
    output[0] = step[0];
    output[1] = step[16];
    output[2] = step[8];
    output[3] = step[24];
    output[4] = step[4];
    output[5] = step[20];
    output[6] = step[12];
    output[7] = step[28];
}

/// Port of C `svt_av1_fdct64_new_N2` (transforms.c).
pub fn fdct64_n2(input: &[i32], output: &mut [i32], cos_bit: i8) {
    let cospi = cospi_arr(cos_bit);
    let step = &mut [0i32; 64];
    // stage 0;
    // stage 1;
    output[0] = input[0] + input[63];
    output[1] = input[1] + input[62];
    output[2] = input[2] + input[61];
    output[3] = input[3] + input[60];
    output[4] = input[4] + input[59];
    output[5] = input[5] + input[58];
    output[6] = input[6] + input[57];
    output[7] = input[7] + input[56];
    output[8] = input[8] + input[55];
    output[9] = input[9] + input[54];
    output[10] = input[10] + input[53];
    output[11] = input[11] + input[52];
    output[12] = input[12] + input[51];
    output[13] = input[13] + input[50];
    output[14] = input[14] + input[49];
    output[15] = input[15] + input[48];
    output[16] = input[16] + input[47];
    output[17] = input[17] + input[46];
    output[18] = input[18] + input[45];
    output[19] = input[19] + input[44];
    output[20] = input[20] + input[43];
    output[21] = input[21] + input[42];
    output[22] = input[22] + input[41];
    output[23] = input[23] + input[40];
    output[24] = input[24] + input[39];
    output[25] = input[25] + input[38];
    output[26] = input[26] + input[37];
    output[27] = input[27] + input[36];
    output[28] = input[28] + input[35];
    output[29] = input[29] + input[34];
    output[30] = input[30] + input[33];
    output[31] = input[31] + input[32];
    output[32] = -input[32] + input[31];
    output[33] = -input[33] + input[30];
    output[34] = -input[34] + input[29];
    output[35] = -input[35] + input[28];
    output[36] = -input[36] + input[27];
    output[37] = -input[37] + input[26];
    output[38] = -input[38] + input[25];
    output[39] = -input[39] + input[24];
    output[40] = -input[40] + input[23];
    output[41] = -input[41] + input[22];
    output[42] = -input[42] + input[21];
    output[43] = -input[43] + input[20];
    output[44] = -input[44] + input[19];
    output[45] = -input[45] + input[18];
    output[46] = -input[46] + input[17];
    output[47] = -input[47] + input[16];
    output[48] = -input[48] + input[15];
    output[49] = -input[49] + input[14];
    output[50] = -input[50] + input[13];
    output[51] = -input[51] + input[12];
    output[52] = -input[52] + input[11];
    output[53] = -input[53] + input[10];
    output[54] = -input[54] + input[9];
    output[55] = -input[55] + input[8];
    output[56] = -input[56] + input[7];
    output[57] = -input[57] + input[6];
    output[58] = -input[58] + input[5];
    output[59] = -input[59] + input[4];
    output[60] = -input[60] + input[3];
    output[61] = -input[61] + input[2];
    output[62] = -input[62] + input[1];
    output[63] = -input[63] + input[0];
    // stage 2
    step[0] = output[0] + output[31];
    step[1] = output[1] + output[30];
    step[2] = output[2] + output[29];
    step[3] = output[3] + output[28];
    step[4] = output[4] + output[27];
    step[5] = output[5] + output[26];
    step[6] = output[6] + output[25];
    step[7] = output[7] + output[24];
    step[8] = output[8] + output[23];
    step[9] = output[9] + output[22];
    step[10] = output[10] + output[21];
    step[11] = output[11] + output[20];
    step[12] = output[12] + output[19];
    step[13] = output[13] + output[18];
    step[14] = output[14] + output[17];
    step[15] = output[15] + output[16];
    step[16] = -output[16] + output[15];
    step[17] = -output[17] + output[14];
    step[18] = -output[18] + output[13];
    step[19] = -output[19] + output[12];
    step[20] = -output[20] + output[11];
    step[21] = -output[21] + output[10];
    step[22] = -output[22] + output[9];
    step[23] = -output[23] + output[8];
    step[24] = -output[24] + output[7];
    step[25] = -output[25] + output[6];
    step[26] = -output[26] + output[5];
    step[27] = -output[27] + output[4];
    step[28] = -output[28] + output[3];
    step[29] = -output[29] + output[2];
    step[30] = -output[30] + output[1];
    step[31] = -output[31] + output[0];
    step[32] = output[32];
    step[33] = output[33];
    step[34] = output[34];
    step[35] = output[35];
    step[36] = output[36];
    step[37] = output[37];
    step[38] = output[38];
    step[39] = output[39];
    step[40] = half_btf(
        -cospi[32],
        output[40],
        cospi[32],
        output[55],
        cos_bit as u32,
    );
    step[41] = half_btf(
        -cospi[32],
        output[41],
        cospi[32],
        output[54],
        cos_bit as u32,
    );
    step[42] = half_btf(
        -cospi[32],
        output[42],
        cospi[32],
        output[53],
        cos_bit as u32,
    );
    step[43] = half_btf(
        -cospi[32],
        output[43],
        cospi[32],
        output[52],
        cos_bit as u32,
    );
    step[44] = half_btf(
        -cospi[32],
        output[44],
        cospi[32],
        output[51],
        cos_bit as u32,
    );
    step[45] = half_btf(
        -cospi[32],
        output[45],
        cospi[32],
        output[50],
        cos_bit as u32,
    );
    step[46] = half_btf(
        -cospi[32],
        output[46],
        cospi[32],
        output[49],
        cos_bit as u32,
    );
    step[47] = half_btf(
        -cospi[32],
        output[47],
        cospi[32],
        output[48],
        cos_bit as u32,
    );
    step[48] = half_btf(cospi[32], output[48], cospi[32], output[47], cos_bit as u32);
    step[49] = half_btf(cospi[32], output[49], cospi[32], output[46], cos_bit as u32);
    step[50] = half_btf(cospi[32], output[50], cospi[32], output[45], cos_bit as u32);
    step[51] = half_btf(cospi[32], output[51], cospi[32], output[44], cos_bit as u32);
    step[52] = half_btf(cospi[32], output[52], cospi[32], output[43], cos_bit as u32);
    step[53] = half_btf(cospi[32], output[53], cospi[32], output[42], cos_bit as u32);
    step[54] = half_btf(cospi[32], output[54], cospi[32], output[41], cos_bit as u32);
    step[55] = half_btf(cospi[32], output[55], cospi[32], output[40], cos_bit as u32);
    step[56] = output[56];
    step[57] = output[57];
    step[58] = output[58];
    step[59] = output[59];
    step[60] = output[60];
    step[61] = output[61];
    step[62] = output[62];
    step[63] = output[63];
    // stage 3
    output[0] = step[0] + step[15];
    output[1] = step[1] + step[14];
    output[2] = step[2] + step[13];
    output[3] = step[3] + step[12];
    output[4] = step[4] + step[11];
    output[5] = step[5] + step[10];
    output[6] = step[6] + step[9];
    output[7] = step[7] + step[8];
    output[8] = -step[8] + step[7];
    output[9] = -step[9] + step[6];
    output[10] = -step[10] + step[5];
    output[11] = -step[11] + step[4];
    output[12] = -step[12] + step[3];
    output[13] = -step[13] + step[2];
    output[14] = -step[14] + step[1];
    output[15] = -step[15] + step[0];
    output[16] = step[16];
    output[17] = step[17];
    output[18] = step[18];
    output[19] = step[19];
    output[20] = half_btf(-cospi[32], step[20], cospi[32], step[27], cos_bit as u32);
    output[21] = half_btf(-cospi[32], step[21], cospi[32], step[26], cos_bit as u32);
    output[22] = half_btf(-cospi[32], step[22], cospi[32], step[25], cos_bit as u32);
    output[23] = half_btf(-cospi[32], step[23], cospi[32], step[24], cos_bit as u32);
    output[24] = half_btf(cospi[32], step[24], cospi[32], step[23], cos_bit as u32);
    output[25] = half_btf(cospi[32], step[25], cospi[32], step[22], cos_bit as u32);
    output[26] = half_btf(cospi[32], step[26], cospi[32], step[21], cos_bit as u32);
    output[27] = half_btf(cospi[32], step[27], cospi[32], step[20], cos_bit as u32);
    output[28] = step[28];
    output[29] = step[29];
    output[30] = step[30];
    output[31] = step[31];
    output[32] = step[32] + step[47];
    output[33] = step[33] + step[46];
    output[34] = step[34] + step[45];
    output[35] = step[35] + step[44];
    output[36] = step[36] + step[43];
    output[37] = step[37] + step[42];
    output[38] = step[38] + step[41];
    output[39] = step[39] + step[40];
    output[40] = -step[40] + step[39];
    output[41] = -step[41] + step[38];
    output[42] = -step[42] + step[37];
    output[43] = -step[43] + step[36];
    output[44] = -step[44] + step[35];
    output[45] = -step[45] + step[34];
    output[46] = -step[46] + step[33];
    output[47] = -step[47] + step[32];
    output[48] = -step[48] + step[63];
    output[49] = -step[49] + step[62];
    output[50] = -step[50] + step[61];
    output[51] = -step[51] + step[60];
    output[52] = -step[52] + step[59];
    output[53] = -step[53] + step[58];
    output[54] = -step[54] + step[57];
    output[55] = -step[55] + step[56];
    output[56] = step[56] + step[55];
    output[57] = step[57] + step[54];
    output[58] = step[58] + step[53];
    output[59] = step[59] + step[52];
    output[60] = step[60] + step[51];
    output[61] = step[61] + step[50];
    output[62] = step[62] + step[49];
    output[63] = step[63] + step[48];
    // stage 4
    step[0] = output[0] + output[7];
    step[1] = output[1] + output[6];
    step[2] = output[2] + output[5];
    step[3] = output[3] + output[4];
    step[4] = -output[4] + output[3];
    step[5] = -output[5] + output[2];
    step[6] = -output[6] + output[1];
    step[7] = -output[7] + output[0];
    step[8] = output[8];
    step[9] = output[9];
    step[10] = half_btf(
        -cospi[32],
        output[10],
        cospi[32],
        output[13],
        cos_bit as u32,
    );
    step[11] = half_btf(
        -cospi[32],
        output[11],
        cospi[32],
        output[12],
        cos_bit as u32,
    );
    step[12] = half_btf(cospi[32], output[12], cospi[32], output[11], cos_bit as u32);
    step[13] = half_btf(cospi[32], output[13], cospi[32], output[10], cos_bit as u32);
    step[14] = output[14];
    step[15] = output[15];
    step[16] = output[16] + output[23];
    step[17] = output[17] + output[22];
    step[18] = output[18] + output[21];
    step[19] = output[19] + output[20];
    step[20] = -output[20] + output[19];
    step[21] = -output[21] + output[18];
    step[22] = -output[22] + output[17];
    step[23] = -output[23] + output[16];
    step[24] = -output[24] + output[31];
    step[25] = -output[25] + output[30];
    step[26] = -output[26] + output[29];
    step[27] = -output[27] + output[28];
    step[28] = output[28] + output[27];
    step[29] = output[29] + output[26];
    step[30] = output[30] + output[25];
    step[31] = output[31] + output[24];
    step[32] = output[32];
    step[33] = output[33];
    step[34] = output[34];
    step[35] = output[35];
    step[36] = half_btf(
        -cospi[16],
        output[36],
        cospi[48],
        output[59],
        cos_bit as u32,
    );
    step[37] = half_btf(
        -cospi[16],
        output[37],
        cospi[48],
        output[58],
        cos_bit as u32,
    );
    step[38] = half_btf(
        -cospi[16],
        output[38],
        cospi[48],
        output[57],
        cos_bit as u32,
    );
    step[39] = half_btf(
        -cospi[16],
        output[39],
        cospi[48],
        output[56],
        cos_bit as u32,
    );
    step[40] = half_btf(
        -cospi[48],
        output[40],
        -cospi[16],
        output[55],
        cos_bit as u32,
    );
    step[41] = half_btf(
        -cospi[48],
        output[41],
        -cospi[16],
        output[54],
        cos_bit as u32,
    );
    step[42] = half_btf(
        -cospi[48],
        output[42],
        -cospi[16],
        output[53],
        cos_bit as u32,
    );
    step[43] = half_btf(
        -cospi[48],
        output[43],
        -cospi[16],
        output[52],
        cos_bit as u32,
    );
    step[44] = output[44];
    step[45] = output[45];
    step[46] = output[46];
    step[47] = output[47];
    step[48] = output[48];
    step[49] = output[49];
    step[50] = output[50];
    step[51] = output[51];
    step[52] = half_btf(
        cospi[48],
        output[52],
        -cospi[16],
        output[43],
        cos_bit as u32,
    );
    step[53] = half_btf(
        cospi[48],
        output[53],
        -cospi[16],
        output[42],
        cos_bit as u32,
    );
    step[54] = half_btf(
        cospi[48],
        output[54],
        -cospi[16],
        output[41],
        cos_bit as u32,
    );
    step[55] = half_btf(
        cospi[48],
        output[55],
        -cospi[16],
        output[40],
        cos_bit as u32,
    );
    step[56] = half_btf(cospi[16], output[56], cospi[48], output[39], cos_bit as u32);
    step[57] = half_btf(cospi[16], output[57], cospi[48], output[38], cos_bit as u32);
    step[58] = half_btf(cospi[16], output[58], cospi[48], output[37], cos_bit as u32);
    step[59] = half_btf(cospi[16], output[59], cospi[48], output[36], cos_bit as u32);
    step[60] = output[60];
    step[61] = output[61];
    step[62] = output[62];
    step[63] = output[63];
    // stage 5
    output[0] = step[0] + step[3];
    output[1] = step[1] + step[2];
    output[2] = -step[2] + step[1];
    output[3] = -step[3] + step[0];
    output[4] = step[4];
    output[5] = half_btf(-cospi[32], step[5], cospi[32], step[6], cos_bit as u32);
    output[6] = half_btf(cospi[32], step[6], cospi[32], step[5], cos_bit as u32);
    output[7] = step[7];
    output[8] = step[8] + step[11];
    output[9] = step[9] + step[10];
    output[10] = -step[10] + step[9];
    output[11] = -step[11] + step[8];
    output[12] = -step[12] + step[15];
    output[13] = -step[13] + step[14];
    output[14] = step[14] + step[13];
    output[15] = step[15] + step[12];
    output[16] = step[16];
    output[17] = step[17];
    output[18] = half_btf(-cospi[16], step[18], cospi[48], step[29], cos_bit as u32);
    output[19] = half_btf(-cospi[16], step[19], cospi[48], step[28], cos_bit as u32);
    output[20] = half_btf(-cospi[48], step[20], -cospi[16], step[27], cos_bit as u32);
    output[21] = half_btf(-cospi[48], step[21], -cospi[16], step[26], cos_bit as u32);
    output[22] = step[22];
    output[23] = step[23];
    output[24] = step[24];
    output[25] = step[25];
    output[26] = half_btf(cospi[48], step[26], -cospi[16], step[21], cos_bit as u32);
    output[27] = half_btf(cospi[48], step[27], -cospi[16], step[20], cos_bit as u32);
    output[28] = half_btf(cospi[16], step[28], cospi[48], step[19], cos_bit as u32);
    output[29] = half_btf(cospi[16], step[29], cospi[48], step[18], cos_bit as u32);
    output[30] = step[30];
    output[31] = step[31];
    output[32] = step[32] + step[39];
    output[33] = step[33] + step[38];
    output[34] = step[34] + step[37];
    output[35] = step[35] + step[36];
    output[36] = -step[36] + step[35];
    output[37] = -step[37] + step[34];
    output[38] = -step[38] + step[33];
    output[39] = -step[39] + step[32];
    output[40] = -step[40] + step[47];
    output[41] = -step[41] + step[46];
    output[42] = -step[42] + step[45];
    output[43] = -step[43] + step[44];
    output[44] = step[44] + step[43];
    output[45] = step[45] + step[42];
    output[46] = step[46] + step[41];
    output[47] = step[47] + step[40];
    output[48] = step[48] + step[55];
    output[49] = step[49] + step[54];
    output[50] = step[50] + step[53];
    output[51] = step[51] + step[52];
    output[52] = -step[52] + step[51];
    output[53] = -step[53] + step[50];
    output[54] = -step[54] + step[49];
    output[55] = -step[55] + step[48];
    output[56] = -step[56] + step[63];
    output[57] = -step[57] + step[62];
    output[58] = -step[58] + step[61];
    output[59] = -step[59] + step[60];
    output[60] = step[60] + step[59];
    output[61] = step[61] + step[58];
    output[62] = step[62] + step[57];
    output[63] = step[63] + step[56];
    // stage 6
    step[0] = half_btf(cospi[32], output[0], cospi[32], output[1], cos_bit as u32);
    step[2] = half_btf(cospi[48], output[2], cospi[16], output[3], cos_bit as u32);
    step[4] = output[4] + output[5];
    step[5] = -output[5] + output[4];
    step[6] = -output[6] + output[7];
    step[7] = output[7] + output[6];
    step[8] = output[8];
    step[9] = half_btf(-cospi[16], output[9], cospi[48], output[14], cos_bit as u32);
    step[10] = half_btf(
        -cospi[48],
        output[10],
        -cospi[16],
        output[13],
        cos_bit as u32,
    );
    step[11] = output[11];
    step[12] = output[12];
    step[13] = half_btf(
        cospi[48],
        output[13],
        -cospi[16],
        output[10],
        cos_bit as u32,
    );
    step[14] = half_btf(cospi[16], output[14], cospi[48], output[9], cos_bit as u32);
    step[15] = output[15];
    step[16] = output[16] + output[19];
    step[17] = output[17] + output[18];
    step[18] = -output[18] + output[17];
    step[19] = -output[19] + output[16];
    step[20] = -output[20] + output[23];
    step[21] = -output[21] + output[22];
    step[22] = output[22] + output[21];
    step[23] = output[23] + output[20];
    step[24] = output[24] + output[27];
    step[25] = output[25] + output[26];
    step[26] = -output[26] + output[25];
    step[27] = -output[27] + output[24];
    step[28] = -output[28] + output[31];
    step[29] = -output[29] + output[30];
    step[30] = output[30] + output[29];
    step[31] = output[31] + output[28];
    step[32] = output[32];
    step[33] = output[33];
    step[34] = half_btf(-cospi[8], output[34], cospi[56], output[61], cos_bit as u32);
    step[35] = half_btf(-cospi[8], output[35], cospi[56], output[60], cos_bit as u32);
    step[36] = half_btf(
        -cospi[56],
        output[36],
        -cospi[8],
        output[59],
        cos_bit as u32,
    );
    step[37] = half_btf(
        -cospi[56],
        output[37],
        -cospi[8],
        output[58],
        cos_bit as u32,
    );
    step[38] = output[38];
    step[39] = output[39];
    step[40] = output[40];
    step[41] = output[41];
    step[42] = half_btf(
        -cospi[40],
        output[42],
        cospi[24],
        output[53],
        cos_bit as u32,
    );
    step[43] = half_btf(
        -cospi[40],
        output[43],
        cospi[24],
        output[52],
        cos_bit as u32,
    );
    step[44] = half_btf(
        -cospi[24],
        output[44],
        -cospi[40],
        output[51],
        cos_bit as u32,
    );
    step[45] = half_btf(
        -cospi[24],
        output[45],
        -cospi[40],
        output[50],
        cos_bit as u32,
    );
    step[46] = output[46];
    step[47] = output[47];
    step[48] = output[48];
    step[49] = output[49];
    step[50] = half_btf(
        cospi[24],
        output[50],
        -cospi[40],
        output[45],
        cos_bit as u32,
    );
    step[51] = half_btf(
        cospi[24],
        output[51],
        -cospi[40],
        output[44],
        cos_bit as u32,
    );
    step[52] = half_btf(cospi[40], output[52], cospi[24], output[43], cos_bit as u32);
    step[53] = half_btf(cospi[40], output[53], cospi[24], output[42], cos_bit as u32);
    step[54] = output[54];
    step[55] = output[55];
    step[56] = output[56];
    step[57] = output[57];
    step[58] = half_btf(cospi[56], output[58], -cospi[8], output[37], cos_bit as u32);
    step[59] = half_btf(cospi[56], output[59], -cospi[8], output[36], cos_bit as u32);
    step[60] = half_btf(cospi[8], output[60], cospi[56], output[35], cos_bit as u32);
    step[61] = half_btf(cospi[8], output[61], cospi[56], output[34], cos_bit as u32);
    step[62] = output[62];
    step[63] = output[63];
    // stage 7
    output[0] = step[0];
    output[2] = step[2];
    output[4] = half_btf(cospi[56], step[4], cospi[8], step[7], cos_bit as u32);
    output[6] = half_btf(cospi[24], step[6], -cospi[40], step[5], cos_bit as u32);
    output[8] = step[8] + step[9];
    output[9] = -step[9] + step[8];
    output[10] = -step[10] + step[11];
    output[11] = step[11] + step[10];
    output[12] = step[12] + step[13];
    output[13] = -step[13] + step[12];
    output[14] = -step[14] + step[15];
    output[15] = step[15] + step[14];
    output[16] = step[16];
    output[17] = half_btf(-cospi[8], step[17], cospi[56], step[30], cos_bit as u32);
    output[18] = half_btf(-cospi[56], step[18], -cospi[8], step[29], cos_bit as u32);
    output[19] = step[19];
    output[20] = step[20];
    output[21] = half_btf(-cospi[40], step[21], cospi[24], step[26], cos_bit as u32);
    output[22] = half_btf(-cospi[24], step[22], -cospi[40], step[25], cos_bit as u32);
    output[23] = step[23];
    output[24] = step[24];
    output[25] = half_btf(cospi[24], step[25], -cospi[40], step[22], cos_bit as u32);
    output[26] = half_btf(cospi[40], step[26], cospi[24], step[21], cos_bit as u32);
    output[27] = step[27];
    output[28] = step[28];
    output[29] = half_btf(cospi[56], step[29], -cospi[8], step[18], cos_bit as u32);
    output[30] = half_btf(cospi[8], step[30], cospi[56], step[17], cos_bit as u32);
    output[31] = step[31];
    output[32] = step[32] + step[35];
    output[33] = step[33] + step[34];
    output[34] = -step[34] + step[33];
    output[35] = -step[35] + step[32];
    output[36] = -step[36] + step[39];
    output[37] = -step[37] + step[38];
    output[38] = step[38] + step[37];
    output[39] = step[39] + step[36];
    output[40] = step[40] + step[43];
    output[41] = step[41] + step[42];
    output[42] = -step[42] + step[41];
    output[43] = -step[43] + step[40];
    output[44] = -step[44] + step[47];
    output[45] = -step[45] + step[46];
    output[46] = step[46] + step[45];
    output[47] = step[47] + step[44];
    output[48] = step[48] + step[51];
    output[49] = step[49] + step[50];
    output[50] = -step[50] + step[49];
    output[51] = -step[51] + step[48];
    output[52] = -step[52] + step[55];
    output[53] = -step[53] + step[54];
    output[54] = step[54] + step[53];
    output[55] = step[55] + step[52];
    output[56] = step[56] + step[59];
    output[57] = step[57] + step[58];
    output[58] = -step[58] + step[57];
    output[59] = -step[59] + step[56];
    output[60] = -step[60] + step[63];
    output[61] = -step[61] + step[62];
    output[62] = step[62] + step[61];
    output[63] = step[63] + step[60];
    // stage 8
    step[0] = output[0];
    step[2] = output[2];
    step[4] = output[4];
    step[6] = output[6];
    step[8] = half_btf(cospi[60], output[8], cospi[4], output[15], cos_bit as u32);
    step[10] = half_btf(cospi[44], output[10], cospi[20], output[13], cos_bit as u32);
    step[12] = half_btf(
        cospi[12],
        output[12],
        -cospi[52],
        output[11],
        cos_bit as u32,
    );
    step[14] = half_btf(cospi[28], output[14], -cospi[36], output[9], cos_bit as u32);
    step[16] = output[16] + output[17];
    step[17] = -output[17] + output[16];
    step[18] = -output[18] + output[19];
    step[19] = output[19] + output[18];
    step[20] = output[20] + output[21];
    step[21] = -output[21] + output[20];
    step[22] = -output[22] + output[23];
    step[23] = output[23] + output[22];
    step[24] = output[24] + output[25];
    step[25] = -output[25] + output[24];
    step[26] = -output[26] + output[27];
    step[27] = output[27] + output[26];
    step[28] = output[28] + output[29];
    step[29] = -output[29] + output[28];
    step[30] = -output[30] + output[31];
    step[31] = output[31] + output[30];
    step[32] = output[32];
    step[33] = half_btf(-cospi[4], output[33], cospi[60], output[62], cos_bit as u32);
    step[34] = half_btf(
        -cospi[60],
        output[34],
        -cospi[4],
        output[61],
        cos_bit as u32,
    );
    step[35] = output[35];
    step[36] = output[36];
    step[37] = half_btf(
        -cospi[36],
        output[37],
        cospi[28],
        output[58],
        cos_bit as u32,
    );
    step[38] = half_btf(
        -cospi[28],
        output[38],
        -cospi[36],
        output[57],
        cos_bit as u32,
    );
    step[39] = output[39];
    step[40] = output[40];
    step[41] = half_btf(
        -cospi[20],
        output[41],
        cospi[44],
        output[54],
        cos_bit as u32,
    );
    step[42] = half_btf(
        -cospi[44],
        output[42],
        -cospi[20],
        output[53],
        cos_bit as u32,
    );
    step[43] = output[43];
    step[44] = output[44];
    step[45] = half_btf(
        -cospi[52],
        output[45],
        cospi[12],
        output[50],
        cos_bit as u32,
    );
    step[46] = half_btf(
        -cospi[12],
        output[46],
        -cospi[52],
        output[49],
        cos_bit as u32,
    );
    step[47] = output[47];
    step[48] = output[48];
    step[49] = half_btf(
        cospi[12],
        output[49],
        -cospi[52],
        output[46],
        cos_bit as u32,
    );
    step[50] = half_btf(cospi[52], output[50], cospi[12], output[45], cos_bit as u32);
    step[51] = output[51];
    step[52] = output[52];
    step[53] = half_btf(
        cospi[44],
        output[53],
        -cospi[20],
        output[42],
        cos_bit as u32,
    );
    step[54] = half_btf(cospi[20], output[54], cospi[44], output[41], cos_bit as u32);
    step[55] = output[55];
    step[56] = output[56];
    step[57] = half_btf(
        cospi[28],
        output[57],
        -cospi[36],
        output[38],
        cos_bit as u32,
    );
    step[58] = half_btf(cospi[36], output[58], cospi[28], output[37], cos_bit as u32);
    step[59] = output[59];
    step[60] = output[60];
    step[61] = half_btf(cospi[60], output[61], -cospi[4], output[34], cos_bit as u32);
    step[62] = half_btf(cospi[4], output[62], cospi[60], output[33], cos_bit as u32);
    step[63] = output[63];
    // stage 9
    output[0] = step[0];
    output[2] = step[2];
    output[4] = step[4];
    output[6] = step[6];
    output[8] = step[8];
    output[10] = step[10];
    output[12] = step[12];
    output[14] = step[14];
    output[16] = half_btf(cospi[62], step[16], cospi[2], step[31], cos_bit as u32);
    output[18] = half_btf(cospi[46], step[18], cospi[18], step[29], cos_bit as u32);
    output[20] = half_btf(cospi[54], step[20], cospi[10], step[27], cos_bit as u32);
    output[22] = half_btf(cospi[38], step[22], cospi[26], step[25], cos_bit as u32);
    output[24] = half_btf(cospi[6], step[24], -cospi[58], step[23], cos_bit as u32);
    output[26] = half_btf(cospi[22], step[26], -cospi[42], step[21], cos_bit as u32);
    output[28] = half_btf(cospi[14], step[28], -cospi[50], step[19], cos_bit as u32);
    output[30] = half_btf(cospi[30], step[30], -cospi[34], step[17], cos_bit as u32);
    output[32] = step[32] + step[33];
    output[33] = -step[33] + step[32];
    output[34] = -step[34] + step[35];
    output[35] = step[35] + step[34];
    output[36] = step[36] + step[37];
    output[37] = -step[37] + step[36];
    output[38] = -step[38] + step[39];
    output[39] = step[39] + step[38];
    output[40] = step[40] + step[41];
    output[41] = -step[41] + step[40];
    output[42] = -step[42] + step[43];
    output[43] = step[43] + step[42];
    output[44] = step[44] + step[45];
    output[45] = -step[45] + step[44];
    output[46] = -step[46] + step[47];
    output[47] = step[47] + step[46];
    output[48] = step[48] + step[49];
    output[49] = -step[49] + step[48];
    output[50] = -step[50] + step[51];
    output[51] = step[51] + step[50];
    output[52] = step[52] + step[53];
    output[53] = -step[53] + step[52];
    output[54] = -step[54] + step[55];
    output[55] = step[55] + step[54];
    output[56] = step[56] + step[57];
    output[57] = -step[57] + step[56];
    output[58] = -step[58] + step[59];
    output[59] = step[59] + step[58];
    output[60] = step[60] + step[61];
    output[61] = -step[61] + step[60];
    output[62] = -step[62] + step[63];
    output[63] = step[63] + step[62];
    // stage 10
    step[0] = output[0];
    step[2] = output[2];
    step[4] = output[4];
    step[6] = output[6];
    step[8] = output[8];
    step[10] = output[10];
    step[12] = output[12];
    step[14] = output[14];
    step[16] = output[16];
    step[18] = output[18];
    step[20] = output[20];
    step[22] = output[22];
    step[24] = output[24];
    step[26] = output[26];
    step[28] = output[28];
    step[30] = output[30];
    step[32] = half_btf(cospi[63], output[32], cospi[1], output[63], cos_bit as u32);
    step[34] = half_btf(cospi[47], output[34], cospi[17], output[61], cos_bit as u32);
    step[36] = half_btf(cospi[55], output[36], cospi[9], output[59], cos_bit as u32);
    step[38] = half_btf(cospi[39], output[38], cospi[25], output[57], cos_bit as u32);
    step[40] = half_btf(cospi[59], output[40], cospi[5], output[55], cos_bit as u32);
    step[42] = half_btf(cospi[43], output[42], cospi[21], output[53], cos_bit as u32);
    step[44] = half_btf(cospi[51], output[44], cospi[13], output[51], cos_bit as u32);
    step[46] = half_btf(cospi[35], output[46], cospi[29], output[49], cos_bit as u32);
    step[48] = half_btf(cospi[3], output[48], -cospi[61], output[47], cos_bit as u32);
    step[50] = half_btf(
        cospi[19],
        output[50],
        -cospi[45],
        output[45],
        cos_bit as u32,
    );
    step[52] = half_btf(
        cospi[11],
        output[52],
        -cospi[53],
        output[43],
        cos_bit as u32,
    );
    step[54] = half_btf(
        cospi[27],
        output[54],
        -cospi[37],
        output[41],
        cos_bit as u32,
    );
    step[56] = half_btf(cospi[7], output[56], -cospi[57], output[39], cos_bit as u32);
    step[58] = half_btf(
        cospi[23],
        output[58],
        -cospi[41],
        output[37],
        cos_bit as u32,
    );
    step[60] = half_btf(
        cospi[15],
        output[60],
        -cospi[49],
        output[35],
        cos_bit as u32,
    );
    step[62] = half_btf(
        cospi[31],
        output[62],
        -cospi[33],
        output[33],
        cos_bit as u32,
    );
    // stage 11
    output[0] = step[0];
    output[1] = step[32];
    output[2] = step[16];
    output[3] = step[48];
    output[4] = step[8];
    output[5] = step[40];
    output[6] = step[24];
    output[7] = step[56];
    output[8] = step[4];
    output[9] = step[36];
    output[10] = step[20];
    output[11] = step[52];
    output[12] = step[12];
    output[13] = step[44];
    output[14] = step[28];
    output[15] = step[60];
    output[16] = step[2];
    output[17] = step[34];
    output[18] = step[18];
    output[19] = step[50];
    output[20] = step[10];
    output[21] = step[42];
    output[22] = step[26];
    output[23] = step[58];
    output[24] = step[6];
    output[25] = step[38];
    output[26] = step[22];
    output[27] = step[54];
    output[28] = step[14];
    output[29] = step[46];
    output[30] = step[30];
    output[31] = step[62];
}

/// Port of C `svt_av1_fdct64_new_N4` (transforms.c).
pub fn fdct64_n4(input: &[i32], output: &mut [i32], cos_bit: i8) {
    let cospi = cospi_arr(cos_bit);
    let step = &mut [0i32; 64];
    // stage 0;
    // stage 1;
    output[0] = input[0] + input[63];
    output[1] = input[1] + input[62];
    output[2] = input[2] + input[61];
    output[3] = input[3] + input[60];
    output[4] = input[4] + input[59];
    output[5] = input[5] + input[58];
    output[6] = input[6] + input[57];
    output[7] = input[7] + input[56];
    output[8] = input[8] + input[55];
    output[9] = input[9] + input[54];
    output[10] = input[10] + input[53];
    output[11] = input[11] + input[52];
    output[12] = input[12] + input[51];
    output[13] = input[13] + input[50];
    output[14] = input[14] + input[49];
    output[15] = input[15] + input[48];
    output[16] = input[16] + input[47];
    output[17] = input[17] + input[46];
    output[18] = input[18] + input[45];
    output[19] = input[19] + input[44];
    output[20] = input[20] + input[43];
    output[21] = input[21] + input[42];
    output[22] = input[22] + input[41];
    output[23] = input[23] + input[40];
    output[24] = input[24] + input[39];
    output[25] = input[25] + input[38];
    output[26] = input[26] + input[37];
    output[27] = input[27] + input[36];
    output[28] = input[28] + input[35];
    output[29] = input[29] + input[34];
    output[30] = input[30] + input[33];
    output[31] = input[31] + input[32];
    output[32] = -input[32] + input[31];
    output[33] = -input[33] + input[30];
    output[34] = -input[34] + input[29];
    output[35] = -input[35] + input[28];
    output[36] = -input[36] + input[27];
    output[37] = -input[37] + input[26];
    output[38] = -input[38] + input[25];
    output[39] = -input[39] + input[24];
    output[40] = -input[40] + input[23];
    output[41] = -input[41] + input[22];
    output[42] = -input[42] + input[21];
    output[43] = -input[43] + input[20];
    output[44] = -input[44] + input[19];
    output[45] = -input[45] + input[18];
    output[46] = -input[46] + input[17];
    output[47] = -input[47] + input[16];
    output[48] = -input[48] + input[15];
    output[49] = -input[49] + input[14];
    output[50] = -input[50] + input[13];
    output[51] = -input[51] + input[12];
    output[52] = -input[52] + input[11];
    output[53] = -input[53] + input[10];
    output[54] = -input[54] + input[9];
    output[55] = -input[55] + input[8];
    output[56] = -input[56] + input[7];
    output[57] = -input[57] + input[6];
    output[58] = -input[58] + input[5];
    output[59] = -input[59] + input[4];
    output[60] = -input[60] + input[3];
    output[61] = -input[61] + input[2];
    output[62] = -input[62] + input[1];
    output[63] = -input[63] + input[0];
    // stage 2
    step[0] = output[0] + output[31];
    step[1] = output[1] + output[30];
    step[2] = output[2] + output[29];
    step[3] = output[3] + output[28];
    step[4] = output[4] + output[27];
    step[5] = output[5] + output[26];
    step[6] = output[6] + output[25];
    step[7] = output[7] + output[24];
    step[8] = output[8] + output[23];
    step[9] = output[9] + output[22];
    step[10] = output[10] + output[21];
    step[11] = output[11] + output[20];
    step[12] = output[12] + output[19];
    step[13] = output[13] + output[18];
    step[14] = output[14] + output[17];
    step[15] = output[15] + output[16];
    step[16] = -output[16] + output[15];
    step[17] = -output[17] + output[14];
    step[18] = -output[18] + output[13];
    step[19] = -output[19] + output[12];
    step[20] = -output[20] + output[11];
    step[21] = -output[21] + output[10];
    step[22] = -output[22] + output[9];
    step[23] = -output[23] + output[8];
    step[24] = -output[24] + output[7];
    step[25] = -output[25] + output[6];
    step[26] = -output[26] + output[5];
    step[27] = -output[27] + output[4];
    step[28] = -output[28] + output[3];
    step[29] = -output[29] + output[2];
    step[30] = -output[30] + output[1];
    step[31] = -output[31] + output[0];
    step[32] = output[32];
    step[33] = output[33];
    step[34] = output[34];
    step[35] = output[35];
    step[36] = output[36];
    step[37] = output[37];
    step[38] = output[38];
    step[39] = output[39];
    step[40] = half_btf(
        -cospi[32],
        output[40],
        cospi[32],
        output[55],
        cos_bit as u32,
    );
    step[41] = half_btf(
        -cospi[32],
        output[41],
        cospi[32],
        output[54],
        cos_bit as u32,
    );
    step[42] = half_btf(
        -cospi[32],
        output[42],
        cospi[32],
        output[53],
        cos_bit as u32,
    );
    step[43] = half_btf(
        -cospi[32],
        output[43],
        cospi[32],
        output[52],
        cos_bit as u32,
    );
    step[44] = half_btf(
        -cospi[32],
        output[44],
        cospi[32],
        output[51],
        cos_bit as u32,
    );
    step[45] = half_btf(
        -cospi[32],
        output[45],
        cospi[32],
        output[50],
        cos_bit as u32,
    );
    step[46] = half_btf(
        -cospi[32],
        output[46],
        cospi[32],
        output[49],
        cos_bit as u32,
    );
    step[47] = half_btf(
        -cospi[32],
        output[47],
        cospi[32],
        output[48],
        cos_bit as u32,
    );
    step[48] = half_btf(cospi[32], output[48], cospi[32], output[47], cos_bit as u32);
    step[49] = half_btf(cospi[32], output[49], cospi[32], output[46], cos_bit as u32);
    step[50] = half_btf(cospi[32], output[50], cospi[32], output[45], cos_bit as u32);
    step[51] = half_btf(cospi[32], output[51], cospi[32], output[44], cos_bit as u32);
    step[52] = half_btf(cospi[32], output[52], cospi[32], output[43], cos_bit as u32);
    step[53] = half_btf(cospi[32], output[53], cospi[32], output[42], cos_bit as u32);
    step[54] = half_btf(cospi[32], output[54], cospi[32], output[41], cos_bit as u32);
    step[55] = half_btf(cospi[32], output[55], cospi[32], output[40], cos_bit as u32);
    step[56] = output[56];
    step[57] = output[57];
    step[58] = output[58];
    step[59] = output[59];
    step[60] = output[60];
    step[61] = output[61];
    step[62] = output[62];
    step[63] = output[63];
    // stage 3
    output[0] = step[0] + step[15];
    output[1] = step[1] + step[14];
    output[2] = step[2] + step[13];
    output[3] = step[3] + step[12];
    output[4] = step[4] + step[11];
    output[5] = step[5] + step[10];
    output[6] = step[6] + step[9];
    output[7] = step[7] + step[8];
    output[8] = -step[8] + step[7];
    output[9] = -step[9] + step[6];
    output[10] = -step[10] + step[5];
    output[11] = -step[11] + step[4];
    output[12] = -step[12] + step[3];
    output[13] = -step[13] + step[2];
    output[14] = -step[14] + step[1];
    output[15] = -step[15] + step[0];
    output[16] = step[16];
    output[17] = step[17];
    output[18] = step[18];
    output[19] = step[19];
    output[20] = half_btf(-cospi[32], step[20], cospi[32], step[27], cos_bit as u32);
    output[21] = half_btf(-cospi[32], step[21], cospi[32], step[26], cos_bit as u32);
    output[22] = half_btf(-cospi[32], step[22], cospi[32], step[25], cos_bit as u32);
    output[23] = half_btf(-cospi[32], step[23], cospi[32], step[24], cos_bit as u32);
    output[24] = half_btf(cospi[32], step[24], cospi[32], step[23], cos_bit as u32);
    output[25] = half_btf(cospi[32], step[25], cospi[32], step[22], cos_bit as u32);
    output[26] = half_btf(cospi[32], step[26], cospi[32], step[21], cos_bit as u32);
    output[27] = half_btf(cospi[32], step[27], cospi[32], step[20], cos_bit as u32);
    output[28] = step[28];
    output[29] = step[29];
    output[30] = step[30];
    output[31] = step[31];
    output[32] = step[32] + step[47];
    output[33] = step[33] + step[46];
    output[34] = step[34] + step[45];
    output[35] = step[35] + step[44];
    output[36] = step[36] + step[43];
    output[37] = step[37] + step[42];
    output[38] = step[38] + step[41];
    output[39] = step[39] + step[40];
    output[40] = -step[40] + step[39];
    output[41] = -step[41] + step[38];
    output[42] = -step[42] + step[37];
    output[43] = -step[43] + step[36];
    output[44] = -step[44] + step[35];
    output[45] = -step[45] + step[34];
    output[46] = -step[46] + step[33];
    output[47] = -step[47] + step[32];
    output[48] = -step[48] + step[63];
    output[49] = -step[49] + step[62];
    output[50] = -step[50] + step[61];
    output[51] = -step[51] + step[60];
    output[52] = -step[52] + step[59];
    output[53] = -step[53] + step[58];
    output[54] = -step[54] + step[57];
    output[55] = -step[55] + step[56];
    output[56] = step[56] + step[55];
    output[57] = step[57] + step[54];
    output[58] = step[58] + step[53];
    output[59] = step[59] + step[52];
    output[60] = step[60] + step[51];
    output[61] = step[61] + step[50];
    output[62] = step[62] + step[49];
    output[63] = step[63] + step[48];
    // stage 4
    step[0] = output[0] + output[7];
    step[1] = output[1] + output[6];
    step[2] = output[2] + output[5];
    step[3] = output[3] + output[4];
    step[4] = -output[4] + output[3];
    step[5] = -output[5] + output[2];
    step[6] = -output[6] + output[1];
    step[7] = -output[7] + output[0];
    step[8] = output[8];
    step[9] = output[9];
    step[10] = half_btf(
        -cospi[32],
        output[10],
        cospi[32],
        output[13],
        cos_bit as u32,
    );
    step[11] = half_btf(
        -cospi[32],
        output[11],
        cospi[32],
        output[12],
        cos_bit as u32,
    );
    step[12] = half_btf(cospi[32], output[12], cospi[32], output[11], cos_bit as u32);
    step[13] = half_btf(cospi[32], output[13], cospi[32], output[10], cos_bit as u32);
    step[14] = output[14];
    step[15] = output[15];
    step[16] = output[16] + output[23];
    step[17] = output[17] + output[22];
    step[18] = output[18] + output[21];
    step[19] = output[19] + output[20];
    step[20] = -output[20] + output[19];
    step[21] = -output[21] + output[18];
    step[22] = -output[22] + output[17];
    step[23] = -output[23] + output[16];
    step[24] = -output[24] + output[31];
    step[25] = -output[25] + output[30];
    step[26] = -output[26] + output[29];
    step[27] = -output[27] + output[28];
    step[28] = output[28] + output[27];
    step[29] = output[29] + output[26];
    step[30] = output[30] + output[25];
    step[31] = output[31] + output[24];
    step[32] = output[32];
    step[33] = output[33];
    step[34] = output[34];
    step[35] = output[35];
    step[36] = half_btf(
        -cospi[16],
        output[36],
        cospi[48],
        output[59],
        cos_bit as u32,
    );
    step[37] = half_btf(
        -cospi[16],
        output[37],
        cospi[48],
        output[58],
        cos_bit as u32,
    );
    step[38] = half_btf(
        -cospi[16],
        output[38],
        cospi[48],
        output[57],
        cos_bit as u32,
    );
    step[39] = half_btf(
        -cospi[16],
        output[39],
        cospi[48],
        output[56],
        cos_bit as u32,
    );
    step[40] = half_btf(
        -cospi[48],
        output[40],
        -cospi[16],
        output[55],
        cos_bit as u32,
    );
    step[41] = half_btf(
        -cospi[48],
        output[41],
        -cospi[16],
        output[54],
        cos_bit as u32,
    );
    step[42] = half_btf(
        -cospi[48],
        output[42],
        -cospi[16],
        output[53],
        cos_bit as u32,
    );
    step[43] = half_btf(
        -cospi[48],
        output[43],
        -cospi[16],
        output[52],
        cos_bit as u32,
    );
    step[44] = output[44];
    step[45] = output[45];
    step[46] = output[46];
    step[47] = output[47];
    step[48] = output[48];
    step[49] = output[49];
    step[50] = output[50];
    step[51] = output[51];
    step[52] = half_btf(
        cospi[48],
        output[52],
        -cospi[16],
        output[43],
        cos_bit as u32,
    );
    step[53] = half_btf(
        cospi[48],
        output[53],
        -cospi[16],
        output[42],
        cos_bit as u32,
    );
    step[54] = half_btf(
        cospi[48],
        output[54],
        -cospi[16],
        output[41],
        cos_bit as u32,
    );
    step[55] = half_btf(
        cospi[48],
        output[55],
        -cospi[16],
        output[40],
        cos_bit as u32,
    );
    step[56] = half_btf(cospi[16], output[56], cospi[48], output[39], cos_bit as u32);
    step[57] = half_btf(cospi[16], output[57], cospi[48], output[38], cos_bit as u32);
    step[58] = half_btf(cospi[16], output[58], cospi[48], output[37], cos_bit as u32);
    step[59] = half_btf(cospi[16], output[59], cospi[48], output[36], cos_bit as u32);
    step[60] = output[60];
    step[61] = output[61];
    step[62] = output[62];
    step[63] = output[63];
    // stage 5
    output[0] = step[0] + step[3];
    output[1] = step[1] + step[2];
    output[4] = step[4];
    output[5] = half_btf(-cospi[32], step[5], cospi[32], step[6], cos_bit as u32);
    output[6] = half_btf(cospi[32], step[6], cospi[32], step[5], cos_bit as u32);
    output[7] = step[7];
    output[8] = step[8] + step[11];
    output[9] = step[9] + step[10];
    output[10] = -step[10] + step[9];
    output[11] = -step[11] + step[8];
    output[12] = -step[12] + step[15];
    output[13] = -step[13] + step[14];
    output[14] = step[14] + step[13];
    output[15] = step[15] + step[12];
    output[16] = step[16];
    output[17] = step[17];
    output[18] = half_btf(-cospi[16], step[18], cospi[48], step[29], cos_bit as u32);
    output[19] = half_btf(-cospi[16], step[19], cospi[48], step[28], cos_bit as u32);
    output[20] = half_btf(-cospi[48], step[20], -cospi[16], step[27], cos_bit as u32);
    output[21] = half_btf(-cospi[48], step[21], -cospi[16], step[26], cos_bit as u32);
    output[22] = step[22];
    output[23] = step[23];
    output[24] = step[24];
    output[25] = step[25];
    output[26] = half_btf(cospi[48], step[26], -cospi[16], step[21], cos_bit as u32);
    output[27] = half_btf(cospi[48], step[27], -cospi[16], step[20], cos_bit as u32);
    output[28] = half_btf(cospi[16], step[28], cospi[48], step[19], cos_bit as u32);
    output[29] = half_btf(cospi[16], step[29], cospi[48], step[18], cos_bit as u32);
    output[30] = step[30];
    output[31] = step[31];
    output[32] = step[32] + step[39];
    output[33] = step[33] + step[38];
    output[34] = step[34] + step[37];
    output[35] = step[35] + step[36];
    output[36] = -step[36] + step[35];
    output[37] = -step[37] + step[34];
    output[38] = -step[38] + step[33];
    output[39] = -step[39] + step[32];
    output[40] = -step[40] + step[47];
    output[41] = -step[41] + step[46];
    output[42] = -step[42] + step[45];
    output[43] = -step[43] + step[44];
    output[44] = step[44] + step[43];
    output[45] = step[45] + step[42];
    output[46] = step[46] + step[41];
    output[47] = step[47] + step[40];
    output[48] = step[48] + step[55];
    output[49] = step[49] + step[54];
    output[50] = step[50] + step[53];
    output[51] = step[51] + step[52];
    output[52] = -step[52] + step[51];
    output[53] = -step[53] + step[50];
    output[54] = -step[54] + step[49];
    output[55] = -step[55] + step[48];
    output[56] = -step[56] + step[63];
    output[57] = -step[57] + step[62];
    output[58] = -step[58] + step[61];
    output[59] = -step[59] + step[60];
    output[60] = step[60] + step[59];
    output[61] = step[61] + step[58];
    output[62] = step[62] + step[57];
    output[63] = step[63] + step[56];
    // stage 6
    step[0] = half_btf(cospi[32], output[0], cospi[32], output[1], cos_bit as u32);
    step[4] = output[4] + output[5];
    step[7] = output[7] + output[6];
    step[8] = output[8];
    step[9] = half_btf(-cospi[16], output[9], cospi[48], output[14], cos_bit as u32);
    step[10] = half_btf(
        -cospi[48],
        output[10],
        -cospi[16],
        output[13],
        cos_bit as u32,
    );
    step[11] = output[11];
    step[12] = output[12];
    step[13] = half_btf(
        cospi[48],
        output[13],
        -cospi[16],
        output[10],
        cos_bit as u32,
    );
    step[14] = half_btf(cospi[16], output[14], cospi[48], output[9], cos_bit as u32);
    step[15] = output[15];
    step[16] = output[16] + output[19];
    step[17] = output[17] + output[18];
    step[18] = -output[18] + output[17];
    step[19] = -output[19] + output[16];
    step[20] = -output[20] + output[23];
    step[21] = -output[21] + output[22];
    step[22] = output[22] + output[21];
    step[23] = output[23] + output[20];
    step[24] = output[24] + output[27];
    step[25] = output[25] + output[26];
    step[26] = -output[26] + output[25];
    step[27] = -output[27] + output[24];
    step[28] = -output[28] + output[31];
    step[29] = -output[29] + output[30];
    step[30] = output[30] + output[29];
    step[31] = output[31] + output[28];
    step[32] = output[32];
    step[33] = output[33];
    step[34] = half_btf(-cospi[8], output[34], cospi[56], output[61], cos_bit as u32);
    step[35] = half_btf(-cospi[8], output[35], cospi[56], output[60], cos_bit as u32);
    step[36] = half_btf(
        -cospi[56],
        output[36],
        -cospi[8],
        output[59],
        cos_bit as u32,
    );
    step[37] = half_btf(
        -cospi[56],
        output[37],
        -cospi[8],
        output[58],
        cos_bit as u32,
    );
    step[38] = output[38];
    step[39] = output[39];
    step[40] = output[40];
    step[41] = output[41];
    step[42] = half_btf(
        -cospi[40],
        output[42],
        cospi[24],
        output[53],
        cos_bit as u32,
    );
    step[43] = half_btf(
        -cospi[40],
        output[43],
        cospi[24],
        output[52],
        cos_bit as u32,
    );
    step[44] = half_btf(
        -cospi[24],
        output[44],
        -cospi[40],
        output[51],
        cos_bit as u32,
    );
    step[45] = half_btf(
        -cospi[24],
        output[45],
        -cospi[40],
        output[50],
        cos_bit as u32,
    );
    step[46] = output[46];
    step[47] = output[47];
    step[48] = output[48];
    step[49] = output[49];
    step[50] = half_btf(
        cospi[24],
        output[50],
        -cospi[40],
        output[45],
        cos_bit as u32,
    );
    step[51] = half_btf(
        cospi[24],
        output[51],
        -cospi[40],
        output[44],
        cos_bit as u32,
    );
    step[52] = half_btf(cospi[40], output[52], cospi[24], output[43], cos_bit as u32);
    step[53] = half_btf(cospi[40], output[53], cospi[24], output[42], cos_bit as u32);
    step[54] = output[54];
    step[55] = output[55];
    step[56] = output[56];
    step[57] = output[57];
    step[58] = half_btf(cospi[56], output[58], -cospi[8], output[37], cos_bit as u32);
    step[59] = half_btf(cospi[56], output[59], -cospi[8], output[36], cos_bit as u32);
    step[60] = half_btf(cospi[8], output[60], cospi[56], output[35], cos_bit as u32);
    step[61] = half_btf(cospi[8], output[61], cospi[56], output[34], cos_bit as u32);
    step[62] = output[62];
    step[63] = output[63];
    // stage 7
    output[0] = step[0];
    output[4] = half_btf(cospi[56], step[4], cospi[8], step[7], cos_bit as u32);
    output[8] = step[8] + step[9];
    output[11] = step[11] + step[10];
    output[12] = step[12] + step[13];
    output[15] = step[15] + step[14];
    output[16] = step[16];
    output[17] = half_btf(-cospi[8], step[17], cospi[56], step[30], cos_bit as u32);
    output[18] = half_btf(-cospi[56], step[18], -cospi[8], step[29], cos_bit as u32);
    output[19] = step[19];
    output[20] = step[20];
    output[21] = half_btf(-cospi[40], step[21], cospi[24], step[26], cos_bit as u32);
    output[22] = half_btf(-cospi[24], step[22], -cospi[40], step[25], cos_bit as u32);
    output[23] = step[23];
    output[24] = step[24];
    output[25] = half_btf(cospi[24], step[25], -cospi[40], step[22], cos_bit as u32);
    output[26] = half_btf(cospi[40], step[26], cospi[24], step[21], cos_bit as u32);
    output[27] = step[27];
    output[28] = step[28];
    output[29] = half_btf(cospi[56], step[29], -cospi[8], step[18], cos_bit as u32);
    output[30] = half_btf(cospi[8], step[30], cospi[56], step[17], cos_bit as u32);
    output[31] = step[31];
    output[32] = step[32] + step[35];
    output[33] = step[33] + step[34];
    output[34] = -step[34] + step[33];
    output[35] = -step[35] + step[32];
    output[36] = -step[36] + step[39];
    output[37] = -step[37] + step[38];
    output[38] = step[38] + step[37];
    output[39] = step[39] + step[36];
    output[40] = step[40] + step[43];
    output[41] = step[41] + step[42];
    output[42] = -step[42] + step[41];
    output[43] = -step[43] + step[40];
    output[44] = -step[44] + step[47];
    output[45] = -step[45] + step[46];
    output[46] = step[46] + step[45];
    output[47] = step[47] + step[44];
    output[48] = step[48] + step[51];
    output[49] = step[49] + step[50];
    output[50] = -step[50] + step[49];
    output[51] = -step[51] + step[48];
    output[52] = -step[52] + step[55];
    output[53] = -step[53] + step[54];
    output[54] = step[54] + step[53];
    output[55] = step[55] + step[52];
    output[56] = step[56] + step[59];
    output[57] = step[57] + step[58];
    output[58] = -step[58] + step[57];
    output[59] = -step[59] + step[56];
    output[60] = -step[60] + step[63];
    output[61] = -step[61] + step[62];
    output[62] = step[62] + step[61];
    output[63] = step[63] + step[60];
    // stage 8
    step[0] = output[0];
    step[4] = output[4];
    step[8] = half_btf(cospi[60], output[8], cospi[4], output[15], cos_bit as u32);
    step[12] = half_btf(
        cospi[12],
        output[12],
        -cospi[52],
        output[11],
        cos_bit as u32,
    );
    step[16] = output[16] + output[17];
    step[19] = output[19] + output[18];
    step[20] = output[20] + output[21];
    step[23] = output[23] + output[22];
    step[24] = output[24] + output[25];
    step[27] = output[27] + output[26];
    step[28] = output[28] + output[29];
    step[31] = output[31] + output[30];
    step[32] = output[32];
    step[33] = half_btf(-cospi[4], output[33], cospi[60], output[62], cos_bit as u32);
    step[34] = half_btf(
        -cospi[60],
        output[34],
        -cospi[4],
        output[61],
        cos_bit as u32,
    );
    step[35] = output[35];
    step[36] = output[36];
    step[37] = half_btf(
        -cospi[36],
        output[37],
        cospi[28],
        output[58],
        cos_bit as u32,
    );
    step[38] = half_btf(
        -cospi[28],
        output[38],
        -cospi[36],
        output[57],
        cos_bit as u32,
    );
    step[39] = output[39];
    step[40] = output[40];
    step[41] = half_btf(
        -cospi[20],
        output[41],
        cospi[44],
        output[54],
        cos_bit as u32,
    );
    step[42] = half_btf(
        -cospi[44],
        output[42],
        -cospi[20],
        output[53],
        cos_bit as u32,
    );
    step[43] = output[43];
    step[44] = output[44];
    step[45] = half_btf(
        -cospi[52],
        output[45],
        cospi[12],
        output[50],
        cos_bit as u32,
    );
    step[46] = half_btf(
        -cospi[12],
        output[46],
        -cospi[52],
        output[49],
        cos_bit as u32,
    );
    step[47] = output[47];
    step[48] = output[48];
    step[49] = half_btf(
        cospi[12],
        output[49],
        -cospi[52],
        output[46],
        cos_bit as u32,
    );
    step[50] = half_btf(cospi[52], output[50], cospi[12], output[45], cos_bit as u32);
    step[51] = output[51];
    step[52] = output[52];
    step[53] = half_btf(
        cospi[44],
        output[53],
        -cospi[20],
        output[42],
        cos_bit as u32,
    );
    step[54] = half_btf(cospi[20], output[54], cospi[44], output[41], cos_bit as u32);
    step[55] = output[55];
    step[56] = output[56];
    step[57] = half_btf(
        cospi[28],
        output[57],
        -cospi[36],
        output[38],
        cos_bit as u32,
    );
    step[58] = half_btf(cospi[36], output[58], cospi[28], output[37], cos_bit as u32);
    step[59] = output[59];
    step[60] = output[60];
    step[61] = half_btf(cospi[60], output[61], -cospi[4], output[34], cos_bit as u32);
    step[62] = half_btf(cospi[4], output[62], cospi[60], output[33], cos_bit as u32);
    step[63] = output[63];
    // stage 9
    output[0] = step[0];
    output[4] = step[4];
    output[8] = step[8];
    output[12] = step[12];
    output[16] = half_btf(cospi[62], step[16], cospi[2], step[31], cos_bit as u32);
    output[20] = half_btf(cospi[54], step[20], cospi[10], step[27], cos_bit as u32);
    output[24] = half_btf(cospi[6], step[24], -cospi[58], step[23], cos_bit as u32);
    output[28] = half_btf(cospi[14], step[28], -cospi[50], step[19], cos_bit as u32);
    output[32] = step[32] + step[33];
    output[35] = step[35] + step[34];
    output[36] = step[36] + step[37];
    output[39] = step[39] + step[38];
    output[40] = step[40] + step[41];
    output[43] = step[43] + step[42];
    output[44] = step[44] + step[45];
    output[47] = step[47] + step[46];
    output[48] = step[48] + step[49];
    output[51] = step[51] + step[50];
    output[52] = step[52] + step[53];
    output[55] = step[55] + step[54];
    output[56] = step[56] + step[57];
    output[59] = step[59] + step[58];
    output[60] = step[60] + step[61];
    output[63] = step[63] + step[62];
    // stage 10
    step[0] = output[0];
    step[4] = output[4];
    step[8] = output[8];
    step[12] = output[12];
    step[16] = output[16];
    step[20] = output[20];
    step[24] = output[24];
    step[28] = output[28];
    step[32] = half_btf(cospi[63], output[32], cospi[1], output[63], cos_bit as u32);
    step[36] = half_btf(cospi[55], output[36], cospi[9], output[59], cos_bit as u32);
    step[40] = half_btf(cospi[59], output[40], cospi[5], output[55], cos_bit as u32);
    step[44] = half_btf(cospi[51], output[44], cospi[13], output[51], cos_bit as u32);
    step[48] = half_btf(cospi[3], output[48], -cospi[61], output[47], cos_bit as u32);
    step[52] = half_btf(
        cospi[11],
        output[52],
        -cospi[53],
        output[43],
        cos_bit as u32,
    );
    step[56] = half_btf(cospi[7], output[56], -cospi[57], output[39], cos_bit as u32);
    step[60] = half_btf(
        cospi[15],
        output[60],
        -cospi[49],
        output[35],
        cos_bit as u32,
    );
    // stage 11
    output[0] = step[0];
    output[1] = step[32];
    output[2] = step[16];
    output[3] = step[48];
    output[4] = step[8];
    output[5] = step[40];
    output[6] = step[24];
    output[7] = step[56];
    output[8] = step[4];
    output[9] = step[36];
    output[10] = step[20];
    output[11] = step[52];
    output[12] = step[12];
    output[13] = step[44];
    output[14] = step[28];
    output[15] = step[60];
}

/// Port of C `svt_av1_fadst8_new_N2` (transforms.c).
pub fn fadst8_n2(input: &[i32], output: &mut [i32], cos_bit: i8) {
    let cospi = cospi_arr(cos_bit);
    let step = &mut [0i32; 8];
    // stage 0;
    // stage 1;
    output[0] = input[0];
    output[1] = -input[7];
    output[2] = -input[3];
    output[3] = input[4];
    output[4] = -input[1];
    output[5] = input[6];
    output[6] = input[2];
    output[7] = -input[5];
    // stage 2
    step[0] = output[0];
    step[1] = output[1];
    step[2] = half_btf(cospi[32], output[2], cospi[32], output[3], cos_bit as u32);
    step[3] = half_btf(cospi[32], output[2], -cospi[32], output[3], cos_bit as u32);
    step[4] = output[4];
    step[5] = output[5];
    step[6] = half_btf(cospi[32], output[6], cospi[32], output[7], cos_bit as u32);
    step[7] = half_btf(cospi[32], output[6], -cospi[32], output[7], cos_bit as u32);
    // stage 3
    output[0] = step[0] + step[2];
    output[1] = step[1] + step[3];
    output[2] = step[0] - step[2];
    output[3] = step[1] - step[3];
    output[4] = step[4] + step[6];
    output[5] = step[5] + step[7];
    output[6] = step[4] - step[6];
    output[7] = step[5] - step[7];
    // stage 4
    step[0] = output[0];
    step[1] = output[1];
    step[2] = output[2];
    step[3] = output[3];
    step[4] = half_btf(cospi[16], output[4], cospi[48], output[5], cos_bit as u32);
    step[5] = half_btf(cospi[48], output[4], -cospi[16], output[5], cos_bit as u32);
    step[6] = half_btf(-cospi[48], output[6], cospi[16], output[7], cos_bit as u32);
    step[7] = half_btf(cospi[16], output[6], cospi[48], output[7], cos_bit as u32);
    // stage 5
    output[0] = step[0] + step[4];
    output[1] = step[1] + step[5];
    output[2] = step[2] + step[6];
    output[3] = step[3] + step[7];
    output[4] = step[0] - step[4];
    output[5] = step[1] - step[5];
    output[6] = step[2] - step[6];
    output[7] = step[3] - step[7];
    // stage 6
    step[1] = half_btf(cospi[60], output[0], -cospi[4], output[1], cos_bit as u32);
    step[3] = half_btf(cospi[44], output[2], -cospi[20], output[3], cos_bit as u32);
    step[4] = half_btf(cospi[36], output[4], cospi[28], output[5], cos_bit as u32);
    step[6] = half_btf(cospi[52], output[6], cospi[12], output[7], cos_bit as u32);
    // stage 7
    output[0] = step[1];
    output[1] = step[6];
    output[2] = step[3];
    output[3] = step[4];
}

/// Port of C `svt_av1_fadst8_new_N4` (transforms.c).
pub fn fadst8_n4(input: &[i32], output: &mut [i32], cos_bit: i8) {
    let cospi = cospi_arr(cos_bit);
    let step = &mut [0i32; 8];
    // stage 0;
    // stage 1;
    output[0] = input[0];
    output[1] = -input[7];
    output[2] = -input[3];
    output[3] = input[4];
    output[4] = -input[1];
    output[5] = input[6];
    output[6] = input[2];
    output[7] = -input[5];
    // stage 2
    step[0] = output[0];
    step[1] = output[1];
    step[2] = half_btf(cospi[32], output[2], cospi[32], output[3], cos_bit as u32);
    step[3] = half_btf(cospi[32], output[2], -cospi[32], output[3], cos_bit as u32);
    step[4] = output[4];
    step[5] = output[5];
    step[6] = half_btf(cospi[32], output[6], cospi[32], output[7], cos_bit as u32);
    step[7] = half_btf(cospi[32], output[6], -cospi[32], output[7], cos_bit as u32);
    // stage 3
    output[0] = step[0] + step[2];
    output[1] = step[1] + step[3];
    output[2] = step[0] - step[2];
    output[3] = step[1] - step[3];
    output[4] = step[4] + step[6];
    output[5] = step[5] + step[7];
    output[6] = step[4] - step[6];
    output[7] = step[5] - step[7];
    // stage 4
    step[0] = output[0];
    step[1] = output[1];
    step[2] = output[2];
    step[3] = output[3];
    step[4] = half_btf(cospi[16], output[4], cospi[48], output[5], cos_bit as u32);
    step[5] = half_btf(cospi[48], output[4], -cospi[16], output[5], cos_bit as u32);
    step[6] = half_btf(-cospi[48], output[6], cospi[16], output[7], cos_bit as u32);
    step[7] = half_btf(cospi[16], output[6], cospi[48], output[7], cos_bit as u32);
    // stage 5
    output[0] = step[0] + step[4];
    output[1] = step[1] + step[5];
    output[6] = step[2] - step[6];
    output[7] = step[3] - step[7];
    // stage 6
    step[1] = half_btf(cospi[60], output[0], -cospi[4], output[1], cos_bit as u32);
    step[6] = half_btf(cospi[52], output[6], cospi[12], output[7], cos_bit as u32);
    // stage 7
    output[0] = step[1];
    output[1] = step[6];
}

/// Port of C `svt_av1_fadst16_new_N2` (transforms.c).
pub fn fadst16_n2(input: &[i32], output: &mut [i32], cos_bit: i8) {
    let cospi = cospi_arr(cos_bit);
    let step = &mut [0i32; 16];
    // stage 0;
    // stage 1;
    output[0] = input[0];
    output[1] = -input[15];
    output[2] = -input[7];
    output[3] = input[8];
    output[4] = -input[3];
    output[5] = input[12];
    output[6] = input[4];
    output[7] = -input[11];
    output[8] = -input[1];
    output[9] = input[14];
    output[10] = input[6];
    output[11] = -input[9];
    output[12] = input[2];
    output[13] = -input[13];
    output[14] = -input[5];
    output[15] = input[10];
    // stage 2
    step[0] = output[0];
    step[1] = output[1];
    step[2] = half_btf(cospi[32], output[2], cospi[32], output[3], cos_bit as u32);
    step[3] = half_btf(cospi[32], output[2], -cospi[32], output[3], cos_bit as u32);
    step[4] = output[4];
    step[5] = output[5];
    step[6] = half_btf(cospi[32], output[6], cospi[32], output[7], cos_bit as u32);
    step[7] = half_btf(cospi[32], output[6], -cospi[32], output[7], cos_bit as u32);
    step[8] = output[8];
    step[9] = output[9];
    step[10] = half_btf(cospi[32], output[10], cospi[32], output[11], cos_bit as u32);
    step[11] = half_btf(
        cospi[32],
        output[10],
        -cospi[32],
        output[11],
        cos_bit as u32,
    );
    step[12] = output[12];
    step[13] = output[13];
    step[14] = half_btf(cospi[32], output[14], cospi[32], output[15], cos_bit as u32);
    step[15] = half_btf(
        cospi[32],
        output[14],
        -cospi[32],
        output[15],
        cos_bit as u32,
    );
    // stage 3
    output[0] = step[0] + step[2];
    output[1] = step[1] + step[3];
    output[2] = step[0] - step[2];
    output[3] = step[1] - step[3];
    output[4] = step[4] + step[6];
    output[5] = step[5] + step[7];
    output[6] = step[4] - step[6];
    output[7] = step[5] - step[7];
    output[8] = step[8] + step[10];
    output[9] = step[9] + step[11];
    output[10] = step[8] - step[10];
    output[11] = step[9] - step[11];
    output[12] = step[12] + step[14];
    output[13] = step[13] + step[15];
    output[14] = step[12] - step[14];
    output[15] = step[13] - step[15];
    // stage 4
    step[0] = output[0];
    step[1] = output[1];
    step[2] = output[2];
    step[3] = output[3];
    step[4] = half_btf(cospi[16], output[4], cospi[48], output[5], cos_bit as u32);
    step[5] = half_btf(cospi[48], output[4], -cospi[16], output[5], cos_bit as u32);
    step[6] = half_btf(-cospi[48], output[6], cospi[16], output[7], cos_bit as u32);
    step[7] = half_btf(cospi[16], output[6], cospi[48], output[7], cos_bit as u32);
    step[8] = output[8];
    step[9] = output[9];
    step[10] = output[10];
    step[11] = output[11];
    step[12] = half_btf(cospi[16], output[12], cospi[48], output[13], cos_bit as u32);
    step[13] = half_btf(
        cospi[48],
        output[12],
        -cospi[16],
        output[13],
        cos_bit as u32,
    );
    step[14] = half_btf(
        -cospi[48],
        output[14],
        cospi[16],
        output[15],
        cos_bit as u32,
    );
    step[15] = half_btf(cospi[16], output[14], cospi[48], output[15], cos_bit as u32);
    // stage 5
    output[0] = step[0] + step[4];
    output[1] = step[1] + step[5];
    output[2] = step[2] + step[6];
    output[3] = step[3] + step[7];
    output[4] = step[0] - step[4];
    output[5] = step[1] - step[5];
    output[6] = step[2] - step[6];
    output[7] = step[3] - step[7];
    output[8] = step[8] + step[12];
    output[9] = step[9] + step[13];
    output[10] = step[10] + step[14];
    output[11] = step[11] + step[15];
    output[12] = step[8] - step[12];
    output[13] = step[9] - step[13];
    output[14] = step[10] - step[14];
    output[15] = step[11] - step[15];
    // stage 6
    step[0] = output[0];
    step[1] = output[1];
    step[2] = output[2];
    step[3] = output[3];
    step[4] = output[4];
    step[5] = output[5];
    step[6] = output[6];
    step[7] = output[7];
    step[8] = half_btf(cospi[8], output[8], cospi[56], output[9], cos_bit as u32);
    step[9] = half_btf(cospi[56], output[8], -cospi[8], output[9], cos_bit as u32);
    step[10] = half_btf(cospi[40], output[10], cospi[24], output[11], cos_bit as u32);
    step[11] = half_btf(
        cospi[24],
        output[10],
        -cospi[40],
        output[11],
        cos_bit as u32,
    );
    step[12] = half_btf(-cospi[56], output[12], cospi[8], output[13], cos_bit as u32);
    step[13] = half_btf(cospi[8], output[12], cospi[56], output[13], cos_bit as u32);
    step[14] = half_btf(
        -cospi[24],
        output[14],
        cospi[40],
        output[15],
        cos_bit as u32,
    );
    step[15] = half_btf(cospi[40], output[14], cospi[24], output[15], cos_bit as u32);
    // stage 7
    output[0] = step[0] + step[8];
    output[1] = step[1] + step[9];
    output[2] = step[2] + step[10];
    output[3] = step[3] + step[11];
    output[4] = step[4] + step[12];
    output[5] = step[5] + step[13];
    output[6] = step[6] + step[14];
    output[7] = step[7] + step[15];
    output[8] = step[0] - step[8];
    output[9] = step[1] - step[9];
    output[10] = step[2] - step[10];
    output[11] = step[3] - step[11];
    output[12] = step[4] - step[12];
    output[13] = step[5] - step[13];
    output[14] = step[6] - step[14];
    output[15] = step[7] - step[15];
    // stage 8
    step[1] = half_btf(cospi[62], output[0], -cospi[2], output[1], cos_bit as u32);
    step[3] = half_btf(cospi[54], output[2], -cospi[10], output[3], cos_bit as u32);
    step[5] = half_btf(cospi[46], output[4], -cospi[18], output[5], cos_bit as u32);
    step[7] = half_btf(cospi[38], output[6], -cospi[26], output[7], cos_bit as u32);
    step[8] = half_btf(cospi[34], output[8], cospi[30], output[9], cos_bit as u32);
    step[10] = half_btf(cospi[42], output[10], cospi[22], output[11], cos_bit as u32);
    step[12] = half_btf(cospi[50], output[12], cospi[14], output[13], cos_bit as u32);
    step[14] = half_btf(cospi[58], output[14], cospi[6], output[15], cos_bit as u32);
    // stage 9
    output[0] = step[1];
    output[1] = step[14];
    output[2] = step[3];
    output[3] = step[12];
    output[4] = step[5];
    output[5] = step[10];
    output[6] = step[7];
    output[7] = step[8];
}

/// Port of C `svt_av1_fadst16_new_N4` (transforms.c).
pub fn fadst16_n4(input: &[i32], output: &mut [i32], cos_bit: i8) {
    let cospi = cospi_arr(cos_bit);
    let step = &mut [0i32; 16];
    // stage 0;
    // stage 1;
    output[0] = input[0];
    output[1] = -input[15];
    output[2] = -input[7];
    output[3] = input[8];
    output[4] = -input[3];
    output[5] = input[12];
    output[6] = input[4];
    output[7] = -input[11];
    output[8] = -input[1];
    output[9] = input[14];
    output[10] = input[6];
    output[11] = -input[9];
    output[12] = input[2];
    output[13] = -input[13];
    output[14] = -input[5];
    output[15] = input[10];
    // stage 2
    step[0] = output[0];
    step[1] = output[1];
    step[2] = half_btf(cospi[32], output[2], cospi[32], output[3], cos_bit as u32);
    step[3] = half_btf(cospi[32], output[2], -cospi[32], output[3], cos_bit as u32);
    step[4] = output[4];
    step[5] = output[5];
    step[6] = half_btf(cospi[32], output[6], cospi[32], output[7], cos_bit as u32);
    step[7] = half_btf(cospi[32], output[6], -cospi[32], output[7], cos_bit as u32);
    step[8] = output[8];
    step[9] = output[9];
    step[10] = half_btf(cospi[32], output[10], cospi[32], output[11], cos_bit as u32);
    step[11] = half_btf(
        cospi[32],
        output[10],
        -cospi[32],
        output[11],
        cos_bit as u32,
    );
    step[12] = output[12];
    step[13] = output[13];
    step[14] = half_btf(cospi[32], output[14], cospi[32], output[15], cos_bit as u32);
    step[15] = half_btf(
        cospi[32],
        output[14],
        -cospi[32],
        output[15],
        cos_bit as u32,
    );
    // stage 3
    output[0] = step[0] + step[2];
    output[1] = step[1] + step[3];
    output[2] = step[0] - step[2];
    output[3] = step[1] - step[3];
    output[4] = step[4] + step[6];
    output[5] = step[5] + step[7];
    output[6] = step[4] - step[6];
    output[7] = step[5] - step[7];
    output[8] = step[8] + step[10];
    output[9] = step[9] + step[11];
    output[10] = step[8] - step[10];
    output[11] = step[9] - step[11];
    output[12] = step[12] + step[14];
    output[13] = step[13] + step[15];
    output[14] = step[12] - step[14];
    output[15] = step[13] - step[15];
    // stage 4
    step[0] = output[0];
    step[1] = output[1];
    step[2] = output[2];
    step[3] = output[3];
    step[4] = half_btf(cospi[16], output[4], cospi[48], output[5], cos_bit as u32);
    step[5] = half_btf(cospi[48], output[4], -cospi[16], output[5], cos_bit as u32);
    step[6] = half_btf(-cospi[48], output[6], cospi[16], output[7], cos_bit as u32);
    step[7] = half_btf(cospi[16], output[6], cospi[48], output[7], cos_bit as u32);
    step[8] = output[8];
    step[9] = output[9];
    step[10] = output[10];
    step[11] = output[11];
    step[12] = half_btf(cospi[16], output[12], cospi[48], output[13], cos_bit as u32);
    step[13] = half_btf(
        cospi[48],
        output[12],
        -cospi[16],
        output[13],
        cos_bit as u32,
    );
    step[14] = half_btf(
        -cospi[48],
        output[14],
        cospi[16],
        output[15],
        cos_bit as u32,
    );
    step[15] = half_btf(cospi[16], output[14], cospi[48], output[15], cos_bit as u32);
    // stage 5
    output[0] = step[0] + step[4];
    output[1] = step[1] + step[5];
    output[2] = step[2] + step[6];
    output[3] = step[3] + step[7];
    output[4] = step[0] - step[4];
    output[5] = step[1] - step[5];
    output[6] = step[2] - step[6];
    output[7] = step[3] - step[7];
    output[8] = step[8] + step[12];
    output[9] = step[9] + step[13];
    output[10] = step[10] + step[14];
    output[11] = step[11] + step[15];
    output[12] = step[8] - step[12];
    output[13] = step[9] - step[13];
    output[14] = step[10] - step[14];
    output[15] = step[11] - step[15];
    // stage 6
    step[0] = output[0];
    step[1] = output[1];
    step[2] = output[2];
    step[3] = output[3];
    step[4] = output[4];
    step[5] = output[5];
    step[6] = output[6];
    step[7] = output[7];
    step[8] = half_btf(cospi[8], output[8], cospi[56], output[9], cos_bit as u32);
    step[9] = half_btf(cospi[56], output[8], -cospi[8], output[9], cos_bit as u32);
    step[10] = half_btf(cospi[40], output[10], cospi[24], output[11], cos_bit as u32);
    step[11] = half_btf(
        cospi[24],
        output[10],
        -cospi[40],
        output[11],
        cos_bit as u32,
    );
    step[12] = half_btf(-cospi[56], output[12], cospi[8], output[13], cos_bit as u32);
    step[13] = half_btf(cospi[8], output[12], cospi[56], output[13], cos_bit as u32);
    step[14] = half_btf(
        -cospi[24],
        output[14],
        cospi[40],
        output[15],
        cos_bit as u32,
    );
    step[15] = half_btf(cospi[40], output[14], cospi[24], output[15], cos_bit as u32);
    // stage 7
    output[0] = step[0] + step[8];
    output[1] = step[1] + step[9];
    output[2] = step[2] + step[10];
    output[3] = step[3] + step[11];
    output[12] = step[4] - step[12];
    output[13] = step[5] - step[13];
    output[14] = step[6] - step[14];
    output[15] = step[7] - step[15];
    // stage 8
    step[1] = half_btf(cospi[62], output[0], -cospi[2], output[1], cos_bit as u32);
    step[3] = half_btf(cospi[54], output[2], -cospi[10], output[3], cos_bit as u32);
    step[12] = half_btf(cospi[50], output[12], cospi[14], output[13], cos_bit as u32);
    step[14] = half_btf(cospi[58], output[14], cospi[6], output[15], cos_bit as u32);
    // stage 9
    output[0] = step[1];
    output[1] = step[14];
    output[2] = step[3];
    output[3] = step[12];
}
