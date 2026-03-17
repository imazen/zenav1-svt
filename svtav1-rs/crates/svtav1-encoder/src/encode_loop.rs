//! Encoding loop — the core predict→transform→quantize→entropy→reconstruct cycle.
//!
//! Ported from SVT-AV1's `coding_loop.c` and `enc_dec_process.c`.

use svtav1_types::transform::TranLow;

/// Result of encoding a single block.
#[derive(Debug, Clone)]
pub struct EncodeBlockResult {
    /// Quantized transform coefficients.
    pub qcoeffs: alloc::vec::Vec<TranLow>,
    /// Reconstructed pixels.
    pub recon: alloc::vec::Vec<u8>,
    /// Number of non-zero coefficients (end of block).
    pub eob: u16,
    /// Distortion (SSE between source and reconstruction).
    pub distortion: u64,
    /// Rate in bits (estimated).
    pub rate: u32,
}

/// Encode a single block: predict → residual → transform → quantize → reconstruct.
///
/// This is the innermost loop of the encoder.
/// Uses DCT-DCT transform type. For RDO TX selection, use `encode_block_tx`.
/// (Spec 10: "The encode-decode cycle applies forward transform, quantization,
/// inverse transform, and reconstruction")
pub fn encode_block(
    src: &[u8],
    src_stride: usize,
    pred: &[u8],
    pred_stride: usize,
    width: usize,
    height: usize,
    qp: u8,
) -> EncodeBlockResult {
    encode_block_tx(
        src,
        src_stride,
        pred,
        pred_stride,
        width,
        height,
        qp,
        svtav1_types::transform::TxType::DctDct,
    )
}

/// Encode a block with a specific transform type.
/// (Spec 04: "16 transform types combine row and column 1D transforms")
pub fn encode_block_tx(
    src: &[u8],
    src_stride: usize,
    pred: &[u8],
    pred_stride: usize,
    width: usize,
    height: usize,
    qp: u8,
    tx_type: svtav1_types::transform::TxType,
) -> EncodeBlockResult {
    let n = width * height;

    // Step 1: Compute residual (src - pred)
    let mut residual = alloc::vec![0i32; n];
    for row in 0..height {
        for col in 0..width {
            residual[row * width + col] =
                src[row * src_stride + col] as i32 - pred[row * pred_stride + col] as i32;
        }
    }

    // Step 2: Forward transform using the specified TxType.
    // For DCT-DCT on common sizes, use optimized wrappers with SIMD dispatch.
    // For other types or sizes, use the general TxType dispatch.
    // (Spec 04: "the forward transform converts residual to frequency domain")
    let mut coeffs = alloc::vec![0i32; n];
    let use_optimized = tx_type == svtav1_types::transform::TxType::DctDct;
    if use_optimized {
        match (width, height) {
            (4, 4) => svtav1_dsp::fwd_txfm::fwd_txfm2d_4x4_dct_dct(&residual, &mut coeffs, width),
            (8, 8) => svtav1_dsp::fwd_txfm::fwd_txfm2d_8x8_dct_dct(&residual, &mut coeffs, width),
            (16, 16) => {
                svtav1_dsp::fwd_txfm::fwd_txfm2d_16x16_dct_dct(&residual, &mut coeffs, width)
            }
            (32, 32) => {
                svtav1_dsp::fwd_txfm::fwd_txfm2d_32x32_dct_dct(&residual, &mut coeffs, width)
            }
            _ => {
                let tx_size = size_to_tx_size(width, height);
                svtav1_dsp::txfm_dispatch::fwd_txfm2d_dispatch(
                    &residual,
                    &mut coeffs,
                    width,
                    tx_size,
                    tx_type,
                );
            }
        }
    } else {
        // Non-DCT-DCT types go through general dispatch
        let tx_size = size_to_tx_size(width, height);
        if !svtav1_dsp::txfm_dispatch::fwd_txfm2d_dispatch(
            &residual,
            &mut coeffs,
            width,
            tx_size,
            tx_type,
        ) {
            // Unsupported type — fall back to DCT-DCT
            svtav1_dsp::txfm_dispatch::fwd_txfm2d_dispatch(
                &residual,
                &mut coeffs,
                width,
                tx_size,
                svtav1_types::transform::TxType::DctDct,
            );
        }
    }

    // Step 3: Quantize
    let dequant_dc = qp_to_dequant(qp, true) as i32;
    let dequant_ac = qp_to_dequant(qp, false) as i32;

    let mut qcoeffs = alloc::vec![0i32; n];
    let mut dqcoeffs = alloc::vec![0i32; n];
    let mut eob: u16 = 0;

    for i in 0..n {
        let dequant = if i == 0 { dequant_dc } else { dequant_ac };
        if dequant == 0 {
            continue;
        }
        // Dead-zone quantization
        let sign = if coeffs[i] < 0 { -1i32 } else { 1 };
        let abs_coeff = coeffs[i].abs();
        let q = (abs_coeff + dequant / 2) / dequant;
        qcoeffs[i] = sign * q;
        dqcoeffs[i] = qcoeffs[i] * dequant;
        if q > 0 {
            eob = (i + 1) as u16;
        }
    }

    // Step 4: Inverse transform — must match the forward transform type.
    // (Spec 10: "the inverse transform type must match the forward type
    // signaled in the bitstream")
    let mut inv_residual = alloc::vec![0i32; n];
    if use_optimized {
        match (width, height) {
            (4, 4) => {
                svtav1_dsp::inv_txfm::inv_txfm2d_4x4_dct_dct(&dqcoeffs, &mut inv_residual, width)
            }
            (8, 8) => {
                svtav1_dsp::inv_txfm::inv_txfm2d_8x8_dct_dct(&dqcoeffs, &mut inv_residual, width)
            }
            (16, 16) => {
                svtav1_dsp::inv_txfm::inv_txfm2d_16x16_dct_dct(&dqcoeffs, &mut inv_residual, width)
            }
            (32, 32) => {
                svtav1_dsp::inv_txfm::inv_txfm2d_32x32_dct_dct(&dqcoeffs, &mut inv_residual, width)
            }
            _ => {
                let tx_size = size_to_tx_size(width, height);
                svtav1_dsp::txfm_dispatch::inv_txfm2d_dispatch(
                    &dqcoeffs,
                    &mut inv_residual,
                    width,
                    tx_size,
                    tx_type,
                );
            }
        }
    } else {
        let tx_size = size_to_tx_size(width, height);
        if !svtav1_dsp::txfm_dispatch::inv_txfm2d_dispatch(
            &dqcoeffs,
            &mut inv_residual,
            width,
            tx_size,
            tx_type,
        ) {
            svtav1_dsp::txfm_dispatch::inv_txfm2d_dispatch(
                &dqcoeffs,
                &mut inv_residual,
                width,
                tx_size,
                svtav1_types::transform::TxType::DctDct,
            );
        }
    }

    // Step 5: Reconstruct (pred + inv_residual, clipped to [0, 255])
    let mut recon = alloc::vec![0u8; n];
    let mut distortion: u64 = 0;
    for row in 0..height {
        for col in 0..width {
            let idx = row * width + col;
            let p = pred[row * pred_stride + col] as i32;
            let r = (p + inv_residual[idx]).clamp(0, 255) as u8;
            recon[idx] = r;
            let diff = src[row * src_stride + col] as i32 - r as i32;
            distortion += (diff * diff) as u64;
        }
    }

    // Step 6: Estimate rate from non-zero coefficients
    let rate = estimate_coeff_rate(&qcoeffs, eob);

    EncodeBlockResult {
        qcoeffs,
        recon,
        eob,
        distortion,
        rate,
    }
}

/// Map (width, height) to the closest TxSize.
fn size_to_tx_size(width: usize, height: usize) -> svtav1_types::transform::TxSize {
    use svtav1_types::transform::TxSize;
    match (width, height) {
        (4, 4) => TxSize::Tx4x4,
        (8, 8) => TxSize::Tx8x8,
        (16, 16) => TxSize::Tx16x16,
        (32, 32) => TxSize::Tx32x32,
        (64, 64) => TxSize::Tx64x64,
        (4, 8) => TxSize::Tx4x8,
        (8, 4) => TxSize::Tx8x4,
        (8, 16) => TxSize::Tx8x16,
        (16, 8) => TxSize::Tx16x8,
        (16, 32) => TxSize::Tx16x32,
        (32, 16) => TxSize::Tx32x16,
        (32, 64) => TxSize::Tx32x64,
        (64, 32) => TxSize::Tx64x32,
        (4, 16) => TxSize::Tx4x16,
        (16, 4) => TxSize::Tx16x4,
        (8, 32) => TxSize::Tx8x32,
        (32, 8) => TxSize::Tx32x8,
        (16, 64) => TxSize::Tx16x64,
        (64, 16) => TxSize::Tx64x16,
        _ => TxSize::Tx4x4, // fallback
    }
}

/// Convert QP to dequantization step size (simplified).
fn qp_to_dequant(qp: u8, is_dc: bool) -> u16 {
    // Simplified: real AV1 uses a lookup table indexed by qindex
    let base = 4 + qp as u16 * 2;
    if is_dc { base } else { base + 2 }
}

/// Estimate rate from quantized coefficients (simplified).
fn estimate_coeff_rate(qcoeffs: &[TranLow], eob: u16) -> u32 {
    if eob == 0 {
        return 64; // Skip flag only
    }
    let mut bits: u32 = 128; // EOB signaling overhead
    for &c in &qcoeffs[..eob as usize] {
        if c == 0 {
            bits += 64; // Zero run
        } else if c.abs() == 1 {
            bits += 256; // Level 1
        } else {
            bits += 384 + c.unsigned_abs().ilog2() * 128; // Higher levels
        }
    }
    bits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_uniform_block() {
        // Source and prediction are identical → zero residual → all zero coefficients
        let src = [128u8; 16];
        let pred = [128u8; 16];
        let result = encode_block(&src, 4, &pred, 4, 4, 4, 30);
        assert_eq!(result.eob, 0);
        assert_eq!(result.distortion, 0);
    }

    #[test]
    fn encode_small_residual() {
        let src = [130u8; 16];
        let pred = [128u8; 16];
        let result = encode_block(&src, 4, &pred, 4, 4, 4, 30);
        // Small residual should result in low distortion
        assert!(result.distortion < 100 * 16); // Less than 10 per pixel
    }

    #[test]
    fn encode_large_residual() {
        let src = [255u8; 16];
        let pred = [0u8; 16];
        let result = encode_block(&src, 4, &pred, 4, 4, 4, 20);
        // Large residual should have non-zero EOB
        assert!(result.eob > 0);
        // Reconstruction should be close to source
        for &r in &result.recon {
            assert!(r > 200, "recon pixel {r} too far from 255");
        }
    }

    #[test]
    fn encode_preserves_sign() {
        let src = [0u8; 16];
        let pred = [128u8; 16];
        let result = encode_block(&src, 4, &pred, 4, 4, 4, 20);
        // Reconstruction should be closer to 0 than to 128
        for &r in &result.recon {
            assert!(r < 100, "recon pixel {r} should be close to 0");
        }
    }

    #[test]
    fn rate_zero_block() {
        let rate = estimate_coeff_rate(&[0i32; 16], 0);
        assert!(rate < 256); // Very cheap
    }
}
