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

use crate::fwd_txfm::{
    NEW_SQRT2, NEW_SQRT2_BITS, cospi_arr, half_btf, round_shift_array, round_shift_i64, sinpi_arr,
};
use alloc::vec;
use svtav1_types::transform::{TxSize, TxType};

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

// =============================================================================
// Transform configuration — port of `svt_aom_transform_config`
// (transforms.c:3074), `set_fwd_txfm_non_scale_range` (:3051) and
// `svt_av1_gen_fwd_stage_range` (:733), with the tables they read.
//
// The port already carries this logic, but SPLIT and re-transcribed across
// `txfm_dispatch::{flip_cfg, tx_type_to_1d, tx_size_dims}` and
// `fwd_txfm::{fwd_txfm_shift, FWD_COS_BIT_COL, FWD_COS_BIT_ROW}` with no
// binding to the real symbol. This is one struct that a single tier-1
// differential covers over all 16 tx_types x 19 tx_sizes.
// =============================================================================

/// C `MAX_TXFM_STAGE_NUM` (inv_transforms.h:25).
pub const MAX_TXFM_STAGE_NUM: usize = 12;

/// C `TxfmType` (inv_transforms.h:84). `Invalid` is C's `TXFM_TYPE_INVALID`,
/// which `av1_txfm_type_ls` yields for ADST at the 64-point row/column.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum TxfmType {
    Dct4 = 0,
    Dct8 = 1,
    Dct16 = 2,
    Dct32 = 3,
    Dct64 = 4,
    Adst4 = 5,
    Adst8 = 6,
    Adst16 = 7,
    Adst32 = 8,
    Identity4 = 9,
    Identity8 = 10,
    Identity16 = 11,
    Identity32 = 12,
    Identity64 = 13,
    /// C `TXFM_TYPE_INVALID` (== `TXFM_TYPES + 1` == 15).
    Invalid = 15,
}

/// C `av1_txfm_type_ls[5][TX_TYPES_1D]` (inv_transforms.h:191), indexed by
/// `[txwh_idx][tx_type_1d]` with `tx_type_1d` in DCT/ADST/FLIPADST/IDTX order.
const AV1_TXFM_TYPE_LS: [[TxfmType; 4]; 5] = [
    [
        TxfmType::Dct4,
        TxfmType::Adst4,
        TxfmType::Adst4,
        TxfmType::Identity4,
    ],
    [
        TxfmType::Dct8,
        TxfmType::Adst8,
        TxfmType::Adst8,
        TxfmType::Identity8,
    ],
    [
        TxfmType::Dct16,
        TxfmType::Adst16,
        TxfmType::Adst16,
        TxfmType::Identity16,
    ],
    [
        TxfmType::Dct32,
        TxfmType::Adst32,
        TxfmType::Adst32,
        TxfmType::Identity32,
    ],
    [
        TxfmType::Dct64,
        TxfmType::Invalid,
        TxfmType::Invalid,
        TxfmType::Identity64,
    ],
];

/// C `av1_txfm_stage_num_list[TXFM_TYPES]` (inv_transforms.h:197).
const AV1_TXFM_STAGE_NUM_LIST: [i8; 14] = [4, 6, 8, 10, 12, 7, 8, 10, 12, 1, 1, 1, 1, 1];

/// C `fwd_txfm_range_mult2_list[TXFM_TYPES]` (transforms.c:687), flattened
/// into a fixed 12-entry row per type (only the first `stage_num` are read).
const FWD_TXFM_RANGE_MULT2_LIST: [[i8; MAX_TXFM_STAGE_NUM]; 14] = [
    [0, 2, 3, 3, 0, 0, 0, 0, 0, 0, 0, 0],        // fdct4
    [0, 2, 4, 5, 5, 5, 0, 0, 0, 0, 0, 0],        // fdct8
    [0, 2, 4, 6, 7, 7, 7, 7, 0, 0, 0, 0],        // fdct16
    [0, 2, 4, 6, 8, 9, 9, 9, 9, 9, 0, 0],        // fdct32
    [0, 2, 4, 6, 8, 10, 11, 11, 11, 11, 11, 11], // fdct64
    [0, 2, 4, 3, 3, 3, 3, 0, 0, 0, 0, 0],        // fadst4
    [0, 0, 1, 3, 3, 5, 5, 5, 0, 0, 0, 0],        // fadst8
    [0, 0, 1, 3, 3, 5, 5, 7, 7, 7, 0, 0],        // fadst16
    [0, 0, 1, 3, 3, 5, 5, 7, 7, 9, 9, 9],        // fadst32
    [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],        // fidtx4
    [2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],        // fidtx8
    [3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],        // fidtx16
    [4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],        // fidtx32
    [5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],        // fidtx64
];

/// C `fwd_txfm_shift_ls[TX_SIZES_ALL]` (transforms.c:722), in `TxSize` order.
const FWD_TXFM_SHIFT_LS: [[i8; 3]; 19] = [
    [2, 0, 0],   // TX_4X4
    [2, -1, 0],  // TX_8X8
    [2, -2, 0],  // TX_16X16
    [2, -4, 0],  // TX_32X32
    [0, -2, -2], // TX_64X64
    [2, -1, 0],  // TX_4X8
    [2, -1, 0],  // TX_8X4
    [2, -2, 0],  // TX_8X16
    [2, -2, 0],  // TX_16X8
    [2, -4, 0],  // TX_16X32
    [2, -4, 0],  // TX_32X16
    [0, -2, -2], // TX_32X64
    [2, -4, -2], // TX_64X32
    [2, -1, 0],  // TX_4X16
    [2, -1, 0],  // TX_16X4
    [2, -2, 0],  // TX_8X32
    [2, -2, 0],  // TX_32X8
    [0, -2, 0],  // TX_16X64
    [2, -4, 0],  // TX_64X16
];

/// C `fwd_cos_bit_col[MAX_TXWH_IDX][MAX_TXWH_IDX]` (transforms.c:17).
const FWD_COS_BIT_COL: [[i8; 5]; 5] = [
    [13, 13, 13, 0, 0],
    [13, 13, 13, 12, 0],
    [13, 13, 13, 12, 13],
    [0, 13, 13, 12, 13],
    [0, 0, 13, 12, 13],
];

/// C `fwd_cos_bit_row[MAX_TXWH_IDX][MAX_TXWH_IDX]` (transforms.c:19).
const FWD_COS_BIT_ROW: [[i8; 5]; 5] = [
    [13, 13, 12, 0, 0],
    [13, 13, 13, 12, 0],
    [13, 13, 12, 13, 12],
    [0, 12, 13, 12, 11],
    [0, 0, 12, 11, 10],
];

/// C `vtx_tab[TX_TYPES]` (inv_transforms.h:45) — 0=DCT, 1=ADST, 2=FLIPADST,
/// 3=IDTX.
const VTX_TAB: [usize; 16] = [0, 1, 0, 1, 2, 0, 2, 1, 2, 3, 0, 3, 1, 3, 2, 3];
/// C `htx_tab[TX_TYPES]` (inv_transforms.h:63).
const HTX_TAB: [usize; 16] = [0, 0, 1, 1, 0, 2, 2, 2, 1, 3, 3, 0, 3, 1, 3, 2];

/// (width, height) for a `TxSize` — C `tx_size_wide` / `tx_size_high`.
pub const fn tx_size_dims(tx_size: TxSize) -> (usize, usize) {
    match tx_size {
        TxSize::Tx4x4 => (4, 4),
        TxSize::Tx8x8 => (8, 8),
        TxSize::Tx16x16 => (16, 16),
        TxSize::Tx32x32 => (32, 32),
        TxSize::Tx64x64 => (64, 64),
        TxSize::Tx4x8 => (4, 8),
        TxSize::Tx8x4 => (8, 4),
        TxSize::Tx8x16 => (8, 16),
        TxSize::Tx16x8 => (16, 8),
        TxSize::Tx16x32 => (16, 32),
        TxSize::Tx32x16 => (32, 16),
        TxSize::Tx32x64 => (32, 64),
        TxSize::Tx64x32 => (64, 32),
        TxSize::Tx4x16 => (4, 16),
        TxSize::Tx16x4 => (16, 4),
        TxSize::Tx8x32 => (8, 32),
        TxSize::Tx32x8 => (32, 8),
        TxSize::Tx16x64 => (16, 64),
        TxSize::Tx64x16 => (64, 16),
    }
}

/// C `get_flip_cfg` (inv_transforms.h:139) → `(ud_flip, lr_flip)`.
pub const fn get_flip_cfg(tx_type: TxType) -> (bool, bool) {
    match tx_type {
        TxType::FlipAdstDct | TxType::FlipAdstAdst | TxType::VFlipAdst => (true, false),
        TxType::DctFlipAdst | TxType::AdstFlipAdst | TxType::HFlipAdst => (false, true),
        TxType::FlipAdstFlipAdst => (true, true),
        _ => (false, false),
    }
}

/// C `Txfm2dFlipCfg` (inv_transforms.h:103).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Txfm2dFlipCfg {
    pub tx_size: TxSize,
    pub ud_flip: bool,
    pub lr_flip: bool,
    pub shift: [i8; 3],
    pub cos_bit_col: i8,
    pub cos_bit_row: i8,
    pub stage_range_col: [i8; MAX_TXFM_STAGE_NUM],
    pub stage_range_row: [i8; MAX_TXFM_STAGE_NUM],
    pub txfm_type_col: TxfmType,
    pub txfm_type_row: TxfmType,
    pub stage_num_col: i32,
    pub stage_num_row: i32,
}

/// Port of C `set_fwd_txfm_non_scale_range` (transforms.c:3051).
///
/// Note the index that a careless read gets wrong: the ROW loop's first term
/// is `range_mult2_**col**[cfg->stage_num_col - 1]`, the COLUMN table's last
/// live entry — not the row table's.
fn set_fwd_txfm_non_scale_range(cfg: &mut Txfm2dFlipCfg) {
    cfg.stage_range_col = [0; MAX_TXFM_STAGE_NUM];
    cfg.stage_range_row = [0; MAX_TXFM_STAGE_NUM];
    if cfg.txfm_type_col == TxfmType::Invalid {
        return;
    }
    let range_mult2_col = &FWD_TXFM_RANGE_MULT2_LIST[cfg.txfm_type_col as usize];
    let stage_num_col = (cfg.stage_num_col as usize).min(MAX_TXFM_STAGE_NUM);
    for i in 0..stage_num_col {
        cfg.stage_range_col[i] = (range_mult2_col[i] + 1) >> 1;
    }
    if cfg.txfm_type_row != TxfmType::Invalid {
        let range_mult2_row = &FWD_TXFM_RANGE_MULT2_LIST[cfg.txfm_type_row as usize];
        let stage_num_row = (cfg.stage_num_row as usize).min(MAX_TXFM_STAGE_NUM);
        for i in 0..stage_num_row {
            cfg.stage_range_row[i] =
                (range_mult2_col[cfg.stage_num_col as usize - 1] + range_mult2_row[i] + 1) >> 1;
        }
    }
}

/// Port of C `svt_aom_transform_config` (transforms.c:3074).
pub fn transform_config(tx_type: TxType, tx_size: TxSize) -> Txfm2dFlipCfg {
    let (ud_flip, lr_flip) = get_flip_cfg(tx_type);
    let (w, h) = tx_size_dims(tx_size);
    let txw_idx = w.trailing_zeros() as usize - 2;
    let txh_idx = h.trailing_zeros() as usize - 2;
    let tx_type_1d_col = VTX_TAB[tx_type as usize];
    let tx_type_1d_row = HTX_TAB[tx_type as usize];
    let txfm_type_col = AV1_TXFM_TYPE_LS[txh_idx][tx_type_1d_col];
    let txfm_type_row = AV1_TXFM_TYPE_LS[txw_idx][tx_type_1d_row];
    let stage_num_of = |t: TxfmType| -> i32 {
        if t == TxfmType::Invalid {
            // C indexes `av1_txfm_stage_num_list[TXFM_TYPE_INVALID]` out of
            // bounds here. Nothing reads the value in that case (the row loop
            // in set_fwd_txfm_non_scale_range is skipped and the kernel
            // lookup asserts), so this returns 0 rather than reproducing an
            // out-of-bounds read.
            0
        } else {
            AV1_TXFM_STAGE_NUM_LIST[t as usize] as i32
        }
    };
    let mut cfg = Txfm2dFlipCfg {
        tx_size,
        ud_flip,
        lr_flip,
        shift: FWD_TXFM_SHIFT_LS[tx_size as usize],
        cos_bit_col: FWD_COS_BIT_COL[txw_idx][txh_idx],
        cos_bit_row: FWD_COS_BIT_ROW[txw_idx][txh_idx],
        stage_range_col: [0; MAX_TXFM_STAGE_NUM],
        stage_range_row: [0; MAX_TXFM_STAGE_NUM],
        txfm_type_col,
        txfm_type_row,
        stage_num_col: stage_num_of(txfm_type_col),
        stage_num_row: stage_num_of(txfm_type_row),
    };
    set_fwd_txfm_non_scale_range(&mut cfg);
    cfg
}

/// Port of C `svt_av1_gen_fwd_stage_range` (transforms.c:733).
pub fn gen_fwd_stage_range(
    cfg: &Txfm2dFlipCfg,
    bd: i32,
) -> ([i8; MAX_TXFM_STAGE_NUM], [i8; MAX_TXFM_STAGE_NUM]) {
    let mut col = [0i8; MAX_TXFM_STAGE_NUM];
    let mut row = [0i8; MAX_TXFM_STAGE_NUM];
    let shift = cfg.shift;
    for i in 0..(cfg.stage_num_col as usize).min(MAX_TXFM_STAGE_NUM) {
        col[i] = (cfg.stage_range_col[i] as i32 + shift[0] as i32 + bd + 1) as i8;
    }
    for i in 0..(cfg.stage_num_row as usize).min(MAX_TXFM_STAGE_NUM) {
        row[i] = (cfg.stage_range_row[i] as i32 + shift[0] as i32 + shift[1] as i32 + bd + 1) as i8;
    }
    (col, row)
}

// =============================================================================
// 2-D composition — ports of `av1_tranform_two_d_core_N2_c` (transforms.c:6135)
// and `av1_tranform_two_d_core_N4_c` (:7732), plus their entry points.
// =============================================================================

/// 1-D kernel signature (C `TxfmFunc` minus the assert-only `stage_range`).
type Kernel1D = fn(&[i32], &mut [i32], i8);

/// Port of C `fwd_txfm_type_to_func_N2` (transforms.c:6099).
///
/// `TXFM_TYPE_ADST32` maps to the FULL `av1_fadst32_new` in C — there is no
/// `_N2` variant of it — so this returns `None` for it and the caller must
/// fall back to the unpruned kernel. (`av1_fadst32_new` is unreachable in
/// practice: `av1_txfm_type_ls` only yields ADST32 for a 32-point ADST, and
/// `get_fwd_txfm_func` in `fwd_txfm.rs` has the same hole.)
pub fn fwd_txfm_type_to_func_n2(t: TxfmType) -> Option<Kernel1D> {
    Some(match t {
        TxfmType::Dct4 => fdct4_n2,
        TxfmType::Dct8 => fdct8_n2,
        TxfmType::Dct16 => fdct16_n2,
        TxfmType::Dct32 => fdct32_n2,
        TxfmType::Dct64 => fdct64_n2,
        TxfmType::Adst4 => fadst4_n2,
        TxfmType::Adst8 => fadst8_n2,
        TxfmType::Adst16 => fadst16_n2,
        TxfmType::Identity4 => fidentity4_n2,
        TxfmType::Identity8 => fidentity8_n2,
        TxfmType::Identity16 => fidentity16_n2,
        TxfmType::Identity32 => fidentity32_n2,
        TxfmType::Identity64 => fidentity64_n2,
        // TXFM_TYPE_ADST32 -> av1_fadst32_new (unpruned) / TXFM_TYPE_INVALID
        TxfmType::Adst32 | TxfmType::Invalid => return None,
    })
}

/// Port of C `fwd_txfm_type_to_func_N4` (transforms.c:7696). Same ADST32
/// hole as [`fwd_txfm_type_to_func_n2`].
pub fn fwd_txfm_type_to_func_n4(t: TxfmType) -> Option<Kernel1D> {
    Some(match t {
        TxfmType::Dct4 => fdct4_n4,
        TxfmType::Dct8 => fdct8_n4,
        TxfmType::Dct16 => fdct16_n4,
        TxfmType::Dct32 => fdct32_n4,
        TxfmType::Dct64 => fdct64_n4,
        TxfmType::Adst4 => fadst4_n4,
        TxfmType::Adst8 => fadst8_n4,
        TxfmType::Adst16 => fadst16_n4,
        TxfmType::Identity4 => fidentity4_n4,
        TxfmType::Identity8 => fidentity8_n4,
        TxfmType::Identity16 => fidentity16_n4,
        TxfmType::Identity32 => fidentity32_n4,
        TxfmType::Identity64 => fidentity64_n4,
        TxfmType::Adst32 | TxfmType::Invalid => return None,
    })
}

/// Shared body of `av1_tranform_two_d_core_N2_c` / `_N4_c`, parameterised by
/// the pruning divisor `div` (2 for N2, 4 for N4). The two C functions are
/// character-identical apart from the three `/ 2` vs `/ 4` and the `>> 1` vs
/// `>> 2` in the final zeroing loop.
///
/// One faithfulness note. C aliases `temp_in` / `temp_out` INTO the caller's
/// `output` buffer, so the untouched tail of `temp_out` is caller garbage and
/// is copied into `buf`. That garbage only ever lands in rows `>= row/div`,
/// which the row pass never reads and the final loop zeroes, so a private
/// zeroed scratch (used here) produces the identical result. The tier-1
/// differential in `tests/c_parity_txfm_pf_2d.rs` pre-fills the output buffer
/// with noise on both sides, which is what proves that rather than assumes it.
#[allow(clippy::too_many_arguments)]
fn transform_two_d_core_pf(
    input: &[i16],
    input_stride: usize,
    output: &mut [i32],
    cfg: &Txfm2dFlipCfg,
    div: usize,
    col_func: Kernel1D,
    row_func: Kernel1D,
) {
    let (txfm_size_col, txfm_size_row) = tx_size_dims(cfg.tx_size);
    let shift = cfg.shift;
    // C `get_rect_tx_log_ratio(txfm_size_col, txfm_size_row)`.
    let rect_type = txfm_size_col.trailing_zeros() as i32 - txfm_size_row.trailing_zeros() as i32;
    let cos_bit_col = cfg.cos_bit_col;
    let cos_bit_row = cfg.cos_bit_row;

    let mut buf = vec![0i32; txfm_size_col * txfm_size_row];
    let mut temp_in = vec![0i32; txfm_size_row];
    let mut temp_out = vec![0i32; txfm_size_row];

    // Columns
    for c in 0..txfm_size_col {
        if !cfg.ud_flip {
            for r in 0..txfm_size_row {
                temp_in[r] = input[r * input_stride + c] as i32;
            }
        } else {
            for r in 0..txfm_size_row {
                temp_in[r] = input[(txfm_size_row - r - 1) * input_stride + c] as i32;
            }
        }
        round_shift_array(&mut temp_in, -(shift[0] as i32));
        col_func(&temp_in, &mut temp_out, cos_bit_col);
        // NOTE the length: only the first row/div entries are shifted.
        round_shift_array(&mut temp_out[..txfm_size_row / div], -(shift[1] as i32));
        if !cfg.lr_flip {
            for r in 0..txfm_size_row {
                buf[r * txfm_size_col + c] = temp_out[r];
            }
        } else {
            for r in 0..txfm_size_row {
                buf[r * txfm_size_col + (txfm_size_col - c - 1)] = temp_out[r];
            }
        }
    }

    // Rows — only the first row/div of them.
    let mut row_out = vec![0i32; txfm_size_col];
    for r in 0..txfm_size_row / div {
        row_out.copy_from_slice(&output[r * txfm_size_col..(r + 1) * txfm_size_col]);
        row_func(
            &buf[r * txfm_size_col..(r + 1) * txfm_size_col],
            &mut row_out,
            cos_bit_row,
        );
        round_shift_array(&mut row_out[..txfm_size_col / div], -(shift[2] as i32));
        if rect_type.abs() == 1 {
            for v in row_out.iter_mut().take(txfm_size_col / div) {
                *v = round_shift_i64(*v as i64 * NEW_SQRT2 as i64, NEW_SQRT2_BITS);
            }
        }
        output[r * txfm_size_col..(r + 1) * txfm_size_col].copy_from_slice(&row_out);
    }

    // Zero everything outside the top-left (col/div) x (row/div) quadrant.
    for i in 0..(txfm_size_col * txfm_size_row) {
        if i % txfm_size_col >= (txfm_size_col / div) || i / txfm_size_col >= (txfm_size_row / div)
        {
            output[i] = 0;
        }
    }
}

/// Forward 2-D transform at a reduced coefficient shape.
///
/// `shape` selects the C entry family: `N2` is `svt_aom_transform_two_d_*_N2_c`
/// / `svt_av1_fwd_txfm2d_*_N2_c`, `N4` the `_N4_c` twins, and `Default` is
/// `svt_av1_transform_two_d_*_c` / `svt_av1_fwd_txfm2d_*_c` (with `div == 1`
/// the shared core reduces exactly to `av1_tranform_two_d_core_c`). Returns
/// `false` for the `(ADST, 32)` hole where C would dispatch the unpruned
/// `av1_fadst32_new` (see [`fwd_txfm_type_to_func_n2`]), and for `OnlyDc`,
/// which has no 2-D entry family of its own.
///
/// `input` is the int16 residual at `input_stride`; `output` is a full
/// `w * h` coefficient block whose entries outside the kept quadrant are
/// zeroed, exactly as C leaves them.
pub fn fwd_txfm2d_pf(
    input: &[i16],
    output: &mut [i32],
    input_stride: usize,
    tx_size: TxSize,
    tx_type: TxType,
    shape: TxCoeffShape,
) -> bool {
    let (div, lookup): (usize, fn(TxfmType) -> Option<Kernel1D>) = match shape {
        TxCoeffShape::N2 => (2, fwd_txfm_type_to_func_n2),
        TxCoeffShape::N4 => (4, fwd_txfm_type_to_func_n4),
        TxCoeffShape::Default => (1, fwd_txfm_type_to_func_default),
        TxCoeffShape::OnlyDc => return false,
    };
    let cfg = transform_config(tx_type, tx_size);
    let (Some(col_func), Some(row_func)) = (lookup(cfg.txfm_type_col), lookup(cfg.txfm_type_row))
    else {
        return false;
    };
    transform_two_d_core_pf(input, input_stride, output, &cfg, div, col_func, row_func);
    true
}

// =============================================================================
// The DEFAULT-shape 1-D dispatch, so `fwd_txfm2d_pf` can also serve
// `TxCoeffShape::Default` — with `div == 1` the shared core reduces EXACTLY to
// C's `av1_tranform_two_d_core_c` (transforms.c:2978): `row / 1` and `col / 1`
// are the full lengths, the row loop runs over every row, and the trailing
// zeroing loop's condition (`i % col >= col`, `i / col >= row`) is never true.
// =============================================================================

/// Port of C `svt_aom_fwd_txfm_type_to_func` (transforms.c:2940), expressed
/// over the existing full 1-D kernels in [`crate::fwd_txfm`]. Same ADST32
/// hole as the reduced-shape tables.
pub fn fwd_txfm_type_to_func_default(t: TxfmType) -> Option<Kernel1D> {
    let (family, n) = match t {
        TxfmType::Dct4 => (0u8, 4usize),
        TxfmType::Dct8 => (0, 8),
        TxfmType::Dct16 => (0, 16),
        TxfmType::Dct32 => (0, 32),
        TxfmType::Dct64 => (0, 64),
        TxfmType::Adst4 => (1, 4),
        TxfmType::Adst8 => (1, 8),
        TxfmType::Adst16 => (1, 16),
        TxfmType::Adst32 => (1, 32),
        TxfmType::Identity4 => (3, 4),
        TxfmType::Identity8 => (3, 8),
        TxfmType::Identity16 => (3, 16),
        TxfmType::Identity32 => (3, 32),
        TxfmType::Identity64 => (3, 64),
        TxfmType::Invalid => return None,
    };
    crate::fwd_txfm::get_fwd_txfm_func(family, n)
}

// =============================================================================
// `svt_av1_highbd_fwd_txfm{,_n2,_n4}` (transforms.c:4476 / :4409 / :4342)
// and `svt_av1_wht_fwd_txfm` (:4527) — TPL's only transform entry.
// =============================================================================

/// What `svt_av1_highbd_fwd_txfm{,_n2,_n4}` actually dispatches to for
/// `(shape, tx_size)`.
///
/// `None` is the **TX_4X4 hole**: all three C dispatchers have
/// `case TX_4X4: //hack highbd_fwd_txfm_4x4(...); break;` (transforms.c:4388,
/// :4455, :4522), so they return leaving the caller's coeff buffer at
/// whatever it already held. TPL never asks for TX_4X4 (its smallest is
/// TX_16X4, src_ops_process.c:380-382), so this is latent — but a dispatch
/// table that "helpfully" transforms 4x4 diverges the moment anything else
/// calls this entry. Note that `av1_estimate_transform_*` does NOT share the
/// hole (transforms.c:3525/:3682/:3864 call `svt_av1_fwd_txfm2d_4x4*`
/// normally).
///
/// `Some(Default)` for `(N2|N4, TX_4X16)` is the second quirk, and it is not
/// a transcription slip: `highbd_fwd_txfm_4x16_n2` (transforms.c:4331) and
/// `_n4` (:4101) both call the FULL `svt_av1_fwd_txfm2d_4x16`, while all 17
/// of their siblings call their `_N2` / `_N4` twin. Verified by extracting
/// the callee of all 54 wrappers.
pub fn highbd_entry_shape(shape: TxCoeffShape, tx_size: TxSize) -> Option<TxCoeffShape> {
    match (shape, tx_size) {
        (_, TxSize::Tx4x4) => None,
        (TxCoeffShape::N2 | TxCoeffShape::N4, TxSize::Tx4x16) => Some(TxCoeffShape::Default),
        _ => Some(shape),
    }
}

/// Port of C `svt_av1_highbd_fwd_txfm` (`shape == Default`, transforms.c:4476),
/// `svt_av1_highbd_fwd_txfm_n2` (:4409) and `_n4` (:4342).
///
/// `coeff` must hold `w * h` entries. Returns `false` only when the (type,
/// size) pair has no kernel (the ADST32 hole); the TX_4X4 hole returns `true`
/// having written nothing, which is what C does.
pub fn highbd_fwd_txfm(
    input: &[i16],
    coeff: &mut [i32],
    diff_stride: usize,
    tx_type: TxType,
    tx_size: TxSize,
    shape: TxCoeffShape,
) -> bool {
    let Some(effective) = highbd_entry_shape(shape, tx_size) else {
        // TX_4X4: C leaves the buffer alone.
        return true;
    };
    // The 64x64 / 16x64 / 64x16 wrappers pass the DCT_DCT *literal* through
    // (the assert above them requires it); 32x64 / 64x32 forward the caller's
    // tx_type. Same in all three variants.
    let effective_type = match tx_size {
        TxSize::Tx64x64 | TxSize::Tx16x64 | TxSize::Tx64x16 => TxType::DctDct,
        _ => tx_type,
    };
    fwd_txfm2d_pf(
        input,
        coeff,
        diff_stride,
        tx_size,
        effective_type,
        effective,
    )
}

/// Port of C `svt_av1_wht_fwd_txfm` (transforms.c:4527) — TPL's ONLY
/// transform entry (`src_ops_process.c:725,857,937,1133`).
///
/// C hard-codes `tx_type = DCT_DCT`, `lossless = 0` and
/// `tx_set_type = EXT_TX_SET_ALL16`, then routes on `pf_shape`: `N4_SHAPE`
/// and `N2_SHAPE` take the matching dispatcher, and EVERY other value —
/// `DEFAULT_SHAPE` and `ONLY_DC_SHAPE` alike — falls through the `default:`
/// arm to `svt_av1_highbd_fwd_txfm`. `bw` is the source stride, not a width.
pub fn wht_fwd_txfm(
    src_diff: &[i16],
    bw: usize,
    coeff: &mut [i32],
    tx_size: TxSize,
    pf_shape: TxCoeffShape,
    _bit_depth: i32,
    _is_hbd: bool,
) -> bool {
    let shape = match pf_shape {
        TxCoeffShape::N4 => TxCoeffShape::N4,
        TxCoeffShape::N2 => TxCoeffShape::N2,
        // C `default:` — DEFAULT_SHAPE *and* ONLY_DC_SHAPE both land here.
        _ => TxCoeffShape::Default,
    };
    highbd_fwd_txfm(src_diff, coeff, bw, TxType::DctDct, tx_size, shape)
}

// =============================================================================
// `svt_handle_transform*` (transforms.c:3105-3291) — the 64-dimension fold
// and row repack. Both the full variants (which return the discarded
// three-quarter energy) and the `_N2_N4_c` variants (which return 0).
// =============================================================================

/// C `energy_computation` (transforms.c:3092): sum of squares over an
/// `area_width x area_height` window at `coeff_stride`.
fn energy_computation(
    coeff: &[i32],
    offset: usize,
    coeff_stride: usize,
    area_width: usize,
    area_height: usize,
) -> u64 {
    let mut acc: u64 = 0;
    let mut base = offset;
    for _ in 0..area_height {
        for c in 0..area_width {
            let v = coeff[base + c] as i64;
            acc = acc.wrapping_add((v * v) as u64);
        }
        base += coeff_stride;
    }
    acc
}

/// Which 64-dimension `svt_handle_transform*` entry to run.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum HandleTransform {
    /// `svt_handle_transform16x64{,_N2_N4}_c`
    T16x64,
    /// `svt_handle_transform32x64{,_N2_N4}_c`
    T32x64,
    /// `svt_handle_transform64x16{,_N2_N4}_c`
    T64x16,
    /// `svt_handle_transform64x32{,_N2_N4}_c`
    T64x32,
    /// `svt_handle_transform64x64{,_N2_N4}_c`
    T64x64,
}

/// Port of the `svt_handle_transform*` family (transforms.c:3105-3291).
///
/// `pf == false` is the full variant: it measures the energy of the region
/// AV1 discards and, for the three entries whose coefficient block is 64 wide,
/// repacks rows 1.. from stride 64 down to stride 32. `pf == true` is the
/// `_N2_N4_c` variant.
///
/// CORRECTION to a common summary of this family: the `_N2_N4_c` variants are
/// NOT all no-ops. 16x64 and 32x64 do nothing and return 0, but 64x16, 64x32
/// and 64x64 STILL DO THE ROW REPACK (transforms.c:3262, :3271, :3280) — they
/// only drop the energy term. Getting that wrong leaves the coefficients at
/// the wrong stride rather than merely mis-costing a block.
pub fn handle_transform(which: HandleTransform, pf: bool, output: &mut [i32]) -> u64 {
    let energy = if pf {
        0
    } else {
        match which {
            // bottom 16x32 area
            HandleTransform::T16x64 => energy_computation(output, 16 * 32, 16, 16, 32),
            // bottom 32x32 area
            HandleTransform::T32x64 => energy_computation(output, 32 * 32, 32, 32, 32),
            // top-right 32x16 area
            HandleTransform::T64x16 => energy_computation(output, 32, 64, 32, 16),
            // top-right 32x32 area
            HandleTransform::T64x32 => energy_computation(output, 32, 64, 32, 32),
            // top-right 32x32 area PLUS the bottom 64x32 area
            HandleTransform::T64x64 => energy_computation(output, 32, 64, 32, 32)
                .wrapping_add(energy_computation(output, 32 * 64, 64, 64, 32)),
        }
    };
    // The repack happens for both variants on the three 64-wide entries.
    let rows = match which {
        HandleTransform::T16x64 | HandleTransform::T32x64 => 0,
        HandleTransform::T64x16 => 16,
        HandleTransform::T64x32 | HandleTransform::T64x64 => 32,
    };
    for row in 1..rows {
        let (dst, src) = (row * 32, row * 64);
        for i in 0..32 {
            output[dst + i] = output[src + i];
        }
    }
    energy
}
// =============================================================================
// Inverse transform configuration — port of `svt_av1_get_inv_txfm_cfg`
// (inv_transforms.c:2469).
//
// Same correctness-gap argument as `transform_config`: the port already has
// this logic, inlined in `inv_txfm::{inv_txfm2d_core, inv_txfm_shift}` with
// no binding to the real symbol. Both sides of the transform pair should be
// gated the same way.
// =============================================================================

/// C `INV_COS_BIT` (inv_transforms.h).
pub const INV_COS_BIT: i8 = 12;

/// C `svt_aom_inv_txfm_shift_ls[TX_SIZES_ALL]` (inv_transforms.c:37), in
/// `TxSize` order. Two entries per size, not three.
const INV_TXFM_SHIFT_LS: [[i8; 2]; 19] = [
    [0, -4],  // TX_4X4
    [-1, -4], // TX_8X8
    [-2, -4], // TX_16X16
    [-2, -4], // TX_32X32
    [-2, -4], // TX_64X64
    [0, -4],  // TX_4X8
    [0, -4],  // TX_8X4
    [-1, -4], // TX_8X16
    [-1, -4], // TX_16X8
    [-1, -4], // TX_16X32
    [-1, -4], // TX_32X16
    [-1, -4], // TX_32X64
    [-1, -4], // TX_64X32
    [-1, -4], // TX_4X16
    [-1, -4], // TX_16X4
    [-2, -4], // TX_8X32
    [-2, -4], // TX_32X8
    [-2, -4], // TX_16X64
    [-2, -4], // TX_64X16
];

/// C `inv_cos_bit_col` / `inv_cos_bit_row` (inv_transforms.h:32/:38). Both
/// tables are `INV_COS_BIT` everywhere the (txw, txh) pair is legal and 0 in
/// the four illegal corners, and they are identical to each other.
const fn inv_cos_bit(txw_idx: usize, txh_idx: usize) -> i8 {
    const T: [[i8; 5]; 5] = [
        [INV_COS_BIT, INV_COS_BIT, INV_COS_BIT, 0, 0],
        [INV_COS_BIT, INV_COS_BIT, INV_COS_BIT, INV_COS_BIT, 0],
        [
            INV_COS_BIT,
            INV_COS_BIT,
            INV_COS_BIT,
            INV_COS_BIT,
            INV_COS_BIT,
        ],
        [0, INV_COS_BIT, INV_COS_BIT, INV_COS_BIT, INV_COS_BIT],
        [0, 0, INV_COS_BIT, INV_COS_BIT, INV_COS_BIT],
    ];
    T[txw_idx][txh_idx]
}

/// C `iadst4_range[7]` (inv_transforms.h:214) — the only stage range
/// `svt_av1_get_inv_txfm_cfg` ever writes.
const IADST4_RANGE: [i8; 7] = [0, 1, 0, 0, 0, 0, 0];

/// Port of C `svt_av1_get_inv_txfm_cfg` (inv_transforms.c:2469).
///
/// The returned `shift` carries C's TWO inverse shifts in slots 0 and 1;
/// slot 2 is unused and stays 0 (the forward table has three).
///
/// Unlike the forward config this one does NOT call
/// `set_fwd_txfm_non_scale_range`: the stage ranges stay all-zero except for
/// an ADST4 column or row, which gets `iadst4_range` memcpy'd over its first
/// seven entries. (C calls `set_flip_cfg` twice, once before and once after
/// zeroing the ranges — harmless, and reproduced here as a single call
/// because `set_flip_cfg` touches only `ud_flip`/`lr_flip`.)
pub fn get_inv_txfm_cfg(tx_type: TxType, tx_size: TxSize) -> Txfm2dFlipCfg {
    let (ud_flip, lr_flip) = get_flip_cfg(tx_type);
    let (w, h) = tx_size_dims(tx_size);
    let txw_idx = w.trailing_zeros() as usize - 2;
    let txh_idx = h.trailing_zeros() as usize - 2;
    let inv_shift = INV_TXFM_SHIFT_LS[tx_size as usize];
    let txfm_type_col = AV1_TXFM_TYPE_LS[txh_idx][VTX_TAB[tx_type as usize]];
    let txfm_type_row = AV1_TXFM_TYPE_LS[txw_idx][HTX_TAB[tx_type as usize]];
    let mut stage_range_col = [0i8; MAX_TXFM_STAGE_NUM];
    let mut stage_range_row = [0i8; MAX_TXFM_STAGE_NUM];
    if txfm_type_col == TxfmType::Adst4 {
        stage_range_col[..IADST4_RANGE.len()].copy_from_slice(&IADST4_RANGE);
    }
    if txfm_type_row == TxfmType::Adst4 {
        stage_range_row[..IADST4_RANGE.len()].copy_from_slice(&IADST4_RANGE);
    }
    let stage_num_of = |t: TxfmType| -> i32 {
        if t == TxfmType::Invalid {
            // C reads av1_txfm_stage_num_list out of bounds here (the
            // 64-point ADST hole); nothing consumes the value.
            0
        } else {
            AV1_TXFM_STAGE_NUM_LIST[t as usize] as i32
        }
    };
    Txfm2dFlipCfg {
        tx_size,
        ud_flip,
        lr_flip,
        shift: [inv_shift[0], inv_shift[1], 0],
        cos_bit_col: inv_cos_bit(txw_idx, txh_idx),
        cos_bit_row: inv_cos_bit(txw_idx, txh_idx),
        stage_range_col,
        stage_range_row,
        txfm_type_col,
        txfm_type_row,
        stage_num_col: stage_num_of(txfm_type_col),
        stage_num_row: stage_num_of(txfm_type_row),
    }
}

// =============================================================================
// `svt_aom_estimate_transform` (transforms.c:3938) and the four static
// shape dispatchers it fans out to — `av1_estimate_transform_default` (:3718),
// `_N2` (:3379), `_N4` (:3536) and `_ONLY_DC` (:3693). This is MD's transform
// entry, as `svt_av1_wht_fwd_txfm` is TPL's.
// =============================================================================

/// Which `svt_handle_transform*` a tx_size pulls in after the 2-D transform.
/// Only the five 64-dimension sizes have one; every other size leaves the
/// caller's `three_quad_energy` untouched.
const fn handle_transform_for(tx_size: TxSize) -> Option<HandleTransform> {
    match tx_size {
        TxSize::Tx64x32 => Some(HandleTransform::T64x32),
        TxSize::Tx32x64 => Some(HandleTransform::T32x64),
        TxSize::Tx64x16 => Some(HandleTransform::T64x16),
        TxSize::Tx16x64 => Some(HandleTransform::T16x64),
        TxSize::Tx64x64 => Some(HandleTransform::T64x64),
        _ => None,
    }
}

/// Port of C `svt_aom_estimate_transform` (transforms.c:3938).
///
/// `three_quad_energy` is written ONLY for the five 64-dimension sizes, as in
/// C — every other size leaves the caller's value alone, which is why it is a
/// `&mut` rather than a return value.
///
/// Shape routing, from the four static dispatchers:
///   * `Default` runs the unpruned 2-D entry and the FULL
///     `svt_handle_transform*` (energy measured, rows repacked);
///   * `N2` / `N4` run their pruned entry and the `_N2_N4` variant
///     (energy 0, rows still repacked on the three 64-wide sizes);
///   * `OnlyDc` runs the whole `N4` path and then zeroes every coefficient
///     except index 0.
///
/// Two things this port deliberately does NOT reproduce, both byte-neutral:
///   * C picks between the RTCD pointer and the `_c` implementation per
///     (tx_size, tx_type) — `svt_av1_fwd_txfm2d_32x16_N4` for DCT_DCT/IDTX
///     and `..._N4_c` otherwise, and so on for eight of the nineteen sizes.
///     Both sides compute the same transform (SVT's SIMD kernels are
///     bit-exact with their `_c` twins, and `tests/c_parity_txfm_pf_entry.rs`
///     measures that on the RTCD-dispatched `svt_av1_highbd_fwd_txfm*` path),
///     so the port always takes the single `_c`-faithful implementation.
///   * `bit_depth`, which every `_c` 2-D entry consumes only through
///     `svt_av1_gen_fwd_stage_range` and therefore only through asserts.
pub fn estimate_transform(
    residual: &[i16],
    residual_stride: usize,
    coeff: &mut [i32],
    tx_size: TxSize,
    tx_type: TxType,
    shape: TxCoeffShape,
    three_quad_energy: &mut u64,
    lossless: bool,
) -> bool {
    // C's lossless guard is on TX_4X4 ONLY (transforms.c:3949, "so larger
    // sizes fall through and fill the full coeff_buffer, avoiding an
    // uninitialized read. Fixes gitlab#2373").
    if lossless && tx_size == TxSize::Tx4x4 {
        let mut dst = [0i32; 16];
        crate::fwd_txfm::fwht4x4(residual, &mut dst, residual_stride);
        // C transposes the kernel output into the caller's buffer; the port's
        // `fwht4x4` deliberately leaves that to this dispatch layer.
        for i in 0..4 {
            for j in 0..4 {
                coeff[(j << 2) + i] = dst[(i << 2) + j];
            }
        }
        return true;
    }

    let inner_shape = match shape {
        // ONLY_DC_SHAPE runs the N4 dispatcher and then prunes further.
        TxCoeffShape::OnlyDc => TxCoeffShape::N4,
        other => other,
    };
    if !fwd_txfm2d_pf(
        residual,
        coeff,
        residual_stride,
        tx_size,
        tx_type,
        inner_shape,
    ) {
        return false;
    }
    if let Some(which) = handle_transform_for(tx_size) {
        *three_quad_energy = handle_transform(which, inner_shape != TxCoeffShape::Default, coeff);
    }
    if shape == TxCoeffShape::OnlyDc {
        // C `av1_estimate_transform_ONLY_DC` (transforms.c:3707). Written as
        // "zero where the index is inside the left quarter-columns OR the top
        // quarter-rows", which — on a buffer the N4 pass has already zeroed
        // outside the top-left quadrant — leaves ONLY coeff[0]. Transcribed
        // literally rather than simplified.
        let (w, h) = tx_size_dims(tx_size);
        for i in 1..(w * h) {
            if i % w < (w >> 2) || i / w < (h >> 2) {
                coeff[i] = 0;
            }
        }
    }
    true
}
