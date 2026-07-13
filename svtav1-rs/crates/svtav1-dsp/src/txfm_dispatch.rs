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
    fwd_txfm2d_c_exact(input, output, stride, w, h, col_1d, row_1d, ud_flip, lr_flip)
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
    let (col_1d, row_1d) = tx_type_to_1d(tx_type);
    let (w, h) = tx_size_dims(tx_size);
    let (ud_flip, lr_flip) = flip_cfg(tx_type);

    if w > 32 || h > 32 {
        // Remap the top-left 32x32 into a zero-extended w x h buffer,
        // mirroring the mod_input construction in svt_av1_inv_txfm2d_add_64x*.
        let keep_w = w.min(32);
        let keep_h = h.min(32);
        let mut mod_input = alloc::vec![0i32; w * h];
        for r in 0..keep_h {
            for c in 0..keep_w {
                mod_input[r * w + c] = input[r * stride + c];
            }
        }
        inv_txfm2d_c_exact(
            &mod_input, w, output, stride, w, h, row_1d, col_1d, ud_flip, lr_flip,
        )
    } else {
        inv_txfm2d_c_exact(
            input, stride, output, stride, w, h, row_1d, col_1d, ud_flip, lr_flip,
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
fn tx_size_dims(tx_size: TxSize) -> (usize, usize) {
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
