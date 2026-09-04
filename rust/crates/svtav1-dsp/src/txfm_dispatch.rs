//! General transform dispatch — maps (TxSize, TxType) to 2D transform calls.
//!
//! Spec 04: Maps (TxSize, TxType) to 2D transform calls.
//!
//! This is the top-level interface that the encoder uses to select the
//! correct forward and inverse transform for any block size and type.

use crate::fwd_txfm::*;
use crate::inv_txfm::*;
use svtav1_types::transform::{TranLow, TxSize, TxType};

/// Forward 2D transform dispatch for any supported (TxSize, TxType) combination.
///
/// C-exact: per-size cos bits (`fwd_cos_bit_col/row`), C shift tables, and
/// FLIPADST input flips, matching `svt_av1_transform_two_d` + config.
///
/// Returns false if the combination is not supported.
pub fn fwd_txfm2d_dispatch(
    input: &[TranLow],
    output: &mut [TranLow],
    stride: usize,
    tx_size: TxSize,
    tx_type: TxType,
) -> bool {
    let (col_1d, row_1d) = tx_type_to_1d(tx_type);
    let (w, h) = tx_size_dims(tx_size);
    let (ud_flip, lr_flip) = flip_cfg(tx_type);
    fwd_txfm2d_c_exact(
        input, output, stride, w, h, col_1d, row_1d, ud_flip, lr_flip,
    )
}

/// Inverse 2D transform dispatch for any supported (TxSize, TxType) combination.
///
/// C-exact port of the `svt_av1_inv_txfm2d_add_*_c` composition at bd=8,
/// producing residuals instead of adding to base pixels (see
/// `inv_txfm::inv_txfm2d_c_exact`). `input` is in the full-stride layout
/// (`stride` elements per row); for 64-dim sizes only the top-left 32x32
/// region is read — the rest is treated as zero exactly like the C decoder,
/// which never receives those coefficients.
pub fn inv_txfm2d_dispatch(
    input: &[TranLow],
    output: &mut [TranLow],
    stride: usize,
    tx_size: TxSize,
    tx_type: TxType,
) -> bool {
    inv_txfm2d_dispatch_bd(input, output, stride, tx_size, tx_type, 8)
}

/// Bit-depth-aware [`inv_txfm2d_dispatch`] for the bd10 u16 MD path (task #94).
/// `bd == 8` is byte-identical to [`inv_txfm2d_dispatch`]; `bd == 10` widens
/// the inverse-transform row-pass clamp/stage range (see
/// `inv_txfm::inv_txfm2d_c_exact_bd`). Coefficients (i32) are otherwise
/// bit-depth-independent — the caller clips the recon ADD to `bd` separately.
pub fn inv_txfm2d_dispatch_bd(
    input: &[TranLow],
    output: &mut [TranLow],
    stride: usize,
    tx_size: TxSize,
    tx_type: TxType,
    bd: u8,
) -> bool {
    let (col_1d, row_1d) = tx_type_to_1d(tx_type);
    let (w, h) = tx_size_dims(tx_size);
    let (ud_flip, lr_flip) = flip_cfg(tx_type);

    if w > 32 || h > 32 {
        // C's `mod_input` (svt_av1_inv_txfm2d_add_64x*): the top-left 32x32
        // read out of the full-stride layout and zero-extended to w x h —
        // the one transcription, shared with the named 64-dim wrappers.
        let mod_input = crate::inv_txfm::mod_input_64(input, stride, w, h);
        inv_txfm2d_c_exact_bd(
            &mod_input, w, output, stride, w, h, row_1d, col_1d, ud_flip, lr_flip, bd,
        )
    } else {
        inv_txfm2d_c_exact_bd(
            input, stride, output, stride, w, h, row_1d, col_1d, ud_flip, lr_flip, bd,
        )
    }
}

/// C `get_flip_cfg` (inv_transforms.h:139): (ud_flip, lr_flip) per TxType.
pub fn flip_cfg(tx_type: TxType) -> (bool, bool) {
    match tx_type {
        TxType::FlipAdstDct | TxType::FlipAdstAdst | TxType::VFlipAdst => (true, false),
        TxType::DctFlipAdst | TxType::AdstFlipAdst | TxType::HFlipAdst => (false, true),
        TxType::FlipAdstFlipAdst => (true, true),
        _ => (false, false),
    }
}

/// Decompose a 2D TxType into (column_1d_type, row_1d_type).
/// 0=DCT, 1=ADST, 2=FLIPADST, 3=IDENTITY
fn tx_type_to_1d(tx_type: TxType) -> (u8, u8) {
    match tx_type {
        TxType::DctDct => (0, 0),
        TxType::AdstDct => (1, 0),
        TxType::DctAdst => (0, 1),
        TxType::AdstAdst => (1, 1),
        TxType::FlipAdstDct => (2, 0),
        TxType::DctFlipAdst => (0, 2),
        TxType::FlipAdstFlipAdst => (2, 2),
        TxType::AdstFlipAdst => (1, 2),
        TxType::FlipAdstAdst => (2, 1),
        TxType::Idtx => (3, 3),
        TxType::VDct => (0, 3),
        TxType::HDct => (3, 0),
        TxType::VAdst => (1, 3),
        TxType::HAdst => (3, 1),
        TxType::VFlipAdst => (2, 3),
        TxType::HFlipAdst => (3, 2),
    }
}

/// Get (width, height) for a TxSize.
const fn tx_size_dims(tx_size: TxSize) -> (usize, usize) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    #[test]
    fn dispatch_dct_dct_all_square_sizes() {
        for tx_size in [
            TxSize::Tx4x4,
            TxSize::Tx8x8,
            TxSize::Tx16x16,
            TxSize::Tx32x32,
            TxSize::Tx64x64,
        ] {
            let (w, h) = tx_size_dims(tx_size);
            let n = w * h;
            let input = vec![100i32; n];
            let mut fwd_output = vec![0i32; n];
            let ok = fwd_txfm2d_dispatch(&input, &mut fwd_output, w, tx_size, TxType::DctDct);
            assert!(ok, "fwd dispatch failed for {tx_size:?}");
            // DC should be large, AC should be ~0
            assert!(fwd_output[0].abs() > 0, "{tx_size:?} DC should be nonzero");
            for i in 1..n {
                assert!(
                    fwd_output[i].abs() <= 2,
                    "{tx_size:?} AC[{i}]={} should be ~0",
                    fwd_output[i]
                );
            }
        }
    }

    #[test]
    fn dispatch_fwd_inv_4x4_roundtrip_is_identity() {
        // The C-exact inverse produces pixel-domain residuals (the C
        // composition ends with a >>4 round shift before the pixel add), so
        // fwd -> inv through the dispatch reconstructs the input exactly up
        // to rounding. This is stricter than the old relative-scale check:
        // it pins the absolute decoder-facing scale.
        let input: Vec<i32> = (0..16).map(|i| i * 7 - 50).collect();
        let mut fwd = vec![0i32; 16];
        let mut inv = vec![0i32; 16];
        assert!(fwd_txfm2d_dispatch(
            &input,
            &mut fwd,
            4,
            TxSize::Tx4x4,
            TxType::DctDct
        ));
        assert!(inv_txfm2d_dispatch(
            &fwd,
            &mut inv,
            4,
            TxSize::Tx4x4,
            TxType::DctDct
        ));
        for i in 0..16 {
            let diff = (inv[i] - input[i]).abs();
            assert!(
                diff <= 2,
                "roundtrip not identity at {i}: inv={} input={} diff={diff}",
                inv[i],
                input[i]
            );
        }
    }

    #[test]
    fn dispatch_adst_dct_4x4() {
        let input = vec![50i32; 16];
        let mut output = vec![0i32; 16];
        let ok = fwd_txfm2d_dispatch(&input, &mut output, 4, TxSize::Tx4x4, TxType::AdstDct);
        assert!(ok, "ADST-DCT 4x4 should be supported");
    }

    #[test]
    fn dispatch_identity_4x4() {
        let input: Vec<i32> = (0..16).map(|i| i * 10).collect();
        let mut output = vec![0i32; 16];
        let ok = fwd_txfm2d_dispatch(&input, &mut output, 4, TxSize::Tx4x4, TxType::Idtx);
        assert!(ok, "IDTX 4x4 should be supported");
    }

    #[test]
    fn dispatch_rect_4x8() {
        let input = vec![100i32; 32]; // 4x8
        let mut output = vec![0i32; 32];
        let ok = fwd_txfm2d_dispatch(&input, &mut output, 4, TxSize::Tx4x8, TxType::DctDct);
        assert!(ok, "DCT-DCT 4x8 should be supported");
    }

    #[test]
    fn dispatch_all_16_tx_types_4x4() {
        let input = vec![50i32; 16];
        for tx_type in [
            TxType::DctDct,
            TxType::AdstDct,
            TxType::DctAdst,
            TxType::AdstAdst,
            TxType::FlipAdstDct,
            TxType::DctFlipAdst,
            TxType::FlipAdstFlipAdst,
            TxType::AdstFlipAdst,
            TxType::FlipAdstAdst,
            TxType::Idtx,
            TxType::VDct,
            TxType::HDct,
            TxType::VAdst,
            TxType::HAdst,
            TxType::VFlipAdst,
            TxType::HFlipAdst,
        ] {
            let mut output = vec![0i32; 16];
            let ok = fwd_txfm2d_dispatch(&input, &mut output, 4, TxSize::Tx4x4, tx_type);
            assert!(ok, "{tx_type:?} 4x4 should be supported");
        }
    }
}

// =============================================================================
// Inverse transform + reconstruction entry — port of
// `svt_aom_inv_transform_recon8bit` (inv_transforms.c:3138),
// `svt_aom_inv_transform_recon` (:3237), `svt_av1_inv_txfm_add_c` (:3266),
// `highbd_inv_txfm_add` (:3166) and its nineteen per-size arms (:2883-:3137).
// =============================================================================

/// Why an inverse-transform reconstruction could not be performed.
///
/// C's `highbd_inv_txfm_add` reaches these through `assert(0)`, which a
/// release build compiles out — leaving the destination buffer **untouched**
/// rather than reconstructed. That is a plausible-but-wrong block at the
/// integration seam, so this port refuses instead (the repo's "refuse, never
/// emit a plausible-but-wrong stream" rule).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum InvReconError {
    /// `highbd_inv_txfm_add_32x32` (inv_transforms.c:2918) has arms for
    /// `DCT_DCT` and `IDTX` only; every other type hits its `default:
    /// assert(0)`.
    UnsupportedType32x32(TxType),
    /// `highbd_inv_txfm_add_64x64` (:2934) asserts `tx_type == DCT_DCT`.
    /// The 64-dimension rect arms (:3082-:3137) carry no such assert but the
    /// bitstream cannot signal anything else there either.
    UnsupportedType64x64(TxType),
    /// No 1-D kernel exists for the (type, size) pair — `get_inv_txfm_func`
    /// answered `None`. C's `svt_aom_inv_txfm_type_to_func` returns NULL and
    /// `inv_txfm2d_add_c`'s `ASSERT(txfm_func_row)` fires.
    NoKernel(TxSize, TxType),
}

/// C `av1_get_max_eob` (inv_transforms.h:129).
pub const fn max_eob(tx_size: TxSize) -> u32 {
    match tx_size {
        TxSize::Tx64x64 | TxSize::Tx64x32 | TxSize::Tx32x64 => 1024,
        TxSize::Tx16x64 | TxSize::Tx64x16 => 512,
        _ => {
            let (w, h) = tx_size_dims(tx_size);
            (w * h) as u32
        }
    }
}

/// Coefficient count a `tx_size`'s buffer actually holds, for both the packed
/// 64-dimension layout and the dense one. C's 64-dim inverse wrappers
/// (inv_transforms.c:2614-2733) read `min(w, 32)` columns over `min(h, 32)`
/// rows and zero-extend, because `svt_handle_transform*` already repacked the
/// forward output to that shape.
const fn coeff_dims(tx_size: TxSize) -> (usize, usize) {
    let (w, h) = tx_size_dims(tx_size);
    (if w > 32 { 32 } else { w }, if h > 32 { 32 } else { h })
}

/// Inverse transform of `coeff` added onto `pred`, written to `recon`, at
/// `bd`-bit precision — the u16 pixel form.
///
/// This is C's `highbd_inv_txfm_add` (inv_transforms.c:3166) with its
/// nineteen per-size arms folded into one, since every arm differs only in
/// which `svt_av1_inv_txfm2d_add_WxH_c` it names. The two arms that do more
/// than name a size are kept: `TX_4X4`'s lossless branch to the
/// Walsh-Hadamard inverse (`svt_av1_highbd_inv_txfm_add_4x4`, :2883), and the
/// type restrictions at 32x32 / 64x64, which become typed errors.
///
/// `coeff` is the encoder's coefficient buffer at `coeff_stride`: dense
/// `w x h` for every size with no 64 dimension, and packed at 32 columns for
/// those that have one (see [`coeff_dims`]).
///
/// `eob` is read on ONE path only — the lossless 4x4 Walsh-Hadamard, where
/// `highbd_iwht4x4_add` (:2874) picks its 16-coefficient or DC-only kernel
/// from it. Every other arm in C ignores `txfm_param->eob` entirely.
#[allow(clippy::too_many_arguments)]
pub fn highbd_inv_txfm_add(
    coeff: &[TranLow],
    coeff_stride: usize,
    pred: &[u16],
    pred_stride: usize,
    recon: &mut [u16],
    recon_stride: usize,
    tx_size: TxSize,
    tx_type: TxType,
    eob: u32,
    lossless: bool,
    bd: u8,
) -> Result<(), InvReconError> {
    let (w, h) = tx_size_dims(tx_size);

    if lossless && tx_size == TxSize::Tx4x4 {
        // C asserts `tx_type == DCT_DCT` here and then ignores it.
        crate::inv_txfm::highbd_iwht4x4_add(
            coeff,
            pred,
            pred_stride,
            recon,
            recon_stride,
            eob as i32,
            bd,
        );
        return Ok(());
    }

    match (w, h, tx_type) {
        (32, 32, TxType::DctDct | TxType::Idtx) => {}
        (32, 32, other) => return Err(InvReconError::UnsupportedType32x32(other)),
        (64, 64, TxType::DctDct) => {}
        (64, 64, other) => return Err(InvReconError::UnsupportedType64x64(other)),
        _ => {}
    }

    // The 2-D inverse, as residuals; `inv_txfm2d_c_exact_bd` has already
    // applied C's `HIGHBD_WRAPLOW` to each one, so all that remains of
    // `highbd_clip_pixel_add` (inv_transforms.c:2443) is the pixel clip.
    let (cw, ch) = coeff_dims(tx_size);
    let mut dense = alloc::vec![0i32; w * h];
    for r in 0..ch {
        for c in 0..cw {
            dense[r * w + c] = coeff[r * coeff_stride + c];
        }
    }
    let mut residual = alloc::vec![0i32; w * h];
    let (col_1d, row_1d) = tx_type_to_1d(tx_type);
    let (ud_flip, lr_flip) = flip_cfg(tx_type);
    if !inv_txfm2d_c_exact_bd(
        &dense,
        w,
        &mut residual,
        w,
        w,
        h,
        row_1d,
        col_1d,
        ud_flip,
        lr_flip,
        bd,
    ) {
        return Err(InvReconError::NoKernel(tx_size, tx_type));
    }

    for r in 0..h {
        for c in 0..w {
            recon[r * recon_stride + c] = crate::hbd::clip_pixel_highbd(
                pred[r * pred_stride + c] as i32 + residual[r * w + c],
                bd,
            );
        }
    }
    Ok(())
}

/// Port of C `svt_aom_inv_transform_recon` (inv_transforms.c:3237) — the
/// encoder's high-bit-depth reconstruction entry.
///
/// **C's `eob` argument is not taken here, and that is the translation, not
/// an omission.** C forces `txfm_param.eob = av1_get_max_eob(txsize)`
/// whenever `recon_buffer_r != recon_buffer_w` (:3251-3255, "cannot be
/// limited by End Of Buffer calculations"). Separate `&[u16]` and
/// `&mut [u16]` cannot alias in Rust, so that branch is the only reachable
/// one through this signature and the forced value is supplied here. The
/// aliasing form C also serves — TPL's `dst_buffer` for both
/// (src_ops_process.c:1142) — is [`inv_transform_recon_in_place`], which
/// does take an `eob`.
///
/// `component_type` is `UNUSED` in C and has no counterpart.
#[allow(clippy::too_many_arguments)]
pub fn inv_transform_recon(
    coeff: &[TranLow],
    coeff_stride: usize,
    pred: &[u16],
    pred_stride: usize,
    recon: &mut [u16],
    recon_stride: usize,
    tx_size: TxSize,
    tx_type: TxType,
    lossless: bool,
    bd: u8,
) -> Result<(), InvReconError> {
    highbd_inv_txfm_add(
        coeff,
        coeff_stride,
        pred,
        pred_stride,
        recon,
        recon_stride,
        tx_size,
        tx_type,
        max_eob(tx_size),
        lossless,
        bd,
    )
}

/// [`inv_transform_recon`] where C's read and write pointers are the SAME
/// buffer, so the caller's `eob` survives (`svt_aom_inv_transform_recon`
/// :3251 does not rewrite it).
///
/// Reachability of the difference, measured: `eob` is consumed on exactly one
/// path, `highbd_iwht4x4_add`'s choice between the 16-coefficient and DC-only
/// Walsh-Hadamard kernels, which needs `lossless`. Of C's two callers, the
/// mode-decision wrapper (full_loop.c:1915) passes distinct pred/recon
/// buffers and TPL (src_ops_process.c:1142) aliases but passes `lossless =
/// 0`. So no shipping C call site reaches
/// `svt_av1_highbd_iwht4x4_1_add_c` through this entry — it is reachable only
/// as `lossless && aliased && eob <= 1`, which this function exposes and the
/// parity test drives directly.
#[allow(clippy::too_many_arguments)]
pub fn inv_transform_recon_in_place(
    coeff: &[TranLow],
    coeff_stride: usize,
    recon: &mut [u16],
    recon_stride: usize,
    tx_size: TxSize,
    tx_type: TxType,
    eob: u32,
    lossless: bool,
    bd: u8,
) -> Result<(), InvReconError> {
    let (w, h) = tx_size_dims(tx_size);
    let pred: alloc::vec::Vec<u16> = (0..h)
        .flat_map(|r| recon[r * recon_stride..r * recon_stride + w].to_vec())
        .collect();
    highbd_inv_txfm_add(
        coeff,
        coeff_stride,
        &pred,
        w,
        recon,
        recon_stride,
        tx_size,
        tx_type,
        eob,
        lossless,
        bd,
    )
}

/// Port of C `svt_aom_inv_transform_recon8bit` (inv_transforms.c:3138) with
/// distinct read/write buffers.
///
/// C reaches the same arithmetic as [`inv_transform_recon`] by a longer road:
/// it sets `bd = 8` and calls `svt_av1_inv_txfm_add` (`_c` at :3266), which
/// widens the u8 destination into a `uint16_t tmp[MAX_TX_SQUARE]` at stride
/// `MAX_TX_SIZE`, runs `highbd_inv_txfm_add` on that scratch **in place**,
/// and narrows back to u8. The staging is byte-inert — every value written
/// there has already been clipped to `0..=255` by `clip_pixel_highbd(.., 8)`
/// — so this port widens and narrows around the same call without
/// materialising a 64-stride scratch.
///
/// No `eob`: see [`inv_transform_recon`].
#[allow(clippy::too_many_arguments)]
pub fn inv_transform_recon8bit(
    coeff: &[TranLow],
    coeff_stride: usize,
    pred: &[u8],
    pred_stride: usize,
    recon: &mut [u8],
    recon_stride: usize,
    tx_size: TxSize,
    tx_type: TxType,
    lossless: bool,
) -> Result<(), InvReconError> {
    recon8bit_impl(
        coeff,
        coeff_stride,
        pred,
        pred_stride,
        recon,
        recon_stride,
        tx_size,
        tx_type,
        max_eob(tx_size),
        lossless,
    )
}

/// [`inv_transform_recon8bit`] where C's read and write pointers are the SAME
/// buffer, so the caller's `eob` survives. `recon` holds the prediction on
/// entry and the reconstruction on return. See
/// [`inv_transform_recon_in_place`] for what the surviving `eob` reaches.
#[allow(clippy::too_many_arguments)]
pub fn inv_transform_recon8bit_in_place(
    coeff: &[TranLow],
    coeff_stride: usize,
    recon: &mut [u8],
    recon_stride: usize,
    tx_size: TxSize,
    tx_type: TxType,
    eob: u32,
    lossless: bool,
) -> Result<(), InvReconError> {
    let (w, h) = tx_size_dims(tx_size);
    let pred: alloc::vec::Vec<u8> = (0..h)
        .flat_map(|r| recon[r * recon_stride..r * recon_stride + w].to_vec())
        .collect();
    recon8bit_impl(
        coeff,
        coeff_stride,
        &pred,
        w,
        recon,
        recon_stride,
        tx_size,
        tx_type,
        eob,
        lossless,
    )
}

#[allow(clippy::too_many_arguments)]
fn recon8bit_impl(
    coeff: &[TranLow],
    coeff_stride: usize,
    pred: &[u8],
    pred_stride: usize,
    recon: &mut [u8],
    recon_stride: usize,
    tx_size: TxSize,
    tx_type: TxType,
    eob: u32,
    lossless: bool,
) -> Result<(), InvReconError> {
    let (w, h) = tx_size_dims(tx_size);
    let mut pred16 = alloc::vec![0u16; w * h];
    for r in 0..h {
        for c in 0..w {
            pred16[r * w + c] = u16::from(pred[r * pred_stride + c]);
        }
    }
    let mut recon16 = alloc::vec![0u16; w * h];
    highbd_inv_txfm_add(
        coeff,
        coeff_stride,
        &pred16,
        w,
        &mut recon16,
        w,
        tx_size,
        tx_type,
        eob,
        lossless,
        8,
    )?;
    for r in 0..h {
        for c in 0..w {
            recon[r * recon_stride + c] = recon16[r * w + c] as u8;
        }
    }
    Ok(())
}
