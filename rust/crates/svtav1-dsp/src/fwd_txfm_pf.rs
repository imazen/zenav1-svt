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

/// Port of C `svt_av1_gen_inv_stage_range` (inv_transforms.c:43).
///
/// The inverse twin of [`gen_fwd_stage_range`], and it does something quite
/// different: it does NOT derive the range from `cfg->stage_range_*`. It
/// computes `real_range_row/col` from them, `(void)`s the result, and then
/// writes the same bit-depth-derived constant into EVERY stage — the derived
/// value only ever feeds an `assert`. So the output is `opt_range_row` /
/// `opt_range_col` repeated `stage_num_row` / `stage_num_col` times, and the
/// `TXFM_TYPE_ADST4 && i == 1` special case (the comment says adst4 may use
/// one extra bit at stage 1) writes exactly the same value as its `else`
/// branch — it exists only to skip the assert.
///
/// `tx_size` is a separate parameter in C even though `cfg` carries one; it
/// is used only for `inv_start_range[tx_size]`, which is likewise
/// assert-only. This port takes `cfg` alone.
pub fn gen_inv_stage_range(
    cfg: &Txfm2dFlipCfg,
    bd: i32,
) -> ([i8; MAX_TXFM_STAGE_NUM], [i8; MAX_TXFM_STAGE_NUM]) {
    let (opt_range_row, opt_range_col) = match bd {
        8 => (16i8, 16i8),
        10 => (18, 16),
        // C asserts bd == 12 here; in a release build the assert is gone and
        // any other depth takes this arm.
        _ => (20, 18),
    };
    let mut col = [0i8; MAX_TXFM_STAGE_NUM];
    let mut row = [0i8; MAX_TXFM_STAGE_NUM];
    for v in row
        .iter_mut()
        .take((cfg.stage_num_row as usize).min(MAX_TXFM_STAGE_NUM))
    {
        *v = opt_range_row;
    }
    for v in col
        .iter_mut()
        .take((cfg.stage_num_col as usize).min(MAX_TXFM_STAGE_NUM))
    {
        *v = opt_range_col;
    }
    (col, row)
}

mod dct16_32;
pub use dct16_32::*;

mod dct64;
pub use dct64::*;

mod adst;
pub use adst::*;
