//! Archmage SIMD (AVX2 `v3`) fast paths for the hot square DCT-DCT 2D
//! transforms, **byte-exact** with the scalar reference.
//!
//! The AV1 integer transforms are fixed-point butterfly networks: every stage
//! is `add` / `sub` / `half_btf(w0,in0,w1,in1,bit)` / `clamp` with a *defined*
//! rounding-shift order. They vectorize naturally across the N independent
//! columns (or rows) of a block — lane `l` carries column (or row) `base + l`,
//! `i32x8` doing 8 at once — with NO cross-lane arithmetic inside a pass. So a
//! SIMD port that performs the SAME multiplies and the SAME `round_shift` in the
//! SAME stage order is **bit-identical** to the scalar kernel.
//!
//! ## Why `_mm256_mullo_epi32` (32-bit) reproduces the scalar's i64 `half_btf`
//!
//! The scalar [`crate::fwd_txfm::half_btf`] widens to i64
//! (`w0 as i64 * in0 as i64 + w1 as i64 * in1 as i64`) then `>> bit`. This code
//! uses `_mm256_mullo_epi32` (low 32 bits) + `_mm256_sra_epi32` — exactly the
//! technique SVT-AV1's own production AVX2 kernels use
//! (`half_btf_avx2`, ASM_AVX2/highbd_{fwd,inv}_txfm_avx2.c), which are
//! bit-identical to the C reference across the whole conformance/fuzz suite.
//! It is exact **iff** every intermediate (`w0·in0`, `w1·in1`, their sum,
//! `+ round`) stays within `i32`: then the wrapping 32-bit ops equal the true
//! i64 value and the arithmetic `>> bit` matches. That range invariant is a
//! designed property of the forward cos-bit choices and the inverse stage
//! clamps; it holds for the whole supported bd8/bd10 envelope (this module gates
//! itself to `bd <= 10`). The `c_parity_txfm` differential proves SIMD == the
//! exported real C **and** SIMD == the scalar port over randomized + edge inputs
//! for every size, under every archmage dispatch tier — so a range violation
//! would fail the build, not ship a wrong pixel.
//!
//! Only the AVX2 (`v3`) arm is vectorized; the `neon`/`scalar` arms report
//! "not handled" and the caller falls through to the scalar core (the CDEF /
//! `txb_init_levels` pattern). Additive — no scalar path is modified.

#![allow(clippy::too_many_arguments)]

use crate::fwd_txfm::{
    COS_BIT, FWD_COS_BIT_COL, FWD_COS_BIT_ROW, NEW_SQRT2, SINPI, cospi_arr, fwd_txfm_shift,
    sinpi_arr,
};
use crate::inv_txfm::{NEW_INV_SQRT2, inv_txfm_shift};
use archmage::prelude::*;
use svtav1_types::transform::TranLow;

/// Sizes the square DCT-DCT SIMD path supports (multiples of 8; 4x4 stays
/// scalar — smaller than a lane group).
#[inline]
fn simd_square_supported(n: usize) -> bool {
    matches!(n, 8 | 16 | 32 | 64)
}

/// `(w, h)` the rectangular DCT-DCT SIMD path supports: both dims a multiple of
/// 8 and `w != h` (the 4-dim rects stay scalar). `bd` gates the inverse only.
#[inline]
fn simd_rect_supported(w: usize, h: usize) -> bool {
    w != h && simd_square_supported(w) && simd_square_supported(h)
}

/// `(w, h, col_1d, row_1d)` the ADST-containing SIMD path supports: both dims in
/// {8, 16} (the only sizes AV1 allows ADST for, both dims <= 16), each 1D type
/// DCT(0) or ADST(1), with at least one ADST (pure DCT-DCT is handled by the
/// square/rect DCT paths). FLIPADST(2)/IDENTITY(3) stay scalar.
#[inline]
fn simd_adst_supported(w: usize, h: usize, col_1d: u8, row_1d: u8) -> bool {
    col_1d <= 1
        && row_1d <= 1
        && (col_1d == 1 || row_1d == 1)
        && matches!(w, 8 | 16)
        && matches!(h, 8 | 16)
}

/// `(w, h, col_1d, row_1d)` the "extended" SIMD path (FLIPADST / IDENTITY /
/// mixed V_/H_) supports. At least one axis must be FLIPADST(2) or IDENTITY(3)
/// — pure DCT/ADST is handled by the DCT / ADST paths. IDTX (both identity)
/// covers every mult-of-8 size up to a 32 dim (its legal AV1 tx-set envelope,
/// via the reused square/rect drivers); the mixed / FLIPADST types are only
/// legal (and only kernel-available) at 8x8/16x16/8x16/16x8. `bd` gates the
/// inverse only.
#[inline]
fn simd_ext_supported(w: usize, h: usize, col_1d: u8, row_1d: u8) -> bool {
    if col_1d < 2 && row_1d < 2 {
        return false;
    }
    if col_1d == 3 && row_1d == 3 {
        return matches!(
            (w, h),
            (8, 8)
                | (16, 16)
                | (32, 32)
                | (8, 16)
                | (16, 8)
                | (16, 32)
                | (32, 16)
                | (8, 32)
                | (32, 8)
        );
    }
    matches!((w, h), (8, 8) | (16, 16) | (8, 16) | (16, 8))
}

/// `(w, h)` the 4-dim SIMD path supports: the five sizes with a 4 dim (4x4,
/// 4x8, 8x4, 4x16, 16x4). All 16 tx types are legal there (max dim <= 16), so
/// the driver handles every (col_1d, row_1d). `bd` gates the inverse only.
#[inline]
fn simd_4dim_supported(w: usize, h: usize) -> bool {
    matches!((w, h), (4, 4) | (4, 8) | (8, 4) | (4, 16) | (16, 4))
}

/// Try the SIMD forward square DCT-DCT (`w == h == n`, no flips). Returns true
/// only when the AVX2 tier actually handled it; false (scalar/neon tiers, or
/// unsupported `n`) tells the caller to run the scalar core.
pub fn try_fwd_dct_square(
    input: &[TranLow],
    output: &mut [TranLow],
    input_stride: usize,
    n: usize,
) -> bool {
    if !simd_square_supported(n) {
        return false;
    }
    incant!(
        try_fwd_dct_square_impl(input, output, input_stride, n),
        [v3, neon, scalar]
    )
}

/// Try the SIMD inverse square DCT-DCT (`w == h == n`, no flips, `bd <= 10`).
/// Same return contract as [`try_fwd_dct_square`].
pub fn try_inv_dct_square(
    input: &[TranLow],
    input_stride: usize,
    output: &mut [TranLow],
    out_stride: usize,
    n: usize,
    bd: u8,
) -> bool {
    if !simd_square_supported(n) || bd > 10 {
        return false;
    }
    incant!(
        try_inv_dct_square_impl(input, input_stride, output, out_stride, n, bd),
        [v3, neon, scalar]
    )
}

/// Try the SIMD forward rectangular DCT-DCT (`w != h`, no flips). Same return
/// contract as [`try_fwd_dct_square`].
pub fn try_fwd_dct_rect(
    input: &[TranLow],
    output: &mut [TranLow],
    input_stride: usize,
    w: usize,
    h: usize,
) -> bool {
    if !simd_rect_supported(w, h) {
        return false;
    }
    incant!(
        try_fwd_dct_rect_impl(input, output, input_stride, w, h),
        [v3, neon, scalar]
    )
}

/// Try the SIMD inverse rectangular DCT-DCT (`w != h`, no flips, `bd <= 10`).
/// For 64-dim sizes the caller must pass the zero-extended `w x h` mod_input at
/// `input_stride` (exactly the scalar `inv_txfm2d_c_exact_bd` contract).
pub fn try_inv_dct_rect(
    input: &[TranLow],
    input_stride: usize,
    output: &mut [TranLow],
    out_stride: usize,
    w: usize,
    h: usize,
    bd: u8,
) -> bool {
    if !simd_rect_supported(w, h) || bd > 10 {
        return false;
    }
    incant!(
        try_inv_dct_rect_impl(input, input_stride, output, out_stride, w, h, bd),
        [v3, neon, scalar]
    )
}

/// Try the SIMD forward ADST-containing 2D transform (ADST_DCT / DCT_ADST /
/// ADST_ADST, no flips). Same return contract as [`try_fwd_dct_square`].
pub fn try_fwd_adst(
    input: &[TranLow],
    output: &mut [TranLow],
    input_stride: usize,
    w: usize,
    h: usize,
    col_1d: u8,
    row_1d: u8,
) -> bool {
    if !simd_adst_supported(w, h, col_1d, row_1d) {
        return false;
    }
    incant!(
        try_fwd_adst_impl(input, output, input_stride, w, h, col_1d, row_1d),
        [v3, neon, scalar]
    )
}

/// Try the SIMD inverse ADST-containing 2D transform (no flips, `bd <= 10`).
pub fn try_inv_adst(
    input: &[TranLow],
    input_stride: usize,
    output: &mut [TranLow],
    out_stride: usize,
    w: usize,
    h: usize,
    col_1d: u8,
    row_1d: u8,
    bd: u8,
) -> bool {
    if !simd_adst_supported(w, h, col_1d, row_1d) || bd > 10 {
        return false;
    }
    incant!(
        try_inv_adst_impl(
            input,
            input_stride,
            output,
            out_stride,
            w,
            h,
            col_1d,
            row_1d,
            bd
        ),
        [v3, neon, scalar]
    )
}

/// Try the SIMD forward "extended" 2D transform — FLIPADST (all combos),
/// IDENTITY (IDTX), and the mixed V_/H_ types. `col_1d`/`row_1d` ∈ {0=DCT,
/// 1=ADST, 2=FLIPADST, 3=IDENTITY}; `ud`/`lr` are the FLIPADST block edge flips
/// (`ud == col_1d==2`, `lr == row_1d==2`). Same return contract as
/// [`try_fwd_dct_square`].
#[allow(clippy::too_many_arguments)]
pub fn try_fwd_ext(
    input: &[TranLow],
    output: &mut [TranLow],
    input_stride: usize,
    w: usize,
    h: usize,
    col_1d: u8,
    row_1d: u8,
    ud: bool,
    lr: bool,
) -> bool {
    if !simd_ext_supported(w, h, col_1d, row_1d) {
        return false;
    }
    incant!(
        try_fwd_ext_impl(input, output, input_stride, w, h, col_1d, row_1d, ud, lr),
        [v3, neon, scalar]
    )
}

/// Try the SIMD inverse "extended" 2D transform (FLIPADST / IDENTITY / mixed
/// V_/H_), `bd <= 10`. Same contract as [`try_fwd_ext`].
#[allow(clippy::too_many_arguments)]
pub fn try_inv_ext(
    input: &[TranLow],
    input_stride: usize,
    output: &mut [TranLow],
    out_stride: usize,
    w: usize,
    h: usize,
    col_1d: u8,
    row_1d: u8,
    ud: bool,
    lr: bool,
    bd: u8,
) -> bool {
    if !simd_ext_supported(w, h, col_1d, row_1d) || bd > 10 {
        return false;
    }
    incant!(
        try_inv_ext_impl(
            input,
            input_stride,
            output,
            out_stride,
            w,
            h,
            col_1d,
            row_1d,
            ud,
            lr,
            bd
        ),
        [v3, neon, scalar]
    )
}

/// Try the SIMD forward 4-dim 2D transform (4x4 / 4x8 / 8x4 / 4x16 / 16x4, any
/// tx type). `col_1d`/`row_1d` ∈ {0=DCT, 1=ADST, 2=FLIPADST, 3=IDENTITY};
/// `ud`/`lr` are the FLIPADST edge flips. Same contract as [`try_fwd_ext`].
#[allow(clippy::too_many_arguments)]
pub fn try_fwd_4dim(
    input: &[TranLow],
    output: &mut [TranLow],
    input_stride: usize,
    w: usize,
    h: usize,
    col_1d: u8,
    row_1d: u8,
    ud: bool,
    lr: bool,
) -> bool {
    if !simd_4dim_supported(w, h) {
        return false;
    }
    incant!(
        try_fwd_4dim_impl(input, output, input_stride, w, h, col_1d, row_1d, ud, lr),
        [v3, neon, scalar]
    )
}

/// Try the SIMD inverse 4-dim 2D transform (any tx type), `bd <= 10`. Same
/// contract as [`try_fwd_4dim`].
#[allow(clippy::too_many_arguments)]
pub fn try_inv_4dim(
    input: &[TranLow],
    input_stride: usize,
    output: &mut [TranLow],
    out_stride: usize,
    w: usize,
    h: usize,
    col_1d: u8,
    row_1d: u8,
    ud: bool,
    lr: bool,
    bd: u8,
) -> bool {
    if !simd_4dim_supported(w, h) || bd > 10 {
        return false;
    }
    incant!(
        try_inv_4dim_impl(
            input,
            input_stride,
            output,
            out_stride,
            w,
            h,
            col_1d,
            row_1d,
            ud,
            lr,
            bd
        ),
        [v3, neon, scalar]
    )
}

// -- scalar / neon arms: not handled, caller runs the scalar core --

fn try_fwd_dct_square_impl_scalar(
    _t: ScalarToken,
    _input: &[TranLow],
    _output: &mut [TranLow],
    _input_stride: usize,
    _n: usize,
) -> bool {
    false
}

fn try_inv_dct_square_impl_scalar(
    _t: ScalarToken,
    _input: &[TranLow],
    _input_stride: usize,
    _output: &mut [TranLow],
    _out_stride: usize,
    _n: usize,
    _bd: u8,
) -> bool {
    false
}

fn try_fwd_dct_rect_impl_scalar(
    _t: ScalarToken,
    _input: &[TranLow],
    _output: &mut [TranLow],
    _input_stride: usize,
    _w: usize,
    _h: usize,
) -> bool {
    false
}

fn try_inv_dct_rect_impl_scalar(
    _t: ScalarToken,
    _input: &[TranLow],
    _input_stride: usize,
    _output: &mut [TranLow],
    _out_stride: usize,
    _w: usize,
    _h: usize,
    _bd: u8,
) -> bool {
    false
}

#[cfg(target_arch = "aarch64")]
#[arcane]
fn try_fwd_dct_rect_impl_neon(
    t: NeonToken,
    input: &[TranLow],
    output: &mut [TranLow],
    input_stride: usize,
    w: usize,
    h: usize,
) -> bool {
    if w.max(h) > NEON_FWD_MAX_DIM {
        return false;
    }
    neon::fwd_dct_rect(t, input, output, input_stride, w, h)
}

#[cfg(target_arch = "aarch64")]
#[arcane]
fn try_inv_dct_rect_impl_neon(
    t: NeonToken,
    input: &[TranLow],
    input_stride: usize,
    output: &mut [TranLow],
    out_stride: usize,
    w: usize,
    h: usize,
    bd: u8,
) -> bool {
    if w.max(h) > NEON_INV_MAX_DIM {
        return false;
    }
    neon::inv_dct_rect(t, input, input_stride, output, out_stride, w, h, bd)
}

fn try_fwd_adst_impl_scalar(
    _t: ScalarToken,
    _input: &[TranLow],
    _output: &mut [TranLow],
    _input_stride: usize,
    _w: usize,
    _h: usize,
    _col_1d: u8,
    _row_1d: u8,
) -> bool {
    false
}

fn try_inv_adst_impl_scalar(
    _t: ScalarToken,
    _input: &[TranLow],
    _input_stride: usize,
    _output: &mut [TranLow],
    _out_stride: usize,
    _w: usize,
    _h: usize,
    _col_1d: u8,
    _row_1d: u8,
    _bd: u8,
) -> bool {
    false
}

#[allow(clippy::too_many_arguments)]
fn try_fwd_ext_impl_scalar(
    _t: ScalarToken,
    _input: &[TranLow],
    _output: &mut [TranLow],
    _input_stride: usize,
    _w: usize,
    _h: usize,
    _col_1d: u8,
    _row_1d: u8,
    _ud: bool,
    _lr: bool,
) -> bool {
    false
}

#[allow(clippy::too_many_arguments)]
fn try_inv_ext_impl_scalar(
    _t: ScalarToken,
    _input: &[TranLow],
    _input_stride: usize,
    _output: &mut [TranLow],
    _out_stride: usize,
    _w: usize,
    _h: usize,
    _col_1d: u8,
    _row_1d: u8,
    _ud: bool,
    _lr: bool,
    _bd: u8,
) -> bool {
    false
}

#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn try_fwd_ext_impl_neon(
    t: NeonToken,
    input: &[TranLow],
    output: &mut [TranLow],
    input_stride: usize,
    w: usize,
    h: usize,
    col_1d: u8,
    row_1d: u8,
    ud: bool,
    lr: bool,
) -> bool {
    if w.max(h) > NEON_FWD_MAX_DIM {
        return false;
    }
    neon::fwd_ext(t, input, output, input_stride, w, h, col_1d, row_1d, ud, lr)
}

#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn try_inv_ext_impl_neon(
    t: NeonToken,
    input: &[TranLow],
    input_stride: usize,
    output: &mut [TranLow],
    out_stride: usize,
    w: usize,
    h: usize,
    col_1d: u8,
    row_1d: u8,
    ud: bool,
    lr: bool,
    bd: u8,
) -> bool {
    if w.max(h) > NEON_INV_MAX_DIM {
        return false;
    }
    neon::inv_ext(
        t,
        input,
        input_stride,
        output,
        out_stride,
        w,
        h,
        col_1d,
        row_1d,
        ud,
        lr,
        bd,
    )
}

#[allow(clippy::too_many_arguments)]
fn try_fwd_4dim_impl_scalar(
    _t: ScalarToken,
    _input: &[TranLow],
    _output: &mut [TranLow],
    _input_stride: usize,
    _w: usize,
    _h: usize,
    _col_1d: u8,
    _row_1d: u8,
    _ud: bool,
    _lr: bool,
) -> bool {
    false
}

#[allow(clippy::too_many_arguments)]
fn try_inv_4dim_impl_scalar(
    _t: ScalarToken,
    _input: &[TranLow],
    _input_stride: usize,
    _output: &mut [TranLow],
    _out_stride: usize,
    _w: usize,
    _h: usize,
    _col_1d: u8,
    _row_1d: u8,
    _ud: bool,
    _lr: bool,
    _bd: u8,
) -> bool {
    false
}

#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn try_fwd_4dim_impl_neon(
    t: NeonToken,
    input: &[TranLow],
    output: &mut [TranLow],
    input_stride: usize,
    w: usize,
    h: usize,
    col_1d: u8,
    row_1d: u8,
    ud: bool,
    lr: bool,
) -> bool {
    if w.max(h) > NEON_FWD_MAX_DIM {
        return false;
    }
    neon::fwd_4dim(t, input, output, input_stride, w, h, col_1d, row_1d, ud, lr)
}

#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn try_inv_4dim_impl_neon(
    t: NeonToken,
    input: &[TranLow],
    input_stride: usize,
    output: &mut [TranLow],
    out_stride: usize,
    w: usize,
    h: usize,
    col_1d: u8,
    row_1d: u8,
    ud: bool,
    lr: bool,
    bd: u8,
) -> bool {
    if w.max(h) > NEON_INV_MAX_DIM {
        return false;
    }
    neon::inv_4dim(
        t,
        input,
        input_stride,
        output,
        out_stride,
        w,
        h,
        col_1d,
        row_1d,
        ud,
        lr,
        bd,
    )
}

#[cfg(target_arch = "aarch64")]
#[arcane]
fn try_fwd_adst_impl_neon(
    t: NeonToken,
    input: &[TranLow],
    output: &mut [TranLow],
    input_stride: usize,
    w: usize,
    h: usize,
    col_1d: u8,
    row_1d: u8,
) -> bool {
    if w.max(h) > NEON_FWD_MAX_DIM {
        return false;
    }
    neon::fwd_adst(t, input, output, input_stride, w, h, col_1d, row_1d)
}

#[cfg(target_arch = "aarch64")]
#[arcane]
fn try_inv_adst_impl_neon(
    t: NeonToken,
    input: &[TranLow],
    input_stride: usize,
    output: &mut [TranLow],
    out_stride: usize,
    w: usize,
    h: usize,
    col_1d: u8,
    row_1d: u8,
    bd: u8,
) -> bool {
    if w.max(h) > NEON_INV_MAX_DIM {
        return false;
    }
    neon::inv_adst(
        t,
        input,
        input_stride,
        output,
        out_stride,
        w,
        h,
        col_1d,
        row_1d,
        bd,
    )
}

#[cfg(target_arch = "aarch64")]
#[arcane]
fn try_fwd_dct_square_impl_neon(
    t: NeonToken,
    input: &[TranLow],
    output: &mut [TranLow],
    input_stride: usize,
    n: usize,
) -> bool {
    if n > NEON_FWD_MAX_DIM {
        return false;
    }
    neon::fwd_dct_square(t, input, output, input_stride, n)
}

#[cfg(target_arch = "aarch64")]
#[arcane]
fn try_inv_dct_square_impl_neon(
    t: NeonToken,
    input: &[TranLow],
    input_stride: usize,
    output: &mut [TranLow],
    out_stride: usize,
    n: usize,
    bd: u8,
) -> bool {
    if n > NEON_INV_MAX_DIM {
        return false;
    }
    neon::inv_dct_square(t, input, input_stride, output, out_stride, n, bd)
}

// ============================================================================
// AVX2 (v3) implementation
// ============================================================================

/// Largest dimension at which the aarch64 tier is measured to BEAT the scalar
/// transform, per direction. 64 is the largest AV1 transform dimension, so as
/// of the real-NEON primitives (2026-08-07) these guards never fire — they are
/// kept because they are the mechanism that would confine a future regression
/// to the sizes it affects, and because the numbers below have to be re-taken
/// if the primitives change again.
///
/// HISTORY, and it is the point of the guard. When `mod neon` carried its
/// 8-lane vector as `[i32; 8]` and relied on LLVM autovectorisation, the tier
/// was not a uniform win — the advantage shrank with size and went NEGATIVE,
/// which is why the caps were 16 (fwd) and 8 (inv):
///
/// | shape | fwd (autovec)  | inv (autovec)  |
/// |-------|----------------|----------------|
/// | 8x8   | 224 -> 109 ns  | 373 -> 322 ns  |
/// | 16x16 | 588 -> 547 ns  | 1.2 -> 1.7 us  |
/// | 32x32 | 2.6 -> 2.7 us  | 5.1 -> 8.6 us  |
///
/// With the primitives rewritten as real `[int32x4_t; 2]` intrinsics the tier
/// wins at EVERY dimension, by 3x-10x (benches/kernel_tiers.rs, zenbench
/// interleaved arms, neon vs forced scalar, Apple M4 Pro):
///
/// | shape | fwd neon | fwd scalar | inv neon | inv scalar |
/// |-------|----------|------------|----------|------------|
/// | 8x8   |  22.4 ns |   218.5 ns |  38.4 ns |   376.4 ns |
/// | 16x16 |   191 ns |     593 ns |   253 ns |    1211 ns |
/// | 32x32 |   758 ns |    2546 ns |  1.13 us |    5.10 us |
/// | 64x64 |  3.68 us |   12.10 us |  5.63 us |   25.40 us |
///
/// Non-square/ext/adst/4dim shapes run the SAME inner kernels, so they are
/// bounded by `max(w, h)` against these same limits — interpolation inside the
/// measured range, not extrapolation past it.
#[cfg(target_arch = "aarch64")]
const NEON_FWD_MAX_DIM: usize = 64;
#[cfg(target_arch = "aarch64")]
const NEON_INV_MAX_DIM: usize = 64;

/// aarch64 tier for the shared transform kernels.
///
/// The 3,000 lines of transform algorithm in the `include!`d files are written
/// against an 8-lane vector plus a small primitive set. Rather than fork them
/// (or refactor the x86 module, which cannot be tested on this host), this
/// module supplies the SAME NAMES the shared files already use — the vector
/// type, the token type, and the handful of intrinsics they call directly — so
/// the identical source compiles here untouched.
///
/// The vector type is `[int32x4_t; 2]` — a pair of NEON registers standing in
/// for one 8-lane x86 vector, the same carrying convention the CDEF and SATD
/// ports in this crate use. It was previously `[i32; 8]` in plain safe Rust on
/// the theory that LLVM would autovectorise 8-lane-shaped code since NEON is
/// baseline on aarch64. **Measured, that theory failed above the smallest
/// sizes**: the autovectorised inverse 16x16 came out at 1.7 us against
/// scalar's 1.2, and 32x32 at 8.6 us against 5.1, which is why
/// [`NEON_INV_MAX_DIM`] was 8 and the port ran scalar transforms for every
/// larger block. The real intrinsics win at every dimension by 3x-10x — see
/// the table on [`NEON_FWD_MAX_DIM`]. Do not re-attempt the autovectorisation
/// route.
///
/// Everything is still safe Rust (`#![forbid(unsafe_code)]` holds): the
/// intrinsics come through archmage's safe wrappers, gated by a `#[rite(neon)]`
/// token region, and take fixed-size array references rather than raw pointers.
///
/// Every function here is a transcription of the x86 one's DOCUMENTED
/// semantics, not of its instruction sequence — see `mod v3` for the derivations
/// (particularly `rect_scale`, which needs a true i64 product, and the note on
/// why 32-bit `mullo` reproduces the scalar i64 result in `hbtf`).
#[cfg(target_arch = "aarch64")]
#[allow(non_camel_case_types, non_snake_case, dead_code)]
#[allow(clippy::identity_op, clippy::needless_range_loop)]
mod neon {
    use super::*;

    /// The shared files' token type. Aliased so their `#[rite]` signatures and
    /// `Desktop64` mentions resolve here.
    pub(super) type Desktop64 = NeonToken;
    /// The shared files' 8-lane vector, carried as a pair of NEON i32x4s.
    pub(super) type __m256i = [int32x4_t; 2];
    /// The shared files' runtime shift-count type.
    pub(super) type __m128i = i32;

    // ----- shims for the intrinsics the shared files call directly -----

    #[rite(neon)]
    pub(super) fn _mm256_setzero_si256() -> __m256i {
        [vdupq_n_s32(0); 2]
    }
    #[rite(neon)]
    pub(super) fn _mm256_add_epi32(a: __m256i, b: __m256i) -> __m256i {
        [vaddq_s32(a[0], b[0]), vaddq_s32(a[1], b[1])]
    }
    #[rite(neon)]
    pub(super) fn _mm256_sub_epi32(a: __m256i, b: __m256i) -> __m256i {
        [vsubq_s32(a[0], b[0]), vsubq_s32(a[1], b[1])]
    }
    /// Truncating 32x32 low product, exactly like `_mm256_mullo_epi32`
    /// (`vmulq_s32` keeps the low 32 bits, i.e. wraps, which is what the x86
    /// arm's doc relies on).
    #[rite(neon)]
    pub(super) fn _mm256_mullo_epi32(a: __m256i, b: __m256i) -> __m256i {
        [vmulq_s32(a[0], b[0]), vmulq_s32(a[1], b[1])]
    }
    /// Arithmetic shift right by a runtime count, like `_mm256_sra_epi32`.
    /// NEON has no variable right shift; a NEGATIVE count on `vshlq_s32` is
    /// the arithmetic right shift.
    #[rite(neon)]
    pub(super) fn _mm256_sra_epi32(a: __m256i, sh: __m128i) -> __m256i {
        let n = vdupq_n_s32(-sh);
        [vshlq_s32(a[0], n), vshlq_s32(a[1], n)]
    }
    /// Logical shift left by a constant, like `_mm256_slli_epi32::<N>`.
    #[rite(neon)]
    pub(super) fn _mm256_slli_epi32<const N: i32>(a: __m256i) -> __m256i {
        let n = vdupq_n_s32(N);
        [vshlq_s32(a[0], n), vshlq_s32(a[1], n)]
    }
    /// Package a runtime shift count, like `_mm_cvtsi32_si128`.
    #[rite(neon)]
    pub(super) fn _mm_cvtsi32_si128(v: i32) -> __m128i {
        v
    }
    // ----- the primitive set (mirrors `mod v3`) -----

    #[rite(neon)]
    pub(super) fn splat(_t: Desktop64, v: i32) -> __m256i {
        [vdupq_n_s32(v); 2]
    }

    /// `((w0·n0 + w1·n1) + round) >> bit`. Truncating 32-bit products, matching
    /// the x86 arm's `mullo` (see its doc for why that equals the scalar i64
    /// result over the reachable coefficient range).
    #[rite(neon)]
    pub(super) fn hbtf(
        _t: Desktop64,
        w0: __m256i,
        n0: __m256i,
        w1: __m256i,
        n1: __m256i,
        rnd: __m256i,
        sh: __m128i,
    ) -> __m256i {
        let n = vdupq_n_s32(-sh);
        let lo = vmlaq_s32(vmulq_s32(w0[0], n0[0]), w1[0], n1[0]);
        let hi = vmlaq_s32(vmulq_s32(w0[1], n0[1]), w1[1], n1[1]);
        [
            vshlq_s32(vaddq_s32(lo, rnd[0]), n),
            vshlq_s32(vaddq_s32(hi, rnd[1]), n),
        ]
    }

    #[rite(neon)]
    pub(super) fn clampv(_t: Desktop64, v: __m256i, lo: __m256i, hi: __m256i) -> __m256i {
        [
            vmaxq_s32(vminq_s32(v[0], hi[0]), lo[0]),
            vmaxq_s32(vminq_s32(v[1], hi[1]), lo[1]),
        ]
    }

    #[rite(neon)]
    pub(super) fn round_shift_v(_t: Desktop64, v: __m256i, bit: i32) -> __m256i {
        if bit > 0 {
            let rnd = vdupq_n_s32(1 << (bit as u32 - 1));
            let n = vdupq_n_s32(-bit);
            [
                vshlq_s32(vaddq_s32(v[0], rnd), n),
                vshlq_s32(vaddq_s32(v[1], rnd), n),
            ]
        } else if bit < 0 {
            let n = vdupq_n_s32(-bit);
            [vshlq_s32(v[0], n), vshlq_s32(v[1], n)]
        } else {
            v
        }
    }

    #[rite(neon)]
    pub(super) fn wraplow(_t: Desktop64, v: __m256i, lo: __m256i, hi: __m256i) -> __m256i {
        [
            vmaxq_s32(vminq_s32(v[0], hi[0]), lo[0]),
            vmaxq_s32(vminq_s32(v[1], hi[1]), lo[1]),
        ]
    }

    /// `(v*k + (1<<11)) >> 12` with a TRUE i64 product — the scalar widens, so a
    /// 32-bit product would overflow for large coefficients. Simpler here than
    /// on AVX2, which lacks a signed 64-bit arithmetic shift and has to split
    /// even/odd lanes: `vmull_s32` / `vmull_high_s32` give the widened products
    /// directly and `vrshrn_n_s64::<12>` does the +(1<<11) rounding AND the
    /// narrowing in one instruction.
    #[rite(neon)]
    pub(super) fn rect_scale(_t: Desktop64, v: __m256i, k: i32) -> __m256i {
        let kk = vdup_n_s32(k);
        let kq = vdupq_n_s32(k);
        let f = |x: int32x4_t| -> int32x4_t {
            let p0 = vmull_s32(vget_low_s32(x), kk);
            let p1 = vmull_high_s32(x, kq);
            vcombine_s32(vrshrn_n_s64::<12>(p0), vrshrn_n_s64::<12>(p1))
        };
        [f(v[0]), f(v[1])]
    }

    /// 4x4 i32 transpose — the building block of [`transpose8`].
    #[rite(neon)]
    fn transpose4(a: [int32x4_t; 4]) -> [int32x4_t; 4] {
        let t0 = vtrn1q_s32(a[0], a[1]);
        let t1 = vtrn2q_s32(a[0], a[1]);
        let t2 = vtrn1q_s32(a[2], a[3]);
        let t3 = vtrn2q_s32(a[2], a[3]);
        [
            vcombine_s32(vget_low_s32(t0), vget_low_s32(t2)),
            vcombine_s32(vget_low_s32(t1), vget_low_s32(t3)),
            vcombine_s32(vget_high_s32(t0), vget_high_s32(t2)),
            vcombine_s32(vget_high_s32(t1), vget_high_s32(t3)),
        ]
    }

    /// 8x8 transpose of eight 8-lane vectors: four 4x4 transposes plus the
    /// off-diagonal quadrant swap (`out[r][0]` takes rows 0..4's low half,
    /// `out[r][1]` takes rows 4..8's low half, and symmetrically for the
    /// high halves).
    #[rite(neon)]
    pub(super) fn transpose8(_t: Desktop64, inp: &[__m256i; 8]) -> [__m256i; 8] {
        let a = transpose4([inp[0][0], inp[1][0], inp[2][0], inp[3][0]]); // rows 0-3, cols 0-3
        let b = transpose4([inp[4][0], inp[5][0], inp[6][0], inp[7][0]]); // rows 4-7, cols 0-3
        let c = transpose4([inp[0][1], inp[1][1], inp[2][1], inp[3][1]]); // rows 0-3, cols 4-7
        let d = transpose4([inp[4][1], inp[5][1], inp[6][1], inp[7][1]]); // rows 4-7, cols 4-7
        [
            [a[0], b[0]],
            [a[1], b[1]],
            [a[2], b[2]],
            [a[3], b[3]],
            [c[0], d[0]],
            [c[1], d[1]],
            [c[2], d[2]],
            [c[3], d[3]],
        ]
    }

    /// `out.lane(i) = in.lane(7 - i)`. Reversing an `int32x4_t` is
    /// `vrev64q_s32` (swap within each 64-bit half) followed by a swap of the
    /// two halves, which `vextq_s32::<2>` against itself does; the pair's two
    /// registers then trade places.
    #[rite(neon)]
    pub(super) fn perm_rev8(_t: Desktop64, v: __m256i) -> __m256i {
        let r = |x: int32x4_t| -> int32x4_t {
            let s = vrev64q_s32(x);
            vextq_s32::<2>(s, s)
        };
        [r(v[1]), r(v[0])]
    }

    #[rite(neon)]
    pub(super) fn load8(_t: Desktop64, buf: &[i32], off: usize) -> __m256i {
        let lo: &[i32; 4] = buf[off..off + 4].try_into().unwrap();
        let hi: &[i32; 4] = buf[off + 4..off + 8].try_into().unwrap();
        [vld1q_s32(lo), vld1q_s32(hi)]
    }

    #[rite(neon)]
    pub(super) fn store8(_t: Desktop64, buf: &mut [i32], off: usize, v: __m256i) {
        let (lo, hi) = buf[off..off + 8].split_at_mut(4);
        vst1q_s32(lo.try_into().unwrap(), v[0]);
        vst1q_s32(hi.try_into().unwrap(), v[1]);
    }

    include!("txfm_simd_kernels.rs");
    include!("txfm_simd_drivers.rs");
    include!("txfm_simd_rect.rs");
    include!("txfm_simd_adst.rs");
    include!("txfm_simd_ext.rs");
    include!("txfm_simd_4dim.rs");
}

#[cfg(target_arch = "x86_64")]
#[allow(clippy::identity_op, clippy::needless_range_loop)]
mod v3 {
    use super::*;

    // ----- primitives -----

    /// Broadcast an i32 to all 8 lanes.
    #[rite]
    pub(super) fn splat(_t: Desktop64, v: i32) -> __m256i {
        _mm256_set1_epi32(v)
    }

    /// `out.lane(i) = in.lane(7 - i)` — a single `vpermd`.
    /// `set_epi32(e7,..,e0)` puts e7 in lane 7, so this index vector is
    /// `idx[i] = 7 - i`.
    #[rite]
    pub(super) fn perm_rev8(_t: Desktop64, v: __m256i) -> __m256i {
        _mm256_permutevar8x32_epi32(v, _mm256_set_epi32(0, 1, 2, 3, 4, 5, 6, 7))
    }

    /// Vector `half_btf`: `((w0·n0 + w1·n1) + round) >> bit`, arithmetic shift.
    /// `w0`/`w1` are broadcast cospi weights, `rnd` = splat(1<<(bit-1)),
    /// `sh` = the runtime shift count in an `__m128i`. See the module docs for
    /// why the 32-bit `mullo` reproduces the scalar i64 result exactly.
    #[rite]
    pub(super) fn hbtf(
        _t: Desktop64,
        w0: __m256i,
        n0: __m256i,
        w1: __m256i,
        n1: __m256i,
        rnd: __m256i,
        sh: __m128i,
    ) -> __m256i {
        let x = _mm256_mullo_epi32(w0, n0);
        let y = _mm256_mullo_epi32(w1, n1);
        _mm256_sra_epi32(_mm256_add_epi32(_mm256_add_epi32(x, y), rnd), sh)
    }

    /// `clamp_value(v, range)` across 8 lanes: clamp to the signed `range`-bit
    /// interval. `range <= 0` is a no-op (caller passes precomputed lo/hi).
    #[rite]
    pub(super) fn clampv(_t: Desktop64, v: __m256i, lo: __m256i, hi: __m256i) -> __m256i {
        _mm256_max_epi32(_mm256_min_epi32(v, hi), lo)
    }

    /// `round_shift_array` element op: `bit > 0` → `(v + (1<<(bit-1))) >> bit`
    /// (rounded right); `bit < 0` → `v << -bit`; `bit == 0` → identity. Matches
    /// `crate::fwd_txfm::round_shift_array` exactly.
    #[rite]
    pub(super) fn round_shift_v(_t: Desktop64, v: __m256i, bit: i32) -> __m256i {
        if bit > 0 {
            let b = bit as u32;
            let rnd = _mm256_set1_epi32(1 << (b - 1));
            _mm256_sra_epi32(_mm256_add_epi32(v, rnd), _mm_cvtsi32_si128(bit))
        } else if bit < 0 {
            _mm256_sll_epi32(v, _mm_cvtsi32_si128(-bit))
        } else {
            v
        }
    }

    /// `highbd_wraplow(v, bd)` across 8 lanes: clamp to `±((1<<(7+bd))-1 +
    /// (914<<(bd-7)))`. `lo`/`hi` precomputed by the caller.
    #[rite]
    pub(super) fn wraplow(_t: Desktop64, v: __m256i, lo: __m256i, hi: __m256i) -> __m256i {
        _mm256_max_epi32(_mm256_min_epi32(v, hi), lo)
    }

    /// The rectangular `NewSqrt2` / `NewInvSqrt2` scale, byte-exact with the
    /// scalar `round_shift_i64(v as i64 * k as i64, NEW_SQRT2_BITS)` applied to
    /// each of the 8 lanes: `(v*k + (1<<11)) >> 12` with a **64-bit** product
    /// (the scalar widens to i64, so 32-bit `mullo` would overflow for large
    /// coefficients — this must use the true i64 product).
    ///
    /// AVX2 has no signed 64-bit arithmetic shift, so the even (0,2,4,6) and odd
    /// (1,3,5,7) lanes are multiplied 32×32→64 via `_mm256_mul_epi32`, rounded,
    /// then `>> 12` **logically**: the true result fits in i32 (a valid
    /// coefficient), and a logical and arithmetic `>>12` agree in bits 0..51, so
    /// the low 32 bits of the logical shift equal the i32 arithmetic result. The
    /// even results land in i32 lanes 0,2,4,6; the odd results are shifted up
    /// into i32 lanes 1,3,5,7 and blended back to lane order. The `c_parity_txfm`
    /// rect differential proves this byte-exact vs real C over edge inputs.
    #[rite]
    pub(super) fn rect_scale(_t: Desktop64, v: __m256i, k: i32) -> __m256i {
        // Fully-qualified rather than added to the file-level `use` list: this
        // is the ONLY reference to it, and it lives inside the x86_64-gated
        // `v3` module, so an import would be an unused-import warning (and so
        // a clippy failure) on aarch64. CI has been red since aafcc9d47 on
        // exactly this — the aarch64 dev box never compiles this module.
        const BITS: i32 = crate::fwd_txfm::NEW_SQRT2_BITS as i32; // 12
        let ks = _mm256_set1_epi32(k);
        let round = _mm256_set1_epi64x(1i64 << (BITS - 1)); // 2048
        // even lanes 0,2,4,6 → four i64 products in slots [0..4]
        let even = _mm256_add_epi64(_mm256_mul_epi32(v, ks), round);
        // odd lanes 1,3,5,7 → move into the low 32 of each 64-bit slot, then mul
        let vodd = _mm256_srli_epi64::<32>(v);
        let odd = _mm256_add_epi64(_mm256_mul_epi32(vodd, ks), round);
        // >>12 logical; low 32 bits of each slot = the i32 arithmetic result
        let even_s = _mm256_srli_epi64::<BITS>(even); // results in i32 lanes 0,2,4,6
        let odd_s = _mm256_srli_epi64::<BITS>(odd);
        let odd_up = _mm256_slli_epi64::<32>(odd_s); // move to i32 lanes 1,3,5,7
        _mm256_blend_epi32::<0xAA>(even_s, odd_up)
    }

    /// Transpose an 8×8 i32 tile: `out[i]` = column `i` = `[in0[i]..in7[i]]`.
    /// Pure data movement (unpack + `permute2x128`) → bit-exact.
    #[rite]
    pub(super) fn transpose8(t: Desktop64, inp: &[__m256i; 8]) -> [__m256i; 8] {
        let a0 = _mm256_unpacklo_epi32(inp[0], inp[1]);
        let a1 = _mm256_unpackhi_epi32(inp[0], inp[1]);
        let a2 = _mm256_unpacklo_epi32(inp[2], inp[3]);
        let a3 = _mm256_unpackhi_epi32(inp[2], inp[3]);
        let a4 = _mm256_unpacklo_epi32(inp[4], inp[5]);
        let a5 = _mm256_unpackhi_epi32(inp[4], inp[5]);
        let a6 = _mm256_unpacklo_epi32(inp[6], inp[7]);
        let a7 = _mm256_unpackhi_epi32(inp[6], inp[7]);
        let b0 = _mm256_unpacklo_epi64(a0, a2);
        let b1 = _mm256_unpackhi_epi64(a0, a2);
        let b2 = _mm256_unpacklo_epi64(a1, a3);
        let b3 = _mm256_unpackhi_epi64(a1, a3);
        let b4 = _mm256_unpacklo_epi64(a4, a6);
        let b5 = _mm256_unpackhi_epi64(a4, a6);
        let b6 = _mm256_unpacklo_epi64(a5, a7);
        let b7 = _mm256_unpackhi_epi64(a5, a7);
        let _ = t;
        [
            _mm256_permute2x128_si256::<0x20>(b0, b4),
            _mm256_permute2x128_si256::<0x20>(b1, b5),
            _mm256_permute2x128_si256::<0x20>(b2, b6),
            _mm256_permute2x128_si256::<0x20>(b3, b7),
            _mm256_permute2x128_si256::<0x31>(b0, b4),
            _mm256_permute2x128_si256::<0x31>(b1, b5),
            _mm256_permute2x128_si256::<0x31>(b2, b6),
            _mm256_permute2x128_si256::<0x31>(b3, b7),
        ]
    }

    /// Load 8 contiguous i32 at `buf[off..off+8]`.
    #[rite]
    pub(super) fn load8(_t: Desktop64, buf: &[i32], off: usize) -> __m256i {
        let a: &[i32; 8] = buf[off..off + 8].try_into().unwrap();
        _mm256_loadu_si256(a)
    }

    /// Store 8 i32 to `buf[off..off+8]`.
    #[rite]
    pub(super) fn store8(_t: Desktop64, buf: &mut [i32], off: usize, v: __m256i) {
        let a: &mut [i32; 8] = (&mut buf[off..off + 8]).try_into().unwrap();
        _mm256_storeu_si256(a, v);
    }

    include!("txfm_simd_kernels.rs");
    include!("txfm_simd_drivers.rs");
    include!("txfm_simd_rect.rs");
    include!("txfm_simd_adst.rs");
    include!("txfm_simd_ext.rs");
    include!("txfm_simd_4dim.rs");
}

/// AVX2 forward square DCT-DCT. Dispatched only on the `v3` tier.
#[cfg(target_arch = "x86_64")]
#[arcane]
fn try_fwd_dct_square_impl_v3(
    t: Desktop64,
    input: &[TranLow],
    output: &mut [TranLow],
    input_stride: usize,
    n: usize,
) -> bool {
    v3::fwd_dct_square(t, input, output, input_stride, n)
}

/// AVX2 inverse square DCT-DCT. Dispatched only on the `v3` tier.
#[cfg(target_arch = "x86_64")]
#[arcane]
fn try_inv_dct_square_impl_v3(
    t: Desktop64,
    input: &[TranLow],
    input_stride: usize,
    output: &mut [TranLow],
    out_stride: usize,
    n: usize,
    bd: u8,
) -> bool {
    v3::inv_dct_square(t, input, input_stride, output, out_stride, n, bd)
}

/// AVX2 forward rectangular DCT-DCT. Dispatched only on the `v3` tier.
#[cfg(target_arch = "x86_64")]
#[arcane]
fn try_fwd_dct_rect_impl_v3(
    t: Desktop64,
    input: &[TranLow],
    output: &mut [TranLow],
    input_stride: usize,
    w: usize,
    h: usize,
) -> bool {
    v3::fwd_dct_rect(t, input, output, input_stride, w, h)
}

/// AVX2 inverse rectangular DCT-DCT. Dispatched only on the `v3` tier.
#[cfg(target_arch = "x86_64")]
#[arcane]
fn try_inv_dct_rect_impl_v3(
    t: Desktop64,
    input: &[TranLow],
    input_stride: usize,
    output: &mut [TranLow],
    out_stride: usize,
    w: usize,
    h: usize,
    bd: u8,
) -> bool {
    v3::inv_dct_rect(t, input, input_stride, output, out_stride, w, h, bd)
}

/// AVX2 forward ADST-containing 2D transform. Dispatched only on the `v3` tier.
#[cfg(target_arch = "x86_64")]
#[arcane]
fn try_fwd_adst_impl_v3(
    t: Desktop64,
    input: &[TranLow],
    output: &mut [TranLow],
    input_stride: usize,
    w: usize,
    h: usize,
    col_1d: u8,
    row_1d: u8,
) -> bool {
    v3::fwd_adst(t, input, output, input_stride, w, h, col_1d, row_1d)
}

/// AVX2 inverse ADST-containing 2D transform. Dispatched only on the `v3` tier.
#[cfg(target_arch = "x86_64")]
#[arcane]
fn try_inv_adst_impl_v3(
    t: Desktop64,
    input: &[TranLow],
    input_stride: usize,
    output: &mut [TranLow],
    out_stride: usize,
    w: usize,
    h: usize,
    col_1d: u8,
    row_1d: u8,
    bd: u8,
) -> bool {
    v3::inv_adst(
        t,
        input,
        input_stride,
        output,
        out_stride,
        w,
        h,
        col_1d,
        row_1d,
        bd,
    )
}

/// AVX2 forward extended 2D transform (FLIPADST / IDENTITY / V_/H_). `v3` only.
#[cfg(target_arch = "x86_64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn try_fwd_ext_impl_v3(
    t: Desktop64,
    input: &[TranLow],
    output: &mut [TranLow],
    input_stride: usize,
    w: usize,
    h: usize,
    col_1d: u8,
    row_1d: u8,
    ud: bool,
    lr: bool,
) -> bool {
    v3::fwd_ext(t, input, output, input_stride, w, h, col_1d, row_1d, ud, lr)
}

/// AVX2 inverse extended 2D transform (FLIPADST / IDENTITY / V_/H_). `v3` only.
#[cfg(target_arch = "x86_64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn try_inv_ext_impl_v3(
    t: Desktop64,
    input: &[TranLow],
    input_stride: usize,
    output: &mut [TranLow],
    out_stride: usize,
    w: usize,
    h: usize,
    col_1d: u8,
    row_1d: u8,
    ud: bool,
    lr: bool,
    bd: u8,
) -> bool {
    v3::inv_ext(
        t,
        input,
        input_stride,
        output,
        out_stride,
        w,
        h,
        col_1d,
        row_1d,
        ud,
        lr,
        bd,
    )
}

/// AVX2 forward 4-dim 2D transform (4x4 / 4x8 / 8x4 / 4x16 / 16x4). `v3` only.
#[cfg(target_arch = "x86_64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn try_fwd_4dim_impl_v3(
    t: Desktop64,
    input: &[TranLow],
    output: &mut [TranLow],
    input_stride: usize,
    w: usize,
    h: usize,
    col_1d: u8,
    row_1d: u8,
    ud: bool,
    lr: bool,
) -> bool {
    v3::fwd_4dim(t, input, output, input_stride, w, h, col_1d, row_1d, ud, lr)
}

/// AVX2 inverse 4-dim 2D transform. `v3` only.
#[cfg(target_arch = "x86_64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn try_inv_4dim_impl_v3(
    t: Desktop64,
    input: &[TranLow],
    input_stride: usize,
    output: &mut [TranLow],
    out_stride: usize,
    w: usize,
    h: usize,
    col_1d: u8,
    row_1d: u8,
    ud: bool,
    lr: bool,
    bd: u8,
) -> bool {
    v3::inv_4dim(
        t,
        input,
        input_stride,
        output,
        out_stride,
        w,
        h,
        col_1d,
        row_1d,
        ud,
        lr,
        bd,
    )
}
