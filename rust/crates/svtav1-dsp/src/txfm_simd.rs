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
//! BOTH the AVX2 (`v3`) and NEON arms are vectorized — the shared kernels are
//! `include!`d into each and compile against that arm's primitives. The NEON
//! arm is DIMENSION-BOUNDED rather than absent: `NEON_FWD_MAX_DIM` /
//! `NEON_INV_MAX_DIM` (set from the measured table above them) cap which shapes
//! it takes, and larger shapes report "not handled" so the caller falls through
//! to the scalar core (the CDEF / `txb_init_levels` pattern). Non-x86,
//! non-aarch64 targets take the scalar core for everything. Additive — no
//! scalar path is modified.

#![allow(clippy::too_many_arguments)]

use crate::fwd_txfm::{
    COS_BIT, FWD_COS_BIT_COL, FWD_COS_BIT_ROW, NEW_SQRT2, SINPI, cospi_arr, fwd_txfm_shift,
    sinpi_arr,
};
// Used only by `mod v3`'s `rect_scale` (the aarch64 twin has its own). Importing
// it unconditionally warned on every non-x86 build; it is NOT dead code.
#[cfg(target_arch = "x86_64")]
use crate::fwd_txfm::NEW_SQRT2_BITS;
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
    col_1d <= 1 && row_1d <= 1 && (col_1d == 1 || row_1d == 1) && matches!(w, 8 | 16) && matches!(h, 8 | 16)
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
            (8, 8) | (16, 16) | (32, 32) | (8, 16) | (16, 8) | (16, 32) | (32, 16) | (8, 32) | (32, 8)
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
        try_inv_adst_impl(input, input_stride, output, out_stride, w, h, col_1d, row_1d, bd),
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
        try_inv_ext_impl(input, input_stride, output, out_stride, w, h, col_1d, row_1d, ud, lr, bd),
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
        try_inv_4dim_impl(input, input_stride, output, out_stride, w, h, col_1d, row_1d, ud, lr, bd),
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
    neon::inv_ext(t, input, input_stride, output, out_stride, w, h, col_1d, row_1d, ud, lr, bd)
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
    neon::inv_4dim(t, input, input_stride, output, out_stride, w, h, col_1d, row_1d, ud, lr, bd)
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
    neon::inv_adst(t, input, input_stride, output, out_stride, w, h, col_1d, row_1d, bd)
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
/// transform, per direction.
///
/// The shared 8-lane kernels are not a uniform win here — the advantage shrinks
/// with size and goes negative. Measured (benches/kernel_tiers.rs, M-series,
/// neon vs forced scalar, CI excluding zero on every row):
///
/// | shape | fwd            | inv            |
/// |-------|----------------|----------------|
/// | 8x8   | 224 -> 109 ns  | 373 -> 322 ns  |
/// | 16x16 | 588 -> 547 ns  | 1.2 -> 1.7 us  |
/// | 32x32 | 2.6 -> 2.7 us  | 5.1 -> 8.6 us  |
///
/// So forward pays through 16 and inverse only through 8; beyond that the
/// scalar transform is faster and the `try_*` protocol returns false to keep
/// it. Non-square/ext/adst/4dim shapes run the SAME inner kernels, so they are
/// bounded by `max(w, h)` against these same limits — interpolation inside the
/// measured range, not extrapolation past it.
#[cfg(target_arch = "aarch64")]
const NEON_FWD_MAX_DIM: usize = 16;
#[cfg(target_arch = "aarch64")]
const NEON_INV_MAX_DIM: usize = 8;

/// aarch64 tier for the shared transform kernels.
///
/// The 3,000 lines of transform algorithm in the `include!`d files are written
/// against an 8-lane vector plus a small primitive set. Rather than fork them
/// (or refactor the x86 module, which cannot be tested on this host), this
/// module supplies the SAME NAMES the shared files already use — the vector
/// type, the token type, and the handful of intrinsics they call directly — so
/// the identical source compiles here untouched.
///
/// The vector type is `[i32; 8]` in plain safe Rust, NOT hand-written NEON
/// intrinsics. On aarch64 NEON is BASELINE, so LLVM autovectorizes this
/// 8-lane-shaped code; the whole point of the shared files is that the shape is
/// already right. That also keeps the module free of `unsafe` and of any
/// lane-order reasoning, which is where hand-ported transforms go wrong.
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
    /// The shared files' 8-lane vector.
    pub(super) type __m256i = [i32; 8];
    /// The shared files' runtime shift-count type.
    pub(super) type __m128i = i32;

    // ----- shims for the intrinsics the shared files call directly -----

    #[inline(always)]
    pub(super) fn _mm256_setzero_si256() -> __m256i {
        [0; 8]
    }
    #[inline(always)]
    pub(super) fn _mm256_add_epi32(a: __m256i, b: __m256i) -> __m256i {
        core::array::from_fn(|i| a[i].wrapping_add(b[i]))
    }
    #[inline(always)]
    pub(super) fn _mm256_sub_epi32(a: __m256i, b: __m256i) -> __m256i {
        core::array::from_fn(|i| a[i].wrapping_sub(b[i]))
    }
    /// Truncating 32x32 low product, exactly like `_mm256_mullo_epi32`.
    #[inline(always)]
    pub(super) fn _mm256_mullo_epi32(a: __m256i, b: __m256i) -> __m256i {
        core::array::from_fn(|i| a[i].wrapping_mul(b[i]))
    }
    /// Arithmetic shift right by a runtime count, like `_mm256_sra_epi32`.
    #[inline(always)]
    pub(super) fn _mm256_sra_epi32(a: __m256i, sh: __m128i) -> __m256i {
        core::array::from_fn(|i| a[i] >> sh)
    }
    /// Logical shift left by a constant, like `_mm256_slli_epi32::<N>`.
    #[inline(always)]
    pub(super) fn _mm256_slli_epi32<const N: i32>(a: __m256i) -> __m256i {
        core::array::from_fn(|i| ((a[i] as u32) << N) as i32)
    }
    /// Package a runtime shift count, like `_mm_cvtsi32_si128`.
    #[inline(always)]
    pub(super) fn _mm_cvtsi32_si128(v: i32) -> __m128i {
        v
    }
    /// NOTE the reversed argument order: x86's `set` takes lane 7 FIRST.
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn _mm256_set_epi32(
        e7: i32, e6: i32, e5: i32, e4: i32, e3: i32, e2: i32, e1: i32, e0: i32,
    ) -> __m256i {
        [e0, e1, e2, e3, e4, e5, e6, e7]
    }
    /// Lane gather by index, like `_mm256_permutevar8x32_epi32`.
    #[inline(always)]
    pub(super) fn _mm256_permutevar8x32_epi32(v: __m256i, idx: __m256i) -> __m256i {
        core::array::from_fn(|i| v[(idx[i] & 7) as usize])
    }

    // ----- the primitive set (mirrors `mod v3`) -----

    #[rite(neon)]
    pub(super) fn splat(_t: Desktop64, v: i32) -> __m256i {
        [v; 8]
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
        core::array::from_fn(|i| {
            let x = w0[i].wrapping_mul(n0[i]);
            let y = w1[i].wrapping_mul(n1[i]);
            x.wrapping_add(y).wrapping_add(rnd[i]) >> sh
        })
    }

    #[rite(neon)]
    pub(super) fn clampv(_t: Desktop64, v: __m256i, lo: __m256i, hi: __m256i) -> __m256i {
        core::array::from_fn(|i| v[i].min(hi[i]).max(lo[i]))
    }

    #[rite(neon)]
    pub(super) fn round_shift_v(_t: Desktop64, v: __m256i, bit: i32) -> __m256i {
        if bit > 0 {
            let rnd = 1i32 << (bit as u32 - 1);
            core::array::from_fn(|i| (v[i].wrapping_add(rnd)) >> bit)
        } else if bit < 0 {
            core::array::from_fn(|i| ((v[i] as u32) << (-bit) as u32) as i32)
        } else {
            v
        }
    }

    #[rite(neon)]
    pub(super) fn wraplow(_t: Desktop64, v: __m256i, lo: __m256i, hi: __m256i) -> __m256i {
        core::array::from_fn(|i| v[i].min(hi[i]).max(lo[i]))
    }

    /// `(v*k + (1<<11)) >> 12` with a TRUE i64 product — the scalar widens, so a
    /// 32-bit product would overflow for large coefficients. Simpler here than
    /// on AVX2, which lacks a signed 64-bit arithmetic shift and has to split
    /// even/odd lanes.
    #[rite(neon)]
    pub(super) fn rect_scale(_t: Desktop64, v: __m256i, k: i32) -> __m256i {
        core::array::from_fn(|i| {
            let p = v[i] as i64 * k as i64;
            ((p + (1i64 << 11)) >> 12) as i32
        })
    }

    /// 8x8 transpose of eight 8-lane vectors.
    #[rite(neon)]
    pub(super) fn transpose8(_t: Desktop64, inp: &[__m256i; 8]) -> [__m256i; 8] {
        let mut out = [[0i32; 8]; 8];
        for (r, row) in inp.iter().enumerate() {
            for (c, &val) in row.iter().enumerate() {
                out[c][r] = val;
            }
        }
        out
    }

    #[rite(neon)]
    pub(super) fn load8(_t: Desktop64, buf: &[i32], off: usize) -> __m256i {
        let a: &[i32; 8] = buf[off..off + 8].try_into().unwrap();
        *a
    }

    #[rite(neon)]
    pub(super) fn store8(_t: Desktop64, buf: &mut [i32], off: usize, v: __m256i) {
        buf[off..off + 8].copy_from_slice(&v);
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
        const BITS: i32 = NEW_SQRT2_BITS as i32; // 12
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
    v3::inv_adst(t, input, input_stride, output, out_stride, w, h, col_1d, row_1d, bd)
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
    v3::inv_ext(t, input, input_stride, output, out_stride, w, h, col_1d, row_1d, ud, lr, bd)
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
    v3::inv_4dim(t, input, input_stride, output, out_stride, w, h, col_1d, row_1d, ud, lr, bd)
}
