//! Encoding loop — the core predict→transform→quantize→entropy→reconstruct cycle.
//!
//! Spec 10: Predict/transform/quantize/reconstruct cycle.
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
///
/// `qindex` is the AV1 qindex (0..255, the frame header's base_q_idx) —
/// NOT the CLI 0..63 qp. It indexes the 256-entry DC/AC step tables
/// directly; CLI-domain callers must convert via
/// `rate_control::qp_to_qindex` first.
pub fn encode_block(
    src: &[u8],
    src_stride: usize,
    pred: &[u8],
    pred_stride: usize,
    width: usize,
    height: usize,
    qindex: u8,
) -> EncodeBlockResult {
    encode_block_tx(
        src,
        src_stride,
        pred,
        pred_stride,
        width,
        height,
        qindex,
        svtav1_types::transform::TxType::DctDct,
    )
}

/// Encode a block with a specific transform type.
/// (Spec 04: "16 transform types combine row and column 1D transforms")
///
/// `qindex`: AV1 qindex 0..255 — see [`encode_block`].
pub fn encode_block_tx(
    src: &[u8],
    src_stride: usize,
    pred: &[u8],
    pred_stride: usize,
    width: usize,
    height: usize,
    qindex: u8,
    tx_type: svtav1_types::transform::TxType,
) -> EncodeBlockResult {
    encode_block_tx_cq(
        src,
        src_stride,
        pred,
        pred_stride,
        width,
        height,
        qindex,
        tx_type,
        None,
        0,
        15,
    )
}

/// [`encode_block_tx`] with an optional C-exact coding quantizer.
///
/// `cq = None` keeps the legacy dead-zone quantizer. `cq = Some(cfg)`
/// quantizes exactly like C's still/MDS3 path (`quant.rs`): plain
/// `svt_aom_quantize_b` at rdoq_level 0, else `quantize_fp` + the
/// `svt_av1_optimize_b` trellis. `plane_type` selects the luma/chroma
/// cost tables and `plane_rd_mult` row (0 = Y, 1 = UV).
#[allow(clippy::too_many_arguments)]
pub fn encode_block_tx_cq(
    src: &[u8],
    src_stride: usize,
    pred: &[u8],
    pred_stride: usize,
    width: usize,
    height: usize,
    qindex: u8,
    tx_type: svtav1_types::transform::TxType,
    cq: Option<&crate::quant::CodingQuantCfg>,
    plane_type: usize,
    qm_level: u8,
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

    // Step 3: Quantize. Two paths, both keeping the decoder's dequant
    // mirror dq = (level * dqv) >> tx_scale exact so reconstruction always
    // matches what the decoder will build:
    //
    // - cq = Some: the C-exact coding quantizer (quant.rs) — the exact
    //   MDS3/still path of `svt_aom_quantize_inv_quantize`
    //   (quantize_b, or quantize_fp + the optimize_b RDOQ trellis).
    // - cq = None: the legacy dead-zone quantizer (homegrown paths).
    let mut qcoeffs = alloc::vec![0i32; n];
    let mut dqcoeffs = alloc::vec![0i32; n];
    let mut eob: u16 = 0;

    if let Some(cfg) = cq {
        // C operates on the ADJUSTED (32-capped) packed coefficient block
        // — pack the top-left quadrant for 64-dim transforms exactly like
        // svt_handle_transform64x64/64x32 leaves it (values unchanged).
        let c_tx = svtav1_entropy::coeff_c::tx_size_from_dims(width, height);
        let packed_w = width.min(32);
        let packed_h = height.min(32);
        let packed: alloc::vec::Vec<i32> = if packed_w != width || packed_h != height {
            let mut v = alloc::vec![0i32; packed_w * packed_h];
            for r in 0..packed_h {
                v[r * packed_w..(r + 1) * packed_w]
                    .copy_from_slice(&coeffs[r * width..r * width + packed_w]);
            }
            v
        } else {
            coeffs.clone()
        };
        let scan = svtav1_entropy::scan_tables::scan(
            c_tx,
            svtav1_entropy::scan_tables::TX_TYPE_TO_SCAN_INDEX[tx_type as usize] as usize,
        );
        let tx_class = svtav1_entropy::coeff_c::TX_TYPE_TO_CLASS[tx_type as usize];
        let mut pq = alloc::vec![0i32; packed_w * packed_h];
        let mut pdq = alloc::vec![0i32; packed_w * packed_h];
        crate::quant::quantize_inv_quantize_still(
            cfg,
            &packed,
            &mut pq,
            &mut pdq,
            scan,
            qindex,
            c_tx,
            tx_class,
            plane_type,
            (width * height) as u32,
            tx_type as usize == 9, // TX_TYPE IDTX (identity)
            // QM applies to 2D transforms only (IS_2D_TRANSFORM = < IDTX).
            if (tx_type as usize) < 9 { qm_level } else { 15 },
        );
        // Unpack into the full raster (zeros outside the kept quadrant)
        // and derive the raster-domain eob the rest of this function uses
        // (the tile writer recomputes the scan-domain eob itself).
        for r in 0..packed_h {
            for c in 0..packed_w {
                let q = pq[r * packed_w + c];
                qcoeffs[r * width + c] = q;
                dqcoeffs[r * width + c] = pdq[r * packed_w + c];
                if q != 0 {
                    eob = (r * width + c + 1) as u16;
                }
            }
        }
    } else {
        // Legacy dead-zone quantization against the decoder-visible step.
        let dequant_dc = svtav1_dsp::quant_tables::DC_QLOOKUP_8[qindex as usize] as i32;
        let dequant_ac = svtav1_dsp::quant_tables::AC_QLOOKUP_8[qindex as usize] as i32;
        // av1_get_tx_scale: 0 for <=256 pels, 1 for 1024, 2 for >1024.
        let pels = (width * height) as i32;
        let tx_scale = i32::from(pels > 256) + i32::from(pels > 1024);

        for i in 0..n {
            let dqv = if i == 0 { dequant_dc } else { dequant_ac };
            let sign = if coeffs[i] < 0 { -1i32 } else { 1 };
            let abs_scaled = (coeffs[i].abs() as i64) << tx_scale;
            let q = ((abs_scaled + i64::from(dqv) / 2) / i64::from(dqv)) as i32;
            qcoeffs[i] = sign * q;
            // Mirror of the decoder: dq = (level * dqv) >> tx_scale.
            dqcoeffs[i] = sign * (((q as i64 * i64::from(dqv)) >> tx_scale) as i32);
            if q > 0 {
                eob = (i + 1) as u16;
            }
        }

        // 64-dim transforms: the EC layer transmits only the top-left
        // 32x32 coefficients (AV1 caps coefficient coding at 32x32), so
        // anything outside that region never reaches the decoder. Zero it
        // here so the encoder reconstruction cannot include energy the
        // decoder never sees, and recompute eob over the survivors.
        if width > 32 || height > 32 {
            let keep_w = width.min(32);
            let keep_h = height.min(32);
            for row in 0..height {
                for col in 0..width {
                    if row >= keep_h || col >= keep_w {
                        let idx = row * width + col;
                        qcoeffs[idx] = 0;
                        dqcoeffs[idx] = 0;
                    }
                }
            }
            eob = 0;
            for i in 0..n {
                if qcoeffs[i] != 0 {
                    eob = (i + 1) as u16;
                }
            }
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
