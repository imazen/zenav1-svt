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
/// Returns false if the combination is not supported.
pub fn fwd_txfm2d_dispatch(
    input: &[TranLow],
    output: &mut [TranLow],
    stride: usize,
    tx_size: TxSize,
    tx_type: TxType,
) -> bool {
    // Decompose TxType into (col_type_1d, row_type_1d)
    let (col_1d, row_1d) = tx_type_to_1d(tx_type);

    // Get block dimensions
    let (w, h) = tx_size_dims(tx_size);

    // Get 1D functions
    let col_func = match get_fwd_txfm_func(col_1d, h) {
        Some(f) => f,
        None => return false,
    };
    let row_func = match get_fwd_txfm_func(row_1d, w) {
        Some(f) => f,
        None => return false,
    };

    // Get shift values
    let shift = fwd_shift(tx_size);

    if w == h {
        fwd_txfm2d(input, output, stride, col_func, row_func, w, shift);
    } else {
        fwd_txfm2d_rect(input, output, stride, col_func, row_func, w, h, shift);
    }
    true
}

/// Inverse 2D transform dispatch for any supported (TxSize, TxType) combination.
pub fn inv_txfm2d_dispatch(
    input: &[TranLow],
    output: &mut [TranLow],
    stride: usize,
    tx_size: TxSize,
    tx_type: TxType,
) -> bool {
    let (col_1d, row_1d) = tx_type_to_1d(tx_type);
    let (w, h) = tx_size_dims(tx_size);

    let row_func = match get_inv_txfm_func(row_1d, w) {
        Some(f) => f,
        None => return false,
    };
    let col_func = match get_inv_txfm_func(col_1d, h) {
        Some(f) => f,
        None => return false,
    };

    let shift = inv_shift(tx_size);

    if w == h {
        inv_txfm2d(input, output, stride, row_func, col_func, w, shift);
    } else {
        inv_txfm2d_rect(input, output, stride, row_func, col_func, w, h, shift);
    }
    true
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

/// Forward transform shift values for each TxSize (8-bit content).
fn fwd_shift(tx_size: TxSize) -> [i32; 3] {
    match tx_size {
        TxSize::Tx4x4 => [2, 0, 0],
        TxSize::Tx8x8 => [2, -1, 0],
        TxSize::Tx16x16 => [2, -2, 0],
        TxSize::Tx32x32 => [2, -4, 0],
        TxSize::Tx64x64 => [2, -6, 0],
        TxSize::Tx4x8 | TxSize::Tx8x4 => [2, 0, 0],
        TxSize::Tx8x16 | TxSize::Tx16x8 => [2, -1, 0],
        TxSize::Tx16x32 | TxSize::Tx32x16 => [2, -2, 0],
        TxSize::Tx32x64 | TxSize::Tx64x32 => [2, -4, 0],
        TxSize::Tx4x16 | TxSize::Tx16x4 => [2, 0, 0],
        TxSize::Tx8x32 | TxSize::Tx32x8 => [2, -1, 0],
        TxSize::Tx16x64 | TxSize::Tx64x16 => [2, -2, 0],
    }
}

/// Inverse transform shift values for each TxSize (8-bit content).
fn inv_shift(tx_size: TxSize) -> [i32; 2] {
    match tx_size {
        TxSize::Tx4x4 => [0, 0],
        TxSize::Tx8x8 => [-1, 0],
        TxSize::Tx16x16 => [-2, 0],
        TxSize::Tx32x32 => [-4, 0],
        TxSize::Tx64x64 => [-6, 0],
        TxSize::Tx4x8 | TxSize::Tx8x4 => [0, 0],
        TxSize::Tx8x16 | TxSize::Tx16x8 => [-1, 0],
        TxSize::Tx16x32 | TxSize::Tx32x16 => [-2, 0],
        TxSize::Tx32x64 | TxSize::Tx64x32 => [-4, 0],
        TxSize::Tx4x16 | TxSize::Tx16x4 => [0, 0],
        TxSize::Tx8x32 | TxSize::Tx32x8 => [-1, 0],
        TxSize::Tx16x64 | TxSize::Tx64x16 => [-2, 0],
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
    fn dispatch_fwd_inv_4x4_preserves_relative_values() {
        // AV1 transforms have a built-in scale factor:
        // fwd pre-shift * 1D scale^2 = 4 * 2 * 2 = 16 for 4x4
        // We verify the roundtrip preserves relative values (ratios)
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
        // All output should be a consistent scale of input
        // Find scale from first nonzero element
        let first_nonzero = input.iter().position(|&x| x != 0).unwrap();
        let scale = inv[first_nonzero] as f64 / input[first_nonzero] as f64;
        assert!(scale.abs() > 1.0, "scale should be > 1: {scale}");
        for i in 0..16 {
            if input[i] != 0 {
                let actual_scale = inv[i] as f64 / input[i] as f64;
                let diff = (actual_scale - scale).abs();
                assert!(
                    diff < 0.5,
                    "inconsistent scale at {i}: {actual_scale} vs {scale}"
                );
            }
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
